//! pulse: the Cheats tab's Save States section (crate::states does the
//! work), a group of its own beside Units, Battle, Items & Resources and
//! Story & Supports, drawn by options.rs like the others: the Save States
//! switch, the slot the quick keys use, and the eight slots. The page shows
//! four rows at a time and, under them, the chosen slot's card: its
//! thumbnail (at its own shape, never stretched), when it was saved and
//! where. A slot opens the game's dialog with Save, Load and Delete;
//! overwriting, loading and deleting ask Yes or No first.
//!
//! statecheck: a slot made with another file layer (other mods, voices,
//! Haiku's own files or language) gets a "!" after its name, as the Mods
//! page marks a mod with a warning, and its card says what changed in red;
//! choosing Load on it shows the whole reason in the game's dialog with OK.

use std::collections::HashMap;
use std::sync::Mutex;

use egui::Color32;

use super::kit::{Art, Canvas, TEXT_CREAM, TEXT_DARK, TEXT_MUTED};
use super::options::{Effect, Item, PANEL_H, PANEL_W, PANEL_X, PANEL_Y, ROW, ROWS_TOP, WIDTH};
use super::Nav;
use crate::settings::Settings;
use crate::states::{self as host, Op, SLOTS};

/// the group's index among the Cheats tab's groups
pub const GROUP: u8 = 4;
/// the rows the page shows at once (the card is under them)
pub const VISIBLE: usize = 4;

pub mod id {
    pub const SWITCH: u16 = 2000;
    pub const HOTKEY: u16 = 2001;
    pub const SLOT: u16 = 2010; // 2010..2017
}

pub fn is_ours(cid: u16) -> bool {
    (id::SWITCH..id::SLOT + SLOTS as u16).contains(&cid)
}

pub fn items() -> Vec<Item> {
    let mut list = vec![Item::Cheat(id::SWITCH), Item::Cheat(id::HOTKEY)];
    list.extend((0..SLOTS as u16).map(|i| Item::Cheat(id::SLOT + i)));
    list
}

#[derive(Default)]
struct Ui {
    /// the slots as last read, and the game's identity to check them against
    slots: Vec<Option<host::Slot>>,
    identity: (String, String),
    /// a slot's dialog: the slot and the choice
    menu: Option<(usize, usize)>,
    /// a question: its text, Yes chosen, and what Yes does
    confirm: Option<(String, bool, Op)>,
    /// statecheck: a message with OK alone (why a slot can't be loaded)
    info: Option<String>,
    /// the thumbnails' textures, by slot, with the file's time they are of
    textures: HashMap<usize, (Option<std::time::SystemTime>, egui::TextureHandle)>,
}

static UI: Mutex<Option<Ui>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Ui) -> R) -> R {
    let mut guard = UI.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(Ui::default))
}

/// the game being played, for checking the slots (the app sets it).
pub fn set_identity(identity: (String, String)) {
    with(|ui| ui.identity = identity);
}

/// statecheck: a message in the page's dialog, closed with OK.
pub fn show_info(text: String) {
    with(|ui| {
        ui.menu = None;
        ui.confirm = None;
        ui.info = Some(text);
    });
}

fn refresh() {
    let identity = with(|ui| ui.identity.clone());
    let slots = host::slots(&identity.0, &identity.1);
    with(|ui| ui.slots = slots);
}

fn slot(index: usize) -> Option<host::Slot> {
    with(|ui| ui.slots.get(index).cloned().flatten())
}

/// a slot's short line, for its pill.
fn summary(slot: &Option<host::Slot>) -> String {
    match slot {
        None => "Empty".to_owned(),
        Some(slot) => match &slot.meta {
            // statecheck: one made with another layer still says where
            // (the "!" after its name says it can't be loaded now)
            Some(meta) if slot.problem.is_none() || slot.mismatch.is_some() => {
                let place = short_place(&meta.place);
                match meta.turn {
                    Some(turn) => format!("{place} \u{b7} Turn {turn}"),
                    None => place,
                }
            }
            _ => "Can't load".to_owned(),
        },
    }
}

fn short_place(place: &str) -> String {
    let place = place.replace("Chapter ", "Ch. ").replace("Paralogue ", "Par. ");
    match place.split_once(':') {
        Some((head, _)) if head.len() <= 10 => head.to_owned(),
        _ => place,
    }
}

pub fn row(cid: u16, settings: &Settings) -> (String, Option<String>, String) {
    let s = |t: &str| t.to_owned();
    let on = settings.pc.save_states;
    if cid == id::SWITCH {
        refresh();
        return (
            s("Save States"),
            Some(s(if on { "On" } else { "Off" })),
            s("Save and load the whole game at any moment, in eight slots of this install's own. The game's own saves are never touched."),
        );
    }
    if cid == id::HOTKEY {
        let slot = settings.pc.save_state_slot.min(SLOTS as u32 - 1);
        let keys = {
            let mut keys = settings.keys.clone();
            let names: Vec<String> = keys.all_mut().into_iter().filter(|(n, _)| n.starts_with("Quick")).map(|(_, b)| b.label()).collect();
            names.join(" and ")
        };
        let keys = if keys.trim().is_empty() { "the quick save and quick load keys".to_owned() } else { keys };
        return (s("Hotkey Slot"), Some(format!("Slot {}", slot + 1)), format!("The slot {keys} save to and load from. Rebind them in Controls."));
    }
    let index = (cid - id::SLOT) as usize;
    let slot = slot(index);
    let help = match &slot {
        None => s("Empty. Press to save the game here."),
        Some(slot) => match (&slot.meta, &slot.problem) {
            // statecheck: made with another layer: the short reason here,
            // the whole one when it is chosen
            (_, Some(_)) if slot.mismatch.is_some() => format!("Can't load with this setup: {}. Press to see why.", slot.mismatch.as_deref().unwrap_or_default()),
            (_, Some(problem)) => problem.clone(),
            (Some(meta), None) => format!("Saved {}: {}{}. Press to save, load or delete.", host::when(meta.saved_unix), meta.place, meta.turn.map_or(String::new(), |t| format!(", turn {t}"))),
            (None, None) => s("Press to save, load or delete."),
        },
    };
    (format!("Slot {}", index + 1), Some(summary(&slot)), help)
}

pub fn changes(cid: u16) -> bool {
    cid == id::SWITCH || cid == id::HOTKEY
}

pub fn adjust(cid: u16, dir: i32, settings: &mut Settings) -> Option<Effect> {
    let d = if dir == 0 { 1 } else { dir };
    match cid {
        id::SWITCH => settings.pc.save_states = !settings.pc.save_states,
        id::HOTKEY => settings.pc.save_state_slot = (settings.pc.save_state_slot as i32 + d).rem_euclid(SLOTS as i32) as u32,
        _ => return None,
    }
    Some(Effect::Apply)
}

/// A on a slot row: its dialog.
pub fn press(cid: u16, settings: &Settings) {
    if !(id::SLOT..id::SLOT + SLOTS as u16).contains(&cid) {
        return;
    }
    if !settings.pc.save_states {
        crate::states::push_notice("Turn on Save States first.");
        return;
    }
    if zakuro_core::netplay::battle() {
        crate::states::push_notice("Save states can't be used in an online battle.");
        return;
    }
    let index = (cid - id::SLOT) as usize;
    with(|ui| ui.menu = Some((index, 0)));
}

const CHOICES: [&str; 4] = ["Save Here", "Load", "Delete", "Cancel"];

pub fn confirming() -> bool {
    with(|ui| ui.menu.is_some() || ui.confirm.is_some() || ui.info.is_some())
}

/// statecheck: the "!" after a slot's name when it was made with another
/// file layer, drawn as the Mods page draws a mod's warning.
pub fn draw_row_extras(canvas: &Canvas, item: Item, x: f32, y: f32, label_width: f32, chosen: bool) {
    let Item::Cheat(cid) = item else { return };
    if !(id::SLOT..id::SLOT + SLOTS as u16).contains(&cid) {
        return;
    }
    let mismatch = slot((cid - id::SLOT) as usize).is_some_and(|slot| slot.mismatch.is_some());
    if mismatch {
        let colour = if chosen { super::kit::TEXT_GOLD } else { TEXT_WARN };
        canvas.text(x + label_width + 6.0, y + 2.0, "!", 0.95, colour);
    }
}

/// the Mods page's warning red.
const TEXT_WARN: Color32 = Color32::from_rgb(176, 24, 8);

fn dialog_box(lines: usize, rows: usize) -> (f32, f32, f32, f32) {
    let w = 300.0;
    let h = lines as f32 * 15.0 + 22.0 + rows as f32 * 19.0 + 10.0;
    ((WIDTH - w) / 2.0, PANEL_Y + (PANEL_H - h) / 2.0, w, h)
}

fn menu_lines(index: usize) -> Vec<String> {
    let slot = slot(index);
    vec![format!("Slot {}: {}", index + 1, summary(&slot))]
}

/// the dialog or question takes the input while it is up.
pub fn confirm_nav(canvas: &Canvas, input: &egui::InputState, nav: &Nav) {
    let pointer = input.pointer.hover_pos().map(|p| canvas.unit_of(p));
    let clicked = input.pointer.primary_clicked();
    // statecheck: a message: OK, back or a click anywhere closes it
    if with(|ui| ui.info.is_some()) {
        if nav.confirm || nav.back || nav.close || clicked || input.pointer.secondary_clicked() {
            with(|ui| ui.info = None);
        }
        return;
    }
    if let Some((text, yes, op)) = with(|ui| ui.confirm.clone()) {
        let mut yes = yes;
        let mut answer = None;
        if nav.up || nav.down || nav.left || nav.right {
            yes = !yes;
        }
        if nav.back || nav.close || input.pointer.secondary_clicked() {
            answer = Some(false);
        } else if nav.confirm {
            answer = Some(yes);
        } else if let Some((px, py)) = pointer {
            let lines = canvas.kit.wrap_even(&text, 264.0, 0.9);
            let (x, y, w, _) = dialog_box(lines.len(), 2);
            let top = y + 12.0 + lines.len() as f32 * 15.0 + 6.0;
            for index in 0..2 {
                let ry = top + index as f32 * 19.0;
                if (x + 30.0..x + w - 30.0).contains(&px) && (ry..ry + 19.0).contains(&py) {
                    if input.pointer.delta() != egui::Vec2::ZERO {
                        yes = index == 0;
                    }
                    if clicked {
                        answer = Some(index == 0);
                    }
                }
            }
        }
        with(|ui| {
            if let Some(c) = &mut ui.confirm {
                c.1 = yes;
            }
            if answer.is_some() {
                ui.confirm = None;
            }
        });
        if answer == Some(true) {
            host::request(op);
        }
        return;
    }
    let Some((index, choice)) = with(|ui| ui.menu) else { return };
    let mut choice = choice;
    let mut picked = None;
    if nav.up {
        choice = (choice + CHOICES.len() - 1) % CHOICES.len();
    }
    if nav.down {
        choice = (choice + 1) % CHOICES.len();
    }
    if nav.back || nav.close || input.pointer.secondary_clicked() {
        picked = Some(3);
    } else if nav.confirm {
        picked = Some(choice);
    } else if let Some((px, py)) = pointer {
        let (x, y, w, _) = dialog_box(1, CHOICES.len());
        let top = y + 12.0 + 15.0 + 6.0;
        for row in 0..CHOICES.len() {
            let ry = top + row as f32 * 19.0;
            if (x + 30.0..x + w - 30.0).contains(&px) && (ry..ry + 19.0).contains(&py) {
                if input.pointer.delta() != egui::Vec2::ZERO {
                    choice = row;
                }
                if clicked {
                    picked = Some(row);
                }
            }
        }
    }
    with(|ui| ui.menu = Some((index, choice)));
    let Some(picked) = picked else { return };
    let slot = slot(index);
    let filled = slot.is_some();
    let loadable = slot.as_ref().is_some_and(|s| s.problem.is_none() && s.meta.is_some());
    let question = match picked {
        0 if filled => Some((format!("Save over slot {}? What it holds now will be lost.", index + 1), Op::Save(index))),
        0 => {
            host::request(Op::Save(index));
            None
        }
        1 if loadable => Some((format!("Load slot {}? Anything since your last save will be lost.", index + 1), Op::Load(index))),
        1 => {
            // statecheck: why it can't be loaded, whole, in the dialog
            match slot.and_then(|s| s.problem) {
                Some(why) => with(|ui| ui.info = Some(why)),
                None => crate::states::push_notice(&format!("Slot {} is empty.", index + 1)),
            }
            None
        }
        2 if filled => Some((format!("Delete slot {}? This can't be undone.", index + 1), Op::Delete(index))),
        2 => None,
        _ => None,
    };
    with(|ui| {
        ui.menu = None;
        if let Some((text, op)) = question {
            ui.confirm = Some((text, false, op));
        }
    });
}

pub fn draw_confirm(canvas: &Canvas, now: f64) {
    let white = Color32::WHITE;
    let bob = ((now * 6.0).sin() * 1.5) as f32;
    let (confirm, menu, info) = with(|ui| (ui.confirm.clone(), ui.menu, ui.info.clone()));
    if confirm.is_none() && menu.is_none() && info.is_none() {
        return;
    }
    canvas.fill(0.0, 0.0, WIDTH, super::options::HEIGHT, Color32::from_black_alpha(90));
    let (lines, rows, chosen): (Vec<String>, Vec<&str>, usize) = match (&confirm, menu) {
        // statecheck: a message with OK
        _ if info.is_some() => (canvas.kit.wrap_even(info.as_deref().unwrap_or_default(), 264.0, 0.9), vec!["OK"], 0),
        (Some((text, yes, _)), _) => (canvas.kit.wrap_even(text, 264.0, 0.9), vec!["Yes", "No"], if *yes { 0 } else { 1 }),
        (None, Some((index, choice))) => (menu_lines(index), CHOICES.to_vec(), choice),
        _ => return,
    };
    let (x, y, w, h) = dialog_box(lines.len(), rows.len());
    canvas.nine(Art::Dialog, x, y, w, h, [22.0, 22.0, 22.0, 22.0], 0.5, white);
    for (i, line) in lines.iter().enumerate() {
        canvas.text_centered(x + w / 2.0, y + 11.0 + i as f32 * 15.0, line, 0.9, TEXT_DARK);
    }
    let top = y + 12.0 + lines.len() as f32 * 15.0 + 6.0;
    for (index, label) in rows.iter().enumerate() {
        let ry = top + index as f32 * 19.0;
        if index == chosen {
            canvas.three(Art::Highlight, x + 70.0, ry - 1.0, w - 140.0, ROW + 1.0, [10.0, 10.0], white);
            canvas.sprite(Art::Hand, x + 50.0 + bob, ry + 2.5, white);
        }
        canvas.text_centered(x + w / 2.0, ry + 2.0, label, 0.95, if index == chosen { TEXT_CREAM } else { TEXT_DARK });
    }
}

/// the chosen slot's card under the rows: its thumbnail and its lines.
pub fn draw_card(canvas: &Canvas, selected: Item) {
    let white = Color32::WHITE;
    let x = PANEL_X + 30.0;
    let y = ROWS_TOP + VISIBLE as f32 * ROW + 6.0;
    let w = PANEL_W - 60.0;
    let h = PANEL_Y + PANEL_H - 16.0 - y;
    canvas.nine(Art::PillCream, x, y, w, h, [9.0, 9.0, 9.0, 9.0], 0.62, white);
    let Item::Cheat(cid) = selected else { return };
    if !(id::SLOT..id::SLOT + SLOTS as u16).contains(&cid) {
        canvas.text(x + 12.0, y + 8.0, "Choose a slot to see what it holds.", 0.85, TEXT_MUTED);
        return;
    }
    let index = (cid - id::SLOT) as usize;
    let slot = slot(index);
    // the thumbnail, at its own shape in a 5:3 box
    let bw = (h - 10.0) * 5.0 / 3.0;
    let (bx, by) = (x + 6.0, y + 5.0);
    canvas.fill(bx, by, bw, h - 10.0, Color32::from_rgb(40, 30, 26));
    if let Some(slot) = &slot {
        if let Some((rgba, tw, th)) = &slot.thumb {
            let texture = with(|ui| {
                let fresh = ui.textures.get(&index).is_some_and(|(at, _)| *at == slot.modified);
                if !fresh {
                    let image = egui::ColorImage::from_rgba_unmultiplied([*tw as usize, *th as usize], rgba);
                    let handle = canvas.painter.ctx().load_texture(format!("pulse-state-{index}"), image, egui::TextureOptions::LINEAR);
                    ui.textures.insert(index, (slot.modified, handle));
                }
                ui.textures.get(&index).map(|(_, t)| t.id())
            });
            if let Some(texture) = texture {
                let aspect = *tw as f32 / (*th).max(1) as f32;
                let box_h = h - 10.0;
                let (iw, ih) = if aspect > bw / box_h { (bw, bw / aspect) } else { (box_h * aspect, box_h) };
                let rect = canvas.rect(bx + (bw - iw) / 2.0, by + (box_h - ih) / 2.0, iw, ih);
                canvas.painter.image(texture, rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), white);
            }
        }
    }
    let tx = bx + bw + 10.0;
    let room = x + w - 10.0 - tx;
    let line = |i: f32, text: &str, colour: Color32| {
        let mut text = text.to_owned();
        while canvas.kit.measure(&text, 0.85) > room && text.pop().is_some() {}
        canvas.text(tx, y + 6.0 + i * 15.0, &text, 0.85, colour);
    };
    line(0.0, &format!("Slot {}", index + 1), TEXT_DARK);
    match &slot {
        None => line(1.0, "Empty", TEXT_MUTED),
        Some(slot) => {
            // statecheck: the card holds four lines; the last says why it
            // can't be loaded (red, made to fit), or that it was made
            // before Haiku checked (quietly); the turn then joins the place
            let status: Option<(String, Color32)> = match (&slot.mismatch, &slot.problem) {
                (Some(short), _) => Some((short.clone(), Color32::from_rgb(150, 40, 30))),
                (None, Some(problem)) => Some((problem.clone(), Color32::from_rgb(150, 40, 30))),
                (None, None) if slot.unchecked => Some(("Made before Haiku checked mods".to_owned(), TEXT_MUTED)),
                _ => None,
            };
            if let Some(meta) = &slot.meta {
                line(1.0, &host::when(meta.saved_unix), TEXT_DARK);
                match (meta.turn, status.is_some()) {
                    (Some(turn), true) => line(2.0, &format!("{}, Turn {turn}", meta.place), TEXT_DARK),
                    (Some(turn), false) => {
                        line(2.0, &meta.place, TEXT_DARK);
                        line(3.0, &format!("Turn {turn}"), TEXT_DARK);
                    }
                    (None, _) => line(2.0, &meta.place, TEXT_DARK),
                }
            }
            if let Some((text, colour)) = status {
                let mut scale = 0.85;
                while canvas.kit.measure(&text, scale) > room && scale > 0.62 {
                    scale -= 0.02;
                }
                let mut text = text;
                if canvas.kit.measure(&text, scale) > room {
                    while canvas.kit.measure(&format!("{text}..."), scale) > room && text.pop().is_some() {}
                    text = format!("{}...", text.trim_end());
                }
                canvas.text(tx, y + 6.0 + 3.0 * 15.0 + (0.85 - scale) * 6.0, &text, scale, colour);
            }
        }
    }
}

/// statecheck, snapshots: no dialog left from a scene before.
pub fn snapshot_reset() {
    with(|ui| {
        ui.menu = None;
        ui.confirm = None;
        ui.info = None;
    });
}

/// statecheck, snapshots: why a slot can't be loaded, in the page's dialog.
#[allow(dead_code)]
pub fn snapshot_info(index: usize) {
    refresh();
    if let Some(problem) = slot(index).and_then(|slot| slot.problem) {
        show_info(problem);
    }
}

/// snapshots: a slot's dialog up, and none after.
#[allow(dead_code)]
pub fn snapshot_menu(slot: usize) {
    with(|ui| {
        ui.menu = Some((slot, 1));
        ui.confirm = None;
    });
}
