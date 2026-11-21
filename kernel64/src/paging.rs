//! Page tables: the map from virtual to physical addresses.
//!
//! x86_64 and AArch64 walk the same shape of tree - four levels, 512
//! entries each, nine bits of the address per level - and differ only
//! in what the bits of one entry mean. So there is one implementation,
//! and a `Format` that says how to write an entry and how to read one
//! back. Callers ask for what they want in the kernel's own terms
//! (`Flags`: writable, user, no-execute...) and never see either
//! CPU's bits.
//!
//! The description below uses x86_64's names for the levels; AArch64
//! calls them level 0 (the root) to level 3.
//!
//! A virtual address is split into four 9-bit indices and a 12-bit
//! offset. Each index selects one of 512 entries in a table one 4 KiB
//! page long; each entry points at the next table down:
//!
//!   bits 47-39  PML4 index  -> a PDPT
//!   bits 38-30  PDPT index  -> a PD      (or a 1 GiB page, if HUGE)
//!   bits 29-21  PD   index  -> a PT      (or a 2 MiB page, if HUGE)
//!   bits 20-12  PT   index  -> a 4 KiB page
//!   bits 11-0   offset within the page
//!
//! CR3 holds the physical address of the PML4. The tables hold physical
//! addresses too - the CPU walks them without translation - so the
//! kernel, which can only use virtual addresses, has to reach each table
//! through some mapping of physical memory. That is `offset`: table at
//! physical `p` is read at virtual `p + offset`. While the bootloader's
//! identity map is in use the offset is 0; afterwards it is the start of
//! the kernel's direct map.
//!
//! This module is pure logic over memory - it never loads CR3 or flushes
//! the TLB. That is the arch layer's job, and it is what lets the host
//! tests run this against a Vec standing in for physical RAM.

use core::fmt;
use core::ops::{BitOr, BitOrAssign};

use crate::memory::addr::{PhysAddr, VirtAddr, PAGE_SIZE};
use crate::memory::frame::FrameAllocator;

/// What a mapping allows, and how it is to be treated.
///
/// These are the kernel's own terms. The numbers happen to be the bits
/// of an x86_64 entry, which is where this code began, so on x86_64
/// writing an entry is a copy; `Format` turns them into AArch64's bits,
/// several of which have the opposite sense. The comments give the
/// x86_64 meaning.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Flags(u64);

impl Flags {
    pub const NONE: Flags = Flags(0);
    /// The entry is valid. Without it the CPU ignores every other bit.
    pub const PRESENT: Flags = Flags(1 << 0);
    /// Writes allowed. In ring 0 this is only enforced with CR0.WP set.
    pub const WRITABLE: Flags = Flags(1 << 1);
    /// Ring 3 may use it. Must be set at every level of the walk.
    pub const USER: Flags = Flags(1 << 2);
    /// PWT: writes go straight to memory instead of staying in the cache.
    pub const WRITE_THROUGH: Flags = Flags(1 << 3);
    /// PCD: no caching at all. With WRITE_THROUGH, the default PAT makes
    /// this "uncacheable" - what device registers need: every read must
    /// reach the device, and every write must leave the CPU, in order.
    pub const NO_CACHE: Flags = Flags(1 << 4);
    /// In a PDPT or PD entry: this is the page itself, not a table.
    pub const HUGE: Flags = Flags(1 << 7);
    /// Survives a CR3 reload in the TLB: for kernel mappings, which every
    /// address space shares. Needs CR4.PGE.
    pub const GLOBAL: Flags = Flags(1 << 8);
    /// Instruction fetches fault. Needs EFER.NXE - without it this bit is
    /// reserved and setting it faults on every access.
    pub const NO_EXECUTE: Flags = Flags(1 << 63);
    /// Ignored by the CPU (bits 9-11 are the OS's to use). Marks a leaf
    /// whose frame belongs to the address space: tearing the space down
    /// frees it. A mapping of someone else's memory - a device, shared
    /// memory - is left without it, and its frame survives.
    pub const OWNED: Flags = Flags(1 << 9);

    pub const fn bits(self) -> u64 {
        self.0
    }
    pub const fn contains(self, other: Flags) -> bool {
        self.0 & other.0 == other.0
    }
    pub const fn without(self, other: Flags) -> Flags {
        Flags(self.0 & !other.0)
    }
}

impl BitOr for Flags {
    type Output = Flags;
    fn bitor(self, other: Flags) -> Flags {
        Flags(self.0 | other.0)
    }
}

impl BitOrAssign for Flags {
    fn bitor_assign(&mut self, other: Flags) {
        self.0 |= other.0;
    }
}

impl fmt::Debug for Flags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names = [
            (Flags::PRESENT, "P"),
            (Flags::WRITABLE, "W"),
            (Flags::USER, "U"),
            (Flags::WRITE_THROUGH, "PWT"),
            (Flags::NO_CACHE, "PCD"),
            (Flags::HUGE, "H"),
            (Flags::GLOBAL, "G"),
            (Flags::OWNED, "OWN"),
            (Flags::NO_EXECUTE, "NX"),
        ];
        let mut first = true;
        for (flag, name) in names {
            if self.contains(flag) {
                if !first {
                    f.write_str("|")?;
                }
                f.write_str(name)?;
                first = false;
            }
        }
        if first {
            f.write_str("-")?;
        }
        Ok(())
    }
}

/// The physical address in a table entry: bits 12-51.
const ADDRESS_MASK: u64 = 0x000F_FFFF_FFFF_F000;
/// Everything in an entry that is not the address.
const FLAG_MASK: u64 = !ADDRESS_MASK;

/// The bits of an AArch64 entry ("descriptor"). The manual's names are
/// in brackets.
mod a64 {
    /// The entry is in use.
    pub const VALID: u64 = 1 << 0;
    /// Set: a pointer to the next table - or, at the last level, a
    /// page. Clear, above the last level: a block, the page itself.
    /// (So "this is a big page" is a bit being *clear*, where x86 sets
    /// one.)
    pub const TABLE_OR_PAGE: u64 = 1 << 1;
    /// Which kind of memory, as an index into the MAIR register
    /// [AttrIndx]: 0 ordinary, 1 a device, 2 ordinary but not cached.
    pub const KIND_DEVICE: u64 = 1 << 2;
    pub const KIND_UNCACHED: u64 = 2 << 2;
    pub const KIND_MASK: u64 = 0b111 << 2;
    /// Programs may use it [AP[1]].
    pub const USER: u64 = 1 << 6;
    /// Nobody may write to it [AP[2]] - the opposite sense from x86's
    /// "writable".
    pub const READ_ONLY: u64 = 1 << 7;
    /// Other CPUs may have it in their caches [SH = inner shareable].
    pub const SHARED: u64 = 0b11 << 8;
    /// "Has been used" [AF]. Clear, the first use faults; set at once
    /// here, since nothing asks which pages are used.
    pub const ACCESSED: u64 = 1 << 10;
    /// Belongs to one address space only [nG] - again the opposite
    /// sense from x86's "global".
    pub const NOT_GLOBAL: u64 = 1 << 11;
    /// The kernel may not execute it [PXN]; programs may not [UXN].
    /// Two bits where x86 has one.
    pub const NO_KERNEL_EXECUTE: u64 = 1 << 53;
    pub const NO_USER_EXECUTE: u64 = 1 << 54;
    /// Ignored by the CPU (bits 55-58 are the OS's): see `Flags::OWNED`.
    pub const OWNED: u64 = 1 << 55;
    /// The physical address: bits 12-47.
    pub const ADDRESS_MASK: u64 = 0x0000_FFFF_FFFF_F000;
}

/// Which CPU's entries to write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    X86_64,
    AArch64,
}

impl Format {
    /// The format of the CPU this is compiled for. (The host tests are
    /// compiled for a PC and ask for the other one by name.)
    pub const NATIVE: Format = if cfg!(target_arch = "aarch64") { Format::AArch64 } else { Format::X86_64 };

    /// Whether the entry is in use. Bit 0 on both.
    fn is_present(self, entry: u64) -> bool {
        entry & 1 != 0
    }

    /// Whether a present entry above the last level points at a table
    /// (rather than being a big page itself).
    fn is_table(self, entry: u64) -> bool {
        match self {
            Format::X86_64 => entry & Flags::HUGE.bits() == 0,
            Format::AArch64 => entry & a64::TABLE_OR_PAGE != 0,
        }
    }

    /// The physical address in an entry that points at a table or a
    /// 4 KiB page.
    fn address(self, entry: u64) -> u64 {
        match self {
            Format::X86_64 => entry & ADDRESS_MASK,
            Format::AArch64 => entry & a64::ADDRESS_MASK,
        }
    }

    /// An entry that points at the table in `frame`.
    ///
    /// Intermediate entries are permissive - writable, executable - and
    /// the leaf decides. The CPU takes the most restrictive combination
    /// along the walk, so this restricts nothing.
    fn table_entry(self, frame: PhysAddr, user: bool) -> u64 {
        match self {
            Format::X86_64 => {
                let mut flags = Flags::PRESENT | Flags::WRITABLE;
                if user {
                    flags |= Flags::USER;
                }
                frame.as_u64() | flags.bits()
            }
            // An AArch64 table entry restricts nothing unless asked to.
            Format::AArch64 => frame.as_u64() | a64::VALID | a64::TABLE_OR_PAGE,
        }
    }

    /// An existing table entry, changed so that a user mapping below it
    /// works. x86 needs USER at every level, not just the leaf; AArch64
    /// looks only at the leaf.
    fn table_entry_for_user(self, entry: u64) -> u64 {
        match self {
            Format::X86_64 => entry | Flags::USER.bits(),
            Format::AArch64 => entry,
        }
    }

    /// The entry that maps a page of `size` at `phys` with `flags`.
    fn leaf_entry(self, phys: PhysAddr, size: PageSize, flags: Flags) -> u64 {
        match self {
            Format::X86_64 => {
                let huge = match size {
                    PageSize::Size2M => Flags::HUGE,
                    PageSize::Size4K => Flags::NONE,
                };
                // HUGE means "PAT" in a 4 KiB entry; never let a caller set it there.
                let flags = (flags.without(Flags::HUGE) | Flags::PRESENT | huge).bits();
                phys.as_u64() | (flags & FLAG_MASK)
            }
            Format::AArch64 => {
                let user = flags.contains(Flags::USER);
                let mut entry = phys.as_u64() | a64::VALID | a64::ACCESSED;
                if size == PageSize::Size4K {
                    entry |= a64::TABLE_OR_PAGE;
                }
                if user {
                    entry |= a64::USER;
                }
                if !flags.contains(Flags::WRITABLE) {
                    entry |= a64::READ_ONLY;
                }
                if !flags.contains(Flags::GLOBAL) {
                    entry |= a64::NOT_GLOBAL;
                }
                // What x86 says with two cache bits: uncached is a
                // device; write-through alone is memory that must not
                // sit in the cache (a framebuffer).
                entry |= if flags.contains(Flags::NO_CACHE) {
                    a64::KIND_DEVICE
                } else if flags.contains(Flags::WRITE_THROUGH) {
                    a64::KIND_UNCACHED | a64::SHARED
                } else {
                    a64::SHARED
                };
                // Execution is allowed to at most one side: a page the
                // kernel may run is never runnable by programs, and the
                // kernel never runs a program's page.
                entry |= if flags.contains(Flags::NO_EXECUTE) {
                    a64::NO_KERNEL_EXECUTE | a64::NO_USER_EXECUTE
                } else if user {
                    a64::NO_KERNEL_EXECUTE
                } else {
                    a64::NO_USER_EXECUTE
                };
                if flags.contains(Flags::OWNED) {
                    entry |= a64::OWNED;
                }
                entry
            }
        }
    }

    /// A leaf entry read back as `Flags`. `big` says it was found above
    /// the last level.
    fn flags(self, entry: u64, big: bool) -> Flags {
        match self {
            Format::X86_64 => Flags(entry & FLAG_MASK),
            Format::AArch64 => {
                let user = entry & a64::USER != 0;
                let mut flags = Flags::PRESENT;
                if big {
                    flags |= Flags::HUGE;
                }
                if user {
                    flags |= Flags::USER;
                }
                if entry & a64::READ_ONLY == 0 {
                    flags |= Flags::WRITABLE;
                }
                if entry & a64::NOT_GLOBAL == 0 {
                    flags |= Flags::GLOBAL;
                }
                match entry & a64::KIND_MASK {
                    a64::KIND_DEVICE => flags |= Flags::NO_CACHE | Flags::WRITE_THROUGH,
                    a64::KIND_UNCACHED => flags |= Flags::WRITE_THROUGH,
                    _ => {}
                }
                // "No execute" for whoever the page is for.
                let no_execute = if user { a64::NO_USER_EXECUTE } else { a64::NO_KERNEL_EXECUTE };
                if entry & no_execute != 0 {
                    flags |= Flags::NO_EXECUTE;
                }
                if entry & a64::OWNED != 0 {
                    flags |= Flags::OWNED;
                }
                flags
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageSize {
    Size4K,
    Size2M,
}

impl PageSize {
    pub const fn bytes(self) -> u64 {
        match self {
            PageSize::Size4K => PAGE_SIZE,
            PageSize::Size2M => 2 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapError {
    /// A new table was needed and there was no frame for it.
    OutOfFrames,
    /// Something is already mapped at this address.
    AlreadyMapped,
    /// A 4 KiB mapping was asked for inside an existing huge page.
    HugePageInTheWay,
    /// The virtual or physical address is not aligned to the page size.
    Misaligned,
}

/// Where page-table frames come from. A trait rather than the frame
/// allocator itself so tests (and later, per-process quotas) can supply
/// their own.
pub trait FrameSource {
    fn allocate_frame(&mut self) -> Option<PhysAddr>;
    fn free_frame(&mut self, frame: PhysAddr);
}

impl FrameSource for FrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysAddr> {
        self.allocate()
    }
    fn free_frame(&mut self, frame: PhysAddr) {
        self.free(frame);
    }
}

/// PML4 slots 256-511 map the upper half - the kernel. Every address
/// space shares them.
const KERNEL_SLOTS: core::ops::Range<usize> = 256..512;

/// What a virtual address maps to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Translation {
    pub physical: PhysAddr,
    /// The leaf entry's flags.
    pub flags: Flags,
    /// The size of the page it lies in: 4 KiB, 2 MiB or 1 GiB.
    pub page_size: u64,
}

type Table = [u64; 512];

fn index(virt: VirtAddr, level: u32) -> usize {
    // level 4 = PML4 (bits 47-39) ... level 1 = PT (bits 20-12)
    ((virt.as_u64() >> (12 + 9 * (level - 1))) & 511) as usize
}

/// One address space: a PML4 and everything under it.
pub struct PageTables {
    root: PhysAddr,
    offset: u64,
    format: Format,
}

impl PageTables {
    /// Creates an empty address space - a zeroed PML4.
    ///
    /// # Safety
    /// For every frame `frames` hands out, now and in later calls,
    /// `frame + offset` must be a writable virtual address of that frame.
    pub unsafe fn new(frames: &mut impl FrameSource, offset: u64) -> Option<PageTables> {
        // SAFETY: the caller's promise, passed on.
        unsafe { PageTables::new_in(frames, offset, Format::NATIVE) }
    }

    /// `new`, for a named format.
    ///
    /// # Safety
    /// As for `new`.
    pub unsafe fn new_in(frames: &mut impl FrameSource, offset: u64, format: Format) -> Option<PageTables> {
        let root = frames.allocate_frame()?;
        let tables = PageTables { root, offset, format };
        tables.table(root).fill(0);
        Some(tables)
    }

    /// Takes over a tree that already exists - one the CPU may be
    /// using this very moment - to add to it.
    ///
    /// # Safety
    /// `root` must be the root of a well-formed tree in `format`, every
    /// table of which is at `its address + offset`; and as for `new`.
    /// Nothing else may change the tree while this value exists.
    pub unsafe fn adopt(root: PhysAddr, offset: u64, format: Format) -> PageTables {
        PageTables { root, offset, format }
    }

    /// A new address space for a process: an empty lower half, and the
    /// same upper half as `kernel` - the very same PDPTs, so the kernel is
    /// mapped identically everywhere and a change to the kernel's
    /// mappings below PML4 level shows up in every process at once.
    ///
    /// That only holds if `kernel` never adds a new PML4 slot later;
    /// `fill_kernel_slots` makes sure it never needs to.
    ///
    /// # Safety
    /// As for `new`.
    pub unsafe fn new_user(
        frames: &mut impl FrameSource,
        offset: u64,
        kernel: &PageTables,
    ) -> Option<PageTables> {
        // SAFETY: the caller's promise, passed on.
        let tables = unsafe { PageTables::new_in(frames, offset, kernel.format) }?;
        // On AArch64 there is nothing to share: the CPU has a second
        // root register for the upper half, and the kernel's tree
        // stays in it while this one comes and goes in the first.
        if kernel.format == Format::X86_64 {
            let source = kernel.table(kernel.root);
            let target = tables.table(tables.root);
            target[KERNEL_SLOTS].copy_from_slice(&source[KERNEL_SLOTS]);
        }
        Some(tables)
    }

    /// Gives every empty upper-half PML4 slot a (empty) PDPT, so the
    /// kernel's PML4 never changes again - which is what lets
    /// `new_user` copy it once. Costs one frame per empty slot.
    pub fn fill_kernel_slots(&mut self, frames: &mut impl FrameSource) -> Result<(), MapError> {
        if self.format == Format::AArch64 {
            return Ok(()); // never copied: see `new_user`
        }
        let root = self.table(self.root);
        for slot in KERNEL_SLOTS {
            self.next_table(root, slot, false, frames)?;
        }
        Ok(())
    }

    /// Frees the lower half: every page table, and every OWNED frame.
    /// The upper half belongs to the kernel and is left alone.
    pub fn destroy(self, frames: &mut impl FrameSource) {
        let root = self.table(self.root);
        for entry in &mut root[..KERNEL_SLOTS.start] {
            if self.format.is_present(*entry) {
                self.free_table(PhysAddr::new(self.format.address(*entry)), 3, frames);
                *entry = 0;
            }
        }
        frames.free_frame(self.root);
    }

    /// Frees the table at `frame` (at `level`: 3 = PDPT ... 1 = PT), what
    /// it points to, and the owned frames below it.
    fn free_table(&self, frame: PhysAddr, level: u32, frames: &mut impl FrameSource) {
        let table = self.table(frame);
        for &entry in table.iter() {
            if !self.format.is_present(entry) {
                continue;
            }
            let leaf = level == 1 || !self.format.is_table(entry);
            if !leaf {
                self.free_table(PhysAddr::new(self.format.address(entry)), level - 1, frames);
            } else if self.format.flags(entry, level > 1).contains(Flags::OWNED) {
                // Owned huge pages are not used; a 4 KiB frame each.
                frames.free_frame(PhysAddr::new(self.format.address(entry)));
            }
        }
        frames.free_frame(frame);
    }

    /// The physical address of the PML4 - what goes in CR3.
    pub fn root(&self) -> PhysAddr {
        self.root
    }

    /// Changes how tables are reached, after the mapping of physical
    /// memory moves (the switch from the identity map to the direct map).
    ///
    /// # Safety
    /// As for `new`, for the new offset.
    pub unsafe fn set_offset(&mut self, offset: u64) {
        self.offset = offset;
    }

    /// The table at physical address `frame`.
    #[allow(clippy::mut_from_ref)] // the tables live outside `self`
    fn table(&self, frame: PhysAddr) -> &mut Table {
        // SAFETY: the contract of `new`/`set_offset` makes this a valid
        // address of a 4 KiB, 4 KiB-aligned page table. The returned
        // reference is only ever used within one method call, with no
        // other reference to the same table alive.
        unsafe { &mut *((frame.as_u64().wrapping_add(self.offset)) as *mut Table) }
    }

    /// The table that entry `table[i]` points to, creating it if absent.
    fn next_table(
        &self,
        table: &mut Table,
        i: usize,
        user: bool,
        frames: &mut impl FrameSource,
    ) -> Result<PhysAddr, MapError> {
        let entry = table[i];
        if self.format.is_present(entry) {
            if !self.format.is_table(entry) {
                return Err(MapError::HugePageInTheWay);
            }
            if user {
                table[i] = self.format.table_entry_for_user(entry);
            }
            return Ok(PhysAddr::new(self.format.address(entry)));
        }

        let frame = frames.allocate_frame().ok_or(MapError::OutOfFrames)?;
        self.table(frame).fill(0);
        table[i] = self.format.table_entry(frame, user);
        Ok(frame)
    }

    /// Maps the page at `virt` to the frame at `phys`.
    ///
    /// Does not flush the TLB. Mapping where nothing was mapped needs no
    /// flush on x86 (the CPU never caches "not present"), so this is only
    /// a concern for `unmap`.
    pub fn map(
        &mut self,
        virt: VirtAddr,
        phys: PhysAddr,
        size: PageSize,
        flags: Flags,
        frames: &mut impl FrameSource,
    ) -> Result<(), MapError> {
        if !virt.is_aligned(size.bytes()) || !phys.is_aligned(size.bytes()) {
            return Err(MapError::Misaligned);
        }
        let user = flags.contains(Flags::USER);

        let pml4 = self.table(self.root);
        let pdpt = self.next_table(pml4, index(virt, 4), user, frames)?;
        let pdpt = self.table(pdpt);
        let pd = self.next_table(pdpt, index(virt, 3), user, frames)?;
        let pd = self.table(pd);

        let (leaf, i) = match size {
            PageSize::Size2M => (pd, index(virt, 2)),
            PageSize::Size4K => {
                let pt = self.next_table(pd, index(virt, 2), user, frames)?;
                (self.table(pt), index(virt, 1))
            }
        };
        if self.format.is_present(leaf[i]) {
            return Err(MapError::AlreadyMapped);
        }
        leaf[i] = self.format.leaf_entry(phys, size, flags);
        Ok(())
    }

    /// Removes the 4 KiB mapping at `virt` and returns the frame it
    /// pointed to - or None if there was none. The tables themselves are
    /// kept (they are cheap and likely to be used again).
    ///
    /// The caller must flush the TLB entry for `virt`: until then the
    /// CPU may still use the old translation.
    pub fn unmap(&mut self, virt: VirtAddr) -> Option<PhysAddr> {
        let mut table = self.table(self.root);
        for level in (2..=4).rev() {
            let entry = table[index(virt, level)];
            if !self.format.is_present(entry) || !self.format.is_table(entry) {
                return None;
            }
            table = self.table(PhysAddr::new(self.format.address(entry)));
        }
        let i = index(virt, 1);
        let entry = table[i];
        if !self.format.is_present(entry) {
            return None;
        }
        table[i] = 0;
        Some(PhysAddr::new(self.format.address(entry)))
    }

    /// Walks the tables the way the CPU would.
    pub fn translate(&self, virt: VirtAddr) -> Option<Translation> {
        let mut table = self.table(self.root);
        for level in (1..=4).rev() {
            let entry = table[index(virt, level)];
            if !self.format.is_present(entry) {
                return None;
            }
            let leaf = level == 1 || (level <= 3 && !self.format.is_table(entry));
            if leaf {
                // In a big page's entry the low address bits are not
                // address (x86 keeps a flag there), so they are cut off.
                let page_size: u64 = match level {
                    3 => 1 << 30,
                    2 => 1 << 21,
                    _ => PAGE_SIZE,
                };
                let within = virt.as_u64() & (page_size - 1);
                return Some(Translation {
                    physical: PhysAddr::new((self.format.address(entry) & !(page_size - 1)) + within),
                    flags: self.format.flags(entry, level > 1),
                    page_size,
                });
            }
            if level == 4 && !self.format.is_table(entry) && self.format == Format::AArch64 {
                return None; // the root cannot hold a block: not a tree the CPU would walk
            }
            table = self.table(PhysAddr::new(self.format.address(entry)));
        }
        unreachable!("level 1 is always a leaf")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A few pages of "physical memory" for tests. Physical address p is
    /// at `base + p`, so the PageTables offset is simply `base`. Frame 0
    /// is never handed out, so a zero entry can never look valid.
    struct FakeMemory {
        _pages: Vec<Page>,
        base: u64,
        next: u64,
        limit: u64,
        freed: Vec<PhysAddr>,
    }

    #[repr(C, align(4096))]
    struct Page([u8; 4096]);

    impl FakeMemory {
        fn new(pages: usize) -> FakeMemory {
            let storage: Vec<Page> = (0..pages).map(|_| Page([0xAA; 4096])).collect();
            let base = storage.as_ptr() as u64;
            FakeMemory { _pages: storage, base, next: PAGE_SIZE, limit: pages as u64 * PAGE_SIZE, freed: Vec::new() }
        }
        /// Frames handed out and not returned.
        fn used(&self) -> u64 {
            self.next / PAGE_SIZE - 1 - self.freed.len() as u64
        }
    }

    impl FrameSource for FakeMemory {
        fn allocate_frame(&mut self) -> Option<PhysAddr> {
            if let Some(frame) = self.freed.pop() {
                return Some(frame);
            }
            if self.next >= self.limit {
                return None;
            }
            let frame = PhysAddr::new(self.next);
            self.next += PAGE_SIZE;
            Some(frame)
        }
        fn free_frame(&mut self, frame: PhysAddr) {
            assert!(!self.freed.contains(&frame), "double free of {frame:?}");
            self.freed.push(frame);
        }
    }

    fn setup(pages: usize) -> (FakeMemory, PageTables) {
        let mut memory = FakeMemory::new(pages);
        let base = memory.base;
        // SAFETY: FakeMemory hands out frames inside its own storage,
        // which `base` maps.
        let tables = unsafe { PageTables::new(&mut memory, base) }.unwrap();
        (memory, tables)
    }

    /// The same, writing AArch64 entries.
    fn setup_a64(pages: usize) -> (FakeMemory, PageTables) {
        let mut memory = FakeMemory::new(pages);
        let base = memory.base;
        // SAFETY: as in setup.
        let tables = unsafe { PageTables::new_in(&mut memory, base, Format::AArch64) }.unwrap();
        (memory, tables)
    }

    /// The entry the CPU would end its walk at for `virt`, as written.
    fn leaf_bits(tables: &PageTables, virt: VirtAddr) -> u64 {
        let mut table = tables.table(tables.root);
        for level in (1..=4).rev() {
            let entry = table[index(virt, level)];
            if level == 1 || !tables.format.is_table(entry) {
                return entry;
            }
            table = tables.table(PhysAddr::new(tables.format.address(entry)));
        }
        unreachable!()
    }

    const RW: Flags = Flags(Flags::WRITABLE.0 | Flags::NO_EXECUTE.0);

    #[test]
    fn a_fresh_address_space_maps_nothing() {
        let (_memory, tables) = setup(4);
        assert_eq!(tables.translate(VirtAddr::new(0)), None);
        assert_eq!(tables.translate(VirtAddr::new(0xFFFF_FFFF_8000_0000)), None);
    }

    #[test]
    fn maps_and_translates_a_4k_page() {
        let (mut memory, mut tables) = setup(16);
        let virt = VirtAddr::new(0xFFFF_8000_1234_5000);
        tables.map(virt, PhysAddr::new(0x7000), PageSize::Size4K, RW, &mut memory).unwrap();
        // PDPT, PD and PT: three new tables below the root.
        assert_eq!(memory.used(), 4);

        let t = tables.translate(virt.add(0x123)).unwrap();
        assert_eq!(t.physical, PhysAddr::new(0x7123));
        assert_eq!(t.page_size, PAGE_SIZE);
        assert!(t.flags.contains(Flags::PRESENT | Flags::WRITABLE | Flags::NO_EXECUTE));
        assert!(!t.flags.contains(Flags::USER));
        // The neighbours stay unmapped.
        assert_eq!(tables.translate(virt.add(PAGE_SIZE)), None);
    }

    #[test]
    fn neighbouring_pages_share_tables() {
        let (mut memory, mut tables) = setup(16);
        for n in 0..512 {
            let virt = VirtAddr::new(0x4000_0000 + n * PAGE_SIZE);
            tables.map(virt, PhysAddr::new(n * PAGE_SIZE), PageSize::Size4K, RW, &mut memory).unwrap();
        }
        assert_eq!(memory.used(), 4, "one 2 MiB stretch needs one PT");
    }

    #[test]
    fn maps_2m_pages_without_a_page_table() {
        let (mut memory, mut tables) = setup(16);
        let virt = VirtAddr::new(0xFFFF_8000_0020_0000);
        tables.map(virt, PhysAddr::new(0x4000_0000), PageSize::Size2M, RW, &mut memory).unwrap();
        assert_eq!(memory.used(), 3);

        let t = tables.translate(virt.add(0x1_2345)).unwrap();
        assert_eq!(t.physical, PhysAddr::new(0x4001_2345));
        assert_eq!(t.page_size, 2 * 1024 * 1024);
        assert!(t.flags.contains(Flags::HUGE));
    }

    #[test]
    fn refuses_to_map_twice_or_misaligned_or_inside_a_huge_page() {
        let (mut memory, mut tables) = setup(16);
        let virt = VirtAddr::new(0x20_0000);
        tables.map(virt, PhysAddr::new(0x20_0000), PageSize::Size2M, RW, &mut memory).unwrap();

        let again = tables.map(virt, PhysAddr::new(0), PageSize::Size2M, RW, &mut memory);
        assert_eq!(again, Err(MapError::AlreadyMapped));
        let inside = tables.map(virt.add(PAGE_SIZE), PhysAddr::new(0), PageSize::Size4K, RW, &mut memory);
        assert_eq!(inside, Err(MapError::HugePageInTheWay));
        let crooked = tables.map(VirtAddr::new(0x1000), PhysAddr::new(0x800), PageSize::Size4K, RW, &mut memory);
        assert_eq!(crooked, Err(MapError::Misaligned));
        let crooked = tables.map(VirtAddr::new(0x1000), PhysAddr::new(0), PageSize::Size2M, RW, &mut memory);
        assert_eq!(crooked, Err(MapError::Misaligned));

        let virt = VirtAddr::new(0x1000);
        tables.map(virt, PhysAddr::new(0x5000), PageSize::Size4K, RW, &mut memory).unwrap();
        let again = tables.map(virt, PhysAddr::new(0x6000), PageSize::Size4K, RW, &mut memory);
        assert_eq!(again, Err(MapError::AlreadyMapped));
    }

    #[test]
    fn reports_running_out_of_frames() {
        let (mut memory, mut tables) = setup(3); // root + 2: one short
        let result = tables.map(VirtAddr::new(0x1000), PhysAddr::new(0), PageSize::Size4K, RW, &mut memory);
        assert_eq!(result, Err(MapError::OutOfFrames));
    }

    #[test]
    fn unmap_returns_the_frame_and_leaves_it_unmapped() {
        let (mut memory, mut tables) = setup(16);
        let virt = VirtAddr::new(0xFFFF_D000_0000_0000);
        tables.map(virt, PhysAddr::new(0x9000), PageSize::Size4K, RW, &mut memory).unwrap();
        assert_eq!(tables.unmap(virt), Some(PhysAddr::new(0x9000)));
        assert_eq!(tables.translate(virt), None);
        assert_eq!(tables.unmap(virt), None, "nothing left to unmap");

        // And the slot can be reused.
        tables.map(virt, PhysAddr::new(0xA000), PageSize::Size4K, RW, &mut memory).unwrap();
        assert_eq!(tables.translate(virt).unwrap().physical, PhysAddr::new(0xA000));
    }

    #[test]
    fn user_mappings_set_user_at_every_level() {
        let (mut memory, mut tables) = setup(16);
        let kernel = VirtAddr::new(0x40_0000);
        let user = VirtAddr::new(0x40_1000); // same tables
        tables.map(kernel, PhysAddr::new(0x1000), PageSize::Size4K, RW, &mut memory).unwrap();
        tables.map(user, PhysAddr::new(0x2000), PageSize::Size4K, RW | Flags::USER, &mut memory).unwrap();

        // Walk by hand: every entry on the way to `user` carries USER.
        let mut table = tables.table(tables.root);
        for level in (1..=4).rev() {
            let entry = table[index(user, level)];
            assert_ne!(entry & Flags::USER.bits(), 0, "level {level} lacks USER");
            table = tables.table(PhysAddr::new(entry & ADDRESS_MASK));
        }
        // The kernel page's own leaf entry stays kernel-only.
        assert!(!tables.translate(kernel).unwrap().flags.contains(Flags::USER));
    }

    #[test]
    fn the_index_split_matches_the_hardware() {
        let virt = VirtAddr::new(0xFFFF_FFFF_8010_0000);
        assert_eq!(index(virt, 4), 511);
        assert_eq!(index(virt, 3), 510);
        assert_eq!(index(virt, 2), 0);
        assert_eq!(index(virt, 1), 256);
    }

    #[test]
    fn processes_share_the_kernel_half_and_give_everything_back() {
        let (mut memory, mut kernel) = setup(400);
        kernel.fill_kernel_slots(&mut memory).unwrap();
        let kernel_page = VirtAddr::new(0xFFFF_FFFF_8010_0000);
        kernel.map(kernel_page, PhysAddr::new(0x10_0000), PageSize::Size4K, RW, &mut memory).unwrap();
        let before = memory.used();

        let base = memory.base;
        // SAFETY: as in setup.
        let mut user = unsafe { PageTables::new_user(&mut memory, base, &kernel) }.unwrap();
        // The kernel is visible in the new space - including a mapping
        // made after it was created, through the shared PDPTs.
        assert!(user.translate(kernel_page).is_some());
        let later = VirtAddr::new(0xFFFF_C000_0000_0000);
        kernel.map(later, PhysAddr::new(0x2000), PageSize::Size4K, RW, &mut memory).unwrap();
        assert_eq!(user.translate(later).map(|t| t.physical), Some(PhysAddr::new(0x2000)));
        // That mapping took a PD and a PT - the kernel's, not the process's.
        let kernel_tables_added = 2;

        // A process maps its own pages, and one it does not own.
        let owned = Flags::USER | Flags::WRITABLE | Flags::OWNED;
        for n in 0..20 {
            let frame = memory.allocate_frame().unwrap();
            let virt = VirtAddr::new(0x40_0000 + n * PAGE_SIZE * 300); // spread over several PTs
            user.map(virt, frame, PageSize::Size4K, owned, &mut memory).unwrap();
        }
        let device = PhysAddr::new(0xFEE0_0000);
        user.map(VirtAddr::new(0x7000_0000), device, PageSize::Size4K, Flags::USER | Flags::WRITABLE, &mut memory)
            .unwrap();
        assert!(memory.used() > before + 20);

        // Destroying it returns every frame it allocated - tables and
        // owned pages - but not the device's, and leaves the kernel alone.
        user.destroy(&mut memory);
        assert_eq!(memory.used(), before + kernel_tables_added, "only the kernel's new tables should remain");
        assert!(!memory.freed.contains(&device), "freed a frame the process did not own");
        assert!(kernel.translate(kernel_page).is_some());
    }

    #[test]
    fn filling_the_kernel_slots_is_idempotent() {
        let (mut memory, mut kernel) = setup(400);
        kernel.fill_kernel_slots(&mut memory).unwrap();
        let used = memory.used();
        kernel.fill_kernel_slots(&mut memory).unwrap();
        assert_eq!(memory.used(), used);
        assert_eq!(used, 1 + 256, "the root and one PDPT per upper slot");
    }

    // ---- AArch64 entries ---------------------------------------------

    #[test]
    fn x86_entries_are_bit_for_bit_what_they_always_were() {
        let (mut memory, mut tables) = setup(16);
        let virt = VirtAddr::new(0xFFFF_8000_1234_5000);
        let flags = Flags::WRITABLE | Flags::GLOBAL | Flags::NO_EXECUTE;
        tables.map(virt, PhysAddr::new(0x7000), PageSize::Size4K, flags, &mut memory).unwrap();
        assert_eq!(leaf_bits(&tables, virt), 0x8000_0000_0000_7103);
        let big = VirtAddr::new(0xFFFF_8000_4000_0000);
        tables.map(big, PhysAddr::new(0x4000_0000), PageSize::Size2M, flags, &mut memory).unwrap();
        assert_eq!(leaf_bits(&tables, big), 0x8000_0000_4000_0183);
        // A table entry: present, writable.
        let root = tables.table(tables.root);
        assert_eq!(root[index(virt, 4)] & 0xFFF, 0x003);
    }

    #[test]
    fn aarch64_kernel_entries_have_the_manuals_bits() {
        let (mut memory, mut tables) = setup_a64(32);
        let at = |n: u64| VirtAddr::new(0xFFFF_8000_0000_0000 + n * PAGE_SIZE);
        let phys = PhysAddr::new(0x4008_5000);
        let mut bits = |n: u64, flags: Flags| {
            tables.map(at(n), phys, PageSize::Size4K, flags, &mut memory).unwrap();
            leaf_bits(&tables, at(n)) & !0x4008_5000
        };
        // valid + page + accessed + shared = 0x703, then:
        // code: read-only (bit 7), programs may not execute it (bit 54).
        assert_eq!(bits(0, Flags::GLOBAL), 0x0040_0000_0000_0783);
        // read-only data: nobody executes it (bits 53 and 54).
        assert_eq!(bits(1, Flags::GLOBAL | Flags::NO_EXECUTE), 0x0060_0000_0000_0783);
        // data: writable is bit 7 *clear*.
        assert_eq!(bits(2, Flags::GLOBAL | Flags::WRITABLE | Flags::NO_EXECUTE), 0x0060_0000_0000_0703);
        // not global: bit 11.
        assert_eq!(bits(3, Flags::WRITABLE | Flags::NO_EXECUTE), 0x0060_0000_0000_0F03);
        // a device: kind 1, and not marked shared.
        let device = Flags::GLOBAL | Flags::WRITABLE | Flags::NO_EXECUTE | Flags::NO_CACHE | Flags::WRITE_THROUGH;
        assert_eq!(bits(4, device), 0x0060_0000_0000_0407);
        // a framebuffer: kind 2.
        let screen = Flags::GLOBAL | Flags::WRITABLE | Flags::NO_EXECUTE | Flags::WRITE_THROUGH;
        assert_eq!(bits(5, screen), 0x0060_0000_0000_070B);
    }

    #[test]
    fn aarch64_user_entries_have_the_manuals_bits() {
        let (mut memory, mut tables) = setup_a64(32);
        let at = |n: u64| VirtAddr::new(0x40_0000 + n * PAGE_SIZE);
        let mut bits = |n: u64, flags: Flags| {
            tables.map(at(n), PhysAddr::new(0), PageSize::Size4K, flags, &mut memory).unwrap();
            leaf_bits(&tables, at(n))
        };
        // A program's code: user (bit 6), read-only, not global, and
        // the *kernel* may not execute it (bit 53).
        assert_eq!(bits(0, Flags::USER), 0x0020_0000_0000_0FC3);
        // Its data, owned by the address space (bit 55).
        let data = Flags::USER | Flags::WRITABLE | Flags::NO_EXECUTE | Flags::OWNED;
        assert_eq!(bits(1, data), 0x00E0_0000_0000_0F43);
        // The tables above it are plain pointers: valid + table, and
        // nothing that restricts.
        let root = tables.table(tables.root);
        assert_eq!(root[index(at(0), 4)] & 0xFFF0_0000_0000_0FFF, 0x003);
    }

    #[test]
    fn aarch64_a_block_is_an_entry_with_bit_one_clear() {
        let (mut memory, mut tables) = setup_a64(16);
        let virt = VirtAddr::new(0xFFFF_8000_0020_0000);
        let flags = Flags::WRITABLE | Flags::GLOBAL | Flags::NO_EXECUTE;
        tables.map(virt, PhysAddr::new(0x4000_0000), PageSize::Size2M, flags, &mut memory).unwrap();
        assert_eq!(memory.used(), 3, "no last-level table");
        assert_eq!(leaf_bits(&tables, virt), 0x0060_0000_4000_0701);

        let t = tables.translate(virt.add(0x1_2345)).unwrap();
        assert_eq!(t.physical, PhysAddr::new(0x4001_2345));
        assert_eq!(t.page_size, 2 * 1024 * 1024);
        assert!(t.flags.contains(Flags::HUGE | Flags::WRITABLE | Flags::NO_EXECUTE | Flags::GLOBAL));

        let inside = tables.map(virt.add(PAGE_SIZE), PhysAddr::new(0), PageSize::Size4K, RW, &mut memory);
        assert_eq!(inside, Err(MapError::HugePageInTheWay));
        assert_eq!(tables.unmap(virt), None, "unmap is for 4 KiB pages");
    }

    #[test]
    fn aarch64_flags_read_back_as_they_were_asked_for() {
        let (mut memory, mut tables) = setup_a64(64);
        let sets = [
            Flags::NONE,
            Flags::WRITABLE,
            Flags::NO_EXECUTE,
            Flags::WRITABLE | Flags::NO_EXECUTE | Flags::GLOBAL,
            Flags::USER,
            Flags::USER | Flags::WRITABLE | Flags::NO_EXECUTE | Flags::OWNED,
            Flags::USER | Flags::NO_EXECUTE,
            Flags::WRITABLE | Flags::NO_EXECUTE | Flags::NO_CACHE | Flags::WRITE_THROUGH,
            Flags::WRITABLE | Flags::NO_EXECUTE | Flags::WRITE_THROUGH | Flags::GLOBAL,
        ];
        for (n, &asked) in sets.iter().enumerate() {
            let virt = VirtAddr::new(0x1000_0000 + n as u64 * PAGE_SIZE);
            tables.map(virt, PhysAddr::new(0x9000), PageSize::Size4K, asked, &mut memory).unwrap();
            let read = tables.translate(virt).unwrap();
            assert_eq!(read.flags, asked | Flags::PRESENT, "asked for {asked:?}");
            assert_eq!(read.physical, PhysAddr::new(0x9000));
        }
    }

    #[test]
    fn aarch64_maps_unmaps_and_runs_out_like_x86() {
        let (mut memory, mut tables) = setup_a64(16);
        assert_eq!(tables.translate(VirtAddr::new(0)), None);
        let virt = VirtAddr::new(0xFFFF_D000_0000_0000);
        tables.map(virt, PhysAddr::new(0x9000), PageSize::Size4K, RW, &mut memory).unwrap();
        assert_eq!(memory.used(), 4);
        assert_eq!(tables.translate(virt.add(0x123)).unwrap().physical, PhysAddr::new(0x9123));
        assert_eq!(tables.translate(virt.add(PAGE_SIZE)), None);
        assert_eq!(tables.map(virt, PhysAddr::new(0), PageSize::Size4K, RW, &mut memory), Err(MapError::AlreadyMapped));
        assert_eq!(tables.unmap(virt), Some(PhysAddr::new(0x9000)));
        assert_eq!(tables.translate(virt), None);
        assert_eq!(tables.unmap(virt), None);

        let (mut memory, mut tables) = setup_a64(3);
        let result = tables.map(VirtAddr::new(0x1000), PhysAddr::new(0), PageSize::Size4K, RW, &mut memory);
        assert_eq!(result, Err(MapError::OutOfFrames));
    }

    #[test]
    fn aarch64_a_process_has_a_tree_of_its_own_and_gives_it_all_back() {
        let (mut memory, mut kernel) = setup_a64(400);
        kernel.fill_kernel_slots(&mut memory).unwrap();
        assert_eq!(memory.used(), 1, "nothing to fill: the kernel's root is never copied");
        let kernel_page = VirtAddr::new(0xFFFF_FFFF_8010_0000);
        kernel.map(kernel_page, PhysAddr::new(0x10_0000), PageSize::Size4K, RW, &mut memory).unwrap();
        let before = memory.used();

        let base = memory.base;
        // SAFETY: as in setup.
        let mut user = unsafe { PageTables::new_user(&mut memory, base, &kernel) }.unwrap();
        assert_eq!(user.translate(kernel_page), None, "the kernel is in the other root register");

        let owned = Flags::USER | Flags::WRITABLE | Flags::OWNED;
        for n in 0..20 {
            let frame = memory.allocate_frame().unwrap();
            let virt = VirtAddr::new(0x40_0000 + n * PAGE_SIZE * 300);
            user.map(virt, frame, PageSize::Size4K, owned, &mut memory).unwrap();
        }
        let device = PhysAddr::new(0x0900_0000);
        user.map(VirtAddr::new(0x7000_0000), device, PageSize::Size4K, Flags::USER | Flags::WRITABLE, &mut memory)
            .unwrap();

        user.destroy(&mut memory);
        assert_eq!(memory.used(), before);
        assert!(!memory.freed.contains(&device), "freed a frame the process did not own");
        assert!(kernel.translate(kernel_page).is_some());
    }

    #[test]
    fn an_adopted_tree_is_the_same_tree() {
        let (mut memory, mut tables) = setup_a64(16);
        let virt = VirtAddr::new(0xFFFF_FFFF_C000_0000);
        tables.map(virt, PhysAddr::new(0x9000), PageSize::Size4K, RW, &mut memory).unwrap();
        // SAFETY: the tree `tables` made, reached the same way.
        let mut adopted = unsafe { PageTables::adopt(tables.root(), memory.base, Format::AArch64) };
        assert_eq!(adopted.translate(virt).unwrap().physical, PhysAddr::new(0x9000));
        let used = memory.used();
        adopted.map(virt.add(PAGE_SIZE), PhysAddr::new(0xA000), PageSize::Size4K, RW, &mut memory).unwrap();
        assert_eq!(memory.used(), used, "the tables were already there");
        assert_eq!(tables.translate(virt.add(PAGE_SIZE)).unwrap().physical, PhysAddr::new(0xA000));
    }

    #[test]
    fn flags_print_readably() {
        assert_eq!(format!("{:?}", Flags::PRESENT | Flags::NO_EXECUTE), "P|NX");
        assert_eq!(format!("{:?}", Flags::NONE), "-");
    }
}
