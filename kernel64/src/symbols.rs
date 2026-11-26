//! What a panic needs to say where it happened: the kernel's own symbols,
//! and the walk up the stack from one frame to its caller's.
//!
//! **The table.** The kernel keeps a fixed-size block in its read-only
//! data, marked with `MAGIC`. After the kernel is linked, the build fills
//! it in place (`scripts/fill-symbols.py`) with the addresses and names of
//! its functions - in place, so that nothing moves and there is no second
//! link. Little-endian:
//!
//! ```text
//!  MAGIC (8 bytes) | count u32 | names u32 | count x [address u64, name at u32, name length u32] | names
//! ```
//!
//! The entries are sorted by address; a name is UTF-8, without the
//! generic arguments (`quicksort`, not `quicksort::<u64, ...>`), which
//! would make the table three times bigger and the backtrace no clearer.
//!
//! **The walk.** With frame pointers (`-C force-frame-pointers`), every
//! function's frame starts with a record of two words: the caller's frame
//! pointer, then the address to return to in the caller. On x86_64 `rbp`
//! points at the record; on AArch64 `x29` does. Following the first word
//! from record to record goes from callee to caller, up the stack. A
//! panic must never fault while it does this - a fault in a panic loses
//! the report - so the kernel says, for each step, whether the next
//! record may be read (`walk`'s `may_follow`).

/// The block's first eight bytes, which the fill script looks for.
pub const MAGIC: [u8; 8] = *b"TAAJSYM1";
/// How big the block is. The x86_64 kernel's functions take some 30 KiB.
pub const BLOCK_SIZE: usize = 64 * 1024;
const HEADER: usize = 16;
const ENTRY: usize = 16;

/// A filled-in table, read where it lies.
pub struct Table<'a> {
    bytes: &'a [u8],
    count: usize,
    names: usize,
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

impl<'a> Table<'a> {
    /// The table in `bytes`, or None if it was never filled in (a count of
    /// 0) or does not hold together.
    pub fn parse(bytes: &'a [u8]) -> Option<Table<'a>> {
        if bytes.get(..8)? != MAGIC {
            return None;
        }
        let count = u32_at(bytes, 8)? as usize;
        let names = u32_at(bytes, 12)? as usize;
        let end = HEADER.checked_add(count.checked_mul(ENTRY)?)?.checked_add(names)?;
        (count > 0 && end <= bytes.len()).then_some(Table { bytes, count, names })
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    fn address(&self, index: usize) -> u64 {
        u64_at(self.bytes, HEADER + index * ENTRY).unwrap_or(0)
    }

    fn name(&self, index: usize) -> &'a str {
        let entry = HEADER + index * ENTRY;
        let names = HEADER + self.count * ENTRY;
        let (Some(at), Some(length)) = (u32_at(self.bytes, entry + 8), u32_at(self.bytes, entry + 12)) else {
            return "?";
        };
        let (at, length) = (at as usize, length as usize);
        if at.saturating_add(length) > self.names {
            return "?";
        }
        core::str::from_utf8(&self.bytes[names + at..names + at + length]).unwrap_or("?")
    }

    /// The function `address` is in, and how far into it: the entry with
    /// the highest address not above it. None below the first function.
    pub fn lookup(&self, address: u64) -> Option<(&'a str, u64)> {
        // The first entry above `address`; the one before it holds it.
        let (mut low, mut high) = (0, self.count);
        while low < high {
            let middle = low + (high - low) / 2;
            if self.address(middle) <= address {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let index = low.checked_sub(1)?;
        Some((self.name(index), address - self.address(index)))
    }
}

/// Walks the frame records from `frame`, giving `each` the return address
/// in every caller, at most `max` of them.
///
/// `read(address)` reads a word; `may_follow(from, to)` says whether the
/// record at `to` may be read, having come from the one at `from` (the
/// first is asked as `may_follow(frame, frame)`). The walk ends at a
/// return address of 0, at a record `may_follow` refuses, or after `max`.
/// Records only go up: a caller's frame is above its callee's, so a link
/// that does not climb ends the walk too - a loop cannot.
pub fn walk(frame: u64, max: usize, read: impl Fn(u64) -> u64, may_follow: impl Fn(u64, u64) -> bool, mut each: impl FnMut(u64)) {
    if frame % 8 != 0 || !may_follow(frame, frame) {
        return;
    }
    let mut at = frame;
    for _ in 0..max {
        let (caller, returns_to) = (read(at), read(at + 8));
        if returns_to == 0 {
            return;
        }
        each(returns_to);
        if caller <= at || caller % 8 != 0 || !may_follow(at, caller) {
            return;
        }
        at = caller;
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::collections::BTreeMap;
    use std::vec::Vec;

    /// A table as fill-symbols.py writes it.
    fn table(functions: &[(u64, &str)]) -> Vec<u8> {
        let mut bytes = MAGIC.to_vec();
        let names: Vec<u8> = functions.iter().flat_map(|(_, n)| n.bytes()).collect();
        bytes.extend((functions.len() as u32).to_le_bytes());
        bytes.extend((names.len() as u32).to_le_bytes());
        let mut at = 0u32;
        for (address, name) in functions {
            bytes.extend(address.to_le_bytes());
            bytes.extend(at.to_le_bytes());
            bytes.extend((name.len() as u32).to_le_bytes());
            at += name.len() as u32;
        }
        bytes.extend(names);
        bytes.resize(BLOCK_SIZE, 0);
        bytes
    }

    #[test]
    fn an_address_is_found_in_the_function_below_it() {
        let bytes = table(&[(0x1000, "start"), (0x1040, "kernel_main"), (0x2000, "panic")]);
        let t = Table::parse(&bytes).unwrap();
        assert_eq!(t.len(), 3);
        assert_eq!(t.lookup(0x1000), Some(("start", 0)));
        assert_eq!(t.lookup(0x103f), Some(("start", 0x3f)));
        assert_eq!(t.lookup(0x1040), Some(("kernel_main", 0)));
        assert_eq!(t.lookup(0x1fff), Some(("kernel_main", 0xfbf)));
        assert_eq!(t.lookup(0x9000), Some(("panic", 0x7000)));
        assert_eq!(t.lookup(0xfff), None);
    }

    #[test]
    fn an_empty_or_broken_table_is_none() {
        let mut empty = MAGIC.to_vec();
        empty.resize(BLOCK_SIZE, 0);
        assert!(Table::parse(&empty).is_none(), "never filled in: count 0");
        assert!(Table::parse(&[0u8; 64]).is_none(), "no magic");
        let mut lying = table(&[(0x1000, "start")]);
        lying[8..12].copy_from_slice(&100_000u32.to_le_bytes()); // more entries than fit
        assert!(Table::parse(&lying).is_none());
        let mut bad_name = table(&[(0x1000, "start")]);
        bad_name[HEADER + 8..HEADER + 12].copy_from_slice(&1000u32.to_le_bytes()); // a name past the end
        assert_eq!(Table::parse(&bad_name).unwrap().lookup(0x1000), Some(("?", 0)));
    }

    /// Memory as a map of words; a stack whose records link up.
    fn memory(words: &[(u64, u64)]) -> impl Fn(u64) -> u64 {
        let map: BTreeMap<u64, u64> = words.iter().copied().collect();
        move |address| map.get(&address).copied().unwrap_or(0)
    }

    #[test]
    fn the_walk_climbs_from_record_to_record() {
        // 0x100 -> 0x140 -> 0x1a0 -> its caller's link is 0: the end.
        let read = memory(&[(0x100, 0x140), (0x108, 0xaaa), (0x140, 0x1a0), (0x148, 0xbbb), (0x1a0, 0), (0x1a8, 0xccc)]);
        let mut seen = Vec::new();
        walk(0x100, 32, &read, |_, _| true, |r| seen.push(r));
        assert_eq!(seen, [0xaaa, 0xbbb, 0xccc]);
    }

    #[test]
    fn the_walk_stops_where_it_may_not_read_at_a_loop_and_at_max() {
        let read = memory(&[(0x100, 0x140), (0x108, 0xaaa), (0x140, 0x9000), (0x148, 0xbbb), (0x9008, 0xccc)]);
        let mut seen = Vec::new();
        // 0x9000 is somewhere it may not go: the walk says what it has, and stops.
        walk(0x100, 32, &read, |_, to| to < 0x1000, |r| seen.push(r));
        assert_eq!(seen, [0xaaa, 0xbbb]);
        // A link that points down, or to itself, would go round for ever.
        let looping = memory(&[(0x100, 0x100), (0x108, 0xaaa)]);
        seen.clear();
        walk(0x100, 32, &looping, |_, _| true, |r| seen.push(r));
        assert_eq!(seen, [0xaaa]);
        // At most `max`.
        let long: Vec<(u64, u64)> = (0..100u64).flat_map(|i| [(0x100 + i * 16, 0x110 + i * 16), (0x108 + i * 16, 0x5000 + i)]).collect();
        seen.clear();
        walk(0x100, 32, &memory(&long), |_, _| true, |r| seen.push(r));
        assert_eq!(seen.len(), 32);
        // A first record it may not read, or a misaligned one: nothing.
        seen.clear();
        walk(0x100, 32, &read, |_, _| false, |r| seen.push(r));
        walk(0x104, 32, &read, |_, _| true, |r| seen.push(r));
        assert!(seen.is_empty());
    }
}
