//! bottom: a scripted check of the bottom screen's layouts in a real window,
//! for development. FATES_BOTTOM_SCRIPT names a file of lines
//! `<game frame> <command> [arguments]`, run as the game reaches each frame:
//!
//! - `size W H`: asks for a window of W x H physical pixels; `fullscreen [off]`
//! - `mode auto|overlay|panel|on_demand|classic` (bottom2: `always` and
//!   `manual` too), `layout stacked|side_by_side|large_top|top_only|bottom_only`
//! - bottom2: `place ANCHOR DX DY HEIGHT` puts the floating window (anchor as
//!   in settings.toml, e.g. bottom_right); `press X Y`, `move X Y`, `up` are
//!   the mouse's left button at window pixels, through the same handling as
//!   the real mouse (a press on the frame takes it, not the game);
//!   `options TAB ROW` opens the options menu there, `options close`;
//!   `linger SECONDS`
//! - `side left|right`, `bottom_size F`, `opacity F`, `cursor_speed F`
//! - `button down|up`: the show/hide button
//! - `touch PX PY`: puts the mouse, held down, over bottom-screen pixel
//!   (PX, PY) as the window shows it and touches through the window's
//!   mapping, logging the pixel the game is given; `release` lets go
//! - `mouse X Y`: the mouse held down at window pixel (X, Y); `hover X Y` up
//! - `cursor PX PY [tap]`: the controller's touch cursor there, tapping
//! - `load STATE`: a saved state, the game's frame going on from its own
//! - `capture NAME`: the next frame drawn, read back to FATES_BOTTOM_OUT/NAME.png
//! - `quit`
//!
//! touches go to the game over a ZAKURO_REPLAY, which otherwise has all the
//! input.

use std::path::PathBuf;

use winit::dpi::PhysicalSize;

use crate::settings::{BottomScreen as Mode, Screens, Side};

pub struct Script {
    lines: Vec<(u64, Vec<String>)>,
    next: usize,
    out: PathBuf,
}

impl Script {
    pub fn from_env() -> Option<Script> {
        let path = std::env::var_os("FATES_BOTTOM_SCRIPT")?;
        let text = std::fs::read_to_string(&path).inspect_err(|error| log::warn!("bottom check script: {error}")).ok()?;
        let mut lines: Vec<(u64, Vec<String>)> = text
            .lines()
            .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
            .filter_map(|line| {
                let mut words = line.split_whitespace().map(str::to_owned);
                let frame = words.next()?.parse().ok()?;
                Some((frame, words.collect()))
            })
            .collect();
        lines.sort_by_key(|(frame, _)| *frame);
        let out = std::env::var_os("FATES_BOTTOM_OUT").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("bottom-check"));
        log::info!("bottom check: {} commands", lines.len());
        Some(Script { lines, next: 0, out })
    }
}

impl crate::App {
    /// runs the script's commands due by the game's frame.
    pub(crate) fn bottom_check_step(&mut self) {
        let Some(frame) = self.game.as_ref().map(|game| game.frame) else { return };
        loop {
            let Some(script) = &mut self.bottom_check else { return };
            let Some((at, words)) = script.lines.get(script.next).cloned() else { return };
            if at > frame {
                return;
            }
            script.next += 1;
            self.bottom_check_command(&words);
        }
    }

    fn bottom_check_command(&mut self, words: &[String]) {
        let arg = |n: usize| words.get(n).map(String::as_str).unwrap_or("");
        let number = |n: usize| arg(n).parse::<f32>().unwrap_or(0.0);
        log::info!("bottom check: {}", words.join(" "));
        match arg(0) {
            "size" => {
                if let Some(window) = &self.window {
                    let _ = window.request_inner_size(PhysicalSize::new(number(1) as u32, number(2) as u32));
                }
            }
            "mode" => {
                self.settings.pc.bottom_screen = match arg(1) {
                    "overlay" | "always" => Mode::Overlay,
                    "on_demand" | "manual" => Mode::OnDemand,
                    "classic" => Mode::Classic,
                    "auto" => Mode::Auto,
                    _ => Mode::Panel,
                };
                self.bottom_check_apply();
            }
            "layout" => {
                self.settings.layout = match arg(1) {
                    "side_by_side" => Screens::SideBySide,
                    "large_top" => Screens::LargeTop,
                    "top_only" => Screens::TopOnly,
                    "bottom_only" => Screens::BottomOnly,
                    _ => Screens::Stacked,
                };
                self.bottom_check_apply();
            }
            "side" => {
                self.settings.pc.bottom_side = if arg(1) == "left" { Side::Left } else { Side::Right };
                self.bottom_check_apply();
            }
            "bottom_size" => {
                self.settings.pc.bottom_size = number(1);
                self.bottom_check_apply();
            }
            "opacity" => {
                self.settings.pc.bottom_opacity = number(1);
                self.bottom_check_apply();
            }
            // bottom2: the floating window
            "place" => {
                let anchor: Result<crate::settings::BottomAnchor, _> = toml::Value::String(arg(1).to_owned()).try_into();
                self.settings.pc.bottom_anchor = anchor.unwrap_or_default();
                self.settings.pc.bottom_x = number(2);
                self.settings.pc.bottom_y = number(3);
                self.settings.pc.bottom_height = number(4);
                self.bottom_check_apply();
            }
            "linger" => {
                self.settings.pc.bottom_linger = number(1);
                self.bottom_check_apply();
            }
            // msaa: Anti-Aliasing set as the options menu sets it (settings.pc, then the active settings)
            "msaa" => {
                self.settings.pc.msaa = number(1) as u32;
                self.bottom_check_apply();
            }
            "press" => {
                self.cursor = (number(1), number(2));
                self.touch();
                let took = self.bottom.press(self.cursor.0, self.cursor.1);
                self.mouse_down = !took;
                self.touch();
                self.bottom_pointer_icon();
                log::info!("bottom check: press at ({}, {}) took the frame {took} ({:?}), the game is touched at {:?}", number(1), number(2), self.bottom.over, self.keyboard.state().touch);
            }
            "move" => {
                self.cursor = (number(1), number(2));
                self.bottom.drag_to(self.cursor.0, self.cursor.1);
                self.touch();
                self.bottom_pointer_icon();
                log::info!("bottom check: mouse at ({}, {}) over {:?}, the game is touched at {:?}", number(1), number(2), self.bottom.over, self.keyboard.state().touch);
            }
            "up" => {
                if let Some(place) = self.bottom.release() {
                    self.bottom_placed(place);
                    log::info!(
                        "bottom check: placed {:?} {} {} {} high",
                        self.settings.pc.bottom_anchor,
                        self.settings.pc.bottom_x,
                        self.settings.pc.bottom_y,
                        self.settings.pc.bottom_height
                    );
                }
                self.mouse_down = false;
                self.touch();
                self.bottom_pointer_icon();
            }
            "options" => {
                if arg(1) == "close" {
                    self.fates.options.hide();
                } else {
                    self.fates.options.show(0.0);
                    self.fates.options.select(number(1) as usize, crate::fates_ui::options::Page::Main, number(2) as usize);
                }
            }
            "cursor_speed" => {
                self.settings.pc.touch_cursor_speed = number(1);
                self.bottom_check_apply();
            }
            "button" => {
                let composes = self.composes();
                self.bottom.button(arg(1) == "down", &self.active, composes);
            }
            "touch" => {
                let (px, py) = (number(1), number(2));
                let bottom = self.bottom.composed_bottom().or_else(|| {
                    let size = self.window.as_ref()?.inner_size();
                    zakuro_gpu::layout(size.width, size.height, self.layout.screens(), self.orientation.screen_orientation(), self.top_shape.width(), self.active.integer_scale).1
                });
                match bottom {
                    Some(bottom) => {
                        self.cursor = zakuro_gpu::backend::bottom_panel::window_point(bottom, px, py);
                        self.mouse_down = true;
                        self.touch();
                        log::info!(
                            "bottom check: pixel ({px}, {py}) is window ({:.1}, {:.1}) in {bottom:?}, the game is touched at {:?}",
                            self.cursor.0,
                            self.cursor.1,
                            self.keyboard.state().touch
                        );
                    }
                    None => log::info!("bottom check: no bottom screen to touch"),
                }
            }
            "mouse" => {
                self.cursor = (number(1), number(2));
                self.mouse_down = true;
                self.touch();
                log::info!("bottom check: window ({}, {}) touches {:?}", number(1), number(2), self.keyboard.state().touch);
            }
            "hover" => {
                self.cursor = (number(1), number(2));
                self.mouse_down = false;
                self.touch();
            }
            "release" => {
                self.mouse_down = false;
                self.touch();
            }
            "cursor" => self.bottom.check_cursor(number(1), number(2), arg(3) == "tap"),
            "capture" => {
                let Some(script) = &self.bottom_check else { return };
                let path = script.out.join(format!("{}.png", arg(1)));
                if let Some(crate::present::Backend::Vulkan(presenter)) = &mut self.backend {
                    presenter.request_capture(path);
                }
                if let Some(window) = &self.window {
                    let size = window.inner_size();
                    log::info!("bottom check: window {}x{}, bottom screen at {:?}, auto {:?}", size.width, size.height, self.bottom.composed_bottom(), self.bottom.auto_reason());
                }
            }
            "fullscreen" => {
                if let Some(window) = &self.window {
                    window.set_fullscreen((arg(1) != "off").then_some(winit::window::Fullscreen::Borderless(None)));
                }
            }
            "load" => {
                // a saved state (FATES_SAVE_AT's or F5's), the game frame
                // going on from the one it was saved at
                let Some(game) = &mut self.game else { return };
                match std::fs::read(arg(1)).map_err(|error| error.to_string()).and_then(|bytes| game.system.load_state(&bytes)) {
                    Ok(()) => {
                        game.frame = game.system.frames;
                        log::info!("bottom check: loaded {} at frame {}", arg(1), game.frame);
                    }
                    Err(error) => log::warn!("bottom check: could not load {}: {error}", arg(1)),
                }
                // wide: a state brings back the console's picture pacing
                // without the frame rate loop's tracking, which hfr_apply
                // turns on again; the loop starts over with its pictures
                self.hfr_apply();
                self.hfr.reset();
            }
            "quit" => self.stop = true,
            // smooth: a scene starts here, in the timing log
            "mark" => zakuro_gpu::smooth::mark(arg(1)),
            other => log::warn!("bottom check: unknown command {other}"),
        }
    }

    fn bottom_check_apply(&mut self) {
        self.active = self.settings.for_game(self.program_id());
        self.layout = self.active.layout;
        if let Some(backend) = &mut self.backend {
            backend.set_layout(self.layout.screens());
        }
    }
}
