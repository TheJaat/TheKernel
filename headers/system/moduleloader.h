#ifndef __MODULELOADER_H__
#define __MODULELOADER_H__

#include <defs.h>
#include <stddef.h>

/* Modules are relocatable ELF objects - plain .o files - loaded from the
 * ramdisk, relocated against a table of kernel symbols, and run as a
 * kernel thread.
 *
 * Relocatable rather than ET_EXEC on purpose: an executable is linked at
 * a fixed address, so every module would need its own reserved slot in
 * the memory map and no two could ever be built independently. A .o
 * carries relocations, so the loader can put it wherever there is room. */

typedef int (*ModuleMain_t)(void);

#ifdef __cplusplus
extern "C" {
#endif

/* ModuleLoad
 * Loads <Name> from the ramdisk, relocates it, and runs its ModuleMain
 * on a new thread. Returns Success once the thread is created - not
 * once the module has finished. */
OsStatus_t ModuleLoad(const char *Name);

/* ModuleLoaderPrintExports
 * Lists the kernel symbols modules are allowed to call. */
void ModuleLoaderPrintExports(void);

#ifdef __cplusplus
}
#endif

#endif /* __MODULELOADER_H__ */