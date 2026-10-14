//! POC-0 Wwise spike: the game's own Wwise runtime (`AkSoundEngine.bundle`) driven from
//! Rust, reproducing the game's initialisation, bank/package loading, voice playback,
//! callbacks, 3D positioning, suspend/resume and shutdown.
//!
//! - [`ffi`]: raw SWIG exports, resolved with `dlopen`.
//! - [`engine`]: the game's lifecycle over them (the seed of `crates/wr-audio`).
//! - [`settings`]: the game's serialized Wwise settings for macOS.
//! - Pure helpers: [`shortid`], [`sound_json`], [`bank_info`], [`akpk`], [`wav`].

pub mod akpk;
pub mod bank_info;
pub mod engine;
pub mod ffi;
pub mod proc_stats;
pub mod settings;
pub mod shortid;
pub mod sound_json;
pub mod wav;
