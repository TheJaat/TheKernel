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
//! Then, as on the PC, every device found is handed to init the same
//! way (K75, TheTaaJ entry NNN): its description - which virtio kind it
//! says it is, which interface, where - and a device handle
//! (device.rs). Which program drives it is init's choice; that program
//! asks through the handle for its registers (`bar`) and its interrupt
//! (`interrupt`), and init for its DMA memory. Until K75 the kernel
//! picked the first disk, the first network card and up to four input
//! devices, and knew how much DMA memory each kind's driver wanted.
//!
//! What this does not protect against. The DMA problem of devices.rs,
//! unchanged: the device writes wherever its driver tells it. And one
//! of this machine's own: the thirty-two places are 512 bytes each,
//! eight to a page, and a page is the smallest thing that can be
//! mapped - so the driver of one device can reach the registers of the
//! seven beside it. Real hardware gives each device its own pages; a
//! kernel that had to share a page between drivers would have to do
//! the register accesses itself.

use alloc::sync::Arc;
use alloc::vec::Vec;

use abi::device::{self as op, Bar};
use abi::{Error, MmioDeviceInfo};
use kernel64::devicetree::{DeviceSlot, Machine};
use kernel64::paging::Flags;
use virtio::mmio::{self, Identity, Nothing};

use crate::device::{Bus, Device};
use crate::interrupts::InterruptLine;
use crate::ipc::Object;
use crate::memory_object::MemoryObject;
use crate::{arch, mm};

/// Every device found, as handed to init: its description, and its
/// device. One device object for each place, made once: whoever else
/// drives one first (the disk self-test) must use the same object, or
/// the place's interrupt would be set up twice, two lines for one
/// interrupt, and only the first would ever hear it.
pub fn hand_out(found: &[Found]) -> Vec<(MmioDeviceInfo, Arc<Device>)> {
    found.iter().map(|device| (describe(device), Arc::new(handle(device)))).collect()
}

/// The description of a device found: what it says it is, and where.
/// No DMA memory yet (init asks for it), and `interrupt` says whether
/// the tree names one to ask for.
pub fn describe(found: &Found) -> MmioDeviceInfo {
    MmioDeviceInfo {
        device: found.identity.device,
        version: found.identity.version,
        registers: found.slot.registers.start,
        registers_size: found.slot.registers.size as u32,
        dma_physical: 0,
        dma_size: 0,
        interrupt: found.slot.interrupt.is_some(),
    }
}

/// A device handle for a device found.
pub fn handle(found: &Found) -> Device {
    let slot = &found.slot;
    Device::new(Bus::Mmio { start: slot.registers.start, size: slot.registers.size, interrupt: slot.interrupt, edge: slot.edge })
}

/// abi::device::BAR: a place has one, its registers - a memory object
/// over the page they are in.
pub fn bar(device: &Device, n: u64) -> Result<(Object, Bar), Error> {
    let Bus::Mmio { start, size, .. } = device.bus;
    if n != 0 {
        return Err(Error::InvalidArgument);
    }
    let registers = MemoryObject::device(start, size, Flags::NO_CACHE | Flags::WRITE_THROUGH);
    Ok((Object::Memory(Arc::new(registers)), Bar { base: start, size, io: false }))
}

/// abi::device::INTERRUPT, the first time: the line the tree names, set
/// up as it says (edge or level). Off until its driver waits.
pub fn interrupt(device: &Device, kind: u64) -> Result<&'static InterruptLine, Error> {
    let Bus::Mmio { start, interrupt, edge, .. } = device.bus;
    if kind != op::WIRED {
        return Err(Error::InvalidArgument);
    }
    let id = interrupt.ok_or(Error::NotFound)?;
    let line = arch::device_interrupt(id, edge).ok_or(Error::NotFound)?;
    // Not "[virtio] 0x...: " - that is how each device found is
    // reported, once, and aarch64-test.sh counts those lines.
    kprintln!("[virtio] interrupt {id} set up for the device at {start:#x}'s driver, off until it waits");
    Ok(line)
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
