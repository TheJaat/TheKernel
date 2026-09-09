#include <stdint.h>
#include <boot/datastructure.h>
#include <terminal/terminal.h>
#include <video/video.h>

#include <stdio.h>
#include <system/log.h>

// GDT
#include <arch/x86/x32/gdt.h>
// IDT
#include <arch/x86/x32/idt.h>
// PIC
#include <arch/x86/pic.h>
// Physical Memory && Virtual Memory
#include <arch/x86/memory.h>
// Kernel Heap
#include <system/heap.h>
// Interrupt manager
#include <interrupts/interrupts.h>
// Timers
#include <system/timers.h>
#include <arch/x86/pit8253.h>
#include <system/iospace.h>
// Threading
#include <system/threading.h>
#include <system/scheduler.h>
#include <system/mutex.h>
#include <system/semaphore.h>
#include <system/pipe.h>
#include <driver/ps2_keyboard.h>
#include <system/shell.h>
#include <system/garbagecollector.h>
#include <string.h>

BootInfo_t x86BootInfo;

const char* kernelInfo = "TheTaaJKernel Version 0.0.1, Author: TheJat";

/* InterruptSelfTest
 * Registers a handler on a free software vector and triggers it with an
 * int instruction. This exercises the whole dispatch path - IDT gate,
 * irq_common stub, InterruptEntry, table lookup, handler call - without
 * needing a device or interrupts to be enabled, since a software int is
 * not gated by EFLAGS.IF. Delete once the timer is up. */
static volatile int GlbSelfTestHits = 0;

extern "C" InterruptStatus_t InterruptSelfTestHandler(void *Data)
{
    (void)Data;
    GlbSelfTestHits++;
    return InterruptHandled;
}

static void InterruptSelfTest(void)
{
    Interrupt_t Irq;
    UUId_t Id;

    memset(&Irq, 0, sizeof(Interrupt_t));
    for (int i = 0; i < INTERRUPT_MAXVECTORS; i++) {
        Irq.Vectors[i] = INTERRUPT_NONE;
    }
    Irq.Vectors[0]   = 0xF0;
    Irq.Line         = INTERRUPT_NONE;
    Irq.Pin          = INTERRUPT_NONE;
    Irq.FastHandler  = InterruptSelfTestHandler;
    Irq.Data         = (void*)&GlbSelfTestHits;   // non-NULL: handler gets this

    Id = InterruptRegister(&Irq, INTERRUPT_KERNEL | INTERRUPT_SOFTWARE);
    if (Id == UUID_INVALID) {
        LogFatal("kmain", "self-test could not register vector 0xF0");
        return;
    }

    __asm__ volatile ("int $0xF0");
    __asm__ volatile ("int $0xF0");

    LogInformation("kmain", "interrupt self-test: handler ran %d times (expected 2)",
        GlbSelfTestHits);

    if (InterruptUnregister(Id) != Success) {
        LogFatal("kmain", "self-test could not unregister");
    }
    else {
        LogInformation("kmain", "interrupt self-test: unregistered cleanly");
    }
}

/* TimerSelfTest
 * Fires the PIT for a second and checks that the tick counter, the ms
 * counter and a periodic software timer all advance. Delete once you
 * trust it. */
static volatile int GlbTimerCallbackHits = 0;

static void TimerSelfTestCallback(void *Args)
{
    (void)Args;
    GlbTimerCallbackHits++;
}

static void TimerSelfTest(void)
{
    UUId_t Id;
    size_t StartTicks, StartMs;

    Id = TimersCreateTimer(TimerSelfTestCallback, NULL, TimerPeriodic, 100);
    if (Id == UUID_INVALID) {
        LogFatal("kmain", "could not create a periodic timer");
        return;
    }

    StartTicks = TimersGetSystemTicks();
    StartMs    = TimersGetSystemMs();

    DelayMs(1000);

    LogInformation("kmain", "timer self-test: %u ticks, %u ms elapsed",
        TimersGetSystemTicks() - StartTicks, TimersGetSystemMs() - StartMs);
    LogInformation("kmain", "timer self-test: 100ms callback ran %d times (expect ~10)",
        GlbTimerCallbackHits);

    TimersDestroyTimer(Id);
    LogInformation("kmain", "timer self-test: destroyed, raw pit ticks = %u",
        PitGetTicks());
}

/* IoSpaceSelfTest
 * Checks registration, overlap rejection, the acquire/release lock and
 * bounds checking. Uses a harmless unused port range. Delete once you
 * trust it. */
static void IoSpaceSelfTest(void)
{
    DeviceIoSpace_t A, B, C;

    memset(&A, 0, sizeof(A));
    A.Type = IO_SPACE_IO; A.PhysicalBase = 0x0278; A.Size = 8;  // LPT2, unused
    if (IoSpaceRegister(&A) != Success) {
        LogFatal("kmain", "io-space self-test: register failed");
        return;
    }

    // Overlapping range must be rejected.
    memset(&B, 0, sizeof(B));
    B.Type = IO_SPACE_IO; B.PhysicalBase = 0x027C; B.Size = 8;
    if (IoSpaceRegister(&B) == Success) {
        LogFatal("kmain", "io-space self-test: overlap was NOT rejected");
    }

    // Adjacent but non-overlapping must be accepted.
    memset(&C, 0, sizeof(C));
    C.Type = IO_SPACE_IO; C.PhysicalBase = 0x0280; C.Size = 8;
    if (IoSpaceRegister(&C) != Success) {
        LogFatal("kmain", "io-space self-test: adjacent range was rejected");
    }

    if (IoSpaceAcquire(&A) != Success) {
        LogFatal("kmain", "io-space self-test: acquire failed");
    }
    // Double acquire must fail.
    if (IoSpaceAcquire(&A) == Success) {
        LogFatal("kmain", "io-space self-test: double acquire succeeded");
    }
    // Destroy while acquired must fail.
    if (IoSpaceDestroy(A.Id) == Success) {
        LogFatal("kmain", "io-space self-test: destroyed while acquired");
    }

    IoSpaceRelease(&A);
    if (IoSpaceDestroy(A.Id) != Success
        || IoSpaceDestroy(C.Id) != Success) {
        LogFatal("kmain", "io-space self-test: cleanup failed");
    }

    LogInformation("kmain", "io-space self-test: passed");
}

/* ThreadSelfTest
 * Spawns three threads. Two sleep at different intervals and log, the
 * third exits immediately so the zombie reaping path gets exercised.
 * Delete once you trust it. */
static volatile int GlbWorkerTicks[2] = { 0, 0 };

static void ThreadWorker(void *Args)
{
    int Index = (int)(uintptr_t)Args;
    int i;

    for (i = 0; i < 5; i++) {
        SleepMs(200 + (Index * 100));
        GlbWorkerTicks[Index]++;
        LogInformation("worker", "thread %d woke, count %d at %u ms",
            Index, GlbWorkerTicks[Index], TimersGetSystemMs());
    }
}

static void ThreadShortLived(void *Args)
{
    (void)Args;
    LogInformation("worker", "short-lived thread ran and is exiting");
}

static void ThreadSelfTest(void)
{
    ThreadingCreateThread("worker0", ThreadWorker, (void*)0, 0);
    ThreadingCreateThread("worker1", ThreadWorker, (void*)1, 0);
    ThreadingCreateThread("brief", ThreadShortLived, NULL, 0);

    ThreadingPrint();

    // The boot thread sleeps too, so everything has to be scheduled -
    // if the switch is broken this never returns.
    //
    // 1500 was too tight: worker1 does 5 x 300ms and does not finish
    // until ~1520ms after it starts, so the check below ran while its
    // last iteration was still asleep and reported 4. Leave margin for
    // sleep granularity and scheduling latency.
    SleepMs(2200);

    LogInformation("kmain", "thread self-test: worker0 %d, worker1 %d (expect 5, 5)",
        GlbWorkerTicks[0], GlbWorkerTicks[1]);
    ThreadingPrint();
}

/* SyncSelfTest
 * Producer/consumer over a semaphore, plus a mutex guarding a shared
 * counter that two threads hammer. If the mutex is broken the counter
 * ends up short; if the semaphore is broken the consumer stalls.
 * Delete once you trust it. */
static Semaphore_t GlbTestSem;
static Mutex_t GlbTestMutex;
static volatile int GlbShared = 0;
static volatile int GlbConsumed = 0;
static volatile int GlbProducersDone = 0;

static void SyncProducer(void *Args)
{
    int i;
    (void)Args;

    for (i = 0; i < 20; i++) {
        // Contended increments. Without the mutex, two read-modify-writes
        // across a preemption lose one.
        MutexLock(&GlbTestMutex);
        GlbShared++;
        MutexUnlock(&GlbTestMutex);

        SemaphoreV(&GlbTestSem, 1);
        SleepMs(10);
    }

    MutexLock(&GlbTestMutex);
    GlbProducersDone++;
    MutexUnlock(&GlbTestMutex);
}

static void SyncConsumer(void *Args)
{
    (void)Args;

    for (;;) {
        if (SemaphoreP(&GlbTestSem, 500) != Success) {
            break;      // timed out, producers must be finished
        }
        GlbConsumed++;
    }
    LogInformation("sync", "consumer stopped after %d items", GlbConsumed);
}

static void SyncSelfTest(void)
{
    SemaphoreConstruct(&GlbTestSem, 0);
    MutexConstruct(&GlbTestMutex);
    GlbShared = 0; GlbConsumed = 0; GlbProducersDone = 0;

    ThreadingCreateThread("prod0", SyncProducer, NULL, 0);
    ThreadingCreateThread("prod1", SyncProducer, NULL, 0);
    ThreadingCreateThread("cons",  SyncConsumer, NULL, 0);

    SleepMs(1200);

    LogInformation("kmain", "sync self-test: shared=%d (expect 40), consumed=%d (expect 40)",
        GlbShared, GlbConsumed);
    LogInformation("kmain", "sync self-test: producers done=%d (expect 2)",
        GlbProducersDone);

    // Recursion must not deadlock.
    MutexLock(&GlbTestMutex);
    MutexLock(&GlbTestMutex);
    MutexUnlock(&GlbTestMutex);
    MutexUnlock(&GlbTestMutex);
    LogInformation("kmain", "sync self-test: recursive lock/unlock survived");
}

/* StartShell
 * Brings up the keyboard and hands its pipe to the shell. The echo
 * thread from the last step is gone - the shell owns the display now,
 * which stops keystrokes interleaving with log lines mid-line. */
static void StartShell(void)
{
    Pipe_t *Pipe;

    if (Ps2KeyboardInitialize() != Success) {
        LogFatal("kmain", "keyboard failed to initialize");
        return;
    }

    Pipe = Ps2KeyboardGetPipe();
    if (Pipe == NULL) {
        return;
    }

    if (ShellStart(Pipe) != Success) {
        LogFatal("kmain", "shell failed to start");
    }
}

extern "C" void kmain(Multiboot_t* BootInfo, BootDescriptor_t* bootDescriptor) {

    // Store the passed arguments value in global data structure
    x86BootInfo.ArchBootInfo = (void*)BootInfo;
	x86BootInfo.BootLoaderName = (char*)BootInfo->BootLoaderName;

	x86BootInfo.Descriptor.KernelAddress = bootDescriptor->KernelAddress;
	x86BootInfo.Descriptor.KernelSize = bootDescriptor->KernelSize;
	x86BootInfo.Descriptor.RamDiskAddress = bootDescriptor->RamDiskAddress;
	x86BootInfo.Descriptor.RamDiskSize = bootDescriptor->RamDiskSize;
	x86BootInfo.Descriptor.ExportsAddress = bootDescriptor->ExportsAddress;
	x86BootInfo.Descriptor.ExportsSize = bootDescriptor->ExportsSize;
	x86BootInfo.Descriptor.SymbolsAddress = bootDescriptor->SymbolsAddress;
	x86BootInfo.Descriptor.SymbolsSize = bootDescriptor->SymbolsSize;

    InitializeVideo(BootInfo);
    // TerminalScroll(&BootTerminal, 5);
    TerminalDrawString(&BootTerminal, x86BootInfo.BootLoaderName);
    TerminalDrawString(&BootTerminal, "\n");
    TerminalDrawString(&BootTerminal, kernelInfo);
    printf("Testing printf\n");

    LogInit();
    int abc = 7;
    int def = 9;
    int ghi = 8;
    Log("This is from logger %p, - %p - %p\n", &abc, &def, &ghi);
    Log("Kmain address = %p", &kmain);
    LogDebug("kmain", "Log debug");

    LogFatal("kmain", "Testing fatal log");

    for(int i = 1; i < 60; i++) {
        LogDebug("kmain", "i = %d", i);
    }

    // Initialize the GDT
	GdtInitialize();

    // Initialize the IDT
	IdtInitialize();
	// Generate the division by zero exception.
	int a = 0;
	int b = 1;
	// int result = b/a;

    // Generate the interrupt manually, for it uncomment below lines.
	// asm volatile (
	// "int $0x35"
	// :
	// :
	// :
	// );

    // Initialize and Remap the PIC
	PicInit();

    // Initialize Physical Memory
	MmPhysicalInit(BootInfo, bootDescriptor);
    
    // Initialize the Virtual Memory
    MmVirtualInit();

    LogDebug("kmain", "After virtual memory initialization");

    // Initialize the kernel heap. Must come after MmVirtualInit, since it
    // maps its pages on demand through MmVirtualMap.
    if (HeapInit() == Success) {
        // The first memory operation: move the log off the small static
        // boot buffer and onto the heap.
        LogUpgrade(LOG_PREFFERED_SIZE);
    }

    // The interrupt manager needs the heap, since every registration
    // allocates a descriptor.
    InterruptInitialize();
    InterruptSelfTest();

    // The timer registry has to exist before any tick source registers
    // itself, so this comes before PitInitialize.
    TimersInitialize();

    // Drivers claim their register ranges through this, so it has to be
    // up before the first one initializes.
    IoSpaceInitialize();
    IoSpaceSelfTest();

    if (PitInitialize(1000) == Success) {
        // Nothing is delivered until now - every PIC line was masked and
        // EFLAGS.IF was clear. Registering the PIT unmasked IRQ 0; this
        // is what actually lets it through.
        // Threading has to be up before interrupts are enabled: the
        // first tick can preempt, and it needs somewhere to go.
        SchedulerInit(0);
        if (ThreadingInitialize(0) != Success) {
            LogFatal("kmain", "threading failed to initialize");
        }

        InterruptEnable();
        LogInformation("kmain", "interrupts enabled");
        // Callbacks can leave interrupt context now that threads exist.
        TimersStartWorker();

        TimerSelfTest();
        ThreadSelfTest();
        SyncSelfTest();

        // Deferred cleanup. Once this is up, exiting threads are reaped
        // by the collector instead of being polled for by idle.
        if (GcInitialize() == Success) {
            ThreadingEnableGc();
        }

        StartShell();
    }

    // TerminalDrawPixel(&BootTerminal, 100, 100, 0x00ff0000);

    

    // unsigned short* vidMem = (unsigned short*)0xb8000;
    // unsigned char attribute = (1 << 4) | 15; // Blue background, white text
    
    // for(int i = 0; i <80*25; i++) {
    // 	vidMem[i] = (attribute << 8) | ' '; // Set each cell to a space with attribute.
    // }


    // Infinite loop
    while(1){}
    
}