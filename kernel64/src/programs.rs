//! The user programs built into the kernel image.
//!
//! Until there is a file system to load them from, programs are compiled
//! first (the `user/` crate) and their ELF files embedded here. The
//! loader treats them exactly as it would a file from disk.
//!
//! The `user/` crate is compiled once for each CPU, into a directory
//! named after the target; each kernel embeds its own. A few programs
//! exist for one CPU only - those that drive a PC's devices through
//! I/O ports, or test something only one CPU has.

#[cfg(target_arch = "x86_64")]
macro_rules! program {
    ($name:literal) => {
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../user/target/x86_64-unknown-none/release/", $name))
    };
}

#[cfg(target_arch = "aarch64")]
macro_rules! program {
    ($name:literal) => {
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../user/target/aarch64-unknown-none-softfloat/release/",
            $name
        ))
    };
}

pub static HELLO: &[u8] = program!("hello");
pub static PEEK: &[u8] = program!("peek");
pub static CLI: &[u8] = program!("cli");
pub static SPIN: &[u8] = program!("spin");
pub static ECHOD: &[u8] = program!("echod");
pub static ECHOC: &[u8] = program!("echoc");
pub static FS: &[u8] = program!("fs");
pub static FSCHECK: &[u8] = program!("fscheck");
pub static MEMRO: &[u8] = program!("memro");
pub static HEAPCHECK: &[u8] = program!("heapcheck");
pub static FOREVER: &[u8] = program!("forever");
/// The disk driver: the same program for a disk on a PCI bus and one
/// at a place in memory.
pub static BLK: &[u8] = program!("blk");

/// The system's own programs: the first process, and what it starts.
pub static INIT: &[u8] = program!("init");
pub static SHELL: &[u8] = program!("shell");
pub static REGISTRY: &[u8] = program!("registry");

/// PC only: they use I/O ports, a segment register, or devices only
/// the PC kernel knows how to hand out so far.
#[cfg(target_arch = "x86_64")]
mod pc {
    pub static GS: &[u8] = program!("gs");
    pub static PORT: &[u8] = program!("port");
    pub static KBD: &[u8] = program!("kbd");
    pub static DISPLAY: &[u8] = program!("display");
}
#[cfg(target_arch = "x86_64")]
pub use pc::*;

/// AArch64 only: it checks registers by name.
#[cfg(target_arch = "aarch64")]
pub static REGS: &[u8] = program!("regs");
/// AArch64 only, so far: the disk driver's test. (The PC's disk holds
/// the system's files, and is tested by using them.)
#[cfg(target_arch = "aarch64")]
pub static BLKCHECK: &[u8] = program!("blkcheck");
/// AArch64 only: the serial port's driver - keyboard and screen in one.
#[cfg(target_arch = "aarch64")]
pub static SERIAL: &[u8] = program!("serial");

/// The programs the user-mode self-test runs, and how each must end:
/// with an exit code of its own, or killed for breaking a rule.
#[cfg(target_arch = "x86_64")]
pub static USER_CHECK: &[(&str, &[u8], i64)] = &[
    ("hello", HELLO, 42),
    ("peek", PEEK, abi::KILLED),
    ("cli", CLI, abi::KILLED),
    ("port", PORT, abi::KILLED),
    ("memro", MEMRO, abi::KILLED),
    ("spin", SPIN, 0),
    ("heapcheck", HEAPCHECK, 0),
    ("gs", GS, abi::KILLED),
];

#[cfg(target_arch = "aarch64")]
pub static USER_CHECK: &[(&str, &[u8], i64)] = &[
    ("hello", HELLO, 42),
    ("peek", PEEK, abi::KILLED),
    ("cli", CLI, abi::KILLED),
    ("memro", MEMRO, abi::KILLED),
    ("spin", SPIN, 0),
    ("heapcheck", HEAPCHECK, 0),
    ("regs", REGS, 0),
];

/// The programs a launcher can start, by name.
#[cfg(target_arch = "x86_64")]
static PROGRAMS: &[(&str, &[u8])] = &[
    ("kbd", KBD),
    ("shell", SHELL),
    ("display", DISPLAY),
    ("fs", FS),
    ("blk", BLK),
    ("registry", REGISTRY),
    ("echod", ECHOD),
    ("echoc", ECHOC),
    ("hello", HELLO),
    ("peek", PEEK),
];

/// On AArch64 the serial port's driver takes the place of the
/// keyboard's and the display's.
#[cfg(target_arch = "aarch64")]
static PROGRAMS: &[(&str, &[u8])] = &[
    ("serial", SERIAL),
    ("shell", SHELL),
    ("fs", FS),
    ("blk", BLK),
    ("registry", REGISTRY),
    ("echod", ECHOD),
    ("echoc", ECHOC),
    ("hello", HELLO),
    ("peek", PEEK),
];

pub fn find(name: &str) -> Option<(&'static str, &'static [u8])> {
    PROGRAMS.iter().find(|(n, _)| *n == name).copied()
}
