//! hitches (issue #14): what opening the options menu, or the Mods page
//! from the game's Main Menu, used to do on the game's thread, done on
//! threads of their own, and a line in the log for any picture that took
//! long to show.
//!
//! Each time the menu opened, the frontend read again, before drawing the
//! next picture:
//!
//! - the displays and every display mode of the chosen one, and the sound
//!   devices (cpal), for the Display and Audio tabs' lists: 75 ms here, more
//!   where a sound driver is slow to answer;
//! - the whole mods folder (ModList::scan): every file of every mod listed
//!   for its notes, each laid-out mod's files looked at for its stamp, the
//!   zips' directories read: 250 ms here with 19,000 files (8 GB) in the
//!   page cache, 1.3 s for the same folder the first time, and many seconds
//!   on a hard disk or with a virus scanner looking at each file;
//! - and once, the compiler that came with Haiku run (`gcc --version`) to
//!   know whether the Mods page offers it.
//!
//! The game stood still meanwhile (the sound ran dry). Now the menu opens at
//! once with what was read last time; the lists are read on a thread of
//! their own and put in when they are ready, a frame or two later. A scan
//! that finishes after the player changed the list (any save of mods.toml
//! counts, see ModList::saves) is thrown away and made again from the list
//! as saved, so a change made meanwhile is never undone.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::haiku_mods::ModList;
use crate::App;

/// a picture that takes this long to show is logged, with where the time
/// went.
const SLOW: Duration = Duration::from_millis(50);

/// the displays' and sound devices' lists, read on a thread.
struct EnvLists {
    monitors: Vec<String>,
    modes: Vec<(u32, u32, u32)>,
    audio_devices: Vec<String>,
    took: Duration,
}

/// a mods folder scanned on a thread: the list, the saves counted when it
/// started, and the frontend's notes it started from.
struct Scanned {
    list: ModList,
    since: u64,
    took: Duration,
}

#[derive(Default)]
struct Jobs {
    env: Option<Receiver<EnvLists>>,
    /// asked again while one was running: another once it is done.
    env_again: bool,
    scan: Option<Receiver<Scanned>>,
    scan_again: bool,
    /// the last slow picture logged, so a run of them is one line a second.
    warned: Option<Instant>,
    slow_since_warned: u32,
}

static JOBS: Mutex<Option<Jobs>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Jobs) -> R) -> R {
    let mut guard = JOBS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    f(guard.get_or_insert_with(Jobs::default))
}

/// after a picture went to the presenter: t0 when the frame's work began,
/// ui when the interface was done, presenter when the presenter was called.
pub fn frame_shown(t0: Instant, ui: Instant, presenter: Instant) {
    let now = Instant::now();
    let total = now - t0;
    if total < SLOW {
        return;
    }
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    with(|jobs| {
        jobs.slow_since_warned += 1;
        if jobs.warned.is_some_and(|at| now - at < Duration::from_secs(1)) {
            return;
        }
        let more = jobs.slow_since_warned - 1;
        jobs.slow_since_warned = 0;
        jobs.warned = Some(now);
        let others = if more > 0 { format!(" ({more} more since the last such line)") } else { String::new() };
        log::warn!(
            "hitches: a picture took {:.0} ms to show: the interface {:.0} ms, the screens and layout {:.0} ms, the presenter {:.0} ms{others}",
            ms(total),
            ms(ui - t0),
            ms(presenter - ui),
            ms(now - presenter)
        );
    });
}

/// the time a piece of work on the game's thread took, logged when it is
/// more than a frame's worth.
pub fn took(what: &str, since: Instant) {
    let elapsed = since.elapsed();
    if elapsed >= Duration::from_millis(4) {
        log::info!("hitches: {what} took {:.1} ms on the game's thread", elapsed.as_secs_f64() * 1e3);
    }
}

/// the displays' names and the chosen one's modes, and the sound devices,
/// read on a thread: what fates_refresh_env read on the game's thread.
pub fn refresh_env(monitors: Vec<winit::monitor::MonitorHandle>, chosen: Option<winit::monitor::MonitorHandle>) {
    with(|jobs| {
        if jobs.env.is_some() {
            jobs.env_again = true;
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("ui: displays and sound devices".to_owned()).spawn(move || {
            let started = Instant::now();
            let names = monitors
                .iter()
                .enumerate()
                .map(|(index, monitor)| {
                    let size = monitor.size();
                    format!("Display {} ({}\u{d7}{})", index + 1, size.width, size.height)
                })
                .collect();
            let mut modes: Vec<(u32, u32, u32)> = chosen
                .map(|monitor| monitor.video_modes().map(|mode| (mode.size().width, mode.size().height, (mode.refresh_rate_millihertz() + 500) / 1000)).collect())
                .unwrap_or_default();
            modes.sort();
            modes.dedup();
            let audio_devices = crate::audio::output_devices();
            let _ = tx.send(EnvLists { monitors: names, modes, audio_devices, took: started.elapsed() });
        });
        if spawned.is_ok() {
            jobs.env = Some(rx);
        }
    });
}

/// the mods folder read again on a thread, keeping what the game started
/// with: what fates_mods_rescan read on the game's thread.
pub fn rescan_mods(current: &ModList) {
    let list = current.for_rescan();
    with(|jobs| {
        if jobs.scan.is_some() {
            jobs.scan_again = true;
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("mods: reading the folder".to_owned()).spawn(move || {
            let started = Instant::now();
            let since = ModList::saves();
            let mut list = list;
            list.scan_unsaved();
            // the new mods found, as off, unless the player changed the
            // list meanwhile (then this scan is not used, see poll)
            list.save_unless_changed(since);
            let _ = tx.send(Scanned { list, since, took: started.elapsed() });
        });
        if spawned.is_ok() {
            jobs.scan = Some(rx);
        }
    });
}

impl App {
    /// each frame, on the game's thread: what the threads read, put in.
    pub(crate) fn hitches_poll(&mut self) {
        // the displays and sound devices
        let env = with(|jobs| {
            let rx = jobs.env.as_ref()?;
            match rx.try_recv() {
                Ok(lists) => {
                    jobs.env = None;
                    Some(Some(lists))
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    jobs.env = None;
                    Some(None)
                }
            }
        });
        let env_done = env.is_some();
        if let Some(lists) = env.flatten() {
            let started = Instant::now();
            log::info!(
                "ui: monitors {:?}, {} display modes, sound devices {:?} (read in {:.0} ms on a thread of their own)",
                lists.monitors,
                lists.modes.len(),
                lists.audio_devices,
                lists.took.as_secs_f64() * 1e3
            );
            self.fates.env.monitors = lists.monitors;
            self.fates.env.modes = lists.modes;
            self.fates.env.audio_devices = lists.audio_devices;
            took("putting in the displays and sound devices", started);
        }
        if env_done && with(|jobs| std::mem::take(&mut jobs.env_again)) {
            self.fates_refresh_env();
        }

        // the mods folder
        let scanned = with(|jobs| {
            let rx = jobs.scan.as_ref()?;
            match rx.try_recv() {
                Ok(scanned) => {
                    jobs.scan = None;
                    Some(Some(scanned))
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    jobs.scan = None;
                    Some(None)
                }
            }
        });
        if let Some(scanned) = scanned {
            let again = with(|jobs| std::mem::take(&mut jobs.scan_again));
            match scanned {
                // the list was saved meanwhile (a mod turned on or off,
                // moved, an option): read again from it as saved
                Some(scanned) if ModList::saves() != scanned.since => {
                    log::info!("mods: the list changed while the folder was read; reading it again");
                    self.fates_mods_rescan();
                }
                Some(Scanned { mut list, took: spent, .. }) => {
                    let started = Instant::now();
                    list.booted = self.fates.mods.booted.take();
                    // notes the frontend added meanwhile (code mods' compiles)
                    if list.code_notes != self.fates.mods.code_notes || list.left_out != self.fates.mods.left_out {
                        list.code_notes = self.fates.mods.code_notes.clone();
                        list.left_out = self.fates.mods.left_out.clone();
                        list.check();
                    }
                    self.fates.mods = list;
                    self.fates_code_view();
                    log::info!("mods: the folder read on a thread of its own in {:.0} ms", spent.as_secs_f64() * 1e3);
                    took("putting in the mods read", started);
                    if again {
                        self.fates_mods_rescan();
                    }
                }
                None => {
                    if again {
                        self.fates_mods_rescan();
                    }
                }
            }
        }

        // codemods: the compiler that came with Haiku, now known
        if crate::haiku_codemods::take_setup_compiler_found() {
            self.fates_code_view();
        }
    }
}
