//! mods: the Mods page of the options menu, also opened on its own from the
//! game's Main Menu (overrides/mods.c), drawn like the rest of the menu in
//! the game's art and font: a row per mod with its name, version and author
//! and an On/Off pill, the rows that open the mods folder and restart the
//! game, a details box with the selected mod's description and notes, and
//! the game's Yes/No dialog to confirm a restart.
//!
//! The list itself is crate::haiku_mods::ModList, in FatesUi; the page sees
//! a copy of what it shows (ModsView, in the options' Env) and asks the app
//! for changes through Effect::Mods, as the other options do.

use egui::Color32;

use super::kit::{Art, Canvas, TEXT_CREAM, TEXT_DARK, TEXT_GOLD};
use super::options::{Effect, Env, Item, PANEL_H, PANEL_W, PANEL_X, PANEL_Y, PILL_W, ROW, ROWS_TOP, WIDTH};
use super::{Nav, Prompt};
use crate::controls::GlyphSet;
use crate::haiku_mods::{ModList, Note};
use crate::settings::CodeModsCompiler;
use crate::App;

/// how many rows the Mods page shows, the details box below them.
pub const VISIBLE: usize = 5;
/// the details box, in units.
const DETAILS_Y: f32 = ROWS_TOP + VISIBLE as f32 * ROW + 3.0;
const DETAILS_H: f32 = PANEL_Y + PANEL_H - 9.0 - DETAILS_Y;
/// a warning's colour: the game's red, as its "cannot" notes are drawn.
const TEXT_WARN: Color32 = Color32::from_rgb(176, 24, 8);
const TEXT_SOFT: Color32 = Color32::from_rgb(120, 76, 52);

/// a mod as the page shows it.
#[derive(Clone, Debug, Default)]
pub struct ModRow {
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub enabled: bool,
    pub can_enable: bool,
    pub notes: Vec<Note>,
}

/// what the page shows: the mods in load order, and whether the game runs
/// with a different list than this one.
#[derive(Clone, Debug, Default)]
pub struct ModsView {
    pub rows: Vec<ModRow>,
    pub pending: bool,
    pub folder: String,
    /// codemods: the code mods being compiled, while they are.
    pub job: Option<super::codemods::JobView>,
    /// codemods: the compiler download to ask about (its size in MB).
    pub ask_download: Option<super::codemods::Ask>,
    /// zigdl: whether the compiler row shows: Haiku Setup's compiler is
    /// there and a mod has code
    pub compiler_row: bool,
}

impl ModsView {
    pub fn of(list: &ModList) -> ModsView {
        ModsView {
            rows: list
                .mods
                .iter()
                .enumerate()
                .map(|(index, info)| ModRow {
                    name: info.name.clone(),
                    version: info.version.clone(),
                    author: info.author.clone(),
                    description: info.description.clone(),
                    enabled: list.enabled[index],
                    can_enable: list.can_enable(index),
                    notes: list.notes.get(index).cloned().unwrap_or_default(),
                })
                .collect(),
            pending: list.pending(),
            folder: list.folder.display().to_string(),
            job: None,
            ask_download: None,
            // zigdl
            compiler_row: list.mods.iter().any(|info| info.code_files.as_ref().is_some_and(|files| files.changes_code()))
                && crate::haiku_codemods::setup_compiler().is_some(),
        }
    }
}

/// a list of mods for the screenshots: the examples, one that shares a
/// file with another, and one that changes the game's code.
pub fn sample() -> ModsView {
    let row = |name: &str, version: &str, author: &str, description: &str, enabled: bool, can_enable: bool, notes: Vec<Note>| ModRow {
        name: name.into(),
        version: version.into(),
        author: author.into(),
        description: description.into(),
        enabled,
        can_enable,
        notes,
    };
    ModsView {
        rows: vec![
            row("Example: Title Text", "1.0", "Haiku", "Renames the Main Menu's New Game to Begin Anew, through the game's own message file (m/@E/TitleMenu.bin.lz).", true, true, vec![]),
            row(
                "Example: Golden Menus",
                "1.0",
                "Haiku",
                "A texture-pack mod: the menus' parchment window drawn in gold.",
                true,
                true,
                vec![Note { warning: false, text: "Replaces 1 file of Example: Title Text, which loads before it.".into() }],
            ),
            row("Better Support Names", "2.3", "Kiran", "Fixes a few support conversation names.", false, true, vec![]),
            row(
                "Gameplay Tweaks",
                "0.9",
                "Someone",
                "Changes growth rates and the experience curve.",
                true,
                true,
                vec![
                    Note { warning: false, text: crate::haiku_codemods::own_saves_note("gameplay-tweaks") }, // codemods2
                    Note { warning: false, text: "Code mod (code.ips): Haiku recompiles the code it changes.".into() },
                    Note { warning: false, text: "Compiled: 3 of the game's functions run patched.".into() },
                ],
            ),
        ],
        pending: true,
        folder: r"C:\Games\Haiku\mods".into(),
        job: None,
        ask_download: None,
        compiler_row: false, // zigdl
    }
}

/// what the page asks of the app.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModsEffect {
    /// turn the mod at an index on or off.
    Toggle(usize, bool),
    /// move the mod at an index up (-1) or down (1) the load order.
    Move(usize, i32),
    OpenFolder,
    /// restart the game with the mods as they are now set.
    Restart,
    /// codemods: the answer to the compiler download question.
    Download(bool),
    /// zigdl: the other answer: the compiler that came with Haiku.
    UseSetupCompiler,
    /// zigdl: the compiler row changed.
    Compiler(CodeModsCompiler),
    /// codemods: stop compiling.
    CancelJob,
}

thread_local! {
    /// zigdl: the compiler row's choice, when the page shows the row (set
    /// each frame from the view and the settings).
    static COMPILER_ROW: std::cell::Cell<Option<CodeModsCompiler>> = const { std::cell::Cell::new(None) };
}

/// zigdl: shows the compiler row with choice, or not (None).
pub fn set_compiler_row(choice: Option<CodeModsCompiler>) {
    COMPILER_ROW.with(|row| row.set(choice));
}

fn compiler_row() -> Option<CodeModsCompiler> {
    COMPILER_ROW.with(|row| row.get())
}

/// the page's rows.
pub fn items(count: usize) -> Vec<Item> {
    let mut items: Vec<Item> = (0..count).map(Item::Mod).collect();
    if count == 0 {
        items.push(Item::ModsNone);
    }
    items.push(Item::ModsFolder);
    if compiler_row().is_some() {
        items.push(Item::ModsCompiler); // zigdl
    }
    items.push(Item::ModsApply);
    items
}

/// zigdl: the compiler row's value.
fn compiler_name(choice: CodeModsCompiler) -> &'static str {
    match choice {
        CodeModsCompiler::Ask => "Ask",
        CodeModsCompiler::Zig => "Zig",
        CodeModsCompiler::Haiku => "Haiku's",
    }
}

/// a row's label, value and help line.
pub fn row(item: Item, env: &Env) -> (String, Option<String>, String) {
    let mods = &env.mods;
    match item {
        Item::Mod(index) => {
            let Some(row) = mods.rows.get(index) else { return (String::new(), None, String::new()) };
            let value = if !row.can_enable { "Unavailable" } else if row.enabled { "On" } else { "Off" };
            (row.name.clone(), Some(value.to_owned()), "Changes take effect when the game restarts.".to_owned())
        }
        Item::ModsNone => ("No mods installed".to_owned(), None, "Put mods in the mods folder next to Haiku.exe, then open this page again.".to_owned()),
        Item::ModsFolder => ("Open Mods Folder".to_owned(), None, format!("Shows the mods folder: {}", mods.folder)),
        // zigdl: Zig, the compiler that came with Haiku, or ask
        Item::ModsCompiler => {
            let choice = compiler_row().unwrap_or_default();
            let help = match choice {
                CodeModsCompiler::Ask => "Code mods need a compiler: asks which the first time.",
                CodeModsCompiler::Zig => "Zig: downloaded once, compiles code mods in under a minute.",
                CodeModsCompiler::Haiku => "The compiler that came with Haiku: no download, but slower.",
            };
            ("Code Mod Compiler".to_owned(), Some(compiler_name(choice).to_owned()), help.to_owned())
        }
        Item::ModsApply => {
            let help = if mods.pending {
                "Restarts the game with the mods as set here. Progress since you last saved is lost."
            } else {
                "The game is running with the mods as set here."
            };
            ("Apply and Restart".to_owned(), None, help.to_owned())
        }
        _ => (String::new(), None, String::new()),
    }
}

/// what a press of a row does: dir -1 or 1 from left and right, 0 pressed.
pub fn adjust(item: Item, dir: i32, env: &Env) -> Option<Effect> {
    match item {
        Item::Mod(index) => {
            // left, right and a press all turn it the other way
            let _ = dir;
            let row = env.mods.rows.get(index)?;
            let on = !row.enabled;
            (row.can_enable || !on).then_some(Effect::Mods(ModsEffect::Toggle(index, on)))
        }
        Item::ModsFolder if dir == 0 => Some(Effect::Mods(ModsEffect::OpenFolder)),
        // zigdl: left goes back, right and a press go on
        Item::ModsCompiler => {
            let order = [CodeModsCompiler::Ask, CodeModsCompiler::Zig, CodeModsCompiler::Haiku];
            let at = order.iter().position(|choice| Some(*choice) == compiler_row()).unwrap_or(0);
            let next = if dir < 0 { (at + order.len() - 1) % order.len() } else { (at + 1) % order.len() };
            Some(Effect::Mods(ModsEffect::Compiler(order[next])))
        }
        _ => None,
    }
}

/// a mod's name, shortened with an ellipsis to fit before its pill and the
/// arrows that move it. Other rows' labels as they are.
pub fn fit_label(canvas: &Canvas, item: Item, label: String, room: f32) -> String {
    if !matches!(item, Item::Mod(_) | Item::Cheat(_)) || canvas.kit.measure(&label, 0.95) <= room { // cheats: long names too
        return label;
    }
    let mut text = label;
    while canvas.kit.measure(&format!("{text}..."), 0.95) > room && text.pop().is_some() {}
    format!("{}...", text.trim_end())
}

/// the second line of a mod's row: its version and author.
fn meta(row: &ModRow) -> String {
    match (row.version.is_empty(), row.author.is_empty()) {
        (false, false) => format!("v{} \u{b7} {}", row.version.trim_start_matches(['v', 'V']), row.author),
        (false, true) => format!("v{}", row.version.trim_start_matches(['v', 'V'])),
        (true, false) => row.author.clone(),
        (true, true) => String::new(),
    }
}

/// where the move arrows of the selected mod's row are, in units.
pub fn move_arrows(y: f32) -> [(f32, f32, i32); 2] {
    let pill_x = PANEL_X + PANEL_W - 24.0 - PILL_W;
    [(pill_x - 30.0, y + 2.0, -1), (pill_x - 16.0, y + 2.0, 1)]
}

/// draws what a mod's row has besides its label and pill: its version and
/// author after the name, a warning mark, and on the selected row the
/// arrows that move it.
pub fn draw_row_extras(canvas: &Canvas, item: Item, env: &Env, x: f32, y: f32, label_width: f32, chosen: bool, now: f64) {
    let Item::Mod(index) = item else { return };
    let Some(row) = env.mods.rows.get(index) else { return };
    let pill_x = PANEL_X + PANEL_W - 24.0 - PILL_W;
    let mut pen = x + label_width + 6.0;
    let room_end = if chosen { pill_x - 34.0 } else { pill_x - 8.0 };
    if row.notes.iter().any(|note| note.warning) {
        let colour = if chosen { TEXT_GOLD } else { TEXT_WARN };
        pen += canvas.text(pen, y + 2.0, "!", 0.95, colour) + 5.0;
    }
    // the version and author, or the version alone, as fit
    let scale = 0.7;
    let version = if row.version.is_empty() { String::new() } else { format!("v{}", row.version.trim_start_matches(['v', 'V'])) };
    let text = [meta(row), version].into_iter().find(|text| !text.is_empty() && canvas.kit.measure(text, scale) <= room_end - pen);
    if let Some(text) = text {
        {
            let colour = if chosen { Color32::from_rgb(250, 214, 170) } else { TEXT_SOFT };
            canvas.text(pen, y + 5.0, &text, scale, colour);
        }
    }
    if chosen {
        let white = Color32::WHITE;
        let bob = ((now * 4.0).sin() * 0.8) as f32;
        let [(ux, uy, _), (dx, dy, _)] = move_arrows(y);
        canvas.image(Art::ScrollUp, ux, uy - bob, 12.0, 7.0, white);
        canvas.image(Art::ScrollDown, dx, dy + 7.0 + bob, 12.0, 7.0, white);
    }
}

/// the details box under the rows: the selected mod's name, version and
/// author, its description and its notes, warnings in red.
pub fn draw_details(canvas: &Canvas, item: Item, env: &Env) {
    canvas.fill(PANEL_X + 26.0, DETAILS_Y - 1.5, PANEL_W - 52.0, 0.8, Color32::from_rgba_unmultiplied(70, 110, 110, 110));
    // fanpatch: what doesn't fit is wrapped again a little smaller first
    let lines = details_lines(canvas, item, env, 0.72);
    let fits = |lines: &[(String, f32, Color32)], line: f32| lines.iter().map(|(_, scale, _)| if *scale < 0.8 { line } else { 15.0 }).sum::<f32>() <= DETAILS_H + 2.0;
    if !fits(&lines, 13.0) {
        draw_detail_lines(canvas, &details_lines(canvas, item, env, 0.62), 0.62, 11.0);
        return;
    }
    draw_detail_lines(canvas, &lines, 0.72, 13.0);
}

/// the details box's lines at a text scale (fanpatch: split from draw_details).
fn details_lines(canvas: &Canvas, item: Item, env: &Env, small: f32) -> Vec<(String, f32, Color32)> {
    let width = PANEL_W - 60.0;
    let mut lines: Vec<(String, f32, Color32)> = Vec::new();
    match item {
        Item::Mod(index) => {
            let Some(row) = env.mods.rows.get(index) else { return lines };
            let mut title = row.name.clone();
            let meta = meta(row);
            if !meta.is_empty() {
                title = format!("{title}   {meta}");
            }
            lines.push((title, 0.8, TEXT_DARK));
            let description = if row.description.is_empty() { "No description." } else { &row.description };
            for line in canvas.kit.wrap(description, width, small) {
                lines.push((line, small, TEXT_SOFT));
            }
            for note in &row.notes {
                for line in canvas.kit.wrap(&note.text, width, small) {
                    lines.push((line, small, if note.warning { TEXT_WARN } else { TEXT_DARK }));
                }
            }
        }
        Item::ModsNone | Item::ModsFolder => {
            lines.push(("Mods folder".to_owned(), 0.8, TEXT_DARK));
            for line in canvas.kit.wrap(&env.mods.folder, width, small) {
                lines.push((line, small, TEXT_SOFT));
            }
            for line in canvas.kit.wrap("Each mod is a folder or a .zip. Mods made for Luma3DS work as they are.", width, small) {
                lines.push((line, small, TEXT_SOFT));
            }
        }
        Item::ModsCompiler => {
            // zigdl: what the two compilers are
            lines.push(("Code mod compiler".to_owned(), 0.8, TEXT_DARK));
            let text = "Zig is a one-time download of about 100 MB and compiles code mods in under a minute. The compiler that came with Haiku needs no download, but a big code mod takes several minutes. When Zig can't be downloaded, Haiku's is used.";
            for line in canvas.kit.wrap(text, width, small) {
                lines.push((line, small, TEXT_SOFT));
            }
        }
        Item::ModsApply => {
            let on = env.mods.rows.iter().filter(|row| row.enabled).count();
            lines.push((format!("{on} {} on", if on == 1 { "mod" } else { "mods" }), 0.8, TEXT_DARK));
            let text = if env.mods.pending {
                "The list has changed since the game started. Restart to play with it."
            } else {
                "Nothing has changed since the game started."
            };
            for line in canvas.kit.wrap(text, width, small) {
                lines.push((line, small, if env.mods.pending { TEXT_WARN } else { TEXT_SOFT }));
            }
        }
        _ => return lines,
    }
    lines
}

/// draws the details box's lines, small ones `line` units apart.
fn draw_detail_lines(canvas: &Canvas, lines: &[(String, f32, Color32)], small: f32, line: f32) {
    let x = PANEL_X + 30.0;
    let width = PANEL_W - 60.0;
    let mut y = DETAILS_Y + 1.0;
    let bottom = DETAILS_Y + DETAILS_H;
    for (index, (text, scale, colour)) in lines.iter().enumerate() {
        let height = if *scale < 0.8 { line } else { 15.0 };
        if y + height > bottom + 2.0 {
            // what doesn't fit ends in an ellipsis on the last line
            if index > 0 {
                canvas.text(x + width - canvas.kit.measure("...", small), y - height, "...", small, TEXT_SOFT);
            }
            break;
        }
        canvas.text(x, y, text, *scale, *colour);
        y += height;
    }
}

/// the prompts the header shows on the Mods page.
pub fn prompts(item: Item, mods_only: bool, set: GlyphSet) -> Vec<(Prompt, &'static str)> {
    let _ = mods_only;
    let back = "Close";
    // hdpacks: while a download is unpacked, only closing the menu does
    if crate::haiku_import::running().is_some() {
        return vec![(Prompt::Back, back)];
    }
    match item {
        Item::Mod(_) if set == GlyphSet::Keyboard => vec![(Prompt::Confirm, "On/Off"), (Prompt::Back, back)],
        Item::Mod(_) => vec![(Prompt::Confirm, "On/Off"), (Prompt::Extra, "Up"), (Prompt::Extra2, "Down"), (Prompt::Back, back)],
        _ => vec![(Prompt::Confirm, "Select"), (Prompt::Back, back)],
    }
}

/// the keyboard's move keys, shown as the game's dark pills in the footer's
/// corner on the keyboard.
pub fn draw_keyboard_hint(canvas: &Canvas, y: f32) {
    let text = "Shift+Up/Down: Move";
    let width = canvas.kit.measure(text, 0.62) + 10.0;
    canvas.nine(Art::PillLabel, PANEL_X + PANEL_W - 24.0 - width, y, width, 13.0, [8.0, 8.0, 8.0, 8.0], 0.5, Color32::WHITE);
    canvas.text_centered(PANEL_X + PANEL_W - 24.0 - width / 2.0, y + 1.5, text, 0.62, TEXT_CREAM);
}

/// hdpacks: the dialog showing a download being unpacked, the game's
/// checkered box with what is unpacked, how far it is on the game's red
/// bar, and that it goes on with the menu closed.
pub fn draw_unpacking(progress: &crate::haiku_import::Progress, canvas: &Canvas, now: f64) {
    let white = Color32::WHITE;
    canvas.fill(0.0, 0.0, WIDTH, super::options::HEIGHT, Color32::from_black_alpha(90));
    let title = format!("Unpacking {}", progress.name);
    let lines = canvas.kit.wrap_even(&title, CONFIRM_W - 36.0, 0.9);
    let height = lines.len() as f32 * 15.0 + 22.0 + 20.0 + 15.0 + 13.0 + 12.0;
    let x = (WIDTH - CONFIRM_W) / 2.0;
    let y = PANEL_Y + (PANEL_H - height) / 2.0;
    canvas.nine(Art::Dialog, x, y, CONFIRM_W, height, [22.0, 22.0, 22.0, 22.0], 0.5, white);
    for (index, line) in lines.iter().enumerate() {
        canvas.text_centered(x + CONFIRM_W / 2.0, y + 11.0 + index as f32 * 15.0, line, 0.9, TEXT_DARK);
    }
    // the bar: a sunken track, the game's red bar filled to how far it is
    let bar_y = y + 14.0 + lines.len() as f32 * 15.0;
    let (bar_x, bar_w) = (x + 34.0, CONFIRM_W - 68.0);
    canvas.fill(bar_x, bar_y, bar_w, ROW - 2.0, Color32::from_rgba_unmultiplied(70, 40, 20, 70));
    let fraction = progress.fraction();
    let filled = (bar_w * fraction).max(20.0);
    canvas.three(Art::Highlight, bar_x, bar_y - 1.0, filled, ROW, [10.0, 10.0], white);
    let done = progress.done.load(std::sync::atomic::Ordering::Relaxed);
    let total = progress.total.load(std::sync::atomic::Ordering::Relaxed);
    let gb = |bytes: u64| bytes as f64 / (1u64 << 30) as f64;
    let dots = ".".repeat(1 + (now * 2.0) as usize % 3);
    canvas.text_centered(x + CONFIRM_W / 2.0, bar_y + 1.0, &format!("{:.0}%", fraction * 100.0), 0.8, TEXT_CREAM);
    canvas.text_centered(x + CONFIRM_W / 2.0, bar_y + 21.0, &format!("{:.1} GB of {:.1} GB{dots}", gb(done), gb(total)), 0.72, TEXT_SOFT);
    canvas.text_centered(x + CONFIRM_W / 2.0, bar_y + 35.0, "It goes on with the menu closed.", 0.72, TEXT_SOFT);
}

/// the Yes/No dialog that confirms a restart, the game's checkered box with
/// its question and the two answers, the chosen one on the red bar with the
/// hand. yes is the answer chosen.
pub struct Confirm {
    pub yes: bool,
    /// codemods: asking to download the compiler (its size in MB) instead.
    pub download: Option<super::codemods::Ask>,
    /// qol: the same dialog asks before Exit to Desktop
    pub exit: bool,
    /// qol4: and before Return to Title
    pub title: bool,
}

pub enum Answer {
    Yes,
    No,
    /// zigdl: closed without an answer (the compiler choice, where No is
    /// the other compiler)
    Cancel,
}

/// zigdl: the dialog's two answers.
fn answers(confirm: &Confirm) -> [&'static str; 2] {
    confirm.download.map_or(["Yes", "No"], super::codemods::answers)
}

/// zigdl: what closing the dialog answers.
fn closed(confirm: &Confirm) -> Answer {
    if confirm.download.is_some_and(|ask| ask.setup) { Answer::Cancel } else { Answer::No }
}

const CONFIRM_W: f32 = 290.0;

fn confirm_text(confirm: &Confirm, pending: bool) -> String {
    if let Some(ask) = confirm.download {
        return super::codemods::download_text(ask);
    }
    if confirm.exit {
        // qol: the same dialog asks before Exit to Desktop
        return "Exit to the desktop? Anything you haven't saved will be lost.".to_owned();
    }
    if crate::haiku_voices::asking() {
        // jpvoice: and before the restart that changes the voices
        return "Restart the game with the voices as set? Anything you haven't saved will be lost.".to_owned();
    }
    if confirm.title {
        // qol4: and before Return to Title
        return "Return to the title screen? Anything you haven't saved will be lost.".to_owned();
    }
    if pending {
        "Restart the game with the mods as set? Anything you haven't saved will be lost.".to_owned()
    } else {
        "The game already runs with these mods. Restart it anyway? Anything you haven't saved will be lost.".to_owned()
    }
}

/// the dialog's box and the answers' rows, in units.
fn confirm_layout(canvas: &Canvas, confirm: &Confirm, pending: bool) -> (f32, f32, f32, Vec<String>, [f32; 2]) {
    let lines = canvas.kit.wrap_even(&confirm_text(confirm, pending), CONFIRM_W - 36.0, 0.9);
    let height = lines.len() as f32 * 15.0 + 22.0 + 2.0 * 19.0 + 10.0;
    let x = (WIDTH - CONFIRM_W) / 2.0;
    let y = PANEL_Y + (PANEL_H - height) / 2.0;
    let answers = y + 12.0 + lines.len() as f32 * 15.0 + 6.0;
    (x, y, height, lines, [answers, answers + 19.0])
}

/// moves through the dialog; an answer once it is given.
pub fn confirm_nav(confirm: &mut Confirm, canvas: &Canvas, input: &egui::InputState, nav: &Nav, pending: bool) -> Option<Answer> {
    if nav.up || nav.down || nav.left || nav.right {
        confirm.yes = !confirm.yes;
    }
    if nav.back || nav.close {
        return Some(closed(confirm)); // zigdl
    }
    if nav.confirm {
        return Some(if confirm.yes { Answer::Yes } else { Answer::No });
    }
    let (x, _, _, _, rows) = confirm_layout(canvas, confirm, pending);
    if let Some(pos) = input.pointer.hover_pos() {
        let (px, py) = canvas.unit_of(pos);
        for (index, top) in rows.iter().enumerate() {
            if (x + 30.0..x + CONFIRM_W - 30.0).contains(&px) && (*top..top + 19.0).contains(&py) {
                if input.pointer.delta() != egui::Vec2::ZERO {
                    confirm.yes = index == 0;
                }
                if input.pointer.primary_clicked() {
                    return Some(if index == 0 { Answer::Yes } else { Answer::No });
                }
            }
        }
    }
    if input.pointer.secondary_clicked() {
        return Some(closed(confirm)); // zigdl
    }
    None
}

/// draws the dialog over the menu.
pub fn draw_confirm(confirm: &Confirm, canvas: &Canvas, now: f64, pending: bool) {
    let white = Color32::WHITE;
    canvas.fill(0.0, 0.0, WIDTH, super::options::HEIGHT, Color32::from_black_alpha(90));
    let (x, y, height, lines, rows) = confirm_layout(canvas, confirm, pending);
    canvas.nine(Art::Dialog, x, y, CONFIRM_W, height, [22.0, 22.0, 22.0, 22.0], 0.5, white);
    for (index, line) in lines.iter().enumerate() {
        canvas.text_centered(x + CONFIRM_W / 2.0, y + 11.0 + index as f32 * 15.0, line, 0.9, TEXT_DARK);
    }
    let bob = ((now * 6.0).sin() * 1.5) as f32;
    // zigdl: the compiler choice's answers are longer: a wider bar
    let inset = if confirm.download.is_some_and(|ask| ask.setup) { 45.0 } else { 70.0 };
    for (index, (top, label)) in rows.iter().zip(answers(confirm)).enumerate() {
        let chosen = (index == 0) == confirm.yes;
        if chosen {
            canvas.three(Art::Highlight, x + inset, *top - 1.0, CONFIRM_W - 2.0 * inset, ROW + 1.0, [10.0, 10.0], white);
            canvas.sprite(Art::Hand, x + inset - 20.0 + bob, top + 2.5, white);
        }
        canvas.text_centered(x + CONFIRM_W / 2.0, top + 2.0, label, 0.95, if chosen { TEXT_CREAM } else { TEXT_DARK });
    }
}

impl App {
    /// reads the mods folder and the list for a game about to start: what
    /// it starts with, and the mods' texture packs.
    pub(crate) fn fates_mods_scan(&mut self) {
        let mut list = ModList::haiku(self.data_dir.as_deref());
        // hdpacks: FATES_MODS_UNPACK_WAIT=1 (tests) waits for the downloads
        // the list turned on to be unpacked, then reads the folder again
        if std::env::var("FATES_MODS_UNPACK_WAIT").is_ok_and(|v| v == "1") && crate::haiku_import::running().is_some() {
            while let Some(progress) = crate::haiku_import::running() {
                log::info!("mods: waiting for {} to be unpacked, {:.0}%", progress.name, progress.fraction() * 100.0);
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
            let _ = crate::haiku_import::take_finished();
            list = ModList::haiku(self.data_dir.as_deref());
        }
        list.booted = Some(list.active());
        self.glyphs.mod_textures = crate::haiku_builtin::with_texture_packs(list.texture_dirs()); // sunglasses: Haiku's own HD pictures under the packs
        self.fates.mods = list;
        self.fates.env.mods = ModsView::of(&self.fates.mods);
    }

    /// hdpacks: a download unpacked meanwhile: the folder is read again and
    /// the player told, the list now differing from the game's.
    pub(crate) fn fates_mods_poll(&mut self) {
        for progress in crate::haiku_import::take_finished() {
            let result = progress.result.lock().ok().and_then(|result| result.clone());
            match result {
                Some(Ok(_)) => {
                    self.menus.toast = Some((format!("{} is unpacked: Apply and Restart to use it", progress.name), std::time::Instant::now()));
                }
                Some(Err(error)) => {
                    self.menus.message = Some(format!("{} could not be unpacked: {error}. Its zips are as they were.", progress.name));
                }
                None => {}
            }
            self.fates_mods_rescan();
        }
    }

    /// reads the mods folder again while the game runs (the menu opened),
    /// keeping what the game started with.
    pub(crate) fn fates_mods_rescan(&mut self) {
        let booted = self.fates.mods.booted.take();
        self.fates.mods.scan();
        self.fates.mods.booted = booted;
        self.fates.env.mods = ModsView::of(&self.fates.mods);
    }

    /// for the headless restart test: turns on the mods with the ids
    /// listed (comma separated) and the others off, as the Mods page would.
    pub(crate) fn fates_mods_test_enable(&mut self, ids: &str) {
        self.fates_mods_rescan();
        let wanted: Vec<&str> = ids.split(',').map(str::trim).filter(|id| !id.is_empty()).collect();
        for index in 0..self.fates.mods.mods.len() {
            let on = wanted.iter().any(|id| id.eq_ignore_ascii_case(&self.fates.mods.mods[index].id));
            self.fates_mods_effect(ModsEffect::Toggle(index, on));
        }
        log::info!("mods: turned on {:?}, the list now differs from the running game's: {}", self.fates.mods.active(), self.fates.mods.pending());
    }

    /// what a change on the Mods page does.
    pub(crate) fn fates_mods_effect(&mut self, effect: ModsEffect) {
        match effect {
            ModsEffect::Toggle(index, on) => {
                self.fates.mods.set_enabled(index, on);
            }
            ModsEffect::Move(index, by) => {
                self.fates.mods.shift(index, by);
            }
            ModsEffect::OpenFolder => {
                let folder = self.fates.mods.folder.clone();
                if let Err(error) = std::fs::create_dir_all(&folder) {
                    log::warn!("mods: could not make {}, {error}", folder.display());
                }
                crate::show_folder(&folder);
            }
            ModsEffect::Restart => self.fates_mods_restart(),
            ModsEffect::Download(yes) => self.fates_code_mods_download(yes), // codemods
            ModsEffect::UseSetupCompiler => self.fates_code_mods_use_setup(), // zigdl
            ModsEffect::Compiler(choice) => self.fates_code_mods_choose(choice), // zigdl
            ModsEffect::CancelJob => self.fates_code_mods_cancel(),          // codemods
        }
        self.fates_code_view(); // codemods: the list and the compile
    }

    /// restarts the game with the mods as they are now set.
    ///
    /// qol3: in a window this is a real restart (fates_relaunch): the
    /// settings and mods.toml are saved, a fresh Haiku.exe starts with the
    /// same arguments and working folder, and this one closes. Everything a
    /// mod changes is then loaded from nothing: the RomFS layers, texture
    /// packs, code-mod DLLs and patched code. The in-process reboot this
    /// used to be (a new System in the same process) kept what lives for the
    /// whole process: a texture pack stayed drawn after its mod was turned
    /// off (Glyphs::apply leaves the renderer's pack as it is when a game
    /// starts with none, and `pack::set_current` is process-wide), and a
    /// code mod's DLL cannot be unloaded at all.
    ///
    /// Headless runs (the FATES_MODS_RESTART_AT test) still reboot in
    /// process, now dropping the old pack first. HAIKU_RESTART=relaunch or
    /// in-process picks either way anywhere.
    pub(crate) fn fates_mods_restart(&mut self) {
        // codemods: the code mods are compiled first (or the player asked
        // to download the compiler); the restart follows
        if self.options.headless.is_none() && !self.fates_code_mods_ready() {
            return;
        }
        self.fates_mods_restart_now();
    }

    /// modfix: the restart itself, once the code mods are ready (or left
    /// out after failing).
    pub(crate) fn fates_mods_restart_now(&mut self) {
        self.fates.options.hide();
        self.fates.swallow = true;
        let relaunch = match std::env::var("HAIKU_RESTART").as_deref() {
            Ok("relaunch") => true,
            Ok("in-process") => false,
            _ => self.options.headless.is_none(),
        };
        if relaunch {
            self.fates_relaunch();
            return;
        }
        let Some(path) = self.game.as_ref().map(|game| game.path.clone()) else { return };
        log::info!("mods: restarting the game with {} mods on", self.fates.mods.active().len());
        let started = std::time::Instant::now();
        // qol3: no pack is left drawn from the old list; the new game's
        // glyphs set the one its mods make, if any
        if let Some(game) = &mut self.game {
            game.system.gpu.set_texture_pack(None);
        }
        // the native renderer stays installed across the restart: it leaves
        // out the SDK's set-up before the new boot's first frame as it did
        // at the first one, rather than handing it to the (absent) emulated GPU
        zakuro_core::native_gfx::lease_boot(120);
        // and the overrides' tails waiting in the old boot's threads go,
        // as when a state is loaded
        zakuro_core::recompiled::forget_tails(0);
        match self.play(&path) {
            Ok(()) => {
                log::info!("mods: the game restarted in {} ms", started.elapsed().as_millis());
                let on = self.fates.mods.active().len();
                let text = match on {
                    0 => "Restarted with no mods".to_owned(),
                    1 => "Restarted with 1 mod".to_owned(),
                    _ => format!("Restarted with {on} mods"),
                };
                self.menus.toast = Some((text, std::time::Instant::now()));
            }
            Err(error) => self.menus.message = Some(error),
        }
    }

    /// starts Haiku.exe again with the same arguments, in the same working
    /// folder, and closes this one. qol3: the new one waits for this one to
    /// be gone before it opens its window (HAIKU_RESTART_WAIT, its process
    /// id; source/src/console.rs), so the two never hold the window, the
    /// sound or the saves at once. A test script (FATES_UI_SCRIPT,
    /// FATES_MODS_RESTART_*) is not handed on, or it would run again.
    fn fates_relaunch(&mut self) {
        let Ok(exe) = std::env::current_exe() else { return };
        let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
        // the settings, mods.toml and saves are on disk; the new process
        // starts clean from them
        self.settings.save();
        self.fates.mods.save();
        let mut command = std::process::Command::new(&exe);
        command.args(&args).env("HAIKU_RESTART_WAIT", std::process::id().to_string());
        if let Ok(folder) = std::env::current_dir() {
            command.current_dir(folder);
        }
        for test in ["FATES_UI_SCRIPT", "FATES_MODS_RESTART_AT", "FATES_MODS_RESTART_ENABLE", "FATES_BOTTOM_SCRIPT", "HAIKU_RESTART_CHILD_ENV"] {
            command.env_remove(test);
        }
        // test: HAIKU_RESTART_CHILD_ENV="NAME=value;NAME=value" is the new
        // process's own (its frame dumps, its capture script)
        if let Ok(child_env) = std::env::var("HAIKU_RESTART_CHILD_ENV") {
            for pair in child_env.split(';').filter_map(|pair| pair.split_once('=')) {
                command.env(pair.0.trim(), pair.1.trim());
            }
        }
        match command.spawn() {
            Ok(child) => {
                log::info!(
                    "mods: restarting: started {} again (process {}) with {} mods on, closing this one (process {})",
                    exe.display(),
                    child.id(),
                    self.fates.mods.active().len(),
                    std::process::id()
                );
                self.stop = true;
                log::logger().flush();
                std::process::exit(0);
            }
            Err(error) => self.menus.message = Some(format!("Haiku could not be started again, {error}. Close it and start it again to use the mods.")),
        }
    }
}
