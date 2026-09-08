#ifndef __IOSPACE_H__
#define __IOSPACE_H__

#include <defs.h>
#include <stddef.h>
#include <stdint.h>

/* The two kinds of io-space. Check IoSpace->Type. */
#define IO_SPACE_INVALID            0x00
#define IO_SPACE_IO                 0x01    /* x86 port space, in/out    */
#define IO_SPACE_MMIO               0x02    /* memory mapped registers   */

/* How many io-spaces can exist. Fixed table for the same reason the
 * timers use one - no list implementation, and this is reachable from
 * paths where allocating would be awkward. */
#define IOSPACE_MAX                 32

/* Represents a region of hardware registers, addressed either through
 * the x86 port space or through memory. */
typedef struct _DeviceIoSpace {
    UUId_t          Id;
    int             Type;
    uintptr_t       PhysicalBase;
    uintptr_t       VirtualBase;    /* MMIO only, filled in by Acquire   */
    size_t          Size;
} DeviceIoSpace_t;

#ifdef __cplusplus
extern "C" {
#endif

/* IoSpaceInitialize
 * Prepares the io-space table. Call before any driver registers. */
void IoSpaceInitialize(void);

/* IoSpaceRegister
 * Registers a region and assigns it an id, written back into
 * IoSpace->Id. Fails if the region overlaps one that already exists -
 * that is the whole point of this subsystem. */
OsStatus_t IoSpaceRegister(DeviceIoSpace_t *IoSpace);

/* IoSpaceAcquire
 * Claims a registered region. Only one owner at a time. For MMIO this
 * also maps the physical range into kernel reserved space and fills in
 * IoSpace->VirtualBase. */
OsStatus_t IoSpaceAcquire(DeviceIoSpace_t *IoSpace);

/* IoSpaceRelease
 * Drops the claim. The MMIO mapping is left in place - see the note in
 * iospace.c. */
OsStatus_t IoSpaceRelease(DeviceIoSpace_t *IoSpace);

/* IoSpaceDestroy
 * Removes a region by id. Fails while it is still acquired. */
OsStatus_t IoSpaceDestroy(UUId_t Id);

/* IoSpaceValidate
 * If <Address> falls inside an acquired MMIO region, returns the
 * corresponding physical address. Returns 0 otherwise. */
uintptr_t IoSpaceValidate(uintptr_t Address);

/* IoSpaceRead / IoSpaceWrite
 * Uniform accessors. <Length> is 1, 2 or 4 bytes. For IO_SPACE_IO these
 * become in/out on PhysicalBase + Offset; for IO_SPACE_MMIO they become
 * ordinary loads and stores through VirtualBase + Offset. */
size_t IoSpaceRead(DeviceIoSpace_t *IoSpace, size_t Offset, size_t Length);
void   IoSpaceWrite(DeviceIoSpace_t *IoSpace, size_t Offset,
                    size_t Value, size_t Length);

/* IoSpacePrint
 * Dumps the table through the logger. */
void IoSpacePrint(void);

#ifdef __cplusplus
}
#endif

#endif /* __IOSPACE_H__ */