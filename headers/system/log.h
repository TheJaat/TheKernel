#ifndef __LOG_H_
#define __LOG_H_

/* Includes */
#include <defs.h>
#include <stddef.h>

// Definitions
typedef enum LogTarget
{
	LogMemory,
	LogConsole,
	LogFile
} LogTarget_t;

typedef enum LogLevel
{
	LogLevel1,
	LogLevel2,
	LogLevel3
} LogLevel_t;

// Colors
#define LOG_COLOR_INFORMATION		0x2ECC71
#define LOG_COLOR_DEBUG				0x9B59B6
#define LOG_COLOR_ERROR				0xFF392B
#define LOG_COLOR_DEFAULT			0x0

// Default size to 4kb
#define LOG_INITIAL_SIZE			(1024 * 4)
#define LOG_PREFFERED_SIZE			(1024 * 65)

// Log Types
#define LOG_TYPE_RAW				0x00
#define LOG_TYPE_INFORMATION		0x01
#define LOG_TYPE_DEBUG				0x02
#define LOG_TYPE_FATAL				0x03


#ifdef __cplusplus
extern "C" {
#endif

// Functions
void LogInit(void);
OsStatus_t LogUpgrade(size_t Size);
void LogRedirect(LogTarget_t Output);
void LogFlush(LogTarget_t Output);

/* The log functions.
 *
 * noinline is load-bearing, not a style choice. These are variadic, and
 * log.cpp calls them from LogUpgrade - i.e. from inside the same
 * translation unit. At -O2 GCC inlines the callee into LogUpgrade, and
 * the inlined va_start then resolves against LogUpgrade's own parameter
 * list instead of the arguments actually passed. Every %u/%x then reads
 * from the wrong stack slot and prints bytes of the format string.
 * At -O1 the call is left alone and everything is fine, which makes this
 * an unusually nasty thing to chase. */
#ifndef __LOG_NOINLINE
#define __LOG_NOINLINE __attribute__((noinline))
#endif

__LOG_NOINLINE void Log(const char *Message, ...);
__LOG_NOINLINE void LogRaw(const char *Message, ...);
__LOG_NOINLINE void LogInformation(const char *System, const char *Message, ...);
__LOG_NOINLINE void LogDebug(const char *System, const char *Message, ...);
__LOG_NOINLINE void LogFatal(const char *System, const char *Message, ...);

#ifdef __cplusplus
}
#endif

#endif