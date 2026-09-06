/* Includes
 * - System */
#include <system/heap.h>
#include <system/log.h>
#include <arch/x86/memory.h>
#include <arch/x86/x32/arch_x32.h>

/* Includes
 * - Library */
#include <stddef.h>
#include <string.h>

/* Internal allocation flags, applied automatically based on size. */
#define ALLOCATION_PAGEALIGN        0x10000000
#define ALLOCATION_BIG              0x20000000

#define ALLOCISNORMAL(x)            (((x) & (ALLOCATION_PAGEALIGN | ALLOCATION_BIG)) == 0)
#define ALLOCISNOTBIG(x)            (((x) & ALLOCATION_BIG) == 0)
#define ALLOCISPAGE(x)              ((((x) & ALLOCATION_PAGEALIGN) != 0) && ALLOCISNOTBIG(x))

/* defs.h defines ALIGN/ISALIGNED without parenthesising their arguments,
 * so ALIGN(a + b, ...) expands wrong. Local, safe versions. Alignment
 * must be a power of two. */
#define HEAP_ALIGN_UP(v, a)         (((uintptr_t)(v) + ((uintptr_t)(a) - 1)) & ~((uintptr_t)(a) - 1))
#define HEAP_ISALIGNED(v, a)        ((((uintptr_t)(v)) & ((uintptr_t)(a) - 1)) == 0)

/* There is no assert() in this kernel. Anything that would have been an
 * assert in the reference either returns an error or lands here. */
#define HEAP_FATAL(msg)                                                 \
    do {                                                                \
        LogFatal("HEAP", "%s (%s:%d)", msg, __FILE__, __LINE__);        \
        for (;;) { }                                                    \
    } while (0)

/* Globals */
static const char *GlbKernelUnknown = "Unknown";
static Heap_t GlbKernelHeap;
static int GlbHeapInitialized = 0;

/* HeapLock / HeapUnlock
 * Placeholders. There is no threading and interrupts are masked, so
 * there is nothing to race against yet. When you add scheduling, this is
 * the only place that needs to change - either a spinlock or a
 * cli/sti pair saving EFLAGS (___getflags / ___cli / ___sti are already
 * exported from irq.asm). */
static void HeapLock(Heap_t *Heap)   { (void)Heap; }
static void HeapUnlock(Heap_t *Heap) { (void)Heap; }

/* HeapSetIdentifier
 * There is no strnlen in this libc, so copy at most HEAP_IDENT_SIZE-1
 * bytes by hand and always terminate. */
static void HeapSetIdentifier(char *Destination, const char *Identifier)
{
    const char *Source = (Identifier == NULL) ? GlbKernelUnknown : Identifier;
    int i;

    for (i = 0; i < (HEAP_IDENT_SIZE - 1) && Source[i] != '\0'; i++) {
        Destination[i] = Source[i];
    }
    for (; i < HEAP_IDENT_SIZE; i++) {
        Destination[i] = '\0';
    }
}

/* HeapMapPage
 * Grabs a physical frame and maps it at <Address> in the current kernel
 * directory. This replaces the reference's AddressSpaceMap, which does
 * not exist here. Returns Success or Error. */
static OsStatus_t HeapMapPage(uintptr_t Address, uintptr_t Mask)
{
    PhysicalAddress_t Physical;

    /* Already mapped? Nothing to do. */
    if (MmVirtualGetMapping(NULL, Address) != 0) {
        return Success;
    }

    Physical = MmPhysicalAllocateBlock(Mask, 1);
    if (Physical == 0) {
        LogFatal("HEAP", "no physical memory for heap page 0x%x", Address);
        return Error;
    }

    return MmVirtualMap(NULL, Physical, Address & PAGE_MASK, 0);
}

/* HeapSAllocator
 * Allocates block/node headers out of the reserved header region at the
 * bottom of the heap, mapping pages in as it goes. */
static uintptr_t *HeapSAllocator(Heap_t *Heap, size_t Size)
{
    uintptr_t *ReturnAddress = NULL;

    if (Size == 0) {
        return NULL;
    }

    if ((Heap->MemHeaderCurrent + Size) >= Heap->MemHeaderMax) {
        if ((Heap->MemHeaderMax + PAGE_SIZE) >=
            (Heap->HeapBase + MEMORY_STATIC_OFFSET)) {
            LogFatal("HEAP", "out of header space, Max 0x%x Current 0x%x",
                Heap->MemHeaderMax, Heap->MemHeaderCurrent);
            HeapPrintStats(Heap);
            return NULL;
        }

        if (HeapMapPage(Heap->MemHeaderMax, __MASK) != Success) {
            return NULL;
        }
        memset((void*)Heap->MemHeaderMax, 0, PAGE_SIZE);
        Heap->MemHeaderMax += PAGE_SIZE;
        Heap->NumPages++;
    }

    ReturnAddress = (uintptr_t*)Heap->MemHeaderCurrent;
    Heap->MemHeaderCurrent += Size;
    return ReturnAddress;
}

/* HeapAppendBlockToList
 * Appends a block to the correct list for its type. */
static void HeapAppendBlockToList(Heap_t *Heap, HeapBlock_t *Block)
{
    HeapBlock_t *CurrBlock = NULL;

    if (Block == NULL) {
        return;
    }

    if (Block->Flags & BLOCK_VERY_LARGE) {
        if (Heap->CustomBlocks == NULL) {
            Heap->CustomBlocks = Block;
            Block->Link = NULL;
            return;
        }
        CurrBlock = Heap->CustomBlocks;
    }
    else if (Block->Flags & BLOCK_ALIGNED) {
        if (Heap->PageBlocks == NULL) {
            Heap->PageBlocks = Block;
            Block->Link = NULL;
            return;
        }
        CurrBlock = Heap->PageBlocks;
    }
    else {
        if (Heap->Blocks == NULL) {
            Heap->Blocks = Block;
            Block->Link = NULL;
            return;
        }
        CurrBlock = Heap->Blocks;
    }

    while (CurrBlock->Link != NULL) {
        CurrBlock = CurrBlock->Link;
    }

    CurrBlock->Link = Block;
    Block->Link = NULL;
}

/* HeapStatsCounter
 * Walks one block list and accumulates counts. */
static void HeapStatsCounter(HeapBlock_t *Block, size_t *BlockCounter,
    size_t *NodeCounter, size_t *NodesAllocated, size_t *BytesAllocated)
{
    while (Block != NULL) {
        HeapNode_t *CurrentNode = Block->Nodes;

        while (CurrentNode != NULL) {
            if (CurrentNode->Flags & NODE_ALLOCATED) {
                (*NodesAllocated) += 1;
                (*BytesAllocated) += CurrentNode->Length;
            }
            (*NodeCounter) += 1;
            CurrentNode = CurrentNode->Link;
        }

        (*BlockCounter) += 1;
        Block = Block->Link;
    }
}

/* HeapPrintStats */
void HeapPrintStats(Heap_t *Heap)
{
    Heap_t *pHeap = (Heap == NULL) ? &GlbKernelHeap : Heap;

    size_t BlockNorm = 0, BlockPage = 0, BlockBig = 0;
    size_t NodeNorm  = 0, NodePage  = 0, NodeBig  = 0;
    size_t AllocNorm = 0, AllocPage = 0, AllocBig = 0;
    size_t BytesNorm = 0, BytesPage = 0, BytesBig = 0;

    HeapStatsCounter(pHeap->Blocks,       &BlockNorm, &NodeNorm, &AllocNorm, &BytesNorm);
    HeapStatsCounter(pHeap->PageBlocks,   &BlockPage, &NodePage, &AllocPage, &BytesPage);
    HeapStatsCounter(pHeap->CustomBlocks, &BlockBig,  &NodeBig,  &AllocBig,  &BytesBig);

    LogDebug("HEAP", "headers at 0x%x, next data at 0x%x",
        pHeap->MemHeaderCurrent, pHeap->MemStartData);
    LogDebug("HEAP", "blocks: %u normal, %u page, %u big",
        BlockNorm, BlockPage, BlockBig);
    LogDebug("HEAP", "nodes : %u normal, %u page, %u big",
        NodeNorm, NodePage, NodeBig);
    LogDebug("HEAP", "in use: %u nodes, %u bytes",
        AllocNorm + AllocPage + AllocBig, BytesNorm + BytesPage + BytesBig);
}

/* HeapCreateBlock
 * Allocates a block (and its first node) from the recycler or the header
 * region. Returns NULL if the heap is exhausted. */
static HeapBlock_t *HeapCreateBlock(Heap_t *Heap, size_t Size,
    uintptr_t Mask, Flags_t Flags, const char *Identifier)
{
    HeapBlock_t *hBlock = NULL;
    HeapNode_t  *hNode  = NULL;

    /* The reference asserts here. Returning NULL lets the caller report
     * a clean out-of-memory instead of halting the machine. */
    if ((Heap->MemStartData + Size) >= Heap->HeapEnd) {
        LogFatal("HEAP", "expansion of 0x%x would pass HeapEnd 0x%x",
            Size, Heap->HeapEnd);
        return NULL;
    }

    if (Heap->BlockRecycler != NULL) {
        hBlock = Heap->BlockRecycler;
        Heap->BlockRecycler = Heap->BlockRecycler->Link;
    }
    else {
        hBlock = (HeapBlock_t*)HeapSAllocator(Heap, sizeof(HeapBlock_t));
    }
    if (hBlock == NULL) {
        return NULL;
    }

    if (Heap->NodeRecycler != NULL) {
        hNode = Heap->NodeRecycler;
        Heap->NodeRecycler = Heap->NodeRecycler->Link;
    }
    else {
        hNode = (HeapNode_t*)HeapSAllocator(Heap, sizeof(HeapNode_t));
    }
    if (hNode == NULL) {
        return NULL;
    }

    hBlock->AddressStart = Heap->MemStartData;
    hBlock->AddressEnd   = (Heap->MemStartData + Size - 1);
    hBlock->BytesFree    = Size;
    hBlock->Flags        = Flags;
    hBlock->Link         = NULL;
    hBlock->Nodes        = hNode;
    hBlock->Mask         = Mask;

    HeapSetIdentifier(&hNode->Identifier[0], Identifier);
    hNode->Address = Heap->MemStartData;
    hNode->Link    = NULL;
    hNode->Flags   = 0;
    hNode->Length  = Size;

    Heap->MemStartData += Size;
    return hBlock;
}

/* HeapExpand
 * Adds a new block sized for the kind of allocation being made. */
static OsStatus_t HeapExpand(Heap_t *Heap, size_t Size, uintptr_t Mask,
    Flags_t Flags, const char *Identifier)
{
    HeapBlock_t *hBlock;

    if (Flags & ALLOCATION_BIG) {
        hBlock = HeapCreateBlock(Heap, Size, Mask, BLOCK_VERY_LARGE, Identifier);
    }
    else if (Flags & ALLOCATION_PAGEALIGN) {
        hBlock = HeapCreateBlock(Heap, HEAP_LARGE_BLOCK, Mask, BLOCK_ALIGNED, Identifier);
    }
    else {
        hBlock = HeapCreateBlock(Heap, HEAP_NORMAL_BLOCK, Mask, BLOCK_NORMAL, Identifier);
    }

    if (hBlock == NULL) {
        return Error;
    }

    HeapAppendBlockToList(Heap, hBlock);
    return Success;
}

/**************************************/
/********** Heap Allocation ***********/
/**************************************/

/* HeapAllocateSizeInBlock
 * Sub-allocates <Size> inside <Block>. Returns 0 if it does not fit. */
static uintptr_t HeapAllocateSizeInBlock(Heap_t *Heap, HeapBlock_t *Block,
    size_t Size, size_t Alignment, const char *Identifier)
{
    HeapNode_t *CurrNode = Block->Nodes;
    HeapNode_t *PrevNode = NULL;
    uintptr_t   RetAddr  = 0;

    while (CurrNode != NULL) {

        if ((CurrNode->Flags & NODE_ALLOCATED) || CurrNode->Length < Size) {
            goto Skip;
        }

        if (CurrNode->Length == Size || (Block->Flags & BLOCK_VERY_LARGE)) {
            /* Exact fit, or a dedicated oversized block - take the whole
             * node as-is. */
            HeapSetIdentifier(&CurrNode->Identifier[0], Identifier);
            CurrNode->Flags = NODE_ALLOCATED;

            RetAddr = CurrNode->Address;
            Block->BytesFree -= Size;
            break;
        }
        else {
            /* Split: a new allocated node in front of the remaining gap. */
            HeapNode_t *hNode = NULL;
            size_t      TakeSize = Size;

            if (Alignment != 0 && !HEAP_ISALIGNED(CurrNode->Address, Alignment)) {
                size_t Padding = Alignment - (CurrNode->Address % Alignment);
                if (CurrNode->Length < (Size + Padding)) {
                    goto Skip;
                }
                TakeSize = Size + Padding;
            }

            if (Heap->NodeRecycler != NULL) {
                hNode = Heap->NodeRecycler;
                Heap->NodeRecycler = Heap->NodeRecycler->Link;
            }
            else {
                hNode = (HeapNode_t*)HeapSAllocator(Heap, sizeof(HeapNode_t));
            }
            if (hNode == NULL) {
                return 0;
            }

            /* The reference writes the identifier onto CurrNode here,
             * which is the node that stays *free*. It belongs on hNode. */
            HeapSetIdentifier(&hNode->Identifier[0], Identifier);

            hNode->Address = CurrNode->Address;
            hNode->Flags   = NODE_ALLOCATED;
            hNode->Link    = CurrNode;
            hNode->Length  = TakeSize;

            if (Alignment != 0) {
                RetAddr = HEAP_ALIGN_UP(hNode->Address, Alignment);
            }
            else {
                RetAddr = hNode->Address;
            }

            CurrNode->Address = hNode->Address + TakeSize;
            CurrNode->Length -= TakeSize;
            Block->BytesFree -= TakeSize;

            if (PrevNode != NULL) {
                PrevNode->Link = hNode;
            }
            else {
                Block->Nodes = hNode;
            }
            break;
        }

    Skip:
        PrevNode = CurrNode;
        CurrNode = CurrNode->Link;
    }

    return RetAddr;
}

/* HeapCommitPages
 * Maps the pages behind a fresh allocation. */
static OsStatus_t HeapCommitPages(uintptr_t Address, size_t Size, uintptr_t Mask)
{
    uintptr_t Start = Address & PAGE_MASK;
    uintptr_t End   = HEAP_ALIGN_UP(Address + Size, PAGE_SIZE);
    uintptr_t i;

    for (i = Start; i < End; i += PAGE_SIZE) {
        if (HeapMapPage(i, Mask) != Success) {
            return Error;
        }
    }
    return Success;
}

/* HeapAllocate */
uintptr_t HeapAllocate(Heap_t *Heap, size_t Size, Flags_t Flags,
    size_t Alignment, uintptr_t Mask, const char *Identifier)
{
    HeapBlock_t *CurrentBlock = NULL;
    size_t       AdjustedSize = Size;
    uintptr_t    RetVal       = 0;
    int          Retried      = 0;

    if (Heap == NULL || Size == 0) {
        return 0;
    }

    /* Pick a block class from the size. */
    if (ALLOCISNORMAL(Flags) && Size >= HEAP_NORMAL_BLOCK) {
        AdjustedSize = HEAP_ALIGN_UP(Size, PAGE_SIZE);
        Flags |= ALLOCATION_PAGEALIGN;
    }
    if (ALLOCISNOTBIG(Flags) && AdjustedSize >= HEAP_LARGE_BLOCK) {
        Flags |= ALLOCATION_BIG;
    }
    if (ALLOCISPAGE(Flags) && !HEAP_ISALIGNED(AdjustedSize, PAGE_SIZE)) {
        AdjustedSize = HEAP_ALIGN_UP(AdjustedSize, PAGE_SIZE);
    }

    HeapLock(Heap);

    /* The reference recurses here after expanding, which can run away if
     * the expansion cannot satisfy the request. One retry is enough:
     * HeapExpand always creates a block large enough for AdjustedSize. */
    for (;;) {
        if (Flags & ALLOCATION_BIG) {
            CurrentBlock = Heap->CustomBlocks;
        }
        else if (Flags & ALLOCATION_PAGEALIGN) {
            CurrentBlock = Heap->PageBlocks;
        }
        else {
            CurrentBlock = Heap->Blocks;
        }

        while (CurrentBlock != NULL) {
            if (CurrentBlock->BytesFree >= AdjustedSize
                && CurrentBlock->Mask >= Mask) {
                RetVal = HeapAllocateSizeInBlock(Heap, CurrentBlock,
                    AdjustedSize, Alignment, Identifier);
                if (RetVal != 0) {
                    break;
                }
            }
            CurrentBlock = CurrentBlock->Link;
        }

        if (RetVal != 0 || Retried) {
            break;
        }

        if (HeapExpand(Heap, AdjustedSize, Mask, Flags, Identifier) != Success) {
            break;
        }
        Retried = 1;
    }

    if (RetVal == 0) {
        HeapUnlock(Heap);
        LogFatal("HEAP", "allocation of %u bytes failed", AdjustedSize);
        return 0;
    }

    if (Flags & ALLOCATION_COMMIT) {
        if (HeapCommitPages(RetVal, AdjustedSize, Mask) != Success) {
            HeapUnlock(Heap);
            return 0;
        }
    }

    Heap->BytesAllocated += AdjustedSize;
    Heap->NumAllocs++;

    HeapUnlock(Heap);
    return RetVal;
}

/**************************************/
/*********** Heap Freeing *************/
/**************************************/

/* HeapRecycleNode */
static void HeapRecycleNode(Heap_t *Heap, HeapNode_t *Node)
{
    Node->Address = 0;
    Node->Length  = 0;
    Node->Flags   = 0;
    Node->Link    = Heap->NodeRecycler;
    Heap->NodeRecycler = Node;
}

/* HeapFreeAddressInNode
 * Frees the node covering <Address> and merges with free neighbours. */
static void HeapFreeAddressInNode(Heap_t *Heap, HeapBlock_t *Block,
    uintptr_t Address)
{
    HeapNode_t *CurrNode = Block->Nodes;
    HeapNode_t *PrevNode = NULL;

    while (CurrNode != NULL) {
        uintptr_t aStart = CurrNode->Address;
        uintptr_t aEnd   = CurrNode->Address + CurrNode->Length;

        /* Half-open range. The reference used `Address < aEnd` with aEnd
         * computed as Length-1, which misses the last byte of a node and
         * fails outright for a one-byte node. */
        if (Address >= aStart && Address < aEnd) {

            if (!(CurrNode->Flags & NODE_ALLOCATED)) {
                LogFatal("HEAP", "double free of 0x%x", Address);
                return;
            }

            CurrNode->Flags = 0;
            Block->BytesFree += CurrNode->Length;
            Heap->NumFrees++;
            if (Heap->BytesAllocated >= CurrNode->Length) {
                Heap->BytesAllocated -= CurrNode->Length;
            }

            /* Merge forward. The reference tested CurrNode's own flags
             * here, which it had just cleared - so the condition was
             * always true and it would happily merge a freed node into
             * an ALLOCATED successor and corrupt the block. Test the
             * successor. */
            if (CurrNode->Link != NULL
                && !(CurrNode->Link->Flags & NODE_ALLOCATED)) {
                HeapNode_t *Next = CurrNode->Link;
                CurrNode->Length += Next->Length;
                CurrNode->Link = Next->Link;
                HeapRecycleNode(Heap, Next);
            }

            /* Merge backward. */
            if (PrevNode != NULL && !(PrevNode->Flags & NODE_ALLOCATED)) {
                PrevNode->Length += CurrNode->Length;
                PrevNode->Link = CurrNode->Link;
                HeapRecycleNode(Heap, CurrNode);
            }

            return;
        }

        PrevNode = CurrNode;
        CurrNode = CurrNode->Link;
    }

    LogFatal("HEAP", "free of unknown address 0x%x", Address);
}

/* HeapFreeLocator */
static HeapBlock_t *HeapFreeLocator(HeapBlock_t *List, uintptr_t Address)
{
    HeapBlock_t *Block = List;

    while (Block != NULL) {
        if (Block->AddressStart <= Address && Block->AddressEnd >= Address) {
            return Block;
        }
        Block = Block->Link;
    }
    return NULL;
}

/* HeapFree */
void HeapFree(Heap_t *Heap, uintptr_t Address)
{
    HeapBlock_t *Block = NULL;

    if (Heap == NULL || Address == 0) {
        return;
    }

    HeapLock(Heap);

    Block = HeapFreeLocator(Heap->Blocks, Address);
    if (Block == NULL) {
        Block = HeapFreeLocator(Heap->PageBlocks, Address);
    }
    if (Block == NULL) {
        Block = HeapFreeLocator(Heap->CustomBlocks, Address);
    }

    if (Block == NULL) {
        HeapUnlock(Heap);
        LogFatal("HEAP", "free of 0x%x which is not in any block", Address);
        return;
    }

    HeapFreeAddressInNode(Heap, Block, Address);
    HeapUnlock(Heap);
}

/**************************************/
/*********** Heap Querying ************/
/**************************************/

static HeapNode_t *HeapQueryAddressInNode(HeapBlock_t *Block, uintptr_t Address)
{
    HeapNode_t *CurrNode = Block->Nodes;

    while (CurrNode != NULL) {
        uintptr_t aStart = CurrNode->Address;
        uintptr_t aEnd   = CurrNode->Address + CurrNode->Length;

        if (Address >= aStart && Address < aEnd) {
            return CurrNode;
        }
        CurrNode = CurrNode->Link;
    }
    return NULL;
}

static HeapNode_t *HeapQuery(Heap_t *Heap, uintptr_t Address)
{
    HeapBlock_t *Block = HeapFreeLocator(Heap->Blocks, Address);

    if (Block == NULL) {
        Block = HeapFreeLocator(Heap->PageBlocks, Address);
    }
    if (Block == NULL) {
        Block = HeapFreeLocator(Heap->CustomBlocks, Address);
    }
    if (Block == NULL) {
        return NULL;
    }
    return HeapQueryAddressInNode(Block, Address);
}

/* HeapQueryMemoryInformation */
int HeapQueryMemoryInformation(Heap_t *Heap, size_t *BytesInUse,
    size_t *BlocksAllocated)
{
    Heap_t *pHeap = (Heap == NULL) ? &GlbKernelHeap : Heap;
    size_t  Blocks = 0, Nodes = 0, Allocated = 0, Bytes = 0;

    if (!GlbHeapInitialized && Heap == NULL) {
        return -1;
    }

    HeapStatsCounter(pHeap->Blocks,       &Blocks, &Nodes, &Allocated, &Bytes);
    HeapStatsCounter(pHeap->PageBlocks,   &Blocks, &Nodes, &Allocated, &Bytes);
    HeapStatsCounter(pHeap->CustomBlocks, &Blocks, &Nodes, &Allocated, &Bytes);

    if (BytesInUse != NULL) {
        *BytesInUse = Bytes;
    }
    if (BlocksAllocated != NULL) {
        *BlocksAllocated = Allocated;
    }
    return 0;
}

/* HeapValidateAddress */
int HeapValidateAddress(Heap_t *Heap, uintptr_t Address)
{
    Heap_t     *pHeap = (Heap == NULL) ? &GlbKernelHeap : Heap;
    HeapNode_t *Node  = HeapQuery(pHeap, Address);

    if (Node == NULL || !(Node->Flags & NODE_ALLOCATED)) {
        return -1;
    }
    return 0;
}

/**************************************/
/******** Heap Initialization *********/
/**************************************/

/* HeapSetup
 * Shared setup for the kernel heap and any secondary heap. */
static OsStatus_t HeapSetup(Heap_t *Heap, uintptr_t Base, uintptr_t End, int IsUser)
{
    Heap->IsUser           = IsUser;
    Heap->HeapBase         = Base;
    Heap->MemStartData     = Base + MEMORY_STATIC_OFFSET;
    Heap->MemHeaderCurrent = Base;
    Heap->MemHeaderMax     = Base;
    Heap->HeapEnd          = End;
    Heap->Lock             = 0;

    Heap->BlockRecycler    = NULL;
    Heap->NodeRecycler     = NULL;
    Heap->Blocks           = NULL;
    Heap->PageBlocks       = NULL;
    Heap->CustomBlocks     = NULL;

    Heap->BytesAllocated   = 0;
    Heap->NumAllocs        = 0;
    Heap->NumFrees         = 0;
    Heap->NumPages         = 0;

    Heap->Blocks = HeapCreateBlock(Heap, HEAP_NORMAL_BLOCK,
        __MASK, BLOCK_NORMAL, GlbKernelUnknown);
    if (Heap->Blocks == NULL) {
        return Error;
    }

    Heap->PageBlocks = HeapCreateBlock(Heap, HEAP_LARGE_BLOCK,
        __MASK, BLOCK_ALIGNED, GlbKernelUnknown);
    if (Heap->PageBlocks == NULL) {
        return Error;
    }

    return Success;
}

/* HeapInit */
OsStatus_t HeapInit(void)
{
    LogInformation("HEAP", "Initializing, 0x%x - 0x%x",
        MEMORY_LOCATION_HEAP, MEMORY_LOCATION_HEAP_END);

    if (HeapSetup(&GlbKernelHeap, MEMORY_LOCATION_HEAP,
            MEMORY_LOCATION_HEAP_END, 0) != Success) {
        LogFatal("HEAP", "failed to initialize the kernel heap");
        return Error;
    }

    GlbHeapInitialized = 1;
    LogInformation("HEAP", "ready, data starts at 0x%x",
        GlbKernelHeap.MemStartData);
    return Success;
}

/* HeapCreate */
Heap_t *HeapCreate(uintptr_t HeapAddress, uintptr_t HeapEnd, int UserHeap)
{
    Heap_t *Heap = (Heap_t*)kmalloc(sizeof(Heap_t));

    if (Heap == NULL) {
        return NULL;
    }

    if (HeapSetup(Heap, HeapAddress, HeapEnd, UserHeap) != Success) {
        kfree(Heap);
        return NULL;
    }
    return Heap;
}

/**************************************/
/*********** kmalloc family ***********/
/**************************************/

void *kmalloc_apm(size_t Size, uintptr_t *Ptr, uintptr_t Mask)
{
    uintptr_t RetAddr;

    if (Size == 0 || !GlbHeapInitialized) {
        return NULL;
    }

    RetAddr = HeapAllocate(&GlbKernelHeap, Size,
        ALLOCATION_COMMIT | ALLOCATION_PAGEALIGN, 0, Mask, GlbKernelUnknown);
    if (RetAddr == 0) {
        return NULL;
    }
    if (!HEAP_ISALIGNED(RetAddr, PAGE_SIZE)) {
        HEAP_FATAL("page-aligned allocation came back unaligned");
    }

    if (Ptr != NULL) {
        *Ptr = MmVirtualGetMapping(NULL, RetAddr);
    }
    return (void*)RetAddr;
}

void *kmalloc_ap(size_t Size, uintptr_t *Ptr)
{
    return kmalloc_apm(Size, Ptr, __MASK);
}

void *kmalloc_p(size_t Size, uintptr_t *Ptr)
{
    uintptr_t RetAddr;

    if (Size == 0 || !GlbHeapInitialized) {
        return NULL;
    }

    RetAddr = HeapAllocate(&GlbKernelHeap, Size, ALLOCATION_COMMIT,
        HEAP_STANDARD_ALIGN, __MASK, NULL);
    if (RetAddr == 0) {
        return NULL;
    }

    if (Ptr != NULL) {
        *Ptr = MmVirtualGetMapping(NULL, RetAddr);
    }
    return (void*)RetAddr;
}

void *kmalloc_a(size_t Size)
{
    return kmalloc_apm(Size, NULL, __MASK);
}

void *kmalloc(size_t Size)
{
    uintptr_t RetAddr;

    if (Size == 0 || !GlbHeapInitialized) {
        return NULL;
    }

    RetAddr = HeapAllocate(&GlbKernelHeap, Size, ALLOCATION_COMMIT,
        HEAP_STANDARD_ALIGN, __MASK, NULL);
    if (RetAddr == 0) {
        return NULL;
    }
    return (void*)RetAddr;
}

void kfree(void *p)
{
    if (p == NULL) {
        return;
    }
    HeapFree(&GlbKernelHeap, (uintptr_t)p);
}

/**************************************/
/************ Heap Testing ************/
/**************************************/

void HeapTest(void)
{
    uintptr_t phys1 = 0, phys2 = 0;
    void *r1, *r2, *r3, *r4, *r5, *r6;
    int i;

    LogInformation("HEAP", "--- test: small allocations ---");
    HeapPrintStats(NULL);

    r1 = kmalloc(0x30);
    r2 = kmalloc(0x50);
    r3 = kmalloc(0x130);
    r4 = kmalloc(0x180);
    r5 = kmalloc(0x600);
    r6 = kmalloc(0x3000);
    LogInformation("HEAP", "0x30=0x%x 0x50=0x%x 0x130=0x%x",
        (uintptr_t)r1, (uintptr_t)r2, (uintptr_t)r3);
    LogInformation("HEAP", "0x180=0x%x 0x600=0x%x 0x3000=0x%x",
        (uintptr_t)r4, (uintptr_t)r5, (uintptr_t)r6);

    /* Write to every allocation - this is what actually proves the pages
     * were committed. If HeapCommitPages is wrong you fault here. */
    if (r1) memset(r1, 0xAA, 0x30);
    if (r5) memset(r5, 0xBB, 0x600);
    if (r6) memset(r6, 0xCC, 0x3000);

    kfree(r5);
    kfree(r2);
    kfree(r3);
    HeapPrintStats(NULL);

    r2 = kmalloc(0x90);
    r3 = kmalloc(0x20);
    r5 = kmalloc(0x320);
    LogInformation("HEAP", "realloc 0x90=0x%x 0x20=0x%x 0x320=0x%x",
        (uintptr_t)r2, (uintptr_t)r3, (uintptr_t)r5);

    kfree(r1); kfree(r2); kfree(r3);
    kfree(r4); kfree(r5); kfree(r6);
    HeapPrintStats(NULL);

    LogInformation("HEAP", "--- test: aligned + physical ---");
    r1 = kmalloc_a(0x30);
    r2 = kmalloc_a(0x210);
    r3 = kmalloc_a(0x900);
    r4 = kmalloc_a(0x4500);
    r5 = kmalloc_ap(0x32, &phys1);
    r6 = kmalloc_ap(0x1000, &phys2);
    LogInformation("HEAP", "aligned 0x%x 0x%x 0x%x 0x%x",
        (uintptr_t)r1, (uintptr_t)r2, (uintptr_t)r3, (uintptr_t)r4);
    LogInformation("HEAP", "phys 0x%x -> 0x%x, 0x%x -> 0x%x",
        (uintptr_t)r5, phys1, (uintptr_t)r6, phys2);

    kfree(r1); kfree(r2); kfree(r3);
    kfree(r4); kfree(r5); kfree(r6);
    HeapPrintStats(NULL);

    LogInformation("HEAP", "--- test: 150 alloc/free cycles ---");
    for (i = 0; i < 50; i++) {
        void *a = kmalloc(0x30);
        void *b = kmalloc(0x130);
        void *c = kmalloc(0x600);
        kfree(a);
        kfree(b);
        kfree(c);
    }

    HeapPrintStats(NULL);
    LogInformation("HEAP", "--- tests done ---");
}