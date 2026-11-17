//! input: what the player's keys and controller buttons do. The bindings
//! kept in the settings (`[keys]`, `[pad]`), the button layout
//! (`pc.button_layout`), controller families and the names their buttons go
//! by (`pc.glyphs`). Pure logic with no devices in it, so all of it is
//! unit-tested; `pads.rs` reads the devices and `input_api.rs` publishes
//! the result to the rest of the port.
//!
//! The controller bindings are stored by *position*, as a Nintendo
//! controller has them: `a = ["East"]` means the right face button, which is
//! A on a 3DS and a Switch Pro Controller. When a pad uses the label layout
//! (Xbox and PlayStation pads under `button_layout = "auto"`, or every pad
//! under `"labels"`), the face buttons are mirrored across the diagonal,
//! South<->East and West<->North, so the bottom button (Xbox A, PS Cross)
//! confirms and the right one (Xbox B, PS Circle) goes back.

use std::collections::HashSet;

use serde::de::{DeserializeOwned, IntoDeserializer};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use winit::keyboard::KeyCode;
use zakuro_core::services::hid::PadState;

use crate::settings::{ButtonLayout, Glyphs};

/// the current default scheme. Settings written before it are moved to it
/// by `migrate` when they still hold the old defaults.
pub const SCHEME: u32 = 2;

/// how far a trigger is pulled before it counts as pressed.
pub const TRIGGER_THRESHOLD: f32 = 0.5;

/// a controller button by its position. The names in the settings file are
/// the ones gilrs used, so files written before SDL keep working.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PadButton {
    South,
    East,
    West,
    North,
    #[serde(rename = "Select")]
    Back,
    #[serde(rename = "Mode")]
    Guide,
    Start,
    #[serde(rename = "LeftThumb")]
    LeftStick,
    #[serde(rename = "RightThumb")]
    RightStick,
    #[serde(rename = "LeftTrigger")]
    LeftShoulder,
    #[serde(rename = "RightTrigger")]
    RightShoulder,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
    /// Xbox Share, PS5 mic, Switch Capture.
    Misc1,
    RightPaddle1,
    LeftPaddle1,
    RightPaddle2,
    LeftPaddle2,
    Touchpad,
    /// the analog triggers, pressed past TRIGGER_THRESHOLD.
    #[serde(rename = "LeftTrigger2")]
    LeftTrigger,
    #[serde(rename = "RightTrigger2")]
    RightTrigger,
}

impl PadButton {
    pub const COUNT: usize = 23;
    pub const ALL: [PadButton; PadButton::COUNT] = [
        PadButton::South,
        PadButton::East,
        PadButton::West,
        PadButton::North,
        PadButton::Back,
        PadButton::Guide,
        PadButton::Start,
        PadButton::LeftStick,
        PadButton::RightStick,
        PadButton::LeftShoulder,
        PadButton::RightShoulder,
        PadButton::DPadUp,
        PadButton::DPadDown,
        PadButton::DPadLeft,
        PadButton::DPadRight,
        PadButton::Misc1,
        PadButton::RightPaddle1,
        PadButton::LeftPaddle1,
        PadButton::RightPaddle2,
        PadButton::LeftPaddle2,
        PadButton::Touchpad,
        PadButton::LeftTrigger,
        PadButton::RightTrigger,
    ];

    /// the button SDL_GamepadButton `index` is. The first 21 buttons are in
    /// SDL's own order, so the index is the same.
    pub fn from_sdl(index: u8) -> Option<PadButton> {
        PadButton::ALL.get(index as usize).copied().filter(|button| button.sdl().is_some())
    }

    /// the SDL_GamepadButton this is, none for the triggers, which SDL
    /// reports as axes.
    pub fn sdl(self) -> Option<i32> {
        let index = self as i32;
        (index < 21).then_some(index)
    }

    /// the button in the same place on a pad with the other layout: the
    /// face buttons mirrored across the diagonal.
    pub fn mirrored(self) -> PadButton {
        match self {
            PadButton::South => PadButton::East,
            PadButton::East => PadButton::South,
            PadButton::West => PadButton::North,
            PadButton::North => PadButton::West,
            other => other,
        }
    }

    /// what the button is called on a controller of `family`.
    pub fn name(self, family: Family) -> &'static str {
        use PadButton::*;
        match (family, self) {
            (Family::Nintendo, South) => "B",
            (Family::Nintendo, East) => "A",
            (Family::Nintendo, West) => "Y",
            (Family::Nintendo, North) => "X",
            (Family::Nintendo, Back) => "-",
            (Family::Nintendo, Start) => "+",
            (Family::Nintendo, Guide) => "Home",
            (Family::Nintendo, LeftShoulder) => "L",
            (Family::Nintendo, RightShoulder) => "R",
            (Family::Nintendo, LeftTrigger) => "ZL",
            (Family::Nintendo, RightTrigger) => "ZR",
            (Family::Nintendo, Misc1) => "Capture",
            (Family::PlayStation, South) => "Cross",
            (Family::PlayStation, East) => "Circle",
            (Family::PlayStation, West) => "Square",
            (Family::PlayStation, North) => "Triangle",
            (Family::PlayStation, Back) => "Create",
            (Family::PlayStation, Start) => "Options",
            (Family::PlayStation, Guide) => "PS",
            (Family::PlayStation, LeftShoulder) => "L1",
            (Family::PlayStation, RightShoulder) => "R1",
            (Family::PlayStation, LeftTrigger) => "L2",
            (Family::PlayStation, RightTrigger) => "R2",
            (Family::PlayStation, LeftStick) => "L3",
            (Family::PlayStation, RightStick) => "R3",
            (Family::PlayStation, Misc1) => "Mic",
            (_, South) => "A",
            (_, East) => "B",
            (_, West) => "X",
            (_, North) => "Y",
            (_, Back) => "View",
            (_, Start) => "Menu",
            (_, Guide) => "Guide",
            (_, LeftShoulder) => "LB",
            (_, RightShoulder) => "RB",
            (_, LeftTrigger) => "LT",
            (_, RightTrigger) => "RT",
            (_, LeftStick) => "LS",
            (_, RightStick) => "RS",
            (_, DPadUp) => "D-pad Up",
            (_, DPadDown) => "D-pad Down",
            (_, DPadLeft) => "D-pad Left",
            (_, DPadRight) => "D-pad Right",
            (_, Misc1) => "Share",
            (_, RightPaddle1) => "P1",
            (_, LeftPaddle1) => "P3",
            (_, RightPaddle2) => "P2",
            (_, LeftPaddle2) => "P4",
            (_, Touchpad) => "Touchpad",
        }
    }
}

/// which kind of controller, for the layout and for the button pictures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Family {
    /// Switch Pro, Joy-Con, NSO NES/SNES/N64/Genesis, GameCube, Wii, and
    /// pads that label their bottom button B.
    Nintendo,
    #[default]
    Xbox,
    PlayStation,
    /// anything else, which is laid out and named as an Xbox pad.
    Generic,
}

impl Family {
    /// from what SDL says of a pad: SDL_GetGamepadType, the label on the
    /// bottom face button, and the name.
    pub fn from_sdl(kind: i32, south_label: i32, name: &str) -> Family {
        use crate::sdl::{label, pad_type as t};
        match kind {
            t::SWITCH_PRO | t::JOYCON_LEFT | t::JOYCON_RIGHT | t::JOYCON_PAIR | t::GAMECUBE => Family::Nintendo,
            t::PS3 | t::PS4 | t::PS5 => Family::PlayStation,
            t::XBOX360 | t::XBOXONE => Family::Xbox,
            _ => match south_label {
                label::B => Family::Nintendo,
                label::CROSS => Family::PlayStation,
                label::A => Family::from_name(name).filter(|family| *family != Family::Generic).unwrap_or(Family::Xbox),
                _ => Family::from_name(name).unwrap_or(Family::Generic),
            },
        }
    }

    /// a guess from a controller's name alone, for backends that say no more.
    pub fn from_name(name: &str) -> Option<Family> {
        let name = name.to_ascii_lowercase();
        let any = |words: &[&str]| words.iter().any(|word| name.contains(word));
        if any(&["nintendo", "switch", "joy-con", "joycon", "pro controller", "snes", "n64", "gamecube", "wii", "famicom", "nes controller"]) {
            Some(Family::Nintendo)
        } else if any(&["dualsense", "dualshock", "playstation", "ps3", "ps4", "ps5", "sony"]) {
            Some(Family::PlayStation)
        } else if any(&["xbox", "xinput", "x-box", "microsoft"]) {
            Some(Family::Xbox)
        } else {
            None
        }
    }

    /// whether the east face button confirms (the 3DS's A) under `layout`.
    pub fn positional(self, layout: ButtonLayout) -> bool {
        match layout {
            ButtonLayout::Positional => true,
            ButtonLayout::Labels => false,
            ButtonLayout::Auto => self == Family::Nintendo,
        }
    }
}

/// the device the player last used, which `pc.glyphs = "auto"` follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Device {
    #[default]
    Keyboard,
    Pad(Family),
}

/// the set of button pictures and names prompts show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum GlyphSet {
    /// the game's own 3DS pictures, which a Nintendo controller matches.
    #[default]
    Nintendo,
    Xbox,
    PlayStation,
    Keyboard,
}

impl GlyphSet {
    pub fn resolve(setting: Glyphs, last: Device) -> GlyphSet {
        match setting {
            Glyphs::Nintendo => GlyphSet::Nintendo,
            Glyphs::Xbox => GlyphSet::Xbox,
            Glyphs::PlayStation => GlyphSet::PlayStation,
            Glyphs::Keyboard => GlyphSet::Keyboard,
            Glyphs::Auto => match last {
                Device::Keyboard => GlyphSet::Keyboard,
                Device::Pad(Family::Nintendo) => GlyphSet::Nintendo,
                Device::Pad(Family::PlayStation) => GlyphSet::PlayStation,
                Device::Pad(Family::Xbox | Family::Generic) => GlyphSet::Xbox,
            },
        }
    }

    /// the controller family these pictures are of, none for the keyboard.
    pub fn family(self) -> Option<Family> {
        match self {
            GlyphSet::Nintendo => Some(Family::Nintendo),
            GlyphSet::Xbox => Some(Family::Xbox),
            GlyphSet::PlayStation => Some(Family::PlayStation),
            GlyphSet::Keyboard => None,
        }
    }
}

/// what the port itself does with a button, beyond the console's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PcAction {
    /// touch the bottom screen where the right stick's pointer is.
    TouchClick,
    /// open or close the PC options menu (keyboard: Escape unless rebound,
    /// qol4: Keys::options).
    Options,
    /// show, hide or hold the bottom screen.
    BottomScreen,
    /// run the game as fast as it goes while held.
    FastForward,
    // cameras: the battle free camera (cameras.rs)
    /// turn the battle free camera on or off.
    FreeCamera,
    /// give the battle scene the game's own camera back.
    CameraReset,
    /// bring the free camera nearer while held.
    CameraZoomIn,
    /// take the free camera further away while held.
    CameraZoomOut,
    /// info-ui: show or hide the growth rates on the lower screen's unit
    /// status.
    GrowthRates,
}

impl PcAction {
    /// cameras: how many there are, for the arrays of them
    pub const COUNT: usize = 9; // info-ui: was 8
    pub const ALL: [PcAction; PcAction::COUNT] = [
        PcAction::TouchClick,
        PcAction::Options,
        PcAction::BottomScreen,
        PcAction::FastForward,
        PcAction::FreeCamera,
        PcAction::CameraReset,
        PcAction::CameraZoomIn,
        PcAction::CameraZoomOut,
        PcAction::GrowthRates, // info-ui
    ];
}

/// the keys or buttons one thing is bound to, any of which does it. In the
/// settings file a single name or a list; names this build does not know
/// are skipped rather than losing the whole file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binds<T>(pub Vec<T>);

impl<T> Default for Binds<T> {
    fn default() -> Self {
        Binds(Vec::new())
    }
}

impl<T: Copy + PartialEq> Binds<T> {
    pub fn of(list: &[T]) -> Binds<T> {
        Binds(list.to_vec())
    }
    pub fn none() -> Binds<T> {
        Binds(Vec::new())
    }
    pub fn contains(&self, item: T) -> bool {
        self.0.contains(&item)
    }
    pub fn first(&self) -> Option<T> {
        self.0.first().copied()
    }
    /// binds to `item` alone.
    pub fn set(&mut self, item: T) {
        self.0 = vec![item];
    }
    /// binds to `item` too.
    pub fn add(&mut self, item: T) {
        if !self.contains(item) {
            self.0.push(item);
        }
    }
    pub fn remove(&mut self, item: T) {
        self.0.retain(|bound| *bound != item);
    }
    pub fn clear(&mut self) {
        self.0.clear();
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<T: Serialize> Serialize for Binds<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de, T: DeserializeOwned> Deserialize<'de> for Binds<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum OneOrMany {
            One(String),
            Many(Vec<String>),
        }
        let names = match OneOrMany::deserialize(deserializer)? {
            OneOrMany::One(name) => vec![name],
            OneOrMany::Many(names) => names,
        };
        let parse = |name: &String| {
            let parsed = T::deserialize(name.as_str().into_deserializer());
            parsed
                .inspect_err(|_: &serde::de::value::Error| {
                    if !name.is_empty() && name != "None" && name != "Unknown" {
                        log::warn!("the settings bind {name}, which this build does not know; left out");
                    }
                })
                .ok()
        };
        Ok(Binds(names.iter().filter_map(parse).collect()))
    }
}

/// what a key is called, for menus and prompts.
pub fn key_name(key: KeyCode) -> String {
    let name = format!("{key:?}");
    let fixed = match key {
        KeyCode::ShiftLeft => "Left Shift",
        KeyCode::ShiftRight => "Right Shift",
        KeyCode::ControlLeft => "Left Ctrl",
        KeyCode::ControlRight => "Right Ctrl",
        KeyCode::AltLeft => "Left Alt",
        KeyCode::AltRight => "Right Alt",
        KeyCode::Backquote => "`",
        KeyCode::Space => "Space",
        _ => "",
    };
    if !fixed.is_empty() {
        return fixed.to_owned();
    }
    for prefix in ["Key", "Digit", "Arrow"] {
        if let Some(rest) = name.strip_prefix(prefix) {
            return rest.to_owned();
        }
    }
    name
}

impl Binds<KeyCode> {
    pub fn label(&self) -> String {
        if self.0.is_empty() {
            return "-".into();
        }
        self.0.iter().map(|key| key_name(*key)).collect::<Vec<_>>().join(" / ")
    }
}

impl Binds<PadButton> {
    /// the stored buttons' names on a controller of `family`.
    pub fn label(&self, family: Family) -> String {
        self.label_on(family, true)
    }

    /// the names of the physical buttons these are on a controller of
    /// `family` with the given layout.
    pub fn label_on(&self, family: Family, positional: bool) -> String {
        if self.0.is_empty() {
            return "-".into();
        }
        self.0.iter().map(|button| physical(*button, positional).name(family)).collect::<Vec<_>>().join(" / ")
    }
}

/// the keys the console's buttons, circle pad and C-stick are on, and the
/// port's own keyboard actions. Escape is not bindable to the others: it is
/// the menus' Back, and the options menu's key unless that is rebound
/// (qol4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Keys {
    pub a: Binds<KeyCode>,
    pub b: Binds<KeyCode>,
    pub x: Binds<KeyCode>,
    pub y: Binds<KeyCode>,
    pub l: Binds<KeyCode>,
    pub r: Binds<KeyCode>,
    pub start: Binds<KeyCode>,
    pub select: Binds<KeyCode>,
    pub up: Binds<KeyCode>,
    pub down: Binds<KeyCode>,
    pub left: Binds<KeyCode>,
    pub right: Binds<KeyCode>,
    pub circle_up: Binds<KeyCode>,
    pub circle_down: Binds<KeyCode>,
    pub circle_left: Binds<KeyCode>,
    pub circle_right: Binds<KeyCode>,
    pub zl: Binds<KeyCode>,
    pub zr: Binds<KeyCode>,
    pub c_up: Binds<KeyCode>,
    pub c_down: Binds<KeyCode>,
    pub c_left: Binds<KeyCode>,
    pub c_right: Binds<KeyCode>,
    pub bottom_screen: Binds<KeyCode>,
    pub fast_forward: Binds<KeyCode>,
    /// which default scheme the bindings were made from; 0 for files from
    /// before the PC scheme.
    #[serde(default)]
    pub scheme: u32,
    // cameras: the battle free camera's keys (files without them get the
    // defaults, through serde(default) on the struct)
    pub free_camera: Binds<KeyCode>,
    pub camera_reset: Binds<KeyCode>,
    pub camera_zoom_in: Binds<KeyCode>,
    pub camera_zoom_out: Binds<KeyCode>,
    // info-ui: the growth rates on the lower screen's unit status
    pub growth_rates: Binds<KeyCode>,
    // qol4: the key that opens and closes the options menu (Escape; files
    // without it get it, through serde(default) on the struct)
    pub options: Binds<KeyCode>,
}

impl Default for Keys {
    fn default() -> Self {
        use KeyCode::*;
        Keys {
            a: Binds::of(&[Enter, Space, KeyZ]),
            b: Binds::of(&[Backspace, KeyX]),
            x: Binds::of(&[KeyF, KeyC]),
            y: Binds::of(&[ShiftLeft, KeyV]),
            l: Binds::of(&[KeyQ]),
            r: Binds::of(&[KeyE]),
            start: Binds::of(&[Tab]),
            select: Binds::of(&[ControlLeft, ShiftRight]),
            up: Binds::of(&[ArrowUp, KeyW]),
            down: Binds::of(&[ArrowDown, KeyS]),
            left: Binds::of(&[ArrowLeft, KeyA]),
            right: Binds::of(&[ArrowRight, KeyD]),
            circle_up: Binds::of(&[KeyI]),
            circle_down: Binds::of(&[KeyK]),
            circle_left: Binds::of(&[KeyJ]),
            circle_right: Binds::of(&[KeyL]),
            zl: Binds::none(),
            zr: Binds::none(),
            c_up: Binds::none(),
            c_down: Binds::none(),
            c_left: Binds::none(),
            c_right: Binds::none(),
            // bottom: M, for the map and unit info the bottom screen shows
            // (Tab is Start in this scheme)
            bottom_screen: Binds::of(&[KeyM]),
            fast_forward: Binds::of(&[Backquote]),
            scheme: SCHEME,
            // cameras
            free_camera: Binds::of(&[KeyG]),
            camera_reset: Binds::of(&[KeyH]),
            camera_zoom_in: Binds::of(&[Equal]),
            camera_zoom_out: Binds::of(&[Minus]),
            // info-ui: R for growth rates (G is the free camera's)
            growth_rates: Binds::of(&[KeyR]),
            options: Binds::of(&[Escape]), // qol4
        }
    }
}

impl Keys {
    /// the Zakuro defaults from before the PC scheme, to tell whether a
    /// settings file was ever customised.
    fn legacy() -> Keys {
        use KeyCode::*;
        let one = |key| Binds::of(&[key]);
        Keys {
            a: one(KeyX),
            b: one(KeyZ),
            x: one(KeyS),
            y: one(KeyA),
            l: one(KeyQ),
            r: one(KeyW),
            start: one(Enter),
            select: one(Backspace),
            up: one(ArrowUp),
            down: one(ArrowDown),
            left: one(ArrowLeft),
            right: one(ArrowRight),
            circle_up: one(KeyI),
            circle_down: one(KeyK),
            circle_left: one(KeyJ),
            circle_right: one(KeyL),
            zl: one(Digit1),
            zr: one(Digit2),
            c_up: one(KeyT),
            c_down: one(KeyG),
            c_left: one(KeyF),
            c_right: one(KeyH),
            ..Keys::default()
        }
    }

    /// whether anything is on key.
    pub fn binds(&self, key: KeyCode) -> bool {
        self.clone().all_mut().into_iter().any(|(_, bound)| bound.contains(key))
    }

    /// every binding with the name it goes by, to show and change them.
    pub fn all_mut(&mut self) -> Vec<(&'static str, &mut Binds<KeyCode>)> {
        vec![
            ("A", &mut self.a),
            ("B", &mut self.b),
            ("X", &mut self.x),
            ("Y", &mut self.y),
            ("L", &mut self.l),
            ("R", &mut self.r),
            ("Start", &mut self.start),
            ("Select", &mut self.select),
            ("D-pad up", &mut self.up),
            ("D-pad down", &mut self.down),
            ("D-pad left", &mut self.left),
            ("D-pad right", &mut self.right),
            ("Circle pad up", &mut self.circle_up),
            ("Circle pad down", &mut self.circle_down),
            ("Circle pad left", &mut self.circle_left),
            ("Circle pad right", &mut self.circle_right),
            ("ZL", &mut self.zl),
            ("ZR", &mut self.zr),
            ("C-stick up", &mut self.c_up),
            ("C-stick down", &mut self.c_down),
            ("C-stick left", &mut self.c_left),
            ("C-stick right", &mut self.c_right),
            ("Bottom screen", &mut self.bottom_screen),
            ("Game Speed", &mut self.fast_forward), // qol: switches the game speed
            // cameras
            ("Free camera", &mut self.free_camera),
            ("Camera reset", &mut self.camera_reset),
            ("Camera zoom in", &mut self.camera_zoom_in),
            ("Camera zoom out", &mut self.camera_zoom_out),
            ("Growth rates", &mut self.growth_rates), // info-ui
            (OPTIONS_ROW, &mut self.options), // qol4: last, so the rows above keep their places
        ]
    }

    /// qol4: binds the row at index to key alone, as the Controls page does.
    /// The options menu's key is kept apart from the rest: a key given to it
    /// leaves every other row (opening the menu never also presses a game
    /// button), a key given to another row leaves it, and it is never left
    /// without one (Escape then, its default, which no other row can take).
    pub fn assign(&mut self, index: usize, key: KeyCode) {
        {
            let mut all = self.all_mut();
            let Some(name) = all.get(index).map(|(name, _)| *name) else { return };
            all[index].1.set(key);
            for (other, bound) in all.iter_mut() {
                if *other != name && (name == OPTIONS_ROW || *other == OPTIONS_ROW) {
                    bound.remove(key);
                }
            }
        }
        if self.options.is_empty() {
            self.options.set(KeyCode::Escape);
        }
    }

    /// qol4: whether the row at index is the options menu's.
    pub fn is_options_row(&self, index: usize) -> bool {
        self.clone().all_mut().get(index).is_some_and(|(name, _)| *name == OPTIONS_ROW)
    }

    /// each of the console's buttons and its keys.
    pub fn map(&self) -> [(PadState, &Binds<KeyCode>); 14] {
        [
            (PadState::A, &self.a),
            (PadState::B, &self.b),
            (PadState::X, &self.x),
            (PadState::Y, &self.y),
            (PadState::L, &self.l),
            (PadState::R, &self.r),
            (PadState::START, &self.start),
            (PadState::SELECT, &self.select),
            (PadState::UP, &self.up),
            (PadState::DOWN, &self.down),
            (PadState::LEFT, &self.left),
            (PadState::RIGHT, &self.right),
            (PadState::ZL, &self.zl),
            (PadState::ZR, &self.zr),
        ]
    }

    /// the console's buttons the held keys press.
    pub fn buttons(&self, held: &HashSet<KeyCode>) -> PadState {
        let mut state = PadState::empty();
        for (button, keys) in self.map() {
            if keys.0.iter().any(|key| held.contains(key)) {
                state |= button;
            }
        }
        state
    }

    /// the circle pad and C-stick the held keys push, holding two opposite
    /// keys cancels out as a real stick would.
    pub fn sticks(&self, held: &HashSet<KeyCode>) -> ([f32; 2], [f32; 2]) {
        let on = |binds: &Binds<KeyCode>| binds.0.iter().any(|key| held.contains(key));
        let axis = |plus: bool, minus: bool| plus as i8 as f32 - minus as i8 as f32;
        (
            [axis(on(&self.circle_right), on(&self.circle_left)), axis(on(&self.circle_up), on(&self.circle_down))],
            [axis(on(&self.c_right), on(&self.c_left)), axis(on(&self.c_up), on(&self.c_down))],
        )
    }

    pub fn action(&self, action: PcAction) -> Option<&Binds<KeyCode>> {
        match action {
            PcAction::BottomScreen => Some(&self.bottom_screen),
            PcAction::FastForward => Some(&self.fast_forward),
            // qol4: the options menu's key is read where it is pressed
            // (App::qol4_options_key), not as a held action
            PcAction::TouchClick | PcAction::Options => None,
            // cameras
            PcAction::FreeCamera => Some(&self.free_camera),
            PcAction::CameraReset => Some(&self.camera_reset),
            PcAction::CameraZoomIn => Some(&self.camera_zoom_in),
            PcAction::CameraZoomOut => Some(&self.camera_zoom_out),
            PcAction::GrowthRates => Some(&self.growth_rates), // info-ui
        }
    }
}

/// the controller buttons the console's buttons and the port's actions are
/// on, by position (see the module's notes). The circle pad is the left
/// stick, and the right stick is the bottom screen's touch pointer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PadButtons {
    pub a: Binds<PadButton>,
    pub b: Binds<PadButton>,
    pub x: Binds<PadButton>,
    pub y: Binds<PadButton>,
    pub l: Binds<PadButton>,
    pub r: Binds<PadButton>,
    pub start: Binds<PadButton>,
    pub select: Binds<PadButton>,
    pub up: Binds<PadButton>,
    pub down: Binds<PadButton>,
    pub left: Binds<PadButton>,
    pub right: Binds<PadButton>,
    pub zl: Binds<PadButton>,
    pub zr: Binds<PadButton>,
    pub touch_click: Binds<PadButton>,
    pub options: Binds<PadButton>,
    pub bottom_screen: Binds<PadButton>,
    pub fast_forward: Binds<PadButton>,
    #[serde(default)]
    pub scheme: u32,
    // cameras: the battle free camera's buttons
    pub free_camera: Binds<PadButton>,
    pub camera_reset: Binds<PadButton>,
    pub camera_zoom_in: Binds<PadButton>,
    pub camera_zoom_out: Binds<PadButton>,
    // info-ui: unbound, as Game Speed: the 3DS uses every button; the
    // touch pointer presses the on-screen button
    pub growth_rates: Binds<PadButton>,
}

impl Default for PadButtons {
    fn default() -> Self {
        use PadButton::*;
        PadButtons {
            a: Binds::of(&[East]),
            b: Binds::of(&[South]),
            x: Binds::of(&[North]),
            y: Binds::of(&[West]),
            // Fates has no use for ZL/ZR, so the triggers are a second L/R
            l: Binds::of(&[LeftShoulder, LeftTrigger]),
            r: Binds::of(&[RightShoulder, RightTrigger]),
            start: Binds::of(&[Start]),
            select: Binds::of(&[Back]),
            up: Binds::of(&[DPadUp]),
            down: Binds::of(&[DPadDown]),
            left: Binds::of(&[DPadLeft]),
            right: Binds::of(&[DPadRight]),
            zl: Binds::none(),
            zr: Binds::none(),
            touch_click: Binds::of(&[RightStick]),
            options: Binds::of(&[Guide]),
            bottom_screen: Binds::of(&[LeftStick]),
            fast_forward: Binds::none(),
            scheme: SCHEME,
            // cameras: Share / Capture / Mic or the touchpad flips it; R3,
            // the touch pointer's tap, gives the game's camera back while
            // the free camera has the right stick. The shoulders and
            // triggers (the 3DS L/R) raise and lower the fly camera, so the
            // zoom steps have no button of their own
            free_camera: Binds::of(&[Misc1, Touchpad]),
            camera_reset: Binds::of(&[RightStick]),
            camera_zoom_in: Binds::none(),
            camera_zoom_out: Binds::none(),
            growth_rates: Binds::none(), // info-ui
        }
    }
}

impl PadButtons {
    fn legacy() -> PadButtons {
        use PadButton::*;
        let one = |button| Binds::of(&[button]);
        PadButtons {
            l: one(LeftShoulder),
            r: one(RightShoulder),
            zl: one(LeftTrigger),
            zr: one(RightTrigger),
            ..PadButtons::default()
        }
    }

    /// every binding with the name it goes by, to show and change them.
    pub fn all_mut(&mut self) -> Vec<(&'static str, &mut Binds<PadButton>)> {
        vec![
            ("A", &mut self.a),
            ("B", &mut self.b),
            ("X", &mut self.x),
            ("Y", &mut self.y),
            ("L", &mut self.l),
            ("R", &mut self.r),
            ("Start", &mut self.start),
            ("Select", &mut self.select),
            ("D-pad up", &mut self.up),
            ("D-pad down", &mut self.down),
            ("D-pad left", &mut self.left),
            ("D-pad right", &mut self.right),
            ("ZL", &mut self.zl),
            ("ZR", &mut self.zr),
            ("Touch (pointer)", &mut self.touch_click),
            (OPTIONS_ROW, &mut self.options), // qol4: by name, as Keys' row
            ("Bottom screen", &mut self.bottom_screen),
            ("Game Speed", &mut self.fast_forward), // qol: switches the game speed
            // cameras
            ("Free camera", &mut self.free_camera),
            ("Camera reset", &mut self.camera_reset),
            ("Camera zoom in", &mut self.camera_zoom_in),
            ("Camera zoom out", &mut self.camera_zoom_out),
            ("Growth rates", &mut self.growth_rates), // info-ui
        ]
    }

    /// each of the console's buttons and the controller buttons it is on.
    pub fn map(&self) -> [(PadState, &Binds<PadButton>); 14] {
        [
            (PadState::A, &self.a),
            (PadState::B, &self.b),
            (PadState::X, &self.x),
            (PadState::Y, &self.y),
            (PadState::L, &self.l),
            (PadState::R, &self.r),
            (PadState::START, &self.start),
            (PadState::SELECT, &self.select),
            (PadState::UP, &self.up),
            (PadState::DOWN, &self.down),
            (PadState::LEFT, &self.left),
            (PadState::RIGHT, &self.right),
            (PadState::ZL, &self.zl),
            (PadState::ZR, &self.zr),
        ]
    }

    pub fn action(&self, action: PcAction) -> &Binds<PadButton> {
        match action {
            PcAction::TouchClick => &self.touch_click,
            PcAction::Options => &self.options,
            PcAction::BottomScreen => &self.bottom_screen,
            PcAction::FastForward => &self.fast_forward,
            // cameras
            PcAction::FreeCamera => &self.free_camera,
            PcAction::CameraReset => &self.camera_reset,
            PcAction::CameraZoomIn => &self.camera_zoom_in,
            PcAction::CameraZoomOut => &self.camera_zoom_out,
            PcAction::GrowthRates => &self.growth_rates, // info-ui
        }
    }

    /// whether any binding uses button.
    pub fn uses(&self, button: PadButton) -> bool {
        self.clone().all_mut().into_iter().any(|(_, bound)| bound.contains(button))
    }

    /// qol4: binds the row at index to button alone, as the Controls page
    /// does, keeping the options menu's button apart from the rest as
    /// Keys::assign does. A controller may be left with no Options button
    /// (Start and Select together still open the menu); it gets Guide back
    /// when nothing else uses it.
    pub fn assign(&mut self, index: usize, button: PadButton) {
        {
            let mut all = self.all_mut();
            let Some(name) = all.get(index).map(|(name, _)| *name) else { return };
            all[index].1.set(button);
            for (other, bound) in all.iter_mut() {
                if *other != name && (name == OPTIONS_ROW || *other == OPTIONS_ROW) {
                    bound.remove(button);
                }
            }
        }
        if self.options.is_empty() && !self.uses(PadButton::Guide) {
            self.options.set(PadButton::Guide);
        }
    }
}

/// qol4: the options menu's row, by its name in both binding lists.
pub const OPTIONS_ROW: &str = "Options menu";

/// moves settings written before the PC scheme to it. Bindings still at
/// Zakuro's old defaults take the new ones; ones the player changed are kept.
/// Returns whether anything changed.
pub fn migrate(keys: &mut Keys, pad: &mut PadButtons) -> bool {
    let mut changed = false;
    if keys.scheme < SCHEME {
        let mut old = keys.clone();
        old.bottom_screen = Keys::legacy().bottom_screen;
        old.fast_forward = Keys::legacy().fast_forward;
        old.scheme = SCHEME;
        if old == Keys::legacy() {
            log::info!("keyboard bindings moved to the PC defaults");
            *keys = Keys::default();
        } else {
            log::info!("keyboard bindings were customised; kept as they are");
            keys.scheme = SCHEME;
        }
        changed = true;
    }
    if pad.scheme < SCHEME {
        let mut old = pad.clone();
        for action in [&mut old.touch_click, &mut old.options, &mut old.bottom_screen, &mut old.fast_forward] {
            action.clear();
        }
        let mut legacy = PadButtons::legacy();
        for action in [&mut legacy.touch_click, &mut legacy.options, &mut legacy.bottom_screen, &mut legacy.fast_forward] {
            action.clear();
        }
        old.scheme = SCHEME;
        if old == legacy {
            *pad = PadButtons::default();
        } else {
            log::info!("controller bindings were customised; kept as they are");
            pad.scheme = SCHEME;
            // the port's actions, which the old files never had
            let defaults = PadButtons::default();
            pad.touch_click = defaults.touch_click;
            pad.options = defaults.options;
            pad.bottom_screen = defaults.bottom_screen;
        }
        changed = true;
    }
    changed
}

/// one pad's raw state, by position.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PadReading {
    pub buttons: [bool; PadButton::COUNT],
    /// -1 to 1, x right and y *up*.
    pub left: [f32; 2],
    pub right: [f32; 2],
    /// 0 to 1.
    pub triggers: [f32; 2],
}

impl PadReading {
    /// whether the physical button is held, the triggers past the threshold.
    pub fn held(&self, button: PadButton) -> bool {
        match button {
            PadButton::LeftTrigger => self.triggers[0] > TRIGGER_THRESHOLD || self.buttons[button as usize],
            PadButton::RightTrigger => self.triggers[1] > TRIGGER_THRESHOLD || self.buttons[button as usize],
            _ => self.buttons[button as usize],
        }
    }

    pub fn set(&mut self, button: PadButton, held: bool) {
        self.buttons[button as usize] = held;
    }

    /// whether anything on the pad is in use, to tell the device last used.
    pub fn active(&self, deadzone: f32) -> bool {
        self.buttons.iter().any(|held| *held)
            || self.triggers.iter().any(|value| *value > TRIGGER_THRESHOLD)
            || self.left[0].hypot(self.left[1]) > deadzone.max(0.3)
            || self.right[0].hypot(self.right[1]) > deadzone.max(0.3)
    }
}

/// what a pad does, after the bindings and the layout.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PadOutput {
    pub buttons: PadState,
    /// the circle pad, from the left stick, after the dead zone.
    pub circle: [f32; 2],
    /// the right stick after the dead zone, for the touch pointer.
    pub right_stick: [f32; 2],
    /// the port's actions held, in PcAction::ALL's order.
    pub actions: [bool; PcAction::COUNT], // cameras: was 4
}

impl PadOutput {
    pub fn action(&self, action: PcAction) -> bool {
        self.actions[PcAction::ALL.iter().position(|a| *a == action).unwrap()]
    }

    /// another pad's output on top: buttons from either, the stick pushed
    /// further.
    pub fn merge(&mut self, other: &PadOutput) {
        self.buttons |= other.buttons;
        for (mine, theirs) in [(&mut self.circle, other.circle), (&mut self.right_stick, other.right_stick)] {
            if theirs[0].hypot(theirs[1]) > mine[0].hypot(mine[1]) {
                *mine = theirs;
            }
        }
        for (mine, theirs) in self.actions.iter_mut().zip(other.actions) {
            *mine |= theirs;
        }
    }
}

/// a stick with a round dead zone, scaled so the range past it still runs
/// from 0 to 1, and clamped to the unit circle.
pub fn dead_zone(stick: [f32; 2], zone: f32) -> [f32; 2] {
    let zone = zone.clamp(0.0, 0.9);
    let length = stick[0].hypot(stick[1]);
    if length <= zone || length == 0.0 {
        return [0.0, 0.0];
    }
    let scaled = ((length - zone) / (1.0 - zone)).min(1.0);
    [stick[0] / length * scaled, stick[1] / length * scaled]
}

/// the pad's physical button a binding means for a pad of this layout.
pub fn physical(bound: PadButton, positional: bool) -> PadButton {
    if positional { bound } else { bound.mirrored() }
}

/// the binding a physical button press stands for, the inverse of
/// `physical`, to store a button pressed while rebinding.
pub fn stored(pressed: PadButton, positional: bool) -> PadButton {
    // mirroring is its own inverse
    physical(pressed, positional)
}

/// resolves one pad.
pub fn resolve(binds: &PadButtons, positional: bool, reading: &PadReading, deadzone: f32) -> PadOutput {
    let held = |binds: &Binds<PadButton>| binds.0.iter().any(|bound| reading.held(physical(*bound, positional)));
    let mut buttons = PadState::empty();
    for (button, bound) in binds.map() {
        if held(bound) {
            buttons |= button;
        }
    }
    let mut actions = [false; PcAction::COUNT]; // cameras: was 4
    for (i, action) in PcAction::ALL.iter().enumerate() {
        actions[i] = held(binds.action(*action));
    }
    PadOutput { buttons, circle: dead_zone(reading.left, deadzone), right_stick: dead_zone(reading.right, deadzone), actions }
}

/// what to show for one of the console's buttons in prompts: the key or
/// controller button it is on in the given glyph set.
pub fn prompt_name(button: PadState, glyphs: GlyphSet, keys: &Keys, pad: &PadButtons, layout: ButtonLayout) -> String {
    match glyphs.family() {
        None => keys.map().iter().find(|(b, _)| *b == button).and_then(|(_, binds)| binds.first()).map_or("-".into(), key_name),
        Some(family) => pad
            .map()
            .iter()
            .find(|(b, _)| *b == button)
            .and_then(|(_, binds)| binds.first())
            .map_or("-".into(), |bound| physical(bound, family.positional(layout)).name(family).to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(pressed: &[PadButton]) -> PadReading {
        let mut reading = PadReading::default();
        for button in pressed {
            reading.set(*button, true);
        }
        reading
    }

    #[test]
    fn nintendo_pads_confirm_with_the_east_button() {
        let positional = Family::Nintendo.positional(ButtonLayout::Auto);
        assert!(positional);
        let out = resolve(&PadButtons::default(), positional, &reading(&[PadButton::East]), 0.15);
        assert_eq!(out.buttons, PadState::A);
        let out = resolve(&PadButtons::default(), positional, &reading(&[PadButton::South]), 0.15);
        assert_eq!(out.buttons, PadState::B);
        let out = resolve(&PadButtons::default(), positional, &reading(&[PadButton::North, PadButton::West]), 0.15);
        assert_eq!(out.buttons, PadState::X | PadState::Y);
    }

    #[test]
    fn xbox_and_playstation_pads_confirm_with_the_south_button() {
        for family in [Family::Xbox, Family::PlayStation, Family::Generic] {
            let positional = family.positional(ButtonLayout::Auto);
            assert!(!positional);
            let out = resolve(&PadButtons::default(), positional, &reading(&[PadButton::South]), 0.15);
            assert_eq!(out.buttons, PadState::A, "{family:?} south is confirm");
            let out = resolve(&PadButtons::default(), positional, &reading(&[PadButton::East]), 0.15);
            assert_eq!(out.buttons, PadState::B, "{family:?} east is back");
            // labels: Xbox X (west) is the 3DS X, Xbox Y (north) the 3DS Y
            let out = resolve(&PadButtons::default(), positional, &reading(&[PadButton::West]), 0.15);
            assert_eq!(out.buttons, PadState::X);
            let out = resolve(&PadButtons::default(), positional, &reading(&[PadButton::North]), 0.15);
            assert_eq!(out.buttons, PadState::Y);
        }
    }

    #[test]
    fn the_layout_setting_overrides_the_family() {
        assert!(Family::Xbox.positional(ButtonLayout::Positional));
        assert!(!Family::Nintendo.positional(ButtonLayout::Labels));
    }

    #[test]
    fn prompts_name_the_button_where_the_layout_put_it() {
        let (keys, pad) = (Keys::default(), PadButtons::default());
        let name = |set| prompt_name(PadState::A, set, &keys, &pad, ButtonLayout::Auto);
        assert_eq!(name(GlyphSet::Nintendo), "A");
        assert_eq!(name(GlyphSet::Xbox), "A");
        assert_eq!(name(GlyphSet::PlayStation), "Cross");
        assert_eq!(name(GlyphSet::Keyboard), "Enter");
        assert_eq!(prompt_name(PadState::B, GlyphSet::PlayStation, &keys, &pad, ButtonLayout::Auto), "Circle");
        assert_eq!(prompt_name(PadState::X, GlyphSet::Xbox, &keys, &pad, ButtonLayout::Auto), "X");
        assert_eq!(prompt_name(PadState::L, GlyphSet::PlayStation, &keys, &pad, ButtonLayout::Auto), "L1");
        // forcing positional on an Xbox pad puts A on the B button
        assert_eq!(prompt_name(PadState::A, GlyphSet::Xbox, &keys, &pad, ButtonLayout::Positional), "B");
    }

    #[test]
    fn rebinding_stores_by_position() {
        // an Xbox player (labels) presses the bottom button for A: stored
        // as East, which an Xbox pad reads back as South
        let stored = stored(PadButton::South, false);
        assert_eq!(stored, PadButton::East);
        assert_eq!(physical(stored, false), PadButton::South);
    }

    #[test]
    fn triggers_are_a_second_l_and_r() {
        let mut r = PadReading::default();
        r.triggers = [0.9, 0.2];
        let out = resolve(&PadButtons::default(), true, &r, 0.15);
        assert_eq!(out.buttons, PadState::L);
        r.triggers = [0.0, 0.6];
        assert_eq!(resolve(&PadButtons::default(), false, &r, 0.15).buttons, PadState::R);
    }

    #[test]
    fn the_left_stick_is_the_circle_pad_with_a_dead_zone() {
        let mut r = PadReading::default();
        r.left = [0.1, 0.05];
        assert_eq!(resolve(&PadButtons::default(), true, &r, 0.15).circle, [0.0, 0.0]);
        r.left = [1.0, 0.0];
        assert_eq!(resolve(&PadButtons::default(), true, &r, 0.15).circle, [1.0, 0.0]);
        r.left = [0.0, -0.575];
        let circle = resolve(&PadButtons::default(), true, &r, 0.15).circle;
        assert!((circle[1] + 0.5).abs() < 1e-4, "scaled past the zone, {circle:?}");
        // the d-pad still works alongside
        r.set(PadButton::DPadUp, true);
        assert!(resolve(&PadButtons::default(), true, &r, 0.15).buttons.contains(PadState::UP));
    }

    #[test]
    fn the_right_stick_and_its_click_are_for_the_touch_pointer() {
        let mut r = reading(&[PadButton::RightStick]);
        r.right = [0.0, 1.0];
        let out = resolve(&PadButtons::default(), true, &r, 0.15);
        assert!(out.action(PcAction::TouchClick));
        assert_eq!(out.right_stick, [0.0, 1.0]);
        assert_eq!(out.buttons, PadState::empty(), "nothing of the console's");
        assert!(resolve(&PadButtons::default(), true, &reading(&[PadButton::Guide]), 0.15).action(PcAction::Options));
    }

    #[test]
    fn many_pads_merge() {
        let mut a = resolve(&PadButtons::default(), true, &reading(&[PadButton::East]), 0.15);
        let mut r = reading(&[PadButton::Start]);
        r.left = [0.0, 1.0];
        let b = resolve(&PadButtons::default(), false, &r, 0.15);
        a.merge(&b);
        assert_eq!(a.buttons, PadState::A | PadState::START);
        assert_eq!(a.circle, [0.0, 1.0]);
    }

    #[test]
    fn families_from_sdl() {
        use crate::sdl::{label, pad_type as t};
        assert_eq!(Family::from_sdl(t::SWITCH_PRO, label::B, "Nintendo Switch Pro Controller"), Family::Nintendo);
        assert_eq!(Family::from_sdl(t::JOYCON_PAIR, label::B, "Joy-Con (L/R)"), Family::Nintendo);
        assert_eq!(Family::from_sdl(t::JOYCON_LEFT, label::UNKNOWN, "Joy-Con (L)"), Family::Nintendo);
        assert_eq!(Family::from_sdl(t::PS5, label::CROSS, "DualSense Wireless Controller"), Family::PlayStation);
        assert_eq!(Family::from_sdl(t::PS4, label::CROSS, "PS4 Controller"), Family::PlayStation);
        assert_eq!(Family::from_sdl(t::XBOXONE, label::A, "Xbox Series X Controller"), Family::Xbox);
        // NSO SNES and third-party Switch pads label the bottom button B
        assert_eq!(Family::from_sdl(t::STANDARD, label::B, "SNES Controller"), Family::Nintendo);
        assert_eq!(Family::from_sdl(t::STANDARD, label::A, "Generic USB Joystick"), Family::Xbox);
        assert_eq!(Family::from_sdl(t::UNKNOWN, label::UNKNOWN, "Generic USB Joystick"), Family::Generic);
        assert_eq!(Family::from_name("Wireless Controller (DualShock 4)"), Some(Family::PlayStation));
    }

    #[test]
    fn auto_glyphs_follow_the_device_last_used() {
        assert_eq!(GlyphSet::resolve(Glyphs::Auto, Device::Keyboard), GlyphSet::Keyboard);
        assert_eq!(GlyphSet::resolve(Glyphs::Auto, Device::Pad(Family::PlayStation)), GlyphSet::PlayStation);
        assert_eq!(GlyphSet::resolve(Glyphs::Auto, Device::Pad(Family::Generic)), GlyphSet::Xbox);
        assert_eq!(GlyphSet::resolve(Glyphs::Nintendo, Device::Pad(Family::Xbox)), GlyphSet::Nintendo);
    }

    #[test]
    fn default_keys_follow_the_pc_scheme() {
        let keys = Keys::default();
        let held = |keys_down: &[KeyCode]| keys.buttons(&keys_down.iter().copied().collect());
        for key in [KeyCode::Enter, KeyCode::Space, KeyCode::KeyZ] {
            assert_eq!(held(&[key]), PadState::A);
        }
        for key in [KeyCode::Backspace, KeyCode::KeyX] {
            assert_eq!(held(&[key]), PadState::B);
        }
        assert_eq!(held(&[KeyCode::KeyW]), PadState::UP);
        assert_eq!(held(&[KeyCode::ArrowLeft, KeyCode::KeyD]), PadState::LEFT | PadState::RIGHT);
        assert_eq!(held(&[KeyCode::KeyQ, KeyCode::KeyE]), PadState::L | PadState::R);
        assert_eq!(held(&[KeyCode::Tab]), PadState::START);
        assert_eq!(held(&[KeyCode::ShiftLeft]), PadState::Y);
        // qol4: Escape is the options menu's key alone
        assert_eq!(keys.options, Binds::of(&[KeyCode::Escape]));
        assert!(!keys.map().iter().any(|(_, binds)| binds.contains(KeyCode::Escape)), "Escape is the options menu's");
        // every key does one thing at most
        let mut seen = HashSet::new();
        for (name, binds) in keys.clone().all_mut() {
            for key in &binds.0 {
                assert!(seen.insert(*key), "{key:?} bound twice, again for {name}");
            }
        }
    }

    #[test]
    fn old_settings_files_still_load_and_move_to_the_new_scheme() {
        #[derive(Deserialize)]
        struct File {
            keys: Keys,
            pad: PadButtons,
        }
        let text = "[keys]\na = \"KeyX\"\nb = \"KeyZ\"\nx = \"KeyS\"\ny = \"KeyA\"\nl = \"KeyQ\"\nr = \"KeyW\"\nstart = \"Enter\"\nselect = \"Backspace\"\n\
                    up = \"ArrowUp\"\ndown = \"ArrowDown\"\nleft = \"ArrowLeft\"\nright = \"ArrowRight\"\ncircle_up = \"KeyI\"\ncircle_down = \"KeyK\"\n\
                    circle_left = \"KeyJ\"\ncircle_right = \"KeyL\"\nzl = \"Digit1\"\nzr = \"Digit2\"\nc_up = \"KeyT\"\nc_down = \"KeyG\"\nc_left = \"KeyF\"\nc_right = \"KeyH\"\n\
                    [pad]\na = \"East\"\nb = \"South\"\nx = \"North\"\ny = \"West\"\nl = \"LeftTrigger\"\nr = \"RightTrigger\"\nstart = \"Start\"\nselect = \"Select\"\n\
                    up = \"DPadUp\"\ndown = \"DPadDown\"\nleft = \"DPadLeft\"\nright = \"DPadRight\"\nzl = \"LeftTrigger2\"\nzr = \"RightTrigger2\"\n";
        let mut file: File = toml::from_str(text).unwrap();
        assert_eq!(file.keys.a, Binds::of(&[KeyCode::KeyX]));
        assert_eq!(file.pad.l, Binds::of(&[PadButton::LeftShoulder]));
        assert!(migrate(&mut file.keys, &mut file.pad));
        assert_eq!(file.keys, Keys::default());
        assert_eq!(file.pad, PadButtons::default());
        assert!(!migrate(&mut file.keys, &mut file.pad), "once");

        // customised bindings stay
        let custom = text.replace("a = \"KeyX\"", "a = \"KeyP\"").replace("a = \"East\"", "a = \"South\"");
        let mut file: File = toml::from_str(&custom).unwrap();
        migrate(&mut file.keys, &mut file.pad);
        assert_eq!(file.keys.a, Binds::of(&[KeyCode::KeyP]));
        assert_eq!(file.pad.a, Binds::of(&[PadButton::South]));
        assert_eq!(file.pad.touch_click, Binds::of(&[PadButton::RightStick]));
    }

    #[test]
    fn unknown_names_are_skipped_and_lists_round_trip() {
        #[derive(Deserialize, Serialize, PartialEq, Debug)]
        struct File {
            pad: PadButtons,
        }
        let file: File = toml::from_str("[pad]\na = [\"East\", \"Bogus\"]\nb = \"Unknown\"\n").unwrap();
        assert_eq!(file.pad.a, Binds::of(&[PadButton::East]));
        assert!(file.pad.b.is_empty());
        let text = toml::to_string(&file).unwrap();
        assert_eq!(toml::from_str::<File>(&text).unwrap(), file);
    }

    #[test]
    fn sdl_button_numbers() {
        assert_eq!(PadButton::from_sdl(0), Some(PadButton::South));
        assert_eq!(PadButton::from_sdl(1), Some(PadButton::East));
        assert_eq!(PadButton::from_sdl(2), Some(PadButton::West));
        assert_eq!(PadButton::from_sdl(3), Some(PadButton::North));
        assert_eq!(PadButton::from_sdl(5), Some(PadButton::Guide));
        assert_eq!(PadButton::from_sdl(8), Some(PadButton::RightStick));
        assert_eq!(PadButton::from_sdl(10), Some(PadButton::RightShoulder));
        assert_eq!(PadButton::from_sdl(14), Some(PadButton::DPadRight));
        assert_eq!(PadButton::from_sdl(20), Some(PadButton::Touchpad));
        assert_eq!(PadButton::from_sdl(21), None);
        assert_eq!(PadButton::LeftTrigger.sdl(), None);
    }
}

// qol4: the options menu's key and button, rebound
#[cfg(test)]
mod qol4_tests {
    use super::*;

    fn row(keys: &Keys, name: &str) -> usize {
        keys.clone().all_mut().iter().position(|(n, _)| *n == name).unwrap()
    }

    #[test]
    fn the_options_key_is_kept_apart() {
        let mut keys = Keys::default();
        let options = row(&keys, OPTIONS_ROW);
        assert!(keys.is_options_row(options));
        // Tab given to the options menu leaves Start
        keys.assign(options, KeyCode::Tab);
        assert_eq!(keys.options, Binds::of(&[KeyCode::Tab]));
        assert!(!keys.start.contains(KeyCode::Tab));
        // and given back to Start, the menu falls back to Escape
        keys.assign(row(&keys, "Start"), KeyCode::Tab);
        assert_eq!(keys.start, Binds::of(&[KeyCode::Tab]));
        assert_eq!(keys.options, Binds::of(&[KeyCode::Escape]));
        // a key given elsewhere leaves the other rows as they were
        keys.assign(row(&keys, "A"), KeyCode::KeyM);
        assert!(keys.bottom_screen.contains(KeyCode::KeyM));
    }

    #[test]
    fn the_options_button_is_kept_apart() {
        let mut pad = PadButtons::default();
        let options = pad.clone().all_mut().iter().position(|(n, _)| *n == OPTIONS_ROW).unwrap();
        let start = pad.clone().all_mut().iter().position(|(n, _)| *n == "Start").unwrap();
        pad.assign(options, PadButton::Start);
        assert_eq!(pad.options, Binds::of(&[PadButton::Start]));
        assert!(pad.start.is_empty());
        pad.assign(start, PadButton::Start);
        // Guide comes back to the menu, nothing else using it
        assert_eq!(pad.options, Binds::of(&[PadButton::Guide]));
    }

    #[test]
    fn an_old_file_gets_escape() {
        let old: Keys = toml::from_str("a = [\"Enter\"]
").unwrap();
        assert_eq!(old.options, Binds::of(&[KeyCode::Escape]));
        assert_eq!(old.a, Binds::of(&[KeyCode::Enter]));
    }
}
