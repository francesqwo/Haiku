//! info-ui: the growth rates on the lower screen's unit status, drawn in
//! the game's own style over its 320 x 240 picture.
//!
//! - **The button** is the status card's own square button (the stats
//!   window's book button, from the game's status sheet) with a rising-bars
//!   icon in place of the book, in its three looks: off, on (the game's lit
//!   red), and pointed at. It sits on the portrait's lower right, just above
//!   the stats window's right column.
//! - **The growth view** is the game's own stats window (the Str to Res
//!   pills, from the same sheet), drawn where the game draws it, with each
//!   pill's number the stat's growth rate, written in the game's UI font as
//!   the game writes the stats; the HP plate shows the HP growth in its own
//!   large numbers. A dark red label pill above says what is shown.

use egui::Color32;

use super::kit::{Art, Canvas, TEXT_CREAM};

/// the button's place on the lower screen, in its pixels.
pub const BUTTON: (f32, f32, f32, f32) = (68.0, 134.0, 39.0, 23.0);

/// where the game draws the stats window and its HP plate.
const WINDOW: (f32, f32) = (0.0, 158.0);
const PLATE: (f32, f32) = (136.0, 30.0);
/// the detailed card's HP plate (Atk, Hit and EXP beside it)
const PLATE_DETAILED: (f32, f32) = (112.0, 22.0);

/// the stats window's numbers, as the game places them (measured on its
/// own picture): the glyphs' right edges (Str, Mag, Skl; Spd, Lck, Def,
/// Res), the first row's glyph top and the rows' step. The digits are the
/// UI font's second size, 8 x 14 cells, the size the game writes them in.
const LEFT_RIGHT_EDGE: f32 = 50.0;
const RIGHT_RIGHT_EDGE: f32 = 101.0;
const FIRST_ROW_TOP: f32 = 162.0;
const ROW_STEP: f32 = 17.0;
const DIGIT_ADVANCE: f32 = 7.0;
/// the outline the game gives its numbers
const OUTLINE: Color32 = Color32::from_rgb(92, 40, 8);
/// the stats' own cream (the game's colour table, 0x00717648 +0x1C)
const NUMBER: Color32 = Color32::from_rgb(253, 236, 185);

/// what to draw this frame.
#[derive(Clone, Debug, Default)]
pub struct Panel {
    /// HP, Str, Mag, Skl, Spd, Lck, Def, Res
    pub rates: [i32; 8],
    /// the growth view is on
    pub on: bool,
    /// the pointer is over the button
    pub hover: bool,
    /// the detailed card (its HP plate higher and further left)
    pub detailed: bool,
}

pub fn inside_button(x: f32, y: f32) -> bool {
    let (bx, by, bw, bh) = BUTTON;
    x >= bx && y >= by && x < bx + bw && y < by + bh
}

/// a glyph of the UI font with the game's outline around it.
fn glyph(canvas: &Canvas, art: Art, x: f32, y: f32, w: f32, h: f32, colour: Color32, outline: Color32) {
    for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0), (-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        canvas.image(art, x + dx * 0.6, y + dy * 0.6, w, h, outline);
    }
    canvas.image(art, x, y, w, h, colour);
}

/// a number in the UI font, its last glyph ending at right.
fn number(canvas: &Canvas, right: f32, top: f32, value: i32) {
    let text = value.max(0).to_string();
    let mut x = right - 8.0 - (text.len() as f32 - 1.0) * DIGIT_ADVANCE;
    for c in text.chars() {
        let d = c.to_digit(10).unwrap_or(0) as u8;
        glyph(canvas, Art::UiGlyph(d), x, top, 8.0, 14.0, NUMBER, OUTLINE);
        x += DIGIT_ADVANCE;
    }
}

/// the HP plate's large numbers, centred on centre.
fn big_number(canvas: &Canvas, centre: f32, top: f32, value: i32) {
    let text = value.max(0).to_string();
    let advance = 15.0;
    let percent = 9.0;
    let width = text.len() as f32 * advance + percent;
    let mut x = centre - width / 2.0;
    for c in text.chars() {
        let d = c.to_digit(10).unwrap_or(0) as u8;
        glyph(canvas, Art::BigDigit(d), x - 1.0, top, 17.0, 25.0, Color32::from_rgb(255, 238, 170), OUTLINE);
        x += advance;
    }
    glyph(canvas, Art::UiGlyph(10), x, top + 7.0, 10.0, 17.5, NUMBER, OUTLINE);
}

/// draws the button and, when on, the growth view; the canvas' origin is
/// the lower screen's top left and its unit one of its pixels.
pub fn draw(canvas: &Canvas, panel: &Panel) {
    let white = Color32::WHITE;
    if panel.on {
        // the label above the window
        canvas.nine(Art::PillLabel, 1.0, 141.0, 64.0, 15.0, [8.0, 8.0, 8.0, 8.0], 0.5, white);
        canvas.text_centered(33.0, 141.5, "Growths", 0.62, TEXT_CREAM);
        // the stats window, its numbers the growth rates
        canvas.sprite(Art::AbilityWindow, WINDOW.0, WINDOW.1, white);
        for i in 1..8 {
            let (right, row) = if i <= 3 { (LEFT_RIGHT_EDGE, i - 1) } else { (RIGHT_RIGHT_EDGE, i - 4) };
            number(canvas, right, FIRST_ROW_TOP + row as f32 * ROW_STEP, panel.rates[i]);
        }
        // the HP plate, its number the HP growth
        let (art, (x, y), centre) = if panel.detailed { (Art::HpPlateDetailed, PLATE_DETAILED, 73.0) } else { (Art::HpPlate, PLATE, 76.0) };
        canvas.sprite(art, x, y, white);
        big_number(canvas, x + centre, y + 7.0, panel.rates[0]);
    }
    // on stays lit (the view's state is always visible); off lights up
    // under the pointer, as the game's buttons do
    let look = if panel.on { 1 } else if panel.hover { 2 } else { 0 };
    let (x, y, _, _) = BUTTON;
    canvas.sprite(Art::GrowthButton(look), x, y, white);
}
