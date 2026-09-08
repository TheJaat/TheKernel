/* Includes
 * - System */
#include <system/timers.h>
#include <system/log.h>
#include <interrupts/interrupts.h>
#include <system/scheduler.h>
#include <system/threading.h>
#include <system/semaphore.h>

/* Includes
 * - Library */
#include <stddef.h>
#include <string.h>

/* Globals */
static SystemTimer_t GlbSystemTimers[TIMERS_MAX_SOURCES];
static Timer_t       GlbTimers[TIMERS_MAX_SOFTWARE];
static SystemTimer_t *GlbActiveSystemTimer = NULL;

static volatile size_t GlbSystemTicks = 0;
static volatile size_t GlbSystemMs    = 0;

/* Nanoseconds carried between ticks. A 1000 Hz PIT tick is 999847 ns,
 * not 1000000, so truncating every tick to 0 ms would stop the clock and
 * rounding up to 1 ms would gain about 15 seconds a day. Accumulate the
 * remainder instead. */
static size_t GlbNsRemainder = 0;

static UUId_t GlbTimerIds = 0;
static int GlbTimersInitialized = 0;

/* Callbacks used to run inside the tick, with interrupts off. Now the
 * tick only marks a timer pending and signals this semaphore; the worker
 * thread runs the callback in normal thread context, where it may block,
 * allocate and log freely. */
static Semaphore_t GlbTimerSignal;
static int GlbTimerWorkerRunning = 0;

/* TimersInitialize */
void TimersInitialize(void)
{
    LogInformation("Timers", "Initializing");

    memset((void*)GlbSystemTimers, 0, sizeof(GlbSystemTimers));
    memset((void*)GlbTimers, 0, sizeof(GlbTimers));

    GlbActiveSystemTimer = NULL;
    GlbSystemTicks = 0;
    GlbSystemMs    = 0;
    GlbNsRemainder = 0;
    GlbTimerIds    = 0;
    GlbTimersInitialized = 1;

    LogInformation("Timers", "Ready, %d sources, %d software timers",
        TIMERS_MAX_SOURCES, TIMERS_MAX_SOFTWARE);
}

/* TimersAbsDelta
 * How far a tick length is from the ideal 1 ms, in nanoseconds. */
static size_t TimersAbsDelta(size_t NsTick)
{
    const size_t Ideal = 1000000u;      /* 1 ms in ns */
    return (NsTick > Ideal) ? (NsTick - Ideal) : (Ideal - NsTick);
}

/* TimersRegister */
OsStatus_t TimersRegister(UUId_t Source, size_t NsTick)
{
    InterruptDescriptor_t *Interrupt = NULL;
    SystemTimer_t *Timer = NULL;
    int State;
    int i;

    if (GlbTimersInitialized != 1) {
        LogFatal("Timers", "register before TimersInitialize");
        return Error;
    }
    if (NsTick == 0) {
        LogFatal("Timers", "a tick source with a zero-length tick");
        return Error;
    }

    /* Only a fast kernel interrupt can drive system time - anything that
     * defers work cannot give an accurate tick. */
    Interrupt = InterruptGet(Source);
    if (Interrupt == NULL) {
        LogFatal("Timers", "no interrupt registered for source 0x%x", Source);
        return Error;
    }
    if ((Interrupt->Flags & (INTERRUPT_FAST | INTERRUPT_KERNEL)) == 0) {
        LogFatal("Timers", "source 0x%x is not a fast kernel interrupt", Source);
        return Error;
    }

    State = InterruptDisable();

    for (i = 0; i < TIMERS_MAX_SOURCES; i++) {
        if (GlbSystemTimers[i].Used == 0) {
            Timer = &GlbSystemTimers[i];
            break;
        }
    }

    if (Timer == NULL) {
        InterruptRestoreState(State);
        LogFatal("Timers", "no free tick source slots");
        return Error;
    }

    Timer->Source = Source;
    Timer->NsTick = NsTick;
    Timer->Ticks  = 0;
    Timer->Used   = 1;

    /* Prefer whichever source lands closest to 1 ms per tick. */
    if (GlbActiveSystemTimer == NULL
        || TimersAbsDelta(NsTick) < TimersAbsDelta(GlbActiveSystemTimer->NsTick)) {
        GlbActiveSystemTimer = Timer;
    }

    InterruptRestoreState(State);

    LogInformation("Timers", "source 0x%x registered, %u ns per tick%s",
        Source, NsTick,
        (GlbActiveSystemTimer == Timer) ? " (now active)" : "");
    return Success;
}

/* TimersTick
 * Applies one tick of <NsTick> nanoseconds. Runs in interrupt context.
 * Does not allocate or free - see the note in timers.h. */
static void TimersTick(size_t NsTick)
{
    size_t MilliTicks;
    int Fired = 0;
    int i;

    GlbSystemTicks++;

    /* Convert ns to whole ms, carrying the remainder so the clock does
     * not drift. */
    GlbNsRemainder += NsTick;
    MilliTicks = GlbNsRemainder / 1000000u;
    if (MilliTicks == 0) {
        return;
    }
    GlbNsRemainder -= MilliTicks * 1000000u;
    GlbSystemMs += MilliTicks;

    /* Advance sleeping threads before the software timers, so a thread
     * woken this tick is runnable by the time the scheduler looks. */
    SchedulerApplyMs(MilliTicks);

    for (i = 0; i < TIMERS_MAX_SOFTWARE; i++) {
        Timer_t *Timer = &GlbTimers[i];

        if (Timer->Used == 0) {
            continue;
        }

        Timer->MsLeft -= (long)MilliTicks;
        if (Timer->MsLeft > 0) {
            continue;
        }

        /* Re-arm or retire *before* calling out, so a callback that
         * destroys its own timer does not fight with us afterwards. */
        if (Timer->Type == TimerPeriodic) {
            Timer->MsLeft = (long)Timer->PeriodicMs;
        }
        else {
            Timer->Used = 0;
        }

        /* Mark and signal rather than call. Setting a flag is
         * idempotent, so a timer that expires again before the worker
         * has drained it simply stays pending instead of needing a
         * queue that could overflow in interrupt context. */
        if (Timer->Callback != NULL) {
            if (GlbTimerWorkerRunning) {
                Timer->Pending = 1;
                Fired++;
            }
            else {
                /* No worker yet - early boot. Call inline, as before. */
                Timer->Callback(Timer->Args);
            }
        }
    }

    if (Fired > 0) {
        SemaphoreV(&GlbTimerSignal, Fired);
    }
}

/* TimersWorker
 * Drains pending timer callbacks. Runs as an ordinary thread, so a
 * callback here may do anything a thread may do. */
static void TimersWorker(void *Args)
{
    (void)Args;

    for (;;) {
        SemaphoreP(&GlbTimerSignal, 0);

        for (int i = 0; i < TIMERS_MAX_SOFTWARE; i++) {
            TimerHandler_t Callback = NULL;
            void *CallbackArgs = NULL;
            int State = InterruptDisable();

            if (GlbTimers[i].Pending != 0) {
                GlbTimers[i].Pending = 0;
                Callback     = GlbTimers[i].Callback;
                CallbackArgs = GlbTimers[i].Args;
            }

            InterruptRestoreState(State);

            /* Outside the critical section, so the callback runs with
             * interrupts on and can be preempted like anything else. */
            if (Callback != NULL) {
                Callback(CallbackArgs);
            }
        }
    }
}

/* TimersStartWorker */
OsStatus_t TimersStartWorker(void)
{
    if (GlbTimersInitialized != 1) {
        return Error;
    }
    if (GlbTimerWorkerRunning != 0) {
        return Success;
    }

    SemaphoreConstruct(&GlbTimerSignal, 0);

    if (ThreadingCreateThread("timers", TimersWorker, NULL, 0)
        == UUID_INVALID) {
        LogFatal("Timers", "could not start the worker thread");
        return Error;
    }

    GlbTimerWorkerRunning = 1;
    LogInformation("Timers", "worker started, callbacks now run in thread context");
    return Success;
}

/* TimersInterrupt */
OsStatus_t TimersInterrupt(UUId_t Source)
{
    if (GlbActiveSystemTimer != NULL
        && GlbActiveSystemTimer->Source == Source) {
        GlbActiveSystemTimer->Ticks++;
        TimersTick(GlbActiveSystemTimer->NsTick);
        return Success;
    }
    return Error;
}

/* TimersCreateTimer */
UUId_t TimersCreateTimer(TimerHandler_t Callback, void *Args,
    TimerType_t Type, size_t TimeoutMs)
{
    Timer_t *Timer = NULL;
    UUId_t Id;
    int State;
    int i;

    if (GlbTimersInitialized != 1 || Callback == NULL || TimeoutMs == 0) {
        return UUID_INVALID;
    }

    State = InterruptDisable();

    for (i = 0; i < TIMERS_MAX_SOFTWARE; i++) {
        if (GlbTimers[i].Used == 0) {
            Timer = &GlbTimers[i];
            break;
        }
    }

    if (Timer == NULL) {
        InterruptRestoreState(State);
        LogFatal("Timers", "no free software timer slots");
        return UUID_INVALID;
    }

    Id = GlbTimerIds++;
    Timer->Id         = Id;
    Timer->Callback   = Callback;
    Timer->Args       = Args;
    Timer->Type       = Type;
    Timer->PeriodicMs = TimeoutMs;
    Timer->MsLeft     = (long)TimeoutMs;
    Timer->Used       = 1;

    InterruptRestoreState(State);
    return Id;
}

/* TimersDestroyTimer */
void TimersDestroyTimer(UUId_t TimerId)
{
    int State = InterruptDisable();
    int i;

    for (i = 0; i < TIMERS_MAX_SOFTWARE; i++) {
        if (GlbTimers[i].Used != 0 && GlbTimers[i].Id == TimerId) {
            GlbTimers[i].Used = 0;
            GlbTimers[i].Callback = NULL;
            break;
        }
    }

    InterruptRestoreState(State);
}

/* TimersGetSystemTicks */
size_t TimersGetSystemTicks(void)
{
    return GlbSystemTicks;
}

/* TimersGetSystemMs */
size_t TimersGetSystemMs(void)
{
    return GlbSystemMs;
}

/* DelayMs */
void DelayMs(size_t MilliSeconds)
{
    size_t Start;

    if (GlbActiveSystemTimer == NULL) {
        LogFatal("Timers", "DelayMs with no active tick source");
        return;
    }
    if (!InterruptIsEnabled()) {
        /* The counter only advances from an interrupt. Waiting on it
         * with interrupts off is an unconditional hang, so refuse. */
        LogFatal("Timers", "DelayMs with interrupts disabled");
        return;
    }

    Start = GlbSystemMs;
    while ((GlbSystemMs - Start) < MilliSeconds) {
        __asm__ volatile ("hlt");
    }
}

/* StallMs
 * Busy-waits without giving up the cpu. Use SleepMs unless you are in a
 * context that cannot yield. */
void StallMs(size_t MilliSeconds)
{
    DelayMs(MilliSeconds);
}

/* SleepMs
 * Blocks the calling thread and lets something else run. Falls back to
 * busy-waiting when threading is not up yet. */
void SleepMs(size_t MilliSeconds)
{
    if (ThreadingIsEnabled() == 0) {
        DelayMs(MilliSeconds);
        return;
    }
    SchedulerSleepThread(MilliSeconds);
}