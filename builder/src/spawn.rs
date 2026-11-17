//! Every program Haiku Setup starts goes through here. Haiku Setup is a
//! GUI program with no console, so a console program it started (gcc, ar,
//! cmd) would get a console window of its own on the desktop: each one is
//! started with CREATE_NO_WINDOW, no input, and its output read into the
//! log. The recompiler's own compiler runs do the same once `quiet` is
//! called (recomp3ds::compile::set_quiet).

use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};

use crate::log;

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// a command for program that opens no window.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command.stdin(Stdio::null());
    command
}

/// runs a command to its end, its output in the log.
pub fn run(mut command: Command, what: &str) -> std::io::Result<ExitStatus> {
    let output = command.stdout(Stdio::piped()).stderr(Stdio::piped()).output()?;
    for line in String::from_utf8_lossy(&output.stdout).lines().chain(String::from_utf8_lossy(&output.stderr).lines()) {
        log::line(format!("  {what}: {line}"));
    }
    Ok(output.status)
}

/// starts a program (Haiku.exe) without waiting for it.
pub fn start(program: &Path, dir: &Path) -> std::io::Result<()> {
    command(program).current_dir(dir).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map(|_| ())
}

/// the recompiler's compiler runs: windowless, their output in the log.
pub fn quiet() {
    recomp3ds::compile::set_quiet(|line| log::line(format!("  gcc: {line}")));
}
