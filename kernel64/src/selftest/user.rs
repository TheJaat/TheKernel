//! The checks of programs: user mode, IPC, files, kill. The same on
//! every CPU - a program is whatever the `user/` crate was compiled
//! to, and what is asked of the kernel here is asked through the same
//! system calls.

use alloc::format;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::{interrupts, mm, threads};

/// User mode: programs, run at once. Some follow the rules and must
/// finish with their exit codes; the others break them and must be
/// killed - without the kernel noticing anything but a log line. Then
/// everything they used must be back.
///
/// Which programs is the CPU's business (`programs::USER_CHECK`): the
/// rules a program can break are not the same on each.
pub fn user() {
    use crate::{process, programs};

    let checks = programs::USER_CHECK;
    let run_all = || {
        let started: Vec<_> = checks
            .iter()
            .map(|&(name, file, _)| (name, process::spawn(name, file, Vec::new()).unwrap_or_else(|e| panic!("loading {name}: {e:?}"))))
            .collect();
        started
            .into_iter()
            .map(|(name, (process, thread))| {
                threads::join(thread);
                (name, process.exit_code())
            })
            .collect::<Vec<_>>()
    };

    let expected: Vec<_> = checks.iter().map(|&(name, _, code)| (name, Some(code))).collect();
    assert_eq!(run_all(), expected, "user programs did not end as expected");

    // A second round must leave memory exactly as the first did: every
    // page of every process - code, data, stack, page tables - returned.
    threads::yield_now(); // let the last exited thread be reaped
    let frames = mm::unused_frames();
    assert_eq!(run_all(), expected);
    threads::yield_now();
    assert_eq!(mm::unused_frames(), frames, "processes leaked frames");
    // Files that are not programs - which the disk may now hold - are
    // refused, and cost nothing: text, and a program cut in half.
    assert!(process::load("text", &[b'x'; 200][..]).is_err());
    assert!(process::load("half", &programs::HELLO[..programs::HELLO.len() / 2]).is_err());
    assert_eq!(mm::unused_frames(), frames, "refused programs leaked frames");

    // "hello exited 42, spin and heapcheck exited 0, peek, cli and
    // memro were killed".
    let list = |wanted: &dyn Fn(i64) -> bool| {
        let names: Vec<&str> = checks.iter().filter(|c| wanted(c.2)).map(|c| c.0).collect();
        match names.split_last() {
            Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
            _ => names.concat(),
        }
    };
    kprintln!(
        "[selftest] user: hello exited 42, {} exited 0, {} were killed; {} processes, nothing leaked",
        list(&|code| code == 0),
        list(&|code| code == abi::KILLED),
        checks.len() * 2
    );
}

/// IPC: a service and its client, connected by one channel the kernel
/// made. The client tests every rule of handles and channels, then asks
/// the service to crash and must survive it. Twice - and then every
/// channel and every frame must be gone.
pub fn ipc() {
    use crate::{ipc, process, programs};
    use kernel64::cap::Rights;

    let run = || {
        let (service_end, client_end) = ipc::channel();
        let (service, service_thread) =
            process::spawn("echod", programs::ECHOD, alloc::vec![(service_end, Rights::ALL)]).expect("loading echod");
        let (client, client_thread) =
            process::spawn("echoc", programs::ECHOC, alloc::vec![(client_end, Rights::ALL)]).expect("loading echoc");
        threads::join(client_thread);
        threads::join(service_thread);
        assert_eq!(client.exit_code(), Some(0), "echoc failed (the code is the line of the failed check)");
        assert_eq!(service.exit_code(), Some(abi::KILLED), "echod should have crashed");
    };

    run();
    threads::yield_now();
    let frames = mm::unused_frames();
    run();
    threads::yield_now();
    assert_eq!(mm::unused_frames(), frames, "IPC leaked frames");
    assert_eq!(ipc::live_channels(), 0, "channels leaked");
    kprintln!("[selftest] ipc: echo service and client - rights, transfer, backpressure, crash isolation; 0 channels left");
}

/// Files: the file service and a client that tests it - long files,
/// listings, refusals, careless requests. When the client is done it
/// closes its end, and the service must end cleanly. Twice, and then
/// nothing may be left: no channel (each request made one for its
/// reply), no frame (the service's heap held every file).
pub fn files() {
    use crate::{ipc, process, programs};
    use kernel64::cap::Rights;

    let run = || {
        let (service_end, client_end) = ipc::channel();
        // The second channel: requests on it may only read.
        let (service_look, client_look) = ipc::channel();
        let (service, service_thread) =
            process::spawn("fs", programs::FS, alloc::vec![(service_end, Rights::ALL), (service_look, Rights::ALL)])
                .expect("loading fs");
        let (client, client_thread) =
            process::spawn("fscheck", programs::FSCHECK, alloc::vec![(client_end, Rights::ALL), (client_look, Rights::ALL)])
                .expect("loading fscheck");
        threads::join(client_thread);
        threads::join(service_thread);
        assert_eq!(client.exit_code(), Some(0), "fscheck failed (the code is the line of the failed check)");
        assert_eq!(service.exit_code(), Some(0), "fs should end when its last client goes");
    };

    run();
    threads::yield_now();
    let frames = mm::unused_frames();
    run();
    threads::yield_now();
    assert_eq!(mm::unused_frames(), frames, "the file service leaked frames");
    assert_eq!(ipc::live_channels(), 0, "channels leaked");
    kprintln!("[selftest] files: fs and fscheck, twice - every check passed, 0 channels and no frames left");
}

/// Kill: a program that never ends, ended from outside - in each state a
/// thread can be in when the kill arrives. Computing in user mode (only
/// the timer interrupt gets it back into the kernel), making system
/// calls in a loop, asleep, waiting for a message that will never come,
/// and not yet run at all. Each must end with STOPPED, promptly; and
/// then nothing may be left: no frame, no channel (the waiting one made
/// a channel of its own, which must close with it).
pub fn kill() {
    use crate::{ipc, process, programs};
    use kernel64::cap::Rights;

    // Starts `forever MODE`: its arguments arrive as a program's do, in
    // a channel with one message.
    let start = |mode: &str| {
        let (tx, rx) = ipc::channel();
        ipc::kernel_send(&tx, mode.as_bytes(), Vec::new()).expect("sending the arguments");
        drop(tx);
        let loaded = process::load("forever", programs::FOREVER).expect("loading forever");
        let thread = process::start(&loaded, alloc::vec![(rx, Rights::ALL)], abi::startup::ARGUMENTS);
        (Arc::clone(&loaded), thread)
    };
    let run = || {
        for mode in ["spin", "yield", "sleep", "wait", "unborn"] {
            // "Promptly" is up to 3 ticks, measured up to three times:
            // ticks follow the CPU's counter, so a host that stops this
            // virtual machine for a moment makes one kill look slow
            // (under KVM with every host CPU busy, 4 boots in 40 did);
            // a kill that really is slow is slow every time (journal
            // entry 105).
            // Every try's ticks, for the message: one slow try among
            // fast ones is the host; slow every time, by about as much,
            // is this kernel (K?-slow-kill, TheTaaJ entry NNN).
            let mut tries = [0u64; 3];
            let mut took = 0;
            for n in 0..3 {
                let (process, thread) = start(mode);
                if mode != "unborn" {
                    threads::sleep(3); // long enough to get where it is going
                }
                assert_eq!(process.exit_code(), None, "forever {mode} ended by itself");
                let asked = interrupts::ticks();
                process::kill(&process);
                process::kill(&process); // asking twice is asking once
                threads::join(thread);
                took = interrupts::ticks() - asked;
                tries[n] = took;
                assert_eq!(process.exit_code(), Some(abi::STOPPED), "forever {mode} after the kill");
                process::kill(&process); // already ended: nothing happens
                assert_eq!(process.exit_code(), Some(abi::STOPPED));
                if took <= 3 {
                    break;
                }
            }
            assert!(took <= 3, "forever {mode} took {took} ticks to end, three times over (the tries: {tries:?} ticks)");
        }
    };

    run();
    threads::yield_now(); // let the last exited thread be reaped
    let frames = mm::unused_frames();
    run();
    threads::yield_now();
    assert_eq!(mm::unused_frames(), frames, "killed processes leaked frames");
    assert_eq!(ipc::live_channels(), 0, "killed processes leaked channels");
    kprintln!("[selftest] kill: spinning, yielding, sleeping, waiting and not yet started - each stopped within 3 ticks; nothing leaked");
}

/// A disk: the block driver, given a device the kernel found and made
/// ready, and a client that tests it - every kind of request, the
/// refusals, and writes read back. The driver is a program: all it has
/// of the device is three handles.
///
/// Twice, with the same handles: the second driver finds the device as
/// the first left it, and must reset it and start over - which is what
/// restarting a crashed driver will be. Then nothing may be left; and
/// the device must have interrupted, or the driver was not waiting for
/// it but asking over and over.
#[cfg(target_arch = "aarch64")]
pub fn disk(device: Option<&crate::mmio_devices::Ready>) {
    use crate::{arch, ipc, process, programs};
    use kernel64::cap::Rights;

    let Some(device) = device else {
        kprintln!("[selftest] disk: no block device a driver exists for - not run");
        return;
    };
    let run = || {
        let (to_driver, description) = ipc::channel();
        ipc::kernel_send(&to_driver, &device.info.to_bytes(), Vec::new()).expect("sending the description");
        let (writers, write_requests) = ipc::channel();
        let (readers, read_requests) = ipc::channel();
        // The driver's handles, in the order it expects: registers,
        // DMA memory, the description, its two request channels, and
        // last the interrupt.
        let mut handles = device.handles.clone();
        let interrupt = (handles.len() > 2).then(|| handles.remove(2));
        handles.extend([(description, Rights::ALL), (write_requests, Rights::ALL), (read_requests, Rights::ALL)]);
        handles.extend(interrupt);
        let (driver, driver_thread) = process::spawn("blk", programs::BLK, handles).expect("loading blk");
        let (client, client_thread) =
            process::spawn("blkcheck", programs::BLKCHECK, alloc::vec![(writers, Rights::ALL), (readers, Rights::ALL)])
                .expect("loading blkcheck");
        threads::join(client_thread);
        threads::join(driver_thread);
        drop(to_driver);
        assert_eq!(driver.exit_code(), Some(0), "blk should end when its last client goes");
        match client.exit_code() {
            Some(0) => true,
            // Not the disk the test is for - one with someone's files
            // on it. The client read its first sector and stopped.
            Some(abi::block::NOT_THE_TEST_DISK) => false,
            code => panic!("blkcheck failed: {code:?} (the code is the line of the failed check)"),
        }
    };

    let before = arch::device_interrupts();
    if !run() {
        kprintln!("[selftest] disk: blk started on it and read it; not the test's own disk, so nothing more was asked of it");
        return;
    }
    threads::yield_now();
    let frames = mm::unused_frames();
    let first = arch::device_interrupts() - before;
    assert!(run(), "the test's disk stopped being the test's disk");
    threads::yield_now();
    let second = arch::device_interrupts() - before - first;
    assert_eq!(mm::unused_frames(), frames, "the disk driver leaked frames");
    assert_eq!(ipc::live_channels(), 0, "channels leaked");
    if device.info.interrupt {
        // Each run makes well over a hundred requests of the device. Not
        // each is an interrupt of its own - one that finishes while the
        // line is off shares the next - but most are.
        assert!(first >= 20 && second >= 20, "the disk interrupted {first} and {second} times: the driver is not waiting for it");
    }
    kprintln!(
        "[selftest] disk: blk and blkcheck, twice on one device - every check passed; {} interrupts passed to the driver; nothing leaked",
        first + second
    );
}
