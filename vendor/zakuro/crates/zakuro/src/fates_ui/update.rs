//! updater: the update's notice on the game's Main Menu and its screen, in
//! the game's look (the options menu's wallpaper, bars, parchment panel,
//! red highlight and hand, the game's font and button pictures).
//!
//! - The notice: a pill in the window's lower left corner while the Main
//!   Menu is up, "Haiku 1.4 is available", with the key (U) or the
//!   controller's button (Y) that opens it; a click opens it too.
//! - The screen: the release's title and notes (GitHub's Markdown as plain
//!   headings, paragraphs and bullets), scrolled with up and down, the
//!   wheel or the arrows; Install, Later and Skip This Version.
//! - Install: the download's progress (Downloading, Checking, Unpacking,
//!   Installing, the bar, the megabytes), B or Esc cancels until the files
//!   go in; then Haiku starts again (App::fates_update_frame). A failure says
//!   why in a line, with Try Again or, for a release Haiku won't install
//!   itself, Open the Download Page.

use std::sync::Arc;

use egui::Color32;

use super::kit::{Art, Canvas, TEXT_CREAM, TEXT_DARK, TEXT_GOLD, TEXT_MUTED};
use super::{Nav, Prompt, Prompts};
use crate::haiku_update::{self, Install, Line, Release, Stage};

/// the screen's measures in units, as the options menu's
pub const WIDTH: f32 = super::options::WIDTH;
pub const HEIGHT: f32 = super::options::HEIGHT;
const HEADER: f32 = 34.0;
const FOOTER_Y: f32 = HEIGHT - HEADER;
const PANEL_X: f32 = 14.0;
const PANEL_Y: f32 = 40.0;
const PANEL_W: f32 = WIDTH - 28.0;
const PANEL_H: f32 = 204.0;
/// the notes' window inside the panel
const NOTES_X: f32 = PANEL_X + 24.0;
const NOTES_Y: f32 = PANEL_Y + 28.0;
const NOTES_W: f32 = PANEL_W - 60.0;
const NOTES_H: f32 = 124.0;
const CHOICES_Y: f32 = PANEL_Y + PANEL_H - 36.0;
const ROW: f32 = 19.0;
const STEP_ROW: f32 = 17.0;

#[derive(Clone, Debug, PartialEq)]
enum Phase {
    Notes,
    Working,
    Failed { line: String, page: bool },
    /// installed: Haiku starts again
    Restarting,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Choice {
    Install,
    Later,
    Skip,
    Cancel,
    TryAgain,
    OpenPage,
    /// OpenPage in the notes' row of three, shorter
    Page,
    Close,
}

impl Choice {
    fn label(self) -> &'static str {
        match self {
            Choice::Install => "Install",
            Choice::Later => "Later",
            Choice::Skip => "Skip This Version",
            Choice::Cancel => "Cancel",
            Choice::TryAgain => "Try Again",
            Choice::OpenPage => "Open the Download Page",
            Choice::Page => "Download Page",
            Choice::Close => "Close",
        }
    }
}

/// a line of the notes laid out: its text, scale, colour, and its x
#[derive(Clone)]
struct Row {
    text: String,
    scale: f32,
    colour: Color32,
    x: f32,
    bullet: bool,
    height: f32,
}

#[derive(Default)]
pub struct UpdateUi {
    pub open: bool,
    release: Option<Release>,
    phase: Option<Phase>,
    rows: Vec<Row>,
    scroll: f32,
    selected: usize,
    job: Option<Arc<Install>>,
    /// a moment's line under the notes ("Cancelled. Nothing was changed.")
    note: Option<String>,
    /// the new Haiku.exe, for the app to start
    ready: Option<std::path::PathBuf>,
    opened_at: f64,
}

impl UpdateUi {
    pub fn show(&mut self, release: Release, now: f64) {
        log::info!("updater: the notice was opened: {} ({})", release.version, release.title);
        self.rows.clear();
        self.scroll = 0.0;
        self.selected = 0;
        self.note = None;
        self.phase = Some(Phase::Notes);
        self.release = Some(release);
        self.open = true;
        self.opened_at = now;
    }

    /// snapshot (FATES_UI_SNAPSHOT): the screen in a phase.
    pub(crate) fn snapshot(&mut self, release: Release, phase: &str) {
        self.show(release.clone(), -1.0);
        match phase {
            "scrolled" => self.scroll = 80.0,
            "skip" => self.selected = 2,
            "downloading" => {
                self.job = Some(Install::snapshot(release, Stage::Downloading, 0.42));
                self.phase = Some(Phase::Working);
            }
            "checking" => {
                self.job = Some(Install::snapshot(release, Stage::Verifying, 1.0));
                self.phase = Some(Phase::Working);
            }
            "failed" => {
                self.phase = Some(Phase::Failed { line: "Haiku.zip isn't signed by Haiku's release key, so it wasn't installed. Nothing was changed.".into(), page: true });
            }
            "offline" => {
                self.phase = Some(Phase::Failed { line: "There is no internet connection, or GitHub can't be reached. Nothing was changed.".into(), page: false });
            }
            _ => {}
        }
    }

    fn hide(&mut self) {
        self.open = false;
        self.phase = None;
        self.job = None;
    }

    /// the new Haiku.exe once installed, for the app to start.
    pub fn take_ready(&mut self) -> Option<std::path::PathBuf> {
        self.ready.take()
    }

    fn choices(&self) -> Vec<Choice> {
        match &self.phase {
            // a release without a signature is never installed by Haiku:
            // its page instead
            Some(Phase::Notes) if self.release.as_ref().is_some_and(|release| release.sig_url.is_empty()) => vec![Choice::Page, Choice::Later, Choice::Skip],
            Some(Phase::Notes) => vec![Choice::Install, Choice::Later, Choice::Skip],
            Some(Phase::Working) if self.job.as_ref().is_some_and(|job| job.stage() != Stage::Installing) => vec![Choice::Cancel],
            Some(Phase::Failed { page: true, .. }) => vec![Choice::OpenPage, Choice::Close],
            Some(Phase::Failed { .. }) => vec![Choice::TryAgain, Choice::Close],
            _ => Vec::new(),
        }
    }

    /// the notes laid out for the panel's width, once.
    fn layout(&mut self, canvas: &Canvas) {
        if !self.rows.is_empty() {
            return;
        }
        let Some(release) = &self.release else { return };
        let kit = canvas.kit;
        let mut rows = Vec::new();
        for line in haiku_update::notes_lines(&release.notes) {
            match line {
                Line::Heading(text) => {
                    for piece in kit.wrap(&plain(&text), NOTES_W, 0.86) {
                        rows.push(Row { text: piece, scale: 0.86, colour: TEXT_DARK, x: 0.0, bullet: false, height: 14.0 });
                    }
                }
                Line::Text(text) => {
                    for piece in kit.wrap(&plain(&text), NOTES_W, 0.7) {
                        rows.push(Row { text: piece, scale: 0.7, colour: TEXT_DARK, x: 0.0, bullet: false, height: 11.5 });
                    }
                }
                Line::Bullet(text) => {
                    let deeper = text.starts_with("  ");
                    let indent = if deeper { 18.0 } else { 9.0 };
                    for (i, piece) in kit.wrap(&plain(text.trim_start()), NOTES_W - indent, 0.7).into_iter().enumerate() {
                        rows.push(Row { text: piece, scale: 0.7, colour: TEXT_DARK, x: indent, bullet: i == 0, height: 11.5 });
                    }
                }
                Line::Blank => rows.push(Row { text: String::new(), scale: 0.7, colour: TEXT_DARK, x: 0.0, bullet: false, height: 5.0 }),
            }
        }
        if rows.is_empty() {
            rows.push(Row { text: "This release has no notes.".to_owned(), scale: 0.7, colour: TEXT_MUTED, x: 0.0, bullet: false, height: 11.5 });
        }
        self.rows = rows;
    }

    fn notes_height(&self) -> f32 {
        self.rows.iter().map(|row| row.height).sum()
    }

    /// what was chosen this frame.
    fn take_input(&mut self, canvas: &Canvas, input: &egui::InputState, nav: &Nav) -> Option<Choice> {
        let choices = self.choices();
        let mut chosen = None;
        // the notes scroll with up and down, the pages, the wheel
        if self.phase == Some(Phase::Notes) {
            let most = (self.notes_height() - NOTES_H).max(0.0);
            if nav.up {
                self.scroll -= 23.0;
            }
            if nav.down {
                self.scroll += 23.0;
            }
            if nav.page_up {
                self.scroll -= NOTES_H - 12.0;
            }
            if nav.page_down {
                self.scroll += NOTES_H - 12.0;
            }
            let wheel = input.smooth_scroll_delta.y;
            if wheel != 0.0 {
                self.scroll -= wheel / canvas.unit;
            }
            self.scroll = self.scroll.clamp(0.0, most);
        }
        if !choices.is_empty() {
            self.selected = self.selected.min(choices.len() - 1);
            if nav.left {
                self.selected = (self.selected + choices.len() - 1) % choices.len();
            }
            if nav.right {
                self.selected = (self.selected + 1) % choices.len();
            }
            // the pointer: on a choice selects it, a click chooses it
            if let Some((px, py)) = input.pointer.hover_pos().map(|p| canvas.unit_of(p)) {
                for (i, (x, w)) in choice_places(canvas, &choices).into_iter().enumerate() {
                    if (x..x + w).contains(&px) && (CHOICES_Y..CHOICES_Y + ROW).contains(&py) {
                        if input.pointer.is_moving() {
                            self.selected = i;
                        }
                        if input.pointer.primary_clicked() {
                            self.selected = i;
                            chosen = Some(choices[i]);
                        }
                    }
                }
            }
            if nav.confirm {
                chosen = Some(choices[self.selected]);
            }
        }
        // back: Later on the notes, Cancel while downloading, Close after a
        // failure; right click the same
        if nav.back || nav.close || input.pointer.secondary_clicked() {
            chosen = match &self.phase {
                Some(Phase::Notes) => Some(Choice::Later),
                Some(Phase::Failed { .. }) => Some(Choice::Close),
                Some(Phase::Working) if choices.contains(&Choice::Cancel) => Some(Choice::Cancel),
                _ => chosen,
            };
        }
        chosen
    }

    fn act(&mut self, choice: Choice) {
        let Some(release) = self.release.clone() else { return };
        match choice {
            Choice::Install | Choice::TryAgain => {
                log::info!("updater: Install chosen for {}", release.version);
                self.note = None;
                self.selected = 0;
                self.job = Some(haiku_update::install(release));
                self.phase = Some(Phase::Working);
            }
            Choice::Later => {
                log::info!("updater: Later chosen for {}", release.version);
                haiku_update::later();
                self.hide();
            }
            Choice::Skip => {
                haiku_update::skip(&release.version);
                self.hide();
            }
            Choice::Cancel => {
                if let Some(job) = &self.job {
                    job.cancel();
                }
            }
            Choice::OpenPage | Choice::Page => {
                haiku_update::open_page(&release.page);
                self.hide();
            }
            Choice::Close => self.hide(),
        }
    }

    /// the job's news.
    fn poll(&mut self) {
        if self.phase != Some(Phase::Working) {
            return;
        }
        let Some(job) = &self.job else { return };
        match job.stage() {
            Stage::Ready(exe) => {
                self.phase = Some(Phase::Restarting);
                self.ready = Some(exe);
            }
            Stage::Failed { line, page } => {
                self.selected = 0;
                self.phase = Some(Phase::Failed { line, page });
            }
            Stage::Cancelled => {
                self.selected = 0;
                self.note = Some("Cancelled. Nothing was changed.".to_owned());
                self.phase = Some(Phase::Notes);
                self.job = None;
            }
            _ => {}
        }
    }

    /// a frame of the screen over the game: drawn, and the player's choice
    /// taken.
    pub fn frame(&mut self, canvas: &Canvas, input: &egui::InputState, nav: &Nav, prompts: &Prompts, screen: egui::Rect) {
        self.poll();
        self.layout(canvas);
        // the first frame's input opened it
        if input.time - self.opened_at > 0.05 {
            if let Some(choice) = self.take_input(canvas, input, nav) {
                self.act(choice);
            }
        }
        if self.open {
            self.draw(canvas, input.time, prompts, screen);
        }
    }

    fn draw(&self, canvas: &Canvas, now: f64, prompts: &Prompts, screen: egui::Rect) {
        let Some(release) = &self.release else { return };
        let white = Color32::WHITE;
        let kit = canvas.kit;
        canvas.painter.rect_filled(screen, 0.0, Color32::from_black_alpha(150));
        canvas.image(Art::Wallpaper, 0.0, HEADER - 2.0, WIDTH, FOOTER_Y - HEADER + 4.0, Color32::from_rgb(210, 220, 215));
        canvas.image(Art::Header, 0.0, 0.0, WIDTH, HEADER, Color32::from_rgb(200, 120, 100));
        canvas.image(Art::Header, 0.0, FOOTER_Y, WIDTH, HEADER, white);
        let title = match &self.phase {
            Some(Phase::Working | Phase::Restarting) => "Updating",
            _ => "Update",
        };
        canvas.text(10.0, 9.0, title, 1.0, TEXT_GOLD);
        let entries: Vec<(Prompt, &str)> = match &self.phase {
            Some(Phase::Notes) => vec![(Prompt::Confirm, "Select"), (Prompt::Back, "Later")],
            Some(Phase::Working) if !self.choices().is_empty() => vec![(Prompt::Back, "Cancel")],
            Some(Phase::Failed { .. }) => vec![(Prompt::Confirm, "Select"), (Prompt::Back, "Close")],
            _ => Vec::new(),
        };
        if !entries.is_empty() {
            let width = super::prompts_width(canvas, prompts, &entries);
            let px = WIDTH - 8.0 - width - 10.0;
            canvas.nine(Art::PillPrompt, px, 8.0, width + 10.0, 18.0, [7.0, 7.0, 7.0, 7.0], 1.0, white);
            super::draw_prompts(canvas, prompts, px + 5.0, 9.0, &entries);
        }

        canvas.nine(Art::Panel, PANEL_X, PANEL_Y, PANEL_W, PANEL_H, [60.0, 40.0, 60.0, 40.0], 1.0, white);
        let mid = WIDTH / 2.0;
        let ours = haiku_update::version().unwrap_or("?");
        let help: String = match &self.phase {
            Some(Phase::Notes) => {
                let heading = plain(&format!("{} (you have Haiku {ours})", release.title));
                canvas.text_centered(mid, PANEL_Y + 11.0, &heading, fit(kit, &heading, 0.9, PANEL_W - 110.0), TEXT_DARK);
                self.draw_notes(canvas);
                match self.choices().get(self.selected) {
                    Some(Choice::Install) => format!(
                        "Downloads Haiku {} ({:.1} MB) from GitHub, checks it and restarts Haiku. Saves, settings and mods stay as they are.",
                        release.version,
                        release.zip_size as f64 / 1e6
                    ),
                    Some(Choice::Skip) => format!("No more notices for Haiku {}. You'll hear of the version after it.", release.version),
                    Some(Choice::Page) => "This release isn't signed, so Haiku won't install it itself. Opens its page in your browser.".to_owned(),
                    _ => "Reminds you the next time Haiku starts.".to_owned(),
                }
            }
            Some(Phase::Working) | Some(Phase::Restarting) => {
                self.draw_progress(canvas, now, &release.version);
                if self.phase == Some(Phase::Restarting) {
                    "Haiku starts again by itself.".to_owned()
                } else if self.choices().is_empty() {
                    "Putting the new files in place. This takes a moment.".to_owned()
                } else {
                    "Cancel stops the update. Nothing changes until it is installed.".to_owned()
                }
            }
            Some(Phase::Failed { line, page }) => {
                let title = "The update wasn't installed";
                canvas.text_centered(mid, PANEL_Y + 11.0, title, fit(kit, title, 0.95, PANEL_W - 110.0), TEXT_DARK);
                for (i, piece) in kit.wrap_even(&plain(line), PANEL_W - 80.0, 0.76).iter().take(4).enumerate() {
                    canvas.text_centered(mid, PANEL_Y + 44.0 + i as f32 * 12.5, piece, 0.76, TEXT_DARK);
                }
                if *page {
                    format!("Opens {} in your browser. Your current Haiku keeps working.", short_page(&release.page))
                } else {
                    "Your current Haiku keeps working; nothing was changed.".to_owned()
                }
            }
            None => String::new(),
        };
        self.draw_choices(canvas, now);
        // the help line in the bottom bar
        let lines = kit.wrap_even(&help, WIDTH - 40.0, 0.85);
        let (lines, scale) = if lines.len() > 2 { (kit.wrap_even(&help, WIDTH - 30.0, 0.75), 0.75) } else { (lines, 0.85) };
        let line_h = 12.0 * scale / 0.85;
        let first = FOOTER_Y + (HEADER - lines.len() as f32 * line_h) / 2.0 - 1.0;
        for (i, line) in lines.iter().enumerate() {
            canvas.text_centered(mid, first + i as f32 * line_h, line, scale, TEXT_CREAM);
        }
    }

    fn draw_notes(&self, canvas: &Canvas) {
        let clip = canvas.rect(NOTES_X - 2.0, NOTES_Y, NOTES_W + 4.0, NOTES_H);
        let notes = Canvas { painter: canvas.painter.with_clip_rect(clip), ..canvas.clone() };
        let mut y = NOTES_Y - self.scroll;
        for row in &self.rows {
            // whole lines only: none cut at the window's edges
            if y >= NOTES_Y - 0.5 && y + row.height <= NOTES_Y + NOTES_H + 0.5 {
                if row.bullet {
                    notes.text(NOTES_X + row.x - 7.0, y, "\u{b7}", row.scale * 1.2, row.colour);
                }
                if !row.text.is_empty() {
                    notes.text(NOTES_X + row.x, y, &row.text, row.scale, row.colour);
                }
            }
            y += row.height;
        }
        // the scroll arrows, while there is more above or below
        let most = (self.notes_height() - NOTES_H).max(0.0);
        let x = NOTES_X + NOTES_W + 8.0;
        if self.scroll > 0.5 {
            canvas.image(Art::ScrollUp, x, NOTES_Y, 12.0, 8.0, Color32::WHITE);
        }
        if self.scroll < most - 0.5 {
            canvas.image(Art::ScrollDown, x, NOTES_Y + NOTES_H - 8.0, 12.0, 8.0, Color32::WHITE);
        }
        if let Some(note) = &self.note {
            canvas.text_centered(WIDTH / 2.0, NOTES_Y + NOTES_H + 1.0, note, 0.66, TEXT_MUTED);
        }
    }

    fn draw_progress(&self, canvas: &Canvas, now: f64, version: &str) {
        let mid = WIDTH / 2.0;
        let kit = canvas.kit;
        let title = format!("Updating to Haiku {version}");
        canvas.text_centered(mid, PANEL_Y + 11.0, &title, fit(kit, &title, 0.95, PANEL_W - 110.0), TEXT_DARK);
        let stage = self.job.as_ref().map(|job| job.stage());
        let current = match (&self.phase, &stage) {
            (Some(Phase::Restarting), _) | (_, Some(Stage::Ready(_))) => 4,
            (_, Some(Stage::Installing)) => 3,
            (_, Some(Stage::Unpacking)) => 2,
            (_, Some(Stage::Verifying)) => 1,
            _ => 0,
        };
        let steps = ["Downloading from GitHub", "Checking its SHA-256 and signature", "Unpacking", "Installing"];
        let (left, right) = (PANEL_X + 40.0, PANEL_X + PANEL_W - 40.0);
        let top = PANEL_Y + 32.0;
        let bob = ((now * 6.0).sin() * 1.5) as f32;
        let white = Color32::WHITE;
        for (i, name) in steps.iter().enumerate() {
            let y = top + i as f32 * STEP_ROW;
            let here = i == current;
            if here {
                canvas.three(Art::Highlight, left, y - 1.0, right - left, STEP_ROW + 1.0, [10.0, 10.0], white);
                canvas.sprite(Art::Hand, left - 18.0 + bob, y + 1.5, white);
            } else if i + 1 < steps.len() {
                canvas.fill(left + 4.0, y + STEP_ROW - 0.5, right - left - 8.0, 0.6, Color32::from_rgba_unmultiplied(70, 110, 110, 70));
            }
            let colour = if here { TEXT_CREAM } else if i < current { TEXT_DARK } else { TEXT_MUTED };
            canvas.text(left + 10.0, y + 2.0, name, 0.8, colour);
            let state = if i < current { "Done".to_owned() } else if here { ".".repeat(((now * 2.0) as usize) % 3 + 1) } else { String::new() };
            canvas.text_right(right - 10.0, y + 2.5, &state, 0.74, colour);
        }
        // the bar: the game's pill as its track, the red bar as its fill
        let fraction = match current {
            0 => self.job.as_ref().map_or(0.0, |job| job.fraction()),
            _ => 1.0,
        };
        let bar_y = top + steps.len() as f32 * STEP_ROW + 8.0;
        let (bar_x, bar_w) = (left + 4.0, right - left - 44.0);
        canvas.nine(Art::PillLabel, bar_x, bar_y, bar_w, 10.0, [8.0, 8.0, 8.0, 8.0], 0.5, white);
        let filled = (bar_w - 4.0) * fraction.clamp(0.0, 1.0);
        if filled > 2.0 {
            canvas.three(Art::Highlight, bar_x + 2.0, bar_y + 1.5, filled, 7.0, [3.0, 3.0], white);
        }
        canvas.text_right(right - 4.0, bar_y - 2.0, &format!("{:.0}%", fraction * 100.0), 0.76, TEXT_DARK);
        if let (Some(job), Some(release)) = (&self.job, &self.release) {
            let size = release.zip_size as f64 / 1e6;
            let detail = if current == 0 { format!("{:.1} of {size:.1} MB", job.fraction() as f64 * size) } else { format!("{size:.1} MB, from GitHub") };
            canvas.text(bar_x, bar_y + 12.0, &detail, 0.64, TEXT_DARK);
        }
    }

    fn draw_choices(&self, canvas: &Canvas, now: f64) {
        let choices = self.choices();
        let bob = ((now * 6.0).sin() * 1.5) as f32;
        let white = Color32::WHITE;
        let scale = choice_scale(canvas, &choices);
        for (i, (x, w)) in choice_places(canvas, &choices).into_iter().enumerate() {
            let here = i == self.selected;
            if here {
                canvas.three(Art::Highlight, x, CHOICES_Y - 1.0, w, ROW + 1.0, [10.0, 10.0], white);
                canvas.sprite(Art::Hand, x - 16.0 + bob, CHOICES_Y + 2.5, white);
            } else {
                canvas.fill(x + 4.0, CHOICES_Y + ROW - 0.5, w - 8.0, 0.6, Color32::from_rgba_unmultiplied(70, 110, 110, 90));
            }
            canvas.text_centered(x + w / 2.0, CHOICES_Y + 2.0 + (0.9 - scale) * 6.0, choices[i].label(), scale, if here { TEXT_CREAM } else { TEXT_DARK });
        }
    }
}

/// the choices' text scale: 0.9, or smaller so that the row stays inside
/// the panel.
fn choice_scale(canvas: &Canvas, choices: &[Choice]) -> f32 {
    let room = PANEL_W - 70.0 - 22.0 * choices.len().saturating_sub(1) as f32 - 36.0 * choices.len() as f32;
    let text: f32 = choices.iter().map(|choice| canvas.kit.measure(choice.label(), 0.9)).sum();
    if text > room { 0.9 * room / text } else { 0.9 }
}

/// where each choice is: its x and width, in a row centred in the panel.
fn choice_places(canvas: &Canvas, choices: &[Choice]) -> Vec<(f32, f32)> {
    let gap = 22.0;
    let scale = choice_scale(canvas, choices);
    let widths: Vec<f32> = choices.iter().map(|choice| (canvas.kit.measure(choice.label(), scale) + 36.0).max(86.0)).collect();
    let total: f32 = widths.iter().sum::<f32>() + gap * widths.len().saturating_sub(1) as f32;
    let mut x = (WIDTH - total) / 2.0;
    widths
        .into_iter()
        .map(|w| {
            let place = (x, w);
            x += w + gap;
            place
        })
        .collect()
}

/// a title's scale: as asked, or smaller to fit width.
fn fit(kit: &super::kit::Kit, text: &str, scale: f32, width: f32) -> f32 {
    scale.min(scale * width / kit.measure(text, scale).max(1.0))
}

/// text in the letters the game's font has: typographic quotes, dashes and
/// the ellipsis as plain ones.
fn plain(text: &str) -> String {
    text.replace(['\u{2018}', '\u{2019}'], "'")
        .replace(['\u{201c}', '\u{201d}'], "\"")
        .replace('\u{2013}', "-")
        .replace('\u{2026}', "...")
        .replace('\u{2022}', "\u{b7}")
        .replace('\u{2192}', "->")
}

fn short_page(url: &str) -> String {
    url.trim_start_matches("https://").trim_end_matches('/').to_owned()
}

/// the notice in the window's lower left corner: "Haiku 1.4 is available"
/// with U (keyboard) or Y (controller); whether it was asked for (the key,
/// the button, a click).
pub fn notice(base: &Canvas, input: &egui::InputState, nav: &Nav, prompts: &Prompts, screen: egui::Rect, release: &Release) -> (bool, bool) {
    let canvas = Canvas { unit: base.unit * 0.9, ..base.clone() };
    let text = format!("Haiku {} is available", release.version);
    let keyboard = prompts.set == crate::controls::GlyphSet::Keyboard;
    let key_w = if keyboard { canvas.kit.measure("U", 0.62) + 8.0 } else { 13.0 };
    let width = canvas.kit.measure(&text, 0.85) + key_w + 18.0;
    let edge = 6.0;
    let x = edge;
    let y = screen.height() / canvas.unit - edge - 18.0;
    let over = input.pointer.hover_pos().is_some_and(|p| {
        let (px, py) = canvas.unit_of(p);
        (x..x + width).contains(&px) && (y..y + 17.0).contains(&py)
    });
    // a gentle pulse, so it is seen without being in the way
    let glow = 0.85 + 0.15 * ((input.time * 2.0).sin() as f32 * 0.5 + 0.5);
    canvas.nine(if over { Art::PillRed } else { Art::PillPrompt }, x, y, width, 17.0, [7.0, 7.0, 7.0, 7.0], if over { 0.6 } else { 1.0 }, Color32::from_white_alpha((255.0 * glow) as u8));
    let pen = x + 6.0;
    let used = if keyboard { super::draw_key(&canvas, pen, y + 0.5, "U") } else { super::draw_prompt(&canvas, prompts, pen, y + 0.5, Prompt::Extra2) };
    canvas.text(pen + used + 4.0, y + 1.5, &text, 0.85, TEXT_CREAM);
    let asked = (over && input.pointer.primary_clicked()) || input.key_pressed(egui::Key::U) || nav.extra2;
    (asked, over)
}

#[cfg(test)]
mod tests {
    #[test]
    fn typographic_letters_become_the_fonts() {
        assert_eq!(super::plain("Haiku\u{2019}s \u{201c}update\u{201d} \u{2013} now\u{2026}"), "Haiku's \"update\" - now...");
    }
}
