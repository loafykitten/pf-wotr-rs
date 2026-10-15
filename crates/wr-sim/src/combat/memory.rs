//! Group memory (decompiled, not copied): `Kingmaker.UnitLogic.UnitGroupMemory`, and
//! `Kingmaker.Controllers.Units.UnitMemoryController`'s tick, visibility update, queued additions
//! and handlers. docs/poc5/memory.md.
//!
//! Pure: generic over the unit key `K` (the sim's `EntityIdx`, or a recording's interned unique
//! id) and the group key `G`. What the controller reads outside the memory (the ready groups, the
//! units in vision range, line of sight, stealth, factions, the clocks, a unit's identity hash)
//! comes through [`MemoryWorld`], one method per read, asked in the game's order; nothing here
//! guesses it. WrathOracle's `memory` observer records each of those reads and the memory after
//! each step (`tick_tests.rs` reruns every recorded tick against them). Not wired into the driven
//! replay yet (docs/poc5/memory.md, "H1: result", wiring notes): the replay keeps taking the
//! memory from WrathOracle's `leave_tick` rows.
//!
//! The memory (`m_Units`) is a .NET `Dictionary<string, UnitInfo>` keyed by unique id: its
//! enumeration order (slots, the LIFO free list) is [`CsDict`]'s. `UnitsList` and `Enemies` are
//! caches with their own validity flags: `Add` (even of a key already there), a removing `Remove`
//! and `Clear` invalidate both, `ClearEnemies` (every `UnitGroup.UpdateAttackFactionsCache`) the
//! enemies' one, and `Cleanup` neither (it removes from the dictionary directly), so a list read
//! after a `Cleanup` can still hold the units it removed.

use std::hash::Hash;

use crate::csharp::arith::CsError;
use crate::csharp::{CsDict, RawSlot, TimeSpan};

/// `9f.Seconds()` (`TimeSpan.FromSeconds(9.0)`, whole milliseconds): how long `Cleanup` keeps a unit
/// that wasn't detected again.
pub const FORGET_AFTER: TimeSpan = TimeSpan(90_000_000);

/// `UnitCondition.Invisible`.
pub const INVISIBLE: i32 = 7;

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
    /// The keys `Add` and `Remove` changed, in order, while logging ([`Self::log_changes`]): what
    /// WrathOracle's `mem_add` and `mem_remove` (by `Remove`) record.
    log: Option<Vec<Change<K>>>,
}

/// A change `Add` or `Remove` made to a memory's keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change<K> {
    /// `Add` of a key not yet there.
    Added(K),
    /// `Remove` of a key that was there.
    Removed(K),
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
            log: None,
        }
    }

    /// From now on, `Add` and `Remove` log the keys they change.
    pub fn log_changes(&mut self) {
        self.log.get_or_insert_with(Vec::new);
    }

    /// The changes logged since the last call.
    pub fn take_changes(&mut self) -> Vec<Change<K>> {
        self.log.as_mut().map(std::mem::take).unwrap_or_default()
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
        if let Some(log) = self.log.as_mut().filter(|_| !self.units.contains_key(&k)) {
            log.push(Change::Added(k));
        }
        self.units
            .get_or_insert_with(k, || UnitInfo { last_detect_time: game_time, visible: false })
    }

    /// `Remove(unit)`: the caches are invalidated only when the unit was there.
    pub fn remove(&mut self, k: K) -> bool {
        if self.units.remove(&k).is_some() {
            self.enemies_valid = false;
            self.units_list_valid = false;
            if let Some(log) = self.log.as_mut() {
                log.push(Change::Removed(k));
            }
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
    /// keys removed from this memory, in that order.
    ///
    /// `pending` is `s_UnitsForRemove`, a static list every memory shares: the entries are collected
    /// into it and it's cleared only after the removals, so an exception part way leaves what was
    /// collected there, and the next `Cleanup` of any memory removes those keys too (where present).
    ///
    /// # Errors
    ///
    /// `LastDetectTime + 9 s` overflowing (`OverflowException`).
    pub fn cleanup(
        &mut self,
        game_time: TimeSpan,
        mut resolve: impl FnMut(K) -> Resolved,
        pending: &mut Vec<K>,
    ) -> Result<Vec<K>, MemError> {
        for (k, info) in self.units.iter() {
            if cleanup_removes(info, resolve(*k), game_time)? {
                pending.push(*k);
            }
        }
        Ok(self.cleanup_finish(pending))
    }

    /// The end of `Cleanup()`: the collected keys (`pending`, `s_UnitsForRemove`) removed from this
    /// memory where present, in order, and the list cleared. Returns the keys removed.
    pub fn cleanup_finish(&mut self, pending: &mut Vec<K>) -> Vec<K> {
        let mut gone = Vec::new();
        for k in std::mem::take(pending) {
            if self.units.remove(&k).is_some() {
                gone.push(k);
            }
        }
        gone
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

    /// A memory with this layout (WrathOracle's `mem_init` and `mem_s`); `None` when it isn't a
    /// dictionary's ([`CsDict::from_slots`]).
    #[must_use]
    pub fn from_layout(l: MemoryLayout<K>) -> Option<Self> {
        Some(Self {
            units: CsDict::from_slots(l.slots, l.free_list, l.free_count)?,
            units_list_valid: l.units_list.is_some(),
            units_list: l.units_list.unwrap_or_default(),
            enemies_valid: l.enemies.is_some(),
            enemies: l.enemies.unwrap_or_default(),
            log: None,
        })
    }

    /// The memory's layout: the dictionary's slots, its free list (-1 while no slot is free) and
    /// both caches while valid.
    #[must_use]
    pub fn layout(&self) -> MemoryLayout<K> {
        let (slots, free_list, free_count) = self.units.slots();
        MemoryLayout {
            slots: slots
                .into_iter()
                .map(|s| match s {
                    RawSlot::Used(k, v) => RawSlot::Used(k, *v),
                    RawSlot::Free(next) => RawSlot::Free(next),
                })
                .collect(),
            free_list,
            free_count,
            units_list: self.units_list_cache().map(<[K]>::to_vec),
            enemies: self.enemies_cache().map(<[K]>::to_vec),
        }
    }
}

/// A memory as the runtime holds it: `m_Units`' slots (`entries[0, count)`), `freeList` (-1 while
/// no slot is free), `freeCount`, and the caches' contents while valid (`None`: invalid).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryLayout<K> {
    pub slots: Vec<RawSlot<K, UnitInfo>>,
    pub free_list: i32,
    pub free_count: usize,
    pub units_list: Option<Vec<K>>,
    pub enemies: Option<Vec<K>>,
}

/// One entry of `Cleanup()`'s walk: its unit doesn't resolve, or wasn't detected for more than 9 s
/// (`LastDetectTime + 9 s < GameTime`), or isn't in the game.
///
/// # Errors
///
/// `LastDetectTime + 9 s` overflowing (`OverflowException`), for a unit that resolves.
pub fn cleanup_removes(
    info: &UnitInfo,
    resolved: Resolved,
    game_time: TimeSpan,
) -> Result<bool, MemError> {
    Ok(match resolved {
        Resolved::Missing => true,
        Resolved::Unit { in_game } => {
            info.last_detect_time.checked_add(FORGET_AFTER)? < game_time || !in_game
        }
    })
}

/// `UnitMemoryController`'s own state: `m_UnitsForAdd`, the (observer, target) pairs queued by its
/// handlers (and by `StartCombat`, random encounters, dungeons) for the next tick. `m_NearUnits` is
/// cleared before each use, so it isn't state between ticks.
#[derive(Debug, Clone, Default)]
pub struct MemoryController<K> {
    pub units_for_add: Vec<(K, K)>,
    /// `UnitGroupMemory.s_UnitsForRemove` (static, shared by every memory's `Cleanup`): empty
    /// unless a `Cleanup` threw part way.
    pub cleanup_pending: Vec<K>,
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
/// (or call) of the game's code, asked where and in the order the game asks it.
pub trait MemoryWorld<K, G> {
    type Error: From<MemError>;
    /// `LoadedAreaState.Settings.Peaceful`.
    fn area_peaceful(&mut self) -> Result<bool, Self::Error>;
    /// `ReadyForCombatUnitGroups` (`SleepingUnitsController`'s list), in order.
    fn ready_groups(&mut self) -> Result<Vec<G>, Self::Error>;
    /// `ExtractNearUnits`: the union of the group's members' `CanBeInVisionRange`, in the `HashSet`'s
    /// enumeration order (each member's set in its own order, first sight kept).
    fn near_units(&mut self, g: G) -> Result<Vec<K>, Self::Error>;
    /// `nearUnit.Faction.Peaceful`.
    fn faction_peaceful(&mut self, u: K) -> Result<bool, Self::Error>;
    /// `(bool)nearUnit.Passive` (read only for a unit whose faction isn't peaceful).
    fn passive(&mut self, u: K) -> Result<bool, Self::Error>;
    /// `unit.Group` (it runs `UpdateGroup`).
    fn group_of(&mut self, u: K) -> Result<G, Self::Error>;
    /// `group.IsFake` (`Id == null`).
    fn is_fake(&mut self, g: G) -> bool;
    /// `UnitGroupsController.GetGroup(unit.GroupId)` (it can create the group).
    fn group_by_id(&mut self, u: K) -> Result<G, Self::Error>;
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
    /// `TimeController.GameTime` (the game reads it at each use; nothing in the tick changes it).
    fn game_time(&mut self) -> Result<TimeSpan, Self::Error>;
    /// The group's memory.
    fn memory(&mut self, g: G) -> Result<&mut GroupMemory<K>, Self::Error>;
    /// `UnitInfo.Unit != null` (`UnitReference.Value`), as `Cleanup` reads it.
    fn resolves(&mut self, u: K) -> Result<bool, Self::Error>;
    /// `UnitInfo.Unit.IsInGame`, as `Cleanup` reads it.
    fn in_game(&mut self, u: K) -> Result<bool, Self::Error>;
    /// `unit.IsDisposed`.
    fn disposed(&mut self, u: K) -> Result<bool, Self::Error>;
    /// `observer.Wake()` (`AwakeTimer = Max(0, AwakeTimer)`).
    fn wake(&mut self, u: K) -> Result<(), Self::Error>;
    /// After a ready group's `Cleanup()` (nothing in the game: where a caller checks the group's
    /// memory), with the keys it removed in removal order.
    ///
    /// # Errors
    ///
    /// Whatever the caller's check reports.
    fn cleaned(&mut self, _g: G, _removed: &[K]) -> Result<(), Self::Error> {
        Ok(())
    }
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
            if w.faction_peaceful(u)? || w.passive(u)? {
                continue;
            }
            if w.group_of(u)? == g {
                w.memory(g)?.add(u, now).last_detect_time = now;
                continue;
            }
            update_visibility(w, g, u, now)?;
        }
        let removed = cleanup_in_tick(ctl, w, g, now)?;
        w.cleaned(g, &removed)?;
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
        // `observer.Group.IsFake ? GetGroup(observer.GroupId) : observer.Group` (`Group` read again
        // each time).
        let og = w.group_of(o)?;
        let g = if w.is_fake(og) { w.group_by_id(o)? } else { w.group_of(o)? };
        w.memory(g)?.add(t, now).last_detect_time = now;
        w.wake(o)?;
    }
    ctl.units_for_add.clear();
    Ok(())
}

/// The group's `Cleanup()` as the tick runs it, each entry's reads as the game makes them: its
/// unit (`value.Unit == null`), its detect time, then its unit again and `IsInGame`. The entries
/// collected go to `s_UnitsForRemove` (`ctl.cleanup_pending`), so a failure part way leaves them
/// there. Returns the keys removed, in removal order.
fn cleanup_in_tick<K, G, W>(
    ctl: &mut MemoryController<K>,
    w: &mut W,
    g: G,
    now: TimeSpan,
) -> Result<Vec<K>, W::Error>
where
    K: Copy + Eq + Hash,
    G: Copy + Eq,
    W: MemoryWorld<K, G>,
{
    let keys: Vec<K> = w.memory(g)?.keys().collect();
    for k in keys {
        let info = *w.memory(g)?.find(k).expect("a key of the walk");
        // `value.Unit == null || LastDetectTime + 9 s < GameTime || !value.Unit.IsInGame`, the
        // reference resolved again for the last test.
        let gone = !w.resolves(k)?
            || info.last_detect_time.checked_add(FORGET_AFTER).map_err(MemError::from)? < now
            || {
                if !w.resolves(k)? {
                    return Err(MemError::NullReference.into());
                }
                !w.in_game(k)?
            };
        if gone {
            ctl.cleanup_pending.push(k);
        }
    }
    Ok(w.memory(g)?.cleanup_finish(&mut ctl.cleanup_pending))
}

/// `UpdateVisibility(group, target)` for a near unit of another group.
fn update_visibility<K, G, W>(w: &mut W, g: G, t: K, now: TimeSpan) -> Result<(), W::Error>
where
    K: Copy + Eq + Hash,
    G: Copy + Eq,
    W: MemoryWorld<K, G>,
{
    if w.in_stealth_for(t, g)? {
        w.memory(g)?.remove(t);
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
        w.memory(g)?.add(t, now).visible = true;
        if w.invisible(t)? {
            let seen = w.sees_invisible(g, t)?;
            if let Some(info) = w.memory(g)?.find_mut(t) {
                info.visible = seen;
            }
        }
        if let Some(info) = w.memory(g)?.find_mut(t).filter(|i| i.visible) {
            info.last_detect_time = now;
        }
        return Ok(());
    }
    if let Some(info) = w.memory(g)?.find_mut(t) {
        info.visible = false;
    }
    Ok(())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod tick_tests;
