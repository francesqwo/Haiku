//! offline-msg: a mouse click closes the game's "Online features aren't
//! available in this version." message.
//!
//! The message is the game's own window (overrides/offline_msg.c,
//! zakuro-core pcos/online.rs). Keys and controllers close it with A as
//! they close any of the game's messages, and a touch of the lower screen
//! closes it too. A click anywhere else on the game is its confirm button
//! while it waits: the click presses A for a few frames, through the same
//! input the keys and controllers go through, so a recording (ZAKURO_RECORD)
//! keeps it as an A press and plays back the same.

use zakuro_core::services::hid::{InputState, PadState};
use zakuro_core::System;

/// frames the click holds A, a short tap as a key press is.
const PRESS_FRAMES: u8 = 4;

#[derive(Default)]
pub struct ClickConfirm {
    left: u8,
}

impl ClickConfirm {
    /// a left click on the game that neither the interface nor the lower
    /// screen took: the confirm button while the message waits for it.
    /// true when it was.
    pub fn click(&mut self, system: &System) -> bool {
        if !zakuro_core::pcos::online::message_waiting(system) {
            return false;
        }
        log::info!("online: a click closes the not-available message (A)");
        self.left = PRESS_FRAMES;
        true
    }

    /// adds the click's A to the game's input for this frame.
    pub fn apply(&mut self, input: &mut InputState) {
        if self.left > 0 {
            self.left -= 1;
            input.buttons |= PadState::A;
        }
    }
}
