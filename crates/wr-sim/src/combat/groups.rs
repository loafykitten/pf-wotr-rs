//! Unit groups and leaving combat (decompiled, not copied): `UnitGroup`
//! (`Kingmaker.UnitLogic.Groups/UnitGroup.cs`), `UnitGroupsController`
//! (`Kingmaker.Controllers.Units`), `UnitEntityData.UpdateGroup`/`Group`, and
//! `UnitCombatLeaveController` (`Kingmaker.Controllers.Combat`).
//!
//! A unit's group is cached on the unit (`m_Group`): a real group (`GetGroup(GroupId)`) once the
//! unit is in the game and held by a scene state, else a fake one of its own. Leaving the game
//! drops the cache (`OnIsInGameChanged`); `UnitGroupsController.Tick` (and any `Group` access)
//! puts the unit back. A group counts its units in combat (`IsInCombat`, a `CountingGuard`) and
//! times how long it has had reason to leave combat (`LeaveCombatTimer`).
//!
//! Group memory (`UnitGroupMemory`) follows vision and line of sight, which the sim doesn't run,
//! and units' commands aren't simulated yet: both are recorded inputs (WrathOracle's `leave_tick`
//! and `cmds` observations, at the leave controller's prefix). A decision that would need them
//! when they aren't recorded is reported.

use std::collections::HashMap;

use wr_content::Guid;

use crate::world::{EntityIdx, World};

/// An index into [`Groups::arena`] (a `UnitGroup` object).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GroupIdx(pub usize);

/// `UnitGroupMemory` as `UnitCombatLeaveController` reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Memory {
    /// `m_Units`' keys (unique ids) in their enumeration order (`Contains`).
    pub dict: Vec<String>,
    /// `m_UnitsList` while `m_UnitsListValid` (`UnitsList` returns it as cached, even after
    /// `Cleanup` removed a key); `None`: invalid, rebuilt from [`Self::dict`] on access.
    pub list: Option<Vec<String>>,
}

/// `UnitGroup`.
#[derive(Debug, Clone, Default)]
pub struct Group {
    /// `Id`; `None` for a fake group (`new UnitGroup(null)`).
    pub id: Option<String>,
    /// `m_Units`, in order.
    pub units: Vec<EntityIdx>,
    /// `IsInCombat`'s guard count.
    pub in_combat: i32,
    /// `LeaveCombatTimer` (seconds).
    pub leave_timer: f32,
    pub disposed: bool,
    /// The memory, when the recording establishes it.
    pub memory: Option<Memory>,
}

impl Group {
    #[must_use]
    pub fn is_fake(&self) -> bool {
        self.id.is_none()
    }

    #[must_use]
    pub fn is_player_party(&self) -> bool {
        self.id.as_deref() == Some(super::units::PLAYER_PARTY_GROUP)
    }
}

/// A unit's command as the leave controller reads it (WrathOracle's `cmds`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// The command's class (`UnitAttack`, `UnitUseAbility`, `UnitMoveTo`…).
    pub class: String,
    /// `Target.UnitRef.Id`.
    pub target: Option<String>,
    pub started: bool,
    pub finished: bool,
}

/// `UnitGroupsController` and the units' cached groups.
#[derive(Debug, Default)]
pub struct Groups {
    /// The model runs (a driven replay of a recording with WrathOracle's group records).
    pub on: bool,
    /// Every `UnitGroup` object made, fake and disposed ones too.
    pub arena: Vec<Group>,
    /// `UnitGroupsController.Groups`, in order.
    pub list: Vec<GroupIdx>,
    /// `UnitEntityData.m_Group`, per unit.
    pub cached: HashMap<EntityIdx, GroupIdx>,
    /// Units' commands (`Commands.Raw`, then `Queue`), as last recorded; `None` before the
    /// first record.
    pub commands: Option<HashMap<String, Vec<Command>>>,
}

impl Groups {
    #[must_use]
    pub fn get(&self, g: GroupIdx) -> &Group {
        &self.arena[g.0]
    }

    pub fn get_mut(&mut self, g: GroupIdx) -> &mut Group {
        &mut self.arena[g.0]
    }

    /// The live group with this id (`Groups.FirstOrDefault`).
    #[must_use]
    pub fn find(&self, id: &str) -> Option<GroupIdx> {
        self.list.iter().copied().find(|g| self.arena[g.0].id.as_deref() == Some(id))
    }

    fn make(&mut self, id: Option<String>) -> GroupIdx {
        // A new group's memory is empty (`new UnitGroupMemory`).
        self.arena.push(Group { id, memory: Some(Memory::default()), ..Group::default() });
        GroupIdx(self.arena.len() - 1)
    }
}

/// `CountingGuard`'s `--`: a count at zero doesn't go negative (the game logs an error).
fn guard_dec(w: &mut World, g: GroupIdx) {
    let grp = w.combat.groups.get_mut(g);
    if grp.in_combat > 0 {
        grp.in_combat -= 1;
    } else {
        w.log("error", "CountingGuard went negative: UnitGroup.IsInCombat");
    }
}

/// `UnitEntityData.HoldingState != null`: the unit belongs to a scene state.
fn held(w: &World, u: EntityIdx) -> bool {
    w.player.cross_scene_units.contains(&u) || w.player.area_units.contains(&u)
}

/// `UnitGroupsController.GetGroup(id)`: the live group with that id, else a new one, added to
/// the list.
pub fn get_group(w: &mut World, id: &str) -> GroupIdx {
    if let Some(g) = w.combat.groups.find(id) {
        return g;
    }
    let g = w.combat.groups.make(Some(id.to_owned()));
    w.combat.groups.list.push(g);
    g
}

/// `UnitGroup.Add(unit)`.
fn add(w: &mut World, g: GroupIdx, u: EntityIdx) {
    let id = w.combat.groups.get(g).id.clone();
    if let Some(id) = &id
        && super::units::group_id(w, u).as_deref() != Some(id.as_str())
    {
        w.log("error", format!("Adding unit to wrong group: {}", w.entity(u).unique_id));
        return;
    }
    let grp = w.combat.groups.get(g);
    if grp.units.contains(&u) || grp.disposed {
        w.log("error", "UnitGroup.Add: unit already in the group, or the group is disposed");
        return;
    }
    let in_combat = super::is_in_combat(w, u);
    let grp = w.combat.groups.get_mut(g);
    grp.units.push(u);
    if in_combat {
        grp.in_combat += 1;
    }
    // `m_Factions.Add`, `UpdateAttackFactionsCache()` (it clears the memory's enemy list, which
    // the leave controller doesn't read): the factions are computed from the members when read.
}

/// `UnitGroup.Remove(unit)`: the count drops for a unit in combat whether or not it was a member.
fn remove(w: &mut World, g: GroupIdx, u: EntityIdx) {
    if w.combat.groups.get(g).disposed {
        return;
    }
    let grp = w.combat.groups.get_mut(g);
    if let Some(p) = grp.units.iter().position(|x| *x == u) {
        grp.units.remove(p);
    }
    if super::is_in_combat(w, u) {
        guard_dec(w, g);
    }
}

/// `UnitEntityData.UpdateGroup()`.
pub fn update_group(w: &mut World, u: EntityIdx) {
    let cur = w.combat.groups.cached.get(&u).copied();
    if w.entity(u).is_disposed {
        if cur.is_none_or(|g| !w.combat.groups.get(g).is_fake()) {
            if let Some(g) = cur {
                remove(w, g, u);
            }
            w.combat.groups.cached.remove(&u);
        }
        if !w.combat.groups.cached.contains_key(&u) {
            let g = w.combat.groups.make(None);
            add(w, g, u);
            w.combat.groups.cached.insert(u, g);
        }
        return;
    }
    let stale = cur.is_none_or(|g| {
        let grp = w.combat.groups.get(g);
        grp.is_fake() || grp.disposed
    });
    if !stale {
        return;
    }
    if held(w, u) && w.entity(u).is_in_game {
        let Some(id) = super::units::group_id(w, u) else { return };
        let g = get_group(w, &id);
        w.combat.groups.cached.insert(u, g);
        add(w, g, u);
    } else if cur.is_none() {
        let g = w.combat.groups.make(None);
        w.combat.groups.cached.insert(u, g);
        add(w, g, u);
    }
}

/// `UnitEntityData.Group`: [`update_group`], then the cached group.
pub fn group_of(w: &mut World, u: EntityIdx) -> Option<GroupIdx> {
    if !w.combat.groups.on {
        return None;
    }
    update_group(w, u);
    w.combat.groups.cached.get(&u).copied()
}

/// `m_Group?.Remove(this); m_Group = null` (`OnIsInGameChanged`, `OnDispose`, the `GroupId`
/// setter).
pub fn drop_cached(w: &mut World, u: EntityIdx) {
    if !w.combat.groups.on {
        return;
    }
    if let Some(g) = w.combat.groups.cached.remove(&u) {
        remove(w, g, u);
    }
}

/// `UnitGroupsController.Tick()`: every unit the state's pool enumerates updates its group, then
/// empty groups are disposed and dropped. (The deferred attack-faction cache updates aren't kept:
/// factions are computed from the members when read, and the recording's caches are checked.)
pub fn tick(w: &mut World) {
    if !w.combat.groups.on {
        return;
    }
    for u in crate::scene::units_pool(w) {
        if crate::scene::enumerated_by_pool(w, u) {
            update_group(w, u);
        }
    }
    let groups = &mut w.combat.groups;
    let empty: Vec<GroupIdx> =
        groups.list.iter().copied().filter(|g| groups.arena[g.0].units.is_empty()).collect();
    for g in empty {
        // `UnitGroup.Dispose()`: units and memory cleared.
        let grp = groups.get_mut(g);
        grp.units.clear();
        grp.memory = Some(Memory::default());
        grp.disposed = true;
        groups.list.retain(|x| *x != g);
    }
}

/// `UnitGroupsController.Start()` (the controllers starting after a load): the model is on and
/// ticks once.
pub fn start(w: &mut World) {
    w.combat.groups = Groups { on: true, ..Groups::default() };
    tick(w);
}

/// `++Unit.Group.IsInCombat` (`UnitCombatState.JoinCombat`).
pub fn joined_combat(w: &mut World, u: EntityIdx) {
    if let Some(g) = group_of(w, u) {
        w.combat.groups.get_mut(g).in_combat += 1;
    }
}

/// `--Unit.Group.IsInCombat` (`UnitCombatState.LeaveCombat`).
pub fn left_combat(w: &mut World, u: EntityIdx) {
    if let Some(g) = group_of(w, u) {
        guard_dec(w, g);
    }
}

/// `UnitGroup.m_Factions`, the attack-faction cache and `m_IsEnemyForEveryone`, from the members.
#[must_use]
pub fn factions(w: &World, g: GroupIdx) -> (Vec<(Guid, i32)>, Vec<Guid>, bool) {
    let mut f: Vec<(Guid, i32)> = Vec::new();
    let mut attack: Vec<Guid> = Vec::new();
    let mut everyone = false;
    for &u in &w.combat.groups.get(g).units {
        let Some(x) = w.unit(u) else { continue };
        if let Some(fg) = x.faction {
            match f.iter_mut().find(|(k, _)| *k == fg) {
                Some((_, n)) => *n += 1,
                None => f.push((fg, 1)),
            }
            everyone |= w.content.get(fg).is_some_and(|b| b.view().bool("EnemyForEveryone"));
        }
        for &a in &x.attack_factions {
            if !attack.contains(&a) {
                attack.push(a);
            }
        }
    }
    (f, attack, everyone)
}

/// `UnitGroup.IsEnemy(g1, g2)` (static, one direction).
fn is_enemy_dir(w: &World, g1: GroupIdx, g2: GroupIdx) -> bool {
    if g1 == g2 {
        return false;
    }
    let (f1, _, e1) = factions(w, g1);
    let (_, a2, e2) = factions(w, g2);
    if e1 || e2 {
        return true;
    }
    if w.combat.groups.get(g2).is_player_party() {
        return false;
    }
    a2.iter().any(|a| {
        w.content.get(*a).is_some_and(|b| b.view().bool("AlwaysEnemy"))
            || f1.iter().any(|(k, _)| k == a)
    })
}

/// `UnitGroup.IsEnemy(group)`: either direction.
#[must_use]
pub fn is_enemy(w: &World, a: GroupIdx, b: GroupIdx) -> bool {
    is_enemy_dir(w, a, b) || is_enemy_dir(w, b, a)
}

/// `UnitGroup.HasEnemyInCombat()`: another listed group in combat that is an enemy.
#[must_use]
pub fn has_enemy_in_combat(w: &World, g: GroupIdx) -> bool {
    w.combat
        .groups
        .list
        .iter()
        .any(|&o| o != g && w.combat.groups.get(o).in_combat > 0 && is_enemy(w, o, g))
}

/// `UnitCommand.IsOffensiveCommand()` for a recorded command of `executor`: an unfinished attack
/// or ability on another unit that isn't finally dead and that the executor `CanAttack`.
fn is_offensive(w: &mut World, executor: EntityIdx, c: &Command) -> Option<bool> {
    if c.finished || !matches!(c.class.as_str(), "UnitAttack" | "UnitUseAbility") {
        return Some(false);
    }
    let Some(t) = c.target.as_deref() else { return Some(false) };
    let Some(target) = w.by_id.get(t).copied() else {
        w.coverage.unsupported("UnitCombatLeaveController: a command's target is unknown");
        return None;
    };
    if target == executor || w.unit(target).is_some_and(|x| x.finally_dead) {
        return Some(false);
    }
    Some(can_attack(w, executor, target))
}

/// `UnitEntityData.CanAttack(unit)`: a neutral target, or an enemy of the executor's group, or
/// (not an enemy) a confused executor attacking the nearest (confusion isn't modelled: reported).
pub(crate) fn can_attack(w: &mut World, executor: EntityIdx, target: EntityIdx) -> bool {
    if super::units::faction_neutral(w, target) {
        return true;
    }
    let (Some(ga), Some(gb)) = (group_of(w, executor), group_of(w, target)) else { return false };
    if is_enemy(w, ga, gb) {
        return true;
    }
    if w.entity(executor).parts.iter().any(|p| p.clr == "UnitPartConfusion") {
        w.coverage.unsupported("UnitEntityData.CanAttack: a confused unit");
    }
    false
}

/// `UnitEntityData.HasOffensiveCommand()` from the recorded commands; `None` when not recorded.
fn has_offensive_command(w: &mut World, u: EntityIdx) -> Option<bool> {
    let id = w.entity(u).unique_id.clone();
    let cmds = w.combat.groups.commands.as_ref()?.get(&id).cloned().unwrap_or_default();
    // `GetAllCommands().Any(...)`: the first offensive command ends the walk.
    for c in &cmds {
        if is_offensive(w, u, c)? {
            return Some(true);
        }
    }
    Some(false)
}

/// The decision of `ShouldLeaveCombat`, or why it isn't established.
fn should_leave_combat(w: &mut World, g: GroupIdx) -> Option<bool> {
    let tb = w.combat.turn_based;
    let members = w.combat.groups.get(g).units.clone();
    let mut all_down = true;
    let mut offensive = false;
    for &u in &members {
        let down = if tb { super::units::is_dead(w, u) } else { !super::units::is_conscious(w, u) };
        all_down &= down;
        if tb && has_offensive_command(w, u)? {
            offensive = true;
        }
    }
    if all_down {
        return Some(true);
    }
    if offensive {
        return Some(false);
    }
    // An enemy in combat that remembers one of the group's members (outside turn-based combat,
    // one the member also sees: `HasLOS`, vision and line of sight, aren't modelled).
    for u in crate::scene::units_pool(w) {
        if !crate::scene::enumerated_by_pool(w, u) || !super::is_in_combat(w, u) {
            continue;
        }
        let gu = group_of(w, u)?;
        if !is_enemy(w, g, gu) {
            continue;
        }
        let Some(mem) = w.combat.groups.get(gu).memory.clone() else {
            w.coverage.unsupported("UnitCombatLeaveController: a group's memory is not recorded");
            return None;
        };
        if members.iter().any(|m| mem.dict.contains(&w.entity(*m).unique_id)) {
            if !tb {
                w.coverage.unsupported(
                    "UnitCombatLeaveController: ShouldLeaveCombat reads line of sight (HasLOS)",
                );
                return None;
            }
            return Some(false);
        }
    }
    // `ShouldEngageEnemy` over the group's remembered units (`UnitsList`: in the game).
    let Some(mem) = w.combat.groups.get(g).memory.clone() else {
        w.coverage.unsupported("UnitCombatLeaveController: a group's memory is not recorded");
        return None;
    };
    // A remembered unit the sim doesn't have: whether the game's reference resolves (and to what)
    // isn't established.
    let resolve = |w: &mut World, id: &str| {
        let e = w.by_id.get(id).copied();
        if e.is_none() {
            w.coverage
                .unsupported("UnitCombatLeaveController: a remembered unit the sim doesn't have");
        }
        e
    };
    let list = match mem.list {
        Some(l) => l,
        None => {
            let mut l = Vec::new();
            for id in &mem.dict {
                let e = resolve(w, id)?;
                if w.entity(e).is_in_game {
                    l.push(id.clone());
                }
            }
            l
        }
    };
    let player_party = w.combat.groups.get(g).is_player_party();
    for id in list {
        let e = resolve(w, &id)?;
        let ge = group_of(w, e)?;
        if is_enemy(w, g, ge) && !super::units::is_dead(w, e) {
            if player_party && !super::is_in_combat(w, e) {
                if w.unit(e).is_some_and(|x| x.life_state == super::life::state::UNCONSCIOUS) {
                    return Some(false);
                }
            } else {
                return Some(false);
            }
        }
    }
    Some(true)
}

/// What the leave controller's turn-based gate reads from the turn controller.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Gate {
    pub has_enemy: bool,
    pub had_enemy: bool,
    pub passing: bool,
}

/// `UnitCombatLeaveController.Tick()`. `gate` is the turn controller's state (its
/// `HasEnemyInCombat`, `HadEnemyAtSomePoint`, `IsPassing()`).
pub fn leave_tick(w: &mut World, gate: Gate) {
    if !w.combat.groups.on {
        return;
    }
    if w.combat.turn_based {
        if !gate.has_enemy && gate.had_enemy {
            let party = get_group(w, super::units::PLAYER_PARTY_GROUP);
            w.combat.groups.get_mut(party).leave_timer = 3.0;
        } else if !gate.passing {
            return;
        }
    }
    tick_groups(w, false);
}

/// `UnitCombatLeaveController.TickGroups(ignoreTimer)`.
pub fn tick_groups(w: &mut World, ignore_timer: bool) {
    let was_in_combat = w.combat.combat_active;
    for g in w.combat.groups.list.clone() {
        tick_group(w, g, ignore_timer);
    }
    // Awake units in combat that combat ignores, or passive ones, leave.
    if w.combat.awake_unknown.iter().any(|u| super::is_in_combat(w, *u)) {
        w.coverage.unsupported(
            "UnitCombatLeaveController: a unit in combat whose awake state is not established",
        );
    }
    for u in w.combat.awake.clone() {
        let passive = w
            .player
            .counters
            .get(&format!("unit:{}.Passive", w.entity(u).unique_id))
            .is_some_and(|n| *n > 0);
        if super::is_in_combat(w, u)
            && (super::weapon::has_feature(w, u, IS_IGNORED_BY_COMBAT) || passive)
        {
            super::life::leave_combat(w, u);
        }
    }
    // `Player.IsInCombat` only changes in `UpdateIsInCombat` (`UnitCombatJoinController`, cutscene
    // commands), so the party's combat end isn't reached from here.
    if !was_in_combat || w.combat.combat_active {
        return;
    }
    w.coverage
        .unsupported("UnitCombatLeaveController.TickGroups: the party's combat ended inside it");
}

/// `MechanicsFeatureType.IsIgnoredByCombat`.
const IS_IGNORED_BY_COMBAT: i64 = 48;

/// `UnitCombatLeaveController.TickGroup(group, ignoreLeaveTimer)`.
fn tick_group(w: &mut World, g: GroupIdx, ignore_timer: bool) {
    if w.combat.groups.get(g).in_combat <= 0 {
        w.combat.groups.get_mut(g).leave_timer = 0.0;
        return;
    }
    match should_leave_combat(w, g) {
        Some(true) => {}
        Some(false) => {
            w.combat.groups.get_mut(g).leave_timer = 0.0;
            return;
        }
        None => {
            w.coverage
                .unsupported("UnitCombatLeaveController: a group's decision is not established");
            return;
        }
    }
    let delta = w.clock.game_delta_time();
    let grp = w.combat.groups.get_mut(g);
    grp.leave_timer += delta;
    if !ignore_timer && grp.leave_timer < 3.0 {
        return;
    }
    grp.leave_timer = 0.0;
    let members = grp.units.clone();
    if w.combat.turn_based {
        for &u in &members {
            if super::is_in_combat(w, u) {
                super::turns::mark_as_leave(w, u);
            }
        }
    }
    // `group[j]` by index: a member leaving combat stays in the group.
    for i in 0.. {
        let Some(&u) = w.combat.groups.get(g).units.get(i) else { break };
        if super::is_in_combat(w, u) {
            super::life::leave_combat(w, u);
        }
    }
}
