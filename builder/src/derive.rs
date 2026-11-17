//! Haiku's game-derived files, made at build time from the player's own
//! dump. The release has none of them: it has `derived.pack`, recipes that
//! say how to make each file from the installed RomFS.
//!
//! A recipe is one of:
//! - bytes: copy runs of bytes out of one or more of the game's files
//!   (each read raw, or LZ11-decompressed) and add Haiku's own bytes
//!   between them; then LZ11-compress the result (lz11.rs, the same
//!   compressor that made the repository's files) or not;
//! - image: decode textures of the game's files (tex.rs), blit pieces of
//!   them onto a canvas (copied, or smoothly scaled), and add a residual,
//!   the per-byte difference to the file Haiku ships (zlib), so the result
//!   is exactly the repository's picture; then write it as a PNG;
//! - amiibo: the four IconData arrays of overrides/amiibo_icons.c, each a
//!   scaled crop of a hero's cut-in portrait on a background in a frame, and
//!   a residual, written as the C file the recompiler compiles.
//!
//! Every source is checked by its SHA-256 before it is used and every
//! result after, so a different or damaged dump can't make a wrong file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::{lz11, pil, tex};

pub const MAGIC: &[u8; 4] = b"HDRV";
pub const VERSION: u32 = 1;

pub type Hash = [u8; 32];

pub fn sha(bytes: &[u8]) -> Hash {
    Sha256::digest(bytes).into()
}

pub fn hex(hash: &Hash) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

/// how a source file is read.
#[derive(Debug, Clone, PartialEq)]
pub enum How {
    /// its bytes as they are.
    Raw,
    /// LZ11-decompressed.
    Plain,
    /// a texture of it (after decompressing when lz), as RGBA, flipped
    /// upside down when flip.
    Texture { lz: bool, arc: u32, tex: tex::Texture, flip: bool, shift4: bool },
    /// another derived picture, made earlier in the pack.
    Derived,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    /// a RomFS path, or for Derived the target path of another entry.
    pub path: String,
    pub how: How,
    /// of what `how` reads: the bytes, or the RGBA of a picture.
    pub hash: Hash,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Copy { source: u16, offset: u64, len: u64 },
    Add(Vec<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Blit {
    pub source: u8,
    /// 0 copied or smoothly scaled (scale), 1 make_glyphs.py's 4x upscale
    /// (pil::glyph_up), 2 Pillow's LANCZOS resize of RGBA.
    pub filter: u8,
    pub sx: u32,
    pub sy: u32,
    pub sw: u32,
    pub sh: u32,
    pub dx: u32,
    pub dy: u32,
    pub dw: u32,
    pub dh: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AmiiboIcon {
    pub label: String,
    pub source: u8,
    /// x0, y0, x1, y1 in the cut-in portrait.
    pub crop: [u32; 4],
    pub background: [u8; 3],
    /// zlib of the u16 differences (wrapping) from the reconstruction.
    pub residual: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    Bytes { ops: Vec<Op>, lz: bool },
    Image { w: u32, h: u32, blits: Vec<Blit>, residual: Vec<u8> },
    Amiibo { header: String, frame: [u8; 3], icons: Vec<AmiiboIcon> },
    /// a font's glyph metrics as JSON (assets/font/System_L-metrics.json):
    /// the .bfnt's glyph records, written as Python's json.dumps(indent=2)
    Metrics { crlf: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// where the file goes, from the install root (assets/..., or
    /// overrides/... for the recompiler's inputs).
    pub target: String,
    pub sources: Vec<Source>,
    pub body: Body,
    /// the finished file's bytes, or for an image its RGBA (w, h first).
    pub hash: Hash,
}

// ---- the file format ------------------------------------------------------

#[derive(Default)]
pub struct Writer(pub Vec<u8>);

impl Writer {
    pub fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    pub fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn var(&mut self, mut v: u64) {
        loop {
            let byte = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                self.0.push(byte);
                break;
            }
            self.0.push(byte | 0x80);
        }
    }
    pub fn bytes(&mut self, b: &[u8]) {
        self.var(b.len() as u64);
        self.0.extend_from_slice(b);
    }
    pub fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }
    pub fn hash(&mut self, h: &Hash) {
        self.0.extend_from_slice(h);
    }
}

pub struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).filter(|&end| end <= self.data.len()).ok_or("derived.pack is cut short")?;
        let out = &self.data[self.at..end];
        self.at = end;
        Ok(out)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn var(&mut self) -> Result<u64, String> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.u8()?;
            v |= ((b & 0x7F) as u64) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err("derived.pack has a bad number".into())
    }
    fn bytes(&mut self) -> Result<Vec<u8>, String> {
        let n = self.var()? as usize;
        Ok(self.take(n)?.to_vec())
    }
    fn str(&mut self) -> Result<String, String> {
        String::from_utf8(self.bytes()?).map_err(|_| "derived.pack has a bad name".to_string())
    }
    fn hash(&mut self) -> Result<Hash, String> {
        Ok(self.take(32)?.try_into().unwrap())
    }
}

pub fn write_pack(entries: &[Entry]) -> Vec<u8> {
    let mut w = Writer::default();
    w.0.extend_from_slice(MAGIC);
    w.u32(VERSION);
    w.var(entries.len() as u64);
    for e in entries {
        w.str(&e.target);
        w.hash(&e.hash);
        w.var(e.sources.len() as u64);
        for s in &e.sources {
            w.str(&s.path);
            match &s.how {
                How::Raw => w.u8(0),
                How::Plain => w.u8(1),
                How::Texture { lz, arc, tex, flip, shift4 } => {
                    w.u8(2);
                    w.u8(*lz as u8 | (*flip as u8) << 1 | (*shift4 as u8) << 2);
                    w.var(*arc as u64);
                    w.var(tex.offset as u64);
                    w.var(tex.w as u64);
                    w.var(tex.h as u64);
                    w.u8(tex.format);
                }
                How::Derived => w.u8(3),
            }
            w.hash(&s.hash);
        }
        match &e.body {
            Body::Bytes { ops, lz } => {
                w.u8(0);
                w.u8(*lz as u8);
                w.var(ops.len() as u64);
                for op in ops {
                    match op {
                        Op::Copy { source, offset, len } => {
                            w.u8(0);
                            w.var(*source as u64);
                            w.var(*offset);
                            w.var(*len);
                        }
                        Op::Add(bytes) => {
                            w.u8(1);
                            w.bytes(bytes);
                        }
                    }
                }
            }
            Body::Image { w: width, h, blits, residual } => {
                w.u8(1);
                w.var(*width as u64);
                w.var(*h as u64);
                w.var(blits.len() as u64);
                for b in blits {
                    w.u8(b.source);
                    w.u8(b.filter);
                    for v in [b.sx, b.sy, b.sw, b.sh, b.dx, b.dy, b.dw, b.dh] {
                        w.var(v as u64);
                    }
                }
                w.bytes(residual);
            }
            Body::Metrics { crlf } => {
                w.u8(3);
                w.u8(*crlf as u8);
            }
            Body::Amiibo { header, frame, icons } => {
                w.u8(2);
                w.str(header);
                w.0.extend_from_slice(frame);
                w.var(icons.len() as u64);
                for icon in icons {
                    w.str(&icon.label);
                    w.u8(icon.source);
                    for v in icon.crop {
                        w.var(v as u64);
                    }
                    w.0.extend_from_slice(&icon.background);
                    w.bytes(&icon.residual);
                }
            }
        }
    }
    w.0
}

pub fn read_pack(data: &[u8]) -> Result<Vec<Entry>, String> {
    let mut r = Reader { data, at: 0 };
    if r.take(4)? != MAGIC {
        return Err("derived.pack is not a Haiku recipe file".into());
    }
    let version = r.u32()?;
    if version != VERSION {
        return Err(format!("derived.pack is version {version}, this setup reads {VERSION}"));
    }
    let count = r.var()? as usize;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let target = r.str()?;
        let hash = r.hash()?;
        let mut sources = Vec::new();
        for _ in 0..r.var()? {
            let path = r.str()?;
            let how = match r.u8()? {
                0 => How::Raw,
                1 => How::Plain,
                2 => {
                    let flags = r.u8()?;
                    let arc = r.var()? as u32;
                    let offset = r.var()? as usize;
                    let w = r.var()? as u32;
                    let h = r.var()? as u32;
                    let format = r.u8()?;
                    How::Texture { lz: flags & 1 != 0, flip: flags & 2 != 0, shift4: flags & 4 != 0, arc, tex: tex::Texture { offset, w, h, format } }
                }
                3 => How::Derived,
                other => return Err(format!("derived.pack: unknown source kind {other}")),
            };
            sources.push(Source { path, how, hash: r.hash()? });
        }
        let body = match r.u8()? {
            0 => {
                let lz = r.u8()? != 0;
                let mut ops = Vec::new();
                for _ in 0..r.var()? {
                    ops.push(match r.u8()? {
                        0 => Op::Copy { source: r.var()? as u16, offset: r.var()?, len: r.var()? },
                        _ => Op::Add(r.bytes()?),
                    });
                }
                Body::Bytes { ops, lz }
            }
            1 => {
                let w = r.var()? as u32;
                let h = r.var()? as u32;
                let mut blits = Vec::new();
                for _ in 0..r.var()? {
                    let source = r.u8()?;
                    let filter = r.u8()?;
                    let mut v = [0u32; 8];
                    for slot in &mut v {
                        *slot = r.var()? as u32;
                    }
                    blits.push(Blit { source, filter, sx: v[0], sy: v[1], sw: v[2], sh: v[3], dx: v[4], dy: v[5], dw: v[6], dh: v[7] });
                }
                Body::Image { w, h, blits, residual: r.bytes()? }
            }
            2 => {
                let header = r.str()?;
                let frame = r.take(3)?.try_into().unwrap();
                let mut icons = Vec::new();
                for _ in 0..r.var()? {
                    let label = r.str()?;
                    let source = r.u8()?;
                    let mut crop = [0u32; 4];
                    for slot in &mut crop {
                        *slot = r.var()? as u32;
                    }
                    let background = r.take(3)?.try_into().unwrap();
                    icons.push(AmiiboIcon { label, source, crop, background, residual: r.bytes()? });
                }
                Body::Amiibo { header, frame, icons }
            }
            3 => Body::Metrics { crlf: r.u8()? != 0 },
            other => return Err(format!("derived.pack: unknown recipe kind {other}")),
        };
        entries.push(Entry { target, sources, body, hash });
    }
    Ok(entries)
}

// ---- pictures -------------------------------------------------------------

#[derive(Clone)]
pub struct Picture {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl Picture {
    pub fn hash(&self) -> Hash {
        let mut h = Sha256::new();
        h.update(self.w.to_le_bytes());
        h.update(self.h.to_le_bytes());
        h.update(&self.rgba);
        h.finalize().into()
    }
}

/// the pixel of p at (x, y), clamped to its edges, as floats.
fn texel(p: &Picture, x: i64, y: i64) -> [f32; 4] {
    let x = x.clamp(0, p.w as i64 - 1) as usize;
    let y = y.clamp(0, p.h as i64 - 1) as usize;
    let at = (y * p.w as usize + x) * 4;
    [p.rgba[at] as f32, p.rgba[at + 1] as f32, p.rgba[at + 2] as f32, p.rgba[at + 3] as f32]
}

/// a rectangle of src scaled onto (dw, dh): copied when the sizes match,
/// the average of the pixels each covers when it shrinks, bilinear (with
/// premultiplied alpha) when it grows.
pub fn scale(src: &Picture, sx: u32, sy: u32, sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let mut out = vec![0u8; dw as usize * dh as usize * 4];
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return out;
    }
    for y in 0..dh {
        for x in 0..dw {
            let mut acc = [0f32; 4];
            if sw == dw && sh == dh {
                acc = texel(src, (sx + x) as i64, (sy + y) as i64);
            } else if dw <= sw && dh <= sh {
                // box filter over the source pixels this one covers
                let x0 = sx as f32 + x as f32 * sw as f32 / dw as f32;
                let x1 = sx as f32 + (x + 1) as f32 * sw as f32 / dw as f32;
                let y0 = sy as f32 + y as f32 * sh as f32 / dh as f32;
                let y1 = sy as f32 + (y + 1) as f32 * sh as f32 / dh as f32;
                let mut total = 0f32;
                let mut yy = y0.floor() as i64;
                while (yy as f32) < y1 {
                    let wy = (y1.min(yy as f32 + 1.0) - y0.max(yy as f32)).max(0.0);
                    let mut xx = x0.floor() as i64;
                    while (xx as f32) < x1 {
                        let wx = (x1.min(xx as f32 + 1.0) - x0.max(xx as f32)).max(0.0);
                        let t = texel(src, xx, yy);
                        let weight = wx * wy;
                        let a = t[3] / 255.0;
                        acc[0] += t[0] * a * weight;
                        acc[1] += t[1] * a * weight;
                        acc[2] += t[2] * a * weight;
                        acc[3] += t[3] * weight;
                        total += weight;
                        xx += 1;
                    }
                    yy += 1;
                }
                if total > 0.0 {
                    let alpha = acc[3] / total;
                    let a = alpha / 255.0;
                    for c in 0..3 {
                        acc[c] = if a > 0.0 { acc[c] / total / a } else { 0.0 };
                    }
                    acc[3] = alpha;
                }
            } else {
                // bilinear between pixel centres
                let fx = sx as f32 + (x as f32 + 0.5) * sw as f32 / dw as f32 - 0.5;
                let fy = sy as f32 + (y as f32 + 0.5) * sh as f32 / dh as f32 - 0.5;
                let (ix, iy) = (fx.floor(), fy.floor());
                let (tx, ty) = (fx - ix, fy - iy);
                let clamp_x = |v: i64| v.clamp(sx as i64, (sx + sw) as i64 - 1);
                let clamp_y = |v: i64| v.clamp(sy as i64, (sy + sh) as i64 - 1);
                let corners = [
                    (texel(src, clamp_x(ix as i64), clamp_y(iy as i64)), (1.0 - tx) * (1.0 - ty)),
                    (texel(src, clamp_x(ix as i64 + 1), clamp_y(iy as i64)), tx * (1.0 - ty)),
                    (texel(src, clamp_x(ix as i64), clamp_y(iy as i64 + 1)), (1.0 - tx) * ty),
                    (texel(src, clamp_x(ix as i64 + 1), clamp_y(iy as i64 + 1)), tx * ty),
                ];
                let mut alpha = 0f32;
                for (t, w) in corners {
                    let a = t[3] / 255.0;
                    acc[0] += t[0] * a * w;
                    acc[1] += t[1] * a * w;
                    acc[2] += t[2] * a * w;
                    alpha += t[3] * w;
                }
                let a = alpha / 255.0;
                for c in 0..3 {
                    acc[c] = if a > 0.0 { acc[c] / a } else { 0.0 };
                }
                acc[3] = alpha;
            }
            let at = (y as usize * dw as usize + x as usize) * 4;
            for c in 0..4 {
                out[at + c] = acc[c].round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// a rectangle of p, clamped to it.
pub fn crop(p: &Picture, x: u32, y: u32, w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    for yy in 0..h {
        for xx in 0..w {
            let t = texel(p, (x + xx) as i64, (y + yy) as i64);
            out.extend(t.map(|v| v as u8));
        }
    }
    out
}

/// the canvas the blits make, before the residual.
pub fn reconstruct(w: u32, h: u32, blits: &[Blit], images: &[Option<&Picture>]) -> Result<Vec<u8>, String> {
    let mut canvas = vec![0u8; w as usize * h as usize * 4];
    for b in blits {
        let src = images.get(b.source as usize).copied().flatten().ok_or("a picture's source is not a picture")?;
        let piece = match b.filter {
            0 => scale(src, b.sx, b.sy, b.sw, b.sh, b.dw, b.dh),
            _ => {
                let crop = crop(src, b.sx, b.sy, b.sw, b.sh);
                if b.filter == 1 && b.dw % b.sw.max(1) == 0 && b.dw / b.sw.max(1) == b.dh / b.sh.max(1) {
                    pil::glyph_up(&crop, b.sw as usize, b.sh as usize, (b.dw / b.sw) as usize)
                } else {
                    pil::resize_rgba(&crop, b.sw as usize, b.sh as usize, b.dw as usize, b.dh as usize, if b.filter == 1 { pil::Filter::Bicubic } else { pil::Filter::Lanczos })
                }
            }
        };
        for y in 0..b.dh {
            let ty = b.dy + y;
            if ty >= h {
                break;
            }
            for x in 0..b.dw {
                let tx = b.dx + x;
                if tx >= w {
                    break;
                }
                let from = (y as usize * b.dw as usize + x as usize) * 4;
                let to = (ty as usize * w as usize + tx as usize) * 4;
                canvas[to..to + 4].copy_from_slice(&piece[from..from + 4]);
            }
        }
    }
    Ok(canvas)
}

pub fn zlib(bytes: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(bytes, 9)
}

pub fn unzlib(bytes: &[u8]) -> Result<Vec<u8>, String> {
    miniz_oxide::inflate::decompress_to_vec_zlib(bytes).map_err(|e| format!("a residual can't be read ({e:?})"))
}

pub fn write_png(path: &Path, p: &Picture) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), p.w, p.h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::High);
    let mut writer = encoder.write_header().map_err(|e| format!("{}: {e}", path.display()))?;
    writer.write_image_data(&p.rgba).map_err(|e| format!("{}: {e}", path.display()))?;
    writer.finish().map_err(|e| format!("{}: {e}", path.display()))
}

/// a PNG as RGBA.
pub fn read_png(bytes: &[u8]) -> Result<Picture, String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("png too big")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let (w, h) = (info.width, info.height);
    let n = (w * h) as usize;
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf[..n * 4].to_vec(),
        png::ColorType::Rgb => buf[..n * 3].chunks_exact(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf[..n * 2].chunks_exact(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        png::ColorType::Grayscale => buf[..n].iter().flat_map(|&c| [c, c, c, 255]).collect(),
        png::ColorType::Indexed => return Err("indexed PNG after expansion".into()),
    };
    Ok(Picture { w, h, rgba })
}

// ---- the amiibo icons -----------------------------------------------------

pub const AMIIBO_SIZE: u32 = 48;
pub const AMIIBO_BORDER: u32 = 2;

fn morton(i: usize) -> (usize, usize) {
    let (mut x, mut y) = (0, 0);
    for b in 0..3 {
        x |= ((i >> (2 * b)) & 1) << b;
        y |= ((i >> (2 * b + 1)) & 1) << b;
    }
    (x, y)
}

/// the reconstruction of one icon's 2304 RGB565 words: the crop scaled into
/// the frame, over the background.
pub fn amiibo_words(art: &Picture, crop: [u32; 4], background: [u8; 3], frame: [u8; 3]) -> Vec<u16> {
    let inner = AMIIBO_SIZE - 2 * AMIIBO_BORDER;
    // art.crop(box).resize((44, 44), Image.LANCZOS), then alpha_composite
    // over the background, as make_icons.py does
    let (cw, ch) = (crop[2] - crop[0], crop[3] - crop[1]);
    let face = pil::resize_rgba(&self::crop(art, crop[0], crop[1], cw, ch), cw as usize, ch as usize, inner as usize, inner as usize, pil::Filter::Lanczos);
    let size = AMIIBO_SIZE as usize;
    let mut rgb = vec![[0u8; 3]; size * size];
    for y in 0..size {
        for x in 0..size {
            let border = AMIIBO_BORDER as usize;
            rgb[y * size + x] = if x < border || y < border || x >= size - border || y >= size - border {
                frame
            } else {
                let at = ((y - border) * inner as usize + (x - border)) * 4;
                let out = pil::alpha_composite([background[0], background[1], background[2], 255], face[at..at + 4].try_into().unwrap());
                [out[0], out[1], out[2]]
            };
        }
    }
    let mut words = Vec::with_capacity(size * size);
    for ty in (0..size).step_by(8) {
        for tx in (0..size).step_by(8) {
            for i in 0..64 {
                let (x, y) = morton(i);
                let [r, g, b] = rgb[(ty + y) * size + tx + x];
                let q = |v: u8, max: u32| (v as u32 * max + 127) / 255;
                words.push((q(r, 31) << 11 | q(g, 63) << 5 | q(b, 31)) as u16);
            }
        }
    }
    words
}

/// overrides/amiibo_icons.c from the four icons' words, exactly as
/// dev/amiibo/make_icons.py writes it.
pub fn amiibo_c(header: &str, icons: &[(String, Vec<u16>)]) -> String {
    let mut lines: Vec<String> = header.lines().map(str::to_owned).collect();
    for (label, words) in icons {
        lines.push(format!("    /* {label} */ {{"));
        for chunk in words.chunks(12) {
            lines.push(format!("        {},", chunk.iter().map(|w| format!("0x{w:04X}")).collect::<Vec<_>>().join(", ")));
        }
        lines.push("    },".into());
    }
    lines.push("};".into());
    lines.join("\n") + "\n"
}

/// the glyph metrics JSON of a .bfnt (its glyph records at 0x30, 16 bytes
/// each: codepoint, x, y, width, height, bearing x and y, advance).
pub fn font_metrics(bfnt: &[u8], crlf: bool) -> Option<String> {
    let count = u16::from_le_bytes(bfnt.get(0x22..0x24)?.try_into().ok()?) as usize;
    let nl = if crlf { "\r\n" } else { "\n" };
    let mut out = format!("[{nl}");
    for i in 0..count {
        let r = bfnt.get(0x30 + i * 16..0x40 + i * 16)?;
        let cp = u32::from_le_bytes(r[0..4].try_into().ok()?);
        let x = u16::from_le_bytes(r[4..6].try_into().ok()?);
        let y = u16::from_le_bytes(r[6..8].try_into().ok()?);
        let (w, h, bx, by) = (r[8], r[9], r[10] as i8, r[11] as i8);
        let advance = u32::from_le_bytes(r[12..16].try_into().ok()?);
        let fields = [("codepoint", cp as i64), ("x", x as i64), ("y", y as i64), ("width", w as i64), ("height", h as i64), ("bearing_x", bx as i64), ("bearing_y", by as i64), ("advance", advance as i64)];
        out += &format!("  {{{nl}");
        for (k, (name, value)) in fields.iter().enumerate() {
            out += &format!("    \"{name}\": {value}{}{nl}", if k + 1 < fields.len() { "," } else { "" });
        }
        out += &format!("  }}{}{nl}", if i + 1 < count { "," } else { "" });
    }
    out += &format!("]{nl}");
    Some(out)
}

// ---- applying the pack ----------------------------------------------------

/// reads the game files the recipes use, by RomFS path, from the installed
/// RomFS (case as the game spells it).
pub struct Game {
    pub romfs: PathBuf,
    cache: HashMap<String, Vec<u8>>,
}

impl Game {
    pub fn new(romfs: PathBuf) -> Game {
        Game { romfs, cache: HashMap::new() }
    }

    pub fn raw(&mut self, path: &str) -> Result<Vec<u8>, String> {
        if let Some(bytes) = self.cache.get(path) {
            return Ok(bytes.clone());
        }
        let file = self.romfs.join(path);
        let bytes = std::fs::read(&file).map_err(|e| format!("the game file {path} can't be read ({e})"))?;
        if self.cache.len() > 64 {
            self.cache.clear();
        }
        self.cache.insert(path.to_owned(), bytes.clone());
        Ok(bytes)
    }
}

/// a file of the game read as a source says, bytes or a picture.
pub enum Read {
    Bytes(Vec<u8>),
    Picture(Picture),
}

/// the bytes of a face .arc's entry `arc` (Fates' "bin" file: a table of
/// LZ11 BCH files), or of the file itself when arc is 0 and it is no arc.
fn arc_entry(raw: &[u8], arc: u32) -> Result<Vec<u8>, String> {
    if arc == 0 {
        return Ok(raw.to_vec());
    }
    // the face arcs keep their one BCH at 0x80, LZ11-compressed
    let at = (arc as usize).checked_sub(1).map(|_| 0x80).unwrap_or(0);
    lz11::decompress(raw.get(at..).ok_or("short arc")?)
}

pub fn read_source(game: &mut Game, derived: &HashMap<String, Picture>, s: &Source) -> Result<Read, String> {
    let read = match &s.how {
        How::Raw => Read::Bytes(game.raw(&s.path)?),
        How::Plain => Read::Bytes(lz11::decompress(&game.raw(&s.path)?).map_err(|e| format!("{}: {e}", s.path))?),
        How::Texture { lz, arc, tex, flip, shift4 } => {
            let raw = game.raw(&s.path)?;
            let mut bytes = if *lz { lz11::decompress(&raw).map_err(|e| format!("{}: {e}", s.path))? } else { raw };
            if *arc != 0 {
                bytes = arc_entry(&bytes, *arc)?;
            }
            let data = bytes.get(tex.offset..).ok_or_else(|| format!("{}: the texture is past the end", s.path))?;
            let mut rgba = tex::decode_with(data, tex.w, tex.h, tex.format, *shift4).ok_or_else(|| format!("{}: the texture can't be decoded", s.path))?;
            if *flip {
                rgba = tex::flip(&rgba, tex.w, tex.h);
            }
            Read::Picture(Picture { w: tex.w, h: tex.h, rgba })
        }
        How::Derived => Read::Picture(derived.get(&s.path).cloned().ok_or_else(|| format!("{} is made after the file that needs it", s.path))?),
    };
    let hash = match &read {
        Read::Bytes(b) => sha(b),
        Read::Picture(p) => p.hash(),
    };
    if hash != s.hash {
        return Err(format!("the game file {} is not the one Haiku expects (a different version of the game, or a damaged dump)", s.path));
    }
    Ok(read)
}

/// makes one entry's file, its bytes back (a PNG's are written by the
/// caller from the picture).
pub fn make(game: &mut Game, derived: &HashMap<String, Picture>, e: &Entry) -> Result<Made, String> {
    let reads: Vec<Read> = e.sources.iter().map(|s| read_source(game, derived, s)).collect::<Result<_, _>>()?;
    match &e.body {
        Body::Bytes { ops, lz } => {
            let mut out = Vec::new();
            for op in ops {
                match op {
                    Op::Copy { source, offset, len } => {
                        let Some(Read::Bytes(b)) = reads.get(*source as usize) else { return Err(format!("{}: a bad source", e.target)) };
                        let (start, end) = (*offset as usize, (*offset + *len) as usize);
                        out.extend_from_slice(b.get(start..end).ok_or_else(|| format!("{}: a copy past the end of its source", e.target))?);
                    }
                    Op::Add(bytes) => out.extend_from_slice(bytes),
                }
            }
            let out = if *lz { lz11::compress(&out) } else { out };
            if sha(&out) != e.hash {
                return Err(format!("{} came out different from Haiku's", e.target));
            }
            Ok(Made::Bytes(out))
        }
        Body::Image { w, h, blits, residual } => {
            let images: Vec<Option<&Picture>> = reads.iter().map(|r| if let Read::Picture(p) = r { Some(p) } else { None }).collect();
            let mut rgba = reconstruct(*w, *h, blits, &images)?;
            let diff = unzlib(residual)?;
            if diff.len() != rgba.len() {
                return Err(format!("{}: the residual is the wrong size", e.target));
            }
            for (v, d) in rgba.iter_mut().zip(&diff) {
                *v = v.wrapping_add(*d);
            }
            let picture = Picture { w: *w, h: *h, rgba };
            if picture.hash() != e.hash {
                return Err(format!("{} came out different from Haiku's", e.target));
            }
            Ok(Made::Picture(picture))
        }
        Body::Metrics { crlf } => {
            let Some(Read::Bytes(bfnt)) = reads.first() else { return Err("metrics: a bad source".into()) };
            let text = font_metrics(bfnt, *crlf).ok_or("metrics: the font can't be read")?.into_bytes();
            if sha(&text) != e.hash {
                return Err(format!("{} came out different from Haiku's", e.target));
            }
            Ok(Made::Bytes(text))
        }
        Body::Amiibo { header, frame, icons } => {
            let mut made = Vec::new();
            for icon in icons {
                let Some(Read::Picture(art)) = reads.get(icon.source as usize) else { return Err("amiibo: a bad source".into()) };
                let mut words = amiibo_words(art, icon.crop, icon.background, *frame);
                let diff = unzlib(&icon.residual)?;
                if diff.len() != words.len() * 2 {
                    return Err("amiibo: the residual is the wrong size".into());
                }
                for (i, w) in words.iter_mut().enumerate() {
                    *w = w.wrapping_add(u16::from_le_bytes([diff[2 * i], diff[2 * i + 1]]));
                }
                made.push((icon.label.clone(), words));
            }
            let text = amiibo_c(header, &made).into_bytes();
            if sha(&text) != e.hash {
                return Err(format!("{} came out different from Haiku's", e.target));
            }
            Ok(Made::Bytes(text))
        }
    }
}

pub enum Made {
    Bytes(Vec<u8>),
    Picture(Picture),
}

/// makes every file of the pack under root (each target's own path),
/// telling progress how far it is.
pub fn apply(pack: &[Entry], romfs: &Path, root: &Path, progress: &dyn Fn(usize, usize)) -> Result<usize, String> {
    let mut game = Game::new(romfs.to_path_buf());
    let mut derived: HashMap<String, Picture> = HashMap::new();
    let needed: std::collections::HashSet<&str> =
        pack.iter().flat_map(|e| e.sources.iter()).filter(|s| s.how == How::Derived).map(|s| s.path.as_str()).collect();
    for (i, e) in pack.iter().enumerate() {
        let made = make(&mut game, &derived, e)?;
        let path = root.join(&e.target);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| format!("{}: {err}", parent.display()))?;
        }
        match made {
            Made::Bytes(bytes) => std::fs::write(&path, bytes).map_err(|err| format!("{}: {err}", path.display()))?,
            Made::Picture(picture) => {
                write_png(&path, &picture)?;
                if needed.contains(e.target.as_str()) {
                    derived.insert(e.target.clone(), picture);
                }
            }
        }
        progress(i + 1, pack.len());
    }
    Ok(pack.len())
}
