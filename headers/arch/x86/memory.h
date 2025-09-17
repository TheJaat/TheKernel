#ifndef __MEMORY_H__
#define __MEMORY_H__

#include <stdint.h>
#include <stddef.h>
#include <defs.h>
#include <boot/datastructure.h>


// Structural Sizes
#define PAGES_PER_TABLE		1024
#define TABLES_PER_PDIR		1024
#define TABLE_SPACE_SIZE	0x400000
#define DIRECTORY_SPACE_SIZE	0xFFFFFFFF

/* Only allocate memory below 4mb */
#define MEMORY_INIT_MASK	0x3FFFFF

/* Shared PT/Page Definitions */
#define PAGE_PRESENT		0x1
#define PAGE_WRITE		0x2
#define PAGE_USER		0x4
#define PAGE_WRITETHROUGH	0x8
#define PAGE_CACHE_DISABLE	0x10
#define PAGE_ACCESSED		0x20

/* Page Table Definitions */
#define PAGETABLE_UNUSED	0x40
#define PAGETABLE_4MB		0x80
#define PAGETABLE_IGNORED	0x100

/* Page Definitions */
#define PAGE_DIRTY		0x40
#define PAGE_UNUSED		0x80
#define PAGE_GLOBAL		0x100

/* MollenOS PT/Page Definitions */
#define PAGE_SYSTEM_MAP		0x200
#define PAGE_INHERITED		0x400
#define PAGE_VIRTUAL		0x800

/* Masks */
#define PAGE_MASK		0xFFFFF000
#define ATTRIBUTE_MASK		0x00000FFF

/* Index's */
#define PAGE_DIRECTORY_INDEX(x) (((x) >> 22) & 0x3FF)
#define PAGE_TABLE_INDEX(x) (((x) >> 12) & 0x3FF)

/* Page Table Structure
 * Denotes how the paging structure is for the X86-32
 * platform, this is different from X86-64 */
typedef struct PageTable {
	uint32_t	Pages[PAGES_PER_TABLE];
} __attribute__((packed)) PageTable_t;


/* Page Directory Structure
 * Denotes how the paging structure is for the X86-32
 * platform, this is different from X86-64 */
typedef struct PageDirectory {
	uint32_t	pTables[TABLES_PER_PDIR];	// Seen by MMU
	uint32_t	vTables[TABLES_PER_PDIR];	// Not seen by MMU
	//Mutex_t		Lock;				// Not seen by MMU
} __attribute__((packed)) PageDirectory_t;

/* Memory Map Structure 
 * This is the structure passed to us by
 * the mBoot bootloader */
typedef struct BIOSMemoryRegion {
	uint64_t				Address;
	uint64_t				Size;
	uint32_t				Type;		//1 => Available, 2 => ACPI, 3 => Reserved
	uint32_t				Nil;
	uint64_t				Padding;
} __attribute__((packed)) BIOSMemoryRegion_t;

/* System reserved memory mappings
 * this is to faster/safer map in system
 * memory like ACPI/device memory etc etc */
typedef struct SystemMemoryMapping {
	PhysicalAddress_t		pAddressStart;
	VirtualAddress_t		vAddressStart;
	size_t					Length;
	int						Type;	//Type. 2 - ACPI
} __attribute__((packed)) SystemMemoryMapping_t;


#ifdef __cplusplus
extern "C" {
#endif

/* MmPhyiscalInit
 * This is the physical memory manager initializor
 * It reads the multiboot memory descriptor(s), initialies
 * the bitmap and makes sure reserved regions are allocated */
OsStatus_t MmPhyiscalInit(void *BootInfo, BootDescriptor_t *Descriptor);

/* MmPhysicalAllocateBlock
 * This is the primary function for allocating
 * physical memory pages, this takes an argument
 * <Mask> which determines where in memory the allocation is OK */
PhysicalAddress_t MmPhysicalAllocateBlock(uintptr_t Mask, int Count);


/* MmVirtualMap
 * Installs a new page-mapping in the given
 * page-directory. The type of mapping is controlled by
 * the Flags parameter. */
OsStatus_t MmVirtualMap(void *PageDirectory, PhysicalAddress_t pAddress, VirtualAddress_t vAddress, Flags_t Flags);

/* MmVirtualInit
 * Initializes the virtual memory system and
 * installs default kernel mappings
 */
OsStatus_t MmVirtualInit(void);


/* MmVirtualGetMapping
 * Retrieves the physical address mapping of the
 * virtual memory address given - from the page directory 
 * that is given */
PhysicalAddress_t MmVirtualGetMapping(
	void *PageDirectory, 
	VirtualAddress_t Address);

#ifdef __cplusplus
}
#endif


#endif /* __MEMORY_H__ */
