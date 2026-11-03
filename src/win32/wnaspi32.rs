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
//!   the one a booted system's IDE channel shows (`cd_to_show`): it stays
//!   the same while it has a disc.
//! - SendASPI32Command does what Windows 98's does with each command,
//!   and returns as it does. SC_EXEC_SCSI_CMD and SC_RESET_DEV run on
//!   the drive at once. Without an event, the block is finished and the
//!   posting routine called before SendASPI32Command returns SS_PENDING,
//!   as if APIX's thread had run the command at once. With an event, it
//!   returns SS_PENDING with the block still empty, and the scheduler
//!   finishes the block and sets the event afterwards (`tick`), as APIX
//!   does from its own thread.
//! - The functions are `cdecl`, as Windows 98's are.

use super::call;
use super::memory::write_process_bytes;
use super::object::{self, Kind};
use super::stubs::{Api, Conv, Dll};
use crate::cpu::Cpu;
use crate::ide::{Atapi, Env};

pub static DLL: Dll = Dll {
    name: "WNASPI32.DLL",
    base: 0xBF66_0000,
    apis: APIS,
    vars: &[],
    init: None,
    term: None,
    vtables: &[],
};

const fn cdecl(name: &'static str, ordinal: u16, args: u8, handler: super::stubs::Handler) -> Api {
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
/// The sense area of the block: SENSE_LEN + 2 bytes.
const SENSE_AREA: usize = 16;

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
    /// Event requests whose drive work is done, waiting for `tick`. Saved
    /// with the rest: a state saved before `tick` finishes them still
    /// sets their events once loaded.
    finishing: Vec<Finished>,
}

crate::state_fields!(State {
    drive,
    clock,
    finishing
});

/// GetASPI32SupportInfo(): SS_COMP (bits 8-15) and the host adapters
/// (bits 0-7). Windows 98 SE's DLL returns its start status and its
/// adapter count from two variables (ordinal 1 at WNASPI32+1160h). With
/// no CD-ROM drive the count is 0 and SS_COMP stays, which is the answer
/// SendASPI32Command's count check turns into SS_FAILED_INIT. The DLL
/// never stores SS_NO_ADAPTERS (E8h) itself.
fn get_aspi32_support_info(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(match cd_drive(cpu) {
        Some(_) => (SS_COMP as u32) << 8 | 1,
        None => (SS_COMP as u32) << 8,
    })
}

/// GetASPI32DLLVersion(): Windows 98 SE's says 1.
fn get_aspi32_dll_version(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(1)
}

/// The DOS drive of the CD-ROM drive ASPI shows, and its target: the
/// drive `cd_to_show` picks, given the one shown before.
pub(super) fn cd_drive(cpu: &mut Cpu) -> Option<(u8, u8)> {
    let disk = &cpu.bus.disk;
    let current = cpu.bus.win32.wnaspi32.drive.as_ref().map(|cd| cd.drive);
    let drive = disk.cd_to_show(current)?;
    let target = disk.ide_slot(drive).is_some_and(|s| s.slave) as u8;
    Some((drive, target))
}

/// The drive ASPI sends commands to. It is made on the first command. When
/// `cd_drive` shows another CD-ROM drive, the new one is made and, if it
/// has a disc, that disc is reported as going in, as the IDE channel does
/// when a disc goes into the drive it shows. None without a CD-ROM drive.
fn drive_state(cpu: &mut Cpu) -> Option<&mut State> {
    let (drive, _) = cd_drive(cpu)?;
    let now = cpu.bus.clock.now_ticks();
    let has_disc = cpu.bus.disk.boot_cd_image(drive).is_some();
    let state = &mut cpu.bus.win32.wnaspi32;
    if state.drive.as_ref().is_none_or(|cd| cd.drive != drive) {
        let shown_before = state.drive.is_some();
        let mut cd = Atapi::new(drive, has_disc && !shown_before);
        if shown_before && has_disc {
            cd.media_changed(true, now.max(state.clock));
        }
        state.drive = Some(cd);
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
    // Without a CD-ROM drive Windows 98's does nothing: ASPI didn't start.
    let Some((_, target)) = cd_drive(cpu) else {
        return Some(SS_FAILED_INIT as u32);
    };
    // The block's command and flags are read first. A null or unmapped
    // block faults the program here, as it does in WNASPI32.
    let cmd = cpu.bus.guest_read_8(srb);
    let flags = cpu.bus.guest_read_8(srb + 3);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let status = match cmd {
        SC_HA_INQUIRY => ha_inquiry(cpu, srb),
        SC_GET_DEV_TYPE => get_dev_type(cpu, srb, target),
        SC_EXEC_SCSI_CMD => {
            let ran = exec_scsi_cmd(cpu, srb, flags, target);
            return complete(cpu, srb, flags, ran);
        }
        SC_ABORT_SRB => SS_ABORT_FAIL,
        SC_RESET_DEV => {
            let ran = reset_dev(cpu, srb, target);
            return complete(cpu, srb, flags, ran);
        }
        SC_GET_DISK_INFO => match device(cpu, srb, target) {
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
    Some(status as u32)
}

/// What SendASPI32Command returns for an SC_EXEC_SCSI_CMD or SC_RESET_DEV
/// that ran on the drive (`Ok`, what it gave) or was rejected (`Err`, its
/// status), and the block finished or queued:
///
/// - An event request returns SS_PENDING with its block still empty, and
///   `tick` finishes it and sets its event later, the way APIX does. A
///   rejected one too, with its status, except a bad block
///   (SS_INVALID_SRB), which returns that at once.
/// - Any other request that ran is finished now and returns SS_PENDING,
///   after its posting routine, if it has one, has been called.
/// - Any other rejected request returns its status, and posts nothing.
fn complete(cpu: &mut Cpu, srb: u32, flags: u8, ran: Result<Finished, u8>) -> Option<u32> {
    // A block or buffer whose page isn't there: the call runs again once
    // the system has put it there.
    if cpu.bus.guest_faulted() {
        return None;
    }
    let pid = super::user32::current_pid(cpu);
    let post = cpu.bus.guest_read_32(srb + EXEC_POST_PROC);
    if flags & SRB_EVENT_NOTIFY != 0 && !matches!(ran, Err(SS_INVALID_SRB)) {
        let mut done = ran.unwrap_or_else(|status| Finished {
            srb,
            status,
            ..Finished::default()
        });
        done.pid = pid;
        done.event = event_object(cpu, post);
        cpu.bus.guest_write_8(srb + 1, SS_PENDING);
        cpu.bus.win32.wnaspi32.finishing.push(done);
        return Some(SS_PENDING as u32);
    }
    let mut done = match ran {
        Ok(done) => done,
        Err(status) => {
            cpu.bus.guest_write_8(srb + 1, status);
            return Some(status as u32);
        }
    };
    done.pid = pid;
    finish(cpu, &done);
    if flags & SRB_POSTING != 0 && post != 0 {
        let resume = call::here(cpu);
        let returns = vec![SS_PENDING as u32];
        if call::call(cpu, post, &[srb], call::ASPI_POSTED, resume, returns) {
            return None;
        }
    }
    Some(SS_PENDING as u32)
}

/// The event object the handle `handle` names in the running process,
/// held until `tick` sets it, so it stays the same object whichever
/// process runs then and whether or not the program closes its handle
/// first. None if the handle isn't an event's.
fn event_object(cpu: &mut Cpu, handle: u32) -> Option<u32> {
    let id = object::resolve(cpu, handle)?;
    let kind = object::objects(cpu).get(id).map(|o| &o.kind);
    if !matches!(kind, Some(Kind::Event { .. })) {
        return None;
    }
    object::objects(cpu).hold(id);
    Some(id)
}

/// Finish the event requests whose drive work is done: write each block
/// (its data, residual count, target status and sense), then its status
/// byte, then set its event. Called on every scheduler tick, which may
/// come in another process or outside any call (an interrupt's), so the
/// blocks are written through their own process's page tables.
pub(crate) fn tick(cpu: &mut Cpu) {
    if cpu.bus.win32.wnaspi32.finishing.is_empty() {
        return;
    }
    let due = std::mem::take(&mut cpu.bus.win32.wnaspi32.finishing);
    for done in due {
        finish(cpu, &done);
        if let Some(id) = done.event {
            super::winmm::callback::set_event(cpu, id);
            object::objects(cpu).release(id);
        }
    }
}

/// A finished SC_EXEC_SCSI_CMD or SC_RESET_DEV: what it writes into its
/// block and the program's buffer, in the memory of the process `pid`
/// that sent it, and the event object to set (an event request's).
#[derive(Clone, Debug, Default)]
pub(crate) struct Finished {
    srb: u32,
    status: u8,
    targ: u8,
    buf: u32,
    data: Vec<u8>,
    residual: Option<u32>,
    sense: Option<Vec<u8>>,
    pid: u32,
    event: Option<u32>,
}

crate::state_fields!(Finished {
    srb,
    status,
    targ,
    buf,
    data,
    residual,
    sense,
    pid,
    event
});

/// Write a finished command's data, residual count, target status and
/// sense into its block and buffer, then its status byte.
fn finish(cpu: &mut Cpu, done: &Finished) {
    let mut write = |lin: u32, bytes: &[u8]| write_process_bytes(&mut cpu.bus, done.pid, lin, bytes);
    write(done.buf, &done.data);
    if let Some(residual) = done.residual {
        write(done.srb + EXEC_BUF_LEN, &residual.to_le_bytes());
    }
    write(done.srb + EXEC_HA_STAT, &[0]);
    write(done.srb + EXEC_TARG_STAT, &[done.targ]);
    if let Some(sense) = &done.sense {
        write(done.srb + EXEC_SENSE, sense);
    }
    write(done.srb + 1, &[done.status]);
}

/// The posting routine returned: SendASPI32Command returns what it would
/// have.
pub(super) fn posted(cpu: &mut Cpu, _: u32, c: call::Call) {
    call::finish(cpu, c.resume, c.data[0]);
}

/// The block's adapter and target, if they are the CD-ROM drive's (`cd`,
/// its target).
fn device(cpu: &mut Cpu, srb: u32, cd: u8) -> Result<(), u8> {
    let ha = cpu.bus.guest_read_8(srb + 2);
    let target = cpu.bus.guest_read_8(srb + 8);
    let lun = cpu.bus.guest_read_8(srb + 9);
    if ha != 0 {
        return Err(SS_INVALID_HA);
    }
    if target != cd || lun != 0 {
        return Err(SS_NO_DEVICE);
    }
    Ok(())
}

/// SC_HA_INQUIRY: the adapter count (one), and adapter 0's identification.
fn ha_inquiry(cpu: &mut Cpu, srb: u32) -> u8 {
    let ha = cpu.bus.guest_read_8(srb + 2);
    if ha != 0 {
        return SS_INVALID_HA;
    }
    let mut info = [0u8; 52];
    info[0] = 1;
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
fn get_dev_type(cpu: &mut Cpu, srb: u32, cd: u8) -> u8 {
    match device(cpu, srb, cd) {
        Ok(()) => {
            cpu.bus.guest_write_8(srb + 10, DTYPE_CDROM);
            SS_COMP
        }
        Err(status) => status,
    }
}

/// SC_RESET_DEV: the drive's DEVICE RESET. The disc stays as it is, and
/// the block gets SS_COMP and zero adapter and target status (`finish`).
fn reset_dev(cpu: &mut Cpu, srb: u32, cd: u8) -> Result<Finished, u8> {
    device(cpu, srb, cd)?;
    Ok(Finished {
        srb,
        status: SS_COMP,
        ..Finished::default()
    })
}

/// SC_EXEC_SCSI_CMD: the block's command run on the drive now. Its data,
/// residual count, target status and sense come back as `Finished`, to be
/// written by `finish`. Rejections return their status.
fn exec_scsi_cmd(cpu: &mut Cpu, srb: u32, flags: u8, cd: u8) -> Result<Finished, u8> {
    if !cpu.bus.guest_probe(srb, EXEC_SIZE, true) {
        return Err(SS_INVALID_SRB);
    }
    device(cpu, srb, cd)?;
    let len = cpu.bus.guest_read_32(srb + EXEC_BUF_LEN);
    let buf = cpu.bus.guest_read_32(srb + EXEC_BUF_POINTER);
    let sense_len = cpu.bus.guest_read_8(srb + EXEC_SENSE_LEN) as usize;
    let cdb_len = (cpu.bus.guest_read_8(srb + EXEC_CDB_LEN) as usize).min(16);
    let mut cdb = [0u8; 16];
    cpu.bus
        .guest_read_bytes(srb + EXEC_CDB, &mut cdb[..cdb_len]);
    if len > MAX_TRANSFER || (len != 0 && !cpu.bus.guest_probe(buf, len as usize, true)) {
        return Err(SS_INVALID_SRB);
    }
    let out_len = if flags & SRB_DIR_OUT != 0 {
        len as usize
    } else {
        0
    };
    let mut out = vec![0u8; out_len];
    cpu.bus.guest_read_bytes(buf, &mut out);

    // Makes the drive if `cd_drive` shows another CD-ROM drive now.
    drive_state(cpu);
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

    let mut data = outcome.data;
    if flags & SRB_DIR_OUT != 0 {
        data.clear();
    } else {
        data.truncate(len as usize);
    }
    let residual = (flags & SRB_ENABLE_RESIDUAL_COUNT != 0 && flags & SRB_DIR_OUT == 0)
        .then(|| len - data.len() as u32);
    let (status, targ, sense) = match outcome.sense {
        None => (SS_COMP, 0, None),
        // The block's sense area is 16 bytes, whatever SenseLen says.
        Some(sense) => (
            SS_ERR,
            STATUS_CHKCOND,
            Some(sense[..sense_len.min(SENSE_AREA)].to_vec()),
        ),
    };
    Ok(Finished {
        srb,
        status,
        targ,
        buf,
        data,
        residual,
        sense,
        ..Finished::default()
    })
}
