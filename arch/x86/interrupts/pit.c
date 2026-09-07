/* Includes
 * - System */
#include <arch/x86/x32/arch_x32.h>
#include <arch/x86/pit8253.h>
#include <interrupts/interrupts.h>
#include <system/timers.h>
#include <system/log.h>

/* Includes
 * - Library */
#include <stddef.h>
#include <string.h>

/* There is only one PIT on the board, so this is static rather than
 * allocated. It is written from interrupt context, hence volatile. */
typedef struct _Pit {
    Interrupt_t     Interrupt;
    UUId_t          Irq;
    size_t          Divisor;
    size_t          NsTick;
    volatile size_t Ticks;
} Pit_t;

static Pit_t GlbPit;
static int GlbPitInitialized = 0;

/* PitOnInterrupt
 * Fast handler, runs in interrupt context with interrupts off. Keep it
 * to a counter bump - the timer registry does the real work, driven from
 * InterruptEntry via TimersInterrupt. */
static InterruptStatus_t PitOnInterrupt(void *Data)
{
    Pit_t *Pit = (Pit_t*)Data;

    if (Pit == NULL) {
        return InterruptNotHandled;
    }

    Pit->Ticks++;
    return InterruptHandled;
}

/* PitInitialize */
OsStatus_t PitInitialize(size_t Frequency)
{
    size_t Divisor;
    size_t ActualHz;
    int i;

    if (GlbPitInitialized) {
        LogFatal("PIT", "already initialized");
        return Error;
    }

    if (Frequency < PIT_MIN_FREQUENCY || Frequency > PIT_MAX_FREQUENCY) {
        LogFatal("PIT", "frequency %u out of range (%u - %u)",
            Frequency, PIT_MIN_FREQUENCY, PIT_MAX_FREQUENCY);
        return Error;
    }

    /* All arithmetic here is deliberately 32-bit. A 64-bit divide would
     * pull in __udivdi3 from libgcc, which this kernel does not link. */
    Divisor = PIT_BASE_FREQUENCY / Frequency;
    if (Divisor > 65535) {
        Divisor = 65535;
    }
    if (Divisor == 0) {
        Divisor = 1;
    }

    /* Derive the tick length from the divisor we can actually program,
     * not from the frequency that was asked for. At 1000 Hz the divisor
     * is 1193, which is really 1000.15 Hz - close, but the drift adds up
     * if you pretend otherwise. */
    ActualHz = PIT_BASE_FREQUENCY / Divisor;
    if (ActualHz == 0) {
        ActualHz = 1;
    }

    memset(&GlbPit, 0, sizeof(Pit_t));
    GlbPit.Divisor = Divisor;
    GlbPit.NsTick  = 1000000000u / ActualHz;
    GlbPit.Ticks   = 0;

    /* Mode 2 (rate generator) rather than mode 3 (square wave). Mode 3
     * halves the counter each half-cycle and misbehaves with odd
     * divisors; mode 2 gives one pulse every N inputs, which is what a
     * tick source wants. */
    outb(PIT_REGISTER_COMMAND,
        PIT_COMMAND_COUNTER_0 | PIT_COMMAND_FULL | PIT_COMMAND_MODE2);
    outb(PIT_REGISTER_COUNTER0, (uint8_t)(Divisor & 0xFF));
    outb(PIT_REGISTER_COUNTER0, (uint8_t)((Divisor >> 8) & 0xFF));

    /* Register the handler. This unmasks IRQ 0, so from here the chip
     * will interrupt as soon as EFLAGS.IF is set. */
    memset(&GlbPit.Interrupt, 0, sizeof(Interrupt_t));
    for (i = 0; i < INTERRUPT_MAXVECTORS; i++) {
        GlbPit.Interrupt.Vectors[i] = INTERRUPT_NONE;
    }
    GlbPit.Interrupt.Line        = PIT_IRQ;
    GlbPit.Interrupt.Pin         = INTERRUPT_NONE;
    GlbPit.Interrupt.FastHandler = PitOnInterrupt;
    GlbPit.Interrupt.Data        = &GlbPit;

    GlbPit.Irq = InterruptRegister(&GlbPit.Interrupt,
        INTERRUPT_KERNEL | INTERRUPT_FAST | INTERRUPT_NOTSHARABLE);
    if (GlbPit.Irq == UUID_INVALID) {
        LogFatal("PIT", "could not register irq %d", PIT_IRQ);
        return Error;
    }

    GlbPitInitialized = 1;

    /* Hand ourselves to the timer registry as a tick source. */
    if (TimersRegister(GlbPit.Irq, GlbPit.NsTick) != Success) {
        LogFatal("PIT", "the timer registry rejected us");
        return Error;
    }

    LogInformation("PIT", "divisor %u -> %u Hz, %u ns per tick",
        Divisor, ActualHz, GlbPit.NsTick);
    return Success;
}

/* PitGetTicks */
size_t PitGetTicks(void)
{
    return GlbPit.Ticks;
}

/* PitGetNsTick */
size_t PitGetNsTick(void)
{
    return GlbPit.NsTick;
}