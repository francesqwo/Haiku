//! Generate a standalone project, rebuilding only changed C translation units.
use std::path::{Path, PathBuf};
use recomp3ds::{build, compile, port, Mods};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 && args.len() != 5 {
        return Err("usage: fates_port <rom> <project> <zakuro checkout> [overrides dir]".into());
    }
    // hand-written C replacing generated functions, see docs/overrides.md
    let overrides = args.get(4).map(Path::new).filter(|p| p.exists());
    let project = Path::new(&args[2]);
    let code = project.join("code");
    let regenerated = project.join("regenerated");
    let generated = build::generate(Path::new(&args[1]), |_| regenerated.clone(), overrides, Mods::default(), &|e| println!("{e:?}"))?;
    std::fs::create_dir_all(&code)?;
    let mut changed = Vec::new();
    // input: overrides come out in a subfolder (hand/), so copy recursively
    let mut folders = vec![(regenerated.clone(), code.clone())];
    while let Some((from, to)) = folders.pop() {
        std::fs::create_dir_all(&to)?;
        for entry in std::fs::read_dir(&from)? {
            let entry = entry?;
            let target = to.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                folders.push((entry.path(), target));
                continue;
            }
            let bytes = std::fs::read(entry.path())?;
            if std::fs::read(&target).ok().as_deref() != Some(bytes.as_slice()) {
                println!("updated {}", target.display());
                std::fs::write(&target, bytes)?;
                changed.push(target);
            }
        }
    }
    let headers_changed = changed.iter().any(|p| p.extension().is_some_and(|e| e == "h"));
    let pending: Vec<String> = generated.sources.iter().filter(|source| {
        let path = code.join(source);
        // codemods: an object older than its source (a run killed between
        // writing the C and compiling it) is compiled again too
        let stale_object = || {
            let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
            modified(&path.with_extension("o")).zip(modified(&path)).is_none_or(|(object, source)| object < source)
        };
        headers_changed || changed.contains(&path) || stale_object()
    }).cloned().collect();
    let compiler = compile::check(None)?;
    println!("compiling {} of {} files", pending.len(), generated.sources.len());
    compile::objects(&compiler, &code, &pending, &|done, total| { println!("compiled {done}/{total}"); true }, true)?;
    let objects: Vec<PathBuf> = generated.sources.iter().map(|s| code.join(s).with_extension("o")).collect();
    compile::archive(&objects, &code.join("librecomp.a"))?;
    if !project.join("Cargo.toml").exists() {
        port::write_project(project, "fates-pc", "Fire Emblem Fates Special Edition USA", generated.title.program_id(), Some(Path::new(&args[3])))?;
    }
    // This directory contains only the temporary output of this generation.
    let canonical_project = std::fs::canonicalize(project)?;
    let canonical_generated = std::fs::canonicalize(&regenerated)?;
    if canonical_generated.parent() == Some(canonical_project.as_path()) {
        std::fs::remove_dir_all(&canonical_generated)?;
    }
    println!("standalone source ready at {}", project.display());
    Ok(())
}
