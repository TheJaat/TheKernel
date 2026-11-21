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

use kernel64::memory::addr::{phys_to_virt, PhysAddr, VirtAddr};
use kernel64::paging::{Flags, PageSize};

use crate::arch::{self, mmu};
use crate::irqlock::IrqLock;
use crate::{interrupts, mm, threads};

// What AArch64 does not have yet: other CPUs.
#[cfg(target_arch = "x86_64")]
mod system;
mod user;
#[cfg(target_arch = "x86_64")]
pub use system::{smp, wakeups};
pub use user::{files, ipc, kill, user};
#[cfg(target_arch = "aarch64")]
pub use user::disk;
#[cfg(feature = "c-programs")]
pub use user::c;

pub fn run() {
    frames();
    top_of_memory();
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

        // The kernel's own memory must never be handed out: first
        // page, last page, and the page under the boot stack (not
        // mapped, but still the image's).
        let layout = mm::kernel_layout();
        for address in [layout.text.start, layout.rodata.start, layout.data.end - 1, layout.stack_guard] {
            let physical = PhysAddr::new(mmu::kernel_physical(address));
            assert!(!frames.is_free(physical), "the kernel's page at {address:#x} is free memory");
        }
    });
    kprintln!("[selftest] frames: allocate, free, reuse; the kernel's own are not free");
}

/// The frame allocator hands out the lowest frame first, so nothing
/// else at boot goes near the top of memory - where a memory map read
/// wrongly, or a direct map that stops short, would show. Take the
/// highest free frame and use it.
fn top_of_memory() {
    let frame = mm::with_frames(|frames| frames.allocate_highest()).expect("no free frame");
    let pointer = phys_to_virt(frame).as_mut_ptr::<u64>();
    // SAFETY: the frame was allocated a moment ago, so nothing else
    // uses it; that the direct map reaches it is what is being checked
    // (if not, this is a page fault with the address in it).
    unsafe {
        for word in 0..512 {
            pointer.add(word).write_volatile(0x7461_616A_0000_0000 | word as u64);
        }
        for word in 0..512 {
            assert_eq!(pointer.add(word).read_volatile(), 0x7461_616A_0000_0000 | word as u64, "word {word} of the top frame");
        }
    }
    mm::with_frames(|frames| frames.free(frame));
    kprintln!("[selftest] top of memory: the highest free frame, at {:#x}, written and read back", frame.as_u64());
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
        mmu::commit_mappings();

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
/// timed with a clock that does not depend on it (on a PC the TSC,
/// calibrated separately against the PIT; on AArch64 the CPU's
/// counter).
pub fn timer() {
    let tsc_hz = arch::clock_hz();
    let first = interrupts::ticks();
    // Line up with a tick boundary, then count ten.
    while interrupts::ticks() == first {
        arch::wait_for_interrupt();
    }
    let start_ticks = interrupts::ticks();
    let start = arch::clock();
    while interrupts::ticks() < start_ticks + 10 {
        arch::wait_for_interrupt();
    }
    let ms = (arch::clock() - start) * 1000 / tsc_hz.max(1);
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
            let deadline = arch::clock() + 2 * arch::clock_hz();
            while !stop.load(Ordering::Relaxed) {
                assert!(arch::clock() < deadline, "the spinning thread was never preempted");
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
