//! The original's shipped bugs: reproduced by default, because NVForever is a parity
//! engine and a record is its own specification; repaired at load only when
//! [`ShippedBugs`] is on. A repair is therefore an explicit, named divergence from the
//! goldens, never something the parity path reaches.
//!
//! Off unless an API caller (a test, and later the settings page) sets it, or
//! `NVF_FIX_SHIPPED_BUGS=1` is in the environment when the game data is built. Every
//! record a repair touches is logged, so a run's log states which game it played. The
//! fixes and their evidence are catalogued in `reports/mods-bugfix.md`; this module builds
//! `fix-in-shining-armor` only.

use crate::actor_values::{ENERGY, ENERGY_WEAPONS};
use crate::data::{GameData, PerkDef, PerkEffect};
use fnv_script::vm::{ArgValue, Value};
use fnv_sim::FormId;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

/// IsWeaponSkillType's condition index (`function` in a CTDA) [documented: GECK wiki, the
/// function; `schemas/script/functions.json` `condition_index` 109].
const IS_WEAPON_SKILL_TYPE: u16 = 109;

/// The two Dead Money perks whose `IsWeaponSkillType` condition asks for `ENERGY` (2, the
/// AI attribute) instead of `ENERGY_WEAPONS` (34): In Shining Armor's +5 DT and its hidden
/// eyewear companion's +2, so neither ever applies.
///
/// [Observed: DeadMoney.esm PERK `0x0100F216` and `0x0100F217`, conditions on tab 1;
/// documented: the wiki's bug note and YUP v1, JSawyer, "In Shining Armor Perk Fix".] The
/// EditorID is tried first; the file-local FormID, with DeadMoney.esm's load-order slot,
/// finds the record even if an override drops the EDID. `FormId` here is a load-order
/// index, so the local ID alone would be ambiguous.
const IN_SHINING_ARMOR: [(&str, &str, u32); 2] = [
    ("In Shining Armor", "nvdlc01inshiningarmor", 0x00F216),
    ("In Shining Armor (eyewear)", "nvdlc01inshiningarmoreyewear", 0x00F217),
];

/// The plugin the two records are defined in.
const DEAD_MONEY: &str = "DeadMoney.esm";

/// The repairs in force at load. `Default` is all off: the original's behaviour, and the
/// parity lab's setting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShippedBugs {
    /// The master switch; a per-fix row can be added here when a second fix lands.
    pub enabled: bool,
}

/// The explicit setting [`ShippedBugs::set_enabled`] writes: unset, off or on.
static EXPLICIT: AtomicU8 = AtomicU8::new(UNSET);
const UNSET: u8 = 0;
const OFF: u8 = 1;
const ON: u8 = 2;

impl ShippedBugs {
    /// The setting in force: an explicit [`ShippedBugs::set_enabled`] wins, otherwise
    /// `NVF_FIX_SHIPPED_BUGS=1` turns every fix on. The environment is read once.
    pub fn current() -> Self {
        let enabled = match EXPLICIT.load(Ordering::Relaxed) {
            ON => true,
            OFF => false,
            _ => env_enabled(),
        };
        ShippedBugs { enabled }
    }

    /// Turn the repairs on or off for the rest of the process. Tests pin a run with it;
    /// the settings page will call it when the player changes the setting.
    pub fn set_enabled(on: bool) {
        EXPLICIT.store(if on { ON } else { OFF }, Ordering::Relaxed);
    }

    /// Forget the explicit setting, so the environment decides again (tests).
    #[cfg(test)]
    fn clear() {
        EXPLICIT.store(UNSET, Ordering::Relaxed);
    }
}

/// Whether `NVF_FIX_SHIPPED_BUGS=1` was in the environment; the first caller fixes it.
fn env_enabled() -> bool {
    static VALUE: OnceLock<bool> = OnceLock::new();
    *VALUE.get_or_init(|| std::env::var_os("NVF_FIX_SHIPPED_BUGS").is_some_and(|v| v.to_str() == Some("1")))
}

/// Apply the repairs [`ShippedBugs`] names to the records just built. Called once at the
/// end of [`GameData::build`](crate::data::GameData::build), after every plugin's
/// override has been resolved.
pub(crate) fn apply(data: &mut GameData) {
    if !ShippedBugs::current().enabled {
        return;
    }
    for (name, key, local) in IN_SHINING_ARMOR {
        let Some(id) = form_of(data, key, local) else { continue };
        let edid = data.edid_of.get(&id).cloned().unwrap_or_else(|| key.to_string());
        let Some(perk) = data.perks.get_mut(&id) else { continue };
        if patch_perk(perk) {
            tracing::info!("fixes: {name} ({edid}): IsWeaponSkillType parameter {ENERGY} -> {ENERGY_WEAPONS}");
        }
    }
}

/// The record's FormID: by EditorID, or by its file-local FormID in DeadMoney.esm's
/// load-order slot when an override kept no EDID.
fn form_of(data: &GameData, key: &str, local: u32) -> Option<FormId> {
    if let Some(&id) = data.edids.get(key) {
        return Some(id);
    }
    let plugin = u32::try_from(data.load_order.iter().position(|p| p.eq_ignore_ascii_case(DEAD_MONEY))?).ok()?;
    Some(FormId((plugin << 24) | local))
}

/// Rewrite each entry condition that asks for `ENERGY` as the weapon skill to
/// `ENERGY_WEAPONS`; whether anything changed. The parameter is matched by its value, not
/// by its `ArgValue` shape: the catalogue types it `ptActorValue` (an `Enum`), but a
/// catalogue without that type leaves it a number, and the fix must find it either way.
fn patch_perk(def: &mut PerkDef) -> bool {
    let mut patched = false;
    for entry in &mut def.entries {
        let PerkEffect::EntryPoint(ep) = &mut entry.effect else { continue };
        for (_, condition) in ep.conditions.iter_mut().flatten() {
            if condition.function != IS_WEAPON_SKILL_TYPE {
                continue;
            }
            let Some(parameter) = condition.params.first_mut() else { continue };
            if parameter.value() != Some(Value::Num(f64::from(ENERGY))) {
                continue;
            }
            match parameter {
                ArgValue::Enum(code) => *code = ENERGY_WEAPONS,
                ArgValue::Value(value) => *value = Value::Num(f64::from(ENERGY_WEAPONS)),
                ArgValue::Int32(value) => *value = i32::from(ENERGY_WEAPONS),
                _ => continue,
            }
            patched = true;
        }
    }
    patched
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::{record, sub, tes4, top, z};
    use crate::cond::{Cmp, Operand, RunOn};
    use crate::data::{EntryPointDef, PerkEntry};
    use fnv_esm::raw::Subrecord;
    use fnv_esm::{LoadOrder, Plugin, Schema};
    use fnv_script::FunctionTable;
    use fnv_sim::RefTable;
    use serde_json::Value as Json;

    const PERK: u32 = 0x8001;
    const EYEWEAR: u32 = 0x8002;
    const OTHER: u32 = 0x8003;
    const TORSO_LIST: u32 = 0x8010;
    const EYEWEAR_LIST: u32 = 0x8011;
    const ARMOR: u32 = 0x8020;

    const EQ: u8 = 0x00;
    const GE: u8 = 3 << 5;
    const GET_EQUIPPED: u16 = 182;
    const GET_PERMANENT_ACTOR_VALUE: u16 = 495;
    const HAS_PERK: u16 = 449;

    /// One CTDA, laid out as the schema's 28-byte Condition: type, comparison value,
    /// function, two unused bytes, parameter 1, parameter 2, run-on, reference.
    fn condition(cmp: u8, value: f32, function: u16, parameter: u32) -> Subrecord {
        let mut d = vec![cmp, 0, 0, 0];
        d.extend(value.to_le_bytes());
        d.extend(function.to_le_bytes());
        d.extend([0, 0]);
        d.extend(parameter.to_le_bytes());
        d.extend([0; 12]);
        assert_eq!(d.len(), 28);
        sub(b"CTDA", d)
    }

    /// A perk with one Modify Damage Threshold (defender) entry, in the real order:
    /// EDID, FULL, DESC, take-conditions, DATA, PRKE, entry DATA, then PRKC 0 and the
    /// GetEquipped condition, PRKC 1 and the IsWeaponSkillType condition, EPFT, EPFD,
    /// PRKF [observed: DeadMoney.esm `0x0100F216` and `0x0100F217`].
    fn perk(fid: u32, edid: &str, full: bool, value: f32, form_list: u32, takes: &[Subrecord]) -> Vec<u8> {
        let mut subs = vec![sub(b"EDID", z(edid))];
        if full {
            subs.push(sub(b"FULL", z(edid)));
        }
        subs.push(sub(b"DESC", z("")));
        subs.extend(takes.iter().cloned());
        subs.extend([
            sub(b"DATA", vec![0, 1, 1, 1, 0]),
            sub(b"PRKE", vec![2, 0, 0]),
            // Real entry DATA is `38 02 03`: point 56 (DT defender), Add Value, type 3.
            sub(b"DATA", vec![56, 2, 3]),
            sub(b"PRKC", vec![0]),
            condition(EQ, 1.0, GET_EQUIPPED, form_list),
            sub(b"PRKC", vec![1]),
            condition(EQ, 1.0, IS_WEAPON_SKILL_TYPE, u32::from(ENERGY)),
            sub(b"EPFT", vec![1]),
            sub(b"EPFD", value.to_le_bytes().to_vec()),
            sub(b"PRKF", Vec::new()),
        ]);
        record(b"PERK", 0, fid, &subs)
    }

    fn form_list(fid: u32, edid: &str) -> Vec<u8> {
        record(b"FLST", 0, fid, &[sub(b"EDID", z(edid)), sub(b"LNAM", ARMOR.to_le_bytes().to_vec())])
    }

    /// A plugin of the two Dead Money perks and one unrelated perk with the same defect.
    fn fixture() -> GameData {
        let takes = [
            condition(GE, 20.0, GET_PERMANENT_ACTOR_VALUE, u32::from(crate::actor_values::REPAIR)),
            condition(GE, 70.0, GET_PERMANENT_ACTOR_VALUE, u32::from(crate::actor_values::SCIENCE)),
        ];
        let eyewear_takes = [condition(EQ, 1.0, HAS_PERK, PERK)];
        let perks = [
            perk(PERK, "NVDLC01InShiningArmor", true, 5.0, TORSO_LIST, &takes),
            perk(EYEWEAR, "NVDLC01InShiningArmorEyewear", false, 2.0, EYEWEAR_LIST, &eyewear_takes),
            perk(OTHER, "OtherPerk", true, 5.0, TORSO_LIST, &takes),
        ]
        .concat();
        let lists = [form_list(TORSO_LIST, "NVDLC01ArmorVsBeamTorso"), form_list(EYEWEAR_LIST, "NVDLC01ArmorVsBeamEyewear")].concat();
        let armor = record(b"STAT", 0, ARMOR, &[sub(b"EDID", z("NVDLC01MetalArmor"))]);
        let bytes = [tes4(), top(b"PERK", &perks), top(b"FLST", &lists), top(b"STAT", &armor)].concat();
        let plugins = vec![Plugin::parse(&bytes, "Fix.esm".into()).unwrap()];
        let lo = LoadOrder::new(&plugins).unwrap();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let schema = Schema::load_dir(root.join("schemas/fnv")).unwrap();
        let p = root.join("schemas/script/functions.json");
        let j: Json = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        let refs = RefTable::build(&lo, &schema);
        GameData::build(&lo, &schema, FunctionTable::load(&p).unwrap(), &j, &refs)
    }

    fn entry(data: &GameData, fid: u32) -> &EntryPointDef {
        let entry = data.perks[&FormId(fid)].entries.first().expect("one entry");
        let PerkEffect::EntryPoint(ep) = &entry.effect else { panic!("an entry point") };
        ep
    }

    #[test]
    fn in_shining_armor_is_patched_only_when_enabled() {
        let tab_of = |data: &GameData, fid: u32| -> Vec<i8> { entry(data, fid).conditions.as_ref().expect("conditions").iter().map(|(t, _)| *t).collect() };
        let parameter_of = |data: &GameData, fid: u32| -> u16 {
            let conditions = entry(data, fid).conditions.as_ref().expect("conditions");
            let (_, condition) = conditions.iter().find(|(_, c)| c.function == IS_WEAPON_SKILL_TYPE).expect("an IsWeaponSkillType condition");
            match condition.params.first() {
                Some(ArgValue::Enum(code)) => *code,
                other => panic!("an actor value parameter, not {other:?}"),
            }
        };

        ShippedBugs::set_enabled(false);
        let off = fixture();
        assert_eq!(tab_of(&off, PERK), vec![0, 1]);
        assert_eq!(parameter_of(&off, PERK), ENERGY);
        assert_eq!(parameter_of(&off, EYEWEAR), ENERGY);
        // The tab 0 GetEquipped condition is still there.
        assert!(entry(&off, PERK).conditions.as_ref().expect("conditions").iter().any(|(t, c)| *t == 0 && c.function == GET_EQUIPPED));

        ShippedBugs::set_enabled(true);
        let on = fixture();
        assert_eq!(tab_of(&on, PERK), vec![0, 1]);
        assert_eq!(parameter_of(&on, PERK), ENERGY_WEAPONS);
        assert_eq!(parameter_of(&on, EYEWEAR), ENERGY_WEAPONS);
        assert!(entry(&on, PERK).conditions.as_ref().expect("conditions").iter().any(|(t, c)| *t == 0 && c.function == GET_EQUIPPED));
        // The fix is the two named records, not every condition that happens to ask for
        // Energy.
        assert_eq!(parameter_of(&on, OTHER), ENERGY);

        ShippedBugs::clear();
    }

    #[test]
    fn a_numeric_parameter_is_patched_too() {
        // A catalogue that does not type IsWeaponSkillType's parameter leaves it a
        // number rather than `ArgValue::Enum`; the repair must still find it.
        let condition = crate::cond::Condition {
            cmp: Cmp::Eq,
            or: false,
            run_on: RunOn::Subject,
            function: IS_WEAPON_SKILL_TYPE,
            params: vec![ArgValue::Value(Value::Num(f64::from(ENERGY)))],
            rhs: Operand::Value(1.0),
        };
        let mut def = PerkDef {
            entries: vec![PerkEntry {
                rank: 0,
                priority: 0,
                effect: PerkEffect::EntryPoint(EntryPointDef { point: 56, function: 2, conditions: Some(vec![(1, condition)]), ..Default::default() }),
                ordinal: 0,
            }],
            ..Default::default()
        };
        assert!(patch_perk(&mut def));
        let PerkEffect::EntryPoint(ep) = &def.entries[0].effect else { panic!("an entry point") };
        let (_, condition) = &ep.conditions.as_ref().expect("conditions")[0];
        assert_eq!(condition.params.first(), Some(&ArgValue::Value(Value::Num(f64::from(ENERGY_WEAPONS)))));
    }
}
