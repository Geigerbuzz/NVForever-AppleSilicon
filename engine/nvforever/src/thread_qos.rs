//! macOS thread priority for the game's own threads (2026-11-20).
//!
//! macOS picks a core for a thread by its quality-of-service class. A thread nobody gave a
//! class runs at the default one, which the scheduler may put on the M2 Pro's efficiency
//! cores; a game frame's parallel systems then wait for the slowest worker. The bare
//! executable is not an app bundle with the games category either, so Game Mode never
//! raises it. Here the main thread, the render thread and Bevy's compute and async-compute
//! workers take `QOS_CLASS_USER_INTERACTIVE`, the class macOS gives the thread a user is
//! waiting on (performance cores first). The IO pool (asset loading) keeps the default.
//!
//! `NVF_QOS=0` leaves every thread at the default class, for measuring against.

use bevy::prelude::*;
use bevy::render::{Render, RenderApp};
use std::cell::Cell;

/// `QOS_CLASS_USER_INTERACTIVE` [documented: <sys/qos.h>, 0x21].
#[cfg(target_os = "macos")]
const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
}

/// Whether the threads are raised (`NVF_QOS` is not `0`).
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("NVF_QOS").map_or(true, |v| v != "0"))
}

thread_local! {
    static RAISED: Cell<bool> = const { Cell::new(false) };
}

/// Give the calling thread the user-interactive class (once per thread; nothing off macOS
/// or with `NVF_QOS=0`).
pub fn raise_this_thread() {
    if !enabled() || RAISED.with(Cell::get) {
        return;
    }
    RAISED.with(|r| r.set(true));
    #[cfg(target_os = "macos")]
    unsafe {
        pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0);
    }
}

/// Raises the render thread (the first time the render schedule runs on it) and logs once.
pub struct ThreadQosPlugin;

impl Plugin for ThreadQosPlugin {
    fn build(&self, app: &mut App) {
        raise_this_thread();
        if enabled() {
            info!("threads: main, render and compute workers at the user-interactive QoS class (NVF_QOS=0 leaves the default)");
        }
        if let Some(render) = app.get_sub_app_mut(RenderApp) {
            render.add_systems(Render, raise_render_thread);
        }
    }
}

fn raise_render_thread() {
    raise_this_thread();
}
