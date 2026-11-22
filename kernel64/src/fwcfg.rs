//! QEMU's firmware configuration device (fw_cfg), and the screen it
//! can switch on (ramfb) - the bytes, with no device in them.
//!
//! The AArch64 `virt` machine has no graphics card unless one is asked
//! for. `-device ramfb` is the simplest screen there is: QEMU shows
//! whatever is in a stretch of the machine's own memory, as pixels, once
//! it is told where that stretch is and how the pixels are laid out.
//! It is told through fw_cfg - a device QEMU gives every machine for
//! passing files between the emulator and the firmware.
//!
//! fw_cfg is a list of named files. Each has a number (its *selector*);
//! file 0x19 is the directory, listing the others by name. The kernel
//! reads the directory, finds "etc/ramfb", and writes the screen's
//! description into that file. Writing goes through fw_cfg's DMA
//! interface: a 16-byte request in memory, whose physical address is
//! written to a register; QEMU carries it out at once and writes 0
//! into its first field when done.
//!
//! Every number fw_cfg reads or writes is big-endian - it was made for
//! firmware on any machine, and chose one order for all.
//!
//! The device's registers are in `main.rs` (they need a mapping); what
//! is here is laid out and read, and tested on the host.

/// The directory's selector.
pub const FILE_DIR: u16 = 0x19;
/// The registers, from the device's base: the data (read a byte at a
/// time), the selector (u16, big-endian) and the DMA address (u64,
/// big-endian; the request runs when its low half is written).
pub const DATA: u64 = 0x00;
pub const SELECTOR: u64 = 0x08;
pub const DMA_ADDRESS: u64 = 0x10;

/// One directory entry: size (u32), selector (u16), two reserved bytes,
/// and a name of up to 56 bytes, ended by a zero.
pub const DIR_ENTRY: usize = 64;
const NAME_MAX: usize = 56;

/// The selector and size of `name` in a directory read whole - its
/// count (u32), then the entries.
pub fn find(directory: &[u8], name: &str) -> Option<(u16, u32)> {
    let count = u32::from_be_bytes(directory.get(0..4)?.try_into().ok()?) as usize;
    let (entries, _) = directory[4..].as_chunks::<DIR_ENTRY>();
    entries.iter().take(count).find_map(|entry| {
        let size = u32::from_be_bytes(entry[0..4].try_into().ok()?);
        let selector = u16::from_be_bytes(entry[4..6].try_into().ok()?);
        let stored = &entry[8..8 + NAME_MAX];
        let length = stored.iter().position(|&byte| byte == 0).unwrap_or(NAME_MAX);
        (&stored[..length] == name.as_bytes()).then_some((selector, size))
    })
}

/// DMA control bits: an error happened; select the file first; write.
pub const DMA_ERROR: u32 = 0x01;
pub const DMA_SELECT: u32 = 0x08;
pub const DMA_WRITE: u32 = 0x10;

/// A DMA request: "select `selector`, then write `length` bytes from
/// physical `address` into it".
pub fn dma_write(selector: u16, length: u32, address: u64) -> [u8; 16] {
    let mut request = [0u8; 16];
    let control = u32::from(selector) << 16 | DMA_SELECT | DMA_WRITE;
    request[0..4].copy_from_slice(&control.to_be_bytes());
    request[4..8].copy_from_slice(&length.to_be_bytes());
    request[8..16].copy_from_slice(&address.to_be_bytes());
    request
}

/// What the request's control field says once QEMU is done with it:
/// Ok when it has become 0, Err if the error bit is set, None while it
/// is still running.
pub fn dma_done(control: u32) -> Option<Result<(), ()>> {
    match control {
        0 => Some(Ok(())),
        _ if control & DMA_ERROR != 0 => Some(Err(())),
        _ => None,
    }
}

/// The pixels the kernel asks of ramfb: 32 bits, 0x00RRGGBB - what
/// Linux's DRM calls XRGB8888, named by four characters ("XR24") as a
/// little-endian number.
pub const XRGB8888: u32 = u32::from_le_bytes(*b"XR24");

/// "etc/ramfb"'s contents: where the pixels are, their format, and the
/// screen's size and the bytes from one row to the next.
pub const RAMFB_CONFIG: usize = 28;

pub fn ramfb_config(address: u64, width: u32, height: u32, stride: u32) -> [u8; RAMFB_CONFIG] {
    let mut config = [0u8; RAMFB_CONFIG];
    config[0..8].copy_from_slice(&address.to_be_bytes());
    config[8..12].copy_from_slice(&XRGB8888.to_be_bytes());
    // 12..16: flags, none
    config[16..20].copy_from_slice(&width.to_be_bytes());
    config[20..24].copy_from_slice(&height.to_be_bytes());
    config[24..28].copy_from_slice(&stride.to_be_bytes());
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(size: u32, selector: u16, name: &str) -> [u8; DIR_ENTRY] {
        let mut entry = [0u8; DIR_ENTRY];
        entry[0..4].copy_from_slice(&size.to_be_bytes());
        entry[4..6].copy_from_slice(&selector.to_be_bytes());
        entry[8..8 + name.len()].copy_from_slice(name.as_bytes());
        entry
    }

    #[test]
    fn a_file_is_found_in_the_directory_by_its_whole_name() {
        let mut directory = 3u32.to_be_bytes().to_vec();
        directory.extend_from_slice(&entry(0x1000, 0x20, "etc/acpi/tables"));
        directory.extend_from_slice(&entry(28, 0x2A, "etc/ramfb"));
        directory.extend_from_slice(&entry(8, 0x2B, "etc/ramfb2"));
        // An entry past the count: not part of the directory.
        directory.extend_from_slice(&entry(1, 0x99, "etc/hidden"));
        assert_eq!(find(&directory, "etc/ramfb"), Some((0x2A, 28)));
        assert_eq!(find(&directory, "etc/ramfb2"), Some((0x2B, 8)));
        assert_eq!(find(&directory, "etc/ram"), None);
        assert_eq!(find(&directory, "etc/hidden"), None);
        assert_eq!(find(&directory, ""), None);
        // A name that fills all 56 bytes, with no zero after it.
        let long = "x".repeat(NAME_MAX);
        let mut directory = 1u32.to_be_bytes().to_vec();
        directory.extend_from_slice(&entry(4, 0x30, &long));
        assert_eq!(find(&directory, &long), Some((0x30, 4)));
        // Cut short, or no count: nothing, and no panic.
        assert_eq!(find(&directory[..40], &long), None);
        assert_eq!(find(&[0, 0], "etc/ramfb"), None);
        assert_eq!(find(&9u32.to_be_bytes(), "etc/ramfb"), None);
    }

    #[test]
    fn a_dma_request_is_big_endian() {
        assert_eq!(
            dma_write(0x2A, 28, 0x4000_1000),
            [0x00, 0x2A, 0x00, 0x18, 0, 0, 0, 28, 0, 0, 0, 0, 0x40, 0x00, 0x10, 0x00]
        );
        assert_eq!(dma_done(0), Some(Ok(())));
        assert_eq!(dma_done(DMA_ERROR), Some(Err(())));
        assert_eq!(dma_done(0x002A_0018), None);
    }

    #[test]
    fn the_screen_is_described_as_ramfb_reads_it() {
        assert_eq!(XRGB8888, 0x3432_5258);
        let config = ramfb_config(0x4800_0000, 1024, 768, 4096);
        assert_eq!(&config[0..8], &[0, 0, 0, 0, 0x48, 0, 0, 0]);
        assert_eq!(&config[8..12], b"42RX"); // "XR24", big-endian
        assert_eq!(&config[12..16], &[0; 4]);
        assert_eq!(&config[16..28], &[0, 0, 4, 0, 0, 0, 3, 0, 0, 0, 0x10, 0]);
    }
}
