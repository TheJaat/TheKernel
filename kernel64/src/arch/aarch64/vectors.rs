//! The exception vector table, and what happens when the CPU takes an
//! exception.
//!
//! "Exception" on AArch64 covers everything that makes the CPU stop
//! what it is doing and enter the kernel: a fault, a system call, an
//! interrupt from a device. When one happens the CPU
//!
//!   - saves where it was in `ELR_EL1` and its state in `SPSR_EL1`,
//!   - says why in `ESR_EL1` (and, for a bad address, which in
//!     `FAR_EL1`),
//!   - switches interrupts off, and
//!   - jumps into the table `VBAR_EL1` points at.
//!
//! The table is not a list of addresses, as the IDT on x86 is. It is
//! code: sixteen slots of 128 bytes each, and the CPU jumps to the
//! start of the slot for what happened - four kinds (a synchronous
//! exception, an interrupt, a fast interrupt, a system error) from each
//! of four places (the kernel using the program stack pointer, the
//! kernel, a 64-bit program, a 32-bit program).
//!
//! The CPU saves nothing else. No register is pushed anywhere - there
//! is not even a stack switch, as there is none to switch from yet. So
//! each slot makes room on the stack, saves two registers to work
//! with, notes which slot it is, and joins the common code, which saves
//! the rest into a `Frame` and calls `exception` below.
//!
//! Two things are expected: the breakpoint that proves this path
//! works, and interrupts, which are handed to `super::interrupt`.
//! Everything else is reported and the kernel stops.

use core::arch::{asm, global_asm};

use kernel64::aarch64::{vector_name, Syndrome};

/// Everything the interrupted code had in the CPU. The assembly below
/// fills it in this order.
#[repr(C)]
pub struct Frame {
    /// x0 to x30. x29 is the frame pointer and x30 the "link register":
    /// where the function running will return to.
    pub x: [u64; 31],
    /// Where the exception was taken: the instruction to return to.
    pub elr: u64,
    /// The CPU's state then: level, interrupt masks, condition flags.
    pub spsr: u64,
    /// The stack pointer before the frame was pushed.
    pub sp: u64,
}

// The assembly knows the frame as the number 272.
const _: () = assert!(core::mem::size_of::<Frame>() == 272);

global_asm!(
    r#"
    .macro vector number
    .balign 0x80
    sub     sp, sp, #272
    stp     x0, x1, [sp]
    mov     x0, #\number
    b       exception_common
    .endm

    .section .text
    .balign 0x800
    .global exception_vectors
exception_vectors:
    vector 0
    vector 1
    vector 2
    vector 3
    vector 4
    vector 5
    vector 6
    vector 7
    vector 8
    vector 9
    vector 10
    vector 11
    vector 12
    vector 13
    vector 14
    vector 15

    .balign 0x80
exception_common:
    stp     x2, x3, [sp, #16]
    stp     x4, x5, [sp, #32]
    stp     x6, x7, [sp, #48]
    stp     x8, x9, [sp, #64]
    stp     x10, x11, [sp, #80]
    stp     x12, x13, [sp, #96]
    stp     x14, x15, [sp, #112]
    stp     x16, x17, [sp, #128]
    stp     x18, x19, [sp, #144]
    stp     x20, x21, [sp, #160]
    stp     x22, x23, [sp, #176]
    stp     x24, x25, [sp, #192]
    stp     x26, x27, [sp, #208]
    stp     x28, x29, [sp, #224]
    mrs     x1, elr_el1
    mrs     x2, spsr_el1
    stp     x30, x1, [sp, #240]
    add     x3, sp, #272
    stp     x2, x3, [sp, #256]

    mov     x1, sp                  // exception(number, &mut frame)
    bl      exception

    // Back to where it happened - or to wherever the handler changed
    // `elr` to.
    ldp     x2, x3, [sp, #256]
    ldp     x30, x1, [sp, #240]
    msr     spsr_el1, x2
    msr     elr_el1, x1
    ldp     x28, x29, [sp, #224]
    ldp     x26, x27, [sp, #208]
    ldp     x24, x25, [sp, #192]
    ldp     x22, x23, [sp, #176]
    ldp     x20, x21, [sp, #160]
    ldp     x18, x19, [sp, #144]
    ldp     x16, x17, [sp, #128]
    ldp     x14, x15, [sp, #112]
    ldp     x12, x13, [sp, #96]
    ldp     x10, x11, [sp, #80]
    ldp     x8, x9, [sp, #64]
    ldp     x6, x7, [sp, #48]
    ldp     x4, x5, [sp, #32]
    ldp     x2, x3, [sp, #16]
    ldp     x0, x1, [sp]
    add     sp, sp, #272
    eret

    // u64 exception_round_trip(void)
    //
    // Puts a different number in every register (register n gets
    // 0x700 + n), takes a breakpoint exception, and counts the
    // registers that do not hold their number afterwards. A handler
    // that saved or restored one register at the wrong place in the
    // frame would otherwise go unnoticed until the day that register
    // mattered.
    //
    // x19 to x30 belong to the caller by the calling convention, so
    // they are kept on the stack meanwhile and put back.
    .global exception_round_trip
exception_round_trip:
    stp     x29, x30, [sp, #-96]!
    stp     x19, x20, [sp, #16]
    stp     x21, x22, [sp, #32]
    stp     x23, x24, [sp, #48]
    stp     x25, x26, [sp, #64]
    stp     x27, x28, [sp, #80]
    .irp    n, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30
    mov     x\n, #(0x700 + \n)
    .endr
    brk     #0
    cmp     x0, #0x700
    cset    x0, ne                  // from here x0 is the count
    .irp    n, 1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30
    cmp     x\n, #(0x700 + \n)
    cinc    x0, x0, ne
    .endr
    ldp     x27, x28, [sp, #80]
    ldp     x25, x26, [sp, #64]
    ldp     x23, x24, [sp, #48]
    ldp     x21, x22, [sp, #32]
    ldp     x19, x20, [sp, #16]
    ldp     x29, x30, [sp], #96
    ret

    // u64 interrupt_round_trip(const u64 *ticks, u64 wait)
    //
    // The same idea for interrupts: register n gets 0x900 + n, and
    // this spins until *ticks has grown by `wait` - so that many
    // interrupts were taken somewhere in the loop - then counts the
    // registers that changed. x0, x1 and x2 are the loop's own; the
    // other 28 are checked.
    .global interrupt_round_trip
interrupt_round_trip:
    stp     x29, x30, [sp, #-96]!
    stp     x19, x20, [sp, #16]
    stp     x21, x22, [sp, #32]
    stp     x23, x24, [sp, #48]
    stp     x25, x26, [sp, #64]
    stp     x27, x28, [sp, #80]
    ldr     x2, [x0]
    add     x1, x2, x1              // the tick count to wait for
    .irp    n, 3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30
    mov     x\n, #(0x900 + \n)
    .endr
2:
    ldr     x2, [x0]
    cmp     x2, x1
    b.lo    2b
    mov     x0, #0
    .irp    n, 3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30
    cmp     x\n, #(0x900 + \n)
    cinc    x0, x0, ne
    .endr
    ldp     x27, x28, [sp, #80]
    ldp     x25, x26, [sp, #64]
    ldp     x23, x24, [sp, #48]
    ldp     x21, x22, [sp, #32]
    ldp     x19, x20, [sp, #16]
    ldp     x29, x30, [sp], #96
    ret
"#
);

extern "C" {
    /// The table above. Only its address is used.
    static exception_vectors: u8;

    /// See the assembly: how many registers changed while interrupts
    /// came and went.
    fn interrupt_round_trip(ticks: *const u64, wait: u64) -> u64;

    /// See the assembly: how many registers an exception changed.
    fn exception_round_trip() -> u64;
}

/// Takes a breakpoint exception with a known value in every register,
/// and returns how many came back different: 0 if the path through the
/// vectors and back is sound.
pub fn round_trip() -> u64 {
    // SAFETY: the function keeps to the calling convention, and the
    // vectors are loaded (`init` comes first), so the breakpoint
    // returns.
    unsafe { exception_round_trip() }
}

/// Waits for `wait` more timer ticks with a known value in 28
/// registers, and returns how many hold something else afterwards.
/// The breakpoint check is taken at one chosen instruction; an
/// interrupt arrives wherever the CPU happens to be.
pub fn interrupted_round_trip(ticks: &core::sync::atomic::AtomicU64, wait: u64) -> u64 {
    // SAFETY: the function keeps to the calling convention and only
    // reads the counter, which outlives it; interrupts are on (the
    // caller's business - or this never returns).
    unsafe { interrupt_round_trip(ticks.as_ptr(), wait) }
}

/// Points this CPU at the vector table.
///
/// # Safety
/// Once per CPU, before anything can raise an exception on purpose.
pub unsafe fn init() {
    let table = &raw const exception_vectors;
    // SAFETY: the table is in the kernel's text, mapped and complete;
    // `isb` makes the new value apply to the very next instruction.
    // The pointer is only a number to put in the register: the
    // instruction reads nothing through it.
    unsafe { asm!("msr vbar_el1, {}", "isb", in(reg) table as u64, options(nomem, nostack)) };
}

fn syndrome() -> Syndrome {
    let value: u64;
    // SAFETY: reading a register that only describes the last exception.
    unsafe { asm!("mrs {}, esr_el1", out(reg) value, options(nomem, nostack)) };
    Syndrome(value)
}

/// The address a faulting access was for (`FAR_EL1`).
fn fault_address() -> u64 {
    let value: u64;
    // SAFETY: as for `syndrome`.
    unsafe { asm!("mrs {}, far_el1", out(reg) value, options(nomem, nostack)) };
    value
}

/// Called by the assembly above for every exception, with the number
/// of the vector slot (0 to 15) and what was saved.
#[no_mangle]
extern "C" fn exception(vector: u64, frame: &mut Frame) {
    // Slots 1, 5, 9 and 13 are interrupts.
    if vector % 4 == 1 {
        super::interrupt();
        return;
    }

    let syndrome = syndrome();
    let synchronous = vector.is_multiple_of(4);

    if synchronous && syndrome.class() == Syndrome::BREAKPOINT {
        kprintln!("[cpu] breakpoint at {:#x} - continuing", frame.elr);
        // On AArch64 the saved address is that of the `brk` itself, not
        // of the instruction after it as on x86. Returning there would
        // take the same exception for ever. Every instruction is four
        // bytes, so the next one is easy to find.
        frame.elr += 4;
        return;
    }

    // Anything else: the kernel cannot go on. Say everything, and stop.
    crate::arch::stop_other_cpus();
    // SAFETY: halting right after; whoever held the console never resumes.
    unsafe { crate::console::break_lock() };
    let (kind, source) = vector_name(vector);
    kprintln!();
    if synchronous {
        kprintln!("*** CPU EXCEPTION {:#04x}: {}", syndrome.class(), syndrome.class_name());
        if let Some(abort) = syndrome.abort() {
            kprintln!("    {abort} at {:#x}: {}", fault_address(), abort.cause);
        }
    } else {
        kprintln!("*** CPU EXCEPTION: an unexpected {kind}");
    }
    kprintln!("    from {source} (vector {vector}), syndrome {:#010x}", syndrome.0);
    dump(frame);
    crate::arch::halt_forever()
}

fn dump(f: &Frame) {
    kprintln!("    pc  {:#018x}  state {:#010x}  sp  {:#018x}", f.elr, f.spsr, f.sp);
    for row in 0..10 {
        let n = row * 3;
        kprintln!(
            "    x{n:<2} {:#018x}  x{:<2} {:#018x}  x{:<2} {:#018x}",
            f.x[n],
            n + 1,
            f.x[n + 1],
            n + 2,
            f.x[n + 2]
        );
    }
    kprintln!("    x30 {:#018x}  (the return address)", f.x[30]);
}
