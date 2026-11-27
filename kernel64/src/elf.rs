//! Reading user programs: ELF64 executables.
//!
//! A program file is untrusted input, possibly broken, possibly hostile,
//! so every number in it is checked before it is used: offsets stay
//! inside the file, segments stay inside user space and do not overlap,
//! sizes do not overflow. A bad file is an error, never a kernel crash.
//!
//! Two rules go beyond the format:
//!   - W^X: no segment may be both writable and executable. Memory that
//!     can be written and then run is how injected code runs; refusing
//!     it at load time closes that door for every program.
//!   - nothing below 64 KiB: page 0 stays unmapped so null pointers
//!     fault, with a margin for null + small offsets.
//!   - at most MEMORY_MAX in all: a few bytes of file can ask for
//!     gigabytes of zeroed memory, and loading it would take every frame
//!     the machine has before failing.
//!
//! A program may say how big a stack it wants: the size of its
//! PT_GNU_STACK header, which `ld.lld -z stack-size=N` writes. Between
//! STACK_MIN and STACK_MAX, in whole pages; a program that says nothing
//! gets STACK_DEFAULT. The stack is reserved, not taken: a page of it
//! gets memory when first touched (process.rs), so a big one costs
//! only what is used.
//!
//! This only parses and validates; the kernel does the copying and
//! mapping (process.rs). The file is read through `Source`, so that it
//! need not be one slice in memory: a big program is read straight out
//! of the pages of the memory object it came in.

use alloc::vec::Vec;
use core::fmt;
use core::ops::Range;

pub const PAGE: u64 = 4096;
/// The lowest address a segment may use.
pub const LOWEST_ADDRESS: u64 = 0x1_0000;
/// The most memory one program's segments may take, in whole pages: 1
/// GiB. The program's memory limit (account.rs) is checked too, before
/// a page is loaded; this only refuses nonsense early.
pub const MEMORY_MAX: u64 = 1 << 30;
/// The stack sizes, from abi::stack (libc reads them there too).
pub const STACK_MIN: u64 = abi::stack::MIN;
pub const STACK_DEFAULT: u64 = abi::stack::DEFAULT;
pub const STACK_MAX: u64 = abi::stack::MAX;
/// The most program headers a file may have. Real programs have fewer
/// than a dozen; the limit keeps a hostile file from making the kernel
/// read megabytes of them.
pub const HEADERS_MAX: u64 = 128;

/// Where a program file's bytes are read from: a slice, or the pages
/// of a memory object.
pub trait Source {
    /// How many bytes the file has.
    fn file_size(&self) -> u64;
    /// Fills `out` with the bytes from `offset` on. The caller keeps
    /// `offset + out.len()` within `file_size()`.
    fn read(&self, offset: u64, out: &mut [u8]);
}

impl Source for [u8] {
    fn file_size(&self) -> u64 {
        self.len() as u64
    }
    fn read(&self, offset: u64, out: &mut [u8]) {
        out.copy_from_slice(&self[offset as usize..][..out.len()]);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub virtual_range: Range<u64>,
    /// The bytes to copy from the file; the rest of the range is zeroed.
    pub file_range: Range<usize>,
    pub writable: bool,
    pub executable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub entry: u64,
    pub segments: Vec<Segment>,
    /// The stack's size in bytes: a whole number of pages.
    pub stack: u64,
    /// The program headers, as in the file (56 bytes each): the program
    /// may ask for them (PROGRAM_HEADERS), to find its thread-local
    /// storage template (PT_TLS) and to answer dl_iterate_phdr.
    pub headers: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfError {
    TooShort,
    NotElf,
    Not64BitLittleEndian,
    NotExecutable,
    WrongMachine,
    BadProgramHeaders,
    SegmentOutsideFile,
    SegmentOutsideUserSpace,
    SegmentsOverlap,
    WritableAndExecutable,
    NothingToLoad,
    EntryNotExecutable,
    TooBig,
}

impl fmt::Display for ElfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

const PT_LOAD: u32 = 1;
const PT_GNU_STACK: u32 = 0x6474_e551;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
/// The "machine" numbers of the two CPUs the kernel runs on: which
/// instructions the file holds.
pub const EM_X86_64: u16 = 62;
pub const EM_AARCH64: u16 = 183;

/// The machine this kernel was built for. A program for the other one
/// is a perfectly good ELF file whose first instruction would be
/// nonsense here, so the loader refuses it by this number.
#[cfg(target_arch = "aarch64")]
pub const NATIVE_MACHINE: u16 = EM_AARCH64;
#[cfg(not(target_arch = "aarch64"))]
pub const NATIVE_MACHINE: u16 = EM_X86_64;
const ET_EXEC: u16 = 2;

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// Checks `file` and describes what to load where. Segments must end at
/// or below `user_end`. The program must be for this kernel's CPU.
pub fn parse(file: &(impl Source + ?Sized), user_end: u64) -> Result<Image, ElfError> {
    parse_for(file, user_end, NATIVE_MACHINE)
}

/// `parse`, for a program that must be for `machine`.
pub fn parse_for(source: &(impl Source + ?Sized), user_end: u64, machine: u16) -> Result<Image, ElfError> {
    let length = source.file_size();
    if length < 64 {
        return Err(ElfError::TooShort);
    }
    // Only the headers are read here, each once, into the kernel's own
    // memory: what is checked is then what is used.
    let mut file = [0u8; 64];
    source.read(0, &mut file);
    if &file[0..4] != b"\x7FELF" {
        return Err(ElfError::NotElf);
    }
    if file[4] != 2 || file[5] != 1 {
        return Err(ElfError::Not64BitLittleEndian);
    }
    if u16_at(&file, 16) != ET_EXEC {
        return Err(ElfError::NotExecutable);
    }
    if u16_at(&file, 18) != machine {
        return Err(ElfError::WrongMachine);
    }
    let entry = u64_at(&file, 24);
    let header_offset = u64_at(&file, 32);
    let header_size = u16_at(&file, 54) as u64;
    let header_count = u16_at(&file, 56) as u64;
    if header_size != 56
        || header_count > HEADERS_MAX
        || header_offset.checked_add(header_size * header_count).is_none_or(|end| end > length)
    {
        return Err(ElfError::BadProgramHeaders);
    }
    let mut headers = alloc::vec![0u8; (header_size * header_count) as usize];
    source.read(header_offset, &mut headers);

    let mut segments: Vec<Segment> = Vec::new();
    let mut memory = 0;
    let mut stack = STACK_DEFAULT;
    for h in headers.as_chunks::<56>().0 {
        if u32_at(h, 0) == PT_GNU_STACK {
            // Its size, if it has one, is the stack the program wants.
            stack = abi::stack::size(u64_at(h, 40)).ok_or(ElfError::TooBig)?;
            continue;
        }
        if u32_at(h, 0) != PT_LOAD {
            continue;
        }
        let flags = u32_at(h, 4);
        let offset = u64_at(h, 8);
        let address = u64_at(h, 16);
        let file_size = u64_at(h, 32);
        let memory_size = u64_at(h, 40);
        if memory_size == 0 {
            continue;
        }
        if file_size > memory_size
            || offset.checked_add(file_size).is_none_or(|end| end > length)
        {
            return Err(ElfError::SegmentOutsideFile);
        }
        let end = address.checked_add(memory_size).ok_or(ElfError::SegmentOutsideUserSpace)?;
        if address < LOWEST_ADDRESS || end > user_end {
            return Err(ElfError::SegmentOutsideUserSpace);
        }
        let (writable, executable) = (flags & PF_W != 0, flags & PF_X != 0);
        if writable && executable {
            return Err(ElfError::WritableAndExecutable);
        }
        // Permissions are per page, so two segments sharing a page would
        // need two permissions at once. Compare whole pages.
        let pages = address & !(PAGE - 1)..(end + PAGE - 1) & !(PAGE - 1);
        for other in &segments {
            let other_pages =
                other.virtual_range.start & !(PAGE - 1)..(other.virtual_range.end + PAGE - 1) & !(PAGE - 1);
            if pages.start < other_pages.end && other_pages.start < pages.end {
                return Err(ElfError::SegmentsOverlap);
            }
        }
        memory += pages.end - pages.start;
        if memory > MEMORY_MAX {
            return Err(ElfError::TooBig);
        }
        segments.push(Segment {
            virtual_range: address..end,
            file_range: offset as usize..(offset + file_size) as usize,
            writable,
            executable,
        });
    }

    if segments.is_empty() {
        return Err(ElfError::NothingToLoad);
    }
    if !segments.iter().any(|s| s.executable && s.virtual_range.contains(&entry)) {
        return Err(ElfError::EntryNotExecutable);
    }
    Ok(Image { entry, segments, stack, headers })
}

#[cfg(test)]
mod tests {
    use super::*;

    const END: u64 = 0x0000_8000_0000_0000;

    /// (flags, file offset, address, file size, memory size)
    type Header = (u32, u64, u64, u64, u64);

    /// A minimal executable: the ELF header, program headers, and
    /// `body` bytes of contents starting at offset 0x1000.
    fn elf(entry: u64, headers: &[Header], body: usize) -> Vec<u8> {
        let mut f = vec![0u8; 0x1000 + body];
        f[0..4].copy_from_slice(b"\x7FELF");
        f[4] = 2; // 64-bit
        f[5] = 1; // little-endian
        f[6] = 1;
        f[16..18].copy_from_slice(&ET_EXEC.to_le_bytes());
        f[18..20].copy_from_slice(&NATIVE_MACHINE.to_le_bytes());
        f[24..32].copy_from_slice(&entry.to_le_bytes());
        f[32..40].copy_from_slice(&64u64.to_le_bytes());
        f[54..56].copy_from_slice(&56u16.to_le_bytes());
        f[56..58].copy_from_slice(&(headers.len() as u16).to_le_bytes());
        for (n, &(flags, offset, address, file_size, memory_size)) in headers.iter().enumerate() {
            let h = &mut f[64 + 56 * n..][..56];
            h[0..4].copy_from_slice(&PT_LOAD.to_le_bytes());
            h[4..8].copy_from_slice(&flags.to_le_bytes());
            h[8..16].copy_from_slice(&offset.to_le_bytes());
            h[16..24].copy_from_slice(&address.to_le_bytes());
            h[32..40].copy_from_slice(&file_size.to_le_bytes());
            h[40..48].copy_from_slice(&memory_size.to_le_bytes());
        }
        f
    }

    const RX: u32 = 5;
    const R: u32 = 4;
    const RW: u32 = 6;

    fn typical() -> Vec<u8> {
        elf(
            0x40_0010,
            &[
                (RX, 0x1000, 0x40_0000, 0x100, 0x100),
                (R, 0x1100, 0x40_1000, 0x80, 0x80),
                (RW, 0x1180, 0x40_2000, 0x40, 0x2000), // .data + .bss
            ],
            0x200,
        )
    }

    #[test]
    fn accepts_a_typical_program() {
        let image = parse(&*typical(), END).unwrap();
        assert_eq!(image.entry, 0x40_0010);
        assert_eq!(image.segments.len(), 3);
        let data = &image.segments[2];
        assert_eq!(data.virtual_range, 0x40_2000..0x40_4000);
        assert_eq!(data.file_range, 0x1180..0x11C0);
        assert!(data.writable && !data.executable);
    }

    #[test]
    fn rejects_damaged_headers() {
        assert_eq!(parse(&[0u8; 10][..], END), Err(ElfError::TooShort));
        let mut f = typical();
        f[0] = 0;
        assert_eq!(parse(&*f, END), Err(ElfError::NotElf));
        let mut f = typical();
        f[4] = 1;
        assert_eq!(parse(&*f, END), Err(ElfError::Not64BitLittleEndian));
        let mut f = typical();
        f[18] = 0xB7; // AArch64
        assert_eq!(parse(&*f, END), Err(ElfError::WrongMachine));
        let mut f = typical();
        f[56..58].copy_from_slice(&0xFFFFu16.to_le_bytes()); // headers past the end
        assert_eq!(parse(&*f, END), Err(ElfError::BadProgramHeaders));
    }

    #[test]
    fn rejects_segments_pointing_outside_the_file() {
        let f = elf(0x40_0000, &[(RX, 0x1000, 0x40_0000, 0x10_0000, 0x10_0000)], 0x100);
        assert_eq!(parse(&*f, END), Err(ElfError::SegmentOutsideFile));
        let f = elf(0x40_0000, &[(RX, u64::MAX, 0x40_0000, 2, 2)], 0x100);
        assert_eq!(parse(&*f, END), Err(ElfError::SegmentOutsideFile));
    }

    #[test]
    fn keeps_programs_in_user_space_and_off_the_null_page() {
        for (address, size) in [(0, 0x100), (0x1000, 0x100), (0xFFFF_FFFF_8000_0000, 0x100), (END - 0x10, 0x100)]
        {
            let f = elf(address, &[(RX, 0x1000, address, 0x10, size)], 0x100);
            assert_eq!(parse(&*f, END), Err(ElfError::SegmentOutsideUserSpace), "{address:#x}");
        }
        let f = elf(0x40_0000, &[(RX, 0x1000, u64::MAX - 0x10, 0x10, 0x100)], 0x100);
        assert_eq!(parse(&*f, END), Err(ElfError::SegmentOutsideUserSpace), "wraps around");
    }

    #[test]
    fn enforces_w_xor_x() {
        let f = elf(0x40_0000, &[(7, 0x1000, 0x40_0000, 0x10, 0x10)], 0x100);
        assert_eq!(parse(&*f, END), Err(ElfError::WritableAndExecutable));
    }

    #[test]
    fn rejects_segments_sharing_a_page() {
        // Code ends at 0x400100; data starts in the same page.
        let f = elf(
            0x40_0000,
            &[(RX, 0x1000, 0x40_0000, 0x100, 0x100), (RW, 0x1100, 0x40_0800, 0x10, 0x10)],
            0x200,
        );
        assert_eq!(parse(&*f, END), Err(ElfError::SegmentsOverlap));
    }

    #[test]
    fn limits_the_memory_a_program_may_ask_for() {
        // 16 bytes of file, MEMORY_MAX and a page of zeros: refused.
        let f = elf(0x40_0000, &[(RX, 0x1000, 0x40_0000, 0x10, 0x10), (RW, 0x1010, 0x80_0000, 0x10, MEMORY_MAX)], 0x100);
        assert_eq!(parse(&*f, END), Err(ElfError::TooBig));
        // The limit is on the total, so many medium segments do not get around it.
        let many: Vec<Header> = (0..5).map(|n| (RW, 0x1000, 0x1000_0000 * (n + 1), 0x10, MEMORY_MAX / 4)).collect();
        assert_eq!(parse(&*elf(0x1000_0000, &many, 0x100), END), Err(ElfError::TooBig));
        let f = elf(0x40_0000, &[(RX, 0x1000, 0x40_0000, 0x10, 0x10), (RW, 0x1010, 0x80_0000, 0x10, MEMORY_MAX - 0x1000)], 0x100);
        assert!(parse(&*f, END).is_ok(), "exactly the limit is fine");
    }

    #[test]
    #[cfg_attr(miri, ignore = "no unsafe in this module: Miri checks nothing the test does not (K187)")]
    fn no_damaged_file_panics() {
        // Every truncation of a good file, and every single byte of its
        // headers flipped: an error or a (still valid) image, never a
        // panic - this runs in the kernel, on files from the disk.
        let good = typical();
        for length in 0..good.len() {
            let _ = parse(&good[..length], END);
        }
        for position in 0..64 + 3 * 56 {
            for flip in [0x01, 0x80, 0xFF] {
                let mut f = good.clone();
                f[position] ^= flip;
                if let Ok(image) = parse(&*f, END) {
                    for s in &image.segments {
                        assert!(s.file_range.end <= f.len() && s.file_range.len() as u64 <= s.virtual_range.end - s.virtual_range.start);
                        assert!(s.virtual_range.start >= LOWEST_ADDRESS && s.virtual_range.end <= END);
                    }
                }
            }
        }
    }

    /// `typical()` with a PT_GNU_STACK header asking for `size` bytes.
    fn asking_for_a_stack(size: u64) -> Vec<u8> {
        let mut f = elf(
            0x40_0010,
            &[(RX, 0x1000, 0x40_0000, 0x100, 0x100), (RW, 0, 0, 0, size)],
            0x200,
        );
        f[64 + 56..][..4].copy_from_slice(&PT_GNU_STACK.to_le_bytes());
        f
    }

    #[test]
    fn a_program_gets_the_stack_it_asks_for() {
        assert_eq!(parse(&*typical(), END).unwrap().stack, STACK_DEFAULT, "not asked: the default");
        assert_eq!(parse(&*asking_for_a_stack(0), END).unwrap().stack, STACK_DEFAULT, "a header without a size");
        assert_eq!(parse(&*asking_for_a_stack(64 << 20), END).unwrap().stack, 64 << 20, "more than the default");
        assert_eq!(parse(&*asking_for_a_stack(1 << 20), END).unwrap().stack, 1 << 20);
        assert_eq!(parse(&*asking_for_a_stack(1000), END).unwrap().stack, STACK_MIN, "never less than the least");
        assert_eq!(parse(&*asking_for_a_stack(STACK_MIN + 1), END).unwrap().stack, STACK_MIN + PAGE, "whole pages");
        assert_eq!(parse(&*asking_for_a_stack(STACK_MAX), END).unwrap().stack, STACK_MAX);
        assert_eq!(parse(&*asking_for_a_stack(STACK_MAX + 1), END), Err(ElfError::TooBig));
    }

    #[test]
    fn limits_how_many_headers_are_read() {
        let headers: Vec<Header> = (0..HEADERS_MAX + 1).map(|_| (R, 0, 0, 0, 0)).collect();
        let mut f = elf(0x40_0000, &headers, 0x2000);
        assert_eq!(parse(&*f, END), Err(ElfError::BadProgramHeaders));
        f[56..58].copy_from_slice(&(HEADERS_MAX as u16).to_le_bytes());
        assert_eq!(parse(&*f, END), Err(ElfError::NothingToLoad), "the limit itself is read");
    }

    #[test]
    fn the_entry_point_must_be_executable() {
        let f = elf(0x40_2000, &[(RX, 0x1000, 0x40_0000, 0x100, 0x100), (RW, 0x1100, 0x40_2000, 0x10, 0x10)], 0x200);
        assert_eq!(parse(&*f, END), Err(ElfError::EntryNotExecutable));
        let f = elf(0x40_0000, &[], 0);
        assert_eq!(parse(&*f, END), Err(ElfError::NothingToLoad));
    }

    #[test]
    fn a_program_is_only_for_the_machine_asked_for() {
        // The same file, said to be for each CPU in turn: each kernel
        // takes its own and refuses the other's.
        for (written, asked, good) in [
            (EM_X86_64, EM_X86_64, true),
            (EM_AARCH64, EM_AARCH64, true),
            (EM_X86_64, EM_AARCH64, false),
            (EM_AARCH64, EM_X86_64, false),
            (3, EM_X86_64, false), // a 32-bit PC
        ] {
            let mut f = typical();
            f[18..20].copy_from_slice(&written.to_le_bytes());
            let result = parse_for(&*f, END, asked);
            if good {
                assert!(result.is_ok(), "{written} for {asked}");
            } else {
                assert_eq!(result, Err(ElfError::WrongMachine), "{written} for {asked}");
            }
        }
        assert_eq!(EM_AARCH64, 0xB7);
    }
}
