//! Many writers, one line: keeping each one's lines whole.
//!
//! The debug log is one serial line, and everything is written to it:
//! the kernel's own messages, and what every program prints. Each
//! writer hands over a piece at a time, and a piece is not always a
//! line - `argc `, then `1`, then the newline. With one CPU that rarely
//! showed: a writer mostly got its line finished before anyone else
//! ran. With several, somebody else's line lands in the gap:
//!
//!   argc [process] 132 "chello" exited with code 3
//!   2
//!
//! On the PC that is an untidy log. On a machine whose serial line is
//! also its screen it is what the person at the terminal reads.
//!
//! So the log is merged by line. While one writer has a line open -
//! its last piece did not end in a newline - what the others write is
//! *held*, and comes out when that line ends.
//!
//! Not for ever. A shell's prompt is a line that stays open until
//! somebody presses Enter, and the kernel's messages cannot wait for
//! that. Held text waits a few ticks at most (`HOLD_TICKS`), and is
//! then written into the open line after all: a torn line is better
//! than a log that says nothing.
//!
//! The bookkeeping only - which writer has the line, what is held and
//! since when. The lock around it and the serial port after it are the
//! kernel's (console.rs).

use crate::logring::LogRing;

/// How long held text waits for an open line to end, in timer ticks.
/// Long enough for a piece that is on its way through a driver to
/// arrive (it takes a fraction of one tick); short enough that nobody
/// reading the log notices the wait.
pub const HOLD_TICKS: u64 = 3;

/// Who wrote a piece of the log.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Writer {
    Kernel,
    /// A program, by its process id.
    Program(u64),
}

/// `N` bytes may wait to be written; `H` of other writers' may be held
/// meanwhile.
pub struct Merge<const N: usize, const H: usize> {
    /// What is to be written, in order.
    queue: LogRing<N>,
    /// Who has the line open: the writer of the last piece that went
    /// towards the line, if that piece did not end it.
    open: Option<Writer>,
    /// What others wrote while it was open.
    held: LogRing<H>,
    /// Who has a line open *inside* the held text - what `open` will be
    /// once it has been let go.
    held_open: Option<Writer>,
    /// The tick at which the oldest held piece arrived.
    held_since: u64,
}

fn ends_line(bytes: &[u8]) -> bool {
    bytes.last() == Some(&b'\n')
}

impl<const N: usize, const H: usize> Merge<N, H> {
    pub const fn new() -> Merge<N, H> {
        Merge { queue: LogRing::new(), open: None, held: LogRing::new(), held_open: None, held_since: 0 }
    }

    /// What is ready to be written: the caller takes bytes out of it.
    pub fn queue(&mut self) -> &mut LogRing<N> {
        &mut self.queue
    }

    /// Is anything waiting for a line to end?
    pub fn holding(&self) -> bool {
        !self.held.is_empty()
    }

    /// May `writer` put a piece on the line now, without tearing
    /// anybody's? (The kernel asks before it writes directly.)
    pub fn free_for(&self, writer: Writer) -> bool {
        self.open.is_none() || self.open == Some(writer)
    }

    /// A piece from `writer`, at tick `now`: queued for the line, or
    /// held until the line is free.
    pub fn write(&mut self, writer: Writer, bytes: &[u8], now: u64) {
        if bytes.is_empty() {
            return;
        }
        if self.free_for(writer) {
            self.queue.push(bytes);
            self.wrote(writer, ends_line(bytes));
            return;
        }
        if self.held.is_empty() {
            self.held_since = now;
        }
        if self.held.push(bytes) {
            self.held_open = if ends_line(bytes) { None } else { Some(writer) };
        } else {
            // No room to hold it. Let everything go, in the order it
            // came: lines are torn, nothing is lost.
            self.held.take_dropped();
            self.let_go();
            self.queue.push(bytes);
            self.open = if ends_line(bytes) { None } else { Some(writer) };
        }
    }

    /// `writer` has put a piece on the line (through the queue, or -
    /// the kernel - directly); `ended` says whether it ended in a
    /// newline. If the line is over, what was held follows it.
    pub fn wrote(&mut self, writer: Writer, ended: bool) {
        if ended {
            self.open = None;
            self.let_go();
        } else {
            self.open = Some(writer);
        }
    }

    /// The timer's turn: what has been held for long enough is written
    /// into the open line.
    pub fn tick(&mut self, now: u64) {
        if self.holding() && now.saturating_sub(self.held_since) >= HOLD_TICKS {
            self.let_go();
        }
    }

    /// Moves everything held to the queue. The line then belongs to
    /// whoever left one open in there.
    pub fn let_go(&mut self) {
        if self.held.is_empty() {
            return;
        }
        let mut piece = [0u8; 64];
        loop {
            let n = self.held.pop(&mut piece);
            if n == 0 {
                break;
            }
            self.queue.push(&piece[..n]);
        }
        self.open = self.held_open.take();
    }
}

impl<const N: usize, const H: usize> Default for Merge<N, H> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const K: Writer = Writer::Kernel;
    const A: Writer = Writer::Program(7);
    const B: Writer = Writer::Program(9);

    /// Everything ready to be written, as text.
    fn out<const N: usize, const H: usize>(merge: &mut Merge<N, H>) -> String {
        let mut bytes = vec![0u8; N];
        let n = merge.queue().pop(&mut bytes);
        String::from_utf8(bytes[..n].to_vec()).unwrap()
    }

    #[test]
    fn whole_lines_come_out_in_the_order_they_were_written() {
        let mut merge: Merge<256, 64> = Merge::new();
        merge.write(A, b"one\n", 0);
        merge.write(B, b"two\n", 0);
        merge.write(K, b"three\n", 0);
        assert!(!merge.holding());
        assert_eq!(out(&mut merge), "one\ntwo\nthree\n");
    }

    #[test]
    fn a_line_written_in_pieces_is_not_torn_by_another_writer() {
        let mut merge: Merge<256, 64> = Merge::new();
        merge.write(A, b"argc ", 0);
        assert!(!merge.free_for(K));
        merge.write(K, b"[process] exited\n", 0);
        merge.write(B, b"[shell] ran\n", 0);
        // Only A's piece so far: the others wait.
        assert_eq!(out(&mut merge), "argc ");
        assert!(merge.holding());
        merge.write(A, b"2\n", 1);
        assert!(!merge.holding());
        assert_eq!(out(&mut merge), "2\n[process] exited\n[shell] ran\n");
        assert!(merge.free_for(K));
    }

    #[test]
    fn held_text_waits_a_few_ticks_and_no_longer() {
        let mut merge: Merge<256, 64> = Merge::new();
        merge.write(A, b"taaj:/> ", 10);
        merge.write(K, b"[kernel] something\n", 12);
        merge.tick(12 + HOLD_TICKS - 1);
        assert_eq!(out(&mut merge), "taaj:/> ");
        merge.tick(12 + HOLD_TICKS);
        assert_eq!(out(&mut merge), "[kernel] something\n");
        // The line was torn; it is nobody's now, and A just carries on.
        assert!(merge.free_for(B));
        merge.write(A, b"ls\n", 20);
        assert_eq!(out(&mut merge), "ls\n");
    }

    #[test]
    fn the_wait_is_counted_from_the_oldest_held_piece() {
        let mut merge: Merge<256, 64> = Merge::new();
        merge.write(A, b"> ", 0);
        merge.write(K, b"first\n", 5);
        merge.write(K, b"second\n", 7);
        merge.tick(5 + HOLD_TICKS);
        assert_eq!(out(&mut merge), "> first\nsecond\n");
    }

    #[test]
    fn a_line_left_open_in_the_held_text_is_open_afterwards() {
        let mut merge: Merge<256, 64> = Merge::new();
        merge.write(A, b"a", 0);
        merge.write(B, b"b-line, unfinished", 0);
        merge.write(A, b"\n", 0);
        // B's line is on its way out and still open: the kernel waits.
        assert!(!merge.free_for(K));
        assert!(merge.free_for(B));
        merge.write(K, b"kernel\n", 1);
        merge.write(B, b"\n", 1);
        assert_eq!(out(&mut merge), "a\nb-line, unfinished\nkernel\n");
    }

    #[test]
    fn the_kernel_writing_directly_opens_and_ends_lines_too() {
        let mut merge: Merge<256, 64> = Merge::new();
        // The kernel wrote "Loading... " itself, not through the queue.
        assert!(merge.free_for(K));
        merge.wrote(K, false);
        merge.write(A, b"program\n", 0);
        assert_eq!(out(&mut merge), "");
        assert!(merge.free_for(K));
        merge.wrote(K, true);
        assert_eq!(out(&mut merge), "program\n");
    }

    #[test]
    fn with_no_room_to_hold_everything_is_let_go_and_nothing_is_lost() {
        let mut merge: Merge<256, 16> = Merge::new();
        merge.write(A, b"open ", 0);
        merge.write(K, b"0123456789\n", 0);
        merge.write(B, b"too long to hold as well\n", 0);
        assert!(!merge.holding());
        assert_eq!(out(&mut merge), "open 0123456789\ntoo long to hold as well\n");
        assert_eq!(merge.queue().take_dropped(), 0);
        assert!(merge.free_for(A));
    }

    #[test]
    fn an_empty_piece_changes_nothing() {
        let mut merge: Merge<64, 16> = Merge::new();
        merge.write(A, b"x", 0);
        merge.write(B, b"", 0);
        merge.write(A, b"", 0);
        assert!(!merge.holding());
        assert!(!merge.free_for(B));
    }
}
