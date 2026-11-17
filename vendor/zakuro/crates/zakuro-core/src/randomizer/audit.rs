//! rando4: test helpers for the talk-line pass (development only):
//! FATES_RANDO_CASTDUMP=<file> writes the cast a run's talk lines know (who
//! is who, who is shown in their place, families), so the pass and the
//! re-wrap can be run offline over every message of the game
//! (dev/randomizer/textcheck).

use super::text::Cast;

pub fn dump_cast(cast: &Cast, path: &std::ffi::OsStr) {
    let s = |v: &[u16]| String::from_utf16_lossy(v);
    let list = |v: &[usize]| v.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let mut out = String::new();
    for (i, p) in cast.people.iter().enumerate() {
        out.push_str(&format!(
            "P\t{i}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            s(&p.name),
            s(&p.key),
            p.female as u8,
            p.child as u8,
            s(&p.shown_name),
            p.shown_female as u8,
            p.either as u8,
            list(&p.siblings),
            list(&p.parents),
            list(&p.children)
        ));
    }
    for (name, g) in &cast.extra {
        out.push_str(&format!("X\t{}\t{}\n", s(name), g.map_or("?".to_string(), |g| (g as u8).to_string())));
    }
    if let Some(c) = cast.corrin {
        out.push_str(&format!("C\t{c}\t{}\n", cast.corrin_name.as_ref().map_or(String::new(), |n| s(n))));
    }
    let _ = std::fs::write(path, out);
    log::info!("randomizer: the talk lines' cast written to {}", path.to_string_lossy());
}
