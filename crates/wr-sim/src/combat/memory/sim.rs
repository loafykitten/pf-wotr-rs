//! The group memory in the sim (docs/poc5/memory.md, H2): each group's [`GroupMemory`] (on
//! `groups::Group::mem`), `UnitMemoryController`'s queue, and the reads the other models make of
//! a memory, each checked against the recording's read while the memory is the sim's (a
//! recording with WrathOracle's `memory` observer).

use std::collections::BTreeMap;
use std::fmt::Debug;

use super::{GroupMemory, MemError, MemoryController, Resolved};
use crate::combat::groups::{self, GroupIdx};
use crate::world::{EntityIdx, World};

/// The memory's run state.
#[derive(Debug, Default)]
pub struct MemoryRun {
    /// The memory is the sim's.
    pub on: bool,
    /// `UnitMemoryController.m_UnitsForAdd` and `UnitGroupMemory.s_UnitsForRemove`.
    pub ctl: MemoryController<EntityIdx>,
    /// The reads checked against the recording's, by reader.
    pub checked: BTreeMap<&'static str, usize>,
}

/// `UnitInfo.Unit` of a remembered unit: the unit while it's the entity with that id and not
/// disposed (else `null`), and its `IsInGame`.
#[must_use]
pub fn resolve(w: &World, u: EntityIdx) -> Resolved {
    let e = w.entity(u);
    if w.by_id.get(&e.unique_id) == Some(&u) && !e.is_disposed && w.unit(u).is_some() {
        Resolved::Unit { in_game: e.is_in_game }
    } else {
        Resolved::Missing
    }
}

/// A group as WrathOracle's records name it: its id, `~fake` or `~disposed:<id>`.
#[must_use]
pub fn group_key(w: &World, g: GroupIdx) -> String {
    let grp = w.combat.groups.get(g);
    match &grp.id {
        None => "~fake".to_owned(),
        Some(id) if grp.disposed => format!("~disposed:{id}"),
        Some(id) => id.clone(),
    }
}

/// `group.IsEnemy(unit)`: the unit's group (`unit.Group`, which runs `UpdateGroup`) against `g`.
pub fn is_enemy_unit(w: &mut World, g: GroupIdx, u: EntityIdx) -> Option<bool> {
    let gu = groups::group_of(w, u)?;
    Some(groups::is_enemy(w, g, gu))
}

/// Runs `f` on group `g`'s memory, taken out of the world for the call (so `f` can read and
/// update the world: `Group` reads run `UpdateGroup`); `None` when the group has none.
pub fn with_memory<T>(
    w: &mut World,
    g: GroupIdx,
    f: impl FnOnce(&mut World, &mut GroupMemory<EntityIdx>) -> T,
) -> Option<T> {
    let mut m = w.combat.groups.get_mut(g).mem.take()?;
    let out = f(w, &mut m);
    w.combat.groups.get_mut(g).mem = Some(m);
    Some(out)
}

/// `Memory.Enemies` of group `g` (filled if invalid: `unit != null && unit.IsInGame &&
/// Group.IsEnemy(unit)`, in enumeration order). The memory stays in the world while it fills: a
/// unit's `Group` read runs `UpdateGroup`, whose `Add` clears this list part way, as in the game.
pub fn enemies_of(w: &mut World, g: GroupIdx) -> Option<Vec<EntityIdx>> {
    fn mem(w: &mut World, g: GroupIdx) -> Option<&mut GroupMemory<EntityIdx>> {
        w.combat.groups.get_mut(g).mem.as_mut()
    }
    let Some(keys) = mem(w, g)?.begin_enemies_fill() else {
        return Some(mem(w, g)?.enemies_cache()?.to_vec());
    };
    for k in keys {
        if resolve(w, k).in_game() && is_enemy_unit(w, g, k)? {
            mem(w, g)?.push_enemy(k);
        }
    }
    Some(mem(w, g)?.end_enemies_fill().to_vec())
}

/// `unit.Group.Memory.Enemies` (`Group` runs `UpdateGroup`).
pub fn enemies(w: &mut World, u: EntityIdx) -> Option<Vec<EntityIdx>> {
    let g = groups::group_of(w, u)?;
    enemies_of(w, g)
}

/// `owner.Memory.Contains(u)` (`ContainsVisible` with `visible`).
pub fn contains(w: &mut World, owner: EntityIdx, u: EntityIdx, visible: bool) -> Option<bool> {
    let g = groups::group_of(w, owner)?;
    let m = w.combat.groups.get(g).mem.as_ref()?;
    Some(if visible { m.contains_visible(u) } else { m.contains(u) })
}

/// `owner.Group.Memory.HasPlayerCharacterInMemory()`: `UnitsList` (filled if invalid), any of the
/// player's faction (a cached unit that no longer resolves throws).
pub fn has_player_character(w: &mut World, owner: EntityIdx) -> Option<Result<bool, MemError>> {
    let g = groups::group_of(w, owner)?;
    with_memory(w, g, |w, m| {
        let list = m.units_list_by(|k| resolve(w, k).in_game()).to_vec();
        for k in list {
            if resolve(w, k) == Resolved::Missing {
                return Err(MemError::NullReference);
            }
            if crate::combat::units::is_player_faction(w, k) {
                return Ok(true);
            }
        }
        Ok(false)
    })
}

/// A memory read another model makes: while the memory is the sim's, the sim's value (`computed`,
/// asked only then), the recording's checked against it and a difference reported; else the
/// recording's.
pub fn check<T: PartialEq + Debug>(
    w: &mut World,
    reader: &'static str,
    recorded: T,
    computed: impl FnOnce(&mut World) -> Option<T>,
) -> T {
    if !w.combat.memory.on {
        return recorded;
    }
    let Some(c) = computed(w) else {
        w.coverage.unsupported(format!("memory: {reader}: the sim can't read the memory"));
        return recorded;
    };
    *w.combat.memory.checked.entry(reader).or_default() += 1;
    if c != recorded {
        if std::env::var_os("WR_DEBUG_MEMORY").is_some() {
            eprintln!("DEBUG memory {reader}: sim {c:?}, recording {recorded:?}");
        }
        w.coverage.unsupported(format!("memory: the sim's {reader} differs from the recording's"));
    }
    c
}

/// A memory the sim keeps: its changes logged (WrathOracle records each `Add` of a new key,
/// `Remove`, `Cleanup`'s removals and `Clear`: the replay matches them).
#[must_use]
pub fn logged(mut m: GroupMemory<EntityIdx>) -> GroupMemory<EntityIdx> {
    m.log_changes();
    m
}

/// Every memory's changes since the last call (disposed groups' too), in the order they were made.
pub fn take_changes(w: &mut World) -> Vec<(GroupIdx, super::Change<EntityIdx>)> {
    let mut changed = Vec::new();
    for (i, grp) in w.combat.groups.arena.iter_mut().enumerate() {
        if let Some(m) = grp.mem.as_mut() {
            changed.extend(m.take_ordered_changes().into_iter().map(|(n, c)| (n, GroupIdx(i), c)));
        }
    }
    changed.sort_unstable_by_key(|x| x.0);
    changed.into_iter().map(|(_, g, c)| (g, c)).collect()
}

/// The units' ids.
#[must_use]
pub fn ids(w: &World, v: &[EntityIdx]) -> Vec<String> {
    v.iter().map(|&u| w.entity(u).unique_id.clone()).collect()
}
