//! `kprint!` and `kprintln!`: formatted output from anywhere in the
//! kernel.
//!
//! Portable: the actual bytes go to `arch::console_write`, which on
//! x86_64 is the COM1 serial port and on AArch64 a PL011.
//!
//! A lock keeps each message whole: without it, a message printed by an
//! interrupt handler could land in the middle of another one's line.
//!
//! Two kinds of text come through here, treated differently:
//!
//!   - The kernel's own messages (`kprintln!`) are written at once,
//!     while the caller waits. They are few, and some must not be
//!     delayed: a panic is followed by nothing.
//!   - What programs print (the debug_write call) is queued, and
//!     written when the CPU is idle. A program does not wait for the
//!     serial line; and the kernel never keeps interrupts off for the
//!     length of a program's message. If programs print faster than the
//!     line can carry - a flood - messages are dropped, and the log says
//!     how much is missing.
//!
//!     "When the CPU is idle" alone is not enough. A machine whose
//!     every CPU is busy computing is never idle, and its queue would
//!     sit there until the computing ended - on a machine whose serial
//!     line is also its screen (AArch64, so far) that is a terminal
//!     that stops answering while a program runs in the background.
//!     So the timer writes a little too, every tick, busy or not
//!     (`write_on_tick`).
//!
//! Order is kept: before the kernel writes a message of its own it
//! writes out everything queued before it.
//!
//! And lines are kept whole, as far as that can be had: while one
//! writer has a line unfinished, what the others write - the kernel
//! too - waits for it to end, for a few ticks at most
//! (`kernel64::logmerge`, which has the why). So a message of the
//! kernel's is *not* always written before `kprintln!` returns: it may
//! be held, for those few ticks, behind a program's half line. When
//! the kernel is stopping nothing is held any more (`break_lock`).

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, Ordering};

use kernel64::logmerge::{Merge, Writer};

use crate::irqlock::IrqLock;

/// Room for a few screens of text between two idle moments.
const QUEUE_SIZE: usize = 64 * 1024;
/// Bytes written per turn of the idle loop, with interrupts off: about
/// 1.4 ms at 115200 baud.
const CHUNK: usize = 16;

/// Bytes written per timer tick, with interrupts off. A tick is 10 ms,
/// in which the line carries 115 bytes at 115200 baud: this is a
/// quarter of that, so at worst - a queue that never empties, a line
/// that really is that slow - a quarter of one CPU goes on writing. It
/// is still some 3000 characters a second: a screenful in under one.
const TICK_CHUNK: usize = 32;

/// Room for what waits behind somebody's unfinished line: a few
/// ticks' worth of messages.
const HELD_SIZE: usize = 4 * 1024;

type Log = Merge<QUEUE_SIZE, HELD_SIZE>;

/// The lock for the serial line, and what it guards: the queue.
static QUEUE: IrqLock<Log> = IrqLock::new(Merge::new());

/// The kernel is on its way to halting: its last words are written at
/// once, whoever's line they land in.
static STOPPING: AtomicBool = AtomicBool::new(false);

/// The serial line itself, for the kernel's own messages.
struct Console {
    /// Whether the last piece written ended a line; None if nothing
    /// was written.
    ended: Option<bool>,
}

/// Implementing `fmt::Write` is all it takes to get `{}` formatting:
/// `write_fmt` comes free with the trait and calls `write_str` with each
/// formatted piece.
impl Write for Console {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if !text.is_empty() {
            self.ended = Some(text.ends_with('\n'));
        }
        crate::arch::console_write(text);
        // And on the screen - once the kernel has taken it, to stop.
        #[cfg(target_arch = "x86_64")]
        crate::lastwords::echo(text);
        Ok(())
    }
}

/// Writes out up to `limit` queued bytes; and, once the queue is empty,
/// a line saying what was dropped, if anything was. Returns whether it
/// wrote anything. The caller holds the lock.
fn flush(log: &mut Log, limit: usize) -> bool {
    let queue = log.queue();
    let mut chunk = [0u8; CHUNK];
    let mut written = 0;
    while written < limit {
        let n = queue.pop(&mut chunk[..CHUNK.min(limit - written)]);
        if n == 0 {
            break;
        }
        crate::arch::console_write_bytes(&chunk[..n]);
        written += n;
    }
    if queue.is_empty() {
        let dropped = queue.take_dropped();
        if dropped > 0 {
            let _ = writeln!(Console { ended: None }, "\n[log] {dropped} bytes of program output are missing here: the serial line could not keep up");
            return true;
        }
    }
    written > 0
}

/// Not for direct use - the macros call it. `pub` only because macros
/// expand at the call site, which may be in another module.
pub fn print(args: fmt::Arguments) {
    let mut log = QUEUE.lock();
    let stopping = STOPPING.load(Ordering::Relaxed);
    if !stopping && !log.free_for(Writer::Kernel) {
        // A program is in the middle of a line. This message waits
        // behind it - formatted now, written when that line ends or
        // the timer has waited long enough.
        let now = crate::interrupts::ticks();
        let _ = Held { log: &mut log, now }.write_fmt(args);
        return;
    }
    if stopping {
        log.let_go();
    }
    // What programs printed before this comes before it.
    flush(&mut log, usize::MAX);
    let mut console = Console { ended: None };
    // Writing to the console cannot fail, so the Result carries nothing.
    let _ = console.write_fmt(args);
    if let Some(ended) = console.ended {
        log.wrote(Writer::Kernel, ended);
    }
}

/// A message of the kernel's that has to wait: its pieces go where a
/// program's would.
struct Held<'a> {
    log: &'a mut Log,
    now: u64,
}

impl Write for Held<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.log.write(Writer::Kernel, text.as_bytes(), self.now);
        Ok(())
    }
}

/// Queues what the program `pid` printed for the log. Never waits: if
/// there is no room the message is dropped, and counted.
pub fn write_later(pid: u64, bytes: &[u8]) {
    QUEUE.lock().write(Writer::Program(pid), bytes, crate::interrupts::ticks());
}

/// Writes a little of what is queued; false if there was nothing. For
/// the idle loop, which calls it until then.
pub fn write_queued() -> bool {
    flush(&mut QUEUE.lock(), CHUNK)
}

/// Writes a little of what is queued, from the timer interrupt: what
/// keeps programs' output moving on a machine that is never idle.
/// And what lets go of text that has waited long enough for somebody
/// else's line to end.
pub fn write_on_tick() {
    let mut log = QUEUE.lock();
    log.tick(crate::interrupts::ticks());
    flush(&mut log, TICK_CHUNK);
}

/// Frees the console lock, for a panic or fatal exception that must be
/// reported even if it struck while something was printing.
///
/// # Safety
/// Only on the way to halting: whoever held the lock never runs again.
pub unsafe fn break_lock() {
    // With several CPUs the lock may be held by another one, which is
    // alive and about to let go: give it a moment, so that its line and
    // the report do not come out letter by letter through each other.
    // If the holder is this CPU - the fault struck while it was
    // printing - nobody will ever let go, and the wait runs out.
    for _ in 0..2_000_000 {
        if !QUEUE.is_locked() {
            break;
        }
        core::hint::spin_loop();
    }
    if QUEUE.is_locked() {
        QUEUE.force_unlock();
    }
    STOPPING.store(true, Ordering::Relaxed);
    // The kernel is stopping: what it says now goes on the screen too.
    #[cfg(target_arch = "x86_64")]
    crate::lastwords::begin();
}

/// Like `print!`, for the kernel.
#[allow(unused_macros)] // part of the console's interface; not every build uses it
macro_rules! kprint {
    ($($arg:tt)*) => {
        $crate::console::print(format_args!($($arg)*))
    };
}

/// Like `println!`, for the kernel.
///
/// `#[macro_use]` on `mod console` in main.rs makes both macros usable by
/// name in every module declared after it.
macro_rules! kprintln {
    () => {
        $crate::console::print(format_args!("\n"))
    };
    ($($arg:tt)*) => {
        $crate::console::print(format_args!("{}\n", format_args!($($arg)*)))
    };
}
