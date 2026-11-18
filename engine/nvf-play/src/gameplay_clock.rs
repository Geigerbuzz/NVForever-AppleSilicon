//! The gameplay clock: a fixed 60 Hz step for the game's *rules* while the renderer
//! runs at its own rate (report 29, stage (a)).
//!
//! Only gameplay logic is gated: `script::run_scripts` skips `script_trigger::update`
//! and `Game::tick` on a frame with no tick due, and `nvforever::residents::drive` skips
//! `ai::think`. Physics (already a fixed step with interpolation), animation, the NIF
//! mirror, streaming, input and the camera stay at the render rate.
//!
//! `NVF_GAMEPLAY_HZ` chooses the rate: `120` (the default) keeps today's behaviour —
//! one gameplay tick per rendered frame with the frame's own `dt`; `60` enables the
//! clock. The clock is a real-time accumulator: a 50 ms frame clamp, at most two ticks
//! per frame, one step of debt carried, and everything beyond dropped (a busy moment
//! slows game time rather than spiralling). Menus and loading screens bypass it: one
//! tick per rendered frame with the frame's `dt`, as before.

use bevy::prelude::*;

/// The longest real frame the gameplay clock counts, in seconds (report 29 §4.1): a
/// hitch longer than this advances game time by 50 ms, not by the whole stall.
pub const MAX_FRAME_SECONDS: f64 = 0.050;

/// The most gameplay ticks one rendered frame runs (report 29 §4.1).
pub const MAX_TICKS_PER_FRAME: u32 = 2;

/// The rate the clock ticks at when enabled, in Hz.
pub const GAMEPLAY_HZ: f64 = 60.0;

/// How a rendered frame counts for the gameplay clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// The played world: accumulate real time and run the ticks that are due.
    Gameplay,
    /// A menu or a loading screen: one tick this frame with the frame's render time,
    /// exactly as before, and the accumulator starts over (report 29 §4.1).
    Bypass,
    /// The pause menu: no tick, and no catch-up when it closes.
    Paused,
}

/// A second's worth of clock numbers, for the log line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClockReport {
    pub hz: f64,
    pub frames: u64,
    pub ticks: u64,
    pub dropped: f64,
    pub alpha: f64,
}

impl ClockReport {
    /// Ticks per rendered gameplay frame.
    pub fn ticks_per_frame(&self) -> f64 {
        if self.frames == 0 { 0.0 } else { self.ticks as f64 / self.frames as f64 }
    }
}

/// The gameplay clock (report 29 §4.1).
///
/// `step` is one fixed tick's game time (1/60 s); `acc` is real time towards the next
/// tick; `game_dt` is this frame's `GameConfig::frame_seconds` (one tick's worth on a
/// tick frame, the render time in menus/loading, and the render time at 120 Hz).
#[derive(Resource, Debug)]
pub struct GameplayClock {
    enabled: bool,
    hz: u32,
    step: f64,
    acc: f64,
    alpha: f64,
    dropped: f64,
    ticks: u64,
    frame_ticks: u32,
    game_dt: f64,
    window_secs: f64,
    window_frames: u64,
    window_ticks: u64,
    window_dropped: f64,
}

impl Default for GameplayClock {
    fn default() -> Self {
        Self::from_env()
    }
}

impl GameplayClock {
    /// The clock at `hz` Hz: `120` is today's one tick per rendered frame; `60` is the
    /// fixed step. Any other rate is treated as `120`.
    pub fn new(hz: u32) -> Self {
        let hz = if hz == 60 { 60 } else { 120 };
        GameplayClock {
            enabled: hz == 60,
            hz,
            step: 1.0 / f64::from(hz),
            acc: 0.0,
            alpha: 0.0,
            dropped: 0.0,
            ticks: 0,
            frame_ticks: 0,
            game_dt: 0.0,
            window_secs: 0.0,
            window_frames: 0,
            window_ticks: 0,
            window_dropped: 0.0,
        }
    }

    /// `NVF_GAMEPLAY_HZ` (`60` enables, `120` or unset keeps today's behaviour).
    pub fn from_env() -> Self {
        let hz = std::env::var("NVF_GAMEPLAY_HZ").ok().and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(120);
        Self::new(hz)
    }

    /// Whether the fixed step is on (`NVF_GAMEPLAY_HZ=60`).
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The rate: 60 while enabled, 120 (one tick a frame) otherwise.
    pub fn hz(&self) -> u32 {
        self.hz
    }

    /// One fixed tick of game time in seconds (1/60), whether or not it is enabled.
    pub fn step(&self) -> f64 {
        self.step
    }

    /// Game seconds the ticks run in one rendered frame, and what `frame_seconds` is
    /// set to: one step on a tick frame, the render time in menus/loading and at 120.
    pub fn game_dt(&self) -> f64 {
        self.game_dt
    }

    /// Ticks due this frame: 0, 1 or 2 (1 in menus/loading and at 120; 0 under pause).
    pub fn ticks_this_frame(&self) -> u32 {
        self.frame_ticks
    }

    /// Whether a gameplay tick runs this frame.
    pub fn is_due(&self) -> bool {
        self.frame_ticks > 0
    }

    /// The accumulator's fraction of the next step, for a later interpolation bridge.
    pub fn alpha(&self) -> f64 {
        self.alpha
    }

    /// Real seconds thrown away since the start: the 50 ms frame clamp, and whatever
    /// lay past the two-tick cap and the one step of debt.
    pub fn dropped(&self) -> f64 {
        self.dropped
    }

    /// Ticks run since the start.
    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    /// One rendered frame.
    ///
    /// `real_dt` is the wall-clock frame time (the accumulator's input), `render_dt` the
    /// virtual frame time (`Time::delta_secs_f64`, menus and 120 use it as the game dt),
    /// and `speed` the virtual clock's relative speed, so a slow-motion tick advances
    /// `step × speed` of game time while still ticking 60 times a second of real time.
    pub fn advance(&mut self, real_dt: f64, render_dt: f64, speed: f64, gate: Gate) -> Option<ClockReport> {
        let real_dt = real_dt.max(0.0);
        let render_dt = render_dt.max(0.0);
        self.frame_ticks = 0;
        if !self.enabled || gate != Gate::Gameplay {
            // Today's behaviour, and the menu/loading bypass: one tick with the frame's
            // own time. `Paused` runs none, and drops what was accumulated so the pause
            // menu does not come back with a burst. No tick was skipped, so there is
            // nothing to interpolate between (alpha 1: the newest state is current).
            self.acc = 0.0;
            self.alpha = 1.0;
            self.game_dt = render_dt;
            if gate != Gate::Paused {
                self.frame_ticks = 1;
                self.ticks += 1;
            }
            return None;
        }
        let dt = real_dt.min(MAX_FRAME_SECONDS);
        let mut dropped = real_dt - dt;
        self.acc += dt;
        let due = ((self.acc / self.step) + 1e-9).floor().max(0.0) as u32;
        let n = due.min(MAX_TICKS_PER_FRAME);
        self.acc = (self.acc - f64::from(n) * self.step).max(0.0);
        // One step of debt at most: a 250 ms frame owes three ticks, runs two, carries
        // the third, and drops anything past that.
        if n == MAX_TICKS_PER_FRAME && self.acc > self.step + 1e-9 {
            dropped += self.acc - self.step;
            self.acc = self.step;
        }
        self.dropped += dropped;
        self.alpha = (self.acc / self.step).clamp(0.0, 1.0);
        self.frame_ticks = n;
        self.ticks += u64::from(n);
        self.game_dt = self.step * speed.max(0.0);
        self.window_secs += real_dt;
        self.window_frames += 1;
        self.window_ticks += u64::from(n);
        self.window_dropped += dropped;
        if self.window_secs >= 1.0 {
            let report = ClockReport {
                hz: f64::from(self.hz),
                frames: std::mem::take(&mut self.window_frames),
                ticks: std::mem::take(&mut self.window_ticks),
                dropped: std::mem::take(&mut self.window_dropped),
                alpha: self.alpha,
            };
            self.window_secs = 0.0;
            return Some(report);
        }
        None
    }
}

/// The clock's system: advance once per rendered frame, after Bevy's time systems and
/// before [`PlaySet::Input`](crate::PlaySet).
fn advance_clock(
    real: Res<Time<Real>>,
    virtual_time: Res<Time<Virtual>>,
    state: Option<Res<State<crate::PlayState>>>,
    paused: Option<Res<crate::pause::Paused>>,
    script: Option<Res<crate::script::ScriptState>>,
    mut clock: ResMut<GameplayClock>,
) {
    let play = state.map_or(crate::PlayState::Playing, |s| *s.get());
    let in_menu = script.as_deref().and_then(|s| s.rt.as_ref()).is_some_and(|rt| rt.game.in_menu());
    let gate = if paused.is_some_and(|p| p.0) {
        Gate::Paused
    } else if play != crate::PlayState::Playing || in_menu {
        Gate::Bypass
    } else {
        Gate::Gameplay
    };
    // The scripts' frame time (`AidTime::delta_secs_f64`): the virtual clock's, or real
    // time while it stands still under a menu (`--menu-pause original`, `PlayClock`).
    let render_dt = if virtual_time.is_paused() {
        real.delta_secs_f64().min(0.25)
    } else {
        virtual_time.delta_secs_f64()
    };
    let report = clock.advance(real.delta_secs_f64(), render_dt, f64::from(virtual_time.relative_speed()), gate);
    if let Some(r) = report {
        info!(
            "gameplay clock: {:.2} ticks/frame over {} frames, {:.1} ms dropped, alpha {:.2}",
            r.ticks_per_frame(),
            r.frames,
            r.dropped * 1e3,
            r.alpha
        );
    }
}

/// Adds [`GameplayClock`] to a play run.
pub struct GameplayClockPlugin;

impl Plugin for GameplayClockPlugin {
    fn build(&self, app: &mut App) {
        let clock = GameplayClock::from_env();
        if clock.enabled() {
            info!("gameplay clock: {} Hz gameplay logic, rendering untouched (NVF_GAMEPLAY_HZ)", clock.hz());
        } else {
            info!("gameplay clock: one tick per rendered frame, today's behaviour (NVF_GAMEPLAY_HZ={})", clock.hz());
        }
        app.insert_resource(clock).add_systems(Update, advance_clock.before(crate::PlaySet::Input));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME_120: f64 = 1.0 / 120.0;
    const FRAME_60: f64 = 1.0 / 60.0;

    fn second(clock: &mut GameplayClock) -> f64 {
        let mut game = 0.0;
        for _ in 0..120 {
            clock.advance(FRAME_120, FRAME_120, 1.0, Gate::Gameplay);
            game += clock.game_dt() * f64::from(clock.ticks_this_frame());
        }
        game
    }

    #[test]
    fn a_twenty_five_ms_frame_is_one_tick_and_carries_the_rest() {
        let mut clock = GameplayClock::new(60);
        clock.advance(0.025, 0.025, 1.0, Gate::Gameplay);
        assert_eq!(clock.ticks_this_frame(), 1);
        assert!((clock.acc - 0.008_333_333).abs() < 1e-6, "{}", clock.acc);
        assert!((clock.game_dt() - FRAME_60).abs() < 1e-9);
        assert_eq!(clock.ticks(), 1);
    }

    #[test]
    fn a_two_hundred_fifty_ms_frame_runs_two_ticks_and_carries_one() {
        let mut clock = GameplayClock::new(60);
        clock.advance(0.25, 0.25, 1.0, Gate::Gameplay);
        assert_eq!(clock.ticks_this_frame(), 2);
        assert!((clock.acc - FRAME_60).abs() < 1e-6, "one step of debt: {}", clock.acc);
        // The 200 ms past the clamp is dropped; the debt is not.
        assert!((clock.dropped() - 0.2).abs() < 1e-6, "{}", clock.dropped());
        // The next ordinary frame spends the debt without a second tick.
        clock.advance(FRAME_120, FRAME_120, 1.0, Gate::Gameplay);
        assert_eq!(clock.ticks_this_frame(), 1);
    }

    #[test]
    fn a_second_250_ms_frame_drops_the_second_step_of_debt() {
        let mut clock = GameplayClock::new(60);
        clock.advance(0.25, 0.25, 1.0, Gate::Gameplay);
        let dropped = clock.dropped();
        clock.advance(0.25, 0.25, 1.0, Gate::Gameplay);
        assert_eq!(clock.ticks_this_frame(), 2);
        assert!((clock.acc - FRAME_60).abs() < 1e-6);
        assert!(clock.dropped() > dropped, "the excess debt is counted as dropped");
    }

    #[test]
    fn two_eight_ms_frames_make_one_tick() {
        let mut clock = GameplayClock::new(60);
        clock.advance(FRAME_120, FRAME_120, 1.0, Gate::Gameplay);
        assert_eq!(clock.ticks_this_frame(), 0);
        clock.advance(FRAME_120, FRAME_120, 1.0, Gate::Gameplay);
        assert_eq!(clock.ticks_this_frame(), 1);
    }

    #[test]
    fn thirty_fps_is_two_ticks_a_frame() {
        let mut clock = GameplayClock::new(60);
        clock.advance(1.0 / 30.0, 1.0 / 30.0, 1.0, Gate::Gameplay);
        assert_eq!(clock.ticks_this_frame(), 2);
        assert!((clock.game_dt() - FRAME_60).abs() < 1e-9);
    }

    #[test]
    fn a_second_of_render_frames_is_a_second_of_game_time() {
        let mut clock = GameplayClock::new(60);
        assert!((second(&mut clock) - 1.0).abs() < 1e-9, "{} ticks", clock.ticks());
        assert_eq!(clock.ticks(), 60);
        assert!(clock.dropped() < 1e-9);
    }

    #[test]
    fn a_menu_frame_ticks_once_with_the_render_time() {
        let mut clock = GameplayClock::new(60);
        clock.advance(0.001, 0.001, 1.0, Gate::Gameplay);
        assert_eq!(clock.ticks_this_frame(), 0);
        clock.advance(0.001, 0.001, 1.0, Gate::Bypass);
        assert_eq!(clock.ticks_this_frame(), 1);
        assert!((clock.game_dt() - 0.001).abs() < 1e-12, "MenuMode keeps the render dt");
        // The accumulator is not carried out of the menu.
        clock.advance(0.001, 0.001, 1.0, Gate::Gameplay);
        assert_eq!(clock.ticks_this_frame(), 0);
    }

    #[test]
    fn the_pause_menu_runs_no_tick_and_catches_nothing_up() {
        let mut clock = GameplayClock::new(60);
        clock.advance(0.25, 0.25, 1.0, Gate::Paused);
        assert_eq!(clock.ticks_this_frame(), 0);
        clock.advance(FRAME_60, FRAME_60, 1.0, Gate::Gameplay);
        assert_eq!(clock.ticks_this_frame(), 1, "not the pause's whole span");
    }

    #[test]
    fn slow_motion_keeps_sixty_ticks_a_second_and_scales_the_step() {
        let mut clock = GameplayClock::new(60);
        let mut game = 0.0;
        for _ in 0..120 {
            clock.advance(FRAME_120, FRAME_120, 0.5, Gate::Gameplay);
            game += clock.game_dt() * f64::from(clock.ticks_this_frame());
        }
        assert_eq!(clock.ticks(), 60);
        assert!((game - 0.5).abs() < 1e-9, "half a game second in one real second: {game}");
    }

    #[test]
    fn one_hundred_twenty_ticks_every_frame_with_the_render_time() {
        let mut clock = GameplayClock::new(120);
        assert!(!clock.enabled());
        for _ in 0..3 {
            clock.advance(FRAME_120, FRAME_120, 1.0, Gate::Gameplay);
            assert_eq!(clock.ticks_this_frame(), 1);
            assert!((clock.game_dt() - FRAME_120).abs() < 1e-12);
        }
        assert_eq!(clock.ticks(), 3);
        assert!(clock.dropped() < 1e-12);
    }

    #[test]
    fn the_second_report_counts_ticks_frames_and_dropped_time() {
        let mut clock = GameplayClock::new(60);
        let mut report = None;
        for _ in 0..61 {
            report = clock.advance(FRAME_60, FRAME_60, 1.0, Gate::Gameplay).or(report);
        }
        let r = report.expect("one second's report");
        assert_eq!(r.hz, 60.0);
        assert_eq!(r.ticks, r.frames, "one tick a frame");
        assert!((r.ticks_per_frame() - 1.0).abs() < 1e-9);
        assert!(r.dropped < 1e-9, "{}", r.dropped);

        let mut clock = GameplayClock::new(60);
        clock.advance(0.25, 0.25, 1.0, Gate::Gameplay);
        let mut report = None;
        for _ in 0..46 {
            report = clock.advance(FRAME_60, FRAME_60, 1.0, Gate::Gameplay).or(report);
        }
        let r = report.expect("the next report after 1.25 s");
        assert!((r.dropped - 0.2).abs() < 1e-6, "{}", r.dropped);
    }
}
