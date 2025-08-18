#ifndef __LOG_H_
#define __LOG_H_

/* Includes */
#include <defs.h>

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
// void LogUpgrade(size_t Size);
void LogRedirect(LogTarget_t Output);
void LogFlush(LogTarget_t Output);

// The log functions
void Log(const char *Message, ...);
void LogRaw(const char *Message, ...);
void LogInformation(const char *System, const char *Message, ...);
void LogDebug(const char *System, const char *Message, ...);
void LogFatal(const char *System, const char *Message, ...);

#ifdef __cplusplus
}
#endif

#endif
