//! Devices at fixed places in memory: found by the kernel, to be
//! driven from user space.
//!
//! A PC's devices sit on a bus that can be asked what is on it (PCI:
//! devices.rs). The machines AArch64 runs on mostly have no such bus.
//! Their devices are simply *there*, at addresses the machine's
//! designer chose, and the only way to know is to be told - which is
//! what the device tree is for.
//!
//! For virtio devices the tree tells less than it seems to. QEMU's
//! `virt` machine describes thirty-two places a virtio device may be,
//! always, whether it was started with thirty-two devices or none. So
//! the kernel looks at each place: the first registers say whether
//! anything is there, and what (`virtio::mmio::identify`).
//!
//! Then, as on the PC, a device is made ready for a driver (`prepare`):
//! what a driver needs, each piece a capability -
//!
//!   - its registers: a memory object over the page they are in
//!   - memory it can DMA to: a contiguous memory object, and its
//!     physical address
//!   - its interrupt: a shared interrupt of the controller's, set up
//!     as the tree says, and an Interrupt object for it
//!
//! and the kernel does not touch the device again.
//!
//! What this does not protect against. The DMA problem of devices.rs,
//! unchanged: the device writes wherever its driver tells it. And one
//! of this machine's own: the thirty-two places are 512 bytes each,
//! eight to a page, and a page is the smallest thing that can be
//! mapped - so the driver of one device can reach the registers of the
//! seven beside it. Real hardware gives each device its own pages; a
//! kernel that had to share a page between drivers would have to do
//! the register accesses itself.
//!
//! Only legacy block devices are made ready so far.

use alloc::sync::Arc;
use alloc::vec::Vec;

use abi::MmioDeviceInfo;
use kernel64::cap::Rights;
use kernel64::devicetree::{DeviceSlot, Machine};
use kernel64::paging::Flags;
use virtio::mmio::{self, Identity, Nothing};

use crate::ipc::Object;
use crate::memory_object::MemoryObject;
use crate::{arch, mm};

/// DMA memory for a block driver: its virtqueue and request buffers.
const BLOCK_DMA_PAGES: u64 = 16;

/// A device made ready: its description, and the handles a driver is
/// started with - registers, DMA memory, then the interrupt if there
/// is one. They can be given again: to the next driver, if one stops.
pub struct Ready {
    pub info: MmioDeviceInfo,
    pub handles: Vec<(Object, Rights)>,
}

/// Makes `found` ready for a driver. After the interrupt controller is
/// set up: the device's interrupt is one of its lines.
pub fn prepare(found: &Found) -> Result<Ready, &'static str> {
    if found.identity.device != mmio::DEVICE_BLOCK {
        return Err("no driver for this kind of device yet");
    }
    if found.identity.version != 1 {
        return Err("the modern interface - no driver speaks it yet");
    }
    let slot = &found.slot;
    let registers = MemoryObject::device(slot.registers.start, slot.registers.size, Flags::NO_CACHE | Flags::WRITE_THROUGH);
    let (dma, dma_physical) = MemoryObject::contiguous(BLOCK_DMA_PAGES).ok_or("no contiguous memory for DMA")?;
    let dma_size = dma.size();
    let mut handles = alloc::vec![
        (Object::Memory(Arc::new(registers)), Rights::ALL),
        (Object::Memory(Arc::new(dma)), Rights::ALL),
    ];
    // Without an interrupt the driver asks the device again and again
    // (it polls): slower, and correct.
    let line = slot.interrupt.and_then(|id| arch::device_interrupt(id, slot.edge));
    if let Some(line) = line {
        handles.push((Object::Interrupt(line), Rights::ALL));
    }
    let info = MmioDeviceInfo {
        device: found.identity.device,
        version: found.identity.version,
        registers: slot.registers.start,
        registers_size: slot.registers.size as u32,
        dma_physical,
        dma_size,
        interrupt: line.is_some(),
    };
    kprintln!(
        "[virtio] {:#x} {}: registers in the page at {:#x}, {} KiB DMA at {:#x}, {} - ready for a driver",
        slot.registers.start,
        found.identity.kind(),
        slot.registers.start & !0xFFF,
        dma_size / 1024,
        dma_physical,
        match (line, slot.interrupt) {
            (Some(_), Some(id)) => alloc::format!("interrupt {id}, off until its driver waits"),
            _ => alloc::string::String::from("no interrupt (polled)"),
        }
    );
    Ok(Ready { info, handles })
}

/// The first block device among `found` that a driver exists for, made
/// ready; what stands in the way of the others is said.
pub fn first_disk(found: &[Found]) -> Option<Ready> {
    for device in found.iter().filter(|device| device.identity.device == mmio::DEVICE_BLOCK) {
        match prepare(device) {
            Ok(ready) => return Some(ready),
            Err(why) => kprintln!("[virtio] {:#x} {}: {why}", device.slot.registers.start, device.identity.kind()),
        }
    }
    None
}

/// A virtio device found in one of the tree's places.
#[derive(Debug, Clone, Copy)]
pub struct Found {
    pub slot: DeviceSlot,
    pub identity: Identity,
    /// For a block device: how many 512-byte sectors it has.
    pub sectors: Option<u64>,
}

/// Looks at every place the tree describes and says what is there.
pub fn find(machine: &Machine) -> Vec<Found> {
    let slots = machine.virtio();
    let mut found = Vec::new();
    if machine.virtio_count == 0 {
        kprintln!("[virtio] the device tree describes no places for virtio devices");
        return found;
    }
    if machine.virtio_count > slots.len() {
        kprintln!("[virtio] {} places described; only the first {} are looked at", machine.virtio_count, slots.len());
    }

    // The places are usually side by side - 512 bytes each, eight to a
    // page - so they are mapped as one stretch, not thirty-two.
    let start = slots.iter().map(|slot| slot.registers.start).min().unwrap_or(0);
    let end = slots.iter().map(|slot| slot.registers.start + slot.registers.size).max().unwrap_or(0);
    const SPREAD_MAX: u64 = 1 << 20;
    if end - start > SPREAD_MAX {
        kprintln!("[virtio] the places are spread over {} KiB: not looked at", (end - start) / 1024);
        return found;
    }
    let mapped = mm::map_mmio(start, end - start);

    let mut empty = 0;
    for slot in slots {
        let registers = (mapped + (slot.registers.start - start)) as *const u32;
        // The identifying registers are in the first 16 bytes and a
        // block device's size at 0x100; a place smaller than that
        // cannot be what it claims.
        if slot.registers.size < u64::from(mmio::CONFIG) + 8 {
            kprintln!("[virtio] {:#x}: only {} bytes of registers - not looked at", slot.registers.start, slot.registers.size);
            continue;
        }
        // SAFETY: inside the range mapped above, as device memory;
        // registers are four bytes each at multiples of four, read
        // volatile because each read is a question put to the device.
        let read = |offset: u32| unsafe { registers.byte_add(offset as usize).read_volatile() };
        match mmio::identify(read) {
            Ok(identity) => {
                // A block device's configuration starts with its size.
                // It may be read before any driver has said a word.
                let sectors = (identity.device == mmio::DEVICE_BLOCK)
                    .then(|| u64::from(read(mmio::CONFIG)) | u64::from(read(mmio::CONFIG + 4)) << 32);
                report(slot, &identity, sectors);
                found.push(Found { slot: *slot, identity, sectors });
            }
            Err(Nothing::Empty) => empty += 1,
            Err(Nothing::NotVirtio { magic }) => {
                kprintln!("[virtio] {:#x}: not a virtio device (it begins {magic:#010x}, not \"virt\")", slot.registers.start)
            }
            Err(Nothing::UnknownVersion { version }) => {
                kprintln!("[virtio] {:#x}: a virtio device of version {version}, which this kernel does not know", slot.registers.start)
            }
        }
    }
    kprintln!(
        "[virtio] {} places at {start:#x} - {end:#x}: {} with a device, {empty} empty",
        slots.len(),
        found.len()
    );
    found
}

fn report(slot: &DeviceSlot, identity: &Identity, sectors: Option<u64>) {
    let interrupt = match slot.interrupt {
        Some(number) => alloc::format!("interrupt {number} ({})", if slot.edge { "edge" } else { "level" }),
        None => alloc::string::String::from("no interrupt"),
    };
    let size = match sectors {
        Some(sectors) => alloc::format!(", {sectors} sectors ({} KiB)", sectors / 2),
        None => alloc::string::String::new(),
    };
    kprintln!(
        "[virtio] {:#x}: {} ({} interface){size}, {interrupt}",
        slot.registers.start,
        identity.kind(),
        identity.interface()
    );
}
