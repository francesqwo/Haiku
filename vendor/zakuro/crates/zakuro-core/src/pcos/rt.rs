//! native-rt: how the game's threads share the time, in two modes.
//!
//! **Deterministic** (os2's model, unchanged). The guest clock moves only as
//! recompiled instructions run: one tick per instruction, plus the jumps to
//! the next event while every thread is blocked. A run of recompiled code
//! gets a *budget* that ends at the next thing the scheduler must look at
//! (the refresh, the next audio frame, a timer or a timeout, a forced
//! scheduling pass every `PREEMPT_INTERVAL` ticks while another thread could
//! run, the frame's end). Replays and save-state checks depend on exactly
//! where those cuts fall, so this mode is kept byte for byte.
//!
//! **Real time** (ultramodern's model). The guest clock is the host's clock,
//! anchored at each refresh: `tick = refresh tick + host time since it`,
//! never past the next refresh, which only the frontend raises
//! (`System::run_frame`), on its own wall-clock schedule (59.83 Hz). The
//! game's threads run on their host threads, one at a time as before, while
//! the frontend *lends* them the time it would otherwise sleep
//! (`System::lend_until`). There is no time slicing:
//!
//! - a thread runs until it blocks in a real wait (an event, a mutex, the
//!   arbiter, a sleep), or until a higher-priority thread becomes ready;
//! - a run of recompiled code gets a fixed budget (`quantum`, about half a
//!   millisecond of code), which serves only as the interrupt check: when it
//!   runs out the host looks at its clock, wakes what is due (sleeps,
//!   timers, the audio frame), and the scheduler decides, by the same
//!   priority policy, whether the thread goes on;
//! - when every thread is blocked, the thread holding the baton sleeps on the
//!   host until the next thing due (no clock jump);
//! - native calls keep no imitation of the console's IPC latency
//!   (`FATES_NATIVE_TIMING=console` keeps it).
//!
//! Real time is not reproducible, so anything that must be is deterministic
//! automatically: headless runs, replays (`ZAKURO_REPLAY`), recordings
//! (`ZAKURO_RECORD`), states loaded for checks (`FATES_LOAD`), input scripts
//! and dumps. `FATES_SCHED=deterministic|realtime` forces either.
//!
//! This module also counts, in both modes, why each run of recompiled code
//! ended and why each hand-off between host threads happened
//! (`FATES_SCHED_STATS=1` prints them at the end of a headless run, and
//! once a minute in a window).

use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering::Relaxed};
use std::time::{Duration, Instant};

use crate::kernel::thread::{ThreadStatus, CPU_CLOCK_HZ};
use crate::{FrameOutcome, System, CYCLES_PER_FRAME};

// -- the mode ----------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Deterministic,
    RealTime,
}

static MODE: AtomicU8 = AtomicU8::new(0);

/// whether the session runs in real time.
#[inline]
pub fn realtime() -> bool {
    MODE.load(Relaxed) == 1
}

pub fn mode() -> Mode {
    if realtime() {
        Mode::RealTime
    } else {
        Mode::Deterministic
    }
}

/// sets the mode; before the game's first frame.
pub fn set(mode: Mode) {
    MODE.store((mode == Mode::RealTime) as u8, Relaxed);
}

/// the default for interactive play (a window, no replay or recording).
pub const INTERACTIVE_DEFAULT: Mode = Mode::RealTime;

/// picks the session's mode, see the module docs, and logs why.
pub fn choose(headless: bool) -> Mode {
    let asked = std::env::var("FATES_SCHED").ok().map(|value| value.trim().to_ascii_lowercase());
    let forced = match asked.as_deref() {
        Some("deterministic" | "det") => Some(Mode::Deterministic),
        Some("realtime" | "real-time" | "rt") => Some(Mode::RealTime),
        Some("" | "auto") | None => None,
        Some(other) => {
            log::warn!("native-rt: FATES_SCHED={other} is neither deterministic nor realtime, choosing by the session");
            None
        }
    };
    let (mode, why) = match forced {
        Some(mode) => (mode, "FATES_SCHED".to_owned()),
        None => {
            let reproducible = [
                ("ZAKURO_REPLAY", "a replay"),
                ("ZAKURO_RECORD", "a recording"),
                ("FATES_LOAD", "a state loaded for a check"),
                ("FATES_INPUT_SCRIPT", "an input script"),
                ("ZAKURO_DUMP_DIR", "frame dumps"),
            ];
            let set = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
            match reproducible.iter().find(|(name, _)| set(name)) {
                _ if headless => (Mode::Deterministic, "a headless run".to_owned()),
                Some((name, what)) => (Mode::Deterministic, format!("{what} ({name})")),
                None => (INTERACTIVE_DEFAULT, "interactive play".to_owned()),
            }
        }
    };
    set(mode);
    log::info!("native-rt: the game's threads are scheduled {} ({why})", match mode {
        Mode::Deterministic => "deterministically, on the guest clock",
        Mode::RealTime => "in real time, on the host clock",
    });
    mode
}

/// how many instructions a run of recompiled code gets in real time before
/// the host looks at its clock (FATES_RT_QUANTUM), about half a
/// millisecond.
pub fn quantum() -> u64 {
    static QUANTUM: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *QUANTUM.get_or_init(|| std::env::var("FATES_RT_QUANTUM").ok().and_then(|v| v.parse().ok()).filter(|&v| v >= 1024).unwrap_or(1 << 17))
}

/// whether native calls wait as long as the IPC request they replace did:
/// always in deterministic mode (unless FATES_NATIVE_TIMING=fast), in real
/// time only with FATES_NATIVE_TIMING=console.
pub fn console_timing_in_realtime() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("FATES_NATIVE_TIMING").is_ok_and(|value| value == "console"))
}

// -- the real-time clock -------------------------------------------------------

/// the real-time clock and lend of a system (System::rt).
#[derive(Default)]
pub struct Clock {
    /// the host instant and the guest tick it was anchored at: the last
    /// refresh, or wherever the clock was taken up again.
    anchor: Option<(Instant, u64)>,
    /// the refresh the anchor leads up to (a loaded state changes it).
    boundary: u64,
    /// the last tick handed out, the clock never goes back.
    last: u64,
    /// the end of the time the frontend lent the threads.
    until: Option<Instant>,
    /// whether the threads had any time since the last refresh, and how
    /// many refreshes in a row they had none before.
    lent: bool,
    unlent: u32,
    /// rt-merge: since the last refresh, the time lent, and whether every
    /// thread was blocked with nothing due for a while (the frame's work
    /// done, the threads waiting for the next refresh); and whether the
    /// lend running ends there (System::lend_until_settled).
    lent_since: Duration,
    raised: Option<Instant>,
    idled: bool,
    stop_when_idle: bool,
    /// rtpace: the lend running ends once this is false (lend_while)
    while_set: Option<&'static std::sync::atomic::AtomicBool>,
}

/// refreshes in a row raised with no time for the threads before them (the
/// frontend catching up a little); later ones are absorbed, see refresh.
const MOST_UNLENT: u32 = 2;

/// rt-merge: every thread blocked with nothing due for this long counts as
/// the frame's work done (see wait_for_event).
const SETTLED_GAP: Duration = Duration::from_millis(1);

/// rt-merge: a refresh's threads that had this much time since the refresh
/// before had their share, done or not (a thread that polls never blocks).
const SETTLED_LENT: Duration = Duration::from_millis(10);

/// qol: the game speed, in thousandths: in real time the guest clock runs
/// this many times as fast as the host's (the frontend raises refreshes as
/// many times as often). 1000, the console's speed, unless Game Speed is set.
static SPEED_MILLI: AtomicU32 = AtomicU32::new(1000);

/// qol: sets the game speed (1.0 the console's).
pub fn set_speed(speed: f64) {
    SPEED_MILLI.store((speed.clamp(1.0, 64.0) * 1000.0) as u32, Relaxed);
}

/// qol: host time scaled to guest time at the game speed.
fn scaled(duration: Duration) -> Duration {
    match SPEED_MILLI.load(Relaxed) {
        1000 => duration,
        milli => Duration::from_nanos((duration.as_nanos() * milli as u128 / 1000) as u64),
    }
}

/// qol: guest time back to host time at the game speed.
fn unscaled(duration: Duration) -> Duration {
    match SPEED_MILLI.load(Relaxed) {
        1000 => duration,
        milli => Duration::from_nanos((duration.as_nanos() * 1000 / milli.max(1) as u128) as u64),
    }
}

fn ticks(duration: Duration) -> u64 {
    (duration.as_nanos() * CPU_CLOCK_HZ as u128 / 1_000_000_000) as u64
}

fn duration(ticks: u64) -> Duration {
    Duration::from_nanos((ticks as u128 * 1_000_000_000 / CPU_CLOCK_HZ as u128) as u64)
}

/// the guest clock in real time: the host's time since the anchor, never
/// back, never to the next refresh (the frontend raises that).
pub(crate) fn now(system: &mut System) -> u64 {
    let boundary = system.next_frame_boundary;
    let clock = &mut system.rt;
    if clock.anchor.is_none() || clock.boundary != boundary {
        // the first run, or a state was loaded: the clock goes on from where
        // the system is
        clock.anchor = Some((Instant::now(), system.cpu.cycles));
        clock.boundary = boundary;
        clock.last = system.cpu.cycles;
    }
    let (at, tick) = clock.anchor.expect("anchored");
    let tick = (tick + ticks(scaled(at.elapsed()))).max(clock.last).min(boundary.saturating_sub(1).max(clock.last)); // qol: at the game speed
    clock.last = tick;
    tick
}

/// real time: the guest clock to the host's.
#[inline]
pub(crate) fn sync(system: &mut System) {
    if realtime() {
        system.cpu.cycles = now(system);
    }
}

/// the host instant the guest clock reaches tick at.
fn instant_of(system: &System, tick: u64) -> Option<Instant> {
    let (at, anchor) = system.rt.anchor?;
    Some(at + unscaled(duration(tick.saturating_sub(anchor)))) // qol: at the game speed
}

/// whether the threads' turn is over: in real time when the lend ends, in
/// deterministic mode when the guest clock reaches deadline.
#[inline]
pub(crate) fn over(system: &System, deadline: u64) -> bool {
    match system.rt.until {
        Some(until) if realtime() => Instant::now() >= until || system.rt.while_set.is_some_and(|set| !set.load(Relaxed)), // rtpace
        _ => system.cpu.cycles >= deadline,
    }
}

/// whether a lend is over, in real time (for code going on after an svc it
/// ran in place, which would otherwise not stop until its next budget).
#[inline]
pub(crate) fn lend_over(system: &System) -> bool {
    realtime() && (system.rt.until.is_some_and(|until| Instant::now() >= until) || system.rt.while_set.is_some_and(|set| !set.load(Relaxed))) // rtpace
}

/// sleeps until an instant on a high-resolution timer, which wakes a few
/// hundred microseconds late at most; nothing waits on it more precisely
/// (an audio frame is 4.9 ms, the frontend queues its pictures ahead), and
/// spinning the rest would cost a core's time.
fn sleep_until(when: Instant) {
    loop {
        let left = when.saturating_duration_since(Instant::now());
        if left < Duration::from_micros(20) {
            return;
        }
        precise_sleep(left);
    }
}

/// a sleep on a high-resolution waitable timer (Windows 10 1803 on), which
/// wakes within a few hundred microseconds where Sleep's 1 ms ticks could
/// be late by up to a tick; the OS's sleep elsewhere.
fn precise_sleep(duration: Duration) {
    #[cfg(windows)]
    {
        unsafe extern "system" {
            fn CreateWaitableTimerExW(attributes: *const u8, name: *const u16, flags: u32, access: u32) -> isize;
            fn SetWaitableTimer(timer: isize, due: *const i64, period: i32, routine: *const u8, argument: *const u8, resume: i32) -> i32;
            fn WaitForSingleObject(handle: isize, milliseconds: u32) -> u32;
        }
        const CREATE_WAITABLE_TIMER_HIGH_RESOLUTION: u32 = 0x2;
        const TIMER_ALL_ACCESS: u32 = 0x1F0003;
        thread_local! {
            // SAFETY: a plain call; 0 when the system has no such timers
            static TIMER: isize = unsafe { CreateWaitableTimerExW(std::ptr::null(), std::ptr::null(), CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS) };
        }
        let timer = TIMER.with(|timer| *timer);
        if timer != 0 {
            // relative, in 100 ns units
            let due = -((duration.as_nanos() / 100).max(1) as i64);
            // SAFETY: the thread's own timer handle
            if unsafe { SetWaitableTimer(timer, &due, 0, std::ptr::null(), std::ptr::null(), 0) } != 0 {
                unsafe { WaitForSingleObject(timer, u32::MAX) };
                return;
            }
        }
    }
    std::thread::sleep(duration);
}

/// real time, every thread blocked: the holder of the baton sleeps until
/// the next thing due, a thread's wake-up or a timer it waits on, an audio
/// frame the emulated DSP's interrupt is due at (states saved with it), or
/// the end of the lend.
pub(crate) fn wait_for_event(system: &mut System) {
    let Some(until) = system.rt.until else { return };
    let mut due = super::sched::next_event(&system.kernel);
    if system.services.dsp.running {
        due = Some(due.map_or(system.next_audio_frame, |due| due.min(system.next_audio_frame)));
    }
    // rt-merge: nothing at or past the next refresh can happen before the
    // frontend raises it (the clock stops short of it): waking for it once
    // its time passed, with the refresh raised late, spun here
    let due = due.filter(|&tick| tick < system.next_frame_boundary);
    let wake = due.and_then(|tick| instant_of(system, tick)).map_or(until, |at| at.min(until));
    let started = Instant::now();
    // rt-merge: blocked with nothing due for a while: the frame's work is
    // done (a short wait, a sleep of a few hundred microseconds, is the
    // middle of it); a lend that waits for that ends here
    if wake.saturating_duration_since(started) >= SETTLED_GAP || due.is_none() {
        system.rt.idled = true;
        if system.rt.stop_when_idle {
            system.rt.until = Some(started);
            return;
        }
    }
    // rtpace: a lend that lasts while something is set looks at it every
    // quarter millisecond while everything sleeps
    let wake = match system.rt.while_set {
        Some(set) if !set.load(Relaxed) => {
            system.rt.until = Some(started);
            return;
        }
        Some(_) => wake.min(started + Duration::from_micros(250)),
        None => wake,
    };
    if stats_wanted() && wake <= started {
        static SHOWN: AtomicU64 = AtomicU64::new(0);
        if SHOWN.fetch_add(1, Relaxed) < 20 {
            eprintln!(
                "sched: idle wait due already: tick {} due {:?} boundary {} audio {} running {} anchor {:?} until in {:?}, frame {}",
                system.cpu.cycles,
                due,
                system.next_frame_boundary,
                system.next_audio_frame,
                system.services.dsp.running,
                system.rt.anchor.map(|(at, tick)| (at.elapsed(), tick)),
                until.saturating_duration_since(started),
                system.frames
            );
        }
    }
    sleep_until(wake);
    STATS.idle_waits.fetch_add(1, Relaxed);
    STATS.idle_ns.fetch_add(started.elapsed().as_nanos() as u64, Relaxed);
    let over = Instant::now().saturating_duration_since(wake).as_nanos() as u64;
    STATS.sleep_over_ns.fetch_add(over, Relaxed);
    STATS.sleep_over_max.fetch_max(over, Relaxed);
    sync(system);
}

/// real time: the refresh, which the frontend raises on its schedule.
pub(crate) fn refresh(system: &mut System) -> FrameOutcome {
    if system.exited {
        return FrameOutcome::Exited;
    }
    if system.broke {
        return FrameOutcome::Faulted;
    }
    // refreshes the threads had no time before: the frontend catching up
    // after a stall of the host (a long native call, a shader built, the
    // window dragged). On the console nothing stalls the core, and the
    // game's watchdogs (Pica::TimeOutCallback on its GPU waits) take a jump
    // of its clock for a hung GPU: so after MOST_UNLENT of them the rest are
    // absorbed, the game's time pausing over the stall as it does in
    // deterministic mode. The ones raised do not run out the native
    // renderer's lease, which counts refreshes the game could draw in.
    // A refresh that finds a thread still running its work (a frame that
    // took longer than its refreshes on the wall clock: in deterministic
    // mode the guest clock waits for it instead) does not run out the
    // lease either: the game is drawing, late, not stopped.
    let busy = system.kernel.threads.iter().any(|thread| matches!(thread.status, ThreadStatus::Ready | ThreadStatus::Running));
    STATS.busy.fetch_add(busy as u64, Relaxed); // rt-merge
    if std::mem::replace(&mut system.rt.lent, false) {
        system.rt.unlent = 0;
        if busy {
            crate::native_gfx::hold_lease();
        }
    } else {
        system.rt.unlent += 1;
        if system.rt.unlent > MOST_UNLENT {
            STATS.absorbed.fetch_add(1, Relaxed);
            return FrameOutcome::Completed;
        }
        crate::native_gfx::hold_lease();
    }
    // the guest clock reaches the refresh
    sync(system);
    system.cpu.cycles = system.cpu.cycles.max(system.next_frame_boundary);
    system.next_frame_boundary = system.cpu.cycles + CYCLES_PER_FRAME;
    system.end_frame();
    let clock = &mut system.rt;
    clock.anchor = Some((Instant::now(), system.cpu.cycles));
    clock.boundary = system.next_frame_boundary;
    clock.last = system.cpu.cycles;
    clock.lent_since = Duration::ZERO; // rt-merge
    clock.raised = Some(Instant::now());
    clock.idled = false;
    STATS.refreshes.fetch_add(1, Relaxed);
    FrameOutcome::Completed
}

/// savehang: the real-time clock's state, for pcos::hang's dump.
pub(crate) fn describe(system: &System) -> String {
    let clock = &system.rt;
    format!(
        "clock: anchored {:?} ago at tick {:?}, last tick {}, lend {:?}, unlent {}, lent since refresh {:.2?}, idled {}, speed {}",
        clock.anchor.map(|(at, _)| at.elapsed()),
        clock.anchor.map(|(_, tick)| tick),
        clock.last,
        clock.until.map(|until| until.saturating_duration_since(Instant::now())),
        clock.unlent,
        clock.lent_since,
        clock.idled,
        SPEED_MILLI.load(Relaxed) as f64 / 1000.0
    )
}

impl System {
    /// native-rt: in real time, lets the game's threads run until the host
    /// clock reaches until (the frontend's next refresh or picture), then
    /// takes them back. Nothing in deterministic mode, where run_frame runs
    /// the threads.
    pub fn lend_until(&mut self, until: Instant) {
        if !realtime() || self.exited || self.broke || Instant::now() >= until {
            return;
        }
        let started = Instant::now();
        self.rt.until = Some(until);
        self.rt.lent = true;
        let _ = super::threads::run_until(self, u64::MAX);
        self.rt.until = None;
        super::rtpace::lend(started, until, self.rt.stop_when_idle); // rtpace: dev
        self.rt.lent_since += started.elapsed(); // rt-merge
        STATS.lends.fetch_add(1, Relaxed);
        STATS.lent_ns.fetch_add(started.elapsed().as_nanos() as u64, Relaxed);
        let late = Instant::now().saturating_duration_since(until).as_nanos() as u64;
        STATS.late_ns.fetch_add(late, Relaxed);
        STATS.late_max.fetch_max(late, Relaxed);
        STATS.late_over_ms.fetch_add((late > 1_000_000) as u64, Relaxed);
    }
}

impl System {
    /// rt-merge: in real time, whether the game's threads finished their
    /// work since the last refresh (every thread blocked with nothing due
    /// for a while) or had SETTLED_LENT, or three quarters of the time since
    /// the refresh, to do it; always in deterministic mode, where the
    /// refresh runs them.
    pub fn rt_settled(&self) -> bool {
        // or the threads had most of the time since the refresh (the
        // frontend did not take it), done or not
        let most = self.rt.raised.is_some_and(|raised| self.rt.lent_since * 4 >= raised.elapsed() * 3);
        !realtime() || self.rt.idled || self.rt.lent_since >= SETTLED_LENT || most
    }

    /// rtpace: lend_until, ending early once `set` is false (the frontend's
    /// picture in between, drawn on a thread of its own, is done).
    pub fn lend_while(&mut self, until: Instant, set: &'static std::sync::atomic::AtomicBool) {
        if !set.load(Relaxed) {
            return;
        }
        self.rt.while_set = Some(set);
        self.lend_until(until);
        self.rt.while_set = None;
    }

    /// rt-merge: lend_until, ending early once every thread is blocked with
    /// nothing due for a while: for the frontend about to raise a refresh
    /// while the game's frame is still in progress (the frontend's own work
    /// took the time since the last refresh), so the game finishes it
    /// rather than missing the refresh.
    pub fn lend_until_settled(&mut self, until: Instant) {
        if self.rt_settled() {
            return;
        }
        // no further than SETTLED_LENT in all since the refresh
        let until = until.min(Instant::now() + SETTLED_LENT.saturating_sub(self.rt.lent_since));
        self.rt.stop_when_idle = true;
        self.lend_until(until);
        self.rt.stop_when_idle = false;
        STATS.settles.fetch_add(1, Relaxed);
    }
}

// -- what the scheduling did, in both modes ---------------------------------------

/// what bounded a run's budget.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Limit {
    Frame,
    Audio,
    Event,
    Preempt,
    Deadline,
    Quantum,
}

const LIMITS: [&str; 6] = ["refresh", "audio frame", "timer/timeout", "preemption", "frame end", "quantum"];

/// why a run of recompiled code ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum End {
    /// its budget ran out, bounded by a Limit.
    Budget,
    /// an svc it ran in place had it leave (see Left).
    SvcLeft,
    /// a native call (pcos::bridge) woke a thread or put this one to sleep.
    NativeLeft,
    /// an svc it could not run in place.
    Svc,
    /// no budget for a whole block after an svc.
    Declined,
    /// anything else (an interpreter exit).
    Other,
}

/// why an svc run in place had the code leave.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Left {
    Over,
    Frame,
    Audio,
    Switch,
    Other,
}

const LEFTS: [&str; 5] = ["run over", "refresh", "audio frame", "thread switch", "other"];

/// why the baton changed hands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Handoff {
    /// from the frontend, at a frame's (or lend's) start.
    FromDriver,
    /// back to the frontend: the frame's end, the lend's end, an exit.
    ToDriver,
    /// the thread slept (svcSleepThread, a native call's console timing).
    Sleep,
    /// the thread waits on objects (events, mutexes, semaphores, timers).
    Wait,
    /// the thread waits on the address arbiter.
    Arbiter,
    /// a more urgent thread became ready through an svc or native call.
    PreemptSignal,
    /// a more urgent thread became ready through time (its sleep, a
    /// timeout or a timer), or the starvation rule, at a budget's end.
    PreemptTime,
    /// the thread ended.
    Ended,
}

const HANDOFFS: [&str; 8] = ["from frontend", "to frontend", "sleep", "wait objects", "wait arbiter", "preempt (signal)", "preempt (time)", "ended"];

struct Stats {
    limits: [AtomicU64; 6],
    budget: AtomicU64,
    svc_left: [AtomicU64; 5],
    native_left: AtomicU64,
    svc: AtomicU64,
    declined: AtomicU64,
    other: AtomicU64,
    handoffs: [AtomicU64; 8],
    /// deterministic: guest ticks the clock jumped while everything was
    /// blocked, and how often.
    jumps: AtomicU64,
    jumped: AtomicU64,
    /// real time: the holder's sleeps while everything was blocked.
    idle_waits: AtomicU64,
    idle_ns: AtomicU64,
    lends: AtomicU64,
    lent_ns: AtomicU64,
    late_ns: AtomicU64,
    late_max: AtomicU64,
    late_over_ms: AtomicU64,
    sleep_over_ns: AtomicU64,
    sleep_over_max: AtomicU64,
    refreshes: AtomicU64,
    absorbed: AtomicU64,
    /// rt-merge: refreshes that found a game thread still running its work.
    busy: AtomicU64,
    /// rt-merge: refreshes held back for the game to finish its frame.
    settles: AtomicU64,
}

static STATS: Stats = Stats {
    limits: [const { AtomicU64::new(0) }; 6],
    budget: AtomicU64::new(0),
    svc_left: [const { AtomicU64::new(0) }; 5],
    native_left: AtomicU64::new(0),
    svc: AtomicU64::new(0),
    declined: AtomicU64::new(0),
    other: AtomicU64::new(0),
    handoffs: [const { AtomicU64::new(0) }; 8],
    jumps: AtomicU64::new(0),
    jumped: AtomicU64::new(0),
    idle_waits: AtomicU64::new(0),
    idle_ns: AtomicU64::new(0),
    lends: AtomicU64::new(0),
    lent_ns: AtomicU64::new(0),
    late_ns: AtomicU64::new(0),
    late_max: AtomicU64::new(0),
    late_over_ms: AtomicU64::new(0),
    sleep_over_ns: AtomicU64::new(0),
    sleep_over_max: AtomicU64::new(0),
    refreshes: AtomicU64::new(0),
    absorbed: AtomicU64::new(0),
    busy: AtomicU64::new(0),
    settles: AtomicU64::new(0),
};

/// rt-merge: running totals for the frontend's timing log (real time):
/// refreshes absorbed, refreshes that found the game busy, time lent and
/// time the threads slept with everything blocked, in nanoseconds.
pub fn totals() -> (u64, u64, u64, u64) {
    (STATS.absorbed.load(Relaxed), STATS.busy.load(Relaxed), STATS.lent_ns.load(Relaxed), STATS.idle_ns.load(Relaxed))
}

/// the limit of the budget given last (one thread runs guest code at a time).
static LIMIT: AtomicU8 = AtomicU8::new(0);
/// whether the last run ended at its budget, for the hand-off after it.
static LAST_BUDGET: AtomicU8 = AtomicU8::new(0);

#[inline]
pub(crate) fn note_limit(limit: Limit) {
    LIMIT.store(limit as u8, Relaxed);
}

#[inline]
pub(crate) fn note_end(end: End) {
    LAST_BUDGET.store((end == End::Budget) as u8, Relaxed);
    match end {
        End::Budget => {
            STATS.budget.fetch_add(1, Relaxed);
            STATS.limits[LIMIT.load(Relaxed) as usize].fetch_add(1, Relaxed);
        }
        End::SvcLeft => {}
        End::NativeLeft => {
            STATS.native_left.fetch_add(1, Relaxed);
        }
        End::Svc => {
            STATS.svc.fetch_add(1, Relaxed);
        }
        End::Declined => {
            STATS.declined.fetch_add(1, Relaxed);
        }
        End::Other => {
            STATS.other.fetch_add(1, Relaxed);
        }
    }
}

#[inline]
pub(crate) fn note_left(left: Left) {
    STATS.svc_left[left as usize].fetch_add(1, Relaxed);
}

#[inline]
pub(crate) fn note_jump(ticks: u64) {
    STATS.jumps.fetch_add(1, Relaxed);
    STATS.jumped.fetch_add(ticks, Relaxed);
}

/// the reason for a hand-off from the thread of seat guest id (None: the
/// frontend) to another.
pub(crate) fn note_handoff(system: &System, from: Option<u32>, to_driver: bool) {
    let why = match (from, to_driver) {
        (_, true) => Handoff::ToDriver,
        (None, false) => Handoff::FromDriver,
        (Some(guest), false) => match system.kernel.threads.iter().find(|thread| thread.guest_id == guest).map(|thread| thread.status) {
            Some(ThreadStatus::Sleeping) => Handoff::Sleep,
            Some(ThreadStatus::WaitSync) => Handoff::Wait,
            Some(ThreadStatus::WaitArbiter) => Handoff::Arbiter,
            Some(ThreadStatus::Ready | ThreadStatus::Running) if LAST_BUDGET.load(Relaxed) == 1 => Handoff::PreemptTime,
            Some(ThreadStatus::Ready | ThreadStatus::Running) => Handoff::PreemptSignal,
            _ => Handoff::Ended,
        },
    };
    STATS.handoffs[why as usize].fetch_add(1, Relaxed);
}

/// where runs ran out of budget, by thread and address (FATES_SCHED_STATS).
static BUDGET_SITES: std::sync::Mutex<Option<std::collections::HashMap<(String, u32), u64>>> = std::sync::Mutex::new(None);

/// notes where a run ran out of budget, with FATES_SCHED_STATS.
pub(crate) fn note_budget_site(system: &System) {
    if !stats_wanted() {
        return;
    }
    let thread = system.kernel.current().map_or("?".to_owned(), |thread| thread.name.clone());
    let pc = system.cpu.regs[15] | system.cpu.cpsr.thumb as u32;
    *BUDGET_SITES.lock().unwrap().get_or_insert_with(Default::default).entry((thread, pc)).or_default() += 1;
}

/// instructions run by thread (FATES_SCHED_STATS).
static THREAD_RUNS: std::sync::Mutex<Option<std::collections::HashMap<String, (u64, u64)>>> = std::sync::Mutex::new(None);

/// counts a run's instructions for its thread, with FATES_SCHED_STATS.
pub(crate) fn note_run(system: &System, ran: u64) {
    if !stats_wanted() {
        return;
    }
    let thread = system.kernel.current().map_or("?".to_owned(), |thread| format!("{} (prio {})", thread.name, thread.priority));
    let mut runs = THREAD_RUNS.lock().unwrap();
    let entry = runs.get_or_insert_with(Default::default).entry(thread).or_default();
    entry.0 += ran;
    entry.1 += 1;
}

/// whether FATES_SCHED_STATS asks for the counts.
pub fn stats_wanted() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("FATES_SCHED_STATS").is_ok_and(|value| value != "0"))
}

/// with FATES_SCHED_STATS, the counts every 1800 refreshes (a window has
/// no end of run to print them at).
pub(crate) fn periodic(frames: u64) {
    if stats_wanted() && frames > 0 && frames.is_multiple_of(1800) {
        for line in summary(frames) {
            eprintln!("{line} [frame {frames}]");
        }
    }
}

/// the process's CPU time so far (user, kernel).
pub fn process_cpu_time() -> (Duration, Duration) {
    #[cfg(windows)]
    {
        #[repr(C)]
        #[derive(Default, Clone, Copy)]
        struct FileTime {
            low: u32,
            high: u32,
        }
        unsafe extern "system" {
            fn GetCurrentProcess() -> isize;
            fn GetProcessTimes(process: isize, creation: *mut FileTime, exit: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
        }
        let (mut a, mut b, mut kernel, mut user) = Default::default();
        // SAFETY: a plain query on this process
        if unsafe { GetProcessTimes(GetCurrentProcess(), &mut a, &mut b, &mut kernel, &mut user) } != 0 {
            let time = |t: FileTime| Duration::from_nanos((((t.high as u64) << 32) | t.low as u64) * 100);
            return (time(user), time(kernel));
        }
    }
    (Duration::ZERO, Duration::ZERO)
}

/// the counts, for the end of a run: lines starting "sched:".
pub fn summary(frames: u64) -> Vec<String> {
    let frames = frames.max(1) as f64;
    let get = |counter: &AtomicU64| counter.load(Relaxed);
    let per = |count: u64| count as f64 / frames;
    let mut lines = Vec::new();
    let budget = get(&STATS.budget);
    let limits: Vec<String> = LIMITS.iter().zip(&STATS.limits).filter(|(_, c)| get(c) > 0).map(|(name, c)| format!("{name} {} ({:.1}/frame)", get(c), per(get(c)))).collect();
    let lefts: Vec<String> = LEFTS.iter().zip(&STATS.svc_left).filter(|(_, c)| get(c) > 0).map(|(name, c)| format!("{name} {}", get(c))).collect();
    let svc_left: u64 = STATS.svc_left.iter().map(get).sum();
    lines.push(format!(
        "sched: mode {}; run ends: budget {budget} ({:.1}/frame: {}), svc in place left {svc_left} ({}), native call left {}, svc stopped {}, declined {}, other {}",
        match mode() {
            Mode::Deterministic => "deterministic",
            Mode::RealTime => "real time",
        },
        per(budget),
        limits.join(", "),
        lefts.join(", "),
        get(&STATS.native_left),
        get(&STATS.svc),
        get(&STATS.declined),
        get(&STATS.other)
    ));
    let handoffs: u64 = STATS.handoffs.iter().map(get).sum();
    let parts: Vec<String> = HANDOFFS.iter().zip(&STATS.handoffs).filter(|(_, c)| get(c) > 0).map(|(name, c)| format!("{name} {} ({:.1}/frame)", get(c), per(get(c)))).collect();
    lines.push(format!("sched: hand-offs {handoffs} ({:.1}/frame): {}", per(handoffs), parts.join(", ")));
    let jumps = get(&STATS.jumps);
    if jumps > 0 {
        lines.push(format!(
            "sched: everything blocked {jumps} times ({:.1}/frame), the clock jumped {:.1}% of the guest time",
            per(jumps),
            get(&STATS.jumped) as f64 * 100.0 / (frames * CYCLES_PER_FRAME as f64)
        ));
    }
    if realtime() {
        let lends = get(&STATS.lends).max(1);
        lines.push(format!(
            "sched: real time: {} refreshes ({} absorbed after stalls), {} lends ({:.2} ms each, ended {:.1} us late on average, at most {:.2} ms, {} over 1 ms), {} idle sleeps ({:.1} ms/frame, woke {:.1} us late on average, at most {:.2} ms), quantum {}",
            get(&STATS.refreshes),
            get(&STATS.absorbed),
            get(&STATS.lends),
            get(&STATS.lent_ns) as f64 / lends as f64 / 1e6,
            get(&STATS.late_ns) as f64 / lends as f64 / 1e3,
            get(&STATS.late_max) as f64 / 1e6,
            get(&STATS.late_over_ms),
            get(&STATS.idle_waits),
            get(&STATS.idle_ns) as f64 / frames / 1e6,
            get(&STATS.sleep_over_ns) as f64 / get(&STATS.idle_waits).max(1) as f64 / 1e3,
            get(&STATS.sleep_over_max) as f64 / 1e6,
            quantum()
        ));        // rt-merge
        lines.push(format!(
            "sched: real time: {} refreshes held back for the game's frame (settled), {} found a thread still busy",
            get(&STATS.settles),
            get(&STATS.busy)
        ));
    }
    if let Some(runs) = THREAD_RUNS.lock().unwrap().as_ref() {
        let mut runs: Vec<_> = runs.iter().collect();
        runs.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
        for (thread, (instructions, count)) in runs {
            lines.push(format!("sched:   thread {thread}: {:.0} instructions/frame in {:.1} runs/frame", *instructions as f64 / frames, *count as f64 / frames));
        }
    }
    if let Some(sites) = BUDGET_SITES.lock().unwrap().as_ref() {
        let mut sites: Vec<_> = sites.iter().collect();
        sites.sort_by(|a, b| b.1.cmp(a.1));
        for ((thread, pc), count) in sites.into_iter().take(12) {
            lines.push(format!("sched:   budget ran out in {thread} at 0x{pc:08X}: {count}"));
        }
    }
    let (user, kernel) = process_cpu_time();
    lines.push(format!(
        "sched: process cpu time {:.2?} user + {:.2?} kernel = {:.2} ms/frame",
        user,
        kernel,
        (user + kernel).as_secs_f64() * 1e3 / frames
    ));
    lines
}
