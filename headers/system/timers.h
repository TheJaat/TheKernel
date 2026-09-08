#ifndef __TIMERS_H__
#define __TIMERS_H__

#include <defs.h>
#include <stddef.h>

/* How many software timers and hardware tick sources can exist.
 * The reference uses linked lists from libds; there is no list
 * implementation in this kernel yet, and more importantly the tick path
 * runs in interrupt context where allocating would race the heap. Fixed
 * tables keep the tick allocation-free. */
#define TIMERS_MAX_SOFTWARE         32
#define TIMERS_MAX_SOURCES          4

typedef void (*TimerHandler_t)(void*);

typedef enum {
    TimerSingleShot,
    TimerPeriodic
} TimerType_t;

typedef struct _SystemTimer {
    UUId_t          Source;         /* interrupt id from InterruptRegister */
    size_t          NsTick;         /* length of one tick, nanoseconds     */
    size_t          Ticks;
    int             Used;
} SystemTimer_t;

typedef struct _Timer {
    UUId_t          Id;
    TimerHandler_t  Callback;
    void           *Args;
    TimerType_t     Type;
    size_t          PeriodicMs;
    long            MsLeft;
    int             Used;
    volatile int    Pending;    /* expired, waiting for the worker */
} Timer_t;

#ifdef __cplusplus
extern "C" {
#endif

/* TimersInitialize
 * Prepares the timer tables. Call before any tick source registers. */
void TimersInitialize(void);

/* TimersStartWorker
 * Spawns the thread that runs expired callbacks. Call once threading is
 * up; until then callbacks run inline in the tick. */
OsStatus_t TimersStartWorker(void);

/* TimersRegister
 * Registers an interrupt as a system tick source. <Source> is the id
 * returned by InterruptRegister and <NsTick> the length of one tick in
 * nanoseconds. The source closest to 1 ms per tick becomes the active
 * one. Returns Success or Error. */
OsStatus_t TimersRegister(UUId_t Source, size_t NsTick);

/* TimersInterrupt
 * Called from InterruptEntry after a handler claims an interrupt. If the
 * interrupt came from the active tick source, this advances system time.
 * Returns Success if it was a tick, Error otherwise. */
OsStatus_t TimersInterrupt(UUId_t Source);

/* TimersCreateTimer
 * Registers a callback to fire after <TimeoutMs>, once or repeatedly.
 * Returns UUID_INVALID if the table is full.
 *
 * Once TimersStartWorker has run, the callback executes in ordinary
 * thread context and may block, allocate and log freely. Before that -
 * during early boot - it runs inline in the tick with interrupts off and
 * must be short. */
UUId_t TimersCreateTimer(TimerHandler_t Callback, void *Args,
    TimerType_t Type, size_t TimeoutMs);

/* TimersDestroyTimer
 * Removes a timer by id. Safe to call from a callback. */
void TimersDestroyTimer(UUId_t TimerId);

/* TimersGetSystemTicks / TimersGetSystemMs
 * Monotonic counters since the active tick source started. */
size_t TimersGetSystemTicks(void);
size_t TimersGetSystemMs(void);

/* DelayMs / StallMs
 * Busy-wait. Requires interrupts to be enabled and a tick source to be
 * active, otherwise the counter never advances and this would hang - so
 * it returns immediately instead. There is no SleepMs until there is a
 * scheduler to sleep on. */
void DelayMs(size_t MilliSeconds);
void StallMs(size_t MilliSeconds);

/* SleepMs
 * Blocks the calling thread for <MilliSeconds> and lets another run.
 * Falls back to DelayMs before threading is up. */
void SleepMs(size_t MilliSeconds);

#ifdef __cplusplus
}
#endif

#endif /* __TIMERS_H__ */