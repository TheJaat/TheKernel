# system/Makefile

# Ensure ROOT_DIR is set, defaulting to one level up if not provided
ROOT_DIR ?= $(abspath .)

# Include the configuration file
include $(ROOT_DIR)/config.mk

# Define directories
BUILD_DIR = $(ROOT_DIR)/build/system
SUBDIRS = # Add your subdirectories here if any

# Define source files
C_SOURCES = heap.c
CPP_SOURCES = log.cpp

REL_DIR := $(patsubst $(ROOT_DIR)/%,%,$(CURDIR))
LIB_NAME := lib$(subst /,_,$(REL_DIR)).a
LIB_DIR := $(ROOT_DIR)/build/lib
LIB_FILE := $(LIB_DIR)/$(LIB_NAME)

# Define object files
C_OBJECTS = $(C_SOURCES:%.c=$(BUILD_DIR)/%.o)
CPP_OBJECTS = $(CPP_SOURCES:%.cpp=$(BUILD_DIR)/%.o)
OBJECTS = $(C_OBJECTS) $(CPP_OBJECTS)

# Build all
all: $(BUILD_DIR) $(SUBDIRS) $(CPP_OBJECTS) $(C_OBJECTS) lib

lib: $(LIB_DIR)
	@echo "→ Creating static library: $(LIB_FILE)"
	$(AR) rcs $(LIB_FILE) $(OBJECTS)

$(LIB_DIR):
	mkdir -p $(LIB_DIR)

# Recursively call make in each subdirectory
## Pass the Root diretory and the build directory
$(SUBDIRS):
	$(MAKE) -C $@ ROOT_DIR=$(ROOT_DIR) BUILD_DIR=$(BUILD_DIR)/$@

# Compile C sources
$(BUILD_DIR)/%.o: %.c
	@echo "Compiling $<..."
	$(CC) -I$(KERNEL_INCLUDE) -I$(KERNEL_LIBC) -I$(KERNEL_DRIVER) -c $< -o $@ $(CFLAGS) -O2 -Wall -Wextra

$(BUILD_DIR)/%.o: %.cpp
	@echo "Compiling $<..."
	$(CXX) -I$(KERNEL_INCLUDE) -I$(KERNEL_LIBC) -I$(KERNEL_DRIVER) -c $< -o $@ $(CXX_FLAGS) -O2 -Wall -Wextra

# Create build directory if not exists
$(BUILD_DIR):
	mkdir -p $(BUILD_DIR)

# Clean build directory
clean:
	rm -f $(C_OBJECTS)
	for dir in $(SUBDIRS); do $(MAKE) -C $$dir ROOT_DIR=$(ROOT_DIR) clean; done

# Phony targets
.PHONY: all clean lib $(SUBDIRS)
