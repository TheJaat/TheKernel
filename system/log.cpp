#include <system/log.h>


#include <stddef.h>
#include <stdio.h>
#include <string.h>

#include <video/interface/video_interface.h>
#include <system/heap.h>

// Globals
// UUId_t GlbLogFileHandle = UUID_INVALID;
// BufferObject_t *GlbLogBuffer = NULL;
char GlbLogStatic[LOG_INITIAL_SIZE];
LogTarget_t GlbLogTarget = LogMemory;
LogLevel_t GlbLogLevel = LogLevel1;
size_t GlbLogSize = 0;
char *GlbLog = NULL;
int GlbLogIndex = 0;

/* Instantiates the Log
 * with default params */
void LogInit(void)
{
	GlbLogTarget = LogConsole;//LogMemory;
	GlbLogLevel = LogLevel1;

	// Set log ptr to initial
	GlbLog = &GlbLogStatic[0];
	GlbLogSize = LOG_INITIAL_SIZE;

	// Clear out log
	memset(GlbLog, 0, GlbLogSize);
	GlbLogIndex = 0;
}

/* LogUpgrade
 * Moves the log out of the static boot buffer into a larger one on the
 * heap. Requires HeapInit() to have run. Safe to call more than once.
 *
 * Returns Success, or Error with the log left on its current buffer -
 * a failed upgrade must never leave GlbLog dangling. */
OsStatus_t LogUpgrade(size_t Size)
{
	char *nBuffer = NULL;
	char *oBuffer = GlbLog;
	int   Copy    = GlbLogIndex;

	/* Shrinking would truncate what is already recorded. */
	if (Size <= (size_t)GlbLogIndex) {
		LogFatal("LOG", "upgrade to %u would truncate %u bytes already logged",
			Size, GlbLogIndex);
		return Error;
	}

	nBuffer = (char*)kmalloc(Size);
	if (nBuffer == NULL) {
		/* kmalloc has already logged the failure. Stay on the current
		 * buffer - the log keeps working, just smaller. */
		LogFatal("LOG", "could not allocate %u bytes, keeping the static buffer",
			Size);
		return Error;
	}

	memset(nBuffer, 0, Size);
	if (Copy > 0) {
		memcpy(nBuffer, oBuffer, (size_t)Copy);
	}

	/* Swap before freeing, so nothing can log into a freed buffer. */
	GlbLog     = nBuffer;
	GlbLogSize = Size;

	if (oBuffer != &GlbLogStatic[0]) {
		kfree(oBuffer);
	}

	LogInformation("LOG", "upgraded to %u bytes at 0x%x, %u carried over",
		Size, (uintptr_t)nBuffer, Copy);
	return Success;
}

// Switches target
void LogRedirect(LogTarget_t Output)
{
	// Ignore if already
	if (GlbLogTarget == Output)
		return;

	// Update target
	GlbLogTarget = Output;

	// If we redirect to anything else than
	// memory, flush the log
	LogFlush(Output);
}

// Flushes the log
void LogFlush(LogTarget_t Output)
{
	/* Valid flush targets are:
	 * Console
	 * File */
	char TempBuffer[256];

	/* If we are flushing to anything 
	 * other than a file, and the logfile is 
	 * opened, we close it */
	// if (GlbLogFileHandle != UUID_INVALID
	// 	&& Output != LogFile)  {
	// 	CloseFile(GlbLogFileHandle);
	// 	GlbLogFileHandle = UUID_INVALID;
	// }

	// Flush to console?
	if (Output == LogConsole)
	{
		/* Vars */
		int Index = 0;

		/* Iterate */
		while (Index < GlbLogIndex)
		{
			/* Get header information */
			char Type = GlbLog[Index];
			/* Must be unsigned: a 128..255 byte message stored in a signed
			 * char reads back negative, and (size_t)Length then becomes a
			 * ~4GB memcpy. */
			unsigned char Length = (unsigned char)GlbLog[Index + 1];

			/* Zero buffer */
			memset(TempBuffer, 0, 256);

			/* What kind of line is this? */
			if (Type == LOG_TYPE_RAW)
			{
				/* Copy data */
				memcpy(TempBuffer, &GlbLog[Index + 2], (size_t)Length);

				/* Flush it */
					VideoGetTerminal()->fgColor = LOG_COLOR_DEFAULT;
				printf("%s", (const char*)&TempBuffer[0]);

				/* Increase */
				Index += 2 + Length;
			}
			else 
			{
				/* We have two chunks to print */
				char *StartPtr = &GlbLog[Index + 2];
				char *StartMsgPtr = strchr(StartPtr, ' ');
					int HeaderLen;

					/* A record with no separating space is corrupt - stop
					 * instead of walking off the end of the buffer. */
					if (StartMsgPtr == NULL) {
						break;
					}
					HeaderLen = (int)(StartMsgPtr - StartPtr);

				/* Copy */
				memcpy(TempBuffer, StartPtr, HeaderLen);

				/* Select Color */
				if (Type == LOG_TYPE_INFORMATION)
					VideoGetTerminal()->fgColor = LOG_COLOR_INFORMATION;
				else if (Type == LOG_TYPE_DEBUG)
					VideoGetTerminal()->fgColor = LOG_COLOR_DEBUG;
				else if (Type == LOG_TYPE_FATAL)
					VideoGetTerminal()->fgColor = LOG_COLOR_ERROR;

				/* Print header */
				printf("[%s] ", (const char*)&TempBuffer[0]);

				/* Clear */
					memset(TempBuffer, 0, 256);

				/* Increament */
				Index += 2 + HeaderLen + 1;

				/* Copy data */
				memcpy(TempBuffer, &GlbLog[Index], (size_t)Length);

				/* Sanity.
				 * The body of this if was commented out, which silently made
				 * the printf below into its body - so FATAL lines lost their
				 * message on every flush. */
				if (Type != LOG_TYPE_FATAL) {
					VideoGetTerminal()->fgColor = LOG_COLOR_DEFAULT;
				}

				/* Finally, flush */
					printf("%s\n", (const char*)&TempBuffer[0]);

				/* Restore */
				VideoGetTerminal()->fgColor = LOG_COLOR_DEFAULT;

				/* Increase again */
					/* Step over the message AND the trailing newline that
					 * LogInternalPrint appends. The newline is part of the
					 * record but is not counted in Length, so not skipping it
					 * left Index one byte short - every record after the first
					 * was then parsed from the wrong offset and came out
					 * garbled. */
					Index += Length + 1;
			}
		}
	}
	else if (Output == LogFile)
	{
        // TODO: Flush to file when fs ready
		
	}
}

// Internal Log Print
void LogInternalPrint(int LogType, const char *Header, const char *Message)
{
	/* Temporary format buffer 
	 * used by fileprint */
	int HeaderLen = (Header != NULL) ? (int)strlen(Header) : 0;
	int MessageLen = (int)strlen(Message);
	int Needed;

	/* The length is stored in one byte, so clamp it. */
	if (MessageLen > 255) {
		MessageLen = 255;
	}

	/* Bytes this record actually consumes: 2 header bytes, plus for
	 * non-raw records the system name, a space and a trailing newline.
	 * The old test counted only MessageLen, so a record could run up to
	 * HeaderLen + 4 bytes past the end of the buffer. Survivable while
	 * GlbLog was a static array; once LogUpgrade moves it onto the heap
	 * that overrun lands in the next allocation. */
	Needed = 2 + MessageLen + ((LogType != LOG_TYPE_RAW) ? (HeaderLen + 2) : 0);


	// Log it into memory - if we have room
	if ((GlbLogIndex + Needed) <= (int)GlbLogSize)
	{
		/* Write header */
		GlbLog[GlbLogIndex] = (char)LogType;
		GlbLog[GlbLogIndex + 1] = (char)MessageLen;

		/* Increase */
		GlbLogIndex += 2;

		if (LogType != LOG_TYPE_RAW && Header != NULL)
		{
			/* Add Header */
			memcpy(&GlbLog[GlbLogIndex], Header, HeaderLen);
			GlbLogIndex += HeaderLen;

			/* Add a space */
			GlbLog[GlbLogIndex] = ' ';
			GlbLogIndex++;
		}

		/* Add it */
		memcpy(&GlbLog[GlbLogIndex], Message, MessageLen);
		GlbLogIndex += MessageLen;

		if (LogType != LOG_TYPE_RAW)
		{
			/* Add a newline */
			GlbLog[GlbLogIndex] = '\n';
			GlbLogIndex++;
		}
	}

	// Print it
	if (GlbLogTarget == LogConsole) 
	{
		/* Header first */
		if (LogType != LOG_TYPE_RAW)
		{
			// Select Color: TODO
			if (LogType == LOG_TYPE_INFORMATION)
				VideoGetTerminal()->fgColor = LOG_COLOR_INFORMATION;
			else if (LogType == LOG_TYPE_DEBUG)
				VideoGetTerminal()->fgColor = LOG_COLOR_DEBUG;
			else if (LogType == LOG_TYPE_FATAL)
				VideoGetTerminal()->fgColor = LOG_COLOR_ERROR;

			/* Print */
			printf("[%s] ", Header);
		}

		/* Sanity */
		if (LogType != LOG_TYPE_FATAL)
			VideoGetTerminal()->fgColor = LOG_COLOR_DEFAULT;

		/* Print */
		if (LogType == LOG_TYPE_RAW)
			printf("%s", Message);
		else
			printf("%s\n", Message);

		/* Restore */
		VideoGetTerminal()->fgColor = LOG_COLOR_DEFAULT;
	}
	else if (GlbLogTarget == LogFile) {
		// TODO
	}

}

// Raw Log
void Log(const char *Message, ...)
{
	// Output Buffer
	char oBuffer[256];
	va_list ArgList;

	// Sanitize arguments
	if (Message == NULL) {
		return;
	}

	// Memset buffer
	memset(&oBuffer[0], 0, 256);

	// Format string
	va_start(ArgList, Message);
	vsprintf(oBuffer, Message, ArgList);
	va_end(ArgList);

	// Append newline
	strcat(oBuffer, "\n");

	// Print
	LogInternalPrint(LOG_TYPE_RAW, NULL, oBuffer);
}

// Raw Log
void LogRaw(const char *Message, ...)
{
	// Output Buffer
	char oBuffer[256];
	va_list ArgList;

	// Sanitize arguments
	if (Message == NULL) {
		return;
	}

	// Memset buffer
	memset(&oBuffer[0], 0, 256);

	/* Format string */
	va_start(ArgList, Message);
	vsprintf(oBuffer, Message, ArgList);
	va_end(ArgList);

	// Print
	LogInternalPrint(LOG_TYPE_RAW, NULL, oBuffer);
}

// Output information to log
void LogInformation(const char *System, const char *Message, ...)
{
	// Output Buffer
	char oBuffer[256];
	va_list ArgList;

	// Sanitize arguments
	if (System == NULL
		|| Message == NULL) {
		return;
	}

	// Memset buffer
	memset(&oBuffer[0], 0, 256);

	// Format string
	va_start(ArgList, Message);
	vsprintf(oBuffer, Message, ArgList);
	va_end(ArgList);

	// Print
	LogInternalPrint(LOG_TYPE_INFORMATION, System, oBuffer);
}

// Output debug to log
void LogDebug(const char *System, const char *Message, ...)
{
	// Output Buffer
	char oBuffer[256];
	va_list ArgList;

	// Sanitize arguments
	if (System == NULL
		|| Message == NULL) {
		return;
	}

	// Memset buffer
	memset(&oBuffer[0], 0, 256);

	// Format string
	va_start(ArgList, Message);
	vsprintf(oBuffer, Message, ArgList);
	va_end(ArgList);

	// Print
	LogInternalPrint(LOG_TYPE_DEBUG, System, oBuffer);
}

// Output Error to log
void LogFatal(const char *System, const char *Message, ...)
{
	// Output Buffer
	char oBuffer[256];
	va_list ArgList;

	// Sanitize arguments
	if (System == NULL
		|| Message == NULL) {
		return;
	}

	// Memset buffer
	memset(&oBuffer[0], 0, 256);

	// Format string
	va_start(ArgList, Message);
	vsprintf(oBuffer, Message, ArgList);
	va_end(ArgList);

	// Print
	LogInternalPrint(LOG_TYPE_FATAL, System, oBuffer);
}