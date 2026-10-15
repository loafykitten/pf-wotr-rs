//! Group memory (decompiled, not copied): `Kingmaker.UnitLogic.UnitGroupMemory`, and
//! `Kingmaker.Controllers.Units.UnitMemoryController`'s tick, visibility update, queued additions
//! and handlers. docs/poc5/memory.md.
//!
//! Pure: generic over the unit key `K` (the sim's `EntityIdx`, or a recording's interned unique
//! id) and the group key `G`. What the controller reads outside the memory (the ready groups, the
//! units in vision range, line of sight, stealth, factions, the clocks, a unit's identity hash)
//! comes through [`MemoryWorld`]; nothing here guesses it. Not wired into the driven replay yet:
//! the recordings don't carry what the tick reads at each call (docs/poc5/memory.md, "Observer
//! changes requested"), so the replay keeps taking the memory from WrathOracle's `leave_tick` rows.
//!
//! The memory (`m_Units`) is a .NET `Dictionary<string, UnitInfo>` keyed by unique id: its
//! enumeration order (slots, the LIFO free list) is [`CsDict`]'s. `UnitsList` and `Enemies` are
//! caches with their own validity flags: `Add` (even of a key already there), a removing `Remove`
//! and `Clear` invalidate both, `ClearEnemies` (every `UnitGroup.UpdateAttackFactionsCache`) the
//! enemies' one, and `Cleanup` neither (it removes from the dictionary directly), so a list read
//! after a `Cleanup` can still hold the units it removed.

use std::hash::Hash;

use crate::csharp::arith::CsError;
use crate::csharp::{CsDict, TimeSpan};

/// `9f.Seconds()` (`TimeSpan.FromSeconds(9.0)`, whole milliseconds): how long `Cleanup` keeps a unit
/// that wasn't detected again.
pub const FORGET_AFTER: TimeSpan = TimeSpan(90_000_000);

/// A failure the game would throw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemError {
    /// A `System.Exception` from the runtime's arithmetic (`TimeSpan` overflow).
    Cs(CsError),
    /// `NullReferenceException`: `HasPlayerCharacterInMemory` reading a cached entry whose unit no
    /// longer resolves.
    NullReference,
}

impl From<CsError> for MemError {
    fn from(e: CsError) -> Self {
        Self::Cs(e)
    }
}

/// `UnitGroupMemory.UnitInfo` (its `UnitReference` is the dictionary's key).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitInfo {
    /// `LastDetectTime` (game time).
    pub last_detect_time: TimeSpan,
    /// `Visible` (not saved; a new entry's is false).
    pub visible: bool,
}

/// What `UnitInfo.Unit` (`UnitReference.Value`) gives when the memory reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolved {
    /// `null`: no entity for the id (destroyed, or never loaded).
    Missing,
    /// A unit, and its `IsInGame`.
    Unit { in_game: bool },
}

impl Resolved {
    /// `unit != null && unit.IsInGame`.
    #[must_use]
    pub fn in_game(self) -> bool {
        matches!(self, Self::Unit { in_game: true })
    }
}

/// `UnitGroupMemory`.
#[derive(Debug, Clone)]
pub struct GroupMemory<K> {
    /// `m_Units`.
    units: CsDict<K, UnitInfo>,
    /// `m_UnitsList`, `m_UnitsListValid`.
    units_list: Vec<K>,
    units_list_valid: bool,
    /// `m_EnemiesList`, `m_EnemiesListValid`.
    enemies: Vec<K>,
    enemies_valid: bool,
}

impl<K: Copy + Eq + Hash> Default for GroupMemory<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Copy + Eq + Hash> GroupMemory<K> {
    /// `new UnitGroupMemory(groupId)`: empty, both caches invalid.
    #[must_use]
    pub fn new() -> Self {
        Self {
            units: CsDict::new(),
            units_list: Vec::new(),
            units_list_valid: false,
            enemies: Vec::new(),
            enemies_valid: false,
        }
    }

    /// A memory holding `keys` in this enumeration order, with no freed slots between them (what a
    /// dictionary that only ever had keys added holds), both caches invalid. Each entry's
    /// `LastDetectTime` and `Visible` are given.
    #[must_use]
    pub fn from_entries(entries: impl IntoIterator<Item = (K, UnitInfo)>) -> Self {
        let mut m = Self::new();
        for (k, info) in entries {
            m.units.insert(k, info);
        }
        m
    }

    /// `Add(unit)`: both caches invalidated; a unit not yet remembered gets an entry (not visible,
    /// `LastDetectTime` now), in the dictionary's next slot. Returns the entry.
    pub fn add(&mut self, k: K, game_time: TimeSpan) -> &mut UnitInfo {
        self.enemies_valid = false;
        self.units_list_valid = false;
        self.units
            .get_or_insert_with(k, || UnitInfo { last_detect_time: game_time, visible: false })
    }

    /// `Remove(unit)`: the caches are invalidated only when the unit was there.
    pub fn remove(&mut self, k: K) -> bool {
        if self.units.remove(&k).is_some() {
            self.enemies_valid = false;
            self.units_list_valid = false;
            true
        } else {
            false
        }
    }

    /// `Find(unit)`.
    #[must_use]
    pub fn find(&self, k: K) -> Option<&UnitInfo> {
        self.units.get(&k)
    }

    pub fn find_mut(&mut self, k: K) -> Option<&mut UnitInfo> {
        self.units.get_mut(&k)
    }

    /// `Contains(unit)`.
    #[must_use]
    pub fn contains(&self, k: K) -> bool {
        self.units.contains_key(&k)
    }

    /// `ContainsVisible(unit)`.
    #[must_use]
    pub fn contains_visible(&self, k: K) -> bool {
        self.units.get(&k).is_some_and(|i| i.visible)
    }

    /// `Clear()`.
    pub fn clear(&mut self) {
        self.units_list_valid = false;
        self.clear_enemies();
        self.units_list.clear();
        self.units.clear();
    }

    /// `ClearEnemies()` (from `UnitGroup.UpdateAttackFactionsCache`).
    pub fn clear_enemies(&mut self) {
        self.enemies_valid = false;
        self.enemies.clear();
    }

    /// `Cleanup()`: every entry whose unit doesn't resolve, wasn't detected for more than 9 s
    /// (`LastDetectTime + 9 s < GameTime`), or isn't in the game is removed, in enumeration order
    /// (so the last one removed heads the free list). The caches are left as they are. Returns the
    /// removed keys in that order.
    ///
    /// # Errors
    ///
    /// `LastDetectTime + 9 s` overflowing (`OverflowException`).
    pub fn cleanup(
        &mut self,
        game_time: TimeSpan,
        mut resolve: impl FnMut(K) -> Resolved,
    ) -> Result<Vec<K>, MemError> {
        let mut gone = Vec::new();
        for (k, info) in self.units.iter() {
            let remove = match resolve(*k) {
                Resolved::Missing => true,
                Resolved::Unit { in_game } => {
                    info.last_detect_time.checked_add(FORGET_AFTER)? < game_time || !in_game
                }
            };
            if remove {
                gone.push(*k);
            }
        }
        for k in &gone {
            self.units.remove(k);
        }
        Ok(gone)
    }

    /// `UnitsList`: when invalid, rebuilt from the dictionary (the units that resolve and are in the
    /// game, in enumeration order); else the cached list.
    pub fn units_list(&mut self, mut resolve: impl FnMut(K) -> Resolved) -> &[K] {
        if !self.units_list_valid {
            self.units_list.clear();
            for k in self.units.keys() {
                if resolve(k).in_game() {
                    self.units_list.push(k);
                }
            }
            self.units_list_valid = true;
        }
        &self.units_list
    }

    /// `Enemies`: when invalid, rebuilt from the dictionary (the units that resolve, are in the game
    /// and are the memory's group's enemies: `is_enemy`, asked only of those), else the cached list.
    pub fn enemies(
        &mut self,
        mut resolve: impl FnMut(K) -> Resolved,
        mut is_enemy: impl FnMut(K) -> bool,
    ) -> &[K] {
        if !self.enemies_valid {
            self.enemies.clear();
            for k in self.units.keys() {
                if resolve(k).in_game() && is_enemy(k) {
                    self.enemies.push(k);
                }
            }
            self.enemies_valid = true;
        }
        &self.enemies
    }

    /// `HasPlayerCharacterInMemory()`: any unit of `UnitsList` (filled if invalid) of the player's
    /// faction. `is_player_faction` is asked of each cached unit that resolves.
    ///
    /// # Errors
    ///
    /// A cached unit that no longer resolves (`NullReferenceException`).
    pub fn has_player_character_in_memory(
        &mut self,
        mut resolve: impl FnMut(K) -> Resolved,
        mut is_player_faction: impl FnMut(K) -> bool,
    ) -> Result<bool, MemError> {
        let list = self.units_list(&mut resolve).to_vec();
        for k in list {
            if resolve(k) == Resolved::Missing {
                return Err(MemError::NullReference);
            }
            if is_player_faction(k) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The dictionary's keys in enumeration order.
    pub fn keys(&self) -> impl Iterator<Item = K> + '_ {
        self.units.keys()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.units.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.units.is_empty()
    }

    /// `m_UnitsList` while valid.
    #[must_use]
    pub fn units_list_cache(&self) -> Option<&[K]> {
        self.units_list_valid.then_some(self.units_list.as_slice())
    }

    /// `m_EnemiesList` while valid.
    #[must_use]
    pub fn enemies_cache(&self) -> Option<&[K]> {
        self.enemies_valid.then_some(self.enemies.as_slice())
    }
}

/// `UnitMemoryController`'s own state: `m_UnitsForAdd`, the (observer, target) pairs queued by its
/// handlers (and by `StartCombat`, random encounters, dungeons) for the next tick. `m_NearUnits` is
/// cleared before each use, so it isn't state between ticks.
#[derive(Debug, Clone, Default)]
pub struct MemoryController<K> {
    pub units_for_add: Vec<(K, K)>,
}

/// What `HandleUnitCommandDidAct` reads of the command that acted.
#[derive(Debug, Clone, Copy)]
pub struct ActedCommand<K> {
    pub executor: K,
    /// `command.TargetUnit`.
    pub target: Option<K>,
    /// `Executor.IsPlayersEnemy`.
    pub executor_players_enemy: bool,
    /// `TargetUnit.IsPlayerFaction`.
    pub target_player_faction: bool,
    /// `TargetUnit.Descriptor.State.HasCondition(UnitCondition.Invisible)`.
    pub target_invisible: bool,
    /// The executor's and the target's `Faction.Peaceful` (`AddToMemory` reads its target's).
    pub executor_peaceful: bool,
    pub target_peaceful: bool,
}

/// What `HandleUnitMakeOffensiveAction(unit, target)` reads for the memory. Its other effects are
/// the caller's: the target's `UnitAggroFilter.OnAggroAction` (before everything), `unit`'s
/// `CombatState.LastSurpriseActionTime` when it isn't in combat, and a neutral target's
/// `AttackFactions.Add(unit's faction)` (after the additions).
#[derive(Debug, Clone, Copy)]
pub struct OffensiveAction<K> {
    pub unit: K,
    pub target: K,
    /// `unit.IsInGame && target.IsInGame` (else no addition).
    pub both_in_game: bool,
    /// `target.Descriptor.State.HasCondition(UnitCondition.Invisible)`.
    pub target_invisible: bool,
    /// `unit.Descriptor.IsSeeInvisibility`.
    pub unit_sees_invisibility: bool,
    /// `unit.Get<UnitPartBlindsense>()?.Reach(target) ?? false` (read only for an invisible target
    /// the unit can't see).
    pub unit_blindsense_reaches: bool,
    pub unit_peaceful: bool,
    pub target_peaceful: bool,
}

impl<K: Copy + Eq> MemoryController<K> {
    /// `AddToMemory(observer, target)`: queued unless the target's faction is peaceful or the same
    /// pair is queued already.
    pub fn add_to_memory(&mut self, observer: K, target: K, target_peaceful: bool) {
        if !target_peaceful && !self.units_for_add.contains(&(observer, target)) {
            self.units_for_add.push((observer, target));
        }
    }

    /// `Start()` and `Stop()`'s own part: the queue cleared (each also clears every group's memory,
    /// the caller's).
    pub fn clear_queue(&mut self) {
        self.units_for_add.clear();
    }

    /// `HandleUnitCommandDidAct(command)`: a players' enemy's command on a unit of the player's
    /// faction makes each remember the other (the executor only a target that isn't invisible).
    pub fn handle_unit_command_did_act(&mut self, c: &ActedCommand<K>) {
        let Some(t) = c.target else { return };
        if c.executor_players_enemy && c.target_player_faction {
            if !c.target_invisible {
                self.add_to_memory(c.executor, t, c.target_peaceful);
            }
            self.add_to_memory(t, c.executor, c.executor_peaceful);
        }
    }

    /// `HandleUnitMakeOffensiveAction(unit, target)`'s additions: with both in the game, `unit`
    /// remembers `target` unless it's invisible to it, and `target` remembers `unit`.
    pub fn handle_offensive_action(&mut self, a: &OffensiveAction<K>) {
        if !a.both_in_game {
            return;
        }
        let hidden = a.target_invisible && !a.unit_sees_invisibility && !a.unit_blindsense_reaches;
        if !hidden {
            self.add_to_memory(a.unit, a.target, a.target_peaceful);
        }
        self.add_to_memory(a.target, a.unit, a.unit_peaceful);
    }
}

/// What `UnitMemoryController.Tick` reads and calls outside the memory. Each method is one read
/// (or call) of the game's code, asked where the game asks it.
pub trait MemoryWorld<K, G> {
    type Error: From<MemError>;
    /// `LoadedAreaState.Settings.Peaceful`.
    fn area_peaceful(&mut self) -> Result<bool, Self::Error>;
    /// `ReadyForCombatUnitGroups` (`SleepingUnitsController`'s list), in order.
    fn ready_groups(&mut self) -> Result<Vec<G>, Self::Error>;
    /// `ExtractNearUnits`: the union of the group's members' `CanBeInVisionRange`, in the `HashSet`'s
    /// enumeration order (each member's set in its own order, first sight kept).
    fn near_units(&mut self, g: G) -> Result<Vec<K>, Self::Error>;
    /// `nearUnit.Faction.Peaceful || nearUnit.Passive`.
    fn ignored(&mut self, u: K) -> Result<bool, Self::Error>;
    /// `unit.Group` (it runs `UpdateGroup`).
    fn group_of(&mut self, u: K) -> Result<G, Self::Error>;
    /// `unit.InStealthFor(group)`.
    fn in_stealth_for(&mut self, u: K, g: G) -> Result<bool, Self::Error>;
    /// `group.IsEnemy(unit)`.
    fn is_enemy(&mut self, g: G, u: K) -> Result<bool, Self::Error>;
    /// `TimeController.GameDeltaTime`.
    fn game_delta_time(&mut self) -> Result<f32, Self::Error>;
    /// `Time.frameCount` (WrathOracle pins it in a replay).
    fn frame_count(&mut self) -> Result<i32, Self::Error>;
    /// `unit.GetHashCode()`: the object's identity hash (`UnitEntityData` doesn't override it).
    fn unit_hash(&mut self, u: K) -> Result<i32, Self::Error>;
    /// `group.HasLOS(unit)`: a member in the game, not helpless, with `HasLOS(unit)` (its vision
    /// range set and its sight cache).
    fn group_has_los(&mut self, g: G, u: K) -> Result<bool, Self::Error>;
    /// `unit.Descriptor.State.HasCondition(UnitCondition.Invisible)`.
    fn invisible(&mut self, u: K) -> Result<bool, Self::Error>;
    /// The loop over the group's members for an invisible target: a member that sees invisibility
    /// and has line of sight to it, or whose blindsense reaches it.
    fn sees_invisible(&mut self, g: G, u: K) -> Result<bool, Self::Error>;
    /// `TimeController.GameTime`.
    fn game_time(&mut self) -> Result<TimeSpan, Self::Error>;
    /// The group's memory.
    fn memory(&mut self, g: G) -> &mut GroupMemory<K>;
    /// `UnitInfo.Unit` (`UnitReference.Value`), as `Cleanup` reads it.
    fn resolve(&mut self, u: K) -> Result<Resolved, Self::Error>;
    /// `unit.IsDisposed`.
    fn disposed(&mut self, u: K) -> Result<bool, Self::Error>;
    /// The group a queued addition goes to: `observer.Group.IsFake ?
    /// UnitGroupsController.GetGroup(observer.GroupId) : observer.Group`.
    fn observer_group(&mut self, u: K) -> Result<G, Self::Error>;
    /// `observer.Wake()` (`AwakeTimer = Max(0, AwakeTimer)`).
    fn wake(&mut self, u: K) -> Result<(), Self::Error>;
}

/// `UnitMemoryController.Tick()`.
///
/// # Errors
///
/// Whatever the world reports, and the game's own exceptions ([`MemError`]).
pub fn tick<K, G, W>(ctl: &mut MemoryController<K>, w: &mut W) -> Result<(), W::Error>
where
    K: Copy + Eq + Hash,
    G: Copy + Eq,
    W: MemoryWorld<K, G>,
{
    // A peaceful area returns before anything, the queue kept.
    if w.area_peaceful()? {
        return Ok(());
    }
    let now = w.game_time()?;
    for g in w.ready_groups()? {
        for u in w.near_units(g)? {
            if w.ignored(u)? {
                continue;
            }
            if w.group_of(u)? == g {
                w.memory(g).add(u, now).last_detect_time = now;
                continue;
            }
            update_visibility(w, g, u, now)?;
        }
        let keys: Vec<K> = w.memory(g).keys().collect();
        let mut resolved = Vec::with_capacity(keys.len());
        for k in keys {
            resolved.push((k, w.resolve(k)?));
        }
        let lookup =
            |k: K| resolved.iter().find(|(x, _)| *x == k).map_or(Resolved::Missing, |r| r.1);
        w.memory(g).cleanup(now, lookup)?;
    }
    // The queue is cleared after the loop: a failure part way keeps every pair (as the game's
    // exception would).
    for (o, t) in ctl.units_for_add.clone() {
        if w.disposed(o)? || w.disposed(t)? {
            continue;
        }
        let og = w.group_of(o)?;
        if w.in_stealth_for(t, og)? {
            continue;
        }
        let g = w.observer_group(o)?;
        w.memory(g).add(t, now).last_detect_time = now;
        w.wake(o)?;
    }
    ctl.units_for_add.clear();
    Ok(())
}

/// `UpdateVisibility(group, target)` for a near unit of another group.
fn update_visibility<K, G, W>(w: &mut W, g: G, t: K, now: TimeSpan) -> Result<(), W::Error>
where
    K: Copy + Eq + Hash,
    G: Copy + Eq,
    W: MemoryWorld<K, G>,
{
    if w.in_stealth_for(t, g)? {
        w.memory(g).remove(t);
        return Ok(());
    }
    // Friends are looked at on one frame in five, by their identity hash
    // (`(uint)(GetHashCode() >>> 2) % 5u` against `Time.frameCount % 5`), unless the frame is long.
    if !w.is_enemy(g, t)? && w.game_delta_time()? < 0.5 {
        #[allow(clippy::cast_sign_loss)]
        let phase = ((w.unit_hash(t)? as u32) >> 2) % 5;
        if i64::from(w.frame_count()? % 5) != i64::from(phase) {
            return Ok(());
        }
    }
    if w.group_has_los(g, t)? {
        // Added and visible first; then the invisibility check (a failure there leaves that state).
        w.memory(g).add(t, now).visible = true;
        if w.invisible(t)? {
            let seen = w.sees_invisible(g, t)?;
            if let Some(info) = w.memory(g).find_mut(t) {
                info.visible = seen;
            }
        }
        if let Some(info) = w.memory(g).find_mut(t).filter(|i| i.visible) {
            info.last_detect_time = now;
        }
        return Ok(());
    }
    if let Some(info) = w.memory(g).find_mut(t) {
        info.visible = false;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
