//! `Owlcat.Runtime.Visual.RenderPipeline.RendererFeatures.FogOfWar.LineOfSightGeometry`
//! (decompiled, not copied): the blockers' segments bucketed in 3.9 m cells over the area's fog
//! of war bounds, and `HasObstacle`'s walk over the cells a segment crosses. Float expressions
//! are Mono's: computed in double and rounded where the C# stores (a local, a field, an
//! argument, a return value); a single operation on `f32`s is the same either way.

use wr_nav::cs::{d, r};

use crate::Blocker;

type V2 = [f32; 2];
type V3 = [f32; 3];

/// `LineOfSightGeometry.GridSize` (`3.8999999f`).
pub const GRID: f32 = 3.899_999_9;

/// Cells walked beyond which a segment's walk is taken to diverge (the C#'s loop would not end).
const WALK_LIMIT: usize = 1_000_000;

/// What the walk can't answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LosError {
    /// A segment's walk over the cells doesn't end (a direction that is NaN or infinite).
    Diverges,
    /// An area too large for the cell grid.
    TooLarge,
}

impl std::fmt::Display for LosError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Diverges => f.write_str("a segment's walk over the cells does not end"),
            Self::TooLarge => f.write_str("the cell grid is too large"),
        }
    }
}

impl std::error::Error for LosError {}

/// A Unity `Bounds` (`m_Center`, `m_Extents`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub center: V3,
    pub extents: V3,
}

impl Bounds {
    /// `Bounds.size`: `m_Extents * 2f`.
    #[must_use]
    pub fn size(&self) -> V3 {
        self.extents.map(|e| e * 2.0)
    }

    /// `Bounds.min`: `center - extents`.
    #[must_use]
    pub fn min(&self) -> V3 {
        [0, 1, 2].map(|i| self.center[i] - self.extents[i])
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Segment {
    a: V2,
    b: V2,
    height: f32,
}

#[derive(Debug, Clone, Default)]
struct Cell {
    center: V2,
    lines: Vec<Segment>,
}

/// The geometry after `Init(bounds)` and the registration of every blocker.
#[derive(Debug, Clone)]
pub struct Geometry {
    nx: i32,
    nz: i32,
    /// `m_Bounds.min.x` and `.z`.
    min_x: f32,
    min_z: f32,
    cells: Vec<Cell>,
}

/// `Mathf.Sign(f)`: `f >= 0 ? 1 : -1` (NaN gives -1).
fn sign(f: f32) -> i32 {
    if f >= 0.0 { 1 } else { -1 }
}

/// `Mathf.FloorToInt(f)`: `(int)Math.Floor(f)`.
fn floor_to_int(f: f32) -> i32 {
    d(f).floor() as i32
}

/// `Mathf.CeilToInt(f)`: `(int)Math.Ceiling(f)`.
fn ceil_to_int(f: f32) -> i32 {
    d(f).ceil() as i32
}

/// `Vector2.magnitude`: `(float)Math.Sqrt(x * x + y * y)`.
fn magnitude(a: V2) -> f32 {
    r((d(a[0]) * d(a[0]) + d(a[1]) * d(a[1])).sqrt())
}

fn sub(a: V2, b: V2) -> V2 {
    [a[0] - b[0], a[1] - b[1]]
}

/// `VectorMath.SegmentsIntersect2D(start1, end1, start2, end2, out normDist1)` (Owlcat.Runtime.Core).
#[must_use]
pub fn segments_intersect_2d(start1: V2, end1: V2, start2: V2, end2: V2) -> Option<f32> {
    let vector = sub(end1, start1);
    let vector2 = sub(end2, start2);
    let num = r(d(vector2[1]) * d(vector[0]) - d(vector2[0]) * d(vector[1]));
    if num == 0.0 {
        return None;
    }
    let num2 = r(d(vector2[0]) * (d(start1[1]) - d(start2[1]))
        - d(vector2[1]) * (d(start1[0]) - d(start2[0])));
    let num3 =
        r(d(vector[0]) * (d(start1[1]) - d(start2[1]))
            - d(vector[1]) * (d(start1[0]) - d(start2[0])));
    let num4 = num2 / num;
    let num5 = num3 / num;
    #[allow(clippy::manual_range_contains)]
    if num4 < 0.0 || num4 > 1.0 || num5 < 0.0 || num5 > 1.0 {
        return None;
    }
    Some(num4)
}

impl Geometry {
    /// `Init(bounds)` with every blocker registered (`FogOfWarBlocker.All`).
    ///
    /// # Errors
    ///
    /// A grid of more than 16 million cells, or a blocker's segment whose walk diverges.
    pub fn init(bounds: Bounds, blockers: &[Blocker]) -> Result<Self, LosError> {
        let size = bounds.size();
        let nx = ceil_to_int(size[0] / GRID);
        let nz = ceil_to_int(size[2] / GRID);
        if nx < 0 || nz < 0 || i64::from(nx) * i64::from(nz) > 16_000_000 {
            return Err(LosError::TooLarge);
        }
        let x = r(d(nx as f32) * d(GRID));
        let z = r(d(nz as f32) * d(GRID));
        let m = Bounds { center: bounds.center, extents: [x * 0.5, 0.0 * 0.5, z * 0.5] };
        let min = m.min();
        let mut cells = Vec::with_capacity(nx as usize * nz as usize);
        for i in 0..nx {
            for j in 0..nz {
                cells.push(Cell {
                    center: [
                        r(d(min[0]) + d(GRID) * (f64::from(i) + 0.5)),
                        r(d(min[2]) + d(GRID) * (f64::from(j) + 0.5)),
                    ],
                    lines: Vec::new(),
                });
            }
        }
        let mut g = Self { nx, nz, min_x: min[0], min_z: min[2], cells };
        for b in blockers {
            g.register(b)?;
        }
        Ok(g)
    }

    fn cell(&self, i: i32, j: i32) -> usize {
        (i as usize) * (self.nz as usize) + j as usize
    }

    /// `RegisterBlocker` / `AddPointsList(points, closed, blocker)`.
    fn register(&mut self, b: &Blocker) -> Result<(), LosError> {
        let n = b.points.len();
        let count = if b.closed { n } else { n.saturating_sub(1) };
        for i in 0..count {
            let p = b.points[i];
            let p2 = b.points[(i + 1) % n];
            for c in self.cells_for_segment(p, p2, false)? {
                self.cells[c].lines.push(Segment { a: p, b: p2, height: b.height_y });
            }
        }
        Ok(())
    }

    /// `GetCellForPoint(p)`: the index of the cell, `None` outside the grid.
    fn cell_for_point(&self, p: V2) -> Option<(i32, i32)> {
        let i = floor_to_int(r((d(p[0]) - d(self.min_x)) / d(GRID)));
        let j = floor_to_int(r((d(p[1]) - d(self.min_z)) / d(GRID)));
        if i < 0 || i >= self.nx || j < 0 || j >= self.nz {
            return None;
        }
        Some((i, j))
    }

    /// `GetCellsForSegment(p1, p2, skipEmpty)`: the cells the walk visits, in order (a cell can
    /// appear twice).
    fn cells_for_segment(&self, p1: V2, p2: V2, skip_empty: bool) -> Result<Vec<usize>, LosError> {
        let mut out = Vec::new();
        let (mut p1, mut p2) = (p1, p2);
        let mut c1 = self.cell_for_point(p1);
        let c2 = self.cell_for_point(p2);
        if c1.is_none() {
            // `GetCellsForSegment(p2, p1, skipEmpty)` (the walk from the cell the segment does
            // have, if either).
            if c2.is_none() {
                return Ok(out);
            }
            std::mem::swap(&mut p1, &mut p2);
            c1 = c2;
        }
        let c2 = self.cell_for_point(p2);
        let Some((x1, y1)) = c1 else { return Ok(out) };
        let keep = |s: &Self, c: usize| !skip_empty || !s.cells[c].lines.is_empty();
        let first = self.cell(x1, y1);
        if keep(self, first) {
            out.push(first);
        }
        let magnitude = magnitude(sub(p2, p1));
        let delta = sub(p2, p1);
        let vector = [delta[0] / magnitude, delta[1] / magnitude];
        let num = sign(vector[0]);
        let num2 = sign(vector[1]);
        let mut at = (x1, y1);
        let mut num3 = 0.0f32;
        let mut steps = 0usize;
        while Some(at) != c2 {
            steps += 1;
            if steps > WALK_LIMIT {
                return Err(LosError::Diverges);
            }
            let cur = self.cell(at.0, at.1);
            let center = self.cells[cur].center;
            // `p1 + vector * num3`: the product stored as a Vector2, then the sum.
            let scaled = [vector[0] * num3, vector[1] * num3];
            let vector2 = [p1[0] + scaled[0], p1[1] + scaled[1]];
            let num4 = r(d(center[0]) + f64::from(num) * d(GRID) / 2.0 - d(vector2[0]));
            let num5 = r(d(center[1]) + f64::from(num2) * d(GRID) / 2.0 - d(vector2[1]));
            let num6 = if vector[0] == 0.0 { f32::MAX } else { num4 / vector[0] };
            let num7 = if vector[1] == 0.0 { f32::MAX } else { num5 / vector[1] };
            let mut num8 = at.0;
            let mut num9 = at.1;
            if num6 <= num7 {
                num8 += num;
                if num8 < 0 || num8 >= self.nx {
                    break;
                }
                at = (num8, num9);
                let c = self.cell(num8, num9);
                if keep(self, c) {
                    out.push(c);
                }
            }
            if num6 >= num7 {
                num9 += num2;
                if num9 < 0 || num9 >= self.nz {
                    break;
                }
                at = (num8, num9);
                let c = self.cell(num8, num9);
                if keep(self, c) {
                    out.push(c);
                }
            }
            if num6 == num7 {
                at = (num8 - num, num9);
                let c = self.cell(at.0, at.1);
                if keep(self, c) {
                    out.push(c);
                }
            }
            // `num3 += Mathf.Min(num6, num7)`.
            let min = if num6 < num7 { num6 } else { num7 };
            num3 += min;
        }
        Ok(out)
    }

    /// `HasObstacle(from, to, fudgeRadius)`: whether a blocker at least as high as `from.y`
    /// crosses the segment's projection on the ground plane.
    ///
    /// # Errors
    ///
    /// A segment whose walk over the cells diverges.
    pub fn has_obstacle(&self, from: V3, to: V3, fudge_radius: f32) -> Result<bool, LosError> {
        let y = from[1];
        let vector = [from[0], from[2]];
        let mut vector2 = [to[0], to[2]];
        if fudge_radius > 0.0 {
            // `vector2 += (vector - vector2).normalized * fudgeRadius`, each step a Vector2.
            let diff = sub(vector, vector2);
            let m = magnitude(diff);
            let n = if m > 1e-5 { [diff[0] / m, diff[1] / m] } else { [0.0, 0.0] };
            let s = [n[0] * fudge_radius, n[1] * fudge_radius];
            vector2 = [vector2[0] + s[0], vector2[1] + s[1]];
        }
        for c in self.cells_for_segment(vector, vector2, true)? {
            for line in &self.cells[c].lines {
                // `!(lineSegment.Height < height)` (a NaN height isn't lower).
                #[allow(clippy::neg_cmp_op_on_partial_ord)]
                if !(line.height < y)
                    && segments_intersect_2d(vector, vector2, line.a, line.b).is_some()
                {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// The grid's size in cells and its `m_Bounds.min` (x, z), for checks against a recording.
    #[must_use]
    pub fn grid(&self) -> (i32, i32, f32, f32) {
        (self.nx, self.nz, self.min_x, self.min_z)
    }

    /// Every cell's segments, `(cell x, cell z, a, b, height)`, in registration order: for
    /// checks against a recording.
    #[must_use]
    pub fn segments(&self) -> Vec<(i32, i32, V2, V2, f32)> {
        let mut out = Vec::new();
        for i in 0..self.nx {
            for j in 0..self.nz {
                for l in &self.cells[self.cell(i, j)].lines {
                    out.push((i, j, l.a, l.b, l.height));
                }
            }
        }
        out
    }
}
