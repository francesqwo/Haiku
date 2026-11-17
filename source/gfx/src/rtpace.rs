//! rtpace: the upper screen's pictures in between drawn on a thread of
//! their own, in real time.
//!
//! In real time the frontend's thread both draws the pictures (one per
//! refresh of the display, 175 a second on the user's monitor) and lends
//! the game's threads their time: one runs at a time (pcos::threads). At
//! 8x with MSAA 8 a picture in between took 5-5.7 ms of that thread
//! (recording its draws, about 3.5 ms, and waiting for the GPU to finish the
//! one before, about 1.7 ms), so presents went back to back and the game
//! got a few tenths of a millisecond per refresh: its 30 Hz tick (about
//! 6 ms of work) then ran late, the refresh it was due at was raised late,
//! and the display showed a picture twice (dev/reports/rtpace.md).
//!
//! Now the frontend prepares the picture (`Gfx::begin_between`, what it
//! needs from the renderer's state), hands it with the in-between
//! renderer to this thread, and lends the game's threads the time until
//! it is drawn (System::lend_while on `BETWEEN_BUSY`). The present then
//! finds it drawn (`reclaim_between`). One picture at a time: the next is
//! prepared only after the present took this one, so everything reaches
//! the frontend's queue in the order it did before (submits are under
//! zakuro_gpu::submit_lock).
//!
//! Real time only, upper screen only. Deterministic runs (replays, the
//! gate, headless dumps) never start one. FATES_RT_DRAW_THREAD=0 turns it
//! off.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::hfr::BetweenJob;
use crate::render::{Output, Renderer};

/// what the drawing thread gets: the job, the in-between renderer (lent to
/// it), the decoded textures, and where to send the result.
struct Work {
    job: BetweenJob,
    renderer: Renderer,
    decoded: HashMap<(crate::TexRef, u64), Arc<crate::Decoded>>,
    done: Sender<Done>,
}

/// what comes back.
pub(crate) struct Done {
    pub job: BetweenJob,
    pub renderer: Renderer,
    pub drawn: Result<Output, String>,
    pub took: Duration,
}

/// jobs handed over and not yet drawn; BETWEEN_BUSY while any.
static OUT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// whether pictures in between may be drawn off the frontend's thread.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    zakuro_core::pcos::rt::realtime() && *ON.get_or_init(|| crate::perf::var("FATES_RT_DRAW_THREAD").map_or(true, |v| v != "0"))
}

thread_local! {
    static DRAWING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// whether the calling thread is the drawing thread (perf scopes log as
/// `gfxw` there).
pub fn on_drawing_thread() -> bool {
    DRAWING.with(|d| d.get())
}

fn sender() -> Option<Sender<Work>> {
    static TX: OnceLock<Option<Mutex<Sender<Work>>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = channel::<Work>();
        std::thread::Builder::new()
            .name("pictures in between".to_owned())
            .spawn(move || {
                DRAWING.with(|d| d.set(true));
                raise_priority();
                while let Ok(work) = rx.recv() {
                    let Work { job, mut renderer, decoded, done } = work;
                    let start = Instant::now();
                    let drawn = job.draw(&mut renderer, &decoded);
                    let took = start.elapsed();
                    zakuro_gpu::smooth::span("between", start, start + took, job.frame_id() as i64, job.at as f64);
                    drop(decoded);
                    let _ = done.send(Done { job, renderer, drawn, took });
                    if OUT.fetch_sub(1, Ordering::AcqRel) == 1 {
                        zakuro_core::native_gfx::BETWEEN_BUSY.store(false, Ordering::Release);
                    }
                }
            })
            .ok()
            .map(|_| Mutex::new(tx))
    })
    .as_ref()
    .map(|tx| tx.lock().unwrap_or_else(|e| e.into_inner()).clone())
}

/// the drawing thread a step above normal, as the frontend's and the
/// game's threads (smooth): a busy normal thread does not hold it up.
fn raise_priority() {
    #[cfg(windows)]
    {
        unsafe extern "system" {
            fn GetCurrentThread() -> isize;
            fn SetThreadPriority(thread: isize, priority: i32) -> i32;
        }
        // SAFETY: plain calls on the calling thread's pseudo-handle
        unsafe { SetThreadPriority(GetCurrentThread(), 1) };
        // and off the game's core: on its other logical processor it took
        // the game's thread's time (FATES_RT_DRAW_ANYWHERE=1 leaves it)
        let game = zakuro_core::pcos::threads::game_core_mask();
        if game != 0 && !crate::perf::var("FATES_RT_DRAW_ANYWHERE").is_ok_and(|v| v == "1") {
            unsafe extern "system" {
                fn GetCurrentProcess() -> isize;
                fn GetProcessAffinityMask(process: isize, process_mask: *mut usize, system_mask: *mut usize) -> i32;
                fn SetThreadAffinityMask(thread: isize, mask: usize) -> usize;
            }
            let (mut process, mut system) = (0usize, 0usize);
            // SAFETY: plain queries and a mask within the process's own
            unsafe {
                if GetProcessAffinityMask(GetCurrentProcess(), &mut process, &mut system) != 0 && process & !game != 0 {
                    SetThreadAffinityMask(GetCurrentThread(), process & !game);
                    log::info!("rtpace: pictures in between are drawn off the game's core (processors 0x{:X})", process & !game);
                }
            }
        }
    }
}

/// hands a job to the drawing thread; the result comes on the receiver.
/// None (and the renderer back) when there is no thread.
pub(crate) fn send(job: BetweenJob, renderer: Renderer, decoded: HashMap<(crate::TexRef, u64), Arc<crate::Decoded>>) -> Result<Receiver<Done>, (BetweenJob, Renderer)> {
    let Some(tx) = sender() else { return Err((job, renderer)) };
    let (done, rx) = channel();
    OUT.fetch_add(1, Ordering::AcqRel);
    zakuro_core::native_gfx::BETWEEN_BUSY.store(true, Ordering::Release);
    match tx.send(Work { job, renderer, decoded, done }) {
        Ok(()) => Ok(rx),
        Err(e) => {
            if OUT.fetch_sub(1, Ordering::AcqRel) == 1 {
                zakuro_core::native_gfx::BETWEEN_BUSY.store(false, Ordering::Release);
            }
            let Work { job, renderer, .. } = e.0;
            Err((job, renderer))
        }
    }
}
