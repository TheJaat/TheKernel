# Compiler and tools
ASM = nasm
LD = i686-elf-ld #x86_64-ld #ld
CC = i686-elf-gcc #x86_64-elf-gcc #gcc

# Flags
STAGE1_ASFLAGS = -f bin
STAGE2_ASFLAGS = -f elf32
LDFLAGS = -m elf_i386

# GCC
GCC_FLAGS = -m32 -fno-pie -ffreestanding -nostdlib

## Header includes
# Common
BOOT_COMMON_INCLUDE = headers/boot/
# Stage1
BOOT_STAGE1_INCLUDE = headers/boot/stage1/
# Stage2
BOOT_STAGE2_ASM_INCLUDE = headers/boot/stage2/asm_includes/
BOOT_STAGE2_C_INCLUDE = headers/boot/stage2/c_includes/
BOOT_STAGE2_C_STD_INCLUDE = headers/boot/stage2/c_includes/std/

KERNEL_INCLUDE = headers/
KERNEL_LIBC = $(KERNEL_INCLUDE)/libc

# Common Directories
OUTPUT_DIR = ../../build


# Color variables
BLACK = \033[30m
RED = \033[31m
GREEN = \033[32m
YELLOW = \033[33m
BLUE = \033[34m
MAGENTA = \033[35m
CYAN = \033[36m
WHITE = \033[37m

LIGHT_BLUE = \033[38;5;45m
BRIGHT_YELLOW = \033[38;5;226m
LIGHT_CYAN = \033[38;5;51m
BOLD_BLUE = \033[1;34m
RESET = \033[0m

