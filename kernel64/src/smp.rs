//! More than one CPU: the decisions, separate from the machine.
//!
//! A PC starts with one CPU running - the "boot CPU" - and the others
//! switched on but doing nothing, each waiting to be told where to
//! begin. Telling them is done through the local APIC, by sending two
//! kinds of message to another CPU's APIC id:
//!
//!   INIT      "reset yourself, and wait"
//!   STARTUP   "begin executing at this page" - in 16-bit real mode, so
//!             the page must be below 1 MiB, and the message has room
//!             for nothing but its number
//!
//! What is here is what can be decided without touching an APIC, and so
//! can be tested on the host: which CPUs to start and what to call them,
//! and what the two messages look like.

use alloc::vec::Vec;

use crate::acpi::Processor;

/// The most CPUs the kernel will use. Each one costs a stack and its
/// own descriptor tables; more than this are left asleep, and said so.
pub const MAX_CPUS: usize = 16;

/// One CPU the kernel will use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cpu {
    /// The kernel's own number for it: 0 is the boot CPU, the others
    /// count up in the order the firmware lists them.
    pub index: usize,
    /// The hardware's name for it: where messages to it are sent.
    pub apic_id: u8,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    /// The boot CPU first, then the ones to start.
    pub cpus: Vec<Cpu>,
    /// CPUs that could have been started but are over the limit.
    pub left_asleep: usize,
}

/// Decides which CPUs to use, from the firmware's list.
///
/// The boot CPU is always first, whether or not the firmware lists it -
/// it is running, so it exists. A CPU the firmware marks as not enabled
/// is not there to start. A CPU listed twice is started once: firmware
/// tables are somebody else's code, and starting a CPU that is already
/// running would reset it under the kernel's feet.
pub fn plan(processors: &[Processor], boot_apic_id: u8, max: usize) -> Plan {
    let mut cpus = Vec::new();
    cpus.push(Cpu { index: 0, apic_id: boot_apic_id });
    let mut left_asleep = 0;
    for processor in processors {
        if !processor.enabled || cpus.iter().any(|cpu| cpu.apic_id == processor.apic_id) {
            continue;
        }
        if cpus.len() < max.max(1) {
            cpus.push(Cpu { index: cpus.len(), apic_id: processor.apic_id });
        } else {
            left_asleep += 1;
        }
    }
    Plan { cpus, left_asleep }
}

// What goes in the low half of the APIC's interrupt command register.
// Bits 8-10 are the kind of message; bit 14 ("level: assert") must be
// set for every kind but one that only CPUs of the 1990s needed.
const KIND_INIT: u32 = 0b101 << 8;
const KIND_STARTUP: u32 = 0b110 << 8;
const ASSERT: u32 = 1 << 14;

/// The INIT message.
pub const fn init_message() -> u32 {
    KIND_INIT | ASSERT
}

/// The STARTUP message that starts a CPU at physical address `page`.
/// None if a CPU cannot be started there: the message carries the page
/// *number* in eight bits, so the address must be a whole page below
/// 1 MiB. Page 0 is refused too: it holds the firmware's own tables.
pub const fn startup_message(page: u64) -> Option<u32> {
    if page == 0 || page & 0xFFF != 0 || page >= 1 << 20 {
        return None;
    }
    Some(KIND_STARTUP | ASSERT | (page >> 12) as u32)
}

/// "Everyone but me": bits 18-19 of the command register name a set of
/// CPUs instead of one, and the destination is then ignored.
const ALL_BUT_SELF: u32 = 0b11 << 18;
const KIND_NMI: u32 = 0b100 << 8;

/// A non-maskable interrupt to every other CPU: the one message a CPU
/// cannot refuse, whatever it is doing - even with interrupts off,
/// spinning on a lock that will never be released.
pub const fn nmi_to_all_others() -> u32 {
    KIND_NMI | ASSERT | ALL_BUT_SELF
}

/// An ordinary interrupt, raised on another CPU as `vector`: delivery
/// kind 000, "fixed". Unlike the NMI it waits until that CPU has
/// interrupts on, and its handler returns.
pub const fn interrupt_message(vector: u8) -> u32 {
    ASSERT | vector as u32
}

/// What goes in the high half of the command register: who the message
/// is for.
pub const fn destination(apic_id: u8) -> u32 {
    (apic_id as u32) << 24
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(ids: &[(u8, bool)]) -> Vec<Processor> {
        ids.iter().map(|&(apic_id, enabled)| Processor { apic_id, enabled }).collect()
    }

    fn ids(plan: &Plan) -> Vec<(usize, u8)> {
        plan.cpus.iter().map(|cpu| (cpu.index, cpu.apic_id)).collect()
    }

    #[test]
    fn the_boot_cpu_is_number_0_and_the_rest_follow_in_the_firmwares_order() {
        let plan = plan(&listed(&[(0, true), (1, true), (2, true), (3, true)]), 0, MAX_CPUS);
        assert_eq!(ids(&plan), vec![(0, 0), (1, 1), (2, 2), (3, 3)]);
        assert_eq!(plan.left_asleep, 0);
    }

    #[test]
    fn the_boot_cpu_need_not_be_the_first_listed_or_have_id_0() {
        let plan = plan(&listed(&[(4, true), (6, true), (2, true)]), 6, MAX_CPUS);
        assert_eq!(ids(&plan), vec![(0, 6), (1, 4), (2, 2)]);
    }

    #[test]
    fn one_cpu_is_a_plan_of_one() {
        let plan = plan(&listed(&[(0, true)]), 0, MAX_CPUS);
        assert_eq!(ids(&plan), vec![(0, 0)]);
    }

    #[test]
    fn a_boot_cpu_the_firmware_forgot_is_still_there() {
        // No usable MADT at all: the kernel assumes a standard PC.
        let plan = plan(&[], 0, MAX_CPUS);
        assert_eq!(ids(&plan), vec![(0, 0)]);
        // Or a list without it.
        let plan = super::plan(&listed(&[(1, true)]), 0, MAX_CPUS);
        assert_eq!(ids(&plan), vec![(0, 0), (1, 1)]);
    }

    #[test]
    fn cpus_that_are_not_enabled_are_not_started() {
        let plan = plan(&listed(&[(0, true), (1, false), (2, true), (3, false)]), 0, MAX_CPUS);
        assert_eq!(ids(&plan), vec![(0, 0), (1, 2)]);
        assert_eq!(plan.left_asleep, 0, "a CPU that is not there was not left out");
    }

    #[test]
    fn a_cpu_listed_twice_is_started_once() {
        let plan = plan(&listed(&[(0, true), (1, true), (1, true), (0, true)]), 0, MAX_CPUS);
        assert_eq!(ids(&plan), vec![(0, 0), (1, 1)]);
    }

    #[test]
    fn cpus_over_the_limit_are_counted_and_left_asleep() {
        let many: Vec<(u8, bool)> = (0..40).map(|id| (id, true)).collect();
        let plan = plan(&listed(&many), 0, MAX_CPUS);
        assert_eq!(plan.cpus.len(), MAX_CPUS);
        assert_eq!(plan.left_asleep, 40 - MAX_CPUS);
        // Numbers have no holes.
        for (n, cpu) in plan.cpus.iter().enumerate() {
            assert_eq!(cpu.index, n);
        }
        // A limit of nothing still leaves the CPU that is running.
        let plan = super::plan(&listed(&many), 0, 0);
        assert_eq!(plan.cpus.len(), 1);
        assert_eq!(plan.left_asleep, 39);
    }

    #[test]
    fn the_messages_are_the_ones_the_manual_gives() {
        // Intel's manual, volume 3, "MP initialization": 000C4500H to
        // every other CPU, then 000C46XXH. Those have "all excluding
        // self" in bits 18-19 (the C); sent to one CPU, that is 0.
        assert_eq!(init_message(), 0x4500);
        assert_eq!(startup_message(0x8000), Some(0x4608));
        assert_eq!(startup_message(0x1000), Some(0x4601));
        assert_eq!(startup_message(0x9F000), Some(0x469F));
        assert_eq!(destination(3), 0x0300_0000);
        assert_eq!(destination(255), 0xFF00_0000);
        // Delivery mode 100 (NMI), level assert, shorthand 11.
        assert_eq!(nmi_to_all_others(), 0x000C_4400);
        // Delivery mode 000 (fixed): only the vector and the level.
        assert_eq!(interrupt_message(0xF0), 0x40F0);
        assert_eq!(interrupt_message(0x20), 0x4020);
    }

    #[test]
    fn a_cpu_cannot_be_started_just_anywhere() {
        assert_eq!(startup_message(0), None, "page 0 is the firmware's");
        assert_eq!(startup_message(0x8001), None, "not a whole page");
        assert_eq!(startup_message(0x10_0000), None, "1 MiB: real mode cannot reach it");
        assert_eq!(startup_message(0xFFFF_F000), None);
        assert_eq!(startup_message(0xFF000), Some(0x46FF), "the last page below 1 MiB");
    }
}
