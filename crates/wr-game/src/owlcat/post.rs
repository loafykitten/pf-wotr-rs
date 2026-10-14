//! Post-processing constants from the light scene's volumes, ported from
//! `ColorGradingLutPass` (the grading LUT) and `PostProcessPass` (Uber: LUT
//! application and vignette). Volume evaluation follows Unity's `VolumeStack`: each
//! parameter starts at the component's default and takes the value of an active
//! component that overrides it (all volumes here are global with weight 1).

use bevy::math::{Vec3, Vec4};
use serde_json::Value as J;
use wr_assets::Volume;

use super::consts::{Consts, gamma_to_linear, linear};

/// LUT size (`PostProcessingData.LutSize`, the pipeline asset's default).
pub const LUT_SIZE: u32 = 32;

/// Looks up an overridden parameter of an active component.
fn param<'a>(components: &'a [(String, J)], class: &str, name: &str) -> Option<&'a J> {
    components
        .iter()
        .filter(|(c, v)| {
            c.rsplit('.').next() == Some(class) && v["active"].as_i64().unwrap_or(1) != 0
        })
        .find_map(|(_, v)| {
            let p = &v[name];
            (p["m_OverrideState"].as_i64() == Some(1)).then_some(&p["m_Value"])
        })
}

fn f(components: &[(String, J)], class: &str, name: &str, default: f32) -> f32 {
    param(components, class, name)
        .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)))
        .map_or(default, |x| x as f32)
}

fn v4(components: &[(String, J)], class: &str, name: &str, default: [f32; 4]) -> Vec4 {
    let Some(v) = param(components, class, name) else { return Vec4::from(default) };
    let g = |k: &str, d: f32| v[k].as_f64().map_or(d, |x| x as f32);
    if v.get("r").is_some() {
        Vec4::new(g("r", default[0]), g("g", default[1]), g("b", default[2]), g("a", default[3]))
    } else {
        Vec4::new(g("x", default[0]), g("y", default[1]), g("z", default[2]), g("w", default[3]))
    }
}

/// The evaluated post-processing settings.
#[derive(Debug, Clone)]
pub struct Post {
    pub lut: Consts,
    pub uber: Consts,
    /// `TonemappingMode`: 0 none, 1 neutral, 2 ACES.
    pub tonemap: i64,
    /// Components found (for the report).
    pub components: Vec<String>,
    /// `BloomEnhanced`, when active (`intensity > 0`).
    pub bloom: Option<Bloom>,
}

/// `PostProcessPass.SetupBloomEnhanced`: the pyramid's shape and its materials' constants.
#[derive(Debug, Clone)]
pub struct Bloom {
    /// Prefilter size: half the camera target (integer division).
    pub half: (u32, u32),
    /// Downsample count: `clamp((int)(log2(half_h) + radius - 8), 1, 16)`.
    pub iterations: u32,
    pub anti_flicker: bool,
    /// The bloom material: `_Params`, `_Params1`, `_Curve`.
    pub material: Consts,
    /// Lens dirt intensity. The original selects Uber's `_BLOOM_HQ_DIRT` when > 0; we
    /// don't import the dirt texture yet, so the renderer logs a skip and uses `_BLOOM_HQ`.
    pub dirt: f32,
}

impl Bloom {
    fn from(c: &[(String, J)], width: u32, height: u32) -> Option<Self> {
        let k = "BloomEnhanced";
        let intensity = f(c, k, "intensity", 0.0);
        if intensity <= 0.0
            || !c.iter().any(|(n, v)| {
                n.rsplit('.').next() == Some(k) && v["active"].as_i64().unwrap_or(1) != 0
            })
        {
            return None;
        }
        let half = (width / 2, height / 2);
        let n3 = (half.1 as f32).log2() + f(c, k, "radius", 4.0) - 8.0;
        let n4 = n3 as i32;
        let iterations = n4.clamp(1, 16) as u32;
        let threshold = gamma_to_linear(f(c, k, "threshold", 1.1));
        let anti_flicker = param(c, k, "antiFlicker").and_then(J::as_i64).unwrap_or(0) != 0;
        let dirt = f(c, k, "dirtIntensity", 0.0);
        let mut material = Consts::default();
        material.f(
            "_Params",
            &[threshold, if anti_flicker { -0.5 } else { 0.0 }, 0.5 + n3 - n4 as f32, dirt],
        );
        material.f("_Params1", &[f(c, k, "clamp", 65472.0), 0.0, 0.0, 0.0]);
        let knee = threshold * f(c, k, "softKnee", 0.5) + 1e-5;
        material.f("_Curve", &[threshold - knee, knee * 2.0, 0.25 / knee]);
        Some(Self { half, iterations, anti_flicker, material, dirt })
    }

    /// The pyramid's level sizes after the prefilter: each halves (at least 1).
    #[must_use]
    pub fn levels(&self) -> Vec<(u32, u32)> {
        let (mut w, mut h) = self.half;
        (0..self.iterations)
            .map(|_| {
                w = (w / 2).max(1);
                h = (h / 2).max(1);
                (w, h)
            })
            .collect()
    }
}

fn luminance(v: Vec3) -> f32 {
    v.dot(Vec3::new(0.2126729, 0.7151522, 0.0721750))
}

fn standard_illuminant_y(x: f32) -> f32 {
    2.87 * x - 3.0 * x * x - 0.275_095_07
}

fn cie_xy_to_lms(x: f32, y: f32) -> Vec3 {
    let big_y = 1.0;
    let big_x = big_y * x / y;
    let big_z = big_y * (1.0 - x - y) / y;
    Vec3::new(
        0.7328 * big_x + 0.4296 * big_y - 0.1624 * big_z,
        -0.7036 * big_x + 1.6975 * big_y + 0.0061 * big_z,
        0.0030 * big_x + 0.0136 * big_y + 0.9834 * big_z,
    )
}

fn color_balance(temperature: f32, tint: f32) -> Vec3 {
    let t = temperature / 65.0;
    let ti = tint / 65.0;
    let x = 0.31271 - t * if t < 0.0 { 0.1 } else { 0.05 };
    let y = standard_illuminant_y(x) + ti * 0.05;
    let w = Vec3::new(0.949_237, 1.035_42, 1.087_28);
    let lms = cie_xy_to_lms(x, y);
    w / lms
}

fn smh(v: Vec4) -> Vec4 {
    let mut o = Vec4::new(gamma_to_linear(v.x), gamma_to_linear(v.y), gamma_to_linear(v.z), 0.0);
    let n = v.w * if v.w < 0.0 { 1.0 } else { 4.0 };
    o.x = (o.x + n).max(0.0);
    o.y = (o.y + n).max(0.0);
    o.z = (o.z + n).max(0.0);
    o
}

fn lift_gamma_gain(lift: Vec4, gamma: Vec4, gain: Vec4) -> (Vec4, Vec4, Vec4) {
    let lin = |v: Vec4, k: f32| {
        Vec3::new(gamma_to_linear(v.x) * k, gamma_to_linear(v.y) * k, gamma_to_linear(v.z) * k)
    };
    let l = lin(lift, 0.15);
    let ll = luminance(l);
    let lift_o = (l - Vec3::splat(ll) + Vec3::splat(lift.w)).extend(0.0);
    let g = lin(gamma, 0.8);
    let gl = luminance(g);
    let gw = gamma.w + 1.0;
    let gamma_o = Vec4::new(
        1.0 / (g.x - gl + gw).max(0.001),
        1.0 / (g.y - gl + gw).max(0.001),
        1.0 / (g.z - gl + gw).max(0.001),
        0.0,
    );
    let n = lin(gain, 0.8);
    let nl = luminance(n);
    let nw = gain.w + 1.0;
    let gain_o = (n - Vec3::splat(nl) + Vec3::splat(nw)).extend(0.0);
    (lift_o, gamma_o, gain_o)
}

fn normalize_color(c: Vec4) -> Vec3 {
    let s = (c.x + c.y + c.z) / 3.0;
    if s.abs() < 1e-6 { Vec3::ONE } else { c.truncate() / s }
}

fn slope_power_offset(slope: Vec4, power: Vec4, offset: Vec4) -> (Vec4, Vec4, Vec4) {
    let v = normalize_color(slope);
    let m = (v.x + v.y + v.z) / 3.0;
    let sw = slope.w * 0.5;
    let s = ((v - Vec3::splat(m)) * 0.1 + Vec3::splat(sw + 1.0))
        .clamp(Vec3::ZERO, Vec3::splat(2.0))
        .extend(0.0);
    let v2 = normalize_color(power);
    let m2 = (v2.x + v2.y + v2.z) / 3.0;
    let pw = power.w * 0.5;
    let b = (v2 - Vec3::splat(m2)) * 0.1 + Vec3::splat(pw + 1.0);
    let p = Vec3::new(1.0 / b.x.max(0.01), 1.0 / b.y.max(0.01), 1.0 / b.z.max(0.01))
        .clamp(Vec3::splat(0.5), Vec3::splat(2.5))
        .extend(0.0);
    let v3 = normalize_color(offset);
    let m3 = (v3.x + v3.y + v3.z) / 3.0;
    let ow = offset.w * 0.5;
    let o = ((v3 - Vec3::splat(m3)) * 0.05 + Vec3::splat(ow))
        .clamp(Vec3::splat(-0.8), Vec3::splat(0.8))
        .extend(0.0);
    (s, p, o)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn neutral(
    black_in: f32,
    white_in: f32,
    black_out: f32,
    white_out: f32,
    white_level: f32,
    white_clip: f32,
) -> (Vec4, Vec4) {
    let n = black_in * 20.0 + 1.0;
    let n2 = black_out * 10.0 + 1.0;
    let n3 = white_in / 20.0;
    let n4 = 1.0 - white_out / 20.0;
    let t = n / n2;
    let t2 = n3 / n4;
    let y = lerp(0.57, 0.37, t).max(0.0);
    let z = lerp(0.01, 0.24, t2);
    let w = lerp(0.02, 0.2, t).max(0.0);
    (Vec4::new(0.2, y, z, w), Vec4::new(0.02, 0.3, white_level, white_clip / 10.0))
}

impl Post {
    /// Evaluates the stack from the volumes (in order) for an output of `width × height`.
    #[must_use]
    pub fn from_volumes(
        volumes: &[Volume],
        width: u32,
        height: u32,
        gamma_setting: Option<f32>,
    ) -> Self {
        let mut comps: Vec<(String, J)> = volumes
            .iter()
            .flat_map(|v| v.components.iter())
            .map(|(c, j)| (c.clone(), serde_json::from_str(j).unwrap_or(J::Null)))
            .collect();
        // The player's gamma setting: `GraphicsSettingsController.SetGamma` writes
        // `lerp(-0.8, 0.8, value + 0.5)` into the gamma profile's LiftGammaGain.gamma.w
        // (`RenderPipelineBridge.GammaProfile`, a global volume). It is matched first
        // here only if the scene's own volumes don't override gamma.
        if let Some(g) = gamma_setting {
            let w = -0.8 + 1.6 * (g + 0.5);
            comps.push((
                "RenderPipelineBridge.GammaProfile.LiftGammaGain".into(),
                serde_json::json!({"active": 1, "gamma": {"m_OverrideState": 1, "m_Value": {"x": 1.0, "y": 1.0, "z": 1.0, "w": w}}}),
            ));
        }
        let c = &comps;
        let mut lut = Consts::default();
        let size = LUT_SIZE as f32;
        let n = size * size;
        lut.f("_Lut_Params", &[size, 0.5 / n, 0.5 / size, size / (size - 1.0)]);
        let cb = color_balance(
            f(c, "WhiteBalance", "temperature", 0.0),
            f(c, "WhiteBalance", "tint", 0.0),
        );
        lut.f("_ColorBalance", &[cb.x, cb.y, cb.z, 0.0]);
        let filt = v4(c, "ColorAdjustments", "colorFilter", [1.0; 4]);
        lut.f(
            "_ColorFilter",
            &[gamma_to_linear(filt.x), gamma_to_linear(filt.y), gamma_to_linear(filt.z), filt.w],
        );
        let cm = |n: &str, d: f32| f(c, "ChannelMixer", n, d) / 100.0;
        lut.f(
            "_ChannelMixerRed",
            &[cm("redOutRedIn", 100.0), cm("redOutGreenIn", 0.0), cm("redOutBlueIn", 0.0), 0.0],
        );
        lut.f(
            "_ChannelMixerGreen",
            &[
                cm("greenOutRedIn", 0.0),
                cm("greenOutGreenIn", 100.0),
                cm("greenOutBlueIn", 0.0),
                0.0,
            ],
        );
        lut.f(
            "_ChannelMixerBlue",
            &[cm("blueOutRedIn", 0.0), cm("blueOutGreenIn", 0.0), cm("blueOutBlueIn", 100.0), 0.0],
        );
        lut.f(
            "_HueSatCon",
            &[
                f(c, "ColorAdjustments", "hueShift", 0.0) / 360.0,
                f(c, "ColorAdjustments", "saturation", 0.0) / 100.0 + 1.0,
                f(c, "ColorAdjustments", "contrast", 0.0) / 100.0 + 1.0,
                0.0,
            ],
        );
        let one = [1.0, 1.0, 1.0, 0.0];
        lut.v4("_Shadows", smh(v4(c, "ShadowsMidtonesHighlights", "shadows", one)));
        lut.v4("_Midtones", smh(v4(c, "ShadowsMidtonesHighlights", "midtones", one)));
        lut.v4("_Highlights", smh(v4(c, "ShadowsMidtonesHighlights", "highlights", one)));
        lut.f(
            "_ShaHiLimits",
            &[
                f(c, "ShadowsMidtonesHighlights", "shadowsStart", 0.0),
                f(c, "ShadowsMidtonesHighlights", "shadowsEnd", 0.3),
                f(c, "ShadowsMidtonesHighlights", "highlightsStart", 0.55),
                f(c, "ShadowsMidtonesHighlights", "highlightsEnd", 1.0),
            ],
        );
        let (l, g, gn) = lift_gamma_gain(
            v4(c, "LiftGammaGain", "lift", one),
            v4(c, "LiftGammaGain", "gamma", one),
            v4(c, "LiftGammaGain", "gain", one),
        );
        lut.v4("_Lift", l);
        lut.v4("_Gamma", g);
        lut.v4("_Gain", gn);
        let (s, p, o) = slope_power_offset(
            v4(c, "SlopePowerOffset", "slope", one),
            v4(c, "SlopePowerOffset", "power", one),
            v4(c, "SlopePowerOffset", "offset", one),
        );
        lut.v4("_Slope", s);
        lut.v4("_Power", p);
        lut.v4("_Offset", o);
        let grey = [0.5, 0.5, 0.5, 1.0];
        let mut sh = v4(c, "SplitToning", "shadows", grey);
        sh.w = f(c, "SplitToning", "balance", 0.0) / 100.0;
        let mut hi = v4(c, "SplitToning", "highlights", grey);
        hi.w = 0.0;
        lut.v4("_SplitShadows", sh);
        lut.v4("_SplitHighlights", hi);
        let tonemap = param(c, "Tonemapping", "mode").and_then(J::as_i64).unwrap_or(0);
        let (n1, n2) = neutral(
            f(c, "Tonemapping", "neutralBlackIn", 0.02),
            f(c, "Tonemapping", "neutralWhiteIn", 10.0),
            f(c, "Tonemapping", "neutralBlackOut", 0.0),
            f(c, "Tonemapping", "neutralWhiteOut", 10.0),
            f(c, "Tonemapping", "neutralWhiteLevel", 5.3),
            f(c, "Tonemapping", "neutralWhiteClip", 10.0),
        );
        lut.v4("_NeutralTonemapperParams1", n1);
        lut.v4("_NeutralTonemapperParams2", n2);

        let mut uber = Consts::default();
        let post_exposure = 2f32.powf(f(c, "ColorAdjustments", "postExposure", 0.0));
        uber.f("_Lut_Params", &[1.0 / n, 1.0 / size, size - 1.0, post_exposure]);
        uber.f("_UserLut_Params", &[0.0; 4]);
        let vc = v4(c, "Vignette", "color", [0.0, 0.0, 0.0, 1.0]);
        let center = param(c, "Vignette", "center").map_or([0.5, 0.5], |v| {
            [v["x"].as_f64().unwrap_or(0.5) as f32, v["y"].as_f64().unwrap_or(0.5) as f32]
        });
        let rounded = param(c, "Vignette", "rounded").and_then(J::as_i64).unwrap_or(0) != 0;
        uber.f(
            "_Vignette_Params1",
            &[vc.x, vc.y, vc.z, if rounded { width as f32 / height as f32 } else { 1.0 }],
        );
        uber.f(
            "_Vignette_Params2",
            &[
                center[0],
                center[1],
                f(c, "Vignette", "intensity", 0.0) * 3.0,
                f(c, "Vignette", "smoothness", 0.2) * 5.0,
            ],
        );
        uber.f("_Sepia_Params", &[f(c, "Sepia", "Intensity", 0.0), 0.0, 0.0, 0.0]);
        let bloom = Bloom::from(c, width, height);
        if bloom.is_some() {
            // Tint: `Color.linear`, normalised by its luminance (white if black).
            let t = linear(v4(c, "BloomEnhanced", "tint", [1.0; 4]).to_array());
            let t = Vec3::new(t[0], t[1], t[2]);
            let l = luminance(t);
            let t = if l > 0.0 { t / l } else { Vec3::ONE };
            uber.f("_Bloom_Params", &[f(c, "BloomEnhanced", "intensity", 0.0), t.x, t.y, t.z]);
            // The bloom targets are RGBA16F (`m_DefaultHDRFormat`), so no RGBM.
            uber.f("_Bloom_RGBM", &[0.0]);
        }
        Self {
            bloom,
            lut,
            uber,
            tonemap,
            components: comps
                .iter()
                .filter(|(_, v)| v["active"].as_i64().unwrap_or(1) != 0)
                .map(|(c, _)| c.clone())
                .collect(),
        }
    }
}

/// `TextureCurve.GetTexture` for the default curves: 128 samples at `i / 128` (so the
/// last identity texel is 127/128), red channel; the RGB/master curves are the
/// identity, the hue/sat curves their zero value (0.5).
#[must_use]
pub fn curve_texels(identity: bool) -> Vec<f32> {
    (0..128).map(|i| if identity { i as f32 / 128.0 } else { 0.5 }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_identity_grading() {
        let p = Post::from_volumes(&[], 2560, 1440, None);
        let hsc = &p.lut.0["_HueSatCon"];
        assert_eq!(f32::from_bits(hsc[1]), 1.0);
        assert_eq!(f32::from_bits(hsc[2]), 1.0);
        // White balance at (0, 0) is ~identity.
        let cb = &p.lut.0["_ColorBalance"];
        for w in &cb[..3] {
            assert!((f32::from_bits(*w) - 1.0).abs() < 0.02, "{}", f32::from_bits(*w));
        }
        // Default slope/power/offset: (1, 1, 1), (1, 1, 1), (0, 0, 0).
        assert_eq!(f32::from_bits(p.lut.0["_Slope"][0]), 1.0);
        assert_eq!(f32::from_bits(p.lut.0["_Offset"][0]), 0.0);
        assert!(p.bloom.is_none());
    }

    #[test]
    fn bloom_enhanced_matches_the_cave_profile_shape() {
        // The cave profile: intensity 0.3, threshold 0.75, soft knee 0.5, radius 6,
        // anti-flicker on, at 2560×1440.
        let v = Volume {
            components: vec![(
                "Owlcat.Runtime.Visual.RenderPipeline.PostProcess.BloomEnhanced".into(),
                r#"{"active":1,"intensity":{"m_OverrideState":1,"m_Value":0.3},"threshold":{"m_OverrideState":1,"m_Value":0.75},"softKnee":{"m_OverrideState":1,"m_Value":0.5},"radius":{"m_OverrideState":1,"m_Value":6.0},"antiFlicker":{"m_OverrideState":1,"m_Value":1}}"#.into(),
            )],
            ..Default::default()
        };
        let p = Post::from_volumes(&[v], 2560, 1440, None);
        let b = p.bloom.expect("active");
        assert_eq!(b.half, (1280, 720));
        // log2(720) + 6 - 8 = 7.49 → 7 levels, 640×360 down to 10×5.
        assert_eq!(b.iterations, 7);
        assert_eq!(b.levels().first(), Some(&(640, 360)));
        assert_eq!(b.levels().last(), Some(&(10, 5)));
        let params = &b.material.0["_Params"];
        let t = f32::from_bits(params[0]);
        assert!((t - gamma_to_linear(0.75)).abs() < 1e-6);
        assert_eq!(f32::from_bits(params[1]), -0.5);
        assert!((f32::from_bits(params[2]) - (0.5 + 720f32.log2() - 2.0 - 7.0)).abs() < 1e-5);
        assert_eq!(f32::from_bits(p.uber.0["_Bloom_Params"][0]), 0.3);
    }

    #[test]
    fn overrides_apply_only_when_active_and_overridden() {
        let v = Volume {
            components: vec![(
                "Owlcat.Runtime.Visual.RenderPipeline.PostProcess.ColorAdjustments".into(),
                r#"{"active":1,"contrast":{"m_OverrideState":1,"m_Value":3.0},"saturation":{"m_OverrideState":0,"m_Value":5.0}}"#.into(),
            )],
            ..Default::default()
        };
        let p = Post::from_volumes(&[v], 2560, 1440, None);
        let hsc = &p.lut.0["_HueSatCon"];
        assert_eq!(f32::from_bits(hsc[1]), 1.0);
        assert!((f32::from_bits(hsc[2]) - 1.03).abs() < 1e-6);
    }
}
