//! A mod's own files live under its own folder: `Config\B42Inertia.ini` is
//! `mods/B42Inertia/Config/B42Inertia.ini`, the same rule the VFS serves assets by. The
//! mod-files layer reads what the engine hands it in `Roots::read`, which the engine builds
//! with [`fnv_install::mods::read_roots`] (one root per mod, `mods.txt` precedence, then
//! the game's Data). Before that, `read` was the mods folder **itself**, so every
//! `Config\*.ini`, Script Runner `NVSE\Plugins\Scripts\*.txt` and AnimGroupOverride
//! `meshes\AnimGroupOverride\*.json` was looked for at the wrong level and found nothing.
//!
//! A dedicated target because `Roots` is process-wide.
use fnv_game::mod_files::{self, Roots};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp(name: &str) -> PathBuf {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let root = std::env::temp_dir().join(format!("nvf-mod-roots-{}-{name}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// The engine's roots: each mod folder from the top of `mods.txt` down, then Data.
fn engine_roots(mods: &Path, data: &Path) -> Roots {
    let mut read = fnv_install::mods::read_roots(mods);
    read.push(data.to_path_buf());
    Roots { read, ..Roots::default() }
}

#[test]
fn a_mods_config_is_read_and_listed() {
    let root = temp("config");
    let (mods, data) = (root.join("mods"), root.join("Data"));
    write(&mods, "B42Inertia/Config/B42Inertia.ini", "[Main]\nFriction=0.50\n");
    write(&mods, "WalkingInertia/Config/Walking_Inertia.ini", "[Walk]\nEnabled=1\n");
    // One name in two mods: the top line of mods.txt wins.
    write(&mods, "B42Inertia/Config/Same.ini", "higher");
    write(&mods, "WalkingInertia/Config/Same.ini", "lower");
    // Data's own copy is the fallback, and beats nothing in the mods layer.
    write(&data, "Config/GameOnly.ini", "game");
    write(&mods, "mods.txt", "# top wins\nB42Inertia\nWalkingInertia\n");
    // The Script Runner's own listing is the same root list.
    write(&mods, "B42Inertia/NVSE/Plugins/Scripts/ln_b42.txt", "Begin Function { }\nEnd\n");

    mod_files::set_roots(engine_roots(&mods, &data));

    // MCM's `GetModINI "B42Inertia.ini"` reaches the mod's own folder, whatever the case
    // the script writes (`config\b42inertia.ini`) and with the file's own name.
    assert_eq!(mod_files::read_data_file("Config\\B42Inertia.ini").as_deref(), Some(b"[Main]\nFriction=0.50\n".as_slice()));
    assert_eq!(mod_files::read_data_file("config\\b42inertia.ini").as_deref(), Some(b"[Main]\nFriction=0.50\n".as_slice()));
    assert_eq!(mod_files::read_data_file("Config\\Same.ini").as_deref(), Some(b"higher".as_slice()));
    assert_eq!(mod_files::read_data_file("Config\\GameOnly.ini").as_deref(), Some(b"game".as_slice()));
    // The listing (`data_files`: the Script Runner's folder walk) sees each mod's file
    // once, from its own folder, and Data's too.
    let names: Vec<String> = mod_files::data_files("Config").into_iter().map(|(n, _)| n).collect();
    for want in ["B42Inertia.ini", "Walking_Inertia.ini", "Same.ini", "GameOnly.ini"] {
        assert_eq!(names.iter().filter(|n| n.eq_ignore_ascii_case(want)).count(), 1, "{want} in {names:?}");
    }
    let (_, same) = mod_files::data_files("Config").into_iter().find(|(n, _)| n == "Same.ini").unwrap();
    assert!(same.starts_with(mods.join("B42Inertia")), "the higher mod's copy is listed: {}", same.display());
    let scripts: Vec<(String, PathBuf)> = mod_files::data_files("NVSE\\Plugins\\Scripts");
    assert_eq!(scripts.len(), 1, "{scripts:?}");
    assert_eq!(scripts[0].0, "ln_b42.txt");
    assert!(scripts[0].1.starts_with(mods.join("B42Inertia")));
    // A file no root has is nothing, not a panic and not Data's folder with a wrong path.
    assert_eq!(mod_files::read_data_file("Config\\Absent.ini"), None);

    mod_files::set_roots(Roots::default());
    let _ = std::fs::remove_dir_all(&root);
}
