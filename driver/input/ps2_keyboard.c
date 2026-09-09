/* Includes
 * - System */
#include <driver/ps2_keyboard.h>
#include <system/iospace.h>
#include <system/pipe.h>
#include <system/log.h>
#include <interrupts/interrupts.h>
#include <arch/x86/x32/arch_x32.h>

/* Includes
 * - Library */
#include <stddef.h>
#include <string.h>

/* Buffered keystrokes. 256 bytes is far more than anyone types between
 * two reads; the reader only has to keep up on average. */
#define PS2_PIPE_SIZE               256

typedef struct _Ps2Keyboard {
    DeviceIoSpace_t Io;
    Interrupt_t     Interrupt;
    UUId_t          Irq;
    Pipe_t         *Pipe;
    volatile size_t Scancodes;
    volatile size_t Dropped;
    int             ShiftHeld;
    int             ExtendedPending;
} Ps2Keyboard_t;

static Ps2Keyboard_t GlbPs2;
static int GlbPs2Initialized = 0;

/* Scancode set 1, make codes only, US layout. Index is the scancode;
 * 0 means "no printable character". */
static const char Ps2ScancodeMap[128] = {
      0,  27, '1', '2', '3', '4', '5', '6', '7', '8', '9', '0', '-', '=','\b',
    '\t','q', 'w', 'e', 'r', 't', 'y', 'u', 'i', 'o', 'p', '[', ']','\n',   0,
     'a', 's', 'd', 'f', 'g', 'h', 'j', 'k', 'l', ';','\'', '`',   0,'\\',
     'z', 'x', 'c', 'v', 'b', 'n', 'm', ',', '.', '/',   0, '*',   0, ' ',
    /* 0x3A caps, 0x3B-0x44 F1-F10, 0x45 numlock, 0x46 scroll = 13 codes.
     * This row had 14 zeros, which pushed the entire keypad block down
     * by one: kp7 returned nothing and every key after it returned its
     * neighbour's character. */
       0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,
    /* 0x47 onwards: the keypad */
     '7', '8', '9', '-', '4', '5', '6', '+', '1', '2', '3', '0', '.',   0,
       0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,
       0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,
       0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0
};

static const char Ps2ScancodeMapShift[128] = {
      0,  27, '!', '@', '#', '$', '%', '^', '&', '*', '(', ')', '_', '+','\b',
    '\t','Q', 'W', 'E', 'R', 'T', 'Y', 'U', 'I', 'O', 'P', '{', '}','\n',   0,
     'A', 'S', 'D', 'F', 'G', 'H', 'J', 'K', 'L', ':', '"', '~',   0, '|',
     'Z', 'X', 'C', 'V', 'B', 'N', 'M', '<', '>', '?',   0, '*',   0, ' ',
    /* 0x3A caps, 0x3B-0x44 F1-F10, 0x45 numlock, 0x46 scroll = 13 codes.
     * This row had 14 zeros, which pushed the entire keypad block down
     * by one: kp7 returned nothing and every key after it returned its
     * neighbour's character. */
       0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,
    /* 0x47 onwards: the keypad */
     '7', '8', '9', '-', '4', '5', '6', '+', '1', '2', '3', '0', '.',   0,
       0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,
       0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,
       0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0
};

/* Ps2OnInterrupt
 * Fast handler. Reads the byte, decodes it, and pushes ASCII into the
 * pipe without blocking.
 *
 * The read is not optional: the 8042 will not raise IRQ 1 again until
 * its output buffer has been drained. Returning without reading wedges
 * the keyboard permanently. */
static InterruptStatus_t Ps2OnInterrupt(void *Data)
{
    Ps2Keyboard_t *Kbd = (Ps2Keyboard_t*)Data;
    uint8_t Scancode;
    char Character;

    if (Kbd == NULL) {
        return InterruptNotHandled;
    }

    /* Nothing there means this interrupt was not ours. */
    if ((IoSpaceRead(&Kbd->Io, PS2_OFFSET_STATUS, 1)
         & PS2_STATUS_OUTPUT_FULL) == 0) {
        return InterruptNotHandled;
    }

    Scancode = (uint8_t)IoSpaceRead(&Kbd->Io, PS2_OFFSET_DATA, 1);
    Kbd->Scancodes++;

    /* 0xE0 prefixes the extended keys (arrows, right ctrl and so on).
     * Swallow the prefix and the byte after it - none of them map to
     * ASCII here. */
    if (Scancode == 0xE0) {
        Kbd->ExtendedPending = 1;
        return InterruptHandled;
    }
    if (Kbd->ExtendedPending) {
        Kbd->ExtendedPending = 0;
        return InterruptHandled;
    }

    /* Bit 7 set is a break code - the key going up. */
    if (Scancode & 0x80) {
        uint8_t Make = Scancode & 0x7F;
        if (Make == 0x2A || Make == 0x36) {
            Kbd->ShiftHeld = 0;
        }
        return InterruptHandled;
    }

    if (Scancode == 0x2A || Scancode == 0x36) {
        Kbd->ShiftHeld = 1;
        return InterruptHandled;
    }

    Character = Kbd->ShiftHeld ? Ps2ScancodeMapShift[Scancode]
                               : Ps2ScancodeMap[Scancode];
    if (Character == 0) {
        return InterruptHandled;
    }

    /* The pipe is non-blocking on the write side precisely because this
     * runs in interrupt context - blocking here would never return. A
     * full pipe means the reader is not keeping up, so count it and move
     * on rather than stalling the keyboard. */
    if (Kbd->Pipe != NULL) {
        if (PipeWrite(Kbd->Pipe, (const uint8_t*)&Character, 1) != 1) {
            Kbd->Dropped++;
        }
    }

    return InterruptHandled;
}

/* Ps2KeyboardInitialize */
OsStatus_t Ps2KeyboardInitialize(void)
{
    int i;

    if (GlbPs2Initialized) {
        return Success;
    }

    memset(&GlbPs2, 0, sizeof(Ps2Keyboard_t));

    GlbPs2.Io.Type         = IO_SPACE_IO;
    GlbPs2.Io.PhysicalBase = PS2_IO_BASE;
    GlbPs2.Io.Size         = PS2_IO_LENGTH;
    if (IoSpaceRegister(&GlbPs2.Io) != Success
        || IoSpaceAcquire(&GlbPs2.Io) != Success) {
        LogFatal("PS2", "could not claim ports 0x%x + 0x%x",
            PS2_IO_BASE, PS2_IO_LENGTH);
        return Error;
    }

    /* Writes never block, so the handler can use it from interrupt
     * context. Reads block, which is what the consumer thread wants. */
    GlbPs2.Pipe = PipeCreate(PS2_PIPE_SIZE, PIPE_NOBLOCK_WRITE);
    if (GlbPs2.Pipe == NULL) {
        LogFatal("PS2", "could not create the keystroke pipe");
        return Error;
    }

    /* Drain anything the firmware left in the output buffer, or the
     * controller will not raise a fresh interrupt. */
    for (i = 0; i < 16; i++) {
        if ((IoSpaceRead(&GlbPs2.Io, PS2_OFFSET_STATUS, 1)
             & PS2_STATUS_OUTPUT_FULL) == 0) {
            break;
        }
        (void)IoSpaceRead(&GlbPs2.Io, PS2_OFFSET_DATA, 1);
    }

    memset(&GlbPs2.Interrupt, 0, sizeof(Interrupt_t));
    for (i = 0; i < INTERRUPT_MAXVECTORS; i++) {
        GlbPs2.Interrupt.Vectors[i] = INTERRUPT_NONE;
    }
    GlbPs2.Interrupt.Line        = PS2_KEYBOARD_IRQ;
    GlbPs2.Interrupt.Pin         = INTERRUPT_NONE;
    GlbPs2.Interrupt.FastHandler = Ps2OnInterrupt;
    GlbPs2.Interrupt.Data        = &GlbPs2;

    GlbPs2.Irq = InterruptRegister(&GlbPs2.Interrupt,
        INTERRUPT_KERNEL | INTERRUPT_FAST | INTERRUPT_NOTSHARABLE);
    if (GlbPs2.Irq == UUID_INVALID) {
        LogFatal("PS2", "could not register irq %d", PS2_KEYBOARD_IRQ);
        return Error;
    }

    GlbPs2Initialized = 1;
    LogInformation("PS2", "keyboard ready on irq %d, %u byte pipe",
        PS2_KEYBOARD_IRQ, PS2_PIPE_SIZE);
    return Success;
}

/* Ps2KeyboardGetPipe */
Pipe_t *Ps2KeyboardGetPipe(void)
{
    return GlbPs2Initialized ? GlbPs2.Pipe : NULL;
}

size_t Ps2KeyboardGetScancodes(void) { return GlbPs2.Scancodes; }
size_t Ps2KeyboardGetDropped(void)   { return GlbPs2.Dropped; }