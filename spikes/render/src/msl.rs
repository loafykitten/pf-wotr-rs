//! Parse the resource interface of a Unity-generated (HLSLcc) Metal shader: entry-point
//! bindings, `[[stage_in]]` vertex attributes, colour outputs, and struct layouts by MSL rules.
//!
//! That is enough to build a wgpu bind group layout whose per-stage slot assignment matches the
//! shader's explicit `[[buffer(n)]]`/`[[texture(n)]]`/`[[sampler(n)]]` indices (wgpu-hal's Metal
//! backend assigns slots sequentially per stage, in bind-group entry order), and to fill Unity's
//! constant buffers by member name.

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Vertex,
    Fragment,
    Compute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TexKind {
    D2,
    D2Array,
    D3,
    Cube,
    CubeArray,
    Depth2,
    Depth2Array,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Resource {
    /// `constant T& name [[buffer(n)]]`
    Uniform {
        ty: String,
    },
    /// `const device T* name [[buffer(n)]]` or `device T* ...`
    Storage {
        ty: String,
        read_only: bool,
    },
    Texture {
        kind: TexKind,
        sample_type: String,
        access_sample: bool,
    },
    Sampler,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Binding {
    pub slot: u32,
    pub name: String,
    pub res: Resource,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VertexInput {
    pub attribute: u32,
    pub ty: String,
    /// HLSL semantic as HLSLcc names it, e.g. `POSITION0`, `TEXCOORD1`.
    pub semantic: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    pub name: String,
    pub ty: String,
    pub count: u32,
    pub offset: u32,
    pub size: u32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct StructLayout {
    pub members: Vec<Member>,
    pub size: u32,
    pub align: u32,
}

impl StructLayout {
    pub fn member(&self, name: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.name == name)
    }
}

#[derive(Debug, Clone)]
pub struct Interface {
    pub stage: Stage,
    pub entry: String,
    pub bindings: Vec<Binding>,
    pub vertex_inputs: Vec<VertexInput>,
    pub color_outputs: u32,
    /// Interpolated varyings: vertex outputs or fragment inputs, as (`user(...)` name, type).
    pub varyings: Vec<(String, String)>,
    pub structs: HashMap<String, StructLayout>,
}

/// (size, align) of an MSL scalar/vector type name.
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
    let (size, align) = match n {
        1 => (s, s),
        2 => (2 * s, 2 * s),
        3 | 4 => (4 * s, 4 * s), // MSL: 3-component vectors have the size of 4
        _ => return None,
    };
    Some((size, align))
}

fn round_up(x: u32, a: u32) -> u32 {
    x.div_ceil(a) * a
}

fn parse_structs(src: &str) -> HashMap<String, StructLayout> {
    let mut out = HashMap::new();
    let mut lines = src.lines();
    while let Some(line) = lines.next() {
        let l = line.trim();
        let Some(name) = l.strip_prefix("struct ") else { continue };
        let name = name.trim().to_string();
        if !lines.next().is_some_and(|l| l.trim() == "{") {
            continue;
        }
        let mut layout = StructLayout { align: 1, ..Default::default() };
        let mut ok = true;
        for body in lines.by_ref() {
            let b = body.trim();
            if b.starts_with('}') {
                break;
            }
            if b.contains("[[") {
                ok = false; // stage_in / stage_out structs: attributes, not memory layout
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
                None => match out.get(ty) {
                    Some(s @ StructLayout { .. }) => (s.size, s.align),
                    None => {
                        ok = false;
                        continue;
                    }
                },
            };
            let offset = round_up(layout.size, align);
            let stride = round_up(size, align);
            layout.members.push(Member {
                name: mname.to_string(),
                ty: ty.to_string(),
                count,
                offset,
                size: stride * count,
            });
            layout.size = offset + stride * count;
            layout.align = layout.align.max(align);
        }
        if ok {
            layout.size = round_up(layout.size, layout.align);
            out.insert(name, layout);
        }
    }
    out
}

fn attr_index(s: &str, key: &str) -> Option<u32> {
    let i = s.find("[[")?;
    let inner = &s[i + 2..s[i..].find("]]")? + i];
    let inner = inner.trim();
    let rest = inner.strip_prefix(key)?.trim_start();
    let rest = rest.strip_prefix('(')?;
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

/// Parse a single-entry-point MSL source as emitted by Unity 2020.3 for Metal.
pub fn parse(src: &str) -> Result<Interface, String> {
    let structs = parse_structs(src);
    let (stage, sig_start) = ["vertex ", "fragment ", "kernel "]
        .iter()
        .find_map(|kw| {
            src.lines().position(|l| l.starts_with(kw)).map(|i| {
                let st = match *kw {
                    "vertex " => Stage::Vertex,
                    "fragment " => Stage::Fragment,
                    _ => Stage::Compute,
                };
                (st, i)
            })
        })
        .ok_or("no vertex/fragment/kernel entry point")?;
    let lines: Vec<&str> = src.lines().collect();
    let head = lines[sig_start];
    let entry = head
        .split_whitespace()
        .nth(2)
        .and_then(|s| s.split('(').next())
        .ok_or("cannot read entry name")?
        .to_string();
    let mut bindings = Vec::new();
    let mut stage_in_ty = None;
    for l in &lines[sig_start + 1..] {
        let t = l.trim();
        if t.starts_with('{') {
            break;
        }
        let t = t.trim_end_matches(',').trim_end_matches(')').trim();
        if t.contains("[[ stage_in ]]") || t.contains("[[stage_in]]") {
            stage_in_ty = t.split_whitespace().next().map(str::to_string);
            continue;
        }
        let words: Vec<&str> = t.split_whitespace().collect();
        if let Some(slot) = attr_index(t, "buffer") {
            if words.first() == Some(&"constant") {
                let ty = words[1].trim_end_matches('&').to_string();
                let name = words[2].to_string();
                bindings.push(Binding { slot, name, res: Resource::Uniform { ty } });
            } else {
                let read_only = words.first() == Some(&"const");
                let ty_i = if read_only { 2 } else { 1 };
                let ty = words.get(ty_i).unwrap_or(&"?").trim_end_matches('*').to_string();
                let name = words.get(ty_i + 1).unwrap_or(&"?").to_string();
                bindings.push(Binding { slot, name, res: Resource::Storage { ty, read_only } });
            }
        } else if let Some(slot) = attr_index(t, "sampler") {
            bindings.push(Binding { slot, name: words[1].to_string(), res: Resource::Sampler });
        } else if let Some(slot) = attr_index(t, "texture") {
            let full = words[0];
            let (base, generic) =
                full.split_once('<').ok_or_else(|| format!("texture type: {t}"))?;
            let kind = tex_kind(base).ok_or_else(|| format!("unsupported texture type {base}"))?;
            let sample_type = generic.trim_end_matches(',').to_string();
            let name_i = words.iter().position(|w| w.ends_with('>')).map_or(2, |i| i + 1);
            bindings.push(Binding {
                slot,
                name: words.get(name_i).unwrap_or(&"?").to_string(),
                res: Resource::Texture {
                    kind,
                    sample_type,
                    access_sample: t.contains("access::sample"),
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
                ty: w[0].to_string(),
                semantic: w[1].to_string(),
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
            let name = t[i + 8..].split(')').next().unwrap_or("").to_string();
            let ty = t.split_whitespace().next().unwrap_or("").to_string();
            varyings.push((name, ty));
        }
    }
    bindings.sort_by_key(|b| (kind_order(&b.res), b.slot));
    vertex_inputs.sort_by_key(|v| v.attribute);
    Ok(Interface { stage, entry, bindings, vertex_inputs, color_outputs, varyings, structs })
}

/// Group bindings by Metal argument table: buffers, then samplers, then textures.
pub fn kind_order(r: &Resource) -> u8 {
    match r {
        Resource::Uniform { .. } | Resource::Storage { .. } => 0,
        Resource::Sampler => 1,
        Resource::Texture { .. } => 2,
    }
}

/// Check that each argument table uses slots 0..n with no gaps (wgpu assigns them
/// sequentially, so a gap would need a dummy binding).
pub fn check_contiguous(iface: &Interface) -> Result<(), String> {
    for k in 0..3u8 {
        let slots: Vec<u32> =
            iface.bindings.iter().filter(|b| kind_order(&b.res) == k).map(|b| b.slot).collect();
        for (i, s) in slots.iter().enumerate() {
            if *s != i as u32 {
                return Err(format!(
                    "{:?} table kind {k}: slot {s} at position {i} (gap)",
                    iface.stage
                ));
            }
        }
    }
    Ok(())
}

/// Whether a vertex stage writes every varying a fragment stage reads, with the same type.
pub fn links(vs: &Interface, fs: &Interface) -> bool {
    fs.varyings.iter().all(|(n, t)| vs.varyings.iter().any(|(vn, vt)| vn == n && vt == t))
}

/// Write `values` (f32s) into `buf` at member `name` of `layout`, if the member exists.
/// Returns whether it was written.
pub fn write_member(buf: &mut [u8], layout: &StructLayout, name: &str, values: &[f32]) -> bool {
    let Some(m) = layout.member(name) else { return false };
    let mut off = m.offset as usize;
    for v in values {
        if off + 4 > buf.len() || off + 4 > (m.offset + m.size) as usize {
            break;
        }
        buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
        off += 4;
    }
    true
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
        // Offsets cross-checked against the binding table Unity stores after the MSL for an
        // Owlcat/Lit GBuffer fragment (_EmissionColor 0x80, Factor 0x90, Scale 0x94,
        // AlbedoSuppression 0x9c).
        let i = parse(SRC).unwrap();
        let l = &i.structs["UnityPerMaterial_Type"];
        assert_eq!(l.member("_RimColor").unwrap().offset, 96);
        assert_eq!(l.member("_EmissionColor").unwrap().offset, 0x80);
        assert_eq!(l.member("_EmissionColorFactor").unwrap().offset, 0x90);
        assert_eq!(l.member("_EmissionColorScale").unwrap().offset, 0x94);
        assert_eq!(l.member("_EmissionAlbedoSuppression").unwrap().offset, 0x9c);
        assert_eq!(l.member("_CamBasisUp").unwrap().offset, 0xa0);
        assert_eq!(l.member("_CamBasisSide").unwrap().offset, 0xb0);
        assert_eq!(l.member("hlslcc_mtx4x4M").unwrap().size, 64);
        assert_eq!(l.size, 0xc0 + 64);
        assert!(!i.structs.contains_key("Mtl_VertexIn"));
    }

    #[test]
    fn entry_bindings_and_outputs() {
        let i = parse(SRC).unwrap();
        assert_eq!(i.stage, Stage::Fragment);
        assert_eq!(i.entry, "xlatMtlMain");
        assert_eq!(i.color_outputs, 2);
        let names: Vec<&str> = i.bindings.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(
            names,
            ["UnityPerMaterial", "_Tiles", "sampler_BaseMap", "_BaseMap", "_Noise", "_Shadow"]
        );
        assert!(matches!(i.bindings[1].res, Resource::Storage { read_only: true, .. }));
        assert!(matches!(i.bindings[4].res, Resource::Texture { kind: TexKind::D3, .. }));
        assert!(matches!(i.bindings[5].res, Resource::Texture { kind: TexKind::Depth2, .. }));
        check_contiguous(&i).unwrap();
    }

    #[test]
    fn write_member_bounds() {
        let i = parse(SRC).unwrap();
        let l = &i.structs["UnityPerMaterial_Type"];
        let mut buf = vec![0u8; l.size as usize];
        assert!(write_member(&mut buf, l, "_BaseColor", &[1.0, 0.5, 0.25, 1.0, 9.0]));
        assert_eq!(&buf[48..52], &1.0f32.to_le_bytes());
        assert_eq!(&buf[60..64], &1.0f32.to_le_bytes());
        assert_eq!(&buf[64..68], &0.0f32.to_le_bytes(), "must not spill into the next member");
        assert!(!write_member(&mut buf, l, "_Missing", &[1.0]));
    }

    #[test]
    fn vertex_inputs() {
        let src = SRC
            .replace("fragment Mtl_FragmentOut", "vertex Mtl_VertexOut")
            .replace("Mtl_FragmentIn input", "Mtl_VertexIn input");
        let i = parse(&src).unwrap();
        assert_eq!(i.stage, Stage::Vertex);
        assert_eq!(i.vertex_inputs.len(), 2);
        assert_eq!(i.vertex_inputs[1].semantic, "TEXCOORD0");
        assert_eq!(i.vertex_inputs[1].attribute, 3);
    }

    #[test]
    fn varyings_link() {
        let vs_src = "struct Mtl_VertexOut\n{\n    float4 TEXCOORD0 [[ user(TEXCOORD0) ]];\n    float4 mtl_Position [[ position ]];\n};\nvertex Mtl_VertexOut xlatMtlMain(\n    Mtl_VertexIn input [[ stage_in ]])\n{\n}\n";
        let fs_ok = "struct Mtl_FragmentIn\n{\n    float4 TEXCOORD0 [[ user(TEXCOORD0) ]] ;\n};\nfragment Mtl_FragmentOut xlatMtlMain(\n    Mtl_FragmentIn input [[ stage_in ]])\n{\n}\n";
        let fs_bad = fs_ok.replace(
            "TEXCOORD0) ]] ;",
            "TEXCOORD0) ]] ;\n    float3 TEXCOORD8 [[ user(TEXCOORD8) ]] ;",
        );
        let vs = parse(vs_src).unwrap();
        assert_eq!(vs.varyings, vec![("TEXCOORD0".to_string(), "float4".to_string())]);
        assert!(links(&vs, &parse(fs_ok).unwrap()));
        assert!(!links(&vs, &parse(&fs_bad).unwrap()));
    }
}
