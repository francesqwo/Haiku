//! codemods: patch modules, the code a mod changes recompiled on its own.
//!
//! A mod that patches the executable (an IPS or BPS patch of code.bin, a
//! code.bin of its own, an exheader making the code longer) changes a few of
//! the functions the program was recompiled from, and may add code of its
//! own in free space. The program finds the functions whose bytes changed
//! by itself (the origins tables and the stale flags), and from ABI 6 a
//! stale function asks its host to run the code for the address it was
//! entered at in its place (Host::redirect). A patch module is that code:
//! every function of the patched executable that holds changed or new code,
//! recompiled apart into a small library the host loads beside the program.
//! Unchanged functions keep running the program's own code.
//!
//! The functions of a patch module go to every other function through the
//! host (recomp_call, Host::lookup), so a module needs nothing of the
//! program but its tails registry, which the host hands it (recomp.h,
//! RECOMP_IMPORTED). Coverage is the same as the program's: discovery runs
//! over the whole patched executable, close_gaps included, and plan()
//! takes in every function a direct transfer of the module's code leads to
//! that the program has no unchanged code for, until there is none left.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write;

use crate::abi;
use crate::build::label_entered;
use crate::codegen;
use crate::discover::{self, Analysis, Function, Mode, Program, Source};
use crate::image::Segment;
use crate::rom::ExHeader;

/// the version of what this module writes, part of every cache key.
/// codemods2: 2, the entries a patch left no code at (recomp_patch_gone),
/// and the analysis that guesses what changed code still is.
/// modfix: 3, holes only guessed code goes to are dead ends, and msr writes
/// the cpsr as user mode does.
/// vow: 4, code in the pages a mod's exheader adds past .data and .bss.
pub const VERSION: u32 = 4;

/// a function of the program, as its tables describe it.
#[derive(Debug, Clone, PartialEq)]
pub struct BaseFunction {
    pub start: u32,
    pub thumb: bool,
    /// the code it was made from, start to end, in order.
    pub spans: Vec<(u32, u32)>,
}

/// what the program's own code was recompiled from: its functions, and the
/// function that runs each address it has code for (bit 0 set for Thumb),
/// none for code written by hand (an override, a native).
#[derive(Debug, Clone, Default)]
pub struct Base {
    pub functions: Vec<BaseFunction>,
    pub entries: BTreeMap<u32, Option<usize>>,
}

impl Base {
    /// the program's code as its tables describe it, the executable's unit.
    pub fn from_tables(origins: &abi::Origins, entries: &[abi::Entry]) -> Base {
        let pieces = origins.pieces();
        let runs = origins.runs();
        let owners = origins.owners(entries.len());
        let mut functions: Vec<BaseFunction> = origins
            .functions()
            .iter()
            .map(|function| {
                let spans = runs[function.first as usize..(function.first + function.count) as usize]
                    .iter()
                    .filter(|run| run.end > run.first)
                    .map(|run| (pieces[run.first as usize].start, pieces[run.end as usize - 1].end))
                    .collect();
                BaseFunction { start: function.start, thumb: false, spans }
            })
            .collect();
        let mut map = BTreeMap::new();
        for (index, entry) in entries.iter().enumerate() {
            let owner = owners.get(index).copied().filter(|&owner| owner != abi::NO_ORIGIN).map(|owner| owner as usize);
            if let Some(function) = owner.and_then(|owner| functions.get_mut(owner)) {
                if function.start == entry.address & !1 {
                    function.thumb = entry.address & 1 != 0;
                }
            }
            map.insert(entry.address, owner);
        }
        Base { functions, entries: map }
    }

    /// a hash of the tables, which a patch module is built for and checked
    /// against when it is loaded: another build of the program has other
    /// functions.
    pub fn digest(&self) -> u64 {
        let mut hash = abi::CodeHash::default();
        for function in &self.functions {
            hash.add(&function.start.to_le_bytes());
            hash.add(&[function.thumb as u8]);
            for (start, end) in &function.spans {
                hash.add(&start.to_le_bytes());
                hash.add(&end.to_le_bytes());
            }
        }
        for (key, owner) in &self.entries {
            hash.add(&key.to_le_bytes());
            hash.add(&owner.map_or(u32::MAX, |owner| owner as u32).to_le_bytes());
        }
        hash.value()
    }

    /// codemods2: the tables as text, one function a line (start, thumb,
    /// spans) then one entry a line, for looking at a mod's plan away from
    /// the program (examples/patch_plan.rs, HAIKU_CODEMODS_DUMP).
    pub fn to_text(&self) -> String {
        let mut text = String::new();
        for function in &self.functions {
            write!(text, "f {:08X} {}", function.start, function.thumb as u8).unwrap();
            for (start, end) in &function.spans {
                write!(text, " {start:08X}-{end:08X}").unwrap();
            }
            text.push('\n');
        }
        for (key, owner) in &self.entries {
            match owner {
                Some(owner) => writeln!(text, "e {key:08X} {owner}").unwrap(),
                None => writeln!(text, "e {key:08X} -").unwrap(),
            }
        }
        text
    }

    /// codemods2: the tables to_text wrote.
    pub fn from_text(text: &str) -> Option<Base> {
        let hex = |word: &str| u32::from_str_radix(word, 16).ok();
        let mut base = Base::default();
        for line in text.lines() {
            let mut words = line.split_whitespace();
            match words.next()? {
                "f" => {
                    let start = hex(words.next()?)?;
                    let thumb = words.next()? == "1";
                    let spans = words.map(|span| span.split_once('-').and_then(|(a, b)| Some((hex(a)?, hex(b)?)))).collect::<Option<Vec<_>>>()?;
                    base.functions.push(BaseFunction { start, thumb, spans });
                }
                "e" => {
                    let key = hex(words.next()?)?;
                    let owner = words.next()?;
                    base.entries.insert(key, if owner == "-" { None } else { Some(owner.parse().ok()?) });
                }
                _ => return None,
            }
        }
        Some(base)
    }
}

/// the places of a byte image where two images differ, as address ranges,
/// image starting at base.
pub fn changed_ranges(base: u32, original: &[u8], patched: &[u8]) -> Vec<(u32, u32)> {
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    let length = original.len().max(patched.len());
    let mut at = 0;
    while at < length {
        if original.get(at) == patched.get(at) {
            at += 1;
            continue;
        }
        let start = at;
        while at < length && original.get(at) != patched.get(at) {
            at += 1;
        }
        ranges.push((base + start as u32, base + at as u32));
    }
    ranges
}

/// whether any of the sorted, separate ranges meets start..end.
fn meets(ranges: &[(u32, u32)], start: u32, end: u32) -> bool {
    let at = ranges.partition_point(|&(_, range_end)| range_end <= start);
    ranges.get(at).is_some_and(|&(range_start, _)| range_start < end)
}

/// how many bytes the instruction at address takes in mode.
fn size(text: &Segment, address: u32, mode: Mode) -> u32 {
    match mode {
        Mode::Arm => 4,
        Mode::Thumb => {
            let op = text.read16(address).unwrap_or(0);
            crate::thumb::decode(op, text.read16(address + 2), address).1
        }
    }
}

fn key(address: u32, mode: Mode) -> u32 {
    address | (mode == Mode::Thumb) as u32
}

fn mode_of(key: u32) -> Mode {
    if key & 1 != 0 { Mode::Thumb } else { Mode::Arm }
}

/// the executable of a patched code image as discovery reads it: the image
/// the way the program's own was read (Image::into_program), except that
/// code a patch put after .text in its last page, or in what an exheader
/// added to it, is code too: .text runs on to the end of the last changed
/// range there, and the pointers in the data that lead to changed code are
/// seeds, as those leading into .text are. codemods2: only .text's pages
/// run: the console maps .rodata and .data without execute permission, so
/// what a patch changes there is data (FE1 changes tables in .rodata, which
/// were read as code before and made Thumb guesses of strings).
pub fn program(exheader: &ExHeader, code: &[u8], changed: &[(u32, u32)]) -> Program {
    program_with(exheader, code, changed, &[])
}

/// vow: the pages a mod's exheader adds to .data past the end of the game's
/// own .data and .bss, where a mod built the Magikoopa way keeps its code
/// (see program_with), as one range; none when it adds none there.
pub fn added_pages(game: &ExHeader, theirs: &ExHeader) -> Vec<(u32, u32)> {
    let page = |address: u32| address.saturating_add(0xFFF) & !0xFFF;
    let own_end = page(game.data.address.saturating_add(game.data.size).saturating_add(game.bss_size));
    let their_end = page(theirs.data.address.saturating_add(theirs.data.size));
    if theirs.data.address == game.data.address && their_end > own_end { vec![(own_end, their_end)] } else { Vec::new() }
}

/// vow: program, with places past .text a code mod makes executable when it
/// runs (sorted and apart, page aligned). a mod built the Magikoopa way
/// (V.O.W Edition) has its exheader make .data longer, puts its code in the
/// pages past the game's own .data and .bss, and its loader, run from a hook
/// at the entry point, makes those pages read-write-execute with
/// svcControlProcessMemory before anything calls into them. its code there
/// is code: the text reaches on to the end of the last of them, and what lies
/// between (.rodata, the game's .data and .bss) is closed (Segment::closed),
/// so nothing there is taken for code, as before.
pub fn program_with(exheader: &ExHeader, code: &[u8], changed: &[(u32, u32)], executable: &[(u32, u32)]) -> Program {
    let image = crate::image::Image::from_code(exheader, code);
    let text_start = exheader.text.address;
    let text_end = image.text.end();
    // where each address of the image is in code, as the loader maps it
    let segments = [
        (exheader.text, 0usize),
        (exheader.rodata, exheader.text.num_pages as usize * 0x1000),
        (exheader.data, (exheader.text.num_pages + exheader.rodata.num_pages) as usize * 0x1000),
    ];
    let byte_at = |address: u32| -> u8 {
        for (info, offset) in segments {
            let mapped = info.num_pages.max((info.size + 0xFFF) / 0x1000) * 0x1000;
            if address >= info.address && address < info.address + mapped {
                return code.get(offset + (address - info.address) as usize).copied().unwrap_or(0);
            }
        }
        0
    };
    let runs_to = text_start + exheader.text.num_pages.max((exheader.text.size + 0xFFF) / 0x1000) * 0x1000;
    let end = changed.iter().map(|&(start, end)| (start, end.min(runs_to))).filter(|&(start, end)| start < runs_to && end > text_end).map(|(_, end)| end).max().unwrap_or(text_end);
    let mut text = image.text;
    if end > text_end {
        text.bytes.extend((text_end..(end + 3) & !3).map(byte_at));
    }
    // vow: on to the pages a mod makes executable, closed in between
    let executable: Vec<(u32, u32)> = executable.iter().copied().filter(|&(start, end)| start >= text.end() && end > start).collect();
    if let Some(&(_, last)) = executable.last() {
        let mut open = text.end();
        let mut closed = Vec::new();
        for &(start, end) in &executable {
            if start > open {
                closed.push((open, start));
            }
            open = open.max(end);
        }
        let from = text.end();
        text.bytes.extend((from..last).map(|address| if closed.iter().any(|&(a, b)| address >= a && address < b) { 0 } else { byte_at(address) }));
        text.closed = closed;
    }
    let in_executable = |value: u32| executable.iter().any(|&(start, end)| value >= start && value < end);
    let in_code = |value: u32| {
        (value & !1) >= text_start && (value & !1) < text_end || meets(changed, value & !1, (value & !1) + 2) || in_executable(value & !1)
    };
    let mut seeds = vec![(image.entry, Source::Entry)];
    seeds.extend(
        [&image.rodata, &image.data]
            .into_iter()
            .flat_map(Segment::words)
            .filter(|&(_, value)| in_code(value) && text.contains(value & !1))
            .map(|(_, value)| (value, Source::Pointer)),
    );
    // codemods2: and the words a patch wrote in .text: a patch that keeps
    // its data in .text (FE1 puts vtables, jump tables and pointer tables
    // there) has pointers into code there that nothing else leads to. they
    // are guesses, as pointers in data are; instructions seldom pass for one
    let mut written: Vec<(u32, Source)> = Vec::new();
    for &(start, end) in changed {
        let mut at = start & !3;
        while at < end.min(text.end()) {
            if let Some(value) = text.read32(at).filter(|&value| at >= text_start && text.contains(value & !1)) {
                written.push((value, Source::Pointer));
            }
            at += 4;
        }
    }
    written.sort_unstable();
    written.dedup();
    seeds.extend(written);
    Program { text, seeds, slots: None }
}

/// what a patch module holds and why.
pub struct Plan {
    /// the functions of the patched code it recompiles, by entry.
    pub functions: BTreeMap<u32, Function>,
    /// the program's functions whose code changed, by index in Base.
    pub stale: Vec<usize>,
    /// addresses the program has code written by hand for (an override)
    /// whose original changed: the program's code goes on running there,
    /// and runs the module's version where it calls the original.
    pub replaced: Vec<u32>,
    /// entries of the changed functions that the patched code has no
    /// instruction at, which nothing may go to any more.
    pub missing: Vec<u32>,
    /// instructions of the module that the program had no code for: new
    /// code, in free space or in what a patch took over.
    pub new_instructions: usize,
    /// places the module's code goes to directly that neither the program
    /// nor the module has code for, which a module must have none of, with
    /// the instruction going there (codemods2: and the function holding it).
    pub holes: Vec<discover::Gap>,
    /// codemods2: places the module's code goes to directly that are no
    /// instruction in the mode it goes there in (an undefined encoding, the
    /// middle of one, a Thumb-2 half the ARM11 doesn't have): the CPU could
    /// not run them either, so they are left to the host, as the program's
    /// own analysis leaves its gaps (Analysis::gaps_left). they come from
    /// code a function of the program was only guessed to be, a pointer in
    /// data taken for Thumb code, say, which a patch then wrote over.
    pub dead_ends: Vec<discover::Gap>,
}

/// works out the functions of a patch module for program, the patched
/// executable, given the ranges of its bytes that differ from the code the
/// program was made from.
pub fn plan(base: &Base, mut program: Program, changed: &[(u32, u32)]) -> (Plan, Program) {
    let mut changed: Vec<(u32, u32)> = changed.to_vec();
    changed.sort_unstable();
    let stale: Vec<usize> = base
        .functions
        .iter()
        .enumerate()
        .filter(|(_, function)| function.spans.iter().any(|&(start, end)| meets(&changed, start, end)))
        .map(|(index, _)| index)
        .collect();
    let is_stale: BTreeSet<usize> = stale.iter().copied().collect();
    // every entry of a changed function, which the program's code may still
    // go to, has to run in the module
    let stale_entries: Vec<u32> = base.entries.iter().filter(|(_, owner)| owner.is_some_and(|owner| is_stale.contains(&owner))).map(|(&key, _)| key).collect();
    // the program's code, by mode, to tell new code from old
    let mut held: [Vec<(u32, u32)>; 2] = [Vec::new(), Vec::new()];
    for function in &base.functions {
        held[function.thumb as usize].extend(function.spans.iter().copied());
    }
    for spans in &mut held {
        spans.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::new();
        for &(start, end) in spans.iter() {
            match merged.last_mut() {
                Some(last) if start <= last.1 => last.1 = last.1.max(end),
                _ => merged.push((start, end)),
            }
        }
        *spans = merged;
    }

    // codemods2: the changed functions' starts are guesses, judged as
    // discovery judges pointers in data, not code for sure: the program's
    // functions include what its analysis only guessed was code (Thumb read
    // over ARM code, string tables a guess ran into), and a patch may write
    // data over a function it no longer uses (FE1 writes strings and tables
    // over several). code that is code still is reached for sure where the
    // program's code calls or branches to it, as when the program was
    // analysed, and passes as a guess where only pointers lead to it
    let seed = |key: u32| (key, Source::Pointer);
    let seeds: Vec<(u32, Source)> = stale.iter().map(|&index| seed(base.functions[index].start | base.functions[index].thumb as u32)).collect();
    program.seeds.extend(seeds);
    let mut analysis = discover::analyze(&program);
    // entries of changed functions nothing in the patched code holds start
    // functions of their own, when they are instructions
    let holds = |analysis: &Analysis, key: u32| {
        let (address, mode) = (key & !1, mode_of(key));
        analysis.functions.values().any(|f| f.mode == mode && f.instructions.binary_search(&address).is_ok())
    };
    let unheld: Vec<u32> = stale_entries.iter().copied().filter(|&key| !holds(&analysis, key)).collect();
    if !unheld.is_empty() {
        let seeds: Vec<(u32, Source)> = unheld.iter().map(|&key| seed(key)).collect();
        program.seeds.extend(seeds);
        analysis = discover::analyze(&program);
    }
    label_entered(&mut analysis, &stale_entries);
    let missing: Vec<u32> = stale_entries.iter().copied().filter(|&key| !holds(&analysis, key)).collect();

    // what each label of the patched code is a label of
    let mut labelled: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&entry, function) in &analysis.functions {
        if !codegen::recompiles(entry, function) {
            continue;
        }
        for &label in &function.labels {
            labelled.entry(key(label, function.mode)).or_default().push(entry);
        }
    }
    let stale_set: BTreeSet<u32> = stale_entries.iter().copied().collect();
    // vow: the code of the changed functions, which the program can't run
    // any more, by mode: a function of the patched code holding some of it
    // runs in the module, even unchanged and reached only through a
    // pointer (V.O.W's hooks replace a function's first instruction with a
    // branch to its own code, which goes back to the second through a
    // literal: `ldr r12, =second; bx r12`)
    let mut stale_code: [Vec<(u32, u32)>; 2] = [Vec::new(), Vec::new()];
    for &index in &stale {
        let function = &base.functions[index];
        stale_code[function.thumb as usize].extend(function.spans.iter().copied());
    }
    for spans in &mut stale_code {
        spans.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::new();
        for &(start, end) in spans.iter() {
            match merged.last_mut() {
                Some(last) if start <= last.1 => last.1 = last.1.max(end),
                _ => merged.push((start, end)),
            }
        }
        *spans = merged;
    }
    let mut selected: BTreeSet<u32> = BTreeSet::new();
    let mut new_instructions = 0;
    for (&entry, function) in &analysis.functions {
        if !codegen::recompiles(entry, function) {
            continue;
        }
        let spans = &held[(function.mode == Mode::Thumb) as usize];
        let stale_spans = &stale_code[(function.mode == Mode::Thumb) as usize];
        let mut take = function.labels.iter().any(|&label| stale_set.contains(&key(label, function.mode)));
        let mut fresh = 0;
        for &address in &function.instructions {
            let end = address + size(&program.text, address, function.mode);
            if meets(&changed, address, end) || meets(stale_spans, address, end) {
                take = true;
            }
            if !meets(spans, address, end) {
                fresh += 1;
            }
        }
        if fresh > 0 || take {
            selected.insert(entry);
            new_instructions += fresh;
        }
    }
    // closure: whatever the module's code goes to directly runs either in
    // the program's unchanged code or in the module
    let base_runs = |key: u32| match base.entries.get(&key) {
        Some(None) => true,
        Some(Some(owner)) => !is_stale.contains(owner),
        None => false,
    };
    let mut module_labels: BTreeSet<u32> = BTreeSet::new();
    let mut work: Vec<u32> = selected.iter().copied().collect();
    for &entry in &selected {
        let function = &analysis.functions[&entry];
        module_labels.extend(function.labels.iter().map(|&label| key(label, function.mode)));
    }
    let mut holes = BTreeSet::new();
    let mut dead_ends = BTreeSet::new();
    while let Some(entry) = work.pop() {
        let function = &analysis.functions[&entry];
        for transfer in discover::transfers(&program.text, function) {
            if transfer.local && (transfer.next_held || function.labels.contains(&transfer.to)) {
                continue;
            }
            let to = key(transfer.to, transfer.mode);
            if module_labels.contains(&to) || base_runs(to) {
                continue;
            }
            match labelled.get(&to) {
                Some(holders) => {
                    for &holder in holders {
                        if selected.insert(holder) {
                            let function = &analysis.functions[&holder];
                            module_labels.extend(function.labels.iter().map(|&label| key(label, function.mode)));
                            work.push(holder);
                        }
                    }
                }
                None => {
                    let gap = discover::Gap { to: transfer.to, mode: transfer.mode, from: transfer.from, function: entry, way: transfer.way };
                    // modfix: a place only code that may not be code at all
                    // goes to (a Thumb guess of the program read over ARM
                    // code, which a patch wrote over, calling into the ARM
                    // code beside it) is left to the host, as the
                    // instructions such code has that the generator can't
                    // lower are: the CPU would never go there either. only
                    // code sure to be code refuses the module
                    if discover::decodes(&program.text, transfer.to, transfer.mode) && !maybe_data(&program, function) {
                        holes.insert(gap);
                    } else {
                        dead_ends.insert(gap);
                    }
                }
            }
        }
    }
    // the overrides whose original changed
    let replaced: Vec<u32> = base
        .entries
        .iter()
        .filter(|(_, owner)| owner.is_none())
        .map(|(&key, _)| key)
        .filter(|&key| stale.iter().any(|&index| base.functions[index].start == key & !1 && base.functions[index].thumb == (key & 1 != 0)))
        .collect();
    let functions = selected.into_iter().map(|entry| (entry, analysis.functions[&entry].clone())).collect();
    (Plan { functions, stale, replaced, missing, new_instructions, holes: holes.into_iter().collect(), dead_ends: dead_ends.into_iter().collect() }, program)
}

/// instructions per source file, as codegen splits them.
const FILE_SIZE: usize = 4_000;

/// codemods2: why a module can't be written: instructions the generator
/// would leave to an interpreter, in code that is sure to be code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unrecompilable {
    pub count: usize,
    /// the first of them.
    pub first: u32,
}

impl std::fmt::Display for Unrecompilable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the patched code has {} instructions Haiku can't recompile, the first at 0x{:08X}", self.count, self.first)
    }
}

/// the C of a patch module, and what of it goes to the host.
pub struct Module {
    /// file names and contents.
    pub files: Vec<(String, String)>,
    /// codemods2: instructions the generator leaves to the interpreter in
    /// what may not be code at all (maybe_data): a host without an
    /// interpreter stops there, as it does in the program's own code, if
    /// the game ever goes there. code a patch writes data over is guessed
    /// again, and data read as code has such instructions.
    pub left_to_host: Vec<u32>,
}

/// whether a function of the module may not be code at all: discovery only
/// guessed it is (a pointer in data, a scan of a gap, a place such code
/// goes to; not a call, even one a guess made, as most of the game's code
/// is reached through vtables), or, read as ARM, its instructions are nearly all conditional,
/// as text and tables read as ARM are and as code a compiler made never is
/// (FE1 writes strings over functions whose callers it no longer runs).
fn maybe_data(program: &Program, function: &Function) -> bool {
    if matches!(function.source, Source::Pointer | Source::Scan | Source::Gap) {
        return true;
    }
    if function.mode != Mode::Arm {
        return false;
    }
    let conditional = function.instructions.iter().filter(|&&address| program.text.read32(address).is_some_and(|word| word >> 28 != 0xE)).count();
    // the rule discovery judges guesses by (MOST_CONDITIONAL), the other way
    conditional * 5 > function.instructions.len() * 4
}

/// the C of a patch module for plan: recomp.h, the functions in files of
/// their own, and patch.c with the table of every entry the module has code
/// for. key is what the host checks the module against: its base digest
/// and the patched code. an error names an instruction the module's code
/// would have left to the interpreter, which a host without one cannot run,
/// in code that is sure to be code (codemods2: see Module::left_to_host).
pub fn source(plan: &Plan, program: &Program, base: &Base, base_digest: u64, code_key: u64) -> Result<Module, Unrecompilable> {
    const PREFIX: &str = "p_";
    let mut names: BTreeMap<u32, String> = BTreeMap::new();
    for (&entry, function) in &plan.functions {
        names.insert(key(entry, function.mode), codegen::function_name(PREFIX, entry, function.mode == Mode::Thumb));
    }
    // code written by hand wins: calls to it, and code running into its
    // start, go to the program's through the host (the stubs)
    let handwritten: Vec<u32> = base.entries.iter().filter(|(_, owner)| owner.is_none()).map(|(&key, _)| key).collect();
    for &key in &handwritten {
        names.insert(key, format!("override_stub_{key:08X}"));
    }
    let mut header = String::from("/* codemods: a patch module's functions, see 3dsrecomp src/patch.rs */\n#ifndef RECOMP_PATCH_H\n#define RECOMP_PATCH_H\n");
    for (&entry, function) in &plan.functions {
        writeln!(header, "void {}(Context *ctx);", codegen::function_name(PREFIX, entry, function.mode == Mode::Thumb)).unwrap();
    }
    header.push_str("/* the program's code written by hand, reached through the host */\n");
    for &key in &handwritten {
        writeln!(header, "static void override_stub_{key:08X}(Context *ctx) {{ recomp_call(ctx); }}").unwrap();
    }
    header.push_str("#endif\n");

    // codemods2: recomp.h declares recomp_imports plainly, and patch.c
    // defines it exported: gcc lets a definition add dllexport, clang (Zig,
    // the compiler Haiku downloads) refuses it. the module's copy of the
    // header declares it exported already (the program's own is unchanged)
    let module_header = codegen::HEADER.replacen("extern RecompImports recomp_imports;", "extern RECOMP_EXPORT RecompImports recomp_imports;", 1);
    debug_assert!(module_header != codegen::HEADER, "recomp.h declares recomp_imports another way now");
    let mut files = vec![("recomp.h".to_owned(), module_header), ("patch.h".to_owned(), header)];
    // every label, preferring the function it starts
    let mut entries: BTreeMap<u32, String> = BTreeMap::new();
    for (&entry, function) in &plan.functions {
        let name = codegen::function_name(PREFIX, entry, function.mode == Mode::Thumb);
        for &label in &function.labels {
            let slot = entries.entry(key(label, function.mode)).or_insert_with(|| name.clone());
            if label == entry {
                slot.clone_from(&name);
            }
        }
    }
    // modfix: an override whose original the patch changed calls the
    // module's function the host finds for that address (entries); there
    // the code at the address is the mod's, not a way back to the override.
    // Gay Fates puts code where debug::Menu::MoveToNext was (0x00421114,
    // which supports.c overrides) and branches into it from its other
    // changed code: each pass through it called the override, which called
    // the original, which went to the override again, and My Castle's menu
    // never opened
    let originals: BTreeMap<u32, String> = plan.replaced.iter().filter_map(|&replaced| Some((replaced, entries.get(&replaced)?.clone()))).collect();
    let mut source = String::new();
    let mut size = 0;
    let mut sources = Vec::new();
    let mut interpreted = Vec::new();
    let mut left_to_host = Vec::new();
    for (&entry, function) in &plan.functions {
        if source.is_empty() {
            source.push_str("#define RECOMP_IMPORTED 1\n#include \"recomp.h\"\n#include \"patch.h\"\n\n");
        }
        let left = codegen::write_patch_function(&mut source, program, &names, &originals, PREFIX, entry, function);
        if maybe_data(program, function) {
            left_to_host.extend(left);
        } else {
            interpreted.extend(left);
        }
        size += function.instructions.len();
        if size >= FILE_SIZE {
            sources.push(std::mem::take(&mut source));
            size = 0;
        }
    }
    if !source.is_empty() {
        sources.push(source);
    }
    if let Some(&first) = interpreted.first() {
        return Err(Unrecompilable { count: interpreted.len(), first });
    }
    let mut table = String::from("#define RECOMP_IMPORTED 1\n#include \"recomp.h\"\n#include \"patch.h\"\n\n");
    table.push_str("/* filled in by the host before any of the module runs */\nRECOMP_EXPORT RecompImports recomp_imports;\n");
    writeln!(table, "RECOMP_EXPORT const uint32_t recomp_patch_abi = RECOMP_ABI;").unwrap();
    writeln!(table, "RECOMP_EXPORT const uint32_t recomp_patch_version = {VERSION}u;").unwrap();
    writeln!(table, "RECOMP_EXPORT const uint64_t recomp_patch_base = 0x{base_digest:016X}ull;").unwrap();
    writeln!(table, "RECOMP_EXPORT const uint64_t recomp_patch_key = 0x{code_key:016X}ull;").unwrap();
    writeln!(table, "RECOMP_EXPORT const uint32_t recomp_patch_count = {};", entries.len()).unwrap();
    writeln!(table, "RECOMP_EXPORT const Entry recomp_patch_entries[] = {{").unwrap();
    for (label, name) in &entries {
        writeln!(table, "    {{0x{label:08X}u, {name}}},").unwrap();
    }
    if entries.is_empty() {
        table.push_str("    {0xFFFFFFFFu, 0},\n");
    }
    table.push_str("};\n");
    // codemods2: the changed functions' entries the patched code has no
    // instruction at (data now, say): the host accepts that the module has
    // no code for them, and stops there as for any code it has none for
    writeln!(table, "RECOMP_EXPORT const uint32_t recomp_patch_gone_count = {};", plan.missing.len()).unwrap();
    writeln!(table, "RECOMP_EXPORT const uint32_t recomp_patch_gone[] = {{").unwrap();
    for key in &plan.missing {
        writeln!(table, "    0x{key:08X}u,").unwrap();
    }
    if plan.missing.is_empty() {
        table.push_str("    0xFFFFFFFFu,\n");
    }
    table.push_str("};\n");
    files.push(("patch.c".to_owned(), table));
    files.extend(sources.into_iter().enumerate().map(|(i, source)| (format!("patch{i:03}.c"), source)));
    left_to_host.sort_unstable();
    left_to_host.dedup();
    Ok(Module { files, left_to_host })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: u32 = 0x0010_0000;

    fn segment(words: &[u32]) -> Segment {
        Segment::new(BASE, words.iter().flat_map(|w| w.to_le_bytes()).collect())
    }

    /// the program as codegen would describe it, one function per function
    /// discovery found, each label an entry of it.
    fn base_of(words: &[u32]) -> Base {
        let program = Program { text: segment(words), seeds: vec![(BASE, Source::Entry)], slots: None };
        let analysis = discover::analyze(&program);
        let mut base = Base::default();
        for (&entry, function) in &analysis.functions {
            let index = base.functions.len();
            let spans = function.instructions.iter().map(|&a| (a, a + 4)).collect();
            base.functions.push(BaseFunction { start: entry, thumb: false, spans });
            for &label in &function.labels {
                base.entries.entry(label).or_insert(Some(index));
            }
        }
        base
    }

    fn bytes(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    #[test]
    fn changed_bytes_come_in_ranges() {
        assert_eq!(changed_ranges(0x100, &[1, 2, 3, 4], &[1, 9, 9, 4, 5]), [(0x101, 0x103), (0x104, 0x105)]);
        assert!(changed_ranges(0, &[1], &[1]).is_empty());
    }

    /// a patched constant recompiles only the function holding it.
    #[test]
    fn only_the_changed_function_goes_in_the_module() {
        let own = [
            0xEB00_0001, // bl BASE + 0xC
            0xEB00_0002, // bl BASE + 0x14
            0xE12F_FF1E, // bx lr
            0xE3A0_0001, // mov r0, #1
            0xE12F_FF1E, // bx lr
            0xE3A0_0002, // mov r0, #2
            0xE12F_FF1E, // bx lr
        ];
        let mut patched = own;
        patched[5] = 0xE3A0_0063; // mov r0, #99
        let base = base_of(&own);
        let changed = changed_ranges(BASE, &bytes(&own), &bytes(&patched));
        let program = Program { text: segment(&patched), seeds: vec![(BASE, Source::Entry)], slots: None };
        let (plan, program) = plan(&base, program, &changed);
        assert_eq!(plan.functions.keys().copied().collect::<Vec<_>>(), [BASE + 0x14]);
        assert_eq!(plan.stale.len(), 1);
        assert!(plan.holes.is_empty() && plan.missing.is_empty());
        let files = source(&plan, &program, &base, base.digest(), 7).unwrap().files;
        let code: String = files.iter().filter(|(name, _)| name.starts_with("patch0")).map(|(_, c)| c.clone()).collect();
        assert!(code.contains("void p_f_00100014(Context *ctx)"), "{code}");
        assert!(!code.contains("STALE_CHECK"));
    }

    /// a branch into a code cave takes the cave in, and what the cave
    /// calls of the program's unchanged code stays the program's.
    #[test]
    fn a_code_cave_is_taken_in_with_the_function_that_goes_there() {
        let own = [
            0xEB00_0001, // bl BASE + 0xC
            0xE12F_FF1E, // bx lr
            0xE320_F000, // nop, padding
            0xE3A0_0001, // mov r0, #1
            0xE12F_FF1E, // bx lr
            0x0000_0000, // free space
            0x0000_0000,
            0x0000_0000,
        ];
        let mut patched = own;
        patched[3] = 0xEA00_0001; // b BASE + 0x18, into the cave
        patched[6] = 0xE3A0_0005; // mov r0, #5
        patched[7] = 0xE12F_FF1E; // bx lr
        let base = base_of(&own);
        let changed = changed_ranges(BASE, &bytes(&own), &bytes(&patched));
        let program = Program { text: segment(&patched), seeds: vec![(BASE, Source::Entry)], slots: None };
        let (plan, _) = plan(&base, program, &changed);
        let held: BTreeSet<u32> = plan.functions.values().flat_map(|f| f.instructions.iter().copied()).collect();
        assert!(held.contains(&(BASE + 0x18)) && held.contains(&(BASE + 0x1C)), "{held:X?}");
        assert!(!held.contains(&BASE), "the caller is unchanged");
        assert!(plan.new_instructions >= 2);
        assert!(plan.holes.is_empty());
    }

    /// an override whose original a patch changes is reported.
    #[test]
    fn a_change_to_code_written_by_hand_is_reported() {
        let own = [0xEB00_0000, 0xE12F_FF1E, 0xE3A0_0001, 0xE12F_FF1E];
        let mut patched = own;
        patched[2] = 0xE3A0_0002;
        let mut base = base_of(&own);
        base.entries.insert(BASE + 8, None);
        let changed = changed_ranges(BASE, &bytes(&own), &bytes(&patched));
        let program = Program { text: segment(&patched), seeds: vec![(BASE, Source::Entry)], slots: None };
        let (plan, program) = plan(&base, program, &changed);
        assert_eq!(plan.replaced, [BASE + 8]);
        let files = source(&plan, &program, &base, 0, 0).unwrap().files;
        let header = &files.iter().find(|(name, _)| name == "patch.h").unwrap().1;
        assert!(header.contains("override_stub_00100008"));
    }

    /// modfix: a patch writes code over a function Haiku overrides and
    /// branches into it from code it changed (Gay Fates at 0x00421114): the
    /// module's function holding that code is the override's original, so
    /// the code there runs, never a way back to the override (which called
    /// that original: the game went round for ever).
    #[test]
    fn code_a_patch_writes_over_an_override_runs_as_its_original() {
        let own = [
            0xEB00_0002, // bl BASE + 0x10
            0xE12F_FF1E, // bx lr
            0xE3A0_0001, // mov r0, #1: a function Haiku overrides
            0xE12F_FF1E, // bx lr
            0xE3A0_0003, // mov r0, #3
            0xE12F_FF1E, // bx lr
        ];
        let mut patched = own;
        patched[2] = 0xE3A0_0002; // mov r0, #2: the mod's code there now
        patched[5] = 0xEAFF_FFFB; // b BASE + 8, into it
        let mut base = base_of(&own);
        base.entries.insert(BASE + 8, None);
        let changed = changed_ranges(BASE, &bytes(&own), &bytes(&patched));
        let program = Program { text: segment(&patched), seeds: vec![(BASE, Source::Entry)], slots: None };
        let (made, program) = plan(&base, program, &changed);
        assert_eq!(made.replaced, [BASE + 8]);
        let module = source(&made, &program, &base, 0, 0).unwrap();
        let table = &module.files.iter().find(|(name, _)| name == "patch.c").unwrap().1;
        let holder = table.lines().find(|line| line.contains("{0x00100008u,")).and_then(|line| line.split(", ").nth(1)).map(|name| name.trim_end_matches("},").to_owned()).unwrap();
        let code: String = module.files.iter().filter(|(name, _)| name.starts_with("patch0")).map(|(_, c)| c.clone()).collect();
        let body = code.split(&format!("void {holder}(Context *ctx) {{")).nth(1).unwrap_or_else(|| panic!("{code}"));
        let body = body.split("\nvoid ").next().unwrap();
        assert!(!body.contains("CALL(override_stub_00100008)"), "{body}");
        assert!(body.contains("/* 00100008 E3A00002 */"), "{body}");
    }

    /// codemods2: FE1 writes strings over functions whose callers it changed
    /// too. the function is no code any more: the module builds, with the
    /// entry gone or its data left to the host, never refused.
    #[test]
    fn a_function_a_patch_writes_data_over_does_not_refuse_the_module() {
        let own = [
            0xEB00_0003, // bl BASE + 0x14
            0xEB00_0004, // bl BASE + 0x1C
            0xE12F_FF1E, // bx lr
            0xE320_F000, // nop
            0xE320_F000, // nop
            0xE3A0_0001, // mov r0, #1
            0xE12F_FF1E, // bx lr
            0xE3A0_0002, // mov r0, #2
            0xE12F_FF1E, // bx lr
        ];
        let mut patched = own;
        patched[1] = 0xE320_F000; // the call goes
        patched[7] = 0x6E6F_425F; // "_Bon", mcrvs p2: no instruction Haiku recompiles
        patched[8] = 0x6572_6966; // "fire"
        let base = base_of(&own);
        let changed = changed_ranges(BASE, &bytes(&own), &bytes(&patched));
        let program = Program { text: segment(&patched), seeds: vec![(BASE, Source::Entry)], slots: None };
        let (plan, program) = plan(&base, program, &changed);
        assert!(plan.holes.is_empty(), "{:X?}", plan.holes);
        let module = source(&plan, &program, &base, 0, 0).expect("data where code was doesn't refuse the module");
        let gone = plan.missing.contains(&(BASE + 0x1C));
        assert!(gone || !module.left_to_host.is_empty(), "the data is either no entry or left to the host");
        let table = &module.files.iter().find(|(name, _)| name == "patch.c").unwrap().1;
        assert!(table.contains("recomp_patch_gone_count"));
    }

    /// codemods2: a pointer a patch writes in .text (a vtable of its own)
    /// leads to the new code it points to, which nothing else reaches.
    #[test]
    fn a_pointer_a_patch_writes_in_text_leads_to_its_code() {
        let own = [
            0xEB00_0001, // bl BASE + 0xC
            0xE12F_FF1E, // bx lr
            0xE320_F000, // nop
            0xE3A0_0001, // mov r0, #1
            0xE12F_FF1E, // bx lr
            0x0000_0000, // free space
            0x0000_0000,
            0x0000_0000,
        ];
        let mut patched = own;
        patched[3] = 0xE3A0_0002; // mov r0, #2: the function changes
        patched[5] = BASE + 0x18; // a pointer to the new code
        patched[6] = 0x03A0_0005; // moveq r0, #5: no scan of a gap starts here
        patched[7] = 0xE12F_FF1E; // bx lr
        let base = base_of(&own);
        let code = bytes(&patched);
        let changed = changed_ranges(BASE, &bytes(&own), &code);
        let info = |address: u32, size: u32| crate::rom::CodeSetInfo { address, num_pages: 1, size };
        let exheader = ExHeader {
            title: String::new(),
            compress_code: false,
            text: info(BASE, code.len() as u32),
            rodata: info(BASE + 0x1000, 0),
            data: info(BASE + 0x2000, 0),
            bss_size: 0,
        };
        let mut image = code.clone();
        image.resize(0x1000, 0);
        let (plan, _) = plan(&base, program(&exheader, &image, &changed), &changed);
        let held: BTreeSet<u32> = plan.functions.values().flat_map(|f| f.instructions.iter().copied()).collect();
        assert!(held.contains(&(BASE + 0x18)), "{held:X?}");
        assert!(plan.holes.is_empty());
    }

    /// codemods2: .rodata and .data don't run on the console: what a patch
    /// changes there is data, never code to recompile.
    #[test]
    fn changes_past_text_pages_are_data() {
        let own = [0xE3A0_0001, 0xE12F_FF1E];
        let code = bytes(&own);
        let info = |address: u32, size: u32| crate::rom::CodeSetInfo { address, num_pages: 1, size };
        let exheader = ExHeader {
            title: String::new(),
            compress_code: false,
            text: info(BASE, code.len() as u32),
            rodata: info(BASE + 0x1000, 8),
            data: info(BASE + 0x2000, 0),
            bss_size: 0,
        };
        let mut image = code.clone();
        image.resize(0x1000, 0);
        image.extend_from_slice(&bytes(&[0xE3A0_0002, 0xE12F_FF1E]));
        let program = program(&exheader, &image, &[(BASE + 0x1000, BASE + 0x1008)]);
        assert_eq!(program.text.end(), BASE + 8, ".text doesn't run on into .rodata");
    }

    /// vow: a mod built the Magikoopa way makes .data longer, keeps its code
    /// in the pages past the game's .data and .bss and calls it from .text:
    /// that code is recompiled, and .rodata and the game's .data between
    /// stay closed, so a pointer into .rodata is no code.
    #[test]
    fn code_in_the_pages_an_exheader_adds_past_data_is_recompiled() {
        let info = |address: u32, size: u32| crate::rom::CodeSetInfo { address, num_pages: 1, size };
        let game = ExHeader {
            title: String::new(),
            compress_code: false,
            text: info(BASE, 8),
            rodata: info(BASE + 0x1000, 8),
            data: info(BASE + 0x2000, 8),
            bss_size: 0x10,
        };
        let theirs = ExHeader { data: info(BASE + 0x2000, 0x2000), ..game.clone() };
        assert_eq!(added_pages(&game, &theirs), [(BASE + 0x3000, BASE + 0x4000)]);
        assert!(added_pages(&game, &game).is_empty());
        let own = [0xE3A0_0001, 0xE12F_FF1E]; // mov r0, #1; bx lr
        let patched = [0xEB00_0BFE, 0xE12F_FF1E]; // bl BASE + 0x3000; bx lr
        let mut image = bytes(&patched);
        image.resize(0x1000, 0);
        image.extend_from_slice(&bytes(&[BASE + 0x1000, 0])); // .rodata: a pointer into itself
        image.resize(0x2000, 0);
        image.extend_from_slice(&bytes(&[0, 0])); // the game's .data
        image.resize(0x3000, 0);
        image.extend_from_slice(&bytes(&[0xE3A0_0002, 0xE12F_FF1E])); // the mod's: mov r0, #2; bx lr
        image.resize(0x4000, 0);
        let changed = vec![(BASE, BASE + 4), (BASE + 0x3000, BASE + 0x3008)];
        let with = program_with(&theirs, &image, &changed, &added_pages(&game, &theirs));
        assert!(with.text.contains(BASE + 0x3000));
        assert!(!with.text.contains(BASE + 0x1000) && !with.text.contains(BASE + 0x2000), ".rodata and .data are closed");
        assert_eq!(with.text.read32(BASE + 0x1000), None);
        let (made, _) = plan(&base_of(&own), with, &changed);
        assert!(made.holes.is_empty(), "{:X?}", made.holes);
        assert!(made.functions.contains_key(&(BASE + 0x3000)), "{:X?}", made.functions.keys());
        assert!(!made.functions.contains_key(&(BASE + 0x1000)));
        // without the added pages the call leads where no code is
        let without = program(&theirs, &image, &changed);
        let (made, _) = plan(&base_of(&own), without, &changed);
        assert!(!made.functions.contains_key(&(BASE + 0x3000)));
    }

    /// vow: a hook replaces a function's first instruction with a branch to
    /// new code, which goes back to the second through a literal; the rest
    /// of the changed function runs in the module, as only the module can.
    #[test]
    fn the_rest_of_a_hooked_function_reached_through_a_literal_is_recompiled() {
        let own = [
            0xEB00_0000, // bl BASE + 8
            0xE12F_FF1E, // bx lr
            0xE3A0_2000, // mov r2, #0
            0xE5C0_1000, // strb r1, [r0]
            0xE12F_FF1E, // bx lr
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        let mut patched = own;
        patched[2] = 0xEA00_0004; // b BASE + 0x20
        patched[8] = 0xE3A0_2000; // mov r2, #0
        patched[9] = 0xE59F_C000; // ldr r12, [pc, #0]
        patched[10] = 0xE12F_FF1C; // bx r12
        patched[11] = BASE + 0xC;
        let changed = changed_ranges(BASE, &bytes(&own), &bytes(&patched));
        let program = Program { text: segment(&patched), seeds: vec![(BASE, Source::Entry)], slots: None };
        let (made, _) = plan(&base_of(&own), program, &changed);
        assert!(made.holes.is_empty(), "{:X?}", made.holes);
        assert!(made.functions.values().any(|f| f.instructions.contains(&(BASE + 0x20))), "the new code: {:X?}", made.functions.keys());
        assert!(
            made.functions.values().any(|f| f.labels.contains(&(BASE + 0xC))),
            "the second instruction runs in the module: {:X?}",
            made.functions.keys()
        );
    }

    /// modfix: the program's analysis guessed Thumb code over bytes a patch
    /// then writes; read again, the guess calls into the ARM function
    /// beside it as Thumb, a place no code holds in that mode. only a guess
    /// goes there, so it is left to the host, and the module builds (Gay
    /// Fates' S-rank patch writes over such a guess at 0x00421172).
    #[test]
    fn a_guess_going_where_no_code_is_does_not_refuse_the_module() {
        let own = [
            0xEB00_0002, // bl BASE + 0x10
            0xE12F_FF1E, // bx lr
            0x0000_0000, // a Thumb guess of the program's (below)
            0x0000_0000,
            0xE3A0_0001, // mov r0, #1
            0xE12F_FF1E, // bx lr
        ];
        let mut patched = own;
        patched[2] = 0xF802_F000; // Thumb: bl BASE + 0x10, into the ARM code
        patched[3] = 0x46C0_4770; // Thumb: bx lr; nop
        let mut base = base_of(&own);
        let guess = base.functions.len();
        base.functions.push(BaseFunction { start: BASE + 8, thumb: true, spans: vec![(BASE + 8, BASE + 0xC)] });
        base.entries.insert(BASE + 9, Some(guess));
        let changed = changed_ranges(BASE, &bytes(&own), &bytes(&patched));
        let program = Program { text: segment(&patched), seeds: vec![(BASE, Source::Entry)], slots: None };
        let (plan, program) = plan(&base, program, &changed);
        assert!(plan.holes.is_empty(), "{:X?}", plan.holes);
        assert!(plan.dead_ends.iter().any(|gap| gap.to == BASE + 0x10 && gap.mode == Mode::Thumb), "{:X?}", plan.dead_ends);
        assert!(source(&plan, &program, &base, 0, 0).is_ok());
    }

    /// modfix: msr writing the control or extension field of the cpsr
    /// (text a patch put where code was reads as one: Gay Fates' "S_%s" at
    /// 0x001519BC is msrvc cpsr_sc) is what user mode makes of it, the flags
    /// and the GE bits, not an instruction that refuses the module. one that
    /// sets E, big-endian data, is still the interpreter's.
    #[test]
    fn msr_of_privileged_fields_writes_what_user_mode_can() {
        let own = [
            0xEB00_0001, // bl BASE + 0xC
            0xE12F_FF1E, // bx lr
            0xE320_F000, // nop
            0xE3A0_0001, // mov r0, #1
            0xE3A0_0002, // mov r0, #2
            0xE12F_FF1E, // bx lr
        ];
        let mut patched = own;
        patched[3] = 0xE325_F80F; // msr cpsr_sc, #0x000F0000
        patched[4] = 0xE129_F000; // msr cpsr_fc, r0
        let base = base_of(&own);
        let changed = changed_ranges(BASE, &bytes(&own), &bytes(&patched));
        let program = Program { text: segment(&patched), seeds: vec![(BASE, Source::Entry)], slots: None };
        let (made, program) = plan(&base, program, &changed);
        let module = source(&made, &program, &base, 0, 0).expect("msr with the control field is lowered");
        let code: String = module.files.iter().filter(|(name, _)| name.starts_with("patch0")).map(|(_, c)| c.clone()).collect();
        assert!(code.contains("ctx->ge = (value >> 16) & 0xF;") && code.contains("0x000F0000u"), "{code}");
        assert!(code.contains("= value >> 31;"), "{code}");
        assert!(!code.contains("INTERPRET"), "{code}");
        // setting E is left to the interpreter: sure code refuses the module
        patched[3] = 0xE322_FC02; // msr cpsr_x, #0x200
        let changed = changed_ranges(BASE, &bytes(&own), &bytes(&patched));
        let program = Program { text: segment(&patched), seeds: vec![(BASE, Source::Entry)], slots: None };
        let (made, program) = plan(&base, program, &changed);
        assert!(source(&made, &program, &base, 0, 0).is_err());
    }

    /// modfix: Thumb code a patch adds in free space, reached with an ARM
    /// blx, is found and recompiled in the module.
    #[test]
    fn a_thumb_cave_reached_by_blx_is_recompiled() {
        let own = [
            0xEB00_0001, // bl BASE + 0xC
            0xE12F_FF1E, // bx lr
            0xE320_F000, // nop
            0xE3A0_0001, // mov r0, #1
            0xE12F_FF1E, // bx lr
            0x0000_0000, // free space
            0x0000_0000,
        ];
        let mut patched = own;
        patched[3] = 0xFA00_0000; // blx BASE + 0x14 (Thumb)
        patched[5] = 0x4770_2005; // Thumb: movs r0, #5; bx lr
        let base = base_of(&own);
        let changed = changed_ranges(BASE, &bytes(&own), &bytes(&patched));
        let program = Program { text: segment(&patched), seeds: vec![(BASE, Source::Entry)], slots: None };
        let (made, program) = plan(&base, program, &changed);
        let cave = made.functions.values().find(|f| f.mode == Mode::Thumb && f.instructions.contains(&(BASE + 0x14)));
        assert!(cave.is_some_and(|f| f.instructions.contains(&(BASE + 0x16))), "{:X?}", made.functions.keys());
        assert!(made.holes.is_empty() && made.new_instructions >= 2);
        let module = source(&made, &program, &base, 0, 0).unwrap();
        let table = &module.files.iter().find(|(name, _)| name == "patch.c").unwrap().1;
        assert!(table.contains("{0x00100015u,"), "the cave's Thumb entry: {table}");
    }

    /// modfix: Thumb code a patch adds, reached through a pointer it writes
    /// in a literal (ldr r12, =cave + 1; bx r12), is found and recompiled.
    #[test]
    fn a_thumb_cave_reached_through_a_literal_is_recompiled() {
        let own = [
            0xEB00_0001, // bl BASE + 0xC
            0xE12F_FF1E, // bx lr
            0xE320_F000, // nop
            0xE3A0_0001, // mov r0, #1
            0xE12F_FF1E, // bx lr
            0x0000_0000, // free space
            0x0000_0000,
            0x0000_0000,
            0x0000_0000,
        ];
        let mut patched = own;
        patched[3] = 0xE59F_C008; // ldr r12, [pc, #8]: the word at BASE + 0x1C
        patched[4] = 0xE12F_FF1C; // bx r12
        patched[7] = BASE + 0x21; // the cave, Thumb
        patched[8] = 0x4770_2005; // Thumb: movs r0, #5; bx lr
        let base = base_of(&own);
        let changed = changed_ranges(BASE, &bytes(&own), &bytes(&patched));
        let program = Program { text: segment(&patched), seeds: vec![(BASE, Source::Entry)], slots: None };
        let (made, program) = plan(&base, program, &changed);
        let cave = made.functions.values().find(|f| f.mode == Mode::Thumb && f.instructions.contains(&(BASE + 0x20)));
        assert!(cave.is_some(), "{:X?}", made.functions.keys());
        assert!(made.holes.is_empty());
        let module = source(&made, &program, &base, 0, 0).unwrap();
        let table = &module.files.iter().find(|(name, _)| name == "patch.c").unwrap().1;
        assert!(table.contains("{0x00100021u,"), "{table}");
    }

    /// codemods2: a module links into a library with the compiler CC names
    /// (gcc, or Zig's clang, which Haiku downloads: it refused patch.c's
    /// recomp_imports before), exporting what the host looks up.
    #[test]
    fn a_module_links_into_a_library() {
        let own = [0xEB00_0000, 0xE12F_FF1E, 0xE3A0_0001, 0xE12F_FF1E];
        let mut patched = own;
        patched[2] = 0xE3A0_0002;
        let base = base_of(&own);
        let changed = changed_ranges(BASE, &bytes(&own), &bytes(&patched));
        let program = Program { text: segment(&patched), seeds: vec![(BASE, Source::Entry)], slots: None };
        let (plan, program) = plan(&base, program, &changed);
        let module = source(&plan, &program, &base, 1, 2).unwrap();
        let dir = std::env::temp_dir().join(format!("3dsrecomp-patch-module-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut sources = Vec::new();
        for (name, text) in &module.files {
            std::fs::write(dir.join(name), text).unwrap();
            if name.ends_with(".c") {
                sources.push(name.clone());
            }
        }
        let compiler = crate::compile::check(None).expect("a C compiler");
        let library = dir.join(if cfg!(windows) { "patch.dll" } else { "patch.so" });
        let built = crate::compile::compile(&compiler, &dir, &sources, &library, &|_, _| true, false);
        let exists = library.is_file();
        std::fs::remove_dir_all(&dir).ok();
        built.unwrap();
        assert!(exists);
    }
}
