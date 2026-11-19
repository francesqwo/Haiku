//! ui: how the app drives the Fates interface: what it shows each frame,
//! and what the options do once changed.

use winit::event_loop::ActiveEventLoop;

use zakuro_core::services::hid::{InputState, PadState};
use zakuro_gpu::Overlay;

use super::options::{Effect, Env};
use super::View;
use crate::App;

impl App {
    /// the interface over the game for a frame, in place of Zakuro's menus.
    pub(crate) fn fates_interface(&mut self, event_loop: &ActiveEventLoop) -> Overlay {
        self.fates_script(event_loop);
        // mods: the game's Main Menu asked for the Mods page
        if crate::haiku_mods::take_open_request() {
            self.fates.open_mods = true;
        }
        self.fates_code_mods_poll(); // codemods: the compile, and the restart after it
        self.fates_mods_poll(); // hdpacks: downloads unpacked meanwhile
        if self.fates.refresh_env {
            self.fates.refresh_env = false;
            self.fates_refresh_env();
            self.fates_mods_rescan(); // mods: mods added while the game runs
        }
        // cheats: what a cheat did, as a notice; the menu's view of the army
        if let Some(text) = crate::cheats::take_notice() {
            self.menus.toast = Some((text, std::time::Instant::now()));
        }
        crate::cheats::set_menu_open(self.fates.options.on_cheats()); // cheats2: only on the Cheats tab
        // pulse: Divine Pulse's and the save states' notices
        if let Some(text) = crate::pulse::take_toast().or_else(crate::states::take_notice) {
            self.menus.toast = Some((text, std::time::Instant::now()));
        }
        // statecheck: a long notice stays long enough to be read
        let toast = self.menus.toast.as_ref().filter(|(text, since)| since.elapsed().as_millis() < 2500u128.max(text.len() as u128 * 55)).map(|(text, _)| text.clone());
        if toast.is_none() {
            self.menus.toast = None;
        }
        // online: the session's news, the host's code, the code box
        crate::online_ui::configure(&self.settings.pc.online);
        crate::online_ui::poll();
        if let Some(text) = crate::online_ui::take_toast() {
            self.menus.toast = Some((text, std::time::Instant::now()));
        }
        if self.menus.message.is_none() {
            if let Some(text) = crate::online_ui::take_message() {
                self.menus.message = Some(text);
            }
        }
        let game_keyboard = self.game.as_ref().and_then(|game| game.system.keyboard_request().cloned());
        let online_keyboard = if game_keyboard.is_none() { crate::online_ui::keyboard_request(self.menus.message.is_some()) } else { None };
        self.fates.keyboard.help = online_keyboard.as_ref().and_then(|_| crate::online_ui::keyboard_help());
        let mut message = self.menus.message.clone();
        if self.game.is_none() && message.is_none() {
            message = Some("The game could not be started. Check that the game folder holds Fates-USA.cxi, then start it again.".to_owned());
        }
        let view = View {
            game: self.game.is_some(),
            // smooth2: new pictures a second as presented, where the frame
            // rate loop runs (the game's own 60 refreshes said nothing of it)
            fps: self.game.as_ref().filter(|_| self.settings.show_fps).map(|game| self.hfr.shown_rate.filter(|_| self.hfr.rate > 0.0).unwrap_or(game.fps)),
            fast: self.fast_forwarding,
            corner: self.overlay_corner.clone().filter(|_| self.game.is_some()),
            toast,
            message,
            keyboard: game_keyboard.clone().or_else(|| online_keyboard.clone()),
            pad_held: self.fates.pad_held | self.fates.script.as_ref().map_or(PadState::empty(), |script| script.pad_held()),
            pad_circle: self.fates.script.as_ref().and_then(|script| script.circle).unwrap_or(self.fates.pad_circle),
            options_action: std::mem::take(&mut self.fates.options_action),
            glyphs: crate::glyphs::forced().unwrap_or_else(crate::input_api::glyph_set),
            rebinding: self.menus.rebinding.is_some() || self.menus.rebinding_pad.is_some(),
            // qol: the boon/bane panel
            chara: self.qol.chara.clone(),
            cover: self.qol.cover.alpha(),
            top_screen: self.qol.top_screen,
            // info-ui: the growth button and view
            growth: crate::info_ui::with_state(|state| state.panel()),
            bottom_screen: crate::info_ui::with_state(|state| state.bottom_screen),
            online: crate::online_ui::host_panel(), // online
            online_status: crate::online_ui::status_line(),
        };
        let (Some(gui), Some(window)) = (&mut self.gui, &self.window) else { return Overlay::default() };
        let (fates, settings) = (&mut self.fates, &mut self.settings);
        let mut output = None;
        let overlay = gui.frame(window, |ui| {
            output = Some(fates.run(ui.ctx(), &view, settings));
        });
        let Some(output) = output else { return overlay };
        if let Some((text, button)) = output.keyboard {
            if game_keyboard.is_none() && online_keyboard.is_some() {
                crate::online_ui::answer(&text, button); // online: the code box
            } else if let Some(game) = &mut self.game {
                game.system.answer_keyboard(&text, button);
            }
            self.fates.swallow = true;
        }
        if output.message_closed {
            if self.game.is_none() {
                event_loop.exit();
            }
            self.menus.message = None;
            self.menus.report = None;
        }
        if output.quit {
            event_loop.exit();
        }
        for effect in output.effects {
            self.fates_effect(effect);
        }
        overlay
    }

    /// what a changed option does. The menu saves every change (through
    /// apply_settings, which also refreshes self.active) and applies what
    /// this module owns. Where another area owns the effect, it reads the
    /// value from self.active / self.settings after apply_settings:
    ///
    /// - hfr: pc.vsync, pc.frame_rate, pc.fps_cap, pc.msaa -- hook: read
    ///   self.active.pc in App::step / the presenter after apply_settings.
    ///   pc.window_mode, window/fullscreen size, refresh_rate, monitor: a
    ///   working winit version is fates_apply_display below; hfr may take it.
    /// - wide: pc.aspect -- applied live: App::present calls wide::tick with
    ///   self.active.pc.aspect each frame, and the GPU takes a new shape
    ///   between frames once it has settled for 300 ms (see wide.rs).
    /// - bottom: pc.bottom_screen, bottom_side, bottom_size, bottom_opacity --
    ///   hook: apply_layout (lib.rs) / the presenter's layout.
    /// - input: pc.button_layout, glyphs, stick_deadzone, rumble,
    ///   touch_cursor_speed -- hook: Gamepads (gamepad.rs) after
    ///   apply_settings; settings.keys and settings.pad are already applied
    ///   there by apply_settings.
    /// - ui (here): volume, mute, pc.audio_device (reopens the output),
    ///   pc.mute_unfocused (apply_volume), pc.ui_scale, show_fps, filter
    ///   (apply_layout); resolution (render scale) and language apply when
    ///   the game next starts, the renderer and the console's config only
    ///   take them then.
    fn fates_effect(&mut self, effect: Effect) {
        match effect {
            Effect::Apply => self.apply_settings(),
            Effect::Display => {
                self.apply_settings();
                self.fates_apply_display();
                self.fates_refresh_env();
            }
            Effect::Audio => {
                self.apply_settings();
                self.fates_reopen_audio();
            }
            Effect::RebindKey(index) => self.menus.rebinding = Some(index),
            Effect::RebindPad(index) => self.menus.rebinding_pad = Some(index),
            Effect::Closed => {
                self.keyboard.release();
                self.menus.rebinding = None;
                self.menus.rebinding_pad = None;
            }
            Effect::Mods(effect) => self.fates_mods_effect(effect), // mods
            // qol: Exit to Desktop, confirmed in the options
            Effect::Quit => {
                log::info!("qol: Exit to Desktop was chosen in the options");
                self.stop = true;
            }
            // qol4: Return to Title, confirmed in the options: the game's soft reset
            Effect::ReturnTitle => crate::qol4::return_to_title("the options"),
        }
    }

    /// the monitors, the chosen one's display modes, and the sound devices,
    /// for the lists the options offer.
    fn fates_refresh_env(&mut self) {
        let mut env = Env::default();
        if let Some(window) = &self.window {
            let monitors = monitors(window);
            env.monitors = monitors
                .iter()
                .enumerate()
                .map(|(index, monitor)| {
                    let size = monitor.size();
                    format!("Display {} ({}\u{d7}{})", index + 1, size.width, size.height)
                })
                .collect();
            let chosen = monitors.get(self.settings.pc.monitor as usize).cloned().or_else(|| window.current_monitor());
            if let Some(monitor) = chosen {
                let mut modes: Vec<(u32, u32, u32)> =
                    monitor.video_modes().map(|mode| (mode.size().width, mode.size().height, (mode.refresh_rate_millihertz() + 500) / 1000)).collect();
                modes.sort();
                modes.dedup();
                env.modes = modes;
            }
        }
        env.audio_devices = crate::audio::output_devices();
        env.mods = std::mem::take(&mut self.fates.env.mods); // mods
        log::info!("ui: monitors {:?}, {} display modes, sound devices {:?}", env.monitors, env.modes.len(), env.audio_devices);
        self.fates.env = env;
    }

    /// hfr: the window mode, size and monitor from settings.pc. The hfr
    /// agent owns presentation timing (frame_rate, fps_cap, vsync, msaa);
    /// it reads those from self.active.pc after apply_settings and may
    /// replace this function if it takes over display modes.
    pub(crate) fn fates_apply_display(&mut self) {
        // hfr: display.rs took this over (exclusive fullscreen at the chosen
        // mode and refresh rate, monitor placement, vsync, live resolution)
        self.display_apply_with(true);
    }

    /// the sound on the device the settings name, or the system's default.
    fn fates_reopen_audio(&mut self) {
        if self.options.mute {
            return;
        }
        self.audio = None;
        self.audio = crate::audio::Audio::open_on(zakuro_core::AUDIO_SAMPLE_RATE, &self.settings.pc.audio_device)
            .inspect_err(|error| log::warn!("no sound, {error}"))
            .ok();
        self.apply_volume();
    }

}

/// the displays, the primary one first, so the default, 0, is the primary.
fn monitors(window: &winit::window::Window) -> Vec<winit::monitor::MonitorHandle> {
    let primary = window.primary_monitor();
    let mut list: Vec<_> = primary.iter().cloned().collect();
    list.extend(window.available_monitors().filter(|monitor| Some(monitor) != primary.as_ref()));
    list
}

/// the game's input, held back after a menu or message closes until the
/// button that closed it is let go.
pub(crate) fn filter_input(fates: &mut super::FatesUi, input: InputState) -> InputState {
    if !fates.swallow {
        return input;
    }
    if input.buttons.is_empty() && input.touch.is_none() {
        fates.swallow = false;
        return input;
    }
    InputState { tilt: input.tilt, ..InputState::default() }
}
