use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde_json::Value;

use super::*;
use crate::combat::oracle::Trace;

const T0: TimeSpan = TimeSpan(434_800_860_000);

fn secs(s: i64) -> TimeSpan {
    TimeSpan(s * crate::csharp::timespan::TICKS_PER_SECOND)
}

fn at(t: TimeSpan, d: TimeSpan) -> TimeSpan {
    TimeSpan(t.0 + d.0)
}

fn keys(m: &GroupMemory<u32>) -> Vec<u32> {
    m.keys().collect()
}

const IN: Resolved = Resolved::Unit { in_game: true };
const OUT: Resolved = Resolved::Unit { in_game: false };

#[test]
fn nine_seconds_is_from_seconds_of_nine() {
    assert_eq!(TimeSpan::from_seconds(f64::from(9.0_f32)), Ok(FORGET_AFTER));
}

#[test]
fn a_new_unit_takes_the_last_freed_slot_and_a_known_one_keeps_its_entry() {
    let mut m = GroupMemory::new();
    for k in [1, 2, 3, 4] {
        m.add(k, T0);
    }
    assert!(m.remove(2));
    assert!(m.remove(4));
    assert!(!m.remove(9));
    // 4's slot was freed last.
    m.add(5, T0);
    m.add(6, T0);
    m.add(7, T0);
    assert_eq!(keys(&m), [1, 6, 3, 5, 7]);
    // A known unit: same slot, same entry (its detect time isn't reset by `Add`).
    m.find_mut(3).unwrap().visible = true;
    let e = m.add(3, at(T0, secs(5)));
    assert_eq!(*e, UnitInfo { last_detect_time: T0, visible: true });
    assert_eq!(keys(&m), [1, 6, 3, 5, 7]);
    let n = m.add(8, at(T0, secs(5)));
    assert_eq!(*n, UnitInfo { last_detect_time: at(T0, secs(5)), visible: false });
    assert!(m.contains(8) && !m.contains_visible(8) && m.contains_visible(3) && !m.contains(2));
}

#[test]
fn add_invalidates_both_caches_even_for_a_known_unit() {
    let mut m = GroupMemory::from_entries([(1, UnitInfo { last_detect_time: T0, visible: false })]);
    assert_eq!(m.units_list_cache(), None);
    assert_eq!(m.units_list(|_| IN), [1]);
    assert_eq!(m.enemies(|_| IN, |_| true), [1]);
    assert_eq!(m.units_list_cache(), Some(&[1][..]));
    m.add(1, T0);
    assert_eq!((m.units_list_cache(), m.enemies_cache()), (None, None));
    // A `Remove` of an absent unit keeps them.
    m.units_list(|_| IN);
    m.enemies(|_| IN, |_| true);
    m.remove(7);
    assert!(m.units_list_cache().is_some() && m.enemies_cache().is_some());
    // `ClearEnemies` drops only the enemies' cache.
    m.clear_enemies();
    assert!(m.units_list_cache().is_some() && m.enemies_cache().is_none());
    m.clear();
    assert!(m.is_empty() && m.units_list_cache().is_none() && m.enemies_cache().is_none());
}

#[test]
fn the_lists_filter_units_in_the_game_and_enemies_in_order_and_stay_cached() {
    let mut m = GroupMemory::new();
    for k in [10, 11, 12, 13] {
        m.add(k, T0);
    }
    let res = |k: u32| match k {
        11 => Resolved::Missing,
        12 => OUT,
        _ => IN,
    };
    let mut asked = Vec::new();
    let e = m
        .enemies(res, |k| {
            asked.push(k);
            k == 13
        })
        .to_vec();
    assert_eq!(e, [13]);
    // `IsEnemy` is asked only of units that resolve and are in the game.
    assert_eq!(asked, [10, 13]);
    assert_eq!(m.units_list(res), [10, 13]);
    // Cached: a changed world isn't seen until the cache is invalidated.
    assert_eq!(m.enemies(|_| IN, |_| true), [13]);
    assert_eq!(m.units_list(|_| IN), [10, 13]);
}

#[test]
fn cleanup_removes_in_order_and_leaves_the_caches_stale() {
    let mut m = GroupMemory::new();
    m.add(1, T0); // detected at T0
    m.add(2, at(T0, secs(1)));
    m.add(3, T0);
    m.add(4, T0);
    m.add(5, T0);
    assert_eq!(m.units_list(|_| IN), [1, 2, 3, 4, 5]);
    // Exactly 9 s later: not yet forgotten (strictly older than 9 s).
    let now = at(T0, FORGET_AFTER);
    let gone =
        m.cleanup(now, |k| if k == 4 { Resolved::Missing } else { IN }, &mut Vec::new()).unwrap();
    assert_eq!(gone, [4]);
    let now = TimeSpan(now.0 + 1);
    let gone = m.cleanup(now, |k| if k == 5 { OUT } else { IN }, &mut Vec::new()).unwrap();
    // 1 and 3 expired, 5 left the game: removed in enumeration order.
    assert_eq!(gone, [1, 3, 5]);
    assert_eq!(keys(&m), [2]);
    // The cached list still holds the removed units (Cleanup doesn't invalidate it).
    assert_eq!(m.units_list_cache(), Some(&[1, 2, 3, 4, 5][..]));
    // 5's slot was freed last, then 3's, then 1's, then 4's.
    for k in [6, 7, 8, 9, 10] {
        m.add(k, now);
    }
    assert_eq!(keys(&m), [8, 2, 7, 9, 6, 10]);
    // Overflow is the game's exception.
    let mut big = GroupMemory::from_entries([(
        1,
        UnitInfo { last_detect_time: TimeSpan::MAX, visible: false },
    )]);
    assert_eq!(big.cleanup(T0, |_| IN, &mut Vec::new()), Err(MemError::Cs(CsError::Overflow)));
}

#[test]
fn has_player_character_reads_the_cached_list() {
    let mut m = GroupMemory::new();
    m.add(1, T0);
    m.add(2, T0);
    assert_eq!(m.has_player_character_in_memory(|_| IN, |k| k == 2), Ok(true));
    assert_eq!(m.has_player_character_in_memory(|_| IN, |_| false), Ok(false));
    // A cached unit that no longer resolves throws.
    assert_eq!(
        m.has_player_character_in_memory(|_| Resolved::Missing, |_| false),
        Err(MemError::NullReference)
    );
}

#[test]
fn additions_are_queued_once_and_never_of_peaceful_targets() {
    let mut c = MemoryController::default();
    c.add_to_memory(1, 2, false);
    c.add_to_memory(1, 2, false);
    c.add_to_memory(2, 1, false);
    c.add_to_memory(1, 3, true);
    assert_eq!(c.units_for_add, [(1, 2), (2, 1)]);
    c.clear_queue();
    assert!(c.units_for_add.is_empty());
}

#[test]
fn an_enemy_command_on_the_party_and_offensive_actions_queue_both_ways() {
    let base = ActedCommand {
        executor: 1,
        target: Some(2),
        executor_players_enemy: true,
        target_player_faction: true,
        target_invisible: false,
        executor_peaceful: false,
        target_peaceful: false,
    };
    let mut c = MemoryController::default();
    c.handle_unit_command_did_act(&base);
    assert_eq!(c.units_for_add, [(1, 2), (2, 1)]);
    let mut c = MemoryController::default();
    c.handle_unit_command_did_act(&ActedCommand { target_invisible: true, ..base });
    assert_eq!(c.units_for_add, [(2, 1)]);
    for no in [
        ActedCommand { target: None, ..base },
        ActedCommand { executor_players_enemy: false, ..base },
        ActedCommand { target_player_faction: false, ..base },
    ] {
        let mut c = MemoryController::default();
        c.handle_unit_command_did_act(&no);
        assert!(c.units_for_add.is_empty());
    }
    let a = OffensiveAction {
        unit: 1,
        target: 2,
        both_in_game: true,
        target_invisible: false,
        unit_sees_invisibility: false,
        unit_blindsense_reaches: false,
        unit_peaceful: false,
        target_peaceful: false,
    };
    let cases = [
        (a, vec![(1, 2), (2, 1)]),
        (OffensiveAction { both_in_game: false, ..a }, vec![]),
        (OffensiveAction { target_invisible: true, ..a }, vec![(2, 1)]),
        (
            OffensiveAction { target_invisible: true, unit_sees_invisibility: true, ..a },
            vec![(1, 2), (2, 1)],
        ),
        (
            OffensiveAction { target_invisible: true, unit_blindsense_reaches: true, ..a },
            vec![(1, 2), (2, 1)],
        ),
        (OffensiveAction { unit_peaceful: true, ..a }, vec![(1, 2)]),
    ];
    for (a, want) in cases {
        let mut c = MemoryController::default();
        c.handle_offensive_action(&a);
        assert_eq!(c.units_for_add, want, "{a:?}");
    }
}

/// A world for the tick: what each read returns, and a log of the reads that matter.
#[derive(Default)]
struct Fake {
    peaceful: bool,
    ready: Vec<u8>,
    near: HashMap<u8, Vec<u32>>,
    /// Units of a peaceful faction, passive units.
    peaceful_units: HashSet<u32>,
    passive: HashSet<u32>,
    passive_reads: Vec<u32>,
    group: HashMap<u32, u8>,
    /// Fake groups, and the real group each unit's `GroupId` names.
    fake: HashSet<u8>,
    by_id: HashMap<u32, u8>,
    stealth: HashSet<(u32, u8)>,
    enemies: HashSet<(u8, u32)>,
    delta: f32,
    frame: i32,
    hash: HashMap<u32, i32>,
    los: HashSet<(u8, u32)>,
    invisible: HashSet<u32>,
    sees_invisible: HashSet<(u8, u32)>,
    now: TimeSpan,
    mem: HashMap<u8, GroupMemory<u32>>,
    resolved: HashMap<u32, Resolved>,
    disposed: HashSet<u32>,
    woken: Vec<u32>,
    log: Vec<String>,
    /// Reads that fail (the game's exception): the invisibility loop, a unit's `Wake`.
    fail_sees_invisible: bool,
    fail_wake: Option<u32>,
    fail_resolve: Option<u32>,
}

impl MemoryWorld<u32, u8> for Fake {
    type Error = MemError;
    fn area_peaceful(&mut self) -> Result<bool, MemError> {
        Ok(self.peaceful)
    }
    fn ready_groups(&mut self) -> Result<Vec<u8>, MemError> {
        Ok(self.ready.clone())
    }
    fn near_units(&mut self, g: u8) -> Result<Vec<u32>, MemError> {
        Ok(self.near.get(&g).cloned().unwrap_or_default())
    }
    fn faction_peaceful(&mut self, u: u32) -> Result<bool, MemError> {
        Ok(self.peaceful_units.contains(&u))
    }
    fn passive(&mut self, u: u32) -> Result<bool, MemError> {
        self.passive_reads.push(u);
        Ok(self.passive.contains(&u))
    }
    fn is_fake(&mut self, g: u8) -> bool {
        self.fake.contains(&g)
    }
    fn group_by_id(&mut self, u: u32) -> Result<u8, MemError> {
        Ok(self.by_id[&u])
    }
    fn group_of(&mut self, u: u32) -> Result<u8, MemError> {
        Ok(self.group[&u])
    }
    fn in_stealth_for(&mut self, u: u32, g: u8) -> Result<bool, MemError> {
        Ok(self.stealth.contains(&(u, g)))
    }
    fn is_enemy(&mut self, g: u8, u: u32) -> Result<bool, MemError> {
        Ok(self.enemies.contains(&(g, u)))
    }
    fn game_delta_time(&mut self) -> Result<f32, MemError> {
        Ok(self.delta)
    }
    fn frame_count(&mut self) -> Result<i32, MemError> {
        Ok(self.frame)
    }
    fn unit_hash(&mut self, u: u32) -> Result<i32, MemError> {
        self.log.push(format!("hash {u}"));
        Ok(self.hash[&u])
    }
    fn group_has_los(&mut self, g: u8, u: u32) -> Result<bool, MemError> {
        self.log.push(format!("los {g} {u}"));
        Ok(self.los.contains(&(g, u)))
    }
    fn invisible(&mut self, u: u32) -> Result<bool, MemError> {
        Ok(self.invisible.contains(&u))
    }
    fn sees_invisible(&mut self, g: u8, u: u32) -> Result<bool, MemError> {
        if self.fail_sees_invisible {
            return Err(MemError::NullReference);
        }
        Ok(self.sees_invisible.contains(&(g, u)))
    }
    fn game_time(&mut self) -> Result<TimeSpan, MemError> {
        Ok(self.now)
    }
    fn memory(&mut self, g: u8) -> Result<&mut GroupMemory<u32>, MemError> {
        Ok(self.mem.entry(g).or_default())
    }
    fn resolves(&mut self, u: u32) -> Result<bool, MemError> {
        if self.fail_resolve == Some(u) {
            return Err(MemError::NullReference);
        }
        Ok(self.resolved.get(&u).copied().unwrap_or(IN) != Resolved::Missing)
    }
    fn in_game(&mut self, u: u32) -> Result<bool, MemError> {
        Ok(self.resolved.get(&u).copied().unwrap_or(IN).in_game())
    }
    fn disposed(&mut self, u: u32) -> Result<bool, MemError> {
        Ok(self.disposed.contains(&u))
    }
    fn wake(&mut self, u: u32) -> Result<(), MemError> {
        if self.fail_wake == Some(u) {
            return Err(MemError::NullReference);
        }
        self.woken.push(u);
        Ok(())
    }
}

/// Group 1 (units 10, 11) sees unit 20 of group 2 (an enemy) and unit 30 of group 3 (a friend).
fn fight() -> Fake {
    let mut f = Fake { ready: vec![1], delta: 1.0 / 60.0, frame: 9415, now: T0, ..Fake::default() };
    f.near.insert(1, vec![10, 20, 11, 30]);
    f.group.extend([(10, 1), (11, 1), (20, 2), (30, 3)]);
    f.enemies.insert((1, 20));
    // poc5-tb-103's 5198: (392888320 >>> 2) % 5 == 0.
    f.hash.insert(30, 392_888_320);
    f.los.extend([(1, 20), (1, 30)]);
    f
}

#[test]
fn a_tick_refreshes_the_group_and_remembers_what_it_sees() {
    let mut f = fight();
    let mut c = MemoryController::default();
    tick(&mut c, &mut f).unwrap();
    let m = &f.mem[&1];
    // Near-unit order; the group's own units aren't visible entries.
    assert_eq!(keys(m), [10, 20, 11, 30]);
    assert!(!m.contains_visible(10) && m.contains_visible(20) && m.contains_visible(30));
    assert_eq!(f.log, ["los 1 20", "hash 30", "los 1 30"]);
    // A friend is looked at only on its frame (9416 % 5 != 0), an enemy every frame.
    f.log.clear();
    f.frame = 9416;
    f.los.clear();
    f.now = at(T0, secs(1));
    tick(&mut c, &mut f).unwrap();
    assert_eq!(f.log, ["los 1 20", "hash 30"]);
    let m = &f.mem[&1];
    assert!(!m.contains_visible(20) && m.contains_visible(30));
    // Out of sight keeps its detect time; the group's own are refreshed.
    assert_eq!(m.find(20).unwrap().last_detect_time, T0);
    assert_eq!(m.find(10).unwrap().last_detect_time, at(T0, secs(1)));
    // A long frame looks at friends every frame.
    f.log.clear();
    f.delta = 0.5;
    tick(&mut c, &mut f).unwrap();
    assert_eq!(f.log, ["los 1 20", "los 1 30"]);
    assert!(!f.mem[&1].contains_visible(30));
}

#[test]
fn a_tick_forgets_after_nine_seconds_and_drops_stealthed_and_ignored_units() {
    let mut f = fight();
    let mut c = MemoryController::default();
    tick(&mut c, &mut f).unwrap();
    // Nothing seen for more than 9 s: 20 (out of sight) and 30 (a friend, not looked at on this
    // frame) are forgotten at the group's cleanup; the group's own units stay.
    f.los.clear();
    f.now = TimeSpan(at(T0, FORGET_AFTER).0 + 1);
    f.frame += 1;
    tick(&mut c, &mut f).unwrap();
    assert_eq!(keys(&f.mem[&1]), [10, 11]);
    // Exactly 9 s after the last detection, a unit is kept.
    f.los.insert((1, 30));
    f.frame -= 1;
    tick(&mut c, &mut f).unwrap();
    f.los.clear();
    f.now = at(f.now, FORGET_AFTER);
    f.frame += 1;
    tick(&mut c, &mut f).unwrap();
    assert_eq!(keys(&f.mem[&1]), [10, 11, 30]);
    f.los.clear();
    // A stealthed unit is removed at once; an ignored (peaceful or passive) one isn't looked at.
    f.los.insert((1, 20));
    tick(&mut c, &mut f).unwrap();
    assert!(f.mem[&1].contains(20));
    f.stealth.insert((20, 1));
    f.passive.insert(11);
    let before = f.mem[&1].find(11).copied();
    f.now = at(f.now, secs(1));
    tick(&mut c, &mut f).unwrap();
    assert!(!f.mem[&1].contains(20));
    assert_eq!(f.mem[&1].find(11).copied(), before);
    // A peaceful area does nothing.
    f.peaceful = true;
    c.add_to_memory(10, 99, false);
    tick(&mut c, &mut f).unwrap();
    assert_eq!(c.units_for_add, [(10, 99)]);
}

#[test]
fn an_invisible_unit_is_remembered_but_not_visible_unless_someone_sees_it() {
    let mut f = fight();
    f.invisible.insert(20);
    let mut c = MemoryController::default();
    tick(&mut c, &mut f).unwrap();
    assert!(f.mem[&1].contains(20) && !f.mem[&1].contains_visible(20));
    f.now = at(T0, secs(2));
    f.frame += 1;
    tick(&mut c, &mut f).unwrap();
    // Not seen: its detect time isn't refreshed.
    assert_eq!(f.mem[&1].find(20).unwrap().last_detect_time, T0);
    f.sees_invisible.insert((1, 20));
    tick(&mut c, &mut f).unwrap();
    assert_eq!(
        f.mem[&1].find(20).copied(),
        Some(UnitInfo { last_detect_time: f.now, visible: true })
    );
}

#[test]
fn queued_additions_go_to_the_observer_group_and_wake_it() {
    let mut f = fight();
    f.ready.clear();
    f.group.extend([(40, 4), (41, 5), (42, 5)]);
    // 40's group is a fake one: the addition goes to the real one its GroupId names.
    f.group.insert(40, 7);
    f.fake.insert(7);
    f.by_id.insert(40, 6);
    f.disposed.insert(42);
    f.stealth.insert((20, 5));
    let mut c = MemoryController::default();
    c.add_to_memory(40, 20, false);
    c.add_to_memory(41, 42, false);
    c.add_to_memory(41, 20, false);
    c.add_to_memory(41, 10, false);
    tick(&mut c, &mut f).unwrap();
    assert!(c.units_for_add.is_empty());
    assert_eq!(keys(&f.mem[&6]), [20]);
    assert_eq!(keys(&f.mem[&5]), [10]);
    assert!(!f.mem.contains_key(&4));
    assert_eq!(f.woken, [40, 41]);
    assert_eq!(
        f.mem[&5].find(10).copied(),
        Some(UnitInfo { last_detect_time: T0, visible: false })
    );
}

// ---- the recordings ----

fn captures() -> Vec<PathBuf> {
    let labels = std::env::var("WR_MEMORY_CAPTURES").unwrap_or_else(|_| "poc5-tb-103".into());
    let mut out = Vec::new();
    for label in labels.split(',') {
        let root =
            wr_paths::repo_root().join("fixtures/captures/oracle").join(label).join("fixtures");
        for fx in ["tb-nearest", "tb-focus"] {
            for run in ["record", "replay-1", "replay-2"] {
                out.push(root.join(fx).join(format!("{run}.trace.jsonl.gz")));
            }
        }
    }
    out
}

/// A group's faction caches as `leave_tick` records them (`UnitGroup.IsEnemy`'s inputs).
#[derive(Debug, Clone, Default)]
struct Factions {
    /// `m_Factions`' keys (with a count above zero).
    own: Vec<String>,
    /// `m_AttackFactions`.
    attack: Vec<String>,
    /// `m_IsEnemyForEveryone`.
    everyone: bool,
}

/// The recordings' state the check needs besides the memory.
struct Rec<'a> {
    content: &'a crate::content::Content,
    ids: HashMap<String, u32>,
    names: Vec<String>,
    mem: HashMap<String, GroupMemory<u32>>,
    /// The dictionary each group's last `leave_tick` row (or `group_state`) showed.
    shown: HashMap<String, Vec<u32>>,
    factions: HashMap<String, Factions>,
    /// Each unit's real group (`m_Group`), as `group_state` and the group records show it.
    group: HashMap<u32, String>,
    /// Units that left their group (`UnitGroup.Remove` of a member) and weren't put back: taken
    /// as out of the game. The recording doesn't say why a unit left (`OnIsInGameChanged`, a
    /// disposal, the `GroupId` setter), so reads that depend on it are counted apart; a unit still
    /// in the game is put back by the next `UnitGroupsController.Tick`, before any `Cleanup`.
    left: HashSet<u32>,
    always_enemy: HashMap<String, bool>,
    /// Units' identity hashes (WrathOracle's `hash`, recorded on first use in the record run; a
    /// replay uses the record's), the frame's `Time.frameCount` and `GameDeltaTime`.
    hashes: HashMap<String, i64>,
    /// A replay's pinned `Time.frameCount` is its own plus `((record's first frame - its first
    /// frame) mod 60)` (WrathOracle's `WallClock.Frame`).
    frame_shift: i64,
    frame: i64,
    /// The frame's `GameDeltaTime` (its `time` record), when one was seen.
    game_delta: Option<f64>,
    errors: Vec<String>,
    counts: HashMap<&'static str, usize>,
}

impl Rec<'_> {
    fn id(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.ids.get(s) {
            return i;
        }
        let i = u32::try_from(self.names.len()).unwrap();
        self.ids.insert(s.to_owned(), i);
        self.names.push(s.to_owned());
        i
    }

    fn unit_of(&mut self, v: &Value) -> Option<u32> {
        let s = v.as_str().or_else(|| v["$unit"].as_str())?;
        Some(self.id(s))
    }

    fn always_enemy(&mut self, f: &str) -> bool {
        if let Some(&b) = self.always_enemy.get(f) {
            return b;
        }
        let g = wr_content::Guid::parse(f).unwrap_or_else(|| panic!("faction guid {f}"));
        let bp = self.content.get(g).unwrap_or_else(|| panic!("faction blueprint {f}"));
        let b = bp.view().bool("AlwaysEnemy");
        self.always_enemy.insert(f.to_owned(), b);
        b
    }

    /// `UnitGroup.IsEnemy(g1, g2)` (static, one direction) on the recorded caches.
    fn enemy_dir(&mut self, g1: &str, g2: &str) -> bool {
        if g1 == g2 {
            return false;
        }
        let (f1, f2) = (self.factions[g1].clone(), self.factions[g2].clone());
        if f1.everyone || f2.everyone {
            return true;
        }
        if g2 == crate::combat::units::PLAYER_PARTY_GROUP {
            return false;
        }
        f2.attack.iter().any(|a| self.always_enemy(a) || f1.own.contains(a))
    }

    fn is_enemy(&mut self, g: &str, u: u32) -> Option<bool> {
        let gu = self.group.get(&u)?.clone();
        Some(self.enemy_dir(g, &gu) || self.enemy_dir(&gu, g))
    }

    fn resolve(&self, u: u32) -> Resolved {
        Resolved::Unit { in_game: !self.left.contains(&u) }
    }

    fn fail(&mut self, t: i64, what: String) {
        self.errors.push(format!("t={t}: {what}"));
    }

    fn names(&self, v: &[u32]) -> Vec<&str> {
        v.iter().map(|&i| self.names[i as usize].as_str()).collect()
    }
}

/// The `leave_tick` row's fields: combat count, timer, `dict=`, `list=`, `f=`, `af=`, `efe=`.
fn row_field<'a>(row: &'a str, key: &str) -> Option<&'a str> {
    row.split('|').find_map(|p| p.strip_prefix(key))
}

fn split_ids(s: &str) -> Vec<&str> {
    if s.is_empty() { Vec::new() } else { s.split(',').collect() }
}

#[test]
#[ignore = "needs the poc5-tb-103 captures and the game's blueprints"]
fn recorded_memory_changes_reproduce_every_memory_read() {
    let content = crate::content::Content::open_default().expect("the game's blueprints");
    let mut totals: HashMap<&'static str, usize> = HashMap::new();
    let mut failures = Vec::new();
    for path in captures() {
        let trace = Trace::load(&path).unwrap_or_else(|e| panic!("{}: {e:#}", path.display()));
        // A replay draws the identity hashes from the record run's tape (WrathOracle's `HashTape`).
        let record = path.with_file_name("record.trace.jsonl.gz");
        let (hashes, frame_shift) = if record == path {
            (hashes_of(&trace.records), 0)
        } else {
            let rec = Trace::load(&record).expect("the record run").records;
            let first = |v: &[Value]| {
                v.iter().find(|r| r["k"] == "begin").and_then(|b| b["frame"].as_i64())
            };
            let shift = (first(&rec).expect("record frame")
                - first(&trace.records).expect("frame"))
            .rem_euclid(60);
            (hashes_of(&rec), shift)
        };
        let mut r = checked(&content, &trace.records, hashes, frame_shift);
        require_coverage(&mut r);
        for (k, v) in &r.counts {
            *totals.entry(k).or_default() += v;
        }
        let name = path.strip_prefix(wr_paths::repo_root()).unwrap_or(&path).display().to_string();
        eprintln!("{name}: {:?}", {
            let mut c: Vec<_> = r.counts.iter().collect();
            c.sort();
            c
        });
        failures.extend(r.errors.iter().take(20).map(|e| format!("{name}: {e}")));
    }
    let mut t: Vec<_> = totals.iter().collect();
    t.sort();
    eprintln!("total: {t:?}");
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
}

/// What every run of `poc5-tb-103`'s fights exercises: a run that checks none of one kind fails.
const COVERAGE: [&str; 10] = [
    "cleared",
    "enemies read: engagement",
    "enemies read: AI",
    "enemies read: command",
    "target remembers initiator",
    "dictionary rows",
    "units lists",
    "added: visibility",
    "friend phase compared",
    "forgotten: left its group",
];

fn require_coverage(r: &mut Rec<'_>) {
    for k in COVERAGE {
        if r.counts.get(k).copied().unwrap_or(0) == 0 {
            r.fail(-1, format!("nothing checked: {k}"));
        }
    }
}

fn hashes_of(recs: &[Value]) -> HashMap<String, i64> {
    recs.iter()
        .filter(|r| r["k"] == "obs" && r["ev"] == "hash")
        .filter_map(|r| Some((r["u"].as_str()?.to_owned(), r["h"].as_i64()?)))
        .collect()
}

fn checked<'a>(
    content: &'a crate::content::Content,
    recs: &[Value],
    hashes: HashMap<String, i64>,
    frame_shift: i64,
) -> Rec<'a> {
    let mut r = Rec {
        content,
        hashes,
        frame_shift,
        frame: -1,
        game_delta: None,
        ids: HashMap::new(),
        names: Vec::new(),
        mem: HashMap::new(),
        shown: HashMap::new(),
        factions: HashMap::new(),
        group: HashMap::new(),
        left: HashSet::new(),
        always_enemy: HashMap::new(),
        errors: Vec::new(),
        counts: HashMap::new(),
    };
    check_trace(&mut r, recs);
    r
}

#[test]
#[ignore = "needs the poc5-tb-103 captures and the game's blueprints"]
fn damaged_memory_records_are_caught() {
    let content = crate::content::Content::open_default().expect("the game's blueprints");
    let path = wr_paths::repo_root()
        .join("fixtures/captures/oracle/poc5-tb-103/fixtures/tb-focus/record.trace.jsonl.gz");
    let recs = Trace::load(&path).expect("capture").records;
    let errors = |v: &[Value]| {
        let mut r = checked(&content, v, hashes_of(v), 0);
        require_coverage(&mut r);
        r.errors
    };
    assert_eq!(errors(&recs), Vec::<String>::new());
    let pos = |f: &dyn Fn(&Value) -> bool| recs.iter().position(f).expect("record");
    let ev = |r: &Value, e: &str| r["k"] == "obs" && r["ev"] == e;
    type Damage = Box<dyn Fn(&mut Vec<Value>)>;
    let first_add = pos(&|r| ev(r, "mem_add"));
    let pair = pos(&|r| {
        ev(r, "uc_v") && r["f"] == "enemies" && r["v"].as_array().is_some_and(|a| a.len() == 2)
    });
    let leaving = pos(&|r| ev(r, "group_remove"));
    let env = pos(&|r| r["k"] == "env" && r["target_remembers_initiator"].is_boolean());
    let clear = pos(&|r| ev(r, "mem_clear"));
    let row =
        pos(&|r| ev(r, "leave_tick") && r["groups"].as_array().is_some_and(|g| !g.is_empty()));
    let hash = pos(&|r| ev(r, "hash") && r["u"] == "558A");
    let without = |f: fn(&Value) -> bool| -> Damage { Box::new(move |v| v.retain(|r| !f(r))) };
    let cases: Vec<(&str, Damage)> = vec![
        ("no flat-footed reads", without(|r| r["k"] == "env")),
        ("no time records", without(|r| r["k"] == "time")),
        ("no AI entries", without(|r| r["k"] == "obs" && r["ev"] == "uc_ai")),
        ("no Cleanup removals", without(|r| r["k"] == "obs" && r["ev"] == "mem_remove")),
        ("no clears", without(|r| r["k"] == "obs" && r["ev"] == "mem_clear")),
        (
            "only the first time record",
            Box::new(|v| {
                let first = v.iter().position(|r| r["k"] == "time").unwrap();
                let mut i = 0;
                v.retain(|r| {
                    i += 1;
                    r["k"] != "time" || i - 1 == first
                });
            }),
        ),
        ("an addition missing", Box::new(move |v| drop(v.remove(first_add)))),
        (
            "an addition twice",
            Box::new(move |v| {
                let r = v[first_add].clone();
                v.insert(first_add, r);
            }),
        ),
        (
            "enemies in another order",
            Box::new(move |v| {
                v[pair]["v"].as_array_mut().unwrap().reverse();
            }),
        ),
        ("a unit that never left the game forgotten", Box::new(move |v| drop(v.remove(leaving)))),
        (
            "a flat-footed memory read flipped",
            Box::new(move |v| {
                let b = v[env]["target_remembers_initiator"].as_bool().unwrap();
                v[env]["target_remembers_initiator"] = Value::from(!b);
            }),
        ),
        ("a clear of another size", Box::new(move |v| v[clear]["n"] = Value::from(99))),
        (
            "a friend's hash on another frame",
            Box::new(move |v| {
                let h = v[hash]["h"].as_i64().unwrap();
                v[hash]["h"] = Value::from(h + 4);
            }),
        ),
        (
            "a row's dictionary reordered",
            Box::new(move |v| {
                for g in v[row]["groups"].as_array_mut().unwrap() {
                    let s = g[1].as_str().unwrap().to_owned();
                    if let Some(d) = row_field(&s, "dict=").filter(|d| d.contains(',')) {
                        let mut ids = split_ids(d);
                        ids.reverse();
                        g[1] = Value::from(s.replacen(
                            &format!("dict={d}"),
                            &format!("dict={}", ids.join(",")),
                            1,
                        ));
                        return;
                    }
                }
                panic!("no row with two remembered units");
            }),
        ),
    ];
    for (what, damage) in cases {
        let mut v = recs.clone();
        damage(&mut v);
        assert!(!errors(&v).is_empty(), "{what}: not caught");
    }
}

/// Who reads `Memory.Enemies` (the frame a leaf belongs to).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reader {
    Engage,
    Ai,
    Command,
}

#[allow(clippy::too_many_lines)]
fn check_trace(r: &mut Rec<'_>, recs: &[Value]) {
    let mut started = false;
    // The readers' frames: (who, the unit whose group memory a read is of).
    let mut frames: Vec<(Reader, Option<u32>)> = Vec::new();
    // A `Cleanup`'s removals of one group, applied together in enumeration order.
    let mut cleanup: Option<(String, Vec<u32>, i64)> = None;
    for x in recs {
        let t = x["t"].as_i64().unwrap_or(-1);
        let ev = x["ev"].as_str().unwrap_or("");
        let is_obs = x["k"] == "obs";
        let continues_cleanup = is_obs && ev == "mem_remove" && x["by"] == "Cleanup";
        let other_group =
            cleanup.as_ref().is_some_and(|c| c.0 != x["group"].as_str().unwrap_or(""));
        if (!continues_cleanup || other_group)
            && let Some((g, gone, ct)) = cleanup.take()
        {
            apply_cleanup(r, &g, &gone, ct);
        }
        if x["k"] == "env" && started {
            if let (Some(tg), Some(ini), Some(want)) = (
                r.unit_of(&x["target"]),
                r.unit_of(&x["initiator"]),
                x["target_remembers_initiator"].as_bool(),
            ) {
                *r.counts.entry("target remembers initiator").or_default() += 1;
                let Some(g) = r.group.get(&tg).cloned() else {
                    r.fail(t, format!("env: target {} has no group", r.names[tg as usize]));
                    continue;
                };
                let got = r.mem.get(&g).is_some_and(|m| m.contains(ini));
                if got != want {
                    r.fail(
                        t,
                        format!(
                            "env: {g} remembers {}: model {got}, game {want}",
                            r.names[ini as usize]
                        ),
                    );
                }
            }
            continue;
        }
        if x["k"] == "tick" {
            r.frame = x["frame"].as_i64().map_or(-1, |f| f + r.frame_shift);
            // `GameDeltaTime` is this frame's `time` record's, or unknown.
            r.game_delta = None;
        }
        if x["k"] == "time" {
            r.game_delta = x["game_delta"].as_f64();
        }
        if !is_obs {
            continue;
        }
        match ev {
            "group_state" => {
                started = true;
                for g in x["groups"].as_array().into_iter().flatten() {
                    let id = g["id"].as_str().unwrap_or("").to_owned();
                    let keys: Vec<u32> = g["memory"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|k| k.as_str())
                        .map(|k| r.id(k))
                        .collect::<Vec<_>>();
                    // The window's first layout: the keys in order, no freed slots between them
                    // (not established by the recording: an addition into a freed slot would
                    // show as a dictionary that differs from the next row).
                    let info = UnitInfo { last_detect_time: TimeSpan::ZERO, visible: false };
                    r.mem.insert(
                        id.clone(),
                        GroupMemory::from_entries(keys.iter().map(|&k| (k, info))),
                    );
                    r.shown.insert(id, keys);
                }
                for u in x["units"].as_array().into_iter().flatten() {
                    let (Some(uid), Some(g)) = (u[0].as_str(), u[1].as_str()) else { continue };
                    if !g.starts_with('~') {
                        let i = r.id(uid);
                        r.group.insert(i, g.to_owned());
                    }
                }
            }
            _ if !started => {}
            "group_new" => {
                let g = x["group"].as_str().unwrap_or("").to_owned();
                r.mem.insert(g, GroupMemory::new());
            }
            "group_add" => {
                let (Some(g), Some(u)) = (x["group"].as_str(), x["u"].as_str()) else { continue };
                let (g, u) = (g.to_owned(), r.id(u));
                r.group.insert(u, g);
                r.left.remove(&u);
            }
            "group_remove" => {
                // A removal of a unit the group didn't hold changes nothing.
                let (Some(u), Some(true)) = (x["u"].as_str(), x["had"].as_bool()) else { continue };
                let u = r.id(u);
                r.group.remove(&u);
                r.left.insert(u);
            }
            "group_dispose" => {
                // `UnitGroup.Dispose()` clears the memory (its `mem_clear` follows when it held
                // units), and the group leaves `UnitGroupsController.Groups`: no more rows.
                if let Some(g) = x["group"].as_str() {
                    r.shown.remove(g);
                }
                *r.counts.entry("group disposed").or_default() += 1;
            }
            "mem_add" => {
                let (Some(g), Some(u)) = (x["group"].as_str(), x["u"].as_str()) else { continue };
                let (g, u) = (g.to_owned(), r.id(u));
                let by = x["by"].as_str().unwrap_or("");
                *r.counts
                    .entry(if by.contains("UpdateVisibility") {
                        "added: visibility"
                    } else {
                        "added: other"
                    })
                    .or_default() += 1;
                let m = r.mem.entry(g.clone()).or_default();
                if m.contains(u) {
                    r.fail(
                        t,
                        format!("mem_add of {} already in {g}'s memory", r.names[u as usize]),
                    );
                    continue;
                }
                m.add(u, TimeSpan::ZERO);
                if by.contains("UpdateVisibility") {
                    check_friend_phase(r, &g, u, t);
                }
            }
            "mem_remove" if x["by"] == "Cleanup" => {
                let (Some(g), Some(u)) = (x["group"].as_str(), x["u"].as_str()) else { continue };
                let u = r.id(u);
                match &mut cleanup {
                    Some(c) => c.1.push(u),
                    None => cleanup = Some((g.to_owned(), vec![u], t)),
                }
            }
            "mem_remove" => {
                let (Some(g), Some(u)) = (x["group"].as_str(), x["u"].as_str()) else { continue };
                let (g, u) = (g.to_owned(), r.id(u));
                *r.counts.entry("removed").or_default() += 1;
                if !r.mem.entry(g.clone()).or_default().remove(u) {
                    r.fail(t, format!("mem_remove of {} not in {g}'s memory", r.names[u as usize]));
                }
            }
            "mem_clear" => {
                let Some(g) = x["group"].as_str() else { continue };
                let m = r.mem.entry(g.to_owned()).or_default();
                let n = usize::try_from(x["n"].as_u64().unwrap_or(0)).unwrap_or(0);
                if m.len() != n {
                    let len = m.len();
                    r.fail(t, format!("mem_clear of {g}: {n} recorded, the model holds {len}"));
                }
                r.mem.entry(g.to_owned()).or_default().clear();
                *r.counts.entry("cleared").or_default() += 1;
            }
            "leave_tick" => check_rows(r, x, t),
            "uc_ai" => frames.push((Reader::Ai, None)),
            "uc_eng" => frames.push((Reader::Engage, None)),
            "uc_tick" => frames.push((Reader::Command, r.unit_of(&x["u"]))),
            "uc_ai_end" | "uc_eng_end" | "uc_tick_end" => {
                frames.pop();
            }
            "eng_tu" => {
                let u = r.unit_of(&x["u"]);
                if let Some(f) = frames.last_mut().filter(|f| f.0 == Reader::Engage) {
                    f.1 = u;
                }
            }
            "uc_v" => {
                let f = x["f"].as_str().unwrap_or("");
                if matches!(f, "ctrl" | "sleep" | "nct" | "conf" | "aien" | "pe") {
                    let u = r.unit_of(&x["u"]);
                    if let Some(fr) = frames.last_mut().filter(|fr| fr.0 == Reader::Ai)
                        && u.is_some()
                    {
                        fr.1 = u;
                    }
                }
                if f == "enemies" {
                    check_enemies(r, x, t, frames.last().copied());
                }
            }
            _ => {}
        }
    }
    if let Some((g, gone, ct)) = cleanup.take() {
        apply_cleanup(r, &g, &gone, ct);
    }
}

/// `UpdateVisibility` looks at a unit that isn't the group's enemy only on its frame (`(uint)(hash >>>
/// 2) % 5 == Time.frameCount % 5`) unless the frame is long (`GameDeltaTime >= 0.5`): an addition of
/// such a unit must fall on its frame.
fn check_friend_phase(r: &mut Rec<'_>, g: &str, u: u32, t: i64) {
    match r.is_enemy(g, u) {
        Some(true) => return,
        Some(false) => {}
        None => {
            r.fail(t, format!("{} has no group to tell an enemy by", r.names[u as usize]));
            return;
        }
    }
    let Some(delta) = r.game_delta else {
        r.fail(t, "a friend's addition on a frame without its time record".to_owned());
        return;
    };
    #[allow(clippy::cast_possible_truncation)]
    let delta = delta as f32;
    if delta.is_nan() {
        r.fail(t, "a friend's addition on a frame whose GameDeltaTime is NaN".to_owned());
        return;
    }
    if delta >= 0.5 {
        *r.counts.entry("friend added on a long frame").or_default() += 1;
        return;
    }
    let name = r.names[u as usize].clone();
    let Some(&h) = r.hashes.get(&name) else {
        r.fail(t, format!("a friend's addition ({name}) without its recorded hash"));
        return;
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let phase = i64::from(((h as i32) as u32 >> 2) % 5);
    *r.counts.entry("friend phase compared").or_default() += 1;
    if r.frame % 5 != phase {
        let f = r.frame;
        r.fail(
            t,
            format!("{g} added the friend {name} on frame {f}, not its frame (phase {phase})"),
        );
    }
}

/// A `Cleanup`'s recorded removals (sorted by id) applied as `Cleanup` makes them: in the
/// dictionary's enumeration order. Each must be explained by what the recording establishes (a unit
/// out of the game); the 9 s horizon can't be checked (detect times aren't recorded).
fn apply_cleanup(r: &mut Rec<'_>, g: &str, gone: &[u32], t: i64) {
    let mut m = r.mem.remove(g).unwrap_or_default();
    let order: Vec<u32> = m.keys().filter(|k| gone.contains(k)).collect();
    if order.len() != gone.len() {
        r.fail(t, format!("Cleanup of {g} removed {:?}, not all in the model", r.names(gone)));
    }
    let removed = m.cleanup(
        TimeSpan::ZERO,
        |k| Resolved::Unit { in_game: !order.contains(&k) },
        &mut Vec::new(),
    );
    if removed.as_deref() != Ok(order.as_slice()) {
        r.fail(t, format!("Cleanup of {g}: {removed:?}"));
    }
    for &u in &order {
        let k = if r.left.contains(&u) {
            "forgotten: left its group"
        } else {
            "forgotten: not explained"
        };
        *r.counts.entry(k).or_default() += 1;
        if !r.left.contains(&u) {
            r.fail(
                t,
                format!(
                    "Cleanup of {g} removed {}, which the recording shows in its group",
                    r.names[u as usize]
                ),
            );
        }
    }
    r.mem.insert(g.to_owned(), m);
}

/// Each group row of a `leave_tick` (rows come as changes): its dictionary equals the model's, a
/// valid cached `UnitsList` equals the dictionary's units in the game, and every group without a row
/// still holds what its last row showed.
fn check_rows(r: &mut Rec<'_>, x: &Value, t: i64) {
    let mut seen = HashSet::new();
    for row in x["groups"].as_array().into_iter().flatten() {
        let (Some(g), Some(s)) = (row[0].as_str(), row[1].as_str()) else { continue };
        seen.insert(g.to_owned());
        let dict: Vec<u32> =
            split_ids(row_field(s, "dict=").unwrap_or("")).into_iter().map(|k| r.id(k)).collect();
        let model: Vec<u32> = r.mem.get(g).map(|m| m.keys().collect()).unwrap_or_default();
        *r.counts.entry("dictionary rows").or_default() += 1;
        if model != dict {
            let (a, b) = (r.names(&model), r.names(&dict));
            r.fail(t, format!("{g}'s memory: model {a:?}, game {b:?}"));
        }
        if let Some(list) = row_field(s, "list=").filter(|l| *l != "~invalid") {
            let list: Vec<u32> = split_ids(list).into_iter().map(|k| r.id(k)).collect();
            let fresh: Vec<u32> =
                model.iter().copied().filter(|&k| r.resolve(k).in_game()).collect();
            *r.counts.entry("units lists").or_default() += 1;
            if model.iter().any(|k| r.left.contains(k)) {
                *r.counts.entry("reads depending on a departure").or_default() += 1;
            }
            if fresh != list {
                let (a, b) = (r.names(&fresh), r.names(&list));
                r.fail(t, format!("{g}'s UnitsList: model {a:?}, game {b:?}"));
            }
        }
        let own = split_ids(row_field(s, "f=").unwrap_or(""))
            .into_iter()
            .filter_map(|e| e.split_once(':'))
            .filter(|(_, n)| n.parse::<i64>().is_ok_and(|n| n > 0))
            .map(|(f, _)| f.to_owned())
            .collect();
        let attack =
            split_ids(row_field(s, "af=").unwrap_or("")).into_iter().map(str::to_owned).collect();
        let everyone = row_field(s, "efe=") == Some("1");
        r.factions.insert(g.to_owned(), Factions { own, attack, everyone });
        r.shown.insert(g.to_owned(), dict);
    }
    let unchanged: Vec<String> = r.shown.keys().filter(|g| !seen.contains(*g)).cloned().collect();
    for g in unchanged {
        let model: Vec<u32> = r.mem.get(&g).map(|m| m.keys().collect()).unwrap_or_default();
        if model != r.shown[&g] {
            let (a, b) = (r.names(&model), r.names(&r.shown[&g]));
            r.fail(t, format!("{g}'s memory changed without a row: model {a:?}, last row {b:?}"));
        }
    }
}

/// A `Memory.Enemies` read (engagement, AI, `UnitAttack.UpdateTarget`) against the model's
/// dictionary filtered as `Enemies` fills its cache (in the game, an enemy of the memory's group).
fn check_enemies(r: &mut Rec<'_>, x: &Value, t: i64, frame: Option<(Reader, Option<u32>)>) {
    let Some((who, Some(u))) = frame else {
        r.fail(t, format!("an enemies read outside a known reader: {x}"));
        return;
    };
    *r.counts
        .entry(match who {
            Reader::Engage => "enemies read: engagement",
            Reader::Ai => "enemies read: AI",
            Reader::Command => "enemies read: command",
        })
        .or_default() += 1;
    *r.counts.entry("enemies read").or_default() += 1;
    let Some(g) = r.group.get(&u).cloned() else {
        r.fail(t, format!("enemies read of {} without a group", r.names[u as usize]));
        return;
    };
    let want: Vec<u32> = x["v"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
        .map(|k| r.id(k))
        .collect::<Vec<_>>();
    let keys: Vec<u32> = r.mem.get(&g).map(|m| m.keys().collect()).unwrap_or_default();
    if keys.iter().any(|k| r.left.contains(k)) {
        *r.counts.entry("reads depending on a departure").or_default() += 1;
    }
    let mut got = Vec::new();
    for k in keys {
        if !r.resolve(k).in_game() {
            continue;
        }
        match r.is_enemy(&g, k) {
            Some(true) => got.push(k),
            Some(false) => {}
            None => {
                r.fail(t, format!("{} has no group to tell an enemy by", r.names[k as usize]));
                return;
            }
        }
    }
    if got != want {
        let (a, b) = (r.names(&got), r.names(&want));
        r.fail(t, format!("{who:?} read {g}'s enemies: model {a:?}, game {b:?}"));
    }
}

#[test]
fn a_failing_invisibility_check_leaves_the_target_added_and_visible() {
    let mut f = fight();
    f.invisible.insert(20);
    f.fail_sees_invisible = true;
    let mut c = MemoryController::default();
    assert_eq!(tick(&mut c, &mut f), Err(MemError::NullReference));
    // As the game: `Add` and `Visible = true` happened before the loop threw.
    assert_eq!(keys(&f.mem[&1]), [10, 20]);
    assert!(f.mem[&1].contains_visible(20));
    assert_eq!(f.mem[&1].enemies_cache(), None);
}

#[test]
fn a_failure_in_the_queue_keeps_every_queued_pair() {
    let mut f = fight();
    f.ready.clear();
    f.group.extend([(41, 5), (42, 5)]);
    let mut c = MemoryController::default();
    c.add_to_memory(41, 20, false);
    c.add_to_memory(42, 10, false);
    f.fail_wake = Some(42);
    assert_eq!(tick(&mut c, &mut f), Err(MemError::NullReference));
    // The first pair was applied, and the queue (cleared only after the loop) still holds both.
    assert_eq!(keys(&f.mem[&5]), [20, 10]);
    assert_eq!(c.units_for_add, [(41, 20), (42, 10)]);
}

#[test]
fn a_cleanup_that_throws_leaves_its_removals_to_the_next_one_of_any_memory() {
    // Group A: unit 1 gone, then unit 2's detect time overflows.
    let mut a = GroupMemory::from_entries([
        (1, UnitInfo { last_detect_time: T0, visible: false }),
        (2, UnitInfo { last_detect_time: TimeSpan::MAX, visible: false }),
    ]);
    let mut pending = Vec::new();
    let res = |k: u32| if k == 1 { Resolved::Missing } else { IN };
    assert_eq!(a.cleanup(T0, res, &mut pending), Err(MemError::Cs(CsError::Overflow)));
    assert_eq!((keys(&a), pending.clone()), (vec![1, 2], vec![1]));
    // Group B also remembers 1 (in the game for it): its next Cleanup removes it all the same.
    let mut b = GroupMemory::new();
    b.add(3, T0);
    b.add(1, T0);
    assert_eq!(b.cleanup(T0, |_| IN, &mut pending), Ok(vec![1]));
    assert!(pending.is_empty());
    assert_eq!(keys(&b), [3]);
}

#[test]
fn a_tick_whose_cleanup_fails_part_way_keeps_what_it_collected() {
    let mut f = fight();
    f.resolved.insert(20, Resolved::Missing);
    f.fail_resolve = Some(30);
    let mut c = MemoryController::default();
    assert_eq!(tick(&mut c, &mut f), Err(MemError::NullReference));
    // 20 (no unit) was collected before 30's resolution failed; nothing was removed yet.
    assert_eq!(c.cleanup_pending, [20]);
    assert_eq!(keys(&f.mem[&1]), [10, 20, 11, 30]);
    // The next Cleanup, of any memory, removes it.
    f.fail_resolve = None;
    f.resolved.clear();
    f.ready = vec![9];
    f.mem.entry(9).or_default().add(20, T0);
    tick(&mut c, &mut f).unwrap();
    assert!(c.cleanup_pending.is_empty() && !f.mem[&9].contains(20));
}
