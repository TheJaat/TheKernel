//! The crash record (TheTaaJ K188): the kernel's last words, kept in
//! memory that survives a warm reboot, for the next boot to find and
//! hand to the system log.
//!
//! When the kernel stops - a panic, a CPU exception it cannot recover
//! from, init ending - it says why on the serial line and on the
//! screen, and halts. Unless someone was reading at that moment, that
//! is gone. A disk would keep it, but the disk's driver is a program,
//! and nothing runs any more. Memory, though, is not cleared by a reset:
//! so the last words are also written into an area of memory the kernel
//! never hands out, at the same place every boot, and the next kernel
//! looks there first.
//!
//! The area: a header, then the text.
//!
//! | Offset | Size | What |
//! |---|---|---|
//! | 0 | 8 | `MAGIC` |
//! | 8 | 4 | the text's length |
//! | 12 | 4 | its checksum (FNV-1a, 32 bits) |
//! | 16 | 8 | when the kernel began to stop: nanoseconds since 1970, 0 if unknown |
//!
//! Each write brings the length and the checksum up to date, so the
//! record is whole at every moment: the kernel may stop writing at any
//! point (a second fault, a CPU that never comes back) and what was
//! written is still found. A record that does not add up - memory that
//! firmware or a loader wrote over, or that held something else - is
//! not one.
//!
//! Pure bookkeeping on a byte slice; where the area is, and reading it
//! through the direct map, are the kernel's (logtap.rs).

use core::ops::Range;

/// "TAAJCRSH", little-endian.
pub const MAGIC: u64 = u64::from_le_bytes(*b"TAAJCRSH");
/// The area's size: 64 KiB, page aligned.
pub const AREA: u64 = 64 * 1024;
/// The header's size; the text follows.
pub const HEADER: usize = 24;

const OFFSET_BASIS: u32 = 0x811c_9dc5;
const PRIME: u32 = 0x0100_0193;

fn fnv(mut hash: u32, bytes: &[u8]) -> u32 {
    for &byte in bytes {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

fn u32_at(area: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([area[at], area[at + 1], area[at + 2], area[at + 3]])
}

fn u64_at(area: &[u8], at: usize) -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&area[at..at + 8]);
    u64::from_le_bytes(bytes)
}

/// The record in `area`, if there is one that adds up: when, and the
/// text.
pub fn read(area: &[u8]) -> Option<(u64, &[u8])> {
    if area.len() < HEADER || u64_at(area, 0) != MAGIC {
        return None;
    }
    let length = u32_at(area, 8) as usize;
    let text = area.get(HEADER..HEADER.checked_add(length)?)?;
    (fnv(OFFSET_BASIS, text) == u32_at(area, 12)).then_some((u64_at(area, 16), text))
}

/// No record any more (it was read, or this boot is starting one).
pub fn clear(area: &mut [u8]) {
    if area.len() >= HEADER {
        area[..HEADER].fill(0);
    }
}

/// A new, empty record: the kernel has begun to stop, at `time`.
pub fn begin(area: &mut [u8], time: u64) {
    if area.len() < HEADER {
        return;
    }
    area[8..12].copy_from_slice(&0u32.to_le_bytes());
    area[12..16].copy_from_slice(&OFFSET_BASIS.to_le_bytes());
    area[16..24].copy_from_slice(&time.to_le_bytes());
    area[0..8].copy_from_slice(&MAGIC.to_le_bytes());
}

/// Adds `text` to the record `begin` started - as much as fits; the
/// rest is dropped (the first words say most: what stopped it, where).
pub fn append(area: &mut [u8], text: &[u8]) {
    if area.len() < HEADER || u64_at(area, 0) != MAGIC {
        return;
    }
    let length = u32_at(area, 8) as usize;
    let room = area.len().saturating_sub(HEADER + length);
    let text = &text[..text.len().min(room)];
    if text.is_empty() {
        return;
    }
    let at = HEADER + length;
    area[at..at + text.len()].copy_from_slice(text);
    let hash = fnv(u32_at(area, 12), text);
    // The checksum first, then the length: a record cut between the two
    // does not add up, and is not taken for a whole one.
    area[12..16].copy_from_slice(&hash.to_le_bytes());
    area[8..12].copy_from_slice(&((length + text.len()) as u32).to_le_bytes());
}

/// Where the area goes: the top `AREA` bytes of usable memory, below
/// `below` and clear of every range in `avoid` (what firmware keeps for
/// itself, the ramdisk, the boot info, the kernel), page aligned - in
/// the highest usable region that has room. The same place every boot
/// of the same machine, which is what lets the next boot find it. None
/// if nowhere fits.
pub fn place(usable: impl Iterator<Item = Range<u64>>, avoid: &[Range<u64>], below: u64) -> Option<Range<u64>> {
    const PAGE: u64 = 4096;
    usable
        .filter_map(|range| {
            let floor = range.start.max(1 << 20);
            let mut end = range.end.min(below) & !(PAGE - 1);
            loop {
                let start = end.checked_sub(AREA).filter(|&start| start >= floor)?;
                // Below whatever overlaps it, and try again.
                match avoid.iter().filter(|a| a.start < end && a.end > start).map(|a| a.start).min() {
                    Some(lowest) => end = lowest & !(PAGE - 1),
                    None => return Some(start..end),
                }
            }
        })
        .max_by_key(|range| range.start)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> std::vec::Vec<u8> {
        std::vec![0u8; 4096]
    }

    #[test]
    fn what_is_written_is_found_at_every_moment() {
        let mut area = area();
        assert_eq!(read(&area), None);
        begin(&mut area, 42);
        assert_eq!(read(&area), Some((42, &b""[..])));
        append(&mut area, b"*** KERNEL PANIC: ");
        assert_eq!(read(&area), Some((42, &b"*** KERNEL PANIC: "[..])));
        append(&mut area, b"at x.rs:1\n");
        assert_eq!(read(&area), Some((42, &b"*** KERNEL PANIC: at x.rs:1\n"[..])));
        clear(&mut area);
        assert_eq!(read(&area), None);
    }

    #[test]
    fn appending_without_a_beginning_does_nothing_and_a_full_area_keeps_the_first_words() {
        let mut area = area();
        append(&mut area, b"lost");
        assert_eq!(read(&area), None);
        begin(&mut area, 1);
        append(&mut area, &[b'a'; 5000]);
        let (_, text) = read(&area).unwrap();
        assert_eq!(text.len(), 4096 - HEADER);
        append(&mut area, b"more"); // no room: nothing, and still whole
        assert_eq!(read(&area).unwrap().1.len(), 4096 - HEADER);
    }

    #[test]
    fn a_record_that_does_not_add_up_is_not_one() {
        let mut area = area();
        begin(&mut area, 7);
        append(&mut area, b"[kernel] the system has stopped\n");
        // A byte of the text written over (by firmware, a loader):
        area[HEADER + 3] ^= 1;
        assert_eq!(read(&area), None);
        // A length past the area:
        let mut area = self::area();
        begin(&mut area, 7);
        area[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(read(&area), None);
        // Too small to hold a header at all:
        let mut tiny = [0u8; 8];
        begin(&mut tiny, 1);
        append(&mut tiny, b"x");
        clear(&mut tiny);
        assert_eq!(read(&tiny), None);
    }

    #[test]
    fn the_area_goes_at_the_top_of_the_highest_region_below_the_limit() {
        const MIB: u64 = 1 << 20;
        const GIB: u64 = 1 << 30;
        // A PC with 1 GiB: low memory, then 1 MiB up to just below 1 GiB.
        let regions = [0..0x9f000, MIB..GIB - 0x20000];
        assert_eq!(place(regions.iter().cloned(), &[], 4 * GIB), Some(GIB - 0x20000 - AREA..GIB - 0x20000));
        // Memory above 4 GiB is passed over; an unaligned end is rounded down.
        let regions = [MIB..2 * GIB + 0x123, 4 * GIB..8 * GIB];
        assert_eq!(place(regions.iter().cloned(), &[], 4 * GIB), Some(2 * GIB - AREA..2 * GIB));
        // A region too small, or below 1 MiB, is no place.
        let regions = [0..0x9f000, MIB..MIB + 0x1000];
        assert_eq!(place(regions.iter().cloned(), &[], 4 * GIB), None);
        assert_eq!(place(regions.iter().cloned(), &[], u64::MAX), None);
    }

    #[test]
    fn the_area_keeps_clear_of_what_is_in_use() {
        const MIB: u64 = 1 << 20;
        // Firmware keeps the top MiB (an AArch64 board), a ramdisk lies
        // just below it: the area goes below both.
        let regions = [0x4000_0000..0x8000_0000];
        let avoid = [0x7ff0_0000..0x8000_0000, 0x7fe0_8000..0x7ff0_0000];
        assert_eq!(place(regions.iter().cloned(), &avoid, u64::MAX), Some(0x7fe0_0000 - AREA + 0x8000..0x7fe0_8000));
        // Something in the way all the way down: no place.
        let regions = [MIB..MIB + 2 * AREA];
        let avoid = [MIB + AREA..MIB + 2 * AREA, MIB + 0x1000..MIB + 0x2000];
        assert_eq!(place(regions.iter().cloned(), &avoid, u64::MAX), None);
        // What lies elsewhere is no reason to move.
        let avoid = [0..MIB];
        assert_eq!(place(regions.iter().cloned(), &avoid, u64::MAX), Some(MIB + AREA..MIB + 2 * AREA));
    }
}
