#include <system/log.h>


#include <stddef.h>
#include <stdio.h>
#include <string.h>

#include <video/interface/video_interface.h>

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

/* Upgrades the log : TODO
 * with a larger buffer */
// void LogUpgrade(size_t Size)
// {
// 	/* Allocate */
// 	char *nBuffer = (char*)kmalloc(Size);

// 	/* Zero it */
// 	memset(nBuffer, 0, Size);

// 	/* Copy current buffer */
// 	memcpy(nBuffer, GlbLog, GlbLogIndex);

// 	/* Free the old if not initial */
// 	if (GlbLog != &GlbLogStatic[0])
// 		kfree(GlbLog);

// 	/* Update */
// 	GlbLog = nBuffer;
// 	GlbLogSize = Size;
// }

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
			char Length = GlbLog[Index + 1];

			/* Zero buffer */
			memset(TempBuffer, 0, 256);

			/* What kind of line is this? */
			if (Type == LOG_TYPE_RAW)
			{
				/* Copy data */
				memcpy(TempBuffer, &GlbLog[Index + 2], (size_t)Length);

				/* Flush it */
				// VideoGetTerminal()->FgColor = LOG_COLOR_DEFAULT;
				printf("%s", (const char*)&TempBuffer[0]);

				/* Increase */
				Index += 2 + Length;
			}
			else 
			{
				/* We have two chunks to print */
				char *StartPtr = &GlbLog[Index + 2];
				char *StartMsgPtr = strchr(StartPtr, ' ');
				int HeaderLen = (int)StartMsgPtr - (int)StartPtr;

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
				memset(TempBuffer, 0, HeaderLen + 1);

				/* Increament */
				Index += 2 + HeaderLen + 1;

				/* Copy data */
				memcpy(TempBuffer, &GlbLog[Index], (size_t)Length);

				/* Sanity */
				if (Type != LOG_TYPE_FATAL)
					// VideoGetTerminal()->FgColor = LOG_COLOR_DEFAULT;

				/* Finally, flush */
				printf("%s", (const char*)&TempBuffer[0]);

				/* Restore */
				// VideoGetTerminal()->FgColor = LOG_COLOR_DEFAULT;

				/* Increase again */
				Index += Length;
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
	char TempBuffer[256];
	int HeaderLen = strlen(Header);
	int MessageLen = strlen(Message);


	// Log it into memory - if we have room
	if (GlbLogIndex + MessageLen < (int)GlbLogSize)
	{
		/* Write header */
		GlbLog[GlbLogIndex] = (char)LogType;
		GlbLog[GlbLogIndex + 1] = (char)MessageLen;

		/* Increase */
		GlbLogIndex += 2;

		if (LogType != LOG_TYPE_RAW)
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
			// VideoGetTerminal()->FgColor = LOG_COLOR_DEFAULT;

		/* Print */
		if (LogType == LOG_TYPE_RAW)
			printf("%s", Message);
		else
			printf("%s\n", Message);

		/* Restore */
		// VideoGetTerminal()->FgColor = LOG_COLOR_DEFAULT;
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
