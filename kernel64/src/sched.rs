//! The scheduler's decisions, separate from the mechanism.
//!
//! Which thread runs next is pure bookkeeping: queues and states. How
//! the CPU actually switches from one thread to another - saving
//! registers, changing stacks - is assembly in the kernel binary. Keeping
//! them apart means every scheduling rule here is tested on the host,
//! where a wrong decision is an assertion failure rather than a hang.
//!
//! The policy is round-robin with time slices and priorities, on any
//! number of CPUs:
//!   - each thread has a priority, from HIGHEST (-20) to LOWEST (19),
//!     DEFAULT 0, and an affinity: the CPUs it may run on (journal entry
//!     124). Runnable threads wait in a queue per CPU (K68, journal entry
//!     129), a FIFO per priority in each. A CPU that needs a thread takes
//!     the most important one allowed on it, from any CPU's queue, and of
//!     those the one that joined a queue first: as fair as one queue for
//!     all, and an idle CPU takes work from a busy one.
//!   - each CPU has a running thread, which gets SLICE timer ticks;
//!     when they run out and someone at least as important is waiting,
//!     it goes to the back of its queue. A less important thread never
//!     takes a CPU from a more important one: priorities are strict, so
//!     a busy important thread that never waits leaves nothing for the
//!     less important ones (shares in proportion are K100's).
//!   - a thread made ready that is more important than one running on a
//!     CPU it may use, with no allowed CPU idling, takes that CPU at
//!     once: the CPU is asked to choose again (and interrupted if it is
//!     another), rather than the thread waiting out a slice
//!   - a thread can also give up the CPU early (yield), sleep until a
//!     tick, block until woken, or exit
//!   - each CPU has an idle thread, which runs only when nothing else
//!     may. It never sits in a queue, and never runs on another CPU.
//!
//! One rule exists only because there is more than one CPU. A thread
//! that has said it will block is, for a moment, still running: it has
//! changed its state but not yet left its CPU. If it is woken in that
//! moment it must NOT go into a queue - another CPU would take it from
//! there and run it while the first CPU is still on its stack. It simply
//! becomes runnable again, and when it gets to the switch it finds it
//! has no reason to leave.
//!
//! ## Locks (K193, journal entry NNN)
//!
//! Until K193 the caller held one lock around every call here, on every
//! CPU, and across every switch. Now the policy locks for itself, and
//! finely, so that two CPUs share a lock only when they touch the same
//! thing:
//!
//! - **each CPU's lock** covers its queue, the thread on it, its slice,
//!   and `rq` of every thread it runs or queues (below). A switch takes
//!   only its own CPU's lock - unless it takes a thread from another
//!   CPU's queue, and then that one's too, for a moment;
//! - **each CPU's sleepers** have a small lock of their own;
//! - **each thread's settings lock** is taken by whoever changes the
//!   thread from outside - a wake-up, a new priority. Two CPUs waking the
//!   same thread take turns, so it is queued once;
//! - **the directory** (thread number to its record) is locked only for
//!   a look-up, an addition or a removal - never while deciding.
//!
//! The order, so that no two CPUs ever wait for each other: a thread's
//! settings, then one CPU's lock, then (sleepers aside) nothing. A
//! second CPU's lock is waited for only if its number is higher than the
//! one held; a lower one is only *tried*, a few times, and passed over if
//! busy. Somebody holding a higher-numbered lock never waits for a lower
//! one, so whoever waits for a higher one is waiting for somebody who
//! will finish. A CPU that passes a queue over asks itself to choose
//! again (`reschedule`), and the mechanism does so at once, its own lock
//! let go (threads.rs, `schedule`): kept for a whole new slice instead,
//! a woken thread waited for the one CPU whose queue it was in.
//!
//! **Peeking** (chosen for K193 over taking from a CPU's own queue
//! first, which entry 129 found unfair, and over one lock). Choosing
//! FIFO across every queue used to read every queue under the one lock.
//! Now each queue *publishes* the key of its first thread - its priority
//! level and when it joined - in an atomic, after every change, under its
//! lock. A choosing CPU reads the published keys of the others without
//! their locks, compares them with its own queue's best, and locks only
//! the queue it then takes from, where it checks again. The choice is
//! exact unless two CPUs change queues at the same instant; then it can
//! be one thread off, never one that must not run.
//!
//! **Nobody sleeps with work waiting** - the same race as rule 2 of
//! threads.rs, between CPUs. A CPU about to idle says so (`idling`)
//! *before* it looks at the queues one last time; a thread queued says
//! so (its queue's published key) *before* it looks for an idling CPU to
//! tell. Both are sequentially consistent, so at least one of them sees
//! the other: the idler finds the thread, or the queuer interrupts the
//! idler.
//!
//! **A thread leaves its CPU after it is queued.** A thread that gives
//! way is queued at once, before its CPU has saved its registers; the
//! one lock held across the switch (rule 1) used to stop another CPU
//! from taking it in that moment. Now the mechanism keeps a "leaving"
//! flag per thread, set when it is switched from and cleared once its
//! registers are saved; a CPU that took it waits for the flag before it
//! loads them (threads.rs, `schedule`). The policy needs nothing for it.

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::sync::Arc;
#[cfg(test)]
use alloc::vec::Vec;
use core::hint;
use core::sync::atomic::{fence, AtomicBool, AtomicI8, AtomicU64, AtomicU8, AtomicUsize, Ordering::SeqCst};

use crate::smp::MAX_CPUS;
use crate::sync::{SpinLock, SpinLockGuard};

pub type ThreadId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Running,
    Ready,
    /// Until the given tick.
    Sleeping(u64),
    /// Until someone calls `wake`.
    Blocked,
    /// Finished; its resources are freed once it is off the CPU.
    Exited,
}

/// How many ticks a thread may run before others get a turn.
pub const SLICE: u64 = 2;

/// Priorities: lower is more important, as with `nice`.
pub type Priority = i8;
pub const HIGHEST: Priority = -20;
pub const LOWEST: Priority = 19;
pub const DEFAULT: Priority = 0;
/// How many priorities there are: one queue each.
const LEVELS: usize = (LOWEST - HIGHEST + 1) as usize;
/// Every CPU, as an affinity.
pub const ANY_CPU: u64 = u64::MAX;

/// What an idle thread counts as, against a priority: less important
/// than any, so that anyone ready may take its CPU.
const IDLE_PRIORITY: Priority = LOWEST + 1;

/// `rq` of a thread that is on no CPU and in no queue.
const NOWHERE: usize = usize::MAX;

/// A thread's `kind`: what it is doing as far as choosing is concerned.
/// Ready and Running are one kind, as in Linux: which of the two a thread
/// is depends on where it is (`rq`, the CPU's current thread), which the
/// CPU's lock covers - not on something a waker would have to change
/// under that lock.
const RUNNABLE: u8 = 0;
const BLOCKED: u8 = 1;
const SLEEPING: u8 = 2;
const EXITED: u8 = 3;

/// What only the thread itself, and whoever holds its settings lock,
/// changes.
struct Settings {
    priority: Priority,
    /// One bit per CPU it may run on.
    affinity: u64,
    /// The CPU it last left to wait (K101): where its data may still be
    /// in the cache, and where it is put back when woken, if it may.
    last: usize,
    /// While Sleeping: until when, and the CPU whose set it is in.
    until: u64,
    asleep_on: usize,
}

/// What the scheduler keeps for each thread.
struct Rec {
    id: ThreadId,
    /// An idle thread: never queued, on one CPU only, no settings.
    idle: bool,
    /// What the mechanism gave to find its own record of the thread
    /// without a look-up (threads.rs: the address of its `Thread`). Set
    /// before the thread is first queued, never changed.
    handle: usize,
    /// RUNNABLE, BLOCKED, SLEEPING or EXITED. Changed by the thread
    /// itself (to wait, or to take a wait back), and by a waker holding
    /// `settings` (back to RUNNABLE). The CPU it runs on reads it when
    /// choosing, under that CPU's lock only.
    kind: AtomicU8,
    /// The CPU whose lock covers where the thread is: the one it runs on
    /// or whose queue it waits in; NOWHERE if it is in neither. Changed
    /// only by someone holding that CPU's lock (and, when the thread
    /// moves, the other CPU's too): so a reader that locks the CPU it
    /// read here, and finds the same number again, has it fixed.
    rq: AtomicUsize,
    settings: SpinLock<Settings>,
}

impl Rec {
    fn new(id: ThreadId, priority: Priority, affinity: u64, last: usize, handle: usize) -> Arc<Rec> {
        let settings = Settings { priority, affinity, last, until: 0, asleep_on: 0 };
        Arc::new(Rec {
            id,
            idle: false,
            handle,
            kind: AtomicU8::new(RUNNABLE),
            rq: AtomicUsize::new(NOWHERE),
            settings: SpinLock::new(settings),
        })
    }

    fn idle(id: ThreadId, cpu: usize, handle: usize) -> Arc<Rec> {
        let settings = Settings { priority: IDLE_PRIORITY, affinity: 1 << cpu, last: cpu, until: 0, asleep_on: 0 };
        Arc::new(Rec {
            id,
            idle: true,
            handle,
            kind: AtomicU8::new(RUNNABLE),
            rq: AtomicUsize::new(cpu),
            settings: SpinLock::new(settings),
        })
    }
}

/// One queued thread: its record, the affinity it was queued with (so
/// that choosing looks nothing up) and when it joined.
struct Entry {
    rec: Arc<Rec>,
    affinity: u64,
    joined: u64,
}

/// A key to compare queued threads by, the smaller first: the priority
/// level in the top byte, when it joined below. What a queue publishes.
fn key(level: usize, joined: u64) -> u64 {
    ((level as u64) << 56) | (joined & ((1 << 56) - 1))
}

fn level_of_key(key: u64) -> usize {
    (key >> 56) as usize
}

/// The key of an empty queue: after every other.
const EMPTY: u64 = u64::MAX;

/// One CPU's runnable threads that are on no CPU: a FIFO queue per
/// priority, the most important first.
pub struct Queue {
    levels: [VecDeque<Entry>; LEVELS],
    /// One bit per level that has anyone in it, so that finding the
    /// first is one instruction (trailing_zeros), not forty looks.
    occupied: u64,
    /// How many wait here, all levels together: what `home` compares.
    count: usize,
}

impl Queue {
    fn new() -> Queue {
        Queue { levels: core::array::from_fn(|_| VecDeque::new()), occupied: 0, count: 0 }
    }

    fn push_back(&mut self, entry: Entry, priority: Priority) {
        let level = level(priority);
        self.levels[level].push_back(entry);
        self.occupied |= 1 << level;
        self.count += 1;
    }

    /// One fewer at `level`: clears its bit if it has emptied.
    fn update(&mut self, level: usize) {
        self.count -= 1;
        if self.levels[level].is_empty() {
            self.occupied &= !(1 << level);
        }
    }

    /// Takes `id` out, wherever it is.
    fn remove(&mut self, id: ThreadId) -> Option<Entry> {
        for level in 0..LEVELS {
            if let Some(at) = self.levels[level].iter().position(|entry| entry.rec.id == id) {
                let entry = self.levels[level].remove(at);
                self.update(level);
                return entry;
            }
        }
        None
    }

    /// Where the thread `cpu` would take next is: its priority's level
    /// and its place there - the most important allowed on `cpu`, no
    /// less important than `max` (a level), the longest waiting of those.
    fn best_for(&self, cpu: usize, max: usize) -> Option<(usize, usize)> {
        let bit = 1u64 << cpu;
        let mut left = self.occupied & (u64::MAX >> (63 - max));
        while left != 0 {
            let level = left.trailing_zeros() as usize;
            if let Some(at) = self.levels[level].iter().position(|entry| entry.affinity & bit != 0) {
                return Some((level, at));
            }
            left &= left - 1;
        }
        None
    }

    fn key_at(&self, (level, at): (usize, usize)) -> u64 {
        key(level, self.levels[level][at].joined)
    }

    /// The key of the first thread, whoever may run it: what is published.
    fn head(&self) -> u64 {
        if self.occupied == 0 {
            return EMPTY;
        }
        let level = self.occupied.trailing_zeros() as usize;
        key(level, self.levels[level][0].joined)
    }

    /// Takes the thread at `place` out: it, and its priority.
    fn take(&mut self, (level, at): (usize, usize)) -> (Entry, Priority) {
        let entry = self.levels[level].remove(at).expect("best_for's place");
        self.update(level);
        (entry, priority_of(level))
    }

    /// In the order they would be taken: the most important first, and
    /// within a priority the longest waiting first.
    #[cfg(test)]
    fn ids(&self) -> impl Iterator<Item = ThreadId> + '_ {
        self.levels.iter().flatten().map(|entry| entry.rec.id)
    }
}

fn level(priority: Priority) -> usize {
    (priority.clamp(HIGHEST, LOWEST) - HIGHEST) as usize
}

fn priority_of(level: usize) -> Priority {
    level as Priority + HIGHEST
}

fn bit(cpu: usize) -> u64 {
    1u64 << cpu
}

/// What one CPU's lock covers.
struct Local {
    queue: Queue,
    /// The thread on this CPU now - until the next `pick_next`, even if
    /// it has already said it will sleep, block or exit.
    current: Arc<Rec>,
    idle: Arc<Rec>,
    slice_left: u64,
    /// The current thread's priority (IDLE_PRIORITY for the idle
    /// thread) and affinity, kept here so that a switch looks nothing up.
    priority: Priority,
    affinity: u64,
}

/// What the scheduler keeps for each CPU: what its lock covers, and what
/// it publishes for the other CPUs to read without it.
struct Cpu {
    local: SpinLock<Option<Local>>,
    /// The sleepers that went to sleep on this CPU, sorted by when they
    /// wake (then by number): a tick looks at the first, not at every
    /// thread (K96, journal entry 128), and each CPU can set its timer
    /// for its own first sleeper, so that a sleep costs one interrupt,
    /// not one on every CPU (K24).
    sleepers: SpinLock<BTreeSet<(u64, ThreadId)>>,
    /// Published under `local`'s lock, after every change: its queue's
    /// first key (EMPTY if none), and how many wait there.
    head: AtomicU64,
    waiting: AtomicUsize,
    /// Published under the lock at every switch: the thread on it, its
    /// priority, and whether it idles.
    current: AtomicU64,
    priority: AtomicI8,
    idling: AtomicBool,
    /// Its idle thread's number (fixed once the CPU is added).
    idle: AtomicU64,
    /// Set when this CPU should choose again as soon as possible - a
    /// wakeup while it idles, or the end of a slice. On an idling CPU
    /// it is also a promise: a queued thread has been offered to this
    /// CPU, so the next one is offered to somebody else. Anyone may set
    /// it; only the CPU clears it.
    reschedule: AtomicBool,
}

impl Cpu {
    fn new() -> Cpu {
        Cpu {
            local: SpinLock::new(None),
            sleepers: SpinLock::new(BTreeSet::new()),
            head: AtomicU64::new(EMPTY),
            waiting: AtomicUsize::new(0),
            current: AtomicU64::new(u64::MAX),
            priority: AtomicI8::new(IDLE_PRIORITY),
            idling: AtomicBool::new(false),
            idle: AtomicU64::new(u64::MAX),
            reschedule: AtomicBool::new(false),
        }
    }
}

type Guard<'a> = SpinLockGuard<'a, Option<Local>>;

fn local<'a>(guard: &'a mut Guard<'_>) -> &'a mut Local {
    guard.as_mut().expect("a CPU that was never added")
}

/// Nothing in the kernel. In the host tests, where two CPUs race -
/// between looking and saying so - a moment's pause, so that the
/// CPUs-at-once tests meet the bad order often rather than once in
/// millions; or, if a test has set one for this `site`, what another CPU
/// does at exactly that moment (`at_race`), once.
#[inline(always)]
fn race_window(site: &'static str) {
    #[cfg(test)]
    {
        let hook = RACE.with(|race| {
            let mut race = race.borrow_mut();
            if race.as_ref().is_some_and(|(at, _)| *at == site) { race.take() } else { None }
        });
        match hook {
            Some((_, act)) => act(),
            None => std::thread::yield_now(),
        }
    }
    #[cfg(not(test))]
    let _ = site;
}

#[cfg(test)]
std::thread_local! {
    static RACE: core::cell::RefCell<Option<(&'static str, Box<dyn FnOnce()>)>> = const { core::cell::RefCell::new(None) };
}

/// For the host tests: `act` is done the next time this host thread
/// reaches `race_window(site)`.
#[cfg(test)]
fn at_race(site: &'static str, act: impl FnOnce() + 'static) {
    RACE.with(|race| *race.borrow_mut() = Some((site, Box::new(act))));
}

/// How often a busy lower-numbered CPU's lock is tried before it is
/// passed over (see "Locks" above).
const TRIES: usize = 64;

pub struct Scheduler {
    /// MAX_CPUS of them from the start, so that adding a CPU moves
    /// nothing another CPU may be reading; `count` of them in use.
    cpus: Box<[Cpu]>,
    count: AtomicUsize,
    /// Every thread's record, by number.
    threads: SpinLock<BTreeMap<ThreadId, Arc<Rec>>>,
    /// Counts every thread that joins a queue: among equally important
    /// threads, the one that joined first is taken first, whichever
    /// CPU's queue it waits in.
    joined: AtomicU64,
    /// The CPUs that must be interrupted, one bit each: idling CPUs
    /// that were offered a thread by another CPU and cannot know it.
    /// The policy cannot interrupt anyone; the caller collects this
    /// with `take_pokes` and does.
    pokes: AtomicU64,
}

/// A switch the caller must carry out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Switch {
    pub from: ThreadId,
    pub to: ThreadId,
}

impl Scheduler {
    /// A scheduler for one CPU whose only thread is `idle`, already
    /// running.
    pub fn new(idle: ThreadId) -> Scheduler {
        Scheduler::with_running(idle, idle)
    }

    /// A scheduler for one CPU with `running` on it now (the boot
    /// thread) and `idle` waiting in the wings for when nothing else
    /// can run.
    pub fn with_running(idle: ThreadId, running: ThreadId) -> Scheduler {
        Scheduler::with_running_handles(idle, 0, running, 0)
    }

    /// `with_running`, with the mechanism's handles of the two threads.
    pub fn with_running_handles(idle: ThreadId, idle_handle: usize, running: ThreadId, running_handle: usize) -> Scheduler {
        let s = Scheduler {
            cpus: (0..MAX_CPUS).map(|_| Cpu::new()).collect(),
            count: AtomicUsize::new(0),
            threads: SpinLock::new(BTreeMap::new()),
            joined: AtomicU64::new(0),
            pokes: AtomicU64::new(0),
        };
        let idle_rec = Rec::idle(idle, 0, idle_handle);
        s.threads.lock().insert(idle, idle_rec.clone());
        let (current, priority) = if running == idle {
            (idle_rec.clone(), IDLE_PRIORITY)
        } else {
            let rec = Rec::new(running, DEFAULT, ANY_CPU, 0, running_handle);
            rec.rq.store(0, SeqCst);
            s.threads.lock().insert(running, rec.clone());
            (rec, DEFAULT)
        };
        let l = Local { queue: Queue::new(), current, idle: idle_rec, slice_left: SLICE, priority, affinity: ANY_CPU };
        s.cpus[0].idle.store(idle, SeqCst);
        s.publish_current(0, &l);
        *s.cpus[0].local.lock() = Some(l);
        s.count.store(1, SeqCst);
        s
    }

    /// One more CPU, running its idle thread `idle`. Returns its number:
    /// CPUs are numbered in the order they are added, the first being 0.
    /// Called before the new CPU runs anything; others may be running.
    pub fn add_cpu(&self, idle: ThreadId) -> usize {
        self.add_cpu_handle(idle, 0)
    }

    /// `add_cpu`, with the mechanism's handle of its idle thread.
    pub fn add_cpu_handle(&self, idle: ThreadId, handle: usize) -> usize {
        let cpu = self.count.load(SeqCst);
        assert!(cpu < MAX_CPUS, "more than {MAX_CPUS} CPUs");
        let rec = Rec::idle(idle, cpu, handle);
        assert!(self.threads.lock().insert(idle, rec.clone()).is_none(), "thread {idle} added twice");
        let l = Local { queue: Queue::new(), current: rec.clone(), idle: rec, slice_left: SLICE, priority: IDLE_PRIORITY, affinity: ANY_CPU };
        let this = &self.cpus[cpu];
        this.idle.store(idle, SeqCst);
        self.publish_current(cpu, &l);
        *this.local.lock() = Some(l);
        // A CPU that arrives when there is already work takes some at
        // its first chance.
        let work = (0..cpu).any(|c| self.cpus[c].head.load(SeqCst) != EMPTY);
        this.reschedule.store(work, SeqCst);
        self.count.store(cpu + 1, SeqCst);
        cpu
    }

    pub fn cpu_count(&self) -> usize {
        self.count.load(SeqCst)
    }

    /// The thread on `cpu`. Exact on `cpu` itself, with interrupts off:
    /// only `cpu` changes it.
    pub fn current(&self, cpu: usize) -> ThreadId {
        self.cpus[cpu].current.load(SeqCst)
    }

    /// The mechanism's handle of the thread on `cpu` (as exact as
    /// `current`).
    pub fn current_handle(&self, cpu: usize) -> usize {
        local(&mut self.cpus[cpu].local.lock()).current.handle
    }

    /// The CPU `id` is on, if it is on one.
    pub fn cpu_of(&self, id: ThreadId) -> Option<usize> {
        (0..self.cpu_count()).find(|&c| self.cpus[c].current.load(SeqCst) == id)
    }

    /// Whether `id` is some CPU's idle thread.
    pub fn is_idle(&self, id: ThreadId) -> bool {
        (0..self.cpu_count()).any(|c| self.cpus[c].idle.load(SeqCst) == id)
    }

    fn rec(&self, id: ThreadId) -> Option<Arc<Rec>> {
        self.threads.lock().get(&id).cloned()
    }

    pub fn state(&self, id: ThreadId) -> Option<State> {
        let rec = self.rec(id)?;
        Some(match rec.kind.load(SeqCst) {
            RUNNABLE if self.cpu_of(id).is_some() => State::Running,
            RUNNABLE => State::Ready,
            BLOCKED => State::Blocked,
            SLEEPING => State::Sleeping(rec.settings.lock().until),
            _ => State::Exited,
        })
    }

    pub fn thread_count(&self) -> usize {
        self.threads.lock().len()
    }

    /// A new thread, ready to run, at the default priority on any CPU.
    /// `cpu` is the CPU this is called on: if it is idling, it should
    /// take the thread at once.
    pub fn add(&self, id: ThreadId, cpu: usize) {
        self.add_with(id, cpu, DEFAULT, ANY_CPU);
    }

    /// A new thread, ready to run, with its priority and affinity.
    pub fn add_with(&self, id: ThreadId, cpu: usize, priority: Priority, affinity: u64) {
        self.add_with_handle(id, cpu, priority, affinity, 0);
    }

    /// `add_with`, with the mechanism's handle of the thread: set before
    /// it is queued, since another CPU may take it at once.
    pub fn add_with_handle(&self, id: ThreadId, cpu: usize, priority: Priority, affinity: u64, handle: usize) {
        let priority = priority.clamp(HIGHEST, LOWEST);
        let rec = Rec::new(id, priority, affinity, cpu, handle);
        assert!(self.threads.lock().insert(id, rec.clone()).is_none(), "thread {id} added twice");
        let settings = rec.settings.lock();
        self.enqueue(&rec, &settings, cpu);
    }

    /// `id`'s priority; None for an idle thread or one not known.
    pub fn priority(&self, id: ThreadId) -> Option<Priority> {
        self.rec(id).filter(|rec| !rec.idle).map(|rec| rec.settings.lock().priority)
    }

    /// The CPUs `id` may run on.
    pub fn affinity(&self, id: ThreadId) -> Option<u64> {
        self.rec(id).filter(|rec| !rec.idle).map(|rec| rec.settings.lock().affinity)
    }

    /// Gives `id` a new priority. A queued thread is queued again, as
    /// woken (and may take a CPU from a less important one); a running
    /// one that is now less important than someone waiting for its CPU
    /// gives way at once. `cpu` is the CPU this is called on. False if
    /// `id` is not a thread here (or is an idle thread).
    pub fn set_priority(&self, id: ThreadId, priority: Priority, cpu: usize) -> bool {
        self.change(id, cpu, |settings| settings.priority = priority.clamp(HIGHEST, LOWEST))
    }

    /// Gives `id` the CPUs it may run on, one bit each. A running thread
    /// on a CPU no longer allowed leaves it at once. The caller checks
    /// that `affinity` names a CPU there is. False as for set_priority.
    pub fn set_affinity(&self, id: ThreadId, affinity: u64, cpu: usize) -> bool {
        self.change(id, cpu, |settings| settings.affinity = affinity)
    }

    /// Changes `id`'s settings, then puts right where it is: a queued
    /// thread is taken out and queued again; the CPU running it is told
    /// its new priority and affinity, and asked to choose again if it
    /// should.
    fn change(&self, id: ThreadId, cpu: usize, f: impl FnOnce(&mut Settings)) -> bool {
        let Some(rec) = self.rec(id).filter(|rec| !rec.idle) else { return false };
        let mut settings = rec.settings.lock();
        f(&mut settings);
        loop {
            let on = rec.rq.load(SeqCst);
            if on == NOWHERE {
                return true; // waiting for something: it is queued with these when woken
            }
            let mut guard = self.cpus[on].local.lock();
            if rec.rq.load(SeqCst) != on {
                continue; // it moved before the lock was had
            }
            let l = local(&mut guard);
            if Arc::ptr_eq(&l.current, &rec) {
                l.priority = settings.priority;
                l.affinity = settings.affinity;
                self.publish_current(on, l);
                if settings.affinity & bit(on) == 0 || self.someone_waiting_for(on, l) {
                    self.ask(on, cpu);
                }
            } else if l.queue.remove(id).is_some() {
                self.publish(on, &l.queue);
                rec.rq.store(NOWHERE, SeqCst);
                drop(guard);
                self.enqueue(&rec, &settings, cpu);
            }
            return true;
        }
    }

    /// Whether a waiting thread may take `cpu` from its current one at
    /// the end of a slice: one at least as important that may run there
    /// - or anyone at all, if the current one may no longer run there.
    /// Other CPUs' queues are read by their published keys, which do not
    /// say whom they hold may run here: a yes from one of them is a
    /// reason to choose again, which may then keep the current thread.
    fn someone_waiting_for(&self, cpu: usize, l: &Local) -> bool {
        let anyone = Arc::ptr_eq(&l.current, &l.idle) || l.affinity & bit(cpu) == 0;
        let max = if anyone { LEVELS - 1 } else { level(l.priority) };
        if l.queue.best_for(cpu, max).is_some() {
            return true;
        }
        (0..self.cpu_count()).any(|c| {
            let head = self.cpus[c].head.load(SeqCst);
            c != cpu && head != EMPTY && level_of_key(head) <= max
        })
    }

    /// Asks `cpu` to choose again; interrupted if it is not `from`, the
    /// CPU this is decided on.
    fn ask(&self, cpu: usize, from: usize) {
        self.cpus[cpu].reschedule.store(true, SeqCst);
        if cpu != from {
            self.pokes.fetch_or(bit(cpu), SeqCst);
        }
    }

    /// One timer tick at time `now`, on the CPU that keeps the time:
    /// wakes sleepers whose time has come, and counts down the slice of
    /// the thread on `cpu`.
    pub fn tick(&self, now: u64, cpu: usize) {
        self.wake_sleepers(now, cpu);
        self.tick_slice(cpu);
    }

    /// Wakes every sleeper whose time is `now` or before, whichever CPU
    /// it slept on: the first ones of each sorted set, so this looks at
    /// no thread that is not due. Any CPU's timer interrupt may do it;
    /// `cpu` is the one deciding.
    pub fn wake_sleepers(&self, now: u64, cpu: usize) {
        for on in 0..self.cpu_count() {
            loop {
                // Taken out of the set under its lock, woken after: the
                // waking takes the thread's settings lock, which comes
                // before the sleepers' (see "Locks").
                let due = {
                    let mut set = self.cpus[on].sleepers.lock();
                    match set.first() {
                        Some(&(until, _)) if until <= now => set.pop_first(),
                        _ => None,
                    }
                };
                let Some((until, id)) = due else { break };
                if let Some(rec) = self.rec(id) {
                    let settings = rec.settings.lock();
                    // Woken early and asleep again since, it is in the set
                    // again under its new time: this entry is not it.
                    if rec.kind.load(SeqCst) == SLEEPING && settings.until == until {
                        self.make_runnable(&rec, &settings, cpu);
                    }
                }
            }
        }
    }

    /// When the first sleeper that went to sleep on `cpu` wakes: what
    /// that CPU's one-shot timer is set for, if it is before its tick.
    pub fn first_wake_on(&self, cpu: usize) -> Option<u64> {
        self.cpus[cpu].sleepers.lock().first().map(|&(until, _)| until)
    }

    /// When the first sleeper of all wakes.
    pub fn first_wake(&self) -> Option<u64> {
        (0..self.cpu_count()).filter_map(|c| self.first_wake_on(c)).min()
    }

    /// One timer tick on a CPU that does not keep the time: only the
    /// slice of the thread on `cpu`.
    pub fn tick_slice(&self, cpu: usize) {
        let mut guard = self.cpus[cpu].local.lock();
        let l = local(&mut guard);
        let waiting = self.someone_waiting_for(cpu, l);
        let reschedule = &self.cpus[cpu].reschedule;
        if Arc::ptr_eq(&l.current, &l.idle) {
            // An idling CPU has no slice to finish: work that arrived
            // from another CPU since its last tick is taken now.
            if waiting {
                reschedule.store(true, SeqCst);
            }
            return;
        }
        l.slice_left = l.slice_left.saturating_sub(1);
        if l.slice_left == 0 {
            if waiting {
                reschedule.store(true, SeqCst);
            } else {
                // Nobody waiting: a fresh slice, so a thread that turns
                // up later still finds a full slice ahead of it to wait
                // out - not an exhausted one that hands over at once.
                l.slice_left = SLICE;
            }
        }
    }

    /// Whether `cpu` has been asked to choose again, the request left as
    /// it is.
    pub fn reschedule_pending(&self, cpu: usize) -> bool {
        self.cpus[cpu].reschedule.load(SeqCst)
    }

    /// Whether `cpu` should choose again; clears the request.
    pub fn take_reschedule(&self, cpu: usize) -> bool {
        self.cpus[cpu].reschedule.swap(false, SeqCst)
    }

    /// The CPUs to interrupt because of what was done since the last
    /// call, one bit each; clears the set. Never the CPU that did it -
    /// though with several CPUs calling, a CPU may collect a bit another
    /// set, even its own: the caller skips its own.
    pub fn take_pokes(&self) -> u64 {
        self.pokes.swap(0, SeqCst)
    }

    /// Publishes what `cpu`'s queue holds: under its lock, after every
    /// change. Sequentially consistent: see "Nobody sleeps with work
    /// waiting".
    fn publish(&self, cpu: usize, queue: &Queue) {
        self.cpus[cpu].head.store(queue.head(), SeqCst);
        self.cpus[cpu].waiting.store(queue.count, SeqCst);
    }

    /// Publishes what `cpu` runs: under its lock, at every switch.
    fn publish_current(&self, cpu: usize, l: &Local) {
        let this = &self.cpus[cpu];
        this.current.store(l.current.id, SeqCst);
        this.priority.store(l.priority, SeqCst);
        this.idling.store(Arc::ptr_eq(&l.current, &l.idle), SeqCst);
    }

    /// Puts `entry` at the back of `cpu`'s queue, its lock held.
    fn push(&self, cpu: usize, l: &mut Local, rec: Arc<Rec>, priority: Priority, affinity: u64) {
        let joined = self.joined.fetch_add(1, SeqCst) + 1;
        rec.rq.store(cpu, SeqCst);
        l.queue.push_back(Entry { rec, affinity, joined }, priority);
        self.publish(cpu, &l.queue);
    }

    /// Into a queue - for a runnable thread that is in none and on no
    /// CPU, its settings held. It waits in the queue of the CPU that is
    /// to take it, if one is; else in its home's (`home`). Queued first,
    /// and only then is the CPU asked: asked first, it could look before
    /// the thread is there, find nothing and go back to idling.
    fn enqueue(&self, rec: &Arc<Rec>, settings: &Settings, cpu: usize) {
        let (priority, affinity, last) = (settings.priority, settings.affinity, settings.last);
        let on = self.offer_from(priority, affinity, cpu, last, false).unwrap_or_else(|| self.home(affinity, last));
        {
            let mut guard = self.cpus[on].local.lock();
            self.push(on, local(&mut guard), rec.clone(), priority, affinity);
        }
        race_window("queued");
        self.offer_from(priority, affinity, cpu, last, true);
    }

    /// Where a thread waits when no CPU is offered it: the allowed CPU
    /// with the fewest waiting - so that busy threads spread over the
    /// CPUs, and three on two take turns on both - and among those its
    /// last CPU first (its cache), then the lowest number.
    fn home(&self, affinity: u64, last: usize) -> usize {
        let count = self.cpu_count();
        let last = if last < count { last } else { 0 };
        (0..count)
            .map(|n| (last + n) % count)
            .filter(|&c| affinity & bit(c) != 0)
            .min_by_key(|&c| self.cpus[c].waiting.load(SeqCst))
            .unwrap_or(last)
    }

    /// Finds a CPU to take a thread (of `priority` and `affinity`) put in
    /// a queue by `cpu`, and - if `ask` - asks it. An idling CPU it may
    /// run on first: this one if it idles - it needs no telling - then
    /// its last one (K101: its data may still be in that cache), then
    /// another, which needs an interrupt: it sits halted, and would look
    /// at the queues only at its next timer tick.
    ///
    /// A CPU already offered a thread is passed over, so that two
    /// threads made ready one after the other get two CPUs, not the
    /// same one twice. Claiming one is a compare-and-swap of its
    /// `reschedule`, so two CPUs offering at once claim two.
    ///
    /// If no allowed CPU idles, the thread may still take one from a
    /// less important thread: the allowed CPU running the least
    /// important one, this CPU first among equals (no interrupt). If
    /// none is less important, nothing is done: it waits for a slice to
    /// end, as it always did.
    ///
    /// Without `ask` nothing is changed: it says where to queue the
    /// thread. With it, after the thread is queued, the answer may
    /// differ - another CPU may have claimed that one meanwhile - and
    /// the CPU asked takes the thread from whichever queue it is in.
    fn offer_from(&self, priority: Priority, affinity: u64, cpu: usize, last: usize, ask: bool) -> Option<usize> {
        let count = self.cpu_count();
        let allowed = |c: usize| c < count && affinity & bit(c) != 0;
        let claim = |c: usize| {
            let this = &self.cpus[c];
            allowed(c)
                && this.idling.load(SeqCst)
                && if ask {
                    this.reschedule.compare_exchange(false, true, SeqCst, SeqCst).is_ok()
                } else {
                    !this.reschedule.load(SeqCst)
                }
        };
        let order = [cpu, last].into_iter().chain((0..count).map(|n| (cpu + n) % count));
        for c in order {
            if claim(c) {
                if ask && c != cpu {
                    self.pokes.fetch_or(bit(c), SeqCst);
                }
                return Some(c);
            }
        }
        let mut victim: Option<(usize, Priority)> = None;
        for c in (0..count).map(|n| (cpu + n) % count) {
            let this = &self.cpus[c];
            if !allowed(c) || this.idling.load(SeqCst) || this.reschedule.load(SeqCst) {
                continue;
            }
            let theirs = this.priority.load(SeqCst);
            if theirs > priority && victim.is_none_or(|(_, worst)| theirs > worst) {
                victim = Some((c, theirs));
            }
        }
        let (chosen, _) = victim?;
        if ask {
            self.ask(chosen, cpu);
        }
        Some(chosen)
    }

    /// Ends the sleep or block of `rec`, its settings held. If it has not
    /// left its CPU yet it stays there, running; otherwise it is queued.
    fn make_runnable(&self, rec: &Arc<Rec>, settings: &Settings, cpu: usize) {
        if rec.kind.load(SeqCst) == SLEEPING {
            self.cpus[settings.asleep_on].sleepers.lock().remove(&(settings.until, rec.id));
        }
        loop {
            let on = rec.rq.load(SeqCst);
            if on == NOWHERE {
                rec.kind.store(RUNNABLE, SeqCst);
                self.enqueue(rec, settings, cpu);
                return;
            }
            // Still on CPU `on` (a waiting thread is never queued): if it
            // still is under that CPU's lock, it stays. Its `pick_next`
            // then finds it runnable, with no reason to leave.
            let _guard = self.cpus[on].local.lock();
            if rec.rq.load(SeqCst) == on {
                rec.kind.store(RUNNABLE, SeqCst);
                return;
            }
        }
    }

    /// Wakes a blocked or sleeping thread. False if it was neither.
    /// `cpu` is the CPU this is called on.
    pub fn wake(&self, id: ThreadId, cpu: usize) -> bool {
        let Some(rec) = self.rec(id) else { return false };
        let settings = rec.settings.lock();
        match rec.kind.load(SeqCst) {
            BLOCKED | SLEEPING => {
                self.make_runnable(&rec, &settings, cpu);
                true
            }
            _ => false,
        }
    }

    fn current_rec(&self, cpu: usize) -> Arc<Rec> {
        local(&mut self.cpus[cpu].local.lock()).current.clone()
    }

    /// Says that the thread on `cpu` will leave it at the next
    /// `pick_next`, into `state` (Sleeping, Blocked or Exited). Until
    /// then it is still there, and can still be woken. Called by that
    /// thread itself.
    pub fn suspend_current(&self, cpu: usize, state: State) {
        let rec = self.current_rec(cpu);
        // The idle thread is what runs when nothing else can, so it can
        // never wait for anything: pick_next would simply choose it again.
        assert!(!rec.idle, "the idle thread cannot sleep, block or exit");
        let mut settings = rec.settings.lock();
        settings.last = cpu;
        // Said "sleep" already and not yet left (then, say, killed: now
        // Exited): the old sleep's entry goes, or a tick would find it.
        if rec.kind.load(SeqCst) == SLEEPING {
            self.cpus[settings.asleep_on].sleepers.lock().remove(&(settings.until, rec.id));
        }
        let kind = match state {
            State::Sleeping(until) => {
                settings.until = until;
                settings.asleep_on = cpu;
                self.cpus[cpu].sleepers.lock().insert((until, rec.id));
                SLEEPING
            }
            State::Blocked => BLOCKED,
            State::Exited => EXITED,
            _ => panic!("suspend into {state:?}"),
        };
        rec.kind.store(kind, SeqCst);
    }

    /// Takes back a `suspend_current` into Sleeping or Blocked: the
    /// thread looked again and found it has nothing to wait for. (Not
    /// Exited: that cannot be taken back.)
    pub fn resume_current(&self, cpu: usize) {
        let rec = self.current_rec(cpu);
        let settings = rec.settings.lock();
        match rec.kind.load(SeqCst) {
            SLEEPING => {
                self.cpus[settings.asleep_on].sleepers.lock().remove(&(settings.until, rec.id));
                rec.kind.store(RUNNABLE, SeqCst);
            }
            BLOCKED => rec.kind.store(RUNNABLE, SeqCst),
            _ => {}
        }
    }

    /// Chooses who runs next on `cpu`. The current thread, if still
    /// running, goes to the back of the queue (unless it is the idle
    /// thread). Returns the switch to make, or None to keep running the
    /// current thread.
    pub fn pick_next(&self, cpu: usize) -> Option<Switch> {
        self.pick_next_with_handles(cpu).map(|(switch, _, _)| switch)
    }

    /// `pick_next`, with the mechanism's handles of the two threads.
    pub fn pick_next_with_handles(&self, cpu: usize) -> Option<(Switch, usize, usize)> {
        let this = &self.cpus[cpu];
        let mut guard = this.local.lock();
        let l = local(&mut guard);
        // Choosing is what a request to choose asks for. Left set, it
        // would follow the CPU into idling and read there as an offer
        // nobody made.
        this.reschedule.store(false, SeqCst);
        let from = l.current.clone();
        let still_running = from.kind.load(SeqCst) == RUNNABLE;
        // A running thread may stay only on a CPU it is allowed.
        let (mine, my_affinity) = (l.priority, l.affinity);
        let may_stay = still_running && my_affinity & bit(cpu) != 0;

        // The best waiting thread - unless it is less important than the
        // one running, which keeps the CPU (priorities are strict).
        let max = if may_stay && !from.idle { level(mine) } else { LEVELS - 1 };
        let (mut chosen, mut missed) = self.choose(cpu, l, max);
        if chosen.is_none() && !may_stay && !from.idle {
            // The host tests widen the race this closes (see `race_window`).
            race_window("before idling");
            // About to idle: say so, then look once more ("Nobody sleeps
            // with work waiting").
            this.idling.store(true, SeqCst);
            this.priority.store(IDLE_PRIORITY, SeqCst);
            fence(SeqCst);
            (chosen, missed) = self.choose(cpu, l, LEVELS - 1);
        }
        if chosen.is_none() && missed {
            // Work in a queue whose lock was busy: look again soon.
            this.reschedule.store(true, SeqCst);
        }
        let Some((to, priority, affinity)) = chosen else {
            if may_stay {
                l.slice_left = SLICE;
                self.publish_current(cpu, l);
                return None; // nobody else may have the CPU
            }
            let idle = l.idle.clone();
            let switch = self.switch_to(cpu, l, idle, IDLE_PRIORITY, ANY_CPU);
            if still_running && !from.idle {
                // No longer allowed here: back in a queue, for a CPU it
                // may run on.
                self.requeue(cpu, l, from.clone(), mine, my_affinity);
            } else if !from.idle {
                from.rq.store(NOWHERE, SeqCst);
            }
            return switch.map(|s| (s, from.handle, l.current.handle));
        };
        let to_handle = to.handle;
        // The switch comes first: `from` is on no CPU from here on, and
        // only then may it be queued.
        let switch = self.switch_to(cpu, l, to, priority, affinity);
        if still_running {
            if !from.idle {
                // It gave way on this CPU; another may be idling, or
                // running someone less important.
                self.requeue(cpu, l, from.clone(), mine, my_affinity);
            }
        } else {
            from.rq.store(NOWHERE, SeqCst);
        }
        switch.map(|s| (s, from.handle, to_handle))
    }

    /// The thread `cpu` should take: the most important it may run, no
    /// less important than `max`, of those the one that joined first,
    /// from its own queue or any other. Taken out of its queue, and
    /// `rq` set to `cpu`. Also whether a queue was passed over because
    /// its lock was busy.
    ///
    /// Another queue is judged by its published key until it has been
    /// looked into, and by what was found there after: its first thread
    /// may be one this CPU may not run, and then what it has for this CPU
    /// is further back - possibly behind another queue's first (the bug
    /// this way of looking fixes: journal entry NNN, bug 2). A queue is
    /// taken from only when what it has for this CPU beats every other
    /// queue's, as far as is known; otherwise the next is looked into,
    /// and a queue seen and let go is locked again only if it is then
    /// the best. Each queue is looked into once, so this ends - in a few
    /// rounds more if other CPUs change the queues meanwhile; after
    /// 2 rounds per CPU it takes its own best rather than go on, and
    /// says so (`missed`), so that it chooses again soon.
    fn choose(&self, cpu: usize, l: &mut Local, max: usize) -> (Option<(Arc<Rec>, Priority, u64)>, bool) {
        let count = self.cpu_count();
        // What each other queue has for this CPU, once looked into.
        let mut seen: [Option<u64>; MAX_CPUS] = [None; MAX_CPUS];
        let mut missed = false;
        let judged = |seen: &[Option<u64>; MAX_CPUS], c: usize| {
            seen[c].unwrap_or_else(|| {
                let head = self.cpus[c].head.load(SeqCst);
                if level_of_key(head) <= max { head } else { EMPTY }
            })
        };
        let mut rounds = 0;
        loop {
            rounds += 1;
            if rounds > 2 * count + 2 {
                missed = true; // queues changing under it: look again soon
                break;
            }
            let own = l.queue.best_for(cpu, max);
            let own_key = own.map_or(EMPTY, |place| l.queue.key_at(place));
            let mut other: Option<(usize, u64)> = None;
            for c in (0..count).filter(|&c| c != cpu) {
                let key = judged(&seen, c);
                if key < own_key && other.is_none_or(|(_, best)| key < best) {
                    other = Some((c, key));
                }
            }
            let Some((c, _)) = other else { break };
            let Some(mut guard) = self.lock_second(cpu, c) else {
                missed = true;
                seen[c] = Some(EMPTY); // busy: passed over this time
                continue;
            };
            let theirs = local(&mut guard);
            let found = theirs.queue.best_for(cpu, max);
            let key = found.map_or(EMPTY, |place| theirs.queue.key_at(place));
            seen[c] = Some(key);
            let beaten = key >= own_key || (0..count).any(|o| o != cpu && o != c && judged(&seen, o) < key);
            if let (Some(place), false) = (found, beaten) {
                let (entry, priority) = theirs.queue.take(place);
                self.publish(c, &theirs.queue);
                entry.rec.rq.store(cpu, SeqCst); // both locks held
                return (Some((entry.rec, priority, entry.affinity)), missed);
            }
        }
        let taken = l.queue.best_for(cpu, max).map(|place| {
            let (entry, priority) = l.queue.take(place);
            self.publish(cpu, &l.queue);
            (entry.rec, priority, entry.affinity)
        });
        (taken, missed)
    }

    /// CPU `other`'s lock, taken while `held`'s is held: waited for if
    /// `other` is the higher number, else only tried (see "Locks").
    fn lock_second(&self, held: usize, other: usize) -> Option<Guard<'_>> {
        let lock = &self.cpus[other].local;
        if other > held {
            return Some(lock.lock());
        }
        for _ in 0..TRIES {
            if let Some(guard) = lock.try_lock() {
                return Some(guard);
            }
            hint::spin_loop();
        }
        None
    }

    /// The thread `from`, which just left `cpu` still runnable, at the
    /// back of `cpu`'s own queue - the one lock held - and offered to an
    /// idling CPU or one running somebody less important, which takes it
    /// from there.
    fn requeue(&self, cpu: usize, l: &mut Local, from: Arc<Rec>, priority: Priority, affinity: u64) {
        self.push(cpu, l, from, priority, affinity);
        self.offer_from(priority, affinity, cpu, cpu, true);
    }

    fn switch_to(&self, cpu: usize, l: &mut Local, to: Arc<Rec>, priority: Priority, affinity: u64) -> Option<Switch> {
        let from = l.current.id;
        let to_id = to.id;
        to.rq.store(cpu, SeqCst);
        l.current = to;
        l.priority = priority;
        l.affinity = affinity;
        l.slice_left = SLICE;
        self.publish_current(cpu, l);
        (from != to_id).then_some(Switch { from, to: to_id })
    }

    /// Whether `remove` would forget `id` now: exited, and on no CPU and
    /// in no queue. Nothing is changed - the mechanism frees the thread's
    /// stack first and only then forgets it, so that whoever waits for
    /// it to be gone (`join`) finds the stack already back (journal
    /// entry NNN, bug 9).
    pub fn removable(&self, id: ThreadId) -> bool {
        self.rec(id).is_some_and(|rec| rec.kind.load(SeqCst) == EXITED && rec.rq.load(SeqCst) == NOWHERE)
    }

    /// Forgets an exited thread, once it is off every CPU. Returns
    /// whether it was there to forget.
    pub fn remove(&self, id: ThreadId) -> bool {
        let mut threads = self.threads.lock();
        let Some(rec) = threads.get(&id) else { return false };
        if rec.kind.load(SeqCst) != EXITED || rec.rq.load(SeqCst) != NOWHERE {
            return false;
        }
        threads.remove(&id);
        true
    }

    // ---- for the tests: what is queued, and where --------------------

    /// Every queued thread: CPU 0's queue in its order, then CPU 1's...
    #[cfg(test)]
    fn queued(&self) -> Vec<ThreadId> {
        (0..self.cpu_count()).flat_map(|c| self.queued_on(c)).collect()
    }

    #[cfg(test)]
    fn queued_on(&self, cpu: usize) -> Vec<ThreadId> {
        local(&mut self.cpus[cpu].local.lock()).queue.ids().collect()
    }

    /// Each CPU's sleepers, in order.
    #[cfg(test)]
    fn sleepers_on(&self, cpu: usize) -> Vec<(u64, ThreadId)> {
        self.cpus[cpu].sleepers.lock().iter().copied().collect()
    }

    /// Whether `id` may run on `cpu`. An idle thread runs only on its
    /// own CPU.
    #[cfg(test)]
    fn allowed(&self, id: ThreadId, cpu: usize) -> bool {
        self.rec(id).is_none_or(|rec| rec.idle || rec.settings.lock().affinity & bit(cpu) != 0)
    }

    /// The thread `cpu` would take next: the most important allowed on
    /// it, the longest waiting of those, from any queue.
    #[cfg(test)]
    fn best_for(&self, cpu: usize) -> Option<ThreadId> {
        let mut best: Option<(u64, ThreadId)> = None;
        for c in 0..self.cpu_count() {
            let guard = self.cpus[c].local.lock();
            let queue = &guard.as_ref().unwrap().queue;
            if let Some(place) = queue.best_for(cpu, LEVELS - 1) {
                let candidate = (queue.key_at(place), queue.levels[place.0][place.1].rec.id);
                if best.is_none_or(|b| candidate.0 < b.0) {
                    best = Some(candidate);
                }
            }
        }
        best.map(|(_, id)| id)
    }

    /// Every CPU's lock at once (in order), and everything checked
    /// against everything: each runnable thread that is not an idle one
    /// is on exactly one CPU or queued exactly once, and its `rq` says
    /// where; a thread that waits is nowhere; the sleeper sets hold
    /// exactly the sleepers. For the tests that run CPUs at once: a lost
    /// thread - runnable, yet in no queue and on no CPU - fails here.
    #[cfg(test)]
    fn check_consistent(&self) {
        let count = self.cpu_count();
        let guards: Vec<_> = (0..count).map(|c| self.cpus[c].local.lock()).collect();
        let mut place: BTreeMap<ThreadId, usize> = BTreeMap::new();
        for (c, guard) in guards.iter().enumerate() {
            let l = guard.as_ref().unwrap();
            assert_eq!(self.cpus[c].current.load(SeqCst), l.current.id, "CPU {c} publishes another current");
            assert_eq!(l.current.rq.load(SeqCst), c, "thread {} runs on CPU {c} but its rq says otherwise", l.current.id);
            if !l.current.idle {
                assert!(place.insert(l.current.id, c).is_none(), "thread {} on two CPUs", l.current.id);
                // A thread whose affinity was changed while it ran stays
                // until its CPU chooses again - which it has been asked to.
                let allowed = l.current.settings.lock().affinity & bit(c) != 0;
                assert!(allowed || self.cpus[c].reschedule.load(SeqCst), "CPU {c} runs thread {}, not allowed there, and was not asked to choose", l.current.id);
            }
            assert_eq!(self.cpus[c].head.load(SeqCst), l.queue.head(), "CPU {c} publishes an old key");
            for id in l.queue.ids() {
                assert!(place.insert(id, c).is_none(), "thread {id} in two places");
            }
            for entry in l.queue.levels.iter().flatten() {
                assert_eq!(entry.rec.rq.load(SeqCst), c, "thread {} queued on CPU {c} but its rq says otherwise", entry.rec.id);
                assert_eq!(entry.rec.kind.load(SeqCst), RUNNABLE, "thread {} queued while waiting", entry.rec.id);
            }
        }
        // Nobody sleeps with work waiting: each queued thread has a CPU
        // it may run on that is busy (its slice will end) or has been
        // asked to choose. Not "every idling CPU is asked": an idling CPU
        // offered one thread is passed over for the next (`offer_from`),
        // so a second may wait for that CPU, or for an idle one's tick.
        // A thread whose allowed CPUs all idle, unasked, is the lost
        // wake-up "Nobody sleeps with work waiting" exists to prevent.
        for guard in &guards {
            for entry in guard.as_ref().unwrap().queue.levels.iter().flatten() {
                let served = (0..count).any(|c| {
                    let l = guards[c].as_ref().unwrap();
                    entry.affinity & bit(c) != 0 && (!Arc::ptr_eq(&l.current, &l.idle) || self.cpus[c].reschedule.load(SeqCst))
                });
                assert!(served, "thread {} waits, and every CPU it may run on idles unasked", entry.rec.id);
            }
        }
        for (&id, rec) in self.threads.lock().iter() {
            if rec.idle {
                continue;
            }
            match rec.kind.load(SeqCst) {
                RUNNABLE => assert!(place.contains_key(&id), "thread {id} is runnable but in no queue and on no CPU: lost"),
                _ if place.contains_key(&id) => {
                    // Waiting, but not yet left its CPU: allowed only there.
                    let c = place[&id];
                    assert_eq!(guards[c].as_ref().unwrap().current.id, id, "thread {id} waits but is queued");
                }
                _ => assert_eq!(rec.rq.load(SeqCst), NOWHERE, "thread {id} waits but its rq names a CPU"),
            }
            let asleep = (0..count).filter(|&c| self.cpus[c].sleepers.lock().iter().any(|&(_, other)| other == id)).count();
            let sleeping = rec.kind.load(SeqCst) == SLEEPING;
            assert_eq!(asleep, usize::from(sleeping), "thread {id}: sleeping {sleeping}, in {asleep} sleeper sets");
        }
    }

    /// Every thread's number.
    #[cfg(test)]
    fn ids(&self) -> Vec<ThreadId> {
        self.threads.lock().keys().copied().collect()
    }

    /// The queued thread that joined first, in any queue.
    #[cfg(test)]
    fn first_joined(&self) -> Option<ThreadId> {
        let mut first: Option<(u64, ThreadId)> = None;
        for c in 0..self.cpu_count() {
            let guard = self.cpus[c].local.lock();
            for entry in guard.as_ref().unwrap().queue.levels.iter().flatten() {
                if first.is_none_or(|(joined, _)| entry.joined < joined) {
                    first = Some((entry.joined, entry.rec.id));
                }
            }
        }
        first.map(|(_, id)| id)
    }

    #[cfg(test)]
    fn reschedule_of(&self, cpu: usize) -> bool {
        self.cpus[cpu].reschedule.load(SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;

    const IDLE: ThreadId = 0;

    /// Runs `pick_next` and returns who is running afterwards.
    fn next(s: &Scheduler) -> ThreadId {
        s.pick_next(0);
        s.current(0)
    }

    #[test]
    fn idle_runs_when_nothing_else_can() {
        let s = Scheduler::new(IDLE);
        assert_eq!(s.pick_next(0), None);
        assert_eq!(s.current(0), IDLE);
    }

    #[test]
    fn round_robin_in_order_and_idle_never_queued() {
        let s = Scheduler::new(IDLE);
        for id in 1..=3 {
            s.add(id, 0);
        }
        assert!(s.take_reschedule(0), "new work while idle asks for a switch");
        let order: Vec<_> = (0..7).map(|_| next(&s)).collect();
        assert_eq!(order, vec![1, 2, 3, 1, 2, 3, 1]);
    }

    #[test]
    fn a_slice_runs_out_only_when_someone_is_waiting() {
        let s = Scheduler::new(IDLE);
        s.add(1, 0);
        next(&s);
        s.take_reschedule(0);
        for t in 1..=10 {
            s.tick(t, 0);
        }
        assert!(!s.take_reschedule(0), "alone: keeps the CPU");

        s.add(2, 0);
        s.tick(11, 0);
        assert!(!s.take_reschedule(0), "slice not over yet");
        s.tick(12, 0);
        assert!(s.take_reschedule(0), "slice over, 2 is waiting");
        assert_eq!(next(&s), 2);
    }

    #[test]
    fn sleepers_wake_on_their_tick() {
        let s = Scheduler::new(IDLE);
        s.add(1, 0);
        next(&s);
        s.suspend_current(0, State::Sleeping(5));
        assert_eq!(next(&s), IDLE);
        s.take_reschedule(0);

        s.tick(4, 0);
        assert_eq!(s.state(1), Some(State::Sleeping(5)));
        assert!(!s.take_reschedule(0));
        s.tick(5, 0);
        assert_eq!(s.state(1), Some(State::Ready));
        assert!(s.take_reschedule(0), "the idle CPU should switch at once");
        assert_eq!(next(&s), 1);
    }

    #[test]
    fn blocked_threads_are_skipped_until_woken() {
        let s = Scheduler::new(IDLE);
        s.add(1, 0);
        s.add(2, 0);
        assert_eq!(next(&s), 1);
        s.suspend_current(0, State::Blocked);
        assert_eq!(next(&s), 2);
        assert_eq!(next(&s), 2, "1 is blocked; 2 keeps going");

        assert!(s.wake(1, 0));
        assert!(!s.wake(1, 0), "already awake");
        assert_eq!(next(&s), 1);
    }

    #[test]
    fn a_thread_that_blocks_and_is_woken_before_switching_keeps_running() {
        // The race a wait queue must survive: block, then the wakeup
        // arrives before the switch happens.
        let s = Scheduler::new(IDLE);
        s.add(1, 0);
        next(&s);
        s.suspend_current(0, State::Blocked);
        s.wake(1, 0);
        assert_eq!(next(&s), 1);
        assert_eq!(s.state(1), Some(State::Running));
    }

    #[test]
    fn exited_threads_leave_and_are_removed_once_off_the_cpu() {
        let s = Scheduler::new(IDLE);
        s.add(1, 0);
        next(&s);
        s.suspend_current(0, State::Exited);
        assert!(!s.remove(1), "still on the CPU");
        assert_eq!(s.pick_next(0), Some(Switch { from: 1, to: IDLE }));
        assert!(s.remove(1));
        assert_eq!(s.thread_count(), 1);
        assert!(!s.wake(1, 0), "gone");
    }

    #[test]
    #[should_panic(expected = "idle thread cannot sleep")]
    fn the_idle_thread_cannot_sleep() {
        let s = Scheduler::new(IDLE);
        s.suspend_current(0, State::Sleeping(10));
    }

    #[test]
    fn a_boot_thread_can_sleep_and_exit_leaving_idle() {
        const BOOT: ThreadId = 1;
        let s = Scheduler::with_running(IDLE, BOOT);
        s.suspend_current(0, State::Sleeping(3));
        assert_eq!(s.pick_next(0), Some(Switch { from: BOOT, to: IDLE }));
        s.tick(3, 0);
        assert!(s.take_reschedule(0));
        assert_eq!(next(&s), BOOT);
        s.suspend_current(0, State::Exited);
        assert_eq!(next(&s), IDLE);
        assert!(s.remove(BOOT));
        assert_eq!(s.thread_count(), 1);
    }

    /// Many threads, random operations: every thread that is ready is
    /// eventually run, nobody runs twice in a row while others wait, and
    /// the idle thread is never queued.
    /// The sorted set holds exactly the Sleeping threads, at their times.
    fn sleepers_match(s: &Scheduler) {
        let sleeping: BTreeSet<(u64, ThreadId)> = s
            .ids()
            .into_iter()
            .filter_map(|id| match s.state(id) {
                Some(State::Sleeping(until)) => Some((until, id)),
                _ => None,
            })
            .collect();
        let sets: BTreeSet<(u64, ThreadId)> = (0..s.cpu_count()).flat_map(|c| s.sleepers_on(c)).collect();
        assert_eq!(sets, sleeping, "the sleeper sets and the states disagree");
        let total: usize = (0..s.cpu_count()).map(|c| s.sleepers_on(c).len()).sum();
        assert_eq!(total, sleeping.len(), "a sleeper in two sets");
    }

    #[test]
    fn sleepers_wake_in_their_order_and_leave_the_set_when_woken_early() {
        let s = Scheduler::new(IDLE);
        for id in 1..=4 {
            s.add(id, 0);
        }
        // Threads 1-4 go to sleep, in turn, until 30, 10, 10 and 20.
        for (id, until) in [(1, 30), (2, 10), (3, 10), (4, 20)] {
            assert_eq!(next(&s), id);
            s.suspend_current(0, State::Sleeping(until));
        }
        assert_eq!(next(&s), IDLE);
        assert_eq!(s.first_wake(), Some(10));
        sleepers_match(&s);
        // Thread 4 is woken early, as a futex or a message would.
        assert!(s.wake(4, 0));
        assert_eq!(s.sleepers_on(0).len(), 3);
        sleepers_match(&s);
        // At 9 nobody is due; at 10 both of that time are, not thread 1.
        s.tick(9, 0);
        assert_eq!(s.sleepers_on(0).len(), 3);
        s.tick(10, 0);
        assert_eq!(s.state(2), Some(State::Ready));
        assert_eq!(s.state(3), Some(State::Ready));
        assert_eq!(s.state(1), Some(State::Sleeping(30)));
        assert_eq!(s.first_wake(), Some(30));
        sleepers_match(&s);
        // A sleep taken back (the thread had nothing to wait for).
        let current = next(&s);
        assert_ne!(current, IDLE);
        s.suspend_current(0, State::Sleeping(5));
        assert_eq!(s.first_wake(), Some(5));
        s.resume_current(0);
        assert_eq!(s.state(current), Some(State::Running));
        assert_eq!(s.first_wake(), Some(30));
        sleepers_match(&s);
        s.tick(1_000, 0);
        assert_eq!(s.first_wake(), None);
        sleepers_match(&s);
    }

    #[test]
    fn randomized_fairness() {
        let s = Scheduler::new(IDLE);
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut random = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for id in 1..=20 {
            s.add(id, 0);
        }
        let mut now = 0;
        let mut ran = BTreeMap::new();
        for _ in 0..10_000 {
            now += 1;
            s.tick(now, 0);
            match random() % 10 {
                0 if s.current(0) != IDLE => s.suspend_current(0, State::Sleeping(now + random() % 5)),
                1 if s.current(0) != IDLE => s.suspend_current(0, State::Blocked),
                2 => {
                    let id = 1 + random() % 20;
                    s.wake(id, 0);
                }
                _ => {}
            }
            let before = s.current(0);
            s.pick_next(0);
            let after = s.current(0);
            *ran.entry(after).or_insert(0u32) += 1;
            assert!(!s.queued().contains(&IDLE));
            if before == after && after != IDLE {
                assert!(s.queued().is_empty(), "kept the CPU while others waited");
            }
        }
        for id in 1..=20 {
            assert!(ran.get(&id).copied().unwrap_or(0) > 50, "thread {id} starved");
        }
    }

    // ---- more than one CPU ------------------------------------------------

    /// Two CPUs: idle threads 0 and 100.
    fn two_cpus() -> Scheduler {
        let s = Scheduler::new(IDLE);
        assert_eq!(s.add_cpu(100), 1);
        s
    }

    // ---- A run queue per CPU (K68, K101; journal entry 129) ----------

    /// Runs `cpu`'s slice out: SLICE ticks, then it chooses if asked.
    fn end_slice(s: &Scheduler, cpu: usize) {
        for _ in 0..SLICE {
            s.tick_slice(cpu);
        }
        if s.take_reschedule(cpu) {
            s.pick_next(cpu);
        }
    }

    #[test]
    fn among_equals_the_longest_waiting_goes_first_and_an_idle_cpu_takes_work_from_a_busy_one() {
        let s = two_cpus();
        // Four threads, both CPUs busy: two wait, one in each queue.
        for id in 1..=4 {
            s.add(id, 0);
            s.pick_next(0);
            s.pick_next(1);
        }
        assert_eq!(s.queued_on(0).len(), 1, "each waits in a queue of its own");
        assert_eq!(s.queued_on(1).len(), 1);
        let first = s.first_joined().unwrap();
        // CPU 1's slice ends: it takes the one that joined first, even if
        // it waits in CPU 0's queue - as fair as one queue for all.
        end_slice(&s, 1);
        assert_eq!(s.current(1), first);
        // CPU 0's thread blocks, then the next: CPU 0 takes from any
        // queue rather than idle.
        s.suspend_current(0, State::Blocked);
        s.pick_next(0);
        assert_ne!(s.current(0), IDLE);
        s.suspend_current(0, State::Blocked);
        s.pick_next(0);
        assert_ne!(s.current(0), IDLE, "an idle CPU takes work from a busy one's queue");
        assert!(s.queued().is_empty());
        sleepers_match(&s);
    }

    #[test]
    fn a_more_important_thread_in_another_queue_comes_before_this_cpus_own() {
        let s = two_cpus();
        s.add(1, 0);
        s.add(2, 0);
        s.pick_next(0);
        s.pick_next(1);
        // Thread 3, at 0, made by CPU 1: no CPU runs anyone less
        // important, so it waits - in CPU 1's queue (its home).
        s.add_with(3, 1, DEFAULT, ANY_CPU);
        assert_eq!(s.queued_on(1).len(), 1);
        // Thread 4, at -3, made by CPU 0: it takes CPU 0 (this CPU first
        // among equals), so it waits in CPU 0's queue, CPU 0 asked.
        s.add_with(4, 0, -3, ANY_CPU);
        assert_eq!(s.queued_on(0).len(), 1);
        // CPU 1 chooses first: thread 4, from CPU 0's queue, before its
        // own thread 3 - priorities are strict across the queues.
        s.suspend_current(1, State::Blocked);
        s.pick_next(1);
        assert_eq!(s.current(1), 4);
        assert_eq!(s.queued_on(1).len(), 1, "thread 3 still waits");
    }

    #[test]
    fn a_woken_thread_goes_back_to_the_cpu_it_last_ran_on_when_that_one_idles() {
        let s = Scheduler::new(IDLE);
        s.add_cpu(100);
        s.add_cpu(200);
        s.add(1, 0);
        s.add(2, 0);
        s.pick_next(0);
        s.pick_next(1);
        let (a, b) = (s.current(0), s.current(1));
        // b runs on CPU 1 and blocks there; CPU 1 idles.
        s.suspend_current(1, State::Blocked);
        s.pick_next(1);
        assert_eq!(s.current(1), 100);
        s.take_pokes();
        // Woken by CPU 0, which is busy (with a): CPU 2 idles too, but b
        // goes back to CPU 1, where it last ran.
        assert!(s.wake(b, 0));
        assert_eq!(s.take_pokes(), 1 << 1, "CPU 1 interrupted, not CPU 2");
        assert!(s.take_reschedule(1));
        s.pick_next(1);
        assert_eq!(s.current(1), b);
        assert_eq!(s.current(0), a);
    }

    #[test]
    fn busy_threads_share_the_cpus_fairly_whatever_order_slices_end_in() {
        for (threads, cpus) in [(4u64, 2usize), (3, 2), (5, 2), (6, 4), (5, 4)] {
            let s = Scheduler::new(IDLE);
            for n in 1..cpus {
                s.add_cpu(100 * n as u64);
            }
            for id in 1..=threads {
                s.add(id, 0);
            }
            for cpu in 0..cpus {
                s.pick_next(cpu);
            }
            let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
            let mut ran = alloc::vec![0u32; threads as usize + 1];
            for _ in 0..4000 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let cpu = (seed % cpus as u64) as usize;
                let current = s.current(cpu);
                if current <= threads {
                    ran[current as usize] += 1;
                }
                s.tick_slice(cpu);
                if s.take_reschedule(cpu) {
                    s.pick_next(cpu);
                }
            }
            let total: u32 = ran.iter().sum();
            let fair = total / threads as u32;
            for id in 1..=threads as usize {
                assert!(
                    ran[id] * 10 >= fair * 7 && ran[id] * 10 <= fair * 13,
                    "{threads} threads on {cpus} CPUs: thread {id} had {} ticks, fair is {fair}: {ran:?}",
                    ran[id]
                );
            }
        }
    }

    #[test]
    fn three_busy_threads_on_two_cpus_share_them_fairly() {
        let s = two_cpus();
        for id in 1..=3 {
            s.add(id, 0);
        }
        s.pick_next(0);
        s.pick_next(1);
        let mut ran = [0u32; 4];
        for _ in 0..300 {
            for cpu in 0..2 {
                ran[s.current(cpu) as usize] += 1;
                end_slice(&s, cpu);
            }
        }
        // 600 slices among three: 200 each, if fair. Each gets most of
        // that, not 300 for one and 150 for the others.
        for id in 1..=3 {
            assert!((170..=230).contains(&ran[id]), "thread {id} ran {} slices of 600: {ran:?}", ran[id]);
        }
        sleepers_match(&s);
    }

    #[test]
    fn two_cpus_take_different_threads_from_the_one_queue() {
        let s = two_cpus();
        s.add(1, 0);
        s.add(2, 0);
        assert_eq!(s.pick_next(0), Some(Switch { from: IDLE, to: 1 }));
        assert_eq!(s.pick_next(1), Some(Switch { from: 100, to: 2 }));
        assert_eq!((s.current(0), s.current(1)), (1, 2));
        assert_eq!(s.cpu_of(2), Some(1));
        // Nobody else is waiting: each keeps its CPU.
        assert_eq!(s.pick_next(0), None);
        assert_eq!(s.pick_next(1), None);
    }

    #[test]
    fn a_thread_runs_on_whichever_cpu_is_free() {
        let s = two_cpus();
        s.add(1, 0);
        s.pick_next(0);
        assert_eq!(s.cpu_of(1), Some(0));
        s.suspend_current(0, State::Blocked);
        s.pick_next(0);
        assert_eq!(s.cpu_of(1), None);
        // Woken by something that happened on CPU 1, which is idle and
        // so takes it: the thread has moved.
        assert!(s.wake(1, 1));
        assert!(s.take_reschedule(1));
        s.pick_next(1);
        assert_eq!(s.cpu_of(1), Some(1));
    }

    #[test]
    fn each_cpu_falls_back_to_its_own_idle_thread() {
        let s = two_cpus();
        s.add(1, 0);
        s.add(2, 0);
        s.pick_next(0);
        s.pick_next(1);
        s.suspend_current(0, State::Blocked);
        s.suspend_current(1, State::Blocked);
        assert_eq!(s.pick_next(1), Some(Switch { from: 2, to: 100 }));
        assert_eq!(s.pick_next(0), Some(Switch { from: 1, to: IDLE }));
        assert!(!s.queued().contains(&IDLE) && !s.queued().contains(&100));
        assert!(s.is_idle(100) && s.is_idle(IDLE) && !s.is_idle(1));
    }

    #[test]
    fn woken_before_leaving_its_cpu_a_thread_is_not_put_in_the_queue() {
        // The race that only exists with two CPUs. Thread 1, on CPU 0,
        // says it will block. Before it has switched away, CPU 1 wakes
        // it. In the queue, CPU 1 could take it and run it - on a stack
        // CPU 0 is still using.
        let s = two_cpus();
        s.add(1, 0);
        s.pick_next(0);
        s.suspend_current(0, State::Blocked);
        assert!(s.wake(1, 1));
        assert!(s.queued().is_empty(), "a thread still on a CPU was queued");
        assert!(!s.take_reschedule(1), "there is nothing for CPU 1 to take");
        assert_eq!(s.pick_next(1), None);
        assert_eq!(s.state(1), Some(State::Running));
        // And CPU 0, reaching the switch, finds no reason to leave.
        assert_eq!(s.pick_next(0), None);
        assert_eq!(s.current(0), 1);
    }

    #[test]
    fn a_sleeper_whose_time_comes_before_it_left_its_cpu_stays_too() {
        let s = two_cpus();
        s.add(1, 1);
        s.pick_next(1);
        s.suspend_current(1, State::Sleeping(5));
        s.tick(5, 0); // the time is kept on CPU 0
        assert!(s.queued().is_empty());
        assert_eq!(s.state(1), Some(State::Running));
        assert_eq!(s.pick_next(1), None);
    }

    #[test]
    fn a_wait_can_be_taken_back() {
        let s = Scheduler::new(IDLE);
        s.add(1, 0);
        s.pick_next(0);
        s.suspend_current(0, State::Blocked);
        s.resume_current(0);
        assert_eq!(s.state(1), Some(State::Running));
        assert_eq!(s.pick_next(0), None);
        // Taking back what was not there changes nothing.
        s.resume_current(0);
        assert_eq!(s.state(1), Some(State::Running));
        // Exiting cannot be taken back.
        s.suspend_current(0, State::Exited);
        s.resume_current(0);
        assert_eq!(s.state(1), Some(State::Exited));
    }

    #[test]
    fn an_exited_thread_is_not_forgotten_while_any_cpu_is_on_it() {
        let s = two_cpus();
        s.add(1, 0);
        s.pick_next(1); // CPU 1 takes it
        s.suspend_current(1, State::Exited);
        assert!(!s.remove(1), "CPU 1 is still on its stack");
        s.pick_next(1);
        assert!(s.remove(1));
    }

    #[test]
    fn an_idle_cpu_takes_work_at_its_tick_and_a_busy_one_at_the_end_of_its_slice() {
        let s = two_cpus();
        s.add(1, 0);
        s.pick_next(0);
        s.take_reschedule(0);
        // Thread 2 is made on CPU 0, which is busy; CPU 1 idles, and is
        // to be interrupted. Suppose that interrupt never comes:
        s.add(2, 0);
        assert_eq!(s.take_pokes(), 0b10);
        assert!(s.take_reschedule(1));
        assert!(!s.take_reschedule(1));
        s.tick_slice(1);
        assert!(s.take_reschedule(1), "the idle CPU's tick still finds the work");
        s.pick_next(1);
        assert_eq!(s.current(1), 2);
        // With both busy and nobody waiting, slices never end.
        for _ in 0..10 {
            s.tick_slice(0);
            s.tick_slice(1);
        }
        assert!(!s.take_reschedule(0) && !s.take_reschedule(1));
        s.add(3, 0);
        s.tick_slice(1);
        assert!(!s.take_reschedule(1));
        s.tick_slice(1);
        assert!(s.take_reschedule(1));
    }

    #[test]
    fn a_cpu_that_joins_late_takes_waiting_work() {
        let s = Scheduler::new(IDLE);
        for id in 1..=3 {
            s.add(id, 0);
        }
        s.pick_next(0);
        let cpu = s.add_cpu(100);
        assert!(s.take_reschedule(cpu));
        s.pick_next(cpu);
        assert_eq!(s.current(cpu), 2);
        assert_eq!(s.cpu_count(), 2);
    }

    /// Four CPUs, many threads, random operations in a random order of
    /// CPUs. After every step: no thread is on two CPUs, no thread on a
    /// CPU is in the queue, nothing is queued twice, no idle thread is
    /// queued or on another CPU, and what is on a CPU is Running or
    /// about to leave. And nobody starves.
    /// Two CPUs, CPU 0 running thread 1 and CPU 1 idling.
    fn one_busy_one_idle() -> Scheduler {
        let s = Scheduler::new(IDLE);
        s.add(1, 0);
        s.pick_next(0);
        s.add_cpu(100);
        assert_eq!(s.take_pokes(), 0);
        s
    }

    #[test]
    fn a_thread_made_ready_on_a_busy_cpu_is_offered_to_an_idling_one() {
        let s = one_busy_one_idle();
        s.add(2, 0);
        assert_eq!(s.take_pokes(), 0b10, "CPU 1 idles and was not told");
        assert_eq!(s.take_pokes(), 0, "told twice");
        // Told, it chooses again, and finds the thread.
        assert!(s.take_reschedule(1));
        assert_eq!(s.pick_next(1), Some(Switch { from: 100, to: 2 }));
        // The same for a thread that wakes.
        s.suspend_current(1, State::Blocked);
        s.pick_next(1);
        assert!(s.wake(2, 0));
        assert_eq!(s.take_pokes(), 0b10);
    }

    #[test]
    fn an_idling_cpu_that_makes_a_thread_ready_takes_it_itself_and_tells_nobody() {
        let s = one_busy_one_idle();
        s.add(2, 1);
        assert_eq!(s.take_pokes(), 0);
        assert!(s.take_reschedule(1));
    }

    #[test]
    fn nobody_is_told_when_every_cpu_is_busy() {
        let s = one_busy_one_idle();
        s.add(2, 0);
        s.take_pokes();
        s.take_reschedule(1);
        s.pick_next(1);
        s.add(3, 0);
        assert_eq!(s.take_pokes(), 0);
        assert!(!s.take_reschedule(0), "a busy CPU does not drop its thread for a new one");
    }

    #[test]
    fn two_threads_made_ready_at_once_go_to_two_cpus() {
        let s = one_busy_one_idle();
        s.add_cpu(200);
        s.add_cpu(300);
        s.add(2, 0);
        s.add(3, 0);
        assert_eq!(s.take_pokes(), 0b0110, "CPUs 1 and 2, one thread each");
        // A third thread gets the third CPU; a fourth has nobody left.
        s.add(4, 0);
        assert_eq!(s.take_pokes(), 0b1000);
        s.add(5, 0);
        assert_eq!(s.take_pokes(), 0);
    }

    #[test]
    fn a_thread_that_gives_way_is_offered_to_an_idling_cpu() {
        // CPU 0 runs 1 with 2 waiting; CPU 1 arrives idling only now.
        let s = Scheduler::new(IDLE);
        s.add(1, 0);
        s.pick_next(0);
        s.add(2, 0);
        s.add_cpu(100);
        s.take_reschedule(1);
        s.take_pokes();
        // Thread 1 yields to 2 and joins the queue - where CPU 1 could
        // run it at once.
        assert_eq!(s.pick_next(0), Some(Switch { from: 1, to: 2 }));
        assert_eq!(s.take_pokes(), 0b10);
    }

    #[test]
    fn a_cpu_told_for_nothing_just_goes_on_idling() {
        let s = one_busy_one_idle();
        s.add(2, 0);
        assert_eq!(s.take_pokes(), 0b10);
        // CPU 0 gets there first: its thread blocks and it takes 2.
        s.suspend_current(0, State::Blocked);
        assert_eq!(s.pick_next(0), Some(Switch { from: 1, to: 2 }));
        // CPU 1's interrupt arrives; there is nothing left.
        assert!(s.take_reschedule(1));
        assert_eq!(s.pick_next(1), None);
        assert_eq!(s.current(1), 100);
        // And it can be offered the next one.
        s.wake(1, 0);
        assert_eq!(s.take_pokes(), 0b10);
    }

    #[test]
    fn randomized_on_four_cpus() {
        // Far more threads than CPUs: nobody starves, and threads move.
        let (moved, _) = randomized(30);
        assert!(moved > 100, "threads did not move between CPUs");
    }

    #[test]
    fn randomized_with_cpus_to_spare() {
        // Fewer threads than CPUs: there is nearly always a CPU idling
        // when a thread wakes, and it must be told.
        let (_, poked) = randomized(3);
        assert!(poked > 100, "idling CPUs were hardly ever told of work: {poked} times");
    }

    /// Four CPUs and `threads` threads doing random things in a random
    /// order, with everything that must always be true checked after
    /// every step. Returns how often a thread changed CPU, and how
    /// often an idling CPU was to be interrupted.
    fn randomized(threads: u64) -> (u32, u32) {
        const CPUS: usize = 4;
        let idle_of = |cpu: usize| if cpu == 0 { IDLE } else { 1000 + cpu as u64 };
        let s = Scheduler::new(IDLE);
        for cpu in 1..CPUS {
            assert_eq!(s.add_cpu(idle_of(cpu)), cpu);
        }
        // After each thing CPU `by` does: whoever is to be interrupted
        // for it idles, was offered a thread, and is not `by` itself.
        let interrupted = |s: &Scheduler, by: usize| {
            let pokes = s.take_pokes();
            for other in 0..CPUS {
                if pokes & (1 << other) != 0 {
                    assert_eq!(s.current(other), idle_of(other), "busy CPU {other} was interrupted");
                    assert!(s.reschedule_of(other), "CPU {other} interrupted without an offer");
                    assert_ne!(other, by, "CPU {other} told itself");
                }
            }
            assert_eq!(pokes >> CPUS, 0, "a CPU that does not exist was interrupted");
            pokes.count_ones()
        };
        let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
        let mut random = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for id in 1..=threads {
            s.add(id, 0);
        }
        let mut now = 0;
        let mut ran = BTreeMap::new();
        let mut moved = 0u32;
        let mut poked = 0u32;
        let mut last_cpu: BTreeMap<ThreadId, usize> = BTreeMap::new();
        for step in 0..40_000 {
            let cpu = (random() % CPUS as u64) as usize;
            if step % CPUS == 0 {
                now += 1;
                s.tick(now, 0);
            } else {
                s.tick_slice(cpu);
            }
            poked += interrupted(&s, 0);
            let on_idle = s.current(cpu) == idle_of(cpu);
            match random() % 12 {
                0 if !on_idle => s.suspend_current(cpu, State::Sleeping(now + random() % 5)),
                1 if !on_idle => s.suspend_current(cpu, State::Blocked),
                // Blocks, and thinks better of it.
                2 if !on_idle => {
                    s.suspend_current(cpu, State::Blocked);
                    s.resume_current(cpu);
                }
                3 | 4 => {
                    s.wake(1 + random() % threads, cpu);
                }
                _ => {}
            }
            // Sometimes the switch does not follow at once: another CPU
            // gets in first, which is the whole difficulty.
            poked += interrupted(&s, cpu);
            if random() % 3 != 0 {
                s.pick_next(cpu);
            }
            poked += interrupted(&s, cpu);

            // No thread waits while a CPU idles unasked: if the queue
            // has threads, every idling CPU has been offered one - or
            // there are at least as many offers as threads.
            let offered = (0..CPUS).filter(|&c| s.current(c) == idle_of(c) && s.reschedule_of(c)).count();
            let idling = (0..CPUS).filter(|&c| s.current(c) == idle_of(c)).count();
            assert!(
                offered >= s.queued().len().min(idling),
                "step {step}: {} threads wait, {idling} CPUs idle, only {offered} were offered one",
                s.queued().len()
            );

            for a in 0..CPUS {
                let current = s.current(a);
                for b in a + 1..CPUS {
                    assert_ne!(current, s.current(b), "one thread on CPUs {a} and {b}");
                }
                assert!(!s.queued().contains(&current), "thread {current} is on CPU {a} and in the queue");
                assert!(!s.queued().contains(&idle_of(a)), "an idle thread is queued");
                if s.is_idle(current) {
                    assert_eq!(current, idle_of(a), "CPU {a} runs another CPU's idle thread");
                }
                assert_ne!(s.state(current), Some(State::Ready), "thread {current} is on CPU {a} and Ready");
                *ran.entry(current).or_insert(0u32) += 1;
                if !s.is_idle(current) && last_cpu.insert(current, a).is_some_and(|before| before != a) {
                    moved += 1;
                }
            }
            for (n, id) in s.queued().iter().enumerate() {
                assert!(!s.queued().iter().skip(n + 1).any(|other| other == id), "thread {id} queued twice");
                assert_eq!(s.state(*id), Some(State::Ready));
            }
            sleepers_match(&s);
        }
        for id in 1..=threads {
            assert!(ran.get(&id).copied().unwrap_or(0) > 50, "thread {id} starved");
        }
        (moved, poked)
    }
    // ---- Priorities and affinity (journal entry 124) ----------------------

    #[test]
    fn the_most_important_ready_thread_runs_first() {
        let s = Scheduler::new(IDLE);
        s.add_with(1, 0, DEFAULT, ANY_CPU);
        s.add_with(2, 0, 5, ANY_CPU);
        s.add_with(3, 0, -5, ANY_CPU);
        assert_eq!(s.queued(), [3, 1, 2], "the queue's order");
        assert_eq!(next(&s), 3);
    }

    #[test]
    fn a_less_important_thread_never_takes_the_cpu_from_a_more_important_one() {
        let s = Scheduler::new(IDLE);
        s.add_with(1, 0, -5, ANY_CPU);
        s.add_with(2, 0, DEFAULT, ANY_CPU);
        assert_eq!(next(&s), 1);
        for _ in 0..10 * SLICE {
            s.tick_slice(0);
            assert!(!s.take_reschedule(0), "a slice ended for a less important thread");
        }
        assert_eq!(next(&s), 1, "a yield gave the CPU to a less important thread");
        // Equals still take turns: round-robin within a priority.
        s.add_with(3, 0, -5, ANY_CPU);
        assert_eq!(next(&s), 3);
        assert_eq!(next(&s), 1);
    }

    #[test]
    fn a_woken_important_thread_takes_this_cpu_at_once() {
        let s = Scheduler::new(IDLE);
        s.add_with(1, 0, DEFAULT, ANY_CPU);
        s.add_with(2, 0, -5, ANY_CPU);
        assert_eq!(next(&s), 2);
        s.suspend_current(0, State::Blocked);
        assert_eq!(next(&s), 1);
        assert!(!s.take_reschedule(0));
        s.wake(2, 0);
        assert!(s.take_reschedule(0), "the woken thread waits out a slice behind a less important one");
        assert_eq!(s.take_pokes(), 0, "nobody else needs telling");
        assert_eq!(next(&s), 2);
        assert_eq!(s.queued(), [1]);
    }

    #[test]
    fn a_sleeper_joins_the_set_of_its_cpu_and_any_cpu_may_wake_it() {
        let s = two_cpus();
        s.add_with(1, 0, DEFAULT, ANY_CPU);
        s.add_with(2, 0, DEFAULT, ANY_CPU);
        s.pick_next(0);
        s.pick_next(1);
        let (on0, on1) = (s.current(0), s.current(1));
        assert!(on0 != IDLE && on1 != IDLE);
        s.suspend_current(0, State::Sleeping(50));
        s.suspend_current(1, State::Sleeping(20));
        assert_eq!(s.first_wake_on(0), Some(50));
        assert_eq!(s.first_wake_on(1), Some(20));
        assert_eq!(s.first_wake(), Some(20));
        sleepers_match(&s);
        // CPU 0's interrupt at 20 wakes the sleeper of CPU 1, not its own.
        s.wake_sleepers(20, 0);
        assert_eq!(s.state(on1), Some(State::Running), "still on CPU 1 until it switches");
        assert_eq!(s.state(on0), Some(State::Sleeping(50)));
        assert_eq!(s.first_wake_on(1), None);
        sleepers_match(&s);
    }

    #[test]
    fn a_woken_important_thread_takes_another_cpu_with_an_interrupt() {
        let s = two_cpus();
        s.add_with(1, 0, DEFAULT, ANY_CPU);
        s.add_with(2, 0, 3, ANY_CPU);
        s.add_with(9, 0, -5, ANY_CPU);
        s.take_pokes();
        assert_eq!(next(&s), 9);
        s.pick_next(1);
        assert_eq!(s.current(1), 1);
        s.suspend_current(0, State::Blocked);
        assert_eq!(next(&s), 2);
        s.take_pokes();
        // Woken from CPU 1: CPU 0 runs the least important thread (3),
        // so CPU 0 is the one interrupted.
        s.wake(9, 1);
        assert_eq!(s.take_pokes(), 1 << 0, "the CPU with the least important thread was not interrupted");
        assert!(s.take_reschedule(0));
        assert!(!s.take_reschedule(1));
    }

    #[test]
    fn a_thread_runs_only_on_the_cpus_it_may() {
        let s = two_cpus();
        s.add_with(1, 0, DEFAULT, 1 << 1);
        assert_eq!(s.take_pokes(), 1 << 1, "the idling CPU it may run on was not told");
        assert!(!s.take_reschedule(0), "a CPU it may not run on was offered it");
        assert_eq!(next(&s), IDLE, "CPU 0 took a thread pinned to CPU 1");
        s.pick_next(1);
        assert_eq!(s.current(1), 1);
        assert_eq!(s.affinity(1), Some(1 << 1));
    }

    #[test]
    fn a_running_thread_leaves_a_cpu_it_may_no_longer_use() {
        let s = two_cpus();
        s.add_with(1, 0, DEFAULT, ANY_CPU);
        s.take_pokes();
        assert_eq!(next(&s), 1);
        assert!(s.set_affinity(1, 1 << 1, 0));
        assert!(s.take_reschedule(0), "the thread stayed on a CPU it may not use");
        assert_eq!(next(&s), IDLE);
        assert_eq!(s.take_pokes(), 1 << 1, "CPU 1 was not told of it");
        s.pick_next(1);
        assert_eq!(s.current(1), 1);
    }

    #[test]
    fn a_new_priority_moves_a_queued_thread_and_can_end_a_running_ones_turn() {
        let s = Scheduler::new(IDLE);
        s.add_with(1, 0, DEFAULT, ANY_CPU);
        s.add_with(2, 0, DEFAULT, ANY_CPU);
        s.add_with(3, 0, DEFAULT, ANY_CPU);
        assert_eq!(next(&s), 1);
        assert!(s.set_priority(3, -1, 0));
        assert_eq!(s.queued(), [3, 2], "the queued thread did not move up");
        assert!(s.take_reschedule(0), "a more important thread waits behind a running one");
        assert_eq!(next(&s), 3);
        // 3 now lowers itself below the others: it gives way.
        assert!(s.set_priority(3, 1, 0));
        assert!(s.take_reschedule(0));
        assert_eq!(next(&s), 2);
        assert!(!s.set_priority(IDLE, -5, 0), "an idle thread has no priority to set");
        assert!(!s.set_priority(77, -5, 0), "a thread that is not there has no priority to set");
    }

    #[test]
    fn randomized_with_priorities_and_affinity() {
        const CPUS: usize = 4;
        let idle_of = |cpu: usize| if cpu == 0 { IDLE } else { 1000 + cpu as u64 };
        let s = Scheduler::new(IDLE);
        for cpu in 1..CPUS {
            s.add_cpu(idle_of(cpu));
        }
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut random = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let threads = 24u64;
        for id in 1..=threads {
            let priority = (random() % 7) as Priority - 3;
            let affinity = if random() % 4 == 0 { 1 << (random() % CPUS as u64) } else { ANY_CPU };
            s.add_with(id, 0, priority, affinity);
        }
        // After each thing CPU `by` does: whoever is to be interrupted for
        // it has been asked to choose again, and is not `by` itself.
        let check_pokes = |s: &Scheduler, by: usize, step: u32| {
            let pokes = s.take_pokes();
            for other in 0..CPUS {
                if pokes & (1 << other) != 0 {
                    assert!(s.reschedule_of(other), "step {step}: CPU {other} interrupted for nothing");
                    assert_ne!(other, by, "step {step}: CPU {other} told itself");
                }
            }
        };
        check_pokes(&s, 0, 0);
        let mut now = 0;
        for step in 0..40_000u32 {
            let cpu = (random() % CPUS as u64) as usize;
            if step as usize % CPUS == 0 {
                now += 1;
                s.tick(now, 0);
                check_pokes(&s, 0, step);
            } else {
                s.tick_slice(cpu);
                check_pokes(&s, cpu, step);
            }
            let on_idle = s.current(cpu) == idle_of(cpu);
            match random() % 16 {
                0 if !on_idle => s.suspend_current(cpu, State::Sleeping(now + random() % 5)),
                1 if !on_idle => s.suspend_current(cpu, State::Blocked),
                2..=4 => {
                    s.wake(1 + random() % threads, cpu);
                }
                5 => {
                    s.set_priority(1 + random() % threads, (random() % 7) as Priority - 3, cpu);
                }
                6 => {
                    let affinity = if random() % 2 == 0 { 1 << (random() % CPUS as u64) } else { ANY_CPU };
                    s.set_affinity(1 + random() % threads, affinity, cpu);
                }
                _ => {}
            }
            check_pokes(&s, cpu, step);
            if random() % 3 != 0 {
                s.pick_next(cpu);
                check_pokes(&s, cpu, step);
                let current = s.current(cpu);
                assert!(s.allowed(current, cpu), "step {step}: CPU {cpu} runs thread {current}, which may not run there");
                // Strict: nobody allowed here and waiting is more
                // important than the thread just chosen.
                if let Some(best) = s.best_for(cpu) {
                    if current != idle_of(cpu) {
                        assert!(
                            s.priority(best) >= s.priority(current),
                            "step {step}: CPU {cpu} chose {current} over the more important {best}"
                        );
                    } else {
                        panic!("step {step}: CPU {cpu} idles while {best} may run there");
                    }
                }
            }
            for a in 0..CPUS {
                let current = s.current(a);
                for b in a + 1..CPUS {
                    assert_ne!(current, s.current(b), "one thread on CPUs {a} and {b}");
                }
                assert!(!s.queued().contains(&current), "thread {current} is on CPU {a} and in the queue");
            }
            for (n, id) in s.queued().iter().enumerate() {
                assert!(!s.queued().iter().skip(n + 1).any(|other| other == id), "thread {id} queued twice");
                assert_eq!(s.state(*id), Some(State::Ready));
            }
            sleepers_match(&s);
        }
    }

    /// A barrier for the CPUs-at-once tests that gives up when any of
    /// them has failed - std's would leave the rest waiting for ever.
    struct Gate {
        cpus: usize,
        arrived: AtomicUsize,
        round: AtomicUsize,
        failed: AtomicBool,
    }

    impl Gate {
        fn new(cpus: usize) -> Gate {
            Gate { cpus, arrived: AtomicUsize::new(0), round: AtomicUsize::new(0), failed: AtomicBool::new(false) }
        }

        /// Waits for all; true for exactly one of them (the last to come).
        fn wait(&self) -> bool {
            let round = self.round.load(SeqCst);
            if self.arrived.fetch_add(1, SeqCst) + 1 == self.cpus {
                self.arrived.store(0, SeqCst);
                self.round.fetch_add(1, SeqCst);
                return true;
            }
            while self.round.load(SeqCst) == round {
                assert!(!self.failed.load(SeqCst), "another CPU failed");
                std::thread::yield_now();
            }
            false
        }
    }

    /// Dropped when a CPU's host thread ends: if it ends by panicking,
    /// the gate is told, so the others stop waiting.
    struct Failed {
        gate: std::sync::Arc<Gate>,
        done: std::sync::Arc<AtomicBool>,
    }

    impl Drop for Failed {
        fn drop(&mut self) {
            if !self.done.load(SeqCst) {
                self.gate.failed.store(true, SeqCst);
            }
        }
    }

    // ---- CPUs at once (K193, journal entry NNN) -------------------------

    /// The policy as the kernel uses it, on host threads: one host thread
    /// per CPU, each doing what that CPU does - ticks, waits, wake-ups of
    /// any thread, new priorities and affinities, switches - all at once,
    /// against one scheduler. Each switch is carried out as the kernel's
    /// mechanism does (threads.rs, `schedule`): the CPU lets go of the
    /// thread it leaves, then waits until no other CPU is on the thread it
    /// takes, and claims it; a thread two CPUs took fails the claim.
    /// Every so often all CPUs stop at a barrier and everything is
    /// checked against everything (`check_consistent`). At the end every
    /// thread is woken, and each must then run.
    ///
    /// With `idle_ticks` false an idling CPU's timer does not run - a CPU
    /// that misses a thread queued as it went idle then stays idle until
    /// asked, where in the kernel its next tick, 10 ms later, would find
    /// the thread. So only that way do these tests show whether "Nobody
    /// sleeps with work waiting" holds by itself.
    fn cpus_at_once(cpus: usize, threads: u64, steps: u32, seed: u64, idle_ticks: bool) {
        use std::sync::Arc as Shared;
        use std::vec;

        const FREE: usize = usize::MAX;
        let idle_of = |cpu: usize| if cpu == 0 { IDLE } else { 1000 + cpu as u64 };
        let s = Shared::new(Scheduler::new(IDLE));
        for cpu in 1..cpus {
            s.add_cpu(idle_of(cpu));
        }
        let mut setup = seed;
        let mut random = move || {
            setup ^= setup << 13;
            setup ^= setup >> 7;
            setup ^= setup << 17;
            setup
        };
        for id in 1..=threads {
            let priority = (random() % 5) as Priority - 2;
            let affinity = if random() % 4 == 0 { 1 << (random() % cpus as u64) } else { ANY_CPU };
            s.add_with(id, 0, priority, affinity);
        }
        // Which CPU is on each thread (FREE: none), as the mechanism would
        // know it; idle threads are their CPU's for ever.
        let owner: Shared<Vec<AtomicUsize>> = Shared::new((0..=threads).map(|_| AtomicUsize::new(FREE)).collect());
        let now = Shared::new(AtomicU64::new(0));
        let barrier = Shared::new(Gate::new(cpus));
        const CHECK_EVERY: u32 = 2000;

        let workers: Vec<_> = (0..cpus)
            .map(|cpu| {
                let (s, owner, now, barrier) = (s.clone(), owner.clone(), now.clone(), barrier.clone());
                let done = Shared::new(AtomicBool::new(false));
                let failed = Failed { gate: barrier.clone(), done: done.clone() };
                let mut state = seed ^ (0x1234_5678_9ABC_DEF1u64.wrapping_mul(cpu as u64 + 1));
                std::thread::spawn(move || {
                    let _failed = failed;
                    let mut random = move || {
                        state ^= state << 13;
                        state ^= state >> 7;
                        state ^= state << 17;
                        state
                    };
                    let mine = |id: ThreadId| id >= 1 && id <= threads;
                    for step in 0..steps {
                        if step % CHECK_EVERY == 0 {
                            // Everyone stops between two actions, as if
                            // each CPU sat with interrupts on; one checks.
                            if barrier.wait() {
                                s.check_consistent();
                            }
                            barrier.wait();
                        }
                        let time = if cpu == 0 { now.fetch_add(1, SeqCst) + 1 } else { now.load(SeqCst) };
                        s.wake_sleepers(time, cpu);
                        if idle_ticks || s.current(cpu) != idle_of(cpu) {
                            s.tick_slice(cpu);
                        }
                        let on_idle = s.current(cpu) == idle_of(cpu);
                        match random() % 16 {
                            0 if !on_idle => s.suspend_current(cpu, State::Sleeping(time + random() % 5)),
                            1 if !on_idle => s.suspend_current(cpu, State::Blocked),
                            2..=5 => {
                                s.wake(1 + random() % threads, cpu);
                            }
                            6 => {
                                s.set_priority(1 + random() % threads, (random() % 5) as Priority - 2, cpu);
                            }
                            7 => {
                                let affinity = if random() % 2 == 0 { 1 << (random() % cpus as u64) } else { ANY_CPU };
                                s.set_affinity(1 + random() % threads, affinity, cpu);
                            }
                            _ => {}
                        }
                        s.take_pokes();
                        // A busy CPU chooses when asked or when its thread
                        // yields, waits or ends; an idling one only when
                        // asked - halted, it does nothing else.
                        let idling = s.current(cpu) == idle_of(cpu);
                        if s.take_reschedule(cpu) || (!idling && random() % 3 == 0) {
                            if let Some(Switch { from, to }) = s.pick_next(cpu) {
                                if mine(from) {
                                    owner[from as usize].store(FREE, SeqCst); // its registers saved
                                }
                                if mine(to) {
                                    while owner[to as usize].load(SeqCst) != FREE {
                                        core::hint::spin_loop(); // its last CPU still saving them
                                    }
                                    let claimed = owner[to as usize].compare_exchange(FREE, cpu, SeqCst, SeqCst);
                                    assert!(claimed.is_ok(), "step {step}: CPU {cpu} took thread {to}, which CPU {:?} has", claimed);
                                }
                            }
                        }
                    }
                    done.store(true, SeqCst);
                })
            })
            .collect();
        for worker in workers {
            worker.join().expect("a CPU failed");
        }
        s.check_consistent();
        sleepers_match(&s);

        // Nobody lost: wake everyone, let them be allowed anywhere at one
        // priority (priorities are strict: a busy more important thread
        // would rightly keep a less important one from running), and run
        // the CPUs one after another until each thread has run.
        for id in 1..=threads {
            s.set_affinity(id, ANY_CPU, 0);
            s.set_priority(id, DEFAULT, 0);
            s.wake(id, 0);
        }
        let mut ran = vec![false; threads as usize + 1];
        for round in 0..threads * 4 {
            for cpu in 0..cpus {
                for _ in 0..SLICE {
                    s.tick_slice(cpu);
                }
                s.pick_next(cpu);
                let current = s.current(cpu);
                if mine(current) {
                    ran[current as usize] = true;
                }
            }
            if round % 8 == 0 {
                s.check_consistent();
            }
        }
        for id in 1..=threads {
            assert!(ran[id as usize], "thread {id} never ran once woken");
        }

        fn mine(id: ThreadId) -> bool {
            id >= 1 && id < 1000
        }
    }

    #[test]
    fn two_cpus_at_once() {
        cpus_at_once(2, 8, 60_000, 0x2545_F491_4F6C_DD1D, true);
    }

    #[test]
    fn four_cpus_at_once() {
        cpus_at_once(4, 24, 60_000, 0x9E37_79B9_7F4A_7C15, true);
    }

    #[test]
    fn eight_cpus_at_once_few_threads() {
        // Fewer threads than CPUs, often: idling and stealing at once.
        cpus_at_once(8, 6, 40_000, 0xD1B5_4A32_D192_ED03, true);
    }

    /// "Nobody sleeps with work waiting", the order the CPUs-at-once
    /// tests cannot catch (journal entry NNN, bug 3): CPU 1 looks, finds
    /// nothing, and - before it says it idles - CPU 0 wakes a thread.
    /// CPU 1 is not idle yet, so the thread waits in busy CPU 0's queue
    /// and nobody is asked. Without its second look CPU 1 idled past it,
    /// and the thread waited for CPU 0's slice to end.
    #[test]
    fn a_cpu_about_to_idle_takes_a_thread_woken_as_it_looked() {
        let s = std::sync::Arc::new(two_cpus());
        // Thread 3 runs on CPU 0, then blocks: it last ran there.
        s.add(3, 0);
        assert_eq!(s.pick_next(0), Some(Switch { from: IDLE, to: 3 }));
        s.suspend_current(0, State::Blocked);
        s.pick_next(0);
        // Thread 1 on CPU 0, thread 2 on CPU 1; nothing queued.
        s.add(1, 0);
        assert_eq!(s.pick_next(0), Some(Switch { from: IDLE, to: 1 }));
        s.add(2, 1);
        assert_eq!(s.pick_next(1), Some(Switch { from: 100, to: 2 }));
        assert!(s.queued().is_empty());
        s.take_pokes();
        let other = s.clone();
        at_race("before idling", move || {
            assert!(other.wake(3, 0)); // on CPU 0, at that moment
            assert_eq!(other.queued_on(0), [3], "queued at busy CPU 0, its last");
        });
        s.suspend_current(1, State::Blocked);
        s.pick_next(1);
        assert!(
            s.current(1) == 3 || s.reschedule_of(1),
            "CPU 1 idles, not asked, while thread 3 waits in busy CPU 0's queue: {:?}",
            s.queued()
        );
    }

    #[test]
    fn an_exited_thread_is_removable_only_once_off_its_cpu() {
        let s = Scheduler::new(IDLE);
        s.add(1, 0);
        s.pick_next(0);
        assert!(!s.removable(1), "running");
        s.suspend_current(0, State::Exited);
        assert!(!s.removable(1), "exited, but still on its CPU");
        s.pick_next(0);
        assert!(s.removable(1));
        assert!(s.removable(1), "asking changes nothing");
        assert!(s.remove(1));
        assert!(!s.removable(1) && s.state(1).is_none());
    }

    #[test]
    fn cpus_at_once_without_idle_ticks() {
        cpus_at_once(4, 6, 60_000, 0x6A09_E667_F3BC_C908, false);
        cpus_at_once(8, 4, 40_000, 0xBB67_AE85_84CA_A73B, false);
    }
}
