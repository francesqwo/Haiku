//! updscreen: Haiku.exe's start when the game's module (or Haiku's files
//! made from the game) is out of date for this Haiku.exe, after an update
//! or for a version switched to: the game's window opens as it always does
//! (the player's window mode, size and aspect), shows the build in the
//! game's look (zakuro's fates_ui/pregame.rs) and the game starts in it once
//! the module is built and bound. The build itself is `Haiku.exe --rebuild
//! --progress` (haiku_setup::updscreen), a windowless process of its own.
//! A start with nothing to build never comes here.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use haiku_setup::pipeline::{self, Paths, Ready};
use haiku_setup::updscreen::{Build, Poll};
use zakuro::pregame::{Pregame, Progress, State};

pub struct Work {
    paths: Paths,
    /// what Haiku.exe was started with, where the game's start changes it
    /// (APPDATA, LOCALAPPDATA), for the build's process
    env: Vec<(OsString, Option<OsString>)>,
    why: String,
    build: Option<Build>,
    /// the module being loaded ahead, once built
    preload: Option<std::thread::JoinHandle<Result<(), String>>>,
}

impl Work {
    /// for the version paths are for; why is what made the start build.
    pub fn new(root: &Path, paths: &Paths, why: &str) -> Work {
        let mut env: Vec<(OsString, Option<OsString>)> = ["APPDATA", "LOCALAPPDATA"].iter().map(|name| (OsString::from(name), std::env::var_os(name))).collect();
        env.push(("HAIKU_HOME".into(), Some(root.as_os_str().to_owned())));
        Work { paths: paths.clone(), env, why: why.to_owned(), build: None, preload: None }
    }

    /// the title of the game installed, for the screen's region (its art and
    /// words) before the code is there.
    pub fn title(&self) -> Option<u64> {
        pipeline::installed_title(&self.paths.data())
    }

    fn log(&self) -> PathBuf {
        self.paths.log_file()
    }

    /// an error's paths from the Haiku folder on, as the player knows it
    /// ("Haiku\setup\w64devkit\bin"), not the whole path to it.
    fn shorten(&self, error: &str) -> String {
        let root = self.paths.root.display().to_string();
        let name = self.paths.root.file_name().map_or_else(|| "Haiku".to_owned(), |n| n.to_string_lossy().into_owned());
        if root.len() > name.len() + 3 {
            error.replace(&root, &name)
        } else {
            error.to_owned()
        }
    }
}

impl Pregame for Work {
    fn start(&mut self) -> Result<(), String> {
        self.preload = None;
        let exe = std::env::current_exe().map_err(|e| format!("Haiku can't find its own program ({e})."))?;
        log::info!("updscreen: {}: building the game's code ({} --rebuild --progress)", self.why, exe.display());
        self.build = None;
        self.build = Some(Build::start(&exe, &self.env).map_err(|e| format!("Haiku can't start building the game's code ({e})."))?);
        Ok(())
    }

    fn poll(&mut self) -> State {
        let starting = || State::Working(Progress { percent: 100.0, step: "Starting the game".into(), detail: "Loading the game's code".into() });
        if let Some(preload) = &self.preload {
            if !preload.is_finished() {
                return starting();
            }
            let started = self.preload.take().map(|p| p.join());
            log::info!("updscreen: the game's module loaded ahead ({:?})", started.and_then(|r| r.ok()));
            return State::Done;
        }
        let Some(build) = &mut self.build else { return State::Failed("The build isn't running.".into()) };
        match build.poll() {
            Poll::Working { percent, step, detail } => State::Working(Progress { percent, step, detail }),
            Poll::Done(said) => {
                log::info!("updscreen: {said}");
                // loaded ahead on a thread of its own (finish binds it)
                self.build = None;
                let path = crate::module::find(&self.paths.game);
                self.preload = std::thread::Builder::new().name("updscreen module".into()).spawn(move || crate::module::preload(&path)).ok();
                if self.preload.is_none() {
                    return State::Done;
                }
                starting()
            }
            Poll::Failed(error) => State::Failed(self.shorten(&error)),
        }
    }

    fn finish(&mut self) -> Result<zakuro::Linked, String> {
        self.build = None;
        // what the start checks, again: a build that still leaves something
        // to make says so here, rather than building again in a loop
        match pipeline::ready(&self.paths) {
            Ready::Yes => {}
            Ready::Install => return Err("The game isn't installed any more. Start Haiku again to set it up.".into()),
            Ready::Update(why) => return Err(format!("The game's code was built, but Haiku still finds something out of date ({why}). The log has the details.")),
        }
        let game = &self.paths.game;
        let started = std::time::Instant::now();
        match crate::module::load(&crate::module::find(game), pipeline::installed_title(&game.join("data"))) {
            Ok(bound) => {
                log::info!("updscreen: the game's module loaded and bound in {:.0} ms", started.elapsed().as_secs_f64() * 1e3);
                Ok(crate::wire(bound))
            }
            Err(why) => {
                // built again by Try Again (or the next start)
                let _ = std::fs::remove_file(crate::module::dir(game).join(haiku_setup::module::NOTE));
                Err(format!("The game's code was built, but it can't be loaded: {why}. Try Again builds it again."))
            }
        }
    }

    fn stop(&mut self) {
        if let Some(build) = &mut self.build {
            build.stop();
        }
        self.build = None;
    }

    fn open_log(&self) {
        let log = self.log();
        log::info!("updscreen: opening {}", log.display());
        open(&log);
    }

    fn why(&self) -> String {
        // what made the start build (pipeline::ready, module::check): an
        // update (or a version not played since one), else a module that
        // is missing or damaged
        if self.why.contains("another version of Haiku") || self.why.contains("Haiku's files") || self.why.contains("another Haiku") {
            "Haiku was updated, so the game's code is built again from your installed game, once.".into()
        } else {
            "The game's code has to be built again from your installed game.".into()
        }
    }

    fn failed_without_art(&mut self, error: &str) -> bool {
        let text = format!("Haiku can't build the game's code: {error}\n\n{} has the details.\n\nTry again?", self.log().display());
        ask_retry(&text)
    }
}

/// a file opened as the system opens its kind (a .log in Notepad); its
/// folder when nothing opens it.
fn open(path: &Path) {
    #[cfg(windows)]
    {
        use std::ffi::c_void;
        #[link(name = "shell32")]
        extern "system" {
            fn ShellExecuteW(hwnd: *mut c_void, verb: *const u16, file: *const u16, params: *const u16, dir: *const u16, show: i32) -> isize;
        }
        let wide = |s: &std::ffi::OsStr| {
            use std::os::windows::ffi::OsStrExt;
            s.encode_wide().chain([0]).collect::<Vec<u16>>()
        };
        let (verb, file) = (wide("open".as_ref()), wide(path.as_os_str()));
        // SAFETY: NUL-terminated strings; no owner window
        let done = unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), 1) };
        if done <= 32 {
            log::warn!("updscreen: {} can't be opened ({done}): its folder instead", path.display());
            let folder = path.parent().unwrap_or(path);
            let (file, verb) = (wide(folder.as_os_str()), wide("explore".as_ref()));
            // SAFETY: as above
            unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), 1) };
        }
    }
    #[cfg(not(windows))]
    let _ = path;
}

/// a message box with Retry and Cancel: true for Retry.
fn ask_retry(text: &str) -> bool {
    #[cfg(windows)]
    {
        use std::ffi::c_void;
        #[link(name = "user32")]
        extern "system" {
            fn MessageBoxW(hwnd: *mut c_void, text: *const u16, caption: *const u16, kind: u32) -> i32;
        }
        let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let (text, caption) = (wide(text), wide("Haiku"));
        // SAFETY: NUL-terminated strings; no owner window. MB_RETRYCANCEL |
        // MB_ICONERROR; IDRETRY is 4
        unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), caption.as_ptr(), 0x5 | 0x10) == 4 }
    }
    #[cfg(not(windows))]
    {
        let _ = text;
        false
    }
}
