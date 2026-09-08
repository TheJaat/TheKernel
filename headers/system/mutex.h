#ifndef __MUTEX_H__
#define __MUTEX_H__

#include <defs.h>
#include <stddef.h>
#include <system/spinlock.h>

#define MUTEX_DEFAULT_TIMEOUT       500

/* A sleeping, recursive lock. The owning thread may take it again
 * without deadlocking; every lock needs a matching unlock. */
typedef struct _Mutex {
    Spinlock_t      Lock;
    UUId_t          Blocker;    /* owning thread, UUID_INVALID when free */
    size_t          Blocks;     /* recursion depth                       */
} Mutex_t;

#ifdef __cplusplus
extern "C" {
#endif

/* MutexConstruct
 * Initializes a mutex the caller already has storage for. */
void MutexConstruct(Mutex_t *Mutex);

/* MutexCreate / MutexDestruct */
Mutex_t *MutexCreate(void);
void MutexDestruct(Mutex_t *Mutex);

/* MutexLock
 * Blocks until the mutex is held. Returns Success, or Error if
 * threading is not up. */
OsStatus_t MutexLock(Mutex_t *Mutex);

/* MutexTryLock
 * Returns Success if taken, Error if it is held by someone else. */
OsStatus_t MutexTryLock(Mutex_t *Mutex);

/* MutexUnlock
 * Drops one level of ownership and wakes a waiter at zero. */
void MutexUnlock(Mutex_t *Mutex);

#ifdef __cplusplus
}
#endif

#endif /* __MUTEX_H__ */