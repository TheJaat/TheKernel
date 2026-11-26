//! What a program may do with each page of one reservation: the record
//! MEMORY_PROTECT (mprotect) keeps.
//!
//! A reservation starts with one access for all of it - readable and
//! writable, or read-only - and mprotect changes parts of it: a guard
//! page with no access below a thread's stack, a JIT's code buffer made
//! read-only, an allocator's arena kept out of reach until it is used.
//! Most of a reservation stays as it began, so only what differs is
//! kept: a sorted list of ranges that do not overlap, each with an
//! access other than the reservation's own. Changing a range splits the
//! ranges it cuts, and ranges that end up with the same access as their
//! neighbour, or as the reservation, are merged or dropped - so the list
//! never grows from changing a range back.
//!
//! The page tables say what the CPU allows for pages already touched;
//! this says it for every page, touched or not, and is what a fault on
//! an untouched page asks (process.rs). Kept apart from the kernel's
//! types, so it is tested on the host.

use alloc::vec::Vec;
use core::ops::Range;

/// What a page allows (abi::access's values).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    None,
    Read,
    ReadWrite,
}

impl Access {
    /// abi::access's number, or None for one that is not.
    pub fn from_abi(value: u64) -> Option<Access> {
        match value {
            abi::access::NONE => Some(Access::None),
            abi::access::READ => Some(Access::Read),
            abi::access::READ_WRITE => Some(Access::ReadWrite),
            _ => None,
        }
    }

    /// Whether a read (or a write, if `write`) is allowed.
    pub fn allows(self, write: bool) -> bool {
        match self {
            Access::None => false,
            Access::Read => !write,
            Access::ReadWrite => true,
        }
    }
}

pub struct Protections {
    /// The access of every page not in `changed`.
    usual: Access,
    /// Sorted, not overlapping, not touching another of the same
    /// access, and never `usual`.
    changed: Vec<(Range<u64>, Access)>,
}

impl Protections {
    pub fn new(usual: Access) -> Protections {
        Protections { usual, changed: Vec::new() }
    }

    /// The access of the page `address` is in.
    pub fn at(&self, address: u64) -> Access {
        let n = self.changed.partition_point(|(range, _)| range.start <= address);
        match n.checked_sub(1).map(|n| &self.changed[n]) {
            Some((range, access)) if range.contains(&address) => *access,
            _ => self.usual,
        }
    }

    /// Gives `range` the access `access`.
    pub fn set(&mut self, range: Range<u64>, access: Access) {
        if range.is_empty() {
            return;
        }
        // What lies outside `range` stays, cut where `range` cuts it.
        let mut kept: Vec<(Range<u64>, Access)> = Vec::with_capacity(self.changed.len() + 2);
        for (piece, piece_access) in self.changed.drain(..) {
            if piece.end <= range.start || piece.start >= range.end {
                kept.push((piece, piece_access));
                continue;
            }
            if piece.start < range.start {
                kept.push((piece.start..range.start, piece_access));
            }
            if piece.end > range.end {
                kept.push((range.end..piece.end, piece_access));
            }
        }
        if access != self.usual {
            kept.push((range, access));
        }
        kept.sort_by_key(|(piece, _)| piece.start);
        // Neighbours with the same access become one.
        for (piece, piece_access) in kept {
            match self.changed.last_mut() {
                Some((last, last_access)) if last.end == piece.start && *last_access == piece_access => last.end = piece.end,
                _ => self.changed.push((piece, piece_access)),
            }
        }
    }

    /// How many ranges differ from the reservation's own access: what
    /// the record costs.
    pub fn changed(&self) -> usize {
        self.changed.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: u64 = 4096;

    #[test]
    fn a_reservation_begins_with_one_access() {
        let p = Protections::new(Access::ReadWrite);
        assert_eq!(p.at(0), Access::ReadWrite);
        assert_eq!(p.at(1 << 40), Access::ReadWrite);
        assert_eq!(p.changed(), 0);
        assert_eq!(Protections::new(Access::Read).at(P), Access::Read);
    }

    #[test]
    fn a_guard_page_and_a_read_only_stretch() {
        let mut p = Protections::new(Access::ReadWrite);
        p.set(0..P, Access::None);
        p.set(4 * P..8 * P, Access::Read);
        assert_eq!(p.at(0), Access::None);
        assert_eq!(p.at(P - 1), Access::None);
        assert_eq!(p.at(P), Access::ReadWrite);
        assert_eq!(p.at(4 * P), Access::Read);
        assert_eq!(p.at(8 * P - 1), Access::Read);
        assert_eq!(p.at(8 * P), Access::ReadWrite);
        assert_eq!(p.changed(), 2);
    }

    #[test]
    fn changing_the_middle_of_a_range_splits_it() {
        let mut p = Protections::new(Access::ReadWrite);
        p.set(0..10 * P, Access::None);
        p.set(3 * P..5 * P, Access::Read);
        assert_eq!(p.at(2 * P), Access::None);
        assert_eq!(p.at(3 * P), Access::Read);
        assert_eq!(p.at(5 * P), Access::None);
        assert_eq!(p.changed(), 3);
        // And back: one range again.
        p.set(3 * P..5 * P, Access::None);
        assert_eq!(p.changed(), 1);
        assert_eq!(p.at(4 * P), Access::None);
    }

    #[test]
    fn changing_back_to_the_usual_access_leaves_nothing_behind() {
        let mut p = Protections::new(Access::ReadWrite);
        p.set(2 * P..6 * P, Access::None);
        p.set(0..8 * P, Access::ReadWrite);
        assert_eq!(p.changed(), 0);
        assert_eq!(p.at(3 * P), Access::ReadWrite);
    }

    #[test]
    fn a_range_over_several_others_replaces_them() {
        let mut p = Protections::new(Access::ReadWrite);
        p.set(P..2 * P, Access::None);
        p.set(3 * P..4 * P, Access::Read);
        p.set(5 * P..6 * P, Access::None);
        p.set(0..10 * P, Access::Read);
        assert_eq!(p.changed(), 1);
        assert_eq!(p.at(0), Access::Read);
        assert_eq!(p.at(9 * P), Access::Read);
        assert_eq!(p.at(10 * P), Access::ReadWrite);
    }

    #[test]
    fn neighbours_of_the_same_access_join() {
        let mut p = Protections::new(Access::ReadWrite);
        p.set(0..P, Access::None);
        p.set(2 * P..3 * P, Access::None);
        assert_eq!(p.changed(), 2);
        p.set(P..2 * P, Access::None);
        assert_eq!(p.changed(), 1);
        assert_eq!(p.at(P), Access::None);
    }

    #[test]
    fn access_numbers_and_what_they_allow() {
        assert_eq!(Access::from_abi(abi::access::NONE), Some(Access::None));
        assert_eq!(Access::from_abi(abi::access::READ), Some(Access::Read));
        assert_eq!(Access::from_abi(abi::access::READ_WRITE), Some(Access::ReadWrite));
        assert_eq!(Access::from_abi(3), None);
        assert!(!Access::None.allows(false) && !Access::None.allows(true));
        assert!(Access::Read.allows(false) && !Access::Read.allows(true));
        assert!(Access::ReadWrite.allows(false) && Access::ReadWrite.allows(true));
    }
}
