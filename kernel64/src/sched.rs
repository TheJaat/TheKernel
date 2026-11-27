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
//!     124). Runnable threads wait in ONE set of FIFO queues, one per
//!     priority, shared by all CPUs: a CPU that needs a thread takes the
//!     most important one allowed on it, and of those the one that has
//!     waited longest. Work spreads over the CPUs by itself, and a
//!     thread runs on whichever allowed CPU is free.
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
//!   - each CPU has an idle thread, which runs only when the queue is
//!     empty. It never sits in the queue, and never runs on another CPU.
//!
//! One rule exists only because there is more than one CPU. A thread
//! that has said it will block is, for a moment, still running: it has
//! changed its state but not yet left its CPU. If it is woken in that
//! moment it must NOT go into the queue - another CPU would take it
//! from there and run it while the first CPU is still on its stack. It
//! simply becomes Running again, and when it gets to the switch it
//! finds it has no reason to leave. So: only a thread that is on no
//! CPU is ever in the queue.
//!
//! Nothing here locks anything. The caller holds one lock around every
//! call, and - this matters - keeps holding it across the switch that
//! `pick_next` asks for, so that no other CPU can act on a decision
//! that has been made but not yet carried out.

use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::vec::Vec;

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

/// What a thread asks of the scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Attributes {
    priority: Priority,
    /// One bit per CPU it may run on.
    affinity: u64,
}

/// The runnable threads that are on no CPU: a FIFO queue per priority,
/// the most important first. Each entry carries its thread's affinity,
/// so that choosing - the scheduler's hottest path - looks nothing up.
pub struct Ready {
    levels: [VecDeque<(ThreadId, u64)>; LEVELS],
    /// One bit per level that has anyone in it, so that finding the
    /// first is one instruction (trailing_zeros), not forty looks.
    occupied: u64,
}

impl Ready {
    fn new() -> Ready {
        Ready { levels: core::array::from_fn(|_| VecDeque::new()), occupied: 0 }
    }

    fn push_back(&mut self, id: ThreadId, priority: Priority, affinity: u64) {
        let level = level(priority);
        self.levels[level].push_back((id, affinity));
        self.occupied |= 1 << level;
    }

    /// Clears `level`'s bit if it has emptied.
    fn update(&mut self, level: usize) {
        if self.levels[level].is_empty() {
            self.occupied &= !(1 << level);
        }
    }

    /// Takes `id` out, wherever it is; false if it was not there.
    fn remove(&mut self, id: ThreadId) -> bool {
        for level in 0..LEVELS {
            if let Some(at) = self.levels[level].iter().position(|&(other, _)| other == id) {
                self.levels[level].remove(at);
                self.update(level);
                return true;
            }
        }
        false
    }

    /// A queued thread's new affinity; false if it is not queued.
    fn set_affinity(&mut self, id: ThreadId, affinity: u64) -> bool {
        for queue in &mut self.levels {
            if let Some(entry) = queue.iter_mut().find(|(other, _)| *other == id) {
                entry.1 = affinity;
                return true;
            }
        }
        false
    }

    /// Where the thread `cpu` would take next is: its priority's level
    /// and its place there - the most important allowed on `cpu`, the
    /// longest waiting of those.
    fn best_for(&self, cpu: usize) -> Option<(usize, usize)> {
        let bit = 1u64 << cpu;
        let mut left = self.occupied;
        while left != 0 {
            let level = left.trailing_zeros() as usize;
            if let Some(at) = self.levels[level].iter().position(|&(_, affinity)| affinity & bit != 0) {
                return Some((level, at));
            }
            left &= left - 1;
        }
        None
    }

    /// Takes the thread at `place` out: it, its priority and affinity.
    fn take(&mut self, (level, at): (usize, usize)) -> (ThreadId, Priority, u64) {
        let (id, affinity) = self.levels[level].remove(at).expect("best_for's place");
        self.update(level);
        (id, priority_of(level), affinity)
    }

    #[cfg(test)]
    fn get(&self, (level, at): (usize, usize)) -> ThreadId {
        self.levels[level][at].0
    }

    pub fn contains(&self, id: &ThreadId) -> bool {
        self.levels.iter().any(|queue| queue.iter().any(|(other, _)| other == id))
    }

    pub fn is_empty(&self) -> bool {
        self.occupied == 0
    }

    pub fn len(&self) -> usize {
        self.levels.iter().map(VecDeque::len).sum()
    }

    /// In the order they would be taken: the most important first, and
    /// within a priority the longest waiting first.
    pub fn iter(&self) -> impl Iterator<Item = &ThreadId> {
        self.levels.iter().flatten().map(|(id, _)| id)
    }
}

fn level(priority: Priority) -> usize {
    (priority.clamp(HIGHEST, LOWEST) - HIGHEST) as usize
}

fn priority_of(level: usize) -> Priority {
    level as Priority + HIGHEST
}

/// What an idle thread counts as, against a priority: less important
/// than any, so that anyone ready may take its CPU.
const IDLE_PRIORITY: Priority = LOWEST + 1;

/// What the scheduler keeps for each CPU.
struct Cpu {
    /// The thread on this CPU now - until the next `pick_next`, even if
    /// it has already said it will sleep, block or exit.
    current: ThreadId,
    idle: ThreadId,
    slice_left: u64,
    /// Set when this CPU should choose again as soon as possible - a
    /// wakeup while it idles, or the end of a slice. On an idling CPU
    /// it is also a promise: a thread in the queue has been offered to
    /// this CPU, so the next one is offered to somebody else.
    reschedule: bool,
    /// The current thread's priority (IDLE_PRIORITY for the idle
    /// thread) and affinity, kept here so that a switch looks nothing up.
    priority: Priority,
    affinity: u64,
}

pub struct Scheduler {
    states: BTreeMap<ThreadId, State>,
    /// Each thread's priority and affinity; an idle thread has none.
    attributes: BTreeMap<ThreadId, Attributes>,
    ready: Ready,
    /// The sleepers, sorted by when they wake (then by number): a tick
    /// looks at the first, not at every thread (K96, journal entry NNN).
    /// Kept by `set_state`, the only way a state changes once a thread
    /// is added, so it always holds exactly the Sleeping threads.
    sleepers: BTreeSet<(u64, ThreadId)>,
    cpus: Vec<Cpu>,
    /// The CPUs that must be interrupted, one bit each: idling CPUs
    /// that were offered a thread by another CPU and cannot know it.
    /// The policy cannot interrupt anyone; the caller collects this
    /// with `take_pokes` and does.
    pokes: u64,
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
        let mut states = BTreeMap::new();
        states.insert(idle, State::Ready); // ready, but never queued
        states.insert(running, State::Running);
        let priority = if running == idle { IDLE_PRIORITY } else { DEFAULT };
        let cpu = Cpu { current: running, idle, slice_left: SLICE, reschedule: false, priority, affinity: ANY_CPU };
        let mut attributes = BTreeMap::new();
        if running != idle {
            attributes.insert(running, Attributes { priority: DEFAULT, affinity: ANY_CPU });
        }
        Scheduler { states, attributes, ready: Ready::new(), sleepers: BTreeSet::new(), cpus: alloc::vec![cpu], pokes: 0 }
    }

    /// One more CPU, running its idle thread `idle`. Returns its number:
    /// CPUs are numbered in the order they are added, the first being 0.
    pub fn add_cpu(&mut self, idle: ThreadId) -> usize {
        assert!(self.states.insert(idle, State::Running).is_none(), "thread {idle} added twice");
        // A CPU that arrives when there is already work takes some at
        // its first chance.
        let reschedule = !self.ready.is_empty();
        self.cpus.push(Cpu { current: idle, idle, slice_left: SLICE, reschedule, priority: IDLE_PRIORITY, affinity: ANY_CPU });
        self.cpus.len() - 1
    }

    pub fn cpu_count(&self) -> usize {
        self.cpus.len()
    }

    /// The thread on `cpu`.
    pub fn current(&self, cpu: usize) -> ThreadId {
        self.cpus[cpu].current
    }

    /// The CPU `id` is on, if it is on one.
    pub fn cpu_of(&self, id: ThreadId) -> Option<usize> {
        self.cpus.iter().position(|cpu| cpu.current == id)
    }

    /// Whether `id` is some CPU's idle thread.
    pub fn is_idle(&self, id: ThreadId) -> bool {
        self.cpus.iter().any(|cpu| cpu.idle == id)
    }

    pub fn state(&self, id: ThreadId) -> Option<State> {
        self.states.get(&id).copied()
    }

    pub fn thread_count(&self) -> usize {
        self.states.len()
    }

    /// A new thread, ready to run, at the default priority on any CPU.
    /// `cpu` is the CPU this is called on: if it is idling, it should
    /// take the thread at once.
    pub fn add(&mut self, id: ThreadId, cpu: usize) {
        self.add_with(id, cpu, DEFAULT, ANY_CPU);
    }

    /// A new thread, ready to run, with its priority and affinity.
    pub fn add_with(&mut self, id: ThreadId, cpu: usize, priority: Priority, affinity: u64) {
        assert!(self.states.insert(id, State::Ready).is_none(), "thread {id} added twice");
        let priority = priority.clamp(HIGHEST, LOWEST);
        self.attributes.insert(id, Attributes { priority, affinity });
        self.enqueue(id, cpu);
    }

    /// `id`'s priority; None for an idle thread or one not known.
    pub fn priority(&self, id: ThreadId) -> Option<Priority> {
        self.attributes.get(&id).map(|a| a.priority)
    }

    /// The CPUs `id` may run on.
    pub fn affinity(&self, id: ThreadId) -> Option<u64> {
        self.attributes.get(&id).map(|a| a.affinity)
    }

    /// Gives `id` a new priority. A queued thread moves to its new
    /// queue (and may take a CPU from a less important one); a running
    /// one that is now less important than someone waiting for its CPU
    /// gives way at once. `cpu` is the CPU this is called on. False if
    /// `id` is not a thread here (or is an idle thread).
    pub fn set_priority(&mut self, id: ThreadId, priority: Priority, cpu: usize) -> bool {
        let Some(attributes) = self.attributes.get_mut(&id) else { return false };
        attributes.priority = priority.clamp(HIGHEST, LOWEST);
        let Attributes { priority, affinity } = *attributes;
        if self.ready.remove(id) {
            self.ready.push_back(id, priority, affinity);
            self.offer(priority, affinity, cpu);
        } else if let Some(on) = self.cpu_of(id) {
            self.cpus[on].priority = priority;
            if self.someone_waiting_for(on) {
                self.ask(on, cpu);
            }
        }
        true
    }

    /// Gives `id` the CPUs it may run on, one bit each. A running thread
    /// on a CPU no longer allowed leaves it at once. The caller checks
    /// that `affinity` names a CPU there is. False as for set_priority.
    pub fn set_affinity(&mut self, id: ThreadId, affinity: u64, cpu: usize) -> bool {
        let Some(attributes) = self.attributes.get_mut(&id) else { return false };
        attributes.affinity = affinity;
        let priority = attributes.priority;
        if self.ready.set_affinity(id, affinity) {
            self.offer(priority, affinity, cpu);
        } else if let Some(on) = self.cpu_of(id) {
            self.cpus[on].affinity = affinity;
            if affinity & (1u64 << on) == 0 {
                self.ask(on, cpu);
            }
        }
        true
    }

    /// Whether `id` may run on `cpu`. An idle thread runs only on its
    /// own CPU, and is never asked. (The scheduler itself reads the
    /// affinities kept with each queued thread and each CPU.)
    #[cfg(test)]
    fn allowed(&self, id: ThreadId, cpu: usize) -> bool {
        self.attributes.get(&id).is_none_or(|a| a.affinity & (1u64 << cpu) != 0)
    }

    /// The thread `cpu` would take next: the most important allowed on
    /// it, the longest waiting of those.
    #[cfg(test)]
    fn best_for(&self, cpu: usize) -> Option<ThreadId> {
        self.ready.best_for(cpu).map(|place| self.ready.get(place))
    }

    /// Whether a thread waiting may take `cpu` from its current one at
    /// the end of a slice: one at least as important that may run there
    /// - or anyone at all, if the current one may no longer run there.
    fn someone_waiting_for(&self, cpu: usize) -> bool {
        let this = &self.cpus[cpu];
        let Some((level, _)) = self.ready.best_for(cpu) else { return false };
        if this.current == this.idle || this.affinity & (1u64 << cpu) == 0 {
            return true;
        }
        priority_of(level) <= this.priority
    }

    /// Asks `cpu` to choose again; interrupted if it is not `from`, the
    /// CPU this is decided on.
    fn ask(&mut self, cpu: usize, from: usize) {
        self.cpus[cpu].reschedule = true;
        if cpu != from {
            self.pokes |= 1 << cpu;
        }
    }

    /// One timer tick at time `now`, on the CPU that keeps the time:
    /// wakes sleepers whose time has come, and counts down the slice of
    /// the thread on `cpu`.
    pub fn tick(&mut self, now: u64, cpu: usize) {
        self.wake_sleepers(now, cpu);
        self.tick_slice(cpu);
    }

    /// Wakes every sleeper whose time is `now` or before: the first ones
    /// of the sorted set, so this looks at no thread that is not due and
    /// allocates nothing.
    pub fn wake_sleepers(&mut self, now: u64, cpu: usize) {
        while let Some(&(until, id)) = self.sleepers.first() {
            if until > now {
                break;
            }
            self.make_runnable(id, cpu); // takes it out of the set
        }
    }

    /// When the first sleeper wakes, if anyone sleeps: what a one-shot
    /// timer is set for.
    pub fn first_wake(&self) -> Option<u64> {
        self.sleepers.first().map(|&(until, _)| until)
    }

    /// Changes `id`'s state, keeping `sleepers` right. Every change of an
    /// added thread's state goes through here.
    fn set_state(&mut self, id: ThreadId, state: State) {
        if let Some(State::Sleeping(until)) = self.states.insert(id, state) {
            let _ = until;
        }
        if let State::Sleeping(until) = state {
            self.sleepers.insert((until, id));
        }
    }

    /// One timer tick on a CPU that does not keep the time: only the
    /// slice of the thread on `cpu`.
    pub fn tick_slice(&mut self, cpu: usize) {
        let waiting = self.someone_waiting_for(cpu);
        let this = &mut self.cpus[cpu];
        if this.current == this.idle {
            // An idling CPU has no slice to finish: work that arrived
            // from another CPU since its last tick is taken now.
            this.reschedule |= waiting;
            return;
        }
        this.slice_left = this.slice_left.saturating_sub(1);
        if this.slice_left == 0 {
            if waiting {
                this.reschedule = true;
            } else {
                // Nobody waiting: a fresh slice, so a thread that turns
                // up later still finds a full slice ahead of it to wait
                // out - not an exhausted one that hands over at once.
                this.slice_left = SLICE;
            }
        }
    }

    /// Whether `cpu` has been asked to choose again, the request left as
    /// it is.
    pub fn reschedule_pending(&self, cpu: usize) -> bool {
        self.cpus[cpu].reschedule
    }

    /// Whether `cpu` should choose again; clears the request.
    pub fn take_reschedule(&mut self, cpu: usize) -> bool {
        core::mem::take(&mut self.cpus[cpu].reschedule)
    }

    /// The CPUs to interrupt because of what was done since the last
    /// call, one bit each; clears the set. Never the CPU that did it.
    pub fn take_pokes(&mut self) -> u64 {
        core::mem::take(&mut self.pokes)
    }

    /// Into the queue - for a thread that is on no CPU.
    fn enqueue(&mut self, id: ThreadId, cpu: usize) {
        debug_assert!(self.cpu_of(id).is_none(), "thread {id} queued while on a CPU");
        self.set_state(id, State::Ready);
        let Attributes { priority, affinity } =
            self.attributes.get(&id).copied().unwrap_or(Attributes { priority: DEFAULT, affinity: ANY_CPU });
        self.enqueue_known(id, priority, affinity, cpu);
    }

    /// `enqueue`, its attributes already known (a switch: no look-up).
    fn enqueue_known(&mut self, id: ThreadId, priority: Priority, affinity: u64, cpu: usize) {
        self.set_state(id, State::Ready);
        self.ready.push_back(id, priority, affinity);
        self.offer(priority, affinity, cpu);
    }

    /// A thread (of `priority` and `affinity`) has just joined the queue,
    /// put there by `cpu`: finds
    /// a CPU to take it. An idling CPU it may run on: this one if it
    /// idles - it needs no telling - or another, which does: it sits
    /// halted, and without an interrupt would look at the queue only at
    /// its next timer tick.
    ///
    /// A CPU already offered a thread is passed over, so that two
    /// threads made ready one after the other get two CPUs, not the
    /// same one twice.
    ///
    /// If no allowed CPU idles, the thread may still take one from a
    /// less important thread: the allowed CPU running the least
    /// important one, this CPU first among equals (no interrupt). If
    /// none is less important, nothing is done: it waits for a slice to
    /// end, as it always did.
    fn offer(&mut self, priority: Priority, affinity: u64, cpu: usize) {
        let count = self.cpus.len();
        let allowed = |c: usize| affinity & (1u64 << c) != 0;
        let free = |s: &Scheduler, c: usize| {
            let this = &s.cpus[c];
            this.current == this.idle && !this.reschedule && allowed(c)
        };
        if let Some(chosen) = (0..count).map(|n| (cpu + n) % count).find(|&c| free(self, c)) {
            self.ask(chosen, cpu);
            return;
        }
        let mut victim: Option<(usize, Priority)> = None;
        for c in (0..count).map(|n| (cpu + n) % count) {
            let this = &self.cpus[c];
            if this.current == this.idle || this.reschedule || !allowed(c) {
                continue;
            }
            let theirs = this.priority;
            if theirs > priority && victim.is_none_or(|(_, worst)| theirs > worst) {
                victim = Some((c, theirs));
            }
        }
        if let Some((chosen, _)) = victim {
            self.ask(chosen, cpu);
        }
    }

    /// Ends the sleep or block of `id`. If it has not left its CPU yet
    /// it stays there, running; otherwise it joins the queue.
    fn make_runnable(&mut self, id: ThreadId, cpu: usize) {
        if self.cpu_of(id).is_some() {
            self.set_state(id, State::Running);
        } else {
            self.enqueue(id, cpu);
        }
    }

    /// Wakes a blocked or sleeping thread. False if it was neither.
    /// `cpu` is the CPU this is called on.
    pub fn wake(&mut self, id: ThreadId, cpu: usize) -> bool {
        match self.states.get(&id) {
            Some(State::Blocked | State::Sleeping(_)) => {
                self.make_runnable(id, cpu);
                true
            }
            _ => false,
        }
    }

    /// Says that the thread on `cpu` will leave it at the next
    /// `pick_next`, into `state` (Sleeping, Blocked or Exited). Until
    /// then it is still there, and can still be woken.
    pub fn suspend_current(&mut self, cpu: usize, state: State) {
        assert!(
            matches!(state, State::Sleeping(_) | State::Blocked | State::Exited),
            "suspend into {state:?}"
        );
        let this = &self.cpus[cpu];
        // The idle thread is what runs when nothing else can, so it can
        // never wait for anything: pick_next would simply choose it again.
        assert!(this.current != this.idle, "the idle thread cannot sleep, block or exit");
        let current = this.current;
        self.set_state(current, state);
    }

    /// Takes back a `suspend_current` into Sleeping or Blocked: the
    /// thread looked again and found it has nothing to wait for. (Not
    /// Exited: that cannot be taken back.)
    pub fn resume_current(&mut self, cpu: usize) {
        let id = self.cpus[cpu].current;
        if let Some(State::Blocked | State::Sleeping(_)) = self.states.get(&id) {
            self.set_state(id, State::Running);
        }
    }

    /// Chooses who runs next on `cpu`. The current thread, if still
    /// running, goes to the back of the queue (unless it is the idle
    /// thread). Returns the switch to make, or None to keep running the
    /// current thread.
    pub fn pick_next(&mut self, cpu: usize) -> Option<Switch> {
        let from = self.cpus[cpu].current;
        let idle = self.cpus[cpu].idle;
        // Choosing is what a request to choose asks for. Left set, it
        // would follow the CPU into idling and read there as an offer
        // nobody made.
        self.cpus[cpu].reschedule = false;
        let still_running = self.states.get(&from) == Some(&State::Running);
        // A running thread may stay only on a CPU it is allowed.
        let (mine, my_affinity) = (self.cpus[cpu].priority, self.cpus[cpu].affinity);
        let may_stay = still_running && my_affinity & (1u64 << cpu) != 0;

        // The best waiting thread - unless it is less important than the
        // one running, which keeps the CPU (priorities are strict).
        let candidate = self
            .ready
            .best_for(cpu)
            .filter(|&(level, _)| !(may_stay && from != idle && priority_of(level) > mine));
        let Some(place) = candidate else {
            if may_stay {
                self.cpus[cpu].slice_left = SLICE;
                return None; // nobody else may have the CPU
            }
            let switch = self.switch_to(cpu, from, idle, IDLE_PRIORITY, ANY_CPU);
            if still_running && from != idle {
                // No longer allowed here: back in the queue, for a CPU
                // it may run on.
                self.enqueue_known(from, mine, my_affinity, cpu);
            }
            return switch;
        };
        let (to, priority, affinity) = self.ready.take(place);
        // The switch comes first: `from` is on no CPU from here on, and
        // only then may it be queued.
        let switch = self.switch_to(cpu, from, to, priority, affinity);
        if still_running {
            if from == idle {
                self.set_state(from, State::Ready); // ready, never queued
            } else {
                // It gave way on this CPU; another may be idling, or
                // running someone less important.
                self.enqueue_known(from, mine, my_affinity, cpu);
            }
        }
        switch
    }

    fn switch_to(&mut self, cpu: usize, from: ThreadId, to: ThreadId, priority: Priority, affinity: u64) -> Option<Switch> {
        self.set_state(to, State::Running);
        let this = &mut self.cpus[cpu];
        this.current = to;
        this.priority = priority;
        this.affinity = affinity;
        this.slice_left = SLICE;
        (from != to).then_some(Switch { from, to })
    }

    /// Forgets an exited thread, once it is off every CPU. Returns
    /// whether it was there to forget.
    pub fn remove(&mut self, id: ThreadId) -> bool {
        if self.cpu_of(id).is_some() || self.states.get(&id) != Some(&State::Exited) {
            return false;
        }
        self.states.remove(&id);
        self.attributes.remove(&id);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDLE: ThreadId = 0;

    /// Runs `pick_next` and returns who is running afterwards.
    fn next(s: &mut Scheduler) -> ThreadId {
        s.pick_next(0);
        s.current(0)
    }

    #[test]
    fn idle_runs_when_nothing_else_can() {
        let mut s = Scheduler::new(IDLE);
        assert_eq!(s.pick_next(0), None);
        assert_eq!(s.current(0), IDLE);
    }

    #[test]
    fn round_robin_in_order_and_idle_never_queued() {
        let mut s = Scheduler::new(IDLE);
        for id in 1..=3 {
            s.add(id, 0);
        }
        assert!(s.take_reschedule(0), "new work while idle asks for a switch");
        let order: Vec<_> = (0..7).map(|_| next(&mut s)).collect();
        assert_eq!(order, vec![1, 2, 3, 1, 2, 3, 1]);
    }

    #[test]
    fn a_slice_runs_out_only_when_someone_is_waiting() {
        let mut s = Scheduler::new(IDLE);
        s.add(1, 0);
        next(&mut s);
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
        assert_eq!(next(&mut s), 2);
    }

    #[test]
    fn sleepers_wake_on_their_tick() {
        let mut s = Scheduler::new(IDLE);
        s.add(1, 0);
        next(&mut s);
        s.suspend_current(0, State::Sleeping(5));
        assert_eq!(next(&mut s), IDLE);
        s.take_reschedule(0);

        s.tick(4, 0);
        assert_eq!(s.state(1), Some(State::Sleeping(5)));
        assert!(!s.take_reschedule(0));
        s.tick(5, 0);
        assert_eq!(s.state(1), Some(State::Ready));
        assert!(s.take_reschedule(0), "the idle CPU should switch at once");
        assert_eq!(next(&mut s), 1);
    }

    #[test]
    fn blocked_threads_are_skipped_until_woken() {
        let mut s = Scheduler::new(IDLE);
        s.add(1, 0);
        s.add(2, 0);
        assert_eq!(next(&mut s), 1);
        s.suspend_current(0, State::Blocked);
        assert_eq!(next(&mut s), 2);
        assert_eq!(next(&mut s), 2, "1 is blocked; 2 keeps going");

        assert!(s.wake(1, 0));
        assert!(!s.wake(1, 0), "already awake");
        assert_eq!(next(&mut s), 1);
    }

    #[test]
    fn a_thread_that_blocks_and_is_woken_before_switching_keeps_running() {
        // The race a wait queue must survive: block, then the wakeup
        // arrives before the switch happens.
        let mut s = Scheduler::new(IDLE);
        s.add(1, 0);
        next(&mut s);
        s.suspend_current(0, State::Blocked);
        s.wake(1, 0);
        assert_eq!(next(&mut s), 1);
        assert_eq!(s.state(1), Some(State::Running));
    }

    #[test]
    fn exited_threads_leave_and_are_removed_once_off_the_cpu() {
        let mut s = Scheduler::new(IDLE);
        s.add(1, 0);
        next(&mut s);
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
        let mut s = Scheduler::new(IDLE);
        s.suspend_current(0, State::Sleeping(10));
    }

    #[test]
    fn a_boot_thread_can_sleep_and_exit_leaving_idle() {
        const BOOT: ThreadId = 1;
        let mut s = Scheduler::with_running(IDLE, BOOT);
        s.suspend_current(0, State::Sleeping(3));
        assert_eq!(s.pick_next(0), Some(Switch { from: BOOT, to: IDLE }));
        s.tick(3, 0);
        assert!(s.take_reschedule(0));
        assert_eq!(next(&mut s), BOOT);
        s.suspend_current(0, State::Exited);
        assert_eq!(next(&mut s), IDLE);
        assert!(s.remove(BOOT));
        assert_eq!(s.thread_count(), 1);
    }

    /// Many threads, random operations: every thread that is ready is
    /// eventually run, nobody runs twice in a row while others wait, and
    /// the idle thread is never queued.
    /// The sorted set holds exactly the Sleeping threads, at their times.
    fn sleepers_match(s: &Scheduler) {
        let sleeping: BTreeSet<(u64, ThreadId)> = s
            .states
            .iter()
            .filter_map(|(&id, state)| match state {
                State::Sleeping(until) => Some((*until, id)),
                _ => None,
            })
            .collect();
        assert_eq!(s.sleepers, sleeping, "the sleeper set and the states disagree");
    }

    #[test]
    fn sleepers_wake_in_their_order_and_leave_the_set_when_woken_early() {
        let mut s = Scheduler::new(IDLE);
        for id in 1..=4 {
            s.add(id, 0);
        }
        // Threads 1-4 go to sleep, in turn, until 30, 10, 10 and 20.
        for (id, until) in [(1, 30), (2, 10), (3, 10), (4, 20)] {
            assert_eq!(next(&mut s), id);
            s.suspend_current(0, State::Sleeping(until));
        }
        assert_eq!(next(&mut s), IDLE);
        assert_eq!(s.first_wake(), Some(10));
        sleepers_match(&s);
        // Thread 4 is woken early, as a futex or a message would.
        assert!(s.wake(4, 0));
        assert_eq!(s.sleepers.len(), 3);
        sleepers_match(&s);
        // At 9 nobody is due; at 10 both of that time are, not thread 1.
        s.tick(9, 0);
        assert_eq!(s.sleepers.len(), 3);
        s.tick(10, 0);
        assert_eq!(s.state(2), Some(State::Ready));
        assert_eq!(s.state(3), Some(State::Ready));
        assert_eq!(s.state(1), Some(State::Sleeping(30)));
        assert_eq!(s.first_wake(), Some(30));
        sleepers_match(&s);
        // A sleep taken back (the thread had nothing to wait for).
        let current = next(&mut s);
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
        let mut s = Scheduler::new(IDLE);
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
            assert!(!s.ready.contains(&IDLE));
            if before == after && after != IDLE {
                assert!(s.ready.is_empty(), "kept the CPU while others waited");
            }
        }
        for id in 1..=20 {
            assert!(ran.get(&id).copied().unwrap_or(0) > 50, "thread {id} starved");
        }
    }

    // ---- more than one CPU ------------------------------------------------

    /// Two CPUs: idle threads 0 and 100.
    fn two_cpus() -> Scheduler {
        let mut s = Scheduler::new(IDLE);
        assert_eq!(s.add_cpu(100), 1);
        s
    }

    #[test]
    fn two_cpus_take_different_threads_from_the_one_queue() {
        let mut s = two_cpus();
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
        let mut s = two_cpus();
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
        let mut s = two_cpus();
        s.add(1, 0);
        s.add(2, 0);
        s.pick_next(0);
        s.pick_next(1);
        s.suspend_current(0, State::Blocked);
        s.suspend_current(1, State::Blocked);
        assert_eq!(s.pick_next(1), Some(Switch { from: 2, to: 100 }));
        assert_eq!(s.pick_next(0), Some(Switch { from: 1, to: IDLE }));
        assert!(!s.ready.contains(&IDLE) && !s.ready.contains(&100));
        assert!(s.is_idle(100) && s.is_idle(IDLE) && !s.is_idle(1));
    }

    #[test]
    fn woken_before_leaving_its_cpu_a_thread_is_not_put_in_the_queue() {
        // The race that only exists with two CPUs. Thread 1, on CPU 0,
        // says it will block. Before it has switched away, CPU 1 wakes
        // it. In the queue, CPU 1 could take it and run it - on a stack
        // CPU 0 is still using.
        let mut s = two_cpus();
        s.add(1, 0);
        s.pick_next(0);
        s.suspend_current(0, State::Blocked);
        assert!(s.wake(1, 1));
        assert!(s.ready.is_empty(), "a thread still on a CPU was queued");
        assert!(!s.take_reschedule(1), "there is nothing for CPU 1 to take");
        assert_eq!(s.pick_next(1), None);
        assert_eq!(s.state(1), Some(State::Running));
        // And CPU 0, reaching the switch, finds no reason to leave.
        assert_eq!(s.pick_next(0), None);
        assert_eq!(s.current(0), 1);
    }

    #[test]
    fn a_sleeper_whose_time_comes_before_it_left_its_cpu_stays_too() {
        let mut s = two_cpus();
        s.add(1, 1);
        s.pick_next(1);
        s.suspend_current(1, State::Sleeping(5));
        s.tick(5, 0); // the time is kept on CPU 0
        assert!(s.ready.is_empty());
        assert_eq!(s.state(1), Some(State::Running));
        assert_eq!(s.pick_next(1), None);
    }

    #[test]
    fn a_wait_can_be_taken_back() {
        let mut s = Scheduler::new(IDLE);
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
        let mut s = two_cpus();
        s.add(1, 0);
        s.pick_next(1); // CPU 1 takes it
        s.suspend_current(1, State::Exited);
        assert!(!s.remove(1), "CPU 1 is still on its stack");
        s.pick_next(1);
        assert!(s.remove(1));
    }

    #[test]
    fn an_idle_cpu_takes_work_at_its_tick_and_a_busy_one_at_the_end_of_its_slice() {
        let mut s = two_cpus();
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
        let mut s = Scheduler::new(IDLE);
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
        let mut s = Scheduler::new(IDLE);
        s.add(1, 0);
        s.pick_next(0);
        s.add_cpu(100);
        assert_eq!(s.take_pokes(), 0);
        s
    }

    #[test]
    fn a_thread_made_ready_on_a_busy_cpu_is_offered_to_an_idling_one() {
        let mut s = one_busy_one_idle();
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
        let mut s = one_busy_one_idle();
        s.add(2, 1);
        assert_eq!(s.take_pokes(), 0);
        assert!(s.take_reschedule(1));
    }

    #[test]
    fn nobody_is_told_when_every_cpu_is_busy() {
        let mut s = one_busy_one_idle();
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
        let mut s = one_busy_one_idle();
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
        let mut s = Scheduler::new(IDLE);
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
        let mut s = one_busy_one_idle();
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
        let mut s = Scheduler::new(IDLE);
        for cpu in 1..CPUS {
            assert_eq!(s.add_cpu(idle_of(cpu)), cpu);
        }
        // After each thing CPU `by` does: whoever is to be interrupted
        // for it idles, was offered a thread, and is not `by` itself.
        let interrupted = |s: &mut Scheduler, by: usize| {
            let pokes = s.take_pokes();
            for other in 0..CPUS {
                if pokes & (1 << other) != 0 {
                    assert_eq!(s.current(other), idle_of(other), "busy CPU {other} was interrupted");
                    assert!(s.cpus[other].reschedule, "CPU {other} interrupted without an offer");
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
            poked += interrupted(&mut s, 0);
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
            poked += interrupted(&mut s, cpu);
            if random() % 3 != 0 {
                s.pick_next(cpu);
            }
            poked += interrupted(&mut s, cpu);

            // No thread waits while a CPU idles unasked: if the queue
            // has threads, every idling CPU has been offered one - or
            // there are at least as many offers as threads.
            let offered = (0..CPUS).filter(|&c| s.current(c) == idle_of(c) && s.cpus[c].reschedule).count();
            let idling = (0..CPUS).filter(|&c| s.current(c) == idle_of(c)).count();
            assert!(
                offered >= s.ready.len().min(idling),
                "step {step}: {} threads wait, {idling} CPUs idle, only {offered} were offered one",
                s.ready.len()
            );

            for a in 0..CPUS {
                let current = s.current(a);
                for b in a + 1..CPUS {
                    assert_ne!(current, s.current(b), "one thread on CPUs {a} and {b}");
                }
                assert!(!s.ready.contains(&current), "thread {current} is on CPU {a} and in the queue");
                assert!(!s.ready.contains(&idle_of(a)), "an idle thread is queued");
                if s.is_idle(current) {
                    assert_eq!(current, idle_of(a), "CPU {a} runs another CPU's idle thread");
                }
                assert_ne!(s.state(current), Some(State::Ready), "thread {current} is on CPU {a} and Ready");
                *ran.entry(current).or_insert(0u32) += 1;
                if !s.is_idle(current) && last_cpu.insert(current, a).is_some_and(|before| before != a) {
                    moved += 1;
                }
            }
            for (n, id) in s.ready.iter().enumerate() {
                assert!(!s.ready.iter().skip(n + 1).any(|other| other == id), "thread {id} queued twice");
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
        let mut s = Scheduler::new(IDLE);
        s.add_with(1, 0, DEFAULT, ANY_CPU);
        s.add_with(2, 0, 5, ANY_CPU);
        s.add_with(3, 0, -5, ANY_CPU);
        assert_eq!(s.ready.iter().copied().collect::<Vec<_>>(), [3, 1, 2], "the queue's order");
        assert_eq!(next(&mut s), 3);
    }

    #[test]
    fn a_less_important_thread_never_takes_the_cpu_from_a_more_important_one() {
        let mut s = Scheduler::new(IDLE);
        s.add_with(1, 0, -5, ANY_CPU);
        s.add_with(2, 0, DEFAULT, ANY_CPU);
        assert_eq!(next(&mut s), 1);
        for _ in 0..10 * SLICE {
            s.tick_slice(0);
            assert!(!s.take_reschedule(0), "a slice ended for a less important thread");
        }
        assert_eq!(next(&mut s), 1, "a yield gave the CPU to a less important thread");
        // Equals still take turns: round-robin within a priority.
        s.add_with(3, 0, -5, ANY_CPU);
        assert_eq!(next(&mut s), 3);
        assert_eq!(next(&mut s), 1);
    }

    #[test]
    fn a_woken_important_thread_takes_this_cpu_at_once() {
        let mut s = Scheduler::new(IDLE);
        s.add_with(1, 0, DEFAULT, ANY_CPU);
        s.add_with(2, 0, -5, ANY_CPU);
        assert_eq!(next(&mut s), 2);
        s.suspend_current(0, State::Blocked);
        assert_eq!(next(&mut s), 1);
        assert!(!s.take_reschedule(0));
        s.wake(2, 0);
        assert!(s.take_reschedule(0), "the woken thread waits out a slice behind a less important one");
        assert_eq!(s.take_pokes(), 0, "nobody else needs telling");
        assert_eq!(next(&mut s), 2);
        assert_eq!(s.ready.iter().copied().collect::<Vec<_>>(), [1]);
    }

    #[test]
    fn a_woken_important_thread_takes_another_cpu_with_an_interrupt() {
        let mut s = two_cpus();
        s.add_with(1, 0, DEFAULT, ANY_CPU);
        s.add_with(2, 0, 3, ANY_CPU);
        s.add_with(9, 0, -5, ANY_CPU);
        s.take_pokes();
        assert_eq!(next(&mut s), 9);
        s.pick_next(1);
        assert_eq!(s.current(1), 1);
        s.suspend_current(0, State::Blocked);
        assert_eq!(next(&mut s), 2);
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
        let mut s = two_cpus();
        s.add_with(1, 0, DEFAULT, 1 << 1);
        assert_eq!(s.take_pokes(), 1 << 1, "the idling CPU it may run on was not told");
        assert!(!s.take_reschedule(0), "a CPU it may not run on was offered it");
        assert_eq!(next(&mut s), IDLE, "CPU 0 took a thread pinned to CPU 1");
        s.pick_next(1);
        assert_eq!(s.current(1), 1);
        assert_eq!(s.affinity(1), Some(1 << 1));
    }

    #[test]
    fn a_running_thread_leaves_a_cpu_it_may_no_longer_use() {
        let mut s = two_cpus();
        s.add_with(1, 0, DEFAULT, ANY_CPU);
        s.take_pokes();
        assert_eq!(next(&mut s), 1);
        assert!(s.set_affinity(1, 1 << 1, 0));
        assert!(s.take_reschedule(0), "the thread stayed on a CPU it may not use");
        assert_eq!(next(&mut s), IDLE);
        assert_eq!(s.take_pokes(), 1 << 1, "CPU 1 was not told of it");
        s.pick_next(1);
        assert_eq!(s.current(1), 1);
    }

    #[test]
    fn a_new_priority_moves_a_queued_thread_and_can_end_a_running_ones_turn() {
        let mut s = Scheduler::new(IDLE);
        s.add_with(1, 0, DEFAULT, ANY_CPU);
        s.add_with(2, 0, DEFAULT, ANY_CPU);
        s.add_with(3, 0, DEFAULT, ANY_CPU);
        assert_eq!(next(&mut s), 1);
        assert!(s.set_priority(3, -1, 0));
        assert_eq!(s.ready.iter().copied().collect::<Vec<_>>(), [3, 2], "the queued thread did not move up");
        assert!(s.take_reschedule(0), "a more important thread waits behind a running one");
        assert_eq!(next(&mut s), 3);
        // 3 now lowers itself below the others: it gives way.
        assert!(s.set_priority(3, 1, 0));
        assert!(s.take_reschedule(0));
        assert_eq!(next(&mut s), 2);
        assert!(!s.set_priority(IDLE, -5, 0), "an idle thread has no priority to set");
        assert!(!s.set_priority(77, -5, 0), "a thread that is not there has no priority to set");
    }

    #[test]
    fn randomized_with_priorities_and_affinity() {
        const CPUS: usize = 4;
        let idle_of = |cpu: usize| if cpu == 0 { IDLE } else { 1000 + cpu as u64 };
        let mut s = Scheduler::new(IDLE);
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
        let check_pokes = |s: &mut Scheduler, by: usize, step: u32| {
            let pokes = s.take_pokes();
            for other in 0..CPUS {
                if pokes & (1 << other) != 0 {
                    assert!(s.cpus[other].reschedule, "step {step}: CPU {other} interrupted for nothing");
                    assert_ne!(other, by, "step {step}: CPU {other} told itself");
                }
            }
        };
        check_pokes(&mut s, 0, 0);
        let mut now = 0;
        for step in 0..40_000u32 {
            let cpu = (random() % CPUS as u64) as usize;
            if step as usize % CPUS == 0 {
                now += 1;
                s.tick(now, 0);
                check_pokes(&mut s, 0, step);
            } else {
                s.tick_slice(cpu);
                check_pokes(&mut s, cpu, step);
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
            check_pokes(&mut s, cpu, step);
            if random() % 3 != 0 {
                s.pick_next(cpu);
                check_pokes(&mut s, cpu, step);
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
                assert!(!s.ready.contains(&current), "thread {current} is on CPU {a} and in the queue");
            }
            for (n, id) in s.ready.iter().enumerate() {
                assert!(!s.ready.iter().skip(n + 1).any(|other| other == id), "thread {id} queued twice");
                assert_eq!(s.state(*id), Some(State::Ready));
            }
            sleepers_match(&s);
        }
    }
}
