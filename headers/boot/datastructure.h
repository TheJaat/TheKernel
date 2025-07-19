#ifndef __BOOT_DATA_STRUCTURE_H__
#define __BOOT_DATA_STRUCTURE_H__

#include <stdint.h>

typedef struct MultibootInfo
{
	uint32_t Flags;
	uint32_t MemoryLow;
	uint32_t MemoryHigh;
	uint32_t BootDevice;
	uint32_t CmdLine;
	uint32_t ModuleCount;
	uint32_t ModuleAddr;
	
	union
	{
		struct
		{
			/* (a.out) Kernel Symbol table info */
			uint32_t TabSize;
			uint32_t StrSize;
			uint32_t Addr;
			uint32_t Pad;
		} A;
		struct
		{
			/* (ELF) Kernel section header table */
			uint32_t Num;
			uint32_t Size;
			uint32_t Addr;
			uint32_t Shndx;
		} Elf;
	} Symbols;

	/* Memory Mappings */
	uint32_t MemoryMapLength;
	uint32_t MemoryMapAddr;

	/* Drive Info */
	uint32_t DrivesLength;
	uint32_t DrivesAddr;
	
	/* ROM Configuration Table */
	uint32_t ConfigTable;
	
	/* BootLoader Name */
	uint32_t BootLoaderName;

	/* APM Table */
	uint32_t ApmTable;

	/* Video Info */
	uint32_t VbeControllerInfo;
	uint32_t VbeModeInfo;
	uint32_t VbeMode;
	uint32_t VbeInterfaceSegment;
	uint32_t VbeInterfaceOffset;
	uint32_t VbeInterfaceLength;

} __attribute__((packed)) Multiboot_t;


/* This structure is passed by stage 2 
 * to the kernel */
typedef struct BootDescriptor
{
	/* Kernel Information */
	uint32_t KernelAddress;
	uint32_t KernelSize;

	/* Ramdisk Information */
	uint32_t RamDiskAddress;
	uint32_t RamDiskSize;

	/* Exports */
	uint32_t ExportsAddress;
	uint32_t ExportsSize;

	/* Symbols */
	uint32_t SymbolsAddress;
	uint32_t SymbolsSize;

} __attribute__((packed)) BootDescriptor_t;


typedef struct CoreBootInfo {
	char			*BootLoaderName;
	BootDescriptor_t	Descriptor;
	void			*ArchBootInfo;
} __attribute__((packed)) BootInfo_t;


#endif /* __BOOT_DATA_STRUCTURE_H__*/