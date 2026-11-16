//! The loaded area as one thing (docs: engine-limits.md §2.1 S2, §5 T1/T2): the scripts'
//! set follows the engine's streaming radius (`GameConfig::loaded_radius`, the streamer's
//! `--radius`), what `GetGridsToLoad` answers is that setting's grid (`2r + 1`), and the
//! reference walks treat the whole square as loaded. At the default (2) the sets are the
//! original's 5 × 5, unchanged. On a synthetic plugin: nine by nine exterior cells of one
//! worldspace and a marker in three of them.

use crate::common;

use common::*;
use fnv_esm::{LoadOrder, Plugin, Schema};
use fnv_game::game::{Game, GameConfig};
use fnv_game::GameData;
use fnv_script::FunctionTable;
use fnv_sim::{FormId, FormStore, RefTable, SimConfig, World};
use fnv_script::vm::XVal;
use serde_json::Value as Json;

const WRLD: u32 = 0x100;
const MARKER_BASE: u32 = 0x110;
/// The markers: in the player's cell, two cells east, and three cells east.
const HERE_REF: u32 = 0x300;
const TWO_EAST_REF: u32 = 0x301;
const THREE_EAST_REF: u32 = 0x302;
const CELL_BASE: u32 = 0x200;
/// The grid spans −4..=4 each way: a 9 × 9, so `--radius 3` has a ring beyond it and
/// `--radius 4` would just fit.
const SIDE: i32 = 4;

/// A cell's FormID: one per grid square, so a square can be found from its coordinates.
fn cell_id(x: i32, y: i32) -> u32 {
    CELL_BASE + ((x + SIDE) * (2 * SIDE + 1) + (y + SIDE)) as u32
}

fn plugin_bytes() -> Vec<u8> {
    let marker = record(b"STAT", 0, MARKER_BASE, &[sub(b"EDID", z("TestMarker"))]);
    let placed = |fid: u32, edid: &str, x: i32, y: i32| {
        let pos = [(x as f32 + 0.5) * 4096.0, (y as f32 + 0.5) * 4096.0, 0.0];
        let data = pos.iter().flat_map(|v| v.to_le_bytes()).chain([0u8; 12]).collect();
        record(b"REFR", 0, fid, &[sub(b"EDID", z(edid)), sub(b"NAME", MARKER_BASE.to_le_bytes().to_vec()), sub(b"DATA", data)])
    };
    // Each CELL with its references, as the world-children groups hold them.
    let mut children = Vec::new();
    for x in -SIDE..=SIDE {
        for y in -SIDE..=SIDE {
            let id = cell_id(x, y);
            let mut xclc = x.to_le_bytes().to_vec();
            xclc.extend(y.to_le_bytes());
            children.extend(record(b"CELL", 0, id, &[sub(b"EDID", z(&format!("TestCell{}_{}", x, y))), sub(b"XCLC", xclc)]));
            let refs: Vec<u8> = match (x, y) {
                (0, 0) => placed(HERE_REF, "TestHereREF", x, y),
                (2, 0) => placed(TWO_EAST_REF, "TestTwoEastREF", x, y),
                (3, 0) => placed(THREE_EAST_REF, "TestThreeEastREF", x, y),
                _ => Vec::new(),
            };
            if !refs.is_empty() {
                children.extend(group(id.to_le_bytes(), 6, &group(id.to_le_bytes(), 9, &refs)));
            }
        }
    }
    let wrld = record(b"WRLD", 0, WRLD, &[sub(b"EDID", z("TestWorld"))]);
    let world_children = group(WRLD.to_le_bytes(), 1, &group(0i32.to_le_bytes(), 4, &group(0i32.to_le_bytes(), 5, &children)));
    [tes4(), top(b"STAT", &marker), top(b"WRLD", &[wrld, world_children].concat())].concat()
}

struct Fixture {
    data: GameData,
    forms: FormStore,
    refs: RefTable,
}

fn fixture() -> Fixture {
    let plugins = vec![Plugin::parse(&plugin_bytes(), "LoadedArea.esm".into()).unwrap()];
    let lo = LoadOrder::new(&plugins).unwrap();
    let schema = Schema::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../schemas/fnv")).unwrap();
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas/script/functions.json");
    let j: Json = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    let table = FunctionTable::load(&p).unwrap();
    let forms = FormStore::build(&lo, &schema);
    let refs = RefTable::build(&lo, &schema);
    let data = GameData::build(&lo, &schema, table, &j, &refs);
    Fixture { data, forms, refs }
}

fn game(f: &Fixture, radius: i32) -> Game<'_> {
    let world = World::new(&f.forms, &f.refs, SimConfig::default());
    let mut g = Game::new(&f.data, world, GameConfig { loaded_radius: radius, ..GameConfig::default() });
    g.new_game();
    query(&mut g, "player.MoveTo TestHereREF\nplayer.SetPos X 0\nplayer.SetPos Y 0\nplayer.SetPos Z 0");
    assert_eq!(g.cell_of(FormId::PLAYER_REF), Some(FormId(cell_id(0, 0))));
    g.player_entered(Some(FormId(cell_id(0, 0))));
    g.events.clear();
    g
}

fn query(g: &mut Game, body: &str) -> XVal {
    g.run_query(&format!("Begin Function {{ }}\n{body}\nEnd\n"), Default::default()).unwrap_or_else(|e| panic!("{body}: {e}"))
}

fn num(g: &mut Game, expr: &str) -> f64 {
    match query(g, &format!("SetFunctionValue ({expr})")) {
        XVal::Num(x) => x,
        v => panic!("{expr}: {v:?}"),
    }
}

/// Default (the engine's `--radius 2`): the original's 25 cells, `GetGridsToLoad` 5, and
/// nothing beyond ±2 is loaded. This is the test that pins the default behaviour.
#[test]
fn the_default_radius_is_the_originals_five_by_five() {
    let f = fixture();
    let mut g = game(&f, 2);
    let expected: std::collections::BTreeSet<FormId> = (-2..=2).flat_map(|x| (-2..=2).map(move |y| FormId(cell_id(x, y)))).collect();
    assert_eq!(g.loaded_cells, expected);
    assert_eq!(g.grids_to_load(), 5);
    assert_eq!(num(&mut g, "GetGridsToLoad"), 5.0);
    assert_eq!(num(&mut g, "TestTwoEastREF.HasLoaded3D"), 1.0);
    assert_eq!(num(&mut g, "TestThreeEastREF.HasLoaded3D"), 0.0);
}

/// `--radius 3`: 7 × 7 loaded for scripts, `GetGridsToLoad` 7 (the setting, not the drawn
/// count), and the third ring's reference is loaded for `HasLoaded3D` and the walks.
#[test]
fn a_wider_streaming_radius_loads_the_same_square_for_scripts() {
    let f = fixture();
    let mut g = game(&f, 3);
    let expected: std::collections::BTreeSet<FormId> = (-3..=3).flat_map(|x| (-3..=3).map(move |y| FormId(cell_id(x, y)))).collect();
    assert_eq!(g.loaded_cells, expected);
    assert_eq!(g.grids_to_load(), 7);
    assert_eq!(num(&mut g, "GetGridsToLoad"), 7.0);
    assert_eq!(num(&mut g, "TestThreeEastREF.HasLoaded3D"), 1.0, "the third ring is script-loaded");
}

/// `GetFirstRef`'s kin treat the whole square as loaded: the third ring's references are
/// walked at `--radius 3` and are persistent-only at the default (T1/T6).
#[test]
fn the_reference_walk_follows_the_loaded_square() {
    let f = fixture();
    let mut narrow = game(&f, 2);
    let mut wide = game(&f, 3);
    // Markers in the player's cell, two and three cells east; depth 3 scans all of them.
    assert_eq!(num(&mut narrow, "GetNumRefs 0 3 0"), 2.0, "the third ring is not loaded");
    assert_eq!(num(&mut wide, "GetNumRefs 0 3 0"), 3.0, "the third ring is loaded");
}

/// The parity shim: `GetInGrid`'s depth default stays at 2 (the 5 × 5 half) whatever the
/// streaming radius, because that default is what this engine and its goldens were
/// recorded with. The wiki's default is 0 and its `-1` means uGridsToLoad: a separate
/// fix that must not ride on the radius (it would change `--radius 2` runs too).
#[test]
fn get_in_grid_keeps_its_fixed_depth() {
    let f = fixture();
    let mut g = game(&f, 3);
    assert_eq!(num(&mut g, "TestTwoEastREF.HasLoaded3D"), 1.0);
    assert_eq!(num(&mut g, "GetInGrid TestTwoEastREF"), 1.0, "default depth 2");
    assert_eq!(num(&mut g, "TestThreeEastREF.HasLoaded3D"), 1.0);
    assert_eq!(num(&mut g, "GetInGrid TestThreeEastREF"), 0.0, "depth 2 by contract, not the radius");
    assert_eq!(num(&mut g, "GetInGrid TestThreeEastREF 3"), 1.0, "an explicit depth reaches it");
}
