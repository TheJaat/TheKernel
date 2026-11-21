//! Finding room in physical memory before there is an allocator.
//!
//! The frame allocator needs somewhere to keep its bitmap, and the
//! bitmap has to be placed before the allocator exists. So the very
//! first placement decision is made by hand: the first spot, in memory
//! the firmware calls usable, that nothing else already occupies.

use core::ops::Range;

use super::addr::align_up;

fn overlaps(a: &Range<u64>, b: &Range<u64>) -> bool {
    a.start < b.end && b.start < a.end
}

/// The lowest address `a` such that `a..a + size`:
///   - lies inside one of the `usable` ranges
///   - overlaps none of the `reserved` ranges
///   - is aligned to `align` (a power of two)
///   - ends at or below `limit`
pub fn find_free_space<U>(
    usable: U,
    reserved: &[Range<u64>],
    size: u64,
    align: u64,
    limit: u64,
) -> Option<u64>
where
    U: IntoIterator<Item = Range<u64>>,
{
    find_free_space_avoiding(usable, || reserved.iter().cloned(), size, align, limit)
}

/// The same, with the reserved ranges given as a function that lists
/// them - for when they come from more than one place (a firmware's
/// map and the kernel's own knowledge, say) and there is no heap yet
/// to gather them into one array.
pub fn find_free_space_avoiding<U, R, I>(usable: U, reserved: R, size: u64, align: u64, limit: u64) -> Option<u64>
where
    U: IntoIterator<Item = Range<u64>>,
    R: Fn() -> I,
    I: IntoIterator<Item = Range<u64>>,
{
    for region in usable {
        let mut candidate = align_up(region.start, align);

        // Try a spot; if it hits a reservation, jump past that
        // reservation and try again. Each retry moves strictly forward,
        // so this ends.
        'retry: while candidate.checked_add(size)? <= region.end.min(limit) {
            let wanted = candidate..candidate + size;
            for taken in reserved() {
                if overlaps(&wanted, &taken) {
                    candidate = align_up(taken.end, align);
                    continue 'retry;
                }
            }
            return Some(candidate);
        }
    }
    None
}

/// The part of `range` that is inside `window`, if any of it is.
pub fn inside(range: &Range<u64>, window: &Range<u64>) -> Option<Range<u64>> {
    let part = range.start.max(window.start)..range.end.min(window.end);
    (part.start < part.end).then_some(part)
}

/// The parts of `range` that are not inside `window`: what is below
/// it, and what is above it. Either may be empty.
pub fn outside(range: &Range<u64>, window: &Range<u64>) -> [Range<u64>; 2] {
    let below = range.start..range.end.min(window.start);
    let above = range.start.max(window.end)..range.end;
    let tidy = |part: Range<u64>| if part.start < part.end { part } else { 0..0 };
    [tidy(below), tidy(above)]
}

#[cfg(test)]
// Arrays of one range are exactly what these tests mean.
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;

    #[test]
    fn takes_the_first_fit() {
        let found = find_free_space([0..MIB], &[], 4 * KIB, 4 * KIB, u64::MAX);
        assert_eq!(found, Some(0));
    }

    #[test]
    fn steps_over_reservations() {
        // The QEMU layout: low memory, then RAM from 1 MiB with the
        // kernel at its start.
        let usable = [0..0x9fc00, MIB..512 * MIB];
        let reserved = [0..MIB, MIB..MIB + 0x1a000];
        let found = find_free_space(usable, &reserved, 16 * KIB, 4 * KIB, u64::MAX);
        assert_eq!(found, Some(MIB + 0x1a000));
    }

    #[test]
    fn steps_over_several_reservations_in_any_order() {
        let reserved = [0x5000..0x6000, 0x0..0x2000, 0x2000..0x5000];
        let found = find_free_space([0..0x10000], &reserved, 0x1000, 0x1000, u64::MAX);
        assert_eq!(found, Some(0x6000));
    }

    #[test]
    fn respects_alignment_and_limit() {
        let found = find_free_space([0x1234..0x100000], &[], 0x1000, 0x10000, u64::MAX);
        assert_eq!(found, Some(0x10000));
        assert_eq!(find_free_space([0x1000..0x100000], &[], 0x1000, 0x1000, 0x1800), None);
    }

    #[test]
    fn moves_to_the_next_region_when_one_is_too_small() {
        let usable = [0..0x800, 0x10000..0x20000];
        let found = find_free_space(usable, &[], 0x1000, 0x1000, u64::MAX);
        assert_eq!(found, Some(0x10000));
    }

    #[test]
    fn reports_when_nothing_fits() {
        assert_eq!(find_free_space([0..0x1000], &[0..0x1000], 0x1000, 0x1000, u64::MAX), None);
        assert_eq!(find_free_space(Vec::<Range<u64>>::new(), &[], 1, 1, u64::MAX), None);
    }

    #[test]
    fn reservations_can_come_from_a_function() {
        // Two lists, chained: neither alone would give the answer.
        let firmware = [0x0..0x3000];
        let kernel = [0x3000..0x8000];
        let both = || firmware.iter().cloned().chain(kernel.iter().cloned());
        assert_eq!(find_free_space_avoiding([0..0x10000], both, 0x1000, 0x1000, u64::MAX), Some(0x8000));
    }

    #[test]
    fn an_empty_reservation_is_in_nobodys_way() {
        assert_eq!(find_free_space([0..0x10000], &[0..0], 0x1000, 0x1000, u64::MAX), Some(0));
    }

    #[test]
    fn a_range_is_cut_at_a_window() {
        let window = 0x4000..0x8000;
        // Wholly inside, across each edge, across both, wholly outside.
        assert_eq!(inside(&(0x5000..0x6000), &window), Some(0x5000..0x6000));
        assert_eq!(inside(&(0x1000..0x5000), &window), Some(0x4000..0x5000));
        assert_eq!(inside(&(0x7000..0x9000), &window), Some(0x7000..0x8000));
        assert_eq!(inside(&(0x1000..0x9000), &window), Some(0x4000..0x8000));
        assert_eq!(inside(&(0x1000..0x4000), &window), None);
        assert_eq!(inside(&(0x8000..0x9000), &window), None);

        assert_eq!(outside(&(0x5000..0x6000), &window), [0..0, 0..0]);
        assert_eq!(outside(&(0x1000..0x5000), &window), [0x1000..0x4000, 0..0]);
        assert_eq!(outside(&(0x7000..0x9000), &window), [0..0, 0x8000..0x9000]);
        assert_eq!(outside(&(0x1000..0x9000), &window), [0x1000..0x4000, 0x8000..0x9000]);
        assert_eq!(outside(&(0x1000..0x4000), &window), [0x1000..0x4000, 0..0]);
        assert_eq!(outside(&(0x8000..0x9000), &window), [0..0, 0x8000..0x9000]);
    }

    #[test]
    fn inside_and_outside_together_are_the_whole_range() {
        let window = 0x4000_0000..0x8000_0000u64;
        for range in [0..0x1_0000_0000u64, 0x4000_0000..0x6000_0000, 0x7fff_f000..0x8000_1000, 0..0x1000] {
            let kept = inside(&range, &window).map_or(0, |part| part.end - part.start);
            let left: u64 = outside(&range, &window).iter().map(|part| part.end - part.start).sum();
            assert_eq!(kept + left, range.end - range.start, "{range:x?}");
        }
    }
}
