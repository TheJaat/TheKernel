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

use kernel64::gic::{after_interrupt, milliseconds, tick_interval};

use kernel64::smp::MAX_CPUS;

use super::gic;

use crate::interrupts::HZ;

/// How many times the timer has interrupted. Not the tick count
/// (`interrupts::ticks`): one interrupt that comes late counts for
/// every tick that passed.
static TAKEN: AtomicU64 = AtomicU64::new(0);
static INTERVAL: AtomicU64 = AtomicU64::new(0);
static FREQUENCY: AtomicU64 = AtomicU64::new(0);
/// Per CPU: when its next tick is due, and what its compare value is
/// now - that tick, or a sleeper's time before it (K24, journal entry
/// 128). Only that CPU writes them, with interrupts masked.
static NEXT_TICK: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static ARMED: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
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

/// The count of timer interrupts itself, for code that watches it
/// change.
pub fn interrupts_taken() -> &'static AtomicU64 {
    &TAKEN
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

    // SAFETY: the caller's promise is `start_here`'s condition.
    unsafe { start_here() };
    frequency
}

/// Starts this CPU's timer. Every CPU has one of its own - the
/// counter is shared, the compare value and the interrupt are not -
/// so each CPU that is started does this for itself.
///
/// # Safety
/// After `init` on the boot CPU; on this CPU, once, interrupts masked,
/// after its part of the interrupt controller is set up.
pub unsafe fn start_here() {
    let first = counter() + INTERVAL.load(Ordering::Relaxed);
    let cpu = super::cpu();
    NEXT_TICK[cpu].store(first, Ordering::Relaxed);
    ARMED[cpu].store(first, Ordering::Relaxed);
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
    gic::enable(INTERRUPT.load(Ordering::Relaxed));
}

/// Sets this CPU's compare value: the timer interrupts when the counter
/// reaches `at`, and stops asking once it is in the future again.
fn set_compare(at: u64) {
    // SAFETY: writing this CPU's own compare value; nothing else uses it.
    unsafe { asm!("msr cntv_cval_el0, {}", "isb", in(reg) at, options(nomem, nostack, preserves_flags)) };
}

/// The timer's interrupt arrived: a tick, or (before the tick) a
/// sleeper's time (K24). The timer goes on asking for as long as the
/// counter is past the compare value, so the first thing is to move the
/// value on - to the next tick; waking sleepers then sets it sooner if
/// one is due before (`wake_by`).
pub fn on_interrupt() {
    let cpu = super::cpu();
    let (passed, next) = after_interrupt(NEXT_TICK[cpu].load(Ordering::Relaxed), counter(), INTERVAL.load(Ordering::Relaxed));
    NEXT_TICK[cpu].store(next, Ordering::Relaxed);
    ARMED[cpu].store(next, Ordering::Relaxed);
    set_compare(next);
    TAKEN.fetch_add(1, Ordering::Relaxed);
    if passed == 0 {
        crate::interrupts::timer_early();
    }
    for _ in 0..passed {
        crate::interrupts::timer_tick();
    }
}

/// Makes this CPU's timer interrupt by the time the counter reads `at`,
/// if it is set for later. With interrupts masked.
pub fn wake_by(at: u64) {
    let cpu = super::cpu();
    if at < ARMED[cpu].load(Ordering::Relaxed) {
        ARMED[cpu].store(at, Ordering::Relaxed);
        set_compare(at);
    }
}
