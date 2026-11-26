//! Processes: an address space, and the threads running in it - one at
//! first, more if the program asks (THREAD_CREATE).
//!
//! The lower half of the address space is the process's own; the upper
//! half is the kernel, shared by every process but inaccessible from
//! ring 3 (no USER bit). A process's frames are marked OWNED, so tearing
//! down its page tables returns every one of them.
//!
//! A process also has a handle table (cap.rs): the only way it can reach
//! any kernel object. When it ends - by exiting or being killed - every
//! handle is closed at once, so whoever was talking to it finds out.
//!
//! Its threads share all of that. A process ends when one of them calls
//! exit or breaks a rule, or when the last one ends by itself; its other
//! threads then end too, each the next time it heads for user mode, as
//! when the process is killed (`kill`).
//!
//! Memory comes two ways. A program's file is loaded whole when it
//! starts, and a memory object is mapped whole. Reserved memory - the
//! stack, and what MEMORY_RESERVE gives - is only address space at
//! first: a page gets a frame the first time it is touched (`fault_in`),
//! when the CPU finds nothing there and faults, or when a system call
//! reads or writes it (`user_pages`). So a program can have a 64 MiB
//! stack and pay for the 20 KiB it uses. Every page taken is counted
//! against the process's limit (account.rs).

use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

use abi::Error;
use kernel64::account::{self, Account};
use kernel64::cap::{CapTable, Rights};
use kernel64::elf::{self, ElfError};
use kernel64::ioports::PortBitmap;
use kernel64::regions::Regions;
use kernel64::memory::addr::{phys_to_virt, PhysAddr, VirtAddr, PAGE_SIZE};
use kernel64::paging::{Flags, PageSize, PageTables};
use kernel64::sched::ThreadId;

use crate::arch::{mmu, syscall};
use crate::irqlock::IrqLock;
use crate::ipc::Object;
use crate::memory_object::MemoryObject;
use crate::{mm, threads};

/// The user stack ends just below this address: 8 MiB of it, or what
/// the program asked for (elf::STACK_DEFAULT, elf::STACK_MAX) - reserved,
/// a page taken as it is touched. The page below it is never filled, so
/// overflowing the stack faults.
const USER_STACK_TOP: u64 = 0x0000_7FFF_FFFF_0000;

/// Where memory objects are mapped, and reserved memory is put: far from
/// the program (which sits low, near 4 MiB) and the stack (at the very
/// top). The page below it is never a program's (`load`), so the first
/// region has an unmapped page under it like every other.
const MAPPING_AREA: core::ops::Range<u64> = 0x0000_1000_0000_0000..0x0000_7000_0000_0000;

/// What a region of the mapping area holds.
enum Mapping {
    /// A memory object, every page mapped when it was. The region holds
    /// a reference, so the frames outlive any handle while mapped.
    /// (Held, never read: dropping it is what frees the frames.)
    Object(#[allow(dead_code)] Arc<MemoryObject>),
    /// Reserved memory (MEMORY_RESERVE): each page filled when first
    /// touched, with a frame of the process's own.
    Reserved { writable: bool },
}

const RUNNING: i64 = i64::MIN;

pub struct Process {
    pub pid: u64,
    pub name: String,
    /// Physical address of the PML4 - what CR3 holds while this runs.
    root: u64,
    tables: IrqLock<Option<PageTables>>,
    exit_code: AtomicI64,
    /// Set by the first thread to end the process (`end_current`): it,
    /// and only it, does the ending. Separate from `exit_code`, which is
    /// published last, once the ending is done (see there).
    ending: AtomicBool,
    /// Set by `kill`: the process ends when it next heads for user mode.
    kill_requested: AtomicBool,
    /// The exit code that end will have: STOPPED, or a signal's.
    kill_code: AtomicI64,
    /// Signals (abi::signal), a bit for each: those sent and caught but
    /// not yet delivered, and what signal_setup said - which are caught,
    /// which ignored, and where caught ones go.
    signals_pending: AtomicU64,
    signals_caught: AtomicU64,
    signals_ignored: AtomicU64,
    signal_entry: AtomicU64,
    /// The caught signals delivered on a thread's signal stack, if it
    /// has one (SA_ONSTACK).
    signals_on_stack: AtomicU64,
    /// Whom to send what when this process ends (PROCESS_NOTIFY): how
    /// a program learns its child ended (SIGCHLD), the child having
    /// been started by init. Weak: a process that went first is
    /// skipped. None once the process is ending and has sent them all:
    /// a watcher who comes later is told at once (`notify_on_end`).
    notify: IrqLock<Option<Vec<(Weak<Process>, u64)>>>,
    /// Its ELF program headers (PROGRAM_HEADERS).
    headers: Vec<u8>,
    /// Its threads that have not ended: whom a kill, or the end of the
    /// process, wakes. At most THREADS_MAX.
    threads: IrqLock<Vec<ThreadId>>,
    pub handles: IrqLock<CapTable<Object>>,
    /// The I/O ports it may use directly; loaded into the TSS whenever
    /// it runs. Granted for the life of the process.
    pub io_ports: IrqLock<PortBitmap>,
    /// Mapped memory objects and reserved memory. Taken before `tables`.
    mappings: IrqLock<Regions<Mapping>>,
    /// The stack of its first thread: reserved, filled as touched.
    stack: core::ops::Range<u64>,
    /// The pages it has taken, and may (account.rs). Shared with the
    /// memory objects it made, which give theirs back when they go.
    pub account: Arc<Account>,
}

#[derive(Debug)]
pub enum LoadError {
    /// Read when the error is printed (Debug), which the compiler's
    /// dead-code check does not count.
    #[allow(dead_code)]
    Elf(ElfError),
    OutOfMemory,
    /// More than the new program's memory limit.
    OverLimit,
}

impl Process {
    pub fn root(&self) -> u64 {
        self.root
    }

    /// The exit code, once the process has ended.
    pub fn exit_code(&self) -> Option<i64> {
        let code = self.exit_code.load(Ordering::Acquire);
        (code != RUNNING).then_some(code)
    }

    /// Whether someone has asked for this process to end.
    pub fn kill_requested(&self) -> bool {
        self.kill_requested.load(Ordering::Acquire)
    }
}

impl Process {
    /// Closes every handle - now, not when the last reference to the
    /// Process goes, which may be much later (a parent still holding it
    /// to read the exit code).
    fn close_all_handles(&self) {
        let objects = self.handles.lock().take_all();
        drop(objects); // outside the lock: closing channels wakes threads
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // The tables go first: they free the process's own (OWNED) frames
        // and leave mapped objects' frames alone. The mappings - the
        // references keeping those objects alive - are dropped after,
        // with the rest of the fields.
        if let Some(tables) = self.tables.lock().take() {
            mm::with_frames(|frames| tables.destroy(frames));
        }
    }
}

/// Maps all of `object` into the current process; returns the address.
pub fn map_memory(object: &Arc<MemoryObject>, writable: bool) -> Result<u64, Error> {
    let process = current().ok_or(Error::InvalidArgument)?;
    let mut mappings = process.mappings.lock();
    let start = mappings.allocate(object.size(), Mapping::Object(Arc::clone(object))).ok_or(Error::OutOfMemory)?;

    // Never executable: shared memory is data. A page another process
    // can write must not be one this process can run (W^X across
    // processes).
    let mut flags = Flags::USER | mm::no_execute() | object.cache;
    if writable {
        flags |= Flags::WRITABLE;
    }
    let mut tables = process.tables.lock();
    let tables = tables.as_mut().ok_or(Error::InvalidArgument)?;
    let mapped = mm::with_frames(|frames| {
        for (n, &frame) in object.frames().iter().enumerate() {
            let page = VirtAddr::new(start + n as u64 * PAGE_SIZE);
            if tables.map(page, frame, PageSize::Size4K, flags, frames).is_err() {
                return Err(n); // out of frames for page tables
            }
        }
        Ok(())
    });
    if let Err(done) = mapped {
        for n in 0..done as u64 {
            tables.unmap(VirtAddr::new(start + n * PAGE_SIZE));
        }
        mappings.remove(start);
        return Err(Error::OutOfMemory);
    }
    Ok(start)
}

/// Removes the current process's mapping, or reserved memory, that
/// starts at `start`.
///
/// Its pages leave the tables first, under the locks. Then every CPU
/// that may remember them forgets (tlb.rs) - other threads of this
/// process may be running on other CPUs - and only then are frames
/// given back: an object's when its last reference goes, reserved
/// memory's here.
pub fn unmap_memory(start: u64) -> Result<u64, Error> {
    let process = current().ok_or(Error::InvalidArgument)?;
    let mut mappings = process.mappings.lock();
    let (range, mapping) = mappings.remove(start).ok_or(Error::InvalidArgument)?;
    let mut taken = Vec::new();
    {
        let mut tables = process.tables.lock();
        let tables = tables.as_mut().ok_or(Error::InvalidArgument)?;
        for page in range.step_by(PAGE_SIZE as usize) {
            let frame = tables.unmap(VirtAddr::new(page));
            // This CPU's TLB; the others' below.
            mmu::flush(page);
            if let (Some(frame), Mapping::Reserved { .. }) = (frame, &mapping) {
                taken.push(frame);
            }
        }
    }
    drop(mappings);
    crate::tlb::shootdown(process.root);
    if !taken.is_empty() {
        mm::with_frames(|frames| taken.iter().for_each(|&frame| frames.free(frame)));
        process.account.uncharge(taken.len() as u64);
    }
    drop(mapping); // an object's last reference, maybe: frees its frames
    Ok(0)
}

/// MEMORY_RESERVE: `size` bytes of address space in the current process,
/// nothing in them yet; returns where.
pub fn reserve_memory(size: u64, writable: u64) -> Result<u64, Error> {
    let writable = match writable {
        0 => false,
        1 => true,
        _ => return Err(Error::InvalidArgument),
    };
    if size == 0 || size > abi::RESERVE_MAX {
        return Err(Error::InvalidArgument);
    }
    let process = current().ok_or(Error::InvalidArgument)?;
    let start = process.mappings.lock().allocate(size, Mapping::Reserved { writable });
    start.ok_or(Error::OutOfMemory)
}

/// MEMORY_LIMIT: lowers the current process's limit to `bytes`, if
/// lower (0 only asks); the limit now, in bytes.
pub fn memory_limit(bytes: u64) -> Result<u64, Error> {
    let process = current().ok_or(Error::InvalidArgument)?;
    let pages = if bytes == 0 { process.account.limit() } else { process.account.lower_limit(bytes / PAGE_SIZE) };
    Ok(pages * PAGE_SIZE)
}

/// MEMORY_USED: the bytes counted against the current process.
pub fn memory_used() -> Result<u64, Error> {
    Ok(current().ok_or(Error::InvalidArgument)?.account.used() * PAGE_SIZE)
}

/// What became of a page fault in user mode.
pub enum Fault {
    /// The page was reserved, and has a frame now (or had one already,
    /// filled by another thread meanwhile): run the instruction again.
    Filled,
    /// Not reserved, or not for this kind of access: the program broke
    /// a rule.
    NotOurs,
    /// Reserved, but the process is at its limit: (pages used, limit).
    OverLimit(u64, u64),
    /// Reserved, but the machine has no frame left.
    NoMemory,
}

/// A user-mode access to `address` (a write if `write`) found no page.
/// If the address is reserved, the page is filled now.
pub fn fault_in(address: u64, write: bool) -> Fault {
    let Some(process) = current() else { return Fault::NotOurs };
    let mappings = process.mappings.lock();
    let mut tables = process.tables.lock();
    let Some(tables) = tables.as_mut() else { return Fault::NotOurs };
    fill(&process, &mappings, tables, address, write)
}

/// Kills the current process for a page it could not have - the fault
/// handlers' answer to OverLimit and NoMemory.
pub fn out_of_memory(fault: Fault) -> ! {
    match fault {
        Fault::OverLimit(used, limit) => {
            let mib = |pages: u64| (pages * PAGE_SIZE) >> 20;
            end_current(
                abi::KILLED,
                format_args!("killed: out of memory - {} MiB used, its limit is {} MiB", mib(used), mib(limit)),
            )
        }
        _ => end_current(abi::KILLED, format_args!("killed: out of memory - the machine has none left")),
    }
}

/// `fault_in`'s work, with both locks held (mappings, then tables).
fn fill(process: &Process, mappings: &Regions<Mapping>, tables: &mut PageTables, address: u64, write: bool) -> Fault {
    let writable = if process.stack.contains(&address) {
        true
    } else {
        match mappings.find(address) {
            Some((_, Mapping::Reserved { writable })) => *writable,
            _ => return Fault::NotOurs,
        }
    };
    if write && !writable {
        return Fault::NotOurs;
    }
    let page = address & !(PAGE_SIZE - 1);
    // Two threads may touch the same new page at once: both fault, and
    // the second to get here finds it filled.
    if tables.translate(VirtAddr::new(page)).is_some() {
        return Fault::Filled;
    }
    if !process.account.charge(1) {
        return Fault::OverLimit(process.account.used(), process.account.limit());
    }
    let mut flags = Flags::USER | Flags::OWNED | mm::no_execute();
    if writable {
        flags |= Flags::WRITABLE;
    }
    let mapped = mm::with_frames(|frames| {
        let frame = frames.allocate()?;
        // SAFETY: a fresh frame, through the direct map. Zeroed: what
        // its last owner left there must not be seen.
        unsafe { phys_to_virt(frame).as_mut_ptr::<u8>().write_bytes(0, PAGE_SIZE as usize) };
        if tables.map(VirtAddr::new(page), frame, PageSize::Size4K, flags, frames).is_err() {
            frames.free(frame); // no frame left for a page table
            return None;
        }
        Some(())
    });
    if mapped.is_none() {
        process.account.uncharge(1);
        return Fault::NoMemory;
    }
    mmu::commit_mappings();
    Fault::Filled
}

static NEXT_PID: AtomicU64 = AtomicU64::new(1);

/// Loads and starts a program: `load`, then `start`.
pub fn spawn(
    name: &str,
    file: &[u8],
    handles: Vec<(Object, Rights)>,
) -> Result<(Arc<Process>, ThreadId), LoadError> {
    let process = load(name, file)?;
    let thread = start(&process, handles, 0);
    Ok((process.process, thread))
}

/// A loaded program, not yet started.
pub struct Loaded {
    process: Arc<Process>,
    entry: u64,
}

impl core::ops::Deref for Loaded {
    type Target = Arc<Process>;
    fn deref(&self) -> &Arc<Process> {
        &self.process
    }
}

/// The memory limit a program gets unless its parent's is lower, in
/// pages: account::default_limit of the machine's, worked out once.
fn default_limit() -> u64 {
    static LIMIT: AtomicU64 = AtomicU64::new(0);
    match LIMIT.load(Ordering::Relaxed) {
        0 => {
            let limit = account::default_limit(mm::boot_free_frames()).max(1);
            LIMIT.store(limit, Ordering::Relaxed);
            limit
        }
        limit => limit,
    }
}

/// Every process that still exists, for `process_list`. Weak: listing a
/// process must not keep it alive.
static ALL: IrqLock<Vec<alloc::sync::Weak<Process>>> = IrqLock::new(Vec::new());

/// The processes that exist (running, or ended but still referenced).
pub fn all() -> Vec<Arc<Process>> {
    let mut all = ALL.lock();
    all.retain(|p| p.strong_count() > 0);
    all.iter().filter_map(|p| p.upgrade()).collect()
}

/// Wakes everyone in `process_wait` whenever any process ends.
pub static EXITS: threads::WaitQueue = threads::WaitQueue::new();

/// Loads the ELF executable `file` into a new address space. Nothing
/// runs yet; this is the step that can fail.
///
/// `file` may be anything - a program from the disk is whatever someone
/// put there. `elf::parse` checks all of it; a program must also lie
/// below the mapping area, so no segment can sit where the kernel will
/// later place a memory object, or on the stack.
///
/// The file is read through `elf::Source`: a slice (the programs built
/// into the kernel), or the pages of the memory object a program from
/// the disk came in, read where they are.
pub fn load(name: &str, file: &(impl elf::Source + ?Sized)) -> Result<Loaded, LoadError> {
    load_limited(name, file, None)
}

/// `load`, the new program's memory limit no more than `limit` pages.
///
/// The limit is the lowest of: the machine's default; that of the
/// process loading it; and `limit`, which spawn_image takes from the
/// program file's memory object - the limit of whoever made it. That is
/// how a program started through init (posix_spawn, the shell) gets
/// the limit of the one who asked, not init's: the asker read the file
/// into an object of its own, and cannot make one counted against
/// anyone else.
pub fn load_limited(name: &str, file: &(impl elf::Source + ?Sized), limit: Option<u64>) -> Result<Loaded, LoadError> {
    // Below the mapping area, and off its first page's neighbour.
    let image = elf::parse(file, MAPPING_AREA.start - PAGE_SIZE).map_err(LoadError::Elf)?;
    let mut pages = default_limit();
    if let Some(loader) = current() {
        pages = pages.min(loader.account.limit());
    }
    if let Some(limit) = limit {
        pages = pages.min(limit);
    }
    let account = Arc::new(Account::new(pages));

    // SAFETY: frames are reached through the direct map.
    let mut tables = mm::with_memory(|kernel, frames| unsafe {
        PageTables::new_user(frames, mm::DIRECT_MAP, kernel)
    })
    .ok_or(LoadError::OutOfMemory)?;

    let loaded = mm::with_frames(|frames| -> Result<(), LoadError> {
        // A page at `virt`: zeroed, then `bytes` of the file from
        // `from` written at `at` in it.
        let mut map_page = |virt: u64, flags: Flags, at: usize, from: u64, bytes: usize| {
            // Counted before it is taken. (If loading fails, the account
            // goes with the half-made process: nothing to give back.)
            if !account.charge(1) {
                return Err(LoadError::OverLimit);
            }
            let frame = frames.allocate().ok_or(LoadError::OutOfMemory)?;
            let page = phys_to_virt(frame).as_mut_ptr::<u8>();
            // SAFETY: a fresh frame, reached through the direct map, and
            // nothing else refers to it yet.
            let contents = unsafe {
                page.write_bytes(0, PAGE_SIZE as usize);
                core::slice::from_raw_parts_mut(page, PAGE_SIZE as usize)
            };
            // (A page past the file's bytes - .bss, the stack - reads
            // nothing: its `from` may be past the end.)
            if bytes > 0 {
                file.read(from, &mut contents[at..at + bytes]);
            }
            // Instructions, written as data: some CPUs must be told
            // before they will run what was written.
            if !flags.contains(Flags::NO_EXECUTE) {
                mmu::code_written(page, PAGE_SIZE as usize);
            }
            let flags = flags | Flags::USER | Flags::OWNED;
            tables.map(VirtAddr::new(virt), frame, PageSize::Size4K, flags, frames).map_err(|_| {
                // Not mapped, so `destroy` will not find it: free it here.
                frames.free(frame);
                LoadError::OutOfMemory
            })
        };

        for segment in &image.segments {
            let mut flags = Flags::NONE;
            if segment.writable {
                flags |= Flags::WRITABLE;
            }
            if !segment.executable {
                flags |= mm::no_execute();
            }
            // The file's bytes occupy virtual [start, start + file length).
            let start = segment.virtual_range.start;
            let length = segment.file_range.len() as u64;
            let first = start & !(PAGE_SIZE - 1);
            for page in (first..segment.virtual_range.end).step_by(PAGE_SIZE as usize) {
                // The part of the file that lands in this page, and where.
                let from = page.max(start);
                let to = (page + PAGE_SIZE).min(start + length);
                let bytes = to.saturating_sub(from) as usize;
                let offset = segment.file_range.start as u64 + (from - start);
                map_page(page, flags, (from - page) as usize, offset, bytes)?;
            }
        }
        Ok(())
    });
    if let Err(error) = loaded {
        mm::with_frames(|frames| tables.destroy(frames));
        return Err(error);
    }

    let process = Arc::new(Process {
        pid: NEXT_PID.fetch_add(1, Ordering::Relaxed),
        name: String::from(name),
        root: tables.root().as_u64(),
        tables: IrqLock::new(Some(tables)),
        exit_code: AtomicI64::new(RUNNING),
        ending: AtomicBool::new(false),
        kill_requested: AtomicBool::new(false),
        kill_code: AtomicI64::new(abi::STOPPED),
        signals_pending: AtomicU64::new(0),
        signals_caught: AtomicU64::new(0),
        signals_ignored: AtomicU64::new(0),
        signal_entry: AtomicU64::new(0),
        signals_on_stack: AtomicU64::new(0),
        notify: IrqLock::new(Some(Vec::new())),
        headers: image.headers,
        threads: IrqLock::new(Vec::new()),
        handles: IrqLock::new(CapTable::new(abi::HANDLES_MAX)),
        io_ports: IrqLock::new(PortBitmap::NONE),
        mappings: IrqLock::new(Regions::new(MAPPING_AREA)),
        stack: USER_STACK_TOP - image.stack..USER_STACK_TOP,
        account,
    });
    ALL.lock().push(Arc::downgrade(&process));
    Ok(Loaded { process, entry: image.entry })
}

/// Starts a loaded program on a new thread, with `handles` as its startup
/// handles: numbered 1..=n in this order, n passed to `_start` in RDI,
/// and `argument` in RSI.
pub fn start(loaded: &Loaded, handles: Vec<(Object, Rights)>, argument: u64) -> ThreadId {
    let process = &loaded.process;
    let count = handles.len() as u64;
    {
        let mut table = process.handles.lock();
        for (n, (object, rights)) in handles.into_iter().enumerate() {
            let handle = table.insert(object, rights).expect("a fresh table has room");
            debug_assert_eq!(handle, n as u64 + 1, "startup handles are 1..=n");
        }
    }
    let entry = loaded.entry;
    // The stack pointer as `_start` expects to find it, which is the
    // calling convention's business and so each CPU's own.
    let stack = USER_STACK_TOP - syscall::ENTRY_STACK_GAP;
    // The list stays locked until the thread is in it: a thread that
    // ended before it was listed would find no thread of its process,
    // and take itself for the last.
    let mut list = process.threads.lock();
    let thread = threads::spawn_in(Some(Arc::clone(process)), "user", move || {
        // SAFETY: this thread runs in `process`'s address space (the
        // switch loaded its CR3 and kernel stack), which maps both.
        unsafe { syscall::enter_user(entry, stack, count, argument) }
    });
    list.push(thread);
    thread
}

/// THREAD_CREATE: another thread in the current process, starting at
/// `entry` with its stack pointer at `stack` (less what the calling
/// convention puts there for a function just called, as for `_start`),
/// `argument` as its first argument, and thread pointer `pointer`.
pub fn create_thread(entry: u64, stack: u64, argument: u64, pointer: u64) -> Result<u64, Error> {
    if entry >= abi::USER_END || stack >= abi::USER_END || pointer >= abi::USER_END {
        return Err(Error::BadAddress);
    }
    if !stack.is_multiple_of(16) || stack < syscall::ENTRY_STACK_GAP {
        return Err(Error::InvalidArgument);
    }
    let process = current().ok_or(Error::InvalidArgument)?;
    let mut list = process.threads.lock();
    if process.kill_requested() {
        return Err(Error::Interrupted); // ending: no new threads
    }
    if list.len() >= abi::THREADS_MAX {
        return Err(Error::OutOfMemory);
    }
    let stack = stack - syscall::ENTRY_STACK_GAP;
    let thread = threads::try_spawn_in(Some(Arc::clone(&process)), "user", pointer, move || {
        // SAFETY: the address space is the process's own (the switch
        // loaded it). Whether `entry` and `stack` are mapped is the
        // program's business: if not, the new thread faults in user
        // mode, and the process is killed for it, as for any fault.
        unsafe { syscall::enter_user(entry, stack, argument, 0) }
    })
    .ok_or(Error::OutOfMemory)?;
    list.push(thread);
    Ok(thread)
}

/// THREAD_EXIT: ends the current thread - after setting the u32 at
/// `clear` to 0 and waking whoever waits there, if `clear` is not 0 -
/// and the process with it if it was the last.
pub fn exit_thread(clear: u64) -> ! {
    let Some(process) = current() else { threads::exit() };
    if clear != 0 && write_user_word(clear, 0).is_ok() {
        crate::futex::wake(process.pid, clear, u64::MAX);
    }
    let me = threads::current();
    let last = {
        let mut list = process.threads.lock();
        list.retain(|&thread| thread != me);
        list.is_empty()
    };
    drop(process);
    if last {
        end_current(0, format_args!("exited with code 0, as its last thread ended"));
    }
    threads::exit()
}

/// The process the running thread belongs to.
pub fn current() -> Option<Arc<Process>> {
    threads::current_process()
}

/// Ends the current process: records `code`, ends its other threads,
/// closes its handles, tells whoever waits for it, and ends this thread.
///
/// Only the first thread here does all that: two threads may exit, or
/// fault, at the same moment, and the process has one exit code - the
/// first. The others, and every thread the end reaches later (through
/// `stop_if_killed`), only end themselves.
fn end_current(code: i64, how: core::fmt::Arguments) -> ! {
    if let Some(process) = current() {
        let me = threads::current();
        process.threads.lock().retain(|&thread| thread != me);
        if process.ending.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            kprintln!("[process] {} {:?} {how}", process.pid, process.name);
            if process.pid == ESSENTIAL.load(Ordering::Acquire) {
                stop_the_system(&process.name, how);
            }
            stop_other_threads(&process);
            process.close_all_handles();
            // Whoever asked to be told (SIGCHLD to a parent): sent before
            // the exit code is published. A parent waiting for this
            // child (PROCESS_WAIT, object_wait) returns as soon as it
            // sees the code - woken, or re-checking on another CPU at
            // any moment - and must find its SIGCHLD already pending, so
            // that the handler runs before its wait returns. Publishing
            // the code first, as this once did, left a window between
            // the two: under KVM a parent's waitid returned in it, its
            // handler not yet run (journal entry 101). The list is taken
            // out of the lock (sending can take the watcher's locks) and
            // left None, so a watcher who comes now is told at once.
            let notify = process.notify.lock().take().unwrap_or_default();
            for (watcher, signal) in notify {
                if let Some(watcher) = watcher.upgrade() {
                    send_signal(&watcher, signal);
                }
            }
            // Only now is the end seen: the code, then the wake-up (a
            // waker changes what the condition reads first).
            process.exit_code.store(code, Ordering::Release);
            EXITS.wake_all();
        }
    }
    threads::exit()
}

/// The process has ended: its other threads end too, each the next time
/// it heads for user mode - a waiting one woken to get there (see
/// `kill`).
fn stop_other_threads(process: &Process) {
    process.kill_requested.store(true, Ordering::Release);
    for &thread in process.threads.lock().iter() {
        threads::wake(thread); // if it is blocked or sleeping; else nothing
    }
}

/// The process the system cannot go on without: init. 0 until set.
static ESSENTIAL: AtomicU64 = AtomicU64::new(0);

/// Marks `process` as the one whose end is the system's end.
pub fn set_essential(process: &Process) {
    ESSENTIAL.store(process.pid, Ordering::Release);
}

/// init has ended, and the system ends with it - here, in one place,
/// with the reason.
///
/// Left alone, it would end anyway, and worse: init holds one end of
/// every channel between the services, so as its handles close each
/// service sees its peers vanish and exits in turn, every one of them
/// correctly - a system that switches itself off in good order and
/// leaves a frozen screen and a log of ordinary exits. Nothing can be
/// restarted, because the thing that restarts is what ended. So the
/// kernel stops while the reason is still the last thing said.
fn stop_the_system(name: &str, how: core::fmt::Arguments) -> ! {
    crate::arch::stop_other_cpus();
    // SAFETY: halting right after; whoever held the console never resumes.
    unsafe { crate::console::break_lock() };
    kprintln!();
    kprintln!("*** THE SYSTEM HAS STOPPED: {name} {how}");
    kprintln!("    {name} starts every service and restarts the ones that fail.");
    kprintln!("    Without it nothing can be started again, so the kernel has");
    kprintln!("    stopped here rather than let the services fall one by one.");
    kprintln!("    Restart the machine.");
    crate::arch::halt_forever()
}

/// Ends the current process with `code`.
pub fn exit_current(code: i64) -> ! {
    end_current(code, format_args!("exited with code {code}"))
}

/// Ends the current process for breaking the rules.
pub fn kill_current(reason: &str) -> ! {
    end_current(abi::KILLED, format_args!("killed: {reason}"))
}

/// Asks for `target` to be ended. It returns at once; the process ends
/// itself, soon.
///
/// A process cannot simply be taken apart from outside. Its thread may
/// be in the middle of a system call - holding a lock, halfway through
/// moving a handle - and a thread stopped there would leave the kernel's
/// own data half-changed. So a kill is a request: a flag, which the
/// thread checks at the two places where it is about to return to user
/// mode and so holds nothing of the kernel's - the end of a system
/// call, and the end of an interrupt that arrived in user mode
/// (`stop_if_killed`). There it ends as if it had called exit.
///
/// A thread that is waiting - for a message, an interrupt, a time -
/// would not get there, so the kill also wakes it: the wait returns
/// early (threads::wait_any), the call gives up, and the exit path
/// finds the flag.
///
/// How long it takes: a waiting thread ends the next time it runs; one
/// computing in user mode, at the next timer tick - 10 ms at most.
pub fn kill(target: &Arc<Process>) {
    kill_with(target, abi::STOPPED);
}

/// `kill`, ending the process with `code` - unless a kill is already
/// on its way, whose code stays.
fn kill_with(target: &Arc<Process>, code: i64) {
    if target.exit_code().is_some() {
        return; // already ended
    }
    if !target.kill_requested() {
        target.kill_code.store(code, Ordering::Release);
    }
    // The flag first (in `stop_other_threads`): a thread woken before
    // it was set would only go back to waiting.
    stop_other_threads(target);
}

/// Ends the current process if a kill was asked for. Called only where
/// the thread is about to return to user mode.
pub fn stop_if_killed() {
    // The code only - not the process: end_current never returns, so a
    // reference held across it would never be dropped, and the process
    // never freed.
    let code = current().filter(|process| process.kill_requested()).map(|process| process.kill_code.load(Ordering::Acquire));
    if let Some(code) = code {
        match code {
            abi::STOPPED => end_current(code, format_args!("stopped: killed by request")),
            _ => {
                let signal = abi::signal::of_exit_code(code).unwrap_or(0);
                end_current(code, format_args!("ended by signal {signal} ({})", abi::signal::name(signal)))
            }
        }
    }
}

// ---- signals ---------------------------------------------------------
//
// What the kernel does with a signal depends on three things the
// process told it (signal_setup): caught ones wait in `signals_pending`
// for a thread to take on its way to user mode (signals.rs delivers
// it); ignored ones are dropped as they come; any other ends the
// process, as a kill does, with the signal's exit code. A
// program that never asked - every Rust program on the taaj crate -
// has nothing caught and nothing ignored: Ctrl+C ends it.

/// signal_setup: where caught signals go, and which are caught and
/// which ignored. SIGKILL is neither, whatever the masks say.
pub fn signal_setup(entry: u64, caught: u64, ignored: u64, on_stack: u64) -> Result<u64, Error> {
    if entry >= abi::USER_END {
        return Err(Error::InvalidArgument);
    }
    let process = current().ok_or(Error::InvalidArgument)?;
    let never = abi::signal::bit(abi::signal::SIGKILL) | 1; // and bit 0, no signal
    process.signal_entry.store(entry, Ordering::Release);
    process.signals_ignored.store(ignored & !never, Ordering::Release);
    process.signals_caught.store(if entry == 0 { 0 } else { caught & !never }, Ordering::Release);
    process.signals_on_stack.store(on_stack & !never, Ordering::Release);
    Ok(0)
}

/// Whether the current process wants `signal` delivered on the
/// thread's signal stack.
pub fn signal_on_stack(signal: u64) -> bool {
    current().is_some_and(|process| process.signals_on_stack.load(Ordering::Acquire) & abi::signal::bit(signal) != 0)
}

/// PROCESS_NOTIFY's work: `signal` to `watcher` when `target` ends - at
/// once, if it has.
pub fn notify_on_end(target: &Arc<Process>, watcher: &Arc<Process>, signal: u64) {
    {
        // Under the lock: end_current takes the list (leaving None) under
        // it too, so either it finds this entry, or this finds the list
        // gone and tells the watcher itself.
        if let Some(list) = target.notify.lock().as_mut() {
            list.push((Arc::downgrade(watcher), signal));
            return;
        }
    }
    send_signal(watcher, signal);
}

/// PROGRAM_HEADERS: as many of the program headers as fit in `length`
/// bytes at `buffer`; how many bytes they all take.
pub fn program_headers(buffer: u64, length: u64) -> Result<u64, Error> {
    let process = current().ok_or(Error::InvalidArgument)?;
    let n = (process.headers.len() as u64).min(length) as usize;
    let n = n - n % 56;
    if n > 0 {
        copy_to_user(buffer, &process.headers[..n])?;
    }
    Ok(process.headers.len() as u64)
}

/// Sends `signal` to `target` (see above for what that does).
pub fn send_signal(target: &Arc<Process>, signal: u64) {
    use abi::signal::{bit, exit_code, IGNORED_BY_DEFAULT, SIGKILL};
    if target.exit_code().is_some() {
        return;
    }
    if signal == SIGKILL {
        return kill_with(target, exit_code(SIGKILL));
    }
    let bit = bit(signal);
    if target.signals_caught.load(Ordering::Acquire) & bit != 0 {
        // Pending first, then the wake-up: a thread woken looks for it
        // (threads.rs, rule 2), and one that was not waiting finds it
        // on its way back to user mode.
        target.signals_pending.fetch_or(bit, Ordering::AcqRel);
        for &thread in target.threads.lock().iter() {
            threads::wake(thread);
        }
    } else if (target.signals_ignored.load(Ordering::Acquire) | IGNORED_BY_DEFAULT) & bit == 0 {
        kill_with(target, exit_code(signal));
    }
}

/// Whether the current process has a caught signal waiting: a wait it
/// is in ends early (threads.rs), so that the signal is taken.
pub fn signal_waiting() -> bool {
    current().is_some_and(|process| process.signals_pending.load(Ordering::Acquire) != 0)
}

/// Takes the lowest-numbered signal waiting for the current process, if
/// one is: (the signal, where to deliver it). Another thread may take
/// it first; each signal is taken once.
pub fn take_signal() -> Option<(u64, u64)> {
    let process = current()?;
    let mut pending = process.signals_pending.load(Ordering::Acquire);
    while pending != 0 {
        let signal = u64::from(pending.trailing_zeros());
        let bit = abi::signal::bit(signal);
        match process.signals_pending.compare_exchange(pending, pending & !bit, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Some((signal, process.signal_entry.load(Ordering::Acquire))),
            Err(now) => pending = now,
        }
    }
    None
}

/// Where a fault's signal (SIGSEGV, SIGILL, ...) goes, if the current
/// process catches it. None: the fault ends the process, as always.
pub fn fault_signal(signal: u64) -> Option<u64> {
    let process = current()?;
    let entry = process.signal_entry.load(Ordering::Acquire);
    (entry != 0 && process.signals_caught.load(Ordering::Acquire) & abi::signal::bit(signal) != 0).then_some(entry)
}

/// The physical address behind each page of `pointer..pointer+length`
/// in the current process, checked: every page mapped for user mode, and
/// writable if `write`.
///
/// The kernel never dereferences a user pointer directly. Every page of
/// the range is looked up in the process's own page tables first, and
/// must be mapped with the USER bit - so a pointer into the kernel half,
/// into unmapped memory, or wrapping around the address space is an
/// error, not a fault and not a leak. A reserved page not yet touched is
/// filled here, as the program's own access would have filled it; if it
/// cannot be (the limit), the call fails with OutOfMemory.
///
/// What is returned are physical addresses, used after the locks are
/// let go. Another thread of the process could unmap the memory
/// meanwhile, and the frame be reused while the call still copies to
/// it - a race the program would have to make on purpose, against its
/// own memory, but one that touches a frame no longer its own. Closing
/// it means holding the frames (or the lock) for the copy: still to do.
fn user_pages(pointer: u64, length: u64, write: bool) -> Result<Vec<(u64, usize)>, Error> {
    let end = pointer.checked_add(length).ok_or(Error::BadAddress)?;
    if end > abi::USER_END {
        return Err(Error::BadAddress);
    }
    let process = current().ok_or(Error::BadAddress)?;
    let mappings = process.mappings.lock();
    let mut tables = process.tables.lock();
    let tables = tables.as_mut().ok_or(Error::BadAddress)?;

    let mut needed = Flags::USER;
    if write {
        needed |= Flags::WRITABLE;
    }
    let mut pieces = Vec::new();
    let mut address = pointer;
    while address < end {
        let translation = match tables.translate(VirtAddr::new(address)) {
            Some(translation) => translation,
            None => match fill(&process, &mappings, tables, address, write) {
                Fault::Filled => tables.translate(VirtAddr::new(address)).ok_or(Error::BadAddress)?,
                Fault::NotOurs => return Err(Error::BadAddress),
                // The call fails; the program is not ended here, in the
                // middle of a system call, whose callers may hold locks
                // and references the end would never let go.
                Fault::OverLimit(used, limit) => {
                    kprintln!(
                        "[process] {} {:?}: a system call found no memory - {} MiB used, its limit is {} MiB",
                        process.pid,
                        process.name,
                        (used * PAGE_SIZE) >> 20,
                        (limit * PAGE_SIZE) >> 20
                    );
                    return Err(Error::OutOfMemory);
                }
                Fault::NoMemory => return Err(Error::OutOfMemory),
            },
        };
        if !translation.flags.contains(needed) {
            return Err(Error::BadAddress);
        }
        let chunk = (PAGE_SIZE - address % PAGE_SIZE).min(end - address);
        pieces.push((translation.physical.as_u64(), chunk as usize));
        address += chunk;
    }
    Ok(pieces)
}

/// Copies `length` bytes from the current process's memory at `pointer`.
pub fn copy_from_user(pointer: u64, length: u64) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::with_capacity(length as usize);
    for (physical, length) in user_pages(pointer, length, false)? {
        let source = phys_to_virt(PhysAddr::new(physical)).as_ptr::<u8>();
        // SAFETY: the page is mapped (checked), and read through the
        // direct map, so it stays valid whatever the process does.
        bytes.extend_from_slice(unsafe { core::slice::from_raw_parts(source, length) });
    }
    Ok(bytes)
}

/// Whether the current process may have `length` bytes at `pointer`
/// written - checked before a call commits to anything, so a bad buffer
/// fails the call rather than losing, say, a received message.
pub fn check_writable(pointer: u64, length: u64) -> Result<(), Error> {
    user_pages(pointer, length, true).map(|_| ())
}

/// Copies `bytes` into the current process's memory at `pointer`: all of
/// them, or (if any page is not writable user memory) none.
pub fn copy_to_user(pointer: u64, bytes: &[u8]) -> Result<(), Error> {
    let mut offset = 0;
    for (physical, length) in user_pages(pointer, bytes.len() as u64, true)? {
        let target = phys_to_virt(PhysAddr::new(physical)).as_mut_ptr::<u8>();
        // SAFETY: as above, and the page is user memory - never the
        // kernel's own - so writing it cannot corrupt the kernel.
        unsafe { target.copy_from_nonoverlapping(bytes[offset..].as_ptr(), length) };
        offset += length;
    }
    Ok(())
}

/// A u32 of the current process's memory, as one atomic access: a futex
/// word, which the program's other threads change with atomic
/// instructions at the same moment. `pointer` must be 4-byte aligned,
/// so the word is all in one page; `write` asks for a writable page.
fn user_word(pointer: u64, write: bool) -> Result<&'static core::sync::atomic::AtomicU32, Error> {
    if !pointer.is_multiple_of(4) {
        return Err(Error::InvalidArgument);
    }
    let pages = user_pages(pointer, 4, write)?;
    let physical = pages.first().ok_or(Error::BadAddress)?.0;
    // SAFETY: an aligned u32 of user memory, mapped for this process -
    // never the kernel's own - reached through the direct map, which
    // maps every frame for as long as the kernel runs. The program may
    // unmap it later; then it is a frame of memory still, and an atomic
    // access to it harms nothing.
    Ok(unsafe { &*phys_to_virt(PhysAddr::new(physical)).as_ptr::<core::sync::atomic::AtomicU32>() })
}

/// Reads the u32 at `pointer` atomically (see `user_word`).
pub fn read_user_word(pointer: u64) -> Result<u32, Error> {
    Ok(user_word(pointer, false)?.load(Ordering::SeqCst))
}

/// Writes the u32 at `pointer` atomically (see `user_word`).
pub fn write_user_word(pointer: u64, value: u32) -> Result<(), Error> {
    user_word(pointer, true)?.store(value, Ordering::SeqCst);
    Ok(())
}
