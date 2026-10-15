//! `MechanicsContext` (`Kingmaker.UnitLogic.Mechanics/MechanicsContext.cs`): the saved
//! state a fact's components evaluate `ContextValue`s against (ranks, shared values,
//! ability parameters), plus caster, owner and source item.

use wr_content::Guid;

use crate::world::{EntityIdx, FactIdx, World};

/// `MechanicsContext.CountOfRanks` / `CountOfValues` are the lengths of the saved arrays.
#[derive(Debug, Clone, Default)]
pub struct MechanicsContext {
    pub associated_blueprint: Option<Guid>,
    /// `ParentContext?.AssociatedBlueprint`.
    pub parent_associated_blueprint: Option<Guid>,
    /// `m_OwnerRef` (unit `UniqueId`).
    pub owner: Option<String>,
    /// `m_CasterRef`; the `[OnDeserialized]` hook sets it to the owner when the save left
    /// it out (which is exactly when caster == owner).
    pub caster: Option<String>,
    /// `m_Ranks`, indexed by `AbilityRankType`.
    pub ranks: Vec<i32>,
    /// `m_SharedValues`, indexed by `AbilitySharedValue`.
    pub shared_values: Vec<i32>,
    /// `m_Params` persisted members (`None` for a saved null).
    pub params: Option<serde_json::Value>,
    /// `ParentContext?.Params`: the first non-null `m_Params` up the parent chain, resolved
    /// when the context is loaded or cloned (`None`: `AbilityParams.Empty`).
    pub parent_params: Option<serde_json::Value>,
    /// `m_SourceItemRef`.
    pub source_item: Option<String>,
    /// `ParentContext?.SourceItem`, which `ModifiableValue.AddModifier(…, component)`
    /// records as the modifier's item source.
    pub parent_source_item: Option<String>,
    /// The context or a parent is an `AbilityExecutionContext` (`SourceAbilityContext`).
    pub has_ability_context: bool,
    /// `SpellDescriptor` (saved): the parent's, or'ed with the associated blueprint's
    /// `SpellDescriptorComponent`.
    pub spell_descriptor: i64,
}

impl MechanicsContext {
    /// `ParentContext?.SourceItem`, resolved now (`EntityRef` is lazy).
    #[must_use]
    pub fn parent_source_item(&self, w: &World) -> Option<EntityIdx> {
        self.parent_source_item.as_ref().and_then(|id| w.by_id.get(id).copied())
    }

    /// `Params` (`m_Params ?? ParentContext?.Params ?? AbilityParams.Empty`), `None` for
    /// the empty params.
    #[must_use]
    pub fn effective_params(&self) -> Option<&serde_json::Value> {
        self.params.as_ref().or(self.parent_params.as_ref())
    }

    /// `this[AbilityRankType]`.
    #[must_use]
    pub fn rank(&self, t: i64) -> i32 {
        usize::try_from(t).ok().and_then(|i| self.ranks.get(i).copied()).unwrap_or(0)
    }

    /// `this[AbilitySharedValue]`.
    #[must_use]
    pub fn shared(&self, t: i64) -> i32 {
        usize::try_from(t).ok().and_then(|i| self.shared_values.get(i).copied()).unwrap_or(0)
    }
}

/// `MechanicsContext.RecalculateAbilityParams(skipParamsSet: false)` of a fact's context:
/// the blueprint's first `ContextAbilityParamsCalculator` computes `m_Params` (a
/// `ContextSetAbilityParams` isn't ported and is reported).
pub fn recalculate_ability_params(w: &mut World, f: FactIdx) {
    let Some(ctx) = w.fact(f).context.clone() else { return };
    let Some(bp) = ctx.associated_blueprint.and_then(|g| w.content.get(g)) else { return };
    let Some(o) = bp
        .components_array()
        .into_iter()
        .find(|o| crate::content::is_a(o.ty, "ContextAbilityParamsCalculator"))
    else {
        return;
    };
    if !crate::content::is_a(o.ty, "ContextCalculateAbilityParams") {
        w.coverage.unsupported(format!(
            "MechanicsContext.RecalculateAbilityParams: {}",
            wr_content::schema::ty(o.ty).short
        ));
        return;
    }
    let Some(caster) = ctx.caster.as_ref().and_then(|id| w.by_id.get(id.as_str()).copied()) else {
        w.coverage.unsupported("MechanicsContext.RecalculateAbilityParams: no caster");
        return;
    };
    if let Some(p) = crate::rules::ability_params::calculate_context_params(
        w,
        &ctx,
        &bp,
        caster,
        crate::content::View(o),
    ) && let Some(c) = w.fact_mut(f).context.as_mut()
    {
        c.params = Some(p);
    }
}

/// `fact.MaybeContext.Params.CasterLevel = level` (`AddFacts.UpdateFacts`): written through
/// whatever `Params` resolves to. For a context with no `m_Params` and no parent params
/// that is the shared `AbilityParams.Empty`; a context that has params of its own is not
/// reached by a fresh fact and is reported.
pub fn set_params_caster_level(w: &mut World, f: FactIdx, level: i32) {
    let Some(ctx) = &w.fact(f).context else { return };
    if ctx.effective_params().is_some() {
        w.coverage.unsupported("AbilityParams: CasterLevel write into a context's own params");
        return;
    }
    w.empty_params.caster_level = level;
    w.empty_params.known = true;
}

/// `MechanicsContext.Recalculate()` for a fact's context: no-op without a caster; else
/// ability params, ranks (`ContextRankConfig`), shared values (`ContextCalculateSharedValue`)
/// and context properties (`PropertyCalculatorComponent`). The ranks are ported
/// ([`rank_value`], for the base value types the sim models); the other calculators aren't,
/// so a recalculation that has any of them is an unsupported execution.
pub fn recalculate(w: &mut World, f: FactIdx) {
    let Some(ctx) = &w.fact(f).context else { return };
    // `MaybeCaster`: the caster, which is the owner when the context was built without one.
    let Some(caster) = ctx.caster.as_ref().and_then(|id| w.by_id.get(id.as_str()).copied()) else {
        return;
    };
    let Some(bp) = ctx.associated_blueprint.and_then(|g| w.content.get(g)) else { return };
    // `RecalculateAbilityParams(skipParamsSet: true)`: the blueprint's first
    // `ContextAbilityParamsCalculator` (`GetComponent` reads `ComponentsArray` only); a
    // `ContextSetAbilityParams` is skipped here and set by `RecalculateRanks`.
    let calc = bp
        .components_array()
        .into_iter()
        .find(|o| crate::content::is_a(o.ty, "ContextAbilityParamsCalculator"));
    let mut others = Vec::new();
    if let Some(o) = calc {
        if crate::content::is_a(o.ty, "ContextCalculateAbilityParams") {
            let ctx = w.fact(f).context.clone().unwrap_or_default();
            if let Some(p) = crate::rules::ability_params::calculate_context_params(
                w,
                &ctx,
                &bp,
                caster,
                crate::content::View(o),
            ) && let Some(c) = w.fact_mut(f).context.as_mut()
            {
                c.params = Some(p);
            }
        } else {
            others.push(wr_content::schema::ty(o.ty).short);
        }
    }
    // Shared values and context properties.
    others.extend(
        bp.components_array()
            .iter()
            .filter(|o| {
                ["ContextCalculateSharedValue", "PropertyCalculatorComponent"]
                    .iter()
                    .any(|n| crate::content::is_a(o.ty, n))
            })
            .map(|o| wr_content::schema::ty(o.ty).short),
    );
    // `RecalculateRanks`: `AssociatedBlueprint.GetComponents<ContextRankConfig>()` reads the
    // blueprint's own `ComponentsArray` only (`BlueprintExtenstions.cs:72`), not the
    // `ComponentsList`s that `BlueprintFact.CollectComponents` expands.
    for o in bp.components_array() {
        if !crate::content::is_a(o.ty, "ContextRankConfig") {
            continue;
        }
        let v = crate::content::View(o);
        let Some(value) = rank_value(w, f, caster, v) else {
            continue;
        };
        let t = usize::try_from(v.int("m_Type")).unwrap_or(0);
        if let Some(ctx) = w.fact_mut(f).context.as_mut() {
            if ctx.ranks.len() <= t {
                ctx.ranks.resize(t + 1, 0);
            }
            ctx.ranks[t] = value;
        }
    }
    for c in others {
        w.coverage.unsupported(format!("MechanicsContext.Recalculate: {c}"));
    }
}

/// `ContextRankConfig.GetValue(context)` for the fact's context and its caster: the base
/// value, the progression, then the minimum and maximum. `None` (reported) for a base value
/// type or condition the sim doesn't model.
fn rank_value(
    w: &mut World,
    f: FactIdx,
    caster: EntityIdx,
    v: crate::content::View<'_>,
) -> Option<i32> {
    use crate::stats::types::StatType;
    use crate::world::FactKind;
    // `context.Params`, or the process-wide `AbilityParams.Empty` (`World::empty_params`:
    // caster level 1 until an `AddFacts` writes it, rank bonus 0). A saved member at its
    // default (0) is left out (`DefaultValueHandling.IgnoreAndPopulate`).
    let param = |w: &mut World, name: &str| -> i32 {
        if w.fact(f).context.as_ref().and_then(MechanicsContext::effective_params).is_none() {
            let e = w.read_empty_params();
            return if name == "CasterLevel" { e.caster_level } else { e.rank_bonus };
        }
        match w.fact(f).context.as_ref().and_then(MechanicsContext::effective_params) {
            None => 0,
            Some(p) => p.get(name).and_then(serde_json::Value::as_i64).map_or(0, |v| v as i32),
        }
    };
    let bad = |w: &mut World, what: &str| -> Option<i32> {
        w.coverage.unsupported(format!("ContextRankConfig: {what}"));
        None
    };
    let has_feature =
        |w: &World, g: Guid| -> Option<usize> {
            w.entity(caster).facts.list.iter().position(|x| {
                w.fact(*x).kind == FactKind::Feature && w.fact(*x).blueprint == Some(g)
            })
        };
    // `Features.GetRank(blueprint)`.
    let feature_rank = |w: &World, g: Option<Guid>| -> i32 {
        g.and_then(|g| has_feature(w, g)).map_or(0, |i| w.fact(w.entity(caster).facts.list[i]).rank)
    };
    // `ContextRankBaseValueType`.
    let base = match v.int("m_BaseValueType") {
        0 => param(w, "CasterLevel"),
        1 => {
            let except = v.bool("m_ExceptClasses");
            let classes = v.guids("m_Class");
            let mut n = param(w, "RankBonus");
            for cd in &w.unit(caster)?.progression.classes {
                if except != classes.contains(&cd.class) {
                    n = n.wrapping_add(cd.level);
                }
            }
            n
        }
        2 => feature_rank(w, v.guid("m_Feature")),
        3 => {
            if v.int("m_SpecificModifier") != 0 {
                return bad(w, "StatBonus with a specific modifier");
            }
            let stat = StatType(v.i32("m_Stat"));
            if stat.is_attribute() {
                crate::stats::ops::attribute_bonus(w, caster, stat)
            } else {
                0
            }
        }
        4 => w.stats(caster).v(StatType::BaseAttackBonus).modified_value,
        5 => crate::unit::character_level(w, caster),
        6 => {
            let n = v
                .guids("m_FeatureList")
                .into_iter()
                .filter(|g| has_feature(w, *g).is_some())
                .count();
            i32::try_from(n).unwrap_or(0)
        }
        7 => v
            .guids("m_FeatureList")
            .into_iter()
            .fold(0i32, |a, g| a.wrapping_add(feature_rank(w, Some(g)))),
        13 => w.stats(caster).get(StatType(v.i32("m_Stat"))).map_or(0, |s| s.base_value),
        18 => crate::unit::mythic_level(w, caster),
        other => return bad(w, &format!("base value type {other}")),
    };
    // `ApplyProgression` (C# `/` truncates; a zero step throws).
    let start = v.i32("m_StartLevel");
    let step = v.i32("m_StepLevel");
    let div =
        |a: i32, b: i32| -> Option<i32> { if b == 0 { None } else { Some(a.wrapping_div(b)) } };
    let value = match v.int("m_Progression") {
        0 => base,
        1 => base / 2,
        2 => 1i32.wrapping_add(base.wrapping_sub(1) / 2),
        3 => 1i32.wrapping_add(0.max(div(base.wrapping_sub(start), step)?)),
        4 => div(base, step)?,
        5 => 1i32.wrapping_add(0.max(div(base, step)?)),
        6 => base.wrapping_mul(step),
        7 => base.wrapping_add(base / 2),
        8 => base.wrapping_add(step),
        9 => {
            if base < start {
                0
            } else {
                1i32.wrapping_add(0.max(div(base.wrapping_sub(start), step)?))
            }
        }
        10 => 1i32.wrapping_add(2i32.wrapping_mul(0.max(div(base.wrapping_sub(start), step)?))),
        11 => (base / 2).wrapping_add(step),
        12 => {
            let items = v.objs("m_CustomProgression");
            let mut out = None;
            for it in &items {
                let iv = crate::content::View(it);
                if base <= iv.i32("BaseValue") {
                    out = Some(iv.i32("ProgressionValue"));
                    break;
                }
            }
            out.unwrap_or_else(|| {
                items.last().map_or(0, |it| crate::content::View(it).i32("ProgressionValue"))
            })
        }
        13 => base.wrapping_mul(2).wrapping_add(step),
        14 => base.wrapping_mul(base),
        other => return bad(w, &format!("progression {other}")),
    };
    // `ApplyMinMax`: intensified metamagic needs the context's metamagic.
    if v.bool("m_AffectedByIntensifiedMetamagic") {
        return bad(w, "intensified metamagic");
    }
    let mut value = value;
    if v.bool("m_UseMin") {
        value = value.max(v.i32("m_Min"));
    }
    if v.bool("m_UseMax") {
        value = value.min(v.i32("m_Max"));
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    use wr_content::node::Node;

    use super::*;
    use crate::facts::tests::{feature, loaded_fact, obj};

    fn rank_config(name: &str, min: i64) -> Node {
        // Character level, no progression, and a minimum of `min` (a unit of level 0 reads it).
        Node::Object(Box::new(obj(
            "Kingmaker.UnitLogic.Mechanics.Components.ContextRankConfig",
            &[
                ("name", Node::Str(name.into())),
                ("m_BaseValueType", Node::Enum(5)),
                ("m_Type", Node::Enum(0)),
                ("m_UseMin", Node::Bool(min > 0)),
                ("m_Min", Node::Int(min)),
            ],
        )))
    }

    fn rank_from_params(base: i64, params: Option<serde_json::Value>) -> Vec<i32> {
        rank_from(base, params, None)
    }

    fn rank_from(
        base: i64,
        params: Option<serde_json::Value>,
        parent_params: Option<serde_json::Value>,
    ) -> Vec<i32> {
        let (mut w, u) = crate::stats::tests::world_with_unit();
        w.by_id.insert("u".into(), u);
        w.empty_params = crate::world::EmptyAbilityParams::supplied(1, 0);
        let cfg = Node::Object(Box::new(obj(
            "Kingmaker.UnitLogic.Mechanics.Components.ContextRankConfig",
            &[
                ("name", Node::Str("$rank$".into())),
                ("m_BaseValueType", Node::Enum(base)),
                ("m_Type", Node::Enum(0)),
            ],
        )));
        let bp = feature(&w, 22, vec![cfg]);
        let f = loaded_fact(&mut w, u, &bp, true);
        w.fact_mut(f).context = Some(MechanicsContext {
            associated_blueprint: Some(bp.guid),
            owner: Some("u".into()),
            caster: Some("u".into()),
            ranks: vec![-1],
            params,
            parent_params,
            ..MechanicsContext::default()
        });
        recalculate(&mut w, f);
        assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
        w.fact(f).context.as_ref().map(|c| c.ranks.clone()).unwrap_or_default()
    }

    /// `ContextRankConfig.GetBaseValue`: `ClassLevel` starts from `context.Params.RankBonus`
    /// and `CasterLevel` is `context.Params.CasterLevel`; without params they read
    /// `AbilityParams.Empty` (rank bonus 0, caster level 1), and a saved member at its
    /// default is left out of the save (0).
    #[test]
    fn rank_base_values_read_the_contexts_ability_params() {
        let p = serde_json::json!({ "SpellSource": 2, "CasterLevel": 7, "RankBonus": 3 });
        assert_eq!(rank_from_params(1, Some(p.clone())), vec![3], "no class levels");
        assert_eq!(rank_from_params(0, Some(p)), vec![7]);
        assert_eq!(rank_from_params(0, None), vec![1]);
        assert_eq!(rank_from_params(1, Some(serde_json::json!({ "CasterLevel": 7 }))), vec![0]);
        // `m_Params ?? ParentContext?.Params`.
        let parent = serde_json::json!({ "CasterLevel": 12 });
        assert_eq!(rank_from(0, None, Some(parent.clone())), vec![12]);
        assert_eq!(
            rank_from(0, Some(serde_json::json!({ "CasterLevel": 5 })), Some(parent)),
            vec![5]
        );
    }

    /// `MechanicsContext.RecalculateRanks` reads `AssociatedBlueprint.GetComponents`, which
    /// is the blueprint's own `ComponentsArray`: a configuration inside a `ComponentsList`
    /// is never read, and can't overwrite a direct one of the same rank type.
    #[test]
    fn rank_configs_come_from_the_blueprints_own_components_only() {
        let (mut w, u) = crate::stats::tests::world_with_unit();
        w.by_id.insert("u".into(), u);
        let list = feature(&w, 20, vec![rank_config("$nested$", 5)]);
        let list_ref = Node::Object(Box::new(obj(
            "Kingmaker.Blueprints.ComponentsList",
            &[("name", Node::Str("$list$".into())), ("m_List", Node::BpRef(list.guid))],
        )));
        let bp = feature(&w, 21, vec![rank_config("$direct$", 0), list_ref]);
        let f = loaded_fact(&mut w, u, &bp, true);
        w.fact_mut(f).context = Some(MechanicsContext {
            associated_blueprint: Some(bp.guid),
            owner: Some("u".into()),
            caster: Some("u".into()),
            ranks: vec![7],
            ..MechanicsContext::default()
        });
        recalculate(&mut w, f);
        let ranks = w.fact(f).context.as_ref().map(|c| c.ranks.clone());
        assert_eq!(ranks, Some(vec![0]), "the nested configuration (min 5) is ignored");
        assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
    }

    #[test]
    fn clone_for_starts_ranks_at_zero_and_copies_shared_values() {
        let (w, u) = crate::stats::tests::world_with_unit();
        let bp = feature(&w, 23, vec![]);
        let p = MechanicsContext {
            ranks: vec![4, 5, 6],
            shared_values: vec![7, 8, 9],
            ..MechanicsContext::default()
        };
        let c = crate::unit_facts::clone_for(&w, &p, &bp, u);
        assert_eq!(c.ranks, vec![0, 0, 0]);
        assert_eq!(c.shared_values, vec![7, 8, 9]);
    }
}
