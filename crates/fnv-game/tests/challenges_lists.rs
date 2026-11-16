//! Challenge filters over run-time form lists and a type-3 challenge's SNAM victim.
//! GunRunnersArsenal.esm exercises both: its setup script adds the GRA 12.7mm SMG to
//! WeaponMachinegunLIST (Nyah! See?), and "Even a God-King Can Bleed" names FortCaesar
//! in SNAM. docs/spec-notes/challenges.md §2–3.

use crate::common;

use common::*;
use fnv_esm::{LoadOrder, Plugin, Schema};
use fnv_game::challenges::{kind, ChallengeDef, HIT_CRIPPLE};
use fnv_game::combat::{HitInfo, LimbHit, HEAD};
use fnv_game::functions::Ctx;
use fnv_game::game::{Game, GameConfig};
use fnv_game::GameData;
use fnv_script::vm::{ArgValue, Value};
use fnv_script::FunctionTable;
use fnv_sim::{FormId, FormStore, RefTable, SimConfig, World};
use serde_json::Value as Json;

const CELL: u32 = 0x200;
const TARGET_BASE: u32 = 0x830;
const OTHER_BASE: u32 = 0x831;
const TARGET_REF: u32 = 0x320;
const TARGET_REF2: u32 = 0x322;
const OTHER_REF: u32 = 0x321;
const WEAP_AUTHORED: u32 = 0x841;
const WEAP_RUNTIME: u32 = 0x842;
const FLST: u32 = 0x850;
const CHAL_LISTS: u32 = 0x900;
const CHAL_HEAD: u32 = 0x901;

const PLAYER: u32 = 0x14;

/// AddFormToFormList [documented: functions.json; `functions::refs`].
const ADD_FORM_TO_FORM_LIST: u16 = 0x1233;
/// KillActor.
const KILL_ACTOR: u16 = 0x108B;

fn f32b(v: f32) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

fn placed(sig: &[u8; 4], fid: u32, base: u32, pos: [f32; 3]) -> Vec<u8> {
    let data = [f32b(pos[0]), f32b(pos[1]), f32b(pos[2]), vec![0; 12]].concat();
    record(sig, 0, fid, &[sub(b"NAME", base.to_le_bytes().to_vec()), sub(b"DATA", data)])
}

fn npc(fid: u32, edid: &str, health: i32) -> Vec<u8> {
    // DATA: base Health, then S.P.E.C.I.A.L. (as functions.rs's fixture).
    let data = [health.to_le_bytes().to_vec(), vec![5, 5, 5, 4, 5, 5, 5]].concat();
    record(b"NPC_", 0, fid, &[sub(b"EDID", z(edid)), sub(b"DATA", data)])
}

fn plugin_bytes() -> Vec<u8> {
    let npcs = [npc(TARGET_BASE, "TargetNpc", 100), npc(OTHER_BASE, "OtherNpc", 100)].concat();
    let weap_data = [10i32.to_le_bytes().to_vec(), 100i32.to_le_bytes().to_vec(), f32b(2.0), 5i16.to_le_bytes().to_vec(), vec![6]].concat();
    let weapons = [
        record(b"WEAP", 0, WEAP_AUTHORED, &[sub(b"EDID", z("AuthoredGun")), sub(b"DATA", weap_data.clone())]),
        record(b"WEAP", 0, WEAP_RUNTIME, &[sub(b"EDID", z("RuntimeGun")), sub(b"DATA", weap_data)]),
    ]
    .concat();
    let flst = record(b"FLST", 0, FLST, &[sub(b"EDID", z("WeaponList")), sub(b"LNAM", u32s(&[WEAP_AUTHORED]))]);
    let cell = record(b"CELL", 0, CELL, &[sub(b"EDID", z("TestCell"))]);
    let refs = [
        placed(b"ACHR", TARGET_REF, TARGET_BASE, [0.0; 3]),
        placed(b"ACHR", TARGET_REF2, TARGET_BASE, [20.0, 0.0, 0.0]),
        placed(b"ACHR", OTHER_REF, OTHER_BASE, [10.0; 3]),
    ]
    .concat();
    let children = group(CELL.to_le_bytes(), 6, &group(CELL.to_le_bytes(), 9, &refs));
    [
        tes4(),
        top(b"NPC_", &npcs),
        top(b"WEAP", &weapons),
        top(b"FLST", &flst),
        top(b"CELL", &group(0i32.to_le_bytes(), 2, &group(0i32.to_le_bytes(), 3, &[cell, children].concat()))),
    ]
    .concat()
}

struct Fixture {
    data: GameData,
    forms: FormStore,
    refs: RefTable,
}

fn fixture() -> Fixture {
    let plugins = vec![Plugin::parse(&plugin_bytes(), "Challenges.esm".into()).unwrap()];
    let lo = LoadOrder::new(&plugins).unwrap();
    let schema = Schema::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../schemas/fnv")).unwrap();
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas/script/functions.json");
    let j: Json = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    let table = FunctionTable::load(&p).unwrap();
    let forms = FormStore::build(&lo, &schema);
    let refs = RefTable::build(&lo, &schema);
    let mut data = GameData::build(&lo, &schema, table, &j, &refs);
    let def = |form, kind, value1, value2, form1, form2| ChallengeDef {
        form,
        kind,
        threshold: 100,
        flags: 0,
        interval: 0,
        value1,
        value2,
        value3: 0,
        form1,
        form2,
        name: Some(format!("Challenge {form:?}")),
        description: None,
    };
    // Kill from the weapon list only (the Nyah! See? shape): no victim filter.
    data.challenge_defs.insert(FormId(CHAL_LISTS), def(FormId(CHAL_LISTS), kind::KILL_FROM_LIST, 0, 0, None, Some(FormId(FLST))));
    // Hit an enemy: the head, crippled, the SNAM base NPC, the weapon list (the Even a
    // God-King Can Bleed shape).
    data.challenge_defs.insert(
        FormId(CHAL_HEAD),
        def(FormId(CHAL_HEAD), kind::HIT_ENEMY, 1, HIT_CRIPPLE, Some(FormId(TARGET_BASE)), Some(FormId(FLST))),
    );
    Fixture { data, forms, refs }
}

fn game(f: &Fixture) -> Game<'_> {
    let world = World::new(&f.forms, &f.refs, SimConfig::default());
    let mut g = Game::new(&f.data, world, GameConfig { strict: true, ..GameConfig::default() });
    g.player_entered(Some(FormId(CELL)));
    g
}

fn r(x: u32) -> ArgValue {
    ArgValue::Value(Value::Ref(x))
}

fn call(g: &mut Game, op: u16, on: Option<u32>, args: &[ArgValue]) -> f64 {
    let ctx = Ctx { unit: "test".into(), opcode: op, on: on.map(FormId), self_ref: None, action_ref: None, quest: None, inst: None, in_condition: false };
    match g.call_function(&ctx, args) {
        Value::Num(x) => x,
        Value::Ref(f) => f as f64,
    }
}

fn progress(g: &Game, chal: u32) -> u32 {
    g.player.challenges.get(&FormId(chal)).map_or(0, |c| c.progress)
}

/// KillActor counts the weapon the player last hit the victim with (`player_weapon_on`).
fn kill_with(g: &mut Game, victim: FormId, weapon: FormId) {
    g.extra.entry(victim).or_default().last_hit =
        Some(fnv_game::game::LastHit { attacker: Some(FormId::PLAYER_REF), weapon: Some(weapon), location: 0, critical: false });
    call(g, KILL_ACTOR, Some(victim.0), &[r(PLAYER)]);
}

/// A head hit that cripples the head (limb condition in points, so it reaches 0).
fn cripple_head(g: &mut Game, target: FormId, weapon: FormId) {
    // The fixture's NPCs have Health from DATA; set it so a 1-point hit is no death.
    fnv_game::functions::set_actor_value_current(g, target, fnv_game::actor_values::HEALTH, 100.0).unwrap();
    let r = g.hit(
        target,
        1.0,
        HitInfo {
            attacker: Some(FormId::PLAYER_REF),
            weapon: Some(weapon),
            location: 1,
            critical: false,
            limb: Some(LimbHit { condition: HEAD, health_percent: 100.0, damage: 1000.0, points: true }),
        },
    );
    assert!(r.crippled, "the head must cripple for the challenge to see a cripple");
}

/// A member a script adds at run time counts for a weapon-list challenge, as
/// AddFormToFormList's additions do [observed: GunRunnersArsenal.esm's setup script adds
/// the GRA 12.7mm SMG 0x0100084E to WeaponMachinegunLIST 0x0015EAD5].
#[test]
fn a_script_added_form_list_member_counts_for_a_kill_challenge() {
    let f = fixture();
    let mut g = game(&f);
    // The authored member counts (the control).
    kill_with(&mut g, FormId(TARGET_REF), FormId(WEAP_AUTHORED));
    assert_eq!(progress(&g, CHAL_LISTS), 1);
    // A weapon the record does not list counts nothing before a script adds it.
    kill_with(&mut g, FormId(OTHER_REF), FormId(WEAP_RUNTIME));
    assert_eq!(progress(&g, CHAL_LISTS), 1);
    // The setup script's AddFormToFormList: the run-time member now counts.
    call(&mut g, ADD_FORM_TO_FORM_LIST, None, &[r(FLST), r(WEAP_RUNTIME)]);
    assert!(fnv_game::functions::list_members(&g, FormId(FLST)).contains(&FormId(WEAP_RUNTIME)));
    // A second victim of the same base: the first is already dead.
    kill_with(&mut g, FormId(TARGET_REF2), FormId(WEAP_RUNTIME));
    assert_eq!(progress(&g, CHAL_LISTS), 2);
}

/// A "hit an enemy" challenge's SNAM narrows the victim, and its XNAM still narrows the
/// weapon: a head cripple on another base counts nothing [hypothesis: GunRunnersArsenal's
/// "Even a God-King Can Bleed" names FortCaesar 0x00121FEF, so any other actor must not
/// count; no base-game type-3 challenge sets SNAM, so base-game results are unchanged].
#[test]
fn a_type_3_challenge_snam_narrows_the_victim() {
    let f = fixture();
    let mut g = game(&f);
    // The wrong weapon (not in XNAM), on the SNAM target: no step.
    cripple_head(&mut g, FormId(TARGET_REF), FormId(WEAP_RUNTIME));
    assert_eq!(progress(&g, CHAL_HEAD), 0);
    // The right weapon, on a victim whose base is not SNAM: no step.
    cripple_head(&mut g, FormId(OTHER_REF), FormId(WEAP_AUTHORED));
    assert_eq!(progress(&g, CHAL_HEAD), 0);
    // The right weapon on the SNAM target: a step (a fresh head, since the first is
    // already crippled).
    cripple_head(&mut g, FormId(TARGET_REF2), FormId(WEAP_AUTHORED));
    assert_eq!(progress(&g, CHAL_HEAD), 1);
}
