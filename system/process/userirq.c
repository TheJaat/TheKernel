/* Includes
 * - System */
#include <system/userirq.h>
#include <system/process.h>
#include <system/semaphore.h>
#include <system/log.h>
#include <interrupts/interrupts.h>
#include <arch/x86/pic.h>

/* Includes
 * - Library */
#include <stddef.h>
#include <string.h>

static UserInterrupt_t GlbUserIrqs[USERIRQ_MAX];
static Interrupt_t GlbIrqDescriptors[USERIRQ_MAX];

/* UserIrqInitialize */
void UserIrqInitialize(void)
{
    memset(GlbUserIrqs, 0, sizeof(GlbUserIrqs));
    memset(GlbIrqDescriptors, 0, sizeof(GlbIrqDescriptors));
    LogInformation("UserIrq", "Ready, %d forwarding slots", USERIRQ_MAX);
}

/* UserIrqStub
 * Runs in interrupt context, with interrupts off.
 *
 * It does three things and nothing else: mask, count, signal. It must
 * not touch the device - it does not know how, that is the driver's job
 * - and it must not block or allocate. SemaphoreV is safe here because
 * it does neither. */
static InterruptStatus_t UserIrqStub(void *Data)
{
    UserInterrupt_t *Entry = (UserInterrupt_t*)Data;

    if (Entry == NULL || Entry->Used == 0) {
        return InterruptNotHandled;
    }

    /* Mask BEFORE returning. InterruptEntry sends the EOI as soon as we
     * return InterruptHandled, and the device is still asserting. */
    PicMaskLine(Entry->Line);
    Entry->Masked = 1;
    Entry->Fired++;

    SemaphoreV(&Entry->Signal, 1);
    return InterruptHandled;
}

/* UserIrqRegister */
UserInterrupt_t *UserIrqRegister(void *Process, int Line)
{
    UserInterrupt_t *Entry = NULL;
    Interrupt_t *Descriptor;
    int Index = -1;
    int i;

    if (Process == NULL || Line < 0 || Line >= PIC_NUM_LINES) {
        return NULL;
    }
    if (Line == PIC_CASCADE_LINE) {
        LogFatal("UserIrq", "line 2 is the cascade and cannot be claimed");
        return NULL;
    }

    for (i = 0; i < USERIRQ_MAX; i++) {
        if (GlbUserIrqs[i].Used != 0 && GlbUserIrqs[i].Line == Line) {
            LogFatal("UserIrq", "line %d is already forwarded", Line);
            return NULL;
        }
        if (GlbUserIrqs[i].Used == 0 && Index < 0) {
            Index = i;
        }
    }
    if (Index < 0) {
        LogFatal("UserIrq", "no free forwarding slots");
        return NULL;
    }

    Entry = &GlbUserIrqs[Index];
    memset(Entry, 0, sizeof(UserInterrupt_t));
    Entry->Line   = Line;
    Entry->Owner  = Process;
    Entry->Used   = 1;
    Entry->Masked = 0;
    SemaphoreConstruct(&Entry->Signal, 0);

    Descriptor = &GlbIrqDescriptors[Index];
    memset(Descriptor, 0, sizeof(Interrupt_t));
    for (i = 0; i < INTERRUPT_MAXVECTORS; i++) {
        Descriptor->Vectors[i] = INTERRUPT_NONE;
    }
    Descriptor->Line        = Line;
    Descriptor->Pin         = INTERRUPT_NONE;
    Descriptor->FastHandler = UserIrqStub;
    Descriptor->Data        = Entry;

    Entry->Id = InterruptRegister(Descriptor,
        INTERRUPT_KERNEL | INTERRUPT_FAST | INTERRUPT_NOTSHARABLE);

    if (Entry->Id == UUID_INVALID) {
        Entry->Used = 0;
        LogFatal("UserIrq", "could not claim line %d", Line);
        return NULL;
    }

    LogInformation("UserIrq", "line %d forwarded to process %u",
        Line, ((Process_t*)Process)->Id);
    return Entry;
}

/* UserIrqWait */
int UserIrqWait(UserInterrupt_t *Entry)
{
    if (Entry == NULL || Entry->Used == 0) {
        return -1;
    }

    /* Blocks. The semaphore counts pending interrupts, so a driver that
     * was slow to run does not lose one - it returns immediately and
     * catches up. */
    SemaphoreP(&Entry->Signal, 0);
    return (int)Entry->Fired;
}

/* UserIrqAcknowledge */
OsStatus_t UserIrqAcknowledge(UserInterrupt_t *Entry)
{
    if (Entry == NULL || Entry->Used == 0) {
        return Error;
    }

    if (Entry->Masked) {
        Entry->Masked = 0;
        PicUnmaskLine(Entry->Line);
    }
    return Success;
}

/* UserIrqUnregister */
OsStatus_t UserIrqUnregister(UserInterrupt_t *Entry)
{
    if (Entry == NULL || Entry->Used == 0) {
        return Error;
    }

    InterruptUnregister(Entry->Id);

    /* InterruptUnregister masks the line once its last handler is gone,
     * which is the behaviour we want: a driver that died leaves its
     * device quiet rather than storming. */
    Entry->Used = 0;
    Entry->Owner = NULL;
    return Success;
}

/* UserIrqPrint */
void UserIrqPrint(void)
{
    int i;

    for (i = 0; i < USERIRQ_MAX; i++) {
        if (GlbUserIrqs[i].Used == 0) {
            continue;
        }
        LogInformation("UserIrq", "  line %d -> process %u, %u fired%s",
            GlbUserIrqs[i].Line,
            ((Process_t*)GlbUserIrqs[i].Owner)->Id,
            GlbUserIrqs[i].Fired,
            GlbUserIrqs[i].Masked ? ", masked (awaiting ack)" : "");
    }
}
