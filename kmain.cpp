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
// Physical Memory
#include <arch/x86/memory.h>
// Heap
#include <heap.h>

BootInfo_t x86BootInfo;

const char* kernelInfo = "TheTaaJKernel Version 0.0.1, Author: TheJat";

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
	MmPhyiscalInit(BootInfo, bootDescriptor);

    // Initialize Virtual Memory
	// MmVirtualInit();

	// Initializing the Heap Memory
	// HeapInit();

    // TerminalDrawPixel(&BootTerminal, 100, 100, 0x00ff0000);

    

    // unsigned short* vidMem = (unsigned short*)0xb8000;
    // unsigned char attribute = (1 << 4) | 15; // Blue background, white text
    
    // for(int i = 0; i <80*25; i++) {
    // 	vidMem[i] = (attribute << 8) | ' '; // Set each cell to a space with attribute.
    // }


    // Infinite loop
    while(1){}
    
}
