//! Latency reduction on Apple platforms: the switch, but no driver to pace with.
//!
//! Both techniques NVForever offers are Vulkan extensions: AMD's `VK_AMD_anti_lag` and
//! NVIDIA's `VK_NV_low_latency2` ([`super::anti_lag`], built on Windows and Linux). Metal
//! has no Vulkan driver, so on Apple the settings row is dimmed by [`available`] returning
//! false and the switch drives nothing. The resource and the plugin stay here under the
//! same names, because `settings.rs` and `frame_gen.rs` read them whatever the platform.

use bevy::prelude::*;

/// The latency reduction switch (the settings row, `--latency-reduction`).
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct LatencyReduction {
    pub on: bool,
    /// Frame generation turns it on while it runs (`frame_gen.rs`), whatever the switch says.
    pub by_frame_generation: bool,
}

impl LatencyReduction {
    /// Whether the driver is asked to pace frames.
    pub fn active(&self) -> bool {
        self.on || self.by_frame_generation
    }
}

/// The render world's copy of the frame being drawn. Nothing writes it on Metal; it exists
/// so the systems that take `Option<Res<Presenting>>` compile and see nothing.
#[derive(Resource, Default, Clone, Copy)]
pub(crate) struct Presenting {
    pub(crate) frame: u64,
    pub(crate) on: bool,
}

/// The technique latency reduction would run on, on this GPU: never one on Metal.
pub fn technique(_world: &World) -> Option<&'static str> {
    None
}

/// Whether this GPU's driver takes latency reduction (for the settings row).
pub fn available(_world: &World) -> bool {
    false
}

/// Nothing to add to the device: the extension is Vulkan's.
pub struct AntiLagInitPlugin;

impl Plugin for AntiLagInitPlugin {
    fn build(&self, _app: &mut App) {}
}

/// The switch, and no calls to a driver.
pub struct AntiLagPlugin(pub bool);

impl Plugin for AntiLagPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(LatencyReduction { on: self.0, by_frame_generation: false });
    }
}
