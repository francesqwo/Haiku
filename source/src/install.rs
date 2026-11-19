//! os: the one-time installer, which extracts the game from the user's own
//! dump into data/ (see zakuro_fs::datadir), and checks an installed data/.
//!
//!   Haiku --install <game.3ds|.cci|.cxi|.cia> [<dlc.cia> ...] [--to <dir>]
//!   Haiku --verify-data <game dump> [<dlc.cia> ...] [--to <dir>]

use std::path::{Path, PathBuf};

use zakuro_fs::datadir;

/// runs the installer when the command line asks for it, the exit code
/// back; None to start the game.
pub fn run(root: &Path) -> Option<i32> {
    let mut args = std::env::args().skip(1);
    let check = match args.next().as_deref() {
        Some("--install") => false,
        Some("--verify-data") => true,
        _ => return None,
    };
    let mut out = root.join("data");
    let mut inputs = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--to" => out = PathBuf::from(args.next()?),
            _ => inputs.push(PathBuf::from(arg)),
        }
    }
    let Some((game, dlc)) = inputs.split_first() else {
        eprintln!("usage: Haiku --install <game dump> [<dlc.cia> ...] [--to <dir>]");
        return Some(2);
    };
    let started = std::time::Instant::now();
    let mut bad = 0;
    match datadir::install_game(game, &out, check) {
        Ok(tally) => {
            println!("game: {} files, {} folders, {} MiB, {} bad", tally.files, tally.dirs, tally.bytes >> 20, tally.bad);
            bad += tally.bad;
        }
        Err(error) => {
            eprintln!("could not {} {}: {error}", if check { "check" } else { "install" }, game.display());
            return Some(1);
        }
    }
    for cia in dlc {
        // eurdlc: the European DLC is not the USA game's
        if let Some(why) = zakuro_fs::eur::dlc_misfit(&out, cia) {
            eprintln!("could not {} DLC {}: {why}", if check { "check" } else { "install" }, cia.display());
            return Some(1);
        }
        match datadir::install_dlc(cia, &out, check) {
            Ok(tally) => bad += tally.bad,
            Err(error) => {
                eprintln!("could not {} DLC {}: {error}", if check { "check" } else { "install" }, cia.display());
                return Some(1);
            }
        }
    }
    // eur: the DLC next to the EUR game, filed as it asks and with its
    // text in every EUR language folder
    if !check && !dlc.is_empty() {
        match zakuro_fs::eur::adapt_dlc(&out) {
            Ok(done) => done.iter().for_each(|line| println!("eur: {line}")),
            Err(error) => {
                eprintln!("could not fit the DLC to the EUR game: {error}");
                return Some(1);
            }
        }
    }
    let verb = if check { "checked" } else { "installed" };
    println!("{verb} {} in {:.1} s, {bad} problems", out.display(), started.elapsed().as_secs_f64());
    Some(if bad == 0 { 0 } else { 1 })
}
