//! The serial port: an Arm PL011 UART.
//!
//! The PL011 is the serial port Arm designed and nearly every AArch64
//! board has (QEMU's virt machine, the Raspberry Pi, most servers). A
//! PC's serial port is reached through I/O ports - a second, small
//! address space with its own instructions, `in` and `out`. AArch64
//! has no such thing: a device's registers are at memory addresses,
//! and are read and written like memory. That is "memory-mapped I/O".
//!
//! "Like memory" with one difference: these are not variables. Writing
//! the data register sends a byte each time; reading the flag register
//! gives a new answer each time. `read_volatile` and `write_volatile`
//! tell the compiler exactly that - do every one of these accesses, in
//! this order, and do not reason about what the "value" must be.

use core::sync::atomic::{AtomicUsize, Ordering};

// Registers, as byte offsets from the device's base address.
const DATA: usize = 0x00;
const FLAGS: usize = 0x18;
const BAUD_INTEGER: usize = 0x24;
const BAUD_FRACTION: usize = 0x28;
const LINE_CONTROL: usize = 0x2C;
const CONTROL: usize = 0x30;
const INTERRUPT_MASK: usize = 0x38;
const INTERRUPT_CLEAR: usize = 0x44;

/// Flag: the transmit queue is full; a byte written now would be lost.
const TRANSMIT_FULL: u32 = 1 << 5;
/// Flag: a byte is still on its way out.
const BUSY: u32 = 1 << 3;

/// Where the registers are, as the kernel sees them; 0 until `init`.
static BASE: AtomicUsize = AtomicUsize::new(0);

fn read(register: usize) -> u32 {
    let base = BASE.load(Ordering::Relaxed);
    // SAFETY: `init` was given the mapped address of a PL011, whose
    // registers are 32 bits wide at these offsets.
    unsafe { ((base + register) as *const u32).read_volatile() }
}

fn write(register: usize, value: u32) {
    let base = BASE.load(Ordering::Relaxed);
    // SAFETY: as for `read`.
    unsafe { ((base + register) as *mut u32).write_volatile(value) }
}

/// Sets the port up: 115200 baud, 8 data bits, no parity, one stop bit,
/// no interrupts - the same line as COM1 on the PC.
///
/// `clock_hz` is the clock the device counts from; the baud rate is
/// that clock divided by 16 times a divisor, and the divisor has a
/// whole part and a part in 64ths.
///
/// # Safety
/// `registers` must be the mapped address of a PL011, and nothing else
/// may be using it.
pub unsafe fn init(registers: *mut u8, clock_hz: u32) {
    BASE.store(registers as usize, Ordering::Relaxed);

    // Let a byte the loader was sending finish, then switch off: the
    // manual says not to change the line settings while it is running.
    while read(FLAGS) & BUSY != 0 {
        core::hint::spin_loop();
    }
    write(CONTROL, 0);

    // The divisor times 64, rounded: clock * 64 / (16 * baud).
    let divisor_64ths = (clock_hz * 4 + 115_200 / 2) / 115_200;
    write(BAUD_INTEGER, divisor_64ths >> 6);
    write(BAUD_FRACTION, divisor_64ths & 0x3F);
    write(LINE_CONTROL, 0b11 << 5 | 1 << 4); // 8 bits; queues on
    write(INTERRUPT_MASK, 0);
    write(INTERRUPT_CLEAR, 0x7FF);
    write(CONTROL, 1 | 1 << 8 | 1 << 9); // on; transmit; receive
}

fn write_byte(byte: u8) {
    while read(FLAGS) & TRANSMIT_FULL != 0 {
        core::hint::spin_loop();
    }
    write(DATA, u32::from(byte));
}

pub fn write_bytes(bytes: &[u8]) {
    if BASE.load(Ordering::Relaxed) == 0 {
        return; // nowhere to write yet
    }
    for &byte in bytes {
        // Terminals expect "\r\n"; Rust strings use "\n".
        if byte == b'\n' {
            write_byte(b'\r');
        }
        write_byte(byte);
    }
}
