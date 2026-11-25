//! The kernel's random numbers: RANDOM (getrandom in libc).
//!
//! The generator is chacha.rs's; this file is where its samples come
//! from and how programs reach it.
//!
//! **At boot** (`init`, right after the clock starts) the key is stirred
//! with:
//!   - the CPU's own random numbers, if it has an instruction for them
//!     (RDSEED or RDRAND on x86, RNDR on AArch64): 8 of them;
//!   - the date and the counter's value;
//!   - `JITTER_SAMPLES` timings of a small loop that touches memory.
//!     How many counter steps the loop takes varies from run to run -
//!     caches, the memory bus, interrupts on the host, other CPUs - and
//!     its lowest bits are what is kept.
//!
//! **Afterwards**, every timer tick adds the counter's value to a pool
//! word (one rotate and one XOR), and so does every RANDOM call. The
//! pool goes into the key at the next request (`Generator::split`).
//!
//! RANDOM never blocks: by the time a program can call it, the boot
//! samples are in. Linux blocks getrandom until it judges it has 256
//! bits of entropy; TheTaaJ does not try to judge (see the journal,
//! entry on step B, for why).

use core::sync::atomic::{AtomicU64, Ordering};

use kernel64::chacha::{self, Generator};

use crate::arch;
use crate::irqlock::IrqLock;

static GENERATOR: IrqLock<Generator> = IrqLock::new(Generator::new());
/// Samples since the last request (chacha::pool_add).
static POOL: AtomicU64 = AtomicU64::new(0);

/// How many loop timings are stirred in at boot.
const JITTER_SAMPLES: usize = 256;

/// Adds `sample` to the pool: cheap enough for an interrupt.
pub fn add(sample: u64) {
    let _ = POOL.try_update(Ordering::Relaxed, Ordering::Relaxed, |pool| Some(chacha::pool_add(pool, sample)));
}

/// Seeds the generator. Returns what went in, for the boot log.
pub fn init(date: u64) -> (&'static str, usize, usize) {
    let mut generator = GENERATOR.lock();
    let mut from_cpu = 0;
    for _ in 0..8 {
        if let Some(value) = arch::hardware_random() {
            generator.stir(value);
            from_cpu += 1;
        }
    }
    generator.stir(date);
    generator.stir(arch::clock());
    // A small buffer, walked with a stride that defeats the cache's
    // neighbours, its contents changed each time so that the work
    // cannot be skipped.
    let mut buffer = [0u64; 512];
    for round in 0..JITTER_SAMPLES {
        let before = arch::clock();
        let mut index = round;
        for step in 0..64 {
            index = (index * 97 + step + 1) % buffer.len();
            buffer[index] = buffer[index].wrapping_add(before ^ step as u64);
        }
        let took = arch::clock().wrapping_sub(before);
        // The low bits vary; the time itself adds the rest of `before`.
        generator.stir(took ^ before.rotate_left(32) ^ buffer[index]);
    }
    (arch::hardware_random_name(), from_cpu, JITTER_SAMPLES)
}

/// RANDOM: fills `length` bytes at `buffer`.
pub fn fill(buffer: u64, length: u64) -> Result<u64, abi::Error> {
    if length > abi::RANDOM_MAX {
        return Err(abi::Error::InvalidArgument);
    }
    add(arch::clock());
    let pool = POOL.swap(0, Ordering::Relaxed);
    let key = GENERATOR.lock().split(pool);
    // Made and copied a page at a time, with no lock held: the key is
    // this request's alone.
    let mut chunk = [0u8; 4096];
    let mut counter = 0;
    let mut done = 0;
    while done < length {
        let n = (length - done).min(chunk.len() as u64) as usize;
        counter = chacha::stream(&key, counter, &mut chunk[..n]);
        crate::process::copy_to_user(buffer + done, &chunk[..n])?;
        done += n as u64;
    }
    Ok(length)
}
