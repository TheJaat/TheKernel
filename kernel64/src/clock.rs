//! Time, kept by the CPU's counter.
//!
//! Every CPU TheTaaJ runs on has a counter that goes up at a fixed
//! rate whatever the CPU is doing: x86's time-stamp counter (at
//! `platform::tsc_hz`), AArch64's generic timer (at CNTFRQ). Before
//! this module the kernel's time was the number of timer interrupts
//! it had seen; but an interrupt can come late, and two that come late
//! enough arrive as one - in a virtual machine whose host is busy, they
//! do - and every one missed made the system's second longer. The
//! counter does not miss: "now" is read from it, and the tick count
//! (`interrupts::ticks`, what sleepers wait for) is worked out from it
//! at each interrupt, so a late interrupt catches up instead of
//! falling behind.
//!
//! The time of day is the real-time clock's reading at boot (rtc.rs),
//! plus the counter's time since then.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch;

const NANOS: u64 = 1_000_000_000;

/// The counter when time started: when `start` was called.
static START: AtomicU64 = AtomicU64::new(0);
/// The counter's rate, 0 until `start`.
static HZ: AtomicU64 = AtomicU64::new(0);
/// The time of day at the start, in nanoseconds since 1970 - from the
/// real-time clock, or 0 if there was none, which makes the "date"
/// the time since boot in January 1970, as it was before.
static EPOCH: AtomicU64 = AtomicU64::new(0);

/// Starts the clock: now is 0 since boot, and `rtc` - seconds since
/// 1970, if the machine's real-time clock said - is the time of day.
/// Called once on the boot CPU, just before its timer interrupts
/// start: the ticks count from here.
pub fn start(rtc: Option<u64>) {
    START.store(arch::clock(), Ordering::Relaxed);
    if let Some(seconds) = rtc {
        EPOCH.store(seconds * NANOS, Ordering::Relaxed);
    }
    HZ.store(arch::clock_hz(), Ordering::Release);
}

pub fn started() -> bool {
    HZ.load(Ordering::Acquire) != 0
}

/// Nanoseconds since `start`; 0 before it.
pub fn monotonic() -> u64 {
    let hz = HZ.load(Ordering::Acquire);
    if hz == 0 {
        return 0;
    }
    let counted = arch::clock().wrapping_sub(START.load(Ordering::Relaxed));
    // In 128 bits: counted * 10^9 overflows 64 after 18 seconds of a
    // 1 GHz counter.
    (u128::from(counted) * u128::from(NANOS) / u128::from(hz)) as u64
}

/// The time of day, in nanoseconds since 1 January 1970 UTC.
pub fn realtime() -> u64 {
    EPOCH.load(Ordering::Relaxed) + monotonic()
}

/// The ticks (interrupts::HZ a second) the counter has counted since
/// `start`: what the tick count should be now.
pub fn ticks() -> u64 {
    monotonic() / (NANOS / crate::interrupts::HZ)
}

/// The CLOCK system call: nanoseconds on clock `which` (abi::clock).
pub fn read(which: u64) -> Result<u64, abi::Error> {
    match which {
        abi::clock::MONOTONIC => Ok(monotonic()),
        abi::clock::REALTIME => Ok(realtime()),
        abi::clock::PROCESS_CPU => crate::threads::process_cpu_ns().ok_or(abi::Error::InvalidArgument),
        _ => Err(abi::Error::InvalidArgument),
    }
}
