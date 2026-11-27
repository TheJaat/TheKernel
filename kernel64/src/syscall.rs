//! System calls: what a user program may ask of the kernel.
//!
//! The numbers and errors are in the `abi` crate, shared with user
//! programs. Every argument is untrusted: numbers are range-checked and
//! pointers go through `process::copy_from_user` and `copy_to_user`.

use alloc::string::String;

use abi::{call, Error};

use crate::{arch, interrupts, ipc, process, threads};

/// Longest sleep accepted, in ticks: ten minutes.
const MAX_SLEEP: u64 = 10 * 60 * interrupts::HZ;

/// Runs system call `number`; returns the value for RAX.
pub fn dispatch(number: u64, args: [u64; 6]) -> u64 {
    // The entry stub runs with interrupts off. A system call may take a
    // while (or sleep), so let the timer in - the thread is on its own
    // kernel stack and can be preempted like any other.
    arch::enable_interrupts();
    let result = match number {
        call::EXIT => process::exit_current(args[0] as i64),
        call::DEBUG_WRITE => debug_write(args[0], args[1]),
        call::YIELD => {
            threads::yield_now();
            Ok(0)
        }
        call::SLEEP if args[0] <= MAX_SLEEP => {
            if threads::sleep(args[0]) {
                Ok(0)
            } else {
                Err(Error::Interrupted)
            }
        }
        call::SLEEP => Err(Error::InvalidArgument),
        call::PROCESS_ID => Ok(process::current().map_or(0, |p| p.pid)),
        call::TICKS => Ok(interrupts::ticks()),
        call::CHANNEL_CREATE => ipc::create(args[0]),
        call::CHANNEL_SEND => ipc::send(args[0], args[1], args[2], args[3], args[4]),
        call::CHANNEL_RECEIVE => ipc::receive(args[0], args[1], args[2], args[3], args[4]),
        call::HANDLE_CLOSE => ipc::close(args[0]),
        call::HANDLE_DUPLICATE => ipc::duplicate(args[0], args[1]),
        call::INTERRUPT_WAIT => ipc::interrupt_wait(args[0]),
        call::IOPORTS_ENABLE => ipc::ioports_enable(args[0]),
        call::PROCESS_SPAWN => ipc::spawn(args[0], args[1], args[2], args[3], args[4], args[5]),
        call::PROCESS_WAIT => ipc::wait(args[0], args[1], args[2]),
        call::MEMORY_CREATE => ipc::memory_create(args[0]),
        call::MEMORY_MAP => ipc::memory_map(args[0], args[1]),
        call::MEMORY_UNMAP => process::unmap_memory(args[0]),
        call::OBJECT_WAIT => ipc::object_wait(args[0], args[1]),
        call::SYSTEM_INFO => ipc::system_info(args[0], args[1]),
        call::PROCESS_LIST => ipc::process_list(args[0], args[1], args[2]),
        call::PROCESS_SPAWN_IMAGE => ipc::spawn_image(args[0], args[1], args[2]),
        call::PROCESS_KILL => ipc::kill(args[0]),
        call::MEMORY_SIZE => ipc::memory_size(args[0]),
        call::OBJECT_WAIT_UNTIL => ipc::object_wait_until(args[0], args[1], Some(args[2])),
        call::POWER => crate::power::system_power(args[0], args[1]),
        call::THREAD_CREATE => process::create_thread(args[0], args[1], args[2], args[3]),
        call::THREAD_EXIT => process::exit_thread(args[0]),
        call::THREAD_POINTER if args[0] < abi::USER_END => {
            threads::set_pointer(args[0]);
            Ok(0)
        }
        call::THREAD_POINTER => Err(Error::BadAddress),
        call::FUTEX_WAIT => crate::futex::wait(args[0], args[1], args[2]),
        call::FUTEX_WAKE => crate::futex::wake_here(args[0], args[1]),
        call::MEMORY_RESERVE => process::reserve_memory(args[0], args[1]),
        call::MEMORY_LIMIT => process::memory_limit(args[0]),
        call::MEMORY_USED => process::memory_used(),
        call::CLOCK => crate::clock::read(args[0]),
        call::SIGNAL_SETUP => process::signal_setup(args[0], args[1], args[2], args[3]),
        call::SIGNAL_SEND => ipc::signal(args[0], args[1]),
        call::ALARM => crate::alarms::set(args[0]),
        call::RANDOM => crate::random::fill(args[0], args[1]),
        call::PROGRAM_HEADERS => process::program_headers(args[0], args[1]),
        call::SIGNAL_STACK => threads::set_signal_stack(args[0], args[1]),
        call::PROCESS_NOTIFY => ipc::notify(args[0], args[1]),
        call::MEMORY_PROTECT => process::protect_memory(args[0], args[1], args[2]),
        call::MEMORY_DISCARD => process::discard_memory(args[0], args[1]),
        call::SCHED_SET => ipc::sched_set(args[0], args[1], args[2], args[3]),
        call::SCHED_GET => ipc::sched_get(args[0], args[1], args[2]),
        // Taken before dispatch, by the architecture's own code: it
        // changes every register. Here only if that code did not.
        call::SIGNAL_RETURN => Err(Error::UnknownCall),
        _ => Err(Error::UnknownCall),
    };
    // A call that woke a thread more important than this one, here on
    // this CPU (a futex, a channel), has asked this CPU to choose again:
    // it does so now, not at the next timer tick (journal entry 123).
    threads::preempt_if_asked();
    // The way back to user mode: where a process someone asked to end,
    // ends. Every call comes through here - one that was waiting when
    // the kill came has just returned Interrupted - with nothing of the
    // kernel's left on the stack to lose.
    process::stop_if_killed();
    arch::disable_interrupts();
    abi::encode(result)
}

fn debug_write(pointer: u64, length: u64) -> Result<u64, Error> {
    if length > abi::DEBUG_WRITE_MAX {
        return Err(Error::InvalidArgument);
    }
    let bytes = process::copy_from_user(pointer, length)?;
    // Not trusted to be UTF-8 either: invalid bytes become U+FFFD. Into
    // the log's queue, not onto the serial line: the program does not
    // wait for it (console.rs).
    // The log is told who wrote it, to keep each writer's lines whole.
    let pid = process::current().map_or(0, |process| process.pid);
    crate::console::write_later(pid, String::from_utf8_lossy(&bytes).as_bytes());
    Ok(length)
}
