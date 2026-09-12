# kernel/system/process/Makefile
ROOT_DIR ?= $(abspath ../../..)
include $(ROOT_DIR)/config.mk

BUILD_DIR = $(ROOT_DIR)/build/system/process
LIB_DIR   = $(ROOT_DIR)/build/lib
LIBRARY   = $(LIB_DIR)/libprocess.a

C_SOURCES   = process.c syscalls.c
C_OBJECTS   = $(C_SOURCES:%.c=$(BUILD_DIR)/%.o)

all: $(BUILD_DIR) $(C_OBJECTS) lib

$(BUILD_DIR):
	mkdir -p $(BUILD_DIR)

$(BUILD_DIR)/%.o: %.c
	@echo "Compiling $<..."
	$(CC) $(KERNEL_INCLUDES) -c $< -o $@ $(GCC_FLAGS) -O2 -Wall -Wextra

lib: $(C_OBJECTS)
	mkdir -p $(LIB_DIR)
	@echo "Creating static library: $(LIBRARY)"
	$(AR) rcs $(LIBRARY) $(C_OBJECTS)

clean:
	rm -f $(C_OBJECTS) $(LIBRARY)

.PHONY: all clean lib
