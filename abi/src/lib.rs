//! The TheTaaJ system call interface: the contract between the kernel
//! and every user program.
//!
//! One crate, used by both sides, so the numbers cannot drift apart. It
//! holds only constants and plain types - no code that runs.
//!
//! The calling convention (x86_64), close to Linux's so it is familiar:
//!
//!   `syscall` instruction
//!   RAX        system call number
//!   RDI, RSI, RDX, R10, R8, R9   arguments 1-6 (R10, not RCX: the
//!              instruction itself overwrites RCX with the return address)
//!   RAX        result: >= 0 success, < 0 an `Error`, negated
//!   RCX, R11   destroyed (the CPU uses them); everything else preserved
//!
//! This is the native interface, deliberately small. The POSIX layer will
//! sit on top of it, in user space.
//!
//! Capabilities: a process reaches kernel objects only through handles -
//! small numbers naming entries in its own handle table, each with a set
//! of `rights`. A handle is meaningful only to the process holding it;
//! the same number in another process names something else, or nothing.
//! Handles are passed between processes by sending them in messages.
//!
//! A program starts with the handles its parent gave it - its startup
//! handles - and nothing else. They are numbered 1, 2, ... n, in the
//! order the parent listed them, with n in RDI. By convention handle 1
//! is the bootstrap channel, when there is one.

#![no_std]

/// System call numbers.
/// How many ticks - what `call::TICKS` counts, and `call::SLEEP`
/// waits for - make a second. A program needs it to turn ticks into
/// time (the C library's `time` and `gettimeofday`).
pub const TICKS_PER_SECOND: u64 = 100;

pub mod call {
    /// exit(code) - ends the process. Never returns.
    pub const EXIT: u64 = 0;
    /// debug_write(pointer, length) - writes text to the kernel console.
    /// A stand-in until there are real I/O channels. Returns the length.
    pub const DEBUG_WRITE: u64 = 1;
    /// yield() - lets other threads run.
    pub const YIELD: u64 = 2;
    /// sleep(ticks) - sleeps for at least that many timer ticks.
    pub const SLEEP: u64 = 3;
    /// process_id() - this process's id.
    pub const PROCESS_ID: u64 = 4;
    /// ticks() - timer ticks since boot.
    pub const TICKS: u64 = 5;
    /// channel_create(out: *mut [u64; 2]) - a new channel; writes the
    /// handles of its two ends, each with all rights.
    pub const CHANNEL_CREATE: u64 = 6;
    /// channel_send(handle, data, data_length, handles, handle_count) -
    /// queues a message on the other end. Never blocks: a full queue is
    /// `QueueFull`. The handles move with the message - they leave the
    /// sender's table, all of them or (on any error) none.
    pub const CHANNEL_SEND: u64 = 7;
    /// channel_receive(handle, buffer, buffer_length, handles,
    /// handle_capacity) - takes the next message, waiting for one if
    /// needed. Returns `data_length | handle_count << 32`. A message that
    /// does not fit is left queued (`BufferTooSmall`).
    pub const CHANNEL_RECEIVE: u64 = 8;
    /// handle_close(handle).
    pub const HANDLE_CLOSE: u64 = 9;
    /// handle_duplicate(handle, rights) - a second handle to the same
    /// object, with `rights`, which must be a subset of the original's.
    /// Needs DUPLICATE.
    pub const HANDLE_DUPLICATE: u64 = 10;
    /// interrupt_wait(handle) - waits until the interrupt has fired;
    /// returns how many times it did since the last wait. Needs RECEIVE.
    pub const INTERRUPT_WAIT: u64 = 11;
    /// ioports_enable(handle) - lets this process use the I/O ports the
    /// handle names directly, with `in` and `out`. Any other port is
    /// still a fault.
    pub const IOPORTS_ENABLE: u64 = 12;
    /// process_spawn(launcher, name, name_length, handles, count,
    /// argument) - starts the program `name`. `handles` points to `count` pairs of
    /// u64s, (handle, rights): each handle moves into the child as its
    /// startup handle, with exactly `rights` - a subset of what the
    /// parent held, so a parent can hand on less than it has. Moving
    /// needs TRANSFER on the parent's handle. `argument` reaches the
    /// child's `_start` in RSI (see `startup`). Returns a handle to the
    /// new process, which may be passed on (TRANSFER) - it only lets its
    /// holder wait for the process. All-or-nothing, like a send.
    pub const PROCESS_SPAWN: u64 = 13;
    /// process_wait(handles, count, code) - waits until any of `count`
    /// processes has ended; returns its index in the list and writes its
    /// exit code (an i64) to `code`.
    pub const PROCESS_WAIT: u64 = 14;
    /// memory_create(size) - a new memory object of `size` bytes (rounded
    /// up to whole pages), zeroed. Returns a handle with all rights.
    pub const MEMORY_CREATE: u64 = 15;
    /// memory_map(handle, writable) - maps the whole object into this
    /// process, at an address the kernel chooses; returns it. Needs READ,
    /// and WRITE too if `writable`. Never executable.
    pub const MEMORY_MAP: u64 = 16;
    /// memory_unmap(address) - removes the mapping that starts there.
    pub const MEMORY_UNMAP: u64 = 17;
    /// object_wait(handles, count) - waits until any of the handles is
    /// ready, and returns its index. Ready means: a channel has a message
    /// or its peer closed (a receive would not wait); a process has
    /// ended; an interrupt is pending. Nothing is consumed - the caller
    /// then receives, or reads the exit code, as usual.
    pub const OBJECT_WAIT: u64 = 18;
    /// system_info(system, out) - writes a `SystemInfo` (as u64 words).
    /// Needs a system handle: what is running is not everyone's business.
    pub const SYSTEM_INFO: u64 = 19;
    /// process_list(system, out, capacity) - writes up to `capacity`
    /// `ProcessEntry`s (as u64 words); returns how many processes exist.
    pub const PROCESS_LIST: u64 = 20;
    /// process_spawn_image(launcher, image, request) - starts a program
    /// from an ELF file held in the memory object `image` (which needs
    /// READ, and stays with the caller). `request` points at
    /// SPAWN_REQUEST_WORDS u64s: name pointer, name length, pointer to
    /// (handle, rights) pairs, their count, and the argument - the same
    /// things process_spawn takes, in memory because seven values do not
    /// fit in six registers. Returns a handle to the new process;
    /// NotExecutable if the file is not a valid program.
    pub const PROCESS_SPAWN_IMAGE: u64 = 21;
    /// process_kill(process) - asks for the process to be ended; the
    /// handle needs KILL. It returns at once: the process ends when it
    /// next would run in user mode - right away if it was waiting or
    /// sleeping, which the kill cuts short - with the exit code STOPPED.
    /// Wait for it as for any other. Killing one that has already ended
    /// does nothing.
    pub const PROCESS_KILL: u64 = 22;
    /// memory_size(handle) - the size of a memory object, in bytes. A
    /// service handed someone else's object must know how far it may
    /// go before it maps and touches it.
    pub const MEMORY_SIZE: u64 = 23;
    /// object_wait_until(handles, count, deadline) - as object_wait,
    /// but gives up with TimedOut once the tick count (TICKS) reaches
    /// `deadline`. A deadline that has passed - 0, say - asks without
    /// waiting: "is any of these ready now?"
    pub const OBJECT_WAIT_UNTIL: u64 = 24;
}

/// The u64s process_spawn_image's request holds.
pub const SPAWN_REQUEST_WORDS: usize = 5;
/// The largest program file process_spawn_image accepts: 128 MiB. The
/// file is read where it lies, in the memory object (copied first only
/// if someone else could change it), so the limit is a guard against a
/// nonsense size, not a measure of the kernel's own memory. LLVM's
/// tools are tens of megabytes.
pub const PROGRAM_IMAGE_MAX: u64 = 128 << 20;

/// What a program learns from its parent besides its handles: the
/// argument of process_spawn, in RSI. Each flag says one more startup
/// handle is present; they come in the order of the flags' bits.
pub mod startup {
    /// Standard output: a channel to send what the program prints on,
    /// as `stream` messages. Whoever started the program decides what
    /// is at the other end - the screen, a file being written, another
    /// program - and the program prints the same way whichever it is.
    pub const STDOUT: u64 = 1 << 0;
    /// A channel holding the program's command line, in two messages.
    /// First its arguments, each followed by a zero byte (see
    /// `arguments`) - perhaps none, an empty message. Then the program
    /// itself, the same way: two words, the name it was started by (C's
    /// argv[0]: "ls", "tools/given") and its file, as a path from the
    /// root ("/ls", "/tools/given"), or nothing for a program built into
    /// the kernel image - what Linux tells a program in /proc/self/exe,
    /// so that it can find what it brings with it (clang its headers).
    /// The second message may be missing: a program started by anything
    /// but init has only the first.
    pub const ARGUMENTS: u64 = 1 << 1;
    /// The program's channel to the registry: where it asks, by name,
    /// for the services it was given - "files", say. Which names exist
    /// on it, and what each means, whoever started the program decided
    /// (the `registry` crate). A program with no services gets none.
    pub const SERVICES: u64 = 1 << 2;
    /// A channel holding one message: the directory the program was
    /// started in, as a path from the root - what its relative paths
    /// start from. Absent means the root. It is a name and nothing
    /// more: it lets the program reach no file it could not reach
    /// without it.
    pub const DIRECTORY: u64 = 1 << 3;
    /// Standard input: a channel to receive `stream` messages from.
    /// When the other end is closed and everything sent has been read,
    /// the input has ended. Absent means there is no input at all.
    pub const STDIN: u64 = 1 << 4;
    /// Standard error: a channel to send on, as standard output is -
    /// for what the program has to say that is not its result: that a
    /// file is not there, how it should have been called. It is a
    /// second output so that it can go somewhere else: when the result
    /// is sent to a file or to another program, the complaint still
    /// reaches whoever is watching. For now it is always the screen.
    pub const STDERR: u64 = 1 << 5;
    /// Not a handle: standard input is the keyboard - a person, typing.
    /// The program gets a channel for its input as always (STDIN), but
    /// nothing is sent down it until the program asks: before it
    /// waits for input, it sends a `stream::WANT` back up the same
    /// channel, and whoever stands at the other end - the shell - then
    /// lets the next line typed go to it. So what is typed belongs to
    /// whoever is reading at the time, and a program that never reads
    /// takes nothing. Without this flag an input is a file or another
    /// program, which need no asking.
    pub const KEYBOARD: u64 = 1 << 6;
    /// Not a handle either: standard output is the screen. A program need not
    /// care, and should not; the runtime does two things differently
    /// for a screen. It copies what is printed to the kernel's debug
    /// log, so the serial line shows what the screen shows. And it
    /// gives up on a screen that takes nothing for a second, where on
    /// any other output it waits: text for a file must not be dropped.
    pub const SCREEN: u64 = 1 << 7;
    /// The flags that each stand for one startup handle: the ones
    /// below the two that do not.
    pub const HANDLES: u64 = KEYBOARD - 1;
}

/// A stream: bytes in order, in messages, on a channel. What standard
/// output and standard input carry.
///
///   [DATA] [bytes]
///
/// A message may hold any number of bytes up to DATA_MAX, and where one
/// message ends means nothing - a reader must not expect a line, or a
/// whole anything, per message. The end of the stream is the sender
/// closing its end. A message that does not start with DATA is not
/// part of the stream: the display, which takes text as a stream, has
/// other messages of its own.
///
/// One message goes the other way, from the reader to the sender, and
/// only when the sender is a keyboard (`startup::KEYBOARD`):
///
///   [WANT]
///
/// "I am about to wait for input." A file is sent whether or not it is
/// asked for; a person's typing is not, because it may be meant for
/// someone else - the shell, if the program is not reading.
pub mod stream {
    /// The same byte as display::TEXT: text for the screen is a stream.
    pub const DATA: u8 = super::display::TEXT;
    /// Most bytes in one message.
    pub const DATA_MAX: usize = super::MESSAGE_MAX as usize - 1;
    /// From the reader: it is waiting for input.
    pub const WANT: u8 = 0x57;
}

/// How a program's arguments travel: each one followed by a zero byte.
///
/// The shell has already decided where one argument ends and the next
/// begins (quotes, spaces); this keeps that decision on the way to the
/// program. A zero byte after each - not between them - so that no
/// arguments (nothing) and one empty argument (a single zero) differ.
/// It is what C calls a string, and how Unix lays out `argv`: the one
/// byte that cannot be typed is the one that separates.
pub mod arguments {
    /// Lays `words` out in `out`; the length, or None if they do not
    /// fit or one of them has a zero byte in it.
    pub fn encode<'a>(words: impl Iterator<Item = &'a str>, out: &mut [u8]) -> Option<usize> {
        let mut at = 0;
        for word in words {
            let end = at + word.len();
            if end >= out.len() || word.as_bytes().contains(&0) {
                return None;
            }
            out[at..end].copy_from_slice(word.as_bytes());
            out[end] = 0;
            at = end + 1;
        }
        Some(at)
    }

    /// The arguments in `bytes`, in order. Bytes after the last zero -
    /// an argument that was cut off - are not one.
    pub fn decode(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
        let mut rest = bytes;
        core::iter::from_fn(move || {
            let end = rest.iter().position(|&byte| byte == 0)?;
            let (word, tail) = rest.split_at(end);
            rest = &tail[1..];
            Some(word)
        })
    }
}

/// Requests to init, from the shell: "start this program".
///
///   [kind] [flags] [name length] [name]
///   [directory length] [directory] [file length] [file] [arguments]
///
/// `kind` is BUILT_IN (a program in the kernel image, by name) or IMAGE
/// (the first handle is a memory object holding the program's file).
/// `flags` are startup flags, STDOUT, SERVICES, STDIN and STDERR: for each one
/// set, one more handle follows, in that order - the caller's own
/// handles, which it hands down to the new program. SCREEN may be set
/// with STDOUT, and brings no handle. init adds nothing of its own, so a
/// program can get no more than whoever asked for it holds.
/// `directory` is where the program is started: the caller's current
/// directory, as a path from the root ("" is the root). init passes it
/// on as it does the arguments, and does not look at it.
/// `file` is the program's file, as a path from the root ("/ls"), for
/// an IMAGE; for a program built in it is "". init tells the program
/// its name and its file (startup::ARGUMENTS), and looks at neither.
///
/// The reply: "ok" and a handle to the new process, or "error: ...".
/// It goes back on the channel the request came on - unless `kind` has
/// REPLY in it: then the last handle is a channel of the asker's own,
/// and the reply goes there. The shell is the one asker on its channel;
/// programs that start programs share theirs, and each asks with a new
/// channel, so that one never reads another's reply.
pub mod init {
    pub const BUILT_IN: u8 = 1;
    pub const IMAGE: u8 = 2;
    /// Added to `kind`: reply on the last handle (see above).
    pub const REPLY: u8 = 0x80;
    /// Added to `kind`: `arguments` carries the new program's
    /// environment too - `NAME=value` words, each ending in a zero byte,
    /// as the arguments do - laid out by `pack`. init sends it to the
    /// program after its command line (startup::ARGUMENTS).
    pub const ENVIRONMENT: u8 = 0x40;

    /// `arguments` and `environment` as one Run's `arguments`, for a
    /// `kind` with ENVIRONMENT: the arguments' length in two bytes
    /// (little-endian), the arguments, the environment. Its length, or
    /// None if it does not fit in `out`.
    pub fn pack(arguments: &[u8], environment: &[u8], out: &mut [u8]) -> Option<usize> {
        let length = 2 + arguments.len() + environment.len();
        let said = u16::try_from(arguments.len()).ok()?;
        if length > out.len() {
            return None;
        }
        out[..2].copy_from_slice(&said.to_le_bytes());
        out[2..2 + arguments.len()].copy_from_slice(arguments);
        out[2 + arguments.len()..length].copy_from_slice(environment);
        Some(length)
    }

    pub struct Run<'a> {
        pub kind: u8,
        pub flags: u8,
        pub name: &'a str,
        pub directory: &'a str,
        pub file: &'a str,
        pub arguments: &'a [u8],
    }

    impl<'a> Run<'a> {
        /// The kind without REPLY and ENVIRONMENT: BUILT_IN or IMAGE.
        pub fn what(&self) -> u8 {
            self.kind & !(REPLY | ENVIRONMENT)
        }

        /// The arguments and the environment (none, without
        /// ENVIRONMENT in `kind`).
        pub fn parts(&self) -> (&'a [u8], &'a [u8]) {
            if self.kind & ENVIRONMENT == 0 {
                return (self.arguments, &[]);
            }
            let Some((said, rest)) = self.arguments.split_first_chunk::<2>() else { return (&[], &[]) };
            rest.split_at_checked(usize::from(u16::from_le_bytes(*said))).unwrap_or((&[], &[]))
        }

        /// Whether the reply goes on a channel of the asker's own, the
        /// last handle.
        pub fn replies_apart(&self) -> bool {
            self.kind & REPLY != 0
        }

        /// Lays the request out in `out`; its length, or None if it
        /// does not fit.
        pub fn encode(&self, out: &mut [u8]) -> Option<usize> {
            let length = 2 + 3 + self.name.len() + self.directory.len() + self.file.len() + self.arguments.len();
            if length > out.len() {
                return None;
            }
            out[..2].copy_from_slice(&[self.kind, self.flags]);
            let mut at = 2;
            // Each string after a byte saying how long it is.
            for text in [self.name, self.directory, self.file] {
                out[at] = u8::try_from(text.len()).ok()?;
                out[at + 1..at + 1 + text.len()].copy_from_slice(text.as_bytes());
                at += 1 + text.len();
            }
            out[at..length].copy_from_slice(self.arguments);
            Some(length)
        }

        pub fn decode(message: &'a [u8]) -> Option<Run<'a>> {
            let (&[kind, flags], mut rest) = message.split_first_chunk::<2>()?;
            let mut text = || -> Option<&'a str> {
                let (&length, after) = rest.split_first()?;
                let (text, after) = after.split_at_checked(length as usize)?;
                rest = after;
                core::str::from_utf8(text).ok()
            };
            let name = text().filter(|name| !name.is_empty())?;
            let directory = text()?;
            let file = text()?;
            let arguments = rest;
            // With an environment, the arguments' length must be right.
            if kind & ENVIRONMENT != 0 {
                let (said, rest) = arguments.split_first_chunk::<2>()?;
                rest.split_at_checked(usize::from(u16::from_le_bytes(*said)))?;
            }
            matches!(kind & !(REPLY | ENVIRONMENT), BUILT_IN | IMAGE).then_some(Run { kind, flags, name, directory, file, arguments })
        }
    }
}

/// What a handle permits. A right, once removed, cannot be added back:
/// duplicates and transfers keep or narrow rights, never widen them.
pub mod rights {
    /// Send on a channel.
    pub const SEND: u32 = 1 << 0;
    /// Receive from a channel.
    pub const RECEIVE: u32 = 1 << 1;
    /// Make another handle to the object.
    pub const DUPLICATE: u32 = 1 << 2;
    /// Send the handle to another process.
    pub const TRANSFER: u32 = 1 << 3;
    /// Map a memory object for reading.
    pub const READ: u32 = 1 << 4;
    /// Map a memory object for writing.
    pub const WRITE: u32 = 1 << 5;
    /// End a process. Whoever started it holds this; a copy of the
    /// handle made without it can only wait.
    pub const KILL: u32 = 1 << 6;
    pub const ALL: u32 = SEND | RECEIVE | DUPLICATE | TRANSFER | READ | WRITE | KILL;
}

/// Most bytes in one message.
pub const MESSAGE_MAX: u64 = 1024;
/// Most handles in one message.
pub const MESSAGE_HANDLES_MAX: u64 = 8;
/// Messages waiting at one channel end before sends fail with QueueFull.
pub const CHANNEL_QUEUE_MAX: usize = 32;
/// Most startup handles a process may be given, and most processes one
/// process_wait may watch.
pub const SPAWN_HANDLES_MAX: u64 = 8;
/// Most handles one object_wait may watch. More than a process can be
/// started with: init watches every service, and an answer from each.
pub const WAIT_HANDLES_MAX: u64 = 32;
/// Longest program name.
pub const PROGRAM_NAME_MAX: u64 = 64;

/// Largest memory object: 128 MiB, as big as a program file may be
/// (PROGRAM_IMAGE_MAX), which travels in one. (Per-process memory quotas
/// come later; until then this bounds what one call can take.)
pub const MEMORY_MAX: u64 = 128 << 20;
pub const PAGE_SIZE: u64 = 4096;

/// Most handles one process may hold.
pub const HANDLES_MAX: usize = 1024;

/// Why a system call failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum Error {
    /// No system call has that number.
    UnknownCall = 1,
    /// An argument is out of range.
    InvalidArgument = 2,
    /// A pointer argument points at memory the process cannot access.
    BadAddress = 3,
    /// Not a handle in this process's table (never was, or closed).
    InvalidHandle = 4,
    /// The handle lacks a right the call needs.
    AccessDenied = 5,
    /// The other end of the channel is closed.
    PeerClosed = 6,
    /// The other end has too many unread messages.
    QueueFull = 7,
    /// The waiting message is larger than the buffers given.
    BufferTooSmall = 8,
    /// The handle table is full.
    TooManyHandles = 9,
    /// The handle is to the wrong kind of object for this call.
    WrongType = 10,
    /// No such program.
    NotFound = 11,
    /// Not enough memory.
    OutOfMemory = 12,
    /// The file is not a program this system can run.
    NotExecutable = 13,
    /// A wait was cut short because the process is being ended. The
    /// process never sees it: it ends before the call returns.
    Interrupted = 14,
    /// The deadline came before what was waited for.
    TimedOut = 15,
}

impl Error {
    pub fn from_code(code: i64) -> Option<Error> {
        match code {
            1 => Some(Error::UnknownCall),
            2 => Some(Error::InvalidArgument),
            3 => Some(Error::BadAddress),
            4 => Some(Error::InvalidHandle),
            5 => Some(Error::AccessDenied),
            6 => Some(Error::PeerClosed),
            7 => Some(Error::QueueFull),
            8 => Some(Error::BufferTooSmall),
            9 => Some(Error::TooManyHandles),
            10 => Some(Error::WrongType),
            11 => Some(Error::NotFound),
            12 => Some(Error::OutOfMemory),
            13 => Some(Error::NotExecutable),
            14 => Some(Error::Interrupted),
            15 => Some(Error::TimedOut),
            _ => None,
        }
    }
}

/// Packs a result into RAX.
pub fn encode(result: Result<u64, Error>) -> u64 {
    match result {
        Ok(value) => value,
        Err(error) => (-(error as i64)) as u64,
    }
}

/// Unpacks RAX.
pub fn decode(raw: u64) -> Result<u64, Error> {
    let signed = raw as i64;
    if signed < 0 {
        Err(Error::from_code(-signed).unwrap_or(Error::InvalidArgument))
    } else {
        Ok(raw)
    }
}

/// Most bytes one debug_write may carry.
pub const DEBUG_WRITE_MAX: u64 = 4096;

/// The highest user address + 1: the lower half of the address space.
pub const USER_END: u64 = 0x0000_8000_0000_0000;

/// Exit code the kernel records for a process it killed for a fault.
pub const KILLED: i64 = -1000;
/// Exit code of a process ended by process_kill.
pub const STOPPED: i64 = -1001;
/// Exit code of a program that ended itself because nobody was reading
/// its standard output any more: the other end of the stream was
/// closed. Not the kernel's doing - the program's runtime exits with
/// this when a write finds no reader - but agreed here, so that whoever
/// started the program can tell it from a failure. In `yes | given`
/// this is how `yes` ends.
pub const NO_READER: i64 = -1002;

/// The display service's messages: the first byte says which.
pub mod display {
    /// Followed by a FramebufferInfo: what to draw on. Sent by init to
    /// each display it starts, before anything else.
    pub const INFO: u8 = 1;
    /// Followed by terminal bytes to show.
    pub const TEXT: u8 = 2;
    /// Followed by a number of ticks (u64 LE, at most STALL_MAX): a
    /// test - the display does nothing for that long, as if stuck.
    pub const STALL: u8 = 3;
    /// The size of the memory init keeps for the display's text, so
    /// that it outlives any one display process: 64 KiB.
    pub const KEPT_SIZE: u64 = 64 * 1024;
    /// Followed by a `Pointer`: what a mouse or a tablet did. Sent by
    /// the drivers of pointing devices (kbd for the PC's PS/2 mouse,
    /// hid for virtio ones), on a send-only copy of this channel.
    pub const POINTER: u8 = 4;

    /// A pointer's report: moved by (x, y) pixels (`absolute` false,
    /// right and down positive), or is at (x, y), each 0..=ABSOLUTE_MAX
    /// across the screen (`absolute` true); and the buttons held, a bit
    /// each (1 left, 2 right, 4 middle).
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct Pointer {
        pub absolute: bool,
        pub x: i32,
        pub y: i32,
        pub buttons: u8,
    }

    /// The range of an absolute position on each axis.
    pub const ABSOLUTE_MAX: i32 = 0x7FFF;

    impl Pointer {
        /// The whole message: POINTER, then whether absolute (0 or 1),
        /// x and y (i32 LE), the buttons.
        pub const SIZE: usize = 1 + 1 + 4 + 4 + 1;

        pub fn to_message(&self) -> [u8; Pointer::SIZE] {
            let mut message = [0u8; Pointer::SIZE];
            message[0] = POINTER;
            message[1] = u8::from(self.absolute);
            message[2..6].copy_from_slice(&self.x.to_le_bytes());
            message[6..10].copy_from_slice(&self.y.to_le_bytes());
            message[10] = self.buttons;
            message
        }

        /// None unless `message` is a whole POINTER message, absolute
        /// positions inside their range.
        pub fn from_message(message: &[u8]) -> Option<Pointer> {
            let message: &[u8; Pointer::SIZE] = message.try_into().ok()?;
            if message[0] != POINTER || message[1] > 1 {
                return None;
            }
            let x = i32::from_le_bytes([message[2], message[3], message[4], message[5]]);
            let y = i32::from_le_bytes([message[6], message[7], message[8], message[9]]);
            let absolute = message[1] == 1;
            if absolute && !((0..=ABSOLUTE_MAX).contains(&x) && (0..=ABSOLUTE_MAX).contains(&y)) {
                return None;
            }
            Some(Pointer { absolute, x, y, buttons: message[10] })
        }
    }
}

/// A ping: the one message a service's ping channel carries - this
/// byte, and one handle, a channel to answer on. The answer is one
/// byte. For services with no requests to answer otherwise (the
/// display, the keyboard driver), so that init can tell a service that
/// is running from one that only exists.
pub const PING: u8 = 1;

/// The longest a service can be asked to play dead (the STALL test
/// requests): 30 seconds of 100 Hz ticks.
pub const STALL_MAX: u64 = 3000;

/// The screen, as the kernel describes it to init: a linear framebuffer
/// of `pitch` bytes per line, each pixel `bpp` bits with its red, green
/// and blue channels at the given bit positions. Sent as bytes, in a
/// message, with a handle to the framebuffer's memory object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FramebufferInfo {
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub bpp: u8,
    pub red_size: u8,
    pub red_shift: u8,
    pub green_size: u8,
    pub green_shift: u8,
    pub blue_size: u8,
    pub blue_shift: u8,
}

impl FramebufferInfo {
    pub const SIZE: usize = 19;

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut b = [0; Self::SIZE];
        b[0..4].copy_from_slice(&self.width.to_le_bytes());
        b[4..8].copy_from_slice(&self.height.to_le_bytes());
        b[8..12].copy_from_slice(&self.pitch.to_le_bytes());
        b[12..].copy_from_slice(&[
            self.bpp,
            self.red_size,
            self.red_shift,
            self.green_size,
            self.green_shift,
            self.blue_size,
            self.blue_shift,
        ]);
        b
    }

    pub fn from_bytes(b: &[u8]) -> Option<FramebufferInfo> {
        if b.len() != Self::SIZE {
            return None;
        }
        let u32_at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        Some(FramebufferInfo {
            width: u32_at(0),
            height: u32_at(4),
            pitch: u32_at(8),
            bpp: b[12],
            red_size: b[13],
            red_shift: b[14],
            green_size: b[15],
            green_shift: b[16],
            blue_size: b[17],
            blue_shift: b[18],
        })
    }
}

/// The kernel's state, for `system_info`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SystemInfo {
    pub ticks: u64,
    pub ticks_per_second: u64,
    pub frames_total: u64,
    pub frames_free: u64,
    pub heap_used: u64,
    pub heap_total: u64,
    pub processes: u64,
    pub threads: u64,
    pub channels: u64,
    /// How many CPUs run programs.
    pub cpus: u64,
    /// Ticks in which a CPU was running something other than its idle
    /// thread, added up over all the CPUs. `ticks` is time on the
    /// clock; this is work done. On one CPU it can grow no faster than
    /// `ticks`; on four, up to four times as fast.
    pub busy_ticks: u64,
}

impl SystemInfo {
    pub const WORDS: usize = 11;

    pub fn to_words(&self) -> [u64; Self::WORDS] {
        [
            self.ticks,
            self.ticks_per_second,
            self.frames_total,
            self.frames_free,
            self.heap_used,
            self.heap_total,
            self.processes,
            self.threads,
            self.channels,
            self.cpus,
            self.busy_ticks,
        ]
    }

    pub fn from_words(w: [u64; Self::WORDS]) -> SystemInfo {
        SystemInfo {
            ticks: w[0],
            ticks_per_second: w[1],
            frames_total: w[2],
            frames_free: w[3],
            heap_used: w[4],
            heap_total: w[5],
            processes: w[6],
            threads: w[7],
            channels: w[8],
            cpus: w[9],
            busy_ticks: w[10],
        }
    }
}

/// One process, for `process_list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProcessEntry {
    pub pid: u64,
    /// The name, padded with zeros.
    pub name: [u8; 16],
    /// None while it runs.
    pub exit_code: Option<i64>,
}

impl ProcessEntry {
    pub const WORDS: usize = 4;
    const RUNNING: u64 = i64::MIN as u64;

    pub fn to_words(&self) -> [u64; Self::WORDS] {
        let name = |half: usize| u64::from_le_bytes(self.name[half * 8..half * 8 + 8].try_into().unwrap());
        [self.pid, name(0), name(1), self.exit_code.map_or(Self::RUNNING, |c| c as u64)]
    }

    pub fn from_words(w: [u64; Self::WORDS]) -> ProcessEntry {
        let mut name = [0; 16];
        name[..8].copy_from_slice(&w[1].to_le_bytes());
        name[8..].copy_from_slice(&w[2].to_le_bytes());
        ProcessEntry { pid: w[0], name, exit_code: (w[3] != Self::RUNNING).then_some(w[3] as i64) }
    }

    pub fn name(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(16);
        core::str::from_utf8(&self.name[..end]).unwrap_or("?")
    }
}

/// A PCI device the kernel set up for a driver, as described to init:
/// sent as b"pci" and these bytes, with handles to its resources -
/// its I/O ports, DMA memory and, if `msix`, its interrupt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PciDeviceInfo {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    /// The I/O port BAR: first port, and how many.
    pub io_base: u16,
    pub io_size: u16,
    /// Physically contiguous memory for the device to read and write:
    /// where it is (what the device must be told) and how big.
    pub dma_physical: u64,
    pub dma_size: u64,
    /// MSI-X is on, with message 0 going to the interrupt handle.
    pub msix: bool,
}

impl PciDeviceInfo {
    pub const SIZE: usize = 28;

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut b = [0; Self::SIZE];
        b[0..3].copy_from_slice(&[self.bus, self.device, self.function]);
        b[3..5].copy_from_slice(&self.vendor_id.to_le_bytes());
        b[5..7].copy_from_slice(&self.device_id.to_le_bytes());
        b[7..9].copy_from_slice(&self.io_base.to_le_bytes());
        b[9..11].copy_from_slice(&self.io_size.to_le_bytes());
        b[11..19].copy_from_slice(&self.dma_physical.to_le_bytes());
        b[19..27].copy_from_slice(&self.dma_size.to_le_bytes());
        b[27] = u8::from(self.msix);
        b
    }

    pub fn from_bytes(b: &[u8]) -> Option<PciDeviceInfo> {
        if b.len() != Self::SIZE {
            return None;
        }
        let u16_at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
        let u64_at = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap_or([0; 8]));
        Some(PciDeviceInfo {
            bus: b[0],
            device: b[1],
            function: b[2],
            vendor_id: u16_at(3),
            device_id: u16_at(5),
            io_base: u16_at(7),
            io_size: u16_at(9),
            dma_physical: u64_at(11),
            dma_size: u64_at(19),
            msix: b[27] != 0,
        })
    }
}

/// A device at a fixed place in memory (virtio-mmio) that the kernel
/// set up for a driver: these bytes, with handles to its resources -
/// the memory its registers are in, DMA memory and, if `interrupt`,
/// its interrupt.
///
/// A different length from `PciDeviceInfo`, on purpose: a driver that
/// can be given either tells them apart by that alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MmioDeviceInfo {
    /// What kind of device (virtio's numbering: 2 is a block device).
    pub device: u32,
    /// Which register interface: 1 legacy, 2 modern.
    pub version: u32,
    /// Where the registers are in physical memory, and how many bytes.
    /// The registers handle maps whole pages, so in the mapping they
    /// begin `registers % 4096` bytes in.
    pub registers: u64,
    pub registers_size: u32,
    /// As in `PciDeviceInfo`.
    pub dma_physical: u64,
    pub dma_size: u64,
    /// There is an interrupt handle. The driver must tell the device it
    /// has seen each interrupt before it waits for the next.
    pub interrupt: bool,
}

impl MmioDeviceInfo {
    pub const SIZE: usize = 37;

    /// Where the registers begin in a mapping of the registers handle.
    pub const fn offset_in_page(&self) -> usize {
        (self.registers % 4096) as usize
    }

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut b = [0; Self::SIZE];
        b[0..4].copy_from_slice(&self.device.to_le_bytes());
        b[4..8].copy_from_slice(&self.version.to_le_bytes());
        b[8..16].copy_from_slice(&self.registers.to_le_bytes());
        b[16..20].copy_from_slice(&self.registers_size.to_le_bytes());
        b[20..28].copy_from_slice(&self.dma_physical.to_le_bytes());
        b[28..36].copy_from_slice(&self.dma_size.to_le_bytes());
        b[36] = u8::from(self.interrupt);
        b
    }

    pub fn from_bytes(b: &[u8]) -> Option<MmioDeviceInfo> {
        if b.len() != Self::SIZE {
            return None;
        }
        let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap_or([0; 4]));
        let u64_at = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap_or([0; 8]));
        Some(MmioDeviceInfo {
            device: u32_at(0),
            version: u32_at(4),
            registers: u64_at(8),
            registers_size: u32_at(16),
            dma_physical: u64_at(20),
            dma_size: u64_at(28),
            interrupt: b[36] != 0,
        })
    }
}

/// The block service's protocol: a disk as numbered 512-byte sectors.
///
/// A request: [op] [sector: u64 LE] [data: one sector, for WRITE]
/// A reply:   [status] [payload]
///
/// As with files, each request carries one handle - the end of a fresh
/// channel to send the reply on.
pub mod block {
    pub const SECTOR: usize = 512;

    /// Payload: the number of sectors (u64 LE), then 1 if read-only.
    pub const INFO: u8 = 1;
    /// Payload: the sector. One sector, carried in the messages: the
    /// reply holds it (READ), or the request does (WRITE).
    pub const READ: u8 = 2;
    pub const WRITE: u8 = 3;
    /// Payload: the first sector (u64 LE) and how many (u32 LE, 1 to
    /// SECTORS_MAX). The data does not travel in messages: the request
    /// carries a second handle, after the reply channel - a memory
    /// object, whose first `count` sectors' worth of bytes the driver
    /// fills (READ_MANY; the handle needs READ and WRITE) or writes to
    /// the disk (WRITE_MANY; READ). The driver maps it for the one
    /// request and keeps nothing: any client, any buffer, any time.
    pub const READ_MANY: u8 = 4;
    pub const WRITE_MANY: u8 = 5;
    /// Most sectors one READ_MANY or WRITE_MANY moves: 128 KiB. (It was
    /// 16 KiB; a program of 80 MB took 20,000 requests to load - journal
    /// entry 080.)
    pub const SECTORS_MAX: u32 = 256;
    pub const MANY_REQUEST_SIZE: usize = 1 + 8 + 4;
    /// Payload: a number of ticks (u64 LE, at most STALL_MAX). A test:
    /// answered, and then the driver does nothing for that long.
    pub const STALL: u8 = 6;

    pub const OK: u8 = 0;
    /// The disk reported an error.
    pub const IO_ERROR: u8 = 1;
    /// No such sector.
    pub const OUT_OF_RANGE: u8 = 2;
    pub const READ_ONLY: u8 = 3;
    pub const BAD_REQUEST: u8 = 4;

    pub const REQUEST_SIZE: usize = 1 + 8;

    /// Not part of the protocol: the exit code of the driver's test
    /// (`blkcheck`) when the disk it was given is not the one made for
    /// it - so that whoever started it knows the disk was left alone.
    pub const NOT_THE_TEST_DISK: i64 = 77;

    pub fn describe(status: u8) -> &'static str {
        match status {
            OK => "ok",
            IO_ERROR => "the disk reported an error",
            OUT_OF_RANGE => "no such sector",
            READ_ONLY => "the disk is read-only",
            _ => "the disk service did not understand",
        }
    }
}

/// The network card service's protocol: whole Ethernet frames, in and
/// out. Nothing here knows what is in a frame beyond its first six
/// bytes (who it is for): that is the business of whoever uses the
/// card.
///
/// A frame is up to 1514 bytes (FRAME_MAX: two addresses, a type, and
/// 1500 bytes of payload; the checksum at the end is the card's) and a
/// message carries 1024. So a frame travels in pieces, each a message:
/// zero or more `PART`s and then one `LAST`, each `[op] [bytes]`. Both
/// directions use the same two messages; `split` cuts a frame up and
/// `Assembler` puts one together.
///
/// Requests to the driver, on its request channel:
///
///   INFO, STATS, STALL   as with the disk: one handle, the end of a
///                        fresh channel, and the reply `[status]
///                        [payload]` comes back on it
///   LISTEN               one handle: a channel the driver is to send
///                        frames it receives on, from now on. Followed
///                        by up to four frame types (u16 each, most
///                        significant byte first, as in a frame): only
///                        frames of those types. With none: every
///                        frame nobody else asked for by type. Whoever
///                        listened for the same before listens no
///                        longer (`Listeners` has the rule). No reply
///   PART, LAST           a frame to send. No handle and no reply: a
///                        network may lose a frame anyway, and a reply
///                        per frame would double the cost of each. What
///                        was refused is counted (STATS)
///   DROP                 forget the PARTs sent so far: for a sender
///                        that could not finish a frame (the queue was
///                        full for its LAST), before it begins another
///
/// One sender at a time: pieces of two frames on one channel would be
/// put together as one.
///
/// A frame the listener has no room for is dropped, and counted: the
/// driver never waits for a client.
pub mod nic {
    /// The longest frame, without its checksum.
    pub const FRAME_MAX: usize = 1514;
    /// The shortest that says anything: two addresses and a type.
    pub const FRAME_MIN: usize = 14;
    /// The shortest a card may put on the wire, again without the
    /// checksum; the driver pads a shorter one with zeroes.
    pub const WIRE_MIN: usize = 60;
    /// An address on an Ethernet: six bytes.
    pub const ADDRESS: usize = 6;
    pub const BROADCAST: [u8; ADDRESS] = [0xFF; ADDRESS];

    /// Reply payload: the card's address (6 bytes), then 1 if the link
    /// is up.
    pub const INFO: u8 = 1;
    pub const INFO_SIZE: usize = ADDRESS + 1;
    /// Bytes of a frame, with more to come.
    pub const PART: u8 = 2;
    /// The last bytes of a frame - or all of it.
    pub const LAST: u8 = 3;
    pub const LISTEN: u8 = 4;
    /// Reply payload: the counters, each u64 LE, in `Stats`' order.
    pub const STATS: u8 = 5;
    /// Payload: a number of ticks (u64 LE, at most STALL_MAX). A test:
    /// answered, and then the driver does nothing for that long.
    pub const STALL: u8 = 6;
    /// The pieces so far are of a frame that will not be finished.
    pub const DROP: u8 = 7;

    pub const OK: u8 = 0;
    pub const BAD_REQUEST: u8 = 1;

    /// Most bytes of a frame in one message.
    pub const PIECE_MAX: usize = super::MESSAGE_MAX as usize - 1;

    /// What the driver has counted since it started.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct Stats {
        /// Frames the card handed over.
        pub received: u64,
        /// Of those: passed on to the listener.
        pub delivered: u64,
        /// Addressed to another card, and not to everyone.
        pub not_ours: u64,
        /// Nobody was listening, or the listener had no room.
        pub dropped: u64,
        /// Frames given to the card to send.
        pub sent: u64,
        /// Frames a client asked to send that were not: too short, too
        /// long, or the card had no room.
        pub refused: u64,
    }

    impl Stats {
        pub const SIZE: usize = 6 * 8;

        pub fn to_bytes(&self) -> [u8; Stats::SIZE] {
            let mut bytes = [0u8; Stats::SIZE];
            let fields = [self.received, self.delivered, self.not_ours, self.dropped, self.sent, self.refused];
            for (chunk, field) in bytes.as_chunks_mut::<8>().0.iter_mut().zip(fields) {
                *chunk = field.to_le_bytes();
            }
            bytes
        }

        pub fn from_bytes(bytes: &[u8]) -> Option<Stats> {
            if bytes.len() != Stats::SIZE {
                return None;
            }
            let mut fields = bytes.as_chunks::<8>().0.iter().map(|&chunk| u64::from_le_bytes(chunk));
            let mut next = || fields.next().unwrap_or(0);
            Some(Stats { received: next(), delivered: next(), not_ours: next(), dropped: next(), sent: next(), refused: next() })
        }
    }

    /// Is a frame that begins `frame` for the card whose address is
    /// `ours`? For it alone, or for everyone. (Not: for a group. No
    /// one here has joined one yet.)
    pub fn for_us(frame: &[u8], ours: &[u8; ADDRESS]) -> bool {
        frame.get(..ADDRESS).is_some_and(|to| to == ours || to == BROADCAST)
    }

    /// `frame` as the messages it travels in: (op, bytes) pairs, the
    /// last one LAST. An empty frame is one empty LAST.
    pub fn split(frame: &[u8]) -> impl Iterator<Item = (u8, &[u8])> {
        let pieces = frame.len().div_ceil(PIECE_MAX).max(1);
        (0..pieces).map(move |n| {
            let piece = &frame[n * PIECE_MAX..frame.len().min((n + 1) * PIECE_MAX)];
            (if n + 1 == pieces { LAST } else { PART }, piece)
        })
    }

    /// Puts a frame together from its messages.
    pub struct Assembler {
        frame: [u8; FRAME_MAX],
        length: usize,
        /// The frame so far is longer than a frame may be: what is left
        /// of it is thrown away, up to and with its LAST.
        too_long: bool,
    }

    /// What one message did.
    #[derive(Debug, PartialEq, Eq)]
    pub enum Fed<'a> {
        /// A piece; the frame is not whole yet.
        More,
        /// The frame, whole.
        Frame(&'a [u8]),
        /// The frame this message ended was longer than FRAME_MAX.
        TooLong,
        /// Not a PART, a LAST or a DROP.
        NotAFrame,
    }

    impl Assembler {
        pub const fn new() -> Assembler {
            Assembler { frame: [0; FRAME_MAX], length: 0, too_long: false }
        }

        /// One message, as it was received (the op first).
        pub fn feed(&mut self, message: &[u8]) -> Fed<'_> {
            let Some((&op, piece)) = message.split_first() else { return Fed::NotAFrame };
            if op == DROP {
                self.length = 0;
                self.too_long = false;
                return Fed::More;
            }
            if op != PART && op != LAST {
                return Fed::NotAFrame;
            }
            match self.frame.get_mut(self.length..self.length + piece.len()) {
                Some(room) if !self.too_long => {
                    room.copy_from_slice(piece);
                    self.length += piece.len();
                }
                _ => self.too_long = true,
            }
            if op == PART {
                return Fed::More;
            }
            let (length, too_long) = (self.length, self.too_long);
            self.length = 0;
            self.too_long = false;
            if too_long {
                Fed::TooLong
            } else {
                Fed::Frame(&self.frame[..length])
            }
        }
    }

    impl Default for Assembler {
        fn default() -> Self {
            Self::new()
        }
    }

    /// Most frame types one LISTEN may name, and most listeners the
    /// driver keeps at once.
    pub const LISTEN_TYPES_MAX: usize = 4;
    pub const LISTENERS_MAX: usize = 4;

    /// The type of a frame: bytes 12 and 13, which say what its payload
    /// is (0x0800: an IPv4 packet).
    pub fn frame_type(frame: &[u8]) -> Option<u16> {
        Some(u16::from_be_bytes([*frame.get(12)?, *frame.get(13)?]))
    }

    /// The types a LISTEN names: what follows its first byte. None if
    /// that is not a list of at most LISTEN_TYPES_MAX types.
    pub fn listen_types(payload: &[u8]) -> Option<([u16; LISTEN_TYPES_MAX], usize)> {
        if !payload.len().is_multiple_of(2) || payload.len() > 2 * LISTEN_TYPES_MAX {
            return None;
        }
        let mut types = [0u16; LISTEN_TYPES_MAX];
        for (kind, bytes) in types.iter_mut().zip(payload.as_chunks::<2>().0) {
            *kind = u16::from_be_bytes(*bytes);
        }
        Some((types, payload.len() / 2))
    }

    /// One listener: who (for the driver, a channel), and for what.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Listener<H> {
        pub who: H,
        types: [u16; LISTEN_TYPES_MAX],
        /// How many of `types` count. None: every frame no one else
        /// asked for by type.
        count: usize,
        /// It was sent the first piece of a frame and not the last: it
        /// must be told to forget it (DROP) before the next.
        pub torn: bool,
    }

    impl<H> Listener<H> {
        fn wants(&self, kind: u16) -> bool {
            self.types[..self.count].contains(&kind)
        }
    }

    /// Who gets which frames. A frame goes to the listener that named
    /// its type; if none did, to the one that named none.
    ///
    /// A new listener takes over what it asks for: a type it names is
    /// taken from whoever had it, and a listener left with nothing -
    /// or an older one that also named no types - is closed. So a
    /// program that starts again simply asks again, and one that
    /// listens for a type of its own disturbs nobody.
    pub struct Listeners<H: Copy, const N: usize> {
        slots: [Option<Listener<H>>; N],
    }

    impl<H: Copy, const N: usize> Listeners<H, N> {
        pub fn new() -> Self {
            Listeners { slots: [None; N] }
        }

        /// Adds a listener for `types` (at most LISTEN_TYPES_MAX are
        /// looked at). `closed` is called for every listener this
        /// ends, and for `who` itself if there is no room - false is
        /// returned then.
        pub fn add(&mut self, who: H, types: &[u16], mut closed: impl FnMut(H)) -> bool {
            let count = types.len().min(LISTEN_TYPES_MAX);
            let mut mine = [0u16; LISTEN_TYPES_MAX];
            mine[..count].copy_from_slice(&types[..count]);
            for slot in &mut self.slots {
                let Some(old) = slot else { continue };
                let ended = if old.count == 0 {
                    count == 0
                } else {
                    // Keep the types the new one did not name.
                    let mut kept = 0;
                    for n in 0..old.count {
                        if !mine[..count].contains(&old.types[n]) {
                            old.types[kept] = old.types[n];
                            kept += 1;
                        }
                    }
                    old.count = kept;
                    kept == 0
                };
                if ended {
                    closed(old.who);
                    *slot = None;
                }
            }
            match self.slots.iter_mut().find(|slot| slot.is_none()) {
                Some(slot) => {
                    *slot = Some(Listener { who, types: mine, count, torn: false });
                    true
                }
                None => {
                    closed(who);
                    false
                }
            }
        }

        /// The listener a frame of type `kind` goes to, if any.
        pub fn find(&mut self, kind: u16) -> Option<&mut Listener<H>> {
            let by_type = self.slots.iter().position(|slot| slot.as_ref().is_some_and(|listener| listener.wants(kind)));
            let found = by_type.or_else(|| self.slots.iter().position(|slot| slot.as_ref().is_some_and(|listener| listener.count == 0)))?;
            self.slots[found].as_mut()
        }

        /// Forgets the listener `who` (it has gone). True if it was one.
        pub fn remove(&mut self, who: H) -> bool
        where
            H: PartialEq,
        {
            let mut found = false;
            for slot in &mut self.slots {
                if slot.as_ref().is_some_and(|listener| listener.who == who) {
                    *slot = None;
                    found = true;
                }
            }
            found
        }

        pub fn count(&self) -> usize {
            self.slots.iter().flatten().count()
        }
    }

    impl<H: Copy, const N: usize> Default for Listeners<H, N> {
        fn default() -> Self {
            Self::new()
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        extern crate std;
        use std::vec::Vec;

        #[test]
        fn a_frame_goes_to_whoever_named_its_type() {
            let mut listeners = Listeners::<u32, 4>::new();
            let mut closed = Vec::new();
            assert!(listeners.find(0x0800).is_none());
            assert!(listeners.add(1, &[], |who| closed.push(who)));
            assert_eq!(listeners.find(0x0800).map(|l| l.who), Some(1), "no types named: everything");
            assert!(listeners.add(2, &[0x0800, 0x0806], |who| closed.push(who)));
            assert_eq!(listeners.find(0x0800).map(|l| l.who), Some(2));
            assert_eq!(listeners.find(0x0806).map(|l| l.who), Some(2));
            assert_eq!(listeners.find(0x88B5).map(|l| l.who), Some(1), "what nobody named");
            assert!(closed.is_empty());

            // A type taken over: the old listener keeps its other one.
            assert!(listeners.add(3, &[0x0806], |who| closed.push(who)));
            assert_eq!(listeners.find(0x0806).map(|l| l.who), Some(3));
            assert_eq!(listeners.find(0x0800).map(|l| l.who), Some(2));
            assert!(closed.is_empty());
            // And then its last: it is closed.
            assert!(listeners.add(4, &[0x0800], |who| closed.push(who)));
            assert_eq!(closed, [2]);
            assert_eq!(listeners.find(0x0800).map(|l| l.who), Some(4));
            // A second listener for everything else replaces the first,
            // and nobody who named a type.
            assert!(listeners.add(5, &[], |who| closed.push(who)));
            assert_eq!(closed, [2, 1]);
            assert_eq!(listeners.find(0x1234).map(|l| l.who), Some(5));
            assert_eq!(listeners.find(0x0806).map(|l| l.who), Some(3));
            assert_eq!(listeners.count(), 3);
        }

        #[test]
        fn listeners_come_and_go() {
            let mut listeners = Listeners::<u32, 2>::new();
            let mut closed = Vec::new();
            assert!(listeners.add(1, &[1], |who| closed.push(who)));
            assert!(listeners.add(2, &[2], |who| closed.push(who)));
            // No room: the newcomer is the one turned away.
            assert!(!listeners.add(3, &[3], |who| closed.push(who)));
            assert_eq!(closed, [3]);
            assert_eq!(listeners.find(3).map(|l| l.who), None);
            // Unless it takes someone's place.
            assert!(listeners.add(4, &[2], |who| closed.push(who)));
            assert_eq!(closed, [3, 2]);
            assert!(listeners.remove(1));
            assert!(!listeners.remove(1));
            assert_eq!(listeners.find(1).map(|l| l.who), None);
            assert_eq!(listeners.count(), 1);
            // What a listener is told is remembered with it.
            listeners.find(2).unwrap().torn = true;
            assert!(listeners.find(2).unwrap().torn);
            // More types than can be named: the first four count.
            assert!(listeners.add(9, &[10, 11, 12, 13, 14], |who| closed.push(who)));
            assert_eq!(listeners.find(13).map(|l| l.who), Some(9));
            assert_eq!(listeners.find(14).map(|l| l.who), None);
        }

        #[test]
        fn the_types_of_a_listen_and_of_a_frame() {
            assert_eq!(listen_types(&[]), Some(([0; 4], 0)));
            assert_eq!(listen_types(&[0x08, 0x00, 0x08, 0x06]), Some(([0x0800, 0x0806, 0, 0], 2)));
            assert_eq!(listen_types(&[1, 2, 3, 4, 5, 6, 7, 8]).map(|(_, count)| count), Some(4));
            assert_eq!(listen_types(&[0x08]), None);
            assert_eq!(listen_types(&[0; 10]), None);
            let mut frame = [0u8; 14];
            frame[12..].copy_from_slice(&[0x88, 0xB5]);
            assert_eq!(frame_type(&frame), Some(0x88B5));
            assert_eq!(frame_type(&frame[..13]), None);
        }

        fn pattern(length: usize) -> Vec<u8> {
            (0..length).map(|i| (i * 7 + (i >> 8)) as u8).collect()
        }

        /// A frame through `split` and back through an `Assembler`.
        fn round_trip(assembler: &mut Assembler, frame: &[u8]) -> Vec<u8> {
            let mut messages: Vec<Vec<u8>> = Vec::new();
            for (op, piece) in split(frame) {
                let mut message = std::vec![op];
                message.extend_from_slice(piece);
                assert!(message.len() <= crate::MESSAGE_MAX as usize);
                messages.push(message);
            }
            let (last, parts) = messages.split_last().unwrap();
            for part in parts {
                assert_eq!(assembler.feed(part), Fed::More);
            }
            match assembler.feed(last) {
                Fed::Frame(whole) => whole.to_vec(),
                other => panic!("{other:?}"),
            }
        }

        #[test]
        fn a_frame_of_any_length_survives_being_cut_into_messages() {
            let mut assembler = Assembler::new();
            // Around every place a cut falls, and both ends.
            for length in [0, 1, FRAME_MIN, 60, PIECE_MAX - 1, PIECE_MAX, PIECE_MAX + 1, 1500, FRAME_MAX - 1, FRAME_MAX] {
                let frame = pattern(length);
                assert_eq!(round_trip(&mut assembler, &frame), frame, "{length} bytes");
            }
        }

        #[test]
        fn how_many_messages_a_frame_takes() {
            let count = |length: usize| split(&pattern(length)).count();
            assert_eq!(count(0), 1);
            assert_eq!(count(PIECE_MAX), 1);
            assert_eq!(count(PIECE_MAX + 1), 2);
            assert_eq!(count(FRAME_MAX), 2);
            // Only the last is LAST.
            let ops: Vec<u8> = split(&pattern(FRAME_MAX)).map(|(op, _)| op).collect();
            assert_eq!(ops, [PART, LAST]);
        }

        #[test]
        fn a_frame_too_long_is_refused_whole_and_the_next_one_is_not() {
            let mut assembler = Assembler::new();
            let mut part = std::vec![PART];
            part.extend_from_slice(&pattern(PIECE_MAX));
            assert_eq!(assembler.feed(&part), Fed::More);
            assert_eq!(assembler.feed(&part), Fed::More); // 2046 bytes by now
            assert_eq!(assembler.feed(&[LAST, 1, 2, 3]), Fed::TooLong);
            // Nothing of it is left over.
            assert_eq!(assembler.feed(&[LAST, 9, 8]), Fed::Frame(&[9, 8]));
        }

        #[test]
        fn one_byte_over_is_too_long() {
            let mut assembler = Assembler::new();
            let frame = pattern(FRAME_MAX + 1);
            let mut result = None;
            for (op, piece) in split(&frame) {
                let mut message = std::vec![op];
                message.extend_from_slice(piece);
                result = Some(matches!(assembler.feed(&message), Fed::TooLong));
            }
            assert_eq!(result, Some(true));
        }

        #[test]
        fn what_is_not_a_piece_of_a_frame_changes_nothing() {
            let mut assembler = Assembler::new();
            assert_eq!(assembler.feed(&[PART, 1, 2]), Fed::More);
            assert_eq!(assembler.feed(&[]), Fed::NotAFrame);
            assert_eq!(assembler.feed(&[INFO]), Fed::NotAFrame);
            assert_eq!(assembler.feed(&[LAST, 3]), Fed::Frame(&[1, 2, 3]));
        }

        #[test]
        fn pieces_of_a_frame_given_up_are_forgotten() {
            let mut assembler = Assembler::new();
            assert_eq!(assembler.feed(&[PART, 0xEE, 0xEE]), Fed::More);
            assert_eq!(assembler.feed(&[DROP]), Fed::More);
            assert_eq!(assembler.feed(&[LAST, 1]), Fed::Frame(&[1]));
            // Also after too much: the next frame starts clean.
            let mut part = std::vec![PART];
            part.extend_from_slice(&pattern(PIECE_MAX));
            assembler.feed(&part);
            assembler.feed(&part);
            assert_eq!(assembler.feed(&[DROP]), Fed::More);
            assert_eq!(assembler.feed(&[LAST, 2]), Fed::Frame(&[2]));
        }

        #[test]
        fn who_a_frame_is_for() {
            let ours = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
            let mut frame = pattern(FRAME_MIN);
            frame[..6].copy_from_slice(&ours);
            assert!(for_us(&frame, &ours));
            frame[..6].copy_from_slice(&BROADCAST);
            assert!(for_us(&frame, &ours));
            frame[..6].copy_from_slice(&[0x52, 0x54, 0x00, 0x12, 0x34, 0x57]);
            assert!(!for_us(&frame, &ours));
            // A group address (the lowest bit of the first byte).
            frame[..6].copy_from_slice(&[0x01, 0x00, 0x5E, 0, 0, 1]);
            assert!(!for_us(&frame, &ours));
            // Too short to say.
            assert!(!for_us(&ours[..5], &ours));
        }

        #[test]
        fn the_counters_survive_the_trip_through_bytes() {
            let stats = Stats { received: 1 << 40, delivered: 2, not_ours: 3, dropped: 4, sent: 5, refused: 6 };
            assert_eq!(Stats::from_bytes(&stats.to_bytes()), Some(stats));
            assert_eq!(Stats::from_bytes(&stats.to_bytes()[1..]), None);
        }
    }
}

/// The network service's protocol: what a program asks of the machine's
/// network stack (`user/src/bin/net.rs`), which owns the card.
///
/// Requests, each with the end of a fresh channel for the reply
/// `[status] [payload]`:
///
///   INFO      who this machine is on its network, and what the stack
///             has counted (`Info`)
///   PING      `Ping`: send an echo request, and answer when its reply
///             comes - or when the wait is over (TIMED_OUT), or when
///             nobody on the wire has the address (UNREACHABLE)
///   UDP_BIND  a port (u16 LE; 0: any free one), and a second handle:
///             one end of a channel that from now on *is* the socket.
///             Reply payload: the port (u16 LE)
///
/// A socket is a channel, both ways: each message one datagram,
/// `[address: 4 bytes] [port: u16 LE] [data]`. Sent by the program,
/// the address and port are where it goes; sent by the service, where
/// it came from. Closing the channel closes the socket and frees the
/// port - also when a program ends without saying so.
///
/// No reply for a datagram: a network promises nothing about one, and
/// neither does this. A datagram a program has no room for is dropped
/// and counted.
pub mod net {
    pub const INFO: u8 = 1;
    pub const PING: u8 = 2;
    pub const UDP_BIND: u8 = 3;
    /// `[TCP_CONNECT, address (4 bytes), port (2, low byte first)]`
    /// with two handles: where to answer, and one end of a channel -
    /// the connection, if it is made (see "streams" below). The answer
    /// comes when it is made, or cannot be: `[OK, port here (2)]`.
    pub const TCP_CONNECT: u8 = 4;
    /// `[TCP_LISTEN, port (2)]` with two handles: where to answer, and
    /// one end of a channel - the listener. Answer: `[OK, port (2)]`.
    /// On the listener's channel the program says `[LISTENER_ACCEPT]`
    /// for each connection it is ready to take, and each connection
    /// made to the port arrives as a message: who it is with
    /// (`[address, port]`) and a handle - one end of the connection's
    /// own channel. Closing the listener's channel ends the listening.
    pub const TCP_LISTEN: u8 = 5;
    /// On a listener's channel, from the program: "one more".
    pub const LISTENER_ACCEPT: u8 = 1;

    pub const OK: u8 = 0;
    pub const BAD_REQUEST: u8 = 1;
    /// A ping: no answer came in the time given.
    pub const TIMED_OUT: u8 = 2;
    /// Nobody on the wire has the address (or the gateway's).
    pub const UNREACHABLE: u8 = 3;
    /// Another program has that port.
    pub const IN_USE: u8 = 4;
    /// No more sockets, or pings, at once.
    pub const NO_ROOM: u8 = 5;
    /// More data than a packet carries.
    pub const TOO_BIG: u8 = 6;
    /// The address is nowhere a packet can be sent.
    pub const NO_ROUTE: u8 = 7;
    /// The machine answered: nothing is listening on that port.
    pub const REFUSED: u8 = 8;
    /// The other side cut the connection off, or forgot it.
    pub const RESET: u8 = 9;

    pub fn describe(status: u8) -> &'static str {
        match status {
            OK => "ok",
            TIMED_OUT => "no answer",
            UNREACHABLE => "nobody on this network has that address",
            IN_USE => "another program has that port",
            NO_ROOM => "the network service has no room for more",
            TOO_BIG => "more than one packet carries",
            NO_ROUTE => "that address is not on this network, and there is no way out of it",
            REFUSED => "nothing is listening on that port there",
            RESET => "the other side cut the connection off",
            _ => "the network service did not understand",
        }
    }

    /// The first bytes of a socket's message: an address and a port.
    pub const DATAGRAM_HEADER: usize = 6;
    /// The most data in one datagram here: what a message carries.
    /// (A packet could carry 1472 bytes; a datagram that arrives with
    /// more than this is dropped and counted.)
    pub const DATAGRAM_MAX: usize = super::MESSAGE_MAX as usize - DATAGRAM_HEADER;

    /// Writes a socket's message into `out`; its length. None if the
    /// data is more than DATAGRAM_MAX or `out` too small.
    pub fn datagram_encode(out: &mut [u8], address: [u8; 4], port: u16, data: &[u8]) -> Option<usize> {
        let length = DATAGRAM_HEADER + data.len();
        if data.len() > DATAGRAM_MAX || out.len() < length {
            return None;
        }
        out[..4].copy_from_slice(&address);
        out[4..6].copy_from_slice(&port.to_le_bytes());
        out[6..length].copy_from_slice(data);
        Some(length)
    }

    /// A socket's message: the address, the port, the data.
    pub fn datagram_decode(message: &[u8]) -> Option<([u8; 4], u16, &[u8])> {
        let (header, data) = message.split_at_checked(DATAGRAM_HEADER)?;
        Some(([header[0], header[1], header[2], header[3]], u16::from_le_bytes([header[4], header[5]]), data))
    }

    // Streams: what travels on a TCP connection's channel, in both
    // directions. The first byte of each message says what it is.

    /// `[STREAM_DATA, bytes...]`: the next bytes of the stream. From a
    /// program: to be sent. To a program: what arrived.
    pub const STREAM_DATA: u8 = 1;
    /// `[STREAM_END]`: no more will come this way. From a program: it
    /// has no more to send (the other direction stays open). To a
    /// program: the other side has no more to send.
    pub const STREAM_END: u8 = 2;
    /// `[STREAM_FAILED, status]`, to a program only, and the last
    /// message there will be: the connection ended, and not by both
    /// sides saying so.
    pub const STREAM_FAILED: u8 = 3;
    /// The most bytes in one STREAM_DATA message.
    pub const STREAM_DATA_MAX: usize = super::MESSAGE_MAX as usize - 1;

    /// How long a program should be prepared to wait for the answer to
    /// TCP_CONNECT, in ticks: the SYN is sent five times over 31
    /// seconds before the service gives up.
    pub const CONNECT_WAIT: u64 = 3500;

    pub fn connect_encode(to: [u8; 4], port: u16) -> [u8; 7] {
        let [low, high] = port.to_le_bytes();
        [TCP_CONNECT, to[0], to[1], to[2], to[3], low, high]
    }

    /// None if `request` is not a TCP_CONNECT, or names port 0.
    pub fn connect_decode(request: &[u8]) -> Option<([u8; 4], u16)> {
        match *request {
            [TCP_CONNECT, a, b, c, d, low, high] if (low, high) != (0, 0) => Some(([a, b, c, d], u16::from_le_bytes([low, high]))),
            _ => None,
        }
    }

    /// The most data a ping carries, and the longest it waits (ticks).
    pub const PING_DATA_MAX: u16 = 1472;
    pub const PING_WAIT_MAX: u32 = 3000;
    /// A PING's reply payload: who answered (4 bytes) and the "time to
    /// live" its packet arrived with.
    pub const PING_REPLY_SIZE: usize = 5;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Ping {
        pub to: [u8; 4],
        /// A number of the asker's choosing, sent and expected back.
        pub sequence: u16,
        /// How many bytes of data to send along.
        pub length: u16,
        /// How long to wait for the answer, in ticks.
        pub wait: u32,
    }

    impl Ping {
        pub const SIZE: usize = 1 + 4 + 2 + 2 + 4;

        pub fn to_bytes(&self) -> [u8; Ping::SIZE] {
            let mut bytes = [0u8; Ping::SIZE];
            bytes[0] = PING;
            bytes[1..5].copy_from_slice(&self.to);
            bytes[5..7].copy_from_slice(&self.sequence.to_le_bytes());
            bytes[7..9].copy_from_slice(&self.length.to_le_bytes());
            bytes[9..13].copy_from_slice(&self.wait.to_le_bytes());
            bytes
        }

        /// None if `request` is not a PING, or asks for more than
        /// PING_DATA_MAX bytes or PING_WAIT_MAX ticks, or for no wait.
        pub fn from_bytes(request: &[u8]) -> Option<Ping> {
            let bytes: &[u8; Ping::SIZE] = request.try_into().ok()?;
            let ping = Ping {
                to: [bytes[1], bytes[2], bytes[3], bytes[4]],
                sequence: u16::from_le_bytes([bytes[5], bytes[6]]),
                length: u16::from_le_bytes([bytes[7], bytes[8]]),
                wait: u32::from_le_bytes([bytes[9], bytes[10], bytes[11], bytes[12]]),
            };
            (bytes[0] == PING && ping.length <= PING_DATA_MAX && (1..=PING_WAIT_MAX).contains(&ping.wait)).then_some(ping)
        }
    }

    /// Who this machine is, and what its network stack has counted.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct Info {
        pub mac: [u8; 6],
        pub address: [u8; 4],
        /// How many leading bits of the address name the network.
        pub prefix: u8,
        pub gateway: [u8; 4],
        pub counters: [u64; COUNTERS],
    }

    /// `Info::counters`, in this order.
    pub const COUNTERS: usize = 14;
    pub const FRAMES_IN: usize = 0;
    pub const FRAMES_OUT: usize = 1;
    /// "Who has this address?" answered.
    pub const ARP_ANSWERED: usize = 2;
    /// Pings answered.
    pub const ECHOES_ANSWERED: usize = 3;
    pub const DATAGRAMS_IN: usize = 4;
    /// Malformed, or a wrong checksum.
    pub const BAD: usize = 5;
    pub const NOT_FOR_US: usize = 6;
    /// Fragments, and protocols not spoken.
    pub const UNSUPPORTED: usize = 7;
    /// Not sent: nobody had the address, or too many were waiting.
    pub const UNDELIVERED: usize = 8;
    /// Datagrams for a port no program has.
    pub const NO_SUCH_PORT: usize = 9;
    /// Datagrams a program had no room for, or that were too long for
    /// a message.
    pub const NOT_TAKEN: usize = 10;
    /// TCP segments that arrived whole.
    pub const SEGMENTS_IN: usize = 11;
    /// TCP segments sent a second time.
    pub const SEGMENTS_AGAIN: usize = 12;
    /// TCP segments answered "no such connection here".
    pub const SEGMENTS_REFUSED: usize = 13;

    impl Info {
        pub const SIZE: usize = 6 + 4 + 1 + 4 + COUNTERS * 8;

        pub fn to_bytes(&self) -> [u8; Info::SIZE] {
            let mut bytes = [0u8; Info::SIZE];
            bytes[..6].copy_from_slice(&self.mac);
            bytes[6..10].copy_from_slice(&self.address);
            bytes[10] = self.prefix;
            bytes[11..15].copy_from_slice(&self.gateway);
            for (chunk, counter) in bytes[15..].as_chunks_mut::<8>().0.iter_mut().zip(self.counters) {
                *chunk = counter.to_le_bytes();
            }
            bytes
        }

        pub fn from_bytes(bytes: &[u8]) -> Option<Info> {
            let bytes: &[u8; Info::SIZE] = bytes.try_into().ok()?;
            let mut info = Info { prefix: bytes[10], ..Info::default() };
            info.mac.copy_from_slice(&bytes[..6]);
            info.address.copy_from_slice(&bytes[6..10]);
            info.gateway.copy_from_slice(&bytes[11..15]);
            for (counter, chunk) in info.counters.iter_mut().zip(bytes[15..].as_chunks::<8>().0) {
                *counter = u64::from_le_bytes(*chunk);
            }
            Some(info)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_connect_request_there_and_back() {
            let request = connect_encode([10, 0, 2, 2], 8080);
            assert_eq!(request, [TCP_CONNECT, 10, 0, 2, 2, 0x90, 0x1F]);
            assert_eq!(connect_decode(&request), Some(([10, 0, 2, 2], 8080)));
            assert_eq!(connect_decode(&request[..6]), None);
            assert_eq!(connect_decode(&[TCP_CONNECT, 10, 0, 2, 2, 0x90, 0x1F, 0]), None);
            assert_eq!(connect_decode(&[TCP_LISTEN, 10, 0, 2, 2, 0x90, 0x1F]), None);
            assert_eq!(connect_decode(&connect_encode([10, 0, 2, 2], 0)), None, "there is no port 0");
            // A stream's message is a whole message: one byte of kind,
            // the rest data.
            assert_eq!(STREAM_DATA_MAX, 1023);
            // Every status has words of its own.
            let words: [&str; 10] = core::array::from_fn(|status| describe(status as u8));
            for (n, text) in words.iter().enumerate() {
                assert!(!words[..n].contains(text), "{text}");
            }
        }

        #[test]
        fn a_datagram_message_there_and_back() {
            let mut message = [0u8; super::super::MESSAGE_MAX as usize];
            let length = datagram_encode(&mut message, [10, 0, 2, 2], 5000, b"hello").unwrap();
            assert_eq!(&message[..length], [10, 0, 2, 2, 0x88, 0x13, b'h', b'e', b'l', b'l', b'o']);
            assert_eq!(datagram_decode(&message[..length]), Some(([10, 0, 2, 2], 5000, &b"hello"[..])));
            assert_eq!(datagram_decode(&message[..6]), Some(([10, 0, 2, 2], 5000, &b""[..])));
            assert_eq!(datagram_decode(&message[..5]), None);
            // The most, and one more.
            assert_eq!(datagram_encode(&mut message, [0; 4], 1, &[7; DATAGRAM_MAX]), Some(1024));
            assert_eq!(datagram_encode(&mut message, [0; 4], 1, &[7; DATAGRAM_MAX + 1]), None);
            assert_eq!(datagram_encode(&mut [0u8; 8], [0; 4], 1, b"abc"), None);
            // However much room there is: a message carries no more.
            assert_eq!(datagram_encode(&mut [0u8; 2048], [0; 4], 1, &[7; DATAGRAM_MAX + 1]), None);
        }

        #[test]
        fn a_ping_request_there_and_back() {
            let ping = Ping { to: [10, 0, 2, 2], sequence: 0x0102, length: 56, wait: 100 };
            let bytes = ping.to_bytes();
            assert_eq!(bytes, [PING, 10, 0, 2, 2, 2, 1, 56, 0, 100, 0, 0, 0]);
            assert_eq!(Ping::from_bytes(&bytes), Some(ping));
            assert_eq!(Ping::from_bytes(&bytes[..12]), None);
            let mut wrong = bytes;
            wrong[0] = INFO;
            assert_eq!(Ping::from_bytes(&wrong), None);
            for refused in [
                Ping { length: PING_DATA_MAX + 1, ..ping },
                Ping { wait: 0, ..ping },
                Ping { wait: PING_WAIT_MAX + 1, ..ping },
            ] {
                assert_eq!(Ping::from_bytes(&refused.to_bytes()), None, "{refused:?}");
            }
            assert!(Ping::from_bytes(&Ping { length: PING_DATA_MAX, wait: PING_WAIT_MAX, ..ping }.to_bytes()).is_some());
        }

        #[test]
        fn info_there_and_back() {
            let mut info = Info { mac: [1, 2, 3, 4, 5, 6], address: [10, 0, 2, 15], prefix: 24, gateway: [10, 0, 2, 2], counters: [0; COUNTERS] };
            for (n, counter) in info.counters.iter_mut().enumerate() {
                *counter = 1000 + n as u64;
            }
            let bytes = info.to_bytes();
            assert_eq!(Info::from_bytes(&bytes), Some(info));
            assert_eq!(Info::from_bytes(&bytes[1..]), None);
            assert_eq!(bytes[15 + 8 * NOT_TAKEN..][..2], 1010u16.to_le_bytes());
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_pointer_message_goes_there_and_back() {
        use crate::display::{self, Pointer, ABSOLUTE_MAX};
        for pointer in [
            Pointer { absolute: false, x: -256, y: 255, buttons: 1 },
            Pointer { absolute: false, x: i32::MIN, y: i32::MAX, buttons: 7 },
            Pointer { absolute: true, x: 0, y: ABSOLUTE_MAX, buttons: 0 },
        ] {
            let message = pointer.to_message();
            assert_eq!(message[0], display::POINTER);
            assert_eq!(Pointer::from_message(&message), Some(pointer));
        }
        let good = Pointer { absolute: true, x: 5, y: 6, buttons: 2 }.to_message();
        assert_eq!(Pointer::from_message(&good[..10]), None);
        let mut longer = [0u8; 12];
        longer[..11].copy_from_slice(&good);
        assert_eq!(Pointer::from_message(&longer), None);
        let mut wrong = good;
        wrong[0] = display::TEXT;
        assert_eq!(Pointer::from_message(&wrong), None);
        let mut wrong = good;
        wrong[1] = 2;
        assert_eq!(Pointer::from_message(&wrong), None);
        // Absolute, and off the range: refused. Relative: any number.
        let outside = Pointer { absolute: true, x: ABSOLUTE_MAX + 1, y: 0, buttons: 0 }.to_message();
        assert_eq!(Pointer::from_message(&outside), None);
        let outside = Pointer { absolute: true, x: 0, y: -1, buttons: 0 }.to_message();
        assert_eq!(Pointer::from_message(&outside), None);
    }
    #[test]
    fn a_memory_mapped_device_survives_the_trip_through_bytes() {
        use super::{MmioDeviceInfo, PciDeviceInfo};
        // Every field different, and bigger than a byte where it can
        // be, so that swapped or cut-off ones would show.
        let info = MmioDeviceInfo {
            device: 0x0102_0304,
            version: 0x0506_0708,
            registers: 0x1122_3344_0a00_3e00,
            registers_size: 0x200,
            dma_physical: 0x5566_7788_99aa_b000,
            dma_size: 0x1_0000,
            interrupt: true,
        };
        let bytes = info.to_bytes();
        assert_eq!(MmioDeviceInfo::from_bytes(&bytes), Some(info));
        assert_eq!(info.offset_in_page(), 0xe00);
        // A wrong length is no description at all - and a PCI one is a
        // wrong length, which is how a driver tells the two apart.
        assert_eq!(MmioDeviceInfo::from_bytes(&bytes[..36]), None);
        assert_eq!(MmioDeviceInfo::from_bytes(&[0; PciDeviceInfo::SIZE]), None);
        assert_eq!(PciDeviceInfo::from_bytes(&bytes), None);
        let off = MmioDeviceInfo { interrupt: false, ..info };
        assert_eq!(MmioDeviceInfo::from_bytes(&off.to_bytes()), Some(off));
    }

    #[test]
    fn system_info_survives_the_trip_through_words() {
        // Every field different, so that two swapped ones would show.
        use super::SystemInfo;
        let info = SystemInfo {
            ticks: 1,
            ticks_per_second: 2,
            frames_total: 3,
            frames_free: 4,
            heap_used: 5,
            heap_total: 6,
            processes: 7,
            threads: 8,
            channels: 9,
            cpus: 10,
            busy_ticks: 11,
        };
        let words = info.to_words();
        assert_eq!(words, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
        assert_eq!(SystemInfo::from_words(words), info);
    }

    use super::init::{Run, BUILT_IN, IMAGE, REPLY};

    #[test]
    fn arguments_round_trip() {
        let mut out = [0u8; 32];
        // The crate is no_std; its tests run on the host, where std
        // exists but has to be asked for by name.
        extern crate std;
        use std::vec::Vec;
        let decode = |bytes: &[u8]| -> Vec<Vec<u8>> { super::arguments::decode(bytes).map(<[u8]>::to_vec).collect() };
        let words = ["notes", "two  words", ""];
        let length = super::arguments::encode(words.into_iter(), &mut out).unwrap();
        assert_eq!(&out[..length], b"notes\0two  words\0\0");
        assert_eq!(decode(&out[..length]), [&b"notes"[..], b"two  words", b""]);
        // None, and one that is empty, are not the same.
        assert_eq!(super::arguments::encode([].into_iter(), &mut out), Some(0));
        assert!(decode(b"").is_empty());
        assert_eq!(super::arguments::encode([""].into_iter(), &mut out), Some(1));
        assert_eq!(decode(b"\0"), [b""]);
        // What does not fit, or cannot be told apart afterwards.
        assert_eq!(super::arguments::encode(["abc"].into_iter(), &mut [0u8; 3]), None);
        assert_eq!(super::arguments::encode(["abc"].into_iter(), &mut [0u8; 4]), Some(4));
        assert_eq!(super::arguments::encode(["a\0b"].into_iter(), &mut out), None);
        // A message cut short loses its last, unfinished argument.
        assert_eq!(decode(b"one\0tw"), [b"one"]);
    }

    #[test]
    fn a_run_request_round_trips() {
        let run = Run { kind: IMAGE, flags: 5, name: "cat", directory: "", file: "/cat", arguments: b"notes welcome" };
        let mut message = [0u8; 64];
        let length = run.encode(&mut message).unwrap();
        assert_eq!(length, 3 + 3 + 1 + 1 + 4 + 13);
        let back = Run::decode(&message[..length]).unwrap();
        assert_eq!((back.kind, back.flags, back.name, back.arguments), (IMAGE, 5, "cat", &b"notes welcome"[..]));
        assert_eq!((back.directory, back.file), ("", "/cat"));
        // No arguments is not the same as no request.
        let length = Run { kind: BUILT_IN, flags: 0, name: "hello", directory: "", file: "", arguments: b"" }
            .encode(&mut message)
            .unwrap();
        let back = Run::decode(&message[..length]).unwrap();
        assert_eq!((back.name, back.file, back.arguments.len()), ("hello", "", 0));
    }

    #[test]
    fn a_run_request_may_carry_an_environment() {
        use super::init::{pack, Run, ENVIRONMENT, IMAGE, REPLY};
        let mut packed = [0u8; 64];
        let length = pack(b"x.c\0-o\0x\0", b"CC=tcc\0MAKEFLAGS=-s\0", &mut packed).unwrap();
        let run = Run { kind: IMAGE | REPLY | ENVIRONMENT, flags: 0, name: "make", directory: "", file: "/make", arguments: &packed[..length] };
        let mut message = [0u8; 128];
        let n = run.encode(&mut message).unwrap();
        let back = Run::decode(&message[..n]).unwrap();
        assert_eq!(back.what(), IMAGE);
        assert!(back.replies_apart());
        assert_eq!(back.parts(), (&b"x.c\0-o\0x\0"[..], &b"CC=tcc\0MAKEFLAGS=-s\0"[..]));
        // Without ENVIRONMENT, all of it is arguments, as before.
        let plain = Run { kind: IMAGE, flags: 0, name: "ls", directory: "", file: "/ls", arguments: b"a\0" };
        assert_eq!(plain.parts(), (&b"a\0"[..], &b""[..]));
        // A length that says more than there is: not a request.
        let bad = Run { kind: IMAGE | ENVIRONMENT, flags: 0, name: "ls", directory: "", file: "/ls", arguments: b"\x09\0ab" };
        let n = bad.encode(&mut message).unwrap();
        assert!(Run::decode(&message[..n]).is_none());
        assert!(pack(&[0; 40], b"", &mut [0u8; 16]).is_none(), "too big for the message");
    }

    #[test]
    fn a_run_request_may_ask_for_its_reply_apart() {
        let run = Run { kind: IMAGE | REPLY, flags: 0, name: "ld.lld", directory: "", file: "/ld.lld", arguments: b"" };
        let mut message = [0u8; 64];
        let length = run.encode(&mut message).unwrap();
        let back = Run::decode(&message[..length]).unwrap();
        assert_eq!((back.what(), back.replies_apart(), back.name), (IMAGE, true, "ld.lld"));
        let run = Run { kind: BUILT_IN, flags: 0, name: "hello", directory: "", file: "", arguments: b"" };
        let length = run.encode(&mut message).unwrap();
        let back = Run::decode(&message[..length]).unwrap();
        assert_eq!((back.what(), back.replies_apart()), (BUILT_IN, false));
        // REPLY alone is no kind.
        assert!(Run::decode(b"\x80\x00\x03cat\x00\x00").is_none());
    }

    #[test]
    fn a_run_request_carries_the_directory() {
        let run = Run { kind: IMAGE, flags: 5, name: "cat", directory: "docs/notes", file: "/cat", arguments: b"today" };
        let mut message = [0u8; 64];
        let length = run.encode(&mut message).unwrap();
        assert_eq!(length, 3 + 3 + 1 + 10 + 1 + 4 + 5);
        let back = Run::decode(&message[..length]).unwrap();
        assert_eq!((back.name, back.directory, back.file, back.arguments), ("cat", "docs/notes", "/cat", &b"today"[..]));
        // A directory and no arguments: the two are not confused.
        let run = Run { kind: IMAGE, flags: 0, name: "ls", directory: "docs", file: "/ls", arguments: b"" };
        let length = run.encode(&mut message).unwrap();
        let back = Run::decode(&message[..length]).unwrap();
        assert_eq!((back.directory, back.file, back.arguments.len()), ("docs", "/ls", 0));
        // Longer than its length byte can say.
        let long = "d".repeat(256);
        let run = Run { kind: IMAGE, flags: 0, name: "ls", directory: &long, file: "", arguments: b"" };
        assert_eq!(run.encode(&mut [0u8; 512]), None);
        let run = Run { kind: IMAGE, flags: 0, name: "ls", directory: "", file: &long, arguments: b"" };
        assert_eq!(run.encode(&mut [0u8; 512]), None);
        // The file is where a path starting "tools/" leads, not a name
        // to be taken apart again: kept whole, as long as it fits.
        let run = Run { kind: IMAGE, flags: 0, name: "tools/given", directory: "docs", file: "/docs/tools/given", arguments: b"a\0" };
        let length = run.encode(&mut message).unwrap();
        let back = Run::decode(&message[..length]).unwrap();
        assert_eq!((back.name, back.file, back.arguments), ("tools/given", "/docs/tools/given", &b"a\0"[..]));
    }

    #[test]
    fn a_run_request_that_does_not_fit_is_refused() {
        let run = Run { kind: IMAGE, flags: 0, name: "cat", directory: "", file: "", arguments: b"notes" };
        assert_eq!(run.encode(&mut [0u8; 12]), None);
        assert_eq!(run.encode(&mut [0u8; 13]), Some(13));
        let long = "n".repeat(256);
        let run = Run { kind: IMAGE, flags: 0, name: &long, directory: "", file: "", arguments: b"" };
        assert_eq!(run.encode(&mut [0u8; 512]), None);
        // A request cut off before its file is no request.
        let mut message = [0u8; 64];
        let run = Run { kind: IMAGE, flags: 0, name: "cat", directory: "d", file: "/d/cat", arguments: b"" };
        let length = run.encode(&mut message).unwrap();
        assert!(Run::decode(&message[..length - 1]).is_none());
        assert!(Run::decode(&message[..2 + 4 + 2]).is_none());
    }

    #[test]
    fn bad_run_requests_decode_to_nothing() {
        // Whatever arrives on the channel: never a panic, never a
        // request with a name that is not there.
        for bad in [
            &b""[..],
            b"\x01\x00",                // cut short
            b"\x01\x00\x00",            // no name
            b"\x01\x00\x05cat",         // the name claims more than there is
            b"\x01\x00\x03cat",         // no directory length (the request before directories)
            b"\x01\x00\x03cat\x04doc",  // the directory claims more than there is
            b"\x01\x00\x03cat\x02\xff\xfe", // a directory that is not text
            b"\x01\x00\x03cat\x00",     // no file length (the request before files)
            b"\x01\x00\x03cat\x00\x04/ca", // the file claims more than there is
            b"\x01\x00\x03cat\x00\x02\xff\xfe", // a file that is not text
            b"\x03\x00\x03cat",         // no such kind
            b"\x01\x00\x02\xff\xfe",    // a name that is not text
            b"run hello",               // the old, text request
        ] {
            assert!(Run::decode(bad).is_none(), "{bad:?}");
        }
        assert!(Run::decode(b"\x01\x00\x03cat\x00\x00").is_some());
    }
}
