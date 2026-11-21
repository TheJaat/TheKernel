//! The CPU's own timer.
//!
//! Every AArch64 CPU has a counter that runs at a fixed rate
//! (`CNTFRQ_EL0` says which) and never stops, and timers that compare
//! it with a value: when the counter reaches the value, the timer
//! raises an interrupt - a private one, each CPU having its own.
//!
//! There is nothing to find and nothing to calibrate, which on a PC
//! took several devices (entry 6: the local APIC timer to interrupt,
//! and the PIT to measure it against).
//!
//! This kernel uses the *virtual* timer. A CPU has a "physical" one
//! too; they differ only under a hypervisor, which can shift the
//! virtual one, and the virtual one is what a guest is always allowed.

use core::arch::asm;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use kernel64::gic::{milliseconds, next_deadline, tick_interval};

use super::gic;

/// Timer interrupts per second - the same as on the PC.
pub const HZ: u64 = 100;

static TICKS: AtomicU64 = AtomicU64::new(0);
static INTERVAL: AtomicU64 = AtomicU64::new(0);
static FREQUENCY: AtomicU64 = AtomicU64::new(0);
/// The timer's interrupt number; 0 until `init`.
static INTERRUPT: AtomicU32 = AtomicU32::new(0);

/// The counter, now.
pub fn counter() -> u64 {
    let value: u64;
    // SAFETY: reading the counter changes nothing. `isb` first, or the
    // CPU may read it earlier than the instructions before it.
    unsafe { asm!("isb", "mrs {}, cntvct_el0", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

/// How many counter steps make a second.
pub fn frequency() -> u64 {
    FREQUENCY.load(Ordering::Relaxed)
}

/// Milliseconds since `earlier`, a value of `counter()`.
pub fn milliseconds_since(earlier: u64) -> u64 {
    milliseconds(counter().wrapping_sub(earlier), frequency())
}

/// The tick counter itself, for code that watches it change.
pub fn tick_counter() -> &'static AtomicU64 {
    &TICKS
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

pub fn interrupt() -> u32 {
    INTERRUPT.load(Ordering::Relaxed)
}

/// Starts the timer interrupting `HZ` times a second, on interrupt
/// number `interrupt`. Returns the counter's frequency.
///
/// # Safety
/// Once, on the boot CPU, after `gic::init`, with interrupts masked.
pub unsafe fn init(interrupt: u32) -> u64 {
    let frequency: u64;
    // SAFETY: reading a register that firmware set and nothing changes.
    unsafe { asm!("mrs {}, cntfrq_el0", out(reg) frequency, options(nomem, nostack, preserves_flags)) };
    let interval = tick_interval(frequency, HZ)
        .unwrap_or_else(|| panic!("the CPU's counter says it runs at {frequency} Hz: cannot make {HZ} ticks a second of that"));
    FREQUENCY.store(frequency, Ordering::Relaxed);
    INTERVAL.store(interval, Ordering::Relaxed);
    INTERRUPT.store(interrupt, Ordering::Relaxed);

    let first = counter() + interval;
    // SAFETY: the compare value, then control: bit 0 enables the
    // timer, bit 1 (left clear) would mask its interrupt.
    unsafe {
        asm!(
            "msr cntv_cval_el0, {first}",
            "mov {t}, #1",
            "msr cntv_ctl_el0, {t}",
            "isb",
            first = in(reg) first,
            t = out(reg) _,
            options(nomem, nostack, preserves_flags),
        );
    }
    gic::enable(interrupt);
    frequency
}

/// The timer's interrupt arrived. The timer goes on asking for as long
/// as the counter is past the compare value, so the first thing is to
/// move the value on.
pub fn on_interrupt() {
    let deadline: u64;
    // SAFETY: reading the compare value changes nothing.
    unsafe { asm!("mrs {}, cntv_cval_el0", out(reg) deadline, options(nomem, nostack, preserves_flags)) };
    let (next, passed) = next_deadline(deadline, counter(), INTERVAL.load(Ordering::Relaxed));
    // SAFETY: a compare value in the future: the timer stops asking.
    unsafe { asm!("msr cntv_cval_el0, {}", "isb", in(reg) next, options(nomem, nostack, preserves_flags)) };
    TICKS.fetch_add(passed, Ordering::Relaxed);
}
