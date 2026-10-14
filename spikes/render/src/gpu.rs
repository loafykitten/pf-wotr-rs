//! Headless wgpu device on the Metal backend, for experiments that don't need Bevy.

use bevy::tasks::block_on;

/// Create a Metal adapter, device and queue. `wanted` features are requested only where the
/// adapter supports them; callers check `device.features()` for what they got.
pub fn headless(
    wanted: wgpu::Features,
) -> Result<(wgpu::Adapter, wgpu::Device, wgpu::Queue), String> {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::METAL;
    let instance = wgpu::Instance::new(desc);
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    }))
    .map_err(|e| format!("no Metal adapter: {e}"))?;
    let features = adapter.features() & wanted;
    let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("render-spike"),
        required_features: features,
        required_limits: adapter.limits(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .map_err(|e| format!("request_device: {e}"))?;
    Ok((adapter, device, queue))
}

/// Block until all submitted work is done.
pub fn wait_idle(device: &wgpu::Device) {
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
}
