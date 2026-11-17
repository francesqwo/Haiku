//! keyboard and mouse to 3DS input. Keys are tracked as held, so a button
//! bound to several keys stays down while any of them is.

use std::collections::HashSet;

use winit::keyboard::{KeyCode, PhysicalKey};
use zakuro_core::services::hid::{InputState, PadState};

use crate::controls::{Keys, PcAction};

/// accumulates key state between frames.
#[derive(Default)]
pub struct Keyboard {
    keys: Keys,
    held: HashSet<KeyCode>,
    touch: Option<(u16, u16)>,
    /// how the mouse tilts the console, see InputState::tilt.
    tilt: [f32; 2],
    /// when a key was last pressed or the mouse last touched, to tell the
    /// device used last.
    pub used: Option<std::time::Instant>,
}

impl Keyboard {
    pub fn new(keys: Keys) -> Keyboard {
        Keyboard { keys, ..Keyboard::default() }
    }

    pub fn set_keys(&mut self, keys: Keys) {
        self.keys = keys;
        self.release();
    }

    /// lets go of everything, when the game stops seeing the keyboard.
    pub fn release(&mut self) {
        self.held.clear();
        self.touch = None;
        self.tilt = [0.0; 2];
    }

    pub fn key(&mut self, key: PhysicalKey, pressed: bool) {
        let PhysicalKey::Code(code) = key else {
            return;
        };
        if pressed {
            self.used = Some(std::time::Instant::now());
            self.held.insert(code);
        } else {
            self.held.remove(&code);
        }
    }

    /// records a click on the bottom screen, in screen pixels, or its release.
    pub fn touch(&mut self, position: Option<(u16, u16)>) {
        if position.is_some() && self.touch.is_none() {
            self.used = Some(std::time::Instant::now());
        }
        self.touch = position;
    }

    /// records the tilt the mouse gives the console.
    pub fn tilt(&mut self, tilt: [f32; 2]) {
        self.tilt = tilt;
    }

    /// the port's actions the held keys do, in PcAction::ALL's order.
    pub fn actions(&self) -> [bool; PcAction::COUNT] {
        PcAction::ALL.map(|action| self.keys.action(action).is_some_and(|binds| binds.0.iter().any(|key| self.held.contains(key))))
    }

    pub fn state(&self) -> InputState {
        let (circle, c_stick) = self.keys.sticks(&self.held);
        InputState {
            buttons: self.keys.buttons(&self.held),
            circle_x: circle[0],
            circle_y: circle[1],
            c_stick_x: c_stick[0],
            c_stick_y: c_stick[1],
            touch: self.touch,
            tilt: self.tilt,
        }
    }

    pub fn buttons(&self) -> PadState {
        self.keys.buttons(&self.held)
    }
}

/// the keyboard's input with the controllers' over it: buttons from either,
/// the circle pad from whichever is pushed further.
pub fn combine(keyboard: InputState, pads: &crate::controls::PadOutput) -> InputState {
    let mut state = keyboard;
    state.buttons |= pads.buttons;
    if pads.circle[0].hypot(pads.circle[1]) > state.circle_x.hypot(state.circle_y) {
        state.circle_x = pads.circle[0];
        state.circle_y = pads.circle[1];
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::Binds;

    #[test]
    fn keys_press_the_buttons_they_are_bound_to() {
        let mut keyboard = Keyboard::new(Keys { a: Binds::of(&[KeyCode::KeyP]), ..Keys::default() });
        keyboard.key(PhysicalKey::Code(KeyCode::KeyP), true);
        assert!(keyboard.state().buttons.contains(PadState::A));
        keyboard.key(PhysicalKey::Code(KeyCode::Enter), true);
        keyboard.key(PhysicalKey::Code(KeyCode::KeyP), false);
        assert!(!keyboard.state().buttons.contains(PadState::A), "Enter is no longer bound to anything");
    }

    #[test]
    fn a_button_on_two_keys_stays_down_while_either_is() {
        let mut keyboard = Keyboard::new(Keys::default());
        keyboard.key(PhysicalKey::Code(KeyCode::Enter), true);
        keyboard.key(PhysicalKey::Code(KeyCode::Space), true);
        keyboard.key(PhysicalKey::Code(KeyCode::Enter), false);
        assert!(keyboard.state().buttons.contains(PadState::A));
        keyboard.key(PhysicalKey::Code(KeyCode::Space), false);
        assert!(!keyboard.state().buttons.contains(PadState::A));
    }

    #[test]
    fn opposite_circle_keys_cancel_out() {
        let mut keyboard = Keyboard::new(Keys::default());
        keyboard.key(PhysicalKey::Code(KeyCode::KeyJ), true);
        assert_eq!(keyboard.state().circle_x, -1.0);
        keyboard.key(PhysicalKey::Code(KeyCode::KeyL), true);
        assert_eq!(keyboard.state().circle_x, 0.0);
        keyboard.release();
        assert_eq!(keyboard.state().circle_x, 0.0);
    }

    #[test]
    fn controllers_add_to_the_keyboard() {
        let mut keyboard = Keyboard::new(Keys::default());
        keyboard.key(PhysicalKey::Code(KeyCode::KeyW), true);
        let mut pads = crate::controls::PadOutput::default();
        pads.buttons = PadState::A;
        pads.circle = [0.0, -1.0];
        let state = combine(keyboard.state(), &pads);
        assert_eq!(state.buttons, PadState::A | PadState::UP);
        assert_eq!((state.circle_x, state.circle_y), (0.0, -1.0));
        assert_eq!((state.c_stick_x, state.c_stick_y), (0.0, 0.0), "the right stick is the touch pointer's");
    }

    #[test]
    fn fast_forward_is_a_key_action() {
        let mut keyboard = Keyboard::new(Keys::default());
        keyboard.key(PhysicalKey::Code(KeyCode::Backquote), true);
        assert_eq!(keyboard.actions()[..4], [false, false, false, true]); // cameras: 8 actions now
    }
}
