//! What each CPU keeps for itself, and how it finds it.
//!
//! With one CPU, "the running thread's kernel stack" could be a global
//! variable. With several it cannot: each CPU runs a different thread.
//! Every CPU needs a block of memory of its own - and, harder, a way to
//! find it that works from the very first instruction of a system call,
//! when every general register still holds something of the program's.
//!
//! x86-64 has a register for exactly this: **GS**. In 64-bit mode a
//! segment register no longer describes a segment, but FS and GS still
//! have a *base address*, and `gs:[8]` means "the memory at that base,
//! plus 8". Point each CPU's GS base at its own block, and the same
//! instruction reads a different CPU's data on each CPU.
//!
//! The catch: GS is also the program's to use. So there are two bases,
//! in two model-specific registers, and an instruction that exchanges
//! them:
//!
//!   GS_BASE          the one `gs:` uses now
//!   KERNEL_GS_BASE   the other one, waiting
//!   swapgs           exchange them (ring 0 only)
//!
//! The rule the kernel keeps: **in ring 0, GS_BASE is this CPU's block;
//! in ring 3, it is the program's.** Every way into the kernel from a
//! program does `swapgs` first, every way out does it last:
//!
//!   system call     syscall.rs: first and last instruction
//!   interrupt or    idt.rs: only if the saved CS says ring 3 - an
//!   exception         interrupt that arrived in the kernel found the
//!                     kernel's GS already there, and swapping again
//!                     would hand the kernel the program's
//!   first entry     syscall.rs, `enter_user`
//!
//! Nothing is saved per thread: a thread in the kernel uses the GS of
//! whichever CPU it is on, which is what "per CPU" means.

use core::arch::asm;

use super::gdt::CpuTables;
use super::mmu::write_msr;

const GS_BASE: u32 = 0xC000_0101;
const KERNEL_GS_BASE: u32 = 0xC000_0102;

/// One CPU's block. `repr(C)`: the assembly in syscall.rs reads the
/// first fields by their offsets, so the order is the layout.
#[repr(C)]
pub struct PerCpu {
    /// Where this block is - so that `gs:[0]` gives a pointer Rust can
    /// use. (Reading the GS base itself needs a slow MSR read.)
    this: *mut PerCpu,
    /// The stack `syscall` switches to: the running thread's.
    kernel_rsp: u64,
    /// Where the system call entry parks the program's stack pointer
    /// for the two instructions before it is on the kernel stack.
    user_rsp: u64,
    /// The kernel's number for this CPU; 0 is the boot CPU.
    pub index: usize,
    /// This CPU's GDT and TSS.
    pub(super) tables: *mut CpuTables,
}

/// Offsets for the assembly, checked against the struct.
pub const KERNEL_RSP: usize = 8;
pub const USER_RSP: usize = 16;
const _: () = assert!(core::mem::offset_of!(PerCpu, kernel_rsp) == KERNEL_RSP);
const _: () = assert!(core::mem::offset_of!(PerCpu, user_rsp) == USER_RSP);
const _: () = assert!(core::mem::offset_of!(PerCpu, this) == 0);

impl PerCpu {
    pub const fn new(index: usize) -> PerCpu {
        PerCpu { this: core::ptr::null_mut(), kernel_rsp: 0, user_rsp: 0, index, tables: core::ptr::null_mut() }
    }
}

/// The boot CPU's: a static, because it is needed before the heap.
static mut BOOT: PerCpu = PerCpu::new(0);

pub fn boot() -> *mut PerCpu {
    &raw mut BOOT
}

/// Makes `block` the block of the CPU this runs on.
///
/// # Safety
/// In ring 0, interrupts off, after this CPU's GDT is loaded (loading
/// a segment into GS resets its base); `block` valid for as long as the
/// CPU runs, and no other CPU's.
pub unsafe fn install(block: *mut PerCpu, tables: *mut CpuTables) {
    (*block).this = block;
    (*block).tables = tables;
    write_msr(GS_BASE, block as u64);
    // What a program finds in GS the first time: nothing.
    write_msr(KERNEL_GS_BASE, 0);
}

/// The block of the CPU this runs on.
///
/// The pointer is this CPU's; a thread that could be moved to another
/// CPU between getting it and using it would be using the wrong one. So
/// far threads run on one CPU only. Callers keep interrupts off anyway.
pub fn current() -> *mut PerCpu {
    let block: *mut PerCpu;
    // SAFETY: in ring 0 GS_BASE is this CPU's block (the rule above),
    // whose first field is its own address.
    unsafe { asm!("mov {}, gs:[0]", out(reg) block, options(nostack, preserves_flags, readonly)) };
    block
}

/// The kernel's number for the CPU this runs on.
pub fn index() -> usize {
    // SAFETY: `current` is a valid block, and `index` never changes.
    unsafe { (*current()).index }
}

/// Sets the stack the next `syscall` on this CPU switches to.
pub(super) fn set_kernel_rsp(top: u64) {
    // SAFETY: written with interrupts off on a thread switch, read only
    // by syscall_entry on this same CPU.
    unsafe { (*current()).kernel_rsp = top };
}
