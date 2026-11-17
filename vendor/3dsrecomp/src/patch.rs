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
pub const VERSION: u32 = 1;

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
/// code a patch put after .text, in .rodata or .data or in what an exheader
/// added, is code too: .text runs on to the end of the last changed range
/// there, and the pointers in the data that lead to changed code are seeds,
/// as those leading into .text are.
pub fn program(exheader: &ExHeader, code: &[u8], changed: &[(u32, u32)]) -> Program {
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
    let end = changed.iter().map(|&(_, end)| end).filter(|&end| end > text_end).max().unwrap_or(text_end);
    let mut text = image.text;
    if end > text_end {
        text.bytes.extend((text_end..(end + 3) & !3).map(byte_at));
    }
    let in_code = |value: u32| (value & !1) >= text_start && (value & !1) < text_end || meets(changed, value & !1, (value & !1) + 2);
    let mut seeds = vec![(image.entry, Source::Entry)];
    seeds.extend(
        [&image.rodata, &image.data]
            .into_iter()
            .flat_map(Segment::words)
            .filter(|&(_, value)| in_code(value) && text.contains(value & !1))
            .map(|(_, value)| (value, Source::Pointer)),
    );
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
    /// nor the module has code for, which a module must have none of.
    pub holes: Vec<u32>,
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

    for &index in &stale {
        let function = &base.functions[index];
        program.seeds.push((function.start | function.thumb as u32, Source::Hint));
    }
    let mut analysis = discover::analyze(&program);
    // entries of changed functions nothing in the patched code holds start
    // functions of their own, when they are instructions
    let holds = |analysis: &Analysis, key: u32| {
        let (address, mode) = (key & !1, mode_of(key));
        analysis.functions.values().any(|f| f.mode == mode && f.instructions.binary_search(&address).is_ok())
    };
    let unheld: Vec<u32> = stale_entries.iter().copied().filter(|&key| !holds(&analysis, key)).collect();
    if !unheld.is_empty() {
        program.seeds.extend(unheld.iter().map(|&key| (key, Source::Hint)));
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
    let mut selected: BTreeSet<u32> = BTreeSet::new();
    let mut new_instructions = 0;
    for (&entry, function) in &analysis.functions {
        if !codegen::recompiles(entry, function) {
            continue;
        }
        let spans = &held[(function.mode == Mode::Thumb) as usize];
        let mut take = function.labels.iter().any(|&label| stale_set.contains(&key(label, function.mode)));
        let mut fresh = 0;
        for &address in &function.instructions {
            let end = address + size(&program.text, address, function.mode);
            if meets(&changed, address, end) {
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
                    holes.insert(to);
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
    (Plan { functions, stale, replaced, missing, new_instructions, holes: holes.into_iter().collect() }, program)
}

/// instructions per source file, as codegen splits them.
const FILE_SIZE: usize = 4_000;

/// the C of a patch module for plan, as file names and contents: recomp.h,
/// the functions in files of their own, and patch.c with the table of
/// every entry the module has code for. key is what the host checks the
/// module against: its base digest and the patched code. an error names an
/// instruction the module's code would have left to the interpreter, which
/// a host without one cannot run.
pub fn source(plan: &Plan, program: &Program, base: &Base, base_digest: u64, code_key: u64) -> Result<Vec<(String, String)>, String> {
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

    let mut files = vec![("recomp.h".to_owned(), codegen::HEADER.to_owned()), ("patch.h".to_owned(), header)];
    let mut source = String::new();
    let mut size = 0;
    let mut sources = Vec::new();
    let mut interpreted = Vec::new();
    for (&entry, function) in &plan.functions {
        if source.is_empty() {
            source.push_str("#define RECOMP_IMPORTED 1\n#include \"recomp.h\"\n#include \"patch.h\"\n\n");
        }
        interpreted.extend(codegen::write_patch_function(&mut source, program, &names, PREFIX, entry, function));
        size += function.instructions.len();
        if size >= FILE_SIZE {
            sources.push(std::mem::take(&mut source));
            size = 0;
        }
    }
    if !source.is_empty() {
        sources.push(source);
    }
    if let Some(first) = interpreted.first() {
        return Err(format!("the patched code has {} instructions Haiku can't recompile, the first at 0x{first:08X}", interpreted.len()));
    }
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
    files.push(("patch.c".to_owned(), table));
    files.extend(sources.into_iter().enumerate().map(|(i, source)| (format!("patch{i:03}.c"), source)));
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: u32 = 0x0010_0000;

    fn segment(words: &[u32]) -> Segment {
        Segment { base: BASE, bytes: words.iter().flat_map(|w| w.to_le_bytes()).collect() }
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
        let files = source(&plan, &program, &base, base.digest(), 7).unwrap();
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
        let files = source(&plan, &program, &base, 0, 0).unwrap();
        let header = &files.iter().find(|(name, _)| name == "patch.h").unwrap().1;
        assert!(header.contains("override_stub_00100008"));
    }
}
