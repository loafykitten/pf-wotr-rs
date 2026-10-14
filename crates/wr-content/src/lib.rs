//! Blueprint content: the game's `blueprints.zip` decoded through schemas generated from
//! its C# declarations (`tools/wr-codegen`), with an opaque fallback that keeps anything
//! the schemas don't cover, reference checking, a canonical form for comparing with the
//! live game, and a memory-mapped content pack.
//!
//! See `docs/poc2/content.md`.

pub mod bbp;
pub mod canon;
pub mod compare;
pub mod decode;
#[rustfmt::skip]
#[allow(clippy::all, clippy::pedantic, clippy::unreadable_literal)]
mod generated;
pub mod guid;
pub mod node;
pub mod pack;
pub mod packfmt;
pub mod refs;
pub mod report;
pub mod schema;
pub mod source;

pub use guid::Guid;
pub use node::Node;
