#ifndef __X86_PIT_H__
#define __X86_PIT_H__

#include <defs.h>
#include <stddef.h>

/* Io-space for accessing the PIT. Spans 0x40-0x44. */
#define PIT_IO_BASE                 0x40
#define PIT_REGISTER_COUNTER0       0x40
#define PIT_REGISTER_COUNTER1       0x41
#define PIT_REGISTER_COUNTER2       0x42
#define PIT_REGISTER_COMMAND        0x43

/* Command register (OCW)
 * bit    0: BCD/binary, 0 = 16-bit binary
 * bits 1-3: operating mode
 * bits 4-5: access mode
 * bits 6-7: channel */
#define PIT_COMMAND_BCD             0x01

#define PIT_COMMAND_MODE0           0x00    /* interrupt on terminal count   */
#define PIT_COMMAND_MODE1           0x02    /* hardware re-triggerable shot  */
#define PIT_COMMAND_MODE2           0x04    /* rate generator                */
#define PIT_COMMAND_MODE3           0x06    /* square wave generator         */
#define PIT_COMMAND_MODE4           0x08    /* software triggered strobe     */
#define PIT_COMMAND_MODE5           0x0A    /* hardware triggered strobe     */

#define PIT_COMMAND_LATCHCOUNT      0x00
#define PIT_COMMAND_LOWBYTE         0x10
#define PIT_COMMAND_HIGHBYTE        0x20
#define PIT_COMMAND_FULL            0x30    /* lobyte then hibyte            */

#define PIT_COMMAND_COUNTER_0       0x00
#define PIT_COMMAND_COUNTER_1       0x40
#define PIT_COMMAND_COUNTER_2       0x80

/* The crystal feeding the chip. Every divisor is derived from this. */
#define PIT_BASE_FREQUENCY          1193182

/* IRQ 0 is hardwired to channel 0 on every PC. */
#define PIT_IRQ                     0

/* Frequency limits. The divisor is 16-bit and a divisor of 0 means
 * 65536, so anything below ~19 Hz cannot be expressed. */
#define PIT_MIN_FREQUENCY           19
#define PIT_MAX_FREQUENCY           PIT_BASE_FREQUENCY

#ifdef __cplusplus
extern "C" {
#endif

/* PitInitialize
 * Programs channel 0 as a rate generator at (approximately) <Frequency>
 * Hz and registers its handler on IRQ 0, which unmasks the line.
 *
 * Must be called after InterruptInitialize and after the heap is up.
 * Interrupts do not have to be enabled yet - nothing is delivered until
 * they are. Returns Success or Error. */
OsStatus_t PitInitialize(size_t Frequency);

/* PitGetTicks
 * Raw interrupt count since PitInitialize. */
size_t PitGetTicks(void);

/* PitGetNsTick
 * Length of one tick in nanoseconds, derived from the divisor actually
 * programmed - not from the frequency that was requested. */
size_t PitGetNsTick(void);

#ifdef __cplusplus
}
#endif

#endif /* __X86_PIT_H__ */