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
//! Three things are expected: the breakpoint that proves this path
//! works; interrupts, which are handed to `super::interrupt`; and
//! whatever a program does - a system call, which is carried out, or
//! a fault, which ends the program. Everything else is reported and
//! the kernel stops.
//!
//! # Programs
//!
//! A program runs at exception level 0 with a stack pointer of its
//! own, `SP_EL0`. The moment it takes an exception the CPU is at
//! level 1 and `sp` means a different register, `SP_EL1`: the stack
//! switch that x86 needs a TSS or a `swapgs` for is, here, the CPU
//! simply using the other register. `SP_EL1` is where the thread left
//! it when it went to the program - the top of its kernel stack
//! (`enter_user`).
//!
//! So the frame of a program records `SP_EL0`, not the kernel's stack
//! pointer, and puts it back on the way out. That is all it takes for
//! each thread to have its own: a thread only ever leaves the CPU
//! from inside the kernel (context.rs), with its program's stack
//! pointer already in its frame.

use core::arch::{asm, global_asm};

use kernel64::aarch64::{vector_name, Access, Cause, Syndrome};

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
    /// The interrupted code's stack pointer: the program's (`SP_EL0`)
    /// if it was a program, else the kernel's before the frame was
    /// pushed.
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

    // The same, for the four slots used when the kernel itself is
    // interrupted - where the stack the frame is about to go on is the
    // kernel's own, and may have run out. Every kernel stack is a
    // 64 KiB slot starting at a multiple of 64 KiB, whose lowest page
    // is not mapped. So the low 16 bits of the stack pointer say how
    // far up its slot it is, and if bits 13 to 15 are all zero it is in
    // the lowest two pages: the missing one, or the last one above it.
    // Either way there is no room left to handle anything, and the
    // frame goes on a stack kept for this instead.
    //
    // There is no register to look at the stack pointer with: all 31
    // still hold the interrupted code's values. Hence the first two
    // lines: x0 becomes the stack pointer and the stack pointer
    // becomes the sum of both - nothing is lost, as the two lines
    // after the test show by undoing it. (The condition flags are free:
    // the CPU saved the interrupted code's in SPSR_EL1.)
    .macro kernel_vector number
    .balign 0x80
    sub     sp, sp, #272
    add     sp, sp, x0
    sub     x0, sp, x0              // x0 = the stack pointer
    tst     x0, #0xe000
    b.eq    stack_overflow
    sub     x0, sp, x0              // x0 = what it was
    sub     sp, sp, x0              // and so is the stack pointer
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
    kernel_vector 4
    kernel_vector 5
    kernel_vector 6
    kernel_vector 7
    vector 8
    vector 9
    vector 10
    vector 11
    vector 12
    vector 13
    vector 14
    vector 15

    // Reached with x0 = the stack pointer that has run out (less the
    // frame) and sp = that plus the interrupted code's x0. Two
    // registers no kernel code uses are borrowed to keep them in while
    // the stack is changed.
    .balign 0x80
stack_overflow:
    msr     tpidrro_el0, x0         // the stack pointer, for the report
    sub     x0, sp, x0
    msr     tpidr_el0, x0           // the interrupted code's x0
    adrp    x0, overflow_stack_top
    add     x0, x0, :lo12:overflow_stack_top
    mov     sp, x0
    mrs     x0, tpidr_el0
    sub     sp, sp, #272
    stp     x0, x1, [sp]
    mov     x0, #16                 // not a slot: "the stack ran out"
    b       exception_common

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
    add     x3, sp, #272            // the kernel's stack pointer, before
    tst     x2, #0xf                // the level it came from: 0 is a program,
    b.ne    1f
    mrs     x3, sp_el0              // whose stack pointer is this one
1:
    stp     x2, x3, [sp, #256]

    mov     x1, sp                  // exception(number, &mut frame)
    bl      exception

    // Back to where it happened - or to wherever the handler changed
    // `elr` to. A program's first entry comes here too, with a frame
    // that was made up (`enter_user_frame`).
exception_return:
    ldp     x2, x3, [sp, #256]
    tst     x2, #0xf
    b.ne    1f
    msr     sp_el0, x3              // going to a program: its stack pointer
1:
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

    // void enter_user_frame(u64 top, u64 entry, u64 stack, u64 first, u64 second)
    //
    // Starts a program: makes up, at the very top of this thread's
    // kernel stack, the frame a program would have left there had it
    // been interrupted at its first instruction - and "returns" to
    // it. Everything in the frame is zero but the two arguments (x0,
    // x1), the entry point and the stack pointer; state 0 means level
    // 0, 64-bit, interrupts on. Whatever was on this stack is
    // abandoned: the thread never returns to it.
    .global enter_user_frame
enter_user_frame:
    msr     daifset, #0xf           // nothing may arrive half-way through
    sub     x0, x0, #272
    mov     sp, x0
    mov     x9, #0
1:
    str     xzr, [x0, x9]
    add     x9, x9, #8
    cmp     x9, #272
    b.lo    1b
    stp     x3, x4, [sp]            // x0 and x1
    str     x1, [sp, #248]          // elr
    str     x2, [sp, #264]          // sp
    b       exception_return

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

    /// See the assembly: the first entry into a program.
    pub fn enter_user_frame(top: u64, entry: u64, stack: u64, first: u64, second: u64) -> !;

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

/// The signal a program's exception means, if it means one.
fn fault_signal(syndrome: Syndrome) -> Option<u64> {
    use abi::signal::*;
    match syndrome.class() {
        // An instruction the CPU does not know, or may not run here.
        0x00 | 0x0E => Some(SIGILL),
        // A misaligned program counter or stack pointer.
        0x22 | 0x26 => Some(SIGBUS),
        // A trapped floating-point exception.
        0x2C => Some(SIGFPE),
        // An abort: an alignment fault (status 0b100001) is a bus
        // error, anything else a bad address.
        0x20 | 0x24 if syndrome.0 & 0x3F == 0b10_0001 => Some(SIGBUS),
        0x20 | 0x24 => Some(SIGSEGV),
        _ => None,
    }
}

/// AArch64's condition flags, in SPSR: N, Z, C and V. The only part of
/// it a program may set - the rest says which level it runs at and what
/// it may be interrupted by.
const SPSR_FLAGS: u64 = 0xF000_0000;

impl crate::signals::UserFrame for Frame {
    fn context(&self) -> abi::signal::Context {
        let mut context = abi::signal::Context::zeroed();
        context.registers = self.x;
        context.sp = self.sp;
        context.pc = self.elr;
        context.flags = self.spsr;
        context
    }

    fn restore(&mut self, context: &abi::signal::Context) {
        self.x = context.registers;
        self.sp = context.sp;
        self.elr = context.pc;
        // The flags only: this frame is the program's own (a system
        // call from EL0), so the rest is already what it must be.
        self.spsr = (self.spsr & !SPSR_FLAGS) | (context.flags & SPSR_FLAGS);
    }

    fn enter(&mut self, entry: u64, stack: u64, signal: u64, context: u64) {
        self.elr = entry;
        self.sp = stack;
        self.x[0] = signal;
        self.x[1] = context;
    }

    fn stack(&self) -> u64 {
        self.sp
    }
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
    if vector == STACK_RAN_OUT {
        stack_ran_out(frame);
    }
    // Slots 1, 5, 9 and 13 are interrupts.
    if vector % 4 == 1 {
        super::interrupt();
        // About to return to a program - possibly long after the
        // interrupt, if it took the CPU away. A program that never
        // makes a system call still passes here on every timer tick:
        // if it has been asked to end meanwhile, this is where it does.
        if vector == FROM_PROGRAM + 1 {
            crate::signals::on_return(frame);
        }
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

    if vector == FROM_PROGRAM {
        // `svc`: the program asks for something. The number is in x8
        // and the arguments in x0 to x5; the answer goes back in x0.
        // Every other register returns as it came. (The saved address
        // is already that of the instruction after the `svc`.)
        if syndrome.class() == Syndrome::SYSTEM_CALL {
            if frame.x[8] == abi::call::SIGNAL_RETURN {
                // Every register from the context, x0 too: nothing is
                // answered. `eret` loads them all, as it always does.
                let context = frame.x[0];
                crate::signals::return_from(frame, context);
                crate::signals::on_return(frame);
                return;
            }
            let args = [frame.x[0], frame.x[1], frame.x[2], frame.x[3], frame.x[4], frame.x[5]];
            frame.x[0] = crate::syscall::dispatch(frame.x[8], args);
            crate::signals::deliver_pending(frame);
            return;
        }
        // A read or write that found no page: reserved memory not yet
        // touched, perhaps - filled, and the instruction runs again.
        if let Some(abort) = syndrome.abort() {
            if matches!(abort.cause, Cause::NotMapped { .. }) && abort.access != Access::Execute {
                match crate::process::fault_in(fault_address(), abort.access == Access::Write) {
                    crate::process::Fault::Filled => {
                        crate::signals::on_return(frame);
                        return;
                    }
                    crate::process::Fault::NotOurs => {}
                    fault => {
                        kprintln!("[process] {abort} at {:#x}, for {:#x}", frame.elr, fault_address());
                        crate::process::out_of_memory(fault);
                    }
                }
            }
        }
        // Anything else: the program broke a rule. If it catches the
        // signal that means, its handler runs; if not, it ends. Either
        // way the kernel carries on.
        if let Some(signal) = fault_signal(syndrome) {
            if let Some(entry) = crate::process::fault_signal(signal) {
                crate::signals::deliver(frame, signal, entry);
                return;
            }
        }
        let name = syndrome.class_name();
        kprintln!("[process] {name} at {:#x} in user mode", frame.elr);
        if let Some(abort) = syndrome.abort() {
            kprintln!("[process]   {abort} at {:#x}: {}", fault_address(), abort.cause);
        }
        crate::process::kill_current(name);
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

/// The first of the four slots for a 64-bit program: its synchronous
/// exceptions. The next is its interrupts.
const FROM_PROGRAM: u64 = 8;

/// What the assembly passes instead of a slot number when the stack
/// an exception arrived on had run out.
const STACK_RAN_OUT: u64 = 16;

/// The end of a kernel stack was reached. This runs on the overflow
/// stack; the thread whose stack it was cannot go on.
fn stack_ran_out(frame: &mut Frame) -> ! {
    crate::arch::stop_other_cpus();
    // SAFETY: halting right after; whoever held the console never resumes.
    unsafe { crate::console::break_lock() };
    let stack_pointer: u64;
    // SAFETY: reading the register the assembly left the value in.
    unsafe { asm!("mrs {}, tpidrro_el0", out(reg) stack_pointer, options(nomem, nostack)) };
    // The assembly had already made room for the frame.
    frame.sp = stack_pointer + 272;
    let syndrome = syndrome();
    let address = fault_address();
    let guard = crate::mm::kernel_layout().stack_guard;
    let whose = if (guard..guard + 0x1_0000).contains(&frame.sp) { "the boot stack" } else { "a thread's stack" };
    kprintln!();
    kprintln!("*** KERNEL STACK OVERFLOW: {whose} ran out");
    // Was it an access to the page kept unmapped below a stack? (The
    // syndrome may be an older exception's, if this one was an
    // interrupt - hence the check of the address too.)
    let in_guard = (guard..guard + 0x1000).contains(&address) || crate::mm::is_stack_guard(address);
    match syndrome.abort() {
        Some(abort) if in_guard => kprintln!("    {abort} at {address:#x}: the page kept unmapped below the stack"),
        _ => kprintln!("    an exception or an interrupt arrived with less than a page of the stack left"),
    }
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
