#ifndef __PS2_KEYBOARD_H__
#define __PS2_KEYBOARD_H__

#include <defs.h>
#include <stddef.h>
#include <system/pipe.h>

/* The 8042 controller. Two ports: data at 0x60, status/command at 0x64.
 * They are not contiguous, so this claims 0x60..0x65 as one io-space and
 * addresses them by offset. */
#define PS2_IO_BASE                 0x60
#define PS2_IO_LENGTH               0x05

#define PS2_OFFSET_DATA             0x00    /* 0x60 */
#define PS2_OFFSET_STATUS           0x04    /* 0x64, read  */
#define PS2_OFFSET_COMMAND          0x04    /* 0x64, write */

/* Status register bits */
#define PS2_STATUS_OUTPUT_FULL      0x01
#define PS2_STATUS_INPUT_FULL       0x02

#define PS2_KEYBOARD_IRQ            1

#ifdef __cplusplus
extern "C" {
#endif

/* Ps2KeyboardInitialize
 * Claims the controller ports and IRQ 1 and starts filling the pipe with
 * decoded ASCII. Requires the heap, io-spaces and interrupts. */
OsStatus_t Ps2KeyboardInitialize(void);

/* Ps2KeyboardGetPipe
 * The pipe keystrokes arrive on. Read it from a thread. NULL before
 * initialization. */
Pipe_t *Ps2KeyboardGetPipe(void);

/* Ps2KeyboardGetStats */
size_t Ps2KeyboardGetScancodes(void);
size_t Ps2KeyboardGetDropped(void);

#ifdef __cplusplus
}
#endif

#endif /* __PS2_KEYBOARD_H__ */