//! codemods: the Mods screen's part in code mods: working out and checking
//! them when the game starts, compiling them when the player applies (after
//! asking to download a compiler when there is none), the progress dialog
//! and the consent dialog, both drawn as the game's own dialogs.

use std::path::Path;
use std::sync::Arc;

use egui::Color32;

use super::kit::{Art, Canvas, TEXT_CREAM, TEXT_DARK};
use super::options::{PANEL_H, PANEL_Y, WIDTH};
use crate::haiku_codemods::{self as codemods, Game, Job, Stage};
use crate::haiku_mods::Note;
use crate::App;
use zakuro_core::codemods::CodeMods;

/// the compile in progress, as the page shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JobView {
    pub title: String,
    pub line: String,
    pub fraction: f32,
}

impl JobView {
    pub fn of(job: &Job) -> JobView {
        let stage = job.stage();
        let line = match &stage {
            Stage::Downloading(f) => format!("Downloading the compiler... {:.0}%", f * 100.0),
            Stage::Analysing => "Finding the code the mods change...".to_owned(),
            Stage::Compiling { done, total } => format!("Compiling... {done} of {total}"),
            Stage::Done => "Done.".to_owned(),
            Stage::Failed(error) => error.clone(),
        };
        JobView { title: format!("Preparing {}", job.names.join(", ")), line, fraction: stage.fraction() }
    }
}

/// the state FatesUi keeps for code mods.
#[derive(Default)]
pub struct CodeModsState {
    pub job: Option<Job>,
    /// the game's code, read once per game.
    pub game: Option<(std::path::PathBuf, Arc<Game>)>,
    /// restart once the job is done.
    pub restart_after: bool,
    /// the consent dialog's question, while it is up: the download's size.
    pub asking: Option<u64>,
}

const DIALOG_W: f32 = 300.0;

/// the consent dialog's text.
pub fn download_text(mb: u64) -> String {
    format!("Code mods need a compiler: a one-time download of {mb}\u{a0}MB from ziglang.org. Download it now?")
}

/// draws the compile's progress over the menu, as the game's dialog with a
/// bar; B or Esc cancels it.
pub fn draw_job(canvas: &Canvas, job: &JobView, now: f64) {
    let white = Color32::WHITE;
    canvas.fill(0.0, 0.0, WIDTH, super::options::HEIGHT, Color32::from_black_alpha(90));
    let lines = canvas.kit.wrap_even(&job.title, DIALOG_W - 36.0, 0.9);
    let height = lines.len() as f32 * 15.0 + 22.0 + 40.0;
    let x = (WIDTH - DIALOG_W) / 2.0;
    let y = PANEL_Y + (PANEL_H - height) / 2.0;
    canvas.nine(Art::Dialog, x, y, DIALOG_W, height, [22.0, 22.0, 22.0, 22.0], 0.5, white);
    for (index, line) in lines.iter().enumerate() {
        canvas.text_centered(x + DIALOG_W / 2.0, y + 11.0 + index as f32 * 15.0, line, 0.9, TEXT_DARK);
    }
    let bar_y = y + 14.0 + lines.len() as f32 * 15.0;
    let bar_w = DIALOG_W - 70.0;
    // the game's pill as the bar's track, the red bar as its fill
    canvas.nine(Art::PillLabel, x + 35.0, bar_y, bar_w, 10.0, [8.0, 8.0, 8.0, 8.0], 0.5, white);
    let filled = (bar_w - 4.0) * job.fraction.clamp(0.0, 1.0);
    if filled > 2.0 {
        canvas.three(Art::Highlight, x + 37.0, bar_y + 1.5, filled, 7.0, [3.0, 3.0], white);
    }
    let dots = ".".repeat(((now * 2.0) as usize) % 3 + 1);
    let line = job.line.trim_end_matches("...").to_owned();
    let line = if job.line.ends_with("...") { format!("{line}{dots}") } else { line };
    canvas.text_centered(x + DIALOG_W / 2.0, bar_y + 14.0, &line, 0.72, TEXT_DARK);
    let _ = TEXT_CREAM;
}

impl App {
    /// the game's code, read once per game.
    fn fates_code_game(&mut self, path: &Path) -> Result<Arc<Game>, String> {
        if let Some((known, game)) = &self.fates.code.game {
            if known == path {
                return Ok(game.clone());
            }
        }
        let game = Arc::new(Game::load(path)?);
        self.fates.code.game = Some((path.to_owned(), game.clone()));
        Ok(game)
    }

    /// what the code mods turned on make of the game, for the game about to
    /// start at path: checked, compiled (headless, or when
    /// FATES_CODEMODS_BUILD=1, with a compiler there), loaded. a code mod
    /// that can't be used is left out whole with a note: its files too.
    pub(crate) fn fates_code_mods(&mut self, path: &Path) -> CodeMods {
        let data_dir = self.data_dir.clone();
        let list = &mut self.fates.mods;
        list.code_notes.clear();
        list.left_out.clear();
        zakuro_core::codemods::set_running(Vec::new(), None);
        // the code mods the last run turned off
        if let Some((names, at)) = codemods::take_failed(data_dir.as_deref()) {
            for index in 0..list.mods.len() {
                if names.iter().any(|name| *name == list.mods[index].name) {
                    list.set_enabled(index, false);
                    let text = format!("It took the game to code Haiku can't run{}, so it was turned off.", if at.is_empty() { String::new() } else { format!(" (at {at})") });
                    list.code_notes.entry(list.mods[index].id.clone()).or_default().push(Note { warning: true, text });
                }
            }
        }
        let using: Vec<usize> = (0..list.mods.len()).filter(|&index| list.uses_code(index)).collect();
        if using.is_empty() {
            list.check();
            return CodeMods::default();
        }
        let leave_out = |list: &mut crate::haiku_mods::ModList, indices: &[usize], why: &str| {
            for &index in indices {
                let id = list.mods[index].id.clone();
                list.left_out.insert(id.clone());
                list.code_notes.entry(id).or_default().push(Note { warning: true, text: why.to_owned() });
                log::warn!("codemods: {} is left out: {why}", list.mods[index].name);
            }
        };
        let Some(linked) = self.linked else {
            leave_out(&mut self.fates.mods, &using, "Code mods need the recompiled game, which this build doesn't have.");
            self.fates.mods.check();
            return CodeMods::default();
        };
        let base = match codemods::base(&linked) {
            Ok(base) => base,
            Err(error) => {
                leave_out(&mut self.fates.mods, &using, &format!("Its code can't be checked: {error}."));
                self.fates.mods.check();
                return CodeMods::default();
            }
        };
        let digest = base.digest();
        let game = match self.fates_code_game(path) {
            Ok(game) => game,
            Err(error) => {
                leave_out(&mut self.fates.mods, &using, &format!("Its code can't be applied: {error}."));
                self.fates.mods.check();
                return CodeMods::default();
            }
        };
        let list = &mut self.fates.mods;
        let plan = codemods::plan(list, &game, digest);
        for (index, problem) in &plan.problems {
            leave_out(list, &[*index], &format!("Its code can't be used: {problem}. It stays out until that is fixed."));
        }
        for (index, notes) in &plan.notes {
            let id = list.mods[*index].id.clone();
            list.code_notes.entry(id).or_default().extend(notes.iter().map(|text| Note { warning: true, text: text.clone() }));
        }
        let patching: Vec<usize> = plan.mods.iter().map(|(index, _)| *index).collect();
        if plan.patches() && !codemods::is_built(&plan) {
            let build_now = self.options.headless.is_some() || std::env::var("FATES_CODEMODS_BUILD").is_ok_and(|value| value == "1");
            match codemods::compiler().filter(|_| build_now) {
                Some(compiler) => {
                    log::info!("codemods: compiling {} with {}", plan.names().join(", "), compiler.describe());
                    let never = std::sync::atomic::AtomicBool::new(false);
                    if let Err(error) = codemods::build(&plan, &game, &base, &compiler, &|stage| log::debug!("codemods: {stage:?}"), &never) {
                        leave_out(list, &patching, &format!("Its code couldn't be compiled: {error}."));
                    }
                }
                None if build_now => leave_out(list, &patching, "Its code isn't compiled, and there is no compiler: open the Mods screen and choose Apply and Restart."),
                None => leave_out(list, &patching, "Its code isn't compiled yet: choose Apply and Restart to compile it."),
            }
        }
        let list = &mut self.fates.mods;
        let mods = if patching.iter().any(|index| list.left_out.contains(&list.mods[*index].id)) {
            // never half the code mods: with one of them out, the patched
            // code is not what was compiled; the plugins still load
            let rest: Vec<usize> = patching.iter().copied().filter(|index| !list.left_out.contains(&list.mods[*index].id)).collect();
            leave_out(list, &rest, "It is left out with the other code mods this time: one of them can't be used.");
            CodeMods { plugins: plan.plugins.clone(), ..CodeMods::default() }
        } else {
            match codemods::prepare(&plan, digest) {
                Ok(mods) => {
                    if let Some(built) = codemods::built(plan.key).filter(|_| plan.patches()) {
                        for &index in &patching {
                            let id = list.mods[index].id.clone();
                            list.code_notes.entry(id).or_default().extend(codemods::built_notes(&built).into_iter().map(|text| Note { warning: text.contains("Haiku has its own"), text }));
                        }
                    }
                    mods
                }
                Err(error) => {
                    leave_out(list, &patching, &format!("Its compiled code can't be used ({error}): choose Apply and Restart to compile it again."));
                    let _ = std::fs::remove_file(codemods::module_path(plan.key));
                    CodeMods { plugins: plan.plugins.clone(), ..CodeMods::default() }
                }
            }
        };
        // plugins of mods left out go too
        let left: Vec<String> = list.left_out.iter().cloned().collect();
        let mut mods = mods;
        mods.plugins.retain(|plugin| !left.contains(&plugin.id));
        let running: Vec<String> = patching.iter().filter(|index| !list.left_out.contains(&list.mods[**index].id)).map(|index| list.mods[*index].name.clone()).chain(mods.plugins.iter().map(|plugin| plugin.name.clone())).collect();
        zakuro_core::codemods::set_running(running, codemods::failed_path(data_dir.as_deref()));
        list.check();
        mods
    }

    /// after the game is loaded: what the loader said about the code mods,
    /// on the mods it was about.
    pub(crate) fn fates_code_mods_loaded(&mut self) {
        let notes = zakuro_core::codemods::take_notes();
        if notes.is_empty() {
            return;
        }
        let list = &mut self.fates.mods;
        for text in notes {
            let index = list.mods.iter().position(|info| text.starts_with(&info.name) || text.contains(&format!("{}:", info.name)));
            if let Some(index) = index {
                let id = list.mods[index].id.clone();
                list.code_notes.entry(id).or_default().push(Note { warning: true, text });
            }
        }
        list.check();
        self.fates.env.mods = super::mods::ModsView::of(&self.fates.mods);
    }

    /// whether the restart can go ahead: the code mods turned on are
    /// compiled. if not, compiling starts (or the player is asked to
    /// download the compiler), and the restart follows once it is done.
    pub(crate) fn fates_code_mods_ready(&mut self) -> bool {
        if self.fates.code.job.is_some() {
            return false;
        }
        let Some(path) = self.game.as_ref().map(|game| game.path.clone()) else { return true };
        let Some(linked) = self.linked else { return true };
        let list = &self.fates.mods;
        if !(0..list.mods.len()).any(|index| list.uses_code(index)) {
            return true;
        }
        let Ok(base) = codemods::base(&linked) else { return true };
        let Ok(game) = self.fates_code_game(&path) else { return true };
        let plan = codemods::plan(&self.fates.mods, &game, base.digest());
        if !plan.patches() || codemods::is_built(&plan) {
            return true;
        }
        match codemods::compiler() {
            Some(compiler) => {
                log::info!("codemods: compiling {} with {} before the restart", plan.names().join(", "), compiler.describe());
                self.fates.code.job = Some(Job::start(plan, game, Arc::new(base), Some(compiler)));
                self.fates.code.restart_after = true;
            }
            None => match codemods::download_mb() {
                Some(mb) => {
                    log::info!("codemods: no compiler; asking to download one ({mb} MB)");
                    self.fates.code.asking = Some(mb);
                }
                None => self.menus.message = Some("Code mods need a C compiler, and Haiku has none to download for this computer. Put one in the tools folder next to Haiku.exe.".to_owned()),
            },
        }
        self.fates_code_view();
        false
    }

    /// the player answered the download question.
    pub(crate) fn fates_code_mods_download(&mut self, yes: bool) {
        self.fates.code.asking = None;
        if !yes {
            log::info!("codemods: the compiler download was declined; the code mods stay as they were");
            self.menus.toast = Some(("Code mods need the compiler: nothing was downloaded".to_owned(), std::time::Instant::now()));
            self.fates_code_view();
            return;
        }
        let Some(path) = self.game.as_ref().map(|game| game.path.clone()) else { return };
        let (Some(linked), Ok(game)) = (self.linked, self.fates_code_game(&path)) else { return };
        let Ok(base) = codemods::base(&linked) else { return };
        let plan = codemods::plan(&self.fates.mods, &game, base.digest());
        log::info!("codemods: downloading the compiler, which the player said yes to");
        self.fates.code.job = Some(Job::start(plan, game, Arc::new(base), None));
        self.fates.code.restart_after = true;
        self.fates_code_view();
    }

    /// cancels the compile.
    pub(crate) fn fates_code_mods_cancel(&mut self) {
        if let Some(job) = &self.fates.code.job {
            job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// each frame: follows the compile, restarting once it is done, and
    /// putting its failure on the mods.
    pub(crate) fn fates_code_mods_poll(&mut self) {
        let Some(job) = &self.fates.code.job else { return };
        match job.stage() {
            Stage::Done => {
                self.fates.code.job = None;
                self.fates_code_view();
                if std::mem::take(&mut self.fates.code.restart_after) {
                    self.fates_mods_restart();
                }
            }
            Stage::Failed(error) => {
                let names = job.names.clone();
                self.fates.code.job = None;
                self.fates.code.restart_after = false;
                let list = &mut self.fates.mods;
                for index in 0..list.mods.len() {
                    if names.contains(&list.mods[index].name) {
                        let id = list.mods[index].id.clone();
                        list.code_notes.entry(id).or_default().push(Note { warning: true, text: format!("Its code couldn't be compiled: {error}.") });
                    }
                }
                list.check();
                self.menus.toast = Some(("The code mods couldn't be compiled".to_owned(), std::time::Instant::now()));
                self.fates_code_view();
            }
            _ => {
                let view = Some(JobView::of(job));
                if self.fates.env.mods.job != view {
                    self.fates.env.mods.job = view;
                }
            }
        }
    }

    /// what the page shows of the code mods.
    pub(crate) fn fates_code_view(&mut self) {
        self.fates.env.mods = super::mods::ModsView::of(&self.fates.mods);
        self.fates.env.mods.job = self.fates.code.job.as_ref().map(JobView::of);
        self.fates.env.mods.ask_download = self.fates.code.asking;
    }
}
