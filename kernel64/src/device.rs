//! A device handle: one device the kernel found, which its holder may
//! ask for the device's resources (call::DEVICE, abi::device; K75,
//! TheTaaJ entry NNN).
//!
//! The kernel hands every device it finds - each PCI function
//! (devices.rs), each occupied virtio place in memory (mmio_devices.rs)
//! - to init the same way: a description, and one of these. It knows no
//! kind of device: not which BAR a controller's registers are in, not
//! which kind of interrupt suits it, not how much DMA memory its driver
//! wants. The driver asks for those, and init for the DMA memory, which
//! it keeps across the driver's restarts. A new driver is a new
//! program and a row in init's table, and no change here.
//!
//! What stays the kernel's is what would let a driver reach past its
//! own device: the PCI command register and BARs (where the device
//! decodes, whether it may DMA), the MSI and MSI-X structures (an
//! interrupt message is a memory write the device makes), and which
//! physical memory is the device's to DMA to. Each is set up here, on
//! request, the same way for every device.

use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use abi::device::{self as op, Bar};
use abi::Error;
use kernel64::cap::Rights;

use crate::interrupts::InterruptLine;
use crate::ipc::Object;
use crate::memory_object::MemoryObject;
use kernel64::sync::SpinLock;

/// Where a device is.
#[derive(Debug, Clone, Copy)]
pub enum Bus {
    /// A PCI function.
    #[cfg(target_arch = "x86_64")]
    Pci(kernel64::pci::Address),
    /// A place in memory the device tree describes: its registers, and
    /// the interrupt the tree names for it, if any.
    #[cfg(target_arch = "aarch64")]
    Mmio { start: u64, size: u64, interrupt: Option<u32>, edge: bool },
}

/// One device, as handed out.
pub struct Device {
    pub bus: Bus,
    /// Its interrupt, once asked for: the kind, and the line. For good -
    /// a restarted driver is given the same one again.
    interrupt: SpinLock<Option<(u64, &'static InterruptLine)>>,
    /// DMA pages given for it so far, at most op::DMA_PAGES_MAX.
    dma_pages: AtomicU64,
    /// Whether its decoding and bus mastering are on (PCI).
    #[cfg_attr(target_arch = "aarch64", allow(dead_code))]
    decoding: AtomicBool,
}

impl Device {
    pub fn new(bus: Bus) -> Device {
        Device { bus, interrupt: SpinLock::new(None), dma_pages: AtomicU64::new(0), decoding: AtomicBool::new(false) }
    }

    /// The kind of interrupt it was given, if any.
    pub fn interrupt_kind(&self) -> Option<u64> {
        self.interrupt.lock().map(|(kind, _)| kind)
    }

    /// Turns its decoding and bus mastering on, the first time (PCI).
    #[cfg(target_arch = "x86_64")]
    pub fn decode(&self, set_up: impl FnOnce()) {
        if !self.decoding.swap(true, Ordering::AcqRel) {
            set_up();
        }
    }

    /// The device's interrupt of `kind`: the one it has, or a new one
    /// from `make` if it has none yet.
    fn interrupt(&self, kind: u64, make: impl FnOnce() -> Result<&'static InterruptLine, Error>) -> Result<&'static InterruptLine, Error> {
        let mut given = self.interrupt.lock();
        match *given {
            Some((had, line)) if had == kind => Ok(line),
            // It has one interrupt, for good, of another kind: none of
            // this one. NotFound, as for a device without it - so that
            // a restarted driver trying its kinds in order (MSI-X, then
            // MSI) comes to the one it had (TheTaaJ entry NNN, bug 2).
            Some(_) => Err(Error::NotFound),
            None => {
                let line = make()?;
                *given = Some((kind, line));
                Ok(line)
            }
        }
    }

    /// Contiguous memory for it to DMA to, and where it is.
    fn dma(&self, pages: u64) -> Result<(MemoryObject, u64), Error> {
        if pages == 0 || pages > op::DMA_PAGES_MAX {
            return Err(Error::InvalidArgument);
        }
        let before = self.dma_pages.fetch_add(pages, Ordering::AcqRel);
        if before + pages > op::DMA_PAGES_MAX {
            self.dma_pages.fetch_sub(pages, Ordering::AcqRel);
            return Err(Error::InvalidArgument);
        }
        MemoryObject::contiguous(pages).ok_or_else(|| {
            self.dma_pages.fetch_sub(pages, Ordering::AcqRel);
            Error::OutOfMemory
        })
    }
}

/// call::DEVICE.
pub fn call(handle: u64, which: u64, argument: u64, out: u64) -> Result<u64, Error> {
    let process = crate::process::current().ok_or(Error::InvalidHandle)?;
    let device = match process.handles.lock().get(handle, Rights(0))? {
        Object::Device(device) => Arc::clone(device),
        _ => return Err(Error::WrongType),
    };
    if process.handles.lock().room() == 0 && matches!(which, op::BAR | op::INTERRUPT | op::DMA) {
        return Err(Error::TooManyHandles);
    }
    let insert = |object: Object| process.handles.lock().insert(object, Rights::ALL);
    match which {
        op::CONFIG_READ => config_read(&device, argument),
        op::BAR => {
            let (object, bar) = bar(&device, argument)?;
            crate::process::copy_to_user(out, &bar.to_bytes())?;
            insert(object)
        }
        op::INTERRUPT => {
            let line = device.interrupt(argument, || new_interrupt(&device, argument))?;
            insert(Object::Interrupt(line))
        }
        op::DMA => {
            crate::process::check_writable(out, 8)?;
            let (memory, physical) = device.dma(argument)?;
            crate::process::copy_to_user(out, &physical.to_le_bytes())?;
            insert(Object::Memory(Arc::new(memory)))
        }
        _ => Err(Error::InvalidArgument),
    }
}

#[cfg(target_arch = "x86_64")]
fn config_read(device: &Device, argument: u64) -> Result<u64, Error> {
    let Bus::Pci(address) = device.bus;
    crate::devices::config_read(address, argument & 0xFFFF, argument >> 16)
}

#[cfg(target_arch = "aarch64")]
fn config_read(_: &Device, _: u64) -> Result<u64, Error> {
    // A place in memory has no configuration space: its registers are all.
    Err(Error::InvalidArgument)
}

#[cfg(target_arch = "x86_64")]
fn bar(device: &Device, n: u64) -> Result<(Object, Bar), Error> {
    crate::devices::bar(device, n)
}

#[cfg(target_arch = "aarch64")]
fn bar(device: &Device, n: u64) -> Result<(Object, Bar), Error> {
    crate::mmio_devices::bar(device, n)
}

#[cfg(target_arch = "x86_64")]
fn new_interrupt(device: &Device, kind: u64) -> Result<&'static InterruptLine, Error> {
    crate::devices::interrupt(device, kind)
}

#[cfg(target_arch = "aarch64")]
fn new_interrupt(device: &Device, kind: u64) -> Result<&'static InterruptLine, Error> {
    crate::mmio_devices::interrupt(device, kind)
}
