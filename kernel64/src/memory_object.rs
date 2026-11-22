//! Memory objects: physical pages as a kernel object.
//!
//! A memory object is a list of physical frames. A handle to one lets a
//! process map those frames into its address space - and two processes
//! mapping the same object see the same bytes, with no copying: what
//! one writes, the other reads. That is how large data moves between
//! processes (pixels, file contents, network packets), where copying it
//! through channel messages would cost too much.
//!
//! Lifetime: the frames are freed when the last reference goes - and a
//! mapping is a reference, not just a handle. Closing the handle of an
//! object that is still mapped leaves the mapping working; the pages go
//! once the mapping is gone too. So no process can ever be left with a
//! mapping of a page that was freed and reused for something else.
//!
//! Most objects own their frames. A device object instead names fixed
//! physical memory - the framebuffer - and frees nothing.

use alloc::vec::Vec;

use kernel64::memory::addr::{phys_to_virt, PhysAddr, PAGE_SIZE};
use kernel64::paging::Flags;

use crate::mm;

pub struct MemoryObject {
    frames: Vec<PhysAddr>,
    owned: bool,
    /// Extra page flags for every mapping - caching, for devices.
    pub cache: Flags,
}

impl MemoryObject {
    /// `pages` fresh, zeroed frames. None if memory runs out.
    pub fn new(pages: u64) -> Option<MemoryObject> {
        let mut object = MemoryObject { frames: Vec::new(), owned: true, cache: Flags::NONE };
        mm::with_frames(|frames| {
            for _ in 0..pages {
                let frame = frames.allocate()?;
                // SAFETY: a fresh frame, through the direct map. Zeroed:
                // whatever the last owner left there must not leak.
                unsafe { phys_to_virt(frame).as_mut_ptr::<u8>().write_bytes(0, PAGE_SIZE as usize) };
                object.frames.push(frame);
            }
            Some(())
        })?;
        Some(object)
    }

    /// `pages` fresh, zeroed frames side by side, and where they start -
    /// for DMA: a device that reads and writes memory itself knows only
    /// physical addresses, and a buffer must be one unbroken range of
    /// them. None if no run that long is free.
    pub fn contiguous(pages: u64) -> Option<(MemoryObject, u64)> {
        let start = mm::with_frames(|frames| frames.allocate_contiguous(pages as usize))?;
        let frames: Vec<PhysAddr> = (0..pages).map(|n| PhysAddr::new(start.as_u64() + n * PAGE_SIZE)).collect();
        // SAFETY: fresh frames, through the direct map.
        unsafe { phys_to_virt(start).as_mut_ptr::<u8>().write_bytes(0, (pages * PAGE_SIZE) as usize) };
        Some((MemoryObject { frames, owned: true, cache: Flags::NONE }, start.as_u64()))
    }

    /// Fixed physical memory - a device's - starting at `physical`.
    pub fn device(physical: u64, bytes: u64, cache: Flags) -> MemoryObject {
        let first = physical & !(PAGE_SIZE - 1);
        let end = (physical + bytes + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let frames = (first..end).step_by(PAGE_SIZE as usize).map(PhysAddr::new).collect();
        MemoryObject { frames, owned: false, cache }
    }

    /// Whether the object's frames are its own (not a device's).
    pub fn owned(&self) -> bool {
        self.owned
    }

    /// A new object holding a copy of this one's bytes - for the kernel
    /// to read what a process prepared (a program file) without anyone
    /// changing it meanwhile, when someone else could. In fresh frames,
    /// not the kernel's heap: they go back to the frame allocator when
    /// the copy is dropped, however big it was. None if memory runs out.
    pub fn copy(&self) -> Option<MemoryObject> {
        let copy = MemoryObject::new(self.frames.len() as u64)?;
        for (&from, &to) in self.frames.iter().zip(&copy.frames) {
            // SAFETY: two frames, through the direct map: `to` is the
            // copy's own and fresh; `from` is this object's.
            unsafe {
                phys_to_virt(to)
                    .as_mut_ptr::<u8>()
                    .copy_from_nonoverlapping(phys_to_virt(from).as_ptr::<u8>(), PAGE_SIZE as usize)
            };
        }
        Some(copy)
    }

    pub fn frames(&self) -> &[PhysAddr] {
        &self.frames
    }

    pub fn size(&self) -> u64 {
        self.frames.len() as u64 * PAGE_SIZE
    }
}

/// A program file is read straight out of the object's pages.
impl kernel64::elf::Source for MemoryObject {
    fn file_size(&self) -> u64 {
        self.size()
    }

    fn read(&self, offset: u64, out: &mut [u8]) {
        let mut done = 0;
        while done < out.len() {
            let at = offset + done as u64;
            let frame = self.frames[(at / PAGE_SIZE) as usize];
            let within = (at % PAGE_SIZE) as usize;
            let n = (PAGE_SIZE as usize - within).min(out.len() - done);
            // SAFETY: a frame of this object, through the direct map;
            // `within + n` stays inside the page.
            let page = unsafe { core::slice::from_raw_parts(phys_to_virt(frame).as_ptr::<u8>(), PAGE_SIZE as usize) };
            out[done..done + n].copy_from_slice(&page[within..within + n]);
            done += n;
        }
    }
}

impl Drop for MemoryObject {
    fn drop(&mut self) {
        if self.owned {
            mm::with_frames(|frames| {
                for &frame in &self.frames {
                    frames.free(frame);
                }
            });
        }
    }
}
