//! AArch64.
//!
//! Everything the kernel asks of an architecture: the first
//! instructions and the MMU, a serial console, exceptions, interrupts
//! and a timer, context switches, system calls, and the other CPUs.

pub mod context;
mod entry;
mod gic;
pub mod mmu;
mod pl011;
pub mod smp;
pub mod timer;
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

/// Which CPU this is: the kernel's number for it, 0 for the one that
/// booted.
///
/// Every CPU runs the same code and sees the same memory, so the
/// answer cannot be in a variable. It is in a register that each CPU
/// has its own copy of and that is there for exactly this kind of
/// thing: `TPIDR_EL1`, "thread ID register" for exception level 1,
/// which the CPU itself never reads or changes. Each CPU's first
/// instructions put its number there (entry.rs).
pub fn cpu() -> usize {
    let number: u64;
    // SAFETY: reading a register that holds only what was put in it.
    unsafe { asm!("mrs {}, tpidr_el1", out(reg) number, options(nomem, nostack, preserves_flags)) };
    number as usize
}

/// The clock to measure short times with: the counter the timer
/// compares against, and how many of its steps make a second.
pub fn clock() -> u64 {
    timer::counter()
}

pub fn clock_hz() -> u64 {
    timer::frequency()
}

/// A PC has "I/O ports", a second address space for devices, and each
/// process a list of the ports it may use. AArch64 has no such thing:
/// every device is in memory, and page tables decide.
pub fn set_io_bitmap(_ports: &kernel64::ioports::PortBitmap) {}

/// Entering programs. (Leaving them is an exception like any other:
/// vectors.rs.)
pub mod syscall {
    use core::sync::atomic::{AtomicU64, Ordering};

    /// How far below the top of its stack a program's stack pointer
    /// starts. Nothing is pushed for it, as a return address is on
    /// x86; the gap only keeps the stack pointer a multiple of 16, as
    /// the calling convention wants, with two words above it that
    /// exist.
    pub const ENTRY_STACK_GAP: u64 = 16;

    /// The top of the kernel stack of the thread running on each CPU.
    static KERNEL_STACK: [AtomicU64; kernel64::smp::MAX_CPUS] = [const { AtomicU64::new(0) }; kernel64::smp::MAX_CPUS];

    /// Notes where the kernel's stack is for the thread about to run.
    /// There is nothing to tell the CPU, as there is on x86: its
    /// kernel stack pointer is a register of its own (`SP_EL1`), which
    /// stays where the thread left it when it went to its program.
    /// The note is for `enter_user`, which puts it there the first
    /// time.
    pub fn set_kernel_stack(top: u64) {
        KERNEL_STACK[super::cpu()].store(top, Ordering::Relaxed);
    }

    /// Drops to level 0 at `entry`, with stack pointer `stack` and two
    /// arguments for `_start` in x0 and x1. Every other register is
    /// zero, so nothing of the kernel's leaks to the program.
    ///
    /// # Safety
    /// The current address space must map `entry` and the stack for
    /// programs, and `set_kernel_stack` must have named this thread's
    /// kernel stack.
    pub unsafe fn enter_user(entry: u64, stack: u64, first: u64, second: u64) -> ! {
        let top = KERNEL_STACK[super::cpu()].load(Ordering::Relaxed);
        assert!(top != 0, "a program started on a thread without a kernel stack of its own");
        // SAFETY: the caller's promise; `top` is the top of the stack
        // this very code runs on, which the thread never needs again.
        unsafe { super::vectors::enter_user_frame(top, entry, stack, first, second) }
    }
}

/// Lets interrupts in and sleeps until one is pending; it is taken at
/// once on waking. For the idle loop - which a CPU started after the
/// first enters with interrupts still off.
///
/// (An interrupt that arrives between the two instructions is taken
/// there, and `wfi` then sleeps until the next. Nothing is lost by
/// that: a thread it made ready was switched to on the way out of the
/// interrupt, and the timer is never more than a tick away.)
pub fn wait_for_interrupt() {
    // SAFETY: clears the I mask bit, as `enable_interrupts` does; wfi
    // touches no memory and changes no register.
    unsafe { asm!("msr daifclr, #2", "wfi", options(nomem, nostack, preserves_flags)) };
}

/// What `init_interrupts` set up, for the kernel to say.
pub struct Interrupts {
    pub version: u8,
    pub lines: u32,
    pub timer_interrupt: u32,
    pub timer_hz: u64,
}

/// The virtual timer's interrupt on every machine that follows Arm's
/// recommendation - used only if the device tree does not say.
const USUAL_TIMER_INTERRUPT: u32 = 27;

/// Sets up the interrupt controller the device tree describes and
/// starts the timer. Interrupts stay masked: `enable_interrupts` is
/// the caller's to call. None if the tree describes no controller this
/// kernel knows.
///
/// # Safety
/// Once, on the boot CPU, after `mm::init`, interrupts masked.
pub unsafe fn init_interrupts(machine: &kernel64::devicetree::Machine) -> Option<Interrupts> {
    let controller = machine.interrupt_controller?;
    // SAFETY: the caller's promises are these functions' conditions.
    unsafe {
        let info = gic::init(controller);
        // The tree's third timer interrupt is the virtual timer's.
        let timer_interrupt = machine.timer_interrupts[2].unwrap_or(USUAL_TIMER_INTERRUPT);
        let timer_hz = timer::init(timer_interrupt);
        // The two interrupts other CPUs may send this one. Asked for
        // even on a machine with one CPU: nobody sends them there.
        gic::enable(kernel64::gic::SOFTWARE_RESCHEDULE);
        gic::enable(kernel64::gic::SOFTWARE_STOP);
        Some(Interrupts { version: info.version, lines: info.lines, timer_interrupt, timer_hz })
    }
}

/// Interrupts that arrived and that nothing handles.
static UNEXPECTED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// How many device interrupts were passed on to drivers.
static DEVICE_INTERRUPTS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

pub fn device_interrupts() -> u64 {
    DEVICE_INTERRUPTS.load(core::sync::atomic::Ordering::Relaxed)
}

/// A line for the device interrupt the controller calls `id`, read as
/// the machine's description says. It is not let through yet: that
/// happens when a driver first waits for it. None if the controller
/// has no such interrupt, or no line is left.
pub fn device_interrupt(id: u32, edge: bool) -> Option<&'static crate::interrupts::InterruptLine> {
    if !gic::has_shared(id) {
        return None;
    }
    let line = crate::interrupts::allocate_device_line_for(id)?;
    // Set up - how it is read, where it goes - and then off again
    // until someone waits.
    gic::enable_shared(id, edge);
    gic::disable(id);
    Some(line)
}

/// Lets device interrupt `id` through again: its driver is waiting.
pub fn rearm_interrupt(id: u32) {
    gic::enable(id);
}

/// Called from the vectors when the CPU's interrupt input is raised:
/// ask the controller which, handle, say done - until it has no more.
/// Then, last, let the scheduler take the CPU from the interrupted
/// thread if its time is up: the switch happens here, inside the
/// handler, and the interrupt "returns" when the thread next runs.
fn interrupt() {
    while let Some((id, raw)) = gic::acknowledge() {
        if id == timer::interrupt() {
            timer::on_interrupt();
        } else if id == kernel64::gic::SOFTWARE_RESCHEDULE {
            // Another CPU made a thread ready while this one sat
            // halted. Nothing to do but count it: the choosing is
            // done below, as after every interrupt.
            crate::interrupts::poked();
        } else if id == kernel64::gic::SOFTWARE_STOP {
            // The kernel is stopping, on another CPU, and says so there.
            gic::end(raw);
            halt_forever();
        } else if gic::has_shared(id) && {
            // A device's. Off first, then counted: its driver is a
            // program, which will not have dealt with the device by
            // the time this returns, and a line still raised would
            // interrupt again at once, for ever. It comes back on
            // when the driver waits again (`rearm_interrupt`).
            gic::disable(id);
            crate::interrupts::source_fired(id)
        } {
            DEVICE_INTERRUPTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        } else {
            // Nothing else is enabled, so this should not happen. If
            // it does, say so once and silence the line: a device
            // that keeps asking would otherwise be all the kernel
            // ever did again.
            UNEXPECTED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            gic::disable(id);
            kprintln!("[irq] unexpected interrupt {id}: disabled");
        }
        gic::end(raw);
    }
    crate::interrupts::interrupt_done();
}

/// Checks two things about interrupts that the timer test shared
/// with the PC (selftest.rs) does not: registers survive being
/// interrupted, and masked means masked - with the time that passed
/// meanwhile caught up afterwards.
pub fn selftest_interrupts() {
    use crate::interrupts::ticks;
    use core::sync::atomic::Ordering;

    // 0. Not interrupts, but the same kind of check and needed before
    // there are threads: a switch to another stack and back keeps
    // every register it must.
    let stack = crate::mm::allocate_stack().expect("no stack for the switch test");
    let were_enabled = disable_interrupts();
    // SAFETY: interrupts are off, and the stack was mapped a moment
    // ago for this alone.
    let changed = unsafe { context::round_trip(stack.top()) };
    restore_interrupts(were_enabled);
    drop(stack);
    assert!(changed == 0, "{changed} of 11 registers were changed by a switch to another stack and back");
    kprintln!("[selftest] context switch: there and back with a known value in each of 11 registers, none changed");

    // 1. Interrupted five times in the middle of a loop that has a
    // number in every register it can spare.
    let changed = vectors::interrupted_round_trip(timer::interrupts_taken(), 5);
    assert!(changed == 0, "{changed} of 28 registers were changed by interrupts");
    kprintln!("[selftest] interrupts: 5 taken with a known value in each of 28 registers, none changed");

    // 2. Masked for 35 ms: three ticks' worth. None may arrive; and
    // once unmasked, all three must be counted - by the one interrupt
    // that was waiting.
    disable_interrupts();
    let (before, start) = (ticks(), timer::counter());
    while timer::milliseconds_since(start) < 35 {
        core::hint::spin_loop();
    }
    let during = ticks() - before;
    enable_interrupts();
    let start = timer::counter();
    while ticks() < before + 3 && timer::milliseconds_since(start) < 5 {
        core::hint::spin_loop();
    }
    let after = ticks() - before;
    assert!(during == 0, "{during} ticks arrived while interrupts were masked");
    assert!(after >= 3, "after 35 ms masked only {after} ticks were counted, not the 3 that passed");
    kprintln!("[selftest] interrupts masked for 35 ms: no tick arrived; unmasked: {after} counted at once");

    let unexpected = UNEXPECTED.load(Ordering::Relaxed);
    assert!(unexpected == 0, "{unexpected} interrupts arrived that nothing asked for");
}

/// Stops every other CPU, for a kernel that is about to halt: each is
/// sent an interrupt whose handler halts it. Not waited for - a CPU
/// with interrupts off will not answer, and the one that is stopping
/// cannot do anything about that.
pub fn stop_other_cpus() {
    disable_interrupts();
    smp::stop_others();
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

    /// Recurses until the stack runs into the page kept unmapped below
    /// it.
    #[allow(unconditional_recursion)]
    pub fn overflow_stack(depth: u64) -> u64 {
        // A big frame, kept alive by black_box, so this can neither be
        // turned into a loop nor optimised away.
        let frame = core::hint::black_box([depth; 64]);
        overflow_stack(depth + 1) + frame[7]
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
