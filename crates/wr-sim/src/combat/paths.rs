//! Turn-based paths (`docs/poc5/paths.md`): the grid as `CombatController` blocks it in
//! turn-based combat, and the movement agent's path requests on it.
//!
//! - [`CombatNav`]: the area's graphs as A* holds them during the fight. At each
//!   `StartTurn` every unit in the turn order but the acting one (and its mount) gets a
//!   `GraphUpdateObject` around its position: an enemy of the acting unit blocks the cells
//!   (`DynamicBlocked`, tag 31), an ally marks them (`AllyBlocked`, its box grown by the
//!   acting unit's corpulence). The previous turn's objects are reverted first. All are
//!   processed at `UpdateNavigationGridTags`' flush with `erodeIterations` 0, then the whole
//!   grid is eroded again with the acting unit's size in cells (tag erosion). A unit leaving
//!   the order reverts its object when `AstarPath` next processes its work items.
//! - [`construct_tags`], [`seeker_clearance`]: what `UnitMovementAgent.PathTo`/`FindPath`
//!   set on the seeker before `StartPathWithClerance`.
//!
//! The search, the ending conditions and the seeker's modifiers are `wr-nav`'s
//! ([`wr_nav::world::World::turn_based_path`]); line-of-sight answers
//! (`LineOfSightGeometry.HasObstacle`) are an input.

use std::collections::HashMap;

use anyhow::{Result, ensure};
use wr_nav::astar::Ending;
use wr_nav::cs::{V3, d, r};
use wr_nav::query::TurnBasedRequest;
use wr_nav::update::{Bounds, GraphUpdate};
use wr_nav::world::World;

/// A unit in `CombatController.m_Units`, as `PrepareUnitNavmeshUpdateObject` reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct OrderUnit {
    pub id: String,
    /// `unit.Position`.
    pub position: V3,
    pub corpulence: f32,
    /// `unit.View.AgentASP.AvoidanceDisabled`.
    pub avoidance_disabled: bool,
    /// `unit.IsEnemy(CurrentTurn.Rider)`.
    pub enemy_of_current: bool,
}

/// The turn `StartTurn` begins (`CurrentTurn`, already replaced when the units are set up).
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    /// The rider (the unit whose turn it is).
    pub unit: String,
    /// `unit.RiderPart?.SaddledUnit`.
    pub mount: Option<String>,
    /// `CurrentTurn.Mount?.Corpulence ?? CurrentTurn.Rider.Corpulence`.
    pub corpulence: f32,
    /// The same unit's agent's `AvoidanceDisabled`.
    pub avoidance_disabled: bool,
    /// The rider's agent's `AvoidanceDisabled` (`StartTurn`'s `avoidanceDisabled`).
    pub rider_avoidance_disabled: bool,
    /// The rider's `View.GetSizeInCells(grid)`.
    pub rider_size_in_cells: i32,
    /// `Rider.IsInCombat && !Rider.State.IsDead` at `UpdateNavigationGridTags`.
    pub rider_in_combat_alive: bool,
}

/// `UnitEntityView.GetSizeInCells(grid)`: `Mathf.RoundToInt(Corpulence / grid.nodeSize)`.
#[must_use]
pub fn size_in_cells(corpulence: f32, node_size: f32) -> i32 {
    // The quotient rounded to float as the argument, then Math.Round (ties to even).
    d(r(d(corpulence) / d(node_size))).round_ties_even() as i32
}

/// What happened to the grid, in order (WrathOracle's observer `paths` records the same:
/// `nav_guo`, `nav_update`, `nav_tags`).
#[derive(Debug, Clone, PartialEq)]
pub enum NavEvent {
    /// `AstarPath.UpdateGraphs(o)`: the object (`id`: one per object; a revert queues the
    /// same object again) as queued.
    Queued { id: u32, update: GraphUpdate },
    /// `GridGraph.UpdateArea(o)`, eroding with `erosion`.
    Applied { id: u32, erosion: i32 },
    /// `CombatController.UpdateNavigationGridTags` done.
    Tags { erode: i32, only: u32, hash: u64, dynamic: usize, ally: usize },
}

/// The grid during a turn-based fight, and `CombatController`'s bookkeeping of it.
#[derive(Debug, Clone)]
pub struct CombatNav {
    pub world: World,
    /// `TBUnitInfo.LastNavmeshUpdateObject` by unit (with its object id).
    last: HashMap<String, (u32, GraphUpdate)>,
    /// `AstarPath`'s queued graph updates, not processed yet.
    pending: Vec<(u32, GraphUpdate)>,
    next_id: u32,
    /// Every queue, application and tag refresh, in order.
    pub log: Vec<NavEvent>,
}

impl CombatNav {
    /// # Errors
    ///
    /// A grid whose areas this port doesn't model (one-way connections).
    pub fn new(world: World) -> Result<Self> {
        world.check_two_way_grid()?;
        Ok(Self { world, last: HashMap::new(), pending: Vec::new(), next_id: 0, log: Vec::new() })
    }

    fn queue(&mut self, id: u32, o: GraphUpdate) {
        self.log.push(NavEvent::Queued { id, update: o });
        self.pending.push((id, o));
    }

    /// Whether graph updates wait for `AstarPath` to process them.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// The units with an applied object.
    pub fn blocking(&self) -> impl Iterator<Item = &str> {
        self.last.keys().map(String::as_str)
    }

    /// `CombatController.RevertNavmeshForUnit`: the unit's object, flipped, queued again.
    pub fn revert(&mut self, unit: &str) {
        if let Some((id, mut o)) = self.last.remove(unit) {
            o.set_dynamic_walkability = !o.set_dynamic_walkability;
            o.set_ally_blocked = !o.set_ally_blocked;
            if o.set_tag > 0 {
                o.set_tag = 0;
            }
            self.queue(id, o);
        }
    }

    /// `CombatController.PrepareUnitNavmeshUpdateObject` (with `CurrentTurn` set).
    fn prepare(u: &OrderUnit, turn: &Turn) -> GraphUpdate {
        let enemy = u.enemy_of_current;
        let grow = if enemy || turn.avoidance_disabled { 0.0 } else { turn.corpulence };
        let side = r(d(u.corpulence) * 2.0 + d(grow));
        let size = if u.avoidance_disabled { [0.0; 3] } else { [side, 10.0, side] };
        let mut o = GraphUpdate::new(Bounds::new(u.position, size));
        o.graph_mask = 2;
        o.update_physics = false;
        if enemy {
            o.modify_dynamic_walkability = true;
            o.set_dynamic_walkability = false;
            o.modify_ally_blocked = false;
            o.modify_tag = true;
            o.set_tag = 31;
        } else {
            o.modify_dynamic_walkability = false;
            o.modify_ally_blocked = true;
            o.set_ally_blocked = true;
            o.modify_tag = false;
        }
        o
    }

    /// `CombatController.StartTurn`'s graph part: every unit's object reverted and, but for
    /// the acting unit and its mount (and none when the acting unit's avoidance is off),
    /// prepared again and queued; then [`Self::update_navigation_grid_tags`].
    ///
    /// # Errors
    ///
    /// A graph update this port doesn't model.
    pub fn start_turn(&mut self, units: &[OrderUnit], turn: &Turn) -> Result<()> {
        for u in units {
            self.revert(&u.id);
            if u.id != turn.unit
                && Some(&u.id) != turn.mount.as_ref()
                && !turn.rider_avoidance_disabled
            {
                let o = Self::prepare(u, turn);
                self.next_id += 1;
                let id = self.next_id;
                self.queue(id, o);
                self.last.insert(u.id.clone(), (id, o));
            }
        }
        self.update_navigation_grid_tags(Some(turn))
    }

    /// `CombatController.UpdateNavigationGridTags`: `erodeIterations` 0, the queued updates
    /// flushed, then the whole grid eroded with the acting rider's size in cells (when there
    /// is a turn whose rider is in combat and alive).
    ///
    /// # Errors
    ///
    /// A graph update this port doesn't model.
    pub fn update_navigation_grid_tags(&mut self, turn: Option<&Turn>) -> Result<()> {
        let g = self.world.grid.as_mut().ok_or_else(|| anyhow::anyhow!("no grid graph"))?;
        g.erode_iterations = 0;
        self.process_pending()?;
        if let Some(t) = turn
            && t.rider_in_combat_alive
        {
            let g = self.world.grid.as_mut().expect("grid");
            g.erode_iterations = t.rider_size_in_cells;
            let (w, dp) = (g.width, g.depth);
            g.erode_walkable_area(0, 0, w, dp)?;
        }
        let g = self.world.grid.as_ref().expect("grid");
        let (dynamic, ally) = g.blocked_counts();
        self.log.push(NavEvent::Tags {
            erode: g.erode_iterations,
            only: g.erosion_only_around_cells_with_tag,
            hash: g.state_hash(),
            dynamic,
            ally,
        });
        Ok(())
    }

    /// `AstarPath` processing its queued graph updates (a flush, or its `Update` once the
    /// pathfinding threads pause): each with the grid's current `erodeIterations`.
    ///
    /// # Errors
    ///
    /// A graph update this port doesn't model.
    pub fn process_pending(&mut self) -> Result<()> {
        let queued = std::mem::take(&mut self.pending);
        let erode = self.erode_iterations();
        let gi = self.world.grid.as_ref().map_or(0, |g| g.graph_index);
        for (id, o) in &queued {
            if (o.graph_mask >> gi) & 1 != 0 {
                let erosion = if o.update_erosion { erode } else { 0 };
                self.log.push(NavEvent::Applied { id: *id, erosion });
            }
        }
        let updates: Vec<GraphUpdate> = queued.into_iter().map(|(_, o)| o).collect();
        self.world.apply_graph_updates(&updates)
    }

    /// `GridGraph.erodeIterations` now.
    #[must_use]
    pub fn erode_iterations(&self) -> i32 {
        self.world.grid.as_ref().map_or(0, |g| g.erode_iterations)
    }

    /// `UnitMovementAgent.ConstructTags` in turn-based combat for a unit of
    /// `size_in_cells` (`None`: the agent has no unit, size 1).
    #[must_use]
    pub fn construct_tags(&self, size_in_cells: Option<i32>) -> i32 {
        let Some(g) = &self.world.grid else { return -1 };
        let mut num = size_in_cells.unwrap_or(1).max(1);
        if num > g.erode_iterations {
            num = g.erode_iterations;
        }
        let mut mask = i32::MAX;
        for i in 0..num - 1 {
            mask &= !(1 << (i + g.erosion_first_tag));
        }
        mask
    }
}

/// The seeker's clearance as `PathTo` (`avoidance_disabled` ignored) or `FindPath` (the
/// agent's own `m_AvoidanceDisabled` field) sets it: `(corpulence * 2, 0, corpulence * 2)`.
#[must_use]
pub fn seeker_clearance(corpulence: f32, find: bool, avoidance_disabled_field: bool) -> V3 {
    if find && avoidance_disabled_field {
        return [0.0; 3];
    }
    let c = r(d(corpulence) * 2.0);
    [c, 0.0, c]
}

/// The maximum distance `PathTo` and `FindPath` give `StartPathWithClerance`:
/// `6f * Unit.Data.CurrentSpeedMps` (the product rounded as the argument), or
/// `float.MaxValue` for an agent without a unit.
#[must_use]
pub fn max_distance(current_speed_mps: Option<f32>) -> f32 {
    current_speed_mps.map_or(f32::MAX, |v| r(6.0 * d(v)))
}

/// A turn-based request of the movement agent (`UnitMovementAgent.PathTo` or `FindPath`
/// while `CombatController.IsInTurnBasedCombat()`), before `ABPathWithClearance.Construct`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AgentRequest {
    /// `FindPath` (else `PathTo`).
    pub find: bool,
    /// `transform.position`.
    pub start: V3,
    pub destination: V3,
    /// `Unit.Corpulence`.
    pub corpulence: f32,
    /// The agent's `m_AvoidanceDisabled` field (`FindPath`'s clearance).
    pub avoidance_disabled_field: bool,
    /// `PathTo`'s `approachRadius`; `FindPath`'s optional one.
    pub approach_radius: Option<f32>,
    /// `PathTo`: `destinationAgent != null`; `FindPath`: `needLOS`.
    pub need_los: bool,
    /// `command?.GetTargetLOSObjectFudgeRadius() ?? 0f`, or `FindPath`'s argument.
    pub ignore_blocker_radius: f32,
    /// [`max_distance`].
    pub max_distance: f32,
    /// `PathTo`'s `distanceXZ` (set on the request's constraint after `StartPath`, so a worker
    /// may already have used it unset); `FindPath`: false.
    pub distance_xz: bool,
}

impl AgentRequest {
    /// The path request: the seeker's clearance and `ConstructTags()` on the grid as it is
    /// now, graph mask 2, the ending condition `Construct` picks (`FindPath` clamps a
    /// negative approach radius to 0; `PathTo` always passes one).
    ///
    /// # Errors
    ///
    /// `PathTo` without an approach radius.
    pub fn to_request(&self, nav: &CombatNav, node_size: f32) -> Result<TurnBasedRequest> {
        let mut ar = self.approach_radius;
        if self.find {
            ar = ar.map(|a| if a < 0.0 { 0.0 } else { a });
        } else {
            ensure!(ar.is_some(), "PathTo passes an approach radius");
        }
        let ending = match ar {
            Some(a) => Ending::Los {
                approach_radius: a,
                check_los: self.need_los,
                ignored_blocker_radius: self.ignore_blocker_radius,
                max: self.max_distance,
            },
            None => Ending::MaxDistance { max: self.max_distance },
        };
        Ok(TurnBasedRequest {
            start: self.start,
            end: self.destination,
            clearance: seeker_clearance(self.corpulence, self.find, self.avoidance_disabled_field),
            enabled_tags: nav.construct_tags(Some(size_in_cells(self.corpulence, node_size))),
            graph_mask: 2,
            ending,
            distance_xz: self.distance_xz && !self.find,
        })
    }
}

#[cfg(test)]
mod tests;
