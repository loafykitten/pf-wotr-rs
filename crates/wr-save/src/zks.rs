//! `.zks` archives: a zip of documents (`ZipSaver`, Ionic Zip). Each `.json` entry is a
//! separate Newtonsoft document with its own `$id` scope:
//!
//! | Entry | Root type | Written by |
//! |---|---|---|
//! | `header.json` | `SaveInfo` | `SaveManager` (with a UTF-8 BOM) |
//! | `player.json` | `Player` | `SerializeAndSaveThread` |
//! | `party.json` | `SceneEntitiesState` (cross-scene: the party) | same, through `SceneEntitiesStateConverter` |
//! | `<area guid>.json` | `AreaPersistentState` | `AreaDataStash.StashAreaState` |
//! | `<area guid><scene>.json` | `SceneEntitiesState` | same, one per additional scene |
//! | `tbm.json` | `CombatController`, or just a BOM outside turn-based combat | `SerializeAndSaveThread` (the loader never reads it) |
//! | `clockwork.json`, `statistic.json` | QA only | |
//! | `<area guid>.<scene>.fog` | fog-of-war mask image | `SavedFogMasks.SaveAll` |
//! | `header.png`, `history`, `checksums` | screenshot, game log text, debug MD5s | |
//!
//! Non-JSON entries are kept as bytes.

use std::io::Read as _;
use std::path::Path;

use anyhow::{Context, Result};
use rayon::prelude::*;

use crate::decode::{self, Stats};
use crate::json;
use crate::value::Doc;

/// Largest entry we inflate (Manual_10's biggest is 26 MB).
pub const MAX_ENTRY: u64 = 1 << 30;
/// Largest whole archive we inflate.
pub const MAX_TOTAL: u64 = 4 << 30;

/// One archive entry as stored.
#[derive(Debug, Clone)]
pub struct RawEntry {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// Reads every entry of a `.zks` (in archive order).
///
/// # Errors
///
/// If the file isn't a readable zip.
pub fn read(path: &Path) -> Result<Vec<RawEntry>> {
    let f = std::fs::File::open(path).with_context(|| path.display().to_string())?;
    let mut z = zip::ZipArchive::new(std::io::BufReader::new(f))
        .with_context(|| format!("{} is not a zip", path.display()))?;
    let mut out = Vec::with_capacity(z.len().min(1 << 16));
    let mut total = 0u64;
    for i in 0..z.len() {
        let e = z.by_index(i)?;
        let name = e.name()?.into_owned();
        // Sizes in the archive are claims: cap the reservation and the inflated bytes.
        let claimed = usize::try_from(e.size()).unwrap_or(usize::MAX).min(64 << 20);
        let mut bytes = Vec::with_capacity(claimed);
        e.take(MAX_ENTRY + 1).read_to_end(&mut bytes)?;
        anyhow::ensure!(
            bytes.len() as u64 <= MAX_ENTRY,
            "{name}: entry larger than {MAX_ENTRY} bytes"
        );
        total += bytes.len() as u64;
        anyhow::ensure!(total <= MAX_TOTAL, "archive inflates past {MAX_TOTAL} bytes");
        out.push(RawEntry { name, bytes });
    }
    Ok(out)
}

/// What an entry holds after decoding.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Body {
    /// A JSON document, decoded.
    Doc(Doc),
    /// Kept as stored: binary entries, blank JSON (`tbm.json`), and JSON that failed to
    /// parse (see [`Decoded::error`]).
    Bytes(Vec<u8>),
}

#[derive(Debug)]
pub struct Decoded {
    pub name: String,
    pub body: Body,
    /// The declared root type, for JSON documents.
    pub root: Option<&'static str>,
    pub stats: Option<Stats>,
    pub error: Option<String>,
}

/// Decodes one entry.
#[must_use]
pub fn decode_entry(e: &RawEntry) -> Decoded {
    let is_json =
        std::path::Path::new(&e.name).extension().is_some_and(|x| x.eq_ignore_ascii_case("json"));
    if !is_json || json::is_blank(&e.bytes) {
        return Decoded {
            name: e.name.clone(),
            body: Body::Bytes(e.bytes.clone()),
            root: None,
            stats: None,
            error: None,
        };
    }
    let root = decode::root_type(&e.name);
    match json::parse(&e.bytes) {
        Ok(doc) => {
            let (d, stats) = decode::decode(&doc, decode::root_index(&e.name));
            Decoded {
                name: e.name.clone(),
                body: Body::Doc(d),
                root,
                stats: Some(stats),
                error: None,
            }
        }
        Err(err) => Decoded {
            name: e.name.clone(),
            body: Body::Bytes(e.bytes.clone()),
            root,
            stats: None,
            error: Some(err.to_string()),
        },
    }
}

/// Decodes all entries in parallel (documents are independent: each is its own
/// `$id` scope).
#[must_use]
pub fn decode_all(entries: &[RawEntry]) -> Vec<Decoded> {
    entries.par_iter().map(decode_entry).collect()
}

/// `SaveInfo.Versions` from `header.json`, if readable.
#[must_use]
pub fn header_versions(entries: &[RawEntry]) -> Option<Vec<i64>> {
    let h = entries.iter().find(|e| e.name == "header.json")?;
    let d = json::parse(&h.bytes).ok()?;
    // Newtonsoft applies every `Versions` property in turn: the last one wins.
    let json::Json::Obj(props) = &d.root else { return None };
    let last = props.iter().rev().find(|(k, _)| &**k == "Versions").map(|(_, v)| v)?;
    match last {
        // Only canonical decimal integers: anything else (octal `010`, `+1`) we refuse
        // rather than guess.
        json::Json::Arr(items) => items
            .iter()
            .map(|x| match x {
                json::Json::Num(n) => n.parse::<i64>().ok().filter(|v| v.to_string() == **n),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

/// `JsonUpgradeSystem`'s registered upgraders (version numbers). A save missing any of
/// them is upgraded on load by text transforms we don't implement.
pub const KNOWN_UPGRADERS: [i64; 6] = [10, 20, 30, 40, 50, 60];

/// Upgraders the game would run on this save before deserializing it.
#[must_use]
pub fn pending_upgraders(entries: &[RawEntry]) -> Vec<i64> {
    let have = header_versions(entries).unwrap_or_default();
    KNOWN_UPGRADERS.iter().copied().filter(|v| !have.contains(v)).collect()
}

/// Fails closed unless the save loads without text upgrades: `header.json` is readable,
/// lists every registered upgrader and no unknown version (`JsonUpgradeSystem` throws on
/// an unknown one).
///
/// # Errors
///
/// A save the game would upgrade or reject, or an unreadable header.
pub fn check_versions(entries: &[RawEntry]) -> Result<()> {
    let have = header_versions(entries).context("header.json has no readable Versions list")?;
    let unknown: Vec<i64> = have.iter().copied().filter(|v| !KNOWN_UPGRADERS.contains(v)).collect();
    anyhow::ensure!(
        unknown.is_empty(),
        "unknown save versions {unknown:?}: the game refuses this save"
    );
    let pending = pending_upgraders(entries);
    anyhow::ensure!(
        pending.is_empty(),
        "the game would run JSON upgraders {pending:?} before loading; not implemented"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(text: &str) -> Vec<RawEntry> {
        vec![RawEntry { name: "header.json".into(), bytes: text.as_bytes().to_vec() }]
    }

    #[test]
    fn version_guard() {
        assert!(check_versions(&header(r#"{"Versions":[10,20,30,40,50,60]}"#)).is_ok());
        assert!(check_versions(&header(r#"{"Versions":[10,20,30,40,50]}"#)).is_err());
        assert!(check_versions(&header(r#"{"Versions":[10,20,30,40,50,60,999]}"#)).is_err());
        // The last `Versions` wins, as Newtonsoft applies each in turn.
        assert!(
            check_versions(&header(r#"{"Versions":[10,20,30,40,50,60],"Versions":[999]}"#))
                .is_err()
        );
        // Octal or other non-canonical numbers are refused.
        assert!(check_versions(&header(r#"{"Versions":[010,20,30,40,50,60]}"#)).is_err());
        assert!(check_versions(&[]).is_err());
    }
}
