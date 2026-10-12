//! The profiler: where the emulator's host time goes, by subsystem and by
//! the guest code it ran, and exact counts that explain why a spot is hot.
//! The debug server starts and reads it (`/api/profile`, see
//! docs/profiling.md).
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
//! interrupt and AH, interrupts delivered, accesses to memory-mapped
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
    MemRom = "mem.rom",
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

    /// A device's ports.
    pub fn is_io(self) -> bool {
        (Zone::IoVga as u8..=Zone::IoUnhandled as u8).contains(&(self as u8))
    }

    /// Memory that isn't plain RAM.
    pub fn is_mem(self) -> bool {
        matches!(self, Zone::MemVideo | Zone::MemRom | Zone::MemDevice)
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
