//! A GUID index over `blueprints.zip`, used to check that blueprint references found in
//! scenes point at blueprints that exist, and to name their types.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// What the index knows about one blueprint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlueprintInfo {
    /// Path of the `.jbp` inside the zip.
    pub path: String,
    /// The root type name from `Data.$type` (`"<type guid>, <TypeName>"`).
    pub type_name: String,
}

/// GUID (lowercase hex, 32 chars) → blueprint.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BlueprintIndex {
    pub by_guid: HashMap<String, BlueprintInfo>,
}

/// Reads `"key": "value"` from the head of a JSON document without parsing all of it.
fn json_head_string<'a>(head: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\"");
    let at = head.find(&pat)? + pat.len();
    let rest = head[at..].trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(&rest[..end])
}

/// Extracts `(guid, type name)` from the first bytes of a `.jbp` file.
#[must_use]
pub fn parse_head(head: &str) -> Option<(String, String)> {
    let guid = json_head_string(head, "AssetId")?.to_ascii_lowercase();
    let ty = json_head_string(head, "$type")?;
    let type_name = ty.split_once(',').map_or(ty, |(_, n)| n).trim().to_owned();
    Some((guid, type_name))
}

impl BlueprintIndex {
    /// Builds the index by reading the first 512 bytes of each `.jbp` (the `AssetId` and
    /// `$type` come first in every file).
    pub fn build(zip_path: &Path) -> Result<Self> {
        let names: Vec<String> = {
            let z = zip::ZipArchive::new(File::open(zip_path)?)?;
            z.file_names()
                .filter_map(Result::ok)
                .filter(|n| n.ends_with(".jbp"))
                .map(std::borrow::Cow::into_owned)
                .collect()
        };
        let chunks: Vec<&[String]> = names.chunks(4096).collect();
        let parts: Vec<Result<Vec<(String, BlueprintInfo)>>> = chunks
            .par_iter()
            .map(|chunk| {
                let mut z = zip::ZipArchive::new(File::open(zip_path)?)?;
                let mut out = Vec::with_capacity(chunk.len());
                let mut buf = Vec::with_capacity(512);
                for name in *chunk {
                    let entry = z.by_name(name)?;
                    buf.clear();
                    entry.take(512).read_to_end(&mut buf)?;
                    let head = String::from_utf8_lossy(&buf);
                    if let Some((guid, type_name)) = parse_head(&head) {
                        out.push((guid, BlueprintInfo { path: name.clone(), type_name }));
                    }
                }
                Ok(out)
            })
            .collect();
        let mut by_guid = HashMap::with_capacity(names.len());
        for p in parts {
            by_guid.extend(p?);
        }
        Ok(Self { by_guid })
    }

    pub fn load_or_build(zip_path: &Path, cache: &Path) -> Result<Self> {
        if let Ok(bytes) = std::fs::read(cache)
            && let Ok(idx) = serde_json::from_slice(&bytes)
        {
            return Ok(idx);
        }
        let idx = Self::build(zip_path).with_context(|| format!("index {}", zip_path.display()))?;
        if let Some(dir) = cache.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(cache, serde_json::to_vec(&idx)?)?;
        Ok(idx)
    }

    #[must_use]
    pub fn get(&self, guid: &str) -> Option<&BlueprintInfo> {
        self.by_guid.get(&guid.to_ascii_lowercase())
    }
}

/// Reads whole blueprints from the zip on demand.
#[derive(Debug)]
pub struct BlueprintReader {
    zip: zip::ZipArchive<File>,
}

impl BlueprintReader {
    pub fn open(zip_path: &Path) -> Result<Self> {
        Ok(Self { zip: zip::ZipArchive::new(File::open(zip_path)?)? })
    }

    /// The parsed JSON of the blueprint at `path` inside the zip.
    pub fn json(&mut self, path: &str) -> Result<serde_json::Value> {
        let mut s = String::new();
        self.zip.by_name(path)?.read_to_string(&mut s)?;
        // Some files start with a UTF-8 BOM.
        Ok(serde_json::from_str(s.trim_start_matches('\u{feff}'))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_jbp_head() {
        let head = "{\n  \"AssetId\": \"0F083f2598b3e6441992ebadbc0325aa\",\n  \"Data\": {\n    \"$type\": \"6370c9530f88b5c41bd101d35b8e4fd2, BlueprintProjectile\",\n";
        let (g, t) = parse_head(head).unwrap();
        assert_eq!(g, "0f083f2598b3e6441992ebadbc0325aa");
        assert_eq!(t, "BlueprintProjectile");
        assert!(parse_head("{\"Data\": {}}").is_none());
    }
}
