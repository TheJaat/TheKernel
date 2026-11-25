//! A process's memory account: how many pages it has taken, and how
//! many it may.
//!
//! Without one, a single program can take every frame the machine has:
//! a compiler on a big crate, or a loop that maps memory and never
//! gives it back. The services - the file system, the console - then
//! find none for themselves, and the whole system fails because of one
//! program. With an account, that program is stopped at its limit, and
//! told so, and the rest carry on.
//!
//! What is counted: the pages a program's file is loaded into, its
//! stack, every page of reserved memory as it is first touched, and
//! the memory objects it makes (counted against it for as long as the
//! object lives, wherever its handle went). Not counted: page tables
//! and the kernel's own bookkeeping, which are small beside these.
//!
//! The limit can only go down (MEMORY_LIMIT): a program may promise to
//! use less, never claim more. A new program starts with its parent's
//! limit, or the machine's default if that is lower - so a shell that
//! lowers its own limit lowers it for everything it runs.
//!
//! In pages, and lock-free: a page fault on one CPU and a system call
//! on another may charge the same account at the same moment.

use core::sync::atomic::{AtomicU64, Ordering};

pub struct Account {
    used: AtomicU64,
    limit: AtomicU64,
}

impl Account {
    pub const fn new(limit: u64) -> Account {
        Account { used: AtomicU64::new(0), limit: AtomicU64::new(limit) }
    }

    /// Takes `pages` more if they fit under the limit; false if not, and
    /// then nothing is taken.
    pub fn charge(&self, pages: u64) -> bool {
        let limit = self.limit.load(Ordering::Relaxed);
        self.used
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(pages).filter(|&total| total <= limit)
            })
            .is_ok()
    }

    /// Gives back `pages` taken earlier.
    pub fn uncharge(&self, pages: u64) {
        let before = self.used.fetch_sub(pages, Ordering::AcqRel);
        debug_assert!(before >= pages, "gave back more than was taken");
    }

    pub fn used(&self) -> u64 {
        self.used.load(Ordering::Acquire)
    }

    pub fn limit(&self) -> u64 {
        self.limit.load(Ordering::Acquire)
    }

    /// Lowers the limit to `pages`, if that is lower; returns the limit
    /// now. Pages already taken stay taken, even above it: only what is
    /// asked for next is refused.
    pub fn lower_limit(&self, pages: u64) -> u64 {
        self.limit.fetch_min(pages, Ordering::AcqRel).min(pages)
    }
}

/// The limit a program gets when nothing lower was asked for: all the
/// memory the machine had free when the kernel started, but a reserve -
/// a sixteenth, at least 16 MiB - that no single program can take, so
/// the services and the kernel's own page tables always find some.
pub fn default_limit(free_pages: u64) -> u64 {
    let reserve = (free_pages / 16).max(16 << 20 >> 12);
    free_pages.saturating_sub(reserve)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charges_stop_at_the_limit() {
        let account = Account::new(10);
        assert!(account.charge(4));
        assert!(account.charge(6), "exactly to the limit");
        assert!(!account.charge(1), "one past it");
        assert_eq!(account.used(), 10, "a refused charge takes nothing");
        account.uncharge(3);
        assert!(account.charge(3));
        assert!(!account.charge(u64::MAX), "no overflow");
    }

    #[test]
    fn the_limit_only_goes_down() {
        let account = Account::new(100);
        assert!(account.charge(50));
        assert_eq!(account.lower_limit(200), 100, "raising is ignored");
        assert_eq!(account.lower_limit(40), 40);
        assert_eq!(account.used(), 50, "taken stays taken");
        assert!(!account.charge(1), "above the limit now");
        account.uncharge(20);
        assert!(!account.charge(11), "30 + 11 > 40");
        assert!(account.charge(10));
    }

    #[test]
    fn the_default_keeps_a_reserve() {
        let mib = 256; // pages per MiB
        assert_eq!(default_limit(4096 * mib), 4096 * mib - 256 * mib, "a sixteenth of 4 GiB");
        assert_eq!(default_limit(128 * mib), 112 * mib, "16 MiB when a sixteenth is less");
        assert_eq!(default_limit(8 * mib), 0, "a tiny machine: nothing to spare");
    }
}
