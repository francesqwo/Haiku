//! rando4: lines made longer by the randomizer's names and words are wrapped
//! again so they stay inside their window.
//!
//! The game's lines carry their own line breaks (`\n`) and pages (`$k` waits
//! for a press, `$k$p` starts a new page; `$k\n` ends a speaker's turn), and
//! its windows never wrap: a line wider than the window is cut off ("King
//! Garon" becoming "King Sumeragi" pushed "kill 'em all!" out of Chapter 1's
//! talk box). After the randomizer rewrote a line (text.rs), each page whose
//! lines got wider than the window, and wider than they were, is wrapped
//! again by words with the advance widths of the talk font
//! (`font/Talk.bfnt`). A page keeps the window's two lines; a page that
//! needs more is split into pages the game's way (`$k$p`). Every other page
//! stays exactly as it was, so short names change nothing.
//!
//! The widths are the talk font's own (the windows draw it scaled, the same
//! for every line). The usable widths were measured: the map and story talk
//! box (`$t1`) cut that line at about 339 (the widest vanilla line of the
//! chapters' scenes is 343); the
//! speech bubbles (`$t0`) fit 317 (the widest vanilla line); everything else
//! (status-screen descriptions, castle chatter, quotes) is kept to its own
//! widest line, and at least 312.

use std::collections::HashMap;

use super::rom::{lz11, Rom};

/// the talk box ($t1): measured from the cut-off line (see above)
const BOX_WIDTH: u32 = 340;
/// the speech bubbles ($t0): their widest vanilla line
const BUBBLE_WIDTH: u32 = 317;
/// windows without a talk type: at least this, else their own widest line
const OTHER_WIDTH: u32 = 312;
/// lines per page in the talk box and bubbles
const PAGE_LINES: usize = 2;
/// $Nu, the player's name (unknown here; the same on both sides)
const NAME_WIDTH: u32 = 60;

pub struct Font {
    advance: HashMap<u32, u32>,
    fallback: u32,
}

impl Font {
    /// the talk font's advance widths (a bfnt: glyph records of 16 bytes
    /// from 0x30, their count at 0x22: u32 code point, u16 x, u16 y, u8
    /// width, u8 height, u8, u8, u32 advance)
    pub fn load(rom: &Rom) -> Option<Font> {
        Font::from_bfnt(&rom.read("font/Talk.bfnt.lz").and_then(|f| lz11(&f))?)
    }

    pub fn from_bfnt(data: &[u8]) -> Option<Font> {
        let count = u16::from_le_bytes([*data.get(0x22)?, *data.get(0x23)?]) as usize;
        let mut advance = HashMap::new();
        for i in 0..count {
            let at = 0x30 + i * 16;
            let rec = data.get(at..at + 16)?;
            let cp = u32::from_le_bytes([rec[0], rec[1], rec[2], rec[3]]);
            let adv = u32::from_le_bytes([rec[12], rec[13], rec[14], rec[15]]);
            advance.insert(cp, adv);
        }
        let fallback = advance.get(&('m' as u32)).copied().unwrap_or(12);
        (!advance.is_empty()).then_some(Font { advance, fallback })
    }

    #[cfg(test)]
    pub fn uniform(width: u32) -> Font {
        Font { advance: HashMap::new(), fallback: width }
    }

    fn char_width(&self, u: u16) -> u32 {
        self.advance.get(&(u as u32)).copied().unwrap_or(self.fallback)
    }
}

const DOLLAR: u16 = b'$' as u16;
const BAR: u16 = b'|' as u16;
const NL: u16 = b'\n' as u16;
const SPACE: u16 = b' ' as u16;

fn is_digit(u: u16) -> bool {
    (b'0' as u16..=b'9' as u16).contains(&u)
}

/// the $ code at i: where it ends, with the arguments some codes have after
/// their bar ($Wm<who>|<slot>, $Sbp<clip>|<n>|, $Sbv<n>|<n>|...)
fn code_span(text: &[u16], i: usize) -> usize {
    let c1 = text.get(i + 1).copied().unwrap_or(0);
    let c2 = text.get(i + 2).copied().unwrap_or(0);
    let to_bar = || text[i..].iter().position(|&u| u == BAR).map_or(text.len(), |p| i + p + 1);
    let mut end = match c1 {
        0x6B | 0x70 | 0x61 => i + 2,               // $k $p $a
        0x4E | 0x74 => i + 3,                      // $Nu $t0
        0x57 if c2 != 0x6D && c2 != 0x73 => i + 3, // $Wa $Wd $Wc ...
        _ => to_bar(),
    }
    .min(text.len());
    if c1 == b'W' as u16 && c2 == b'm' as u16 && text.get(end).copied().is_some_and(is_digit) {
        end += 1;
    } else if c1 == b'S' as u16 {
        // a number and a bar after: $Sbp<clip>|0|, $Sls<se>|1000|
        let mut j = end;
        while text.get(j).copied().is_some_and(is_digit) {
            j += 1;
        }
        if j > end && text.get(j) == Some(&BAR) {
            end = j + 1;
        }
    }
    end.min(text.len())
}

fn is(text: &[u16], at: usize, s: &str) -> bool {
    s.encode_utf16().enumerate().all(|(k, u)| text.get(at + k) == Some(&u))
}

/// the width a code shows: the player's name, the widest of a $G code's
/// words, else nothing
fn code_width(font: &Font, code: &[u16]) -> u32 {
    if is(code, 0, "$Nu") {
        return NAME_WIDTH;
    }
    if is(code, 0, "$G") && code.len() > 3 {
        return code[2..code.len() - 1].split(|&u| u == b',' as u16).map(|w| w.iter().map(|&u| font.char_width(u)).sum()).max().unwrap_or(0);
    }
    0
}

/// a page: its text, and what ends it ($k, $k$p, $k\n, or the end)
struct Page {
    body: (usize, usize),
    sep: (usize, usize),
}

fn pages(text: &[u16]) -> Vec<Page> {
    let mut out = Vec::new();
    let (mut start, mut i) = (0, 0);
    while i < text.len() {
        if text[i] == DOLLAR {
            let end = code_span(text, i);
            if is(text, i, "$k") && end == i + 2 {
                let mut sep_end = end;
                if is(text, end, "$p") {
                    sep_end += 2;
                } else if text.get(end) == Some(&NL) {
                    sep_end += 1;
                }
                out.push(Page { body: (start, i), sep: (i, sep_end) });
                start = sep_end;
                i = sep_end;
                continue;
            }
            i = end.max(i + 1);
            continue;
        }
        i += 1;
    }
    out.push(Page { body: (start, text.len()), sep: (text.len(), text.len()) });
    out
}

/// a page's lines: (start, end) of each, split at its line breaks
fn lines(text: &[u16], (start, end): (usize, usize)) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let (mut from, mut i) = (start, start);
    while i < end {
        if text[i] == DOLLAR {
            i = code_span(text, i).max(i + 1).min(end);
            continue;
        }
        if text[i] == NL {
            out.push((from, i));
            from = i + 1;
        }
        i += 1;
    }
    out.push((from, end));
    out
}

fn width(font: &Font, text: &[u16]) -> u32 {
    let mut w = 0;
    let mut i = 0;
    while i < text.len() {
        if text[i] == DOLLAR {
            let end = code_span(text, i).max(i + 1);
            w += code_width(font, &text[i..end]);
            i = end;
            continue;
        }
        w += font.char_width(text[i]);
        i += 1;
    }
    w
}

/// a page's words (codes stay with the word they are in or before)
fn words(text: &[u16]) -> Vec<Vec<u16>> {
    let mut out: Vec<Vec<u16>> = Vec::new();
    let mut cur: Vec<u16> = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if text[i] == DOLLAR {
            let end = code_span(text, i).max(i + 1);
            cur.extend_from_slice(&text[i..end]);
            i = end;
            continue;
        }
        if text[i] == SPACE || text[i] == NL {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(text[i]);
        }
        i += 1;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// the window a line is for: its widest line, and how many lines a page has
/// (None: no pages, keep every line)
fn window(original: &[u16]) -> (u32, Option<usize>) {
    let start = original.iter().take(8).copied().collect::<Vec<u16>>();
    let at = if is(&start, 0, "$a") { 2 } else { 0 };
    if is(&start, at, "$t1") {
        (BOX_WIDTH, Some(PAGE_LINES))
    } else if is(&start, at, "$t0") {
        (BUBBLE_WIDTH, Some(PAGE_LINES))
    } else {
        (OTHER_WIDTH, None)
    }
}

/// words in lines no wider than the limit, greedily
fn wrap_words(font: &Font, words: &[Vec<u16>], limit: u32) -> Vec<Vec<u16>> {
    let space = font.char_width(SPACE);
    let mut lines: Vec<Vec<u16>> = vec![Vec::new()];
    let mut w = 0;
    for word in words {
        let ww = width(font, word);
        let last = lines.last_mut().expect("a line");
        if !last.is_empty() && w + space + ww > limit {
            lines.push(word.clone());
            w = ww;
        } else {
            if !last.is_empty() {
                last.push(SPACE);
                w += space;
            }
            last.extend_from_slice(word);
            w += ww;
        }
    }
    lines
}

/// whether a word ends a sentence (. ! ? … then quotes)
fn ends_sentence(word: &[u16]) -> bool {
    let shown: Vec<u16> = word.iter().copied().filter(|&u| u != b'"' as u16 && u != 0x27 && u != 0x201D && u != 0x2019 && u != b')' as u16).collect();
    shown.last().is_some_and(|&u| u == b'.' as u16 || u == b'!' as u16 || u == b'?' as u16 || u == 0x2026)
}

/// words in pages of at most per_page lines: a page that would need more is
/// split at the last sentence that fits, else after its last full line
fn paginate(font: &Font, words: &[Vec<u16>], limit: u32, per_page: usize) -> Vec<Vec<Vec<u16>>> {
    let lines = wrap_words(font, words, limit);
    if lines.len() <= per_page || words.len() < 2 {
        return vec![lines];
    }
    let fits = |k: usize| wrap_words(font, &words[..k], limit).len() <= per_page;
    let split = (1..words.len())
        .rev()
        .find(|&k| ends_sentence(&words[k - 1]) && fits(k))
        .or_else(|| (1..words.len()).rev().find(|&k| fits(k)))
        .unwrap_or(1);
    let mut out = vec![wrap_words(font, &words[..split], limit)];
    out.extend(paginate(font, &words[split..], limit, per_page));
    out
}

/// the line rewritten (`new`) with each page that got too wide wrapped
/// again; None when every page still fits (or the two don't line up).
pub fn rewrap(font: &Font, original: &[u16], new: &[u16]) -> Option<Vec<u16>> {
    let (old_pages, new_pages) = (pages(original), pages(new));
    if old_pages.len() != new_pages.len() {
        return None;
    }
    let (mut limit, page_lines) = window(original);
    if page_lines.is_none() {
        // no talk type: the window fits its own widest line
        let widest = old_pages.iter().flat_map(|p| lines(original, p.body)).map(|(a, b)| width(font, &original[a..b])).max().unwrap_or(0);
        limit = limit.max(widest);
    }
    let mut out: Vec<u16> = Vec::with_capacity(new.len() + 16);
    let mut changed = false;
    for (old, page) in old_pages.iter().zip(&new_pages) {
        let old_lines = lines(original, old.body);
        let new_lines = lines(new, page.body);
        let too_wide = old_lines.len() == new_lines.len()
            && new_lines.iter().zip(&old_lines).any(|(&(a, b), &(c, d))| {
                let w = width(font, &new[a..b]);
                w > limit && w > width(font, &original[c..d])
            });
        let body = &new[page.body.0..page.body.1];
        if !too_wide {
            out.extend_from_slice(body);
        } else {
            let per_page = page_lines.map_or(usize::MAX, |n| n.max(old_lines.len()));
            for (k, page_text) in paginate(font, &words(body), limit, per_page).iter().enumerate() {
                if k > 0 {
                    out.extend("$k$p".encode_utf16());
                }
                for (j, line) in page_text.iter().enumerate() {
                    if j > 0 {
                        out.push(NL);
                    }
                    out.extend_from_slice(line);
                }
            }
            changed = true;
        }
        out.extend_from_slice(&new[page.sep.0..page.sep.1]);
    }
    changed.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn short_names_change_nothing() {
        let font = Font::uniform(8);
        let old = w("$t1$Wmガンズ|3$w0|$Wsガンズ|$WaListen up!\norder from King Garon.$k");
        let new = w("$t1$Wmガンズ|3$w0|$Wsガンズ|$WaListen up!\norder from King Leo.$k");
        assert!(rewrap(&font, &old, &new).is_none());
    }

    #[test]
    fn long_line_wraps_into_a_new_page() {
        let font = Font::uniform(8); // 42 characters fit a line of 340
        let old = w("$t1$Wmガンズ|3$w0|$WaListen up, cannon fodder! We have a direct\norder from King Garon. He says kill 'em all!$k");
        let new = w("$t1$Wmガンズ|3$w0|$WaListen up, cannon fodder! We have a direct\norder from King Sumeragi. He says kill 'em all!$k");
        let out = String::from_utf16(&rewrap(&font, &old, &new).unwrap()).unwrap();
        assert!(out.ends_with("King Sumeragi.$k$pHe says kill 'em all!$k"), "{out}");
    }
}
