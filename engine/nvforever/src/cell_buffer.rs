//! The cell buffer: which cells the player left recently enough to be "still buffered in
//! memory", for the AI's process levels (`fnv_ai::process`, docs/design/actor-behaviour.md
//! §5.9).
//!
//! - Actors in exterior cells "not currently loaded, but which have recently been
//!   visited by the player and are still buffered in memory" are at the original's
//!   Mid-High level (updated about every 18 s); actors in cells "neither currently loaded
//!   nor buffered" at Low (about every 120 s); entering a casino puts the Strip's exterior
//!   actors at Mid-High [documented: GECK wiki (JIP) GetActorsByProcessingLevel].
//! - The buffers' sizes are INI settings: `uExterior Cell Buffer=36`, `uInterior Cell
//!   Buffer=3` with `uGridsToLoad=5` [observed: the owner's install, Fallout_default.ini
//!   and Fallout.ini]. The exterior buffer follows the loaded area here
//!   ([`exterior_cells`]): the community rule is `(uGridsToLoad + 1)²`, 36 at the
//!   original's 5 × 5 and 64 at 7 × 7 [documented: TweakGuides' rule, quoted in the
//!   Nexus/AFK threads]. With a buffer smaller than the loaded square, `keep_newest`
//!   evicts *loaded* cells arbitrarily and their actors drop from Mid-High to Low while
//!   in view.
//! - What a buffer holds [hypothesis]: the cells drawn most recently, the drawn ones
//!   included (36 = the 5 × 5 loaded cells and the 11 left last; the interior buffer is
//!   the interior the player is in and the two before). An interior does not touch the
//!   exterior buffer (the Strip example) nor the other way round.
//! - Actors in "interior cells that have not been visited, but are accessible through
//!   teleport doors within player's loaded cell grid" are at Mid-Low, and "once actor's
//!   level goes up to Mid-Low, it never seems to go back to Low" (Chet in Goodsprings)
//!   [documented: same page]. Its rate is "unknown at this time" [documented:
//!   GetActorProcessingLevel]: here such an interior counts as buffered, every 18 s
//!   [design: the rate those actors had before the Low level existed]. The interiors are
//!   the destinations of the load doors placed in the exterior cells drawn this session.
//! - An actor walking out of the loaded area (or, already out, into another cell) takes
//!   the cell it walks into into the buffer, as if drawn then [hypothesis: an actor being
//!   processed keeps its surroundings in memory; design: otherwise an actor walking out
//!   of the grid into cells never drawn would drop at once to the 120 s rate, which left
//!   Sunny Smiles's dog behind her]. Such a cell leaves the buffer like any other.

use bevy::prelude::*;
use fnv_nav::Space;
use std::collections::HashMap;

use crate::world::{Streamer, WorldData};
use fnv_sim::{FormId, RefTable};
use std::collections::HashSet;

/// Exterior cells kept at the original's loaded area [observed: `uExterior Cell Buffer`,
/// Fallout_default.ini].
pub const EXTERIOR_CELLS: usize = 36;
/// Interior cells kept [observed: `uInterior Cell Buffer`, Fallout_default.ini].
pub const INTERIOR_CELLS: usize = 3;

/// Exterior cells the buffer keeps with `radius` cells loaded each way
/// (`nvforever --radius`): `(uGridsToLoad + 1)²` with `uGridsToLoad = 2r + 1`, i.e.
/// `(2r + 2)²` — 36 at the original's 5 × 5, 64 at 7 × 7 [documented: the community's
/// `(uGrids + 1)²` rule, quoted in the Nexus/AFK threads; observed: 36 at uGrids=5].
/// Never below [`EXTERIOR_CELLS`]: a loaded square smaller than the original's keeps the
/// buffer the original had. The buffer is always larger than the loaded square
/// (`(2r + 2)² > (2r + 1)²`), so `keep_newest` cannot evict a cell that is drawn.
pub fn exterior_cells(radius: i32) -> usize {
    let side = 2usize.saturating_mul(radius.max(0) as usize).saturating_add(2);
    side.saturating_mul(side).max(EXTERIOR_CELLS)
}

/// The cells drawn most recently (module notes).
#[derive(Debug)]
pub struct CellBuffer {
    /// Exterior cells kept ([`exterior_cells`]).
    exterior_cells: usize,
    /// Exterior cells (worldspace, grid square) and interior cells, with the frame each
    /// was last drawn in.
    exterior: HashMap<(u32, (i32, i32)), u64>,
    interior: HashMap<u32, u64>,
    frame: u64,
    /// Interiors behind the load doors of the exterior cells drawn so far (Mid-Low),
    /// and the exterior cells whose doors were looked at.
    behind_doors: HashSet<u32>,
    doors_seen: HashSet<(u32, (i32, i32))>,
    /// Cells actors walked into this frame ([`CellBuffer::carry`]), taken in at the next
    /// [`CellBuffer::remember`].
    carried: std::sync::Mutex<Vec<Key>>,
}

impl Default for CellBuffer {
    /// The original's buffer, for the original's loaded area.
    fn default() -> Self {
        CellBuffer::new(fnv_game::game::DEFAULT_LOADED_RADIUS)
    }
}

impl CellBuffer {
    /// A buffer for `radius` cells loaded each way: [`exterior_cells`] exterior cells and
    /// the original's three interiors.
    pub fn new(radius: i32) -> CellBuffer {
        CellBuffer {
            exterior_cells: exterior_cells(radius),
            exterior: HashMap::new(),
            interior: HashMap::new(),
            frame: 0,
            behind_doors: HashSet::new(),
            doors_seen: HashSet::new(),
            carried: Default::default(),
        }
    }

    /// The cells kept: exterior grid squares by worldspace, and interiors.
    pub fn cells(&self) -> (impl Iterator<Item = (u32, (i32, i32))> + '_, impl Iterator<Item = u32> + '_) {
        (self.exterior.keys().copied(), self.interior.keys().copied())
    }

    /// The cells drawn in this frame: an interior, or the exterior grid squares of
    /// worldspace `world`.
    pub fn remember(&mut self, world: u32, interior: Option<u32>, exterior: impl Iterator<Item = (i32, i32)>) {
        self.frame += 1;
        let carried = self.carried.get_mut().map(std::mem::take).unwrap_or_default();
        for k in carried {
            match k {
                Key::Exterior(w, c) => self.exterior.insert((w, c), self.frame),
                Key::Interior(i) => self.interior.insert(i, self.frame),
            };
        }
        match interior {
            Some(i) => {
                self.interior.insert(i, self.frame);
            }
            None => {
                for c in exterior {
                    self.exterior.insert((world, c), self.frame);
                }
            }
        }
        keep_newest(&mut self.exterior, self.exterior_cells);
        keep_newest(&mut self.interior, INTERIOR_CELLS);
    }

    /// An actor walked into the cell at `feet` in `space`: the cell counts as buffered
    /// from now (module notes).
    pub fn carry(&self, space: Space, feet: [f32; 3]) {
        if let Ok(mut c) = self.carried.lock() {
            c.push(key(space, feet));
        }
    }

    /// Note the interiors the load doors of the drawn exterior cells of `world` lead to
    /// (each cell looked at once).
    pub fn note_doors(&mut self, refs: &RefTable, world: u32, exterior: impl Iterator<Item = (i32, i32)>) {
        let Some(grid) = refs.world_grid.get(&FormId(world)) else { return };
        for c in exterior {
            if !self.doors_seen.insert((world, c)) {
                continue;
            }
            let Some(cell) = grid.get(&c) else { continue };
            for r in refs.by_cell.get(cell).into_iter().flatten() {
                let to = refs.get(*r).and_then(|p| p.teleport.as_ref()).and_then(|t| refs.teleport_cell(t));
                if let Some(to) = to.filter(|&t| refs.world_of(t).is_none()) {
                    self.behind_doors.insert(to.0);
                }
            }
        }
    }

    /// Whether the cell at `feet` in `space` is in the buffer (or an interior behind the
    /// doors of the cells drawn so far, module notes).
    pub fn holds(&self, space: Space, feet: [f32; 3]) -> bool {
        if self.carried.lock().is_ok_and(|c| c.contains(&key(space, feet))) {
            return true;
        }
        match space {
            Space::Interior(c) => self.interior.contains_key(&c) || self.behind_doors.contains(&c),
            Space::World(w) => self.exterior.contains_key(&(w, fnv_world::cell_of(feet[0], feet[1]))),
        }
    }

    #[cfg(test)]
    fn len(&self) -> (usize, usize) {
        (self.exterior.len(), self.interior.len())
    }
}

/// A cell of the buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    Exterior(u32, (i32, i32)),
    Interior(u32),
}

fn key(space: Space, feet: [f32; 3]) -> Key {
    match space {
        Space::Interior(c) => Key::Interior(c),
        Space::World(w) => Key::Exterior(w, fnv_world::cell_of(feet[0], feet[1])),
    }
}

/// Drop the oldest entries beyond `n` (among cells left in the same frame, any).
fn keep_newest<K: Copy + Eq + std::hash::Hash>(m: &mut HashMap<K, u64>, n: usize) {
    if m.len() <= n {
        return;
    }
    let mut by_age: Vec<(u64, K)> = m.iter().map(|(k, f)| (*f, *k)).collect();
    by_age.sort_unstable_by_key(|e| std::cmp::Reverse(e.0));
    for (_, k) in &by_age[n..] {
        m.remove(k);
    }
}

/// Note the cells drawn this frame (after `world::stream`), and the interiors behind
/// their doors (the placed references: the played game's, else the residents' own).
pub fn remember(mut streamer: ResMut<Streamer>, world: Res<WorldData>, sim: Option<Res<nvf_play::sim::SimState>>, residents: Option<ResMut<crate::residents::Residents>>) {
    let refs: Option<&'static RefTable> = match sim {
        Some(s) => Some(s.refs),
        None => residents.and_then(|mut r| r.own_game().map(|g| g.world.placed)),
    };
    let s = &mut *streamer;
    // The loading screen's placeholder key is no cell.
    let drawn = || s.loaded.keys().copied().filter(|&(x, _)| x != i32::MIN);
    if let (Some(refs), None) = (refs, s.interior) {
        s.buffer.note_doors(refs, world.world, drawn());
    }
    let drawn: Vec<(i32, i32)> = drawn().collect();
    s.buffer.remember(world.world, s.interior, drawn.into_iter());
}

/// Tell the scripts' game the cells kept, when they change (GetBufferedCells,
/// GetCellBuffered; `fnv_game::nvse_jip12`): the exterior squares as their cells.
pub fn tell_game(streamer: Res<Streamer>, script: Option<ResMut<nvf_play::script::ScriptState>>, mut told: Local<Vec<(u32, (i32, i32))>>, mut told_in: Local<Vec<u32>>) {
    let Some(rt) = script.and_then(|s| s.into_inner().rt.as_mut()) else { return };
    let (ext, int) = streamer.buffer.cells();
    let (mut ext, mut int): (Vec<_>, Vec<_>) = (ext.collect(), int.collect());
    ext.sort_unstable();
    int.sort_unstable();
    if *told == ext && *told_in == int {
        return;
    }
    let g = &mut rt.game;
    let mut cells: std::collections::BTreeSet<fnv_sim::FormId> = int.iter().map(|&c| fnv_sim::FormId(c)).collect();
    for &(w, xy) in &ext {
        if let Some(c) = g.world.placed.world_grid.get(&fnv_sim::FormId(w)).and_then(|grid| grid.get(&xy)) {
            cells.insert(*c);
        }
    }
    g.set_buffered_cells(cells);
    (*told, *told_in) = (ext, int);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(x0: i32, y0: i32) -> impl Iterator<Item = (i32, i32)> {
        (-2..=2).flat_map(move |dx| (-2..=2).map(move |dy| (x0 + dx, y0 + dy)))
    }

    fn feet(c: (i32, i32)) -> [f32; 3] {
        [c.0 as f32 * 4096.0 + 100.0, c.1 as f32 * 4096.0 + 100.0, 0.0]
    }

    #[test]
    fn cells_left_stay_until_newer_ones_push_them_out() {
        let mut b = CellBuffer::default();
        b.remember(7, None, grid(0, 0));
        // One step north: the southern row (y = -2) is no longer drawn, but buffered.
        b.remember(7, None, grid(0, 1));
        assert!(b.holds(Space::World(7), feet((0, -2))));
        assert_eq!(b.len().0, 30);
        // More steps: 25 drawn and 11 left, the oldest go.
        b.remember(7, None, grid(0, 2));
        assert_eq!(b.len().0, 35);
        b.remember(7, None, grid(0, 3));
        assert_eq!(b.len().0, 36);
        b.remember(7, None, grid(0, 4));
        assert!(!b.holds(Space::World(7), feet((0, -2))), "pushed out");
        assert!(b.holds(Space::World(7), feet((0, 0))), "a row left later is still in");
        assert!(!b.holds(Space::World(8), feet((0, 3))), "another worldspace");
    }

    #[test]
    fn a_cell_an_actor_walks_into_is_buffered_until_newer_ones_push_it_out() {
        let mut b = CellBuffer::default();
        b.remember(7, None, grid(0, 0));
        assert!(!b.holds(Space::World(7), feet((0, -3))));
        b.carry(Space::World(7), feet((0, -3)));
        assert!(b.holds(Space::World(7), feet((0, -3))), "at once");
        b.remember(7, None, grid(0, 0));
        assert!(b.holds(Space::World(7), feet((0, -3))), "kept");
        for y in 1..=4 {
            b.remember(7, None, grid(0, y));
        }
        assert!(!b.holds(Space::World(7), feet((0, -3))), "pushed out");
    }

    #[test]
    fn an_interior_keeps_the_exterior_buffer_and_three_interiors_are_kept() {
        let mut b = CellBuffer::default();
        b.remember(7, None, grid(0, 0));
        for i in 1..=4 {
            b.remember(7, Some(100 + i), std::iter::empty());
        }
        assert!(b.holds(Space::World(7), feet((2, 2))), "the exterior stays buffered");
        assert!(!b.holds(Space::Interior(101), [0.0; 3]));
        assert!((102..=104).all(|i| b.holds(Space::Interior(i), [0.0; 3])));
    }

    /// The buffer's size follows the loaded area: `(uGridsToLoad + 1)²`, 36 at the
    /// original's 5 × 5 and 64 at `--radius 3`'s 7 × 7 [documented: `(uGrids + 1)²`],
    /// and never below the original's. A loaded square larger than the buffer would let
    /// `keep_newest` evict cells that are drawn (T2).
    #[test]
    fn the_buffer_is_never_smaller_than_the_loaded_square() {
        assert_eq!(exterior_cells(2), EXTERIOR_CELLS, "the default stays the owner's 36");
        assert_eq!(exterior_cells(3), 64, "7 × 7 plus a ring");
        for r in 0..=8 {
            let square = (2 * r + 1) * (2 * r + 1);
            assert!(exterior_cells(r) > square, "radius {r}: {} cells for a {square}-cell square", exterior_cells(r));
        }
        assert_eq!(exterior_cells(-1), EXTERIOR_CELLS, "a nonsense radius keeps the original's");
    }

    #[test]
    fn a_seven_by_seven_grid_is_not_evicted_by_the_cells_left_behind() {
        let grid7 = |x0: i32, y0: i32| (-3..=3).flat_map(move |dx| (-3..=3).map(move |dy| (x0 + dx, y0 + dy)));
        let mut b = CellBuffer::new(3);
        b.remember(7, None, grid7(0, 0));
        // One step north: the 49 drawn cells and the row left are 56, inside 64.
        b.remember(7, None, grid7(0, 1));
        for c in grid7(0, 0) {
            assert!(b.holds(Space::World(7), feet(c)), "{c:?} was evicted while drawn (or left)");
        }
        assert_eq!(b.len().0, 56);
    }
}
