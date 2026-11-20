//! Measuring aid: what the drawn triangles are (2026-11-20). `NVF_TRI_COUNT=<seconds>` logs,
//! every that many seconds, the triangles of every mesh visible in some view this frame
//! (after frustum and distance culling; the GPU's own occlusion culling is not seen here),
//! split by kind (land, placed references, the rest: distant LOD, imposters, sky, water,
//! effects), and the ten largest single meshes with the name of the nearest named entity.
//! Each mesh counts once however many passes draw it (prepass, main, shadow).

use crate::terrain::LandMesh;
use bevy::mesh::{Indices, Mesh3d};
use bevy::prelude::*;
use nvf_play::RefVisual;

pub struct TriCountPlugin;

impl Plugin for TriCountPlugin {
    fn build(&self, app: &mut App) {
        let Some(every) = std::env::var("NVF_TRI_COUNT").ok().and_then(|v| v.parse::<f32>().ok()).filter(|v| *v > 0.0) else {
            return;
        };
        app.insert_resource(Every(every)).add_systems(Startup, spawn_label).add_systems(Last, count);
    }
}

#[derive(Resource)]
struct Every(f32);

/// The on-screen line (top left) the counts are written to.
#[derive(Component)]
struct Label;

fn spawn_label(mut commands: Commands) {
    commands.spawn((
        Label,
        Text::new("triangles: counting..."),
        TextFont { font_size: bevy::text::FontSize::Px(22.0), ..default() },
        TextColor(Color::srgb(1.0, 0.85, 0.2)),
        Node { position_type: PositionType::Absolute, left: Val::Px(12.0), top: Val::Px(12.0), ..default() },
        GlobalZIndex(1000),
    ));
}

/// Triangles and vertices of every mesh asset, read the frame it appears: the game's meshes
/// keep their data only until it moves to the render world (RENDER_WORLD usage), after which
/// reading it panics, so only the non-panicking accessors are used.
#[derive(Default)]
struct Counts(std::collections::HashMap<AssetId<Mesh>, (u64, u64)>);

fn read_counts(mesh: &Mesh) -> Option<(u64, u64)> {
    let verts = mesh.try_attribute_option(Mesh::ATTRIBUTE_POSITION).ok()?.map_or(0, |a| a.len() as u64);
    let tris = match mesh.try_indices_option().ok()? {
        Some(Indices::U16(i)) => i.len() as u64 / 3,
        Some(Indices::U32(i)) => i.len() as u64 / 3,
        None => verts / 3,
    };
    Some((tris, verts))
}

fn count(
    every: Res<Every>,
    time: Res<Time<Real>>,
    mut last: Local<f32>,
    meshes: Res<Assets<Mesh>>,
    mut counts: Local<Counts>,
    q: Query<(Entity, &Mesh3d, &ViewVisibility, Has<LandMesh>, Has<RefVisual>)>,
    parents: Query<&ChildOf>,
    names: Query<&Name>,
    mut label: Query<&mut Text, With<Label>>,
) {
    for (id, mesh) in meshes.iter() {
        if !counts.0.contains_key(&id) {
            if let Some(c) = read_counts(mesh) {
                counts.0.insert(id, c);
            }
        }
    }
    let now = time.elapsed_secs();
    if now - *last < every.0 {
        return;
    }
    *last = now;
    let (mut land, mut refs, mut other) = ((0u64, 0u32), (0u64, 0u32), (0u64, 0u32));
    let mut top: Vec<(u64, Entity)> = Vec::new();
    let mut hidden = 0u64;
    let (mut drawn_v, mut hidden_v, mut hidden_n) = (0u64, 0u64, 0u32);
    for (e, m, vis, is_land, is_ref) in &q {
        let Some(&(t, v)) = counts.0.get(&m.0.id()) else { continue };
        if !vis.get() {
            hidden += t;
            hidden_v += v;
            hidden_n += 1;
            continue;
        }
        drawn_v += v;
        let slot = if is_land { &mut land } else if is_ref { &mut refs } else { &mut other };
        slot.0 += t;
        slot.1 += 1;
        top.push((t, e));
    }
    top.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    let name_of = |mut e: Entity| -> String {
        for _ in 0..8 {
            if let Ok(n) = names.get(e) {
                return n.as_str().to_string();
            }
            match parents.get(e) {
                Ok(p) => e = p.parent(),
                Err(_) => break,
            }
        }
        format!("{e:?}")
    };
    let total = land.0 + refs.0 + other.0;
    info!(
        "triangles drawn: {total} in {} meshes (land {} in {}, placed {} in {}, other {} in {}); {hidden} more in culled meshes",
        land.1 + refs.1 + other.1, land.0, land.1, refs.0, refs.1, other.0, other.1
    );
    if let Ok(mut text) = label.single_mut() {
        let m = |v: u64| v as f64 / 1e6;
        text.0 = format!(
            "drawn: {:.2}M triangles, {:.2}M vertices, {} meshes\nculled (off-screen / out of range): {:.2}M triangles, {:.2}M vertices, {hidden_n} meshes\nland {:.2}M | placed {:.2}M | other (LOD, sky, water...) {:.2}M",
            m(total), m(drawn_v), land.1 + refs.1 + other.1, m(hidden), m(hidden_v), m(land.0), m(refs.0), m(other.0)
        );
    }
    let list: Vec<String> = top.iter().take(10).map(|(t, e)| format!("{t} {}", name_of(*e))).collect();
    info!("triangles drawn, largest meshes: {}", list.join("; "));
}
