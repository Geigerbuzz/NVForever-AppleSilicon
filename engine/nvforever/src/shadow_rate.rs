//! The sun shadows' modes: the settings page's Sun shadows row, Off, Performance or Quality
//! ([`SUN_SHADOW_CHOICES`]; `--sun-shadows[=quality|performance]`). Performance trades some
//! of the shadows' precision for frame time. [engineering choices throughout; the original
//! casts no sun shadows from the world at all, docs/spec-notes/world.md §6]
//!
//! Performance:
//! - **The sun's shadow maps are drawn every other frame** ([`hold_cascades`]). On the
//!   frames between, the lights keep the cascades of the frame that drew them, so the maps
//!   and the matrices they are read with still belong together, and the renderer is told
//!   to leave the maps alone (`bevy::pbr::HoldDirectionalShadowMaps`, a patched bevy_pbr:
//!   engine/vendor/README.md). The shadow of a moving thing then moves at half the frame
//!   rate, and on a fast turn the edge of the view can lie outside a held cascade for one
//!   frame (unshadowed).
//! - **The sun's shadows are split over [`PERFORMANCE_CASCADES`] cascades** instead of
//!   `sky::DEFAULT_SHADOW_CASCADES` ([`set_performance`]): one shadow view less, and shadows
//!   near the camera a little coarser.
//! - **The first-person arms have no shade light of their own** (player_body.rs
//!   `shade_light`): another shadow view less; the arms take the sun's own shadows.
//! - **A tree casts a shadow only within [`TREE_SHADOW_NEAR`] of the camera**
//!   ([`far_trees_cast_no_shadow`]): a SpeedTree tree is thousands of leaf cards and a
//!   finely divided trunk in every cascade it falls in.
//! - **One cascade a frame, and the far one less often** ([`PERFORMANCE_SCHEDULE`]): the
//!   nearest every other frame as before, the far one every fourth, and never both in one
//!   frame. 0.75 maps a frame instead of 1.0, and no frame carries two shadow passes --
//!   which is the frame that cannot be paid for when the frame time is held to the refresh
//!   rate.
//! - **The shadow budget** ([`ShadowBudget`], `--shadow-budget=<casters>,<passes>`): a cap
//!   on the cascade maps a frame draws and on the casters one cascade draws, the nearest
//!   cascades kept and the farthest and smallest left out.
//!
//! A cascade whose light-space grid is unchanged since the frame that drew its map is
//! held in both modes ([`UNCHANGED_HOLD_FRAMES`]): Bevy rounds each cascade's grid to whole
//! texels, so an unchanged grid would redraw the same depth. A still camera therefore
//! draws the far cascade once and no more, and a cascade the fog hides is held as well
//! ([`fog_hidden`]).
//!
//! Quality (the default) draws the sun's cascades on a staggered schedule
//! ([`STAGGERED_SCHEDULE`], #2063): the nearest every frame, the middle one every other
//! frame, the farthest every fourth, each held cascade kept with the matrices of the frame
//! that drew it, as above. A moving thing's shadow in the far cascades moves at a half or
//! a quarter of the frame rate, and on a fast turn the edge of the view can lie outside a
//! held cascade for up to three frames. Test aid `NVF_PLAY_TEST_SHADOW_STAGGER=0` draws
//! every cascade every frame.
//!
//! What it costs and saves: docs/design/frame-rate-audit.md §9 and §10.

use crate::fly::FlyCam;
use crate::sky::SkyState;
use bevy::ecs::entity::EntityHashMap;
use bevy::light::cascade::Cascade;
use bevy::light::{CascadeShadowConfig, Cascades, DirectionalLight, DirectionalLightShadowMap, NotShadowCaster, SimulationLightSystems};
use bevy::pbr::HoldDirectionalShadowMaps;
use bevy::prelude::*;
use bevy::render::extract_resource::ExtractResourcePlugin;

/// `--sun-shadows=<mode>`: the mode the sun's shadows start in.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SunShadowMode {
    Performance,
    Quality,
}

/// Sun shadows in Performance mode (the Sun shadows row's middle choice).
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub struct PerformanceShadows(pub bool);

/// The Sun shadows row's choices, in the order the page and the INI keep them: an INI
/// written while the row was an on/off switch (`1`/`0`) reads back as Quality or Off.
pub const SUN_SHADOW_CHOICES: [&str; 3] = ["Off", "Performance", "Quality"];

/// The Sun shadows row's choice now (an index into [`SUN_SHADOW_CHOICES`]).
pub fn sun_shadow_choice(world: &World) -> usize {
    if !world.resource::<SkyState>().shadows {
        0
    } else if world.resource::<PerformanceShadows>().0 {
        1
    } else {
        2
    }
}

/// Set the Sun shadows row's choice: Off turns the sun's shadows off and leaves the mode
/// as it was; Performance and Quality set the mode, then turn the shadows on.
pub fn set_sun_shadow_choice(world: &mut World, choice: usize) {
    if choice > 0 {
        let performance = choice == 1;
        if world.resource::<PerformanceShadows>().0 != performance {
            set_performance(world, performance);
        }
    }
    if world.resource::<SkyState>().shadows != (choice > 0) {
        crate::sky::set_shadows(world, choice > 0);
    }
}

/// The sun's cascades in Performance mode.
pub const PERFORMANCE_CASCADES: usize = 2;

/// With the switch on a tree casts a shadow within this distance of the camera (game
/// units: about 43 m), and again once it is back within [`TREE_SHADOW_BACK`] of it.
pub const TREE_SHADOW_NEAR: f32 = 3000.0;
pub const TREE_SHADOW_BACK: f32 = 2700.0;

/// Seconds between two looks at the trees.
const TREES_EVERY: f32 = 0.5;

/// The sun's cascade count Performance replaced when it was chosen (`--graphics-cycle
/// cascades=` or the default), given back with Quality.
#[derive(Resource, Default)]
struct CascadesBeforePerformance(Option<usize>);

/// Performance mode on or off now (the Sun shadows row): the cascade count changes with it.
/// Only the count is set here; graphics.rs `sun_shadow_quality` puts the cascades on the
/// sun as the hour wants them (the moon's single cascade by night).
pub fn set_performance(world: &mut World, on: bool) {
    let was = std::mem::replace(&mut world.resource_mut::<PerformanceShadows>().0, on);
    let current = world.resource::<SkyState>().shadow_cascades;
    let mut before = world.get_resource_or_init::<CascadesBeforePerformance>();
    let cascades = if on {
        if !was {
            before.0 = Some(current);
        }
        PERFORMANCE_CASCADES
    } else if was {
        before.0.take().unwrap_or(crate::sky::DEFAULT_SHADOW_CASCADES)
    } else {
        current
    };
    world.resource_mut::<SkyState>().shadow_cascades = cascades;
    let how = if on {
        "performance (maps drawn every other frame, trees cast near the camera only, no arms shade light)"
    } else if staggered() {
        "cascades on a staggered schedule (the nearest every frame, the farther ones less often)"
    } else {
        "drawn every frame"
    };
    info!("sun shadows: {how}, {cascades} cascades");
}

/// A part of a SpeedTree tree (tree.rs `spawn`).
#[derive(Component)]
pub struct TreePart;

/// Marks the tree parts whose `NotShadowCaster` [`far_trees_cast_no_shadow`] put there.
#[derive(Component)]
pub struct FarTreeNoShadow;

/// Whether a tree part `distance` from the camera casts a shadow with the switch on,
/// given whether it casts now.
pub fn tree_casts(distance: f32, casting_now: bool) -> bool {
    distance <= if casting_now { TREE_SHADOW_NEAR } else { TREE_SHADOW_BACK }
}

/// With the switch on, trees far from the camera cast no shadow (module notes); with it
/// off every tree casts again.
fn far_trees_cast_no_shadow(
    mut commands: Commands,
    on: Res<PerformanceShadows>,
    time: Res<Time<Real>>,
    cams: Query<&GlobalTransform, With<FlyCam>>,
    trees: Query<(Entity, &GlobalTransform, Has<FarTreeNoShadow>), With<TreePart>>,
    ours: Query<Entity, With<FarTreeNoShadow>>,
    mut since: Local<f32>,
) {
    if !on.0 {
        for e in &ours {
            commands.entity(e).try_remove::<(NotShadowCaster, FarTreeNoShadow)>();
        }
        return;
    }
    *since += time.delta_secs();
    if *since < TREES_EVERY && !on.is_changed() {
        return;
    }
    *since = 0.0;
    let Some(cam) = cams.iter().next().map(|t| t.translation()) else { return };
    for (e, at, held) in &trees {
        let casts = tree_casts(at.translation().distance(cam), !held);
        if casts && held {
            commands.entity(e).try_remove::<(NotShadowCaster, FarTreeNoShadow)>();
        } else if !casts && !held {
            commands.entity(e).try_insert((NotShadowCaster, FarTreeNoShadow));
        }
    }
}

/// How often each cascade's map is drawn, nearest cascade first: `(n, phase)` draws it on
/// the frames where (frame + phase) is a multiple of n; a cascade past the list is drawn
/// every frame. With the switch on, the nearest cascade every other frame, as it was, and
/// the far one every fourth -- the cadence Quality gives its own far cascade, and the one
/// map that holds almost every caster in the two-cascade split ([17, 874] of 891 at
/// Goodsprings, reports/exp-shadow-anomaly.md §1.1). The two are never drawn in the same
/// frame: 0.75 maps a frame at no more than one, where both together put two shadow passes,
/// their culling and their preparation into every other frame -- and a frame time held to
/// 120 a second is set by that frame, not by the mean.
pub const PERFORMANCE_SCHEDULE: [(u32, u32); 2] = [(2, 0), (4, 1)];
/// With the switch off: the nearest cascade every frame, the middle one every other frame
/// and the farthest every fourth, on different frames, so at most two are drawn in one
/// frame (1.75 of 3 on average). The farthest holds most of the casters (about 1,150 of
/// 1,400 at Goodsprings, 1,570 of 2,140 at Novac), and its shadows are the coarsest, so a
/// moving thing's shadow there moving at a quarter of the frame rate shows least.
pub const STAGGERED_SCHEDULE: [(u32, u32); 3] = [(1, 0), (2, 1), (4, 2)];

/// A cascade whose cascade matrices are the ones the frame that drew its map had is held
/// this many frames at most: its grid did not move (Bevy rounds it to whole texels,
/// `bevy_light` `cascade.rs` `calculate_cascade`), so drawing it again writes the same
/// depth in the same texels, but a caster that moves while the view does not still has to
/// be drawn: at 120 a second this is one draw in 66 ms.
pub const UNCHANGED_HOLD_FRAMES: u16 = 8;

/// The shadow budget: what the sun's shadows may draw in a frame, degrading gracefully --
/// the nearest cascades stay in and the farthest are held over to the next frame (their
/// maps and matrices kept, as for any held cascade), and within a cascade the smallest
/// casters go first (`crate::sky::sun_casters_over_slice`). `--shadow-budget=<casters>,
/// <passes>`; 0 for either is no cap, which is the default.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShadowBudget {
    /// Most casters drawn from one cascade; 0, no cap.
    pub casters: usize,
    /// Most cascade maps drawn in one frame; 0, no cap.
    pub passes: usize,
}

impl ShadowBudget {
    /// `casters,passes`; a part that is missing or not a number counts as 0 (no cap).
    pub fn parse(text: &str) -> Self {
        let number = |v: Option<&str>| v.and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0);
        let mut parts = text.split(',');
        Self { casters: number(parts.next()), passes: number(parts.next()) }
    }
}

/// Whether cascade `now` draws the same map as `kept`: the same light-space grid, with the
/// same matrices to read it with.
fn unchanged(now: &Cascade, kept: &Cascade) -> bool {
    now.clip_from_world == kept.clip_from_world && now.world_from_cascade == kept.world_from_cascade && now.clip_from_cascade == kept.clip_from_cascade
}

/// The cascades (a bit each, from the second on) whose every receiver the fog hides: at
/// the cascade's near boundary the fog is already opaque, so nothing in its slice changes
/// colour whether it is lit or shadowed, and drawing the cascade cannot change a pixel.
/// The first cascade is left alone: the arms' shade light and the ground at the player's
/// feet are in it.
fn fog_hidden(sky: &SkyState, config: &CascadeShadowConfig, count: usize) -> u32 {
    let range = sky.fog_override.map(|(near, far)| (near, far, 1.0)).or_else(|| sky.fog());
    let Some((near, far, strength)) = range else { return 0 };
    if strength < 0.99 {
        return 0;
    }
    let power = sky.fog_power();
    let overlap = 1.0 - config.overlap_proportion;
    (1..count.min(32))
        .filter(|&i| {
            let from = config.bounds.get(i - 1).map_or(config.minimum_distance, |bound| bound * overlap);
            crate::fog::share(near.max(0.0), far, power, from) >= 0.995
        })
        .fold(0, |mask, i| mask | 1 << i)
}

/// The cascades (a bit each, nearest first) whose maps frame `frame` keeps from an earlier
/// frame under `schedule`, of `cascades` cascades.
pub fn held_mask(schedule: &[(u32, u32)], frame: u64, cascades: usize) -> u32 {
    (0..cascades.min(32)).fold(0, |mask, i| {
        let (n, phase) = schedule.get(i).copied().unwrap_or((1, 0));
        if n > 1 && !(frame + u64::from(phase)).is_multiple_of(u64::from(n)) { mask | 1 << i } else { mask }
    })
}

/// Whether the cascades kept from earlier frames can stand in for this frame's: every
/// light has kept cascades for the same views, as many of them.
fn same_layout(kept: &EntityHashMap<Cascades>, now: &[(Entity, &Cascades)]) -> bool {
    !now.is_empty()
        && now.iter().all(|(light, cascades)| {
            kept.get(light).is_some_and(|k| {
                k.cascades.len() == cascades.cascades.len()
                    && cascades.cascades.iter().all(|(view, c)| k.cascades.get(view).is_some_and(|kc| kc.len() == c.len()))
            })
        })
}

/// Whether the cascades are drawn on a staggered schedule with the switch off (test aid
/// `NVF_PLAY_TEST_SHADOW_STAGGER=0` draws every cascade every frame).
fn staggered() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("NVF_PLAY_TEST_SHADOW_STAGGER").map_or(true, |v| v != "0"))
}

/// The cascades not drawn this frame ([`PERFORMANCE_SCHEDULE`], [`STAGGERED_SCHEDULE`]): give
/// the directional lights back those cascades as the frame that drew them had them, and
/// have the renderer keep their maps (module notes). Runs after Bevy has built this
/// frame's cascades and before it fits the lights' frusta to them.
#[allow(clippy::too_many_arguments)]
fn hold_cascades(
    on: Res<PerformanceShadows>,
    map: Option<Res<DirectionalLightShadowMap>>,
    sky: Option<Res<SkyState>>,
    budget: Res<ShadowBudget>,
    mut hold: ResMut<HoldDirectionalShadowMaps>,
    mut lights: Query<(Entity, &mut Cascades, Option<&CascadeShadowConfig>), With<DirectionalLight>>,
    mut kept: Local<EntityHashMap<Cascades>>,
    mut frame: Local<u64>,
    mut since: Local<Vec<u16>>,
    mut said: Local<Option<bool>>,
) {
    *frame = frame.wrapping_add(1);
    let schedule: &[(u32, u32)] = if on.0 {
        &PERFORMANCE_SCHEDULE
    } else if staggered() {
        &STAGGERED_SCHEDULE
    } else {
        &[]
    };
    let count = lights.iter().flat_map(|(_, c, _)| c.cascades.values().map(Vec::len)).max().unwrap_or(0);
    since.resize(count, 0);
    // A new map size makes new maps: nothing to keep.
    let same_maps = !map.is_some_and(|m| m.is_changed());
    let mut mask = held_mask(schedule, *frame, count);
    if mask != 0 && !(same_maps && same_layout(&kept, &lights.iter().map(|(e, c, _)| (e, c)).collect::<Vec<_>>())) {
        mask = 0;
    }
    if mask != 0 {
        // The cascades whose grid is the one the frame that drew them had, in every light and
        // view, and the ones the fog hides. Only with maps kept already: with no kept cascade
        // nothing can stand in.
        let mut same = u32::MAX;
        let mut seen = 0u32;
        for (light, cascades) in lights.iter() {
            let Some(k) = kept.get(&light) else { continue };
            for (view, list) in cascades.cascades.iter() {
                let Some(kl) = k.cascades.get(view) else { continue };
                for (i, (c, kc)) in list.iter().zip(kl.iter()).enumerate().take(32) {
                    seen |= 1 << i;
                    if unchanged(c, kc) {
                        same |= 1 << i;
                    } else {
                        same &= !(1 << i);
                    }
                }
            }
        }
        let fogged = match (sky.as_deref(), lights.iter().find_map(|(_, c, config)| config.filter(|config| config.bounds.len() == count).map(|config| (config, c)))) {
            (Some(sky), Some((config, _))) => fog_hidden(sky, config, count),
            _ => 0,
        };
        for i in 1..count.min(32) {
            if (same & seen & (1 << i) != 0 || fogged & (1 << i) != 0) && since[i] < UNCHANGED_HOLD_FRAMES {
                mask |= 1 << i;
            }
        }
        // The budget: the nearest cascades drawn first, the farthest held over to the next
        // frame, which then draws them with the maps this frame's schedules would have --
        // but never one that has waited [`UNCHANGED_HOLD_FRAMES`], so every cascade is drawn
        // at least that often whatever the budget.
        if budget.passes > 0 {
            let mut drawn: Vec<usize> = (0..count.min(32)).filter(|i| mask & (1 << i) == 0).collect();
            while drawn.len() > budget.passes {
                let Some(at) = drawn.iter().rposition(|&i| since[i] < UNCHANGED_HOLD_FRAMES) else { break };
                let i = drawn.remove(at);
                mask |= 1 << i;
            }
        }
    }
    if mask == 0 {
        kept.clear();
        if !schedule.is_empty() {
            kept.extend(lights.iter().map(|(e, c, _)| (e, c.clone())));
        }
    } else {
        for (light, mut cascades, _) in &mut lights {
            let Some(k) = kept.get_mut(&light) else { continue };
            for (view, list) in cascades.cascades.iter_mut() {
                let Some(kl) = k.cascades.get_mut(view) else { continue };
                for (i, (c, kc)) in list.iter_mut().zip(kl.iter_mut()).enumerate() {
                    if mask & (1 << i) != 0 {
                        *c = kc.clone();
                    } else {
                        *kc = c.clone();
                    }
                }
            }
        }
    }
    // How long each cascade has gone without being drawn ([`UNCHANGED_HOLD_FRAMES`]).
    for (i, drawn) in since.iter_mut().enumerate() {
        if mask & (1 << i) != 0 {
            *drawn = drawn.saturating_add(1);
        } else {
            *drawn = 0;
        }
    }
    // Once for each schedule that comes into use (the switch on, or the staggered one with
    // it off): the first frame that kept maps.
    if schedule.is_empty() {
        *said = None;
    } else if mask != 0 && *said != Some(on.0) {
        if on.0 {
            info!("sun shadows: a frame kept the shadow maps of the frame before ({} light(s))", kept.len());
        } else {
            info!("sun shadows: a frame kept its farther cascades' maps (staggered schedule, held cascades {mask:#05b}, {} light(s))", kept.len());
        }
        *said = Some(on.0);
    }
    if hold.0 != mask {
        hold.0 = mask;
    }
}

/// `performance`: start in Performance mode (`--sun-shadows=performance`; the program sets
/// `SkyState`'s cascade count to match). `budget` is the shadow budget
/// (`--shadow-budget=<casters>,<passes>`, [`ShadowBudget`]).
pub struct ShadowRatePlugin {
    pub performance: bool,
    pub budget: ShadowBudget,
}

impl Plugin for ShadowRatePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(PerformanceShadows(self.performance))
            .insert_resource(self.budget)
            .insert_resource(CascadesBeforePerformance(self.performance.then_some(crate::sky::DEFAULT_SHADOW_CASCADES)))
            .init_resource::<HoldDirectionalShadowMaps>()
            .add_plugins(ExtractResourcePlugin::<HoldDirectionalShadowMaps>::default())
            .add_systems(
                PostUpdate,
                hold_cascades.after(SimulationLightSystems::UpdateDirectionalLightCascades).before(SimulationLightSystems::UpdateLightFrusta),
            )
            .add_systems(Update, far_trees_cast_no_shadow);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cascades(view: Entity, n: usize) -> Cascades {
        let mut c = Cascades::default();
        c.cascades.insert(view, vec![Cascade::default(); n]);
        c
    }

    #[test]
    fn maps_are_held_only_for_the_same_lights_and_cascades() {
        let (sun, arms, view) = (Entity::from_raw_u32(1).unwrap(), Entity::from_raw_u32(2).unwrap(), Entity::from_raw_u32(3).unwrap());
        let (three, two) = (cascades(view, 3), cascades(view, 2));
        let mut kept = EntityHashMap::default();
        kept.insert(sun, three.clone());
        assert!(same_layout(&kept, &[(sun, &three)]));
        // Never without lights.
        assert!(!same_layout(&kept, &[]));
        // A light that was not there (the arms' shade light switching on), a different
        // number of cascades, or another view: this frame draws.
        assert!(!same_layout(&kept, &[(sun, &three), (arms, &three)]));
        assert!(!same_layout(&kept, &[(sun, &two)]));
        assert!(!same_layout(&kept, &[(sun, &cascades(arms, 3))]));
    }

    /// The switch gives back the cascade count it replaced (#2182 review: it reset a
    /// `--graphics-cycle cascades=` value to the default), and only sets the count.
    #[test]
    fn the_switch_gives_back_the_cascades_it_replaced() {
        let mut world = World::new();
        world.init_resource::<PerformanceShadows>();
        let mut sky = SkyState::default();
        sky.shadow_cascades = 4;
        world.insert_resource(sky);
        set_performance(&mut world, true);
        assert_eq!(world.resource::<SkyState>().shadow_cascades, PERFORMANCE_CASCADES);
        set_performance(&mut world, true);
        set_performance(&mut world, false);
        assert_eq!(world.resource::<SkyState>().shadow_cascades, 4);
        // Off when it already is: the count stays.
        set_performance(&mut world, false);
        assert_eq!(world.resource::<SkyState>().shadow_cascades, 4);
    }

    #[test]
    fn the_schedules_hold_the_cascades_they_should() {
        // Performance: the nearest every other frame, the far one every fourth, on other
        // frames still: never two cascades in a frame, and three of four frames draw one.
        let performance: Vec<u32> = (0..8).map(|f| held_mask(&PERFORMANCE_SCHEDULE, f, 2)).collect();
        assert_eq!(performance, [0b10, 0b11, 0b10, 0b01, 0b10, 0b11, 0b10, 0b01]);
        let drawn: Vec<u32> = performance.iter().map(|held| 2 - held.count_ones()).collect();
        assert_eq!(drawn, [1, 0, 1, 1, 1, 0, 1, 1], "0.75 maps a frame, never two");
        // Staggered: the nearest every frame, the middle every other, the farthest every
        // fourth, never both of the far two drawn in one frame.
        let staggered: Vec<u32> = (0..4).map(|f| held_mask(&STAGGERED_SCHEDULE, f, 3)).collect();
        assert_eq!(staggered, [0b110, 0b100, 0b010, 0b100]);
        // More cascades than the schedule: those are drawn every frame.
        assert_eq!(held_mask(&PERFORMANCE_SCHEDULE, 1, 3), 0b011);
        assert_eq!(held_mask(&[], 1, 3), 0);
    }

    /// A cascade whose grid did not move redraws the same depth: it is held, but not for
    /// ever (a caster may move while the view does not).
    #[test]
    fn a_grid_that_did_not_move_is_held() {
        let kept = Cascade { world_from_cascade: Mat4::IDENTITY, clip_from_cascade: Mat4::IDENTITY, clip_from_world: Mat4::IDENTITY, texel_size: 9.4 };
        assert!(unchanged(&kept, &kept.clone()));
        let mut moved = kept.clone();
        moved.clip_from_world.w_axis.x += moved.texel_size;
        assert!(!unchanged(&moved, &kept), "a grid a texel over is another map");
        assert!(UNCHANGED_HOLD_FRAMES >= 2, "a caster that moves is drawn again");
    }

    #[test]
    fn the_budget_has_no_caps_by_default_and_parses_its_text() {
        assert_eq!(ShadowBudget::default(), ShadowBudget { casters: 0, passes: 0 });
        assert_eq!(ShadowBudget::parse("1200,1"), ShadowBudget { casters: 1200, passes: 1 });
        assert_eq!(ShadowBudget::parse("800"), ShadowBudget { casters: 800, passes: 0 });
        assert_eq!(ShadowBudget::parse(""), ShadowBudget::default());
        assert_eq!(ShadowBudget::parse("x,y"), ShadowBudget::default());
    }

    #[test]
    fn fog_that_is_opaque_at_a_cascades_near_bound_hides_it() {
        let mut sky = SkyState::default();
        // Fog 0..500: everything from 500 out is fog colour.
        sky.fog_override = Some((0.0, 500.0));
        // Bounds 667, 2,309, 8,000 with a fifth of overlap: the second cascade starts at
        // 533 and the third at 1,847, both past the fog's end; the first starts at 5.
        let config = crate::sky::shadow_cascades(8000.0, 1.0, 3);
        assert_eq!(fog_hidden(&sky, &config, 3), 0b110);
        // Fog far away: nothing is hidden.
        sky.fog_override = Some((0.0, 80_000.0));
        assert_eq!(fog_hidden(&sky, &config, 3), 0);
        // No fog at all.
        sky.fog_override = None;
        assert_eq!(fog_hidden(&sky, &config, 3), 0);
    }

    #[test]
    fn a_far_tree_stops_casting_and_starts_again_nearer() {
        assert!(tree_casts(TREE_SHADOW_NEAR, true) && !tree_casts(TREE_SHADOW_NEAR + 1.0, true));
        // Between the two distances it stays as it is.
        let between = (TREE_SHADOW_NEAR + TREE_SHADOW_BACK) / 2.0;
        assert!(tree_casts(between, true) && !tree_casts(between, false));
        assert!(tree_casts(TREE_SHADOW_BACK, false));
    }
}
