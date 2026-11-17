//! running code that 3dsrecomp turned into a library ahead of time. the
//! library's functions work on a context holding the registers and reach
//! memory through the same page tables the interpreter uses, calling back
//! here for anything else. whatever the library has no code for stays with
//! the interpreter.

use std::cell::Cell;
use std::ffi::c_void;
use std::path::Path;

use recomp_abi::{Code, Context, Host, EXIT_BUDGET, EXIT_SVC, EXIT_UNWIND};
use zakuro_cpu::{Bus, Cpu, Exit};

use crate::memory::Memory;
use crate::System;

pub use recomp_abi::Linked;

/// hooks: the code's recomp_tails_forget, 0 until the program gives it
/// (set_tails_forget).
static TAILS_FORGET: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// hooks: the program linking the code gives its recomp_tails_forget, so
/// that forget_tails reaches it.
pub fn set_tails_forget(forget: unsafe extern "C" fn(u32)) {
    TAILS_FORGET.store(forget as usize, std::sync::atomic::Ordering::Relaxed);
}

/// hooks: drops the overrides' tails (recomp.h, CALL_THEN) waiting in the
/// thread whose TLS address is tls, or in every thread for 0: the thread
/// ended, or a saved state replaced every thread. They wait for their
/// thread to come back to a return address with a stack pointer, which a
/// new thread with the same TLS slot, or a loaded state, could otherwise do
/// for a frame that is not theirs.
pub fn forget_tails(tls: u32) {
    let forget = TAILS_FORGET.load(std::sync::atomic::Ordering::Relaxed);
    if forget != 0 {
        // SAFETY: the code's recomp_tails_forget, given by set_tails_forget;
        // only the thread running guest code changes the registry
        unsafe { std::mem::transmute::<usize, unsafe extern "C" fn(u32)>(forget)(tls) };
    }
}

/// the library 3dsrecomp build installed for a title, if there is one.
pub fn installed(program_id: u64) -> Option<std::path::PathBuf> {
    recomp_abi::installed(program_id)
}

/// whether the library at path is worth building again, having come from
/// an older 3dsrecomp than this build has, or not running at all.
pub fn outdated(path: &Path) -> bool {
    !recomp_abi::Library::open(path).is_ok_and(|code| code.generation() >= recomp_abi::GENERATION)
}

/// recompiled code, and how much of its work it handed back.
pub struct Library {
    code: recomp_abi::Library,
    /// instructions the code handed to the interpreter one at a time.
    fallbacks: Cell<u64>,
    /// svcs the code ran in place and went on after.
    in_place: Cell<u64>,
    /// the code found for addresses lately, each in a slot its address
    /// picks. a jump through a register looks its target up, which is a
    /// search through every function otherwise.
    found: Box<[Found]>,
    // cpu: lookups the slots answered, and those that searched
    hits: Cell<u64>,
    misses: Cell<u64>,
    // native-rt: how the last run left (LEFT_*), for pcos::rt's counts
    last_left: Cell<u8>,
    /// codemods: the patched functions a code mod brought, run in place of
    /// the stale ones (Host::redirect, lookup).
    patch: Option<std::sync::Arc<crate::codemods::PatchModule>>,
    /// codemods: the native plugins' hooks.
    plugins: Option<Box<crate::codemods::Plugins>>,
    /// codemods: which functions check found changed, by index, apart from
    /// those flagged only for a hook, whose own code still runs.
    code_stale: Vec<bool>,
    /// codemods: whether any function is flagged for a hook.
    hooked: bool,
}

// native-rt: how a run that stopped with Stop::Left left
const LEFT_BUDGET: u8 = 0;
const LEFT_SVC: u8 = 1;
const LEFT_NATIVE: u8 = 2;
const LEFT_OTHER: u8 = 3;

/// a slot of Library::found, an address and the code found for it.
type Found = Cell<(u32, Option<Code>)>;

/// slots of Library::found, a power of two.
const FOUND: usize = 65536;

/// the slot of Library::found an address goes in, the top bits of the
/// address times the golden ratio. ARM addresses step by four and Thumb ones
/// by two with bit 0 set, and taking low bits would leave half the slots to
/// one of them.
fn slot(address: u32) -> usize {
    (address.wrapping_mul(0x9E37_79B1) >> (32 - FOUND.trailing_zeros())) as usize
}

/// why a run of recompiled code ended.
pub enum Stop {
    /// an svc, the cpu is already past it.
    Svc(u32),
    /// the interpreter stopped on an instruction the code handed it.
    Exit(Exit),
    /// the budget ran out, or execution left for code the library does not
    /// have.
    Left,
    /// after an svc it ran in place, the code had no budget for the next
    /// block, which the interpreter goes through instead, as it would have
    /// in the step after the svc.
    Declined,
}

/// what the callbacks reach through the context.
struct Machine<'a> {
    /// the system the code runs in, which the library is out of meanwhile.
    system: *mut System,
    library: &'a Library,
    /// what the interpreter stopped with inside a callback.
    pending: Option<Exit>,
    /// the cycle count and the budget when the code started, or went on
    /// after an svc, and how many instructions it ran before.
    cycles: u64,
    budget: i32,
    ran: u64,
    /// the deadline the step had, and the next event's tick, which the
    /// budget stops at.
    deadline: Option<u64>,
    event: Option<u64>,
    /// an svc the code ran had the step do more, which the code left for.
    left: bool,
    /// the code went on after an svc it ran.
    went_on: bool,
    /// native-rt: a native call had it leave (leave_if_rescheduling).
    native: bool,
}

/// # Safety
///
/// ctx has to be a context run set up, whose user field points at a live
/// Machine.
unsafe fn machine<'a>(ctx: *mut Context) -> &'a mut Machine<'a> {
    unsafe { &mut *((*ctx).user as *mut Machine) }
}

/// os: the system a native call from the code (pcos::bridge) works on,
/// the one the code is running in.
///
/// # Safety
///
/// as for machine.
pub(crate) unsafe fn system_of(ctx: *mut Context) -> *mut System {
    unsafe { machine(ctx).system }
}

/// os: the system clock, in ticks, as far as the code has run: what
/// svcGetSystemTick would read at this point of the run.
///
/// # Safety
///
/// as for machine.
pub(crate) unsafe fn ticks_of(ctx: *mut Context) -> u64 {
    unsafe {
        let machine = machine(ctx);
        // native-rt: in real time the clock is the host's
        if crate::pcos::rt::realtime() {
            return crate::pcos::rt::now(&mut *machine.system);
        }
        let ran = machine.budget.saturating_sub((*ctx).budget).max(0) as u64;
        machine.cycles + ran
    }
}

/// gfx: the system a context the code runs on belongs to, for host code an
/// override calls (the native renderer reads guest memory through it).
///
/// # Safety
///
/// ctx has to be the context of a run in progress, as for machine.
pub unsafe fn gfx_system_of(ctx: *mut c_void) -> *mut System {
    unsafe { machine(ctx as *mut Context).system }
}

/// os: has the code leave its run for the step, as a run does after an
/// svc that makes another thread ready, when a native call (pcos) woke
/// something or the scheduler wants to switch threads. the code returns
/// to where r15 points, after the call.
///
/// # Safety
///
/// as for machine.
pub(crate) unsafe fn leave_if_rescheduling(ctx: *mut Context) {
    unsafe {
        let machine = machine(ctx);
        if (*machine.system).kernel.reschedule_pending {
            machine.left = true;
            machine.native = true; // native-rt
            (*ctx).exit = EXIT_UNWIND;
        }
    }
}

/// the memory the code runs on.
///
/// # Safety
///
/// as for machine, and nothing else may hold the system.
unsafe fn memory<'a>(ctx: *mut Context) -> &'a mut Memory {
    unsafe { &mut (*machine(ctx).system).memory }
}

unsafe extern "C" fn read8(ctx: *mut Context, address: u32) -> u8 {
    unsafe { memory(ctx).read8(address) }
}

unsafe extern "C" fn read16(ctx: *mut Context, address: u32) -> u16 {
    unsafe { memory(ctx).read16(address) }
}

unsafe extern "C" fn read32(ctx: *mut Context, address: u32) -> u32 {
    unsafe { memory(ctx).read32(address) }
}

unsafe extern "C" fn write8(ctx: *mut Context, address: u32, value: u8) {
    unsafe { memory(ctx).write8(address, value) }
}

unsafe extern "C" fn write16(ctx: *mut Context, address: u32, value: u16) {
    unsafe { memory(ctx).write16(address, value) }
}

unsafe extern "C" fn write32(ctx: *mut Context, address: u32, value: u32) {
    unsafe { memory(ctx).write32(address, value) }
}

/// runs one instruction the code left to the interpreter.
#[cfg_attr(any(not(feature = "emulation"), feature = "strip-interpreter"), allow(unreachable_code))]
unsafe extern "C" fn interpret(ctx: *mut Context, address: u32, opcode: u32) {
    unsafe {
        let machine = machine(ctx);
        machine.library.fallbacks.set(machine.library.fallbacks.get() + 1);
        let ctx = &mut *ctx;
        let system = &mut *machine.system;
        load(ctx, &mut system.cpu);
        system.cpu.regs[15] = address;
        // cpu: note it, and stop here in native-only mode
        system.coverage_fallback(address, opcode);
        #[cfg(all(feature = "emulation", not(feature = "strip-interpreter")))] // native-final: the interpreter exists with emulation only
        let exit = system.cpu.step(&mut system.memory);
        #[cfg(any(not(feature = "emulation"), feature = "strip-interpreter"))]
        let exit: Option<zakuro_cpu::Exit> = unreachable!("native-only stops before the interpreter");
        store(&system.cpu, ctx);
        let step = if ctx.thumb != 0 { 2 } else { 4 };
        if exit.is_some() || ctx.r[15] != address.wrapping_add(step) {
            machine.pending = exit;
            ctx.exit = EXIT_UNWIND;
        }
    }
}

unsafe extern "C" fn lookup(ctx: *mut Context, address: u32) -> Option<Code> {
    unsafe { machine(ctx).library.lookup(address) }
}

/// whether svcs run in place, unless ZAKURO_SVC_IN_PLACE is 0, which has
/// the code stop at every one as before.
pub(crate) fn svcs_in_place() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("ZAKURO_SVC_IN_PLACE").map_or(true, |value| value != "0"))
}

/// runs an svc the code made, which the code goes on after when the next
/// step would only run it on, see System::svc_in_place. one that can't run
/// in place stops the code for the step to run it.
unsafe extern "C" fn svc(ctx: *mut Context, number: u32) {
    unsafe {
        // os: the SVCs the native layer answers itself, without the kernel
        if crate::pcos::bridge::native_svc(ctx, number) {
            return;
        }
        let machine = machine(ctx);
        let ctx = &mut *ctx;
        if !svcs_in_place() || !crate::pcos::svc::in_place(number) {
            ctx.svc = number;
            ctx.exit = EXIT_SVC;
            return;
        }
        let system = &mut *machine.system;
        // as far as the step goes, the code stopped at the svc
        let ran = machine.budget.saturating_sub(ctx.budget).max(0) as u64;
        machine.ran += ran;
        load(ctx, &mut system.cpu);
        system.cpu.cycles = machine.cycles + ran;
        // cpu: timed with FATES_CPU_PROFILE=1
        let started = crate::coverage::profiling().then(std::time::Instant::now);
        let next = system.svc_in_place(number, machine.event, machine.deadline, machine.library);
        if let Some(started) = started {
            let took = started.elapsed();
            system.coverage.profile.svc_in_place += took;
            let entry = system.coverage.profile.svcs.entry(number | 0x100).or_default();
            entry.0 += 1;
            entry.1 += took;
        }
        store(&system.cpu, ctx);
        machine.cycles = system.cpu.cycles;
        match next {
            Some((budget, event)) => {
                ctx.budget = budget.min(i32::MAX as u64) as i32;
                machine.event = event;
                machine.went_on = true;
                machine.library.in_place.set(machine.library.in_place.get() + 1);
            }
            None => {
                // nothing is left of the run but the unwinding, its event
                // looked at already
                machine.event = None;
                machine.left = true;
                ctx.exit = EXIT_UNWIND;
            }
        }
        machine.budget = ctx.budget;
    }
}

thread_local! {
    /// codemods: the hooked address whose own code runs next, which a
    /// plugin's hook calls as the original: the redirect it asks lets it
    /// through once.
    static PASS: Cell<u32> = const { Cell::new(u32::MAX) };
}

/// codemods: what a stale function asks on the way in (recomp.h ABI 6): a
/// plugin's hooks run around the code for r15, a patch module's version of
/// it runs in its place, and a function flagged only for a hook entered
/// anywhere but at a hooked address runs its own code. with nothing for it
/// the code leaves, as before ABI 6.
unsafe extern "C" fn redirect(ctx: *mut Context) -> u32 {
    unsafe {
        let machine = machine(ctx);
        let library = machine.library;
        let address = (*ctx).r[15] | (*ctx).thumb as u32;
        if library.hooked {
            if PASS.with(Cell::get) == address {
                PASS.with(|pass| pass.set(u32::MAX));
                return 0;
            }
            if let Some(index) = library.plugins.as_ref().and_then(|plugins| plugins.keys.binary_search(&address).ok()) {
                hooked(ctx, index);
                return 1;
            }
        }
        if let Some(code) = library.patch.as_ref().and_then(|patch| patch.lookup(address)) {
            code(ctx);
            return 1;
        }
        if library.hooked && library.code.lookup_entry(address).and_then(|(_, owner)| owner).is_some_and(|owner| !library.code_stale.get(owner).copied().unwrap_or(false)) {
            return 0;
        }
        log::error!("codemods: changed code at 0x{:08X} has no recompiled version", address & !1);
        (*ctx).exit = EXIT_UNWIND;
        1
    }
}

/// codemods: a hooked function: the befores, the original (the patch
/// module's version or the function's own code), then the afters, once the
/// original has returned, as CALL_THEN does.
unsafe fn hooked(ctx: *mut Context, index: usize) {
    unsafe {
        let machine = machine(ctx);
        let library = machine.library;
        let Some(plugins) = library.plugins.as_ref() else { return };
        let address = plugins.keys[index];
        let set = &plugins.hooks[&address];
        let regs = ctx as *mut crate::codemods::Regs;
        let mut skip = false;
        for hook in &set.before {
            skip |= crate::codemods::call_hook(machine.system, hook, regs) == 1;
        }
        let (ret, sp) = ((*ctx).r[14], (*ctx).r[13]);
        if skip {
            // the function returns at once, r0 as the hook left it
            let to = ret;
            (*ctx).thumb = (to & 1) as u8;
            (*ctx).r[15] = to & if to & 1 != 0 { !1 } else { !3 };
        } else {
            let original = library.patch.as_ref().and_then(|patch| patch.lookup(address));
            let code = match original {
                Some(code) => code,
                None => {
                    let Some((code, _)) = library.code.lookup_entry(address) else {
                        (*ctx).exit = EXIT_UNWIND;
                        return;
                    };
                    PASS.with(|pass| pass.set(address));
                    code
                }
            };
            code(ctx);
            PASS.with(|pass| pass.set(u32::MAX));
        }
        if set.after.is_empty() {
            return;
        }
        if (*ctx).exit == 0 || (((*ctx).r[15] | (*ctx).thumb as u32) == ret && (*ctx).r[13] == sp) {
            run_afters(ctx, index);
        } else if let Some(imports) = crate::codemods::imports() {
            let words = [index as u32, 0, 0, 0, 0, 0];
            (imports.tail_defer)(ctx, ret, sp, after_tail, c"plugin after-hook".as_ptr(), words.as_ptr());
        }
    }
}

unsafe fn run_afters(ctx: *mut Context, index: usize) {
    unsafe {
        let machine = machine(ctx);
        let Some(plugins) = machine.library.plugins.as_ref() else { return };
        let Some(set) = plugins.keys.get(index).and_then(|key| plugins.hooks.get(key)) else { return };
        for hook in &set.after {
            crate::codemods::call_hook(machine.system, hook, ctx as *mut crate::codemods::Regs);
        }
    }
}

/// codemods: the afters of a hook whose original left for the host on the
/// way, run when it returns (the tails registry, recomp.h CALL_THEN).
unsafe extern "C" fn after_tail(ctx: *mut Context, words: *const u32) {
    unsafe { run_afters(ctx, *words as usize) }
}

static HOST: Host = Host { read8, read16, read32, write8, write16, write32, interpret, lookup, svc, redirect };

fn load(ctx: &Context, cpu: &mut Cpu) {
    cpu.regs = ctx.r;
    cpu.cpsr.n = ctx.n != 0;
    cpu.cpsr.z = ctx.z != 0;
    cpu.cpsr.c = ctx.c != 0;
    cpu.cpsr.v = ctx.v != 0;
    cpu.cpsr.q = ctx.q != 0;
    cpu.cpsr.ge = ctx.ge;
    cpu.cpsr.thumb = ctx.thumb != 0;
    cpu.exclusive_addr = (ctx.exclusive != 0).then_some(ctx.exclusive_address);
}

fn store(cpu: &Cpu, ctx: &mut Context) {
    ctx.r = cpu.regs;
    ctx.n = cpu.cpsr.n as u8;
    ctx.z = cpu.cpsr.z as u8;
    ctx.c = cpu.cpsr.c as u8;
    ctx.v = cpu.cpsr.v as u8;
    ctx.q = cpu.cpsr.q as u8;
    ctx.ge = cpu.cpsr.ge;
    ctx.thumb = cpu.cpsr.thumb as u8;
    ctx.exclusive = cpu.exclusive_addr.is_some() as u8;
    ctx.exclusive_address = cpu.exclusive_addr.unwrap_or(0);
    ctx.tls = cpu.cp15.thread_id_ro;
}

impl Library {
    /// a library 3dsrecomp built.
    pub fn open(path: &Path) -> Result<Library, String> {
        recomp_abi::Library::open(path).map(Library::new)
    }

    /// the code linked into the program.
    pub fn linked(linked: &Linked) -> Result<Library, String> {
        recomp_abi::Library::linked(linked).map(Library::new)
    }

    fn new(mut code: recomp_abi::Library) -> Library {
        // ZAKURO_NATIVES=0 has the title's own code run for the C library
        // functions the library does on the host
        if std::env::var("ZAKURO_NATIVES").is_ok_and(|value| value == "0") && code.set_natives(false) {
            log::info!("the library's natives are off, the title's own code runs for them");
        }
        Library {
            code,
            fallbacks: Cell::new(0),
            in_place: Cell::new(0),
            found: Library::nothing_found(),
            hits: Cell::new(0),
            misses: Cell::new(0),
            last_left: Cell::new(LEFT_OTHER), // native-rt
            patch: None,
            plugins: None,
            code_stale: Vec::new(),
            hooked: false,
        }
    }

    /// native-rt: whether the last run that stopped with Stop::Left ran out
    /// of budget.
    pub(crate) fn left_at_budget(&self) -> bool {
        self.last_left.get() == LEFT_BUDGET
    }

    /// native-rt: whether a native call had the last run leave.
    pub(crate) fn left_for_native(&self) -> bool {
        self.last_left.get() == LEFT_NATIVE
    }

    /// every slot of found empty, an address no code starts at in each.
    fn nothing_found() -> Box<[Found]> {
        (0..FOUND).map(|_| Cell::new((u32::MAX, None))).collect()
    }

    /// whether the code came from an older 3dsrecomp than the one this
    /// build has, so that recompiling the game again makes it better.
    pub fn outdated(&self) -> bool {
        self.code.generation() < recomp_abi::GENERATION
    }

    /// whether the code can tell the functions whose code changed in
    /// memory, which it can from generation 2 on.
    pub fn checks_itself(&self) -> bool {
        self.code.origins(0).is_some()
    }

    /// how many instructions the code has handed to the interpreter.
    pub fn fallbacks(&self) -> u64 {
        self.fallbacks.get()
    }

    /// cpu: lookups the slots answered, and those that searched the tables.
    pub fn lookups(&self) -> (u64, u64) {
        (self.hits.get(), self.misses.get())
    }

    /// how many svcs the code ran in place, going on after them.
    pub fn in_place(&self) -> u64 {
        self.in_place.get()
    }

    /// how many functions and modules there is code for.
    pub fn describe(&self) -> String {
        self.code.describe()
    }

    /// the code that can run from address, bit 0 set for Thumb, in the
    /// executable or in a module that is loaded.
    fn lookup(&self, address: u32) -> Option<Code> {
        let slot = &self.found[slot(address)];
        match slot.get() {
            (seen, code) if seen == address => {
                self.hits.set(self.hits.get() + 1);
                code
            }
            _ => {
                self.misses.set(self.misses.get() + 1);
                let mut code = self.code.lookup(address);
                // codemods: a function a code mod changed runs from its
                // patch module, one flagged only for a hook its own code
                if code.is_none() && (self.patch.is_some() || self.hooked) {
                    code = self.patch.as_ref().and_then(|patch| patch.lookup(address)).or_else(|| {
                        let (code, owner) = self.code.lookup_entry(address)?;
                        (self.hooked && owner.is_some_and(|owner| !self.code_stale.get(owner).copied().unwrap_or(false))).then_some(code)
                    });
                }
                slot.set((address, code));
                code
            }
        }
    }

    pub fn has_code(&self, address: u32) -> bool {
        self.lookup(address).is_some()
    }

    /// marks the executable's functions whose code in memory is not what
    /// they were recompiled from, which a mod or another version of the
    /// game does, so that the interpreter runs them. how many there are.
    pub fn check(&mut self, memory: &mut Memory) -> usize {
        let stale = self.code.check(0, 0, |address, bytes| memory.read_bytes(address, bytes));
        self.found = Library::nothing_found();
        // codemods: which changed, before any hook flags more
        self.code_stale = match self.code.origins(0) {
            Some(origins) => {
                let (flags, _) = origins.flags();
                let count = origins.functions().len();
                // SAFETY: a flag per function, the code's own
                (0..count).map(|i| unsafe { *flags.add(i) } != 0).collect()
            }
            None => Vec::new(),
        };
        // the plugins' hooks stay on (a cheat that changes the code checks
        // again, as a loaded state does)
        self.hooked = false;
        self.flag_hooks();
        stale
    }

    /// codemods: check against a code image instead of memory, before it is
    /// mapped: how many functions it changes.
    pub fn check_image(&mut self, exheader: &zakuro_fs::ncch::ExHeader, code: &[u8]) -> usize {
        let stale = self.code.check(0, 0, |address, bytes| crate::codemods::read_image(exheader, code, address, bytes));
        self.found = Library::nothing_found();
        self.code_stale = match self.code.origins(0) {
            Some(origins) => {
                let (flags, _) = origins.flags();
                // SAFETY: a flag per function, the code's own
                (0..origins.functions().len()).map(|i| unsafe { *flags.add(i) } != 0).collect()
            }
            None => Vec::new(),
        };
        stale
    }

    /// codemods: the functions check found changed, by their starts, bit 0
    /// set for Thumb.
    pub fn stale_starts(&self) -> Vec<u32> {
        let Some(origins) = self.code.origins(0) else { return Vec::new() };
        let functions = origins.functions();
        let mut starts = Vec::new();
        for (index, function) in functions.iter().enumerate() {
            if !self.code_stale.get(index).copied().unwrap_or(false) {
                continue;
            }
            let owned = |key: u32| self.code.lookup_entry(key).is_some_and(|(_, owner)| owner == Some(index));
            starts.push(if owned(function.start) { function.start } else if owned(function.start | 1) { function.start | 1 } else { function.start });
        }
        starts
    }

    /// codemods: runs the patch module's code in place of the functions
    /// check found changed. every one of them has to start in it, or it is
    /// turned down.
    pub fn attach_patch(&mut self, module: std::sync::Arc<crate::codemods::PatchModule>) -> Result<usize, String> {
        let starts = self.stale_starts();
        let missing: Vec<u32> = starts.iter().copied().filter(|&start| module.lookup(start).is_none()).collect();
        if let Some(first) = missing.first() {
            return Err(format!("{} changed functions have no recompiled version in {} (the first at 0x{first:08X})", missing.len(), module.path.display()));
        }
        log::info!("codemods: {} changed functions run from {} ({} entries)", starts.len(), module.path.display(), module.len());
        self.patch = Some(module);
        self.found = Library::nothing_found();
        Ok(starts.len())
    }

    /// codemods: the plugins' hooks: each hooked function's flag is set, and
    /// its start too when it starts at the hooked address, so that every way
    /// in asks the redirect. an address with no recompiled entry, or with
    /// code written by hand (Haiku's own override, which wins), can't be
    /// hooked and is left out with a note.
    pub fn attach_plugins(&mut self, mut plugins: crate::codemods::Plugins) {
        let keys: Vec<u32> = plugins.hooks.keys().copied().collect();
        for key in keys {
            match self.code.lookup_entry(key) {
                None => plugins.refuse(key, "the game has no function there"),
                Some((_, None)) => plugins.refuse(key, "Haiku has its own version of that function"),
                Some(_) if self.code.origins(0).is_none() => plugins.refuse(key, "this build can't hook functions"),
                Some(_) => {}
            }
        }
        for note in &plugins.notes {
            crate::codemods::note(note.clone());
        }
        log::info!("codemods: plugins {:?}, {} hooked addresses", plugins.names(), plugins.keys.len());
        self.plugins = Some(Box::new(plugins));
        self.flag_hooks();
        self.found = Library::nothing_found();
    }

    /// codemods: sets the flag of each hooked function, and its start when
    /// it starts at the hooked address, so that every way in asks the
    /// redirect.
    fn flag_hooks(&mut self) {
        let Some(plugins) = self.plugins.as_ref() else { return };
        let Some(origins) = self.code.origins(0) else { return };
        let (flags, starts) = origins.flags();
        for &key in &plugins.keys {
            let Some((_, Some(owner))) = self.code.lookup_entry(key) else { continue };
            let start = origins.functions()[owner].start;
            // SAFETY: a flag and a start per function, the code's own
            unsafe {
                *flags.add(owner) = 1;
                if start == key & !1 {
                    *starts.add(owner) = recomp_abi::NO_START;
                }
            }
            self.hooked = true;
        }
        self.found = Library::nothing_found();
    }

    /// codemods: the plugins loaded, for their frame callbacks.
    pub(crate) fn plugins(&self) -> Option<&crate::codemods::Plugins> {
        self.plugins.as_deref()
    }

    /// tells the code of the module called name where the title loaded it,
    /// zero when it unloads it, and checks the module's code once it is in
    /// memory.
    pub fn place(&mut self, name: &str, base: u32, memory: &mut Memory) {
        let Some(index) = self.code.module_index(name) else { return };
        self.code.place(index, base);
        // what was found before may have moved or gone
        self.found = Library::nothing_found();
        if base == 0 {
            return;
        }
        log::info!("recompiled code for {name} runs at 0x{base:08X}");
        let stale = self.code.check(index + 1, base, |address, bytes| memory.read_bytes(address, bytes));
        if stale > 0 {
            log::warn!("{stale} functions of {name} changed since it was recompiled, they run in the interpreter");
        }
    }

    /// runs recompiled code from the cpu's pc for about budget instructions,
    /// returning how many ran, why it stopped and the tick of the event its
    /// budget last stopped at. the code runs the svcs it can in place, going
    /// on after them, and deadline and event are what the step had, for
    /// those. the library has to be out of the system, which the code has
    /// to itself.
    pub fn run(&self, system: &mut System, budget: u64, deadline: Option<u64>, event: Option<u64>) -> (u64, Stop, Option<u64>) {
        let system: *mut System = system;
        let budget = budget.min(i32::MAX as u64) as i32;
        // SAFETY: everything the code reaches of the system comes from this
        // one pointer, which nothing else uses until the run is over
        let (cycles, read_pages, write_pages, vfp, fpscr) = unsafe {
            let (read_pages, write_pages) = (*system).memory.page_tables();
            // the code works on the VFP registers where they are, so the
            // interpreter sees its changes without copying them around
            let vfp = &raw mut (*system).cpu.vfp.regs as *mut u32;
            let fpscr = &raw mut (*system).cpu.vfp.fpscr;
            ((*system).cpu.cycles, read_pages, write_pages, vfp, fpscr)
        };
        let mut ctx = Context {
            r: [0; 16],
            n: 0,
            z: 0,
            c: 0,
            v: 0,
            q: 0,
            thumb: 0,
            ge: 0,
            exclusive: 0,
            budget,
            exit: 0,
            svc: 0,
            depth: 0,
            exclusive_address: 0,
            tls: 0,
            read_pages,
            write_pages,
            vfp,
            fpscr,
            host: &HOST,
            user: std::ptr::null_mut(),
        };
        // SAFETY: as above
        store(unsafe { &(*system).cpu }, &mut ctx);
        let mut machine =
            Machine { system, library: self, pending: None, cycles, budget, ran: 0, deadline, event, left: false, went_on: false, native: false };
        ctx.user = &mut machine as *mut Machine as *mut c_void;

        // native-rt: how a Stop::Left left, counted by pcos::rt
        self.last_left.set(LEFT_OTHER);
        let stop = loop {
            if ctx.budget <= 0 {
                self.last_left.set(LEFT_BUDGET);
                break Stop::Left;
            }
            let Some(code) = self.lookup(ctx.r[15] | ctx.thumb as u32) else {
                break Stop::Left;
            };
            ctx.exit = 0;
            ctx.depth = 0;
            // SAFETY: the code came from the library and ctx is set up the
            // way it expects, with user pointing at machine
            unsafe { code(&mut ctx) };
            if let Some(exit) = machine.pending.take() {
                break Stop::Exit(exit);
            }
            // the step does the rest, the svc ran
            if machine.left {
                self.last_left.set(if machine.native { LEFT_NATIVE } else { LEFT_SVC });
                break Stop::Left;
            }
            match ctx.exit {
                EXIT_SVC => break Stop::Svc(ctx.svc),
                EXIT_BUDGET if machine.went_on && ctx.budget == machine.budget => break Stop::Declined,
                EXIT_BUDGET => {
                    self.last_left.set(LEFT_BUDGET);
                    break Stop::Left;
                }
                _ => {}
            }
        };

        // SAFETY: the code is done with the system
        let cpu = unsafe { &mut (*system).cpu };
        load(&ctx, cpu);
        let ran = machine.budget.saturating_sub(ctx.budget.max(0)).max(0) as u64;
        // the interpreter counted the instructions it ran for the code, which
        // the budget already has
        cpu.cycles = machine.cycles + ran;
        (machine.ran + ran, stop, machine.event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// a library that doesn't open has nothing to run, recompiling is what
    /// mends it.
    #[test]
    fn a_library_that_cant_be_opened_is_outdated() {
        assert!(outdated(Path::new("/nowhere/at/all.so")));
    }

    #[test]
    fn arm_and_thumb_code_use_every_slot() {
        let arm: std::collections::BTreeSet<usize> = (0..FOUND as u32).map(|i| slot(0x0010_0000 + 4 * i)).collect();
        let thumb: std::collections::BTreeSet<usize> = (0..FOUND as u32).map(|i| slot(0x0010_0001 + 2 * i)).collect();
        assert!(arm.len() > FOUND * 3 / 4, "{} of {FOUND}", arm.len());
        assert!(thumb.len() > FOUND * 3 / 4, "{} of {FOUND}", thumb.len());
    }
}
