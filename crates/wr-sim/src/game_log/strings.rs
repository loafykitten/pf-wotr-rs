//! The log's strings: `BlueprintRoot.LocalizedTexts`' string containers (`GameLogStrings` and
//! the others the log reads), `StringsContainer` assets outside the blueprints. The runtime pack
//! holds each as an index into `BlueprintReferencedAssets`; `tools/wr-rules-data` follows it into
//! `Bundles/blueprint.assets` and writes `cache/sim/log-strings.json`: each container's fields as
//! the type tree decodes them, every `LocalizedString` reduced to its actual key
//! (`{"$key": …}`: `m_Key`, or the end of its `Shared` chain, as `LocalizedString.GetActualKey`).
//! No text is stored: the keys are looked up in the localization pack.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde_json::Value as J;
use wr_text::Color32;

pub const LOG_STRINGS_SCHEMA: &str = "wr-log-strings-v1";

/// The extracted containers.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct LogStrings {
    pub schema: String,
    pub source: BTreeMap<String, J>,
    /// By `LocalizedTexts` field name (`GameLog`, …).
    pub containers: BTreeMap<String, J>,
}

impl LogStrings {
    /// `cache/sim/log-strings.json`.
    #[must_use]
    pub fn default_path() -> std::path::PathBuf {
        wr_paths::cache_root().join("sim/log-strings.json")
    }

    /// Loads the extracted table.
    ///
    /// # Errors
    ///
    /// If it's missing (run `wr-rules-data`), unreadable or another schema.
    pub fn load(path: &std::path::Path) -> Result<Self> {
        let b = std::fs::read(path)
            .with_context(|| format!("{} (run wr-rules-data)", path.display()))?;
        let s: Self = serde_json::from_slice(&b).context("log-strings.json")?;
        anyhow::ensure!(s.schema == LOG_STRINGS_SCHEMA, "log-strings.json: schema {}", s.schema);
        Ok(s)
    }

    /// A container's field, by a `/`-separated path (`GameLog/Attack/Message`; an array's
    /// element by its index, `Stats/Entries/3/Text`).
    #[must_use]
    pub fn get(&self, path: &str) -> Option<&J> {
        let mut parts = path.split('/');
        let mut v = self.containers.get(parts.next()?)?;
        for p in parts {
            v = match v {
                J::Array(a) => a.get(p.parse::<usize>().ok()?)?,
                _ => v.get(p)?,
            };
        }
        Some(v)
    }

    /// A `LocalizedString` field's actual key (`""` for an empty key).
    ///
    /// # Errors
    ///
    /// When the path is missing or isn't a localized string.
    pub fn key(&self, path: &str) -> Result<&str, String> {
        self.get(path)
            .and_then(|v| v.get("$key"))
            .and_then(J::as_str)
            .ok_or_else(|| format!("log strings: {path} isn't a localized string"))
    }

    /// A `Color32` field.
    ///
    /// # Errors
    ///
    /// When the path is missing or isn't a colour.
    pub fn color(&self, path: &str) -> Result<Color32, String> {
        let v = self.get(path).ok_or_else(|| format!("log strings: {path} missing"))?;
        color32(v).ok_or_else(|| format!("log strings: {path} isn't a Color32"))
    }
}

/// A `Color32` as the type tree decodes it: `r g b a` bytes, or the packed `rgba` (`r` in the
/// low byte).
#[must_use]
pub fn color32(v: &J) -> Option<Color32> {
    let byte = |k: &str| v.get(k).and_then(J::as_u64).and_then(|x| u8::try_from(x).ok());
    if let (Some(r), Some(g), Some(b), Some(a)) = (byte("r"), byte("g"), byte("b"), byte("a")) {
        return Some(Color32 { r, g, b, a });
    }
    let p = v.get("rgba").and_then(J::as_u64).and_then(|x| u32::try_from(x).ok())?;
    let [r, g, b, a] = p.to_le_bytes();
    Some(Color32 { r, g, b, a })
}
