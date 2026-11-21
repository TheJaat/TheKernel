//! The MMU: what the memory manager (mm.rs) asks of this CPU, and the
//! one thing the kernel needs before there is a memory manager - a way
//! to reach a device.
//!
//! # Before the memory manager
//!
//! entry.rs left one table empty: `boot_l2_device`, 512 entries of
//! 2 MiB each, covering the top 1 GiB of the address space
//! (`DEVICE_WINDOW`). `map_early_device` fills the next free entry and
//! returns where the device now is.
//!
//! Why not reach the device at its physical address, through the low
//! mapping entry.rs also made? That mapping is ordinary cached memory,
//! and is only there to get through the first instructions; a device
//! needs the "device" kind (each access really happens, in order), and
//! the low half of the address space belongs to programs.
//!
//! # The change of tables
//!
//! mm.rs builds the kernel's real tables - 4 KiB pages, each with the
//! permissions its contents need - and calls `install_kernel_tables`.
//! The devices mapped early are mapped again in the new tables, at the
//! same addresses (`map_early_devices`), so the console never notices.
//!
//! Changing the tables under running code has a rule on this CPU,
//! "break before make": an address must not go from one valid
//! translation straight to a different one. The CPU remembers
//! translations (in its TLB), and if it ever held the old and the new
//! for one address at once - a 2 MiB one from the boot tables and a
//! 4 KiB one from the new - it may do anything. So between old and new
//! there must be a moment with *no* translation, and everything
//! remembered forgotten. For the kernel's own code that moment is a
//! problem: the code doing it would vanish from under itself.
//!
//! The way out is the low mapping entry.rs made: the image is still
//! reachable at its physical addresses, through the *other* root
//! register. `replace_kernel_tables` is called there, turns the
//! kernel's half off altogether, forgets, installs the new root, turns
//! it on, and returns - to a high address, now translated by the new
//! tables. After that the low mapping has done its last job and is
//! turned off: address 0, and every other low address, faults.

use core::arch::{asm, global_asm};
use core::ops::Range;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use kernel64::aarch64::{descriptor, BLOCK_SIZE, DEVICE_WINDOW, KERNEL_WINDOW, TEXT_OFFSET};
use kernel64::memory::addr::{phys_to_virt, PhysAddr, VirtAddr, PAGE_SIZE};
use kernel64::memory::frame::FrameAllocator;
use kernel64::paging::{Flags, PageSize, PageTables};

use crate::mm;

extern "C" {
    /// The table, in entry.rs.
    static mut boot_l2_device: [u64; 512];
}

/// The next entry of the table not yet used.
static NEXT: AtomicUsize = AtomicUsize::new(0);
/// Set when the kernel's own tables take over: the boot table is then
/// no longer part of what the CPU uses, and writing to it maps nothing.
static EARLY_OVER: AtomicBool = AtomicBool::new(false);

/// Maps the 2 MiB step that holds `physical` as device memory, and
/// returns the address at which `physical` itself can now be used.
///
/// Never undone, and at most 512 times: this is for the handful of
/// devices needed before memory management starts. After it has
/// started, `mm::map_mmio` is the way.
pub fn map_early_device(physical: u64) -> *mut u8 {
    assert!(!EARLY_OVER.load(Ordering::Relaxed), "map_early_device after the memory manager started");
    let slot = NEXT.fetch_add(1, Ordering::Relaxed);
    assert!(slot < 512, "no room left in the early device window");
    let step = physical & !(BLOCK_SIZE - 1);
    // SAFETY: the table is in the kernel's .bss, mapped and writable;
    // `slot` is inside it and - NEXT only counts up - was never used,
    // so no CPU can be relying on what the entry held. Going from "not
    // mapped" to "mapped" needs nothing forgotten: the CPU never
    // remembers a missing entry. `dsb` waits for the write to have
    // happened and `isb` makes the next instruction see it.
    unsafe {
        (&raw mut boot_l2_device).cast::<u64>().add(slot).write_volatile(step | descriptor::DEVICE_BLOCK);
        asm!("dsb ishst", "isb", options(nostack, preserves_flags));
    }
    (DEVICE_WINDOW + slot as u64 * BLOCK_SIZE + (physical - step)) as *mut u8
}

// ---- what the memory manager asks of any CPU ---------------------------

/// How far the kernel's addresses are from where its bytes really are:
/// `virtual - physical`, for anything in the image. Unknown until the
/// kernel is told where it was loaded.
static IMAGE_DELTA: AtomicU64 = AtomicU64::new(0);
/// Where the image was loaded.
static LOADED_AT: AtomicU64 = AtomicU64::new(0);

/// Records where the image was loaded. Call once, first.
pub fn set_load_address(loaded_at: u64) {
    LOADED_AT.store(loaded_at, Ordering::Relaxed);
    IMAGE_DELTA.store((KERNEL_WINDOW + TEXT_OFFSET).wrapping_sub(loaded_at), Ordering::Relaxed);
}

/// Where the byte the kernel sees at `virt`, an address inside its own
/// image, really is. Not a constant as on a PC: an AArch64 machine has
/// its memory wherever it likes, and the loader puts the image where
/// it likes in that.
pub fn kernel_physical(virt: u64) -> u64 {
    virt.wrapping_sub(IMAGE_DELTA.load(Ordering::Relaxed))
}

/// The physical memory that can be read and written, at its physical
/// address, before the kernel has page tables of its own: the one GiB
/// the image is in, which entry.rs mapped to itself. (It mapped the
/// GiB of the device tree too, but that need not be memory.)
pub fn early_reach() -> Range<u64> {
    const GIB: u64 = 1 << 30;
    let start = LOADED_AT.load(Ordering::Relaxed) & !(GIB - 1);
    start..start + GIB
}

/// Every AArch64 CPU can forbid execution page by page.
pub fn supports_no_execute() -> bool {
    true
}

/// Maps the devices `map_early_device` mapped again, in `tables`, at
/// the same addresses - so that whoever was given a pointer to one can
/// go on using it when `tables` take over. From here on
/// `map_early_device` refuses.
pub fn map_early_devices(tables: &mut PageTables, frames: &mut FrameAllocator) {
    EARLY_OVER.store(true, Ordering::Relaxed);
    let flags = Flags::WRITABLE | Flags::NO_CACHE | Flags::WRITE_THROUGH | Flags::GLOBAL | Flags::NO_EXECUTE;
    for slot in 0..NEXT.load(Ordering::Relaxed).min(512) {
        // SAFETY: the table is in the kernel's .bss and `slot` is
        // inside it; nothing writes to it any more.
        let entry = unsafe { (&raw const boot_l2_device).cast::<u64>().add(slot).read_volatile() };
        // Bits 21-47 of a 2 MiB entry are where it points.
        let physical = PhysAddr::new(entry & 0x0000_FFFF_FFE0_0000);
        let virt = VirtAddr::new(DEVICE_WINDOW + slot as u64 * BLOCK_SIZE);
        tables
            .map(virt, physical, PageSize::Size2M, flags, frames)
            .unwrap_or_else(|error| panic!("mapping the early device at {physical:#x} again failed: {error:?}"));
    }
}

global_asm!(
    r#"
    .section .text
    .balign 4
    .global replace_kernel_tables
    // x0: the physical address of the new top table for the kernel's
    // half. Must be called at this code's PHYSICAL address (through
    // the low mapping), with interrupts off. Touches no memory: while
    // the kernel's half is off there is no stack to touch. Every
    // instruction is position-independent - there is not one address
    // in it.
replace_kernel_tables:
    mrs     x9, tcr_el1
    orr     x10, x9, #(1 << 23)     // EPD1: look nothing up through TTBR1
    msr     tcr_el1, x10
    isb                             // the kernel's half no longer exists
    tlbi    vmalle1                 // forget every translation remembered
    dsb     nsh                     // ...and wait until that is done
    isb
    msr     ttbr1_el1, x0           // the new tables
    msr     tcr_el1, x9             // the kernel's half exists again
    isb
    ret                             // to a high address: the new tables' first use
"#
);

extern "C" {
    fn replace_kernel_tables(root: u64);
}

/// Makes the tree at physical `root` the kernel's page tables, and
/// turns the low half of the address space off.
///
/// # Safety
/// The tree must map everything the kernel uses from here on, where it
/// is now: its code, its data, the stack this runs on, the console.
/// Nothing may be using a low (physical) address any more. One CPU,
/// interrupts off.
pub unsafe fn install_kernel_tables(root: u64, _no_execute: bool) {
    // The routine's low address: where its bytes really are.
    let low = kernel_physical(replace_kernel_tables as *const () as u64);
    assert!(early_reach().contains(&low), "the kernel image runs over the end of the one GiB mapped low");
    // SAFETY: `low` is the routine's own code, seen through the low
    // mapping, which is executable; it takes one number and follows
    // the C convention as far as it uses anything (x0, x9, x10, x30).
    let replace: extern "C" fn(u64) = unsafe { core::mem::transmute(low as *const ()) };
    replace(root);

    // The low mapping has done its last job. Turn that half off too
    // (EPD0, bit 7) and forget what was remembered of it; it comes
    // back when there is a program to run in it.
    // SAFETY: the caller promised nothing uses a low address.
    unsafe {
        asm!(
            "mrs {t}, tcr_el1",
            "orr {t}, {t}, #(1 << 7)",
            "msr tcr_el1, {t}",
            "isb",
            "tlbi vmalle1",
            "dsb nsh",
            "isb",
            t = out(reg) _,
            options(nostack, preserves_flags),
        );
    }
}

/// Switches the low half of the address space to the tree at physical
/// `root` - a process's - or, given the kernel's own root, to nothing.
///
/// The kernel's half never changes: it is in the other root register
/// (`TTBR1_EL1`), which is why a process's tree holds no copy of the
/// kernel's mappings here, as it must on x86.
///
/// The order is the one the change of the kernel's tables used
/// (`replace_kernel_tables`), for the same reason: the low half is
/// switched off, what the CPU remembers of it is forgotten, and only
/// then does the new tree go in - so there is no moment at which the
/// CPU could hold one process's translation for an address while
/// walking another's tables. Forgetting *everything* at each switch is
/// the plain way to do it; the cheaper way is to give each process a
/// number (an ASID) that tags what the CPU remembers, which can come
/// when it matters.
///
/// # Safety
/// The tree must be a process's, made by `new_user`, or the kernel's.
/// The caller must not be using a low address.
pub unsafe fn activate(root: u64) {
    let off = root == mm::kernel_root();
    // SAFETY: the caller's promise. This code, its stack and its data
    // are in the kernel's half, which none of this touches.
    unsafe {
        asm!(
            "mrs {t}, tcr_el1",
            "orr {t}, {t}, #(1 << 7)",  // EPD0: look nothing up through TTBR0
            "msr tcr_el1, {t}",
            "isb",
            "tlbi vmalle1",
            "dsb nsh",
            "isb",
            "cbnz {off}, 2f",
            "msr ttbr0_el1, {root}",
            "bic {t}, {t}, #(1 << 7)",
            "msr tcr_el1, {t}",
            "isb",
            "2:",
            t = out(reg) _,
            off = in(reg) u64::from(off),
            root = in(reg) root,
            options(nostack),
        );
    }
}

/// Called after instructions were written to memory - a program's
/// code, copied into fresh pages - before they are run.
///
/// This CPU keeps two caches, one for data and one for instructions,
/// and does not keep them in step: bytes written through the data
/// cache may not have reached memory, and the instruction cache may
/// still hold what those addresses held before. x86 does this by
/// itself. Here it is two steps: push the written lines out of the
/// data cache (`dc cvau`, line by line), then empty the instruction
/// cache (`ic ialluis`: all of it, on every CPU - cruder than line by
/// line, but right whatever address the program will run the code at,
/// which is not the address it was written through).
pub fn code_written(start: *const u8, length: usize) {
    let kind: u64;
    // SAFETY: reading the register that describes the caches.
    unsafe { asm!("mrs {}, ctr_el0", out(reg) kind, options(nomem, nostack, preserves_flags)) };
    // Bits 16-19: the data cache's line, as log2 of its size in words.
    let line = 4usize << ((kind >> 16) & 0xF);
    let end = start as usize + length;
    let mut address = start as usize & !(line - 1);
    while address < end {
        // SAFETY: only moves cached bytes to memory; the address is
        // inside what the caller wrote, so it is mapped.
        unsafe { asm!("dc cvau, {}", in(reg) address, options(nostack, preserves_flags)) };
        address += line;
    }
    // SAFETY: only discards cached instructions.
    unsafe { asm!("dsb ish", "ic ialluis", "dsb ish", "isb", options(nostack, preserves_flags)) };
}

/// Called after mappings are added, before they are used. This CPU
/// may go on for a while not seeing a write to a table (the write can
/// sit in a buffer, and instructions already under way looked before
/// it): `dsb` waits until the write has happened, `isb` makes
/// everything after it start afresh.
pub fn commit_mappings() {
    // SAFETY: both only wait.
    unsafe { asm!("dsb ishst", "isb", options(nostack, preserves_flags)) };
}

/// Asks the CPU itself what `virt` translates to, for a read or for a
/// write by the kernel: the `at` ("address translate") instruction
/// does the walk the CPU would do for a real access, permissions
/// included, and reports in a register instead of faulting.
pub fn cpu_translate(virt: u64, write: bool) -> Option<u64> {
    let result: u64;
    // SAFETY: `at` touches no memory at `virt`; it only walks the
    // tables. The answer is in PAR_EL1 once `isb` has let it settle.
    unsafe {
        if write {
            asm!("at s1e1w, {}", "isb", "mrs {}, par_el1", in(reg) virt, out(reg) result, options(nostack, preserves_flags));
        } else {
            asm!("at s1e1r, {}", "isb", "mrs {}, par_el1", in(reg) virt, out(reg) result, options(nostack, preserves_flags));
        }
    }
    // Bit 0 set: the access would fault. Else bits 12-47 are the page.
    if result & 1 != 0 {
        None
    } else {
        Some((result & 0x0000_FFFF_FFFF_F000) | (virt & 0xFFF))
    }
}

/// Makes every CPU forget what it remembers of the translation of the
/// page at `virt` - needed after a mapping is removed or changed, or
/// the old one may go on being used.
pub fn flush(virt: u64) {
    // SAFETY: only discards remembered translations. The instruction
    // takes the page number; `dsb` waits until every CPU has done it.
    unsafe {
        asm!("tlbi vaae1is, {}", "dsb ish", "isb", in(reg) (virt >> 12) & 0xFFF_FFFF_FFFF, options(nostack, preserves_flags));
    }
}

// ---- checked against the CPU -------------------------------------------

/// An address nothing else uses (2 MiB above the one selftest.rs has).
const TEST_PAGE: u64 = 0xFFFF_D000_0020_0000;

/// An immutable static, so it lands in .rodata.
static READ_ONLY: u64 = 0x0063_6F6E_7374;

/// What selftest.rs checks, it checks by reading the tables with the
/// kernel's own code. This asks the CPU: that it reads the kernel's
/// tables as that code does, that it would refuse what they forbid,
/// that the low half is gone, and that a changed mapping is forgotten
/// when it is told to forget.
pub fn selftest() {
    let boxed = alloc::boxed::Box::new(0x6865_6170u64);
    let code = selftest as *const () as u64;
    let constant = &raw const READ_ONLY as u64;
    let variable = &raw const IMAGE_DELTA as u64;
    let heap = &raw const *boxed as u64;
    let direct = mm::DIRECT_MAP + kernel_physical(constant);
    let device = DEVICE_WINDOW + 0x123;
    let guard = mm::kernel_layout().stack_guard;

    // 1. Reading: every kind of mapping the kernel has, and two
    // addresses that have none.
    let addresses = [code, constant, variable, heap, direct, device, guard, KERNEL_WINDOW - PAGE_SIZE];
    mm::with_memory(|tables, _| {
        for address in addresses {
            let ours = tables.translate(VirtAddr::new(address)).map(|found| found.physical.as_u64());
            let cpus = cpu_translate(address, false);
            assert_eq!(ours, cpus, "the tables read differently from the CPU at {address:#x}");
        }
    });
    assert_eq!(cpu_translate(constant, false), Some(kernel_physical(constant)));
    assert_eq!(cpu_translate(direct, false), Some(kernel_physical(constant)), "the direct map is not direct");
    assert_eq!(cpu_translate(guard, false), None, "the page under the boot stack is mapped");
    // SAFETY: the direct map shows all memory, the kernel's image
    // included; this reads the constant through it.
    assert_eq!(unsafe { (direct as *const u64).read_volatile() }, READ_ONLY);
    kprintln!("[paging] the kernel's own tables, read by the kernel and by the CPU: {} lookups, all the same", addresses.len());

    // 2. Permissions, as the CPU would apply them to a write.
    assert_eq!(cpu_translate(code, true), None, "the CPU would let the kernel write to its code");
    assert_eq!(cpu_translate(constant, true), None, "the CPU would let the kernel write to its constants");
    assert!(cpu_translate(variable, true).is_some(), "the CPU would not let the kernel write to its variables");
    assert!(cpu_translate(heap, true).is_some(), "the CPU would not let the kernel write to its heap");
    kprintln!("[paging] the CPU refuses a write to code and to constants, and allows one to variables and the heap");

    // 3. The low half. The kernel's own bytes were readable at their
    // physical address until a moment ago.
    let low = kernel_physical(code);
    assert_eq!(cpu_translate(0, false), None, "address 0 is mapped");
    assert_eq!(cpu_translate(low, false), None, "the low mapping the kernel started on is still there");
    kprintln!("[paging] the low half is off: address 0 and {low:#x}, where this code really is, translate to nothing");

    // 4. Removing, and changing. An address mapped to one page and
    // read through - so the CPU now remembers that translation - then
    // unmapped, and mapped to a different page. What is read there
    // next must come from the new page: if the CPU were not made to
    // forget (`flush`), it would go on reading the old one.
    let (first, second) = mm::with_memory(|tables, frames| {
        let first = frames.allocate().expect("no free frame");
        let second = frames.allocate().expect("no free frame");
        let flags = Flags::GLOBAL | Flags::NO_EXECUTE;
        // SAFETY: both frames were allocated a moment ago, so nothing
        // else uses them; they are written through the direct map and
        // read at TEST_PAGE, which is mapped to one of them each time
        // it is read.
        unsafe {
            phys_to_virt(first).as_mut_ptr::<u64>().write_volatile(0x0066_6972_7374);
            phys_to_virt(second).as_mut_ptr::<u64>().write_volatile(0x7365_636F_6E64);
            tables.map(VirtAddr::new(TEST_PAGE), first, PageSize::Size4K, flags, frames).expect("mapping the test page");
            commit_mappings();
            assert_eq!((TEST_PAGE as *const u64).read_volatile(), 0x0066_6972_7374);
            assert_eq!(cpu_translate(TEST_PAGE, true), None, "the CPU would allow a write to a read-only page");

            assert_eq!(tables.unmap(VirtAddr::new(TEST_PAGE)), Some(first));
            flush(TEST_PAGE);
            assert_eq!(cpu_translate(TEST_PAGE, false), None, "the CPU still translates an address that was unmapped");

            tables.map(VirtAddr::new(TEST_PAGE), second, PageSize::Size4K, flags, frames).expect("mapping the test page");
            commit_mappings();
            let read = (TEST_PAGE as *const u64).read_volatile();
            assert_eq!(read, 0x7365_636F_6E64, "after being mapped to another page, the address still reads the old one");

            assert_eq!(tables.unmap(VirtAddr::new(TEST_PAGE)), Some(second));
            flush(TEST_PAGE);
        }
        frames.free(first);
        frames.free(second);
        (first, second)
    });
    kprintln!(
        "[paging] one address mapped to the page at {:#x}, then to the one at {:#x}: the CPU reads the new page there",
        first.as_u64(),
        second.as_u64()
    );
}
