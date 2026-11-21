//! AArch64.
//!
//! The port is at its beginning: the kernel starts, turns on the MMU,
//! writes to the serial port and catches exceptions. What the rest of
//! the kernel asks of an architecture - memory management, interrupts,
//! a timer, context switches, system calls, other CPUs - is still to
//! come, a piece at a time.

mod entry;
pub mod mmu;
mod pl011;
mod vectors;

use core::arch::asm;

/// Where QEMU's `virt` machine has its serial port, and the clock that
/// port counts from. Fixed numbers, for one machine: the right way to
/// learn both is from the device tree, which the kernel cannot read
/// yet.
const QEMU_VIRT_UART: u64 = 0x0900_0000;
const QEMU_VIRT_UART_CLOCK: u32 = 24_000_000;

/// Sets up the earliest output there is: the serial port.
pub fn init_early_console() {
    let registers = mmu::map_early_device(QEMU_VIRT_UART);
    // SAFETY: on the virt machine a PL011 is at that address, just
    // mapped as device memory; this is called once, first thing.
    unsafe { pl011::init(registers, QEMU_VIRT_UART_CLOCK) };
}

pub fn console_write(text: &str) {
    pl011::write_bytes(text.as_bytes());
}

/// The same for bytes that may not be whole text.
pub fn console_write_bytes(bytes: &[u8]) {
    pl011::write_bytes(bytes);
}

/// Points the CPU at the kernel's exception vectors: from here on a
/// fault is a message rather than silence.
pub fn init_cpu() {
    // SAFETY: called once, from kernel_main, on the only CPU running.
    unsafe { vectors::init() };
}

/// Raises a breakpoint exception, which the handler reports and
/// resumes from - with every register holding a known value, checked
/// afterwards. Proves the whole exception path - vector, save, Rust,
/// restore, `eret` - round-trips, and loses nothing on the way.
pub fn breakpoint() {
    let changed = vectors::round_trip();
    assert!(changed == 0, "{changed} of 31 registers were changed by an exception that should have restored them all");
}

/// Whether this CPU accepts interrupts now. PSTATE has four mask bits,
/// "DAIF"; the I bit (bit 7 as `mrs` shows it) masks ordinary
/// interrupts. Set means masked - the opposite sense from x86's IF.
pub fn interrupts_enabled() -> bool {
    let daif: u64;
    // SAFETY: reading the mask bits has no side effects.
    unsafe { asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack, preserves_flags)) };
    daif & (1 << 7) == 0
}

/// Disables interrupts, returning whether they were enabled - to hand
/// back to `restore_interrupts`.
pub fn disable_interrupts() -> bool {
    let were_enabled = interrupts_enabled();
    // SAFETY: only sets the I mask bit.
    unsafe { asm!("msr daifset, #2", options(nomem, nostack, preserves_flags)) };
    were_enabled
}

pub fn restore_interrupts(were_enabled: bool) {
    if were_enabled {
        enable_interrupts();
    }
}

pub fn enable_interrupts() {
    // SAFETY: only clears the I mask bit; the vectors handle whatever
    // then arrives.
    unsafe { asm!("msr daifclr, #2", options(nomem, nostack, preserves_flags)) };
}

/// There are no other CPUs running yet; this CPU just stops listening.
pub fn stop_other_cpus() {
    disable_interrupts();
}

/// Stops this CPU for good. `wfi` ("wait for interrupt") sleeps until
/// one is pending - even a masked one wakes it, hence the loop.
pub fn halt_forever() -> ! {
    disable_interrupts();
    loop {
        // SAFETY: wfi touches no memory; this CPU simply stops.
        unsafe { asm!("wfi", options(nomem, nostack, preserves_flags)) };
    }
}

/// What the kernel can say about the CPU it finds itself on.
pub struct Report {
    /// `MIDR_EL1`: who designed the CPU, and which one it is.
    pub cpu_id: u64,
    /// The exception level the kernel is running at.
    pub level: u64,
    /// The code for how many bits of physical address the CPU has.
    pub physical_range: u64,
}

pub fn report() -> Report {
    let (cpu_id, level, features): (u64, u64, u64);
    // SAFETY: three identification registers, readable at EL1, with no
    // side effects.
    unsafe {
        asm!("mrs {}, midr_el1", out(reg) cpu_id, options(nomem, nostack, preserves_flags));
        asm!("mrs {}, CurrentEL", out(reg) level, options(nomem, nostack, preserves_flags));
        asm!("mrs {}, id_aa64mmfr0_el1", out(reg) features, options(nomem, nostack, preserves_flags));
    }
    Report { cpu_id, level: level >> 2, physical_range: features & 0xF }
}

/// The first bytes of the device tree the loader left at `physical`,
/// or None if that cannot be an address of one.
///
/// Read through the low mapping entry.rs made for the purpose, which
/// covers the tree only if it lies below 512 GiB.
pub fn device_tree_header(physical: u64) -> Option<&'static [u8; kernel64::devicetree::HEADER_SIZE]> {
    // The loader's contract: 8-byte aligned. Zero means "none".
    if physical == 0 || !physical.is_multiple_of(8) || physical >> 39 != 0 {
        return None;
    }
    // SAFETY: entry.rs mapped the GiB this address is in to itself, as
    // readable memory, and nothing writes there: the loader put the
    // tree in memory the kernel has not touched.
    Some(unsafe { &*(physical as *const [u8; kernel64::devicetree::HEADER_SIZE]) })
}

/// Deliberately faults, for checking the handlers. Only built with one
/// of the `fault-*` features.
#[cfg(feature = "fault-tests")]
pub mod fault_tests {
    /// Reads an address no table maps: a data abort.
    pub fn page_fault() -> u64 {
        // In the kernel's half, 2 GiB below its window: the level 1
        // table has no entry there.
        let address = 0xFFFF_FFFF_0000_0000u64 as *const u64;
        // SAFETY: none - this is meant to fault.
        unsafe { address.read_volatile() }
    }
}
