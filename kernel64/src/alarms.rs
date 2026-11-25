//! Alarms: SIGALRM to a process when a time it chose comes (the ALARM
//! call, C's `alarm`).
//!
//! Each process has at most one alarm. They are kept in one list, and
//! one kernel thread, "alarms", rings them: it sleeps until the earliest
//! is due, sends the signals, and sleeps again. Setting an alarm that is
//! earlier than every other wakes it to look again.
//!
//! Why a thread, and not the timer interrupt: sending a signal can wake
//! threads and end a process, and wants locks the interrupt may already
//! be inside (the scheduler's); and an alarm only needs to be as exact
//! as a tick. The thread starts with the first alarm, so a system whose
//! programs set none never has it.

use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use abi::Error;

use crate::irqlock::IrqLock;
use crate::process::{self, Process};
use crate::threads::{self, WaitQueue};
use crate::interrupts;

struct Alarm {
    /// The tick it rings at.
    at: u64,
    process: Weak<Process>,
}

static ALARMS: IrqLock<Vec<Alarm>> = IrqLock::new(Vec::new());
/// Moved on whenever the list changes, and the thread woken: it waits
/// for the number to change or the earliest alarm to come.
static CHANGES: AtomicU64 = AtomicU64::new(0);
static CHANGED: WaitQueue = WaitQueue::new();
static STARTED: AtomicBool = AtomicBool::new(false);

const NANOS_PER_TICK: u64 = 1_000_000_000 / interrupts::HZ;

/// The ALARM call: SIGALRM to this process `nanoseconds` from now
/// (rounded up to a tick), in place of any alarm it had; 0 only cancels.
/// Returns the nanoseconds the one before had left, or 0.
pub fn set(nanoseconds: u64) -> Result<u64, Error> {
    if nanoseconds > abi::ALARM_MAX {
        return Err(Error::InvalidArgument);
    }
    let me = process::current().ok_or(Error::InvalidArgument)?;
    let now = interrupts::ticks();
    let left = {
        let mut alarms = ALARMS.lock();
        let mine = alarms.iter().position(|alarm| core::ptr::eq(alarm.process.as_ptr(), Arc::as_ptr(&me)));
        let left = mine.map_or(0, |n| alarms.swap_remove(n).at.saturating_sub(now) * NANOS_PER_TICK);
        if nanoseconds > 0 {
            alarms.push(Alarm { at: now + nanoseconds.div_ceil(NANOS_PER_TICK), process: Arc::downgrade(&me) });
        }
        // Gone processes' alarms go too - on any change, so the list
        // never holds more than the processes that set one since.
        alarms.retain(|alarm| alarm.process.strong_count() > 0);
        left
    };
    CHANGES.fetch_add(1, Ordering::AcqRel);
    if !STARTED.swap(true, Ordering::AcqRel) {
        threads::spawn("alarms", ring);
    }
    CHANGED.wake_all();
    Ok(left)
}

/// The "alarms" thread.
fn ring() {
    loop {
        let seen = CHANGES.load(Ordering::Acquire);
        let now = interrupts::ticks();
        let (due, next): (Vec<Arc<Process>>, Option<u64>) = {
            let mut alarms = ALARMS.lock();
            let mut due = Vec::new();
            alarms.retain(|alarm| {
                if alarm.at > now {
                    return true;
                }
                due.extend(alarm.process.upgrade());
                false
            });
            (due, alarms.iter().map(|alarm| alarm.at).min())
        };
        for process in due {
            process::send_signal(&process, abi::signal::SIGALRM);
        }
        // Until the next is due, or the list changes. Rule 2 of
        // threads.rs: a change after `seen` was read is seen here.
        let _ = threads::wait_any_until(&[&CHANGED], next, || CHANGES.load(Ordering::Acquire) == seen);
    }
}
