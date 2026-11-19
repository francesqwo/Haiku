//! the executable image of a title, split into its segments at the addresses
//! the title expects to run at.

use crate::rom::{CodeSetInfo, Error, ExHeader, Title};

use crate::discover::{Program, Source};

const PAGE_SIZE: usize = 0x1000;

pub struct Segment {
    pub base: u32,
    pub bytes: Vec<u8>,
}

impl Segment {
    pub fn end(&self) -> u32 {
        self.base + self.bytes.len() as u32
    }

    pub fn contains(&self, address: u32) -> bool {
        address >= self.base && address < self.end()
    }

    pub fn read32(&self, address: u32) -> Option<u32> {
        let offset = address.checked_sub(self.base)? as usize;
        let bytes = self.bytes.get(offset..offset + 4)?;
        Some(u32::from_le_bytes(bytes.try_into().unwrap()))
    }

    pub fn write32(&mut self, address: u32, value: u32) {
        let offset = (address - self.base) as usize;
        self.bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    pub fn read16(&self, address: u32) -> Option<u16> {
        let offset = address.checked_sub(self.base)? as usize;
        let bytes = self.bytes.get(offset..offset + 2)?;
        Some(u16::from_le_bytes(bytes.try_into().unwrap()))
    }

    /// every aligned word in the segment, with its address.
    pub fn words(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.bytes
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .map(|(i, word)| (self.base + i as u32 * 4, u32::from_le_bytes(*word)))
    }
}

pub struct Image {
    pub entry: u32,
    pub text: Segment,
    pub rodata: Segment,
    pub data: Segment,
}

impl Image {
    /// the title's own code.
    pub fn from_title(title: &Title) -> Result<Image, Error> {
        Ok(Image::from_code(&title.exheader, &title.code()?))
    }

    /// the image of code, decompressed, at the addresses header gives. the
    /// segments sit one after another in it, each padded to whole pages,
    /// the same way the loader maps them.
    pub fn from_code(header: &ExHeader, code: &[u8]) -> Image {
        let segment = |info: CodeSetInfo, offset: usize| Segment {
            base: info.address,
            bytes: code
                .get(offset..offset + info.size as usize)
                .unwrap_or_default()
                .to_vec(),
        };
        let rodata_offset = header.text.num_pages as usize * PAGE_SIZE;
        let data_offset = rodata_offset + header.rodata.num_pages as usize * PAGE_SIZE;
        Image {
            entry: header.text.address,
            text: segment(header.text, 0),
            rodata: segment(header.rodata, rodata_offset),
            data: segment(header.data, data_offset),
        }
    }

    /// the executable as discovery sees it, starting from the entry point,
    /// the exports and what modules take from it without a name. nothing
    /// says where it keeps pointers, so any word in the data segments that
    /// lands in the code is taken for one.
    pub fn into_program(self, exports: &[u32], imported: &[u32]) -> Program {
        let entry = std::iter::once((self.entry, Source::Entry));
        let exports = exports.iter().map(|&address| (address, Source::Export));
        let imported = imported.iter().map(|&address| (address, Source::Import));
        let pointers = [&self.rodata, &self.data]
            .into_iter()
            .flat_map(Segment::words)
            .filter(|&(_, value)| self.text.contains(value & !1))
            .map(|(_, value)| (value, Source::Pointer));
        let seeds = entry.chain(exports).chain(imported).chain(pointers).collect();
        Program { text: self.text, seeds, slots: None }
    }

    /// eur2: the functions the ARM C++ runtime's `__cpp_initialize__aeabi_`
    /// calls at start-up: the static initializers (`__sti___..._cpp`). It
    /// walks a table of 32-bit offsets, each relative to its own entry, so
    /// no word anywhere holds their addresses and a discovery that only
    /// follows pointers can miss them (the EUR build of Fire Emblem Fates
    /// stopped at frame 0 on one; dev/region/eur/hints-extra.txt holds them as
    /// hints). Not a seed of the discovery yet: seeding them, even after
    /// everything else, gives both builds' discovery more functions (on EUR
    /// 69 fewer hints start one), so every generated file changes and both
    /// builds need recompiling and testing again (dev/reports/eur2.md).
    /// The walker is found by its Thumb start,
    /// `push {r4-r6, lr}; ldr r4, =table - .; add r4, pc; ldr r5, =end - .;
    /// add r5, pc`; a table must lie in the data and every target in the
    /// code, else nothing is taken from it.
    pub fn init_table(&self) -> Vec<u32> {
        let text = &self.text;
        let read = |address: u32| text.read32(address).or_else(|| self.rodata.read32(address)).or_else(|| self.data.read32(address));
        let in_data = |address: u32| self.rodata.contains(address) || self.data.contains(address);
        let mut found = Vec::new();
        let bytes = &text.bytes;
        let mut at = 0;
        while at + 10 <= bytes.len() {
            let b = &bytes[at..at + 10];
            if b[0] == 0x70 && b[1] == 0xB5 && b[3] == 0x4C && b[4] == 0x7C && b[5] == 0x44 && b[7] == 0x4D && b[8] == 0x7D && b[9] == 0x44 {
                let walker = text.base + at as u32;
                // ldr rN, [pc, #imm8*4]: pc is the instruction's address + 4, word aligned
                let literal = |ldr: u32, imm: u8| ((ldr + 4) & !3) + imm as u32 * 4;
                let start = read(literal(walker + 2, b[2])).map(|offset| offset.wrapping_add(walker + 4 + 4));
                let end = read(literal(walker + 6, b[6])).map(|offset| offset.wrapping_add(walker + 8 + 4));
                if let (Some(start), Some(end)) = (start, end) {
                    if start < end && (end - start) % 4 == 0 && in_data(start) && in_data(end - 4) && end - start < 0x10000 {
                        let targets: Vec<u32> =
                            (start..end).step_by(4).filter_map(|entry| read(entry).map(|offset| entry.wrapping_add(offset))).collect();
                        if targets.iter().all(|&target| text.contains(target & !1)) {
                            found.extend(targets);
                        }
                    }
                }
            }
            at += 2;
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::{self, Mode};

    const BASE: u32 = 0x0010_0000;

    fn segment(base: u32, words: &[u32]) -> Segment {
        Segment { base, bytes: words.iter().flat_map(|w| w.to_le_bytes()).collect() }
    }

    #[test]
    fn pointers_in_data_become_functions() {
        let image = Image {
            entry: BASE,
            text: segment(BASE, &[0xE12F_FF1E, 0x4770_4770, 0xE12F_FF1E]),
            rodata: segment(0x0020_0000, &[BASE + 8, 12345]),
            data: segment(0x0030_0000, &[BASE + 5]),
        };
        let program = image.into_program(&[], &[]);
        assert_eq!(program.seeds.len(), 3);
        let analysis = discover::analyze(&program);
        assert_eq!(analysis.functions[&(BASE + 8)].source, Source::Pointer);
        assert_eq!(analysis.functions[&(BASE + 4)].mode, Mode::Thumb);
    }
}
