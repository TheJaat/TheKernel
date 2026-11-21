//! What interrupts do, independent of the hardware that raises them.
//!
//! Handlers here run with interrupts disabled, on whatever stack was
//! interrupted, so they do the minimum: count a tick, or note that a
//! device interrupted and wake its driver. Device drivers run in user
//! mode; the kernel never touches their hardware.

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::threads::WaitQueue;

/// Timer interrupts per second.
pub const HZ: u64 = 100;

static TICKS: AtomicU64 = AtomicU64::new(0);

/// A timer interrupt, on whichever CPU it arrived. Every CPU has its
/// own timer, at the same rate, for its own time slices. Time itself -
/// the tick count, and waking sleepers - is kept by the boot CPU alone:
/// four CPUs counting would make a second four times too short.
pub fn timer_tick() {
    if crate::arch::cpu() == 0 {
        let now = TICKS.fetch_add(1, Ordering::Relaxed) + 1;
        crate::threads::on_tick(now);
    } else {
        crate::threads::on_tick_elsewhere();
    }
}

/// How many times a CPU was interrupted by another to choose again.
static POKES: AtomicU64 = AtomicU64::new(0);

/// A "choose again" interrupt arrived on this CPU. Nothing to do but
/// count it: `interrupt_done` does the choosing.
pub fn poked() {
    POKES.fetch_add(1, Ordering::Relaxed);
}

pub fn pokes() -> u64 {
    POKES.load(Ordering::Relaxed)
}

/// The last thing every interrupt does, after acknowledging it: if the
/// interrupt made a thread ready that should run now - a sleeper's time
/// came, a slice ran out, a driver has work - switch.
pub fn interrupt_done() {
    crate::threads::preempt_if_needed();
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// A device's interrupt, as a kernel object a driver holds a handle to.
///
/// The handler only counts; the driver waits for the count to become
/// non-zero, then talks to its device. The count is what makes this
/// race-free: an interrupt that arrives while the driver is still busy
/// with the last one is not lost - the next wait returns at once.
///
/// This suits edge-triggered lines, where one interrupt is one event:
/// the ISA keyboard's, and every MSI-X message. A level-triggered line
/// (a PCI device's legacy pin) keeps interrupting until the device is
/// serviced, so it would need masking here and unmasking by the
/// driver's acknowledgement - which is why PCI devices get MSI-X.
///
/// For a line that does need it there is `source`: the interrupt
/// controller's number for the line, or 0. When one is set, the
/// handler switches the line off at the controller before it counts
/// (`source_fired`), and the line is switched on again only when its
/// driver comes back to wait - by which time the driver has dealt with
/// the device. The interrupt cannot arrive a second time while the
/// first is being handled in a program, whichever way the wire is
/// read, and the kernel needs to know nothing about the device.
pub struct InterruptLine {
    pending: AtomicU64,
    waiters: WaitQueue,
    source: AtomicU32,
}

impl InterruptLine {
    const fn new() -> InterruptLine {
        InterruptLine { pending: AtomicU64::new(0), waiters: WaitQueue::new(), source: AtomicU32::new(0) }
    }

    /// Fired and not yet waited for?
    pub fn is_pending(&self) -> bool {
        self.pending.load(Ordering::Relaxed) > 0
    }

    pub fn waiters(&self) -> &WaitQueue {
        &self.waiters
    }

    /// From the interrupt handler.
    pub fn fire(&self) {
        self.pending.fetch_add(1, Ordering::Relaxed);
        self.waiters.wake_all();
    }

    /// Waits until the line has fired; returns how many times it did
    /// since the last wait. None if the wait was cut short: the
    /// waiting process is being killed.
    pub fn wait(&self) -> Option<u64> {
        loop {
            // A line that was switched off when it fired: on again,
            // now that its driver is ready for the next one. If the
            // device has been asking meanwhile, it arrives at once.
            let source = self.source.load(Ordering::Relaxed);
            if source != 0 {
                crate::arch::rearm_interrupt(source);
            }
            if !self.waiters.wait_while(|| self.pending.load(Ordering::Relaxed) == 0) {
                return None;
            }
            let count = self.pending.swap(0, Ordering::Relaxed);
            if count > 0 {
                return Some(count); // else another holder took it; wait again
            }
        }
    }
}

/// ISA IRQ 1.
pub static KEYBOARD: InterruptLine = InterruptLine::new();

/// Lines for devices that interrupt with messages (MSI-X): one per
/// vector, handed out as devices are set up, never reused.
pub const DEVICE_LINES: usize = 16;
static DEVICES: [InterruptLine; DEVICE_LINES] = [const { InterruptLine::new() }; DEVICE_LINES];
static DEVICES_USED: AtomicUsize = AtomicUsize::new(0);

/// A free device line: its number and the line. None when all are out.
pub fn allocate_device_line() -> Option<(usize, &'static InterruptLine)> {
    let n = DEVICES_USED.fetch_add(1, Ordering::Relaxed);
    DEVICES.get(n).map(|line| (n, line))
}

#[cfg(target_arch = "aarch64")]
/// A free device line for the interrupt the controller calls `source`
/// (not 0), which is to be switched off each time it fires.
pub fn allocate_device_line_for(source: u32) -> Option<&'static InterruptLine> {
    let (_, line) = allocate_device_line()?;
    line.source.store(source, Ordering::Relaxed);
    Some(line)
}

#[cfg(target_arch = "aarch64")]
/// From the interrupt handler: the controller's interrupt `source`
/// arrived, and the handler has switched it off. Counts it on its
/// line; false if no line has that source.
pub fn source_fired(source: u32) -> bool {
    match DEVICES.iter().find(|line| line.source.load(Ordering::Relaxed) == source) {
        Some(line) => {
            line.fire();
            true
        }
        None => false,
    }
}

/// From the interrupt handler, for device line `n`.
pub fn device_fired(n: usize) {
    if let Some(line) = DEVICES.get(n) {
        line.fire();
    }
}
