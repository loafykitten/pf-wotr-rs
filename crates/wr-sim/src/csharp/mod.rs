//! C# and Mono runtime semantics the simulation depends on (PLAN.md, simulation/
//! presentation contract item 4: integer wrap, Mono's float evaluation, rounding and
//! ties; item 5: tie-breaking in sorts and collection order).
//!
//! Everything here mirrors the game's own runtime, read from the shipped `mscorlib.dll`
//! (decompiled with `ilspycmd` into the scratchpad, not kept): `ArraySortHelper<T>`,
//! `Dictionary<TKey, TValue>`, `List<T>`, `TimeSpan` and `String.GetHashCode` of the Unity
//! 2020.3 Mono profile.

pub mod arith;
pub mod decimal;
pub mod dict;
pub mod hashset;
pub mod sort;
pub mod string_hash;
pub mod timespan;

pub use decimal::Decimal;
pub use dict::{CsDict, RawSlot};
pub use sort::{SortReport, introsort};
pub use timespan::TimeSpan;
