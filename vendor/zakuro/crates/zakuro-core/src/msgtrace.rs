//! fanpatch: a trace of the messages the game looks up and doesn't find
//! (`HAIKU_MSG_TRACE=1`), for checking a translation mod against what the
//! US code asks for.
//!
//! The game reads every line through `Mess::Get` / `Mess::GetNoReplace`,
//! which look the label up in the message table (`IdentHash::GetIdent` on
//! Mess's IdentHash); a label that isn't there gives an empty line. Haiku's
//! supports override already sees those lookups (overrides/supports.c,
//! for labels starting MI or MK: MID_ and MKID_, while Haiku's message hook
//! is on, which it is by default), so the trace is a few lines in its host
//! side and costs nothing when off. Each label missed is logged once, with
//! the counts so far.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// the return addresses of the lookups whose miss shows an empty line
/// (USA): Mess::Get's and Mess::GetNoReplace's first lookup. The others
/// (Mess::IsExist, the "%s_COM" lookups) ask whether a label exists.
const GET: u32 = 0x003D_1D08;
const GET_NO_REPLACE: u32 = 0x003D_16E8;

static LOOKUPS: AtomicU64 = AtomicU64::new(0);
static MISSED: Mutex<Option<HashSet<Vec<u8>>>> = Mutex::new(None);

/// whether HAIKU_MSG_TRACE is set.
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("HAIKU_MSG_TRACE").is_some())
}

/// a lookup of label (Shift JIS) the game made from return address lr
/// (the running build's), answered with node (0: not found, by the game's
/// table or Haiku's variants).
pub fn lookup(label: &[u8], lr: u32, node: u32) {
    let from = crate::region::to_us(lr);
    let what = match from {
        GET => "Mess::Get",
        GET_NO_REPLACE => "Mess::GetNoReplace",
        _ => return,
    };
    let looked = LOOKUPS.fetch_add(1, Ordering::Relaxed) + 1;
    if node != 0 {
        return;
    }
    let mut missed = MISSED.lock().unwrap_or_else(|e| e.into_inner());
    let set = missed.get_or_insert_with(HashSet::new);
    if set.insert(label.to_vec()) {
        log::warn!("msgtrace: no message {} ({what}): {} labels missed, of {looked} lookups so far", sjis_lossy(label), set.len());
    }
}

/// the label as text: ASCII as it is, other bytes in hex (Shift JIS).
fn sjis_lossy(label: &[u8]) -> String {
    let mut out = String::new();
    for &b in label {
        if b.is_ascii_graphic() {
            out.push(b as char);
        } else {
            out.push_str(&format!("\\x{b:02X}"));
        }
    }
    out
}
