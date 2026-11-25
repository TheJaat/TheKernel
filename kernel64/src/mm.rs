//! Memory management, wired to the machine.
//!
//! `kernel64::memory` and `kernel64::paging` are the algorithms; this is
//! where they meet real RAM. It is the same on every CPU. What differs
//! (how a table is made the one in use, where the kernel's bytes
//! really are, how much memory can be reached before the kernel has
//! tables of its own) it asks `arch::mmu` for. At boot it:
//!
//!   1. places the frame bitmap, using the memory map
//!   2. builds the kernel's own page tables, replacing the ones it was
//!      started on (the loader's, or its own first rough ones):
//!
//! ```text
//!        0x0000_0000_0000_0000  lower half: nothing (for user space).
//!                               Address 0 faults, so null pointers do.
//!        0xFFFF_8000_0000_0000  the direct map: all physical memory,
//!                               read/write, never executable
//!        0xFFFF_C000_0000_0000  the kernel heap, growing upward
//!        0xFFFF_D800_0000_0000  kernel thread stacks, each with a guard
//!        0xFFFF_E000_0000_0000  device registers (MMIO), uncached
//!        0xFFFF_FFFF_8000_0000  the kernel image: code read+execute,
//!                               rodata read-only, data read/write -
//!                               and an unmapped guard page below the
//!                               boot stack
//! ```
//!
//!   3. turns on the protections and switches to the new tables
//!   4. starts the heap, so `alloc` (Box, Vec, String) works
//!
//! Before the switch only part of memory can be reached (`early_reach`:
//! the first 4 GiB on a PC, the one GiB the kernel is in on AArch64),
//! and the bitmap and every page table have to be written. So memory
//! is handed to the frame allocator in two goes: what is in reach
//! first - the tables are built from that - and the rest once they are
//! built.
//!
//! Lock order, whenever more than one is held: HEAP, then TABLES, then
//! FRAMES. Taking them in one fixed order is what makes deadlock between
//! them impossible.

use core::alloc::{GlobalAlloc, Layout};
use core::ops::Range;
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};

use kernel64::bootinfo::{MemoryRegion, MEMORY_ACPI_NVS, MEMORY_ACPI_RECLAIMABLE};
use kernel64::memory::addr::{
    align_down, align_up, phys_to_virt, set_physical_offset, PhysAddr, VirtAddr, PAGE_SIZE,
};
use kernel64::memory::frame::FrameAllocator;
use heap::Heap;
use kernel64::memory::layout::{find_free_space, find_free_space_avoiding, inside, outside};
use kernel64::paging::{Flags, MapError, PageSize, PageTables};
use kernel64::acpi::PhysicalMemory;

use crate::arch::mmu;
use crate::irqlock::IrqLock;

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

/// Where the kernel is linked to run (linker.ld, linker-aarch64.ld).
pub const KERNEL_WINDOW: u64 = 0xFFFF_FFFF_8000_0000;
pub const DIRECT_MAP: u64 = 0xFFFF_8000_0000_0000;
pub const HEAP_START: u64 = 0xFFFF_C000_0000_0000;
/// Kernel thread stacks, one slot each (see KernelStack).
pub const STACKS_START: u64 = 0xFFFF_D800_0000_0000;
/// Device registers, mapped uncached, one after another.
pub const MMIO_START: u64 = 0xFFFF_E000_0000_0000;
const MMIO_SIZE: u64 = GIB;
const HEAP_INITIAL: u64 = MIB;
const HEAP_GROWTH: u64 = MIB;
const HEAP_MAX: u64 = 256 * MIB;

pub static FRAMES: IrqLock<Option<FrameAllocator>> = IrqLock::new(None);
pub static TABLES: IrqLock<Option<PageTables>> = IrqLock::new(None);
static NO_EXECUTE: AtomicBool = AtomicBool::new(false);
/// Physical address of the kernel's PML4 - the address space kernel
/// threads run in.
static KERNEL_ROOT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// A page of ordinary memory below 1 MiB that nothing uses, or 0 if
/// there is none: where another CPU can be started (arch::smp). Found
/// here because here is where it is known what the loader left in low
/// memory that the kernel still reads.
static LOW_PAGE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

pub fn low_page() -> Option<u64> {
    match LOW_PAGE.load(Ordering::Relaxed) {
        0 => None,
        page => Some(page),
    }
}

pub fn kernel_root() -> u64 {
    KERNEL_ROOT.load(Ordering::Relaxed)
}

/// Runs `f` with the frame allocator alone - for work on a process's
/// page tables, which are not the kernel's.
pub fn with_frames<R>(f: impl FnOnce(&mut FrameAllocator) -> R) -> R {
    f(FRAMES.lock().as_mut().expect("memory not initialised"))
}

extern "C" {
    // The linker scripts. Only their addresses mean anything.
    static __kernel_start: u8;
    static __text_end: u8;
    static __rodata_end: u8;
    static __kernel_end: u8;
    static __boot_stack_guard: u8;
}

/// The kernel's section boundaries, as virtual addresses.
pub struct KernelLayout {
    pub text: Range<u64>,
    pub rodata: Range<u64>,
    pub data: Range<u64>,
    pub stack_guard: u64,
}

pub fn kernel_layout() -> KernelLayout {
    let start = &raw const __kernel_start as u64;
    let text_end = &raw const __text_end as u64;
    let rodata_end = &raw const __rodata_end as u64;
    let end = &raw const __kernel_end as u64;
    KernelLayout {
        text: start..text_end,
        rodata: text_end..rodata_end,
        data: rodata_end..end,
        stack_guard: &raw const __boot_stack_guard as u64,
    }
}

/// NO_EXECUTE if this CPU supports it, nothing otherwise - so the same
/// mapping code works either way.
pub fn no_execute() -> Flags {
    if NO_EXECUTE.load(Ordering::Relaxed) {
        Flags::NO_EXECUTE
    } else {
        Flags::NONE
    }
}

/// Runs `f` with the kernel's page tables and frame allocator, taking
/// the locks in the documented order.
pub fn with_memory<R>(f: impl FnOnce(&mut PageTables, &mut FrameAllocator) -> R) -> R {
    let mut tables = TABLES.lock();
    let mut frames = FRAMES.lock();
    f(
        tables.as_mut().expect("memory not initialised"),
        frames.as_mut().expect("memory not initialised"),
    )
}

/// Maps `range` (physical) into the direct map with 2 MiB pages,
/// skipping 2 MiB pages already there.
fn map_direct(tables: &mut PageTables, frames: &mut FrameAllocator, range: Range<u64>) {
    let size = PageSize::Size2M.bytes();
    let flags = Flags::WRITABLE | Flags::GLOBAL | no_execute();
    let mut physical = align_down(range.start, size);
    while physical < align_up(range.end, size) {
        let virt = VirtAddr::new(DIRECT_MAP + physical);
        match tables.map(virt, PhysAddr::new(physical), PageSize::Size2M, flags, frames) {
            Ok(()) | Err(MapError::AlreadyMapped) => {}
            Err(error) => panic!("direct map of {physical:#x} failed: {error:?}"),
        }
        physical += size;
    }
}

/// Maps the kernel image pages in `range` with `flags`, leaving out the
/// stack guard page.
fn map_kernel(tables: &mut PageTables, frames: &mut FrameAllocator, range: Range<u64>, flags: Flags) {
    let guard = kernel_layout().stack_guard;
    for virt in (range.start..range.end).step_by(PAGE_SIZE as usize) {
        if virt == guard {
            continue;
        }
        let physical = PhysAddr::new(mmu::kernel_physical(virt));
        tables
            .map(VirtAddr::new(virt), physical, PageSize::Size4K, flags | Flags::GLOBAL, frames)
            .unwrap_or_else(|error| panic!("mapping the kernel at {virt:#x} failed: {error:?}"));
    }
}

/// What the kernel is told about memory when it starts - by the
/// loader's boot info on a PC, by the device tree on AArch64.
pub struct BootMemory<'a> {
    /// What memory there is, and of what kind. Regions may overlap;
    /// where they do, anything not usable wins.
    pub regions: &'a [MemoryRegion],
    /// Physical ranges that hold something already, whatever the
    /// regions say: the boot info, the ramdisk, the device tree. The
    /// kernel's own image need not be listed. Unused places are `0..0`.
    pub taken: [Range<u64>; BootMemory::TAKEN],
    /// Physical ranges to put in the direct map besides the memory
    /// itself: the screen, where firmware keeps its tables.
    pub also_mapped: [Range<u64>; BootMemory::ALSO_MAPPED],
}

impl BootMemory<'_> {
    pub const TAKEN: usize = 4;
    pub const ALSO_MAPPED: usize = 2;
}

/// Looks for a page of ordinary memory below 1 MiB that nothing uses,
/// for `low_page` - on a PC, where another CPU is started; they begin
/// in real mode and can run nowhere else. `still_read` is what down
/// there the kernel goes on reading; of everything else the loader is
/// done.
pub fn pick_low_page(regions: &[MemoryRegion], still_read: &[Range<u64>]) {
    let usable = regions.iter().filter(|r| r.is_usable()).map(|r| r.base..r.base + r.length);
    if let Some(page) = find_free_space(usable, still_read, PAGE_SIZE, PAGE_SIZE, MIB) {
        // Firmware maps can overlap, and anything not usable wins: a
        // page that is also in a region of another kind is not free.
        let disputed = regions.iter().any(|r| !r.is_usable() && r.base < page + PAGE_SIZE && page < r.base + r.length);
        if !disputed {
            LOW_PAGE.store(page, Ordering::Relaxed);
        }
    }
}

/// Sets up physical memory, the kernel's page tables and the heap.
///
/// Everything in `memory` must be readable now and is not used after
/// the switch: on return, physical memory is reached through the
/// direct map and no other way.
pub fn init(memory: &BootMemory) {
    let regions = memory.regions;
    let usable = || regions.iter().filter(|r| r.is_usable()).map(|r| r.base..r.base + r.length);
    let unusable = || regions.iter().filter(|r| !r.is_usable()).map(|r| r.base..r.base + r.length);
    // What can be read and written before the switch, at its physical
    // address.
    let reach = mmu::early_reach();

    // ---- 1. physical frames ---------------------------------------------

    let top = usable().map(|r| r.end).max().expect("the memory map has no usable memory");
    let frame_count = (top / PAGE_SIZE) as usize;
    let words = FrameAllocator::words_for(frame_count);
    let bitmap_bytes = (words * 8) as u64;

    let layout = kernel_layout();
    let kernel = mmu::kernel_physical(layout.text.start)..mmu::kernel_physical(layout.data.end);
    // Everything that is not free, whatever kind of region it is in:
    // what the caller listed, the kernel, and whatever the map calls
    // by any name but "usable".
    let occupied = || memory.taken.iter().cloned().chain(core::iter::once(kernel.clone())).chain(unusable());

    let bitmap_address =
        find_free_space_avoiding(usable().filter_map(|r| inside(&r, &reach)), occupied, bitmap_bytes, PAGE_SIZE, u64::MAX)
            .expect("no room for the frame bitmap");
    let bitmap_range = bitmap_address..bitmap_address + bitmap_bytes;

    // SAFETY: the range is usable RAM that nothing else occupies (that is
    // what the search checked), inside the reach: readable and writable
    // at its physical address right now.
    let bitmap = unsafe { core::slice::from_raw_parts_mut(bitmap_address as *mut u64, words) };
    let mut frames = FrameAllocator::new(bitmap, frame_count);
    let reserve_occupied = |frames: &mut FrameAllocator| {
        for range in occupied().chain(core::iter::once(bitmap_range.clone())) {
            frames.reserve_range(PhysAddr::new(range.start), PhysAddr::new(range.end));
        }
    };
    // The first go: only the memory in reach.
    for range in usable().filter_map(|r| inside(&r, &reach)) {
        frames.free_range(PhysAddr::new(range.start), PhysAddr::new(range.end));
    }
    reserve_occupied(&mut frames);

    // ---- 2. the kernel's page tables -------------------------------------

    let nx = mmu::supports_no_execute();
    NO_EXECUTE.store(nx, Ordering::Relaxed);

    // SAFETY: every frame the allocator has so far is in the reach, so
    // each new table can be written at its physical address (offset 0).
    let mut tables = unsafe { PageTables::new(&mut frames, 0) }.expect("no frame for the top table");

    for region in regions {
        let interesting = region.is_usable()
            || region.kind == MEMORY_ACPI_RECLAIMABLE
            || region.kind == MEMORY_ACPI_NVS;
        if interesting {
            map_direct(&mut tables, &mut frames, region.base..region.base + region.length);
        }
    }
    for range in memory.also_mapped.iter().chain(&memory.taken).filter(|range| !range.is_empty()) {
        map_direct(&mut tables, &mut frames, range.clone());
    }

    map_kernel(&mut tables, &mut frames, layout.text.clone(), Flags::NONE);
    map_kernel(&mut tables, &mut frames, layout.rodata.clone(), no_execute());
    map_kernel(&mut tables, &mut frames, layout.data.clone(), Flags::WRITABLE | no_execute());

    // Devices the kernel is already using (its console, where that is
    // a device with registers) stay where they are.
    mmu::map_early_devices(&mut tables, &mut frames);

    // Every process shares the kernel half by copying these top-level
    // slots once, so they must never change: give each one its table
    // now. (Only where the two halves share a top table: x86.)
    tables.fill_kernel_slots(&mut frames).expect("no frames for the kernel's top-level slots");

    // The second go: the tables are built, so the memory out of reach
    // can join. Then everything occupied is marked again - a region
    // that is not usable may lie over memory just freed. (Frames taken
    // for tables are not touched by either: they are in the reach,
    // which this go leaves alone, and marking a used frame used
    // changes nothing.)
    for range in usable().flat_map(|r| outside(&r, &reach)) {
        frames.free_range(PhysAddr::new(range.start), PhysAddr::new(range.end));
    }
    reserve_occupied(&mut frames);
    BOOT_FREE_FRAMES.store(frames.free_frames() as u64, Ordering::Relaxed);
    kprintln!(
        "[memory] {} frames free of {} ({} MiB), bitmap {} KiB at {:#x}",
        frames.free_frames(),
        frames.total_frames(),
        frames.free_frames() as u64 * PAGE_SIZE / MIB,
        bitmap_bytes / 1024,
        bitmap_address
    );

    // ---- 3. switch -------------------------------------------------------

    let root = tables.root();
    KERNEL_ROOT.store(root.as_u64(), Ordering::Relaxed);
    // SAFETY: the new tables map the kernel's code, data and stack (the
    // boot stack is in .bss), its console and the direct map; nothing
    // after the switch uses a physical address as it stands - `memory`
    // and `regions` are dead.
    unsafe {
        mmu::install_kernel_tables(root.as_u64(), nx);
        set_physical_offset(DIRECT_MAP);
        tables.set_offset(DIRECT_MAP);
        let bitmap = phys_to_virt(PhysAddr::new(bitmap_address)).as_mut_ptr::<u64>();
        frames.relocate(core::slice::from_raw_parts_mut(bitmap, words));
    }
    kprintln!(
        "[memory] own page tables at {:#x}: direct map {:#x}, NX {}, write protection on",
        root,
        DIRECT_MAP,
        if nx { "on" } else { "unsupported" }
    );

    *TABLES.lock() = Some(tables);
    *FRAMES.lock() = Some(frames);

    // ---- 4. the heap -----------------------------------------------------

    assert!(HEAP.state.lock().grow(HEAP_INITIAL), "no memory for the initial heap");
    kprintln!("[memory] heap at {HEAP_START:#x}, {} KiB to start", HEAP_INITIAL / 1024);
}

// ---- the heap ---------------------------------------------------------

struct HeapState {
    heap: Heap,
    /// The first unmapped heap address.
    end: u64,
}

impl HeapState {
    /// Maps `bytes` more at the end of the heap and hands them to it.
    /// Returns whether any memory was added.
    fn grow(&mut self, bytes: u64) -> bool {
        let bytes = bytes.min(HEAP_START + HEAP_MAX - self.end);
        let mut mapped = 0;
        {
            let mut tables = TABLES.lock();
            let mut frames = FRAMES.lock();
            let (Some(tables), Some(frames)) = (tables.as_mut(), frames.as_mut()) else {
                return false; // allocating before init: no heap yet
            };
            let flags = Flags::WRITABLE | Flags::GLOBAL | no_execute();
            while mapped < bytes {
                let Some(frame) = frames.allocate() else { break };
                let virt = VirtAddr::new(self.end + mapped);
                tables
                    .map(virt, frame, PageSize::Size4K, flags, frames)
                    .unwrap_or_else(|error| panic!("heap mapping at {virt:#x} failed: {error:?}"));
                mapped += PAGE_SIZE;
            }
        }
        if mapped == 0 {
            return false;
        }
        mmu::commit_mappings();
        // SAFETY: just mapped, writable, and used by nothing else.
        unsafe { self.heap.add_region(self.end as usize, mapped as usize) };
        self.end += mapped;
        true
    }
}

struct KernelHeap {
    state: IrqLock<HeapState>,
}

/// What `alloc` - Box, Vec, String, ... - uses for memory.
#[global_allocator]
static HEAP: KernelHeap =
    KernelHeap { state: IrqLock::new(HeapState { heap: Heap::empty(), end: HEAP_START }) };

unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut state = self.state.lock();
        loop {
            if let Some(pointer) = state.heap.allocate(layout) {
                return pointer;
            }
            // Enough for this request even at its worst alignment.
            let needed = align_up(layout.size() as u64 + layout.align() as u64, HEAP_GROWTH);
            if !state.grow(needed) {
                return ptr::null_mut(); // `alloc` turns this into a panic
            }
        }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        self.state.lock().heap.deallocate(pointer, layout);
    }
}

/// (bytes in use, bytes the heap holds).
pub fn heap_usage() -> (usize, usize) {
    let state = HEAP.state.lock();
    (state.heap.used(), state.heap.total())
}

// ---- device memory ------------------------------------------------------

/// The next free address in the MMIO window. Taken before TABLES.
static MMIO_NEXT: IrqLock<u64> = IrqLock::new(MMIO_START);

/// Maps `size` bytes of device registers at physical `physical`,
/// uncached, and returns the virtual address of `physical`.
///
/// The direct map also covers most device memory, but with normal
/// (write-back) caching: a read could be answered from the cache instead
/// of the device, and writes could be delayed or merged. Registers need
/// every access to reach the device, in order.
pub fn map_mmio(physical: u64, size: u64) -> u64 {
    let start = align_down(physical, PAGE_SIZE);
    let end = align_up(physical + size, PAGE_SIZE);
    let flags = Flags::WRITABLE | Flags::NO_CACHE | Flags::WRITE_THROUGH | Flags::GLOBAL | no_execute();

    let mut next = MMIO_NEXT.lock();
    let virt = *next;
    assert!(virt + (end - start) <= MMIO_START + MMIO_SIZE, "MMIO window full");
    with_memory(|tables, frames| {
        for offset in (0..end - start).step_by(PAGE_SIZE as usize) {
            tables
                .map(VirtAddr::new(virt + offset), PhysAddr::new(start + offset), PageSize::Size4K, flags, frames)
                .unwrap_or_else(|error| panic!("MMIO mapping of {physical:#x} failed: {error:?}"));
        }
    });
    mmu::commit_mappings();
    *next += end - start;
    virt + (physical - start)
}

/// Physical memory as the ACPI parser reads it: through the direct map,
/// refusing any range the direct map does not cover - firmware tables
/// can point anywhere, and a bad pointer must be an error, not a fault.
pub struct DirectMap;

impl PhysicalMemory for DirectMap {
    fn read(&self, address: u64, buffer: &mut [u8]) -> bool {
        if buffer.is_empty() {
            return true;
        }
        let Some(end) = address.checked_add(buffer.len() as u64) else { return false };
        let mapped = with_memory(|tables, _| {
            (align_down(address, PAGE_SIZE)..end)
                .step_by(PAGE_SIZE as usize)
                .all(|page| tables.translate(phys_to_virt(PhysAddr::new(page))).is_some())
        });
        if mapped {
            // SAFETY: every page of the range is mapped, checked above.
            let source = phys_to_virt(PhysAddr::new(address)).as_ptr::<u8>();
            unsafe { core::ptr::copy_nonoverlapping(source, buffer.as_mut_ptr(), buffer.len()) };
        }
        mapped
    }
}

// ---- kernel stacks ------------------------------------------------------

/// Each thread's stack lives in its own 64 KiB slot: the lowest page is
/// left unmapped as a guard, the other 15 pages (60 KiB) are the stack.
/// Overflowing the stack touches the guard page and faults at once,
/// instead of silently overwriting the slot below - another thread's
/// stack.
///
/// A stack, once mapped, stays mapped: a thread that ends gives its
/// stack back whole, and the next thread to start takes it as it is.
/// The reason is the other CPUs. Each CPU remembers the translations it
/// has used (its TLB), and `invlpg` makes only the CPU that executes it
/// forget. Unmap a stack here and map other frames at the same address
/// later, and a CPU that once ran the old thread could still write
/// through its memory of the old frames - into memory that by then
/// belongs to someone else. Never unmapping means there is nothing to
/// forget. The price is that the memory for the most threads there
/// have ever been at once stays stacks.
const STACK_SLOT: u64 = 64 * 1024;
const STACK_SLOTS: u64 = 16 * 1024; // 1 GiB of address space
const STACK_PAGES: u64 = STACK_SLOT / PAGE_SIZE - 1;

struct Slots {
    next: u64,
    /// Slots whose thread has ended: still mapped, ready to be reused.
    free: alloc::vec::Vec<u64>,
}

/// Taken before TABLES.
static SLOTS: IrqLock<Slots> = IrqLock::new(Slots { next: 0, free: alloc::vec::Vec::new() });

/// A mapped kernel stack. Dropping it puts it aside for the next
/// thread.
pub struct KernelStack {
    slot: u64,
}

impl KernelStack {
    fn base(&self) -> u64 {
        STACKS_START + self.slot * STACK_SLOT
    }

    /// The highest address: stacks grow down from here.
    pub fn top(&self) -> u64 {
        self.base() + STACK_SLOT
    }
}

pub fn allocate_stack() -> Option<KernelStack> {
    let mut slots = SLOTS.lock();
    if let Some(slot) = slots.free.pop() {
        return Some(KernelStack { slot }); // mapped already
    }
    if slots.next == STACK_SLOTS {
        return None;
    }
    slots.next += 1;
    let stack = KernelStack { slot: slots.next - 1 };
    let flags = Flags::WRITABLE | Flags::GLOBAL | no_execute();
    let mapped = with_memory(|tables, frames| {
        for page in (stack.base() + PAGE_SIZE..stack.top()).step_by(PAGE_SIZE as usize) {
            let Some(frame) = frames.allocate() else { return false };
            tables
                .map(VirtAddr::new(page), frame, PageSize::Size4K, flags, frames)
                .unwrap_or_else(|error| panic!("stack mapping at {page:#x} failed: {error:?}"));
        }
        true
    });
    if mapped {
        mmu::commit_mappings();
        return Some(stack);
    }
    // Out of memory part-way. This slot has never been used as a stack,
    // so no CPU remembers anything about it: what was mapped can go
    // back, and so can the slot's number (it was the last one given).
    with_memory(|tables, frames| {
        for page in (stack.base() + PAGE_SIZE..stack.top()).step_by(PAGE_SIZE as usize) {
            if let Some(frame) = tables.unmap(VirtAddr::new(page)) {
                mmu::flush(page);
                frames.free(frame);
            }
        }
    });
    slots.next -= 1;
    core::mem::forget(stack);
    None
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        SLOTS.lock().free.push(self.slot);
    }
}

/// Frames nothing is using: the free ones, and those of stacks waiting
/// for a thread. What "nothing leaked" must compare - how many stacks
/// are waiting depends on how many threads have just ended.
/// The frames free once the kernel had set memory up: the memory the
/// machine has for everything else. (`total_frames` is the span of the
/// bitmap, holes and all - 5 GiB on a PC with 4, its devices just below
/// 4 GiB.)
static BOOT_FREE_FRAMES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

pub fn boot_free_frames() -> u64 {
    BOOT_FREE_FRAMES.load(Ordering::Relaxed)
}

pub fn unused_frames() -> usize {
    let waiting = SLOTS.lock().free.len();
    with_memory(|_, frames| frames.free_frames()) + waiting * STACK_PAGES as usize
}

/// Whether `address` is in the guard page of some thread's stack.
pub fn is_stack_guard(address: u64) -> bool {
    (STACKS_START..STACKS_START + STACK_SLOTS * STACK_SLOT).contains(&address)
        && (address - STACKS_START) % STACK_SLOT < PAGE_SIZE
}

/// How many stack slots have ever been used - stays low if stacks are
/// recycled.
pub fn stack_slots_used() -> u64 {
    SLOTS.lock().next
}
