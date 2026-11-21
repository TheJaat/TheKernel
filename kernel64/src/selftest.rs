//! Checks the kernel runs on itself at every boot.
//!
//! The host tests prove the algorithms; these prove the wiring - that
//! the real frame allocator, the real page tables and the real heap work
//! on the real machine. Each one asserts, so a failure is a kernel panic
//! with the exact line, and `make boot64-test` fails on it.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use kernel64::memory::addr::{phys_to_virt, VirtAddr};
use kernel64::paging::{Flags, PageSize};

use crate::arch::{self, mmu};
use crate::irqlock::IrqLock;
use crate::{interrupts, mm, threads};

pub fn run() {
    frames();
    paging();
    protections();
    heap();
    kprintln!("[selftest] memory: all passed");
}

fn frames() {
    mm::with_memory(|_, frames| {
        let before = frames.free_frames();
        let a = frames.allocate().expect("no free frame");
        let b = frames.allocate().expect("no free frame");
        assert_ne!(a, b);
        frames.free(a);
        assert_eq!(frames.allocate(), Some(a), "a freed frame is reused first");
        frames.free(a);
        frames.free(b);
        assert_eq!(frames.free_frames(), before);
    });
    kprintln!("[selftest] frames: allocate, free, reuse");
}

/// An address nothing else uses.
const TEST_PAGE: u64 = 0xFFFF_D000_0000_0000;

fn paging() {
    mm::with_memory(|tables, frames| {
        let virt = VirtAddr::new(TEST_PAGE);
        let frame = frames.allocate().expect("no free frame");
        tables
            .map(virt, frame, PageSize::Size4K, Flags::WRITABLE | mm::no_execute(), frames)
            .expect("mapping the test page failed");

        // Write through the new mapping, read through the direct map:
        // the same physical memory, seen at two virtual addresses.
        let page = virt.as_mut_ptr::<u64>();
        let alias = phys_to_virt(frame).as_ptr::<u64>();
        // SAFETY: both point at the frame just mapped, which nothing
        // else uses.
        unsafe {
            page.write_volatile(0x5441_414A_0000_0001);
            assert_eq!(alias.read_volatile(), 0x5441_414A_0000_0001);
        }
        assert_eq!(tables.translate(virt).map(|t| t.physical), Some(frame));

        assert_eq!(tables.unmap(virt), Some(frame));
        mmu::flush(TEST_PAGE);
        assert_eq!(tables.translate(virt), None);
        frames.free(frame);
    });
    kprintln!("[selftest] paging: map, alias through the direct map, unmap");
}

/// An immutable static, so it lands in .rodata.
static READ_ONLY: u64 = 7;
/// A mutable one, in .data/.bss.
static WRITABLE: AtomicU64 = AtomicU64::new(0);

/// The kernel's own mappings have the permissions linker.ld intends.
fn protections() {
    mm::with_memory(|tables, _| {
        let flags = |address: u64| tables.translate(VirtAddr::new(address)).map(|t| t.flags);
        let nx = mm::no_execute();

        let code = flags(run as *const () as u64).expect("kernel code is unmapped");
        assert!(!code.contains(Flags::WRITABLE) && !code.contains(Flags::NO_EXECUTE), "code is {code:?}");

        let rodata = flags(&raw const READ_ONLY as u64).expect("rodata is unmapped");
        assert!(!rodata.contains(Flags::WRITABLE) && rodata.contains(nx), "rodata is {rodata:?}");

        WRITABLE.fetch_add(1, Ordering::Relaxed);
        let data = flags(&raw const WRITABLE as u64).expect("data is unmapped");
        assert!(data.contains(Flags::WRITABLE | nx), "data is {data:?}");

        assert_eq!(flags(0), None, "the null page must be unmapped");
        assert_eq!(flags(mm::kernel_layout().stack_guard), None, "the stack guard must be unmapped");
    });
    kprintln!("[selftest] protections: code RX, rodata R, data RW+NX, null and stack guard unmapped");
}

fn heap() {
    let (used_before, total_before) = mm::heap_usage();
    {
        let boxed = Box::new(42u64);
        assert_eq!(*boxed, 42);

        // 4 MiB - well past the initial 1 MiB, so the heap has to grow.
        let numbers: Vec<u64> = (0..512 * 1024).collect();
        let sum: u64 = numbers.iter().sum();
        assert_eq!(sum, 512 * 1024 * (512 * 1024 - 1) / 2);

        let mut text = String::from("Hello");
        text.push_str(", heap");
        assert_eq!(format!("{text}!"), "Hello, heap!");

        let (_, total) = mm::heap_usage();
        assert!(total > total_before, "the heap did not grow");
        kprintln!("[selftest] heap: Box, Vec, String; grew from {} KiB to {} KiB", total_before / 1024, total / 1024);
    }
    let (used_after, _) = mm::heap_usage();
    assert_eq!(used_after, used_before, "dropping everything did not free everything");
}

/// The timer interrupt arrives, at the rate it was set to: ten ticks,
/// timed with the TSC (calibrated separately, against the PIT).
pub fn timer() {
    let tsc_hz = arch::platform::tsc_hz();
    let first = interrupts::ticks();
    // Line up with a tick boundary, then count ten.
    while interrupts::ticks() == first {
        arch::wait_for_interrupt();
    }
    let start_ticks = interrupts::ticks();
    let start = arch::tsc();
    while interrupts::ticks() < start_ticks + 10 {
        arch::wait_for_interrupt();
    }
    let ms = (arch::tsc() - start) * 1000 / tsc_hz.max(1);
    let expected = 10 * 1000 / interrupts::HZ;
    // Generous bounds: a VM's clocks can stretch under load.
    assert!(ms >= expected / 2 && ms <= expected * 3, "10 timer ticks took {ms} ms, expected {expected}");
    kprintln!("[selftest] timer: 10 ticks in {ms} ms (expected {expected})");
}

/// Threads: they take turns, a thread that never yields is still
/// interrupted, sleep sleeps, and dead threads give everything back.
pub fn threads() {
    // 1. Taking turns: two threads that yield after every step.
    let log = Arc::new(IrqLock::new(Vec::new()));
    let ids: Vec<_> = (*b"ab")
        .into_iter()
        .map(|tag| {
            let log = Arc::clone(&log);
            threads::spawn("turns", move || {
                for _ in 0..3 {
                    log.lock().push(tag);
                    threads::yield_now();
                }
            })
        })
        .collect();
    for id in ids {
        threads::join(id);
    }
    let log = log.lock().clone();
    assert_eq!(log.iter().filter(|&&b| b == b'a').count(), 3);
    assert_eq!(log.iter().filter(|&&b| b == b'b').count(), 3);
    assert_ne!(log, b"aaabbb", "yield did not hand over the CPU");
    kprintln!("[selftest] threads: two yielding threads ran as {:?}", core::str::from_utf8(&log).unwrap());

    // 2. Preemption: this thread never yields. If the timer did not take
    // the CPU from it, nothing else - including this test - would ever
    // run again; it gives up after two seconds and says so.
    let stop = Arc::new(AtomicBool::new(false));
    let spinner = {
        let stop = Arc::clone(&stop);
        threads::spawn("spinner", move || {
            let deadline = arch::tsc() + 2 * arch::platform::tsc_hz();
            while !stop.load(Ordering::Relaxed) {
                assert!(arch::tsc() < deadline, "the spinning thread was never preempted");
                core::hint::spin_loop();
            }
        })
    };
    threads::sleep(3); // only returns if the spinner is preempted
    stop.store(true, Ordering::Relaxed);
    threads::join(spinner);
    kprintln!("[selftest] threads: a thread that never yields was preempted");

    // 3. Sleeping.
    let start = interrupts::ticks();
    threads::sleep(5);
    let slept = interrupts::ticks() - start;
    assert!((5..=7).contains(&slept), "sleep(5) took {slept} ticks");

    // 4. Churn: 100 threads come and go. Their stacks must be recycled -
    // no growth in slots or frames after the first round.
    //
    // All 100 wait at a gate until every one exists. Otherwise the peak
    // number alive would depend on timing - the timer can preempt the
    // spawning loop and let early threads finish - and a second round
    // that happened to peak higher would reach a stack slot never used
    // before, whose page table stays: one frame that looks like a leak.
    let churn = || {
        let gate = Arc::new(AtomicBool::new(false));
        let ids: Vec<_> = (0..100)
            .map(|n| {
                let gate = Arc::clone(&gate);
                threads::spawn("churn", move || {
                    while !gate.load(Ordering::Acquire) {
                        threads::yield_now();
                    }
                    let v: Vec<u64> = (0..n).collect();
                    assert_eq!(v.len() as u64, n);
                })
            })
            .collect();
        gate.store(true, Ordering::Release);
        for id in ids {
            threads::join(id);
        }
        threads::yield_now(); // let the last exited thread be reaped
    };
    churn();
    let frames = mm::unused_frames();
    let slots = mm::stack_slots_used();
    churn();
    assert_eq!(mm::unused_frames(), frames, "thread churn leaked frames");
    assert_eq!(mm::stack_slots_used(), slots, "thread churn leaked stack slots");
    assert_eq!(threads::count(), 2, "only boot and idle should remain");
    kprintln!("[selftest] threads: sleep(5) took {slept} ticks; 200 threads spawned and reaped, nothing leaked");
}

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

    let per_microsecond = arch::platform::tsc_hz() / 1_000_000;
    let spin_until = |tsc: u64| {
        while arch::tsc() < tsc {
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
        spin_until(arch::tsc() + (round as u64 * 37 % 10) * 1000 * per_microsecond);
        let start = arch::tsc();
        shared.asked.store(round, Ordering::Release);
        shared.queue.wake_all();
        // Spinning, not sleeping: this CPU must stay busy, so that the
        // sleeper can only run on another.
        while shared.answered.load(Ordering::Acquire) < round {
            core::hint::spin_loop();
        }
        times.push((arch::tsc() - start) / per_microsecond.max(1));
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

/// User mode: four programs, run at once. Two follow the rules and must
/// finish with their exit codes; two break them and must be killed -
/// without the kernel noticing anything but a log line. Then everything
/// they used must be back.
pub fn user() {
    use crate::{process, programs};

    let run_all = || {
        let programs: [(&'static str, &[u8]); 8] = [
            ("hello", programs::HELLO),
            ("peek", programs::PEEK),
            ("cli", programs::CLI),
            ("port", programs::PORT),
            ("memro", programs::MEMRO),
            ("spin", programs::SPIN),
            ("heapcheck", programs::HEAPCHECK),
            ("gs", programs::GS),
        ];
        let started: Vec<_> = programs
            .iter()
            .map(|&(name, file)| (name, process::spawn(name, file, Vec::new()).unwrap_or_else(|e| panic!("loading {name}: {e:?}"))))
            .collect();
        started
            .into_iter()
            .map(|(name, (process, thread))| {
                threads::join(thread);
                (name, process.exit_code())
            })
            .collect::<Vec<_>>()
    };

    let results = run_all();
    let expected = [
        ("hello", Some(42)),
        ("peek", Some(abi::KILLED)),
        ("cli", Some(abi::KILLED)),
        ("port", Some(abi::KILLED)),
        ("memro", Some(abi::KILLED)),
        ("spin", Some(0)),
        ("heapcheck", Some(0)),
        ("gs", Some(abi::KILLED)),
    ];
    assert_eq!(results, expected, "user programs did not end as expected");

    // A second round must leave memory exactly as the first did: every
    // page of every process - code, data, stack, page tables - returned.
    threads::yield_now(); // let the last exited thread be reaped
    let frames = mm::unused_frames();
    assert_eq!(run_all(), expected);
    threads::yield_now();
    assert_eq!(mm::unused_frames(), frames, "processes leaked frames");
    // Files that are not programs - which the disk may now hold - are
    // refused, and cost nothing: text, and a program cut in half.
    assert!(process::load("text", &[b'x'; 200]).is_err());
    assert!(process::load("half", &programs::HELLO[..programs::HELLO.len() / 2]).is_err());
    assert_eq!(mm::unused_frames(), frames, "refused programs leaked frames");
    kprintln!("[selftest] user: hello exited 42, spin and heapcheck exited 0, peek, cli, port, memro and gs were killed; 16 processes, nothing leaked");
}

/// IPC: a service and its client, connected by one channel the kernel
/// made. The client tests every rule of handles and channels, then asks
/// the service to crash and must survive it. Twice - and then every
/// channel and every frame must be gone.
pub fn ipc() {
    use crate::{ipc, process, programs};
    use kernel64::cap::Rights;

    let run = || {
        let (service_end, client_end) = ipc::channel();
        let (service, service_thread) =
            process::spawn("echod", programs::ECHOD, alloc::vec![(service_end, Rights::ALL)]).expect("loading echod");
        let (client, client_thread) =
            process::spawn("echoc", programs::ECHOC, alloc::vec![(client_end, Rights::ALL)]).expect("loading echoc");
        threads::join(client_thread);
        threads::join(service_thread);
        assert_eq!(client.exit_code(), Some(0), "echoc failed (the code is the line of the failed check)");
        assert_eq!(service.exit_code(), Some(abi::KILLED), "echod should have crashed");
    };

    run();
    threads::yield_now();
    let frames = mm::unused_frames();
    run();
    threads::yield_now();
    assert_eq!(mm::unused_frames(), frames, "IPC leaked frames");
    assert_eq!(ipc::live_channels(), 0, "channels leaked");
    kprintln!("[selftest] ipc: echo service and client - rights, transfer, backpressure, crash isolation; 0 channels left");
}

/// Files: the file service and a client that tests it - long files,
/// listings, refusals, careless requests. When the client is done it
/// closes its end, and the service must end cleanly. Twice, and then
/// nothing may be left: no channel (each request made one for its
/// reply), no frame (the service's heap held every file).
pub fn files() {
    use crate::{ipc, process, programs};
    use kernel64::cap::Rights;

    let run = || {
        let (service_end, client_end) = ipc::channel();
        // The second channel: requests on it may only read.
        let (service_look, client_look) = ipc::channel();
        let (service, service_thread) =
            process::spawn("fs", programs::FS, alloc::vec![(service_end, Rights::ALL), (service_look, Rights::ALL)])
                .expect("loading fs");
        let (client, client_thread) =
            process::spawn("fscheck", programs::FSCHECK, alloc::vec![(client_end, Rights::ALL), (client_look, Rights::ALL)])
                .expect("loading fscheck");
        threads::join(client_thread);
        threads::join(service_thread);
        assert_eq!(client.exit_code(), Some(0), "fscheck failed (the code is the line of the failed check)");
        assert_eq!(service.exit_code(), Some(0), "fs should end when its last client goes");
    };

    run();
    threads::yield_now();
    let frames = mm::unused_frames();
    run();
    threads::yield_now();
    assert_eq!(mm::unused_frames(), frames, "the file service leaked frames");
    assert_eq!(ipc::live_channels(), 0, "channels leaked");
    kprintln!("[selftest] files: fs and fscheck, twice - every check passed, 0 channels and no frames left");
}

/// Kill: a program that never ends, ended from outside - in each state a
/// thread can be in when the kill arrives. Computing in user mode (only
/// the timer interrupt gets it back into the kernel), making system
/// calls in a loop, asleep, waiting for a message that will never come,
/// and not yet run at all. Each must end with STOPPED, promptly; and
/// then nothing may be left: no frame, no channel (the waiting one made
/// a channel of its own, which must close with it).
pub fn kill() {
    use crate::{ipc, process, programs};
    use kernel64::cap::Rights;

    // Starts `forever MODE`: its arguments arrive as a program's do, in
    // a channel with one message.
    let start = |mode: &str| {
        let (tx, rx) = ipc::channel();
        ipc::kernel_send(&tx, mode.as_bytes(), Vec::new()).expect("sending the arguments");
        drop(tx);
        let loaded = process::load("forever", programs::FOREVER).expect("loading forever");
        let thread = process::start(&loaded, alloc::vec![(rx, Rights::ALL)], abi::startup::ARGUMENTS);
        (Arc::clone(&loaded), thread)
    };
    let run = || {
        for mode in ["spin", "yield", "sleep", "wait", "unborn"] {
            let (process, thread) = start(mode);
            if mode != "unborn" {
                threads::sleep(3); // long enough to get where it is going
            }
            assert_eq!(process.exit_code(), None, "forever {mode} ended by itself");
            let asked = interrupts::ticks();
            process::kill(&process);
            process::kill(&process); // asking twice is asking once
            threads::join(thread);
            let took = interrupts::ticks() - asked;
            assert_eq!(process.exit_code(), Some(abi::STOPPED), "forever {mode} after the kill");
            assert!(took <= 3, "forever {mode} took {took} ticks to end");
            process::kill(&process); // already ended: nothing happens
            assert_eq!(process.exit_code(), Some(abi::STOPPED));
        }
    };

    run();
    threads::yield_now(); // let the last exited thread be reaped
    let frames = mm::unused_frames();
    run();
    threads::yield_now();
    assert_eq!(mm::unused_frames(), frames, "killed processes leaked frames");
    assert_eq!(ipc::live_channels(), 0, "killed processes leaked channels");
    kprintln!("[selftest] kill: spinning, yielding, sleeping, waiting and not yet started - each stopped within 3 ticks; nothing leaked");
}
