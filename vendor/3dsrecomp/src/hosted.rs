//! oneexe: overrides a host program links into itself.
//!
//! Normally the overrides are C files compiled with the title's code (see
//! docs/overrides.md). A host that ships its overrides already compiled,
//! and builds only the title's code on the player's machine, gives the
//! recompiler their addresses instead ([`crate::build::Hosted`]). The
//! library then has, in each override's place, a function that goes on
//! through a slot (`recomp_hosted_slots`), so calls and the entries table
//! reach the override as they would reach one compiled in, and these tables
//! for the host to bind by when it loads the library:
//!
//! - `recomp_hosted_overrides`: each override's module (null for the
//!   executable), address, slot, and its original (the generated function
//!   entered where the override replaces it, as overrides.h's original_
//!   wrapper does; null when no code was generated there). The host writes
//!   its override into the slot and calls the original through the pointer.
//!   A slot starts out holding the original, so an override the host does
//!   not bind runs the title's own code;
//! - `recomp_hosted_functions`: the generated function that runs each
//!   address the host's overrides call by name (f_XXXXXXXX);
//! - `recomp_hosted_imports`: the tails registry (recomp.h, CALL_THEN), for
//!   the host's overrides, built with RECOMP_IMPORTED, to share;
//! - `recomp_hosted_version` (this layout, [`VERSION`]),
//!   `recomp_hosted_title` (the title's program id) and
//!   `recomp_hosted_stamp` (what the host built the library from, given in
//!   `Hosted::stamp`).

use std::fmt::Write;

use crate::build::Hosted;
use crate::codegen::Named;
use crate::overrides::Override;

/// the file the slots and tables are written to.
pub const FILE: &str = "hosted.c";

/// the layout of the tables; a host refuses a library of another.
pub const VERSION: u32 = 1;

/// the override item for an address the host overrides, named as
/// RECOMP_OVERRIDE would name it written with 8 uppercase hex digits.
pub fn override_of(module: Option<&str>, address: u32) -> Override {
    match module {
        None => Override {
            module: None,
            address,
            name: format!("override_0x{address:08X}"),
            original: format!("original_0x{address:08X}"),
        },
        Some(module) => Override {
            module: Some(module.to_owned()),
            address,
            name: format!("override_{module}_0x{address:X}"),
            original: format!("original_{module}_0x{address:X}"),
        },
    }
}

/// hosted.c: the slots, the functions in the overrides' places and the
/// tables, for the overrides and functions the host gave and what codegen
/// named.
pub fn source(hosted: &Hosted, named: &Named, program_id: u64) -> String {
    let mut out = String::new();
    let o = &mut out;
    // overrides.h, with the originals, is written whenever there are any
    let has_overrides_h = !hosted.overrides.is_empty();
    writeln!(o, "/* oneexe: the overrides the host links into itself go on from here, see hosted.rs. */\n").unwrap();
    if has_overrides_h {
        writeln!(o, "#include \"overrides.h\"").unwrap();
    }
    writeln!(o, "#include \"recomp.h\"\n#include \"functions.h\"\n#include <stdio.h>\n#include <stdlib.h>\n").unwrap();
    writeln!(o, "typedef struct {{ const char *module; uint32_t address; Code *slot; Code original; }} RecompHostedOverride;").unwrap();
    writeln!(o, "typedef struct {{ uint32_t address; Code code; }} RecompHostedFunction;").unwrap();
    writeln!(
        o,
        "typedef struct {{\n    uint32_t *tails_pending;\n    uint8_t *tails_counting;\n    void (*tail_defer)(Context *ctx, uint32_t ret, uint32_t sp, RecompTail tail, const char *name, const uint32_t *words);\n    void (*tails_due)(Context *ctx);\n    void (*tail_count)(RecompTail tail, const char *name, int how);\n}} RecompHostedImports;\n"
    )
    .unwrap();
    writeln!(o, "RECOMP_EXPORT const uint32_t recomp_hosted_version = {VERSION}u;").unwrap();
    writeln!(o, "RECOMP_EXPORT const uint64_t recomp_hosted_title = 0x{program_id:016X}ull;").unwrap();
    writeln!(o, "RECOMP_EXPORT const char recomp_hosted_stamp[] = \"{}\";", escape(hosted.stamp)).unwrap();
    writeln!(o, "RECOMP_EXPORT const RecompHostedImports recomp_hosted_imports = {{&recomp_tails_pending, &recomp_tails_counting, recomp_tail_defer, recomp_tails_due, recomp_tail_count}};\n").unwrap();
    // an override with no original, and no binding: there is nothing to run
    writeln!(
        o,
        "static void recomp_hosted_unbound(Context *ctx) {{\n    fprintf(stderr, \"[hosted] an override at %08X was never bound by the host\\n\", ctx->r[15]);\n    abort();\n}}\n"
    )
    .unwrap();

    let items: Vec<Override> = hosted.overrides.iter().map(|(module, address)| override_of(module.as_deref(), *address)).collect();
    let count = items.len();
    let has_original = |item: &Override| item.module.is_none() && named.originals.contains(&item.address);
    write!(o, "RECOMP_EXPORT Code recomp_hosted_slots[{}] = {{", count.max(1)).unwrap();
    for (i, item) in items.iter().enumerate() {
        if i % 4 == 0 {
            write!(o, "\n   ").unwrap();
        }
        write!(o, " {},", if has_original(item) { item.original.as_str() } else { "recomp_hosted_unbound" }).unwrap();
    }
    if count == 0 {
        write!(o, " recomp_hosted_unbound").unwrap();
    }
    writeln!(o, "\n}};\n").unwrap();
    for (i, item) in items.iter().enumerate() {
        writeln!(o, "void {}(Context *ctx) {{ recomp_hosted_slots[{i}](ctx); }}", item.name).unwrap();
    }
    writeln!(o).unwrap();
    writeln!(o, "RECOMP_EXPORT const uint32_t recomp_hosted_override_count = {count}u;").unwrap();
    write!(o, "RECOMP_EXPORT const RecompHostedOverride recomp_hosted_overrides[{}] = {{", count.max(1)).unwrap();
    for (i, item) in items.iter().enumerate() {
        let module = item.module.as_deref().map_or("0".to_owned(), |m| format!("\"{}\"", escape(m)));
        let original = if has_original(item) { item.original.as_str() } else { "0" };
        write!(o, "\n    {{{module}, 0x{:08X}u, &recomp_hosted_slots[{i}], {original}}},", item.address).unwrap();
    }
    if count == 0 {
        write!(o, "\n    {{0, 0, 0, 0}}").unwrap();
    }
    writeln!(o, "\n}};\n").unwrap();

    let functions: Vec<&(u32, Option<String>)> = named.functions.iter().collect();
    writeln!(o, "RECOMP_EXPORT const uint32_t recomp_hosted_function_count = {}u;", functions.len()).unwrap();
    write!(o, "RECOMP_EXPORT const RecompHostedFunction recomp_hosted_functions[{}] = {{", functions.len().max(1)).unwrap();
    for (address, name) in &functions {
        write!(o, "\n    {{0x{address:08X}u, {}}},", name.as_deref().unwrap_or("0")).unwrap();
    }
    if functions.is_empty() {
        write!(o, "\n    {{0, 0}}").unwrap();
    }
    writeln!(o, "\n}};").unwrap();
    out
}

fn escape(text: &str) -> String {
    text.chars()
        .flat_map(|c| match c {
            '"' | '\\' => vec!['\\', c],
            c if c.is_ascii_graphic() || c == ' ' => vec![c],
            _ => vec!['?'],
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn the_tables_name_every_override_and_function() {
        let overrides = [(None, 0x0010_0000), (None, 0x0010_0011)];
        let hosted = Hosted { overrides: &overrides, functions: &[0x0010_0020], stamp: "a \"b\"", extra: &[] };
        let named = Named { originals: BTreeSet::from([0x0010_0000]), functions: vec![(0x0010_0020, Some("f_00100020".into()))] };
        let c = source(&hosted, &named, 0x0004_0000_0017_9800);
        assert!(c.contains("void override_0x00100000(Context *ctx) { recomp_hosted_slots[0](ctx); }"), "{c}");
        assert!(c.contains("void override_0x00100011(Context *ctx) { recomp_hosted_slots[1](ctx); }"), "{c}");
        assert!(c.contains("{0, 0x00100000u, &recomp_hosted_slots[0], original_0x00100000},"), "{c}");
        assert!(c.contains("{0, 0x00100011u, &recomp_hosted_slots[1], 0},"), "{c}");
        assert!(c.contains("{0x00100020u, f_00100020},"), "{c}");
        assert!(c.contains("recomp_hosted_stamp[] = \"a \\\"b\\\"\";"), "{c}");
        assert!(c.contains("0x0004000000179800ull"), "{c}");
    }
}
