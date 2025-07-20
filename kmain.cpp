#include <stdint.h>
#include <boot/datastructure.h>
#include <terminal/terminal.h>
#include <video/video.h>

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
    // TerminalDrawPixel(&BootTerminal, 100, 100, 0x00ff0000);

    

    // unsigned short* vidMem = (unsigned short*)0xb8000;
    // unsigned char attribute = (1 << 4) | 15; // Blue background, white text
    
    // for(int i = 0; i <80*25; i++) {
    // 	vidMem[i] = (attribute << 8) | ' '; // Set each cell to a space with attribute.
    // }


    // Infinite loop
    while(1){}
    
}
