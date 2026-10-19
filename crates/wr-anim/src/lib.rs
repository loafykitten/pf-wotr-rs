//! The animation runtime shared by the importer and the game: Unity generic clips, Mecanim
//! controllers and the pose of a bone hierarchy. Nothing here reads a Unity file; the importer
//! decodes the game's objects into these types and the pack carries them.

pub mod clip;
pub mod controller;
pub mod loco;
pub mod rig;

pub use clip::{Binding, Clip, Dense, Key, Streamed};
pub use controller::{Controller, Player};
pub use loco::{Locomotion, UnitAnimation, UnitPlayer};
pub use rig::{Bone, Pose, Rig, Trs, palette};
