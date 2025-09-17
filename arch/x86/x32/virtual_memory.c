/* Includes
 * - System
 */
#include <arch/x86/memory.h>
#include <arch/x86/x32/arch_x32.h>
#include <defs.h>
#include <system/address_space.h>
#include <video/video.h>

// For physical base pointer
#include <video/vbe.h>

/* Includes
 * - Library
 */
#include <stddef.h>
#include <string.h>

/* Globals 
 * Needed for the virtual memory manager to keep
 * track of current directories
 */
static PageDirectory_t *g_KernelPageDirectory = NULL;
static PageDirectory_t *g_PageDirectories[MAX_SUPPORTED_CPUS];
//static Spinlock_t GlbVmLock = SPINLOCK_INIT;
static uintptr_t g_ReservedPtr = 0;

/* Extern acess to system mappings in the
 * physical memory manager */
extern SystemMemoryMapping_t SysMappings[32];

/* Extern assembly functions that are
 * implemented in paging.asm */
extern void memory_set_paging(int enable);
extern void memory_load_cr3(uintptr_t pda);
extern void memory_reload_cr3(void);
extern void memory_invalidate_addr(uintptr_t pda);
extern uint32_t memory_get_cr3(void);


// TODO: Move it out from here.
/* CpuGetCurrentId 
 * Retrieves the current cpu id for caller */
UUId_t
CpuGetCurrentId(void)
{
	// Get apic id
	return 0;
}

/* MmVirtualCreatePageTable
 * Creates and initializes a new empty page-table */
PageTable_t*
MmVirtualCreatePageTable(void)
{
	// Variables
	PageTable_t *Table = NULL;
	PhysicalAddress_t Address = 0;

	// Allocate a new page-table instance
	Address = MmPhysicalAllocateBlock(MEMORY_INIT_MASK, 1);
	Table = (PageTable_t*)Address;

	// Make sure all is good
//	assert(Table != NULL);

	// Initialize it and return
	memset((void*)Table, 0, sizeof(PageTable_t));
	return Table;
}

/* MmVirtualFillPageTable
 * Identity maps a memory region inside the given
 * page-table - type of mappings is controlled with Flags */
void 
MmVirtualFillPageTable(
	 PageTable_t *pTable, 
	 PhysicalAddress_t pAddressStart, 
	 VirtualAddress_t vAddressStart, 
	 Flags_t Flags)
{
	// Variables
	uintptr_t pAddress, vAddress;
	int i;

	// Iterate through pages and map them
	for (i = PAGE_TABLE_INDEX(vAddressStart), pAddress = pAddressStart, vAddress = vAddressStart;
		i < 1024;
		i++, pAddress += PAGE_SIZE, vAddress += PAGE_SIZE) {
		uint32_t pEntry = pAddress | PAGE_PRESENT | PAGE_WRITE | PAGE_SYSTEM_MAP | Flags;
		pTable->Pages[PAGE_TABLE_INDEX(vAddress)] = pEntry;
	}
}

/* MmVirtualIdentityMapMemoryRange
 * Identity maps not only a page-table or a region inside
 * it can identity map an entire memory region and create
 * page-table for the region automatically */
void 
MmVirtualIdentityMapMemoryRange(
	 PageDirectory_t* PageDirectory,
	 PhysicalAddress_t pAddressStart, 
	 VirtualAddress_t vAddressStart, 
	 uintptr_t Length, 
	 int Fill, 
	 Flags_t Flags)
{
	// Variables
	unsigned i, k;

	// Iterate the afflicted page-tables
	for (i = PAGE_DIRECTORY_INDEX(vAddressStart), k = 0;
		i < (PAGE_DIRECTORY_INDEX(vAddressStart + Length - 1) + 1);
		i++, k++) {
		PageTable_t *Table = MmVirtualCreatePageTable();
		uintptr_t pAddress = pAddressStart + (k * TABLE_SPACE_SIZE);
		uintptr_t vAddress = vAddressStart + (k * TABLE_SPACE_SIZE);

		// Fill it with pages?
		if (Fill != 0) {
			MmVirtualFillPageTable(Table, pAddress, vAddress, Flags);
		}

		// Install the table into the given page-directory
		PageDirectory->pTables[i] = (PhysicalAddress_t)Table 
			| (PAGE_SYSTEM_MAP | PAGE_PRESENT | PAGE_WRITE | Flags);
		PageDirectory->vTables[i] = (uintptr_t)Table;
	}
}

/* MmVirtualSwitchPageDirectory
 * Switches page-directory for the current cpu
 * but the current cpu should be given as parameter
 * as well */
OsStatus_t
MmVirtualSwitchPageDirectory(
	 UUId_t Cpu, 
	 PageDirectory_t* PageDirectory, 
	 PhysicalAddress_t Pdb)
{
	// Sanitize the parameter
//	assert(PageDirectory != NULL);

	// Update current and load the page-directory
	g_PageDirectories[Cpu] = PageDirectory;
	memory_load_cr3(Pdb);

	// Done - no errors
	return Success;
}

/* MmVirtualGetCurrentDirectory
 * Retrieves the current page-directory for the given cpu */
PageDirectory_t*
MmVirtualGetCurrentDirectory(
	 UUId_t Cpu)
{
	// Sanitize
//	assert(Cpu < MAX_SUPPORTED_CPUS);

	// Return the current - even if null
	return g_PageDirectories[Cpu];
}

/* MmVirtualInstallPaging
 * Initializes paging for the given cpu id */
OsStatus_t
MmVirtualInstallPaging(
	 UUId_t Cpu)
{
	MmVirtualSwitchPageDirectory(Cpu, g_KernelPageDirectory, 
		(uintptr_t)g_KernelPageDirectory);
	memory_set_paging(1);
	return Success;
}

/* MmVirtualMap
 * Installs a new page-mapping in the given
 * page-directory. The type of mapping is controlled by
 * the Flags parameter. */
 OsStatus_t
 MmVirtualMap(
	 void *PageDirectory, 
	 PhysicalAddress_t pAddress, 
	 VirtualAddress_t vAddress, 
	 Flags_t Flags)
{
	// Variabes
	OsStatus_t Result = Success;
	PageDirectory_t *Directory = (PageDirectory_t*)PageDirectory;
	PageTable_t *Table = NULL;
	int IsCurrent = 0;

	// Determine page directory 
	// If we were given null, select the cuyrrent
	if (Directory == NULL) {
		Directory = g_PageDirectories[CpuGetCurrentId()];
	}

	// Sanitizie if we are modifying the currently
	// loaded page-directory
	if (g_PageDirectories[CpuGetCurrentId()] == Directory) {
		IsCurrent = 1;
	}

	// Sanitize again
	// If its still null something is wrong
//	assert(Directory != NULL);

	// Get lock on the page-directory 
	// we don't want people to touch 
	// MutexLock(&Directory->Lock);

	// Does page table exist? 
	// If the page-table is not even mapped in we need to 
	// do that beforehand
	if (!(Directory->pTables[PAGE_DIRECTORY_INDEX(vAddress)] & PAGE_PRESENT)) {
		uintptr_t Physical = 0;
		Table = (PageTable_t*)kmalloc_ap(PAGE_SIZE, &Physical);

		// Sanitize the newly allocated table
		// and then initialize the table
//		assert(Table != NULL);
		memset((void*)Table, 0, sizeof(PageTable_t));

		// Install it into our directory, now if the address
		// we are mapping is user-accessible, we should add flags
		Directory->pTables[PAGE_DIRECTORY_INDEX(vAddress)] =
			Physical | PAGE_PRESENT | PAGE_WRITE | Flags;
		Directory->vTables[PAGE_DIRECTORY_INDEX(vAddress)] =
			(uintptr_t)Table;

		// Reload CR3 directory to force 
		// the MMIO to see our changes 
		if (IsCurrent) {
			memory_reload_cr3();
		}
	}
	else {
		Table = (PageTable_t*)Directory->vTables[PAGE_DIRECTORY_INDEX(vAddress)];
	}

	// Sanitize the table before we use it 
	// otherwise we might fuck up
//	assert(Table != NULL);

	// Sanitize that the index isn't already
	// mapped in, thats a fatality
	if (Table->Pages[PAGE_TABLE_INDEX(vAddress)] != 0) {
//		FATAL(FATAL_SCOPE_KERNEL, 
//			"Trying to remap virtual 0x%x to physical 0x%x (original mapping 0x%x)",
//			vAddress, pAddress, Table->Pages[PAGE_TABLE_INDEX(vAddress)]);
	}

	// Map it, make sure we mask the page address
	// so we don't accidently set any flags
	Table->Pages[PAGE_TABLE_INDEX(vAddress)] =
		(pAddress & PAGE_MASK) | PAGE_PRESENT | PAGE_WRITE | Flags;

	// Unlock
//	MutexUnlock(&Directory->Lock);

	// Last step is to invalidate the 
	// the address in the MMIO
	if (IsCurrent) {
		memory_invalidate_addr(vAddress);
	}

	// Done - no errors
	return Result;
}

/* MmVirtualUnmap
 * Unmaps a previous mapping from the given page-directory
 * the mapping must be present */
// OsStatus_t
// MmVirtualUnmap(
// 	 void *PageDirectory, 
// 	 VirtualAddress_t Address)
// {
// 	// Variables needed for finding out page index
// 	OsStatus_t Result = Success;
// 	PageDirectory_t *Directory = (PageDirectory_t*)PageDirectory;
// 	PageTable_t *Table = NULL;
// 	int IsCurrent = 0;

// 	// Determine page directory 
// 	// if pDir is null we get for current cpu
// 	if (Directory == NULL) {
// 		Directory = g_PageDirectories[CpuGetCurrentId()];
// 	}

// 	// Sanitizie if we are modifying the currently
// 	// loaded page-directory
// 	if (g_PageDirectories[CpuGetCurrentId()] == Directory) {
// 		IsCurrent = 1;
// 	}

// 	// Sanitize the page-directory
// 	// If it's still NULL somethings wrong
// //	assert(Directory != NULL);

// 	// Acquire the mutex
// 	// MutexLock(&Directory->Lock);

// 	// Does page table exist? 
// 	// or is a system table, we can't unmap these!
// 	if (!(Directory->pTables[PAGE_DIRECTORY_INDEX(Address)] & PAGE_PRESENT)
// 		|| (Directory->pTables[PAGE_DIRECTORY_INDEX(Address)] & PAGE_SYSTEM_MAP)) {
// 		Result = Error;
// 		goto Leave;
// 	}

// 	/* Acquire the proper page-table */
// 	Table = (PageTable_t*)Directory->vTables[PAGE_DIRECTORY_INDEX(Address)];

// 	// Sanitize the page-index, if it's not mapped in
// 	// then we are trying to unmap somethings that not even mapped
// //	assert(Table->Pages[PAGE_TABLE_INDEX(Address)] != 0);

// 	// System memory? Don't unmap, for gods sake
// 	if (Table->Pages[PAGE_TABLE_INDEX(Address)] & PAGE_SYSTEM_MAP) {
// 		Result = Error;
// 		goto Leave;
// 	}
// 	else
// 	{
// 		// Ok, step one is to extract the physical page of this index
// 		PhysicalAddress_t Physical = Table->Pages[PAGE_TABLE_INDEX(Address)];

// 		// Clear the mapping out
// 		Table->Pages[PAGE_TABLE_INDEX(Address)] = 0;

// 		// Release memory, but don't if it 
// 		// is a virtual mapping, that means we should not free
// 		// the physical page
// 		if (!(Physical & PAGE_VIRTUAL)) {
// 			MmPhysicalFreeBlock(Physical & PAGE_MASK);
// 		}

// 		// Last step is to validate the page-mapping
// 		// now this should be an IPC to all cpu's
// 		if (IsCurrent) {
// 			memory_invalidate_addr(Address);
// 		}
// 	}

// Leave:
// 	// Release the mutex and allow 
// 	// others to use the page-directory
// //	MutexUnlock(&Directory->Lock);

// 	// Done - return error code
// 	return Result;
// }

/* MmVirtualGetMapping
 * Retrieves the physical address mapping of the
 * virtual memory address given - from the page directory 
 * that is given */
PhysicalAddress_t
MmVirtualGetMapping(
	 void *PageDirectory, 
	 VirtualAddress_t Address)
{
	// Initiate our variables
	PageDirectory_t *Directory = (PageDirectory_t*)PageDirectory;
	PageTable_t *Table = NULL;
	PhysicalAddress_t Mapping = 0;

	// If none was given - use the current
	if (Directory == NULL) {
		Directory = g_PageDirectories[CpuGetCurrentId()];
	}

	// Sanitize the page-directory
	// If it's still NULL somethings wrong
//	assert(Directory != NULL);

	// Acquire lock for this directory
	// MutexLock(&Directory->Lock);

	// Is the table even present in the directory? 
	// If not, then no mapping 
	if (!(Directory->pTables[PAGE_DIRECTORY_INDEX(Address)] & PAGE_PRESENT)) {
		goto NotMapped;
	}

	// Fetch the page table from the page-directory
	Table = (PageTable_t*)Directory->vTables[PAGE_DIRECTORY_INDEX(Address)];

	/* Sanitize the page-table just in case */
//	assert(Table != NULL);

	// Sanitize the mapping before anything
	if (!(Table->Pages[PAGE_TABLE_INDEX(Address)] & PAGE_PRESENT)) {
		goto NotMapped;
	}

	// Retrieve mapping
	Mapping = Table->Pages[PAGE_TABLE_INDEX(Address)] & PAGE_MASK;

	// Release mutex on page-directory
	// we should not keep it longer than neccessary
//	MutexUnlock(&Directory->Lock);

	// Done - Return with offset
	return (Mapping + (Address & ATTRIBUTE_MASK));

NotMapped:
	// On fail - release and return 0
//	MutexUnlock(&Directory->Lock);
	return 0;
}

/* MmVirtualInitialMap
 * Maps a virtual memory address to a physical
 * memory address in a given page-directory
 * If page-directory is NULL, current directory
 * is used */
void 
MmVirtualInitialMap(
	 PhysicalAddress_t pAddress, 
	 VirtualAddress_t vAddress)
{
	// Variables
	PageDirectory_t *Directory = g_KernelPageDirectory;
	PageTable_t *Table = NULL;

	// If table is not present in directory
	// we must allocate a new one and install it
	if (!(Directory->pTables[PAGE_DIRECTORY_INDEX(vAddress)] & PAGE_PRESENT)) {
		Table = MmVirtualCreatePageTable();
		Directory->pTables[PAGE_DIRECTORY_INDEX(vAddress)] = (PhysicalAddress_t)Table
			| PAGE_PRESENT | PAGE_WRITE;
		Directory->vTables[PAGE_DIRECTORY_INDEX(vAddress)] = (PhysicalAddress_t)Table;
	}
	else {
		Table = (PageTable_t*)Directory->vTables[PAGE_DIRECTORY_INDEX(vAddress)];
	}

	// Sanitize no previous mapping exists
//	assert(Table->Pages[PAGE_TABLE_INDEX(vAddress)] == 0
//		&& "Dont remap pages without freeing :(");

	// Install the mapping
	Table->Pages[PAGE_TABLE_INDEX(vAddress)] =
		(pAddress & PAGE_MASK) | PAGE_PRESENT | PAGE_WRITE;
}

/* MmReserveMemory
 * Reserves memory for system use - should be allocated
 * from a fixed memory region that won't interfere with
 * general usage */
VirtualAddress_t*
MmReserveMemory(
	 int Pages)
{
	// Variables
	VirtualAddress_t ReturnAddress = 0;

	// Calculate new address 
	// this is a locked operation
//	SpinlockAcquire(&GlbVmLock);
	ReturnAddress = g_ReservedPtr;
	g_ReservedPtr += (PAGE_SIZE * Pages);
//	SpinlockRelease(&GlbVmLock);

	// Done - return address
	return (VirtualAddress_t*)ReturnAddress;
}

/* MmVirtualInit
 * Initializes the virtual memory system and
 * installs default kernel mappings */
OsStatus_t
MmVirtualInit(void)
{
	// Variables
	AddressSpace_t KernelSpace;
	PageTable_t *iTable = NULL;
	int i;

	// Trace information
	LogInformation("Virtual_Memory", "Initializing");

	// Initialize reserved pointer
	g_ReservedPtr = MEMORY_LOCATION_RESERVED;

	// Allocate 3 pages for the kernel page directory
	// and reset it by zeroing it out
	g_KernelPageDirectory = (PageDirectory_t*)
		MmPhysicalAllocateBlock(MEMORY_INIT_MASK, 3);
	memset((void*)g_KernelPageDirectory, 0, sizeof(PageDirectory_t));

	// Allocate initial page directory and
	// identify map the page-directory
	iTable = MmVirtualCreatePageTable();
	MmVirtualFillPageTable(iTable, 0x1000, 0x1000, 0);

	// Install the first page-table
	g_KernelPageDirectory->pTables[0] = (PhysicalAddress_t)iTable | PAGE_USER
		| PAGE_PRESENT | PAGE_WRITE | PAGE_SYSTEM_MAP;
	g_KernelPageDirectory->vTables[0] = (uintptr_t)iTable;
	
	// Initialize locks
//	MutexConstruct(&g_KernelPageDirectory->Lock);
//	SpinlockReset(&GlbVmLock);

	// Pre-map heap region
	LogInformation("Virtual_Memory", "Mapping heap region to 0x%x", MEMORY_LOCATION_HEAP);
	MmVirtualIdentityMapMemoryRange(g_KernelPageDirectory, 0, MEMORY_LOCATION_HEAP,
		(MEMORY_LOCATION_HEAP_END - MEMORY_LOCATION_HEAP), 0, 0);

	// Pre-map video region
	LogInformation("Virtual_Memory", "Mapping video memory to 0x%x", MEMORY_LOCATION_VIDEO);
	MmVirtualIdentityMapMemoryRange(g_KernelPageDirectory, VideoGetTerminal()->Info.FrameBufferAddress,
		MEMORY_LOCATION_VIDEO, (VideoGetTerminal()->Info.BytesPerScanline * VideoGetTerminal()->Info.Height),
		1, PAGE_USER);

	// Install the page table at the reserved system
	// memory, important! 
	LogInformation("Virtual_Memory", "Mapping reserved memory to 0x%x", MEMORY_LOCATION_RESERVED);

	// Iterate all saved physical system memory mappings
	for (i = 0; i < 32; i++) {
		if (SysMappings[i].Length != 0 && SysMappings[i].Type != 1) {
			int PageCount = DIVUP(SysMappings[i].Length, PAGE_SIZE);
			int j;

			// Update virtual address for this entry
			SysMappings[i].vAddressStart = g_ReservedPtr;

			// Map it with our special initial mapping function
			// that is a simplified version
			for (j = 0; j < PageCount; j++, g_ReservedPtr += PAGE_SIZE) {
				MmVirtualInitialMap(
					((SysMappings[i].pAddressStart & PAGE_MASK) + (j * PAGE_SIZE)), 
					g_ReservedPtr);
			}
		}
	}

	// Update video address to the new
	VideoGetTerminal()->Info.FrameBufferAddress = MEMORY_LOCATION_VIDEO;
	// Update video address to the new for the printing string.
	updatePhysBasePtr(MEMORY_LOCATION_VIDEO);

	// Update and switch page-directory for boot-cpu
	MmVirtualSwitchPageDirectory(0, g_KernelPageDirectory, (uintptr_t)g_KernelPageDirectory);
	memory_set_paging(1);
	// Setup kernel addressing space
	KernelSpace.Flags = AS_TYPE_KERNEL;
	KernelSpace.Cr3 = (uintptr_t)g_KernelPageDirectory;
	KernelSpace.PageDirectory = g_KernelPageDirectory;

	LogInformation("Virtual_Memory", "done");
	// Done! 
	return AddressSpaceInitKernel(&KernelSpace);
}

