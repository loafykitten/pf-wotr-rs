//! Readers for the Unity formats the game ships: UnityFS bundles, serialized files and
//! the type trees embedded in them. Read-only, bounds-checked, no Unity dependency.

pub mod bundle;
pub mod reader;
pub mod serialized;
pub mod typetree;
pub mod value;
