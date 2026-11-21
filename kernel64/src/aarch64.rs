//! AArch64: the numbers the architecture defines, and how to read them.
//!
//! Nothing here touches a register. It is the part of the AArch64 port
//! that is arithmetic - which bits make a page-table entry, what the
//! CPU's report of an exception means - kept in the library so that it
//! is tested on the host like everything else here. The code that puts
//! these numbers into registers is in `arch/aarch64/`.
//!
//! Words used below:
//!
//! - **exception level (EL)**: the privilege the CPU runs at. EL0 is
//!   for programs, EL1 for a kernel, EL2 for a hypervisor, EL3 for
//!   firmware. (x86 counts the other way: ring 0 is the kernel.)
//! - **system register**: a register that configures the CPU, read and
//!   written with the `mrs` and `msr` instructions. Each has a name
//!   ending in the level it belongs to: `TCR_EL1`, `ESR_EL1`.
//! - **translation table**: AArch64's name for a page table.

/// Where the kernel's addresses begin: the top 2 GiB of the address
/// space, the same as on x86_64. The linker script repeats it.
pub const KERNEL_WINDOW: u64 = 0xFFFF_FFFF_8000_0000;

/// How far into a 2 MiB step the image is loaded, and linked. Half a
/// megabyte is what every AArch64 loader has always used (it is where
/// a Raspberry Pi's firmware puts a kernel too).
pub const TEXT_OFFSET: u64 = 0x8_0000;

/// The top 1 GiB of the address space, kept for devices the kernel
/// must reach before it has a memory manager: 512 steps of 2 MiB.
pub const DEVICE_WINDOW: u64 = 0xFFFF_FFFF_C000_0000;

/// One 2 MiB step: what one entry of a level 2 table maps.
pub const BLOCK_SIZE: u64 = 2 * 1024 * 1024;

/// Which entry of the table at `level` (0 to 3) translates `address`,
/// with 4 KiB pages: nine bits of the address per level, the lowest
/// twelve being the offset inside the page.
pub const fn index(level: u32, address: u64) -> usize {
    ((address >> (39 - 9 * level)) & 0x1FF) as usize
}

/// The bits of a translation table entry.
pub mod descriptor {
    /// An entry that points at the next level's table.
    pub const TABLE: u64 = 0b11;
    /// An entry at level 1 or 2 that maps memory itself: 1 GiB or 2 MiB
    /// in one step, with no table below it.
    pub const BLOCK: u64 = 0b01;
    /// Which of the kinds of memory in `MAIR` this is: ordinary.
    pub const NORMAL: u64 = 0 << 2;
    /// ...or a device's registers.
    pub const DEVICE: u64 = 1 << 2;
    /// Other CPUs may hold this memory in their caches too.
    pub const SHARED: u64 = 0b11 << 8;
    /// "Has been used." With it clear the first use is a fault, which
    /// is how a kernel can learn what is used; this one sets it at once.
    pub const ACCESSED: u64 = 1 << 10;
    /// The kernel may not execute this.
    pub const NO_KERNEL_EXECUTE: u64 = 1 << 53;
    /// Programs may not execute this.
    pub const NO_USER_EXECUTE: u64 = 1 << 54;

    /// Ordinary memory for the kernel: read, write and - until the
    /// kernel makes tables of its own - execute.
    pub const NORMAL_BLOCK: u64 = BLOCK | NORMAL | SHARED | ACCESSED | NO_USER_EXECUTE;
    /// A device's registers: never executed, never cached.
    pub const DEVICE_BLOCK: u64 = BLOCK | DEVICE | ACCESSED | NO_USER_EXECUTE | NO_KERNEL_EXECUTE;
}

/// `MAIR_EL1`: the kinds of memory a descriptor chooses between, one
/// byte each. Kind 0 (`0xFF`) is ordinary memory, cached for reading
/// and writing; kind 1 (`0x00`) is a device: every access happens, in
/// order, once; kind 2 (`0x44`) is ordinary memory that is not cached,
/// for a screen's pixels, which should reach the screen. The
/// page-table code (`paging.rs`) chooses between them by these numbers.
pub const MEMORY_KINDS: u64 = 0x0044_00FF;

/// `SCTLR_EL1` ("system control") with the MMU off: only the bits the
/// architecture says must be 1.
pub const CONTROL_MMU_OFF: u64 = 0x30D0_0800;

/// ...and with it on: the MMU (bit 0), the data cache (2), a check that
/// the stack pointer is 16-byte aligned whenever it is used (3), and
/// the instruction cache (12).
pub const CONTROL_MMU_ON: u64 = CONTROL_MMU_OFF | 1 | 1 << 2 | 1 << 3 | 1 << 12;

/// `TCR_EL1` ("translation control"): how addresses are translated.
///
/// Each half of the address space gets 48 bits (64 - 16), 4 KiB pages,
/// and tables that are read through the cache. `physical_range` is what
/// the CPU reports it can address (`ID_AA64MMFR0_EL1`, bits 0-3: 0 is
/// 32 bits, 5 is 48); the kernel passes it straight back, and never
/// more than 5, since 48 bits is all these tables can express.
pub const fn translation_control(physical_range: u64) -> u64 {
    let range = if physical_range > 5 { 5 } else { physical_range };
    let size = 16; // 64 - 48
    let low = size | 1 << 8 | 1 << 10 | 0b11 << 12; // TTBR0: size, cached, shared, 4 KiB
    let high = size << 16 | 1 << 24 | 1 << 26 | 0b11 << 28 | 0b10 << 30; // TTBR1 likewise
    low | high | range << 32
}

/// How many bits of physical address a `physical_range` code means.
pub const fn physical_bits(physical_range: u64) -> u32 {
    match physical_range {
        0 => 32,
        1 => 36,
        2 => 40,
        3 => 42,
        4 => 44,
        5 => 48,
        _ => 52,
    }
}

/// What `ESR_EL1` (the "exception syndrome register") says about an
/// exception: the CPU's own account of why it stopped what it was
/// doing. The top six bits are the *class* - the kind of thing that
/// happened - and the low 25 are details whose meaning depends on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Syndrome(pub u64);

/// What a data or instruction abort was trying to do, and why it could
/// not. ("Abort" is AArch64's word for what x86 calls a page fault.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Abort {
    /// From a program (EL0) rather than the kernel.
    pub from_user: bool,
    pub access: Access,
    pub cause: Cause,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
    Execute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    /// No table maps the address; the walk stopped at this level.
    NotMapped { level: u8 },
    /// Mapped, but not for this kind of access.
    NotAllowed { level: u8 },
    /// Mapped, but its "accessed" bit is clear.
    NotYetAccessed { level: u8 },
    /// The address has more bits than the tables are set up for.
    AddressTooLarge { level: u8 },
    /// Anything else, by its number.
    Other(u8),
}

impl Syndrome {
    pub const BREAKPOINT: u8 = 0x3C;
    pub const SYSTEM_CALL: u8 = 0x15;

    /// The exception class: bits 26 to 31.
    pub const fn class(self) -> u8 {
        ((self.0 >> 26) & 0x3F) as u8
    }

    /// The details: bits 0 to 24.
    pub const fn details(self) -> u32 {
        (self.0 & 0x01FF_FFFF) as u32
    }

    pub const fn class_name(self) -> &'static str {
        match self.class() {
            0x00 => "unknown reason",
            0x01 => "trapped wait instruction",
            0x07 => "floating point used while it is switched off",
            0x0E => "illegal execution state",
            0x15 => "system call",
            0x18 => "trapped system register access",
            0x20 => "instruction abort in a program",
            0x21 => "instruction abort",
            0x22 => "misaligned program counter",
            0x24 => "data abort in a program",
            0x25 => "data abort",
            0x26 => "misaligned stack pointer",
            0x2F => "system error",
            0x3C => "breakpoint",
            _ => "exception",
        }
    }

    /// The reading of an abort (classes 0x20, 0x21, 0x24, 0x25), or
    /// None for any other class.
    pub const fn abort(self) -> Option<Abort> {
        let class = self.class();
        let instruction = match class {
            0x20 | 0x21 => true,
            0x24 | 0x25 => false,
            _ => return None,
        };
        let access = if instruction {
            Access::Execute
        } else if self.0 & (1 << 6) != 0 {
            // "WnR": write, not read.
            Access::Write
        } else {
            Access::Read
        };
        // The fault status code: its top four bits say what, and - for
        // the faults that come from walking the tables - the low two
        // say at which level.
        let status = (self.0 & 0x3F) as u8;
        let level = status & 0b11;
        let cause = match status >> 2 {
            0b0000 => Cause::AddressTooLarge { level },
            0b0001 => Cause::NotMapped { level },
            0b0010 => Cause::NotYetAccessed { level },
            0b0011 => Cause::NotAllowed { level },
            _ => Cause::Other(status),
        };
        Some(Abort { from_user: class == 0x20 || class == 0x24, access, cause })
    }
}

impl core::fmt::Display for Abort {
    /// "kernel read", "user write" - the same words as on x86_64.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let who = if self.from_user { "user" } else { "kernel" };
        let access = match self.access {
            Access::Read => "read",
            Access::Write => "write",
            Access::Execute => "instruction fetch",
        };
        write!(f, "{who} {access}")
    }
}

impl core::fmt::Display for Cause {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Cause::NotMapped { level } => write!(f, "not mapped (the level {level} table has no entry)"),
            Cause::NotAllowed { level } => write!(f, "protection violation (at level {level})"),
            Cause::NotYetAccessed { level } => write!(f, "accessed bit clear (at level {level})"),
            Cause::AddressTooLarge { level } => write!(f, "address too large (at level {level})"),
            Cause::Other(status) => write!(f, "fault status {status:#x}"),
        }
    }
}

/// The names of the sixteen entries of the exception vector table, in
/// the table's order: four sources, and four kinds from each.
pub const fn vector_name(vector: u64) -> (&'static str, &'static str) {
    let kind = match vector % 4 {
        0 => "synchronous exception",
        1 => "interrupt (IRQ)",
        2 => "fast interrupt (FIQ)",
        _ => "system error",
    };
    let source = match vector / 4 {
        0 => "the kernel, on the program stack pointer",
        1 => "the kernel",
        2 => "a 64-bit program",
        _ => "a 32-bit program",
    };
    (kind, source)
}

/// The `MIDR_EL1` register: who designed this CPU, and which one it is.
pub const fn cpu_name(midr: u64) -> &'static str {
    let implementer = (midr >> 24) & 0xFF;
    let part = (midr >> 4) & 0xFFF;
    match (implementer, part) {
        (0x41, 0xD03) => "Arm Cortex-A53",
        (0x41, 0xD07) => "Arm Cortex-A57",
        (0x41, 0xD08) => "Arm Cortex-A72",
        (0x41, 0xD0B) => "Arm Cortex-A76",
        (0x41, 0xD40) => "Arm Neoverse V1",
        (0x41, _) => "an Arm design",
        (0x61, _) => "an Apple design",
        (0x51, _) => "a Qualcomm design",
        (0x00, 0x051) => "QEMU's own (\"max\")",
        _ => "an unknown design",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kernel_window_is_one_entry_of_the_top_tables() {
        let first = KERNEL_WINDOW + TEXT_OFFSET;
        assert_eq!(index(0, first), 511);
        assert_eq!(index(1, first), 510);
        assert_eq!(index(2, first), 0);
        assert_eq!(index(3, first), 0x80);
        // The whole window, to its last byte, is under the same level 1 entry.
        assert_eq!(index(1, KERNEL_WINDOW + (1 << 30) - 1), 510);
        assert_eq!(index(2, KERNEL_WINDOW + (1 << 30) - 1), 511);
    }

    #[test]
    fn the_device_window_is_the_entry_after_it() {
        assert_eq!(index(0, DEVICE_WINDOW), 511);
        assert_eq!(index(1, DEVICE_WINDOW), 511);
        assert_eq!(index(2, DEVICE_WINDOW + 3 * BLOCK_SIZE), 3);
        assert_eq!(DEVICE_WINDOW, KERNEL_WINDOW + (1 << 30));
    }

    #[test]
    fn a_low_address_uses_the_first_entry_at_the_top() {
        // QEMU's virt machine: memory starts at 1 GiB.
        assert_eq!(index(0, 0x4008_0000), 0);
        assert_eq!(index(1, 0x4008_0000), 1);
    }

    #[test]
    fn block_descriptors_have_the_bits_the_manual_asks_for() {
        assert_eq!(descriptor::NORMAL_BLOCK, 0x0040_0000_0000_0701);
        assert_eq!(descriptor::DEVICE_BLOCK, 0x0060_0000_0000_0405);
        // Neither sets a bit an address would occupy (12 to 47).
        assert_eq!(descriptor::NORMAL_BLOCK & 0x0000_FFFF_FFFF_F000, 0);
        assert_eq!(descriptor::DEVICE_BLOCK & 0x0000_FFFF_FFFF_F000, 0);
    }

    #[test]
    fn translation_control_for_a_cortex_a72() {
        // 44 bits of physical address is code 4.
        let tcr = translation_control(4);
        assert_eq!(tcr & 0x3F, 16, "the low half is 48 bits");
        assert_eq!((tcr >> 16) & 0x3F, 16, "the high half is 48 bits");
        assert_eq!((tcr >> 14) & 0b11, 0b00, "4 KiB pages below");
        assert_eq!((tcr >> 30) & 0b11, 0b10, "4 KiB pages above (a different code)");
        assert_eq!((tcr >> 32) & 0b111, 4);
        assert_eq!(tcr, 0x4_B510_3510);
    }

    #[test]
    fn a_cpu_with_more_physical_bits_than_the_tables_gets_48() {
        assert_eq!((translation_control(6) >> 32) & 0b111, 5);
        assert_eq!(physical_bits(4), 44);
        assert_eq!(physical_bits(6), 52);
    }

    #[test]
    fn control_values() {
        assert_eq!(CONTROL_MMU_ON, 0x30D0_180D);
        assert_eq!(CONTROL_MMU_ON & 1, 1);
        assert_eq!(CONTROL_MMU_OFF & 1, 0);
    }

    #[test]
    fn a_breakpoint_is_recognised() {
        // `brk #0`, as the CPU reports it.
        let syndrome = Syndrome(0xF200_0000);
        assert_eq!(syndrome.class(), Syndrome::BREAKPOINT);
        assert_eq!(syndrome.class_name(), "breakpoint");
        assert_eq!(syndrome.details(), 0);
        assert_eq!(syndrome.abort(), None);
        // `brk #7`: the number is in the details.
        assert_eq!(Syndrome(0xF200_0007).details(), 7);
    }

    #[test]
    fn a_kernel_read_of_unmapped_memory() {
        let abort = Syndrome(0x9600_0005).abort().expect("class 0x25 is an abort");
        assert_eq!(abort, Abort { from_user: false, access: Access::Read, cause: Cause::NotMapped { level: 1 } });
        assert_eq!(format!("{abort}"), "kernel read");
        assert_eq!(format!("{}", abort.cause), "not mapped (the level 1 table has no entry)");
    }

    #[test]
    fn a_kernel_write_that_is_not_allowed() {
        let abort = Syndrome(0x9600_004F).abort().unwrap();
        assert_eq!(abort.access, Access::Write);
        assert_eq!(abort.cause, Cause::NotAllowed { level: 3 });
        assert_eq!(format!("{abort}"), "kernel write");
    }

    #[test]
    fn aborts_from_programs_and_from_fetching() {
        let user = Syndrome(0x9200_0046).abort().unwrap(); // class 0x24
        assert!(user.from_user);
        assert_eq!(format!("{user}"), "user write");

        let fetch = Syndrome(0x8600_000F).abort().unwrap(); // class 0x21
        assert_eq!(fetch.access, Access::Execute);
        assert_eq!(format!("{fetch}"), "kernel instruction fetch");
        // An instruction abort has no "write" bit; bit 6 means nothing.
        assert_eq!(Syndrome(0x8600_004F).abort().unwrap().access, Access::Execute);

        assert_eq!(Syndrome(0x9600_0010).abort().unwrap().cause, Cause::Other(0x10));
        assert_eq!(Syndrome(0x9600_0000).abort().unwrap().cause, Cause::AddressTooLarge { level: 0 });
        assert_eq!(Syndrome(0x9600_000A).abort().unwrap().cause, Cause::NotYetAccessed { level: 2 });
    }

    #[test]
    fn every_vector_has_a_name() {
        assert_eq!(vector_name(4), ("synchronous exception", "the kernel"));
        assert_eq!(vector_name(5).0, "interrupt (IRQ)");
        assert_eq!(vector_name(8), ("synchronous exception", "a 64-bit program"));
        assert_eq!(vector_name(15), ("system error", "a 32-bit program"));
        assert_eq!(vector_name(0).1, "the kernel, on the program stack pointer");
    }

    #[test]
    fn cpus_are_named() {
        assert_eq!(cpu_name(0x410F_D083), "Arm Cortex-A72");
        assert_eq!(cpu_name(0x410F_D034), "Arm Cortex-A53");
        assert_eq!(cpu_name(0x4100_0000), "an Arm design");
        assert_eq!(cpu_name(0x000F_0510), "QEMU's own (\"max\")");
        assert_eq!(cpu_name(0x7700_0000), "an unknown design");
    }
}
