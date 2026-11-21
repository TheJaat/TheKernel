//! The checks of several CPUs at once: threads on all of them, and
//! wake-ups from one to another. Only where the kernel starts more
//! than one, which so far is the PC.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arch;
use crate::{interrupts, mm, threads};

/// Several CPUs: threads really run on all of them, a wake-up sent from
/// one CPU to a thread going to sleep on another is never lost, and
/// threads that end on any CPU give everything back.
pub fn smp() {
    use crate::threads::WaitQueue;
    use core::sync::atomic::AtomicUsize;

    let cpus = threads::cpus();
    let work = || {
        // 1. One thread for each CPU, none of which ever yields. Each
        // notes which CPUs it finds itself on. One CPU could run them
        // all in turn; but then only one CPU would ever be noted.
        let seen = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let spinners: Vec<_> = (0..cpus)
            .map(|_| {
                let (seen, stop) = (Arc::clone(&seen), Arc::clone(&stop));
                threads::spawn("spinner", move || {
                    while !stop.load(Ordering::Relaxed) {
                        seen.fetch_or(1 << arch::cpu(), Ordering::Relaxed);
                        core::hint::spin_loop();
                    }
                })
            })
            .collect();
        let everyone = (1u64 << cpus) - 1;
        let start = interrupts::ticks();
        while seen.load(Ordering::Relaxed) != everyone && interrupts::ticks() < start + 200 {
            threads::sleep(1);
        }
        stop.store(true, Ordering::Relaxed);
        for id in spinners {
            threads::join(id);
        }
        assert_eq!(seen.load(Ordering::Relaxed), everyone, "threads did not run on every CPU");

        // 2. Two threads hand a token back and forth. Each waits for its
        // turn - sometimes spinning a little first, so that it is still
        // deciding to sleep when the other wakes it - takes it, and
        // wakes the other. One lost wake-up and both sleep for ever.
        const HANDOFFS: usize = 20_000;
        struct Token {
            turn: AtomicUsize,
            passed: AtomicUsize,
            queue: WaitQueue,
        }
        let token = Arc::new(Token { turn: AtomicUsize::new(0), passed: AtomicUsize::new(0), queue: WaitQueue::new() });
        let players: Vec<_> = (0..2usize)
            .map(|me| {
                let token = Arc::clone(&token);
                threads::spawn("handoff", move || {
                    for round in 0..HANDOFFS / 2 {
                        for _ in 0..round % 97 {
                            if token.turn.load(Ordering::Acquire) == me {
                                break;
                            }
                            core::hint::spin_loop();
                        }
                        let mine = token.queue.wait_while(|| token.turn.load(Ordering::Acquire) != me);
                        assert!(mine, "a kernel thread's wait was cut short");
                        token.passed.fetch_add(1, Ordering::Relaxed);
                        // The change first, the wake-up after.
                        token.turn.store(1 - me, Ordering::Release);
                        token.queue.wake_all();
                    }
                })
            })
            .collect();
        for id in players {
            threads::join(id);
        }
        assert_eq!(token.passed.load(Ordering::Relaxed), HANDOFFS);
    };

    work();
    let frames = mm::unused_frames();
    work();
    assert_eq!(mm::unused_frames(), frames, "threads on several CPUs leaked frames");
    if cpus == 1 {
        kprintln!("[selftest] smp: 1 CPU; 40000 hand-offs between two threads, none lost; nothing leaked");
    } else {
        kprintln!("[selftest] smp: threads ran on all {cpus} CPUs at once; 40000 hand-offs between two threads, none lost; nothing leaked");
    }
}

/// A thread made ready by a busy CPU starts on an idling one at once,
/// not at that CPU's next timer tick: the time from the wake-up to the
/// woken thread's answer, measured many times.
pub fn wakeups() {
    use crate::threads::WaitQueue;
    use core::sync::atomic::AtomicUsize;

    if threads::cpus() == 1 {
        kprintln!("[selftest] wake-ups: 1 CPU, so nobody to tell");
        return;
    }
    const ROUNDS: usize = 40;
    struct Shared {
        asked: AtomicUsize,
        answered: AtomicUsize,
        queue: WaitQueue,
    }
    let shared = Arc::new(Shared { asked: AtomicUsize::new(0), answered: AtomicUsize::new(0), queue: WaitQueue::new() });
    let sleeper = {
        let shared = Arc::clone(&shared);
        threads::spawn("sleeper", move || {
            for round in 1..=ROUNDS {
                let asked = shared.queue.wait_while(|| shared.asked.load(Ordering::Acquire) < round);
                assert!(asked, "a kernel thread's wait was cut short");
                shared.answered.store(round, Ordering::Release);
            }
        })
    };

    let per_microsecond = arch::clock_hz() / 1_000_000;
    let spin_until = |tsc: u64| {
        while arch::clock() < tsc {
            core::hint::spin_loop();
        }
    };
    let pokes = interrupts::pokes();
    let mut times: Vec<u64> = Vec::with_capacity(ROUNDS);
    for round in 1..=ROUNDS {
        // Long enough for the sleeper to be asleep and off its CPU.
        threads::sleep(2);
        // The sleep ends on a tick. Every CPU's timer keeps its own
        // fixed distance from that tick, so without this wait - a
        // different part of the 10 ms each round - one lucky distance
        // would make every round look quick.
        spin_until(arch::clock() + (round as u64 * 37 % 10) * 1000 * per_microsecond);
        let start = arch::clock();
        shared.asked.store(round, Ordering::Release);
        shared.queue.wake_all();
        // Spinning, not sleeping: this CPU must stay busy, so that the
        // sleeper can only run on another.
        while shared.answered.load(Ordering::Acquire) < round {
            core::hint::spin_loop();
        }
        times.push((arch::clock() - start) / per_microsecond.max(1));
    }
    threads::join(sleeper);

    times.sort_unstable();
    let (median, slowest) = (times[ROUNDS / 2], times[ROUNDS - 1]);
    let told = interrupts::pokes() - pokes;
    // What is required is that the interrupts were sent and taken,
    // about one for each wake-up. The time is reported and not judged:
    // a tick is 10 000 microseconds and the median here is usually a
    // few hundred, but on an emulator sharing a busy host it has been
    // three thousand, and no limit tells that from a lucky run of ticks.
    assert!(told >= ROUNDS as u64 / 2, "only {told} CPUs were interrupted in {ROUNDS} wake-ups");
    kprintln!(
        "[selftest] wake-ups: a thread woken by a busy CPU ran on an idle one {median} us later (median of {ROUNDS}; slowest {slowest} us; {told} interrupts between CPUs)"
    );
}
