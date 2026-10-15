//! `wr-sim`: the game simulation, in plain Rust (Decision 1): no Bevy, deterministic,
//! headless, with durable game IDs. See `docs/sim/architecture.md`.
//!
//! - [`csharp`]: C#/Mono runtime semantics (integer wrap, float evaluation, rounding,
//!   `List.Sort`, `Dictionary` order, `TimeSpan`).
//! - [`rng`]: the one RNG interface (replay tape, play generator).
//! - [`time`]: the sim clock (`TimeController`).
//! - [`world`], [`entity`], [`unit`], [`item`], [`facts`], [`context`]: entities, facts with
//!   per-fact component state, and their lifecycle.
//! - [`stats`]: `ModifiableValue` and its subclasses.
//! - [`rulebook`]: nested rules and subscriber order.
//! - [`registry`]: fail-closed behaviour registries and the coverage manifest.
//! - [`components`]: ported blueprint components.
//! - [`hydrate`], [`sheet`], [`manifest`]: loading a save, the character-sheet dump, the
//!   required-versus-implemented manifest.

pub mod armor;
pub mod combat;
pub mod compare;
pub mod components;
pub mod content;
pub mod context;
pub mod csharp;
pub mod dialog;
pub mod difficulty;
pub mod elements;
pub mod entity;
pub mod events;
pub mod facts;
pub mod feature_param;
pub mod game_log;
pub mod hydrate;
pub mod item;
pub mod items;
pub mod manifest;
pub mod narrative;
pub mod registry;
pub mod resources;
pub mod rng;
pub mod rulebook;
pub mod rules;
pub mod runner;
pub mod scene;
pub mod scene_parts;
pub mod scene_spawn;
#[cfg(test)]
mod scene_tests;
pub mod sheet;
pub mod spawn;
pub mod spellbook;
pub mod stats;
pub mod text;
pub mod time;
pub mod unit;
pub mod unit_facts;
pub mod unit_state;
pub mod world;
pub mod xp;
