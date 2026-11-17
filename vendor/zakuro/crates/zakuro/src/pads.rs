//! input: game controllers through SDL3's gamepad API, with SDL's HIDAPI
//! drivers (Switch Pro, Joy-Con pair and single, NSO NES/SNES/N64/Genesis,
//! GameCube adapters, DualShock 4, DualSense, Xbox over USB, Bluetooth and
//! the wireless adapter) and XInput/RawInput for the rest. Hotplug, any
//! number of pads (any of them drives the game), the community
//! gamecontrollerdb mappings, and rumble. When SDL3 cannot be loaded, gilrs
//! takes over (gamepad.rs).

use std::ffi::CString;

use crate::controls::{self, Family, PadButton, PadButtons, PadOutput, PadReading};
use crate::input_api::PadInfo;
use crate::sdl::{self, pad_type, Sdl, SDL_Event};
use crate::settings::ButtonLayout;

/// the SDL_GameControllerDB community mappings, built in (zlib licence,
/// data/gamecontrollerdb-LICENSE.txt). SDL keeps the lines for this platform.
static CONTROLLER_DB: &[u8] = include_bytes!("../data/gamecontrollerdb.txt");

/// a button newly pressed, the pad's family, and whether that pad uses the
/// positional layout, to store it the right way round when rebinding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Press {
    pub button: PadButton,
    pub family: Family,
    pub positional: bool,
}

pub struct Pads {
    backend: Backend,
    binds: PadButtons,
    layout: ButtonLayout,
    deadzone: f32,
    /// the family of the pad used last and when, none until one is.
    last_used: Option<(Family, std::time::Instant)>,
}

enum Backend {
    Sdl(SdlPads),
    Gilrs(crate::gamepad::GilrsPads),
    None,
}

struct SdlPads {
    sdl: Sdl,
    open: Vec<OpenPad>,
}

struct OpenPad {
    id: sdl::SDL_JoystickID,
    handle: *mut sdl::SDL_Gamepad,
    name: String,
    family: Family,
    kind: i32,
    rumble: bool,
    reading: PadReading,
}

impl Pads {
    pub fn new(binds: PadButtons, layout: ButtonLayout, deadzone: f32) -> Pads {
        let backend = if std::env::var_os("FATES_INPUT_GILRS").is_some() {
            log::info!("input: FATES_INPUT_GILRS set, using gilrs");
            None
        } else {
            match SdlPads::new() {
                Ok(pads) => Some(Backend::Sdl(pads)),
                Err(error) => {
                    log::warn!("input: SDL3 controllers unavailable ({error}); falling back to gilrs");
                    None
                }
            }
        };
        let backend = backend.unwrap_or_else(|| match crate::gamepad::GilrsPads::new() {
            Some(pads) => Backend::Gilrs(pads),
            None => Backend::None,
        });
        // a pad connected at start is the device in use until another is used
        let mut pads = Pads { backend, binds, layout, deadzone, last_used: None };
        pads.last_used = pads.list().first().map(|pad| (pad.family, std::time::Instant::now()));
        pads
    }

    pub fn configure(&mut self, binds: PadButtons, layout: ButtonLayout, deadzone: f32) {
        self.binds = binds;
        self.layout = layout;
        self.deadzone = deadzone;
    }

    #[allow(dead_code)]
    pub fn backend_name(&self) -> &'static str {
        match self.backend {
            Backend::Sdl(_) => "sdl",
            Backend::Gilrs(_) => "gilrs",
            Backend::None => "none",
        }
    }

    /// takes in what the controllers did since the last call, sends rumble
    /// asked for, and returns the buttons newly pressed.
    pub fn poll(&mut self) -> Vec<Press> {
        let layout = self.layout;
        let presses: Vec<Press> = match &mut self.backend {
            Backend::Sdl(pads) => pads.poll(),
            Backend::Gilrs(pads) => pads.poll(),
            Backend::None => Vec::new(),
        }
        .into_iter()
        .map(|(button, family)| Press { button, family, positional: family.positional(layout) })
        .collect();
        if let Some(press) = presses.last() {
            self.last_used = Some((press.family, std::time::Instant::now()));
        }
        let rumble = crate::input_api::take_rumble();
        if let Backend::Sdl(pads) = &mut self.backend {
            for (low, high, ms) in rumble {
                pads.rumble(low, high, ms);
            }
        }
        presses
    }

    /// what the controllers hold, merged, and the family of the pad used
    /// last (sticks and triggers count as use too).
    pub fn output(&mut self) -> (PadOutput, Option<(Family, std::time::Instant)>) {
        let readings: Vec<(Family, PadReading)> = match &self.backend {
            Backend::Sdl(pads) => pads.open.iter().map(|pad| (pad.family, pad.reading)).collect(),
            Backend::Gilrs(pads) => pads.readings().into_iter().map(|(_, family, reading)| (family, reading)).collect(),
            Backend::None => Vec::new(),
        };
        let mut merged = PadOutput::default();
        for (family, reading) in readings {
            let out = controls::resolve(&self.binds, family.positional(self.layout), &reading, self.deadzone);
            if reading.active(self.deadzone) {
                self.last_used = Some((family, std::time::Instant::now()));
            }
            merged.merge(&out);
        }
        (merged, self.last_used)
    }

    /// whether any controller holds a physical button.
    #[allow(dead_code)]
    pub fn holding(&self, button: PadButton) -> bool {
        match &self.backend {
            Backend::Sdl(pads) => pads.open.iter().any(|pad| pad.reading.held(button)),
            Backend::Gilrs(pads) => pads.readings().iter().any(|(_, _, reading)| reading.held(button)),
            Backend::None => false,
        }
    }

    pub fn list(&self) -> Vec<PadInfo> {
        match &self.backend {
            Backend::Sdl(pads) => pads
                .open
                .iter()
                .map(|pad| PadInfo { name: pad.name.clone(), family: pad.family, kind: kind_name(pad.kind), backend: "sdl", rumble: pad.rumble })
                .collect(),
            Backend::Gilrs(pads) => pads
                .readings()
                .into_iter()
                .map(|(name, family, _)| PadInfo { name, family, kind: "unknown", backend: "gilrs", rumble: false })
                .collect(),
            Backend::None => Vec::new(),
        }
    }
}

impl SdlPads {
    fn new() -> Result<SdlPads, String> {
        let sdl = Sdl::load()?;
        // the app has no SDL window: read controllers whatever window has
        // focus (winit tells focus apart), and let SDL run its own thread for
        // RawInput/WGI messages rather than relying on our message loop
        sdl.set_hint("SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS", "1");
        sdl.set_hint("SDL_JOYSTICK_THREAD", "1");
        // Joy-Cons: a pair together is one pad, a single one held sideways
        sdl.set_hint("SDL_JOYSTICK_HIDAPI_COMBINE_JOY_CONS", "1");
        sdl.set_hint("SDL_JOYSTICK_HIDAPI_VERTICAL_JOY_CONS", "0");
        // SAFETY: plain call; SDL is not used from any other thread
        if !unsafe { (sdl.SDL_Init)(sdl::SDL_INIT_GAMEPAD) } {
            return Err(format!("SDL_Init failed, {}", sdl.error()));
        }
        log::info!("input: SDL {} gamepad subsystem up", sdl.version());
        // SAFETY: the bytes are static; SDL reads them and closes the stream
        let added = unsafe {
            let io = (sdl.SDL_IOFromConstMem)(CONTROLLER_DB.as_ptr().cast(), CONTROLLER_DB.len());
            if io.is_null() { -1 } else { (sdl.SDL_AddGamepadMappingsFromIO)(io, true) }
        };
        log::info!("input: {added} controller mappings from the built-in gamecontrollerdb");
        // a newer gamecontrollerdb.txt beside the program or in userdata
        let mut files: Vec<std::path::PathBuf> = Vec::new();
        if let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|dir| dir.to_path_buf())) {
            files.push(dir.join("gamecontrollerdb.txt"));
        }
        if let Some(dir) = std::env::var_os("APPDATA") {
            files.push(std::path::PathBuf::from(dir).join("gamecontrollerdb.txt"));
        }
        for file in files.into_iter().filter(|file| file.is_file()) {
            let Ok(path) = CString::new(file.to_string_lossy().as_bytes()) else { continue };
            // SAFETY: a valid C path
            let added = unsafe { (sdl.SDL_AddGamepadMappingsFromFile)(path.as_ptr()) };
            log::info!("input: {added} controller mappings from {}", file.display());
        }
        let mut pads = SdlPads { sdl, open: Vec::new() };
        let ids = pads.sdl.gamepads();
        if ids.is_empty() {
            log::info!("input: no controller connected");
        }
        for id in ids {
            pads.open(id);
        }
        Ok(pads)
    }

    fn open(&mut self, id: sdl::SDL_JoystickID) {
        if self.open.iter().any(|pad| pad.id == id) {
            return;
        }
        let sdl = &self.sdl;
        // SAFETY: id came from SDL; the handle stays valid until closed
        unsafe {
            let handle = (sdl.SDL_OpenGamepad)(id);
            if handle.is_null() {
                log::warn!("input: could not open controller {id}, {}", sdl.error());
                return;
            }
            let name = sdl::cstr((sdl.SDL_GetGamepadName)(handle));
            let kind = (sdl.SDL_GetGamepadType)(handle);
            let real = (sdl.SDL_GetRealGamepadType)(handle);
            let south = (sdl.SDL_GetGamepadButtonLabel)(handle, 0);
            let family = Family::from_sdl(kind, south, &name);
            let (vendor, product) = ((sdl.SDL_GetGamepadVendor)(handle), (sdl.SDL_GetGamepadProduct)(handle));
            let connection = match (sdl.SDL_GetGamepadConnectionState)(handle) {
                1 => "wired",
                2 => "wireless",
                _ => "connection unknown",
            };
            // a zero-length rumble tells whether it can, without a buzz
            let rumble = (sdl.SDL_RumbleGamepad)(handle, 0, 0, 0);
            // the first player's light, and Fates' blue on DualSense/DS4 bars
            (sdl.SDL_SetGamepadPlayerIndex)(handle, 0);
            (sdl.SDL_SetGamepadLED)(handle, 0x30, 0x60, 0xD0);
            log::info!(
                "input: controller connected: \"{name}\" type {} (real {}), {vendor:04x}:{product:04x}, {connection}, family {family:?}, {} layout, rumble {}",
                kind_name(kind),
                kind_name(real),
                if family.positional(ButtonLayout::Auto) { "positional (east = A)" } else { "label (south = confirm)" },
                if rumble { "yes" } else { "no" },
            );
            self.open.push(OpenPad { id, handle, name, family, kind, rumble, reading: PadReading::default() });
        }
    }

    fn close(&mut self, id: sdl::SDL_JoystickID) {
        if let Some(index) = self.open.iter().position(|pad| pad.id == id) {
            let pad = self.open.remove(index);
            log::info!("input: controller disconnected: \"{}\"", pad.name);
            // SAFETY: opened by open() and not closed since
            unsafe { (self.sdl.SDL_CloseGamepad)(pad.handle) };
        }
    }

    fn poll(&mut self) -> Vec<(PadButton, Family)> {
        let mut pressed = Vec::new();
        let mut event = SDL_Event([0; 128]);
        // SAFETY: event is a buffer the size and alignment of SDL_Event
        while unsafe { (self.sdl.SDL_PollEvent)(&mut event) } {
            match event.kind() {
                sdl::SDL_EVENT_GAMEPAD_ADDED => self.open(event.which()),
                sdl::SDL_EVENT_GAMEPAD_REMOVED => self.close(event.which()),
                sdl::SDL_EVENT_GAMEPAD_REMAPPED => {
                    if let Some(pad) = self.open.iter().find(|pad| pad.id == event.which()) {
                        log::info!("input: controller \"{}\" remapped", pad.name);
                    }
                }
                sdl::SDL_EVENT_GAMEPAD_BUTTON_DOWN => {
                    let family = self.open.iter().find(|pad| pad.id == event.which()).map_or(Family::Generic, |pad| pad.family);
                    if let Some(button) = PadButton::from_sdl(event.button()) {
                        pressed.push((button, family));
                    }
                }
                _ => {}
            }
        }
        // the state now, and the triggers crossing the threshold as presses
        let sdl = &self.sdl;
        for pad in &mut self.open {
            let previous = pad.reading;
            let mut reading = PadReading::default();
            // SAFETY: an open handle; out-of-range values are SDL's to refuse
            unsafe {
                for button in PadButton::ALL {
                    if let Some(index) = button.sdl() {
                        reading.set(button, (sdl.SDL_GetGamepadButton)(pad.handle, index));
                    }
                }
                let axis = |index| (sdl.SDL_GetGamepadAxis)(pad.handle, index) as f32 / 32767.0;
                // SDL's y axes point down; the console's up
                reading.left = [axis(sdl::AXIS_LEFTX), -axis(sdl::AXIS_LEFTY)].map(|v| v.clamp(-1.0, 1.0));
                reading.right = [axis(sdl::AXIS_RIGHTX), -axis(sdl::AXIS_RIGHTY)].map(|v| v.clamp(-1.0, 1.0));
                reading.triggers = [axis(sdl::AXIS_LEFT_TRIGGER), axis(sdl::AXIS_RIGHT_TRIGGER)].map(|v| v.clamp(0.0, 1.0));
            }
            for trigger in [PadButton::LeftTrigger, PadButton::RightTrigger] {
                if reading.held(trigger) && !previous.held(trigger) {
                    pressed.push((trigger, pad.family));
                }
            }
            pad.reading = reading;
        }
        pressed
    }

    fn rumble(&mut self, low: u16, high: u16, ms: u32) {
        for pad in self.open.iter().filter(|pad| pad.rumble) {
            // SAFETY: an open handle
            unsafe { (self.sdl.SDL_RumbleGamepad)(pad.handle, low, high, ms) };
        }
    }
}

impl Drop for SdlPads {
    fn drop(&mut self) {
        for pad in self.open.drain(..) {
            // SAFETY: opened and not closed
            unsafe { (self.sdl.SDL_CloseGamepad)(pad.handle) };
        }
        // SAFETY: SDL was initialised with this subsystem
        unsafe { (self.sdl.SDL_QuitSubSystem)(sdl::SDL_INIT_GAMEPAD) };
    }
}

pub fn kind_name(kind: i32) -> &'static str {
    match kind {
        pad_type::STANDARD => "standard",
        pad_type::XBOX360 => "Xbox 360",
        pad_type::XBOXONE => "Xbox One/Series",
        pad_type::PS3 => "PS3",
        pad_type::PS4 => "DualShock 4",
        pad_type::PS5 => "DualSense",
        pad_type::SWITCH_PRO => "Switch Pro",
        pad_type::JOYCON_LEFT => "Joy-Con (L)",
        pad_type::JOYCON_RIGHT => "Joy-Con (R)",
        pad_type::JOYCON_PAIR => "Joy-Con pair",
        pad_type::GAMECUBE => "GameCube",
        pad_type::STEAM => "Steam",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SDL3 loads (from SDL3.dll on PATH or beside the test), initialises
    /// with or without controllers, and lists what is connected. Skipped
    /// where SDL3 is not installed.
    #[test]
    fn sdl_initialises_and_enumerates() {
        match SdlPads::new() {
            Ok(mut pads) => {
                let presses = pads.poll();
                eprintln!("SDL {}: {} controllers: {:?}", pads.sdl.version(), pads.open.len(), pads.open.iter().map(|pad| (&pad.name, kind_name(pad.kind), pad.family)).collect::<Vec<_>>());
                assert!(presses.len() < 64);
                // rumble on nothing, or on what is there, does not fail
                pads.rumble(0, 0, 0);
            }
            Err(error) => eprintln!("SDL3 not available here, {error}"),
        }
    }

    /// a live probe for virtual or real controllers (dev/tools/vpad):
    /// FATES_PROBE_SECS seconds of hotplug, presses and what the game would
    /// get, with a rumble sent to each pad as it connects.
    ///   cargo test --lib pads::tests::probe -- --ignored --nocapture
    #[test]
    #[ignore]
    fn probe() {
        let secs: u64 = std::env::var("FATES_PROBE_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(10);
        let _ = env_logger::builder().is_test(true).filter_level(log::LevelFilter::Info).try_init();
        let mut pads = Pads::new(PadButtons::default(), ButtonLayout::Auto, 0.15);
        let start = std::time::Instant::now();
        let mut known = 0;
        let mut shown = PadOutput::default();
        while start.elapsed().as_secs() < secs {
            for press in pads.poll() {
                println!("probe {:>5} ms: pressed {:?} on a {:?} pad ({} layout)", start.elapsed().as_millis(), press.button, press.family, if press.positional { "positional" } else { "label" });
            }
            let list = pads.list();
            if list.len() != known {
                known = list.len();
                println!("probe {:>5} ms: {} controllers {:?}", start.elapsed().as_millis(), known, list);
                crate::input_api::configure(true, crate::settings::Glyphs::Auto, ButtonLayout::Auto, &crate::controls::Keys::default(), &PadButtons::default());
                crate::input_api::rumble(0.8, 0.4, 300);
            }
            let (out, last) = pads.output();
            if out != shown {
                println!("probe {:>5} ms: game gets {:?} circle {:?} right stick {:?} actions {:?} (last pad {:?})", start.elapsed().as_millis(), out.buttons, out.circle, out.right_stick, out.actions, last.map(|l| l.0));
                shown = out;
            }
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
    }
}
