//! Futexes: a thread sleeps on an address of its process's memory until
//! another thread of the process wakes that address (FUTEX_WAIT and
//! FUTEX_WAKE). The C library builds its mutexes, condition variables
//! and `pthread_join` on these two calls, and does everything else in
//! user mode with atomic instructions: a lock nobody else wants is
//! taken and given back without the kernel at all.
//!
//! The kernel keeps no object per address. A waiter is a record in one
//! list - process, address, thread, and a flag a wake sets - and sleeps
//! in one of a few wait queues, chosen by its address. A wake picks the
//! first `count` records for its address, sets their flags, and wakes
//! that queue: everyone in it looks at their own flag, and those whose
//! flag is still clear (other addresses in the same queue) go back to
//! sleep.
//!
//! The one thing that must hold: a wake that comes after the value
//! changed is never missed. A waiter reads the value and joins the list
//! as one step, under the list's lock; a waker changes the value
//! (in user mode) first and then takes the same lock to wake. So either
//! the waiter read the new value and did not wait, or it was in the
//! list before the waker looked.

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, Ordering};

use abi::Error;
use kernel64::sched::ThreadId;

use crate::irqlock::IrqLock;
use crate::process;
use crate::threads::{self, WaitQueue, Waited};

struct Waiter {
    pid: u64,
    address: u64,
    /// Not needed to wake (the queue does that); here so a dump of the
    /// list can say who waits.
    #[allow(dead_code)]
    thread: ThreadId,
    /// Set by the wake that chose this waiter: it is no longer in the
    /// list, and should return 0.
    woken: AtomicBool,
}

/// Everyone waiting, in the order they began to.
static WAITERS: IrqLock<VecDeque<Arc<Waiter>>> = IrqLock::new(VecDeque::new());

/// Where they sleep: a waiter in the queue its address picks.
const QUEUES: usize = 64;
static SLEEPING: [WaitQueue; QUEUES] = [const { WaitQueue::new() }; QUEUES];

fn queue(pid: u64, address: u64) -> &'static WaitQueue {
    // Futex words are at least 4 bytes apart; neighbouring words of one
    // process land in different queues.
    &SLEEPING[((address >> 2) ^ pid.wrapping_mul(0x9E37_79B9)) as usize % QUEUES]
}

/// FUTEX_WAIT: sleeps while the u32 at `address` holds `expected`.
pub fn wait(address: u64, expected: u64, deadline: u64) -> Result<u64, Error> {
    let expected = u32::try_from(expected).map_err(|_| Error::InvalidArgument)?;
    let pid = process::current().ok_or(Error::InvalidArgument)?.pid;
    let waiter = Arc::new(Waiter { pid, address, thread: threads::current(), woken: AtomicBool::new(false) });
    {
        let mut waiters = WAITERS.lock();
        if process::read_user_word(address)? != expected {
            return Err(Error::Changed);
        }
        waiters.push_back(Arc::clone(&waiter));
    }

    let deadline = (deadline != 0).then_some(deadline);
    let how = threads::wait_any_until(&[queue(pid, address)], deadline, || !waiter.woken.load(Ordering::Acquire));
    if how == Waited::Done {
        return Ok(0);
    }
    // Timed out or killed: leave the list - unless a wake chose this
    // waiter meanwhile. Then the wake counted it, and it must not be
    // lost: it is reported as one.
    let mut waiters = WAITERS.lock();
    if waiter.woken.load(Ordering::Acquire) {
        return Ok(0);
    }
    waiters.retain(|w| !Arc::ptr_eq(w, &waiter));
    Err(if how == Waited::TimedOut { Error::TimedOut } else { Error::Interrupted })
}

/// FUTEX_WAKE: wakes up to `count` threads waiting at `address` in the
/// current process; how many.
pub fn wake_here(address: u64, count: u64) -> Result<u64, Error> {
    if !address.is_multiple_of(4) {
        return Err(Error::InvalidArgument);
    }
    let pid = process::current().ok_or(Error::InvalidArgument)?.pid;
    Ok(wake(pid, address, count))
}

/// Wakes up to `count` threads of process `pid` waiting at `address`.
pub fn wake(pid: u64, address: u64, count: u64) -> u64 {
    let mut woken = 0;
    {
        let mut waiters = WAITERS.lock();
        waiters.retain(|w| {
            let chosen = woken < count && w.pid == pid && w.address == address;
            if chosen {
                w.woken.store(true, Ordering::Release);
                woken += 1;
            }
            !chosen
        });
    }
    if woken > 0 {
        queue(pid, address).wake_all();
    }
    woken
}
