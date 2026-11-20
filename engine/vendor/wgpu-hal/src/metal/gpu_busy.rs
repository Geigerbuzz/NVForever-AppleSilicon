//! NVForever measuring aid: GPU busy time per frame from Metal's own command-buffer clocks.
//!
//! On by `NVF_METAL_GPU_TIME=<seconds>` (the logging interval). Every committed command buffer
//! gets a completion handler that reads `GPUStartTime`/`GPUEndTime`; the intervals are merged
//! (work from neighbouring buffers overlaps on Apple GPUs, and counts once) into busy seconds,
//! and each interval logs the busy share of the wall clock: near 100% the GPU is the limit,
//! and busy share x frame time is the GPU's time a frame (unsnapped to the display's steps).
//!
//! Why: on this Mac, Bevy's per-pass GPU timers need timestamp queries inside encoders, which
//! wgpu's Metal backend does not offer, and frame times snap to the display's 8.3 ms steps
//! (a 26 ms and a 33 ms GPU frame both show as 33.3 ms), so savings inside a step were
//! invisible. Off (no handler, no lock) unless the variable is set.

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use objc2::runtime::ProtocolObject;
use objc2_metal::MTLCommandBuffer;

struct Window {
    started: Instant,
    last_end: f64,
    busy: f64,
    frames: u64,
}

static INTERVAL: OnceLock<Option<f64>> = OnceLock::new();
static STATE: Mutex<Option<Window>> = Mutex::new(None);

fn interval() -> Option<f64> {
    *INTERVAL.get_or_init(|| {
        std::env::var("NVF_METAL_GPU_TIME").ok().and_then(|v| v.parse::<f64>().ok()).filter(|s| *s > 0.0)
    })
}

pub(super) fn enabled() -> bool {
    interval().is_some()
}

/// Adds the completion handler that records this buffer's GPU interval.
pub(super) fn watch(raw: &ProtocolObject<dyn MTLCommandBuffer>) {
    let block = block2::RcBlock::new(|cb: core::ptr::NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
        let cb = unsafe { cb.as_ref() };
        // Sent directly: objc2-metal gates these behind a feature wgpu does not enable.
        let (start, end): (f64, f64) = unsafe { (objc2::msg_send![cb, GPUStartTime], objc2::msg_send![cb, GPUEndTime]) };
        if end > start && start > 0.0 {
            record(start, end);
        }
    });
    unsafe { raw.addCompletedHandler(block2::RcBlock::as_ptr(&block)) };
}

/// A submit that presents (the game makes two such submits a displayed frame, so this is a
/// count of submits, not of frames: GPU time a frame = busy share x frame time).
pub(super) fn frame() {
    let mut state = STATE.lock().unwrap();
    window(&mut state).frames += 1;
}

fn window(state: &mut Option<Window>) -> &mut Window {
    state.get_or_insert_with(|| Window { started: Instant::now(), last_end: 0.0, busy: 0.0, frames: 0 })
}

fn record(start: f64, end: f64) {
    let Some(every) = interval() else { return };
    let mut state = STATE.lock().unwrap();
    let w = window(&mut state);
    // Union of busy intervals: count only the part after the latest end seen so far.
    let from = start.max(w.last_end);
    if end > from {
        w.busy += end - from;
    }
    w.last_end = w.last_end.max(end);
    let wall = w.started.elapsed().as_secs_f64();
    if wall >= every && w.frames > 0 {
        // stderr, not `log`: Bevy's default filter keeps only wgpu's errors.
        eprintln!(
            "metal gpu busy: {:.1}% of the wall clock, {:.0} ms a second ({} presenting submits in {:.1} s)",
            100.0 * w.busy / wall,
            1000.0 * w.busy / wall,
            w.frames,
            wall
        );
        let last_end = w.last_end;
        *w = Window { started: Instant::now(), last_end, busy: 0.0, frames: 0 };
    }
}
