//! info-ui: information the game knows but does not show.
//!
//! - **Enemy Targeting Lines** (`pc.targeting_lines`): on the battle map,
//!   during the player phase's free cursor, a red arc from each enemy that
//!   will attack next enemy phase to the unit it will attack, as newer Fire
//!   Emblem games show. overrides/info_ui.c asks the game's own AI
//!   (AI::Processing) on a copy-on-write view of the game's memory, and draws
//!   the arcs on the map with the game's own prim calls: thin arcs of
//!   light of constant width on screen, drawn procedurally (lines2).
//!   On or off (qol4; on by default): every enemy's lines, which follow a
//!   unit while it is moved.
//! - **Growth rates** (`pc.growth_button`): a button on the lower screen's
//!   unit status (fates_ui/growth.rs) that turns the stats window into the
//!   unit's growth rates, as the game's own Unit::GetGrow computes them.

use std::sync::OnceLock;

use crate::settings::{PcSettings, TargetingLines};

/// indices into overrides/info_ui.c's `info_ui_words`.
pub mod word {
    pub const MODE: usize = 0;
    pub const DEBUG: usize = 1;
    pub const RUNS: usize = 2;
    pub const ABORTS: usize = 3;
    pub const MICROS: usize = 4;
    pub const LINES: usize = 5;
    pub const SIMULATE: usize = 6;
    pub const GROWTH: usize = 7;
    pub const SVCS: usize = 9;
    pub const PAGES: usize = 10;
    pub const STATUS_DRAWS: usize = 11;
    pub const STATUS_UNIT: usize = 12;
    pub const GROWTH_UNIT: usize = 13;
    pub const STATUS_HELP: usize = 14;
    pub const GROWTH_RATES: usize = 16;
    pub const DECISIONS: usize = 24;
    pub const COUNT: usize = 512;
}

/// what the game build exposes of overrides/info_ui.c.
#[derive(Clone, Copy)]
pub struct Hooks {
    pub words: fn() -> *mut u32,
}

static HOOKS: OnceLock<Hooks> = OnceLock::new();

/// called once by the program, before zakuro::run.
pub fn set_hooks(hooks: Hooks) {
    let _ = HOOKS.set(hooks);
}

pub fn read(index: usize) -> Option<u32> {
    let hooks = HOOKS.get()?;
    if index >= word::COUNT {
        return None;
    }
    // SAFETY: a word of the game build's array, written by the game's
    // threads while they hold the baton; a u32 is never torn
    Some(unsafe { std::ptr::read_volatile((hooks.words)().add(index)) })
}

pub fn write(index: usize, value: u32) {
    if let Some(hooks) = HOOKS.get() {
        if index < word::COUNT {
            // SAFETY: as above
            unsafe { std::ptr::write_volatile((hooks.words)().add(index), value) }
        }
    }
}

fn env_u32(name: &str) -> Option<u32> {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok())
}

/// the lines mode: the setting, or FATES_TARGETING_LINES (0, 1, 2).
/// qol4: the setting is on or off; on is mode 1, every enemy's lines (and
/// the lines following a moved unit). Mode 2, the cursor's only, is still
/// there for FATES_TARGETING_LINES=2.
pub fn lines_mode(pc: &PcSettings) -> u32 {
    env_u32("FATES_TARGETING_LINES").unwrap_or(match pc.targeting_lines {
        TargetingLines::Off => 0,
        TargetingLines::On => 1,
    })
}

/// whether the growth button is on: the setting, or FATES_GROWTH_BUTTON.
pub fn growth_on(pc: &PcSettings) -> bool {
    env_u32("FATES_GROWTH_BUTTON").map_or(pc.growth_button, |v| v != 0)
}

/// puts the options to use, at start and after each change.
pub fn apply(pc: &PcSettings) {
    if HOOKS.get().is_none() {
        return;
    }
    write(word::MODE, lines_mode(pc));
    write(word::GROWTH, growth_on(pc) as u32);
    // tests: FATES_INFO_DEBUG=1 logs every prediction, 2 the real enemy
    // phase's decisions, 3 both; FATES_INFO_SIMULATE=0 does not move each
    // enemy before the next one thinks
    write(word::DEBUG, env_u32("FATES_INFO_DEBUG").unwrap_or(0));
    write(word::SIMULATE, env_u32("FATES_INFO_SIMULATE").unwrap_or(1));
}

/// the counts for the end of a headless run.
pub fn summary() -> Option<String> {
    HOOKS.get()?;
    let r = |i| read(i).unwrap_or(0);
    Some(format!(
        "info-ui: lines mode {}, predictions {} ({} abandoned, last {} us, {} pages copied, {} svcs), decisions {}, lines drawn {}; growth {} (unit status draws {})",
        r(word::MODE),
        r(word::RUNS),
        r(word::ABORTS),
        r(word::MICROS),
        r(word::PAGES),
        r(word::SVCS),
        r(word::DECISIONS),
        r(word::LINES),
        r(word::GROWTH),
        r(word::STATUS_DRAWS)
    ))
}

/// the growth rates of the unit the lower screen shows, from Unit::GetGrow:
/// HP, Str, Mag, Skl, Spd, Lck, Def, Res.
#[derive(Clone, Debug, PartialEq)]
pub struct Growth {
    pub unit: u32,
    pub rates: [i32; 8],
    /// how many times the unit status was drawn, for telling whether it is
    /// still on the lower screen.
    pub draws: u32,
}

pub fn growth() -> Option<Growth> {
    HOOKS.get()?;
    let unit = read(word::GROWTH_UNIT)?;
    if unit == 0 || read(word::GROWTH)? == 0 {
        return None;
    }
    let mut rates = [0i32; 8];
    for (i, rate) in rates.iter_mut().enumerate() {
        *rate = read(word::GROWTH_RATES + i)? as i32;
    }
    Some(Growth { unit, rates, draws: read(word::STATUS_DRAWS)? })
}

// ---------------------------------------------------------------------------
// the growth button: what the app keeps, and the lower screen's touches
// ---------------------------------------------------------------------------

/// ProcGameInfo, the lower screen's manager: its viewer byte (+0x30) is 1
/// or 2 while it shows a unit's status, and +0x1058, +0x105C count its
/// suspends (a class intro, a tutorial, a battle over it) (bottom2.md).
const PROC_GAME_INFO: u32 = 0x006D_526C;
/// GameConfigData: +4 bit 7 picks the unit status' simple card (the HP plate
/// low, the items) over the detailed one (Atk, Hit, EXP; the plate higher);
/// ViewerUnitStatus::Draw reads it.
const GAME_CONFIG: u32 = 0x006D_A0F8;

#[derive(Default)]
pub struct State {
    /// the growth view is on (the button or the key).
    pub view_on: bool,
    /// the lower screen shows the unit status now.
    pub status_visible: bool,
    /// the unit status' draws and the game frame they last changed at.
    draws: u32,
    changed_at: u64,
    /// the growth rates of the unit shown.
    rates: [i32; 8],
    /// the detailed card is shown, not the simple one.
    detailed: bool,
    /// a touch went down on the button and is still held.
    pressing: bool,
    /// where the lower screen is in the window, in points.
    pub bottom_screen: Option<egui::Rect>,
    /// times the button was pressed, for the log.
    pub presses: u32,
}

fn read_u32(memory: &mut zakuro_core::memory::Memory, at: u32) -> u32 {
    let mut b = [0u8; 4];
    memory.read_bytes(at, &mut b);
    u32::from_le_bytes(b)
}

impl State {
    /// reads the game, once a refresh.
    pub fn poll(&mut self, memory: &mut zakuro_core::memory::Memory, frame: u64) {
        let Some(growth) = growth() else {
            self.status_visible = false;
            return;
        };
        if growth.draws != self.draws {
            self.draws = growth.draws;
            self.changed_at = frame;
        }
        self.rates = growth.rates;
        let info = read_u32(memory, zakuro_core::region::us(PROC_GAME_INFO));
        let (viewer, suspended) = if info != 0 {
            let mut b = [0u8; 1];
            memory.read_bytes(info + 0x30, &mut b);
            (b[0], read_u32(memory, info + 0x1058) as i32 > 0 || read_u32(memory, info + 0x105C) as i32 > 0)
        } else {
            (0, true)
        };
        let config = read_u32(memory, zakuro_core::region::us(GAME_CONFIG));
        self.detailed = config != 0 && read_u32(memory, config + 4) & 0x80 == 0;
        // round 3: never over the game's own help boxes (a skill's, an
        // item's description): the button and the view step away while one
        // is up
        let help = read(word::STATUS_HELP).unwrap_or(0) != 0;
        self.status_visible = matches!(viewer, 1 | 2) && !suspended && !help && frame.saturating_sub(self.changed_at) <= 2;
    }

    /// the panel for the interface, while the button shows.
    pub fn panel(&self) -> Option<crate::fates_ui::growth::Panel> {
        self.status_visible.then(|| crate::fates_ui::growth::Panel { rates: self.rates, on: self.view_on, hover: false, detailed: self.detailed })
    }

    /// toggles the view (the key).
    pub fn toggle(&mut self) {
        if self.status_visible {
            self.view_on = !self.view_on;
            log::info!("info-ui: growth rates {}", if self.view_on { "shown" } else { "hidden" });
        }
    }

    /// a touch of the button, from the mouse, the touch pointer or a
    /// replay, presses it and never reaches the game.
    pub fn filter(&mut self, input: &mut zakuro_core::services::hid::InputState) {
        let on_button = self.status_visible
            && input.touch.is_some_and(|(x, y)| crate::fates_ui::growth::inside_button(x as f32, y as f32));
        if on_button || (self.pressing && input.touch.is_some()) {
            if !self.pressing {
                self.pressing = true;
                self.presses += 1;
                self.view_on = !self.view_on;
                log::info!("info-ui: growth button pressed, growth rates {}", if self.view_on { "shown" } else { "hidden" });
            }
            input.touch = None;
        } else {
            self.pressing = false;
        }
    }
}

static STATE: std::sync::Mutex<Option<State>> = std::sync::Mutex::new(None);

/// the growth button's state, shared by the window's loop and a headless run.
pub fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(State::default))
}

/// once a game frame, before the frame runs: reads the unit status and
/// takes a touch of the button out of the frame's input. Used by the
/// window's loop and by headless runs (a replay's touches).
pub fn frame_input(memory: &mut zakuro_core::memory::Memory, frame: u64, input: &mut zakuro_core::services::hid::InputState, headless: bool) {
    if HOOKS.get().is_none() {
        return;
    }
    with_state(|state| {
        state.poll(memory, frame);
        let presses = state.presses;
        state.filter(input);
        if state.presses != presses && headless {
            println!("info-ui: frame {frame} the growth button was pressed: growth rates {}", if state.view_on { "shown" } else { "hidden" });
        }
    });
}

impl crate::App {
    /// info-ui: once a refresh, the growth key.
    pub(crate) fn info_poll(&mut self) {
        if crate::input_api::take_action(crate::controls::PcAction::GrowthRates) {
            with_state(State::toggle);
        }
    }
}
