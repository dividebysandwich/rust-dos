//! WNASPI32.DLL: ASPI for Win32, which Windows 98 SE ships for programs
//! that send SCSI commands to drives. Copy protections read their discs
//! through it: SafeDisc 2 loads it to check the CD.
//!
//! Windows 98's WNASPI32 hands each request block to APIX.VXD, which
//! passes it to the IOS and the port driver of the drive (ESDI_506.PDR
//! for an IDE CD-ROM drive) as an ATAPI packet. Here the machine's
//! CD-ROM drive is an ATAPI drive of its own (`ide::Atapi`, the drive a
//! booted Windows finds on the IDE channel), and the packets run on it
//! through its task file and data port at once (`Atapi::run_packet`), so
//! the disc answers as it does to a booted Windows 98: the same data,
//! the same sense data for a sector it can't read.
//!
//! What the program sees:
//!
//! - One host adapter, number 0, whose target 0 is the CD-ROM drive (1
//!   when MOUNT's `-ide` puts it on a slave slot), LUN 0. The drive is
//!   the first CD-ROM drive with a disc, as on a booted system.
//! - SendASPI32Command does what Windows 98's does with each command,
//!   and returns as it does: SS_PENDING for SC_EXEC_SCSI_CMD and
//!   SC_RESET_DEV unless the block asks for an event, the block's status
//!   otherwise. Commands finish before it returns, as if APIX's thread
//!   had run them at once: the status is final, the event set and the
//!   posting routine called by the time the program looks.
//! - The functions are `cdecl`, as Windows 98's are.

use super::call;
use super::stubs::{Api, Conv, Dll};
use crate::cpu::Cpu;
use crate::disk::DriveKind;
use crate::ide::{Atapi, Env};

pub static DLL: Dll = Dll {
    name: "WNASPI32.DLL",
    base: 0xBF6C_0000,
    apis: APIS,
    vars: &[],
    init: None,
    vtables: &[],
};

const fn cdecl(
    name: &'static str,
    ordinal: u16,
    args: u8,
    handler: super::stubs::Handler,
) -> Api {
    Api {
        name,
        ordinal,
        args,
        conv: Conv::Cdecl,
        handler,
    }
}

/// The functions, with Windows 98 SE's ordinals. Append only (see
/// `stubs`).
static APIS: &[Api] = &[
    cdecl("GetASPI32SupportInfo", 1, 0, get_aspi32_support_info),
    cdecl("SendASPI32Command", 2, 1, send_aspi32_command),
    cdecl("GetASPI32DLLVersion", 4, 0, get_aspi32_dll_version),
];

// The commands (SRB_Cmd).
const SC_HA_INQUIRY: u8 = 0x00;
const SC_GET_DEV_TYPE: u8 = 0x01;
const SC_EXEC_SCSI_CMD: u8 = 0x02;
const SC_ABORT_SRB: u8 = 0x03;
const SC_RESET_DEV: u8 = 0x04;
const SC_GET_DISK_INFO: u8 = 0x06;

// The status (SRB_Status), and what the functions return.
const SS_PENDING: u8 = 0x00;
const SS_COMP: u8 = 0x01;
const SS_ABORT_FAIL: u8 = 0x03;
const SS_ERR: u8 = 0x04;
const SS_INVALID_CMD: u8 = 0x80;
const SS_INVALID_HA: u8 = 0x81;
const SS_NO_DEVICE: u8 = 0x82;
const SS_INVALID_SRB: u8 = 0xE0;
const SS_FAILED_INIT: u8 = 0xE4;

// SRB_Flags.
const SRB_POSTING: u8 = 0x01;
const SRB_ENABLE_RESIDUAL_COUNT: u8 = 0x04;
const SRB_DIR_OUT: u8 = 0x10;
const SRB_EVENT_NOTIFY: u8 = 0x40;

/// The target's status after a command: CHECK CONDITION.
const STATUS_CHKCOND: u8 = 0x02;
/// SCSI's device type of a CD-ROM drive.
const DTYPE_CDROM: u8 = 0x05;

/// SRB_ExecSCSICmd: where its fields are, and its size with the 16 bytes
/// of sense data Windows 98's header gives it (SENSE_LEN + 2).
const EXEC_BUF_LEN: u32 = 12;
const EXEC_BUF_POINTER: u32 = 16;
const EXEC_SENSE_LEN: u32 = 20;
const EXEC_CDB_LEN: u32 = 21;
const EXEC_HA_STAT: u32 = 22;
const EXEC_TARG_STAT: u32 = 23;
const EXEC_POST_PROC: u32 = 24;
const EXEC_CDB: u32 = 48;
const EXEC_SENSE: u32 = 64;
const EXEC_SIZE: usize = 80;

/// The host adapter's identification in SC_HA_INQUIRY: the ASPI manager,
/// as APIX.VXD names itself to Win32 programs, and the adapter, by its
/// port driver.
const MANAGER_ID: &[u8; 16] = b"ASPI for WIN32  ";
const ADAPTER_ID: &[u8; 16] = b"ESDI_506        ";
/// The adapter's SCSI ID.
const ADAPTER_SCSI_ID: u8 = 7;
/// Targets on the adapter: an IDE channel's master and slave.
const TARGETS: u8 = 2;
/// The most a command transfers, a data block of the drive's.
const MAX_TRANSFER: u32 = 0x1_0000;

/// The state: the drive the commands go to, and the time of the last
/// command (PIT ticks), which its delays moved on from the machine's.
#[derive(Clone, Debug, Default)]
pub struct State {
    drive: Option<Atapi>,
    clock: u64,
}

crate::state_fields!(State { drive, clock });

/// GetASPI32SupportInfo(): the host adapters (bits 0-7) and SS_COMP
/// (bits 8-15). With no CD-ROM drive there is no adapter, and ASPI
/// says so with SS_NO_ADAPTERS, as Windows 98's does on a machine
/// without one.
fn get_aspi32_support_info(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    const SS_NO_ADAPTERS: u32 = 0xE8;
    Some(match cd_drive(cpu) {
        Some(_) => (SS_COMP as u32) << 8 | 1,
        None => SS_NO_ADAPTERS << 8,
    })
}

/// GetASPI32DLLVersion(): Windows 98 SE's says 1.
fn get_aspi32_dll_version(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(1)
}

/// The DOS drive of the CD-ROM drive ASPI shows, and its target: the
/// first CD-ROM drive with a disc, or the first one.
fn cd_drive(cpu: &mut Cpu) -> Option<(u8, u8)> {
    let disk = &cpu.bus.disk;
    let cds = disk.drives_of_kind(DriveKind::CdRom);
    let drive = cds
        .iter()
        .copied()
        .find(|&d| disk.boot_cd_image(d).is_some())
        .or(cds.first().copied())?;
    let target = disk.ide_slot(drive).is_some_and(|s| s.slave) as u8;
    Some((drive, target))
}

/// The drive ASPI sends commands to. It is made on the first command, and
/// again when `cd_drive` picks another CD-ROM drive (a disc moved), with
/// the disc standing still, as after the BIOS. None without a CD-ROM drive.
fn drive_state(cpu: &mut Cpu) -> Option<&mut State> {
    let (drive, _) = cd_drive(cpu)?;
    let has_disc = cpu.bus.disk.boot_cd_image(drive).is_some();
    let state = &mut cpu.bus.win32.wnaspi32;
    if state.drive.as_ref().is_none_or(|cd| cd.drive != drive) {
        state.drive = Some(Atapi::new(drive, has_disc));
    }
    Some(state)
}

/// The disc in `drive` changed: ASPI's drive reports it, as the one on a
/// booted system's IDE channel does.
pub(crate) fn media_changed(state: &mut State, drive: u8, has_disc: bool, now: u64) {
    if let Some(cd) = &mut state.drive
        && cd.drive == drive
    {
        cd.media_changed(has_disc, now.max(state.clock));
    }
}

/// SendASPI32Command(srb): carry out the request block `srb`.
fn send_aspi32_command(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let srb = args[0];
    // Without an adapter Windows 98's does nothing: ASPI didn't start.
    if cd_drive(cpu).is_none() {
        return Some(SS_FAILED_INIT as u32);
    }
    // It reads the command and flags first, and faults on a bad pointer.
    if srb == 0 {
        return Some(SS_INVALID_SRB as u32);
    }
    let cmd = cpu.bus.guest_read_8(srb);
    let flags = cpu.bus.guest_read_8(srb + 3);
    let status = match cmd {
        SC_HA_INQUIRY => ha_inquiry(cpu, srb),
        SC_GET_DEV_TYPE => get_dev_type(cpu, srb),
        SC_EXEC_SCSI_CMD => exec_scsi_cmd(cpu, srb, flags),
        SC_ABORT_SRB => SS_ABORT_FAIL,
        SC_RESET_DEV => reset_dev(cpu, srb),
        SC_GET_DISK_INFO => match device(cpu, srb) {
            // Not a disk of INT 13h's: no flags, no geometry.
            Ok(()) => {
                cpu.bus.guest_fill(srb + 10, 4, 0);
                SS_COMP
            }
            Err(status) => status,
        },
        _ => SS_INVALID_CMD,
    };
    cpu.bus.guest_write_8(srb + 1, status);
    // Only a command that reached the drive completes later on. A rejected
    // one (no such target, a bad block) returns its error status and posts
    // nothing.
    let ran = matches!(cmd, SC_EXEC_SCSI_CMD | SC_RESET_DEV)
        && matches!(status, SS_COMP | SS_ERR);
    if !ran {
        return Some(status as u32);
    }
    // A command that runs on the drive completes as APIX does: the
    // event set, or the posting routine called with the block.
    let post = cpu.bus.guest_read_32(srb + EXEC_POST_PROC);
    let returns = if flags & SRB_EVENT_NOTIFY != 0 {
        status
    } else {
        SS_PENDING
    };
    if flags & SRB_EVENT_NOTIFY != 0 && post != 0 {
        super::kernel32::set_event_handle(cpu, post);
    } else if flags & SRB_POSTING != 0 && post != 0 {
        let resume = call::here(cpu);
        if call::call(
            cpu,
            post,
            &[srb],
            call::ASPI_POSTED,
            resume,
            vec![returns as u32],
        ) {
            return None;
        }
    }
    Some(returns as u32)
}

/// The posting routine returned: SendASPI32Command returns what it would
/// have.
pub(super) fn posted(cpu: &mut Cpu, _: u32, c: call::Call) {
    call::finish(cpu, c.resume, c.data[0]);
}

/// The block's adapter and target, if they are the CD-ROM drive's.
fn device(cpu: &mut Cpu, srb: u32) -> Result<(), u8> {
    let ha = cpu.bus.guest_read_8(srb + 2);
    let target = cpu.bus.guest_read_8(srb + 8);
    let lun = cpu.bus.guest_read_8(srb + 9);
    let Some((_, cd)) = cd_drive(cpu) else {
        return Err(SS_INVALID_HA);
    };
    if ha != 0 {
        return Err(SS_INVALID_HA);
    }
    if target != cd || lun != 0 {
        return Err(SS_NO_DEVICE);
    }
    Ok(())
}

/// SC_HA_INQUIRY: the adapter count, and adapter 0's identification.
fn ha_inquiry(cpu: &mut Cpu, srb: u32) -> u8 {
    let ha = cpu.bus.guest_read_8(srb + 2);
    let count = cd_drive(cpu).is_some() as u8;
    if ha >= count.max(1) {
        return SS_INVALID_HA;
    }
    let mut info = [0u8; 52];
    info[0] = count;
    info[1] = ADAPTER_SCSI_ID;
    info[2..18].copy_from_slice(MANAGER_ID);
    info[18..34].copy_from_slice(ADAPTER_ID);
    // HA_Unique: buffer alignment (none), residual counts, the targets,
    // the largest transfer.
    info[36] = 0x02;
    info[37] = TARGETS;
    info[38..42].copy_from_slice(&MAX_TRANSFER.to_le_bytes());
    cpu.bus.guest_write_bytes(srb + 8, &info);
    SS_COMP
}

/// SC_GET_DEV_TYPE: the SCSI device type of the target.
fn get_dev_type(cpu: &mut Cpu, srb: u32) -> u8 {
    match device(cpu, srb) {
        Ok(()) => {
            cpu.bus.guest_write_8(srb + 10, DTYPE_CDROM);
            SS_COMP
        }
        Err(status) => status,
    }
}

/// SC_RESET_DEV: the drive's DEVICE RESET. The disc stays as it is.
fn reset_dev(cpu: &mut Cpu, srb: u32) -> u8 {
    if let Err(status) = device(cpu, srb) {
        return status;
    }
    cpu.bus.guest_write_8(srb + EXEC_HA_STAT, 0);
    cpu.bus.guest_write_8(srb + EXEC_TARG_STAT, 0);
    SS_COMP
}

/// SC_EXEC_SCSI_CMD: the block's command run on the drive, its data
/// moved to or from the program's buffer, and the drive's sense data
/// in the block when the drive reports an error (CHECK CONDITION).
fn exec_scsi_cmd(cpu: &mut Cpu, srb: u32, flags: u8) -> u8 {
    if !cpu.bus.guest_probe(srb, EXEC_SIZE, true) {
        return SS_INVALID_SRB;
    }
    if let Err(status) = device(cpu, srb) {
        return status;
    }
    let len = cpu.bus.guest_read_32(srb + EXEC_BUF_LEN);
    let buf = cpu.bus.guest_read_32(srb + EXEC_BUF_POINTER);
    let sense_len = cpu.bus.guest_read_8(srb + EXEC_SENSE_LEN) as usize;
    let cdb_len = (cpu.bus.guest_read_8(srb + EXEC_CDB_LEN) as usize).min(16);
    let mut cdb = [0u8; 16];
    cpu.bus.guest_read_bytes(srb + EXEC_CDB, &mut cdb[..cdb_len]);
    if len > MAX_TRANSFER || len != 0 && !cpu.bus.guest_probe(buf, len as usize, true) {
        return SS_INVALID_SRB;
    }
    let mut out = vec![0u8; if flags & SRB_DIR_OUT != 0 { len as usize } else { 0 }];
    cpu.bus.guest_read_bytes(buf, &mut out);

    if drive_state(cpu).is_none() {
        return SS_NO_DEVICE;
    }
    let now = cpu.bus.clock.now_ticks();
    let bus = &mut cpu.bus;
    let state = &mut bus.win32.wnaspi32;
    let mut env = Env {
        disks: &bus.disk,
        player: &mut bus.cdaudio,
        now: now.max(state.clock),
        hard_disk_ns_per_sector: 0,
        faked: false,
        moved: 0,
    };
    let cd = state.drive.as_mut().expect("the drive was made");
    let outcome = cd.run_packet(&cdb[..cdb_len], &out, &mut env);
    state.clock = env.now;
    let moved = env.moved;
    let log = std::mem::take(&mut cd.log);
    bus.activity.io_bytes += moved;
    for line in log {
        bus.log_string(&line);
    }

    let data = if flags & SRB_DIR_OUT != 0 {
        &[][..]
    } else {
        &outcome.data[..outcome.data.len().min(len as usize)]
    };
    cpu.bus.guest_write_bytes(buf, data);
    if flags & SRB_ENABLE_RESIDUAL_COUNT != 0 && flags & SRB_DIR_OUT == 0 {
        cpu.bus
            .guest_write_32(srb + EXEC_BUF_LEN, len - data.len() as u32);
    }
    cpu.bus.guest_write_8(srb + EXEC_HA_STAT, 0);
    match outcome.sense {
        None => {
            cpu.bus.guest_write_8(srb + EXEC_TARG_STAT, 0);
            SS_COMP
        }
        Some(sense) => {
            cpu.bus.guest_write_8(srb + EXEC_TARG_STAT, STATUS_CHKCOND);
            let n = sense_len.min(sense.len());
            cpu.bus.guest_write_bytes(srb + EXEC_SENSE, &sense[..n]);
            SS_ERR
        }
    }
}
