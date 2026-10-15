//! Graph updates on the grid, as A* Pathfinding Project 4.3.10 applies them with Owlcat's
//! additions: `GraphUpdateObject` (`Apply` with `DynamicBlocked`/`AllyBlocked`),
//! `GridGraph.UpdateArea` (both erosion branches), `CalculateConnections`,
//! `ErodeWalkableArea` (tag erosion, `erosionOnlyAroundCellsWithTag`) and the connected
//! components the work item processor recomputes afterwards (`HierarchicalGraph`, compared
//! for equality only). Turn-based combat drives these through `CombatController`, which
//! blocks the grid around the units who aren't acting (see `wr-sim`'s `combat::paths`).
//!
//! Unity's `Bounds` is managed except `Contains`, which is the player's native
//! `BoundsScripting::Contains` (checked against it in `native_check`).

use anyhow::{Result, bail, ensure};

use super::cs::{self, V3, d};
use super::world::{Grid, World};
use crate::{FLAG_TAG_MASK, FLAG_TAG_SHIFT, FLAG_WALKABLE};

/// Unity's `Bounds` (`m_Center`, `m_Extents`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub center: V3,
    pub extents: V3,
}

impl Bounds {
    /// `new Bounds(center, size)`: `m_Extents = size * 0.5f`.
    #[must_use]
    pub fn new(center: V3, size: V3) -> Self {
        Self { center, extents: cs::mul(size, 0.5) }
    }

    /// `Bounds.min`: `center - extents`.
    #[must_use]
    pub fn min(&self) -> V3 {
        cs::sub(self.center, self.extents)
    }

    /// `Bounds.max`: `center + extents`.
    #[must_use]
    pub fn max(&self) -> V3 {
        cs::add(self.center, self.extents)
    }

    /// `Bounds.SetMinMax(min, max)`: `extents = (max - min) * 0.5f; center = min + extents`.
    pub fn set_min_max(&mut self, min: V3, max: V3) {
        self.extents = cs::mul(cs::sub(max, min), 0.5);
        self.center = cs::add(min, self.extents);
    }

    /// `Bounds.min = value` (`SetMinMax(value, max)`).
    pub fn set_min(&mut self, min: V3) {
        let max = self.max();
        self.set_min_max(min, max);
    }

    /// `Bounds.max = value` (`SetMinMax(min, value)`).
    pub fn set_max(&mut self, max: V3) {
        let min = self.min();
        self.set_min_max(min, max);
    }

    /// `Bounds.Contains`: the player's `BoundsScripting::Contains`, per axis in single
    /// precision `!(p < c - e) && !(p > c + e)` (a NaN coordinate counts as inside).
    #[must_use]
    #[expect(clippy::neg_cmp_op_on_partial_ord, reason = "the player's comparisons (NaN inside)")]
    pub fn contains(&self, p: V3) -> bool {
        (0..3).all(|i| {
            let lo = self.center[i] - self.extents[i];
            let hi = self.center[i] + self.extents[i];
            !(p[i] < lo) && !(p[i] > hi)
        })
    }
}

/// `IntRect` (`ymin`/`ymax` are grid z).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntRect {
    pub xmin: i32,
    pub ymin: i32,
    pub xmax: i32,
    pub ymax: i32,
}

impl IntRect {
    #[must_use]
    pub fn new(xmin: i32, ymin: i32, xmax: i32, ymax: i32) -> Self {
        Self { xmin, ymin, xmax, ymax }
    }

    #[must_use]
    pub fn intersection(a: Self, b: Self) -> Self {
        Self::new(a.xmin.max(b.xmin), a.ymin.max(b.ymin), a.xmax.min(b.xmax), a.ymax.min(b.ymax))
    }

    #[must_use]
    pub fn union(a: Self, b: Self) -> Self {
        Self::new(a.xmin.min(b.xmin), a.ymin.min(b.ymin), a.xmax.max(b.xmax), a.ymax.max(b.ymax))
    }

    #[must_use]
    pub fn expand(self, n: i32) -> Self {
        Self::new(self.xmin - n, self.ymin - n, self.xmax + n, self.ymax + n)
    }

    #[must_use]
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.xmin && y >= self.ymin && x <= self.xmax && y <= self.ymax
    }

    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.xmin <= self.xmax && self.ymin <= self.ymax
    }
}

/// A `GraphUpdateObject` with the fields `CombatController` sets (`shape` null,
/// `addPenalty` 0, no change tracking).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GraphUpdate {
    pub bounds: Bounds,
    /// `nnConstraint.graphMask` (`SuitableGraph`).
    pub graph_mask: i32,
    pub update_physics: bool,
    pub update_erosion: bool,
    pub modify_walkability: bool,
    pub set_walkability: bool,
    pub modify_dynamic_walkability: bool,
    pub set_dynamic_walkability: bool,
    pub modify_ally_blocked: bool,
    pub set_ally_blocked: bool,
    pub modify_tag: bool,
    pub set_tag: u32,
}

impl GraphUpdate {
    /// `new GraphUpdateObject { bounds }` with A*'s defaults.
    #[must_use]
    pub fn new(bounds: Bounds) -> Self {
        Self {
            bounds,
            graph_mask: -1,
            update_physics: true,
            update_erosion: true,
            modify_walkability: false,
            set_walkability: false,
            modify_dynamic_walkability: false,
            set_dynamic_walkability: false,
            modify_ally_blocked: false,
            set_ally_blocked: false,
            modify_tag: false,
            set_tag: 0,
        }
    }
}

/// `Mathf.RoundToInt(f)`: `(int)Math.Round(f)`, ties to even.
fn round_to_int(f: f32) -> i32 {
    cs::round_to_i32(d(f))
}

impl Grid {
    fn cell_xz(&self, cell: usize) -> (i32, i32) {
        (cell as i32 % self.width, cell as i32 / self.width)
    }

    fn whole(&self) -> IntRect {
        IntRect::new(0, 0, self.width - 1, self.depth - 1)
    }

    fn ensure_dynamic(&mut self) {
        let n = self.positions.len();
        if self.dynamic_blocked.len() != n {
            self.dynamic_blocked.resize(n, 0);
        }
        if self.ally_blocked.len() != n {
            self.ally_blocked.resize(n, 0);
        }
    }

    /// `GraphNode.Walkable = value`: the walkable flag (`AddDirtyNode` is the caller's
    /// area recomputation).
    fn set_walkable(&mut self, cell: usize, v: bool) {
        self.flags[cell] = (self.flags[cell] & !FLAG_WALKABLE) | u32::from(v);
    }

    #[must_use]
    pub fn tag(&self, cell: usize) -> u32 {
        (self.flags[cell] & FLAG_TAG_MASK) >> FLAG_TAG_SHIFT
    }

    /// `GraphNode.Tag = value`.
    fn set_tag(&mut self, cell: usize, v: u32) {
        self.flags[cell] =
            (self.flags[cell] & !FLAG_TAG_MASK) | ((v << FLAG_TAG_SHIFT) & FLAG_TAG_MASK);
    }

    fn walkable_erosion(&self, cell: usize) -> bool {
        self.grid_flags[cell] & 0x100 != 0
    }

    fn set_walkable_erosion(&mut self, cell: usize, v: bool) {
        self.grid_flags[cell] = (self.grid_flags[cell] & !0x100) | if v { 0x100 } else { 0 };
    }

    fn tmp_walkable(&self, cell: usize) -> bool {
        self.grid_flags[cell] & 0x200 != 0
    }

    fn set_tmp_walkable(&mut self, cell: usize, v: bool) {
        self.grid_flags[cell] = (self.grid_flags[cell] & !0x200) | if v { 0x200 } else { 0 };
    }

    fn edge_node(&self, cell: usize) -> bool {
        self.grid_flags[cell] & 0x400 != 0
    }

    /// `GraphTransform.InverseTransform(Bounds)` (the eight corners when the transform
    /// isn't a translation).
    #[must_use]
    pub fn inverse_transform_bounds(&self, b: &Bounds) -> Bounds {
        if self.only_translational {
            return Bounds::new(cs::sub(b.center, self.translation), cs::mul(b.extents, 2.0));
        }
        let e = b.extents;
        let corners = [
            [e[0], e[1], e[2]],
            [e[0], e[1], -e[2]],
            [e[0], -e[1], e[2]],
            [e[0], -e[1], -e[2]],
            [-e[0], e[1], e[2]],
            [-e[0], e[1], -e[2]],
            [-e[0], -e[1], e[2]],
            [-e[0], -e[1], -e[2]],
        ]
        .map(|c| self.inverse_transform(cs::add(b.center, c)));
        let mut lo = corners[0];
        let mut hi = corners[0];
        // Vector3.Min/Max: Mathf.Min(a, b) = a < b ? a : b; Mathf.Max = a > b ? a : b.
        for c in &corners[1..] {
            for i in 0..3 {
                lo[i] = if lo[i] < c[i] { lo[i] } else { c[i] };
                hi[i] = if hi[i] > c[i] { hi[i] } else { c[i] };
            }
        }
        Bounds::new(cs::mul(cs::add(lo, hi), 0.5), cs::sub(hi, lo))
    }

    /// `GridGraph.GetRectFromBounds`.
    #[must_use]
    pub fn rect_from_bounds(&self, b: &Bounds) -> IntRect {
        let lb = self.inverse_transform_bounds(b);
        let (min, max) = (lb.min(), lb.max());
        let a = IntRect::new(
            round_to_int(min[0] - 0.5),
            round_to_int(min[2] - 0.5),
            round_to_int(max[0] - 0.5),
            round_to_int(max[2] - 0.5),
        );
        IntRect::intersection(a, self.whole())
    }

    /// `GridGraph.GetNodesInRegion(Bounds)`: cells of the rectangle (x outer, z inner)
    /// whose position the bounds contain.
    #[must_use]
    pub fn nodes_in_bounds(&self, b: &Bounds) -> Vec<usize> {
        let r = self.rect_from_bounds(b);
        let mut out = Vec::new();
        if !r.is_valid() {
            return out;
        }
        for x in r.xmin..=r.xmax {
            for z in r.ymin..=r.ymax {
                let cell = (z * self.width + x) as usize;
                if b.contains(cs::from_int3(self.positions[cell])) {
                    out.push(cell);
                }
            }
        }
        out
    }

    /// `GridGraph.GetNodesInRegion(IntRect)` (z outer, x inner).
    fn nodes_in_rect(&self, r: IntRect) -> Vec<usize> {
        let r = IntRect::intersection(r, self.whole());
        let mut out = Vec::new();
        if !r.is_valid() {
            return out;
        }
        for z in r.ymin..=r.ymax {
            for x in r.xmin..=r.xmax {
                out.push((z * self.width + x) as usize);
            }
        }
        out
    }

    /// `GridGraph.IsValidConnection`.
    fn is_valid_connection(&self, a: usize, b: usize) -> bool {
        if !self.walkable(a) || !self.walkable(b) {
            return false;
        }
        if self.max_climb <= 0.0 {
            return true;
        }
        let (pa, pb) = (self.positions[a], self.positions[b]);
        if self.only_translational {
            // (float)Math.Abs(y1 - y2) <= maxClimb * 1000f (the product in double).
            return d((pa[1].wrapping_sub(pb[1])).wrapping_abs() as f32)
                <= d(self.max_climb) * 1000.0;
        }
        let (va, vb) = (cs::from_int3(pa), cs::from_int3(pb));
        // Math.Abs(float): its argument rounded to float.
        let up = self.transform_up;
        let diff = cs::r(d(cs::dot(up, va)) - d(cs::dot(up, vb))).abs();
        diff <= self.max_climb
    }

    /// `GridGraph.CalculateConnections(x, z)` for four or eight neighbours.
    pub fn calculate_connections(&mut self, x: i32, z: i32) {
        let cell = (z * self.width + x) as usize;
        if !self.walkable(cell) {
            self.grid_flags[cell] &= 0xFF00;
            return;
        }
        let mut num = 0i32;
        for i in 0..4 {
            let nx = x + self.neighbour_x[i];
            let nz = z + self.neighbour_z[i];
            if nx >= 0 && nz >= 0 && nx < self.width && nz < self.depth {
                let o = (cell as i64 + i64::from(self.neighbour_offsets[i])) as usize;
                if self.is_valid_connection(cell, o) {
                    num |= 1 << i;
                }
            }
        }
        let mut num4 = 0i32;
        if self.eight_neighbours {
            // C# masks shift counts to five bits: `num >> j + 1 - 4` with j < 3 shifts by 29..31.
            let shr = |v: i32, s: i32| v >> (s & 31);
            for j in 0..4i32 {
                let any = if self.cut_corners {
                    ((shr(num, j) | shr(num, j + 1) | shr(num, j + 1 - 4)) & 1) != 0
                } else {
                    (shr(num, j) & 1) != 0 && ((shr(num, j + 1) | shr(num, j + 1 - 4)) & 1) != 0
                };
                if !any {
                    continue;
                }
                let k = (j + 4) as usize;
                let nx = x + self.neighbour_x[k];
                let nz = z + self.neighbour_z[k];
                // Without cut corners the original indexes the diagonal unchecked: both
                // axis neighbours exist, so it is inside the grid too.
                if nx >= 0 && nz >= 0 && nx < self.width && nz < self.depth {
                    let o = (cell as i64 + i64::from(self.neighbour_offsets[k])) as usize;
                    if self.is_valid_connection(cell, o) {
                        num4 |= 1 << k;
                    }
                }
            }
        }
        self.grid_flags[cell] = (self.grid_flags[cell] & 0xFF00) | ((num | num4) as u16 & 0xFF);
    }

    /// `GridGraph.HasNodeConnection(GridNode, dir)`.
    fn has_node_connection(&self, cell: usize, dir: usize) -> bool {
        if !self.has_connection(cell, dir) {
            return false;
        }
        if !self.edge_node(cell) {
            return true;
        }
        let (x, z) = self.cell_xz(cell);
        let nx = x + self.neighbour_x[dir];
        let nz = z + self.neighbour_z[dir];
        nx >= 0 && nx < self.width && nz >= 0 && nz < self.depth
    }

    /// `GridGraph.HasTagInDirection` (bounds-checked on the array only, as the original).
    fn has_tag_in_direction(&self, cell: usize, dir: usize, tag: u32) -> bool {
        let n = cell as i64 + i64::from(self.neighbour_offsets[dir]);
        if n < 0 || n >= self.positions.len() as i64 {
            return false;
        }
        self.tag(n as usize) == tag
    }

    /// `GridGraph.ErosionAnyFalseConnections` (four or eight neighbours: the axis ones).
    fn erosion_any_false_connections(&self, cell: usize) -> bool {
        let only = self.erosion_only_around_cells_with_tag;
        (0..4).any(|j| {
            (only == 0 || self.has_tag_in_direction(cell, j, only))
                && !self.has_node_connection(cell, j)
        })
    }

    /// `GridNode.GetNeighbourAlongDirection`.
    fn neighbour_along(&self, cell: usize, dir: usize) -> Option<usize> {
        self.has_connection(cell, dir)
            .then(|| (cell as i64 + i64::from(self.neighbour_offsets[dir])) as usize)
    }

    /// `GridGraph.ErodeWalkableArea(xmin, zmin, xmax, zmax)` (exclusive maxima).
    ///
    /// # Errors
    ///
    /// Erosion without tags (it changes walkability and connections): not ported.
    pub fn erode_walkable_area(
        &mut self,
        xmin: i32,
        zmin: i32,
        xmax: i32,
        zmax: i32,
    ) -> Result<()> {
        if self.erosion_use_tags {
            // The original logs an error and returns.
            if self.erode_iterations + self.erosion_first_tag > 31 || self.erosion_first_tag <= 0 {
                return Ok(());
            }
        }
        if self.erode_iterations == 0 {
            return Ok(());
        }
        ensure!(self.erosion_use_tags, "erosion without tags is not ported");
        let list = self.nodes_in_rect(IntRect::new(xmin, zmin, xmax - 1, zmax - 1));
        let first = self.erosion_first_tag as u32;
        let only = self.erosion_only_around_cells_with_tag;
        for i in 0..self.erode_iterations as u32 {
            if i == 0 {
                for &c in &list {
                    // ErodeNodeWithTagsInit.
                    if only == 0 || self.tag(c) != only {
                        let t = if self.walkable(c) && self.erosion_any_false_connections(c) {
                            first
                        } else {
                            0
                        };
                        self.set_tag(c, t);
                    }
                }
            } else {
                for &c in &list {
                    // ErodeNodeWithTags.
                    let t = self.tag(c);
                    if !self.walkable(c) || t < first || t >= first + i {
                        continue;
                    }
                    for j in 0..4 {
                        if let Some(n) = self.neighbour_along(c, j) {
                            let tn = self.tag(n);
                            if tn > first + i || tn < first {
                                self.set_tag(n, first + i);
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// The grid's state as WrathOracle's `nav_tags` hashes it: FNV-1a 64 over each cell in
    /// index order of `flags & 0xFFF80001` (u32 LE), `gridFlags` (u16 LE), `DynamicBlocked`
    /// and `AllyBlocked` (a byte each).
    #[must_use]
    pub fn state_hash(&self) -> u64 {
        let mut bytes = Vec::with_capacity(self.positions.len() * 8);
        for c in 0..self.positions.len() {
            bytes.extend_from_slice(&(self.flags[c] & 0xFFF8_0001).to_le_bytes());
            bytes.extend_from_slice(&self.grid_flags[c].to_le_bytes());
            bytes.push(self.dynamic_blocked(c) as u8);
            bytes.push(self.ally_blocked(c) as u8);
        }
        crate::fnv1a64(&bytes)
    }

    /// Cells with a dynamic blocker, and with an ally blocker.
    #[must_use]
    pub fn blocked_counts(&self) -> (usize, usize) {
        let n = self.positions.len();
        (
            (0..n).filter(|&c| self.dynamic_blocked(c) != 0).count(),
            (0..n).filter(|&c| self.ally_blocked(c) != 0).count(),
        )
    }

    /// `GraphUpdateObject.Apply(node)` (no shape, `addPenalty` 0).
    fn apply(&mut self, o: &GraphUpdate, cell: usize) {
        if o.modify_walkability {
            self.set_walkable(cell, o.set_walkability);
        }
        if o.modify_dynamic_walkability {
            let delta: i8 = if o.set_dynamic_walkability { -1 } else { 1 };
            self.dynamic_blocked[cell] = self.dynamic_blocked[cell].wrapping_add(delta);
        }
        if o.modify_ally_blocked {
            let delta: i8 = if o.set_ally_blocked { 1 } else { -1 };
            self.ally_blocked[cell] = self.ally_blocked[cell].wrapping_add(delta);
        }
        if o.modify_tag {
            self.set_tag(cell, o.set_tag);
        }
    }

    /// `GridGraph.UpdateArea(o)` with `erosion` = `updateErosion ? erodeIterations : 0`.
    ///
    /// # Errors
    ///
    /// A physics update (it rescans cells against the colliders): not ported.
    pub fn update_area(&mut self, o: &GraphUpdate) -> Result<()> {
        ensure!(!o.update_physics, "graph updates with physics are not ported");
        self.ensure_dynamic();
        let b = self.whole();
        // CalculateAffectedRegions.
        let lb = self.inverse_transform_bounds(&o.bounds);
        let (min, max) = (lb.min(), lb.max());
        let original = IntRect::new(
            round_to_int(min[0] - 0.5),
            round_to_int(min[2] - 0.5),
            round_to_int(max[0] - 0.5),
            round_to_int(max[2] - 0.5),
        );
        let physics = original;
        let erosion = if o.update_erosion { self.erode_iterations } else { 0 };
        let will_change = o.modify_walkability || o.modify_dynamic_walkability;
        let affect =
            if will_change || erosion > 0 { original.expand(erosion + 1) } else { original };
        // The cells of the original rectangle the bounds contain.
        let r = IntRect::intersection(original, b);
        for z in r.ymin..=r.ymax {
            for x in r.xmin..=r.xmax {
                let cell = (z * self.width + x) as usize;
                let p = cs::from_int3(self.positions[cell]);
                if !o.bounds.contains(p) {
                    continue;
                }
                if will_change {
                    // bounds.min = (min.x, 0, min.z); bounds.max = (max.x, 0, max.z), the
                    // second read after the first write; bounds.extents = (e.x, 1, e.z).
                    let mut flat = o.bounds;
                    let bmin = flat.min();
                    flat.set_min([bmin[0], 0.0, bmin[2]]);
                    let bmax = flat.max();
                    flat.set_max([bmax[0], 0.0, bmax[2]]);
                    flat.extents = [flat.extents[0], 1.0, flat.extents[2]];
                    if flat.contains([p[0], 0.0, p[2]]) {
                        let we = self.walkable_erosion(cell);
                        self.set_walkable(cell, we);
                        self.apply(o, cell);
                        let sw = self.static_walkable(cell);
                        self.set_walkable_erosion(cell, sw);
                    }
                } else {
                    self.apply(o, cell);
                }
            }
        }
        if will_change && erosion == 0 {
            let r = IntRect::intersection(affect, b);
            for x in r.xmin..=r.xmax {
                for z in r.ymin..=r.ymax {
                    self.calculate_connections(x, z);
                }
            }
        } else if will_change && erosion > 0 {
            let a0 = IntRect::union(original, physics).expand(erosion);
            let a2 = IntRect::intersection(a0.expand(erosion), b);
            let a = IntRect::intersection(a0, b);
            for x in a2.xmin..=a2.xmax {
                for z in a2.ymin..=a2.ymax {
                    let cell = (z * self.width + x) as usize;
                    let w = self.walkable(cell);
                    let we = self.walkable_erosion(cell);
                    self.set_walkable(cell, we);
                    if !a.contains(x, z) {
                        self.set_tmp_walkable(cell, w);
                    }
                }
            }
            for x in a2.xmin..=a2.xmax {
                for z in a2.ymin..=a2.ymax {
                    self.calculate_connections(x, z);
                }
            }
            self.erode_walkable_area(a2.xmin, a2.ymin, a2.xmax + 1, a2.ymax + 1)?;
            for x in a2.xmin..=a2.xmax {
                for z in a2.ymin..=a2.ymax {
                    if !a.contains(x, z) {
                        let cell = (z * self.width + x) as usize;
                        let t = self.tmp_walkable(cell);
                        self.set_walkable(cell, t);
                    }
                }
            }
            for x in a2.xmin..=a2.xmax {
                for z in a2.ymin..=a2.ymax {
                    self.calculate_connections(x, z);
                }
            }
        }
        Ok(())
    }
}

impl Grid {
    /// Connections without their reverse (a diagonal with `cutCorners` can be one-way when
    /// the two cells' axis neighbours differ). [`World::compute_areas`] equals A*'s
    /// `HierarchicalGraph` components only when there are none: the hierarchical graph
    /// follows connections one way within its 256-node clusters.
    #[must_use]
    pub fn one_way_connections(&self) -> usize {
        let opposite = |d: usize| if d < 4 { (d + 2) % 4 } else { (d - 2) % 4 + 4 };
        let mut n = 0;
        for c in 0..self.positions.len() {
            for d in 0..8 {
                if self.has_connection(c, d) {
                    let o = (c as i64 + i64::from(self.neighbour_offsets[d])) as usize;
                    if !self.has_connection(o, opposite(d)) {
                        n += 1;
                    }
                }
            }
        }
        n
    }
}

impl World {
    /// Refuses a grid with one-way connections (see [`Grid::one_way_connections`]).
    ///
    /// # Errors
    ///
    /// Any one-way connection.
    pub fn check_two_way_grid(&self) -> Result<()> {
        if let Some(g) = &self.grid {
            let n = g.one_way_connections();
            ensure!(
                n == 0,
                "{n} one-way grid connections: the hierarchical graph's areas aren't modelled for them"
            );
        }
        Ok(())
    }

    /// Processes queued graph updates as `GraphUpdateProcessor` does when its work item
    /// runs (each object on every suitable updatable graph, in queue order), then the
    /// connected components (`EnsureValidFloodFill`).
    ///
    /// # Errors
    ///
    /// An update reaching the navmesh (navmesh cuts and updates aren't ported), or one the
    /// grid can't apply.
    pub fn apply_graph_updates(&mut self, updates: &[GraphUpdate]) -> Result<()> {
        self.check_two_way_grid()?;
        if updates.is_empty() {
            return Ok(());
        }
        for o in updates {
            if let Some(rc) = &self.recast
                && (o.graph_mask >> rc.graph_index) & 1 != 0
            {
                bail!("a graph update on the navmesh is not ported");
            }
            if let Some(g) = &mut self.grid
                && (o.graph_mask >> g.graph_index) & 1 != 0
            {
                g.update_area(o)?;
            }
        }
        self.check_two_way_grid()?;
        self.area = self.compute_areas();
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn bounds_contains_is_inclusive_and_nan_inside() {
        let b = Bounds::new([1.0, 2.0, 3.0], [2.0, 0.0, 4.0]);
        assert!(b.contains([0.0, 2.0, 1.0]));
        assert!(b.contains([2.0, 2.0, 5.0]));
        assert!(!b.contains([2.000_001, 2.0, 5.0]));
        assert!(!b.contains([1.0, 2.000_001, 3.0]));
        assert!(b.contains([f32::NAN, 2.0, 3.0]));
    }

    fn caves() -> World {
        let path = crate::path_for("bad465e5404604346bdf71ef50b3299a");
        crate::NavFile::read(&path).unwrap_or_else(|e| panic!("{e:#}")).world
    }

    /// The baked grid is what `CalculateConnections` and tag erosion (with the serialized
    /// `erodeIterations`) produce from its walkability: every cell's connections and tag.
    #[test]
    #[ignore = "needs the caves' nav file (wr-import nav-pack)"]
    fn connections_and_erosion_rebuild_the_baked_grid() {
        let w = caves();
        let baked = w.grid.clone().unwrap();
        let mut g = baked.clone();
        let n = g.positions.len();
        assert!((0..n).all(|c| g.walkable_erosion(c) == g.static_walkable(c)));
        for c in 0..n {
            let (x, z) = g.cell_xz(c);
            g.calculate_connections(x, z);
        }
        let conn_diff = (0..n).filter(|&c| g.grid_flags[c] != baked.grid_flags[c]).count();
        assert_eq!(conn_diff, 0, "connections differ on {conn_diff} cells");
        assert_eq!(g.one_way_connections(), 0);
        // The navmesh's connections too go both ways (its areas, computed the same way).
        let mut buf = Vec::new();
        let one_way_tris = (0..w.tris.len() as u32)
            .filter(|&t| {
                buf.clear();
                w.connections(t, &mut buf);
                buf.iter().any(|&o| {
                    let mut back = Vec::new();
                    w.connections(o, &mut back);
                    !back.contains(&t)
                })
            })
            .count();
        assert_eq!(one_way_tris, 0);
        assert_eq!(g.erode_iterations, 2);
        let (wd, dp) = (g.width, g.depth);
        g.erode_walkable_area(0, 0, wd, dp).unwrap();
        let tag_diff: Vec<usize> = (0..n).filter(|&c| g.tag(c) != baked.tag(c)).collect();
        assert!(
            tag_diff.is_empty(),
            "tags differ on {} cells, e.g. {:?}",
            tag_diff.len(),
            &tag_diff[..tag_diff.len().min(10)]
        );
        let tagged = (0..n).filter(|&c| g.tag(c) != 0).count();
        assert!(tagged > 4000, "{tagged} eroded cells");
    }

    /// A flat `w` x `d` grid of 1 m cells from the origin, every cell walkable.
    pub(crate) fn flat(w: i32, d: i32) -> World {
        let n = (w * d) as usize;
        let positions =
            (0..n as i32).map(|c| [(c % w) * 1000 + 500, 0, (c / w) * 1000 + 500]).collect();
        let mut g = Grid {
            graph_index: 0,
            first: 0,
            width: w,
            depth: d,
            node_size: 1.0,
            positions,
            flags: vec![FLAG_WALKABLE; n],
            grid_flags: vec![0x100; n],
            matrix: cs::IDENTITY,
            inverse: cs::IDENTITY,
            only_translational: true,
            translation: [0.0; 3],
            neighbour_offsets: [-w, 1, w, -1, -w + 1, w + 1, w - 1, -w - 1],
            neighbour_costs: [1000, 1000, 1000, 1000, 1414, 1414, 1414, 1414],
            neighbour_x: [0, 1, 0, -1, 1, 1, -1, -1],
            neighbour_z: [-1, 0, 1, 0, -1, 1, 1, -1],
            eight_neighbours: true,
            erode_iterations: 0,
            erosion_first_tag: 1,
            erosion_use_tags: true,
            erosion_only_around_cells_with_tag: 31,
            cut_corners: true,
            max_climb: 1.0,
            up: [0.0, 1.0, 0.0],
            transform_up: [0.0, 1.0, 0.0],
            dynamic_blocked: Vec::new(),
            ally_blocked: Vec::new(),
        };
        for c in 0..n {
            let (x, z) = g.cell_xz(c);
            g.calculate_connections(x, z);
        }
        let mut world = World {
            tris: Vec::new(),
            recast: None,
            grid: Some(g),
            area: Vec::new(),
            max_nearest_node_distance: 100.0,
            full_get_nearest_search: false,
        };
        world.area = world.compute_areas();
        world
    }

    /// `CombatController`'s enemy object around a cell centre.
    pub(crate) fn enemy_box(x: f32, z: f32, side: f32) -> GraphUpdate {
        let mut o = GraphUpdate::new(Bounds::new([x, 0.0, z], [side, 10.0, side]));
        o.graph_mask = 1;
        o.update_physics = false;
        o.modify_dynamic_walkability = true;
        o.modify_tag = true;
        o.set_tag = 31;
        o
    }

    #[test]
    fn an_enemy_box_blocks_its_cells_and_its_revert_restores_them() {
        let mut w = flat(5, 5);
        let before = w.grid.clone().unwrap();
        // A wall of blocked cells across x = 2 cuts the grid in two areas.
        let mut boxes = Vec::new();
        for z in 0..5 {
            boxes.push(enemy_box(2.5, z as f32 + 0.5, 1.5));
        }
        w.apply_graph_updates(&boxes).unwrap();
        let g = w.grid.as_ref().unwrap();
        for z in 0..5usize {
            let c = z * 5 + 2;
            assert_eq!(g.dynamic_blocked(c), 1);
            assert_eq!(g.tag(c), 31);
            assert!(!g.walkable(c) && g.static_walkable(c));
            assert_eq!(g.grid_flags[c] & 0xFF, 0);
            assert!(!g.has_connection(c - 1, 1) && !g.has_connection(c + 1, 3));
        }
        assert_ne!(w.area[0], w.area[4]);
        assert_eq!(w.area[2], 0);
        // Erosion around the blocked cells only, one cell deep.
        let g = w.grid.as_mut().unwrap();
        g.erode_iterations = 1;
        g.erode_walkable_area(0, 0, 5, 5).unwrap();
        assert_eq!((0..5).map(|x| g.tag(x)).collect::<Vec<_>>(), [0, 1, 31, 1, 0]);
        g.erode_iterations = 0;
        let reverts: Vec<GraphUpdate> = boxes
            .iter()
            .map(|o| GraphUpdate { set_dynamic_walkability: true, set_tag: 0, ..*o })
            .collect();
        w.apply_graph_updates(&reverts).unwrap();
        let g = w.grid.as_ref().unwrap();
        assert_eq!(g.grid_flags, before.grid_flags);
        assert!((0..25).all(|c| g.dynamic_blocked(c) == 0));
        assert_eq!(w.area[0], w.area[4]);
    }

    /// A one-way connection (whose areas A*'s hierarchical graph builds otherwise) is refused,
    /// even with nothing to update.
    #[test]
    fn one_way_connections_are_refused() {
        let mut w = flat(3, 3);
        assert!(w.check_two_way_grid().is_ok());
        // Cell 4 (the centre) loses its connection to cell 5 (+x); 5 keeps its own to 4.
        w.grid.as_mut().unwrap().grid_flags[4] &= !(1 << 1);
        assert_eq!(w.grid.as_ref().unwrap().one_way_connections(), 1);
        assert!(w.apply_graph_updates(&[]).is_err());
        assert!(w.check_two_way_grid().is_err());
    }

    #[test]
    fn set_min_keeps_max() {
        let mut b = Bounds::new([1.0, 5.0, 3.0], [2.0, 10.0, 4.0]);
        b.set_min([0.0, 0.0, 1.0]);
        assert_eq!(b.max(), [2.0, 10.0, 5.0]);
        assert_eq!(b.min(), [0.0, 0.0, 1.0]);
    }
}
