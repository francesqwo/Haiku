//! codemods2: a code mod's patch plan away from the game, from what Haiku
//! wrote with HAIKU_CODEMODS_DUMP=<dir> (base.txt, patched.bin,
//! exheader.bin, changed.txt): the functions it recompiles, the entries it
//! has no code for, and every hole with the code that goes there.
//! usage: patch_plan <dir> [address ...]   (addresses: print the functions
//! of the plan holding them)
use recomp3ds::discover::Mode;
use recomp3ds::patch;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::path::PathBuf::from(args.get(1).ok_or("usage: patch_plan <dir> [address ...]")?);
    let base = patch::Base::from_text(&std::fs::read_to_string(dir.join("base.txt"))?).ok_or("base.txt doesn't read")?;
    let code = std::fs::read(dir.join("patched.bin"))?;
    let exheader = recomp3ds::rom::ExHeader::read(&std::fs::read(dir.join("exheader.bin"))?);
    let changed: Vec<(u32, u32)> = std::fs::read_to_string(dir.join("changed.txt"))?
        .lines()
        .filter_map(|line| {
            let (a, b) = line.split_once(' ')?;
            Some((u32::from_str_radix(a, 16).ok()?, u32::from_str_radix(b, 16).ok()?))
        })
        .collect();
    let started = std::time::Instant::now();
    let program = patch::program(&exheader, &code, &changed);
    let (made, program) = patch::plan(&base, program, &changed);
    println!(
        "{} functions changed, {} to recompile, {} new instructions, {} entries with no code now, {} of Haiku's own replaced, {} holes, {} dead ends, {:.1} s",
        made.stale.len(),
        made.functions.len(),
        made.new_instructions,
        made.missing.len(),
        made.replaced.len(),
        made.holes.len(),
        made.dead_ends.len(),
        started.elapsed().as_secs_f32()
    );
    for &missing in &made.missing {
        println!("missing 0x{missing:08X}");
    }
    for hole in made.holes.iter().chain(&made.dead_ends) {
        let function = made.functions.get(&hole.function);
        println!(
            "hole {:?} 0x{:08X} <- 0x{:08X} {:?} in 0x{:08X} ({} instructions, source {:?}) decodes {}",
            hole.mode,
            hole.to,
            hole.from,
            hole.way,
            hole.function,
            function.map_or(0, |f| f.instructions.len()),
            function.map(|f| f.source),
            recomp3ds::discover::decodes(&program.text, hole.to, hole.mode)
        );
    }
    if std::env::var("PLAN_FUNCTIONS").is_ok() {
        for (entry, function) in &made.functions {
            let first = function.instructions.first().copied().unwrap_or(0);
            let last = function.instructions.last().copied().unwrap_or(0);
            println!("function 0x{entry:08X} {:?} {:?} {} instructions 0x{first:08X}..0x{last:08X}", function.mode, function.source, function.instructions.len());
        }
    }
    for wanted in args.iter().skip(2).filter_map(|word| u32::from_str_radix(word.trim_start_matches("0x"), 16).ok()) {
        for (entry, function) in &made.functions {
            if function.instructions.binary_search(&(wanted & !1)).is_ok() {
                println!("0x{wanted:08X} is in 0x{entry:08X} {:?} {:?}, labels {:X?}", function.mode, function.source, function.labels);
                let stale = base.functions.iter().position(|f| f.spans.iter().any(|&(s, e)| s <= wanted && wanted < e));
                println!("  the program's function there: {:?}", stale.map(|i| (format!("0x{:08X}", base.functions[i].start), base.functions[i].thumb)));
            }
        }
    }
    // the changed bytes of .text no function of the module holds, in runs
    if std::env::var("PLAN_UNCOVERED").is_ok() {
        let mut held = std::collections::BTreeSet::new();
        for function in made.functions.values() {
            for &address in &function.instructions {
                let size = if function.mode == Mode::Arm { 4 } else { 2 };
                for byte in address..address + size {
                    held.insert(byte);
                }
            }
        }
        let text_end = exheader.text.address + exheader.text.num_pages * 0x1000;
        for &(start, end) in changed.iter().filter(|&&(start, _)| start < text_end) {
            let mut run: Option<u32> = None;
            for byte in start..=end {
                let out = byte < end && !held.contains(&byte);
                match (out, run) {
                    (true, None) => run = Some(byte),
                    (false, Some(from)) => {
                        println!("uncovered 0x{from:08X}..0x{byte:08X} ({} bytes) of 0x{start:08X}..0x{end:08X}", byte - from);
                        run = None;
                    }
                    _ => {}
                }
            }
        }
    }
    // the words the patch wrote in .text that point into .text, and whether
    // the program's unchanged code or the module runs there
    if std::env::var("PLAN_POINTERS").is_ok() {
        let stale: std::collections::BTreeSet<usize> = made.stale.iter().copied().collect();
        let mut labels = std::collections::BTreeSet::new();
        for function in made.functions.values() {
            let thumb = (function.mode == Mode::Thumb) as u32;
            labels.extend(function.labels.iter().map(|&label| label | thumb));
        }
        for &(start, end) in &changed {
            let mut at = start & !3;
            while at < end && at < program.text.end() {
                if let Some(value) = program.text.read32(at).filter(|&value| program.text.contains(value & !1) && value & 3 != 2) {
                    let base_runs = match base.entries.get(&value) {
                        Some(None) => "override",
                        Some(Some(owner)) if !stale.contains(owner) => "program",
                        Some(Some(_)) => "stale",
                        None => "none",
                    };
                    let module = labels.contains(&value);
                    if !module && base_runs != "program" && base_runs != "override" {
                        println!("pointer at 0x{at:08X} = 0x{value:08X}: program {base_runs}, module {module}");
                    }
                }
                at += 4;
            }
        }
    }
    // the C, as Haiku would write it
    match patch::source(&made, &program, &base, base.digest(), 0) {
        Ok(module) => println!(
            "source: {} files, {} lines; {} instructions left to the host{}",
            module.files.len(),
            module.files.iter().map(|(_, text)| text.lines().count()).sum::<usize>(),
            module.left_to_host.len(),
            module.left_to_host.first().map(|first| format!(", the first at 0x{first:08X}")).unwrap_or_default()
        ),
        Err(error) => println!("source: {error}"),
    }
    Ok(())
}
