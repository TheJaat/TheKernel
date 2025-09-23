#ifndef __ADDRESS_SPACE_H__
#define __ADDRESS_SPACE_H__

#include <arch/x86/x32/arch_x32.h>
#include <stddef.h>

/* Address Space Structure
 * Denotes the must have and architecture specific
 * members of an addressing space */
typedef struct AddressSpace {
	//Spinlock_t				Lock;
	int						References;
	Flags_t					Flags;

	// Architecture Members
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
OsStatus_t AddressSpaceInitKernel(AddressSpace_t *Kernel);

/* AddressSpaceGetMap
 * Retrieves a physical mapping from an address space determined
 * by the virtual address given */
PhysicalAddress_t AddressSpaceGetMap(
	AddressSpace_t *AddressSpace, 
	VirtualAddress_t Address);


OsStatus_t
AddressSpaceMap(AddressSpace_t *AddressSpace,
    VirtualAddress_t Address,
    size_t Size,
    uintptr_t Mask,
    Flags_t Flags,
    uintptr_t *Physical);

AddressSpace_t*
AddressSpaceGetCurrent(void);

#ifdef __cplusplus
}
#endif

#endif
