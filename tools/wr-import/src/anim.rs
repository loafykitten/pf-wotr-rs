//! Unity `AnimationClip` objects decoded into `wr-anim` clips.

use anyhow::{Context, Result, bail, ensure};
use wr_anim::clip::{Binding, Clip, Dense, decode_streamed};

use crate::unity::value::Value;

fn words(v: Option<&Value>) -> Vec<u32> {
    v.and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .map(|w| match w {
            Value::UInt(u) => *u as u32,
            Value::Int(i) => *i as u32,
            _ => 0,
        })
        .collect()
}

fn floats(v: Option<&Value>) -> Vec<f32> {
    v.and_then(Value::as_array).unwrap_or_default().iter().filter_map(Value::as_f32).collect()
}

/// Decodes a generic (non-legacy) clip's muscle clip: streamed, dense and constant curves with
/// their bindings.
///
/// # Errors
///
/// Legacy, compressed or humanoid-only clips, and data that doesn't add up.
pub fn decode_clip(v: &Value) -> Result<Clip> {
    let name = v.str_at("m_Name").unwrap_or_default().to_owned();
    ensure!(!v.get("m_Legacy").and_then(Value::as_bool).unwrap_or(false), "{name}: legacy clip");
    ensure!(
        v.get("m_Compressed").and_then(Value::as_bool) != Some(true)
            && v.get("m_CompressedRotationCurves")
                .and_then(Value::as_array)
                .is_none_or(<[_]>::is_empty),
        "{name}: compressed curves"
    );
    let mc = v.get("m_MuscleClip").with_context(|| format!("{name}: no muscle clip"))?;
    let data = mc.path("m_Clip.data").with_context(|| format!("{name}: no clip data"))?;
    let sc = data.get("m_StreamedClip").context("streamed clip")?;
    let curve_count = usize::try_from(sc.i64_at("curveCount").unwrap_or(0))?;
    let streamed = decode_streamed(&words(sc.get("data")), curve_count)
        .with_context(|| format!("{name}: streamed clip"))?;
    let dc = data.get("m_DenseClip").context("dense clip")?;
    let dense = Dense {
        begin: dc.f32_at("m_BeginTime").unwrap_or(0.0),
        rate: dc.f32_at("m_SampleRate").unwrap_or(30.0),
        frames: u32::try_from(dc.i64_at("m_FrameCount").unwrap_or(0))?,
        curves: u32::try_from(dc.i64_at("m_CurveCount").unwrap_or(0))?,
        samples: floats(dc.get("m_SampleArray")),
    };
    let constant = floats(data.path("m_ConstantClip.data"));
    let mut bindings = Vec::new();
    for b in v
        .path("m_ClipBindingConstant.genericBindings")
        .and_then(Value::as_array)
        .unwrap_or_default()
    {
        if b.get("isPPtrCurve").and_then(Value::as_bool).unwrap_or(false) {
            bail!("{name}: PPtr curve binding");
        }
        bindings.push(Binding {
            path: b.i64_at("path").unwrap_or(0) as u32,
            attribute: b.i64_at("attribute").unwrap_or(0) as u32,
            type_id: b.i64_at("typeID").unwrap_or(0) as u32,
        });
    }
    let clip = Clip {
        name,
        start: mc.f32_at("m_StartTime").unwrap_or(0.0),
        stop: mc.f32_at("m_StopTime").unwrap_or(0.0),
        looping: mc.get("m_LoopTime").and_then(Value::as_bool).unwrap_or(false),
        bindings,
        streamed,
        dense,
        constant,
    };
    clip.validate()?;
    Ok(clip)
}

/// What [`CheckReport::add`] found over a set of clips.
#[derive(Debug, Default, serde::Serialize)]
pub struct CheckReport {
    pub clips: usize,
    /// Clips that decode (the rest are in `failures`).
    pub decoded: usize,
    pub failures: Vec<String>,
    pub non_finite: usize,
    /// Worst `Clip::continuity_error` and its clip.
    pub worst_continuity: (f32, String),
    /// Worst deviation of a sampled rotation quaternion's length from one.
    pub worst_quaternion_norm: f32,
    /// Looping clips, and the worst first-to-last-pose gap over their curves (a gap a clip's editor
    /// closes by blending is expected to be small, not zero).
    pub looping: usize,
    pub worst_loop_gap: (f32, String),
    /// Clips with a binding to a property other than a transform's.
    pub non_transform_bindings: usize,
    /// Clips whose path 0 (the root itself) is animated.
    pub root_bindings: usize,
}

impl CheckReport {
    pub fn add(&mut self, v: &Value) {
        self.clips += 1;
        let clip = match decode_clip(v) {
            Ok(c) => c,
            Err(e) => {
                self.failures.push(format!("{e:#}"));
                return;
            }
        };
        self.decoded += 1;
        let c = clip.continuity_error();
        if c > self.worst_continuity.0 {
            self.worst_continuity = (c, clip.name.clone());
        }
        let times: Vec<f32> =
            (0..=16).map(|i| clip.start + clip.length() * i as f32 / 16.0).collect();
        let mut out = Vec::new();
        let offsets = clip.binding_offsets();
        for t in &times {
            clip.sample(*t, &mut out);
            if out.iter().any(|x| !x.is_finite()) {
                self.non_finite += 1;
                break;
            }
            for (b, o) in clip.bindings.iter().zip(&offsets) {
                if b.type_id == wr_anim::clip::TYPE_TRANSFORM
                    && b.attribute == wr_anim::clip::ATTR_ROTATION
                {
                    let n = out[*o..*o + 4].iter().map(|x| x * x).sum::<f32>().sqrt();
                    self.worst_quaternion_norm = self.worst_quaternion_norm.max((n - 1.0).abs());
                }
            }
        }
        if clip.bindings.iter().any(|b| b.type_id != wr_anim::clip::TYPE_TRANSFORM) {
            self.non_transform_bindings += 1;
        }
        if clip.bindings.iter().any(|b| b.path == 0) {
            self.root_bindings += 1;
        }
        if clip.looping && clip.length() > 0.0 {
            self.looping += 1;
            let (mut a, mut b) = (Vec::new(), Vec::new());
            clip.sample(clip.start, &mut a);
            clip.sample(clip.stop, &mut b);
            let mut gap = 0.0f32;
            for (bd, o) in clip.bindings.iter().zip(&offsets) {
                let n = bd.components();
                let d =
                    |s: f32| (0..n).map(|k| (a[o + k] - s * b[o + k]).abs()).fold(0.0f32, f32::max);
                // A quaternion and its negation are the same rotation.
                let rot = bd.type_id == wr_anim::clip::TYPE_TRANSFORM
                    && bd.attribute == wr_anim::clip::ATTR_ROTATION;
                gap = gap.max(if rot { d(1.0).min(d(-1.0)) } else { d(1.0) });
            }
            if gap > self.worst_loop_gap.0 {
                self.worst_loop_gap = (gap, clip.name.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(fields: &[(&str, Value)]) -> Value {
        Value::Object(fields.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect())
    }

    fn uints(w: &[u32]) -> Value {
        Value::Array(w.iter().map(|x| Value::UInt(u64::from(*x))).collect())
    }

    fn floats_v(f: &[f32]) -> Value {
        Value::Array(f.iter().map(|x| Value::Float(*x)).collect())
    }

    /// A clip as the type tree reads it: one streamed position curve set (three curves), one dense
    /// quaternion's worth and one constant scale.
    fn clip_value() -> Value {
        let b = |attr: i64, path: i64| {
            obj(&[
                ("path", Value::Int(path)),
                ("attribute", Value::Int(attr)),
                ("typeID", Value::Int(4)),
                ("isPPtrCurve", Value::Int(0)),
            ])
        };
        // Streamed: three curves, each one key at -f32::MAX holding 1, 2, 3, then a key at 0 whose
        // segment rises by one unit by t = 1 (c1 = 1) and a closing key at 1.
        let mut w = vec![(-f32::MAX).to_bits(), 3];
        for c in 0..3u32 {
            w.extend([c, 0, 0, 0, (c as f32 + 1.0).to_bits()]);
        }
        w.extend([0f32.to_bits(), 3]);
        for c in 0..3u32 {
            // v(t) = (c + 1) + t (c0 = c2 = 0 would make it constant; c2 = 1 is the slope)
            w.extend([c, 0, 0, 1.0f32.to_bits(), (c as f32 + 1.0).to_bits()]);
        }
        w.extend([1f32.to_bits(), 3]);
        for c in 0..3u32 {
            w.extend([c, 0, 0, 0, (c as f32 + 2.0).to_bits()]);
        }
        w.extend([f32::INFINITY.to_bits(), 0]);
        obj(&[
            ("m_Name", Value::Str("walk".into())),
            ("m_Legacy", Value::Bool(false)),
            ("m_Compressed", Value::Bool(false)),
            ("m_CompressedRotationCurves", Value::Array(vec![])),
            (
                "m_ClipBindingConstant",
                obj(&[("genericBindings", Value::Array(vec![b(1, 100), b(2, 100), b(3, 200)]))]),
            ),
            (
                "m_MuscleClip",
                obj(&[
                    ("m_StartTime", Value::Float(0.0)),
                    ("m_StopTime", Value::Float(1.0)),
                    ("m_LoopTime", Value::Bool(true)),
                    (
                        "m_Clip",
                        obj(&[(
                            "data",
                            obj(&[
                                (
                                    "m_StreamedClip",
                                    obj(&[("curveCount", Value::Int(3)), ("data", uints(&w))]),
                                ),
                                (
                                    "m_DenseClip",
                                    obj(&[
                                        ("m_BeginTime", Value::Float(0.0)),
                                        ("m_SampleRate", Value::Float(2.0)),
                                        ("m_FrameCount", Value::Int(2)),
                                        ("m_CurveCount", Value::Int(4)),
                                        (
                                            "m_SampleArray",
                                            floats_v(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0]),
                                        ),
                                    ]),
                                ),
                                ("m_ConstantClip", obj(&[("data", floats_v(&[1.0, 1.0, 1.0]))])),
                            ]),
                        )]),
                    ),
                ]),
            ),
        ])
    }

    #[test]
    fn a_clip_decodes_and_samples() {
        let c = decode_clip(&clip_value()).unwrap();
        assert!(c.looping);
        assert_eq!(c.bindings.len(), 3);
        assert_eq!(c.curve_count(), 3 + 4 + 3);
        let mut out = Vec::new();
        c.sample(0.5, &mut out);
        // Streamed curves rise from 1, 2, 3 by the slope; dense curves interpolate (at 2 Hz, half a
        // second is the second frame); constants hold.
        assert_eq!(&out[..3], &[1.5, 2.5, 3.5]);
        assert_eq!(&out[3..7], &[0.0, 0.0, 1.0, 0.0]);
        c.sample(0.25, &mut out);
        assert_eq!(&out[3..7], &[0.0, 0.0, 0.5, 0.5]);
        c.sample(0.5, &mut out);
        assert_eq!(&out[7..], &[1.0, 1.0, 1.0]);
        // The segment ends at the next key's value: the checker's continuity reads zero.
        assert!(c.continuity_error() < 1e-6, "{}", c.continuity_error());
    }

    #[test]
    fn legacy_and_inconsistent_clips_are_refused() {
        let mut v = clip_value();
        if let Value::Object(f) = &mut v {
            f.iter_mut().find(|(k, _)| k == "m_Legacy").unwrap().1 = Value::Bool(true);
        }
        assert!(decode_clip(&v).is_err());
        // One binding fewer than the curves need.
        let mut v = clip_value();
        if let Value::Object(f) = &mut v
            && let Some((_, Value::Object(bc))) =
                f.iter_mut().find(|(k, _)| k == "m_ClipBindingConstant")
            && let Some((_, Value::Array(a))) = bc.iter_mut().find(|(k, _)| k == "genericBindings")
        {
            a.pop();
        }
        assert!(decode_clip(&v).is_err());
    }

    #[test]
    fn the_checker_counts_what_it_sees() {
        let mut r = CheckReport::default();
        r.add(&clip_value());
        r.add(&Value::Object(vec![]));
        assert_eq!((r.clips, r.decoded, r.failures.len()), (2, 1, 1));
        assert_eq!(r.looping, 1);
        assert!(r.worst_continuity.0 < 1e-6);
        assert_eq!(r.non_finite, 0);
    }
}
