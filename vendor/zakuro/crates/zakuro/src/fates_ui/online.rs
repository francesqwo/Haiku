//! online: the host's code over the game's wait for a player, in the
//! game's checkered message box: "Your battle code", the code in large
//! letters, who to send it to, and Copy (a click on it, Ctrl+C, or the
//! controller's X). A line under it says when the router didn't open the
//! port, so a player on another network may not get in.
//!
//! online2: the host's rule for the battle, "Characters modified with
//! cheats: Allow / Don't allow", with Change (a click on the line or the
//! button, Tab, or the controller's Y). It is sent to the player who joins
//! and holds for the session; the panel goes once they are in.

use egui::Color32;

use super::kit::{self, Art, Canvas, TEXT_CREAM, TEXT_DARK, TEXT_MUTED};
use super::{draw_key, draw_prompt, Nav, Prompt, Prompts};
use crate::controls::GlyphSet;
use crate::online_ui::HostPanel;

const WIDTH: f32 = 320.0;

/// the largest scale up to `most` at which text fits in width.
fn fit(canvas: &Canvas, text: &str, most: f32, width: f32) -> f32 {
    let mut scale = most;
    while scale > 0.55 && canvas.kit.measure(text, scale) > width {
        scale -= 0.05;
    }
    scale
}

/// what the player asked of the panel this frame.
#[derive(Default, Clone, Copy, Debug)]
pub struct Asked {
    /// Copy: the code to the clipboard.
    pub copy: bool,
    /// online2: Change: the rule for characters modified with cheats.
    pub toggle: bool,
}

/// online2: the rule's line, in the game's languages' English.
pub fn rule_text(allow: bool) -> String {
    format!("Characters modified with cheats: {}", if allow { "Allow" } else { "Don't allow" })
}

/// draws the panel at the top of the window, and says what was asked.
pub fn draw(canvas: &Canvas, input: &egui::InputState, nav: &Nav, prompts: &Prompts, panel: &HostPanel, screen: egui::Rect) -> Asked {
    let unit = canvas.unit;
    let canvas = Canvas { origin: screen.min, ..canvas.clone() };
    let inner = WIDTH - 32.0;
    let note = "Your router didn't open a port, so some players may not get in.";
    let height = 12.0 + 15.0 + 24.0 + kit::LINE + if panel.mapped { 0.0 } else { 13.0 } + 15.0 + 24.0;
    let x = (screen.width() / unit - WIDTH) / 2.0;
    let y = 8.0;
    canvas.nine(Art::Dialog, x, y, WIDTH, height, [22.0, 22.0, 22.0, 22.0], 0.5, Color32::WHITE);
    let mut pen = y + 11.0;
    canvas.text_centered(x + WIDTH / 2.0, pen, "Your battle code", 0.9, TEXT_MUTED);
    pen += 15.0;
    // the code, as large as the box lets it be
    let scale = fit(&canvas, &panel.code, 1.5, inner);
    canvas.text_centered(x + WIDTH / 2.0, pen + (1.5 - scale) * 6.0, &panel.code, scale, TEXT_DARK);
    pen += 24.0;
    let send = "Send it to the player you're inviting.";
    canvas.text_centered(x + WIDTH / 2.0, pen, send, fit(&canvas, send, 0.9, inner), TEXT_DARK);
    pen += kit::LINE;
    if !panel.mapped {
        canvas.text_centered(x + WIDTH / 2.0, pen, note, fit(&canvas, note, 0.75, inner), TEXT_MUTED);
        pen += 13.0;
    }
    // online2: the rule, a line the player can click
    let rule = rule_text(panel.allow_cheats);
    let rule_scale = fit(&canvas, &rule, 0.85, inner);
    let rule_width = canvas.kit.measure(&rule, rule_scale);
    canvas.text_centered(x + WIDTH / 2.0, pen + 1.0, &rule, rule_scale, TEXT_DARK);
    let over_rule = input.pointer.hover_pos().is_some_and(|p| {
        let (px, py) = canvas.unit_of(p);
        (x + (WIDTH - rule_width) / 2.0..x + (WIDTH + rule_width) / 2.0).contains(&px) && (pen..pen + 13.0).contains(&py)
    });

    // Copy, in a pill at the box's lower right, inside it
    let label = if panel.copied { "Copied" } else { "Copy" };
    let keyboard = prompts.set == GlyphSet::Keyboard;
    let glyph = if keyboard { canvas.kit.measure("Ctrl+C", 0.62) + 8.0 } else { 13.0 };
    let width = glyph + 4.0 + canvas.kit.measure(label, 0.85) + 12.0;
    let (bx, by) = (x + WIDTH - width - 14.0, y + height - 24.0);
    let over = input.pointer.hover_pos().is_some_and(|p| {
        let (px, py) = canvas.unit_of(p);
        (bx..bx + width).contains(&px) && (by..by + 17.0).contains(&py)
    });
    canvas.nine(if over { Art::PillRed } else { Art::PillPrompt }, bx, by, width, 17.0, [7.0, 7.0, 7.0, 7.0], 1.0, Color32::WHITE);
    let used = if keyboard { draw_key(&canvas, bx + 6.0, by + 0.5, "Ctrl+C") } else { draw_prompt(&canvas, prompts, bx + 6.0, by + 0.5, Prompt::Extra) };
    canvas.text(bx + 6.0 + used + 4.0, by + 1.5, label, 0.85, TEXT_CREAM);

    // online2: Change, in a pill at the box's lower left
    let label = "Change";
    let glyph = if keyboard { canvas.kit.measure("Tab", 0.62) + 8.0 } else { 13.0 };
    let width = glyph + 4.0 + canvas.kit.measure(label, 0.85) + 12.0;
    let (cx, cy) = (x + 14.0, y + height - 24.0);
    let over_change = input.pointer.hover_pos().is_some_and(|p| {
        let (px, py) = canvas.unit_of(p);
        (cx..cx + width).contains(&px) && (cy..cy + 17.0).contains(&py)
    });
    canvas.nine(if over_change || over_rule { Art::PillRed } else { Art::PillPrompt }, cx, cy, width, 17.0, [7.0, 7.0, 7.0, 7.0], 1.0, Color32::WHITE);
    let used = if keyboard { draw_key(&canvas, cx + 6.0, cy + 0.5, "Tab") } else { draw_prompt(&canvas, prompts, cx + 6.0, cy + 0.5, Prompt::Extra2) };
    canvas.text(cx + 6.0 + used + 4.0, cy + 1.5, label, 0.85, TEXT_CREAM);

    let ctrl_c = input.events.iter().any(|event| matches!(event, egui::Event::Copy)) || (input.modifiers.command && input.key_pressed(egui::Key::C));
    let clicked = input.pointer.primary_clicked();
    Asked {
        copy: (over && clicked) || ctrl_c || nav.extra,
        toggle: ((over_change || over_rule) && clicked) || input.key_pressed(egui::Key::Tab) || nav.extra2,
    }
}

/// a line in the game's dark pill under the top edge: "Connecting to the
/// host…".
pub fn status(canvas: &Canvas, text: &str, screen: egui::Rect, time: f64) {
    let canvas = Canvas { origin: screen.min, ..canvas.clone() };
    let dots = ((time * 2.0) as usize) % 4;
    let text = format!("{}{}", text.trim_end_matches('…'), ".".repeat(dots));
    let width = canvas.kit.measure(&text, 0.95) + 24.0;
    let x = (screen.width() / canvas.unit - width) / 2.0;
    canvas.nine(Art::PillPrompt, x, 10.0, width, 18.0, [7.0, 7.0, 7.0, 7.0], 1.0, Color32::WHITE);
    canvas.text(x + 12.0, 11.5, &text, 0.95, TEXT_CREAM);
}
