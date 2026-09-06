#ifndef __HEAP_H__
#define __HEAP_H__

/* Includes
 * - Library */
#include <stdint.h>
#include <stddef.h>
#include <defs.h>

/* Includes
 * - System */
#include <arch/x86/x32/arch_x32.h>

/***************************
 * Heap Management
 ***************************/

/* The heap is laid out inside MEMORY_LOCATION_HEAP .. MEMORY_LOCATION_HEAP_END
 * (0x1000000 .. 0x4000000, 48 mB) as:
 *
 *   HeapBase        0x1000000  +---------------------------+
 *                              | block/node headers        |  MEMORY_STATIC_OFFSET
 *   MemStartData    0x1400000  +---------------------------+
 *                              | actual allocations        |
 *   HeapEnd         0x4000000  +---------------------------+
 *
 * Nothing here is mapped up front. MmVirtualInit() installs empty page
 * tables across the whole range; pages are mapped on demand as the heap
 * grows into them. */
#define MEMORY_STATIC_OFFSET    0x400000
#define HEAP_NORMAL_BLOCK       0x2000
#define HEAP_LARGE_BLOCK        0x20000

#define HEAP_IDENT_SIZE         8
#define HEAP_STANDARD_ALIGN     4

/* The basic HeapNode, describes one allocation (or one free gap) inside
 * a block. */
typedef struct _HeapNode {
    char                Identifier[HEAP_IDENT_SIZE];
    uintptr_t           Address;
    Flags_t             Flags;
    size_t              Length;
    struct _HeapNode   *Link;
} HeapNode_t;

/* Heap node flags */
#define NODE_ALLOCATED          0x1

/* A block descriptor - a bucket covering an address range. */
typedef struct _HeapBlock {
    uintptr_t           AddressStart;
    uintptr_t           AddressEnd;
    Flags_t             Flags;
    uintptr_t           Mask;
    size_t              BytesFree;
    struct _HeapBlock  *Link;
    struct _HeapNode   *Nodes;
} HeapBlock_t;

/* Heap block flags */
#define BLOCK_NORMAL            0x0
#define BLOCK_ALIGNED           0x1
#define BLOCK_VERY_LARGE        0x2

/* A heap region that can be allocated from. */
typedef struct _HeapRegion {
    uintptr_t           HeapBase;
    uintptr_t           MemStartData;
    uintptr_t           MemHeaderCurrent;
    uintptr_t           MemHeaderMax;
    uintptr_t           HeapEnd;

    int                 IsUser;

    size_t              BytesAllocated;
    size_t              NumAllocs;
    size_t              NumFrees;
    size_t              NumPages;

    /* No threading yet, so no real lock. See HeapLock() in heap.c for
     * where to add one once you have scheduling. */
    int                 Lock;

    HeapBlock_t        *BlockRecycler;
    HeapNode_t         *NodeRecycler;

    struct _HeapBlock  *Blocks;
    struct _HeapBlock  *PageBlocks;
    struct _HeapBlock  *CustomBlocks;
} Heap_t;

/* Allocation Flags */
#define ALLOCATION_COMMIT       0x1

#ifdef __cplusplus
extern "C" {
#endif

/* HeapInit
 * Initializes the kernel heap. MUST be called after MmVirtualInit and
 * before any call to kmalloc. Returns Success or Error - unlike the
 * reference this does not assert its way out of a failure. */
OsStatus_t HeapInit(void);

/* HeapAllocate
 * Finds a suitable block and allocates in it. Returns 0 on failure. */
uintptr_t HeapAllocate(Heap_t *Heap, size_t Size, Flags_t Flags,
    size_t Alignment, uintptr_t Mask, const char *Identifier);

/* HeapFree
 * Finds the block that should contain the address and frees it. */
void HeapFree(Heap_t *Heap, uintptr_t Address);

/* HeapCreate
 * Creates a secondary heap over an arbitrary virtual range. The range
 * must already have page tables installed in the current directory. */
Heap_t *HeapCreate(uintptr_t HeapAddress, uintptr_t HeapEnd, int UserHeap);

/* HeapQueryMemoryInformation
 * Queries bytes-in-use / blocks-allocated for a heap. NULL = kernel heap. */
int HeapQueryMemoryInformation(Heap_t *Heap, size_t *BytesInUse,
    size_t *BlocksAllocated);

/* HeapValidateAddress
 * Returns 0 if <Address> is a live allocation in <Heap>, -1 otherwise.
 * NULL = kernel heap. */
int HeapValidateAddress(Heap_t *Heap, uintptr_t Address);

/* HeapPrintStats
 * Dumps block/node counts through the logger. NULL = kernel heap. */
void HeapPrintStats(Heap_t *Heap);

/* kmalloc and friends - wrappers around HeapAllocate on the kernel heap.
 * All of them return NULL on failure; check it. */
void *kmalloc(size_t Size);
void *kmalloc_a(size_t Size);                                /* page aligned */
void *kmalloc_p(size_t Size, uintptr_t *Ptr);                /* + physical   */
void *kmalloc_ap(size_t Size, uintptr_t *Ptr);               /* both         */
void *kmalloc_apm(size_t Size, uintptr_t *Ptr, uintptr_t Mask);
void  kfree(void *p);

/* HeapTest
 * Small self-check suite. Safe to delete once you trust the heap. */
void HeapTest(void);

#ifdef __cplusplus
}
#endif

#endif /* __HEAP_H__ */