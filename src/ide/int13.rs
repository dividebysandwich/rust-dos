//! The IDE disks' side of the BIOS's INT 13h, as DOSBox-X's
//! `int13fakeio` and `int13fakev86io` have it. The BIOS's disk services
//! are the emulator's own and touch no ports, but a system that takes
//! over from them looks for a BIOS that drives its disks through the IDE
//! ports:
//!
//! * Windows 9x's ESDI_506.PDR, starting, traps the channels' ports in
//!   virtual-8086 mode and calls INT 13h to see which ports and drive the
//!   BIOS uses for each unit. Only then does it take the disk over with
//!   32-bit disk access; otherwise the disk stays in MS-DOS compatibility
//!   mode. When the ports are trapped, the service leaves the accesses a
//!   BIOS makes (select the drive, set the task file, READ SECTORS, wait,
//!   read the data, acknowledge the interrupt) for the processor to make
//!   on its way out (`bios::PORT_ACCESSES`), where Windows sees them.
//! * Windows for Workgroups' WDCTRL reads the task file back after an
//!   INT 13h read to check the BIOS left it pointing at the sector, and
//!   the disk reset after AH=00h: otherwise the service sets them at once.

use crate::bios::PortAccess;
use crate::cpu::Cpu;

/// What the BIOS did to a disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BiosAccess {
    /// AH=00h.
    Reset,
    /// A sector read: the address registers and drive/head register a
    /// BIOS leaves.
    Read { lba: [u8; 3], drivehead: u8 },
}

/// After INT 13h AH=00h on hard disk unit `unit`.
pub fn reset(cpu: &mut Cpu, unit: u8) {
    after_bios(cpu, unit, None);
}

/// After INT 13h read `count` sectors from sector `lba` of hard disk unit
/// `unit`, AH=02h by cylinder, head and sector (`chs`) or AH=42h by LBA.
pub fn read(cpu: &mut Cpu, unit: u8, lba: u64, count: usize, chs: bool) {
    for i in 0..count as u64 {
        after_bios(cpu, unit, Some((lba + i, chs)));
    }
}

/// What a BIOS driving the IDE disk of `unit` does for a reset (None) or
/// a sector read (its LBA, and whether the BIOS addresses it by CHS).
fn after_bios(cpu: &mut Cpu, unit: u8, read: Option<(u64, bool)>) {
    if cpu.bus.boot.is_none() || unit < 0x80 {
        return;
    }
    let Some(drive) = crate::boot::unit_drive(&cpu.bus, unit) else { return };
    let Some((id, slot, geometry)) = super::ChannelId::ALL
        .into_iter()
        .find_map(|id| cpu.bus.ide[id.index()].as_ref()?.disk_of(drive).map(|(slot, g)| (id, slot, g)))
    else {
        return;
    };
    let access = match read {
        None => BiosAccess::Reset,
        // By LBA, which has 28 bits, or by the cylinder, head and sector
        // of the disk's own geometry.
        Some((lba, false)) if lba < 1 << 28 => BiosAccess::Read {
            lba: [lba as u8, (lba >> 8) as u8, (lba >> 16) as u8],
            drivehead: 0xE0 | (slot as u8) << 4 | (lba >> 24) as u8,
        },
        Some((lba, _)) => {
            let per_cylinder = geometry.heads as u64 * geometry.sectors as u64;
            let (cylinder, rest) = (lba / per_cylinder, lba % per_cylinder);
            if cylinder > 0xFFFF {
                return;
            }
            let (head, sector) = (rest / geometry.sectors as u64, rest % geometry.sectors as u64 + 1);
            BiosAccess::Read {
                lba: [sector as u8, cylinder as u8, (cylinder >> 8) as u8],
                drivehead: 0xA0 | (slot as u8) << 4 | head as u8,
            }
        }
    };
    let base = id.base();
    // Ports a V86 monitor traps: the accesses for it to see.
    if cpu.v86() && cpu.check_io(base + 7, 1).is_err() {
        let devices: Vec<bool> = cpu.bus.ide[id.index()].as_ref().map_or(vec![], |c| c.devices.iter().map(Option::is_some).collect());
        let mut queue = vec![PortAccess::Faked(true)];
        for (s, present) in devices.into_iter().enumerate().take(slot + 1) {
            if present {
                queue.extend([PortAccess::In(base + 7), PortAccess::Out(base + 6, (s as u8) << 4)]);
            }
        }
        match access {
            BiosAccess::Reset => {
                queue.extend([PortAccess::In(base + 7), PortAccess::Out(base + 7, 0x08), PortAccess::In(base + 7)]);
            }
            BiosAccess::Read { lba, drivehead } => {
                // Interrupts off, as a BIOS has them for the data.
                queue.extend([
                    PortAccess::Cli,
                    PortAccess::In(base + 7),
                    PortAccess::Out(base + 6, drivehead),
                    PortAccess::In(base + 7),
                    PortAccess::Out(base + 2, 1),
                    PortAccess::Out(base + 3, lba[0]),
                    PortAccess::Out(base + 4, lba[1]),
                    PortAccess::Out(base + 5, lba[2]),
                    PortAccess::Out(base + 6, drivehead),
                    PortAccess::In(base + 7),
                    PortAccess::Out(base + 7, 0x20),
                    PortAccess::WaitWhile { port: id.alt(), mask: 0x80 },
                    PortAccess::In(base + 7),
                    PortAccess::InWords { port: base, count: 256 },
                    PortAccess::In(base + 7),
                ]);
            }
        }
        // The interrupt acknowledged: a specific EOI to the slave PIC.
        queue.extend([PortAccess::Out(0xA0, 0x60 + id.irq() - 8), PortAccess::Faked(false)]);
        cpu.bus.port_accesses.extend(queue);
        return;
    }
    cpu.bus.ide_bios_access(id, slot, access);
}
