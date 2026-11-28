//! TheTaaJ - the 64-bit kernel.
//!
//! The bootloader puts the CPU in long mode and jumps here with a
//! TaajBootInfo. The kernel takes over the CPU tables, then memory: its
//! own page tables, a frame allocator and a heap. Threads, IPC and
//! capabilities get rebuilt on top of this, in Rust.
//!
//! Two crates share this directory:
//!   lib.rs (kernel64::)  pure logic - boot info, frames, page tables,
//!                        heap, locks - tested on the host by `cargo test`
//!   main.rs (this)       the kernel proper:
//!     arch/        the only code that knows which CPU it is on
//!     console      kprint!/kprintln!, over whatever the arch provides
//!     mm           memory management wired to the real machine
//!     selftest     checks that wiring at every boot
//!     framebuffer  drawing on the screen the bootloader set up

// No standard library: there is no OS underneath to provide one. `core`
// (types, traits, formatting) is still available - it needs nothing.
#![no_std]
// No `fn main`: nothing calls it. The entry point is `_start`, in
// arch/x86_64/entry.rs, which the linker script names.
#![no_main]
// The port to AArch64 is under way. Everything marked
// `cfg(target_arch = "x86_64")` in this file has not crossed yet; the
// mark comes off a module when AArch64 has what that module needs. On
// AArch64 that leaves parts of the shared modules unused for now, which
// is not a mistake to warn about.
#![cfg_attr(target_arch = "aarch64", allow(dead_code))]

// Box, Vec, String and friends - backed by the heap in mm.rs.
extern crate alloc;

#[macro_use]
mod console; // first, so its macros are visible in every later module

mod alarms;
mod arch;
mod clock;
#[cfg(target_arch = "x86_64")]
mod devices;
mod interrupts;
mod irqlock;
#[cfg(target_arch = "x86_64")]
mod lastwords;
mod logtap;
mod futex;
mod ipc;
mod memory_object;
mod mm;
mod power;
mod process;
mod signals;
#[cfg(target_arch = "aarch64")]
mod mmio_devices;
mod programs;
mod random;
mod selftest;
mod backtrace;
mod syscall;
mod threads;
mod tlb;

#[cfg(target_arch = "x86_64")]
use alloc::string::String;

#[cfg(target_arch = "x86_64")]
use kernel64::acpi::{Acpi, IoApic, Madt};
#[cfg(target_arch = "x86_64")]
use kernel64::bootinfo::BootInfo;

#[cfg(target_arch = "x86_64")]
/// The first Rust code to run, called by `_start` with the physical
/// address of the TaajBootInfo (in RDI, per the C calling convention).
///
/// `-> !` means "never returns": there is nothing to return to.
extern "C" fn kernel_main(boot_info_address: u64) -> ! {
    arch::init_early_console();

    kprintln!();
    kprintln!("Hello from TheTaaJ!");
    // `{:p}` prints a pointer; the address shows the kernel really runs
    // in the higher half.
    kprintln!("64-bit kernel, kernel_main at {:p}", kernel_main as *const ());

    // From here on, a CPU fault is a message rather than a reboot.
    arch::init_cpu();
    kprintln!("[cpu] GDT, TSS and IDT loaded");
    arch::breakpoint();
    kprintln!("[cpu] resumed after the breakpoint - exceptions round-trip");

    // SAFETY: the TaajBoot protocol guarantees RDI held the structure's
    // physical address, and that the first 4 GiB are identity-mapped -
    // which the physical offset (still 0) reflects.
    let info = match unsafe { BootInfo::from_physical(boot_info_address) } {
        Ok(info) => info,
        Err(error) => panic!("boot info at {boot_info_address:#x} is unusable: {error}"),
    };

    report(info);

    // Off the loader's page tables and onto the kernel's own. `info` is
    // re-read through the direct map; the old reference is gone.
    kprintln!();
    let info = start_memory(info, boot_info_address);

    // The screen, mapped now for the one time the kernel will draw on
    // it: to say why it stopped. Later may be too late to map anything.
    if let Some(fb) = info.framebuffer() {
        lastwords::prepare(fb);
    }

    // The system log's ring (K136): from here on, everything written to
    // the debug log is kept for the log service, the self-tests too.
    logtap::start();
    selftest::run();

    // Interrupts: find the controllers, set them up, switch on.
    kprintln!();
    let madt = read_madt(info.acpi_rsdp);
    let ids: alloc::vec::Vec<u8> = madt.processors.iter().filter(|p| p.enabled).map(|p| p.apic_id).collect();
    kprintln!("[acpi] {} CPU(s), APIC ids {ids:?}", ids.len());
    // SAFETY: once, on the boot CPU, interrupts still disabled.
    let platform = unsafe { arch::platform::init(&madt, interrupts::HZ) };
    report_platform(&platform);
    start_clock(arch::cmos::seconds(), "the CMOS clock");
    arch::enable_interrupts();
    selftest::timer();

    // The other CPUs: woken and checked. They wait until there are
    // threads for them to run.
    // SAFETY: once, on the boot CPU, after platform::init, before any
    // process exists.
    unsafe { arch::smp::start(&madt) };

    // Threads: the code running now becomes thread 1, "boot"; thread 0
    // is the idle thread.
    threads::init();
    // Still on one CPU: these checks are about taking turns, and turns
    // are only taken where there is one CPU to share.
    selftest::threads();

    // Now all of them. Everything after this runs on whichever CPU is
    // free.
    arch::smp::release();
    selftest::smp();
    selftest::wakeups();

    // User mode: programs in their own address spaces, in ring 3.
    selftest::user();

    // IPC: processes talking through channels, by capability.
    selftest::ipc();

    // Files: the file service, tested by a client in ring 3.
    selftest::files();

    // Kill: ending a program from outside, whatever it is doing.
    selftest::kill();

    start_init(info);

    // Only in a build with one of the `fault-*` features: proves each
    // protection holds and is reported. Once there is a kernel command
    // line, this can move there.
    #[cfg(feature = "fault-tests")]
    run_fault_test();

    #[cfg(feature = "big-image")]
    check_big_image();

    idle()
}

/// Starts memory management on a PC, from what the loader said.
///
/// `info` must be the boot info as read through the loader's identity
/// map, at physical `info_address`. Returns it re-read through the
/// direct map: the old reference is invalid once this returns.
#[cfg(target_arch = "x86_64")]
fn start_memory(info: &'static BootInfo, info_address: u64) -> &'static BootInfo {
    const MIB: u64 = 1 << 20;
    const GIB: u64 = 1 << 30;
    use kernel64::bootinfo::{MemoryRegion, MEMORY_RESERVED};

    let regions = info.memory_map();
    let info_range = info_address..info_address + size_of::<BootInfo>() as u64;
    let map_start = regions.as_ptr() as u64; // identity map: virtual == physical
    let map_range = map_start..map_start + size_of_val(regions) as u64;

    // The first megabyte is kept out of the allocator whole; one page
    // of it is put aside for starting the other CPUs. Not page 0 (the
    // firmware's tables), and not the two things of the loader's the
    // kernel goes on reading.
    mm::pick_low_page(regions, &[0..4096, info_range.clone(), map_range.clone()]);

    let screen = info.framebuffer().map_or(0..0, |fb| {
        let size = u64::from(fb.pitch) * u64::from(fb.height);
        fb.address..fb.address + size
    });
    // The crash record's place (K188): the loader's map, and one more
    // region over it - reserved, so the allocator never hands it out
    // (where regions overlap, the one that is not usable wins). Below
    // 4 GiB, which is all in the direct map.
    const REGIONS_MAX: usize = 256;
    let mut with_crash = [MemoryRegion::new(0, 0, MEMORY_RESERVED); REGIONS_MAX + 1];
    let usable = regions.iter().filter(|r| r.is_usable()).map(|r| r.base..r.base + r.length);
    // Clear of what is in use: below 1 MiB, the ramdisk, the boot info
    // and its map, the screen, the kernel, and what is not usable.
    let mut avoid: [core::ops::Range<u64>; REGIONS_MAX + 6] = core::array::from_fn(|_| 0..0);
    let mut avoided = 0;
    let kernel = info.kernel_physical..info.kernel_physical + info.kernel_size;
    let ramdisk = info.ramdisk_address..info.ramdisk_address + info.ramdisk_size;
    let not_usable = regions.iter().filter(|r| !r.is_usable()).map(|r| r.base..r.base + r.length);
    for range in [0..MIB, ramdisk, info_range.clone(), map_range.clone(), screen.clone(), kernel].into_iter().chain(not_usable) {
        if avoided < avoid.len() {
            avoid[avoided] = range;
            avoided += 1;
        }
    }
    let regions = match kernel64::crashrecord::place(usable, &avoid[..avoided], 4 * GIB) {
        Some(area) if regions.len() <= REGIONS_MAX => {
            with_crash[..regions.len()].copy_from_slice(regions);
            with_crash[regions.len()] = MemoryRegion::new(area.start, area.end - area.start, MEMORY_RESERVED);
            logtap::set_crash_area(area.start);
            &with_crash[..regions.len() + 1]
        }
        _ => regions,
    };
    mm::init(&mm::BootMemory {
        regions,
        taken: [
            0..MIB, // real-mode IVT, BIOS data, the loader, its page tables
            info.ramdisk_address..info.ramdisk_address + info.ramdisk_size,
            info_range,
            map_range,
        ],
        // The first 4 GiB whatever is there: firmware tables and
        // devices live between the pieces of memory.
        also_mapped: [0..4 * GIB, screen],
    });

    // SAFETY: the boot info is below 1 MiB, now reached via the direct map.
    unsafe { BootInfo::from_physical(info_address) }.expect("boot info vanished")
}

/// The first Rust code to run on AArch64, called by `_start`
/// (arch/aarch64/entry.rs) with what it learned on the way: the
/// physical address of the device tree, the exception level the loader
/// entered at, and the physical address the image was loaded at.
///
/// It does less than the x86_64 one above, and stops: this is how far
/// the port has come.
#[cfg(target_arch = "aarch64")]
#[no_mangle]
extern "C" fn kernel_main(device_tree: u64, entered_at: u64, loaded_at: u64, mmu_was_on: u64) -> ! {
    use kernel64::aarch64::{cpu_name, physical_bits, KERNEL_WINDOW, TEXT_OFFSET};
    use kernel64::bootinfo::{MemoryRegion, MEMORY_RESERVED, MEMORY_USABLE};
    use kernel64::devicetree::{Machine, Tree};
    use kernel64::memory::addr::PhysAddr;

    arch::mmu::set_load_address(loaded_at);

    // The device tree first, and silently: it says where the serial
    // port is, so nothing can be said until it has been read.
    let tree = arch::device_tree(device_tree).map(Tree::new);
    let machine = match &tree {
        Some(Ok(tree)) => Machine::read(tree),
        _ => Machine::default(),
    };
    let console_from_tree = arch::init_early_console(machine.serial);

    kprintln!();
    kprintln!("Hello from TheTaaJ!");
    kprintln!("64-bit kernel on AArch64, kernel_main at {:p}", kernel_main as *const ());

    // From here on, a CPU fault is a message rather than silence.
    arch::init_cpu();
    kprintln!("[cpu] exception vectors loaded");
    arch::breakpoint();
    kprintln!("[cpu] resumed after the breakpoint - exceptions round-trip");

    let cpu = arch::report();
    kprintln!(
        "[cpu] {} (id {:#x}), at exception level {}; the loader entered at level {entered_at}, with the MMU {}",
        cpu_name(cpu.cpu_id),
        cpu.cpu_id,
        cpu.level,
        if mmu_was_on == 0 { "off" } else { "on - it is to be off: the loader has a bug, which this kernel happens to survive" }
    );
    kprintln!(
        "[cpu] {} bits of physical address; MMU on, the kernel loaded at {loaded_at:#x} and running at {:#x}",
        physical_bits(cpu.physical_range),
        KERNEL_WINDOW + TEXT_OFFSET
    );

    kprintln!();
    match &tree {
        Some(Ok(tree)) => report_machine(device_tree, tree, &machine),
        Some(Err(error)) => kprintln!("[tree] at {device_tree:#x} the loader left {error}"),
        None => kprintln!("[tree] no device tree: the loader passed {device_tree:#x}"),
    }
    if !console_from_tree {
        kprintln!("[tree] no serial port with a clock described: using the one QEMU's virt machine has, as a guess");
    }

    // Memory: the tree's description, turned into the list of regions
    // the memory manager is given on every machine. Reserved ranges
    // come after the memory they lie in; where regions overlap, the
    // one that is not usable wins.
    kprintln!();
    let mut regions = [MemoryRegion::new(0, 0, MEMORY_RESERVED); Machine::MEMORY_MAX + Machine::RESERVED_MAX + 1];
    let mut count = 0;
    let usable = machine.memory().iter().map(|range| MemoryRegion::new(range.start, range.size, MEMORY_USABLE));
    let reserved = machine.reserved().iter().map(|range| MemoryRegion::new(range.start, range.size, MEMORY_RESERVED));
    for region in usable.chain(reserved) {
        regions[count] = region;
        count += 1;
    }
    // The crash record's place (K188): kept out of the allocator, and in
    // the direct map, where the kernel writes its last words.
    // Clear of what firmware keeps (the tree's reserved ranges) and of
    // the tree itself; the kernel is at the bottom of memory.
    let mut avoid: [core::ops::Range<u64>; Machine::RESERVED_MAX + 1] = core::array::from_fn(|_| 0..0);
    for (n, range) in machine.reserved().iter().enumerate().take(Machine::RESERVED_MAX) {
        avoid[n] = range.start..range.start + range.size;
    }
    avoid[Machine::RESERVED_MAX] = device_tree..device_tree + tree.as_ref().map_or(0, |t| t.as_ref().map_or(0, |t| u64::from(t.header.total_size)));
    let crash = kernel64::crashrecord::place(machine.memory().iter().map(|range| range.start..range.start + range.size), &avoid, u64::MAX);
    if let Some(area) = &crash {
        regions[count] = MemoryRegion::new(area.start, area.end - area.start, MEMORY_RESERVED);
        count += 1;
        logtap::set_crash_area(area.start);
    }
    // The tree itself is in memory the tree calls free.
    let tree_range = match &tree {
        Some(Ok(tree)) => device_tree..device_tree + u64::from(tree.header.total_size),
        _ => 0..0,
    };
    let nodes = machine.nodes;
    mm::init(&mm::BootMemory {
        regions: &regions[..count],
        taken: [tree_range, 0..0, 0..0, 0..0],
        also_mapped: [crash.unwrap_or(0..0), 0..0],
    });

    // The low addresses are gone, and the tree was read through them:
    // `tree` and `machine` point at nothing now. Read it again, where
    // it is to be found from here on.
    let tree = arch::device_tree(device_tree).map(Tree::new);
    let machine = match &tree {
        Some(Ok(tree)) => Machine::read(tree),
        _ => Machine::default(),
    };
    assert_eq!(machine.nodes, nodes, "the device tree reads differently through the direct map");
    // And it must stay as it is: its memory is not free memory.
    if let Some(Ok(tree)) = &tree {
        let last = device_tree + u64::from(tree.header.total_size) - 1;
        let kept = mm::with_frames(|frames| ![device_tree, last].iter().any(|&byte| frames.is_free(PhysAddr::new(byte))));
        assert!(kept, "the device tree is in memory the frame allocator will hand out");
    }
    kprintln!("[tree] read again through the direct map: the same {} nodes, in memory kept out of the allocator", machine.nodes);

    // The system log's ring (K136): from here on, everything written to
    // the debug log is kept for the log service, the self-tests too.
    logtap::start();
    selftest::run();
    arch::mmu::selftest();

    // Devices: what is in the places the tree describes. Looking
    // needs only memory; making one ready for a driver needs its
    // interrupt, and so waits for the controller.
    kprintln!();
    let devices = mmio_devices::find(&machine);

    // Interrupts: the controller the tree describes, and the CPU's
    // own timer.
    kprintln!();
    let mut disk = None;
    let mut network = None;
    let mut inputs = alloc::vec::Vec::new();
    let mut screen = None;
    // SAFETY: once, on the only CPU running, interrupts still masked.
    let threads_run = match unsafe { arch::init_interrupts(&machine) } {
        Some(interrupts) => {
            kprintln!(
                "[irq] GIC version {}: {} interrupt lines, all off but the timer's",
                interrupts.version,
                interrupts.lines
            );
            kprintln!(
                "[timer] the CPU's own: counts at {} Hz, interrupt {}, {} ticks a second",
                interrupts.timer_hz,
                interrupts.timer_interrupt,
                interrupts::HZ
            );
            start_clock(machine.rtc.and_then(arch::pl031_seconds), "the PL031");
            arch::enable_interrupts();
            selftest::timer();
            arch::selftest_interrupts();

            // The other CPUs, if the machine has any: woken, shown to
            // run at the same time, and then left waiting.
            kprintln!();
            // SAFETY: once, on the boot CPU, the controller and the
            // timer set up above, before any process exists.
            unsafe { arch::smp::start(&machine) };

            // Threads: the code running now becomes thread 1, "boot";
            // thread 0 is the idle thread. They need the tick, so they
            // start only where there is one.
            kprintln!();
            threads::init();
            // Still on one CPU: these checks are about taking turns,
            // and turns are only taken where there is one CPU to share.
            selftest::threads();

            // Now all of them. Everything after this runs on
            // whichever CPU is free.
            arch::smp::release();
            selftest::smp();
            selftest::wakeups();

            // Programs: in their own address spaces, at exception
            // level 0, asking for things with `svc`.
            selftest::user();
            selftest::ipc();
            selftest::files();
            selftest::kill();

            // A device, driven by a program.
            kprintln!();
            disk = mmio_devices::first_disk(&devices);
            selftest::disk(disk.as_ref());
            // A network card is only made ready here: its driver needs
            // someone on the wire to be tested against, which is a
            // test of the whole system's.
            network = mmio_devices::first_network(&devices);
            // Keyboards, mice and tablets: each made ready for the one
            // driver that serves them all.
            inputs = mmio_devices::inputs(&devices);
            // And a screen, if the machine was given one.
            screen = match &tree {
                Some(Ok(tree)) => ramfb(tree),
                _ => None,
            };
            true
        }
        None => {
            kprintln!("[irq] the device tree describes no interrupt controller this kernel knows: no interrupts, no threads");
            false
        }
    };

    #[cfg(feature = "fault-stack")]
    {
        kprintln!();
        kprintln!("[test] overflowing the boot stack...");
        let depth = arch::fault_tests::overflow_stack(0);
        kprintln!("[test] FAILED: recursion returned {depth}");
    }
    #[cfg(feature = "fault-thread-stack")]
    {
        kprintln!();
        kprintln!("[test] overflowing a thread's stack...");
        let id = threads::spawn("overflow", || {
            let depth = arch::fault_tests::overflow_stack(0);
            kprintln!("[test] FAILED: recursion returned {depth}");
        });
        threads::join(id);
    }
    #[cfg(feature = "fault-null")]
    {
        kprintln!();
        kprintln!("[test] reading through a null pointer...");
        let value = arch::fault_tests::null_read();
        kprintln!("[test] FAILED: read {value:#x} from address 0");
    }
    #[cfg(feature = "fault-page")]
    {
        kprintln!();
        kprintln!("[test] forcing a page fault...");
        let value = arch::fault_tests::page_fault();
        kprintln!("[test] FAILED: read {value:#x} from unmapped memory");
    }
    #[cfg(feature = "fault-rodata")]
    {
        kprintln!();
        kprintln!("[test] writing to read-only data...");
        arch::fault_tests::write_rodata();
        kprintln!("[test] FAILED: the fault did not happen");
    }
    #[cfg(feature = "fault-nx")]
    {
        kprintln!();
        kprintln!("[test] executing data...");
        arch::fault_tests::execute_data();
        kprintln!("[test] FAILED: the fault did not happen");
    }

    // The rest is programs: init, and what init starts. They need
    // threads, and a terminal to be of any use.
    if threads_run && start_init(&machine, screen, disk.into_iter().chain(network).chain(inputs)) {
        idle()
    }
    kprintln!();
    kprintln!("[kernel] no programs can run on this machine - halting");
    arch::halt_forever()
}

/// Starts `init`, the first process, on AArch64, with five startup
/// handles:
///
///   1  the launcher: the right to start programs
///   2  the serial port's interrupt line
///   3  the memory the serial port's registers are in
///   4  a channel from the kernel, carrying one message per device it
///      made ready: "mmio", an `MmioDeviceInfo`, and the device's
///      handles (a disk, a network card)
///   5  the system handle: the right to look at memory and processes
///
/// The serial port is this machine's keyboard and its screen at once,
/// so where the PC's init gets a keyboard's interrupt and ports here,
/// this one gets the port's interrupt and registers - for a driver
/// that is a program, like the disk's. The kernel goes on writing its
/// own log to the same port, and never reads from it.
///
/// False - and nothing started - if the device tree describes no
/// serial port with an interrupt.
#[cfg(target_arch = "aarch64")]
fn start_init(
    machine: &kernel64::devicetree::Machine,
    screen: Option<(abi::FramebufferInfo, memory_object::MemoryObject)>,
    devices: impl IntoIterator<Item = mmio_devices::Ready>,
) -> bool {
    use ipc::Object;
    use kernel64::cap::Rights;
    use kernel64::paging::Flags;
    use memory_object::MemoryObject;

    let Some((serial, (interrupt, edge))) = machine.serial.and_then(|serial| Some((serial, serial.interrupt?))) else {
        kprintln!("[kernel] the device tree gives the serial port no interrupt: no terminal, and so no init");
        return false;
    };
    let Some(line) = arch::device_interrupt(interrupt, edge) else {
        kprintln!("[kernel] the serial port's interrupt {interrupt} is not one the controller has: no terminal, and so no init");
        return false;
    };
    let registers = MemoryObject::device(serial.registers.start, serial.registers.size, Flags::NO_CACHE | Flags::WRITE_THROUGH);
    kprintln!(
        "[kernel] serial port at {:#x}, interrupt {interrupt} ({}): handed to init, for a driver",
        serial.registers.start,
        if edge { "edge" } else { "level" }
    );

    let (from_kernel, to_init) = ipc::channel();
    // The screen first, described as the PC's is: init cannot tell
    // which kind of machine drew it.
    if let Some((description, memory)) = screen {
        let mut message = b"framebuffer".to_vec();
        message.extend_from_slice(&description.to_bytes());
        match ipc::kernel_send(&to_init, &message, alloc::vec![(Object::Memory(alloc::sync::Arc::new(memory)), Rights::ALL)]) {
            Ok(()) => kprintln!("framebuffer: {}x{}, handed to init", description.width, description.height),
            Err(error) => kprintln!("framebuffer: could not hand it to init: {error:?}"),
        }
    }
    for device in devices {
        let what = match device.info.device {
            virtio::mmio::DEVICE_NETWORK => "network card",
            virtio::mmio::DEVICE_INPUT => "input device",
            _ => "block device",
        };
        let mut message = b"mmio".to_vec();
        message.extend_from_slice(&device.info.to_bytes());
        match ipc::kernel_send(&to_init, &message, device.handles) {
            Ok(()) => kprintln!("[virtio] {:#x} {what}: handed to init", device.info.registers),
            Err(error) => kprintln!("[virtio] {:#x} {what}: could not hand it to init: {error:?}", device.info.registers),
        }
    }
    power::offer(&to_init, machine.firmware_call);
    logtap::offer(&to_init);
    drop(to_init); // what was sent stays readable; the kernel keeps nothing

    let handles = alloc::vec![
        (Object::Launcher, Rights(0)),
        (Object::Interrupt(line), Rights::ALL),
        (Object::Memory(alloc::sync::Arc::new(registers)), Rights::ALL),
        (from_kernel, Rights(abi::rights::RECEIVE)),
        (Object::System, Rights::ALL),
    ];
    alarms::start();
    let (init, _) = process::spawn("init", programs::INIT, handles).expect("starting init");
    process::set_essential(&init);
    true
}

/// The screen's size. The PC's loader picks the same, so that both
/// machines show the same console.
#[cfg(target_arch = "aarch64")]
const SCREEN: (u32, u32) = (1024, 768);

/// A screen, if QEMU was asked for one (`-device ramfb`): memory for
/// its pixels, and QEMU told where it is (kernel64::fwcfg says how).
/// None, and a line saying why, if there is no fw_cfg device, no
/// "etc/ramfb" in it, or no memory.
#[cfg(target_arch = "aarch64")]
fn ramfb(tree: &kernel64::devicetree::Tree) -> Option<(abi::FramebufferInfo, memory_object::MemoryObject)> {
    use core::sync::atomic::{fence, Ordering};
    use kernel64::fwcfg;
    use kernel64::memory::addr::{phys_to_virt, PhysAddr};
    use memory_object::MemoryObject;

    let registers = tree.compatible("qemu,fw-cfg-mmio")?.reg().next()?;
    if registers.size < fwcfg::DMA_ADDRESS + 8 {
        kprintln!("[screen] the fw_cfg device at {:#x} is too small to be one", registers.start);
        return None;
    }
    let base = mm::map_mmio(registers.start, registers.size);
    // The three closures reach registers of the fw_cfg device, mapped as
    // device memory just above; volatile, because each access is an
    // instruction to the device.
    // SAFETY: the selector register, within the mapping.
    let select = |selector: u16| unsafe { ((base + fwcfg::SELECTOR) as *mut u16).write_volatile(selector.to_be()) };
    // SAFETY: the data register, within the mapping.
    let byte = || unsafe { ((base + fwcfg::DATA) as *const u8).read_volatile() };
    // SAFETY: the two halves of the DMA address register, within the mapping.
    let dma = |half: u64, value: u32| unsafe { ((base + fwcfg::DMA_ADDRESS + half) as *mut u32).write_volatile(value.to_be()) };

    // The directory: a count, then an entry per file. Read whole (it is
    // a few kilobytes), with a limit on what a broken count can ask.
    select(fwcfg::FILE_DIR);
    let mut directory = alloc::vec::Vec::new();
    directory.extend((0..4).map(|_| byte()));
    let count = u32::from_be_bytes([directory[0], directory[1], directory[2], directory[3]]).min(1024) as usize;
    directory.extend((0..count * fwcfg::DIR_ENTRY).map(|_| byte()));
    let Some((selector, size)) = fwcfg::find(&directory, "etc/ramfb") else {
        kprintln!("[screen] none: QEMU has no ramfb device (fw_cfg lists {count} files)");
        return None;
    };
    if size as usize != fwcfg::RAMFB_CONFIG {
        kprintln!("[screen] etc/ramfb is {size} bytes, not the {} it should be: not used", fwcfg::RAMFB_CONFIG);
        return None;
    }

    let (width, height) = SCREEN;
    let stride = width * 4;
    let pages = (u64::from(stride) * u64::from(height)).div_ceil(4096);
    let Some((pixels, physical)) = MemoryObject::contiguous(pages) else {
        kprintln!("[screen] no {} KiB of contiguous memory for the pixels", pages * 4);
        return None;
    };
    // The request and the description it carries, in one page of its
    // own: QEMU reads both by their physical addresses.
    let (page, page_physical) = MemoryObject::contiguous(1)?;
    let config = fwcfg::ramfb_config(physical, width, height, stride);
    let request = fwcfg::dma_write(selector, config.len() as u32, page_physical + 64);
    let window = phys_to_virt(PhysAddr::new(page_physical)).as_mut_ptr::<u8>();
    // SAFETY: the fresh page, through the direct map, ours alone.
    unsafe {
        window.add(64).copy_from_nonoverlapping(config.as_ptr(), config.len());
        window.copy_from_nonoverlapping(request.as_ptr(), request.len());
    }
    // Everything in memory before the device is told where to look.
    fence(Ordering::SeqCst);
    dma(0, (page_physical >> 32) as u32);
    dma(4, page_physical as u32);
    fence(Ordering::SeqCst);
    // QEMU does it at once; the wait is for a device that does not.
    let done = (0..1_000_000).find_map(|_| {
        // SAFETY: the request's first field, in the page above.
        let control = u32::from_be(unsafe { (window as *const u32).read_volatile() });
        fwcfg::dma_done(control)
    });
    match done {
        Some(Ok(())) => drop(page),
        Some(Err(())) => {
            kprintln!("[screen] fw_cfg refused the ramfb description");
            return None;
        }
        None => {
            // The device may still read the request, and write its
            // answer into it, later: the page must never be reused.
            core::mem::forget(page);
            kprintln!("[screen] fw_cfg did not finish writing the ramfb description");
            return None;
        }
    }
    kprintln!("[screen] ramfb: {width}x{height}, 32 bits a pixel, {} KiB at {physical:#x}", pages * 4);
    let description = abi::FramebufferInfo {
        width,
        height,
        pitch: stride,
        bpp: 32,
        red_size: 8,
        red_shift: 16,
        green_size: 8,
        green_shift: 8,
        blue_size: 8,
        blue_shift: 0,
    };
    Some((description, pixels))
}

/// Prints what the device tree said about the machine.
#[cfg(target_arch = "aarch64")]
fn report_machine(address: u64, tree: &kernel64::devicetree::Tree, machine: &kernel64::devicetree::Machine) {
    use kernel64::devicetree::{FirmwareCall, Machine};

    kprintln!(
        "[tree] device tree at {address:#x}: {} bytes, version {}, {} nodes, the deepest {} down",
        tree.header.total_size,
        tree.header.version,
        machine.nodes,
        machine.deepest
    );
    if let Some(model) = machine.model {
        kprintln!("[tree] machine     {model:?}");
    }
    for range in machine.memory() {
        kprintln!(
            "[tree] memory      {:#x} - {:#x} ({} MiB)",
            range.start,
            range.start + range.size,
            range.size / (1024 * 1024)
        );
    }
    if machine.memory_count > Machine::MEMORY_MAX {
        kprintln!("[tree]             and {} more ranges, not kept", machine.memory_count - Machine::MEMORY_MAX);
    }
    for range in tree.reserved() {
        kprintln!("[tree] reserved    {:#x} - {:#x}", range.start, range.start + range.size);
    }
    kprintln!("[tree] cpus        {}, with ids {:x?}", machine.cpu_count, machine.cpus());
    match machine.serial {
        Some(serial) => {
            kprintln!(
                "[tree] serial      PL011 at {:#x}, {}",
                serial.registers.start,
                if serial.chosen { "named as the console" } else { "the first one found" }
            );
            match serial.clock_hz {
                Some(clock) => kprintln!("[tree]             its clock runs at {clock} Hz"),
                None => kprintln!("[tree]             its clock is not given"),
            }
        }
        None => kprintln!("[tree] serial      none that this kernel knows"),
    }
    match machine.interrupt_controller {
        Some(gic) => kprintln!(
            "[tree] interrupts  GIC version {}: shared part at {:#x}, per-CPU part at {:#x}",
            gic.version,
            gic.distributor.start,
            gic.per_cpu.start
        ),
        None => kprintln!("[tree] interrupts  no controller that this kernel knows"),
    }
    match machine.timer_interrupts {
        [Some(secure), Some(physical), Some(virtual_), Some(hypervisor)] => kprintln!(
            "[tree] timer       interrupts {secure} (secure), {physical} (physical), {virtual_} (virtual), {hypervisor} (hypervisor)"
        ),
        _ => kprintln!("[tree] timer       not described"),
    }
    match machine.firmware_call {
        Some(FirmwareCall::Hvc) => kprintln!("[tree] firmware    called with hvc"),
        Some(FirmwareCall::Smc) => kprintln!("[tree] firmware    called with smc"),
        None => kprintln!("[tree] firmware    no way to call it described"),
    }
}

/// Only in a `big-image` build: the kernel carries 4 MiB it does not
/// need, which puts its end near 7 MiB - far past the 3 MiB wall the BIOS
/// loader had until K130 (it staged the whole file at 3 MiB, and the
/// segments had to fit below). If the loader read all of it to the right
/// place, every byte is what the build script wrote. (256 KiB from entry
/// 122 until K130, to stay under that wall.)
#[cfg(all(target_arch = "x86_64", feature = "big-image"))]
fn check_big_image() {
    static BALLAST: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ballast.bin"));
    // `black_box`: the compiler must read the bytes from memory when
    // the kernel runs, not work the answer out while compiling.
    let ballast = core::hint::black_box(BALLAST);
    let wrong = ballast
        .iter()
        .enumerate()
        .filter(|&(i, &byte)| byte != ((i as u32).wrapping_mul(2_654_435_761) >> 13) as u8)
        .count();
    let start = ballast.as_ptr() as u64 - mm::KERNEL_WINDOW;
    let end = start + ballast.len() as u64;
    let image_end = mm::kernel_layout().data.end - mm::KERNEL_WINDOW;
    kprintln!();
    if wrong == 0 && image_end > 0x30_0000 {
        kprintln!(
            "[test] big image: the kernel ends at {image_end:#x}, above 3 MiB; {} KiB of it at {start:#x}-{end:#x} is byte for byte what was built",
            ballast.len() / 1024
        );
    } else {
        kprintln!("[test] FAILED - big image: ends at {image_end:#x}, {wrong} of {} bytes wrong", ballast.len());
    }
}

#[cfg(target_arch = "x86_64")]
/// Reads the MADT, or - with no usable ACPI - assumes the standard PC
/// layout: one CPU, the local APIC and one I/O APIC at their
/// architectural default addresses, no overrides.
fn read_madt(rsdp: u64) -> Madt {
    let memory = mm::DirectMap;
    let found = match Acpi::new(&memory, rsdp) {
        Ok(acpi) => {
            let mut names = String::new();
            for signature in acpi.signatures(&memory) {
                names.push_str(core::str::from_utf8(&signature).unwrap_or("????"));
                names.push(' ');
            }
            kprintln!("[acpi] revision {}, OEM {:?}, tables: {names}", acpi.revision,
                core::str::from_utf8(&acpi.oem).unwrap_or("?").trim_end());
            acpi.find(&memory, b"APIC").and_then(|table| Madt::parse(&table))
        }
        Err(error) => Err(error),
    };
    found.unwrap_or_else(|error| {
        kprintln!("[acpi] no usable MADT ({error}) - assuming the standard PC layout");
        Madt {
            local_apic_address: 0xFEE0_0000,
            has_legacy_pics: true,
            io_apics: alloc::vec![IoApic { id: 0, address: 0xFEC0_0000, gsi_base: 0 }],
            ..Madt::default()
        }
    })
}

/// Starts the kernel's clock (clock.rs) from the real-time clock's
/// reading, if there was one, and says what the date is.
fn start_clock(rtc: Option<u64>, name: &str) {
    clock::start(rtc);
    let (instruction, from_cpu, timings) = random::init(rtc.unwrap_or(0));
    match from_cpu {
        0 => kprintln!("[random] seeded from {timings} timings (no random instruction on this CPU)"),
        n => kprintln!("[random] seeded from {n} numbers from {instruction} and {timings} timings"),
    }
    match rtc {
        Some(seconds) => {
            kprintln!("[clock] {} says {} UTC; time is counted from here at {} Hz", name, kernel64::rtc::Date::from_unix_seconds(seconds), arch::clock_hz())
        }
        None => kprintln!("[clock] no date from {name}: the time of day is the time since boot, from 1970"),
    }
}

#[cfg(target_arch = "x86_64")]
fn report_platform(p: &arch::platform::Report) {
    kprintln!("[irq] local APIC id {}; legacy PICs remapped and masked", p.cpu_apic_id);
    for &(id, address, gsi_base, inputs) in &p.io_apics {
        kprintln!("[irq] I/O APIC {id} at {address:#x}: GSIs {gsi_base}-{}", gsi_base + inputs - 1);
    }
    let source = match p.measured {
        Some((bus, tsc)) => alloc::format!(
            "stated by the hypervisor (the PIT measured {} and {} MHz)",
            bus / 1_000_000,
            tsc / 1_000_000
        ),
        None => String::from("measured against the PIT"),
    };
    kprintln!(
        "[irq] APIC timer at {} Hz (bus clock {} MHz), TSC {} MHz: {source}",
        p.timer_hz,
        p.bus_hz / 1_000_000,
        p.tsc_hz / 1_000_000
    );
    match p.keyboard_gsi {
        Some(gsi) => kprintln!("[irq] PS/2 keyboard on GSI {gsi}"),
        None => kprintln!("[irq] no PS/2 keyboard"),
    }
    match p.mouse_gsi {
        Some(gsi) => kprintln!("[irq] PS/2 mouse on GSI {gsi}, sharing the keyboard's vector"),
        None => kprintln!("[irq] no PS/2 mouse"),
    }
}

/// The end of booting: the boot thread's work is done, so it exits and
/// leaves the CPU to the threads it started - and to the idle thread.
fn idle() -> ! {
    kprintln!();
    kprintln!("[kernel] boot done; running: {}", threads::names().join(", "));
    kprintln!("[kernel] idle - the services take it from here");
    threads::exit()
}

#[cfg(target_arch = "x86_64")]
/// Starts `init`, the first process, and gives it everything a user
/// space needs to build itself:
///
///   1  the launcher: the right to start programs
///   2  the keyboard's interrupt line
///   3  I/O port 0x60
///   4  I/O port 0x64
///   5  a channel from the kernel, carrying one message per device it
///      found: for now, "framebuffer" - its description, and a handle to
///      its memory
///   6  the system handle: the right to look at memory and processes
///
/// From here on init decides what runs and who may touch what; the
/// kernel enforces it. The kernel itself no longer draws on the screen.
fn start_init(info: &BootInfo) {
    use ipc::Object;
    use kernel64::cap::Rights;
    use kernel64::paging::Flags;
    use memory_object::MemoryObject;

    let (devices, to_init) = ipc::channel();
    if let Some(fb) = info.framebuffer() {
        let description = abi::FramebufferInfo {
            width: fb.width,
            height: fb.height,
            pitch: fb.pitch,
            bpp: fb.bpp,
            red_size: fb.red_size,
            red_shift: fb.red_shift,
            green_size: fb.green_size,
            green_shift: fb.green_shift,
            blue_size: fb.blue_size,
            blue_shift: fb.blue_shift,
        };
        // Device memory: write-through, so pixels reach the screen
        // rather than waiting in the cache.
        let size = u64::from(fb.pitch) * u64::from(fb.height);
        let memory = MemoryObject::device(fb.address, size, Flags::WRITE_THROUGH);
        let mut message = b"framebuffer".to_vec();
        message.extend_from_slice(&description.to_bytes());
        ipc::kernel_send(&to_init, &message, alloc::vec![(Object::Memory(alloc::sync::Arc::new(memory)), Rights::ALL)])
            .expect("a new channel has room");
        kprintln!("framebuffer: {}x{}, handed to init", fb.width, fb.height);
    } else {
        kprintln!("framebuffer: none");
    }
    devices::offer(&to_init);
    power::offer(&to_init, info.acpi_rsdp);
    logtap::offer(&to_init);
    drop(to_init); // what was sent stays readable; the kernel keeps nothing

    let handles = alloc::vec![
        (Object::Launcher, Rights(0)),
        (Object::Interrupt(&interrupts::KEYBOARD), Rights::ALL),
        (Object::IoPorts(0x60..0x61), Rights::ALL),
        (Object::IoPorts(0x64..0x65), Rights::ALL),
        (devices, Rights(abi::rights::RECEIVE)),
        (Object::System, Rights::ALL),
    ];
    // In the fault-init test, "init" is a program that says hello and
    // ends - to see what the kernel does when init does.
    let program = if cfg!(feature = "fault-init") { programs::HELLO } else { programs::INIT };
    alarms::start();
    let (init, _) = process::spawn("init", program, handles).expect("starting init");
    process::set_essential(&init);
}

#[cfg(all(target_arch = "x86_64", feature = "fault-tests"))]
fn run_fault_test() {
    use arch::fault_tests as test;
    kprintln!();
    if cfg!(feature = "fault-init") {
        kprintln!("[test] init is a program that ends: the kernel must stop, and say so");
        return; // it runs once this thread lets it
    }
    if cfg!(feature = "fault-panic") {
        // The first boot panics (and resets); the next one finds the
        // crash record, and goes on as usual - init, logd, the shell.
        if logtap::found_last_boot() {
            kprintln!("[test] the boot before this one left its last words: going on as usual");
            return;
        }
        panic!("a deliberate panic, for the crash record (fault-panic)");
    }
    if cfg!(feature = "fault-double") {
        kprintln!("[test] forcing a double fault...");
        test::double_fault();
    }
    if cfg!(feature = "fault-rodata") {
        kprintln!("[test] writing to read-only data...");
        test::write_rodata();
    }
    if cfg!(feature = "fault-nx") {
        kprintln!("[test] executing data...");
        test::execute_data();
    }
    if cfg!(feature = "fault-stack") {
        kprintln!("[test] overflowing the boot stack...");
        let depth = test::overflow_stack(0);
        kprintln!("[test] FAILED: recursion returned {depth}");
    }
    if cfg!(feature = "fault-thread-stack") {
        kprintln!("[test] overflowing a thread's stack...");
        let id = threads::spawn("overflow", || {
            let depth = test::overflow_stack(0);
            kprintln!("[test] FAILED: recursion returned {depth}");
        });
        threads::join(id);
    }
    if cfg!(feature = "fault-page") {
        kprintln!("[test] forcing a page fault...");
        let value = test::page_fault();
        kprintln!("[test] FAILED: read {value:#x} from unmapped memory");
    }
    kprintln!("[test] FAILED: the fault did not happen");
}

#[cfg(target_arch = "x86_64")]
/// Prints everything the bootloader told us.
fn report(info: &BootInfo) {
    kprintln!();
    kprintln!("boot info:");
    kprintln!("  loader         {} ({})", info.loader_name(), info.boot_method_name());
    kprintln!(
        "  kernel         {:#x} bytes at physical {:#x}, virtual {:#x}",
        info.kernel_size,
        info.kernel_physical,
        info.kernel_virtual
    );

    match (info.ramdisk_address, info.ramdisk_size) {
        (0, _) => kprintln!("  ramdisk        none"),
        (address, size) => kprintln!("  ramdisk        {size:#x} bytes at {address:#x}"),
    }
    match info.acpi_rsdp {
        0 => kprintln!("  ACPI RSDP      not found"),
        rsdp => kprintln!("  ACPI RSDP      {rsdp:#x}"),
    }
    if info.device_tree != 0 {
        kprintln!("  device tree    {:#x}", info.device_tree);
    }
    if let Some(line) = info.command_line() {
        kprintln!("  command line   {line}");
    }
    if let Some(fb) = info.framebuffer() {
        kprintln!(
            "  framebuffer    {}x{}, {} bpp, pitch {}, at {:#x}",
            fb.width,
            fb.height,
            fb.bpp,
            fb.pitch,
            fb.address
        );
    }

    let regions = info.memory_map();
    let mut usable: u64 = 0;
    kprintln!("  memory map     {} regions:", regions.len());
    for region in regions {
        kprintln!(
            "    {:#014x} - {:#014x}  {:>10} KiB  {}",
            region.base,
            region.base + region.length,
            region.length / 1024,
            region.kind_name()
        );
        if region.is_usable() {
            usable += region.length;
        }
    }
    kprintln!("  usable memory  {} MiB", usable / (1024 * 1024));
}

/// What happens on a bug the kernel cannot recover from: say so, loudly,
/// and stop. `PanicInfo` formats as the location plus the message.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    arch::stop_other_cpus();
    // SAFETY: halting right after; whoever held the console never resumes.
    unsafe { console::break_lock() };
    kprintln!();
    kprintln!("*** KERNEL PANIC: {info}");
    backtrace::report(None, None, arch::frame_pointer());
    // The crash-record test (K188): the machine is reset, so that the
    // next boot - in the same memory - finds these last words.
    #[cfg(all(target_arch = "x86_64", feature = "fault-panic"))]
    {
        kprintln!("[test] resetting, for the next boot to find the crash record");
        // SAFETY: the PC's reset control register (0xCF9, "full reset");
        // nothing runs after it.
        unsafe { core::arch::asm!("out dx, al", in("dx") 0xCF9u16, in("al") 0x06u8, options(nomem, nostack)) };
    }
    arch::halt_forever()
}
