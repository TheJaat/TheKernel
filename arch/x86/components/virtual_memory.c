/* Includes
 * - System
 */
#include <arch/x86/memory.h>
#include <arch/x86/x32/arch_x32.h>
#include <defs.h>
#include <arch/x86/address_space.h>
#include <system/log.h>

/* For the framebuffer base pointer */
#include <video/interface/video_interface.h>
#include <video/vbe.h>
#include <terminal/terminal.h>

/* Includes
 * - Library
 */
#include <stddef.h>
#include <string.h>

/* Globals
 * Needed for the virtual memory manager to keep
 * track of current directories */
static PageDirectory_t *g_KernelPageDirectory = NULL;
static PageDirectory_t *g_PageDirectories[MAX_SUPPORTED_CPUS];
static uintptr_t g_ReservedPtr = 0;

/* The upper bound of the region we identity-map 1:1 at boot.
 * MmPhysicalAllocateBlock() must never hand out a frame above this,
 * otherwise page-tables become unreachable once CR0.PG is set. */
#define MEMORY_IDENTITY_LIMIT   0x1000000   /* 16 mB */

/* Extern acess to system mappings in the physical memory manager */
extern SystemMemoryMapping_t SysMappings[32];

/* Extern assembly functions implemented in paging.asm */
extern void memory_set_paging(int enable);
extern void memory_load_cr3(uintptr_t pda);
extern void memory_reload_cr3(void);
extern void memory_invalidate_addr(uintptr_t Address);
extern uint32_t memory_get_cr3(void);

/* CpuGetCurrentId
 * Retrieves the current cpu id for caller. TODO: move out of here. */
UUId_t
CpuGetCurrentId(void)
{
	return 0;
}

/* MmVirtualCreatePageTable
 * Creates and initializes a new empty page-table.
 * Returns NULL if no suitable frame could be allocated. */
PageTable_t*
MmVirtualCreatePageTable(void)
{
	PhysicalAddress_t Address = MmPhysicalAllocateBlock(MEMORY_INIT_MASK, 1);

	/* MmPhysicalAllocateBlock returns 0 on failure. Frame 0 is never
	 * handed out (it is reserved in MmPhysicalInit), so 0 is unambiguous. */
	if (Address == 0) {
		LogFatal("Virtual_Memory", "out of low physical memory for a page-table");
		return NULL;
	}

	memset((void*)Address, 0, sizeof(PageTable_t));
	return (PageTable_t*)Address;
}

/* MmVirtualFillPageTable
 * Maps <Length> bytes of pAddressStart into pTable, starting at the
 * entry that corresponds to vAddressStart. Stops at the end of the
 * table or when Length is exhausted, whichever comes first. */
void
MmVirtualFillPageTable(
	 PageTable_t *pTable,
	 PhysicalAddress_t pAddressStart,
	 VirtualAddress_t vAddressStart,
	 uintptr_t Length,
	 Flags_t Flags)
{
	uintptr_t pAddress = (uintptr_t)pAddressStart & PAGE_MASK;
	uintptr_t vAddress = (uintptr_t)vAddressStart & PAGE_MASK;
	uintptr_t Mapped   = 0;
	unsigned  Index    = PAGE_TABLE_INDEX(vAddress);

	for (; Index < PAGES_PER_TABLE && Mapped < Length;
		 Index++, pAddress += PAGE_SIZE, Mapped += PAGE_SIZE) {
		pTable->Pages[Index] = (uint32_t)(pAddress & PAGE_MASK)
			| PAGE_PRESENT | PAGE_WRITE | PAGE_SYSTEM_MAP | Flags;
	}
}

/* MmVirtualIdentityMapMemoryRange
 * Maps [pAddressStart, pAddressStart+Length) to
 * [vAddressStart, vAddressStart+Length), creating page-tables as needed.
 * Handles ranges that are not 4mB aligned and reuses an already
 * installed table instead of leaking it. */
void
MmVirtualIdentityMapMemoryRange(
	 PageDirectory_t* PageDirectory,
	 PhysicalAddress_t pAddressStart,
	 VirtualAddress_t vAddressStart,
	 uintptr_t Length,
	 int Fill,
	 Flags_t Flags)
{
	uintptr_t pAddress = (uintptr_t)pAddressStart & PAGE_MASK;
	uintptr_t vAddress = (uintptr_t)vAddressStart & PAGE_MASK;
	uintptr_t End;

	if (Length == 0) {
		return;
	}

	/* Round the end up so a partial trailing page is still covered */
	End = ((uintptr_t)vAddressStart + Length + PAGE_SIZE - 1) & PAGE_MASK;

	while (vAddress < End) {
		unsigned     Index    = PAGE_DIRECTORY_INDEX(vAddress);
		uintptr_t    TableEnd = (vAddress & ~((uintptr_t)TABLE_SPACE_SIZE - 1))
								+ TABLE_SPACE_SIZE;
		uintptr_t    Chunk    = ((End < TableEnd) ? End : TableEnd) - vAddress;
		PageTable_t *Table    = NULL;

		if (PageDirectory->pTables[Index] & PAGE_PRESENT) {
			/* Reuse the table that is already installed here */
			Table = (PageTable_t*)PageDirectory->vTables[Index];
		}
		else {
			Table = MmVirtualCreatePageTable();
			if (Table == NULL) {
				return;
			}
			PageDirectory->pTables[Index] = ((uint32_t)(uintptr_t)Table & PAGE_MASK)
				| PAGE_SYSTEM_MAP | PAGE_PRESENT | PAGE_WRITE | Flags;
			PageDirectory->vTables[Index] = (uintptr_t)Table;
		}

		if (Fill != 0) {
			MmVirtualFillPageTable(Table, pAddress, vAddress, Chunk, Flags);
		}

		vAddress += Chunk;
		pAddress += Chunk;
	}
}

/* MmVirtualSwitchPageDirectory
 * Switches page-directory for the given cpu */
OsStatus_t
MmVirtualSwitchPageDirectory(
	 UUId_t Cpu,
	 PageDirectory_t* PageDirectory,
	 PhysicalAddress_t Pdb)
{
	if (PageDirectory == NULL) {
		return Error;
	}

	g_PageDirectories[Cpu] = PageDirectory;
	memory_load_cr3(Pdb);
	return Success;
}

/* MmVirtualGetCurrentDirectory
 * Retrieves the current page-directory for the given cpu */
PageDirectory_t*
MmVirtualGetCurrentDirectory(
	 UUId_t Cpu)
{
	if (Cpu >= MAX_SUPPORTED_CPUS) {
		return NULL;
	}
	return g_PageDirectories[Cpu];
}

/* MmVirtualInitialMap
 * Maps a single page into the kernel directory. Only safe to call
 * while the frame the page-table lives in is still reachable 1:1. */
void
MmVirtualInitialMap(
	 PhysicalAddress_t pAddress,
	 VirtualAddress_t vAddress)
{
	PageDirectory_t *Directory = g_KernelPageDirectory;
	PageTable_t *Table = NULL;

	if (!(Directory->pTables[PAGE_DIRECTORY_INDEX(vAddress)] & PAGE_PRESENT)) {
		Table = MmVirtualCreatePageTable();
		if (Table == NULL) {
			return;
		}
		Directory->pTables[PAGE_DIRECTORY_INDEX(vAddress)] =
			((uint32_t)(uintptr_t)Table & PAGE_MASK) | PAGE_PRESENT | PAGE_WRITE;
		Directory->vTables[PAGE_DIRECTORY_INDEX(vAddress)] = (uintptr_t)Table;
	}
	else {
		Table = (PageTable_t*)Directory->vTables[PAGE_DIRECTORY_INDEX(vAddress)];
	}

	Table->Pages[PAGE_TABLE_INDEX(vAddress)] =
		((uint32_t)pAddress & PAGE_MASK) | PAGE_PRESENT | PAGE_WRITE;
}

/* MmVirtualMap
 * Installs a new page-mapping in the given page-directory.
 * Works after paging is live because every page-table frame comes from
 * the identity-mapped window below MEMORY_IDENTITY_LIMIT. */
OsStatus_t
MmVirtualMap(
	 void *PageDirectory,
	 PhysicalAddress_t pAddress,
	 VirtualAddress_t vAddress,
	 Flags_t Flags)
{
	PageDirectory_t *Directory = (PageDirectory_t*)PageDirectory;
	PageTable_t *Table = NULL;
	int IsCurrent = 0;

	if (Directory == NULL) {
		Directory = g_PageDirectories[CpuGetCurrentId()];
	}
	if (Directory == NULL) {
		return Error;
	}
	if (g_PageDirectories[CpuGetCurrentId()] == Directory) {
		IsCurrent = 1;
	}

	if (!(Directory->pTables[PAGE_DIRECTORY_INDEX(vAddress)] & PAGE_PRESENT)) {
		Table = MmVirtualCreatePageTable();
		if (Table == NULL) {
			return Error;
		}

		Directory->pTables[PAGE_DIRECTORY_INDEX(vAddress)] =
			((uint32_t)(uintptr_t)Table & PAGE_MASK) | PAGE_PRESENT | PAGE_WRITE | Flags;
		Directory->vTables[PAGE_DIRECTORY_INDEX(vAddress)] = (uintptr_t)Table;

		if (IsCurrent) {
			memory_reload_cr3();
		}
	}
	else {
		Table = (PageTable_t*)Directory->vTables[PAGE_DIRECTORY_INDEX(vAddress)];
	}

	if (Table->Pages[PAGE_TABLE_INDEX(vAddress)] & PAGE_PRESENT) {
		LogFatal("Virtual_Memory", "remap of 0x%x (old 0x%x)",
			vAddress, Table->Pages[PAGE_TABLE_INDEX(vAddress)]);
		return Error;
	}

	Table->Pages[PAGE_TABLE_INDEX(vAddress)] =
		((uint32_t)pAddress & PAGE_MASK) | PAGE_PRESENT | PAGE_WRITE | Flags;

	if (IsCurrent) {
		memory_invalidate_addr(vAddress);
	}

	return Success;
}

/* MmVirtualGetMapping
 * Retrieves the physical address behind a virtual address, or 0. */
PhysicalAddress_t
MmVirtualGetMapping(
	 void *PageDirectory,
	 VirtualAddress_t Address)
{
	PageDirectory_t *Directory = (PageDirectory_t*)PageDirectory;
	PageTable_t *Table = NULL;

	if (Directory == NULL) {
		Directory = g_PageDirectories[CpuGetCurrentId()];
	}
	if (Directory == NULL) {
		return 0;
	}
	if (!(Directory->pTables[PAGE_DIRECTORY_INDEX(Address)] & PAGE_PRESENT)) {
		return 0;
	}

	Table = (PageTable_t*)Directory->vTables[PAGE_DIRECTORY_INDEX(Address)];
	if (Table == NULL
		|| !(Table->Pages[PAGE_TABLE_INDEX(Address)] & PAGE_PRESENT)) {
		return 0;
	}

	return (Table->Pages[PAGE_TABLE_INDEX(Address)] & PAGE_MASK)
		+ (Address & ATTRIBUTE_MASK);
}

/* MmVirtualInit
 * Initializes the virtual memory system and installs default
 * kernel mappings */
OsStatus_t
MmVirtualInit(void)
{
	AddressSpace_t   KernelSpace;
	PageTable_t     *iTable       = NULL;
	Terminal        *Term         = VideoGetTerminal();
	VbeContext      *Ctx          = NULL;
	uintptr_t        FbPhysical   = 0;
	uintptr_t        FbOffset     = 0;
	size_t           FbSize       = 0;
	int i;

	LogInformation("Virtual_Memory", "Initializing");

	g_ReservedPtr = MEMORY_LOCATION_RESERVED;

	/* Page-directory: 2 pages of structure, 3 allocated (guard) */
	g_KernelPageDirectory = (PageDirectory_t*)
		MmPhysicalAllocateBlock(MEMORY_INIT_MASK, 3);
	if (g_KernelPageDirectory == NULL) {
		LogFatal("Virtual_Memory", "could not allocate the page-directory");
		return Error;
	}
	memset((void*)g_KernelPageDirectory, 0, sizeof(PageDirectory_t));

	/* Identity map 0x1000 - 0x400000. Page 0 stays unmapped so that a
	 * NULL dereference faults instead of silently working. */
	iTable = MmVirtualCreatePageTable();
	if (iTable == NULL) {
		return Error;
	}
	MmVirtualFillPageTable(iTable, 0x1000, 0x1000,
		TABLE_SPACE_SIZE - 0x1000, 0);

	g_KernelPageDirectory->pTables[0] = ((uint32_t)(uintptr_t)iTable & PAGE_MASK)
		| PAGE_PRESENT | PAGE_WRITE | PAGE_SYSTEM_MAP;
	g_KernelPageDirectory->vTables[0] = (uintptr_t)iTable;

	/* Identity map 4mB - 16mB as well. The physical allocator's "low"
	 * path can return frames anywhere below 16mB, and every page-table
	 * it hands out must stay addressable once paging is on. */
	LogInformation("Virtual_Memory", "Identity mapping 0x%x - 0x%x",
		TABLE_SPACE_SIZE, MEMORY_IDENTITY_LIMIT);
	MmVirtualIdentityMapMemoryRange(g_KernelPageDirectory,
		TABLE_SPACE_SIZE, TABLE_SPACE_SIZE,
		MEMORY_IDENTITY_LIMIT - TABLE_SPACE_SIZE, 1, 0);

	/* Pre-map (reserve tables for) the heap region */
	LogInformation("Virtual_Memory", "Mapping heap region to 0x%x",
		MEMORY_LOCATION_HEAP);
	MmVirtualIdentityMapMemoryRange(g_KernelPageDirectory, 0,
		MEMORY_LOCATION_HEAP,
		(MEMORY_LOCATION_HEAP_END - MEMORY_LOCATION_HEAP), 0, 0);

	/* ---- Framebuffer -------------------------------------------------
	 * Only valid when the bootloader actually got us a linear VBE mode.
	 * In VGA text mode driver->context is a VgaContext, which is 20 bytes
	 * long - reading VbeMode_t::PhysBasePtr (offset 40) out of it is an
	 * out-of-bounds read and yields a garbage physical address. */
	if (Term != NULL
		&& Term->driver != NULL
		&& Term->driver->context != NULL
		&& Term->videoModeType == VIDEO_GRAPHICS) {

		Ctx        = (VbeContext*)Term->driver->context;
		FbPhysical = (uintptr_t)Ctx->mode.PhysBasePtr;
		FbOffset   = FbPhysical & ATTRIBUTE_MASK;
		FbSize     = (size_t)Ctx->mode.BytesPerScanLine
					 * (size_t)Ctx->mode.YResolution;

		LogInformation("Virtual_Memory", "VBE %ux%u bpp=%u pitch=%u lfb=0x%x",
			(unsigned)Ctx->mode.XResolution,
			(unsigned)Ctx->mode.YResolution,
			(unsigned)Ctx->mode.BitsPerPixel,
			(unsigned)Ctx->mode.BytesPerScanLine,
			(unsigned)FbPhysical);

		if (FbPhysical == 0 || FbSize == 0) {
			LogFatal("Virtual_Memory", "VBE reported no linear framebuffer");
			Ctx = NULL;
		}
		else if ((FbSize + FbOffset)
				 > (MEMORY_LOCATION_RESERVED - MEMORY_LOCATION_VIDEO)) {
			LogFatal("Virtual_Memory",
				"framebuffer (%u bytes) exceeds the video window", FbSize);
			Ctx = NULL;
		}
		else {
			LogInformation("Virtual_Memory", "Mapping video memory to 0x%x",
				MEMORY_LOCATION_VIDEO);
			MmVirtualIdentityMapMemoryRange(g_KernelPageDirectory,
				FbPhysical & PAGE_MASK,
				MEMORY_LOCATION_VIDEO,
				FbSize + FbOffset,
				1, PAGE_USER);
		}
	}
	else {
		LogInformation("Virtual_Memory",
			"no linear framebuffer, skipping video mapping");
	}

	/* Install the reserved system memory mappings */
	LogInformation("Virtual_Memory", "Mapping reserved memory to 0x%x",
		MEMORY_LOCATION_RESERVED);

	for (i = 0; i < 32; i++) {
		if (SysMappings[i].Length != 0 && SysMappings[i].Type != 1) {
			int PageCount = (int)DIVUP(SysMappings[i].Length, PAGE_SIZE);
			int j;

			SysMappings[i].vAddressStart = g_ReservedPtr;

			for (j = 0; j < PageCount; j++, g_ReservedPtr += PAGE_SIZE) {
				MmVirtualInitialMap(
					((SysMappings[i].pAddressStart & PAGE_MASK) + (j * PAGE_SIZE)),
					g_ReservedPtr);
			}
		}
	}

	/* ---- Go live -----------------------------------------------------
	 * Order matters. Until CR0.PG is set, MEMORY_LOCATION_VIDEO is just
	 * plain RAM at 64mB; after it is set, the old physical LFB address is
	 * no longer mapped. So the switch has to happen here, with nothing
	 * that touches the screen in between. */
	MmVirtualSwitchPageDirectory(0, g_KernelPageDirectory,
		(uintptr_t)g_KernelPageDirectory);
	memory_set_paging(1);

	if (Ctx != NULL) {
		Ctx->mode.PhysBasePtr = (uint32_t)(MEMORY_LOCATION_VIDEO + FbOffset);
	}

	/* Safe to print again from here on */
	LogInformation("Virtual_Memory", "paging enabled, framebuffer at 0x%x",
		Ctx != NULL ? Ctx->mode.PhysBasePtr : 0);

	KernelSpace.References   = 1;
	KernelSpace.Flags        = AS_TYPE_KERNEL;
	KernelSpace.Cr3          = (uintptr_t)g_KernelPageDirectory;
	KernelSpace.PageDirectory = g_KernelPageDirectory;

	LogInformation("Virtual_Memory", "done");
	return AddressSpaceInitKernel(&KernelSpace);
}