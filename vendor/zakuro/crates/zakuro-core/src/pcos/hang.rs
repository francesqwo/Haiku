//! savehang: a dump of every guest thread's state, for hangs.
//!
//! The game can stall without an error: a thread the rest waits for is
//! blocked or never scheduled, while the main thread keeps drawing. The save
//! list waiting forever for its slots' headers is one such case (the header
//! reads run on the Backup controller thread, see dev/reports/savehang.md).
//! This module makes that visible in the log:
//!
//! - `dump` writes, for every guest thread, its state, priority, pc/lr/sp,
//!   what it waits on (objects with their state, the arbiter address and the
//!   word there, the timeout), and when it last ran; then the scheduler's
//!   and the real-time clock's state, the arbiters' waiters, the Backup
//!   controller's queue, and the last file-system errors.
//! - It is triggered by F12 (the frontend calls `request`), by
//!   `FATES_DUMP_THREADS=<seconds>` (every so many seconds), and on its own
//!   by `watch`: when the Backup controller's queue holds the same job at
//!   its head for `STUCK_FRAMES` refreshes (5 s), once per stuck job.
//!
//! Everything is cheap when nothing triggers it: one atomic per thread
//! switch, a few guest memory reads per refresh.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::Mutex;

use zakuro_cpu::Bus;

use crate::kernel::object::{KObject, ObjectId};
use crate::kernel::thread::{ThreadStatus, CPU_CLOCK_HZ};
use crate::System;

/// a dump asked for (F12), done at the next refresh.
static REQUESTED: AtomicBool = AtomicBool::new(false);

/// asks for a dump at the next refresh (the frontend's hotkey).
pub fn request() {
    REQUESTED.store(true, Relaxed);
}

/// the refresh each thread slot was last given the core at.
const SLOTS: usize = 64;
static LAST_RAN: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];
/// the frame count, for note_ran (which has no system at hand).
static FRAME: AtomicU64 = AtomicU64::new(0);

/// a thread was given the core (pcos::sched's scheduling pass).
#[inline]
pub(crate) fn note_ran(slot: u32) {
    if let Some(at) = LAST_RAN.get(slot as usize) {
        at.store(FRAME.load(Relaxed), Relaxed);
    }
}

/// save files opened (host archives), for FATES_SAVEHANG_STATS.
static SAVE_OPENS: AtomicU64 = AtomicU64::new(0);

/// a save or extra-data file was opened (pcos::fs).
pub(crate) fn note_save_open() {
    SAVE_OPENS.fetch_add(1, Relaxed);
}

/// the last file-system errors, newest last.
static FS_ERRORS: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
static FS_WARNED: AtomicU64 = AtomicU64::new(0);

/// a native file-system call failed: kept for the dump; failures other than
/// "not found" (which empty save slots give every time) are logged, the
/// first few.
pub(crate) fn note_fs_error(what: &str, detail: &str, code: u32) {
    let line = format!("{what} {detail} -> 0x{code:08X}");
    // FS module 17, descriptions 100-179: not found / not there
    let description = code & 0x3FF;
    let not_found = (code >> 10) & 0xFF == 17 && (100..180).contains(&description);
    if !not_found && FS_WARNED.fetch_add(1, Relaxed) < 20 {
        log::warn!("savehang: a file-system call failed: {line}");
    }
    let mut errors = FS_ERRORS.lock().unwrap();
    if errors.len() == 16 {
        errors.pop_front();
    }
    errors.push_back(format!("[frame {}] {line}", FRAME.load(Relaxed)));
}

/// where the game keeps the Backup controller (Backup::Initialize stores it
/// at this word's +4); its queue: +0x90 head, +0x94 tail, +0x98 count, +0xC0
/// the mutex handle, +0xC4 the LightEvent its thread idles on.
const BACKUP_GLOBAL: u32 = 0x006B_D790;

/// a job at the head of the queue this many refreshes (5 s) is stuck.
const STUCK_FRAMES: u64 = 300;

struct Watch {
    /// queues that started (the count went from 0 up) and jobs that left
    /// the head, for FATES_SAVEHANG_STATS
    batches: u64,
    jobs: u64,
    head: u32,
    since: u64,
    dumped: bool,
    periodic: Option<u64>,
    last_periodic: u64,
    /// the save list: since when one of its slots has waited for its
    /// header, and whether that was dumped
    slots_since: Option<u64>,
    slots_dumped: bool,
}

static WATCH: Mutex<Watch> = Mutex::new(Watch { batches: 0, jobs: 0, head: 0, since: 0, dumped: false, periodic: None, last_periodic: 0, slots_since: None, slots_dumped: false });

fn read32(system: &mut System, address: u32) -> Option<u32> {
    (address != 0 && system.memory.is_readable(address, 4)).then(|| system.memory.read32(address))
}

/// the Backup controller and its queue: (controller, head job, job count).
fn backup_queue(system: &mut System) -> Option<(u32, u32, u32)> {
    let controller = read32(system, crate::region::us(BACKUP_GLOBAL) + 4)?;
    let head = read32(system, controller + 0x90)?;
    let count = read32(system, controller + 0x98)?;
    Some((controller, head, count))
}

/// the save list on screen (GameFileMenu, the current one at *0x006D820C +4,
/// GameFileMenu::GameFileMenu), for the watchdog and the dump.
const FILE_MENU_GLOBAL: u32 = 0x006D_820C;

struct SaveList {
    menu: u32,
    /// its GameBackupHeaderManager's Data array (+0x144) and count (+0x148);
    /// a Data is 0x15C bytes, its state byte at +0x158: 0/1 loading, 2 read,
    /// 3/4 failed (GameBackupHeaderManager::Data::Callback)
    data: u32,
    count: u32,
    /// its items (BasicMenu +0x34 / +0x38): (item, the Data it took (+0x38),
    /// header shown (+0x3C), slot (+0x40), that Data's state)
    items: Vec<(u32, u32, u8, u32, Option<u8>)>,
}

fn save_list(system: &mut System) -> Option<SaveList> {
    let menu = read32(system, crate::region::us(FILE_MENU_GLOBAL) + 4).filter(|&menu| menu != 0)?;
    let data = read32(system, menu + 0x144).unwrap_or(0);
    let count = read32(system, menu + 0x148).unwrap_or(0);
    let array = read32(system, menu + 0x34).unwrap_or(0);
    let n = read32(system, menu + 0x38).unwrap_or(0).min(16);
    let mut items = Vec::new();
    for i in 0..n {
        let Some(item) = read32(system, array + 4 * i) else { continue };
        let (Some(taken), Some(slot)) = (read32(system, item + 0x38), read32(system, item + 0x40)) else { continue };
        let shown = system.memory.is_readable(item + 0x3C, 1).then(|| system.memory.read8(item + 0x3C)).unwrap_or(0);
        let at = if taken != 0 { taken } else if slot < count { data + slot * 0x15C } else { 0 };
        let state = (at != 0 && system.memory.is_readable(at + 0x158, 1)).then(|| system.memory.read8(at + 0x158));
        items.push((item, taken, shown, slot, state));
    }
    Some(SaveList { menu, data, count, items })
}

/// once a refresh (pcos::frame::end_frame): the watchdog, F12 and
/// FATES_DUMP_THREADS.
pub(crate) fn watch(system: &mut System) {
    let frame = system.frames;
    FRAME.store(frame, Relaxed);
    if REQUESTED.swap(false, Relaxed) {
        dump(system, "asked for (F12)");
    }
    let mut watch = WATCH.lock().unwrap();
    if watch.periodic.is_none() {
        let seconds = std::env::var("FATES_DUMP_THREADS").ok().and_then(|v| v.parse::<f64>().ok()).filter(|&v| v > 0.0);
        watch.periodic = Some(seconds.map_or(0, |s| (s * 59.83) as u64));
    }
    if let Some(every) = watch.periodic.filter(|&every| every > 0) {
        if frame >= watch.last_periodic + every {
            watch.last_periodic = frame;
            drop(watch);
            dump(system, "FATES_DUMP_THREADS");
            watch = WATCH.lock().unwrap();
        }
    }
    if stats_wanted() && frame % 600 == 0 {
        log::info!(
            "savehang: save-data queue: {} batches, {} jobs seen, {} save files opened so far; aging turns {}",
            watch.batches,
            watch.jobs,
            SAVE_OPENS.load(Relaxed),
            super::sched::AGING.lock().unwrap().turns
        );
    }
    // the save list: a slot still loading its header after STUCK_FRAMES
    let waiting = save_list(system).is_some_and(|list| list.items.iter().any(|&(_, _, shown, slot, state)| shown == 0 && slot < list.count && matches!(state, Some(0 | 1) | None)));
    match (waiting, watch.slots_since) {
        (false, _) => (watch.slots_since, watch.slots_dumped) = (None, false),
        (true, None) => watch.slots_since = Some(frame),
        (true, Some(since)) if !watch.slots_dumped && frame.saturating_sub(since) >= STUCK_FRAMES => {
            watch.slots_dumped = true;
            drop(watch);
            log::warn!("savehang: the save list has waited {STUCK_FRAMES} refreshes for a slot's header: it looks stuck");
            dump(system, "the save list is still loading");
            watch = WATCH.lock().unwrap();
        }
        _ => {}
    }
    let Some((_, head, count)) = backup_queue(system) else { return };
    if count == 0 || head == 0 {
        if watch.head != 0 {
            watch.jobs += 1;
        }
        watch.head = 0;
        watch.dumped = false;
        return;
    }
    if head != watch.head {
        if watch.head == 0 {
            watch.batches += 1;
        } else {
            watch.jobs += 1;
        }
        watch.head = head;
        watch.since = frame;
        watch.dumped = false;
        return;
    }
    if !watch.dumped && frame.saturating_sub(watch.since) >= STUCK_FRAMES {
        watch.dumped = true;
        drop(watch);
        log::warn!("savehang: the game's save-data thread has had the same job for {STUCK_FRAMES} refreshes ({count} queued): it looks stuck");
        dump(system, "the save-data queue is not moving");
    }
}

/// FATES_SAVEHANG_STATS=1: the queue's counts every 600 refreshes.
fn stats_wanted() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("FATES_SAVEHANG_STATS").is_ok_and(|v| v != "0"))
}

fn ms(ticks: u64) -> f64 {
    ticks as f64 * 1000.0 / CPU_CLOCK_HZ as f64
}

fn describe(system: &mut System, object: ObjectId) -> String {
    let now = system.cpu.cycles;
    let kernel = &system.kernel;
    let name_of = |id: u32| kernel.threads.get(id as usize).map_or(format!("#{id}"), |t| t.name.clone());
    match kernel.objects.get(object) {
        Some(KObject::Event(event)) => format!("Event#{} {:?} signalled {}", object.0, event.reset_type, event.signaled),
        Some(KObject::Mutex(mutex)) => {
            format!("Mutex#{} owner {} count {}", object.0, mutex.owner.map_or("none".to_owned(), name_of), mutex.lock_count)
        }
        Some(KObject::Semaphore(s)) => format!("Semaphore#{} {}/{}", object.0, s.count, s.max_count),
        Some(KObject::Timer(t)) => format!(
            "Timer#{} {:?} signalled {} fires {} interval {:.2} ms",
            object.0,
            t.reset_type,
            t.signaled,
            t.fire_at.map_or("never".to_owned(), |at| if at >= now { format!("in {:.2} ms", ms(at - now)) } else { format!("{:.2} ms ago", ms(now - at)) }),
            ms(t.interval)
        ),
        Some(KObject::Thread(id)) => format!("Thread {} ({:?})", name_of(*id), kernel.threads.get(*id as usize).map(|t| t.status)),
        Some(other) => format!("{}#{}", other.type_name(), object.0),
        None => format!("#{} (gone)", object.0),
    }
}

/// writes the dump to the log, as warnings starting "savehang:".
pub fn dump(system: &mut System, why: &str) {
    let mut lines = Vec::new();
    let now = system.cpu.cycles;
    let frame = system.frames;
    lines.push(format!(
        "thread dump ({why}) at frame {frame}, guest tick {now}, mode {:?}, {}",
        super::rt::mode(),
        system.host_threads.summary()
    ));
    lines.push(format!(
        "  scheduler: current {:?}, reschedule pending {}, all blocked {}, pulsed {:?}, next event {}, next refresh in {:.2} ms, aging turns {}; {}; {}",
        system.kernel.current().map(|t| t.name.clone()),
        system.kernel.reschedule_pending,
        system.kernel.all_blocked,
        system.kernel.pulsed.iter().map(|o| o.0).collect::<Vec<_>>(),
        super::sched::next_event(&system.kernel).map_or("none".to_owned(), |at| format!("in {:.2} ms", ms(at.saturating_sub(now)))),
        ms(system.next_frame_boundary.saturating_sub(now)),
        super::sched::AGING.lock().unwrap().turns,
        system.host_threads.baton(),
        super::rt::describe(system)
    ));
    let current = system.kernel.current_thread;
    for index in 0..system.kernel.threads.len() {
        let thread = &system.kernel.threads[index];
        if thread.status == ThreadStatus::Dead {
            continue;
        }
        let regs = if current == Some(index as u32) { system.cpu.regs } else { thread.context.regs };
        let last = LAST_RAN.get(index).map(|at| at.load(Relaxed));
        let mut line = format!(
            "  {} (slot {index}, id {}) prio {} {:?} pc 0x{:08X} lr 0x{:08X} sp 0x{:08X} last ran {}",
            thread.name,
            thread.guest_id,
            thread.priority,
            thread.status,
            regs[15],
            regs[14],
            regs[13],
            match last {
                Some(_) if current == Some(index as u32) => "now".to_owned(),
                Some(at) => format!("{} refreshes ago", frame.saturating_sub(at)),
                None => "?".to_owned(),
            }
        );
        let (objects, all, address, wakeup) = (thread.wait_objects.clone(), thread.wait_all, thread.wait_address, thread.wakeup_at);
        let status = thread.status;
        if status == ThreadStatus::WaitSync {
            let described: Vec<String> = objects.iter().map(|&o| describe(system, o)).collect();
            line += &format!("; waits on {} of [{}]", if all { "all" } else { "any" }, described.join(", "));
        }
        if status == ThreadStatus::WaitArbiter {
            let word = address.and_then(|a| read32(system, a));
            line += &format!("; waits on the arbiter at 0x{:08X} (word {:?})", address.unwrap_or(0), word.map(|w| w as i32));
        }
        if status.is_blocked() {
            line += &match wakeup {
                Some(at) if at >= now => format!(", times out in {:.2} ms", ms(at - now)),
                Some(at) => format!(", timed out {:.2} ms ago", ms(now - at)),
                None => ", no timeout".to_owned(),
            };
        }
        // boot3: the words on its stack that look like return addresses
        // into the game's code, nearest first: who called the wait
        let sp = regs[13];
        let mut calls = Vec::new();
        for at in (sp..sp.saturating_add(0x200)).step_by(4) {
            match read32(system, at) {
                Some(word) if (0x0010_0000..0x0060_0000).contains(&word) => calls.push(format!("{word:08X}")),
                Some(_) => {}
                None => break,
            }
            if calls.len() >= 12 {
                break;
            }
        }
        if !calls.is_empty() {
            line += &format!("; stack [{}]", calls.join(" "));
        }
        lines.push(line);
    }
    let arbiters: Vec<String> = system
        .kernel
        .objects
        .iter()
        .filter_map(|(id, object)| match object {
            KObject::AddressArbiter(arbiter) if !arbiter.waiters.is_empty() => Some(format!(
                "#{}: {}",
                id.0,
                arbiter.waiters.iter().map(|(t, a)| format!("slot {t} at 0x{a:08X}")).collect::<Vec<_>>().join(", ")
            )),
            _ => None,
        })
        .collect();
    lines.push(format!("  arbiter waiters: {}", if arbiters.is_empty() { "none".to_owned() } else { arbiters.join("; ") }));
    // boot3: the GPU interrupt queue as the GSP writes it, and as the game's
    // receiver (nn::gxlow InterruptReceiver at 0x006BAC40) reads it
    {
        let queue = super::gsp::interrupt_queue_address(system);
        let header = queue.and_then(|q| read32(system, q));
        let receiver_queue = read32(system, crate::region::us(0x006B_AC40) + 0x34);
        let flags = read32(system, crate::region::us(0x006B_AC40) + 0x74);
        let event = system.services.gsp.interrupt_event_object.map(|o| describe(system, o));
        lines.push(format!(
            "  gpu interrupts: shared memory 0x{:08X}, client {}, queue {queue:08X?} header {header:08X?}; the receiver's queue {receiver_queue:08X?}, its flags (+0x74) {flags:08X?}; event {event:?}; ring held {}",
            system.services.gsp.shared_memory_address,
            system.services.gsp.thread_index,
            system.services.gsp.held
        ));
    }
    if let Some((controller, head, count)) = backup_queue(system) {
        let event = read32(system, controller + 0xC4).map(|w| w as i32);
        let mutex = read32(system, controller + 0xC0).unwrap_or(0);
        let mut line = format!("  save-data queue (Backup controller 0x{controller:08X}): {count} jobs, head 0x{head:08X}, its LightEvent {event:?}, mutex handle 0x{mutex:X}");
        if head != 0 && system.memory.is_readable(head, 0xA8) {
            let kind = system.memory.read8(head + 0xC);
            let mut path = Vec::new();
            for i in 0..0x40 {
                let unit = system.memory.read16(head + 0xE + 2 * i);
                if unit == 0 {
                    break;
                }
                path.push(unit);
            }
            let callback = system.memory.read32(head + 0xA0);
            line += &format!(
                "; head job {} '{}' callback 0x{callback:08X}",
                ["format", "read", "write", "delete", "commit"].get(kind as usize).copied().unwrap_or("?"),
                String::from_utf16_lossy(&path)
            );
        }
        lines.push(line);
    }
    if let Some(list) = save_list(system) {
        let items: Vec<String> = list
            .items
            .iter()
            .map(|(item, taken, shown, slot, state)| format!("item 0x{item:08X} slot {slot} data 0x{taken:08X} shown {shown} state {state:?}"))
            .collect();
        lines.push(format!(
            "  save list 0x{:08X}: {} headers at 0x{:08X}; {}",
            list.menu,
            list.count,
            list.data,
            if items.is_empty() { "no items".to_owned() } else { items.join("; ") }
        ));
    }
    lines.push(format!("  the null page is {}", if system.memory.is_readable(0x144, 4) { "mapped (a null pointer reads zeros)" } else { "unmapped" }));
    let errors = FS_ERRORS.lock().unwrap();
    lines.push(format!("  last file-system errors: {}", if errors.is_empty() { "none".to_owned() } else { errors.iter().cloned().collect::<Vec<_>>().join("; ") }));
    drop(errors);
    for line in lines {
        log::warn!("savehang: {line}");
    }
}
