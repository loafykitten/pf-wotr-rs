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
    /// The memory, when it's the sim's ([`Groups::memory_computed`]: `combat::memory::sim`).
    pub mem: Option<crate::combat::memory::GroupMemory<EntityIdx>>,
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
    /// Groups' memory isn't established (a dialogue run: memory follows vision, and nothing
    /// records it there): a group is made with none, so a read of it is reported.
    pub memory_unknown: bool,
    /// Groups' memory is the sim's (`Group::mem`): a group is made with an empty one.
    pub memory_computed: bool,
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
        let memory = (!self.memory_unknown).then(Memory::default);
        let mem = self.memory_computed.then(crate::combat::memory::GroupMemory::new);
        self.arena.push(Group { id, memory, mem, ..Group::default() });
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
    // `m_Factions.Add`, `UpdateAttackFactionsCache()`: the factions are computed from the members
    // when read; the cache update clears the memory's enemy list.
    clear_enemies(w, g);
}

/// `UnitGroup.UpdateAttackFactionsCache()`'s `Memory.ClearEnemies()` (a memory the sim keeps).
fn clear_enemies(w: &mut World, g: GroupIdx) {
    if let Some(m) = w.combat.groups.get_mut(g).mem.as_mut() {
        m.clear_enemies();
    }
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
    clear_enemies(w, g);
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
        if let Some(m) = grp.mem.as_mut() {
            m.clear();
        }
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

/// `UnitGroupsController.Start()` in a dialogue run: as [`start`], the groups' memory unknown.
pub fn start_without_memory(w: &mut World) {
    w.combat.groups = Groups { on: true, memory_unknown: true, ..Groups::default() };
    tick(w);
}

/// `UnitGroupsController.HandleUnitSpawned`: `entityData.Group` (its `UpdateGroup`; the attack
/// factions are computed from the members when read).
pub fn spawned(w: &mut World, u: EntityIdx) {
    if let Some(g) = group_of(w, u) {
        // (`UpdateAttackFactionsCache`: the memory's enemy list cleared.)
        clear_enemies(w, g);
    }
}

/// `UnitGroupsController.HandleUnitDestroyed`: `entityData.Group` (its cache update), then every
/// group's memory forgets the unit (`UnitGroupMemory.Remove`: the sim's memory, else reported
/// where a recorded memory holds it).
pub fn destroyed(w: &mut World, u: EntityIdx) {
    let Some(g) = group_of(w, u) else { return };
    if w.combat.groups.memory_computed {
        clear_enemies(w, g);
        for g in w.combat.groups.list.clone() {
            if let Some(m) = w.combat.groups.get_mut(g).mem.as_mut() {
                m.remove(u);
            }
        }
        return;
    }
    let id = w.entity(u).unique_id.clone();
    let groups = &w.combat.groups;
    if groups
        .list
        .iter()
        .any(|g| groups.get(*g).memory.as_ref().is_some_and(|m| m.dict.contains(&id)))
    {
        w.coverage.unsupported("UnitGroupsController.HandleUnitDestroyed: UnitGroupMemory.Remove");
    }
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
        let remembers = if w.combat.groups.memory_computed {
            let Some(mem) = w.combat.groups.get(gu).mem.as_ref() else {
                w.coverage.unsupported("UnitCombatLeaveController: a group without its memory");
                return None;
            };
            members.iter().any(|m| mem.contains(*m))
        } else {
            let Some(mem) = w.combat.groups.get(gu).memory.clone() else {
                w.coverage
                    .unsupported("UnitCombatLeaveController: a group's memory is not recorded");
                return None;
            };
            members.iter().any(|m| mem.dict.contains(&w.entity(*m).unique_id))
        };
        if remembers {
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
    if w.combat.groups.memory_computed {
        // (The getter fills the cache when it's invalid.)
        let list = crate::combat::memory::sim::with_memory(w, g, |w, m| {
            m.units_list_by(|k| crate::combat::memory::sim::resolve(w, k).in_game()).to_vec()
        });
        let Some(list) = list else {
            w.coverage.unsupported("UnitCombatLeaveController: a group without its memory");
            return None;
        };
        let ids = list.iter().map(|&u| w.entity(u).unique_id.clone()).collect();
        return should_engage_any(w, g, ids);
    }
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
    should_engage_any(w, g, list)
}

/// `ShouldEngageEnemy` over the remembered units of `UnitsList` (`list`, ids): `false` (stay in
/// combat) for the first enemy the group should engage.
fn should_engage_any(w: &mut World, g: GroupIdx, list: Vec<String>) -> Option<bool> {
    let resolve = |w: &mut World, id: &str| {
        let e = w.by_id.get(id).copied();
        if e.is_none() {
            w.coverage
                .unsupported("UnitCombatLeaveController: a remembered unit the sim doesn't have");
        }
        e
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

/// A recorded `grp_tick` (WrathOracle outside turn-based windows, after a
/// `UnitGroupsController.Tick` that changed anything): the groups in order and every unit's
/// cached group.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupTick {
    /// Each group: its id, units, combat count and leave timer (bits).
    pub groups: Vec<(String, Vec<String>, i64, u32)>,
    /// Each unit of `State.Units`, in order: its id and cached group (`GroupObs.GroupKey`: the
    /// id, `~fake`, `~disposed:<id>`; `None` without one).
    pub units: Vec<(String, Option<String>)>,
}

/// The recording's group states by oracle tick (a dialogue run with `group_ticks`).
#[derive(Debug, Default)]
pub struct RecordedGroups {
    pub by_tick: std::collections::BTreeMap<i64, GroupTick>,
    /// The state the last recorded change left.
    pub last: Option<GroupTick>,
    /// The first check ran (it takes the recorded list order).
    pub checked: bool,
}

impl RecordedGroups {
    /// From a recording's records: `Ok(None)` when it doesn't declare `group_ticks`.
    ///
    /// # Errors
    ///
    /// A malformed record, or two in one tick.
    pub fn of(records: &[serde_json::Value]) -> Result<Option<Self>, String> {
        let declared = records.iter().find(|r| r["k"] == "begin").is_some_and(|b| {
            b["observers"].as_array().is_some_and(|a| a.iter().any(|o| o == "group_ticks"))
        });
        if !declared {
            return Ok(None);
        }
        let mut out = Self::default();
        for r in records.iter().filter(|r| r["k"] == "obs" && r["ev"] == "grp_tick") {
            let t = r["t"].as_i64().ok_or("grp_tick without its tick")?;
            let ids = |v: &serde_json::Value| -> Result<Vec<String>, String> {
                v.as_array()
                    .ok_or("grp_tick: units aren't an array")?
                    .iter()
                    .map(|x| x.as_str().map(str::to_owned).ok_or_else(|| format!("grp_tick: {x}")))
                    .collect()
            };
            let mut groups = Vec::new();
            for g in r["groups"].as_array().ok_or("grp_tick: groups aren't an array")? {
                let (Some(id), Some(ic), Some(timer)) =
                    (g["id"].as_str(), g["in_combat"].as_i64(), g["timer"].as_f64())
                else {
                    return Err(format!("grp_tick: a group {g}"));
                };
                #[allow(clippy::cast_possible_truncation)]
                groups.push((id.to_owned(), ids(&g["units"])?, ic, (timer as f32).to_bits()));
            }
            let mut units = Vec::new();
            for u in r["units"].as_array().ok_or("grp_tick: units aren't an array")? {
                let (Some(id), key) = (u[0].as_str(), &u[1]) else {
                    return Err(format!("grp_tick: a unit {u}"));
                };
                if !(key.is_null() || key.is_string()) || u.as_array().is_none_or(|a| a.len() != 2)
                {
                    return Err(format!("grp_tick: a unit {u}"));
                }
                units.push((id.to_owned(), key.as_str().map(str::to_owned)));
            }
            if out.by_tick.insert(t, GroupTick { groups, units }).is_some() {
                return Err(format!("two grp_tick records in tick {t}"));
            }
        }
        Ok(Some(out))
    }
}

/// The sim's groups as WrathOracle writes them, the run-created ids renamed to the recording's
/// (`RecordedAwake::mapped`).
fn group_tick_of(w: &mut World) -> GroupTick {
    let pool: Vec<EntityIdx> = crate::scene::units_pool(w)
        .into_iter()
        .filter(|u| crate::scene::enumerated_by_pool(w, *u))
        .collect();
    let w = &*w;
    let renamed: HashMap<String, String> = w
        .recorded_awake
        .as_ref()
        .map(|a| {
            a.mapped.iter().map(|(rec, e)| (w.entity(*e).unique_id.clone(), rec.clone())).collect()
        })
        .unwrap_or_default();
    let name = |id: &str| renamed.get(id).cloned().unwrap_or_else(|| id.to_owned());
    let groups = &w.combat.groups;
    let gs = groups
        .list
        .iter()
        .map(|g| {
            let grp = groups.get(*g);
            (
                name(grp.id.as_deref().unwrap_or_default()),
                grp.units.iter().map(|u| name(&w.entity(*u).unique_id)).collect(),
                i64::from(grp.in_combat),
                grp.leave_timer.to_bits(),
            )
        })
        .collect();
    let units = pool
        .into_iter()
        .map(|u| {
            let key = groups.cached.get(&u).map(|g| {
                let grp = groups.get(*g);
                let id = name(grp.id.as_deref().unwrap_or_default());
                if grp.is_fake() {
                    "~fake".to_owned()
                } else if grp.disposed {
                    format!("~disposed:{id}")
                } else {
                    id
                }
            });
            (name(&w.entity(u).unique_id), key)
        })
        .collect();
    GroupTick { groups: gs, units }
}

/// After the sim's `UnitGroupsController.Tick` in the oracle's tick `t`: the groups and every
/// unit's cached group equal the recording's latest state (written when it changed). The first
/// check takes the recorded list order when the groups are the same (the list's order is the
/// order of the area load's first `Group` accesses, which aren't modelled). A record in a tick
/// the sim's controller didn't run is reported.
pub fn check_recorded(w: &mut World, t: i64) {
    let Some(mut rec) = w.recorded_groups.take() else { return };
    if rec.by_tick.range(..t).next().is_some() {
        rec.by_tick.retain(|k, _| *k >= t);
        w.coverage.unsupported(
            "dialogue replay: a recorded unit group state in a tick the sim's UnitGroupsController didn't run",
        );
    }
    if let Some(g) = rec.by_tick.remove(&t) {
        rec.last = Some(g);
    }
    let Some(want) = rec.last.clone() else {
        w.coverage.unsupported("dialogue replay: the unit groups tick before any recorded state");
        w.recorded_groups = Some(rec);
        return;
    };
    // The run-created units the state names, matched to the sim's before the renaming (a unit
    // created last tick may not have been in an awake record yet).
    if let Some(mut awake) = w.recorded_awake.take() {
        for id in want.units.iter().map(|u| &u.0) {
            if awake.new_bp.contains_key(id) && !awake.mapped.contains_key(id) {
                awake.resolve(w, id);
            }
        }
        w.recorded_awake = Some(awake);
    }
    if !rec.checked {
        rec.checked = true;
        let ours = group_tick_of(w);
        let mut a: Vec<&String> = ours.groups.iter().map(|g| &g.0).collect();
        let mut b: Vec<&String> = want.groups.iter().map(|g| &g.0).collect();
        a.sort();
        b.sort();
        if a == b {
            let order: Vec<GroupIdx> = want
                .groups
                .iter()
                .filter_map(|g| ours.groups.iter().position(|o| o.0 == g.0))
                .map(|i| w.combat.groups.list[i])
                .collect();
            w.combat.groups.list = order;
        }
    }
    let ours = group_tick_of(w);
    if ours != want {
        if std::env::var_os("WR_DEBUG_GROUPS").is_some() {
            let only = |a: &GroupTick, b: &GroupTick| {
                let g: Vec<_> = a.groups.iter().filter(|x| !b.groups.contains(x)).collect();
                let u: Vec<_> = a.units.iter().filter(|x| !b.units.contains(x)).collect();
                format!("groups {g:?} units {u:?}")
            };
            eprintln!(
                "groups t={t}: ours only {}; recorded only {}; order equal {}",
                only(&ours, &want),
                only(&want, &ours),
                ours.groups.iter().map(|g| &g.0).eq(want.groups.iter().map(|g| &g.0))
                    && ours.units.iter().map(|u| &u.0).eq(want.units.iter().map(|u| &u.0))
            );
        }
        w.coverage.unsupported("dialogue replay: the unit groups differ from the recording's");
    }
    w.recorded_groups = Some(rec);
}

/// The run ended (at the oracle's tick `last`): a recorded state it didn't check is reported,
/// in a tick it ran or after its end.
pub fn recorded_finish(w: &mut World, last: i64) {
    let Some(r) = w.recorded_groups.as_ref() else { return };
    if r.by_tick.range(..=last).next().is_some() {
        w.coverage.unsupported(
            "dialogue replay: a recorded unit group state in a tick the sim's UnitGroupsController didn't run",
        );
    }
    if r.by_tick.range(last + 1..).next().is_some() {
        w.coverage.unsupported("dialogue replay: a recorded unit group state after the run's end");
    }
}
