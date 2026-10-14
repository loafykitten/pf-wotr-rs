//! Unity 2020.3 `Shader` and `ComputeShader` objects: the parsed form (passes, tags,
//! render state, properties), and the Metal subprograms stored as MSL text in the
//! LZ4-compressed blob.
//!
//! Blob layout (per platform, split into segments of up to ~16 MiB): segment 0 starts
//! with an entry count and `(offset, length, segment)` triples; each entry is one
//! subprogram: version, GPU program type, four statistics, global then local keyword
//! names (length-prefixed, 4-aligned), then the code (a small Metal header, the MSL
//! text, and Unity's binding table).

use std::collections::{BTreeSet, HashMap};

use anyhow::{Context, Result, bail, ensure};
use wr_assets::{BlendState, PassState, ShaderPass, ShaderProperty, StateValue};

use crate::unity::reader::{Endian, Reader};
use crate::unity::value::Value;

/// `ShaderCompilerPlatform.Metal`.
pub const PLATFORM_METAL: i64 = 14;

/// One subprogram (variant of one stage of one pass).
#[derive(Debug, Clone)]
pub struct SubProgram {
    pub subshader: usize,
    pub pass: usize,
    pub stage: wr_assets::Stage,
    pub blob_index: usize,
    pub gpu_program_type: i32,
    pub keywords: BTreeSet<String>,
    pub msl: String,
}

#[derive(Debug, Clone)]
pub struct DecodedPass {
    pub info: ShaderPass,
    /// `progVertex`, `progFragment` subprogram blob indices.
    pub vertex: Vec<usize>,
    pub fragment: Vec<usize>,
}

/// A decoded shader. Subprograms are decoded on demand (`subprogram`).
#[derive(Debug)]
pub struct DecodedShader {
    pub name: String,
    pub properties: Vec<ShaderProperty>,
    /// Passes of the first subshader (Owlcat's shaders have one subshader per platform
    /// tier; we use subshader 0 as Unity does when it is supported).
    pub passes: Vec<DecodedPass>,
    segments: Vec<Vec<u8>>,
    entries: Vec<(usize, usize, usize)>,
    cache: HashMap<usize, (i32, BTreeSet<String>, String)>,
}

fn float_value(v: Option<&Value>) -> StateValue {
    let Some(v) = v else { return StateValue::default() };
    // Unity writes `<noninit>` for values not bound to a property.
    let name = v.str_at("name").unwrap_or_default();
    StateValue {
        value: v.f32_at("val").unwrap_or(0.0),
        property: (!name.is_empty() && name != "<noninit>").then(|| name.to_owned()),
    }
}

fn pass_state(s: &Value) -> PassState {
    let mut blend = Vec::new();
    for i in 0..8 {
        let Some(b) = s.get(&format!("rtBlend{i}")) else { break };
        blend.push(BlendState {
            src: float_value(b.get("srcBlend")),
            dst: float_value(b.get("destBlend")),
            src_alpha: float_value(b.get("srcBlendAlpha")),
            dst_alpha: float_value(b.get("destBlendAlpha")),
            op: float_value(b.get("blendOp")),
            op_alpha: float_value(b.get("blendOpAlpha")),
            color_mask: float_value(b.get("colMask")),
        });
    }
    let op = |k: &str| -> [StateValue; 4] {
        let o = s.get(k);
        let g = |f: &str| float_value(o.and_then(|o| o.get(f)));
        [g("pass"), g("fail"), g("zFail"), g("comp")]
    };
    PassState {
        blend,
        separate_blend: s.get("rtSeparateBlend").and_then(Value::as_bool).unwrap_or(false),
        z_test: float_value(s.get("zTest")),
        z_write: float_value(s.get("zWrite")),
        cull: float_value(s.get("culling")),
        offset_factor: float_value(s.get("offsetFactor")),
        offset_units: float_value(s.get("offsetUnits")),
        alpha_to_mask: float_value(s.get("alphaToMask")),
        stencil_ref: float_value(s.get("stencilRef")),
        stencil_read_mask: float_value(s.get("stencilReadMask")),
        stencil_write_mask: float_value(s.get("stencilWriteMask")),
        stencil_front: op("stencilOpFront"),
        stencil_back: op("stencilOpBack"),
    }
}

fn tags(v: Option<&Value>) -> Vec<(String, String)> {
    v.and_then(|t| t.get("tags"))
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .map(|p| {
            (
                p.str_at("first").unwrap_or_default().to_owned(),
                p.str_at("second").unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

fn u32_list(v: Option<&Value>) -> Vec<u64> {
    match v {
        Some(Value::Array(a)) => a
            .iter()
            .flat_map(|x| match x {
                Value::Array(inner) => {
                    inner.iter().filter_map(Value::as_i64).map(|i| i as u64).collect::<Vec<u64>>()
                }
                other => other.as_i64().map(|i| i as u64).into_iter().collect(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn properties(form: &Value) -> Vec<ShaderProperty> {
    form.path("m_PropInfo.m_Props")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .map(|p| {
            let mut d = [0f32; 4];
            for (i, slot) in d.iter_mut().enumerate() {
                *slot = p.f32_at(&format!("m_DefValue[{i}]")).unwrap_or(0.0);
            }
            let kind = p.i64_at("m_Type").unwrap_or(-1) as i32;
            ShaderProperty {
                name: p.str_at("m_Name").unwrap_or_default().to_owned(),
                kind,
                default_value: d,
                flags: u32::try_from(p.i64_at("m_Flags").unwrap_or(0)).unwrap_or(0),
                default_texture: (kind == 4)
                    .then(|| p.str_at("m_DefTexture.m_DefaultName").unwrap_or_default().to_owned()),
            }
        })
        .collect()
}

impl DecodedShader {
    /// Decodes a `Shader` object's parsed form and its Metal blob segments.
    pub fn decode(v: &Value) -> Result<Self> {
        let form = v.get("m_ParsedForm").context("Shader without m_ParsedForm")?;
        let name = form.str_at("m_Name").unwrap_or_default().to_owned();
        let platforms: Vec<i64> = v
            .get("platforms")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_i64)
            .collect();
        let pi = platforms
            .iter()
            .position(|&p| p == PLATFORM_METAL)
            .with_context(|| format!("{name}: no Metal programs"))?;
        let nth = |k: &str| -> Vec<u64> {
            match v.get(k).and_then(Value::as_array).and_then(|a| a.get(pi)) {
                Some(x @ Value::Array(_)) => u32_list(Some(x)),
                Some(x) => x.as_i64().map(|i| i as u64).into_iter().collect(),
                None => Vec::new(),
            }
        };
        let (offs, clen, dlen) =
            (nth("offsets"), nth("compressedLengths"), nth("decompressedLengths"));
        ensure!(
            offs.len() == clen.len() && clen.len() == dlen.len() && !offs.is_empty(),
            "{name}: inconsistent blob segment tables"
        );
        let blob =
            v.get("compressedBlob").and_then(Value::as_bytes).context("no compressedBlob")?;
        let mut segments = Vec::new();
        for ((&o, &c), &d) in offs.iter().zip(&clen).zip(&dlen) {
            let (o, c, d) = (usize::try_from(o)?, usize::try_from(c)?, usize::try_from(d)?);
            let src = blob.get(o..o + c).context("blob segment out of range")?;
            let out = lz4_flex::block::decompress(src, d)
                .map_err(|e| anyhow::anyhow!("{name}: lz4: {e}"))?;
            ensure!(out.len() == d, "{name}: segment decompressed to {} != {d}", out.len());
            segments.push(out);
        }
        let mut r = Reader::new(&segments[0], Endian::Little);
        let n = r.len_i32(12)?;
        let mut entries = Vec::with_capacity(n);
        for _ in 0..n {
            let (o, l, s) = (r.i32()?, r.i32()?, r.i32()?);
            entries.push((usize::try_from(o)?, usize::try_from(l)?, usize::try_from(s)?));
        }
        let mut passes = Vec::new();
        let subshaders = form.get("m_SubShaders").and_then(Value::as_array).unwrap_or_default();
        if let Some(ss) = subshaders.first() {
            for p in ss.get("m_Passes").and_then(Value::as_array).unwrap_or_default() {
                let state = p.get("m_State").context("pass without m_State")?;
                let blobs = |k: &str| -> Vec<usize> {
                    p.path(&format!("{k}.m_SubPrograms"))
                        .and_then(Value::as_array)
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|s| s.i64_at("m_BlobIndex"))
                        .filter_map(|i| usize::try_from(i).ok())
                        .collect()
                };
                let mut t = tags(p.get("m_Tags"));
                if t.is_empty() {
                    t = tags(state.get("m_Tags"));
                }
                passes.push(DecodedPass {
                    info: ShaderPass {
                        name: state.str_at("m_Name").unwrap_or_default().to_owned(),
                        tags: t,
                        state: pass_state(state),
                    },
                    vertex: blobs("progVertex"),
                    fragment: blobs("progFragment"),
                });
            }
        }
        Ok(Self {
            name,
            properties: properties(form),
            passes,
            segments,
            entries,
            cache: HashMap::new(),
        })
    }

    /// Decodes one subprogram: GPU program type, keywords, MSL text.
    pub fn subprogram(&mut self, blob_index: usize) -> Result<(i32, BTreeSet<String>, String)> {
        if let Some(c) = self.cache.get(&blob_index) {
            return Ok(c.clone());
        }
        let &(o, l, s) = self.entries.get(blob_index).context("blob index out of range")?;
        let seg = self.segments.get(s).context("segment out of range")?;
        let data = seg.get(o..o + l).context("subprogram out of range")?;
        let parsed = parse_subprogram(data)?;
        self.cache.insert(blob_index, parsed.clone());
        Ok(parsed)
    }

    /// Index of the pass with this name or `LightMode` tag.
    #[must_use]
    pub fn pass_index(&self, name: &str) -> Option<usize> {
        self.passes.iter().position(|p| {
            p.info.name.eq_ignore_ascii_case(name) || p.info.light_mode().eq_ignore_ascii_case(name)
        })
    }
}

fn read_str(r: &mut Reader<'_>) -> Result<String> {
    let n = r.len_i32(1)?;
    let s = String::from_utf8_lossy(r.take(n)?).into_owned();
    r.align(4)?;
    Ok(s)
}

/// Strips the Metal header and Unity's binding table: the MSL text runs from the first
/// `#include` to the first NUL.
fn msl_text(code: &[u8]) -> Result<String> {
    let start =
        code.windows(8).position(|w| w == b"#include").context("no MSL in subprogram code")?;
    let end = code[start..].iter().position(|&b| b == 0).map_or(code.len(), |e| start + e);
    Ok(String::from_utf8_lossy(&code[start..end]).into_owned())
}

fn parse_subprogram(data: &[u8]) -> Result<(i32, BTreeSet<String>, String)> {
    let mut r = Reader::new(data, Endian::Little);
    let _version = r.i32()?;
    let gtype = r.i32()?;
    for _ in 0..4 {
        r.i32()?;
    }
    let mut kw = BTreeSet::new();
    for _ in 0..2 {
        let n = r.len_i32(4)?;
        for _ in 0..n {
            kw.insert(read_str(&mut r)?);
        }
    }
    let clen = r.len_i32(1)?;
    let code = r.take(clen)?;
    Ok((gtype, kw, msl_text(code)?))
}

/// A `ComputeShader` kernel's Metal code.
#[derive(Debug, Clone)]
pub struct Kernel {
    pub name: String,
    pub keywords: String,
    pub threads: [u32; 3],
    pub msl: String,
}

/// Kernels of a `ComputeShader` for Metal (`targetRenderer` 16).
pub fn compute_kernels(v: &Value) -> Result<Vec<Kernel>> {
    let mut out = Vec::new();
    for var in v.get("variants").and_then(Value::as_array).unwrap_or_default() {
        if var.i64_at("targetRenderer") != Some(16) {
            continue;
        }
        for k in var.get("kernels").and_then(Value::as_array).unwrap_or_default() {
            let name = k.str_at("name").unwrap_or_default().to_owned();
            for entry in k.get("variantMap").and_then(Value::as_array).unwrap_or_default() {
                let keywords = entry.str_at("first").unwrap_or_default().to_owned();
                let Some(body) = entry.get("second") else { continue };
                let code = body.get("code").and_then(Value::as_bytes).context("kernel code")?;
                // Header: magic 0xdeadcafe, version, header size, then u16 thread counts.
                ensure!(code.len() > 20, "kernel code too short");
                let u16_at = |o: usize| u32::from(u16::from_le_bytes([code[o], code[o + 1]]));
                let threads = [u16_at(12), u16_at(14), u16_at(16)];
                out.push(Kernel { name: name.clone(), keywords, threads, msl: msl_text(code)? });
            }
        }
    }
    if out.is_empty() {
        bail!("no Metal kernels");
    }
    Ok(out)
}

/// Unity's variant choice for one stage: the subprogram whose keywords equal the
/// enabled keywords restricted to the stage's keyword universe; failing that, the one
/// with the most enabled keywords, then the fewest others, then the first. Returns the
/// position in `candidates` and whether the match was exact.
#[must_use]
pub fn select_variant(
    candidates: &[BTreeSet<String>],
    enabled: &BTreeSet<String>,
) -> Option<(usize, bool)> {
    let universe: BTreeSet<&String> = candidates.iter().flatten().collect();
    let want: BTreeSet<&String> = enabled.iter().filter(|k| universe.contains(k)).collect();
    if let Some(i) = candidates.iter().position(|c| c.iter().collect::<BTreeSet<_>>() == want) {
        return Some((i, true));
    }
    let mut best: Option<(usize, (usize, std::cmp::Reverse<usize>))> = None;
    for (i, c) in candidates.iter().enumerate() {
        let hit = c.iter().filter(|k| want.contains(k)).count();
        let score = (hit, std::cmp::Reverse(c.len() - hit));
        if best.is_none_or(|(_, b)| score > b) {
            best = Some((i, score));
        }
    }
    best.map(|(i, _)| (i, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(s: &str) -> BTreeSet<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn variant_selection_prefers_exact_then_closest() {
        let c = vec![set("A"), set("A B"), set("A B C"), set("D")];
        assert_eq!(select_variant(&c, &set("A B X")), Some((1, true)));
        // B C enabled, no exact match among {A,B,C,D}-restricted sets: A B C has 2 hits.
        assert_eq!(select_variant(&c, &set("B C")), Some((2, false)));
        // Ties on hits go to fewer extra keywords.
        assert_eq!(select_variant(&c, &set("A E")), Some((0, true)));
        assert_eq!(select_variant(&c, &set("")), Some((0, false)));
    }

    #[test]
    fn shadow_caster_variants_follow_geometry_clip() {
        // The Owlcat/Lit ShadowCaster vertex programs' keyword sets, in blob order.
        let v = vec![
            set(""),
            set("GEOMETRY_CLIP PBD_SKINNING VERTEX_ANIMATION_ENABLED"),
            set("PBD_SKINNING"),
            set("GEOMETRY_CLIP INSTANCING_ON PBD_GRASS"),
            set("PBD_MESH"),
            set("GEOMETRY_CLIP PBD_MESH"),
            set("PBD_GRASS"),
            set("GEOMETRY_CLIP PBD_SKINNING"),
            set("PBD_SKINNING VERTEX_ANIMATION_ENABLED"),
            set("GEOMETRY_CLIP"),
            set("GEOMETRY_CLIP PBD_GRASS"),
            set("INSTANCING_ON PBD_GRASS"),
        ];
        // A plain material (keywords outside the stage's universe don't matter).
        assert_eq!(select_variant(&v, &set("DYNAMIC_FX _NORMALMAP")), Some((0, true)));
        assert_eq!(
            select_variant(&v, &set("DYNAMIC_FX _NORMALMAP GEOMETRY_CLIP")),
            Some((9, true))
        );
        // Fragment programs add the alpha test.
        let f = vec![
            set(""),
            set("_ALPHATEST_ON"),
            set("GEOMETRY_CLIP VERTEX_ANIMATION_ENABLED _ALPHATEST_ON"),
            set("GEOMETRY_CLIP _ALPHATEST_ON"),
            set("GEOMETRY_CLIP"),
        ];
        assert_eq!(select_variant(&f, &set("_ALPHATEST_ON GEOMETRY_CLIP")), Some((3, true)));
        assert_eq!(select_variant(&f, &set("_ALPHATEST_ON")), Some((1, true)));
    }

    #[test]
    fn subprogram_parse() {
        let mut d = Vec::new();
        for x in [201_910_210i32, 23, 0, 0, 0, 0] {
            d.extend_from_slice(&x.to_le_bytes());
        }
        let push_str = |d: &mut Vec<u8>, s: &str| {
            d.extend_from_slice(&(s.len() as i32).to_le_bytes());
            d.extend_from_slice(s.as_bytes());
            while !d.len().is_multiple_of(4) {
                d.push(0);
            }
        };
        d.extend_from_slice(&2i32.to_le_bytes());
        push_str(&mut d, "DEFERRED_ON");
        push_str(&mut d, "LIGHTMAP_ON");
        d.extend_from_slice(&1i32.to_le_bytes());
        push_str(&mut d, "_NORMALMAP");
        let code = b"\xfe\xca\x0d\xf0junk#include <metal_stdlib>\nvoid f(){}\n\0binding table";
        d.extend_from_slice(&(code.len() as i32).to_le_bytes());
        d.extend_from_slice(code);
        let (t, kw, msl) = parse_subprogram(&d).unwrap();
        assert_eq!(t, 23);
        assert_eq!(kw, set("DEFERRED_ON LIGHTMAP_ON _NORMALMAP"));
        assert_eq!(msl, "#include <metal_stdlib>\nvoid f(){}\n");
    }
}
