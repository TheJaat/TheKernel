/* Includes
 * - System */
#include <system/iospace.h>
#include <system/log.h>
#include <interrupts/interrupts.h>
#include <arch/x86/memory.h>
#include <arch/x86/x32/arch_x32.h>

/* Includes
 * - Library */
#include <stddef.h>
#include <string.h>

/* The system's copy of every registered region. A driver's own
 * DeviceIoSpace_t is just a handle; this table is the truth. */
typedef struct _SystemIoSpace {
    DeviceIoSpace_t Io;
    int             Used;
    int             Acquired;
    int             Mapped;
} SystemIoSpace_t;

static SystemIoSpace_t GlbIoSpaces[IOSPACE_MAX];
static UUId_t GlbIoSpaceId = 1;
static int GlbIoSpaceInitialized = 0;

/* IoSpaceInitialize */
void IoSpaceInitialize(void)
{
    LogInformation("IoSpace", "Initializing");

    memset((void*)GlbIoSpaces, 0, sizeof(GlbIoSpaces));
    GlbIoSpaceId = 1;
    GlbIoSpaceInitialized = 1;

    LogInformation("IoSpace", "Ready, %d slots", IOSPACE_MAX);
}

/* IoSpaceFind
 * Looks up the system copy by id. */
static SystemIoSpace_t *IoSpaceFind(UUId_t Id)
{
    int i;

    for (i = 0; i < IOSPACE_MAX; i++) {
        if (GlbIoSpaces[i].Used != 0 && GlbIoSpaces[i].Io.Id == Id) {
            return &GlbIoSpaces[i];
        }
    }
    return NULL;
}

/* IoSpaceOverlaps
 * Two regions of the same type collide if their address ranges
 * intersect. Ranges are half-open. */
static SystemIoSpace_t *IoSpaceOverlaps(int Type, uintptr_t Base, size_t Size)
{
    uintptr_t End = Base + Size;
    int i;

    for (i = 0; i < IOSPACE_MAX; i++) {
        SystemIoSpace_t *Entry = &GlbIoSpaces[i];
        uintptr_t EntryEnd;

        if (Entry->Used == 0 || Entry->Io.Type != Type) {
            continue;
        }

        EntryEnd = Entry->Io.PhysicalBase + Entry->Io.Size;
        if (Base < EntryEnd && Entry->Io.PhysicalBase < End) {
            return Entry;
        }
    }
    return NULL;
}

/* IoSpaceRegister */
OsStatus_t IoSpaceRegister(DeviceIoSpace_t *IoSpace)
{
    SystemIoSpace_t *Entry = NULL;
    SystemIoSpace_t *Clash = NULL;
    int State;
    int i;

    if (GlbIoSpaceInitialized != 1) {
        LogFatal("IoSpace", "register before IoSpaceInitialize");
        return Error;
    }
    if (IoSpace == NULL || IoSpace->Size == 0) {
        LogFatal("IoSpace", "register with no region");
        return Error;
    }
    if (IoSpace->Type != IO_SPACE_IO && IoSpace->Type != IO_SPACE_MMIO) {
        LogFatal("IoSpace", "register with bad type %d", IoSpace->Type);
        return Error;
    }
    /* The x86 port space is 16-bit. A region running off the end is a
     * bug in the caller, not something to silently truncate. */
    if (IoSpace->Type == IO_SPACE_IO
        && (IoSpace->PhysicalBase + IoSpace->Size) > 0x10000) {
        LogFatal("IoSpace", "port region 0x%x+0x%x runs past 0xFFFF",
            IoSpace->PhysicalBase, IoSpace->Size);
        return Error;
    }

    State = InterruptDisable();

    Clash = IoSpaceOverlaps(IoSpace->Type, IoSpace->PhysicalBase, IoSpace->Size);
    if (Clash != NULL) {
        InterruptRestoreState(State);
        LogFatal("IoSpace", "0x%x+0x%x overlaps id %u (0x%x+0x%x)",
            IoSpace->PhysicalBase, IoSpace->Size, Clash->Io.Id,
            Clash->Io.PhysicalBase, Clash->Io.Size);
        return Error;
    }

    for (i = 0; i < IOSPACE_MAX; i++) {
        if (GlbIoSpaces[i].Used == 0) {
            Entry = &GlbIoSpaces[i];
            break;
        }
    }

    if (Entry == NULL) {
        InterruptRestoreState(State);
        LogFatal("IoSpace", "no free slots");
        return Error;
    }

    IoSpace->Id          = GlbIoSpaceId++;
    IoSpace->VirtualBase = 0;

    memset(Entry, 0, sizeof(SystemIoSpace_t));
    Entry->Io   = *IoSpace;
    Entry->Used = 1;

    InterruptRestoreState(State);

    LogInformation("IoSpace", "id %u registered, %s 0x%x + 0x%x",
        Entry->Io.Id,
        (Entry->Io.Type == IO_SPACE_IO) ? "port" : "mmio",
        Entry->Io.PhysicalBase, Entry->Io.Size);
    return Success;
}

/* IoSpaceMapMmio
 * Maps the physical range into kernel reserved space. Caller holds the
 * lock. */
static OsStatus_t IoSpaceMapMmio(SystemIoSpace_t *Entry)
{
    uintptr_t Physical = Entry->Io.PhysicalBase & PAGE_MASK;
    uintptr_t Offset   = Entry->Io.PhysicalBase & ATTRIBUTE_MASK;
    uintptr_t Virtual;
    int PageCount;
    int i;

    /* Round out to whole pages at both ends. A region starting at
     * 0xFED00010 and 0x100 long still needs the page containing it. */
    PageCount = (int)DIVUP((Offset + Entry->Io.Size), PAGE_SIZE);
    if (PageCount == 0) {
        PageCount = 1;
    }

    Virtual = (uintptr_t)MmReserveMemory(PageCount);
    if (Virtual == 0) {
        LogFatal("IoSpace", "no reserved virtual space for %d pages", PageCount);
        return Error;
    }

    for (i = 0; i < PageCount; i++) {
        /* Device registers must not be cached or the cpu will happily
         * hand back a stale status word forever. PAGE_CACHE_DISABLE is
         * not optional here. */
        if (MmVirtualMap(NULL, Physical + (i * PAGE_SIZE),
                Virtual + (i * PAGE_SIZE),
                PAGE_CACHE_DISABLE) != Success) {
            LogFatal("IoSpace", "failed mapping page %d of id %u",
                i, Entry->Io.Id);
            return Error;
        }
    }

    Entry->Io.VirtualBase = Virtual + Offset;
    Entry->Mapped = 1;
    return Success;
}

/* IoSpaceAcquire */
OsStatus_t IoSpaceAcquire(DeviceIoSpace_t *IoSpace)
{
    SystemIoSpace_t *Entry = NULL;
    int State;

    if (IoSpace == NULL) {
        return Error;
    }

    State = InterruptDisable();

    Entry = IoSpaceFind(IoSpace->Id);
    if (Entry == NULL) {
        InterruptRestoreState(State);
        LogFatal("IoSpace", "acquire of unknown id %u", IoSpace->Id);
        return Error;
    }
    if (Entry->Acquired != 0) {
        InterruptRestoreState(State);
        LogFatal("IoSpace", "id %u is already acquired", IoSpace->Id);
        return Error;
    }

    if (Entry->Io.Type == IO_SPACE_MMIO && Entry->Mapped == 0) {
        if (IoSpaceMapMmio(Entry) != Success) {
            InterruptRestoreState(State);
            return Error;
        }
    }

    /* IO_SPACE_IO needs nothing here. The reference opens the port range
     * in the thread's TSS io-permission bitmap so a ring-3 driver can
     * use in/out directly; everything here runs in ring 0, where the
     * bitmap is not consulted. This is where that goes when you have
     * user-space drivers. */

    Entry->Acquired = 1;

    /* Hand the caller's copy the mapping we just made. */
    IoSpace->VirtualBase = Entry->Io.VirtualBase;

    InterruptRestoreState(State);
    return Success;
}

/* IoSpaceRelease */
OsStatus_t IoSpaceRelease(DeviceIoSpace_t *IoSpace)
{
    SystemIoSpace_t *Entry = NULL;
    int State;

    if (IoSpace == NULL) {
        return Error;
    }

    State = InterruptDisable();

    Entry = IoSpaceFind(IoSpace->Id);
    if (Entry == NULL || Entry->Acquired == 0) {
        InterruptRestoreState(State);
        LogFatal("IoSpace", "release of id %u which is not acquired",
            IoSpace->Id);
        return Error;
    }

    Entry->Acquired = 0;

    /* Tear the MMIO mapping down now that MmVirtualUnmap exists.
     *
     * ReleaseFrame is 0 on purpose: these frames are device registers,
     * not RAM from the physical allocator. Handing 0xFED00000 to
     * MmPhysicalFreeBlock would mark a bit for memory that was never
     * ours and eventually hand a device aperture out as a page.
     *
     * The reserved *virtual* range is still not reclaimed -
     * MmReserveMemory only bumps a pointer - so a re-acquire allocates
     * fresh virtual space. Bounded, but noted. */
    if (Entry->Io.Type == IO_SPACE_MMIO && Entry->Mapped != 0) {
        uintptr_t Virtual = Entry->Io.VirtualBase & PAGE_MASK;
        uintptr_t Offset  = Entry->Io.PhysicalBase & ATTRIBUTE_MASK;
        int PageCount = (int)DIVUP((Offset + Entry->Io.Size), PAGE_SIZE);
        int i;

        if (PageCount == 0) {
            PageCount = 1;
        }
        for (i = 0; i < PageCount; i++) {
            MmVirtualUnmap(NULL, Virtual + (i * PAGE_SIZE), 0);
        }
        Entry->Mapped = 0;
        Entry->Io.VirtualBase = 0;
    }

    InterruptRestoreState(State);
    return Success;
}

/* IoSpaceDestroy */
OsStatus_t IoSpaceDestroy(UUId_t Id)
{
    SystemIoSpace_t *Entry = NULL;
    int State = InterruptDisable();

    Entry = IoSpaceFind(Id);
    if (Entry == NULL) {
        InterruptRestoreState(State);
        LogFatal("IoSpace", "destroy of unknown id %u", Id);
        return Error;
    }
    if (Entry->Acquired != 0) {
        InterruptRestoreState(State);
        LogFatal("IoSpace", "id %u is still acquired", Id);
        return Error;
    }

    memset(Entry, 0, sizeof(SystemIoSpace_t));

    InterruptRestoreState(State);
    return Success;
}

/* IoSpaceValidate */
uintptr_t IoSpaceValidate(uintptr_t Address)
{
    uintptr_t Result = 0;
    int State = InterruptDisable();
    int i;

    for (i = 0; i < IOSPACE_MAX; i++) {
        SystemIoSpace_t *Entry = &GlbIoSpaces[i];

        if (Entry->Used == 0 || Entry->Acquired == 0
            || Entry->Io.Type != IO_SPACE_MMIO || Entry->Mapped == 0) {
            continue;
        }

        if (Address >= Entry->Io.VirtualBase
            && Address < (Entry->Io.VirtualBase + Entry->Io.Size)) {
            Result = Entry->Io.PhysicalBase
                + (Address - Entry->Io.VirtualBase);
            break;
        }
    }

    InterruptRestoreState(State);
    return Result;
}

/* IoSpaceRead */
size_t IoSpaceRead(DeviceIoSpace_t *IoSpace, size_t Offset, size_t Length)
{
    if (IoSpace == NULL || (Offset + Length) > IoSpace->Size) {
        LogFatal("IoSpace", "read of %u bytes at +%u is out of bounds",
            Length, Offset);
        return 0;
    }

    if (IoSpace->Type == IO_SPACE_IO) {
        uint16_t Port = (uint16_t)(IoSpace->PhysicalBase + Offset);
        switch (Length) {
            case 1:  return (size_t)inb(Port);
            case 2:  return (size_t)inw(Port);
            case 4:  return (size_t)inl(Port);
            default: break;
        }
    }
    else if (IoSpace->Type == IO_SPACE_MMIO) {
        uintptr_t Address = IoSpace->VirtualBase + Offset;
        if (IoSpace->VirtualBase == 0) {
            LogFatal("IoSpace", "mmio read before acquire, id %u", IoSpace->Id);
            return 0;
        }
        switch (Length) {
            case 1:  return (size_t)(*(volatile uint8_t*)Address);
            case 2:  return (size_t)(*(volatile uint16_t*)Address);
            case 4:  return (size_t)(*(volatile uint32_t*)Address);
            default: break;
        }
    }

    LogFatal("IoSpace", "bad access width %u", Length);
    return 0;
}

/* IoSpaceWrite */
void IoSpaceWrite(DeviceIoSpace_t *IoSpace, size_t Offset,
                  size_t Value, size_t Length)
{
    if (IoSpace == NULL || (Offset + Length) > IoSpace->Size) {
        LogFatal("IoSpace", "write of %u bytes at +%u is out of bounds",
            Length, Offset);
        return;
    }

    if (IoSpace->Type == IO_SPACE_IO) {
        uint16_t Port = (uint16_t)(IoSpace->PhysicalBase + Offset);
        switch (Length) {
            case 1:  outb(Port, (uint8_t)Value);  return;
            case 2:  outw(Port, (uint16_t)Value); return;
            case 4:  outl(Port, (uint32_t)Value); return;
            default: break;
        }
    }
    else if (IoSpace->Type == IO_SPACE_MMIO) {
        uintptr_t Address = IoSpace->VirtualBase + Offset;
        if (IoSpace->VirtualBase == 0) {
            LogFatal("IoSpace", "mmio write before acquire, id %u", IoSpace->Id);
            return;
        }
        switch (Length) {
            case 1:  *(volatile uint8_t*)Address  = (uint8_t)Value;  return;
            case 2:  *(volatile uint16_t*)Address = (uint16_t)Value; return;
            case 4:  *(volatile uint32_t*)Address = (uint32_t)Value; return;
            default: break;
        }
    }

    LogFatal("IoSpace", "bad access width %u", Length);
}

/* IoSpacePrint */
void IoSpacePrint(void)
{
    int i;

    for (i = 0; i < IOSPACE_MAX; i++) {
        SystemIoSpace_t *Entry = &GlbIoSpaces[i];
        if (Entry->Used == 0) {
            continue;
        }
        LogInformation("IoSpace", "  id %u  %s  0x%x + 0x%x  %s",
            Entry->Io.Id,
            (Entry->Io.Type == IO_SPACE_IO) ? "port" : "mmio",
            Entry->Io.PhysicalBase, Entry->Io.Size,
            Entry->Acquired ? "acquired" : "free");
    }
}