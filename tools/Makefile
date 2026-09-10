# Makefile for kernel/tools/

.PHONY: all clean revision ramdisk

# Ensure ROOT_DIR is set, defaulting to one level up if not provided
ROOT_DIR ?= $(abspath ..)

# Include the configuration file
include $(ROOT_DIR)/config.mk

# Define Build Directory
Build_DIR = $(ROOT_DIR)/build

# Subdirectories
SUBDIRS = revision ramdisk

# Target to build everything
all: revision ramdisk

# Build the revision tool
revision:
	#@for dir in $(SUBDIRS); do\
	#	echo "$(MAGENTA)Entering Directory $$dir...$(RESET)";\
	#	$(MAKE) -C $$dir ROOT_DIR=$(ROOT_DIR) || { \
	#		echo "$(RED)Error: Make failed in $$dir!$(RESET)"; \
	#		exit 1; \
	#	} \
	#done
	@echo "$(GREEN)Building kernel revision tool...$(RESET)"
	$(MAKE) -C revision ROOT_DIR=$(ROOT_DIR)

# Build the ramdisk packing tool
ramdisk:
	@echo "$(GREEN)Building kernel ramdisk tool...$(RESET)"
	$(MAKE) -C ramdisk ROOT_DIR=$(ROOT_DIR)

# Target to clean everything
clean:
	@echo "$(YELLOW)Cleaning kernel tools directory...$(RESET)"
	rm -rf $(BUILD_DIR)