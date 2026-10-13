//! The profiler: where the emulator's host time goes, by subsystem and by
//! the guest code it ran, and exact counts that explain why a spot is hot.
//! The debug server starts and reads it (`/api/profile`), and `--profile
//! FILE` runs one from startup to exit (docs/profiling.md).
//!
//! The emulation thread keeps one word up to date, `WORD`: the host zone it
//! is in now (`Zone`: the interpreter, translated code, a device's ports,
//! drawing the picture, ...) and the guest code it runs, CS:EIP with the
//! CPU mode. Each update is a relaxed store, with no clock read, so even
//! port handlers and the slow memory paths can mark themselves. A sampler
//! thread (`sampler`) reads the word a few thousand times a second and
//! counts what it finds: each read is a sample of where host time went.
//! The sampler only reads, so profiling can't change what the machine
//! does.
//!
//! While a profile runs, the bus counts what explains a hot spot in fixed
//! arrays (`Counters`): port accesses by port, BIOS and DOS services by
//! interrupt and function, interrupts delivered, accesses to memory-mapped
//! devices, and the recompiler's translations by page.
//!
//! The word is one for the process, and only the thread being profiled
//! writes it (`set_active`): tests that run machines on other threads
//! leave it alone.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub mod counters;
pub mod report;
#[cfg(not(target_arch = "wasm32"))]
pub mod sampler;
#[cfg(not(target_arch = "wasm32"))]
pub mod session;

pub use counters::Counters;

/// Where the emulation thread is: see `Loc`.
pub static WORD: AtomicU64 = AtomicU64::new(0);

/// The layout of `WORD`: EIP in bits 0-31, the CS selector (or real-mode
/// segment) in 32-47, the zone in 48-54, bit 55 set where the guest
/// location is known, the engine in 56-57 and the CPU mode in 58-59.
pub const CS_SHIFT: u32 = 32;
pub const ZONE_SHIFT: u32 = 48;
pub const ZONE_MASK: u64 = 0x7F << ZONE_SHIFT;
pub const GUEST: u64 = 1 << 55;
pub const ENGINE_SHIFT: u32 = 56;
pub const MODE_SHIFT: u32 = 58;

/// What runs the guest code: nothing (the front end's own work), the
/// interpreter or translated code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Engine {
    None = 0,
    Interp = 1,
    Jit = 2,
}

impl Engine {
    pub fn name(self) -> &'static str {
        match self {
            Engine::None => "none",
            Engine::Interp => "cpu.interp",
            Engine::Jit => "cpu.jit",
        }
    }
}

/// The CPU mode of the guest code: real mode, virtual-8086 mode, or
/// protected mode with a 16-bit or 32-bit code segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Mode {
    Real = 0,
    V86 = 1,
    Pm16 = 2,
    Pm32 = 3,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Real => "real",
            Mode::V86 => "v86",
            Mode::Pm16 => "pm16",
            Mode::Pm32 => "pm32",
        }
    }

    /// The mode of a CPU with `pe` (CR0.PE), `v86` (EFLAGS.VM) and a code
    /// segment that is 32-bit or not.
    #[inline(always)]
    pub fn of(pe: bool, v86: bool, code32: bool) -> Mode {
        match (pe, v86, code32) {
            (false, ..) => Mode::Real,
            (true, true, _) => Mode::V86,
            (true, false, false) => Mode::Pm16,
            (true, false, true) => Mode::Pm32,
        }
    }

    /// Whether CS holds a real-mode segment, whose base is it times 16.
    pub fn segmented(self) -> bool {
        matches!(self, Mode::Real | Mode::V86)
    }

    /// Protected mode outside virtual-8086 mode, where CS's low two bits
    /// are the privilege level.
    pub fn is_protected(self) -> bool {
        matches!(self, Mode::Pm16 | Mode::Pm32)
    }
}

macro_rules! zones {
    ($($zone:ident = $name:literal,)*) => {
        /// Where the emulation thread's time goes. The names group by the
        /// part before the first dot (`io`, `hle`, `mem`, ...).
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[repr(u8)]
        pub enum Zone {
            $($zone,)*
        }

        impl Zone {
            pub const ALL: &'static [Zone] = &[$(Zone::$zone,)*];

            pub fn name(self) -> &'static str {
                match self {
                    $(Zone::$zone => $name,)*
                }
            }
        }
    };
}

zones! {
    // The front end's frame, around the machine's batch.
    Frontend = "frontend",
    FrameWait = "frame.wait",
    FramePresent = "frame.present",
    FrameCompose = "frame.compose",
    Events = "frame.events",
    VgaRender = "vga.render",
    VoodooFrame = "voodoo.frame",
    AudioPump = "audio.pump",
    Rewind = "rewind",
    Debug = "debug",
    Ui = "ui",
    // The execution loop.
    Interp = "cpu.interp",
    Jit = "cpu.jit",
    JitDispatch = "dynrec.dispatch",
    Translate = "dynrec.translate",
    JitFallback = "dynrec.fallback",
    JitSlowMem = "dynrec.slow_mem",
    Timers = "timers",
    Irq = "irq",
    Shell = "shell",
    IdleProof = "idle.proof",
    // Memory that isn't plain RAM.
    MemVideo = "mem.video",
    MemUpper = "mem.upper",
    MemDevice = "mem.device",
    // Sound made as the machine runs.
    AudioSynth = "audio.synth",
    SbDma = "sb.dma",
    Gus = "gus.advance",
    // The 3D cards' work on the emulation thread.
    Voodoo = "voodoo",
    PowerVr = "powervr",
    // The emulator's BIOS, DOS and other services.
    HleInt08 = "hle.int08",
    HleInt09 = "hle.int09",
    HleInt10 = "hle.int10",
    HleInt13 = "hle.int13",
    HleInt15 = "hle.int15",
    HleInt16 = "hle.int16",
    HleInt1A = "hle.int1a",
    HleInt21 = "hle.int21",
    HleInt2F = "hle.int2f",
    HleInt33 = "hle.int33",
    HleInt67 = "hle.int67",
    HleOther = "hle.other",
    HleInline = "hle.inline",
    HleFar = "hle.far",
    Dpmi = "hle.dpmi",
    // Port accesses, by the device that has the port.
    IoVga = "io.vga",
    IoPic = "io.pic",
    IoPit = "io.pit",
    IoSpeaker = "io.speaker",
    IoKbc = "io.kbc",
    IoCmos = "io.cmos",
    IoDma = "io.dma",
    IoSb = "io.sb",
    IoOpl = "io.opl",
    IoGus = "io.gus",
    IoAwe = "io.awe",
    IoMpu = "io.mpu",
    IoTandy = "io.tandy",
    IoLptDac = "io.lpt_dac",
    IoPrinter = "io.printer",
    IoSerial = "io.serial",
    IoIde = "io.ide",
    IoNe2000 = "io.ne2000",
    IoJoystick = "io.joystick",
    IoS3 = "io.s3",
    IoEt4000 = "io.et4000",
    IoVerite = "io.verite",
    IoPci = "io.pci",
    IoSystem = "io.system",
    IoUnhandled = "io.unhandled",
}

/// The number of zones.
pub const ZONES: usize = Zone::ALL.len();
const _: () = assert!(ZONES <= 0x80);

impl Zone {
    /// The zone a sample's word has.
    pub fn from_u8(v: u8) -> Option<Zone> {
        Zone::ALL.get(v as usize).copied()
    }

    /// The name's part before the first dot: `io`, `hle`, `cpu`, ...
    pub fn group(self) -> &'static str {
        let name = self.name();
        name.split('.').next().unwrap_or(name)
    }

    /// The zone of a BIOS or DOS service at interrupt `vector`.
    pub fn hle(vector: u8) -> Zone {
        match vector {
            0x08 => Zone::HleInt08,
            0x09 => Zone::HleInt09,
            0x10 => Zone::HleInt10,
            0x13 => Zone::HleInt13,
            0x15 => Zone::HleInt15,
            0x16 => Zone::HleInt16,
            0x1A => Zone::HleInt1A,
            0x21 => Zone::HleInt21,
            0x2F => Zone::HleInt2F,
            0x33 => Zone::HleInt33,
            0x67 => Zone::HleInt67,
            _ => Zone::HleOther,
        }
    }

    /// The front end waiting for the next frame: the time the emulator has
    /// nothing to do, which busy shares leave out.
    pub fn is_wait(self) -> bool {
        self == Zone::FrameWait
    }

    /// A device's ports.
    pub fn is_io(self) -> bool {
        (Zone::IoVga as u8..=Zone::IoUnhandled as u8).contains(&(self as u8))
    }

    /// Memory that isn't plain RAM.
    pub fn is_mem(self) -> bool {
        matches!(self, Zone::MemVideo | Zone::MemUpper | Zone::MemDevice)
    }

    /// The zone by its name.
    pub fn named(name: &str) -> Option<Zone> {
        Zone::ALL.iter().copied().find(|z| z.name() == name)
    }
}

/// A sample: the word as the sampler read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Loc(pub u64);

impl Loc {
    /// The word for guest code at CS:EIP, run by `engine` in `mode`, in
    /// the engine's own zone.
    #[inline(always)]
    pub const fn guest(engine: Engine, mode: Mode, cs: u16, eip: u32) -> Loc {
        let zone = match engine {
            Engine::Jit => Zone::Jit,
            _ => Zone::Interp,
        };
        Loc(eip as u64
            | (cs as u64) << CS_SHIFT
            | (zone as u64) << ZONE_SHIFT
            | GUEST
            | (engine as u64) << ENGINE_SHIFT
            | (mode as u64) << MODE_SHIFT)
    }

    /// The word for the front end's own work, with no guest code.
    pub const fn host(zone: Zone) -> Loc {
        Loc((zone as u64) << ZONE_SHIFT)
    }

    pub fn zone(self) -> Zone {
        Zone::from_u8((self.0 >> ZONE_SHIFT) as u8 & 0x7F).unwrap_or(Zone::Frontend)
    }

    pub fn engine(self) -> Engine {
        match (self.0 >> ENGINE_SHIFT) & 3 {
            1 => Engine::Interp,
            2 => Engine::Jit,
            _ => Engine::None,
        }
    }

    pub fn mode(self) -> Mode {
        match (self.0 >> MODE_SHIFT) & 3 {
            0 => Mode::Real,
            1 => Mode::V86,
            2 => Mode::Pm16,
            _ => Mode::Pm32,
        }
    }

    /// CS and EIP, where the word has guest code.
    pub fn cs_eip(self) -> Option<(u16, u32)> {
        (self.0 & GUEST != 0).then_some(((self.0 >> CS_SHIFT) as u16, self.0 as u32))
    }

    /// The same place with EIP rounded down to a multiple of `bucket` (a
    /// power of two), to count code by routine rather than instruction.
    pub fn bucketed(self, bucket: u32) -> Loc {
        if self.0 & GUEST == 0 || bucket <= 1 {
            return self;
        }
        Loc(self.0 & !(bucket as u64 - 1))
    }

    /// The guest place alone: the engine, mode and CS:EIP, in the engine's
    /// zone.
    pub fn place(self) -> Loc {
        match self.cs_eip() {
            Some((cs, eip)) => Loc::guest(self.engine(), self.mode(), cs, eip),
            None => self,
        }
    }

    /// CS:EIP as the debugger writes addresses: four hex digits of offset
    /// in real and 16-bit code, eight in 32-bit code.
    pub fn addr_text(self) -> Option<String> {
        let (cs, eip) = self.cs_eip()?;
        Some(if self.mode() == Mode::Pm32 || eip > 0xFFFF {
            format!("{:04X}:{:08X}", cs, eip)
        } else {
            format!("{:04X}:{:04X}", cs, eip)
        })
    }
}

/// Whether a profile runs on any thread: while none does, marking a zone
/// costs one load of this and a branch.
static ANY_ACTIVE: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// Whether this thread is the one being profiled.
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
}

/// Whether this thread writes the word.
#[inline(always)]
pub fn active() -> bool {
    ANY_ACTIVE.load(Ordering::Relaxed) && this_thread()
}

#[cold]
#[inline(never)]
fn this_thread() -> bool {
    ACTIVE.with(Cell::get)
}

/// Make this thread the one being profiled, or not.
pub fn set_active(on: bool) {
    ACTIVE.with(|a| a.set(on));
    ANY_ACTIVE.store(on, Ordering::Relaxed);
    if !on {
        WORD.store(0, Ordering::Relaxed);
    }
    // A profile charges nothing to an entry or a thunk of the one before.
    ENTRY.store(0, Ordering::Relaxed);
    drop_thunk_now();
}

/// Put the emulation thread in `zone` doing the front end's work, with no
/// guest code.
#[inline(always)]
pub fn set_host(zone: Zone) {
    if ANY_ACTIVE.load(Ordering::Relaxed) {
        set_host_now(zone);
    }
}

#[cold]
#[inline(never)]
fn set_host_now(zone: Zone) {
    if this_thread() {
        WORD.store(Loc::host(zone).0, Ordering::Relaxed);
    }
}

/// Note guest code at CS:EIP about to run on `engine`. The caller checks
/// that a profile runs (the execution loop once a batch).
#[inline(always)]
pub fn set_guest(loc: Loc) {
    WORD.store(loc.0, Ordering::Relaxed);
}

/// Keep the guest code but move to `zone`: a translated block went back
/// to the execution loop, say.
#[inline(always)]
pub fn set_zone(zone: Zone) {
    if ANY_ACTIVE.load(Ordering::Relaxed) {
        set_zone_now(zone);
    }
}

#[cold]
#[inline(never)]
fn set_zone_now(zone: Zone) {
    if this_thread() {
        let now = WORD.load(Ordering::Relaxed);
        WORD.store(now & !ZONE_MASK | (zone as u64) << ZONE_SHIFT, Ordering::Relaxed);
    }
}

/// `set_zone` on the thread being profiled, as the caller knows (its
/// machine has the profile's counters): no check of the thread.
#[inline(always)]
pub fn set_zone_here(zone: Zone) {
    let now = WORD.load(Ordering::Relaxed);
    WORD.store(now & !ZONE_MASK | (zone as u64) << ZONE_SHIFT, Ordering::Relaxed);
}

/// A zone for as long as it lives, inside whatever the thread was doing:
/// the guest code stays, and the zone before comes back when it drops, so
/// nested zones count only their own time.
/// Nothing happens on a thread that isn't profiled.
#[must_use]
pub struct ZoneGuard {
    prev: Option<u64>,
}

/// Enter `zone` until the guard drops.
#[inline(always)]
pub fn zone(zone: Zone) -> ZoneGuard {
    if ANY_ACTIVE.load(Ordering::Relaxed) {
        return enter(zone);
    }
    ZoneGuard { prev: None }
}

#[cold]
#[inline(never)]
fn enter(zone: Zone) -> ZoneGuard {
    if !this_thread() {
        return ZoneGuard { prev: None };
    }
    let prev = WORD.load(Ordering::Relaxed);
    WORD.store(prev & !ZONE_MASK | (zone as u64) << ZONE_SHIFT, Ordering::Relaxed);
    ZoneGuard { prev: Some(prev) }
}

impl Drop for ZoneGuard {
    #[inline(always)]
    fn drop(&mut self) {
        if let Some(prev) = self.prev {
            WORD.store(prev, Ordering::Relaxed);
        }
    }
}

/// Run `f` in `zone`. Out of line, so a hot loop that calls it only in
/// its copy for profiling (a const generic) keeps no guard in the other.
#[inline(never)]
pub fn within<R>(zone: Zone, f: impl FnOnce() -> R) -> R {
    let _zone = self::zone(zone);
    f()
}

/// The word now (the sampler's read, and the tests').
pub fn now() -> Loc {
    Loc(WORD.load(Ordering::Relaxed))
}

/// The place that last entered ring 0 from an outer level (ring 3 or
/// virtual-8086 mode), and how: an `Entry`, or 0 before the first. A ring-0
/// sample is charged to it, since the CPU gets to ring 0 only through such
/// an entry.
pub static ENTRY: AtomicU64 = AtomicU64::new(0);

/// The ring-3 32-bit code that last switched to 16-bit ring-3 code: the
/// place of the switch (a `Loc`), then up to three return addresses up its
/// EBP chain, the nearest first; all 0 once 16-bit code switched back to
/// 32-bit code. Under Windows 9x this is a Win32 program calling USER, GDI
/// or KERNEL through KERNEL32's thunk, whose callers need EBP frames, so
/// the return addresses lead back to the program. The 16-bit samples, and
/// the ring-0 ones the 16-bit code enters, are charged to it.
pub static THUNK: [AtomicU64; THUNK_FRAMES] = [const { AtomicU64::new(0) }; THUNK_FRAMES];

/// The words of `THUNK`.
pub const THUNK_FRAMES: usize = 4;

/// What `THUNK` holds, as the sampler read it.
pub type Thunk = [u64; THUNK_FRAMES];

/// `THUNK` now.
pub fn thunk_now() -> Thunk {
    std::array::from_fn(|i| THUNK[i].load(Ordering::Relaxed))
}

/// How the CPU entered ring 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Cause {
    /// A far call or jump through a call gate (vector 0), or SYSENTER
    /// (vector 1).
    Gate = 0,
    /// INT n, INT3 or INTO.
    Int = 1,
    /// An exception.
    Exception = 2,
    /// A hardware interrupt.
    Irq = 3,
}

/// An entry to ring 0: the outer place (EIP in bits 0-31, CS in 32-47), the
/// vector in 48-55, the `Cause` in 56-57, the outer place's `Mode` in 58-59
/// and bit 60 set. EIP is the return address: the faulting instruction for
/// a fault, the next one for the others.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Entry(pub u64);

impl Entry {
    const SET: u64 = 1 << 60;

    pub fn new(mode: Mode, cs: u16, eip: u32, cause: Cause, vector: u8) -> Entry {
        Entry(eip as u64 | (cs as u64) << 32 | (vector as u64) << 48 | (cause as u64) << 56 | (mode as u64) << MODE_SHIFT | Self::SET)
    }

    /// The entry a word of `ENTRY` holds, if any.
    pub fn of(word: u64) -> Option<Entry> {
        (word & Self::SET != 0).then_some(Entry(word))
    }

    pub fn cause(self) -> Cause {
        match (self.0 >> 56) & 3 {
            0 => Cause::Gate,
            1 => Cause::Int,
            2 => Cause::Exception,
            _ => Cause::Irq,
        }
    }

    pub fn vector(self) -> u8 {
        (self.0 >> 48) as u8
    }

    /// The outer place, as a sample of it would be.
    pub fn place(self) -> Loc {
        let mode = Loc(self.0).mode();
        Loc::guest(Engine::None, mode, (self.0 >> 32) as u16, self.0 as u32)
    }

    /// The same entry with the place's EIP rounded down to a multiple of
    /// `bucket`.
    pub fn bucketed(self, bucket: u32) -> Entry {
        if bucket <= 1 {
            return self;
        }
        Entry(self.0 & !(bucket as u64 - 1))
    }

    /// How it entered: `int 30h`, `#GP`, `irq int 50h`, `call gate`.
    pub fn via(self) -> String {
        match self.cause() {
            Cause::Gate if self.vector() == 1 => "sysenter".into(),
            Cause::Gate => "call gate".into(),
            Cause::Int => format!("int {:02X}h", self.vector()),
            Cause::Exception => crate::cpu::fault::exception_name(self.vector()).into(),
            Cause::Irq => format!("irq int {:02X}h", self.vector()),
        }
    }
}

/// Whether a profile runs on any thread: the check before noting an entry
/// or a thunk, a load and a branch.
#[inline(always)]
pub fn any_active() -> bool {
    ANY_ACTIVE.load(Ordering::Relaxed)
}

/// Note an entry to ring 0 from `mode` at CS:EIP. The caller checks
/// `any_active` first.
#[cold]
#[inline(never)]
pub fn note_entry(mode: Mode, cs: u16, eip: u32, cause: Cause, vector: u8) {
    if this_thread() {
        ENTRY.store(Entry::new(mode, cs, eip, cause, vector).0, Ordering::Relaxed);
    }
}

/// Note a transfer in ring 3 from 32-bit code at CS:EIP (the return
/// address) to 16-bit code, called from `returns` (up its EBP chain, the
/// nearest first). The caller checks `any_active` first.
#[cold]
#[inline(never)]
pub fn note_thunk(cs: u16, eip: u32, returns: &[u32]) {
    if !this_thread() {
        return;
    }
    THUNK[0].store(Loc::guest(Engine::None, Mode::Pm32, cs, eip).0, Ordering::Relaxed);
    for (i, word) in THUNK.iter().enumerate().skip(1) {
        let at = returns.get(i - 1).map_or(0, |&ret| Loc::guest(Engine::None, Mode::Pm32, cs, ret).0);
        word.store(at, Ordering::Relaxed);
    }
}

/// Note a transfer in ring 3 from 16-bit code back to 32-bit code. The
/// caller checks `any_active` first.
#[cold]
#[inline(never)]
pub fn drop_thunk() {
    if this_thread() {
        drop_thunk_now();
    }
}

fn drop_thunk_now() {
    for word in &THUNK {
        word.store(0, Ordering::Relaxed);
    }
}

/// What a sample is charged to: the entry (`ENTRY`) for ring-0 code, and
/// the Win32 caller (`THUNK`) for 16-bit ring-3 code and for ring-0 code
/// that 16-bit code entered. `None` for the other samples.
pub fn chain(loc: Loc, entry: u64, thunk: Thunk) -> Option<(u64, Thunk)> {
    let (cs, _) = loc.cs_eip()?;
    match (loc.mode(), cs & 3) {
        (Mode::Pm16 | Mode::Pm32, 0) => {
            let e = Entry::of(entry)?;
            let thunk = if e.place().mode() == Mode::Pm16 { thunk } else { [0; THUNK_FRAMES] };
            Some((entry, thunk))
        }
        (Mode::Pm16, 3) if thunk[0] != 0 => Some((0, thunk)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_guest_word_keeps_its_fields() {
        let loc = Loc::guest(Engine::Jit, Mode::Pm32, 0x0168, 0x0012_ABCD);
        assert_eq!(loc.cs_eip(), Some((0x0168, 0x0012_ABCD)));
        assert_eq!(loc.engine(), Engine::Jit);
        assert_eq!(loc.mode(), Mode::Pm32);
        assert_eq!(loc.zone(), Zone::Jit);
        assert_eq!(loc.addr_text().as_deref(), Some("0168:0012ABCD"));
        let real = Loc::guest(Engine::Interp, Mode::Real, 0x1234, 0x0100);
        assert_eq!(real.addr_text().as_deref(), Some("1234:0100"));
        assert_eq!(real.zone(), Zone::Interp);
        assert_eq!(Loc::host(Zone::VgaRender).cs_eip(), None);
        assert_eq!(Loc::host(Zone::VgaRender).engine(), Engine::None);
    }

    #[test]
    fn buckets_round_eip_down() {
        let loc = Loc::guest(Engine::Interp, Mode::Real, 0x1000, 0x1237);
        assert_eq!(loc.bucketed(16).cs_eip(), Some((0x1000, 0x1230)));
        assert_eq!(loc.bucketed(1), loc);
        assert_eq!(Loc::host(Zone::FrameWait).bucketed(256), Loc::host(Zone::FrameWait));
    }

    #[test]
    fn zone_names_are_unique_and_found_again() {
        let mut names: Vec<_> = Zone::ALL.iter().map(|z| z.name()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), ZONES);
        for &z in Zone::ALL {
            assert_eq!(Zone::named(z.name()), Some(z));
            assert_eq!(Zone::from_u8(z as u8), Some(z));
        }
        assert_eq!(Zone::IoVga.group(), "io");
        assert_eq!(Zone::Rewind.group(), "rewind");
    }

    #[test]
    fn an_entry_keeps_its_fields() {
        let e = Entry::new(Mode::Pm16, 0x003F, 0x03D2, Cause::Int, 0x30);
        assert_eq!(Entry::of(e.0), Some(e));
        assert_eq!(Entry::of(0), None);
        assert_eq!(e.place().cs_eip(), Some((0x003F, 0x03D2)));
        assert_eq!(e.place().mode(), Mode::Pm16);
        assert_eq!((e.cause(), e.vector(), e.via()), (Cause::Int, 0x30, "int 30h".to_string()));
        assert_eq!(Entry::new(Mode::V86, 0x1234, 5, Cause::Exception, 13).via(), "#GP");
        assert_eq!(Entry::new(Mode::Pm32, 0x137, 5, Cause::Irq, 0x50).via(), "irq int 50h");
        assert_eq!(Entry::new(Mode::Pm32, 0x137, 5, Cause::Gate, 1).via(), "sysenter");
        let b = Entry::new(Mode::Pm32, 0x137, 0x1000_A08F, Cause::Int, 0x2E).bucketed(0x10000);
        assert_eq!(b.place().cs_eip(), Some((0x137, 0x1000_0000)));
        assert_eq!(b.via(), "int 2Eh", "a bucket keeps how it entered");
    }

    #[test]
    fn samples_are_charged_by_privilege_level_and_code_size() {
        let ring0 = Loc::guest(Engine::Jit, Mode::Pm32, 0x0028, 0xC000_13AC);
        let win16 = Loc::guest(Engine::Jit, Mode::Pm16, 0x003F, 0x03D0);
        let win32 = Loc::guest(Engine::Jit, Mode::Pm32, 0x0137, 0x1000_A08F);
        let from16 = Entry::new(Mode::Pm16, 0x003F, 0x03D2, Cause::Int, 0x30).0;
        let from32 = Entry::new(Mode::Pm32, 0x0137, 0x1000_A091, Cause::Exception, 13).0;
        let thunk = [Loc::guest(Engine::None, Mode::Pm32, 0x0137, 0xBFF7_13D4).0, 0, 0, 0];
        assert_eq!(chain(ring0, from32, thunk), Some((from32, [0; 4])), "a thunk counts only for 16-bit callers");
        assert_eq!(chain(ring0, from16, thunk), Some((from16, thunk)));
        assert_eq!(chain(ring0, 0, thunk), None, "no entry yet");
        assert_eq!(chain(win16, from16, thunk), Some((0, thunk)));
        assert_eq!(chain(win16, from16, [0; 4]), None, "16-bit code nobody called through a thunk");
        assert_eq!(chain(win32, from32, thunk), None, "32-bit ring 3 code is its own");
        assert_eq!(chain(Loc::host(Zone::FrameWait), from32, thunk), None);
        let v86 = Loc::guest(Engine::Interp, Mode::V86, 0x0070, 0x046E);
        assert_eq!(chain(v86, from32, thunk), None, "CS in V86 mode is a segment, not a privilege level");
    }

    #[test]
    fn a_thunk_is_noted_going_to_16_bit_code_and_dropped_coming_back() {
        let _one = crate::profile::session::tests::ONE_PROFILE.lock().unwrap_or_else(|e| e.into_inner());
        set_active(true);
        note_thunk(0x0137, 0xBFF7_13D9, &[0xBFF5_1000, 0x0040_2000]);
        let t = thunk_now();
        assert_eq!(Loc(t[0]).cs_eip(), Some((0x0137, 0xBFF7_13D9)));
        assert_eq!(Loc(t[1]).cs_eip(), Some((0x0137, 0xBFF5_1000)));
        assert_eq!(Loc(t[2]).cs_eip(), Some((0x0137, 0x0040_2000)));
        assert_eq!(t[3], 0);
        note_thunk(0x0137, 0xBFF7_13D9, &[]);
        assert_eq!(thunk_now()[1], 0, "a thunk without frames leaves none of the last one's");
        drop_thunk();
        assert_eq!(thunk_now(), [0; 4]);
        note_entry(Mode::Pm32, 0x0137, 0x1000, Cause::Int, 0x30);
        assert_ne!(ENTRY.load(Ordering::Relaxed), 0);
        set_active(false);
        assert_eq!(ENTRY.load(Ordering::Relaxed), 0, "the next profile starts without it");
    }

    #[test]
    fn guards_nest_and_restore() {
        let _one = crate::profile::session::tests::ONE_PROFILE.lock().unwrap_or_else(|e| e.into_inner());
        set_active(true);
        set_guest(Loc::guest(Engine::Interp, Mode::Real, 0x2000, 0x10));
        {
            let _io = zone(Zone::IoVga);
            assert_eq!(now().zone(), Zone::IoVga);
            assert_eq!(now().cs_eip(), Some((0x2000, 0x10)));
            {
                let _sb = zone(Zone::SbDma);
                assert_eq!(now().zone(), Zone::SbDma);
            }
            assert_eq!(now().zone(), Zone::IoVga);
        }
        assert_eq!(now().zone(), Zone::Interp);
        set_zone(Zone::JitDispatch);
        assert_eq!(now().zone(), Zone::JitDispatch);
        assert_eq!(now().engine(), Engine::Interp);
        set_active(false);
        let _idle = zone(Zone::IoPic);
        assert_eq!(now(), Loc(0), "a thread not profiled leaves the word alone");
    }
}
