//! x86_64.

mod apic;
pub mod cmos;
pub mod context;
mod entry;
mod gdt;
mod idt;
pub mod percpu;
pub mod mmu;
pub mod pci;
mod pic;
mod pit;
pub mod platform;
mod port;
mod serial;
pub mod smp;
pub mod syscall;

pub use gdt::set_io_bitmap;

use core::arch::asm;

/// Sets up the earliest output there is: the COM1 serial port.
pub fn init_early_console() {
    serial::init();
}

pub fn console_write(text: &str) {
    serial::write_bytes(text.as_bytes());
}

/// The same for bytes that may not be whole text - a piece of a
/// program's output, cut anywhere.
pub fn console_write_bytes(bytes: &[u8]) {
    serial::write_bytes(bytes);
}

/// Replaces the bootloader's descriptor tables with the kernel's own:
/// a GDT with kernel and user segments, a TSS with a separate stack for
/// double faults, and an IDT that catches every CPU exception.
pub fn init_cpu() {
    // SAFETY: called once, from kernel_main, on the only CPU running,
    // with interrupts disabled - the conditions both inits require.
    unsafe {
        gdt::init();
        // Right after the GDT: loading it reset GS, and from here on
        // "this CPU's block" must be findable.
        percpu::install(percpu::boot(), gdt::boot_tables());
        idt::init();
        syscall::init();
        context::enable_fpu();
    }
}

/// The kernel's number for the CPU this runs on; 0 is the boot CPU.
///
/// Only as good as the moment it is asked: with interrupts on, the
/// thread asking may be on another CPU by the time it uses the answer.
/// This function's caller's frame pointer, for a backtrace: the frame
/// record of whoever called this (always inlined, so: of the function it
/// is written in).
#[inline(always)]
pub fn frame_pointer() -> u64 {
    let frame: u64;
    // SAFETY: reads a register; touches no memory.
    unsafe { core::arch::asm!("mov {}, rbp", out(reg) frame, options(nomem, nostack, preserves_flags)) };
    frame
}

pub fn cpu() -> usize {
    percpu::index()
}

/// Whether this CPU accepts interrupts now (RFLAGS.IF).
pub fn interrupts_enabled() -> bool {
    let flags: u64;
    // SAFETY: reading RFLAGS has no side effects.
    unsafe { asm!("pushfq", "pop {}", out(reg) flags, options(nomem, preserves_flags)) };
    flags & (1 << 9) != 0
}

/// Disables interrupts, returning whether they were enabled - to hand
/// back to `restore_interrupts`.
pub fn disable_interrupts() -> bool {
    let were_enabled = interrupts_enabled();
    // SAFETY: cli only changes RFLAGS.IF.
    unsafe { asm!("cli", options(nomem, nostack)) };
    were_enabled
}

pub fn restore_interrupts(were_enabled: bool) {
    if were_enabled {
        enable_interrupts();
    }
}

pub fn enable_interrupts() {
    // SAFETY: the IDT handles every vector by the time anyone calls this.
    unsafe { asm!("sti", options(nomem, nostack)) };
}

/// Enables interrupts and sleeps until one arrives. `sti` takes effect
/// only after the NEXT instruction, so no interrupt can slip in between
/// the two and leave `hlt` waiting for a second one.
pub fn wait_for_interrupt() {
    // SAFETY: as for enable_interrupts.
    unsafe { asm!("sti", "hlt", options(nomem, nostack)) };
}

/// The clock to measure short times with, and how many of its steps
/// make a second - by these names on every CPU. Here it is the
/// time-stamp counter.
pub fn clock() -> u64 {
    tsc()
}

/// Called when a driver waits for an interrupt that was switched off
/// when it fired. No line on the PC is: every device interrupts with a
/// message (MSI-X), which is one event and needs no switching off.
pub fn rearm_interrupt(_source: u32) {}

pub fn clock_hz() -> u64 {
    platform::tsc_hz()
}

/// A random number from the CPU itself - RDSEED, else RDRAND - if it
/// has either (CPUID says), and it gives one within a few tries; None
/// otherwise. QEMU's default CPU has neither; `-cpu max` and most PCs
/// since 2012 have RDRAND.
pub fn hardware_random() -> Option<u64> {
    let (leaf1, leaf7) = (core::arch::x86_64::__cpuid(1), core::arch::x86_64::__cpuid_count(7, 0));
    let has_rdrand = leaf1.ecx & (1 << 30) != 0;
    let has_rdseed = leaf7.ebx & (1 << 18) != 0;
    for _ in 0..10 {
        let (value, ok): (u64, u8);
        if has_rdseed {
            // SAFETY: the CPU said it has RDSEED; it only writes the register and CF.
            unsafe { asm!("rdseed {0}", "setc {1}", out(reg) value, out(reg_byte) ok, options(nomem, nostack)) };
        } else if has_rdrand {
            // SAFETY: as above, for RDRAND.
            unsafe { asm!("rdrand {0}", "setc {1}", out(reg) value, out(reg_byte) ok, options(nomem, nostack)) };
        } else {
            return None;
        }
        if ok != 0 {
            return Some(value);
        }
    }
    None
}

/// The name of the instruction `hardware_random` uses, for the log.
pub fn hardware_random_name() -> &'static str {
    let (leaf1, leaf7) = (core::arch::x86_64::__cpuid(1), core::arch::x86_64::__cpuid_count(7, 0));
    if leaf7.ebx & (1 << 18) != 0 {
        "RDSEED"
    } else if leaf1.ecx & (1 << 30) != 0 {
        "RDRAND"
    } else {
        "none"
    }
}

/// The CPU's time-stamp counter: cycles since reset.
pub fn tsc() -> u64 {
    let (low, high): (u32, u32);
    // SAFETY: rdtsc only reads the counter.
    unsafe { asm!("rdtsc", out("eax") low, out("edx") high, options(nomem, nostack, preserves_flags)) };
    (u64::from(high) << 32) | u64::from(low)
}

/// Waits about `microseconds`, by watching the time-stamp counter - for
/// the few places that must wait before there is a scheduler to sleep
/// on, or with interrupts off. The CPU does nothing else meanwhile.
pub fn delay_us(microseconds: u64) {
    let cycles = platform::tsc_hz() / 1_000_000 * microseconds;
    let start = tsc();
    while tsc().wrapping_sub(start) < cycles {
        core::hint::spin_loop();
    }
}

/// Raises a breakpoint exception, which the IDT reports and resumes
/// from. Proves the whole exception path - stub, save, Rust, restore,
/// `iretq` - round-trips correctly.
pub fn breakpoint() {
    // SAFETY: int3 only raises vector 3, whose handler returns.
    unsafe { core::arch::asm!("int3", options(nomem, nostack)) };
}

/// Set once, by the CPU that is stopping the system.
static STOPPING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Stops every other CPU, where it stands, for good. For the CPU that
/// is about to report why the kernel cannot go on: the others must not
/// carry on meanwhile - running programs, printing over the report,
/// repainting the screen it is written on.
///
/// The others are sent a non-maskable interrupt, and halt in its
/// handler (idt.rs). If another CPU is already stopping the system,
/// this one halts instead: one report, not two through each other.
pub fn stop_other_cpus() {
    use core::sync::atomic::Ordering;
    disable_interrupts();
    if STOPPING.swap(true, Ordering::AcqRel) {
        halt_forever();
    }
    smp::nmi_others();
    // Long enough for them to get there. Not `delay_us`: it needs the
    // clock's speed, and this may be called before that is measured.
    for _ in 0..200_000 {
        core::hint::spin_loop();
    }
}

/// Whether some CPU is stopping the system.
pub fn stopping() -> bool {
    STOPPING.load(core::sync::atomic::Ordering::Acquire)
}

/// Stops this CPU for good. `hlt` waits for an interrupt; with
/// interrupts disabled only an NMI can wake it, hence the loop.
pub fn halt_forever() -> ! {
    loop {
        // SAFETY: cli and hlt touch no memory; this CPU simply stops.
        unsafe { core::arch::asm!("cli", "hlt", options(nomem, nostack)) };
    }
}

/// Deliberately faults, for checking the handlers and the page-table
/// protections. Only built with one of the `fault-*` features.
#[cfg(feature = "fault-tests")]
pub mod fault_tests {
    /// Reads an address no page table maps: a page fault.
    pub fn page_fault() -> u64 {
        // 256 GiB, in the lower half - which the kernel maps nothing in.
        let address = 0x40_0000_0000 as *const u64;
        // SAFETY: none - this is meant to fault.
        unsafe { address.read_volatile() }
    }

    /// Points the stack at unmapped memory, then raises an exception.
    /// The CPU cannot push the exception frame, so delivering *that*
    /// page fault fails too - a double fault, which only works because
    /// its handler runs on its own IST stack.
    pub fn double_fault() -> ! {
        // SAFETY: none - this is meant to fault, and never returns.
        unsafe { core::arch::asm!("mov rsp, 0x4000000000", "int3", options(noreturn)) }
    }

    /// An immutable static: the linker puts it in .rodata, which the
    /// kernel maps read-only.
    static READ_ONLY: u64 = 0x1234;

    /// Writes to read-only data. Without CR0.WP the kernel would be
    /// allowed to; with it, this is a protection fault.
    pub fn write_rodata() {
        let address = &raw const READ_ONLY as *mut u64;
        // SAFETY: none - this is meant to fault.
        unsafe { address.write_volatile(0xBAD) };
    }

    /// One `ret` instruction, stored as data in .data, which is mapped
    /// no-execute. `#[used]` keeps it even though nothing reads it as data.
    #[used]
    static mut DATA: [u8; 1] = [0xC3];

    /// Calls into data. Code injected into a buffer is exactly this - and
    /// NX is what stops it running.
    pub fn execute_data() {
        // black_box hides where the pointer came from. Without it, LLVM
        // sees a call to a data object, reasons about it as if it were a
        // function, and the build fails to link.
        let address = core::hint::black_box(&raw const DATA as *const ());
        // SAFETY: none - this is meant to fault.
        let function: extern "C" fn() = unsafe { core::mem::transmute(address) };
        function();
    }

    /// Recurses until the boot stack runs into its guard page.
    #[allow(unconditional_recursion)]
    pub fn overflow_stack(depth: u64) -> u64 {
        // A big frame, kept alive by black_box, so this can neither be
        // turned into a loop nor optimised away.
        let frame = core::hint::black_box([depth; 64]);
        overflow_stack(depth + 1) + frame[7]
    }
}
