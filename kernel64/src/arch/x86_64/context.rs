//! Switching the CPU from one thread to another.
//!
//! A thread, to the CPU, is just its registers. Most of them need no
//! saving here: `switch_context` is an ordinary function call, and the
//! calling convention already lets it destroy the "caller-saved"
//! registers (RAX, RCX, RDX, RSI, RDI, R8-R11) - the caller saved
//! anything it needed in them. What a callee must preserve are the six
//! "callee-saved" registers and the stack pointer. So a switch is:
//!
//!   push the six onto the old thread's stack
//!   store RSP into the old thread's record
//!   load RSP from the new thread's record      <- now on the new stack
//!   pop the six the new thread pushed when it was switched out
//!   ret                                         <- into the new thread
//!
//! That `ret` returns to wherever the new thread called switch_context
//! from - the new thread's own earlier call. Every thread is "inside"
//! switch_context while it is not running.
//!
//! A brand-new thread was never inside it, so `initial_stack` fakes the
//! frame: six zeros and a return address of `thread_trampoline`.
//!
//! The floating-point and vector registers - the x87 FPU, and the
//! sixteen XMM registers with MXCSR - are the exception to "the calling
//! convention saves the rest". The kernel never touches them (it is
//! built for the soft-float target), so when a thread enters the kernel
//! its program's values are still in them, and they would leak into the
//! next thread run. So every switch also saves them, with one
//! instruction - `fxsave64` writes all of them as a 512-byte block -
//! below the six on the old stack, and `fxrstor64` loads the new
//! thread's block. A new thread's block is the state the CPU has after
//! a reset: nothing in the FPU, every exception masked.

use core::arch::{asm, global_asm};

/// The `fxsave64` block, and 8 bytes that keep it 16-byte aligned: on
/// entry to switch_context RSP is 8 past a multiple of 16 (the call
/// pushed the return address), the six pushes keep it so, and 520 more
/// make it a multiple of 16, which `fxsave64` requires.
const FPU_AREA: u64 = 520;
/// Offsets in the block, and the reset values: the x87 control word
/// (every exception masked, 64-bit precision, round to nearest) and
/// MXCSR (the same, for SSE).
const FCW: usize = 0;
const FCW_DEFAULT: u16 = 0x037F;
const MXCSR: usize = 24;
const MXCSR_DEFAULT: u32 = 0x1F80;

const CR0_MP: u64 = 1 << 1;
const CR0_EM: u64 = 1 << 2;
const CR0_TS: u64 = 1 << 3;
const CR0_NE: u64 = 1 << 5;
const CR4_OSFXSR: u64 = 1 << 9;
const CR4_OSXMMEXCPT: u64 = 1 << 10;

/// Lets programs use the FPU and SSE: CR0.EM off (the instructions run
/// instead of faulting), MP and NE on (FPU errors are reported as
/// exceptions, the modern way), TS off (no lazy switching: every switch
/// saves and loads); CR4.OSFXSR (the OS saves this state with fxsave,
/// so SSE may be used) and OSXMMEXCPT (SSE errors come as their own
/// exception, #XM). The other CPUs copy CR0 and CR4 from this one.
///
/// # Safety
/// Once, on the boot CPU, before any thread is created.
pub unsafe fn enable_fpu() {
    // SAFETY: the bits above are the documented way to turn the FPU and
    // SSE on; the kernel itself never uses them.
    unsafe {
        let (mut cr0, mut cr4): (u64, u64);
        asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack, preserves_flags));
        cr0 = (cr0 & !(CR0_EM | CR0_TS)) | CR0_MP | CR0_NE;
        asm!("mov cr0, {}", in(reg) cr0, options(nostack, preserves_flags));
        asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
        cr4 |= CR4_OSFXSR | CR4_OSXMMEXCPT;
        asm!("mov cr4, {}", in(reg) cr4, options(nostack, preserves_flags));
        asm!("fninit", options(nomem, nostack));
    }
}

global_asm!(
    ".global switch_context",
    "switch_context:", // (save: *mut u64 = rdi, load: u64 = rsi)
    "    push rbp",
    "    push rbx",
    "    push r12",
    "    push r13",
    "    push r14",
    "    push r15",
    "    sub rsp, {fpu}",
    "    fxsave64 [rsp]",
    "    mov [rdi], rsp",
    "    mov rsp, rsi",
    "    fxrstor64 [rsp]",
    "    add rsp, {fpu}",
    "    pop r15",
    "    pop r14",
    "    pop r13",
    "    pop r12",
    "    pop rbx",
    "    pop rbp",
    "    ret",
    "",
    // Where a new thread's first `ret` lands. RBP = 0 ends the chain of
    // frames for anything walking the stack; the ABI wants RSP 16-byte
    // aligned at a call.
    ".global thread_trampoline",
    "thread_trampoline:",
    "    xor ebp, ebp",
    "    and rsp, -16",
    "    call {entry}",
    "    ud2", // thread_entry never returns
    entry = sym crate::threads::thread_entry,
    fpu = const FPU_AREA,
);

extern "C" {
    fn switch_context(save: *mut u64, load: u64);
    static thread_trampoline: u8;
}

/// Saves the current thread's stack pointer to `*save` and resumes the
/// thread whose stack pointer is `load`. Returns when something switches
/// back to this thread.
///
/// # Safety
/// Interrupts disabled; `save` valid for writing; `load` a stack pointer
/// saved by this function or made by `initial_stack`, for a thread that
/// is not running.
pub unsafe fn switch(save: *mut u64, load: u64) {
    switch_context(save, load);
}

/// Prepares a fresh stack ending at `top` so that switching to it starts
/// `thread_trampoline`. Returns the stack pointer to switch to.
///
/// From the top down: the return address (where a real call would have
/// left it, 8 below a multiple of 16), the six registers, and the FPU
/// block at a multiple of 16 - exactly what switch_context leaves.
///
/// # Safety
/// `top` must be the 16-byte aligned end of a mapped, writable stack.
pub unsafe fn initial_stack(top: u64) -> u64 {
    let stack = top as *mut u64;
    let fpu = top - 8 * 7 - FPU_AREA;
    // SAFETY: everything written is within the top 576 bytes of the
    // stack the caller vouches for.
    unsafe {
        stack.sub(1).write(&raw const thread_trampoline as u64); // ret target
        for slot in 2..=7 {
            stack.sub(slot).write(0); // rbp, rbx, r12-r15
        }
        core::ptr::write_bytes(fpu as *mut u8, 0, FPU_AREA as usize);
        (fpu as *mut u8).add(FCW).cast::<u16>().write(FCW_DEFAULT);
        (fpu as *mut u8).add(MXCSR).cast::<u32>().write(MXCSR_DEFAULT);
    }
    fpu
}
