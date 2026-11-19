//! updscreen: the game's module built again (after an update, or for a
//! version switched to) while the game's own window shows it, in the game's
//! look (zakuro's fates_ui/pregame.rs). The build runs in a process of its
//! own, `Haiku.exe --rebuild --progress` (windowless, as every program
//! Haiku starts), so nothing it changes in its environment (the compiler's
//! PATH, TMP, APPDATA while the recompiler runs) and none of the memory it
//! takes stays with the game, which starts in the window's process after.
//!
//! The child says how far it is on its standard output, a line at a time:
//!   `@progress <percent> <step> <detail>` (tab-separated)
//!   `@ok <what was done>`, or `@failed <the error, for the player>`
//! and stops (where it can carry on from next time) when its standard input
//! closes: the window went away.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use crate::{log, pipeline};

/// a line's text on one line, with no tabs.
fn flat(text: &str) -> String {
    text.replace(['\t', '\r'], " ").replace('\n', "\\n")
}

/// `Haiku.exe --rebuild --progress`: pipeline::update, as `--rebuild` does,
/// its progress for the window that started it.
pub fn child() -> i32 {
    let paths = pipeline::Paths::new(crate::root());
    log::open(&paths.log_file(), false);
    log::line(format!("updscreen: building in {} for the game's window", paths.game.display()));
    let status: pipeline::Shared = Arc::new(Mutex::new(pipeline::Status::default()));
    let cancel = Arc::new(AtomicBool::new(false));
    {
        // the window's end of the pipe closing (it quit, or was closed)
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            let mut sink = [0u8; 256];
            let mut stdin = std::io::stdin();
            while matches!(stdin.read(&mut sink), Ok(n) if n > 0) {}
            log::line("updscreen: the window is gone: stopping");
            cancel.store(true, Ordering::Relaxed);
        });
    }
    let out = std::io::stdout();
    let say = |line: String| {
        let mut out = out.lock();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    };
    let result = std::thread::scope(|scope| {
        let job = scope.spawn(|| pipeline::update(&paths, &status, &cancel));
        let mut last = String::new();
        while !job.is_finished() {
            let s = status.lock().unwrap().clone();
            let line = format!("@progress\t{:.1}\t{}\t{}", s.percent, flat(&s.step), flat(&s.detail));
            if line != last {
                say(line.clone());
                last = line;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        job.join().unwrap_or_else(|_| Err("Building the game's code stopped with an internal error. The log has the details.".into()))
    });
    match result {
        Ok(said) => {
            log::line(format!("ok: {said}"));
            say(format!("@ok\t{}", flat(&said)));
            0
        }
        Err(error) => {
            log::line(format!("failed: {error}"));
            say(format!("@failed\t{}", flat(&error)));
            1
        }
    }
}

/// what the build is doing, as the window shows it.
#[derive(Clone, Debug, PartialEq)]
pub enum Poll {
    Working { percent: f32, step: String, detail: String },
    Done(String),
    Failed(String),
}

/// the build, from the window's side.
pub struct Build {
    child: Child,
    /// held open while the build should go on
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    last: Poll,
    end: Option<Poll>,
}

impl Build {
    /// starts `exe --rebuild --progress` with no window, its environment
    /// this process's with `env` over it (a value of None removed).
    pub fn start(exe: &Path, env: &[(OsString, Option<OsString>)]) -> std::io::Result<Build> {
        let mut command = crate::spawn::command(exe);
        command.args(["--rebuild", "--progress"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        // what a restart or the setup hands Haiku.exe isn't for this one
        for name in ["HAIKU_RESTART_WAIT", "HAIKU_SETUP_DONE"] {
            command.env_remove(name);
        }
        for (name, value) in env {
            match value {
                Some(value) => command.env(name, value),
                None => command.env_remove(name),
            };
        }
        if let Some(dir) = exe.parent() {
            command.current_dir(dir);
        }
        let mut child = command.spawn()?;
        let stdout = child.stdout.take().expect("piped");
        let stdin = child.stdin.take();
        let (tx, lines) = std::sync::mpsc::channel();
        std::thread::Builder::new().name("updscreen".into()).spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        })?;
        Ok(Build { child, stdin, lines, last: Poll::Working { percent: 0.0, step: String::new(), detail: String::new() }, end: None })
    }

    /// the build's news since the last call.
    pub fn poll(&mut self) -> Poll {
        if let Some(end) = &self.end {
            return end.clone();
        }
        loop {
            match self.lines.try_recv() {
                Ok(line) => {
                    let mut parts = line.split('\t');
                    let kind = parts.next().unwrap_or("");
                    let mut text = || parts.next().unwrap_or("").replace("\\n", "\n");
                    match kind {
                        "@progress" => {
                            let percent = text().parse().unwrap_or(0.0);
                            let (step, detail) = (text(), text());
                            self.last = Poll::Working { percent, step, detail };
                        }
                        "@ok" => self.end = Some(Poll::Done(text())),
                        "@failed" => self.end = Some(Poll::Failed(text())),
                        _ => {}
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    // the build's process ended without saying how
                    // (every line it wrote was taken before this)
                    if self.end.is_none() {
                        let code = self.child.wait().ok().and_then(|status| status.code());
                        self.end = Some(Poll::Failed(format!(
                            "Building the game's code stopped unexpectedly{}. The log has the details.",
                            code.map_or(String::new(), |code| format!(" (exit code {code})"))
                        )));
                    }
                    break;
                }
            }
        }
        if let Some(end) = &self.end {
            // its process is gone, or going: nothing is left running
            let _ = self.child.try_wait();
            return end.clone();
        }
        self.last.clone()
    }

    /// stops the build: its process stops its compilers and ends by itself
    /// (it carries on from there next time).
    pub fn stop(&mut self) {
        self.stdin = None;
    }
}

impl Drop for Build {
    fn drop(&mut self) {
        self.stop();
    }
}
