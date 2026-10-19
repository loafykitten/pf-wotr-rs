//! The character atlases, composed on the CPU: Owlcat's `CharacterAtlas.Build` and the
//! `Hidden/Character*` shaders it blits with (`CharacterAtlasGenerator`, `CharacterTextureRepaint`),
//! read from the player's MSL.
//!
//! One atlas per channel (diffuse, normal, masks), `size` pixels square. Each body part's textures
//! are blitted into the part type's fixed rectangle of the 2048-pixel layout (scaled by
//! `size / 2048`); meshes carry UVs already in that layout. Blits run in the order the character
//! lists its overlay parts, each reading what the previous ones left (`_PreviousTex`).
//!
//! Colours are blended where the GPU blends them: the diffuse atlas and the repaint target are
//! sRGB render textures, so their RGB is decoded, mixed and encoded; the normal and mask atlases
//! are linear.

use super::image::Image;

/// A rectangle of the atlas in the 2048-pixel layout, `(x, y, w, h)`, `y` from the bottom.
pub type Rect = (f32, f32, f32, f32);

/// `CharacterTextureChannel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Diffuse,
    Normal,
    Masks,
}

impl Channel {
    /// The clear colour of a fresh atlas (`CharacterAtlas.Build`).
    #[must_use]
    pub fn clear(self) -> [u8; 4] {
        match self {
            Self::Diffuse => [0, 0, 0, 0],
            Self::Masks => [217, 0, 0, 0],
            Self::Normal => [128, 128, 255, 255],
        }
    }
}

fn srgb_to_linear(b: u8) -> f32 {
    let c = f32::from(b) / 255.0;
    if c <= 0.040_45 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(l: f32) -> u8 {
    let l = l.clamp(0.0, 1.0);
    let c = if l <= 0.003_130_8 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 };
    (c * 255.0 + 0.5) as u8
}

fn unorm(f: f32) -> u8 {
    (f.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// A square RGBA8 atlas.
#[derive(Debug, Clone)]
pub struct Atlas {
    pub size: usize,
    pub channel: Channel,
    pub px: Vec<[u8; 4]>,
}

/// Source texels for a destination of `dst` pixels: the image halved while that still leaves at
/// least `dst` texels (the GPU would pick a mip), then sampled bilinearly.
struct Source {
    levels: Vec<Image>,
}

impl Source {
    fn new(img: &Image, dst_w: usize) -> Self {
        let mut levels = vec![img.clone()];
        while levels.last().is_some_and(|l| l.w / 2 >= dst_w.max(1) && l.w > 1 && l.h > 1) {
            let next = levels.last().map(Image::halved).expect("a level");
            levels.push(next);
        }
        Self { levels }
    }

    fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        self.levels.last().expect("a level").sample(u, v)
    }
}

impl Atlas {
    #[must_use]
    pub fn new(size: usize, channel: Channel) -> Self {
        Self { size, channel, px: vec![channel.clear(); size * size] }
    }

    /// The destination pixels of `rect` (layout units scaled to this atlas) and each one's UV in
    /// the rectangle.
    fn region(&self, rect: Rect) -> (usize, usize, usize, usize) {
        let s = self.size as f32 / 2048.0;
        let (x, y, w, h) = (rect.0 * s, rect.1 * s, rect.2 * s, rect.3 * s);
        let (x0, y0) = (x.round() as usize, y.round() as usize);
        let (x1, y1) =
            (((x + w).round() as usize).min(self.size), ((y + h).round() as usize).min(self.size));
        (x0, y0, x1.max(x0), y1.max(y0))
    }

    /// `CharacterAtlasGenerator` for the diffuse channel: `out = lerp(prev, src, alpha)`, with the
    /// diffuse texture's alpha as the weight; `dont_blend_alpha` keeps the previous alpha.
    pub fn blit_diffuse(
        &mut self,
        rect: Rect,
        src: &Image,
        weight: &Image,
        dont_blend_alpha: bool,
    ) {
        let (x0, y0, x1, y1) = self.region(rect);
        let (w, h) = ((x1 - x0).max(1) as f32, (y1 - y0).max(1) as f32);
        let source = Source::new(src, x1 - x0);
        for y in y0..y1 {
            for x in x0..x1 {
                let (u, v) = ((x - x0) as f32 / w + 0.5 / w, (y - y0) as f32 / h + 0.5 / h);
                let s = source.sample(u, v);
                let a = weight.sample(u, v)[3];
                let prev = self.px[y * self.size + x];
                let mut out = prev;
                for k in 0..3 {
                    let p = srgb_to_linear(prev[k]);
                    let q = srgb_to_linear(unorm(s[k]));
                    out[k] = linear_to_srgb(p + a * (q - p));
                }
                if !dont_blend_alpha {
                    out[3] =
                        unorm(f32::from(prev[3]) / 255.0 + a * (s[3] - f32::from(prev[3]) / 255.0));
                }
                self.px[y * self.size + x] = out;
            }
        }
    }

    /// `CharacterAtlasGenerator` with `ALPHA_MASK_ON` and `NORMAL_MAP_ON`: the shipped normal map
    /// (x in alpha scaled by red, y in green) decoded to a vector, faded toward `(0, 0, 1)` by
    /// the weight, stored as a colour and blended over the previous contents.
    pub fn blit_normal(&mut self, rect: Rect, src: &Image, weight: &Image, dont_blend_alpha: bool) {
        let (x0, y0, x1, y1) = self.region(rect);
        let (w, h) = ((x1 - x0).max(1) as f32, (y1 - y0).max(1) as f32);
        let source = Source::new(src, x1 - x0);
        for y in y0..y1 {
            for x in x0..x1 {
                let (u, v) = ((x - x0) as f32 / w + 0.5 / w, (y - y0) as f32 / h + 0.5 / h);
                let s = source.sample(u, v);
                let a = weight.sample(u, v)[3];
                let nx = s[3] * s[0];
                let ny = s[1];
                let (fx, fy) = (nx * 2.0 - 1.0, ny * 2.0 - 1.0);
                let z = (1.0 - (fx * fx + fy * fy).min(1.0)).sqrt();
                // `(2x - 1, 2y - 1, z - 1)` faded: `a * d + (0, 0, 1)`, then `* 0.5 + 0.5`.
                let n =
                    [(a * fx) * 0.5 + 0.5, (a * fy) * 0.5 + 0.5, (a * (z - 1.0) + 1.0) * 0.5 + 0.5];
                let prev = self.px[y * self.size + x];
                let mut out = prev;
                for k in 0..3 {
                    let p = f32::from(prev[k]) / 255.0;
                    out[k] = unorm(p + a * (n[k] - p));
                }
                if !dont_blend_alpha {
                    let p = f32::from(prev[3]) / 255.0;
                    out[3] = unorm(p + a * (a - p));
                }
                self.px[y * self.size + x] = out;
            }
        }
    }

    /// `CharacterAtlasGenerator` with `ALPHA_MASK_ON` for the masks channel: the mask texture
    /// scaled by the material's roughness, emission and metallic factors, added over the previous
    /// contents faded by `1 - weight` (emission at a tenth). `src` is `None` for an empty mask
    /// (`(1, 0, 0, 0)`).
    pub fn blit_masks(
        &mut self,
        rect: Rect,
        src: Option<&Image>,
        weight: &Image,
        factors: [f32; 3],
        dont_blend_alpha: bool,
    ) {
        let (x0, y0, x1, y1) = self.region(rect);
        let (w, h) = ((x1 - x0).max(1) as f32, (y1 - y0).max(1) as f32);
        let source = src.map(|s| Source::new(s, x1 - x0));
        for y in y0..y1 {
            for x in x0..x1 {
                let (u, v) = ((x - x0) as f32 / w + 0.5 / w, (y - y0) as f32 / h + 0.5 / h);
                let s = source.as_ref().map_or([1.0, 0.0, 0.0, 0.0], |s| s.sample(u, v));
                let a = weight.sample(u, v)[3];
                let prev = self.px[y * self.size + x];
                let p = prev.map(|c| f32::from(c) / 255.0);
                let k = 1.0 - a;
                let m = [s[0] * factors[0], s[1] * factors[1], s[2] * factors[2]];
                let out = [
                    p[0] * k + m[0] * a,
                    p[1] * k + m[1] * a * 0.1,
                    p[2] * k + m[2] * a,
                    if dont_blend_alpha { p[3] * k } else { p[3] * k + s[3] * a },
                ];
                self.px[y * self.size + x] = out.map(unorm);
            }
        }
    }

    /// The atlas as a mip chain (box filter), level 0 first, tightly packed RGBA8.
    #[must_use]
    pub fn with_mips(&self) -> (Vec<u8>, u32) {
        let mut level = Image { w: self.size, h: self.size, px: self.px.clone(), has_alpha: true };
        let mut out: Vec<u8> = Vec::new();
        let mut n = 0;
        loop {
            out.extend(level.px.iter().flatten());
            n += 1;
            if level.w == 1 && level.h == 1 {
                break;
            }
            level = level.halved();
        }
        (out, n)
    }
}

/// `Hidden/CharacterTextureRepaint`, as `CharacterTextureDescription.Repaint` runs it: `target`
/// (a copy of the diffuse texture) gets the ramp colour for each texel's luminance, blended in by
/// the mask. `primary` picks the mask channel: red for the primary ramp, green for the secondary.
pub fn repaint(target: &mut Image, main: &Image, ramp: &Image, mask: &Image, primary: bool) {
    for y in 0..target.h {
        for x in 0..target.w {
            let (u, v) = ((x as f32 + 0.5) / target.w as f32, (y as f32 + 0.5) / target.h as f32);
            let m = main.sample(u, v);
            // The shader re-encodes the decoded texel to gamma before taking its luminance: the
            // stored bytes themselves.
            let luma = 0.299 * m[0] + 0.587 * m[1] + 0.114 * m[2];
            let r = ramp.sample(luma.clamp(0.0, 1.0), 0.5);
            let mk = mask.sample(u, v);
            let a = if primary { mk[0] } else { mk[1] };
            let prev = target.px[y * target.w + x];
            let mut out = prev;
            for k in 0..3 {
                let p = srgb_to_linear(prev[k]);
                let q = srgb_to_linear(unorm(r[k]));
                out[k] = linear_to_srgb(p + a * (q - p));
            }
            target.px[y * target.w + x] = out;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, c: [u8; 4]) -> Image {
        Image::filled(w, h, c)
    }

    #[test]
    fn diffuse_blit_lerps_by_the_textures_alpha_in_the_rectangle_only() {
        let mut a = Atlas::new(2048, Channel::Diffuse);
        let red = solid(4, 4, [255, 0, 0, 255]);
        a.blit_diffuse((0.0, 0.0, 100.0, 50.0), &red, &red, false);
        assert_eq!(a.px[10 * 2048 + 10], [255, 0, 0, 255]);
        assert_eq!(a.px[60 * 2048 + 10], [0, 0, 0, 0]);
        // Half weight mixes in linear light: sRGB 188 is half of linear 1.0.
        let mut b = Atlas::new(2048, Channel::Diffuse);
        let half = solid(4, 4, [0, 0, 0, 128]);
        b.blit_diffuse((0.0, 0.0, 10.0, 10.0), &red, &half, false);
        let p = b.px[5 * 2048 + 5];
        assert!((i32::from(p[0]) - 188).abs() <= 2, "{p:?}");
        assert!((i32::from(p[3]) - 128).abs() <= 2, "{p:?}");
    }

    #[test]
    fn diffuse_blit_can_keep_the_previous_alpha() {
        let mut a = Atlas::new(2048, Channel::Diffuse);
        let tex = solid(2, 2, [10, 20, 30, 255]);
        a.blit_diffuse((0.0, 0.0, 8.0, 8.0), &tex, &tex, true);
        assert_eq!(a.px[2 * 2048 + 2][3], 0);
    }

    #[test]
    fn rectangles_scale_with_the_atlas() {
        let mut a = Atlas::new(1024, Channel::Diffuse);
        let tex = solid(2, 2, [255, 255, 255, 255]);
        a.blit_diffuse((0.0, 0.0, 200.0, 200.0), &tex, &tex, false);
        assert_eq!(a.px[99 * 1024 + 99][3], 255);
        assert_eq!(a.px[101 * 1024 + 99][3], 0);
    }

    #[test]
    fn flat_normal_maps_decode_to_the_flat_colour() {
        // x = alpha * red and y = green at 0.5 decode to (0, 0): a flat normal, stored (128,128,255).
        let mut a = Atlas::new(2048, Channel::Normal);
        let flat = solid(2, 2, [255, 128, 0, 128]);
        let on = solid(2, 2, [0, 0, 0, 255]);
        a.blit_normal((0.0, 0.0, 8.0, 8.0), &flat, &on, true);
        let p = a.px[3 * 2048 + 3];
        assert!((i32::from(p[0]) - 128).abs() <= 2 && (i32::from(p[1]) - 128).abs() <= 2, "{p:?}");
        assert!(p[2] >= 253, "{p:?}");
    }

    #[test]
    fn mask_blit_scales_channels_and_fades_the_previous_contents() {
        let mut a = Atlas::new(2048, Channel::Masks);
        let src = solid(2, 2, [200, 100, 50, 0]);
        let on = solid(2, 2, [0, 0, 0, 255]);
        a.blit_masks((0.0, 0.0, 8.0, 8.0), Some(&src), &on, [0.5, 1.0, 1.0], true);
        let p = a.px[3 * 2048 + 3];
        assert_eq!(p[0], 100);
        assert_eq!(p[1], 10);
        assert_eq!(p[2], 50);
        // An empty mask is (1, 0, 0, 0): full smoothness channel, nothing else.
        let mut b = Atlas::new(2048, Channel::Masks);
        b.blit_masks((0.0, 0.0, 8.0, 8.0), None, &on, [1.0, 1.0, 1.0], true);
        assert_eq!(b.px[3 * 2048 + 3][0], 255);
    }

    #[test]
    fn repaint_maps_luminance_through_the_ramp_under_the_mask() {
        // A ramp from black to red; a mid-grey texel picks half red where the mask is on.
        let ramp =
            Image { w: 2, h: 1, px: vec![[0, 0, 0, 255], [255, 0, 0, 255]], has_alpha: true };
        let main = solid(2, 2, [128, 128, 128, 255]);
        let mask_on = solid(2, 2, [255, 0, 0, 255]);
        let mut t = main.clone();
        repaint(&mut t, &main, &ramp, &mask_on, true);
        assert!((i32::from(t.px[0][0]) - 128).abs() <= 3 && t.px[0][1] == 0, "{:?}", t.px[0]);
        // The secondary pass reads the other mask channel (off here): the target stays as it was.
        let mut u = main.clone();
        repaint(&mut u, &main, &ramp, &mask_on, false);
        assert_eq!(u.px[0], [128, 128, 128, 255]);
    }

    #[test]
    fn mip_chain_runs_to_one_texel() {
        let a = Atlas::new(4, Channel::Diffuse);
        let (data, n) = a.with_mips();
        assert_eq!(n, 3);
        assert_eq!(data.len(), (16 + 4 + 1) * 4);
    }
}
