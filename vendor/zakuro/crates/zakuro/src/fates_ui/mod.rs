//! ui: the PC port's own interface, drawn entirely with Fire Emblem Fates'
//! art and font so that nothing in it looks like anything but the game: the
//! options menu, the naming keyboard, message boxes, notices, the frame
//! counter. With FATES_ORIGINAL_UI set (always, in fates-pc) this replaces
//! every egui window the generic frontend would show.
//!
//! Controllers come in through the input agent's API: the pads' console
//! buttons after bindings and `pc.button_layout` (so A confirms and B goes
//! back on any pad), the Options action (Guide/Home/PS), and the glyph set
//! (`pc.glyphs`, Auto = the device used last) whose button pictures, the
//! game's own art redrawn per family in assets/glyphs, the prompts show.

mod app;
pub(crate) use app::filter_input;
pub mod codemods; // codemods: compiling code mods, its dialogs
pub mod keyboard;
pub mod ja; // jpprep: Haiku's menus in Japanese on the Japanese game
pub mod kit;
pub mod mods; // mods: the Mods page
pub mod options;
pub mod qol; // qol: the boon/bane panel
pub mod growth; // info-ui: the growth rates on the lower screen
pub mod cheats; // cheats: the Cheats tab
pub mod online; // online: the host's code and the connecting line
pub mod pregame; // updscreen: the game's code being built, before the game starts
pub mod pulse; // pulse: Divine Pulse's rewind screen
pub mod states; // pulse: the Cheats tab's Save States
pub mod tones; // tones: Options > Gameplay > Speaking Style
pub mod update; // updater: the update's notice on the Main Menu and its screen
pub mod script;
pub mod snapshot;

use egui::Color32;
use zakuro_core::services::hid::PadState;

use kit::{Art, Canvas, Kit, TEXT_CREAM, TEXT_DARK};
use options::{Effect, Env, OptionsMenu};

use crate::controls::GlyphSet;
use crate::settings::Settings;

/// whether this build shows the game's own interface rather than Zakuro's.
pub fn enabled() -> bool {
    std::env::var_os("FATES_ORIGINAL_UI").is_some()
}

/// a frame's worth of moving through a menu, from the keys, the
/// controllers, whatever pressed it.
#[derive(Default, Clone, Debug)]
pub struct Nav {
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
    pub confirm: bool,
    pub back: bool,
    pub prev_tab: bool,
    pub next_tab: bool,
    pub page_up: bool,
    pub page_down: bool,
    /// Start, or the open/close combination: leaves the menu at once.
    pub close: bool,
    /// the console's Start, X and Y, for the keyboard.
    pub start: bool,
    pub extra: bool,
    pub extra2: bool,
}

/// a button a prompt names, by what it does: the console's A, B, L, R,
/// Start, X and Y.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Prompt {
    Confirm,
    Back,
    PrevTab,
    NextTab,
    Start,
    Extra,
    Extra2,
}

/// how prompts are drawn: which glyph set.
#[derive(Clone, Copy, Debug)]
pub struct Prompts {
    pub set: GlyphSet,
}

impl Prompts {
    /// the key a prompt names on the keyboard, in the menus.
    fn key(prompt: Prompt) -> &'static str {
        match prompt {
            Prompt::Confirm => "Enter",
            Prompt::Back => "Esc",
            Prompt::PrevTab => "Q",
            Prompt::NextTab => "E",
            Prompt::Start => "Enter",
            Prompt::Extra => "Del",
            Prompt::Extra2 => "Space",
        }
    }

    /// the slot of the glyph atlas the console button is drawn in.
    fn slot(prompt: Prompt) -> u8 {
        match prompt {
            Prompt::Confirm => 0,
            Prompt::Back => 1,
            Prompt::Extra => 2,
            Prompt::Extra2 => 3,
            Prompt::PrevTab => 4,
            Prompt::NextTab => 5,
            Prompt::Start => 6,
        }
    }
}

/// the width a prompt's picture takes, in units.
fn glyph_width(canvas: &Canvas, prompts: &Prompts, prompt: Prompt) -> f32 {
    match (prompts.set, prompt) {
        (GlyphSet::Keyboard, _) => canvas.kit.measure(Prompts::key(prompt), 0.62) + 8.0,
        (_, Prompt::Start) => 21.0,
        _ => 13.0,
    }
}

/// draws a prompt's button picture with its top at y on a text line,
/// returning its width.
pub fn draw_prompt(canvas: &Canvas, prompts: &Prompts, x: f32, y: f32, prompt: Prompt) -> f32 {
    let white = Color32::WHITE;
    let top = y + 1.0;
    let width = glyph_width(canvas, prompts, prompt);
    if prompts.set == GlyphSet::Keyboard {
        let name = Prompts::key(prompt);
        canvas.nine(Art::PillLabel, x, top, width, 13.0, [8.0, 8.0, 8.0, 8.0], 0.5, white);
        canvas.text_centered(x + width / 2.0, top + 1.5, name, 0.62, TEXT_CREAM);
        return width;
    }
    let set = match prompts.set {
        GlyphSet::Xbox => 1,
        GlyphSet::PlayStation => 2,
        _ => 0,
    };
    canvas.image(Art::Glyph(set, Prompts::slot(prompt)), x, top, width, 13.0, white);
    width
}

/// qol4: a key's name in the keyboard prompts' pill, as draw_prompt draws
/// one, for a key that is not a fixed prompt's.
fn draw_key(canvas: &Canvas, x: f32, y: f32, name: &str) -> f32 {
    let top = y + 1.0;
    let width = canvas.kit.measure(name, 0.62) + 8.0;
    canvas.nine(Art::PillLabel, x, top, width, 13.0, [8.0, 8.0, 8.0, 8.0], 0.5, Color32::WHITE);
    canvas.text_centered(x + width / 2.0, top + 1.5, name, 0.62, TEXT_CREAM);
    width
}

/// a row of prompts, each its picture and what it does.
pub fn draw_prompts(canvas: &Canvas, prompts: &Prompts, x: f32, y: f32, entries: &[(Prompt, &str)]) -> f32 {
    let mut pen = x;
    for (index, (prompt, label)) in entries.iter().enumerate() {
        if index > 0 {
            pen += 6.0;
        }
        pen += draw_prompt(canvas, prompts, pen, y, *prompt) + 2.0;
        pen += canvas.text(pen, y + 1.0, label, 0.82, TEXT_CREAM);
    }
    pen - x
}

pub fn prompts_width(canvas: &Canvas, prompts: &Prompts, entries: &[(Prompt, &str)]) -> f32 {
    entries
        .iter()
        .enumerate()
        .map(|(index, (prompt, label))| (if index > 0 { 6.0 } else { 0.0 }) + glyph_width(canvas, prompts, *prompt) + 2.0 + canvas.kit.measure(label, 0.82))
        .sum()
}

/// a controller held in a direction repeats it, as the game's menus do.
#[derive(Default)]
struct Repeat {
    held: (i32, i32),
    next: f64,
}

/// what the game shows that the interface draws over it this frame.
#[derive(Default)]
pub struct View {
    pub game: bool,
    pub fps: Option<f32>,
    pub fast: bool,
    pub corner: Option<String>,
    pub toast: Option<String>,
    pub message: Option<String>,
    pub keyboard: Option<zakuro_core::services::keyboard::Request>,
    /// the pads' console buttons held (bindings and layout applied).
    pub pad_held: PadState,
    /// the pads' circle pad, the left stick past the dead zone.
    pub pad_circle: [f32; 2],
    /// the Options action was pressed (Guide/Home/PS).
    pub options_action: bool,
    /// the button pictures to show.
    pub glyphs: GlyphSet,
    /// a binding is waiting for a key or button.
    pub rebinding: bool,
    /// qol: the boon/bane panel while the avatar's menu shows, and where
    /// the top screen is, in points.
    pub chara: Option<crate::qol::CharaPanel>,
    /// qol: Skip Story's cover over the game, 0 none to 1 black.
    pub cover: f32,
    pub top_screen: Option<egui::Rect>,
    /// info-ui: the growth button and view while the lower screen shows
    /// the unit status, and where the lower screen is, in points.
    pub growth: Option<growth::Panel>,
    pub bottom_screen: Option<egui::Rect>,
    /// online: the host's code, and the line while a session starts
    pub online: Option<crate::online_ui::HostPanel>,
    pub online_status: Option<String>,
}

/// what the interface asks of the app after a frame.
#[derive(Default)]
pub struct Output {
    pub effects: Vec<Effect>,
    pub keyboard: Option<(String, usize)>,
    pub message_closed: bool,
    pub quit: bool,
}

pub struct FatesUi {
    kit: Option<Kit>,
    failed: bool,
    pub options: OptionsMenu,
    pub keyboard: keyboard::Keyboard,
    /// the window has the keyboard, for muting in the background.
    pub focused: bool,
    pub env: Env,
    repeat: Repeat,
    /// when the pointer last moved, for the corner button.
    pointer_moved: f64,
    last_pointer: Option<egui::Pos2>,
    /// the pointer is over something of the interface's.
    pub wants_pointer: bool,
    /// the game gets no buttons until all are let go, after a menu closes,
    /// so the button that closed it does not reach the game.
    pub swallow: bool,
    /// a binding was waiting last frame.
    was_rebinding: bool,
    /// the menu just opened: the app lists the monitors and sound devices.
    pub refresh_env: bool,
    /// the pads' console buttons and circle pad, from App::poll_gamepads,
    /// and those held at the last frame the interface ran.
    pub pad_held: PadState,
    pub pad_circle: [f32; 2],
    prev_pad: PadState,
    /// the Options action, waiting for the interface to take it.
    pub options_action: bool,
    /// a test script driving the interface, FATES_UI_SCRIPT.
    pub script: Option<script::Script>,
    /// where the options menu was last drawn and its unit, in points.
    pub menu_at: (egui::Pos2, f32),
    /// bottom2: the floating bottom screen shown while its options change,
    /// in points: the menu steps aside from it and leaves it undimmed.
    pub keep_clear: Option<egui::Rect>,
    /// mods: the mods folder's mods, and the game's Main Menu asking for
    /// the Mods page.
    pub mods: crate::haiku_mods::ModList,
    pub open_mods: bool,
    /// qol3: the game's Main Menu asked for the Exit to Desktop question.
    pub open_exit: bool,
    /// qol4: the game's Main Menu asked for the options; the options
    /// menu's key, when it is not Escape, was pressed.
    pub open_options: bool,
    pub options_key: bool,
    /// codemods: compiling code mods, and the game's code.
    pub code: codemods::CodeModsState,
    /// pulse: the rewind screen was up last frame
    pulse_open: bool,
    /// updater: the update's screen, and whether the check was started
    pub update: update::UpdateUi,
    pub update_started: bool,
}

impl Default for FatesUi {
    fn default() -> Self {
        FatesUi {
            kit: None,
            failed: false,
            options: OptionsMenu::default(),
            keyboard: keyboard::Keyboard::default(),
            focused: true,
            env: Env::default(),
            repeat: Repeat::default(),
            pointer_moved: -10.0,
            last_pointer: None,
            wants_pointer: false,
            swallow: false,
            was_rebinding: false,
            refresh_env: false,
            pad_held: PadState::empty(),
            pad_circle: [0.0; 2],
            prev_pad: PadState::empty(),
            options_action: false,
            script: script::Script::load(),
            menu_at: (egui::Pos2::ZERO, 1.0),
            keep_clear: None,
            mods: crate::haiku_mods::ModList::default(),
            open_mods: false,
            open_exit: false, // qol3
            open_options: false, // qol4
            options_key: false,
            code: codemods::CodeModsState::default(),
            pulse_open: false, // pulse
            update: update::UpdateUi::default(), // updater
            update_started: false,
        }
    }
}

/// the unit, points per 3DS pixel, the menus are drawn at in a window of
/// size points, honouring the menu size setting while keeping them on it.
pub fn unit_for(size: egui::Vec2, ui_scale: f32) -> f32 {
    let base = size.y / 240.0 * 0.62 * ui_scale.clamp(0.5, 3.0);
    base.min(size.y * 0.96 / options::HEIGHT).min(size.x * 0.97 / options::WIDTH).max(0.5)
}

impl FatesUi {
    /// whether the game should stop for the interface.
    pub fn pauses(&self, message: bool) -> bool {
        self.modal() || message
    }

    /// whether the keyboard's keys are for the interface, not the game.
    pub fn takes_keys(&self) -> bool {
        self.modal()
    }

    /// updater: whether a screen of the interface's is up over the game
    /// (the options menu, the update's screen): it takes every input.
    pub fn modal(&self) -> bool {
        self.options.open || self.update.open
    }

    /// the art, loaded on first use; none when assets/ is missing.
    fn kit(&mut self) -> Option<&mut Kit> {
        if self.kit.is_none() && !self.failed {
            match Kit::load() {
                Ok(kit) => self.kit = Some(kit),
                Err(error) => {
                    log::error!("the Fates interface art could not be loaded, {error}");
                    self.failed = true;
                }
            }
        }
        self.kit.as_mut()
    }

    /// reads the keys and the controllers into moves for the menus. The
    /// keyboard's come from egui (arrows/WASD, Enter/Space, Esc/Backspace,
    /// Q/E/Tab); the pads' from their console buttons, so A confirms and B
    /// goes back whatever the controller and layout.
    fn nav(&mut self, input: &egui::InputState, view: &View, typing: bool) -> Nav {
        use egui::Key;
        let mut nav = Nav::default();
        let key = |k: Key| input.key_pressed(k);
        if !typing {
            nav.up = key(Key::ArrowUp) || key(Key::W);
            nav.down = key(Key::ArrowDown) || key(Key::S);
            nav.left = key(Key::ArrowLeft) || key(Key::A);
            nav.right = key(Key::ArrowRight) || key(Key::D);
            nav.confirm = key(Key::Enter) || key(Key::Space);
            nav.back = key(Key::Escape) || key(Key::Backspace);
            nav.prev_tab = key(Key::Q) || (key(Key::Tab) && input.modifiers.shift);
            nav.next_tab = key(Key::E) || (key(Key::Tab) && !input.modifiers.shift);
            nav.page_up = key(Key::PageUp);
            nav.page_down = key(Key::PageDown);
        } else {
            nav.up = key(Key::ArrowUp);
            nav.down = key(Key::ArrowDown);
            nav.left = key(Key::ArrowLeft);
            nav.right = key(Key::ArrowRight);
        }

        // the controllers: a direction repeats while held
        let now = input.time;
        let held = view.pad_held;
        let [cx, cy] = view.pad_circle;
        let x = if held.contains(PadState::LEFT) || cx < -0.5 {
            -1
        } else if held.contains(PadState::RIGHT) || cx > 0.5 {
            1
        } else {
            0
        };
        let y = if held.contains(PadState::DOWN) || cy < -0.5 {
            -1
        } else if held.contains(PadState::UP) || cy > 0.5 {
            1
        } else {
            0
        };
        let dir = (x, y);
        let fire = if dir != self.repeat.held {
            self.repeat.held = dir;
            self.repeat.next = now + 0.38;
            dir != (0, 0)
        } else if dir != (0, 0) && now >= self.repeat.next {
            self.repeat.next = now + 0.075;
            true
        } else {
            false
        };
        if fire {
            nav.up |= dir.1 > 0;
            nav.down |= dir.1 < 0;
            nav.left |= dir.0 < 0;
            nav.right |= dir.0 > 0;
        }
        let pressed = held & !self.prev_pad;
        let on = |button: PadState| pressed.contains(button);
        nav.confirm |= on(PadState::A);
        nav.back |= on(PadState::B);
        nav.prev_tab |= on(PadState::L);
        nav.next_tab |= on(PadState::R);
        nav.page_up |= on(PadState::ZL);
        nav.page_down |= on(PadState::ZR);
        nav.start |= on(PadState::START) && !held.contains(PadState::SELECT);
        nav.extra |= on(PadState::X);
        nav.extra2 |= on(PadState::Y);
        nav
    }

    /// whether the controllers asked to open or close the options: the
    /// Options action, or Start and Select together.
    fn pad_toggles(&self, view: &View) -> bool {
        let held = view.pad_held;
        let pressed = held & !self.prev_pad;
        let both = PadState::START | PadState::SELECT;
        view.options_action || (held.contains(both) && pressed.intersects(both))
    }

    /// runs the interface for a frame.
    pub fn run(&mut self, ctx: &egui::Context, view: &View, settings: &mut Settings) -> Output {
        let mut out = Output::default();
        let screen = ctx.content_rect();
        let ppp = ctx.pixels_per_point();
        let unit = unit_for(screen.size(), settings.pc.ui_scale);
        if self.kit().is_none() {
            return out;
        }
        // qol: the art at the larger of the menus' and the boon/bane
        // panel's scales while that shows
        let art_unit = match (&view.chara, view.top_screen, self.options.open) {
            (Some(_), Some(top), false) => unit.max(top.height() / 240.0),
            _ => unit,
        };
        if let Some(kit) = self.kit.as_mut() {
            kit.prepare(ctx, art_unit * ppp);
            // perf3: the boon/bane panel's scale made ahead, off this thread
            if let Some(top) = view.top_screen {
                kit.warm(unit.max(top.height() / 240.0) * ppp);
            }
        }
        let prompts = Prompts { set: view.glyphs };
        let input = ctx.input(|input| input.clone());
        let now = input.time;
        let pointer = input.pointer.hover_pos();
        if pointer.is_some() && pointer != self.last_pointer {
            if self.last_pointer.is_some() {
                self.pointer_moved = now;
            }
            self.last_pointer = pointer;
        }
        // a binding just took its key or button: this frame's input was it
        let rebinding_ended = self.was_rebinding && !view.rebinding;
        self.was_rebinding = view.rebinding;
        if !view.rebinding && self.options.rebinding.is_some() {
            self.options.rebinding = None;
        }
        let typing = view.keyboard.is_some();
        let mut nav = self.nav(&input, view, typing);
        if rebinding_ended || view.rebinding {
            nav = Nav::default();
        }
        let toggle = self.pad_toggles(view) && !view.rebinding && !rebinding_ended;
        // qol4: the options menu's key, when rebound from Escape, opens and
        // closes the menu as the controllers' Options does
        let options_key = std::mem::take(&mut self.options_key) && !view.rebinding && !rebinding_ended;
        let toggle = toggle || options_key;
        self.prev_pad = view.pad_held;

        let kit = self.kit.as_ref().expect("loaded above");
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("fates ui")));
        let top_painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("fates notices")));
        let base = Canvas { kit, painter: top_painter, origin: screen.min, unit, ppp, alpha: 1.0 };
        self.wants_pointer = false;

        // the notices over the game: the frame rate, fast forward, a
        // moment's notice, the corner line
        let small = Canvas { unit: unit * 0.8, ..base.clone() };
        let edge = 6.0;
        if let Some(fps) = view.fps.filter(|_| view.game) {
            pill(&small, edge, edge, &format!("{fps:.0} FPS"), 0.85);
        }
        if view.fast {
            let width = small.kit.measure("Fast Forward", 0.85) + 26.0;
            let x = screen.width() / small.unit - edge - width;
            pill(&small, x, edge, "Fast Forward", 0.85);
            small.image(Art::ArrowRight, x + width - 16.0, edge + 3.0, 5.0, 8.0, Color32::WHITE);
            small.image(Art::ArrowRight, x + width - 11.0, edge + 3.0, 5.0, 8.0, Color32::WHITE);
        }
        if let Some(text) = &view.toast {
            let width = small.kit.measure(text, 0.85) + 16.0;
            pill(&small, (screen.width() / small.unit - width) / 2.0, edge + 18.0, text, 0.85);
        }
        if let Some(text) = &view.corner {
            let width = small.kit.measure(text, 0.85) + 16.0;
            pill(&small, screen.width() / small.unit - edge - width, screen.height() / small.unit - edge - 16.0, text, 0.85);
        }

        // qol: Skip Story's cover: the game's screens black while it passes
        // a story event, under the menus and notices
        if view.cover > 0.0 {
            let layer = ctx.layer_painter(egui::LayerId::new(egui::Order::Background, egui::Id::new("qol cover")));
            layer.rect_filled(screen, 0.0, Color32::from_black_alpha((view.cover.clamp(0.0, 1.0) * 255.0) as u8));
        }

        // qol: the boon/bane panel beside the avatar's menu, at the top
        // screen's scale, under the options when they open
        if let (Some(panel), Some(top), false) = (&view.chara, view.top_screen, self.options.open) {
            let game_unit = top.height() / 240.0;
            let left = top.center().x - 200.0 * game_unit;
            let extra_right = (top.max.x - (left + 400.0 * game_unit)) / game_unit;
            let (x, y) = qol::place(extra_right);
            let canvas = Canvas { origin: egui::pos2(left, top.min.y), unit: game_unit, painter: painter.clone(), ..base.clone() };
            qol::draw(&canvas, x, y, panel);
        }

        // info-ui: the growth button and view over the lower screen, at its
        // scale, under the options
        if let (Some(panel), Some(bottom), false) = (&view.growth, view.bottom_screen, self.options.open) {
            let unit = bottom.height() / 240.0;
            let canvas = Canvas { origin: bottom.min, unit, painter: painter.clone(), ..base.clone() };
            let mut panel = panel.clone();
            panel.hover = ctx.input(|i| i.pointer.hover_pos()).is_some_and(|pos| {
                let (x, y) = canvas.unit_of(pos);
                growth::inside_button(x, y)
            });
            growth::draw(&canvas, &panel);
        }

        // online: the host's code over the game's wait, and the line while
        // a session starts
        if !self.options.open && view.keyboard.is_none() {
            if let Some(panel) = &view.online {
                let canvas = Canvas { painter: painter.clone(), ..base.clone() };
                let asked = online::draw(&canvas, &input, &nav, &prompts, panel, screen);
                if asked.copy {
                    ctx.copy_text(panel.code.clone());
                    crate::online_ui::mark_copied();
                }
                // online2: the host's rule for characters modified with cheats
                if asked.toggle {
                    settings.pc.online.allow_cheated_units = !settings.pc.online.allow_cheated_units;
                    log::info!("online: {}", online::rule_text(settings.pc.online.allow_cheated_units));
                    crate::online_ui::configure(&settings.pc.online);
                    out.effects.push(options::Effect::Apply);
                }
            } else if let Some(text) = &view.online_status {
                let canvas = Canvas { painter: painter.clone(), ..base.clone() };
                online::status(&canvas, text, screen, now);
            }
        }

        // the naming keyboard the game asked for
        if let Some(request) = &view.keyboard {
            let canvas = Canvas { painter: painter.clone(), ..base.clone() };
            out.keyboard = self.keyboard.frame(&canvas, &input, &nav, &prompts, request, screen);
            self.wants_pointer = true;
            return out;
        }
        self.keyboard.forget();

        // a message: the game's own message box, closed with confirm
        if let Some(text) = &view.message {
            let canvas = Canvas { painter: painter.clone(), ..base.clone() };
            if message_box(&canvas, &input, &nav, &prompts, text, screen) {
                out.message_closed = true;
                self.swallow = true;
            }
            self.wants_pointer = true;
            return out;
        }

        // pulse: Divine Pulse's rewind takes the input while it is up; the
        // button that closed it does not reach the game
        let pulse_open = crate::pulse::screen_open();
        if self.pulse_open && !pulse_open {
            self.swallow = true;
        }
        self.pulse_open = pulse_open;
        if pulse_open && !self.options.open {
            let canvas = Canvas { painter: painter.clone(), ..base.clone() };
            pulse::frame(&canvas, &input, &nav, &prompts, screen, view.top_screen);
            self.wants_pointer = true;
            return out;
        }

        // updater: the update's screen takes the input while it is up; its
        // notice shows on the game's Main Menu, under the options
        if self.update.open {
            let (width, height) = (update::WIDTH * unit, update::HEIGHT * unit);
            let snap = |v: f32| (v * ppp).round() / ppp;
            let origin = egui::pos2(snap(screen.center().x - width / 2.0), snap(screen.center().y - height / 2.0));
            let canvas = Canvas { painter: painter.clone(), origin, ..base.clone() };
            self.update.frame(&canvas, &input, &nav, &prompts, screen);
            self.wants_pointer = true;
            if !self.update.open {
                self.swallow = true;
            }
            return out;
        }
        if !self.options.open && view.game {
            if let Some(release) = crate::haiku_update::notice().filter(|_| crate::haiku_update::on_main_menu()) {
                let (asked, over) = update::notice(&base, &input, &nav, &prompts, screen, &release);
                self.wants_pointer |= over;
                if asked {
                    self.update.show(release, now);
                    self.swallow = true;
                    return out;
                }
            }
        }

        // the options, opened with Esc, Home, Start and Select, or the corner
        // qol4: Escape opens the menu while it is the options menu's key
        let esc_opens = settings.keys.options.contains(winit::keyboard::KeyCode::Escape);
        let esc = esc_opens && input.key_pressed(egui::Key::Escape) && !rebinding_ended && !view.rebinding;
        // mods: the Main Menu's Mods entry opens the Mods page alone
        if std::mem::take(&mut self.open_mods) && view.game {
            self.options.show_mods(now);
            self.refresh_env = true;
            return out;
        }
        // qol3: the Main Menu's Exit to Desktop asks the options' question alone
        if std::mem::take(&mut self.open_exit) && view.game {
            self.options.show_exit(now);
            return out;
        }
        // qol4: the Main Menu's Options opens the options menu
        if std::mem::take(&mut self.open_options) && view.game {
            self.options.show(now);
            self.refresh_env = true;
            return out;
        }
        if !self.options.open {
            let mut open = (esc || toggle) && view.game;
            // the corner button, shown a moment after the pointer moves
            let since = now - self.pointer_moved;
            if view.game && since < 3.0 {
                let fade = (1.0 - ((since - 2.4) / 0.6).clamp(0.0, 1.0)) as f32;
                let corner = Canvas { unit: unit * 0.8, alpha: fade, ..base.clone() };
                let label = "Options";
                let glyph = if prompts.set == GlyphSet::Keyboard { 0.0 } else { 21.0 };
                // qol4: the options menu's key, as bound
                let key_name = settings.keys.options.0.first().map_or_else(|| "Esc".to_owned(), |key| crate::controls::key_name(*key));
                let width = corner.kit.measure(label, 0.85) + 14.0 + glyph + if prompts.set == GlyphSet::Keyboard { corner.kit.measure(&key_name, 0.62) + 10.0 } else { 0.0 };
                let x = screen.width() / corner.unit - edge - width;
                let y = edge + if view.fast { 20.0 } else { 0.0 };
                let over = pointer.is_some_and(|p| {
                    let (px, py) = corner.unit_of(p);
                    (x..x + width).contains(&px) && (y..y + 17.0).contains(&py)
                });
                corner.nine(if over { Art::PillRed } else { Art::PillPrompt }, x, y, width, 17.0, [7.0, 7.0, 7.0, 7.0], if over { 0.6 } else { 1.0 }, Color32::WHITE);
                let pen = x + 6.0;
                let used = if prompts.set == GlyphSet::Keyboard {
                    draw_key(&corner, pen, y + 0.5, &key_name) // qol4: the bound key
                } else {
                    draw_prompt(&corner, &prompts, pen, y + 0.5, Prompt::Start)
                };
                corner.text(pen + used + 3.0, y + 1.5, label, 0.85, TEXT_CREAM);
                if over {
                    self.wants_pointer = true;
                    if input.pointer.primary_clicked() {
                        open = true;
                    }
                }
            }
            if open {
                self.options.show(now);
                self.refresh_env = true;
                // the key that opened it is not a move in it
                return out;
            }
            return out;
        }

        // the options menu itself, over the dimmed game
        let fade = self.options.fade(now);
        let width = options::WIDTH * unit;
        let height = options::HEIGHT * unit;
        let mut origin = egui::pos2(screen.center().x - width / 2.0, screen.center().y - height / 2.0);
        // bottom2: aside from the bottom screen being placed, as far as the
        // window lets it
        if let Some(clear) = self.keep_clear.filter(|_| self.options.previewing_bottom()) {
            let menu = egui::Rect::from_min_size(origin, egui::vec2(width, height));
            if menu.intersects(clear) {
                origin.x = if clear.center().x >= screen.center().x {
                    origin.x.min((clear.min.x - width).max(screen.min.x))
                } else {
                    origin.x.max(clear.max.x.min(screen.max.x - width))
                };
            }
        }
        let snap = |v: f32| (v * ppp).round() / ppp;
        let canvas = Canvas { kit, painter: painter.clone(), origin: egui::pos2(snap(origin.x), snap(origin.y)), unit, ppp, alpha: fade };
        self.menu_at = (canvas.origin, unit);
        // the game behind, dimmed as the game dims what is under its menus
        let dim = Color32::from_black_alpha((150.0 * fade) as u8);
        match self.keep_clear.filter(|_| self.options.previewing_bottom()).map(|clear| clear.intersect(screen)).filter(|clear| clear.is_positive()) {
            // bottom2: all but the bottom screen being placed
            Some(clear) => {
                painter.rect_filled(egui::Rect::from_min_max(screen.min, egui::pos2(screen.max.x, clear.min.y)), 0.0, dim);
                painter.rect_filled(egui::Rect::from_min_max(egui::pos2(screen.min.x, clear.max.y), screen.max), 0.0, dim);
                painter.rect_filled(egui::Rect::from_min_max(egui::pos2(screen.min.x, clear.min.y), egui::pos2(clear.min.x, clear.max.y)), 0.0, dim);
                painter.rect_filled(egui::Rect::from_min_max(egui::pos2(clear.max.x, clear.min.y), egui::pos2(screen.max.x, clear.max.y)), 0.0, dim);
            }
            None => {
                painter.rect_filled(screen, 0.0, dim);
            }
        }
        if toggle {
            nav.close = true;
        }
        if nav.start {
            nav.close = true;
        }
        let env = self.env.clone();
        out.effects = self.options.frame(&canvas, &input, &nav, &prompts, settings, &env);
        self.wants_pointer = true;
        if out.effects.contains(&Effect::Closed) {
            self.swallow = true;
        }
        out
    }
}

/// a line in the game's dark prompt pill.
fn pill(canvas: &Canvas, x: f32, y: f32, text: &str, scale: f32) {
    let width = canvas.kit.measure(text, scale) + 16.0;
    canvas.nine(Art::PillPrompt, x, y, width, 16.0, [7.0, 7.0, 7.0, 7.0], 1.0, Color32::WHITE);
    canvas.text(x + 8.0, y + 1.5, text, scale, TEXT_CREAM);
}

/// the game's message box, its checkered cream dialog with the text in
/// the game's dark brown and the confirm button's picture in the corner.
/// true once it is closed.
fn message_box(canvas: &Canvas, input: &egui::InputState, nav: &Nav, prompts: &Prompts, text: &str, screen: egui::Rect) -> bool {
    let scale = 1.0;
    let width = 300.0;
    let kit = canvas.kit;
    let lines = kit.wrap_even(text, width - 32.0, scale);
    let shown: Vec<&String> = lines.iter().take(12).collect();
    let height = shown.len() as f32 * kit::LINE + 30.0;
    let unit = canvas.unit;
    let x = (screen.width() / unit - width) / 2.0;
    let y = (screen.height() / unit - height) / 2.0;
    let canvas = Canvas { origin: screen.min, ..canvas.clone() };
    canvas.painter.rect_filled(screen, 0.0, Color32::from_black_alpha(90));
    canvas.nine(Art::Dialog, x, y, width, height, [22.0, 22.0, 22.0, 22.0], 0.5, Color32::WHITE);
    for (i, line) in shown.iter().enumerate() {
        canvas.text(x + 16.0, y + 12.0 + i as f32 * kit::LINE, line, scale, TEXT_DARK);
    }
    let bob = ((input.time * 4.0).sin() * 1.0) as f32;
    let glyph = glyph_width(&canvas, prompts, Prompt::Confirm);
    draw_prompt(&canvas, prompts, x + width - glyph - 6.0, y + height - 19.0 + bob, Prompt::Confirm);
    let clicked = input.pointer.primary_clicked()
        && input.pointer.hover_pos().is_some_and(|p| {
            let (px, py) = canvas.unit_of(p);
            (x..x + width).contains(&px) && (y..y + height).contains(&py)
        });
    nav.confirm || nav.back || clicked
}
