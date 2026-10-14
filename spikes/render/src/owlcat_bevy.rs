//! `scene --owlcat`: the Owlcat passthrough draw hosted by Bevy itself: built on Bevy's
//! `RenderDevice` (wgpu 30) and recorded every frame by a system in Bevy's root render-graph
//! schedule, alongside Bevy's own passes. After 60 frames the G-buffer is read back once and
//! checked like the headless run.

use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use bevy::render::RenderApp;
use bevy::render::renderer::{
    PendingCommandBuffers, RenderDevice, RenderGraph, RenderGraphSystems, RenderQueue,
};

use crate::owlcat::{OwlcatDraw, default_msl_dir};

/// Report shared with the main world (written into summary.txt).
#[derive(Resource, Clone, Default, Debug)]
pub struct OwlcatReport(pub Arc<Mutex<String>>);

#[derive(Resource)]
struct Draw {
    draw: OwlcatDraw,
    frames: u64,
    checked: bool,
}

#[derive(Debug)]
pub struct OwlcatInBevyPlugin {
    pub report: OwlcatReport,
}

impl Plugin for OwlcatInBevyPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(self.report.clone());
    }

    fn finish(&self, app: &mut App) {
        let report = self.report.clone();
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        let device = render_app.world().resource::<RenderDevice>().clone();
        let queue = render_app.world().resource::<RenderQueue>().clone();
        let dir = default_msl_dir();
        let srcs = std::fs::read_to_string(dir.join("blob153.metal"))
            .and_then(|v| Ok((v, std::fs::read_to_string(dir.join("blob869.metal"))?)));
        let msg = match srcs
            .map_err(|e| e.to_string())
            .and_then(|(vs, fs)| OwlcatDraw::new(device.wgpu_device(), &queue, &vs, &fs))
        {
            Ok(draw) => {
                let notes = format!("owlcat in Bevy: built on Bevy's RenderDevice\n{}", draw.notes);
                render_app
                    .insert_resource(Draw { draw, frames: 0, checked: false })
                    .insert_resource(report.clone())
                    .add_systems(
                        RenderGraph,
                        (
                            record.in_set(RenderGraphSystems::Begin),
                            check.in_set(RenderGraphSystems::Finish),
                        ),
                    );
                notes
            }
            Err(e) => format!("owlcat in Bevy: not built: {e}\n"),
        };
        if let Ok(mut r) = report.0.lock() {
            r.push_str(&msg);
        }
    }
}

fn record(
    mut d: ResMut<Draw>,
    device: Res<RenderDevice>,
    mut pending: ResMut<PendingCommandBuffers>,
) {
    let mut enc = device
        .wgpu_device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("owlcat-in-bevy") });
    d.draw.encode(&mut enc, None);
    pending.push([enc.finish()]);
    d.frames += 1;
}

fn check(
    mut d: ResMut<Draw>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    report: Res<OwlcatReport>,
) {
    if d.checked || d.frames < 60 {
        return;
    }
    d.checked = true;
    let text = d
        .draw
        .read_targets(device.wgpu_device(), &queue)
        .and_then(|imgs| d.draw.check(&imgs))
        .unwrap_or_else(|e| format!("readback failed: {e}\n"));
    if let Ok(mut r) = report.0.lock() {
        r.push_str(&format!("after {} Bevy frames:\n{text}", d.frames));
    }
}
