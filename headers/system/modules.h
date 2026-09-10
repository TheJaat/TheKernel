#ifndef __MODULES_H__
#define __MODULES_H__

#include <defs.h>
#include <stddef.h>
#include <stdint.h>
#include <boot/datastructure.h>
#include <boot/ramdisk.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ModulesInitialize
 * Validates the ramdisk the bootloader left at Descriptor->RamDiskAddress
 * and makes its contents available. Returns Error when there is no
 * ramdisk or it does not validate - neither is fatal to the kernel. */
OsStatus_t ModulesInitialize(BootDescriptor_t *Descriptor);

/* ModulesGetCount / ModulesGetEntry
 * Enumerate. Entry indices are stable for the life of the boot. */
uint32_t ModulesGetCount(void);
RamdiskEntry_t *ModulesGetEntry(uint32_t Index);

/* ModulesFind
 * Looks a file up by name, or NULL. */
RamdiskEntry_t *ModulesFind(const char *Name);

/* ModulesGetData
 * Pointer to the file's bytes, in place inside the loaded image. Nothing
 * is copied, so this stays valid and must not be freed. */
const uint8_t *ModulesGetData(RamdiskEntry_t *Entry, size_t *Size);

/* ModulesVerify
 * Recomputes the checksum recorded by the rd tool. */
OsStatus_t ModulesVerify(RamdiskEntry_t *Entry);

/* ModulesPrint */
void ModulesPrint(void);

#ifdef __cplusplus
}
#endif

#endif /* __MODULES_H__ */