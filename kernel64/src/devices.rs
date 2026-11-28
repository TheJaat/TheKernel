//! Devices on the PCI bus: found by the kernel, driven from user space.
//!
//! The kernel's part is mechanism, the same for every device (K75,
//! TheTaaJ entry NNN). It scans the bus, logs what is there, sizes each
//! function's BARs while nothing is using them, and hands every function
//! to init: its description (where it is, its vendor, device and class)
//! and a device handle (device.rs). Which program drives it is init's
//! business; that program asks through the handle for what its device
//! needs, and the kernel sets each up the same way whatever the device:
//!
//!   - a BAR: an IoPorts object for an I/O BAR's range, or a memory
//!     object of a memory BAR, mapped uncached - short of any MSI-X
//!     structure in it (`bar`)
//!   - its interrupt: MSI-X message 0, or MSI's one message, as the
//!     driver asks, pointed at a fresh vector (`interrupt`)
//!   - memory it can DMA to (device.rs): asked for by init, which keeps
//!     it across the driver's restarts
//!
//! Until K75 the kernel decided which devices were handed out - AHCI,
//! NVMe and xHCI by their class, Intel's e1000 cards by their numbers,
//! virtio's by theirs - and knew each one's BAR, interrupt and DMA size:
//! every new driver was a kernel change. Those choices are the drivers'
//! now (`user/src/device.rs` and each driver's start).
//!
//! What this does not protect against: a device does DMA to whatever
//! physical address its driver tells it, so a malicious disk driver could
//! have the disk overwrite the kernel. Confining DMA needs an IOMMU
//! (Intel VT-d, AMD-Vi), programmed so that each device can reach only
//! its own memory objects - Era 15's K78. Until then, a user-space driver
//! is protected from by the CPU (it cannot touch anything else directly)
//! but not by the bus.

use alloc::sync::Arc;
use alloc::vec::Vec;

use abi::device::{self as op, Bar as BarInfo};
use abi::{Error, PciDeviceInfo};
use kernel64::cap::Rights;
use kernel64::paging::Flags;
use kernel64::pci::{self, Address, Bar, ConfigSpace};

use crate::arch::{pci::Ports, platform};
use crate::device::{Bus, Device};
use crate::ipc::{self, Object};
use crate::memory_object::MemoryObject;
use crate::{interrupts, mm};

/// The most memory a BAR handed out may cover: 256 MiB, the biggest a
/// PC's 32-bit PCI hole commonly gives one device (a display's).
const BAR_MAX: u64 = 256 << 20;

/// The BARs of each function found, sized at boot (device.rs: sizing
/// switches decoding off for a moment, which a device in use must never
/// see - so it is done once, before any driver runs).
static BARS: kernel64::sync::SpinLock<Vec<(Address, [Option<Bar>; 6])>> = kernel64::sync::SpinLock::new(Vec::new());

/// Scans the bus and hands every function to init.
pub fn offer(to_init: &Object) {
    let mut config = Ports;
    let devices = pci::scan(&mut config);
    let mut handed = 0;
    for d in &devices {
        kprintln!(
            "[pci] {} {:04x}:{:04x} class {:02x}.{:02x}.{:02x}",
            d.address, d.vendor, d.device, d.class, d.subclass, d.prog_if
        );
        BARS.lock().push((d.address, size_bars(&mut config, d)));
        let info = PciDeviceInfo {
            bus: d.address.bus,
            device: d.address.device,
            function: d.address.function,
            vendor_id: d.vendor,
            device_id: d.device,
            class: d.class,
            subclass: d.subclass,
            interface: d.prog_if,
            ..PciDeviceInfo::default()
        };
        let mut message = b"pci".to_vec();
        message.extend_from_slice(&info.to_bytes());
        let handle = Object::Device(Arc::new(Device::new(Bus::Pci(d.address))));
        match ipc::kernel_send(to_init, &message, alloc::vec![(handle, Rights::ALL)]) {
            Ok(()) => handed += 1,
            Err(error) => kprintln!("[pci] {}: could not hand it to init: {error:?}", d.address),
        }
    }
    kprintln!("[pci] {handed} of {} functions handed to init, each with a handle to ask for its BARs, interrupt and DMA memory", devices.len());
}

/// Every BAR of an ordinary device (header type 0); a bridge's are the
/// bus's, not a driver's, and it gets none. A 64-bit memory BAR takes
/// two slots: the second is not one of its own.
fn size_bars(config: &mut Ports, device: &pci::Device) -> [Option<Bar>; 6] {
    let mut bars = [None; 6];
    if device.header_type & 0x7F != 0 {
        return bars;
    }
    let mut index = 0;
    while index < 6 {
        let raw = config.read32(device.address, 0x10 + 4 * index);
        bars[usize::from(index)] = pci::bar(config, device.address, index);
        let wide = raw & 1 == 0 && (raw >> 1) & 0x3 == 0x2;
        index += if wide { 2 } else { 1 };
    }
    bars
}

/// abi::device::CONFIG_READ.
pub fn config_read(address: Address, offset: u64, width: u64) -> Result<u64, Error> {
    if !matches!(width, 1 | 2 | 4) || offset % width != 0 || offset + width > 256 {
        return Err(Error::InvalidArgument);
    }
    let word = Ports.read32(address, (offset & !3) as u8);
    let shift = 8 * (offset & 3);
    let mask = if width == 4 { u64::from(u32::MAX) } else { (1 << (8 * width)) - 1 };
    Ok((u64::from(word) >> shift) & mask)
}

/// abi::device::BAR.
pub fn bar(device: &Device, n: u64) -> Result<(Object, BarInfo), Error> {
    let Bus::Pci(address) = device.bus;
    let found = BARS.lock().iter().find(|(at, _)| *at == address).map(|(_, bars)| bars.get(n as usize).copied().flatten());
    let Some(Some(found)) = found else {
        return Err(Error::InvalidArgument);
    };
    let mut config = Ports;
    let handed = match found {
        Bar::Io { base, size } => {
            if base == 0 || base + size > 0x1_0000 {
                return Err(Error::InvalidArgument);
            }
            (Object::IoPorts(base..base + size), BarInfo { base: u64::from(base), size: u64::from(size), io: true })
        }
        Bar::Memory { base, mut size, .. } => {
            if base == 0 || size > BAR_MAX {
                return Err(Error::InvalidArgument);
            }
            // An MSI-X table in this BAR (NVMe controllers often put it
            // after the doorbells) is the kernel's: a driver that could
            // write it could make the device write any memory it likes
            // - an interrupt message is a memory write. So the driver
            // gets the BAR up to the first MSI-X structure in it, whole
            // pages only (the page is what a mapping protects) - unless
            // the device's interrupt is MSI, given before: MSI-X then
            // stays off for good, and its table is inert (writing it
            // makes no interrupt and no memory write). An xHCI
            // controller's table is usually among its registers.
            let msi_for_good = device.interrupt_kind() == Some(op::MSI);
            if let Some(start) = pci::msix(&mut config, address).and_then(|msix| msix.first_in(n as u8)).filter(|_| !msi_for_good) {
                size = size.min(u64::from(start) & !0xFFF);
            }
            if size == 0 {
                return Err(Error::AccessDenied); // all of it the kernel's
            }
            // Device registers: never cached - every read must reach
            // the device, every write leave at once and in order.
            let memory = MemoryObject::device(base, size, Flags::NO_CACHE | Flags::WRITE_THROUGH);
            (Object::Memory(Arc::new(memory)), BarInfo { base, size, io: false })
        }
    };
    decode(device, &mut config, address);
    Ok(handed)
}

/// Decoding of its registers on, DMA allowed, no pin interrupts - the
/// first time a BAR or an interrupt is asked for. (K77 will tie bus
/// mastering to a handle; until then it stays on.)
fn decode(device: &Device, config: &mut Ports, address: Address) {
    device.decode(|| {
        let command = pci::command(config, address);
        pci::set_command(
            config,
            address,
            command | pci::COMMAND_IO | pci::COMMAND_MEMORY | pci::COMMAND_BUS_MASTER | pci::COMMAND_INTX_DISABLE,
        );
    });
}

/// abi::device::INTERRUPT, the first time: a new line, and the device's
/// message pointed at it.
pub fn interrupt(device: &Device, kind: u64) -> Result<&'static interrupts::InterruptLine, Error> {
    let Bus::Pci(address) = device.bus;
    let mut config = Ports;
    // Its MSI-X table is in a BAR, which must decode for the table to
    // be written.
    decode(device, &mut config, address);
    let line = match kind {
        op::MSIX => msix_to_new_line(&mut config, address),
        op::MSI => msi_to_new_line(&mut config, address),
        _ => return Err(Error::InvalidArgument),
    };
    line.ok_or(Error::NotFound)
}

/// Points the device's MSI-X message 0 at a new device line, and turns
/// MSI-X on. None if it has no MSI-X, or no line is left.
fn msix_to_new_line(config: &mut Ports, address: Address) -> Option<&'static interrupts::InterruptLine> {
    let msix = pci::msix(config, address)?;
    let table_bar = BARS.lock().iter().find(|(at, _)| *at == address).and_then(|(_, bars)| bars[usize::from(msix.table_bar)]);
    let Some(Bar::Memory { base, size, .. }) = table_bar else {
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
    pci::enable_msix(config, address, &msix);
    Some(line)
}

/// Points the device's MSI message at a new device line, and turns MSI
/// on. None if it has no MSI, or no line is left.
fn msi_to_new_line(config: &mut Ports, address: Address) -> Option<&'static interrupts::InterruptLine> {
    let msi = pci::msi(config, address)?;
    let (n, line) = interrupts::allocate_device_line()?;
    let (vector, cpu) = platform::device_vector(n);
    let (message_address, message_data) = pci::msi_message(cpu, vector);
    pci::enable_msi(config, address, &msi, message_address, message_data);
    Some(line)
}
