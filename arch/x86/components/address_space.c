#include <defs.h>
#include <system/address_space.h>

#include <arch/x86/memory.h>
#include <heap.h>

//#include <threading.h>

/* Includes
 * - Library
 */
#include <stddef.h>
#include <string.h>

/* Globals */
static AddressSpace_t g_KernelAddressSpace;

/* AddressSpaceInitKernel
 * Initializes the Kernel Address Space 
 * This only copies the data into a static global
 * storage, which means users should just pass something
 * temporary structure
 */
OsStatus_t AddressSpaceInitKernel(AddressSpace_t *Kernel)
{
	// Sanitize parameter
//	assert(Kernel != NULL);

	// Copy data into our static storage
	g_KernelAddressSpace.Cr3 = Kernel->Cr3;
	g_KernelAddressSpace.Flags = Kernel->Flags;
	g_KernelAddressSpace.PageDirectory = Kernel->PageDirectory;

	// Setup reference and lock
//	SpinlockReset(&g_KernelAddressSpace.Lock);
	g_KernelAddressSpace.References = 1;

	// No errors
	return Success;
}


/* AddressSpaceMap
 * Maps the given virtual address into the given address space
 * automatically allocates physical pages based on the passed Flags
 * It returns the start address of the allocated physical region */
OsStatus_t
AddressSpaceMap(AddressSpace_t *AddressSpace,
VirtualAddress_t Address,
size_t Size,
uintptr_t Mask,
Flags_t Flags,
uintptr_t *Physical)
{
	// Variables
	size_t PageCount = DIVUP(Size, PAGE_SIZE);
	PhysicalAddress_t PhysicalBase = 0;
	Flags_t AllocFlags = 0;
	size_t Itr = 0;

	// Parse and convert flags
	if (Flags & AS_FLAG_APPLICATION) {
		AllocFlags |= PAGE_USER;
	}
	if (Flags & AS_FLAG_NOCACHE) {
		AllocFlags |= PAGE_CACHE_DISABLE;
	}
	if (Flags & AS_FLAG_VIRTUAL) {
		AllocFlags |= PAGE_VIRTUAL;
	}
	if (Flags & AS_FLAG_CONTIGIOUS) {
		PhysicalBase = MmPhysicalAllocateBlock(Mask, (int)PageCount);
	}

	// Iterate the number of pages to map 
	for (Itr = 0; Itr < PageCount; Itr++) {
		uintptr_t PhysBlock = 0;
		if (PhysicalBase != 0) {
			PhysBlock = PhysicalBase + (Itr * PAGE_SIZE);
		}
		else {
			PhysBlock = MmPhysicalAllocateBlock(Mask, 1);
		}

		// Only return the base physical page
		if (PhysicalBase == 0) {
			PhysicalBase = PhysBlock;
		}

		// Redirect call to our virtual page manager
		if (MmVirtualMap(AddressSpace->PageDirectory, PhysBlock,
			(Address + (Itr * PAGE_SIZE)), AllocFlags) != Success) {
			return Error;
		}
	}

	// Update out and return
	if (Physical != NULL) {
		*Physical = PhysicalBase;
	}
	return Success;
}


/* AddressSpaceGetCurrent
 * Returns the current address space
 * if there is no active threads or threading
 * is not setup it returns the kernel address space */
AddressSpace_t*
AddressSpaceGetCurrent(void)
{
// TODO: It returnes the currrent thread's address space
// however, i just only have kernel space for the time being.
	// Lookup current thread
	//MCoreThread_t *CurrThread = 
	//	ThreadingGetCurrentThread(CpuGetCurrentId());

	// if no threads are active return
	// the kernel address space
	//if (CurrThread == NULL) {
		return &g_KernelAddressSpace;
	//}
	//else {
	//	return CurrThread->AddressSpace;
	//}
}

/* AddressSpaceGetMap
 * Retrieves a physical mapping from an address space determined
 * by the virtual address given */
PhysicalAddress_t
AddressSpaceGetMap(
	AddressSpace_t *AddressSpace, 
	VirtualAddress_t Address) {
	return MmVirtualGetMapping(AddressSpace->PageDirectory, Address);
}
