//! cpu: native-only execution of the title's code.
//!
//! the title's code runs as C that 3dsrecomp generated ahead of time. this
//! module keeps track of every place where execution would still fall to
//! Zakuro's ARM interpreter, which happens three ways:
//!
//! - **missing**: the pc is at code the recompiler did not find, so the
//!   step loop interprets it one instruction at a time;
//! - **fallback**: recompiled code hands one instruction it did not
//!   translate (an unusual coprocessor access, say) to the interpreter
//!   through the `interpret` callback;
//! - **declined**: the library has code at the pc but the budget the
//!   scheduler gave was smaller than the block, see
//!   `System::run_recompiled`, which now retries with a larger budget so
//!   this should not happen at all.
//!
//! with native-only on (the default for fates-pc, `set_native_only`), the
//! first time any of them happens the address, the thread, the registers
//! and a guest callstack are logged and the program stops with a clear
//! message instead of interpreting. `FATES_ALLOW_INTERPRETER=1` is the
//! escape hatch for diagnostics. either way `FATES_CPU_REPORT=<file>`
//! writes every site seen to a file (and `<file>.hints`, the addresses in
//! the format 3dsrecomp's hints take).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use crate::System;

static NATIVE_ONLY: AtomicBool = AtomicBool::new(false);
static NOTICE: OnceLock<fn(&str)> = OnceLock::new();

/// turns native-only execution on or off for the whole process.
pub fn set_native_only(on: bool) {
    NATIVE_ONLY.store(on, Ordering::Relaxed);
}

/// whether reaching the interpreter stops the program, always in a build
/// without one.
pub fn native_only() -> bool {
    !HAS_INTERPRETER || NATIVE_ONLY.load(Ordering::Relaxed)
}

/// whether this build has the interpreter at all.
pub const HAS_INTERPRETER: bool = cfg!(feature = "emulation") && !cfg!(feature = "strip-interpreter"); // native-final

/// a function the frontend gives to show the stop message to the player,
/// in a dialog when there is a window. called once, before the exit.
pub fn set_stop_notice(notice: fn(&str)) {
    let _ = NOTICE.set(notice);
}

/// how a site came to the interpreter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Missing,
    Fallback,
    Declined,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Missing => "missing",
            Kind::Fallback => "fallback",
            Kind::Declined => "declined",
        }
    }
}

/// one place the interpreter ran from.
#[derive(Clone, Debug)]
pub struct Site {
    pub kind: Kind,
    /// the instruction, for a fallback.
    pub opcode: u32,
    /// times execution entered the interpreter here.
    pub entries: u64,
    /// instructions interpreted from here until recompiled code ran again.
    pub instructions: u64,
    /// what it was like the first time.
    pub frame: u64,
    pub thread: String,
    pub lr: u32,
    pub callstack: Vec<u32>,
}

/// whether FATES_CPU_PROFILE=1 has the step loop time itself.
pub fn profiling() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("FATES_CPU_PROFILE").is_ok_and(|value| value == "1"))
}

/// where the time of the CPU side of a run goes, with FATES_CPU_PROFILE=1.
/// the counts are kept always, they cost nothing.
#[derive(Default, Debug, Clone)]
pub struct Profile {
    /// System::step calls.
    pub steps: u64,
    /// Library::run calls, and those made again with more budget.
    pub runs: u64,
    pub retries: u64,
    /// time inside Library::run, recompiled code and the svcs it ran in
    /// place.
    pub run: Duration,
    /// time inside the svcs recompiled code ran in place.
    pub svc_in_place: Duration,
    /// time in the svcs the step loop ran.
    pub svc_step: Duration,
    /// end of frame and audio frame work.
    pub frame_work: Duration,
    /// time in System::step altogether.
    pub step: Duration,
    /// count and time by svc number, 0x100 added for those run in place.
    pub svcs: BTreeMap<u32, (u64, Duration)>,
    /// count and time of svcSendSyncRequest by service and command.
    pub services: BTreeMap<String, (u64, Duration)>,
}

/// every site seen this run.
#[derive(Default)]
pub struct Coverage {
    pub profile: Profile,
    pub sites: BTreeMap<(Kind, u32), Site>,
    /// the site the instructions interpreted now are charged to.
    current: Option<(Kind, u32)>,
    report: Option<PathBuf>,
}

impl Coverage {
    pub fn new() -> Coverage {
        Coverage { report: std::env::var_os("FATES_CPU_REPORT").map(PathBuf::from), ..Coverage::default() }
    }

    /// recompiled code ran, what the interpreter runs next starts a new run.
    pub fn library_ran(&mut self) {
        self.current = None;
    }

    /// instructions interpreted in total, by kind.
    pub fn totals(&self) -> BTreeMap<Kind, u64> {
        let mut totals = BTreeMap::new();
        for site in self.sites.values() {
            *totals.entry(site.kind).or_insert(0) += site.instructions;
        }
        totals
    }

    /// writes the report, when one is asked for and anything is new.
    pub fn save(&mut self, frames: u64, recompiled: u64) {
        let Some(path) = &self.report else { return };
        let mut text = format!("# cpu coverage after {frames} frames, {recompiled} recompiled instructions\n");
        for (kind, total) in self.totals() {
            text += &format!("# {} {total} instructions\n", kind.name());
        }
        text += "# kind address(odd=thumb) opcode entries instructions frame thread lr callstack\n";
        for ((kind, address), site) in &self.sites {
            let stack: Vec<String> = site.callstack.iter().map(|a| format!("{a:08X}")).collect();
            text += &format!(
                "{} {address:08X} {:08X} {} {} {} {} {:08X} {}\n",
                kind.name(),
                site.opcode,
                site.entries,
                site.instructions,
                site.frame,
                site.thread.replace(' ', "_"),
                site.lr,
                stack.join(",")
            );
        }
        let hints: String = self
            .sites
            .iter()
            .filter(|((kind, _), _)| *kind == Kind::Missing)
            .map(|((_, address), _)| format!("{address:08X}\n"))
            .collect();
        let _ = std::fs::write(path, text);
        let _ = std::fs::write(path.with_extension("hints"), hints);
    }
}

impl System {
    /// lines for the end of a headless run: what the interpreter ran, and
    /// what the CPU side cost.
    /// native-final: instructions the interpreter ran in this run (a saved
    /// state's count since boot left out)
    pub fn interpreted_this_run(&self) -> u64 {
        self.coverage.totals().values().sum()
    }

    pub fn cpu_summary(&self) -> Vec<String> {
        let mut lines = Vec::new();
        let totals = self.coverage.totals();
        let fallbacks = self.recompiled.as_ref().map_or(0, |library| library.fallbacks());
        let interpreted = self.interpreted_instructions + fallbacks;
        let all = (self.interpreted_instructions + self.recompiled_instructions).max(1);
        // this run's: what a saved state it went on from carries is in the
        // totals since boot, not in the sites
        let this_run: u64 = totals.values().sum();
        lines.push(format!(
            "cpu: this run interpreted {this_run} instructions; since boot {} recompiled, {interpreted} interpreted ({:.6}%): missing {}, declined {}, fallback {}; {} sites; native-only {}",
            self.recompiled_instructions,
            interpreted as f64 * 100.0 / all as f64,
            totals.get(&Kind::Missing).copied().unwrap_or(0),
            totals.get(&Kind::Declined).copied().unwrap_or(0),
            totals.get(&Kind::Fallback).copied().unwrap_or(0),
            self.coverage.sites.len(),
            if native_only() { "on" } else { "off" }
        ));
        for ((kind, address), site) in self.coverage.sites.iter().take(40) {
            lines.push(format!(
                "cpu:   {} 0x{:08X}{} entries {} instructions {} first frame {} thread {} lr 0x{:08X}",
                kind.name(),
                address & !1,
                if address & 1 != 0 { " T" } else { "" },
                site.entries,
                site.instructions,
                site.frame,
                site.thread,
                site.lr
            ));
        }
        let p = &self.coverage.profile;
        let in_place = self.recompiled.as_ref().map_or(0, |library| library.in_place());
        let (hits, misses) = self.recompiled.as_ref().map_or((0, 0), |library| library.lookups());
        lines.push(format!(
            "cpu: {} steps, {} library runs ({} retried for budget), {in_place} svcs in place, lookups {hits} cached / {misses} searched",
            p.steps, p.runs, p.retries
        ));
        if profiling() {
            let native = p.run.saturating_sub(p.svc_in_place);
            let svcs = p.svc_in_place + p.svc_step;
            let rest = p.step.saturating_sub(p.run + p.svc_step + p.frame_work);
            lines.push(format!(
                "cpu profile: step loop {:.2?} = recompiled code {native:.2?} + svcs {svcs:.2?} (in place {:.2?}) + frame/audio work {:.2?} + scheduling and dispatch {rest:.2?}; {:.0} ns of scheduling and dispatch per library run",
                p.step,
                p.svc_in_place,
                p.frame_work,
                rest.as_nanos() as f64 / p.runs.max(1) as f64
            ));
            let mut svcs: Vec<_> = p.svcs.iter().collect();
            svcs.sort_by_key(|(_, (_, took))| std::cmp::Reverse(*took));
            for (number, (count, took)) in svcs.into_iter().take(8) {
                lines.push(format!(
                    "cpu profile:   svc 0x{:02X}{} x{count} {took:.2?}",
                    number & 0xFF,
                    if number & 0x100 != 0 { " (in place)" } else { "" }
                ));
            }
            let mut services: Vec<_> = p.services.iter().collect();
            services.sort_by_key(|(_, (_, took))| std::cmp::Reverse(*took));
            for (key, (count, took)) in services.into_iter().take(8) {
                lines.push(format!("cpu profile:   request {key} x{count} {took:.2?}"));
            }
        }
        lines
    }

    /// the step loop is about to interpret the instruction at the pc,
    /// `missing` when the library has no code there.
    pub(crate) fn coverage_interpreting(&mut self, missing: bool) {
        let (pc, thumb) = (self.cpu.regs[15], self.cpu.cpsr.thumb);
        let address = pc | thumb as u32;
        let kind = if missing { Kind::Missing } else { Kind::Declined };
        // a run of interpreted instructions is charged to where it started
        let key = match self.coverage.current {
            Some(key) if key.0 == kind => key,
            _ => {
                let key = (kind, address);
                self.coverage_enter(key, 0);
                key
            }
        };
        if let Some(site) = self.coverage.sites.get_mut(&key) {
            site.instructions += 1;
        }
        self.coverage.current = Some(key);
        if native_only() {
            self.native_only_stop(kind, address, 0);
        }
    }

    /// recompiled code handed the instruction at address to the
    /// interpreter. the cpu holds the guest's state with the pc there.
    pub(crate) fn coverage_fallback(&mut self, address: u32, opcode: u32) {
        let address = address | self.cpu.cpsr.thumb as u32;
        let key = (Kind::Fallback, address);
        self.coverage_enter(key, opcode);
        if let Some(site) = self.coverage.sites.get_mut(&key) {
            site.instructions += 1;
        }
        if native_only() {
            self.native_only_stop(Kind::Fallback, address, opcode);
        }
    }

    fn coverage_enter(&mut self, key: (Kind, u32), opcode: u32) {
        if let Some(site) = self.coverage.sites.get_mut(&key) {
            site.entries += 1;
            return;
        }
        let thread = self.kernel.current().map_or("?".to_owned(), |t| t.name.clone());
        let site = Site {
            kind: key.0,
            opcode,
            entries: 1,
            instructions: 0,
            frame: self.frames,
            thread,
            lr: self.cpu.regs[14],
            callstack: self.guest_callstack(16),
        };
        log::warn!(
            "cpu: {} code at 0x{:08X} ({}) went to the interpreter, frame {}, thread {}, lr 0x{:08X}",
            key.0.name(),
            key.1 & !1,
            if key.1 & 1 != 0 { "Thumb" } else { "ARM" },
            site.frame,
            site.thread,
            site.lr
        );
        self.coverage.sites.insert(key, site);
    }

    /// likely return addresses on the guest's stack, newest first, lr
    /// first: words in executable memory that follow a call instruction.
    pub fn guest_callstack(&mut self, limit: usize) -> Vec<u32> {
        let mut out = Vec::new();
        let lr = self.cpu.regs[14];
        if self.memory.is_executable(lr & !1) {
            out.push(lr);
        }
        let sp = self.cpu.regs[13] & !3;
        let mut words = vec![0u8; 0x1000];
        self.memory.read_bytes(sp, &mut words);
        for chunk in words.chunks_exact(4) {
            if out.len() >= limit {
                break;
            }
            let value = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            if value < 0x0010_0000 || out.last() == Some(&value) || !self.follows_call(value) {
                continue;
            }
            out.push(value);
        }
        out
    }

    /// whether the instruction before address (odd for Thumb) is a call.
    fn follows_call(&mut self, address: u32) -> bool {
        let at = address & !1;
        if !self.memory.is_executable(at) || !self.memory.is_executable(at.wrapping_sub(4)) {
            return false;
        }
        let mut bytes = [0u8; 4];
        self.memory.read_bytes(at.wrapping_sub(4), &mut bytes);
        if address & 1 != 0 {
            let first = u16::from_le_bytes([bytes[0], bytes[1]]);
            let second = u16::from_le_bytes([bytes[2], bytes[3]]);
            // bl/blx pair, or blx register
            (first & 0xF800 == 0xF000 && second & 0xC000 == 0xC000) || second & 0xFF87 == 0x4780
        } else {
            if at & 3 != 0 {
                return false;
            }
            let op = u32::from_le_bytes(bytes);
            // bl, blx immediate, blx register
            op & 0x0F00_0000 == 0x0B00_0000 || op & 0xFE00_0000 == 0xFA00_0000 || op & 0x0FFF_FFF0 == 0x012F_FF30
        }
    }

    /// stops the program because native-only execution reached the
    /// interpreter: logs everything there is to know and exits.
    fn native_only_stop(&mut self, kind: Kind, address: u32, opcode: u32) -> ! {
        let frames = self.frames;
        self.coverage.save(frames, self.recompiled_instructions);
        let thread = self.kernel.current().map_or("?".to_owned(), |t| t.name.clone());
        let stack = self.guest_callstack(24);
        let mode = if address & 1 != 0 { "Thumb" } else { "ARM" };
        let why = match kind {
            Kind::Missing => "is not in the recompiled code".to_owned(),
            Kind::Fallback => format!("holds instruction 0x{opcode:08X}, which the recompiled code leaves to an interpreter"),
            Kind::Declined => "did not get budget for its block".to_owned(),
        };
        let hatch = if HAS_INTERPRETER {
            "Set FATES_ALLOW_INTERPRETER=1 to run it in the diagnostic interpreter, and report the address.\n"
        } else {
            "This build has no interpreter at all (strip-interpreter); report the address.\n"
        };
        let mut message = format!(
            "The game reached {mode} code at 0x{:08X} that {why} (frame {frames}, thread {thread}).\n\
             This build runs only natively recompiled code and has no ARM interpreter enabled.\n{hatch}",
            address & !1
        );
        // codemods: with code mods on, they are turned off for the next start
        if let Some(note) = crate::codemods::failed(address) {
            message.insert_str(0, &note);
        }
        message += &format!("registers: {:?}\n", self.cpu);
        message += "guest callstack (lr, then return addresses found on the stack):\n";
        for entry in &stack {
            message += &format!("  0x{:08X} {}\n", entry & !1, if entry & 1 != 0 { "T" } else { "A" });
        }
        message += "recent entry points:\n";
        for entry in self.recent_instructions().iter().rev().take(16) {
            message += &format!("  0x{:08X} {}\n", entry & !1, if entry & 1 != 0 { "T" } else { "A" });
        }
        for line in message.lines() {
            log::error!("cpu: {line}");
        }
        eprintln!("{message}");
        if let Some(notice) = NOTICE.get() {
            notice(&message);
        }
        log::logger().flush();
        std::process::exit(70);
    }
}
