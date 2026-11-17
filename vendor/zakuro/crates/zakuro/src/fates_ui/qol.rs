//! qol: the boon/bane panel beside the avatar's Name / Birthday / Boon /
//! Bane / Talent menu, drawn in the game's own style: the parchment window
//! the menu itself sits in, the dark red label pills of its rows, the
//! cream value pills, and the game's font. It shows, for the boon and bane
//! chosen, how each of the eight stats changes: its base, its growth rate
//! and its cap, read from the game's Personality table (crate::qol).

use egui::Color32;

use super::kit::{Art, Canvas, TEXT_CREAM, TEXT_DARK, TEXT_MUTED};
use crate::qol::{CharaPanel, STAT_NAMES};

/// a stat going up, in the blue the game's own stat screens use for a
/// raised value, and going down in its red.
const UP: Color32 = Color32::from_rgb(28, 84, 196);
const DOWN: Color32 = Color32::from_rgb(196, 30, 24);

/// the avatar menu's window is the parchment in orange.
const PARCHMENT: Color32 = Color32::from_rgb(255, 178, 96);

/// the panel's size in 3DS pixels.
pub const WIDTH: f32 = 164.0;
pub const HEIGHT: f32 = 154.0;

fn signed(value: i8, percent: bool) -> String {
    let sign = if value > 0 { "+" } else { "" };
    if percent { format!("{sign}{value}%") } else { format!("{sign}{value}") }
}

/// where the panel goes, in 3DS pixels from the top screen's 400 x 240
/// picture's top left: beside the menu when a wide top screen leaves room
/// on its right, otherwise over the lower left of the avatar's portrait,
/// clear of the menu (which starts at x 183).
pub fn place(extra_right: f32) -> (f32, f32) {
    if extra_right >= WIDTH + 8.0 {
        (400.0 + 4.0, 240.0 - HEIGHT - 6.0)
    } else {
        (6.0, 240.0 - HEIGHT - 4.0)
    }
}

/// draws the panel with its top left at x, y (units).
pub fn draw(canvas: &Canvas, x: f32, y: f32, panel: &CharaPanel) {
    let (boon_name, bane_name) = (panel.boon_name.as_str(), panel.bane_name.as_str());
    let white = Color32::WHITE;
    // the menus' parchment, warmed to the avatar menu's own orange window
    canvas.nine(Art::Panel, x, y, WIDTH, HEIGHT, [60.0, 40.0, 60.0, 40.0], 0.42, PARCHMENT);
    // the boon and the bane, as the menu's own rows show them: the dark
    // label pill, the cream value pill
    let row = |row_y: f32, label: &str, value: &str, chosen: bool| {
        canvas.nine(Art::PillLabel, x + 10.0, row_y, 40.0, 14.0, [8.0, 8.0, 8.0, 8.0], 0.5, white);
        canvas.text_centered(x + 30.0, row_y + 0.5, label, 0.66, TEXT_CREAM);
        let pill = if chosen { Art::PillRed } else { Art::PillCream };
        canvas.nine(pill, x + 52.0, row_y, WIDTH - 62.0, 14.0, [8.0, 8.0, 8.0, 8.0], 0.5, white);
        canvas.text_centered(x + 52.0 + (WIDTH - 62.0) / 2.0, row_y + 0.5, value, 0.66, if chosen { TEXT_CREAM } else { TEXT_DARK });
    };
    row(y + 9.0, "Boon", boon_name, panel.selected == 3);
    row(y + 25.0, "Bane", bane_name, panel.selected == 4);
    // the table: a stat's base, growth and cap changes
    let columns = [x + 66.0, x + 110.0, x + 150.0];
    let head = y + 42.0;
    for (column, name) in columns.iter().zip(["Base", "Growth", "Max"]) {
        canvas.text_right(*column, head, name, 0.56, TEXT_DARK);
    }
    let boon_stat = panel.boon.checked_sub(1).map(usize::from);
    let bane_stat = panel.bane.checked_sub(1).map(usize::from);
    for (i, stat) in STAT_NAMES.iter().enumerate() {
        let line = head + 11.0 + i as f32 * 11.5;
        let colour = if Some(i) == boon_stat {
            UP
        } else if Some(i) == bane_stat {
            DOWN
        } else {
            TEXT_DARK
        };
        canvas.text(x + 14.0, line, stat, 0.62, colour);
        let values = [(panel.total.base[i], false), (panel.total.growth[i], true), (panel.total.cap[i], false)];
        for (column, (value, percent)) in columns.iter().zip(values) {
            let (text, colour) = match value {
                // the game's own "none", its long dash
                0 => ("\u{2014}".to_owned(), TEXT_MUTED),
                v if v > 0 => (signed(v, percent), UP),
                v => (signed(v, percent), DOWN),
            };
            canvas.text_right(*column, line, &text, 0.62, colour);
        }
    }
}
