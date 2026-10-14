//! Original `.zks` saves: the Newtonsoft object graph the game writes (`$id`/`$ref`
//! scoped per document, `$type` CLR names, the game's converters), decoded into typed
//! values through a schema generated from the decompiled save-side types, with a
//! lossless fallback for anything the schema doesn't cover; and our own save format.
//!
//! See `docs/poc3/saves.md`.

pub mod canon;
pub mod clr;
pub mod decode;
pub mod format;
#[rustfmt::skip]
#[allow(clippy::all, clippy::pedantic, clippy::unreadable_literal)]
mod generated;
pub mod json;
pub mod report;
pub mod schema;
pub mod value;
pub mod zks;

pub use json::Json;
pub use value::{Doc, Value};
