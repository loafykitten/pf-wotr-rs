//! Cross-checks against AssetRipper's export (`cache/extract/<run>/ExportedProject`):
//! exported object counts by class, byte-level mesh comparison, and scene GameObject and
//! Transform values compared with our own bundle reader.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use crate::report::{MeshPrint, fnv1a};
use crate::scene::SceneData;
use crate::unity::serialized::class_name;

fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        if p.is_dir() {
            walk_files(&p, out)?;
        } else {
            out.push(p);
        }
    }
    Ok(())
}

fn hex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

/// `{x: 1, y: -2.5, z: 3}` → values in order.
fn inline_floats(s: &str) -> Vec<f32> {
    s.trim()
        .trim_start_matches('{')
        .trim_end_matches('}')
        .split(',')
        .filter_map(|kv| kv.split_once(':').and_then(|(_, v)| v.trim().parse().ok()))
        .collect()
}

/// One YAML document of a Unity text asset: class id, file id and its lines.
struct Doc<'a> {
    class: i32,
    file_id: i64,
    lines: Vec<&'a str>,
}

fn docs(text: &str) -> Vec<Doc<'_>> {
    let mut out: Vec<Doc<'_>> = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("--- !u!") {
            let mut it = rest.split_whitespace();
            let class = it.next().and_then(|c| c.parse().ok()).unwrap_or(-1);
            let file_id =
                it.next().and_then(|f| f.trim_start_matches('&').parse().ok()).unwrap_or(0);
            out.push(Doc { class, file_id, lines: Vec::new() });
        } else if let Some(d) = out.last_mut() {
            d.lines.push(line);
        }
    }
    out
}

fn field<'a>(d: &Doc<'a>, key: &str) -> Option<&'a str> {
    let pat = format!("  {key}:");
    d.lines.iter().find_map(|l| l.strip_prefix(&pat)).map(str::trim)
}

/// A YAML scalar as written by Unity/AssetRipper: plain, or single-quoted with `''`
/// escapes (used for values with `: `, brackets or trailing spaces).
fn unquote(s: &str) -> String {
    match s.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')) {
        Some(inner) => inner.replace("''", "'"),
        None => s.to_owned(),
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct MeshComparison {
    pub ours: usize,
    pub exported: usize,
    pub identical: usize,
    pub index_only: usize,
    pub different: usize,
    pub missing_in_export: usize,
    pub samples: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SceneComparison {
    pub scene: String,
    pub game_objects_ours: usize,
    pub game_objects_exported: usize,
    pub transforms_compared: usize,
    pub transforms_bit_identical: usize,
    pub transforms_within_1e6: usize,
    pub names_matching: usize,
    pub samples: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ArCheck {
    pub export_dir: String,
    pub files_by_extension: BTreeMap<String, usize>,
    pub exported_docs_by_class: BTreeMap<String, usize>,
    pub pruned_pngs: usize,
    /// PNGs (kept and pruned) plus textures exported as YAML.
    pub exported_textures: usize,
    pub pruned_png_bytes: u64,
    pub meshes: MeshComparison,
    pub scenes: Vec<SceneComparison>,
    /// Exported documents per bundle directory and class, for classes whose counts
    /// differ from ours: (bundle, class) → (ours, exported).
    pub per_bundle_differences: BTreeMap<String, (usize, usize)>,
}

/// Runs the comparison. `pruned` is the TSV of PNGs recorded then deleted to save disk.
pub fn check(
    export: &Path,
    pruned: Option<&Path>,
    meshes: &[MeshPrint],
    scenes: &[SceneData],
    ours_per_bundle: &BTreeMap<(String, String), usize>,
) -> Result<ArCheck> {
    let mut out = ArCheck { export_dir: export.display().to_string(), ..Default::default() };
    let assets = export.join("ExportedProject/Assets");
    let mut files = Vec::new();
    walk_files(&assets, &mut files)?;
    // Exported meshes grouped by (bundle dir, name) and by name alone.
    // Each exported mesh once: (index hash, vertex hash, consumed); both indexes point
    // into this list so a mesh can match only one of ours.
    let mut ar_list: Vec<(u64, u64, bool)> = Vec::new();
    let mut ar_meshes: HashMap<(String, String), Vec<usize>> = HashMap::new();
    let mut ar_by_name: HashMap<String, Vec<usize>> = HashMap::new();
    let mut exported_per_bundle: BTreeMap<(String, String), usize> = BTreeMap::new();
    for f in &files {
        let ext = f.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        *out.files_by_extension.entry(ext.clone()).or_default() += 1;
        if matches!(
            ext.as_str(),
            "meta" | "png" | "dll" | "cs" | "json" | "bytes" | "txt" | "asmdef"
        ) {
            continue;
        }
        let text = String::from_utf8_lossy(&std::fs::read(f)?).into_owned();
        let bundle_dir = f
            .strip_prefix(assets.join("AssetBundles"))
            .ok()
            .and_then(|r| r.components().next())
            .map_or_else(
                || "(scenes)".to_owned(),
                |c| c.as_os_str().to_string_lossy().into_owned(),
            );
        for d in docs(&text) {
            *out.exported_docs_by_class.entry(class_name(d.class)).or_default() += 1;
            *exported_per_bundle.entry((bundle_dir.clone(), class_name(d.class))).or_default() += 1;
            if d.class == 43 {
                let name = unquote(field(&d, "m_Name").unwrap_or_default());
                let index = field(&d, "m_IndexBuffer").and_then(hex).map_or(0, |b| fnv1a(&b));
                let vert = d
                    .lines
                    .iter()
                    .find_map(|l| l.trim_start().strip_prefix("_typelessdata:"))
                    .and_then(hex)
                    .map_or(0, |b| fnv1a(&b));
                let bundle = f
                    .strip_prefix(assets.join("AssetBundles"))
                    .ok()
                    .and_then(|r| r.components().next())
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .unwrap_or_default();
                let id = ar_list.len();
                ar_list.push((index, vert, false));
                ar_meshes.entry((bundle, name.clone())).or_default().push(id);
                ar_by_name.entry(name).or_default().push(id);
                out.meshes.exported += 1;
            }
        }
    }
    if let Some(p) = pruned
        && let Ok(t) = std::fs::read_to_string(p)
    {
        for l in t.lines() {
            out.pruned_pngs += 1;
            out.pruned_png_bytes +=
                l.split('\t').next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        }
    }
    // Textures are exported as images (PNG), not YAML: count both per bundle as one
    // "Texture (any)" class, and compare with our 2D, cube, array and 3D textures.
    let bundle_of_path = |p: &str| -> String {
        p.split("/AssetBundles/")
            .nth(1)
            .and_then(|r| r.split('/').next())
            .map_or_else(|| "(scenes)".to_owned(), str::to_owned)
    };
    let mut png_paths: Vec<String> = files
        .iter()
        .filter(|f| f.extension().is_some_and(|e| e == "png"))
        .map(|f| f.display().to_string())
        .collect();
    if let Some(p) = pruned
        && let Ok(t) = std::fs::read_to_string(p)
    {
        png_paths.extend(t.lines().filter_map(|l| l.split('\t').nth(1)).map(str::to_owned));
    }
    for p in &png_paths {
        *exported_per_bundle.entry((bundle_of_path(p), "Texture (any)".to_owned())).or_default() +=
            1;
    }
    let tex_classes = ["Texture2D", "Cubemap", "Texture2DArray", "Texture3D"];
    let yaml_tex: Vec<((String, String), usize)> = exported_per_bundle
        .iter()
        .filter(|((_, c), _)| tex_classes.contains(&c.as_str()))
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    for ((b, _), v) in yaml_tex {
        *exported_per_bundle.entry((b, "Texture (any)".to_owned())).or_default() += v;
    }
    let mut ours_per_bundle = ours_per_bundle.clone();
    let ours_tex: Vec<((String, String), usize)> = ours_per_bundle
        .iter()
        .filter(|((_, c), _)| tex_classes.contains(&c.as_str()))
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    for ((b, _), v) in ours_tex {
        *ours_per_bundle.entry((b, "Texture (any)".to_owned())).or_default() += v;
    }
    out.exported_textures = png_paths.len()
        + tex_classes
            .iter()
            .map(|c| out.exported_docs_by_class.get(*c).copied().unwrap_or(0))
            .sum::<usize>();
    for cls in ["Shader", "Material", "Mesh", "AnimationClip", "Sprite", "Texture (any)"] {
        let keys: std::collections::BTreeSet<&String> = ours_per_bundle
            .keys()
            .chain(exported_per_bundle.keys())
            .filter(|(_, c)| c == cls)
            .map(|(b, _)| b)
            .collect();
        for b in keys {
            let k = (b.clone(), cls.to_owned());
            let o = ours_per_bundle.get(&k).copied().unwrap_or(0);
            let e = exported_per_bundle.get(&k).copied().unwrap_or(0);
            if o != e {
                out.per_bundle_differences.insert(format!("{b} {cls}"), (o, e));
            }
        }
    }
    // Compare meshes.
    out.meshes.ours = meshes.len();
    for m in meshes {
        let key = (m.bundle.clone(), m.name.clone());
        let free = |ids: Option<&Vec<usize>>| -> Vec<usize> {
            ids.map(|v| v.iter().copied().filter(|&i| !ar_list[i].2).collect()).unwrap_or_default()
        };
        let mut pool = free(ar_meshes.get(&key));
        if pool.is_empty() {
            pool = free(ar_by_name.get(&m.name));
        }
        if pool.is_empty() {
            out.meshes.missing_in_export += 1;
            if out.meshes.samples.len() < 20 {
                out.meshes.samples.push(format!("missing: {} in {}", m.name, m.bundle));
            }
            continue;
        }
        let exact = pool
            .iter()
            .copied()
            .find(|&i| ar_list[i].0 == m.index_hash && ar_list[i].1 == m.vertex_hash);
        let index_only = pool.iter().copied().find(|&i| ar_list[i].0 == m.index_hash);
        let (chosen, note) = if let Some(i) = exact {
            out.meshes.identical += 1;
            (i, None)
        } else if let Some(i) = index_only {
            out.meshes.index_only += 1;
            (i, Some("vertex data differs"))
        } else {
            out.meshes.different += 1;
            (pool[0], Some("index data differs"))
        };
        ar_list[chosen].2 = true;
        if let Some(n) = note
            && out.meshes.samples.len() < 20
        {
            out.meshes.samples.push(format!("{n}: {} in {}", m.name, m.bundle));
        }
    }
    // Scenes.
    for s in scenes {
        let path = assets.join(format!("{}.unity", s.scene));
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let mut cmp = SceneComparison {
            scene: s.scene.clone(),
            game_objects_ours: s.game_object_count(),
            ..Default::default()
        };
        for d in docs(&text) {
            match d.class {
                1 => {
                    cmp.game_objects_exported += 1;
                    if let (Some(n), Some(ours)) =
                        (field(&d, "m_Name"), s.game_object_name(d.file_id))
                        && unquote(n) == ours
                    {
                        cmp.names_matching += 1;
                    }
                }
                4 => {
                    let (Some(p), Some(r), Some(sc)) = (
                        field(&d, "m_LocalPosition"),
                        field(&d, "m_LocalRotation"),
                        field(&d, "m_LocalScale"),
                    ) else {
                        continue;
                    };
                    let Some(ours) = s.local_trs(d.file_id) else {
                        if cmp.samples.len() < 10 {
                            cmp.samples.push(format!("transform {} not in our read", d.file_id));
                        }
                        continue;
                    };
                    let theirs: Vec<f32> =
                        [inline_floats(p), inline_floats(r), inline_floats(sc)].concat();
                    let mine: Vec<f32> = [
                        ours.position.to_array().to_vec(),
                        ours.rotation.to_array().to_vec(),
                        ours.scale.to_array().to_vec(),
                    ]
                    .concat();
                    cmp.transforms_compared += 1;
                    if theirs.len() == 10
                        && theirs.iter().zip(&mine).all(|(a, b)| a.to_bits() == b.to_bits())
                    {
                        cmp.transforms_bit_identical += 1;
                    }
                    if theirs.len() == 10
                        && theirs
                            .iter()
                            .zip(&mine)
                            .all(|(a, b)| (a - b).abs() <= 1e-6 * a.abs().max(1.0))
                    {
                        cmp.transforms_within_1e6 += 1;
                    } else if cmp.samples.len() < 10 {
                        cmp.samples.push(format!(
                            "transform {} differs: {theirs:?} vs {mine:?}",
                            d.file_id
                        ));
                    }
                }
                _ => {}
            }
        }
        out.scenes.push(cmp);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_yaml_docs_and_inline_maps() {
        let t = "%YAML 1.1\n--- !u!4 &12\nTransform:\n  m_LocalPosition: {x: 1.5, y: -2, z: 0}\n--- !u!1 &6\nGameObject:\n  m_Name: Door\n";
        let d = docs(t);
        assert_eq!(d.len(), 2);
        assert_eq!((d[0].class, d[0].file_id), (4, 12));
        assert_eq!(inline_floats(field(&d[0], "m_LocalPosition").unwrap()), [1.5, -2.0, 0.0]);
        assert_eq!(field(&d[1], "m_Name"), Some("Door"));
        assert_eq!(hex("00ff10"), Some(vec![0, 255, 16]));
        assert_eq!(hex("0"), None);
        assert_eq!(unquote("'Combined Mesh (root: scene) 2'"), "Combined Mesh (root: scene) 2");
        assert_eq!(unquote("'it''s'"), "it's");
        assert_eq!(unquote("plain"), "plain");
    }
}
