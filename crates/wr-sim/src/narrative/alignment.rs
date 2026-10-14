//! `UnitAlignment` (decompiled: `Kingmaker.UnitLogic.Alignments/UnitAlignment.cs`,
//! `AlignmentShiftExtension.cs`): the alignment vector, its history, and `Shift`.
//!
//! Unity's `Vector2` operators and `Mathf` work in `float`; each operator returns a stored
//! `float` struct, so every step rounds to f32. `magnitude` is `(float)Math.Sqrt(x*x+y*y)`
//! (the products and sum evaluated in double by Mono, then the root cast to float);
//! `normalized` divides by it when it is above 1e-5. Lock masks other than `None`
//! (`CorrectByLockedMask`'s quaternion rotation) are not ported and report unsupported.

use serde::Serialize;

use crate::csharp::arith::store_f32;
use crate::world::World;

/// `Alignment` enum values in `AlignmentsCircle` order (the dictionary's insertion order,
/// which `GetAlignment`'s `Aggregate` walks).
pub const CIRCLE_ORDER: [i32; 9] = [
    4, // TrueNeutral
    5, // ChaoticNeutral
    2, // ChaoticGood
    1, // NeutralGood
    0, // LawfulGood
    3, // LawfulNeutral
    6, // LawfulEvil
    7, // NeutralEvil
    8, // ChaoticEvil
];

/// `Alignment` names by value.
pub const ALIGNMENT_NAMES: [&str; 9] = [
    "LawfulGood",
    "NeutralGood",
    "ChaoticGood",
    "LawfulNeutral",
    "TrueNeutral",
    "ChaoticNeutral",
    "LawfulEvil",
    "NeutralEvil",
    "ChaoticEvil",
];

/// `AlignmentShiftDirection` names by value.
pub const DIRECTION_NAMES: [&str; 13] = [
    "LawfulGood",
    "NeutralGood",
    "ChaoticGood",
    "LawfulNeutral",
    "TrueNeutral",
    "ChaoticNeutral",
    "LawfulEvil",
    "NeutralEvil",
    "ChaoticEvil",
    "Good",
    "Evil",
    "Lawful",
    "Chaotic",
];

/// One `AlignmentHistoryRecord`: position, direction, provider blueprint.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HistoryRecord {
    pub position: [f32; 2],
    pub direction: i32,
    #[serde(serialize_with = "ser_guid")]
    pub provider: Option<wr_content::Guid>,
}

#[allow(clippy::ref_option)]
fn ser_guid<S: serde::Serializer>(g: &Option<wr_content::Guid>, s: S) -> Result<S::Ok, S::Error> {
    match g {
        Some(g) => s.serialize_str(&g.to_string()),
        None => s.serialize_none(),
    }
}

/// `UnitAlignment`'s saved state.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UnitAlignment {
    pub vector: [f32; 2],
    pub locked_mask: i32,
    pub target_alignment: Option<i32>,
    /// `m_History` (`None`: null, created on first shift).
    pub history: Option<Vec<HistoryRecord>>,
    pub undetectable: i32,
}

fn deg(a: f32) -> f32 {
    // `angle * ((float)Math.PI / 180f)`: the constant folds to a float; the product is
    // stored as the argument.
    #[allow(clippy::cast_possible_truncation)]
    let k = std::f64::consts::PI as f32 / 180.0;
    store_f32(f64::from(a) * f64::from(k))
}

/// `Mathf.Cos`/`Mathf.Sin`: `(float)Math.Cos((double)f)`.
fn cos(a: f32) -> f32 {
    store_f32(f64::from(deg(a)).cos())
}

fn sin(a: f32) -> f32 {
    store_f32(f64::from(deg(a)).sin())
}

/// `AlignmentsCircle[alignment]`.
#[must_use]
pub fn circle(alignment: i32) -> [f32; 2] {
    let angle = match alignment {
        5 => 0.0,
        2 => 45.0,
        1 => 90.0,
        0 => 135.0,
        3 => 180.0,
        6 => 225.0,
        7 => 270.0,
        8 => 315.0,
        _ => return [0.0, 0.0],
    };
    [cos(angle), sin(angle)]
}

fn sub(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [store_f32(f64::from(a[0]) - f64::from(b[0])), store_f32(f64::from(a[1]) - f64::from(b[1]))]
}

fn add(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [store_f32(f64::from(a[0]) + f64::from(b[0])), store_f32(f64::from(a[1]) + f64::from(b[1]))]
}

fn mul(a: [f32; 2], d: f32) -> [f32; 2] {
    [store_f32(f64::from(a[0]) * f64::from(d)), store_f32(f64::from(a[1]) * f64::from(d))]
}

fn sqr_magnitude(a: [f32; 2]) -> f32 {
    store_f32(f64::from(a[0]) * f64::from(a[0]) + f64::from(a[1]) * f64::from(a[1]))
}

fn magnitude(a: [f32; 2]) -> f32 {
    store_f32((f64::from(a[0]) * f64::from(a[0]) + f64::from(a[1]) * f64::from(a[1])).sqrt())
}

fn normalized(a: [f32; 2]) -> [f32; 2] {
    let m = magnitude(a);
    if m > 1e-5 {
        [store_f32(f64::from(a[0]) / f64::from(m)), store_f32(f64::from(a[1]) / f64::from(m))]
    } else {
        [0.0, 0.0]
    }
}

/// `GetAlignment(vec, None)`: true neutral inside radius 1/3, else the nearest circle
/// point (`Aggregate` keeps the first on ties: `!(a1 < a2) ? a2 : a1` keeps the *later*
/// one on equality).
#[must_use]
pub fn get_alignment(vec: [f32; 2]) -> i32 {
    // `TrueNeutralRadius = Mathf.Sqrt(1f / 9f)`.
    let r = store_f32((f64::from(store_f32(1.0 / 9.0))).sqrt());
    if sqr_magnitude(vec) <= store_f32(f64::from(r) * f64::from(r)) {
        return 4;
    }
    let mut best: Option<(i32, f32)> = None;
    for a in CIRCLE_ORDER.into_iter().filter(|a| *a != 4) {
        let d = sqr_magnitude(sub(circle(a), vec));
        best = match best {
            None => Some((a, d)),
            Some((b, bd)) => {
                if bd < d {
                    Some((b, bd))
                } else {
                    Some((a, d))
                }
            }
        };
    }
    best.map_or(4, |b| b.0)
}

/// `(int)Alignment` for an alignment index (LG, NG, CG, LN, TN, CN, LE, NE, CE): the enum's
/// values (`Kingmaker.Enums/Alignment.cs`), as WrathOracle writes them.
#[must_use]
pub fn enum_value(index: i32) -> i32 {
    const V: [i32; 9] = [10, 3, 18, 9, 1, 17, 12, 5, 20];
    usize::try_from(index).ok().and_then(|i| V.get(i).copied()).unwrap_or(-1)
}

/// The alignment index (`UnitAlignment` circle order) of an `Alignment` enum value, the
/// inverse of `enum_value`.
#[must_use]
pub fn index_of_enum_value(value: i64) -> Option<i32> {
    (0..9).find(|i| i64::from(enum_value(*i)) == value)
}

impl UnitAlignment {
    /// A fresh `UnitAlignment` after `Initialize(alignment)` (`UnitAlignment.cs`,
    /// `Set(alignment, raiseEvents: false)`): the circle point times 0.66f, and the history
    /// created by `OnChanged` with its initial record and the alignment's own.
    #[must_use]
    pub fn initialized(index: i32) -> Self {
        let vector = mul(circle(index), 0.66f32);
        UnitAlignment {
            vector,
            locked_mask: 0,
            target_alignment: None,
            history: Some(vec![
                HistoryRecord { position: [0.0, 0.0], direction: 4, provider: None },
                HistoryRecord { position: vector, direction: index, provider: None },
            ]),
            undetectable: 0,
        }
    }

    #[must_use]
    pub fn value(&self) -> i32 {
        get_alignment(self.vector)
    }

    fn direction(&self, d: i32) -> Option<[f32; 2]> {
        Some(match d {
            0..=8 => {
                let target = match d {
                    0 => 0,
                    1 => 1,
                    2 => 2,
                    3 => 3,
                    4 => 4,
                    5 => 5,
                    6 => 6,
                    7 => 7,
                    _ => 8,
                };
                normalized(sub(circle(target), self.vector))
            }
            9 => [0.0, 1.0],
            10 => [0.0, -1.0],
            11 => [-1.0, 0.0],
            12 => [1.0, 0.0],
            _ => return None,
        })
    }

    /// `Shift(direction, value, provider)`.
    pub fn shift(
        &mut self,
        w: &mut World,
        direction: i32,
        value: i32,
        radius: i32,
        provider: Option<wr_content::Guid>,
    ) {
        if value == 0 {
            return;
        }
        if self.locked_mask != 0 {
            w.coverage.unsupported("UnitAlignment: locked alignment mask");
        }
        let Some(dir) = self.direction(direction) else {
            w.coverage.unsupported("UnitAlignment.GetDirection: direction out of range");
            return;
        };
        // `(float)value / (float)Radius`.
        let num = store_f32(f64::from(value) / f64::from(radius));
        let mut v = add(self.vector, mul(dir, num));
        if magnitude(v) > 1.0 {
            v = normalized(v);
        }
        if direction == 4 && magnitude(sub(circle(4), self.vector)) < num {
            v = circle(4);
        }
        let prev = self.vector;
        self.vector = v;
        let _ = prev;
        // `OnChanged`: raises `IAlignmentShiftHandler` (and `IAlignmentChangeHandler` on a
        // change) to global subscribers, then appends the history record.
        crate::rulebook::bus::raise_global(w, "IAlignmentShiftHandler", None);
        if get_alignment(prev) != self.value() {
            crate::rulebook::bus::raise_global(w, "IAlignmentChangeHandler", None);
        }
        let h = self.history.get_or_insert_with(|| {
            vec![HistoryRecord { position: [0.0, 0.0], direction: 4, provider: None }]
        });
        h.push(HistoryRecord { position: self.vector, direction, provider });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alignment_of_vectors() {
        assert_eq!(get_alignment([0.0, 0.0]), 4);
        assert_eq!(get_alignment([0.0, -0.66]), 7); // NeutralEvil
        assert_eq!(get_alignment([0.66, 0.0]), 5); // ChaoticNeutral
        assert_eq!(get_alignment([-0.5, 0.5]), 0); // LawfulGood
        assert_eq!(get_alignment([0.1, 0.1]), 4); // inside the neutral radius
    }

    #[test]
    fn circle_points_are_unit_vectors() {
        for a in CIRCLE_ORDER {
            if a == 4 {
                continue;
            }
            let m = magnitude(circle(a));
            assert!((m - 1.0).abs() < 1e-6, "{a}");
        }
    }
}
