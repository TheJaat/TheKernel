# kernel/tools/ramdisk/Makefile
#
# HOST tool: built with the system gcc, never the cross compiler. It runs
# on the build machine to pack the ramdisk image, so cross-compiling it
# would produce a binary make cannot execute.

ROOT_DIR ?= $(abspath ../..)
BUILD_DIR = $(ROOT_DIR)/build/tools/ramdisk

HOST_CC = gcc
HOST_CFLAGS = -Wall -Wextra -O2

all: $(BUILD_DIR)/ramdisk

$(BUILD_DIR):
	@echo "Creating ramdisk tool directory..."
	mkdir -p $(BUILD_DIR)

$(BUILD_DIR)/ramdisk: rd.c ramdisk.h | $(BUILD_DIR)
	@echo "Building ramdisk tool..."
	$(HOST_CC) $(HOST_CFLAGS) -I. -o $@ rd.c

clean:
	rm -f $(BUILD_DIR)/ramdisk

.PHONY: all clean