//! smooth: a timing log for measuring how evenly frames reach the display,
//! and what takes long on the way (FATES_SMOOTH_LOG=path).
//!
//! Every crate of the program can note an event; the log is one line per
//! event, tab separated:
//!
//! ```text
//! kind  start_us  dur_us  a  b
//! ```
//!
//! `start_us` counts from the first event (the log's own clock, an
//! `Instant`), `dur_us` is how long it took (0 for a point in time), `a`
//! and `b` are the event's own numbers. The kinds written now:
//!
//! | kind | written by | a | b |
//! |---|---|---|---|
//! | `emu` | frontend, one game refresh emulated | game frame | game clock total (1/60 s units) |
//! | `present` | frontend, a present handed over (whole call) | present id | content time in refreshes |
//! | `acquire` | presenter, fence wait + acquire | present id | 0 |
//! | `queue` | presenter, vkQueuePresentKHR | present id | 0 |
//! | `shown` | present-wait thread, the image reached the display | present id | 0 |
//! | `pipe` | fates-gfx pipeline creation | 1 from the disk cache, 0 compiled | 0 |
//! | `zpipe` | zakuro-gpu pipeline creation | 0 | 0 |
//! | `jit` | shader JIT compile, off the game thread | tier (0 quick, 1 full) | 0 |
//! | `mark` | anything, a named point (the name in `b`'s place) | 0 | 0 |
//!
//! `dev/smooth/analyze.py` turns a log into per-scene numbers and graphs.

use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// lines are kept in memory and written by a thread of their own: a write
/// to the file can stall (a busy disk, a scanner), which must not stall the
/// frame that logged (measured: 2.2 s once, when the frame thread wrote).
struct Recorder {
    start: Instant,
    lines: Mutex<Vec<u8>>,
    out: Mutex<std::fs::File>,
}

impl Recorder {
    /// what is kept, to the file.
    fn write_out(&self) {
        let lines = std::mem::take(&mut *self.lines.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
        if !lines.is_empty() {
            let _ = self.out.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).write_all(&lines);
        }
    }
}

fn recorder() -> Option<&'static Recorder> {
    static RECORDER: OnceLock<Option<Recorder>> = OnceLock::new();
    RECORDER
        .get_or_init(|| {
            let path = std::env::var_os("FATES_SMOOTH_LOG").filter(|path| !path.is_empty())?;
            let mut out = std::fs::File::create(&path).ok()?;
            let _ = writeln!(out, "# kind\tstart_us\tdur_us\ta\tb");
            let _ = std::thread::Builder::new().name("timing log".to_owned()).spawn(|| loop {
                std::thread::sleep(std::time::Duration::from_millis(200));
                if let Some(recorder) = recorder() {
                    recorder.write_out();
                }
            });
            Some(Recorder { start: Instant::now(), lines: Mutex::new(Vec::with_capacity(1 << 20)), out: Mutex::new(out) })
        })
        .as_ref()
}

/// whether events are logged.
pub fn enabled() -> bool {
    recorder().is_some()
}

fn write(line: std::fmt::Arguments<'_>) {
    let Some(recorder) = recorder() else { return };
    let _ = recorder.lines.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).write_fmt(line);
}

fn micros(at: Instant) -> u64 {
    recorder().map_or(0, |recorder| at.saturating_duration_since(recorder.start).as_micros() as u64)
}

/// an event that started at `start` and ended at `end`.
pub fn span(kind: &str, start: Instant, end: Instant, a: i64, b: f64) {
    if !enabled() {
        return;
    }
    let dur = end.saturating_duration_since(start).as_micros();
    write(format_args!("{kind}\t{}\t{dur}\t{a}\t{b}\n", micros(start)));
}

/// an event that started at `start` and ends now.
pub fn since(kind: &str, start: Instant, a: i64, b: f64) {
    if enabled() {
        span(kind, start, Instant::now(), a, b);
    }
}

/// a point in time.
pub fn point(kind: &str, at: Instant, a: i64, b: f64) {
    span(kind, at, at, a, b);
}

/// a named point, for scene changes and the like.
pub fn mark(name: &str) {
    if !enabled() {
        return;
    }
    let name: String = name.chars().map(|c| if c.is_whitespace() { '_' } else { c }).collect();
    write(format_args!("mark\t{}\t0\t0\t{name}\n", micros(Instant::now())));
}

/// writes what is buffered, before the program ends.
pub fn flush() {
    if let Some(recorder) = recorder() {
        recorder.write_out();
    }
}
