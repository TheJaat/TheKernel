//! The PC's real-time clock, in the "CMOS" chip: the date, read once
//! at boot (rtc.rs turns it into seconds).
//!
//! Its registers are reached through two I/O ports: write a register's
//! number to 0x70, then read its value from 0x71. Once a second the
//! chip spends about two milliseconds moving its registers on, and a
//! read in that time can see half of the change - 23:59:59 becoming
//! 00:00:00 read as 00:59:59. Status A's top bit says an update is
//! coming; and to be sure, the registers are read until two reads in
//! a row agree.

use super::port::{inb, outb};
use kernel64::rtc::CmosRegisters;

const SELECT: u16 = 0x70;
const DATA: u16 = 0x71;

const STATUS_A: u8 = 0x0A;
const STATUS_B: u8 = 0x0B;
/// Status A: the chip is about to update its registers.
const UPDATING: u8 = 0x80;

fn read(register: u8) -> u8 {
    // SAFETY: the CMOS chip's ports, on every PC; selecting a register
    // and reading it changes nothing. Called at boot, before anything
    // else uses them, with interrupts off - nothing can select another
    // register in between.
    unsafe {
        outb(SELECT, register);
        inb(DATA)
    }
}

fn read_all() -> CmosRegisters {
    // Wait out an update (at most ~2 ms, once a second); give up after
    // a million reads, which only a machine with no chip at all would
    // reach - its port reads 0xFF.
    for _ in 0..1_000_000 {
        if read(STATUS_A) & UPDATING == 0 {
            break;
        }
    }
    CmosRegisters {
        second: read(0x00),
        minute: read(0x02),
        hour: read(0x04),
        day: read(0x07),
        month: read(0x08),
        year: read(0x09),
        status_b: read(STATUS_B),
    }
}

/// Seconds since 1970 the clock says, or None if what it says is not a
/// date.
pub fn seconds() -> Option<u64> {
    let mut last = read_all();
    for _ in 0..10 {
        let now = read_all();
        if now == last {
            return now.date().unix_seconds();
        }
        last = now;
    }
    None
}
