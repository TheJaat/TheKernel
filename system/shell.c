/* Includes
 * - System */
#include <system/shell.h>
#include <system/threading.h>
#include <system/heap.h>
#include <system/timers.h>
#include <system/iospace.h>
#include <system/log.h>
#include <arch/x86/memory.h>
#include <driver/ps2_keyboard.h>
#include <video/interface/video_interface.h>
#include <terminal/terminal.h>

/* Includes
 * - Library */
#include <stddef.h>
#include <stdio.h>
#include <string.h>

#define SHELL_PROMPT                "taaj> "

static int GlbShellRunning = 0;

/* ShellSkipSpaces */
static const char *ShellSkipSpaces(const char *Text)
{
    while (*Text == ' ' || *Text == '\t') {
        Text++;
    }
    return Text;
}

/* ShellParseUInt
 * Small decimal parser - there is no atoi in this libc. Sets *Ok to 0
 * when the text holds no digits, so "sleep abc" is rejected rather than
 * silently sleeping for zero. */
static size_t ShellParseUInt(const char *Text, int *Ok)
{
    size_t Value = 0;
    int Digits = 0;

    Text = ShellSkipSpaces(Text);

    while (*Text >= '0' && *Text <= '9') {
        Value = (Value * 10) + (size_t)(*Text - '0');
        Text++;
        Digits++;
    }

    if (Ok != NULL) {
        *Ok = (Digits > 0) ? 1 : 0;
    }
    return Value;
}

/* ShellCommandHelp */
static void ShellCommandHelp(void)
{
    printf("Commands:\n");
    printf("  help          this list\n");
    printf("  ps            thread table\n");
    printf("  mem           physical memory and heap statistics\n");
    printf("  io            registered io-spaces\n");
    printf("  uptime        milliseconds since the timer started\n");
    printf("  kb            keyboard counters\n");
    printf("  echo <text>   print text back\n");
    printf("  sleep <ms>    block this shell for a while\n");
    printf("  spawn <n>     start n short-lived worker threads\n");
    printf("  clear         clear the screen\n");
    printf("  fault         dereference NULL, to see the fault report\n");
}

/* ShellWorker
 * Used by 'spawn' so the scheduler is doing something visible. */
static void ShellWorker(void *Args)
{
    size_t Index = (size_t)Args;

    SleepMs(200 * (Index + 1));
    printf("[worker %u done at %u ms]\n", Index, TimersGetSystemMs());
}

/* ShellExecute */
static void ShellExecute(char *Line)
{
    const char *Args;

    Line = (char*)ShellSkipSpaces(Line);
    if (*Line == '\0') {
        return;
    }

    if (strcmp(Line, "help") == 0) {
        ShellCommandHelp();
    }
    else if (strcmp(Line, "ps") == 0) {
        ThreadingPrint();
    }
    else if (strcmp(Line, "mem") == 0) {
        MmMemoryDebugPrint();
        HeapPrintStats(NULL);
    }
    else if (strcmp(Line, "io") == 0) {
        IoSpacePrint();
    }
    else if (strcmp(Line, "uptime") == 0) {
        printf("up %u ms (%u ticks)\n",
            TimersGetSystemMs(), TimersGetSystemTicks());
    }
    else if (strcmp(Line, "kb") == 0) {
        printf("scancodes %u, dropped %u\n",
            Ps2KeyboardGetScancodes(), Ps2KeyboardGetDropped());
    }
    else if (strcmp(Line, "clear") == 0) {
        TerminalClear(VideoGetTerminal());
    }
    else if (strncmp(Line, "echo", 4) == 0
             && (Line[4] == ' ' || Line[4] == '\0')) {
        Args = ShellSkipSpaces(Line + 4);
        printf("%s\n", Args);
    }
    else if (strncmp(Line, "sleep", 5) == 0
             && (Line[5] == ' ' || Line[5] == '\0')) {
        int Ok = 0;
        size_t Ms = ShellParseUInt(Line + 5, &Ok);
        if (!Ok) {
            printf("sleep: expected a number of milliseconds\n");
        }
        else {
            printf("sleeping %u ms...\n", Ms);
            SleepMs(Ms);
            printf("awake at %u ms\n", TimersGetSystemMs());
        }
    }
    else if (strncmp(Line, "spawn", 5) == 0
             && (Line[5] == ' ' || Line[5] == '\0')) {
        int Ok = 0;
        size_t Count = ShellParseUInt(Line + 5, &Ok);
        size_t i;

        if (!Ok || Count == 0) {
            Count = 3;
        }
        if (Count > 8) {
            Count = 8;      /* the thread table is only 32 deep */
        }
        for (i = 0; i < Count; i++) {
            if (ThreadingCreateThread("shell-w", ShellWorker, (void*)i, 0)
                == UUID_INVALID) {
                printf("spawn: could not create worker %u\n", i);
                break;
            }
        }
        printf("spawned %u workers\n", Count);
    }
    else if (strcmp(Line, "fault") == 0) {
        volatile int *Null = (volatile int*)0;
        printf("dereferencing NULL...\n");
        *Null = 1;
        printf("(not reached)\n");
    }
    else {
        printf("unknown command: %s\n", Line);
        printf("try 'help'\n");
    }
}

/* ShellThread */
static void ShellThread(void *Args)
{
    Pipe_t *Input = (Pipe_t*)Args;
    char Line[SHELL_LINE_MAX];
    size_t Length = 0;
    uint8_t Character;

    printf("\nTheTaaJ shell. Type 'help'.\n");
    printf(SHELL_PROMPT);

    for (;;) {
        /* One byte at a time. PipeRead blocks, so this thread costs
         * nothing at all while nobody is typing. */
        if (PipeRead(Input, &Character, 1, 0) != 1) {
            continue;
        }

        if (Character == '\n') {
            printf("\n");
            Line[Length] = '\0';
            ShellExecute(Line);
            Length = 0;
            printf(SHELL_PROMPT);
            continue;
        }

        if (Character == '\b') {
            /* Only erase when there is something to erase, or the cursor
             * walks back over the prompt. */
            if (Length > 0) {
                Length--;
                printf("\b");
            }
            continue;
        }

        /* Drop anything unprintable rather than putting control codes on
         * the screen. */
        if (Character < 0x20 || Character > 0x7E) {
            continue;
        }

        if (Length >= (SHELL_LINE_MAX - 1)) {
            /* Full. Dropping is better than truncating silently - a
             * command that quietly loses its tail is worse than one that
             * refuses to grow. */
            continue;
        }

        Line[Length++] = (char)Character;
        printf("%c", (char)Character);
    }
}

/* ShellStart */
OsStatus_t ShellStart(Pipe_t *Input)
{
    if (Input == NULL) {
        LogFatal("Shell", "no input pipe");
        return Error;
    }
    if (GlbShellRunning) {
        return Success;
    }

    if (ThreadingCreateThread("shell", ShellThread, Input, 0)
        == UUID_INVALID) {
        LogFatal("Shell", "could not start the shell thread");
        return Error;
    }

    GlbShellRunning = 1;
    return Success;
}