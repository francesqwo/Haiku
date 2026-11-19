//! The setup's log (oneexe: setup-work/setup.log; 1.x wrote `Haiku
//! Setup.log` beside Haiku Setup.exe): every step, what it found and every
//! tool's failure, so a failed build can be looked into.

use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static LOG: Mutex<Option<std::fs::File>> = Mutex::new(None);
static ECHO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn open(path: &Path, echo: bool) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let file = std::fs::OpenOptions::new().create(true).append(true).open(path).ok();
    *LOG.lock().unwrap() = file;
    ECHO.store(echo, std::sync::atomic::Ordering::Relaxed);
}

fn stamp() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (h, m, s) = (secs / 3600 % 24, secs / 60 % 60, secs % 60);
    format!("{h:02}:{m:02}:{s:02}Z")
}

pub fn line(text: impl AsRef<str>) {
    let text = text.as_ref();
    if ECHO.load(std::sync::atomic::Ordering::Relaxed) {
        println!("{text}");
    }
    if let Some(file) = LOG.lock().unwrap().as_mut() {
        let _ = writeln!(file, "{} {text}", stamp());
        let _ = file.flush();
    }
}
