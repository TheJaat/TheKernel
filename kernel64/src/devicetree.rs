//! The device tree: how a machine without ACPI describes itself.
//!
//! On a PC the firmware leaves ACPI tables in memory, and the kernel
//! reads from them how many CPUs there are and where the interrupt
//! controllers sit. On most AArch64 and RISC-V machines the loader
//! instead hands the kernel one block of bytes, the "flattened device
//! tree" (often "DTB", for device tree blob): a tree of named nodes -
//! `/memory`, `/cpus/cpu@0`, a serial port - each with properties.
//!
//! This is the beginning of a reader for it: the header only. It is
//! enough to tell a real device tree from a stray pointer, and to know
//! how many bytes it is. Walking the nodes comes next.
//!
//! Every number in a device tree is big-endian - most significant byte
//! first - whatever the CPU is. Hence `from_be_bytes` throughout.

/// The first four bytes of every device tree.
pub const MAGIC: u32 = 0xD00D_FEED;

/// The header is ten 32-bit numbers.
pub const HEADER_SIZE: usize = 40;

/// The oldest layout this reader understands. Version 17 is what every
/// tool has written since 2005; `last_compatible` says how old a reader
/// may be and still understand the blob.
pub const VERSION: u32 = 17;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// The whole blob, header included.
    pub total_size: u32,
    /// Where the tree itself is, from the start of the blob.
    pub structure_offset: u32,
    pub structure_size: u32,
    /// Where the property names are kept.
    pub strings_offset: u32,
    pub strings_size: u32,
    /// Where the list of memory the kernel must leave alone is.
    pub reserved_offset: u32,
    pub version: u32,
    pub last_compatible: u32,
    /// The number of the CPU the loader started the kernel on.
    pub boot_cpu: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The first four bytes are not `MAGIC`: this is not a device tree.
    NotADeviceTree { found: u32 },
    /// Written for a reader newer than this one.
    TooNew { last_compatible: u32 },
    /// A part lies outside the blob, or the blob is smaller than its
    /// own header.
    Inconsistent,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::NotADeviceTree { found } => {
                write!(f, "not a device tree: it begins {found:#010x}, not {MAGIC:#010x}")
            }
            Error::TooNew { last_compatible } => {
                write!(f, "a device tree for readers of version {last_compatible} or later; this one is {VERSION}")
            }
            Error::Inconsistent => write!(f, "a device tree whose parts lie outside it"),
        }
    }
}

impl Header {
    /// Reads and checks a header. Nothing beyond these forty bytes is
    /// looked at; the checks are only that the numbers agree with each
    /// other.
    pub fn parse(bytes: &[u8; HEADER_SIZE]) -> Result<Header, Error> {
        // The `n`th 32-bit number of the header.
        let word = |n: usize| u32::from_be_bytes([bytes[4 * n], bytes[4 * n + 1], bytes[4 * n + 2], bytes[4 * n + 3]]);

        if word(0) != MAGIC {
            return Err(Error::NotADeviceTree { found: word(0) });
        }
        let header = Header {
            total_size: word(1),
            structure_offset: word(2),
            strings_offset: word(3),
            reserved_offset: word(4),
            version: word(5),
            last_compatible: word(6),
            boot_cpu: word(7),
            strings_size: word(8),
            structure_size: word(9),
        };
        if header.last_compatible > VERSION {
            return Err(Error::TooNew { last_compatible: header.last_compatible });
        }
        // u64, so that an offset plus a size near 4 GiB cannot wrap
        // round and look small.
        let total = u64::from(header.total_size);
        let inside = |offset: u32, size: u32| {
            u64::from(offset) >= HEADER_SIZE as u64 && u64::from(offset) + u64::from(size) <= total
        };
        if !inside(header.structure_offset, header.structure_size)
            || !inside(header.strings_offset, header.strings_size)
            || !inside(header.reserved_offset, 16)
        {
            return Err(Error::Inconsistent);
        }
        Ok(header)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A header as a loader would write it: ten big-endian numbers.
    fn header(words: [u32; 10]) -> [u8; HEADER_SIZE] {
        let mut bytes = [0u8; HEADER_SIZE];
        for (n, word) in words.iter().enumerate() {
            bytes[4 * n..4 * n + 4].copy_from_slice(&word.to_be_bytes());
        }
        bytes
    }

    /// What QEMU's virt machine really hands over (sizes rounded).
    const GOOD: [u32; 10] = [MAGIC, 0x10_0000, 0x38, 0x1C00, 0x28, 17, 16, 0, 0x300, 0x1BC8];

    #[test]
    fn a_good_header_is_read() {
        let parsed = Header::parse(&header(GOOD)).expect("a good header");
        assert_eq!(parsed.total_size, 0x10_0000);
        assert_eq!(parsed.structure_offset, 0x38);
        assert_eq!(parsed.structure_size, 0x1BC8);
        assert_eq!(parsed.strings_offset, 0x1C00);
        assert_eq!(parsed.strings_size, 0x300);
        assert_eq!(parsed.reserved_offset, 0x28);
        assert_eq!(parsed.version, 17);
        assert_eq!(parsed.last_compatible, 16);
        assert_eq!(parsed.boot_cpu, 0);
    }

    #[test]
    fn the_magic_is_big_endian() {
        let bytes = header(GOOD);
        assert_eq!(&bytes[..4], &[0xD0, 0x0D, 0xFE, 0xED]);
        // The same four bytes read the little-endian way are refused.
        let mut swapped = bytes;
        swapped[..4].reverse();
        assert_eq!(Header::parse(&swapped), Err(Error::NotADeviceTree { found: 0xEDFE_0DD0 }));
    }

    #[test]
    fn zeroes_are_not_a_device_tree() {
        assert_eq!(Header::parse(&[0; HEADER_SIZE]), Err(Error::NotADeviceTree { found: 0 }));
    }

    #[test]
    fn a_tree_for_a_newer_reader_is_refused() {
        let mut words = GOOD;
        words[5] = 19;
        words[6] = 18;
        assert_eq!(Header::parse(&header(words)), Err(Error::TooNew { last_compatible: 18 }));
        // Newer, but says a version 17 reader will do: accepted.
        words[6] = 17;
        assert!(Header::parse(&header(words)).is_ok());
    }

    #[test]
    fn parts_outside_the_blob_are_refused() {
        // The blob ends where the strings begin: they lie outside it.
        let mut words = GOOD;
        words[1] = 0x1C00;
        assert_eq!(Header::parse(&header(words)), Err(Error::Inconsistent));
        // The strings begin inside the header.
        let mut words = GOOD;
        words[3] = 8;
        assert_eq!(Header::parse(&header(words)), Err(Error::Inconsistent));
        // An offset and size that would wrap round in 32 bits.
        let mut words = GOOD;
        words[2] = 0xFFFF_FFF0;
        words[9] = 0x20;
        assert_eq!(Header::parse(&header(words)), Err(Error::Inconsistent));
        // A blob smaller than its header.
        let mut words = GOOD;
        words[1] = 16;
        assert_eq!(Header::parse(&header(words)), Err(Error::Inconsistent));
    }

    #[test]
    fn errors_say_what_was_found() {
        let text = format!("{}", Error::NotADeviceTree { found: 0x1234 });
        assert_eq!(text, "not a device tree: it begins 0x00001234, not 0xd00dfeed");
    }
}
