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
//! Floating-point registers are not saved: the kernel is built not to
//! use them (the "softfloat" target). When programs arrive, theirs
//! will have to be.

use core::arch::global_asm;

global_asm!(
    r#"
    .section .text
    .global switch_context
switch_context:                     // (save: *mut u64 = x0, load: u64 = x1)
    sub     sp, sp, #96
    stp     x19, x20, [sp]
    stp     x21, x22, [sp, #16]
    stp     x23, x24, [sp, #32]
    stp     x25, x26, [sp, #48]
    stp     x27, x28, [sp, #64]
    stp     x29, x30, [sp, #80]
    mov     x9, sp
    str     x9, [x0]
    mov     sp, x1
    ldp     x19, x20, [sp]
    ldp     x21, x22, [sp, #16]
    ldp     x23, x24, [sp, #32]
    ldp     x25, x26, [sp, #48]
    ldp     x27, x28, [sp, #64]
    ldp     x29, x30, [sp, #80]
    add     sp, sp, #96
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
    sub     x9, x0, #96
    add     x10, sp, #96
    str     x10, [x9]
    adr     x10, 3f
    str     x10, [x9, #88]
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
    let frame = (top - 96) as *mut u64;
    // SAFETY: the twelve slots are the top 96 bytes of the stack the
    // caller vouches for.
    unsafe {
        for slot in 0..11 {
            frame.add(slot).write(0); // x19 to x29
        }
        frame.add(11).write(&raw const thread_trampoline as u64); // x30
    }
    top - 96
}
