//! The physical frame allocator: which 4 KiB pages of RAM are free.
//!
//! A bitmap, one bit per frame: 1 = in use, 0 = free. For 512 MiB of RAM
//! that is 131072 bits, 16 KiB - small enough to scan, simple enough to
//! be obviously right, and every question about a frame is one bit test.
//!
//! Everything starts out "in use". The kernel then frees the ranges the
//! memory map says are usable, and re-reserves what it knows is taken
//! (itself, the ramdisk, the loader's low memory). Starting from "used"
//! means a mistake in the map can only waste memory, never hand out RAM
//! that is not really there.
//!
//! Allocation always returns the lowest free frame. That keeps early
//! page tables low in memory (below 4 GiB, where the bootloader's
//! identity map can reach them) and makes behaviour predictable in tests.

use super::addr::{PhysAddr, PAGE_SIZE};

pub struct FrameAllocator {
    bitmap: &'static mut [u64],
    frames: usize,
    free: usize,
    /// Every frame below this index is in use, so searches start here.
    hint: usize,
}

impl FrameAllocator {
    /// How many u64 words of bitmap `frames` frames need.
    pub const fn words_for(frames: usize) -> usize {
        frames.div_ceil(64)
    }

    /// Takes over `bitmap` as the allocator's storage, with every frame
    /// marked in use.
    pub fn new(bitmap: &'static mut [u64], frames: usize) -> FrameAllocator {
        assert!(bitmap.len() >= Self::words_for(frames), "bitmap too small");
        bitmap.fill(u64::MAX);
        FrameAllocator { bitmap, frames, free: 0, hint: 0 }
    }

    fn is_used(&self, index: usize) -> bool {
        self.bitmap[index / 64] & (1 << (index % 64)) != 0
    }

    fn set_used(&mut self, index: usize) {
        self.bitmap[index / 64] |= 1 << (index % 64);
    }

    fn set_free(&mut self, index: usize) {
        self.bitmap[index / 64] &= !(1 << (index % 64));
    }

    /// Marks every frame lying WHOLLY inside `start..end` free. Rounding
    /// inward: a frame only partly in a usable region is not usable.
    pub fn free_range(&mut self, start: PhysAddr, end: PhysAddr) {
        let first = (start.align_up(PAGE_SIZE).as_u64() / PAGE_SIZE) as usize;
        let last = ((end.align_down(PAGE_SIZE).as_u64() / PAGE_SIZE) as usize).min(self.frames);
        for index in first..last {
            if self.is_used(index) {
                self.set_free(index);
                self.free += 1;
            }
        }
        self.hint = self.hint.min(first);
    }

    /// Marks every frame touching `start..end` in use. Rounding outward:
    /// a frame partly used by something is not free.
    pub fn reserve_range(&mut self, start: PhysAddr, end: PhysAddr) {
        let first = (start.align_down(PAGE_SIZE).as_u64() / PAGE_SIZE) as usize;
        let last = ((end.align_up(PAGE_SIZE).as_u64() / PAGE_SIZE) as usize).min(self.frames);
        for index in first..last {
            if !self.is_used(index) {
                self.set_used(index);
                self.free -= 1;
            }
        }
    }

    /// The lowest free frame, now marked in use - or None if none is left.
    pub fn allocate(&mut self) -> Option<PhysAddr> {
        for word in self.hint / 64..self.bitmap.len() {
            let bits = self.bitmap[word];
            if bits == u64::MAX {
                continue; // all 64 in use
            }
            // The lowest 0 bit is the lowest trailing 1 of the inverse.
            let index = word * 64 + (!bits).trailing_zeros() as usize;
            if index >= self.frames {
                return None; // the bits past the last frame are never free
            }
            self.set_used(index);
            self.free -= 1;
            self.hint = index + 1;
            return Some(PhysAddr::new(index as u64 * PAGE_SIZE));
        }
        None
    }

    /// The highest free frame, now marked in use - or None if none is
    /// left. Nothing needs the top of memory in particular; this is for
    /// checking that the top of memory is really there and reachable,
    /// which handing out the lowest frame first would never show.
    pub fn allocate_highest(&mut self) -> Option<PhysAddr> {
        for word in (0..self.bitmap.len()).rev() {
            // The bits past the last frame are always 1, so a 0 bit is
            // a free frame; the highest 0 is the highest 1 of the inverse.
            let free = !self.bitmap[word];
            if free == 0 {
                continue;
            }
            let index = word * 64 + 63 - free.leading_zeros() as usize;
            self.set_used(index);
            self.free -= 1;
            return Some(PhysAddr::new(index as u64 * PAGE_SIZE));
        }
        None
    }

    /// `count` frames side by side, lowest first - for a device that
    /// reads memory itself (DMA) and needs one unbroken physical range.
    /// Returns the first; None if there is no run that long.
    pub fn allocate_contiguous(&mut self, count: usize) -> Option<PhysAddr> {
        if count == 0 {
            return None;
        }
        let mut start = self.hint;
        let mut index = start;
        while index < self.frames {
            if self.is_used(index) {
                start = index + 1;
            } else if index + 1 - start == count {
                for frame in start..=index {
                    self.set_used(frame);
                }
                self.free -= count;
                if start == self.hint {
                    self.hint = index + 1;
                }
                return Some(PhysAddr::new(start as u64 * PAGE_SIZE));
            }
            index += 1;
        }
        None
    }

    /// Returns a frame.
    ///
    /// Freeing a frame twice means two owners each think it is theirs -
    /// the classic way memory gets corrupted. That is a bug in the
    /// caller, so it panics here, at the moment of the mistake, rather
    /// than letting the frame be handed out twice later.
    pub fn free(&mut self, frame: PhysAddr) {
        assert!(frame.is_aligned(PAGE_SIZE), "freeing unaligned frame {frame:?}");
        let index = (frame.as_u64() / PAGE_SIZE) as usize;
        assert!(index < self.frames, "freeing frame {frame:?} past the end of memory");
        assert!(self.is_used(index), "double free of frame {frame:?}");
        self.set_free(index);
        self.free += 1;
        self.hint = self.hint.min(index);
    }

    /// Points the allocator at the same bitmap through a new address -
    /// for when the mapping of physical memory moves.
    ///
    /// # Safety
    /// `bitmap` must be the memory the current bitmap occupies, as seen
    /// through the new mapping.
    pub unsafe fn relocate(&mut self, bitmap: &'static mut [u64]) {
        assert_eq!(bitmap.len(), self.bitmap.len());
        self.bitmap = bitmap;
    }

    /// Whether the frame holding `address` is free. Memory past the
    /// end is not.
    pub fn is_free(&self, address: PhysAddr) -> bool {
        let index = (address.as_u64() / PAGE_SIZE) as usize;
        index < self.frames && !self.is_used(index)
    }

    pub fn free_frames(&self) -> usize {
        self.free
    }

    pub fn total_frames(&self) -> usize {
        self.frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allocator(frames: usize) -> FrameAllocator {
        let storage = vec![0u64; FrameAllocator::words_for(frames)].leak();
        FrameAllocator::new(storage, frames)
    }

    fn frame(n: u64) -> PhysAddr {
        PhysAddr::new(n * PAGE_SIZE)
    }

    #[test]
    fn everything_starts_in_use() {
        let mut a = allocator(100);
        assert_eq!(a.free_frames(), 0);
        assert_eq!(a.allocate(), None);
    }

    #[test]
    fn hands_out_the_lowest_free_frame_first() {
        let mut a = allocator(100);
        a.free_range(frame(10), frame(20));
        assert_eq!(a.free_frames(), 10);
        assert_eq!(a.allocate(), Some(frame(10)));
        assert_eq!(a.allocate(), Some(frame(11)));

        a.free(frame(10));
        assert_eq!(a.allocate(), Some(frame(10)), "a freed low frame is reused first");
    }

    #[test]
    fn free_range_rounds_inward_reserve_rounds_outward() {
        let mut a = allocator(100);
        // 0x1800..0x5800 wholly contains frames 2, 3 and 4 only.
        a.free_range(PhysAddr::new(0x1800), PhysAddr::new(0x5800));
        assert_eq!(a.free_frames(), 3);

        // One byte inside frame 3 reserves all of it.
        a.reserve_range(PhysAddr::new(0x3800), PhysAddr::new(0x3801));
        assert_eq!(a.free_frames(), 2);
        assert_eq!(a.allocate(), Some(frame(2)));
        assert_eq!(a.allocate(), Some(frame(4)));
        assert_eq!(a.allocate(), None);
    }

    #[test]
    fn never_returns_a_frame_past_the_end() {
        // 70 frames: the second bitmap word has 58 bits past the end.
        let mut a = allocator(70);
        a.free_range(frame(0), frame(1000));
        assert_eq!(a.free_frames(), 70);
        for n in 0..70 {
            assert_eq!(a.allocate(), Some(frame(n)));
        }
        assert_eq!(a.allocate(), None);
    }

    #[test]
    fn the_highest_free_frame_can_be_asked_for() {
        // 100 frames: the bitmap's second word has 28 bits past the end.
        let mut frames = allocator(100);
        assert_eq!(frames.allocate_highest(), None);
        frames.free_range(frame(3), frame(100));
        frames.reserve_range(frame(90), frame(100));
        assert_eq!(frames.allocate_highest(), Some(frame(89)));
        assert_eq!(frames.allocate_highest(), Some(frame(88)));
        assert_eq!(frames.free_frames(), 85);
        // The lowest is still handed out first, and a frame taken from
        // the top comes back like any other.
        assert_eq!(frames.allocate(), Some(frame(3)));
        frames.free(frame(89));
        assert_eq!(frames.allocate_highest(), Some(frame(89)));
        // Down across a word boundary, until nothing is left.
        frames.reserve_range(frame(4), frame(63));
        frames.reserve_range(frame(65), frame(88));
        assert_eq!(frames.allocate_highest(), Some(frame(64)));
        assert_eq!(frames.allocate_highest(), Some(frame(63)));
        assert_eq!(frames.allocate_highest(), None);
        assert_eq!(frames.free_frames(), 0);
    }

    #[test]
    fn a_frame_can_be_asked_about() {
        let mut frames = allocator(100);
        frames.free_range(frame(10), frame(20));
        assert!(!frames.is_free(frame(9)));
        assert!(frames.is_free(frame(10)));
        // Any address inside the frame is an answer about the frame.
        assert!(frames.is_free(PhysAddr::new(19 * PAGE_SIZE + 0xFFF)));
        assert!(!frames.is_free(frame(20)));
        assert_eq!(frames.allocate(), Some(frame(10)));
        assert!(!frames.is_free(frame(10)));
        // Past the end, and in the bitmap's unused last bits.
        assert!(!frames.is_free(frame(100)));
        assert!(!frames.is_free(frame(1 << 30)));
    }

    #[test]
    fn freeing_everything_restores_the_count() {
        let mut a = allocator(256);
        a.free_range(frame(0), frame(256));
        let taken: Vec<_> = (0..200).map(|_| a.allocate().unwrap()).collect();
        assert_eq!(a.free_frames(), 56);
        for f in taken.into_iter().rev() {
            a.free(f);
        }
        assert_eq!(a.free_frames(), 256);
    }

    #[test]
    fn contiguous_runs() {
        let mut a = allocator(100);
        a.free_range(frame(0), frame(100));
        a.reserve_range(frame(3), frame(4)); // a hole at 3
        a.reserve_range(frame(10), frame(11)); // and at 10
        assert_eq!(a.allocate_contiguous(4), Some(frame(4)), "0-2 too short, 4-7 fits");
        assert_eq!(a.allocate_contiguous(3), Some(frame(0)));
        assert_eq!(a.allocate_contiguous(5), Some(frame(11)), "8-9 too short");
        assert_eq!(a.allocate(), Some(frame(8)), "single frames still fill the gaps");
        assert_eq!(a.allocate_contiguous(90), None);
        assert_eq!(a.allocate_contiguous(0), None);
        assert_eq!(a.free_frames(), 100 - 2 - 4 - 3 - 5 - 1);
        for n in 4..8 {
            a.free(frame(n));
        }
        assert_eq!(a.allocate_contiguous(4), Some(frame(4)), "freed and found again");
    }

    #[test]
    #[should_panic(expected = "double free")]
    fn double_free_is_caught() {
        let mut a = allocator(10);
        a.free_range(frame(0), frame(10));
        let f = a.allocate().unwrap();
        a.free(f);
        a.free(f);
    }

    #[test]
    #[should_panic(expected = "unaligned")]
    fn unaligned_free_is_caught() {
        let mut a = allocator(10);
        a.free(PhysAddr::new(0x1234));
    }
}
