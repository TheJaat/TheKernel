//! Tracing (TheTaaJ K139): the kernel's trace points, written to a ring
//! of each CPU's (abi::trace) in memory init is handed as "trace" - the
//! handle for tracing. Its holder switches tracing on and off and reads
//! the events; while it is off, a trace point is one load and a branch.
//!
//! No locks: a trace point may be anywhere - in an interrupt, in the
//! middle of switching threads. Each CPU writes only its own ring, with
//! event numbers from a counter of its own, taken atomically (an
//! interrupt in the middle of an event takes the next).

use crate::memory_object::MemoryObject;
use abi::trace::Trace;
use alloc::boxed::Box;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use kernel64::memory::addr::{phys_to_virt, PhysAddr, PAGE_SIZE};

/// Events in each CPU's ring: a second or two of a busy CPU (a busy
/// boot is some thousand a second) - `trace` reads every tenth of one.
const SLOTS: usize = 4096;

/// The rings, once made; null before (and if there was no memory).
static TRACE: AtomicPtr<Trace> = AtomicPtr::new(core::ptr::null_mut());
/// Each CPU's next event number.
static NEXT: [AtomicU64; kernel64::smp::MAX_CPUS] = [const { AtomicU64::new(0) }; kernel64::smp::MAX_CPUS];

/// A trace point: `kind` (abi::trace::kind) happened, with `value`.
#[inline]
pub fn point(kind: u16, value: u64) {
    let trace = TRACE.load(Ordering::Acquire);
    if trace.is_null() {
        return;
    }
    // SAFETY: made once by `offer`, never freed.
    let trace = unsafe { &*trace };
    if !trace.on() {
        return;
    }
    let cpu = crate::arch::cpu();
    let Some(next) = NEXT.get(cpu) else { return };
    let number = next.fetch_add(1, Ordering::Relaxed);
    let pid = crate::threads::current_process().map_or(0, |p| p.pid as u32);
    trace.put(cpu, number, crate::clock::monotonic(), kind, pid, value);
}

/// Makes the rings - one for each CPU running - and offers them to init
/// as "trace", to read and write: holding it is what tracing takes.
/// Nothing if there is no memory for them.
pub fn offer(to_init: &crate::ipc::Object) {
    use crate::ipc::Object;
    use kernel64::cap::Rights;
    let cpus = crate::threads::cpus().clamp(1, kernel64::smp::MAX_CPUS);
    let pages = (abi::trace::size(cpus, SLOTS) as u64).div_ceil(PAGE_SIZE);
    let Some((memory, physical)) = MemoryObject::contiguous(pages) else {
        kprintln!("[kernel] no memory for tracing's rings: no tracing");
        return;
    };
    let base = phys_to_virt(PhysAddr::new(physical)).as_mut_ptr::<u8>();
    // SAFETY: fresh frames of our own, through the direct map, page
    // aligned; kept for ever (the reference below).
    let Some(trace) = (unsafe { Trace::create(base, (pages * PAGE_SIZE) as usize, cpus) }) else { return };
    let memory = Arc::new(memory);
    // The kernel's own reference, never given back: it writes there for
    // ever, whatever the holders do with theirs.
    core::mem::forget(memory.clone());
    TRACE.store(Box::into_raw(Box::new(trace)), Ordering::Release);
    let rights = Rights(abi::rights::READ | abi::rights::WRITE | abi::rights::DUPLICATE | abi::rights::TRANSFER);
    let mut handles = alloc::vec![(Object::Memory(memory), rights)];
    // And a copy of the kernel's symbols, for the profiler to name the
    // kernel's functions (K140) - read only.
    let block = crate::backtrace::block();
    if let Some((symbols, at)) = MemoryObject::contiguous((block.len() as u64).div_ceil(PAGE_SIZE)) {
        // SAFETY: fresh frames of our own, through the direct map.
        unsafe { core::ptr::copy_nonoverlapping(block.as_ptr(), phys_to_virt(PhysAddr::new(at)).as_mut_ptr::<u8>(), block.len()) };
        handles.push((Object::Memory(Arc::new(symbols)), Rights(abi::rights::READ | abi::rights::DUPLICATE | abi::rights::TRANSFER)));
    }
    match crate::ipc::kernel_send(to_init, b"trace", handles) {
        Ok(()) => kprintln!("[kernel] tracing: {cpus} rings of {SLOTS} events, handed to init"),
        Err(error) => kprintln!("[kernel] could not hand tracing to init: {error:?}"),
    }
}
