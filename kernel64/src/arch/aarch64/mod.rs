//! AArch64.
//!
//! The port is under way: the kernel starts, turns on the MMU, writes
//! to the serial port, catches exceptions, and manages memory with the
//! same code as on a PC. What else the kernel asks of an architecture
//! (interrupts, a timer, context switches, system calls, other CPUs)
//! is still to come, a piece at a time.

mod entry;
pub mod mmu;
mod pl011;
mod vectors;

use core::arch::asm;

/// Where QEMU's `virt` machine has its serial port, and the clock that
/// port counts from. Used only when the device tree does not say: a
/// guess, which is right on that one machine and may be anything on
/// another - but a kernel with no tree and no console could not even
/// say that it has no tree.
const QEMU_VIRT_UART: u64 = 0x0900_0000;
const QEMU_VIRT_UART_CLOCK: u32 = 24_000_000;

/// Sets up the earliest output there is: the serial port the device
/// tree described. Returns false if it described none (or none whose
/// clock it gave) and the guess above was used for what was missing.
pub fn init_early_console(serial: Option<kernel64::devicetree::Serial>) -> bool {
    let address = serial.map(|serial| serial.registers.start);
    let clock = serial.and_then(|serial| serial.clock_hz);
    let registers = mmu::map_early_device(address.unwrap_or(QEMU_VIRT_UART));
    // SAFETY: the tree (or, failing it, the virt machine's layout) says
    // a PL011 is at that address, just mapped as device memory; this is
    // called once, before anything is printed.
    unsafe { pl011::init(registers, clock.unwrap_or(QEMU_VIRT_UART_CLOCK)) };
    address.is_some() && clock.is_some()
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

/// The device tree the loader left at `physical`, as bytes - or None
/// if that cannot be the address of one, or the bytes there do not
/// begin as one does.
///
/// Read wherever physical memory is to be found at the moment. Before
/// the kernel has its own tables that is the low mapping entry.rs
/// made: the one GiB the address is in - so a tree that runs over the
/// end of that GiB is refused rather than read into memory that is not
/// mapped. Afterwards it is the direct map, and the caller must have
/// had the tree's memory put in it (`BootMemory::taken`).
pub fn device_tree(physical: u64) -> Option<&'static [u8]> {
    use kernel64::devicetree::{Header, HEADER_SIZE};
    use kernel64::memory::addr::{phys_to_virt, physical_offset, PhysAddr};
    const GIB: u64 = 1 << 30;

    // The loader's contract: 8-byte aligned. Zero means "none". And
    // entry.rs mapped nothing above 512 GiB.
    if physical == 0 || !physical.is_multiple_of(8) || physical >> 39 != 0 {
        return None;
    }
    let early = physical_offset() == 0;
    let room = if early { GIB - physical % GIB } else { u64::MAX };
    if room < HEADER_SIZE as u64 {
        return None;
    }
    let address = phys_to_virt(PhysAddr::new(physical));
    // SAFETY: the forty bytes are mapped, readable - by entry.rs, which
    // mapped the GiB this address is in to itself, or by the direct
    // map. Nothing writes there: the loader put the tree in memory the
    // kernel has kept out of its allocator.
    let header = unsafe { &*address.as_ptr::<[u8; HEADER_SIZE]>() };
    let size = u64::from(Header::parse(header).ok()?.total_size);
    if size > room {
        return None;
    }
    // SAFETY: as above, for the whole of the blob - which the header,
    // now known to be a device tree's, says is `size` bytes.
    Some(unsafe { core::slice::from_raw_parts(address.as_ptr::<u8>(), size as usize) })
}

/// Deliberately faults, for checking the handlers and the page-table
/// protections. Only built with one of the `fault-*` features.
#[cfg(feature = "fault-tests")]
pub mod fault_tests {
    /// An immutable static: the linker puts it in .rodata, which the
    /// kernel maps read-only.
    static READ_ONLY: u64 = 0x1234;

    /// Writes to read-only data.
    pub fn write_rodata() {
        let address = &raw const READ_ONLY as *mut u64;
        // SAFETY: none - this is meant to fault.
        unsafe { address.write_volatile(0xBAD) };
    }

    /// One `ret` instruction, stored as data in .data, which is mapped
    /// no-execute. `#[used]` keeps it even though nothing reads it as
    /// data.
    #[used]
    static mut DATA: [u32; 1] = [0xD65F_03C0];

    /// Calls into data. Code injected into a buffer is exactly this -
    /// and the no-execute bit is what stops it running.
    pub fn execute_data() {
        // black_box hides where the pointer came from, so the compiler
        // cannot reason about a call to a data object.
        let address = core::hint::black_box(&raw const DATA as *const ());
        // SAFETY: none - this is meant to fault.
        let code: extern "C" fn() = unsafe { core::mem::transmute(address) };
        code();
    }

    /// Reads an address no table maps: a data abort.
    pub fn page_fault() -> u64 {
        // In the kernel's half, 2 GiB below its window: the level 1
        // table has no entry there.
        let address = 0xFFFF_FFFF_0000_0000u64 as *const u64;
        // SAFETY: none - this is meant to fault.
        unsafe { address.read_volatile() }
    }

    /// Reads through a null pointer - the commonest mistake there is.
    /// The low half of the address space is off, so there is nothing
    /// at address 0 to read.
    pub fn null_read() -> u64 {
        let address = core::hint::black_box(core::ptr::null::<u64>());
        // SAFETY: none - this is meant to fault.
        unsafe { address.read_volatile() }
    }
}
