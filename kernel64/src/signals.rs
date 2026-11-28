//! Delivering a signal: making a thread that is about to return to its
//! program return somewhere else first.
//!
//! The thread is on its way back to user mode - at the end of a system
//! call, or of an interrupt or fault that came while the program ran -
//! with the program's registers saved on its kernel stack. Delivering
//! changes those saved registers before they are loaded:
//!
//!   1. The registers as they are (a `Context`) are written onto the
//!      program's own stack, below whatever it had there.
//!   2. The saved program counter becomes the program's signal entry
//!      (libc's trampoline), the stack pointer the context's address,
//!      and the first two arguments the signal and that address.
//!
//! The thread then "returns" into the trampoline, which saves the
//! floating-point registers, runs the handler, puts them back, and asks
//! for signal_return(context): the kernel reads the context back into
//! the saved registers and returns to where the program really was.
//!
//! Nothing of this needs the kernel to remember anything per signal:
//! the context is in the program's memory, which is why handlers can
//! nest - a second signal during a handler pushes a second context
//! below the first. And it is why signal_return has to be careful: the
//! context is the program's to change, so the kernel takes from it only
//! what a program may set anyway (`UserFrame::restore`).

use abi::signal::{Context, CONTEXT_SIZE, STACK_SKIP};

use crate::process;

/// A thread's saved user registers, however its architecture and the
/// way it entered the kernel laid them out.
pub trait UserFrame {
    /// The registers, as a context (its `signal` not yet set).
    fn context(&self) -> Context;
    /// Every register from `context` - the flags only as far as a
    /// program may set them.
    fn restore(&mut self, context: &Context);
    /// Return to `entry(signal, context)`, with the stack at `stack`.
    fn enter(&mut self, entry: u64, stack: u64, signal: u64, context: u64);
    /// The program's stack pointer.
    fn stack(&self) -> u64;
}

/// On the way back to user mode: delivers a caught signal, if one is
/// waiting for the process.
pub fn deliver_pending(frame: &mut impl UserFrame) {
    if let Some((signal, entry)) = process::take_signal() {
        deliver(frame, signal, entry);
    }
}

/// Delivers `signal` to the program `frame` belongs to, at `entry`. A
/// program whose stack cannot take the context - overflowed, or not
/// memory at all - ends: there is nowhere to run its handler.
pub fn deliver(frame: &mut impl UserFrame, signal: u64, entry: u64) {
    let mut context = frame.context();
    context.signal = signal;
    // On the thread's signal stack, if the signal asks for it and the
    // thread has one it is not already on (a signal during a handler
    // there stacks below the first, as on any stack). That is how a
    // program hears of its own stack overflowing: its stack has no room
    // for the context, the signal stack has.
    let (base, size) = crate::threads::signal_stack();
    let sp = frame.stack();
    let top = if size != 0 && !(base..base + size).contains(&sp) && crate::process::signal_on_stack(signal) {
        base + size
    } else {
        sp
    };
    // 16-byte aligned, below the red zone. The stack is the program's;
    // a value that wraps is no stack at all.
    let at = top.checked_sub(STACK_SKIP + CONTEXT_SIZE as u64).map(|at| at & !15);
    match at.map(|at| (at, process::copy_to_user(at, &context.to_bytes()))) {
        Some((at, Ok(()))) => frame.enter(entry, at, signal, at),
        _ => process::kill_current("no room on its stack for a signal"),
    }
}

/// signal_return(pointer): back to where the program was when the
/// signal came, as the context at `pointer` says. A pointer to no
/// context, or one whose program counter is not the program's, ends the
/// process: there is nowhere sensible to return to.
pub fn return_from(frame: &mut impl UserFrame, pointer: u64) {
    let context = process::copy_from_user(pointer, CONTEXT_SIZE as u64).ok().and_then(|bytes| Context::from_bytes(&bytes));
    match context {
        Some(context) if context.pc < abi::USER_END => frame.restore(&context),
        _ => process::kill_current("returned from a signal to no context"),
    }
}

/// What every return to user mode does: the process ends there if it
/// was asked to; otherwise a caught signal is delivered there.
pub fn on_return(frame: &mut impl UserFrame) {
    // Stopped by a debugger (K142): here, until let go.
    crate::debug::checkpoint(frame);
    process::stop_if_killed();
    deliver_pending(frame);
}
