# arch/Makefile

# Ensure ROOT_DIR is set, defaulting to one level up if not provided
ROOT_DIR ?= $(abspath .)

# Include the configuration file
include $(ROOT_DIR)/config.mk

# Define directories
BUILD_DIR = $(ROOT_DIR)/build/arch
SUBDIRS = x86 # Add your subdirectories here if any

# Define source files
C_SOURCES =

# Define object files
C_OBJECTS = $(C_SOURCES:%.c=$(BUILD_DIR)/%.o)

# Build all
all: $(BUILD_DIR) $(SUBDIRS) 
#$(C_OBJECTS)

# Recursively call make in each subdirectory
## Pass the Root diretory and the build directory
$(SUBDIRS):
	$(MAKE) -C $@ ROOT_DIR=$(ROOT_DIR) BUILD_DIR=$(BUILD_DIR)/$@

# Compile C sources
$(BUILD_DIR)/%.o: %.c
	@echo "Compiling $<..."
	$(CC) -I$(KERNEL_INCLUDE) -I$(LIBC_INCLUDE) -c $< -o $@ $(CFLAGS) -O2 -Wall -Wextra

# Create build directory if not exists
$(BUILD_DIR):
	mkdir -p $(BUILD_DIR)

# Clean build directory
clean:
	rm -f $(C_OBJECTS)
	for dir in $(SUBDIRS); do $(MAKE) -C $$dir ROOT_DIR=$(ROOT_DIR) clean; done

# Phony targets
.PHONY: all clean $(SUBDIRS)
