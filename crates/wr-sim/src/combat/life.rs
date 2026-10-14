//! Life, death and what follows (decompiled, not copied): `UnitLifeController`
//! (`Kingmaker.Controllers.Units/UnitLifeController.cs`), `UnitHelplessController`, the
//! dead-owner part of `BuffCollection.Tick` (`UpdateOnDeadOwner`) and the
//! `UnitStatusEffectHelpless` stat penalty (in `unit_state`).
//!
//! The original runs these as controllers once per frame, in the game mode's registration
//! order (`GameModesFactory.Initialize`: `UnitBuffsController` ... `UnitLifeController` ...
//! `UnitHelplessController`). The driven replay runs [`buffs_tick`], [`life_tick`] and
//! [`helpless_tick`] at those phases of each recorded frame (`replay::Phase`).
//!
//! Not modelled (reported where reached): the controllers that listen to a death
//! (`UnitGroupsController`, `SummonPoolsManager`, `UnitImpatienceController`, the turn
//! controller's queue), the unit view's death handling, `UnitCombatState.LeaveCombat`'s
//! command, AI and view parts.

use crate::combat::units;
use crate::csharp::arith;
use crate::rulebook::bus;
use crate::stats::types::StatType;
use crate::unit_state::{cond, has_condition};
use crate::world::{EntityIdx, World};

/// `UnitLifeState`.
pub mod state {
    pub const CONSCIOUS: i32 = 0;
    pub const UNCONSCIOUS: i32 = 1;
    pub const DEAD: i32 = 2;
}

/// `MechanicsFeatureType.MythicHardToKill`, `Immortality`, `AllowDyingCondition`.
const MYTHIC_HARD_TO_KILL: i64 = 69;
const IMMORTALITY: i64 = 44;
const ALLOW_DYING_CONDITION: i64 = 45;

/// `Game.Instance.State.Units` in order (the entity pool): every unit entity.
pub(crate) fn all_units(w: &World) -> Vec<EntityIdx> {
    (0..w.entities.len())
        .filter_map(|i| u32::try_from(i).ok().map(EntityIdx))
        .filter(|e| w.unit(*e).is_some() && w.entity(*e).clr == "UnitEntityData")
        .collect()
}

/// `SleepingUnitsController.ShouldBeSleeping` for what the sim can establish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Awake {
    Yes,
    No,
    Unknown,
}

/// `BlueprintArea.HasParts`: a part reference that resolves.
fn area_has_parts(w: &World) -> bool {
    w.player
        .area
        .and_then(|g| w.content.get(g))
        .is_some_and(|a| a.view().guids("m_Parts").iter().any(|p| w.content.get(*p).is_some()))
}

/// `CurrentlyLoadedArea.HideParty`: a global map, a settlement or a tactical combat area.
fn area_hides_party(w: &World) -> bool {
    w.player.area.and_then(|g| w.content.get(g)).is_some_and(|a| {
        a.view().bool("IsGlobalMap")
            || crate::content::is_a(a.obj.ty, "BlueprintTacticalCombatArea")
            || crate::content::is_a(a.obj.ty, "SettlementBlueprintArea")
    })
}

/// `ShouldBeSleeping(unit)` in the C# order: not in the game, a fake blueprint or a suppressed
/// entity (never saved: runtime only; [`update_awake`] reports an area whose parts could
/// suppress) sleeps; a companion sleeps while the area hides the party and is awake in a pit;
/// then `AwakeTimer`/`FadeAwakeTimer` keep a unit awake: `UnitEntityData.Wake` sets them, and so
/// does every `Position` change, a memory addition (`UnitMemoryController`) and the view (prone,
/// fading), none of which the sim tracks, so a unit the later checks would put to sleep (a
/// finally dead one) is *unknown*, not asleep; a sleepless unit or one in combat is awake; an extra unit depends
/// on the camera; a party member isn't in fog of war so is awake; anything else depends on fog
/// of war, cutscene control and commands: unknown.
fn awake_state(w: &World, u: EntityIdx, party: &[EntityIdx]) -> Awake {
    let Some(x) = w.unit(u) else { return Awake::No };
    if !w.entity(u).is_in_game {
        return Awake::No;
    }
    let fake = x.blueprint.and_then(|g| w.content.get(g)).is_some_and(|b| b.view().bool("IsFake"));
    if fake {
        return Awake::No;
    }
    if has_part(w, u, "UnitPartCompanion") {
        if area_hides_party(w) {
            return Awake::No;
        }
        if has_part(w, u, "UnitPartInPit") {
            return Awake::Yes;
        }
    }
    if x.finally_dead {
        // Asleep unless its `AwakeTimer` still runs (not tracked).
        return Awake::Unknown;
    }
    // `unit.Sleepless` (a `CountableFlag`: etudes, `UnitPartFollowUnit`).
    let sleepless = w
        .player
        .counters
        .get(&format!("unit:{}.Sleepless", w.entity(u).unique_id))
        .is_some_and(|n| *n > 0);
    if sleepless || x.combat.in_combat {
        return Awake::Yes;
    }
    if x.is_extra {
        return Awake::Unknown;
    }
    if party.contains(&u) { Awake::Yes } else { Awake::Unknown }
}

/// `SleepingUnitsController.Tick`'s `AwakeUnits` snapshot for the frame, as far as it can be
/// established ([`awake_state`]); the units whose state is unknown are kept apart, and a
/// controller that would change one of them reports it ([`unknown_awake_would_change`]).
pub fn update_awake(w: &mut World) {
    let party = crate::narrative::party::character_lists(w).party_and_pets;
    // `SuppressEntitiesController` suppresses entities outside the current area part (by
    // position), which puts them to sleep first: with parts, no unit's state is established.
    let parts = area_has_parts(w);
    if parts {
        w.coverage.unsupported(
            "SleepingUnitsController: the area has parts (SuppressEntitiesController by position)",
        );
    }
    let (mut known, mut unknown) = (Vec::new(), Vec::new());
    for u in all_units(w) {
        let state = awake_state(w, u, &party);
        match if parts && state == Awake::Yes { Awake::Unknown } else { state } {
            Awake::Yes => known.push(u),
            Awake::Unknown => unknown.push(u),
            Awake::No => {}
        }
    }
    w.combat.awake = known;
    w.combat.awake_unknown = unknown;
}

/// A controller that would change a unit whose awake state is unknown can't be run
/// faithfully: reported, and the unit is left alone.
fn unknown_awake_would_change(w: &mut World, controller: &str, u: EntityIdx) {
    w.coverage
        .unsupported(format!("{controller}: would change a unit whose awake state is unknown"));
    let _ = u;
}

/// `UnitLifeController.Tick` (`TickSleeping` false): `ForceTickOnUnit` on the frame's awake
/// units that aren't finally dead (`ShouldTickOnUnit`).
pub fn life_tick(w: &mut World) {
    for u in w.combat.awake.clone() {
        if !w.unit(u).is_some_and(|x| x.finally_dead) {
            force_tick_on_unit(w, u);
        }
    }
    for u in w.combat.awake_unknown.clone() {
        // `ShouldTickOnUnit`: never a finally dead unit, awake or not.
        if !w.unit(u).is_some_and(|x| x.finally_dead) && life_tick_would_change(w, u) {
            unknown_awake_would_change(w, "UnitLifeController", u);
        }
    }
}

/// Whether `ForceTickOnUnit` would do anything to the unit: a new life state, a dead unit that
/// isn't finally dead going through death processing again (`SetLifeState` returns early only
/// for a finally dead one), a forced death raising the damage, the pending death flags being
/// cleared, or a ferocity part being removed.
fn life_tick_would_change(w: &mut World, u: EntityIdx) -> bool {
    let (new, raise) = life_decision(w, u);
    let Some(x) = w.unit(u) else { return false };
    let hp = w.stats(u).v(StatType::HitPoints).modified_value;
    let (cur, fin, flags, dmg) =
        (x.life_state, x.finally_dead, x.marked_for_death || x.force_kill, x.damage);
    new != cur
        || (new == state::DEAD && !fin)
        || raise.is_some_and(|d| d != dmg)
        || flags
        || (dmg < hp && has_part(w, u, "UnitPartFerocity"))
}

fn has_part(w: &World, u: EntityIdx, clr: &str) -> bool {
    w.entity(u).parts.iter().any(|p| p.clr == clr)
}

/// Whether the frame's `UnitLifeController` would take the unit out of combat (a new life
/// state other than conscious for an awake unit in combat that isn't finally dead).
pub(crate) fn life_tick_would_leave_combat(w: &mut World, u: EntityIdx) -> bool {
    let (new, _) = life_decision(w, u);
    let Some(x) = w.unit(u) else { return false };
    w.combat.awake.contains(&u)
        && x.combat.in_combat
        && !x.finally_dead
        && new != state::CONSCIOUS
        && new != x.life_state
}

/// What `ForceTickOnUnit` decides for a unit: the new life state and the damage a forced
/// death raises to (`unit.Damage = Math.Max(unit.Damage, num2)`).
fn life_decision(w: &mut World, u: EntityIdx) -> (i32, Option<i32>) {
    if has_part(w, u, "UnitPartNegativeLevels") {
        w.coverage.unsupported("UnitLifeController: UnitPartNegativeLevels");
    }
    let hp = w.stats(u).v(StatType::HitPoints).modified_value;
    let con = w.stats(u).v(StatType::Constitution).modified_value;
    let hard = super::weapon::has_feature(w, u, MYTHIC_HARD_TO_KILL);
    let num2 = arith::add(hp, arith::mul(con, if hard { 2 } else { 1 }));
    let (marked, force) = w.unit(u).map_or((false, false), |x| (x.marked_for_death, x.force_kill));
    let petrified = has_condition(w, u, cond::PETRIFIED);
    let flag2 = marked || force || petrified;
    // `state.Features.Infallible` (`UnitHealthGuard`) and `IsRegenerating`
    // (`AddEffectRegeneration`) are set by facts the sim doesn't port.
    if w.entity(u).facts.list.iter().any(|f| {
        w.fact(*f).bp.as_ref().is_some_and(|b| {
            b.components_array().iter().any(|o| {
                matches!(
                    wr_content::schema::ty(o.ty).short,
                    "UnitHealthGuard" | "AddEffectRegeneration"
                )
            })
        })
    }) {
        w.coverage.unsupported("UnitLifeController: Infallible or regenerating unit");
    }
    let damage = w.unit(u).map_or(0, |x| x.damage);
    let non_lethal = w.stats(u).v(StatType::DamageNonLethal).modified_value;
    if flag2 {
        return (state::DEAD, Some(damage.max(num2)));
    }
    let new_life = if damage >= num2 {
        state::DEAD
    } else if arith::add(damage, non_lethal) >= hp {
        let ferocity = super::weapon::has_feature(w, u, 17) || super::weapon::has_feature(w, u, 18);
        if ferocity {
            w.coverage.unsupported("UnitLifeController: ferocity");
        }
        let mut allow = super::weapon::has_feature(w, u, ALLOW_DYING_CONDITION);
        if w.unit(u).is_some_and(|x| x.is_pet) {
            let master = w.unit(u).and_then(|x| x.master);
            allow = allow
                && master.is_some_and(|m| super::weapon::has_feature(w, m, ALLOW_DYING_CONDITION));
        }
        if allow { state::UNCONSCIOUS } else { state::DEAD }
    } else {
        state::CONSCIOUS
    };
    (new_life, None)
}

/// `UnitLifeController.ForceTickOnUnit`.
pub fn force_tick_on_unit(w: &mut World, u: EntityIdx) {
    let hp = w.stats(u).v(StatType::HitPoints).modified_value;
    let (new_life, raise) = life_decision(w, u);
    if let Some(d) = raise {
        super::set_damage(w, u, d);
    }
    if w.unit(u).map_or(0, |x| x.damage) < hp && has_part(w, u, "UnitPartFerocity") {
        w.coverage.unsupported("UnitLifeController: UnitPartFerocity");
    }
    set_life_state(w, u, new_life);
    if let Some(x) = w.unit_mut(u) {
        x.marked_for_death = false;
        x.force_kill = false;
    }
}

/// `UnitLifeController.SetLifeState`.
fn set_life_state(w: &mut World, u: EntityIdx, new: i32) {
    let prev = w.unit(u).map_or(0, |x| x.life_state);
    let finally_dead = w.unit(u).is_some_and(|x| x.finally_dead);
    let walking_dead = has_condition(w, u, 44);
    if (prev == new && (new != state::DEAD || finally_dead))
        || (new != state::CONSCIOUS && walking_dead)
    {
        return;
    }
    // `RaiseAboutToChangeLifeState`: subscribers can prevent the change.
    bus::raise_global_call(
        w,
        "IUnitAboutToChangeLifeStateHandler",
        "HandleUnitAboutToChangeLifeState",
        Some(u),
    );
    // `HandleDualCompanionDeath`.
    if has_part(w, u, "UnitPartDualCompanion") {
        w.coverage.unsupported("UnitLifeController: dual companion");
    }
    if w.entity(u).parts.iter().any(|p| p.clr == "SilentDeathUnitPart") {
        w.coverage.unsupported("UnitLifeController: Kill.SilentDeathUnitPart");
    }
    // `state.UpdateLifeState(newLifeState)`.
    if let Some(x) = w.unit_mut(u) {
        x.life_state = new;
    }
    if new == state::DEAD {
        bus::raise_global_call(w, "IUnitSpawnOnlyHandler", "HandleUnitDeathForSpawnOnly", Some(u));
    }
    if new != state::CONSCIOUS {
        let in_combat = w.unit(u).is_some_and(|x| x.combat.in_combat);
        let dead_now = w.unit(u).is_some_and(|x| x.life_state == state::DEAD);
        if in_combat && (!w.combat.turn_based || dead_now) {
            leave_combat(w, u);
        }
        if new == state::DEAD {
            on_unit_death(w, u, prev == state::DEAD);
        }
    } else {
        w.coverage.unsupported("UnitLifeController: return to consciousness");
    }
    let conscious = w.unit(u).is_some_and(|x| x.life_state == state::CONSCIOUS);
    if !conscious {
        // `unit.Commands.InterruptAll()`: the sim runs no commands of its own here.
    }
    if prev != new {
        bus::raise_global_call(w, "IUnitLifeStateChanged", "HandleUnitLifeStateChanged", Some(u));
    }
}

/// The controllers subscribed to `IUnitHandler.HandleUnitDeath` (the facts' handlers are raised
/// through the bus): the turn controller removes the unit from the turn order; the summon pools
/// drop it unless their blueprint keeps dead units; `Player.Stalker` is cleared for its blueprint;
/// the inspection controller queues it. `UnitGroupsController`'s handler is empty;
/// `UnitImpatienceController` (the AI's bother list) and `GameHistoryLog` aren't modelled.
fn death_listeners(w: &mut World, u: EntityIdx) {
    remove_tb_unit(w, u);
    for pool in w.scene.summon_pools.iter().map(|(g, _)| *g).collect::<Vec<_>>() {
        let keeps = w.content.get(pool).is_some_and(|b| b.view().bool("DoNotRemoveDeadUnits"));
        if !keeps {
            crate::scene::summon_pool_unregister(w, pool, u);
        }
    }
    if w.player.stalker.is_some() && w.player.stalker == w.unit(u).and_then(|x| x.blueprint) {
        w.player.stalker = None;
    }
    super::inspect::queue(w, u);
    w.coverage.stub(
        "UnitImpatienceController.HandleUnitDeath: the AI's bother lists (the AI isn't ported)",
    );
}

/// `UnitEntityData.LeaveCombat`. `UnitCombatState.LeaveCombat` (for a unit in combat): out of
/// combat, `--Group.IsInCombat`, `IInCombatHandler.HandleObjectLeaveCombat`, the combat state
/// cleared; then `RemoveAnnoyingBuffs` with its difficulty setting and
/// `IUnitCombatHandler.HandleUnitLeaveCombat`, whose turn-based controller removes the unit from
/// the turn order. Without the turn port (an older recording) the turn controller's part is
/// reported. Commands (the return to the starting position, `InterruptAiCommands`, `CancelStop`)
/// and the view are T1; tactical combat isn't modelled.
pub(crate) fn leave_combat(w: &mut World, u: EntityIdx) {
    if w.combat.turns.on {
        w.combat.leave_log.push_back(u);
    }
    if super::is_in_combat(w, u) {
        if let Some(x) = w.unit_mut(u) {
            x.combat.in_combat = false;
        }
        super::groups::left_combat(w, u);
        w.coverage.stub(
            "UnitCombatState.LeaveCombat: ReturnToStartingPositionIfNeeded (a move command: T1)",
        );
        bus::raise_global_call(w, "IInCombatHandler", "HandleObjectLeaveCombat", Some(u));
        combat_state_clear(w, u);
    }
    if w.entity(u).is_disposed || w.entity(u).disposing {
        return;
    }
    if !w.combat.turns.on {
        w.coverage.unsupported("UnitEntityData.LeaveCombat: commands, view, turn controller");
    }
    w.coverage.stub("UnitEntityData.LeaveCombat: CancelStop, InterruptAiCommands, the view (T1)");
    remove_annoying_buffs(w, u);
    bus::raise_global_call(w, "IUnitCombatHandler", "HandleUnitLeaveCombat", Some(u));
    remove_tb_unit(w, u);
}

/// `UnitCombatState.Clear()` for what the sim keeps: the cooldowns, `Initiative` and `Prepared`
/// (`InitiativeRandom` stays), and the engagement when it's modelled (`engage::clear`). The AI's
/// combat data and targets aren't modelled.
fn combat_state_clear(w: &mut World, u: EntityIdx) {
    let before = w.unit(u).map(|x| x.combat.cooldown).unwrap_or_default();
    if let Some(q) = &mut w.combat.cooldown_clears {
        q.push_back((u, before));
    }
    if let Some(x) = w.unit_mut(u) {
        x.combat.cooldown = super::Cooldowns::default();
        x.combat.initiative.initiative = 0;
        x.combat.prepared = false;
    }
    super::engage::clear(w, u);
}

/// `if (SettingsRoot.Difficulty.RemoveAnnoyingBuffsAfterCombat) Buffs.RemoveAnnoyingBuffs()`:
/// buffs with `RemoveWhenCombatEnded` get a short duration (`SetDuration`, not ported: reported).
fn remove_annoying_buffs(w: &mut World, u: EntityIdx) {
    let setting = w
        .settings
        .difficulty
        .settings
        .raw
        .get("settings.difficulty.remove-annoying-buffs-after-combat")
        .and_then(serde_json::Value::as_bool);
    match setting {
        Some(false) => {}
        None => {
            w.coverage.unsupported("SettingsRoot.Difficulty.RemoveAnnoyingBuffsAfterCombat missing")
        }
        Some(true) => {
            let annoying = w.entity(u).facts.list.iter().any(|f| {
                w.fact(*f).kind == crate::world::FactKind::Buff
                    && w.fact(*f).bp.as_ref().is_some_and(|b| {
                        b.components_array()
                            .iter()
                            .any(|o| wr_content::schema::ty(o.ty).short == "RemoveWhenCombatEnded")
                    })
            });
            if annoying {
                w.coverage.unsupported("BuffCollection.RemoveAnnoyingBuffs: Buff.SetDuration");
            }
        }
    }
}

/// `UnitLifeController.GiveExperience`.
fn give_experience(w: &mut World, u: EntityIdx) {
    let no_xp = w.unit(u).is_some_and(|x| x.no_xp_on_death);
    if no_xp || !units::is_players_enemy(w, u) {
        return;
    }
    // `unit.BonusExperience`: set when the unit was initialized, kept on it.
    let extra = match w.unit(u).and_then(|x| x.bonus_experience) {
        Some(b) => b,
        None => {
            w.coverage.unsupported("UnitEntityData.BonusExperience not initialized");
            crate::xp::bonus_experience(w)
        }
    };
    crate::xp::apply_blueprint_experience(w, u, extra);
    if let Some(x) = w.unit_mut(u) {
        x.no_xp_on_death = true;
    }
}

/// `unit.Blueprint.Faction == PlayerFaction || unit.Master?.Blueprint.Faction == PlayerFaction`:
/// the *blueprints'* factions, not the units' current ones.
fn blueprint_faction_is_player(w: &World, u: EntityIdx) -> bool {
    let player = wr_content::Guid::parse(units::PLAYER_FACTION);
    let of = |u: EntityIdx| {
        w.unit(u)
            .and_then(|x| x.blueprint)
            .and_then(|g| w.content.get(g))
            .and_then(|b| b.view().guid("m_Faction"))
    };
    let master = w.unit(u).and_then(|x| x.master);
    (of(u).is_some() && of(u) == player)
        || master.is_some_and(|m| of(m).is_some() && of(m) == player)
}

/// `UnitLifeController.OnUnitDeath`.
fn on_unit_death(w: &mut World, u: EntityIdx, already_dead: bool) {
    bus::raise_global_call(w, "IUnitAboutToDeathHandler", "HandleUnitAboutToDeath", Some(u));
    let immortal = super::weapon::has_feature(w, u, IMMORTALITY);
    let was_finally_dead = w.unit(u).is_some_and(|x| x.finally_dead);
    let companion = blueprint_faction_is_player(w, u);
    let companion_state = w.unit(u).and_then(|x| x.companion_state);
    let flag2 = companion && companion_state != Some(3);
    let force_kill = w.unit(u).is_some_and(|x| x.force_kill);
    if already_dead {
        if force_kill {
            set_finally_dead(w, u, true);
        } else if flag2 {
            w.coverage.unsupported("UnitLifeController: dead companion");
        } else {
            set_finally_dead(w, u, !immortal);
        }
    } else if flag2 {
        w.coverage.unsupported("UnitLifeController: dead companion");
    } else {
        give_experience(w, u);
        if !immortal {
            set_finally_dead(w, u, true);
            // `foreach (ItemEntity item in unit.Inventory) item.UpdateSlotIndex(force: true)`.
            for item in w.unit(u).map(|x| x.inventory.clone()).unwrap_or_default() {
                crate::items::update_slot_index(w, item, true);
            }
        }
    }
    if !already_dead {
        if force_kill {
            set_finally_dead(w, u, true);
        }
        // `unit.View.HandleDeath()`: the view's.
        let now = w.clock.game_time.0;
        if let Some(x) = w.unit_mut(u) {
            x.death_time = now;
        }
        bus::raise_unit_fact(w, u, "IUnitDieHandler", "OnUnitDie", Some(u), None);
        bus::raise_global_call(w, "IUnitHandler", "HandleUnitDeath", Some(u));
        death_listeners(w, u);
    }
    if !w.unit(u).is_some_and(|x| x.finally_dead) {
        return;
    }
    if has_condition(w, u, cond::DEATH_DOOR) {
        crate::unit_state::remove_condition(w, u, cond::DEATH_DOOR, true);
    }
    if !was_finally_dead {
        bus::raise_global_call(
            w,
            "IUnitFinallyDeadHandler",
            "HandleUnitBecameFinallyDead",
            Some(u),
        );
    }
}

fn set_finally_dead(w: &mut World, u: EntityIdx, v: bool) {
    if let Some(x) = w.unit_mut(u) {
        x.finally_dead = v;
    }
}

/// `UnitHelplessController.Tick`: the party and pets, then the awake units; each gets or loses
/// `UnitCondition.Helpless` as `IsHelpless` says.
pub fn helpless_tick(w: &mut World) {
    let mut order = crate::narrative::party::character_lists(w).party_and_pets;
    order.extend(w.combat.awake.iter().copied());
    for u in order {
        update_helpless(w, u, true);
    }
    for u in w.combat.awake_unknown.clone() {
        update_helpless(w, u, false);
    }
}

fn update_helpless(w: &mut World, u: EntityIdx, apply: bool) {
    let helpless = units::is_helpless(w, u);
    let has = has_condition(w, u, cond::HELPLESS);
    if helpless == has {
        return;
    }
    if !apply {
        unknown_awake_would_change(w, "UnitHelplessController", u);
    } else if helpless {
        crate::unit_state::add_condition(w, u, cond::HELPLESS, None);
    } else {
        crate::unit_state::remove_condition(w, u, cond::HELPLESS, true);
    }
}

/// `UnitBuffsController.Tick` (`TickSleeping`): `BuffCollection.Tick` on every unit of the
/// entity pool that is in the game (suppressed entities, which the pool also drops, are not
/// modelled): `UpdateOnDeadOwner`, then the buff with the earliest event, if it is due.
pub fn buffs_tick(w: &mut World) {
    for u in all_units(w) {
        if w.entity(u).is_in_game {
            buff_collection_tick(w, u);
        }
    }
}

/// `UnitEntityData.IsCurrentUnit()`: the current turn's rider or its mount
/// (`CombatController.Rider`/`Mount`). The mount isn't tracked: a mounted or saddled unit
/// reports it. `None` when it can't be established: the rider of a turn that has ended may
/// already have been cleared ([`super::CombatWorld::current_unit_maybe_cleared`]).
fn is_current_unit(w: &mut World, u: EntityIdx) -> Option<bool> {
    let mounted = |w: &World, x: EntityIdx| {
        has_part(w, x, "UnitPartRider") || has_part(w, x, "UnitPartSaddled")
    };
    if mounted(w, u) || w.combat.current_unit.is_some_and(|c| mounted(w, c)) {
        w.coverage
            .unsupported("UnitEntityData.IsCurrentUnit: mounted units (CombatController.Mount)");
    }
    let is = w.combat.current_unit == Some(u);
    if is && w.combat.current_unit_maybe_cleared { None } else { Some(is) }
}

/// `BuffCollection.Tick`'s turn-based gate on the buff with the earliest event: the owner is the
/// current unit, or that buff's caster (`Context.MaybeCaster`) is. `None` when it depends on a
/// current unit that isn't established.
fn turn_gate(w: &mut World, u: EntityIdx, first: crate::world::FactIdx) -> Option<bool> {
    let owner = is_current_unit(w, u);
    if owner == Some(true) {
        return Some(true);
    }
    let caster = w
        .fact(first)
        .context
        .as_ref()
        .and_then(|c| c.caster.clone())
        .and_then(|id| w.by_id.get(&id).copied());
    let by_caster = match caster {
        Some(c) => is_current_unit(w, c),
        None => Some(false),
    };
    match (owner, by_caster) {
        (_, Some(true)) => Some(true),
        (Some(false), Some(false)) => Some(false),
        _ => None,
    }
}

/// The unit's buffs (`RawFacts`), in order.
fn buff_facts(w: &World, u: EntityIdx) -> Vec<crate::world::FactIdx> {
    w.entity(u)
        .facts
        .list
        .iter()
        .copied()
        .filter(|f| w.fact(*f).kind == crate::world::FactKind::Buff && w.fact(*f).buff.is_some())
        .collect()
}

/// `BuffCollection.UpdateNextEvent`: the first buff with the smallest `NextEventTime`.
fn next_event(w: &World, u: EntityIdx) -> Option<crate::world::FactIdx> {
    let mut best: Option<crate::world::FactIdx> = None;
    for f in buff_facts(w, u) {
        if best.is_none_or(|b| next_event_time(w, f) < next_event_time(w, b)) {
            best = Some(f);
        }
    }
    best
}

/// `Buff.NextEventTime`.
fn next_event_time(w: &World, f: crate::world::FactIdx) -> i64 {
    let b = w.fact(f).buff.as_ref();
    let end = b.and_then(|b| b.end_time).unwrap_or(i64::MAX);
    let tick = b.map_or(i64::MAX, |b| b.next_tick_time);
    if end < tick { end } else { tick }
}

/// `CombatController.StartTurn`'s `Buffs tick` sweep: with the new turn installed
/// (`CurrentTurn`) and the previous `TurnStartTime`, every awake unit's buffs update their next
/// event and tick. Awake units of unknown state that the tick would change are reported.
pub fn turn_start_sweep(w: &mut World) {
    for u in w.combat.awake.clone() {
        buff_collection_tick(w, u);
    }
    for u in w.combat.awake_unknown.clone() {
        if buff_tick_would_change(w, u) {
            unknown_awake_would_change(w, "CombatController.StartTurn buff sweep", u);
        }
    }
}

/// Whether `BuffCollection.Tick` would change the unit (or whether that can't be told):
/// `UpdateOnDeadOwner` removing the buffs of an owner that has died since its last tick (before
/// any time or turn check), or a due event that the turn-based gate lets through.
fn buff_tick_would_change(w: &mut World, u: EntityIdx) -> bool {
    let newly_dead =
        w.unit(u).is_some_and(|x| !x.buffs_owner_dead_seen && x.life_state == state::DEAD);
    let removable = buff_facts(w, u).into_iter().any(|f| {
        !w.fact(f)
            .bp
            .as_ref()
            .is_some_and(|b| crate::unit_facts::buff_flag(b, crate::unit_facts::BUFF_STAY_ON_DEATH))
    });
    if newly_dead && removable {
        return true;
    }
    let Some(first) = next_event(w, u) else { return false };
    let time = if w.combat.turn_based { w.combat.turn_start_time.0 } else { w.clock.game_time.0 };
    if next_event_time(w, first) > time {
        return false;
    }
    !w.combat.turn_based || turn_gate(w, u, first) != Some(false)
}

/// `CombatController.IsInTurnBasedCombat()`: the player is in combat in turn-based mode and the
/// game is not in a cutscene. Combat starts and ends with the recording's `tb` events.
pub fn update_turn_based(w: &mut World) {
    w.combat.turn_based = w.combat.tb_mode_on && w.combat.combat_active && !w.combat.cutscene;
}

/// `BuffCollection.Tick()`.
pub fn buff_collection_tick(w: &mut World, u: EntityIdx) {
    update_on_dead_owner(w, u);
    // `m_NextEvent`: the first buff with the smallest `NextEventTime`.
    let Some(first) = next_event(w, u) else { return };
    let mut time = w.clock.game_time.0;
    if w.combat.turn_based {
        match turn_gate(w, u, first) {
            Some(true) => {}
            Some(false) => return,
            None => {
                if next_event_time(w, first) <= w.combat.turn_start_time.0 {
                    w.coverage.unsupported(
                        "BuffCollection.Tick: whether an ended turn's unit is still current is not recorded",
                    );
                }
                return;
            }
        }
        time = w.combat.turn_start_time.0;
    }
    while let Some(b) = next_event(w, u) {
        if next_event_time(w, b) > time {
            break;
        }
        tick_buff(w, u, b, time);
    }
}

/// `BuffCollection.TickBuff(buff, currentTime)`.
fn tick_buff(w: &mut World, u: EntityIdx, f: crate::world::FactIdx, now: i64) {
    let due = w.fact(f).buff.as_ref().is_some_and(|b| b.next_tick_time <= now);
    if due {
        buff_tick_mechanics(w, f);
    }
    let end = w.fact(f).buff.as_ref().and_then(|b| b.end_time).unwrap_or(i64::MAX);
    if end <= now {
        crate::unit_facts::remove(w, u, f);
        return;
    }
    let tick_each_second = w.fact(f).bp.as_ref().is_some_and(|b| b.view().bool("TickEachSecond"));
    if !tick_each_second {
        // A buff that fell far behind catches up at most 30 ticks.
        let (tick, next) = w.fact(f).buff.as_ref().map_or((i64::MAX, i64::MAX), |b| {
            (
                b.tick_time,
                if b.end_time.unwrap_or(i64::MAX) < b.next_tick_time {
                    b.end_time.unwrap_or(i64::MAX)
                } else {
                    b.next_tick_time
                },
            )
        });
        #[allow(clippy::cast_precision_loss)]
        let (total, behind) = (tick as f64 / 1e7, (now.wrapping_sub(next)) as f64 / 1e7);
        if tick != i64::MAX && behind >= 0.0 && behind / total > 30.0 {
            #[allow(clippy::cast_possible_truncation)]
            let back = (total * 30.0 * 1e7) as i64;
            if let Some(b) = &mut w.fact_mut(f).buff {
                b.next_tick_time = now.wrapping_sub(back);
            }
        }
    }
}

/// `Buff.TickMechanics()`: the round counter, `ITickEachRound.OnNewRound` of the buff's
/// components, the next tick. Only components whose `OnNewRound` is empty are run.
fn buff_tick_mechanics(w: &mut World, f: crate::world::FactIdx) {
    if let Some(b) = &mut w.fact_mut(f).buff {
        b.round_number += 1;
    }
    if w.fact(f).is_active {
        for c in crate::facts::comps(w, f) {
            let Some(ty) = w.component(c).ty() else { continue };
            let name = wr_content::schema::ty(ty).short;
            if !crate::unit_facts::ticks_each_round(name) {
                continue;
            }
            match name {
                // `ImpatienceWatcherTickingResolve.OnNewRound` is empty.
                "ImpatienceWatcherTickingResolve" => {}
                "AddFactContextActions" => {
                    crate::components::mechanics::add_fact_context_actions_new_round(w, c);
                }
                _ => w.coverage.unsupported(format!("ITickEachRound.OnNewRound: {name}")),
            }
        }
    }
    if let Some(b) = &mut w.fact_mut(f).buff {
        b.next_tick_time = if b.tick_time < i64::MAX {
            b.next_tick_time.wrapping_add(b.tick_time)
        } else {
            i64::MAX
        };
    }
}

fn update_on_dead_owner(w: &mut World, u: EntityIdx) {
    let alive = !w.unit(u).is_some_and(|x| x.buffs_owner_dead_seen);
    let dead = w.unit(u).is_some_and(|x| x.life_state == state::DEAD);
    if alive && dead {
        let buffs: Vec<_> = w
            .entity(u)
            .facts
            .list
            .iter()
            .copied()
            .filter(|f| {
                w.fact(*f).kind == crate::world::FactKind::Buff
                    && !w.fact(*f).bp.as_ref().is_some_and(|b| {
                        crate::unit_facts::buff_flag(b, crate::unit_facts::BUFF_STAY_ON_DEATH)
                    })
            })
            .collect();
        for f in buffs {
            crate::unit_facts::remove(w, u, f);
        }
    }
    if let Some(x) = w.unit_mut(u) {
        x.buffs_owner_dead_seen = dead;
    }
}

/// `UnitEntityData.JoinCombat`: the combat state (`++Group.IsInCombat`) and
/// `IUnitCombatHandler.HandleUnitJoinCombat` (the view's hands and the command check are not
/// modelled).
pub fn join_combat(w: &mut World, u: EntityIdx) {
    if super::is_in_combat(w, u) {
        // `UnitCombatState.JoinCombat` returns for a unit in combat; the event is still raised.
    } else {
        if let Some(x) = w.unit_mut(u) {
            x.combat.in_combat = true;
        }
        super::groups::joined_combat(w, u);
    }
    bus::raise_global_call(w, "IUnitCombatHandler", "HandleUnitJoinCombat", Some(u));
    // `CombatController.HandleUnitJoinCombat`: `AddUnit` once initialized.
    if w.combat.turns.on {
        if w.combat.tb_units.is_some() {
            super::turns::add_unit(w, u);
        }
    } else if let Some(units) = &mut w.combat.tb_units
        && !units.contains(&u)
    {
        units.push(u);
    }
}

/// `CombatController.RemoveUnit` (from `HandleUnitLeaveCombat`, `HandleUnitDeath`): a member
/// leaves `m_Units`. With the turn port, `ChooseNextUnit` runs for the current or next unit and
/// the turn is cleared ([`super::turns::remove_unit`]); without it, the current turn's clear is
/// left for the recorded `turn_clear` to confirm.
pub fn remove_tb_unit(w: &mut World, u: EntityIdx) {
    if w.combat.turns.on {
        if w.combat.tb_units.is_some() {
            super::turns::remove_unit(w, u);
        }
        return;
    }
    let Some(units) = &mut w.combat.tb_units else { return };
    let Some(p) = units.iter().position(|x| *x == u) else { return };
    units.remove(p);
    if w.combat.current_unit == Some(u) {
        w.combat.current_unit = None;
        w.combat.current_unit_maybe_cleared = false;
        w.combat.cleared_by_remove = Some(u);
    }
}

/// `EntityDataBase.IsInGame = value` for a unit: the in-game handlers, then
/// `UnitEntityData.OnIsInGameChanged`: the buffs, activatable abilities and abilities are
/// deactivated (or activated) with the unit. Pets, the view and the group membership are not
/// modelled.
pub fn set_in_game(w: &mut World, u: EntityIdx, in_game: bool) {
    crate::entity::set_is_in_game(w, u, in_game);
}

/// A recorded firing of a `DropLootAndDestroyOnDeactivate` timer (WrathOracle's `loot_timer`):
/// the pending timer of `owner`'s buff with the recorded unique id is removed and its callback
/// runs (`buff.Remove()`, the buff it captured). Whether there was one.
pub fn fire_timer(w: &mut World, owner: Option<EntityIdx>, fact: Option<&str>) -> bool {
    let pick = (0..w.combat.timers.len()).find(|&i| {
        let f = w.fact(w.combat.timers[i].fact);
        owner.is_some() && f.owner == owner && fact.is_some_and(|id| f.unique_id == id)
    });
    let Some(p) = pick else { return false };
    let t = w.combat.timers.remove(p);
    if let Some(o) = w.fact(t.fact).owner {
        crate::unit_facts::remove(w, o, t.fact);
    }
    true
}

/// The real-time timers that are due (`Observable.Timer` callbacks): the buff is removed.
pub fn fire_timers(w: &mut World) {
    let now = w.clock.real_time.0;
    while let Some(p) = w.combat.timers.iter().position(|t| t.due <= now) {
        let t = w.combat.timers.remove(p);
        if let Some(owner) = w.fact(t.fact).owner {
            crate::unit_facts::remove(w, owner, t.fact);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use wr_content::Guid;
    use wr_content::node::Node;

    use super::*;
    use crate::content::{Blueprint, Source};
    use crate::csharp::TimeSpan;
    use crate::facts::tests::obj;
    use crate::stats::tests::world_with_unit;

    fn buff_bp(w: &World, g: u128) -> Rc<Blueprint> {
        let o = obj(
            "Kingmaker.UnitLogic.Buffs.Blueprints.BlueprintBuff",
            &[("Components", Node::Seq(Vec::new()))],
        );
        w.content.insert(Blueprint {
            guid: Guid(g),
            name: format!("B{g}"),
            obj: o,
            source: Source::Bbp,
        });
        w.content.get(Guid(g)).expect("inserted")
    }

    fn add_timed(w: &mut World, u: EntityIdx, g: u128, seconds: i64) -> crate::world::FactIdx {
        let bp = buff_bp(w, g);
        let ctx = crate::unit_facts::context(w, Some(u), Some(u), &bp);
        crate::unit_facts::add_buff(w, u, &bp, ctx, Some(TimeSpan(seconds * 10_000_000)))
            .expect("buff")
    }

    fn has(w: &World, u: EntityIdx, f: crate::world::FactIdx) -> bool {
        w.entity(u).facts.list.contains(&f)
    }

    /// An ordinary buff leaves when its end time passes (`BuffCollection.Tick` / `TickBuff`).
    #[test]
    fn a_buff_expires_when_game_time_passes_its_end() {
        let (mut w, u) = world_with_unit();
        w.clock.game_time = TimeSpan(1_000);
        let f = add_timed(&mut w, u, 900, 6);
        w.clock.game_time = TimeSpan(1_000 + 59_999_999);
        buffs_tick(&mut w);
        assert!(has(&w, u, f), "one tick short");
        w.clock.game_time = TimeSpan(1_000 + 60_000_000);
        buffs_tick(&mut w);
        assert!(!has(&w, u, f), "gone at its end time");
    }

    /// Turn-based: only the current unit's buffs (or those it cast) tick, and on the turn's
    /// start time, not on game time.
    #[test]
    fn turn_based_buff_ticks_follow_the_current_unit_and_turn_start_time() {
        let (mut w, u) = world_with_unit();
        w.combat.turn_based = true;
        w.clock.game_time = TimeSpan(0);
        w.combat.turn_start_time = TimeSpan(0);
        let f = add_timed(&mut w, u, 901, 6);
        // Game time is far past the end, the turn start isn't, and nobody's turn it is.
        w.clock.game_time = TimeSpan(1_000_000_000);
        buffs_tick(&mut w);
        assert!(has(&w, u, f), "not the owner's turn: nothing ticks");
        w.combat.current_unit = Some(u);
        buffs_tick(&mut w);
        assert!(has(&w, u, f), "the turn started before the end");
        w.combat.turn_start_time = TimeSpan(60_000_000);
        buffs_tick(&mut w);
        assert!(!has(&w, u, f), "the turn start reached the end");
    }

    /// A buff whose owner died is removed at the next buff tick unless it stays on death.
    #[test]
    fn a_dead_owners_buffs_go_at_the_next_buff_tick() {
        let (mut w, u) = world_with_unit();
        let f = add_timed(&mut w, u, 902, 600);
        w.unit_mut(u).expect("unit").life_state = state::DEAD;
        buffs_tick(&mut w);
        assert!(!has(&w, u, f));
    }

    /// The companion test uses the blueprints' factions, not the units' current ones.
    #[test]
    fn death_checks_the_blueprint_faction_not_the_current_one() {
        let (mut w, u) = world_with_unit();
        let player = Guid::parse(units::PLAYER_FACTION).expect("guid");
        let mk = |w: &World, g: u128, faction: Option<Guid>| {
            let mut set = Vec::new();
            if let Some(f) = faction {
                set.push(("m_Faction", Node::BpRef(f)));
            }
            let o = obj("Kingmaker.Blueprints.BlueprintUnit", &set);
            w.content.insert(Blueprint {
                guid: Guid(g),
                name: "U".into(),
                obj: o,
                source: Source::Bbp,
            });
            Guid(g)
        };
        let party_bp = mk(&w, 910, Some(player));
        let wild_bp = mk(&w, 911, Some(Guid(77)));
        // A player-blueprint unit that was made hostile (current faction differs): a companion.
        w.unit_mut(u).expect("unit").blueprint = Some(party_bp);
        w.unit_mut(u).expect("unit").faction = Some(Guid(77));
        assert!(blueprint_faction_is_player(&w, u));
        // A wild-blueprint unit now in the player's faction (current): not.
        w.unit_mut(u).expect("unit").blueprint = Some(wild_bp);
        w.unit_mut(u).expect("unit").faction = Some(player);
        assert!(!blueprint_faction_is_player(&w, u));
    }

    /// `ShouldBeSleeping`'s gates in order: out of the game or a fake blueprint sleeps even in
    /// combat; a finally dead unit is unknown (its `AwakeTimer` comes first); combat wakes; an
    /// extra unit depends on the camera; a party member is awake; anyone else is unknown.
    #[test]
    fn awake_state_follows_the_sleeping_controllers_order() {
        let (mut w, u) = world_with_unit();
        let party = [u];
        assert_eq!(awake_state(&w, u, &[]), Awake::Unknown);
        assert_eq!(awake_state(&w, u, &party), Awake::Yes);
        w.unit_mut(u).expect("unit").is_extra = true;
        assert_eq!(awake_state(&w, u, &party), Awake::Unknown, "an extra unit is camera-bound");
        w.unit_mut(u).expect("unit").combat.in_combat = true;
        assert_eq!(awake_state(&w, u, &[]), Awake::Yes, "combat wakes before the extra check");
        w.unit_mut(u).expect("unit").finally_dead = true;
        assert_eq!(
            awake_state(&w, u, &party),
            Awake::Unknown,
            "finally dead sleeps before combat, unless its AwakeTimer (not tracked) runs"
        );
        w.unit_mut(u).expect("unit").finally_dead = false;
        w.entity_mut(u).is_in_game = false;
        assert_eq!(awake_state(&w, u, &party), Awake::No);
        w.entity_mut(u).is_in_game = true;
        let fake = obj("Kingmaker.Blueprints.BlueprintUnit", &[("IsFake", Node::Bool(true))]);
        w.content.insert(Blueprint {
            guid: Guid(930),
            name: "Fake".into(),
            obj: fake,
            source: Source::Bbp,
        });
        w.unit_mut(u).expect("unit").blueprint = Some(Guid(930));
        assert_eq!(awake_state(&w, u, &party), Awake::No, "a fake blueprint sleeps even in combat");
    }

    /// A unit already dead but not finally dead goes through death processing again; a
    /// forced kill raises the damage; a conscious unit with nothing pending is inert.
    #[test]
    fn force_tick_is_not_inert_for_an_unchanged_life_state() {
        let (mut w, u) = world_with_unit();
        assert!(!life_tick_would_change(&mut w, u), "conscious, nothing pending");
        w.unit_mut(u).expect("unit").damage = 1_000;
        w.unit_mut(u).expect("unit").life_state = state::DEAD;
        assert!(life_tick_would_change(&mut w, u), "dead but not finally dead");
        w.unit_mut(u).expect("unit").finally_dead = true;
        assert!(!life_tick_would_change(&mut w, u), "finally dead: SetLifeState returns early");
        let (mut w, u) = world_with_unit();
        w.unit_mut(u).expect("unit").force_kill = true;
        assert!(life_tick_would_change(&mut w, u), "a forced kill");
    }

    /// A mounted unit's `IsCurrentUnit` also accepts the mount: reported, not guessed.
    #[test]
    fn mounted_units_are_reported_by_is_current_unit() {
        let (mut w, u) = world_with_unit();
        assert_eq!(is_current_unit(&mut w, u), Some(false));
        assert!(w.coverage.unsupported.is_empty());
        w.entity_mut(u).parts.push(crate::world::Part { clr: "UnitPartRider" });
        w.combat.current_unit = Some(u);
        assert_eq!(is_current_unit(&mut w, u), Some(true));
        assert!(!w.coverage.unsupported.is_empty());
    }

    /// After its turn ends, the rider may already have been cleared: whether it is current is
    /// unknown, and a due buff of it is reported instead of ticked; a buff that isn't due needs
    /// no answer. Another unit is not current either way.
    #[test]
    fn an_ended_turns_rider_is_neither_current_nor_not() {
        let (mut w, u) = world_with_unit();
        w.combat.turn_based = true;
        w.combat.turn_start_time = TimeSpan(0);
        let f = add_timed(&mut w, u, 905, 6);
        w.combat.current_unit = Some(u);
        w.combat.current_unit_maybe_cleared = true;
        assert_eq!(is_current_unit(&mut w, u), None);
        buffs_tick(&mut w);
        assert!(w.coverage.unsupported.is_empty(), "not due: nothing to decide");
        w.combat.turn_start_time = TimeSpan(60_000_000);
        buffs_tick(&mut w);
        assert!(has(&w, u, f), "not ticked on a guess");
        assert!(w.coverage.unsupported.keys().any(|k| k.starts_with("BuffCollection.Tick")));
        w.combat.current_unit_maybe_cleared = false;
        buffs_tick(&mut w);
        assert!(!has(&w, u, f), "established: ticked");
    }

    /// The turn-start sweep's check for a unit whose awake state is unknown includes
    /// `UpdateOnDeadOwner`, which runs before any time or turn check: a dead (not finally dead)
    /// owner's buffs would go even when none is due and it isn't anyone's turn.
    #[test]
    fn the_sweep_reports_a_dead_unknown_units_buff_cleanup() {
        let (mut w, u) = world_with_unit();
        w.combat.turn_based = true;
        let _ = add_timed(&mut w, u, 906, 600);
        w.combat.awake_unknown = vec![u];
        turn_start_sweep(&mut w);
        assert!(w.coverage.unsupported.is_empty(), "alive, nothing due, not current: inert");
        w.unit_mut(u).expect("unit").life_state = state::DEAD;
        turn_start_sweep(&mut w);
        assert!(
            w.coverage.unsupported.keys().any(|k| k.starts_with("CombatController.StartTurn")),
            "UpdateOnDeadOwner would remove the buff"
        );
    }

    /// An item entity of the unit's own inventory (collection 1) with the given saved state.
    fn add_item(
        w: &mut World,
        u: EntityIdx,
        id: &str,
        not_lootable: bool,
        index: Option<i32>,
    ) -> EntityIdx {
        let item = crate::item::Item {
            count: 1,
            collection: Some(1),
            not_lootable,
            inventory_slot_index: index,
            ..crate::item::Item::default()
        };
        let e = w.add_entity(crate::world::Entity {
            unique_id: id.into(),
            clr: "ItemEntityUsable",
            kind: crate::world::EntityKind::Item(Box::new(item)),
            facts: crate::world::FactsManager::default(),
            parts: Vec::new(),
            is_turned_on: true,
            post_load_executed: true,
            is_in_game: true,
            subscribed: false,
            disposing: false,
            is_disposed: false,
        });
        let x = w.unit_mut(u).expect("unit");
        x.inventory.push(e);
        x.inventory_collection = Some(1);
        x.has_own_inventory = true;
        e
    }

    fn index_of(w: &World, item: EntityIdx) -> Option<i32> {
        w.item(item).expect("item").inventory_slot_index
    }

    /// `UnitLifeController`: when a non-immortal unit dies, `UpdateSlotIndex(force: true)` runs
    /// on every item of its inventory: a non-lootable item (a natural weapon) gets -1, a lootable
    /// one the lowest index the collection leaves free; nothing is reported.
    #[test]
    fn a_dying_units_items_get_their_slot_indices_recomputed() {
        let (mut w, u) = world_with_unit();
        let loot = add_item(&mut w, u, "i1", false, Some(3));
        let natural = add_item(&mut w, u, "i2", true, Some(5));
        let other = add_item(&mut w, u, "i3", false, Some(0));
        w.unit_mut(u).expect("unit").damage = 1_000;
        force_tick_on_unit(&mut w, u);
        assert!(w.unit(u).expect("unit").finally_dead);
        assert_eq!(index_of(&w, natural), Some(-1), "not lootable: -1");
        assert_eq!(index_of(&w, loot), Some(1), "0 is held by another item, 1 is free");
        assert_eq!(index_of(&w, other), Some(0), "its own turn: reset, then the lowest free (0)");
        assert!(
            w.coverage.unsupported.keys().all(|k| !k.contains("UpdateSlotIndex")),
            "{:?}",
            w.coverage.unsupported
        );
    }

    /// An item that entered the collection at run time has no known index: reported, and the
    /// others' indices aren't computed from it.
    #[test]
    fn an_unknown_slot_index_is_reported_not_guessed() {
        let (mut w, u) = world_with_unit();
        let a = add_item(&mut w, u, "i1", false, None);
        let _ = add_item(&mut w, u, "i2", false, Some(2));
        assert_eq!(crate::items::update_slot_index(&mut w, a, false), None);
        assert!(w.coverage.unsupported.keys().any(|k| k.starts_with("ItemEntity.UpdateSlotIndex")));
        assert_eq!(index_of(&w, a), None);
        // Forced, the item's own index is -1; the other item's must be known: it is.
        assert_eq!(crate::items::update_slot_index(&mut w, a, true), Some(0));
    }
}
