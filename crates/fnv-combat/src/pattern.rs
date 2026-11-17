//! Viewmodel Recoil's own per-weapon patterns, read from the mod's INI files and turned
//! into the view kick [`crate::spread::Kick`] applies.
//!
//! The mod ships two kinds of file (installed under `mods/ViewmodelRecoil/Config/`):
//!
//! - `ProfilesVRecoil/Default.ini`: the global intensities, limits, damage coefficients,
//!   ironsight terms, the `[Stats]` block and the randomization switches.
//! - `WeaponVRecoil/<EDID>.ini`: one weapon's `[PATTERN]` — `X`, `Y`, `Z`, `dX`, `dY`,
//!   `dZ`, `cIS`.
//!
//! **What the numbers mean** (from the mod's own comments in `WeaponVRecoil/TEMPLATE.ini`,
//! which ship with it; the values are the mod's): `X` and `Y` are the shot's sideways and
//! upward movement, `Z` its rotation, `dX`/`dY`/`dZ` how far each is randomized and which
//! way, `cIS` the multiplier down the sights. The template comments say, of `dY`,
//! "positive give you Y + dY … negative value will give you a chance to have Y = 0", and of
//! `dX`, "positive is just randomization in one direction … `dX=-0.2` … randomization in
//! both directions". That is the arithmetic below.
//!
//! **What we map them onto** [`crate::spread::ViewKick`] (pitch up, yaw) [design]:
//!
//! - `X` → yaw, `Y` → pitch. The mod's own global keys are `vVerticalIntensity` and
//!   `vHorizontalIntensity`, so `Y` is read as the vertical one and `X` as the
//!   horizontal [hypothesis: the pairing is by the mod's own key names, not stated in the
//!   template].
//! - The pattern value is clamped to the profile's own `…LowerLimit`/`…UpperLimit` first
//!   ("the values are absolute and taken before any randomization" [quoted: Default.ini]),
//!   then randomized, then scaled: `degrees = pattern × intensity × (1 + coefficient ×
//!   damage)`, and down the sights `× cIS`.
//! - `Z` (rotation) has **no counterpart**: the view is heading and pitch only, with no
//!   roll to turn (`crate::spread::ViewKick`), so it is read, kept and not applied.
//!
//! **Where the kick lands**: the mod moves the *viewmodel*; this engine's recoil moves the
//! player's view (`nvf_play::combat`'s `Kick`), which is the code the design note says to
//! use. So the pattern's degrees are the view's climb, not a separate model offset — the
//! engine has no second recoil channel, and inventing one is not this change.
//!
//! Every one of those terms is what the file says; nothing here invents a number. A weapon
//! with no file gets `None` from [`RecoilProfile::kick`] and the caller keeps the engine's
//! own recoil exactly as it was (parity).

use crate::spread::ViewKick;
use std::collections::HashMap;

/// One weapon's `[PATTERN]` (`Config/WeaponVRecoil/<EDID>.ini`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ShotPattern {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub dx: f32,
    pub dy: f32,
    pub dz: f32,
    /// The down-the-sights multiplier; `None` when the file has no `cIS`, so the
    /// profile's `vConstantCoefficient` is used instead.
    pub cis: Option<f32>,
}

impl ShotPattern {
    /// Whether the file said anything at all (an empty or `[PATTERN]`-less file is not a
    /// pattern, and the weapon keeps the engine's own recoil).
    pub fn is_set(&self) -> bool {
        self.x != 0.0 || self.y != 0.0 || self.z != 0.0
    }
}

/// `Config/ProfilesVRecoil/Default.ini`: the global side of the mod.
#[derive(Debug, Clone, PartialEq)]
pub struct RecoilProfile {
    pub vertical_intensity: f32,
    pub horizontal_intensity: f32,
    /// `[General]` limits, in pattern units, before randomization.
    pub vertical_lower: f32,
    pub vertical_upper: f32,
    pub horizontal_lower: f32,
    pub horizontal_upper: f32,
    pub damage_coeff: f32,
    pub damage_coeff_h: f32,
    /// `[Ironsights]`.
    pub ironsights_limits: f32,
    pub ironsights_constant: f32,
    /// `[Individual] vUseIndividualRecoil`: 0 ignores the weapon files.
    pub use_individual: bool,
    /// `[Randomization] vIsRandomized`: 0 fixes the randomization at half way.
    pub randomized: bool,
    /// `[Pattern]` files by EDID, lowercased.
    pub patterns: HashMap<String, ShotPattern>,
}

impl Default for RecoilProfile {
    /// The values `ViewmodelRecoil/Config/ProfilesVRecoil/Default.ini` ships
    /// [observed: the installed file]. They are what the loader falls back to for a key
    /// the file lacks or spells unreadably.
    fn default() -> Self {
        RecoilProfile {
            vertical_intensity: 0.4,
            horizontal_intensity: 0.01,
            vertical_lower: 0.6,
            vertical_upper: 4.4,
            horizontal_lower: 0.2,
            horizontal_upper: 3.4,
            damage_coeff: 0.07,
            damage_coeff_h: 0.02,
            ironsights_limits: 0.1,
            ironsights_constant: 0.1,
            use_individual: true,
            randomized: true,
            patterns: HashMap::new(),
        }
    }
}

impl RecoilProfile {
    /// The pattern for a weapon, by EDID (case-insensitive).
    pub fn pattern(&self, edid: Option<&str>) -> Option<&ShotPattern> {
        if !self.use_individual {
            return None;
        }
        let edid = edid?;
        self.patterns.get(&edid.to_ascii_lowercase()).filter(|p| p.is_set())
    }

    /// The kick one shot of weapon `edid` puts on the view, or `None` when the weapon has
    /// no pattern file (the caller then keeps its own recoil). `damage` is the weapon's
    /// base damage, `aiming` down the sights, `rnd` uniform in [0, 1).
    pub fn kick(&self, edid: Option<&str>, damage: f32, aiming: bool, rnd: &mut impl FnMut() -> f32) -> Option<ViewKick> {
        let p = *self.pattern(edid)?;
        let limits = if aiming { self.ironsights_limits } else { 1.0 };
        let y = clamp_mag(p.y, self.vertical_lower * limits, self.vertical_upper * limits);
        let x = clamp_mag(p.x, self.horizontal_lower * limits, self.horizontal_upper * limits);
        // d >= 0: one way (grows); d < 0: both ways, and for the vertical axis a chance
        // of no climb at all ("a negative value will give you a chance to have Y = 0").
        let roll = |d: f32, v: f32, rnd: &mut dyn FnMut() -> f32| {
            let u = if self.randomized { rnd() } else { 0.5 };
            if d >= 0.0 {
                v * (1.0 + d * u)
            } else {
                v * (1.0 - (-d).min(1.0) * (2.0 * u - 1.0))
            }
        };
        let y = if p.dy < 0.0 {
            let u = if self.randomized { rnd() } else { 0.5 };
            if u < (-p.dy).min(1.0) {
                0.0
            } else {
                roll(p.dy, y, rnd)
            }
        } else {
            roll(p.dy, y, rnd)
        };
        let x = roll(p.dx, x, rnd);
        let dmg_v = 1.0 + self.damage_coeff * damage.max(0.0);
        let dmg_h = 1.0 + self.damage_coeff_h * damage.max(0.0);
        let ads = if aiming { p.cis.unwrap_or(self.ironsights_constant) } else { 1.0 };
        Some(ViewKick { pitch_deg: y * self.vertical_intensity * dmg_v * ads, yaw_deg: x * self.horizontal_intensity * dmg_h * ads })
    }
}

/// `v` with its magnitude inside `[lo, hi]`, keeping its sign.
fn clamp_mag(v: f32, lo: f32, hi: f32) -> f32 {
    let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
    let m = v.abs().clamp(lo, hi);
    if v < 0.0 {
        -m
    } else {
        m
    }
}

/// A `key=value` reader for the mods' INI files: `;` and `#` comments, `[Section]`
/// headers, keys case-insensitive, the last of a repeated key winning.
#[derive(Debug, Default, Clone)]
pub struct Ini {
    sections: Vec<(String, Vec<(String, String)>)>,
}

impl Ini {
    pub fn parse(text: &str) -> Ini {
        let mut ini = Ini::default();
        let mut section = String::new();
        ini.sections.push((section.clone(), Vec::new()));
        for raw in text.lines() {
            let line = raw.split([';', '#']).next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                section = name.trim().to_ascii_lowercase();
                ini.sections.push((section.clone(), Vec::new()));
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let key = k.trim().to_ascii_lowercase();
            if key.is_empty() {
                continue;
            }
            let value = v.trim().to_string();
            let s = ini.sections.last_mut().expect("a section is always open");
            match s.1.iter_mut().find(|(have, _)| *have == key) {
                Some(slot) => slot.1 = value,
                None => s.1.push((key, value)),
            }
        }
        ini
    }

    /// The raw text of `key` in `section` (`None` when the section or key is absent).
    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        let section = section.to_ascii_lowercase();
        let key = key.to_ascii_lowercase();
        self.sections.iter().find(|(s, _)| *s == section)?.1.iter().find(|(k, _)| *k == key).map(|(_, v)| v.as_str())
    }

    /// `key` as a finite number; a missing key or an unreadable value is `None`.
    pub fn f32(&self, section: &str, key: &str) -> Option<f32> {
        self.get(section, key).and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| v.is_finite())
    }

    /// `key` as the mods write switches: `1`/`0`, `true`/`false`.
    pub fn bool(&self, section: &str, key: &str) -> Option<bool> {
        match self.get(section, key)?.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        }
    }

    /// Every key of `section`, lowercased, in file order (a section's keys with no reader
    /// are reported rather than silently dropped).
    pub fn keys(&self, section: &str) -> Vec<String> {
        let section = section.to_ascii_lowercase();
        self.sections.iter().filter(|(s, _)| *s == section).flat_map(|(_, kv)| kv.iter().map(|(k, _)| k.clone())).collect()
    }
}

/// `Config/WeaponVRecoil/<EDID>.ini`: the `[PATTERN]` block. `None` when the file holds no
/// movement at all (an empty or comment-only file, or one without `X`/`Y`/`Z`).
pub fn parse_pattern(text: &str) -> Option<ShotPattern> {
    let ini = Ini::parse(text);
    let p = ShotPattern {
        x: ini.f32("pattern", "x")?,
        y: ini.f32("pattern", "y")?,
        z: ini.f32("pattern", "z").unwrap_or(0.0),
        dx: ini.f32("pattern", "dx").unwrap_or(0.0),
        dy: ini.f32("pattern", "dy").unwrap_or(0.0),
        dz: ini.f32("pattern", "dz").unwrap_or(0.0),
        cis: ini.f32("pattern", "cis"),
    };
    p.is_set().then_some(p)
}

/// `Config/ProfilesVRecoil/Default.ini`: the global profile, with [`RecoilProfile::default`]
/// standing in for every key the text lacks or spells unreadably.
pub fn parse_profile(text: &str) -> RecoilProfile {
    let ini = Ini::parse(text);
    let d = RecoilProfile::default();
    let f = |section: &str, key: &str, fallback: f32| ini.f32(section, key).unwrap_or(fallback);
    RecoilProfile {
        vertical_intensity: f("general", "vverticalintensity", d.vertical_intensity),
        horizontal_intensity: f("general", "vhorizontalintensity", d.horizontal_intensity),
        vertical_lower: f("general", "vverticallowerlimit", d.vertical_lower),
        vertical_upper: f("general", "vverticalupperlimit", d.vertical_upper),
        horizontal_lower: f("general", "vhorizontallowerlimit", d.horizontal_lower),
        horizontal_upper: f("general", "vhorizontalupperlimit", d.horizontal_upper),
        damage_coeff: f("general", "vweapondamagecoeff", d.damage_coeff),
        damage_coeff_h: f("general", "vweapondamagecoeffh", d.damage_coeff_h),
        ironsights_limits: f("ironsights", "vironsightslimitsmultiplier", d.ironsights_limits),
        ironsights_constant: f("ironsights", "vconstantcoefficient", d.ironsights_constant),
        use_individual: ini.bool("individual", "vuseindividualrecoil").unwrap_or(d.use_individual),
        randomized: ini.bool("randomization", "visrandomized").unwrap_or(d.randomized),
        patterns: HashMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The values and keys of `ViewmodelRecoil/Config/ProfilesVRecoil/Default.ini` as the
    /// overlay ships it [observed: the installed file]; the numbers are facts about the
    /// mod, so the test carries them rather than the mod's own text.
    const DEFAULT_INI: &str = "\
[Optimization]
vDirectLoad=1
[General]
vVerticalIntensity=0.4
vHorizontalIntensity=0.01
vVerticalLowerLimit=0.6
vVerticalUpperLimit=4.4
vHorizontalLowerLimit=0.2
vHorizontalUpperLimit=3.4
vWeaponDamageCoeff=0.07
vWeaponDamageCoeffH=0.02
[Stats]
vStrengthCoeffV=0.5
vStrengthCoeffH=0.05
vWeightInfluenceV=0.3
vWeightInfluenceH=0.02
vUseAmmoPseudoCalc=1
[Ironsights]
vConstantCoefficient=0.1
vIronsightsSkillBonus=20
vIronsightsLimitsMultiplier=0.1
[Randomization]
vIsRandomized=1
vRotationAsVertical=1
vFirstShotMultiplierV=0.5
vFirstShotMultiplierH=0.1
vFirstShotPeriod=10
vFirstShotRandomization=0.0
[Individual]
vUseIndividualRecoil=1
vUseOldGenericFormula=0
[Movements]
vTiltCoefficient=0.5
vTiltConstant=0
vRotCoefficient=1
vRotConstant=1
[BUGFIXING]
vVATSfix=1
vDebugAmmo=0
";
    const TEN_MM: &str = "[PATTERN]\nX=1\nY=2\nZ=-4\ndX=-0.1\ndY=0.9\ndZ=0.9\ncIS=0.5\n";

    fn profile() -> RecoilProfile {
        let mut p = parse_profile(DEFAULT_INI);
        p.patterns.insert("weap10mmpistol".into(), parse_pattern(TEN_MM).unwrap());
        p
    }

    /// The same profile with the randomization settled at its middle, for an exact
    /// expected number.
    fn fixed() -> RecoilProfile {
        RecoilProfile { randomized: false, ..profile() }
    }

    fn rng(seed: u64) -> impl FnMut() -> f32 {
        // The engine's own SplitMix numbers are not needed: a deterministic, uniform
        // sequence is all this test reads.
        let mut s = seed;
        move || {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((s >> 40) as f32) / (1u64 << 24) as f32
        }
    }

    #[test]
    fn a_pattern_is_read_and_a_missing_key_stays_zero() {
        let p = parse_pattern("[PATTERN]\nX=1\nY=2\nZ=-4\n").unwrap();
        assert_eq!((p.x, p.y, p.z), (1.0, 2.0, -4.0));
        assert_eq!((p.dx, p.dy, p.dz), (0.0, 0.0, 0.0));
        assert_eq!(p.cis, None);
    }

    #[test]
    fn a_bad_value_is_not_a_number_and_leaves_the_default() {
        let p = parse_profile("[General]\nvVerticalIntensity=banana\nvHorizontalIntensity=0.02\n");
        assert_eq!(p.vertical_intensity, RecoilProfile::default().vertical_intensity);
        assert_eq!(p.horizontal_intensity, 0.02);
        assert!(!p.vertical_intensity.is_nan());
    }

    #[test]
    fn a_file_with_no_movement_is_no_pattern() {
        assert_eq!(parse_pattern("; nothing here\n[PATTERN]\nX=0\nY=0\nZ=0\n"), None);
        assert_eq!(parse_pattern(""), None);
        assert_eq!(parse_pattern("[PATTERN]\nY=2\n"), None, "X and Y are both required");
    }

    #[test]
    fn the_installed_default_profile_reads_as_its_values() {
        let p = parse_profile(DEFAULT_INI);
        assert_eq!(p, RecoilProfile::default());
        assert_eq!(p.vertical_intensity, 0.4);
        assert_eq!(p.vertical_upper, 4.4);
        assert_eq!(p.ironsights_limits, 0.1);
        assert!(p.use_individual);
    }

    #[test]
    fn a_weapon_without_a_file_gets_no_kick_of_its_own() {
        let p = profile();
        assert!(p.kick(Some("WeapNV9mmPistol"), 10.0, false, &mut rng(1)).is_none());
        assert!(p.kick(None, 10.0, false, &mut rng(1)).is_none());
    }

    #[test]
    fn the_10mm_pattern_kicks_up_two_degrees_times_the_intensity() {
        let p = fixed();
        // Y=2, damage 0, randomization settled at its middle — half of dY=0.9:
        // 2 × (1 + 0.9 × 0.5) × 0.4 × (1 + 0.07 × 0) = 1.16.
        let k = p.kick(Some("Weap10mmPistol"), 0.0, false, &mut rng(1)).unwrap();
        assert!((k.pitch_deg - 1.16).abs() < 1e-5, "{k:?}");
        // X=1 with dX=-0.1, both ways around X: at its middle it is X itself, 0.01.
        assert!((k.yaw_deg - 0.01).abs() <= 0.001 + 1e-6, "{k:?}");
    }

    #[test]
    fn a_fixed_seed_and_pattern_keeps_every_shot_inside_the_bounds() {
        let p = profile();
        let mut r = rng(7);
        for i in 0..200 {
            let k = p.kick(Some("Weap10mmPistol"), 16.0, false, &mut r).unwrap();
            // Y plus up to dY of itself, times intensity and the damage term.
            let top = 2.0 * (1.0 + 0.9) * 0.4 * (1.0 + 0.07 * 16.0);
            let bottom = 2.0 * 1.0 * 0.4 * (1.0 + 0.07 * 16.0);
            assert!(k.pitch_deg >= bottom - 1e-4 && k.pitch_deg <= top + 1e-4, "shot {i}: {k:?}");
            let ytop = 1.0 * (1.0 + 0.1) * 0.01 * (1.0 + 0.02 * 16.0);
            assert!(k.yaw_deg.abs() <= ytop + 1e-6, "shot {i}: {k:?}");
        }
    }

    #[test]
    fn down_the_sights_the_pattern_shrinks_and_the_limits_with_it() {
        let p = fixed();
        let hip = p.kick(Some("Weap10mmPistol"), 0.0, false, &mut rng(3)).unwrap();
        let ads = p.kick(Some("Weap10mmPistol"), 0.0, true, &mut rng(3)).unwrap();
        assert!((hip.pitch_deg - 1.16).abs() < 1e-5, "{hip:?}");
        // The limits are absolute pattern units, so down the sights they become
        // 0.6 × 0.1 … 4.4 × 0.1: Y=2 lands on the upper one, 0.44, before the intensity and
        // the pattern's cIS 0.5 — 0.44 × 1.45 × 0.4 × 0.5 = 0.1276.
        assert!((ads.pitch_deg - 0.1276).abs() < 1e-5, "{ads:?}");
        assert!(ads.pitch_deg < hip.pitch_deg * 0.2);
    }

    #[test]
    fn a_negative_dy_may_take_the_whole_climb_away() {
        let mut p = profile();
        p.patterns.insert("weap10mmpistol".into(), ShotPattern { dy: -0.5, ..parse_pattern(TEN_MM).unwrap() });
        let mut r = rng(5);
        let mut zeros = 0;
        for _ in 0..200 {
            if p.kick(Some("Weap10mmPistol"), 0.0, false, &mut r).unwrap().pitch_deg == 0.0 {
                zeros += 1;
            }
        }
        assert!(zeros > 60 && zeros < 140, "dy=-0.5 zeroes the climb about half the time, got {zeros}");
        // -1 or below always takes it away.
        p.patterns.insert("weap10mmpistol".into(), ShotPattern { dy: -1.0, ..parse_pattern(TEN_MM).unwrap() });
        for _ in 0..20 {
            assert_eq!(p.kick(Some("Weap10mmPistol"), 0.0, false, &mut r).unwrap().pitch_deg, 0.0);
        }
    }

    #[test]
    fn without_individual_recoil_the_weapon_files_are_ignored() {
        let mut p = profile();
        p.use_individual = false;
        assert!(p.pattern(Some("Weap10mmPistol")).is_none());
        assert!(p.kick(Some("Weap10mmPistol"), 10.0, false, &mut rng(1)).is_none());
    }

    #[test]
    fn the_limits_clamp_a_pattern_before_it_is_randomized() {
        let mut p = profile();
        p.patterns.insert("weap10mmpistol".into(), ShotPattern { x: 0.0, y: 40.0, ..parse_pattern(TEN_MM).unwrap() });
        let k = p.kick(Some("Weap10mmPistol"), 0.0, false, &mut rng(1)).unwrap();
        let unit = 4.4 * 0.4;
        assert!(k.pitch_deg >= unit - 1e-4 && k.pitch_deg <= unit * 1.9 + 1e-3, "clamped to 4.4 first: {k:?}");
    }

    #[test]
    fn ini_keys_are_case_insensitive_and_comments_are_skipped() {
        let ini = Ini::parse("[Tuning]\n; a comment\nfIntensityMultVertical = 0.6 # trailing\n\nfSpeedMult=5\n");
        assert_eq!(ini.f32("TUNING", "FintensityMultVertical"), Some(0.6));
        assert_eq!(ini.f32("tuning", "fspeedmult"), Some(5.0));
        assert_eq!(ini.f32("tuning", "missing"), None);
    }
}
