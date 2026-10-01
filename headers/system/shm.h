#ifndef __SHM_H__
#define __SHM_H__

#include <defs.h>
#include <stddef.h>

/* Shared memory.
 *
 * For bulk data, copying is the wrong model: a 4 KB disk block passed
 * through a message is copied at least twice on the way in and twice on
 * the way out. A shared region is mapped into both processes and the
 * data is never copied at all.
 *
 * The kernel provides no structure inside the region. That is
 * deliberate - the protocol is the users' business, and imposing one
 * would be exactly the policy a microkernel should not contain. */

#define SHM_MAX                 16
#define SHM_MAX_PAGES           64      /* 256 KB per region */

typedef struct _SharedMemory {
    UUId_t              Id;
    void               *Owner;
    PhysicalAddress_t   Frames[SHM_MAX_PAGES];
    size_t              PageCount;
    size_t              Length;
    int                 References;
    int                 Used;
} SharedMemory_t;

#ifdef __cplusplus
extern "C" {
#endif

void            ShmInitialize(void);
SharedMemory_t *ShmCreate(void *Process, size_t Length);
uintptr_t       ShmMap(SharedMemory_t *, void *Process);
void            ShmRelease(SharedMemory_t *);
void            ShmPrint(void);

#ifdef __cplusplus
}
#endif

#endif /* __SHM_H__ */
