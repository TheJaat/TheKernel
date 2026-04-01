#ifndef _MCORE_ADDRESSINGSPACE_H_
#define _MCORE_ADDRESSINGSPACE_H_

/* Includes 
 * - Library */
// #include <os/osdefs.h>
#include <defs.h>

// #include <os/spinlock.h>

/* Includes
 * - System */
// #include <arch.h>
#include <arch/x86/x32/arch_x32.h>

#include <stddef.h>


/* Address Space Structure
 * Denotes the must have and architecture specific
 * members of an addressing space */
// PACKED_TYPESTRUCT(AddressSpace, {
// 	Spinlock_t				Lock;
// 	int						References;
// 	Flags_t					Flags;

// 	// Architecture Members
// 	ADDRESSSPACE_MEMBERS
// });

typedef struct AddressSpace {
    // Spinlock_t Lock;
    int References;
    Flags_t Flags;

    ADDRESSSPACE_MEMBERS
} __attribute__((packed)) AddressSpace_t;

/* Address space creation flags, use these
 * to specify which kind of address space that is
 * created */
#define AS_TYPE_KERNEL					0x00000001
#define AS_TYPE_INHERIT					0x00000002
#define AS_TYPE_APPLICATION				0x00000004
#define AS_TYPE_DRIVER					0x00000008

/* Address space mapping flags, use these to
 * specify which kind of address-map that is
 * being doine */
#define AS_FLAG_APPLICATION				0x00000001
#define AS_FLAG_RESERVE					0x00000002
#define AS_FLAG_NOCACHE					0x00000004
#define AS_FLAG_VIRTUAL					0x00000008
#define AS_FLAG_CONTIGIOUS				0x00000010

#ifdef __cplusplus
extern "C" {
#endif

/* AddressSpaceInitKernel
 * Initializes the Kernel Address Space 
 * This only copies the data into a static global
 * storage, which means users should just pass something
 * temporary structure */
OsStatus_t AddressSpaceInitKernel(
	AddressSpace_t *Kernel);

/* AddressSpaceCreate
 * Initialize a new address space, depending on 
 * what user is requesting we might recycle a already
 * existing address space */
// AddressSpace_t* AddressSpaceCreate(
// 	Flags_t Flags);

/* AddressSpaceDestroy
 * Destroy and release all resources related
 * to an address space, only if there is no more
 * references */
// OsStatus_t AddressSpaceDestroy(
// 	AddressSpace_t *AddressSpace);

/* AddressSpaceSwitch
 * Switches the current address space out with the
 * the address space provided for the current cpu */
// OsStatus_t AddressSpaceSwitch(
// 	AddressSpace_t *AddressSpace);

/* AddressSpaceGetCurrent
 * Returns the current address space
 * if there is no active threads or threading
 * is not setup it returns the kernel address space */
AddressSpace_t* AddressSpaceGetCurrent(void);

/* AddressSpaceTranslate
 * Translates the given address to the correct virtual
 * address, this can be used to correct any special cases on
 * virtual addresses in the sub-layer */
// VirtualAddress_t AddressSpaceTranslate(
// 	AddressSpace_t *AddressSpace,
// 	VirtualAddress_t Address);

/* AddressSpaceMap
 * Maps the given virtual address into the given address space
 * automatically allocates physical pages based on the passed Flags
 * It returns the start address of the allocated physical region */
// OsStatus_t AddressSpaceMap(
// 	AddressSpace_t *AddressSpace,
// 	VirtualAddress_t Address, 
// 	size_t Size,
// 	uintptr_t Mask, 
// 	Flags_t Flags,
// 	uintptr_t *Physical);

/* AddressSpaceMapFixed
 * Maps the given virtual address into the given address space
 * uses the given physical pages instead of automatic allocation
 * It returns the start address of the allocated physical region */
// OsStatus_t AddressSpaceMapFixed(
// 	AddressSpace_t *AddressSpace,
// 	PhysicalAddress_t pAddress, 
// 	VirtualAddress_t vAddress, 
// 	size_t Size, 
// 	Flags_t Flags);

/* AddressSpaceUnmap
 * Unmaps a virtual memory region from an address space */
OsStatus_t AddressSpaceUnmap(
	AddressSpace_t *AddressSpace, 
	VirtualAddress_t Address, 
	size_t Size);

/* AddressSpaceGetMap
 * Retrieves a physical mapping from an address space determined
 * by the virtual address given */
// PhysicalAddress_t AddressSpaceGetMap(
// 	AddressSpace_t *AddressSpace, 
// 	VirtualAddress_t Address);


#ifdef __cplusplus
}
#endif

#endif
