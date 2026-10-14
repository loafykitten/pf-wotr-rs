//! Wwise short IDs: 32-bit FNV-1 over the lower-cased name.
//!
//! This is `AK::SoundEngine::GetIDFromString` and the game's
//! `AkUtilities.ShortIDGenerator.Compute` (used for the audio device shareset). Event,
//! bank, switch and package IDs in `SoundbanksInfo.xml` and in `.pck` lookup tables are
//! all computed this way.

const OFFSET_BASIS: u32 = 2_166_136_261;
const PRIME: u32 = 16_777_619;

/// The short ID of `name` (ASCII case-insensitive, like Wwise).
#[must_use]
pub fn short_id(name: &str) -> u32 {
    name.bytes()
        .fold(OFFSET_BASIS, |hash, b| hash.wrapping_mul(PRIME) ^ u32::from(b.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::short_id;

    // Vectors from the game's own SoundbanksInfo.xml and Init.bnk.
    #[test]
    fn matches_the_game_ids() {
        assert_eq!(short_id("CH0_MeetSeelahAnevia_01"), 249_985_486); // event Id
        assert_eq!(short_id("CH0_MeetSeelahAnevia_02"), 249_985_485);
        assert_eq!(short_id("Dialogs_Prologue_ENG"), 247_461_405); // bank Id
        assert_eq!(short_id("System"), 3_859_886_410); // device shareset in Init.bnk
        assert_eq!(short_id("No_Output"), 2_317_455_096);
    }

    #[test]
    fn is_case_insensitive() {
        assert_eq!(short_id("ch0_meetseelahanevia_01"), short_id("CH0_MEETSEELAHANEVIA_01"));
    }
}
