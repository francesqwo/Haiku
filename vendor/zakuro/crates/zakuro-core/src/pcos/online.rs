//! offline-msg: what the player is told when an online feature cannot be
//! used.
//!
//! Every Internet failure in the game ends in one function,
//! `HomeButton::StartErrEulaApplet(ProcInst* parent, int code)`
//! (0x0017CC4C). It is called by `link::ProcShowError::Start`,
//! `link::ProcErrorReport::ShowCode` and the eShop session procs, and it
//! starts the console's error display (erreula, library applet 0x406) with
//! the error code. PC has no error display, so before this the applet
//! closed at once (pcos/applet.rs) and the player was sent back to the menu
//! without a word.
//!
//! overrides/offline_msg.c replaces that function. It asks this module how
//! to answer (op ERROR_DISPLAY):
//!
//! - NOT_AVAILABLE: the override shows the game's own message window
//!   (`GameMessage::CreateKeyWait`, the window the game uses for "Please
//!   wait and try the update again later.") with "Online features aren't
//!   available in this version." in the game's language, as a child of the
//!   same proc. The player closes it with A (Enter, Space or Z on the
//!   keyboard, A on a controller) as any of the game's messages, a touch of
//!   the lower screen, or a click anywhere else on the game (op
//!   MESSAGE_WAITING and zakuro/src/offline_msg.rs), and the game carries on
//!   exactly as it did after the error display.
//! - SYSTEM_ERROR_DISPLAY: the override runs the original, which starts the
//!   error display with the code (answered by pcos/applet.rs).
//!
//! The answer is always NOT_AVAILABLE today. A future network backend (a
//! custom server for castle visits and online battles) decides here: once it
//! is connected, a failure it reports is a real error with a real code, and
//! it answers SYSTEM_ERROR_DISPLAY so the code is shown (after drawing an
//! error display in pcos/applet.rs). See dev/reports/offline-msg.md for
//! the services such a backend has to answer.

use super::bridge::Call;
use crate::System;

/// call numbers, the same as overrides/offline_msg.c
pub mod op {
    pub const FIRST: u32 = 0x700;
    /// HomeButton::StartErrEulaApplet(ProcInst* parent, int code): how to
    /// tell the player an online feature failed; r0 = an Answer.
    pub const ERROR_DISPLAY: u32 = 0x700;
    /// GameMessage::Tick of the not-available message, once it takes the
    /// confirm button: the frontend may turn a mouse click into it.
    pub const MESSAGE_WAITING: u32 = 0x701;
    pub const LAST: u32 = 0x70F;
}

/// how a failed online feature is shown, in r0 of ERROR_DISPLAY.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Answer {
    /// the console's error display with the error code (the original).
    SystemErrorDisplay = 0,
    /// the game's message window: online features aren't available.
    NotAvailable = 1,
}

/// whether a network backend is connected that can reach a server for the
/// game's online features. There is none: pcos/net.rs answers every
/// Internet request "not connected". A backend plugs in here.
pub fn backend_connected() -> bool {
    false
}

/// the answer for an online failure with this error code.
pub fn answer_for(_code: u32) -> Answer {
    if backend_connected() { Answer::SystemErrorDisplay } else { Answer::NotAvailable }
}

/// how many times this session told the player online features aren't
/// available (for logs and tests).
static SHOWN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub fn shown() -> u32 {
    SHOWN.load(std::sync::atomic::Ordering::Relaxed)
}

/// the console frame the not-available message last waited for the
/// confirm button at (u64::MAX: never).
static WAITING_AT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(u64::MAX);

/// whether the not-available message is on screen and takes the confirm
/// button now: its GameMessage::Tick ran within the last few frames (the
/// game ticks its procs at 30 or 60 frames a second). The frontend turns a
/// mouse click on the game into the confirm button while it does
/// (zakuro/src/offline_msg.rs), as the keys and controllers already are.
pub fn message_waiting(system: &System) -> bool {
    let at = WAITING_AT.load(std::sync::atomic::Ordering::Relaxed);
    at != u64::MAX && system.frames >= at && system.frames - at <= 3
}

/// runs a call from overrides/offline_msg.c; false leaves it to the original.
pub fn call(system: &mut System, call: &mut Call, number: u32) -> bool {
    match number {
        op::ERROR_DISPLAY => {
            let code = call.arg(system, 1);
            let answer = answer_for(code);
            match answer {
                Answer::NotAvailable => {
                    SHOWN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    log::info!("online: an online feature failed (error code {code} / 0x{code:08X}); the game's message window says online features aren't available");
                }
                Answer::SystemErrorDisplay => log::info!("online: an online feature failed (error code {code} / 0x{code:08X}); the error display shows it"),
            }
            call.ret(answer as u32);
            // a function of the game's, not a request: no wait
            call.wait = None;
            super::stats::native("online.ErrorDisplay");
            true
        }
        op::MESSAGE_WAITING => {
            WAITING_AT.store(system.frames, std::sync::atomic::Ordering::Relaxed);
            call.wait = None;
            super::stats::native("online.MessageWaiting");
            true
        }
        _ => false,
    }
}
