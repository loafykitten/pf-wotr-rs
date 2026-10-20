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
    entity: Option<String>,
    on: bool,
    pos: V3,
    radius: f32,
    /// The `enabled` the creation's `entity` record showed: the making call's must equal it.
    link_on: Option<bool>,
}

/// A record, as the machine takes it.
#[derive(Debug, Clone, Copy)]
pub(super) enum Ev<'a> {
    /// A `bounds_init` root.
    Root {
        id: i32,
        kind: Kind,
        entity: Option<&'a str>,
        on: bool,
        radius: f32,
        pos: V3,
    },
    /// `EntityDataLink.Entity`'s setter (`bounds_set` `entity`).
    Entity {
        c: i32,
        e0: Option<&'a str>,
        e: Option<&'a str>,
        on: Option<bool>,
        pos: Option<V3>,
    },
    /// A `bounds_tick`.
    Tick {
        entity: &'a str,
        kind: Kind,
        made: bool,
        c: Option<i32>,
        r0: Option<f32>,
        r: Option<f32>,
        pos: Option<V3>,
        on: Option<bool>,
    },
    Active {
        c: i32,
        on0: bool,
        on: bool,
    },
    Pos {
        c: i32,
        pos0: V3,
        pos: V3,
    },
    Gone {
        c: i32,
    },
    /// `SetupSphereColliderEntityLink` (`bounds_set` `link`).
    Link {
        c: Option<i32>,
        v: Option<&'a str>,
        e0: Option<&'a str>,
        e: Option<&'a str>,
        on0: Option<bool>,
        on: Option<bool>,
        pos0: Option<V3>,
        pos: Option<V3>,
    },
}

fn bits(v: V3) -> [u32; 3] {
    v.map(f32::to_bits)
}

/// The colliders' lives.
#[derive(Debug, Default)]
pub(super) struct Lifecycle {
    cols: HashMap<i32, Life>,
    current: HashMap<String, i32>,
}

impl Lifecycle {
    /// The kind of a collider that has been made (`None`: unknown, or not made yet).
    pub(super) fn kind_of(&self, c: i32) -> Option<Kind> {
        self.cols.get(&c).and_then(|l| l.kind)
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
    #[allow(clippy::too_many_lines)]
    pub(super) fn apply(&mut self, ev: Ev<'_>, out: &mut Vec<String>) {
        match ev {
            Ev::Root { id, kind, entity, on, radius, pos } => {
                if self.cols.contains_key(&id) {
                    out.push(format!("the root {id} names an instance id that is owned already"));
                    return;
                }
                if let Some(e) = entity {
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
                        entity: entity.map(str::to_owned),
                        on,
                        pos,
                        radius,
                        link_on: None,
                    },
                );
            }
            Ev::Entity { c, e0, e, on, pos } => match (self.cols.get(&c), e0, e) {
                (None, None, Some(e)) => {
                    // The creation's link: the previous view collider is cleared first.
                    if let Some(cur) = self.current.get(e) {
                        out.push(format!(
                            "collider {c} is linked to {e} while collider {cur} is its view collider (the previous one is destroyed first)"
                        ));
                    }
                    self.current.insert(e.to_owned(), c);
                    self.cols.insert(
                        c,
                        Life {
                            state: State::Linking,
                            kind: None,
                            entity: Some(e.to_owned()),
                            on: false,
                            pos: [0.0; 3],
                            radius: 0.0,
                            link_on: on,
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
                    if on.is_some_and(|x| x != l.on) || pos.is_some_and(|p| bits(p) != bits(l.pos))
                    {
                        out.push(format!(
                            "an entity record of collider {c} shows enabled {on:?} at {pos:?}, the collider is {} at {:?}",
                            l.on, l.pos
                        ));
                    }
                }
                (Some(l), _, _) => out.push(format!(
                    "an entity record of collider {c} ({:?}, bound to {:?}) from {e0:?} to {e:?}: a second link, or an unlink or relink, not modelled",
                    l.state, l.entity
                )),
            },
            Ev::Tick { entity, kind, made, c, r0, r, pos, on } => {
                let (Some(c), Some(r), Some(pos), Some(on)) = (c, r, pos, on) else {
                    // No collider after the call (the entity has no view, or the view has none).
                    if made || c.is_some() || r0.is_some() || r.is_some() || pos.is_some() || on.is_some()
                    {
                        out.push(format!("a bounds_tick of {entity} with a collider in part"));
                    } else if let Some(cur) = self.current.get(entity) {
                        out.push(format!(
                            "a bounds_tick of {entity} without a collider, but collider {cur} is its view collider"
                        ));
                    }
                    return;
                };
                if made {
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
                    if l.link_on.is_some_and(|x| x != on) {
                        out.push(format!(
                            "collider {c} was linked enabled {:?} and its making call shows {on}",
                            l.link_on
                        ));
                    }
                    (l.state, l.kind, l.on, l.pos, l.radius) = (State::Live, Some(kind), on, pos, r);
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
                    out.push(format!("a bounds_tick of collider {c} shows enabled {on}, the collider is {}", l.on));
                }
                if let Some(l) = self.cols.get_mut(&c) {
                    (l.radius, l.pos, l.on) = (r, pos, on);
                }
            }
            Ev::Active { c, on0, on } => {
                if self.view_collider(c, "bounds_set active", out).is_some() {
                    let Some(l) = self.cols.get_mut(&c) else { return };
                    if l.on != on0 {
                        out.push(format!(
                            "bounds_set active of collider {c} says it was enabled {on0}, it is {}",
                            l.on
                        ));
                    }
                    l.on = on;
                }
            }
            Ev::Pos { c, pos0, pos } => {
                if self.view_collider(c, "bounds_set pos", out).is_some() {
                    let Some(l) = self.cols.get_mut(&c) else { return };
                    if bits(l.pos) != bits(pos0) {
                        out.push(format!(
                            "bounds_set pos of collider {c} says it was at {pos0:?}, it is at {:?}",
                            l.pos
                        ));
                    }
                    l.pos = pos;
                }
            }
            Ev::Gone { c } => {
                if self.view_collider(c, "bounds_set gone", out).is_some() {
                    let Some(l) = self.cols.get_mut(&c) else { return };
                    l.state = State::Dying;
                    if let Some(e) = l.entity.clone() {
                        self.current.remove(&e);
                    }
                }
            }
            Ev::Link { c: None, v, e0, e, .. } => {
                // The view has no collider (a view attached or detached: the records can't tell
                // which): nothing changes, and nothing is permitted by it.
                if e0.is_some() || e.is_some() {
                    out.push(format!("a link record with no collider from {e0:?} to {e:?}"));
                }
                if let Some(cur) = v.and_then(|v| self.current.get(v)) {
                    out.push(format!(
                        "a link record says {v:?}'s view has no collider, but collider {cur} is its view collider"
                    ));
                }
            }
            Ev::Link { c: Some(c), e0, e, on0, on, pos0, pos, .. } => {
                let Some(l) = self.view_collider(c, "a link record", out) else { return };
                if e0 != e || e.is_none() || l.entity.as_deref() != e {
                    out.push(format!(
                        "a link record of collider {c} from {e0:?} to {e:?}: the collider is bound to {:?} (an unlink or relink, not modelled)",
                        l.entity
                    ));
                    return;
                }
                // (A relink to what it holds: the collider's whole state is the record's.)
                let (on_l, pos_l) = (Some(l.on), Some(bits(l.pos)));
                if on0 != on_l || on != on_l || pos0.map(bits) != pos_l || pos.map(bits) != pos_l {
                    out.push(format!(
                        "a link record of collider {c} shows enabled {on0:?}/{on:?} at {pos0:?}/{pos:?}, the collider is {} at {:?}",
                        l.on, l.pos
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: V3 = [1.0, 2.0, 3.0];

    fn run(m: &mut Lifecycle, evs: &[Ev<'_>]) -> Vec<String> {
        let mut out = Vec::new();
        for e in evs {
            m.apply(*e, &mut out);
        }
        out
    }

    fn root(id: i32, kind: Kind, entity: Option<&str>) -> Ev<'_> {
        Ev::Root { id, kind, entity, on: true, radius: 0.5, pos: P }
    }

    fn entity<'a>(c: i32, e: &'a str) -> Ev<'a> {
        Ev::Entity { c, e0: None, e: Some(e), on: Some(true), pos: Some([0.0; 3]) }
    }

    fn making<'a>(c: i32, e: &'a str, kind: Kind) -> Ev<'a> {
        Ev::Tick {
            entity: e,
            kind,
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
        let ok = run(
            &mut m,
            &[Ev::Gone { c: 7 }, entity(8, "a"), making(8, "a", Kind::Unit), tick(8, "a")],
        );
        assert!(ok.is_empty(), "{ok:?}");
        // The destroyed one takes no view operation this frame, and is gone after it.
        let bad = run(&mut m, &[Ev::Pos { c: 7, pos0: P, pos: P }, tick(7, "a")]);
        assert_eq!(bad.len(), 2, "{bad:?}");
        let mut end = Vec::new();
        m.frame(&mut end);
        assert!(end.is_empty());
    }

    #[test]
    fn the_creation_needs_the_previous_collider_destroyed_before_its_link() {
        // Sol's round 5: `entity(c9), gone(c7), making(c9)` can't be written by the game.
        let mut m = started();
        let bad = run(&mut m, &[entity(9, "a"), Ev::Gone { c: 7 }, making(9, "a", Kind::Unit)]);
        assert!(bad.iter().any(|x| x.contains("is its view collider")), "{bad:?}");
        // A making with no link, a link with no making, a second link.
        let mut m = started();
        let bad = run(&mut m, &[Ev::Gone { c: 7 }, making(8, "a", Kind::Unit)]);
        assert!(bad.iter().any(|x| x.contains("no entity record linked")), "{bad:?}");
        let mut m = started();
        let mut end = Vec::new();
        let bad = run(&mut m, &[Ev::Gone { c: 7 }, entity(8, "a")]);
        assert!(bad.is_empty());
        m.frame(&mut end);
        assert!(end.iter().any(|x| x.contains("no call made it")), "{end:?}");
        let mut m = started();
        let bad = run(
            &mut m,
            &[Ev::Gone { c: 7 }, entity(8, "a"), entity(8, "a"), making(8, "a", Kind::Unit)],
        );
        assert!(bad.iter().any(|x| x.contains("a second link")), "{bad:?}");
        // The link of the making and the call must agree on the entity and on `enabled`.
        let mut m = started();
        let bad = run(&mut m, &[Ev::Gone { c: 7 }, entity(8, "b"), making(8, "a", Kind::Unit)]);
        assert!(!bad.is_empty());
        let mut m = started();
        let mut call = making(8, "a", Kind::Unit);
        if let Ev::Tick { on, .. } = &mut call {
            *on = Some(false);
        }
        let bad = run(&mut m, &[Ev::Gone { c: 7 }, entity(8, "a"), call]);
        assert!(bad.iter().any(|x| x.contains("was linked enabled")), "{bad:?}");
    }

    #[test]
    fn an_instance_id_has_one_owner_for_good_across_kinds() {
        // Sol's round 5: a unit's making can't take a live map object's id; nor a gone one's.
        let mut m = started();
        let bad = run(&mut m, &[Ev::Gone { c: 7 }, entity(50, "a")]);
        assert!(
            bad.iter().any(|x| x.contains("no record made it") || x.contains("second link")),
            "{bad:?}"
        );
        let mut m = started();
        let bad = run(&mut m, &[root(50, Kind::Unit, Some("z"))]);
        assert!(bad.iter().any(|x| x.contains("owned already")), "{bad:?}");
        let mut m = started();
        run(&mut m, &[Ev::Gone { c: 7 }]);
        let mut end = Vec::new();
        m.frame(&mut end);
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
    fn a_link_record_is_checked_against_the_colliders_whole_state() {
        let link = |on0, on, pos0, pos| Ev::Link {
            c: Some(7),
            v: Some("a"),
            e0: Some("a"),
            e: Some("a"),
            on0,
            on,
            pos0,
            pos,
        };
        let mut m = started();
        assert!(run(&mut m, &[link(Some(true), Some(true), Some(P), Some(P))]).is_empty());
        // Sol's round 5: a disabled collider at [99,0,0] claimed for an enabled one at P.
        for bad in [
            link(Some(false), Some(false), Some([99.0, 0.0, 0.0]), Some([99.0, 0.0, 0.0])),
            link(Some(true), Some(true), Some(P), Some([99.0, 0.0, 0.0])),
            link(Some(true), Some(false), Some(P), Some(P)),
            link(None, None, None, None),
        ] {
            let out = run(&mut m, &[bad]);
            assert_eq!(out.len(), 1, "{out:?}");
        }
        // A relink or unlink is not modelled; a null-collider link says the view has none.
        let unlink = Ev::Link {
            c: Some(7),
            v: Some("a"),
            e0: Some("a"),
            e: None,
            on0: Some(true),
            on: Some(true),
            pos0: Some(P),
            pos: Some(P),
        };
        assert_eq!(run(&mut m, &[unlink]).len(), 1);
        let none = Ev::Link {
            c: None,
            v: Some("a"),
            e0: None,
            e: None,
            on0: None,
            on: None,
            pos0: None,
            pos: None,
        };
        assert_eq!(run(&mut m, &[none]).len(), 1);
        let other = Ev::Link {
            c: None,
            v: Some("nobody"),
            e0: None,
            e: None,
            on0: None,
            on: None,
            pos0: None,
            pos: None,
        };
        assert!(run(&mut m, &[other]).is_empty());
    }

    #[test]
    fn calls_and_writes_name_the_current_view_collider() {
        let mut m = started();
        // Not its entity's, not live, not made, wrong radius or enabled.
        let mut wrong_r = tick(7, "a");
        if let Ev::Tick { r0, .. } = &mut wrong_r {
            *r0 = Some(0.9);
        }
        for (bad, why) in [
            (tick(7, "b"), "bound to"),
            (tick(99, "a"), "no record made"),
            (wrong_r, "radius before"),
            (Ev::Active { c: 7, on0: false, on: true }, "says it was enabled"),
            (Ev::Pos { c: 7, pos0: [9.0; 3], pos: P }, "says it was at"),
            (Ev::Pos { c: 99, pos0: P, pos: P }, "no record made"),
        ] {
            let out = run(&mut m, &[bad]);
            assert!(out.iter().any(|x| x.contains(why)), "{why}: {out:?}");
        }
        // A call with no collider while the entity has one.
        let none = Ev::Tick {
            entity: "a",
            kind: Kind::Unit,
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
            made: false,
            c: None,
            r0: None,
            r: None,
            pos: None,
            on: None,
        };
        assert!(run(&mut m, &[free]).is_empty());
        // The writes and calls of the live collider update its state.
        assert!(
            run(
                &mut m,
                &[
                    Ev::Active { c: 7, on0: true, on: false },
                    Ev::Pos { c: 7, pos0: P, pos: [5.0; 3] },
                ]
            )
            .is_empty()
        );
        let mut off = tick(7, "a");
        if let Ev::Tick { on, pos, .. } = &mut off {
            (*on, *pos) = (Some(false), Some([5.0; 3]));
        }
        assert!(run(&mut m, &[off]).is_empty());
    }

    #[test]
    fn a_collider_being_made_takes_nothing_until_its_call() {
        let mut m = started();
        run(&mut m, &[Ev::Gone { c: 7 }, entity(8, "a")]);
        // Linked, not made: a write, a call without a collider, a link, a second root all find it.
        let none = Ev::Tick {
            entity: "a",
            kind: Kind::Unit,
            made: false,
            c: None,
            r0: None,
            r: None,
            pos: None,
            on: None,
        };
        for (bad, why) in [
            (Ev::Active { c: 8, on0: true, on: false }, "Linking"),
            (Ev::Pos { c: 8, pos0: P, pos: P }, "Linking"),
            (Ev::Gone { c: 8 }, "Linking"),
            (none, "is its view collider"),
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
}
