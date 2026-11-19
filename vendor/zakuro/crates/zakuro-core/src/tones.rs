//! tones: Corrin's speaking style, the Japanese game's talk tones, on the US
//! build with a Japanese-script translation (Team If, the Uncensored Patch).
//! See dev/reports/tones.md and tones-research.md.
//!
//! **The Japanese feature.** Kamui's voice, picked in the avatar creator,
//! sets one of three speaking styles (口調); every line it changes has
//! three labels per gender, `<label>_PCM1/2/3` and `_PCF1/2/3`, and
//! `TalkUtil::GetTalkTone` gives the suffix. On the US build that function
//! still reads the table but its tone is the constant 0, and a US save has
//! no tone.
//!
//! **How Haiku does it.**
//!
//! - *The text* (zakuro::haiku_fanpatch::lay_text): the US game asks for
//!   `_PCM1`/`_PCF1` only, and the other tones are whole scenes the US
//!   game's memory has no room for. So for each message archive with tone
//!   variants Haiku's text layer holds the archive three times, one per
//!   style, the style's text under the tone-1 labels, all three the same
//!   size; `zakuro_fs::layered::ALTERNATE` picks which one a read gets.
//! - *The style*: the byte at `unit::Edit+0x2D` (padding on the US build;
//!   0, 1, 2 for tone 1, 2, 3). overrides/sunglasses.c keeps it in the save
//!   in bits 4 and 5 of the edit's version byte (a 3DS reads that version
//!   as 4 and the tone is ignored). overrides/tones.c sets it when the
//!   creator's Voice step picks a voice (the Japanese game's way: a voice is
//!   a style), and asks this module to [`sync`] before the game loads a
//!   message archive (`Mess::FileLoad`, `FileLoadRoute`).
//! - *The options*: Options > Gameplay > Speaking Style changes Corrin's
//!   byte through [`request`]; the next archive loaded is in that style.
//!
//! Nothing here runs without such a translation turned on ([`available`]).

use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};
use std::sync::Mutex;

use recomp_abi::Context;
use zakuro_cpu::Bus;

use crate::memory::Memory;

/// the overrides' operations (overrides/tones.c, TN_*)
pub mod op {
    /// a message archive is about to be loaded
    pub const SYNC: u32 = 1;
}

/// the edit's speaking style byte (0, 1, 2: tones 1, 2, 3)
pub const EDIT_TONE: u32 = 0x2D;
/// the edit's gender byte (0 male, 1 female)
const EDIT_GENDER: u32 = 0x20;
/// title::ProcMyUnitEdit*, the avatar creator (qol's MY_UNIT_EDIT); its edit
/// is at +0x38 (ChangedSelect hands it to SetVoiceType)
const MY_UNIT_EDIT: u32 = 0x006F_EC10;
const CREATOR_EDIT: u32 = 0x38;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static ACTIVE_WORD: AtomicPtr<u32> = AtomicPtr::new(std::ptr::null_mut());
/// the styles' names in the translation's words: male, then female
static NAMES: Mutex<Option<[[String; 3]; 2]>> = Mutex::new(None);
/// a style the options asked for (1 to 3), 0 when none is waiting
static REQUEST: AtomicU32 = AtomicU32::new(0);
/// what the last sync saw: bits 0-1 the style (1 to 3, 0 unknown), bit 2
/// female, bit 3 from a loaded game's Corrin (else the creator's)
static SEEN: AtomicU32 = AtomicU32::new(0);
/// frames since the last sync from [`frame`]
static TICK: AtomicU32 = AtomicU32::new(0);

/// the style names the Japanese game's text gives when a translation's
/// own can't be read (Team If 4.3's words).
pub fn default_names() -> [[String; 3]; 2] {
    [["Spirited", "Mature", "Gentle"].map(String::from), ["Polite", "Youthful", "Proud"].map(String::from)]
}

/// the word overrides/tones.c reads: 1 while a translation with speaking
/// styles is on.
///
/// # Safety
///
/// the pointer has to stay valid for the program's life.
pub unsafe fn set_active_word(word: *mut u32) {
    ACTIVE_WORD.store(word, Ordering::Relaxed);
    update_word();
}

fn update_word() {
    let word = ACTIVE_WORD.load(Ordering::Relaxed);
    if !word.is_null() {
        // SAFETY: set_active_word's contract; a plain word the code only reads
        unsafe { std::ptr::write_volatile(word, ACTIVE.load(Ordering::Relaxed) as u32) };
    }
}

/// the text layer has archives in several speaking styles (some: their
/// names), or none (None: a plain game or a mod without them). Called once
/// per start, before the game's code runs.
pub fn set_available(names: Option<[[String; 3]; 2]>) {
    let on = names.is_some();
    if let Ok(mut held) = NAMES.lock() {
        *held = names;
    }
    ACTIVE.store(on, Ordering::Relaxed);
    if !on {
        zakuro_fs::layered::ALTERNATE.store(0, Ordering::Relaxed);
        SEEN.store(0, Ordering::Relaxed);
    }
    update_word();
}

/// whether a translation with speaking styles is on (and the overrides'
/// hooks are in: FATES_TONES=0 leaves them out).
pub fn available() -> bool {
    ACTIVE.load(Ordering::Relaxed) && !ACTIVE_WORD.load(Ordering::Relaxed).is_null()
}

/// the three styles' names for a male (false) or female (true) Corrin.
pub fn names(female: bool) -> [String; 3] {
    let held = NAMES.lock().ok().and_then(|names| names.clone()).unwrap_or_else(default_names);
    held[female as usize].clone()
}

/// Corrin's style as last seen (1 to 3) and gender (true: female), and
/// whether it is a loaded game's Corrin (else the creator's); None before
/// either is seen.
pub fn current() -> Option<(u8, bool, bool)> {
    let seen = SEEN.load(Ordering::Relaxed);
    let style = (seen & 3) as u8;
    (style != 0).then_some((style, seen & 4 != 0, seen & 8 != 0))
}

/// the options chose a style (1 to 3) for the loaded game's Corrin: set at
/// the next frame, so the next archive the game loads is in it.
pub fn request(style: u8) {
    if (1..=3).contains(&style) {
        REQUEST.store(style as u32, Ordering::Relaxed);
        // shown at once, while the game catches up
        let seen = SEEN.load(Ordering::Relaxed);
        if seen & 8 != 0 {
            SEEN.store((seen & !3) | style as u32, Ordering::Relaxed);
        }
    }
}

/// snapshots (FATES_UI_SNAPSHOT, after the run): the loaded game's Corrin
/// shown with `style`, its gender as last seen.
pub fn show(style: u8) {
    let seen = SEEN.load(Ordering::Relaxed);
    SEEN.store((seen & 4) | 8 | (style as u32 & 3), Ordering::Relaxed);
}

/// the edit whose style counts: the loaded game's Corrin's, else the
/// creator's while it is open; and whether it is the game's.
fn edit(m: &mut Memory) -> Option<(u32, bool)> {
    if let Some(edit) = crate::supports::corrin_edit(m) {
        return Some((edit, true));
    }
    let pointer = crate::region::us(MY_UNIT_EDIT);
    if !m.is_readable(pointer, 4) {
        return None;
    }
    let creator = m.read32(pointer);
    if creator == 0 || !m.is_readable(creator + CREATOR_EDIT, 0x30) {
        return None;
    }
    Some((creator + CREATOR_EDIT, false))
}

/// brings the text layer's style in line with Corrin's (and sets one the
/// options asked for).
pub fn sync(m: &mut Memory) {
    if !available() {
        return;
    }
    let Some((at, game)) = edit(m) else { return };
    let gender = m.read8(at + EDIT_GENDER);
    if gender > 1 {
        return;
    }
    if let Some(style) = forced() {
        m.write8(at + EDIT_TONE, style - 1);
    }
    let request = REQUEST.swap(0, Ordering::Relaxed);
    if game && (1..=3).contains(&request) {
        let before = m.read8(at + EDIT_TONE);
        m.write8(at + EDIT_TONE, request as u8 - 1);
        if before != request as u8 - 1 {
            log::info!("tones: Corrin's speaking style is now {} ({}), saved with the game", request, names(gender == 1)[request as usize - 1]);
        }
    }
    let mut tone = m.read8(at + EDIT_TONE);
    if tone > 2 {
        tone = 0;
    }
    let seen = (tone as u32 + 1) | (gender as u32) << 2 | (game as u32) << 3;
    let before = SEEN.swap(seen, Ordering::Relaxed);
    if zakuro_fs::layered::ALTERNATE.swap(tone as usize, Ordering::Relaxed) != tone as usize || before != seen {
        log::info!(
            "tones: the text in speaking style {} ({}) for {} Corrin{}",
            tone + 1,
            names(gender == 1)[tone as usize],
            if gender == 1 { "a female" } else { "a male" },
            if game { "" } else { " (the avatar creator's)" }
        );
    }
}

/// between two frames: a style the options asked for is set, and now and
/// then the layer follows Corrin (a save loaded, a state loaded).
pub fn frame(m: &mut Memory) {
    if !available() {
        return;
    }
    let tick = TICK.fetch_add(1, Ordering::Relaxed);
    // dev: HAIKU_TONES_SET_AT=<frame>:<style> asks for a style at a frame,
    // as the options would (tests of a change mid-game and of the save)
    if let Some((at, style)) = std::env::var("HAIKU_TONES_SET_AT").ok().and_then(|v| {
        let (a, b) = v.split_once(':')?;
        Some((a.trim().parse::<u64>().ok()?, b.trim().parse::<u8>().ok()?))
    }) {
        if crate::native_gfx::HEADLESS_FRAME.load(Ordering::Relaxed) == at {
            log::info!("tones: frame {at}: style {style} asked for (HAIKU_TONES_SET_AT)");
            request(style);
        }
    }
    if REQUEST.load(Ordering::Relaxed) != 0 || tick % 30 == 0 {
        sync(m);
    }
}

/// the hook the overrides call (haiku_tones_hook).
///
/// # Safety
///
/// ctx has to be the context of a run of recompiled code, which the
/// overrides are only ever given.
pub unsafe extern "C" fn hook(ctx: *mut std::ffi::c_void, op: u32, a: u32, _b: u32) -> u32 {
    // SAFETY: as above, the system is the one the code runs in
    let system = unsafe { &mut *crate::recompiled::system_of(ctx as *mut Context) };
    if op == op::SYNC {
        sync(&mut system.memory);
        // dev: HAIKU_TONES_TRACE=1 names each archive loaded and its style
        if std::env::var_os("HAIKU_TONES_TRACE").is_some() {
            let m = &mut system.memory;
            let mut name = Vec::new();
            while name.len() < 64 && a != 0 && m.is_readable(a + name.len() as u32, 1) {
                let byte = m.read8(a + name.len() as u32);
                if byte == 0 {
                    break;
                }
                name.push(byte);
            }
            let name: String = name.iter().map(|&b| if b.is_ascii() { (b as char).to_string() } else { format!("<{b:02x}>") }).collect();
            log::info!("tones: frame {} load {name} in style {}", crate::native_gfx::HEADLESS_FRAME.load(Ordering::Relaxed), zakuro_fs::layered::ALTERNATE.load(Ordering::Relaxed) + 1);
        }
    }
    0
}

/// dev: HAIKU_TONES_FORCE=<1-3> sets Corrin's style whenever it is seen (the
/// loaded game's or the creator's), for tests.
fn forced() -> Option<u8> {
    static FORCED: std::sync::OnceLock<Option<u8>> = std::sync::OnceLock::new();
    *FORCED.get_or_init(|| std::env::var("HAIKU_TONES_FORCE").ok().and_then(|v| v.parse().ok()).filter(|v| (1..=3).contains(v)))
}
