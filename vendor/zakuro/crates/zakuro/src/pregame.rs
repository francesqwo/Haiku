//! updscreen: work a program must finish before its game's code can be
//! loaded, done while the game's own window shows it. Haiku builds the
//! game's module again after an update (or for a version switched to whose
//! module is out of date) this way: the window opens with the player's
//! window settings, shows the work in the game's look (fates_ui/pregame.rs)
//! and, once it is done, the game starts in the same window.
//!
//! The program gives `run_after` its work; nothing of this runs on a start
//! that has nothing to do (`run` as before).

pub use zakuro_core::recompiled::Linked;

/// how far the work is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress {
    /// 0 to 100
    pub percent: f32,
    /// the step, in the player's words ("Compiling the game's code")
    pub step: String,
    /// what of it ("12 of 254 files, about 3 min left")
    pub detail: String,
}

/// what the work is doing.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Working(Progress),
    /// finished: `finish` gives the code
    Done,
    /// stopped with an error, in the player's words
    Failed(String),
}

/// the work, as the program does it (on threads or processes of its own:
/// the window's thread only asks).
pub trait Pregame {
    /// starts the work, or starts it again after it failed.
    fn start(&mut self) -> Result<(), String>;
    /// what it is doing now (called each frame: cheap).
    fn poll(&mut self) -> State;
    /// the work is done: the game's code, ready to run. Called on the
    /// window's thread before the game starts, so the program can set what
    /// must be set before its code runs.
    fn finish(&mut self) -> Result<Linked, String>;
    /// the player quits: the work stops (it carries on next time).
    fn stop(&mut self);
    /// shows the player the work's log.
    fn open_log(&self);
    /// what the screen says it does, a line under its title.
    fn why(&self) -> String;
    /// the game's art can't be read at all (no game-derived files): the
    /// error the program tells the player its own way; true to try again.
    fn failed_without_art(&mut self, error: &str) -> bool;
}
