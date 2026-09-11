/* Includes
 * - System */
#include <system/shell.h>
#include <system/threading.h>
#include <system/heap.h>
#include <system/timers.h>
#include <system/iospace.h>
#include <system/garbagecollector.h>
#include <system/modules.h>
#include <system/moduleloader.h>
#include <system/syscalls.h>
#include <ds/list.h>
#include <system/log.h>
#include <arch/x86/memory.h>
#include <arch/x86/x32/arch_x32.h>
#include <arch/x86/address_space.h>
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
    printf("  gc            garbage collector counters\n");
    printf("  vm            map/unmap test, shows frames being reclaimed\n");
    printf("  ls            list files on the ramdisk\n");
    printf("  cat <file>    print a file from the ramdisk\n");
    printf("  run <file>    load and run a module from the ramdisk\n");
    printf("  urun <file>   load and run a module in ring 3\n");
    printf("  exports       kernel symbols modules may call\n");
    printf("  sys           syscall counter\n");
    printf("  reap          return unused heap pages to the allocator\n");
    printf("  list          run the list self-test\n");
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
        printf("address spaces live: %u\n", AddressSpaceGetCount());
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
    else if (strcmp(Line, "ls") == 0) {
        uint32_t Count = ModulesGetCount();
        uint32_t i;

        if (Count == 0) {
            printf("no ramdisk loaded\n");
        }
        for (i = 0; i < Count; i++) {
            RamdiskEntry_t *Entry = ModulesGetEntry(i);
            printf("%-32s %u bytes%s\n", Entry->Name, Entry->Size,
                (ModulesVerify(Entry) == Success) ? "" : "  [BAD CHECKSUM]");
        }
    }
    else if (strncmp(Line, "cat", 3) == 0
             && (Line[3] == ' ' || Line[3] == '\0')) {
        const char *Name = ShellSkipSpaces(Line + 3);
        RamdiskEntry_t *Entry;

        if (*Name == '\0') {
            printf("cat: expected a filename\n");
        }
        else if ((Entry = ModulesFind(Name)) == NULL) {
            printf("cat: %s not found\n", Name);
        }
        else if (ModulesVerify(Entry) != Success) {
            printf("cat: %s failed its checksum, refusing\n", Name);
        }
        else {
            size_t Size = 0;
            const uint8_t *Data = ModulesGetData(Entry, &Size);
            size_t i;

            /* Byte at a time: the data is not null-terminated, so %s
             * would run off the end of the file into the next one. */
            for (i = 0; i < Size; i++) {
                printf("%c", (char)Data[i]);
            }
            if (Size > 0 && Data[Size - 1] != '\n') {
                printf("\n");
            }
        }
    }
    else if (strncmp(Line, "run", 3) == 0
             && (Line[3] == ' ' || Line[3] == '\0')) {
        const char *Name = ShellSkipSpaces(Line + 3);
        if (*Name == '\0') {
            printf("run: expected a module name\n");
        }
        else if (ModuleLoad(Name) != Success) {
            printf("run: %s could not be loaded\n", Name);
        }
    }
    else if (strncmp(Line, "urun", 4) == 0
             && (Line[4] == ' ' || Line[4] == '\0')) {
        const char *Name = ShellSkipSpaces(Line + 4);
        if (*Name == '\0') {
            printf("urun: expected a module name\n");
        }
        else if (ModuleLoadUser(Name) != Success) {
            printf("urun: %s could not be loaded\n", Name);
        }
    }
    else if (strcmp(Line, "sys") == 0) {
        printf("%u syscalls made\n", SyscallsGetCount());
    }
    else if (strcmp(Line, "exports") == 0) {
        ModuleLoaderPrintExports();
    }
    else if (strcmp(Line, "reap") == 0) {
        size_t Before = MmPhysicalGetBlocksUsed();
        size_t Pages = HeapReap(NULL);
        size_t After = MmPhysicalGetBlocksUsed();

        printf("reclaimed %u pages\n", Pages);
        printf("blocks used %u -> %u\n", Before, After);
    }
    else if (strcmp(Line, "gc") == 0) {
        printf("collected %u, dropped %u\n",
            GcGetCollected(), GcGetDropped());
    }
    else if (strcmp(Line, "vm") == 0) {
        /* Map a scratch page, write to it, unmap it, and check the
         * frame came back. Before MmVirtualUnmap existed the used count
         * only ever went up. */
        VirtualAddress_t Va = MmReserveMemory(1);
        PhysicalAddress_t Pa;
        size_t Before, After;

        if (Va == 0) {
            printf("vm: no reserved virtual space left\n");
        }
        else {
            Before = MmPhysicalGetBlocksUsed();
            Pa = MmPhysicalAllocateBlock(__MASK, 1);
            if (Pa == 0) {
                printf("vm: out of physical memory\n");
            }
            else if (MmVirtualMap(NULL, Pa, Va, 0) != Success) {
                printf("vm: map failed\n");
            }
            else {
                volatile uint32_t *Probe = (volatile uint32_t*)Va;
                *Probe = 0xC0FFEE;
                printf("mapped 0x%x -> 0x%x, read back 0x%x\n",
                    Va, Pa, *Probe);
                printf("resolved back to 0x%x\n",
                    MmVirtualGetMapping(NULL, Va));

                MmVirtualUnmap(NULL, Va, 1);
                After = MmPhysicalGetBlocksUsed();
                printf("blocks used %u -> %u -> %u\n",
                    Before, Before + 1, After);
                printf("mapping after unmap: 0x%x (expect 0)\n",
                    MmVirtualGetMapping(NULL, Va));
                printf("vm test: %s\n",
                    (After == Before) ? "PASS" : "FAIL - frame not reclaimed");
            }
        }
    }
    else if (strcmp(Line, "list") == 0) {
        List_t *L = ListCreate(KeyInteger, LIST_SAFE);
        DataKey_t Key;
        ListNode_t *Node;
        int Ok = 1;
        int i;

        if (L == NULL) {
            printf("list: out of memory\n");
        }
        else {
            for (i = 0; i < 5; i++) {
                Key.Value = i;
                ListAppend(L, ListCreateNode(Key, (void*)(uintptr_t)(i * 10)));
            }
            if (ListLength(L) != 5) { Ok = 0; }

            Key.Value = 3;
            Node = ListGetNodeByKey(L, Key);
            if (Node == NULL || (uintptr_t)Node->Data != 30) { Ok = 0; }

            if (ListRemoveByKey(L, Key) != Success) { Ok = 0; }
            if (ListLength(L) != 4) { Ok = 0; }
            if (ListGetNodeByKey(L, Key) != NULL) { Ok = 0; }

            Node = ListPopFront(L);
            if (Node == NULL || Node->Key.Value != 0) { Ok = 0; }
            ListDestroyNode(Node);
            if (ListLength(L) != 3) { Ok = 0; }

            /* order must survive the middle removal */
            i = 1;
            {
                ListNode_t *It;
                _foreach(It, L) {
                    if (It->Key.Value != i) { Ok = 0; }
                    i++;
                    if (i == 3) { i = 4; }   /* 3 was removed */
                }
            }

            ListDestroy(L);
            printf("list self-test: %s\n", Ok ? "PASS" : "FAIL");
        }
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