//! oneexe: Haiku's overrides compiled into Haiku.exe (build.rs), apart from
//! the game's code, which the player's PC recompiles into a module of its
//! own (data/code/game.dll, see src/module.rs).
//!
//! The overrides are written against the USA build, with the game's
//! addresses as literals. For one Haiku.exe to serve the USA, EUR and JPN
//! builds, every game address in them is made a variable the exe fills when
//! it binds the module, through the region's address map (the same tables,
//! with the same reading, that dev/region/translate.py and Haiku Setup's
//! text translation used): each `0x00[1-7]XXXXX` literal outside comments
//! and strings becomes `haiku_ga_XXXXXXXX` (an `extern uint32_t`; a
//! literal without `u` was an int: every use was checked to read the same
//! with an unsigned, see dev/reports/oneexe.md), and each
//! `f_XXXXXXXX` (a generated function an override calls by name) becomes
//! `haiku_gf_XXXXXXXX` (an `extern Code` the exe fills from the module's
//! `recomp_hosted_functions`). The literals inside RECOMP_OVERRIDE(...) and
//! RECOMP_ORIGINAL(...) stay: they name functions, and the exe binds each
//! override by its USA address. A literal used where C needs a constant (a
//! `case`, a static initializer) can't be a variable, and fails to compile
//! here, never silently.
//!
//! What this writes into the build folder (`ov/`):
//!
//! - each override file, transformed;
//! - `overrides.h`: recomp.h (built with RECOMP_IMPORTED, the tails
//!   registry reached through recomp_imports, which lives in the module),
//!   the declarations above, and `original_<address>` for every override,
//!   which calls the module's original through `haiku_originals[k]`;
//! - `haiku_hosted.c`: the variables, and the tables the exe binds by
//!   (`haiku_hosted_overrides`, `haiku_hosted_addresses`,
//!   `haiku_hosted_functions`);
//! - `hosted_interface.rs` (in OUT_DIR): the same lists for the Rust side,
//!   and the stamp of what the module has to be built from.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::path::Path;

pub struct Scanned {
    /// (file name, transformed text)
    pub files: Vec<(String, String)>,
    /// every override's address (odd for Thumb), sorted, and the literal it
    /// was written with
    pub overrides: BTreeMap<u32, String>,
    /// every game address made a variable
    pub addresses: BTreeSet<u32>,
    /// every generated function called by name (ARM, even)
    pub functions: BTreeSet<u32>,
}

fn is_hex(b: u8) -> bool {
    b.is_ascii_hexdigit()
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// one piece of C code (no comments, no strings), as eur.rs's
/// translate_code reads it: every game address and f_ name replaced.
fn transform_code(code: &str, out: &mut String, scan: &mut Scanned) {
    let b = code.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let before_ok = i == 0 || !is_word(b[i - 1]);
        if before_ok && b[i..].starts_with(b"0x00") && i + 10 <= b.len() && (b'1'..=b'7').contains(&b[i + 4]) && b[i + 5..i + 10].iter().all(|&c| is_hex(c)) {
            let mut end = i + 10;
            let suffix = end < b.len() && (b[end] == b'u' || b[end] == b'U');
            if suffix {
                end += 1;
            }
            if end >= b.len() || !is_word(b[end]) {
                let address = u32::from_str_radix(&code[i + 2..i + 10], 16).unwrap();
                // RECOMP_OVERRIDE( and RECOMP_ORIGINAL( keep theirs: they
                // name functions
                let head = out.trim_end();
                if head.ends_with("RECOMP_OVERRIDE(") || head.ends_with("RECOMP_ORIGINAL(") {
                    out.push_str(&code[i..end]);
                } else {
                    // one name whatever the suffix: a macro's argument is
                    // expanded before RECOMP_ORIGINAL pastes it into a name
                    // (os_fs.c's OS_NATIVE), so it has to stay one token
                    scan.addresses.insert(address);
                    let _ = write!(out, "haiku_ga_{address:08X}");
                }
                i = end;
                continue;
            }
        }
        if before_ok && b[i..].starts_with(b"f_00") && i + 10 <= b.len() && (b'1'..=b'7').contains(&b[i + 4]) && b[i + 5..i + 10].iter().all(|&c| c.is_ascii_digit() || (b'A'..=b'F').contains(&c)) && (i + 10 >= b.len() || !is_word(b[i + 10])) {
            let address = u32::from_str_radix(&code[i + 2..i + 10], 16).unwrap();
            scan.functions.insert(address);
            let _ = write!(out, "haiku_gf_{address:08X}");
            i += 10;
            continue;
        }
        let ch = code[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
}

/// a C file's text with its game addresses made variables outside comments
/// and strings (the reading of eur.rs's translate_c).
fn transform_c(text: &str, scan: &mut Scanned) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 8);
    let mut in_block = false;
    for line in text.split_inclusive('\n') {
        let mut rest = line;
        while !rest.is_empty() {
            if in_block {
                match rest.find("*/") {
                    Some(at) => {
                        out.push_str(&rest[..at + 2]);
                        rest = &rest[at + 2..];
                        in_block = false;
                    }
                    None => {
                        out.push_str(rest);
                        rest = "";
                    }
                }
                continue;
            }
            let next = [rest.find("/*"), rest.find("//"), rest.find('"')].into_iter().flatten().min();
            let Some(at) = next else {
                transform_code(rest, &mut out, scan);
                break;
            };
            transform_code(&rest[..at], &mut out, scan);
            rest = &rest[at..];
            if rest.starts_with('"') {
                let bytes = rest.as_bytes();
                let mut k = 1;
                while k < bytes.len() && bytes[k] != b'"' {
                    k += if bytes[k] == b'\\' { 2 } else { 1 };
                }
                let end = (k + 1).min(rest.len());
                out.push_str(&rest[..end]);
                rest = &rest[end..];
            } else if rest.starts_with("//") {
                out.push_str(rest);
                rest = "";
            } else {
                in_block = true;
                out.push_str("/*");
                rest = &rest[2..];
            }
        }
    }
    // the generated functions' prototypes the files give themselves
    // (`void f_XXXXXXXX(Context *ctx);`) are the exe's variables now
    let mut cleaned = String::with_capacity(out.len());
    for line in out.split_inclusive('\n') {
        let t = line.trim();
        if t.starts_with("void haiku_gf_") && t.ends_with(';') && t.contains("(Context") {
            cleaned.push_str("/* oneexe: a generated function's prototype, the exe's variable now (overrides.h) */\n");
        } else {
            cleaned.push_str(line);
        }
    }
    cleaned
}

/// the overrides a file marks, by address, with their literals (the
/// recompiler's own parse is stricter; this one only has to agree on
/// RECOMP_OVERRIDE(0x........)).
fn marks(text: &str, overrides: &mut BTreeMap<u32, String>, file: &str) {
    let code = strip_comments(text);
    let mut rest = code.as_str();
    while let Some(at) = rest.find("RECOMP_OVERRIDE") {
        let before = rest[..at].chars().next_back();
        rest = &rest[at + "RECOMP_OVERRIDE".len()..];
        if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        if rest.starts_with("_IN") {
            panic!("{file}: RECOMP_OVERRIDE_IN is not supported by the hosted overrides (oneexe)");
        }
        let Some(after) = rest.trim_start().strip_prefix('(') else { continue };
        let Some(end) = after.find(')') else { panic!("{file}: a RECOMP_OVERRIDE is not closed") };
        let literal = after[..end].trim();
        let hex = literal.strip_prefix("0x").or_else(|| literal.strip_prefix("0X")).unwrap_or_else(|| panic!("{file}: RECOMP_OVERRIDE({literal}) is not an address"));
        let address = u32::from_str_radix(hex, 16).unwrap_or_else(|_| panic!("{file}: RECOMP_OVERRIDE({literal}) is not an address"));
        if overrides.insert(address, literal.to_owned()).is_some() {
            panic!("{file}: 0x{address:08X} is overridden twice");
        }
    }
}

fn strip_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut last = ' ';
                for c in chars.by_ref() {
                    if last == '*' && c == '/' {
                        break;
                    }
                    last = c;
                }
                out.push(' ');
            }
            '"' => {
                out.push('"');
                let mut escaped = false;
                for c in chars.by_ref() {
                    if escaped {
                        escaped = false;
                    } else if c == '\\' {
                        escaped = true;
                    } else if c == '"' {
                        break;
                    }
                }
                out.push('"');
            }
            c => out.push(c),
        }
    }
    out
}

/// every override file in dir but the game-derived amiibo_icons.c, read and
/// transformed.
pub fn scan(dir: &Path) -> Scanned {
    let mut scan = Scanned { files: Vec::new(), overrides: BTreeMap::new(), addresses: BTreeSet::new(), functions: BTreeSet::new() };
    let mut paths: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "c")).collect();
    paths.sort();
    for path in paths {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name == "amiibo_icons.c" {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
        marks(&text, &mut scan.overrides, &name);
        let out = transform_c(&text, &mut scan);
        scan.files.push((name, out));
    }
    scan
}

/// overrides.h for the exe's overrides.
pub fn overrides_h(scan: &Scanned) -> String {
    let mut h = String::from(
        "/* oneexe: what Haiku's overrides include when they are compiled into Haiku.exe (source/hosted_build.rs). */\n\
         #ifndef HAIKU_HOSTED_OVERRIDES_H\n#define HAIKU_HOSTED_OVERRIDES_H\n\n\
         #ifndef RECOMP_IMPORTED\n#define RECOMP_IMPORTED 1\n#endif\n#include \"recomp.h\"\n\n\
         /* the game's addresses, the running build's (filled when the module is bound) */\n",
    );
    for address in &scan.addresses {
        let _ = writeln!(h, "extern uint32_t haiku_ga_{address:08X};");
    }
    h += "\n/* the module's generated functions the overrides call by name */\n";
    for address in &scan.functions {
        let _ = writeln!(h, "extern Code haiku_gf_{address:08X};");
    }
    let _ = writeln!(h, "\n/* the module's originals of the overrides (recomp_hosted_overrides), in the order of haiku_hosted_overrides */\nextern Code haiku_originals[{}];", scan.overrides.len().max(1));
    for (k, (address, literal)) in scan.overrides.iter().enumerate() {
        let _ = writeln!(h, "void override_{literal}(Context *ctx);");
        let mut spellings = BTreeSet::new();
        spellings.insert(literal.clone());
        spellings.insert(format!("0x{address:08X}"));
        spellings.insert(format!("0x{address:08x}"));
        spellings.insert(format!("haiku_ga_{address:08X}"));
        for spelling in spellings {
            let _ = writeln!(h, "#define original_{spelling} (haiku_originals[{k}])");
        }
    }
    h += "\n#endif\n";
    h
}

/// haiku_hosted.c: the variables and the tables the exe binds by.
pub fn hosted_c(scan: &Scanned) -> String {
    let mut c = String::from("/* oneexe: the variables and tables Haiku.exe binds the game's code module by (source/hosted_build.rs, src/hosted.rs). */\n#include \"overrides.h\"\n\n");
    for address in &scan.addresses {
        let _ = writeln!(c, "uint32_t haiku_ga_{address:08X} = 0x{address:08X}u;");
    }
    for address in &scan.functions {
        let _ = writeln!(c, "Code haiku_gf_{address:08X};");
    }
    let _ = writeln!(c, "Code haiku_originals[{}];\n", scan.overrides.len().max(1));
    c += "/* the tails registry, the module's, reached through these (recomp.h, RECOMP_IMPORTED) */\nRecompImports recomp_imports;\n\n";
    c += "/* amiibo: the icons made from the dump, which the module carries (amiibo.c) */\nuint16_t amiibo_icons[4][2304];\n\n";
    c += "typedef struct { uint32_t address; Code code; } HaikuHostedOverride;\ntypedef struct { uint32_t address; uint32_t *at; } HaikuHostedAddress;\ntypedef struct { uint32_t address; Code *at; } HaikuHostedFunction;\n\n";
    let _ = writeln!(c, "const uint32_t haiku_hosted_override_count = {};", scan.overrides.len());
    let _ = write!(c, "const HaikuHostedOverride haiku_hosted_overrides[{}] = {{", scan.overrides.len().max(1));
    for (address, literal) in &scan.overrides {
        let _ = write!(c, "\n    {{0x{address:08X}u, override_{literal}}},");
    }
    c += "\n};\n";
    let _ = writeln!(c, "const uint32_t haiku_hosted_address_count = {};", scan.addresses.len());
    let _ = write!(c, "const HaikuHostedAddress haiku_hosted_addresses[{}] = {{", scan.addresses.len().max(1));
    for address in &scan.addresses {
        let _ = write!(c, "\n    {{0x{address:08X}u, &haiku_ga_{address:08X}}},");
    }
    if scan.addresses.is_empty() {
        c += "{0, 0}";
    }
    c += "\n};\n";
    let _ = writeln!(c, "const uint32_t haiku_hosted_function_count = {};", scan.functions.len());
    let _ = write!(c, "const HaikuHostedFunction haiku_hosted_functions[{}] = {{", scan.functions.len().max(1));
    for address in &scan.functions {
        let _ = write!(c, "\n    {{0x{address:08X}u, &haiku_gf_{address:08X}}},");
    }
    if scan.functions.is_empty() {
        c += "{0, 0}";
    }
    c += "\n};\n";
    c
}

/// 64-bit FNV-1a.
pub fn fnv(hash: &mut u64, bytes: &[u8]) {
    for &byte in bytes {
        *hash = (*hash ^ byte as u64).wrapping_mul(0x0100_0000_01B3);
    }
}

pub const FNV_START: u64 = 0xCBF2_9CE4_8422_2325;

/// what the module's code is made by: the recompiler's sources, its
/// interface and the hints it carries (line endings left out, so a CRLF
/// checkout builds the same module).
pub fn recompiler_hash(recomp: &Path, rerun: &mut Vec<String>) -> u64 {
    let mut files = Vec::new();
    let mut dirs = vec![recomp.join("src")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|x| x == "rs") && path.file_name().is_none_or(|name| name != "patch.rs") {
                // vow: src/patch.rs writes only code mods' patch modules
                // (their cache key has patch::VERSION), never the game's
                // module, so a fix to it doesn't make every player build
                // the game's module again
                files.push(path);
            }
        }
    }
    files.push(recomp.join("abi").join("recomp.h"));
    files.push(recomp.join("abi").join("src").join("lib.rs"));
    files.push(recomp.join("hints.txt"));
    files.sort();
    let mut hash = FNV_START;
    for path in files {
        let rel = path.strip_prefix(recomp).unwrap().to_string_lossy().replace('\\', "/");
        // tests don't change the code it writes, but are in the same files:
        // the whole file counts, which errs on the side of building again
        let text = std::fs::read(&path).unwrap();
        let text: Vec<u8> = text.into_iter().filter(|&b| b != b'\r').collect();
        fnv(&mut hash, rel.as_bytes());
        fnv(&mut hash, &text);
        rerun.push(path.to_string_lossy().into_owned());
    }
    hash
}

/// the Rust side's lists and stamp (OUT_DIR/hosted_interface.rs).
pub fn interface_rs(scan: &Scanned, recompiler: u64, flags: &[&str]) -> String {
    let mut iface = FNV_START;
    for address in scan.overrides.keys() {
        fnv(&mut iface, &address.to_le_bytes());
    }
    fnv(&mut iface, b"|");
    for address in &scan.functions {
        fnv(&mut iface, &address.to_le_bytes());
    }
    let mut r = String::from("// oneexe: made by build.rs (hosted_build.rs) from the overrides\n");
    let _ = writeln!(r, "/// the overrides' USA addresses (odd for Thumb), sorted: the module has a slot for each\npub const OVERRIDES: &[u32] = &[{}];", scan.overrides.keys().map(|a| format!("0x{a:08X}")).collect::<Vec<_>>().join(", "));
    let _ = writeln!(r, "/// the generated functions the overrides call by name (USA)\npub const FUNCTIONS: &[u32] = &[{}];", scan.functions.iter().map(|a| format!("0x{a:08X}")).collect::<Vec<_>>().join(", "));
    let _ = writeln!(r, "/// what the module is built by (the recompiler's sources, recomp.h, hints.txt)\npub const RECOMPILER: u64 = 0x{recompiler:016X};");
    let _ = writeln!(r, "/// the overrides' addresses and named functions, the module's side of the boundary\npub const INTERFACE: u64 = 0x{iface:016X};");
    let _ = writeln!(r, "/// the flags the exe's overrides were compiled with (the module's code takes the recompiler's)\npub const FLAGS: &str = {:?};", flags.join(" "));
    r
}
