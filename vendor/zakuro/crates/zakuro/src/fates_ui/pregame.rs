//! updscreen: the work before the game's code loads (Haiku building the
//! game's module again after an update, or for a version switched to), in
//! the game's own window and look: the teal wallpaper at its own shape, its
//! plain edges extended over a wider window, the ornate bars across the top
//! and bottom, the parchment panel with the steps, the hand on the one
//! being done, the game's bar and font. An error stays on this screen with
//! Try Again, Open the Log and Quit; Esc (or B) asks before stopping.
//!
//! The art is read once, before the work starts (the work may make the
//! game-derived files again, assets/ among them); without it (no assets/
//! yet) the screen stays black and tries again until the work has made it.

use std::time::{Duration, Instant};

use egui::{Color32, Pos2, Rect};
use winit::event_loop::ActiveEventLoop;
use zakuro_gpu::Overlay;

use super::kit::{Art, Canvas, Kit, TEXT_CREAM, TEXT_DARK, TEXT_GOLD, TEXT_MUTED};
use super::{Prompt, Prompts, View};
use crate::pregame::{Pregame, Progress, State};
use crate::App;

/// the steps the screen lists, in order, and what the work calls each
/// (its Progress::step starts with it).
const STEPS: &[(&str, &str)] = &[
    ("Making Haiku's own files", "Haiku's own files"),
    ("Recompiling the game's code", "Writing the game's code as C"),
    ("Compiling the game's code", "Compiling the game's code"),
    ("Linking the game's code", "Linking the game's code"),
];

/// the screen's measures, in the 3DS's pixels (units): a 400x240 stage
/// fitted to the window's height, centred.
const STAGE_W: f32 = 400.0;
const STAGE_H: f32 = 240.0;
const BAR_H: f32 = 30.0;
const PANEL_X: f32 = 34.0;
const PANEL_Y: f32 = 36.0;
const PANEL_W: f32 = 332.0;
const PANEL_H: f32 = 172.0;
const ROW: f32 = 19.0;
/// a step's row, a little shorter than a choice's
const STEP_ROW: f32 = 17.0;

#[derive(Clone, Debug, PartialEq)]
enum Phase {
    /// the window just opened: the art is read, then the work started
    Begin,
    Working,
    /// Esc while working: stop and quit?
    Asking,
    /// the work is done; frames shown since, before the game starts
    Done(u32),
    Failed(String),
}

/// what the player chose this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    Retry,
    OpenLog,
    Quit,
    Ask,
    Keep,
}

pub struct Screen {
    work: Box<dyn Pregame>,
    kit: Option<Kit>,
    kit_tried: Option<Instant>,
    kit_missing_said: bool,
    phase: Phase,
    progress: Progress,
    /// when the work started, and how long it took once it ended
    started: Instant,
    took: Option<Duration>,
    selected: usize,
}

impl Screen {
    pub fn new(work: Box<dyn Pregame>) -> Screen {
        Screen { work, kit: None, kit_tried: None, kit_missing_said: false, phase: Phase::Begin, progress: Progress::default(), started: Instant::now(), took: None, selected: 0 }
    }

    /// the art, read now and then until it is there.
    fn load_kit(&mut self) {
        if self.kit.is_some() || self.kit_tried.is_some_and(|at| at.elapsed() < Duration::from_millis(1000)) {
            return;
        }
        self.kit_tried = Some(Instant::now());
        match Kit::load() {
            Ok(kit) => {
                log::info!("updscreen: the game's art read from {}", super::kit::assets_dir().display());
                self.kit = Some(kit);
            }
            // said once: it is tried again each second until the work made it
            Err(error) if !self.kit_missing_said => {
                self.kit_missing_said = true;
                log::warn!("updscreen: the game's art can't be read yet ({error}): the screen waits for it");
            }
            Err(_) => {}
        }
    }

    fn start(&mut self) {
        self.started = Instant::now();
        self.took = None;
        self.progress = Progress::default();
        self.selected = 0;
        self.phase = match self.work.start() {
            Ok(()) => Phase::Working,
            Err(error) => Phase::Failed(error),
        };
    }

    /// the work's news since the last frame.
    fn poll(&mut self) {
        if !matches!(self.phase, Phase::Working | Phase::Asking) {
            return;
        }
        match self.work.poll() {
            State::Working(progress) => self.progress = progress,
            State::Done => {
                self.took = Some(self.started.elapsed());
                self.progress.percent = 100.0;
                self.phase = Phase::Done(0);
            }
            State::Failed(error) => {
                log::warn!("updscreen: {error}");
                self.took = Some(self.started.elapsed());
                self.selected = 0;
                self.phase = Phase::Failed(error);
            }
        }
    }

    /// the step being done, as an index of STEPS (all of them once done).
    fn stage(&self) -> usize {
        if matches!(self.phase, Phase::Done(_)) {
            return STEPS.len();
        }
        // a step after the last (loading what was built) is past them all
        STEPS.iter().position(|(prefix, _)| self.progress.step.starts_with(prefix)).unwrap_or(if self.progress.percent >= 100.0 { STEPS.len() } else { 0 })
    }

    /// the rows of a choice, while one shows.
    fn choices(&self) -> &'static [&'static str] {
        match self.phase {
            Phase::Failed(_) => &["Try Again", "Open the Log", "Quit"],
            Phase::Asking => &["Keep Building", "Stop and Quit"],
            _ => &[],
        }
    }

    fn choose(&self, index: usize) -> Action {
        match (&self.phase, index) {
            (Phase::Failed(_), 0) => Action::Retry,
            (Phase::Failed(_), 1) => Action::OpenLog,
            (Phase::Failed(_), _) => Action::Quit,
            (_, 0) => Action::Keep,
            _ => Action::Quit,
        }
    }

    /// draws the screen for a frame and takes the player's moves.
    fn frame(&mut self, ctx: &egui::Context, nav: &super::Nav, prompts: &Prompts) -> Option<Action> {
        let screen = ctx.content_rect();
        let ppp = ctx.pixels_per_point();
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("haiku pregame")));
        // nothing to draw with yet: black (the window's own)
        let Some(kit) = self.kit.as_mut() else {
            painter.rect_filled(screen, 0.0, Color32::BLACK);
            return None;
        };
        let unit = (screen.height() / STAGE_H).min(screen.width() / STAGE_W).max(0.25);
        kit.prepare(ctx, unit * ppp);
        let kit = self.kit.as_ref().expect("read above");
        let input = ctx.input(|input| input.clone());
        let now = input.time;
        let white = Color32::WHITE;
        let snap = |v: f32| (v * ppp).round() / ppp;
        let full = Canvas { kit, painter: painter.clone(), origin: screen.min, unit, ppp, alpha: 1.0 };
        let wide = screen.width() / unit;
        let tall = screen.height() / unit;
        let stage_x = (wide - STAGE_W) / 2.0;
        let stage_y = (tall - STAGE_H) / 2.0;
        let stage = Canvas { origin: Pos2::new(snap(screen.min.x + stage_x * unit), snap(screen.min.y + stage_y * unit)), ..full.clone() };

        // the wallpaper at its own shape, as tall as the window, centred; a
        // wider window gets its plain edges (a hatched gradient) extended
        painter.rect_filled(screen, 0.0, Color32::BLACK);
        let paper = kit.size(Art::Wallpaper);
        let (paper_w, paper_h) = (paper.x * tall / paper.y.max(1.0), tall);
        let paper_x = (wide - paper_w) / 2.0;
        full.image(Art::Wallpaper, paper_x, 0.0, paper_w, paper_h, white);
        if paper_x > 0.0 {
            let texel = 1.0 / paper.x.max(1.0);
            let column = |u: f32| Rect::from_min_max(Pos2::new(u, 0.0), Pos2::new(u + texel * 0.5, 1.0));
            // a little darker than the art, so the stage stands out
            let side = Color32::from_gray(150);
            full.image_uv(Art::Wallpaper, full.rect(0.0, 0.0, paper_x + 0.5, paper_h), column(texel * 0.25), side);
            full.image_uv(Art::Wallpaper, full.rect(paper_x + paper_w - 0.5, 0.0, paper_x + 0.5, paper_h), column(1.0 - texel * 0.75), side);
        }
        // the bars across the whole width, their ornate ends as they are
        full.three(Art::Header, 0.0, stage_y, wide, BAR_H, [60.0, 60.0], Color32::from_rgb(200, 120, 100));
        full.three(Art::Header, 0.0, stage_y + STAGE_H - BAR_H, wide, BAR_H, [60.0, 60.0], white);
        stage.text(14.0, 7.0, "Updating", 1.0, TEXT_GOLD);

        // the prompts in the top bar's corner
        let entries: Vec<(Prompt, &str)> = match self.phase {
            Phase::Working => vec![(Prompt::Back, "Stop")],
            Phase::Asking => vec![(Prompt::Confirm, "Select"), (Prompt::Back, "Keep Building")],
            Phase::Failed(_) => vec![(Prompt::Confirm, "Select"), (Prompt::Back, "Quit")],
            _ => Vec::new(),
        };
        if !entries.is_empty() {
            let width = super::prompts_width(&stage, prompts, &entries);
            let px = STAGE_W - 8.0 - width - 10.0;
            stage.nine(Art::PillPrompt, px, 6.0, width + 10.0, 18.0, [7.0, 7.0, 7.0, 7.0], 1.0, white);
            super::draw_prompts(&stage, prompts, px + 5.0, 7.0, &entries);
        }

        // the panel
        stage.nine(Art::Panel, PANEL_X, PANEL_Y, PANEL_W, PANEL_H, [60.0, 40.0, 60.0, 40.0], 1.0, white);
        let mid = STAGE_W / 2.0;
        let bob = ((now * 6.0).sin() * 1.5) as f32;
        let (left, right) = (PANEL_X + 22.0, PANEL_X + PANEL_W - 22.0);
        let pointer = input.pointer.hover_pos().filter(|_| input.pointer.is_moving() || input.pointer.any_click()).map(|p| stage.unit_of(p));
        let clicked = input.pointer.primary_clicked();
        let mut action = None;
        let help;
        let choices = self.choices();
        if choices.is_empty() {
            // the steps, the bar, the files and the time
            let current = self.stage();
            let title = if current == STEPS.len() { "Starting the game" } else { "Building the game's code" };
            stage.text_centered(mid, PANEL_Y + 10.0, title, fit(kit, title, 0.95), TEXT_DARK);
            // two lines under it, smaller when a translation needs three
            let why = self.work.why();
            let (mut scale, mut lines) = (0.66, kit.wrap_even(&why, PANEL_W - 80.0, 0.66));
            if lines.len() > 2 {
                scale = 0.56;
                lines = kit.wrap_even(&why, PANEL_W - 70.0, scale);
            }
            let line_h = if lines.len() > 2 { 8.5 } else { 10.5 } * spacing(&why);
            for (i, line) in lines.iter().take(3).enumerate() {
                stage.text_centered(mid, PANEL_Y + 28.0 + i as f32 * line_h, line, scale, TEXT_MUTED);
            }
            let top = PANEL_Y + 54.0;
            for (i, (_, name)) in STEPS.iter().enumerate() {
                let y = top + i as f32 * STEP_ROW;
                let here = i == current;
                if here {
                    stage.three(Art::Highlight, left, y - 1.0, right - left, STEP_ROW + 1.0, [10.0, 10.0], white);
                    stage.sprite(Art::Hand, left - 18.0 + bob, y + 1.5, white);
                } else if i + 1 < STEPS.len() {
                    stage.fill(left + 4.0, y + STEP_ROW - 0.5, right - left - 8.0, 0.6, Color32::from_rgba_unmultiplied(70, 110, 110, 70));
                }
                let colour = if here { TEXT_CREAM } else if i < current { TEXT_DARK } else { TEXT_MUTED };
                stage.text(left + 10.0, y + 2.0, name, 0.8, colour);
                let state = if i < current {
                    "Done".to_owned()
                } else if here {
                    files(&self.progress.detail).map_or_else(|| ".".repeat(((now * 2.0) as usize) % 3 + 1), |(n, m)| format!("{n} / {m}"))
                } else {
                    String::new()
                };
                stage.text_right(right - 10.0, y + 2.5, &state, 0.74, colour);
            }
            // the bar: the game's pill as its track, the red bar as its fill
            let bar_y = top + STEPS.len() as f32 * STEP_ROW + 6.0;
            let (bar_x, bar_w) = (left + 4.0, right - left - 44.0);
            stage.nine(Art::PillLabel, bar_x, bar_y, bar_w, 10.0, [8.0, 8.0, 8.0, 8.0], 0.5, white);
            let filled = (bar_w - 4.0) * (self.progress.percent / 100.0).clamp(0.0, 1.0);
            if filled > 2.0 {
                stage.three(Art::Highlight, bar_x + 2.0, bar_y + 1.5, filled, 7.0, [3.0, 3.0], white);
            }
            stage.text_right(right - 4.0, bar_y - 2.0, &format!("{:.0}%", self.progress.percent.clamp(0.0, 100.0)), 0.76, TEXT_DARK);
            let detail = if matches!(self.phase, Phase::Done(_)) { "Done".to_owned() } else { self.progress.detail.clone() };
            let seconds = self.took.unwrap_or_else(|| self.started.elapsed()).as_secs();
            let time = format!("{}:{:02}", seconds / 60, seconds % 60);
            // release141: the compile's longer lines (compile32: "2 of 4 files,
            // 0:02 so far. Compiling code233.c (0:02) and 1 other") end before
            // the time instead of running under it and off the panel
            let room = right - 4.0 - kit.measure(&time, 0.64) - 6.0 - bar_x;
            let (line, scale) = fit_line(kit, &detail, room, 0.64);
            stage.text(bar_x, bar_y + 12.0 + (0.64 - scale) * 6.0, &line, scale, TEXT_DARK);
            stage.text_right(right - 4.0, bar_y + 12.0, &time, 0.64, TEXT_DARK);
            help = if matches!(self.phase, Phase::Done(_)) { "" } else { "The game starts by itself when this is done." };
            if self.phase == Phase::Working && nav.back {
                action = Some(Action::Ask);
            }
        } else {
            // a question, or the error: the text, then the choices
            let (title, text) = match &self.phase {
                Phase::Failed(error) => ("The game's code couldn't be built", error.clone()),
                _ => ("Stop building?", "Haiku closes now, and carries on from where it stopped the next time it starts.".to_owned()),
            };
            stage.text_centered(mid, PANEL_Y + 10.0, title, fit(kit, title, 0.95), TEXT_DARK);
            let lines = kit.wrap(&break_long(kit, &text, PANEL_W - 70.0, 0.72), PANEL_W - 70.0, 0.72);
            for (i, line) in lines.iter().take(4).enumerate() {
                stage.text_centered(mid, PANEL_Y + 30.0 + i as f32 * 11.5 * spacing(&text), line, 0.72, TEXT_DARK);
            }
            let top = PANEL_Y + PANEL_H - 26.0 - choices.len() as f32 * ROW;
            if nav.up {
                self.selected = (self.selected + choices.len() - 1) % choices.len();
            }
            if nav.down {
                self.selected = (self.selected + 1) % choices.len();
            }
            for (i, label) in choices.iter().enumerate() {
                let y = top + i as f32 * ROW;
                let (x0, x1) = (left + 40.0, right - 40.0);
                if pointer.is_some_and(|(px, py)| (x0..x1).contains(&px) && (y..y + ROW).contains(&py)) {
                    self.selected = i;
                    if clicked {
                        action = Some(self.choose(i));
                    }
                }
                let here = i == self.selected;
                if here {
                    stage.three(Art::Highlight, x0, y - 1.0, x1 - x0, ROW + 1.0, [10.0, 10.0], white);
                    stage.sprite(Art::Hand, x0 - 18.0 + bob, y + 2.5, white);
                }
                stage.text_centered(mid, y + 2.0, label, 0.95, if here { TEXT_CREAM } else { TEXT_DARK });
            }
            if nav.confirm {
                action = Some(self.choose(self.selected));
            }
            if nav.back {
                action = Some(match self.phase {
                    Phase::Asking => Action::Keep,
                    _ => Action::Quit,
                });
            }
            help = match (&self.phase, self.selected) {
                (Phase::Failed(_), 0) => "Builds the game's code again, from where it stopped.",
                (Phase::Failed(_), 1) => "Opens Haiku's setup log, which has the compiler's messages.",
                (Phase::Failed(_), _) => "Closes Haiku. It tries again the next time it starts.",
                (_, 0) => "Goes on building the game's code.",
                _ => "Closes Haiku. It carries on from where it stopped the next time it starts.",
            };
        }

        // the help line in the bottom bar
        let footer = STAGE_H - BAR_H;
        let lines = kit.wrap_even(help, wide.min(STAGE_W + 60.0) - 40.0, 0.85);
        let line_h = 12.0;
        let first = footer + (BAR_H - lines.len() as f32 * line_h) / 2.0 - 1.0;
        for (i, line) in lines.iter().enumerate() {
            stage.text_centered(mid, first + i as f32 * line_h, line, 0.85, TEXT_CREAM);
        }
        action
    }
}

/// how much further apart lines go in the text's language: Japanese
/// letters fill the whole line's height, Latin ones don't.
fn spacing(text: &str) -> f32 {
    if super::ja::is_cjk(&super::ja::tr(text)) {
        1.2
    } else {
        1.0
    }
}

/// the scale a title is drawn at: as asked, or smaller to keep it inside
/// the panel's top edge.
fn fit(kit: &Kit, title: &str, scale: f32) -> f32 {
    scale.min(scale * (PANEL_W - 110.0) / kit.measure(title, scale).max(1.0))
}

/// release141: a line and the scale it is drawn at to fit in width units:
/// as asked, else smaller (down to 0.5), else also cut after a word with
/// "...".
fn fit_line(kit: &Kit, text: &str, width: f32, scale: f32) -> (String, f32) {
    let wide = kit.measure(text, scale);
    if wide <= width {
        return (text.to_owned(), scale);
    }
    let smaller = (scale * width / wide * 0.99).max(0.5);
    if kit.measure(text, smaller) <= width {
        return (text.to_owned(), smaller);
    }
    let words: Vec<&str> = text.split(' ').collect();
    for n in (1..words.len()).rev() {
        let cut = format!("{}...", words[..n].join(" ").trim_end_matches([',', '.', ';', ':']));
        if kit.measure(&cut, smaller) <= width {
            return (cut, smaller);
        }
    }
    (text.to_owned(), smaller)
}

/// text whose words (a path, say) are wider than width units broken after
/// their \ or / where they would run over, so that wrapping keeps them on
/// the panel.
fn break_long(kit: &Kit, text: &str, width: f32, scale: f32) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for (i, word) in text.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        if kit.measure(word, scale) <= width {
            out.push_str(word);
            continue;
        }
        let mut line = String::new();
        for piece in word.split_inclusive(['\\', '/']) {
            if !line.is_empty() && kit.measure(&format!("{line}{piece}"), scale) > width {
                out.push_str(&line);
                out.push('\n');
                line.clear();
            }
            line.push_str(piece);
        }
        out.push_str(&line);
    }
    out
}

/// "12 of 254 files..." as (12, 254).
fn files(detail: &str) -> Option<(u32, u32)> {
    let (n, rest) = detail.split_once(" of ")?;
    let m = rest.split(|c: char| !c.is_ascii_digit()).next()?;
    rest[m.len()..].trim_start().starts_with("files").then_some(())?;
    Some((n.trim().parse().ok()?, m.parse().ok()?))
}

impl App {
    /// updscreen: a frame of the work before the game: the art read and the
    /// work started first, the screen drawn, the player's choice done; once
    /// the work is done the game's code is taken and the game started here.
    pub(crate) fn pregame_frame(&mut self, event_loop: &ActiveEventLoop) -> Overlay {
        let Some(screen) = self.pregame.as_mut() else { return Overlay::default() };
        if screen.phase == Phase::Begin {
            // before the work, which may make assets/ again
            screen.load_kit();
            screen.start();
        }
        screen.load_kit();
        screen.poll();
        // no art at all and an error: the program says it its own way
        if let (None, Phase::Failed(error)) = (&screen.kit, &screen.phase) {
            let error = error.clone();
            if screen.work.failed_without_art(&error) {
                screen.start();
            } else {
                screen.work.stop();
                event_loop.exit();
            }
            return Overlay::default();
        }
        // the game's code, a frame after "Starting the game" showed
        if let Phase::Done(shown) = &mut screen.phase {
            *shown += 1;
            if *shown > 2 {
                match screen.work.finish() {
                    Ok(linked) => {
                        log::info!("updscreen: the game's code is ready after {:.1} s: the game starts", screen.started.elapsed().as_secs_f32());
                        self.pregame = None;
                        self.linked = Some(linked);
                        self.next_frame = Instant::now();
                        self.start();
                        return Overlay::default();
                    }
                    Err(error) => {
                        log::warn!("updscreen: {error}");
                        screen.selected = 0;
                        screen.phase = Phase::Failed(error);
                    }
                }
            }
        }
        let view = View { pad_held: self.fates.pad_held, pad_circle: self.fates.pad_circle, ..View::default() };
        let prompts = Prompts { set: crate::glyphs::forced().unwrap_or_else(crate::input_api::glyph_set) };
        let (Some(gui), Some(window)) = (&mut self.gui, &self.window) else { return Overlay::default() };
        let fates = &mut self.fates;
        let Some(screen) = self.pregame.as_mut() else { return Overlay::default() };
        let mut action = None;
        let overlay = gui.frame(window, |ui| {
            let input = ui.ctx().input(|input| input.clone());
            let nav = fates.nav(&input, &view, false);
            action = screen.frame(ui.ctx(), &nav, &prompts);
        });
        fates.prev_pad = view.pad_held;
        match action {
            Some(Action::Retry) => {
                log::info!("updscreen: Try Again");
                screen.start();
            }
            Some(Action::OpenLog) => screen.work.open_log(),
            Some(Action::Quit) => {
                log::info!("updscreen: the player quit");
                screen.work.stop();
                event_loop.exit();
            }
            Some(Action::Ask) => {
                screen.selected = 0;
                screen.phase = Phase::Asking;
            }
            Some(Action::Keep) => screen.phase = Phase::Working,
            None => {}
        }
        overlay
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn files_are_read_from_the_detail() {
        assert_eq!(super::files("12 of 254 files, about 3 min left"), Some((12, 254)));
        assert_eq!(super::files("0 of 2 files"), Some((0, 2)));
        assert_eq!(super::files("Writing the C"), None);
        assert_eq!(super::files("3 of 9 EUR files"), None);
    }
}
