/* Includes
 * - System */
#include <system/modules.h>
#include <system/log.h>
#include <arch/x86/memory.h>
#include <arch/x86/x32/arch_x32.h>
#include <boot/datastructure.h>

/* Includes
 * - Library */
#include <stddef.h>
#include <string.h>

/* The loaded image. Read in place - nothing is copied out, so an entry's
 * data pointer is just an offset into this. */
static RamdiskHeader_t *GlbRamdisk = NULL;
static RamdiskEntry_t  *GlbEntries = NULL;
static size_t GlbRamdiskSize = 0;
static int GlbModulesInitialized = 0;

/* ModulesChecksum
 * Additive sum, matching the rd tool. Not a strong check - it will not
 * catch a reordering - but it does catch the two failures that actually
 * happen: a truncated load, and an entry whose offset points somewhere
 * it should not. */
static uint32_t ModulesChecksum(const uint8_t *Data, size_t Length)
{
    uint32_t Sum = 0;
    size_t i;

    for (i = 0; i < Length; i++) {
        Sum += Data[i];
    }
    return Sum;
}

/* ModulesInitialize */
OsStatus_t ModulesInitialize(BootDescriptor_t *Descriptor)
{
    uint32_t i;

    LogInformation("Modules", "Initializing");

    if (Descriptor == NULL
        || Descriptor->RamDiskAddress == 0
        || Descriptor->RamDiskSize == 0) {
        LogInformation("Modules", "no ramdisk was loaded");
        return Error;
    }

    if (Descriptor->RamDiskSize < sizeof(RamdiskHeader_t)) {
        LogFatal("Modules", "ramdisk is too small to hold a header (%u bytes)",
            Descriptor->RamDiskSize);
        return Error;
    }

    GlbRamdisk = (RamdiskHeader_t*)Descriptor->RamDiskAddress;
    GlbRamdiskSize = Descriptor->RamDiskSize;

    if (GlbRamdisk->Magic != RAMDISK_MAGIC) {
        LogFatal("Modules", "bad magic 0x%x, expected 0x%x",
            GlbRamdisk->Magic, RAMDISK_MAGIC);
        GlbRamdisk = NULL;
        return Error;
    }
    if (GlbRamdisk->Version != RAMDISK_VERSION) {
        LogFatal("Modules", "version %u, this kernel understands %u",
            GlbRamdisk->Version, RAMDISK_VERSION);
        GlbRamdisk = NULL;
        return Error;
    }

    /* The image declares its own size. If the loader gave us less than
     * that, entries near the end would point past what was actually
     * read - and reading them would look like data corruption rather
     * than a short load. */
    if (GlbRamdisk->TotalSize > GlbRamdiskSize) {
        LogFatal("Modules", "image claims %u bytes but only %u were loaded",
            GlbRamdisk->TotalSize, GlbRamdiskSize);
        GlbRamdisk = NULL;
        return Error;
    }

    GlbEntries = (RamdiskEntry_t*)((uint8_t*)GlbRamdisk
                 + sizeof(RamdiskHeader_t));

    /* Validate every entry up front rather than on first use. A bad
     * table found at boot is a clear message; the same table found
     * later is a mysterious fault inside whatever asked for a file. */
    for (i = 0; i < GlbRamdisk->FileCount; i++) {
        RamdiskEntry_t *Entry = &GlbEntries[i];
        uint32_t End = Entry->Offset + Entry->Size;

        if (End < Entry->Offset || End > GlbRamdisk->TotalSize) {
            LogFatal("Modules", "entry %u ('%s') runs outside the image",
                i, Entry->Name);
            GlbRamdisk = NULL;
            return Error;
        }
        if (Entry->Name[RAMDISK_NAME_LENGTH - 1] != '\0') {
            LogFatal("Modules", "entry %u has an unterminated name", i);
            GlbRamdisk = NULL;
            return Error;
        }
    }

    GlbModulesInitialized = 1;

    LogInformation("Modules", "ramdisk at 0x%x, %u files, %u bytes",
        (uintptr_t)GlbRamdisk, GlbRamdisk->FileCount, GlbRamdisk->TotalSize);
    return Success;
}

/* ModulesGetCount */
uint32_t ModulesGetCount(void)
{
    if (GlbModulesInitialized == 0) {
        return 0;
    }
    return GlbRamdisk->FileCount;
}

/* ModulesGetEntry */
RamdiskEntry_t *ModulesGetEntry(uint32_t Index)
{
    if (GlbModulesInitialized == 0 || Index >= GlbRamdisk->FileCount) {
        return NULL;
    }
    return &GlbEntries[Index];
}

/* ModulesFind */
RamdiskEntry_t *ModulesFind(const char *Name)
{
    uint32_t i;

    if (GlbModulesInitialized == 0 || Name == NULL) {
        return NULL;
    }

    for (i = 0; i < GlbRamdisk->FileCount; i++) {
        if (strcmp(GlbEntries[i].Name, Name) == 0) {
            return &GlbEntries[i];
        }
    }
    return NULL;
}

/* ModulesGetData */
const uint8_t *ModulesGetData(RamdiskEntry_t *Entry, size_t *Size)
{
    if (GlbModulesInitialized == 0 || Entry == NULL) {
        return NULL;
    }
    if (Size != NULL) {
        *Size = Entry->Size;
    }
    return (const uint8_t*)GlbRamdisk + Entry->Offset;
}

/* ModulesVerify */
OsStatus_t ModulesVerify(RamdiskEntry_t *Entry)
{
    const uint8_t *Data;
    size_t Size = 0;
    uint32_t Sum;

    Data = ModulesGetData(Entry, &Size);
    if (Data == NULL) {
        return Error;
    }

    Sum = ModulesChecksum(Data, Size);
    if (Sum != Entry->Checksum) {
        LogFatal("Modules", "'%s' checksum 0x%x, expected 0x%x",
            Entry->Name, Sum, Entry->Checksum);
        return Error;
    }
    return Success;
}

/* ModulesPrint */
void ModulesPrint(void)
{
    uint32_t i;

    if (GlbModulesInitialized == 0) {
        LogInformation("Modules", "no ramdisk loaded");
        return;
    }

    for (i = 0; i < GlbRamdisk->FileCount; i++) {
        LogInformation("Modules", "  %s  %u bytes at +%u",
            GlbEntries[i].Name, GlbEntries[i].Size, GlbEntries[i].Offset);
    }
}