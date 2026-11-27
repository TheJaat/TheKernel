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
//! as the timer: since K24 (journal entry 128) the thread sleeps to the
//! nanosecond, as alarms are kept, so an alarm rings as soon as a timer
//! interrupt can say so - not at the next tick. The thread starts at
//! boot (`start`), and waits blocked until an alarm is set.

use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use abi::Error;

use crate::irqlock::IrqLock;
use crate::process::{self, Process};
use crate::threads::{self, WaitQueue};

struct Alarm {
    /// When it rings: nanoseconds of the clock (`threads::now_ns`).
    at: u64,
    /// Then again every this many nanoseconds; 0: once (setitimer's
    /// it_interval).
    interval: u64,
    process: Weak<Process>,
}

static ALARMS: IrqLock<Vec<Alarm>> = IrqLock::new(Vec::new());
/// Moved on whenever the list changes, and the thread woken: it waits
/// for the number to change or the earliest alarm to come.
static CHANGES: AtomicU64 = AtomicU64::new(0);
static CHANGED: WaitQueue = WaitQueue::new();
static STARTED: AtomicBool = AtomicBool::new(false);


/// The ALARM call: SIGALRM to this process `nanoseconds` from now, in
/// place of any alarm it had, and then every `interval` if that is not
/// 0; 0 only cancels. With `keep` 1, nothing changes. Returns the
/// nanoseconds the one before had left, or 0.
pub fn set(nanoseconds: u64, interval: u64, keep: u64) -> Result<u64, Error> {
    if nanoseconds > abi::ALARM_MAX || interval > abi::ALARM_MAX || keep > 1 {
        return Err(Error::InvalidArgument);
    }
    let me = process::current().ok_or(Error::InvalidArgument)?;
    let now = threads::now_ns();
    let left = {
        let mut alarms = ALARMS.lock();
        let mine = alarms.iter().position(|alarm| core::ptr::eq(alarm.process.as_ptr(), Arc::as_ptr(&me)));
        // At least 1 while it is still to ring: due, but not yet rung by
        // the thread, is not "no alarm" - setitimer's old value would say
        // no interval (found by clib compiled by tcc, journal entry 128).
        let left = mine.map_or(0, |n| alarms[n].at.saturating_sub(now).max(1));
        if keep == 1 {
            return Ok(left);
        }
        if let Some(n) = mine {
            alarms.swap_remove(n);
        }
        if nanoseconds > 0 {
            alarms.push(Alarm { at: now + nanoseconds, interval, process: Arc::downgrade(&me) });
        }
        // Gone processes' alarms go too - on any change, so the list
        // never holds more than the processes that set one since.
        alarms.retain(|alarm| alarm.process.strong_count() > 0);
        left
    };
    CHANGES.fetch_add(1, Ordering::AcqRel);
    start();
    CHANGED.wake_all();
    Ok(left)
}

/// Starts the "alarms" thread, if it has not started. At boot, before
/// init (journal entry 128): it used to start with the first alarm, so
/// that a system none set never had it; but then the first program that
/// set one left a thread behind that was not there before - which the
/// boot test's "ps before and after" counts as a leak. Waiting, it costs
/// nothing: a tick looks only at the sleepers due (K96).
pub fn start() {
    if !STARTED.swap(true, Ordering::AcqRel) {
        threads::spawn("alarms", ring);
    }
}

/// The "alarms" thread.
fn ring() {
    loop {
        let seen = CHANGES.load(Ordering::Acquire);
        let now = threads::now_ns();
        let (due, next): (Vec<Arc<Process>>, Option<u64>) = {
            let mut alarms = ALARMS.lock();
            let mut due = Vec::new();
            alarms.retain_mut(|alarm| {
                if alarm.at > now {
                    return true;
                }
                due.extend(alarm.process.upgrade());
                if alarm.interval == 0 {
                    return false;
                }
                // Again, stepped from when it was due, so that it does not
                // drift; intervals missed meanwhile ring once, as on Linux
                // (a signal pending is pending once).
                let behind = (now - alarm.at) / alarm.interval;
                alarm.at += (behind + 1) * alarm.interval;
                alarm.process.strong_count() > 0
            });
            (due, alarms.iter().map(|alarm| alarm.at).min())
        };
        for process in due {
            process::send_signal(&process, abi::signal::SIGALRM);
        }
        // Until the next is due, or the list changes. Rule 2 of
        // threads.rs: a change after `seen` was read is seen here.
        let _ = threads::wait_any_until_ns(&[&CHANGED], next, || CHANGES.load(Ordering::Acquire) == seen);
    }
}
