//! How long disk access takes, as DOSBox Staging's `hard_disk_speed` and
//! `floppy_disk_speed` settings have it, and the settings for the noises
//! the drives make (see `disknoise`).
//!
//! DOS and BIOS services do their disk work at once and charge the time it
//! would have taken (`DiskIo::charge`). When the service is done,
//! `exec::service_trap` holds its return back for that long in emulated
//! time (`begin_wait`), with interrupts on: the machine keeps running, the
//! program waits.

use crate::cpu::{Cpu, CpuFlags};
use crate::disk::DriveKind;
use crate::timer::PIT_HZ;

/// What DOSBox charges for the file calls that move no data.
pub const CREATE_BYTES: u32 = 2048;
pub const OPEN_BYTES: u32 = 1024;
pub const SEEK_BYTES: u32 = 512;

/// A drive's speed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiskSpeed {
    /// No slowdown.
    #[default]
    Maximum,
    Fast,
    Medium,
    Slow,
}

impl DiskSpeed {
    pub const ALL: [DiskSpeed; 4] = [DiskSpeed::Maximum, DiskSpeed::Fast, DiskSpeed::Medium, DiskSpeed::Slow];

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|speed| speed.name().eq_ignore_ascii_case(s.trim()))
    }

    pub fn name(self) -> &'static str {
        match self {
            DiskSpeed::Maximum => "maximum",
            DiskSpeed::Fast => "fast",
            DiskSpeed::Medium => "medium",
            DiskSpeed::Slow => "slow",
        }
    }

    /// Transfer rate in KB per second: a mid-1990s, early 1990s and 1980s
    /// hard disk; an extra-high, high and double density floppy. None for
    /// no slowdown.
    pub fn kb_per_second(self, class: DiskClass) -> Option<u64> {
        match (class, self) {
            (_, DiskSpeed::Maximum) => None,
            (DiskClass::HardDisk, DiskSpeed::Fast) => Some(15_000),
            (DiskClass::HardDisk, DiskSpeed::Medium) => Some(2_500),
            (DiskClass::HardDisk, DiskSpeed::Slow) => Some(600),
            (DiskClass::Floppy, DiskSpeed::Fast) => Some(120),
            (DiskClass::Floppy, DiskSpeed::Medium) => Some(60),
            (DiskClass::Floppy, DiskSpeed::Slow) => Some(30),
        }
    }

    /// The setting as the settings window shows it: "slow (~600 kB/s)".
    pub fn describe(self, class: DiskClass) -> String {
        match self.kb_per_second(class) {
            None => self.name().to_string(),
            Some(kb) if kb >= 1000 => format!("{} (~{} MB/s)", self.name(), kb as f64 / 1000.0),
            Some(kb) => format!("{} (~{} kB/s)", self.name(), kb),
        }
    }
}

/// Which disk noises a drive makes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NoiseMode {
    #[default]
    Off,
    /// The heads moving, not the disk spinning.
    SeekOnly,
    On,
}

impl NoiseMode {
    pub const ALL: [NoiseMode; 3] = [NoiseMode::Off, NoiseMode::SeekOnly, NoiseMode::On];

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "on" | "true" | "yes" => Some(NoiseMode::On),
            "seek-only" => Some(NoiseMode::SeekOnly),
            "off" | "false" | "no" => Some(NoiseMode::Off),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            NoiseMode::Off => "off",
            NoiseMode::SeekOnly => "seek-only",
            NoiseMode::On => "on",
        }
    }
}

/// The two kinds of drive that are slow and make noises.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskClass {
    Floppy,
    HardDisk,
}

impl DiskClass {
    /// CD-ROMs and the drives held in memory take no time.
    pub fn of(kind: DriveKind) -> Option<Self> {
        match kind {
            DriveKind::Floppy => Some(DiskClass::Floppy),
            DriveKind::HardDisk => Some(DiskClass::HardDisk),
            DriveKind::CdRom | DriveKind::Virtual => None,
        }
    }
}

/// The disk speed and noise settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiskSettings {
    pub hard_disk_speed: DiskSpeed,
    pub floppy_disk_speed: DiskSpeed,
    pub hard_disk_noise: NoiseMode,
    pub floppy_disk_noise: NoiseMode,
}

impl DiskSettings {
    pub fn speed(&self, class: DiskClass) -> DiskSpeed {
        match class {
            DiskClass::Floppy => self.floppy_disk_speed,
            DiskClass::HardDisk => self.hard_disk_speed,
        }
    }
}

/// The time the disk work of the service running now takes.
#[derive(Debug, Default)]
pub struct DiskIo {
    pub settings: DiskSettings,
    pending_ns: u64,
}

impl DiskIo {
    /// Charge moving `bytes` on a `class` drive. Returns the time charged
    /// so far, in nanoseconds.
    pub fn charge(&mut self, class: DiskClass, bytes: u32) -> u64 {
        if let Some(kb) = self.settings.speed(class).kb_per_second(class) {
            self.pending_ns += bytes as u64 * 1_000_000_000 / (kb * 1024);
        }
        self.pending_ns
    }

    pub fn take_pending(&mut self) -> u64 {
        std::mem::take(&mut self.pending_ns)
    }

    pub fn clear(&mut self) {
        self.pending_ns = 0;
    }
}

/// Instead of returning from the HLE service `vector`, wait `ns` of emulated
/// time first with interrupts on, in the ROM's wait loop (`wait`). The
/// service's return frame stays on the stack under the deadline and the
/// vector, so a disk service that an interrupt handler calls meanwhile
/// waits on its own.
pub fn begin_wait(cpu: &mut Cpu, vector: u8, ns: u64) {
    let until = cpu.bus.clock.now_ticks() + ns * PIT_HZ / 1_000_000_000;
    cpu.push(vector as u16);
    for shift in [48, 32, 16, 0] {
        cpu.push((until >> shift) as u16);
    }
    cpu.set_cs(0xF000);
    cpu.set_ip(crate::bios::IO_WAIT);
    cpu.set_cpu_flag(CpuFlags::IF, true);
}

/// Make the code at CS:IP wait `ns` of emulated time before it runs, as a
/// program the shell loaded from a slow disk does.
pub fn wait_before(cpu: &mut Cpu, ns: u64) {
    let flags = cpu.get_cpu_flags().bits() as u16;
    cpu.push(flags);
    cpu.push(cpu.cs());
    cpu.push(cpu.ip());
    begin_wait(cpu, 0x21, ns);
}

/// The ROM wait loop (`FE 39 SERVICE_IO_WAIT`), entered with IP past it:
/// until the deadline on the stack, go round again after the interrupts
/// that come; then return from the service as it would have.
pub fn wait(cpu: &mut Cpu) {
    // The deadline's low word is on top.
    let words = [cpu.pop(), cpu.pop(), cpu.pop(), cpu.pop()];
    let until = words.iter().rev().fold(0u64, |acc, &w| acc << 16 | w as u64);
    if cpu.bus.clock.now_ticks() < until {
        for &w in words.iter().rev() {
            cpu.push(w);
        }
        cpu.set_ip(cpu.ip().wrapping_sub(3));
        cpu.idle = true;
        let clock = &mut cpu.bus.clock;
        clock.deadline = clock.deadline.min(clock.icount_at(until));
    } else {
        let vector = cpu.pop() as u8;
        crate::interrupts::return_from_hle(cpu, vector);
    }
}

// The delay a disk access still owes; how long they take is the
// configuration's.
crate::state_fields!(DiskIo { pending_ns } skip { settings });


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_parse_and_rates() {
        assert_eq!(DiskSpeed::parse(" Slow "), Some(DiskSpeed::Slow));
        assert_eq!(DiskSpeed::parse("warp"), None);
        assert_eq!(NoiseMode::parse("seek-only"), Some(NoiseMode::SeekOnly));
        assert_eq!(NoiseMode::parse("true"), Some(NoiseMode::On));
        assert_eq!(DiskSpeed::Medium.kb_per_second(DiskClass::Floppy), Some(60));
        assert_eq!(DiskSpeed::Fast.describe(DiskClass::HardDisk), "fast (~15 MB/s)");
        assert_eq!(DiskSpeed::Slow.describe(DiskClass::Floppy), "slow (~30 kB/s)");
        assert_eq!(DiskSpeed::Medium.describe(DiskClass::HardDisk), "medium (~2.5 MB/s)");
    }

    #[test]
    fn time_adds_up_until_taken() {
        let mut io = DiskIo::default();
        assert_eq!(io.charge(DiskClass::Floppy, 512), 0);
        io.settings.floppy_disk_speed = DiskSpeed::Slow;
        // 30 KB/s: a 512-byte sector takes 1/60 s.
        assert_eq!(io.charge(DiskClass::Floppy, 512), 16_666_666);
        assert_eq!(io.charge(DiskClass::HardDisk, 1_000_000), 16_666_666);
        assert_eq!(io.charge(DiskClass::Floppy, 512), 33_333_332);
        assert_eq!(io.take_pending(), 33_333_332);
        assert_eq!(io.take_pending(), 0);
    }
}
