//! Starting the other CPUs.
//!
//! When the kernel begins, one CPU is running it. The others are powered
//! but idle, waiting for a message through their local APICs. This file
//! sends it, and holds the code each one runs first.
//!
//! The difficulty is where a CPU starts: exactly where the boot CPU
//! started when the machine was switched on - in 16-bit real mode, as an
//! 8086 would, able to reach 1 MiB of memory, with no page tables and no
//! stack. The boot CPU had a bootloader to bring it from there to 64-bit
//! mode. The others have only the kernel, so the kernel carries a small
//! bootloader of its own for them: the *trampoline*, below.
//!
//!   boot CPU                              a CPU being started
//!   --------                              -------------------
//!   copy the trampoline to a page
//!   below 1 MiB; write into it the
//!   page tables, stack and function
//!   the new CPU is to use
//!   send INIT, wait 10 ms                 resets, waits
//!   send STARTUP(page)                    begins at page:0000, real mode
//!   wait for it to say "awake"            trampoline: real mode ->
//!                                           protected mode -> long mode
//!                                         jumps to `ap_main`, in the
//!                                           kernel, on its own stack
//!                                         loads its GDT, TSS, the IDT;
//!                                           says "awake"
//!   next CPU
//!
//! One at a time: the trampoline page holds *one* stack pointer.
//!
//! The firmware's name for these CPUs is "application processors" (AP),
//! and for the first one "bootstrap processor" (BSP); hence `ap_main`.
//!
//! What the started CPUs do afterwards: they prove they can run kernel
//! code alongside the boot CPU (`count_together`), and wait. When the
//! boot CPU has set up threads and calls `release`, each starts its own
//! timer and becomes its idle thread - from then on it runs whatever
//! the scheduler gives it (threads.rs).

use core::arch::global_asm;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use kernel64::acpi::Madt;
use kernel64::memory::addr::{phys_to_virt, PhysAddr, VirtAddr, PAGE_SIZE};
use kernel64::paging::{Flags, PageSize, PageTables};
use kernel64::smp::{self, MAX_CPUS};
use kernel64::sync::SpinLock;

use super::gdt::{self, CpuTables};
use super::mmu::{self, Control};
use super::percpu::{self, PerCpu};
use super::{apic, idt, platform, syscall};
use crate::mm;

// The trampoline. It is assembled into the kernel like everything else,
// but never runs where the linker put it: it is data there, copied to
// low memory and run from the copy. So it cannot name its own labels by
// address - the linker's addresses are wrong for the copy - and works
// out where it is instead: real mode starts it with CS = page / 16, so
// CS * 16 is the address of its first byte, kept in EBX from the first
// instructions to the last. Every label is then "EBX + how far the
// label is from the start", and that distance the assembler knows.
//
// AT&T syntax (source first, destination second, `%` before registers),
// because that is the syntax in which LLVM's assembler knows 16-bit
// code best.
//
// It climbs through three modes, and each step is one bit and one jump:
//
//   real mode       16-bit. Set CR0.PE (protection enable), and jump to
//                   a 32-bit code segment: the jump is what loads CS,
//                   and CS is what says how instructions are decoded.
//   protected mode  32-bit, no paging. 64-bit mode exists only with
//                   paging, so: CR4.PAE, CR3 = the page tables, EFER.LME
//                   (long mode enable), then CR0.PG (paging). Now the
//                   CPU is in long mode but still decoding 32-bit code,
//                   at the same address - which is why the tables must
//                   map this page at its own address.
//   long mode       jump to a 64-bit code segment. Take the stack, call
//                   the kernel.
global_asm!(
    r#"
    .pushsection .rodata.ap_trampoline, "a"
    .balign 16
    .global ap_trampoline_start
    .global ap_trampoline_end
    .global ap_trampoline_cr3
    .global ap_trampoline_efer
    .global ap_trampoline_stack
    .global ap_trampoline_entry
    .global ap_trampoline_argument

    .code16
ap_trampoline_start:
    cli
    cld
    xorl    %ebx, %ebx
    movw    %cs, %bx
    shll    $4, %ebx                # EBX = where this page is
    movw    %cs, %ax
    movw    %ax, %ds                # data is in the same page as code

    # The three places that need a full address, filled in now that it
    # is known: where the GDT is, and the two jumps.
    leal    (2f - ap_trampoline_start)(%ebx), %eax
    movl    %eax, (3f + 2 - ap_trampoline_start)
    leal    (6f - ap_trampoline_start)(%ebx), %eax
    movl    %eax, (4f - ap_trampoline_start)
    leal    (7f - ap_trampoline_start)(%ebx), %eax
    movl    %eax, (5f - ap_trampoline_start)

    lgdtl   (3f - ap_trampoline_start)
    movl    %cr0, %eax
    orl     $1, %eax                # PE
    movl    %eax, %cr0
    ljmpl   *(4f - ap_trampoline_start)

    .code32
6:
    movw    $0x10, %ax
    movw    %ax, %ds
    movw    %ax, %es
    movw    %ax, %ss
    movl    %cr4, %eax
    orl     $0x20, %eax             # PAE
    movl    %eax, %cr4
    movl    (ap_trampoline_cr3 - ap_trampoline_start)(%ebx), %eax
    movl    %eax, %cr3
    movl    $0xC0000080, %ecx       # EFER
    movl    (ap_trampoline_efer - ap_trampoline_start)(%ebx), %eax
    xorl    %edx, %edx
    wrmsr
    movl    %cr0, %eax
    orl     $0x80000000, %eax       # PG
    movl    %eax, %cr0
    ljmpl   *(5f - ap_trampoline_start)(%ebx)

    .code64
7:
    movl    %ebx, %ebx              # the top half of RBX is not promised to be 0
    movq    (ap_trampoline_stack - ap_trampoline_start)(%rbx), %rsp
    movq    (ap_trampoline_argument - ap_trampoline_start)(%rbx), %rdi
    movq    (ap_trampoline_entry - ap_trampoline_start)(%rbx), %rax
    xorl    %ebp, %ebp
    callq   *%rax
8:  cli
    hlt
    jmp     8b

    # A GDT for the climb only: flat 32-bit code and data, and 64-bit
    # code. The kernel's own replaces it within a few instructions.
    .balign 8
2:  .quad   0
    .quad   0x00CF9A000000FFFF      # 0x08: 32-bit code
    .quad   0x00CF92000000FFFF      # 0x10: data
    .quad   0x00AF9A000000FFFF      # 0x18: 64-bit code
3:  .word   31                      # what lgdt reads: size - 1, address
    .long   0
4:  .long   0                       # a far pointer: address, then segment
    .word   0x08
5:  .long   0
    .word   0x18

    # What the boot CPU fills in before each start.
    .balign 8
ap_trampoline_cr3:      .quad 0
ap_trampoline_efer:     .quad 0
ap_trampoline_stack:    .quad 0
ap_trampoline_entry:    .quad 0
ap_trampoline_argument: .quad 0
ap_trampoline_end:
    .popsection
    .code64
"#,
    options(att_syntax)
);

extern "C" {
    static ap_trampoline_start: u8;
    static ap_trampoline_end: u8;
    static ap_trampoline_cr3: u8;
    static ap_trampoline_efer: u8;
    static ap_trampoline_stack: u8;
    static ap_trampoline_entry: u8;
    static ap_trampoline_argument: u8;
}

/// EFER.LMA: "long mode is active". The CPU sets it; it is not ours to
/// write.
const EFER_LMA: u64 = 1 << 10;

/// How long a CPU gets to say it is awake after being told to start.
/// A working one takes microseconds; this is for one that never will.
const START_TIMEOUT_US: u64 = 500_000;

/// Everything a new CPU is handed. Made by the boot CPU, never freed.
struct Start {
    tables: *mut CpuTables,
    block: *mut PerCpu,
    double_fault_stack: u64,
    stack_top: u64,
    control: Control,
    /// STARTING until the new CPU says AWAKE or the boot CPU, tired of
    /// waiting, says ABANDONED. Whoever changes it first wins, so the
    /// two always agree on whether this CPU is in use.
    state: AtomicUsize,
}

const STARTING: usize = 0;
const AWAKE: usize = 1;
const ABANDONED: usize = 2;

/// How many CPUs besides the boot CPU are awake.
static OTHERS: AtomicUsize = AtomicUsize::new(0);
/// Set by `release`: the scheduler is ready for them.
static RELEASED: AtomicBool = AtomicBool::new(false);
/// How many of them have started their timers and begun to idle.
static IDLING: AtomicUsize = AtomicUsize::new(0);

/// Writes `value` into the copy of the trampoline at `page`, at the
/// field whose place in the original is `field`.
///
/// # Safety
/// `page` holds a copy of the trampoline, reached through the direct
/// map; `field` is one of its eight-byte fields.
unsafe fn set_field(page: u64, field: *const u8, value: u64) {
    let offset = field as u64 - &raw const ap_trampoline_start as u64;
    phys_to_virt(PhysAddr::new(page + offset)).as_mut_ptr::<u64>().write_unaligned(value);
}

/// Starts every CPU the firmware listed, one after another, and reports.
/// Returns once each has either said it is awake or been given up on.
///
/// # Safety
/// Once, on the boot CPU, after `platform::init` (the local APIC and
/// the TSC's speed are needed) and before any process exists.
pub unsafe fn start(madt: &Madt) {
    let plan = smp::plan(&madt.processors, apic::local_id(), MAX_CPUS);
    if plan.left_asleep > 0 {
        kprintln!("[smp] {} more CPU(s) than the {MAX_CPUS} this kernel uses: left asleep", plan.left_asleep);
    }
    if plan.cpus.len() == 1 {
        kprintln!("[smp] 1 CPU: no others to start");
        return;
    }

    let trampoline = &raw const ap_trampoline_start;
    let length = &raw const ap_trampoline_end as usize - trampoline as usize;
    assert!(length <= PAGE_SIZE as usize, "the trampoline has outgrown its page");
    let Some(page) = mm::low_page() else {
        kprintln!("[smp] no free page below 1 MiB to start a CPU in: staying with 1 CPU");
        return;
    };
    let startup = smp::startup_message(page).expect("mm::low_page gave a page no CPU can start in");

    // An address space for the climb: the kernel's half, as every
    // process has it, plus the trampoline's page at its own address -
    // which is in the half that belongs to programs, so it cannot go in
    // the kernel's own tables. The new CPU uses this for a few dozen
    // instructions and then moves to the kernel's.
    let climb = mm::with_memory(|kernel, frames| {
        let mut tables = PageTables::new_user(frames, mm::DIRECT_MAP, kernel)?;
        let flags = Flags::WRITABLE; // and executable: no NO_EXECUTE
        match tables.map(VirtAddr::new(page), PhysAddr::new(page), PageSize::Size4K, flags, frames) {
            Ok(()) => Some(tables),
            Err(_) => {
                tables.destroy(frames);
                None
            }
        }
    });
    let Some(climb) = climb else {
        kprintln!("[smp] no memory for the page tables to start a CPU with: staying with 1 CPU");
        return;
    };
    // In protected mode CR3 is a 32-bit register.
    if climb.root().as_u64() >= 1 << 32 {
        kprintln!("[smp] the page tables to start a CPU with are above 4 GiB: staying with 1 CPU");
        mm::with_frames(|frames| climb.destroy(frames));
        return;
    }

    let control = mmu::control();
    phys_to_virt(PhysAddr::new(page)).as_mut_ptr::<u8>().copy_from_nonoverlapping(trampoline, length);
    set_field(page, &raw const ap_trampoline_cr3, climb.root().as_u64());
    set_field(page, &raw const ap_trampoline_efer, control.efer & !EFER_LMA);
    set_field(page, &raw const ap_trampoline_entry, ap_main as *const () as u64);

    // The kernel's numbers of the CPUs that woke, in order: CPU n is
    // the nth to answer, so the numbers have no holes even if a CPU
    // the firmware listed stays silent.
    let mut awake: Vec<usize> = alloc::vec![0];
    let mut silent = 0;
    for &listed in &plan.cpus[1..] {
        let cpu = smp::Cpu { index: awake.len(), apic_id: listed.apic_id };
        // Two stacks of its own, each with a guard page below it: the
        // one it runs on, and the one for a double fault.
        let (Some(stack), Some(double_fault_stack)) = (mm::allocate_stack(), mm::allocate_stack()) else {
            kprintln!("[smp] no memory for CPU {}'s stacks: not started", cpu.index);
            continue;
        };
        let start: &'static Start = Box::leak(Box::new(Start {
            tables: gdt::new_tables(),
            block: Box::into_raw(Box::new(PerCpu::new(cpu.index))),
            double_fault_stack: double_fault_stack.top(),
            stack_top: stack.top(),
            control,
            state: AtomicUsize::new(STARTING),
        }));
        // The CPU keeps its stacks for as long as the machine is on.
        core::mem::forget(stack);
        core::mem::forget(double_fault_stack);
        set_field(page, &raw const ap_trampoline_stack, start.stack_top);
        set_field(page, &raw const ap_trampoline_argument, start as *const Start as u64);

        // The sequence is Intel's (manual volume 3, "MP initialization
        // protocol"): INIT, 10 ms for the CPU to finish resetting, then
        // STARTUP - twice, because a CPU may miss the first; one that
        // did not miss it is already running and ignores the second.
        apic::send(cpu.apic_id, smp::init_message());
        super::delay_us(10_000);
        for _ in 0..2 {
            apic::send(cpu.apic_id, startup);
            super::delay_us(200);
        }

        let mut waited = 0;
        while start.state.load(Ordering::Acquire) == STARTING && waited < START_TIMEOUT_US {
            super::delay_us(50);
            waited += 50;
        }
        // Giving up is a compare-and-exchange: it fails if the CPU
        // answered in this very moment, and then it did answer.
        if start.state.compare_exchange(STARTING, ABANDONED, Ordering::AcqRel, Ordering::Acquire).is_err() {
            awake.push(cpu.index);
        } else {
            kprintln!("[smp] the CPU with APIC id {} did not answer: left out", cpu.apic_id);
            silent += 1;
        }
    }
    OTHERS.store(awake.len() - 1, Ordering::Release);

    if silent == 0 {
        mm::with_frames(|frames| climb.destroy(frames));
    }
    // Otherwise the tables are left where they are, and the page: a CPU
    // that was only slow may still come through them.

    let mut others = String::new();
    for cpu in &awake[1..] {
        if !others.is_empty() {
            others.push_str(", ");
        }
        others.push_str(&alloc::format!("{cpu}"));
    }
    if awake.len() == 1 {
        kprintln!("[smp] no other CPU answered: staying with 1 CPU");
        return;
    }
    kprintln!("[smp] {} CPUs awake: CPU {others} started through the page at {page:#x}", awake.len());
    count_together(awake.len());
}

/// Lets the started CPUs into the scheduler: gives it an idle thread
/// for each, and tells them. Returns when every one has its timer
/// running. Before this they wait; after it, any thread may run on any
/// of them.
///
/// Called once by the boot CPU, after `threads::init`.
pub fn release() {
    let others = OTHERS.load(Ordering::Acquire);
    if others == 0 {
        return;
    }
    crate::threads::add_cpus(others);
    RELEASED.store(true, Ordering::Release);
    let mut waited = 0;
    while IDLING.load(Ordering::Acquire) < others && waited < START_TIMEOUT_US {
        super::delay_us(50);
        waited += 50;
    }
    let idling = IDLING.load(Ordering::Acquire);
    if idling == others {
        kprintln!("[smp] {} CPUs run threads: each has its own timer and idle thread, and all take from one queue", others + 1);
    } else {
        kprintln!("[smp] FAILED - only {idling} of {others} started CPUs joined the scheduler");
    }
}

/// The first Rust a started CPU runs: called by the trampoline, in long
/// mode, on this CPU's own stack, in the address space made for the
/// climb. Interrupts are off and stay off.
extern "C" fn ap_main(start: &'static Start) -> ! {
    // SAFETY: this CPU is being set up for the first time, with
    // interrupts off, with what the boot CPU prepared for it alone.
    unsafe {
        // The same protections as the boot CPU - write protection for
        // the kernel among them - and off the borrowed page tables.
        mmu::set_control(start.control);
        mmu::activate(mm::kernel_root());
        gdt::load(start.tables, start.double_fault_stack);
        percpu::install(start.block, start.tables);
        // From here a fault on this CPU is a message, as on the first.
        idt::load();
        syscall::init();
    }
    apic::enable_local(platform::SPURIOUS_VECTOR);

    kprintln!(
        "[smp] CPU {} awake: APIC id {}, stack below {:#x}",
        // Not from `start`: from this CPU's own block, found through GS.
        percpu::index(),
        apic::local_id(),
        start.stack_top
    );
    // Release: the boot CPU, seeing this, also sees everything above.
    if start.state.compare_exchange(STARTING, AWAKE, Ordering::AcqRel, Ordering::Acquire).is_err() {
        // The boot CPU gave up waiting and has gone on without this
        // one; its number may be another CPU's by now.
        kprintln!("[smp] ...too late: the boot CPU had stopped waiting. Halted.");
        super::halt_forever();
    }

    count();
    while !RELEASED.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }
    platform::start_timer_here();
    IDLING.fetch_add(1, Ordering::Release);
    // Interrupts are still off; the idle loop turns them on as it
    // halts, and the first timer tick may already bring a thread.
    crate::threads::run_idle()
}

// ---- proof that they run at the same time --------------------------------

/// How many times each CPU adds one.
const ROUNDS: u64 = 100_000;

static GO: AtomicBool = AtomicBool::new(false);
static FINISHED: AtomicUsize = AtomicUsize::new(0);
/// A number that only the holder of the lock may change.
static LOCKED: SpinLock<u64> = SpinLock::new(0);
/// The same number with nothing guarding it. An AtomicU64 only so that
/// Rust allows sharing it at all; it is used as a plain variable would
/// be - read, add one, write back, as three separate steps.
static UNLOCKED: AtomicU64 = AtomicU64::new(0);

/// What every CPU does: add one, ROUNDS times, to both numbers.
fn count() {
    while !GO.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }
    for _ in 0..ROUNDS {
        *LOCKED.lock() += 1;
        // Between the read and the write another CPU may do both: its
        // addition is then written over, and lost.
        let seen = UNLOCKED.load(Ordering::Relaxed);
        UNLOCKED.store(seen + 1, Ordering::Relaxed);
    }
    FINISHED.fetch_add(1, Ordering::Release);
}

/// Has all `cpus` CPUs count at once, and says what came of it. With a
/// lock every addition must be there; that is the test. Without one
/// some are lost - how many differs from boot to boot - which is the
/// reason for every lock in the kernel, shown rather than argued.
fn count_together(cpus: usize) {
    GO.store(true, Ordering::Release);
    count();
    let mut waited = 0;
    while FINISHED.load(Ordering::Acquire) < cpus && waited < 5_000_000 {
        super::delay_us(100);
        waited += 100;
    }
    let expected = cpus as u64 * ROUNDS;
    let locked = *LOCKED.lock();
    let unlocked = UNLOCKED.load(Ordering::Relaxed);
    if FINISHED.load(Ordering::Acquire) < cpus {
        kprintln!("[smp] counting together: FAILED - only {} of {cpus} CPUs finished", FINISHED.load(Ordering::Acquire));
    } else if locked != expected {
        kprintln!("[smp] counting together: FAILED - {locked} of {expected} with a lock");
    } else {
        kprintln!(
            "[smp] counting together: {cpus} CPUs reached {locked} of {expected} with a lock; without one {unlocked}, {} lost",
            expected - unlocked.min(expected)
        );
    }
}
