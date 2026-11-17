//! cpu: static coverage audit of a title's executable. it analyzes the code
//! as a build does (seeds, overrides' addresses aside, hints) and then looks
//! for every way execution can get somewhere indirectly, reporting the
//! places that are not entry points of the recompiled code:
//!
//! - words in .rodata, .data and the literal pools of .text that point into
//!   the code (vtables, callback and state tables, function pointers),
//!   by run (vtable-like runs of pointers) and alone;
//! - addresses computed by `add/sub rd, pc, #imm` (ARM) and `adr` (Thumb);
//! - the indirect jumps that are not returns or calls (computed gotos);
//! - names from a symbol table (`address<TAB>name`) and Ghidra's function
//!   list (`address<TAB>mode<TAB>instructions`) without an entry.
//!
//! usage: fates_coverage <rom> <out dir> [symbols.tsv] [function-modes.tsv]
//! writes <out dir>/audit.txt and <out dir>/entered.txt, the places that
//! should be entries, one `missing ADDR` line each (odd for Thumb), for
//! dev/cpu/merge_hints.py.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use recomp3ds::discover::{Analysis, Byte, Mode};
use recomp3ds::image::{Image, Segment};
use recomp3ds::rom::Title;
use recomp3ds::{build, discover, Mods};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        return Err("usage: fates_coverage <rom> <out dir> [symbols.tsv] [function-modes.tsv]".into());
    }
    let title = Title::load(&args[1]).map_err(|e| format!("{e:?}"))?;
    let out = Path::new(&args[2]);
    std::fs::create_dir_all(out)?;
    let mut programs = recomp3ds::programs(&title, Mods::default()).map_err(|e| format!("{e:?}"))?;
    // the seeds a build adds first: what overrides (FATES_OVERRIDES, a
    // folder) and the host's C library functions replace
    let mut replaced: Vec<u32> = recomp3ds::natives::find(&programs[0].1.text).iter().map(|n| n.address).collect();
    if let Some(dir) = std::env::var_os("FATES_OVERRIDES") {
        for file in recomp3ds::overrides::load(Path::new(&dir))? {
            replaced.extend(file.overrides.iter().filter(|o| o.module.is_none()).map(|o| o.address));
        }
    }
    for &address in &replaced {
        programs[0].1.seeds.insert(0, (address, discover::Source::Override));
    }
    let mut analyses: Vec<Analysis> = programs.iter().map(|(_, program)| discover::analyze(program)).collect();
    let hinted = build::apply_hints(&title, &mut programs, &mut analyses);
    let image = Image::from_title(&title).map_err(|e| format!("{e:?}"))?;
    let (program, analysis) = (&programs[0].1, &analyses[0]);
    let text = &program.text;
    let mut report = String::new();
    writeln!(report, "# static coverage audit of {} ({} modules)", args[1], programs.len() - 1)?;
    writeln!(report, "# {} functions, {} hinted places start functions", analysis.functions.len(), hinted)?;
    writeln!(
        report,
        "# discovery: {} jump tables, {} indirect sites, {} jumps through pc-computed addresses, {} labels at pc-computed addresses, {} labels at pointers",
        analysis.jump_tables, analysis.indirect_sites, analysis.computed_jumps, analysis.computed_labels, analysis.pointer_labels
    )?;

    // what the generated code has entries for: every label, odd for Thumb,
    // and which instructions each mode holds
    let mut entries = BTreeSet::new();
    let mut held = BTreeSet::new();
    for function in analysis.functions.values() {
        let t = (function.mode == Mode::Thumb) as u32;
        entries.extend(function.labels.iter().map(|&a| a | t));
        held.extend(function.instructions.iter().map(|&a| a | t));
    }
    writeln!(report, "# {} entries, {} instructions held", entries.len(), held.len())?;
    // in the form entries.c has, for the scripts in dev/cpu
    std::fs::write(
        out.join("entries.c"),
        entries.iter().map(|e| format!("{{0x{e:08X}u, {}_x}},
", if e & 1 != 0 { "t" } else { "f" })).collect::<String>(),
    )?;
    let byte = |a: u32| analysis.map[(a - text.base) as usize];

    let symbols = read_symbols(args.get(3));
    let modes = read_modes(args.get(4));
    let name = |a: u32| symbols.get(&(a & !1)).cloned().unwrap_or_default();

    // a place is what a pointer to it would be: odd for Thumb
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    enum Class {
        Entry,
        MidInstruction,
        HeldNotLabel,
        OtherModeOnly,
        Literal,
        Unknown,
    }
    let classify = |v: u32| -> Class {
        let a = v & !1;
        if entries.contains(&v) {
            Class::Entry
        } else if held.contains(&v) {
            Class::HeldNotLabel
        } else if held.contains(&(a | (1 - (v & 1)))) {
            Class::OtherModeOnly
        } else if byte(a) == Byte::Literal {
            Class::Literal
        } else if byte(a) == Byte::Code {
            Class::MidInstruction
        } else {
            Class::Unknown
        }
    };
    let plausible = |v: u32| v & 1 == 1 || v & 3 == 0;

    let mut entered: BTreeSet<u32> = BTreeSet::new();

    // 1. pointer-like words in data and literal pools
    let literal_words: Vec<(u32, u32)> =
        text.words().filter(|&(a, _)| byte(a) == Byte::Literal).collect();
    for (label, words) in [
        ("rodata", image.rodata.words().collect::<Vec<_>>()),
        ("data", image.data.words().collect::<Vec<_>>()),
        ("text literal pools", literal_words),
    ] {
        let pointers: Vec<(u32, u32)> =
            words.iter().copied().filter(|&(_, v)| text.contains(v & !1) && plausible(v)).collect();
        let mut by_class: BTreeMap<Class, Vec<(u32, u32)>> = BTreeMap::new();
        for &(at, v) in &pointers {
            by_class.entry(classify(v)).or_default().push((at, v));
        }
        // runs of two or more code pointers next to each other: vtables,
        // tables of callbacks and states
        let in_runs = runs(&pointers);
        writeln!(report, "\n## pointers in {label}: {} words point into the code, {} of them in runs of 2+", pointers.len(), in_runs.len())?;
        for (class, list) in &by_class {
            let run_count = list.iter().filter(|(at, _)| in_runs.contains(at)).count();
            let named = list.iter().filter(|(_, v)| symbols.contains_key(&(v & !1))).count();
            writeln!(report, "{class:?}: {} ({} in runs, {} at a symbol)", list.len(), run_count, named)?;
        }
        for (class, list) in &by_class {
            if *class == Class::Entry {
                continue;
            }
            for &(at, v) in list.iter().take(400) {
                let in_run = in_runs.contains(&at);
                writeln!(report, "  {class:?} {at:08X} -> {v:08X}{} {}", if in_run { " run" } else { "" }, name(v))?;
            }
            // what should be entries: a pointer to an instruction held in
            // its mode, or one in a run or at a symbol whose bytes decode
            for &(at, v) in list {
                let strong = in_runs.contains(&at) || symbols.contains_key(&(v & !1)) || modes.contains_key(&(v & !1));
                match class {
                    Class::HeldNotLabel => {
                        entered.insert(v);
                    }
                    Class::Unknown | Class::OtherModeOnly if strong && looks_like_code(text, v) => {
                        entered.insert(v);
                    }
                    _ => {}
                }
            }
        }
    }

    // 2. addresses the code computes from the pc
    let mut computed: BTreeMap<Class, Vec<(u32, u32)>> = BTreeMap::new();
    for function in analysis.functions.values() {
        for &a in &function.instructions {
            let target = match function.mode {
                Mode::Arm => text.read32(a).and_then(|w| adr_arm(w, a)),
                Mode::Thumb => text.read16(a).and_then(|h| adr_thumb(h, a)),
            };
            let Some(target) = target.filter(|t| text.contains(*t)) else { continue };
            // an address in a data pool is data; only code is of interest
            let c = classify(target);
            let c_thumb = classify(target | 1);
            let best = c.min(c_thumb);
            computed.entry(best).or_default().push((a, target));
        }
    }
    writeln!(report, "\n## addresses computed from the pc (add/sub rd, pc / adr)")?;
    for (class, list) in &computed {
        writeln!(report, "{class:?}: {}", list.len())?;
    }
    for (class, list) in &computed {
        if matches!(class, Class::Entry | Class::Literal) {
            continue;
        }
        for &(a, t) in list.iter().take(200) {
            writeln!(report, "  {class:?} at {a:08X} -> {t:08X} {}", name(t))?;
        }
    }

    // 3. indirect jumps that are not returns or calls
    writeln!(report, "\n## computed jumps (bx/mov pc/ldr pc that are not returns or calls)")?;
    let mut jumps = 0;
    for function in analysis.functions.values() {
        for &a in &function.instructions {
            let kind = match function.mode {
                Mode::Arm => text.read32(a).and_then(computed_jump_arm),
                Mode::Thumb => text.read16(a).and_then(computed_jump_thumb),
            };
            if let Some(kind) = kind {
                jumps += 1;
                if jumps <= 100_000 {
                    let entry = function.instructions.first().copied().unwrap_or(a);
                    writeln!(report, "  {a:08X} {kind} in {} {}", entry, name(nearest(&symbols, a)))?;
                }
            }
        }
    }
    writeln!(report, "{jumps} computed jumps")?;

    // 3b. functions that return through an lr they changed without saving
    // it first: a return to an offset from where they were called
    writeln!(report, "
## ARM functions that return through an lr they changed and never saved")?;
    let mut offset_returns = 0;
    for (&entry, function) in &analysis.functions {
        if function.mode != Mode::Arm {
            continue;
        }
        let (mut saves, mut changes, mut returns) = (false, Vec::new(), false);
        for &a in &function.instructions {
            let Some(w) = text.read32(a) else { continue };
            if w >> 28 == 0xF {
                continue;
            }
            let (rd, rn) = ((w >> 12) & 0xF, (w >> 16) & 0xF);
            // push {.., lr} / str lr, [sp, ...]
            if (w & 0x0FFF_0000 == 0x092D_0000 && w & 0x4000 != 0) || (w & 0x0C10_0000 == 0x0400_0000 && rd == 14 && rn == 13) {
                saves = true;
            }
            if w & 0x0FFF_FFFF == 0x012F_FF1E || w & 0x0FFF_FFFF == 0x01A0_F00E {
                returns = true;
            }
            let bl = w & 0x0F00_0000 == 0x0B00_0000 || w & 0x0FFF_FFF0 == 0x012F_FF30;
            let mov_lr_pc = w & 0x0FEF_FFFF == 0x01A0_E00F || w & 0x0FEF_F000 == 0x028F_E000 || w & 0x0FEF_F000 == 0x024F_E000;
            let writes_lr = (w & 0x0C00_0000 == 0 && rd == 14 && w & 0x0190_0000 != 0x0100_0000)
                || (w & 0x0C10_0000 == 0x0410_0000 && rd == 14)
                || (w & 0x0C00_0000 == 0x0400_0000 && rn == 14 && (w & 0x0100_0000 == 0 || w & 0x0020_0000 != 0))
                || (w & 0x0E10_0000 == 0x0810_0000 && w & 0x4000 != 0 && rn != 13);
            if writes_lr && !bl && !mov_lr_pc {
                changes.push(a);
            }
        }
        if returns && !saves && !changes.is_empty() {
            offset_returns += 1;
            writeln!(report, "  {entry:08X} {} lr changed at {:08X?}", name(entry), &changes[..changes.len().min(4)])?;
        }
    }
    writeln!(report, "{offset_returns} functions")?;

    // 4. known functions without an entry
    let mut known_missing = Vec::new();
    for (&a, &(thumb, n)) in &modes {
        let v = a | thumb as u32;
        if text.contains(a) && !entries.contains(&v) && n > 0 && looks_like_code(text, v) {
            known_missing.push(v);
        }
    }
    for &a in symbols.keys() {
        if text.contains(a) && !modes.contains_key(&a) && !entries.contains(&a) && !entries.contains(&(a | 1)) {
            let v = if a & 2 != 0 { a | 1 } else { a };
            if looks_like_code(text, v) {
                known_missing.push(v);
            }
        }
    }
    writeln!(report, "\n## known functions without an entry whose bytes decode: {}", known_missing.len())?;
    for &v in known_missing.iter().take(200) {
        writeln!(report, "  {v:08X} {:?} {}", classify(v), name(v))?;
    }
    entered.extend(known_missing);

    entered.retain(|&v| !entries.contains(&v));
    writeln!(report, "\n# {} places should become entries (written to entered.txt)", entered.len())?;
    std::fs::write(out.join("audit.txt"), report)?;
    std::fs::write(out.join("entered.txt"), entered.iter().map(|v| format!("missing {v:08X}\n")).collect::<String>())?;
    println!("wrote {}", out.join("audit.txt").display());
    Ok(())
}

/// the words of pointers that sit in runs of two or more consecutive words
/// all pointing into the code.
fn runs(pointers: &[(u32, u32)]) -> BTreeSet<u32> {
    let mut out = BTreeSet::new();
    for pair in pointers.windows(2) {
        if pair[1].0 == pair[0].0 + 4 {
            out.insert(pair[0].0);
            out.insert(pair[1].0);
        }
    }
    out
}

/// whether the bytes at v (odd for Thumb) start like code a compiler made:
/// ARM whose first words are unconditional and not zero, or a Thumb push or
/// a common first instruction.
fn looks_like_code(text: &Segment, v: u32) -> bool {
    let a = v & !1;
    if v & 1 == 0 {
        let words: Vec<u32> = (0..4).filter_map(|i| text.read32(a + 4 * i)).collect();
        if words.len() < 4 || words.contains(&0) {
            return false;
        }
        words[0] & 0xFFFF_0000 == 0xE92D_0000 || (words[0] >> 28 == 0xE && words.iter().filter(|w| *w >> 28 == 0xE).count() >= 3)
    } else {
        let Some(h) = text.read16(a) else { return false };
        // push, add sp, mov/ldr/str low registers, bx lr, bl prefix
        h & 0xFE00 == 0xB400 || h & 0xFF00 == 0xB000 || h == 0x4770 || h & 0xF800 == 0xF000 || h & 0xF800 == 0x2000 || h & 0xF800 == 0x6800 || h & 0xFFC0 == 0x1C00 || h & 0xFF00 == 0x4600
    }
}

/// add rd, pc, #imm / sub rd, pc, #imm (rd not pc), the address it makes.
fn adr_arm(w: u32, a: u32) -> Option<u32> {
    if w >> 28 == 0xF || (w >> 12) & 0xF == 15 {
        return None;
    }
    let imm = || (w & 0xFF).rotate_right(((w >> 8) & 0xF) * 2);
    match w & 0x0FEF_0000 {
        0x028F_0000 => Some(a.wrapping_add(8).wrapping_add(imm())),
        0x024F_0000 => Some(a.wrapping_add(8).wrapping_sub(imm())),
        _ => None,
    }
}

/// adr rd, label (Thumb add rd, pc, #imm8*4).
fn adr_thumb(h: u16, a: u32) -> Option<u32> {
    (h & 0xF800 == 0xA000).then(|| ((a + 4) & !3) + (h as u32 & 0xFF) * 4)
}

/// an ARM write to pc that is neither a return nor a call: bx rN (not lr),
/// mov pc, rN (not lr), ldr pc, [rN, ...] (not the stack), add pc, ...
fn computed_jump_arm(w: u32) -> Option<&'static str> {
    if w >> 28 == 0xF {
        return None;
    }
    if w & 0x0FFF_FFF0 == 0x012F_FF10 && w & 0xF != 14 {
        return Some("bx");
    }
    if w & 0x0DEF_F000 == 0x01A0_F000 && w & 0x0200_0000 == 0 && w & 0xF != 14 && w & 0xFF0 == 0 {
        return Some("mov pc");
    }
    // ldr pc, [rn, ...], rn not sp and not pc (literal or table)
    if w & 0x0C10_F000 == 0x0410_F000 && (w >> 16) & 0xF != 13 && (w >> 16) & 0xF != 15 {
        return Some("ldr pc");
    }
    // ldm rn, {.., pc} with rn not sp
    if w & 0x0E10_8000 == 0x0810_8000 && (w >> 16) & 0xF != 13 {
        return Some("ldm pc");
    }
    if w & 0x0DE0_F000 == 0x0080_F000 && (w >> 16) & 0xF == 15 && w & 0x0200_0000 == 0 {
        return Some("add pc (table)");
    }
    None
}

/// a Thumb bx rN (not lr) or mov pc, rN.
fn computed_jump_thumb(h: u16) -> Option<&'static str> {
    if h & 0xFF87 == 0x4700 && (h >> 3) & 0xF != 14 {
        return Some("bx");
    }
    if h & 0xFF87 == 0x4687 && (h >> 3) & 0xF != 14 {
        return Some("mov pc");
    }
    None
}

fn nearest(symbols: &BTreeMap<u32, String>, a: u32) -> u32 {
    symbols.range(..=a).next_back().map_or(a, |(&s, _)| s)
}

fn read_symbols(path: Option<&String>) -> BTreeMap<u32, String> {
    let Some(path) = path else { return BTreeMap::new() };
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .skip(1)
        .filter_map(|l| {
            let (a, n) = l.split_once('\t')?;
            Some((u32::from_str_radix(a.trim_start_matches("0x"), 16).ok()?, n.to_owned()))
        })
        .collect()
}

fn read_modes(path: Option<&String>) -> BTreeMap<u32, (bool, usize)> {
    let Some(path) = path else { return BTreeMap::new() };
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .skip(1)
        .filter_map(|l| {
            let mut f = l.split('\t');
            let a: u32 = f.next()?.parse().ok()?;
            let thumb = f.next()? == "Thumb";
            let n: usize = f.next()?.parse().ok()?;
            Some((a, (thumb, n)))
        })
        .collect()
}
