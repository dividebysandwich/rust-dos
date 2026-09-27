//! INT 25h/26h — DOS absolute disk read and write.
//!
//! AL = drive (0 = A:), CX = sectors, DX = first sector and DS:BX = buffer;
//! or, for volumes over 32 MB, CX = FFFFh and DS:BX -> a packet of the first
//! sector (dword), the count (word) and the buffer (far pointer). Sectors
//! are numbered from the start of the drive's volume. The calls return
//! with the caller's flags still on the stack (see `return_from_hle`),
//! CF set and AX = error on failure: AH a BIOS status, AL a DOS critical
//! error code.
//!
//! Drives mounted from disk images read and write their sectors. The
//! others follow DOSBox: floppies and CD-ROMs fail, as if unreadable, and
//! hard disks pretend to succeed.

use crate::cpu::{Cpu, CpuFlags};
use crate::disk::DriveKind;
use crate::diskimage::SECTOR_SIZE;

/// Error AX values.
const NOT_READY: u16 = 0x8002;
const BAD_REQUEST: u16 = 0x0207;
const WRITE_PROTECTED: u16 = 0x0300;

pub fn handle(cpu: &mut Cpu, write: bool) {
    let drive = cpu.get_al();
    let packet = cpu.cx() == 0xFFFF;
    let (start, count, buffer) = if packet {
        let p = cpu.get_physical_addr(cpu.ds(), cpu.bx());
        let start = cpu.bus.guest_read_32(p as u32) as u64;
        let count = cpu.bus.guest_read_16((p + 4) as u32);
        let offset = cpu.bus.guest_read_16((p + 6) as u32);
        let segment = cpu.bus.guest_read_16((p + 8) as u32);
        (start, count, cpu.get_physical_addr(segment, offset))
    } else {
        (cpu.dx() as u64, cpu.cx(), cpu.get_physical_addr(cpu.ds(), cpu.bx()))
    };

    let result = match (cpu.bus.disk.fat_volume(drive), cpu.bus.disk.drive_kind(drive)) {
        (Some(volume), _) => {
            let data_len = count as usize * SECTOR_SIZE;
            if !packet && volume.layout().total_sectors() > 0xFFFF {
                Err(BAD_REQUEST)
            } else if write {
                if cpu.bus.disk.is_writable(drive) {
                    let mut data = vec![0u8; data_len];
                    cpu.bus.guest_read_bytes(buffer as u32, &mut data);
                    // A page of the buffer isn't there: the call runs
                    // again once it is, and nothing goes to the disk before.
                    if cpu.bus.guest_faulted() { Ok(()) } else { volume.write_sectors(start, &data) }
                } else {
                    Err(WRITE_PROTECTED)
                }
            } else {
                let mut data = vec![0u8; data_len];
                volume.read_sectors(start, &mut data).map(|()| cpu.bus.guest_write_bytes(buffer as u32, &data))
            }
            .inspect(|()| {
                let lba = volume.start() + start;
                cpu.bus.sector_activity(drive, lba, count as u32, write);
            })
        }
        (None, Some(DriveKind::HardDisk | DriveKind::Virtual)) => {
            // MicroProse installers read the boot sector of C: to find the
            // hidden sectors of its BPB.
            if !write && drive >= 2 && !packet && count == 1 && start == 0 {
                let hidden = cpu.get_physical_addr(cpu.ds(), cpu.bx().wrapping_add(0x1C));
                cpu.bus.guest_write_16(hidden as u32, 0x3F);
            }
            Ok(())
        }
        _ => Err(NOT_READY),
    };

    match result {
        Ok(()) => {
            cpu.set_ax(0);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }
        Err(error) => {
            cpu.set_ax(error);
            cpu.set_cpu_flag(CpuFlags::CF, true);
        }
    }
}
