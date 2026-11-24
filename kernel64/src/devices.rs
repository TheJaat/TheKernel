//! Devices on the PCI bus: found by the kernel, driven from user space.
//!
//! The kernel's part is mechanism. It scans the bus, logs what is there,
//! and for each device it knows how to hand out, gathers what a driver
//! needs and turns each piece into a capability:
//!
//!   - its registers: an IoPorts object for an I/O BAR's range, or a
//!     memory object of a memory BAR, mapped uncached
//!   - memory it can DMA to: a contiguous memory object, and its
//!     physical address (devices know no other kind)
//!   - its interrupt: MSI-X message 0, or else MSI's one message,
//!     pointed at a fresh vector, and an Interrupt object for it
//!
//! and sends them to init, which decides which driver gets them - policy
//! is init's. The kernel never talks to the device again.
//!
//! What this does not protect against: a device does DMA to whatever
//! physical address its driver tells it, so a malicious disk driver could
//! have the disk overwrite the kernel. Confining DMA needs an IOMMU
//! (Intel VT-d, AMD-Vi), programmed so that each device can reach only
//! its own memory objects - a later step. Until then, a user-space driver
//! is protected from by the CPU (it cannot touch anything else directly)
//! but not by the bus.
//!
//! Offered so far: virtio block devices (disks) and virtio network
//! cards, both of the kind that speaks the legacy interface; and AHCI
//! (SATA) controllers and NVMe controllers, by their class - any
//! vendor's. What is on an AHCI controller's ports, or behind an NVMe
//! controller's namespaces, the kernel does not look at: that is the
//! driver's business.

use alloc::sync::Arc;
use alloc::vec::Vec;

use abi::PciDeviceInfo;
use kernel64::cap::Rights;
use kernel64::paging::Flags;
use kernel64::pci::{self, Bar, Device};

use crate::arch::{pci::Ports, platform};
use crate::ipc::{self, Object};
use crate::memory_object::MemoryObject;
use crate::{interrupts, mm};

/// DMA memory for a block driver: its virtqueue and request buffers -
/// 36 KiB, then room for abi::block::SECTORS_MAX sectors (128 KiB;
/// `user/src/bin/blk.rs` says where each lies).
const BLOCK_DMA_PAGES: u64 = 48;
/// And for a network card's: two queues, and forty buffers of a frame
/// each (`user/src/bin/nic.rs` says where each lies).
const NETWORK_DMA_PAGES: u64 = 40;
/// And for an AHCI controller's: a page for one port's command list,
/// received FISes and a command table, then the same 128 KiB of data
/// as a virtio disk's (`ahci/src/lib.rs`, `layout`).
const AHCI_DMA_PAGES: u64 = 1 + 32;
/// And for an NVMe controller's: a page each for the admin queues and
/// one I/O queue pair, one for a PRP list, then 128 KiB of data
/// (`nvme/src/lib.rs`, `layout`).
const NVME_DMA_PAGES: u64 = 5 + 32;
/// The most memory a BAR of registers is taken to be: AHCI's is 4 or
/// 8 KiB, NVMe's 8 or 16. More is a BAR this kernel does not understand.
const REGISTERS_MAX: u64 = 1 << 20;

/// The PCI class of an AHCI controller: storage, SATA, AHCI 1.0.
const AHCI_CLASS: (u8, u8, u8) = (0x01, 0x06, 0x01);
/// And where its registers are: BAR 5 ("ABAR").
const AHCI_BAR: u8 = 5;
/// The PCI class of an NVMe controller: storage, non-volatile memory,
/// NVMe.
const NVME_CLASS: (u8, u8, u8) = (0x01, 0x08, 0x02);
/// Its registers: BAR 0 (64 bits wide).
const NVME_BAR: u8 = 0;
/// An NVMe driver needs the registers (a page) and the doorbells of two
/// queue pairs after them: two pages when the doorbells are 4 bytes
/// apart, as nearly always. (Wider, and the driver finds them out of
/// its reach and says so.)
const NVME_REGISTERS_MIN: u64 = 0x2000;

/// How a device is handed on.
#[derive(Clone, Copy)]
enum Kind {
    /// Registers in I/O ports (BAR 0); MSI-X or polled.
    VirtioLegacy,
    /// Registers in memory (BAR 5); MSI-X, MSI or polled.
    Ahci,
    /// Registers in memory (BAR 0, cut short of any MSI-X structure in
    /// it); MSI-X, MSI or polled.
    Nvme,
}

/// What a device this kernel can hand on is called, how, and how much
/// DMA memory its driver gets. None: no driver for it.
fn offered(device: &Device) -> Option<(&'static str, Kind, u64)> {
    if (device.class, device.subclass, device.prog_if) == AHCI_CLASS {
        return Some(("AHCI controller", Kind::Ahci, AHCI_DMA_PAGES));
    }
    if (device.class, device.subclass, device.prog_if) == NVME_CLASS {
        return Some(("NVMe controller", Kind::Nvme, NVME_DMA_PAGES));
    }
    if device.vendor != virtio::PCI_VENDOR {
        return None;
    }
    match device.device {
        virtio::PCI_DEVICE_BLOCK => Some(("virtio block device", Kind::VirtioLegacy, BLOCK_DMA_PAGES)),
        virtio::PCI_DEVICE_NETWORK => Some(("virtio network card", Kind::VirtioLegacy, NETWORK_DMA_PAGES)),
        _ => None,
    }
}

/// Scans the bus and sends init a message for each device it can drive.
pub fn offer(to_init: &Object) {
    let mut config = Ports;
    let devices = pci::scan(&mut config);
    for d in &devices {
        kprintln!(
            "[pci] {} {:04x}:{:04x} class {:02x}.{:02x} {}",
            d.address, d.vendor, d.device, d.class, d.subclass, d.kind()
        );
    }
    for (device, (what, kind, dma_pages)) in devices.iter().filter_map(|device| Some((device, offered(device)?))) {
        match prepare(&mut config, device, kind, dma_pages) {
            Ok((info, handles)) => {
                let mut message = b"pci".to_vec();
                message.extend_from_slice(&info.to_bytes());
                match ipc::kernel_send(to_init, &message, handles) {
                    Ok(()) => kprintln!(
                        "[pci] {} {what}: {}, {} KiB DMA at {:#x}, {} - handed to init",
                        device.address,
                        Registers(&info),
                        info.dma_size / 1024,
                        info.dma_physical,
                        match (info.msix, info.msi) {
                            (true, _) => "MSI-X",
                            (_, true) => "MSI",
                            _ => "no interrupt (polled)",
                        }
                    ),
                    Err(error) => kprintln!("[pci] {}: could not hand it to init: {error:?}", device.address),
                }
            }
            Err(why) => kprintln!("[pci] {} {what} not usable: {why}", device.address),
        }
    }
}

type Handles = Vec<(Object, Rights)>;

/// The registers part of a boot-log line.
struct Registers<'a>(&'a PciDeviceInfo);

impl core::fmt::Display for Registers<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        let info = self.0;
        if info.registers_in_memory() {
            write!(f, "registers {:#x}+{:#x}", info.registers, info.registers_size)
        } else {
            write!(f, "ports {:#x}+{:#x}", info.io_base, info.io_size)
        }
    }
}

fn prepare(config: &mut Ports, device: &Device, kind: Kind, dma_pages: u64) -> Result<(PciDeviceInfo, Handles), &'static str> {
    let address = device.address;
    let mut info = PciDeviceInfo {
        bus: address.bus,
        device: address.device,
        function: address.function,
        vendor_id: device.vendor,
        device_id: device.device,
        class: device.class,
        subclass: device.subclass,
        interface: device.prog_if,
        ..PciDeviceInfo::default()
    };
    // The registers, as a capability.
    let registers = match kind {
        Kind::VirtioLegacy => {
            let Some(Bar::Io { base, size }) = pci::bar(config, address, 0) else {
                return Err("BAR 0 is not I/O ports (not a legacy-capable device)");
            };
            if base == 0 || base + size > 0x1_0000 {
                return Err("BAR 0 has no usable port range");
            }
            info.io_base = base as u16;
            info.io_size = size as u16;
            Object::IoPorts(base..base + size)
        }
        Kind::Ahci | Kind::Nvme => {
            let (bar, missing) = match kind {
                Kind::Nvme => (NVME_BAR, "BAR 0 is not memory (no NVMe registers)"),
                _ => (AHCI_BAR, "BAR 5 is not memory (no AHCI registers)"),
            };
            let Some(Bar::Memory { base, mut size, .. }) = pci::bar(config, address, bar) else {
                return Err(missing);
            };
            if base == 0 || size > REGISTERS_MAX {
                return Err("the registers' BAR has no usable memory range");
            }
            // An MSI-X table in the same BAR (NVMe controllers often put
            // it after the doorbells) is the kernel's: a driver that
            // could write it could make the device write any memory it
            // likes - an interrupt message is a memory write. So the
            // driver gets the BAR up to the first MSI-X structure in it,
            // whole pages only (the page is what a mapping protects).
            if let Some(start) = pci::msix(config, address).and_then(|msix| msix.first_in(bar)) {
                size = size.min(u64::from(start) & !0xFFF);
            }
            if matches!(kind, Kind::Nvme) && size < NVME_REGISTERS_MIN {
                return Err("BAR 0 leaves no room for the doorbells before the MSI-X table");
            }
            info.registers = base;
            info.registers_size = size;
            // Device registers: never cached - every read must reach
            // the device, every write leave at once and in order.
            let memory = MemoryObject::device(base, size, Flags::NO_CACHE | Flags::WRITE_THROUGH);
            Object::Memory(Arc::new(memory))
        }
    };

    // Decode its registers, let it do DMA, and no pin interrupts.
    let command = pci::command(config, address);
    pci::set_command(
        config,
        address,
        command | pci::COMMAND_IO | pci::COMMAND_MEMORY | pci::COMMAND_BUS_MASTER | pci::COMMAND_INTX_DISABLE,
    );

    let (dma, dma_physical) = MemoryObject::contiguous(dma_pages).ok_or("no contiguous memory for DMA")?;
    info.dma_physical = dma_physical;
    info.dma_size = dma.size();
    let mut handles: Handles = alloc::vec![(registers, Rights::ALL), (Object::Memory(Arc::new(dma)), Rights::ALL)];

    // A legacy virtio device is told which MSI-X entry to use, and has
    // no other way; AHCI and NVMe controllers take whichever they have.
    let line = match msix_to_new_line(config, device) {
        Some(line) => {
            info.msix = true;
            Some(line)
        }
        None if matches!(kind, Kind::Ahci | Kind::Nvme) => {
            let line = msi_to_new_line(config, device);
            info.msi = line.is_some();
            line
        }
        None => None,
    };
    if let Some(line) = line {
        handles.push((Object::Interrupt(line), Rights::ALL));
    }
    Ok((info, handles))
}

/// Points the device's MSI-X message 0 at a new device line, and turns
/// MSI-X on. None if it has no MSI-X, or no line is left.
fn msix_to_new_line(config: &mut Ports, device: &Device) -> Option<&'static interrupts::InterruptLine> {
    let msix = pci::msix(config, device.address)?;
    let Some(Bar::Memory { base, size, .. }) = pci::bar(config, device.address, msix.table_bar) else {
        return None;
    };
    if u64::from(msix.table_offset) + pci::MSIX_ENTRY_SIZE > size {
        return None;
    }
    let (n, line) = interrupts::allocate_device_line()?;
    let (vector, cpu) = platform::device_vector(n);
    let (message_address, message_data) = pci::msi_message(cpu, vector);

    // The table: in the BAR's memory, device registers, uncached.
    let table = mm::map_mmio(base + u64::from(msix.table_offset), pci::MSIX_ENTRY_SIZE) as *mut u32;
    // SAFETY: entry 0 of the table, just mapped; volatile, as for any
    // device register. The vector control word last: it unmasks.
    unsafe {
        table.write_volatile(message_address as u32);
        table.add(1).write_volatile((message_address >> 32) as u32);
        table.add(2).write_volatile(message_data);
        table.add(3).write_volatile(0);
    }
    pci::enable_msix(config, device.address, &msix);
    Some(line)
}

/// Points the device's MSI message at a new device line, and turns MSI
/// on. None if it has no MSI, or no line is left.
fn msi_to_new_line(config: &mut Ports, device: &Device) -> Option<&'static interrupts::InterruptLine> {
    let msi = pci::msi(config, device.address)?;
    let (n, line) = interrupts::allocate_device_line()?;
    let (vector, cpu) = platform::device_vector(n);
    let (message_address, message_data) = pci::msi_message(cpu, vector);
    pci::enable_msi(config, device.address, &msi, message_address, message_data);
    Some(line)
}
