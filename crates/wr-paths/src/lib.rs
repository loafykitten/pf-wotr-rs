//! Locations of the original game's files and of this project's local caches.
//!
//! Everything is read-only with respect to the Steam install: callers must never write
//! under [`GamePaths::app`]. Each path can be overridden with an environment variable,
//! which is how tests and the oracle point at the APFS clone in
//! `builds/arm64-oracle` (gitignored) instead of Steam's copy.

use std::env;
use std::path::{Path, PathBuf};

/// Steam's install of the game, relative to `$HOME`.
const STEAM_APP: &str =
    "Library/Application Support/Steam/steamapps/common/Pathfinder Second Adventure/Wrath.app";
/// The game's persistent data folder (saves, logs written by the game), relative to `$HOME`.
const PERSISTENT: &str =
    "Library/Application Support/Owlcat Games/Pathfinder Wrath Of The Righteous";

/// Resolved locations of the original game's data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GamePaths {
    /// `Wrath.app`. Override: `WRATH_APP`.
    pub app: PathBuf,
    /// The game's persistent data folder. Override: `WRATH_PERSISTENT`.
    pub persistent: PathBuf,
}

impl GamePaths {
    /// Resolves paths from the environment, falling back to Steam's default locations.
    ///
    /// # Panics
    ///
    /// If `HOME` is unset and an override is missing.
    #[must_use]
    pub fn from_env() -> Self {
        let home = || PathBuf::from(env::var_os("HOME").expect("HOME is not set"));
        let app = env::var_os("WRATH_APP").map_or_else(|| home().join(STEAM_APP), PathBuf::from);
        let persistent =
            env::var_os("WRATH_PERSISTENT").map_or_else(|| home().join(PERSISTENT), PathBuf::from);
        Self { app, persistent }
    }

    /// `Contents/Resources/Data`, Unity's data folder.
    #[must_use]
    pub fn data(&self) -> PathBuf {
        self.app.join("Contents/Resources/Data")
    }

    /// `StreamingAssets`: localization, Wwise audio, animation data.
    #[must_use]
    pub fn streaming_assets(&self) -> PathBuf {
        self.data().join("StreamingAssets")
    }

    /// `Wrath.app/Bundles`: the game's asset bundles (`.scenes`, `.nav`, `.unit`, …) and the blueprint pack.
    #[must_use]
    pub fn bundles(&self) -> PathBuf {
        self.app.join("Bundles")
    }

    /// `blueprints.zip` (at the root of `Wrath.app`), our source for content.
    #[must_use]
    pub fn blueprints_zip(&self) -> PathBuf {
        self.app.join("blueprints.zip")
    }

    /// Managed assemblies (`Assembly-CSharp.dll`, …).
    #[must_use]
    pub fn managed(&self) -> PathBuf {
        self.data().join("Managed")
    }

    /// The game's own saves. Read only; we write our saves elsewhere.
    #[must_use]
    pub fn saves(&self) -> PathBuf {
        self.persistent.join("Saved Games")
    }
}

/// This repository's root.
#[must_use]
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Root of this repository's gitignored local caches (`<repo>/cache`). Override:
/// `WR_CACHE`.
#[must_use]
pub fn cache_root() -> PathBuf {
    env::var_os("WR_CACHE").map_or_else(|| repo_root().join("cache"), PathBuf::from)
}

/// Ember's fixture saves, read-only copies (`<repo>/fixtures/saves`, gitignored). Override:
/// `WR_FIXTURE_SAVES`.
#[must_use]
pub fn fixture_saves() -> PathBuf {
    env::var_os("WR_FIXTURE_SAVES")
        .map_or_else(|| repo_root().join("fixtures/saves"), PathBuf::from)
}

/// The decompiled game code, a reading reference (`<repo>/decompiled`, gitignored, never
/// committed). Override: `WR_DECOMPILED`.
#[must_use]
pub fn decompiled() -> PathBuf {
    env::var_os("WR_DECOMPILED").map_or_else(|| repo_root().join("decompiled"), PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_paths_hang_off_the_app() {
        let p = GamePaths { app: "/x/Wrath.app".into(), persistent: "/p".into() };
        assert_eq!(p.bundles(), Path::new("/x/Wrath.app/Bundles"));
        assert_eq!(p.blueprints_zip(), Path::new("/x/Wrath.app/blueprints.zip"));
        assert_eq!(p.saves(), Path::new("/p/Saved Games"));
    }
}
