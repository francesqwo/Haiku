//! playtest: headless tools for driving the game deeper than the replays
//! alone reach, and for reading what it is doing.
//!
//! - `FATES_PEEK=<frames>:<addr>:<len>[,...]` prints guest memory as hex at
//!   the end of those frames.
//! - `FATES_POKE=<frames>:<addr>=<value>[/<size>][,...]` writes guest memory
//!   before those frames run. `<size>` is 1, 2 or 4 bytes (4 by default).
//! - `FATES_UNITS=<frames>` prints the unit pool's units on the map (index,
//!   person id, class, level, HP, position, force) at the end of those
//!   frames, which is what a replay needs to plan moves.
//!
//! `<frames>` is one frame `N`, a range `A-B` (every frame) or `A-B/K`
//! (every K-th). `<addr>` is hex, followed by any of `@` (read the word
//! there: follow a pointer) and `+hex`, e.g. `6d7ed0@+18` is the byte the
//! map's `Situation` keeps its result in (`map::Situation::SetComplete`,
//! 0x003A6054, writes 9 there for a win, see dev/reports/playtest.md).
//!
//! These are debugging tools for testing; a player never sees them.

use zakuro_core::memory::Memory;

#[derive(Debug, Clone, Copy)]
struct Frames {
    first: u64,
    last: u64,
    step: u64,
}

impl Frames {
    fn parse(spec: &str) -> Option<Frames> {
        let (range, step) = match spec.split_once('/') {
            Some((range, step)) => (range, step.parse().ok()?),
            None => (spec, 1),
        };
        let (first, last) = match range.split_once('-') {
            Some((a, b)) => (a.parse().ok()?, b.parse().ok()?),
            None => {
                let at = range.parse().ok()?;
                (at, at)
            }
        };
        Some(Frames { first, last, step: u64::max(step, 1) })
    }

    fn has(&self, frame: u64) -> bool {
        frame >= self.first && frame <= self.last && (frame - self.first) % self.step == 0
    }
}

fn hex(text: &str) -> Option<u32> {
    u32::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok()
}

/// an address: hex, then `@` (dereference) and `+hex` in any order.
fn resolve(memory: &mut Memory, expr: &str) -> Option<u32> {
    let mut rest = expr.trim();
    let end = rest.find(['@', '+']).unwrap_or(rest.len());
    let mut addr = hex(&rest[..end])?;
    rest = &rest[end..];
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('@') {
            if !memory.is_readable(addr, 4) {
                return None;
            }
            let mut word = [0u8; 4];
            memory.read_bytes(addr, &mut word);
            addr = u32::from_le_bytes(word);
            // a null pointer: the object is not there (yet)
            if addr == 0 {
                return None;
            }
            rest = after;
        } else if let Some(after) = rest.strip_prefix('+') {
            let end = after.find(['@', '+']).unwrap_or(after.len());
            addr = addr.wrapping_add(hex(&after[..end])?);
            rest = &after[end..];
        } else {
            return None;
        }
    }
    Some(addr)
}

struct Peek {
    frames: Frames,
    addr: String,
    len: usize,
}

struct Poke {
    frames: Frames,
    addr: String,
    value: u32,
    size: usize,
}

/// the tools a run asked for.
pub struct Playtest {
    peeks: Vec<Peek>,
    pokes: Vec<Poke>,
    units: Vec<Frames>,
}

impl Playtest {
    pub fn from_env() -> Option<Playtest> {
        let list = |name: &str| std::env::var(name).unwrap_or_default();
        let mut peeks = Vec::new();
        for spec in list("FATES_PEEK").split(',').filter(|s| !s.trim().is_empty()) {
            let mut parts = spec.trim().splitn(3, ':');
            let parsed = (|| Some(Peek { frames: Frames::parse(parts.next()?)?, addr: parts.next()?.to_owned(), len: parts.next()?.parse().ok()? }))();
            match parsed {
                Some(peek) => peeks.push(peek),
                None => eprintln!("playtest: FATES_PEEK: could not read {spec}"),
            }
        }
        let mut pokes = Vec::new();
        for spec in list("FATES_POKE").split(',').filter(|s| !s.trim().is_empty()) {
            let parsed = (|| {
                let (frames, rest) = spec.trim().split_once(':')?;
                let (addr, value) = rest.split_once('=')?;
                let (value, size) = match value.split_once('/') {
                    Some((value, size)) => (value, size.parse().ok()?),
                    None => (value, 4),
                };
                Some(Poke { frames: Frames::parse(frames)?, addr: addr.to_owned(), value: hex(value)?, size })
            })();
            match parsed {
                Some(poke) => pokes.push(poke),
                None => eprintln!("playtest: FATES_POKE: could not read {spec}"),
            }
        }
        let units: Vec<Frames> = list("FATES_UNITS").split(',').filter_map(|s| Frames::parse(s.trim())).collect();
        (!peeks.is_empty() || !pokes.is_empty() || !units.is_empty()).then_some(Playtest { peeks, pokes, units })
    }

    /// before frame `frame` runs.
    pub fn before(&mut self, memory: &mut Memory, frame: u64) {
        for poke in self.pokes.iter().filter(|poke| poke.frames.has(frame)) {
            match resolve(memory, &poke.addr) {
                Some(addr) => {
                    let bytes = poke.value.to_le_bytes();
                    memory.write_bytes(addr, &bytes[..poke.size.clamp(1, 4)]);
                    if poke.frames.first == poke.frames.last {
                        println!("playtest: frame {frame}: wrote {:#x} to {addr:08X} ({})", poke.value, poke.addr);
                    }
                }
                None if poke.frames.first == poke.frames.last => println!("playtest: frame {frame}: {} does not resolve", poke.addr),
                None => {}
            }
        }
    }

    /// after frame `frame` ran.
    pub fn after(&mut self, memory: &mut Memory, frame: u64) {
        for peek in self.peeks.iter().filter(|peek| peek.frames.has(frame)) {
            let Some(addr) = resolve(memory, &peek.addr) else {
                println!("peek {frame} {}: does not resolve", peek.addr);
                continue;
            };
            let mut bytes = vec![0u8; peek.len];
            memory.read_bytes(addr, &mut bytes);
            for (row, chunk) in bytes.chunks(16).enumerate() {
                let words: Vec<String> = chunk.chunks(4).map(|w| w.iter().rev().map(|b| format!("{b:02x}")).collect()).collect();
                let text: String = chunk.iter().map(|&b| if b.is_ascii_graphic() { b as char } else { '.' }).collect();
                println!("peek {frame} {:08X}: {}  {text}", addr as usize + row * 16, words.join(" "));
            }
        }
        if self.units.iter().any(|frames| frames.has(frame)) {
            units(memory, frame);
        }
    }
}

/// UnitPool::Get's pool (0x140 bytes a unit), Force::Get's forces (12
/// bytes each at *0x006D9F44: the first unit, units linked at +0x14), the
/// map cursor and the map's image (a byte per tile, the unit's pool index).
const UNIT_POOL: u32 = 0x006D_9F40;
const UNIT_SIZE: u32 = 0x140;
const FORCES: u32 = 0x006D_9F44;
const MAP_CURSOR: u32 = 0x006D_6EC4;
const MAP_IMAGE: u32 = 0x006D_73E8;

fn word(memory: &mut Memory, addr: u32) -> u32 {
    if addr == 0 || !memory.is_readable(addr, 4) {
        return 0;
    }
    let mut bytes = [0u8; 4];
    memory.read_bytes(addr, &mut bytes);
    u32::from_le_bytes(bytes)
}

/// the units on the map by force, with their tiles from the map's image,
/// and the map cursor's tile.
fn units(memory: &mut Memory, frame: u64) {
    let pool = word(memory, UNIT_POOL);
    let forces = word(memory, FORCES);
    if pool == 0 || forces == 0 {
        println!("units {frame}: no unit pool");
        return;
    }
    let mut tiles: std::collections::HashMap<u32, (u32, u32)> = std::collections::HashMap::new();
    let image = word(memory, MAP_IMAGE);
    if image != 0 && memory.is_readable(image, 8 + 1024) {
        let mut grid = [0u8; 1024];
        memory.read_bytes(image + 8, &mut grid);
        for (at, index) in grid.iter().enumerate() {
            if *index != 0 {
                tiles.insert(*index as u32, (at as u32 & 31, at as u32 >> 5));
            }
        }
    }
    let cursor = word(memory, MAP_CURSOR);
    if cursor != 0 {
        let x = f32::from_bits(word(memory, cursor + 0x18));
        let y = f32::from_bits(word(memory, cursor + 0x1C));
        println!("units {frame}: cursor at {x},{y}");
    }
    for force in 0..4u32 {
        let mut unit = word(memory, forces + force * 12);
        let mut seen = 0;
        while unit != 0 && seen < 128 {
            seen += 1;
            let index = (unit.wrapping_sub(pool)) / UNIT_SIZE + 1;
            let person = word(memory, unit + 0x9C);
            let name = person_name(memory, person);
            let mut raw = [0u8; 0x40];
            memory.read_bytes(unit + 0xC0, &mut raw);
            let tile = tiles.get(&index).map(|(x, y)| format!("{x},{y}")).unwrap_or_else(|| "-".to_owned());
            let hex: String = raw.iter().map(|b| format!("{b:02x}")).collect();
            let flags = word(memory, unit + 8);
            println!("unit {frame} force{force} #{index:03} {name:<16} tile {tile:<6} flags {flags:08x} c0: {hex}");
            unit = word(memory, unit + 0x14);
        }
    }
}

fn person_name(memory: &mut Memory, person: u32) -> String {
    if person == 0 || !memory.is_readable(person, 16) {
        return "-".to_owned();
    }
    // the record starts with pointers to its id ("PID_...") strings
    for at in [0u32, 4, 8, 12, 16, 20] {
        let mut word = [0u8; 4];
        memory.read_bytes(person + at, &mut word);
        let ptr = u32::from_le_bytes(word);
        if ptr != 0 && memory.is_readable(ptr, 16) {
            let mut text = [0u8; 24];
            memory.read_bytes(ptr, &mut text);
            if text.starts_with(b"PID_") {
                let end = text.iter().position(|b| *b == 0).unwrap_or(text.len());
                // amiibo: Shift-JIS bytes (PID_マルス) as \xNN, so the name can be read back
                return text[..end].iter().map(|&b| if b.is_ascii_graphic() { (b as char).to_string() } else { format!("\\x{b:02x}") }).collect();
            }
        }
    }
    format!("person@{person:08X}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_read_one_a_range_and_a_step() {
        let one = Frames::parse("100").unwrap();
        assert!(one.has(100) && !one.has(101));
        let every = Frames::parse("100-110/5").unwrap();
        assert!(every.has(105) && !every.has(104) && every.has(110) && !every.has(115));
    }
}
