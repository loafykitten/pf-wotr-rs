//! `wr-import`: reads the game's Unity bundles in place and writes our own formats.
//!
//! POC-0 scope: the gameplay-object inventory and navigation graph of one area, plus
//! the yield report. Nothing here writes under the game install.

pub mod archeck;
pub mod area;
pub mod blueprints;
pub mod catalog;
pub mod families;
pub mod los;
pub mod nav;
pub mod oracle_diff;
pub mod pack;
pub mod phys;
pub mod prefabs;
pub mod refs;
pub mod report;
pub mod scene;
pub mod texture;
pub mod transform;
pub mod tree;
pub mod unity;
