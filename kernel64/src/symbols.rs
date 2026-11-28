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

// The table itself - its format, parsing and lookup - is in the abi
// crate, shared with the profiler (`profile`, K140), which reads a copy.
pub use abi::symbols::{Table, BLOCK_SIZE, MAGIC};

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
