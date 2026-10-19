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

/// A point looking from `eye` at `target` with `base`'s lens, as Unity's camera would be
/// placed there: `worldToCamera` has the camera's right, up and negated forward axes as rows
/// (`Quaternion.LookRotation(forward, up)`, then the `(1, 1, -1)` scale of camera space).
/// For viewing units from where no recorded pose has them; there is no original to compare.
#[must_use]
pub fn look_at(base: &RoutePoint, eye: Vec3, target: Vec3) -> RoutePoint {
    let f = (target - eye).normalize();
    // Straight up or down has no right vector: take +Z as the reference.
    let up = if f.cross(Vec3::Y).length() < 1e-4 { Vec3::Z } else { Vec3::Y };
    let r = up.cross(f).normalize();
    let u = f.cross(r);
    let w2c = bevy::math::Mat4::from_cols(
        bevy::math::Vec4::new(r.x, u.x, -f.x, 0.0),
        bevy::math::Vec4::new(r.y, u.y, -f.y, 0.0),
        bevy::math::Vec4::new(r.z, u.z, -f.z, 0.0),
        bevy::math::Vec4::new(-r.dot(eye), -u.dot(eye), f.dot(eye), 1.0),
    );
    let mut pose = base.pose;
    pose.world_to_camera = w2c;
    pose.position = eye;
    pose.rig = None;
    RoutePoint { sample: base.sample, pose }
}

/// Parses `ex,ey,ez,tx,ty,tz`.
#[must_use]
pub fn parse_look(s: &str) -> Option<(Vec3, Vec3)> {
    let v: Vec<f32> = s
        .split(',')
        .map(|x| x.trim().parse().ok().filter(|f: &f32| f.is_finite()))
        .collect::<Option<_>>()?;
    let (eye, target) = (
        Vec3::new(*v.first()?, *v.get(1)?, *v.get(2)?),
        Vec3::new(*v.get(3)?, *v.get(4)?, *v.get(5)?),
    );
    (v.len() == 6 && (target - eye).length() > 1e-3).then_some((eye, target))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn look_at_puts_the_target_straight_ahead() {
        let base = RoutePoint {
            sample: 0,
            pose: CameraPose {
                world_to_camera: bevy::math::Mat4::IDENTITY,
                gpu_projection: bevy::math::Mat4::IDENTITY,
                projection: bevy::math::Mat4::IDENTITY,
                position: Vec3::ZERO,
                near: 0.3,
                far: 100.0,
                width: 16,
                height: 9,
                rig: None,
            },
        };
        let p = look_at(&base, Vec3::new(1.0, 2.0, 3.0), Vec3::new(1.0, 2.0, 13.0));
        // The target is 10 in front of the camera: camera space looks down -Z.
        let c = p.pose.world_to_camera.transform_point3(Vec3::new(1.0, 2.0, 13.0));
        assert!((c - Vec3::new(0.0, 0.0, -10.0)).length() < 1e-5, "{c:?}");
        // World +X is camera right for a camera looking along +Z (Unity's left-handed world).
        let x = p.pose.world_to_camera.transform_vector3(Vec3::X);
        assert!(x.x > 0.99, "{x:?}");
        assert_eq!(
            parse_look("1,2,3,4,5,6"),
            Some((Vec3::new(1.0, 2.0, 3.0), Vec3::new(4.0, 5.0, 6.0)))
        );
        assert_eq!(parse_look("1,2"), None);
        assert_eq!(parse_look("1,2,3,4,5,x"), None);
        assert_eq!(parse_look("1,2,3,1,2,3"), None);
        assert_eq!(parse_look("1,2,3,4,5,6,7"), None);
        // Straight down still gives a camera.
        let down = look_at(&base, Vec3::new(0.0, 10.0, 0.0), Vec3::ZERO);
        assert!(down.pose.world_to_camera.is_finite());
    }
}
