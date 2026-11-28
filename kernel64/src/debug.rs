//! The debugger's call (TheTaaJ K142, abi::debug): stop a process, read
//! and write its memory and its stopped threads' registers, and stop it
//! at breakpoints - on a process handle with `rights::DEBUG`, which only
//! whoever started the process holds.
//!
//! A thread stops at its next return to the program (`checkpoint`,
//! called first thing by `signals::on_return`): its registers are then
//! what it will go back to, kept here as a `signal::Context` while it
//! waits - read, and perhaps changed, by the debugger - and put back
//! when it is let go. A thread waiting in a system call stops when the
//! call returns.

use crate::ipc::Object;
use crate::process::{self, Process};
use crate::signals::UserFrame;
use crate::threads::WaitQueue;
use abi::debug::{op, reason, Stop};
use abi::signal::Context;
use abi::{rights, Error};
use kernel64::cap::Handle;
use alloc::sync::Arc;
use alloc::vec::Vec;
use kernel64::cap::Rights;

/// A process's debugging, in its `Process`.
#[derive(Default)]
pub struct State {
    /// A debugger has used the call on it: a breakpoint then stops it
    /// (otherwise, as before, it is passed over).
    attached: bool,
    /// Its threads are to stop at their next return to the program.
    stop: bool,
    /// The threads stopped now.
    stopped: Vec<Stopped>,
    /// The next stopped thread's number.
    next: u64,
    /// Why the next thread to stop does: a breakpoint's, else ASKED.
    pending: Option<(u64, u64)>,
}

struct Stopped {
    number: u64,
    context: Context,
    changed: bool,
    reason: u64,
    address: u64,
    /// Told to the debugger by WAIT.
    reported: bool,
}

/// Every stopping and every letting go: what stopped threads and a
/// debugger in WAIT wait on.
static CHANGED: WaitQueue = WaitQueue::new();

/// On the way back to the program: if the process is to stop, this
/// thread stops here until it is let go, its registers given to the
/// debugger - and back, as the debugger left them.
pub fn checkpoint(frame: &mut impl UserFrame) {
    let Some(process) = process::current() else { return };
    let number = {
        let mut state = process.debug.lock();
        if !state.stop {
            return;
        }
        let number = state.next;
        state.next += 1;
        let context = frame.context();
        let (reason, address) = state.pending.take().unwrap_or((reason::ASKED, context.pc));
        state.stopped.push(Stopped { number, context, changed: false, reason, address, reported: false });
        number
    };
    CHANGED.wake_all();
    // Until let go - or killed: then the return path ends it.
    // (Cut short by a signal, it goes on: its next return stops it again.)
    let _ = CHANGED.wait_while(|| process.debug.lock().stop && !process.kill_requested());
    let mut state = process.debug.lock();
    if let Some(at) = state.stopped.iter().position(|s| s.number == number) {
        let stopped = state.stopped.remove(at);
        if stopped.changed {
            frame.restore(&stopped.context);
        }
    }
}

/// A breakpoint was reached at `address` in the current process. True
/// if a debugger is attached - the process then stops, this thread
/// first (its caller goes on to the return path); false if not: the
/// breakpoint is passed over, as before.
pub fn breakpoint(address: u64) -> bool {
    let Some(process) = process::current() else { return false };
    let mut state = process.debug.lock();
    if !state.attached {
        return false;
    }
    state.stop = true;
    state.pending = Some((reason::BREAKPOINT, address));
    true
}

fn target(handle: Handle) -> Result<Arc<Process>, Error> {
    match process::current().ok_or(Error::InvalidHandle)?.handles.lock().get(handle, Rights(rights::DEBUG))? {
        Object::Process(target) => Ok(Arc::clone(target)),
        _ => Err(Error::WrongType),
    }
}

/// The most bytes READ and WRITE move in one call.
const BYTES_MAX: u64 = 4096;

/// `process_debug(handle, op, a, b, c)`.
pub fn call(handle: Handle, operation: u64, a: u64, b: u64, c: u64) -> Result<u64, Error> {
    let target = target(handle)?;
    target.debug.lock().attached = true;
    match operation {
        op::STOP => {
            target.debug.lock().stop = true;
            Ok(0)
        }
        op::CONTINUE => {
            target.debug.lock().stop = false;
            CHANGED.wake_all();
            Ok(0)
        }
        op::WAIT => {
            // Polled a tick at a time: a debugger waits for a person.
            for tick in 0..=a {
                let found = {
                    let mut state = target.debug.lock();
                    state.stopped.iter_mut().find(|s| !s.reported).map(|s| {
                        s.reported = true;
                        (s.number, Stop { reason: s.reason, address: s.address, pid: target.pid })
                    })
                };
                if let Some((number, stop)) = found {
                    let words = stop.to_words();
                    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
                    process::copy_to_user(b, &bytes)?;
                    return Ok(number);
                }
                if tick < a && !crate::threads::sleep(1) {
                    return Err(Error::Interrupted);
                }
            }
            Err(Error::TimedOut)
        }
        op::READ => {
            if c > BYTES_MAX {
                return Err(Error::InvalidArgument);
            }
            let mut bytes = alloc::vec![0u8; c as usize];
            target.debug_memory(a, &mut bytes, false)?;
            process::copy_to_user(b, &bytes)?;
            Ok(c)
        }
        op::WRITE => {
            if c > BYTES_MAX {
                return Err(Error::InvalidArgument);
            }
            let mut bytes = process::copy_from_user(b, c)?;
            target.debug_memory(a, &mut bytes, true)?;
            Ok(c)
        }
        op::GET_REGISTERS => {
            let bytes = {
                let state = target.debug.lock();
                state.stopped.iter().find(|s| s.number == a).ok_or(Error::InvalidArgument)?.context.to_bytes()
            };
            process::copy_to_user(b, &bytes)?;
            Ok(0)
        }
        op::SET_REGISTERS => {
            let bytes = process::copy_from_user(b, abi::signal::CONTEXT_SIZE as u64)?;
            let context = Context::from_bytes(&bytes).ok_or(Error::InvalidArgument)?;
            let mut state = target.debug.lock();
            let stopped = state.stopped.iter_mut().find(|s| s.number == a).ok_or(Error::InvalidArgument)?;
            stopped.context = context;
            stopped.changed = true;
            Ok(0)
        }
        _ => Err(Error::InvalidArgument),
    }
}
