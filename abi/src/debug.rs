//! The debugger's call, `call::PROCESS_DEBUG` (TheTaaJ K142):
//! `process_debug(process, op, a, b, c)` on a process handle with
//! `rights::DEBUG`, which only whoever started the process holds.
//!
//! A process is stopped thread by thread: each of its threads stops on
//! its next way back to the program - from a system call, an interrupt,
//! a fault - and waits there until it is let go. A thread waiting in a
//! system call (asleep, receiving) stops when the call returns. While a
//! thread is stopped, its registers are what it will go back to: read
//! and changed as a `signal::Context`.
//!
//! A breakpoint is an instruction the debugger writes into the
//! program's code (`BREAKPOINT_X86_64`, `BREAKPOINT_AARCH64`) with
//! `WRITE`: a process's code is its own copy, so no other process is
//! touched. The thread that reaches one stops, and the whole process
//! with it; `WAIT` says where. To go on past it, the debugger puts the
//! original bytes back and sets the stopped thread's pc to the
//! breakpoint's address (on x86_64 the CPU reports the address after
//! the `int3`).

/// The operations: `op`, and what `a`, `b` and `c` are.
pub mod op {
    /// Stop the process: every thread at its next return to the program.
    pub const STOP: u64 = 1;
    /// Let every stopped thread go on (with its registers as last set).
    pub const CONTINUE: u64 = 2;
    /// Wait up to `a` ticks for a stopped thread: returns its number
    /// (0 to 255), and writes `Stop` (3 words) at `b`. `TimedOut` if
    /// none stopped by then.
    pub const WAIT: u64 = 3;
    /// Read `c` bytes of the process's memory at address `a` into the
    /// caller's at `b`. Only memory the process has (pages it touched).
    pub const READ: u64 = 4;
    /// Write `c` bytes from the caller's memory at `b` into the
    /// process's at `a` - its code too: how a breakpoint is set.
    pub const WRITE: u64 = 5;
    /// The registers of stopped thread `a` (a `WAIT` number), as a
    /// `signal::Context`, at `b` (`signal::CONTEXT_SIZE` bytes).
    pub const GET_REGISTERS: u64 = 6;
    /// Sets stopped thread `a`'s registers from the `signal::Context`
    /// at `b`: they are what it goes back to.
    pub const SET_REGISTERS: u64 = 7;
}

/// Why a thread stopped.
pub mod reason {
    /// The debugger asked (`op::STOP`).
    pub const ASKED: u64 = 1;
    /// It reached a breakpoint: `Stop::address` is the breakpoint's.
    pub const BREAKPOINT: u64 = 2;
}

/// `int3`.
pub const BREAKPOINT_X86_64: [u8; 1] = [0xcc];
/// `brk #0`.
pub const BREAKPOINT_AARCH64: [u8; 4] = 0xd420_0000u32.to_le_bytes();

/// What `op::WAIT` writes: why a thread stopped, and where.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stop {
    pub reason: u64,
    /// Where it stopped: a breakpoint's address; else the pc.
    pub address: u64,
    /// The process's id, which the debugger may not otherwise know.
    pub pid: u64,
}

impl Stop {
    pub const WORDS: usize = 3;

    pub fn to_words(&self) -> [u64; Self::WORDS] {
        [self.reason, self.address, self.pid]
    }

    pub fn from_words(words: [u64; Self::WORDS]) -> Stop {
        Stop { reason: words[0], address: words[1], pid: words[2] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stop_survives_its_words() {
        let stop = Stop { reason: reason::BREAKPOINT, address: 0x401234, pid: 48 };
        assert_eq!(Stop::from_words(stop.to_words()), stop);
        assert_eq!(BREAKPOINT_AARCH64, [0x00, 0x00, 0x20, 0xd4]);
        assert_ne!(super::super::rights::ALL & super::super::rights::DEBUG, 0);
    }
}
