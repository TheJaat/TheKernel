//! Switching the machine off and resetting it: what the kernel hands on.
//!
//! The kernel does not switch the machine off itself. It finds out how
//! this machine is switched off, and gives exactly that - and nothing
//! more - to init, which gives it to a program, `power`, that does it
//! when init asks. The same pattern as a disk: the kernel knows where
//! the controls are, a program in user space knows what to do with them.
//!
//! On a PC the controls are ACPI's: I/O ports, named by the FADT, and
//! values to write to them, in the DSDT. The values are AML - ACPI's
//! byte code, which the firmware writes and which may be anything - so
//! they are `power`'s to read, not the kernel's: a mistake in reading
//! them costs a crashed program, not a crashed kernel. The kernel reads
//! only the FADT (fixed fields, like the MADT it already reads), and
//! hands on: the DSDT as read-only memory, and a handle to each port
//! the FADT names for this - the PM1 control registers, the SMI command
//! port (which switches the chipset into ACPI mode), and the reset
//! register (or 0xCF9, the PC's reset control register, if the FADT
//! names none). `power` cannot touch any other port.
//!
//! On AArch64 the controls are PSCI calls, SYSTEM_OFF and SYSTEM_RESET,
//! and only exception level 1 can make them (`hvc` and `smc` are not
//! allowed at level 0). So the kernel hands on a power handle, which
//! lets its holder make those two calls and nothing else
//! (abi::call::POWER).

use abi::{power, PowerInfo};
use kernel64::cap::Rights;

use crate::ipc::{self, Object};

/// The PC's reset control register: writing 0x06 to it resets the
/// whole machine ("hard reset", CPU and chipset). Every PC chipset
/// since the 1990s has it.
#[cfg(target_arch = "x86_64")]
const RESET_CONTROL: (u16, u8) = (0xCF9, 0x06);

/// Reads the FADT and the DSDT, and sends init a "power" message with
/// their ports and memory. Says why not, if it cannot.
#[cfg(target_arch = "x86_64")]
pub fn offer(to_init: &Object, rsdp: u64) {
    use alloc::sync::Arc;
    use kernel64::acpi::{read_table, Acpi, Fadt};
    use kernel64::paging::Flags;

    use crate::memory_object::MemoryObject;

    let memory = crate::mm::DirectMap;
    let found = Acpi::new(&memory, rsdp)
        .and_then(|acpi| acpi.find(&memory, b"FACP"))
        .and_then(|table| Fadt::parse(&table))
        .and_then(|fadt| Ok((fadt, read_table(&memory, fadt.dsdt)?)));
    let (fadt, dsdt) = match found {
        Ok((fadt, dsdt)) if &dsdt.signature() == b"DSDT" => (fadt, dsdt),
        Ok(_) => return kprintln!("[acpi] power: the FADT's DSDT is not a DSDT - no power control"),
        Err(error) => return kprintln!("[acpi] power: {error} - no power control"),
    };
    let (reset_port, reset_value) = fadt.reset.unwrap_or(RESET_CONTROL);
    let info = PowerInfo {
        kind: power::ACPI,
        dsdt: dsdt.address,
        dsdt_length: dsdt.bytes.len() as u32,
        pm1a_control: fadt.pm1a_control,
        pm1b_control: fadt.pm1b_control,
        pm1_control_length: fadt.pm1_control_length,
        smi_command: if fadt.acpi_enable == 0 { 0 } else { fadt.smi_command },
        acpi_enable: fadt.acpi_enable,
        reset_port,
        reset_value,
        reset_from_fadt: fadt.reset.is_some(),
    };
    // The DSDT: firmware memory, ordinary RAM, read only - the handle
    // carries READ and not WRITE, so it can be mapped only so, by
    // whoever it is passed on to (init copies it for `power`: DUPLICATE
    // and TRANSFER, which copies keep no more of than this).
    let table = MemoryObject::device(dsdt.address, dsdt.bytes.len() as u64, Flags::NONE);
    let ports = |port: u16, width: u8| Object::IoPorts(u32::from(port)..u32::from(port) + u32::from(width));
    let read = Rights(abi::rights::READ | abi::rights::DUPLICATE | abi::rights::TRANSFER);
    let mut handles = alloc::vec![
        (Object::Memory(Arc::new(table)), read),
        (ports(info.pm1a_control, info.pm1_control_length), Rights::ALL),
    ];
    if info.pm1b_control != 0 {
        handles.push((ports(info.pm1b_control, info.pm1_control_length), Rights::ALL));
    }
    if info.smi_command != 0 {
        handles.push((ports(info.smi_command, 1), Rights::ALL));
    }
    handles.push((ports(info.reset_port, 1), Rights::ALL));
    debug_assert_eq!(handles.len(), info.handles());

    let mut message = b"power".to_vec();
    message.extend_from_slice(&info.to_bytes());
    match ipc::kernel_send(to_init, &message, handles) {
        Ok(()) => kprintln!(
            "[acpi] power: PM1a control {:#x}{}, reset {:#x} <- {:#x}{}, DSDT {} bytes at {:#x} - handed to init",
            info.pm1a_control,
            if info.pm1b_control != 0 { alloc::format!(", PM1b control {:#x}", info.pm1b_control) } else { alloc::string::String::new() },
            info.reset_port,
            info.reset_value,
            if info.reset_from_fadt { "" } else { " (the PC's; the FADT names none)" },
            info.dsdt_length,
            info.dsdt,
        ),
        Err(error) => kprintln!("[acpi] power: could not hand it to init: {error:?}"),
    }
}

/// The instruction the firmware is called with: 0 none yet, 1 hvc,
/// 2 smc. Set once, when the power handle is made.
#[cfg(target_arch = "aarch64")]
static CALL: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// Sends init a "power" message with a power handle, if the device tree
/// names a way to call the firmware.
#[cfg(target_arch = "aarch64")]
pub fn offer(to_init: &Object, call: Option<kernel64::devicetree::FirmwareCall>) {
    use core::sync::atomic::Ordering;
    use kernel64::devicetree::FirmwareCall;

    let Some(call) = call else {
        return kprintln!("[psci] power: the device tree names no firmware calls - no power control");
    };
    CALL.store(if call == FirmwareCall::Hvc { 1 } else { 2 }, Ordering::Relaxed);
    let info = PowerInfo { kind: power::PSCI, ..PowerInfo::default() };
    let mut message = b"power".to_vec();
    message.extend_from_slice(&info.to_bytes());
    match ipc::kernel_send(to_init, &message, alloc::vec![(Object::Power, Rights::ALL)]) {
        Ok(()) => kprintln!("[psci] power: SYSTEM_OFF and SYSTEM_RESET - handed to init"),
        Err(error) => kprintln!("[psci] power: could not hand it to init: {error:?}"),
    }
}

/// The POWER system call: what the holder of a power handle asks. Only
/// returns if the firmware refused (or there is no firmware to ask).
pub fn system_power(handle: kernel64::cap::Handle, action: u64) -> Result<u64, abi::Error> {
    ipc::require_power(handle)?;
    let reset = match u8::try_from(action) {
        Ok(power::OFF) => false,
        Ok(power::RESET) => true,
        _ => return Err(abi::Error::InvalidArgument),
    };
    firmware_power(reset)
}

#[cfg(target_arch = "aarch64")]
fn firmware_power(reset: bool) -> Result<u64, abi::Error> {
    use core::sync::atomic::Ordering;
    use kernel64::devicetree::FirmwareCall;

    let call = match CALL.load(Ordering::Relaxed) {
        1 => FirmwareCall::Hvc,
        2 => FirmwareCall::Smc,
        _ => return Err(abi::Error::InvalidArgument),
    };
    kprintln!("[psci] {}", if reset { "SYSTEM_RESET" } else { "SYSTEM_OFF" });
    let answer = crate::arch::smp::system_power(call, reset);
    // Still here: the firmware said no.
    kprintln!("[psci] the firmware refused: {:?}", kernel64::psci::cpu_on_result(answer));
    Err(abi::Error::InvalidArgument)
}

/// A PC has no power handles: its power is ACPI's ports, in user space.
#[cfg(target_arch = "x86_64")]
fn firmware_power(_reset: bool) -> Result<u64, abi::Error> {
    Err(abi::Error::InvalidArgument)
}
