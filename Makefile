# Kernel Root Makefile

# Define ROOT_DIR before including config.mk
# Ensure ROOT_DIR is set, defaulting to one level up if not provided
ROOT_DIR ?= $(abspath .)

# Include the configuration file
include $(ROOT_DIR)/config.mk

# Define Build directory
# BUILD_DIR := build
BUILD_DIR = $(ROOT_DIR)/build

# Define directories
SUBDIRS =

# Define source files in the root directory
ROOT_ASM_SOURCES = kernel_entry.asm
ROOT_C_SOURCES = kmain.c

# Define object files in the root directory
ROOT_ASM_OBJECTS = $(ROOT_ASM_SOURCES:%.asm=$(BUILD_DIR)/%.o)
ROOT_C_OBJECTS = $(ROOT_C_SOURCES:%.c=$(BUILD_DIR)/%.o)
ROOT_OBJECTS = $(ROOT_ASM_OBJECTS) $(ROOT_C_OBJECTS)

# generate object files from source files
C_OBJECTS := $(patsubst %.c, %.o, $(C_SOURCES))
ASM_OBJECTS := $(ASM_SOURCES:.asm=.o)

# All object files
OBJECTS := $(C_OBJECTS) $(ASM_OBJECTS)

# Default target
all: $(BUILD_DIR) tools increase-build kernel


## Generates the ELF executable type file,
## Since the nasm -f elf32 option only generates Relocatable file.
## But our ELF loader is capable of parsing only Executable file
## for the time being.
#kernel_entry_executable: kernel_entry.elf
#	ld -m elf_i386 -T kernel.ld -o build/kernel.elf build/kernel_entry.elf


# Create build directory if not exists
$(BUILD_DIR):
	@echo "Building kernel build directory"
	mkdir -p $(BUILD_DIR)

# Recursively call make in each subdirectory
#$(SUBDIRS):
#	$(MAKE) -C $@ ROOT_DIR=$(ROOT_DIR)

# Compile root C sources
$(BUILD_DIR)/%.o: %.c
	@echo "Compiling $<..."
	$(CC) -m32 -c $< -o $@ $(CFLAGS) $(INCLUDES) -I $(KERNEL_INCLUDE) -I $(KERNEL_LIBC)

# Assemble root ASM sources
$(BUILD_DIR)/%.o: %.asm
	@echo "Assembling $<..."
	$(ASM) -f elf32 -I $(KERNEL_INCLUDE) $< -o $@

# Build kernel
kernel: $(SUBDIRS) $(ROOT_OBJECTS)
	@echo "Linking kernel..."
	$(LD) $(LDFLAGS) -T kernel.ld -o $(BUILD_DIR)/kernel.elf $(BUILD_DIR)/kernel_entry.o $(BUILD_DIR)/kmain.o

#kernel.bin: $(OBJECTS)
#	ld -m elf_i386 -Ttext 0xB000 --oformat binary -o build/$@ build/kernel_entry.o build/kernel_main.o build/idt.o build/idt_asm.o build/isr.o build/isr_asm.o build/vga.o build/kernel_utilities.o

# Rule to compile C files
#%.o: %.c
#	@echo $@
#	gcc  -m32 -fno-pie -ffreestanding -I $(KERNEL_INCLUDE) -c $< -o build/$(notdir $@)

#%.o: %.asm
#	nasm -f elf32 -I $(BOOT_STAGE2_INCLUDE) $< -o build/$(notdir $@)

tools:
	@echo "$(GREEN)Building kernel tools...$(RESET)"
	$(MAKE) -C tools ROOT_DIR=$(ROOT_DIR)

# Just increment the build version in every build
increase-build:
	@echo "$(CYAN)Incrementing kernel build version...$(RESET)"
	$(BUILD_DIR)/tools/revision/revision build gcc

# Run revision tool with parameters
## make run-revision ARGS="build gcc" // for increasing build version
## make run-revision ARGS="major gcc"
## make run-revision ARGS="minor gcc"
run-revision:
	@echo "$(CYAN)Running kernel revision tool...$(RESET)"
	$(BUILD_DIR)/tools/revision/revision $(ARGS)

## Make the kernel Multiboot iso and run it
grub:
	mkdir -p iso/boot/grub
	cp $(BUILD_DIR)/kernel.elf iso/boot/
	cp grub.cfg iso/boot/grub/
	grub-mkrescue -o $(BUILD_DIR)/TheKernel.iso iso
	qemu-system-i386 -cdrom $(BUILD_DIR)/TheKernel.iso -monitor stdio


# Clean up generated files
clean:
	rm -rf $(BUILD_DIR)
	rm -rf iso
#	for dir in $(SUBDIRS); do $(MAKE) -C $$dir ROOT_DIR=$(ROOT_DIR) clean; done

# Phony targets
.PHONY: all clean $(SUBDIRS) kernel grub tools

