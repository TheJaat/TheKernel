//! The kernel's own symbols (TheTaaJ entry 122): the table the build
//! fills in after linking (`kernel/kernel64/scripts/fill-symbols.py`),
//! read by the kernel for its backtraces and - a copy, handed out with
//! tracing's rings - by the profiler (`profile`, K140).
//!
//! ```text
//!  MAGIC (8 bytes) | count u32 | names u32 | count x [address u64, name at u32, name length u32] | names
//! ```
//!
//! The entries are sorted by address; a name is UTF-8, without generic
//! arguments.

/// The block's first eight bytes, which the fill script looks for.
pub const MAGIC: [u8; 8] = *b"TAAJSYM1";
/// How big the block is. The x86_64 kernel's functions take some 30 KiB.
pub const BLOCK_SIZE: usize = 64 * 1024;
const HEADER: usize = 16;
const ENTRY: usize = 16;

/// A filled-in table, read where it lies.
pub struct Table<'a> {
    bytes: &'a [u8],
    count: usize,
    names: usize,
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

impl<'a> Table<'a> {
    /// The table in `bytes`, or None if it was never filled in (a count of
    /// 0) or does not hold together.
    pub fn parse(bytes: &'a [u8]) -> Option<Table<'a>> {
        if bytes.get(..8)? != MAGIC {
            return None;
        }
        let count = u32_at(bytes, 8)? as usize;
        let names = u32_at(bytes, 12)? as usize;
        let end = HEADER.checked_add(count.checked_mul(ENTRY)?)?.checked_add(names)?;
        (count > 0 && end <= bytes.len()).then_some(Table { bytes, count, names })
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    fn address(&self, index: usize) -> u64 {
        u64_at(self.bytes, HEADER + index * ENTRY).unwrap_or(0)
    }

    fn name(&self, index: usize) -> &'a str {
        let entry = HEADER + index * ENTRY;
        let names = HEADER + self.count * ENTRY;
        let (Some(at), Some(length)) = (u32_at(self.bytes, entry + 8), u32_at(self.bytes, entry + 12)) else {
            return "?";
        };
        let (at, length) = (at as usize, length as usize);
        if at.saturating_add(length) > self.names {
            return "?";
        }
        core::str::from_utf8(&self.bytes[names + at..names + at + length]).unwrap_or("?")
    }

    /// The function `address` is in, and how far into it: the entry with
    /// the highest address not above it. None below the first function.
    pub fn lookup(&self, address: u64) -> Option<(&'a str, u64)> {
        // The first entry above `address`; the one before it holds it.
        let (mut low, mut high) = (0, self.count);
        while low < high {
            let middle = low + (high - low) / 2;
            if self.address(middle) <= address {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let index = low.checked_sub(1)?;
        Some((self.name(index), address - self.address(index)))
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    /// A table as fill-symbols.py writes it.
    fn table(functions: &[(u64, &str)]) -> Vec<u8> {
        let mut bytes = MAGIC.to_vec();
        let names: Vec<u8> = functions.iter().flat_map(|(_, n)| n.bytes()).collect();
        bytes.extend((functions.len() as u32).to_le_bytes());
        bytes.extend((names.len() as u32).to_le_bytes());
        let mut at = 0u32;
        for (address, name) in functions {
            bytes.extend(address.to_le_bytes());
            bytes.extend(at.to_le_bytes());
            bytes.extend((name.len() as u32).to_le_bytes());
            at += name.len() as u32;
        }
        bytes.extend(names);
        bytes.resize(BLOCK_SIZE, 0);
        bytes
    }

    #[test]
    fn an_address_is_found_in_the_function_below_it() {
        let bytes = table(&[(0x1000, "start"), (0x1040, "kernel_main"), (0x2000, "panic")]);
        let t = Table::parse(&bytes).unwrap();
        assert_eq!(t.len(), 3);
        assert_eq!(t.lookup(0x1000), Some(("start", 0)));
        assert_eq!(t.lookup(0x103f), Some(("start", 0x3f)));
        assert_eq!(t.lookup(0x1040), Some(("kernel_main", 0)));
        assert_eq!(t.lookup(0x1fff), Some(("kernel_main", 0xfbf)));
        assert_eq!(t.lookup(0x9000), Some(("panic", 0x7000)));
        assert_eq!(t.lookup(0xfff), None);
    }

    #[test]
    fn an_empty_or_broken_table_is_none() {
        let mut empty = MAGIC.to_vec();
        empty.resize(BLOCK_SIZE, 0);
        assert!(Table::parse(&empty).is_none(), "never filled in: count 0");
        assert!(Table::parse(&[0u8; 64]).is_none(), "no magic");
        let mut lying = table(&[(0x1000, "start")]);
        lying[8..12].copy_from_slice(&100_000u32.to_le_bytes()); // more entries than fit
        assert!(Table::parse(&lying).is_none());
        let mut bad_name = table(&[(0x1000, "start")]);
        bad_name[HEADER + 8..HEADER + 12].copy_from_slice(&1000u32.to_le_bytes()); // a name past the end
        assert_eq!(Table::parse(&bad_name).unwrap().lookup(0x1000), Some(("?", 0)));
    }
}
