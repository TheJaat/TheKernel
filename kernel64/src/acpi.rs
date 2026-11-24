//! ACPI tables: how the firmware describes the machine.
//!
//! The RSDP (which the loader found) points to a root table, the XSDT
//! (64-bit pointers) or, on ACPI 1.0, the RSDT (32-bit pointers). That
//! lists every other table by physical address. Each table starts with
//! the same 36-byte header - a 4-letter signature, its length, and a
//! checksum: all bytes of the table must sum to 0 (mod 256).
//!
//! For interrupts, the table that matters is the MADT (signature "APIC"):
//! the CPUs' local APICs, the I/O APICs, and how the old ISA IRQ numbers
//! map onto I/O APIC inputs ("interrupt source overrides" - on almost
//! every PC, the timer's IRQ 0 is wired to input 2, for instance).
//!
//! Firmware tables are input the kernel does not control, and buggy
//! firmware exists, so everything here is checked - signatures,
//! checksums, lengths - and a bad table is an error, not a crash. The
//! parsing reads through a `PhysicalMemory` so host tests can hand it
//! tables built in a buffer.

use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

/// Read access to physical memory.
pub trait PhysicalMemory {
    /// Fills `buffer` from physical `address`; false if it cannot.
    fn read(&self, address: u64, buffer: &mut [u8]) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpiError {
    Unreadable(u64),
    BadSignature(u64),
    BadChecksum(u64),
    BadLength(u64),
    NotFound([u8; 4]),
    /// A table this kernel reads, describing something it cannot use.
    Unsupported(&'static str),
}

impl fmt::Display for AcpiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AcpiError::Unreadable(a) => write!(f, "cannot read memory at {a:#x}"),
            AcpiError::BadSignature(a) => write!(f, "bad signature at {a:#x}"),
            AcpiError::BadChecksum(a) => write!(f, "bad checksum at {a:#x}"),
            AcpiError::BadLength(a) => write!(f, "implausible length at {a:#x}"),
            AcpiError::Unsupported(what) => write!(f, "{what}"),
            AcpiError::NotFound(s) => {
                write!(f, "no {} table", core::str::from_utf8(s).unwrap_or("????"))
            }
        }
    }
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

fn checksum_ok(bytes: &[u8]) -> bool {
    bytes.iter().fold(0u8, |sum, &b| sum.wrapping_add(b)) == 0
}

const HEADER_LENGTH: usize = 36;
/// No real table is anywhere near this; a bigger length is corruption.
const MAX_TABLE_LENGTH: u32 = 1 << 20;

/// A table read into memory, checksum verified.
pub struct Table {
    pub address: u64,
    pub bytes: Vec<u8>,
}

impl Table {
    pub fn signature(&self) -> [u8; 4] {
        self.bytes[0..4].try_into().unwrap()
    }
    /// The bytes after the common header.
    pub fn body(&self) -> &[u8] {
        &self.bytes[HEADER_LENGTH..]
    }
}

/// The table at `address`, checked: a plausible length, its checksum.
/// (For a table the root does not list, such as the DSDT.)
pub fn read_table(memory: &impl PhysicalMemory, address: u64) -> Result<Table, AcpiError> {
    let mut header = [0u8; HEADER_LENGTH];
    if !memory.read(address, &mut header) {
        return Err(AcpiError::Unreadable(address));
    }
    let length = u32_at(&header, 4);
    if !(HEADER_LENGTH as u32..=MAX_TABLE_LENGTH).contains(&length) {
        return Err(AcpiError::BadLength(address));
    }
    let mut bytes = vec![0u8; length as usize];
    if !memory.read(address, &mut bytes) {
        return Err(AcpiError::Unreadable(address));
    }
    if !checksum_ok(&bytes) {
        return Err(AcpiError::BadChecksum(address));
    }
    Ok(Table { address, bytes })
}

/// The root of the tables: every table's address, from the XSDT or RSDT.
pub struct Acpi {
    pub revision: u8,
    pub oem: [u8; 6],
    tables: Vec<u64>,
}

impl Acpi {
    /// Reads the RSDP at `rsdp` and the root table it points to.
    pub fn new(memory: &impl PhysicalMemory, rsdp: u64) -> Result<Acpi, AcpiError> {
        // The ACPI 1.0 RSDP is 20 bytes; 2.0 extends it to 36.
        let mut bytes = [0u8; 36];
        if !memory.read(rsdp, &mut bytes[..20]) {
            return Err(AcpiError::Unreadable(rsdp));
        }
        if &bytes[0..8] != b"RSD PTR " {
            return Err(AcpiError::BadSignature(rsdp));
        }
        if !checksum_ok(&bytes[..20]) {
            return Err(AcpiError::BadChecksum(rsdp));
        }
        let revision = bytes[15];
        let oem = bytes[9..15].try_into().unwrap();

        // Revision 2+: prefer the XSDT, whose entries are 64-bit.
        let (root, entry_size, signature) = if revision >= 2 {
            if !memory.read(rsdp, &mut bytes) {
                return Err(AcpiError::Unreadable(rsdp));
            }
            if !checksum_ok(&bytes) {
                return Err(AcpiError::BadChecksum(rsdp));
            }
            (u64_at(&bytes, 24), 8, b"XSDT")
        } else {
            (u64::from(u32_at(&bytes, 16)), 4, b"RSDT")
        };

        let root_table = read_table(memory, root)?;
        if &root_table.signature() != signature {
            return Err(AcpiError::BadSignature(root));
        }
        let tables = root_table
            .body()
            .chunks_exact(entry_size)
            .map(|entry| if entry_size == 8 { u64_at(entry, 0) } else { u64::from(u32_at(entry, 0)) })
            .collect();
        Ok(Acpi { revision, oem, tables })
    }

    /// The signatures of every table the root lists (unreadable ones
    /// are skipped).
    pub fn signatures(&self, memory: &impl PhysicalMemory) -> Vec<[u8; 4]> {
        self.tables
            .iter()
            .filter_map(|&address| {
                let mut signature = [0u8; 4];
                memory.read(address, &mut signature).then_some(signature)
            })
            .collect()
    }

    /// The first table with this signature.
    pub fn find(&self, memory: &impl PhysicalMemory, signature: &[u8; 4]) -> Result<Table, AcpiError> {
        for &address in &self.tables {
            let mut found = [0u8; 4];
            if memory.read(address, &mut found) && &found == signature {
                return read_table(memory, address);
            }
        }
        Err(AcpiError::NotFound(*signature))
    }
}

// ---- the MADT ---------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Processor {
    pub apic_id: u8,
    /// Usable now, or could be brought online.
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoApic {
    pub id: u8,
    pub address: u64,
    /// The first global system interrupt (GSI) this I/O APIC handles.
    pub gsi_base: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Polarity {
    ActiveHigh,
    ActiveLow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Edge,
    Level,
}

/// Where an ISA IRQ really arrives, and how its signal behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IsaRoute {
    pub irq: u8,
    pub gsi: u32,
    pub polarity: Polarity,
    pub trigger: Trigger,
}

#[derive(Debug, Default)]
pub struct Madt {
    pub local_apic_address: u64,
    /// The machine also has the old 8259 PICs, which must be disabled.
    pub has_legacy_pics: bool,
    pub processors: Vec<Processor>,
    pub io_apics: Vec<IoApic>,
    pub overrides: Vec<IsaRoute>,
}

impl Madt {
    pub fn parse(table: &Table) -> Result<Madt, AcpiError> {
        let body = table.body();
        if body.len() < 8 {
            return Err(AcpiError::BadLength(table.address));
        }
        let mut madt = Madt {
            local_apic_address: u64::from(u32_at(body, 0)),
            has_legacy_pics: u32_at(body, 4) & 1 != 0,
            ..Madt::default()
        };

        // Variable-length entries: a type byte, a length byte, data.
        let mut entries = &body[8..];
        while entries.len() >= 2 {
            let (kind, length) = (entries[0], entries[1] as usize);
            if length < 2 || length > entries.len() {
                return Err(AcpiError::BadLength(table.address));
            }
            let entry = &entries[..length];
            match (kind, length) {
                (0, 8..) => madt.processors.push(Processor {
                    apic_id: entry[3],
                    // bit 0: enabled; bit 1: online-capable
                    enabled: u32_at(entry, 4) & 0b11 != 0,
                }),
                (1, 12..) => madt.io_apics.push(IoApic {
                    id: entry[2],
                    address: u64::from(u32_at(entry, 4)),
                    gsi_base: u32_at(entry, 8),
                }),
                (2, 10..) => {
                    let flags = u16_at(entry, 8);
                    madt.overrides.push(IsaRoute {
                        irq: entry[3],
                        gsi: u32_at(entry, 4),
                        // Bits 0-1 and 2-3: 00 means "the bus's default",
                        // which for ISA is active high, edge triggered.
                        polarity: if flags & 0b11 == 0b11 { Polarity::ActiveLow } else { Polarity::ActiveHigh },
                        trigger: if (flags >> 2) & 0b11 == 0b11 { Trigger::Level } else { Trigger::Edge },
                    });
                }
                // A 64-bit local APIC address replaces the 32-bit one.
                (5, 12..) => madt.local_apic_address = u64_at(entry, 4),
                _ => {} // NMIs, x2APIC entries, ...: not needed yet
            }
            entries = &entries[length..];
        }
        Ok(madt)
    }

    /// Where ISA `irq` arrives: an override if the firmware gives one,
    /// otherwise the ISA default - the same number, active high, edge.
    pub fn isa_route(&self, irq: u8) -> IsaRoute {
        self.overrides.iter().copied().find(|o| o.irq == irq).unwrap_or(IsaRoute {
            irq,
            gsi: u32::from(irq),
            polarity: Polarity::ActiveHigh,
            trigger: Trigger::Edge,
        })
    }
}

// ---- the FADT ---------------------------------------------------------

/// What the FADT (signature "FACP", the "fixed ACPI description table")
/// says about switching the machine off and resetting it: which I/O
/// ports do it. The values to write to switch off are not here - they
/// are in the DSDT's AML (`\_S5`), which is not the kernel's to read;
/// the kernel only finds the DSDT and hands it on.
///
/// The FADT has grown with each ACPI version. Version 1's is 116 bytes,
/// with 32-bit addresses. Version 2 added the reset register (at 116)
/// and 64-bit "X_" addresses, each a generic address structure (GAS:
/// which address space, then the address), which take precedence when
/// they are there and not zero. Only I/O-space registers are taken: on
/// a PC the PM1 registers are always I/O ports, and a machine that has
/// them in memory (or none at all: "hardware-reduced" ACPI, as on most
/// ARM servers) is one this kernel does not switch off this way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fadt {
    /// The DSDT's physical address.
    pub dsdt: u64,
    /// The port that switches the chipset into ACPI mode, and the value
    /// to write (0, 0: already in ACPI mode, always).
    pub smi_command: u16,
    pub acpi_enable: u8,
    /// The PM1 control registers (PM1b: 0 if none), and their width in
    /// bytes.
    pub pm1a_control: u16,
    pub pm1b_control: u16,
    pub pm1_control_length: u8,
    /// The reset register, if the FADT names one in I/O space: its port
    /// and the value that resets.
    pub reset: Option<(u16, u8)>,
}

/// FADT flags: the reset register is there and works.
const FADT_RESET_REGISTER: u32 = 1 << 10;
/// And: "hardware-reduced ACPI" - no PM1 registers at all.
const FADT_HARDWARE_REDUCED: u32 = 1 << 20;
/// A generic address structure's address space: I/O ports.
const GAS_IO: u8 = 1;

impl Fadt {
    pub fn parse(table: &Table) -> Result<Fadt, AcpiError> {
        let b = &table.bytes;
        let length = b.len();
        if &table.signature() != b"FACP" {
            return Err(AcpiError::BadSignature(table.address));
        }
        if length < 116 {
            return Err(AcpiError::BadLength(table.address));
        }
        let flags = u32_at(b, 112);
        if flags & FADT_HARDWARE_REDUCED != 0 {
            return Err(AcpiError::Unsupported("hardware-reduced ACPI: no PM1 registers"));
        }
        // A generic address at `at`, if the table is long enough to have
        // it and it is not zero: (address space, address).
        let gas = |at: usize| -> Option<(u8, u64)> {
            (length >= at + 12).then(|| (b[at], u64_at(b, at + 4))).filter(|&(_, address)| address != 0)
        };
        // A port: an I/O-space address that fits in 16 bits.
        let port = |(space, address): (u8, u64), what: &'static str| -> Result<u16, AcpiError> {
            if space != GAS_IO {
                return Err(AcpiError::Unsupported(what));
            }
            u16::try_from(address).map_err(|_| AcpiError::Unsupported(what))
        };
        let dsdt = match (length >= 148).then(|| u64_at(b, 140)).filter(|&a| a != 0) {
            Some(wide) => wide,
            None => u64::from(u32_at(b, 40)),
        };
        if dsdt == 0 {
            return Err(AcpiError::Unsupported("the FADT names no DSDT"));
        }
        let pm1a_control = match gas(172) {
            Some(address) => port(address, "the PM1a control register is not an I/O port")?,
            None => port((GAS_IO, u64::from(u32_at(b, 64))), "the PM1a control register is not an I/O port")?,
        };
        if pm1a_control == 0 {
            return Err(AcpiError::Unsupported("the FADT names no PM1a control register"));
        }
        let pm1b_control = match gas(184) {
            Some(address) => port(address, "the PM1b control register is not an I/O port")?,
            None => port((GAS_IO, u64::from(u32_at(b, 68))), "the PM1b control register is not an I/O port")?,
        };
        let smi_command = port((GAS_IO, u64::from(u32_at(b, 48))), "the SMI command port is past 0xFFFF")?;
        let reset = if length >= 129 && flags & FADT_RESET_REGISTER != 0 {
            // A reset register in memory or PCI space: not taken. 0xCF9
            // does the same on any PC.
            gas(116).and_then(|address| port(address, "").ok()).map(|p| (p, b[128]))
        } else {
            None
        };
        Ok(Fadt {
            dsdt,
            smi_command,
            acpi_enable: b[52],
            pm1a_control,
            pm1b_control,
            pm1_control_length: match b[89] { 0 => 2, n => n },
            reset,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fake physical memory: a buffer, with physical address = offset.
    struct Memory(Vec<u8>);

    impl PhysicalMemory for Memory {
        fn read(&self, address: u64, buffer: &mut [u8]) -> bool {
            let start = address as usize;
            match self.0.get(start..start + buffer.len()) {
                Some(bytes) => {
                    buffer.copy_from_slice(bytes);
                    true
                }
                None => false,
            }
        }
    }

    impl Memory {
        fn put(&mut self, address: usize, bytes: &[u8]) {
            self.0[address..address + bytes.len()].copy_from_slice(bytes);
        }
    }

    /// Makes `bytes` sum to zero by setting the byte at `at`.
    fn fix_checksum(bytes: &mut [u8], at: usize) {
        bytes[at] = 0;
        let sum = bytes.iter().fold(0u8, |s, &b| s.wrapping_add(b));
        bytes[at] = 0u8.wrapping_sub(sum);
    }

    fn table(signature: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0u8; HEADER_LENGTH];
        bytes[0..4].copy_from_slice(signature);
        bytes.extend_from_slice(body);
        let length = bytes.len() as u32;
        bytes[4..8].copy_from_slice(&length.to_le_bytes());
        bytes[10..16].copy_from_slice(b"TAAJ  ");
        fix_checksum(&mut bytes, 9);
        bytes
    }

    /// A MADT like QEMU's: one CPU, one I/O APIC, and the usual
    /// overrides (IRQ 0 -> GSI 2; IRQ 9 level-triggered, active high).
    fn qemu_madt() -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&0xFEE0_0000u32.to_le_bytes());
        body.extend_from_slice(&1u32.to_le_bytes()); // PCAT_COMPAT
        body.extend_from_slice(&[0, 8, 0, 0, 1, 0, 0, 0]); // CPU 0, APIC 0, enabled
        body.extend_from_slice(&[0, 8, 1, 1, 0, 0, 0, 0]); // CPU 1, APIC 1, disabled
        body.extend_from_slice(&[1, 12, 0, 0]);
        body.extend_from_slice(&0xFEC0_0000u32.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&[2, 10, 0, 0]);
        body.extend_from_slice(&2u32.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&[2, 10, 0, 9]);
        body.extend_from_slice(&9u32.to_le_bytes());
        body.extend_from_slice(&0b1101u16.to_le_bytes()); // high, level
        body.extend_from_slice(&[4, 6, 0xFF, 5, 0, 1]); // an NMI entry, ignored
        table(b"APIC", &body)
    }

    /// RSDP at 0, XSDT at 0x100, an "HPET" at 0x200, the MADT at 0x300.
    fn machine(revision: u8) -> Memory {
        let mut memory = Memory(vec![0; 0x1000]);
        memory.put(0x200, &table(b"HPET", &[0; 20]));
        memory.put(0x300, &qemu_madt());

        let mut rsdp = [0u8; 36];
        rsdp[0..8].copy_from_slice(b"RSD PTR ");
        rsdp[9..15].copy_from_slice(b"TAAJ  ");
        rsdp[15] = revision;
        if revision >= 2 {
            let mut entries = Vec::new();
            entries.extend_from_slice(&0x200u64.to_le_bytes());
            entries.extend_from_slice(&0x300u64.to_le_bytes());
            memory.put(0x100, &table(b"XSDT", &entries));
            rsdp[20..24].copy_from_slice(&36u32.to_le_bytes());
            rsdp[24..32].copy_from_slice(&0x100u64.to_le_bytes());
            fix_checksum(&mut rsdp[..20], 8);
            fix_checksum(&mut rsdp, 32);
        } else {
            let mut entries = Vec::new();
            entries.extend_from_slice(&0x200u32.to_le_bytes());
            entries.extend_from_slice(&0x300u32.to_le_bytes());
            memory.put(0x100, &table(b"RSDT", &entries));
            rsdp[16..20].copy_from_slice(&0x100u32.to_le_bytes());
            fix_checksum(&mut rsdp[..20], 8);
        }
        memory.put(0, &rsdp);
        memory
    }

    #[test]
    fn finds_tables_through_the_xsdt_and_the_rsdt() {
        for revision in [0, 2] {
            let memory = machine(revision);
            let acpi = Acpi::new(&memory, 0).unwrap();
            assert_eq!(acpi.revision, revision);
            assert_eq!(&acpi.oem, b"TAAJ  ");
            assert_eq!(acpi.signatures(&memory), vec![*b"HPET", *b"APIC"]);
            assert_eq!(acpi.find(&memory, b"APIC").unwrap().address, 0x300);
            assert_eq!(acpi.find(&memory, b"FACP").err(), Some(AcpiError::NotFound(*b"FACP")));
        }
    }

    #[test]
    fn parses_the_madt() {
        let memory = machine(2);
        let acpi = Acpi::new(&memory, 0).unwrap();
        let madt = Madt::parse(&acpi.find(&memory, b"APIC").unwrap()).unwrap();

        assert_eq!(madt.local_apic_address, 0xFEE0_0000);
        assert!(madt.has_legacy_pics);
        assert_eq!(
            madt.processors,
            vec![Processor { apic_id: 0, enabled: true }, Processor { apic_id: 1, enabled: false }]
        );
        assert_eq!(madt.io_apics, vec![IoApic { id: 0, address: 0xFEC0_0000, gsi_base: 0 }]);

        // The timer is rerouted, the keyboard is not.
        assert_eq!(madt.isa_route(0).gsi, 2);
        let keyboard = madt.isa_route(1);
        assert_eq!((keyboard.gsi, keyboard.polarity, keyboard.trigger), (1, Polarity::ActiveHigh, Trigger::Edge));
        assert_eq!(madt.isa_route(9).trigger, Trigger::Level);
    }

    #[test]
    fn rejects_corrupt_tables() {
        // A flipped byte in the MADT breaks its checksum.
        let mut memory = machine(2);
        memory.0[0x300 + 40] ^= 0xFF;
        let acpi = Acpi::new(&memory, 0).unwrap();
        assert_eq!(acpi.find(&memory, b"APIC").err(), Some(AcpiError::BadChecksum(0x300)));

        // A wrong RSDP signature or checksum.
        let mut memory = machine(2);
        memory.0[0] = b'X';
        assert_eq!(Acpi::new(&memory, 0).err(), Some(AcpiError::BadSignature(0)));
        let mut memory = machine(0);
        memory.0[10] ^= 1;
        assert_eq!(Acpi::new(&memory, 0).err(), Some(AcpiError::BadChecksum(0)));

        // A table claiming to run past the end of memory.
        let mut memory = machine(2);
        memory.put(0x304, &0x10_0000u32.to_le_bytes());
        let acpi = Acpi::new(&memory, 0).unwrap();
        assert_eq!(acpi.find(&memory, b"APIC").err(), Some(AcpiError::Unreadable(0x300)));
    }

    #[test]
    fn rejects_a_madt_entry_running_off_the_end() {
        let mut body = vec![0u8; 8];
        body.extend_from_slice(&[1, 40, 0, 0]); // claims 40 bytes, has 4
        let bytes = table(b"APIC", &body);
        let t = Table { address: 0x300, bytes };
        assert_eq!(Madt::parse(&t).err(), Some(AcpiError::BadLength(0x300)));
    }

    /// A FADT `length` bytes long (header included), with QEMU's PC
    /// values: DSDT, SMI command 0xB2 (ACPI enable 0xF1), PM1a control
    /// at 0x604, 2 bytes wide.
    fn fadt(length: usize) -> Table {
        let mut b = vec![0u8; length];
        b[0..4].copy_from_slice(b"FACP");
        b[4..8].copy_from_slice(&(length as u32).to_le_bytes());
        b[40..44].copy_from_slice(&0x7FE0_0040u32.to_le_bytes());
        b[48..52].copy_from_slice(&0xB2u32.to_le_bytes());
        b[52] = 0xF1;
        b[64..68].copy_from_slice(&0x604u32.to_le_bytes());
        b[89] = 2;
        Table { address: 0x1000, bytes: b }
    }

    /// A generic address: I/O space (1) or memory (0), at `address`.
    fn put_gas(t: &mut Table, at: usize, space: u8, address: u64) {
        t.bytes[at] = space;
        t.bytes[at + 1] = 16;
        t.bytes[at + 4..at + 12].copy_from_slice(&address.to_le_bytes());
    }

    #[test]
    fn a_version_1_fadt_gives_its_ports() {
        let f = Fadt::parse(&fadt(116)).unwrap();
        assert_eq!(f, Fadt {
            dsdt: 0x7FE0_0040,
            smi_command: 0xB2,
            acpi_enable: 0xF1,
            pm1a_control: 0x604,
            pm1b_control: 0,
            pm1_control_length: 2,
            reset: None,
        });
    }

    #[test]
    fn the_wide_fields_win_and_the_reset_register_is_taken() {
        let mut t = fadt(244); // ACPI 5's length
        t.bytes[112..116].copy_from_slice(&FADT_RESET_REGISTER.to_le_bytes());
        put_gas(&mut t, 116, GAS_IO, 0xCF9);
        t.bytes[128] = 0x0F;
        t.bytes[140..148].copy_from_slice(&0x1_0000_2000u64.to_le_bytes()); // X_DSDT
        put_gas(&mut t, 172, GAS_IO, 0x1804); // X_PM1a_CNT
        put_gas(&mut t, 184, GAS_IO, 0x1904); // X_PM1b_CNT
        let f = Fadt::parse(&t).unwrap();
        assert_eq!((f.dsdt, f.pm1a_control, f.pm1b_control), (0x1_0000_2000, 0x1804, 0x1904));
        assert_eq!(f.reset, Some((0xCF9, 0x0F)));
        // The reset register without the flag that says it works: not taken.
        t.bytes[112..116].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(Fadt::parse(&t).unwrap().reset, None);
        // In memory: not taken either (0xCF9 will do).
        t.bytes[112..116].copy_from_slice(&FADT_RESET_REGISTER.to_le_bytes());
        put_gas(&mut t, 116, 0, 0xFED0_0000);
        assert_eq!(Fadt::parse(&t).unwrap().reset, None);
        // Zero wide fields: the 32-bit ones count.
        let mut t = fadt(244);
        put_gas(&mut t, 172, GAS_IO, 0);
        assert_eq!(Fadt::parse(&t).unwrap().pm1a_control, 0x604);
        assert_eq!(Fadt::parse(&t).unwrap().dsdt, 0x7FE0_0040);
    }

    #[test]
    fn what_cannot_be_switched_off_this_way_is_refused() {
        let mut t = fadt(244);
        t.bytes[112..116].copy_from_slice(&FADT_HARDWARE_REDUCED.to_le_bytes());
        assert_eq!(Fadt::parse(&t), Err(AcpiError::Unsupported("hardware-reduced ACPI: no PM1 registers")));
        // In memory - at an address small enough to pass for a port, so
        // that only the address space can be what refuses it.
        let mut t = fadt(244);
        put_gas(&mut t, 172, 0, 0x1804);
        assert_eq!(Fadt::parse(&t), Err(AcpiError::Unsupported("the PM1a control register is not an I/O port")));
        let mut t = fadt(116);
        t.bytes[64..68].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(Fadt::parse(&t), Err(AcpiError::Unsupported("the FADT names no PM1a control register")));
        let mut t = fadt(116);
        t.bytes[40..44].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(Fadt::parse(&t), Err(AcpiError::Unsupported("the FADT names no DSDT")));
        assert_eq!(Fadt::parse(&fadt(100)), Err(AcpiError::BadLength(0x1000)));
        let mut t = fadt(116);
        t.bytes[0..4].copy_from_slice(b"APIC");
        assert_eq!(Fadt::parse(&t), Err(AcpiError::BadSignature(0x1000)));
        // A width of 0 in a version 1 table: 2, the size every PC has.
        let mut t = fadt(116);
        t.bytes[89] = 0;
        assert_eq!(Fadt::parse(&t).unwrap().pm1_control_length, 2);
    }
}
