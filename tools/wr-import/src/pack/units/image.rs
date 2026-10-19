//! Decoded textures for CPU work (the character atlases): a Unity `Texture2D` as RGBA8, mip 0.
//!
//! Rows run in Unity's order (row 0 is `v = 0`, the bottom of the picture), for raw and block
//! formats alike, so a texel keeps its UV when the image is blitted into an atlas.

use anyhow::{Result, bail};

use crate::area::Universe;
use crate::pack::tex::payload;
use crate::unity::value::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub w: usize,
    pub h: usize,
    /// RGBA8, `w * h`, row 0 first.
    pub px: Vec<[u8; 4]>,
    /// The source format carried an alpha channel (`CharacterAtlas.HasAlphaChannel`).
    pub has_alpha: bool,
}

impl Image {
    #[must_use]
    pub fn filled(w: usize, h: usize, c: [u8; 4]) -> Self {
        Self { w, h, px: vec![c; w * h], has_alpha: true }
    }

    #[must_use]
    pub fn get(&self, x: usize, y: usize) -> [u8; 4] {
        self.px[y.min(self.h - 1) * self.w + x.min(self.w - 1)]
    }

    /// Bilinear sample at UV (clamped), texel centres at `(i + 0.5) / size`.
    #[must_use]
    pub fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        let x = (u * self.w as f32 - 0.5).clamp(0.0, (self.w - 1) as f32);
        let y = (v * self.h as f32 - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let mut out = [0.0; 4];
        for (k, o) in out.iter_mut().enumerate() {
            let t = |xx: usize, yy: usize| f32::from(self.get(xx, yy)[k]);
            let top = t(x0, y0) * (1.0 - fx) + t(x0 + 1, y0) * fx;
            let bot = t(x0, y0 + 1) * (1.0 - fx) + t(x0 + 1, y0 + 1) * fx;
            *o = (top * (1.0 - fy) + bot * fy) / 255.0;
        }
        out
    }

    /// Halves the image `n` times with a box filter (never below 1x1), for downsampling by powers
    /// of two before the final bilinear step.
    #[must_use]
    pub fn halved(&self) -> Self {
        let (w, h) = ((self.w / 2).max(1), (self.h / 2).max(1));
        let mut px = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0u32; 4];
                let mut n = 0;
                for dy in 0..2 {
                    for dx in 0..2 {
                        let p = self.get(x * 2 + dx, y * 2 + dy);
                        for k in 0..4 {
                            acc[k] += u32::from(p[k]);
                        }
                        n += 1;
                    }
                }
                px.push([
                    ((acc[0] + n / 2) / n) as u8,
                    ((acc[1] + n / 2) / n) as u8,
                    ((acc[2] + n / 2) / n) as u8,
                    ((acc[3] + n / 2) / n) as u8,
                ]);
            }
        }
        Self { w, h, px, has_alpha: self.has_alpha }
    }
}

/// `CharacterAtlas.HasAlphaChannel` for a Unity `TextureFormat`.
#[must_use]
pub fn format_has_alpha(unity_format: i64) -> bool {
    !matches!(unity_format, 3 | 7 | 9 | 10 | 15 | 16 | 22 | 26 | 27 | 28 | 62 | 63)
}

fn from_bgra(px: &[u32]) -> Vec<[u8; 4]> {
    px.iter()
        .map(|p| {
            let b = p.to_le_bytes();
            [b[2], b[1], b[0], b[3]]
        })
        .collect()
}

/// Decodes mip 0 of a Unity `Texture2D`.
///
/// # Errors
///
/// An unsupported format or a payload shorter than mip 0.
pub fn decode(universe: &Universe, v: &Value) -> Result<Image> {
    let name = v.str_at("m_Name").unwrap_or_default();
    let fmt = v.i64_at("m_TextureFormat").unwrap_or(-1);
    let (w, h) = (
        usize::try_from(v.i64_at("m_Width").unwrap_or(0))?,
        usize::try_from(v.i64_at("m_Height").unwrap_or(0))?,
    );
    let raw = payload(universe, v)?;
    let mut bgra = vec![0u32; w * h];
    let block = |r: Result<(), &'static str>| r.map_err(|e| anyhow::anyhow!("{name}: {e}"));
    let px: Vec<[u8; 4]> = match fmt {
        10 => {
            block(texture2ddecoder::decode_bc1(&raw, w, h, &mut bgra))?;
            from_bgra(&bgra)
        }
        12 => {
            block(texture2ddecoder::decode_bc3(&raw, w, h, &mut bgra))?;
            from_bgra(&bgra)
        }
        25 => {
            block(texture2ddecoder::decode_bc7(&raw, w, h, &mut bgra))?;
            from_bgra(&bgra)
        }
        26 => {
            block(texture2ddecoder::decode_bc4(&raw, w, h, &mut bgra))?;
            from_bgra(&bgra)
        }
        27 => {
            block(texture2ddecoder::decode_bc5(&raw, w, h, &mut bgra))?;
            from_bgra(&bgra)
        }
        28 | 29 => {
            block(texture2ddecoder::decode_unity_crunch(&raw, w, h, &mut bgra))?;
            from_bgra(&bgra)
        }
        4 => raw.as_chunks::<4>().0.to_vec(),
        14 => raw.as_chunks::<4>().0.iter().map(|p| [p[2], p[1], p[0], p[3]]).collect(),
        5 => raw.as_chunks::<4>().0.iter().map(|p| [p[1], p[2], p[3], p[0]]).collect(),
        3 => raw.as_chunks::<3>().0.iter().map(|p| [p[0], p[1], p[2], 255]).collect(),
        63 => raw.iter().map(|&r| [r, r, r, 255]).collect(),
        62 => raw.as_chunks::<2>().0.iter().map(|p| [p[0], p[1], 0, 255]).collect(),
        1 => raw.iter().map(|&a| [255, 255, 255, a]).collect(),
        other => bail!("{name}: texture format {other} not supported for composition"),
    };
    if px.len() < w * h {
        bail!("{name}: payload decodes to {} of {} texels", px.len(), w * h);
    }
    Ok(Image { w, h, px: px[..w * h].to_vec(), has_alpha: format_has_alpha(fmt) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bilinear_sampling_interpolates_between_texel_centres() {
        let img =
            Image { w: 2, h: 1, px: vec![[0, 0, 0, 255], [255, 255, 255, 255]], has_alpha: true };
        let s = img.sample(0.5, 0.5);
        assert!((s[0] - 0.5).abs() < 1e-6);
        // Clamped outside the centres.
        assert_eq!(img.sample(0.0, 0.5)[0], 0.0);
        assert_eq!(img.sample(1.0, 0.5)[0], 1.0);
    }

    #[test]
    fn halving_averages_blocks() {
        let img = Image {
            w: 2,
            h: 2,
            px: vec![[0, 0, 0, 0], [100, 0, 0, 0], [100, 0, 0, 0], [200, 0, 0, 255]],
            has_alpha: true,
        };
        let h = img.halved();
        assert_eq!((h.w, h.h), (1, 1));
        assert_eq!(h.px[0], [100, 0, 0, 64]);
    }

    #[test]
    fn alpha_formats_follow_the_atlas_rule() {
        assert!(format_has_alpha(12));
        assert!(format_has_alpha(4));
        assert!(!format_has_alpha(10));
        assert!(!format_has_alpha(3));
        assert!(!format_has_alpha(26));
    }
}
