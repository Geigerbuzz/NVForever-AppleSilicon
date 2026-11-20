//! Measuring aid (2026-11-20): where the script tick's time goes. `NVF_SCRIPT_TIMING=1`
//! turns it on (read once); off, every call is one relaxed atomic load. [`time`] wraps a
//! phase of [`crate::game::Game::tick`]; [`script`] adds one script run (by script FormID and
//! by what ran it). [`take`] hands the totals over and clears them; the engine logs them.

use fnv_sim::FormId;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::Instant;

/// 0 unread, 1 off, 2 on.
static STATE: AtomicU8 = AtomicU8::new(0);
static TOTALS: Mutex<Option<Totals>> = Mutex::new(None);

/// Seconds and runs since the last [`take`].
#[derive(Default, Debug, Clone)]
pub struct Totals {
    /// Per phase of the tick: seconds.
    pub phases: HashMap<&'static str, f64>,
    /// Per script: seconds and runs.
    pub scripts: HashMap<FormId, (f64, u32)>,
    /// Per kind of runner (quest, reference, effect, item, other): seconds and runs.
    pub kinds: HashMap<&'static str, (f64, u32)>,
}

pub fn enabled() -> bool {
    match STATE.load(Ordering::Relaxed) {
        2 => true,
        1 => false,
        _ => {
            let on = std::env::var("NVF_SCRIPT_TIMING").is_ok_and(|v| v == "1");
            STATE.store(if on { 2 } else { 1 }, Ordering::Relaxed);
            on
        }
    }
}

/// Runs `f`, adding its time to `phase` when timing is on.
pub fn time<R>(phase: &'static str, f: impl FnOnce() -> R) -> R {
    if !enabled() {
        return f();
    }
    let started = Instant::now();
    let r = f();
    let secs = started.elapsed().as_secs_f64();
    let mut t = TOTALS.lock().unwrap();
    *t.get_or_insert_with(Totals::default).phases.entry(phase).or_insert(0.0) += secs;
    r
}

/// Adds one run of `script` by a runner of `kind` that took `secs`.
pub fn script(script: FormId, kind: &'static str, secs: f64) {
    let mut t = TOTALS.lock().unwrap();
    let t = t.get_or_insert_with(Totals::default);
    let e = t.scripts.entry(script).or_insert((0.0, 0));
    e.0 += secs;
    e.1 += 1;
    let k = t.kinds.entry(kind).or_insert((0.0, 0));
    k.0 += secs;
    k.1 += 1;
}

/// The totals since the last call, cleared.
pub fn take() -> Totals {
    TOTALS.lock().unwrap().take().unwrap_or_default()
}
