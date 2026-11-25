//! Entering and leaving user mode.
//!
//! Into ring 3, the first time: `enter_user` builds the frame an
//! interrupt return expects - the user's SS, RSP, RFLAGS, CS, RIP - and
//! executes `iretq`, which "returns" to a place the thread has never been.
//!
//! Back into the kernel, deliberately: the `syscall` instruction. It is
//! fast because it does very little: RIP goes to RCX, RFLAGS to R11, CS
//! and SS come from the STAR register, RIP from LSTAR, and the flags in
//! FMASK are cleared (interrupts among them). It does NOT switch stacks -
//! RSP is still the user's, which the kernel must never trust or use. So
//! the entry stub's first act is to swap in the thread's kernel stack -
//! which it finds in this CPU's own block, through GS (percpu.rs), since
//! every other register still holds something of the program's.
//!
//! Back to the user: `sysretq`, the mirror image - RCX to RIP, R11 to
//! RFLAGS, user CS and SS from STAR. Which leaves no way to give the
//! program back an RCX or R11 of its own - and that is just what the
//! end of a signal handler must do (signal_return: every register as it
//! was when the signal came, possibly in the middle of code that was
//! using both). So there is a second way out: through `iretq`, as from
//! an interrupt, which loads RIP, RFLAGS and RSP from memory and leaves
//! every general register free. The handler says which (`Exit`).

use core::arch::{asm, global_asm};

use super::gdt::{KERNEL_CODE, USER_CODE, USER_DATA};
use super::mmu::{read_msr, write_msr, EFER};
use super::percpu;

const STAR: u32 = 0xC000_0081;
const LSTAR: u32 = 0xC000_0082;
const FMASK: u32 = 0xC000_0084;
const EFER_SCE: u64 = 1 << 0; // system call extensions: syscall/sysret on

/// RFLAGS bits cleared on entry: TF (trap), IF (interrupts), DF
/// (direction - the ABI wants it clear), NT, AC (alignment checks).
const ENTRY_CLEARED_FLAGS: u64 = 0x100 | 0x200 | 0x400 | 0x4000 | 0x40000;

/// The user's registers, as `syscall_entry` saves them (lowest address
/// first - the reverse of the pushes).
///
/// `r11` and `rcx` start as what `syscall` left in them - RFLAGS and
/// RIP - and are only used on the way out through `iretq`, which can
/// return different ones.
#[repr(C)]
pub struct SyscallFrame {
    pub r11: u64,
    pub rcx: u64,
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rbx: u64,
    pub rax: u64,
    pub rip: u64,    // from RCX
    pub rflags: u64, // from R11
    pub rsp: u64,
}

global_asm!(
    ".global syscall_entry",
    "syscall_entry:",
    // From the program's GS to this CPU's block. Interrupts are off
    // (FMASK), so nothing can come between this and the kernel stack.
    "    swapgs",
    "    mov gs:[{user_rsp}], rsp",
    "    mov rsp, gs:[{kernel_rsp}]",
    "    push qword ptr gs:[{user_rsp}]",
    "    push r11",
    "    push rcx",
    "    push rax",
    "    push rbx",
    "    push rdx",
    "    push rsi",
    "    push rdi",
    "    push rbp",
    "    push r8",
    "    push r9",
    "    push r10",
    "    push r12",
    "    push r13",
    "    push r14",
    "    push r15",
    "    push rcx",
    "    push r11",
    // 18 pushes: RSP is still 16-byte aligned for the call.
    "    mov rdi, rsp",
    "    call {handler}",
    // Interrupts off before RSP becomes the user's again: an interrupt
    // taken in ring 0 would push onto whatever RSP holds.
    "    cli",
    "    test eax, eax",
    "    jnz 2f",
    // The usual way: sysretq, RIP and RFLAGS through RCX and R11.
    "    add rsp, 16",
    "    pop r15",
    "    pop r14",
    "    pop r13",
    "    pop r12",
    "    pop r10",
    "    pop r9",
    "    pop r8",
    "    pop rbp",
    "    pop rdi",
    "    pop rsi",
    "    pop rdx",
    "    pop rbx",
    "    pop rax",
    "    pop rcx",
    "    pop r11",
    "    pop rsp",
    // The last thing: the program's GS back.
    "    swapgs",
    "    sysretq",
    // The other way: every general register from the frame, then RIP,
    // RFLAGS and RSP by `iretq`. Its five words are pushed below the
    // frame's last three - RIP, RFLAGS, RSP - with `push` from memory,
    // which needs no register: an address based on RSP is worked out
    // before the push moves RSP. Each push moves RSP 8 lower, so each
    // word is 8 further from it than the one before was.
    "2:",
    "    pop r11",
    "    pop rcx",
    "    pop r15",
    "    pop r14",
    "    pop r13",
    "    pop r12",
    "    pop r10",
    "    pop r9",
    "    pop r8",
    "    pop rbp",
    "    pop rdi",
    "    pop rsi",
    "    pop rdx",
    "    pop rbx",
    "    pop rax",
    // RSP points at the saved RIP; RFLAGS is 8 above, RSP 16 above.
    "    push {ss}",
    "    push qword ptr [rsp + 24]", // the saved RSP
    "    push qword ptr [rsp + 24]", // the saved RFLAGS
    "    push {cs}",
    "    push qword ptr [rsp + 32]", // the saved RIP
    "    swapgs",
    "    iretq",
    user_rsp = const percpu::USER_RSP,
    kernel_rsp = const percpu::KERNEL_RSP,
    handler = sym syscall_handler,
    ss = const USER_DATA as u64,
    cs = const USER_CODE as u64,
);

extern "C" {
    fn syscall_entry();
}

/// RFLAGS a user may have: the arithmetic flags, direction, and
/// alignment check. Nothing that changes privilege (IOPL) or traps.
const USER_FLAGS: u64 = 0x8D5 | 0x400 | 0x40000; // CF PF AF ZF SF OF, DF, AC
const INTERRUPTS_ON: u64 = 0x200;

/// Which way `syscall_entry` returns: what `syscall_handler` returns.
#[repr(u32)]
enum Exit {
    Sysret = 0,
    Iret = 1,
}

extern "C" fn syscall_handler(frame: &mut SyscallFrame) -> u32 {
    let exit = if frame.rax == abi::call::SIGNAL_RETURN {
        // Every register from the context; then, as below, the end of a
        // system call - where another signal may be waiting.
        let context = frame.rdi;
        crate::signals::return_from(frame, context);
        crate::signals::on_return(frame);
        Exit::Iret
    } else {
        let args = [frame.rdi, frame.rsi, frame.rdx, frame.r10, frame.r8, frame.r9];
        frame.rax = crate::syscall::dispatch(frame.rax, args);
        crate::signals::deliver_pending(frame);
        Exit::Sysret
    };

    // `sysretq` to a non-canonical address faults - in ring 0, but on the
    // user's stack (a known x86 hazard). Never attempt it; nor `iretq`.
    if frame.rip >= abi::USER_END {
        crate::process::kill_current("returned to a kernel or non-canonical address");
    }
    frame.rflags = (frame.rflags & USER_FLAGS) | INTERRUPTS_ON;
    exit as u32
}

/// The flags a program's context may give it: its own, and interrupts
/// on (bit 1 always reads as 1).
pub fn user_flags(flags: u64) -> u64 {
    (flags & USER_FLAGS) | INTERRUPTS_ON | 0x2
}

impl crate::signals::UserFrame for SyscallFrame {
    fn context(&self) -> abi::signal::Context {
        let mut context = abi::signal::Context::zeroed();
        let f = self;
        let registers = [f.rax, f.rbx, f.rcx, f.rdx, f.rsi, f.rdi, f.rbp, f.r8, f.r9, f.r10, f.r11, f.r12, f.r13, f.r14, f.r15];
        context.registers[..15].copy_from_slice(&registers);
        context.sp = f.rsp;
        context.pc = f.rip;
        context.flags = f.rflags;
        context
    }

    fn restore(&mut self, context: &abi::signal::Context) {
        let r = &context.registers;
        (self.rax, self.rbx, self.rcx, self.rdx, self.rsi, self.rdi, self.rbp) = (r[0], r[1], r[2], r[3], r[4], r[5], r[6]);
        (self.r8, self.r9, self.r10, self.r11, self.r12, self.r13, self.r14, self.r15) = (r[7], r[8], r[9], r[10], r[11], r[12], r[13], r[14]);
        self.rsp = context.sp;
        self.rip = context.pc;
        self.rflags = user_flags(context.flags);
    }

    fn enter(&mut self, entry: u64, stack: u64, signal: u64, context: u64) {
        self.rip = entry;
        self.rsp = stack;
        self.rdi = signal;
        self.rsi = context;
    }

    fn stack(&self) -> u64 {
        self.rsp
    }
}

/// How far below the top of its stack a program's stack pointer
/// starts: RSP as if `_start` had just been called - 16-byte aligned,
/// minus the return address a call would have pushed.
pub const ENTRY_STACK_GAP: u64 = 8;

/// Turns on `syscall`/`sysret`.
///
/// # Safety
/// Once on each CPU, after its GDT is loaded: the registers written
/// here are the CPU's own.
pub unsafe fn init() {
    write_msr(EFER, read_msr(EFER) | EFER_SCE);
    // STAR[47:32]: the kernel CS (SS is CS + 8). STAR[63:48]: a base from
    // which sysret takes user SS = base + 8 and user CS = base + 16. The
    // GDT order (user data, then user code) exists to fit this.
    let user_base = u64::from(USER_DATA) - 8;
    write_msr(STAR, (user_base << 48) | (u64::from(KERNEL_CODE) << 32));
    write_msr(LSTAR, syscall_entry as *const () as u64);
    write_msr(FMASK, ENTRY_CLEARED_FLAGS);
    const _: () = assert!(USER_DATA + 8 == USER_CODE, "sysret needs user code right after user data");
}

/// The kernel stack for the next `syscall` and for interrupts from ring 3.
/// On the CPU this runs on.
pub fn set_kernel_stack(top: u64) {
    super::gdt::set_kernel_stack(top);
    percpu::set_kernel_rsp(top);
}

/// Drops to ring 3 at `entry`, with stack pointer `stack` and two
/// arguments for `_start` in RDI and RSI. Every other general register
/// is zeroed first, so nothing of the kernel's leaks to the program.
///
/// # Safety
/// The current address space must map `entry` and the stack for user
/// mode, and `set_kernel_stack` must name this thread's kernel stack.
pub unsafe fn enter_user(entry: u64, stack: u64, first: u64, second: u64) -> ! {
    asm!(
        // Interrupts off until the `iretq` turns them on: after the
        // `swapgs` this is ring 0 with the program's GS, and an
        // interrupt taken then would run the kernel with it.
        "cli",
        "push {ss}",
        "push {rsp}",
        "push {rflags}",
        "push {cs}",
        "push {rip}",
        "xor eax, eax",
        "xor ebx, ebx",
        "xor ecx, ecx",
        "xor edx, edx",
        "xor ebp, ebp",
        "xor r8d, r8d",
        "xor r9d, r9d",
        "xor r10d, r10d",
        "xor r11d, r11d",
        "xor r12d, r12d",
        "xor r13d, r13d",
        "xor r14d, r14d",
        "xor r15d, r15d",
        "swapgs",
        "iretq",
        ss = const USER_DATA as u64,
        cs = const USER_CODE as u64,
        rflags = const INTERRUPTS_ON | 0x2, // bit 1 always reads as 1
        rsp = in(reg) stack,
        rip = in(reg) entry,
        in("rdi") first,
        in("rsi") second,
        options(noreturn),
    )
}
