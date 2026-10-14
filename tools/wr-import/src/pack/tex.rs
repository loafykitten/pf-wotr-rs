//! Texture payloads for the pack: the shipped bytes unchanged where the GPU can sample
//! them (BC1/3/4/5/6H/7, RGBA8, BGRA8, half and float formats), and small, labelled
//! transcodes where it can't (RGB24 and ARGB32 to RGBA8, Alpha8 to RGBA8 with the value
//! in alpha, crunch to BGRA8).

use anyhow::{Context, Result, bail};
use wr_assets::{TexDimension, TexFormat, Texture};

use crate::area::Universe;
use crate::texture::graphics_to_texture_format;
use crate::unity::value::Value;

fn u64_of(v: Option<&Value>) -> u64 {
    match v {
        Some(Value::UInt(u)) => *u,
        Some(Value::Int(i)) => u64::try_from(*i).unwrap_or(0),
        _ => 0,
    }
}

/// Raw payload bytes (inline `image data` or the `.resS` slice).
pub fn payload(universe: &Universe, v: &Value) -> Result<Vec<u8>> {
    let size = u64_of(v.path("m_StreamData.size"));
    if size > 0 {
        let path = v.str_at("m_StreamData.path").unwrap_or_default();
        let sname = path.rsplit('/').next().unwrap_or_default();
        let s = universe.stream(sname)?.with_context(|| format!("stream {sname}"))?;
        let off = usize::try_from(u64_of(v.path("m_StreamData.offset")))?;
        let len = usize::try_from(size)?;
        return Ok(s.get(off..off + len).context("texture stream slice out of range")?.to_vec());
    }
    Ok(v.get("image data").and_then(Value::as_bytes).unwrap_or_default().to_vec())
}

/// The pack texture (without its blob) and the bytes to store.
pub fn convert(universe: &Universe, class: &str, v: &Value) -> Result<(Texture, Vec<u8>)> {
    let name = v.str_at("m_Name").unwrap_or_default().to_owned();
    let (unity_format, dimension, layers) = match class {
        "Texture2DArray" => (
            graphics_to_texture_format(v.i64_at("m_Format").unwrap_or(-1)),
            TexDimension::D2Array,
            u32::try_from(v.i64_at("m_Depth").unwrap_or(1))?,
        ),
        "Cubemap" => (v.i64_at("m_TextureFormat").unwrap_or(-1), TexDimension::Cube, 6),
        _ => (v.i64_at("m_TextureFormat").unwrap_or(-1), TexDimension::D2, 1),
    };
    let width = u32::try_from(v.i64_at("m_Width").unwrap_or(0))?;
    let height = u32::try_from(v.i64_at("m_Height").unwrap_or(0))?;
    let mut mips = u32::try_from(v.i64_at("m_MipCount").unwrap_or(1))?.max(1);
    let ts = v.get("m_TextureSettings");
    let tsi = |k: &str| ts.and_then(|t| t.i64_at(k)).unwrap_or(0);
    let wrap = [tsi("m_WrapU") as u8, tsi("m_WrapV") as u8, tsi("m_WrapW") as u8];
    // `m_ColorSpace`: 1 = sRGB (gamma), 0 = linear.
    let srgb = v.i64_at("m_ColorSpace").unwrap_or(1) == 1;
    let raw = payload(universe, v)?;
    let mut transcoded = None;
    let (format, data) = match unity_format {
        10 => (TexFormat::Bc1, raw),
        12 => (TexFormat::Bc3, raw),
        26 => (TexFormat::Bc4, raw),
        27 => (TexFormat::Bc5, raw),
        24 => (TexFormat::Bc6hUfloat, raw),
        25 => (TexFormat::Bc7, raw),
        4 => (TexFormat::Rgba8, raw),
        14 => (TexFormat::Bgra8, raw),
        63 => (TexFormat::R8, raw),
        62 => (TexFormat::Rg8, raw),
        15 => (TexFormat::R16Float, raw),
        17 => (TexFormat::Rgba16Float, raw),
        20 => (TexFormat::Rgba32Float, raw),
        3 => {
            transcoded = Some("RGB24 -> RGBA8 (alpha 255)".to_owned());
            let mut out = Vec::with_capacity(raw.len() / 3 * 4);
            for p in raw.as_chunks::<3>().0 {
                out.extend_from_slice(&[p[0], p[1], p[2], 255]);
            }
            (TexFormat::Rgba8, out)
        }
        5 => {
            transcoded = Some("ARGB32 -> RGBA8 (byte swizzle)".to_owned());
            let out =
                raw.as_chunks::<4>().0.iter().flat_map(|p| [p[1], p[2], p[3], p[0]]).collect();
            (TexFormat::Rgba8, out)
        }
        1 => {
            transcoded = Some("Alpha8 -> RGBA8 (0, 0, 0, a)".to_owned());
            (TexFormat::Rgba8, raw.iter().flat_map(|&a| [0, 0, 0, a]).collect())
        }
        28 | 29 => {
            // Unity transcodes crunch to DXT1/DXT5 blocks at load. The public decoder we
            // use returns pixels of mip 0 only, so the pack stores BGRA8 mip 0: texel
            // values equal the DXT decode, but not the block bytes, and no mip chain.
            let w = width as usize;
            let h = height as usize;
            let mut px = vec![0u32; w * h];
            texture2ddecoder::decode_unity_crunch(&raw, w, h, &mut px)
                .map_err(|e| anyhow::anyhow!("{name}: crunch: {e}"))?;
            transcoded = Some(format!(
                "crunch ({}) -> BGRA8, mip 0 only (shipped {mips} mips)",
                if unity_format == 28 { "DXT1" } else { "DXT5" }
            ));
            mips = 1;
            (TexFormat::Bgra8, px.iter().flat_map(|p| p.to_le_bytes()).collect())
        }
        other => bail!("{name}: texture format {other} not supported"),
    };
    // Check the payload covers every level of every layer.
    let mut expect = 0u64;
    for m in 0..mips {
        expect += format.level_size((width >> m).max(1), (height >> m).max(1));
    }
    expect *= u64::from(layers);
    if (data.len() as u64) < expect {
        bail!("{name}: payload {} B < expected {expect} B", data.len());
    }
    let tex = Texture {
        name,
        dimension,
        format,
        srgb,
        width,
        height,
        layers,
        mips,
        wrap,
        filter: tsi("m_FilterMode") as u8,
        aniso: tsi("m_Aniso") as u8,
        mip_bias: ts.and_then(|t| t.f32_at("m_MipBias")).unwrap_or(0.0),
        unity_format: unity_format as i32,
        transcoded,
        ..Default::default()
    };
    Ok((tex, data))
}
