//! Making every CPU forget a translation that was taken away.
//!
//! Each CPU keeps the translations it has used in its TLB, and keeps
//! using them after the page table entry is gone, until it is told to
//! forget. While a process ran on one CPU at a time that CPU was the
//! only one to tell, and it was the one doing the unmapping. Now that a
//! program's threads run on several CPUs at once, a page one thread
//! unmaps may still be in another CPU's TLB - and if its frame were
//! freed and handed to someone else, that thread could go on reading
//! and writing someone else's memory.
//!
//! AArch64 settles it in the instruction: `tlbi vaae1is` (mmu::flush)
//! is broadcast to every CPU, and the `dsb` after it waits until all of
//! them have done it. x86 has nothing like that: `invlpg` is for this
//! CPU only. The others must be interrupted and asked - a "TLB
//! shootdown", which is what this does there:
//!
//!   1. the page table entries are taken out (under the process's locks);
//!   2. every other CPU now running the process's tables is marked, and
//!      sent TLB_VECTOR; each reloads CR3 - forgetting all it held for
//!      the process - and clears its mark;
//!   3. the sender waits until every mark is clear, and only then are
//!      the frames freed.
//!
//! Which CPUs: those whose ACTIVE_ROOT (threads.rs) is the process's.
//! One that loads the tables after step 1 reads the new entries, as a
//! reload starts with an empty TLB. A CPU that ran the process and has
//! moved on reloaded CR3 when it did.
//!
//! The sender waits with interrupts off, so it cannot be moved to
//! another CPU meanwhile, and holds no lock: a CPU waiting for a lock
//! it held, with its interrupts off, would never answer. Two CPUs may
//! be shooting at each other at once; each, while it waits, answers
//! its own mark, so neither waits for ever.

#[cfg(target_arch = "x86_64")]
mod x86 {
    use core::sync::atomic::{AtomicBool, Ordering};

    use kernel64::smp::MAX_CPUS;

    use crate::arch;

    /// Set by a CPU that wants this one to reload CR3; cleared by this
    /// one once it has.
    static PENDING: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];

    /// Every CPU now running the tables at `root` forgets what it
    /// remembered of them, this one included. Returns when they have.
    /// Called after page table entries were removed, before what they
    /// pointed at is reused.
    pub fn shootdown(root: u64) {
        let were_enabled = arch::disable_interrupts();
        let me = arch::cpu();
        // The removals must be seen before the CPUs are chosen: a CPU
        // that loads the tables after this sees them.
        core::sync::atomic::fence(Ordering::SeqCst);
        let others = crate::threads::cpus_running(root) & !(1 << me);
        for (cpu, pending) in PENDING.iter().enumerate() {
            if others & (1 << cpu) != 0 {
                pending.store(true, Ordering::SeqCst);
            }
        }
        if others != 0 {
            arch::smp::send_tlb_flush(others);
        }
        reload();
        while PENDING.iter().enumerate().any(|(cpu, pending)| others & (1 << cpu) != 0 && pending.load(Ordering::SeqCst)) {
            // Another CPU may be waiting on this one, with its
            // interrupts off too: answer it here.
            answer();
            core::hint::spin_loop();
        }
        arch::restore_interrupts(were_enabled);
    }

    /// TLB_VECTOR arrived, or a waiting CPU looks: if this CPU was asked,
    /// it forgets.
    pub fn answer() {
        let me = arch::cpu();
        // Cleared first, then reloaded: a request made after the clear
        // is answered by the reload, or by the next interrupt.
        if PENDING[me].swap(false, Ordering::SeqCst) {
            reload();
        }
    }

    fn reload() {
        // SAFETY: CR3 written with what it holds - the tables do not
        // change, only what the CPU remembered of them (all but global
        // pages, which are the kernel's) is dropped.
        unsafe {
            core::arch::asm!("mov {t}, cr3", "mov cr3, {t}", t = out(reg) _, options(nostack, preserves_flags));
        }
    }
}

#[cfg(target_arch = "x86_64")]
pub use x86::{answer, shootdown};

/// AArch64: mmu::flush was already seen by every CPU.
#[cfg(target_arch = "aarch64")]
pub fn shootdown(_root: u64) {}
