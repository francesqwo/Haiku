//! Development only: makes derived.pack, the recipes derive.rs follows,
//! from the repository's game-derived files and a RomFS extracted from the
//! test dump. `haiku-setup --make-derived <repo> <romfs> <pack> [report]`.
//!
//! What each file is made from is worked out here: the same RomFS path (or
//! files like it) for edited game files, the texture of the game's file
//! whose pixels match for a decoded picture, and the hand-made pictures'
//! own recipes (the glyph sets, the growth button, the amiibo icons). The
//! report lists every file, its recipe and how many bytes of it are
//! Haiku's own (added or residual), which is what the release carries.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::derive::{self, AmiiboIcon, Blit, Body, Entry, How, Op, Picture, Source};
use crate::{lz11, tex};

fn rel(path: &Path, root: &Path) -> String {
    path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

// ---- byte deltas ------------------------------------------------------------

const MIN_MATCH: usize = 12;
const KEY: usize = 8;

fn key(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + KEY].try_into().unwrap())
}

/// copies from sources and additions that make target; sources[0] is
/// indexed at every byte, the rest (a pool of similar files) at every
/// `step` bytes.
pub fn delta(target: &[u8], sources: &[Vec<u8>], step: usize) -> Vec<Op> {
    let mut index: HashMap<u64, Vec<(u16, u32)>> = HashMap::new();
    for (s, src) in sources.iter().enumerate() {
        let stride = if s == 0 { 1 } else { step.max(1) };
        let mut at = 0;
        while at + KEY <= src.len() {
            let list = index.entry(key(src, at)).or_default();
            if list.len() < 512 {
                list.push((s as u16, at as u32));
            }
            at += stride;
        }
    }
    let mut ops: Vec<Op> = Vec::new();
    let mut literal: Vec<u8> = Vec::new();
    let mut last: Option<(u16, usize)> = None;
    let mut i = 0;
    let push_copy = |ops: &mut Vec<Op>, source: u16, offset: usize, len: usize| {
        if let Some(Op::Copy { source: s, offset: o, len: l }) = ops.last_mut() {
            if *s == source && *o + *l == offset as u64 {
                *l += len as u64;
                return;
            }
        }
        ops.push(Op::Copy { source, offset: offset as u64, len: len as u64 });
    };
    while i < target.len() {
        let mut best: Option<(u16, usize, usize, usize)> = None; // source, start, back, forward
        let mut consider = |s: u16, p: usize| {
            let src = &sources[s as usize];
            let mut fwd = 0;
            while i + fwd < target.len() && p + fwd < src.len() && target[i + fwd] == src[p + fwd] {
                fwd += 1;
            }
            let mut back = 0;
            while back < literal.len() && back < p && target[i - back - 1] == src[p - back - 1] {
                back += 1;
            }
            if fwd + back >= MIN_MATCH && best.is_none_or(|(_, _, b, f)| fwd + back > b + f) {
                best = Some((s, p, back, fwd));
            }
        };
        if let Some((s, p)) = last {
            if p < sources[s as usize].len() {
                consider(s, p);
            }
        }
        if i + KEY <= target.len() {
            if let Some(list) = index.get(&key(target, i)) {
                for &(s, p) in list {
                    consider(s, p as usize);
                }
            }
        }
        match best {
            Some((s, p, back, fwd)) if back + fwd >= MIN_MATCH => {
                literal.truncate(literal.len() - back);
                if !literal.is_empty() {
                    ops.push(Op::Add(std::mem::take(&mut literal)));
                }
                push_copy(&mut ops, s, p - back, back + fwd);
                i += fwd;
                last = Some((s, p + fwd));
            }
            _ => {
                literal.push(target[i]);
                i += 1;
            }
        }
    }
    if !literal.is_empty() {
        ops.push(Op::Add(literal));
    }
    ops
}

fn added(ops: &[Op]) -> usize {
    ops.iter().map(|op| if let Op::Add(b) = op { b.len() } else { 0 }).sum()
}

// ---- reading the game ---------------------------------------------------------

struct Rom {
    root: PathBuf,
}

impl Rom {
    fn exists(&self, path: &str) -> bool {
        self.root.join(path).is_file()
    }
    fn raw(&self, path: &str) -> Vec<u8> {
        std::fs::read(self.root.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
    }
    /// a file as a source reads it: decompressed when it is .lz.
    fn source(&self, path: &str) -> Option<(How, Vec<u8>)> {
        let raw = std::fs::read(self.root.join(path)).ok()?;
        if path.ends_with(".lz") {
            lz11::decompress(&raw).ok().map(|plain| (How::Plain, plain))
        } else {
            Some((How::Raw, raw))
        }
    }
    /// files in the same folder as path that differ from its name only in
    /// numbers (アクセサリ1_13 -> アクセサリ1_0..12), or every file there for
    /// the message folders.
    fn pool(&self, path: &str) -> Vec<String> {
        let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
        let shape = |n: &str| -> String {
            let mut out = String::new();
            for c in n.chars() {
                if c.is_ascii_digit() {
                    if !out.ends_with('#') {
                        out.push('#');
                    }
                } else {
                    out.push(c);
                }
            }
            out
        };
        let everything = path.starts_with("m/");
        let mut out = Vec::new();
        if let Ok(entries) = std::fs::read_dir(self.root.join(dir)) {
            let mut names: Vec<String> = entries.flatten().filter(|e| e.path().is_file()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
            names.sort();
            for other in names {
                if other != name && (everything || shape(&other) == shape(name)) {
                    out.push(if dir.is_empty() { other } else { format!("{dir}/{other}") });
                }
            }
        }
        out
    }
}

/// one finished entry and what of it is Haiku's.
pub struct Made {
    pub entry: Entry,
    pub size: usize,
    pub own: usize,
    pub note: String,
}

fn bytes_entry(target: &str, wanted: &[u8], rom: &Rom, candidates: &[String]) -> Made {
    // a .lz file the builder's compressor makes again is rebuilt from its
    // plain bytes; anything else is made as it is
    let lz = target.ends_with(".lz") && lz11::decompress(wanted).is_ok_and(|plain| lz11::compress(&plain) == wanted);
    let plain = if lz { lz11::decompress(wanted).unwrap() } else { wanted.to_vec() };
    let mut sources: Vec<(String, How, Vec<u8>)> = Vec::new();
    for path in candidates {
        if sources.iter().any(|(p, _, _)| p == path) {
            continue;
        }
        if let Some((how, bytes)) = rom.source(path) {
            sources.push((path.clone(), how, bytes));
        }
        // the raw bytes too, when the file is kept compressed as it was
        if !lz && path.ends_with(".lz") && rom.exists(path) && sources.len() < 2 {
            sources.push((path.clone(), How::Raw, rom.raw(path)));
        }
    }
    let datas: Vec<Vec<u8>> = sources.iter().map(|(_, _, b)| b.clone()).collect();
    let total: usize = datas.iter().map(Vec::len).sum();
    let step = if total > 4 << 20 { 8 } else if total > 1 << 20 { 4 } else { 1 };
    let ops = delta(&plain, &datas, step);
    // keep only the sources used, renumbered
    let mut used: Vec<usize> = Vec::new();
    for op in &ops {
        if let Op::Copy { source, .. } = op {
            if !used.contains(&(*source as usize)) {
                used.push(*source as usize);
            }
        }
    }
    used.sort();
    let ops: Vec<Op> = ops
        .into_iter()
        .map(|op| match op {
            Op::Copy { source, offset, len } => Op::Copy { source: used.iter().position(|&u| u == source as usize).unwrap() as u16, offset, len },
            other => other,
        })
        .collect();
    let srcs: Vec<Source> = used.iter().map(|&u| Source { path: sources[u].0.clone(), how: sources[u].1.clone(), hash: derive::sha(&sources[u].2) }).collect();
    let own = added(&ops);
    let note = format!(
        "bytes from {}{}",
        if srcs.is_empty() { "nothing (all Haiku's)".to_string() } else { srcs.iter().map(|s| s.path.clone()).collect::<Vec<_>>().join(", ") },
        if lz { ", LZ11 again" } else { "" }
    );
    Made { entry: Entry { target: target.to_owned(), sources: srcs, body: Body::Bytes { ops, lz }, hash: derive::sha(wanted) }, size: wanted.len(), own, note }
}

// ---- pictures -----------------------------------------------------------------

fn diff_count(a: &[u8], b: &[u8]) -> usize {
    a.chunks_exact(4).zip(b.chunks_exact(4)).filter(|(x, y)| x != y).count()
}

/// the texture of one of files (and its orientation) closest to picture.
fn find_texture(picture: &Picture, files: &[String], rom: &Rom, format_hint: Option<u8>) -> Option<(Source, usize)> {
    let mut best: Option<(Source, usize)> = None;
    for path in files {
        let Some((how, bytes)) = rom.source(path) else { continue };
        let lz = how == How::Plain;
        let mut textures: Vec<tex::Texture> = tex::bch_named_textures(&bytes).into_iter().map(|(_, t)| t).collect();
        for t in tex::bch_textures(&bytes) {
            if !textures.contains(&t) {
                textures.push(t);
            }
        }
        // a font sheet: no BCH, so every place a sheet of the size fits
        if textures.is_empty() || path.ends_with(".bfnt.lz") {
            for format in format_hint.map(|f| vec![f]).unwrap_or_else(|| vec![8, 9, 11, 10, 5, 4, 13, 12]) {
                let size = tex::size(picture.w, picture.h, format);
                if size == 0 || size > bytes.len() {
                    continue;
                }
                let mut at = 0;
                while at + size <= bytes.len() {
                    textures.push(tex::Texture { offset: at, w: picture.w, h: picture.h, format });
                    at += 4;
                }
            }
        }
        for t in textures {
            if t.w != picture.w || t.h != picture.h || format_hint.is_some_and(|f| f != t.format) {
                continue;
            }
            let Some(data) = bytes.get(t.offset..) else { continue };
            // the first tile first, to pass over the wrong places quickly
            if bytes.len() > 1 << 16 && t.offset % 0x80 != 0 && path.ends_with(".bfnt.lz") {
                continue;
            }
            for shift4 in [false, true] {
                let Some(rgba) = tex::decode_with(data, t.w, t.h, t.format, shift4) else { continue };
                for flip in [true, false] {
                    let candidate = if flip { tex::flip(&rgba, t.w, t.h) } else { rgba.clone() };
                    let d = diff_count(&candidate, &picture.rgba);
                    if best.as_ref().is_none_or(|(_, bd)| d < *bd) {
                        let p = Picture { w: t.w, h: t.h, rgba: candidate };
                        best = Some((Source { path: path.clone(), how: How::Texture { lz, arc: 0, tex: t, flip, shift4 }, hash: p.hash() }, d));
                    }
                    if d == 0 {
                        return best;
                    }
                }
            }
        }
    }
    best
}

fn residual(target: &Picture, recon: &[u8]) -> Vec<u8> {
    let diff: Vec<u8> = target.rgba.iter().zip(recon).map(|(t, r)| t.wrapping_sub(*r)).collect();
    derive::zlib(&diff)
}

fn image_entry(target: &str, picture: &Picture, sources: Vec<Source>, blits: Vec<Blit>, images: &[Option<&Picture>], note: String) -> Made {
    let recon = derive::reconstruct(picture.w, picture.h, &blits, images).unwrap();
    let res = residual(picture, &recon);
    let changed = diff_count(&recon, &picture.rgba);
    let own = res.len();
    Made {
        entry: Entry { target: target.to_owned(), sources, body: Body::Image { w: picture.w, h: picture.h, blits, residual: res }, hash: picture.hash() },
        size: picture.rgba.len(),
        own,
        note: format!("{note}; {changed} of {} pixels corrected by the residual", picture.w * picture.h),
    }
}

fn decoded_entry(target: &str, picture: &Picture, files: &[String], rom: &Rom, format_hint: Option<u8>) -> Option<(Made, Picture)> {
    let (source, diff) = find_texture(picture, files, rom, format_hint)?;
    // the decoded source, the reconstruction's only layer
    let How::Texture { lz, tex: t, flip, shift4, .. } = source.how.clone() else { unreachable!() };
    let raw = rom.raw(&source.path);
    let bytes = if lz { lz11::decompress(&raw).unwrap() } else { raw };
    let mut rgba = tex::decode_with(&bytes[t.offset..], t.w, t.h, t.format, shift4).unwrap();
    if flip {
        rgba = tex::flip(&rgba, t.w, t.h);
    }
    let decoded = Picture { w: t.w, h: t.h, rgba };
    let blits = vec![Blit { source: 0, filter: 0, sx: 0, sy: 0, sw: t.w, sh: t.h, dx: 0, dy: 0, dw: t.w, dh: t.h }];
    let note = format!("texture of {} (format {}, at 0x{:X}{}{}){}", source.path, t.format, t.offset, if flip { ", flipped" } else { "" }, if shift4 { ", 4-bit by shift" } else { "" }, if diff == 0 { ", exact" } else { "" });
    let made = image_entry(target, picture, vec![source], blits, &[Some(&decoded)], note);
    Some((made, decoded))
}

/// the first texture of a face arc's BCH, as dev/amiibo/make_icons.py
/// finds it.
fn face_texture(bch: &[u8]) -> Option<tex::Texture> {
    let u32_at = |at: usize| u32::from_le_bytes(bch[at..at + 4].try_into().unwrap()) as usize;
    let (info, desc, base) = (u32_at(8), u32_at(16), u32_at(20));
    let table = u32_at(info + 0x24) + info;
    let entry = u32_at(table);
    let at = u32_at(info + entry) + desc;
    let h = u16::from_le_bytes(bch[at..at + 2].try_into().unwrap()) as u32;
    let w = u16::from_le_bytes(bch[at + 2..at + 4].try_into().unwrap()) as u32;
    let (offset, format) = if info == 0x44 || info == 0x3C { (u32_at(at + 0x10), u32_at(at + 0x18)) } else { (u32_at(at + 8), u32_at(at + 0x10)) };
    Some(tex::Texture { offset: base + offset, w, h, format: format as u8 })
}

fn amiibo_entry(repo: &Path, rom: &Rom) -> Made {
    let text = std::fs::read_to_string(repo.join("overrides/amiibo_icons.c")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.iter().position(|l| l.contains("/* Marth */")).unwrap();
    let header = lines[..start].join("\n") + "\n";
    // dev/amiibo/make_icons.py's HEROES and FRAME
    let heroes: [(&str, &str, [u32; 4], [u8; 3]); 4] = [
        ("マルス", "Marth", [244, 86, 366, 208], [28, 34, 70]),
        ("アイク", "Ike", [269, 89, 391, 211], [60, 36, 28]),
        ("ルキナ", "Lucina", [266, 78, 388, 200], [30, 32, 62]),
        ("ルフレ", "Robin", [251, 80, 373, 202], [52, 38, 34]),
    ];
    let frame = [49, 28, 8];
    let mut words_by_icon: Vec<Vec<u16>> = Vec::new();
    let mut current: Option<Vec<u16>> = None;
    for line in &lines[start..] {
        if line.contains("/* ") && line.contains("*/ {") {
            if let Some(w) = current.take() {
                words_by_icon.push(w);
            }
            current = Some(Vec::new());
        } else if let Some(w) = current.as_mut() {
            for part in line.split(',') {
                let part = part.trim();
                if let Some(hex) = part.strip_prefix("0x") {
                    w.push(u16::from_str_radix(hex, 16).unwrap());
                }
            }
        }
    }
    if let Some(w) = current.take() {
        words_by_icon.push(w);
    }
    let mut sources = Vec::new();
    let mut icons = Vec::new();
    let mut own = 0;
    let mut changed = 0;
    for (i, (name, label, crop, background)) in heroes.iter().enumerate() {
        let path = format!("face/face/{name}_ct.arc");
        let raw = rom.raw(&path);
        let bch = lz11::decompress(&raw[0x80..]).unwrap();
        let t = face_texture(&bch).unwrap();
        let art = Picture { w: t.w, h: t.h, rgba: tex::decode(&bch[t.offset..], t.w, t.h, t.format).unwrap() };
        sources.push(Source { path, how: How::Texture { lz: false, arc: 1, tex: t, flip: false, shift4: false }, hash: art.hash() });
        let recon = derive::amiibo_words(&art, *crop, *background, frame);
        let target = &words_by_icon[i];
        let diff: Vec<u8> = target.iter().zip(&recon).flat_map(|(t, r)| t.wrapping_sub(*r).to_le_bytes()).collect();
        changed += target.iter().zip(&recon).filter(|(t, r)| t != r).count();
        let res = derive::zlib(&diff);
        own += res.len();
        icons.push(AmiiboIcon { label: label.to_string(), source: i as u8, crop: *crop, background: *background, residual: res });
    }
    Made {
        entry: Entry { target: "overrides/amiibo_icons.c".into(), sources, body: Body::Amiibo { header, frame, icons }, hash: derive::sha(text.as_bytes()) },
        size: text.len(),
        own,
        note: format!("the four heroes' cut-in portraits, cropped, scaled and framed; {changed} of 9216 words corrected by the residual"),
    }
}

/// every recipe, in the order they are made.
pub fn make_all(repo: &Path, romfs: &Path) -> Vec<Made> {
    let rom = Rom { root: romfs.to_path_buf() };
    let assets = repo.join("assets");
    let mut made = Vec::new();
    let mut derived_pictures: HashMap<String, Picture> = HashMap::new();

    // 1. Haiku's RomFS layer: the game's files edited (or new ones made from
    // the game's own)
    let builtin = assets.join("builtin/romfs");
    let mut files = Vec::new();
    walk(&builtin, &mut files);
    for file in &files {
        let path = rel(file, &builtin);
        let mut candidates = Vec::new();
        if rom.exists(&path) {
            candidates.push(path.clone());
        }
        candidates.extend(rom.pool(&path));
        let target = format!("assets/builtin/romfs/{path}");
        made.push(bytes_entry(&target, &std::fs::read(file).unwrap(), &rom, &candidates));
    }

    // 2. the UI and font files, decompressed copies of the game's
    let mut ui_files = Vec::new();
    walk(&assets.join("ui"), &mut ui_files);
    walk(&assets.join("font"), &mut ui_files);
    let mut pictures = Vec::new();
    let mut later = Vec::new();
    for file in &ui_files {
        let path = rel(file, &assets);
        let target = format!("assets/{path}");
        let ext = file.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        match ext.as_str() {
            "bch" | "bfnt" => {
                let romfs_path = format!("{path}.lz");
                made.push(bytes_entry(&target, &std::fs::read(file).unwrap(), &rom, &[romfs_path]));
            }
            "png" => pictures.push((path, target)),
            _ => later.push((path, target)),
        }
    }
    for (path, target) in &pictures {
        let picture = derive::read_png(&std::fs::read(assets.join(path)).unwrap()).unwrap();
        if path == "ui/info-ui/GrowthButton.png" {
            continue;
        }
        // ui/<sheet>_/<name>.png comes from ui/<sheet>.bch; font/<font>_0.png
        // from font/<font>.bfnt
        let dir = path.rsplit_once('/').map_or("", |(d, _)| d);
        let file = path.rsplit_once('/').map_or(path.as_str(), |(_, f)| f);
        let mut candidates = Vec::new();
        if let Some(sheet) = dir.strip_suffix('_') {
            candidates.push(format!("{sheet}.bch.lz"));
        } else if let Some(font) = file.strip_suffix("_0.png") {
            candidates.push(format!("{dir}/{font}.bfnt.lz"));
        }
        match decoded_entry(target, &picture, &candidates, &rom, None) {
            Some((m, _)) => {
                derived_pictures.insert(target.clone(), picture.clone());
                made.push(m);
            }
            None => panic!("no source found for {path}"),
        }
    }
    // the growth button: three states of Status1's book button, the book
    // taken out and a growth icon drawn in (dev/info-ui/make_button.py)
    {
        let target = "assets/ui/info-ui/GrowthButton.png";
        let picture = derive::read_png(&std::fs::read(repo.join(target)).unwrap()).unwrap();
        let status = "assets/ui/status_/Status1.png";
        let src = derived_pictures.get(status).unwrap().clone();
        let blits: Vec<Blit> = (0..3).map(|i| Blit { source: 0, filter: 0, sx: 244, sy: 4 + 32 * i, sw: 39, sh: 23, dx: 0, dy: 25 * i, dw: 39, dh: 23 }).collect();
        let sources = vec![Source { path: status.into(), how: How::Derived, hash: src.hash() }];
        made.push(image_entry(target, &picture, sources, blits, &[Some(&src)], "the three states of Status1.png's book button".into()));
    }
    // 3. the glyph sets: the source textures, and each set at 4x
    let glyph_source = assets.join("glyphs/source");
    let mut sources_found = HashMap::new();
    let mut glyph_files = Vec::new();
    walk(&glyph_source, &mut glyph_files);
    // the UI sheets, and icon/ (the message boxes' "A" coin is there)
    let ui_sheets: Vec<String> = {
        let mut v = Vec::new();
        for dir in ["ui", "icon"] {
            if let Ok(entries) = std::fs::read_dir(romfs.join(dir)) {
                let mut names: Vec<String> = entries.flatten().filter(|e| e.path().is_file()).map(|e| format!("{dir}/{}", e.file_name().to_string_lossy())).collect();
                names.sort();
                v.extend(names);
            }
        }
        v
    };
    for file in &glyph_files {
        let path = rel(file, &assets);
        let target = format!("assets/{path}");
        let picture = derive::read_png(&std::fs::read(file).unwrap()).unwrap();
        // tex1_<w>x<h>_<hash>_<format>_mip0.png
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        let format = name.split('_').nth(3).and_then(|f| f.parse::<u8>().ok());
        let found = decoded_entry(&target, &picture, &ui_sheets, &rom, format).filter(|(m, _)| m.note.contains("exact"));
        let (m, _) = found.or_else(|| {
            // anywhere else in the RomFS: every BCH with a texture this size
            let mut all = Vec::new();
            walk(romfs, &mut all);
            let bchs: Vec<String> = all.iter().map(|p| rel(p, romfs)).filter(|p| p.ends_with(".bch.lz") || p.ends_with(".bch")).collect();
            decoded_entry(&target, &picture, &bchs, &rom, format)
        }).unwrap_or_else(|| panic!("no source for {path}"));
        derived_pictures.insert(target.clone(), picture.clone());
        sources_found.insert(name, target.clone());
        made.push(m);
    }
    for set in ["keyboard", "playstation", "xbox"] {
        let mut set_files = Vec::new();
        walk(&assets.join("glyphs").join(set), &mut set_files);
        for file in &set_files {
            let path = rel(file, &assets);
            let target = format!("assets/{path}");
            let picture = derive::read_png(&std::fs::read(file).unwrap()).unwrap();
            let name = file.file_name().unwrap().to_string_lossy().into_owned();
            let source_target = sources_found.get(&name).unwrap_or_else(|| panic!("no source picture for {path}"));
            let src = derived_pictures.get(source_target).unwrap().clone();
            let blits = vec![Blit { source: 0, filter: 1, sx: 0, sy: 0, sw: src.w, sh: src.h, dx: 0, dy: 0, dw: picture.w, dh: picture.h }];
            let sources = vec![Source { path: source_target.clone(), how: How::Derived, hash: src.hash() }];
            made.push(image_entry(&target, &picture, sources, blits, &[Some(&src)], format!("{source_target} at {}x (make_glyphs.py's upscale)", picture.w / src.w.max(1))));
        }
    }
    // 4. the metadata the decoders wrote beside the pictures: the font's
    // metrics are its glyph records; the rest is carried as it is
    for (path, target) in &later {
        if let Some(font) = path.strip_suffix("-metrics.json") {
            let source = format!("{font}.bfnt.lz");
            if let Some((how, bfnt)) = rom.source(&source) {
                let wanted = std::fs::read(repo.join(target)).unwrap();
                let crlf = wanted.windows(2).any(|w| w == b"\r\n");
                if derive::font_metrics(&bfnt, crlf).is_some_and(|t| t.as_bytes() == wanted.as_slice()) {
                    made.push(Made {
                        entry: Entry { target: target.clone(), sources: vec![Source { path: source.clone(), how, hash: derive::sha(&bfnt) }], body: Body::Metrics { crlf }, hash: derive::sha(&wanted) },
                        size: wanted.len(),
                        own: 0,
                        note: format!("the glyph records of {source}, as JSON"),
                    });
                    continue;
                }
            }
        }
        made.push(bytes_entry(target, &std::fs::read(repo.join(target)).unwrap(), &rom, &[]));
    }
    // 5. the amiibo icons the recompiler compiles
    made.push(amiibo_entry(repo, &rom));
    made
}
