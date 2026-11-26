//! The interrupt controller, touched. What the registers are and mean
//! is in `kernel64::gic`; this file reads and writes them.
//!
//! Both versions are set up to do the same thing: every interrupt off
//! except those `enable` is called for, all at one priority, all
//! arriving as the CPU's ordinary interrupt ("IRQ").

use core::arch::asm;
use core::sync::atomic::{AtomicPtr, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};

use kernel64::devicetree::InterruptController;
use kernel64::gic::{self, cpu_interface, distributor, redistributor, Kind};
use kernel64::smp::MAX_CPUS;

use crate::mm;

/// 2 or 3, once `init` has run; 0 before.
static VERSION: AtomicU8 = AtomicU8::new(0);
/// How many interrupt numbers the controller has.
static LINES: AtomicU32 = AtomicU32::new(0);

/// Whether `id` is a shared interrupt this controller really has.
pub fn has_shared(id: u32) -> bool {
    gic::kind(id) == Kind::Shared && id < LINES.load(Ordering::Relaxed)
}
/// The distributor's registers.
static DISTRIBUTOR: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());
/// Each CPU's own part of the controller, by the kernel's number for
/// the CPU. Version 2: the CPU interface - one address for all, at
/// which each CPU sees its own registers. Version 3: the CPU's
/// redistributor, which is a different place for each.
static PER_CPU: [AtomicPtr<u8>; MAX_CPUS] = [const { AtomicPtr::new(core::ptr::null_mut()) }; MAX_CPUS];
/// Version 3: where the redistributors are, all of them, and how many
/// bytes of them there are - so that a CPU started later can find its.
static FRAMES: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());
static FRAMES_SIZE: AtomicUsize = AtomicUsize::new(0);
/// Version 2: the bit that stands for each CPU in a TARGETS byte and
/// in a software interrupt's list. The controller numbers CPUs its own
/// way; each CPU reads its bit from the controller when it starts.
static TARGET: [AtomicU8; MAX_CPUS] = [const { AtomicU8::new(0) }; MAX_CPUS];
/// The boot CPU's `MPIDR_EL1`: where devices' interrupts are sent,
/// whichever CPU happens to run the code that sets one up.
static BOOT_MPIDR: AtomicU64 = AtomicU64::new(0);

/// This CPU's own part of the controller.
fn per_cpu() -> *mut u8 {
    PER_CPU[super::cpu()].load(Ordering::Relaxed)
}

fn mpidr() -> u64 {
    let value: u64;
    // SAFETY: reading this CPU's name changes nothing.
    unsafe { asm!("mrs {}, mpidr_el1", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

/// Version 2: this CPU's bit, asked of the controller. The TARGETS
/// bytes of the first interrupts - private ones, which can go nowhere
/// else - read as the CPU that is reading them. A controller built
/// for one CPU answers 0 there: then there is only bit 0.
///
/// # Safety
/// `dist` is the mapped distributor.
unsafe fn own_target(dist: *mut u8) -> u8 {
    // SAFETY: the caller's promise; a byte of a register that exists.
    match unsafe { dist.add(distributor::TARGETS).read_volatile() } {
        0 => 1,
        bit => bit,
    }
}

/// A 32-bit register at `base + offset`.
///
/// # Safety
/// `base` must be device registers mapped by `mm::map_mmio`, and
/// `offset` inside them.
unsafe fn read(base: *mut u8, offset: usize) -> u32 {
    // SAFETY: the caller's promise; `read_volatile` because reading a
    // device register is an action, never to be skipped or repeated.
    unsafe { base.add(offset).cast::<u32>().read_volatile() }
}

/// # Safety
/// As for `read`.
unsafe fn write(base: *mut u8, offset: usize, value: u32) {
    // SAFETY: as for `read`.
    unsafe { base.add(offset).cast::<u32>().write_volatile(value) }
}

/// What `init` found.
pub struct Info {
    pub version: u8,
    /// How many interrupt numbers the controller has.
    pub lines: u32,
}

/// Sets the controller up, with every interrupt disabled, and this
/// CPU ready to be interrupted by any that is enabled later.
///
/// # Safety
/// Once, on the boot CPU, with interrupts masked, after `mm::init`;
/// `controller` must describe the machine's controller.
pub unsafe fn init(controller: InterruptController) -> Info {
    let dist = mm::map_mmio(controller.distributor.start, controller.distributor.size.max(0x1_0000)) as *mut u8;
    let version = controller.version;

    // SAFETY: (this block is the whole of the set-up) the registers were
    // mapped a moment ago from the addresses the machine gave, and every
    // offset is one the controller's manual defines.
    let lines = unsafe {
        // Off while it is set up.
        write(dist, distributor::CONTROL, 0);
        let lines = gic::lines(read(dist, distributor::TYPE));
        // Nothing enabled: whatever a loader left on is not ours.
        for id in (32..lines).step_by(32) {
            write(dist, gic::bit(distributor::CLEAR_ENABLE, id).0, u32::MAX);
        }
        lines
    };

    let per_cpu = if version == 2 {
        let cpu = mm::map_mmio(controller.per_cpu.start, controller.per_cpu.size.max(0x1000)) as *mut u8;
        // SAFETY: as above.
        unsafe {
            // This CPU's private interrupts (0-31) off too; they are
            // in the distributor, which shows each CPU its own.
            write(dist, distributor::CLEAR_ENABLE, u32::MAX);
            write(cpu, cpu_interface::PRIORITY_MASK, gic::ANY_PRIORITY);
            write(cpu, cpu_interface::CONTROL, cpu_interface::ENABLE);
            write(dist, distributor::CONTROL, distributor::ENABLE);
            TARGET[0].store(own_target(dist), Ordering::Relaxed);
        }
        cpu
    } else {
        // SAFETY: as above; the system registers are described where
        // they are written.
        unsafe { init_v3(dist, controller) }
    };

    DISTRIBUTOR.store(dist, Ordering::Relaxed);
    PER_CPU[0].store(per_cpu, Ordering::Relaxed);
    BOOT_MPIDR.store(mpidr(), Ordering::Relaxed);
    VERSION.store(version, Ordering::Relaxed);
    LINES.store(lines, Ordering::Relaxed);
    Info { version, lines }
}

/// Version 3's share of `init`. Returns this CPU's redistributor.
///
/// # Safety
/// As for `init`.
unsafe fn init_v3(dist: *mut u8, controller: InterruptController) -> *mut u8 {
    let frames = mm::map_mmio(controller.per_cpu.start, controller.per_cpu.size) as *mut u8;
    FRAMES.store(frames, Ordering::Relaxed);
    FRAMES_SIZE.store(controller.per_cpu.size as usize, Ordering::Relaxed);
    // SAFETY: mapped a moment ago, all of it.
    let mine = unsafe { own_redistributor() };

    // SAFETY: as for `init`.
    unsafe {
        // The distributor: version 3's own way of naming CPUs, and
        // group 1 on. Then wait: this register says when it has taken.
        write(dist, distributor::CONTROL, distributor::AFFINITY_ROUTING | distributor::ENABLE_GROUP_1);
        while read(dist, distributor::CONTROL) & distributor::WRITE_PENDING != 0 {
            core::hint::spin_loop();
        }

    }
    // SAFETY: this CPU's redistributor, found above.
    unsafe { wake_v3(mine) };
    mine
}

/// Version 3: finds this CPU's redistributor. They lie one after
/// another, each naming its CPU, the last one marked.
///
/// # Safety
/// After `FRAMES` and `FRAMES_SIZE` are set, by `init_v3`.
unsafe fn own_redistributor() -> *mut u8 {
    let frames = FRAMES.load(Ordering::Relaxed);
    let size = FRAMES_SIZE.load(Ordering::Relaxed);
    let mpidr = mpidr();
    let mut offset = 0;
    loop {
        assert!(
            offset + redistributor::STRIDE <= size,
            "no redistributor for this CPU ({mpidr:#x}) in the range the device tree gives"
        );
        // SAFETY: inside the mapped range, checked above. TYPE is 64
        // bits, read as two halves.
        let (low, high) = unsafe {
            let frame = frames.add(offset);
            (read(frame, redistributor::TYPE), read(frame, redistributor::TYPE + 4))
        };
        let type_register = u64::from(high) << 32 | u64::from(low);
        if gic::is_redistributor_of(type_register, mpidr) {
            // SAFETY: inside the mapped range.
            return unsafe { frames.add(offset) };
        }
        assert!(type_register & redistributor::LAST == 0, "no redistributor names this CPU ({mpidr:#x})");
        offset += redistributor::STRIDE;
    }
}

/// Version 3, on each CPU: its redistributor woken, its private
/// interrupts off, and the CPU's own registers set to take interrupts.
///
/// # Safety
/// `mine` is this CPU's redistributor; interrupts masked.
unsafe fn wake_v3(mine: *mut u8) {
    // SAFETY: the caller's promise; every offset is one the manual defines.
    unsafe {
        // Tell the controller this CPU is awake, and wait until it
        // agrees.
        write(mine, redistributor::WAKE, read(mine, redistributor::WAKE) & !redistributor::PROCESSOR_SLEEP);
        while read(mine, redistributor::WAKE) & redistributor::CHILDREN_ASLEEP != 0 {
            core::hint::spin_loop();
        }

        // This CPU's private interrupts: all off, all in group 1 (the
        // group an ordinary kernel is allowed; group 0 is firmware's).
        let private = mine.add(redistributor::PRIVATE);
        write(private, distributor::CLEAR_ENABLE, u32::MAX);
        write(private, distributor::GROUP, u32::MAX);

        // The CPU's side, in its own registers. First ask for those
        // registers at all (SRE); then let every priority through, and
        // switch group 1 on.
        asm!(
            "mrs {t}, icc_sre_el1",
            "orr {t}, {t}, #1",
            "msr icc_sre_el1, {t}",
            "isb",
            "mov {t}, #0xff",
            "msr icc_pmr_el1, {t}",
            "mov {t}, #1",
            "msr icc_igrpen1_el1, {t}",
            "isb",
            t = out(reg) _,
            options(nomem, nostack, preserves_flags),
        );
    }
}

/// Sets up the part of the controller that belongs to this CPU - one
/// started after the first, whose number for the kernel is `cpu`. The
/// part all CPUs share was set up by `init`.
///
/// # Safety
/// Once on each such CPU, interrupts masked, after `init` on the boot
/// CPU and after this CPU's number is what `super::cpu()` returns.
pub unsafe fn init_here(cpu: usize) {
    let dist = DISTRIBUTOR.load(Ordering::Relaxed);
    if VERSION.load(Ordering::Relaxed) == 2 {
        let interface = PER_CPU[0].load(Ordering::Relaxed);
        PER_CPU[cpu].store(interface, Ordering::Relaxed);
        // SAFETY: mapped by `init`. The first registers of the
        // distributor and the whole CPU interface show each CPU its
        // own: this is the same code as the boot CPU's, and other
        // registers.
        unsafe {
            write(dist, distributor::CLEAR_ENABLE, u32::MAX);
            write(interface, cpu_interface::PRIORITY_MASK, gic::ANY_PRIORITY);
            write(interface, cpu_interface::CONTROL, cpu_interface::ENABLE);
            TARGET[cpu].store(own_target(dist), Ordering::Relaxed);
        }
    } else {
        // SAFETY: `init_v3` ran on the boot CPU.
        unsafe {
            let mine = own_redistributor();
            PER_CPU[cpu].store(mine, Ordering::Relaxed);
            wake_v3(mine);
        }
    }
}

/// Sends software interrupt `id` to the CPU the kernel numbers `cpu`,
/// whose `MPIDR_EL1` is `mpidr`. Returns at once: the other CPU takes
/// it when it next can.
pub fn send(cpu: usize, mpidr: u64, id: u32) {
    if VERSION.load(Ordering::Relaxed) == 2 {
        let target = TARGET[cpu].load(Ordering::Relaxed);
        // SAFETY: the distributor, mapped by `init`; one write sends.
        unsafe { write(DISTRIBUTOR.load(Ordering::Relaxed), distributor::SOFTWARE, gic::software_v2(target, id)) };
    } else if let Some(value) = gic::software_v3(mpidr, id) {
        // SAFETY: writing the register is how the interrupt is sent.
        unsafe { asm!("msr icc_sgi1r_el1, {}", "isb", in(reg) value, options(nomem, nostack, preserves_flags)) };
    }
}

/// Lets interrupt `id` through to this CPU.
pub fn enable(id: u32) {
    let version = VERSION.load(Ordering::Relaxed);
    assert!(version != 0, "gic::enable before gic::init");
    let dist = DISTRIBUTOR.load(Ordering::Relaxed);
    // Where the interrupt's registers are: a shared one's in the
    // distributor; a private one's too in version 2, but in this CPU's
    // redistributor in version 3.
    let base = if version == 3 && gic::kind(id) != Kind::Shared {
        // SAFETY: the redistributor's second frame is part of what was mapped.
        unsafe { per_cpu().add(redistributor::PRIVATE) }
    } else {
        dist
    };
    let (enable, mask) = gic::bit(distributor::SET_ENABLE, id);
    // SAFETY: registers mapped by `init`; the offsets are the
    // interrupt's own priority byte, group bit and enable bit.
    unsafe {
        base.add(gic::byte(distributor::PRIORITY, id)).write_volatile(gic::PRIORITY);
        if version == 2 && gic::kind(id) == Kind::Shared {
            // To the boot CPU, whoever is asking.
            base.add(gic::byte(distributor::TARGETS, id)).write_volatile(TARGET[0].load(Ordering::Relaxed));
        }
        if version == 3 {
            let (group, bit) = gic::bit(distributor::GROUP, id);
            write(base, group, read(base, group) | bit);
        }
        write(base, enable, mask);
    }
}

/// Lets a device's interrupt - a shared one, `id` 32 or more -
/// through, to this CPU, read as the machine's description says
/// (`edge` or level).
///
/// The order: off first, since how a line is read may only be changed
/// while it is off; then how, and to whom; then on.
pub fn enable_shared(id: u32, edge: bool) {
    assert!(has_shared(id), "interrupt {id} is not a shared one this controller has");
    disable(id);
    let version = VERSION.load(Ordering::Relaxed);
    let dist = DISTRIBUTOR.load(Ordering::Relaxed);
    let (config, edge_bit) = gic::trigger(id);
    // SAFETY: the distributor, mapped by `init`; `id` is below the
    // number of lines it has (checked above).
    unsafe {
        let now = read(dist, config);
        write(dist, config, if edge { now | edge_bit } else { now & !edge_bit });
        if version == 3 {
            // To the boot CPU, whoever is asking.
            let route = gic::route(BOOT_MPIDR.load(Ordering::Relaxed));
            let at = distributor::ROUTE + 8 * id as usize;
            write(dist, at, route as u32);
            write(dist, at + 4, (route >> 32) as u32);
        }
    }
    enable(id);
}

/// "Which interrupt?" Taking the answer tells the controller this CPU
/// has it, so each `Some` must be followed by `end`.
pub fn acknowledge() -> Option<(u32, u32)> {
    let version = VERSION.load(Ordering::Relaxed);
    let raw = if version == 2 {
        // SAFETY: the CPU interface, mapped by `init`.
        unsafe { read(per_cpu(), cpu_interface::ACKNOWLEDGE) }
    } else {
        let value: u64;
        // SAFETY: reading the register acknowledges, which is the point.
        unsafe { asm!("mrs {}, icc_iar1_el1", out(reg) value, options(nomem, nostack, preserves_flags)) };
        value as u32
    };
    gic::acknowledged(raw, version).map(|id| (id, raw))
}

/// "Done": `raw` is what `acknowledge` returned beside the number.
pub fn end(raw: u32) {
    if VERSION.load(Ordering::Relaxed) == 2 {
        // SAFETY: the CPU interface, mapped by `init`.
        unsafe { write(per_cpu(), cpu_interface::END, raw) };
    } else {
        // SAFETY: writing the number back is how an interrupt is ended.
        unsafe { asm!("msr icc_eoir1_el1, {}", in(reg) u64::from(raw), options(nomem, nostack, preserves_flags)) };
    }
}

/// Stops interrupt `id` reaching any CPU - for one that arrived and
/// that nothing here knows how to handle: left enabled, a device that
/// keeps its line raised would interrupt for ever.
pub fn disable(id: u32) {
    let version = VERSION.load(Ordering::Relaxed);
    let base = if version == 3 && gic::kind(id) != Kind::Shared {
        // SAFETY: as in `enable`.
        unsafe { per_cpu().add(redistributor::PRIVATE) }
    } else {
        DISTRIBUTOR.load(Ordering::Relaxed)
    };
    let (offset, mask) = gic::bit(distributor::CLEAR_ENABLE, id);
    // SAFETY: as in `enable`.
    unsafe { write(base, offset, mask) };
}
