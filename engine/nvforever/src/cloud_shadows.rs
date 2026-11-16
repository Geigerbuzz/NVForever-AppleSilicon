//! Cloud shadows (`--cloud-shadows`, off by default): coarse, moving patches of shade
//! from the weather's clouds, projected onto the ground and everything standing on it.
//! Rebuilds the look of Cloud Shadows (Nexus 79147) natively: the original is an NVSE
//! plugin, which this engine cannot load.
//!
//! **What it is.** One screen-space multiply over the tonemapped picture (after the
//! ambient occlusion and the anti-aliasing, before bloom and the image-space grade). The
//! depth prepass gives each pixel's place in the world; a small tileable mask texture is
//! read there in world XZ, so the patches stay put on the ground and on objects as the
//! camera moves. The mask is driven by the weather:
//! - **Coverage.** The weather's DATA "classification" flags (1 Pleasant, 2 Cloudy,
//!   4 Rainy, 8 Snow) give the share of the sky the clouds cover, blended across a weather
//!   fade like the sky's own colours ([`coverage`], [`coverage_now`]). A clear Mojave
//!   noon is 0.25 (thin wisps), an overcast weather 0.8, rain 0.95.
//! - **Motion.** The mask scrolls with the lowest sky cloud layer's own ONAM speed
//!   (`fnv_world::sky::Weather::cloud_offset`, the same offset `sky.rs` gives that layer's
//!   texture) and with the weather's wind: direction and strength from
//!   `crate::tree::wind_vector` (a script's `SetWindDirection` included), drifting
//!   [`WIND_DRIFT`] game units a second at full strength.
//! - **Strength.** Only direct sunlight is occluded by a cloud, so the darkening is
//!   scaled by [`sun_share`] (the sunlight colour's luminance against the ambient's, once
//!   the sun is up: `SkyState::daylight`); at night, indoors, and under a weather whose
//!   light is almost all ambient the shadows fade out. [`SHADOW_STRENGTH`] caps how dark
//!   a cloud's core gets.
//!
//! **Where it multiplies.** The best place physically is the sun's shadow term in the lit
//! shaders (only the direct light a cloud blocks), but this engine has no single hook
//! there: every material's lighting goes through Bevy's vendored `apply_pbr_lighting`,
//! which would need a view-level texture and uniform on every lit pipeline (the standard
//! material, terrain, tree, water, sky) and would cost a texture fetch in all of them
//! even with the switch off. A post multiply is the same choice the ambient occlusion
//! makes (ssao.rs) and costs one pass only while the switch is on; it darkens the
//! ambient light too, which is why its strength is scaled by the sunlight's share and
//! kept low.
//!
//! **Cost.** One full-screen pass at the display size: one depth load, one mask fetch and
//! a matrix multiply a pixel. A plain full-screen multiply is about 0.2 ms at 1280x800;
//! this one is estimated at 0.25-0.4 ms there [hypothesis, to be measured with
//! `--log-frame-times`; see reports/clouds.md].
//!
//! **Default off.** The plugin adds nothing unless `--cloud-shadows` is passed: no mask
//! texture, no component, no pipeline and no pass. With the switch off the render is
//! bit-identical to before.
//!
//! The mask is generated once at startup by [`mask_texels`] (tileable value noise, three
//! octaves) rather than taken from the sky's cloud textures [engineering choice]: the
//! game projects those through the sky dome's own coordinates, and their world-space
//! mapping is not known (docs/spec-notes/world.md §6 "Cloud texture placement does not
//! match the game's"), so using them for ground shadows would invent one. The weather's
//! data still sets the coverage, the speed and the wind.

use crate::fly::FlyCam;
use crate::sky::SkyState;
use bevy::asset::RenderAssetUsages;
use bevy::core_pipeline::prepass::ViewPrepassTextures;
use bevy::core_pipeline::tonemapping::tonemapping;
use bevy::core_pipeline::{Core3d, Core3dSystems, FullscreenShader};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::diagnostic::RecordDiagnostics;
use bevy::render::extract_component::{ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin, UniformComponentPlugin};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::binding_types::{sampler, texture_2d, texture_depth_2d, texture_depth_2d_multisampled, uniform_buffer};
use bevy::render::render_resource::{
    AddressMode, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedRenderPipelineId, ColorTargetState, ColorWrites, Extent3d, FilterMode,
    FragmentState, Operations, PipelineCache, RenderPassColorAttachment, RenderPassDescriptor, RenderPipelineDescriptor, Sampler, SamplerBindingType,
    SamplerDescriptor, ShaderStages, ShaderType, TextureDimension, TextureFormat, TextureSampleType,
};
use bevy::render::renderer::{RenderContext, RenderDevice, ViewQuery};
use bevy::render::texture::GpuImage;
use bevy::render::view::{ExtractedView, ViewTarget};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use fnv_world::sky::{ColorType, Weather};

/// Share of the sky an overcast weather's clouds cover, by WTHR DATA classification flags
/// (1 Pleasant, 2 Cloudy, 4 Rainy, 8 Snow; docs/spec-notes/world.md §6) [hypothesis: the
/// records have no coverage byte, so the class is the game's own statement about the sky;
/// a weather carrying several flags takes the highest].
pub const COVERAGE_PLEASANT: f32 = 0.25;
pub const COVERAGE_CLOUDY: f32 = 0.80;
pub const COVERAGE_RAINY: f32 = 0.95;
pub const COVERAGE_SNOW: f32 = 0.75;
/// A weather with no classification flag of those four: unsettled, neither clear nor
/// overcast [hypothesis].
pub const COVERAGE_UNKNOWN: f32 = 0.35;
/// The most a cloud's core darkens the picture at full coverage and full sun
/// [engineering choice: strong enough to read on sand and concrete at noon, not a
/// blackout; the mask's own patchiness means few pixels reach it].
pub const SHADOW_STRENGTH: f32 = 0.75;
/// Game units per repeat of the mask texture: the patches are about this far apart
/// (roughly 280 m; a cloud some 2 km up casts a shadow of the same order)
/// [engineering choice, to be tuned by eye].
pub const MASK_REPEAT: f32 = 20_000.0;
/// Game units a second the mask drifts with the wind at strength 1 (WTHR wind speed 255)
/// [engineering choice, to be tuned by eye: fast enough to see, slow enough not to flicker].
pub const WIND_DRIFT: f32 = 600.0;
/// Side of the generated mask in texels; the mask is deliberately coarse.
pub const MASK_SIZE: u32 = 256;

/// The weather's cloud coverage (0..1) from its classification flags ([`COVERAGE_*`]).
pub fn coverage(w: &Weather) -> f32 {
    let c = w.classification;
    if c & 4 != 0 {
        COVERAGE_RAINY
    } else if c & 2 != 0 {
        COVERAGE_CLOUDY
    } else if c & 8 != 0 {
        COVERAGE_SNOW
    } else if c & 1 != 0 {
        COVERAGE_PLEASANT
    } else {
        COVERAGE_UNKNOWN
    }
}

/// The coverage shown now, blended from the outgoing weather to the shown one over the
/// fade (the same way `SkyState::color` blends the sky's colours; a change to a weather
/// with different clouds therefore thickens or thins the shadows smoothly). Indoors the
/// sky is not drawn, so there is no coverage either.
pub fn coverage_now(sky: &SkyState) -> f32 {
    if sky.interior.is_some() {
        return 0.0;
    }
    let cur = coverage(&sky.weather);
    match &sky.prev {
        Some(p) => coverage(p) + (cur - coverage(p)) * sky.fade,
        None => cur,
    }
}

/// The share of the scene's light that is direct sunlight now (0..1): the sunlight
/// colour's luminance set against the ambient's, times `SkyState::daylight` (0 at the
/// horizon). A cloud can only take away the direct part [hypothesis: the weather colours
/// stand for the two lights' strengths; the renderer's ambient and sun scales are not
/// folded in].
pub fn sun_share(sky: &SkyState) -> f32 {
    let lum = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    let sun = lum(sky.color(ColorType::Sunlight)) * sky.daylight();
    let ambient = lum(sky.color(ColorType::Ambient));
    sun / (sun + ambient + 0.02)
}

/// How far a cloud's core darkens the picture now (0: none): [`SHADOW_STRENGTH`] times
/// the coverage and the sunlight's share.
pub fn strength(coverage: f32, sun_share: f32) -> f32 {
    (SHADOW_STRENGTH * coverage.clamp(0.0, 1.0) * sun_share.clamp(0.0, 1.0)).clamp(0.0, 1.0)
}

/// One 0..1 hash of a lattice point (deterministic on every platform).
fn hash2(x: i32, y: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8DA6_B343) ^ (y as u32).wrapping_mul(0xD816_3841);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h & 0x00FF_FFFF) as f32 / 16_777_215.0
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Tileable value noise at `u, v` in 0..1 with `period` lattice cells a side.
fn value_noise(u: f32, v: f32, period: i32) -> f32 {
    let x = u * period as f32;
    let y = v * period as f32;
    let (x0, y0) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (smooth(x - x0 as f32), smooth(y - y0 as f32));
    let wrap = |i: i32| i.rem_euclid(period);
    let (x0, x1, y0, y1) = (wrap(x0), wrap(x0 + 1), wrap(y0), wrap(y0 + 1));
    let top = hash2(x0, y0) + (hash2(x1, y0) - hash2(x0, y0)) * fx;
    let bottom = hash2(x0, y1) + (hash2(x1, y1) - hash2(x0, y1)) * fx;
    top + (bottom - top) * fy
}

/// The mask's texels (R8, `size` x `size`, tileable): broad cloud bodies with soft edges,
/// most of the sky clear. Three octaves of value noise, then a smoothstep so the thin
/// tails disappear and the cores stay [engineering choice, not the game's cloud art].
pub fn mask_texels(size: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((size * size) as usize);
    for j in 0..size {
        for i in 0..size {
            let u = i as f32 / size as f32;
            let v = j as f32 / size as f32;
            let n = 0.60 * value_noise(u, v, 5) + 0.30 * value_noise(u, v, 11) + 0.10 * value_noise(u, v, 23);
            let a = {
                let t = ((n - 0.35) / (0.80 - 0.35)).clamp(0.0, 1.0);
                t * t * (3.0 - 2.0 * t)
            };
            out.push((a * 255.0 + 0.5) as u8);
        }
    }
    out
}

/// The generated mask texture, made once at startup.
#[derive(Resource)]
struct CloudMask(Handle<Image>);

/// Cloud shadows on a camera: the uniform of the pass. Presence = the switch is on.
#[derive(Component, ExtractComponent, Clone, Copy, PartialEq, ShaderType)]
#[extract_app(bevy::render::RenderApp)]
pub struct CloudShadows {
    /// Clip space of this frame to world space.
    pub world_from_clip: Mat4,
    /// x: strength now (0 none), y unused, z: world units per mask repeat, w: the lowest
    /// cloud layer's ONAM offset (texture repeats).
    pub params: Vec4,
    /// x, z: the wind (unit, world XZ); y: how far it has carried the mask (game units).
    pub wind: Vec4,
    /// xyz: the camera; w: the distance beyond which the depth prepass is the sky.
    pub camera: Vec4,
}

/// The mask texture the pass samples (a handle of its own so extraction can find the
/// image; the component above is the uniform).
#[derive(Component, ExtractComponent, Clone)]
#[extract_app(bevy::render::RenderApp)]
struct CloudShadowMask(Handle<Image>);

/// `--cloud-shadows`: add the pass (off: nothing at all).
pub struct CloudShadowsPlugin {
    pub on: bool,
}

impl CloudShadowsPlugin {
    pub fn new(on: bool) -> Self {
        CloudShadowsPlugin { on }
    }
}

impl Plugin for CloudShadowsPlugin {
    fn build(&self, app: &mut App) {
        if !self.on {
            return;
        }
        bevy::asset::embedded_asset!(app, "cloud_shadows.wesl");
        app.add_plugins((ExtractComponentPlugin::<CloudShadows>::default(), UniformComponentPlugin::<CloudShadows>::default()))
            .add_plugins(ExtractComponentPlugin::<CloudShadowMask>::default())
            .add_systems(Startup, make_mask)
            .add_systems(
                PostUpdate,
                follow_shadows.after(bevy::camera::CameraUpdateSystems).after(bevy::transform::TransformSystems::Propagate),
            );
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .add_systems(RenderStartup, init_pipelines)
            .add_systems(Render, prepare.in_set(RenderSystems::PrepareResources))
            .add_systems(
                Core3d,
                shadows_pass
                    .in_set(Core3dSystems::PostProcess)
                    .after(tonemapping)
                    .after(crate::ssao::ao_pass)
                    .after(crate::taa::taa_pass)
                    .after(bevy::anti_alias::fxaa::fxaa)
                    .after(bevy::anti_alias::smaa::smaa)
                    .after(crate::shafts::shafts_pass)
                    .before(crate::post::post_pass)
                    .before(crate::imagespace::blur_pass),
            );
    }
}

/// The mask texture, once.
fn make_mask(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let image = Image::new(
        Extent3d { width: MASK_SIZE, height: MASK_SIZE, depth_or_array_layers: 1 },
        TextureDimension::D2,
        mask_texels(MASK_SIZE),
        TextureFormat::R8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    commands.insert_resource(CloudMask(images.add(image)));
}

/// Each frame: the camera's clip-to-world matrix, the weather's coverage/strength, the
/// wind and the lowest cloud layer's scroll, into the camera's component. The camera's
/// `GlobalTransform` is this frame's (after propagation), so the mask does not lag the view.
fn follow_shadows(
    mut commands: Commands,
    sky: Res<SkyState>,
    time: Res<Time>,
    mask: Option<Res<CloudMask>>,
    cams: Query<(Entity, &GlobalTransform, &Projection, Option<&CloudShadows>, Option<&CloudShadowMask>), With<FlyCam>>,
) {
    let Some(mask) = mask else { return };
    let secs = time.elapsed_secs_f64();
    let wind = crate::tree::wind_vector(&sky);
    let coverage = coverage_now(&sky);
    let amount = strength(coverage, sun_share(&sky));
    let offset = sky.weather.cloud_offset(3, secs);
    for (e, gt, proj, current, current_mask) in &cams {
        let world_from_clip = gt.to_matrix() * proj.get_clip_from_view().inverse();
        let want = CloudShadows {
            world_from_clip,
            params: Vec4::new(amount, 0.0, MASK_REPEAT, offset),
            wind: Vec4::new(wind.x, (wind.w * WIND_DRIFT * secs as f32).max(0.0), wind.z, 0.0),
            camera: gt.translation().extend(crate::sky::SKY_DEPTH_FROM),
        };
        if current != Some(&want) {
            commands.entity(e).insert(want);
        }
        if current_mask.is_none() {
            commands.entity(e).insert(CloudShadowMask(mask.0.clone()));
        }
    }
}

#[derive(Resource)]
struct ShadowsPipelines {
    /// The pass's layout with a single-sampled and a multisampled depth prepass.
    layouts: [BindGroupLayoutDescriptor; 2],
    /// The picture, clamped; the mask repeats.
    sampler: Sampler,
    mask_sampler: Sampler,
    base: RenderPipelineDescriptor,
    pipelines: HashMap<(TextureFormat, bool), CachedRenderPipelineId>,
}

/// A view's pipeline, whether its depth is multisampled, and the picture's format.
#[derive(Component)]
struct ShadowsView {
    pipeline: CachedRenderPipelineId,
    multi: bool,
    format: TextureFormat,
}

fn init_pipelines(mut commands: Commands, device: Res<RenderDevice>, assets: Res<AssetServer>, fullscreen: Res<FullscreenShader>) {
    let layout = |multi: bool| {
        let depth = if multi { texture_depth_2d_multisampled() } else { texture_depth_2d() };
        BindGroupLayoutDescriptor::new(
            "cloud_shadows_layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::FRAGMENT,
                (
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    sampler(SamplerBindingType::Filtering),
                    uniform_buffer::<CloudShadows>(true),
                    depth,
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    sampler(SamplerBindingType::Filtering),
                ),
            ),
        )
    };
    let sampler = device.create_sampler(&SamplerDescriptor {
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        address_mode_u: AddressMode::ClampToEdge,
        address_mode_v: AddressMode::ClampToEdge,
        ..default()
    });
    let mask_sampler = device.create_sampler(&SamplerDescriptor {
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        address_mode_u: AddressMode::Repeat,
        address_mode_v: AddressMode::Repeat,
        ..default()
    });
    let base = RenderPipelineDescriptor {
        label: Some("cloud_shadows_pipeline".into()),
        layout: vec![layout(false)],
        vertex: fullscreen.to_vertex_state(),
        fragment: Some(FragmentState { shader: assets.load("embedded://nvforever/cloud_shadows.wesl"), entry_point: Some("apply".into()), ..default() }),
        ..default()
    };
    commands.insert_resource(ShadowsPipelines { layouts: [layout(false), layout(true)], sampler, mask_sampler, base, pipelines: HashMap::default() });
}

fn prepare(
    mut commands: Commands,
    cache: Res<PipelineCache>,
    mut pipelines: ResMut<ShadowsPipelines>,
    views: Query<(Entity, &ExtractedView, &Msaa, &CloudShadows)>,
) {
    for (entity, view, msaa, _) in &views {
        let multi = msaa.samples() > 1;
        let format = view.target_format;
        let id = match pipelines.pipelines.get(&(format, multi)) {
            Some(id) => *id,
            None => {
                let mut d = pipelines.base.clone();
                d.layout = vec![pipelines.layouts[multi as usize].clone()];
                if let Some(f) = d.fragment.as_mut() {
                    f.targets = vec![Some(ColorTargetState { format, blend: None, write_mask: ColorWrites::ALL })];
                    if multi {
                        f.shader_defs.push("MULTISAMPLED".into());
                    }
                }
                let id = cache.queue_render_pipeline(d);
                pipelines.pipelines.insert((format, multi), id);
                id
            }
        };
        commands.entity(entity).insert(ShadowsView { pipeline: id, multi, format });
    }
}

/// The picture times the cloud shadow, from the depth prepass's world positions.
fn shadows_pass(
    view: ViewQuery<(&ViewTarget, &DynamicUniformIndex<CloudShadows>, &CloudShadows, &ShadowsView, &ViewPrepassTextures, &CloudShadowMask)>,
    pipelines: Res<ShadowsPipelines>,
    cache: Res<PipelineCache>,
    uniforms: Res<ComponentUniforms<CloudShadows>>,
    images: Res<RenderAssets<GpuImage>>,
    device: Res<RenderDevice>,
    mut ctx: RenderContext,
) {
    let (target, index, shadows, sv, prepass, mask) = view.into_inner();
    if shadows.params.x <= 0.0 || !crate::ssao::format_matches(target, sv.format) {
        return;
    }
    if crate::ssao::depth_multisampled(prepass) != Some(sv.multi) {
        return;
    }
    let Some(depth) = prepass.depth_only_view() else { return };
    let Some(mask) = images.get(&mask.0) else { return };
    let Some(pipeline) = cache.get_render_pipeline(sv.pipeline) else { return };
    let Some(binding) = uniforms.uniforms().binding() else { return };
    // GPU time of the pass (recorded with --log-frame-times, graphics.rs).
    let diagnostics = ctx.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let span = diagnostics.time_span(ctx.command_encoder(), "nvf_cloud_shadows");
    let post = target.post_process_write();
    let layout = cache.get_bind_group_layout(&pipelines.layouts[sv.multi as usize]);
    let bg = device.create_bind_group(
        "cloud_shadows",
        &layout,
        &BindGroupEntries::sequential((post.source, &pipelines.sampler, binding, depth, &mask.texture_view, &pipelines.mask_sampler)),
    );
    // The pass borrows the encoder, so it is dropped before the profiling span is closed.
    {
        let mut pass = ctx.command_encoder().begin_render_pass(&RenderPassDescriptor {
            label: Some("cloud_shadows_pass"),
            color_attachments: &[Some(RenderPassColorAttachment { view: post.destination, depth_slice: None, resolve_target: None, ops: Operations::default() })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bg, &[index.index()]);
        pass.draw(0..3, 0..1);
    }
    span.end(ctx.command_encoder());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weather(classification: u8) -> Weather {
        Weather { classification, ..Default::default() }
    }

    #[test]
    fn coverage_follows_the_weather_class() {
        assert_eq!(coverage(&weather(1)), COVERAGE_PLEASANT);
        assert_eq!(coverage(&weather(2)), COVERAGE_CLOUDY);
        assert_eq!(coverage(&weather(4)), COVERAGE_RAINY);
        assert_eq!(coverage(&weather(8)), COVERAGE_SNOW);
        assert_eq!(coverage(&weather(0)), COVERAGE_UNKNOWN);
        // A weather with several flags takes the most overcast.
        assert_eq!(coverage(&weather(1 | 2)), COVERAGE_CLOUDY);
        assert!(coverage(&weather(2)) > coverage(&weather(1)));
        assert!(coverage(&weather(4)) >= coverage(&weather(2)));
    }

    #[test]
    fn no_shadow_without_cloud_or_sun() {
        assert_eq!(strength(0.0, 1.0), 0.0);
        assert_eq!(strength(1.0, 0.0), 0.0);
        assert!((strength(0.8, 0.6) - 0.36).abs() < 1e-6);
        assert_eq!(strength(2.0, 2.0), 1.0);
    }

    #[test]
    fn the_mask_is_deterministic_and_patchy() {
        let a = mask_texels(64);
        let b = mask_texels(64);
        assert_eq!(a, b);
        let mean = a.iter().map(|&v| f32::from(v)).sum::<f32>() / a.len() as f32;
        // Some clear sky, some cover: not a flat field in either direction.
        assert!((20.0..200.0).contains(&mean), "mean {mean}");
        assert!(a.iter().any(|&v| v < 40));
        assert!(a.iter().any(|&v| v > 180));
    }

    #[test]
    fn the_mask_tiles() {
        // The same noise at u and u + 1 (tileable): the lattice wraps.
        for i in 0..16 {
            let u = i as f32 / 16.0;
            assert!((value_noise(u, 0.3, 5) - value_noise(u + 1.0, 0.3, 5)).abs() < 1e-5);
        }
    }
}
