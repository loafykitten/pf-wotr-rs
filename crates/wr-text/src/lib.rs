//! Text: the game's localization packs and its text template engine.
//!
//! - [`pack`]: `StreamingAssets/Localization/<locale>.json` read once into our own
//!   memory-mapped format (`cache/text/<locale>.wrtext`), with `LocalizationPack.GetText`'s
//!   lookups;
//! - [`engine`]: `TextTemplateEngine.Process` and its templates, the game state they read
//!   asked of an [`Env`] that fails closed by default;
//! - [`display`]: a `LocalizedString` turned into a `string`, as the game shows it.
//!
//! See `docs/poc4/text.md`.

#[rustfmt::skip]
mod casing;
pub mod engine;
pub mod pack;

pub use engine::{
    Color32, Env, GameTime, Gender, LogIntField, LogRoll, LogRollField, LogStrField, LogTag, LogUnit,
    LogUnitField, NoEnv, TextError, TooltipType, process,
};
pub use pack::{Locale, Pack};

/// The implicit `LocalizedString` → `string` conversion for a string whose actual key is
/// `key`: the pack's text (`[unknown key: …]` for a missing key, "" for an empty key or no
/// pack), processed when it holds a `{`.
///
/// # Errors
///
/// As [`process`], and [`TextError::Pack`] for a damaged pack.
pub fn display(pack: Option<&Pack>, key: &str, env: &mut dyn Env) -> Result<String, TextError> {
    let text = pack::load_string(pack, key).map_err(|e| TextError::Pack(format!("{e:#}")))?;
    if text.contains('{') { process(&text, env) } else { Ok(text) }
}

#[cfg(test)]
mod tests;
