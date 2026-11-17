//! coverage: every gap in the recompiled code of a title's executable, the
//! places the generated code goes to directly (falling through, branching,
//! returning from a call) that no function holds as an entry, each
//! classified as data, padding or code with the evidence for it.
//!
//! usage: fates_gaps <rom> <out file> [symbols.tsv]
//! FATES_OVERRIDES=<dir> adds the overrides' addresses as a build does, and
//! FATES_HINTS=<file> reads the hints from that list instead of the bundled
//! one (an older hints.txt, to see the gaps before some hints).
//! FATES_DUMP=<lo>-<hi> also lists the functions holding code in that range.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use recomp3ds::discover::{self, Analysis, Byte, Mode};
use recomp3ds::image::Segment;
use recomp3ds::rom::Title;
use recomp3ds::{build, Mods};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        return Err("usage: fates_gaps <rom> <out file> [symbols.tsv]".into());
    }
    let title = Title::load(&args[1]).map_err(|e| format!("{e:?}"))?;
    let mut programs = recomp3ds::programs(&title, Mods::default()).map_err(|e| format!("{e:?}"))?;
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
    let hinted = match std::env::var_os("FATES_HINTS") {
        Some(path) => build::apply_hints_from(&title, &mut programs, &mut analyses, &std::fs::read_to_string(path)?),
        None => build::apply_hints(&title, &mut programs, &mut analyses),
    };
    let symbols = read_symbols(args.get(3));
    let (program, analysis) = (&programs[0].1, &analyses[0]);
    let text = &program.text;
    let name = |a: u32| symbols.range(..=a).next_back().map(|(&s, n)| format!("{n} (0x{s:08X})")).unwrap_or_default();

    let mut report = String::new();
    writeln!(report, "# gaps in the recompiled code of {}", args[1])?;
    writeln!(report, "# {} functions, {} hinted places start functions", analysis.functions.len(), hinted)?;
    let held: usize = analysis.functions.values().map(|f| f.instructions.len()).sum();
    writeln!(report, "# {} instructions held (with overlaps); code bytes {}, literal bytes {}, unknown bytes {}", held, analysis.count(Byte::Code), analysis.count(Byte::Literal), analysis.count(Byte::Unknown))?;
    writeln!(
        report,
        "# close_gaps: {} labels, {} functions, {} guesses in the other mode taken out, {} left",
        analysis.gap_labels, analysis.gap_functions, analysis.gap_evictions, analysis.gaps_left
    )?;
    let gaps = discover::gaps(text, analysis);
    // the ARM instructions recompiled functions hold
    let arm_held: BTreeSet<u32> = analysis
        .functions
        .iter()
        .filter(|(entry, f)| f.mode == Mode::Arm && f.labels.contains(entry))
        .flat_map(|(_, f)| f.instructions.iter().copied())
        .collect();
    let mut by_class: BTreeMap<&str, usize> = BTreeMap::new();
    let mut lines = Vec::new();
    for gap in &gaps {
        let (class, evidence) = classify(text, analysis, &arm_held, gap);
        *by_class.entry(class).or_default() += 1;
        let mode = if gap.mode == Mode::Thumb { "T" } else { "A" };
        lines.push(format!(
            "{class:7} {:08X}{mode} {:?} from {:08X} in f_{:08X}: {evidence} | {}",
            gap.to,
            gap.way,
            gap.from,
            gap.function,
            name(gap.to)
        ));
    }
    writeln!(report, "# {} gaps: {}", gaps.len(), by_class.iter().map(|(c, n)| format!("{n} {c}")).collect::<Vec<_>>().join(", "))?;
    for mode in ["A", "T"] {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for line in &lines {
            if line[8..17].ends_with(mode) {
                *counts.entry(line[..7].trim()).or_default() += 1;
            }
        }
        writeln!(report, "# {} gaps: {:?}", if mode == "A" { "ARM" } else { "Thumb" }, counts)?;
    }
    let made: Vec<String> = analysis
        .functions
        .iter()
        .filter(|(_, f)| f.source == discover::Source::Gap)
        .map(|(&e, f)| format!("f_{e:08X} {:?} {} instructions {}", f.mode, f.instructions.len(), name(e)))
        .collect();
    writeln!(report, "# {} functions close_gaps made:", made.len())?;
    for line in made {
        writeln!(report, "#   {line}")?;
    }
    for line in lines {
        writeln!(report, "{line}")?;
    }
    // the starts of functions written by hand (and the host's divisions) that
    // are instructions inside other generated functions: codegen makes the
    // code there call the override (main 253f0aa), check the C for each
    writeln!(report, "\n# override starts inside other functions (codegen calls the override there)")?;
    let mut inside = 0;
    for &address in &replaced {
        let (at, mode) = (address & !1, if address & 1 != 0 { Mode::Thumb } else { Mode::Arm });
        for (&entry, f) in &analysis.functions {
            if entry != at && f.mode == mode && f.labels.contains(&entry) && f.instructions.binary_search(&at).is_ok() {
                inside += 1;
                writeln!(report, "inside {address:08X} in f_{entry:08X} ({:?}) | {}", f.source, name(at))?;
            }
        }
    }
    writeln!(report, "# {inside} places")?;
    if let Some(range) = std::env::var("FATES_DUMP").ok().filter(|r| !r.is_empty()) {
        let (lo, hi) = range.split_once('-').ok_or("FATES_DUMP=lo-hi")?;
        let (lo, hi) = (u32::from_str_radix(lo, 16)?, u32::from_str_radix(hi, 16)?);
        writeln!(report, "\n# functions holding code in {lo:08X}-{hi:08X}")?;
        for (&entry, f) in &analysis.functions {
            let inside: Vec<u32> = f.instructions.iter().copied().filter(|a| (lo..hi).contains(a)).collect();
            if inside.is_empty() {
                continue;
            }
            writeln!(
                report,
                "f_{entry:08X} {:?} {:?} {} instructions, {} in range {:08X}-{:08X}, labels in range {:08X?}",
                f.source,
                f.mode,
                f.instructions.len(),
                inside.len(),
                inside[0],
                inside[inside.len() - 1],
                f.labels.range(lo..hi).collect::<Vec<_>>()
            )?;
            // runs of instructions it holds there
            let mut runs: Vec<(u32, u32)> = Vec::new();
            for &a in &inside {
                match runs.last_mut() {
                    Some(r) if r.1 + 4 >= a => r.1 = a,
                    _ => runs.push((a, a)),
                }
            }
            writeln!(report, "    runs {:08X?}", runs)?;
        }
        let mut a = lo;
        while a < hi {
            let byte = analysis.map[(a - text.base) as usize];
            writeln!(report, "  {a:08X} {:08X} {byte:?}", text.read32(a).unwrap_or(0))?;
            a += 4;
        }
    }
    // FATES_HELD=<file>: every instruction recompiled functions hold, with
    // its mode and the entries of the functions holding it, to compare runs
    if let Some(path) = std::env::var_os("FATES_HELD") {
        let mut held: BTreeMap<(u32, Mode), Vec<u32>> = BTreeMap::new();
        for (&entry, f) in &analysis.functions {
            if f.labels.contains(&entry) {
                for &a in &f.instructions {
                    held.entry((a, f.mode)).or_default().push(entry);
                }
            }
        }
        let mut out = String::new();
        for ((a, mode), owners) in held {
            let labelled = owners.iter().any(|e| analysis.functions[e].labels.contains(&a));
            writeln!(out, "{a:08X} {} {} {:08X?}", if mode == Mode::Thumb { "T" } else { "A" }, if labelled { "L" } else { "-" }, owners)?;
        }
        std::fs::write(path, out)?;
    }
    std::fs::write(&args[2], report)?;
    println!("{} gaps: {by_class:?}", gaps.len());
    Ok(())
}

/// data, padding or code, and why. data: no instruction there in the mode
/// the code goes in, so the CPU could not run it either (a lone half of a
/// Thumb bl/blx pair, which is what the upper half of most ARM words reads
/// as), with what the code that goes there is; padding: nops up to code a
/// function starts; code otherwise, a real gap.
fn classify(text: &Segment, analysis: &Analysis, arm_held: &BTreeSet<u32>, gap: &discover::Gap) -> (&'static str, String) {
    let byte = |a: u32| text.contains(a).then(|| analysis.map[(a - text.base) as usize]);
    let to = gap.to;
    // what the code that goes there is
    let from = &analysis.functions[&gap.function];
    let ascii = |a: u32| (0..4).all(|i| text.read32(a & !3).is_some_and(|w| matches!((w >> (8 * i)) as u8, 0 | 0x09 | 0x0A | 0x0D | 0x20..=0x7E)));
    let wrong_mode = from.mode == Mode::Thumb && arm_held.contains(&(gap.from & !3));
    let text_read = ascii(gap.from);
    let source = if wrong_mode {
        format!("f_{:08X} is a {:?} guess in Thumb over ARM code (ARM word {:08X} at {:08X} is held as ARM)", gap.function, from.source, text.read32(gap.from & !3).unwrap_or(0), gap.from & !3)
    } else if text_read {
        format!("f_{:08X} is a {:?} guess over text ({:08X} at {:08X} is ASCII)", gap.function, from.source, text.read32(gap.from & !3).unwrap_or(0), gap.from & !3)
    } else {
        format!("f_{:08X} ({:?}, {:?})", gap.function, from.source, from.mode)
    };
    if !text.contains(to) {
        return ("data", format!("outside the code; {source}"));
    }
    if !discover::decodes(text, to, gap.mode) {
        let what = match gap.mode {
            Mode::Thumb => format!("halfword {:04X} is no Thumb instruction (a lone half of bl/blx)", text.read16(to).unwrap_or(0)),
            Mode::Arm => format!("word {:08X} is no ARM instruction", text.read32(to).unwrap_or(0)),
        };
        return ("data", format!("{what}; {source}"));
    }
    if gap.mode == Mode::Arm {
        let nop = |w: u32| w == 0xE1A0_0000 || w == 0xE320_F000;
        if text.read32(to).is_some_and(nop) {
            let mut a = to;
            while text.read32(a).is_some_and(nop) {
                a += 4;
            }
            if analysis.functions.get(&a).is_some_and(|f| f.labels.contains(&a)) {
                return ("padding", format!("nops up to f_{a:08X}'s entry; {source}"));
            }
        }
    }
    if wrong_mode || text_read {
        // the code that goes there is not code in that mode, so it never runs
        let what = analysis.functions.get(&to).filter(|f| f.mode != gap.mode).map(|f| format!("a {:?} {:?} function starts there", f.source, f.mode)).unwrap_or_else(|| format!("{:?} bytes", byte(to).unwrap_or(Byte::Unknown)));
        return ("data", format!("{what}; {source}"));
    }
    let other = analysis.functions.get(&to).filter(|f| f.mode != gap.mode).map(|f| format!(", a {:?} function in the other mode starts there", f.source)).unwrap_or_default();
    ("code", format!("{:?} {:08X}{other}; {source}", byte(to).unwrap_or(Byte::Unknown), text.read32(to & !3).unwrap_or(0)))
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
