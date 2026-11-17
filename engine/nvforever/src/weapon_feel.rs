//! Weapon feel driven by the installed mods' own config files (WEAPON-FEEL.md).
//!
//! The overlay ships the mods that tune how the first-person weapon feels, but their
//! numbers sat inert: the engine used its own constants. This module reads their files
//! once at start-up and hands the numbers to the systems that already exist —
//! [`crate::view_sway`] (B42 Inertia's tuning), [`nvf_play::combat`] through
//! [`fnv_combat::pattern`] (Viewmodel Recoil's per-weapon patterns) and
//! [`crate::weapon_lower`] (FPS Weapon Lowering's sprint and idle state). One switch,
//! `--weapon-feel` (default **off**: every constant stays the engine's, so behaviour is
//! exactly what it was), also a row on the settings page.
//!
//! | Mod | File we read | Keys | Where they go |
//! |---|---|---|---|
//! | B42 Inertia | `Config/B42Inertia.ini`, `[Tuning]` | `fIntensityMultVertical`, `fIntensityMultHorizontal` | [`SwayTuning::intensity_v`] / [`SwayTuning::intensity_h`] |
//! | | | `fSpeedMult`, `fCenteringSpeed` | the turn lag's time constant while turning / while still ([`SwayTuning::from_rates`]) |
//! | | | `bRotationMode` | **no counterpart**: recorded, not applied |
//! | Viewmodel Recoil | `Config/ProfilesVRecoil/Default.ini` | `[General]` intensities, limits and the two damage coefficients; `[Ironsights]` limit multiplier and constant coefficient; `[Individual] vUseIndividualRecoil`; `[Randomization] vIsRandomized` | [`fnv_combat::pattern::RecoilProfile`], which scales the per-weapon patterns |
//! | | `Config/WeaponVRecoil/<EDID>.ini`, `[PATTERN]` | `X`, `Y`, `dX`, `dY`, `cIS` (and `Z`, `dZ`, which are read but unused) | the view kick of a shot of that weapon |
//! | FPS Weapon Lowering | `config/FPS WL/FPSWL Config.ini`, `[SETTINGS]` | `iWLMode` (0 hotkey, 1 automatic, 2 complex) | whether the sprint/idle state lowers the weapon at all; the mod ships **no** offset numbers, so the pose and shares below are ours |
//! | Smooth True Ironsights | `NVSE/Plugins/Tweaks/MenuConfig/SmoothTrueIronsights.json` | — | nothing: the file ships settings sliders (min/max) and **no default values** |
//!
//! **What has no counterpart, stated rather than guessed.** B42's `bRotationMode` picks
//! how the mod rotates the viewmodel; this module's pose always turns the arms about the
//! eye, which is neither of the pivots the design note names ("rear", "centre"), so the
//! key is read and printed, never applied (and no pivot key name is invented: the real
//! file is not on this machine to read one from). Viewmodel Recoil's `Z`/`dZ` (rotation)
//! have no axis to move — the view is heading and pitch only. Its `[Stats]` block
//! (`vStrengthCoeffV`, `vWeightInfluenceV`, `vUseAmmoPseudoCalc` …) has no viewmodel mass
//! model to act on and is parsed-but-unused. `iWLKey`/`iWLSound` are the mod's own key and
//! sound, which the engine already has its own of (Lower Weapon, H).
//!
//! **Where the files are read from**: through the run's own `Vfs` (`nvf_assets::GameData`),
//! so a config file is read from the same overlay as an asset and the overlay's precedence
//! decides which copy wins (`mods.txt`, TOP WINS; `fnv-vfs`'s one-folder-per-mod indexing).
//! A file that is absent, a key that is missing, and a value that is not a finite number
//! all leave the engine's constant in place, so an install without these mods is
//! byte-for-byte the old behaviour even with the switch on.

use crate::view_sway::SwayTuning;
use bevy::prelude::*;
use fnv_combat::pattern::{self, Ini, RecoilProfile};

/// The switch (`--weapon-feel`), and the settings page's key.
pub const SWITCH: &str = "--weapon-feel";

/// The files' paths **as the overlay sees them**: mod-relative asset keys, one path inside
/// a mod folder (`fnv_vfs` normalizes these), so the lookup is the engine's own — the
/// highest-precedence mod that has the file wins, exactly as an asset does.
const B42_INERTIA: &[&str] = &["Config/B42Inertia.ini"];
/// Viewmodel Recoil's global profile, and the folder its per-weapon patterns sit in.
const VRECOIL_PROFILE: &[&str] = &["Config/ProfilesVRecoil/Default.ini"];
const VRECOIL_PATTERNS: &str = "config\\weaponvrecoil\\";
/// FPS Weapon Lowering's INI, in a promoted install and in the FOMOD layout the overlay
/// has (the mod's payload sits under `[MAIN]/`, so it is no asset path but still indexed).
const FPS_LOWERING: &[&str] = &["Config/FPS Weapon Lowering.ini", "config/FPS WL/FPSWL Config.ini", "[MAIN]/config/FPS WL/FPSWL Config.ini"];
const FPS_LOWERING_JSON: &[&str] = &["Meshes/AnimGroupOverride/FPS Weapon Lowering.json", "[MAIN]/Meshes/AnimGroupOverride/FPS Weapon Lowering.json"];
/// Smooth True Ironsights' settings menu file.
const SMOOTH_IRONSIGHTS: &[&str] = &["NVSE/Plugins/Tweaks/MenuConfig/SmoothTrueIronsights.json"];

/// A ground speed (units/s) above which the player is running rather than walking
/// (a walk is 85, a run 300 in `view_sway`'s terms) [design].
pub const SPRINT_SPEED: f32 = 250.0;
/// How far down the weapon sits while running, and standing idle for [`IDLE_SECONDS`]
/// [design: the mod lowers it by playing a lowered idle clip, which we do not have, so the
/// share of the same pose `weapon_lower.rs` already uses is ours].
pub const SPRINT_SHARE: f32 = 0.85;
pub const IDLE_SHARE: f32 = 0.45;
pub const IDLE_SECONDS: f32 = 3.0;

/// FPS Weapon Lowering's `iWLMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoweringMode {
    /// 0: the lower key only (the engine's Lower Weapon, H, already does that).
    Hotkey,
    /// 1: automatic.
    Automatic,
    /// 2: the mod's "complex" mode — automatic, and the key.
    Complex,
}

impl LoweringMode {
    fn parse(v: f32) -> LoweringMode {
        match v {
            v if v >= 2.0 => LoweringMode::Complex,
            v if v >= 1.0 => LoweringMode::Automatic,
            _ => LoweringMode::Hotkey,
        }
    }

    /// Whether the sprint/idle state lowers the weapon by itself.
    pub fn automatic(self) -> bool {
        !matches!(self, LoweringMode::Hotkey)
    }
}

/// What FPS Weapon Lowering's own file said (the numbers of the pose are ours: the mod
/// ships none).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lowering {
    pub mode: LoweringMode,
    /// The mod's own key and sound switches, read for the log and unused (the engine has
    /// its own Lower Weapon key).
    pub key: Option<f32>,
    pub sound: Option<bool>,
}

impl Lowering {
    /// The share of the lowering this frame's state asks for: running, else idle after
    /// [`IDLE_SECONDS`]. Aiming or a shot at hand (`busy`) keeps the weapon up.
    pub fn state_share(self, speed: f32, sneaking: bool, busy: bool, idle: f32) -> f32 {
        if !self.mode.automatic() || busy {
            return 0.0;
        }
        if speed > SPRINT_SPEED && !sneaking {
            SPRINT_SHARE
        } else if idle >= IDLE_SECONDS {
            IDLE_SHARE
        } else {
            0.0
        }
    }
}

/// All the mods' numbers this run found, and whether any of them is in effect.
#[derive(Resource, Debug, Clone, Default)]
pub struct WeaponFeel {
    /// The switch: `--weapon-feel` or the settings page. Off means none of the files'
    /// numbers reach a system, which is the engine's behaviour before this module.
    pub enabled: bool,
    /// B42 Inertia's tuning of [`crate::view_sway`].
    pub sway: SwayTuning,
    /// Viewmodel Recoil's profile and per-weapon patterns, when *any* file was found.
    pub recoil: Option<RecoilProfile>,
    /// FPS Weapon Lowering's mode, when its file was found.
    pub lowering: Option<Lowering>,
    /// What the load found and what it did with it, one line each, for the log and the
    /// report.
    pub report: Vec<String>,
}

/// The first of `candidates` the overlay has, as the file's text (its key is for the log).
fn read_first(vfs: &fnv_vfs::Vfs, candidates: &[&str]) -> Option<(String, String)> {
    for rel in candidates {
        if let Some(Ok(bytes)) = vfs.read(rel) {
            return Some((fnv_vfs::normalize(rel), String::from_utf8_lossy(&bytes).into_owned()));
        }
    }
    None
}

impl WeaponFeel {
    /// Read the mods layer's configs once, through the `vfs` the run mounted (so a file is
    /// the one the engine reads: the highest-precedence mod that has it wins).
    pub fn load(vfs: &fnv_vfs::Vfs, enabled: bool) -> WeaponFeel {
        let mut feel = WeaponFeel { enabled, ..WeaponFeel::default() };
        feel.load_sway(vfs);
        feel.load_recoil(vfs);
        feel.load_lowering(vfs);
        feel.load_smooth_ironsights(vfs);
        feel.report.push(if enabled { "the switch is on: the files' numbers are applied".into() } else { "the switch is off: the files' numbers are read and logged, nothing applied".into() });
        feel
    }

    /// B42 Inertia: the four numbers with a counterpart in [`crate::view_sway`], plus the
    /// keys it has none for.
    fn load_sway(&mut self, vfs: &fnv_vfs::Vfs) {
        let Some((path, text)) = read_first(vfs, B42_INERTIA) else {
            self.report.push("B42 Inertia: no Config/B42Inertia.ini in the overlay; the engine's sway constants stand".into());
            return;
        };
        let ini = Ini::parse(&text);
        let v = ini.f32("tuning", "fintensitymultvertical");
        let h = ini.f32("tuning", "fintensitymulthorizontal");
        let speed = ini.f32("tuning", "fspeedmult");
        let centering = ini.f32("tuning", "fcenteringspeed");
        let mut tuning = SwayTuning::from_rates(speed.unwrap_or(5.0), centering.unwrap_or(1.0));
        tuning.intensity_v = v.filter(|v| *v >= 0.0).unwrap_or(1.0);
        tuning.intensity_h = h.filter(|v| *v >= 0.0).unwrap_or(1.0);
        self.sway = tuning;
        let gone = |name: &str, v: Option<f32>| match v {
            Some(v) => format!("{name} {v}"),
            None => format!("{name} missing"),
        };
        let mut line = format!(
            "B42 Inertia {path}: {v} → vertical ×{iv:.2}, {h} → horizontal ×{ih:.2}, {s} → turn lag {tc:.3} s, {c} → rest lag {rc:.3} s",
            path = path,
            v = gone("fIntensityMultVertical", v),
            iv = tuning.intensity_v,
            h = gone("fIntensityMultHorizontal", h),
            ih = tuning.intensity_h,
            s = gone("fSpeedMult", speed),
            tc = tuning.turn_chase,
            c = gone("fCenteringSpeed", centering),
            rc = tuning.rest_chase
        );
        // Keys with no counterpart, named: rotation mode and whichever pivot key the file
        // has, so a future session can map them from the mod's own description.
        let unmapped: Vec<String> = ini
            .keys("tuning")
            .into_iter()
            .filter(|k| !matches!(k.as_str(), "fintensitymultvertical" | "fintensitymulthorizontal" | "fspeedmult" | "fcenteringspeed"))
            .collect();
        if !unmapped.is_empty() {
            line.push_str(&format!("; no counterpart, not applied: {}", unmapped.join(", ")));
        }
        self.report.push(line);
    }

    /// Viewmodel Recoil: the global profile and every per-weapon pattern file.
    fn load_recoil(&mut self, vfs: &fnv_vfs::Vfs) {
        let Some((profile_path, text)) = read_first(vfs, VRECOIL_PROFILE) else {
            self.report.push("Viewmodel Recoil: no Config/ProfilesVRecoil/Default.ini; the engine's own recoil stands".into());
            return;
        };
        let mut profile = pattern::parse_profile(&text);
        let mut files = 0;
        let mut read = 0;
        for key in vfs.paths(|k| k.starts_with(VRECOIL_PATTERNS) && k.ends_with(".ini")) {
            let Some(name) = key.rsplit('\\').next() else { continue };
            let Some(edid) = name.strip_suffix(".ini") else { continue };
            if edid.eq_ignore_ascii_case("template") {
                continue;
            }
            files += 1;
            let Some(Ok(bytes)) = vfs.read(&key) else { continue };
            if let Some(p) = pattern::parse_pattern(&String::from_utf8_lossy(&bytes)) {
                profile.patterns.insert(edid.to_ascii_lowercase(), p);
                read += 1;
            }
        }
        self.report.push(format!(
            "Viewmodel Recoil {}: vertical intensity {:.2}, horizontal {:.2}, limits {:.1}–{:.1} / {:.1}–{:.1}, damage coefficients {:.2}/{:.2}, down the sights ×{:.2} limits and ×{:.2} movement, individual recoil {}: {read} of {files} weapon pattern file(s) read",
            profile_path,
            profile.vertical_intensity,
            profile.horizontal_intensity,
            profile.vertical_lower,
            profile.vertical_upper,
            profile.horizontal_lower,
            profile.horizontal_upper,
            profile.damage_coeff,
            profile.damage_coeff_h,
            profile.ironsights_limits,
            profile.ironsights_constant,
            if profile.use_individual { "on" } else { "off" },
        ));
        self.report.push("Viewmodel Recoil: `Z`/`dZ` (roll) and the `[Stats]` block are read but unused — the view has no roll and no viewmodel mass".into());
        self.recoil = Some(profile);
    }

    /// FPS Weapon Lowering: its mode (the only number it ships well-defined), and whether
    /// its offset JSON is there at all.
    fn load_lowering(&mut self, vfs: &fnv_vfs::Vfs) {
        let Some((path, text)) = read_first(vfs, FPS_LOWERING) else {
            self.report.push("FPS Weapon Lowering: no INI; sprint and idle lowering stays off".into());
            return;
        };
        let ini = Ini::parse(&text);
        // A file without a mode (or with one that is not a number) does not invent
        // automatic lowering: the engine's own behaviour is the weapon staying up.
        let mode = ini.f32("settings", "iwlmode").map(LoweringMode::parse).unwrap_or(LoweringMode::Hotkey);
        self.lowering = Some(Lowering { mode, key: ini.f32("settings", "iwlkey"), sound: ini.bool("settings", "iwlsound") });
        self.report.push(format!(
            "FPS Weapon Lowering {}: iWLMode {mode:?} (automatic lowering {}), key {:?}, sound {:?}; the offset numbers are ours (sprint ×{SPRINT_SHARE}, idle ×{IDLE_SHARE} after {IDLE_SECONDS} s): the mod ships none",
            path,
            if mode.automatic() { "on" } else { "off" },
            self.lowering.and_then(|l| l.key),
            self.lowering.and_then(|l| l.sound),
        ));
        match read_first(vfs, FPS_LOWERING_JSON) {
            Some((p, _)) => self.report.push(format!("FPS Weapon Lowering: {p} has offset numbers: read, but the pose still uses ours (it is a clip list here)")),
            None => self.report.push("FPS Weapon Lowering: no AnimGroupOverride JSON in the overlay (the mod lowers by playing clips we do not have), so the pose is ours".into()),
        }
    }

    /// Smooth True Ironsights: report, do not invent. Its file is a settings-menu
    /// description (sliders with a range) and carries no default values.
    fn load_smooth_ironsights(&mut self, vfs: &fnv_vfs::Vfs) {
        let Some((path, text)) = read_first(vfs, SMOOTH_IRONSIGHTS) else {
            self.report.push("Smooth True Ironsights: no JSON; nothing to apply".into());
            return;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
            self.report.push(format!("Smooth True Ironsights: {path} is not JSON"));
            return;
        };
        let mut defaults = 0;
        let mut sliders = 0;
        let mut walk = vec![&json];
        while let Some(v) = walk.pop() {
            match v {
                serde_json::Value::Object(o) => {
                    if o.contains_key("minValue") || o.contains_key("maxValue") {
                        sliders += 1;
                    }
                    if o.contains_key("defaultValue") || o.contains_key("default") {
                        defaults += 1;
                    }
                    walk.extend(o.values());
                }
                serde_json::Value::Array(a) => walk.extend(a),
                _ => {}
            }
        }
        self.report.push(format!(
            "Smooth True Ironsights {}: {sliders} slider(s), {defaults} default value(s) — {}",
            path,
            if defaults == 0 { "no defaults shipped, nothing to apply" } else { "defaults present, but the aim transition is the engine's own (iron_sights.rs), not mapped" },
        ));
    }
}

pub struct WeaponFeelPlugin {
    pub on: bool,
}

impl Plugin for WeaponFeelPlugin {
    fn build(&self, app: &mut App) {
        // The overlay is the run's own file system (`ArchivePlugin` inserted it before
        // this plugin): a config file is read from the same layer as an asset, so the
        // highest-precedence mod that has it wins.
        let mut feel = match app.world().get_resource::<nvf_assets::GameData>() {
            Some(data) => WeaponFeel::load(&data.vfs, self.on),
            None => WeaponFeel { enabled: self.on, ..WeaponFeel::default() },
        };
        if feel.report.is_empty() {
            feel.report.push("no game data mounted: nothing to read".into());
        }
        for line in &feel.report {
            info!("weapon feel: {line}");
        }
        app.insert_resource(feel).add_systems(Update, apply.in_set(nvf_play::PlaySet::Input));
    }
}

/// Hand the reload-safe parts of the config to the systems that use them. The sway and the
/// lowering read [`WeaponFeel`] every frame themselves; combat holds its own copy of the
/// recoil profile, rewritten only when the switch changes (the profile itself is read once,
/// at start-up).
fn apply(feel: Res<WeaponFeel>, combat: Option<ResMut<nvf_play::combat::Combat>>, mut last: Local<Option<bool>>) {
    let Some(mut combat) = combat else { return };
    if *last == Some(feel.enabled) {
        return;
    }
    *last = Some(feel.enabled);
    combat.view_recoil = if feel.enabled { feel.recoil.clone() } else { None };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh overlay for one test: one overlay root, as `Vfs::open` indexes it, holding
    /// the mod-relative paths themselves (`Config/B42Inertia.ini` → `config\b42inertia.ini`).
    fn overlay(name: &str, files: &[(&str, &str)]) -> fnv_vfs::Vfs {
        let dir = std::env::temp_dir().join(format!("nvf-weapon-feel-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (rel, text) in files {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        std::fs::create_dir_all(&dir).unwrap();
        fnv_vfs::Vfs::open(&dir, &[]).unwrap()
    }

    const B42: &str = "[Tuning]\nfIntensityMultVertical=0.6\nfIntensityMultHorizontal=0.4\nfCenteringSpeed=1\nfSpeedMult=5\nbRotationMode=1\nsPivot=rear\n";

    #[test]
    fn b42_inertia_maps_its_numbers_onto_the_sway() {
        let vfs = overlay("b42", &[("Config/B42Inertia.ini", B42)]);
        let feel = WeaponFeel::load(&vfs, true);
        assert!((feel.sway.intensity_v - 0.6).abs() < 1e-6);
        assert!((feel.sway.intensity_h - 0.4).abs() < 1e-6);
        // The mod's own defaults (5, 1) are the engine's constants exactly.
        assert!((feel.sway.turn_chase - SwayTuning::default().turn_chase).abs() < 1e-6);
        assert!((feel.sway.rest_chase - SwayTuning::default().rest_chase).abs() < 1e-6);
        let line = feel.report.iter().find(|l| l.starts_with("B42 Inertia")).unwrap();
        assert!(line.contains("brotationmode"), "the unmapped keys are named: {line}");
        assert!(line.contains("spivot"), "{line}");
    }

    #[test]
    fn b42_rates_change_the_time_constants_and_defaults_stand_when_absent() {
        let vfs = overlay("b42rates", &[("Config/B42Inertia.ini", "[Tuning]\nfSpeedMult=10\nfCenteringSpeed=2\n")]);
        let feel = WeaponFeel::load(&vfs, true);
        assert!((feel.sway.turn_chase - SwayTuning::default().turn_chase / 2.0).abs() < 1e-6);
        assert!((feel.sway.rest_chase - SwayTuning::default().rest_chase / 2.0).abs() < 1e-6);
        // The intensities are absent: 1, the engine's.
        assert_eq!(feel.sway.intensity_v, 1.0);
        assert_eq!(feel.sway.intensity_h, 1.0);
    }

    #[test]
    fn a_bad_value_leaves_the_engines_constant_and_says_so() {
        let vfs = overlay("b42bad", &[("Config/B42Inertia.ini", "[Tuning]\nfIntensityMultVertical=-3\nfIntensityMultHorizontal=nonsense\nfSpeedMult=0\n")]);
        let feel = WeaponFeel::load(&vfs, true);
        assert_eq!(feel.sway, SwayTuning::default(), "a negative intensity, an unreadable one and a zero rate are all refused");
        assert!(feel.report.iter().any(|l| l.contains("fIntensityMultHorizontal missing")), "{:?}", feel.report);
    }

    #[test]
    fn an_empty_overlay_keeps_every_default() {
        let vfs = overlay("empty", &[]);
        let feel = WeaponFeel::load(&vfs, true);
        assert!(feel.recoil.is_none() && feel.lowering.is_none());
        assert_eq!(feel.sway, SwayTuning::default());
        assert!(feel.report.iter().any(|l| l.contains("B42 Inertia: no")), "{:?}", feel.report);
        assert!(feel.report.iter().any(|l| l.contains("the switch is on")), "{:?}", feel.report);
    }

    #[test]
    fn viewmodel_recoil_is_found_by_edid_and_a_weapon_without_a_file_keeps_the_engines() {
        let vfs = overlay(
            "recoil",
            &[
                ("Config/ProfilesVRecoil/Default.ini", "[General]\nvVerticalIntensity=0.4\n[Individual]\nvUseIndividualRecoil=1\n"),
                ("Config/WeaponVRecoil/Weap10mmPistol.ini", "[PATTERN]\nX=1\nY=2\nZ=-4\ndX=-0.1\ndY=0.9\ndZ=0.9\ncIS=0.5\n"),
                ("Config/WeaponVRecoil/TEMPLATE.ini", "[PATTERN]\nX=-2\nY=3\nZ=1\n"),
                ("Config/WeaponVRecoil/WeapBroken.ini", "[PATTERN]\nY=not-a-number\n"),
            ],
        );
        let feel = WeaponFeel::load(&vfs, true);
        let p = feel.recoil.as_ref().unwrap();
        assert_eq!(p.patterns.len(), 1, "TEMPLATE and the unreadable one are not weapons: {:?}", p.patterns.keys());
        assert!(p.pattern(Some("Weap10mmPistol")).is_some());
        assert!(p.pattern(Some("weap10mmpistol")).is_some(), "EDIDs match without regard to case");
        assert!(p.kick(Some("WeapNV9mmPistol"), 10.0, false, &mut || 0.5).is_none(), "no file: the engine's recoil");
        let line = feel.report.iter().find(|l| l.starts_with("Viewmodel Recoil")).unwrap();
        assert!(line.contains("1 of 2 weapon pattern file(s) read"), "{line}");
    }

    #[test]
    fn fps_lowering_reads_its_mode_from_the_fomod_layout_the_install_has() {
        let vfs = overlay("lowering", &[("[MAIN]/config/FPS WL/FPSWL Config.ini", "[SETTINGS]\niWLKey=21\niWLMode=2\niWLSound=1\n")]);
        let feel = WeaponFeel::load(&vfs, true);
        let l = feel.lowering.unwrap();
        assert_eq!(l.mode, LoweringMode::Complex);
        assert!(l.mode.automatic());
        assert_eq!((l.key, l.sound), (Some(21.0), Some(true)));
        assert!(feel.report.iter().any(|r| r.contains("no AnimGroupOverride JSON")));
    }

    #[test]
    fn the_sprint_and_idle_state_drives_the_lowering_share() {
        let l = Lowering { mode: LoweringMode::Complex, key: None, sound: None };
        assert_eq!(l.state_share(300.0, false, false, 0.0), SPRINT_SHARE);
        assert_eq!(l.state_share(300.0, true, false, 0.0), 0.0, "sneaking is not running");
        assert_eq!(l.state_share(300.0, false, true, 0.0), 0.0, "aiming keeps it up");
        assert_eq!(l.state_share(85.0, false, false, IDLE_SECONDS - 0.1), 0.0, "walking is neither");
        assert_eq!(l.state_share(0.0, false, false, IDLE_SECONDS + 0.1), IDLE_SHARE);
        let hotkey = Lowering { mode: LoweringMode::Hotkey, ..l };
        assert_eq!(hotkey.state_share(300.0, false, false, 99.0), 0.0, "hotkey mode lowers by the key alone");
        let auto = Lowering { mode: LoweringMode::Automatic, ..l };
        assert_eq!(auto.state_share(300.0, false, false, 0.0), SPRINT_SHARE);
    }

    #[test]
    fn a_zero_mode_is_the_hotkey_only() {
        let vfs = overlay("lowering0", &[("Config/FPS Weapon Lowering.ini", "[SETTINGS]\niWLMode=0\n")]);
        let feel = WeaponFeel::load(&vfs, true);
        assert_eq!(feel.lowering.unwrap().mode, LoweringMode::Hotkey);
    }

    #[test]
    fn a_missing_mode_does_not_invent_automatic_lowering() {
        let vfs = overlay("loweringmiss", &[("Config/FPS Weapon Lowering.ini", "[SETTINGS]\niWLKey=21\niWLMode=two\n")]);
        let feel = WeaponFeel::load(&vfs, true);
        assert_eq!(feel.lowering.unwrap().mode, LoweringMode::Hotkey, "an unreadable mode leaves the weapon up, as before");
    }

    #[test]
    fn smooth_ironsights_reports_that_no_defaults_ship() {
        let vfs = overlay(
            "smooth",
            &[("NVSE/Plugins/Tweaks/MenuConfig/SmoothTrueIronsights.json", r#"[{"name":"Smooth","subsettings":[{"internalName":"iAimTransitionTimeMS","minValue":0,"maxValue":500}]}]"#)],
        );
        let feel = WeaponFeel::load(&vfs, true);
        let line = feel.report.iter().find(|l| l.starts_with("Smooth True Ironsights")).unwrap();
        assert!(line.contains("1 slider(s), 0 default value(s)"), "{line}");
        assert!(line.contains("no defaults shipped, nothing to apply"), "{line}");
    }

    #[test]
    fn the_switch_off_changes_nothing_though_the_files_are_read() {
        let vfs = overlay("off", &[("Config/B42Inertia.ini", B42)]);
        let off = WeaponFeel::load(&vfs, false);
        assert!(!off.enabled);
        assert!((off.sway.intensity_v - 0.6).abs() < 1e-6, "the numbers are still read");
        assert!(off.report.iter().any(|l| l.contains("the switch is off")), "{:?}", off.report);
    }

    /// The per-frame cost of the whole feature, and the one-off cost of reading the files:
    /// one minute of frames at 60 fps, before (the engine's constants) and after (the
    /// mods' numbers), repeated; then 100 loads. A measurement, not a check:
    /// `cargo test --release -- --ignored --nocapture per_frame_cost`.
    #[test]
    #[ignore = "a measurement, run with --ignored"]
    fn per_frame_cost() {
        use crate::view_sway::{SwayInput, ViewSway};
        use crate::weapon_lower::{LowerInput, WeaponLower};
        use fnv_combat::spread::Kick;
        use fnv_combat::tuning::CombatTuning;
        use std::time::Instant;

        // A representative overlay: the real profile, as many pattern files as the
        // installed mod ships (60), and the lowering INI in the layout the overlay has.
        let pattern = "[PATTERN]\nX=1\nY=2\nZ=-4\ndX=-0.1\ndY=0.9\ndZ=0.9\ncIS=0.5\n";
        let mut files: Vec<(String, &str)> = vec![
            ("Config/ProfilesVRecoil/Default.ini".into(), "[General]\nvVerticalIntensity=0.4\nvHorizontalIntensity=0.01\n"),
            ("[MAIN]/config/FPS WL/FPSWL Config.ini".into(), "[SETTINGS]\niWLMode=2\n"),
            ("Config/B42Inertia.ini".into(), B42),
        ];
        for i in 0..60 {
            files.push((format!("Config/WeaponVRecoil/WeapTest{i}.ini"), pattern));
        }
        let refs: Vec<(&str, &str)> = files.iter().map(|(a, b)| (a.as_str(), *b)).collect();
        let vfs = overlay("cost", &refs);
        let feel = WeaponFeel::load(&vfs, true);
        assert_eq!(feel.recoil.as_ref().unwrap().patterns.len(), 60);
        let profile = feel.recoil.clone().expect("a profile");
        let lowering = feel.lowering.expect("a mode");
        let w = fnv_combat::data::WeaponStats { edid: Some("WeapTest0".into()), base_damage: 16, ..Default::default() };
        let t = CombatTuning::default();
        let frames = 60 * 60;
        let runs = 20;

        let mut time = |mods: bool| {
            let mut sway = ViewSway::default();
            let mut lower = WeaponLower::default();
            let mut kick = Kick::default();
            let mut rnd = 0.5f32;
            if mods {
                sway.tuning = feel.sway;
            }
            let mut h = 0.0;
            let start = Instant::now();
            for i in 0..frames {
                h += 0.001;
                sway.update(SwayInput { dt: 1.0 / 60.0, heading: h, pitch: 0.0, velocity: Vec2::new(0.0, 300.0), phase: (i % 60) as f32 / 60.0, sights: false, sneaking: false, airborne: false, on: true });
                let share = if mods { lowering.state_share(300.0, false, false, 0.0) } else { 0.0 };
                lower.update(LowerInput { dt: 1.0 / 60.0, state_share: share, on: true, ..Default::default() });
                if mods {
                    kick.shot_with(&t, &w, 0.1, Some(&profile), false, &mut || { rnd = 0.5 + 0.25 * rnd; rnd });
                } else {
                    kick.shot(&t, &w, 0.1, &mut || { rnd = 0.5 + 0.25 * rnd; rnd });
                }
            }
            start.elapsed().as_secs_f64() * 1e6 / frames as f64
        };
        // Warm both paths once, then take the best of `runs`: a million frames is a
        // millisecond of work, so a single sample is all noise.
        let _ = time(false);
        let _ = time(true);
        let (mut before, mut after) = (f64::MAX, f64::MAX);
        for _ in 0..runs {
            before = before.min(time(false));
            after = after.min(time(true));
        }
        let start = Instant::now();
        for _ in 0..100 {
            let _ = WeaponFeel::load(&vfs, true);
        }
        let load = start.elapsed().as_secs_f64() * 1e3 / 100.0;
        println!(
            "weapon feel cost: {before:.4} µs/frame before, {after:.4} µs/frame after ({:+.4} µs), {load:.3} ms to read and parse {} files at start-up",
            after - before,
            files.len()
        );
        assert!(after < 20.0, "a handful of vector ops: {after} µs/frame");
        assert!(load < 50.0, "71 small files: {load} ms");
    }
}
