//! The camera route exported by WrathOracle (`route-passN.json`, schema
//! `wrath-camera-replay-v1`): per point, the render camera exactly as the original drew
//! it (world-to-camera, projection and GPU projection, row-major).

use std::path::Path;

use anyhow::{Context, Result};
use bevy::math::Vec3;
use serde_json::Value as J;

use crate::owlcat::CameraPose;
use crate::owlcat::consts::mat_rows;

#[derive(Debug, Clone)]
pub struct RoutePoint {
    pub sample: usize,
    pub pose: CameraPose,
}

fn f(v: &J) -> Option<f32> {
    v.as_f64().map(|x| x as f32)
}

fn rows(v: &J) -> Option<[[f32; 4]; 4]> {
    let a = v.as_array()?;
    let mut out = [[0.0; 4]; 4];
    for (r, row) in a.iter().enumerate().take(4) {
        for (c, x) in row.as_array()?.iter().enumerate().take(4) {
            out[r][c] = f(x)?;
        }
    }
    Some(out)
}

pub fn load(path: &Path) -> Result<Vec<RoutePoint>> {
    let j: J = serde_json::from_slice(
        &std::fs::read(path).with_context(|| format!("read {}", path.display()))?,
    )?;
    let mut out = Vec::new();
    for p in j["points"].as_array().context("no points")? {
        let c = &p["camera"];
        let pos = c["position"].as_array().context("camera.position")?;
        let pose = CameraPose {
            world_to_camera: mat_rows(rows(&c["world_to_camera"]).context("world_to_camera")?),
            gpu_projection: mat_rows(
                rows(&c["gpu_projection_render_into_texture"]).context("gpu projection")?,
            ),
            projection: mat_rows(rows(&c["projection"]).context("projection")?),
            position: Vec3::new(
                f(&pos[0]).unwrap_or(0.0),
                f(&pos[1]).unwrap_or(0.0),
                f(&pos[2]).unwrap_or(0.0),
            ),
            near: f(&c["near"]).unwrap_or(1.0),
            far: f(&c["far"]).unwrap_or(1000.0),
            width: c["pixel_width"].as_u64().unwrap_or(2560) as u32,
            height: c["pixel_height"].as_u64().unwrap_or(1440) as u32,
            rig: {
                let rp = p["rig"]["position"].as_array();
                let rr = p["rig"]["rotation"].as_array();
                match (rp, rr) {
                    (Some(a), Some(q)) if a.len() == 3 && q.len() == 4 => Some((
                        Vec3::new(
                            f(&a[0]).unwrap_or(0.0),
                            f(&a[1]).unwrap_or(0.0),
                            f(&a[2]).unwrap_or(0.0),
                        ),
                        bevy::math::Quat::from_xyzw(
                            f(&q[0]).unwrap_or(0.0),
                            f(&q[1]).unwrap_or(0.0),
                            f(&q[2]).unwrap_or(0.0),
                            f(&q[3]).unwrap_or(1.0),
                        ),
                    )),
                    _ => None,
                }
            },
        };
        out.push(RoutePoint {
            sample: p["sample"].as_u64().unwrap_or(out.len() as u64) as usize,
            pose,
        });
    }
    Ok(out)
}
