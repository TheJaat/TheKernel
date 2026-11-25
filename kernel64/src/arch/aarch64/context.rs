//! Switching the CPU from one thread to another.
//!
//! The idea is the PC's (arch/x86_64/context.rs): a switch is an
//! ordinary function call, so the calling convention has already made
//! the caller save every register it is allowed to lose. What a
//! function must hand back unchanged on AArch64 are x19 to x28, the
//! frame pointer x29, the stack pointer - and x30, the "link
//! register", which is where `ret` goes. x86 keeps the return address
//! on the stack, pushed by `call`; AArch64 keeps it in a register, so
//! here it is saved and loaded like the others, and loading it is what
//! makes the final `ret` land in the *new* thread.
//!
//!   make room for twelve registers on the old thread's stack; store them
//!   store SP into the old thread's record
//!   load SP from the new thread's record        <- now on the new stack
//!   load the twelve the new thread stored when it was switched out
//!   ret                                          <- to the new thread's x30
//!
//! A brand-new thread was never switched out, so `initial_stack` fakes
//! what it would have stored: eleven zeroes and an x30 of
//! `thread_trampoline`.
//!
//! The floating-point and vector registers - v0 to v31, 128 bits each,
//! with FPCR (rounding, which exceptions trap) and FPSR (what has
//! happened) - are saved too, after the twelve: the kernel never uses
//! them (it is built for the "softfloat" target), so a program's values
//! are still in them when its thread enters the kernel, and the next
//! thread must not see them. A new thread starts with all of them zero,
//! which is the reset state: round to nearest, nothing trapping.

use core::arch::{asm, global_asm};

/// The frame: the twelve general registers (96 bytes), v0 to v31 (512),
/// FPCR and FPSR (16).
const FRAME: u64 = 96 + 512 + 16;

/// CPACR_EL1.FPEN, both bits: FP and SIMD instructions run at EL0 and
/// EL1 instead of trapping.
const CPACR_FPEN: u64 = 0b11 << 20;

/// Lets programs use the FP and SIMD registers. The other CPUs copy
/// CPACR_EL1 from this one as they start.
///
/// # Safety
/// Once, on the boot CPU, before any thread is created.
pub unsafe fn enable_fpu() {
    // SAFETY: FPEN only stops FP instructions trapping; the kernel uses
    // none, and the ISB makes the change take effect before anything
    // after it.
    unsafe {
        let mut cpacr: u64;
        asm!("mrs {}, cpacr_el1", out(reg) cpacr, options(nomem, nostack, preserves_flags));
        cpacr |= CPACR_FPEN;
        asm!("msr cpacr_el1, {}", "isb", in(reg) cpacr, options(nostack, preserves_flags));
    }
}

global_asm!(
    r#"
    .section .text
    // The kernel's target has no FP instructions; these lines are the
    // only ones that use them, so they are allowed here alone.
    .arch_extension fp
    .arch_extension simd
    .global switch_context
switch_context:                     // (save: *mut u64 = x0, load: u64 = x1)
    sub     sp, sp, #{frame}
    stp     x19, x20, [sp]
    stp     x21, x22, [sp, #16]
    stp     x23, x24, [sp, #32]
    stp     x25, x26, [sp, #48]
    stp     x27, x28, [sp, #64]
    stp     x29, x30, [sp, #80]
    stp     q0, q1, [sp, #96]
    stp     q2, q3, [sp, #128]
    stp     q4, q5, [sp, #160]
    stp     q6, q7, [sp, #192]
    stp     q8, q9, [sp, #224]
    stp     q10, q11, [sp, #256]
    stp     q12, q13, [sp, #288]
    stp     q14, q15, [sp, #320]
    stp     q16, q17, [sp, #352]
    stp     q18, q19, [sp, #384]
    stp     q20, q21, [sp, #416]
    stp     q22, q23, [sp, #448]
    stp     q24, q25, [sp, #480]
    stp     q26, q27, [sp, #512]
    stp     q28, q29, [sp, #544]
    stp     q30, q31, [sp, #576]
    mrs     x9, fpcr
    mrs     x10, fpsr
    str     x9, [sp, #608]          // beyond the reach of stp for x registers
    str     x10, [sp, #616]
    mov     x9, sp
    str     x9, [x0]
    mov     sp, x1
    ldp     x19, x20, [sp]
    ldp     x21, x22, [sp, #16]
    ldp     x23, x24, [sp, #32]
    ldp     x25, x26, [sp, #48]
    ldp     x27, x28, [sp, #64]
    ldp     x29, x30, [sp, #80]
    ldp     q0, q1, [sp, #96]
    ldp     q2, q3, [sp, #128]
    ldp     q4, q5, [sp, #160]
    ldp     q6, q7, [sp, #192]
    ldp     q8, q9, [sp, #224]
    ldp     q10, q11, [sp, #256]
    ldp     q12, q13, [sp, #288]
    ldp     q14, q15, [sp, #320]
    ldp     q16, q17, [sp, #352]
    ldp     q18, q19, [sp, #384]
    ldp     q20, q21, [sp, #416]
    ldp     q22, q23, [sp, #448]
    ldp     q24, q25, [sp, #480]
    ldp     q26, q27, [sp, #512]
    ldp     q28, q29, [sp, #544]
    ldp     q30, q31, [sp, #576]
    ldr     x9, [sp, #608]
    ldr     x10, [sp, #616]
    msr     fpcr, x9
    msr     fpsr, x10
    add     sp, sp, #{frame}
    ret

    // Where a new thread's first `ret` lands. No frame pointer and no
    // return address: a walk up the stack ends here.
    .global thread_trampoline
thread_trampoline:
    mov     x29, xzr
    mov     x30, xzr
    bl      {entry}
    brk     #1                      // thread_entry never returns

    // u64 switch_round_trip(u64 other_stack_top)
    //
    // Checks switch_context itself, with nothing of the scheduler in
    // the way. Every register a switch must keep gets a known number
    // (register n gets 0xA00 + n); then the CPU is switched to a second
    // context on the stack given, which puts other numbers in all of
    // them and switches straight back. Returns how many registers do
    // not hold their number afterwards.
    //
    // The thread tests cannot see this: between them and the switch
    // are Rust functions that save and restore, as the calling
    // convention asks, whichever of these registers they use - and so
    // put right, by accident, what a broken switch got wrong.
    //
    // Interrupts must be off: the second context is nobody's thread.
    .global switch_round_trip
switch_round_trip:
    stp     x29, x30, [sp, #-112]!
    stp     x19, x20, [sp, #16]
    stp     x21, x22, [sp, #32]
    stp     x23, x24, [sp, #48]
    stp     x25, x26, [sp, #64]
    stp     x27, x28, [sp, #80]
    // [sp, #96]: this context's saved stack pointer; [sp, #104]: the other's.
    // The other context, as initial_stack would make it: twelve
    // saved registers, of which x19 is a pointer to those two slots
    // and x30 where it starts.
    sub     x9, x0, #{frame}
    add     x10, sp, #96
    str     x10, [x9]
    adr     x10, 3f
    str     x10, [x9, #88]
    // Its FP registers, FPCR and FPSR: zero, as initial_stack makes them.
    add     x11, x9, #96
    mov     x12, #(({frame} - 96) / 16)
4:  stp     xzr, xzr, [x11], #16
    subs    x12, x12, #1
    b.ne    4b
    .irp    n, 19,20,21,22,23,24,25,26,27,28,29
    mov     x\n, #(0xA00 + \n)
    .endr
    add     x0, sp, #96
    mov     x1, x9
    bl      switch_context          // comes back when the other switches here
    mov     x0, #0
    .irp    n, 19,20,21,22,23,24,25,26,27,28,29
    cmp     x\n, #(0xA00 + \n)
    cinc    x0, x0, ne
    .endr
    ldp     x27, x28, [sp, #80]
    ldp     x25, x26, [sp, #64]
    ldp     x23, x24, [sp, #48]
    ldp     x21, x22, [sp, #32]
    ldp     x19, x20, [sp, #16]
    ldp     x29, x30, [sp], #112
    ret
3:  // The other context: x19 = the pointer to the two slots.
    add     x0, x19, #8
    ldr     x1, [x19]
    .irp    n, 19,20,21,22,23,24,25,26,27,28,29
    mov     x\n, #(0xB00 + \n)
    .endr
    bl      switch_context
    brk     #2                      // nothing ever switches back here
"#,
    entry = sym crate::threads::thread_entry,
    frame = const FRAME,
);

extern "C" {
    fn switch_context(save: *mut u64, load: u64);
    fn switch_round_trip(other_stack_top: u64) -> u64;
    static thread_trampoline: u8;
}

/// Switches to a second context on the stack ending at
/// `other_stack_top` and back, with a known value in each of the
/// eleven registers a switch must keep, and returns how many came back
/// different.
///
/// # Safety
/// Interrupts disabled; `other_stack_top` the 16-byte aligned end of a
/// mapped, writable stack that nothing else uses.
pub unsafe fn round_trip(other_stack_top: u64) -> u64 {
    // SAFETY: the caller's promises are the function's conditions.
    unsafe { switch_round_trip(other_stack_top) }
}

/// Saves the current thread's stack pointer to `*save` and resumes the
/// thread whose stack pointer is `load`. Returns when something
/// switches back to this thread.
///
/// # Safety
/// Interrupts disabled; `save` valid for writing; `load` a stack
/// pointer saved by this function or made by `initial_stack`, for a
/// thread that is not running.
pub unsafe fn switch(save: *mut u64, load: u64) {
    // SAFETY: the caller's promises are the function's conditions.
    unsafe { switch_context(save, load) };
}

/// Prepares a fresh stack ending at `top` so that switching to it
/// starts `thread_trampoline`. Returns the stack pointer to switch to.
///
/// # Safety
/// `top` must be the 16-byte aligned end of a mapped, writable stack.
pub unsafe fn initial_stack(top: u64) -> u64 {
    let frame = (top - FRAME) as *mut u64;
    // SAFETY: the frame is the top FRAME bytes of the stack the caller
    // vouches for.
    unsafe {
        core::ptr::write_bytes(frame, 0, FRAME as usize / 8); // x19-x29, v0-v31, FPCR, FPSR
        frame.add(11).write(&raw const thread_trampoline as u64); // x30
    }
    top - FRAME
}

/// The running program's thread pointer: TPIDR_EL0, which a program
/// may write itself, so it is read back when its thread leaves the CPU.
/// (Only the kernel's stack-overflow report borrows it, and that report
/// is the kernel's last act.)
pub fn save_thread_pointer() -> Option<u64> {
    let value: u64;
    // SAFETY: reading a register that holds only the program's value.
    unsafe { asm!("mrs {}, tpidr_el0", out(reg) value, options(nomem, nostack, preserves_flags)) };
    Some(value)
}

/// Loads a thread's pointer into TPIDR_EL0, for when it runs at EL0.
///
/// # Safety
/// Only a program may rely on TPIDR_EL0: nothing of the kernel's may.
pub unsafe fn load_thread_pointer(value: u64) {
    asm!("msr tpidr_el0, {}", in(reg) value, options(nomem, nostack, preserves_flags));
}
