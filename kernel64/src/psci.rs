//! PSCI: asking the firmware to start a CPU.
//!
//! On a PC the kernel starts the other CPUs itself, with messages
//! between interrupt controllers and a page of 16-bit code (smp.rs).
//! On AArch64 a kernel cannot: a CPU that is off is held by software
//! more privileged than any kernel - firmware at exception level 3, or
//! a hypervisor at level 2 - and only that software can let it go. Arm
//! wrote down one way of asking, the Power State Coordination
//! Interface, and every machine of the last ten years speaks it.
//!
//! Asking is a function call that crosses exception levels: a number
//! for the function in x0, arguments in x1 to x3, and one instruction -
//! `smc` ("secure monitor call", to level 3) or `hvc` ("hypervisor
//! call", to level 2). Which of the two, the device tree says. The
//! answer comes back in x0.
//!
//! This module is the part with no instruction in it: the numbers,
//! what an answer means, and which CPUs to ask for. The call itself is
//! in `arch/aarch64`.

use alloc::vec::Vec;

/// PSCI_VERSION: which version the firmware speaks.
pub const VERSION: u64 = 0x8400_0000;
/// CPU_ON, the 64-bit form: x1 names the CPU (its `MPIDR_EL1`, the
/// parts that are a name), x2 is the physical address it begins to run
/// at, x3 a number it finds in x0 when it does.
pub const CPU_ON: u64 = 0xC400_0003;

/// Why a CPU could not be switched on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The firmware has no such function.
    NotSupported,
    /// No CPU has that name, or the address is not one it may start at.
    InvalidParameters,
    Denied,
    /// It is running already.
    AlreadyOn,
    /// Someone asked before, and it has not started yet.
    OnPending,
    InternalFailure,
    /// A number this kernel does not know.
    Other(i64),
}

/// What CPU_ON answered. The answer is a 32-bit signed number, which
/// firmware may or may not have extended to 64 bits: only the low
/// half is looked at.
pub fn cpu_on_result(answer: u64) -> Result<(), Refusal> {
    match i64::from(answer as u32 as i32) {
        0 => Ok(()),
        -1 => Err(Refusal::NotSupported),
        -2 => Err(Refusal::InvalidParameters),
        -3 => Err(Refusal::Denied),
        -4 => Err(Refusal::AlreadyOn),
        -5 => Err(Refusal::OnPending),
        -6 => Err(Refusal::InternalFailure),
        other => Err(Refusal::Other(other)),
    }
}

/// What PSCI_VERSION answered, as (major, minor). None if the firmware
/// does not know the question - which is the answer of something that
/// is not PSCI at all.
pub fn version(answer: u64) -> Option<(u16, u16)> {
    let answer = answer as u32;
    if answer as i32 == -1 {
        return None;
    }
    Some(((answer >> 16) as u16, answer as u16))
}

/// The part of an `MPIDR_EL1` value that names the CPU: four numbers
/// of eight bits ("affinity levels" 3, 2, 1, 0), with other bits in
/// between that say things *about* the CPU and are not part of its
/// name. This is the form CPU_ON takes and the device tree gives.
pub fn name(mpidr: u64) -> u64 {
    mpidr & 0x0000_00FF_00FF_FFFF
}

#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    /// The names of the CPUs to start, in the order the device tree
    /// lists them. The boot CPU is not among them.
    pub start: Vec<u64>,
    /// CPUs that could have been started but are over the limit.
    pub left_asleep: usize,
}

/// Decides which CPUs to start: every one the device tree lists that
/// is not the boot CPU, each once, up to `max` CPUs in all.
///
/// The boot CPU is known by what it says of itself (`boot`, its
/// MPIDR), not by being listed first - a loader may start the kernel
/// on any of them.
pub fn plan(listed: &[u64], boot: u64, max: usize) -> Plan {
    let mut start: Vec<u64> = Vec::new();
    let mut left_asleep = 0;
    for &cpu in listed {
        let cpu = name(cpu);
        if cpu == name(boot) || start.contains(&cpu) {
            continue;
        }
        if start.len() + 1 < max.max(1) {
            start.push(cpu);
        } else {
            left_asleep += 1;
        }
    }
    Plan { start, left_asleep }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_of_cpu_on() {
        assert_eq!(cpu_on_result(0), Ok(()));
        // -4 as firmware writes it: in 32 bits, the upper half left zero...
        assert_eq!(cpu_on_result(0xFFFF_FFFC), Err(Refusal::AlreadyOn));
        // ...or extended to 64.
        assert_eq!(cpu_on_result(-4i64 as u64), Err(Refusal::AlreadyOn));
        assert_eq!(cpu_on_result(-1i64 as u64), Err(Refusal::NotSupported));
        assert_eq!(cpu_on_result(-2i64 as u64), Err(Refusal::InvalidParameters));
        assert_eq!(cpu_on_result(-3i64 as u64), Err(Refusal::Denied));
        assert_eq!(cpu_on_result(-5i64 as u64), Err(Refusal::OnPending));
        assert_eq!(cpu_on_result(-6i64 as u64), Err(Refusal::InternalFailure));
        assert_eq!(cpu_on_result(-9i64 as u64), Err(Refusal::Other(-9)));
        // Rubbish in the upper half does not turn a success into a failure.
        assert_eq!(cpu_on_result(0xDEAD_BEEF_0000_0000), Ok(()));
    }

    #[test]
    fn versions() {
        assert_eq!(version(0x0001_0001), Some((1, 1)));
        assert_eq!(version(0x0000_0002), Some((0, 2)));
        assert_eq!(version(0xFFFF_FFFF), None);
        assert_eq!(version(u64::MAX), None);
    }

    #[test]
    fn a_name_is_four_numbers() {
        // Bit 31 is always set, bit 30 says "the only CPU", bit 24
        // says how the lowest level is to be read: none is a name.
        assert_eq!(name(0x8000_0003), 3);
        assert_eq!(name(0xC100_0000), 0);
        assert_eq!(name(0x04_8103_0201), 0x04_0003_0201);
    }

    #[test]
    fn the_others_are_started_once_each() {
        // QEMU's four, booted on the first.
        assert_eq!(plan(&[0, 1, 2, 3], 0x8000_0000, 16), Plan { start: alloc::vec![1, 2, 3], left_asleep: 0 });
        // Booted on the third: it is the one left out.
        assert_eq!(plan(&[0, 1, 2, 3], 0x8000_0002, 16).start, [0, 1, 3]);
        // Listed twice: started once.
        assert_eq!(plan(&[0, 1, 1, 0, 2], 0, 16).start, [1, 2]);
        // Two clusters.
        assert_eq!(plan(&[0, 1, 0x100, 0x101], 0x100, 16).start, [0, 1, 0x101]);
    }

    #[test]
    fn no_more_than_the_limit() {
        let plan8 = plan(&[0, 1, 2, 3, 4, 5, 6, 7], 0, 4);
        assert_eq!(plan8, Plan { start: alloc::vec![1, 2, 3], left_asleep: 4 });
        // One CPU allowed, or none: the boot CPU is all there is.
        assert_eq!(plan(&[0, 1], 0, 1), Plan { start: alloc::vec![], left_asleep: 1 });
        assert_eq!(plan(&[0, 1], 0, 0), Plan { start: alloc::vec![], left_asleep: 1 });
        // A tree that lists nobody, or only the boot CPU.
        assert_eq!(plan(&[], 0, 16), Plan { start: alloc::vec![], left_asleep: 0 });
        assert_eq!(plan(&[0], 0x8000_0000, 16).start, []);
    }
}
