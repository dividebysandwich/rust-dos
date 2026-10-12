//! What the bus counts while a profile runs: plain increments in arrays
//! made when the profile starts, with no clock reads, lookups by key or
//! allocation on the emulation thread.

use std::cell::Cell;

use super::{ZONES, Zone, ZoneGuard};

/// Kinds of memory accesses that aren't plain RAM (`MemKind`).
pub const MEM_KINDS: usize = 3;

/// Memory that isn't plain RAM: the video memory windows at A0000h-BFFFFh,
/// the ROM and upper memory area, and devices above RAM (linear frame
/// buffers, the 3D cards, MMIO).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemKind {
    Video = 0,
    Rom = 1,
    Device = 2,
}

impl MemKind {
    pub const ALL: [MemKind; MEM_KINDS] = [MemKind::Video, MemKind::Rom, MemKind::Device];

    /// The kind of address `addr`, which isn't plain RAM.
    #[inline]
    pub fn of(addr: usize) -> MemKind {
        if addr < 0xC0000 {
            MemKind::Video
        } else if addr < 0x10_0000 {
            MemKind::Rom
        } else {
            MemKind::Device
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            MemKind::Video => "video",
            MemKind::Rom => "rom",
            MemKind::Device => "device",
        }
    }

    pub fn zone(self) -> Zone {
        match self {
            MemKind::Video => Zone::MemVideo,
            MemKind::Rom => Zone::MemRom,
            MemKind::Device => Zone::MemDevice,
        }
    }
}

/// The emulator's services by the kind of trap (`exec::service_trap`)
/// besides the interrupt handlers: inline services (FE 39), far-call
/// services (FE 3A) and the DPMI host's entries (FE 3B).
pub const SERVICE_KINDS: usize = 3;

/// A page of guest code: translations of blocks in it, and blocks thrown
/// away because their bytes changed before they ran (stale) or as they ran
/// (self-modifying code).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CodePage {
    pub translated: u32,
    pub stale: u32,
    pub smc: u32,
}

/// The counts.
pub struct Counters {
    /// Port reads and writes by port (a wide access counts once, at its
    /// first port), and the zone of each port: its device.
    pub port_reads: Box<[u64]>,
    pub port_writes: Box<[u64]>,
    pub port_zone: Box<[u8]>,
    /// Calls of the emulator's interrupt handlers, by vector and AH
    /// (`vector << 8 | ah`).
    pub hle: Box<[u64]>,
    /// The other services, by kind and number.
    pub services: [[u64; 256]; SERVICE_KINDS],
    /// Hardware interrupts delivered, by IRQ line.
    pub irqs: [u64; 16],
    /// Exceptions the CPU raised, by vector.
    pub exceptions: [u64; 32],
    /// Reads and writes of memory that isn't plain RAM, by `MemKind` (the
    /// bus reads it through `&self`).
    pub mem_reads: [Cell<u64>; MEM_KINDS],
    pub mem_writes: [Cell<u64>; MEM_KINDS],
    /// The recompiler's work by 4 KB page of physical memory.
    pub code_pages: Box<[CodePage]>,
}

impl Counters {
    /// Empty counts for a machine with `ram_len` bytes of RAM, whose
    /// ports are where `zone_of` says.
    pub fn new(ram_len: usize, zone_of: impl Fn(u16) -> Zone) -> Box<Self> {
        Box::new(Counters {
            port_reads: vec![0; 0x10000].into_boxed_slice(),
            port_writes: vec![0; 0x10000].into_boxed_slice(),
            port_zone: (0..=0xFFFFu16).map(|p| zone_of(p) as u8).collect(),
            hle: vec![0; 0x10000].into_boxed_slice(),
            services: [[0; 256]; SERVICE_KINDS],
            irqs: [0; 16],
            exceptions: [0; 32],
            mem_reads: Default::default(),
            mem_writes: Default::default(),
            code_pages: vec![CodePage::default(); ram_len.div_ceil(0x1000)].into_boxed_slice(),
        })
    }

    /// The zone of port `port`.
    #[inline(always)]
    pub fn port_zone(&self, port: u16) -> Zone {
        Zone::from_u8(self.port_zone[port as usize]).unwrap_or(Zone::IoUnhandled)
    }

    /// Count a port access and enter its device's zone. An access made
    /// of others (a word to ports that take bytes) counts once, as the
    /// outermost.
    #[inline(always)]
    pub fn port(&mut self, port: u16, write: bool) -> ZoneGuard {
        if super::now().zone().is_io() {
            return ZoneGuard { prev: None };
        }
        if write {
            self.port_writes[port as usize] += 1;
        } else {
            self.port_reads[port as usize] += 1;
        }
        super::zone(self.port_zone(port))
    }

    /// Count an access of memory that isn't plain RAM, as the outermost
    /// of the accesses it is made of (a doubleword written a byte at a
    /// time).
    #[inline(always)]
    pub fn mem(&self, addr: usize, write: bool) -> ZoneGuard {
        if super::now().zone().is_mem() {
            return ZoneGuard { prev: None };
        }
        let kind = MemKind::of(addr);
        let count = if write { &self.mem_writes[kind as usize] } else { &self.mem_reads[kind as usize] };
        count.set(count.get() + 1);
        super::zone(kind.zone())
    }

    /// The page of physical address `phys`, if RAM has it.
    pub fn code_page(&mut self, phys: u32) -> Option<&mut CodePage> {
        self.code_pages.get_mut(phys as usize >> 12)
    }

    /// The port accesses by zone: reads and writes.
    pub fn ports_by_zone(&self) -> [(u64, u64); ZONES] {
        let mut by = [(0, 0); ZONES];
        for port in 0..0x10000 {
            let (r, w) = (self.port_reads[port], self.port_writes[port]);
            if r | w != 0 {
                let z = &mut by[self.port_zone[port] as usize];
                z.0 += r;
                z.1 += w;
            }
        }
        by
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_count_by_port_and_device() {
        let _one = crate::profile::session::tests::ONE_PROFILE.lock().unwrap_or_else(|e| e.into_inner());
        crate::profile::set_active(true);
        let mut c = Counters::new(1 << 20, |p| if (0x3C0..=0x3DF).contains(&p) { Zone::IoVga } else { Zone::IoUnhandled });
        for _ in 0..3 {
            drop(c.port(0x3DA, false));
        }
        drop(c.port(0x3D4, true));
        drop(c.port(0x80, true));
        assert_eq!(c.port_reads[0x3DA], 3);
        assert_eq!(c.port_writes[0x3D4], 1);
        let by = c.ports_by_zone();
        assert_eq!(by[Zone::IoVga as usize], (3, 1));
        assert_eq!(by[Zone::IoUnhandled as usize], (0, 1));
        assert_eq!(c.code_pages.len(), 256);
        {
            let _outer = c.port(0x3C4, true);
            drop(c.port(0x3C5, true));
        }
        assert_eq!((c.port_writes[0x3C4], c.port_writes[0x3C5]), (1, 0), "a word counts once");
        {
            let _outer = c.mem(0xA0000, true);
            drop(c.mem(0xA0001, true));
        }
        assert_eq!(c.mem_writes[MemKind::Video as usize].get(), 1);
        crate::profile::set_active(false);
    }

    #[test]
    fn memory_kinds_by_address() {
        assert_eq!(MemKind::of(0xA0000), MemKind::Video);
        assert_eq!(MemKind::of(0xB8000), MemKind::Video);
        assert_eq!(MemKind::of(0xF0000), MemKind::Rom);
        assert_eq!(MemKind::of(0xE000_0000), MemKind::Device);
    }
}
