/* Includes
 * - System */
#include <system/moduleloader.h>
#include <system/modules.h>
#include <system/threading.h>
#include <system/timers.h>
#include <system/heap.h>
#include <system/log.h>
#include <system/elf.h>

/* Includes
 * - Library */
#include <stddef.h>
#include <stdio.h>
#include <string.h>

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

/* ModuleLoad */
OsStatus_t ModuleLoad(const char *Name)
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

    LogInformation("Module", "'%s' loaded at 0x%x, %u bytes, entry 0x%x",
        Name, (uintptr_t)Image, ImageSize, (uintptr_t)Module->Entry);

    if (ThreadingCreateThread("module", ModuleThread, Module, 0)
        == UUID_INVALID) {
        LogFatal("Module", "could not create a thread for '%s'", Name);
        kfree(Image); kfree(SectionBase); kfree(Module);
        return Error;
    }

    return Success;
}