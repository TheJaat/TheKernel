/* Includes
 * - System */
#include <system/syscalls.h>
#include <system/threading.h>
#include <system/timers.h>
#include <system/log.h>
#include <interrupts/interrupts.h>
#include <arch/x86/x32/context.h>

/* Includes
 * - Library */
#include <stddef.h>
#include <stdio.h>
#include <string.h>

static Interrupt_t GlbSyscallInterrupt;
static volatile size_t GlbSyscallCount = 0;
static int GlbSyscallsInitialized = 0;

/* SyscallWrite
 * Writes a user buffer to the terminal.
 *
 * The length is clamped and the buffer is copied a chunk at a time
 * rather than handed straight to printf. A user pointer is not to be
 * trusted: it may be null, may be enormous, and may not be
 * null-terminated - printf("%s") on it would run until it faulted, in
 * kernel mode, on the user's behalf. */
static int SyscallWrite(const char *Buffer, size_t Length)
{
    size_t i;

    if (Buffer == NULL) {
        return -1;
    }
    if (Length > 4096) {
        Length = 4096;
    }

    for (i = 0; i < Length; i++) {
        char c = Buffer[i];
        if (c == '\0') {
            break;
        }
        printf("%c", c);
    }
    return (int)i;
}

/* SyscallHandler
 * Runs on the syscall vector with the interrupted register frame.
 *
 * Convention, matching the wrappers in the user library:
 *   eax = syscall number,  ebx, ecx, edx = arguments
 *   eax on return = result */
static InterruptStatus_t SyscallHandler(void *Data)
{
    Context_t *Registers = (Context_t*)Data;
    uint32_t Number;
    int Result = -1;

    if (Registers == NULL) {
        return InterruptNotHandled;
    }

    Number = Registers->Eax;
    GlbSyscallCount++;

    switch (Number) {
        case SYS_EXIT: {
            /* ThreadingExit yields and never returns, so eax is never
             * written back - which is correct, there is nobody left to
             * read it. */
            LogInformation("Syscall", "thread %u exited with %d",
                ThreadingGetCurrentThreadId(), (int)Registers->Ebx);
            ThreadingExit();
            break;
        }
        case SYS_WRITE: {
            Result = SyscallWrite((const char*)Registers->Ebx,
                (size_t)Registers->Ecx);
            break;
        }
        case SYS_SLEEP: {
            SleepMs((size_t)Registers->Ebx);
            Result = 0;
            break;
        }
        case SYS_GETMS: {
            Result = (int)TimersGetSystemMs();
            break;
        }
        case SYS_GETTID: {
            Result = (int)ThreadingGetCurrentThreadId();
            break;
        }
        default: {
            LogFatal("Syscall", "thread %u made unknown call %u",
                ThreadingGetCurrentThreadId(), Number);
            Result = -1;
            break;
        }
    }

    Registers->Eax = (uint32_t)Result;
    return InterruptHandled;
}

/* SyscallsInitialize */
OsStatus_t SyscallsInitialize(void)
{
    int i;

    if (GlbSyscallsInitialized) {
        return Success;
    }

    memset(&GlbSyscallInterrupt, 0, sizeof(Interrupt_t));
    for (i = 0; i < INTERRUPT_MAXVECTORS; i++) {
        GlbSyscallInterrupt.Vectors[i] = INTERRUPT_NONE;
    }
    GlbSyscallInterrupt.Vectors[0]  = SYSCALL_VECTOR;
    GlbSyscallInterrupt.Line        = INTERRUPT_NONE;
    GlbSyscallInterrupt.Pin         = INTERRUPT_NONE;
    GlbSyscallInterrupt.FastHandler = SyscallHandler;
    /* Data stays NULL on purpose: InterruptEntry passes the Context_t
     * when Data is NULL, and the register frame is the whole point. */
    GlbSyscallInterrupt.Data        = NULL;

    if (InterruptRegister(&GlbSyscallInterrupt,
            INTERRUPT_KERNEL | INTERRUPT_SOFTWARE | INTERRUPT_FAST)
        == UUID_INVALID) {
        LogFatal("Syscall", "could not register vector %d", SYSCALL_VECTOR);
        return Error;
    }

    GlbSyscallsInitialized = 1;
    LogInformation("Syscall", "Ready, %d calls on vector %d",
        SYS_MAX, SYSCALL_VECTOR);
    return Success;
}

size_t SyscallsGetCount(void) { return GlbSyscallCount; }