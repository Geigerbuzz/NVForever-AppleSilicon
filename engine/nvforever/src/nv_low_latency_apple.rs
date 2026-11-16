//! NVIDIA's low latency mode on Apple platforms: never loaded.
//!
//! The real module drives `VK_NV_low_latency2` (or Streamline's Reflex) on the Vulkan device
//! ([`super::nv_low_latency`], built on Windows and Linux). Metal has neither, so [`load`]
//! always says why not and nothing presents out of band. The names stay the same, because
//! `frame_gen.rs`, `fg_presenter.rs` and `anti_lag.rs` refer to them whatever the platform.

use bevy::prelude::*;

use super::anti_lag::Presenting;

/// The driver for the render device: never one here.
#[derive(Resource, Clone)]
pub(crate) struct NvLowLatency;

impl NvLowLatency {
    /// Whether the calls go through Streamline's Reflex; never on Metal.
    pub(crate) fn through_streamline(&self) -> bool {
        false
    }
}

/// Nothing to add: the extension is Vulkan's.
pub(crate) fn add_systems(_app: &mut App) {}

/// Why there is no driver, for the start's log line.
pub(crate) fn load(_app: &App) -> Result<NvLowLatency, &'static str> {
    Err("NVIDIA low latency is a Vulkan extension (VK_NV_low_latency2) and this device is Metal")
}

/// Nothing to mark at the end of a present.
pub(crate) fn present_end(_presenting: Res<Presenting>, _driver: Option<Res<NvLowLatency>>) {}

/// One rendered frame's presents by frame generation: never wrapped in markers here.
#[derive(Clone)]
pub(crate) struct OutOfBandPresents;

impl OutOfBandPresents {
    pub(crate) fn of(_presenting: &Presenting, _driver: Option<&NvLowLatency>) -> Option<Self> {
        None
    }

    /// Runs `present` (one present call), with nothing around it.
    pub(crate) fn around(presents: Option<&Self>, present: impl FnOnce()) {
        let _ = presents;
        present();
    }
}
