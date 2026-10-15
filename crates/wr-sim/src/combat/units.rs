//! Unit queries combat rules read (decompiled, not copied): relations
//! (`Kingmaker.UnitLogic.Groups/UnitGroup.cs` `IsEnemy`, `UnitEntityData.IsPlayersEnemy`,
//! `IsEnemy`), life and helplessness (`Kingmaker.UnitLogic/UnitState.cs` `IsHelpless`,
//! `IsConscious`), difficulty settings (`SettingsRoot.Difficulty`), and the controller
//! state an attack reads that the sim doesn't simulate yet (engagement, memory,
//! concealment), supplied per attack as driven inputs.

use std::rc::Rc;

use wr_content::Guid;

use crate::content::Blueprint;
use crate::unit_state::{cond, has_condition};
use crate::world::{EntityIdx, World};

/// `UnitCondition` values read by attacks.
pub mod condition {
    pub const BLINDNESS: i32 = 1;
    pub const PARALYZED: i32 = 4;
    pub const SLEEPING: i32 = 16;
    pub const LOSE_DEXTERITY_TO_AC: i32 = 21;
    pub const CONFUSION: i32 = 25;
}

/// The player party's group id (`UnitGroup.IsPlayerParty`).
pub const PLAYER_PARTY_GROUP: &str = "<directly-controllable-unit>";

/// `BlueprintRoot.PlayerFaction`.
pub const PLAYER_FACTION: &str = "72f240260881111468db610b6c37c099";

fn faction(w: &World, u: EntityIdx) -> Option<Rc<Blueprint>> {
    w.content.get(w.unit(u)?.faction?)
}

/// `unit.Faction.Peaceful` (`None`: no faction the sim knows).
#[must_use]
pub fn faction_peaceful(w: &World, u: EntityIdx) -> Option<bool> {
    faction(w, u).map(|f| f.view().bool("Peaceful"))
}

/// `UnitEntityData.GroupId`: the saved `m_GroupId`, else the party group for a player-faction
/// unit and the unit's own id otherwise (the getter stores it; here it is only computed).
pub(crate) fn group_id(w: &World, u: EntityIdx) -> Option<String> {
    group(w, u)
}

fn group(w: &World, u: EntityIdx) -> Option<String> {
    let unit = w.unit(u)?;
    match unit.group_id.as_deref() {
        Some(g) if !g.is_empty() => Some(g.to_owned()),
        _ if unit.is_player_faction => Some(PLAYER_PARTY_GROUP.to_owned()),
        _ => Some(w.entity(u).unique_id.clone()),
    }
}

/// The units of a group, in entity order.
fn group_units(w: &World, g: &str) -> Vec<EntityIdx> {
    (0..w.entities.len())
        .filter_map(|i| u32::try_from(i).ok().map(EntityIdx))
        .filter(|e| group(w, *e).as_deref() == Some(g))
        .collect()
}

/// `BlueprintUnit.AttackFactions`: the faction's attack factions plus the unit blueprint's
/// `FactionOverrides.AttackFactionsToAdd`, minus its `AttackFactionsToRemove`.
#[must_use]
pub fn blueprint_attack_factions(w: &World, unit_bp: &Blueprint) -> Vec<Guid> {
    let mut out: Vec<Guid> = Vec::new();
    if let Some(f) = unit_bp.view().guid("m_Faction").and_then(|g| w.content.get(g)) {
        for a in f.view().guids("m_AttackFactions") {
            if !out.contains(&a) {
                out.push(a);
            }
        }
    }
    if let Some(o) = unit_bp.view().obj("FactionOverrides") {
        for a in o.guids("m_AttackFactionsToAdd") {
            if !out.contains(&a) {
                out.push(a);
            }
        }
        let remove = o.guids("m_AttackFactionsToRemove");
        out.retain(|a| !remove.contains(a));
    }
    out
}

/// `UnitGroup.m_Factions` and the attack-faction cache (`UpdateAttackFactionsCache`: the
/// union of the units' own attack factions, `Descriptor.AttackFactions`).
fn group_factions(w: &World, g: &str) -> (Vec<Guid>, Vec<Guid>, bool) {
    let mut factions = Vec::new();
    let mut attack = Vec::new();
    let mut everyone = false;
    for u in group_units(w, g) {
        let Some(f) = faction(w, u) else { continue };
        if !factions.contains(&f.guid) {
            factions.push(f.guid);
        }
        everyone |= f.view().bool("EnemyForEveryone");
        for &a in &w.unit(u).map(|x| x.attack_factions.clone()).unwrap_or_default() {
            if !attack.contains(&a) {
                attack.push(a);
            }
        }
    }
    (factions, attack, everyone)
}

/// `UnitGroup.IsEnemy(g1, g2)` (static, one direction).
fn is_enemy_dir(w: &World, g1: &str, g2: &str) -> bool {
    if g1 == g2 {
        return false;
    }
    let (f1, _, e1) = group_factions(w, g1);
    let (_, a2, e2) = group_factions(w, g2);
    if e1 || e2 {
        return true;
    }
    if g2 == PLAYER_PARTY_GROUP {
        return false;
    }
    for f in a2 {
        if w.content.get(f).is_some_and(|b| b.view().bool("AlwaysEnemy")) {
            return true;
        }
        if f1.contains(&f) {
            return true;
        }
    }
    false
}

/// `UnitEntityData.IsEnemy(unit)`: `Group.IsEnemy(unit.Group)`, either direction. Units
/// without a group report unsupported.
pub fn is_enemy(w: &mut World, a: EntityIdx, b: EntityIdx) -> bool {
    let (Some(ga), Some(gb)) = (group(w, a), group(w, b)) else {
        w.coverage.unsupported("UnitEntityData.IsEnemy: unit without a group");
        return false;
    };
    is_enemy_dir(w, &ga, &gb) || is_enemy_dir(w, &gb, &ga)
}

/// `UnitEntityData.IsPlayersEnemy`: the faction is everyone's enemy, or the group attacks
/// the player faction.
pub fn is_players_enemy(w: &World, u: EntityIdx) -> bool {
    if faction(w, u).is_some_and(|f| f.view().bool("EnemyForEveryone")) {
        return true;
    }
    let Some(g) = group(w, u) else { return false };
    let player = Guid::parse(PLAYER_FACTION);
    group_factions(w, &g).1.iter().any(|f| Some(*f) == player)
}

/// `Faction.Neutral`.
#[must_use]
pub fn faction_neutral(w: &World, u: EntityIdx) -> bool {
    faction(w, u).is_some_and(|f| f.view().bool("Neutral"))
}

#[must_use]
pub fn is_player_faction(w: &World, u: EntityIdx) -> bool {
    w.unit(u).is_some_and(|x| x.is_player_faction)
}

/// `UnitState.IsConscious` (`LifeState == Conscious`).
#[must_use]
pub fn is_conscious(w: &World, u: EntityIdx) -> bool {
    w.unit(u).is_some_and(|x| x.life_state == 0)
}

/// `UnitState.IsDead` (`LifeState == Dead`).
#[must_use]
pub fn is_dead(w: &World, u: EntityIdx) -> bool {
    w.unit(u).is_some_and(|x| x.life_state == 2)
}

/// `UnitState.IsAnimating`: `DisableAnimation` clear (a cutscene command's counter, never
/// saved, so clear here) and, for a paralyzed or petrified unit, a cutscene controlling
/// it. The sim plays no cutscenes, so none controls a unit; that branch is reported.
pub fn is_animating(w: &mut World, u: EntityIdx) -> bool {
    if has_condition(w, u, condition::PARALYZED) || has_condition(w, u, cond::PETRIFIED) {
        w.coverage.unsupported(
            "UnitState.IsAnimating: paralyzed or petrified unit (cutscene control not modelled)",
        );
        return false;
    }
    true
}

/// `UnitState.IsHelpless`: not (conscious, animating and awake), or unconscious.
pub fn is_helpless(w: &mut World, u: EntityIdx) -> bool {
    if is_conscious(w, u) && is_animating(w, u) && !has_condition(w, u, condition::SLEEPING) {
        return has_condition(w, u, cond::UNCONSCIOUS);
    }
    true
}

fn raw_setting<'a>(w: &'a World, key: &str) -> Option<&'a serde_json::Value> {
    w.settings.difficulty.settings.raw.get(&format!("settings.difficulty.{key}"))
}

/// `SettingsRoot.Difficulty.DamageToParty` (the save's setting; reported when absent).
pub fn damage_to_party(w: &mut World) -> f32 {
    match raw_setting(w, "damage-to-party").and_then(serde_json::Value::as_f64) {
        #[allow(clippy::cast_possible_truncation)]
        Some(v) => v as f32,
        None => {
            w.coverage.unsupported("SettingsRoot.Difficulty.DamageToParty missing");
            1.0
        }
    }
}

/// `CriticalHitPower`.
pub const CRIT_OFF: i32 = 0;
pub const CRIT_WEAK: i32 = 1;
pub const CRIT_NORMAL: i32 = 2;

/// `SettingsRoot.Difficulty.EnemyCriticalHits`.
pub fn enemy_critical_hits(w: &mut World) -> i32 {
    match raw_setting(w, "enemy-critical-hits").and_then(serde_json::Value::as_str) {
        Some("Off") => CRIT_OFF,
        Some("Weak") => CRIT_WEAK,
        Some("Normal") => CRIT_NORMAL,
        _ => {
            w.coverage.unsupported("SettingsRoot.Difficulty.EnemyCriticalHits missing");
            CRIT_NORMAL
        }
    }
}

/// `StatsAdjustmentsType`: `ExtraDecline` 0, `StrongDecline` 1, `Decline` 2, …
pub fn stats_adjustments(w: &mut World) -> i32 {
    match w.settings.difficulty.settings.stats_adjustments {
        Some(v) => v,
        None => {
            w.coverage.unsupported("SettingsRoot.Difficulty.StatsAdjustments missing");
            4
        }
    }
}

/// What an attack roll reads from controllers the sim doesn't run yet, supplied by the
/// replay from the recording's `env` record for that roll.
#[derive(Debug, Clone, PartialEq)]
pub struct AttackEnv {
    /// `Target.CombatState.IsFlanked`.
    pub target_flanked: bool,
    /// `Target.Memory.Contains(Initiator)`.
    pub target_remembers_initiator: bool,
    /// `Target.CombatState.CanActInCombat`.
    pub target_can_act_in_combat: bool,
    /// `UnitPartConcealment.Calculate(Initiator, Target, attack: true)`.
    pub concealment_target: i32,
    /// `UnitPartConcealment.Calculate(Target, Initiator)`.
    pub concealment_initiator: i32,
    /// `Target.View.IsGetUp` (the unit view's stand-up state), when the environment
    /// carries it (`None`: not recorded).
    pub target_is_get_up: Option<bool>,
    /// `Target.CombatState.EngagedBy`: the units that engage the target (`None`: not
    /// recorded).
    pub target_engaged_by: Option<Vec<EntityIdx>>,
    /// The initiator's and the target's positions (`UnitEntityData.Position`), when recorded.
    pub positions: Option<([f32; 3], [f32; 3])>,
}

impl AttackEnv {
    /// The environment of an ordinary attack: the target can act, remembers the attacker,
    /// isn't flanked, no concealment. Only the fail-closed fallback of [`take_env`] uses it
    /// (there is deliberately no `Default`: all-false would make the target flat-footed).
    #[must_use]
    pub fn neutral() -> Self {
        Self {
            target_flanked: false,
            target_remembers_initiator: true,
            target_can_act_in_combat: true,
            concealment_target: 0,
            concealment_initiator: 0,
            target_is_get_up: Some(false),
            target_engaged_by: None,
            positions: None,
        }
    }
}

/// The attack environment for the next roll of `a` against `t`: the driven input when the
/// replay supplied one (consumed), else reported unsupported (engagement, memory and
/// concealment aren't simulated) and the neutral environment.
pub fn take_env(w: &mut World, a: EntityIdx, t: EntityIdx) -> AttackEnv {
    let key = (a, t);
    if let Some(p) = w.combat.env.iter().position(|(k, _)| *k == key) {
        let (_, e) = w.combat.env.remove(p);
        w.combat.driven_inputs += 1;
        return e;
    }
    w.coverage.unsupported("attack environment (engagement, memory, concealment) not simulated");
    AttackEnv::neutral()
}

/// `GeometryUtils.MechanicsDistance(a, b)` (Owlcat.Runtime.Core.Utils): the horizontal
/// distance, unless the heights differ by more than 1.5, then at least half the difference.
#[must_use]
pub fn mechanics_distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dy = (a[1] - b[1]).abs();
    let (dx, dz) = (a[0] - b[0], a[2] - b[2]);
    let magnitude = (dx * dx + dz * dz).sqrt();
    if dy <= 1.5 { magnitude } else { magnitude.max(dy / 2.0) }
}

/// `Feet.Meters` of a serialized `Feet` field.
#[must_use]
pub fn feet_meters(v: crate::content::View<'_>, field: &str) -> f32 {
    v.obj(field).map_or(0.0, |f| f.f32("m_Value")) * 0.3048_f32
}
