//! pulsefix: the console's state that lives outside `System`, in the
//! program's own C: the overrides' variables (overrides/keep.c lists them)
//! and the tails waiting for their originals to return (recomp.h,
//! CALL_THEN). A thread's registers can say it is in the middle of a chain
//! of guest calls an override makes; only the override's record says where
//! in it. An override can hold a guest object it made (info_ui.c's line
//! VertexBuffer). Going back to a moment, or loading a saved state, has to
//! put these back with memory and the registers, or the game goes on with
//! an override that believes another moment.
//!
//! The program linking the overrides gives the hooks (`set_hooks`), and
//! `Rewind` (rewind.rs) and `System::save_state`/`load_state` (mod.rs)
//! keep and put back what `keep` takes. Without hooks (other programs,
//! tests) there is nothing to keep.

use std::ffi::{c_char, CStr};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// overrides/keep.c's KeepRegion.
#[repr(C)]
pub struct Region {
    pub name: *const c_char,
    pub at: *mut u8,
    pub size: u32,
}

/// the tails registry's RecompKeptTail (3dsrecomp codegen/tails.rs).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tail {
    pub tail: u64,
    pub name: u64,
    pub tls: u32,
    pub ret: u32,
    pub sp: u32,
    pub words: [u32; 6],
}

/// what the program gives.
#[derive(Clone, Copy)]
pub struct Hooks {
    /// haiku_keep_regions
    pub regions: unsafe extern "C" fn(*mut *const Region) -> u32,
    /// recomp_tails_keep
    pub tails_keep: unsafe extern "C" fn(*mut Tail, u32) -> u32,
    /// recomp_tails_put
    pub tails_put: unsafe extern "C" fn(*const Tail, u32),
}

struct Known {
    hooks: Hooks,
    /// each variable: its name, where it is, how long, and what it held when
    /// the program started (for a state that has none of them)
    regions: Vec<(String, usize, usize, Vec<u8>)>,
}

// SAFETY: the addresses are the program's own statics, touched only by the
// thread holding the console (the guest's threads run one at a time, and
// the frontend keeps and puts back between frames)
unsafe impl Send for Known {}
unsafe impl Sync for Known {}

static KNOWN: OnceLock<Known> = OnceLock::new();

/// the program's hooks, given once at start, before the game runs.
pub fn set_hooks(hooks: Hooks) {
    let mut list: *const Region = std::ptr::null();
    // SAFETY: the program's haiku_keep_regions, which fills in its own list
    let count = unsafe { (hooks.regions)(&mut list) } as usize;
    let mut regions = Vec::with_capacity(count);
    for i in 0..count {
        // SAFETY: count regions at list, each a static of the program
        let region = unsafe { &*list.add(i) };
        let name = unsafe { CStr::from_ptr(region.name) }.to_string_lossy().into_owned();
        let initial = unsafe { std::slice::from_raw_parts(region.at, region.size as usize) }.to_vec();
        regions.push((name, region.at as usize, region.size as usize, initial));
    }
    log::info!("pulsefix: {} of the overrides' variables go with the console's state ({} bytes)", regions.len(), regions.iter().map(|r| r.2).sum::<usize>());
    let _ = KNOWN.set(Known { hooks, regions });
}

/// whether the program gave its hooks.
pub fn active() -> bool {
    KNOWN.get().is_some()
}

/// what was kept: each variable's bytes by name, and the tails waiting.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Kept {
    pub regions: Vec<(String, Vec<u8>)>,
    pub tails: Vec<Tail>,
}

const MAX_TAILS: usize = 64;

/// the overrides' state as it is now.
pub fn keep() -> Kept {
    let Some(known) = KNOWN.get() else { return Kept::default() };
    let regions = known
        .regions
        .iter()
        // SAFETY: the program's static, as listed
        .map(|(name, at, size, _)| (name.clone(), unsafe { std::slice::from_raw_parts(*at as *const u8, *size) }.to_vec()))
        .collect();
    let mut tails = vec![Tail::default(); MAX_TAILS];
    // SAFETY: room for MAX_TAILS records
    let count = unsafe { (known.hooks.tails_keep)(tails.as_mut_ptr(), MAX_TAILS as u32) } as usize;
    tails.truncate(count.min(MAX_TAILS));
    Kept { regions, tails }
}

/// puts back what `keep` took: each variable by its name (one this build
/// doesn't have, or of another size, is left out), and the tails in place
/// of those waiting now.
pub fn put_back(kept: &Kept) {
    let Some(known) = KNOWN.get() else { return };
    for (name, at, size, _) in &known.regions {
        if let Some((_, bytes)) = kept.regions.iter().find(|(n, _)| n == name) {
            if bytes.len() == *size {
                // SAFETY: the program's static, as listed, as long as bytes
                unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), *at as *mut u8, *size) };
            }
        }
    }
    // SAFETY: the records as recomp_tails_keep wrote them
    unsafe { (known.hooks.tails_put)(kept.tails.as_ptr(), kept.tails.len() as u32) };
}

/// what the program started with: for a saved state from before, which
/// kept none of it (nothing of this moment's own is left to mislead the
/// game: no tails, the variables as at start).
pub fn initial() -> Kept {
    let Some(known) = KNOWN.get() else { return Kept::default() };
    Kept { regions: known.regions.iter().map(|(name, _, _, initial)| (name.clone(), initial.clone())).collect(), tails: Vec::new() }
}

/// the names of the variables that differ between two, and "tails" when
/// the tails do.
pub fn differences(a: &Kept, b: &Kept) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (name, bytes) in &a.regions {
        match b.regions.iter().find(|(n, _)| n == name) {
            Some((_, other)) if other == bytes => {}
            _ => out.push(name.clone()),
        }
    }
    for (name, _) in &b.regions {
        if !a.regions.iter().any(|(n, _)| n == name) {
            out.push(name.clone());
        }
    }
    if a.tails != b.tails {
        out.push(format!("tails ({} against {})", a.tails.len(), b.tails.len()));
    }
    out
}

// ---- in a saved state's file -------------------------------------------------

/// the section's first bytes; a new layout gets new ones.
pub const SECTION_MAGIC: &[u8; 8] = b"HKKEEP01";

/// a tail as a file has it: its addresses as offsets into the program's
/// image, which are the same in every run of the same program.
#[derive(Serialize, Deserialize)]
struct FileTail {
    tail: u64,
    name: u64,
    tls: u32,
    ret: u32,
    sp: u32,
    words: [u32; 6],
}

#[derive(Serialize, Deserialize)]
struct FileKept {
    /// the program's image: its link stamp and size (the offsets hold for
    /// that one only)
    image: (u32, u32),
    regions: Vec<(String, Vec<u8>)>,
    tails: Vec<FileTail>,
}

/// where the program's image is, and its stamp: (base, size, stamp).
fn image() -> Option<(u64, u64, u32)> {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetModuleHandleW(name: *const u16) -> *const u8;
        }
        // SAFETY: the running program's own image, mapped as the loader
        // left it: the DOS header's e_lfanew, then the PE header
        unsafe {
            let base = GetModuleHandleW(std::ptr::null());
            if base.is_null() {
                return None;
            }
            let pe = base.add(std::ptr::read_unaligned(base.add(0x3C) as *const u32) as usize);
            if std::ptr::read_unaligned(pe as *const u32) != 0x0000_4550 {
                return None;
            }
            let stamp = std::ptr::read_unaligned(pe.add(8) as *const u32);
            let size = std::ptr::read_unaligned(pe.add(24 + 56) as *const u32);
            Some((base as u64, size as u64, stamp))
        }
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// the section a saved state's file keeps of `kept`. A tail whose code is
/// not in the program's image (a code mod's module) is left out.
pub fn section(kept: &Kept) -> Result<Vec<u8>, String> {
    let (base, size, stamp) = image().unwrap_or((0, 0, 0));
    let inside = |at: u64| size > 0 && at >= base && at < base + size;
    let mut tails = Vec::new();
    for tail in &kept.tails {
        if inside(tail.tail) && inside(tail.name) {
            tails.push(FileTail { tail: tail.tail - base, name: tail.name - base, tls: tail.tls, ret: tail.ret, sp: tail.sp, words: tail.words });
        } else {
            log::warn!("pulsefix: a tail outside the program's image is not kept in the saved state");
        }
    }
    let file = FileKept { image: (stamp, size as u32), regions: kept.regions.clone(), tails };
    postcard::to_extend(&file, SECTION_MAGIC.to_vec()).map_err(|error| format!("the overrides' state could not be written, {error}"))
}

/// reads a section `section` wrote. The tails are left out when the state
/// was saved by another build of the program (their offsets mean nothing
/// in this one).
pub fn read_section(bytes: &[u8]) -> Result<Kept, String> {
    let body = bytes.strip_prefix(SECTION_MAGIC.as_slice()).ok_or("it is not the overrides' state")?;
    let file: FileKept = postcard::from_bytes(body).map_err(|error| format!("it could not be read, {error}"))?;
    let mut tails = Vec::new();
    match image() {
        Some((base, size, stamp)) if (stamp, size as u32) == file.image => {
            for tail in file.tails {
                if tail.tail < size && tail.name < size {
                    tails.push(Tail { tail: base + tail.tail, name: base + tail.name, tls: tail.tls, ret: tail.ret, sp: tail.sp, words: tail.words });
                }
            }
        }
        _ if !file.tails.is_empty() => {
            log::warn!("pulsefix: the saved state's {} waiting tails are of another build of the program, and are left out", file.tails.len());
        }
        _ => {}
    }
    Ok(Kept { regions: file.regions, tails })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    use super::*;
    use crate::memory::{MemoryRegion, MemoryState, Permission};
    use crate::services::tests::system_with_thread;
    use crate::state::rewind::Rewind;

    /// an override's variable, and the tails registry, as a program has them
    static VARIABLE: AtomicU64 = AtomicU64::new(0x1111);
    static WAITING: Mutex<Vec<Tail>> = Mutex::new(Vec::new());

    struct List(Region);
    unsafe impl Sync for List {}
    unsafe impl Send for List {}
    static NAME: &[u8] = b"test.variable\0";

    unsafe extern "C" fn regions(out: *mut *const Region) -> u32 {
        static LIST: OnceLock<List> = OnceLock::new();
        let list = LIST.get_or_init(|| List(Region { name: NAME.as_ptr().cast(), at: VARIABLE.as_ptr().cast(), size: 8 }));
        *out = &list.0;
        1
    }
    unsafe extern "C" fn tails_keep(out: *mut Tail, max: u32) -> u32 {
        let waiting = WAITING.lock().unwrap();
        for (i, tail) in waiting.iter().take(max as usize).enumerate() {
            *out.add(i) = *tail;
        }
        waiting.len() as u32
    }
    unsafe extern "C" fn tails_put(kept: *const Tail, count: u32) {
        *WAITING.lock().unwrap() = std::slice::from_raw_parts(kept, count as usize).to_vec();
    }

    fn tail(ret: u32) -> Tail {
        Tail { tail: 0x1234, name: 0x5678, tls: 0x1FF8_2000, ret, sp: 0x0FFF_FE00, words: [ret, 0, 0, 0, 0, 0] }
    }

    /// the overrides' variables and the tails waiting come back with a
    /// moment (previewed and kept) and with a saved state; a state without
    /// them (from before) puts back what the program started with, never
    /// what the present left; the check says the console came back whole.
    #[test]
    fn the_overrides_state_goes_with_moments_and_states() {
        let _serial = crate::state::serial();
        set_hooks(Hooks { regions, tails_keep, tails_put });
        let (mut system, _) = system_with_thread();
        let page = system.memory.phys.allocate(MemoryRegion::Application, 0x2000).unwrap();
        system.memory.map(0x0800_0000, page.addr, 0x2000, Permission::RW, MemoryState::Private);
        let mut rewind = Rewind::new(64 << 20);
        rewind.verify = true;

        // the moment: the variable 0x2222, a tail waiting
        VARIABLE.store(0x2222, Ordering::SeqCst);
        *WAITING.lock().unwrap() = vec![tail(0x0010_0100)];
        system.memory.write_bytes(0x0800_0000, &[1; 4]);
        let first = rewind.snapshot(&mut system).unwrap().0;
        // later: another value, another tail, memory changed
        VARIABLE.store(0x3333, Ordering::SeqCst);
        *WAITING.lock().unwrap() = vec![tail(0x0010_0200), tail(0x0010_0300)];
        system.memory.write_bytes(0x0800_1000, &[2; 4]);
        let second = rewind.snapshot(&mut system).unwrap().0;
        VARIABLE.store(0x4444, Ordering::SeqCst);
        WAITING.lock().unwrap().clear();

        rewind.restore(&mut system, first, false).unwrap();
        assert_eq!(VARIABLE.load(Ordering::SeqCst), 0x2222);
        assert_eq!(*WAITING.lock().unwrap(), vec![tail(0x0010_0100)]);
        let report = rewind.last_check.take().unwrap().unwrap();
        assert!(report.whole(), "{report}");
        rewind.restore(&mut system, second, true).unwrap();
        assert_eq!(VARIABLE.load(Ordering::SeqCst), 0x3333);
        assert_eq!(WAITING.lock().unwrap().len(), 2);
        assert!(rewind.last_check.take().unwrap().unwrap().whole());

        // the old behaviour, for the check to show: the present's left
        rewind.keep_host = false;
        VARIABLE.store(0x5555, Ordering::SeqCst);
        rewind.restore(&mut system, first, false).unwrap();
        let report = rewind.last_check.take().unwrap().unwrap();
        assert!(!report.whole() && report.host.iter().any(|name| name == "test.variable"), "{report}");
        rewind.keep_host = true;

        // a saved state
        VARIABLE.store(0x6666, Ordering::SeqCst);
        *WAITING.lock().unwrap() = vec![tail(0x0010_0400)];
        let state = system.save_state().unwrap();
        VARIABLE.store(0x7777, Ordering::SeqCst);
        WAITING.lock().unwrap().clear();
        system.load_state(&state).unwrap();
        assert_eq!(VARIABLE.load(Ordering::SeqCst), 0x6666);
        // the tails' addresses here are no program's own: a file keeps only
        // those in the program's image, so none come back
        let report = system.check_loaded_state(&state).unwrap();
        assert!(report.differing.is_empty() && report.rest.is_none() && report.sound_same, "{report}");
        assert!(crate::state::tag(&state).unwrap().is_none());

        // a state from before: without the section, the variables as the
        // program started (0x1111, read when the hooks were set) and no tails
        let section = {
            let mut out = Vec::new();
            super::super::section(&mut out, &super::section(&keep()).unwrap());
            out.len()
        };
        let before = &state[..state.len() - section];
        VARIABLE.store(0x8888, Ordering::SeqCst);
        *WAITING.lock().unwrap() = vec![tail(0x0010_0500)];
        system.load_state(before).unwrap();
        assert_eq!(VARIABLE.load(Ordering::SeqCst), 0x1111);
        assert!(WAITING.lock().unwrap().is_empty());
    }
}
