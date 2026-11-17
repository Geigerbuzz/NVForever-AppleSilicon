//! The pause menu's native MCM page: the mods' `MCM/*.json` menus drawn as rows, with the
//! value of each control written back to the mod's own INI (`fnv_game::mcm`).
//!
//! MCM draws its pages from its own menu files and scripts, and MCM Extender registers a
//! JSON menu with it (`docs/spec-notes/mcm.md`). NVForever runs both when they are
//! installed. This page is for the other case: the owner's overlay ships six
//! `MCM/<name>.json` files and no MCM, so without this page every framework mod's settings
//! are invisible to the player.
//!
//! - **The switch**: [`McmSwitch`], on for play, off with `NVF_MCM_MENUS=0` (a parity or
//!   picture run), and a settings row ("Gameplay > MCM menus") that keeps the choice in
//!   `NVForever.ini`.
//! - **The entry**: with the switch on and one menu found, the pause column has a "Mod
//!   settings" entry before Quit (`crate::pause::McmEntry`). The page is drawn over the
//!   pause menu, in the settings page's look and units: the mods at the left, their pages
//!   in the middle, the page's controls at the right, the chosen row boxed.
//! - **The controls**: a section title is a line nobody chooses; a list shows its chosen
//!   word and Left/Right (or a click) step through it, wrapping; a slider steps by its
//!   `valueIncrement` inside `valueMin`..`valueMax` with the places `valueDecimal` asks
//!   for; a checkbox and a toggle flip between 0 and 1; a labelled toggle shows its
//!   `textOff`/`textOn`. `type` 3 (a key binding) and a type this build does not know are
//!   drawn as a row that says so and refuse a change, with the log line saying why — a
//!   TODO in the open, not a silent no-op.
//! - **The keys**: Up/Down (W/S) choose, Left/Right (A/D) change, Enter or Space take,
//!   Tab/Shift+Tab the next or previous page, Esc/Backspace/the Back row go back to the
//!   column. The pointer chooses the row it moves onto and a click takes it, as the
//!   settings page's rows do.
//! - **What a change writes**: the mod's own `Config\<saveFile>` inside its folder in the
//!   mods layer, through `fnv_game::mcm::set_value`; the text is kept for this session's
//!   readers, so `GetModINI`/`GetINI*` see it in the same frame, and a `call`/`Call`
//!   `{"type":"udf"}` handler runs in the same frame (`mcm::run_call`). Log lines:
//!   `mcm: <mod>: <Section:Key> = <value> (Config\…ini)`, `mcm: <handler> ran` or why not.
//! - **The log at start**: `mcm: N menu(s) from the mods layer`, then per menu its name,
//!   pages and controls, and any key or control type this build does not place.
//! - **Not drawn yet** (each logged once per menu): a splash's picture (`image.filename`),
//!   a two-column page's exact MCM order (the page is one column, the controls in number
//!   order; `columns` is read), a page's own `state` (shown when the page opens instead),
//!   `mouse` notes (the chosen row's note is drawn), and the `form`/`auxVar`/`numVar`/
//!   `uiTrait` hand-off (the INI is the contract; see `fnv_game::mcm`).
use crate::interface::{at, in_menu_frame, texture};
use crate::pause::{Page, Paused};
use crate::script::ScriptState;
use crate::ui_pointer::{Button, PointerState};
use crate::start_menu::{Faces, ITEM, LINE, RIGHT, Shade, COLUMN as PAUSE_COLUMN};
use bevy::image::CompressedImageFormatSupport;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use fnv_game::mcm::{self, Control, Kind, Source};
use nvf_assets::GameData;

/// The column's entry [design: NVForever writes its own words for a screen the data files
/// do not name; MCM's own title is its file's].
pub const LABEL: &str = "Mod settings";

/// The page's stacking level: over the pause menu's own drawing and above the Mods page's
/// (`crate::tiles::mods::MODS_Z`).
pub const Z: i32 = 116;

/// The switch: set from `NVF_MCM_MENUS` at start (`0` off) and from the settings row.
pub const ENV: &str = "NVF_MCM_MENUS";

/// The rows shown at once.
const ROWS: usize = 12;
/// A row's height, the pause menu's own ([`ITEM`]).
const ROW: f32 = ITEM;
/// Where the title, the tabs and the rows stand, and the three columns' widths.
const TITLE_TOP: f32 = 60.0;
const TABS_TOP: f32 = 150.0;
const ROWS_TOP: f32 = 200.0;
const MODS_W: f32 = 260.0;
const PAGES_W: f32 = 190.0;
const GAP: f32 = 18.0;
/// The value's room at a row's right end.
const VALUE_W: f32 = 220.0;

/// Whether the page is on: the switch's state.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct McmSwitch(pub bool);

impl Default for McmSwitch {
    fn default() -> Self {
        McmSwitch(std::env::var(ENV).map_or(true, |v| !matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "off" | "false" | "no")))
    }
}

/// The menus read from the mods layer, once.
#[derive(Resource, Default)]
pub struct McmMenus {
    pub loaded: bool,
    pub menus: Vec<Source>,
    /// Why a menu did not load, for the log.
    pub errors: Vec<(String, String)>,
}

/// Whether the pause column has the page ([`LABEL`]).
#[derive(Resource, Default, PartialEq, Eq, Debug)]
pub struct McmEntry(pub bool);

/// Where the page stands: the chosen mod, page and control, and the last line said.
#[derive(Resource, Default, Debug, Clone)]
pub struct McmPage {
    pub m: usize,
    pub p: usize,
    pub row: usize,
    /// The first row shown.
    pub first: usize,
    /// What the last change said (the line under the rows).
    pub status: String,
    /// Frames the page has been up (0 before its first).
    pub frames: u64,
}

/// One thing the keys or the pointer asked for, worked out by `crate::pause` and the page's
/// own `feed`, applied to the world in [`key`] (so the game and the files are one borrow).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Up,
    Down,
    Left,
    Right,
    Take,
    NextPage(i32),
    Back,
    /// A click on a mod's row.
    Mod(usize),
    /// A click on a page's row.
    Page(usize),
    /// A click on a control's row: choose it, and take it once (as Enter does).
    Row(usize),
    /// A click on the Back row.
    BackRow,
}

/// A row the pointer can hit: what it stands for.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub struct McmHit(pub Hit);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    Mod(usize),
    Page(usize),
    Row(usize),
    Back,
}

/// The page's root, spawned each frame it is up.
#[derive(Component)]
struct McmRoot;

/// The fonts, read once.
#[derive(Default)]
struct Look {
    faces: Option<Faces>,
}

type Looked<'w, 's> = (Local<'s, Look>, Option<Res<'w, GameData>>, ResMut<'w, Assets<Image>>, Option<Res<'w, CompressedImageFormatSupport>>);

/// Read the menus from the owner's mods layer, once the home is known.
pub fn load(mut menus: ResMut<McmMenus>, mut entry: ResMut<McmEntry>, switch: Res<McmSwitch>) {
    if menus.loaded {
        return;
    }
    let Some(root) = fnv_install::EngineHome::default_root() else {
        menus.loaded = true;
        return;
    };
    let dir = fnv_install::EngineHome::new(root).mods();
    let found = mcm::discover(&dir);
    for (file, why) in &found.errors {
        warn!("mcm: {file}: {why}");
    }
    for s in &found.menus {
        let pages: Vec<String> = s.pages().iter().map(|p| format!("{} {:?}", p.id, s.text(&p.list_title))).collect();
        let controls: usize = s.menu.submenus.iter().map(|p| p.controls.len()).sum();
        info!("mcm: {} ({}) {} page(s), {} control(s), saveFile {:?}: {}", s.name(), s.folder_name(), pages.len(), controls, s.menu.save_file, pages.join(", "));
        if !s.menu.unknown.is_empty() {
            warn!("mcm: {}: menu key(s) this build does not place: {}", s.name(), s.menu.unknown.join(", "));
        }
        for p in &s.menu.submenus {
            if !p.unknown.is_empty() {
                warn!("mcm: {} page {}: key(s) this build does not place: {}", s.name(), p.id, p.unknown.join(", "));
            }
            for c in p.controls.iter().filter(|c| !c.unknown.is_empty()) {
                warn!("mcm: {} page {} control {}: key(s) this build does not place: {}", s.name(), p.id, c.id, c.unknown.join(", "));
            }
            for c in p.controls.iter().filter(|c| c.kind.unsupported().is_some() || !c.enable) {
                let why = c.kind.unsupported().unwrap_or("off (`enable` 0)");
                info!("mcm: {} page {} control {} {:?}: {why}", s.name(), p.id, c.id, s.text(&c.title));
            }
        }
    }
    info!("mcm: {} menu(s) from the mods layer ({}), {} that did not parse", found.menus.len(), dir.display(), found.errors.len());
    menus.menus = found.menus;
    menus.errors = found.errors;
    menus.loaded = true;
    let on = switch.0 && !menus.menus.is_empty();
    if entry.0 != on {
        info!("mcm: {}", if on { "the pause menu has a Mod settings entry" } else { "no Mod settings entry: no menu to show" });
    }
    entry.0 = on;
}

/// The pause column's entry, kept current with the switch and what loaded.
pub fn refresh_entry(menus: Res<McmMenus>, switch: Res<McmSwitch>, mut entry: ResMut<McmEntry>) {
    if !menus.loaded {
        return;
    }
    let on = switch.0 && !menus.menus.is_empty();
    if entry.0 != on && menus.loaded {
        info!("mcm: {}", if on { "the pause menu has a Mod settings entry" } else { "no Mod settings entry" });
    }
    entry.0 = on;
}

/// Reset the page to a fresh open (the pause menu calls this when its entry is taken).
pub fn open(menus: &McmMenus) -> McmPage {
    let mut st = McmPage::default();
    // The page MCM opens first is the one marked `active`; [`Source::pages`] puts it first.
    st.m = menus.menus.iter().position(|s| !s.pages().is_empty()).unwrap_or(0);
    st
}

/// The chosen page's controls, in number order.
#[cfg(test)]
fn controls_of(s: &Source, st: &McmPage) -> Vec<Control> {
    s.pages().get(st.p).map(|p| p.controls.clone()).unwrap_or_default()
}

fn next_row(controls: &[Control], from: usize, step: i32) -> usize {
    if controls.is_empty() {
        return 0;
    }
    let n = controls.len() as i32;
    let mut i = from as i32;
    for _ in 0..n {
        i = (i + step).rem_euclid(n);
        if controls[i as usize].enable && controls[i as usize].kind != Kind::Section {
            return i as usize;
        }
    }
    from.min(controls.len() - 1)
}

/// Apply one action to the world: the keys' and the pointer's whole page logic. Called
/// from `crate::pause`'s key handling through `commands.queue` (it needs the game, the
/// files and the page state at once).
pub fn key(world: &mut World, action: Action) {
    let mut st = world.get_resource_mut::<McmPage>().map(|s| s.clone()).unwrap_or_default();
    let (count, names, source) = {
        let Some(menus) = world.get_resource::<McmMenus>() else { return };
        let count = menus.menus.len();
        let names: Vec<String> = menus.menus.iter().map(|s| s.name()).collect();
        (count, names, menus.menus.get(st.m).cloned())
    };
    let back_to_column = |world: &mut World| {
        if let Some(mut page) = world.get_resource_mut::<Page>() {
            if *page == Page::Mcms {
                info!("mcm: page closed");
                *page = Page::Main;
            }
        }
    };
    if count == 0 {
        back_to_column(world);
        return;
    }
    st.m = st.m.min(count - 1);
    if matches!(action, Action::Back | Action::BackRow) {
        back_to_column(world);
        return;
    }
    // The chosen menu, copied out: the world is borrowed mutably below.
    let Some(source) = source else { return };
    let pages: Vec<mcm::Submenu> = source.pages().into_iter().cloned().collect();
    let controls = pages.get(st.p).map(|p| p.controls.clone()).unwrap_or_default();
    match action {
        Action::Back | Action::BackRow => unreachable!("handled above"),
        Action::Up | Action::Down => {
            st.row = next_row(&controls, st.row, if action == Action::Up { -1 } else { 1 });
            if st.row < st.first {
                st.first = st.row;
            } else if st.row >= st.first + ROWS {
                st.first = st.row + 1 - ROWS;
            }
        }
        Action::NextPage(dir) => {
            if !pages.is_empty() {
                let n = pages.len() as i32;
                st.p = ((st.p as i32 + dir).rem_euclid(n)) as usize;
                st.row = 0;
                st.first = 0;
                st.status = format!("{}: {}", source.name(), source.text(&pages[st.p].page_title));
                info!("mcm: {} page {} {:?}", source.name(), pages[st.p].id, source.text(&pages[st.p].list_title));
            }
        }
        Action::Mod(i) => {
            if i < count {
                st.m = i;
                st.p = 0;
                st.row = 0;
                st.first = 0;
                let name = names.get(i).cloned().unwrap_or_default();
                st.status = name.clone();
                info!("mcm: chose {name}");
            }
        }
        Action::Page(i) => {
            if i < pages.len() {
                st.p = i;
                st.row = 0;
                st.first = 0;
                st.status = source.text(&pages[i].page_title);
            }
        }
        Action::Row(i) => {
            if let Some(c) = controls.get(i) {
                st.row = i;
                if !c.enable {
                    st.status = "this row is off (`enable` 0)".to_string();
                } else {
                    take(world, &source, c, 1, &mut st);
                }
            }
        }
        Action::Left | Action::Right => {
            if let Some(c) = controls.get(st.row) {
                take(world, &source, c, if action == Action::Right { 1 } else { -1 }, &mut st);
            }
        }
        Action::Take => {
            if let Some(c) = controls.get(st.row).filter(|c| c.enable) {
                take(world, &source, c, 1, &mut st);
            }
        }
    }
    if let Some(mut now) = world.get_resource_mut::<McmPage>() {
        *now = st;
    }
}

/// Change control `c` one step, write it, and say what happened (in `st.status`).
fn take(world: &mut World, s: &Source, c: &Control, dir: i32, st: &mut McmPage) {
    let Some((section, key)) = c.config() else {
        st.status = format!("{}: no `configINI` to write", c.title);
        warn!("mcm: {}: {} has no configINI", s.name(), c.title);
        return;
    };
    let Some(script) = world.get_resource_mut::<ScriptState>() else { return };
    let mut script = script;
    let Some(rt) = script.rt.as_mut() else { return };
    let game = &mut rt.game;
    if let Some(why) = c.kind.unsupported() {
        st.status = format!("{}: {why} (TODO)", s.text(&c.title));
        warn!("mcm: {}: control {} ({}): {why}; nothing written", s.name(), c.id, c.title);
        return;
    }
    if !c.kind.changes() {
        return;
    }
    let old = mcm::value_in(game, s, c);
    let new = c.stepped(old, dir);
    if (new - old).abs() < 1e-9 {
        st.status = format!("{}: {}", s.text(&c.title), c.show(old));
        return;
    }
    let text = c.stored(new);
    match mcm::set_value(game, s, section, key, &text) {
        Ok((rel, path)) => {
            st.status = format!("{}: {} ({rel})", s.text(&c.title), c.show(new));
            info!("mcm: {}: {section}:{key} = {text} ({}) [{rel}]", s.name(), path.display());
        }
        Err(e) => {
            st.status = format!("{}: not written: {e}", s.text(&c.title));
            warn!("mcm: {}: {section}:{key}: not written: {e}", s.name());
            return;
        }
    }
    if let Some(call) = &c.call {
        match mcm::run_call(game, call) {
            Some(Ok(())) => {
                info!("mcm: {}: ran {} ({})", s.name(), call.value, call.kind);
                st.status.push_str(&format!("; ran {}", call.value));
            }
            Some(Err(e)) => {
                warn!("mcm: {}: {} did not run: {e}", s.name(), call.value);
                st.status.push_str(&format!("; {} did not run: {e}", call.value));
            }
            None => {}
        }
    }
}

/// The switch's settings row and the page's systems.
pub fn setup(app: &mut App) {
    app.init_resource::<McmSwitch>()
        .init_resource::<McmEntry>()
        .init_resource::<McmPage>()
        .init_resource::<McmMenus>()
        .add_systems(Update, (load, refresh_entry).chain().before(crate::PlaySet::Ui))
        .add_systems(Update, (feed, draw).chain().in_set(crate::PlaySet::Ui));
    crate::settings::add_setting(
        app,
        crate::settings::Setting {
            key: "mcm-menus",
            group: crate::pause::GROUP_MENUS,
            label: "MCM menus",
            note: "Show the settings the installed mods describe in MCM/*.json on a page of the pause menu.",
            switches: "--mcm-menus",
            get: |w| w.resource::<McmSwitch>().0,
            set: |w, on| w.resource_mut::<McmSwitch>().0 = on,
            slider: None,
        },
    );
}

/// The pointer: a click on a row the page drew. Runs before the pause menu's own key
/// handling reads what it queued, and only while the page is up.
#[allow(clippy::too_many_arguments)]
fn feed(
    paused: Res<Paused>,
    page: Res<Page>,
    switch: Res<McmSwitch>,
    menus: Res<McmMenus>,
    mut st: ResMut<McmPage>,
    hits: Query<(&PointerState, &McmHit), Changed<PointerState>>,
    mut commands: Commands,
) {
    let on = paused.0 && *page == Page::Mcms && switch.0 && !menus.menus.is_empty();
    if !on {
        st.frames = 0;
        return;
    }
    st.frames += 1;
    if st.frames == 1 {
        info!("mcm: the Mod settings page opens on {}", menus.menus.get(st.m).map(|s| s.name()).unwrap_or_default());
    }
    for (state, hit) in &hits {
        if *state != PointerState::Pressed {
            continue;
        }
        let action = match hit.0 {
            Hit::Mod(i) => Action::Mod(i),
            Hit::Page(i) => Action::Page(i),
            Hit::Row(i) => Action::Row(i),
            Hit::Back => Action::BackRow,
        };
        commands.queue(move |w: &mut World| key(w, action));
    }
}

/// Draw the page over the pause menu.
#[allow(clippy::too_many_arguments)]
fn draw(
    mut commands: Commands,
    roots: Query<Entity, With<McmRoot>>,
    paused: Res<Paused>,
    page: Res<Page>,
    switch: Res<McmSwitch>,
    menus: Res<McmMenus>,
    st: Res<McmPage>,
    script: Res<ScriptState>,
    windows: Query<&Window, With<PrimaryWindow>>,
    look: Looked,
) {
    for e in &roots {
        commands.entity(e).despawn();
    }
    let on = paused.0 && *page == Page::Mcms && switch.0 && !menus.menus.is_empty();
    if !on {
        return;
    }
    let (mut look, data, mut images, formats) = look;
    let bc = crate::interface::bc_supported(formats.as_deref());
    let faces = look.faces.get_or_insert_with(|| Faces::load(data.as_deref(), &mut images, bc));
    let Some(s) = menus.menus.get(st.m) else { return };
    let view_w = windows.single().map_or(1480.0, |w| 960.0 * w.width() / w.height());
    let (left, width) = band(view_w);
    let pages = s.pages();
    let controls = pages.get(st.p).map(|p| p.controls.clone()).unwrap_or_default();
    let game = script.rt.as_ref().map(|rt| &rt.game);
    commands.spawn((McmRoot, Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() }, GlobalZIndex(Z))).with_children(|root| {
        // The band behind the page, as the settings page's own.
        root.spawn((at(left - 30.0, 0.0, width + 60.0, 960.0), BackgroundColor(Color::BLACK.with_alpha(0.45))));
        in_menu_frame(root, crate::interface::MIDDLE, |p| {
            let f = faces;
            title(p, f, s, pages.get(st.p).copied(), left, width);
            // The mods' rows.
            for (i, m) in menus.menus.iter().enumerate() {
                let y = ROWS_TOP + i as f32 * ROW;
                row(p, f, left, y, MODS_W, &m.name(), "", i == st.m, Some(Hit::Mod(i)));
            }
            // The pages' rows.
            let px = left + MODS_W + GAP;
            for (i, pg) in pages.iter().enumerate() {
                let y = ROWS_TOP + i as f32 * ROW;
                row(p, f, px, y, PAGES_W, &s.text(&pg.list_title), "", i == st.p, Some(Hit::Page(i)));
            }
            // The controls.
            let ox = px + PAGES_W + GAP;
            let ow = (left + width - ox).max(240.0);
            let columns = pages.get(st.p).map_or(1, |p| p.columns).max(1);
            if controls.is_empty() {
                text(p, f, "This page has no controls.", ox, ROWS_TOP, shade(Shade::Dim));
            }
            let first = st.first.min(controls.len().saturating_sub(1));
            for (n, (i, c)) in controls.iter().enumerate().skip(first).take(ROWS * columns).enumerate() {
                let col = (n % columns) as f32;
                let line = (n / columns) as f32;
                let y = ROWS_TOP + line * ROW;
                let x = ox + col * (ow / columns as f32);
                let w = ow / columns as f32 - GAP;
                if c.kind == Kind::Section {
                    let words = fit(f, &s.text(&c.title), w);
                    text(p, f, &words, x, y + ROW * 0.55, shade(Shade::Dim));
                    continue;
                }
                let value = match game {
                    Some(g) => c.show(mcm::value_in(g, s, c)),
                    None => c.show(c.default()),
                };
                let value = s.text(&value);
                let dim = if !c.enable { " (off)" } else { "" };
                let value = format!("{value}{dim}");
                row(p, f, x, y, w, &s.text(&c.title), &value, i == st.row && c.enable, Some(Hit::Row(i)));
            }
            // The chosen row's note, the status line and the keys' line.
            let note = controls.get(st.row).and_then(|c| c.note.as_ref()).map(|n| s.text(n)).unwrap_or_default();
            let note = if st.status.is_empty() { note } else { st.status.clone() };
            let bottom = ROWS_TOP + ROWS as f32 * ROW;
            let words = fit(f, &s.text(&note), left + width - left);
            small(p, f, &words, left, bottom + 10.0, shade(Shade::Dim));
            let hint = "Up/Down choose   Left/Right change   Enter take   Tab page   Esc back";
            small(p, f, hint, left, bottom + 40.0, shade(Shade::Dim));
            // The Back row, at the column's left as the settings page's.
            let back = "Back";
            let y = 960.0 - 2.0 * ROW;
            row(p, f, left + width - 200.0, y, 200.0, back, "", false, Some(Hit::Back));
        });
    });
}

/// The title and the page's own title.
fn title(p: &mut ChildSpawnerCommands, f: &Faces, s: &Source, page: Option<&mcm::Submenu>, left: f32, width: f32) {
    let name = s.name();
    text(p, f, &fit(f, &name, width), left, TITLE_TOP, shade(Shade::Bright));
    if let Some(pg) = page {
        let words = s.text(&pg.page_title);
        let words = if words.trim().is_empty() { s.text(&pg.list_title) } else { words };
        small(p, f, &fit(f, &words, width), left, TITLE_TOP + 30.0, shade(Shade::Dim));
    }
    // A line under them, as the pause menu's own.
    p.spawn((at(left, TABS_TOP - 24.0, width, LINE), BackgroundColor(shade(Shade::Dim))));
}

fn band(view_w: f32) -> (f32, f32) {
    let right = view_w / 2.0 - (RIGHT + PAUSE_COLUMN) - 30.0;
    let left = -view_w / 2.0 + 90.0;
    (left, (right - left).max(360.0))
}

fn shade(s: Shade) -> Color {
    crate::start_menu::color(s)
}

fn text(p: &mut ChildSpawnerCommands, f: &Faces, s: &str, x: f32, base: f32, color: Color) {
    crate::start_menu::text(p, &f.entry, s, x, base, color);
}

/// A line in the small face (font 3), the pause menu's own for notes.
fn small(p: &mut ChildSpawnerCommands, f: &Faces, s: &str, x: f32, base: f32, color: Color) {
    crate::start_menu::text(p, &f.small, s, x, base, color);
}

/// A row: its label at the left, its value at the right, boxed when chosen. The pointer
/// hits it through [`McmHit`].
fn row(p: &mut ChildSpawnerCommands, f: &Faces, x: f32, y: f32, w: f32, label: &str, value: &str, on: bool, hit: Option<Hit>) {
    let node = at(x, y, w, ROW);
    if on {
        let (line, fill) = crate::start_menu::highlight_colors();
        let mut e = p.spawn((Button, node, BorderColor::all(line), BackgroundColor(fill)));
        if let Some(h) = hit {
            e.insert(McmHit(h));
        }
        e.with_children(|r| {
            let label = fit(f, label, w - VALUE_W - 20.0);
            text(r, f, &label, 10.0, ROW * 0.72, shade(Shade::Bright));
            let value = fit(f, value, VALUE_W - 10.0);
            let vw = f.entry.font.width(&value);
            text(r, f, &value, w - vw - 10.0, ROW * 0.72, shade(Shade::Bright));
        });
    } else {
        let mut e = p.spawn((Button, node));
        if let Some(h) = hit {
            e.insert(McmHit(h));
        }
        e.with_children(|r| {
            let label = fit(f, label, w - VALUE_W - 20.0);
            text(r, f, &label, 10.0, ROW * 0.72, shade(Shade::Bright));
            let value = fit(f, value, VALUE_W - 10.0);
            let vw = f.entry.font.width(&value);
            text(r, f, &value, w - vw - 10.0, ROW * 0.72, shade(Shade::Dim));
        });
    }
}

/// `s` cut to `room` units, its end replaced by "..." when it does not fit.
fn fit(f: &Faces, s: &str, room: f32) -> String {
    if f.entry.font.width(s) <= room {
        return s.to_string();
    }
    if f.entry.font.width("...") > room {
        return String::new();
    }
    let mut out = String::new();
    for ch in s.chars() {
        let mut next = out.clone();
        next.push(ch);
        next.push_str("...");
        if f.entry.font.width(&next) > room {
            break;
        }
        out.push(ch);
    }
    out.push_str("...");
    out
}

/// A splash's picture, when the menu draws one (a type-0 control with a `filename`): the
/// mod's own file through the VFS, as the other menus' pictures are.
pub fn picture(_s: &Source, control: &Control, data: &GameData, images: &mut Assets<Image>, bc: bool) -> Option<Handle<Image>> {
    let file = control.image.as_ref()?;
    // A mod's picture path is Data-relative (`Interface/...`), and the VFS already has the
    // mod folders as roots.
    texture(file, data, images, bc).map(|(h, _)| h)
}

/// Whether a menu source's file lives in a folder that is no longer there (a test of the
/// switch's other side).
pub fn still_there(s: &Source) -> bool {
    s.folder.is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fnv_game::mcm::{Control, Menu, Submenu, Var};
    use std::path::PathBuf;

    fn control(id: u32, kind: Kind, enable: bool) -> Control {
        Control {
            id,
            title: format!("c{id}"),
            kind,
            enable,
            note: None,
            suffix: String::new(),
            strings: vec!["a".into(), "b".into()],
            text_on: None,
            text_off: None,
            scale: None,
            var: Some(Var { config: Some(("S".into(), format!("k{id}"))), default: 0.0, ..Default::default() }),
            call: None,
            image: None,
            unknown: Vec::new(),
        }
    }

    fn menus() -> McmMenus {
        let menu = Menu {
            mod_name: "M".into(),
            display_name: "M".into(),
            description: String::new(),
            save_file: "M.ini".into(),
            min_mcm_version: None,
            min_mod_version: None,
            requirements: Vec::new(),
            submenus: vec![
                Submenu { id: 0, list_title: "zero".into(), page_title: "zero".into(), columns: 1, enable: true, active: true, state: None, controls: vec![control(1, Kind::Section, true)], unknown: Vec::new() },
                Submenu {
                    id: 1,
                    list_title: "one".into(),
                    page_title: "one".into(),
                    columns: 1,
                    enable: true,
                    active: false,
                    state: None,
                    controls: vec![control(1, Kind::Section, true), control(2, Kind::Checkbox, true), control(3, Kind::Checkbox, false), control(4, Kind::List, true), control(5, Kind::Slider { decimals: 0 }, true)],
                    unknown: Vec::new(),
                },
            ],
            unknown: Vec::new(),
        };
        McmMenus { loaded: true, menus: vec![Source { folder: PathBuf::from("/nonexistent"), rel: "MCM\\M.json".into(), menu, translations: Default::default() }], errors: Vec::new() }
    }

    #[test]
    fn the_entry_needs_the_switch_and_a_menu() {
        let mut m = menus();
        let on = McmSwitch(true);
        let off = McmSwitch(false);
        let entry = |menus: &McmMenus, switch: McmSwitch| switch.0 && !menus.menus.is_empty();
        assert!(entry(&m, on));
        assert!(!entry(&m, off), "the switch off");
        m.menus.clear();
        assert!(!entry(&m, on), "no menu");
    }

    #[test]
    fn the_keys_pass_off_and_toggles_and_wrap_lists() {
        let m = menus();
        let s = &m.menus[0];
        let st = McmPage::default();
        let c = controls_of(s, &st);
        assert_eq!(c.len(), 1, "the first page has one section");
        // Page 1's rows: the section is passed over, the off control too.
        let mut st = st;
        st.p = 1;
        let c = controls_of(s, &st);
        assert_eq!(st.row, 0);
        assert_eq!(next_row(&c, 0, 1), 1, "the section is skipped");
        assert_eq!(next_row(&c, 1, 1), 3, "the row with enable 0 is skipped");
        assert_eq!(next_row(&c, 3, 1), 4);
        assert_eq!(next_row(&c, 4, 1), 1, "it wraps");
        assert_eq!(next_row(&c, 1, -1), 4);
        // Values: a list wraps, a switch flips, a slider steps.
        let list = &c[3];
        assert_eq!(list.stepped(1.0, 1), 0.0);
        assert_eq!(c[1].stepped(0.0, 1), 1.0);
        assert_eq!(c[1].stepped(1.0, -1), 0.0);
        assert!(c[4].editable());
        // A section is not editable and says nothing.
        assert!(!c[0].editable());
        assert_eq!(c[0].kind.unsupported(), None);
    }

    #[test]
    fn the_fit_cuts_a_long_title_rather_than_overrunning_the_row() {
        let f = Faces::plain();
        assert_eq!(fit(&f, "short", 200.0), "short");
        let cut = fit(&f, "a very long mod title that cannot fit into the row", 80.0);
        assert!(cut.ends_with("..."));
        assert!(f.entry.font.width(&cut) <= 80.0, "{cut:?}");
        assert_eq!(fit(&f, "a very long mod title", 1.0), "", "no room even for the dots");
    }
}
