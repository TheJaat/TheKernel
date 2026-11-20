//! The MMU's control registers: the few instructions that make page
//! tables take effect. The tables themselves are built by
//! `kernel64::paging`, which never touches hardware.

use core::arch::asm;
use core::arch::x86_64::__cpuid;

pub(super) const EFER: u32 = 0xC000_0080;
const EFER_NXE: u64 = 1 << 11;
const CR0_WP: u64 = 1 << 16;
const CR4_PGE: u64 = 1 << 7;

pub(super) unsafe fn read_msr(msr: u32) -> u64 {
    let (low, high): (u32, u32);
    asm!("rdmsr", in("ecx") msr, out("eax") low, out("edx") high, options(nomem, nostack, preserves_flags));
    (u64::from(high) << 32) | u64::from(low)
}

pub(super) unsafe fn write_msr(msr: u32, value: u64) {
    asm!("wrmsr", in("ecx") msr, in("eax") value as u32, in("edx") (value >> 32) as u32,
         options(nostack, preserves_flags));
}

/// Whether this CPU can mark pages no-execute (CPUID 0x80000001, EDX
/// bit 20). Every x86_64 CPU ever made can, but asking costs nothing.
pub fn supports_no_execute() -> bool {
    // Every x86_64 CPU has CPUID and extended leaf 0x80000001 - the
    // bootloader already used it to check for long mode.
    let leaf = __cpuid(0x8000_0001);
    leaf.edx & (1 << 20) != 0
}

/// Turns on the protections the kernel's page tables rely on:
///   - EFER.NXE: the NO_EXECUTE bit means something (if supported)
///   - CR0.WP: read-only pages are read-only for the kernel too. Without
///     it ring 0 can write anywhere, and a stray kernel write to its own
///     code would silently succeed.
///   - CR4.PGE: GLOBAL entries stay in the TLB across CR3 reloads.
///
/// # Safety
/// Must run before tables using NO_EXECUTE are loaded, and the current
/// tables must not rely on the kernel writing to read-only pages.
pub unsafe fn enable_protection(no_execute: bool) {
    if no_execute {
        write_msr(EFER, read_msr(EFER) | EFER_NXE);
    }
    let mut cr0: u64;
    asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack, preserves_flags));
    cr0 |= CR0_WP;
    asm!("mov cr0, {}", in(reg) cr0, options(nostack, preserves_flags));

    let mut cr4: u64;
    asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
    cr4 |= CR4_PGE;
    asm!("mov cr4, {}", in(reg) cr4, options(nostack, preserves_flags));
}

/// The control registers and EFER of the CPU this runs on - what a
/// CPU started later must be given to be set up the same way.
pub fn control() -> Control {
    let (cr0, cr4): (u64, u64);
    // SAFETY: reading control registers and an MSR every x86_64 CPU has
    // changes nothing.
    unsafe {
        asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack, preserves_flags));
        asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
        Control { cr0, cr4, efer: read_msr(EFER) }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Control {
    pub cr0: u64,
    pub cr4: u64,
    pub efer: u64,
}

/// Makes this CPU's CR0 and CR4 the same as another's (EFER the
/// start-up code has already set: paging needs it).
///
/// # Safety
/// `control` must come from `control()` on a CPU of the same machine,
/// already running the kernel: every bit in it is then one this code
/// runs correctly with.
pub unsafe fn set_control(control: Control) {
    asm!("mov cr4, {}", in(reg) control.cr4, options(nostack, preserves_flags));
    asm!("mov cr0, {}", in(reg) control.cr0, options(nostack, preserves_flags));
}

/// Switches to the address space whose PML4 is at physical `root`.
/// Loading CR3 also flushes every non-global TLB entry.
///
/// # Safety
/// The new tables must map everything the kernel uses from here on:
/// its code, its stack, and every static it touches.
pub unsafe fn activate(root: u64) {
    asm!("mov cr3, {}", in(reg) root, options(nostack, preserves_flags));
}

/// Drops the TLB's cached translation of the page containing `address`.
/// Needed after unmapping or changing a mapping; the CPU would otherwise
/// keep using the old one.
pub fn flush(address: u64) {
    // SAFETY: invlpg only discards a cached translation.
    unsafe { asm!("invlpg [{}]", in(reg) address, options(nostack, preserves_flags)) };
}
