//! The system log's copy of the debug log (TheTaaJ K136).
//!
//! Everything written to the debug log - the kernel's messages, what
//! every program prints - goes to the serial line as before, and is
//! also put here, a piece at a time, with who wrote it and when: a
//! ring in memory (abi::log) that init is handed for the log service,
//! which keeps it in a file. The serial line does not wait for it, nor
//! it for the serial line.
//!
//! The kernel keeps its own reference to the memory, so the ring stays
//! whatever init or the log service do with theirs.

use crate::irqlock::IrqLock;
use crate::memory_object::MemoryObject;
use abi::log::{kind, Ring};
use alloc::sync::Arc;
use kernel64::logmerge::Writer;
use kernel64::memory::addr::{phys_to_virt, PhysAddr, PAGE_SIZE};

/// 256 KiB: some minutes of a busy boot's messages, if the log service
/// falls behind or has not started yet.
const PAGES: u64 = 64;

static RING: IrqLock<Option<Ring>> = IrqLock::new(None);

/// The memory, kept from `start` to `offer`.
static MEMORY: IrqLock<Option<Arc<MemoryObject>>> = IrqLock::new(None);

/// Makes the ring, as early as the kernel can: everything written to the
/// debug log from then on is in it. With no memory for it there is no
/// system log - the serial line is all there is.
pub fn start() {
    let Some((memory, physical)) = MemoryObject::contiguous(PAGES) else {
        kprintln!("[kernel] no memory for the system log's ring: the serial line only");
        return;
    };
    let base = phys_to_virt(PhysAddr::new(physical)).as_mut_ptr::<u8>();
    // SAFETY: fresh frames of our own, through the direct map, page
    // aligned; kept for ever (the reference below).
    let Some(ring) = (unsafe { Ring::create(base, (PAGES * PAGE_SIZE) as usize) }) else { return };
    let memory = Arc::new(memory);
    // The kernel's own reference, never given back: the frames must
    // outlive every other holder, and the kernel writes them for ever.
    core::mem::forget(memory.clone());
    *MEMORY.lock() = Some(memory);
    *RING.lock() = Some(ring);
}

/// Offers the ring to init, as "log" with the memory - to read only:
/// only the kernel writes. Nothing if there is none (init then starts
/// no log service).
pub fn offer(to_init: &crate::ipc::Object) {
    use crate::ipc::Object;
    use kernel64::cap::Rights;
    let Some(memory) = MEMORY.lock().take() else { return };
    // init copies it for the service (DUPLICATE), and hands the copy on.
    let rights = Rights(abi::rights::READ | abi::rights::DUPLICATE | abi::rights::TRANSFER);
    match crate::ipc::kernel_send(to_init, b"log", alloc::vec![(Object::Memory(memory), rights)]) {
        Ok(()) => kprintln!("[kernel] system log: {} KiB ring, handed to init", PAGES * PAGE_SIZE / 1024),
        Err(error) => kprintln!("[kernel] could not hand the system log to init: {error:?}"),
    }
}

fn now() -> u64 {
    if crate::clock::started() {
        crate::clock::realtime()
    } else {
        0
    }
}

/// A piece of the debug log, as `writer` gave it.
pub fn piece(writer: Writer, bytes: &[u8]) {
    if let Some(ring) = RING.lock().as_ref() {
        match writer {
            Writer::Kernel => ring.put(kind::KERNEL, 0, now(), b"", bytes),
            Writer::Program(pid) => ring.put(kind::PROGRAM, pid as u32, now(), b"", bytes),
        }
    }
}

/// A program started: its name, for the log service to put on its lines.
pub fn started(pid: u64, name: &str) {
    if let Some(ring) = RING.lock().as_ref() {
        ring.put(kind::STARTED, pid as u32, now(), name.as_bytes(), b"");
    }
}
