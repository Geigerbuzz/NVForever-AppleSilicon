//! MCM Extender's `MCM\<name>.json` menus: the format, where they are found, and where a
//! changed value goes.
//!
//! MCM (the Mod Configuration Menu) draws its pages from MCM's own menu files and scripts;
//! MCM Extender lets a mod describe its page in JSON instead, and its Script Runner file
//! registers that page with MCM. NVForever runs both when they are installed
//! (`docs/spec-notes/mcm.md`), and draws the same JSON natively when they are not: one row
//! per mod with a menu, its sub pages, and its controls (`nvf_play::tiles::mcm`).
//!
//! **The format, as the installed menus show it** [observed: the five menus in the owner's
//! overlay, 2026-11-17]. Nothing here is read from MCM Extender's source; a key this module
//! does not place is kept in [`Source::unknown`] and named in the log rather than guessed
//! at.
//!
//! ```json
//! { "modName": "…", "displayName": "$key", "description": "$key",
//!   "saveFile": "Sub/Name.ini",            // Data\Config\Sub\Name.ini
//!   "minMCMVersion": 1.3, "minModVersion": 1.0,
//!   "requirements": [ {"type": "plugin"|"file", "file": "…"} ],
//!   "submenus": { "0": { "listTitle": "…", "pageTitle": "…", "columns": 1|2,
//!                        "enable": 1, "active": 1,
//!                        "state": {"form": …, "auxVar": …, "key": 0, "value": 1},
//!                        "options": { "1": { …control… } } } } }
//! ```
//!
//! A control: `title`, `type`, `enable`, `mouse` (its note), `vars`, and per type
//! `strings` (a list's words), `scale` (`valueDecimal`, `valueIncrement`, `valueMin`,
//! `valueMax`, `suffix`), `textOn`/`textOff`, `suffix`, `call`, and for a splash `image`
//! (`filename`, `width`, `height`, `offsetX`, `offsetY`, `systemcolor`) or the same fields
//! loose on the control. Types: 0 a section title (a splash with a `filename`), 1 a list of
//! `strings`, 2 a whole-number slider, 2.5 a decimal slider, 4 a checkbox, 5 a toggle,
//! 6 a toggle with its own words. Type 3 (a key binding, per MCM's guide, mcm.md §2.3)
//! appears in no installed menu and is drawn as a row that says so. A `type` beyond these
//! is kept as [`Kind::Unknown`].
//!
//! A variable (`vars[0]`) is how MCM Extender hands the value to the mod: `form` + `auxVar`
//! + `key` (an auxiliary variable of a form), `form` + `numVar` (a script variable) or
//! `uiTrait` (a trait of a drawn menu). None of the installed menus needs those for the
//! value the player sets: each carries `configINI` (`Section:Key`) and a `default`, and the
//! file it names in `saveFile` is the one the mod's own script reads through
//! `GetModINI`/`GetINI*` (`B42Inertia` reads `Tuning:fIntensityMultVertical` of
//! `B42Inertia.ini`; `PipBoyUITweaks` and `AnimatedGreetings` the same way). So the value
//! is read from and written to that INI, which is what makes the change real for a script.
//! The `form`/`auxVar`/`numVar`/`uiTrait` keys are understood and recorded, not acted on.
//!
//! **Where the file is written.** The mod's own folder in the mods layer
//! (`<mods>/<Mod>/Config/<saveFile>`, the copy the readers already reach), never the game's
//! Data ([`fnv_install::guard_write`]). The text stays readable this session through
//! [`crate::mod_files::ModFiles`], so `GetModINI` sees the new value in the same frame; the
//! ShowOff INI cache of that file is dropped for the same reason (a script that read it
//! with `GetINIFloat_Cached` would otherwise keep the old number).
//!
//! **`call`/`Call`** is `{"type": "udf", "value": "Name"}`: the user-defined function the
//! mod wants run after a change. [`run_call`] runs it by its editor ID in the same frame;
//! a type this module does not know is reported, not silently skipped.

use crate::game::Game;
use std::collections::HashMap;
use serde_json::Value as Json;
use std::path::{Path, PathBuf};

/// The folder a mod's menu lives in, under its own folder.
pub const DIR: &str = "MCM";

/// A control's kind, from its `type`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// 0: a section's title, chosen by nobody.
    Section,
    /// 1: a list of `strings`, the value its index.
    List,
    /// 2 or 2.5: a slider; `decimals` is the places shown (0 for type 2, 1 for 2.5).
    Slider { decimals: u8 },
    /// 3: a key binding (MCM's guide; no installed menu uses it).
    Key,
    /// 4: OFF/ON.
    Checkbox,
    /// 5: a check box.
    Toggle,
    /// 6: a toggle with its own words (`textOn`/`textOff`).
    LabelledToggle,
    /// A type this module does not place.
    Unknown(f64),
}

impl Kind {
    pub fn from_type(t: f64) -> Kind {
        match t {
            x if x == 0.0 => Kind::Section,
            x if x == 1.0 => Kind::List,
            x if x == 2.0 => Kind::Slider { decimals: 0 },
            x if x == 2.5 => Kind::Slider { decimals: 1 },
            x if x == 3.0 => Kind::Key,
            x if x == 4.0 => Kind::Checkbox,
            x if x == 5.0 => Kind::Toggle,
            x if x == 6.0 => Kind::LabelledToggle,
            x => Kind::Unknown(x),
        }
    }

    /// Whether a row of this kind is one the player changes.
    pub fn changes(self) -> bool {
        matches!(self, Kind::List | Kind::Slider { .. } | Kind::Checkbox | Kind::Toggle | Kind::LabelledToggle)
    }

    /// What the page says for a kind it does not draw.
    pub fn unsupported(self) -> Option<&'static str> {
        match self {
            Kind::Section | Kind::List | Kind::Slider { .. } | Kind::Checkbox | Kind::Toggle | Kind::LabelledToggle => None,
            Kind::Key => Some("a key binding: not drawn yet"),
            Kind::Unknown(_) => Some("a control type NVForever does not draw yet"),
        }
    }
}

/// A slider's steps and range (`scale`).
#[derive(Debug, Clone, PartialEq)]
pub struct Scale {
    pub decimals: u8,
    pub increment: f64,
    pub min: f64,
    pub max: f64,
    /// The words after the number (`scale.suffix`), when the control has none of its own.
    pub suffix: String,
}

/// One variable of a control (`vars[0]`): how MCM Extender hands it to the mod, and the
/// INI key the value lives in.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Var {
    pub form: Option<String>,
    pub aux_var: Option<String>,
    pub key: Option<f64>,
    pub num_var: Option<String>,
    pub ui_trait: Option<String>,
    /// `configINI` as `(section, key)`.
    pub config: Option<(String, String)>,
    /// `configINI` as the file spells it.
    pub config_raw: String,
    pub default: f64,
}

/// `call`/`Call`: the function a mod wants run after a change.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub kind: String,
    pub value: String,
}

/// One control of a page (`options`).
#[derive(Debug, Clone, PartialEq)]
pub struct Control {
    pub id: u32,
    /// Raw: a `$key` is translated ([`Source::text`]).
    pub title: String,
    pub kind: Kind,
    pub enable: bool,
    pub note: Option<String>,
    pub suffix: String,
    pub strings: Vec<String>,
    pub text_on: Option<String>,
    pub text_off: Option<String>,
    pub scale: Option<Scale>,
    pub var: Option<Var>,
    pub call: Option<Call>,
    /// A splash's picture (`image.filename`, or a loose `filename`).
    pub image: Option<String>,
    /// Keys of this control this module does not place.
    pub unknown: Vec<String>,
}

impl Control {
    /// The value the player starts from: the INI's, else the JSON's default.
    pub fn default(&self) -> f64 {
        self.var.as_ref().map_or(0.0, |v| v.default)
    }

    /// The INI `Section:Key` this control's value is kept in.
    pub fn config(&self) -> Option<(&str, &str)> {
        self.var.as_ref()?.config.as_ref().map(|(s, k)| (s.as_str(), k.as_str()))
    }

    /// Whether the row can be changed: an understood value kind with an INI key.
    pub fn editable(&self) -> bool {
        self.kind.changes() && self.config().is_some()
    }

    /// The value as the page shows it, before translation (the caller translates a list's
    /// words).
    pub fn show(&self, v: f64) -> String {
        match self.kind {
            Kind::Section => String::new(),
            Kind::List => self.strings.get(v.max(0.0) as usize).cloned().unwrap_or_else(|| format!("{}", v as i64)),
            Kind::Slider { decimals } => {
                let d = self.scale.as_ref().map_or(decimals, |s| s.decimals);
                let mut t = format!("{v:.d$}", d = d as usize);
                let suffix = if !self.suffix.is_empty() { self.suffix.clone() } else { self.scale.as_ref().map(|s| s.suffix.clone()).unwrap_or_default() };
                if !suffix.is_empty() {
                    t.push_str(&suffix);
                }
                t
            }
            Kind::Key => "a key".to_string(),
            Kind::Checkbox | Kind::Toggle => if v != 0.0 { "On" } else { "Off" }.to_string(),
            Kind::LabelledToggle => {
                let on = v != 0.0;
                self.text_on.as_ref().filter(|_| on).or_else(|| self.text_off.as_ref().filter(|_| !on)).cloned().unwrap_or_else(|| if on { "On" } else { "Off" }.to_string())
            }
            Kind::Unknown(t) => format!("type {t}"),
        }
    }

    /// `v` moved one `step` of `dir` (+1 or -1), within the control's own range: a list's
    /// choices wrap, a slider's steps clamp, a switch's sides are 0 and 1.
    pub fn stepped(&self, v: f64, dir: i32) -> f64 {
        match self.kind {
            Kind::List => {
                let n = self.strings.len().max(1) as i32;
                (((v as i32 + dir) % n + n) % n) as f64
            }
            Kind::Slider { decimals } => {
                let (inc, min, max, d) = match &self.scale {
                    Some(s) => (s.increment, s.min, s.max, s.decimals),
                    None => (1.0, f64::MIN, f64::MAX, decimals),
                };
                let inc = if inc == 0.0 { 1.0 } else { inc };
                let step = dir as f64 * inc;
                let x = ((v + step) / inc).round() * inc;
                let k = 10f64.powi(d as i32);
                ((x * k).round() / k).clamp(min, max)
            }
            Kind::Checkbox | Kind::Toggle | Kind::LabelledToggle => f64::from(u8::from(v == 0.0)),
            _ => v,
        }
    }

    /// The value written to the INI: a switch 0 or 1, a list its index, a slider its number
    /// with the places the slider shows.
    pub fn stored(&self, v: f64) -> String {
        match self.kind {
            Kind::List | Kind::Checkbox | Kind::Toggle | Kind::LabelledToggle => format!("{}", v as i64),
            Kind::Slider { decimals } => {
                let d = self.scale.as_ref().map_or(decimals, |s| s.decimals);
                format!("{v:.d$}", d = d as usize)
            }
            _ => format!("{v}"),
        }
    }
}

/// One of a page's `options`, by number.
#[derive(Debug, Clone, PartialEq)]
pub struct Submenu {
    pub id: u32,
    pub list_title: String,
    pub page_title: String,
    pub columns: usize,
    pub enable: bool,
    /// The page MCM opens first.
    pub active: bool,
    pub state: Option<Var>,
    /// Controls in `options` order (numbers ascending).
    pub controls: Vec<Control>,
    pub unknown: Vec<String>,
}

/// A mod's own page (`MCM/<name>.json`).
#[derive(Debug, Clone, PartialEq)]
pub struct Menu {
    pub mod_name: String,
    pub display_name: String,
    pub description: String,
    /// `Data\Config\<saveFile>`.
    pub save_file: String,
    pub min_mcm_version: Option<f64>,
    pub min_mod_version: Option<f64>,
    /// `requirements` as `(type, file)`.
    pub requirements: Vec<(String, String)>,
    pub submenus: Vec<Submenu>,
    pub unknown: Vec<String>,
}

/// A menu and where it came from: the mod folder that ships it, its file, and the
/// translations of its `$keys`.
#[derive(Debug, Clone)]
pub struct Source {
    /// The mod's own folder in the mods layer (`mods/<Mod>`).
    pub folder: PathBuf,
    /// The file's path under the folder (`MCM\X.json`).
    pub rel: String,
    pub menu: Menu,
    /// `$key` → its words, from `MCM/Translations/<name>.ini`.
    pub translations: HashMap<String, String>,
}

impl Source {
    /// The mod folder's name (the layer's own name for it).
    pub fn folder_name(&self) -> String {
        self.folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }

    /// The words of `raw`: a `$key` translated, else the key without its `$` (the page
    /// says something rather than a bare key).
    pub fn text(&self, raw: &str) -> String {
        match raw.strip_prefix('$') {
            Some(k) => self.translations.get(&format!("${k}")).or_else(|| self.translations.get(k)).cloned().unwrap_or_else(|| k.to_string()),
            None => raw.to_string(),
        }
    }

    /// The menu's name as the page shows it.
    pub fn name(&self) -> String {
        let n = self.text(&self.menu.display_name);
        if n.trim().is_empty() {
            if self.menu.mod_name.trim().is_empty() {
                self.folder_name()
            } else {
                self.menu.mod_name.clone()
            }
        } else {
            n
        }
    }

    /// The Data-relative file the values live in (`Config\<saveFile>` with `\` between its
    /// parts).
    pub fn config_rel(&self) -> Option<String> {
        let f = self.menu.save_file.replace('/', "\\");
        let f = f.trim_start_matches('\\');
        if f.is_empty() || f.contains(':') || f.split('\\').any(|p| p.is_empty() || p == "." || p == "..") {
            return None;
        }
        Some(if f.to_ascii_lowercase().starts_with("config\\") { f.to_string() } else { format!("Config\\{f}") })
    }

    /// The submenus in `active`-first order: the one MCM opens first, then by number.
    pub fn pages(&self) -> Vec<&Submenu> {
        let mut v: Vec<&Submenu> = self.menu.submenus.iter().filter(|s| s.enable).collect();
        v.sort_by_key(|s| (!s.active, s.id));
        v
    }
}

fn num(j: &Json) -> Option<f64> {
    match j {
        Json::Number(n) => n.as_f64(),
        Json::String(s) => s.trim().parse().ok(),
        Json::Bool(b) => Some(f64::from(u8::from(*b))),
        _ => None,
    }
}

fn string(j: &Json) -> Option<String> {
    match j {
        Json::String(s) => Some(s.clone()),
        Json::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn flag(j: &Json, key: &str, default: bool) -> bool {
    j.get(key).and_then(num).map_or(default, |v| v != 0.0)
}

fn section_key(raw: &str) -> Option<(String, String)> {
    let i = raw.find([':', '/', '\\'])?;
    let (s, k) = (raw[..i].trim(), raw[i + 1..].trim());
    (!s.is_empty() && !k.is_empty()).then(|| (s.to_string(), k.to_string()))
}

fn var_of(j: &Json) -> Var {
    let raw = j.get("configINI").and_then(string).unwrap_or_default();
    Var {
        form: j.get("form").and_then(string),
        aux_var: j.get("auxVar").and_then(string),
        key: j.get("key").and_then(num),
        num_var: j.get("numVar").and_then(string),
        ui_trait: j.get("uiTrait").and_then(string),
        config: section_key(&raw),
        config_raw: raw,
        default: j.get("default").and_then(num).unwrap_or(0.0),
    }
}

/// Control keys this module reads.
const CONTROL_KEYS: [&str; 20] = [
    "title", "type", "enable", "mouse", "suffix", "strings", "textOff", "textOn", "scale", "vars", "call", "Call", "image", "filename", "width", "height", "offsetX", "offsetY", "systemcolor", "state",
];
const SUBMENU_KEYS: [&str; 7] = ["listTitle", "pageTitle", "columns", "enable", "active", "state", "options"];
const MENU_KEYS: [&str; 8] = ["modName", "displayName", "description", "saveFile", "minMCMVersion", "minModVersion", "requirements", "submenus"];

fn control_of(id: u32, j: &Json) -> Control {
    let kind = Kind::from_type(j.get("type").and_then(num).unwrap_or(0.0));
    let scale = j.get("scale").map(|s| Scale {
        decimals: s.get("valueDecimal").and_then(num).unwrap_or(f64::from(match kind {
            Kind::Slider { decimals } => decimals,
            _ => 0,
        })) as u8,
        increment: s.get("valueIncrement").and_then(num).unwrap_or(1.0),
        min: s.get("valueMin").and_then(num).unwrap_or(f64::MIN),
        max: s.get("valueMax").and_then(num).unwrap_or(f64::MAX),
        suffix: s.get("suffix").and_then(string).unwrap_or_default(),
    });
    let suffix = j.get("suffix").and_then(string).unwrap_or_default();
    let image = j.get("image").and_then(|i| i.get("filename")).and_then(string).or_else(|| j.get("filename").and_then(string));
    let call = j.get("call").or_else(|| j.get("Call")).and_then(|c| {
        Some(Call { kind: c.get("type").and_then(string).unwrap_or_default(), value: c.get("value").and_then(string)? })
    });
    Control {
        id,
        title: j.get("title").and_then(string).unwrap_or_default(),
        kind,
        enable: flag(j, "enable", true),
        note: j.get("mouse").and_then(string).filter(|m| !m.trim().is_empty()),
        suffix,
        strings: j.get("strings").and_then(|s| s.as_array()).map(|a| a.iter().filter_map(string).collect()).unwrap_or_default(),
        text_on: j.get("textOn").and_then(string),
        text_off: j.get("textOff").and_then(string),
        scale,
        var: j.get("vars").and_then(|v| v.as_array()).and_then(|a| a.first()).map(var_of),
        call,
        image,
        unknown: j.as_object().map(|o| o.keys().filter(|k| !CONTROL_KEYS.iter().any(|c| c.eq_ignore_ascii_case(k))).cloned().collect()).unwrap_or_default(),
    }
}

/// A menu from its JSON text.
pub fn parse(text: &str) -> Result<Menu, String> {
    let root: Json = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("not JSON: {e}"))?;
    let obj = root.as_object().ok_or("the root is not an object")?;
    let mut submenus: Vec<Submenu> = Vec::new();
    let subs = obj.get("submenus").and_then(|s| s.as_object()).ok_or("no `submenus` object")?;
    for (sid, s) in subs {
        let id: u32 = sid.trim().parse().map_err(|_| format!("submenu {sid:?} is not a number"))?;
        let state = s.get("state").map(var_of);
        let mut controls: Vec<Control> = Vec::new();
        if let Some(opts) = s.get("options").and_then(|o| o.as_object()) {
            for (oid, o) in opts {
                let oid: u32 = oid.trim().parse().map_err(|_| format!("submenu {sid} option {oid:?} is not a number"))?;
                controls.push(control_of(oid, o));
            }
        }
        controls.sort_by_key(|c| c.id);
        submenus.push(Submenu {
            id,
            list_title: s.get("listTitle").and_then(string).unwrap_or_default(),
            page_title: s.get("pageTitle").and_then(string).unwrap_or_default(),
            columns: s.get("columns").and_then(num).unwrap_or(1.0).max(1.0) as usize,
            enable: flag(s, "enable", true),
            active: flag(s, "active", false),
            state,
            controls,
            unknown: s.as_object().map(|o| o.keys().filter(|k| !SUBMENU_KEYS.iter().any(|c| c.eq_ignore_ascii_case(k))).cloned().collect()).unwrap_or_default(),
        });
    }
    submenus.sort_by_key(|s| s.id);
    let requirements = obj
        .get("requirements")
        .and_then(|r| r.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|r| Some((r.get("type").and_then(string).unwrap_or_default(), r.get("file").and_then(string).unwrap_or_default())))
                .filter(|(t, f)| !t.is_empty() && !f.is_empty())
                .collect()
        })
        .unwrap_or_default();
    Ok(Menu {
        mod_name: obj.get("modName").and_then(string).unwrap_or_default(),
        display_name: obj.get("displayName").and_then(string).unwrap_or_default(),
        description: obj.get("description").and_then(string).unwrap_or_default(),
        save_file: obj.get("saveFile").and_then(string).unwrap_or_default(),
        min_mcm_version: obj.get("minMCMVersion").and_then(num),
        min_mod_version: obj.get("minModVersion").and_then(num),
        requirements,
        submenus,
        unknown: obj.keys().filter(|k| !MENU_KEYS.iter().any(|c| c.eq_ignore_ascii_case(k))).cloned().collect(),
    })
}

/// The `$keys` of a translations file: every `key=value` line of every section, the key
/// with its `$` when it has one.
pub fn parse_translations(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::default();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with(';') || l.starts_with('[') {
            continue;
        }
        if let Some((k, v)) = l.split_once('=') {
            let k = k.trim();
            if k.starts_with('$') {
                out.insert(k.to_string(), v.trim().to_string());
            }
        }
    }
    out
}

/// Read a translations file beside a menu: `MCM/Translations/<menu file name>.ini`, in any
/// case; empty when the mod ships none.
pub fn translations_for(folder: &Path, menu_file: &str) -> HashMap<String, String> {
    let Some(dir) = fnv_install::child_ci(&fnv_install::child_ci(folder, DIR).unwrap_or_else(|| folder.join(DIR)), "Translations") else {
        return HashMap::default();
    };
    let Ok(rd) = std::fs::read_dir(&dir) else { return HashMap::default() };
    let stem = Path::new(menu_file).file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ini")))
        .find(|p| p.file_stem().is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(&stem)))
        .and_then(|p| std::fs::read(p).ok())
        .map(|b| parse_translations(&String::from_utf8_lossy(&b)))
        .unwrap_or_default()
}

/// Every menu of the mods layer, the highest precedence first. Each mod folder's
/// `MCM/*.json` is one menu; a file of the same name in a lower folder is the hidden copy
/// of the higher one (the same rule the VFS indexes loose files by), so it is not listed
/// twice. A file that does not parse is reported in [`Discovery::errors`] and left out.
#[derive(Debug, Default, Clone)]
pub struct Discovery {
    pub menus: Vec<Source>,
    /// `(relative file, why)` for each menu that did not parse.
    pub errors: Vec<(String, String)>,
}

pub fn discover(mods: &Path) -> Discovery {
    let mut out = Discovery::default();
    let mut claimed: Vec<String> = Vec::new();
    for folder in fnv_install::mods::mod_folders(mods).into_iter().rev() {
        let name = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let Some(dir) = fnv_install::child_ci(&folder, DIR) else { continue };
        if !dir.is_dir() {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut files: Vec<(String, PathBuf)> = rd
            .flatten()
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
            .filter(|(n, p)| p.is_file() && n.to_ascii_lowercase().ends_with(".json"))
            .collect();
        files.sort_by_key(|(n, _)| n.to_ascii_lowercase());
        for (file, path) in files {
            let rel = format!("{DIR}\\{file}");
            if claimed.iter().any(|c| c.eq_ignore_ascii_case(&rel)) {
                continue;
            }
            claimed.push(rel.clone());
            let text = match std::fs::read(&path) {
                Ok(b) => String::from_utf8_lossy(&b).into_owned(),
                Err(e) => {
                    out.errors.push((format!("{name}\\{rel}"), e.to_string()));
                    continue;
                }
            };
            match parse(&text) {
                Ok(menu) => out.menus.push(Source { folder: folder.clone(), rel, menu, translations: translations_for(&folder, &file) }),
                Err(e) => out.errors.push((format!("{name}\\{rel}"), e)),
            }
        }
    }
    out
}

/// A control's value as the mod's own script sees it: the INI's number, else the JSON's
/// default. A value that is not a number reads as the default (the page says so in the
/// log rather than showing 0).
pub fn value_in(g: &Game, source: &Source, control: &Control) -> f64 {
    let (Some(rel), Some((section, key))) = (source.config_rel(), control.config()) else { return control.default() };
    let Some(text) = g.mod_files.read_text(rel.as_str()) else { return control.default() };
    match crate::mod_files::ini_get(&text, &section.to_ascii_lowercase(), key) {
        None => control.default(),
        Some(v) => v.trim().parse().unwrap_or_else(|_| {
            tracing::warn!(file = %rel, section = %section, key = %key, value = %v, "mcm: a value that is not a number reads as the menu's default");
            control.default()
        }),
    }
}

/// Write `value` into the mod's own copy of its Config file: returns the Data-relative
/// path written and the file it went to.
///
/// The text is kept for this session's readers too ([`crate::mod_files::ModFiles`]), so
/// `GetModINI` of the same frame sees it, and the ShowOff INI cache of the file is
/// dropped. An existing file keeps its comments, its key's place and its other keys; a
/// missing one is made.
pub fn set_value(g: &mut Game, source: &Source, section: &str, key: &str, value: &str) -> Result<(String, PathBuf), String> {
    let rel = source.config_rel().ok_or_else(|| format!("{} has no usable saveFile", source.folder_name()))?;
    let old = g.mod_files.read_text(&rel).unwrap_or_default();
    let new = crate::mod_files::ini_set(&old, &section.to_ascii_lowercase(), &key.to_ascii_lowercase(), value, section, key);
    let target = fnv_install::mods::resolve_in(&source.folder, &rel).unwrap_or_else(|| {
        let mut p = source.folder.clone();
        for part in rel.split('\\') {
            p.push(part);
        }
        p
    });
    fnv_install::guard_write(&target, None).map_err(|e| e.to_string())?;
    if let Some(d) = target.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    std::fs::write(&target, new.as_bytes()).map_err(|e| format!("{}: {e}", target.display()))?;
    g.mod_files.keep_written(&rel, new);
    Ok((rel, target))
}

/// Run the function a control names (`call`/`Call`) in the same frame: `None` when the
/// menu names none, `Err` when the function is not a script of the load order or did not
/// run.
pub fn run_call(g: &mut Game, call: &Call) -> Option<Result<(), String>> {
    if !call.kind.eq_ignore_ascii_case("udf") {
        return Some(Err(format!("a {:?} handler is not run: only \"udf\" is", call.kind)));
    }
    let Some(id) = g.data.by_edid(&call.value) else {
        return Some(Err(format!("no form has the editor ID {}", call.value)));
    };
    // MCM Extender calls its functions with the player as the calling reference.
    let ctx = crate::game::RunCtx { self_ref: Some(fnv_sim::FormId::PLAYER_REF), ..Default::default() };
    Some(g.run_function(&fnv_script::vm::XVal::Ref(id.0), None, Vec::new(), None, ctx).map(|_| ()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::*;
    use crate::game::GameConfig;
    use crate::GameData;
    use fnv_esm::{LoadOrder, Plugin, Schema};
    use fnv_script::FunctionTable;
    use fnv_sim::{FormStore, RefTable, SimConfig, World};

    /// The fixture's menu: one of every control type the installed menus show, the words
    /// and ids the same shape (`crates/fnv-game/tests/data/mcm/Fixture.json`). It is
    /// written from the schema the overlay's menus use, with our own labels: the mods'
    /// files are not copied into the tree (CLAUDE.md: third-party text is not shipped).
    const FIXTURE: &str = include_str!("../tests/data/mcm/Fixture.json");
    const TRANSLATIONS: &str = include_str!("../tests/data/mcm/Translations/Fixture.ini");

    fn menu() -> Menu {
        parse(FIXTURE).expect("the fixture parses")
    }

    #[test]
    fn every_control_type_the_installed_menus_use_has_a_kind() {
        let m = menu();
        assert_eq!(m.mod_name, "FixtureMod");
        assert_eq!(m.save_file, "Fixture.ini");
        assert_eq!((m.min_mcm_version, m.min_mod_version), (Some(1.3), Some(1.0)));
        assert_eq!(m.requirements, [("plugin".to_string(), "Fixture.esp".to_string()), ("file".to_string(), "Config/Fixture/Fixture.ini".to_string())]);
        assert!(m.unknown.is_empty(), "{:?}", m.unknown);
        let pages = m.submenus.iter().map(|s| s.id).collect::<Vec<_>>();
        assert_eq!(pages, [0, 1]);
        let options = &m.submenus[1].controls;
        let kinds: Vec<Kind> = options.iter().map(|c| c.kind).collect();
        assert_eq!(
            kinds,
            [Kind::Section, Kind::List, Kind::Slider { decimals: 0 }, Kind::Slider { decimals: 1 }, Kind::Checkbox, Kind::Toggle, Kind::LabelledToggle, Kind::Key, Kind::Unknown(8.0)],
        );
        for c in options.iter().take(7).skip(1) {
            assert!(c.unknown.is_empty(), "{}: {:?}", c.title, c.unknown);
            assert!(c.editable(), "{} has a configINI", c.title);
        }
        // The values the JSON gives each kind.
        let list = &options[1];
        assert_eq!(list.strings, ["$Off", "$Middle", "$Most"]);
        assert_eq!((list.config(), list.default()), (Some(("General", "iList")), 1.0));
        let slider = &options[2];
        assert_eq!(slider.scale, Some(Scale { decimals: 0, increment: 1.0, min: 1.0, max: 100.0, suffix: String::new() }));
        assert_eq!(slider.show(30.0), "30%");
        let decimal = &options[3];
        assert_eq!(decimal.scale.as_ref().map(|s| (s.decimals, s.increment, s.min, s.max)), Some((1, 0.1, -3.0, 3.0)));
        assert_eq!(decimal.show(0.4), "0.4x");
        assert_eq!(decimal.var.as_ref().and_then(|v| v.form.as_deref()), Some("Goo1"));
        assert_eq!(decimal.var.as_ref().and_then(|v| v.aux_var.as_deref()), Some("*Fixture_INI"));
        assert_eq!(decimal.var.as_ref().and_then(|v| v.key), Some(1.0));
        let toggle = &options[6];
        assert_eq!((toggle.text_on.as_deref(), toggle.text_off.as_deref()), (Some("$Near"), Some("$Far")));
        assert_eq!(options[7].kind, Kind::Key);
        assert_eq!(options[7].kind.unsupported(), Some("a key binding: not drawn yet"));
        // The splash: a section with a picture, the fields either nested or loose.
        assert_eq!(m.submenus[0].controls[0].image.as_deref(), Some("Interface/Fixture/Splash.dds"));
        assert_eq!(options[0].kind, Kind::Section);
        // A call, spelled either way; the fixture uses both (`call` on the slider, `Call`
        // on the checkbox) and the toggle names none.
        assert_eq!(options[2].call, Some(Call { kind: "udf".into(), value: "FixtureUpdate".into() }));
        assert_eq!(options[4].call, Some(Call { kind: "udf".into(), value: "FixtureUpdate".into() }));
        assert_eq!(options[5].call, None);
    }

    #[test]
    fn every_key_the_format_has_is_placed_or_named() {
        // A key the module does not read stays in `unknown`, per level, rather than being
        // dropped silently.
        let text = r#"{"modName":"M","saveFile":"M.ini","whatever":1,
            "submenus":{"0":{"listTitle":"a","mystery":[1],"options":{"1":{"title":"t","type":0,"strange":true}}}}}"#;
        let m = parse(text).expect("parses");
        assert_eq!(m.unknown, ["whatever"]);
        assert_eq!(m.submenus[0].unknown, ["mystery"]);
        assert_eq!(m.submenus[0].controls[0].unknown, ["strange"]);
        assert!(parse("{}").is_err(), "no submenus");
        assert!(parse("not json").is_err());
    }

    #[test]
    fn the_translations_give_a_key_its_words() {
        let t = parse_translations(TRANSLATIONS);
        assert_eq!(t.get("$modDisplayName").map(String::as_str), Some("Fixture Mod"));
        assert_eq!(t.get("$Near").map(String::as_str), Some("Near"));
        let src = Source {
            folder: PathBuf::from("/x/FixtureMod"),
            rel: "MCM\\Fixture.json".into(),
            menu: menu(),
            translations: t,
        };
        assert_eq!(src.name(), "Fixture Mod");
        assert_eq!(src.text("$Near"), "Near");
        // A key with no translation shows the key, not a `$`.
        assert_eq!(src.text("$NoSuchKey"), "NoSuchKey");
        assert_eq!(src.text("plain"), "plain");
        assert_eq!(src.config_rel().as_deref(), Some("Config\\Fixture.ini"));
        // A saveFile with a folder is kept under Config the same way (`Stentorious/…` in
        // the overlay); `GetModINI`'s `File/Section/Key` cannot spell it, `GetINIFloat` can.
        let nested = parse(r#"{"modName":"N","saveFile":"Lime/Encounters.ini","submenus":{"0":{"options":{}}}}"#).unwrap();
        let nested = Source { folder: PathBuf::from("/x/N"), rel: "MCM\\N.json".into(), menu: nested, translations: HashMap::default() };
        assert_eq!(nested.config_rel().as_deref(), Some("Config\\Lime\\Encounters.ini"));
    }

    #[test]
    fn a_lists_choices_wrap_and_a_sliders_steps_clamp() {
        let m = menu();
        let o = &m.submenus[1].controls;
        let (list, slider) = (&o[1], &o[2]);
        assert_eq!(list.stepped(2.0, 1), 0.0, "a list wraps");
        assert_eq!(list.stepped(0.0, -1), 2.0);
        assert_eq!(slider.stepped(100.0, 1), 100.0, "a slider clamps");
        assert_eq!(slider.stepped(1.0, -1), 1.0);
        assert_eq!(slider.stepped(30.0, 1), 31.0);
        let decimal = &o[3];
        assert_eq!(decimal.stepped(0.4, 1), 0.5, "a decimal slider steps by 0.1");
        assert_eq!(decimal.stepped(-3.0, -1), -3.0);
        assert_eq!(decimal.stored(0.5), "0.5");
        assert_eq!(o[4].stored(o[4].stepped(0.0, 1)), "1", "a switch is 0 or 1");
        // A negative slider keeps its sign and its places.
        assert_eq!(decimal.show(-0.5), "-0.5x");
    }

    // ---- where the menus are found, and precedence ----

    fn home(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("nvf-mcm-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("mods")).unwrap();
        root.join("mods")
    }

    fn write(mods: &Path, rel: &str, text: &str) {
        let p = mods.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn two_mods_menus_are_both_found_and_precedence_wins_a_shared_name() {
        let mods = home("registry");
        std::fs::create_dir_all(mods.join("Alpha/MCM")).unwrap();
        std::fs::create_dir_all(mods.join("Bravo/MCM")).unwrap();
        std::fs::create_dir_all(mods.join("Charlie/MCM")).unwrap();
        write(&mods, "Alpha/MCM/Alpha.json", r#"{"modName":"Alpha","displayName":"Alpha","saveFile":"Alpha.ini","submenus":{"0":{"options":{}}}}"#);
        write(&mods, "Bravo/MCM/Bravo.json", r#"{"modName":"Bravo","displayName":"Bravo","saveFile":"Bravo.ini","submenus":{"0":{"options":{}}}}"#);
        write(&mods, "Bravo/MCM/Shared.json", r#"{"modName":"Bravo's copy","saveFile":"S.ini","submenus":{"0":{"options":{}}}}"#);
        write(&mods, "Charlie/MCM/Shared.json", r#"{"modName":"Charlie's copy","saveFile":"S.ini","submenus":{"0":{"options":{}}}}"#);
        // `mods.txt` top line wins: Charlie over Bravo.
        write(&mods, "mods.txt", "Charlie\nBravo\nAlpha\n");
        let d = discover(&mods);
        let names: Vec<String> = d.menus.iter().map(|s| s.menu.mod_name.clone()).collect();
        assert_eq!(names, ["Charlie's copy", "Bravo", "Alpha"], "highest precedence first, each mod's own menu kept");
        assert_eq!(d.menus[0].folder_name(), "Charlie");
        assert!(d.errors.is_empty(), "{:?}", d.errors);
        // A broken menu is reported, not guessed at, and the others still load.
        write(&mods, "Alpha/MCM/Broken.json", "{ not json");
        let d = discover(&mods);
        assert_eq!(d.menus.len(), 3);
        assert_eq!(d.errors.len(), 1);
        assert!(d.errors[0].0.ends_with("Broken.json"));
        // Names are matched in any case, as Windows does.
        write(&mods, "Alpha/mcm/alpha.JSON", r#"{"modName":"Other","saveFile":"A.ini","submenus":{"0":{"options":{}}}}"#);
        let _ = std::fs::remove_dir_all(mods.parent().unwrap());
    }

    #[test]
    fn translations_are_found_beside_the_menu_in_any_case() {
        let mods = home("translations");
        write(&mods, "Delta/mcm/Fixture.json", FIXTURE);
        write(&mods, "Delta/mcm/translations/fixture.INI", TRANSLATIONS);
        let d = discover(&mods);
        assert_eq!(d.menus.len(), 1);
        assert_eq!(d.menus[0].name(), "Fixture Mod");
        let _ = std::fs::remove_dir_all(mods.parent().unwrap());
    }

    // ---- the file a change goes to, and the script's view of it ----

    fn game(name: &str, folder_ini: &str) -> (GameData, FormStore, RefTable, PathBuf) {
        let bytes = [tes4(), top(b"STAT", &record(b"STAT", 0, 0xD10, &[sub(b"EDID", z("Neighbour"))]))].concat();
        let plugins = [Plugin::parse(&bytes, "Mcm.esp".into()).unwrap()];
        let lo = LoadOrder::new(&plugins).unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
        let schema = Schema::load_dir(root.join("fnv")).unwrap();
        let path = root.join("script/functions.json");
        let catalogue = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let forms = FormStore::build(&lo, &schema);
        let refs = RefTable::build(&lo, &schema);
        let data = GameData::build(&lo, &schema, FunctionTable::load(&path).unwrap(), &catalogue, &refs);
        let mods = home(name);
        write(&mods, "FixtureMod/MCM/Fixture.json", FIXTURE);
        write(&mods, "FixtureMod/MCM/Translations/Fixture.ini", TRANSLATIONS);
        std::fs::create_dir_all(mods.join("FixtureMod/Config")).unwrap();
        std::fs::write(mods.join("FixtureMod/Config/Fixture.ini"), folder_ini).unwrap();
        (data, forms, refs, mods)
    }

    // One test, because the file roots ([`crate::mod_files::set_roots`]) are global to the
    // process: a second test setting them would race this one.
    #[test]
    fn a_change_round_trips_through_the_mods_own_ini_and_get_mod_ini_sees_it() {
        let ini = "; kept\r\n[General]\r\niList = 1\r\nfDec = 0.4\r\n\r\n[Tuning]\r\nbMode = 1\r\n";
        let (data, forms, refs, mods) = game("roundtrip", ini);
        let mut g = Game::new(&data, World::new(&forms, &refs, SimConfig::default()), GameConfig { strict: true, ..Default::default() });
        // The readers reach the mod folder and Data, as the engine sets them.
        crate::mod_files::set_roots(crate::mod_files::Roots { read: vec![mods.join("FixtureMod"), mods.join("FixtureMod")], ..Default::default() });
        let mut d = discover(&mods);
        let mut src = d.menus.remove(0);
        src.folder = mods.join("FixtureMod");
        let control = src.menu.submenus.iter().find(|s| s.id == 1).and_then(|s| s.controls.iter().find(|c| c.kind == Kind::List)).unwrap();
        assert_eq!(value_in(&g, &src, control), 1.0, "read from the file");
        let (rel, path) = set_value(&mut g, &src, "General", "iList", "2").unwrap();
        assert_eq!(rel, "Config\\Fixture.ini");
        assert!(path.ends_with("FixtureMod/Config/Fixture.ini"));
        // The disk copy: the value changed, the comment and the other keys stayed.
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(on_disk.contains("; kept"));
        assert!(on_disk.contains("iList=2"), "{on_disk}");
        assert!(on_disk.contains("fDec = 0.4"), "a line it did not touch keeps its spelling: {on_disk}");
        assert!(on_disk.contains("bMode = 1"));
        // The next start reads the new value back.
        assert_eq!(value_in(&g, &src, control), 2.0);
        // `GetModINI "File/Section/Key"`, the function the mod's script calls, and LU's
        // `GetINIFloat`/`GetINIString` form.
        let got = crate::mod_files::call(&mut g, "getmodinisetting", &[fnv_script::vm::XVal::Str("Fixture.ini/General/iList".into())]);
        assert_eq!(got, Some(Ok(fnv_script::vm::XVal::Num(2.0))));
        let text = crate::mod_files::call(&mut g, "getinistring", &[fnv_script::vm::XVal::Str("General:iList".into()), fnv_script::vm::XVal::Str("Fixture.ini".into())]);
        assert_eq!(text, Some(Ok(fnv_script::vm::XVal::Str("2".into()))));
        // A slider's stored places: 2 decimal places would lose 0.4; the JSON's 1 keeps it.
        let dec = src.menu.submenus.iter().find(|s| s.id == 1).and_then(|s| s.controls.iter().find(|c| c.kind == Kind::Slider { decimals: 1 })).unwrap();
        let v = dec.stepped(value_in(&g, &src, dec), 1);
        set_value(&mut g, &src, "Tuning", "fDec", &dec.stored(v)).unwrap();
        assert_eq!(value_in(&g, &src, dec), 0.5);
        let _ = std::fs::remove_dir_all(mods.parent().unwrap());
    }

    #[test]
    fn a_named_handler_that_does_not_exist_is_said_so() {
        let (data, forms, refs, mods) = game("handler", "[General]\r\n");
        let mut g = Game::new(&data, World::new(&forms, &refs, SimConfig::default()), GameConfig { strict: true, ..Default::default() });
        let call = Call { kind: "udf".into(), value: "NoSuchFunction".into() };
        let said = run_call(&mut g, &call).expect("a handler was named");
        assert!(said.unwrap_err().contains("NoSuchFunction"));
        let not_udf = run_call(&mut g, &Call { kind: "script".into(), value: "x".into() }).unwrap();
        assert!(not_udf.unwrap_err().contains("only \"udf\" is"));
        let _ = std::fs::remove_dir_all(mods.parent().unwrap());
    }
}
