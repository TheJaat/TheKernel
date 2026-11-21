//! Translation tables - for now, only the one thing the kernel needs
//! before it has a memory manager: a way to reach a device.
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

use core::arch::asm;
use core::sync::atomic::{AtomicUsize, Ordering};

use kernel64::aarch64::{descriptor, BLOCK_SIZE, DEVICE_WINDOW};

extern "C" {
    /// The table, in entry.rs.
    static mut boot_l2_device: [u64; 512];
}

/// The next entry of the table not yet used.
static NEXT: AtomicUsize = AtomicUsize::new(0);

/// Maps the 2 MiB step that holds `physical` as device memory, and
/// returns the address at which `physical` itself can now be used.
///
/// Never undone, and at most 512 times: this is for the handful of
/// devices needed before memory management starts.
pub fn map_early_device(physical: u64) -> *mut u8 {
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

// ---- the kernel's tables, through the shared page-table code ---------

use core::cell::UnsafeCell;
use core::sync::atomic::AtomicU64;

use kernel64::aarch64::{KERNEL_WINDOW, TEXT_OFFSET};
use kernel64::memory::addr::{PhysAddr, VirtAddr, PAGE_SIZE};
use kernel64::paging::{Flags, Format, FrameSource, PageSize, PageTables};

/// How far the kernel's addresses are from where its bytes really are:
/// `virtual - physical`, for anything in the image. Unknown until the
/// kernel is told where it was loaded.
static IMAGE_DELTA: AtomicU64 = AtomicU64::new(0);

/// Records where the image was loaded. Call once, first.
pub fn set_load_address(loaded_at: u64) {
    IMAGE_DELTA.store((KERNEL_WINDOW + TEXT_OFFSET).wrapping_sub(loaded_at), Ordering::Relaxed);
}

#[repr(C, align(4096))]
struct Page(UnsafeCell<[u8; 4096]>);

// SAFETY: the pages are handed out one at a time, each once (`Pool`),
// and from then on belong to whoever was given them.
unsafe impl Sync for Page {}

/// A few pages inside the kernel image, for page tables made before
/// there is a frame allocator. In `.bss`: zeroed, and already mapped.
const POOL_PAGES: usize = 8;
static POOL: [Page; POOL_PAGES] = [const { Page(UnsafeCell::new([0; 4096])) }; POOL_PAGES];
static POOL_NEXT: AtomicUsize = AtomicUsize::new(0);

/// Hands out the pool's pages, by physical address. Nothing given back
/// is reused: there are eight, for a handful of early mappings.
pub struct Pool;

impl FrameSource for Pool {
    fn allocate_frame(&mut self) -> Option<PhysAddr> {
        let page = POOL.get(POOL_NEXT.fetch_add(1, Ordering::Relaxed))?;
        Some(PhysAddr::new((page as *const Page as u64).wrapping_sub(IMAGE_DELTA.load(Ordering::Relaxed))))
    }

    fn free_frame(&mut self, _frame: PhysAddr) {}
}

/// The tree the CPU is using for the kernel's half right now - the one
/// entry.rs built by hand - as a `PageTables`, so that the ordinary
/// mapping code can add to it.
///
/// # Safety
/// One at a time: two of these alive together could change the same
/// table at once. And the frames given to it must come from the image
/// (`Pool`), since it reaches tables at `physical + IMAGE_DELTA`.
pub unsafe fn kernel_tables() -> PageTables {
    let root: u64;
    // SAFETY: reading the register changes nothing.
    unsafe { asm!("mrs {}, ttbr1_el1", out(reg) root, options(nomem, nostack, preserves_flags)) };
    // SAFETY: the root register holds a tree in this CPU's own format,
    // whose tables are all in the image; the rest is the caller's.
    unsafe {
        PageTables::adopt(PhysAddr::new(root & 0x0000_FFFF_FFFF_F000), IMAGE_DELTA.load(Ordering::Relaxed), Format::AArch64)
    }
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

/// Two addresses for the checks below, in the last 2 MiB of the
/// address space - the last entry of the device table, never used for
/// a device (`map_early_device` counts up from the first).
pub const TEST_WRITABLE: u64 = 0xFFFF_FFFF_FFE0_0000;
pub const TEST_READ_ONLY: u64 = TEST_WRITABLE + PAGE_SIZE;

/// Checks the shared page-table code against the CPU: that it reads
/// the live tables as the CPU does, and that what it writes means to
/// the CPU what was asked for.
///
/// Leaves one page mapped twice - writable and not executable at
/// `TEST_WRITABLE`, read-only at `TEST_READ_ONLY` - for the fault
/// tests to break the rules on.
pub fn selftest() {
    // SAFETY: the only one; the CPU is alone and interrupts are off.
    let mut tables = unsafe { kernel_tables() };
    let mut pool = Pool;

    // 1. Reading. Addresses of every kind the boot tables hold - code,
    // a variable, a device - and two that are not mapped. The CPU's
    // answer and the code's must be the same.
    let variable = &IMAGE_DELTA as *const AtomicU64 as u64;
    let device = map_early_device(0x0900_0000) as u64; // any device will do: it is not touched
    let addresses = [selftest as *const () as u64, variable, device + 0x123, TEST_WRITABLE, KERNEL_WINDOW - PAGE_SIZE];
    for address in addresses {
        let ours = tables.translate(VirtAddr::new(address)).map(|found| found.physical.as_u64());
        let cpus = cpu_translate(address, false);
        assert_eq!(ours, cpus, "the tables read differently from the CPU at {address:#x}");
    }
    assert!(cpu_translate(variable, false).is_some() && cpu_translate(TEST_WRITABLE, false).is_none());
    kprintln!("[paging] the tables the CPU is using, read by the kernel: {} lookups, all as the CPU does them", addresses.len());

    // 2. Writing. One page of the pool, mapped twice.
    let frame = pool.allocate_frame().expect("the pool has a page");
    let data = Flags::WRITABLE | Flags::NO_EXECUTE | Flags::GLOBAL;
    let constant = Flags::NO_EXECUTE | Flags::GLOBAL;
    tables
        .map(VirtAddr::new(TEST_WRITABLE), frame, PageSize::Size4K, data, &mut pool)
        .and_then(|()| tables.map(VirtAddr::new(TEST_READ_ONLY), frame, PageSize::Size4K, constant, &mut pool))
        .expect("mapping the test page");
    // SAFETY: both addresses were mapped a moment ago to a page of the
    // pool that nothing else has; `dsb`/`isb` as in map_early_device.
    let read_back = unsafe {
        asm!("dsb ishst", "isb", options(nostack, preserves_flags));
        (TEST_WRITABLE as *mut u64).write_volatile(0x5441_414A_2D36_3421);
        (TEST_READ_ONLY as *const u64).read_volatile()
    };
    assert_eq!(read_back, 0x5441_414A_2D36_3421, "the two mappings are not of the same page");
    assert_eq!(cpu_translate(TEST_WRITABLE, true), Some(frame.as_u64()), "the CPU would not allow a write to the writable page");
    assert_eq!(cpu_translate(TEST_READ_ONLY, false), Some(frame.as_u64()));
    assert_eq!(cpu_translate(TEST_READ_ONLY, true), None, "the CPU would allow a write to the read-only page");
    kprintln!(
        "[paging] a page at {:#x} mapped twice: written at {TEST_WRITABLE:#x}, read at {TEST_READ_ONLY:#x}, where the CPU refuses a write",
        frame.as_u64()
    );

    // 3. Removing, and changing. A third address, mapped to the page
    // and read through - so the CPU now remembers that translation -
    // then unmapped, and mapped to a different page. What is read
    // there next must come from the new page: if the CPU were not
    // made to forget (`flush`), it would go on reading the old one.
    let moved = TEST_READ_ONLY + PAGE_SIZE;
    let other = pool.allocate_frame().expect("the pool has a second page");
    tables.map(VirtAddr::new(moved), frame, PageSize::Size4K, constant, &mut pool).expect("mapping the test page");
    // SAFETY: `moved` is mapped, each time it is read, to a page of
    // the pool; `other`'s page is in the image, so it can be written at
    // its ordinary address, which is the physical one plus the delta.
    let (before, between, after) = unsafe {
        asm!("dsb ishst", "isb", options(nostack, preserves_flags));
        let before = (moved as *const u64).read_volatile();
        assert_eq!(tables.unmap(VirtAddr::new(moved)), Some(frame));
        flush(moved);
        let between = cpu_translate(moved, false);
        (other.as_u64().wrapping_add(IMAGE_DELTA.load(Ordering::Relaxed)) as *mut u64).write_volatile(0x6F74_6865_7221);
        tables.map(VirtAddr::new(moved), other, PageSize::Size4K, constant, &mut pool).expect("mapping the test page");
        asm!("dsb ishst", "isb", options(nostack, preserves_flags));
        (before, between, (moved as *const u64).read_volatile())
    };
    assert_eq!(before, 0x5441_414A_2D36_3421);
    assert_eq!(between, None, "the CPU still translates an address that was unmapped");
    assert_eq!(after, 0x6F74_6865_7221, "after being mapped to another page, the address still reads the old one");
    kprintln!(
        "[paging] a third address unmapped, then mapped to the page at {:#x}: the CPU reads the new page there",
        other.as_u64()
    );
}
