//! Kernel threads: the mechanism around `kernel64::sched`'s decisions.
//!
//! Each thread has its own kernel stack (with a guard page below it), a
//! saved stack pointer while it is not running, and - until it starts -
//! the closure it will run. The scheduler policy says who runs next;
//! this module makes it happen with `arch::context::switch`.
//!
//! Everything that touches the thread table or switches runs with
//! interrupts disabled: a timer interrupt arriving halfway through a
//! switch would find the bookkeeping half-updated.
//!
//! With several CPUs that is no longer enough: interrupts off stops this
//! CPU from being interrupted, not another CPU from running. So there
//! is one lock, THREADS, around the table and the policy - and four
//! rules that exist only because of the other CPUs:
//!
//! 1. **The lock is held across the switch.** `schedule` decides under
//!    the lock, and does not release it: the thread it switches *to*
//!    does, once it is running. Released before the switch, another CPU
//!    could take the thread just put in the queue and run it while this
//!    CPU is still saving its registers - two CPUs on one stack.
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
//!    switched away.
//!
//! Threads do not belong to CPUs. There is one queue; a thread runs on
//! whichever CPU takes it next.

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

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

struct Threads {
    policy: Scheduler,
    /// Boxed so each Thread stays put: the switch writes `rsp` through a
    /// raw pointer after the table's lock is released.
    table: BTreeMap<ThreadId, Box<Thread>>,
    next_id: ThreadId,
}

static THREADS: IrqLock<Option<Threads>> = IrqLock::new(None);

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
    let stack_top = Some(stack.top());
    let idle = Thread {
        name: "idle",
        rsp,
        _stack: Some(stack),
        stack_top,
        process: None,
        entry: Some(Box::new(idle_loop)),
        pointer: 0,
        signal_stack: (0, 0),
    };
    let boot = Thread { name: "boot", rsp: 0, _stack: None, stack_top: None, process: None, entry: None, pointer: 0, signal_stack: (0, 0) };
    ACTIVE_ROOT[0].store(mm::kernel_root(), Ordering::Relaxed);

    let mut table = BTreeMap::new();
    table.insert(IDLE, Box::new(idle));
    table.insert(BOOT, Box::new(boot));
    *THREADS.lock() = Some(Threads { policy: Scheduler::with_running(IDLE, BOOT), table, next_id: 2 });
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
        arch::wait_for_interrupt();
    }
}

/// Gives the scheduler `count` more CPUs, numbered 1 to `count`: an
/// idle thread for each, already "running" there. Called once by the
/// boot CPU, before those CPUs are let go; each then calls
/// `run_idle`.
pub fn add_cpus(count: usize) {
    with(|t| {
        for cpu in 1..=count {
            let id = t.next_id;
            t.next_id += 1;
            // Like the boot thread: it runs on the stack the CPU was
            // started on, which nobody frees, and its `rsp` is filled in
            // the first time it is switched away from.
            let idle = Thread { name: "idle", rsp: 0, _stack: None, stack_top: None, process: None, entry: None, pointer: 0, signal_stack: (0, 0) };
            t.table.insert(id, Box::new(idle));
            assert_eq!(t.policy.add_cpu(id), cpu);
        }
    });
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
/// NNN).
fn charge(t: &Threads, cpu: usize, now: u64) {
    let since = RUNNING_SINCE[cpu].swap(now, Ordering::Relaxed);
    if since == 0 || now <= since {
        return;
    }
    if let Some(process) = t.table.get(&t.policy.current(cpu)).and_then(|thread| thread.process.as_ref()) {
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
    let answer = with(|t| {
        let process = t.table.get(&t.policy.current(cpu)).and_then(|thread| thread.process.as_ref())?;
        let since = RUNNING_SINCE[cpu].load(Ordering::Relaxed);
        let running = if since != 0 && now > since { now - since } else { 0 };
        Some(process.cpu_ns() + running)
    });
    arch::restore_interrupts(were_enabled);
    answer
}

fn with<R>(f: impl FnOnce(&mut Threads) -> R) -> R {
    let mut guard = THREADS.lock();
    let t = guard.as_mut().expect("threads not initialised");
    let result = f(t);
    tell_idle_cpus(t);
    result
}

/// Interrupts the idling CPUs the policy has just offered a thread to.
/// Called before the lock is let go, after anything that can put a
/// thread in the queue. The interrupted CPU will want this same lock,
/// and waits for it - a few instructions.
fn tell_idle_cpus(t: &mut Threads) {
    let cpus = t.policy.take_pokes();
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
    let stack_top = Some(stack.top());
    let thread =
        Box::new(Thread { name, rsp, _stack: Some(stack), stack_top, process, entry: Some(Box::new(f)), pointer, signal_stack: (0, 0) });
    Some(with(|t| {
        let id = t.next_id;
        t.next_id += 1;
        t.table.insert(id, thread);
        t.policy.add(id, arch::cpu());
        id
    }))
}

/// Sets the running thread's thread pointer, in the CPU now and in its
/// record for when it runs again. With the lock held, interrupts are
/// off: the thread cannot be moved to another CPU between the two.
pub fn set_pointer(value: u64) {
    with(|t| {
        let id = t.policy.current(arch::cpu());
        if let Some(thread) = t.table.get_mut(&id) {
            thread.pointer = value;
        }
        // SAFETY: only a program reads its thread pointer; the kernel
        // uses neither FS nor TPIDR_EL0.
        unsafe { context::load_thread_pointer(value) };
    });
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
    with(|t| {
        let id = t.policy.current(arch::cpu());
        if let Some(thread) = t.table.get_mut(&id) {
            thread.signal_stack = if size == 0 { (0, 0) } else { (base, size) };
        }
    });
    Ok(0)
}

/// The running thread's signal stack: (base, size), size 0 for none.
pub fn signal_stack() -> (u64, u64) {
    with(|t| {
        let id = t.policy.current(arch::cpu());
        t.table.get(&id).map_or((0, 0), |thread| thread.signal_stack)
    })
}

/// The thread running on this CPU.
///
/// `arch::cpu()` is asked inside the lock, with interrupts off: a
/// thread can be moved to another CPU whenever it can be interrupted,
/// and "this CPU" asked a moment earlier may no longer be this one.
pub fn current() -> ThreadId {
    with(|t| t.policy.current(arch::cpu()))
}

/// The process the running thread belongs to, if any.
pub fn current_process() -> Option<Arc<Process>> {
    with(|t| t.table.get(&t.policy.current(arch::cpu())).and_then(|thread| thread.process.clone()))
}

/// The names of every thread but the idle threads, in creation order.
pub fn names() -> Vec<&'static str> {
    with(|t| t.table.iter().filter(|(&id, _)| !t.policy.is_idle(id)).map(|(_, thread)| thread.name).collect())
}

/// How many CPUs run threads.
pub fn cpus() -> usize {
    with(|t| t.policy.cpu_count())
}

pub fn count() -> usize {
    with(|t| t.policy.thread_count())
}

/// Lets the scheduler pick again - possibly this thread again, if nobody
/// else is ready.
pub fn yield_now() {
    schedule();
}

/// Says that the current thread will leave the CPU into `state` at the
/// next `schedule`. Interrupts must be off, and stay off until then.
fn announce(state: State) {
    with(|t| t.policy.suspend_current(arch::cpu(), state));
}

/// Takes that back: the thread looked, and has nothing to wait for.
fn stay() {
    with(|t| t.policy.resume_current(arch::cpu()));
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
    let were_enabled = arch::disable_interrupts();
    let until = interrupts::ticks() + ticks;
    announce(State::Sleeping(until));
    if interrupted() {
        stay();
    } else {
        schedule();
    }
    arch::restore_interrupts(were_enabled);
    interrupts::ticks() >= until
}

/// Ends the current thread.
pub fn exit() -> ! {
    arch::disable_interrupts();
    announce(State::Exited);
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
    let done = EXITED.wait_while(|| with(|t| t.policy.state(id).is_some()));
    debug_assert!(done, "join was interrupted");
}

/// Whether the running thread's process has been asked to end.
fn being_killed() -> bool {
    with(|t| {
        t.table
            .get(&t.policy.current(arch::cpu()))
            .and_then(|thread| thread.process.as_ref())
            .is_some_and(|p| p.kill_requested())
    })
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
    with(|t| t.policy.wake(id, arch::cpu()))
}

/// Asks the policy for the next thread and switches to it, if it is a
/// different one.
fn schedule() {
    let were_enabled = arch::disable_interrupts();
    let mut guard = THREADS.lock();
    let t = guard.as_mut().expect("threads not initialised");
    let cpu = arch::cpu();
    // Count the leaving thread's time before the policy names the next.
    charge(t, cpu, crate::clock::monotonic());
    let switch = t.policy.pick_next(cpu);
    // The thread that gives way here may be one an idling CPU can run.
    tell_idle_cpus(t);
    match switch {
        None => drop(guard),
        Some(Switch { from, to }) => {
            let leaving = t.table.get_mut(&from).expect("current thread missing");
            // A program may change its own thread pointer on AArch64
            // (TPIDR_EL0 is writable at EL0): what it holds now is the
            // leaving thread's. On x86_64 only `set_pointer` changes it,
            // and the record already has it.
            if let Some(value) = context::save_thread_pointer() {
                leaving.pointer = value;
            }
            let save = &raw mut leaving.rsp;
            let next = t.table.get(&to).expect("next thread missing");
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
                // process is borrowed from the table, not cloned: a
                // clone dropped here could be the last reference, and
                // freeing a process is not something to do under this
                // lock.
                match &next.process {
                    Some(p) => arch::set_io_bitmap(&p.io_ports.lock()),
                    None => arch::set_io_bitmap(&PortBitmap::NONE),
                }
            }
            // Rule 1: the lock stays held. Forgetting the guard keeps it
            // locked without a guard to drop; the thread switched to
            // unlocks it - just below if it was once switched away from
            // here, or in `thread_entry` if it is new.
            core::mem::forget(guard);
            // SAFETY: interrupts are off and the lock is held, so no
            // other CPU can pick either thread or free either record
            // until this CPU is on the new stack.
            unsafe { context::switch(save, load) };
            // Running again - possibly much later, possibly on another
            // CPU - holding the lock some other `schedule` took.
            // SAFETY: that `schedule` forgot its guard for exactly this.
            unsafe { THREADS.force_unlock() };
        }
    }
    reap();
    arch::restore_interrupts(were_enabled);
}

/// Frees the stacks of exited threads, and tells whoever waits for them.
/// A thread cannot free the stack it is running on, so the next thread
/// to run does it - and only once no CPU is on that stack, which the
/// policy checks (`remove`).
fn reap() {
    let dead: Vec<Box<Thread>> = with(|t| {
        let exited: Vec<ThreadId> =
            t.table.keys().copied().filter(|&id| t.policy.remove(id)).collect();
        exited.iter().filter_map(|id| t.table.remove(id)).collect()
    });
    if !dead.is_empty() {
        drop(dead); // outside the lock: freeing stacks takes other locks
        EXITED.wake_all();
    }
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
fn count_if_busy(t: &Threads) {
    if !t.policy.is_idle(t.policy.current(arch::cpu())) {
        BUSY_TICKS.fetch_add(1, Ordering::Relaxed);
    }
}

/// Called by the timer interrupt of the CPU that keeps the time: `now`
/// is the new tick count.
pub fn on_tick(now: u64) {
    if let Some(t) = THREADS.lock().as_mut() {
        count_if_busy(t);
        charge(t, arch::cpu(), crate::clock::monotonic());
        t.policy.tick(now, arch::cpu());
        // Sleepers whose time came: this CPU keeps the time for all,
        // but need not run them all.
        tell_idle_cpus(t);
    }
}

/// Called by the timer interrupt of every other CPU.
pub fn on_tick_elsewhere() {
    if let Some(t) = THREADS.lock().as_mut() {
        count_if_busy(t);
        charge(t, arch::cpu(), crate::clock::monotonic());
        t.policy.tick_slice(arch::cpu());
    }
}

/// Called at the end of every interrupt: switches if a slice ran out or a
/// wakeup made a thread ready while the CPU idled. Runs on the
/// interrupted thread's stack; the interrupt returns when this thread is
/// scheduled again.
pub fn preempt_if_needed() {
    let due = THREADS.lock().as_mut().is_some_and(|t| t.policy.take_reschedule(arch::cpu()));
    if due {
        schedule();
    }
}

/// Where every new thread starts (from `thread_trampoline`).
pub extern "C" fn thread_entry() -> ! {
    // The `schedule` that switched here left the lock held (rule 1).
    // SAFETY: it forgot its guard so that this thread would unlock.
    unsafe { THREADS.force_unlock() };
    // A thread may first be switched to from inside an interrupt, with
    // interrupts still off; a running thread has them on.
    reap();
    let entry = with(|t| {
        let id = t.policy.current(arch::cpu());
        t.table.get_mut(&id).and_then(|thread| thread.entry.take())
    });
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
pub fn wait_any_until(queues: &[&WaitQueue], deadline: Option<u64>, mut condition: impl FnMut() -> bool) -> Waited {
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
        } else if deadline.is_some_and(|deadline| interrupts::ticks() >= deadline) {
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
