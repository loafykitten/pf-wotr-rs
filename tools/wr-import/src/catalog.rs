//! The game's bundle catalog: `dependencylist.json` (bundle → bundles it needs),
//! `locationlist.json` (asset GUID → bundle), and our index of which bundle holds
//! each serialized file (`CAB-…` name → bundle), built by scanning bundle headers.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::unity::bundle::BundleHeader;

/// The common bundles the game loads at startup and never unloads
/// (`AssetBundleNames.CommonBundles`). `BundlesLoadService.LoadDependencies` skips them.
pub const COMMON_BUNDLES: [&str; 10] = [
    "shaders",
    "techart",
    "fx",
    "ui",
    "icons",
    "portraits",
    "equipment_prologue",
    "weapons.prefabs",
    "blueprint.assets",
    "humananimation.animations",
];

#[derive(Deserialize)]
struct KeyValues {
    #[serde(rename = "Key")]
    key: String,
    #[serde(rename = "Value")]
    value: Vec<String>,
}

#[derive(Deserialize)]
struct DependencyFile {
    #[serde(rename = "m_List")]
    list: Vec<KeyValues>,
}

#[derive(Deserialize)]
struct LocationFile {
    #[serde(rename = "m_Guids")]
    guids: Vec<String>,
    #[serde(rename = "m_Bundles")]
    bundles: Vec<String>,
}

/// `dependencylist.json`, as `BundlesLoadService.ReadDependencyList` reads it.
#[derive(Debug, Clone, Default)]
pub struct Dependencies {
    pub map: HashMap<String, Vec<String>>,
}

impl Dependencies {
    pub fn load(bundles_dir: &Path) -> Result<Self> {
        let p = bundles_dir.join("dependencylist.json");
        let f: DependencyFile = serde_json::from_slice(&std::fs::read(&p)?)
            .with_context(|| format!("parse {}", p.display()))?;
        Ok(Self { map: f.list.into_iter().map(|kv| (kv.key, kv.value)).collect() })
    }

    /// The transitive closure from `roots`, in breadth-first discovery order. This is
    /// the set `RequestBundle` ends up loading (recursively through `LoadDependencies`),
    /// counting common bundles and their dependencies, which are loaded at startup.
    #[must_use]
    pub fn closure(&self, roots: &[String]) -> Vec<String> {
        let mut seen = BTreeSet::new();
        let mut order = Vec::new();
        let mut queue: VecDeque<String> = roots.iter().cloned().collect();
        while let Some(b) = queue.pop_front() {
            if !seen.insert(b.clone()) {
                continue;
            }
            if let Some(deps) = self.map.get(&b) {
                queue.extend(deps.iter().cloned());
            }
            order.push(b);
        }
        order
    }
}

/// `locationlist.json`: resource asset GUID → bundle name.
#[derive(Debug, Clone, Default)]
pub struct Locations {
    pub guid_to_bundle: HashMap<String, String>,
}

impl Locations {
    pub fn load(bundles_dir: &Path) -> Result<Self> {
        let p = bundles_dir.join("locationlist.json");
        let f: LocationFile = serde_json::from_slice(&std::fs::read(&p)?)
            .with_context(|| format!("parse {}", p.display()))?;
        anyhow::ensure!(f.guids.len() == f.bundles.len(), "locationlist arrays differ in length");
        Ok(Self { guid_to_bundle: f.guids.into_iter().zip(f.bundles).collect() })
    }
}

/// Serialized-file name (`CAB-…`, `CAB-….sharedAssets`, `CAB-….resS`) → bundle file name.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CabIndex {
    pub cab_to_bundle: BTreeMap<String, String>,
    pub bundles_scanned: usize,
    pub errors: Vec<String>,
}

impl CabIndex {
    /// Scans every bundle header under `bundles_dir` (directory listings only).
    pub fn build(bundles_dir: &Path) -> Result<Self> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(bundles_dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.is_file()
                    && !p
                        .extension()
                        .is_some_and(|e| e == "json" || e == "bbp" || e == "tutorial_skip")
            })
            .collect();
        files.sort();
        let results: Vec<(String, Result<Vec<String>>)> = files
            .par_iter()
            .map(|p| {
                let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let r = BundleHeader::read_from_file(p)
                    .map(|h| h.nodes.into_iter().map(|n| n.path).collect());
                (name, r)
            })
            .collect();
        let mut idx = Self::default();
        for (bundle, r) in results {
            idx.bundles_scanned += 1;
            match r {
                Ok(nodes) => {
                    for n in nodes {
                        idx.cab_to_bundle.insert(n, bundle.clone());
                    }
                }
                Err(e) => idx.errors.push(format!("{bundle}: {e:#}")),
            }
        }
        Ok(idx)
    }

    /// Loads the cached index, or builds and caches it.
    pub fn load_or_build(bundles_dir: &Path, cache: &Path) -> Result<Self> {
        if let Ok(bytes) = std::fs::read(cache)
            && let Ok(idx) = serde_json::from_slice(&bytes)
        {
            return Ok(idx);
        }
        let idx = Self::build(bundles_dir)?;
        if let Some(dir) = cache.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(cache, serde_json::to_vec(&idx)?)?;
        Ok(idx)
    }

    #[must_use]
    pub fn bundle_of(&self, cab: &str) -> Option<&str> {
        self.cab_to_bundle.get(cab).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closure_is_transitive_and_cycle_safe() {
        let mut d = Dependencies::default();
        d.map.insert("a".into(), vec!["b".into(), "c".into()]);
        d.map.insert("b".into(), vec!["c".into(), "a".into()]);
        d.map.insert("c".into(), vec!["d".into()]);
        let c = d.closure(&["a".into()]);
        assert_eq!(c, ["a", "b", "c", "d"]);
    }
}
