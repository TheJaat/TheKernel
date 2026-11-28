//! Kernel threads: the mechanism around `kernel64::sched`'s decisions.
//!
//! Each thread has its own kernel stack (with a guard page below it), a
//! saved stack pointer while it is not running, and - until it starts -
//! the closure it will run. The scheduler policy says who runs next;
//! this module makes it happen with `arch::context::switch`.
//!
//! Everything that touches the scheduler or switches runs with
//! interrupts disabled: a timer interrupt arriving halfway through a
//! switch would find the bookkeeping half-updated, and the policy's locks
//! are taken by the timer interrupt too.
//!
//! With several CPUs that is no longer enough: interrupts off stops this
//! CPU from being interrupted, not another CPU from running. The policy
//! locks for itself - a lock per CPU since K193 (journal entry 153; see
//! "Locks" in sched.rs) - and the table of threads has its own lock,
//! THREADS, which no switch takes. Four rules exist only because of the
//! other CPUs:
//!
//! 1. **A thread is loaded only after it has been saved.** A thread
//!    that gives way is queued by `pick_next` at once, before its CPU
//!    has saved its registers; another CPU may take it in that moment.
//!    So each thread has a `leaving` flag: set before `pick_next` may
//!    queue it, cleared by its CPU once the switch away from it is done
//!    (`switched`); a CPU that is to load a thread first waits while the
//!    flag is set. Without the wait, two CPUs would be on one stack.
//!    (Until K193 one lock was held across every switch instead - rule
//!    1 was "the lock is held across the switch" - and every switch on
//!    every CPU waited for it.)
//!
//! 2. **Say you will sleep, then look.** A thread that waits first
//!    marks itself blocked and only then checks what it is waiting for.
//!    The other way round, the thing could happen - on another CPU -
//!    between the check and the mark, its wake-up would find nobody
//!    blocked, and the thread would sleep for ever. In this order a
//!    wake-up that comes too early finds the mark and undoes it
//!    (sched.rs: a thread woken before it left its CPU stays).
//!
//! 3. **A waiter stays in its queue until it leaves.** Waking does not
//!    take anyone out (see `WaitQueue::wake_all` for the wake-up this
//!    lost when it did).
//!
//! 4. **A thread is gone when no CPU is on it.** An exited thread's
//!    stack is freed, and `join` returns, only after its CPU has
//!    switched away: the policy has forgotten it (`remove`) and its
//!    `leaving` flag is clear.
//!
//! Threads do not belong to CPUs. Each CPU has a queue, and takes from
//! any; a thread runs on whichever CPU takes it next.

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Ordering};

use kernel64::ioports::PortBitmap;
use kernel64::sched::{Scheduler, State, Switch, ThreadId};
use kernel64::smp::MAX_CPUS;

use crate::arch::{self, context, mmu, syscall};
use crate::process::Process;
use crate::irqlock::IrqLock;
use crate::mm::{self, KernelStack};
use crate::interrupts;

type Entry = Box<dyn FnOnce() + Send>;

struct Thread {
    name: &'static str,
    /// Saved while the thread is not running. Written by the switch.
    rsp: u64,
    /// Owned for its Drop, which unmaps and frees the stack once the
    /// thread is reaped. None for the boot thread (the boot stack).
    _stack: Option<KernelStack>,
    /// The top of that stack: where the CPU lands on a system call or an
    /// interrupt from user mode.
    stack_top: Option<u64>,
    /// The process whose address space this thread runs in; None for
    /// kernel threads, which use the kernel's.
    process: Option<Arc<Process>>,
    entry: Option<Entry>,
    /// A thread of a program: its thread pointer (FS base on x86_64,
    /// TPIDR_EL0 on AArch64), through which it finds its thread-local
    /// variables. Kept here while the thread is not running, loaded into
    /// the CPU when it is switched to (see `schedule`).
    pointer: u64,
    /// A thread of a program: the stack its signals marked "on stack"
    /// are delivered on (SIGNAL_STACK) - base and size; size 0, none.
    signal_stack: (u64, u64),
    /// Rule 1: set while a CPU is switching away from this thread and has
    /// not yet saved it; a CPU that is to load it waits for it to clear.
    leaving: AtomicBool,
}

impl Thread {
    fn new(name: &'static str, rsp: u64, stack: Option<KernelStack>, process: Option<Arc<Process>>, entry: Option<Entry>, pointer: u64) -> Box<Thread> {
        let stack_top = stack.as_ref().map(KernelStack::top);
        Box::new(Thread { name, rsp, _stack: stack, stack_top, process, entry, pointer, signal_stack: (0, 0), leaving: AtomicBool::new(false) })
    }

    /// What the policy keeps to find this record again: its address. A
    /// Box never moves what it holds, and the record is freed only once
    /// the policy has forgotten the thread (rule 4).
    fn handle(&self) -> usize {
        self as *const Thread as usize
    }
}

/// The thread record at `handle` (`Thread::handle`).
///
/// Every field but `leaving` is touched only by the CPU the thread runs
/// on, or by the CPU switching to or from it - and those take turns by
/// rule 1. The table (THREADS) owns the record; it is freed only after
/// rule 4, when no CPU can hold its handle any more.
fn at(handle: usize) -> *mut Thread {
    handle as *mut Thread
}

/// The address space loaded in each CPU's CR3 now, to skip reloading
/// the same one (a reload flushes that CPU's TLB) - and to know which
/// CPUs to tell when a translation is taken away (tlb.rs).
///
/// Each CPU remembers translations on its own. A CPU that stops running
/// a process always loads other page tables before it is idle - its
/// idle thread's, at least - so a CPU that runs a process again has
/// reloaded CR3 since, and remembers nothing stale about it. While it
/// runs the process, though, another thread of the same process on
/// another CPU may unmap a page: then this table is how the unmapping
/// CPU finds the others to tell (`cpus_running`). Keeping a process's
/// tables loaded while idling ("lazy TLB") would be faster, and would
/// mean telling idle CPUs too.
static ACTIVE_ROOT: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// The table of threads: who exists. Taken to add a thread, to reap one,
/// and to list them - never by a switch, a tick or a wake-up.
struct Threads {
    /// Boxed so each Thread stays put: the policy keeps its address
    /// (`handle`), and the switch reads and writes it without this lock.
    table: BTreeMap<ThreadId, Box<Thread>>,
    next_id: ThreadId,
}

static THREADS: IrqLock<Option<Threads>> = IrqLock::new(None);

/// The policy: made once by `init`, never freed. It locks for itself.
static POLICY: AtomicPtr<Scheduler> = AtomicPtr::new(core::ptr::null_mut());

/// The policy, if `init` has run.
fn try_policy() -> Option<&'static Scheduler> {
    // SAFETY: set once from a leaked Box, never changed or freed after.
    unsafe { POLICY.load(Ordering::Acquire).as_ref() }
}

fn policy() -> &'static Scheduler {
    try_policy().expect("threads not initialised")
}

/// The record of the thread each CPU runs (its handle): what the switch,
/// a tick and a system call find it by, with no lock and no look-up.
/// Written only by that CPU, at a switch, with interrupts off.
static CURRENT: [AtomicUsize; MAX_CPUS] = [const { AtomicUsize::new(0) }; MAX_CPUS];

/// The thread each CPU has just switched away from: the one whose
/// `leaving` flag the thread switched to clears (rule 1, `switched`).
static PREVIOUS: [AtomicUsize; MAX_CPUS] = [const { AtomicUsize::new(0) }; MAX_CPUS];

/// The thread running here: its record. Interrupts must be off, or the
/// thread could move to another CPU between asking and using. A raw
/// pointer, used a field at a time: while it runs only this CPU touches
/// its fields, but another CPU may read its `leaving` flag at any time,
/// and a `&mut` to the whole record would claim it was not.
fn here() -> *mut Thread {
    at(CURRENT[arch::cpu()].load(Ordering::Relaxed))
}

/// Threads that have exited and are not yet reaped: `reap` looks at the
/// table only when there are some, so that a switch takes no lock.
static EXITS: AtomicUsize = AtomicUsize::new(0);

/// Wakes everyone waiting in `join` whenever any thread exits.
static EXITED: WaitQueue = WaitQueue::new();

const IDLE: ThreadId = 0;
const BOOT: ThreadId = 1;

/// Turns the code running now into thread 1, "boot", and creates thread
/// 0, the idle thread, which runs whenever nothing else is ready.
pub fn init() {
    let stack = mm::allocate_stack().expect("no stack for the idle thread");
    // SAFETY: a freshly mapped stack, nobody else's.
    let rsp = unsafe { context::initial_stack(stack.top()) };
    let idle = Thread::new("idle", rsp, Some(stack), None, Some(Box::new(idle_loop)), 0);
    let boot = Thread::new("boot", 0, None, None, None, 0);
    ACTIVE_ROOT[0].store(mm::kernel_root(), Ordering::Relaxed);
    let policy = Scheduler::with_running_handles(IDLE, idle.handle(), BOOT, boot.handle());
    CURRENT[0].store(boot.handle(), Ordering::Relaxed);

    let mut table = BTreeMap::new();
    table.insert(IDLE, idle);
    table.insert(BOOT, boot);
    *THREADS.lock() = Some(Threads { table, next_id: 2 });
    POLICY.store(Box::into_raw(Box::new(policy)), Ordering::Release);
}

/// The idle thread: sleeps until an interrupt. If the interrupt makes a
/// thread ready, the switch happens on the way out of the interrupt.
///
/// While it has the CPU it also empties the debug log's queue: writing
/// to the serial line is slow, and this is time nobody else wants. An
/// interrupt that makes a thread ready ends it at once, as above - the
/// queue is emptied a few bytes at a time, with interrupts on between.
fn idle_loop() {
    loop {
        while crate::console::write_queued() {}
        // A CPU that went idle because another's queue was busy asked
        // itself to look again (sched.rs, `choose`): with no interrupt
        // coming, it would otherwise wait for its next tick.
        if policy().reschedule_pending(arch::cpu()) {
            preempt_if_needed();
            continue;
        }
        arch::wait_for_interrupt();
    }
}

/// Gives the scheduler `count` more CPUs, numbered 1 to `count`: an
/// idle thread for each, already "running" there. Called once by the
/// boot CPU, before those CPUs are let go; each then calls
/// `run_idle`.
pub fn add_cpus(count: usize) {
    let mut guard = THREADS.lock();
    let t = guard.as_mut().expect("threads not initialised");
    for cpu in 1..=count {
        let id = t.next_id;
        t.next_id += 1;
        // Like the boot thread: it runs on the stack the CPU was
        // started on, which nobody frees, and its `rsp` is filled in
        // the first time it is switched away from.
        let idle = Thread::new("idle", 0, None, None, None, 0);
        CURRENT[cpu].store(idle.handle(), Ordering::Relaxed);
        assert_eq!(policy().add_cpu_handle(id, idle.handle()), cpu);
        t.table.insert(id, idle);
    }
}

/// What a started CPU does for the rest of its life: it is its idle
/// thread from here on, and everything else it ever runs it reaches by
/// an interrupt switching away from this loop.
pub fn run_idle() -> ! {
    ACTIVE_ROOT[arch::cpu()].store(mm::kernel_root(), Ordering::Relaxed);
    idle_loop();
    unreachable!("the idle loop ended")
}

/// The CPUs whose CR3 holds `root` now, as a bit set.
pub fn cpus_running(root: u64) -> u64 {
    ACTIVE_ROOT
        .iter()
        .enumerate()
        .filter(|(_, active)| active.load(Ordering::SeqCst) == root)
        .fold(0, |set, (cpu, _)| set | 1 << cpu)
}

/// When each CPU's current thread began to be counted (clock::monotonic,
/// in nanoseconds): set at every switch and tick. 0 until the CPU's first.
static RUNNING_SINCE: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// Counts the time since the last switch or tick to the process of the
/// thread `cpu` is running (if it is a program's), and starts counting
/// again from `now`. Called with the lock held, before the policy changes
/// what `cpu` runs: at a switch it is still the thread that leaves.
///
/// CPU time is the time a thread was on a CPU. A thread that sleeps or
/// waits is on none and is not counted; an idle thread belongs to no
/// process. Counting at each tick too keeps a thread that never leaves
/// its CPU from being counted all at once, much later (journal entry
/// 114).
fn charge(cpu: usize, now: u64) {
    let since = RUNNING_SINCE[cpu].swap(now, Ordering::Relaxed);
    if since == 0 || now <= since {
        return;
    }
    // SAFETY: the running thread's record lives while it runs (rule 4).
    if let Some(process) = unsafe { (*here()).process.as_ref() } {
        process.add_cpu(now - since);
    }
}

/// The CPU time the current process has used: what its threads were
/// counted up to their last switch or tick, and the calling thread's time
/// since its own (clock::PROCESS_CPU). Another of its threads running on
/// another CPU right now is counted up to its last tick, 10 ms at most.
pub fn process_cpu_ns() -> Option<u64> {
    let were_enabled = arch::disable_interrupts();
    let cpu = arch::cpu();
    let now = crate::clock::monotonic();
    // SAFETY: as in `charge`.
    let answer = unsafe { (*here()).process.as_ref() }.map(|process| {
        let since = RUNNING_SINCE[cpu].load(Ordering::Relaxed);
        let running = if since != 0 && now > since { now - since } else { 0 };
        process.cpu_ns() + running
    });
    arch::restore_interrupts(were_enabled);
    answer
}

/// Set when something done on a CPU - a wake-up, a new priority - asked
/// that CPU to choose again: what the end of a system call reads before
/// it takes the lock (`preempt_if_asked`). Without it, every call took
/// the lock once more on its way out, about 130 ns a switch on MaK-WSL;
/// with it, only calls that need to (journal entry 124).
static ASKED: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];

/// Asks the policy something, with interrupts off - its locks are taken
/// by the timer interrupt too - and afterwards tells whoever it asked.
fn with<R>(f: impl FnOnce(&'static Scheduler, usize) -> R) -> R {
    let were_enabled = arch::disable_interrupts();
    let cpu = arch::cpu();
    let policy = policy();
    let result = f(policy, cpu);
    if policy.reschedule_pending(cpu) {
        ASKED[cpu].store(true, Ordering::Relaxed);
    }
    tell_idle_cpus(policy, cpu);
    arch::restore_interrupts(were_enabled);
    result
}

/// At the end of a system call: chooses again if the call asked this
/// CPU to (a more important thread made ready here). Only the hint is
/// read unless so; a request it misses - none should - is still taken
/// at the next interrupt, which checks in full (`preempt_if_needed`).
pub fn preempt_if_asked() {
    let were_enabled = arch::disable_interrupts();
    let asked = ASKED[arch::cpu()].swap(false, Ordering::Relaxed);
    arch::restore_interrupts(were_enabled);
    if asked {
        preempt_if_needed();
    }
}

/// Interrupts the CPUs the policy has asked to choose again: idling ones
/// offered a thread, busy ones running somebody less important. Called
/// after anything that can put a thread in a queue. Several CPUs share
/// the set the policy keeps, so this one may collect a bit another set,
/// even its own: its own is left out - it is not halted, and chooses
/// when it next looks (`ASKED`, the end of the interrupt).
fn tell_idle_cpus(policy: &Scheduler, cpu: usize) {
    let cpus = policy.take_pokes() & !(1 << cpu);
    if cpus != 0 {
        arch::smp::poke(cpus);
    }
}

/// Starts `f` on a new kernel thread.
pub fn spawn(name: &'static str, f: impl FnOnce() + Send + 'static) -> ThreadId {
    spawn_in(None, name, f)
}

/// Starts `f` on a new thread in `process`'s address space (or the
/// kernel's, for None).
pub fn spawn_in(
    process: Option<Arc<Process>>,
    name: &'static str,
    f: impl FnOnce() + Send + 'static,
) -> ThreadId {
    try_spawn_in(process, name, 0, f).expect("out of kernel stacks")
}

/// `spawn_in`, for a thread a program asked for: None, instead of a
/// panic, when there is no kernel stack for it; and with its thread
/// pointer.
pub fn try_spawn_in(
    process: Option<Arc<Process>>,
    name: &'static str,
    pointer: u64,
    f: impl FnOnce() + Send + 'static,
) -> Option<ThreadId> {
    let stack = mm::allocate_stack()?;
    // SAFETY: a freshly mapped stack, nobody else's.
    let rsp = unsafe { context::initial_stack(stack.top()) };
    // A program's new thread gets its process's priority and CPUs.
    let (priority, affinity) = process
        .as_ref()
        .map_or((kernel64::sched::DEFAULT, kernel64::sched::ANY_CPU), |p| (p.sched_priority() as i8, p.sched_affinity()));
    let thread = Thread::new(name, rsp, Some(stack), process, Some(Box::new(f)), pointer);
    let handle = thread.handle();
    let id = {
        let mut guard = THREADS.lock();
        let t = guard.as_mut().expect("threads not initialised");
        let id = t.next_id;
        t.next_id += 1;
        t.table.insert(id, thread);
        id
    };
    // In the table first: from here another CPU may run it, and reap it.
    with(|policy, cpu| policy.add_with_handle(id, cpu, priority, affinity, handle));
    Some(id)
}

/// Sets the running thread's thread pointer, in the CPU now and in its
/// record for when it runs again. With the lock held, interrupts are
/// off: the thread cannot be moved to another CPU between the two.
pub fn set_pointer(value: u64) {
    let were_enabled = arch::disable_interrupts();
    // SAFETY: the running thread's own field; only this CPU touches it.
    unsafe { (*here()).pointer = value };
    // SAFETY: only a program reads its thread pointer; the kernel uses
    // neither FS nor TPIDR_EL0.
    unsafe { context::load_thread_pointer(value) };
    arch::restore_interrupts(were_enabled);
}

/// SIGNAL_STACK: the running thread's signal stack, `size` bytes from
/// `base` (size 0: none).
pub fn set_signal_stack(base: u64, size: u64) -> Result<u64, abi::Error> {
    if size != 0 {
        if size < abi::SIGNAL_STACK_MIN {
            return Err(abi::Error::InvalidArgument);
        }
        if base.checked_add(size).is_none_or(|end| end > abi::USER_END) {
            return Err(abi::Error::BadAddress);
        }
    }
    let were_enabled = arch::disable_interrupts();
    // SAFETY: as in `set_pointer`.
    unsafe { (*here()).signal_stack = if size == 0 { (0, 0) } else { (base, size) } };
    arch::restore_interrupts(were_enabled);
    Ok(0)
}

/// The running thread's signal stack: (base, size), size 0 for none.
pub fn signal_stack() -> (u64, u64) {
    let were_enabled = arch::disable_interrupts();
    // SAFETY: as in `set_pointer`.
    let stack = unsafe { (*here()).signal_stack };
    arch::restore_interrupts(were_enabled);
    stack
}

/// The thread running on this CPU.
///
/// `arch::cpu()` is asked with interrupts off: a thread can be moved to
/// another CPU whenever it can be interrupted, and "this CPU" asked a
/// moment earlier may no longer be this one.
pub fn current() -> ThreadId {
    with(|policy, cpu| policy.current(cpu))
}

/// The process the running thread belongs to, if any.
pub fn current_process() -> Option<Arc<Process>> {
    let were_enabled = arch::disable_interrupts();
    // SAFETY: as in `charge`.
    let process = unsafe { (*here()).process.clone() };
    arch::restore_interrupts(were_enabled);
    process
}

/// The names of every thread but the idle threads, in creation order.
pub fn names() -> Vec<&'static str> {
    let policy = policy();
    let guard = THREADS.lock();
    let t = guard.as_ref().expect("threads not initialised");
    t.table.iter().filter(|(&id, _)| !policy.is_idle(id)).map(|(_, thread)| thread.name).collect()
}

/// How many CPUs run threads.
pub fn cpus() -> usize {
    policy().cpu_count()
}

/// Gives thread `id` a new priority (journal entry 124). A thread more
/// important than one running somewhere it may use takes that CPU: this
/// one when the call returns (syscall.rs), another by an interrupt.
pub fn set_priority(id: ThreadId, priority: i64) -> bool {
    with(|policy, cpu| policy.set_priority(id, priority as i8, cpu))
}

/// Gives thread `id` the CPUs it may run on; one on a CPU no longer
/// allowed leaves it.
pub fn set_affinity(id: ThreadId, affinity: u64) -> bool {
    with(|policy, cpu| policy.set_affinity(id, affinity, cpu))
}

/// Thread `id`'s priority and CPUs; None for an idle thread or none.
pub fn scheduling(id: ThreadId) -> Option<(i64, u64)> {
    with(|policy, _| Some((policy.priority(id)? as i64, policy.affinity(id)?)))
}

pub fn count() -> usize {
    with(|policy, _| policy.thread_count())
}

/// Lets the scheduler pick again - possibly this thread again, if nobody
/// else is ready.
pub fn yield_now() {
    schedule();
}

/// Says that the current thread will leave the CPU into `state` at the
/// next `schedule`. Interrupts must be off, and stay off until then.
fn announce(state: State) {
    with(|policy, cpu| policy.suspend_current(cpu, state));
    if let State::Sleeping(until) = state {
        // This CPU's timer, for this sleeper if it is due before the
        // next tick (K24). Interrupts are off: callers announce so.
        arch::timer_wake_by(crate::clock::counter_at(until));
    }
}

/// Nanoseconds in a tick.
const TICK_NS: u64 = 1_000_000_000 / interrupts::HZ;

/// Now, as sleepers' deadlines are kept: nanoseconds since the clock
/// started (K24, journal entry 128) - before it starts, the ticks so far
/// in nanoseconds, which is the same scale.
pub fn now_ns() -> u64 {
    if crate::clock::started() {
        crate::clock::monotonic()
    } else {
        interrupts::ticks() * TICK_NS
    }
}

/// Tick `tick`, as a deadline in nanoseconds: the tick count reaches it
/// exactly when the clock reaches this.
fn tick_ns(tick: u64) -> u64 {
    tick.saturating_mul(TICK_NS)
}

/// Sets this CPU's timer for its first sleeper, if that is before the
/// next tick. After a timer interrupt, which set it for the tick.
fn arm_for_sleepers(policy: &Scheduler) {
    if let Some(until) = policy.first_wake_on(arch::cpu()) {
        arch::timer_wake_by(crate::clock::counter_at(until));
    }
}

/// Takes that back: the thread looked, and has nothing to wait for.
fn stay() {
    with(|policy, cpu| policy.resume_current(cpu));
}

/// Sleeps for at least `ticks` timer ticks.
///
/// Less, if the thread's process is killed meanwhile: the kill wakes
/// it. And not at all if it has been killed already. The order is rule
/// 2 at the top of this file: first the thread is marked as sleeping,
/// then the kill flag is read. A kill before the mark is seen here; a
/// kill after it finds a sleeping thread and wakes it. There is no
/// moment in between.
///
/// A caught signal cuts it short as a kill does (see `interrupted`).
/// Returns whether the whole time was slept.
pub fn sleep(ticks: u64) -> bool {
    sleep_until_ns(tick_ns(interrupts::ticks() + ticks))
}

/// Sleeps until the clock reads `until` nanoseconds (`now_ns`), as
/// `sleep` does: cut short by a kill or a caught signal. Returns whether
/// the time came (K24, journal entry 128).
pub fn sleep_until_ns(until: u64) -> bool {
    let were_enabled = arch::disable_interrupts();
    announce(State::Sleeping(until));
    if interrupted() {
        stay();
    } else {
        schedule();
    }
    arch::restore_interrupts(were_enabled);
    now_ns() >= until
}

/// Ends the current thread.
pub fn exit() -> ! {
    arch::disable_interrupts();
    announce(State::Exited);
    // Counted before the switch: the thread that reaps it looks at the
    // table only when this is not 0.
    EXITS.fetch_add(1, Ordering::SeqCst);
    // Nobody is told yet: this thread is still on its stack. The thread
    // that runs next on this CPU reaps it and wakes whoever waits.
    schedule();
    unreachable!("an exited thread was scheduled again");
}

/// Waits until thread `id` is gone: exited, off its CPU, its stack and
/// its process's reference given back.
pub fn join(id: ThreadId) {
    // Kernel threads join; they belong to no process, so nothing cuts
    // their waits short.
    let done = EXITED.wait_while(|| with(|policy, _| policy.state(id).is_some()));
    debug_assert!(done, "join was interrupted");
}

/// Whether the running thread's process has been asked to end.
fn being_killed() -> bool {
    let were_enabled = arch::disable_interrupts();
    // SAFETY: as in `charge`.
    let killed = unsafe { (*here()).process.as_ref() }.is_some_and(|p| p.kill_requested());
    arch::restore_interrupts(were_enabled);
    killed
}

/// Whether a wait of the running thread's should end now, though what
/// it waits for has not come: its process is being killed, or has a
/// caught signal waiting (process.rs). The call gives up with
/// Interrupted, and on the way back to user mode the process ends, or
/// the signal is delivered.
fn interrupted() -> bool {
    being_killed() || crate::process::signal_waiting()
}

/// Makes a blocked or sleeping thread ready.
pub fn wake(id: ThreadId) -> bool {
    with(|policy, cpu| policy.wake(id, cpu))
}

/// Asks the policy for the next thread and switches to it, if it is a
/// different one.
fn schedule() {
    let were_enabled = arch::disable_interrupts();
    let cpu = arch::cpu();
    let policy = policy();
    // Count the leaving thread's time before the policy names the next.
    charge(cpu, crate::clock::monotonic());
    // Rule 1: marked before `pick_next` may queue it - from that moment
    // another CPU may take it, and must wait until it is saved.
    let leaving = here();
    // SAFETY: the running thread's record, which lives while it runs
    // (rule 4); its fields are this CPU's until `switched` clears the
    // flag set here. The same holds for every `*leaving` below.
    unsafe { (*leaving).leaving.store(true, Ordering::SeqCst) };
    let mut decision = policy.pick_next_with_handles(cpu);
    // Nothing chosen, yet asked to choose again: the policy passed over a
    // lower-numbered CPU's queue whose lock was busy (sched.rs, "Locks")
    // - or somebody asked meanwhile. Choose again now, with this CPU's
    // lock let go so that the other can finish, rather than keep the
    // thread here a whole slice: kept, a woken thread waited for the one
    // CPU whose queue it was in, 15 ms instead of 1-7 on four CPUs
    // (journal entry 153, bug 6). A few times, then as before.
    for _ in 0..RETRIES {
        if decision.is_some() || !policy.take_reschedule(cpu) {
            break;
        }
        decision = policy.pick_next_with_handles(cpu);
    }
    // The thread that gives way here may be one an idling CPU can run.
    tell_idle_cpus(policy, cpu);
    match decision {
        // SAFETY: as above; it stays, so it is still this CPU's.
        None => unsafe { (*leaving).leaving.store(false, Ordering::Release) },
        Some((Switch { .. }, from, to)) => {
            debug_assert_eq!(from, leaving as usize);
            // A program may change its own thread pointer on AArch64
            // (TPIDR_EL0 is writable at EL0): what it holds now is the
            // leaving thread's. On x86_64 only `set_pointer` changes it,
            // and the record already has it.
            if let Some(value) = context::save_thread_pointer() {
                // SAFETY: as above: not yet saved, so still this CPU's.
                unsafe { (*leaving).pointer = value };
            }
            // SAFETY: as above; only an address is taken here, written
            // by the switch before `switched` clears the flag.
            let save = unsafe { &raw mut (*leaving).rsp };
            let next = at(to);
            // Rule 1: its last CPU may still be saving it. That CPU holds
            // no lock while it does, and waits for nobody who waits for
            // this one: it took its own next thread from a queue before
            // this one could have been queued.
            // SAFETY: the policy gave this CPU the thread; its record
            // lives (rule 4: it has not exited). Only its atomic flag is
            // read until the flag says its last CPU is done with it.
            while unsafe { (*next).leaving.load(Ordering::Acquire) } {
                core::hint::spin_loop();
            }
            // SAFETY: its last CPU is done with it (Acquire above), and
            // no other CPU touches it while it is this CPU's to run.
            let next = unsafe { &*next };
            if next.process.is_some() {
                // SAFETY: as in `set_pointer`. Kernel threads leave it
                // as it is: it is only read in user mode.
                unsafe { context::load_thread_pointer(next.pointer) };
            }
            let load = next.rsp;
            let root = next.process.as_ref().map_or_else(mm::kernel_root, |p| p.root());
            // The next thread's world: its kernel stack for entries from
            // user mode, and its address space. Both are safe to change
            // here - this code and its stack are in the kernel half,
            // mapped in every address space.
            if let Some(top) = next.stack_top {
                syscall::set_kernel_stack(top);
            }
            // SeqCst: a CPU removing a mapping from these tables reads
            // this after its removal (tlb.rs); either it sees this CPU
            // here, or this CPU's walk of the tables sees the removal.
            if ACTIVE_ROOT[cpu].swap(root, Ordering::SeqCst) != root {
                // SAFETY: a process's tables share the kernel half.
                unsafe { mmu::activate(root) };
                // A different process (or none): its I/O ports. The
                // process is borrowed from the record, not cloned: a
                // clone dropped here could be the last reference, and
                // freeing a process is not something to do mid-switch.
                match &next.process {
                    Some(p) => arch::set_io_bitmap(&p.io_ports.lock()),
                    None => arch::set_io_bitmap(&PortBitmap::NONE),
                }
            }
            CURRENT[cpu].store(to, Ordering::Relaxed);
            PREVIOUS[cpu].store(from, Ordering::Relaxed);
            // SAFETY: interrupts are off; `save` is the leaving thread's,
            // which no other CPU loads until `switched` clears its flag;
            // `load` was saved before its flag was cleared (Acquire above).
            unsafe { context::switch(save, load) };
            // Running again - possibly much later, possibly on another
            // CPU, which has just switched here from some thread.
            switched();
        }
    }
    reap();
    arch::restore_interrupts(were_enabled);
}

/// How often `schedule` chooses again at once when the policy passed over
/// a busy queue.
const RETRIES: usize = 4;

/// Rule 1's second half, first thing after every switch, on the thread
/// switched to (here, or in `thread_entry` for a new thread): the thread
/// this CPU switched away from is saved now, and may be loaded anywhere.
fn switched() {
    let previous = PREVIOUS[arch::cpu()].load(Ordering::Relaxed);
    // SAFETY: the thread switched away from cannot be freed until this
    // clears its flag (rule 4: `reap` waits for it).
    let previous = unsafe { &*at(previous) };
    rule_1_holds(previous);
    previous.leaving.store(false, Ordering::Release);
}

/// Frees the stacks of exited threads, and tells whoever waits for them.
/// A thread cannot free the stack it is running on, so the next thread
/// to run does it - and only once no CPU is on that stack, which the
/// policy checks (`remove`).
fn reap() {
    if EXITS.load(Ordering::SeqCst) == 0 {
        return; // nobody to reap: no lock taken at all
    }
    let policy = policy();
    // Out of the table under its lock - so that of two CPUs reaping at
    // once, one takes each - but forgotten by the policy only after its
    // stack is freed: `join` waits for the policy to forget it, and must
    // then find the stack back. The other order let a joiner count the
    // free frames one stack short (journal entry 153, bug 9). Off its CPU
    // (`leaving` clear) is asked first: removable before that, it could
    // still be saving onto that stack.
    let dead: Vec<(ThreadId, Box<Thread>)> = {
        let mut guard = THREADS.lock();
        let t = guard.as_mut().expect("threads not initialised");
        let exited: Vec<ThreadId> = t
            .table
            .iter()
            .filter(|(&id, thread)| !thread.leaving.load(Ordering::Acquire) && policy.removable(id))
            .map(|(&id, _)| id)
            .collect();
        exited.into_iter().filter_map(|id| t.table.remove(&id).map(|thread| (id, thread))).collect()
    };
    if dead.is_empty() {
        return;
    }
    let count = dead.len();
    for (id, thread) in dead {
        drop(thread); // outside the lock: freeing stacks takes other locks
        let forgotten = policy.remove(id);
        debug_assert!(forgotten, "thread {id} reaped twice");
    }
    EXITS.fetch_sub(count, Ordering::SeqCst);
    EXITED.wake_all();
}

/// Ticks in which a CPU was found running something other than its
/// idle thread, over all CPUs: the work done, as against the time
/// passed. A sample, not a measurement - a thread that ran for most of
/// a tick and slept just before its end is not counted - but over a
/// second the samples add up to the truth.
static BUSY_TICKS: AtomicU64 = AtomicU64::new(0);

pub fn busy_ticks() -> u64 {
    BUSY_TICKS.load(Ordering::Relaxed)
}

/// One tick on this CPU: was it working?
fn count_if_busy(policy: &Scheduler) {
    if !policy.is_idle(policy.current(arch::cpu())) {
        BUSY_TICKS.fetch_add(1, Ordering::Relaxed);
    }
}

/// Called by the timer interrupt of the CPU that keeps the time, at each
/// tick: sleepers due by now (`now_ns`) are woken, and the slice counted.
pub fn on_tick() {
    if let Some(policy) = try_policy() {
        let cpu = arch::cpu();
        count_if_busy(policy);
        charge(cpu, crate::clock::monotonic());
        policy.tick(now_ns(), cpu);
        // Sleepers whose time came: this CPU keeps the time for all,
        // but need not run them all.
        tell_idle_cpus(policy, cpu);
        arm_for_sleepers(policy);
    }
}

/// Called by the timer interrupt of every other CPU.
pub fn on_tick_elsewhere() {
    if let Some(policy) = try_policy() {
        let cpu = arch::cpu();
        count_if_busy(policy);
        charge(cpu, crate::clock::monotonic());
        // Any CPU's tick wakes whoever is due (K24): a sleeper's own CPU
        // may be this one, and its timer set for it.
        policy.wake_sleepers(now_ns(), cpu);
        policy.tick_slice(cpu);
        tell_idle_cpus(policy, cpu);
        arm_for_sleepers(policy);
    }
}

/// An early timer interrupt, set for a sleeper due before this CPU's
/// tick (K24): wakes whoever is due, and sets the timer for the next.
pub fn on_timer_early() {
    if let Some(policy) = try_policy() {
        let cpu = arch::cpu();
        policy.wake_sleepers(now_ns(), cpu);
        tell_idle_cpus(policy, cpu);
        arm_for_sleepers(policy);
    }
}

/// Called at the end of every interrupt: switches if a slice ran out or a
/// wakeup made a thread ready while the CPU idled. Runs on the
/// interrupted thread's stack; the interrupt returns when this thread is
/// scheduled again.
pub fn preempt_if_needed() {
    let were_enabled = arch::disable_interrupts();
    let due = try_policy().is_some_and(|policy| policy.take_reschedule(arch::cpu()));
    arch::restore_interrupts(were_enabled);
    if due {
        schedule();
    }
}

/// Where every new thread starts (from `thread_trampoline`).
pub extern "C" fn thread_entry() -> ! {
    // The `schedule` that switched here has not finished rule 1: the
    // thread it left is saved now.
    switched();
    // A thread may first be switched to from inside an interrupt, with
    // interrupts still off; a running thread has them on.
    reap();
    // SAFETY: as in `set_pointer`.
    let entry = unsafe { (*here()).entry.take() };
    arch::enable_interrupts();
    if let Some(entry) = entry {
        entry();
    }
    exit()
}

/// Threads waiting for something - a key, a message, another thread's
/// exit.
pub struct WaitQueue {
    waiters: IrqLock<VecDeque<ThreadId>>,
}

impl WaitQueue {
    pub const fn new() -> WaitQueue {
        WaitQueue { waiters: IrqLock::new(VecDeque::new()) }
    }

    /// Blocks the current thread for as long as `condition` holds.
    ///
    /// A wakeup cannot slip in between the check and the blocking and be
    /// lost - the classic bug of "check, then sleep": the event arrives
    /// after the check, and the sleeper never hears of it. See
    /// `wait_any_until` for how.
    ///
    /// Returns false if the wait was cut short instead (see `wait_any`).
    #[must_use = "a wait can be cut short: check before relying on the condition"]
    pub fn wait_while(&self, condition: impl FnMut() -> bool) -> bool {
        wait_any(&[self], condition)
    }

    fn add(&self, id: ThreadId) {
        let mut waiters = self.waiters.lock();
        if !waiters.contains(&id) {
            waiters.push_back(id);
        }
    }

    fn remove(&self, id: ThreadId) {
        self.waiters.lock().retain(|&w| w != id);
    }

    /// Wakes every waiter; each re-checks its condition.
    ///
    /// Nobody is taken out of the queue here: a waiter leaves by itself
    /// when its wait is over. This used to empty the queue, and with
    /// several CPUs that lost wake-ups. A thread that has joined the
    /// queue but not yet gone to sleep cannot be woken - it is awake -
    /// so a wake-up that came for somebody else's reason only removed
    /// it. It then looked, found its own condition unchanged, slept,
    /// and was in no queue when its own wake-up came. (Several threads
    /// wait in the same queue for different things: every `join` waits
    /// in EXITED, each for another thread.)
    ///
    /// The queue's lock is held while waking, so that a waiter cannot
    /// leave and come back between being read here and being woken.
    pub fn wake_all(&self) {
        let waiters = self.waiters.lock();
        for &id in waiters.iter() {
            wake(id);
        }
    }

    fn holds(&self, id: ThreadId) -> bool {
        self.waiters.lock().contains(&id)
    }
}

/// The thread running on this CPU, by name and number - for a panic's
/// report. None if the table's lock is held (by this CPU, or by one the
/// panic has stopped) or there is no table yet: a panic must not wait.
pub fn current_for_report() -> Option<(&'static str, ThreadId)> {
    let policy = try_policy()?;
    let cpu = arch::cpu();
    let handle = CURRENT[cpu].load(Ordering::Relaxed);
    if handle == 0 {
        return None;
    }
    // SAFETY: as `here`: the thread this CPU runs, read with no lock - a
    // panic must not wait.
    Some((unsafe { &*at(handle) }.name, policy.current(cpu)))
}

/// Rule 1, checked at every switch: the thread just switched away from
/// was marked as leaving, by the `schedule` that switched away from it -
/// so no other CPU loaded it in the meantime. A `schedule` that forgot
/// to mark it (the mistake rule 1 is about) fails here at the first
/// switch, every boot, instead of two CPUs on one stack once in a long
/// while (journal entry 046 broke the old rule 1 on purpose, and twelve
/// boots passed). One atomic load.
fn rule_1_holds(previous: &Thread) {
    assert!(previous.leaving.load(Ordering::Relaxed), "rule 1 of threads.rs: a thread was switched from without being marked as leaving");
}

/// Rule 3, checked where the harm is done: a wake-up that lands between
/// a waiter joining a queue and saying it waits. No test can make a
/// wake-up land in that gap on purpose - it is two neighbouring steps of
/// `wait_any_until` (journal entry 046 tried) - but what the wake-up
/// leaves behind can be checked: the waiter must still be in the queue,
/// or its own wake-up later finds it in none and it sleeps for ever.
/// Here, as `wait_any_until` does: join the queue, and before saying
/// "I am waiting", be woken. (Waking a thread that is running does
/// nothing else: the policy's `wake` answers false.) For the boot
/// self-test (selftest.rs).
pub fn rule_3_holds() -> bool {
    let queue = WaitQueue::new();
    let me = current();
    queue.add(me);
    queue.wake_all();
    let held = queue.holds(me);
    queue.remove(me);
    held
}

/// Blocks for as long as `condition` holds, woken by any of `queues` -
/// waiting for whichever of several things happens first.
///
/// The thread joins every queue, sleeps, and on waking leaves the queues
/// that did not fire, so none keeps a stale entry. A wakeup from a queue
/// that has nothing to do with the condition is harmless: every wait
/// here re-checks its condition before it returns.
///
/// What a waker must do, for this to be safe: change whatever the
/// condition reads FIRST, and call `wake_all` after.
///
/// Returns true once the condition no longer holds. Returns false if
/// the thread's process is being killed or has a signal waiting
/// (`interrupted`): the wait is over, though what was waited for has
/// not happened. The caller must then give up and return to the system
/// call's exit, where the process ends or takes the signal - returning
/// normally, so that everything it holds on the way (references to
/// channels, to processes) is released as usual. Ending the thread
/// right here would strand them.
#[must_use = "a wait can be cut short: check before relying on the condition"]
pub fn wait_any(queues: &[&WaitQueue], condition: impl FnMut() -> bool) -> bool {
    wait_any_until(queues, None, condition) == Waited::Done
}

/// How a wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waited {
    /// The condition no longer holds: what was waited for happened.
    Done,
    /// The thread's process is being killed, or has a signal to take.
    Interrupted,
    /// The deadline came first.
    TimedOut,
}

/// `wait_any`, with a limit: gives up once the tick count reaches
/// `deadline`.
///
/// A wait with a deadline is a sleep that can be woken early. The
/// thread goes to sleep until the deadline - the scheduler wakes
/// sleepers when their tick comes - while also sitting in the queues,
/// whose wake-ups work on a sleeping thread as on a blocked one.
/// Whichever comes first, it then looks at the condition and the clock
/// again.
#[must_use = "a wait can be cut short: check before relying on the condition"]
pub fn wait_any_until(queues: &[&WaitQueue], deadline: Option<u64>, condition: impl FnMut() -> bool) -> Waited {
    wait_any_until_ns(queues, deadline.map(tick_ns), condition)
}

/// `wait_any_until`, the deadline in nanoseconds of the clock (`now_ns`)
/// rather than a tick (K24, journal entry 128).
pub fn wait_any_until_ns(queues: &[&WaitQueue], deadline: Option<u64>, mut condition: impl FnMut() -> bool) -> Waited {
    loop {
        // Interrupts off from here to the switch: this thread must not
        // be taken off its CPU by the timer while it is marked as
        // waiting, or it would not be put back.
        let were_enabled = arch::disable_interrupts();

        // Rule 2, in three steps. Where wakers will look for me:
        let me = current();
        for queue in queues {
            queue.add(me);
        }
        // "I am waiting" - so that a wake-up from now on has an effect:
        announce(deadline.map_or(State::Blocked, State::Sleeping));
        // And only now, look. Whatever happens after this look comes
        // with a wake-up that finds the two steps above already done.
        let ended = if !condition() {
            Some(Waited::Done)
        } else if interrupted() {
            Some(Waited::Interrupted)
        } else if deadline.is_some_and(|deadline| now_ns() >= deadline) {
            Some(Waited::TimedOut)
        } else {
            None
        };

        match ended {
            Some(_) => stay(),
            // If a wake-up came since the look, this returns at once:
            // the thread is no longer marked, and keeps its CPU.
            None => schedule(),
        }
        for queue in queues {
            queue.remove(me);
        }
        arch::restore_interrupts(were_enabled);
        if let Some(how) = ended {
            return how;
        }
    }
}
