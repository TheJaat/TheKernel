//! Where a panic or a CPU exception happened, in words: the thread, and
//! the calls that led there, named by the kernel's own symbols. The table
//! and the walk are kernel64::symbols; this is where they are in memory.

use kernel64::symbols::{self, Table, BLOCK_SIZE, MAGIC};

/// The block the build fills in after linking (scripts/fill-symbols.py).
#[repr(C, align(8))]
pub struct Block {
    magic: [u8; 8],
    rest: [u8; BLOCK_SIZE - 8],
}

/// The kernel's own symbols. `#[no_mangle]`, so the fill script finds it
/// by name; `#[used]`, so it is kept though nothing names it but `table`.
/// Not all zeros (the magic), so it is in the file, not in .bss.
#[no_mangle]
#[used]
pub static TAAJ_SYMBOLS: Block = Block { magic: MAGIC, rest: [0; BLOCK_SIZE - 8] };

/// The block as the build filled it in: for the profiler's copy (K140).
pub fn block() -> &'static [u8] {
    // `black_box`, as in `table`: the bytes the fill script wrote.
    let at = core::hint::black_box(&raw const TAAJ_SYMBOLS).cast::<u8>();
    // SAFETY: BLOCK_SIZE bytes of the kernel's read-only data, mapped for
    // as long as the kernel runs.
    unsafe { core::slice::from_raw_parts(at, BLOCK_SIZE) }
}

/// The table, or None in a kernel whose block was never filled in.
fn table() -> Option<Table<'static>> {
    // The compiler knows what TAAJ_SYMBOLS was compiled as - zeros after
    // the magic - and would read the table as empty. `black_box` hides
    // where the pointer comes from, so the bytes are read from memory:
    // the ones the fill script wrote.
    let at = core::hint::black_box(&raw const TAAJ_SYMBOLS).cast::<u8>();
    // SAFETY: BLOCK_SIZE bytes of the kernel's read-only data, mapped for
    // as long as the kernel runs.
    Table::parse(unsafe { core::slice::from_raw_parts(at, BLOCK_SIZE) })
}

/// Says which thread this is and how it got here: `at`, if given, is
/// where it stopped (the faulting instruction); `link`, on AArch64, the
/// link register - where the function it stopped in returns to, which a
/// function that calls nothing never saves in a frame record (the nx
/// fault test branches into data from kernel_main: only x30 says so);
/// then each caller, from the frame record at `frame`. Called with the
/// other CPUs stopped.
pub fn report(at: Option<u64>, link: Option<u64>, frame: u64) {
    if let Some((name, id)) = crate::threads::current_for_report() {
        kprintln!("    in thread \"{name}\" ({id:?})");
    }
    let table = table();
    kprintln!("    backtrace{}:", if table.is_some() { "" } else { " (no symbols in this kernel)" });
    // A return address is just past the call; the call itself, one byte
    // back, is what is inside the calling function.
    let line = |n: usize, address: u64, returned_to: bool| {
        let look = if returned_to { address.saturating_sub(1) } else { address };
        match table.as_ref().and_then(|t| t.lookup(look)) {
            Some((name, offset)) => kprintln!("    #{n:<2} {address:#018x}  {name}+{:#x}", offset + (look != address) as u64),
            None => kprintln!("    #{n:<2} {address:#018x}"),
        }
    };
    let mut n = 0;
    if let Some(at) = at {
        line(0, at, false);
        n = 1;
    }
    if let Some(link) = link {
        match table.as_ref().and_then(|t| t.lookup(link.saturating_sub(1))) {
            Some((name, offset)) => kprintln!("    lr  {link:#018x}  {name}+{:#x}", offset + 1),
            None => kprintln!("    lr  {link:#018x}"),
        }
    }
    symbols::walk(
        frame,
        32,
        // SAFETY: `walk` reads only what may_read_stack allowed: a word of
        // the stack the walk started on, or of the kernel image's data.
        |address| unsafe { (address as *const u64).read_volatile() },
        // The whole record, both words, must be readable.
        |from, to| crate::mm::may_read_stack(from, to) && crate::mm::may_read_stack(from, to + 8),
        |returned_to| {
            line(n, returned_to, true);
            n += 1;
        },
    );
}
