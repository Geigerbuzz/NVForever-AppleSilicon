//! GPU occlusion culling for the gameplay camera (`--occlusion-culling`), report 26 §3(b).
//!
//! Bevy 0.20 ships two-phase Hi-Z occlusion culling: with `OcclusionCulling` on a view
//! that has a `DepthPrepass` (the gameplay camera has one for water), the prepass splits
//! into an early pass tested against last frame's depth pyramid and a late pass tested
//! against this frame's, so a newly visible mesh is still drawn in the same frame. The
//! `Mac` advertises GPU preprocessing in `Culling` mode ("GPU preprocessing is fully
//! supported on this device"), so the path is available; the component is simply absent
//! today.
//!
//! The flag is off by default for the A/B (report 26 §6, stage 1): it costs an estimated
//! 0.4–0.9 ms of depth-pyramid and early/late pass work and removes hidden geometry from
//! the prepass and the main pass. There is no visual change by construction.

use bevy::prelude::*;
use bevy::render::batching::gpu_preprocessing::{GpuPreprocessingMode, GpuPreprocessingSupport};
use bevy::render::occlusion_culling::OcclusionCulling;
use bevy::render::{init_gpu_resource, RenderApp, RenderStartup};

/// Adds `OcclusionCulling` to the view when the run asked for it.
pub fn add_to_camera(commands: &mut Commands, camera: Entity, on: bool) {
    if on {
        commands.entity(camera).insert(OcclusionCulling);
    }
}

/// Logs, once the render device is up, whether the culling path can run at all: the
/// prepass split needs `GpuPreprocessingMode::Culling`, and without it the component is
/// ignored (a safe no-op, not a crash).
fn log_support(support: Option<Res<GpuPreprocessingSupport>>) {
    let culling = support.is_some_and(|s| s.max_supported_mode == GpuPreprocessingMode::Culling);
    info!(
        "occlusion culling: on, {}",
        if culling {
            "GPU preprocessing is fully supported (Culling): two-phase Hi-Z runs on the gameplay camera"
        } else {
            "but GPU preprocessing does not support culling on this device; the component is ignored"
        }
    );
}

/// The flag's side: the support log at render startup. The component itself is inserted
/// with the camera (`setup` in main.rs), so a test can see it without a renderer.
pub struct OcclusionCullPlugin {
    pub on: bool,
}

impl Plugin for OcclusionCullPlugin {
    fn build(&self, app: &mut App) {
        if !self.on {
            return;
        }
        let Some(render) = app.get_sub_app_mut(RenderApp) else { return };
        // `BatchingPlugin::finish` adds the support resource to `RenderStartup`; read it
        // after that system so the first line says what the device can do.
        render.add_systems(RenderStartup, log_support.after(init_gpu_resource::<GpuPreprocessingSupport>));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The camera gets the component with the flag and not without it (report 26 §6,
    /// stage 1): the smoke test the A/B starts from.
    #[test]
    fn the_component_follows_the_flag() {
        fn present(on: bool) -> bool {
            let mut app = App::new();
            app.add_systems(Startup, move |mut commands: Commands| {
                let camera = commands.spawn_empty().id();
                add_to_camera(&mut commands, camera, on);
            });
            app.update();
            let mut query = app.world_mut().query_filtered::<Entity, With<OcclusionCulling>>();
            query.iter(app.world()).count() > 0
        }
        assert!(present(true), "--occlusion-culling inserts the component");
        assert!(!present(false), "the default leaves it off for the A/B");
    }
}
