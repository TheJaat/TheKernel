//! The other CPUs: asking the firmware to start them, and what each
//! does when it wakes.
//!
//! The PC's kernel starts its CPUs by hand (arch/x86_64/smp.rs). Here
//! the firmware does it, when asked: PSCI's CPU_ON takes a CPU's name,
//! an address and a number, and the CPU begins at that address with
//! the number in x0 (`kernel64::psci`). What the device tree
//! contributes is the list of CPUs and the instruction to ask with.
//!
//! A started CPU arrives as the boot CPU once did - MMU off, at a
//! physical address - at `secondary_start` (entry.rs), which adopts
//! the kernel's page tables and calls `secondary_main` here, on a
//! stack of its own. One CPU is started at a time, and the boot CPU
//! waits for each: so one `Start` is enough for all of them.
//!
//! After that the two machines are the same. The scheduler was written
//! for several CPUs and knows them by number; what it needs from this
//! file is `poke` - interrupt a CPU that sits halted - and each CPU
//! running its own timer.

use alloc::string::String;
use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use kernel64::devicetree::{FirmwareCall, Machine};
use kernel64::gic::{SOFTWARE_RESCHEDULE, SOFTWARE_STOP};
use kernel64::psci;
use kernel64::smp::MAX_CPUS;
use kernel64::sync::SpinLock;

use super::{gic, mmu, timer, vectors};
use crate::mm;

/// Each CPU's `MPIDR_EL1`, by the kernel's number for it.
static MPIDRS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
/// How many CPUs besides the boot CPU woke.
static OTHERS: AtomicUsize = AtomicUsize::new(0);
/// Set by `release`: the scheduler is ready for them.
static RELEASED: AtomicBool = AtomicBool::new(false);
/// How many of them have started their timers and begun to idle.
static IDLING: AtomicUsize = AtomicUsize::new(0);

/// What the boot CPU prepares for the CPU being started. Read by
/// `secondary_start` before it has a stack or an MMU, by each field's
/// place - eight bytes each, in this order - so the order is part of
/// that code. (`AtomicU64` is laid out exactly as a `u64`; it is used
/// only so that a `static` can be written without `unsafe`.)
#[repr(C)]
struct Start {
    /// The kernel's page tables (`TTBR1_EL1`).
    tables: AtomicU64,
    /// `TCR_EL1` as the boot CPU has it, but with the low half on.
    translation: AtomicU64,
    /// `SCTLR_EL1`: MMU on, and every protection the boot CPU runs with.
    control: AtomicU64,
    /// `MAIR_EL1`: what the kinds of memory in the tables mean.
    memory_kinds: AtomicU64,
    /// `CPACR_EL1` and `CNTKCTL_EL1`: what programs may use.
    features: AtomicU64,
    counter_access: AtomicU64,
    /// The top of the CPU's own stack.
    stack_top: AtomicU64,
    /// The kernel's number for the CPU.
    index: AtomicU64,
}

static START: Start = Start {
    tables: AtomicU64::new(0),
    translation: AtomicU64::new(0),
    control: AtomicU64::new(0),
    memory_kinds: AtomicU64::new(0),
    features: AtomicU64::new(0),
    counter_access: AtomicU64::new(0),
    stack_top: AtomicU64::new(0),
    index: AtomicU64::new(0),
};

const STARTING: usize = 0;
const AWAKE: usize = 1;
const ABANDONED: usize = 2;
/// Where the CPU being started has got to.
static STATE: AtomicUsize = AtomicUsize::new(STARTING);

/// How long a CPU is given to answer. On real hardware it takes
/// milliseconds; an emulator short of host CPUs can take much longer.
const START_TIMEOUT_US: u64 = 1_000_000;

extern "C" {
    /// entry.rs. Never called: its address is given to the firmware.
    fn secondary_start();
}

fn own_mpidr() -> u64 {
    let value: u64;
    // SAFETY: reading this CPU's name changes nothing.
    unsafe { asm!("mrs {}, mpidr_el1", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

/// Waits `us` microseconds, by the CPU's counter - which runs whether
/// or not interrupts do.
fn delay_us(us: u64) {
    let start = timer::counter();
    let steps = u128::from(us) * u128::from(timer::frequency()) / 1_000_000;
    while u128::from(timer::counter().wrapping_sub(start)) < steps {
        core::hint::spin_loop();
    }
}

/// One call to the firmware: function `function`, three arguments,
/// one answer - by the instruction the device tree named.
///
/// # Safety
/// `function` must be a PSCI function and the arguments what it takes:
/// the firmware does what it is asked.
unsafe fn firmware(call: FirmwareCall, function: u64, first: u64, second: u64, third: u64) -> u64 {
    let answer: u64;
    // SAFETY: the caller's promise. The calling convention lets the
    // firmware change x0 to x17 and no more, which is no more than a
    // C function may ("clobber_abi").
    unsafe {
        match call {
            FirmwareCall::Hvc => asm!(
                "hvc #0",
                inout("x0") function => answer, inout("x1") first => _, inout("x2") second => _, inout("x3") third => _,
                clobber_abi("C"), options(nostack),
            ),
            FirmwareCall::Smc => asm!(
                "smc #0",
                inout("x0") function => answer, inout("x1") first => _, inout("x2") second => _, inout("x3") third => _,
                clobber_abi("C"), options(nostack),
            ),
        }
    }
    answer
}

/// Starts every other CPU the device tree lists, one after another,
/// and reports. Returns once each has either said it is awake or been
/// given up on.
///
/// # Safety
/// Once, on the boot CPU, after the interrupt controller and the timer
/// are set up and before any process exists.
pub unsafe fn start(machine: &Machine) {
    let boot = own_mpidr();
    MPIDRS[0].store(boot, Ordering::Relaxed);
    let plan = psci::plan(machine.cpus(), boot, MAX_CPUS);
    // The tree's list itself stops at a limit; the tree's count does not.
    let left_asleep = plan.left_asleep + machine.cpu_count.saturating_sub(machine.cpus().len());
    if left_asleep > 0 {
        kprintln!("[smp] {left_asleep} more CPU(s) than the {MAX_CPUS} this kernel uses: left asleep");
    }
    if plan.start.is_empty() {
        kprintln!("[smp] 1 CPU: no others to start");
        return;
    }
    let Some(call) = machine.firmware_call else {
        kprintln!(
            "[smp] {} more CPU(s) listed, but the device tree names no firmware (PSCI) to start them: staying with 1 CPU",
            plan.start.len()
        );
        return;
    };
    let by = match call {
        FirmwareCall::Hvc => "hvc",
        FirmwareCall::Smc => "smc",
    };
    // SAFETY: PSCI_VERSION takes no arguments and changes nothing.
    let Some((major, minor)) = psci::version(unsafe { firmware(call, psci::VERSION, 0, 0, 0) }) else {
        kprintln!("[smp] the firmware does not answer as PSCI (asked by {by}): staying with 1 CPU");
        return;
    };

    // Where a started CPU begins, and what it is handed: both as
    // physical addresses, and both where the low mapping it borrows
    // can reach - they are in the kernel's image.
    let entry = mmu::kernel_physical(secondary_start as *const () as u64);
    let prepared = mmu::kernel_physical(&raw const START as u64);
    assert!(
        mmu::early_reach().contains(&entry) && mmu::early_reach().contains(&prepared),
        "the kernel image runs over the end of the one GiB a starting CPU can see"
    );
    let (tables, translation, control, memory_kinds, features, counter_access): (u64, u64, u64, u64, u64, u64);
    // SAFETY: reading six registers of this CPU; nothing changes.
    unsafe {
        asm!(
            "mrs {0}, ttbr1_el1", "mrs {1}, tcr_el1", "mrs {2}, sctlr_el1",
            "mrs {3}, mair_el1", "mrs {4}, cpacr_el1", "mrs {5}, cntkctl_el1",
            out(reg) tables, out(reg) translation, out(reg) control,
            out(reg) memory_kinds, out(reg) features, out(reg) counter_access,
            options(nomem, nostack, preserves_flags),
        );
    }
    START.tables.store(tables, Ordering::Relaxed);
    // The boot CPU has its low half off (bit 7) unless a program is
    // running; the starting CPU needs it, for a moment.
    START.translation.store(translation & !(1 << 7), Ordering::Relaxed);
    START.control.store(control, Ordering::Relaxed);
    START.memory_kinds.store(memory_kinds, Ordering::Relaxed);
    START.features.store(features, Ordering::Relaxed);
    START.counter_access.store(counter_access, Ordering::Relaxed);

    // The kernel's numbers of the CPUs that woke, in order: CPU n is
    // the nth to answer, so the numbers have no holes even if a CPU
    // the tree lists stays silent.
    let mut awake = 1;
    let mut others = String::new();
    for &name in &plan.start {
        let Some(stack) = mm::allocate_stack() else {
            kprintln!("[smp] no memory for CPU {awake}'s stack: not started");
            continue;
        };
        START.stack_top.store(stack.top(), Ordering::Relaxed);
        START.index.store(awake as u64, Ordering::Relaxed);
        MPIDRS[awake].store(name, Ordering::Relaxed);
        // Release: the new CPU, reading this, also reads everything above.
        STATE.store(STARTING, Ordering::Release);

        // SAFETY: CPU_ON with the name of a CPU the tree lists, the
        // address of code written to be started at, and the address
        // of what that code reads.
        match psci::cpu_on_result(unsafe { firmware(call, psci::CPU_ON, name, entry, prepared) }) {
            Ok(()) => {}
            Err(refusal) => {
                kprintln!("[smp] the firmware would not start the CPU named {name:#x}: {refusal:?}");
                continue; // the stack goes back: nobody is on it
            }
        }
        let mut waited = 0;
        while STATE.load(Ordering::Acquire) == STARTING && waited < START_TIMEOUT_US {
            delay_us(50);
            waited += 50;
        }
        // Giving up is a compare-and-exchange: it fails if the CPU
        // answered in this very moment, and then it did answer.
        if STATE.compare_exchange(STARTING, ABANDONED, Ordering::AcqRel, Ordering::Acquire).is_err() {
            if !others.is_empty() {
                others.push_str(", ");
            }
            others.push_str(&alloc::format!("{awake}"));
            awake += 1;
        } else {
            kprintln!("[smp] the CPU named {name:#x} was started and did not answer: left out");
        }
        // Awake or only slow, the CPU has its stack for as long as the
        // machine is on.
        core::mem::forget(stack);
    }
    OTHERS.store(awake - 1, Ordering::Release);
    if awake == 1 {
        kprintln!("[smp] no other CPU answered: staying with 1 CPU");
        return;
    }
    kprintln!("[smp] {awake} CPUs awake: CPU {others} started by the firmware (PSCI {major}.{minor}, asked by {by})");
    count_together(awake);
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
        delay_us(50);
        waited += 50;
    }
    let idling = IDLING.load(Ordering::Acquire);
    if idling == others {
        kprintln!("[smp] {} CPUs run threads: each has its own timer and idle thread, and all take from one queue", others + 1);
    } else {
        kprintln!("[smp] FAILED - only {idling} of {others} started CPUs joined the scheduler");
    }
}

/// Interrupts the CPUs in `cpus` (a bit each, by the kernel's numbers)
/// so that they look at the scheduler's queue. Returns at once.
pub fn poke(cpus: u64) {
    // `enumerate` gives each element with its position: the CPU's
    // number, and the name the hardware knows it by.
    for (cpu, mpidr) in MPIDRS.iter().enumerate() {
        if cpus & (1 << cpu) != 0 {
            gic::send(cpu, mpidr.load(Ordering::Relaxed), SOFTWARE_RESCHEDULE);
        }
    }
}

/// Tells every other CPU to stop, for good. Does not wait for them.
pub fn stop_others() {
    let this = super::cpu();
    let running = OTHERS.load(Ordering::Acquire) + 1;
    for (cpu, mpidr) in MPIDRS.iter().enumerate().take(running) {
        if cpu != this {
            gic::send(cpu, mpidr.load(Ordering::Relaxed), SOFTWARE_STOP);
        }
    }
}

/// The first Rust a started CPU runs: called by `secondary_start`, on
/// this CPU's own stack, with the kernel's tables, at a high address.
/// Interrupts are off. `index` is the kernel's number for this CPU,
/// which `super::cpu()` already returns.
#[no_mangle]
extern "C" fn secondary_main(index: u64) -> ! {
    // SAFETY: this CPU is being set up for the first time, with
    // interrupts off; the low half was needed only to get here.
    unsafe {
        // From here a fault on this CPU is a message, as on the first.
        vectors::init();
        mmu::low_half_off();
        gic::init_here(index as usize);
    }
    kprintln!("[smp] CPU {} awake: named {:#x}, stack below {:#x}", super::cpu(), psci::name(own_mpidr()), START.stack_top.load(Ordering::Relaxed));
    // Release: the boot CPU, seeing this, also sees everything above.
    if STATE.compare_exchange(STARTING, AWAKE, Ordering::AcqRel, Ordering::Acquire).is_err() {
        // The boot CPU gave up waiting and has gone on without this
        // one; its number may be another CPU's by now.
        kprintln!("[smp] ...too late: the boot CPU had stopped waiting. Halted.");
        super::halt_forever();
    }

    count();
    while !RELEASED.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }
    // SAFETY: once, here, interrupts off, the controller's part for
    // this CPU set up above.
    unsafe { timer::start_here() };
    // The two interrupts other CPUs send this one.
    gic::enable(SOFTWARE_RESCHEDULE);
    gic::enable(SOFTWARE_STOP);
    IDLING.fetch_add(1, Ordering::Release);
    // Interrupts are still off; the idle loop turns them on as it
    // waits, and the first timer tick may already bring a thread.
    crate::threads::run_idle()
}

// ---- proof that they run at the same time --------------------------------
// The same proof as on the PC (arch/x86_64/smp.rs), by the same means.

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
/// lock every addition must be there; that is the test.
fn count_together(cpus: usize) {
    GO.store(true, Ordering::Release);
    count();
    let mut waited = 0;
    while FINISHED.load(Ordering::Acquire) < cpus && waited < 5_000_000 {
        delay_us(100);
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
