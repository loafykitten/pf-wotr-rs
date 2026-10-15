//! Navigation: the A* Pathfinding Project graph cache from `.nav` bundles, and path
//! queries on it.
//!
//! `graph` decodes the cache; `world` holds it as A* does at runtime; `astar`,
//! `nearest`, `funnel` and `modifiers` port the search, nearest-node rules and path
//! modifiers of A* Pathfinding Project 4.3.10 and Owlcat's additions; `query` runs the
//! query kinds the WrathOracle mod records. `path` is the older, approximate baseline.

pub mod astar;
pub mod cs;
pub mod funnel;
pub mod graph;
pub mod modifiers;
#[cfg(test)]
mod native_check;
pub mod nearest;
pub mod path;
pub mod query;
pub mod world;
