//! dispatch: finding the code for a guest address in one or two looks
//! instead of a search. a code mod's patch module has tens of thousands of
//! entries (37,002 for V.O.W Edition), and every call the game's own code
//! makes into a function the mod changed asks for one of them
//! (Host::redirect); a binary search of them was about a third of the time
//! in V.O.W's code.
//!
//! AddressMap is built once from a module's entries and never changes, so
//! nothing has to be forgotten when a saved state is loaded, a mod is turned
//! off (its module goes, and its map with it) or the code checks itself
//! again: it answers exactly what a search of the same entries would.

/// a key no slot holds: the map's empty slots. no code starts there (an odd
/// address past the 3DS's user memory), so leaving it out answers right.
const EMPTY: u32 = u32::MAX;

/// guest addresses (bit 0 set for Thumb) to values, in an open-addressed
/// table at most half full: a look at the slot the address hashes to, and
/// the next ones until the address or an empty slot, which with the table
/// this empty is one slot nearly always, in one cache line.
pub struct AddressMap<T: Copy> {
    slots: Box<[(u32, Option<T>)]>,
    /// slots.len() - 1, a power of two less one.
    mask: usize,
    /// how far right the hash is shifted to give a slot, 32 - log2(slots).
    shift: u32,
    len: usize,
}

impl<T: Copy> AddressMap<T> {
    /// a map of entries, the first of an address that comes twice winning.
    pub fn new(entries: impl IntoIterator<Item = (u32, T)>) -> AddressMap<T> {
        let entries: Vec<(u32, T)> = entries.into_iter().filter(|&(address, _)| address != EMPTY).collect();
        // at most half full, and never under 16 slots
        let size = (entries.len() * 2).next_power_of_two().max(16);
        let mut map = AddressMap { slots: vec![(EMPTY, None); size].into_boxed_slice(), mask: size - 1, shift: 32 - size.trailing_zeros(), len: 0 };
        for (address, value) in entries {
            let mut i = map.slot(address);
            loop {
                match map.slots[i].0 {
                    EMPTY => {
                        map.slots[i] = (address, Some(value));
                        map.len += 1;
                        break;
                    }
                    key if key == address => break,
                    _ => i = (i + 1) & map.mask,
                }
            }
        }
        map
    }

    /// the slot an address hashes to: the top bits of the address times the
    /// golden ratio, which spreads ARM addresses (a step of four) and Thumb
    /// ones (two, bit 0 set) over every slot alike.
    #[inline(always)]
    fn slot(&self, address: u32) -> usize {
        (address.wrapping_mul(0x9E37_79B1) >> self.shift) as usize
    }

    /// the value for address, if it has one.
    #[inline]
    pub fn get(&self, address: u32) -> Option<T> {
        let mut i = self.slot(address);
        loop {
            // SAFETY: i is masked to the table, whose length is mask + 1
            let (key, value) = unsafe { *self.slots.get_unchecked(i) };
            if key == address {
                // EMPTY itself is never a key: it finds an empty slot, None
                return value;
            }
            if key == EMPTY {
                return None;
            }
            i = (i + 1) & self.mask;
        }
    }

    /// how many addresses the map holds.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// a binary search of the same entries, as PatchModule::lookup did.
    fn searched(entries: &[(u32, u32)], address: u32) -> Option<u32> {
        entries.binary_search_by_key(&address, |&(at, _)| at).ok().map(|i| entries[i].1)
    }

    /// entries like a patch module's: ARM functions a word apart, Thumb ones
    /// two bytes apart with bit 0 set, in the game's code and in pages a mod
    /// added after it (V.O.W's 0x0077B000 to 0x00824000).
    fn patch_like() -> Vec<(u32, u32)> {
        let mut entries = Vec::new();
        let mut seed = 0x1234_5678u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for i in 0..30_000u32 {
            let address = 0x0010_0000 + (next() % 0x0067_B000) & !3;
            entries.push((address, i));
        }
        for i in 0..7_000u32 {
            let address = 0x0077_B000 + (next() % 0xA_9000) & !1;
            entries.push((address | 1, 30_000 + i));
        }
        entries.sort_unstable_by_key(|&(address, _)| address);
        entries.dedup_by_key(|entry| entry.0);
        entries
    }

    #[test]
    fn it_answers_what_a_search_does() {
        let entries = patch_like();
        let map = AddressMap::new(entries.iter().copied());
        assert_eq!(map.len(), entries.len());
        for &(address, value) in &entries {
            assert_eq!(map.get(address), Some(value), "0x{address:08X}");
            // the same address in the other state is another entry
            assert_eq!(map.get(address ^ 1), searched(&entries, address ^ 1), "0x{:08X}", address ^ 1);
        }
        // every address around the entries, entries or not
        for address in (0x000F_F000..0x0083_0000u32).step_by(3) {
            assert_eq!(map.get(address), searched(&entries, address), "0x{address:08X}");
        }
    }

    #[test]
    fn arm_and_thumb_at_the_same_word_are_apart() {
        let map = AddressMap::new([(0x0077_B000, 1u8), (0x0077_B001, 2), (0x0077_B003, 3)]);
        assert_eq!(map.get(0x0077_B000), Some(1));
        assert_eq!(map.get(0x0077_B001), Some(2));
        assert_eq!(map.get(0x0077_B002), None);
        assert_eq!(map.get(0x0077_B003), Some(3));
        assert_eq!(map.get(0x0077_B004), None);
    }

    #[test]
    fn an_empty_map_has_nothing() {
        let map: AddressMap<u32> = AddressMap::new([]);
        assert!(map.is_empty());
        assert_eq!(map.get(0), None);
        assert_eq!(map.get(0x0010_0000), None);
        assert_eq!(map.get(EMPTY), None);
    }

    #[test]
    fn the_empty_key_is_never_found() {
        let map = AddressMap::new([(EMPTY, 1u32), (0x0010_0000, 2)]);
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(EMPTY), None);
        assert_eq!(map.get(0x0010_0000), Some(2));
    }

    #[test]
    fn the_first_of_an_address_given_twice_wins() {
        let map = AddressMap::new([(0x0010_0000, 1u32), (0x0010_0000, 2)]);
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(0x0010_0000), Some(1));
    }

    /// addresses that all hash to one slot still each find their own, and
    /// a miss runs on to the empty slot after them.
    #[test]
    fn collisions_probe_on() {
        let size = 64; // the table for 32 entries
        let shift = 32 - (size as u32).trailing_zeros();
        let hash = |address: u32| address.wrapping_mul(0x9E37_79B1) >> shift;
        let colliding: Vec<u32> = (0x0010_0000..0x0100_0000u32).step_by(4).filter(|&a| hash(a) == hash(0x0010_0000)).take(32).collect();
        assert_eq!(colliding.len(), 32);
        let map = AddressMap::new(colliding.iter().enumerate().map(|(i, &a)| (a, i)));
        for (i, &address) in colliding.iter().enumerate() {
            assert_eq!(map.get(address), Some(i));
        }
        let missing = (colliding[31] + 4..).step_by(4).find(|&a| hash(a) == hash(0x0010_0000)).unwrap();
        assert_eq!(map.get(missing), None);
    }

    /// how long a look takes, the map against the search it replaced, over
    /// entries like V.O.W's and calls spread over a few hundred of them.
    /// `cargo test -p zakuro-core --features emulation --release --lib
    /// dispatch -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn speed_against_the_search() {
        // 16 bytes an entry, as a patch module's (address, code pointer)
        let entries: Vec<(u32, u64)> = patch_like().into_iter().map(|(a, v)| (a, v as u64)).collect();
        let map = AddressMap::new(entries.iter().copied());
        let calls: Vec<u32> = (0..620).map(|i| entries[(i * 59) % entries.len()].0).collect();
        let rounds = 20_000;
        let time = |f: &dyn Fn(u32) -> Option<u64>| {
            let started = std::time::Instant::now();
            let mut sum = 0u64;
            for _ in 0..rounds {
                for &address in &calls {
                    sum = sum.wrapping_add(std::hint::black_box(f(std::hint::black_box(address))).unwrap_or(0));
                }
            }
            (started.elapsed().as_secs_f64() * 1e9 / (rounds * calls.len()) as f64, sum)
        };
        let search = |address: u32| entries.binary_search_by_key(&address, |&(at, _)| at).ok().map(|i| entries[i].1);
        let (searched_ns, a) = time(&search);
        let (map_ns, b) = time(&|address| map.get(address));
        assert_eq!(a, b);
        println!("dispatch: {} entries, a search {searched_ns:.2} ns, the map {map_ns:.2} ns a look", entries.len());
    }

    /// the table is at most half full, so a miss ends soon.
    #[test]
    fn it_is_at_most_half_full() {
        for count in [0usize, 1, 7, 8, 9, 1000, 37_002] {
            let map = AddressMap::new((0..count as u32).map(|i| (0x0010_0000 + 4 * i, i)));
            assert_eq!(map.len(), count);
            assert!(map.slots.len() >= 2 * count && map.slots.len() >= 16, "{count}: {}", map.slots.len());
            assert!(map.slots.len().is_power_of_two());
        }
    }
}
