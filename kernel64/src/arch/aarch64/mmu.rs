//! Translation tables - for now, only the one thing the kernel needs
//! before it has a memory manager: a way to reach a device.
//!
//! entry.rs left one table empty: `boot_l2_device`, 512 entries of
//! 2 MiB each, covering the top 1 GiB of the address space
//! (`DEVICE_WINDOW`). `map_early_device` fills the next free entry and
//! returns where the device now is.
//!
//! Why not reach the device at its physical address, through the low
//! mapping entry.rs also made? That mapping is ordinary cached memory,
//! and is only there to get through the first instructions; a device
//! needs the "device" kind (each access really happens, in order), and
//! the low half of the address space belongs to programs.

use core::arch::asm;
use core::sync::atomic::{AtomicUsize, Ordering};

use kernel64::aarch64::{descriptor, BLOCK_SIZE, DEVICE_WINDOW};

extern "C" {
    /// The table, in entry.rs.
    static mut boot_l2_device: [u64; 512];
}

/// The next entry of the table not yet used.
static NEXT: AtomicUsize = AtomicUsize::new(0);

/// Maps the 2 MiB step that holds `physical` as device memory, and
/// returns the address at which `physical` itself can now be used.
///
/// Never undone, and at most 512 times: this is for the handful of
/// devices needed before memory management starts.
pub fn map_early_device(physical: u64) -> *mut u8 {
    let slot = NEXT.fetch_add(1, Ordering::Relaxed);
    assert!(slot < 512, "no room left in the early device window");
    let step = physical & !(BLOCK_SIZE - 1);
    // SAFETY: the table is in the kernel's .bss, mapped and writable;
    // `slot` is inside it and - NEXT only counts up - was never used,
    // so no CPU can be relying on what the entry held. Going from "not
    // mapped" to "mapped" needs nothing forgotten: the CPU never
    // remembers a missing entry. `dsb` waits for the write to have
    // happened and `isb` makes the next instruction see it.
    unsafe {
        (&raw mut boot_l2_device).cast::<u64>().add(slot).write_volatile(step | descriptor::DEVICE_BLOCK);
        asm!("dsb ishst", "isb", options(nostack, preserves_flags));
    }
    (DEVICE_WINDOW + slot as u64 * BLOCK_SIZE + (physical - step)) as *mut u8
}
