//! `UnlockableFlagsManager` (decompiled:
//! `Kingmaker.AreaLogic.QuestSystem/UnlockableFlagsManager.cs`). Missing and present-with-0
//! are different states. Each change raises `IUnlockHandler`/`IUnlockValueHandler` to
//! global subscribers (the etude system marks its conditions dirty; anything else is
//! fail-closed).

use serde_json::json;
use wr_content::Guid;

use crate::rulebook::bus::raise_global;
use crate::world::World;

pub fn is_unlocked(w: &World, flag: Guid) -> bool {
    w.player.flags.contains_key(&flag)
}

pub fn value(w: &World, flag: Guid) -> i32 {
    w.player.flags.get(&flag).copied().unwrap_or(0)
}

/// WrathOracle's flag record, a postfix on each `UnlockableFlagsManager` method.
fn record(w: &mut World, method: &str, flag: Guid) {
    let unlocked = is_unlocked(w, flag);
    let value = if unlocked { value(w, flag) } else { 0 };
    w.events.mutation(
        &format!("flag_{method}"),
        json!({ "flag": flag.to_string(), "unlocked": unlocked, "value": value }),
    );
}

/// `Unlock(flag)`.
pub fn unlock(w: &mut World, flag: Guid) {
    if !is_unlocked(w, flag) {
        w.player.flags.insert(flag, 0);
        raise_global(w, "IUnlockHandler", None);
        crate::scene_parts::update_all_visibilities(w);
    }
    record(w, "Unlock", flag);
}

/// `Lock(flag)`.
pub fn lock(w: &mut World, flag: Guid) {
    w.player.flags.remove(&flag);
    raise_global(w, "IUnlockHandler", None);
    crate::scene_parts::update_all_visibilities(w);
    record(w, "Lock", flag);
}

/// `SetFlagValue(flag, value)`.
pub fn set_value(w: &mut World, flag: Guid, v: i32) {
    let new = !w.player.flags.contains_key(&flag);
    w.player.flags.insert(flag, v);
    if new {
        raise_global(w, "IUnlockHandler", None);
        crate::scene_parts::update_all_visibilities(w);
    }
    raise_global(w, "IUnlockValueHandler", None);
    record(w, "SetFlagValue", flag);
}
