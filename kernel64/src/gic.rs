//! Arm's interrupt controller, the "GIC" - the part of it that is
//! arithmetic.
//!
//! A device does not interrupt a CPU directly. Its wire goes to the
//! interrupt controller, which knows for each wire whether it is
//! enabled, how urgent it is and which CPU should hear of it, and
//! raises the CPU's one interrupt input. The CPU then asks the
//! controller "which?", handles it, and says "done".
//!
//! Interrupts have numbers:
//!
//! ```text
//!      0 - 15    "SGI": one CPU poking another (software generated)
//!     16 - 31    "PPI": private to each CPU - its timer is one
//!     32 - 1019  "SPI": shared - devices
//!   1020 - 1023  not interrupts: answers meaning "nothing, after all"
//! ```
//!
//! The controller has a part shared by all CPUs (the "distributor")
//! and a part per CPU. Two versions are in use and this kernel speaks
//! both:
//!
//!   - version 2: the per-CPU part is a block of registers in memory,
//!     the same address showing each CPU its own;
//!   - version 3: the per-CPU part is CPU registers (`ICC_*`), plus a
//!     "redistributor" in memory for each CPU, where its private
//!     interrupts are enabled.
//!
//! This file is where registers are and what their bits mean, so that
//! it can be tested on the host. Touching them is `arch/aarch64/gic.rs`.

/// The first number that is not an interrupt.
pub const FIRST_SPECIAL: u32 = 1020;

/// The distributor's registers (both versions), as offsets.
pub mod distributor {
    /// On and off.
    pub const CONTROL: usize = 0x000;
    /// What this controller has: bits 0-4 say how many interrupts.
    pub const TYPE: usize = 0x004;
    /// One bit per interrupt: which group (0 or 1) it belongs to.
    pub const GROUP: usize = 0x080;
    /// One bit per interrupt: writing 1 enables it.
    pub const SET_ENABLE: usize = 0x100;
    /// One bit per interrupt: writing 1 disables it.
    pub const CLEAR_ENABLE: usize = 0x180;
    /// One byte per interrupt: its priority. Lower is more urgent.
    pub const PRIORITY: usize = 0x400;
    /// Version 2 only - one byte per interrupt: a bit for each CPU
    /// that may be given it.
    pub const TARGETS: usize = 0x800;
    /// Two bits per interrupt: how its line says "now" (see `trigger`).
    pub const CONFIG: usize = 0xC00;
    /// Version 3 only - eight bytes per shared interrupt: the name of
    /// the CPU it goes to, in the form of `route`.
    pub const ROUTE: usize = 0x6000;

    /// Version 2 only: writing here sends a software interrupt to
    /// other CPUs (see `software_v2`).
    pub const SOFTWARE: usize = 0xF00;

    /// Version 2: forward interrupts to the CPUs.
    pub const ENABLE: u32 = 1;
    /// Version 3: forward group 1 interrupts.
    pub const ENABLE_GROUP_1: u32 = 1 << 1;
    /// Version 3: "affinity routing" - CPUs are named by their
    /// `MPIDR` rather than by a bit in a byte. Version 3's own way.
    pub const AFFINITY_ROUTING: u32 = 1 << 4;
    /// Version 3: a write to this register is still taking effect.
    pub const WRITE_PENDING: u32 = 1 << 31;
}

/// Version 2's per-CPU registers.
pub mod cpu_interface {
    pub const CONTROL: usize = 0x00;
    /// Only interrupts more urgent than this reach the CPU.
    pub const PRIORITY_MASK: usize = 0x04;
    /// Reading it is the question "which interrupt?" - and takes it.
    pub const ACKNOWLEDGE: usize = 0x0C;
    /// Writing an interrupt's number here says it has been handled.
    pub const END: usize = 0x10;

    pub const ENABLE: u32 = 1;
}

/// Version 3's per-CPU part in memory. Each CPU has two 64 KiB frames
/// side by side: one about the CPU itself, one for its SGIs and PPIs
/// (laid out like the start of a distributor).
pub mod redistributor {
    /// What this redistributor is: bit 4 says it is the last one, and
    /// bits 32-63 name its CPU.
    pub const TYPE: usize = 0x008;
    /// Whether the CPU is "asleep" to the controller.
    pub const WAKE: usize = 0x014;
    /// The second frame begins here.
    pub const PRIVATE: usize = 0x1_0000;
    /// From one CPU's frames to the next's.
    pub const STRIDE: usize = 0x2_0000;

    pub const LAST: u64 = 1 << 4;
    /// WAKE: the kernel says the CPU is asleep...
    pub const PROCESSOR_SLEEP: u32 = 1 << 1;
    /// ...and the controller agrees.
    pub const CHILDREN_ASLEEP: u32 = 1 << 2;
}

/// The priority given to every interrupt this kernel enables: in the
/// middle, so that the mask register can let it through.
pub const PRIORITY: u8 = 0xA0;
/// A priority mask that lets every priority through.
pub const ANY_PRIORITY: u32 = 0xFF;

/// Where interrupt `id`'s bit is among registers of one bit each
/// starting at `base`: the register's offset, and the bit as a mask.
pub fn bit(base: usize, id: u32) -> (usize, u32) {
    (base + (id as usize / 32) * 4, 1 << (id % 32))
}

/// Where interrupt `id` says how its line is to be read: the
/// register's offset, and the bit that means "edge".
///
/// A device asks for attention by raising a wire. The controller can
/// read the wire two ways. *Level*: the interrupt is wanted for as
/// long as the wire is up - so it must be kept from arriving again
/// until someone has made the device lower it. *Edge*: the interrupt
/// is wanted once each time the wire goes up - the controller
/// remembers that it did, and a wire left up asks for nothing more.
/// Which one a device means is in the machine's description.
pub fn trigger(id: u32) -> (usize, u32) {
    (distributor::CONFIG + (id as usize / 16) * 4, 0b10 << ((id % 16) * 2))
}

/// The value of a shared interrupt's ROUTE register that sends it to
/// the CPU whose `MPIDR_EL1` is `mpidr`: the four parts of the CPU's
/// name, where that register has them (bits 0-23 and 32-39).
pub fn route(mpidr: u64) -> u64 {
    mpidr & 0x0000_00FF_00FF_FFFF
}

/// Where interrupt `id`'s byte is among registers of one byte each
/// starting at `base`.
pub fn byte(base: usize, id: u32) -> usize {
    base + id as usize
}

/// How many interrupt numbers the distributor has, from its TYPE
/// register: 32 for each step of the low five bits, at most 1020.
pub fn lines(type_register: u32) -> u32 {
    (32 * ((type_register & 0x1F) + 1)).min(FIRST_SPECIAL)
}

/// The interrupt a read of the acknowledge register reported - or None
/// if it was one of the answers that mean "nothing": the interrupt
/// went away, or another CPU took it. `version` 2 keeps the number in
/// ten bits (the next three say which CPU sent an SGI); version 3 in
/// twenty-four.
pub fn acknowledged(value: u32, version: u8) -> Option<u32> {
    let id = if version == 2 { value & 0x3FF } else { value & 0xFF_FFFF };
    // Only the four numbers 1020-1023 are special; version 3 has real
    // interrupts above them.
    (!(FIRST_SPECIAL..FIRST_SPECIAL + 4).contains(&id)).then_some(id)
}

/// What kind of interrupt a number is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// From another CPU.
    Software,
    /// This CPU's own.
    Private,
    /// A device's.
    Shared,
}

pub fn kind(id: u32) -> Kind {
    match id {
        0..=15 => Kind::Software,
        16..=31 => Kind::Private,
        _ => Kind::Shared,
    }
}

/// A CPU's name as a redistributor's TYPE register gives it, from the
/// CPU's own `MPIDR_EL1`: that register has the four parts of the name
/// in bits 0-23 and 32-39, the redistributor has them in bits 32-63.
pub fn affinity(mpidr: u64) -> u64 {
    ((mpidr >> 32) & 0xFF) << 24 | (mpidr & 0xFF_FFFF)
}

/// Whether the redistributor whose TYPE register reads `type_register`
/// is the one of the CPU whose `MPIDR_EL1` is `mpidr`.
pub fn is_redistributor_of(type_register: u64, mpidr: u64) -> bool {
    type_register >> 32 == affinity(mpidr)
}

// ---- one CPU interrupting another -----------------------------------------

/// Interrupts 0 to 15 are "software generated": no device raises
/// them, a CPU does, by writing a register, to interrupt another CPU.
/// The kernel uses two.
///
/// "Look at the queue": a thread was made ready that the CPU could
/// run, and it sits halted (sched.rs calls this a poke).
pub const SOFTWARE_RESCHEDULE: u32 = 0;
/// "Stop": the kernel is halting, on every CPU.
pub const SOFTWARE_STOP: u32 = 1;

/// Version 2: what to write to the distributor's SOFTWARE register to
/// send software interrupt `id` to the CPUs in `targets` - a bit for
/// each, the same bits as in a TARGETS byte.
pub fn software_v2(targets: u8, id: u32) -> u32 {
    u32::from(targets) << 16 | (id & 0xF)
}

/// Version 3: what to write to the CPU's own `ICC_SGI1R_EL1` register
/// to send software interrupt `id` to the CPU whose `MPIDR_EL1` is
/// `mpidr`. The register names a group of up to 16 CPUs by the upper
/// three parts of their name, and each one in it by a bit - so a CPU
/// whose lowest part is 16 or more cannot be named: None.
pub fn software_v3(mpidr: u64, id: u32) -> Option<u64> {
    let lowest = mpidr & 0xFF;
    if lowest >= 16 {
        return None;
    }
    let part = |shift: u32| (mpidr >> shift) & 0xFF;
    Some(part(32) << 48 | part(16) << 32 | u64::from(id & 0xF) << 24 | part(8) << 16 | 1 << lowest)
}

// ---- the timer -----------------------------------------------------------

/// How far the CPU's counter moves between two ticks, for a counter
/// running at `frequency` Hz and `hz` ticks a second. None if the
/// frequency is not known (0) or is too low for that many ticks.
pub fn tick_interval(frequency: u64, hz: u64) -> Option<u64> {
    if hz == 0 {
        return None;
    }
    match frequency / hz {
        0 => None,
        interval => Some(interval),
    }
}

/// The next time to interrupt after the one at `deadline`, when the
/// counter reads `now`: the next multiple of `interval` past
/// `deadline` that is still to come. Stepping from the deadline rather
/// than from `now` keeps ticks from drifting; skipping whole intervals
/// keeps a late kernel from being interrupted over and over to catch
/// up. Also returns how many ticks that was (at least 1).
pub fn next_deadline(deadline: u64, now: u64, interval: u64) -> (u64, u64) {
    let behind = now.saturating_sub(deadline) / interval;
    (deadline + (behind + 1) * interval, behind + 1)
}

/// A one-shot timer's interrupt, when the counter reads `now` and the
/// next tick was due at `next_tick` (K24, journal entry NNN): how many
/// ticks have come (0 if this is an early interrupt, for a sleeper),
/// and when the next tick is due. The timer is set again each time, for
/// the next tick or a sleeper before it, whichever is first.
pub fn after_interrupt(next_tick: u64, now: u64, interval: u64) -> (u64, u64) {
    if now < next_tick {
        (0, next_tick)
    } else {
        let (next, passed) = next_deadline(next_tick, now, interval);
        (passed, next)
    }
}

/// Counter steps as milliseconds.
pub fn milliseconds(steps: u64, frequency: u64) -> u64 {
    if frequency == 0 {
        return 0;
    }
    (u128::from(steps) * 1000 / u128::from(frequency)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_early_interrupt_is_no_tick_and_a_late_one_counts_every_tick_passed() {
        // Ticks due every 100 steps, the next at 1000.
        assert_eq!(after_interrupt(1000, 950, 100), (0, 1000), "early: a sleeper's");
        assert_eq!(after_interrupt(1000, 1000, 100), (1, 1100), "on time");
        assert_eq!(after_interrupt(1000, 1001, 100), (1, 1100));
        assert_eq!(after_interrupt(1000, 1250, 100), (3, 1300), "late by two and a half");
    }

    #[test]
    fn a_bit_per_interrupt() {
        // The timer, 27: first register, bit 27.
        assert_eq!(bit(distributor::SET_ENABLE, 27), (0x100, 1 << 27));
        // 32 is the first of the second register; 33 its neighbour.
        assert_eq!(bit(distributor::SET_ENABLE, 32), (0x104, 1));
        assert_eq!(bit(distributor::SET_ENABLE, 33), (0x104, 2));
        // The serial port of QEMU's virt machine, 33; the last there can be.
        assert_eq!(bit(distributor::CLEAR_ENABLE, 1019), (0x180 + 31 * 4, 1 << 27));
        assert_eq!(bit(distributor::GROUP, 0), (0x080, 1));
    }

    #[test]
    fn a_byte_per_interrupt() {
        assert_eq!(byte(distributor::PRIORITY, 0), 0x400);
        assert_eq!(byte(distributor::PRIORITY, 27), 0x41B);
        assert_eq!(byte(distributor::TARGETS, 33), 0x821);
    }

    #[test]
    fn how_many_lines() {
        assert_eq!(lines(0), 32);
        // QEMU's virt machine: 288 = 32 private and software + 256 shared.
        assert_eq!(lines(8), 288);
        // Other bits of the register are about other things.
        assert_eq!(lines(0x0000_FC28), 288);
        // The field's largest value would be 1024; four are not interrupts.
        assert_eq!(lines(0x1F), 1020);
    }

    #[test]
    fn an_acknowledgement_is_an_interrupt_or_nothing() {
        assert_eq!(acknowledged(27, 2), Some(27));
        assert_eq!(acknowledged(27, 3), Some(27));
        // Version 2: an SGI from CPU 3 has the CPU above the number.
        assert_eq!(acknowledged(3 << 10 | 5, 2), Some(5));
        assert_eq!(acknowledged(1019, 2), Some(1019));
        for nothing in 1020..=1023 {
            assert_eq!(acknowledged(nothing, 2), None);
            assert_eq!(acknowledged(nothing, 3), None);
        }
        // Version 3 has numbers above 1023 (for interrupts sent as
        // messages); they are interrupts.
        assert_eq!(acknowledged(8192, 3), Some(8192));
    }

    #[test]
    fn kinds() {
        assert_eq!(kind(0), Kind::Software);
        assert_eq!(kind(15), Kind::Software);
        assert_eq!(kind(16), Kind::Private);
        assert_eq!(kind(27), Kind::Private);
        assert_eq!(kind(31), Kind::Private);
        assert_eq!(kind(32), Kind::Shared);
    }

    #[test]
    fn a_cpu_finds_its_redistributor() {
        // CPU 2 of a simple machine: MPIDR has bit 31 always set.
        let mpidr = 0x8000_0002;
        assert_eq!(affinity(mpidr), 2);
        assert!(is_redistributor_of(2 << 32 | 0x10, mpidr));
        assert!(!is_redistributor_of(1 << 32, mpidr));
        // A name with all four parts: 4.3.2.1.
        let mpidr = 0x04_8003_0201;
        assert_eq!(affinity(mpidr), 0x0403_0201);
        assert!(is_redistributor_of(0x0403_0201 << 32, mpidr));
        // The low half of TYPE is not part of the name.
        assert!(is_redistributor_of(0x0403_0201 << 32 | 0xFFFF_FFFF, mpidr));
    }

    #[test]
    fn one_cpu_interrupts_another() {
        // Version 2: CPUs 1 and 3, interrupt 0; CPU 0, interrupt 1.
        assert_eq!(software_v2(0b1010, SOFTWARE_RESCHEDULE), 0x000A_0000);
        assert_eq!(software_v2(1, SOFTWARE_STOP), 0x0001_0001);
        // Version 3: CPU 2 of a simple machine.
        assert_eq!(software_v3(0x8000_0002, SOFTWARE_RESCHEDULE), Some(1 << 2));
        assert_eq!(software_v3(0x8000_0002, SOFTWARE_STOP), Some(1 << 24 | 1 << 2));
        // A name with all four parts, 4.3.2.1: each in its own place.
        assert_eq!(software_v3(0x04_8003_0201, 0), Some(4 << 48 | 3 << 32 | 2 << 16 | 1 << 1));
        // The sixteenth CPU of a group can be named, the seventeenth not.
        assert_eq!(software_v3(15, 0), Some(1 << 15));
        assert_eq!(software_v3(16, 0), None);
    }

    #[test]
    fn the_interval_between_ticks() {
        // QEMU's counter: 62.5 MHz. 100 ticks a second.
        assert_eq!(tick_interval(62_500_000, 100), Some(625_000));
        // A Raspberry Pi 4's: 54 MHz.
        assert_eq!(tick_interval(54_000_000, 100), Some(540_000));
        assert_eq!(tick_interval(0, 100), None);
        assert_eq!(tick_interval(50, 100), None);
        assert_eq!(tick_interval(1000, 0), None);
    }

    #[test]
    fn the_next_deadline_does_not_drift_and_does_not_pile_up() {
        // On time: handled a little after the deadline.
        assert_eq!(next_deadline(1000, 1003, 100), (1100, 1));
        // Exactly on it.
        assert_eq!(next_deadline(1000, 1000, 100), (1100, 1));
        // Handled early (cannot happen, but must not go backwards).
        assert_eq!(next_deadline(1000, 990, 100), (1100, 1));
        // Late by three and a half intervals: three ticks were missed,
        // and the next is at the next multiple - not 350 from now.
        assert_eq!(next_deadline(1000, 1350, 100), (1400, 4));
        // Late by exactly one interval: that tick is due now, so the
        // next deadline is the one after.
        assert_eq!(next_deadline(1000, 1100, 100), (1200, 2));
    }

    #[test]
    fn counter_steps_as_time() {
        assert_eq!(milliseconds(62_500_000, 62_500_000), 1000);
        assert_eq!(milliseconds(6_250_000, 62_500_000), 100);
        assert_eq!(milliseconds(624_999, 62_500_000), 9);
        assert_eq!(milliseconds(5, 0), 0);
        // Days of uptime must not overflow on the way.
        assert_eq!(milliseconds(u64::MAX, 1000), u64::MAX);
    }

    #[test]
    fn where_an_interrupt_says_how_its_line_is_read() {
        // Sixteen interrupts to a register, two bits each, the higher
        // of the two meaning "edge".
        assert_eq!(trigger(0), (0xC00, 0b10));
        assert_eq!(trigger(15), (0xC00, 0b10 << 30));
        assert_eq!(trigger(16), (0xC04, 0b10));
        assert_eq!(trigger(79), (0xC00 + 4 * 4, 0b10 << 30)); // the last virtio place of QEMU's machine
        assert_eq!(trigger(48), (0xC0C, 0b10));
    }

    #[test]
    fn a_shared_interrupt_is_routed_by_the_cpus_name() {
        // MPIDR has other bits set (31 always; 30 on a one-CPU machine).
        assert_eq!(route(0x8000_0000), 0);
        assert_eq!(route(0x0000_0000_8000_0003), 3);
        assert_eq!(route(0x0000_0001_C000_0102), 0x0000_0001_0000_0102);
        assert_eq!(distributor::ROUTE + 8 * 79, 0x6278);
    }
}
