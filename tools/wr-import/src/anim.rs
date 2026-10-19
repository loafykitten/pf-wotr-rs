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
