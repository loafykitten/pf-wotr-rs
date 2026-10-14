//! The resource interface of a Unity-generated (HLSLcc) Metal shader: entry-point
//! bindings with their explicit `[[buffer(n)]]`/`[[texture(n)]]`/`[[sampler(n)]]` slots,
//! `[[stage_in]]` vertex attributes, colour outputs, varyings, and constant-buffer
//! struct layouts by MSL rules (cross-checked against Unity's own binding tables in the
//! POC-0 rendering spike).
//!
//! The runtime builds wgpu bind group layouts from this so that wgpu-hal's sequential
//! per-stage slot assignment lands on the shader's slots, and fills constant buffers by
//! member name.

use std::collections::HashMap;

use wr_assets::{Binding, CBufferLayout, Member, Resource, Stage, TexKind, VertexInput};

#[derive(Debug, Clone)]
pub struct Interface {
    pub stage: Stage,
    pub entry: String,
    pub bindings: Vec<Binding>,
    pub vertex_inputs: Vec<VertexInput>,
    pub color_outputs: u32,
    /// Interpolated varyings (vertex outputs or fragment inputs): (`user(...)`, type).
    pub varyings: Vec<(String, String)>,
    pub structs: HashMap<String, CBufferLayout>,
}

impl Interface {
    /// Layouts of the constant buffers this stage binds.
    #[must_use]
    pub fn cbuffers(&self) -> Vec<CBufferLayout> {
        let mut out: Vec<CBufferLayout> = self
            .bindings
            .iter()
            .filter_map(|b| match &b.res {
                Resource::Uniform { ty } | Resource::Storage { ty, .. } => self.structs.get(ty),
                _ => None,
            })
            .cloned()
            .collect();
        out.sort_by(|a, b| a.ty.cmp(&b.ty));
        out.dedup_by(|a, b| a.ty == b.ty);
        out
    }
}

/// (size, align) of an MSL scalar or vector type.
fn prim(ty: &str) -> Option<(u32, u32)> {
    let (base, n) = match ty.find(|c: char| c.is_ascii_digit()) {
        Some(i) => (&ty[..i], ty[i..].parse::<u32>().ok()?),
        None => (ty, 1),
    };
    let s = match base {
        "float" | "int" | "uint" => 4,
        "half" | "short" | "ushort" => 2,
        "bool" | "char" | "uchar" => 1,
        _ => return None,
    };
    match n {
        1 => Some((s, s)),
        2 => Some((2 * s, 2 * s)),
        // MSL: 3-component vectors have the size and alignment of 4.
        3 | 4 => Some((4 * s, 4 * s)),
        _ => None,
    }
}

fn round_up(x: u32, a: u32) -> u32 {
    x.div_ceil(a) * a
}

fn parse_structs(src: &str) -> HashMap<String, CBufferLayout> {
    let mut out: HashMap<String, CBufferLayout> = HashMap::new();
    let mut aligns: HashMap<String, u32> = HashMap::new();
    let mut lines = src.lines();
    while let Some(line) = lines.next() {
        let Some(name) = line.trim().strip_prefix("struct ") else { continue };
        let name = name.trim().to_owned();
        if !lines.next().is_some_and(|l| l.trim() == "{") {
            continue;
        }
        let mut layout = CBufferLayout { ty: name.clone(), ..Default::default() };
        let mut align_all = 1;
        let mut ok = true;
        for body in lines.by_ref() {
            let b = body.trim();
            if b.starts_with('}') {
                break;
            }
            if b.contains("[[") {
                ok = false; // stage_in / stage_out structs: attributes, not memory
                continue;
            }
            let b = b.trim_end_matches(';').trim();
            let mut parts = b.split_whitespace();
            let (Some(ty), Some(decl)) = (parts.next(), parts.next()) else { continue };
            let (mname, count) = match decl.find('[') {
                Some(i) => (&decl[..i], decl[i + 1..decl.len() - 1].parse::<u32>().unwrap_or(1)),
                None => (decl, 1),
            };
            let (size, align) = match prim(ty) {
                Some(p) => p,
                None => match (out.get(ty), aligns.get(ty)) {
                    (Some(s), Some(&a)) => (s.size, a),
                    _ => {
                        ok = false;
                        continue;
                    }
                },
            };
            let offset = round_up(layout.size, align);
            let stride = round_up(size, align);
            layout.members.push(Member {
                name: mname.to_owned(),
                ty: ty.to_owned(),
                count,
                offset,
                size: stride * count,
            });
            layout.size = offset + stride * count;
            align_all = align_all.max(align);
        }
        if ok {
            layout.size = round_up(layout.size, align_all);
            aligns.insert(name.clone(), align_all);
            out.insert(name, layout);
        }
    }
    out
}

fn attr_index(s: &str, key: &str) -> Option<u32> {
    let i = s.find("[[")?;
    let close = s[i..].find("]]")? + i;
    let inner = s[i + 2..close].trim();
    let rest = inner.strip_prefix(key)?.trim_start().strip_prefix('(')?;
    rest[..rest.find(')')?].trim().parse().ok()
}

fn tex_kind(ty: &str) -> Option<TexKind> {
    Some(match ty {
        "texture2d" => TexKind::D2,
        "texture2d_array" => TexKind::D2Array,
        "texture3d" => TexKind::D3,
        "texturecube" => TexKind::Cube,
        "texturecube_array" => TexKind::CubeArray,
        "depth2d" => TexKind::Depth2,
        "depth2d_array" => TexKind::Depth2Array,
        _ => return None,
    })
}

/// Buffers, then samplers, then textures: Metal's three argument tables.
#[must_use]
pub fn kind_order(r: &Resource) -> u8 {
    match r {
        Resource::Uniform { .. } | Resource::Storage { .. } => 0,
        Resource::Sampler => 1,
        Resource::Texture { .. } => 2,
    }
}

/// Parses a single-entry-point MSL source as Unity 2020.3 emits it for Metal.
pub fn parse(src: &str) -> Result<Interface, String> {
    let structs = parse_structs(src);
    let lines: Vec<&str> = src.lines().collect();
    let (stage, sig_start) = lines
        .iter()
        .enumerate()
        .find_map(|(i, l)| {
            let st = if l.starts_with("vertex ") {
                Stage::Vertex
            } else if l.starts_with("fragment ") {
                Stage::Fragment
            } else if l.starts_with("kernel ") {
                Stage::Compute
            } else {
                return None;
            };
            Some((st, i))
        })
        .ok_or("no vertex/fragment/kernel entry point")?;
    let entry = lines[sig_start]
        .split_whitespace()
        .nth(2)
        .and_then(|s| s.split('(').next())
        .ok_or("cannot read entry name")?
        .to_owned();
    let mut bindings = Vec::new();
    let mut stage_in_ty = None;
    // The signature may start on the entry line itself (`kernel void f(` + args).
    for l in &lines[sig_start..] {
        let full = l.trim();
        let t = full.split_once('(').map_or(full, |(head, rest)| {
            if head.contains("xlatMtlMain") || head.starts_with("kernel") { rest } else { full }
        });
        if t.starts_with('{') {
            break;
        }
        let t = t.trim().trim_end_matches(',').trim_end_matches(')').trim();
        if t.contains("[[ stage_in ]]") || t.contains("[[stage_in]]") {
            stage_in_ty = t.split_whitespace().next().map(str::to_owned);
            continue;
        }
        let words: Vec<&str> = t.split_whitespace().collect();
        if let Some(slot) = attr_index(t, "buffer") {
            if words.first() == Some(&"constant") {
                let ty = words.get(1).unwrap_or(&"?").trim_end_matches('&').to_owned();
                let name = words.get(2).unwrap_or(&"?").to_string();
                bindings.push(Binding { slot, name, res: Resource::Uniform { ty } });
            } else {
                let read_only = words.first() == Some(&"const");
                let ty_i = if read_only { 2 } else { 1 };
                let ty = words.get(ty_i).unwrap_or(&"?").trim_end_matches('*').to_owned();
                let name = words.get(ty_i + 1).unwrap_or(&"?").trim_start_matches('*').to_owned();
                bindings.push(Binding { slot, name, res: Resource::Storage { ty, read_only } });
            }
        } else if let Some(slot) = attr_index(t, "sampler") {
            let name = words.get(1).unwrap_or(&"?").to_string();
            bindings.push(Binding { slot, name, res: Resource::Sampler });
        } else if let Some(slot) = attr_index(t, "texture") {
            let full = words.first().copied().unwrap_or_default();
            let (base, generic) =
                full.split_once('<').ok_or_else(|| format!("texture type: {t}"))?;
            let kind = tex_kind(base).ok_or_else(|| format!("unsupported texture type {base}"))?;
            let sample_type = generic.trim_end_matches(',').to_owned();
            let name_i = words.iter().position(|w| w.ends_with('>')).map_or(2, |i| i + 1);
            bindings.push(Binding {
                slot,
                name: words.get(name_i).unwrap_or(&"?").to_string(),
                res: Resource::Texture {
                    kind,
                    sample_type,
                    write: t.contains("access::write") || t.contains("access::read_write"),
                },
            });
        }
    }
    let mut vertex_inputs = Vec::new();
    let mut varyings = Vec::new();
    let mut color_outputs = 0;
    let mut cur: Option<&str> = None;
    for l in &lines {
        let t = l.trim();
        if let Some(n) = t.strip_prefix("struct ") {
            cur = Some(n.trim());
            continue;
        }
        if t.starts_with('}') {
            cur = None;
            continue;
        }
        if stage == Stage::Vertex
            && cur == stage_in_ty.as_deref()
            && let Some(attribute) = attr_index(t, "attribute")
        {
            let w: Vec<&str> = t.split_whitespace().collect();
            vertex_inputs.push(VertexInput {
                attribute,
                ty: w.first().copied().unwrap_or_default().to_owned(),
                semantic: w.get(1).copied().unwrap_or_default().to_owned(),
            });
        }
        if stage == Stage::Fragment && cur == Some("Mtl_FragmentOut") && t.contains("[[ color(") {
            color_outputs += 1;
        }
        let varying_struct = match stage {
            Stage::Vertex => Some("Mtl_VertexOut"),
            Stage::Fragment => stage_in_ty.as_deref(),
            Stage::Compute => None,
        };
        if cur.is_some()
            && cur == varying_struct
            && let Some(i) = t.find("[[ user(")
        {
            let name = t[i + 8..].split(')').next().unwrap_or("").to_owned();
            let ty = t.split_whitespace().next().unwrap_or("").to_owned();
            varyings.push((name, ty));
        }
    }
    bindings.sort_by_key(|b| (kind_order(&b.res), b.slot));
    vertex_inputs.sort_by_key(|v| v.attribute);
    Ok(Interface { stage, entry, bindings, vertex_inputs, color_outputs, varyings, structs })
}

/// Checks that each argument table uses slots `0..n` with no gaps (wgpu assigns slots
/// sequentially, so a gap would need a dummy binding).
pub fn check_contiguous(iface: &Interface) -> Result<(), String> {
    for k in 0..3u8 {
        let slots: Vec<u32> =
            iface.bindings.iter().filter(|b| kind_order(&b.res) == k).map(|b| b.slot).collect();
        for (i, s) in slots.iter().enumerate() {
            if *s != i as u32 {
                return Err(format!("{:?} table {k}: slot {s} at position {i} (gap)", iface.stage));
            }
        }
    }
    Ok(())
}

/// Whether a vertex stage writes every varying a fragment stage reads, with the same type.
#[must_use]
pub fn links(vs: &Interface, fs: &Interface) -> bool {
    fs.varyings.iter().all(|(n, t)| vs.varyings.iter().any(|(vn, vt)| vn == n && vt == t))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"#include <metal_stdlib>
using namespace metal;
struct UnityPerMaterial_Type
{
    float4 _BaseMap_ST;
    float4 _BaseMap_TexelSize;
    float4 _BaseMap_MipInfo;
    float4 _BaseColor;
    float _BaseColorBlending;
    float _Cutoff;
    float _AlphaToCoverage;
    float _Roughness;
    float _Metallic;
    float _BumpScale;
    float _RimLighting;
    float4 _RimColor;
    float _RimPower;
    float4 _EmissionColor;
    float _EmissionColorFactor;
    float _EmissionColorScale;
    float _EmissionMapUsage;
    float _EmissionAlbedoSuppression;
    float3 _CamBasisUp;
    float3 _CamBasisSide;
    float4 hlslcc_mtx4x4M[4];
};
struct Mtl_VertexIn
{
    float4 POSITION0 [[ attribute(0) ]] ;
    float2 TEXCOORD0 [[ attribute(3) ]] ;
};
struct Mtl_FragmentOut
{
    float4 SV_Target0 [[ color(xlt_remap_o[0]) ]];
    float4 SV_Target1 [[ color(xlt_remap_o[1]) ]];
};
fragment Mtl_FragmentOut xlatMtlMain(
    constant UnityPerMaterial_Type& UnityPerMaterial [[ buffer(0) ]],
    const device uint* _Tiles [[ buffer(1) ]],
    sampler sampler_BaseMap [[ sampler (0) ]],
    texture2d<float, access::sample > _BaseMap [[ texture(0) ]] ,
    texture3d<half, access::sample > _Noise [[ texture(1) ]] ,
    depth2d<float, access::sample > _Shadow [[ texture(2) ]] ,
    float4 mtl_FragCoord [[ position ]],
    Mtl_FragmentIn input [[ stage_in ]])
{
}
"#;

    #[test]
    fn layout_matches_unity_reflection_offsets() {
        // Offsets cross-checked against Unity's binding table for an Owlcat/Lit GBuffer
        // fragment (POC-0 spike): _EmissionColor 0x80, Factor 0x90, AlbedoSuppression 0x9c.
        let i = parse(SRC).unwrap();
        let l = &i.structs["UnityPerMaterial_Type"];
        let off = |n: &str| l.members.iter().find(|m| m.name == n).unwrap().offset;
        assert_eq!(off("_RimColor"), 96);
        assert_eq!(off("_EmissionColor"), 0x80);
        assert_eq!(off("_EmissionColorFactor"), 0x90);
        assert_eq!(off("_EmissionAlbedoSuppression"), 0x9c);
        assert_eq!(off("_CamBasisSide"), 0xb0);
        assert_eq!(l.size, 0xc0 + 64);
        assert!(!i.structs.contains_key("Mtl_VertexIn"));
    }

    #[test]
    fn bindings_and_outputs() {
        let i = parse(SRC).unwrap();
        assert_eq!(i.stage, Stage::Fragment);
        assert_eq!(i.color_outputs, 2);
        let names: Vec<&str> = i.bindings.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(
            names,
            ["UnityPerMaterial", "_Tiles", "sampler_BaseMap", "_BaseMap", "_Noise", "_Shadow"]
        );
        assert!(matches!(i.bindings[1].res, Resource::Storage { read_only: true, .. }));
        assert!(matches!(i.bindings[5].res, Resource::Texture { kind: TexKind::Depth2, .. }));
        check_contiguous(&i).unwrap();
        assert_eq!(i.cbuffers().len(), 1);
    }

    #[test]
    fn kernels_and_vertex_inputs() {
        let k = "struct CB_Type\n{\n    float4 a;\n};\nkernel void computeMain(\n    constant CB_Type& CB [[ buffer(1) ]],\n    device uint *_Out [[ buffer(0) ]],\n    texture2d<float, access::write > _Res [[ texture(0) ]] ,\n    uint3 id [[ thread_position_in_grid ]])\n{\n}\n";
        let i = parse(k).unwrap();
        assert_eq!(i.stage, Stage::Compute);
        assert_eq!(i.entry, "computeMain");
        assert_eq!(i.bindings.len(), 3);
        assert_eq!(i.bindings[0].name, "_Out");
        assert!(matches!(i.bindings[2].res, Resource::Texture { write: true, .. }));
        let vs = SRC
            .replace("fragment Mtl_FragmentOut", "vertex Mtl_VertexOut")
            .replace("Mtl_FragmentIn input", "Mtl_VertexIn input");
        let i = parse(&vs).unwrap();
        assert_eq!(i.vertex_inputs.len(), 2);
        assert_eq!(i.vertex_inputs[1].semantic, "TEXCOORD0");
        assert_eq!(i.vertex_inputs[1].attribute, 3);
    }
}
