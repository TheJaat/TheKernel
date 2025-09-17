# arch/x86/x32/Makefile

# Ensure ROOT_DIR is set, defaulting to one level up if not provided
ROOT_DIR ?= $(abspath .)

# Include the configuration file
include $(ROOT_DIR)/config.mk

# Define directories
BUILD_DIR ?= $(ROOT_DIR)/arch/x86/x32
SUBDIRS =

# Define source files
ASM_SOURCES = gdt_asm.asm tss_asm.asm idt_asm.asm irq.asm paging.asm
C_SOURCES = gdt.c idt.c virtual_memory.c
CPP_SOURCES =

REL_DIR := $(patsubst $(ROOT_DIR)/%,%,$(CURDIR))
LIB_NAME := lib$(subst /,_,$(REL_DIR)).a
LIB_DIR := $(ROOT_DIR)/build/lib
LIB_FILE := $(LIB_DIR)/$(LIB_NAME)

# Define object files
ASM_OBJECTS = $(ASM_SOURCES:%.asm=$(BUILD_DIR)/%.o)
C_OBJECTS = $(C_SOURCES:%.c=$(BUILD_DIR)/%.o)
CPP_OBJECTS = $(CPP_SOURCES:%.cpp=$(BUILD_DIR)/%.o)
OBJECTS = $(ASM_OBJECTS) $(C_OBJECTS) $(CPP_OBJECTS)

# Build all
all: $(BUILD_DIR) $(SUBDIRS) $(OBJECTS) lib

lib: $(LIB_DIR)
	@echo "→ Creating static library: $(LIB_FILE)"
	$(AR) rcs $(LIB_FILE) $(OBJECTS)

$(LIB_DIR):
	mkdir -p $(LIB_DIR)

# Recursively call make in each subdirectory
$(SUBDIRS):
	$(MAKE) -C $@ ROOT_DIR=$(ROOT_DIR)

# Compile Cpp sources
$(BUILD_DIR)/%.o: %.cpp
	@echo "Compiling $<..."
	$(info KERNEL_INCLUDE=$(KERNEL_INCLUDE))
	$(info KERNEL_LIBC=$(KERNEL_LIBC))
	$(info KERNEL_DRIVER=$(KERNEL_DRIVER))

# 	$(CXX) -I$(KERNEL_INCLUDE) -I $(KERNEL_LIBC) -c $< -o $@ $(CXX_FLAGS) -O2 -Wall -Wextra
	$(CXX) -I$(KERNEL_INCLUDE) -I$(KERNEL_LIBC) -I$(KERNEL_DRIVER) -c $< -o $@ $(CXX_FLAGS) -O2 -Wall -Wextra

# Compile C sources
$(BUILD_DIR)/%.o: %.c
	@echo "Compiling $<..."
	$(info KERNEL_INCLUDE=$(KERNEL_INCLUDE))
	$(info KERNEL_LIBC=$(KERNEL_LIBC))
	$(info KERNEL_DRIVER=$(KERNEL_DRIVER))

# 	$(CXX) -I$(KERNEL_INCLUDE) -I $(KERNEL_LIBC) -c $< -o $@ $(CXX_FLAGS) -O2 -Wall -Wextra
	$(CC) -I$(KERNEL_INCLUDE) -I$(KERNEL_LIBC) -I$(KERNEL_DRIVER) -c $< -o $@ $(CXX_FLAGS) -O2 -Wall -Wextra


# Assemble ASM sources
$(BUILD_DIR)/%.o: %.asm
	@echo "Assembling $<..."
	$(ASM) -f elf $< -o $@

# Create build directory if not exists
$(BUILD_DIR):
	mkdir -p $(BUILD_DIR)

# Clean build directory
clean:
	rm -f $(OBJECTS)
	for dir in $(SUBDIRS); do $(MAKE) -C $$dir ROOT_DIR=$(ROOT_DIR) clean; done

# Phony targets
.PHONY: all clean lib $(SUBDIRS)
