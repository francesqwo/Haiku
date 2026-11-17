//! os2: the native scheduler and synchronisation of the game's threads.
//!
//! This is the 3DS kernel's thread and synchronisation layer as the game
//! sees it through its SVCs (nn::os::Thread, Event, Mutex, Semaphore,
//! Timer, LightEvent / CriticalSection through svcArbitrateAddress,
//! svcSleepThread, svcWaitSynchronization), implemented on the host and
//! driving the host threads of pcos::threads:
//!
//! - a thread that waits (arbiter, events, mutexes, semaphores, timers,
//!   other threads, a sleep) is marked blocked here and its host thread
//!   parks until the scheduler hands it the baton again;
//! - wake-ups come from native signals: the game's own SignalEvent /
//!   ArbitrateAddress / ReleaseMutex / ReleaseSemaphore, the timers and
//!   timeouts on the guest clock, and the native layers' events
//!   (signal_event: the GPU interrupt event from the GSP relay, y2r's
//!   transfer end), and the native sound layer's sleeps;
//! - the scheduling pass picks the next thread by the console's policy:
//!   strict priority, the running thread keeps the core unless something
//!   more urgent is ready, round robin within a priority, and a thread that
//!   runs four frames without waiting shares the core with the ones below
//!   it (titles' threads spin on work another thread does).
//!
//! The policy and every answer are the same as those of Zakuro's emulated
//! kernel (kernel/mod.rs, kernel/svc.rs), which this replaces: replays
//! recorded on it stay exact, which is checked by comparing frame dumps.
//! The thread records and the kernel object and handle tables are the same
//! data (`Kernel::threads`, `objects`, `handles`), so save states keep
//! working and the services still emulated can signal the same events.
//!
//! On with the host threads (FATES_NATIVE_THREADS, default on); with them
//! off, Zakuro's kernel schedules as before.

use zakuro_common::result::errors;
use zakuro_cpu::Bus;

use super::stats;
use crate::kernel::object::{KObject, ObjectId, CURRENT_THREAD};
use crate::kernel::sync::{self, ArbitrationType, ResetType};
use crate::kernel::thread::{nanos_to_ticks, ThreadId, ThreadStatus, WaitResult, WaitSyscall};
use crate::kernel::{Kernel, LOWEST_PRIORITY};
use crate::System;

/// how long a thread may run without waiting before the threads below it
/// get a share, four frames.
const STARVATION_TICKS: u64 = 4 * crate::CYCLES_PER_FRAME;

/// how long a yielding thread steps aside when others are ready, about a
/// scheduler time slice.
const YIELD_TICKS: u64 = 20_000;

/// whether the native scheduler is the one in use: with the host threads.
pub fn enabled() -> bool {
    super::threads::enabled()
}

// -- the scheduling pass ----------------------------------------------------

/// the scheduling pass a step makes when it is due: wakes what is due,
/// picks the thread to run, switches the core's registers to it, and gives
/// a thread that was waiting the result of its wait.
pub fn reschedule(system: &mut System) {
    system.kernel.reschedule_pending = false;
    if schedule(system) {
        system.idle_interrupted(true);
    }
    apply_wait_result(system);
}

/// wakes anything whose wait has been satisfied, then switches to the
/// highest-priority runnable thread. whether the running thread changed.
fn schedule(system: &mut System) -> bool {
    let tick = system.cpu.cycles;
    let System { kernel, cpu, .. } = system;
    fire_timers(kernel, tick);
    for id in 0..kernel.threads.len() as ThreadId {
        try_satisfy_wait(kernel, id, tick);
    }
    for id in std::mem::take(&mut kernel.pulsed) {
        match kernel.objects.get_mut(id) {
            Some(KObject::Event(event)) => event.signaled = false,
            Some(KObject::Timer(timer)) => timer.signaled = false,
            _ => {}
        }
    }

    if let Some(current) = kernel.current_thread {
        let thread = &mut kernel.threads[current as usize];
        if !thread.is_runnable() {
            thread.waited = true;
        }
    }
    let mut next = pick_next(kernel);
    kernel.all_blocked = next.is_none();
    if let Some(chosen) = next {
        let thread = &mut kernel.threads[chosen as usize];
        if thread.waited {
            thread.waited = false;
            thread.running_since = tick;
        }
        if tick.saturating_sub(thread.running_since) > STARVATION_TICKS {
            let priority = thread.priority;
            kernel.relief = !kernel.relief;
            if kernel.relief {
                next = pick_other(kernel, chosen, priority).or(next);
            }
        }
    }
    // savehang: in real time, a thread kept from the core too long gets a
    // turn, whatever runs above it (see Aging)
    if super::rt::realtime() {
        next = AGING.lock().unwrap().pass(&kernel.threads, tick, next);
    }
    if let Some(chosen) = next {
        let thread = &mut kernel.threads[chosen as usize];
        if thread.waited {
            thread.waited = false;
            thread.running_since = tick;
        }
    }

    match (kernel.current_thread, next) {
        (Some(current), Some(next)) if current == next => {
            kernel.threads[current as usize].status = ThreadStatus::Running;
            false
        }
        (current, Some(next)) => {
            if let Some(current) = current {
                let thread = &mut kernel.threads[current as usize];
                thread.context.save_from(cpu);
                if thread.status == ThreadStatus::Running {
                    thread.status = ThreadStatus::Ready;
                }
            }
            let thread = &mut kernel.threads[next as usize];
            thread.status = ThreadStatus::Running;
            thread.context.restore_to(cpu);
            kernel.current_thread = Some(next);
            super::hang::note_ran(next); // savehang
            true
        }
        (Some(current), None) => {
            let thread = &mut kernel.threads[current as usize];
            thread.context.save_from(cpu);
            if thread.status == ThreadStatus::Running {
                thread.status = ThreadStatus::Ready;
            }
            kernel.current_thread = None;
            true
        }
        (None, None) => false,
    }
}

/// savehang: how long, in guest ticks, a runnable thread may go without the
/// core in real time before it gets a turn, and how long that turn lasts:
/// four refreshes (as the starvation rule), and a millisecond.
pub const AGE_TICKS: u64 = 4 * crate::CYCLES_PER_FRAME;
pub const TURN_TICKS: u64 = crate::kernel::thread::CPU_CLOCK_HZ / 1000;

/// savehang: real time's guarantee that every runnable thread runs.
///
/// The console's policy is strict priority, and the game's threads are
/// written for a console whose core has time left over in every frame: the
/// lowest ones (the Backup controller, which reads and writes the saves,
/// runs at 63, the lowest priority) run in what the others leave. In
/// deterministic mode that holds, as the guest clock gives every frame the
/// console's time however long the host takes. In real time the threads
/// only have what the frontend lends them on the wall clock; when the PC
/// falls behind (the window catching up, 8x pictures, a texture pack
/// loading, other programs), the threads above can take every lent
/// millisecond, frame after frame. The starvation rule does not help then:
/// it only looks at a thread that ran four frames without waiting, and gives
/// the turn to the most urgent thread below it, so a thread that waits each
/// refresh (for the VBlank) and works the rest of it, or a busy thread
/// between the two, keeps the lowest thread from the core for good. The save
/// list then waits forever for its slots' headers.
///
/// So in real time a thread runnable for AGE_TICKS without being picked is
/// picked, ahead of everything, for TURN_TICKS or until it blocks; the
/// oldest such first. Deterministic mode never comes here.
#[derive(Default)]
pub struct Aging {
    /// the tick each thread slot became runnable without the core.
    waiting_since: Vec<Option<u64>>,
    /// the thread on a turn, and when it ends.
    turn: Option<(ThreadId, u64)>,
    /// turns given, for the dump.
    pub turns: u64,
}

pub(crate) static AGING: std::sync::Mutex<Aging> = std::sync::Mutex::new(Aging { waiting_since: Vec::new(), turn: None, turns: 0 });

impl Aging {
    /// one scheduling pass: the thread to run, given the one the policy
    /// picked.
    pub fn pass(&mut self, threads: &[crate::kernel::thread::Thread], tick: u64, picked: Option<ThreadId>) -> Option<ThreadId> {
        self.waiting_since.resize(threads.len(), None);
        let mut next = picked;
        match self.turn {
            Some((id, until)) if tick < until && threads.get(id as usize).is_some_and(|t| t.is_runnable()) => next = Some(id),
            _ => {
                self.turn = None;
                let oldest = (0..threads.len())
                    .filter(|&id| Some(id as ThreadId) != picked && threads[id].is_runnable())
                    .filter_map(|id| self.waiting_since[id].filter(|&since| tick.saturating_sub(since) >= AGE_TICKS).map(|since| (since, id)))
                    .min();
                if let Some((_, id)) = oldest {
                    next = Some(id as ThreadId);
                    self.turn = Some((id as ThreadId, tick.saturating_add(TURN_TICKS)));
                    self.turns += 1;
                }
            }
        }
        for (id, thread) in threads.iter().enumerate() {
            let waiting = thread.is_runnable() && Some(id as ThreadId) != next;
            let since = &mut self.waiting_since[id];
            *since = if waiting { Some(since.unwrap_or(tick)) } else { None };
        }
        next
    }
}

/// the most urgent runnable thread besides one, at its priority or below.
fn pick_other(kernel: &Kernel, chosen: ThreadId, priority: u32) -> Option<ThreadId> {
    kernel
        .threads
        .iter()
        .enumerate()
        .filter(|&(id, t)| id as ThreadId != chosen && t.is_runnable() && t.priority >= priority)
        .min_by_key(|(_, t)| t.priority)
        .map(|(id, _)| id as ThreadId)
}

/// the highest-priority runnable thread, round-robining within a priority
/// by preferring the one after the current thread.
fn pick_next(kernel: &Kernel) -> Option<ThreadId> {
    let best = kernel.threads.iter().filter(|t| t.is_runnable()).map(|t| t.priority).min()?;
    // the running thread keeps the core unless something strictly more
    // urgent is ready
    if let Some(current) = kernel.current_thread {
        let thread = &kernel.threads[current as usize];
        if thread.status == ThreadStatus::Running && thread.priority <= best {
            return Some(current);
        }
    }
    let count = kernel.threads.len();
    let start = kernel.current_thread.map_or(0, |id| id as usize + 1);
    (0..count)
        .map(|offset| (start + offset) % count)
        .find(|&index| {
            let t = &kernel.threads[index];
            t.is_runnable() && t.priority == best
        })
        .map(|index| index as ThreadId)
}

/// writes back the result registers of a wait a thread was released from,
/// when it is the thread that runs now.
fn apply_wait_result(system: &mut System) {
    let Some(id) = system.kernel.current_thread else { return };
    let thread = system.kernel.thread_mut(id);
    let (Some(result), Some(syscall)) = (thread.wait_result.take(), thread.wait_syscall.take()) else {
        return;
    };
    let regs = &mut system.cpu.regs;
    match (syscall, result) {
        (WaitSyscall::WaitSynchronization1, WaitResult::Signaled(_)) => regs[0] = 0,
        (WaitSyscall::WaitSynchronizationN, WaitResult::Signaled(index)) => {
            regs[0] = 0;
            regs[1] = index as u32;
        }
        (WaitSyscall::ArbitrateAddress, WaitResult::Signaled(_)) => regs[0] = 0,
        (_, WaitResult::TimedOut) => regs[0] = errors::TIMEOUT.0,
        (WaitSyscall::SleepThread, _) => regs[0] = 0,
    }
}

// -- timers, waits, wake-ups -------------------------------------------------

/// fires the timers whose time has come, once however many periods went
/// by, and arms periodic ones for their first period after tick. a pulse
/// releases what waits on it then and clears after. whether any fired.
fn fire_timers(kernel: &mut Kernel, tick: u64) -> bool {
    let mut fired = false;
    for &id in &kernel.timers {
        if let Some(KObject::Timer(timer)) = kernel.objects.get_mut(id) {
            let Some(at) = timer.fire_at.filter(|&at| at <= tick) else { continue };
            timer.signaled = true;
            timer.fire_at =
                (timer.interval > 0).then(|| at.saturating_add(timer.interval.saturating_mul((tick - at) / timer.interval + 1)));
            if timer.reset_type == ResetType::Pulse {
                kernel.pulsed.push(id);
            }
            fired = true;
        }
    }
    fired
}

/// fires what came due before the game sets, clears, cancels or waits on a
/// timer, as the console's interrupt had by then. a pulse nobody waits on
/// is lost.
fn catch_up_timers(kernel: &mut Kernel, tick: u64) {
    if !fire_timers(kernel, tick) {
        return;
    }
    kernel.reschedule_pending = true;
    let threads = &kernel.threads;
    let objects = &mut kernel.objects;
    kernel.pulsed.retain(|&id| {
        let waited = threads.iter().any(|t| t.status == ThreadStatus::WaitSync && t.wait_objects.contains(&id));
        if !waited {
            if let Some(KObject::Timer(timer)) = objects.get_mut(id) {
                timer.signaled = false;
            }
        }
        waited
    });
}

/// the earliest tick a blocked thread or a timer one waits on wants the
/// scheduler at.
pub fn next_event(kernel: &Kernel) -> Option<u64> {
    if !enabled() {
        return kernel.next_event();
    }
    let mut best: Option<u64> = None;
    let mut any_blocked = false;
    for thread in &kernel.threads {
        if thread.status.is_blocked() {
            any_blocked = true;
            if let Some(at) = thread.wakeup_at {
                best = Some(best.map_or(at, |b: u64| b.min(at)));
            }
        }
    }
    if !any_blocked {
        return None;
    }
    for &id in &kernel.timers {
        let waited = kernel.threads.iter().any(|t| t.status.is_blocked() && t.wait_objects.contains(&id));
        if !waited {
            continue;
        }
        if let Some(KObject::Timer(timer)) = kernel.objects.get(id) {
            if let Some(at) = timer.fire_at {
                best = Some(best.map_or(at, |b: u64| b.min(at)));
            }
        }
    }
    best
}

/// whether a thread other than the running one could take the core.
pub fn others_runnable(kernel: &Kernel) -> bool {
    if !enabled() {
        return kernel.others_runnable();
    }
    kernel.threads.iter().enumerate().any(|(id, thread)| Some(id as ThreadId) != kernel.current_thread && thread.is_runnable())
}

/// checks one blocked thread's condition and readies it when it is met.
fn try_satisfy_wait(kernel: &mut Kernel, id: ThreadId, tick: u64) -> bool {
    let thread = &kernel.threads[id as usize];
    if !thread.status.is_blocked() {
        return false;
    }
    let timed_out = thread.wakeup_at.is_some_and(|at| tick >= at);
    match thread.status {
        ThreadStatus::Sleeping => {
            if timed_out {
                end_wait(kernel, id);
                kernel.threads[id as usize].status = ThreadStatus::Ready;
                return true;
            }
        }
        ThreadStatus::WaitSync => {
            let wait_all = thread.wait_all;
            let count = thread.wait_objects.len();
            let signaled = |k: usize| is_signaled(kernel, kernel.threads[id as usize].wait_objects[k], id);
            let satisfied = if wait_all { (0..count).all(signaled).then_some(0) } else { (0..count).position(signaled) };
            if let Some(index) = satisfied {
                if wait_all {
                    for k in 0..count {
                        let object = kernel.threads[id as usize].wait_objects[k];
                        acquire(kernel, object, id);
                    }
                } else {
                    let object = kernel.threads[id as usize].wait_objects[index];
                    acquire(kernel, object, id);
                }
                end_wait(kernel, id);
                let thread = &mut kernel.threads[id as usize];
                thread.wait_result = Some(WaitResult::Signaled(index));
                thread.status = ThreadStatus::Ready;
                return true;
            }
            if timed_out {
                end_wait(kernel, id);
                let thread = &mut kernel.threads[id as usize];
                thread.wait_result = Some(WaitResult::TimedOut);
                thread.status = ThreadStatus::Ready;
                return true;
            }
        }
        // an arbiter wait ends when a thread signals the address; only its
        // timeout is handled here
        ThreadStatus::WaitArbiter if timed_out => {
            end_wait(kernel, id);
            let thread = &mut kernel.threads[id as usize];
            thread.wait_result = Some(WaitResult::TimedOut);
            thread.status = ThreadStatus::Ready;
            return true;
        }
        _ => {}
    }
    false
}

/// whether waiting on object would succeed now for waiter.
fn is_signaled(kernel: &Kernel, object: ObjectId, waiter: ThreadId) -> bool {
    match kernel.objects.get(object) {
        Some(KObject::Event(event)) => event.signaled,
        Some(KObject::Timer(timer)) => timer.signaled,
        Some(KObject::Semaphore(semaphore)) => semaphore.count > 0,
        Some(KObject::Mutex(mutex)) => mutex.owner.is_none() || mutex.owner == Some(waiter),
        Some(KObject::Thread(id)) => kernel.threads[*id as usize].status == ThreadStatus::Dead,
        // a request to a service is answered before the svc returns
        Some(KObject::ClientSession(_)) | Some(KObject::ClientPort(_)) => true,
        Some(KObject::SharedMemory(_)) | Some(KObject::Process) | Some(KObject::ResourceLimit) | Some(KObject::AddressArbiter(_)) => true,
        None => true,
    }
}

/// consumes the signal a successful wait picked up.
fn acquire(kernel: &mut Kernel, object: ObjectId, waiter: ThreadId) {
    match kernel.objects.get_mut(object) {
        Some(KObject::Event(event)) => {
            if event.reset_type == ResetType::OneShot {
                event.signaled = false;
            }
        }
        Some(KObject::Timer(timer)) => {
            if timer.reset_type == ResetType::OneShot {
                timer.signaled = false;
            }
        }
        Some(KObject::Semaphore(semaphore)) => semaphore.count -= 1,
        Some(KObject::Mutex(mutex)) => {
            mutex.owner = Some(waiter);
            mutex.lock_count += 1;
        }
        _ => {}
    }
}

/// blocks the current thread on objects; what it waits on is kept alive
/// while it waits.
fn begin_wait(kernel: &mut Kernel, objects: Vec<ObjectId>, wait_all: bool, timeout_ticks: Option<u64>, tick: u64) {
    let Some(id) = kernel.current_thread else { return };
    for &object in &objects {
        kernel.objects.add_ref(object);
    }
    let thread = &mut kernel.threads[id as usize];
    thread.wait_objects = objects;
    thread.wait_all = wait_all;
    thread.wakeup_at = timeout_ticks.map(|t| tick.saturating_add(t));
    thread.wait_result = None;
    thread.status = ThreadStatus::WaitSync;
    kernel.reschedule_pending = true;
}

/// ends a thread's wait, letting go of what it kept.
fn end_wait(kernel: &mut Kernel, id: ThreadId) {
    for object in std::mem::take(&mut kernel.threads[id as usize].wait_objects) {
        kernel.objects.release(object);
    }
    kernel.threads[id as usize].clear_wait();
}

/// puts the current thread to sleep for ticks: svcSleepThread, and the
/// console timing of the native calls that replace IPC requests.
pub fn sleep_current(system: &mut System, ticks: u64, tick: u64) {
    if !enabled() {
        return system.kernel.sleep_current(ticks, tick);
    }
    let kernel = &mut system.kernel;
    let Some(id) = kernel.current_thread else { return };
    let thread = &mut kernel.threads[id as usize];
    thread.clear_wait();
    thread.wakeup_at = Some(tick.saturating_add(ticks));
    thread.status = ThreadStatus::Sleeping;
    kernel.reschedule_pending = true;
}

/// online: blocks the current thread on an event until it is signalled or
/// the timeout passes, for a native request the game waits on (local
/// wireless's ConnectNetwork waits for the host's answer).
pub fn wait_current(system: &mut System, object: ObjectId, timeout_ticks: Option<u64>, tick: u64) {
    if !enabled() {
        return system.kernel.begin_wait(vec![object], false, timeout_ticks, tick);
    }
    begin_wait(&mut system.kernel, vec![object], false, timeout_ticks, tick);
}

/// signals an event, readying what waits on it at the next scheduling
/// pass: the native wake-up the GPU interrupt relay, y2r and the game's
/// SignalEvent use.
pub fn signal_event(system: &mut System, object: ObjectId) {
    if !enabled() {
        return system.kernel.signal_event(object);
    }
    let kernel = &mut system.kernel;
    let waited = kernel.threads.iter().any(|t| t.status == ThreadStatus::WaitSync && t.wait_objects.contains(&object));
    if let Some(KObject::Event(event)) = kernel.objects.get_mut(object) {
        // a pulse wakes what waits on it now and is gone after, lost when
        // nothing does
        if event.reset_type == ResetType::Pulse {
            if !waited {
                return;
            }
            kernel.pulsed.push(object);
        }
        event.signaled = true;
        kernel.reschedule_pending = true;
    }
}

/// ends a thread, giving back every mutex it holds, as the console does.
fn end_thread(kernel: &mut Kernel, id: ThreadId) {
    end_wait(kernel, id);
    let thread = &mut kernel.threads[id as usize];
    thread.status = ThreadStatus::Dead;
    thread.wait_result = None;
    thread.wait_syscall = None;
    let tls = thread.tls;
    kernel.free_tls.insert(tls);
    crate::recompiled::forget_tails(tls); // hooks: its overrides' tails
    let held: Vec<ObjectId> = kernel
        .objects
        .iter()
        .filter(|(_, object)| matches!(object, KObject::Mutex(mutex) if mutex.owner == Some(id)))
        .map(|(object, _)| object)
        .collect();
    for object in held {
        if let Some(KObject::Mutex(mutex)) = kernel.objects.get_mut(object) {
            log::debug!("thread {} ended holding {}, it goes to whoever waits on it", kernel.threads[id as usize].name, mutex.name);
            mutex.owner = None;
            mutex.lock_count = 0;
        }
    }
    if kernel.current_thread == Some(id) {
        kernel.current_thread = None;
    }
    kernel.reschedule_pending = true;
}

/// ends a thread from the run loop (its entry point returned, or it
/// branched to unmapped memory).
pub fn end_thread_of(system: &mut System, id: ThreadId) {
    if !enabled() {
        return system.kernel.end_thread(id);
    }
    end_thread(&mut system.kernel, id);
}

// -- the SVCs -------------------------------------------------------------

/// runs a thread or synchronisation SVC natively. false when it is not
/// one, or the native scheduler is off.
pub fn svc(system: &mut System, number: u32) -> bool {
    if !enabled() {
        return false;
    }
    let name = match number {
        0x08 => {
            create_thread(system);
            "sched.CreateThread"
        }
        0x09 => {
            if let Some(id) = system.kernel.current_thread {
                log::debug!("thread {} exited", system.kernel.thread(id).name);
                end_thread(&mut system.kernel, id);
            }
            "sched.ExitThread"
        }
        0x0A => {
            sleep_thread(system);
            "sched.SleepThread"
        }
        0x0B => {
            let handle = system.cpu.regs[1];
            match resolve_thread(system, handle) {
                Some(id) => {
                    system.cpu.regs[0] = 0;
                    system.cpu.regs[1] = system.kernel.thread(id).priority;
                }
                None => system.cpu.regs[0] = invalid_handle(system, handle, "svcGetThreadPriority"),
            }
            "sched.GetThreadPriority"
        }
        0x0C => {
            let (handle, priority) = (system.cpu.regs[0], system.cpu.regs[1]);
            match resolve_thread(system, handle) {
                Some(id) => {
                    system.kernel.thread_mut(id).priority = priority.min(LOWEST_PRIORITY);
                    system.kernel.reschedule_pending = true;
                    system.cpu.regs[0] = 0;
                }
                None => system.cpu.regs[0] = invalid_handle(system, handle, "svcSetThreadPriority"),
            }
            "sched.SetThreadPriority"
        }
        0x13 => {
            let mut mutex = sync::Mutex::new("Mutex");
            if system.cpu.regs[1] != 0 {
                mutex.owner = system.kernel.current_thread;
                mutex.lock_count = 1;
            }
            create(system, KObject::Mutex(mutex), "Mutex");
            "sched.CreateMutex"
        }
        0x14 => {
            release_mutex(system);
            "sched.ReleaseMutex"
        }
        0x15 => {
            let (count, max_count) = (system.cpu.regs[1] as i32, system.cpu.regs[2] as i32);
            create(system, KObject::Semaphore(sync::Semaphore { name: "Semaphore".into(), count, max_count }), "Semaphore");
            "sched.CreateSemaphore"
        }
        0x16 => {
            release_semaphore(system);
            "sched.ReleaseSemaphore"
        }
        0x17 => {
            let reset_type = ResetType::from_raw(system.cpu.regs[1]);
            let (_, handle) = system.kernel.create_event(reset_type, "Event");
            system.cpu.regs[0] = 0;
            system.cpu.regs[1] = handle;
            "sched.CreateEvent"
        }
        0x18 => {
            let handle = system.cpu.regs[0];
            match system.kernel.resolve(handle) {
                Some(object) => {
                    signal_event(system, object);
                    system.cpu.regs[0] = 0;
                }
                None => system.cpu.regs[0] = invalid_handle(system, handle, "svcSignalEvent"),
            }
            "sched.SignalEvent"
        }
        0x19 => {
            let handle = system.cpu.regs[0];
            match system.kernel.resolve(handle) {
                Some(object) => {
                    if let Some(KObject::Event(event)) = system.kernel.objects.get_mut(object) {
                        event.signaled = false;
                    }
                    system.cpu.regs[0] = 0;
                }
                None => system.cpu.regs[0] = invalid_handle(system, handle, "svcClearEvent"),
            }
            "sched.ClearEvent"
        }
        0x1A => {
            let reset_type = ResetType::from_raw(system.cpu.regs[1]);
            let timer = sync::Timer { name: "Timer".into(), reset_type, signaled: false, fire_at: None, interval: 0 };
            let object = create(system, KObject::Timer(timer), "Timer");
            system.kernel.timers.push(object);
            "sched.CreateTimer"
        }
        0x1B => {
            set_timer(system);
            "sched.SetTimer"
        }
        0x1C | 0x1D => {
            let handle = system.cpu.regs[0];
            catch_up_timers(&mut system.kernel, system.cpu.cycles);
            if let Some(object) = system.kernel.resolve(handle) {
                if let Some(KObject::Timer(timer)) = system.kernel.objects.get_mut(object) {
                    if number == 0x1C {
                        timer.fire_at = None;
                    } else {
                        timer.signaled = false;
                    }
                }
            }
            system.cpu.regs[0] = 0;
            if number == 0x1C { "sched.CancelTimer" } else { "sched.ClearTimer" }
        }
        0x21 => {
            create(system, KObject::AddressArbiter(sync::AddressArbiter::default()), "AddressArbiter");
            "sched.CreateAddressArbiter"
        }
        0x22 => {
            arbitrate_address(system);
            "sched.ArbitrateAddress"
        }
        0x23 => {
            let handle = system.cpu.regs[0];
            let kernel = &mut system.kernel;
            kernel.handles.close(&mut kernel.objects, handle);
            system.cpu.regs[0] = 0;
            "sched.CloseHandle"
        }
        0x24 => {
            wait_synchronization1(system);
            "sched.WaitSynchronization1"
        }
        0x25 => {
            wait_synchronization_n(system);
            "sched.WaitSynchronizationN"
        }
        0x27 => {
            let handle = system.cpu.regs[1];
            match system.kernel.resolve(handle) {
                Some(object) => {
                    let kernel = &mut system.kernel;
                    let label = kernel.handles.label(handle).to_owned();
                    let duplicate = kernel.handles.duplicate_object(&mut kernel.objects, object, &label);
                    system.cpu.regs[0] = 0;
                    system.cpu.regs[1] = duplicate;
                }
                None => {
                    log::warn!("svcDuplicateHandle on invalid handle 0x{handle:X}");
                    system.cpu.regs[0] = errors::INVALID_HANDLE.0;
                }
            }
            "sched.DuplicateHandle"
        }
        0x35 => {
            // GetProcessId(out, process): the one process there is, whatever
            // the handle names
            system.cpu.regs[0] = 0;
            system.cpu.regs[1] = system.kernel.process_id;
            "sched.GetProcessId"
        }
        0x37 => {
            let handle = system.cpu.regs[1];
            match resolve_thread(system, handle) {
                Some(id) => {
                    system.cpu.regs[0] = 0;
                    system.cpu.regs[1] = system.kernel.thread(id).guest_id;
                }
                None => system.cpu.regs[0] = invalid_handle(system, handle, "svcGetThreadId"),
            }
            "sched.GetThreadId"
        }
        _ => return false,
    };
    stats::native(name);
    true
}

/// reports a handle an SVC could not resolve.
fn invalid_handle(system: &mut System, handle: u32, what: &str) -> u32 {
    log::warn!("{what}: handle 0x{handle:X} does not exist (called from 0x{:08X})", system.cpu.current_pc());
    errors::INVALID_HANDLE.0
}

fn resolve_thread(system: &mut System, handle: u32) -> Option<ThreadId> {
    if handle == CURRENT_THREAD {
        return system.kernel.current_thread;
    }
    match system.kernel.objects.get(system.kernel.handles.resolve(handle)?) {
        Some(KObject::Thread(id)) => Some(*id),
        _ => None,
    }
}

/// makes an object and a handle for it, the handle in r1.
fn create(system: &mut System, object: KObject, label: &str) -> ObjectId {
    let kernel = &mut system.kernel;
    let id = kernel.objects.insert(object);
    let handle = kernel.handles.create(&mut kernel.objects, id, label);
    system.cpu.regs[0] = 0;
    system.cpu.regs[1] = handle;
    id
}

/// svcCreateThread, r0 = priority, r1 = entry, r2 = argument, r3 = stack
/// top, r4 = processor id; the handle in r1. Its host thread starts when it
/// first runs (pcos::threads).
fn create_thread(system: &mut System) {
    let [priority, entry, arg, stack_top, processor_id] =
        [system.cpu.regs[0], system.cpu.regs[1], system.cpu.regs[2], system.cpu.regs[3], system.cpu.regs[4]];
    let name = format!("thread{}", system.kernel.next_guest_id);
    let id = system.kernel.create_thread(name, entry, stack_top, arg, priority, processor_id as i32);
    // the new thread's TLS page has to exist before it runs
    system.map_tls_page(id);
    let handle = system.kernel.thread_handle(id);
    log::debug!(
        "os2: CreateThread {} handle 0x{handle:X} priority {priority} core {} entry 0x{entry:08X} arg 0x{arg:08X} stack 0x{stack_top:08X}",
        system.kernel.thread(id).name,
        processor_id as i32
    );
    system.cpu.regs[0] = 0;
    system.cpu.regs[1] = handle;
}

/// svcSleepThread, r0:r1 = nanoseconds.
fn sleep_thread(system: &mut System) {
    let nanos = (system.cpu.regs[0] as u64) | ((system.cpu.regs[1] as u64) << 32);
    let tick = system.cpu.cycles;
    // a zero sleep is a yield: while others are ready they run for a while,
    // even below this thread's priority (titles polling a worker that way
    // would spin forever otherwise)
    let ticks = if nanos != 0 {
        nanos_to_ticks(nanos)
    } else if others_runnable(&system.kernel) {
        YIELD_TICKS
    } else if next_event(&system.kernel).is_some_and(|at| at <= tick) {
        // something is due, and the pass at the yield wakes it on time
        0
    } else {
        // nobody else can take the core, so the yield is a no-op, with no
        // scheduling pass; it still counts as letting the core go
        if let Some(thread) = system.kernel.current_mut() {
            thread.running_since = tick;
        }
        system.cpu.regs[0] = 0;
        return;
    };
    sleep_current(system, ticks, tick);
    system.cpu.regs[0] = 0;
}

fn release_mutex(system: &mut System) {
    let handle = system.cpu.regs[0];
    let Some(object) = system.kernel.resolve(handle) else {
        system.cpu.regs[0] = invalid_handle(system, handle, "svcReleaseMutex");
        return;
    };
    if let Some(KObject::Mutex(mutex)) = system.kernel.objects.get_mut(object) {
        mutex.lock_count = mutex.lock_count.saturating_sub(1);
        if mutex.lock_count == 0 {
            mutex.owner = None;
            system.kernel.reschedule_pending = true;
        }
        system.cpu.regs[0] = 0;
    } else {
        system.cpu.regs[0] = invalid_handle(system, handle, "svcReleaseMutex");
    }
}

/// svcReleaseSemaphore, r1 = handle, r2 = count; the count before in r1.
fn release_semaphore(system: &mut System) {
    let (handle, count) = (system.cpu.regs[1], system.cpu.regs[2] as i32);
    let Some(object) = system.kernel.resolve(handle) else {
        system.cpu.regs[0] = invalid_handle(system, handle, "svcReleaseSemaphore");
        return;
    };
    if let Some(KObject::Semaphore(semaphore)) = system.kernel.objects.get_mut(object) {
        let previous = semaphore.count;
        semaphore.count = (semaphore.count + count).min(semaphore.max_count);
        system.kernel.reschedule_pending = true;
        system.cpu.regs[0] = 0;
        system.cpu.regs[1] = previous as u32;
    } else {
        system.cpu.regs[0] = invalid_handle(system, handle, "svcReleaseSemaphore");
    }
}

/// svcSetTimer, r0 = handle, r2:r3 = initial delay, r1:r4 = interval, in
/// nanoseconds, a negative one taken as zero.
fn set_timer(system: &mut System) {
    let handle = system.cpu.regs[0];
    let nanos = |low: u32, high: u32| ((low as u64) | ((high as u64) << 32)) as i64;
    let initial = nanos(system.cpu.regs[2], system.cpu.regs[3]).max(0) as u64;
    let interval = nanos(system.cpu.regs[1], system.cpu.regs[4]).max(0) as u64;
    let now = system.cpu.cycles;
    catch_up_timers(&mut system.kernel, now);
    let Some(object) = system.kernel.resolve(handle) else {
        system.cpu.regs[0] = invalid_handle(system, handle, "svcSetTimer");
        return;
    };
    if let Some(KObject::Timer(timer)) = system.kernel.objects.get_mut(object) {
        timer.fire_at = Some(now.saturating_add(nanos_to_ticks(initial)));
        timer.interval = nanos_to_ticks(interval);
        timer.signaled = false;
        system.cpu.regs[0] = 0;
    } else {
        system.cpu.regs[0] = invalid_handle(system, handle, "svcSetTimer");
    }
}

/// svcArbitrateAddress, r0 = arbiter, r1 = address, r2 = type, r3 = value,
/// r4:r5 = timeout in nanoseconds (negative: none). The futex under
/// nn::os::LightEvent, LightSemaphore and CriticalSection.
fn arbitrate_address(system: &mut System) {
    let (handle, address) = (system.cpu.regs[0], system.cpu.regs[1]);
    let Some(kind) = ArbitrationType::from_raw(system.cpu.regs[2]) else {
        system.cpu.regs[0] = errors::INVALID_ENUM_VALUE.0;
        return;
    };
    let value = system.cpu.regs[3] as i32;
    let timeout_nanos = ((system.cpu.regs[4] as u64) | ((system.cpu.regs[5] as u64) << 32)) as i64;
    let Some(object) = system.kernel.resolve(handle) else {
        system.cpu.regs[0] = invalid_handle(system, handle, "svcArbitrateAddress");
        return;
    };
    system.cpu.regs[0] = 0;

    if kind == ArbitrationType::Signal {
        // wakes up to value threads parked on the address, all of them for
        // a negative value; entries a timed-out or ended thread left go
        // without counting
        let mut woken = 0;
        let unlimited = value < 0;
        let mut to_wake = Vec::new();
        let kernel = &mut system.kernel;
        let threads = &kernel.threads;
        if let Some(KObject::AddressArbiter(arbiter)) = kernel.objects.get_mut(object) {
            arbiter.waiters.retain(|&(thread, addr)| {
                if addr != address {
                    return true;
                }
                let parked = &threads[thread as usize];
                if parked.status != ThreadStatus::WaitArbiter || parked.wait_address != Some(address) || to_wake.contains(&thread) {
                    return false;
                }
                if unlimited || woken < value {
                    woken += 1;
                    to_wake.push(thread);
                    false
                } else {
                    true
                }
            });
        }
        for thread in to_wake {
            let thread = system.kernel.thread_mut(thread);
            thread.clear_wait();
            thread.wait_result = Some(WaitResult::Signaled(0));
            thread.status = ThreadStatus::Ready;
        }
        system.kernel.reschedule_pending = true;
        return;
    }

    // the waiting forms sleep only while the word is below value
    let current = system.memory.read32(address) as i32;
    if current >= value {
        return;
    }
    if kind.decrements() {
        system.memory.write32(address, (current - 1) as u32);
    }
    let Some(thread_id) = system.kernel.current_thread else { return };
    if let Some(KObject::AddressArbiter(arbiter)) = system.kernel.objects.get_mut(object) {
        arbiter.waiters.push((thread_id, address));
    }
    let now = system.cpu.cycles;
    let thread = system.kernel.thread_mut(thread_id);
    thread.clear_wait();
    thread.wait_address = Some(address);
    thread.wakeup_at =
        if kind.has_timeout() && timeout_nanos >= 0 { Some(now.saturating_add(nanos_to_ticks(timeout_nanos as u64))) } else { None };
    thread.status = ThreadStatus::WaitArbiter;
    thread.wait_syscall = Some(WaitSyscall::ArbitrateAddress);
    system.kernel.reschedule_pending = true;
}

/// svcWaitSynchronization1, r0 = handle, r2:r3 = timeout in nanoseconds.
fn wait_synchronization1(system: &mut System) {
    let handle = system.cpu.regs[0];
    catch_up_timers(&mut system.kernel, system.cpu.cycles);
    let timeout = (system.cpu.regs[2] as u64) | ((system.cpu.regs[3] as u64) << 32);
    let Some(object) = system.kernel.resolve(handle) else {
        log::warn!("svcWaitSynchronization1 on invalid handle 0x{handle:X}");
        system.cpu.regs[0] = errors::INVALID_HANDLE.0;
        return;
    };
    let Some(thread_id) = system.kernel.current_thread else {
        system.cpu.regs[0] = 0;
        return;
    };
    if is_signaled(&system.kernel, object, thread_id) {
        acquire(&mut system.kernel, object, thread_id);
        system.cpu.regs[0] = 0;
        return;
    }
    if timeout as i64 == 0 {
        system.cpu.regs[0] = errors::TIMEOUT.0;
        return;
    }
    let tick = system.cpu.cycles;
    let ticks = (timeout as i64 > 0).then(|| nanos_to_ticks(timeout));
    begin_wait(&mut system.kernel, vec![object], false, ticks, tick);
    // the result registers are written when the wait ends
    system.kernel.thread_mut(thread_id).wait_syscall = Some(WaitSyscall::WaitSynchronization1);
}

/// svcWaitSynchronizationN, r0 = timeout low, r1 = handles, r2 = count,
/// r3 = wait for all, r4 = timeout high.
fn wait_synchronization_n(system: &mut System) {
    catch_up_timers(&mut system.kernel, system.cpu.cycles);
    let (handles, count, wait_all) = (system.cpu.regs[1], system.cpu.regs[2], system.cpu.regs[3] != 0);
    let timeout = (system.cpu.regs[0] as u64) | ((system.cpu.regs[4] as u64) << 32);
    let mut objects = Vec::with_capacity(count as usize);
    for i in 0..count {
        let handle = system.memory.read32(handles + i * 4);
        match system.kernel.resolve(handle) {
            Some(object) => objects.push(object),
            None => {
                log::warn!("svcWaitSynchronizationN: invalid handle 0x{handle:X} at index {i}");
                system.cpu.regs[0] = errors::INVALID_HANDLE.0;
                return;
            }
        }
    }
    let Some(thread_id) = system.kernel.current_thread else {
        system.cpu.regs[0] = 0;
        return;
    };
    let kernel = &system.kernel;
    let signaled = |&object: &ObjectId| is_signaled(kernel, object, thread_id);
    let satisfied = if wait_all { objects.iter().all(signaled).then_some(0) } else { objects.iter().position(signaled) };
    if let Some(index) = satisfied {
        if wait_all {
            for &object in &objects {
                acquire(&mut system.kernel, object, thread_id);
            }
        } else {
            acquire(&mut system.kernel, objects[index], thread_id);
        }
        system.cpu.regs[0] = 0;
        system.cpu.regs[1] = index as u32;
        return;
    }
    if timeout as i64 == 0 {
        system.cpu.regs[0] = errors::TIMEOUT.0;
        return;
    }
    let tick = system.cpu.cycles;
    let ticks = (timeout as i64 > 0).then(|| nanos_to_ticks(timeout));
    begin_wait(&mut system.kernel, objects, wait_all, ticks, tick);
    system.kernel.thread_mut(thread_id).wait_syscall = Some(WaitSyscall::WaitSynchronizationN);
}

/// native-final: the timers gathered again once a frame, without the ones
/// closed since; they fire when the scheduling pass reaches their tick.
pub fn gather_timers(kernel: &mut Kernel) {
    kernel.timers.clear();
    for (id, object) in kernel.objects.iter() {
        if let KObject::Timer(_) = object {
            kernel.timers.push(id);
        }
    }
}

