//! 3-D rain (`--rain-3d`): the 3D Rain mod's feature rebuilt natively, from the game's
//! own weather state and with no game assets of its own.
//!
//! - **A volume of streaks around the camera.** A fixed budget of streaks ([`STREAKS`] at
//!   density 1, at most [`MAX_STREAKS`]) lives in a box that travels with the main camera
//!   ([`WorldCamera`]): a streak's place is stored as an offset from the camera and wrapped
//!   into the box each frame, so the budget covers every view and a long walk costs
//!   nothing. Streaks fall at their own speed and drift with the weather's wind
//!   (`crate::tree::wind_vector`, the same wind the trees and grass fires take), with a
//!   slow gust on top.
//! - **One batched draw, not one entity a drop.** Every streak (and every splash ring) is
//!   a camera-facing quad in one [`Mesh`] with one unlit [`StandardMaterial`]; the quads
//!   are rebuilt on the CPU each frame (the simulation is CPU, the drawing is GPU: see
//!   docs/design/rain3d.md §2). UVs point into a small procedural atlas (a soft vertical
//!   streak and a soft ring), so no game texture is needed and the mod's own look is not
//!   copied.
//! - **Only while the weather precipitates.** The strength ramps from the shaded weather's
//!   `WTHR` DATA: a weather precipitates when it is `Rainy` or names a precipitation model
//!   (`MODL`), and its own begin-fade-in / end-fade-out bytes decide when, through the
//!   same [`crate::weather_fx::fade_share_on`] rule the precipitation model follows
//!   [documented: docs/spec-notes/world.md §6]. Outdoors only. A change ramps the strength
//!   over [`RAMP_SECONDS`] so the fade byte does not pop.
//! - **Splashes where it lands.** Once a frame a few impacts are sampled on a disc around
//!   the camera. An impact over visible water becomes a drop in the water's own ripple
//!   simulation ([`crate::ripples::Ripples::splash`]) — the height field the water shader
//!   already reads — and no ring is drawn there. An impact on land is a small expanding
//!   ring quad at the landscape height, sampled through the same LAND record the terrain
//!   and grass use.
//! - **Off by default.** [`Rain3dPlugin`] is only added with `--rain-3d`; without it no
//!   system, resource, asset or random draw of this module exists, and the frame is
//!   bit-identical (docs/design/rain3d.md §3). The hard cap is [`MAX_STREAKS`] streaks +
//!   [`MAX_SPLASHES`] rings, whatever the density.
//! - **Known limits** [design]: no roof/occlusion test (rain falls through a porch roof,
//!   as camera-relative volumes of this kind do), and the game data does not distinguish
//!   rain from snow in `WTHR` DATA beyond the classification byte, so every precipitating
//!   weather draws as rain [hypothesis].
//!
//! Log: `rain3d: rain on …` / `rain3d: rain off`, and at exit `rain3d summary: …`.
//! Test aid `NVF_PLAY_TEST_WEATHER_FX=precip=<model>` makes every weather precipitate for
//! the 3-D rain too (the same aid `weather_fx.rs` reads; a screenshot without a rainy
//! weather can use it).

use crate::fly::WorldCamera;
use crate::particles::camera_basis;
use crate::sky::SkyState;
use crate::water::DistantWater;
use crate::weather_fx::{fade_share_on, Role};
use crate::world::{bevy_to_game, WorldData, WorldRoot};
use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, MeshVertexAttribute, VertexAttributeValues};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use fnv_combat::rng::SplitMix;
use fnv_world::cell_of;
use fnv_world::sky::{smoothstep, Weather};
use nvf_assets::particles::particle_mesh;
use std::collections::HashMap;
use std::sync::Arc;

/// Streaks at density 1 (`--rain-3d-density 1`), the default [chosen: enough drops in the
/// air for a heavy shower without the volume reading as a fog of quads].
pub const STREAKS: f32 = 4000.0;
/// The hard cap on streaks, whatever the density [design: a typo or a large density must
/// not cost the 120 fps target; at the cap the geometry is still one draw call].
pub const MAX_STREAKS: usize = 8000;
/// The density the command line may ask for, at most (2 = [`MAX_STREAKS`]).
pub const MAX_DENSITY: f32 = MAX_STREAKS as f32 / STREAKS;

/// Half the box's width and depth around the camera (game units; a unit is roughly 1.4 cm,
/// so this is about 29 m of rain) [chosen].
const HALF_XZ: f32 = 2048.0;
/// The box's top above and bottom below the camera (game units) [chosen: the bottom stays
/// near the ground under a standing player, the top covers looking up].
const ABOVE: f32 = 900.0;
const BELOW: f32 = 350.0;
/// A streak is invisible nearer than this and full from the second value (units from the
/// camera), so a drop just in front of the lens is not a screen-filling quad [chosen].
const NEAR_FADE: (f32, f32) = (40.0, 180.0);
/// The same at the far side of the box [chosen].
const FAR_FADE: (f32, f32) = (1400.0, 2048.0);
/// A streak's length and width (game units) and its fall speed (units a second), each drawn
/// per streak in the range [chosen: rain streaks read at about a 20 ms exposure].
const LENGTH: (f32, f32) = (16.0, 42.0);
const WIDTH: (f32, f32) = (1.6, 2.8);
const FALL: (f32, f32) = (850.0, 1450.0);
/// The wind's speed at wind strength 1 (`WTHR` DATA wind 255), units a second [chosen: a
/// 50/255 breeze tilts a streak by about 8°].
const WIND_UNITS: f32 = 700.0;

/// The rain's opacity at full strength, and a streak's own share of it [chosen].
const STREAK_ALPHA: f32 = 0.32;
const SPLASH_ALPHA: f32 = 0.5;

/// Seconds the strength takes to go from 0 to 1 or back when the weather's fade byte
/// changes [chosen: short enough to follow a weather change, long enough not to pop].
const RAMP_SECONDS: f32 = 0.4;

/// Impacts sampled a second at density 1, the most alive at once, their life (seconds) and
/// the most made in one frame [chosen; the cap holds the ring geometry under
/// [`MAX_SPLASHES`] quads].
const SPLASHES_PER_SECOND: f32 = 120.0;
pub const MAX_SPLASHES: usize = 256;
const SPLASH_LIFE: f32 = 0.5;
const SPLASHES_PER_FRAME: usize = 8;
/// The disc around the camera impacts are sampled on (game units) and the farthest the
/// ground may be above or below the camera for a splash to show [chosen].
const SPLASH_RADIUS: f32 = 1200.0;
const SPLASH_MAX_DROP: f32 = 500.0;
/// A ring's radius when it starts and when it has grown fully (game units) [chosen].
const SPLASH_SIZE: (f32, f32) = (4.0, 14.0);
/// How far above the sampled surface a ring is drawn, so it does not z-fight the ground
/// (game units) [chosen].
const SPLASH_LIFT: f32 = 1.5;
/// Land cells whose heights are kept for splash sampling [design: a disc of
/// [`SPLASH_RADIUS`] touches at most four 4096-unit cells].
const LAND_CELLS: usize = 48;

/// The atlas: left half a soft vertical streak, right half a soft ring, both white so the
/// vertices tint them [design].
const ATLAS: u32 = 128;

/// The atlas half-width: the streak's U range and the ring's.
const STREAK_U: (f32, f32) = (0.004, 0.496);
const RING_U: (f32, f32) = (0.504, 0.996);

/// Marks the run as drawing 3-D rain: `weather_fx.rs` leaves the weather's precipitation
/// model undrawn while it exists (the streaks replace its planes).
#[derive(Resource)]
pub struct NativeRain;

/// One drop: where it is relative to the camera (game units), how it falls, its length,
/// width and share of the opacity.
struct Streak {
    offset: Vec3,
    fall: f32,
    len: f32,
    width: f32,
    alpha: f32,
}

/// One ground splash ring: where it is (game units), its age and life, and its full radius.
struct Splash {
    at: Vec3,
    age: f32,
    life: f32,
    size: f32,
}

/// The scratch geometry and the mesh's previous set, exchanged each frame so neither
/// allocation is dropped (as `particles.rs` does for its systems).
#[derive(Default)]
struct Geometry {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    colors: Vec<[f32; 4]>,
    indices: Vec<u32>,
}

impl Geometry {
    fn clear(&mut self) {
        self.positions.clear();
        self.normals.clear();
        self.uvs.clear();
        self.colors.clear();
        self.indices.clear();
    }

    /// A quad, its corners in the order the indices expect (`a` top left, `b` top right,
    /// `c` bottom right, `d` bottom left), with one colour and the atlas rectangle.
    fn quad(&mut self, a: Vec3, b: Vec3, c: Vec3, d: Vec3, colour: [f32; 4], uv: [f32; 4]) {
        let base = self.positions.len() as u32;
        self.positions.extend([a.to_array(), b.to_array(), c.to_array(), d.to_array()]);
        self.uvs.extend([[uv[0], uv[3]], [uv[1], uv[3]], [uv[1], uv[2]], [uv[0], uv[2]]]);
        self.colors.extend([colour; 4]);
        self.indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// Put the built attributes in the mesh, keeping both sets' allocations (the mesh's
    /// previous vectors become the scratch ones).
    fn flush(&mut self, mesh: &mut Mesh, back: Vec3) {
        self.normals.clear();
        self.normals.resize(self.positions.len(), back.to_array());
        swap_attribute(mesh, Mesh::ATTRIBUTE_POSITION, &mut self.positions, |v| match v {
            VertexAttributeValues::Float32x3(v) => Some(v),
            _ => None,
        });
        swap_attribute(mesh, Mesh::ATTRIBUTE_NORMAL, &mut self.normals, |v| match v {
            VertexAttributeValues::Float32x3(v) => Some(v),
            _ => None,
        });
        swap_attribute(mesh, Mesh::ATTRIBUTE_UV_0, &mut self.uvs, |v| match v {
            VertexAttributeValues::Float32x2(v) => Some(v),
            _ => None,
        });
        swap_attribute(mesh, Mesh::ATTRIBUTE_COLOR, &mut self.colors, |v| match v {
            VertexAttributeValues::Float32x4(v) => Some(v),
            _ => None,
        });
        if let Some(Indices::U32(values)) = mesh.indices_mut() {
            std::mem::swap(values, &mut self.indices);
        } else {
            mesh.insert_indices(Indices::U32(std::mem::take(&mut self.indices)));
        }
    }
}

/// Swap `scratch` with the mesh's `attribute` when `values` finds a vector of its type
/// there; otherwise insert it (taking its storage).
fn swap_attribute<T>(mesh: &mut Mesh, attribute: MeshVertexAttribute, scratch: &mut Vec<T>, values: fn(&mut VertexAttributeValues) -> Option<&mut Vec<T>>)
where
    Vec<T>: Into<VertexAttributeValues>,
{
    if let Some(v) = mesh.attribute_mut(attribute.id).and_then(values) {
        std::mem::swap(v, scratch);
        return;
    }
    mesh.insert_attribute(attribute, std::mem::take(scratch));
}

/// LAND heights by cell, for the ground a splash lands on [design: `load_land` re-reads
/// the record, so the heights are kept per cell while the camera stays nearby].
#[derive(Default)]
struct Lands {
    world: u32,
    cells: HashMap<(i32, i32), Option<Arc<Vec<f32>>>>,
}

impl Lands {
    fn height(&mut self, world: &WorldData, g: (i32, i32), x: f32, y: f32) -> Option<f32> {
        if self.world != world.world {
            self.world = world.world;
            self.cells.clear();
        }
        if self.cells.len() > LAND_CELLS {
            self.cells.retain(|k, _| (k.0 - g.0).abs() <= 2 && (k.1 - g.1).abs() <= 2);
        }
        let heights = self.cells.entry(g).or_insert_with(|| {
            let land = world.grid_cell(g)?.land?;
            let land = fnv_world::land::load_land(world.lo, world.schema, land, fnv_world::land::LandParts::HEIGHTS)?;
            Some(Arc::new(land.heights?))
        });
        let h = heights.as_ref()?;
        let ox = g.0 as f32 * fnv_world::CELL_SIZE;
        let oy = g.1 as f32 * fnv_world::CELL_SIZE;
        Some(fnv_world::grass::height_at(h.as_slice(), x - ox, y - oy))
    }
}

#[derive(Resource)]
pub struct Rain3d {
    density: f32,
    /// `NVF_PLAY_TEST_WEATHER_FX=precip=<model>`: every weather precipitates.
    forced: bool,
    streaks: Vec<Streak>,
    splashes: Vec<Splash>,
    /// Fractional impacts owed.
    acc: f32,
    /// 0..1, eased toward the weather's own rule ([`precipitation_strength`]).
    strength: f32,
    was_raining: bool,
    rng: SplitMix,
    lands: Lands,
    texture: Handle<Image>,
    material: Handle<StandardMaterial>,
    mesh: Handle<Mesh>,
    entity: Option<Entity>,
    geometry: Geometry,
    /// Exit figures.
    rings: u64,
    water_impacts: u64,
    most_splashes: usize,
}

impl Rain3d {
    fn new(density: f32, forced: bool) -> Rain3d {
        let count = (STREAKS * density).round().clamp(0.0, MAX_STREAKS as f32) as usize;
        let mut rng = SplitMix(0x5241_494E_3344_0001);
        let spread = |rng: &mut SplitMix, a: f32, b: f32| a + (b - a) * rng.next_f32();
        let streaks = (0..count)
            .map(|_| Streak {
                offset: Vec3::new(spread(&mut rng, -HALF_XZ, HALF_XZ), spread(&mut rng, -HALF_XZ, HALF_XZ), spread(&mut rng, -BELOW, ABOVE)),
                fall: spread(&mut rng, FALL.0, FALL.1),
                len: spread(&mut rng, LENGTH.0, LENGTH.1),
                width: spread(&mut rng, WIDTH.0, WIDTH.1),
                alpha: spread(&mut rng, 0.7, 1.0),
            })
            .collect();
        Rain3d {
            density,
            forced,
            streaks,
            splashes: Vec::new(),
            acc: 0.0,
            strength: 0.0,
            was_raining: false,
            rng,
            lands: Lands::default(),
            texture: Handle::default(),
            material: Handle::default(),
            mesh: Handle::default(),
            entity: None,
            geometry: Geometry::default(),
            rings: 0,
            water_impacts: 0,
            most_splashes: 0,
        }
    }
}

/// 3-D rain, opt-in: `Rain3dPlugin(None)` (the default, `--rain-3d` absent) adds nothing at
/// all; `Some(density)` adds the systems, the marker and the assets.
pub struct Rain3dPlugin(pub Option<f32>);

impl Plugin for Rain3dPlugin {
    fn build(&self, app: &mut App) {
        let Some(given) = self.0 else { return };
        let density = if given.is_finite() { given.clamp(0.0, MAX_DENSITY) } else { 1.0 };
        if density != given {
            warn!("rain3d: --rain-3d-density {given} clamped to {density} (at most {MAX_DENSITY}, {MAX_STREAKS} streaks)");
        }
        app.insert_resource(NativeRain)
            .insert_resource(Rain3d::new(density, test_aid_forced()))
            .add_systems(Startup, setup)
            .add_systems(Update, update)
            .add_systems(Last, report);
    }
}

/// Whether the test aid asks for precipitation in every weather. The aid is
/// `NVF_PLAY_TEST_WEATHER_FX=precip=<model>` (`weather_fx.rs` reads the same string for the
/// model it draws); here only its presence matters.
fn test_aid_forced() -> bool {
    let forced = std::env::var("NVF_PLAY_TEST_WEATHER_FX")
        .is_ok_and(|v| v.split(';').any(|p| p.split_once('=').is_some_and(|(k, v)| k.trim() == "precip" && !v.trim().is_empty())));
    if forced {
        info!("rain3d: test aid NVF_PLAY_TEST_WEATHER_FX=precip: rain falls in every weather");
    }
    forced
}

fn setup(mut rain: ResMut<Rain3d>, mut images: ResMut<Assets<Image>>, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>) {
    let rain = &mut *rain;
    rain.texture = images.add(atlas());
    rain.mesh = meshes.add(particle_mesh());
    rain.material = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        base_color_texture: Some(rain.texture.clone()),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        double_sided: true,
        cull_mode: None,
        // The rain is near the camera and must not take the far fog's colour twice; the
        // distance fade in its vertices is the only fade.
        fog_enabled: false,
        // Splash rings lie on the ground: keep them off a z-fight with it.
        depth_bias: -20.0,
        ..default()
    });
    info!(
        "rain3d: {} streaks at density {}, at most {MAX_STREAKS} streaks and {MAX_SPLASHES} splash rings, one draw call",
        rain.streaks.len(),
        rain.density
    );
}

/// The procedural atlas: a soft streak in the left half, a soft ring in the right half.
fn atlas() -> Image {
    let n = ATLAS;
    let half = (n / 2) as f32;
    let mut data = Vec::with_capacity((n * n * 4) as usize);
    for y in 0..n {
        for x in 0..n {
            let a = if x < n / 2 {
                // Across the streak: a soft falloff; along it: the head and tail fade.
                let u = (x as f32 + 0.5) / half;
                let v = (y as f32 + 0.5) / n as f32;
                let across = (1.0 - (2.0 * u - 1.0).abs()).powf(1.8);
                let along = smoothstep(0.0, 0.18, v) * (1.0 - smoothstep(0.82, 1.0, v));
                across * along
            } else {
                // The ring: a band around r = 0.72, wider across than along it looks.
                let dx = (x as f32 + 0.5 - 3.0 * half / 2.0) / (half / 2.0);
                let dy = (y as f32 + 0.5 - half) / half;
                let r = (dx * dx + dy * dy).sqrt();
                let band = (-((r - 0.72) / 0.18).powi(2)).exp();
                band * (1.0 - smoothstep(0.9, 1.0, r))
            };
            data.extend([255, 255, 255, (a.clamp(0.0, 1.0) * 255.0).round() as u8]);
        }
    }
    let mut image = Image::new(
        Extent3d { width: n, height: n, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        ..ImageSamplerDescriptor::linear()
    });
    image
}

/// The share of the rain to draw now (0 or 1 before the ramp): a shown weather
/// precipitates when it is `Rainy` or names a precipitation model, and its precipitation
/// begin-fade-in / end-fade-out bytes say whether the change has reached it. Both weathers
/// of a change are read, so rain carries on from the outgoing one until its end fade.
pub fn precipitation_strength(sky: &SkyState, forced: bool) -> f32 {
    if sky.interior.is_some() {
        return 0.0;
    }
    let precipitates = |w: &Weather| w.rainy || w.precipitation_model.is_some() || forced;
    let current = if sky.prev.is_some() { Role::Incoming(sky.fade) } else { Role::Steady };
    if precipitates(&sky.weather) && fade_share_on(sky.weather.precipitation_fade, current) {
        return 1.0;
    }
    if let Some(prev) = &sky.prev {
        if precipitates(prev) && fade_share_on(prev.precipitation_fade, Role::Outgoing(sky.fade)) {
            return 1.0;
        }
    }
    0.0
}

/// `v` wrapped into −h..h.
fn wrap(v: f32, h: f32) -> f32 {
    (v + h).rem_euclid(2.0 * h) - h
}

/// The vertical offset wrapped into the box's −BELOW..ABOVE.
fn wrap_z(v: f32) -> f32 {
    (v + BELOW).rem_euclid(ABOVE + BELOW) - BELOW
}

/// A display value as the linear value the material multiplies its texture by.
fn linear(v: f32) -> f32 {
    Color::srgb(v.max(0.0), 0.0, 0.0).to_linear().red
}

/// The rain's display colour by daylight: pale grey by day, a dark blue-grey at night
/// [chosen].
fn tint(sky: &SkyState) -> [f32; 3] {
    let day = sky.daylight();
    let night = [0.30, 0.33, 0.40];
    let noon = [0.78, 0.83, 0.90];
    [night[0] + (noon[0] - night[0]) * day, night[1] + (noon[1] - night[1]) * day, night[2] + (noon[2] - night[2]) * day]
}

/// Where a splash lands: the water's surface when the point is over visible water (with
/// its WATR), else the LAND height. `None`: no drawn ground there.
fn surface_at(world: &WorldData, water: &mut DistantWater, lands: &mut Lands, x: f32, y: f32) -> Option<(f32, Option<u32>)> {
    let g = cell_of(x, y);
    let wet = water.wet_area(world, g).is_some_and(|[x0, y0, x1, y1]| (x0..=x1).contains(&x) && (y0..=y1).contains(&y));
    let surface = if wet { water.surface(world, g) } else { None };
    if let Some((h, wid)) = surface {
        return Some((h, Some(wid)));
    }
    Some((lands.height(world, g, x, y)?, None))
}

/// Queue a sampled water impact in the ripple grid, when it exists: whether it did.
fn water_impact(ripples: &mut Option<ResMut<crate::ripples::Ripples>>, x: f32, y: f32, wid: u32) -> bool {
    let Some(ripples) = ripples.as_deref_mut() else { return false };
    ripples.splash(x, y, wid);
    true
}

#[allow(clippy::too_many_arguments)]
fn update(
    mut commands: Commands,
    time: Res<Time>,
    sky: Res<SkyState>,
    world: Option<Res<WorldData>>,
    mut water: ResMut<DistantWater>,
    mut ripples: Option<ResMut<crate::ripples::Ripples>>,
    camera: Query<&Transform, WorldCamera>,
    root: Query<(Entity, &Transform), With<WorldRoot>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut visibility: Query<&mut Visibility>,
    mut rain: ResMut<Rain3d>,
) {
    let rain = &mut *rain;
    let (Ok(cam), Ok((root_e, root_t))) = (camera.single(), root.single()) else {
        hide(rain, &mut visibility);
        return;
    };
    let dt = time.delta_secs().clamp(0.0, 0.1);
    let target = precipitation_strength(&sky, rain.forced);
    let step = dt / RAMP_SECONDS;
    rain.strength += (target - rain.strength).clamp(-step, step);
    if (target - rain.strength).abs() < 1e-3 {
        rain.strength = target;
    }
    if rain.strength > 0.0 && !rain.was_raining {
        rain.was_raining = true;
        info!(
            "rain3d: rain on in {} ({} streaks, {} splash rings at most, wind {})",
            sky.weather.edid.as_deref().unwrap_or("?"),
            rain.streaks.len(),
            MAX_SPLASHES,
            sky.weather.wind_speed
        );
    } else if rain.strength <= 0.0 && rain.was_raining {
        rain.was_raining = false;
        info!("rain3d: rain off");
    }
    if rain.strength <= 0.0 {
        rain.splashes.clear();
        rain.acc = 0.0;
        hide(rain, &mut visibility);
        return;
    }

    // The rain volume follows the camera; the drops are offsets from it.
    if let Some(ripples) = ripples.as_deref_mut() {
        // 3-D rain draws the rain: the ripple grid takes its sampled impacts instead of
        // synthesizing drops of its own (bit-identical without the flag).
        ripples.set_rain3d();
    }
    let cam_t = *cam;
    let cam = bevy_to_game(cam_t.translation);
    let to_game = root_t.compute_affine().inverse();
    let basis = camera_basis(cam_t.rotation, &to_game);
    let wind = crate::tree::wind_vector(&sky);
    let gust = 1.0 + 0.25 * (time.elapsed_secs() * 0.37).sin() + 0.12 * (time.elapsed_secs() * 0.91 + 1.7).sin();
    let wind = Vec3::new(wind.x, -wind.z, 0.0) * (WIND_UNITS * wind.w * gust.max(0.0));
    for s in &mut rain.streaks {
        s.offset += (wind - Vec3::Z * s.fall) * dt;
        s.offset.x = wrap(s.offset.x, HALF_XZ);
        s.offset.y = wrap(s.offset.y, HALF_XZ);
        s.offset.z = wrap_z(s.offset.z);
    }

    // Splashes: a few impacts a frame on the ground or the water around the camera.
    if let Some(world) = world.as_deref() {
        rain.acc = (rain.acc + SPLASHES_PER_SECOND * rain.density * rain.strength * dt).min(SPLASHES_PER_FRAME as f32);
        let mut made = 0;
        while rain.acc >= 1.0 && made < SPLASHES_PER_FRAME {
            rain.acc -= 1.0;
            made += 1;
            if rain.splashes.len() >= MAX_SPLASHES {
                rain.acc = 0.0;
                break;
            }
            let angle = rain.rng.next_f32() * std::f32::consts::TAU;
            let radius = SPLASH_RADIUS * rain.rng.next_f32().sqrt();
            let (x, y) = (cam.x + angle.cos() * radius, cam.y + angle.sin() * radius);
            let Some((h, wid)) = surface_at(world, &mut water, &mut rain.lands, x, y) else { continue };
            if (h - cam.z).abs() > SPLASH_MAX_DROP {
                continue;
            }
            if let Some(wid) = wid {
                // On water the splash is the height-field's own drop, not a ring.
                rain.water_impacts += water_impact(&mut ripples, x, y, wid) as u64;
            } else {
                let size = SPLASH_SIZE.0 + (SPLASH_SIZE.1 - SPLASH_SIZE.0) * rain.rng.next_f32();
                let life = SPLASH_LIFE * (0.8 + 0.4 * rain.rng.next_f32());
                rain.splashes.push(Splash { at: Vec3::new(x, y, h), age: 0.0, life, size });
                rain.rings += 1;
            }
        }
    }
    for s in &mut rain.splashes {
        s.age += dt;
    }
    rain.splashes.retain(|s| s.age < s.life);
    rain.most_splashes = rain.most_splashes.max(rain.splashes.len());

    // Build the one mesh: streaks as velocity-aligned camera-facing quads, splashes as
    // rings lying on the ground.
    let [right, up, back] = basis;
    let colour = tint(&sky);
    let colour = [linear(colour[0]), linear(colour[1]), linear(colour[2])];
    let g = &mut rain.geometry;
    g.clear();
    {
        let capacity = rain.streaks.len() + rain.splashes.len();
        g.positions.reserve(capacity * 4);
        g.uvs.reserve(capacity * 4);
        g.colors.reserve(capacity * 4);
        g.indices.reserve(capacity * 6);
    }
    for s in &rain.streaks {
        let p = cam + s.offset;
        let distance = s.offset.length();
        let alpha = STREAK_ALPHA * s.alpha * rain.strength * smoothstep(NEAR_FADE.0, NEAR_FADE.1, distance) * (1.0 - smoothstep(FAR_FADE.0, FAR_FADE.1, distance));
        if alpha <= 0.002 {
            continue;
        }
        // The fall direction, as the camera sees it: a drop falling away from the view is
        // shorter, one crossing it is at its full length.
        let v = wind - Vec3::Z * s.fall;
        let along = v - back * v.dot(back);
        let foreshorten = (along.length() / v.length().max(1e-3)).clamp(0.15, 1.0);
        let axis = along.normalize_or(up);
        let half = 0.5 * s.len * foreshorten;
        let side = right * (0.5 * s.width);
        let (a, b) = (p - axis * half, p + axis * half);
        g.quad(a - side, a + side, b + side, b - side, [colour[0], colour[1], colour[2], alpha], [STREAK_U.0, STREAK_U.1, 0.0, 1.0]);
    }
    for s in &rain.splashes {
        let t = (s.age / s.life).clamp(0.0, 1.0);
        let radius = s.size * (0.35 + 0.65 * t);
        let alpha = SPLASH_ALPHA * rain.strength * (1.0 - t) * (1.0 - t);
        if alpha <= 0.002 {
            continue;
        }
        let at = s.at + Vec3::Z * SPLASH_LIFT;
        g.quad(
            Vec3::new(at.x - radius, at.y - radius, at.z),
            Vec3::new(at.x + radius, at.y - radius, at.z),
            Vec3::new(at.x + radius, at.y + radius, at.z),
            Vec3::new(at.x - radius, at.y + radius, at.z),
            [colour[0], colour[1], colour[2], alpha],
            [RING_U.0, RING_U.1, 0.0, 1.0],
        );
    }

    // Make the render entity on first use, under the world root (game space).
    let entity = match rain.entity {
        Some(e) => e,
        None => {
            let e = commands
                .spawn((
                    Mesh3d(rain.mesh.clone()),
                    MeshMaterial3d(rain.material.clone()),
                    Transform::IDENTITY,
                    Visibility::Hidden,
                    NoFrustumCulling,
                    NotShadowCaster,
                    ChildOf(root_e),
                ))
                .id();
            rain.entity = Some(e);
            e
        }
    };
    if let Some(mut mesh) = meshes.get_mut(&rain.mesh) {
        g.flush(&mut mesh, back);
    }
    let Ok(mut v) = visibility.get_mut(entity) else { return };
    if *v != Visibility::Inherited {
        *v = Visibility::Inherited;
    }
}

/// Hide the rain's one render entity (no rain, or no world root to hang it from).
fn hide(rain: &mut Rain3d, visibility: &mut Query<&mut Visibility>) {
    let Some(e) = rain.entity else { return };
    let Ok(mut v) = visibility.get_mut(e) else { return };
    if *v != Visibility::Hidden {
        *v = Visibility::Hidden;
    }
}

fn report(mut exit: MessageReader<AppExit>, rain: Option<Res<Rain3d>>) {
    let Some(rain) = rain else { return };
    if exit.read().last().is_some() {
        info!(
            "rain3d summary: {} streaks, {} splash rings drew, {} impacts went to the water ripples, at most {} rings at once",
            rain.streaks.len(),
            rain.rings,
            rain.water_impacts,
            rain.most_splashes
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_stay_in_the_box() {
        for v in [-1e6, -5000.0, -HALF_XZ, -3.0, 0.0, 3.0, HALF_XZ, 1e6] {
            let w = wrap(v, HALF_XZ);
            assert!((-HALF_XZ..HALF_XZ).contains(&w), "{v} -> {w}");
        }
        for v in [-1e6, -BELOW, -3.0, 0.0, ABOVE, 1e6] {
            let w = wrap_z(v);
            assert!((-BELOW..ABOVE).contains(&w), "{v} -> {w}");
        }
    }

    #[test]
    fn only_a_precipitating_weather_rains() {
        let mut sky = SkyState::default();
        assert_eq!(precipitation_strength(&sky, false), 0.0);
        sky.weather.rainy = true;
        assert_eq!(precipitation_strength(&sky, false), 1.0);
        // A model alone precipitates too (a mod's snow weather).
        sky.weather.rainy = false;
        sky.weather.precipitation_model = Some("meshes\\rain.nif".into());
        assert_eq!(precipitation_strength(&sky, false), 1.0);
        // The test aid makes any weather precipitate.
        sky.weather.precipitation_model = None;
        assert_eq!(precipitation_strength(&sky, false), 0.0);
        assert_eq!(precipitation_strength(&sky, true), 1.0);
    }

    #[test]
    fn a_change_keeps_the_rain_until_the_end_fade() {
        let mut sky = SkyState::default();
        sky.weather.rainy = false;
        sky.prev = Some(Weather { rainy: true, precipitation_fade: (128, 128), ..default() });
        sky.fade = 0.3;
        assert_eq!(precipitation_strength(&sky, false), 1.0, "the outgoing rain is still falling");
        sky.fade = 0.8;
        assert_eq!(precipitation_strength(&sky, false), 0.0, "past its end fade out");
        // The incoming weather's rain begins at its own fade-in byte.
        sky.prev = None;
        sky.weather = Weather { rainy: true, precipitation_fade: (128, 128), ..default() };
        sky.fade = 1.0;
        assert_eq!(precipitation_strength(&sky, false), 1.0);
    }

    #[test]
    fn the_atlas_has_both_halves() {
        let image = atlas();
        assert_eq!(image.data.as_ref().unwrap().len(), (ATLAS * ATLAS * 4) as usize);
        let alpha = |x: u32, y: u32| image.data.as_ref().unwrap()[((y * ATLAS + x) * 4 + 3) as usize];
        // A pixel at the streak's centre line and the ring's right side is opaque; the
        // corners are clear.
        assert!(alpha(ATLAS / 4, ATLAS / 2) > 200);
        assert!(alpha(0, 0) < 16 && alpha(ATLAS - 1, 0) < 16);
        assert!((0..ATLAS).any(|x| alpha(x, ATLAS / 2) > 100 && x > ATLAS / 2), "the ring has ink");
    }
}
