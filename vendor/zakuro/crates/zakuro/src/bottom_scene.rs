//! bottom2: what Fates is showing on its lower screen, read from the game's
//! own state, so the Auto mode can bring the bottom screen out when it
//! matters and put it away when it does not.
//!
//! Fates runs everything as a tree of `ProcInst`s ("procs"), each created
//! with a name (`ProcInst::Create(desc, name, block, parent)`, 0x0010A1F4 and
//! 0x004EE0BC). `Proc::FindByName` (0x003D27D0) walks that tree from three
//! roots, one per priority, in the array at 0x00752B48. A proc is:
//!
//! ```text
//! +0x00 vtable        +0x04 desc          +0x08 desc cursor
//! +0x0C name (char*)  +0x10 parent        +0x14 first child
//! +0x18 prev sibling  +0x1C next sibling  +0x28 flags (bit 0: dying)
//! +0x2C name hash (h = h*31 + c, 0 made 1)
//! ```
//!
//! The scenes that put something on the lower screen worth looking at are
//! procs by name (found by logging the tree through the replays, see
//! dev/reports/bottom2.md): `ProcTutorialViewer` (the tutorial pages, closed
//! by touch or a button), `SequenceTargetSelect` (choosing whom to attack,
//! the lower screen showing the target), `UnitListMapMenu` (the unit
//! list, the lower screen showing the chosen unit), `ProcMyUnitEdit` (the
//! avatar creator: its touch grids are on the lower screen while none of
//! its own menus, which are on the top screen with the lower one blank, is
//! up: `LooksMenu` the summary, `CategoryMenu`/`CharacterMenu` the name,
//! `CharaMenu` the talent, `GameDialogYesNo`, `GameMessage`). Combat comes first: `SequenceBattle`,
//! `BattleSeq` and `ProcBattleInfo` (the battle animation, its lower screen
//! the HP, hit, crit and damage of both sides and the speed buttons),
//! `ProcBattle` (map combat with animations off), and after it
//! `ExpSequence`, `GrowSequence` and `ProcGrowUp` (the EXP bar and the
//! level-up), so it stays out until those are done. `ProcMovie` is purely
//! decorative and puts it away at once.
//!
//! On the map, the lower screen is `ProcGameInfo`, its singleton at
//! 0x006D526C. Its viewer (`ProcGameInfo::ChangeViewer`, 0x001BB7FC) is the
//! byte at +0x30: 0 none, 1 a unit's status, 2 the other unit's (a pair-up's
//! or target's), 3 the situation (the map overview). +0x1058 and +0x105C
//! count suspends. It keeps the last unit the cursor was on, so whether the
//! map cursor is on a unit right now is read as the game's own
//! `FreeCursorTick` (0x00351904) does: the cursor (`map::Cursor::Get`,
//! *0x006D6EC4) holds its tile as floats at +0x18 and +0x1C, and the map's
//! image (*0x006D73E8) has a byte per tile at +0x08 + (x | y << 5), the
//! unit's index in the unit pool (`UnitPool::Get`, *0x006D9F40, 0x140 bytes
//! a unit, its flags at +0x08, bit 0 a unit not on the map). Only while the
//! player moves the cursor (`MapSequenceHuman`) does that count.
//!
//! Everything here is a plain read of guest memory, no hook and no
//! recompile, and only for Fates (USA Special Edition v2).

use std::collections::{BTreeSet, HashMap};
use std::io::Write;

use zakuro_core::memory::Memory;

/// Fates USA, whose addresses these are.
pub const FATES_USA: u64 = 0x0004_0000_0017_9800;

/// the proc tree's three roots.
const PROC_ROOTS: u32 = 0x0075_2B48;
/// the ProcGameInfo singleton's pointer.
const GAME_INFO: u32 = 0x006D_526C;
/// Map::Get's map, the map cursor and the map's image.
const MAP: u32 = 0x006D_72F4;
const MAP_CURSOR: u32 = 0x006D_6EC4;
const MAP_IMAGE: u32 = 0x006D_73E8;
const UNIT_POOL: u32 = 0x006D_9F40;

/// the most procs walked, so a corrupt link cannot loop forever.
const MAX_PROCS: usize = 4096;

fn read_u32(memory: &mut Memory, addr: u32) -> Option<u32> {
    if addr == 0 || addr & 3 != 0 || !memory.is_readable(addr, 4) {
        return None;
    }
    let mut word = [0u8; 4];
    memory.read_bytes(addr, &mut word);
    Some(u32::from_le_bytes(word))
}

fn read_u8(memory: &mut Memory, addr: u32) -> Option<u8> {
    if addr == 0 || !memory.is_readable(addr, 1) {
        return None;
    }
    let mut byte = [0u8; 1];
    memory.read_bytes(addr, &mut byte);
    Some(byte[0])
}

fn read_name(memory: &mut Memory, addr: u32) -> Option<String> {
    if addr == 0 || !memory.is_readable(addr, 1) {
        return None;
    }
    let mut bytes = [0u8; 64];
    let readable = if memory.is_readable(addr, 64) { 64 } else { (1..=64u32).take_while(|n| memory.is_readable(addr, *n)).last().unwrap_or(0) as usize };
    memory.read_bytes(addr, &mut bytes[..readable]);
    let end = bytes[..readable].iter().position(|&b| b == 0)?;
    let name = &bytes[..end];
    (!name.is_empty() && name.iter().all(|b| b.is_ascii_graphic() || *b == b' ')).then(|| String::from_utf8_lossy(name).into_owned())
}

/// a proc name's hash, as ProcInst::Create keeps it at +0x2C.
pub const fn name_hash(name: &str) -> u32 {
    let bytes = name.as_bytes();
    let mut hash: u32 = 0;
    let mut i = 0;
    while i < bytes.len() {
        hash = hash.wrapping_mul(31).wrapping_add(bytes[i] as i8 as i32 as u32);
        i += 1;
    }
    if hash == 0 { 1 } else { hash }
}

/// every live proc: its address and name hash, depth first from each root.
pub fn live_procs(memory: &mut Memory, out: &mut Vec<(u32, u32)>) {
    out.clear();
    let mut stack = Vec::with_capacity(32);
    for root in 0..3 {
        if let Some(proc) = read_u32(memory, zakuro_core::region::us(PROC_ROOTS) + root * 4).filter(|p| *p != 0) {
            stack.push(proc);
        }
    }
    let mut seen = 0;
    while let Some(proc) = stack.pop() {
        seen += 1;
        if seen > MAX_PROCS {
            break;
        }
        let mut head = [0u8; 0x30];
        if !memory.is_readable(proc, 0x30) {
            continue;
        }
        memory.read_bytes(proc, &mut head);
        let word = |at: usize| u32::from_le_bytes(head[at..at + 4].try_into().expect("four bytes"));
        if word(0x1C) != 0 {
            stack.push(word(0x1C));
        }
        if head[0x28] & 1 != 0 {
            continue;
        }
        if word(0x14) != 0 {
            stack.push(word(0x14));
        }
        out.push((proc, word(0x2C)));
    }
}

/// the map's lower screen, ProcGameInfo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GameInfo {
    /// 0 none, 1 a unit, 2 the other unit, 3 the situation.
    pub viewer: u8,
    /// it is suspended (not ticking), e.g. under a tutorial or a battle.
    pub suspended: bool,
    /// the map cursor is on a unit on the map.
    pub cursor_on_unit: bool,
}

impl GameInfo {
    pub fn read(memory: &mut Memory) -> Option<GameInfo> {
        let this = read_u32(memory, zakuro_core::region::us(GAME_INFO)).filter(|p| *p != 0)?;
        let viewer = read_u8(memory, this + 0x30)?;
        let suspends = [read_u32(memory, this + 0x1058)? as i32, read_u32(memory, this + 0x105C)? as i32];
        Some(GameInfo { viewer, suspended: suspends.iter().any(|n| *n > 0), cursor_on_unit: cursor_on_unit(memory).unwrap_or(false) })
    }
}

/// whether the map cursor's tile has a unit on it, as FreeCursorTick asks.
fn cursor_on_unit(memory: &mut Memory) -> Option<bool> {
    read_u32(memory, zakuro_core::region::us(MAP)).filter(|p| *p != 0)?;
    let cursor = read_u32(memory, zakuro_core::region::us(MAP_CURSOR)).filter(|p| *p != 0)?;
    let image = read_u32(memory, zakuro_core::region::us(MAP_IMAGE)).filter(|p| *p != 0)?;
    let x = f32::from_bits(read_u32(memory, cursor + 0x18)?);
    let y = f32::from_bits(read_u32(memory, cursor + 0x1C)?);
    if !(0.0..32.0).contains(&x) || !(0.0..32.0).contains(&y) {
        return Some(false);
    }
    let index = read_u8(memory, image + 8 + (x as u32 | (y as u32) << 5))?;
    if index == 0 {
        return Some(false);
    }
    let pool = read_u32(memory, zakuro_core::region::us(UNIT_POOL)).filter(|p| *p != 0)?;
    let unit = pool + (index as u32 - 1) * 0x140;
    Some(read_u32(memory, unit + 8)? & 1 == 0)
}

/// why the bottom screen is worth bringing out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// the map cursor is on a unit: its status card.
    UnitInfo,
    /// choosing whom to attack, heal or trade with: the target's card.
    Target,
    /// the map's unit list: the chosen unit's card.
    UnitList,
    /// a tutorial page, read and closed on the lower screen.
    Tutorial,
    /// the avatar creator's touch grids.
    AvatarCreator,
    /// a battle, animated or on the map, to its EXP bar and level-up.
    Combat,
    /// playtest: the conversation log (R in a conversation), which Fates
    /// shows only on the lower screen.
    Log,
    /// playtest: a message on the lower screen waiting for a key or touch
    /// (`GameMessage::CreateKeyTouchWaitLower`).
    LowerMessage,
    /// playtest: My Castle, whose lower screen is the castle's map with its
    /// buildings and materials, and the touch tabs for the army and the
    /// profile card.
    Castle,
    /// jpprep: a My Room visit's Live2D (`ProcLive2D`), where the Japanese
    /// game's petting minigame is played by touch (stroking is a mouse drag
    /// on the lower screen): on the Japanese build only.
    MyRoom,
}

impl Reason {
    pub fn name(self) -> &'static str {
        match self {
            Reason::UnitInfo => "unit info (map cursor on a unit)",
            Reason::Target => "target select (battle forecast)",
            Reason::UnitList => "unit list",
            Reason::Tutorial => "tutorial page",
            Reason::AvatarCreator => "avatar creator choices",
            Reason::Combat => "combat (forecast, HP, hit, crit)",
            Reason::Log => "conversation log",
            Reason::LowerMessage => "message on the lower screen",
            Reason::Castle => "My Castle map and tabs",
            Reason::MyRoom => "My Room (touch the visitor)", // jpprep
        }
    }

    /// the least it stays once the reason has gone: the avatar creator
    /// takes its grid away for a second and a half between its steps.
    pub fn least_linger(self) -> f32 {
        match self {
            Reason::AvatarCreator => 2.5,
            _ => 0.0,
        }
    }

    /// Haiku: whether the player lets this reason bring it out (Bottom
    /// Screen settings, "Show For").
    pub fn allowed(self, pc: &crate::settings::PcSettings) -> bool {
        match self {
            Reason::UnitInfo | Reason::UnitList => pc.bottom_show_units,
            Reason::Target => pc.bottom_show_forecast,
            Reason::Combat => pc.bottom_show_combat,
            Reason::Tutorial | Reason::AvatarCreator => pc.bottom_show_guides,
            // playtest: things read or touched on the lower screen, like a
            // tutorial page
            Reason::Log | Reason::LowerMessage | Reason::Castle | Reason::MyRoom => pc.bottom_show_guides, // jpprep: MyRoom
        }
    }

    /// whether it waits a moment before coming out, so the cursor sweeping
    /// across units does not flick it in and out.
    pub fn settle(self) -> f32 {
        match self {
            Reason::UnitInfo => 0.3,
            // playtest: the castle's menus hand over to free roam through
            // a few frames without either
            Reason::Castle => 0.3,
            // the creator's menus hand over to one another through a frame
            // without one
            Reason::AvatarCreator => 0.2,
            _ => 0.0,
        }
    }
}

/// what the lower screen shows this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Reading {
    /// something worth seeing or touching.
    pub want: Option<Reason>,
    /// only decoration (a movie): put it away at once.
    pub quiet: bool,
}

/// a battle and what follows it.
const COMBAT: [u32; 7] = [
    name_hash("SequenceBattle"),
    name_hash("BattleSeq"),
    name_hash("ProcBattleInfo"),
    name_hash("ProcBattle"),
    name_hash("ExpSequence"),
    name_hash("GrowSequence"),
    name_hash("ProcGrowUp"),
];
const PROC_MOVIE: u32 = name_hash("ProcMovie");
const TUTORIAL: u32 = name_hash("ProcTutorialViewer");
const TARGET_SELECT: u32 = name_hash("SequenceTargetSelect");
const UNIT_COMMAND: u32 = name_hash("UnitCommand");
const MY_UNIT_EDIT: u32 = name_hash("ProcMyUnitEdit");
/// the avatar creator's menus on the top screen, the lower one blank.
const CREATOR_MENUS: [u32; 9] = [
    name_hash("LooksMenu"),
    name_hash("CategoryMenu"),
    name_hash("CharacterMenu"),
    name_hash("CharaMenu"),
    name_hash("GameDialogYesNo"),
    name_hash("GameMessage"),
    name_hash("ProcTextChecker"),
    name_hash("ProcChange"),
    name_hash("FadeWait"),
];
const UNIT_LIST: u32 = name_hash("UnitListMapMenu");
/// playtest: the conversation log, a message, My Castle and a conversation.
const TALK_LOG: u32 = name_hash("TalkLogViewer");
const GAME_MESSAGE: u32 = name_hash("GameMessage");
const CASTLE: u32 = name_hash("CastleSeq");
/// jpprep: My Room's Live2D view (the petting minigame on the Japanese game)
const LIVE2D: u32 = name_hash("ProcLive2D");
const TALK: u32 = name_hash("ProcTalkManager");
/// a GameMessage's byte that CreateKeyTouchWaitLower sets to 1 (0x0019A9F8):
/// the message is on the lower screen.
const MESSAGE_LOWER: u32 = 0x60;
const MAP_HUMAN: u32 = name_hash("MapSequenceHuman");

/// reads the game each frame.
#[derive(Default)]
pub struct Probe {
    procs: Vec<(u32, u32)>,
    pub info: Option<GameInfo>,
}

impl Probe {
    pub fn read(&mut self, memory: &mut Memory) -> Reading {
        live_procs(memory, &mut self.procs);
        let has = |hash: u32| self.procs.iter().any(|(_, h)| *h == hash);
        self.info = GameInfo::read(memory);
        if has(PROC_MOVIE) {
            return Reading { want: None, quiet: true };
        }
        let want = if COMBAT.iter().any(|proc| has(*proc)) {
            Some(Reason::Combat)
        } else if zakuro_core::region::is_jpn() && has(LIVE2D) {
            Some(Reason::MyRoom) // jpprep
        } else if has(TALK_LOG) {
            Some(Reason::Log) // playtest
        } else if has(TUTORIAL) {
            Some(Reason::Tutorial)
        } else if self.procs.iter().any(|(proc, hash)| *hash == GAME_MESSAGE && read_u8(memory, proc + MESSAGE_LOWER) == Some(1)) {
            Some(Reason::LowerMessage) // playtest
        } else if has(MY_UNIT_EDIT) && !CREATOR_MENUS.iter().any(|menu| has(*menu)) {
            Some(Reason::AvatarCreator)
        } else if has(TARGET_SELECT) {
            Some(Reason::Target)
        } else if has(UNIT_COMMAND) {
            // the moved unit's card again, while the command menu, which
            // Fates puts beside the unit on the top screen, is up: nothing
            // new, and the floating window could cover the menu
            None
        } else if has(UNIT_LIST) {
            Some(Reason::UnitList)
        } else if has(MAP_HUMAN) && self.info.is_some_and(|info| !info.suspended && matches!(info.viewer, 1 | 2) && info.cursor_on_unit) {
            Some(Reason::UnitInfo)
        } else if has(CASTLE) && !has(TALK) {
            // playtest: free roam, building and the castle's own menus; a
            // conversation in the castle uses only the top screen
            Some(Reason::Castle)
        } else {
            None
        };
        Reading { want, quiet: false }
    }
}

/// bottom2: Auto's decision, from what the game shows, in seconds: it comes
/// out when something is worth seeing (after a short settle for the unit
/// card) and goes away `linger` seconds after that ended, or at once for a
/// battle or a movie.
#[derive(Debug, Clone, Default)]
pub struct AutoReveal {
    /// the reason wanted now and for how long.
    pending: Option<(Reason, f32)>,
    /// out for this reason.
    pub shown: Option<Reason>,
    /// seconds since the reason went away, while lingering.
    since_gone: f32,
    /// the player put it away while this reason held: it stays away until
    /// that reason ends.
    pub dismissed: Option<Reason>,
    /// Haiku: the player's wait before a unit's card comes out ("Show
    /// After"); None keeps the built-in 0.3 s.
    pub unit_settle: Option<f32>,
}

/// what changed, for the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Revealed(Reason),
    Hidden { after: Reason, quiet: bool },
}

impl AutoReveal {
    pub fn step(&mut self, reading: Reading, dt: f32, linger: f32) -> Option<Change> {
        match reading.want {
            Some(reason) => {
                let held = match self.pending {
                    Some((r, t)) if r == reason => t + dt,
                    _ => 0.0,
                };
                self.pending = Some((reason, held));
            }
            None => self.pending = None,
        }
        if self.dismissed.is_some() && self.dismissed != reading.want {
            self.dismissed = None;
        }
        // out already (lingering), a new reason keeps it out at once
        let out = self.shown.is_some();
        let unit_settle = self.unit_settle;
        let settle = |reason: Reason| match (reason, unit_settle) {
            (Reason::UnitInfo, Some(wait)) => wait,
            _ => reason.settle(),
        };
        let wanted = self.pending.filter(|(reason, held)| (out || *held >= settle(*reason)) && self.dismissed != Some(*reason)).map(|(reason, _)| reason);
        if reading.quiet {
            self.pending = None;
            return self.shown.take().map(|after| Change::Hidden { after, quiet: true });
        }
        match (wanted, self.shown) {
            (Some(reason), None) => {
                self.shown = Some(reason);
                self.since_gone = 0.0;
                Some(Change::Revealed(reason))
            }
            (Some(reason), Some(_)) => {
                self.shown = Some(reason);
                self.since_gone = 0.0;
                None
            }
            (None, Some(after)) => {
                self.since_gone += dt;
                if self.since_gone >= linger.max(after.least_linger()) || self.dismissed.is_some() {
                    self.shown = None;
                    Some(Change::Hidden { after, quiet: false })
                } else {
                    None
                }
            }
            (None, None) => None,
        }
    }

    /// the player put it away by hand.
    pub fn dismiss(&mut self) {
        if let Some((reason, _)) = self.pending {
            self.dismissed = Some(reason);
        }
        self.shown = None;
    }
}

/// a research log of the procs coming and going, FATES_BOTTOM_PROCS=file:
/// one line per frame where the set of live proc names, the map's lower
/// screen or Auto's decision changed.
pub struct ProcLog {
    out: std::io::BufWriter<std::fs::File>,
    names: HashMap<u32, String>,
    last: BTreeSet<String>,
    last_info: Option<GameInfo>,
    probe: Probe,
    auto: AutoReveal,
    last_reading: Reading,
}

impl ProcLog {
    pub fn from_env() -> Option<ProcLog> {
        let path = std::env::var_os("FATES_BOTTOM_PROCS")?;
        let out = std::fs::File::create(&path).inspect_err(|error| log::warn!("FATES_BOTTOM_PROCS: {error}")).ok()?;
        Some(ProcLog {
            out: std::io::BufWriter::new(out),
            names: HashMap::new(),
            last: BTreeSet::new(),
            last_info: None,
            probe: Probe::default(),
            auto: AutoReveal::default(),
            last_reading: Reading::default(),
        })
    }

    pub fn frame(&mut self, memory: &mut Memory, frame: u64) {
        let reading = self.probe.read(memory);
        // Auto's decision at the default linger, a game frame at a time
        let change = self.auto.step(reading, 1.0 / 60.0, 1.0);
        let mut procs = BTreeSet::new();
        for (proc, _) in &self.probe.procs {
            let Some(name_at) = read_u32(memory, proc + 0x0C) else { continue };
            let name = match self.names.get(&name_at) {
                Some(name) => name.clone(),
                None => {
                    let name = read_name(memory, name_at).unwrap_or_else(|| format!("?{name_at:08X}"));
                    self.names.insert(name_at, name.clone());
                    name
                }
            };
            procs.insert(name);
        }
        // boot3, dev: FATES_BOOT3_PROCWATCH=<name> writes that proc's words
        // +0x70..+0x90 (a ProcThread's running flag, timer and state) every
        // 60 frames
        if frame % 60 == 0 {
            if let Some(watch) = std::env::var_os("FATES_BOOT3_PROCWATCH") {
                for (proc, _) in &self.probe.procs {
                    let Some(name_at) = read_u32(memory, proc + 0x0C) else { continue };
                    if self.names.get(&name_at).is_some_and(|name| watch.to_string_lossy() == name.as_str()) {
                        let words: Vec<String> = (0..9).map(|k| read_u32(memory, proc + 0x70 + 4 * k).map_or("?".into(), |w| format!("{w:08X}"))).collect();
                        let delta = read_u32(memory, zakuro_core::region::us(0x006D_CA9C)).map(|w| w as i32);
                        let _ = writeln!(self.out, "{frame} watch {proc:08X} +70: {}; frame ms (0x006DCA9C) {delta:?}", words.join(" "));
                    }
                }
            }
        }
        let info = self.probe.info;
        if procs == self.last && info == self.last_info && reading == self.last_reading && change.is_none() {
            return;
        }
        let added: Vec<&String> = procs.difference(&self.last).collect();
        let removed: Vec<&String> = self.last.difference(&procs).collect();
        let _ = write!(self.out, "{frame}");
        if info != self.last_info {
            let _ = write!(self.out, " info={info:?}");
        }
        if reading != self.last_reading {
            let _ = write!(self.out, " reading={:?}{}", reading.want, if reading.quiet { " quiet" } else { "" });
        }
        if let Some(change) = change {
            let _ = write!(self.out, " AUTO {change:?}");
        }
        if !added.is_empty() {
            let _ = write!(self.out, " +{added:?}");
        }
        if !removed.is_empty() {
            let _ = write!(self.out, " -{removed:?}");
        }
        let _ = writeln!(self.out);
        let _ = self.out.flush();
        self.last = procs;
        self.last_info = info;
        self.last_reading = reading;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_hash_as_the_game_does() {
        // h = h * 31 + c over "ab": 97 * 31 + 98
        assert_eq!(name_hash("ab"), 97 * 31 + 98);
        assert_eq!(name_hash(""), 1);
        assert_ne!(name_hash("ProcTutorialViewer"), name_hash("ProcGameInfo"));
    }

    fn want(reason: Reason) -> Reading {
        Reading { want: Some(reason), quiet: false }
    }

    #[test]
    fn auto_settles_lingers_and_hides_for_a_movie() {
        let mut auto = AutoReveal::default();
        let dt = 1.0 / 60.0;
        // the unit card waits a moment
        let mut revealed_at = None;
        for frame in 0..60 {
            if let Some(Change::Revealed(_)) = auto.step(want(Reason::UnitInfo), dt, 1.0) {
                revealed_at = Some(frame);
            }
        }
        assert!(revealed_at.is_some_and(|f| (17..=20).contains(&f)), "{revealed_at:?}");
        // a quick pass over a unit does not bring it out
        let mut quick = AutoReveal::default();
        for _ in 0..5 {
            assert_eq!(quick.step(want(Reason::UnitInfo), dt, 1.0), None);
        }
        assert_eq!(quick.step(Reading::default(), dt, 1.0), None);
        // lingering, the next unit keeps it out without a gap
        let mut linger = AutoReveal::default();
        for _ in 0..30 {
            linger.step(want(Reason::UnitInfo), dt, 1.0);
        }
        for _ in 0..50 {
            assert_eq!(linger.step(Reading::default(), dt, 1.0), None);
        }
        for _ in 0..30 {
            assert_eq!(linger.step(want(Reason::UnitInfo), dt, 1.0), None, "no hide and show again");
        }
        assert_eq!(linger.shown, Some(Reason::UnitInfo));
        // it lingers a second, then goes
        let mut hidden_at = None;
        for frame in 0..120 {
            if let Some(Change::Hidden { .. }) = auto.step(Reading::default(), dt, 1.0) {
                hidden_at = Some(frame);
            }
        }
        assert!(hidden_at.is_some_and(|f| (58..=61).contains(&f)), "{hidden_at:?}");
        // a tutorial comes at once, a movie sends it away at once
        assert_eq!(auto.step(want(Reason::Tutorial), dt, 1.0), Some(Change::Revealed(Reason::Tutorial)));
        assert_eq!(auto.step(Reading { want: None, quiet: true }, dt, 1.0), Some(Change::Hidden { after: Reason::Tutorial, quiet: true }));
        // combat comes at once and stays through to the end
        assert_eq!(auto.step(want(Reason::Combat), dt, 1.0), Some(Change::Revealed(Reason::Combat)));
    }

    #[test]
    fn put_away_by_hand_stays_away_for_that_reason() {
        let mut auto = AutoReveal::default();
        let dt = 1.0 / 60.0;
        assert!(matches!(auto.step(want(Reason::Tutorial), dt, 1.0), Some(Change::Revealed(_))));
        auto.dismiss();
        for _ in 0..30 {
            assert_eq!(auto.step(want(Reason::Tutorial), dt, 1.0), None);
        }
        // the next reason brings it out again
        assert_eq!(auto.step(want(Reason::Target), dt, 1.0), Some(Change::Revealed(Reason::Target)));
    }
}
