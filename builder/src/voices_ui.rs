//! jpvoice: Haiku Setup's Japanese voices screen, and the Voices choice on
//! the Play screen. A child of ui.rs's window module (it draws with the
//! same helpers); ui.rs only dispatches to it, at the places marked
//! `jpvoice`. The work is voices.rs's.
//!
//! jpdlc: the screen has a second, optional pick, the Japanese DLC (its .cia
//! or an sdmc folder with it), added with the game's voices or later. A
//! file dropped or given to the `voices` script word goes where it fits (a
//! DLC to the DLC pick); a folder fills both picks when it holds both.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::*;
use crate::voices::{self, Installed, InstalledDlc, Progress, VoiceDlc, VoiceDump};

/// jpdlc: what a job installed: the game's voices, the DLC's, or both.
#[derive(Clone, Debug, Default)]
pub struct VoicesDone {
    pub base: Option<Installed>,
    pub dlc: Option<InstalledDlc>,
    /// the DLC's voices failed after the game's were installed.
    pub dlc_error: Option<String>,
}

#[derive(Default)]
pub struct VoicesState {
    /// the dump picked and what it is
    pub picked: Option<Result<VoiceDump, crate::dump::Problem>>,
    pub picked_name: String,
    /// jpdlc: the DLC picked and what it is
    pub dlc_picked: Option<Result<VoiceDlc, crate::dump::Problem>>,
    pub dlc_picked_name: String,
    pub progress: voices::Shared,
    pub cancel: Arc<AtomicBool>,
    pub job: Option<std::thread::JoinHandle<Result<VoicesDone, String>>>,
    /// what the last extraction said
    pub done: Option<Result<VoicesDone, String>>,
}

impl VoicesState {
    pub fn new() -> VoicesState {
        VoicesState { progress: Arc::new(Mutex::new(Progress::default())), ..VoicesState::default() }
    }
}

impl App {
    pub(super) fn voices_installed(&self) -> Option<Installed> {
        voices::installed(&self.paths.data())
    }

    /// jpdlc: the DLC's voices installed.
    fn voices_dlc_installed(&self) -> Option<InstalledDlc> {
        voices::installed_dlc(&self.paths.data())
    }

    /// jpn: whether the installed game is the Japanese one (its install.txt)
    pub(super) fn game_is_japanese(&self) -> bool {
        crate::jpn::installed_region(&self.paths.data()) == Some(crate::jpn::GameRegion::Jpn)
    }

    pub(super) fn voices_japanese(&self) -> bool {
        self.voices_installed().is_some() && voices::japanese_set(&self.paths.root)
    }

    /// a file or folder for the voices: a DLC goes to the DLC pick, a game
    /// to the game's, a folder to both when it holds both.
    pub(super) fn set_voices_dump(&mut self, path: &Path) {
        log::line(format!("voices: picked: {}", path.display()));
        let base = voices::check(path);
        let dlc = voices::check_dlc(path);
        match (&base, &dlc) {
            (Ok(_), Ok(_)) if path.is_dir() => {
                self.pick_base(path, base);
                self.pick_dlc(path, dlc);
            }
            (_, Ok(_)) => self.pick_dlc(path, dlc),
            _ => self.pick_base(path, base),
        }
        self.voices.done = None;
        self.screen = Screen::Voices;
        self.invalidate();
    }

    /// jpdlc: the DLC buttons' pick.
    fn set_voices_dlc(&mut self, path: &Path) {
        log::line(format!("voices: DLC picked: {}", path.display()));
        let dlc = voices::check_dlc(path);
        self.pick_dlc(path, dlc);
        self.voices.done = None;
        self.screen = Screen::Voices;
        self.invalidate();
    }

    fn pick_base(&mut self, path: &Path, result: Result<VoiceDump, crate::dump::Problem>) {
        match &result {
            Ok(d) => log::line(format!("  game: ok: {}", d.describe())),
            Err(p) => log::line(format!("  game: {}: {}", p.kind(), p.message())),
        }
        self.voices.picked_name = file_name(path);
        self.voices.picked = Some(result);
    }

    fn pick_dlc(&mut self, path: &Path, result: Result<VoiceDlc, crate::dump::Problem>) {
        match &result {
            Ok(d) => log::line(format!("  DLC: ok: {}", d.describe())),
            Err(p) => log::line(format!("  DLC: {}: {}", p.kind(), p.message())),
        }
        self.voices.dlc_picked_name = file_name(path);
        self.voices.dlc_picked = Some(result);
    }

    /// what Add the voices would install: the picks that check out, the
    /// DLC's only with the game's voices (picked or installed).
    fn voices_ready(&self) -> (Option<VoiceDump>, Option<VoiceDlc>) {
        let base = self.voices.picked.clone().and_then(|r| r.ok());
        let dlc = self.voices.dlc_picked.clone().and_then(|r| r.ok()).filter(|_| base.is_some() || self.voices_installed().is_some());
        (base, dlc)
    }

    fn start_voices(&mut self) {
        let (base, dlc) = self.voices_ready();
        if (base.is_none() && dlc.is_none()) || self.voices.job.is_some() {
            return;
        }
        *self.voices.progress.lock().unwrap() = Progress { step: "Starting".into(), ..Progress::default() };
        self.voices.cancel.store(false, Ordering::Relaxed);
        self.voices.done = None;
        let (data, progress, cancel) = (self.paths.data(), self.voices.progress.clone(), self.voices.cancel.clone());
        self.voices.job = Some(std::thread::spawn(move || {
            let installed = data.join("romfs");
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<VoicesDone, String> {
                let mut done = VoicesDone::default();
                if let Some(dump) = &base {
                    done.base = Some(voices::extract(dump, &installed, &data, &progress, &cancel)?);
                }
                // jpdlc: then the DLC's
                if let Some(dlc) = &dlc {
                    match voices::extract_dlc(dlc, &data, &progress, &cancel) {
                        Ok(d) => done.dlc = Some(d),
                        Err(e) if done.base.is_some() => done.dlc_error = Some(e),
                        Err(e) => return Err(e),
                    }
                }
                Ok(done)
            }));
            result.unwrap_or_else(|_| Err("Haiku Setup ran into an internal error. The log has the details.".into()))
        }));
    }

    pub(super) fn poll_voices(&mut self) {
        if self.voices.job.is_some() && self.screen == Screen::Voices {
            self.invalidate();
        }
        if self.voices.job.as_ref().is_some_and(|j| j.is_finished()) {
            let result = self.voices.job.take().unwrap().join().unwrap_or_else(|_| Err("Haiku Setup ran into an internal error.".into()));
            match &result {
                Ok(done) => {
                    // jpdlc: what was installed is no longer to add
                    if done.base.is_some() {
                        self.voices.picked = None;
                    }
                    if done.dlc.is_some() {
                        self.voices.dlc_picked = None;
                    }
                    if let Some(base) = &done.base {
                        log::line(format!("voices: installed {} files from {}", base.files, base.source));
                    }
                    if let Some(dlc) = &done.dlc {
                        log::line(format!("voices: DLC: installed {} files from {}", dlc.files, dlc.source));
                    }
                    if let Some(e) = &done.dlc_error {
                        log::line(format!("voices: DLC: failed: {e}"));
                    }
                    // what the player came for: the Japanese voices, on
                    if self.voices_installed().is_some() {
                        if let Err(e) = voices::set_japanese(&self.paths.root, true) {
                            log::line(format!("voices: the setting can't be saved ({e})"));
                        }
                    }
                }
                Err(e) => log::line(format!("voices: failed: {e}")),
            }
            self.voices.done = Some(result);
            self.invalidate();
        }
    }

    pub(super) fn voices_click(&mut self, id: Id) {
        match id {
            Id::PickFile => {
                let filter = [("3DS game dumps (*.3ds, *.cci, *.cia, *.cxi)", "*.3ds;*.cci;*.cia;*.cxi;*.app"), ("All files", "*.*")];
                if let Some(p) = pick_file(self.hwnd, "Select a dump with the Japanese voices", &filter) {
                    self.set_voices_dump(&p);
                }
            }
            Id::PickFolder => {
                if let Some(p) = pick_folder(self.hwnd, "Select the folder with the dump (an emulator's sdmc folder, or an extracted RomFS)") {
                    self.set_voices_dump(&p);
                }
            }
            // jpdlc
            Id::VoicesDlcFile => {
                let filter = [("DLC (*.cia)", "*.cia"), ("All files", "*.*")];
                if let Some(p) = pick_file(self.hwnd, "Select the Japanese DLC (Fire Emblem if), a decrypted .cia", &filter) {
                    self.set_voices_dlc(&p);
                }
            }
            Id::VoicesDlcFolder => {
                if let Some(p) = pick_folder(self.hwnd, "Select the emulator's sdmc folder with the Japanese DLC installed") {
                    self.set_voices_dlc(&p);
                }
            }
            Id::VoicesExtract => self.start_voices(),
            Id::Cancel => self.voices.cancel.store(true, Ordering::Relaxed),
            Id::VoicesRemove => {
                match voices::remove(&self.paths.data()) {
                    Ok(()) => log::line("voices: the Japanese voices were removed (the DLC's too)"),
                    Err(e) => log::line(format!("voices: can't remove ({e})")),
                }
                let _ = voices::set_japanese(&self.paths.root, false);
                self.voices.done = None;
            }
            Id::Back => {
                if self.voices.job.is_none() {
                    self.screen = Screen::Ready;
                }
            }
            _ => {}
        }
    }

    /// the Play screen's Voices choice.
    pub(super) fn voices_choose(&mut self, japanese: bool) {
        if japanese && self.voices_installed().is_none() {
            self.screen = Screen::Voices;
            return;
        }
        match voices::set_japanese(&self.paths.root, japanese) {
            Ok(()) => log::line(format!("voices: set to {}", if japanese { "Japanese" } else { "English" })),
            Err(e) => log::line(format!("voices: the setting can't be saved ({e})")),
        }
    }

    pub(super) fn voices_buttons(&self, add: &mut dyn FnMut(Id, &str, i32, i32, i32, bool, bool)) {
        let s = |v: i32| self.px(v);
        let bottom = s(HEIGHT - 70);
        let running = self.voices.job.is_some();
        match self.screen {
            Screen::Voices => {
                add(Id::PickFile, "Select file\u{2026}", 40, s(214), 170, false, !running);
                add(Id::PickFolder, "Select folder\u{2026}", 222, s(214), 170, false, !running);
                // jpdlc: the DLC's pick, beside the game's
                add(Id::VoicesDlcFile, "Japanese DLC\u{2026}", 404, s(214), 170, false, !running);
                add(Id::VoicesDlcFolder, "DLC folder\u{2026}", 586, s(214), 134, false, !running);
                add(Id::Back, "\u{2039}  Back", 40, bottom, 130, false, !running);
                if self.voices_installed().is_some() && !running {
                    add(Id::VoicesRemove, "Remove the voices", 182, bottom, 190, false, true);
                }
                if running {
                    let stopping = self.voices.cancel.load(Ordering::Relaxed);
                    add(Id::Cancel, if stopping { "Stopping\u{2026}" } else { "Cancel" }, WIDTH - 40 - 150, bottom, 150, false, !stopping);
                } else {
                    let (base, dlc) = self.voices_ready();
                    let bad = matches!(self.voices.picked, Some(Err(_))) || matches!(self.voices.dlc_picked, Some(Err(_)));
                    let ok = (base.is_some() || dlc.is_some()) && !bad;
                    add(Id::VoicesExtract, "Add the voices", WIDTH - 40 - 190, bottom, 190, true, ok);
                }
            }
            // jpn: a Japanese game's voices are its own: no Voices choice
            Screen::Ready if self.game_is_japanese() => {}
            Screen::Ready => {
                let japanese = self.voices_japanese();
                let have = self.voices_installed().is_some();
                add(Id::VoicesEnglish, "English", 130, s(356), 120, !japanese, true);
                add(Id::VoicesJapanese, "Japanese", 258, s(356), 120, japanese, have);
                add(Id::VoicesOpen, if have { "Japanese voices\u{2026}" } else { "Add Japanese voices\u{2026}" }, 390, s(356), 210, false, true);
            }
            _ => {}
        }
    }

    /// the Play screen's Voices line, beside its buttons.
    pub(super) unsafe fn draw_ready_voices(&self, dc: HDC) {
        let s = |v: i32| self.px(v);
        self.text(dc, self.fonts.bold, CREAM, RECT { left: s(40), top: s(356), right: s(126), bottom: s(398) }, "Voices", DT_SINGLELINE | DT_VCENTER);
        // jpn: the Japanese game speaks Japanese itself
        if self.game_is_japanese() {
            self.text(dc, self.fonts.small, MUTED, RECT { left: s(40), top: s(402), right: s(WIDTH - 40), bottom: s(424) }, "Japanese, the game's own (Fire Emblem if). Nothing to add.", DT_SINGLELINE | DT_END_ELLIPSIS);
            return;
        }
        // jpdlc: and whether the DLC's are there
        let note = match (self.voices_installed(), self.voices_dlc_installed()) {
            (Some(i), Some(d)) => format!("Japanese: {} voice clips, and the DLC's ({} clips, {} narration). Also in the game's Options > Audio.", i.clips, d.clips, d.narration),
            (Some(i), None) => format!("Japanese: {} voice clips, from {}. The DLC's voices aren't added: they stay English.", i.clips, short_source(&i.source)),
            (None, _) => "English, the game's own. Japanese voices can be added from a Japanese dump or an undub.".to_owned(),
        };
        self.text(dc, self.fonts.small, MUTED, RECT { left: s(40), top: s(402), right: s(WIDTH - 40), bottom: s(424) }, &note, DT_SINGLELINE | DT_END_ELLIPSIS);
    }

    pub(super) unsafe fn draw_voices(&self, dc: HDC, r: RECT) {
        let s = |v: i32| self.px(v);
        self.heading(
            dc,
            r,
            "Japanese voices (optional).",
            "A dump with the Japanese voices: Fire Emblem if (Japan) or an \u{201C}undub\u{201D}, and if you like the Japanese DLC. Only voice and sound files are taken; your game isn't changed.",
        );
        let top = s(274);
        if self.voices.job.is_some() {
            let p = self.voices.progress.lock().unwrap().clone();
            self.text(dc, self.fonts.bold, GOLD, RECT { left: r.left, top, right: r.right, bottom: top + s(24) }, &format!("{}\u{2026} {:.0}%", p.step, p.percent.floor()), DT_SINGLELINE);
            let bar = RECT { left: r.left, top: top + s(30), right: r.right, bottom: top + s(50) };
            self.round_rect(dc, bar, rgb(12, 32, 36), TRIM, s(10), 1);
            let fill_w = ((bar.right - bar.left - s(6)) as f32 * (p.percent / 100.0).clamp(0.0, 1.0)) as i32;
            if fill_w > s(4) {
                self.round_rect(dc, RECT { left: bar.left + s(3), top: bar.top + s(3), right: bar.left + s(3) + fill_w, bottom: bar.bottom - s(3) }, rgb(196, 150, 72), rgb(232, 190, 110), s(8), 1);
            }
            self.text(dc, self.fonts.body, CREAM, RECT { left: r.left, top: top + s(58), right: r.right, bottom: top + s(80) }, &p.detail, DT_SINGLELINE | DT_END_ELLIPSIS);
            return;
        }
        match &self.voices.done {
            Some(Ok(done)) => {
                let mut lines = Vec::new();
                if let Some(base) = &done.base {
                    lines.push(format!("{} files ({} voice clips, {} MB) are in data\\voices-ja.", base.files, base.clips, base.bytes / 1_000_000));
                }
                if let Some(dlc) = &done.dlc {
                    lines.push(format!(
                        "The DLC's: {} files of {} contents ({} voice clips, {} narration, {} MB) are in data\\voices-ja-dlc.",
                        dlc.files,
                        dlc.contents,
                        dlc.clips,
                        dlc.narration,
                        dlc.bytes / 1_000_000
                    ));
                }
                if let Some(e) = &done.dlc_error {
                    lines.push(format!("The DLC's voices couldn't be taken out: {e}"));
                }
                lines.push("Voices is now Japanese; change it on the Play screen or in the game's Options > Audio.".into());
                let headline = match (&done.base, &done.dlc_error) {
                    (None, _) => "The DLC's Japanese voices are installed",
                    (Some(_), Some(_)) => "The game's Japanese voices are installed, not the DLC's",
                    (Some(_), None) => "The Japanese voices are installed",
                };
                self.result_panel(dc, top, Some(done.dlc_error.is_none()), headline, &lines.join("\n"));
            }
            Some(Err(e)) => {
                self.result_panel(dc, top, Some(false), "The voices couldn't be taken out", &format!("{e}\n\nThe log is Haiku Setup.log, beside Haiku Setup."));
            }
            None if self.voices.picked.is_some() || self.voices.dlc_picked.is_some() => self.draw_voices_picks(dc, top),
            None => {
                if let Some(i) = self.voices_installed() {
                    let dlc = match self.voices_dlc_installed() {
                        Some(d) => format!("The DLC's too: {} voice clips and {} narration, from {}.", d.clips, d.narration, short_source(&d.source)),
                        None => "The DLC's voices aren't added: Japanese DLC\u{2026} adds them.".to_owned(),
                    };
                    self.result_panel(dc, top, None, "Japanese voices are installed", &format!("{} voice clips, from {}. {dlc} Select another dump to replace them.", i.clips, short_source(&i.source)));
                }
            }
        }
    }

    /// jpdlc: what the picks are, in one panel: the game's line, the DLC's.
    unsafe fn draw_voices_picks(&self, dc: HDC, top: i32) {
        let mut lines = Vec::new();
        let mut good = true;
        let mut headline = "Ready to add the voices";
        match &self.voices.picked {
            Some(Ok(d)) => lines.push(format!("Game: {}: {}.", self.voices.picked_name, d.describe())),
            Some(Err(p)) => {
                lines.push(format!("Game: {}: {}", self.voices.picked_name, p.message()));
                good = false;
                headline = "Not a dump with Japanese voices";
            }
            None => {}
        }
        match &self.voices.dlc_picked {
            Some(Ok(d)) => {
                lines.push(format!("DLC: {}: {}.", self.voices.dlc_picked_name, d.describe()));
                if self.voices.picked.is_none() && self.voices_installed().is_none() {
                    lines.push("The DLC's voices go with the game's: select a dump with the Japanese voices too.".into());
                    good = false;
                    headline = "The game's Japanese voices are needed first";
                }
            }
            Some(Err(p)) => {
                lines.push(format!("DLC: {}: {}", self.voices.dlc_picked_name, p.message()));
                if good {
                    headline = "Not the Japanese DLC";
                }
                good = false;
            }
            None => {}
        }
        // the next step, when there is room for it (one pick)
        if good && lines.len() < 2 {
            lines.push("Next, Haiku Setup checks them against their hashes and your game's, then copies the voices.".into());
        }
        self.result_panel(dc, top, Some(good), headline, &lines.join("\n"));
    }
}

fn short_source(source: &str) -> String {
    file_name(Path::new(source))
}
