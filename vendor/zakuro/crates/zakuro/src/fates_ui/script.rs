//! ui: a test script for the interface in a real window, FATES_UI_SCRIPT=
//! <file>, so it can be driven and checked without sending the desktop any
//! input (other programs' windows share it). Its input goes to the
//! interface alone: keys, text, the pointer, controller buttons. One
//! command a line:
//!
//!   wait N             frames presented
//!   key NAME           an egui key pressed and let go (Escape, ArrowDown, Enter...)
//!   text STRING        typed text
//!   move X Y           the pointer, as fractions of the window
//!   mmove X Y, mclick X Y  the pointer in the options menu's units
//!   click X Y          the left button there
//!   rclick X Y         the right button there
//!   wheel N            the wheel, N lines (negative down)
//!   pad BUTTON[+B2]    console buttons on the pads, after bindings and layout
//!                      (A B X Y L R ZL ZR Start Select Up Down Left Right), or Home
//!   dir X Y            the circle pad (left stick) held, until dir 0 0
//!   place X Y          the window's position on the desktop
//!   size W H           the window's inner size in pixels
//!   ontop              keep the window over others, for capturing it
//!   shot NAME          writes NAME.request beside the script and waits until
//!                      something (dev/ui-capture.ps1) captures the window and deletes it
//!   log TEXT           a line in the log
//!   settings           logs the options' saved values
//!   winkey NAME        qol4: a key (winit KeyCode name, F2, Tab...) pressed as the
//!                      window's keyboard would: a binding waiting takes it, else
//!                      the options menu's key opens or closes the options
//!   waitframe N        qol4: waits until the game's frame N (a loaded machine
//!                      presents fewer pictures, the game's frames stay the game's)
//!   capture NAME       qol4: the next frame presented, read back by the app itself
//!                      to NAME.png beside the script (Vulkan)
//!   quit

use std::path::PathBuf;

use winit::window::WindowLevel;
use zakuro_core::services::hid::PadState;

use crate::App;

pub struct Script {
    lines: Vec<String>,
    at: usize,
    wait: u32,
    shot: Option<PathBuf>,
    dir: PathBuf,
    /// a pointer button to let go next frame.
    release: Option<(egui::Pos2, egui::PointerButton)>,
    keys_up: Vec<egui::Key>,
    /// console buttons held by `pad`, and for how many more frames.
    pad: PadState,
    pad_frames: u32,
    /// the circle pad, set by `dir`.
    pub circle: Option<[f32; 2]>,
    /// qol4: waitframe's game frame.
    until: Option<u64>,
}

impl Script {
    pub fn load() -> Option<Script> {
        let path = PathBuf::from(std::env::var_os("FATES_UI_SCRIPT")?);
        let text = std::fs::read_to_string(&path).inspect_err(|error| log::warn!("ui script {}: {error}", path.display())).ok()?;
        let lines = text.lines().map(|line| line.split('#').next().unwrap_or("").trim().to_owned()).filter(|line| !line.is_empty()).collect();
        log::info!("ui: running the script {}", path.display());
        Some(Script {
            lines,
            at: 0,
            wait: 0,
            shot: None,
            dir: path.parent().map(|p| p.to_owned()).unwrap_or_default(),
            release: None,
            keys_up: Vec::new(),
            pad: PadState::empty(),
            pad_frames: 0,
            circle: None,
            until: None, // qol4
        })
    }
}

fn button(name: &str) -> Option<PadState> {
    Some(match name {
        "A" => PadState::A,
        "B" => PadState::B,
        "X" => PadState::X,
        "Y" => PadState::Y,
        "L" => PadState::L,
        "R" => PadState::R,
        "ZL" => PadState::ZL,
        "ZR" => PadState::ZR,
        "Start" => PadState::START,
        "Select" => PadState::SELECT,
        "Up" => PadState::UP,
        "Down" => PadState::DOWN,
        "Left" => PadState::LEFT,
        "Right" => PadState::RIGHT,
        _ => return None,
    })
}

impl Script {
    /// the console buttons the script holds on the pads now.
    pub fn pad_held(&self) -> PadState {
        if self.pad_frames > 0 { self.pad } else { PadState::empty() }
    }
}

impl App {
    /// runs the script's commands due this frame.
    pub(crate) fn fates_script(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        let Some(mut script) = self.fates.script.take() else { return };
        let (Some(gui), Some(window)) = (&mut self.gui, &self.window) else {
            self.fates.script = Some(script);
            return;
        };
        let size = window.inner_size();
        let ppp = window.scale_factor() as f32;
        let at = |x: &str, y: &str| egui::pos2(x.parse::<f32>().unwrap_or(0.5) * size.width as f32 / ppp, y.parse::<f32>().unwrap_or(0.5) * size.height as f32 / ppp);
        let modifiers = egui::Modifiers::default();
        for key in script.keys_up.drain(..) {
            gui.inject.push(egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers });
        }
        if let Some((pos, button)) = script.release.take() {
            gui.inject.push(egui::Event::PointerButton { pos, button, pressed: false, modifiers });
        }
        script.pad_frames = script.pad_frames.saturating_sub(1);
        if let Some(shot) = &script.shot {
            if shot.exists() {
                self.fates.script = Some(script);
                return;
            }
            script.shot = None;
        }
        if script.wait > 0 {
            script.wait -= 1;
            self.fates.script = Some(script);
            return;
        }
        // qol4: waitframe
        if script.until.is_some_and(|until| self.game.as_ref().is_none_or(|game| game.frame < until)) {
            self.fates.script = Some(script);
            return;
        }
        script.until = None;
        let mut winkey = None; // qol4
        let mut capture = None;
        while let Some(line) = script.lines.get(script.at).cloned() {
            script.at += 1;
            let words: Vec<&str> = line.split_whitespace().collect();
            let arg = |i: usize| words.get(i).copied().unwrap_or("");
            match words[0] {
                "wait" => {
                    script.wait = arg(1).parse().unwrap_or(1);
                    break;
                }
                "key" => {
                    if let Some(key) = egui::Key::from_name(arg(1)) {
                        gui.inject.push(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers });
                        script.keys_up.push(key);
                    } else {
                        log::warn!("ui script: no key {}", arg(1));
                    }
                    script.wait = 1;
                    break;
                }
                "text" => gui.inject.push(egui::Event::Text(line[5..].to_owned())),
                "move" => gui.inject.push(egui::Event::PointerMoved(at(arg(1), arg(2)))),
                // the same in the options menu's units, the 3DS's pixels
                "mmove" | "mclick" => {
                    let (origin, unit) = self.fates.menu_at;
                    let pos = egui::pos2(origin.x + arg(1).parse::<f32>().unwrap_or(0.0) * unit, origin.y + arg(2).parse::<f32>().unwrap_or(0.0) * unit);
                    gui.inject.push(egui::Event::PointerMoved(pos));
                    if words[0] == "mclick" {
                        gui.inject.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers });
                        script.release = Some((pos, egui::PointerButton::Primary));
                    }
                    script.wait = 1;
                    break;
                }
                "click" | "rclick" => {
                    let pos = at(arg(1), arg(2));
                    let button = if words[0] == "click" { egui::PointerButton::Primary } else { egui::PointerButton::Secondary };
                    gui.inject.push(egui::Event::PointerMoved(pos));
                    gui.inject.push(egui::Event::PointerButton { pos, button, pressed: true, modifiers });
                    script.release = Some((pos, button));
                    script.wait = 1;
                    break;
                }
                "wheel" => {
                    let lines: f32 = arg(1).parse().unwrap_or(-1.0);
                    gui.inject.push(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: egui::vec2(0.0, lines), modifiers, phase: egui::TouchPhase::Move });
                }
                "pad" if arg(1) == "Home" => self.fates.options_action = true,
                "pad" => {
                    // pressed together: pad Start+Select
                    let buttons = arg(1).split('+').filter_map(button).fold(PadState::empty(), |all, b| all | b);
                    if buttons.is_empty() {
                        log::warn!("ui script: no button {}", arg(1));
                    }
                    script.pad = buttons;
                    script.pad_frames = 3;
                    script.wait = 4;
                    break;
                }
                "dir" => {
                    let (x, y): (f32, f32) = (arg(1).parse().unwrap_or(0.0), arg(2).parse().unwrap_or(0.0));
                    script.circle = (x != 0.0 || y != 0.0).then_some([x, y]);
                }
                "place" => window.set_outer_position(winit::dpi::PhysicalPosition::new(arg(1).parse::<i32>().unwrap_or(0), arg(2).parse::<i32>().unwrap_or(0))),
                "size" => {
                    let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(arg(1).parse::<u32>().unwrap_or(1280), arg(2).parse::<u32>().unwrap_or(720)));
                }
                "ontop" => window.set_window_level(WindowLevel::AlwaysOnTop),
                "shot" => {
                    let path = script.dir.join(format!("{}.request", arg(1)));
                    let position = window.inner_position().unwrap_or_default();
                    let text = format!("{} {} {} {}", position.x, position.y, size.width, size.height);
                    let _ = std::fs::write(&path, text);
                    log::info!("ui script: shot {} at {:?} {:?}", arg(1), position, size);
                    script.shot = Some(path);
                    script.wait = 0;
                    break;
                }
                "log" => log::info!("ui script: {}", &line[4..]),
                "waitframe" => {
                    script.until = arg(1).parse().ok();
                    break;
                }
                // qol4: the app's own capture of the next picture
                "capture" => {
                    capture = Some(script.dir.join(format!("{}.png", arg(1))));
                    script.wait = 1;
                    break;
                }
                // qol4: a key as the window's keyboard gives it
                "winkey" => {
                    match toml::Value::String(arg(1).to_owned()).try_into::<winit::keyboard::KeyCode>() {
                        Ok(code) => winkey = Some(code),
                        Err(_) => log::warn!("ui script: no key {}", arg(1)),
                    }
                    script.wait = 1;
                    break;
                }
                "settings" => {
                    let s = &self.settings;
                    log::info!(
                        "ui script: settings volume {:.2} mute {} show_fps {} resolution {} filter {:?} pc {:?}",
                        s.volume,
                        s.mute,
                        s.show_fps,
                        s.resolution,
                        s.filter,
                        s.pc
                    );
                }
                "quit" => {
                    log::info!("ui script: done");
                    event_loop.exit();
                    break;
                }
                other => log::warn!("ui script: unknown command {other}"),
            }
        }
        self.fates.script = Some(script);
        if let Some(path) = capture {
            log::info!("ui script: capture {}", path.display());
            if let Some(crate::present::Backend::Vulkan(presenter)) = &mut self.backend {
                presenter.request_capture(path);
            }
        }
        // qol4: the key, as App::key takes it: a binding waiting for a key,
        // else the options menu's key
        if let Some(code) = winkey {
            log::info!("ui script: winkey {code:?}");
            if let Some(index) = self.menus.rebinding.take() {
                if code != winit::keyboard::KeyCode::Escape || self.settings.keys.is_options_row(index) {
                    self.settings.keys.assign(index, code);
                    self.apply_settings();
                }
            } else {
                self.qol4_options_key(code, true, false);
            }
        }
    }
}
