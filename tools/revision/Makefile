# Makefile for kernel/tools/revision

.PHONY: all clean

# Ensure ROOT_DIR is set, defaulting to one level up if not provided
ROOT_DIR ?= $(abspath ..)

# Include the configuration file
include $(ROOT_DIR)/config.mk

# Define Build Directory
BUILD_DIR = $(ROOT_DIR)/build/tools
REVISION_DIR = $(BUILD_DIR)/revision

SRC_DIR = .
SRC = $(wildcard $(SRC_DIR)/*.c)  # Finds all .asm files in the current directory
OBJ = $(patsubst $(SRC_DIR)/%.c, $(REVISION_DIR)/%.o, $(SRC))
# BIN = $(patsubst $(SRC_DIR)/%.asm, $(STAGE1_DIR)/%.bin, $(SRC))

# Final binary name
BIN = $(REVISION_DIR)/revision

# Compiler settings
CC = gcc
CFLAGS = -Wall -Wextra -O2 -ffreestanding
LDFLAGS = 

# Target to build all binaries
all: $(BIN)

# Rule to create the stage1 directory if it doesn't exist
$(REVISION_DIR):
	@echo "$(CYAN)Creating kernel revision directory...$(RESET)"
	mkdir -p $(REVISION_DIR)

# Rule to build each binary
$(REVISION_DIR)/%.o: $(SRC_DIR)/%.c | $(REVISION_DIR) 
	@echo "$(GREEN)Compiling $< to $@...$(RESET)"
	gcc -c $< -o $@

$(BIN): $(OBJ) | $(REVISION_DIR)
	@echo "$(GREEN)Linking $@...$(RESET)"
	$(CC) $(LDFLAGS) $^ -o $@



# Target to clean the output
clean:
	# TODO: Clean stage1 directory and other outputs
	@echo "$(YELLOW)Cleaning up the kernel/tools/build directory...$(RESET)"
	rm -rf $(REVISION_DIR)

