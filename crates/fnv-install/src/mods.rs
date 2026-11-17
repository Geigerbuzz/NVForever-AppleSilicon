//! The mods layer's shape and precedence: one folder per mod under the home's `mods/`
//! ([`EngineHome::mods`]), each of those an overlay root, and `mods/mods.txt` stating
//! which folder wins (docs/design/install-layout.md, docs/design/editor.md §2.2 item 9).
//!
//! A mod-relative path is the path **inside** a mod folder: `Config\B42Inertia.ini` is
//! `mods/B42Inertia/Config/B42Inertia.ini`, and a plugin is `mods/JAM/JustAssortedMods.esp`
//! — never `mods/Config\…` or `mods\JustAssortedMods.esp`. A lookup therefore goes through
//! the mod folders, highest precedence first, and that one order is what the VFS indexes
//! loose files in, what the plugin list resolves names through, and what the mod-files
//! layer reads configs, Script Runner files and AnimGroupOverride JSONs through.
//!
//! Precedence (the order the VFS has indexed loose files in since `mods.txt` landed):
//!
//! - the mods folder itself is the **lowest** root, where editor mode writes its own
//!   plugin (`nvf_play::editor`) and where an older flat layout kept files;
//! - then one folder per mod, in this order:
//!   - a folder `mods.txt` does not name, in name order, at the bottom;
//!   - a folder it names, in the file's order read bottom line first, so the **top line
//!     wins**. A name that is not installed is warned about once and skipped.
//!
//! So a freshly installed mod lands at the bottom and cannot silently outrank a curated
//! order — the failure that made MUVAT 2K override 40 clothing textures and an old
//! Pip-Boy icon pack hide all five v5 packs.

use std::path::{Path, PathBuf};

/// The installed mod folders under `mods`, **lowest precedence first**: the last one wins,
/// which is the order the VFS indexes loose files in (`fnv-vfs`, `Vfs::open_for_plugins_in`).
/// An absent folder and a folder with no `mods.txt` are ordinary: the name order is the
/// fallback.
pub fn mod_folders(mods: &Path) -> Vec<PathBuf> {
    let mut payloads: Vec<PathBuf> = std::fs::read_dir(mods).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default();
    payloads.sort();
    let Ok(text) = std::fs::read_to_string(mods.join("mods.txt")) else { return payloads };
    let mut named: Vec<String> = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let l = l.trim_start_matches(['*', '!', '+', ' ']);
        if !l.is_empty() {
            named.push(l.to_string());
        }
    }
    let find = |name: &str| {
        payloads.iter().find(|p| p.file_name().and_then(|s| s.to_str()).is_some_and(|n| n.eq_ignore_ascii_case(name))).cloned()
    };
    let mut order: Vec<PathBuf> = Vec::new();
    for p in &payloads {
        let n = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if !named.iter().any(|x| x.eq_ignore_ascii_case(n)) {
            order.push(p.clone());
        }
    }
    for name in named.iter().rev() {
        match find(name) {
            Some(p) => {
                if !order.iter().any(|o| o == &p) {
                    order.push(p);
                }
            }
            None => tracing::warn!(mods_txt = %name, "mods.txt names a folder that is not installed"),
        }
    }
    order
}

/// Every lookup root of the mods layer, **lowest precedence first**: the mods folder
/// itself, then [`mod_folders`]. Plugins, archives and mod files are looked for here; a
/// loose *asset* is not (a file at the mods folder's own top level is not an asset, so the
/// VFS indexes [`mod_folders`] only).
pub fn mod_roots(mods: &Path) -> Vec<PathBuf> {
    let mut roots = vec![mods.to_path_buf()];
    roots.extend(mod_folders(mods));
    roots
}

/// [`mod_roots`] with the highest precedence first: the order
/// [`crate::EngineHome`]'s files are read in (`fnv_game::mod_files::Roots::read`, the first
/// root winning, then the game's `Data` at the end).
pub fn read_roots(mods: &Path) -> Vec<PathBuf> {
    let mut roots = mod_roots(mods);
    roots.reverse();
    roots
}

/// `rel` in `root`, each component matched as on Windows (a mod spells `NVSE\Plugins` and
/// `nvse\plugins` both, #995): the exact name when it exists, else the entry equal to it
/// ignoring ASCII case. `None` for a path that could leave the root or has no name.
pub fn resolve_in(root: &Path, rel: &str) -> Option<PathBuf> {
    if rel.is_empty() || rel.starts_with(['/', '\\']) || rel.contains(':') || rel.contains('\0') {
        return None;
    }
    let mut at = root.to_path_buf();
    let mut any = false;
    for part in rel.split(['/', '\\']) {
        match part {
            "" | "." => continue,
            ".." => return None,
            part => {
                at = crate::child_ci(&at, part)?;
                any = true;
            }
        }
    }
    any.then_some(at)
}

/// The copy of the mod-relative path `rel` that wins: the highest-precedence root of
/// [`mod_roots`] that has it, the file matched in any case. `None` when no root has it, so
/// the caller falls back to the game's `Data`.
pub fn resolve(mods: &Path, rel: &str) -> Option<PathBuf> {
    mod_roots(mods).into_iter().rev().find_map(|root| resolve_in(&root, rel))
}

/// Whether any root of the mods layer has the mod-relative path `rel`
/// ([`resolve`] found one).
pub fn exists(mods: &Path, rel: &str) -> bool {
    resolve(mods, rel).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EngineHome;

    fn home(name: &str) -> EngineHome {
        let t = std::env::temp_dir().join(format!("nvf-mods-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        EngineHome::new(t)
    }

    fn folder(h: &EngineHome, name: &str) {
        std::fs::create_dir_all(h.mods().join(name)).unwrap();
    }

    fn file(h: &EngineHome, rel: &str, text: &str) {
        let p = h.mods().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    /// `mods.txt` puts its first line on top and leaves an unnamed folder at the bottom,
    /// which is the order the VFS indexes in (so the last folder wins).
    #[test]
    fn mod_folders_follow_mods_txt_top_wins() {
        let h = home("folders");
        for name in ["Alpha", "Bravo", "Charlie"] {
            folder(&h, name);
        }
        std::fs::write(h.mods().join("mods.txt"), "# curated\nCharlie\nAlpha\n").unwrap();
        let names: Vec<String> = mod_folders(&h.mods()).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        // Unnamed first (Bravo), then the named ones bottom to top.
        assert_eq!(names, ["Bravo", "Alpha", "Charlie"]);
        // Read highest precedence first, so the top line's folder first and the mods
        // folder itself last (with the game's Data after it, the caller's job).
        let read = read_roots(&h.mods());
        assert_eq!(read.first().unwrap(), &h.mods().join("Charlie"));
        assert_eq!(read.last().unwrap(), &h.mods());
        let _ = std::fs::remove_dir_all(&h.root);
    }

    /// A file in two mods is the higher-precedence mod's; a named mod beats an unnamed one;
    /// the mods folder itself is the lowest root of all.
    #[test]
    fn a_file_in_two_mods_is_the_higher_ones() {
        let h = home("precedence");
        for name in ["Alpha", "Bravo", "Zulu"] {
            folder(&h, name);
        }
        std::fs::write(h.mods().join("mods.txt"), "Alpha\nBravo\n").unwrap();
        file(&h, "Alpha/Config/Shared.ini", "alpha");
        file(&h, "Bravo/Config/Shared.ini", "bravo");
        file(&h, "Zulu/Config/Shared.ini", "zulu");
        file(&h, "Config/Shared.ini", "the mods folder itself");
        assert_eq!(std::fs::read_to_string(resolve(&h.mods(), "Config/Shared.ini").unwrap()).unwrap(), "alpha");
        // A file only the unnamed mod has resolves there, not to the mods folder's own copy.
        file(&h, "Zulu/Config/OnlyZulu.ini", "zulu");
        file(&h, "Config/OnlyZulu.ini", "flat");
        assert_eq!(std::fs::read_to_string(resolve(&h.mods(), "config\\onlyzulu.ini").unwrap()).unwrap(), "zulu");
        // Without mods.txt the name order decides: the last folder in name order wins.
        std::fs::remove_file(h.mods().join("mods.txt")).unwrap();
        assert_eq!(std::fs::read_to_string(resolve(&h.mods(), "Config/Shared.ini").unwrap()).unwrap(), "zulu");
        // Nothing in any root: no answer, so Data's copy is the caller's fallback.
        assert!(!exists(&h.mods(), "Config/Missing.ini"));
        // A path that could leave the layer is refused, not joined.
        assert!(resolve(&h.mods(), "../Alpha/Config/Shared.ini").is_none());
        let _ = std::fs::remove_dir_all(&h.root);
    }

    /// Each component is matched without regard to case, as Windows does.
    #[test]
    fn path_components_match_in_any_case() {
        let h = home("any-case");
        folder(&h, "B42Inertia");
        file(&h, "B42Inertia/Config/B42Inertia.ini", "[Main]\n");
        let p = resolve(&h.mods(), "config\\b42inertia.INI").expect("found in any case");
        assert!(p.ends_with("b42inertia.INI") || p.file_name().unwrap().to_string_lossy().eq_ignore_ascii_case("b42inertia.ini"));
        assert_eq!(std::fs::read_to_string(p).unwrap(), "[Main]\n");
        assert!(resolve(&h.mods(), "Config").is_some(), "a folder resolves too");
        assert!(resolve(&h.mods(), "Config/Missing.ini").is_none());
        let _ = std::fs::remove_dir_all(&h.root);
    }
}
