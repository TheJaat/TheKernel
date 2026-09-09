#ifndef __GARBAGECOLLECTOR_H__
#define __GARBAGECOLLECTOR_H__

#include <defs.h>
#include <stddef.h>

/* A collector handler. Runs on the gc thread, so it may block, allocate
 * and free freely - which is the whole point. */
typedef OsStatus_t (*GcHandler_t)(void*);

#define GC_MAX_HANDLERS             8

#ifdef __cplusplus
extern "C" {
#endif

/* GcInitialize
 * Starts the collector thread. Requires threading, the heap and
 * semaphores. */
OsStatus_t GcInitialize(void);

/* GcRegister
 * Registers a handler and returns its id, or UUID_INVALID. */
UUId_t GcRegister(GcHandler_t Handler);

/* GcUnregister */
OsStatus_t GcUnregister(UUId_t Handler);

/* GcSignal
 * Queues <Data> for the given handler and wakes the collector.
 *
 * Safe from interrupt context: it allocates a list node, which is the
 * one thing that could block... so it does NOT. See the comment in
 * garbagecollector.c about why the node comes from a preallocated pool. */
OsStatus_t GcSignal(UUId_t Handler, void *Data);

/* GcGetStats */
size_t GcGetCollected(void);
size_t GcGetDropped(void);

#ifdef __cplusplus
}
#endif

#endif /* __GARBAGECOLLECTOR_H__ */