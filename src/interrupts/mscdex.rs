//! MSCDEX, the CD-ROM extensions (INT 2Fh AH=15h), and the CD-ROM device
//! driver behind them: the requests programs send through MSCDEX (AX=1510h)
//! or straight to the driver, whose header they find with AX=1501h.
//!
//! Drives mounted from CD images answer everything a CD drive does: raw
//! and cooked sector reads, the volume descriptors, the table of contents
//! and audio playback. Drives backed by a host directory have only files,
//! and answer the few requests that make sense without sectors.

use crate::bus::Bus;
use crate::cdrom::audio::PlayState;
use crate::cdrom::image::CdImage;
use crate::cdrom::{self, DATA_SECTOR, RAW_SECTOR};
use crate::cpu::{Cpu, CpuFlags};
use crate::disk::DriveKind;
use iced_x86::Register;
use std::rc::Rc;

/// MSCDEX version reported by AX=150Ch (2.23).
const MSCDEX_VERSION: u16 = 0x0217;

/// The CD-ROM driver in the BIOS segment F000: its device header, and the
/// strategy and interrupt entries, which are emulator services.
pub const DEVICE_HEADER: u16 = 0x1160;
pub const STRATEGY_ENTRY: u16 = 0x1180;
pub const INTERRUPT_ENTRY: u16 = 0x1184;

/// Device driver request status: done, busy (playing audio), and errors:
/// bit 15, done, and the error code in the low byte.
const DONE: u16 = 0x0100;
const BUSY: u16 = 0x0200;
const UNKNOWN_UNIT: u16 = 0x8101;
const UNKNOWN_COMMAND: u16 = 0x8103;
const SECTOR_NOT_FOUND: u16 = 0x8108;
const GENERAL_FAILURE: u16 = 0x810C;

/// Errors of the INT 2Fh calls, in AX with CF set.
const INVALID_DRIVE: u16 = 15;
const NOT_READY: u16 = 21;
const FILE_NOT_FOUND: u16 = 2;

/// Device status (IOCTL input 06h): cooked and raw reads, data and audio,
/// audio channel control, HSG and Red Book addressing. Bit 1 is set while
/// the door is unlocked.
const DEVICE_STATUS: u32 = 0x0314;
const UNLOCKED: u32 = 0x0002;
/// Volume size of a drive without an image, in 2048-byte sectors.
const FOLDER_SECTORS: u32 = 0xFFFF;

/// What MSCDEX remembers per drive.
#[derive(Default)]
pub struct MscdexState {
    /// The request header the CD driver's strategy entry was given.
    request: (u16, u16),
    /// A disc was mounted that programs haven't asked about yet.
    changed: [bool; 26],
    locked: [bool; 26],
}

impl MscdexState {
    /// A new disc is in `drive`: the next media check reports it.
    pub fn disc_changed(&mut self, drive: u8) {
        if let Some(changed) = self.changed.get_mut(drive as usize) {
            *changed = true;
        }
    }
}

fn set_error(cpu: &mut Cpu, code: u16) {
    cpu.set_ax(code);
    cpu.set_cpu_flag(CpuFlags::CF, true);
}

fn clear_error(cpu: &mut Cpu) {
    cpu.set_cpu_flag(CpuFlags::CF, false);
}

/// The image in CD drive `drive` (from CX), or the INT 2Fh error for it.
fn drive_image(cpu: &Cpu, drive: u16) -> Result<Rc<CdImage>, u16> {
    let drive = u8::try_from(drive).map_err(|_| INVALID_DRIVE)?;
    if cpu.bus.disk.drive_kind(drive) != Some(DriveKind::CdRom) {
        return Err(INVALID_DRIVE);
    }
    cpu.bus.disk.cd_image(drive).ok_or(NOT_READY)
}

/// INT 2Fh AH=15h.
pub fn handle(cpu: &mut Cpu, function: u8) {
    let cd_drives = cpu.bus.disk.drives_of_kind(DriveKind::CdRom);
    // Without CD-ROM drives MSCDEX isn't loaded: leave everything untouched.
    if cd_drives.is_empty() {
        return;
    }
    let es_bx = cpu.get_physical_addr(cpu.es(), cpu.bx());

    match function {
        // Installation check: BX = number of CD-ROM drives, CX = first one (0=A)
        0x00 => {
            cpu.set_bx(cd_drives.len() as u16);
            cpu.set_cx(cd_drives[0] as u16);
        }
        // Drive device list: a subunit number and the driver's header for
        // each drive.
        0x01 => {
            for (unit, _) in cd_drives.iter().enumerate() {
                cpu.bus.write_8(es_bx + unit * 5, unit as u8);
                cpu.bus.write_16(es_bx + unit * 5 + 1, DEVICE_HEADER);
                cpu.bus.write_16(es_bx + unit * 5 + 3, 0xF000);
            }
        }
        // Copyright, abstract and bibliographic file names from the
        // Primary Volume Descriptor.
        0x02..=0x04 => match drive_image(cpu, cpu.cx()) {
            Ok(image) => match cdrom::iso9660::primary_descriptor(&image) {
                Ok(pvd) => {
                    let at = 702 + (function as usize - 2) * 37;
                    for (i, &b) in pvd[at..at + 37].iter().enumerate() {
                        cpu.bus.write_8(es_bx + i, b);
                    }
                    cpu.bus.write_8(es_bx + 37, 0);
                    clear_error(cpu);
                }
                Err(_) => set_error(cpu, NOT_READY),
            },
            Err(code) => set_error(cpu, code),
        },
        // Read volume descriptor DX (0 = the first, at sector 16). AX is
        // its type: 1 primary, FFh terminator, 0 others.
        0x05 => match drive_image(cpu, cpu.cx()) {
            Ok(image) => {
                let start = image.data_track().map_or(0, |t| t.start);
                let mut sector = [0u8; DATA_SECTOR];
                if image.read_data(start + 16 + cpu.dx() as u32, &mut sector).is_err() {
                    set_error(cpu, NOT_READY);
                    return;
                }
                for (i, &b) in sector.iter().enumerate() {
                    cpu.bus.write_8(es_bx + i, b);
                }
                cpu.set_ax(match sector[0] {
                    1 => 1,
                    0xFF => 0xFF,
                    _ => 0,
                });
                clear_error(cpu);
            }
            Err(code) => set_error(cpu, code),
        },
        // Debugging on and off: nothing to do.
        0x06 | 0x07 => {}
        // Absolute read: DX sectors from SI:DI into ES:BX.
        0x08 => match drive_image(cpu, cpu.cx()) {
            Ok(image) => {
                let start = (cpu.si() as u32) << 16 | cpu.di() as u32;
                let mut sector = [0u8; DATA_SECTOR];
                for i in 0..cpu.dx() as u32 {
                    if image.read_data(start + i, &mut sector).is_err() {
                        set_error(cpu, NOT_READY);
                        return;
                    }
                    let at = es_bx + i as usize * DATA_SECTOR;
                    for (j, &b) in sector.iter().enumerate() {
                        cpu.bus.write_8(at + j, b);
                    }
                }
                clear_error(cpu);
            }
            Err(code) => set_error(cpu, code),
        },
        // Absolute write: CDs are read-only.
        0x09 => match drive_image(cpu, cpu.cx()) {
            Ok(_) => set_error(cpu, NOT_READY),
            Err(code) => set_error(cpu, code),
        },
        // CD-ROM drive check: CX = drive. BX=ADADh marks MSCDEX; AX is
        // nonzero when the drive is a CD-ROM.
        0x0B => {
            let is_cd = cpu.cx() < 26 && cd_drives.contains(&(cpu.cx() as u8));
            cpu.set_ax(if is_cd { 0x5AD8 } else { 0 });
            cpu.set_bx(0xADAD);
        }
        // MSCDEX version: BX = major/minor
        0x0C => cpu.set_bx(MSCDEX_VERSION),
        // Get CD-ROM drive letters: one byte (0=A) per drive at ES:BX
        0x0D => {
            for (i, &drive) in cd_drives.iter().enumerate() {
                cpu.bus.write_8(es_bx + i, drive);
            }
        }
        // Volume descriptor preference: always the primary one.
        0x0E => {
            if cpu.bx() == 0 {
                cpu.set_dx(0x0100);
            }
            clear_error(cpu);
        }
        // Directory entry of the path at ES:BX, copied to SI:DI. AX = 1:
        // the disc is ISO 9660.
        0x0F => get_directory_entry(cpu, es_bx),
        // Send device driver request: CX = drive, ES:BX -> request header
        0x10 => {
            let drive = u8::try_from(cpu.cx()).ok().filter(|d| cd_drives.contains(d));
            if let Some(unit) = drive.and_then(|d| cd_drives.iter().position(|&x| x == d)) {
                cpu.bus.write_8(es_bx + 1, unit as u8);
            }
            device_request(cpu, drive, es_bx);
        }
        _ => {
            cpu.bus.log_string(&format!("[MSCDEX] Unhandled INT 2Fh AX={:04X}", cpu.ax()));
        }
    }
}

fn get_directory_entry(cpu: &mut Cpu, path_addr: usize) {
    let drive = cpu.get_reg8(Register::CL) as u16;
    let image = match drive_image(cpu, drive) {
        Ok(image) => image,
        Err(code) => return set_error(cpu, code),
    };
    let mut path = String::new();
    for i in 0..128 {
        match cpu.bus.read_8(path_addr + i) {
            0 => break,
            b => path.push(b as char),
        }
    }
    let path = match path.as_bytes() {
        [_, b':', ..] => &path[2..],
        _ => &path[..],
    };
    match cdrom::iso9660::find_record(&image, path) {
        Some(record) => {
            let buffer = cpu.get_physical_addr(cpu.si(), cpu.di());
            for (i, &b) in record.iter().take(255).enumerate() {
                cpu.bus.write_8(buffer + i, b);
            }
            cpu.set_ax(1);
            clear_error(cpu);
        }
        None => set_error(cpu, FILE_NOT_FOUND),
    }
}

/// The CD driver's strategy entry: remember the request at ES:BX.
pub fn strategy(cpu: &mut Cpu) {
    cpu.bus.mscdex.request = (cpu.es(), cpu.bx());
}

/// The CD driver's interrupt entry: carry out the remembered request for
/// the drive its subunit number names.
pub fn interrupt(cpu: &mut Cpu) {
    let (seg, off) = cpu.bus.mscdex.request;
    let req = cpu.get_physical_addr(seg, off);
    let unit = cpu.bus.read_8(req + 1) as usize;
    let drive = cpu.bus.disk.drives_of_kind(DriveKind::CdRom).get(unit).copied();
    device_request(cpu, drive, req);
}

/// A sector address in the addressing mode of a request: 0 is HSG (the
/// sector number), 1 Red Book (minute, second, frame).
fn sector(mode: u8, address: u32) -> u32 {
    if mode == 1 { cdrom::redbook_to_lba(address) } else { address }
}

/// The real-mode far pointer at `at`, as a physical address.
fn far(cpu: &Cpu, at: usize) -> usize {
    cpu.get_physical_addr(cpu.bus.read_16(at + 2), cpu.bus.read_16(at))
}

/// Carry out the device driver request at `req` for CD drive `drive`.
fn device_request(cpu: &mut Cpu, drive: Option<u8>, req: usize) {
    // Audio and its position must be up to date for what follows.
    cpu.bus.audio_catch_up();
    let status = match drive {
        None => UNKNOWN_UNIT,
        Some(drive) => match cpu.bus.disk.cd_image(drive) {
            Some(image) => image_request(cpu, drive, &image, req),
            None => folder_request(cpu, req),
        },
    };
    let busy = drive.is_some_and(|d| cpu.bus.cdaudio.is_busy(d));
    cpu.bus.write_16(req + 3, status | if busy { BUSY } else { 0 });
}

/// A drive backed by a host directory: it opens and closes, and answers
/// the IOCTL queries that need no sectors.
fn folder_request(cpu: &mut Cpu, req: usize) -> u16 {
    match cpu.bus.read_8(req + 2) {
        0x03 => {
            let buf = far(cpu, req + 0x0E);
            match cpu.bus.read_8(buf) {
                0x06 => {
                    cpu.bus.write_32(buf + 1, DEVICE_STATUS | UNLOCKED);
                }
                0x07 => {
                    cpu.bus.write_8(buf + 1, 0); // cooked mode
                    cpu.bus.write_16(buf + 2, DATA_SECTOR as u16);
                }
                0x08 => {
                    cpu.bus.write_32(buf + 1, FOLDER_SECTORS);
                }
                // Media not changed.
                0x09 => {
                    cpu.bus.write_8(buf + 1, 1);
                }
                _ => return UNKNOWN_COMMAND,
            }
            DONE
        }
        // Input flush, device open, device close: nothing to do
        0x07 | 0x0D | 0x0E => DONE,
        _ => UNKNOWN_COMMAND,
    }
}

fn image_request(cpu: &mut Cpu, drive: u8, image: &Rc<CdImage>, req: usize) -> u16 {
    let command = cpu.bus.read_8(req + 2);
    let mode = cpu.bus.read_8(req + 0x0D);
    match command {
        0x03 => ioctl_input(cpu, drive, image, far(cpu, req + 0x0E)),
        0x0C => ioctl_output(cpu, drive, far(cpu, req + 0x0E)),
        // Input flush, device open, device close, output flush.
        0x07 | 0x0B | 0x0D | 0x0E => DONE,
        // Read Long: sectors cooked (2048 bytes) or raw (2352).
        0x80 => {
            let buffer = far(cpu, req + 0x0E);
            let count = cpu.bus.read_16(req + 0x12) as u32;
            let start = sector(mode, cpu.bus.read_32(req + 0x14));
            let raw = cpu.bus.read_8(req + 0x18) != 0;
            read_long(cpu, image, buffer, start, count, raw)
        }
        // Read Long Prefetch: a hint.
        0x82 => DONE,
        // Seek: the drive stops playing.
        0x83 => {
            cpu.bus.cdaudio.stop_drive(drive);
            DONE
        }
        // Play Audio: from a sector, for a number of sectors.
        0x84 => {
            let start = sector(mode, cpu.bus.read_32(req + 0x0E));
            let count = cpu.bus.read_32(req + 0x12);
            cpu.bus.log_string(&format!(
                "[MSCDEX] Play audio on {}: sector {} for {}",
                crate::disk::drive_letter(drive),
                start,
                count
            ));
            if count == 0 {
                cpu.bus.cdaudio.stop_drive(drive);
            } else {
                cpu.bus.cdaudio.play(drive, image.clone(), start, count);
            }
            DONE
        }
        // Stop Audio: pauses; a second stop forgets the position.
        0x85 => {
            if cpu.bus.cdaudio.drive() == Some(drive) {
                cpu.bus.cdaudio.stop();
            }
            DONE
        }
        // Resume Audio after a stop.
        0x88 => {
            if cpu.bus.cdaudio.drive() == Some(drive) && cpu.bus.cdaudio.resume() {
                DONE
            } else {
                GENERAL_FAILURE
            }
        }
        _ => UNKNOWN_COMMAND,
    }
}

fn read_long(cpu: &mut Cpu, image: &CdImage, buffer: usize, start: u32, count: u32, raw: bool) -> u16 {
    let mut data = [0u8; RAW_SECTOR];
    let size = if raw { RAW_SECTOR } else { DATA_SECTOR };
    for i in 0..count {
        let read = if raw {
            image.read_raw(start + i, &mut data)
        } else {
            image.read_data(start + i, (&mut data[..DATA_SECTOR]).try_into().unwrap())
        };
        if read.is_err() {
            return SECTOR_NOT_FOUND;
        }
        let at = buffer + i as usize * size;
        for (j, &b) in data[..size].iter().enumerate() {
            cpu.bus.write_8(at + j, b);
        }
    }
    DONE
}

/// Minute, second and frame of a time in sectors, without the two-second
/// offset of disc addresses.
fn duration_msf(sectors: u32) -> [u8; 3] {
    let seconds = sectors / cdrom::FRAMES_PER_SECOND;
    [(seconds / 60) as u8, (seconds % 60) as u8, (sectors % cdrom::FRAMES_PER_SECOND) as u8]
}

/// IOCTL input: the control block at `buf` starts with its code.
fn ioctl_input(cpu: &mut Cpu, drive: u8, image: &CdImage, buf: usize) -> u16 {
    let player = &cpu.bus.cdaudio;
    let position = match player.drive() {
        Some(d) if d == drive => player.position(),
        _ => 0,
    };
    let bus = &mut cpu.bus;
    match bus.read_8(buf) {
        // Address of the device header.
        0x00 => {
            bus.write_16(buf + 1, DEVICE_HEADER);
            bus.write_16(buf + 3, 0xF000);
        }
        // Location of the head, in the addressing mode asked for.
        0x01 => {
            let address = if bus.read_8(buf + 1) == 1 { cdrom::redbook(position) } else { position };
            bus.write_32(buf + 2, address);
        }
        // Audio channel info: input channel and volume per output.
        0x04 => {
            let channels = bus.cdaudio.channels;
            for (i, (input, volume)) in channels.into_iter().chain([(2, 0), (3, 0)]).enumerate() {
                bus.write_8(buf + 1 + i * 2, input);
                bus.write_8(buf + 2 + i * 2, volume);
            }
        }
        // Drive bytes: none.
        0x05 => {
            bus.write_8(buf + 1, 0);
        }
        0x06 => {
            let unlocked = if bus.mscdex.locked[drive as usize] { 0 } else { UNLOCKED };
            bus.write_32(buf + 1, DEVICE_STATUS | unlocked);
        }
        // Sector size in the read mode asked for.
        0x07 => {
            let size = if bus.read_8(buf + 1) == 1 { RAW_SECTOR } else { DATA_SECTOR };
            bus.write_16(buf + 2, size as u16);
        }
        // Volume size: the sectors up to the lead-out.
        0x08 => {
            bus.write_32(buf + 1, image.leadout());
        }
        // Media changed: once after a new disc, then not.
        0x09 => {
            let changed = std::mem::take(&mut bus.mscdex.changed[drive as usize]);
            bus.write_8(buf + 1, if changed { 0xFF } else { 1 });
        }
        // Audio disk info: first and last track, lead-out address.
        0x0A => {
            let tracks = image.tracks();
            bus.write_8(buf + 1, tracks.first().map_or(1, |t| t.number));
            bus.write_8(buf + 2, tracks.last().map_or(1, |t| t.number));
            bus.write_32(buf + 3, cdrom::redbook(image.leadout()));
        }
        // Audio track info: where a track starts and its control bits
        // (40h for data).
        0x0B => {
            let number = bus.read_8(buf + 1);
            let Some(track) = image.tracks().iter().find(|t| t.number == number) else {
                return SECTOR_NOT_FOUND;
            };
            bus.write_32(buf + 2, cdrom::redbook(track.start));
            bus.write_8(buf + 6, if track.is_audio() { 0x00 } else { 0x40 });
        }
        // Q channel: the track, index and times at the head.
        0x0C => {
            let track = image.track_at(position);
            let (control, number, index, relative) = match track {
                Some(t) if position < t.start => (t.is_audio(), t.number, 0, t.start - position),
                Some(t) => (t.is_audio(), t.number, 1, position - t.start),
                None => (false, 0, 0, 0),
            };
            bus.write_8(buf + 1, if control { 0x01 } else { 0x41 });
            bus.write_8(buf + 2, number);
            bus.write_8(buf + 3, index);
            for (i, b) in duration_msf(relative).into_iter().enumerate() {
                bus.write_8(buf + 4 + i, b);
            }
            bus.write_8(buf + 7, 0);
            let (m, s, f) = cdrom::lba_to_msf(position);
            for (i, b) in [m, s, f].into_iter().enumerate() {
                bus.write_8(buf + 8 + i, b);
            }
        }
        // UPC/EAN code: the disc has none.
        0x0E => {
            bus.write_8(buf + 1, 0x02);
            for i in 2..11 {
                bus.write_8(buf + i, 0);
            }
        }
        // Audio status: paused, and the range of the last Play.
        0x0F => {
            let paused = bus.cdaudio.drive() == Some(drive) && bus.cdaudio.state() == PlayState::Paused;
            let (start, end) = bus.cdaudio.range();
            bus.write_16(buf + 1, paused as u16);
            bus.write_32(buf + 3, cdrom::redbook(start));
            bus.write_32(buf + 7, cdrom::redbook(end));
        }
        _ => return UNKNOWN_COMMAND,
    }
    DONE
}

/// IOCTL output: the control block at `buf` starts with its code.
fn ioctl_output(cpu: &mut Cpu, drive: u8, buf: usize) -> u16 {
    let bus = &mut cpu.bus;
    match bus.read_8(buf) {
        // Eject, reset: the drive stops playing.
        0x00 | 0x02 => bus.cdaudio.stop_drive(drive),
        // Lock or unlock the door.
        0x01 => bus.mscdex.locked[drive as usize] = bus.read_8(buf + 1) != 0,
        // Audio channel control: input channel and volume per output.
        0x03 => {
            for i in 0..2 {
                bus.cdaudio.channels[i] = (bus.read_8(buf + 1 + i * 2), bus.read_8(buf + 2 + i * 2));
            }
        }
        // Close the tray.
        0x05 => {}
        _ => return UNKNOWN_COMMAND,
    }
    DONE
}

/// Put the CD driver's device header in the ROM, filled in for the
/// mounted CD drives, and chain it after NUL (at `nul`) when there are any.
pub fn install_device(bus: &mut Bus, nul: usize) {
    let header = 0xF0000 + DEVICE_HEADER as usize;
    let cd_drives = bus.disk.drives_of_kind(DriveKind::CdRom);
    bus.write_32(header, 0xFFFF_FFFF);
    bus.write_16(header + 0x04, 0xC800); // character device, IOCTL, open/close
    bus.write_16(header + 0x06, STRATEGY_ENTRY);
    bus.write_16(header + 0x08, INTERRUPT_ENTRY);
    for (i, &b) in b"MSCD001 ".iter().enumerate() {
        bus.write_8(header + 0x0A + i, b);
    }
    bus.write_16(header + 0x12, 0);
    bus.write_8(header + 0x14, cd_drives.first().map_or(0, |d| d + 1));
    bus.write_8(header + 0x15, cd_drives.len() as u8);
    for (entry, service) in [(STRATEGY_ENTRY, crate::bios::SERVICE_CD_STRATEGY), (INTERRUPT_ENTRY, crate::bios::SERVICE_CD_INTERRUPT)] {
        let at = 0xF0000 + entry as usize;
        for (i, b) in [0xFE, 0x39, service, 0xCB].into_iter().enumerate() {
            bus.write_8(at + i, b);
        }
    }
    if !cd_drives.is_empty() {
        bus.write_16(nul, DEVICE_HEADER);
        bus.write_16(nul + 2, 0xF000);
    }
}

crate::state_fields!(MscdexState { request, changed, locked });
