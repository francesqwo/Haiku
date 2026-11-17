//! the system keyboard a title can open to have text typed, drawn as the
//! game's own name-entry screen (the one it names the avatar with): the
//! orange panels with their light rims, the name on a bar between the L and
//! R glyphs with the underline, the button column (red selection with the
//! hand, cream Del, dark red Back, green OK), the letters in plain brown
//! with the bracket cursor, and the help line on the dark band along the
//! bottom. Typing on the keyboard, the pointer and a controller all work.

use egui::{Color32, CornerRadius};
use zakuro_core::services::keyboard::Request;

use super::kit::{Art, Canvas, TEXT_CREAM, TEXT_DARK, TEXT_MUTED};
use super::{draw_prompts, prompts_width, Nav, Prompt, Prompts};
use crate::controls::GlyphSet;

const WIDTH: f32 = 400.0;
const HEIGHT: f32 = 240.0;
/// the letter grid: two blocks of five, upper and lower case, as the game.
const ROWS: [&str; 7] = ["ABCDEabcde", "FGHIJfghij", "KLMNOklmno", "PQRSTpqrst", "UVWXYuvwxy", "Z-.'&z!?,/", "0123456789"];
const COLUMN_X: [f32; 10] = [139.0, 159.0, 179.0, 199.0, 219.0, 258.0, 278.0, 298.0, 318.0, 338.0];
const ROW_Y: f32 = 63.0;
const PITCH: f32 = 20.0;
const BUTTON_X: f32 = 45.0;
const BUTTON_W: f32 = 75.0;

/// the game's panel colours: the dark edge, the light rim, the inner
/// shade and the flat orange.
const EDGE: Color32 = Color32::from_rgb(177, 109, 41);
const RIM: Color32 = Color32::from_rgb(239, 173, 115);
const SHADE: Color32 = Color32::from_rgb(205, 139, 65);
const FILL: Color32 = Color32::from_rgb(206, 156, 74);
const CURSOR_FILL: Color32 = Color32::from_rgb(232, 196, 135);
const UNDERLINE: Color32 = Color32::from_rgb(153, 115, 54);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    Space,
    Delete,
    /// one of the title's buttons, the last confirms.
    Button(usize),
}

#[derive(Default)]
pub struct Keyboard {
    /// what it asked for, and the text so far.
    typing: Option<(Request, String)>,
    /// the selected place: column 0 is the buttons, 1 to 10 the letters.
    row: usize,
    col: usize,
    last_pointer: Option<egui::Pos2>,
    opened: f64,
    /// online: the line under the letters, in place of "Enter a name."
    pub help: Option<String>,
}

/// the game's orange panel, in units.
fn panel(canvas: &Canvas, x: f32, y: f32, w: f32, h: f32) {
    let u = canvas.unit;
    let mut rect = canvas.rect(x, y, w, h);
    let mut radius = 3.5 * u;
    for (colour, inset) in [(EDGE, 0.0), (RIM, 0.8), (SHADE, 1.0), (FILL, 1.0)] {
        rect = rect.shrink(inset * u);
        radius = (radius - inset * u).max(0.0);
        canvas.painter.rect_filled(rect, CornerRadius::same(radius.min(255.0) as u8), colour.gamma_multiply(canvas.alpha));
    }
}

impl Keyboard {
    pub fn forget(&mut self) {
        self.typing = None;
    }

    fn actions(request: &Request) -> Vec<Action> {
        let mut list = vec![Action::Space, Action::Delete];
        list.extend((0..request.buttons.len().max(1)).map(Action::Button));
        list
    }

    fn label(request: &Request, action: Action) -> String {
        match action {
            Action::Space => "Space".to_owned(),
            Action::Delete => "Del".to_owned(),
            Action::Button(index) => {
                let given = request.buttons.get(index).map(|s| s.trim()).unwrap_or("");
                if !given.is_empty() {
                    given.to_owned()
                } else if index + 1 == request.buttons.len().max(1) {
                    "OK".to_owned()
                } else {
                    "Back".to_owned()
                }
            }
        }
    }

    fn key_at(row: usize, col: usize) -> char {
        ROWS[row].chars().nth(col - 1).unwrap_or(' ')
    }

    /// shows the keyboard and takes what is typed. some text and the
    /// button that closed it, once one does.
    pub fn frame(&mut self, canvas: &Canvas, input: &egui::InputState, nav: &Nav, prompts: &Prompts, request: &Request, screen: egui::Rect) -> Option<(String, usize)> {
        let fresh = self.typing.as_ref().is_none_or(|(asked, _)| asked != request);
        if fresh {
            self.typing = Some((request.clone(), request.text.clone()));
            self.row = 0;
            self.col = 1;
            self.opened = input.time;
        }
        let actions = Self::actions(request);
        let confirm_index = request.buttons.len().max(1) - 1;
        let unit = canvas.unit;
        let origin = egui::pos2(screen.center().x - WIDTH * unit / 2.0, screen.center().y - HEIGHT * unit / 2.0);
        let snap = |v: f32| (v * canvas.ppp).round() / canvas.ppp;
        let fade = ((input.time - self.opened) / 0.12).clamp(0.0, 1.0) as f32;
        let canvas = Canvas { origin: egui::pos2(snap(origin.x), snap(origin.y)), alpha: fade, ..canvas.clone() };
        let mut text = self.typing.as_ref()?.1.clone();
        let mut pressed: Option<Action> = None;
        let mut typed = String::new();

        // typing on the keyboard
        for event in &input.events {
            if let egui::Event::Text(t) = event {
                typed.push_str(t);
            }
            // online: a code pasted with Ctrl+V
            if let egui::Event::Paste(t) = event {
                typed.push_str(t.trim());
            }
        }
        if input.key_pressed(egui::Key::Backspace) {
            pressed = Some(Action::Delete);
        }
        if input.key_pressed(egui::Key::Enter) {
            pressed = Some(Action::Button(confirm_index));
        }
        if input.key_pressed(egui::Key::Escape) && request.buttons.len() > 1 {
            pressed = Some(Action::Button(0));
        }

        // moving through the keys: the button column, then the letters
        let height = |col: usize| if col == 0 { actions.len() } else { ROWS.len() };
        if nav.up {
            self.row = (self.row + height(self.col) - 1) % height(self.col);
        }
        if nav.down {
            self.row = (self.row + 1) % height(self.col);
        }
        if nav.left {
            self.col = (self.col + 10) % 11;
        }
        if nav.right {
            self.col = (self.col + 1) % 11;
        }
        self.row = self.row.min(height(self.col) - 1);
        if nav.back || nav.extra {
            pressed = Some(Action::Delete);
        }
        if nav.extra2 {
            pressed = Some(Action::Space);
        }
        if nav.start {
            self.col = 0;
            self.row = actions.len() - 1;
        }
        if nav.confirm {
            if self.col == 0 {
                pressed = Some(actions[self.row]);
            } else {
                typed.push(Self::key_at(self.row, self.col));
            }
        }

        // the pointer
        let pointer = input.pointer.hover_pos();
        let moved = pointer.is_some() && pointer != self.last_pointer;
        self.last_pointer = pointer;
        if let Some(pos) = pointer {
            let (x, y) = canvas.unit_of(pos);
            let mut over = None;
            for (row, _) in actions.iter().enumerate() {
                let by = 54.0 + row as f32 * 22.0;
                if (BUTTON_X..BUTTON_X + BUTTON_W).contains(&x) && (by..by + 19.0).contains(&y) {
                    over = Some((row, 0));
                }
            }
            for row in 0..ROWS.len() {
                for (index, cx) in COLUMN_X.iter().enumerate() {
                    let cy = ROW_Y + row as f32 * PITCH;
                    if (x - cx).abs() < 9.5 && (y - cy).abs() < 9.5 {
                        over = Some((row, index + 1));
                    }
                }
            }
            if let Some((row, col)) = over {
                if moved {
                    self.row = row;
                    self.col = col;
                }
                if input.pointer.primary_clicked() {
                    self.row = row;
                    self.col = col;
                    if col == 0 {
                        pressed = Some(actions[row]);
                    } else {
                        typed.push(Self::key_at(row, col));
                    }
                }
            }
            if input.pointer.secondary_clicked() {
                pressed = Some(Action::Delete);
            }
        }

        // what the keys did
        for c in typed.chars().filter(|c| !c.is_control()) {
            if (text.encode_utf16().count() + c.len_utf16()) <= request.max_length {
                text.push(c);
            }
        }
        let mut done = None;
        match pressed {
            Some(Action::Delete) => {
                text.pop();
            }
            Some(Action::Space) => {
                if text.encode_utf16().count() < request.max_length {
                    text.push(' ');
                }
            }
            Some(Action::Button(index)) => {
                if index != confirm_index || request.accepts(&text) {
                    done = Some((text.clone(), index));
                }
            }
            None => {}
        }
        let valid = request.accepts(&text);
        if let Some((_, kept)) = self.typing.as_mut() {
            kept.clone_from(&text);
        }

        // drawing it, the game's screen over the dimmed game
        let white = Color32::WHITE;
        let bob = ((input.time * 6.0).sin() * 1.5) as f32;
        canvas.painter.rect_filled(screen, 0.0, Color32::from_black_alpha((140.0 * fade) as u8));

        // the name between L and R
        panel(&canvas, 36.0, 6.0, 328.0, 36.0);
        let shown = if text.is_empty() { request.hint.trim() } else { text.as_str() };
        let colour = if text.is_empty() { TEXT_MUTED } else { TEXT_DARK };
        // online: a long text (a code) shrinks to stay on the bar
        let mut scale = 1.05;
        while scale > 0.6 && canvas.kit.measure(shown, scale) > 300.0 {
            scale -= 0.05;
        }
        let width = canvas.kit.measure(shown, scale);
        canvas.text(200.0 - width / 2.0, 12.0 + (1.05 - scale) * 8.0, shown, scale, colour);
        canvas.fill(160.0, 33.0, 80.0, 1.6, UNDERLINE);
        if (input.time * 2.0).fract() < 0.6 && !text.is_empty() {
            canvas.fill(200.0 + width / 2.0 + 1.0, 33.0, 5.0, 1.6, Color32::from_rgb(220, 40, 20));
        }
        // online: the code box has no length to count to
        if self.help.is_none() {
            let count = format!("{}/{}", text.encode_utf16().count(), request.max_length);
            canvas.text_right(325.0, 16.0, &count, 0.7, UNDERLINE);
        }

        // the panel of keys
        panel(&canvas, 36.0, 44.0, 328.0, 160.0);
        for (row, action) in actions.iter().enumerate() {
            let y = 54.0 + row as f32 * 22.0;
            let chosen = self.col == 0 && self.row == row;
            let label = Self::label(request, *action);
            match action {
                Action::Button(i) => {
                    let confirm = *i == confirm_index;
                    let art = match (confirm, chosen, confirm && !valid) {
                        (_, _, true) => Art::ButtonGray,
                        (true, false, _) => Art::ButtonGreen,
                        (true, true, _) => Art::ButtonGreenLit,
                        (false, false, _) => Art::ButtonRed,
                        (false, true, _) => Art::ButtonRedLit,
                    };
                    canvas.nine(art, BUTTON_X, y - 1.0, BUTTON_W, 21.0, [9.0, 9.0, 9.0, 9.0], 0.5, white);
                    let colour = if confirm && !valid { Color32::from_rgb(222, 218, 208) } else { TEXT_CREAM };
                    canvas.text_centered(BUTTON_X + BUTTON_W / 2.0, y + 2.0, &label, 1.05, colour);
                }
                _ => {
                    let pill = if chosen { Art::PillRed } else { Art::PillCream };
                    canvas.nine(pill, BUTTON_X, y, BUTTON_W, 18.0, [9.0, 9.0, 9.0, 9.0], 0.7, white);
                    let colour = if chosen { TEXT_CREAM } else { TEXT_DARK };
                    let prompt = if *action == Action::Delete { Prompt::Extra } else { Prompt::Extra2 };
                    let glyph = if prompts.set == GlyphSet::Keyboard { 0.0 } else { 15.0 };
                    let width = glyph + canvas.kit.measure(&label, 1.05);
                    let x = BUTTON_X + (BUTTON_W - width) / 2.0;
                    if glyph > 0.0 {
                        super::draw_prompt(&canvas, prompts, x, y + 1.5, prompt);
                    }
                    canvas.text(x + glyph, y + 1.5, &label, 1.05, colour);
                }
            }
            if chosen {
                canvas.sprite(Art::Hand, BUTTON_X - 22.0 + bob, y + 3.0, white);
            }
        }
        for (row, keys) in ROWS.iter().enumerate() {
            for (index, (c, cx)) in keys.chars().zip(COLUMN_X).enumerate() {
                let cy = ROW_Y + row as f32 * PITCH;
                if self.col == index + 1 && self.row == row {
                    canvas.fill(cx - 7.5, cy - 8.5, 15.0, 17.0, CURSOR_FILL);
                    canvas.image(Art::Bracket, cx - 9.5, cy - 10.5, 19.0, 21.0, white);
                }
                canvas.text_centered(cx, cy - 9.0, &c.to_string(), 1.05, TEXT_DARK);
            }
        }

        // the help line on the game's dark band, and the prompts
        let band = canvas.rect(0.0, 208.0, WIDTH, 32.0);
        canvas.painter.rect_filled(band, 0.0, Color32::from_black_alpha((150.0 * fade) as u8));
        let help = if let Some(help) = &self.help {
            help.as_str() // online
        } else if request.hint.trim().is_empty() || !text.is_empty() {
            "Enter a name."
        } else {
            request.hint.trim()
        };
        canvas.text_centered(WIDTH / 2.0, 215.0, help, 1.05, TEXT_CREAM);
        let entries: Vec<(Prompt, &str)> = if prompts.set == GlyphSet::Keyboard {
            vec![(Prompt::Confirm, "OK"), (Prompt::Back, "Back")][..if request.buttons.len() > 1 { 2 } else { 1 }].to_vec()
        } else {
            vec![(Prompt::Confirm, "Enter"), (Prompt::Back, "Delete"), (Prompt::Start, "Finish")]
        };
        let width = prompts_width(&canvas, prompts, &entries);
        let px = WIDTH - 4.0 - width - 10.0;
        canvas.nine(Art::PillPrompt, px, -14.0, width + 10.0, 17.0, [7.0, 7.0, 7.0, 7.0], 1.0, white);
        draw_prompts(&canvas, prompts, px + 5.0, -13.5, &entries);

        if done.is_some() {
            self.typing = None;
        }
        done
    }
}
