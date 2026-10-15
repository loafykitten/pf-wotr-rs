//! Components that add facts (decompiled, not copied): `AddFacts`
//! (`Kingmaker.UnitLogic.FactLogic/AddFacts.cs`), `AddFeatureToNPC`
//! (`Kingmaker.Designers.Mechanics.Facts/AddFeatureToNPC.cs`), `FactSinglify`
//! (`Kingmaker.Designers.Mechanics.Facts/FactSinglify.cs`), `AddFeatureOnAlignment`
//! (`Kingmaker.Designers.Mechanics.Facts/AddFeatureOnAlignment.cs`), `AddFeatureOnSkill`
//! (`Kingmaker.Designers.Mechanics.Facts/AddFeatureOnSkill.cs`), `BuffExtraEffects`
//! (`Kingmaker.Designers.Mechanics.Facts/BuffExtraEffects.cs`), `BuffOnEntityCreated`
//! (`Kingmaker.Designers.TempMapCode.Ambush/BuffOnEntityCreated.cs`: its buff's unique id in
//! the component's data, `BuffId`).
//!
//! Each keeps its applied facts in its runtime data, as the C# does (`AddFactsData.
//! AppliedFacts`, `AddFeatureToNPCData.AppliedFact`, `FactSinglifyData.AppliedFacts`,
//! `AddFeatureOnAlignmentData.AppliedFacts`, `AddFeatureOnSkillData.AppliedFacts`,
//! `BuffExtraEffectsData.AppliedBuff`), in
//! the component's `fact_refs`.

use super::{Port, bits, owner_unit, with_settings};
use crate::registry::{Hook, Registry};
use crate::world::{CompRef, EntityIdx, FactIdx, World};

pub fn register(r: &mut Registry) {
    use Hook::{Activate, ApplyPostLoadFixes, Deactivate, Recalculate};
    r.register(Box::new(Port::new(
        "AddFacts",
        bits(&[Activate, Deactivate, Recalculate, ApplyPostLoadFixes]),
        add_facts,
    )));
    r.register(Box::new(Port::new(
        "AddFeatureToNPC",
        bits(&[Activate, Deactivate]),
        add_feature_to_npc,
    )));
    r.register(Box::new(Port::new("FactSinglify", bits(&[Activate, Deactivate]), fact_singlify)));
    let mut p = Port::new(
        "AddFeatureOnAlignment",
        bits(&[Activate, Deactivate, ApplyPostLoadFixes]),
        add_feature_on_alignment,
    );
    p.handlers = &["IAlignmentChangeHandler"];
    p.ev = Some(add_feature_on_alignment_event);
    r.register(Box::new(p));
    let mut p = Port::new(
        "BuffExtraEffects",
        bits(&[Activate, Deactivate, ApplyPostLoadFixes]),
        buff_extra_effects,
    );
    p.handlers = &["IUnitBuffHandler"];
    p.ev = Some(buff_extra_effects_event);
    r.register(Box::new(p));
    r.register(Box::new(Port::new(
        "BuffOnEntityCreated",
        bits(&[Activate, Deactivate]),
        buff_on_entity_created,
    )));
    let mut p = Port::new("AddFeatureOnSkill", bits(&[Activate, Deactivate]), add_feature_on_skill);
    p.handlers = &["IOwnerGainLevelHandler"];
    p.ev = Some(add_feature_on_skill_event);
    r.register(Box::new(p));
}

/// `AddFeatureOnSkill.OnActivate` / `OnDeactivate`.
fn add_feature_on_skill(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    match h {
        Hook::Activate => add_feature_on_skill_apply(w, c, u),
        // `Data.AppliedFacts.ForEach(Owner.RemoveFact)`, then `Clear()`.
        Hook::Deactivate => {
            for f in applied(w, c, "AppliedFacts") {
                remove_fact(w, u, f);
            }
            set_applied(w, c, "AppliedFacts", Vec::new());
        }
        _ => {}
    }
}

/// `IOwnerGainLevelHandler.HandleUnitGainLevel` (raised for the owner): as `OnActivate`.
fn add_feature_on_skill_event(w: &mut World, c: CompRef, _iface: &str) {
    if let Some(u) = owner_unit(w, c) {
        add_feature_on_skill_apply(w, c, u);
    }
}

/// With the stat's `PermanentValue` at least `MinimalStat`, each listed fact the owner doesn't
/// have is added (`Data.AppliedFacts.Add(Owner.AddFact(fact))`, nulls included).
fn add_feature_on_skill_apply(w: &mut World, c: CompRef, u: EntityIdx) {
    let Some((stat, minimal, facts)) = with_settings(w, c, |v| {
        (v.i32("StatType"), v.i32("MinimalStat"), v.guids_with_nulls("m_Facts"))
    }) else {
        return;
    };
    let Some(permanent) = w
        .unit(u)
        .and_then(|x| x.stats.get(crate::stats::types::StatType(stat)))
        .map(|s| s.permanent_value)
    else {
        w.coverage.unsupported("AddFeatureOnSkill: the owner has no such stat");
        return;
    };
    if permanent < minimal {
        return;
    }
    for g in facts {
        // `Owner.HasFact(fact)` (a null blueprint: `HasFact(null)` is false, `AddFact(null)` null).
        let has = g.is_some_and(|g| {
            w.entity(u).facts.list.iter().any(|f| w.fact(*f).blueprint == Some(g))
        });
        if has {
            continue;
        }
        let f =
            g.and_then(|g| w.content.get(g)).and_then(|bp| crate::unit_facts::add_fact(w, u, &bp));
        let mut v = applied(w, c, "AppliedFacts");
        v.push(f);
        set_applied(w, c, "AppliedFacts", v);
    }
}

/// `AddFeatureOnAlignment.OnActivate` / `OnDeactivate` / `OnApplyPostLoadFixes`.
fn add_feature_on_alignment(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    match h {
        // `if (Owner.Alignment.ValueRaw.HasComponent(Alignment))`: each fact added
        // (`Data.AppliedFacts.Add(Owner.AddFact(f))`, nulls included).
        Hook::Activate => {
            let Some((component, facts)) =
                with_settings(w, c, |v| (v.int("Alignment"), v.guids_with_nulls("m_Facts")))
            else {
                return;
            };
            let Some((al, locked)) =
                w.unit(u).and_then(|x| x.alignment.as_ref()).map(|a| (a.value(), a.locked_mask))
            else {
                w.coverage.unsupported("AddFeatureOnAlignment: owner without an alignment");
                return;
            };
            // `ValueRaw` resolves through the lock mask, which isn't modelled.
            if locked != 0 {
                w.coverage.unsupported("AddFeatureOnAlignment: locked alignment mask");
                return;
            }
            // `Alignment.HasComponent(component)`: `None` (0) always; otherwise a shared bit.
            let value = i64::from(crate::narrative::alignment::enum_value(al));
            if component != 0 && value & component == 0 {
                return;
            }
            // (`Owner.AddFact(null)` returns null, which is kept.)
            for g in facts {
                let f = g
                    .and_then(|g| w.content.get(g))
                    .and_then(|bp| crate::unit_facts::add_fact(w, u, &bp));
                let mut v = applied(w, c, "AppliedFacts");
                v.push(f);
                set_applied(w, c, "AppliedFacts", v);
            }
        }
        // `Data.AppliedFacts.ForEach(Owner.RemoveFact)`, then `Clear()`.
        Hook::Deactivate => {
            for f in applied(w, c, "AppliedFacts") {
                remove_fact(w, u, f);
            }
            set_applied(w, c, "AppliedFacts", Vec::new());
        }
        // A later applied fact with an earlier one's blueprint is removed.
        Hook::ApplyPostLoadFixes => {
            let mut v = applied(w, c, "AppliedFacts");
            let mut i = 0;
            while i < v.len() {
                let mut j = i + 1;
                while j < v.len() {
                    // `entityFact.Blueprint == entityFact2.Blueprint`: a null fact throws here.
                    let (Some(a), Some(b)) = (v[i], v[j]) else {
                        set_applied(w, c, "AppliedFacts", v);
                        w.coverage.unsupported(
                            "AddFeatureOnAlignment.OnApplyPostLoadFixes: a null applied fact",
                        );
                        return;
                    };
                    if w.fact(a).blueprint == w.fact(b).blueprint {
                        remove_fact(w, u, v[j]);
                        v.remove(j);
                    } else {
                        j += 1;
                    }
                }
                i += 1;
            }
            set_applied(w, c, "AppliedFacts", v);
        }
        _ => {}
    }
}

/// `AddFeatureOnAlignment.HandleAlignmentChange(unit, new, prev)`: the sim's event doesn't
/// carry the unit or the alignments, so a change is reported.
fn add_feature_on_alignment_event(w: &mut World, c: CompRef, iface: &str) {
    let unit = w.bus.call.last().and_then(|x| x.1);
    if unit.is_some() && unit != owner_unit(w, c) {
        return;
    }
    w.coverage.unsupported(format!("AddFeatureOnAlignment: {iface}"));
}

fn applied(w: &World, c: CompRef, key: &str) -> Vec<Option<FactIdx>> {
    w.component(c).fact_refs.get(key).cloned().unwrap_or_default()
}

fn set_applied(w: &mut World, c: CompRef, key: &str, v: Vec<Option<FactIdx>>) {
    w.component_mut(c).fact_refs.insert(key.to_owned(), v);
}

fn reapply_all(w: &mut World, c: CompRef, key: &str) {
    for f in applied(w, c, key).into_iter().flatten() {
        crate::facts::lifecycle::reapply(w, f);
    }
}

/// `UnitDescriptor.RemoveFact(fact)` (null-safe).
fn remove_fact(w: &mut World, u: EntityIdx, f: Option<FactIdx>) {
    if let Some(f) = f
        && w.fact(f).owner == Some(u)
    {
        crate::unit_facts::remove(w, u, f);
    }
}

fn add_facts(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    let reapplying = w.fact(c.fact).reapplying;
    match h {
        // `OnActivate`: while the owner fact reapplies, reapply the applied facts;
        // otherwise `UpdateFacts(postLoad: false)`.
        Hook::Activate => {
            if reapplying {
                reapply_all(w, c, "AppliedFacts");
            } else {
                update_facts(w, c, u, false);
            }
        }
        // `OnDeactivate`: unless reapplying, `Clear()`.
        Hook::Deactivate => {
            if !reapplying {
                for f in applied(w, c, "AppliedFacts") {
                    remove_fact(w, u, f);
                }
                set_applied(w, c, "AppliedFacts", Vec::new());
            }
        }
        Hook::Recalculate => reapply_all(w, c, "AppliedFacts"),
        Hook::ApplyPostLoadFixes if w.fact(c.fact).is_active => update_facts(w, c, u, true),
        _ => {}
    }
}

/// `AddFacts.UpdateFacts(postLoad)`.
fn update_facts(w: &mut World, c: CompRef, u: EntityIdx, post_load: bool) {
    let Some((listed, no_restore, difficulty, caster_level)) = with_settings(w, c, |v| {
        (
            v.guids("m_Facts"),
            v.bool("DoNotRestoreMissingFacts"),
            (
                v.bool("HasDifficultyRequirements"),
                v.bool("InvertDifficultyRequirements"),
                v.i32("MinDifficulty"),
            ),
            v.i32("CasterLevel"),
        )
    }) else {
        return;
    };
    // `IsRestrictedByDifficulty()`: without `InvertDifficultyRequirements` the facts need the
    // current stats adjustment to reach the minimum difficulty's; inverted, to not exceed it.
    if difficulty.0 {
        match w.settings.difficulty.stat_adjustment_compare_to(difficulty.2) {
            Ok(cmp) => {
                let restricted = if difficulty.1 { cmp > 0 } else { cmp < 0 };
                if restricted {
                    // `Clear()`.
                    for f in applied(w, c, "AppliedFacts") {
                        remove_fact(w, u, f);
                    }
                    set_applied(w, c, "AppliedFacts", Vec::new());
                    return;
                }
            }
            Err(why) => {
                w.coverage.unsupported(format!("AddFacts: difficulty requirements ({why})"));
                return;
            }
        }
    }
    let mut cur: Vec<FactIdx> = applied(w, c, "AppliedFacts")
        .into_iter()
        .flatten()
        .filter(|f| !w.fact(*f).is_disposed && w.fact(*f).owner.is_some())
        .collect();
    let stale: Vec<FactIdx> = cur
        .iter()
        .copied()
        .filter(|f| !w.fact(*f).blueprint.is_some_and(|g| listed.contains(&g)))
        .collect();
    let mut missing = Vec::new();
    if !post_load || !no_restore {
        for g in &listed {
            if !cur.iter().any(|f| w.fact(*f).blueprint == Some(*g)) {
                missing.push(*g);
            }
        }
    }
    for f in stale {
        cur.retain(|x| *x != f);
        remove_fact(w, u, Some(f));
    }
    set_applied(w, c, "AppliedFacts", cur.iter().map(|f| Some(*f)).collect());
    for g in missing {
        let Some(bp) = w.content.get(g) else {
            w.coverage.unsupported(format!("AddFacts: missing blueprint {g}"));
            continue;
        };
        // `CreateFact(null, Owner, null)` then `Owner.Facts.Add`; a buff's `CreateFact` is
        // `new Buff(this, new MechanicsContext(null, owner, this), null)`.
        // `if (CasterLevel > 0 && unitFact.MaybeContext != null)
        // unitFact.MaybeContext.Params.CasterLevel = CasterLevel`, before `Facts.Add`: the
        // fresh context has no params, so this writes the shared `AbilityParams.Empty`.
        let write = move |w: &mut World, nf: FactIdx| {
            if caster_level > 0 {
                crate::context::set_params_caster_level(w, nf, caster_level);
            }
        };
        let added = if crate::unit_facts::kind_for(&bp).0 == crate::world::FactKind::Buff {
            let ctx = crate::unit_facts::context(w, None, Some(u), &bp);
            crate::unit_facts::add_buff_with(w, u, &bp, ctx, None, write)
        } else {
            let nf = crate::unit_facts::create_fact(w, &bp, u, None);
            write(w, nf);
            crate::unit_facts::add(w, u, nf)
        };
        // `SetSourceItem(Context?.Root?.SourceItem)`.
        if w.fact(c.fact).context.as_ref().is_some_and(|x| x.source_item.is_some()) {
            w.coverage.unsupported("AddFacts: source item");
        }
        if let Some(a) = added {
            let mut v = applied(w, c, "AppliedFacts");
            v.push(Some(a));
            set_applied(w, c, "AppliedFacts", v);
        }
    }
}

/// `AddFeatureToNPC.OnActivate`/`OnDeactivate`.
fn add_feature_to_npc(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    match h {
        Hook::Activate => {
            let Some((check_party, check_summoned, feature)) = with_settings(w, c, |v| {
                (v.bool("CheckParty"), v.bool("CheckSummoned"), v.guid("m_Feature"))
            }) else {
                return;
            };
            // `Rulebook.CurrentContext.LastEvent<RuleSummonUnit>()`: no summoning here.
            let _ = check_summoned;
            let master = w.unit(u).and_then(|x| x.master);
            let player = match master {
                Some(m) => w.unit(m).is_some_and(|x| x.is_player_faction),
                None => w.unit(u).is_some_and(|x| x.is_player_faction),
            };
            let flag3 = (check_party && player) || (!check_party && !player);
            if flag3 {
                let bp = feature.and_then(|g| w.content.get(g));
                let f = bp.and_then(|bp| crate::unit_facts::add_fact(w, u, &bp));
                set_applied(w, c, "AppliedFact", vec![f]);
            }
        }
        Hook::Deactivate => {
            let f = applied(w, c, "AppliedFact").first().copied().flatten();
            remove_fact(w, u, f);
            set_applied(w, c, "AppliedFact", vec![None]);
        }
        _ => {}
    }
}

/// `FactSinglify.OnActivate`/`OnDeactivate`.
fn fact_singlify(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    let reapplying = w.fact(c.fact).reapplying;
    match h {
        Hook::Activate => {
            if reapplying {
                reapply_all(w, c, "AppliedFacts");
                return;
            }
            let Some((old, new)) =
                with_settings(w, c, |v| (v.guids("m_OldFacts"), v.guids("m_NewFacts")))
            else {
                return;
            };
            for g in old {
                // `Owner.RemoveFact(blueprint)`: the first fact with that blueprint.
                let f = w
                    .entity(u)
                    .facts
                    .list
                    .iter()
                    .copied()
                    .find(|f| w.fact(*f).blueprint == Some(g));
                remove_fact(w, u, f);
            }
            let mut applied_now = Vec::new();
            for g in new {
                let existing: Vec<FactIdx> = w
                    .entity(u)
                    .facts
                    .list
                    .iter()
                    .copied()
                    .filter(|f| w.fact(*f).blueprint == Some(g))
                    .collect();
                let f = match existing.first() {
                    None => w.content.get(g).and_then(|bp| crate::unit_facts::add_fact(w, u, &bp)),
                    Some(first) => {
                        for x in existing.iter().skip(1) {
                            remove_fact(w, u, Some(*x));
                        }
                        Some(*first)
                    }
                };
                applied_now.push(f);
            }
            set_applied(w, c, "AppliedFacts", applied_now);
        }
        Hook::Deactivate if !reapplying => {
            for f in applied(w, c, "AppliedFacts") {
                remove_fact(w, u, f);
            }
            set_applied(w, c, "AppliedFacts", Vec::new());
        }
        _ => {}
    }
}

/// `BuffExtraEffects`' settings: the checked buffs (`m_CheckedBuff`, then `m_CheckedBuffList`),
/// the extra buff, the exception fact and `m_UseBaffContext`.
struct ExtraEffects {
    checked: Option<wr_content::Guid>,
    list: Vec<Option<wr_content::Guid>>,
    extra: Option<wr_content::Guid>,
    exception: Option<wr_content::Guid>,
    buff_context: bool,
}

fn extra_effects(w: &World, c: CompRef) -> Option<ExtraEffects> {
    with_settings(w, c, |v| ExtraEffects {
        checked: v.guid("m_CheckedBuff"),
        list: v.guids_with_nulls("m_CheckedBuffList"),
        extra: v.guid("m_ExtraEffectBuff"),
        exception: v.guid("m_ExceptionFact"),
        buff_context: v.bool("m_UseBaffContext"),
    })
}

/// `Owner.HasFact(blueprint)` (`false` for a null blueprint).
fn owner_has(w: &World, u: EntityIdx, g: Option<wr_content::Guid>) -> bool {
    g.is_some_and(|g| w.entity(u).facts.list.iter().any(|f| w.fact(*f).blueprint == Some(g)))
}

impl ExtraEffects {
    /// `CheckHadPrerequiredBuff()`.
    fn had_prerequired(&self, w: &World, u: EntityIdx) -> bool {
        owner_has(w, u, self.checked) || self.list.iter().any(|g| owner_has(w, u, *g))
    }

    /// `buff.Blueprint == CheckedBuff || m_CheckedBuffList` contains it.
    fn checks(&self, bp: Option<wr_content::Guid>) -> bool {
        bp == self.checked || self.list.contains(&bp)
    }

    /// `ExceptionFact == null || !Owner.HasFact(ExceptionFact)`.
    fn allowed(&self, w: &World, u: EntityIdx) -> bool {
        self.exception.is_none() || !owner_has(w, u, self.exception)
    }
}

/// `Data.AppliedBuff?.Remove()`: off its owner (a buff no longer attached only gets its end
/// time, which nothing reads off the unit).
fn remove_applied_buff(w: &mut World, c: CompRef) {
    if let Some(b) = applied(w, c, "AppliedBuff").first().copied().flatten()
        && let Some(o) = w.fact(b).owner
    {
        crate::unit_facts::remove(w, o, b);
    }
}

/// `Owner.AddBuff(ExtraEffectBuff, parentContext)`: the parent context cloned for the buff, or
/// (none) a context of the owner's own; a missing blueprint gives null.
fn add_extra_buff(
    w: &mut World,
    u: EntityIdx,
    extra: Option<wr_content::Guid>,
    parent: Option<crate::context::MechanicsContext>,
) -> Option<FactIdx> {
    let bp = extra.and_then(|g| w.content.get(g))?;
    let ctx = match &parent {
        Some(p) => {
            w.pending_buffs.parent_descriptor = p.spell_descriptor;
            crate::unit_facts::clone_for(w, p, &bp, u)
        }
        None => crate::unit_facts::context(w, Some(u), Some(u), &bp),
    };
    crate::unit_facts::add_buff(w, u, &bp, ctx, None)
}

/// `BuffExtraEffects.OnActivate` / `OnApplyPostLoadFixes` (`CheckAndReapply`) / `OnDeactivate`.
fn buff_extra_effects(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    let Some(s) = extra_effects(w, c) else { return };
    match h {
        Hook::Activate | Hook::ApplyPostLoadFixes => {
            remove_applied_buff(w, c);
            set_applied(w, c, "AppliedBuff", vec![None]);
            if s.allowed(w, u) && s.had_prerequired(w, u) {
                let ctx = w.fact(c.fact).context.clone();
                let b = add_extra_buff(w, u, s.extra, ctx);
                set_applied(w, c, "AppliedBuff", vec![b]);
            }
        }
        // `Owner.RemoveFact(ExtraEffectBuff)`: the owner's first fact of that blueprint
        // (`Data.AppliedBuff` stays).
        Hook::Deactivate => {
            let first = s.extra.and_then(|g| {
                w.entity(u).facts.list.iter().copied().find(|f| w.fact(*f).blueprint == Some(g))
            });
            if let Some(f) = first {
                crate::unit_facts::remove(w, u, f);
            }
        }
        _ => {}
    }
}

/// `HandleBuffDidAdded(buff)` / `HandleBuffDidRemoved(buff)` for a buff of the owner's that the
/// component checks: added, with no applied buff, the extra buff goes on (in the buff's context
/// with `m_UseBaffContext`); removed, with an applied buff and no checked buff left, it comes off.
fn buff_extra_effects_event(w: &mut World, c: CompRef, _iface: &str) {
    let Some(u) = owner_unit(w, c) else { return };
    let method = w.bus.call.last().map_or("", |x| x.0);
    let Some(Some(buff)) = w.bus.call_fact.last().copied() else {
        w.coverage.unsupported("BuffExtraEffects: IUnitBuffHandler without its buff");
        return;
    };
    if w.fact(buff).owner != Some(u) {
        return;
    }
    let Some(s) = extra_effects(w, c) else { return };
    let applied_buff = applied(w, c, "AppliedBuff").first().copied().flatten();
    match method {
        "HandleBuffDidAdded" => {
            if applied_buff.is_none()
                && s.allowed(w, u)
                && s.checks(w.fact(buff).blueprint)
                && s.had_prerequired(w, u)
            {
                let ctx = if s.buff_context {
                    w.fact(buff).context.clone()
                } else {
                    w.fact(c.fact).context.clone()
                };
                let b = add_extra_buff(w, u, s.extra, ctx);
                set_applied(w, c, "AppliedBuff", vec![b]);
            }
        }
        "HandleBuffDidRemoved" => {
            if applied_buff.is_some()
                && s.checks(w.fact(buff).blueprint)
                && !s.had_prerequired(w, u)
            {
                remove_applied_buff(w, c);
                set_applied(w, c, "AppliedBuff", vec![None]);
            }
        }
        m => w.coverage.unsupported(format!("BuffExtraEffects: IUnitBuffHandler.{m}")),
    }
}

/// `BuffOnEntityCreated.OnActivate`: `GameHelper.ApplyBuff(Owner, Buff)` (the owner its own
/// caster, permanent), its unique id kept; `OnDeactivate`: the owner's fact of that id is removed
/// (`Facts.FindById`), the id cleared.
fn buff_on_entity_created(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    match h {
        Hook::Activate => {
            let buff = with_settings(w, c, |v| v.guid("m_Buff"))
                .flatten()
                .and_then(|g| w.content.get(g))
                .and_then(|bp| {
                    let ctx = crate::unit_facts::context(w, Some(u), Some(u), &bp);
                    crate::unit_facts::add_buff(w, u, &bp, ctx, None)
                });
            let id = buff.map(|b| w.fact(b).unique_id.clone());
            w.component_mut(c).data = Some(serde_json::json!({ "BuffId": id }));
        }
        Hook::Deactivate => {
            let id = w
                .component(c)
                .data
                .as_ref()
                .and_then(|d| d.get("BuffId"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            if let Some(id) = id {
                let f = w.entity(u).facts.list.iter().copied().find(|f| w.fact(*f).unique_id == id);
                if let Some(f) = f {
                    crate::unit_facts::remove(w, u, f);
                }
                w.component_mut(c).data = Some(serde_json::json!({ "BuffId": null }));
            }
        }
        _ => {}
    }
}
