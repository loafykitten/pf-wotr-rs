//! Etude brackets: the generic `EtudeBracketRuntime` and the bracket components the
//! slice's saves carry (decompiled, not copied: `Kingmaker.AreaLogic.Etudes/
//! EtudeBracketTrigger.cs` and each `EtudeBracket*.cs` beside it;
//! `Kingmaker.Designers.EventConditionActionSystem.Events/{EtudePlayTrigger,
//! EtudePlayTriggerData,EtudeInvokeActionsDelayed}.cs`; `Kingmaker.Corruption/
//! {EtudeCorruptionFreeZone,EtudeDisableCraft}.cs`; `Kingmaker.AreaLogic.Etudes/
//! {EtudeIgnorePartyEncumbrance,EtudeIgnorePersonalEncumbrance,EtudePeacefulZone}.cs`;
//! `Kingmaker.Tutorial.Etudes/EtudeBracketEnableTutorials.cs`;
//! `Kingmaker.AreaLogic.Etudes/DisableMountRiding.cs`;
//! `Kingmaker.AreaLogic.Capital/CapitalCompanionLogic.cs`;
//! `Kingmaker.EntitySystem/EntityFactComponentDelegate.cs` `ComponentRuntime`;
//! `Kingmaker.PubSubSystem/{EventBus,SubscriptionManager}.cs` for the proxy subscription).
//!
//! Every `EtudeBracketTrigger<TData>` subclass gets the same runtime class,
//! `EtudeBracketRuntime`, a `ComponentRuntime` that forwards each lifecycle hook to the
//! delegate (the blueprint component) and adds the bracket logic:
//!
//! - `OnActivate`: the delegate's `OnActivate`, then `m_AlreadyProcessedActivation =
//!   false` (saved).
//! - `OnDeactivate`: the delegate's `OnDeactivate`; if the activation was processed, clear
//!   it and `Exit` (skipped, with an error, when the linked area is unavailable now and on
//!   exit).
//! - `OnPostLoad`: the delegate's `OnPostLoad`; if the fact is active and the activation
//!   was processed, `Resume` (or, when the linked area is unavailable or the etude is
//!   read-only, `m_NeedResume`, not saved). At a save's load no area is loaded yet, so any
//!   linked area part is unavailable there.
//! - `OnEtudesUpdate` / `OnAreaActivated` (the runtime subscribes to both): `MaybeEnter`,
//!   which `Enter`s the first time and `Resume`s once after a deferred post-load.
//!
//! `Enter`, `Exit` and `Resume` call the delegate's `OnEnter`/`OnExit`/`OnResume` inside
//! the runtime's event context and catch every exception (logged).
//!
//! **Subscription** (`EventBus.Subscribe(runtime)`): the runtime is an `ISubscriptionProxy`,
//! so first the delegate's interfaces are subscribed with the runtime as the listener
//! ([`Sub::Component`]), then the runtime's own (`IEtudesUpdateHandler`,
//! `IAreaActivationHandler`; [`Sub::Runtime`]). `SubscriptionManager.ExecuteAction` prefers
//! the delegate whenever the delegate implements the raised interface, so for
//! `EtudePlayTrigger` (whose delegate implements both) the delegate's handler runs twice
//! per raise and the runtime's `MaybeEnter` never runs; that is what the original does.
//!
//! The brackets' effects: unit and area `CountableFlag`s (passive, essential, direct
//! control, peaceful, encumbrance, craft, corruption) are kept as named counters in
//! [`crate::narrative::Player::counters`]; unit interaction overrides as a list; audio,
//! music and weather brackets are presentation (recorded stubs).
//!
//! [`Sub::Component`]: crate::rulebook::bus::Sub::Component
//! [`Sub::Runtime`]: crate::rulebook::bus::Sub::Runtime

use std::rc::Rc;

use serde_json::{Value, json};
use wr_content::Guid;
use wr_content::schema::{self, TypeIdx};

use super::{Port, bits, settings};
use crate::content::{Blueprint, View};
use crate::registry::{Hook, Registry};
use crate::world::{CompRef, EntityIdx, World};

/// Whether a component type is an `EtudeBracketTrigger<TData>` subclass (it runs on an
/// `EtudeBracketRuntime`).
#[must_use]
pub fn is_bracket(ty: TypeIdx) -> bool {
    crate::content::is_a(ty, "EtudeBracketTrigger")
}

/// Whether a component's runtime subscribes on its own (besides its delegate): bracket
/// runtimes and `EtudeInvokeActionDelayedData` (`ITimedEvent`).
#[must_use]
pub fn runtime_interfaces(ty: TypeIdx) -> &'static [&'static str] {
    if is_bracket(ty) {
        &["IEtudesUpdateHandler", "IAreaActivationHandler"]
    } else if crate::content::is_a(ty, "EtudeInvokeActionsDelayed") {
        &["ITimedEvent"]
    } else {
        &[]
    }
}

fn short(w: &World, c: CompRef) -> &'static str {
    w.component(c).ty().map_or("?", |t| schema::ty(t).short)
}

fn etude_bp(w: &World, c: CompRef) -> Option<Rc<Blueprint>> {
    w.fact(c.fact).bp.clone()
}

fn processed(w: &World, c: CompRef) -> bool {
    w.component(c).runtime.get("m_AlreadyProcessedActivation").and_then(Value::as_bool)
        == Some(true)
}

fn set_processed(w: &mut World, c: CompRef, v: bool) {
    w.component_mut(c).runtime.insert("m_AlreadyProcessedActivation".into(), Value::Bool(v));
}

// ---- the runtime ----------------------------------------------------------------------

/// A lifecycle hook of a bracket runtime (called by [`crate::registry::dispatch_hook`]).
pub fn runtime_hook(w: &mut World, c: CompRef, h: Hook) {
    match h {
        Hook::Activate => {
            delegate_hook(w, c, h);
            set_processed(w, c, false);
        }
        Hook::Deactivate => {
            delegate_hook(w, c, h);
            if processed(w, c) {
                set_processed(w, c, false);
                exit(w, c);
            }
        }
        Hook::PostLoad => {
            delegate_hook(w, c, h);
            // `Player.PostLoad` runs before the save's area loads: `AreaService.CurrentAreaPart`
            // is null then (`CurrentlyLoadedArea == null`), so an etude with a linked area part
            // (its own or up its parents) defers its resume to `MaybeEnter`.
            let no_area = w.fact(c.fact).blueprint.is_some_and(|g| unavailable_for(w, g, None));
            if w.fact(c.fact).is_active && processed(w, c) {
                if no_area || etude_read_only(w, c) {
                    w.component_mut(c).need_resume = true;
                } else {
                    phase(w, c, Phase::Resume);
                }
            }
        }
        Hook::Recalculate => {
            // `ComponentRuntime.OnRecalculate` → the delegate's; the base delegate method
            // runs its `OnTurnOff` then `OnTurnOn`.
            if overrides(w, c, Hook::Recalculate) {
                delegate_hook(w, c, h);
            } else {
                delegate_hook(w, c, Hook::TurnOff);
                delegate_hook(w, c, Hook::TurnOn);
            }
        }
        _ => delegate_hook(w, c, h),
    }
}

fn overrides(w: &World, c: CompRef, h: Hook) -> bool {
    w.component(c).ty().is_some_and(|t| schema::ty(t).fact_hooks & h.bit() != 0)
}

/// `ComponentRuntime.On*` → `Delegate.On*`: a no-op unless the delegate type overrides it.
fn delegate_hook(w: &mut World, c: CompRef, h: Hook) {
    if !overrides(w, c, h) {
        return;
    }
    let name = short(w, c);
    let reg = w.registry.clone();
    match reg.component(name) {
        Some(l) if l.implemented() & h.bit() != 0 => {
            w.coverage.executed(format!("{name}.{}", h.name()));
            l.hook(w, c, h);
        }
        _ => w.coverage.unsupported(format!("{name}.{}", h.name())),
    }
}

/// `IsLinkedAreaUnavailable(etude, area)`: some etude up the parent chain has a linked
/// area part that isn't `area`.
fn unavailable_for(w: &World, etude: Guid, area: Option<Guid>) -> bool {
    let mut cur = Some(etude);
    let mut n = 0;
    while let Some(g) = cur {
        let Some(b) = w.content.get(g) else { return false };
        if let Some(linked) = b.view().guid("m_LinkedAreaPart") {
            let ok = area.is_some_and(|a| {
                crate::narrative::etudes::is_linked_area_part_of(w, &b, linked, a)
            });
            if !ok {
                return true;
            }
        }
        cur = b.view().guid("m_Parent");
        n += 1;
        if n > 64 {
            return false;
        }
    }
    false
}

fn linked_area_unavailable(w: &World, c: CompRef) -> bool {
    w.fact(c.fact).blueprint.is_some_and(|g| unavailable_for(w, g, w.player.area_part))
}

/// `LinkedAreaUnavailableOnExit`: unavailable now and for `AreaPartBeingExited` (null
/// unless an area is being left; leaving areas isn't modelled).
fn linked_area_unavailable_on_exit(w: &World, c: CompRef) -> bool {
    linked_area_unavailable(w, c)
        && w.fact(c.fact).blueprint.is_some_and(|g| unavailable_for(w, g, None))
}

fn etude_read_only(w: &World, c: CompRef) -> bool {
    w.fact(c.fact).blueprint.is_some_and(|g| crate::narrative::etudes::is_read_only(w, g))
}

/// `EtudeBracketRuntime.MaybeEnter()`.
fn maybe_enter(w: &mut World, c: CompRef) {
    if linked_area_unavailable(w, c) || etude_read_only(w, c) {
        return;
    }
    if processed(w, c) {
        if w.component(c).need_resume {
            w.component_mut(c).need_resume = false;
            phase(w, c, Phase::Resume);
        }
    } else {
        set_processed(w, c, true);
        phase(w, c, Phase::Enter);
    }
}

/// An event delivered to the runtime itself ([`crate::rulebook::bus::Sub::Runtime`]) when
/// the delegate doesn't implement the interface.
pub fn runtime_event(w: &mut World, c: CompRef, iface: &str) {
    let Some(ty) = w.component(c).ty() else { return };
    match iface {
        "IEtudesUpdateHandler" | "IAreaActivationHandler" if is_bracket(ty) => {
            w.coverage.executed(format!("EtudeBracketRuntime.{iface}"));
            maybe_enter(w, c);
        }
        "ITimedEvent" if crate::content::is_a(ty, "EtudeInvokeActionsDelayed") => {
            w.coverage.executed("EtudeInvokeActionDelayedData.ITimedEvent");
            delayed_time_passed(w, c);
        }
        _ => w.coverage.unsupported(format!("{} runtime: {iface}", short(w, c))),
    }
}

/// `Exit()`.
fn exit(w: &mut World, c: CompRef) {
    if linked_area_unavailable_on_exit(w, c) {
        w.log("error", "EtudeBracketTrigger.OnExit: skip, because of linked area is unavailable");
        return;
    }
    phase(w, c, Phase::Exit);
}

/// Which bracket method runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Enter,
    Exit,
    Resume,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Self::Enter => "OnEnter",
            Self::Exit => "OnExit",
            Self::Resume => "OnResume",
        }
    }
}

/// `Enter()`/`Exit()`/`Resume()`: the delegate's method in the runtime's event context;
/// exceptions are caught and logged (`PFLog.Etudes.ExceptionWithReport`).
fn phase(w: &mut World, c: CompRef, p: Phase) {
    let name = short(w, c);
    w.events.mutation(
        "etude_bracket",
        json!({ "etude": w.fact(c.fact).blueprint.map(|g| g.to_string()), "component": w.component(c).name, "class": name, "phase": p.name() }),
    );
    let r = run_phase(w, c, name, p);
    match r {
        Ok(()) => {}
        Err(e) => {
            w.log("error", format!("{name}.{}: {e:?}", p.name()));
        }
    }
}

type R = crate::elements::Result<()>;

fn run_phase(w: &mut World, c: CompRef, name: &'static str, p: Phase) -> R {
    use Phase::{Enter, Exit, Resume};
    let Some((bp, i)) = settings(w, c) else { return Ok(()) };
    let objs = bp.components_array();
    let v = View(objs[i]);
    let done = |w: &mut World| w.coverage.executed(format!("{name}.{}", p.name()));
    match (name, p) {
        // Area and player `CountableFlag`s: `OnResume` is `OnEnter`.
        ("EtudeDisableCraft", Enter | Resume) => retain(w, "player.CraftManager.Disabled"),
        ("EtudeDisableCraft", Exit) => release(w, "player.CraftManager.Disabled"),
        ("EtudeIgnorePartyEncumbrance", Enter | Resume) => retain(w, "area.IgnorePartyEncumbrance"),
        ("EtudeIgnorePartyEncumbrance", Exit) => release(w, "area.IgnorePartyEncumbrance"),
        ("EtudeIgnorePersonalEncumbrance", Enter | Resume) => {
            retain(w, "area.IgnorePersonalEncumbrance");
        }
        ("EtudeIgnorePersonalEncumbrance", Exit) => release(w, "area.IgnorePersonalEncumbrance"),
        ("EtudePeacefulZone", Enter | Resume) => retain(w, "area.Peaceful"),
        ("EtudePeacefulZone", Exit) => release(w, "area.Peaceful"),
        ("EtudeCorruptionFreeZone", Enter | Resume) => {
            if v.bool("m_ClearAllCorruption") {
                // `Player.Corruption.Clear()`: corruption isn't modelled.
                w.coverage.unsupported("EtudeCorruptionFreeZone: Corruption.Clear");
            }
            retain(w, "player.Corruption.Disabled");
        }
        ("EtudeCorruptionFreeZone", Exit) => release(w, "player.Corruption.Disabled"),
        // Unit flags behind an evaluator: `if ((bool)Unit && Unit.CanEvaluate())`.
        ("EtudeBracketMakePassive", Enter | Resume) => {
            unit_flag(w, &bp, v.obj("Unit"), "Passive", true)?
        }
        ("EtudeBracketMakePassive", Exit) => unit_flag(w, &bp, v.obj("Unit"), "Passive", false)?,
        ("EtudeBracketPreventDirectControl", Enter | Resume) => {
            unit_flag(w, &bp, v.obj("Unit"), "PreventDirectControl", true)?;
        }
        ("EtudeBracketPreventDirectControl", Exit) => {
            unit_flag(w, &bp, v.obj("Unit"), "PreventDirectControl", false)?;
        }
        // `MarkUnitEssential`: no `OnResume` (the mark is the descriptor's).
        ("EtudeBracketMarkUnitEssential", Enter) => {
            let target = crate::elements::try_get_unit(w, &bp, v.obj("Target"))?;
            mark_essential(w, c, target);
        }
        ("EtudeBracketMarkUnitEssential", Exit) => {
            if let Some(u) = essential_ref(w, c) {
                release(w, &unit_key(w, u, "Essential"));
            }
            set_data(w, c, "UnitRef", Value::Null);
        }
        ("EtudeBracketMarkUnitEssential", Resume) => {}
        // Interaction overrides on a unit (`UnitPartInteractions`): `Unit.GetValue()` throws
        // when it can't evaluate (caught by the runtime).
        (
            "EtudeBracketOverrideActionsOnClick"
            | "EtudeBracketOverrideBark"
            | "EtudeBracketOverrideDialog",
            Enter | Resume,
        ) => {
            let u = crate::elements::get_unit(w, &bp, v.obj("Unit"))?;
            ensure_part(w, u, "UnitPartInteractions");
            w.player.interactions.push((u, c));
        }
        (
            "EtudeBracketOverrideActionsOnClick"
            | "EtudeBracketOverrideBark"
            | "EtudeBracketOverrideDialog",
            Exit,
        ) => {
            let u = crate::elements::get_unit(w, &bp, v.obj("Unit"))?;
            ensure_part(w, u, "UnitPartInteractions");
            if let Some(p) = w.player.interactions.iter().position(|x| *x == (u, c)) {
                w.player.interactions.remove(p);
            }
        }
        // Area overrides of the camp and skip-time actions.
        ("EtudeBracketCampingAction", Enter | Resume) => {
            set_override(w, "CampingOverride", Some(c))
        }
        ("EtudeBracketCampingAction", Exit) => set_override(w, "CampingOverride", None),
        ("EtudeBracketSkipTimeAction", Enter | Resume) => {
            set_override(w, "SkipTimeOverride", Some(c))
        }
        ("EtudeBracketSkipTimeAction", Exit) => set_override(w, "SkipTimeOverride", None),
        ("EtudeBracketEnableTutorials", Enter | Resume | Exit) => {
            for t in v.guids("m_Tutorials") {
                crate::narrative::tutorial::enable_by_etude(w, t, p != Exit);
            }
        }
        ("EtudeBracketPinCompanionInParty", Enter | Resume) => pin_companion(w, &bp, v, true)?,
        ("EtudeBracketPinCompanionInParty", Exit) => pin_companion(w, &bp, v, false)?,
        ("EtudeBracketSetCompanionPosition", Enter | Resume) => set_companion_position(w, v),
        ("EtudeBracketSetCompanionPosition", Exit) => {
            if companion_of(w, v).is_some_and(|u| {
                w.player
                    .counters
                    .get(&unit_key(w, u, "IsEtudeBracketSetCompanionPosition"))
                    .is_some_and(|n| *n > 0)
            }) {
                w.coverage.unsupported(
                    "EtudeBracketSetCompanionPosition.OnExit: companion spawner reset",
                );
            }
        }
        // `EtudeBracketFollowUnit`: units follow a leader (`UnitPartFollowUnit`: movement, T1).
        // `Data` has no `[JsonProperty]` members, so nothing of it is saved.
        ("EtudeBracketFollowUnit", Enter | Resume) => {
            // `Data.Leader = …` creates `m_Data`.
            data_obj(w, c);
            let leader = match v.obj("Leader") {
                Some(e) => Some(crate::elements::get_unit(w, &bp, Some(e))?),
                None => w.main_character,
            };
            let Some(leader) = leader else {
                w.log("error", "EtudeBracketFollowUnit.OnEnter: leader is null");
                return Ok(());
            };
            if v.bool("UseSummonPool") {
                w.coverage.unsupported("EtudeBracketFollowUnit: summon pool");
                return Ok(());
            }
            let u = crate::elements::get_unit(w, &bp, v.obj("Unit"))?;
            ensure_part(w, u, "UnitPartFollowUnit");
            // `UnitPartFollowUnit.Init` (once per part). The following itself is movement (T1).
            if !w.player.follow_leaders.contains_key(&u) {
                let id = w.entity(leader).unique_id.clone();
                w.player.follow_leader_refs.insert(u, id);
                follow_init(w, u, leader);
            }
            w.coverage.stub("UnitPartFollowUnit (following the leader: movement)");
            let rt = &mut w.component_mut(c).runtime;
            rt.insert("$Leader".into(), json!(leader.0));
            let followers = rt.entry("$Followers").or_insert_with(|| json!([]));
            if let Some(a) = followers.as_array_mut() {
                a.push(json!(u.0));
            }
        }
        ("EtudeBracketFollowUnit", Exit) => {
            data_obj(w, c);
            let followers: Vec<u32> = w
                .component(c)
                .runtime
                .get("$Followers")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_u64().and_then(|n| u32::try_from(n).ok()))
                        .collect()
                })
                .unwrap_or_default();
            for f in followers {
                let u = EntityIdx(f);
                // `Remove<UnitPartFollowUnit>()`: the part turns off (`OnTurnOff`).
                follow_release(w, u);
                w.player.follow_leader_refs.remove(&u);
                w.entity_mut(u).parts.retain(|p| p.clr != "UnitPartFollowUnit");
            }
            // `UnitPartFollowedByUnits.Cleanup()`: drops followers that no longer follow
            // (already released above).
            let rt = &mut w.component_mut(c).runtime;
            rt.remove("$Leader");
            rt.remove("$Followers");
        }
        // `CapitalCompanionLogic`: the capital mode and the companions' placement
        // (`crate::companion_spawner`).
        ("CapitalCompanionLogic", Enter) => {
            crate::companion_spawner::capital_on_enter(w, v.bool("m_RestAllRemoteCompanions"));
        }
        ("CapitalCompanionLogic", Resume) => crate::companion_spawner::place_all_companions(w),
        ("CapitalCompanionLogic", Exit) => crate::companion_spawner::capital_on_exit(w),
        // Presentation: Wwise events and objects, sound banks, music, weather.
        (
            "EtudeBracketAudioEvents"
            | "EtudeBracketAudioObjects"
            | "EtudeBracketEnsureAudio"
            | "EtudeBracketMusic"
            | "EtudeBracketOverrideWeatherInclemency"
            | "EtudeBracketOverrideWeatherProfile",
            _,
        ) => {
            w.coverage.stub(format!("{name}.{} (audio, music or weather)", p.name()));
            return Ok(());
        }
        _ => {
            w.coverage.unsupported(format!("{name}.{}", p.name()));
            return Ok(());
        }
    }
    done(w);
    Ok(())
}

/// `UnitPartFollowUnit.Init` / `OnTurnOn` when not initialized: `Owner.Sleepless.Retain()` and
/// the leader's `UnitPartFollowedByUnits.AddFollower(Owner)`.
pub(crate) fn follow_init(w: &mut World, u: EntityIdx, leader: EntityIdx) {
    w.player.follow_leaders.insert(u, leader);
    retain(w, &unit_key(w, u, "Sleepless"));
    ensure_part(w, leader, "UnitPartFollowedByUnits");
    let k = format!("unit:{}.Followers:{}", w.entity(leader).unique_id, w.entity(u).unique_id);
    retain(w, &k);
}

/// `UnitPartFollowUnit.OnTurnOff` when initialized: `Sleepless.Release` and the leader's
/// `RemoveFollower`; the leader's `UnitPartFollowedByUnits` goes when it has no followers left.
pub(crate) fn follow_release(w: &mut World, u: EntityIdx) {
    let Some(l) = w.player.follow_leaders.remove(&u) else { return };
    release(w, &unit_key(w, u, "Sleepless"));
    let prefix = format!("unit:{}.Followers:", w.entity(l).unique_id);
    let k = format!("{prefix}{}", w.entity(u).unique_id);
    release(w, &k);
    let any = w.player.counters.iter().any(|(k, n)| k.starts_with(&prefix) && *n > 0);
    if !any {
        w.entity_mut(l).parts.retain(|p| p.clr != "UnitPartFollowedByUnits");
    }
}

fn unit_key(w: &World, u: EntityIdx, flag: &str) -> String {
    format!("unit:{}.{flag}", w.entity(u).unique_id)
}

/// `CountableFlag.Retain()`.
pub fn retain(w: &mut World, key: &str) {
    let n = w.player.counters.entry(key.to_owned()).or_default();
    *n += 1;
    let v = *n;
    w.events.mutation("flag_retain", json!({ "flag": key, "count": v }));
}

/// `CountableFlag.Release()` (never below zero).
pub fn release(w: &mut World, key: &str) {
    let n = w.player.counters.entry(key.to_owned()).or_default();
    *n = (*n - 1).max(0);
    let v = *n;
    w.events.mutation("flag_release", json!({ "flag": key, "count": v }));
}

fn unit_flag(w: &mut World, bp: &Rc<Blueprint>, ev: Option<View<'_>>, flag: &str, on: bool) -> R {
    if ev.is_none() {
        return Ok(());
    }
    if let Some(u) = crate::elements::try_get_unit(w, bp, ev)? {
        let key = unit_key(w, u, flag);
        if on {
            retain(w, &key);
        } else {
            release(w, &key);
        }
    }
    Ok(())
}

fn ensure_part(w: &mut World, u: EntityIdx, part: &'static str) {
    if !w.entity(u).parts.iter().any(|p| p.clr == part) {
        w.entity_mut(u).parts.push(crate::world::Part { clr: part });
    }
}

fn set_override(w: &mut World, which: &'static str, c: Option<CompRef>) {
    let cur = w.player.area_overrides.get(which).copied().flatten();
    match (cur, c) {
        (Some(_), Some(_)) => w.log("error", format!("Overriding {which}: already overridden")),
        (None, None) => {
            w.log("error", format!("Overriding {which}: not overridden when turning off"))
        }
        _ => {}
    }
    w.player.area_overrides.insert(which, c);
}

/// `new TData()` for the data classes the ports write: every member at its default.
fn new_data(w: &World, c: CompRef) -> Value {
    match short(w, c) {
        // `EtudePlayTriggerData`.
        "EtudePlayTrigger" => {
            json!({ "AlreadyTriggered": false, "AlreadyProcessedActivation": false })
        }
        // `EtudeBracketMarkUnitEssential.ComponentData`.
        "EtudeBracketMarkUnitEssential" => json!({ "UnitRef": null }),
        _ => json!({}),
    }
}

fn data_obj(w: &mut World, c: CompRef) -> &mut serde_json::Map<String, Value> {
    // `Data` creates `new TData()` when `m_Data` is null.
    let fresh = new_data(w, c);
    let d = w.component_mut(c).data.get_or_insert(fresh);
    if !d.is_object() {
        *d = json!({});
    }
    d.as_object_mut().expect("object")
}

fn set_data(w: &mut World, c: CompRef, k: &str, v: Value) {
    data_obj(w, c).insert(k.into(), v);
}

fn data_bool(w: &World, c: CompRef, k: &str) -> bool {
    w.component(c).data.as_ref().and_then(|d| d.get(k)).and_then(Value::as_bool) == Some(true)
}

/// `Data.UnitRef.Entity` of `MarkUnitEssential` (an `EntityRef`, saved as its id).
fn essential_ref(w: &World, c: CompRef) -> Option<EntityIdx> {
    let v = w.component(c).data.as_ref()?.get("UnitRef")?;
    let id = match v {
        Value::String(s) => s.clone(),
        Value::Object(o) => o.get("m_UniqueId").and_then(Value::as_str)?.to_owned(),
        _ => return None,
    };
    w.by_id.get(&id).copied()
}

/// `MarkUnit(unit)`.
fn mark_essential(w: &mut World, c: CompRef, u: Option<EntityIdx>) {
    let Some(u) = u else { return };
    if essential_ref(w, c).is_some() {
        return;
    }
    retain(w, &unit_key(w, u, "Essential"));
    let id = w.entity(u).unique_id.clone();
    set_data(w, c, "UnitRef", Value::String(id));
}

/// `EtudeBracketMarkUnitEssential.HandleUnitSpawned(unit)`.
pub fn essential_unit_spawned(w: &mut World, c: CompRef, unit: EntityIdx) -> R {
    if essential_ref(w, c).is_some() {
        return Ok(());
    }
    let Some((bp, i)) = settings(w, c) else { return Ok(()) };
    let objs = bp.components_array();
    let target = crate::elements::try_get_unit(w, &bp, View(objs[i]).obj("Target"))?;
    if target == Some(unit) {
        mark_essential(w, c, Some(unit));
    }
    Ok(())
}

/// `EtudeBracketPinCompanionInParty.OnEnter`/`OnExit`, with `UpdateParty` on enter. The
/// party change `UpdateParty` can make (companions joining or leaving, the group changer
/// UI) isn't ported: reported if it would change any companion's state.
fn pin_companion(w: &mut World, bp: &Rc<Blueprint>, v: View<'_>, on: bool) -> R {
    let u = crate::elements::get_unit(w, bp, v.obj("Unit"))?;
    if !w.entity(u).parts.iter().any(|p| p.clr == "UnitPartCompanion") {
        w.log("error", "Can't pin unit in party: not companion");
        return Ok(());
    }
    let key = unit_key(w, u, "Pinned");
    if !on {
        release(w, &key);
        return Ok(());
    }
    retain(w, &key);
    // `UpdateParty()`: Party ∪ pinned non-party companions, by weight (main character 0,
    // pinned 1, others 2; `OrderBy` is stable), the first six.
    let lists = crate::narrative::party::character_lists(w);
    let pinned = |w: &World, x: EntityIdx| {
        w.player.counters.get(&unit_key(w, x, "Pinned")).is_some_and(|n| *n > 0)
    };
    let mut source: Vec<EntityIdx> = lists.party.clone();
    for x in &lists.all_characters {
        let unit = w.unit(*x);
        if unit.is_some_and(|y| {
            !y.is_pet && y.companion_state.is_some() && y.companion_state != Some(2)
        }) && pinned(w, *x)
        {
            source.push(*x);
        }
    }
    let weight = |w: &World, x: EntityIdx| {
        if Some(x) == w.main_character {
            0
        } else if pinned(w, x) {
            1
        } else {
            2
        }
    };
    source.sort_by_key(|x| weight(w, *x));
    source.truncate(6);
    for x in lists.all_characters {
        let Some(unit) = w.unit(x) else { continue };
        if unit.is_pet || unit.companion_state.is_none() {
            continue;
        }
        let in_party = unit.companion_state == Some(2);
        if source.contains(&x) != in_party {
            w.coverage.unsupported("EtudeBracketPinCompanionInParty.UpdateParty: party change");
        }
    }
    Ok(())
}

fn companion_of(w: &World, v: View<'_>) -> Option<EntityIdx> {
    let bp = v.guid("m_Companion")?;
    // `Player.AllCharacters.FirstItem(c => m_Companion.Is(c.Blueprint))`.
    let order: Vec<EntityIdx> = w
        .player
        .party_characters
        .iter()
        .chain(w.player.cross_scene_units.iter())
        .copied()
        .collect();
    order.into_iter().find(|u| w.unit(*u).and_then(|x| x.blueprint) == Some(bp))
}

/// `EtudeBracketSetCompanionPosition.OnEnter`: places a companion who isn't in the party
/// at a scene locator. Locators aren't hydrated: reported when it would act.
fn set_companion_position(w: &mut World, v: View<'_>) {
    let Some(u) = companion_of(w, v) else { return };
    ensure_part(w, u, "UnitPartCompanion");
    if w.player
        .counters
        .get(&unit_key(w, u, "IsEtudeBracketSetCompanionPosition"))
        .is_some_and(|n| *n > 0)
    {
        return;
    }
    let state = w.unit(u).and_then(|x| x.companion_state);
    // `m_IgnoreWhenEx` defaults to true.
    let ignore_ex = if v.has("m_IgnoreWhenEx") { v.bool("m_IgnoreWhenEx") } else { true };
    let acts =
        (state != Some(2) || w.player.capital_party_mode()) && (state != Some(3) || !ignore_ex);
    if acts {
        w.coverage.unsupported("EtudeBracketSetCompanionPosition: locator placement");
    }
}

// ---- EtudePlayTrigger ---------------------------------------------------------------------

/// `EtudePlayTrigger.OnActivate`: `Data.AlreadyProcessedActivation = false`.
fn play_trigger_hook(w: &mut World, c: CompRef, h: Hook) {
    if h == Hook::Activate {
        set_data(w, c, "AlreadyProcessedActivation", Value::Bool(false));
    }
}

/// `OnEtudesUpdate` / `OnAreaActivated` (the delegate's own handlers).
fn play_trigger_event(w: &mut World, c: CompRef, iface: &str) {
    if iface == "IAreaActivationHandler" {
        let on_load = settings(w, c)
            .is_some_and(|(bp, i)| View(bp.components_array()[i]).bool("IsActivateOnLoadArea"));
        if on_load {
            set_data(w, c, "AlreadyProcessedActivation", Value::Bool(false));
        }
    }
    maybe_trigger(w, c);
}

/// `MaybeTrigger()`.
fn maybe_trigger(w: &mut World, c: CompRef) {
    if data_bool(w, c, "AlreadyProcessedActivation") {
        return;
    }
    // Area mechanics of this etude in the loaded area whose scene isn't loaded yet.
    if let Some(eb) = etude_bp(w, c) {
        for m in eb.view().guids("m_AddedAreaMechanics") {
            let area = w.content.get(m).and_then(|mb| mb.view().guid("Area"));
            if area.is_some() && area == w.player.area && !w.player.loaded_mechanics.contains(&m) {
                return;
            }
        }
    }
    set_data(w, c, "AlreadyProcessedActivation", Value::Bool(true));
    let Some((bp, i)) = settings(w, c) else { return };
    let objs = bp.components_array();
    let v = View(objs[i]);
    let once = v.bool("m_Once");
    if (!once || !data_bool(w, c, "AlreadyTriggered"))
        && crate::elements::check_conditions(w, &bp, v.obj("Conditions"))
    {
        w.events.mutation(
            "etude_play_trigger",
            json!({ "etude": w.fact(c.fact).blueprint.map(|g| g.to_string()), "component": w.component(c).name }),
        );
        crate::elements::run_actions(w, &bp, v.obj("Actions"));
        set_data(w, c, "AlreadyTriggered", Value::Bool(true));
    }
}

// ---- EtudeInvokeActionsDelayed ------------------------------------------------------------

/// `EtudeInvokeActionDelayedData` (its own runtime): `OnInitialize`, `OnTurnOn`,
/// `OnTurnOff`.
fn delayed_hook(w: &mut World, c: CompRef, h: Hook) {
    match h {
        Hook::Initialize => {
            let days =
                settings(w, c).map_or(0, |(bp, i)| View(bp.components_array()[i]).i32("m_Days"));
            let ticks = i64::from(days) * 864_000_000_000;
            w.component_mut(c).runtime.insert("TimeRemaining".into(), json!(ticks));
        }
        Hook::TurnOn => {
            let now = w.clock.game_time.ticks();
            w.component_mut(c).runtime.insert("LastTickTime".into(), json!(now));
        }
        Hook::TurnOff => delayed_time_passed(w, c),
        _ => {}
    }
}

/// `HandleTimePassed()`.
fn delayed_time_passed(w: &mut World, c: CompRef) {
    let executed = w.component(c).runtime.get("Executed").and_then(Value::as_bool) == Some(true);
    let playing = w.fact(c.fact).is_active;
    if executed || !playing {
        return;
    }
    let now = w.clock.game_time.ticks();
    let rt = &w.component(c).runtime;
    let last = rt.get("LastTickTime").and_then(Value::as_i64).unwrap_or(now);
    let remaining = rt.get("TimeRemaining").and_then(Value::as_i64).unwrap_or(0) - (now - last);
    let rt = &mut w.component_mut(c).runtime;
    rt.insert("LastTickTime".into(), json!(now));
    rt.insert("TimeRemaining".into(), json!(remaining));
    if remaining <= 0 {
        if let Some((bp, i)) = settings(w, c) {
            let objs = bp.components_array();
            crate::elements::run_actions(w, &bp, View(objs[i]).obj("m_ActionList"));
        }
        w.component_mut(c).runtime.insert("Executed".into(), Value::Bool(true));
    }
}

/// Bracket handlers the delegates declare (`IUnitSpawnHandler` and the like).
fn essential_event(w: &mut World, c: CompRef, iface: &str) {
    // `EntityCreationController` raises `HandleUnitSpawned` through both `IUnitHandler`
    // (which extends `IUnitSpawnHandler`) and `IUnitSpawnHandler`; `HandleUnitDestroyed`
    // and `HandleUnitDeath` are empty.
    match (iface, w.bus.call.last().copied()) {
        ("IUnitHandler" | "IUnitSpawnHandler", Some(("HandleUnitSpawned", Some(u)))) => {
            if let Err(e) = essential_unit_spawned(w, c, u) {
                w.log("error", format!("EtudeBracketMarkUnitEssential.HandleUnitSpawned: {e:?}"));
            }
        }
        ("IUnitHandler", Some(("HandleUnitDestroyed" | "HandleUnitDeath", _))) => {}
        _ => w
            .coverage
            .unsupported(format!("EtudeBracketMarkUnitEssential: {iface} (method unknown)")),
    }
}

fn set_companion_position_event(w: &mut World, c: CompRef, iface: &str) {
    let call = w.bus.call.last().copied();
    match (iface, call) {
        ("IPartyHandler", Some(("HandleAddCompanion" | "HandleCompanionActivated", Some(u)))) => {
            let Some((bp, i)) = settings(w, c) else { return };
            let objs = bp.components_array();
            let v = View(objs[i]);
            if v.guid("m_Companion").is_some()
                && w.unit(u).and_then(|x| x.blueprint) == v.guid("m_Companion")
            {
                phase(w, c, Phase::Enter);
            }
        }
        ("IPartyHandler", Some(("HandleCompanionRemoved", _))) => {}
        // `HandleCapitalModeChanged`: `OnEnter()` when the capital mode is on.
        ("IPartyHandler", Some(("HandleCapitalModeChanged", _))) => {
            if w.player.capital_party_mode() {
                phase(w, c, Phase::Enter);
            }
        }
        _ => w.coverage.unsupported(format!("EtudeBracketSetCompanionPosition: {iface}")),
    }
}

/// `DisableMountRiding.OnTurnOn`: each party unit dismounts and gets the condition;
/// `OnTurnOff`: every character loses one count.
fn disable_mount_riding_hook(w: &mut World, _c: CompRef, h: Hook) {
    use crate::unit_state::{add_condition, cond::DISABLE_MOUNT_RIDING, remove_condition};
    match h {
        Hook::TurnOn => {
            for u in crate::narrative::party::party(w) {
                dismount(w, u);
                add_condition(w, u, DISABLE_MOUNT_RIDING, None);
            }
        }
        Hook::TurnOff => {
            for u in crate::narrative::party::character_lists(w).all_characters {
                remove_condition(w, u, DISABLE_MOUNT_RIDING, false);
            }
        }
        _ => {}
    }
}

/// `RiderPart?.Dismount()`: riding isn't modelled, so a rider part is reported.
fn dismount(w: &mut World, u: EntityIdx) {
    if w.entity(u).parts.iter().any(|p| p.clr == "UnitPartRider") {
        w.coverage.unsupported("DisableMountRiding: UnitPartRider.Dismount");
    }
}

/// `DisableMountRiding.HandleAddCompanion`: a new non-pet companion gets the condition
/// unless it has it; the other `IPartyHandler` methods are empty.
fn disable_mount_riding_event(w: &mut World, _c: CompRef, iface: &str) {
    use crate::unit_state::{add_condition, cond::DISABLE_MOUNT_RIDING, has_condition};
    match (iface, w.bus.call.last().copied()) {
        ("IPartyHandler", Some(("HandleAddCompanion", Some(u)))) => {
            let pet = w.unit(u).is_some_and(|x| x.is_pet);
            if !pet && !has_condition(w, u, DISABLE_MOUNT_RIDING) {
                add_condition(w, u, DISABLE_MOUNT_RIDING, None);
            }
        }
        (
            "IPartyHandler",
            Some((
                "HandleCompanionActivated" | "HandleCompanionRemoved" | "HandleCapitalModeChanged",
                _,
            )),
        ) => {}
        _ => w.coverage.unsupported(format!("DisableMountRiding: {iface} (method unknown)")),
    }
}

/// Registers the ports whose delegate methods aren't bracket phases.
pub fn register(r: &mut Registry) {
    let mut dmr = Port::new(
        "DisableMountRiding",
        bits(&[Hook::TurnOn, Hook::TurnOff]),
        disable_mount_riding_hook,
    );
    dmr.handlers = &["IPartyHandler"];
    dmr.ev = Some(disable_mount_riding_event);
    r.register(Box::new(dmr));
    let mut play = Port::new("EtudePlayTrigger", bits(&[Hook::Activate]), play_trigger_hook);
    play.handlers = &["IEtudesUpdateHandler", "IAreaActivationHandler"];
    play.ev = Some(play_trigger_event);
    r.register(Box::new(play));
    // A custom runtime: every hook reaches the port; the ones it doesn't list are the
    // base class's no-ops.
    r.register(Box::new(Port::new("EtudeInvokeActionsDelayed", super::all_hooks(), delayed_hook)));
    let mut ess = Port::new("EtudeBracketMarkUnitEssential", 0, |_, _, _| {});
    ess.handlers = &["IUnitHandler", "IUnitSpawnHandler"];
    ess.ev = Some(essential_event);
    r.register(Box::new(ess));
    let mut pos = Port::new("EtudeBracketSetCompanionPosition", 0, |_, _, _| {});
    pos.handlers = &["IPartyHandler"];
    pos.ev = Some(set_companion_position_event);
    r.register(Box::new(pos));
    // `EtudeBracketEnsureAudio.OnDispose`: `OnExit` (sound banks) unless the etude
    // completed: presentation.
    r.register(Box::new(Port::new(
        "EtudeBracketEnsureAudio",
        bits(&[Hook::Dispose]),
        |w, _, _| {
            w.coverage.stub("EtudeBracketEnsureAudio.OnDispose (sound banks)");
        },
    )));
}
