/* Includes
 * - System */
#include <system/shm.h>
#include <system/process.h>
#include <system/log.h>
#include <interrupts/interrupts.h>
#include <arch/x86/memory.h>
#include <arch/x86/x32/arch_x32.h>

/* Includes
 * - Library */
#include <stddef.h>
#include <string.h>

static SharedMemory_t GlbShm[SHM_MAX];
static UUId_t GlbShmIds = 1;

/* ShmInitialize */
void ShmInitialize(void)
{
    memset(GlbShm, 0, sizeof(GlbShm));
    GlbShmIds = 1;
    LogInformation("Shm", "Ready, %d regions, %d pages each at most",
        SHM_MAX, SHM_MAX_PAGES);
}

/* ShmCreate
 * Allocates the frames. They are not mapped anywhere yet - a region
 * exists independently of who can see it, which is what lets it be
 * granted to a second process later. */
SharedMemory_t *ShmCreate(void *Process, size_t Length)
{
    SharedMemory_t *Region = NULL;
    size_t Pages, i;
    int State, Slot = -1;

    if (Length == 0) {
        return NULL;
    }

    Pages = DIVUP(Length, PAGE_SIZE);
    if (Pages > SHM_MAX_PAGES) {
        LogFatal("Shm", "%u pages requested, limit is %d",
            Pages, SHM_MAX_PAGES);
        return NULL;
    }

    State = InterruptDisable();
    for (i = 0; i < SHM_MAX; i++) {
        if (GlbShm[i].Used == 0) {
            Slot = (int)i;
            GlbShm[i].Used = 1;     /* claim it before releasing */
            break;
        }
    }
    InterruptRestoreState(State);

    if (Slot < 0) {
        LogFatal("Shm", "no free regions");
        return NULL;
    }

    Region = &GlbShm[Slot];
    Region->Id         = GlbShmIds++;
    Region->Owner      = Process;
    Region->PageCount  = Pages;
    Region->Length     = Length;
    Region->References = 1;

    /* One frame at a time. Contiguity is not required - each page is
     * mapped individually into every sharer, so the region is
     * contiguous in virtual space regardless. A DMA-capable device
     * would need physical contiguity, and that is a separate allocator. */
    for (i = 0; i < Pages; i++) {
        Region->Frames[i] = MmPhysicalAllocateBlock(__MASK, 1);
        if (Region->Frames[i] == 0) {
            size_t j;
            for (j = 0; j < i; j++) {
                MmPhysicalFreeBlock(Region->Frames[j]);
            }
            Region->Used = 0;
            LogFatal("Shm", "out of physical memory");
            return NULL;
        }
    }

    LogInformation("Shm", "region %u created, %u pages", Region->Id, Pages);
    return Region;
}

/* ShmMap
 * Maps the region into <Process> and returns the address. Each process
 * gets its own virtual address for the same frames - they are not
 * required to agree, which is why a shared region must never contain
 * pointers, only offsets. */
uintptr_t ShmMap(SharedMemory_t *Region, void *ProcessPtr)
{
    Process_t *Process = (Process_t*)ProcessPtr;
    uintptr_t Base;
    size_t i;

    if (Region == NULL || Region->Used == 0 || Process == NULL
        || Process->AddressSpace == NULL) {
        return 0;
    }

    /* Carved from the same region device apertures use - both are
     * "memory that is not this process's own heap". */
    Base = Process->NextIoSpace;
    if ((Base + (Region->PageCount * PAGE_SIZE))
        >= MEMORY_LOCATION_RING3_IOSPACE_END) {
        LogFatal("Shm", "'%s' has no room to map a region", Process->Name);
        return 0;
    }

    for (i = 0; i < Region->PageCount; i++) {
        if (MmVirtualMap(Process->AddressSpace->PageDirectory,
                Region->Frames[i], Base + (i * PAGE_SIZE),
                PAGE_USER | PAGE_WRITE) != Success) {
            LogFatal("Shm", "could not map region %u into '%s'",
                Region->Id, Process->Name);
            return 0;
        }
    }

    Process->NextIoSpace = Base + (Region->PageCount * PAGE_SIZE);

    LogInformation("Shm", "region %u mapped into '%s' at 0x%x",
        Region->Id, Process->Name, Base);
    return Base;
}

/* ShmRelease */
void ShmRelease(SharedMemory_t *Region)
{
    size_t i;
    int State, Last = 0;

    if (Region == NULL || Region->Used == 0) {
        return;
    }

    State = InterruptDisable();
    Region->References--;
    if (Region->References <= 0) {
        Region->Used = 0;
        Last = 1;
    }
    InterruptRestoreState(State);

    if (!Last) {
        return;
    }

    /* The mappings themselves go away with each sharer's address space.
     * Only the frames are freed here, and only once the last reference
     * is gone - freeing them while another process still has them
     * mapped would hand live memory to the allocator. */
    for (i = 0; i < Region->PageCount; i++) {
        MmPhysicalFreeBlock(Region->Frames[i]);
    }

    LogInformation("Shm", "region %u released", Region->Id);
}

/* ShmPrint */
void ShmPrint(void)
{
    int i;

    for (i = 0; i < SHM_MAX; i++) {
        if (GlbShm[i].Used == 0) {
            continue;
        }
        LogInformation("Shm", "  region %u  %u bytes  %u pages  %d refs",
            GlbShm[i].Id, GlbShm[i].Length, GlbShm[i].PageCount,
            GlbShm[i].References);
    }
}
