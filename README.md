# TheKernel
The Kernel for the `TheTaaJ` OS.

## Current Status
- [x] Modular and scalable Logging Terminal

## How to Build and Run
### Requirements:
- **NASM**: An assembler for x86 programs.
- **CROSS Compiler**: x86 cross compiler.
- **QEMU**: An emulator to test the kernel.

### Build Steps:
1. Clone the repository:
   ```bash
   git clone https://github.com/TheJaat/TheKernel
   cd TheKernel
   ```
3. Build the binary using the Makefile:
   ```bash
   make
   ```
3. Run the Kernel in QEMU:
   ```bash
   make run
   ```
4. Clean Up:
   ```bash
   make clean
   ```

## License
This project is open-source and available under the MIT License.
