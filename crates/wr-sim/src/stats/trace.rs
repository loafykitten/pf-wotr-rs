//! WrathOracle's stat records (`Hooks.cs`: `AddModifierPostfix`, `RemoveModifierPostfix`,
//! `UpdateValuePostfix`, `ScopeAddPostfix`, `ScopeRemovePrefix`), written by the ports in
//! [`super::ops`] at the points the oracle's hooks fire, with the oracle's fields. They are
//! `mut` events of the run's log (`stat_value`, `stat_mod_add`, `stat_mod_remove`,
//! `stat_temp_scope_add`, `stat_temp_scope_remove`); `combat::project` compares them
//! (`docs/poc5/stats.md`). Nothing is written while the log is off (hydration).

use serde_json::{Value as J, json};

use super::types::StatType;
use crate::world::{EntityIdx, ModIdx, World};

/// Writes one stat `mut` record: `{"what": what, "stat": t, "owner": u, …fields}` (`StatId`).
/// Inside a recorded combat run the pending draws go first (as every other record of the run).
fn emit(w: &mut World, what: &str, u: EntityIdx, t: StatType, fields: J) {
    if !w.events.on {
        return;
    }
    let J::Object(mut m) = fields else { return };
    m.insert("k".into(), json!("mut"));
    m.insert("what".into(), json!(what));
    m.insert("stat".into(), json!(t.0));
    m.insert("owner".into(), json!(w.entity(u).unique_id));
    match crate::combat::fire::current(w) {
        Some(cx) => cx.emit(w, J::Object(m)),
        None => {
            m.remove("k");
            w.events.mutation(what, J::Object(m));
        }
    }
}

/// `UpdateValuePostfix`: the stat's value changed across its outermost `UpdateValue`.
pub(super) fn value_changed(w: &mut World, u: EntityIdx, t: StatType, old: i32) {
    if !w.events.on {
        return;
    }
    let s = w.stats(u).v(t);
    let fields = json!({"old": old, "new": s.modified_value, "permanent": s.permanent_value});
    emit(w, "stat_value", u, t, fields);
}

/// `AddModifierPostfix` / `RemoveModifierPostfix` (`what`: `stat_mod_add` / `stat_mod_remove`).
pub(super) fn modifier(w: &mut World, what: &str, u: EntityIdx, t: StatType, m: ModIdx) {
    if !w.events.on {
        return;
    }
    let fields = json!({"mod": crate::sheet::modifier_json(w, m), "modified": w.stats(u).v(t).modified_value});
    emit(w, what, u, t, fields);
}

/// `ScopeMut`: a temporary-modifiers scope opened (after its update) or about to close.
pub(super) fn scope(w: &mut World, what: &str, u: EntityIdx, t: StatType, mods: &[ModIdx]) {
    if !w.events.on {
        return;
    }
    let list: Vec<J> = mods.iter().map(|&m| crate::sheet::modifier_json(w, m)).collect();
    emit(w, what, u, t, json!({"mods": list}));
}
