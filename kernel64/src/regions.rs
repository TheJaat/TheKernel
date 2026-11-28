//! Where mappings go: the free and used parts of a process's mapping
//! area.
//!
//! When a process maps a memory object, the kernel picks the address.
//! Letting the process choose would work too, but every check it would
//! need - aligned? inside user space? clear of the program, the stack,
//! other mappings? - disappears if the kernel simply hands out addresses
//! from an area reserved for it.
//!
//! First fit over a sorted list: the lowest gap that is big enough. Each
//! mapping is followed by one unmapped guard page, so running off the
//! end of a buffer faults instead of landing in the next one.
//!
//! Every region has an unmapped page after it (its guard), and so one
//! before it too: the previous region's guard, or - for the first - the
//! page below the area, which the kernel keeps clear (process.rs). A
//! stack that grows down, in a region, overflows into a fault.
//!
//! Generic over what each region records (the kernel stores the memory
//! object, or that the region is reserved memory filled in as it is
//! touched), so it is tested on the host.

use alloc::vec::Vec;
use core::ops::Range;

pub const PAGE: u64 = 4096;

pub struct Regions<T> {
    area: Range<u64>,
    /// Sorted by start; ranges exclude their guard page.
    used: Vec<(Range<u64>, T)>,
}

impl<T> Regions<T> {
    pub fn new(area: Range<u64>) -> Regions<T> {
        assert!(area.start.is_multiple_of(PAGE) && area.end.is_multiple_of(PAGE));
        Regions { area, used: Vec::new() }
    }

    /// Finds room for `bytes` (rounded up to pages) and records `value`
    /// there. Returns the start, or None if the area is full.
    pub fn allocate(&mut self, bytes: u64, value: T) -> Option<u64> {
        let size = bytes.checked_add(PAGE - 1)? & !(PAGE - 1);
        if size == 0 {
            return None;
        }
        let mut start = self.area.start;
        let mut position = self.used.len();
        for (n, (range, _)) in self.used.iter().enumerate() {
            if start.checked_add(size + PAGE)? <= range.start {
                position = n;
                break;
            }
            start = range.end + PAGE; // past it and its guard
        }
        if start.checked_add(size + PAGE)? > self.area.end {
            return None;
        }
        self.used.insert(position, (start..start + size, value));
        Some(start)
    }

    /// Removes the region starting at `start`.
    pub fn remove(&mut self, start: u64) -> Option<(Range<u64>, T)> {
        let n = self.used.iter().position(|(range, _)| range.start == start)?;
        Some(self.used.remove(n))
    }

    /// The region `address` falls in (not counting guard pages), and
    /// what it records.
    pub fn find(&self, address: u64) -> Option<(&Range<u64>, &T)> {
        // The last region starting at or below `address`: the only one
        // that can hold it, the list being sorted and the regions apart.
        let n = self.used.partition_point(|(range, _)| range.start <= address).checked_sub(1)?;
        let (range, value) = &self.used[n];
        range.contains(&address).then_some((range, value))
    }

    /// As `find`, with what the region records open to change.
    pub fn find_mut(&mut self, address: u64) -> Option<(&Range<u64>, &mut T)> {
        let n = self.used.partition_point(|(range, _)| range.start <= address).checked_sub(1)?;
        let (range, value) = &mut self.used[n];
        if range.contains(&address) { Some((&*range, value)) } else { None }
    }

    /// Every region and its value, lowest first - for a program's crash
    /// report (K138), which lists its memory.
    pub fn iter(&self) -> impl Iterator<Item = (&Range<u64>, &T)> {
        self.used.iter().map(|(range, value)| (range, value))
    }

    pub fn len(&self) -> usize {
        self.used.len()
    }

    pub fn is_empty(&self) -> bool {
        self.used.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Range<u64> = 0x10_0000..0x20_0000; // 256 pages

    #[test]
    fn what_a_region_records_can_be_changed_in_place() {
        let mut r = Regions::new(AREA);
        let a = r.allocate(3 * PAGE, 1).unwrap();
        *r.find_mut(a + PAGE).unwrap().1 = 7;
        assert_eq!(r.find(a).map(|(_, v)| *v), Some(7));
        assert!(r.find_mut(a + 3 * PAGE).is_none(), "the guard page is no region's");
    }

    #[test]
    fn regions_are_page_sized_and_guarded() {
        let mut r = Regions::new(AREA);
        let a = r.allocate(1, "a").unwrap();
        let b = r.allocate(PAGE + 1, "b").unwrap();
        let c = r.allocate(PAGE, "c").unwrap();
        assert_eq!(a, AREA.start);
        assert_eq!(b, a + 2 * PAGE, "one page, then a guard");
        assert_eq!(c, b + 3 * PAGE, "two pages, then a guard");
    }

    #[test]
    fn freed_gaps_are_reused_lowest_first() {
        let mut r = Regions::new(AREA);
        let a = r.allocate(PAGE, 1).unwrap();
        let b = r.allocate(4 * PAGE, 2).unwrap();
        let _c = r.allocate(PAGE, 3).unwrap();
        assert_eq!(r.remove(b), Some((b..b + 4 * PAGE, 2)));
        assert_eq!(r.allocate(8 * PAGE, 4).map(|x| x > b), Some(true), "too big for the gap");
        assert_eq!(r.allocate(2 * PAGE, 5), Some(b), "fits the gap");
        assert_eq!(r.remove(a + PAGE), None, "not a region start");
        assert_eq!(r.len(), 4);
    }

    #[test]
    fn an_address_finds_its_region_and_no_other() {
        let mut r = Regions::new(AREA);
        assert_eq!(r.find(AREA.start), None, "nothing yet");
        let a = r.allocate(2 * PAGE, 'a').unwrap();
        let b = r.allocate(PAGE, 'b').unwrap();
        assert_eq!(r.find(a), Some((&(a..a + 2 * PAGE), &'a')));
        assert_eq!(r.find(a + 2 * PAGE - 1).map(|x| *x.1), Some('a'), "its last byte");
        assert_eq!(r.find(a + 2 * PAGE), None, "its guard page");
        assert_eq!(r.find(b + 100).map(|x| *x.1), Some('b'));
        assert_eq!(r.find(b + PAGE), None, "past the last");
        assert_eq!(r.find(AREA.start - 1), None, "below the area");
        r.remove(a);
        assert_eq!(r.find(a), None, "removed");
        assert_eq!(r.find(b).map(|x| *x.1), Some('b'));
    }

    #[test]
    fn a_full_area_and_silly_sizes_are_refused() {
        let mut r = Regions::new(AREA);
        assert_eq!(r.allocate(0, ()), None);
        assert_eq!(r.allocate(u64::MAX, ()), None, "no overflow");
        assert_eq!(r.allocate(256 * PAGE, ()), None, "no room for its guard");
        assert!(r.allocate(255 * PAGE, ()).is_some());
        assert_eq!(r.allocate(1, ()), None);
    }
}
