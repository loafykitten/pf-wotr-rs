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

use super::image::{Image, linear_to_srgb, srgb_to_linear};

/// A blit destination: its pixel range and the fractional rectangle the UVs are measured in.
#[derive(Debug, Clone, Copy)]
struct Region {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    rx: f32,
    ry: f32,
    rw: f32,
    rh: f32,
}

impl Region {
    /// UV of pixel `(x, y)`'s centre in the rectangle.
    fn uv(&self, x: usize, y: usize) -> (f32, f32) {
        ((x as f32 + 0.5 - self.rx) / self.rw, (y as f32 + 0.5 - self.ry) / self.rh)
    }

    /// Whole pixels across, for picking the source mip.
    fn width(&self) -> usize {
        self.rw.ceil().max(1.0) as usize
    }
}

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

    /// The destination of `rect` (layout units scaled to this atlas). The quad keeps its fractional
    /// bounds, as the shader's `_DstRect` does: a pixel is drawn when its centre is inside, and its
    /// source UV is measured from the fractional corner.
    fn region(&self, rect: Rect) -> Region {
        let s = self.size as f32 / 2048.0;
        let (rx, ry, rw, rh) = (rect.0 * s, rect.1 * s, rect.2 * s, rect.3 * s);
        let lo = |v: f32| (v - 0.5).ceil().max(0.0) as usize;
        let hi = |v: f32| ((v - 0.5).ceil().max(0.0) as usize).min(self.size);
        Region { x0: lo(rx), y0: lo(ry), x1: hi(rx + rw), y1: hi(ry + rh), rx, ry, rw, rh }
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
        let r = self.region(rect);
        let (x0, y0, x1, y1) = (r.x0, r.y0, r.x1, r.y1);
        let source = Source::new(src, r.width());
        for y in y0..y1 {
            for x in x0..x1 {
                let (u, v) = r.uv(x, y);
                let s = source.sample(u, v);
                let a = weight.sample(u, v)[3];
                let prev = self.px[y * self.size + x];
                let mut out = prev;
                for k in 0..3 {
                    let p = srgb_to_linear(prev[k]);
                    out[k] = linear_to_srgb(p + a * (s[k] - p));
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
        let r = self.region(rect);
        let (x0, y0, x1, y1) = (r.x0, r.y0, r.x1, r.y1);
        let source = Source::new(src, r.width());
        for y in y0..y1 {
            for x in x0..x1 {
                let (u, v) = r.uv(x, y);
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
        let r = self.region(rect);
        let (x0, y0, x1, y1) = (r.x0, r.y0, r.x1, r.y1);
        let source = src.map(|s| Source::new(s, r.width()));
        for y in y0..y1 {
            for x in x0..x1 {
                let (u, v) = r.uv(x, y);
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
        let mut level = Image {
            w: self.size,
            h: self.size,
            px: self.px.clone(),
            has_alpha: true,
            srgb: self.channel == Channel::Diffuse,
        };
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
    // The shader re-encodes the decoded texel to gamma before taking its luminance.
    let encode = |l: f32, srgb: bool| if srgb { f32::from(linear_to_srgb(l)) / 255.0 } else { l };
    for y in 0..target.h {
        for x in 0..target.w {
            let (u, v) = ((x as f32 + 0.5) / target.w as f32, (y as f32 + 0.5) / target.h as f32);
            let m = main.sample(u, v);
            let luma = 0.299 * encode(m[0], main.srgb)
                + 0.587 * encode(m[1], main.srgb)
                + 0.114 * encode(m[2], main.srgb);
            let r = ramp.sample(luma.clamp(0.0, 1.0), 0.5);
            let mk = mask.sample(u, v);
            let a = if primary { mk[0] } else { mk[1] };
            let prev = target.px[y * target.w + x];
            let mut out = prev;
            for k in 0..3 {
                let p = srgb_to_linear(prev[k]);
                out[k] = linear_to_srgb(p + a * (r[k] - p));
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

    fn solid_srgb(w: usize, h: usize, c: [u8; 4]) -> Image {
        Image { srgb: true, ..Image::filled(w, h, c) }
    }

    #[test]
    fn diffuse_blit_lerps_by_the_textures_alpha_in_the_rectangle_only() {
        let mut a = Atlas::new(2048, Channel::Diffuse);
        let red = solid_srgb(4, 4, [255, 0, 0, 255]);
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
    fn a_full_weight_diffuse_blit_reproduces_an_srgb_texture() {
        let mut a = Atlas::new(2048, Channel::Diffuse);
        let grey = solid_srgb(4, 4, [128, 64, 200, 255]);
        a.blit_diffuse((0.0, 0.0, 16.0, 16.0), &grey, &grey, false);
        let p = a.px[8 * 2048 + 8];
        assert!(
            (i32::from(p[0]) - 128).abs() <= 1
                && (i32::from(p[1]) - 64).abs() <= 1
                && (i32::from(p[2]) - 200).abs() <= 1,
            "{p:?}"
        );
    }

    #[test]
    fn fractional_rectangles_keep_their_corner_for_the_source_uvs() {
        // At 1024 px the brows rectangle (1295, 193, 265, 125) is (647.5, 96.5, 132.5, 62.5). A pixel is
        // drawn when its centre is inside, centres on the left and top edges included: columns 647..=779,
        // rows 96..=158. The first column's centre sits exactly on the corner, so its u is 0 (not the
        // 0.5/132 a rounded rectangle gives), and every u is measured from the fractional corner.
        // A high-frequency source (alternating black and white texels) makes any shift of u show.
        let mut a = Atlas::new(1024, Channel::Diffuse);
        let stripes = Image {
            w: 256,
            h: 1,
            px: (0..256).map(|i| [(i % 2) as u8 * 255, 0, 0, 255]).collect(),
            has_alpha: true,
            srgb: false,
        };
        let on = solid(2, 2, [0, 0, 0, 255]);
        a.blit_diffuse((1295.0, 193.0, 265.0, 125.0), &stripes, &on, false);
        let drawn = |x: usize, y: usize| a.px[y * 1024 + x][3] == 255;
        // Exact boundary pixels, horizontally and vertically.
        assert!(!drawn(646, 100) && drawn(647, 100), "left edge");
        assert!(drawn(779, 100) && !drawn(780, 100), "right edge");
        assert!(!drawn(700, 95) && drawn(700, 96), "top edge");
        assert!(drawn(700, 158) && !drawn(700, 159), "bottom edge");
        // Interior values: what the sampler returns at the exact fractional u, encoded as the sRGB
        // target stores it (the source is raw, so it reads as linear light).
        let encode = |v: f32| crate::pack::units::image::linear_to_srgb(v);
        let mut differs_from_rounded = 0;
        for x in [647usize, 648, 649, 700, 714, 778, 779] {
            let u_exact = (x as f32 + 0.5 - 647.5) / 132.5;
            let u_rounded = (x as f32 - 648.0 + 0.5) / 132.0;
            let want = encode(stripes.sample(u_exact, 0.5)[0]);
            let old = encode(stripes.sample(u_rounded, 0.5)[0]);
            let got = a.px[100 * 1024 + x][0];
            assert!(got.abs_diff(want) <= 1, "x {x}: got {got}, want {want}");
            differs_from_rounded += usize::from(want.abs_diff(old) > 8);
        }
        // The test only means something if the rounded rectangle would have given other values.
        assert!(differs_from_rounded >= 3, "{differs_from_rounded}");
    }

    #[test]
    fn diffuse_blit_can_keep_the_previous_alpha() {
        let mut a = Atlas::new(2048, Channel::Diffuse);
        let tex = solid_srgb(2, 2, [10, 20, 30, 255]);
        a.blit_diffuse((0.0, 0.0, 8.0, 8.0), &tex, &tex, true);
        assert_eq!(a.px[2 * 2048 + 2][3], 0);
    }

    #[test]
    fn rectangles_scale_with_the_atlas() {
        let mut a = Atlas::new(1024, Channel::Diffuse);
        let tex = solid_srgb(2, 2, [255, 255, 255, 255]);
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
        let ramp = Image {
            w: 2,
            h: 1,
            px: vec![[0, 0, 0, 255], [255, 0, 0, 255]],
            has_alpha: true,
            srgb: true,
        };
        let main = solid_srgb(2, 2, [128, 128, 128, 255]);
        let mask_on = solid(2, 2, [255, 0, 0, 255]);
        let mut t = main.clone();
        repaint(&mut t, &main, &ramp, &mask_on, true);
        // Half way along the ramp in linear light is sRGB 188.
        assert!((i32::from(t.px[0][0]) - 188).abs() <= 3 && t.px[0][1] == 0, "{:?}", t.px[0]);
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
