//! Dynamic resolution: the world drawn smaller than the window and scaled back up, as much
//! as holding a frame time takes. Off unless `--dyn-res` asks for it or the settings page's
//! Dynamic resolution row is on.
//!
//! The scene is drawn into the top-left of Bevy's HDR main texture: the camera carries
//! `MainPassResolutionOverride(render_size)` (bevy_camera; the same component FSR and DLSS
//! use, `upscaler/prep.rs`), which Bevy's 3D nodes turn into the viewport of the opaque,
//! transmissive and transparent passes and of the depth prepass, and which the shaders of the
//! world read through `View::main_pass_viewport` (`nif_material.wesl`, `water.wesl`; every
//! bevy_pbr shader too). The rest of the texture is this module's one pass's business
//! (`dyn_pass`): a fullscreen triangle sampling the render rectangle with a linear filter
//! into the picture at the window's size — FSR's spatial fallback exactly
//! (`upscaler::prep::Pipelines::fallback`, `fsr_fallback.wgsl`), the branch `fsr_pass` takes
//! when it has no motion vectors. After it the frame is a full-size HDR picture, so bloom,
//! tonemapping, the image space, eye adaptation and the interface all run at the window's
//! size, unscaled: the interface is never drawn smaller, so its text stays sharp.
//!
//! What is shared with the temporal upscalers, all of it through `upscaler::prep` (which
//! prepares any `UpscaleSettings`, and is not FSR's): the prepared view and its resolution
//! uniform, the ambient occlusion before the scale-up (`ao_before_fsr`), the depth prepass
//! drawn at the render size and expanded for the passes after it, the
//! `MainPassResolutionOverride` itself, the mip bias, and the cut/size transition rules. The
//! scaler is a third `UpscaleSettings` case and adds no pipeline and no texture of its own.
//!
//! The governor (`govern`) measures the frame, not the GPU: Metal's timestamp queries return
//! nothing on this build (`--log-frame-times` prints `-0.00 ms`), so the frame time is the
//! same `Time<Real>` delta the frame-time aid prints, over half-second windows, with a dead
//! band and a settle after each change. It steps down 5 % at a time (two steps when the frame
//! is far over) and steps back up when it is comfortably under, and it stops at 0.5 and at
//! native. The scale is only meaningful while the frame is not held by the display: with
//! vsync the frame time is the display's, not the engine's, and with a frame-rate cap at or
//! above the target it is the limiter's; in either the governor says so once and does nothing
//! (`--log-frame-times` runs with neither).
//!
//! Not covered, on purpose: the shadow maps (their views carry no override), the other
//! cameras (the water's mirror, the scope's lens, the Pip-Boy and the other screen views, the
//! frame generation's), and every run that saves a picture (a screenshot or the lab's frame
//! document is of the native picture, for the checks that compare it).

use bevy::core_pipeline::prepass::ViewPrepassTextures;
use bevy::core_pipeline::{Core3d, Core3dSystems};
use bevy::prelude::*;
use bevy::render::renderer::{RenderContext, RenderDevice, ViewQuery};
use bevy::render::view::ViewTarget;
use bevy::render::RenderApp;

use crate::fly::FlyCam;
use crate::frame_pace::{DisplaySync, FramePace};
use crate::upscaler::prep::{Policy, PreparedUpscale, UpscaleRenderer, UpscaleSettings};

/// The scales the governor steps through, coarsest first. Native (1.0) is the last: there the
/// override and this module's pass are taken away and the camera draws as it did.
pub const LADDER: [f32; 11] = [0.5, 0.55, 0.6, 0.65, 0.7, 0.75, 0.8, 0.85, 0.9, 0.95, 1.0];

/// The frame time `--dyn-res` holds without a value: 120 Hz, the panel's own grid.
pub const TARGET_MS: f32 = 8.33;

/// One window: how long since the scale last moved, and the frames timed in it.
const WINDOW: f32 = 0.5;
/// A change's own cost (textures made, the pass's first bind group) is not the picture's:
/// this long after one, the frames are left out of the mean.
const SETTLE: f32 = 0.2;
/// The dead band around the target: inside it the scale holds, so a frame time that wobbles
/// by a few percent does not make the picture's resolution wobble with it.
const OVER: f32 = 1.05;
const UNDER: f32 = 0.92;
/// A frame this far over the target (a place that costs half again as much) takes two steps
/// down in one window, so a scene change settles in a window or two, not ten.
const FAR_OVER: f32 = 2.0;

/// Dynamic resolution on a camera: the scale its world is drawn at (1.0 is native, and the
/// component is taken away then). In the main world, read by `upscaler::prep::extract`;
/// written by the governor, never by the renderer.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct DynamicResolution {
    pub scale: f32,
}

/// The scaler's request and the scale it chose, in the main world: `--dyn-res`, the settings
/// page's row, and what `govern` steps.
#[derive(Resource, Clone, Copy, Debug)]
pub struct DynamicRes {
    /// On at all (the row and the switch).
    pub enabled: bool,
    /// This run may draw smaller at all. A run that saves a picture (a screenshot, a frame
    /// document, a timelapse frame) is not one, unless `NVF_DYN_RES_PICTURES=1`: those checks
    /// compare the native picture. The settings row sets `enabled`, never this, so a stored
    /// row cannot make a pictured run differ from the native picture.
    pub allowed: bool,
    /// The frame time to hold, milliseconds.
    pub target_millis: f32,
    /// A scale from `--dyn-res-hold`: the governor leaves it alone, for measuring one scale.
    pub hold: Option<f32>,
    /// The scale the governor chose, 0.5 to 1.0.
    pub scale: f32,
}

impl DynamicRes {
    /// This run's request: off without `--dyn-res` (the settings page can still turn it on),
    /// and never on in the runs that save a picture (the screenshot and frame-document checks
    /// compare the native picture). `NVF_DYN_RES_PICTURES=1` lets a pictured run use it
    /// anyway, to look at a picture of it against the native one.
    pub fn from_args(args: &crate::Args) -> Self {
        let pictures = std::env::var("NVF_DYN_RES_PICTURES").is_ok_and(|v| v.trim() == "1");
        let pictured =
            !pictures && (args.screenshot.is_some() || args.frame_json.is_some() || args.timelapse.is_some());
        let hold = args.dyn_res_hold.map(|scale| scale.clamp(LADDER[0], 1.0));
        Self {
            enabled: args.dyn_res.is_some(),
            allowed: !pictured,
            target_millis: args.dyn_res.unwrap_or(TARGET_MS).clamp(2.0, 100.0),
            hold,
            scale: hold.unwrap_or(1.0),
        }
    }

    /// On, and this run may draw smaller: the camera is prepared as an upscaler's is
    /// (`graphics::apply`: single-sampled, HDR, the jitter and mip bias components) from the
    /// moment the feature is on, whatever the scale. Keeping that state while the ladder sits
    /// at 1.0 is what stops a round trip through native from turning multisampling and the HDR
    /// view back on and off again (a pipeline re-specialisation and a main-texture swap at
    /// every crossing).
    pub fn requested(&self) -> bool {
        self.allowed && self.enabled
    }

    /// The world is drawn smaller than the window: the prepared view, the override and
    /// `dyn_pass`.
    pub fn active(&self) -> bool {
        self.requested() && self.scale < 1.0
    }
}

pub struct DynResPlugin(pub DynamicRes);

impl Plugin for DynResPlugin {
    fn build(&self, app: &mut App) {
        if !self.0.allowed {
            info!("dynamic resolution: off in this run, which saves a picture (NVF_DYN_RES_PICTURES=1 overrides)");
        } else if self.0.requested() {
            match self.0.hold {
                Some(scale) => info!("dynamic resolution: on, held at {scale:.2}"),
                None => info!("dynamic resolution: on, holding {:.2} ms", self.0.target_millis),
            }
        }
        // Before `graphics::apply`, so the frame the scale first leaves native is also the
        // frame the camera turns single-sampled and HDR: the prepared view is never briefly
        // unsuitable (which would draw it natively and log the warning once).
        app.insert_resource(self.0)
            .add_systems(Update, (govern, attach).chain().before(crate::graphics::apply));
        let Some(render) = app.get_sub_app_mut(RenderApp) else { return };
        render.add_systems(
            Core3d,
            dyn_pass
                .in_set(Core3dSystems::EarlyPostProcess)
                // The occlusion is drawn over the render rectangle before this, and the
                // scaled picture before everything that expects the window's size.
                .after(crate::ssao::ao_before_fsr)
                .before(bevy::post_process::bloom::bloom)
                .before(crate::post::bright_pass_hdr)
                .before(crate::fsr::fsr_pass),
        );
    }
}

/// The scene's size for a window of `output` at `scale`: even numbers (a linear filter's half
/// texel lands on a whole one), and below 1.0 always smaller than `output`, which
/// `MainPassResolutionOverride` requires.
pub(crate) fn render_size(output: UVec2, scale: f32) -> UVec2 {
    if scale >= 1.0 {
        return output;
    }
    let scaled = (output.as_vec2() * scale.clamp(0.25, 1.0)).round().as_uvec2();
    let limit = output.saturating_sub(UVec2::ONE);
    UVec2::new(scaled.x.min(limit.x) & !1, scaled.y.min(limit.y) & !1).max(UVec2::splat(2))
}

/// The policy the governor asks `upscaler::prep` for: the render size of `scale`, no jitter
/// (nothing here accumulates several frames, and an unresolved offset only softens the
/// picture) and the mip bias a smaller picture needs so its textures do not alias.
pub(crate) fn policy(scale: f32, output: UVec2) -> Option<Policy> {
    if scale >= 1.0 {
        return None;
    }
    let render_size = render_size(output, scale);
    (render_size != output && render_size.min_element() >= 64).then_some(Policy {
        render_size,
        jitter_phases: 1,
        mip_bias: (render_size.x as f32 / output.x.max(1) as f32).log2(),
    })
}

/// The ladder index nearest `scale`.
fn nearest(scale: f32) -> usize {
    let mut best = 0;
    for (index, step) in LADDER.iter().enumerate() {
        if (*step - scale).abs() < (LADDER[best] - scale).abs() {
            best = index;
        }
    }
    best
}

/// The ladder index the next window takes, from this window's mean frame time (`None`: hold
/// it). One step up or down, two down when the frame is far over, nothing at either end.
pub(crate) fn step(index: usize, mean_ms: f32, target_ms: f32) -> Option<usize> {
    let next = if mean_ms > target_ms * OVER {
        index.saturating_sub(if mean_ms > target_ms * FAR_OVER { 2 } else { 1 })
    } else if mean_ms < target_ms * UNDER {
        (index + 1).min(LADDER.len() - 1)
    } else {
        index
    };
    (next != index).then_some(next)
}

/// The governor: half-second windows of the frame time, a step down when over the target and
/// up when under, held in between.
fn govern(
    time: Res<Time<Real>>,
    pace: Res<FramePace>,
    mut res: ResMut<DynamicRes>,
    play_state: Option<Res<State<nvf_play::PlayState>>>,
    // A camera a temporal upscaler draws: it is the one that chose the picture's resolution,
    // and this scaler never shares it (`upscaler::prep::extract`).
    temporal: Query<(), (With<FlyCam>, Or<(With<crate::fsr::FsrUpscaling>, With<crate::upscaler::DlssUpscaling>)>)>,
    mut window: Local<(f32, f32, u32)>,
    mut said: Local<u8>,
) {
    // Off (or not allowed in this run): nothing measured, nothing said. Before the display
    // checks below, so an ordinary run never hears about a feature it did not ask for.
    if !res.requested() || res.hold.is_some() {
        *window = (0.0, 0.0, 0);
        return;
    }
    // Nothing is chosen while the display hands the frame its time: with vsync every frame
    // lasts the refresh interval whatever the engine did, and the scaler would fall to its
    // floor and stay there. Said once.
    if pace.sync == DisplaySync::Vsync {
        if *said & 1 == 0 {
            *said |= 1;
            warn!("dynamic resolution: vsync hands every frame the display's time; it needs vsync off or variable refresh to see the frame's own, and holds its scale until then");
        }
        *window = (0.0, 0.0, 0);
        return;
    }
    // The same for a frame-rate cap at or above the target (the page's, `--log-frame-times`
    // leaves it 0): the limiter waits, so the time it measures is the cap and not this frame's
    // work, and the scale would walk to its floor for nothing.
    if pace.cap != 0 && 1000.0 / pace.cap as f32 > res.target_millis * UNDER {
        if *said & 4 == 0 {
            *said |= 4;
            warn!("dynamic resolution: a {}-fps frame-rate cap hands every frame its time and it holds its scale; it needs the cap off (or above the target) to see the frame's own", pace.cap);
        }
        *window = (0.0, 0.0, 0);
        return;
    }
    // Nor while the world is not what is drawn: a menu, a conversation or a loading screen
    // is the interface's frame, and holding a scale to it would take the world away when the
    // player comes back to it.
    let playing = play_state.as_ref().is_none_or(|state| *state.get() == nvf_play::PlayState::Playing);
    if !playing || !temporal.is_empty() {
        *window = (0.0, 0.0, 0);
        return;
    }
    let dt = time.delta_secs();
    // A stall (a cell load, a pipeline compiled on the frame) is not this frame's cost: one
    // 300 ms frame in a half-second window would mean 300 ms and take two steps off the
    // ladder for the half second after it. The same 250 ms is a cut for the upscaler's
    // history (`upscaler::prep::UpscaleCamera::cut_from`).
    if dt > 0.25 {
        *window = (0.0, 0.0, 0);
        return;
    }
    window.0 += dt;
    if window.0 > SETTLE {
        window.1 += dt;
        window.2 += 1;
    }
    if window.0 < WINDOW || window.2 == 0 {
        return;
    }
    let mean_ms = window.1 * 1e3 / window.2 as f32;
    *window = (0.0, 0.0, 0);
    let index = nearest(res.scale);
    match step(index, mean_ms, res.target_millis) {
        Some(next) => {
            res.scale = LADDER[next];
            info!("dyn res: {:.2}, {mean_ms:.2} ms mean against a {:.2} ms target", res.scale, res.target_millis);
        }
        // At the floor and still over: the rest of this frame is not the picture's size.
        None if index == 0 && mean_ms > res.target_millis * OVER && *said & 2 == 0 => {
            *said |= 2;
            warn!("dyn res: at 0.50, {mean_ms:.2} ms against a {:.2} ms target: the rest of this frame costs the same at every resolution (CPU work per view and per pass), so it has to come from elsewhere", res.target_millis);
        }
        None => {}
    }
}

/// Put the governor's scale on the game's camera, or take it away (native, or a camera a
/// temporal upscaler draws and this one must not).
fn attach(
    mut commands: Commands,
    res: Res<DynamicRes>,
    temporal: Query<(), (With<FlyCam>, Or<(With<crate::fsr::FsrUpscaling>, With<crate::upscaler::DlssUpscaling>)>)>,
    missing: Query<Entity, (With<FlyCam>, Without<DynamicResolution>)>,
    present: Query<(Entity, &DynamicResolution)>,
) {
    if !res.active() || !temporal.is_empty() {
        for (entity, _) in &present {
            commands.entity(entity).remove::<DynamicResolution>();
        }
        return;
    }
    let scale = DynamicResolution { scale: res.scale };
    for entity in &missing {
        commands.entity(entity).insert(scale);
    }
    for (entity, have) in &present {
        if *have != scale {
            commands.entity(entity).insert(scale);
        }
    }
}

/// The scale-up and the depth expansion after it, exactly as `fsr_pass` does them when it
/// reconstructs nothing: the render rectangle into the picture at the window's size, then the
/// render-size depth nearest-expanded over the display-size depth for the effects that read
/// it there (the sun shafts; the water reads it before this, at the render size).
fn dyn_pass(
    view: ViewQuery<(&ViewTarget, &PreparedUpscale, Option<&ViewPrepassTextures>)>,
    upscale: Res<UpscaleRenderer>,
    device: Res<RenderDevice>,
    mut ctx: RenderContext,
) {
    let (target, prepared, prepass) = view.into_inner();
    if !matches!(prepared.camera.settings, UpscaleSettings::Dynamic(_)) {
        return;
    }
    let Some(common) = &upscale.0 else { return };
    let device = device.wgpu_device();
    let encoder = ctx.command_encoder();
    common.fallback(device, encoder, prepared, target);
    if let Some(depth) = prepass.and_then(|prepass| prepass.depth.as_ref()) {
        common.expand_depth(device, encoder, prepared, &depth.texture.texture);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rendered_size_is_even_smaller_than_the_window_and_on_the_ladder() {
        let output = UVec2::new(1280, 800);
        for scale in LADDER[..LADDER.len() - 1].iter() {
            let size = render_size(output, *scale);
            assert_eq!(size.x % 2, 0, "{scale}");
            assert_eq!(size.y % 2, 0, "{scale}");
            assert!(size.x < output.x && size.y < output.y, "{scale}: {size:?} is smaller than the window");
            assert!(size.x >= 64 && size.y >= 64, "{scale}: {size:?} is drawable");
        }
        assert_eq!(render_size(output, 1.0), output, "native is the window itself");
        assert_eq!(render_size(output, 0.7), UVec2::new(896, 560));
        assert_eq!(render_size(output, 0.5), UVec2::new(640, 400));
        // Monotone: a smaller scale is never a bigger picture.
        let sizes: Vec<UVec2> = LADDER.iter().map(|&scale| render_size(output, scale)).collect();
        assert!(sizes.windows(2).all(|pair| pair[0].x <= pair[1].x && pair[0].y <= pair[1].y));
    }

    #[test]
    fn native_asks_for_no_prepared_view_and_no_jitter() {
        let output = UVec2::new(1280, 800);
        assert_eq!(policy(1.0, output), None, "native: no override, no pass");
        let prepared = policy(0.7, output).expect("0.7 is a prepared view");
        assert_eq!(prepared.render_size, UVec2::new(896, 560));
        assert_eq!(prepared.jitter_phases, 1);
        assert!((prepared.mip_bias - 0.7f32.log2()).abs() < 1e-6, "the mip bias is log2 of the scale");
        assert_eq!(policy(0.5, UVec2::splat(32)), None, "a view too small to halve renders natively");
    }

    #[test]
    fn the_governor_steps_down_fast_up_slowly_and_holds_in_the_band() {
        let target = TARGET_MS;
        assert_eq!(step(5, target * OVER + 0.1, target), Some(4), "a little over: one step down");
        assert_eq!(step(5, target * FAR_OVER + 0.1, target), Some(3), "far over: two steps down");
        assert_eq!(step(1, target * FAR_OVER + 0.1, target), Some(0), "no lower than the floor");
        assert_eq!(step(0, target * FAR_OVER + 0.1, target), None, "at the floor: nothing to do");
        assert_eq!(step(5, target * UNDER - 0.1, target), Some(6), "under: one step up");
        assert_eq!(step(LADDER.len() - 1, 1.0, target), None, "at native: stays");
        assert_eq!(step(5, target, target), None, "in the band: holds");
        assert_eq!(step(5, target * 1.04, target), None, "just over but inside the band: holds");
    }

    #[test]
    fn the_nearest_ladder_step_is_the_one_the_governor_keeps() {
        for (index, scale) in LADDER.iter().enumerate() {
            assert_eq!(nearest(*scale), index);
        }
        assert_eq!(nearest(0.72), 4, "0.70");
        assert_eq!(nearest(0.51), 0, "0.50");
    }

    #[test]
    fn only_a_requested_scale_below_native_draws_smaller() {
        let off = DynamicRes { enabled: false, allowed: true, target_millis: TARGET_MS, hold: None, scale: 0.7 };
        assert!(!off.requested() && !off.active(), "off draws native and stays far from the camera");
        let on = DynamicRes { enabled: true, ..off };
        assert!(on.requested() && on.active());
        let pictured = DynamicRes { allowed: false, ..on };
        assert!(!pictured.requested() && !pictured.active(), "a pictured run never scales, whatever the settings row says");
        let native = DynamicRes { scale: 1.0, ..on };
        assert!(native.requested() && !native.active(), "native keeps the prepared-view state but draws the whole window");
    }
}
