//! randomizer: the New Game screens that choose a run.
//!
//! After the difficulty and the mode (Phoenix / Casual / Classic), one
//! more step asks how to play. It is the game's own mode screen built
//! again with two panels (Casual's green and Classic's pink, the panels a
//! Hard game shows):
//!
//! - Normal: the game as it is (the new game goes on as the mode's A
//!   would have it);
//! - Randomizer: opens the randomizer screen.
//!
//! The randomizer screen is the mode screen once more, with its three
//! panels saying what each choice does and the footer giving the run's
//! seed:
//!
//! - Randomizer (Phoenix's orange panel): characters, story characters and
//!   classes;
//! - Characters (Casual's green panel): who appears where, classes only
//!   changed when a gender needs it;
//! - Classes (Classic's pink panel): everyone keeps who they are, with a
//!   random class.
//!
//! A there opens the "Randomize Corrin too?" step (Keep / Swap Corrin),
//! except after Classes, which moves no characters (qol5): there A starts
//! the game in the mode chosen with that randomizer; B goes
//! back to the Normal / Randomizer step, and B there back to the mode
//! screen. overrides/randomizer.c drives the screens; this answers with
//! the texts (UTF-16, in the randomizer's page) and keeps the choice until
//! the new game starts (op::NEW_GAME).
//!
//! A replay (ZAKURO_REPLAY) recorded before the step existed does not
//! answer it, so during a replay the step is skipped (Normal) unless
//! FATES_RANDO_STEP=1 says the replay answers it.

use super::{plan, seed_setting, seed_text, wstring_at, Choice, State};
use crate::System;

pub const FOOTER: u32 = 0x40;
pub const OPEN: u32 = 0x41;
pub const TEXT: u32 = 0x42;
pub const CHOOSE: u32 = 0x43;
pub const CANCEL: u32 = 0x44;
pub const STEP: u32 = 0x45;
pub const SHOWN: u32 = 0x46;
pub const POINTER: u32 = 0x47;
/// corrinswap: whether the Corrin step follows the randomizer screen
pub const CORRIN_STEP: u32 = 0x48;
/// corrinswap: the Corrin step's choice (a = 1: Corrin swaps places)
pub const CORRIN: u32 = 0x49;

/// the mouse on the mode screens (overrides/randomizer.c, rando_pointer):
/// how many panels the screen shows while it is up (0 when it is not), and
/// the click waiting for it: the panel + 1, or BACK.
static PANELS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static PICK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
pub const BACK: u32 = 0x80;

/// the panels a mode screen shows (difficulty, mode, the Normal /
/// Randomizer step, the randomizer screen), 0 while none is up.
pub fn menu_panels() -> u32 {
    PANELS.load(std::sync::atomic::Ordering::Relaxed)
}

/// the panel at a point of the upper screen, in the console's 400x240
/// (x measured from the 400-wide picture's left), when a mode screen is up.
/// The panels are 48 apart, 46 high, from y 49 with three and 73 with two
/// (as the game lays them out), between x 40 and 360 with the cursor hand.
pub fn panel_at(x: f32, y: f32) -> Option<u32> {
    let panels = menu_panels();
    if panels == 0 || !(40.0..360.0).contains(&x) {
        return None;
    }
    let top = 49.0 + (3 - panels.min(3)) as f32 * 24.0;
    let row = ((y - top) / 48.0).floor();
    (row >= 0.0 && (row as u32) < panels && y - top - row * 48.0 < 46.0).then_some(row as u32)
}

/// a click on the upper screen at (x, y), in the console's 400x240: the
/// panel under it is chosen. Answers whether it hit one.
pub fn menu_click(x: f32, y: f32) -> bool {
    let Some(panel) = panel_at(x, y) else { return false };
    PICK.store(panel + 1, std::sync::atomic::Ordering::Relaxed);
    log::info!("randomizer: a click on the mode screen's panel {panel}");
    true
}

/// back (the right button) while a mode screen is up. Answers whether one is.
pub fn menu_back() -> bool {
    if menu_panels() == 0 {
        return false;
    }
    PICK.store(BACK, std::sync::atomic::Ordering::Relaxed);
    true
}

pub(super) fn shown(panels: u32) {
    PANELS.store(panels, std::sync::atomic::Ordering::Relaxed);
    PICK.store(0, std::sync::atomic::Ordering::Relaxed);
}

pub(super) fn pick() -> u32 {
    PICK.load(std::sync::atomic::Ordering::Relaxed)
}

/// the options of the three panels: Phoenix's, Casual's, Classic's
pub const PANEL_OPTIONS: [u32; 3] = [
    plan::CHARACTERS | plan::CLASSES | plan::STORY | plan::CHILDREN,
    plan::CHARACTERS | plan::STORY | plan::CHILDREN,
    plan::CLASSES,
];
const PANEL_NAMES: [&str; 3] = ["Randomizer", "Characters", "Classes"];
const PANEL_HELP: [&str; 3] = [
    "Characters and their classes are shuffled.",
    "Characters swap places; classes stay.",
    "Everyone gets a random class.",
];
/// the Normal / Randomizer step's panels: Casual's (1) and Classic's (2)
const STEP_NAMES: [&str; 2] = ["Normal", "Randomizer"];
const STEP_HELP: [&str; 2] = ["Play the game as it is.", "Shuffle characters and classes."];
/// corrinswap: the Corrin step's panels (after the randomizer screen):
/// Casual's (1) and Classic's (2)
const CORRIN_NAMES: [&str; 2] = ["Keep Corrin", "Swap Corrin"];
const CORRIN_HELP: [&str; 2] = ["Corrin plays their own part.", "Your Corrin and someone trade places."];

/// corrinswap: whether A on the randomizer screen opens the Corrin step:
/// always, except during a replay that does not answer it (FATES_RANDO_STEP=2
/// says a replay answers both steps)
pub fn corrin_step_shown() -> bool {
    std::env::var_os("ZAKURO_REPLAY").is_none() || std::env::var("FATES_RANDO_STEP").is_ok_and(|v| v == "2")
}

/// qol5: whether a randomizer's options leave anyone for Corrin to trade
/// places with: only one that moves characters (not Classes only)
pub fn corrin_step_applies(options: u32) -> bool {
    options & plan::CHARACTERS != 0
}

/// whether A on a mode opens the Normal / Randomizer step: always, except
/// during a replay that was recorded without it
pub fn step_shown() -> bool {
    std::env::var_os("ZAKURO_REPLAY").is_none() || std::env::var("FATES_RANDO_STEP").is_ok_and(|v| v == "1" || v == "2")
}

fn new_seed() -> u32 {
    if let Some(seed) = seed_setting() {
        return seed;
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    let mut rng = plan::Rng::new(nanos as u32, nanos >> 32);
    rng.next() as u32
}

pub(super) fn call(state: &mut State, system: &mut System, op: u32, a: u32, _b: u32) -> u32 {
    match op {
        FOOTER => {
            let text = match a {
                1 => format!("Choose a randomizer. Seed: {}", seed_text(state.menu_seed)),
                2 => "Play normally, or randomized?".to_string(),
                3 => "Randomize Corrin too?".to_string(),
                _ => return 0,
            };
            wstring_at(state, system, &text).unwrap_or(0)
        }
        OPEN => {
            state.menu_seed = new_seed();
            state.pending = None;
            log::info!("randomizer: the New Game randomizer screen, seed {}", seed_text(state.menu_seed));
            1
        }
        TEXT => {
            let panel = (a & 0xFF).min(2) as usize;
            let help = (a >> 8) & 0xFF != 0;
            let text = if a >> 16 == 2 {
                let i = panel.max(1) - 1;
                if help { STEP_HELP[i] } else { STEP_NAMES[i] }
            } else if a >> 16 == 3 {
                let i = panel.max(1) - 1;
                if help { CORRIN_HELP[i] } else { CORRIN_NAMES[i] }
            } else if help {
                PANEL_HELP[panel]
            } else {
                PANEL_NAMES[panel]
            };
            wstring_at(state, system, text).unwrap_or(0)
        }
        CHOOSE => {
            let panel = (a as usize).min(2);
            // tests: FATES_RANDO_V=1 makes a run with the first rounds' rules (an old save's)
            let version = std::env::var("FATES_RANDO_V").ok().and_then(|v| v.parse().ok()).unwrap_or(plan::VERSION_CURRENT);
            state.pending = Some(Choice { seed: state.menu_seed, options: PANEL_OPTIONS[panel], version });
            log::info!("randomizer: chosen for the new game: {} (seed {})", PANEL_NAMES[panel], seed_text(state.menu_seed));
            1
        }
        CANCEL => {
            state.pending = None;
            1
        }
        // qol5: the Corrin step only follows a randomizer that moves
        // characters; with Classes only there is nobody to trade places
        // with, so A there starts the game (the bit stays off)
        CORRIN_STEP => {
            let shown = corrin_step_shown() && state.pending.as_ref().is_some_and(|choice| corrin_step_applies(choice.options));
            if !shown && state.pending.is_some() {
                log::info!("randomizer: no Corrin step: this randomizer keeps everyone in their place");
            }
            shown as u32
        }
        CORRIN => {
            if let Some(choice) = state.pending.as_mut() {
                // qol5: never with a randomizer that moves no characters
                if a != 0 && corrin_step_applies(choice.options) {
                    choice.options |= plan::CORRIN;
                }
                log::info!("randomizer: Corrin {} (options {:#x})", if a != 0 { "swaps places with someone" } else { "stays" }, choice.options);
            }
            1
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panels_under_the_pointer() {
        shown(3);
        assert_eq!(panel_at(200.0, 60.0), Some(0));
        assert_eq!(panel_at(200.0, 117.0), Some(1));
        assert_eq!(panel_at(200.0, 165.0), Some(2));
        assert_eq!(panel_at(200.0, 30.0), None);
        assert_eq!(panel_at(20.0, 117.0), None);
        shown(2);
        assert_eq!(panel_at(200.0, 90.0), Some(0));
        assert_eq!(panel_at(200.0, 141.0), Some(1));
        assert_eq!(panel_at(200.0, 60.0), None);
        assert_eq!(panel_at(200.0, 190.0), None);
        shown(0);
        assert_eq!(panel_at(200.0, 117.0), None);
    }

    /// qol5: the Corrin step follows Randomizer and Characters, not Classes
    #[test]
    fn corrin_step_only_when_characters_move() {
        assert!(corrin_step_applies(PANEL_OPTIONS[0]));
        assert!(corrin_step_applies(PANEL_OPTIONS[1]));
        assert!(!corrin_step_applies(PANEL_OPTIONS[2]));
    }
}
