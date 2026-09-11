/* Includes 
 * - System */
#include <arch/x86/address_space.h>
#include <arch/x86/memory.h>
#include <system/heap.h>
#include <system/log.h>
#include <interrupts/interrupts.h>
#include <string.h>
#include <arch/x86/memory.h>
// #include <system/utils.h>
// #include <threading.h>
// #include <memory.h>
// #include <heap.h>
// #include <log.h>
#include <system/log.h>

/* Includes
 * - Library */
// #include <assert.h>
#include <stddef.h>
#include <string.h>

/* Globals */
static AddressSpace_t GlbKernelAddressSpace;

/* AddressSpaceInitKernel
 * Initializes the Kernel Address Space 
 * This only copies the data into a static global
 * storage, which means users should just pass something
 * temporary structure */
/* The first page-directory index that belongs to user space. Everything
 * below it is kernel and is shared by every address space - not copied,
 * shared: the directory entries point at the same page tables, so a
 * kernel mapping made after a process exists is visible to it too. Copy
 * them instead and a later MmVirtualMap would be invisible to every
 * process created before it. */
#define ADDRESSSPACE_KERNEL_TABLES  PAGE_DIRECTORY_INDEX(MEMORY_LOCATION_KERNEL_END)

/* A PageDirectory_t is two pages (pTables and vTables); the third is a
 * guard. Named so allocate and free cannot drift apart - they already
 * did once, and the result was two frames lost per process with nothing
 * to point at. */
#define ADDRESSSPACE_DIRECTORY_BLOCKS   3

/* Live address spaces, excluding the kernel's. Exposed so the leak this
 * closes is visible rather than something you have to infer from the
 * physical block count. */
static volatile size_t GlbAddressSpaceCount = 0;

/* AddressSpaceGetCount */
size_t AddressSpaceGetCount(void)
{
	return GlbAddressSpaceCount;
}

/* AddressSpaceCreate
 * Builds an address space that shares the kernel half and has an empty
 * user half. */
AddressSpace_t *AddressSpaceCreate(Flags_t Flags)
{
	AddressSpace_t *Space;
	PageDirectory_t *Directory;
	PageDirectory_t *Kernel;
	int i;

	Space = (AddressSpace_t*)kmalloc(sizeof(AddressSpace_t));
	if (Space == NULL) {
		LogFatal("AddressSpace", "out of memory");
		return NULL;
	}

	/* Three blocks: the structure is two pages, the third is a guard,
	 * matching how MmVirtualInit allocates the kernel's own. */
	Directory = (PageDirectory_t*)MmPhysicalAllocateBlock(MEMORY_INIT_MASK,
		ADDRESSSPACE_DIRECTORY_BLOCKS);
	if (Directory == NULL) {
		kfree(Space);
		LogFatal("AddressSpace", "no physical memory for a page-directory");
		return NULL;
	}

	memset((void*)Directory, 0, sizeof(PageDirectory_t));

	Kernel = (PageDirectory_t*)GlbKernelAddressSpace.PageDirectory;
	for (i = 0; i < ADDRESSSPACE_KERNEL_TABLES; i++) {
		Directory->pTables[i] = Kernel->pTables[i];
		Directory->vTables[i] = Kernel->vTables[i];
	}

	Space->References = 1;
	Space->Flags = Flags;
	Space->Cr3 = (uintptr_t)Directory;
	Space->PageDirectory = (void*)Directory;

	GlbAddressSpaceCount++;
	return Space;
}

/* AddressSpaceDestroy
 * Releases an address space once nothing references it. Only the user
 * half is freed - the kernel tables are shared, and freeing them would
 * unmap the kernel from every other address space at once. */
OsStatus_t AddressSpaceDestroy(AddressSpace_t *Space)
{
	PageDirectory_t *Directory;
	int i;

	if (Space == NULL || Space == &GlbKernelAddressSpace) {
		return Error;
	}

	Space->References--;
	if (Space->References > 0) {
		return Success;
	}

	Directory = (PageDirectory_t*)Space->PageDirectory;

	for (i = ADDRESSSPACE_KERNEL_TABLES; i < TABLES_PER_PDIR; i++) {
		if (Directory->pTables[i] & PAGE_PRESENT) {
			PageTable_t *Table = (PageTable_t*)Directory->vTables[i];
			int j;

			/* Release the frames the user half was using. */
			for (j = 0; j < PAGES_PER_TABLE; j++) {
				if (Table->Pages[j] & PAGE_PRESENT) {
					MmPhysicalFreeBlock(Table->Pages[j] & PAGE_MASK);
				}
			}
			MmPhysicalFreeBlock((PhysicalAddress_t)Directory->vTables[i]);
		}
	}

	MmPhysicalFreeBlocks((PhysicalAddress_t)Directory,
		ADDRESSSPACE_DIRECTORY_BLOCKS);
	kfree(Space);

	if (GlbAddressSpaceCount > 0) {
		GlbAddressSpaceCount--;
	}
	return Success;
}

/* AddressSpaceGetKernel */
AddressSpace_t *AddressSpaceGetKernel(void)
{
	return &GlbKernelAddressSpace;
}

/* AddressSpaceSwitch
 * Loads the given space on the current cpu. */
OsStatus_t AddressSpaceSwitch(AddressSpace_t *Space)
{
	if (Space == NULL) {
		return Error;
	}
	return MmVirtualSwitchPageDirectory(0,
		(PageDirectory_t*)Space->PageDirectory, Space->Cr3);
}

OsStatus_t AddressSpaceInitKernel(
	AddressSpace_t *Kernel)
{
	// Sanitize parameter
	// assert(Kernel != NULL);

	// Copy data into our static storage
	GlbKernelAddressSpace.Cr3 = Kernel->Cr3;
	GlbKernelAddressSpace.Flags = Kernel->Flags;
	GlbKernelAddressSpace.PageDirectory = Kernel->PageDirectory;

	// Setup reference and lock
	// SpinlockReset(&GlbKernelAddressSpace.Lock);
	GlbKernelAddressSpace.References = 1;

	// No errors
	return Success;
}

/* AddressSpaceCreate
 * Initialize a new address space, depending on 
 * what user is requesting we might recycle a already
 * existing address space */
//// TODO: Implement later with threading.
// AddressSpace_t*
// AddressSpaceCreate(
// 	Flags_t Flags)
// {
// 	// Variables
// 	AddressSpace_t *AddressSpace = NULL;
// 	UUId_t CurrentCpu = 0;//CpuGetCurrentId();
// 	int Itr = 0;

// 	// If we want to create a new kernel address
// 	// space we instead want to re-use the current 
// 	// If kernel is specified, ignore rest 
// 	if (Flags & AS_TYPE_KERNEL) {
// 		// SpinlockAcquire(&GlbKernelAddressSpace.Lock);
// 		GlbKernelAddressSpace.References++;
// 		// SpinlockRelease(&GlbKernelAddressSpace.Lock);
// 		AddressSpace = &GlbKernelAddressSpace;
// 	}
// 	else if (Flags == AS_TYPE_INHERIT) {
// 		// Inheritance is a bit different, we re-use again
// 		// but instead of reusing the kernel, we reuse the current
// 		// MCoreThread_t *Current = ThreadingGetCurrentThread(CurrentCpu);
// 		// SpinlockAcquire(&Current->AddressSpace->Lock);
// 		// Current->AddressSpace->References++;
// 		// SpinlockRelease(&Current->AddressSpace->Lock);
// 		// AddressSpace = Current->AddressSpace;
// 	}
// 	else if (Flags & (AS_TYPE_APPLICATION | AS_TYPE_DRIVER))
// 	{
// 		// This is the only case where we should create a 
// 		// new and seperate address space, user processes!
// 		uintptr_t PhysAddr = 0;
// 		PageDirectory_t *NewPd = (PageDirectory_t*)kmalloc_ap(sizeof(PageDirectory_t), &PhysAddr);
// 		PageDirectory_t *CurrPd = (PageDirectory_t*)AddressSpaceGetCurrent()->PageDirectory;
// 		PageDirectory_t *KernPd = (PageDirectory_t*)GlbKernelAddressSpace.PageDirectory;

// 		// Copy at max kernel directories up to MEMORY_SEGMENT_RING3_BASE
// 		int KernelRegion = 0;
// 		int KernelRegionEnd = PAGE_DIRECTORY_INDEX(MEMORY_LOCATION_KERNEL_END);

// 		// Lookup which table-region is the stack region
// 		int StackRegion = PAGE_DIRECTORY_INDEX(MEMORY_LOCATION_STACK_END);
// 		int StackRegionEnd = PAGE_DIRECTORY_INDEX(MEMORY_SEGMENT_STACK_BASE);

// 		// Allocate a new address space
// 		AddressSpace = (AddressSpace_t*)kmalloc(sizeof(AddressSpace_t));
// 		memset(NewPd, 0, sizeof(PageDirectory_t));

// 		// Initialize members
// 		// MutexConstruct(&NewPd->Lock);

// 		// Initialize base mappings
// 		for (Itr = 0; Itr < TABLES_PER_PDIR; Itr++) {
// 			// Sanitize if it's inside kernel region
// 			if (Itr >= KernelRegion && Itr < KernelRegionEnd) {
// 				NewPd->pTables[Itr] = KernPd->pTables[Itr];
// 				NewPd->vTables[Itr] = KernPd->vTables[Itr];
// 				continue;
// 			}

// 			// Sanitize stack region, never copy
// 			if (Itr >= StackRegion && Itr < StackRegionEnd) {
// 				continue;
// 			}

// 			// Inherit? We must mark that table inherited to avoid
// 			// it being freed again
// 			if (Flags & AS_TYPE_INHERIT && CurrPd->pTables[Itr]) {
// 				NewPd->pTables[Itr] = CurrPd->pTables[Itr] | PAGE_INHERITED;
// 				NewPd->vTables[Itr] = CurrPd->vTables[Itr];
// 			}
// 		}

// 		// Store new configuration into AS
// 		AddressSpace->Flags = Flags;
// 		AddressSpace->Cr3 = PhysAddr;
// 		AddressSpace->PageDirectory = NewPd;

// 		// Reset lock and ref count
// 		// SpinlockReset(&AddressSpace->Lock);
// 		AddressSpace->References = 1;
// 	}
// 	else {
// 		LogFatal("VMEM", "Invalid flags parsed in AddressSpaceCreate 0x%x", Flags);
// 	}

// 	/* Done */
// 	return AddressSpace;
// }

/* AddressSpaceDestroy
 * Destroy and release all resources related
 * to an address space, only if there is no more
 * references */
//// TODO: Implement later with the threading.
// OsStatus_t
// AddressSpaceDestroy(
// 	AddressSpace_t *AddressSpace)
// {
// 	// Acquire lock on the address space
// 	// SpinlockAcquire(&AddressSpace->Lock);
// 	AddressSpace->References--;

// 	// In case that was the last reference
// 	// cleanup the address space otherwise
// 	// just unlock
// 	if (AddressSpace->References == 0) {
// 		if (AddressSpace->Flags & (AS_TYPE_APPLICATION | AS_TYPE_DRIVER)) {
// 			PageDirectory_t *KernPd = (PageDirectory_t*)GlbKernelAddressSpace.PageDirectory;
// 			PageDirectory_t *Pd = (PageDirectory_t*)AddressSpace->PageDirectory;
// 			int i, j;

// 			// Iterate page-mappings
// 			for (i = 0; i < TABLES_PER_PDIR; i++) {
// 				if (Pd->pTables[i] == 0)
// 					continue;

// 				// Is it a kernel page-table? Ignore it 
// 				if (Pd->pTables[i] == KernPd->pTables[i])
// 					continue;

// 				// Is this an inherited page-table?
// 				// We don't free our parents stuff
// 				if (Pd->pTables[i] & PAGE_INHERITED)
// 					continue;

// 				// Ok, OUR user page-table, free everything in it
// 				PageTable_t *Pt = (PageTable_t*)Pd->vTables[i];

// 				// Iterate pages in table
// 				for (j = 0; j < PAGES_PER_TABLE; j++) {
// 					if (Pt->Pages[i] & PAGE_VIRTUAL)
// 						continue;

// 					// If it has a mapping - free it
// 					if (Pt->Pages[i] != 0) {
// 						MmPhysicalFreeBlock(Pt->Pages[i] & PAGE_MASK);
// 					}
// 				}

// 				// Free the page-table
// 				kfree(Pt);
// 			}
// 		}

// 		/* Free structure */
// 		kfree(AddressSpace);
// 	}
// 	else {
// 		// SpinlockRelease(&AddressSpace->Lock);
// 	}

// 	// No errors
// 	return Success;
// }

/* AddressSpaceGetCurrent
 * Returns the current address space
 * if there is no active threads or threading
 * is not setup it returns the kernel address space */
//// TODO: Implement later
// AddressSpace_t*
// AddressSpaceGetCurrent(void)
// {
// 	// Lookup current thread
// 	MCoreThread_t *CurrThread = 
// 		ThreadingGetCurrentThread(CpuGetCurrentId());

// 	// if no threads are active return
// 	// the kernel address space
// 	if (CurrThread == NULL) {
// 		return &GlbKernelAddressSpace;
// 	}
// 	else {
// 		return CurrThread->AddressSpace;
// 	}
// }

/* AddressSpaceSwitch
 * Switches the current address space out with the
 * the address space provided for the current cpu */
// OsStatus_t
// AddressSpaceSwitch(
// 	AddressSpace_t *AddressSpace)
// {
// 	// Redirect to our virtual memory manager
//     //// TODO: APIC is not implemented yet so CpuGetCurrentId() return 0.
// 	// return MmVirtualSwitchPageDirectory(CpuGetCurrentId(),
// 	// 	AddressSpace->PageDirectory, AddressSpace->Cr3);
//     return MmVirtualSwitchPageDirectory(0,
// 		AddressSpace->PageDirectory, AddressSpace->Cr3);
// }

/* AddressSpaceTranslate
 * Translates the given address to the correct virtual
 * address, this can be used to correct any special cases on
 * virtual addresses in the sub-layer */
// VirtualAddress_t
// AddressSpaceTranslate(
// 	AddressSpace_t *AddressSpace,
// 	VirtualAddress_t Address)
// {
// 	// Sanitize on the address, and the
// 	// the type of addressing space 
// 	if (AddressSpace->Flags & AS_TYPE_KERNEL) {
// 		return Address;
// 	}
// 	else {
// 		return Address;
// 	}
// }

/* AddressSpaceMap
 * Maps the given virtual address into the given address space
 * automatically allocates physical pages based on the passed Flags
 * It returns the start address of the allocated physical region */
//// TODO: Do it later
// OsStatus_t
// AddressSpaceMap(
// 	AddressSpace_t *AddressSpace,
// 	VirtualAddress_t Address,
// 	size_t Size,
// 	uintptr_t Mask,
// 	Flags_t Flags,
// 	uintptr_t *Physical)
// {
// 	// Variables
// 	size_t PageCount = DIVUP(Size, PAGE_SIZE);
// 	PhysicalAddress_t PhysicalBase = 0;
// 	Flags_t AllocFlags = 0;
// 	size_t Itr = 0;

// 	// Parse and convert flags
// 	if (Flags & AS_FLAG_APPLICATION) {
// 		AllocFlags |= PAGE_USER;
// 	}
// 	if (Flags & AS_FLAG_NOCACHE) {
// 		AllocFlags |= PAGE_CACHE_DISABLE;
// 	}
// 	if (Flags & AS_FLAG_VIRTUAL) {
// 		AllocFlags |= PAGE_VIRTUAL;
// 	}
// 	if (Flags & AS_FLAG_CONTIGIOUS) {
// 		PhysicalBase = MmPhysicalAllocateBlock(Mask, (int)PageCount);
// 	}

// 	// Iterate the number of pages to map 
// 	for (Itr = 0; Itr < PageCount; Itr++) {
// 		uintptr_t PhysBlock = 0;
// 		if (PhysicalBase != 0) {
// 			PhysBlock = PhysicalBase + (Itr * PAGE_SIZE);
// 		}
// 		else {
// 			PhysBlock = MmPhysicalAllocateBlock(Mask, 1);
// 		}

// 		// Only return the base physical page
// 		if (PhysicalBase == 0) {
// 			PhysicalBase = PhysBlock;
// 		}

// 		// Redirect call to our virtual page manager
// 		if (MmVirtualMap(AddressSpace->PageDirectory, PhysBlock,
// 			(Address + (Itr * PAGE_SIZE)), AllocFlags) != Success) {
// 			return Error;
// 		}
// 	}

// 	// Update out and return
// 	if (Physical != NULL) {
// 		*Physical = PhysicalBase;
// 	}
// 	return Success;
// }

/* AddressSpaceMapFixed
 * Maps the given virtual address into the given address space
 * uses the given physical pages instead of automatic allocation
 * It returns the start address of the allocated physical region */
// OsStatus_t
// AddressSpaceMapFixed(
// 	AddressSpace_t *AddressSpace,
// 	PhysicalAddress_t pAddress, 
// 	VirtualAddress_t vAddress, 
// 	size_t Size, 
// 	Flags_t Flags)
// {
// 	// Variables
// 	size_t PageCount = DIVUP(Size, PAGE_SIZE);
// 	int AllocFlags = 0;
// 	size_t Itr = 0;

// 	// Parse and convert flags
// 	// MapFixed does not support CONTIGIOUS
// 	if (Flags & AS_FLAG_APPLICATION) {
// 		AllocFlags |= PAGE_USER;
// 	}
// 	if (Flags & AS_FLAG_NOCACHE) {
// 		AllocFlags |= PAGE_CACHE_DISABLE;
// 	}
// 	if (Flags & AS_FLAG_VIRTUAL) {
// 		AllocFlags |= PAGE_VIRTUAL;
// 	}

// 	// Now map it in by redirecting to virtual memory manager
// 	for (Itr = 0; Itr < PageCount; Itr++) {
// 		MmVirtualMap(AddressSpace->PageDirectory, (pAddress + (Itr * PAGE_SIZE)),
// 			(vAddress + (Itr * PAGE_SIZE)), (uint32_t)AllocFlags);
// 	}

// 	// No errors
// 	return Success;
// }

/* AddressSpaceUnmap
 * Unmaps a virtual memory region from an address space */
//// TODO: Don't need yet
// OsStatus_t
// AddressSpaceUnmap(
// 	AddressSpace_t *AddressSpace, 
// 	VirtualAddress_t Address, 
// 	size_t Size)
// {
// 	// Variables
// 	size_t PageCount = DIVUP(Size, PAGE_SIZE);
// 	size_t Itr = 0;

// 	// Iterate page-count and unmap
// 	for (Itr = 0; Itr < PageCount; Itr++) {
// 		MmVirtualUnmap(AddressSpace->PageDirectory, (Address + (Itr * PAGE_SIZE)));
// 	}
	
// 	// Done - no errors
// 	return Success;
// }

/* AddressSpaceGetMap
 * Retrieves a physical mapping from an address space determined
 * by the virtual address given */
// PhysicalAddress_t
// AddressSpaceGetMap(
// 	AddressSpace_t *AddressSpace, 
// 	VirtualAddress_t Address) {
// 	return MmVirtualGetMapping(AddressSpace->PageDirectory, Address);
// }