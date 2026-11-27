//! Interrupt setup for a PC, and the first stop for every interrupt.
//!
//! Vectors (the IDT slot each source raises):
//!
//!   0x00-0x1F  CPU exceptions (idt.rs)
//!   0x20       local APIC timer
//!   0x21       the PS/2 controller: keyboard (ISA IRQ 1) and mouse
//!              (ISA IRQ 12), both through the I/O APIC
//!   0x30-0x3F  PCI devices' MSI-X messages, one vector per device line
//!   0xE0-0xEF  the disabled 8259 PICs - only ever spurious
//!   0xF0       "choose again": sent by one CPU to another (smp.rs)
//!   0xFF       local APIC spurious interrupt
//!
//! An interrupt's vector is also its priority (vector / 16): the timer
//! and keyboard are lowest, which is fine while they are the only ones.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use kernel64::smp::MAX_CPUS;

use kernel64::acpi::Madt;

use super::{apic, pic};
use crate::interrupts;
use crate::irqlock::IrqLock;
use crate::mm;

pub const TIMER_VECTOR: u8 = 0x20;
pub const KEYBOARD_VECTOR: u8 = 0x21;
pub const SPURIOUS_VECTOR: u8 = 0xFF;
/// Raised on a CPU by another CPU, to say: there is a thread in the
/// queue for you. The interrupt carries nothing; arriving is all it
/// does, because every interrupt ends by asking the scheduler whether
/// to switch.
pub const RESCHEDULE_VECTOR: u8 = 0xF0;
/// Another CPU took away a translation this one may remember (tlb.rs).
pub const TLB_VECTOR: u8 = 0xF1;
const DEVICE_VECTOR_BASE: u8 = 0x30;

/// The vector device line `n` (interrupts::allocate_device_line) is
/// raised by, and the local APIC id of the CPU to send it to - what an
/// MSI-X message must name.
pub fn device_vector(n: usize) -> (u8, u8) {
    (DEVICE_VECTOR_BASE + n as u8, apic::local_id())
}

/// What `init` found and set up, for the boot log.
pub struct Report {
    pub cpu_apic_id: u8,
    pub io_apics: Vec<(u8, u64, u32, u32)>, // id, physical address, first GSI, inputs
    pub timer_hz: u64,
    pub bus_hz: u64,
    pub tsc_hz: u64,
    /// When the frequencies were stated by a hypervisor: what the PIT
    /// measured (timer, TSC), for comparison.
    pub measured: Option<(u64, u64)>,
    pub keyboard_gsi: Option<u32>,
    pub mouse_gsi: Option<u32>,
}

static IO_APICS: IrqLock<Vec<apic::IoApic>> = IrqLock::new(Vec::new());
static TSC_HZ: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// How fast the local APIC's timer counts, measured once on the boot
/// CPU: the CPUs of one machine share the clock it counts.
static BUS_HZ: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

pub fn tsc_hz() -> u64 {
    TSC_HZ.load(core::sync::atomic::Ordering::Relaxed)
}

/// Sets up every interrupt source the kernel uses. Interrupts stay
/// disabled; the caller enables them when it is ready.
///
/// # Safety
/// Once, on the boot CPU, interrupts disabled, after memory is set up.
pub unsafe fn init(madt: &Madt, hz: u64) -> Report {
    // Every PC has the 8259s, whatever the MADT's flag says: switch them
    // off unconditionally.
    pic::disable();

    let local = mm::map_mmio(madt.local_apic_address, 4096);
    apic::init_local(local, SPURIOUS_VECTOR);
    let cpu = apic::local_id();

    let mut report = Report {
        cpu_apic_id: cpu,
        io_apics: Vec::new(),
        timer_hz: hz,
        bus_hz: 0,
        tsc_hz: 0,
        measured: None,
        keyboard_gsi: None,
        mouse_gsi: None,
    };

    {
        let mut io_apics = IO_APICS.lock();
        for io in &madt.io_apics {
            let base = mm::map_mmio(io.address, 4096);
            let apic = apic::IoApic::new(base, io.gsi_base);
            report.io_apics.push((io.id, io.address, io.gsi_base, apic.inputs));
            io_apics.push(apic);
        }

        // The keyboard: ISA IRQ 1, wherever the firmware wired it. The
        // kernel only routes it; the PS/2 controller itself is set up by
        // the user-mode driver (user/src/bin/kbd.rs).
        let route = madt.isa_route(1);
        if let Some(io) = io_apics.iter().find(|io| io.handles(route.gsi)) {
            io.route(route.gsi, KEYBOARD_VECTOR, route.polarity, route.trigger, cpu);
            report.keyboard_gsi = Some(route.gsi);
        }
        // And the mouse: ISA IRQ 12, on the same controller - to the
        // same vector, and so the same interrupt line. Its driver is
        // the keyboard's: both devices' bytes arrive at port 0x60, and
        // the controller's status says whose each one is, so one wait
        // and one read serve both.
        let route = madt.isa_route(12);
        if let Some(io) = io_apics.iter().find(|io| io.handles(route.gsi)) {
            io.route(route.gsi, KEYBOARD_VECTOR, route.polarity, route.trigger, cpu);
            report.mouse_gsi = Some(route.gsi);
        }
    }

    // What the hypervisor states, if it states it; else measured. The
    // PIT is still read either way, so the log can show both.
    let measured = apic::calibrate();
    let stated = apic::stated_frequencies();
    report.measured = stated.map(|_| measured);
    let (bus_hz, tsc_hz) = stated.unwrap_or(measured);
    report.bus_hz = bus_hz;
    report.tsc_hz = tsc_hz;
    TSC_HZ.store(tsc_hz, core::sync::atomic::Ordering::Relaxed);
    BUS_HZ.store(bus_hz, core::sync::atomic::Ordering::Relaxed);
    TICK_TSC.store((tsc_hz / hz).max(1), Ordering::Relaxed);
    start_timer_here();
    report
}

// ---- The timer, one-shot (K24, journal entry NNN) -----------------------
//
// Each CPU's local APIC timer interrupts once, and is set again at every
// interrupt: for its next tick (every TICK_TSC of the time-stamp
// counter, stepped from the last so that ticks do not drift), or sooner
// if a thread that went to sleep on this CPU is due before that. All in
// time-stamp counter steps, the clock's own (`arch::clock`).


/// A tick, in time-stamp counter steps.
static TICK_TSC: AtomicU64 = AtomicU64::new(0);
/// Per CPU: when its next tick is due, and what its timer is set for
/// now (that tick, or a sleeper's time before it). Only that CPU writes
/// them, with interrupts off.
static NEXT_TICK: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static ARMED: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// Sets this CPU's timer to interrupt when the time-stamp counter reads
/// `at` (at once if that is past).
fn set_timer_at(at: u64) {
    let steps = at.saturating_sub(super::tsc());
    let tsc_hz = TSC_HZ.load(Ordering::Relaxed).max(1);
    let count = u128::from(steps) * u128::from(BUS_HZ.load(Ordering::Relaxed)) / u128::from(tsc_hz);
    apic::set_timer(u32::try_from(count).unwrap_or(u32::MAX));
}

/// Starts the timer of the CPU this runs on: its first tick a tick from
/// now. On the boot CPU from `init`, on every other once it is ready to
/// be interrupted.
pub fn start_timer_here() {
    let cpu = super::cpu();
    let first = super::tsc() + TICK_TSC.load(Ordering::Relaxed);
    NEXT_TICK[cpu].store(first, Ordering::Relaxed);
    ARMED[cpu].store(first, Ordering::Relaxed);
    apic::start_timer(TIMER_VECTOR);
    set_timer_at(first);
}

/// The timer interrupted: a tick, or (before the tick) a sleeper's time.
/// The timer is set for the next tick first; waking sleepers then sets
/// it sooner if one is due before (`timer_wake_by`).
fn on_timer() {
    let cpu = super::cpu();
    let (ticks, next) = kernel64::gic::after_interrupt(NEXT_TICK[cpu].load(Ordering::Relaxed), super::tsc(), TICK_TSC.load(Ordering::Relaxed));
    NEXT_TICK[cpu].store(next, Ordering::Relaxed);
    ARMED[cpu].store(next, Ordering::Relaxed);
    set_timer_at(next);
    if ticks == 0 {
        interrupts::timer_early();
    } else {
        // One call however late: the tick count follows the counter, and
        // a late interrupt catches up (interrupts.rs).
        interrupts::timer_tick();
    }
}

/// Makes this CPU's timer interrupt by the time the clock reads `at`
/// (counter steps, `clock::counter_at`), if it is set for later. With
/// interrupts off.
pub fn timer_wake_by(at: u64) {
    let cpu = super::cpu();
    if at < ARMED[cpu].load(Ordering::Relaxed) {
        ARMED[cpu].store(at, Ordering::Relaxed);
        set_timer_at(at);
    }
}

/// Called by the IDT stub for every vector from 32 up, with interrupts
/// disabled. Must acknowledge the interrupt (EOI) or the local APIC will
/// deliver nothing more at this priority.
pub fn handle_interrupt(vector: u8) {
    match vector {
        // EOI before `interrupt_done`, which may switch threads: the
        // APIC must be ready for the next interrupt even if this thread
        // does not run again for a while.
        TIMER_VECTOR => {
            on_timer();
            apic::end_of_interrupt();
            interrupts::interrupt_done();
        }
        KEYBOARD_VECTOR => {
            // Not even reading the device: that is the driver's job. The
            // kernel only tells it something happened.
            interrupts::KEYBOARD.fire();
            apic::end_of_interrupt();
            interrupts::interrupt_done();
        }
        v if (DEVICE_VECTOR_BASE..DEVICE_VECTOR_BASE + interrupts::DEVICE_LINES as u8).contains(&v) => {
            interrupts::device_fired(usize::from(v - DEVICE_VECTOR_BASE));
            apic::end_of_interrupt();
            interrupts::interrupt_done();
        }
        RESCHEDULE_VECTOR => {
            interrupts::poked();
            apic::end_of_interrupt();
            interrupts::interrupt_done();
        }
        // No `interrupt_done`: nothing was made ready, and the CPU that
        // asked is waiting for this one with its interrupts off.
        TLB_VECTOR => {
            crate::tlb::answer();
            apic::end_of_interrupt();
        }
        // Spurious: nothing happened. Neither the APIC nor the masked
        // PICs expect an EOI for these.
        SPURIOUS_VECTOR => {}
        v if (pic::VECTOR_BASE..pic::VECTOR_BASE + 16).contains(&v) => {}
        _ => {
            kprintln!("[irq] unexpected interrupt {vector:#x}");
            apic::end_of_interrupt();
        }
    }
}
