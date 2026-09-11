/* Includes
 * - System
 */
#include <arch/x86/x32/arch_x32.h>
#include <boot/datastructure.h>
#include <system/log.h>
#include <arch/x86/memory.h>
#include <defs.h>

/* Includes
 * - C-Library
 */
#include <stdint.h>
#include <stddef.h>
#include <string.h>

/* Globals */
uintptr_t *MemoryBitmap = NULL;
size_t MemoryBitmapSize = 0;
size_t MemoryBlocks = 0;
size_t MemoryBlocksUsed = 0;
size_t MemorySize = 0;

/* Reserved Regions, from the region-descriptor */
SystemMemoryMapping_t SysMappings[32];

/* The lowest frame the allocator is allowed to return.
 * Frames below this hold the stage2 image, its VBE/e820 buffers and the
 * real-mode IVT/BDA. Nothing re-marks them as used, so the allocator has
 * to stay out of the way by construction.
 *
 * stage2 is linked at 0xCC00 and its .bss currently ends around 0x23580,
 * so 0x20000 was NOT enough - the first page-table allocation landed on
 * top of it. Re-check this if stage2 grows:
 *     nm bootloader/build/stage2/stage2.elf | sort | tail -1          */
#define MEMORY_ALLOC_FLOOR      0x30000

/* MmMemoryDebugPrint */
void MmMemoryDebugPrint(void)
{
	LogInformation("Physical_Memory", "Bitmap size: %u Bytes", MemoryBitmapSize);
	LogInformation("Physical_Memory", "Memory in use %u Bytes", MemoryBlocksUsed * PAGE_SIZE);
	LogInformation("Physical_Memory", "Block status %u/%u", MemoryBlocksUsed, MemoryBlocks);
}

/* Bit helpers. All of them are bounds-checked now: MmFreeRegion used to
 * be able to walk off the end of the bitmap when the e820 map described
 * more memory than MemorySize claimed. */
static int MmMemoryMapValidBit(size_t Bit) {
	return (Bit < MemoryBlocks) ? 1 : 0;
}

void MmMemoryMapSetBit(int Bit) {
	if (!MmMemoryMapValidBit((size_t)Bit)) return;
	MemoryBitmap[Bit / __BITS] |= (1u << (Bit % __BITS));
}

void MmMemoryMapUnsetBit(int Bit) {
	if (!MmMemoryMapValidBit((size_t)Bit)) return;
	MemoryBitmap[Bit / __BITS] &= ~(1u << (Bit % __BITS));
}

int MmMemoryMapTestBit(int Bit) {
	if (!MmMemoryMapValidBit((size_t)Bit)) return 1;
	return (MemoryBitmap[Bit / __BITS] & (1u << (Bit % __BITS))) ? 1 : 0;
}

/* MmGetFreeBlocks
 * Finds <Count> consecutive free frames in [FirstBlock, LastBlock).
 * Unlike the old MmGetFreeMapBitLow/High this correctly handles runs
 * that straddle a 32-bit word boundary. Returns -1 on failure. */
static int MmGetFreeBlocks(int Count, size_t FirstBlock, size_t LastBlock)
{
	size_t i, j;

	if (Count <= 0) {
		return -1;
	}
	if (LastBlock > MemoryBlocks) {
		LastBlock = MemoryBlocks;
	}
	if (FirstBlock + (size_t)Count > LastBlock) {
		return -1;
	}

	for (i = FirstBlock; i + (size_t)Count <= LastBlock; i++) {
		/* Fast skip: a fully allocated word can be jumped over */
		if ((i % __BITS) == 0 && MemoryBitmap[i / __BITS] == __MASK) {
			i += __BITS - 1;
			continue;
		}
		if (MmMemoryMapTestBit((int)i)) {
			continue;
		}

		for (j = 1; j < (size_t)Count; j++) {
			if (MmMemoryMapTestBit((int)(i + j))) {
				break;
			}
		}
		if (j == (size_t)Count) {
			return (int)i;
		}
		i += j; /* skip the block that broke the run */
	}

	return -1;
}

/* MmFreeRegion
 * Marks a region free. The bounds are rounded *inward* so a partial
 * page at either end stays reserved rather than being handed out. */
void MmFreeRegion(uintptr_t Base, size_t Size)
{
	uintptr_t Start = (Base + PAGE_SIZE - 1) & PAGE_MASK;
	uintptr_t End   = (Base + Size) & PAGE_MASK;
	uintptr_t Address;

	for (Address = Start; Address < End; Address += PAGE_SIZE) {
		int Frame = (int)(Address / PAGE_SIZE);
		if (MmMemoryMapTestBit(Frame) && MemoryBlocksUsed != 0) {
			MemoryBlocksUsed--;
		}
		MmMemoryMapUnsetBit(Frame);
	}
}

/* MmAllocateRegion
 * Marks a region used. Bounds are rounded *outward* so a partially
 * covered page is fully reserved. */
void MmAllocateRegion(uintptr_t Base, size_t Size)
{
	uintptr_t Start = Base & PAGE_MASK;
	uintptr_t End   = (Base + Size + PAGE_SIZE - 1) & PAGE_MASK;
	uintptr_t Address;

	for (Address = Start; Address < End; Address += PAGE_SIZE) {
		int Frame = (int)(Address / PAGE_SIZE);
		if (!MmMemoryMapTestBit(Frame)) {
			MemoryBlocksUsed++;
		}
		MmMemoryMapSetBit(Frame);
	}
}

/* MmSysMappingsContain */
int MmSysMappingsContain(uintptr_t Base, int Type)
{
	for (int i = 0; i < 32; i++) {
		if (SysMappings[i].Length == 0)
			continue;
		if (SysMappings[i].Type == Type && SysMappings[i].pAddressStart == Base) {
			return 1;
		}
	}
	return 0;
}

/* MmPhysicalInit */
OsStatus_t MmPhysicalInit(void *BootInfo, BootDescriptor_t *Descriptor)
{
	LogInformation("Physical_Memory", "KernelAddress=0x%x KernelSize=0x%x",
    	Descriptor->KernelAddress, Descriptor->KernelSize);

	Multiboot_t *BootDesc = (Multiboot_t*)BootInfo;
	BIOSMemoryRegion_t *RegionItr = NULL;
	int i, j;

	RegionItr = (BIOSMemoryRegion_t*)BootDesc->MemoryMapAddr;

	/* MemoryHigh is in 64kB blocks above 16mB.
	 * MemoryLow is in *kilobytes* - the bootloader stores
	 * (int15/E801 AX + 0x400) there. It was being added as bytes. */
	MemorySize  = (size_t)BootDesc->MemoryHigh * 64 * 1024;
	MemorySize += (size_t)BootDesc->MemoryLow * 1024;

	LogInformation("Physical_Memory", "Low Memory  = %u KB", BootDesc->MemoryLow);
	LogInformation("Physical_Memory", "High Memory = %u bytes", BootDesc->MemoryHigh * 64 * 1024);
	LogInformation("Physical_Memory", "Total Memory = %u bytes", MemorySize);

	MemoryBitmap     = (uintptr_t*)MEMORY_LOCATION_BITMAP;
	MemoryBlocks     = MemorySize / PAGE_SIZE;
	MemoryBlocksUsed = MemoryBlocks;
	MemoryBitmapSize = DIVUP(MemoryBlocks, 8);

	LogInformation("Physical_Memory", "MemoryBlocks = %u", MemoryBlocks);
	LogInformation("Physical_Memory", "MemoryBitmapSize = %u", MemoryBitmapSize);

	/* The bitmap must fit inside the region we identity map at boot */
	if ((MEMORY_LOCATION_BITMAP + MemoryBitmapSize) >= MEMORY_LOCATION_HEAP) {
		LogFatal("Physical_Memory", "bitmap does not fit its reserved window");
		return Error;
	}

	memset((void*)MemoryBitmap, 0xFF, MemoryBitmapSize);
	memset((void*)SysMappings, 0, sizeof(SysMappings));

	SysMappings[0].Type = 2;
	SysMappings[0].pAddressStart = 0;
	SysMappings[0].vAddressStart = 0;
	SysMappings[0].Length = PAGE_SIZE;

	for (i = 0, j = 1; i < (int)BootDesc->MemoryMapLength && j < 32; i++) {
		if (!MmSysMappingsContain((PhysicalAddress_t)RegionItr->Address, (int)RegionItr->Type))
		{
			if (RegionItr->Type == 1)
				MmFreeRegion((uintptr_t)RegionItr->Address, (size_t)RegionItr->Size);

			LogInformation("Physical_Memory", "Region %u: Address: %x, Size %x",
				RegionItr->Type, (PhysicalAddress_t)RegionItr->Address,
				(size_t)RegionItr->Size);

			SysMappings[j].Type = RegionItr->Type;
			SysMappings[j].pAddressStart = (PhysicalAddress_t)RegionItr->Address;
			SysMappings[j].vAddressStart = 0;
			SysMappings[j].Length = (size_t)RegionItr->Size;
			j++;
		}
		RegionItr++;
	}

	/* Null page */
	MmAllocateRegion(0, PAGE_SIZE);

	/* 0x4000 - 0x6000 and 0x9000 - 0xB000: memory map & trampoline */
	MmAllocateRegion(0x4000, 0x2000);
	MmAllocateRegion(0x9000, 0x2000);

	/* 0x90000 - 0x9F000: kernel stack */
	MmAllocateRegion(0x90000, 0xF000);

	/* Kernel image.
	 * NOTE: Descriptor->KernelSize is the size of kernel.elf *on disk*
	 * (g_kernelSize in vfs.c), not the size of the loaded image. It
	 * happens to be larger for this kernel because of the section
	 * headers, but that is luck, not a guarantee - if you ever strip
	 * the ELF this reservation can end up smaller than .bss. */
	MmAllocateRegion(MEMORY_LOCATION_KERNEL, Descriptor->KernelSize + PAGE_SIZE);

	/* Bitmap space + a guard page */
	MmAllocateRegion(MEMORY_LOCATION_BITMAP, MemoryBitmapSize + PAGE_SIZE);

	/* The ramdisk, if the bootloader loaded one. Without this the
	 * allocator would happily hand out the frames the image is sitting
	 * in - and the corruption would only show up whenever something
	 * next read a file, long after the allocation that caused it. */
	if (Descriptor->RamDiskAddress != 0 && Descriptor->RamDiskSize != 0) {
		LogInformation("Physical_Memory", "Reserving ramdisk 0x%x + 0x%x",
			Descriptor->RamDiskAddress, Descriptor->RamDiskSize);
		MmAllocateRegion(Descriptor->RamDiskAddress,
			Descriptor->RamDiskSize + PAGE_SIZE);
	}

	MmMemoryDebugPrint();
	return Success;
}

/* MmPhysicalFreeBlock */
OsStatus_t MmPhysicalFreeBlock(PhysicalAddress_t Address)
{
	int Frame = (int)(Address / PAGE_SIZE);

	if (!MmMemoryMapValidBit((size_t)Frame)) {
		return Error;
	}

	MmMemoryMapUnsetBit(Frame);
	if (MemoryBlocksUsed != 0) {
		MemoryBlocksUsed--;
	}
	return Success;
}

/* MmPhysicalFreeBlocks
 * Releases <Count> consecutive frames.
 *
 * The counterpart to MmPhysicalAllocateBlock's Count argument. Without
 * it, anything allocated as a run had to be freed one frame at a time
 * by hand - and the page-directory allocation, three blocks, was being
 * released as one. That leaked two frames per address space, invisibly,
 * because the address space itself was accounted for correctly. */
OsStatus_t MmPhysicalFreeBlocks(PhysicalAddress_t Address, int Count)
{
	int Frame = (int)(Address / PAGE_SIZE);
	int i;

	if (Count <= 0) {
		return Error;
	}

	for (i = 0; i < Count; i++) {
		if (!MmMemoryMapValidBit((size_t)(Frame + i))) {
			return Error;
		}
		MmMemoryMapUnsetBit(Frame + i);
		if (MemoryBlocksUsed != 0) {
			MemoryBlocksUsed--;
		}
	}

	return Success;
}

/* MmPhysicalAllocateBlock
 * Allocates <Count> consecutive frames. <Mask> is now actually honoured:
 * it is the highest physical address the caller can accept.
 * Returns 0 on failure (frame 0 is permanently reserved, so 0 is safe
 * to use as the error value). */
PhysicalAddress_t MmPhysicalAllocateBlock(uintptr_t Mask, int Count)
{
	size_t FirstBlock = MEMORY_ALLOC_FLOOR / PAGE_SIZE;
	size_t LastBlock;
	int Frame;

	if (Count <= 0) {
		return 0;
	}

	/* Mask is inclusive, so +1 gives the exclusive limit */
	LastBlock = ((size_t)Mask / PAGE_SIZE) + 1;
	if (LastBlock > MemoryBlocks) {
		LastBlock = MemoryBlocks;
	}

	Frame = MmGetFreeBlocks(Count, FirstBlock, LastBlock);

	/* Fall back to the whole of memory when the caller's window is full,
	 * but only for masks that were not deliberately restrictive. */
	if (Frame == -1 && Mask >= __MASK) {
		Frame = MmGetFreeBlocks(Count, FirstBlock, MemoryBlocks);
	}

	if (Frame == -1) {
		LogFatal("Physical_Memory", "out of memory: %d blocks under 0x%x",
			Count, Mask);
		return 0;
	}

	for (int i = 0; i < Count; i++) {
		MmMemoryMapSetBit(Frame + i);
	}
	MemoryBlocksUsed += (size_t)Count;

	return (PhysicalAddress_t)((uintptr_t)Frame * PAGE_SIZE);
}

/* MmPhyiscalGetSysMappingVirtual */
VirtualAddress_t MmPhyiscalGetSysMappingVirtual(PhysicalAddress_t PhysicalAddress)
{
	for (int i = 0; i < 32; i++) {
		if (SysMappings[i].Length != 0 && SysMappings[i].Type != 1)
		{
			PhysicalAddress_t Start = SysMappings[i].pAddressStart;
			PhysicalAddress_t End = SysMappings[i].pAddressStart + SysMappings[i].Length;

			if (PhysicalAddress >= Start && PhysicalAddress < End) {
				return SysMappings[i].vAddressStart
					+ (PhysicalAddress - SysMappings[i].pAddressStart);
			}
		}
	}
	return 0;
}
/* MmPhysicalGetBlocksUsed / MmPhysicalGetBlocksTotal
 * Exposed so callers can verify that frames actually come back. */
size_t MmPhysicalGetBlocksUsed(void)
{
	return MemoryBlocksUsed;
}

size_t MmPhysicalGetBlocksTotal(void)
{
	return MemoryBlocks;
}