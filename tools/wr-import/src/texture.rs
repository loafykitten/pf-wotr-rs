//! Texture payload checks: the shipped pixel data (BC blocks or raw) must be reachable
//! byte for byte, at the size its format implies, so the runtime can upload it without
//! transcoding.

use anyhow::Result;
use serde::Serialize;

use crate::area::Universe;
use crate::unity::value::Value;

/// Unity `TextureFormat` names for the IDs seen in these bundles.
#[must_use]
pub fn format_name(id: i64) -> String {
    let s = match id {
        1 => "Alpha8",
        3 => "RGB24",
        4 => "RGBA32",
        5 => "ARGB32",
        7 => "RGB565",
        9 => "R16",
        10 => "DXT1 (BC1)",
        12 => "DXT5 (BC3)",
        13 => "RGBA4444",
        14 => "BGRA32",
        15 => "RHalf",
        16 => "RGHalf",
        17 => "RGBAHalf",
        18 => "RFloat",
        19 => "RGFloat",
        20 => "RGBAFloat",
        22 => "RGB9e5Float",
        24 => "BC6H",
        25 => "BC7",
        26 => "BC4",
        27 => "BC5",
        28 => "DXT1Crunched",
        29 => "DXT5Crunched",
        62 => "RG16",
        63 => "R8",
        _ => return format!("Format{id}"),
    };
    s.to_owned()
}

/// Bytes per 4×4 block for block-compressed formats, or `None`.
#[must_use]
pub fn block_bytes(format: i64) -> Option<u64> {
    match format {
        10 | 26 => Some(8),
        12 | 24 | 25 | 27 => Some(16),
        _ => None,
    }
}

/// Bytes per pixel for uncompressed formats, or `None`.
#[must_use]
pub fn pixel_bytes(format: i64) -> Option<u64> {
    match format {
        1 | 63 => Some(1),
        2 | 7 | 9 | 13 | 15 | 62 => Some(2),
        3 => Some(3),
        4 | 5 | 14 | 16 | 18 | 22 => Some(4),
        17 | 19 => Some(8),
        20 => Some(16),
        _ => None,
    }
}

/// Size of one image (all mips) for a format, or `None` if the format is unknown or
/// crunched (variable size).
#[must_use]
pub fn image_size(format: i64, width: u64, height: u64, mips: u32) -> Option<u64> {
    let mut total = 0u64;
    for i in 0..mips.max(1) {
        let w = (width >> i).max(1);
        let h = (height >> i).max(1);
        total += if let Some(b) = block_bytes(format) {
            w.div_ceil(4) * h.div_ceil(4) * b
        } else {
            w * h * pixel_bytes(format)?
        };
    }
    Some(total)
}

/// Result of checking one texture.
#[derive(Debug, Clone, Serialize)]
pub struct TextureCheck {
    pub name: String,
    pub class: String,
    pub format: String,
    pub width: u64,
    pub height: u64,
    pub mips: u32,
    pub images: u64,
    pub stored_bytes: u64,
    pub in_stream: bool,
    /// Our computed size matches `m_CompleteImageSize × images` and the bytes are there.
    pub ok: bool,
    pub note: String,
}

/// `GraphicsFormat` (used by `Texture2DArray.m_Format`) → the equivalent `TextureFormat`
/// for the formats seen here.
#[must_use]
pub fn graphics_to_texture_format(g: i64) -> i64 {
    match g {
        96 | 97 => 10,
        100 | 101 => 12,
        102 | 103 => 26,
        104 | 105 => 27,
        106 | 107 => 24,
        108 | 109 => 25,
        4 | 8 => 4,
        _ => -1,
    }
}

/// Checks a texture through the stream lookup in [`check_with`], using the universe.
pub fn check(universe: &Universe, class: &str, v: &Value) -> Result<TextureCheck> {
    let mut err = None;
    let c = check_with(class, v, |name| match universe.stream(name) {
        Ok(s) => s.map(|d| d.len() as u64),
        Err(e) => {
            err = Some(e);
            None
        }
    });
    match err {
        Some(e) => Err(e),
        None => Ok(c),
    }
}

/// Checks a `Texture2D`/`Cubemap`/`Texture2DArray` value: computes the expected payload
/// size from format, dimensions and mips, and confirms the payload is present (inline,
/// or as a slice of the bundle's `.resS` stream, whose length `stream_len` returns) at
/// exactly that size.
pub fn check_with(
    class: &str,
    v: &Value,
    mut stream_len: impl FnMut(&str) -> Option<u64>,
) -> TextureCheck {
    let num = |k: &str| v.i64_at(k).and_then(|x| u64::try_from(x).ok());
    let name = v.str_at("m_Name").unwrap_or_default().to_owned();
    let fmt = if class == "Texture2DArray" {
        graphics_to_texture_format(v.i64_at("m_Format").unwrap_or(-1))
    } else {
        v.i64_at("m_TextureFormat").unwrap_or(-1)
    };
    let width = num("m_Width").unwrap_or(0);
    let height = num("m_Height").unwrap_or(0);
    let mips = u32::try_from(num("m_MipCount").unwrap_or(1)).unwrap_or(1);
    let images = if class == "Texture2DArray" {
        num("m_Depth").unwrap_or(1)
    } else {
        num("m_ImageCount").unwrap_or(1).max(1)
    };
    // Arrays store the total (`m_DataSize`); 2D textures and cubemaps store one image.
    let complete = if class == "Texture2DArray" {
        num("m_DataSize").map(|d| d / images.max(1))
    } else {
        num("m_CompleteImageSize")
    };
    let inline = match v.get("image data") {
        Some(Value::Bytes(b)) => b.len() as u64,
        _ => 0,
    };
    let stream_size = num("m_StreamData.size").unwrap_or(0);
    let stream_off = v
        .path("m_StreamData.offset")
        .and_then(|x| match x {
            Value::UInt(u) => Some(*u),
            Value::Int(i) => u64::try_from(*i).ok(),
            _ => None,
        })
        .unwrap_or(0);
    let stream_path = v.str_at("m_StreamData.path").unwrap_or_default();
    let in_stream = stream_size > 0;
    let stored = if in_stream { stream_size } else { inline };
    let expected_one = image_size(fmt, width, height, mips);
    let mut notes = Vec::new();
    let mut ok = true;
    match (expected_one, complete) {
        (Some(e), Some(c)) if e != c => {
            ok = false;
            notes.push(format!("computed image size {e} != stored image size {c}"));
        }
        (None, _) => {
            ok = false;
            notes.push("format size unknown".to_owned());
        }
        _ => {}
    }
    if let Some(c) = complete
        && stored != c * images
    {
        ok = false;
        notes.push(format!("stored {stored} != {c} x {images} images"));
    }
    if in_stream {
        let stream_name = stream_path.rsplit('/').next().unwrap_or_default();
        match stream_len(stream_name) {
            Some(len) if stream_off + stream_size <= len => {}
            Some(_) => {
                ok = false;
                notes.push("stream slice past end of .resS".to_owned());
            }
            None => {
                ok = false;
                notes.push(format!("stream {stream_name} not found"));
            }
        }
    }
    TextureCheck {
        name,
        class: class.to_owned(),
        format: format_name(fmt),
        width,
        height,
        mips,
        images,
        stored_bytes: stored,
        in_stream,
        ok,
        note: notes.join("; "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bc_sizes_follow_block_math() {
        // 256×256 BC1, full chain (9 mips): 32768+8192+2048+512+128+32+8+8+8 = 43704.
        assert_eq!(image_size(10, 256, 256, 9), Some(43_704));
        // BC3 doubles BC1.
        assert_eq!(image_size(12, 256, 256, 9), Some(87_408));
        // Non-multiple-of-4 sizes round up to whole blocks.
        assert_eq!(image_size(10, 5, 3, 1), Some(2 * 8));
        assert_eq!(image_size(4, 4, 2, 2), Some(4 * 2 * 4 + 2 * 4));
        assert_eq!(image_size(28, 4, 4, 1), None);
    }
}
