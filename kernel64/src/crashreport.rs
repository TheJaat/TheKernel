//! A crash report per program (TheTaaJ K138): when a program breaks a
//! rule - reads memory it was not given, runs data, divides by zero -
//! and has no handler for it, the kernel ends it, as before, and now
//! also writes down what it was doing: its registers, the words on its
//! stack that point into its code, and its memory. The report goes into
//! the system log's ring (K136); the log service adds the last lines
//! the program printed, and keeps it in a file of its own.
//!
//! Programs are not built with frame pointers, so there is no chain of
//! frames to walk. What there is: the top of the stack, where the return
//! addresses of the calls still running lie among other words. A word
//! that points into the program's code is likely one of them - likely,
//! not surely, and the report says which it is: words, at their place
//! on the stack.
//!
//! The formatting only, on what the kernel gathered (process.rs).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;
use core::ops::Range;

/// What the kernel gathered about the program as it broke the rule.
pub struct Report<'a> {
    /// What it did: "page fault at 0x401234, for 0x0 (read: nothing there)".
    pub what: &'a str,
    /// Every register, by its name, in the order to show them.
    pub registers: &'a [(&'a str, u64)],
    /// The stack pointer, and the bytes from there up (as many as could
    /// be read).
    pub stack_pointer: u64,
    pub stack: &'a [u8],
    /// The program's memory: each range, what it allows ("r-x"), and
    /// what it is ("code", "stack", "memory object").
    pub memory: &'a [(Range<u64>, &'a str, &'a str)],
}

/// The most stack words shown.
pub const WORDS_MAX: usize = 12;

/// The words in `stack` (read at the stack pointer) that point into
/// `code`: their offset from the stack pointer, and their value.
pub fn code_words(stack: &[u8], code: &[Range<u64>]) -> Vec<(u64, u64)> {
    stack
        .chunks_exact(8)
        .enumerate()
        .filter_map(|(n, word)| {
            let value = u64::from_le_bytes(word.try_into().ok()?);
            code.iter().any(|range| range.contains(&value)).then_some(((n * 8) as u64, value))
        })
        .take(WORDS_MAX)
        .collect()
}

/// The report as text, a line each, for the system log.
pub fn format(report: &Report) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "broke a rule: {}", report.what);
    out.push_str("registers:\n");
    for row in report.registers.chunks(4) {
        let cells: Vec<String> = row.iter().map(|(name, value)| format!("{name:>6} {value:#018x}")).collect();
        let _ = writeln!(out, " {}", cells.join(" "));
    }
    let code: Vec<Range<u64>> = report.memory.iter().filter(|(_, allows, _)| allows.contains('x')).map(|(range, _, _)| range.clone()).collect();
    let words = code_words(report.stack, &code);
    if words.is_empty() {
        let _ = writeln!(out, "stack at {:#x}: no word in the {} bytes read points into its code", report.stack_pointer, report.stack.len());
    } else {
        let _ = writeln!(out, "stack at {:#x}: words that point into its code (likely return addresses, newest first):", report.stack_pointer);
        for (offset, value) in words {
            let _ = writeln!(out, "   sp+{offset:<#6x} {value:#x}");
        }
    }
    out.push_str("memory:\n");
    for (range, allows, what) in report.memory {
        let _ = writeln!(out, "   {:#014x}-{:#014x} {allows} {what}", range.start, range.end);
    }
    out
}

/// A program's loadable segments, from its ELF program headers (56
/// bytes each, as the kernel keeps them): each range, page-rounded, and
/// what it allows.
pub fn segments(headers: &[u8]) -> Vec<(Range<u64>, &'static str)> {
    const PT_LOAD: u32 = 1;
    let u32_at = |h: &[u8], at: usize| u32::from_le_bytes([h[at], h[at + 1], h[at + 2], h[at + 3]]);
    let u64_at = |h: &[u8], at: usize| {
        let mut b = [0u8; 8];
        b.copy_from_slice(&h[at..at + 8]);
        u64::from_le_bytes(b)
    };
    headers
        .chunks_exact(56)
        .filter(|h| u32_at(h, 0) == PT_LOAD)
        .map(|h| {
            let (flags, start, size) = (u32_at(h, 4), u64_at(h, 16), u64_at(h, 40));
            let end = start.saturating_add(size).saturating_add(4095) & !4095;
            let allows = match (flags & 4 != 0, flags & 2 != 0, flags & 1 != 0) {
                (true, false, true) => "r-x",
                (true, true, false) => "rw-",
                (true, false, false) => "r--",
                (true, true, true) => "rwx",
                _ => "---",
            };
            ((start & !4095)..end, allows)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(kind: u32, flags: u32, start: u64, size: u64) -> [u8; 56] {
        let mut h = [0u8; 56];
        h[0..4].copy_from_slice(&kind.to_le_bytes());
        h[4..8].copy_from_slice(&flags.to_le_bytes());
        h[16..24].copy_from_slice(&start.to_le_bytes());
        h[40..48].copy_from_slice(&size.to_le_bytes());
        h
    }

    #[test]
    fn the_loadable_segments_are_read_from_the_headers() {
        let mut headers = Vec::new();
        headers.extend_from_slice(&header(1, 5, 0x400000, 0x1234)); // code, r-x
        headers.extend_from_slice(&header(6, 4, 0x400040, 0x100)); // PHDR: not loaded
        headers.extend_from_slice(&header(1, 6, 0x402234, 0x10)); // data, rw-
        headers.extend_from_slice(&header(1, 4, 0x403000, 0x1)); // read-only
        headers.extend_from_slice(&header(1, 7, 0x404000, 0x1)); // all three
        headers.extend_from_slice(&header(1, 0, 0x405000, 0x1)); // none
        headers.extend_from_slice(&[0u8; 20]); // a torn end is ignored
        assert_eq!(
            segments(&headers),
            [
                (0x400000..0x402000, "r-x"),
                (0x402000..0x403000, "rw-"),
                (0x403000..0x404000, "r--"),
                (0x404000..0x405000, "rwx"),
                (0x405000..0x406000, "---")
            ]
        );
    }

    #[test]
    fn only_words_into_the_code_are_shown_at_most_twelve() {
        let mut stack = Vec::new();
        for value in [7u64, 0x401000, 0x7fff_0000, 0x401fff, 0x402000] {
            stack.extend_from_slice(&value.to_le_bytes());
        }
        stack.extend_from_slice(&[1, 2, 3]); // a torn last word
        assert_eq!(code_words(&stack, &[0x400000..0x402000]), [(8, 0x401000), (24, 0x401fff)]);
        let many: Vec<u8> = (0..20).flat_map(|_| 0x400010u64.to_le_bytes()).collect();
        assert_eq!(code_words(&many, &[0x400000..0x402000]).len(), WORDS_MAX);
    }

    #[test]
    fn a_report_says_what_registers_stack_and_memory() {
        let mut stack = Vec::new();
        for value in [0u64, 0x401234] {
            stack.extend_from_slice(&value.to_le_bytes());
        }
        let memory = [(0x400000..0x402000, "r-x", "code"), (0x7ff0_0000..0x8000_0000, "rw-", "stack")];
        let registers = [("rip", 0x401000), ("rsp", 0x7fff_fff0), ("rax", 0), ("rbx", 1), ("rcx", 2)];
        let text = format(&Report { what: "page fault at 0x401000, for 0x0", registers: &registers, stack_pointer: 0x7fff_fff0, stack: &stack, memory: &memory });
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "broke a rule: page fault at 0x401000, for 0x0");
        assert_eq!(lines[1], "registers:");
        assert_eq!(lines[2], "    rip 0x0000000000401000    rsp 0x000000007ffffff0    rax 0x0000000000000000    rbx 0x0000000000000001");
        assert_eq!(lines[3], "    rcx 0x0000000000000002");
        assert_eq!(lines[4], "stack at 0x7ffffff0: words that point into its code (likely return addresses, newest first):");
        assert_eq!(lines[5], "   sp+0x8    0x401234");
        assert_eq!(lines[6], "memory:");
        assert_eq!(lines[7], "   0x000000400000-0x000000402000 r-x code");
        // Nothing on the stack points into the code: said so.
        let text = format(&Report { what: "x", registers: &[], stack_pointer: 16, stack: &[0; 16], memory: &memory });
        assert!(text.contains("stack at 0x10: no word in the 16 bytes read points into its code"));
    }
}
