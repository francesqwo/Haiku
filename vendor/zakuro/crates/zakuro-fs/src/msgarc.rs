//! fanpatch: Fates' message archives (`m/**/*.bin.lz`, once LZ11 is off),
//! read and patched at the level of single messages.
//!
//! Layout (dev/supports/msgarc.py): a 0x20 header (file size, data size,
//! pointer count 0, label count), the data ("MESS_ARCHIVE_<name>\0", then
//! each text in UTF-16LE with a zero, 4-aligned), the label table (text
//! offset in the data, name offset) and the label names (Shift JIS,
//! zero-terminated). The game finds a message by its label (an IdentHash),
//! never by its place, so a label can be added at the end, or pointed at a
//! new text appended to the data, and the game reads the archive as before.
//!
//! What Haiku uses it for (haiku_fanpatch.rs), and how another language or
//! region can (eur2's message files):
//!
//! - [`Edits::between`]: what Haiku's own version of an archive (the builtin
//!   layer, `assets/builtin/romfs/m/@E/...`) changes in the game's: the
//!   labels it adds (Kana's `_HK` lines, the same-gender S-supports, the
//!   `_HKC/_HKS/_HKK` markers, the Sunglasses name...) and the lines it
//!   rewrites (End Turn).
//! - [`merge`]: those edits laid on any other version of the archive (a
//!   translation mod's, or the game's in another language), message by
//!   message: an added label goes in where that version lacks it; a
//!   rewritten line replaces that version's line only where it still reads
//!   as the game's (a translation's own wording of it stays); and, with
//!   `fill`, a label the game has that that version lacks gets the game's
//!   line, so the game never looks one up in vain.

/// one message: its label (Shift JIS bytes, as in the file) and its text
/// (UTF-16 units, without the zero).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub label: Vec<u8>,
    pub text: Vec<u16>,
}

impl Message {
    pub fn new(label: &[u8], text: &str) -> Message {
        Message { label: label.to_vec(), text: text.encode_utf16().collect() }
    }
}

fn u32_at(raw: &[u8], at: usize) -> Result<u32, String> {
    raw.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or_else(|| "the archive is cut short".to_string())
}

/// the parts of an archive.
struct Parts {
    header: Vec<u8>,
    data: Vec<u8>,
    table: Vec<(u32, u32)>,
    names: Vec<u8>,
}

fn split(raw: &[u8]) -> Result<Parts, String> {
    if raw.len() < 0x20 {
        return Err("not a message archive".into());
    }
    let data_size = u32_at(raw, 4)? as usize;
    let pointers = u32_at(raw, 8)?;
    let count = u32_at(raw, 12)? as usize;
    if pointers != 0 {
        return Err(format!("not a message archive ({pointers} pointers)"));
    }
    let table_at = 0x20usize.checked_add(data_size).ok_or("bad data size")?;
    let names_at = table_at.checked_add(count.checked_mul(8).ok_or("bad label count")?).ok_or("bad label count")?;
    if names_at > raw.len() {
        return Err("not a message archive (cut short)".into());
    }
    // the data starts with "MESS_ARCHIVE_<name>" in the game's; other tools
    // write other things there (Team If's: a text of its own), which the
    // game doesn't read
    let data = raw[0x20..table_at].to_vec();
    let mut table = Vec::with_capacity(count);
    for i in 0..count {
        table.push((u32_at(raw, table_at + i * 8)?, u32_at(raw, table_at + i * 8 + 4)?));
    }
    Ok(Parts { header: raw[..0x20].to_vec(), data, table, names: raw[names_at..].to_vec() })
}

impl Parts {
    fn label(&self, index: usize) -> Result<Vec<u8>, String> {
        let at = self.table[index].1 as usize;
        let rest = self.names.get(at..).ok_or("a label is outside the archive")?;
        let end = rest.iter().position(|&b| b == 0).ok_or("a label has no end")?;
        Ok(rest[..end].to_vec())
    }

    fn text(&self, index: usize) -> Result<Vec<u16>, String> {
        // the offset counts from the start of the data (0x20 in the file)
        let mut at = self.table[index].0 as usize;
        let mut text = Vec::new();
        loop {
            let unit = self.data.get(at..at + 2).ok_or("a text runs past the data")?;
            let unit = u16::from_le_bytes([unit[0], unit[1]]);
            if unit == 0 {
                return Ok(text);
            }
            text.push(unit);
            at += 2;
        }
    }

    fn append(&mut self, text: &[u16]) -> u32 {
        pad4(&mut self.data);
        let at = self.data.len() as u32;
        for unit in text {
            self.data.extend_from_slice(&unit.to_le_bytes());
        }
        self.data.extend_from_slice(&[0, 0]);
        at
    }

    fn build(mut self) -> Vec<u8> {
        pad4(&mut self.data);
        let mut out = self.header;
        out.extend_from_slice(&self.data);
        for (text, name) in &self.table {
            out.extend_from_slice(&text.to_le_bytes());
            out.extend_from_slice(&name.to_le_bytes());
        }
        out.extend_from_slice(&self.names);
        let size = out.len() as u32;
        let data_size = self.data.len() as u32;
        let count = self.table.len() as u32;
        out[0..4].copy_from_slice(&size.to_le_bytes());
        out[4..8].copy_from_slice(&data_size.to_le_bytes());
        out[8..12].copy_from_slice(&0u32.to_le_bytes());
        out[12..16].copy_from_slice(&count.to_le_bytes());
        out
    }
}

fn pad4(data: &mut Vec<u8>) {
    while data.len() % 4 != 0 {
        data.push(0);
    }
}

/// the messages of an (uncompressed) archive, in its order.
pub fn parse(raw: &[u8]) -> Result<Vec<Message>, String> {
    let parts = split(raw)?;
    (0..parts.table.len()).map(|i| Ok(Message { label: parts.label(i)?, text: parts.text(i)? })).collect()
}

/// the archive's name, after MESS_ARCHIVE_.
pub fn name(raw: &[u8]) -> Result<Vec<u8>, String> {
    let parts = split(raw)?;
    let end = parts.data.iter().position(|&b| b == 0).unwrap_or(parts.data.len());
    let first = &parts.data[..end];
    Ok(first.strip_prefix(b"MESS_ARCHIVE_".as_slice()).unwrap_or(first).to_vec())
}

/// the archive with these messages set: a label it has gets the new text
/// (appended, the label pointed at it; the old text stays, unread), one it
/// lacks is added at the end. Its other bytes are kept as they were.
pub fn set(raw: &[u8], messages: &[Message]) -> Result<Vec<u8>, String> {
    let mut parts = split(raw)?;
    let mut index: std::collections::HashMap<Vec<u8>, usize> = std::collections::HashMap::new();
    for i in 0..parts.table.len() {
        index.entry(parts.label(i)?).or_insert(i);
    }
    for message in messages {
        let at = parts.append(&message.text);
        match index.get(&message.label) {
            Some(&i) => parts.table[i].0 = at,
            None => {
                let name_at = parts.names.len() as u32;
                parts.names.extend_from_slice(&message.label);
                parts.names.push(0);
                parts.table.push((at, name_at));
                index.insert(message.label.clone(), parts.table.len() - 1);
            }
        }
    }
    Ok(parts.build())
}

/// the archive with each text passed through f (None: kept), and how many
/// texts f changed; the same bytes back when it changed none.
pub fn map_texts(raw: &[u8], f: impl Fn(&[u16]) -> Option<Vec<u16>>) -> Result<(Vec<u8>, usize), String> {
    let messages = parse(raw)?;
    let changed: Vec<Message> = messages
        .iter()
        .filter_map(|m| {
            let text = f(&m.text)?;
            (text != m.text).then(|| Message { label: m.label.clone(), text })
        })
        .collect();
    if changed.is_empty() {
        return Ok((raw.to_vec(), 0));
    }
    let count = changed.len();
    Ok((set(raw, &changed)?, count))
}

/// a new archive named name holding messages.
pub fn new(name: &[u8], messages: &[Message]) -> Vec<u8> {
    let mut data = b"MESS_ARCHIVE_".to_vec();
    data.extend_from_slice(name);
    data.push(0);
    pad4(&mut data);
    let parts = Parts { header: vec![0; 0x20], data, table: Vec::new(), names: Vec::new() };
    set(&parts.build(), messages).unwrap_or_default()
}

/// what one version of an archive changes in another (Haiku's own in the
/// game's): the labels it adds, and the lines it rewrites with the line
/// the game has there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Edits {
    pub added: Vec<Message>,
    /// (Haiku's message, the game's text of it)
    pub changed: Vec<(Message, Vec<u16>)>,
}

impl Edits {
    /// what `ours` changes in `game` (none: a file the game doesn't have,
    /// all of it added).
    pub fn between(game: Option<&[Message]>, ours: &[Message]) -> Edits {
        let game: std::collections::HashMap<&[u8], &[u16]> = game.unwrap_or(&[]).iter().map(|m| (m.label.as_slice(), m.text.as_slice())).collect();
        let mut edits = Edits::default();
        for message in ours {
            match game.get(message.label.as_slice()) {
                None => edits.added.push(message.clone()),
                Some(text) if *text != message.text.as_slice() => edits.changed.push((message.clone(), text.to_vec())),
                Some(_) => {}
            }
        }
        edits
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty()
    }
}

/// what [`merge`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Merged {
    /// the game's labels the other version lacked, given the game's line
    pub filled: usize,
    /// Haiku's labels added
    pub added: usize,
    /// Haiku's rewritten lines put in (that version had the game's line)
    pub changed: usize,
    /// Haiku's rewritten lines left out: that version has its own line
    pub kept: usize,
    /// Haiku's labels that version has already: its own stays
    pub present: usize,
    /// labels of that version the game doesn't have, left out (prune)
    pub pruned: usize,
}

impl Merged {
    pub fn any(&self) -> bool {
        self.filled + self.added + self.changed + self.pruned > 0
    }
}

/// Haiku's edits laid on `top` (another version of the archive, such as a
/// translation's), and, with `fill`, the game's lines for the labels the
/// game has and `top` lacks. With `prune` (and the game's archive known),
/// top's labels that neither the game nor Haiku has are left out: a
/// translation made for the Japanese game carries every line in three
/// talk tones and the Japan-only scenes, which the game never asks for and
/// which made its archives too big for the memory the US game gives them.
/// Gives top unchanged when there is nothing to do.
pub fn merge(top: &[u8], game: Option<&[Message]>, edits: &Edits, fill: bool, prune: bool) -> Result<(Vec<u8>, Merged), String> {
    let theirs = parse(top)?;
    let have: std::collections::HashMap<&[u8], &[u16]> = theirs.iter().map(|m| (m.label.as_slice(), m.text.as_slice())).collect();
    let mut merged = Merged::default();
    let mut set_list: Vec<Message> = Vec::new();
    let mut queued: std::collections::HashSet<Vec<u8>> = std::collections::HashSet::new();
    for message in &edits.added {
        if have.contains_key(message.label.as_slice()) {
            merged.present += 1;
        } else if queued.insert(message.label.clone()) {
            set_list.push(message.clone());
            merged.added += 1;
        }
    }
    for (message, game_text) in &edits.changed {
        match have.get(message.label.as_slice()) {
            // the line as the game has it: Haiku's rewrite goes in
            Some(text) if *text == game_text.as_slice() => {
                set_list.push(message.clone());
                merged.changed += 1;
            }
            // that version's own wording stays
            Some(_) => merged.kept += 1,
            // missing there: Haiku's line (rather than the game's)
            None => {
                if queued.insert(message.label.clone()) {
                    set_list.push(message.clone());
                    merged.changed += 1;
                }
            }
        }
    }
    if fill {
        for message in game.unwrap_or(&[]) {
            if !have.contains_key(message.label.as_slice()) && queued.insert(message.label.clone()) {
                set_list.push(message.clone());
                merged.filled += 1;
            }
        }
    }
    if let (true, Some(game)) = (prune, game) {
        let known: std::collections::HashSet<&[u8]> =
            game.iter().map(|m| m.label.as_slice()).chain(edits.added.iter().map(|m| m.label.as_slice())).chain(edits.changed.iter().map(|(m, _)| m.label.as_slice())).collect();
        let kept: Vec<Message> = theirs.iter().filter(|m| known.contains(m.label.as_slice())).cloned().collect();
        merged.pruned = theirs.len() - kept.len();
        if merged.pruned > 0 {
            let base = new(&name(top)?, &kept);
            return Ok((set(&base, &set_list)?, merged));
        }
    }
    if set_list.is_empty() {
        return Ok((top.to_vec(), merged));
    }
    Ok((set(top, &set_list)?, merged))
}

/// tones: the label Corrin's talk tone `tone` (1 to 3) gives where the game
/// asks for tone 1 (`<base>_PCM1` -> `<base>_PCM<tone>`, `_PCF1` likewise);
/// none for a label without tone 1's suffix.
pub fn tone_label(label: &[u8], tone: u8) -> Option<Vec<u8>> {
    let base = label.strip_suffix(b"_PCM1".as_slice()).map(|base| (base, b'M')).or_else(|| label.strip_suffix(b"_PCF1".as_slice()).map(|base| (base, b'F')))?;
    let mut out = base.0.to_vec();
    out.extend_from_slice(&[b'_', b'P', b'C', base.1, b'0' + tone]);
    Some(out)
}

/// tones: an archive with Corrin's lines in speaking style `tone` (2 or 3)
/// put where the game reads them. The US game only ever asks for tone 1
/// (TalkUtil::GetTalkTone's tone is the constant 0 there), and the
/// Japanese game's other tones are whole scenes that would not fit in the
/// memory the US game gives its text; so each `_PCM1`/`_PCF1` message of
/// `merged` (the archive as Haiku lays it, Japanese tones pruned) gets the
/// text of its `_PCM<tone>`/`_PCF<tone>` sibling in `theirs` (the
/// translation's own archive, all three tones), and nothing else changes.
/// None when no line differs. The archive is rebuilt compactly (texts no
/// label points at are left out).
pub fn tone_variant(merged: &[u8], theirs: &[Message], tone: u8) -> Result<Option<Vec<u8>>, String> {
    let wanted: std::collections::HashMap<&[u8], &[u16]> = theirs.iter().map(|m| (m.label.as_slice(), m.text.as_slice())).collect();
    let mut messages = parse(merged)?;
    let mut changed = 0;
    for message in &mut messages {
        let Some(label) = tone_label(&message.label, tone) else { continue };
        if let Some(text) = wanted.get(label.as_slice()) {
            if *text != message.text.as_slice() {
                message.text = text.to_vec();
                changed += 1;
            }
        }
    }
    if changed == 0 {
        return Ok(None);
    }
    Ok(Some(new(&name(merged)?, &messages)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(m: &[Message], label: &[u8]) -> Option<String> {
        m.iter().find(|m| m.label == label).map(|m| String::from_utf16_lossy(&m.text))
    }

    #[test]
    fn archives_are_read_set_and_merged() {
        let game = new(b"ResidentMenu", &[Message::new(b"MID_MENU_END", "End"), Message::new(b"MID_A", "A"), Message::new(b"MID_US_ONLY", "US")]);
        let ours = set(&game, &[Message::new(b"MID_MENU_END", "End Turn"), Message::new(b"MID_HAIKU_X", "Haiku")]).unwrap();
        assert_eq!(name(&ours).unwrap(), b"ResidentMenu");
        let game_m = parse(&game).unwrap();
        let ours_m = parse(&ours).unwrap();
        assert_eq!(text(&ours_m, b"MID_MENU_END").as_deref(), Some("End Turn"));
        assert_eq!(text(&ours_m, b"MID_A").as_deref(), Some("A"));
        let edits = Edits::between(Some(&game_m), &ours_m);
        assert_eq!(edits.added.len(), 1);
        assert_eq!(edits.changed.len(), 1);

        // a translation that kept the game's "End", lacks MID_US_ONLY
        let mod_a = new(b"ResidentMenu", &[Message::new(b"MID_MENU_END", "End"), Message::new(b"MID_A", "Ah"), Message::new(b"MID_JP_ONLY", "JP")]);
        let (out, merged) = merge(&mod_a, Some(&game_m), &edits, true, false).unwrap();
        let out = parse(&out).unwrap();
        assert_eq!(text(&out, b"MID_MENU_END").as_deref(), Some("End Turn"));
        assert_eq!(text(&out, b"MID_A").as_deref(), Some("Ah"), "the mod's own lines stay");
        assert_eq!(text(&out, b"MID_HAIKU_X").as_deref(), Some("Haiku"));
        assert_eq!(text(&out, b"MID_US_ONLY").as_deref(), Some("US"), "filled from the game");
        assert_eq!(text(&out, b"MID_JP_ONLY").as_deref(), Some("JP"));
        assert_eq!(merged, Merged { filled: 1, added: 1, changed: 1, kept: 0, present: 0, pruned: 0 });
        // pruned: the label the game doesn't have goes
        let (out, merged) = merge(&mod_a, Some(&game_m), &edits, true, true).unwrap();
        let out = parse(&out).unwrap();
        assert_eq!(text(&out, b"MID_JP_ONLY"), None);
        assert_eq!(text(&out, b"MID_A").as_deref(), Some("Ah"));
        assert_eq!(text(&out, b"MID_HAIKU_X").as_deref(), Some("Haiku"));
        assert_eq!(text(&out, b"MID_MENU_END").as_deref(), Some("End Turn"));
        assert_eq!(merged.pruned, 1);

        // one that words End its own way keeps it
        let mod_b = new(b"ResidentMenu", &[Message::new(b"MID_MENU_END", "Finish"), Message::new(b"MID_A", "A"), Message::new(b"MID_US_ONLY", "US")]);
        let (out, merged) = merge(&mod_b, Some(&game_m), &edits, true, false).unwrap();
        assert_eq!(text(&parse(&out).unwrap(), b"MID_MENU_END").as_deref(), Some("Finish"));
        assert_eq!(merged.kept, 1);

        // tones: tone 2 and 3 of a translation put in tone 1's place
        let jp = new(
            b"A001",
            &[
                Message::new(b"MID_A_PCM1", "one"),
                Message::new(b"MID_A_PCM2", "two"),
                Message::new(b"MID_A_PCM3", "three"),
                Message::new(b"MID_A_PCF1", "une"),
                Message::new(b"MID_A_PCF3", "une"),
                Message::new(b"MID_B", "plain"),
            ],
        );
        let us = new(b"A001", &[Message::new(b"MID_A_PCM1", "one"), Message::new(b"MID_A_PCF1", "une"), Message::new(b"MID_B", "plain")]);
        let us_m = parse(&us).unwrap();
        let (pruned, _) = merge(&jp, Some(&us_m), &Edits::default(), true, true).unwrap();
        let theirs = parse(&jp).unwrap();
        let two = parse(&tone_variant(&pruned, &theirs, 2).unwrap().unwrap()).unwrap();
        assert_eq!(text(&two, b"MID_A_PCM1").as_deref(), Some("two"));
        assert_eq!(text(&two, b"MID_A_PCF1").as_deref(), Some("une"), "no tone 2: tone 1's line");
        assert_eq!(text(&two, b"MID_B").as_deref(), Some("plain"));
        assert_eq!(text(&two, b"MID_A_PCM2"), None, "no new labels");
        let three = parse(&tone_variant(&pruned, &theirs, 3).unwrap().unwrap()).unwrap();
        assert_eq!(text(&three, b"MID_A_PCM1").as_deref(), Some("three"));
        assert_eq!(name(&tone_variant(&pruned, &theirs, 3).unwrap().unwrap()).unwrap(), b"A001");
        // the same text in every tone: nothing to make
        assert_eq!(tone_variant(&us, &us_m, 2).unwrap(), None);
        assert_eq!(tone_label(b"MID_X_PCF1", 3).as_deref(), Some(b"MID_X_PCF3".as_slice()));
        assert_eq!(tone_label(b"MID_X_PCF0", 3), None);

        // nothing to do: the same bytes back
        let (same, merged) = merge(&ours, Some(&game_m), &edits, true, false).unwrap();
        assert_eq!(same, ours);
        assert!(!merged.any());
    }
}
