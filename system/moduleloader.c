/* Includes
 * - System */
#include <system/moduleloader.h>
#include <system/modules.h>
#include <system/threading.h>
#include <system/timers.h>
#include <system/heap.h>
#include <system/log.h>
#include <system/elf.h>
#include <arch/x86/memory.h>
#include <arch/x86/x32/arch_x32.h>
#include <arch/x86/address_space.h>
#include <system/process.h>
#include <interrupts/interrupts.h>

/* Includes
 * - Library */
#include <stddef.h>
#include <stdio.h>
#include <string.h>
#include <stdint.h>

/* A symbol a module is allowed to call. Anything not in here is an
 * undefined reference and the load is refused - which is the whole point
 * of having the table rather than letting modules reach into the kernel
 * by address. */
typedef struct _KernelExport {
    const char *Name;
    uintptr_t   Address;
} KernelExport_t;

static const KernelExport_t GlbExports[] = {
    { "printf",                     (uintptr_t)&printf                  },
    { "LogInformation",             (uintptr_t)&LogInformation          },
    { "LogFatal",                   (uintptr_t)&LogFatal                },
    { "kmalloc",                    (uintptr_t)&kmalloc                 },
    { "kfree",                      (uintptr_t)&kfree                   },
    { "memset",                     (uintptr_t)&memset                  },
    { "memcpy",                     (uintptr_t)&memcpy                  },
    { "strlen",                     (uintptr_t)&strlen                  },
    { "strcmp",                     (uintptr_t)&strcmp                  },
    { "SleepMs",                    (uintptr_t)&SleepMs                 },
    { "TimersGetSystemMs",          (uintptr_t)&TimersGetSystemMs       },
    { "ThreadingGetCurrentThreadId",(uintptr_t)&ThreadingGetCurrentThreadId },
    { NULL, 0 }
};

/* One loaded module. */
typedef struct _LoadedModule {
    char          Name[RAMDISK_NAME_LENGTH];
    uint8_t      *Image;        /* the allocation holding every section */
    uintptr_t    *SectionBase;  /* where each section landed, or 0      */
    ModuleMain_t  Entry;
} LoadedModule_t;

/* ModuleFindExport */
static uintptr_t ModuleFindExport(const char *Name)
{
    int i;

    for (i = 0; GlbExports[i].Name != NULL; i++) {
        if (strcmp(GlbExports[i].Name, Name) == 0) {
            return GlbExports[i].Address;
        }
    }
    return 0;
}

/* ModuleLoaderPrintExports */
void ModuleLoaderPrintExports(void)
{
    int i;

    for (i = 0; GlbExports[i].Name != NULL; i++) {
        printf("  %-28s 0x%x\n", GlbExports[i].Name, GlbExports[i].Address);
    }
}

/* ModuleAlignUp */
static uintptr_t ModuleAlignUp(uintptr_t Value, uintptr_t Align)
{
    if (Align <= 1) {
        return Value;
    }
    return (Value + (Align - 1)) & ~(Align - 1);
}

/* ModuleThread
 * Runs a loaded module's entry point, then releases its image. */
static void ModuleThread(void *Args)
{
    LoadedModule_t *Module = (LoadedModule_t*)Args;
    int Result;

    if (Module == NULL || Module->Entry == NULL) {
        return;
    }

    Result = Module->Entry();
    LogInformation("Module", "'%s' returned %d", Module->Name, Result);

    /* The module's code lives in this allocation, so it can only be
     * freed once nothing is executing it - i.e. after Entry returns,
     * from this thread, which is about to exit anyway. */
    kfree(Module->SectionBase);
    kfree(Module->Image);
    kfree(Module);
}

/* ModuleRelocateAgain
 * Re-applies relocations so the image works at <NewBase> instead of
 * <OldBase>.
 *
 * The first pass resolved everything against the staging copy in the
 * kernel heap. A user module has no undefined symbols, so every
 * relocation is internal - which means each one can simply be shifted
 * by the delta rather than recomputed. R_386_PC32 needs no adjustment
 * at all: it is relative, and moving the whole image leaves the
 * distance between two points inside it unchanged. */
static OsStatus_t ModuleRelocateAgain(const uint8_t *Raw, Elf32_Ehdr *Header,
    Elf32_Shdr *Sections, uintptr_t *SectionBase,
    uintptr_t OldBase, uintptr_t NewBase, const char *Name)
{
    /* Unsigned on purpose: this libc has no intptr_t, and wrap-around
     * gives the right answer for a negative delta anyway - adding the
     * wrapped value is the same as subtracting the difference. */
    uintptr_t Delta = NewBase - OldBase;
    uint32_t i, j;

    for (i = 0; i < Header->e_shnum; i++) {
        Elf32_Rel *Relocs;
        uint32_t Count;
        uintptr_t TargetBase;

        if (Sections[i].sh_type != SHT_REL) {
            continue;
        }
        TargetBase = SectionBase[Sections[i].sh_info];
        if (TargetBase == 0) {
            continue;
        }

        Relocs = (Elf32_Rel*)(Raw + Sections[i].sh_offset);
        Count = Sections[i].sh_size / sizeof(Elf32_Rel);

        for (j = 0; j < Count; j++) {
            uint32_t Type = ELF32_R_TYPE(Relocs[j].r_info);
            uint32_t *Target = (uint32_t*)(TargetBase + Relocs[j].r_offset);

            if (Type == R_386_32) {
                *Target = (uint32_t)(*Target + Delta);
            }
            else if (Type == R_386_PC32 || Type == R_386_NONE) {
                /* relative - unaffected by moving the image */
            }
            else {
                LogFatal("Module", "'%s' relocation type %u cannot be "
                    "rebased", Name, Type);
                return Error;
            }
        }
    }

    return Success;
}

/* ModuleLoadInternal */
static OsStatus_t ModuleLoadInternal(const char *Name, int UserMode,
    Flags_t Privileges)
{
    RamdiskEntry_t *File;
    const uint8_t *Raw;
    size_t RawSize = 0;
    Elf32_Ehdr *Header;
    Elf32_Shdr *Sections;
    LoadedModule_t *Module = NULL;
    uintptr_t *SectionBase = NULL;
    uint8_t *Image = NULL;
    size_t ImageSize = 0;
    uintptr_t Cursor;
    uint32_t i, j;

    File = ModulesFind(Name);
    if (File == NULL) {
        LogFatal("Module", "'%s' is not on the ramdisk", Name);
        return Error;
    }
    if (ModulesVerify(File) != Success) {
        LogFatal("Module", "'%s' failed its checksum", Name);
        return Error;
    }

    Raw = ModulesGetData(File, &RawSize);
    if (Raw == NULL || RawSize < sizeof(Elf32_Ehdr)) {
        LogFatal("Module", "'%s' is too small to be an object", Name);
        return Error;
    }

    Header = (Elf32_Ehdr*)Raw;
    if (Header->e_ident[0] != ELFMAG0 || Header->e_ident[1] != ELFMAG1
        || Header->e_ident[2] != ELFMAG2 || Header->e_ident[3] != ELFMAG3) {
        LogFatal("Module", "'%s' is not an ELF file", Name);
        return Error;
    }
    if (Header->e_ident[EI_CLASS] != ELFCLASS32
        || Header->e_ident[EI_DATA] != ELFDATA2LSB
        || Header->e_machine != EM_386) {
        LogFatal("Module", "'%s' is not a 32-bit little-endian i386 object", Name);
        return Error;
    }
    if (Header->e_type != ET_REL) {
        LogFatal("Module", "'%s' is type %u, expected ET_REL (a .o)",
            Name, Header->e_type);
        return Error;
    }
    if (Header->e_shoff == 0 || Header->e_shnum == 0
        || Header->e_shentsize < sizeof(Elf32_Shdr)) {
        LogFatal("Module", "'%s' has no usable section table", Name);
        return Error;
    }
    if ((Header->e_shoff + ((size_t)Header->e_shnum * Header->e_shentsize))
        > RawSize) {
        LogFatal("Module", "'%s' section table runs past end of file", Name);
        return Error;
    }

    Sections = (Elf32_Shdr*)(Raw + Header->e_shoff);

    /* Pass 1: how much room do the allocated sections need, respecting
     * each one's alignment. */
    for (i = 0; i < Header->e_shnum; i++) {
        if ((Sections[i].sh_flags & SHF_ALLOC) == 0
            || Sections[i].sh_size == 0) {
            continue;
        }
        ImageSize = ModuleAlignUp(ImageSize, Sections[i].sh_addralign);
        ImageSize += Sections[i].sh_size;
    }

    if (ImageSize == 0) {
        LogFatal("Module", "'%s' has nothing to load", Name);
        return Error;
    }

    Image = (uint8_t*)kmalloc_a(ImageSize);
    SectionBase = (uintptr_t*)kmalloc(sizeof(uintptr_t) * Header->e_shnum);
    Module = (LoadedModule_t*)kmalloc(sizeof(LoadedModule_t));

    if (Image == NULL || SectionBase == NULL || Module == NULL) {
        LogFatal("Module", "out of memory loading '%s'", Name);
        kfree(Image); kfree(SectionBase); kfree(Module);
        return Error;
    }

    memset(Image, 0, ImageSize);
    memset(SectionBase, 0, sizeof(uintptr_t) * Header->e_shnum);
    memset(Module, 0, sizeof(LoadedModule_t));

    /* Pass 2: place each section and copy it in. */
    Cursor = (uintptr_t)Image;
    for (i = 0; i < Header->e_shnum; i++) {
        if ((Sections[i].sh_flags & SHF_ALLOC) == 0
            || Sections[i].sh_size == 0) {
            continue;
        }

        Cursor = ModuleAlignUp(Cursor, Sections[i].sh_addralign);
        SectionBase[i] = Cursor;

        if (Sections[i].sh_type == SHT_NOBITS) {
            /* .bss - already zeroed by the memset above. */
        }
        else {
            if ((Sections[i].sh_offset + Sections[i].sh_size) > RawSize) {
                LogFatal("Module", "'%s' section %u runs past end of file",
                    Name, i);
                kfree(Image); kfree(SectionBase); kfree(Module);
                return Error;
            }
            memcpy((void*)Cursor, Raw + Sections[i].sh_offset,
                Sections[i].sh_size);
        }

        Cursor += Sections[i].sh_size;
    }

    /* Pass 3: relocations. i386 objects use SHT_REL - the addend lives
     * in the target word, not in the relocation entry. */
    for (i = 0; i < Header->e_shnum; i++) {
        Elf32_Rel *Relocs;
        Elf32_Sym *Symbols;
        const char *Strings;
        uint32_t Count;
        uintptr_t TargetBase;

        if (Sections[i].sh_type != SHT_REL) {
            continue;
        }

        /* sh_info is the section being relocated; sh_link is the symbol
         * table to resolve against. */
        TargetBase = SectionBase[Sections[i].sh_info];
        if (TargetBase == 0) {
            continue;   /* relocations for a section we did not load */
        }

        Relocs = (Elf32_Rel*)(Raw + Sections[i].sh_offset);
        Count = Sections[i].sh_size / sizeof(Elf32_Rel);

        Symbols = (Elf32_Sym*)(Raw + Sections[Sections[i].sh_link].sh_offset);
        Strings = (const char*)(Raw
                + Sections[Sections[Sections[i].sh_link].sh_link].sh_offset);

        for (j = 0; j < Count; j++) {
            uint32_t SymIndex = ELF32_R_SYM(Relocs[j].r_info);
            uint32_t Type = ELF32_R_TYPE(Relocs[j].r_info);
            Elf32_Sym *Symbol = &Symbols[SymIndex];
            uint32_t *Target = (uint32_t*)(TargetBase + Relocs[j].r_offset);
            uintptr_t S;

            if (Symbol->st_shndx == SHN_UNDEF) {
                const char *SymName = Strings + Symbol->st_name;

                /* A user module may not link against the kernel at all.
                 * Ring 3 cannot call a ring-0 address, so resolving one
                 * would just produce a protection fault at the call
                 * site - far better to refuse now and say why. User
                 * modules reach the kernel through int 0x80 only. */
                if (UserMode) {
                    LogFatal("Module", "'%s' references '%s'; user modules "
                        "may only use syscalls", Name, SymName);
                    kfree(Image); kfree(SectionBase); kfree(Module);
                    return Error;
                }

                S = ModuleFindExport(SymName);
                if (S == 0) {
                    /* Refusing here is the point of the export table.
                     * Leaving it unresolved would produce a call to
                     * address 0 the first time that path ran. */
                    LogFatal("Module", "'%s' needs '%s', which the kernel "
                        "does not export", Name, SymName);
                    kfree(Image); kfree(SectionBase); kfree(Module);
                    return Error;
                }
            }
            else if (Symbol->st_shndx == SHN_ABS) {
                S = Symbol->st_value;
            }
            else {
                if (Symbol->st_shndx >= Header->e_shnum
                    || SectionBase[Symbol->st_shndx] == 0) {
                    LogFatal("Module", "'%s' symbol in an unloaded section",
                        Name);
                    kfree(Image); kfree(SectionBase); kfree(Module);
                    return Error;
                }
                S = SectionBase[Symbol->st_shndx] + Symbol->st_value;
            }

            switch (Type) {
                case R_386_NONE:
                    break;
                case R_386_32:
                    /* S + A, the addend already sitting in the word. */
                    *Target = (uint32_t)(S + *Target);
                    break;
                case R_386_PC32:
                    /* S + A - P. Getting this one wrong gives a module
                     * that loads cleanly and jumps into hyperspace on
                     * its first call. */
                    *Target = (uint32_t)(S + *Target - (uintptr_t)Target);
                    break;
                default:
                    LogFatal("Module", "'%s' uses relocation type %u, "
                        "which is not supported", Name, Type);
                    kfree(Image); kfree(SectionBase); kfree(Module);
                    return Error;
            }
        }
    }

    /* Find the entry point. */
    Module->Entry = NULL;
    for (i = 0; i < Header->e_shnum && Module->Entry == NULL; i++) {
        Elf32_Sym *Symbols;
        const char *Strings;
        uint32_t Count;

        if (Sections[i].sh_type != SHT_SYMTAB) {
            continue;
        }

        Symbols = (Elf32_Sym*)(Raw + Sections[i].sh_offset);
        Count = Sections[i].sh_size / sizeof(Elf32_Sym);
        Strings = (const char*)(Raw + Sections[Sections[i].sh_link].sh_offset);

        for (j = 0; j < Count; j++) {
            if (Symbols[j].st_shndx == SHN_UNDEF
                || Symbols[j].st_shndx >= Header->e_shnum) {
                continue;
            }
            if (strcmp(Strings + Symbols[j].st_name, "ModuleMain") == 0) {
                Module->Entry = (ModuleMain_t)(SectionBase[Symbols[j].st_shndx]
                                + Symbols[j].st_value);
                break;
            }
        }
    }

    if (Module->Entry == NULL) {
        LogFatal("Module", "'%s' has no ModuleMain", Name);
        kfree(Image); kfree(SectionBase); kfree(Module);
        return Error;
    }

    for (i = 0; i < (RAMDISK_NAME_LENGTH - 1) && Name[i] != '\0'; i++) {
        Module->Name[i] = Name[i];
    }
    Module->Image = Image;
    Module->SectionBase = SectionBase;

    LogInformation("Module", "'%s' loaded at 0x%x, %u bytes, entry 0x%x%s",
        Name, (uintptr_t)Image, ImageSize, (uintptr_t)Module->Entry,
        UserMode ? " (ring 3)" : "");

    if (UserMode) {
        Process_t *Process;
        AddressSpace_t *Space;
        uintptr_t EntryOffset = (uintptr_t)Module->Entry - (uintptr_t)Image;
        uintptr_t UserImage = MEMORY_LOCATION_RING3_CODE;
        uintptr_t UserStackTop;
        size_t Offset;
        int State;

        Process = ProcessCreate(Name, Privileges);
        if (Process == NULL) {
            kfree(Image); kfree(SectionBase); kfree(Module);
            return Error;
        }
        Space = Process->AddressSpace;
        ProcessAddThread(Process);      /* held until the thread exists */

        /* Map private frames for the image and the stack into the new
         * space at the ring-3 addresses. These are the process's own
         * pages: nothing else can see them, which is the entire point
         * of giving it a directory of its own. */
        for (Offset = 0; Offset < ImageSize; Offset += PAGE_SIZE) {
            PhysicalAddress_t Frame = MmPhysicalAllocateBlock(__MASK, 1);
            if (Frame == 0 || MmVirtualMap(Space->PageDirectory, Frame,
                    UserImage + Offset, PAGE_USER) != Success) {
                LogFatal("Module", "could not map the image for '%s'", Name);
                ProcessRemoveThread(Process);
                kfree(Image); kfree(SectionBase); kfree(Module);
                return Error;
            }
        }
        /* The process owns stack allocation, so a second thread in the
         * same process gets its own rather than sharing this one. */
        UserStackTop = ProcessAllocateUserStack(Process);
        if (UserStackTop == 0) {
            ProcessRemoveThread(Process);
            kfree(Image); kfree(SectionBase); kfree(Module);
            return Error;
        }

        /* Relocations were resolved against the kernel-heap copy, so
         * redo them for where the image will actually live. Simpler
         * than a second relocation pass: adjust by the delta, which is
         * only valid because every internal reference is an absolute
         * address inside the image. */
        if (ModuleRelocateAgain(Raw, Header, Sections, SectionBase,
                (uintptr_t)Image, UserImage, Name) != Success) {
            ProcessRemoveThread(Process);
            kfree(Image); kfree(SectionBase); kfree(Module);
            return Error;
        }

        /* Copy the prepared image into the process. Switching CR3 is
         * the simplest way to reach those addresses; interrupts are off
         * so no thread switch can reload CR3 underneath us. */
        State = InterruptDisable();
        AddressSpaceSwitch(Space);
        memcpy((void*)UserImage, Image, ImageSize);
        memset((void*)(UserStackTop - PROCESS_USER_STACK_SIZE), 0,
            PROCESS_USER_STACK_SIZE);
        AddressSpaceSwitch(AddressSpaceGetKernel());
        InterruptRestoreState(State);

        LogInformation("Module", "'%s' mapped at 0x%x in its own space",
            Name, UserImage);

        {
            UUId_t Tid = ThreadingCreateUserThreadInSpace("umodule",
                UserImage + EntryOffset, Space, UserStackTop, 0);
            Thread_t *Thread;

            if (Tid == UUID_INVALID) {
                LogFatal("Module", "could not create a user thread for '%s'",
                    Name);
                ProcessRemoveThread(Process);
                kfree(Image); kfree(SectionBase); kfree(Module);
                return Error;
            }

            Thread = ThreadingGetThread(Tid);
            if (Thread != NULL) {
                Thread->Process = Process;
            }
            /* Drop the reference held across setup now the real thread
             * owns one. */
            ProcessAddThread(Process);
            ProcessRemoveThread(Process);
        }

        /* The kernel-side staging copy has done its job. */
        kfree(Image);
        kfree(SectionBase);
        kfree(Module);
        return Success;
    }

    if (ThreadingCreateThread("module", ModuleThread, Module, 0)
        == UUID_INVALID) {
        LogFatal("Module", "could not create a thread for '%s'", Name);
        kfree(Image); kfree(SectionBase); kfree(Module);
        return Error;
    }

    return Success;
}

/* ModuleLoad / ModuleLoadUser */
OsStatus_t ModuleLoad(const char *Name)
{
    return ModuleLoadInternal(Name, 0, PROCESS_PRIV_NONE);
}

OsStatus_t ModuleLoadUser(const char *Name)
{
    return ModuleLoadInternal(Name, 1, PROCESS_PRIV_NONE);
}

/* ModuleLoadServer
 * A user module granted hardware privileges. Reserved for modules
 * started from the ramdisk at boot - an application loaded later must
 * not be able to claim an interrupt line. */
OsStatus_t ModuleLoadServer(const char *Name)
{
    return ModuleLoadInternal(Name, 1, PROCESS_PRIV_HARDWARE);
}