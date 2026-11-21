//! The architecture layer.
//!
//! The rest of the kernel calls only what is re-exported here, never an
//! arch module directly. Adding AArch64 means adding `arch/aarch64/`
//! with the same functions - and nothing outside `arch/` changes.

#[cfg(target_arch = "x86_64")]
mod x86_64;

#[cfg(target_arch = "x86_64")]
pub use self::x86_64::{
    breakpoint, clock, clock_hz, console_write, console_write_bytes, cpu, disable_interrupts, enable_interrupts, halt_forever, init_cpu,
    init_early_console, mmu, pci, platform, context, syscall, rearm_interrupt, restore_interrupts, set_io_bitmap, smp, stop_other_cpus, tsc,
    wait_for_interrupt,
};

#[cfg(all(target_arch = "x86_64", feature = "fault-tests"))]
pub use self::x86_64::fault_tests;

// AArch64 exports only what it has so far. The list grows towards the
// one above as the port goes on; when the two are the same, nothing
// outside `arch/` will know which CPU it runs on.
#[cfg(target_arch = "aarch64")]
mod aarch64;

#[cfg(target_arch = "aarch64")]
pub use self::aarch64::{
    breakpoint, clock, clock_hz, console_write, console_write_bytes, context, cpu, device_interrupt, device_interrupts, device_tree,
    disable_interrupts, rearm_interrupt,
    enable_interrupts, halt_forever, init_cpu, init_early_console, init_interrupts, mmu, report, restore_interrupts,
    selftest_interrupts, set_io_bitmap, smp, stop_other_cpus, syscall, wait_for_interrupt,
};

#[cfg(all(target_arch = "aarch64", feature = "fault-tests"))]
pub use self::aarch64::fault_tests;
