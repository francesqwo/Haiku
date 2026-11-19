//! tones: Options > Gameplay > Speaking Style, Corrin's speaking style (the
//! Japanese game's talk tones). The row is there only while a
//! Japanese-script translation with the styles is on
//! (zakuro_core::tones::available); without one the US game has one style
//! and nothing is shown. The style is the loaded game's (saved with it);
//! see zakuro-core/src/tones.rs.

use super::options::Effect;

/// the Gameplay tab's row, while a translation with speaking styles is on:
/// after `after` (Kana's Gender), else at the end.
pub fn gameplay_rows<T: PartialEq + Copy>(list: &mut Vec<T>, row: T, after: T) {
    if !zakuro_core::tones::available() {
        return;
    }
    let at = list.iter().position(|&item| item == after).map_or(list.len(), |at| at + 1);
    list.insert(at, row);
}

/// the row: its name, value and help line.
pub fn row() -> (String, Option<String>, String) {
    let name = "Speaking Style".to_owned();
    match zakuro_core::tones::current() {
        Some((style, female, true)) => {
            let names = zakuro_core::tones::names(female);
            (
                name,
                Some(names[style as usize - 1].clone()),
                format!("How Corrin speaks, as in the Japanese game: {}, {} or {}. Saved with your game; from the next scene on.", names[0], names[1], names[2]),
            )
        }
        _ => {
            let (male, female) = (zakuro_core::tones::names(false), zakuro_core::tones::names(true));
            (
                name,
                Some("\u{2014}".to_owned()),
                format!("In the avatar creator each voice is a style ({}, {}, {}; {}, {}, {}). Load a game to change it here.", male[0], male[1], male[2], female[0], female[1], female[2]),
            )
        }
    }
}

/// left or right: the next style for the loaded game's Corrin.
pub fn adjust(dir: i32) -> Option<Effect> {
    let (style, _, true) = zakuro_core::tones::current()? else { return None };
    let next = ((style as i32 - 1 + dir).rem_euclid(3) + 1) as u8;
    zakuro_core::tones::request(next);
    Some(Effect::Apply)
}
