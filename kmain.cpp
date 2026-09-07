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
    if (PitInitialize(1000) == Success) {
        // Nothing is delivered until now - every PIC line was masked and
        // EFLAGS.IF was clear. Registering the PIT unmasked IRQ 0; this
        // is what actually lets it through.
        InterruptEnable();
        LogInformation("kmain", "interrupts enabled");
        TimerSelfTest();
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