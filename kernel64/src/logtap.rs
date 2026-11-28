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
//!
//! And the crash record (K188, kernel64::crashrecord): while the kernel
//! is stopping, what it says is also written into an area of memory a
//! reset does not clear; the next boot finds it there and puts it in
//! the ring, first, as the "crash" lines of the log.

use crate::irqlock::IrqLock;
use crate::memory_object::MemoryObject;
use abi::log::{kind, Ring};
use alloc::sync::Arc;
use kernel64::logmerge::Writer;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use kernel64::crashrecord;
use kernel64::memory::addr::{phys_to_virt, PhysAddr, PAGE_SIZE};

/// 256 KiB: some minutes of a busy boot's messages, if the log service
/// falls behind or has not started yet.
const PAGES: u64 = 64;

static RING: IrqLock<Option<Ring>> = IrqLock::new(None);

/// The memory, kept from `start` to `offer`.
static MEMORY: IrqLock<Option<Arc<MemoryObject>>> = IrqLock::new(None);

/// The crash record's place: physical (set before memory management
/// starts), then virtual, through the direct map (set by `start`); 0
/// for none. No lock: they are read while the kernel stops, when a lock
/// may never be let go.
static CRASH_PHYSICAL: AtomicU64 = AtomicU64::new(0);
static CRASH_AREA: AtomicUsize = AtomicUsize::new(0);
/// Whether this boot's record was begun.
static CRASH_BEGUN: AtomicBool = AtomicBool::new(false);
/// Whether the boot before this one left a record.
static FOUND: AtomicBool = AtomicBool::new(false);

/// Whether the boot before this one left a crash record (it is in the
/// ring now). For the crash-record test (`fault-panic`).
#[cfg(feature = "fault-tests")]
pub fn found_last_boot() -> bool {
    FOUND.load(Ordering::Relaxed)
}

/// Where the crash record is, physically: memory the caller keeps out
/// of the allocator, `crashrecord::AREA` bytes.
pub fn set_crash_area(physical: u64) {
    CRASH_PHYSICAL.store(physical, Ordering::Relaxed);
}

fn crash_area() -> Option<&'static mut [u8]> {
    let base = CRASH_AREA.load(Ordering::Relaxed);
    // SAFETY: the area set aside by `set_crash_area`, through the direct
    // map; only this module touches it - at start, then while stopping,
    // when nothing else runs.
    (base != 0).then(|| unsafe { core::slice::from_raw_parts_mut(base as *mut u8, crashrecord::AREA as usize) })
}

/// The last boot's record, if it left one: into the ring, as the log's
/// first lines, and said on the serial line. Then this boot's place is
/// empty, ready.
fn last_boot() {
    let physical = CRASH_PHYSICAL.load(Ordering::Relaxed);
    if physical == 0 {
        return;
    }
    CRASH_AREA.store(phys_to_virt(PhysAddr::new(physical)).as_mut_ptr::<u8>() as usize, Ordering::Relaxed);
    let Some(area) = crash_area() else { return };
    if let Some((time, text)) = crashrecord::read(area) {
        FOUND.store(true, Ordering::Relaxed);
        if let Some(ring) = RING.lock().as_ref() {
            ring.put(kind::CRASH, 0, time, b"", text);
        }
        let first = text.split(|&b| b == b'\n').find(|line| !line.is_empty()).unwrap_or(b"");
        kprintln!(
            "[kernel] the boot before this one stopped: \"{}\" - its last words ({} bytes) are in the system log (log -s crash)",
            core::str::from_utf8(first).unwrap_or("?").trim(),
            text.len()
        );
    }
    crashrecord::clear(area);
}

/// What the kernel says while it stops (console.rs calls this instead
/// of `piece` then): into the crash record. No locks.
pub fn last_words(bytes: &[u8]) {
    let Some(area) = crash_area() else { return };
    if !CRASH_BEGUN.swap(true, Ordering::Relaxed) {
        crashrecord::begin(area, now());
    }
    crashrecord::append(area, bytes);
}

/// Makes the ring, as early as the kernel can: everything written to the
/// debug log from then on is in it. With no memory for it there is no
/// system log - the serial line is all there is.
pub fn start() {
    let Some((memory, physical)) = MemoryObject::contiguous(PAGES) else {
        kprintln!("[kernel] no memory for the system log's ring: the serial line only");
        last_boot();
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
    last_boot();
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

/// A program's crash report (K138): for the log service, which adds
/// the last lines the program printed.
pub fn report(pid: u64, text: &str) {
    if let Some(ring) = RING.lock().as_ref() {
        ring.put(kind::REPORT, pid as u32, now(), b"", text.as_bytes());
    }
}
