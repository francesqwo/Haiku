//! enhancements, the community's codes that make games run at 60 FPS and
//! the like, for the builds of games they were tested on, which a player
//! turns on by name instead of finding and pasting the code.

use crate::cheats::Cheat;

/// the codes, one game after another, its title id and name, then each
/// enhancement, its name, the build it is for, who made it and what to know
/// about it, and its code.
const LIST: &str = include_str!("enhancements.txt");

/// one enhancement for a game.
#[derive(Debug, Clone, PartialEq)]
pub struct Enhancement {
    pub name: String,
    pub author: String,
    /// what a player should know before turning it on, empty if nothing.
    pub note: String,
    pub lines: Vec<String>,
}

/// the build a title runs, which an enhancement's code has to be made for:
/// its update's version, or else its cartridge's revision.
pub fn build(title: &zakuro_fs::Title) -> String {
    match title.update() {
        Some(update) => format!("update {}.{}", update.version >> 10, update.version >> 4 & 0x3F),
        None => format!("rev{}", title.exheader.remaster_version),
    }
}

/// the enhancements for a game of program_id running build.
pub fn for_title(program_id: u64, build: &str) -> Vec<Enhancement> {
    for_title_in(LIST, program_id, build)
}

/// public: the list (enhancements.txt) is empty in Haiku, which runs one
/// game; the reading is kept, tested on a list of the tests' own.
fn for_title_in(list: &str, program_id: u64, build: &str) -> Vec<Enhancement> {
    let mut found: Vec<Enhancement> = Vec::new();
    let (mut game, mut taking) = (false, false);
    for line in list.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')) {
        if let Some(header) = line.strip_prefix('=') {
            let id = header.split_whitespace().next().and_then(|id| u64::from_str_radix(id, 16).ok());
            game = id == Some(program_id);
            taking = false;
        } else if let Some((name, rest)) = line.split_once('|').filter(|_| game) {
            let mut fields = rest.split('|').map(str::trim);
            let made_for = fields.next().unwrap_or_default();
            taking = made_for == "any" || made_for == build;
            if taking {
                found.push(Enhancement {
                    name: name.trim().to_owned(),
                    author: fields.next().unwrap_or_default().to_owned(),
                    note: fields.next().unwrap_or_default().to_owned(),
                    lines: Vec::new(),
                });
            }
        } else if game && taking {
            if let Some(enhancement) = found.last_mut() {
                enhancement.lines.push(line.to_owned());
            }
        }
    }
    found
}

/// the game's enhancements as cheats of its own, the ones named in on
/// turned on, which go nowhere near the player's cheat file.
pub fn cheats(program_id: u64, build: &str, on: &[String]) -> Vec<Cheat> {
    as_cheats(for_title(program_id, build), on)
}

fn as_cheats(enhancements: Vec<Enhancement>, on: &[String]) -> Vec<Cheat> {
    enhancements
        .into_iter()
        .map(|enhancement| {
            let credit = format!("Code by {}, from the 60 FPS thread on GBAtemp.", enhancement.author);
            let notes = [enhancement.note, credit].into_iter().filter(|line| !line.is_empty()).collect();
            let mut cheat = Cheat::new(&enhancement.name, enhancement.lines, notes);
            cheat.builtin = true;
            cheat.enabled = on.contains(&enhancement.name);
            cheat
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// a list in the format of enhancements.txt, with made-up codes.
    const SAMPLE: &str = "# a comment

= 0004000000123400 A Game (USA)
60 FPS | rev2 | Someone | The game runs faster with it.
D3000000 08000000
2020A748 0000003C
D2000000 00000000

= 0004000000123500 Another Game (USA)
60 FPS | any | Someone else | 
28C61A85 00000000
";

    /// each enhancement is for its own game and build, with its note and
    /// who made it, and only its own code.
    #[test]
    fn a_game_gets_the_enhancements_for_its_build() {
        let enhancements = for_title_in(SAMPLE, 0x0004_0000_0012_3400, "rev2");
        assert_eq!(enhancements.len(), 1);
        let fps = &enhancements[0];
        assert_eq!((fps.name.as_str(), fps.author.as_str()), ("60 FPS", "Someone"));
        assert!(!fps.note.is_empty());
        assert_eq!(fps.lines, ["D3000000 08000000", "2020A748 0000003C", "D2000000 00000000"]);
        assert!(for_title_in(SAMPLE, 0x0004_0000_0012_3400, "rev0").is_empty());
        assert!(for_title_in(SAMPLE, 0x0004_0000_0012_3400, "update 1.1").is_empty());
        assert_eq!(for_title_in(SAMPLE, 0x0004_0000_0012_3500, "update 1.1")[0].lines, ["28C61A85 00000000"]);

        let cheats = as_cheats(for_title_in(SAMPLE, 0x0004_0000_0012_3400, "rev2"), &["60 FPS".to_owned()]);
        assert!(cheats[0].enabled && cheats[0].builtin);
        assert_eq!(cheats[0].notes.len(), 2);
    }

    #[test]
    fn every_listed_code_is_code() {
        for list in [LIST, SAMPLE] {
            for line in list.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with('=') && !line.contains('|')) {
                assert!(crate::cheats::is_code(line), "{line}");
            }
        }
    }
}
