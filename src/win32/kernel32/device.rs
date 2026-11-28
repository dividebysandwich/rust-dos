//! VxD handles: CreateFileA of "\\.\NAME" and DeviceIoControl on what it
//! gives. On Windows 98 such a name opens the VxD NAME for a Win32
//! program, which then sends it requests with DeviceIoControl, each a
//! control code and buffers that the VxD reads and fills.
//!
//! The built-in Windows has VWIN32's, the VxD through which Win32
//! programs make the DOS calls that KERNEL32 has no function for: its
//! control code 1 (VWIN32_DIOC_DOS_IOCTL) makes an INT 21h call with the
//! registers of a DIOC_REGISTERS structure and gives back the registers
//! it leaves. It is meant for the IOCTLs (AH=44h), but Windows 98 runs
//! whatever function AX names (AX=3000h gives the DOS version). Of those
//! calls the built-in one has Get Drive Map Info (440Dh with CX=086Fh),
//! which Prince of Persia 3D makes for each drive, and gives the others
//! Invalid Function.
//!
//! What Windows 98 SE gives was measured with a probe program on it,
//! booted on an IDE hard disk with a CD-ROM drive: the opens, the drive
//! map of each kind of drive, the errors of each control code and of
//! buffers too small, and DeviceIoControl on handles that aren't a
//! VxD's.

use super::{
    ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, FALSE, TRUE,
    set_error,
};
use crate::cpu::Cpu;
use crate::disk::{DriveKind, FLOPPY_DRIVES, LASTDRIVE};
use crate::win32::object::{self, Kind};

const ERROR_FILE_NOT_FOUND: u32 = super::path::ERROR_FILE_NOT_FOUND;
const ERROR_NOT_SUPPORTED: u32 = 50;

/// The VxDs the built-in Windows opens for Win32 programs.
const VWIN32: &str = "VWIN32";

/// VWIN32's control codes that make a DOS call with a DIOC_REGISTERS:
/// INT 21h AH=44h (DIOC_DOS_IOCTL), INT 25h and 26h, INT 13h, and INT 21h
/// AX=7305h (DIOC_DOS_DRIVEINFO). Codes 0 and 5 fail with
/// ERROR_INVALID_PARAMETER, and the others with ERROR_NOT_SUPPORTED.
const DIOC_DOS_IOCTL: u32 = 1;
const DIOC_DOS_CALLS: [u32; 5] = [DIOC_DOS_IOCTL, 2, 3, 4, 6];
const DIOC_INVALID: [u32; 2] = [0, 5];

/// DIOC_REGISTERS: EBX, EDX, ECX, EAX, EDI, ESI and the flags.
const DIOC_SIZE: u32 = 28;
const EBX: usize = 0;
const EDX: usize = 1;
const ECX: usize = 2;
const EAX: usize = 3;
const FLAGS: usize = 6;

/// The flags a DOS call gives back through VWIN32 on Windows 98 SE, with
/// the carry clear and set: those of the virtual machine it ran in (IF,
/// ZF and PF), whatever the caller put in.
const FLAGS_OK: u32 = 0x0246;
const FLAGS_CARRY: u32 = 0x0247;

/// DOS's errors.
const DOS_INVALID_FUNCTION: u16 = 0x01;
const DOS_ACCESS_DENIED: u16 = 0x05;
const DOS_INVALID_DRIVE: u16 = 0x0F;

/// DRIVE_MAP_INFO, and its dmiFlags: a protected-mode driver for the
/// logical drive, one for the physical drive, a drive that DOS doesn't
/// have, and one that ejects its media.
const DRIVE_MAP_INFO_SIZE: u8 = 16;
const PROT_MODE_LOGICAL_DRIVE: u8 = 0x01;
const PROT_MODE_PHYSICAL_DRIVE: u8 = 0x02;
const PROT_MODE_ONLY_DRIVE: u8 = 0x04;
const PROT_MODE_EJECT: u8 = 0x08;

/// dmiInt13Unit of a drive with no BIOS unit.
const NO_UNIT: u8 = 0xFF;

/// CreateFileA of `name`, if it is a VxD's ("\\.\NAME"): the handle, or
/// the error. On Windows 98 the open of VWIN32 succeeds whatever the
/// access, sharing and disposition, and leaves the last error of the
/// file open it tried first: ERROR_INVALID_PARAMETER for a disposition
/// that isn't one, ERROR_FILE_NOT_FOUND for the others. A VxD that isn't
/// there, a name with an extension or a drive (`\\.\C:`) fail with
/// ERROR_FILE_NOT_FOUND.
pub(super) fn open(cpu: &mut Cpu, name: &str, disposition: u32) -> Option<Result<u32, u32>> {
    let vxd = name.strip_prefix(r"\\.\")?;
    if !vxd.eq_ignore_ascii_case(VWIN32) {
        return Some(Err(ERROR_FILE_NOT_FOUND));
    }
    let kind = Kind::Vxd {
        name: VWIN32.to_string(),
    };
    let handle = object::create(cpu, kind, None);
    let valid = (super::file::CREATE_NEW..=super::file::TRUNCATE_EXISTING).contains(&disposition);
    set_error(
        cpu,
        if valid {
            ERROR_FILE_NOT_FOUND
        } else {
            ERROR_INVALID_PARAMETER
        },
    );
    Some(Ok(handle))
}

/// DeviceIoControl(handle, code, in, in size, out, out size, returned,
/// overlapped): VWIN32's request `code`. A handle of anything but a VxD
/// fails with ERROR_INVALID_PARAMETER, as a file's and an event's do on
/// Windows 98, and one that isn't a handle with ERROR_INVALID_HANDLE.
pub(super) fn device_io_control(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, code, input, in_size, output, out_size, returned, _] = args else {
        unreachable!()
    };
    let error = match object::kind_of(cpu, handle) {
        None => ERROR_INVALID_HANDLE,
        Some(Kind::Vxd { .. }) if DIOC_INVALID.contains(&code) => ERROR_INVALID_PARAMETER,
        Some(Kind::Vxd { .. }) if !DIOC_DOS_CALLS.contains(&code) => ERROR_NOT_SUPPORTED,
        Some(Kind::Vxd { .. })
            if input == 0 || output == 0 || in_size < DIOC_SIZE || out_size < DIOC_SIZE =>
        {
            ERROR_INSUFFICIENT_BUFFER
        }
        Some(Kind::Vxd { .. }) => return dos_call(cpu, code, input, output, returned),
        Some(_) => ERROR_INVALID_PARAMETER,
    };
    set_error(cpu, error);
    Some(FALSE)
}

/// VWIN32's DOS call `code` on the DIOC_REGISTERS at `input`, whose
/// registers as the call leaves them go to `output`, and their size to
/// `returned` (if not null). The call's own failure is in the registers:
/// DeviceIoControl succeeds. None if a page isn't there, which runs the
/// function again.
fn dos_call(cpu: &mut Cpu, code: u32, input: u32, output: u32, returned: u32) -> Option<u32> {
    let mut regs = [0u32; 7];
    for (i, reg) in regs.iter_mut().enumerate() {
        *reg = cpu.bus.guest_read_32(input + 4 * i as u32);
    }
    if cpu.bus.guest_faulted() {
        return None;
    }
    let result = match code {
        DIOC_DOS_IOCTL => ioctl(cpu, &mut regs)?,
        _ => Err(DOS_INVALID_FUNCTION),
    };
    if result == Err(DOS_INVALID_FUNCTION) {
        cpu.bus.log_string(&format!(
            "[W32] VWIN32's DeviceIoControl: code {code} with AX={:04X} BX={:04X} \
             CX={:04X} DX={:04X} isn't there",
            regs[EAX] & 0xFFFF,
            regs[EBX] & 0xFFFF,
            regs[ECX] & 0xFFFF,
            regs[EDX] & 0xFFFF,
        ));
    }
    match result {
        Ok(ax) => {
            regs[EAX] = regs[EAX] & 0xFFFF_0000 | ax as u32;
            regs[FLAGS] = FLAGS_OK;
        }
        Err(error) => {
            regs[EAX] = regs[EAX] & 0xFFFF_0000 | error as u32;
            regs[FLAGS] = FLAGS_CARRY;
        }
    }
    for (i, reg) in regs.iter().enumerate() {
        cpu.bus.guest_write_32(output + 4 * i as u32, *reg);
    }
    if returned != 0 {
        cpu.bus.guest_write_32(returned, DIOC_SIZE);
    }
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(TRUE)
}

/// INT 21h AH=44h with `regs`: AX as it leaves it, or DOS's error. None
/// if a page isn't there.
fn ioctl(cpu: &mut Cpu, regs: &mut [u32; 7]) -> Option<Result<u16, u16>> {
    let ax = regs[EAX] as u16;
    let cx = regs[ECX] as u16;
    if ax == 0x440D && cx == 0x086F {
        return drive_map_info(cpu, regs[EBX] as u8, regs[EDX]);
    }
    Some(Err(DOS_INVALID_FUNCTION))
}

/// Get Drive Map Info: the DRIVE_MAP_INFO of `drive` (1 for A:, 0 for
/// the current drive) at the flat address `at`, whose first byte the
/// caller set to the buffer's size. Windows 98 SE gives AX 0, writes the
/// structure's 16 bytes but the first, and gives Access Denied for a
/// buffer under 16 bytes and Invalid Drive for a drive there isn't,
/// writing nothing.
///
/// A hard disk has the protected-mode drivers of the logical and the
/// physical drive, its BIOS unit (80h for the first) and the partition's
/// start (the BPB's hidden sectors: 63 for a host folder, as FDISK puts
/// the first partition on the second track); a CD-ROM drive also has no
/// DOS drive and an eject, and no BIOS unit (FFh). The associated drive
/// map is 0 for every drive, as Windows 98 SE gives it. A floppy drive is
/// a logical and physical drive of its BIOS unit. The BIOS units are
/// those INT 13h takes for the drive (`interrupts::int13::bios_drive`),
/// so a program can read the drive through the unit it is given. The
/// built-in Windows' drives held in memory (Z:) are DOS's alone, as a
/// RAM drive of a DOS driver is: no protected-mode driver and no BIOS
/// unit.
fn drive_map_info(cpu: &mut Cpu, drive: u8, at: u32) -> Option<Result<u16, u16>> {
    let disk = &cpu.bus.disk;
    let drive = match drive {
        0 => disk.get_current_drive(),
        n => n - 1,
    };
    let kind = (drive < LASTDRIVE)
        .then(|| disk.drive_kind(drive))
        .flatten();
    let Some(kind) = kind else {
        return Some(Err(DOS_INVALID_DRIVE));
    };
    let protected = PROT_MODE_LOGICAL_DRIVE | PROT_MODE_PHYSICAL_DRIVE;
    let (flags, unit, start) = match kind {
        DriveKind::Floppy => {
            let unit = (0..FLOPPY_DRIVES).find(|&unit| disk.floppy_unit(unit) == drive);
            (protected, unit.unwrap_or(NO_UNIT), 0)
        }
        DriveKind::HardDisk => {
            let units = disk.hard_disk_units(|_| true);
            let unit = units.iter().position(|&d| d == drive);
            let unit = unit.map_or(NO_UNIT, |n| 0x80 + n as u8);
            let start = disk.layout(drive).map_or(0, |l| l.hidden_sectors);
            (protected, unit, u64::from(start))
        }
        DriveKind::CdRom => {
            let flags = protected | PROT_MODE_ONLY_DRIVE | PROT_MODE_EJECT;
            (flags, NO_UNIT, 0)
        }
        DriveKind::Virtual => (0, NO_UNIT, 0),
    };
    let size = cpu.bus.guest_read_8(at);
    if cpu.bus.guest_faulted() {
        return None;
    }
    if size < DRIVE_MAP_INFO_SIZE {
        return Some(Err(DOS_ACCESS_DENIED));
    }
    let mut info = [0u8; DRIVE_MAP_INFO_SIZE as usize - 1];
    info[0] = DRIVE_MAP_INFO_SIZE;
    info[1] = flags;
    info[2] = unit;
    // dmiAssociatedDriveMap at 4 stays 0.
    info[7..15].copy_from_slice(&start.to_le_bytes());
    cpu.bus.guest_write_bytes(at + 1, &info);
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(Ok(0))
}
