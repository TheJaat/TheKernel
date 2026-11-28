//! Tracing (TheTaaJ K139): what the kernel does, as it does it - system
//! calls, messages sent and received, threads switched, faults,
//! interrupts - each an event in a ring of its CPU's, in memory the
//! kernel hands out as the handle for tracing. Whoever holds it can
//! switch tracing on and off (a word in the header) and read the
//! events; nobody else can, and while it is off each trace point costs
//! the kernel one load.
//!
//! The memory: a header of `HEADER` bytes, then a block for each CPU.
//!
//! | Offset | Size | What |
//! |---|---|---|
//! | 0 | 8 | `MAGIC` |
//! | 8 | 8 | how many CPUs (blocks) |
//! | 16 | 8 | events in each CPU's ring |
//! | 24 | 8 | on: not 0 while tracing (the holder's to write) |
//!
//! A CPU's block: its `written` count (8 bytes: events ever written,
//! each CPU's own), padding to `BLOCK_HEADER`, then its ring of events,
//! `EVENT` bytes each:
//!
//! | Offset | Size | What |
//! |---|---|---|
//! | 0 | 8 | sequence: the event's number + 1, written last |
//! | 8 | 8 | when: nanoseconds since the kernel's clock started |
//! | 16 | 2 | kind (`kind`) |
//! | 18 | 2 | the CPU |
//! | 20 | 4 | the process (0: the kernel's own threads) |
//! | 24 | 8 | a value, by kind |
//!
//! Each CPU writes only its own ring, so CPUs never wait for one
//! another. On one CPU an interrupt may come in the middle of an event:
//! the kernel takes each event's number from a counter of its own
//! (atomically), so the two take different slots; and a slot's sequence
//! is written last, so a reader that finds the sequence it expected -
//! before and after copying - has a whole event, and one that finds a
//! later one knows the ring came round.

use core::sync::atomic::{AtomicU64, Ordering};

/// "TAAJTRC1", little-endian.
pub const MAGIC: u64 = u64::from_le_bytes(*b"TAAJTRC1");
pub const HEADER: usize = 64;
pub const BLOCK_HEADER: usize = 64;
pub const EVENT: usize = 32;

/// What an event is, and what its value means.
pub mod kind {
    /// A system call began: its number.
    pub const SYSCALL: u16 = 1;
    /// A message was sent on a channel: its length in bytes.
    pub const SEND: u16 = 2;
    /// A message was received: its length in bytes.
    pub const RECEIVE: u16 = 3;
    /// The CPU switched threads: the process of the thread it switched
    /// to (0: a thread of the kernel's). The event's own process is the
    /// leaving thread's.
    pub const SWITCH: u16 = 4;
    /// A program's page fault: the address.
    pub const FAULT: u16 = 5;
    /// An interrupt: its number (the vector on a PC, the GIC's id on Arm).
    pub const INTERRUPT: u16 = 6;
    /// Where the CPU was when the timer interrupted it (K140, the
    /// profiler): the address - the kernel's (high) or the event's
    /// process's (low). On AArch64, at every interrupt (mostly the
    /// timer's).
    pub const SAMPLE: u16 = 7;

    /// Its name, for showing.
    pub fn name(kind: u16) -> &'static str {
        match kind {
            SYSCALL => "syscall",
            SEND => "send",
            RECEIVE => "receive",
            SWITCH => "switch",
            FAULT => "fault",
            INTERRUPT => "interrupt",
            SAMPLE => "sample",
            _ => "?",
        }
    }
}

/// One event, as read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    /// Its number on its CPU.
    pub number: u64,
    pub time: u64,
    pub kind: u16,
    pub cpu: u16,
    pub pid: u32,
    pub value: u64,
}

/// The memory, through a raw pointer: it is shared between address
/// spaces.
#[derive(Clone, Copy)]
pub struct Trace {
    base: *mut u8,
    cpus: usize,
    slots: u64,
}

// SAFETY: the memory is shared on purpose; every access is atomic or
// volatile, and a reader checks each event's sequence (see the module).
unsafe impl Send for Trace {}
// SAFETY: as for Send.
unsafe impl Sync for Trace {}

/// The bytes the memory needs for `cpus` rings of `slots` events.
pub const fn size(cpus: usize, slots: usize) -> usize {
    HEADER + cpus * (BLOCK_HEADER + slots * EVENT)
}

impl Trace {
    /// Makes the rings in `bytes` bytes at `base`: for the kernel, once.
    /// None if `cpus` rings of at least 16 events do not fit.
    ///
    /// # Safety
    /// `base` is valid for reads and writes of `bytes` bytes, 8-aligned,
    /// for as long as the trace is used.
    pub unsafe fn create(base: *mut u8, bytes: usize, cpus: usize) -> Option<Trace> {
        if cpus == 0 || bytes < HEADER {
            return None;
        }
        let slots = ((bytes - HEADER) / cpus).checked_sub(BLOCK_HEADER)? / EVENT;
        if slots < 16 {
            return None;
        }
        let trace = Trace { base, cpus, slots: slots as u64 };
        trace.word(8).store(cpus as u64, Ordering::Relaxed);
        trace.word(16).store(slots as u64, Ordering::Relaxed);
        trace.word(24).store(0, Ordering::Relaxed);
        for cpu in 0..cpus {
            trace.word(trace.block(cpu)).store(0, Ordering::Relaxed);
        }
        trace.word(0).store(MAGIC, Ordering::Release);
        Some(trace)
    }

    /// The rings the kernel made in `bytes` bytes at `base`: for the
    /// holder. None if they are not.
    ///
    /// # Safety
    /// As for `create`.
    pub unsafe fn open(base: *mut u8, bytes: usize) -> Option<Trace> {
        let probe = Trace { base, cpus: 0, slots: 0 };
        if bytes < HEADER || probe.word(0).load(Ordering::Acquire) != MAGIC {
            return None;
        }
        let cpus = usize::try_from(probe.word(8).load(Ordering::Relaxed)).ok()?;
        let slots = usize::try_from(probe.word(16).load(Ordering::Relaxed)).ok()?;
        (cpus > 0 && slots >= 16 && cpus <= 1024 && size(cpus, slots) <= bytes).then_some(Trace { base, cpus, slots: slots as u64 })
    }

    fn word(&self, at: usize) -> &AtomicU64 {
        // SAFETY: inside the memory, 8-aligned (`base` is, and every
        // offset used is a multiple of 8); valid while the trace is.
        unsafe { AtomicU64::from_ptr(self.base.add(at).cast::<u64>()) }
    }

    fn block(&self, cpu: usize) -> usize {
        HEADER + cpu * (BLOCK_HEADER + self.slots as usize * EVENT)
    }

    fn slot(&self, cpu: usize, number: u64) -> usize {
        self.block(cpu) + BLOCK_HEADER + (number % self.slots) as usize * EVENT
    }

    pub fn cpus(&self) -> usize {
        self.cpus
    }

    pub fn slots(&self) -> u64 {
        self.slots
    }

    /// Whether tracing is on.
    pub fn on(&self) -> bool {
        self.word(24).load(Ordering::Relaxed) != 0
    }

    /// Switches tracing on or off: the holder's.
    pub fn set_on(&self, on: bool) {
        self.word(24).store(u64::from(on), Ordering::Release);
    }

    /// Events ever written on `cpu`.
    pub fn written(&self, cpu: usize) -> u64 {
        self.word(self.block(cpu)).load(Ordering::Acquire)
    }

    /// Writes event `number` of `cpu` (the kernel's: each number once,
    /// from its own counter). Does nothing for a CPU past the last.
    pub fn put(&self, cpu: usize, number: u64, time: u64, kind: u16, pid: u32, value: u64) {
        if cpu >= self.cpus {
            return;
        }
        let at = self.slot(cpu, number);
        let sequence = self.word(at);
        sequence.store(0, Ordering::Relaxed);
        core::sync::atomic::fence(Ordering::Release);
        self.word(at + 8).store(time, Ordering::Relaxed);
        self.word(at + 16).store(u64::from(kind) | u64::from(cpu as u16) << 16 | u64::from(pid) << 32, Ordering::Relaxed);
        self.word(at + 24).store(value, Ordering::Relaxed);
        sequence.store(number + 1, Ordering::Release);
        self.word(self.block(cpu)).fetch_max(number + 1, Ordering::Release);
    }

    /// Event `number` of `cpu`: Ok if it is there and whole; Err(true)
    /// if the ring came round past it, Err(false) if it is not written
    /// yet (or is being written).
    pub fn get(&self, cpu: usize, number: u64) -> Result<Event, bool> {
        let at = self.slot(cpu, number);
        let sequence = self.word(at).load(Ordering::Acquire);
        if sequence != number + 1 {
            return Err(sequence > number + 1);
        }
        let time = self.word(at + 8).load(Ordering::Relaxed);
        let packed = self.word(at + 16).load(Ordering::Relaxed);
        let value = self.word(at + 24).load(Ordering::Relaxed);
        core::sync::atomic::fence(Ordering::Acquire);
        // Written over meanwhile?
        let again = self.word(at).load(Ordering::Relaxed);
        if again != sequence {
            return Err(again > number + 1);
        }
        Ok(Event { number, time, kind: packed as u16, cpu: (packed >> 16) as u16, pid: (packed >> 32) as u32, value })
    }

    /// The events since `from[cpu]` (each CPU's `written` when the span
    /// began, or where the last read stopped) up to `written` now, CPU by
    /// CPU, oldest first, each to `each`; `from` is moved on to where
    /// this read stopped, for the next. Returns how many were lost: the
    /// ring came round before they were read, or they were being
    /// written over. Read often enough, a span can be any length.
    pub fn read(&self, from: &mut [u64], mut each: impl FnMut(Event)) -> u64 {
        let mut lost = 0;
        for (cpu, place) in from.iter_mut().enumerate().take(self.cpus) {
            let start = *place;
            let end = self.written(cpu);
            // Only the last `slots` can still be there.
            let first = start.max(end.saturating_sub(self.slots));
            lost += first - start;
            for number in first..end {
                match self.get(cpu, number) {
                    Ok(event) => each(event),
                    Err(_) => lost += 1,
                }
            }
            *place = end.max(start);
        }
        lost
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec;
    use std::vec::Vec;

    fn memory(cpus: usize, slots: usize) -> Vec<u64> {
        vec![0u64; size(cpus, slots) / 8]
    }

    #[test]
    fn events_come_back_as_written_cpu_by_cpu() {
        let mut m = memory(2, 16);
        let kernel = unsafe { Trace::create(m.as_mut_ptr().cast(), m.len() * 8, 2) }.unwrap();
        assert_eq!((kernel.cpus(), kernel.slots()), (2, 16));
        let holder = unsafe { Trace::open(m.as_mut_ptr().cast(), m.len() * 8) }.unwrap();
        assert!(!kernel.on());
        holder.set_on(true);
        assert!(kernel.on());
        let mut from = [holder.written(0), holder.written(1)];
        kernel.put(0, 0, 100, kind::SYSCALL, 7, 3);
        kernel.put(1, 0, 101, kind::INTERRUPT, 0, 32);
        kernel.put(0, 1, 102, kind::SEND, 7, 40);
        kernel.put(9, 0, 1, kind::FAULT, 1, 1); // no such CPU: nothing
        let mut got = Vec::new();
        assert_eq!(holder.read(&mut from, |e| got.push(e)), 0);
        assert_eq!(from, [2, 1]);
        assert_eq!(
            got,
            [
                Event { number: 0, time: 100, kind: kind::SYSCALL, cpu: 0, pid: 7, value: 3 },
                Event { number: 1, time: 102, kind: kind::SEND, cpu: 0, pid: 7, value: 40 },
                Event { number: 0, time: 101, kind: kind::INTERRUPT, cpu: 1, pid: 0, value: 32 },
            ]
        );
        // Read again: only what came since.
        kernel.put(0, 2, 103, kind::RECEIVE, 7, 40);
        let mut again = Vec::new();
        assert_eq!(holder.read(&mut from, |e| again.push(e.number)), 0);
        assert_eq!((again, from), (std::vec![2], [3, 1]));
        holder.set_on(false);
        assert!(!kernel.on());
    }

    #[test]
    fn a_ring_that_came_round_says_how_much_was_lost() {
        let mut m = memory(1, 16);
        let kernel = unsafe { Trace::create(m.as_mut_ptr().cast(), m.len() * 8, 1) }.unwrap();
        for n in 0..40 {
            kernel.put(0, n, n, kind::SWITCH, 1, n);
        }
        let mut got = Vec::new();
        assert_eq!(kernel.read(&mut [0], |e| got.push(e.number)), 24);
        assert_eq!(got, (24..40).collect::<Vec<_>>());
        // An event not written yet, and one written over.
        assert_eq!(kernel.get(0, 40), Err(false));
        assert_eq!(kernel.get(0, 3), Err(true));
    }

    #[test]
    fn numbers_taken_out_of_order_on_one_cpu_both_arrive() {
        // An interrupt in the middle of an event: number 1 is written
        // before number 0 is finished.
        let mut m = memory(1, 16);
        let kernel = unsafe { Trace::create(m.as_mut_ptr().cast(), m.len() * 8, 1) }.unwrap();
        kernel.put(0, 1, 20, kind::INTERRUPT, 0, 32);
        kernel.put(0, 0, 10, kind::SYSCALL, 3, 1);
        let mut got = Vec::new();
        assert_eq!(kernel.read(&mut [0], |e| got.push(e.kind)), 0);
        assert_eq!(got, [kind::SYSCALL, kind::INTERRUPT]);
        assert_eq!(kernel.written(0), 2);
    }

    #[test]
    fn what_is_not_a_trace_is_not_opened_and_too_little_is_not_made() {
        let mut m = memory(1, 16);
        assert!(unsafe { Trace::open(m.as_mut_ptr().cast(), m.len() * 8) }.is_none());
        assert!(unsafe { Trace::create(m.as_mut_ptr().cast(), m.len() * 8, 0) }.is_none());
        assert!(unsafe { Trace::create(m.as_mut_ptr().cast(), HEADER + 100, 1) }.is_none());
        assert!(unsafe { Trace::create(m.as_mut_ptr().cast(), 8, 1) }.is_none());
        let _ = unsafe { Trace::create(m.as_mut_ptr().cast(), m.len() * 8, 1) }.unwrap();
        // Says it is bigger than the memory it is said to be in.
        assert!(unsafe { Trace::open(m.as_mut_ptr().cast(), HEADER + 64) }.is_none());
        for k in 0..8 {
            assert!(!kind::name(k).is_empty());
        }
    }
}
