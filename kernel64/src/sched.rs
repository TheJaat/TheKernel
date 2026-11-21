//! The scheduler's decisions, separate from the mechanism.
//!
//! Which thread runs next is pure bookkeeping: queues and states. How
//! the CPU actually switches from one thread to another - saving
//! registers, changing stacks - is assembly in the kernel binary. Keeping
//! them apart means every scheduling rule here is tested on the host,
//! where a wrong decision is an assertion failure rather than a hang.
//!
//! The policy is round-robin with time slices, on any number of CPUs:
//!   - runnable threads wait in ONE FIFO queue, shared by all CPUs:
//!     whichever CPU needs a thread next takes the one that has waited
//!     longest. Work spreads over the CPUs by itself, and a thread runs
//!     on whichever CPU is free - it does not belong to one.
//!   - each CPU has a running thread, which gets SLICE timer ticks;
//!     when they run out and someone is waiting, it goes to the back of
//!     the queue
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

use alloc::collections::{BTreeMap, VecDeque};
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

/// What the scheduler keeps for each CPU.
struct Cpu {
    /// The thread on this CPU now - until the next `pick_next`, even if
    /// it has already said it will sleep, block or exit.
    current: ThreadId,
    idle: ThreadId,
    slice_left: u64,
    /// Set when this CPU should choose again as soon as possible - a
    /// wakeup while it idles, or the end of a slice.
    reschedule: bool,
}

pub struct Scheduler {
    states: BTreeMap<ThreadId, State>,
    ready: VecDeque<ThreadId>,
    cpus: Vec<Cpu>,
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
        let cpu = Cpu { current: running, idle, slice_left: SLICE, reschedule: false };
        Scheduler { states, ready: VecDeque::new(), cpus: alloc::vec![cpu] }
    }

    /// One more CPU, running its idle thread `idle`. Returns its number:
    /// CPUs are numbered in the order they are added, the first being 0.
    pub fn add_cpu(&mut self, idle: ThreadId) -> usize {
        assert!(self.states.insert(idle, State::Running).is_none(), "thread {idle} added twice");
        // A CPU that arrives when there is already work takes some at
        // its first chance.
        let reschedule = !self.ready.is_empty();
        self.cpus.push(Cpu { current: idle, idle, slice_left: SLICE, reschedule });
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

    /// A new thread, ready to run. `cpu` is the CPU this is called on:
    /// if it is idling, it should take the thread at once.
    pub fn add(&mut self, id: ThreadId, cpu: usize) {
        assert!(self.states.insert(id, State::Ready).is_none(), "thread {id} added twice");
        self.enqueue(id, cpu);
    }

    /// One timer tick at time `now`, on the CPU that keeps the time:
    /// wakes sleepers whose time has come, and counts down the slice of
    /// the thread on `cpu`.
    pub fn tick(&mut self, now: u64, cpu: usize) {
        let due: Vec<ThreadId> = self
            .states
            .iter()
            .filter(|(_, s)| matches!(s, State::Sleeping(until) if *until <= now))
            .map(|(&id, _)| id)
            .collect();
        for id in due {
            self.make_runnable(id, cpu);
        }
        self.tick_slice(cpu);
    }

    /// One timer tick on a CPU that does not keep the time: only the
    /// slice of the thread on `cpu`.
    pub fn tick_slice(&mut self, cpu: usize) {
        let waiting = !self.ready.is_empty();
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

    /// Whether `cpu` should choose again; clears the request.
    pub fn take_reschedule(&mut self, cpu: usize) -> bool {
        core::mem::take(&mut self.cpus[cpu].reschedule)
    }

    /// Into the queue - for a thread that is on no CPU.
    fn enqueue(&mut self, id: ThreadId, cpu: usize) {
        debug_assert!(self.cpu_of(id).is_none(), "thread {id} queued while on a CPU");
        self.states.insert(id, State::Ready);
        self.ready.push_back(id);
        let this = &mut self.cpus[cpu];
        if this.current == this.idle {
            this.reschedule = true;
        }
    }

    /// Ends the sleep or block of `id`. If it has not left its CPU yet
    /// it stays there, running; otherwise it joins the queue.
    fn make_runnable(&mut self, id: ThreadId, cpu: usize) {
        if self.cpu_of(id).is_some() {
            self.states.insert(id, State::Running);
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
        self.states.insert(this.current, state);
    }

    /// Takes back a `suspend_current` into Sleeping or Blocked: the
    /// thread looked again and found it has nothing to wait for. (Not
    /// Exited: that cannot be taken back.)
    pub fn resume_current(&mut self, cpu: usize) {
        let id = self.cpus[cpu].current;
        if let Some(State::Blocked | State::Sleeping(_)) = self.states.get(&id) {
            self.states.insert(id, State::Running);
        }
    }

    /// Chooses who runs next on `cpu`. The current thread, if still
    /// running, goes to the back of the queue (unless it is the idle
    /// thread). Returns the switch to make, or None to keep running the
    /// current thread.
    pub fn pick_next(&mut self, cpu: usize) -> Option<Switch> {
        let from = self.cpus[cpu].current;
        let idle = self.cpus[cpu].idle;
        let still_running = self.states.get(&from) == Some(&State::Running);

        let Some(to) = self.ready.pop_front() else {
            if still_running {
                self.cpus[cpu].slice_left = SLICE;
                return None; // nobody else wants the CPU
            }
            return self.switch_to(cpu, from, idle);
        };
        // The switch comes first: `from` is on no CPU from here on, and
        // only then may it be queued.
        let switch = self.switch_to(cpu, from, to);
        if still_running {
            if from == idle {
                self.states.insert(from, State::Ready); // ready, never queued
            } else {
                self.states.insert(from, State::Ready);
                self.ready.push_back(from);
            }
        }
        switch
    }

    fn switch_to(&mut self, cpu: usize, from: ThreadId, to: ThreadId) -> Option<Switch> {
        self.states.insert(to, State::Running);
        let this = &mut self.cpus[cpu];
        this.current = to;
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
        // Thread 2 is made on CPU 0, which is busy; CPU 1 idles and does
        // not know yet.
        s.add(2, 0);
        assert!(!s.take_reschedule(1));
        s.tick_slice(1);
        assert!(s.take_reschedule(1), "the idle CPU's tick finds the work");
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
    #[test]
    fn randomized_on_four_cpus() {
        const CPUS: usize = 4;
        const THREADS: u64 = 30;
        let idle_of = |cpu: usize| if cpu == 0 { IDLE } else { 1000 + cpu as u64 };
        let mut s = Scheduler::new(IDLE);
        for cpu in 1..CPUS {
            assert_eq!(s.add_cpu(idle_of(cpu)), cpu);
        }
        let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
        let mut random = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for id in 1..=THREADS {
            s.add(id, 0);
        }
        let mut now = 0;
        let mut ran = BTreeMap::new();
        let mut moved = 0u32;
        let mut last_cpu: BTreeMap<ThreadId, usize> = BTreeMap::new();
        for step in 0..40_000 {
            let cpu = (random() % CPUS as u64) as usize;
            if step % CPUS == 0 {
                now += 1;
                s.tick(now, 0);
            } else {
                s.tick_slice(cpu);
            }
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
                    s.wake(1 + random() % THREADS, cpu);
                }
                _ => {}
            }
            // Sometimes the switch does not follow at once: another CPU
            // gets in first, which is the whole difficulty.
            if random() % 3 != 0 {
                s.pick_next(cpu);
            }

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
        }
        for id in 1..=THREADS {
            assert!(ran.get(&id).copied().unwrap_or(0) > 50, "thread {id} starved");
        }
        assert!(moved > 100, "threads did not move between CPUs");
    }
}
