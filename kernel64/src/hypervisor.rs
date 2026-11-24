//! What a hypervisor says about the machine it pretends to be.
//!
//! A CPU running under a hypervisor sets CPUID leaf 1's ECX bit 31
//! ("hypervisor present"), and answers leaves from 0x4000_0000 with
//! the hypervisor's own information: leaf 0x4000_0000 gives the highest
//! such leaf (EAX) and a 12-byte name (EBX, ECX, EDX) - "VMwareVMware",
//! "KVMKVMKVM\0\0\0", "Microsoft Hv", "TCGTCGTCGTCG".
//!
//! Each hypervisor numbers the leaves after that its own way. VMware's
//! 0x4000_0010 is "timing information": the TSC's frequency (EAX) and
//! the local APIC timer's input clock (EBX), in kHz. KVM and QEMU give
//! the same leaf when asked to (`-cpu ...,vmware-cpuid-freq=on`).
//!
//! This module only decodes; the CPUID instruction itself is the
//! kernel's (`arch::x86_64::apic::stated_frequencies`), so the decoding
//! can be tested on the host with the answers real hypervisors give.

/// The leaf with the frequencies.
pub const TIMING_LEAF: u32 = 0x4000_0010;

/// CPUID leaf 1, ECX: running under a hypervisor.
pub const HYPERVISOR_PRESENT: u32 = 1 << 31;

/// The hypervisors known to mean "timing information" by leaf
/// 0x4000_0010.
const TIMING_VENDORS: [&[u8; 12]; 3] = [b"VMwareVMware", b"KVMKVMKVM\0\0\0", b"TCGTCGTCGTCG"];

/// One CPUID answer: EAX, EBX, ECX, EDX.
#[derive(Clone, Copy, Debug)]
pub struct Answer {
    pub eax: u32,
    pub ebx: u32,
    pub ecx: u32,
    pub edx: u32,
}

/// The hypervisor's name, from leaf 0x4000_0000's answer.
pub fn vendor(top: Answer) -> [u8; 12] {
    let mut name = [0u8; 12];
    name[0..4].copy_from_slice(&top.ebx.to_le_bytes());
    name[4..8].copy_from_slice(&top.ecx.to_le_bytes());
    name[8..12].copy_from_slice(&top.edx.to_le_bytes());
    name
}

/// Whether to ask leaf 0x4000_0010 at all: a hypervisor is there, it
/// is one that means timing by it, and it has that many leaves.
/// `leaf1_ecx` is leaf 1's ECX, `top` leaf 0x4000_0000's answer.
pub fn has_timing(leaf1_ecx: u32, top: Answer) -> bool {
    leaf1_ecx & HYPERVISOR_PRESENT != 0 && top.eax >= TIMING_LEAF && TIMING_VENDORS.contains(&&vendor(top))
}

/// (TSC Hz, APIC timer input clock Hz) from leaf 0x4000_0010's answer,
/// or None when they make no sense: a leaf of zeros (KVM without the
/// option), a TSC outside 10 MHz - 100 GHz, an APIC clock under 1 MHz
/// or not slower than the TSC.
pub fn timing(answer: Answer) -> Option<(u64, u64)> {
    let (tsc_khz, bus_khz) = (u64::from(answer.eax), u64::from(answer.ebx));
    let sane = (10_000..100_000_000).contains(&tsc_khz) && bus_khz >= 1_000 && bus_khz < tsc_khz;
    sane.then_some((tsc_khz * 1000, bus_khz * 1000))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &[u8; 12], highest: u32) -> Answer {
        let word = |n: usize| u32::from_le_bytes([name[n], name[n + 1], name[n + 2], name[n + 3]]);
        Answer { eax: highest, ebx: word(0), ecx: word(4), edx: word(8) }
    }

    #[test]
    fn vmware_is_believed() {
        // VMware Workstation on a 3.07 GHz host: highest leaf
        // 0x4000_0010, the bus clock VMware always states, 65 MHz.
        let top = named(b"VMwareVMware", 0x4000_0010);
        assert_eq!(&vendor(top), b"VMwareVMware");
        assert!(has_timing(HYPERVISOR_PRESENT, top));
        let leaf = Answer { eax: 3_075_000, ebx: 65_000, ecx: 0, edx: 0 };
        assert_eq!(timing(leaf), Some((3_075_000_000, 65_000_000)));
    }

    #[test]
    fn others_are_not_asked() {
        // No hypervisor bit; Hyper-V (its 0x4000_0010 means something
        // else); a VMware too old to have the leaf; KVM is fine.
        assert!(!has_timing(0, named(b"VMwareVMware", 0x4000_0010)));
        assert!(!has_timing(HYPERVISOR_PRESENT, named(b"Microsoft Hv", 0x4000_0010)));
        assert!(!has_timing(HYPERVISOR_PRESENT, named(b"VMwareVMware", 0x4000_0001)));
        assert!(has_timing(HYPERVISOR_PRESENT, named(b"KVMKVMKVM\0\0\0", 0x4000_0010)));
    }

    #[test]
    fn nonsense_is_refused() {
        let leaf = |eax, ebx| Answer { eax, ebx, ecx: 0, edx: 0 };
        assert_eq!(timing(leaf(0, 0)), None); // KVM without vmware-cpuid-freq
        assert_eq!(timing(leaf(5_000, 1_000)), None); // a 5 MHz TSC
        assert_eq!(timing(leaf(3_000_000, 500)), None); // a 0.5 MHz bus
        assert_eq!(timing(leaf(3_000_000, 3_000_000)), None); // bus = TSC
        assert_eq!(timing(leaf(2_000_000, 1_000_000)), Some((2_000_000_000, 1_000_000_000)));
    }
}
