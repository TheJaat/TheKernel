/* Includes
 * - System */
#include <system/garbagecollector.h>
#include <system/threading.h>
#include <system/semaphore.h>
#include <system/spinlock.h>
#include <system/heap.h>
#include <system/log.h>
#include <interrupts/interrupts.h>
#include <ds/list.h>

/* Includes
 * - Library */
#include <stddef.h>
#include <string.h>

/* Work items are allocated from a fixed pool rather than the heap.
 *
 * GcSignal is called from interrupt context - ThreadingExit signals it,
 * and so will any driver that frees on an interrupt path. kmalloc there
 * would take the heap lock, and while masking interrupts makes that
 * safe on one cpu, an allocation failure in the middle of a teardown has
 * nowhere sensible to go. A pool means signalling cannot fail for lack
 * of memory - only for lack of pool slots, which is a bounded, countable
 * condition. */
#define GC_POOL_SIZE                64

typedef struct _GcWork {
    UUId_t          Handler;
    void           *Data;
    int             Used;
} GcWork_t;

static GcHandler_t GlbGcHandlers[GC_MAX_HANDLERS];
static int GlbGcHandlerUsed[GC_MAX_HANDLERS];

static GcWork_t GlbGcPool[GC_POOL_SIZE];
static Spinlock_t GlbGcLock = SPINLOCK_INIT;
static Semaphore_t GlbGcSignal;

static volatile size_t GlbGcCollected = 0;
static volatile size_t GlbGcDropped = 0;
static int GlbGcInitialized = 0;

/* GcRegister */
UUId_t GcRegister(GcHandler_t Handler)
{
    UUId_t Id = UUID_INVALID;
    int i;

    if (Handler == NULL) {
        return UUID_INVALID;
    }

    SpinlockAcquireIrq(&GlbGcLock);

    for (i = 0; i < GC_MAX_HANDLERS; i++) {
        if (GlbGcHandlerUsed[i] == 0) {
            GlbGcHandlerUsed[i] = 1;
            GlbGcHandlers[i] = Handler;
            Id = (UUId_t)i;
            break;
        }
    }

    SpinlockReleaseIrq(&GlbGcLock);

    if (Id == UUID_INVALID) {
        LogFatal("Gc", "no free handler slots");
    }
    return Id;
}

/* GcUnregister */
OsStatus_t GcUnregister(UUId_t Handler)
{
    OsStatus_t Result = Error;

    if (Handler >= GC_MAX_HANDLERS) {
        return Error;
    }

    SpinlockAcquireIrq(&GlbGcLock);
    if (GlbGcHandlerUsed[Handler] != 0) {
        GlbGcHandlerUsed[Handler] = 0;
        GlbGcHandlers[Handler] = NULL;
        Result = Success;
    }
    SpinlockReleaseIrq(&GlbGcLock);

    return Result;
}

/* GcSignal */
OsStatus_t GcSignal(UUId_t Handler, void *Data)
{
    int Slot = -1;
    int i;

    if (GlbGcInitialized == 0 || Handler >= GC_MAX_HANDLERS) {
        return Error;
    }

    SpinlockAcquireIrq(&GlbGcLock);

    if (GlbGcHandlerUsed[Handler] == 0) {
        SpinlockReleaseIrq(&GlbGcLock);
        return Error;
    }

    for (i = 0; i < GC_POOL_SIZE; i++) {
        if (GlbGcPool[i].Used == 0) {
            GlbGcPool[i].Used    = 1;
            GlbGcPool[i].Handler = Handler;
            GlbGcPool[i].Data    = Data;
            Slot = i;
            break;
        }
    }

    if (Slot < 0) {
        GlbGcDropped++;
    }

    SpinlockReleaseIrq(&GlbGcLock);

    if (Slot < 0) {
        /* Not fatal, but it does mean something leaked. Counted so 'gc'
         * in the shell can show it rather than it being invisible. */
        return Error;
    }

    SemaphoreV(&GlbGcSignal, 1);
    return Success;
}

/* GcWorker */
static void GcWorker(void *Args)
{
    (void)Args;

    for (;;) {
        SemaphoreP(&GlbGcSignal, 0);

        for (int i = 0; i < GC_POOL_SIZE; i++) {
            GcHandler_t Handler = NULL;
            void *Data = NULL;

            SpinlockAcquireIrq(&GlbGcLock);
            if (GlbGcPool[i].Used != 0) {
                UUId_t Id = GlbGcPool[i].Handler;
                if (Id < GC_MAX_HANDLERS && GlbGcHandlerUsed[Id] != 0) {
                    Handler = GlbGcHandlers[Id];
                }
                Data = GlbGcPool[i].Data;
                GlbGcPool[i].Used = 0;
            }
            SpinlockReleaseIrq(&GlbGcLock);

            /* Run the handler outside the lock. It is allowed to free,
             * allocate and block, and holding the gc lock across any of
             * that would defeat the purpose. */
            if (Handler != NULL) {
                Handler(Data);
                GlbGcCollected++;
            }
        }
    }
}

/* GcInitialize */
OsStatus_t GcInitialize(void)
{
    LogInformation("Gc", "Initializing");

    memset(GlbGcHandlers, 0, sizeof(GlbGcHandlers));
    memset(GlbGcHandlerUsed, 0, sizeof(GlbGcHandlerUsed));
    memset(GlbGcPool, 0, sizeof(GlbGcPool));
    SpinlockReset(&GlbGcLock);
    SemaphoreConstruct(&GlbGcSignal, 0);
    GlbGcCollected = 0;
    GlbGcDropped = 0;

    if (ThreadingCreateThread("gc", GcWorker, NULL, 0) == UUID_INVALID) {
        LogFatal("Gc", "could not start the collector thread");
        return Error;
    }

    GlbGcInitialized = 1;
    LogInformation("Gc", "Ready, %d handlers, %d work slots",
        GC_MAX_HANDLERS, GC_POOL_SIZE);
    return Success;
}

size_t GcGetCollected(void) { return GlbGcCollected; }
size_t GcGetDropped(void)   { return GlbGcDropped; }