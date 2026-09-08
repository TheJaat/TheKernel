/* Includes
 * - System */
#include <system/pipe.h>
#include <system/heap.h>
#include <system/log.h>
#include <system/threading.h>
#include <system/scheduler.h>
#include <interrupts/interrupts.h>

/* Includes
 * - Library */
#include <stddef.h>
#include <string.h>

/* PipeInternalAvailable / PipeInternalFree
 * Caller holds the lock. The ring keeps one slot empty so that
 * IndexWrite == IndexRead can only mean "empty" - without that, full
 * and empty are indistinguishable. */
static size_t PipeInternalAvailable(Pipe_t *Pipe)
{
    if (Pipe->IndexWrite >= Pipe->IndexRead) {
        return Pipe->IndexWrite - Pipe->IndexRead;
    }
    return Pipe->Length - (Pipe->IndexRead - Pipe->IndexWrite);
}

static size_t PipeInternalFree(Pipe_t *Pipe)
{
    return (Pipe->Length - 1) - PipeInternalAvailable(Pipe);
}

/* PipeConstruct */
void PipeConstruct(Pipe_t *Pipe, uint8_t *Buffer, size_t Length, Flags_t Flags)
{
    if (Pipe == NULL || Buffer == NULL || Length < 2) {
        return;
    }

    Pipe->Flags        = Flags;
    Pipe->Buffer       = Buffer;
    Pipe->Length       = Length;
    Pipe->IndexWrite   = 0;
    Pipe->IndexRead    = 0;
    Pipe->ReadWaiting  = 0;
    Pipe->WriteWaiting = 0;
    Pipe->Owned        = 0;

    SpinlockReset(&Pipe->Lock);
    SemaphoreConstruct(&Pipe->ReadQueue, 0);
    SemaphoreConstruct(&Pipe->WriteQueue, 0);
}

/* PipeCreate */
Pipe_t *PipeCreate(size_t Size, Flags_t Flags)
{
    Pipe_t *Pipe;
    uint8_t *Buffer;

    if (Size < 2) {
        return NULL;
    }

    Pipe = (Pipe_t*)kmalloc(sizeof(Pipe_t));
    if (Pipe == NULL) {
        LogFatal("Pipe", "out of memory for the pipe");
        return NULL;
    }

    Buffer = (uint8_t*)kmalloc(Size);
    if (Buffer == NULL) {
        kfree(Pipe);
        LogFatal("Pipe", "out of memory for a %u byte buffer", Size);
        return NULL;
    }

    memset(Pipe, 0, sizeof(Pipe_t));
    memset(Buffer, 0, Size);
    PipeConstruct(Pipe, Buffer, Size, Flags);
    Pipe->Owned = 1;
    return Pipe;
}

/* PipeDestroy */
void PipeDestroy(Pipe_t *Pipe)
{
    if (Pipe == NULL) {
        return;
    }

    /* Release both queues first. A thread blocked on a pipe that is
     * being torn down would otherwise wait forever. */
    SchedulerWakeAll(&Pipe->ReadQueue);
    SchedulerWakeAll(&Pipe->WriteQueue);

    if (Pipe->Owned) {
        kfree(Pipe->Buffer);
        kfree(Pipe);
    }
}

/* PipeWrite */
size_t PipeWrite(Pipe_t *Pipe, const uint8_t *Data, size_t Length)
{
    size_t Written = 0;

    if (Pipe == NULL || Data == NULL || Length == 0) {
        return 0;
    }

    while (Written < Length) {
        int WakeReader = 0;
        int MustWait = 0;

        SpinlockAcquireIrq(&Pipe->Lock);

        while (Written < Length && PipeInternalFree(Pipe) > 0) {
            Pipe->Buffer[Pipe->IndexWrite] = Data[Written++];
            Pipe->IndexWrite = (Pipe->IndexWrite + 1) % Pipe->Length;
            WakeReader = 1;
        }

        if (Written < Length) {
            if (Pipe->Flags & PIPE_NOBLOCK_WRITE) {
                SpinlockReleaseIrq(&Pipe->Lock);
                break;
            }
            Pipe->WriteWaiting++;
            MustWait = 1;
        }

        SpinlockReleaseIrq(&Pipe->Lock);

        /* Signal after dropping the lock. Waking a reader that then
         * immediately blocks on this same lock is pure waste. */
        if (WakeReader) {
            int State = InterruptDisable();
            if (Pipe->ReadWaiting > 0) {
                Pipe->ReadWaiting--;
                SemaphoreV(&Pipe->ReadQueue, 1);
            }
            InterruptRestoreState(State);
        }

        if (MustWait) {
            SemaphoreP(&Pipe->WriteQueue, 0);
        }
    }

    return Written;
}

/* PipeRead */
size_t PipeRead(Pipe_t *Pipe, uint8_t *Buffer, size_t Length, int Peek)
{
    size_t Read = 0;

    if (Pipe == NULL || Buffer == NULL || Length == 0) {
        return 0;
    }

    for (;;) {
        int WakeWriter = 0;
        int MustWait = 0;
        size_t PeekIndex;

        SpinlockAcquireIrq(&Pipe->Lock);

        PeekIndex = Pipe->IndexRead;
        while (Read < Length && PipeInternalAvailable(Pipe) > 0) {
            if (Peek) {
                /* A peek that consumed would make the name a lie, and
                 * the caller could never see the same byte twice. */
                if (PeekIndex == Pipe->IndexWrite) {
                    break;
                }
                Buffer[Read++] = Pipe->Buffer[PeekIndex];
                PeekIndex = (PeekIndex + 1) % Pipe->Length;
            }
            else {
                Buffer[Read++] = Pipe->Buffer[Pipe->IndexRead];
                Pipe->IndexRead = (Pipe->IndexRead + 1) % Pipe->Length;
                WakeWriter = 1;
            }
        }

        if (Read == 0 && !Peek) {
            if (Pipe->Flags & PIPE_NOBLOCK_READ) {
                SpinlockReleaseIrq(&Pipe->Lock);
                break;
            }
            Pipe->ReadWaiting++;
            MustWait = 1;
        }

        SpinlockReleaseIrq(&Pipe->Lock);

        if (WakeWriter) {
            int State = InterruptDisable();
            if (Pipe->WriteWaiting > 0) {
                Pipe->WriteWaiting--;
                SemaphoreV(&Pipe->WriteQueue, 1);
            }
            InterruptRestoreState(State);
        }

        if (!MustWait) {
            break;
        }

        SemaphoreP(&Pipe->ReadQueue, 0);
    }

    return Read;
}

/* PipeBytesAvailable */
size_t PipeBytesAvailable(Pipe_t *Pipe)
{
    size_t Result;

    if (Pipe == NULL) {
        return 0;
    }
    SpinlockAcquireIrq(&Pipe->Lock);
    Result = PipeInternalAvailable(Pipe);
    SpinlockReleaseIrq(&Pipe->Lock);
    return Result;
}

/* PipeBytesFree */
size_t PipeBytesFree(Pipe_t *Pipe)
{
    size_t Result;

    if (Pipe == NULL) {
        return 0;
    }
    SpinlockAcquireIrq(&Pipe->Lock);
    Result = PipeInternalFree(Pipe);
    SpinlockReleaseIrq(&Pipe->Lock);
    return Result;
}