//! The device tree: how a machine without ACPI describes itself.
//!
//! On a PC the firmware leaves ACPI tables in memory, and the kernel
//! reads from them how many CPUs there are and where the interrupt
//! controllers sit. On most AArch64 and RISC-V machines the loader
//! instead hands the kernel one block of bytes, the "flattened device
//! tree" (often "DTB", for device tree blob): a tree of named nodes -
//! `/memory`, `/cpus/cpu@0`, a serial port - each with properties.
//!
//! # The format
//!
//! ```text
//!   header            ten numbers: sizes, and where the parts are
//!   reserved memory   pairs (address, size), ended by (0, 0)
//!   structure         the tree, as a stream of tokens
//!   strings           the property names, each ended by a zero byte
//! ```
//!
//! The structure is the tree written out in the order you would read
//! it aloud: BEGIN a node (its name follows), its PROPerties (each a
//! length, the place of its name in the strings part, and that many
//! bytes), its child nodes in the same form, END. Every token starts
//! on a multiple of four bytes.
//!
//! A property's value is only bytes. What they mean is a matter of
//! convention per property: text, a list of texts, or 32-bit numbers
//! ("cells"). The one that takes care is `reg`, a device's addresses:
//! how many cells make an address and how many a size is said by the
//! *parent* node, in `#address-cells` and `#size-cells`.
//!
//! Every number in a device tree is big-endian - most significant byte
//! first - whatever the CPU is. Hence `from_be_bytes` throughout.
//!
//! # What this reader promises
//!
//! It allocates nothing: the kernel reads the tree before it has a
//! heap. Everything it returns is a slice of the blob. And it never
//! trusts the blob: a length that runs off the end, a name with no
//! end, a tree that never closes - each is "nothing more here", not a
//! panic. The loader is not an attacker, but the address in x0 may be
//! wrong, and a kernel that crashes reading its own description says
//! nothing useful.

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


// The tokens of the structure part.
const BEGIN_NODE: u32 = 1;
const END_NODE: u32 = 2;
const PROPERTY: u32 = 3;
const NOTHING: u32 = 4;

/// Nodes deeper than this are not visited. Real trees are five or six
/// deep; the limit is what lets the reader keep, without a heap, the
/// cell sizes of every ancestor of the node it is at.
pub const DEPTH_MAX: usize = 16;

/// The 32-bit big-endian number at `offset`, if it is inside `bytes`.
fn number(bytes: &[u8], offset: usize) -> Option<u32> {
    let four = bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_be_bytes([four[0], four[1], four[2], four[3]]))
}

/// The text that starts at `offset` and ends before the next zero byte.
fn text_at(bytes: &[u8], offset: usize) -> Option<&str> {
    let rest = bytes.get(offset..)?;
    let end = rest.iter().position(|&byte| byte == 0)?;
    core::str::from_utf8(&rest[..end]).ok()
}

/// Reads `cells` 32-bit numbers from the front of `value` as one
/// number, and returns it with what is left. Two cells make a 64-bit
/// number; a third or fourth (PCI uses three) would not fit, so only
/// the last two are kept.
fn take_cells(value: &[u8], cells: u32) -> Option<(u64, &[u8])> {
    let length = cells as usize * 4;
    let (front, rest) = (value.get(..length)?, value.get(length..)?);
    let mut result: u64 = 0;
    // `as_chunks` cuts a slice into arrays of a fixed length (and a
    // remainder, empty here: `length` is a multiple of four).
    for &cell in front.as_chunks::<4>().0 {
        result = result << 32 | u64::from(u32::from_be_bytes(cell));
    }
    Some((result, rest))
}

/// One step through the structure part.
enum Token<'a> {
    Begin(&'a str),
    End,
    Property(&'a str, &'a [u8]),
}

/// A device tree, checked as far as its header and held as its parts.
#[derive(Clone, Copy)]
pub struct Tree<'a> {
    pub header: Header,
    reserved: &'a [u8],
    structure: &'a [u8],
    strings: &'a [u8],
}

impl<'a> Tree<'a> {
    /// Takes a blob - at least `total_size` bytes of it - and checks
    /// its header. The tree itself is checked as it is walked.
    pub fn new(blob: &'a [u8]) -> Result<Tree<'a>, Error> {
        let first: &[u8; HEADER_SIZE] =
            blob.get(..HEADER_SIZE).and_then(|bytes| bytes.try_into().ok()).ok_or(Error::Inconsistent)?;
        let header = Header::parse(first)?;
        let blob = blob.get(..header.total_size as usize).ok_or(Error::Inconsistent)?;
        // `Header::parse` checked that each part lies inside total_size.
        let part = |offset: u32, size: u32| &blob[offset as usize..offset as usize + size as usize];
        Ok(Tree {
            header,
            reserved: &blob[header.reserved_offset as usize..],
            structure: part(header.structure_offset, header.structure_size),
            strings: part(header.strings_offset, header.strings_size),
        })
    }

    /// The next token at `*offset`, moving it past; None at the end of
    /// the tree or at anything that is not a token.
    fn token(&self, offset: &mut usize) -> Option<Token<'a>> {
        loop {
            let kind = number(self.structure, *offset)?;
            *offset += 4;
            match kind {
                BEGIN_NODE => {
                    let name = text_at(self.structure, *offset)?;
                    // The name, its zero byte, then up to a multiple of four.
                    *offset = (*offset + name.len() + 1).checked_next_multiple_of(4)?;
                    return Some(Token::Begin(name));
                }
                END_NODE => return Some(Token::End),
                PROPERTY => {
                    let length = number(self.structure, *offset)? as usize;
                    let name = text_at(self.strings, number(self.structure, *offset + 4)? as usize)?;
                    let start = *offset + 8;
                    let value = self.structure.get(start..start.checked_add(length)?)?;
                    *offset = (start + length).checked_next_multiple_of(4)?;
                    return Some(Token::Property(name, value));
                }
                NOTHING => {}
                // END (9), or rubbish: either way there is no more.
                _ => return None,
            }
        }
    }

    /// Memory the kernel must not use, as (address, size) pairs.
    pub fn reserved(&self) -> impl Iterator<Item = Range> + 'a {
        self.reserved
            .as_chunks::<16>()
            .0
            .iter()
            .map(|pair| {
                let (start, rest) = take_cells(pair, 2).unwrap_or((0, &[]));
                let (size, _) = take_cells(rest, 2).unwrap_or((0, &[]));
                Range { start, size }
            })
            .take_while(|range| range.start != 0 || range.size != 0)
    }

    /// Every node, parents before children, in the order of the blob.
    pub fn nodes(&self) -> Nodes<'a> {
        Nodes { tree: *self, offset: 0, depth: 0, cells: [(2, 1, true); DEPTH_MAX + 1], skipping: 0 }
    }

    /// The root node, "/".
    pub fn root(&self) -> Option<Node<'a>> {
        self.nodes().next()
    }

    /// The node at a path like `/cpus/cpu@0`. A step may leave out the
    /// `@address` part of a name if only one child has that name.
    ///
    /// A path that does not begin with `/` is an *alias*: a short name
    /// that the node `/aliases` gives the full path for.
    pub fn find(&self, path: &str) -> Option<Node<'a>> {
        if !path.starts_with('/') {
            let full = self.find("/aliases")?.text(path)?;
            // An alias that names another alias would loop for ever.
            return if full.starts_with('/') { self.find(full) } else { None };
        }
        let mut node = self.root()?;
        for step in path.split('/').filter(|step| !step.is_empty()) {
            node = node.children().find(|child| child.name == step || child.base_name() == step)?;
        }
        Some(node)
    }

    /// The first node that says it is, or works like, `device`.
    pub fn compatible(&self, device: &str) -> Option<Node<'a>> {
        self.nodes().find(|node| node.is_compatible(device))
    }

    /// The node with this *phandle*: a number a node is given so that
    /// others can point at it ("my clock is node 0x8000").
    pub fn by_phandle(&self, phandle: u32) -> Option<Node<'a>> {
        self.nodes().find(|node| node.number("phandle") == Some(phandle))
    }
}

/// Some memory, or a device's registers: where, and how much.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Range {
    pub start: u64,
    pub size: u64,
}

/// One node of the tree.
#[derive(Clone, Copy)]
pub struct Node<'a> {
    tree: Tree<'a>,
    /// As written: `memory@40000000`. The root's name is empty.
    pub name: &'a str,
    /// How many nodes are above this one; 0 for the root.
    pub depth: usize,
    /// Where in the structure this node's properties begin.
    offset: usize,
    /// How this node's own `reg` is to be read: its parent's
    /// `#address-cells` and `#size-cells`.
    address_cells: u32,
    size_cells: u32,
    /// Whether an address in `reg` is one the CPU can use as it
    /// stands. A bus can number its devices its own way and say, in
    /// `ranges`, how its numbers turn into its parent's; an empty
    /// `ranges` means "the same". This is true if every bus above the
    /// node says "the same". (Turning the numbers is not done yet.)
    pub direct: bool,
}

impl<'a> Node<'a> {
    /// The name without its `@address` part: `memory`.
    pub fn base_name(&self) -> &'a str {
        self.name.split('@').next().unwrap_or(self.name)
    }

    /// This node's properties, in the order written.
    pub fn properties(&self) -> impl Iterator<Item = (&'a str, &'a [u8])> + 'a {
        let tree = self.tree;
        let mut offset = self.offset;
        // Properties come before child nodes: the first token that is
        // not a property ends them.
        core::iter::from_fn(move || match tree.token(&mut offset)? {
            Token::Property(name, value) => Some((name, value)),
            _ => None,
        })
    }

    /// The bytes of one property.
    pub fn property(&self, name: &str) -> Option<&'a [u8]> {
        self.properties().find(|&(found, _)| found == name).map(|(_, value)| value)
    }

    /// A property that is one 32-bit number.
    pub fn number(&self, name: &str) -> Option<u32> {
        match self.property(name)? {
            value if value.len() == 4 => number(value, 0),
            _ => None,
        }
    }

    /// A property that is a list of 32-bit numbers.
    pub fn numbers(&self, name: &str) -> impl Iterator<Item = u32> + 'a {
        self.property(name)
            .unwrap_or(&[])
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&cell| u32::from_be_bytes(cell))
    }

    /// A property that is text - or the first, if it is a list of texts.
    pub fn text(&self, name: &str) -> Option<&'a str> {
        self.texts(name).next()
    }

    /// A property that is a list of texts, each ended by a zero byte.
    pub fn texts(&self, name: &str) -> impl Iterator<Item = &'a str> + 'a {
        // Text ends with a zero byte: a value without one (an empty
        // property, or numbers) holds no texts at all. The zero is
        // taken off first, or splitting at zeroes would find an empty
        // text after the last one.
        let value = self.property(name).and_then(|value| value.strip_suffix(&[0]));
        value.into_iter().flat_map(|value| value.split(|&byte| byte == 0)).filter_map(|text| core::str::from_utf8(text).ok())
    }

    /// Whether the node is to be used. A node can be in the tree and
    /// say that it is not: a board's tree often lists every device the
    /// chip has, with those not wired up marked "disabled".
    pub fn enabled(&self) -> bool {
        matches!(self.text("status"), None | Some("okay") | Some("ok"))
    }

    /// Whether the node's `compatible` list names `device`. The list
    /// runs from the exact part to more general ones it can be driven
    /// as: "arm,pl011" then "arm,primecell".
    pub fn is_compatible(&self, device: &str) -> bool {
        self.texts("compatible").any(|name| name == device)
    }

    /// The node's addresses: (address, size) pairs from `reg`, read
    /// with the parent's cell counts.
    pub fn reg(&self) -> impl Iterator<Item = Range> + 'a {
        let (address_cells, size_cells) = (self.address_cells, self.size_cells);
        let mut rest = self.property("reg").unwrap_or(&[]);
        core::iter::from_fn(move || {
            if rest.is_empty() {
                return None;
            }
            let (start, after) = take_cells(rest, address_cells)?;
            let (size, after) = take_cells(after, size_cells)?;
            rest = after;
            Some(Range { start, size })
        })
    }

    /// The nodes directly inside this one.
    pub fn children(&self) -> impl Iterator<Item = Node<'a>> + 'a {
        let depth = self.depth + 1;
        // A walk of everything, begun at this node's first token with
        // this node as the context: it yields this node's descendants
        // and stops when the node closes.
        let mut cells = [(2, 1, true); DEPTH_MAX + 1];
        if depth <= DEPTH_MAX {
            cells[depth] = self.own_cells();
        }
        let walk = Nodes { tree: self.tree, offset: self.offset, depth, cells, skipping: 0 };
        walk.take_while(move |node| node.depth >= depth).filter(move |node| node.depth == depth)
    }

    /// What this node says of its children's `reg`: `#address-cells`
    /// (2 if it does not say), `#size-cells` (1), and whether their
    /// addresses are the CPU's own.
    fn own_cells(&self) -> (u32, u32, bool) {
        let ranges_are_empty = self.property("ranges").is_some_and(|ranges| ranges.is_empty());
        // The root has no bus above it: its children's addresses are
        // the CPU's whatever it says.
        let direct = self.direct && (self.depth == 0 || ranges_are_empty);
        (self.number("#address-cells").unwrap_or(2), self.number("#size-cells").unwrap_or(1), direct)
    }
}

/// The walk behind `Tree::nodes`.
pub struct Nodes<'a> {
    tree: Tree<'a>,
    offset: usize,
    /// The depth the next node met will have.
    depth: usize,
    /// For each depth: how nodes at that depth read their `reg`, as
    /// their parent said (address cells, size cells, direct).
    cells: [(u32, u32, bool); DEPTH_MAX + 1],
    /// How many levels below `DEPTH_MAX` the walk is, visiting nothing.
    skipping: usize,
}

impl<'a> Iterator for Nodes<'a> {
    type Item = Node<'a>;

    fn next(&mut self) -> Option<Node<'a>> {
        loop {
            match self.tree.token(&mut self.offset)? {
                Token::Property(..) => {}
                Token::End => {
                    if self.skipping > 0 {
                        self.skipping -= 1;
                    } else {
                        // The node the walk began in has closed.
                        self.depth = self.depth.checked_sub(1)?;
                    }
                }
                Token::Begin(_) if self.depth > DEPTH_MAX || self.skipping > 0 => self.skipping += 1,
                Token::Begin(name) => {
                    let (address_cells, size_cells, direct) = self.cells[self.depth];
                    let node =
                        Node { tree: self.tree, name, depth: self.depth, offset: self.offset, address_cells, size_cells, direct };
                    self.depth += 1;
                    if self.depth <= DEPTH_MAX {
                        self.cells[self.depth] = node.own_cells();
                    }
                    return Some(node);
                }
            }
        }
    }
}

/// What the kernel wants to know about the machine before it can do
/// anything else, gathered from the tree in one pass. Fixed-size, like
/// everything here: it is read before there is a heap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Machine<'a> {
    /// The board's name for itself.
    pub model: Option<&'a str>,
    /// Where memory is. `memory_count` may exceed the array: the rest
    /// were found and not kept.
    pub memory: [Range; Machine::MEMORY_MAX],
    pub memory_count: usize,
    /// Memory that is there but is not the kernel's to use: the
    /// blob's own reserved list, and what the `/reserved-memory` node
    /// gives an address to. Counted past the array like `memory`.
    pub reserved: [Range; Machine::RESERVED_MAX],
    pub reserved_count: usize,
    /// Each CPU's identifier (what its `MPIDR_EL1` register reads).
    pub cpus: [u64; Machine::CPUS_MAX],
    pub cpu_count: usize,
    pub serial: Option<Serial>,
    pub interrupt_controller: Option<InterruptController>,
    /// The interrupt numbers of the CPU's four timers, in the tree's
    /// order: secure, physical, virtual, hypervisor.
    pub timer_interrupts: [Option<u32>; 4],
    /// How to ask the firmware to start a CPU or switch the machine
    /// off (the "PSCI" calls).
    pub firmware_call: Option<FirmwareCall>,
    pub nodes: usize,
    pub deepest: usize,
}

/// A PL011 serial port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Serial {
    pub registers: Range,
    /// The clock it divides down to make the baud rate.
    pub clock_hz: Option<u32>,
    /// Whether the tree named it as the console (`/chosen`,
    /// `stdout-path`), or it was just the first one found.
    pub chosen: bool,
}

/// Arm's "generic interrupt controller".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterruptController {
    /// 2 or 3. They are set up differently.
    pub version: u8,
    /// The part shared by all CPUs.
    pub distributor: Range,
    /// The per-CPU part: in version 2 the "CPU interface", in version
    /// 3 the "redistributors".
    pub per_cpu: Range,
}

/// The instruction that reaches the firmware: `hvc` calls exception
/// level 2, `smc` level 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirmwareCall {
    Hvc,
    Smc,
}

impl<'a> Machine<'a> {
    pub const MEMORY_MAX: usize = 8;
    pub const RESERVED_MAX: usize = 16;
    pub const CPUS_MAX: usize = 16;

    pub fn read(tree: &Tree<'a>) -> Machine<'a> {
        let mut machine = Machine { model: tree.root().and_then(|root| root.text("model")), ..Machine::default() };

        for node in tree.nodes() {
            machine.nodes += 1;
            machine.deepest = machine.deepest.max(node.depth);
            if !node.enabled() {
                continue;
            }
            match node.text("device_type") {
                Some("memory") if node.direct => {
                    for range in node.reg().filter(|range| range.size > 0) {
                        if let Some(slot) = machine.memory.get_mut(machine.memory_count) {
                            *slot = range;
                        }
                        machine.memory_count += 1;
                    }
                }
                Some("cpu") => {
                    if let Some(id) = node.reg().next() {
                        if let Some(slot) = machine.cpus.get_mut(machine.cpu_count) {
                            *slot = id.start;
                        }
                        machine.cpu_count += 1;
                    }
                }
                _ => {}
            }

            if machine.interrupt_controller.is_none() && node.direct {
                let version = if node.is_compatible("arm,gic-v3") {
                    Some(3)
                } else if ["arm,cortex-a15-gic", "arm,gic-400", "arm,cortex-a9-gic"].iter().any(|name| node.is_compatible(name)) {
                    Some(2)
                } else {
                    None
                };
                let mut reg = node.reg();
                if let (Some(version), Some(distributor), Some(per_cpu)) = (version, reg.next(), reg.next()) {
                    machine.interrupt_controller = Some(InterruptController { version, distributor, per_cpu });
                }
            }

            if node.is_compatible("arm,armv8-timer") {
                // Each interrupt is three numbers, as the interrupt
                // controller defines them: the kind (1 = private to
                // each CPU, 0 = shared), a number within that kind,
                // and flags. Private interrupts are numbers 16 to 31
                // of the controller's own numbering; shared ones
                // follow from 32.
                let mut numbers = node.numbers("interrupts");
                for slot in &mut machine.timer_interrupts {
                    let (Some(kind), Some(number), Some(_flags)) = (numbers.next(), numbers.next(), numbers.next()) else {
                        break;
                    };
                    *slot = Some(number + if kind == 1 { 16 } else { 32 });
                }
            }

            if node.is_compatible("arm,psci") || node.is_compatible("arm,psci-0.2") || node.is_compatible("arm,psci-1.0") {
                machine.firmware_call = match node.text("method") {
                    Some("hvc") => Some(FirmwareCall::Hvc),
                    Some("smc") => Some(FirmwareCall::Smc),
                    _ => None,
                };
            }
        }

        // Reserved memory is described twice over. The blob has a
        // list of its own, outside the tree (the old way); and the
        // tree may have a `/reserved-memory` node whose children each
        // say what one range is for (the new way). A child with no
        // `reg` asks for memory to be found for it, which is not a
        // range to stay out of. A disabled child reserves nothing.
        let listed = tree.reserved();
        let described = tree
            .find("/reserved-memory")
            .into_iter()
            .flat_map(|node| node.children())
            .filter(|child| child.enabled() && child.direct)
            .flat_map(|child| child.reg());
        for range in listed.chain(described).filter(|range| range.size > 0) {
            if let Some(slot) = machine.reserved.get_mut(machine.reserved_count) {
                *slot = range;
            }
            machine.reserved_count += 1;
        }

        machine.serial = Machine::find_serial(tree);
        machine
    }

    /// The console the tree names, if it is a PL011; else the first
    /// PL011 there is.
    fn find_serial(tree: &Tree<'a>) -> Option<Serial> {
        let usable = |node: &Node<'a>| node.is_compatible("arm,pl011") && node.direct && node.enabled();
        let named = tree
            .find("/chosen")
            .and_then(|chosen| chosen.text("stdout-path"))
            // "path:115200n8": after the colon are line settings.
            .and_then(|path| tree.find(path.split(':').next().unwrap_or(path)))
            .filter(usable);
        let chosen = named.is_some();
        let node = named.or_else(|| tree.nodes().find(usable))?;
        let registers = node.reg().next()?;
        // `clocks` is a list of pointers to clock nodes; the first is
        // the one the baud rate comes from.
        let clock_hz = node
            .numbers("clocks")
            .next()
            .and_then(|phandle| tree.by_phandle(phandle))
            .and_then(|clock| clock.number("clock-frequency"));
        Some(Serial { registers, clock_hz, chosen })
    }

    /// The memory ranges kept.
    pub fn memory(&self) -> &[Range] {
        &self.memory[..self.memory_count.min(Machine::MEMORY_MAX)]
    }

    /// The reserved ranges kept.
    pub fn reserved(&self) -> &[Range] {
        &self.reserved[..self.reserved_count.min(Machine::RESERVED_MAX)]
    }

    /// The CPU identifiers kept.
    pub fn cpus(&self) -> &[u64] {
        &self.cpus[..self.cpu_count.min(Machine::CPUS_MAX)]
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

    // ---- the tree -------------------------------------------------------

    /// Builds a device tree blob, for the tests: nodes are opened and
    /// closed in the order a tree is written.
    struct Builder {
        structure: Vec<u8>,
        strings: Vec<u8>,
        /// The blob's reserved list: (start, size) pairs.
        reserved: Vec<(u64, u64)>,
    }

    impl Builder {
        fn new() -> Builder {
            Builder { structure: Vec::new(), strings: Vec::new(), reserved: Vec::new() }
        }

        fn pad(&mut self) {
            while !self.structure.len().is_multiple_of(4) {
                self.structure.push(0);
            }
        }

        fn begin(mut self, name: &str) -> Builder {
            self.structure.extend(BEGIN_NODE.to_be_bytes());
            self.structure.extend(name.as_bytes());
            self.structure.push(0);
            self.pad();
            self
        }

        fn end(mut self) -> Builder {
            self.structure.extend(END_NODE.to_be_bytes());
            self
        }

        fn bytes(mut self, name: &str, value: &[u8]) -> Builder {
            let name_offset = self.strings.len() as u32;
            self.strings.extend(name.as_bytes());
            self.strings.push(0);
            self.structure.extend(PROPERTY.to_be_bytes());
            self.structure.extend((value.len() as u32).to_be_bytes());
            self.structure.extend(name_offset.to_be_bytes());
            self.structure.extend(value);
            self.pad();
            self
        }

        fn cells(self, name: &str, cells: &[u32]) -> Builder {
            let value: Vec<u8> = cells.iter().flat_map(|cell| cell.to_be_bytes()).collect();
            self.bytes(name, &value)
        }

        fn text(self, name: &str, texts: &[&str]) -> Builder {
            let value: Vec<u8> = texts.iter().flat_map(|text| text.bytes().chain([0])).collect();
            self.bytes(name, &value)
        }

        fn reserve(mut self, start: u64, size: u64) -> Builder {
            self.reserved.push((start, size));
            self
        }

        fn nothing(mut self) -> Builder {
            self.structure.extend(NOTHING.to_be_bytes());
            self
        }

        /// The finished blob: header, the reserved list (which ends
        /// with a pair of zeroes), the structure (with its END token),
        /// the strings.
        fn blob(mut self) -> Vec<u8> {
            self.structure.extend(9u32.to_be_bytes());
            let reserved_offset = HEADER_SIZE as u32;
            let structure_offset = reserved_offset + 16 * (self.reserved.len() as u32 + 1);
            let strings_offset = structure_offset + self.structure.len() as u32;
            let total = strings_offset + self.strings.len() as u32;
            let words = [
                MAGIC,
                total,
                structure_offset,
                strings_offset,
                reserved_offset,
                17,
                16,
                0,
                self.strings.len() as u32,
                self.structure.len() as u32,
            ];
            let mut blob: Vec<u8> = header(words).to_vec();
            for (start, size) in &self.reserved {
                blob.extend(start.to_be_bytes());
                blob.extend(size.to_be_bytes());
            }
            blob.extend([0u8; 16]);
            blob.extend(&self.structure);
            blob.extend(&self.strings);
            blob
        }
    }

    /// A small machine: two cells per address and size at the top, a
    /// bus that keeps the CPU's addresses, and one that does not.
    fn small() -> Vec<u8> {
        Builder::new()
            .begin("")
            .text("model", &["a test board"])
            .cells("#address-cells", &[2])
            .cells("#size-cells", &[2])
            .begin("aliases")
            .text("console", &["/soc/serial@1000"])
            .text("loop", &["loop"])
            .end()
            .begin("chosen")
            .text("stdout-path", &["console:115200n8"])
            .end()
            .begin("memory@80000000")
            .text("device_type", &["memory"])
            .cells("reg", &[0, 0x8000_0000, 0, 0x1000_0000, 1, 0, 0, 0x4000_0000])
            .end()
            .nothing()
            .begin("clock")
            .cells("phandle", &[7])
            .cells("clock-frequency", &[48_000_000])
            .end()
            .begin("soc")
            .cells("#address-cells", &[1])
            .cells("#size-cells", &[1])
            .bytes("ranges", &[])
            .begin("serial@1000")
            .text("compatible", &["arm,pl011", "arm,primecell"])
            .cells("reg", &[0x1000, 0x100])
            .cells("clocks", &[7, 7])
            .end()
            .begin("serial@2000")
            .text("compatible", &["arm,pl011"])
            .text("status", &["disabled"])
            .cells("reg", &[0x2000, 0x100])
            .end()
            .end()
            .begin("far-bus")
            .cells("#address-cells", &[1])
            .cells("#size-cells", &[1])
            .cells("ranges", &[0, 0xF, 0, 0x1000])
            .begin("serial@0")
            .text("compatible", &["arm,pl011"])
            .cells("reg", &[0, 0x100])
            .end()
            .end()
            .begin("cpus")
            .cells("#address-cells", &[1])
            .cells("#size-cells", &[0])
            .begin("cpu@0")
            .text("device_type", &["cpu"])
            .cells("reg", &[0])
            .end()
            .begin("cpu@101")
            .text("device_type", &["cpu"])
            .cells("reg", &[0x101])
            .end()
            .end()
            .end()
            .blob()
    }

    #[test]
    fn nodes_come_in_order_with_their_depth() {
        let blob = small();
        let tree = Tree::new(&blob).unwrap();
        let seen: Vec<(usize, &str)> = tree.nodes().map(|node| (node.depth, node.name)).collect();
        assert_eq!(
            seen,
            [
                (0, ""),
                (1, "aliases"),
                (1, "chosen"),
                (1, "memory@80000000"),
                (1, "clock"),
                (1, "soc"),
                (2, "serial@1000"),
                (2, "serial@2000"),
                (1, "far-bus"),
                (2, "serial@0"),
                (1, "cpus"),
                (2, "cpu@0"),
                (2, "cpu@101"),
            ]
        );
    }

    #[test]
    fn properties_are_read_as_what_they_are() {
        let blob = small();
        let tree = Tree::new(&blob).unwrap();
        let root = tree.root().unwrap();
        assert_eq!(root.text("model"), Some("a test board"));
        assert_eq!(root.number("#address-cells"), Some(2));
        assert_eq!(root.number("model"), None, "twelve bytes are not one number");
        assert_eq!(root.property("nothing"), None);
        let names: Vec<&str> = root.properties().map(|(name, _)| name).collect();
        assert_eq!(names, ["model", "#address-cells", "#size-cells"]);

        let serial = tree.find("/soc/serial@1000").unwrap();
        assert_eq!(serial.texts("compatible").collect::<Vec<_>>(), ["arm,pl011", "arm,primecell"]);
        assert!(serial.is_compatible("arm,primecell"));
        assert!(!serial.is_compatible("arm,pl01"));
        assert_eq!(serial.numbers("clocks").collect::<Vec<_>>(), [7, 7]);
        assert_eq!(serial.base_name(), "serial");
        // An empty property is there, and is no text.
        let soc = tree.find("/soc").unwrap();
        assert_eq!(soc.property("ranges"), Some(&[][..]));
        assert_eq!(soc.text("ranges"), None);
    }

    #[test]
    fn reg_is_read_with_the_parents_cell_counts() {
        let blob = small();
        let tree = Tree::new(&blob).unwrap();
        // Two cells each: the second range starts above 4 GiB.
        let memory: Vec<Range> = tree.find("/memory").unwrap().reg().collect();
        assert_eq!(
            memory,
            [Range { start: 0x8000_0000, size: 0x1000_0000 }, Range { start: 0x1_0000_0000, size: 0x4000_0000 }]
        );
        // One cell each, as /soc says.
        let serial: Vec<Range> = tree.find("/soc/serial@1000").unwrap().reg().collect();
        assert_eq!(serial, [Range { start: 0x1000, size: 0x100 }]);
        // One cell and none: a CPU has a number, not a size.
        let cpu: Vec<Range> = tree.find("/cpus/cpu@101").unwrap().reg().collect();
        assert_eq!(cpu, [Range { start: 0x101, size: 0 }]);
    }

    #[test]
    fn a_node_that_does_not_say_gets_the_defaults() {
        // No #address-cells anywhere: 2 and 1.
        let blob = Builder::new().begin("").begin("thing@0").cells("reg", &[1, 2, 3]).end().end().blob();
        let tree = Tree::new(&blob).unwrap();
        let thing: Vec<Range> = tree.find("/thing").unwrap().reg().collect();
        assert_eq!(thing, [Range { start: 0x1_0000_0002, size: 3 }]);
        // A reg that is not a whole number of entries gives the whole ones.
        let blob = Builder::new().begin("").begin("thing").cells("reg", &[1, 2, 3, 4]).end().end().blob();
        assert_eq!(Tree::new(&blob).unwrap().find("/thing").unwrap().reg().count(), 1);
    }

    #[test]
    fn children_are_only_the_next_level() {
        let blob = small();
        let tree = Tree::new(&blob).unwrap();
        let top: Vec<&str> = tree.root().unwrap().children().map(|node| node.name).collect();
        assert_eq!(top, ["aliases", "chosen", "memory@80000000", "clock", "soc", "far-bus", "cpus"]);
        let cpus: Vec<&str> = tree.find("/cpus").unwrap().children().map(|node| node.name).collect();
        assert_eq!(cpus, ["cpu@0", "cpu@101"]);
        assert_eq!(tree.find("/chosen").unwrap().children().count(), 0);
        // A child found through `children` reads its reg as one found by the walk does.
        let through_children = tree.find("/soc").unwrap().children().next().unwrap();
        assert_eq!(through_children.reg().next(), Some(Range { start: 0x1000, size: 0x100 }));
        assert_eq!(through_children.depth, 2);
    }

    #[test]
    fn paths_and_aliases() {
        let blob = small();
        let tree = Tree::new(&blob).unwrap();
        assert_eq!(tree.find("/").unwrap().name, "");
        assert_eq!(tree.find("/cpus/cpu@0").unwrap().name, "cpu@0");
        assert_eq!(tree.find("/memory").unwrap().name, "memory@80000000");
        // Two children are called "serial": the first is taken.
        assert_eq!(tree.find("/soc/serial").unwrap().name, "serial@1000");
        assert!(tree.find("/soc/serial@3000").is_none());
        assert!(tree.find("/cpus/cpu@0/deeper").is_none());
        assert_eq!(tree.find("console").unwrap().name, "serial@1000");
        assert!(tree.find("nobody").is_none());
        assert!(tree.find("loop").is_none(), "an alias for an alias is refused, not followed for ever");
        assert_eq!(tree.by_phandle(7).unwrap().name, "clock");
        assert!(tree.by_phandle(8).is_none());
        assert_eq!(tree.compatible("arm,primecell").unwrap().name, "serial@1000");
    }

    #[test]
    fn a_bus_that_renumbers_makes_its_devices_not_direct() {
        let blob = small();
        let tree = Tree::new(&blob).unwrap();
        assert!(tree.find("/memory").unwrap().direct);
        assert!(tree.find("/soc/serial@1000").unwrap().direct, "empty ranges: the same addresses");
        assert!(!tree.find("/far-bus/serial@0").unwrap().direct);
        // No `ranges` at all means the children cannot be reached from above.
        let blob = Builder::new().begin("").begin("bus").begin("thing").end().end().end().blob();
        assert!(!Tree::new(&blob).unwrap().find("/bus/thing").unwrap().direct);
    }

    #[test]
    fn the_machine_is_read_from_the_small_tree() {
        let blob = small();
        let machine = Machine::read(&Tree::new(&blob).unwrap());
        assert_eq!(machine.model, Some("a test board"));
        assert_eq!(machine.memory().len(), 2);
        assert_eq!(machine.memory()[1], Range { start: 0x1_0000_0000, size: 0x4000_0000 });
        assert_eq!(machine.cpus(), [0, 0x101]);
        // Named by /chosen through an alias, with line settings after it;
        // its clock found through a phandle.
        assert_eq!(
            machine.serial,
            Some(Serial { registers: Range { start: 0x1000, size: 0x100 }, clock_hz: Some(48_000_000), chosen: true })
        );
        assert_eq!(machine.interrupt_controller, None);
        assert_eq!(machine.firmware_call, None);
        assert_eq!(machine.nodes, 13);
        assert_eq!(machine.deepest, 2);
    }

    #[test]
    fn reserved_memory_is_gathered_from_both_places() {
        let blob = Builder::new()
            .reserve(0x8000_0000, 0x1_0000)
            .reserve(0x9000_0000, 0) // an empty one reserves nothing
            .begin("")
            .cells("#address-cells", &[2])
            .cells("#size-cells", &[2])
            .begin("reserved-memory")
            .cells("#address-cells", &[2])
            .cells("#size-cells", &[2])
            .bytes("ranges", &[])
            .begin("firmware@88000000")
            .cells("reg", &[0, 0x8800_0000, 0, 0x20_0000])
            .bytes("no-map", &[])
            .end()
            // Wants 64 MiB found for it, anywhere: no address, so
            // nothing to stay out of.
            .begin("pool")
            .cells("size", &[0, 0x400_0000])
            .end()
            .begin("unused@a0000000")
            .text("status", &["disabled"])
            .cells("reg", &[0, 0xA000_0000, 0, 0x1000])
            .end()
            .begin("two@b0000000")
            .cells("reg", &[0, 0xB000_0000, 0, 0x1000, 1, 0, 0, 0x2000])
            .end()
            .end()
            // Not under /reserved-memory: an ordinary device.
            .begin("device@c0000000")
            .cells("reg", &[0, 0xC000_0000, 0, 0x1000])
            .end()
            .end()
            .blob();
        let machine = Machine::read(&Tree::new(&blob).unwrap());
        assert_eq!(
            machine.reserved(),
            [
                Range { start: 0x8000_0000, size: 0x1_0000 },
                Range { start: 0x8800_0000, size: 0x20_0000 },
                Range { start: 0xB000_0000, size: 0x1000 },
                Range { start: 0x1_0000_0000, size: 0x2000 },
            ]
        );
    }

    #[test]
    fn more_reserved_ranges_than_there_is_room_for_are_counted() {
        let mut builder = Builder::new();
        for n in 0..20u64 {
            builder = builder.reserve(n << 20, 0x1000);
        }
        let blob = builder.begin("").end().blob();
        let machine = Machine::read(&Tree::new(&blob).unwrap());
        assert_eq!(machine.reserved_count, 20);
        assert_eq!(machine.reserved().len(), Machine::RESERVED_MAX);
        assert_eq!(machine.reserved()[15], Range { start: 15 << 20, size: 0x1000 });
    }

    #[test]
    fn a_serial_port_nobody_named_is_still_found() {
        // The only usable one is behind nothing: the disabled one and
        // the one on the renumbering bus are passed over.
        let blob = Builder::new()
            .begin("")
            .begin("bus")
            .cells("#address-cells", &[1])
            .cells("ranges", &[0, 1, 2])
            .begin("serial@0")
            .text("compatible", &["arm,pl011"])
            .cells("reg", &[0, 0x100])
            .end()
            .end()
            .begin("serial@9000")
            .text("compatible", &["arm,pl011"])
            .cells("reg", &[0, 0x9000, 0x1000])
            .end()
            .end()
            .blob();
        let machine = Machine::read(&Tree::new(&blob).unwrap());
        assert_eq!(
            machine.serial,
            Some(Serial { registers: Range { start: 0x9000, size: 0x1000 }, clock_hz: None, chosen: false })
        );
    }

    #[test]
    fn what_is_disabled_is_not_used() {
        let blob = Builder::new()
            .begin("")
            .begin("serial@1000")
            .text("compatible", &["arm,pl011"])
            .text("status", &["disabled"])
            .cells("reg", &[0, 0x1000, 0x100])
            .end()
            .begin("serial@2000")
            .text("compatible", &["arm,pl011"])
            .text("status", &["okay"])
            .cells("reg", &[0, 0x2000, 0x100])
            .end()
            .begin("memory@0")
            .text("device_type", &["memory"])
            .text("status", &["disabled"])
            .cells("reg", &[0, 0, 0x1000])
            .end()
            .begin("cpus")
            .cells("#address-cells", &[1])
            .cells("#size-cells", &[0])
            .begin("cpu@0")
            .text("device_type", &["cpu"])
            .cells("reg", &[0])
            .end()
            .begin("cpu@1")
            .text("device_type", &["cpu"])
            .text("status", &["fail"])
            .cells("reg", &[1])
            .end()
            .end()
            .end()
            .blob();
        let tree = Tree::new(&blob).unwrap();
        assert!(!tree.find("/serial@1000").unwrap().enabled());
        assert!(tree.find("/serial@2000").unwrap().enabled());
        let machine = Machine::read(&tree);
        assert_eq!(machine.serial.unwrap().registers.start, 0x2000, "the first serial port is disabled");
        assert_eq!(machine.memory_count, 0);
        assert_eq!(machine.cpus(), [0]);
    }

    #[test]
    fn an_address_of_three_cells_keeps_its_last_two() {
        // A PCI bus: three cells of address, two of size.
        let blob = Builder::new()
            .begin("")
            .begin("pci")
            .cells("#address-cells", &[3])
            .cells("#size-cells", &[2])
            .bytes("ranges", &[])
            .begin("device")
            .cells("reg", &[0xAAAA, 0x1, 0x2, 0x0, 0x10])
            .end()
            .end()
            .end()
            .blob();
        let device = Tree::new(&blob).unwrap().find("/pci/device").unwrap();
        assert_eq!(device.reg().collect::<Vec<_>>(), [Range { start: 0x1_0000_0002, size: 0x10 }]);
    }

    #[test]
    fn more_memory_ranges_than_there_is_room_for_are_counted() {
        let mut builder = Builder::new().begin("");
        for n in 0..10u32 {
            builder = builder.begin("memory").text("device_type", &["memory"]).cells("reg", &[0, n << 20, 0x1000]).end();
        }
        let blob = builder.end().blob();
        let machine = Machine::read(&Tree::new(&blob).unwrap());
        assert_eq!(machine.memory_count, 10);
        assert_eq!(machine.memory().len(), 8);
    }

    #[test]
    fn nodes_deeper_than_the_limit_are_passed_over() {
        let mut builder = Builder::new().begin("");
        for _ in 0..DEPTH_MAX + 4 {
            builder = builder.begin("down");
        }
        for _ in 0..DEPTH_MAX + 4 {
            builder = builder.end();
        }
        let blob = builder.begin("after").end().end().blob();
        let tree = Tree::new(&blob).unwrap();
        let names: Vec<(usize, &str)> = tree.nodes().map(|node| (node.depth, node.name)).collect();
        assert_eq!(names.len(), 1 + DEPTH_MAX + 1, "the root, sixteen levels, and the node after");
        assert_eq!(names.last(), Some(&(1, "after")));
        assert_eq!(tree.root().unwrap().children().count(), 2);
    }

    /// QEMU 8.2's `virt` machine as `make run-aarch64` starts it (one
    /// Cortex-A72, 512 MiB), cut down to the bytes in use.
    const VIRT: &[u8] = include_bytes!("testdata/qemu-virt.dtb");
    /// The same machine with four CPUs, 2 GiB, a version 3 interrupt
    /// controller, and exception level 2.
    const VIRT_BIG: &[u8] = include_bytes!("testdata/qemu-virt-4cpu-gicv3-el2.dtb");

    #[test]
    fn qemus_machine_is_read() {
        let tree = Tree::new(VIRT).unwrap();
        assert_eq!(tree.reserved().count(), 0);
        let machine = Machine::read(&tree);
        assert_eq!(machine.model, Some("linux,dummy-virt"));
        assert_eq!(machine.memory(), [Range { start: 0x4000_0000, size: 0x2000_0000 }]);
        assert_eq!(machine.cpus(), [0]);
        assert_eq!(
            machine.serial,
            Some(Serial { registers: Range { start: 0x900_0000, size: 0x1000 }, clock_hz: Some(24_000_000), chosen: true })
        );
        assert_eq!(
            machine.interrupt_controller,
            Some(InterruptController {
                version: 2,
                distributor: Range { start: 0x800_0000, size: 0x1_0000 },
                per_cpu: Range { start: 0x801_0000, size: 0x1_0000 },
            })
        );
        assert_eq!(machine.timer_interrupts, [Some(29), Some(30), Some(27), Some(26)]);
        assert_eq!(machine.firmware_call, Some(FirmwareCall::Hvc));
        assert!(machine.nodes > 40, "{} nodes", machine.nodes);
    }

    #[test]
    fn qemus_bigger_machine_is_read() {
        let machine = Machine::read(&Tree::new(VIRT_BIG).unwrap());
        assert_eq!(machine.memory(), [Range { start: 0x4000_0000, size: 0x8000_0000 }]);
        assert_eq!(machine.cpus(), [0, 1, 2, 3]);
        let controller = machine.interrupt_controller.unwrap();
        assert_eq!(controller.version, 3);
        assert_eq!(controller.per_cpu, Range { start: 0x80A_0000, size: 0xF6_0000 });
        assert_eq!(machine.firmware_call, Some(FirmwareCall::Smc));
        assert_eq!(machine.serial.unwrap().registers.start, 0x900_0000);
    }

    #[test]
    fn a_damaged_tree_is_never_a_panic() {
        // Cut short at every length: the header may refuse it, or the
        // walk may end early - either is fine; panicking is not.
        for length in 0..VIRT.len() {
            if let Ok(tree) = Tree::new(&VIRT[..length]) {
                let _ = Machine::read(&tree);
            }
        }
        // The right length, with bytes changed: a few thousand different
        // ways, from a fixed seed so that a failure can be repeated.
        let mut seed: u32 = 0x1234_5678;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for _ in 0..4000 {
            let mut blob = VIRT.to_vec();
            for _ in 0..1 + next() % 4 {
                // Past the header's first numbers, or nearly every
                // change would only be refused at the door.
                let at = 8 + next() as usize % (blob.len() - 8);
                blob[at] = next() as u8;
            }
            if let Ok(tree) = Tree::new(&blob) {
                let machine = Machine::read(&tree);
                assert!(machine.nodes <= blob.len(), "more nodes than bytes");
                for node in tree.nodes() {
                    let _ = node.reg().count();
                    let _ = node.children().count();
                }
                let _ = tree.find("/cpus/cpu@0");
            }
        }
    }

    #[test]
    fn errors_say_what_was_found() {
        let text = format!("{}", Error::NotADeviceTree { found: 0x1234 });
        assert_eq!(text, "not a device tree: it begins 0x00001234, not 0xd00dfeed");
    }
}
