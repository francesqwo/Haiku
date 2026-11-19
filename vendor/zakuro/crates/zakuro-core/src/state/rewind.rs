//! pulse: Divine Pulse's snapshots of the console, kept in memory only.
//!
//! A saved state (state/mod.rs) is the whole console: about 140 MB of
//! memory and half a megabyte of everything else. Taking one at every
//! action of a battle map that way would cost hundreds of milliseconds and
//! gigabytes, so a rewind keeps memory as the pages that changed:
//!
//! - `shadow` is a copy of the four memories (FCRAM, VRAM, DSP RAM, AXI
//!   WRAM) as they were at the newest snapshot. A snapshot compares memory
//!   with it page by page (4 KiB, memcmp, on a few threads) and keeps each
//!   page that differs, LZ4-compressed on its own, then brings the shadow
//!   up to date. The rest of the console (registers, kernel, services,
//!   GPU registers, the memory map) is `System::save_rest`, LZ4-compressed.
//! - `base` holds, for every page that ever changed, what it held before
//!   its first change, so the oldest snapshot can be rebuilt; a page that
//!   never changed is in the shadow as it always was.
//! - the page at snapshot k is the newest snapshot at or before k that has
//!   it, else `base`, else the shadow.
//! - going back to k writes only the pages that differ from k: those that
//!   changed since the newest snapshot (memory against the shadow) and
//!   those any later snapshot holds. Then `System::load_rest` takes the rest
//!   exactly as loading a saved state does.
//! - the store is bounded: past `budget` bytes the oldest snapshot is folded
//!   into `base` (its pages become what the next one starts from) and can no
//!   longer be gone back to, as Mila's Turnwheel's limited record does.
//!
//! Nothing here touches a file: snapshots live and die with the process.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use rayon::prelude::*;

use crate::System;

pub const PAGE: usize = 4096;
/// a page's index across the four memories: the memory in the top bits.
const REGION_SHIFT: u32 = 28;

fn region_of(page: u32) -> (usize, usize) {
    ((page >> REGION_SHIFT) as usize, (page & ((1 << REGION_SHIFT) - 1)) as usize)
}

fn memories(system: &System) -> [&[u8]; 4] {
    let phys = &system.memory.phys;
    [&phys.fcram, &phys.vram, &phys.dsp_ram, &phys.axi_wram]
}

fn memories_mut(system: &mut System) -> [&mut [u8]; 4] {
    let phys = &mut system.memory.phys;
    [&mut phys.fcram, &mut phys.vram, &mut phys.dsp_ram, &mut phys.axi_wram]
}

fn pack(bytes: &[u8]) -> Box<[u8]> {
    lz4_flex::compress_prepend_size(bytes).into_boxed_slice()
}

fn unpack(bytes: &[u8]) -> Vec<u8> {
    lz4_flex::decompress_size_prepended(bytes).unwrap_or_else(|_| vec![0; PAGE])
}

/// one snapshot.
struct Snap {
    id: u64,
    /// System::save_rest, in LZ4
    rest: Vec<u8>,
    /// the pages that changed since the snapshot before, sorted by page
    pages: Vec<(u32, Box<[u8]>)>,
    bytes: usize,
    /// pulse2: the native sound layer's voices, which `save_rest` leaves
    /// out (saved states do not keep them)
    sound: crate::pcos::snd::Kept,
    /// pulsefix: and the AAC decoder's settings, as a saved state keeps them
    aac: crate::pcos::snd::aac::Saved,
    /// pulsefix: the overrides' own state and the tails waiting (keep.rs)
    host: super::keep::Kept,
    /// pulsefix, tests: the console as the check compares it (check.rs)
    picture: Option<super::check::Picture>,
}

impl Snap {
    fn page(&self, page: u32) -> Option<&[u8]> {
        self.pages.binary_search_by_key(&page, |(p, _)| *p).ok().map(|i| &*self.pages[i].1)
    }
}

/// what one snapshot or one rewind cost.
#[derive(Clone, Copy, Debug, Default)]
pub struct Cost {
    /// comparing memory with the shadow
    pub scan: Duration,
    /// compressing (a snapshot) or decompressing and writing (a rewind) pages
    pub pages_time: Duration,
    /// the rest of the console, written or taken
    pub rest: Duration,
    pub total: Duration,
    /// pages kept or written
    pub pages: usize,
    /// bytes the snapshot added to the store
    pub bytes: usize,
}

pub struct Rewind {
    shadow: [Vec<u8>; 4],
    base: HashMap<u32, Box<[u8]>>,
    base_bytes: usize,
    snaps: Vec<Snap>,
    snap_bytes: usize,
    /// the most the snapshots and base may take, in bytes
    pub budget: usize,
    next_id: u64,
    /// snapshots folded away for room
    pub folded: u64,
    pool: Option<rayon::ThreadPool>,
    /// pulsefix, tests: each snapshot keeps a picture for the check, and
    /// each restore compares (`last_check`)
    pub verify: bool,
    /// pulsefix, tests: what the check found at the last restore
    pub last_check: Option<Result<super::check::Report, String>>,
    /// pulsefix, tests: the overrides' state is left as it is at a restore
    /// (FATES_PULSE_HOSTKEEP=0: the behaviour before, for the check to show)
    pub keep_host: bool,
}

impl Default for Rewind {
    fn default() -> Self {
        Rewind::new(384 << 20)
    }
}

impl Rewind {
    pub fn new(budget: usize) -> Rewind {
        // a few threads, not all of them: the machine stays responsive
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 4);
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).thread_name(|i| format!("pulse-{i}")).build().ok();
        Rewind { shadow: Default::default(), base: HashMap::new(), base_bytes: 0, snaps: Vec::new(), snap_bytes: 0, budget, next_id: 1, folded: 0, pool, verify: false, last_check: None, keep_host: true }
    }

    fn run<R: Send>(&self, f: impl FnOnce() -> R + Send) -> R {
        match &self.pool {
            Some(pool) => pool.install(f),
            None => f(),
        }
    }

    /// forgets every snapshot (the map ended); the shadow's memory is kept
    /// for the next map.
    pub fn clear(&mut self) {
        self.base.clear();
        self.base_bytes = 0;
        self.snaps.clear();
        self.snap_bytes = 0;
    }

    pub fn len(&self) -> usize {
        self.snaps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.snaps.is_empty()
    }

    /// the snapshots' ids, oldest first.
    pub fn ids(&self) -> Vec<u64> {
        self.snaps.iter().map(|snap| snap.id).collect()
    }

    pub fn position(&self, id: u64) -> Option<usize> {
        self.snaps.iter().position(|snap| snap.id == id)
    }

    /// the bytes the store holds: the shadow, base and the snapshots.
    pub fn bytes(&self) -> (usize, usize, usize) {
        (self.shadow.iter().map(Vec::len).sum(), self.base_bytes, self.snap_bytes)
    }

    /// a snapshot of the console as it is, between frames. Returns its id.
    pub fn snapshot(&mut self, system: &mut System) -> Result<(u64, Cost), String> {
        let started = Instant::now();
        let rest_whole = system.save_rest()?;
        let rest = lz4_flex::compress_prepend_size(&rest_whole);
        // pulsefix: what lives outside System: the overrides' own state, the
        // tails waiting, the AAC decoder's settings
        let host = super::keep::keep();
        let aac = system.services.dsp.native.aac.saved();
        let picture = self.verify.then(|| super::check::Picture::take(system, &rest_whole));
        let rest_time = started.elapsed();
        let mut cost = Cost { rest: rest_time, ..Cost::default() };
        let live = memories(system);
        let first = self.snaps.is_empty();
        if first || self.shadow.iter().zip(live.iter()).any(|(shadow, live)| shadow.len() != live.len()) {
            // the first of a map: the shadow is memory as it is
            for (shadow, live) in self.shadow.iter_mut().zip(live.iter()) {
                shadow.clear();
                shadow.extend_from_slice(live);
            }
            if !first {
                self.clear();
            }
            let snap = Snap { id: self.next_id, bytes: rest.len(), rest, pages: Vec::new(), sound: system.services.dsp.native.keep(), aac, host, picture };
            self.next_id += 1;
            self.snap_bytes += snap.bytes;
            let id = snap.id;
            self.snaps.push(snap);
            cost.total = started.elapsed();
            cost.bytes = self.snap_bytes;
            return Ok((id, cost));
        }
        let scan_from = Instant::now();
        let changed = self.changed(&live);
        cost.scan = scan_from.elapsed();
        let pack_from = Instant::now();
        let shadow = &self.shadow;
        let base = &self.base;
        let packed: Vec<(u32, Box<[u8]>, Option<Box<[u8]>>)> = self.run(|| {
            changed
                .par_iter()
                .map(|&page| {
                    let (region, index) = region_of(page);
                    let at = index * PAGE;
                    let now = pack(&live[region][at..at + PAGE]);
                    let before = (!base.contains_key(&page)).then(|| pack(&shadow[region][at..at + PAGE]));
                    (page, now, before)
                })
                .collect()
        });
        let mut pages = Vec::with_capacity(packed.len());
        let mut bytes = rest.len();
        for (page, now, before) in packed {
            let (region, index) = region_of(page);
            let at = index * PAGE;
            self.shadow[region][at..at + PAGE].copy_from_slice(&live[region][at..at + PAGE]);
            if let Some(before) = before {
                self.base_bytes += before.len();
                self.base.insert(page, before);
            }
            bytes += now.len();
            pages.push((page, now));
        }
        cost.pages_time = pack_from.elapsed();
        cost.pages = pages.len();
        cost.bytes = bytes;
        let snap = Snap { id: self.next_id, rest, pages, bytes, sound: system.services.dsp.native.keep(), aac, host, picture };
        self.next_id += 1;
        self.snap_bytes += bytes;
        let id = snap.id;
        self.snaps.push(snap);
        // bounded: the oldest give way
        while self.base_bytes + self.snap_bytes > self.budget && self.snaps.len() > 2 {
            self.fold_oldest();
        }
        cost.total = started.elapsed();
        Ok((id, cost))
    }

    /// the oldest snapshot goes into base: what its pages held is what the
    /// next one starts from.
    fn fold_oldest(&mut self) {
        let oldest = self.snaps.remove(0);
        self.snap_bytes -= oldest.bytes;
        for (page, bytes) in oldest.pages {
            self.base_bytes += bytes.len();
            if let Some(old) = self.base.insert(page, bytes) {
                self.base_bytes -= old.len();
            }
        }
        self.folded += 1;
    }

    /// the pages memory has that differ from the shadow.
    fn changed(&self, live: &[&[u8]; 4]) -> Vec<u32> {
        let shadow = &self.shadow;
        self.run(|| {
            let mut out = Vec::new();
            for region in 0..4 {
                let found: Vec<u32> = live[region]
                    .par_chunks(PAGE)
                    .zip(shadow[region].par_chunks(PAGE))
                    .enumerate()
                    .filter(|(_, (a, b))| a != b)
                    .map(|(index, _)| (region as u32) << REGION_SHIFT | index as u32)
                    .collect();
                out.extend(found);
            }
            out
        })
    }

    /// puts the console back as snapshot `id` had it. With `keep`, every
    /// snapshot after it is forgotten and the store goes on from it (a
    /// rewind made); without, the store is left as it is (a preview).
    pub fn restore(&mut self, system: &mut System, id: u64, keep: bool) -> Result<Cost, String> {
        let started = Instant::now();
        let k = self.position(id).ok_or("that moment is no longer kept")?;
        let rest = lz4_flex::decompress_size_prepended(&self.snaps[k].rest).map_err(|error| format!("a snapshot is damaged, {error}"))?;
        let mut cost = Cost::default();
        let live = memories(system);
        if live.iter().zip(self.shadow.iter()).any(|(live, shadow)| live.len() != shadow.len()) {
            return Err("the console's memory is not the size the snapshot has".into());
        }
        // the pages that may differ from snapshot k
        let scan_from = Instant::now();
        let mut wanted: HashSet<u32> = self.changed(&live).into_iter().collect();
        for snap in &self.snaps[k + 1..] {
            wanted.extend(snap.pages.iter().map(|(page, _)| *page));
        }
        cost.scan = scan_from.elapsed();
        let pages_from = Instant::now();
        // the newest snapshot at or before k with each of them
        let mut from: HashMap<u32, usize> = HashMap::with_capacity(wanted.len());
        for (index, snap) in self.snaps[..=k].iter().enumerate() {
            for (page, _) in &snap.pages {
                if wanted.contains(page) {
                    from.insert(*page, index);
                }
            }
        }
        let wanted: Vec<u32> = wanted.into_iter().collect();
        let snaps = &self.snaps;
        let base = &self.base;
        let shadow = &self.shadow;
        let values: Vec<(u32, Vec<u8>)> = self.run(|| {
            wanted
                .par_iter()
                .map(|&page| {
                    let value = match from.get(&page) {
                        Some(&index) => unpack(snaps[index].page(page).expect("listed above")),
                        None => match base.get(&page) {
                            Some(bytes) => unpack(bytes),
                            None => {
                                let (region, index) = region_of(page);
                                shadow[region][index * PAGE..(index + 1) * PAGE].to_vec()
                            }
                        },
                    };
                    (page, value)
                })
                .collect()
        });
        let mut written = 0;
        {
            let live = memories_mut(system);
            for (page, value) in &values {
                let (region, index) = region_of(*page);
                let at = index * PAGE;
                let target = &mut live[region][at..at + PAGE];
                if target != &value[..] {
                    target.copy_from_slice(value);
                    written += 1;
                }
            }
        }
        if keep {
            for (page, value) in &values {
                let (region, index) = region_of(*page);
                let at = index * PAGE;
                self.shadow[region][at..at + PAGE].copy_from_slice(value);
            }
            for snap in self.snaps.drain(k + 1..) {
                self.snap_bytes -= snap.bytes;
            }
        }
        cost.pages_time = pages_from.elapsed();
        cost.pages = written;
        let rest_from = Instant::now();
        // pulse2: load_rest leaves the native sound layer as new (saved
        // states do not keep it), which lost every voice playing, the
        // map's music too, until the game started a track again (the next
        // phase). Its voices come back as the snapshot had them instead.
        let carried = std::mem::take(&mut system.services.dsp.native);
        let loaded = system.load_rest(&rest, &mut |_| {});
        system.services.dsp.native = carried;
        loaded?;
        system.services.dsp.native.put_back(&self.snaps[k].sound);
        // pulsefix: the AAC decoder's settings, and the overrides' own state
        // and the tails waiting, as the moment had them (load_rest dropped
        // the tails waiting now; the variables still held now's)
        system.services.dsp.native.aac.put_back(self.snaps[k].aac);
        if self.keep_host {
            super::keep::put_back(&self.snaps[k].host);
        }
        cost.rest = rest_from.elapsed();
        cost.total = started.elapsed();
        // pulsefix, tests: everything compared with the moment
        self.last_check = self.snaps[k].picture.as_ref().map(|picture| super::check::compare(system, picture));
        Ok(cost)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{MemoryRegion, MemoryState, Permission};
    use crate::services::tests::system_with_thread;

    /// snapshots taken as memory changes come back exactly, newest and
    /// oldest, previewed and kept.
    #[test]
    fn snapshots_put_memory_back() {
        let _serial = crate::state::serial();
        let (mut system, _) = system_with_thread();
        let page = system.memory.phys.allocate(MemoryRegion::Application, 0x4000).unwrap();
        system.memory.map(0x0800_0000, page.addr, 0x4000, Permission::RW, MemoryState::Private);
        let mut rewind = Rewind::new(64 << 20);
        let mut ids = Vec::new();
        for step in 0..5u8 {
            system.memory.write_bytes(0x0800_0000 + step as u32 * 0x1000, &[step + 1; 16]);
            system.cpu.regs[3] = step as u32;
            ids.push(rewind.snapshot(&mut system).unwrap().0);
        }
        let read = |system: &mut System, at: u32| {
            let mut b = [0u8; 1];
            system.memory.read_bytes(at, &mut b);
            b[0]
        };
        // a change after the newest, then a preview of the second
        system.memory.write_bytes(0x0800_3000, &[99; 4]);
        rewind.restore(&mut system, ids[1], false).unwrap();
        assert_eq!(read(&mut system, 0x0800_0000), 1);
        assert_eq!(read(&mut system, 0x0800_1000), 2);
        assert_eq!(read(&mut system, 0x0800_2000), 0);
        assert_eq!(read(&mut system, 0x0800_3000), 0);
        assert_eq!(system.cpu.regs[3], 1);
        // the newest again
        rewind.restore(&mut system, ids[4], false).unwrap();
        assert_eq!(read(&mut system, 0x0800_3000), 4);
        assert_eq!(system.cpu.regs[3], 4);
        // kept: the later ones are gone, and play goes on from there
        rewind.restore(&mut system, ids[2], true).unwrap();
        assert_eq!(rewind.len(), 3);
        assert_eq!(read(&mut system, 0x0800_2000), 3);
        assert_eq!(read(&mut system, 0x0800_3000), 0);
        system.memory.write_bytes(0x0800_3000, &[7; 4]);
        let after = rewind.snapshot(&mut system).unwrap().0;
        rewind.restore(&mut system, ids[0], false).unwrap();
        assert_eq!(read(&mut system, 0x0800_0000), 1);
        assert_eq!(read(&mut system, 0x0800_1000), 0);
        rewind.restore(&mut system, after, false).unwrap();
        assert_eq!(read(&mut system, 0x0800_3000), 7);
        assert_eq!(read(&mut system, 0x0800_2000), 3);
    }

    /// past the budget the oldest are folded away, and what is left still
    /// comes back exactly.
    #[test]
    fn the_store_stays_in_its_budget() {
        let _serial = crate::state::serial();
        let (mut system, _) = system_with_thread();
        let page = system.memory.phys.allocate(MemoryRegion::Application, 0x40000).unwrap();
        system.memory.map(0x0800_0000, page.addr, 0x40000, Permission::RW, MemoryState::Private);
        let mut rewind = Rewind::new(1 << 20);
        let mut ids = Vec::new();
        let mut noise = 12345u32;
        for step in 0..12u32 {
            // pages that do not compress
            let mut bytes = vec![0u8; 0x40000];
            for byte in bytes.iter_mut() {
                noise ^= noise << 13;
                noise ^= noise >> 17;
                noise ^= noise << 5;
                *byte = noise as u8;
            }
            bytes[0] = step as u8;
            system.memory.write_bytes(0x0800_0000, &bytes);
            ids.push(rewind.snapshot(&mut system).unwrap().0);
        }
        let (_, base, snaps) = rewind.bytes();
        assert!(base + snaps <= (1 << 20) + 0x50000, "{base} + {snaps}");
        assert!(rewind.folded > 0);
        let oldest = rewind.ids()[0];
        let at = ids.iter().position(|id| *id == oldest).unwrap();
        rewind.restore(&mut system, oldest, false).unwrap();
        let mut b = [0u8; 1];
        system.memory.read_bytes(0x0800_0000, &mut b);
        assert_eq!(b[0] as usize, at);
    }
}
