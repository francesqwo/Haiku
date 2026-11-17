//! gfx-post: native surfaces, keyed by the GPU memory they stand for.
//!
//! The PICA has no render-target objects: a framebuffer is an address, a
//! width and a format, and a texture read from the same address sees what
//! was drawn there. Post effects depend on that: BloomContext reads the
//! display's colour buffer as a texture, downscales it with display
//! transfers (nngxFilterBlockImage) into a buffer that it then renders into
//! and reads back as a texture of another shape, moves tiles around inside
//! it with texture copies (nngxAddBlockImageCopyCommand) and clears parts of
//! it with memory fills.
//!
//! This module is the native model of that memory. It is pure bookkeeping
//! (no Vulkan): the renderer (render/post.rs) executes the [`Action`]s it
//! plans. The model:
//!
//! - A **surface** is a native image standing for the GPU memory from a
//!   physical address on, with a row stride (the width in pixels) and a
//!   format: [`Key`]. Its rows are the memory's rows, in memory order: image
//!   row 0 is memory row 0, which is the *top* of a PICA framebuffer (the
//!   PICA lays framebuffers and textures out from the top row down; texture
//!   coordinate t = 1 is memory row 0). This is the 3DS's own portrait
//!   orientation, so a framebuffer read back as a texture of any height maps
//!   directly: a view of height h at an offset of r rows samples memory row
//!   `r + h * (1 - t)`. A surface grows downwards when something addresses
//!   rows past its end ([`Desc::rows`] is its height so far).
//! - Which surface holds the current contents of each byte of memory is the
//!   **owner map** ([`Owner`]): a surface, a memory fill not yet applied to
//!   any surface, or a display (the renderer's upright display target, whose
//!   colour buffer address the game draws to).
//! - Before a surface is drawn into or read, the rows involved are brought
//!   up to date from their owners ([`Surfaces::write`], [`Surfaces::read`]):
//!   copied from a surface with the same stride and format, cleared from a
//!   fill, captured (turned from upright to portrait) from the display.
//!   Bytes owned by a surface of another stride hold, on the console, the
//!   other surface's tiles reinterpreted; nothing samples that, so they are
//!   left as they are.
//!
//! Everything is in tiles: the PICA's block layout stores 8x8-pixel tiles
//! one after another, a row of tiles across the width, so a byte range is a
//! set of tile rectangles (`rects`).
//!
//! Other users (the gfx-shadow agent's shadow-map target) can register a
//! render target with [`Surfaces::write`] and look a texture up with
//! [`Surfaces::read`]; [`Surfaces::owned`] says whether an address is
//! surface memory at all (otherwise it is a texture in guest memory, decoded
//! from there).

use std::collections::BTreeMap;

/// A surface's identity: base physical address, stride in pixels, PICA
/// colour/texture format (0 RGBA8, 1 RGB8, 2 RGB5A1, 3 RGB565, 4 RGBA4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub addr: u32,
    pub width: u32,
    pub format: u32,
}

/// A surface as an action uses it: its key and how many memory rows its
/// image has to have.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Desc {
    pub key: Key,
    pub rows: u32,
}

/// bytes per pixel of a PICA colour format.
pub fn bpp(format: u32) -> u32 {
    match format {
        0 => 4,
        1 => 3,
        _ => 2,
    }
}

impl Key {
    pub fn row_bytes(&self) -> u32 {
        self.width * bpp(self.format)
    }
    pub fn tile_bytes(&self) -> u32 {
        64 * bpp(self.format)
    }
    /// bytes in one row of tiles (8 memory rows)
    pub fn tile_row_bytes(&self) -> u32 {
        self.row_bytes() * 8
    }
    /// the memory rows from `addr` on, if `addr` is a whole number of tile
    /// rows into this surface
    pub fn row_of(&self, addr: u32) -> Option<u32> {
        let off = addr.checked_sub(self.addr)?;
        (off % self.tile_row_bytes() == 0).then(|| off / self.row_bytes())
    }
    /// the tile rectangles (x0, y0, x1, y1 in pixels, memory rows) a byte
    /// range covers; bytes before the surface are dropped
    pub fn rects(&self, start: u32, end: u32) -> Vec<[u32; 4]> {
        let (tile, per_row) = (self.tile_bytes(), self.width / 8);
        let mut out = Vec::new();
        if per_row == 0 {
            return out;
        }
        let s = start.max(self.addr) - self.addr;
        let e = end.saturating_sub(self.addr);
        let (mut k, last) = (s / tile, e.div_ceil(tile));
        while k < last {
            let (ty, tx) = (k / per_row, k % per_row);
            let n = (per_row - tx).min(last - k);
            out.push([tx * 8, ty * 8, (tx + n) * 8, ty * 8 + 8]);
            k += n;
        }
        // whole rows of tiles merged
        let mut merged: Vec<[u32; 4]> = Vec::new();
        for r in out {
            match merged.last_mut() {
                Some(m) if m[0] == 0 && m[2] == self.width && r[0] == 0 && r[2] == self.width && m[3] == r[1] => m[3] = r[3],
                _ => merged.push(r),
            }
        }
        merged
    }
    /// the bytes of memory rows [r0, r1), widened to whole rows of tiles
    pub fn row_bytes_range(&self, r0: u32, r1: u32) -> (u32, u32) {
        let t = self.tile_row_bytes();
        (self.addr + r0 / 8 * t, self.addr + r1.div_ceil(8) * t)
    }
}

/// Who holds the current contents of a byte range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    Surface(Key),
    /// a memory fill (the 32-bit value) applied to no surface yet
    Fill(u32),
    /// display 0 (top) or 1 (bottom): its native upright target
    Display(u8),
}

/// What the renderer does to bring surfaces up to date (render/post.rs).
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// rows of a display's upright target turned into `dst`'s portrait
    /// rows: dst pixel (x, y) is the display framebuffer's memory pixel
    /// (x, y + row_offset); columns past the display's width are cleared
    Capture { disp: u8, dst: Desc, rect: [u32; 4], row_offset: i32, fb_rows: u32 },
    /// a rectangle set to a fill value (32-bit, as the fill wrote it)
    Clear { dst: Desc, rect: [u32; 4], value: u32 },
    /// rectangles copied between surfaces of the same format, at the same
    /// size: (source rectangle, destination x, y)
    Copy { src: Desc, dst: Desc, rects: Vec<([u32; 4], [u32; 2])> },
    /// a display-transfer downscale: `src_rect` averaged 2x2 into `dst_rect`
    Filter { src: Desc, src_rect: [u32; 4], dst: Desc, dst_rect: [u32; 4] },
}

/// A display's colour buffer as the game set it up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Display {
    pub key: Key,
    /// the framebuffer's height in rows (400 for the shared 256x400 buffer)
    pub rows: u32,
}

/// The largest surface, in rows; addresses further on start a new surface.
const MAX_ROWS: u32 = 1024;

#[derive(Default)]
pub struct Surfaces {
    /// start -> (end, owner), not overlapping
    owners: BTreeMap<u32, (u32, Owner)>,
    /// the surfaces, with the rows their images have
    surfaces: Vec<Desc>,
    displays: [Option<Display>; 2],
}

impl Surfaces {
    pub fn new() -> Surfaces {
        Surfaces::default()
    }

    /// the owners of [start, end), in address order, clipped to it; gaps
    /// (memory no native surface knows) are left out
    pub fn owners(&self, start: u32, end: u32) -> Vec<(u32, u32, Owner)> {
        let mut out = Vec::new();
        if let Some((_, &(e, o))) = self.owners.range(..start).next_back() {
            if e > start {
                out.push((start, e.min(end), o));
            }
        }
        for (&s, &(e, o)) in self.owners.range(start..end) {
            out.push((s, e.min(end), o));
        }
        out
    }

    /// whether any of [start, end) is native surface memory
    pub fn owned(&self, start: u32, end: u32) -> bool {
        !self.owners(start, end).is_empty()
    }

    fn assign(&mut self, start: u32, end: u32, owner: Owner) {
        if end <= start {
            return;
        }
        let mut keep = Vec::new();
        let overlapping: Vec<u32> = self
            .owners
            .range(..end)
            .filter(|(_, (e, _))| *e > start)
            .map(|(s, _)| *s)
            .collect();
        for s in overlapping {
            let (e, o) = self.owners.remove(&s).unwrap();
            if s < start {
                keep.push((s, start, o));
            }
            if e > end {
                keep.push((end, e, o));
            }
        }
        for (s, e, o) in keep {
            self.owners.insert(s, (e, o));
        }
        self.owners.insert(start, (end, owner));
    }

    /// the display's colour buffer: from now on its memory is the display's
    /// native target (each post effect starts with this)
    pub fn set_display(&mut self, disp: u8, display: Display) {
        self.displays[disp as usize] = Some(display);
        let k = display.key;
        self.assign(k.addr, k.addr + display.rows * k.row_bytes(), Owner::Display(disp));
    }

    pub fn display(&self, disp: u8) -> Option<Display> {
        self.displays[disp as usize]
    }

    /// which display's colour buffer starts at `addr`
    pub fn display_at(&self, addr: u32) -> Option<u8> {
        (0..2u8).find(|&d| self.displays[d as usize].is_some_and(|x| x.key.addr == addr))
    }

    /// the surface holding memory rows from `addr` on with this stride and
    /// format, made or grown to hold `rows` rows; and the row `addr` is at
    /// in it
    pub fn surface(&mut self, addr: u32, width: u32, format: u32, rows: u32) -> (Desc, u32) {
        let probe = Key { addr, width, format };
        // an existing one this is a whole number of tile rows into
        let found = self
            .surfaces
            .iter()
            .enumerate()
            .filter(|(_, d)| d.key.width == width && d.key.format == format)
            .filter_map(|(i, d)| d.key.row_of(addr).map(|r| (i, r)))
            .filter(|&(_, r)| r + rows <= MAX_ROWS)
            .max_by_key(|&(i, _)| self.surfaces[i].key.addr);
        let (i, r) = match found {
            Some(f) => f,
            None => {
                self.surfaces.push(Desc { key: probe, rows: 0 });
                (self.surfaces.len() - 1, 0)
            }
        };
        let d = &mut self.surfaces[i];
        d.rows = d.rows.max((r + rows).div_ceil(8) * 8);
        (*d, r)
    }

    /// brings memory rows [r0, r1) of surface `dst` (widened to whole rows
    /// of tiles) up to date from their owners
    fn materialize(&mut self, dst: Desc, r0: u32, r1: u32, out: &mut Vec<Action>) {
        let k = dst.key;
        let (start, end) = k.row_bytes_range(r0, r1);
        for (s, e, owner) in self.owners(start, end) {
            match owner {
                Owner::Surface(src) if src == k => {}
                Owner::Surface(src) if src.width == k.width && src.format == k.format => {
                    let Some(src_desc) = self.surfaces.iter().copied().find(|d| d.key == src) else { continue };
                    let shift = (k.addr as i64 - src.addr as i64) / k.row_bytes() as i64;
                    let rects: Vec<([u32; 4], [u32; 2])> = k
                        .rects(s, e)
                        .into_iter()
                        .filter_map(|r| {
                            let sy = r[1] as i64 + shift;
                            (sy >= 0 && (sy as u32 + r[3] - r[1]) <= src_desc.rows).then(|| ([r[0], sy as u32, r[2], sy as u32 + r[3] - r[1]], [r[0], r[1]]))
                        })
                        .collect();
                    if !rects.is_empty() {
                        out.push(Action::Copy { src: src_desc, dst, rects });
                    }
                }
                // gfx-gaps (experiment, FATES_GFX_POST_RETILE=1): bytes a
                // surface of another stride holds are, on the console, its
                // tiles read with this stride. A tile's 64 pixels keep
                // their layout at any width, so the bytes can be brought
                // over tile by tile. The bloom's 32x64 downscale reads the
                // 64-wide buffer's rows 104-128, which hold the 128-wide
                // buffer's tiles. Measured, this does not explain the
                // bloom's right-edge strip (it widens it slightly), so it
                // is off; see dev/reports/gfx-gaps.md.
                Owner::Surface(src) if src.format == k.format && src.width != k.width && std::env::var("FATES_GFX_POST_RETILE").is_ok_and(|v| v == "1") => {
                    let Some(src_desc) = self.surfaces.iter().copied().find(|d| d.key == src) else { continue };
                    let tile = k.tile_bytes();
                    let mut rects = Vec::new();
                    let mut t = s - (s - k.addr) % tile;
                    while t < e {
                        if t >= src.addr && (t - src.addr) % tile == 0 {
                            if let (Some(dr), Some(sr)) = (k.rects(t, t + tile).first().copied(), src.rects(t, t + tile).first().copied()) {
                                if sr[3] <= src_desc.rows {
                                    rects.push((sr, [dr[0], dr[1]]));
                                }
                            }
                        }
                        t += tile;
                    }
                    if !rects.is_empty() {
                        out.push(Action::Copy { src: src_desc, dst, rects });
                    }
                }
                Owner::Surface(_) => {}
                Owner::Fill(value) => {
                    for rect in k.rects(s, e) {
                        out.push(Action::Clear { dst, rect, value });
                    }
                }
                Owner::Display(disp) => {
                    let Some(display) = self.displays[disp as usize] else { continue };
                    let dk = display.key;
                    if dk.width != k.width || dk.format != k.format {
                        continue;
                    }
                    let row_offset = ((k.addr as i64 - dk.addr as i64) / k.row_bytes() as i64) as i32;
                    for rect in k.rects(s, e) {
                        out.push(Action::Capture { disp, dst, rect, row_offset, fb_rows: display.rows });
                    }
                }
            }
        }
    }

    /// a framebuffer at `addr` (stride `width`, `format`) is about to be
    /// drawn in memory rows [r0, r1) (counted from `addr`): the actions
    /// that bring those rows of its surface up to date, then they belong to
    /// it. Returns the surface and the row `addr` is at in it. A display's
    /// own colour buffer is its native target: nothing to do.
    pub fn write(&mut self, addr: u32, width: u32, format: u32, fb_rows: u32, r0: u32, r1: u32, out: &mut Vec<Action>) -> Option<(Desc, u32)> {
        if self.display_at(addr).is_some() {
            return None;
        }
        let (desc, at) = self.surface(addr, width, format, fb_rows.max(r1));
        let (a, b) = (at + r0, at + r1.min(fb_rows.max(r1)));
        self.materialize(desc, a, b, out);
        let (s, e) = desc.key.row_bytes_range(a, b);
        self.assign(s, e, Owner::Surface(desc.key));
        Some((desc, at))
    }

    /// a texture at `addr` (stride `width`, `rows` tall, `format`) is about
    /// to be sampled in its memory rows [r0, r1): the actions that bring
    /// them up to date in its surface. Returns the surface and the row the
    /// texture starts at in it.
    pub fn read(&mut self, addr: u32, width: u32, format: u32, rows: u32, r0: u32, r1: u32, out: &mut Vec<Action>) -> (Desc, u32) {
        let (desc, at) = self.surface(addr, width, format, rows);
        self.materialize(desc, at + r0.min(rows), at + r1.min(rows), out);
        // re-read the descriptor: materializing never grows it
        (desc, at)
    }

    /// nngxAddMemoryFillCommand: [start, end) filled with a 32-bit value
    pub fn fill(&mut self, start: u32, end: u32, value: u32) {
        self.assign(start, end, Owner::Fill(value));
    }

    /// nngxFilterBlockImage(src, dst, width, height): the block image at
    /// `src` (width x height) averaged 2x2 into one of half the size at
    /// `dst`, same format
    pub fn filter(&mut self, src: u32, dst: u32, width: u32, height: u32, format: u32, out: &mut Vec<Action>) {
        let (sd, sat) = self.read(src, width, format, height, 0, height, out);
        let (w2, h2) = (width / 2, height / 2);
        let (dd, dat) = self.surface(dst, w2, format, h2);
        out.push(Action::Filter { src: sd, src_rect: [0, sat, width, sat + height], dst: dd, dst_rect: [0, dat, w2, dat + h2] });
        let (s, e) = dd.key.row_bytes_range(dat, dat + h2);
        self.assign(s, e, Owner::Surface(dd.key));
    }

    /// nngxAddBlockImageCopyCommand (a GX texture copy): lines of
    /// `src_unit` bytes every `src_unit + src_gap` bytes copied into lines
    /// of `dst_unit` bytes every `dst_unit + dst_gap`, `size` bytes in all.
    /// Copies of whole tiles within surfaces of one format are done as tile
    /// rectangles; anything else is reported as not handled (false).
    pub fn copy(&mut self, src: u32, src_unit: u32, src_gap: u32, dst: u32, dst_unit: u32, dst_gap: u32, size: u32, out: &mut Vec<Action>) -> bool {
        // the byte pieces: (src, dst, length)
        let mut pieces = Vec::new();
        let (su, du) = (if src_unit == 0 { size } else { src_unit }, if dst_unit == 0 { size } else { dst_unit });
        let (mut s, mut d, mut left) = (src, dst, size);
        let (mut sl, mut dl) = (su, du);
        while left > 0 {
            let n = sl.min(dl).min(left);
            pieces.push((s, d, n));
            s += n;
            d += n;
            sl -= n;
            dl -= n;
            left -= n;
            if sl == 0 {
                s += src_gap;
                sl = su;
            }
            if dl == 0 {
                d += dst_gap;
                dl = du;
            }
        }
        let mut handled = true;
        let first = out.len();
        for (s, d, n) in pieces {
            let owners = self.owners(s, s + n);
            let Some(&(_, _, owner)) = owners.first() else { continue };
            if owners.len() > 1 {
                handled = false;
                continue;
            }
            match owner {
                Owner::Surface(sk) => {
                    let Some(sdesc) = self.surfaces.iter().copied().find(|x| x.key == sk) else { continue };
                    let tile = sk.tile_bytes();
                    if (s - sk.addr) % tile != 0 || n % tile != 0 {
                        handled = false;
                        continue;
                    }
                    // the destination: the surface its bytes belong to, if
                    // it has this stride, else the source's own
                    let dkey = match self.owners(d, d + n).first() {
                        Some(&(_, _, Owner::Surface(k))) if k.width == sk.width && k.format == sk.format && d >= k.addr => k,
                        _ => sk,
                    };
                    if d < dkey.addr || (d - dkey.addr) % tile != 0 {
                        handled = false;
                        continue;
                    }
                    let src_rects = sk.rects(s, s + n);
                    let dst_rects = dkey.rects(d, d + n);
                    let rows_needed = dst_rects.iter().map(|r| r[3]).max().unwrap_or(0);
                    let (ddesc, _) = self.surface(dkey.addr, dkey.width, dkey.format, rows_needed);
                    if src_rects.len() != dst_rects.len() || src_rects.iter().zip(&dst_rects).any(|(a, b)| a[2] - a[0] != b[2] - b[0] || a[3] - a[1] != b[3] - b[1]) {
                        // split differently: copy tile by tile
                        let tiles = |k: Key, from: u32| -> Vec<[u32; 4]> { (0..n / tile).flat_map(|i| k.rects(from + i * tile, from + (i + 1) * tile)).collect() };
                        let rects = tiles(sk, s).into_iter().zip(tiles(dkey, d)).map(|(a, b)| (a, [b[0], b[1]])).collect();
                        out.push(Action::Copy { src: sdesc, dst: ddesc, rects });
                    } else {
                        let rects = src_rects.into_iter().zip(dst_rects).map(|(a, b)| (a, [b[0], b[1]])).collect();
                        out.push(Action::Copy { src: sdesc, dst: ddesc, rects });
                    }
                    self.assign(d, d + n, Owner::Surface(ddesc.key));
                }
                Owner::Fill(v) => self.assign(d, d + n, Owner::Fill(v)),
                // gfx-gaps: a copy out of the display's colour buffer
                // (RadialBlur::Capture keeps the frame for the next one's
                // blur): the destination becomes a surface of the
                // display's stride and format, captured from the display
                // target as it is at this point of the frame
                Owner::Display(disp) => {
                    let Some(display) = self.displays[disp as usize] else {
                        handled = false;
                        continue;
                    };
                    let dk = display.key;
                    let tile = dk.tile_bytes();
                    if (s - dk.addr) % tile != 0 || n % tile != 0 {
                        handled = false;
                        continue;
                    }
                    let src_rects = dk.rects(s, s + n);
                    let (ddesc, _) = self.surface(d, dk.width, dk.format, 8);
                    if d < ddesc.key.addr || (d - ddesc.key.addr) % tile != 0 {
                        handled = false;
                        continue;
                    }
                    let dst_rects = ddesc.key.rects(d, d + n);
                    if src_rects.len() != dst_rects.len() || src_rects.iter().zip(&dst_rects).any(|(a, b)| a[0] != b[0] || a[2] != b[2] || a[3] - a[1] != b[3] - b[1]) {
                        handled = false;
                        continue;
                    }
                    // the surface grown to cover every rectangle
                    let rows = dst_rects.iter().map(|r| r[3]).max().unwrap_or(0);
                    let (ddesc, _) = self.surface(ddesc.key.addr, dk.width, dk.format, rows);
                    for (a, b) in src_rects.iter().zip(&dst_rects) {
                        out.push(Action::Capture { disp, dst: ddesc, rect: *b, row_offset: a[1] as i32 - b[1] as i32, fb_rows: display.rows });
                    }
                    self.assign(d, d + n, Owner::Surface(ddesc.key));
                }
            }
        }
        // one action per pair of surfaces
        let mut merged: Vec<Action> = Vec::new();
        for a in out.drain(first..) {
            match (merged.last_mut(), a) {
                (Some(Action::Copy { src, dst, rects }), Action::Copy { src: s2, dst: d2, rects: r2 }) if src.key == s2.key && dst.key == d2.key => {
                    src.rows = src.rows.max(s2.rows);
                    dst.rows = dst.rows.max(d2.rows);
                    rects.extend(r2);
                }
                // gfx-gaps: captures of rows one after another, as one
                (Some(Action::Capture { disp, dst, rect, row_offset, fb_rows }), Action::Capture { disp: d2, dst: t2, rect: r2, row_offset: o2, fb_rows: f2 })
                    if *disp == d2 && dst.key == t2.key && *row_offset == o2 && *fb_rows == f2 && rect[0] == r2[0] && rect[2] == r2[2] && rect[3] == r2[1] =>
                {
                    dst.rows = dst.rows.max(t2.rows);
                    rect[3] = r2[3];
                }
                (_, a) => merged.push(a),
            }
        }
        out.extend(merged);
        handled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rects_of_tile_ranges() {
        let k = Key { addr: 0x1000_0000, width: 128, format: 0 };
        // one tile in column 14 of tile row 0
        assert_eq!(k.rects(k.addr + 0xE00, k.addr + 0xF00), vec![[112, 0, 120, 8]]);
        // two whole tile rows merge
        assert_eq!(k.rects(k.addr, k.addr + 0x2000), vec![[0, 0, 128, 16]]);
        // half a tile row
        assert_eq!(k.rects(k.addr + 0x19000, k.addr + 0x19800), vec![[0, 200, 64, 208]]);
    }

    #[test]
    fn bloom_capture_sequence() {
        // the avatar creator's bloom (BloomCaptureCallback::OnReplace and
        // BloomContext::Draw), addresses as the game has them
        let mut m = Surfaces::new();
        let display = Display { key: Key { addr: 0x1800_0000, width: 256, format: 0 }, rows: 400 };
        m.set_display(0, display);
        let a = 0x180A_6280;
        let mut out = Vec::new();
        m.filter(display.key.addr, a, 256, 400, 0, &mut out);
        assert!(matches!(out[0], Action::Capture { rect: [0, 0, 256, 400], row_offset: 0, .. }), "{out:?}");
        assert!(matches!(out[1], Action::Filter { dst_rect: [0, 0, 128, 200], .. }));
        out.clear();
        assert!(m.copy(a + 0xE00, 0x100, 0xF00, a + 0xF00, 0x100, 0xF00, 0x1900, &mut out));
        assert!(m.copy(a + 0x18000, 0xF000, 0, a + 0x19000, 0xF000, 0, 0xF00, &mut out));
        assert!(m.copy(a + 0x1000, 0xF000, 0, a, 0xF000, 0, 0xF00, &mut out));
        assert_eq!(out.len(), 3);
        out.clear();
        m.filter(a, a, 128, 208, 0, &mut out);
        assert!(matches!(&out[..], [Action::Filter { src_rect: [0, 0, 128, 208], dst_rect: [0, 0, 64, 104], .. }]), "{out:?}");
        m.fill(a + 0xC000, a + 0x13000, 0);
        m.fill(a + 0x12800, a + 0x19800, 0);
        out.clear();
        m.filter(a, 0x180A_CA80, 64, 128, 0, &mut out);
        assert_eq!(out.len(), 1, "{out:?}");
        out.clear();
        // the 64x128 view of the half-size picture: its first 104 rows
        // come from the 64x104 filter output, the same surface
        let (d, at) = m.read(a, 64, 0, 128, 0, 104, &mut out);
        assert_eq!((d.key.width, at), (64, 0));
        assert!(out.is_empty(), "{out:?}");
        // the 128x256 target, drawn in rows 100..200: the fill clears it
        let (t, at) = m.write(a, 128, 0, 256, 100, 200, &mut out).unwrap();
        assert_eq!((t.key.width, at), (128, 0));
        assert!(out.iter().all(|x| matches!(x, Action::Clear { value: 0, .. })), "{out:?}");
        assert!(!out.is_empty());
        // the final pass reads the display: a capture
        out.clear();
        m.read(display.key.addr, 256, 0, 512, 0, 400, &mut out);
        assert!(matches!(out[..], [Action::Capture { .. }]), "{out:?}");
    }
}
