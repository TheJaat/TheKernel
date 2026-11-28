//! The system log's ring (TheTaaJ K136): what the kernel writes to the
//! debug log, kept in memory the kernel hands to init for the log
//! service (`logd`), which writes it to a file.
//!
//! The serial line stays as it is. This is a second copy of the same
//! text, in pieces as the writers give them - a program's `argc `, then
//! `1`, then the newline - each piece a record that says who wrote it
//! and when. The log service puts each writer's pieces back into lines.
//!
//! One writer, the kernel, under its own lock; one reader, the log
//! service, in another address space. Neither waits for the other: the
//! ring has a fixed size, and when the writer comes round to records
//! the reader has not read yet, it writes over them. The reader finds
//! out, and knows how many bytes it lost - a log that says something is
//! missing, rather than one that quietly pretends to be complete.
//!
//! The memory: a header of `HEADER` bytes, then the ring.
//!
//! | Offset | Size | What |
//! |---|---|---|
//! | 0 | 8 | `MAGIC` |
//! | 8 | 8 | the ring's size in bytes |
//! | 16 | 8 | `written`: bytes ever written, the next record's place |
//! | 24 | 8 | `first`: where the oldest whole record still in the ring starts |
//!
//! Places are counts of bytes since the start, never wrapped: the ring
//! byte for place `p` is `p % size`. A record is `RECORD_HEADER` bytes -
//! its length (u16), its kind (u8), the length of the name (u8), the
//! process id (u32), the time (u64, nanoseconds since 1970, 0 if the
//! clock was not running yet) - then the name, then the text.
//!
//! The writer moves `first` past the records it is about to write over
//! *before* writing, and `written` on *after*. A reader that copied a
//! record and then finds `first` past the record's start knows it may
//! have copied it half overwritten, and drops it.

use core::sync::atomic::{AtomicU64, Ordering};

/// "TAAJLOG1", little-endian.
pub const MAGIC: u64 = u64::from_le_bytes(*b"TAAJLOG1");
/// The header's size in bytes; the ring follows.
pub const HEADER: usize = 64;
/// A record's own bytes, before the name and the text.
pub const RECORD_HEADER: usize = 16;
/// The longest text in one record: a longer piece is cut into several.
pub const TEXT_MAX: usize = 1024;
/// The longest name kept with a record.
pub const NAME_MAX: usize = 32;
/// The longest record.
pub const RECORD_MAX: usize = RECORD_HEADER + NAME_MAX + TEXT_MAX;

/// What a record is.
pub mod kind {
    /// What the kernel wrote.
    pub const KERNEL: u8 = 0;
    /// What a program wrote: its process id; the name if known.
    pub const PROGRAM: u8 = 1;
    /// A program started: its process id and its name, no text - so
    /// the log service can name the program in its later records.
    pub const STARTED: u8 = 2;
    /// The boot before this one stopped, and these were its last words
    /// (K188): the crash record the kernel found at start. The time is
    /// when that kernel began to stop.
    pub const CRASH: u8 = 3;
}

const SIZE_AT: usize = 8;
const WRITTEN_AT: usize = 16;
const FIRST_AT: usize = 24;

/// The memory: its header and its ring. Shared between address spaces,
/// so every access is through a raw pointer - the counters atomically.
#[derive(Clone, Copy)]
pub struct Ring {
    base: *mut u8,
    size: u64,
}

// SAFETY: the memory is shared between address spaces on purpose: the
// counters are atomics, the ring's bytes are read and written volatile,
// and a reader checks `first` after copying (a record written over meanwhile is
// dropped). One writer at a time is the caller's part (the kernel's lock).
unsafe impl Send for Ring {}
// SAFETY: as for Send - every access through `&Ring` is atomic or
// volatile, and torn records are found by the reader.
unsafe impl Sync for Ring {}

impl Ring {
    /// The ring in `bytes` bytes at `base`, made empty: for the writer,
    /// once. None if there is no room for a ring of at least four of the
    /// longest records.
    ///
    /// # Safety
    /// `base` is valid for reads and writes of `bytes` bytes, aligned to
    /// 8, for as long as the ring is used.
    pub unsafe fn create(base: *mut u8, bytes: usize) -> Option<Ring> {
        let size = bytes.checked_sub(HEADER).filter(|&size| size >= 4 * RECORD_MAX)? as u64;
        let ring = Ring { base, size };
        ring.counter(SIZE_AT).store(size, Ordering::Relaxed);
        ring.counter(WRITTEN_AT).store(0, Ordering::Relaxed);
        ring.counter(FIRST_AT).store(0, Ordering::Relaxed);
        ring.counter(0).store(MAGIC, Ordering::Release);
        Some(ring)
    }

    /// The ring a writer made in `bytes` bytes at `base`: for the
    /// reader. None if it is not one (the magic, or a size that does
    /// not fit).
    ///
    /// # Safety
    /// As for `create`.
    pub unsafe fn open(base: *mut u8, bytes: usize) -> Option<Ring> {
        let probe = Ring { base, size: 0 };
        if bytes < HEADER || probe.counter(0).load(Ordering::Acquire) != MAGIC {
            return None;
        }
        let size = probe.counter(SIZE_AT).load(Ordering::Relaxed);
        (size >= 4 * RECORD_MAX as u64 && size <= (bytes - HEADER) as u64).then_some(Ring { base, size })
    }

    fn counter(&self, at: usize) -> &AtomicU64 {
        // SAFETY: inside the header, 8-aligned (`base` is), valid while
        // the ring is (the constructors' promise).
        unsafe { AtomicU64::from_ptr(self.base.add(at).cast::<u64>()) }
    }

    /// Bytes ever written: where the next record goes.
    pub fn written(&self) -> u64 {
        self.counter(WRITTEN_AT).load(Ordering::Acquire)
    }

    /// Where the oldest whole record still in the ring starts.
    pub fn first(&self) -> u64 {
        self.counter(FIRST_AT).load(Ordering::Acquire)
    }

    fn put_bytes(&self, place: u64, bytes: &[u8]) {
        for (n, &byte) in bytes.iter().enumerate() {
            let at = HEADER + ((place + n as u64) % self.size) as usize;
            // SAFETY: inside the ring.
            unsafe { self.base.add(at).write_volatile(byte) };
        }
    }

    fn get_bytes(&self, place: u64, out: &mut [u8]) {
        for (n, byte) in out.iter_mut().enumerate() {
            let at = HEADER + ((place + n as u64) % self.size) as usize;
            // SAFETY: inside the ring.
            *byte = unsafe { self.base.add(at).read_volatile() };
        }
    }

    fn length_at(&self, place: u64) -> u64 {
        let mut length = [0u8; 2];
        self.get_bytes(place, &mut length);
        u64::from(u16::from_le_bytes(length))
    }

    /// Adds a record - several, if `text` is longer than `TEXT_MAX`. A
    /// name is cut to `NAME_MAX` bytes. The writer's: one at a time.
    pub fn put(&self, kind: u8, pid: u32, time: u64, name: &[u8], text: &[u8]) {
        let name = &name[..name.len().min(NAME_MAX)];
        let mut pieces = text.chunks(TEXT_MAX);
        let mut piece = pieces.next().unwrap_or(&[]);
        loop {
            self.put_one(kind, pid, time, name, piece);
            match pieces.next() {
                Some(next) => piece = next,
                None => break,
            }
        }
    }

    fn put_one(&self, kind: u8, pid: u32, time: u64, name: &[u8], text: &[u8]) {
        let length = (RECORD_HEADER + name.len() + text.len()) as u64;
        let written = self.counter(WRITTEN_AT).load(Ordering::Relaxed);
        // Past what this record will write over, first.
        let mut first = self.counter(FIRST_AT).load(Ordering::Relaxed);
        while written + length - first > self.size {
            first += self.length_at(first);
        }
        self.counter(FIRST_AT).store(first, Ordering::Release);
        let mut header = [0u8; RECORD_HEADER];
        header[0..2].copy_from_slice(&(length as u16).to_le_bytes());
        header[2] = kind;
        header[3] = name.len() as u8;
        header[4..8].copy_from_slice(&pid.to_le_bytes());
        header[8..16].copy_from_slice(&time.to_le_bytes());
        self.put_bytes(written, &header);
        self.put_bytes(written + RECORD_HEADER as u64, name);
        self.put_bytes(written + (RECORD_HEADER + name.len()) as u64, text);
        self.counter(WRITTEN_AT).store(written + length, Ordering::Release);
    }
}

/// One record, as the reader copied it.
#[derive(Clone, Copy)]
pub struct Record<'a> {
    pub kind: u8,
    pub pid: u32,
    /// Nanoseconds since 1970; 0 if the kernel's clock was not running.
    pub time: u64,
    pub name: &'a [u8],
    pub text: &'a [u8],
}

/// What the reader found next.
pub enum Next<'a> {
    Record(Record<'a>),
    /// The writer came round and wrote over this many bytes before
    /// they were read.
    Lost(u64),
    /// Nothing new.
    Empty,
}

/// The reader: where it is in the ring, and a record's copy.
pub struct Reader {
    ring: Ring,
    at: u64,
    copy: [u8; RECORD_MAX],
}

impl Reader {
    /// Reads from the oldest record still in the ring.
    pub fn new(ring: Ring) -> Reader {
        Reader { at: ring.first(), ring, copy: [0; RECORD_MAX] }
    }

    /// The next record, or that some were lost, or that there is none.
    pub fn next(&mut self) -> Next<'_> {
        let first = self.ring.first();
        if self.at < first {
            let lost = first - self.at;
            self.at = first;
            return Next::Lost(lost);
        }
        if self.at >= self.ring.written() {
            return Next::Empty;
        }
        let start = self.at;
        let length = self.ring.length_at(start) as usize;
        let damaged = !(RECORD_HEADER..=RECORD_MAX).contains(&length);
        if !damaged {
            self.ring.get_bytes(start, &mut self.copy[..length]);
        }
        // Written over while it was being copied? Then the copy may be
        // half old, half new - and its length may have been too.
        let first = self.ring.first();
        if first > start {
            self.at = first;
            return Next::Lost(first - start);
        }
        if damaged {
            // No record can be that long: the ring is not what this
            // reader thinks. Everything up to now is given up for lost.
            let written = self.ring.written();
            self.at = written;
            return Next::Lost(written - start);
        }
        self.at = start + length as u64;
        let copy = &self.copy[..length];
        let name_length = usize::from(copy[3]).min(length - RECORD_HEADER);
        Next::Record(Record {
            kind: copy[2],
            pid: u32::from_le_bytes([copy[4], copy[5], copy[6], copy[7]]),
            time: u64::from_le_bytes([copy[8], copy[9], copy[10], copy[11], copy[12], copy[13], copy[14], copy[15]]),
            name: &copy[RECORD_HEADER..RECORD_HEADER + name_length],
            text: &copy[RECORD_HEADER + name_length..],
        })
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec;
    use std::vec::Vec;

    /// Memory for a ring of `ring` bytes, 8-aligned.
    fn memory(ring: usize) -> Vec<u64> {
        vec![0u64; (HEADER + ring).div_ceil(8)]
    }

    fn texts(reader: &mut Reader) -> Vec<Result<(u8, u32, u64, Vec<u8>, Vec<u8>), u64>> {
        let mut out = Vec::new();
        loop {
            match reader.next() {
                Next::Record(r) => out.push(Ok((r.kind, r.pid, r.time, r.name.to_vec(), r.text.to_vec()))),
                Next::Lost(n) => out.push(Err(n)),
                Next::Empty => return out,
            }
        }
    }

    #[test]
    fn records_are_read_back_as_they_were_written() {
        let mut m = memory(8192);
        let ring = unsafe { Ring::create(m.as_mut_ptr().cast(), m.len() * 8) }.unwrap();
        ring.put(kind::KERNEL, 0, 5, b"", b"[kernel] idle\n");
        ring.put(kind::STARTED, 7, 6, b"hello", b"");
        ring.put(kind::PROGRAM, 7, 0, b"", b"argc ");
        let reader_ring = unsafe { Ring::open(m.as_mut_ptr().cast(), m.len() * 8) }.unwrap();
        let mut reader = Reader::new(reader_ring);
        assert_eq!(
            texts(&mut reader),
            [
                Ok((kind::KERNEL, 0, 5, Vec::new(), b"[kernel] idle\n".to_vec())),
                Ok((kind::STARTED, 7, 6, b"hello".to_vec(), Vec::new())),
                Ok((kind::PROGRAM, 7, 0, Vec::new(), b"argc ".to_vec())),
            ]
        );
        // And what comes later, later.
        ring.put(kind::PROGRAM, 7, 0, b"", b"1\n");
        assert_eq!(texts(&mut reader), [Ok((kind::PROGRAM, 7, 0, Vec::new(), b"1\n".to_vec()))]);
    }

    #[test]
    fn what_is_not_a_ring_is_not_opened() {
        let mut m = memory(8192);
        assert!(unsafe { Ring::open(m.as_mut_ptr().cast(), m.len() * 8) }.is_none()); // no magic
        assert!(unsafe { Ring::open(m.as_mut_ptr().cast(), 8) }.is_none()); // no header
        let _ = unsafe { Ring::create(m.as_mut_ptr().cast(), m.len() * 8) }.unwrap();
        // A size bigger than the memory it is said to be in.
        assert!(unsafe { Ring::open(m.as_mut_ptr().cast(), HEADER + 4 * RECORD_MAX) }.is_none());
        // Too small to make.
        assert!(unsafe { Ring::create(m.as_mut_ptr().cast(), HEADER + RECORD_MAX) }.is_none());
    }

    #[test]
    fn a_long_piece_is_cut_and_a_long_name_shortened() {
        let mut m = memory(16384);
        let ring = unsafe { Ring::create(m.as_mut_ptr().cast(), m.len() * 8) }.unwrap();
        let text = vec![b'x'; TEXT_MAX * 2 + 10];
        ring.put(kind::PROGRAM, 3, 1, &[b'n'; 40], &text);
        let records = texts(&mut Reader::new(ring));
        let lengths: Vec<usize> = records.iter().map(|r| r.as_ref().unwrap().4.len()).collect();
        assert_eq!(lengths, [TEXT_MAX, TEXT_MAX, 10]);
        assert!(records.iter().all(|r| r.as_ref().unwrap().3.len() == NAME_MAX));
    }

    #[test]
    fn a_reader_left_behind_is_told_how_much_it_lost_and_goes_on() {
        let mut m = memory(4 * RECORD_MAX);
        let ring = unsafe { Ring::create(m.as_mut_ptr().cast(), m.len() * 8) }.unwrap();
        let mut reader = Reader::new(ring);
        // Far more than fits: the oldest are written over.
        for n in 0..1000u32 {
            ring.put(kind::PROGRAM, n, 0, b"", &[b'a'; 100]);
        }
        let records = texts(&mut reader);
        let Err(lost) = records[0] else { panic!("the loss is said first") };
        let read: Vec<u32> = records[1..].iter().map(|r| r.as_ref().unwrap().1).collect();
        // What is left is the newest, in order, whole, and nothing is
        // counted twice: lost and read add up to everything written.
        assert_eq!(*read.last().unwrap(), 999);
        assert!(read.windows(2).all(|w| w[1] == w[0] + 1));
        assert!(records[1..].iter().all(|r| r.as_ref().unwrap().4 == [b'a'; 100]));
        assert_eq!(lost + read.len() as u64 * (RECORD_HEADER as u64 + 100), ring.written());
        assert!(ring.written() - ring.first() <= ring.size);
    }

    #[test]
    fn a_reader_behind_first_or_at_a_damaged_record_skips_it() {
        let mut m = memory(4 * RECORD_MAX);
        let ring = unsafe { Ring::create(m.as_mut_ptr().cast(), m.len() * 8) }.unwrap();
        ring.put(kind::PROGRAM, 1, 0, b"", b"old");
        let mut reader = Reader::new(ring);
        // As if the writer had come round meanwhile: `first` moved past
        // the record the reader is on - lost, not a record.
        ring.counter(FIRST_AT).store(10, Ordering::Release);
        assert!(matches!(reader.next(), Next::Lost(10)));
        // A damaged length (a record that cannot be) is lost too.
        let mut m = memory(4 * RECORD_MAX);
        let ring = unsafe { Ring::create(m.as_mut_ptr().cast(), m.len() * 8) }.unwrap();
        ring.put_bytes(0, &[3, 0]);
        ring.counter(WRITTEN_AT).store(20, Ordering::Release);
        let mut reader = Reader::new(ring);
        assert!(matches!(reader.next(), Next::Lost(20)));
        assert!(matches!(reader.next(), Next::Empty));
    }
}
