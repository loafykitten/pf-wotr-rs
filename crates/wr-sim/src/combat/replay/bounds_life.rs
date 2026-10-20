//! The life of each entity-bounds collider, as one state machine for units' and others' alike
//! (docs/poc5/vision.md, "The window's start, from the records"). Every record family that names
//! a collider is checked against it at its place in the stream, with its full field set against
//! the collider's state, so no family has a rule of its own that another can step around.
//!
//! One owner of each instance id, across both kinds, for good (Unity doesn't reuse them within
//! a session). A collider is
//! - `Live` from `bounds_init`'s root, or from the `bounds_tick` call that made it;
//! - `Linking` from the `entity` record (`EntityDataLink.Entity = view.Data`, written inside
//!   `SetupSphereBoundsCollider`) to the making call's own record; the record needs the entity to
//!   have no collider that is its view's (the previous one is cleared first: `gone`);
//! - `Dying` from its `gone` (`DestroySphereBoundsCollider` clears the view's reference at once
//!   and `Object.Destroy` removes the collider at the frame's end) to the next frame;
//! - `Gone` after that.
//!
//! `current` is each entity's view collider (what `TickEntity`, `UpdateViewActive` and
//! `UpdateSphereColliderPositionIfNeeded` act on).
//!
//! What the records of a view replaced would show (the old collider unlinked, the new view
//! attached) is not modelled: an unlink or relink is reported, counted, not guessed at.

use std::collections::HashMap;

use crate::combat::orientation;
use crate::combat::vision::V3;

/// A collider's owner: an entity of a kind the bounds model uses (a unit) or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Unit,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum State {
    Linking,
    Live,
    Dying,
    Gone,
}

#[derive(Debug, Clone)]
struct Life {
    state: State,
    kind: Option<Kind>,
    /// The entity type's name, as the records write it.
    kind_name: Option<String>,
    entity: Option<String>,
    on: bool,
    pos: V3,
    radius: f32,
    /// The `enabled` the creation's `entity` record showed: the making call's must equal it.
    link_on: Option<bool>,
    /// The transform the creation's `entity` record showed: the view's, which the making call
    /// leaves for any entity but a unit (a unit's is set to its position unless `==` this).
    link_pos: Option<V3>,
}

/// Which `bounds_set` record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SetFn {
    Active,
    Pos,
    Gone,
    Link,
    Entity,
}

/// A `bounds_set` record with every field it carries (docs/poc5/vision.md has the table of what
/// each must be).
#[derive(Debug, Clone, Copy)]
pub(super) struct SetRec<'a> {
    pub f: SetFn,
    pub v: Option<&'a str>,
    pub c: Option<i32>,
    pub on0: Option<bool>,
    pub on: Option<bool>,
    pub pos0: Option<V3>,
    pub pos: Option<V3>,
    pub e0: Option<&'a str>,
    pub e: Option<&'a str>,
}

/// A record, as the machine takes it.
#[derive(Debug, Clone, Copy)]
pub(super) enum Ev<'a> {
    /// A `bounds_init` root.
    Root {
        id: i32,
        kind: Kind,
        kind_name: &'a str,
        entity: Option<&'a str>,
        on: bool,
        radius: f32,
        pos: V3,
    },
    /// A `bounds_tick`.
    Tick {
        entity: &'a str,
        kind: Kind,
        kind_name: &'a str,
        made: bool,
        c: Option<i32>,
        r0: Option<f32>,
        r: Option<f32>,
        pos: Option<V3>,
        on: Option<bool>,
    },
    /// A `bounds_set`.
    Set(SetRec<'a>),
    /// A `phys_sync`.
    Sync,
}

fn bits(v: V3) -> [u32; 3] {
    v.map(f32::to_bits)
}

/// The collider fields of a view's collider as `ColliderOf` writes them, against the machine's.
fn want<T: PartialEq + std::fmt::Debug>(
    out: &mut Vec<String>,
    what: &str,
    field: &str,
    got: Option<T>,
    expected: Option<T>,
) {
    if got != expected {
        out.push(format!("{what}: `{field}` is {got:?}, it must be {expected:?}"));
    }
}

/// The colliders' lives.
#[derive(Debug, Default)]
pub(super) struct Lifecycle {
    cols: HashMap<i32, Life>,
    current: HashMap<String, i32>,
    /// The collider being made: its `entity` record was written, its call's record has not been.
    /// `TickEntity` calls are sequential and the creation's link is written inside the call, so
    /// nothing else is written in between.
    making: Option<i32>,
    /// Each entity's type name, from the first record that names it (a type doesn't change).
    types: HashMap<String, String>,
}

impl Lifecycle {
    /// The kind of a collider that has been made (`None`: unknown, or not made yet).
    pub(super) fn kind_of(&self, c: i32) -> Option<Kind> {
        self.cols.get(&c).and_then(|l| l.kind)
    }

    /// An entity's type, as the records write it: the same for every collider it ever has.
    fn type_of(&mut self, entity: &str, kind_name: &str, out: &mut Vec<String>) {
        let known = self.types.entry(entity.to_owned()).or_insert_with(|| kind_name.to_owned());
        if known != kind_name {
            out.push(format!("{entity} is a {known} and a {kind_name} in the records"));
        }
    }

    /// The transform a collider's creation link showed (until its call is applied).
    pub(super) fn link_pos(&self, c: i32) -> Option<V3> {
        self.cols.get(&c).and_then(|l| l.link_pos)
    }

    /// How many colliders were ever seen.
    pub(super) fn seen(&self) -> usize {
        self.cols.len()
    }

    /// The end of a frame: what was destroyed is gone, and a collider that was linked and not
    /// made by a call is reported.
    pub(super) fn frame(&mut self, out: &mut Vec<String>) {
        let mut ended = Vec::new();
        for (c, l) in &mut self.cols {
            match l.state {
                State::Dying => l.state = State::Gone,
                State::Linking => {
                    out.push(format!(
                        "collider {c} was linked to {:?} and no call made it",
                        l.entity
                    ));
                    l.state = State::Gone;
                    ended.push(*c);
                }
                _ => {}
            }
        }
        self.making = None;
        self.current.retain(|_, c| !ended.contains(c));
    }

    /// The collider a view operation acts on: live, and its entity's view collider.
    fn view_collider(&self, c: i32, what: &str, out: &mut Vec<String>) -> Option<&Life> {
        let Some(l) = self.cols.get(&c) else {
            out.push(format!("{what} names collider {c}, which no record made"));
            return None;
        };
        if l.state != State::Live {
            out.push(format!(
                "{what} names collider {c}, which is {:?} (its view's reference is cleared, or it isn't made yet)",
                l.state
            ));
            return None;
        }
        if let Some(e) = &l.entity
            && self.current.get(e) != Some(&c)
        {
            out.push(format!("{what} names collider {c}, which isn't {e}'s view collider"));
            return None;
        }
        Some(l)
    }

    /// Takes a record at its place in the stream; what contradicts the machine is pushed to `out`.
    pub(super) fn apply(&mut self, ev: Ev<'_>, out: &mut Vec<String>) {
        if let Some(m) = self.making {
            let own = matches!(ev, Ev::Tick { made: true, c: Some(c), .. } if c == m);
            if !own {
                out.push(format!(
                    "a record inside the making of collider {m} (its link is written inside the call, before the call's own record)"
                ));
            }
        }
        match ev {
            Ev::Root { id, kind, kind_name, entity, on, radius, pos } => {
                if self.cols.contains_key(&id) {
                    out.push(format!("the root {id} names an instance id that is owned already"));
                    return;
                }
                if let Some(e) = entity {
                    self.type_of(e, kind_name, out);
                    if let Some(cur) = self.current.get(e) {
                        out.push(format!("{e} has two colliders at the start: {cur} and {id}"));
                    }
                    self.current.insert(e.to_owned(), id);
                }
                self.cols.insert(
                    id,
                    Life {
                        state: State::Live,
                        kind: Some(kind),
                        kind_name: Some(kind_name.to_owned()),
                        entity: entity.map(str::to_owned),
                        on,
                        pos,
                        radius,
                        link_on: None,
                        link_pos: None,
                    },
                );
            }
            Ev::Sync => {}
            Ev::Tick { entity, kind, kind_name, made, c, r0, r, pos, on } => {
                self.tick(entity, (kind, kind_name), made, c, r0, r, pos, on, out);
            }
            Ev::Set(r) => self.set(r, out),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn tick(
        &mut self,
        entity: &str,
        (kind, kind_name): (Kind, &str),
        made: bool,
        c: Option<i32>,
        r0: Option<f32>,
        r: Option<f32>,
        pos: Option<V3>,
        on: Option<bool>,
        out: &mut Vec<String>,
    ) {
        // (The oracle writes the entity's type whether or not its view has a collider.)
        self.type_of(entity, kind_name, out);
        let (Some(c), Some(r), Some(pos), Some(on)) = (c, r, pos, on) else {
            // No collider after the call (the entity has no view, or the view has none).
            if made || c.is_some() || r0.is_some() || r.is_some() || pos.is_some() || on.is_some() {
                out.push(format!("a bounds_tick of {entity} with a collider in part"));
            } else if let Some(cur) = self.current.get(entity) {
                out.push(format!(
                    "a bounds_tick of {entity} without a collider, but collider {cur} is its view collider"
                ));
            }
            return;
        };
        if made {
            self.making = None;
            let Some(l) = self.cols.get_mut(&c) else {
                out.push(format!(
                    "a bounds_tick of {entity} made collider {c}, which no entity record linked before the call"
                ));
                return;
            };
            if l.state != State::Linking || l.entity.as_deref() != Some(entity) {
                out.push(format!(
                    "a bounds_tick of {entity} made collider {c}, which is {:?} and bound to {:?}",
                    l.state, l.entity
                ));
                return;
            }
            if r0.is_some() {
                out.push(format!("a bounds_tick that made collider {c} has a radius before"));
            }
            // `SetupSphereBoundsCollider` adds an enabled collider of at least 0.05, and the call
            // doesn't change `enabled` (a hidden entity's collider is enabled too).
            if !on {
                out.push(format!("a bounds_tick made collider {c} disabled"));
            }
            if r < 0.05 {
                out.push(format!(
                    "a bounds_tick made collider {c} of radius {r}, under the 0.05 minimum"
                ));
            }
            if l.link_on.is_some_and(|x| x != on) {
                out.push(format!(
                    "collider {c} was linked enabled {:?} and its making call shows {on}",
                    l.link_on
                ));
            }
            // `TickEntity` moves a unit's collider only: any other kind's keeps the transform
            // the creation set (the link showed it).
            if kind == Kind::Other && l.link_pos.map(bits) != Some(bits(pos)) {
                out.push(format!(
                    "collider {c} (not a unit's) was made at {pos:?}, it was linked at {:?}",
                    l.link_pos
                ));
            }
            (l.state, l.kind, l.on, l.pos, l.radius) = (State::Live, Some(kind), on, pos, r);
            l.kind_name = Some(kind_name.to_owned());
            return;
        }
        let Some(l) = self.cols.get(&c) else {
            out.push(format!("a bounds_tick of {entity} names collider {c}, which no record made"));
            return;
        };
        if l.state != State::Live
            || l.entity.as_deref() != Some(entity)
            || self.current.get(entity) != Some(&c)
            || l.kind != Some(kind)
            || l.kind_name.as_deref() != Some(kind_name)
        {
            out.push(format!(
                "a bounds_tick of {entity} names collider {c}, which is {:?}, bound to {:?}, and {entity}'s view collider is {:?}",
                l.state,
                l.entity,
                self.current.get(entity)
            ));
            return;
        }
        if r0.map(f32::to_bits) != Some(l.radius.to_bits()) {
            out.push(format!(
                "a bounds_tick of collider {c} has radius before {r0:?}, the collider's is {}",
                l.radius
            ));
        }
        if l.on != on {
            out.push(format!(
                "a bounds_tick of collider {c} shows enabled {on}, the collider is {}",
                l.on
            ));
        }
        // `ScaleRadius` never gives less than 0.05, a unit's included.
        if r < 0.05 {
            out.push(format!(
                "a bounds_tick of collider {c} left radius {r}, under the 0.05 minimum"
            ));
        }
        // `TickEntity` moves and resizes a unit's collider only: any other kind's is left as it
        // was made.
        if kind == Kind::Other && (r.to_bits() != l.radius.to_bits() || bits(pos) != bits(l.pos)) {
            out.push(format!(
                "a bounds_tick of collider {c} (not a unit's) changed its radius to {r} or its transform to {pos:?}"
            ));
            return;
        }
        if let Some(l) = self.cols.get_mut(&c) {
            (l.radius, l.pos) = (r, pos);
        }
    }

    #[allow(clippy::too_many_lines)]
    fn set(&mut self, r: SetRec<'_>, out: &mut Vec<String>) {
        let SetRec { f, v, c, on0, on, pos0, pos, e0, e } = r;
        let n0 = out.len();
        match f {
            SetFn::Link if c.is_none() => {
                // The view has no collider (a view attached or detached: the records can't tell
                // which): every collider field is null and nothing is permitted.
                let what = "a link record with no collider";
                for (field, null) in [
                    ("on0", on0.is_none()),
                    ("on", on.is_none()),
                    ("pos0", pos0.is_none()),
                    ("pos", pos.is_none()),
                    ("e0", e0.is_none()),
                    ("e", e.is_none()),
                ] {
                    if !null {
                        out.push(format!("{what}: `{field}` must be null"));
                    }
                }
                if v.is_none() {
                    out.push(format!("{what}: `v` (the view's data) must be set"));
                }
                if let Some(cur) = v.and_then(|v| self.current.get(v)) {
                    out.push(format!(
                        "{what} says {v:?}'s view has none, but collider {cur} is its view collider"
                    ));
                }
            }
            SetFn::Entity => self.entity(r, out),
            _ => {
                let what = match f {
                    SetFn::Active => "bounds_set active",
                    SetFn::Pos => "bounds_set pos",
                    SetFn::Gone => "bounds_set gone",
                    _ => "a link record",
                };
                let Some(c) = c else {
                    out.push(format!("{what} names no collider"));
                    return;
                };
                let Some(l) = self.view_collider(c, what, out).cloned() else { return };
                let ex = l.entity.as_deref();
                // The view's data and the link's entity are the collider's entity throughout.
                want(out, what, "v", v, ex);
                want(out, what, "e0", e0, ex);
                if f != SetFn::Gone {
                    want(out, what, "e", e, ex);
                } else {
                    // (The view's collider is cleared: `ColliderOf` finds none after.)
                    want(out, what, "e", e, None);
                    want(out, what, "on", on, None);
                    want(out, what, "pos", pos, None);
                }
                want(out, what, "on0", on0, Some(l.on));
                want(out, what, "pos0", pos0.map(bits), Some(bits(l.pos)));
                match f {
                    // Written only when something changed.
                    SetFn::Active => {
                        want(out, what, "on", on, Some(!l.on));
                        want(out, what, "pos", pos.map(bits), Some(bits(l.pos)));
                    }
                    SetFn::Pos => {
                        want(out, what, "on", on, Some(l.on));
                        // (Unity's `Vector3 !=`: a smaller move isn't one, and isn't written.)
                        if pos.is_none_or(|p| orientation::vec3_eq(p, l.pos)) {
                            out.push(format!("{what}: `pos` must be a position that changed"));
                        }
                    }
                    SetFn::Link => {
                        want(out, what, "on", on, Some(l.on));
                        want(out, what, "pos", pos.map(bits), Some(bits(l.pos)));
                        if ex.is_none() {
                            out.push(format!("{what}: the collider has no entity to relink"));
                        }
                    }
                    _ => {}
                }
                if out.len() > n0 {
                    return;
                }
                let Some(life) = self.cols.get_mut(&c) else { return };
                match f {
                    SetFn::Active => life.on = on.unwrap_or(life.on),
                    SetFn::Pos => life.pos = pos.unwrap_or(life.pos),
                    SetFn::Gone => {
                        life.state = State::Dying;
                        if let Some(e) = l.entity {
                            self.current.remove(&e);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    /// `EntityDataLink.Entity`'s setter: the link of a collider made (once, before the call's
    /// record) or the setter set to what it holds.
    fn entity(&mut self, r: SetRec<'_>, out: &mut Vec<String>) {
        let SetRec { v, c, on0, on, pos0, pos, e0, e, .. } = r;
        let what = "an entity record";
        want(out, what, "v", v, None);
        want(out, what, "on0", on0, None);
        want(out, what, "pos0", pos0, None);
        let Some(c) = c else {
            out.push(format!("{what} names no collider"));
            return;
        };
        let (Some(on), Some(pos)) = (on, pos) else {
            out.push(format!("{what} of collider {c} lacks the collider's `enabled` or position"));
            return;
        };
        match (self.cols.get(&c), e0, e) {
            (None, None, Some(e)) => {
                // The creation's link: the previous view collider is cleared first, and the new
                // collider is enabled (its position is the view's, set before the call sets the
                // transform: taken).
                if !on {
                    out.push(format!("collider {c} is linked disabled"));
                }
                if let Some(cur) = self.current.get(e) {
                    out.push(format!(
                        "collider {c} is linked to {e} while collider {cur} is its view collider (the previous one is destroyed first)"
                    ));
                }
                self.current.insert(e.to_owned(), c);
                self.making = Some(c);
                self.cols.insert(
                    c,
                    Life {
                        state: State::Linking,
                        kind: None,
                        kind_name: None,
                        entity: Some(e.to_owned()),
                        on: false,
                        pos: [0.0; 3],
                        radius: 0.0,
                        link_on: Some(on),
                        link_pos: Some(pos),
                    },
                );
            }
            (None, _, _) => out.push(format!(
                "an entity record of collider {c} from {e0:?} to {e:?}: no record made it"
            )),
            (Some(l), Some(a), Some(b))
                if a == b && l.state == State::Live && l.entity.as_deref() == Some(a) =>
            {
                // (The setter set to what it holds.)
                want(out, what, "on", Some(on), Some(l.on));
                want(out, what, "pos", Some(bits(pos)), Some(bits(l.pos)));
            }
            (Some(l), _, _) => out.push(format!(
                "an entity record of collider {c} ({:?}, bound to {:?}) from {e0:?} to {e:?}: a second link, or an unlink or relink, not modelled",
                l.state, l.entity
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: V3 = [1.0, 2.0, 3.0];
    const Q: V3 = [5.0, 6.0, 7.0];

    fn run(m: &mut Lifecycle, evs: &[Ev<'_>]) -> Vec<String> {
        let mut out = Vec::new();
        for e in evs {
            m.apply(*e, &mut out);
        }
        out
    }

    fn root(id: i32, kind: Kind, entity: Option<&str>) -> Ev<'_> {
        Ev::Root {
            id,
            kind,
            kind_name: if kind == Kind::Unit { "UnitEntityData" } else { "MapObjectEntityData" },
            entity,
            on: true,
            radius: 0.5,
            pos: P,
        }
    }

    /// A `bounds_set` of collider 7 (a unit's, "a", enabled at P) of kind `f` with the fields the
    /// oracle writes for it.
    fn set<'a>(f: SetFn, c: i32, e: &'a str) -> SetRec<'a> {
        let base = SetRec {
            f,
            v: Some(e),
            c: Some(c),
            on0: Some(true),
            on: Some(true),
            pos0: Some(P),
            pos: Some(P),
            e0: Some(e),
            e: Some(e),
        };
        match f {
            SetFn::Active => SetRec { on: Some(false), ..base },
            SetFn::Pos => SetRec { pos: Some(Q), ..base },
            SetFn::Gone => SetRec { on: None, pos: None, e: None, ..base },
            SetFn::Link => base,
            SetFn::Entity => SetRec { v: None, on0: None, pos0: None, ..base },
        }
    }

    fn entity<'a>(c: i32, e: &'a str) -> Ev<'a> {
        Ev::Set(SetRec {
            f: SetFn::Entity,
            v: None,
            c: Some(c),
            on0: None,
            on: Some(true),
            pos0: None,
            pos: Some(P),
            e0: None,
            e: Some(e),
        })
    }

    fn gone(c: i32, e: &str) -> Ev<'_> {
        Ev::Set(set(SetFn::Gone, c, e))
    }

    fn making<'a>(c: i32, e: &'a str, kind: Kind) -> Ev<'a> {
        Ev::Tick {
            entity: e,
            kind,
            kind_name: if kind == Kind::Unit { "UnitEntityData" } else { "MapObjectEntityData" },
            made: true,
            c: Some(c),
            r0: None,
            r: Some(0.5),
            pos: Some(P),
            on: Some(true),
        }
    }

    fn tick<'a>(c: i32, e: &'a str) -> Ev<'a> {
        Ev::Tick {
            entity: e,
            kind: Kind::Unit,
            kind_name: "UnitEntityData",
            made: false,
            c: Some(c),
            r0: Some(0.5),
            r: Some(0.5),
            pos: Some(P),
            on: Some(true),
        }
    }

    fn started() -> Lifecycle {
        let mut m = Lifecycle::default();
        let out =
            run(&mut m, &[root(7, Kind::Unit, Some("a")), root(50, Kind::Other, Some("obj"))]);
        assert!(out.is_empty(), "{out:?}");
        m
    }

    #[test]
    fn a_collider_is_destroyed_and_made_again_in_the_oracles_order() {
        let mut m = started();
        let ok =
            run(&mut m, &[gone(7, "a"), entity(8, "a"), making(8, "a", Kind::Unit), tick(8, "a")]);
        assert!(ok.is_empty(), "{ok:?}");
        // The destroyed one takes no view operation this frame, and is gone after it.
        let bad = run(&mut m, &[Ev::Set(set(SetFn::Pos, 7, "a")), tick(7, "a")]);
        assert_eq!(bad.len(), 2, "{bad:?}");
        let mut end = Vec::new();
        m.frame(&mut end);
        assert!(end.is_empty());
    }

    #[test]
    fn the_creation_needs_the_previous_collider_destroyed_before_its_link() {
        // Sol's round 5: `entity(c9), gone(c7), making(c9)` can't be written by the game.
        let mut m = started();
        let bad = run(&mut m, &[entity(9, "a"), gone(7, "a"), making(9, "a", Kind::Unit)]);
        assert!(bad.iter().any(|x| x.contains("is its view collider")), "{bad:?}");
        // A making with no link, a link with no making, a second link.
        let mut m = started();
        let bad = run(&mut m, &[gone(7, "a"), making(8, "a", Kind::Unit)]);
        assert!(bad.iter().any(|x| x.contains("no entity record linked")), "{bad:?}");
        let mut m = started();
        let mut end = Vec::new();
        let bad = run(&mut m, &[gone(7, "a"), entity(8, "a")]);
        assert!(bad.is_empty());
        m.frame(&mut end);
        assert!(end.iter().any(|x| x.contains("no call made it")), "{end:?}");
        let mut m = started();
        let bad = run(
            &mut m,
            &[gone(7, "a"), entity(8, "a"), entity(8, "a"), making(8, "a", Kind::Unit)],
        );
        assert!(bad.iter().any(|x| x.contains("a second link")), "{bad:?}");
        // The link of the making and the call must agree on the entity, and the collider is
        // enabled whatever is written.
        let mut m = started();
        let bad = run(&mut m, &[gone(7, "a"), entity(8, "b"), making(8, "a", Kind::Unit)]);
        assert!(!bad.is_empty());
        let mut m = started();
        let mut call = making(8, "a", Kind::Unit);
        if let Ev::Tick { on, .. } = &mut call {
            *on = Some(false);
        }
        let bad = run(&mut m, &[gone(7, "a"), entity(8, "a"), call]);
        assert!(bad.iter().any(|x| x.contains("made collider 8 disabled")), "{bad:?}");
        // (Sol's round 6: a disabled link and a matching disabled call agree with each other
        // and with nothing the game writes.)
        let mut m = started();
        let mut link = set(SetFn::Entity, 8, "a");
        (link.e0, link.on) = (None, Some(false));
        let mut call = making(8, "a", Kind::Unit);
        if let Ev::Tick { on, .. } = &mut call {
            *on = Some(false);
        }
        let bad = run(&mut m, &[gone(7, "a"), Ev::Set(link), call]);
        assert!(bad.iter().any(|x| x.contains("linked disabled")), "{bad:?}");
    }

    #[test]
    fn making_intervals_do_not_interleave() {
        // Sol's round 6: `entity(c7), entity(c8), made(c8), made(c7)` can't be written: the
        // calls are sequential and each link is inside its own call.
        let mut m = Lifecycle::default();
        let bad = run(
            &mut m,
            &[
                entity(7, "a"),
                entity(8, "b"),
                making(8, "b", Kind::Unit),
                making(7, "a", Kind::Unit),
            ],
        );
        assert!(bad.iter().any(|x| x.contains("inside the making of collider 7")), "{bad:?}");
        // Anything written inside a making is reported, a sync included.
        let mut m = started();
        let bad =
            run(&mut m, &[gone(7, "a"), entity(8, "a"), Ev::Sync, making(8, "a", Kind::Unit)]);
        assert_eq!(bad.len(), 1, "{bad:?}");
        let mut m = started();
        let ok = run(&mut m, &[gone(7, "a"), entity(8, "a"), making(8, "a", Kind::Unit), Ev::Sync]);
        assert!(ok.is_empty(), "{ok:?}");
    }

    #[test]
    fn an_instance_id_has_one_owner_for_good_across_kinds() {
        // Sol's round 5: a unit's making can't take a live map object's id; nor a gone one's.
        let mut m = started();
        let bad = run(&mut m, &[gone(7, "a"), entity(50, "a")]);
        assert!(bad.iter().any(|x| x.contains("a second link")), "{bad:?}");
        let mut m = started();
        let bad = run(&mut m, &[root(50, Kind::Unit, Some("z"))]);
        assert!(bad.iter().any(|x| x.contains("owned already")), "{bad:?}");
        let mut m = started();
        run(&mut m, &[gone(7, "a")]);
        m.frame(&mut Vec::new());
        let bad = run(&mut m, &[entity(7, "a")]);
        assert!(!bad.is_empty(), "a destroyed collider's id isn't reused");
        // A map object's creation link twice, across frames or late.
        let mut m = started();
        let ok = run(&mut m, &[entity(60, "obj2"), making(60, "obj2", Kind::Other)]);
        assert!(ok.is_empty(), "{ok:?}");
        m.frame(&mut Vec::new());
        let bad = run(&mut m, &[entity(60, "obj2")]);
        assert!(bad.iter().any(|x| x.contains("a second link")), "{bad:?}");
        assert_eq!(m.kind_of(60), Some(Kind::Other));
        assert_eq!(m.kind_of(7), Some(Kind::Unit));
    }

    #[test]
    fn a_map_objects_collider_keeps_its_radius_and_transform() {
        // Sol's round 6: `TickEntity` resizes and moves a unit's collider only.
        let mut m = started();
        let other = |r: f32, pos: V3| Ev::Tick {
            entity: "obj",
            kind: Kind::Other,
            kind_name: "MapObjectEntityData",
            made: false,
            c: Some(50),
            r0: Some(0.5),
            r: Some(r),
            pos: Some(pos),
            on: Some(true),
        };
        assert!(run(&mut m, &[other(0.5, P)]).is_empty());
        assert_eq!(run(&mut m, &[other(99.0, P)]).len(), 1);
        assert_eq!(run(&mut m, &[other(0.5, Q)]).len(), 1);
        // (A unit's does change.)
        let mut unit = tick(7, "a");
        if let Ev::Tick { r, pos, .. } = &mut unit {
            (*r, *pos) = (Some(0.9), Some(Q));
        }
        assert!(run(&mut m, &[unit]).is_empty());
    }

    #[test]
    fn every_set_record_is_checked_in_all_its_fields() {
        // Sol's round 6: `active`, `pos` and `gone` carry the collider's state and its link.
        let mut m = started();
        assert!(run(&mut m, &[Ev::Set(set(SetFn::Active, 7, "a"))]).is_empty());
        let mut m = started();
        assert!(run(&mut m, &[Ev::Set(set(SetFn::Pos, 7, "a"))]).is_empty());
        let mut m = started();
        assert!(run(&mut m, &[Ev::Set(set(SetFn::Link, 7, "a"))]).is_empty());
        for f in [SetFn::Active, SetFn::Pos, SetFn::Gone, SetFn::Link] {
            for (field, why) in [
                ("v", "`v`"),
                ("e0", "`e0`"),
                ("on0", "`on0`"),
                ("pos0", "`pos0`"),
                ("on", "`on`"),
                ("pos", "`pos`"),
                ("e", "`e`"),
            ] {
                let mut r = set(f, 7, "a");
                match field {
                    "v" => r.v = Some("b"),
                    "e0" => r.e0 = Some("b"),
                    "e" => r.e = r.e.map_or(Some("b"), |_| None),
                    "on0" => r.on0 = Some(false),
                    "on" => r.on = r.on.map_or(Some(true), |x| Some(!x)),
                    "pos0" => r.pos0 = Some([9.0; 3]),
                    _ => r.pos = r.pos.map_or(Some([9.0; 3]), |_| None),
                }
                let mut m = started();
                let out = run(&mut m, &[Ev::Set(r)]);
                assert!(!out.is_empty(), "{f:?} {field} {why}: nothing reported");
            }
        }
    }

    #[test]
    fn a_link_record_is_checked_against_the_colliders_whole_state() {
        let mut m = started();
        let ok = run(&mut m, &[Ev::Set(set(SetFn::Link, 7, "a"))]);
        assert!(ok.is_empty(), "{ok:?}");
        // The view's data is the collider's entity (Sol's round 6).
        let mut r = set(SetFn::Link, 7, "a");
        r.v = Some("b");
        assert_eq!(run(&mut m, &[Ev::Set(r)]).len(), 1);
        // An unlink or relink is not modelled.
        let mut r = set(SetFn::Link, 7, "a");
        r.e = None;
        assert!(!run(&mut m, &[Ev::Set(r)]).is_empty());
        // A null-collider link says the view has none, with every field null and `v` set.
        let none = SetRec {
            f: SetFn::Link,
            v: Some("a"),
            c: None,
            on0: None,
            on: None,
            pos0: None,
            pos: None,
            e0: None,
            e: None,
        };
        assert_eq!(run(&mut m, &[Ev::Set(none)]).len(), 1);
        let nobody = SetRec { v: Some("nobody"), ..none };
        assert!(run(&mut m, &[Ev::Set(nobody)]).is_empty());
        for bad in [
            SetRec { on: Some(true), ..nobody },
            SetRec { on0: Some(true), ..nobody },
            SetRec { pos: Some(P), ..nobody },
            SetRec { pos0: Some(P), ..nobody },
            SetRec { e: Some("a"), ..nobody },
            SetRec { e0: Some("a"), ..nobody },
            SetRec { v: None, ..nobody },
        ] {
            assert_eq!(run(&mut m, &[Ev::Set(bad)]).len(), 1, "{bad:?}");
        }
    }

    #[test]
    fn an_entity_record_carries_the_colliders_state() {
        // Sol's round 6: the same-to-same setter shows `enabled` and the position, not nulls.
        let same = |on: Option<bool>, pos: Option<V3>| {
            Ev::Set(SetRec { on, pos, ..set(SetFn::Entity, 7, "a") })
        };
        let mut m = started();
        assert!(run(&mut m, &[same(Some(true), Some(P))]).is_empty());
        for bad in [
            same(None, Some(P)),
            same(Some(true), None),
            same(Some(false), Some(P)),
            same(Some(true), Some(Q)),
        ] {
            assert!(!run(&mut m, &[bad]).is_empty());
        }
        // The creation's link needs both too, and `v`, `on0`, `pos0` null.
        let mut m = started();
        let create = |f: &dyn Fn(&mut SetRec<'_>)| {
            let mut r = set(SetFn::Entity, 8, "a");
            r.e0 = None;
            f(&mut r);
            r
        };
        let bad = run(&mut m, &[gone(7, "a"), Ev::Set(create(&|r| r.pos = None))]);
        assert!(!bad.is_empty());
        for f in [
            (&|r: &mut SetRec<'_>| r.v = Some("a")) as &dyn Fn(&mut SetRec<'_>),
            &|r| r.on0 = Some(true),
            &|r| r.pos0 = Some(P),
            &|r| r.on = None,
        ] {
            let mut m = started();
            assert!(!run(&mut m, &[gone(7, "a"), Ev::Set(create(f))]).is_empty());
        }
    }

    #[test]
    fn calls_and_writes_name_the_current_view_collider() {
        let mut m = started();
        // Not its entity's, not live, not made, wrong radius or enabled.
        let mut wrong_r = tick(7, "a");
        if let Ev::Tick { r0, .. } = &mut wrong_r {
            *r0 = Some(0.9);
        }
        let mut wrong_on = set(SetFn::Active, 7, "a");
        wrong_on.on0 = Some(false);
        let mut wrong_pos = set(SetFn::Pos, 7, "a");
        wrong_pos.pos0 = Some([9.0; 3]);
        let mut unknown = set(SetFn::Pos, 99, "a");
        unknown.c = Some(99);
        for (bad, why) in [
            (tick(7, "b"), "bound to"),
            (tick(99, "a"), "no record made"),
            (wrong_r, "radius before"),
            (Ev::Set(wrong_on), "`on0`"),
            (Ev::Set(wrong_pos), "`pos0`"),
            (Ev::Set(unknown), "no record made"),
        ] {
            let out = run(&mut m, &[bad]);
            assert!(out.iter().any(|x| x.contains(why)), "{why}: {out:?}");
        }
        // A call with no collider while the entity has one.
        let none = Ev::Tick {
            entity: "a",
            kind: Kind::Unit,
            kind_name: "UnitEntityData",
            made: false,
            c: None,
            r0: None,
            r: None,
            pos: None,
            on: None,
        };
        assert_eq!(run(&mut m, &[none]).len(), 1);
        let free = Ev::Tick {
            entity: "free",
            kind: Kind::Unit,
            kind_name: "UnitEntityData",
            made: false,
            c: None,
            r0: None,
            r: None,
            pos: None,
            on: None,
        };
        assert!(run(&mut m, &[free]).is_empty());
        // The writes and calls of the live collider update its state.
        let mut pos = set(SetFn::Pos, 7, "a");
        pos.on0 = Some(false);
        pos.on = Some(false);
        let out = run(&mut m, &[Ev::Set(set(SetFn::Active, 7, "a")), Ev::Set(pos)]);
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn a_collider_being_made_takes_nothing_until_its_call() {
        let mut m = started();
        run(&mut m, &[gone(7, "a"), entity(8, "a")]);
        // Linked, not made: each write finds it Linking (and the making interval open).
        for (bad, why) in [
            (Ev::Set(set(SetFn::Active, 8, "a")), "Linking"),
            (Ev::Set(set(SetFn::Pos, 8, "a")), "Linking"),
            (gone(8, "a"), "Linking"),
            (root(8, Kind::Unit, Some("a")), "owned already"),
        ] {
            let out = run(&mut m, &[bad]);
            assert!(out.iter().any(|x| x.contains(why)), "{why}: {out:?}");
        }
        // Two colliders for one entity at the start.
        let mut m = Lifecycle::default();
        let out = run(&mut m, &[root(1, Kind::Unit, Some("a")), root(2, Kind::Unit, Some("a"))]);
        assert_eq!(out.len(), 1, "{out:?}");
    }

    #[test]
    fn an_entity_keeps_its_type_in_the_records() {
        let mut m = started();
        let mut call = making(8, "a", Kind::Other);
        if let Ev::Tick { kind_name, .. } = &mut call {
            *kind_name = "MapObjectEntityData";
        }
        let bad = run(&mut m, &[gone(7, "a"), entity(8, "a"), call]);
        assert!(bad.iter().any(|x| x.contains("is a UnitEntityData and a")), "{bad:?}");
    }

    #[test]
    fn the_intrinsic_constraints_hold_whatever_the_sim_knows() {
        // Sol's round 7: a unit's collider can't shrink under 0.05 (ScaleRadius), a map object's
        // making keeps the transform its link showed, a pos write moves by more than Unity's
        // `Vector3 ==` tolerance, and a call with no collider still names the entity's type.
        let mut m = started();
        let mut small = tick(7, "a");
        if let Ev::Tick { r, .. } = &mut small {
            *r = Some(0.01);
        }
        let out = run(&mut m, &[small]);
        assert!(out.iter().any(|x| x.contains("under the 0.05 minimum")), "{out:?}");
        let mut m = started();
        let mut moved = making(60, "obj2", Kind::Other);
        if let Ev::Tick { pos, .. } = &mut moved {
            *pos = Some(Q);
        }
        let out = run(&mut m, &[entity(60, "obj2"), moved]);
        assert!(out.iter().any(|x| x.contains("was linked at")), "{out:?}");
        // (A unit's making may move it: the sim's rule, in `Bounds`.)
        let mut m = started();
        let mut moved = making(8, "a", Kind::Unit);
        if let Ev::Tick { pos, .. } = &mut moved {
            *pos = Some(Q);
        }
        let out = run(&mut m, &[gone(7, "a"), entity(8, "a"), moved]);
        assert!(out.is_empty(), "{out:?}");
        // A move under the tolerance isn't a move.
        let mut m = started();
        let mut r = set(SetFn::Pos, 7, "a");
        r.pos = Some([P[0] + 1e-7, P[1], P[2]]);
        let out = run(&mut m, &[Ev::Set(r)]);
        assert!(out.iter().any(|x| x.contains("a position that changed")), "{out:?}");
        // A call with no collider, a type that differs from the entity's root.
        let mut m = started();
        let none = Ev::Tick {
            entity: "obj",
            kind: Kind::Other,
            kind_name: "ScriptZoneData",
            made: false,
            c: None,
            r0: None,
            r: None,
            pos: None,
            on: None,
        };
        let out = run(&mut m, &[none]);
        assert!(
            out.iter().any(|x| x.contains("is a MapObjectEntityData and a ScriptZoneData")),
            "{out:?}"
        );
    }
}
