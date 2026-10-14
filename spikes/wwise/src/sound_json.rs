//! `StreamingAssets/Localization/Sound.json`: localized-string key -> Wwise event name.
//!
//! `LocalizedString.PlayCueVoiceOver` looks the cue's string key up in this pack
//! (`LocalizationManager.SoundPack`) and posts the resulting name as an event with
//! `AK_EndOfEvent`, on the dialogue speaker object (`SoundState.Get2DSoundObject`) unless a
//! target is given.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

/// The voice-over pack.
#[derive(Debug, Clone, Deserialize)]
pub struct SoundPack {
    strings: HashMap<String, String>,
}

impl SoundPack {
    /// Parses the pack from JSON text.
    ///
    /// # Errors
    ///
    /// If the text isn't a `{"strings": {key: event}}` object.
    pub fn from_json(text: &str) -> Result<Self> {
        serde_json::from_str(text).context("parsing Sound.json")
    }

    /// Reads and parses `Sound.json`.
    ///
    /// # Errors
    ///
    /// If the file can't be read or parsed.
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::from_json(&text)
    }

    /// The event for a localized-string key (`LocalizedString.m_Key`), if the line is voiced.
    #[must_use]
    pub fn event_for(&self, key: &str) -> Option<&str> {
        self.strings.get(key).map(String::as_str).filter(|s| !s.is_empty())
    }

    /// Number of voiced keys.
    #[must_use]
    pub fn len(&self) -> usize {
        self.strings.len()
    }

    /// Whether the pack is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::SoundPack;

    const SAMPLE: &str = r#"{
      "$id": "1",
      "strings": {
        "37dd6a74-125f-459d-9f2c-cacf563a1bcb": "CH0_MeetSeelahAnevia_01",
        "00000000-0000-0000-0000-000000000000": ""
      }
    }"#;

    #[test]
    fn maps_keys_to_events() {
        let pack = SoundPack::from_json(SAMPLE).unwrap();
        assert_eq!(pack.len(), 2);
        assert_eq!(
            pack.event_for("37dd6a74-125f-459d-9f2c-cacf563a1bcb"),
            Some("CH0_MeetSeelahAnevia_01")
        );
    }

    #[test]
    fn empty_and_unknown_keys_are_unvoiced() {
        let pack = SoundPack::from_json(SAMPLE).unwrap();
        assert_eq!(pack.event_for("00000000-0000-0000-0000-000000000000"), None);
        assert_eq!(pack.event_for("nope"), None);
    }
}
