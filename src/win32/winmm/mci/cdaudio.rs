//! MCI's CD audio device (cdaudio) on the emulated CD drive, whose
//! player the bus mixes (Bus::cdaudio), as MSCDEX's Play Audio uses it.
//!
//! Positions are in milliseconds, MSF (minutes, seconds and frames of
//! the disc, track 1 at 00:02:00) or TMSF (a track and the time in it),
//! as `set time format` has them; MSF at first. A play goes on when the
//! device closes, as a drive plays on once told to, and only the
//! notification is lost.

use super::*;
use crate::cdrom::audio::PlayState;
use crate::cdrom::image::CdImage;
use crate::disk::DriveKind;
use std::rc::Rc;

/// The frames (sectors) of a second, and those before the first track.
const FRAMES: u32 = 75;
const LEAD_IN: u32 = 150;

/// MCI_CDA_STATUS_TYPE_TRACK, and what it returns.
const MCI_CDA_STATUS_TYPE_TRACK: u32 = 0x4001;
const MCI_CDA_TRACK_AUDIO: u32 = 1088;
const MCI_CDA_TRACK_OTHER: u32 = 1089;

/// The device's state.
#[derive(Clone, Debug, Default)]
pub struct Cd {
    /// The drive (0 for A:).
    pub drive: u8,
    /// Where it is when it doesn't play (a sector).
    pub pos: u32,
    /// When the play ends, by the clock's microseconds, while it plays.
    pub end_at: Option<u64>,
    /// The sector after the play's last.
    pub to: u32,
    pub door_open: bool,
}

fn cd(cpu: &mut Cpu, i: usize) -> &mut Cd {
    &mut device(cpu, i).cd
}

/// The disc in the device's drive.
fn image(cpu: &Cpu, i: usize) -> Option<Rc<CdImage>> {
    let d = &cpu.bus.win32.winmm.mci.devices[i].cd;
    if d.door_open {
        return None;
    }
    let disk = &cpu.bus.disk;
    disk.cd_image(d.drive)
        .or_else(|| disk.boot_cd_image(d.drive))
}

/// Open the device: on the drive its element names ("E:"), or the first
/// CD-ROM drive.
pub fn open(cpu: &mut Cpu, i: usize) -> u32 {
    let element = device(cpu, i).element.clone();
    let drives = cpu.bus.disk.drives_of_kind(DriveKind::CdRom);
    let drive = match element.as_bytes() {
        [letter, b':', ..] => {
            let d = letter.to_ascii_uppercase().wrapping_sub(b'A');
            if !drives.contains(&d) {
                return MCIERR_INVALID_FILE;
            }
            d
        }
        _ => match drives.first() {
            Some(&d) => d,
            None => return MCIERR_CANNOT_LOAD_DRIVER,
        },
    };
    cd(cpu, i).drive = drive;
    let start = image(cpu, i).and_then(|img| img.tracks().first().map(|t| t.start));
    cd(cpu, i).pos = start.unwrap_or(0);
    0
}

/// Close the device: a play goes on.
pub fn close(cpu: &mut Cpu, i: usize) {
    cd(cpu, i).end_at = None;
}

/// Whether the drive's player plays (or holds a pause) for the device.
fn holds(cpu: &Cpu, i: usize) -> Option<PlayState> {
    let d = &cpu.bus.win32.winmm.mci.devices[i].cd;
    let player = &cpu.bus.cdaudio;
    (player.drive() == Some(d.drive) && player.state() != PlayState::Stopped)
        .then(|| player.state())
}

/// Where the device is: the sector it plays, or where it stopped.
fn position(cpu: &Cpu, i: usize) -> u32 {
    match holds(cpu, i) {
        Some(_) => cpu.bus.cdaudio.position(),
        None => cpu.bus.win32.winmm.mci.devices[i].cd.pos,
    }
}

/// The microseconds `sectors` sectors play for.
fn duration(sectors: u32) -> u64 {
    (sectors as u64 * 1_000_000).div_ceil(FRAMES as u64)
}

/// MSF of a count of frames, packed as MCI_MAKE_MSF does.
fn msf(frames: u32) -> u32 {
    let (m, s, f) = (
        frames / (60 * FRAMES),
        frames / FRAMES % 60,
        frames % FRAMES,
    );
    m.min(255) | s << 8 | f << 16
}

/// The frames of MSF bytes.
fn frames_of_msf(m: u32, s: u32, f: u32) -> u32 {
    (m * 60 + s) * FRAMES + f
}

/// A position in the time format `format`, and how it reads.
fn from_sector(img: &CdImage, format: u32, lba: u32) -> (u32, Ret) {
    match format {
        MCI_FORMAT_TMSF => {
            let tracks = img.tracks();
            let t = tracks
                .iter()
                .rev()
                .find(|t| t.pregap_start <= lba)
                .or(tracks.first());
            let (number, start) = t.map_or((1, 0), |t| (t.number as u32, t.start));
            let rel = msf(lba.saturating_sub(start));
            (number | rel << 8, Ret::Colon4)
        }
        MCI_FORMAT_MSF => (msf(lba + LEAD_IN), Ret::Colon3),
        _ => (
            ((lba + LEAD_IN) as u64 * 1000 / FRAMES as u64) as u32,
            Ret::Int,
        ),
    }
}

/// A length of `sectors` in the time format: MSF for MSF and TMSF.
fn length(format: u32, sectors: u32) -> (u32, Ret) {
    match format {
        MCI_FORMAT_MSF | MCI_FORMAT_TMSF => (msf(sectors), Ret::Colon3),
        _ => ((sectors as u64 * 1000 / FRAMES as u64) as u32, Ret::Int),
    }
}

/// The sector a position in the time format names, or the error.
fn to_sector(img: &CdImage, format: u32, value: u32) -> Result<u32, u32> {
    let [b0, b1, b2, b3] = value.to_le_bytes().map(u32::from);
    match format {
        MCI_FORMAT_TMSF => {
            let t = img
                .tracks()
                .iter()
                .find(|t| t.number as u32 == b0)
                .ok_or(MCIERR_OUTOFRANGE)?;
            if b2 >= 60 || b3 >= FRAMES {
                return Err(MCIERR_OUTOFRANGE);
            }
            Ok(t.start + frames_of_msf(b1, b2, b3))
        }
        MCI_FORMAT_MSF => {
            if b1 >= 60 || b2 >= FRAMES {
                return Err(MCIERR_OUTOFRANGE);
            }
            Ok(frames_of_msf(b0, b1, b2).saturating_sub(LEAD_IN))
        }
        _ => Ok(((value as u64 * FRAMES as u64 / 1000) as u32).saturating_sub(LEAD_IN)),
    }
}

/// Stop the device's play, and keep where it was.
fn stop(cpu: &mut Cpu, i: usize) {
    cpu.bus.audio_catch_up();
    if holds(cpu, i).is_some() {
        let at = cpu.bus.cdaudio.position();
        cd(cpu, i).pos = at;
        cpu.bus.cdaudio.reset();
    }
    cd(cpu, i).end_at = None;
}

/// The device's commands.
pub fn command(cpu: &mut Cpu, i: usize, msg: u32, flags: u32, p: &mut Parms) -> u32 {
    let format = device(cpu, i).time_format;
    match msg {
        MCI_PLAY => play(cpu, i, flags, p),
        MCI_SEEK => {
            let Some(img) = image(cpu, i) else {
                return MCIERR_DEVICE_NOT_READY;
            };
            let to = if flags & MCI_SEEK_TO_START != 0 {
                img.tracks().first().map_or(0, |t| t.start)
            } else if flags & MCI_SEEK_TO_END != 0 {
                img.leadout()
            } else if flags & MCI_TO != 0 {
                match to_sector(&img, format, p.to) {
                    Ok(s) if s <= img.leadout() => s,
                    Ok(_) => return MCIERR_OUTOFRANGE,
                    Err(e) => return e,
                }
            } else {
                return MCIERR_MISSING_PARAMETER;
            };
            stop(cpu, i);
            cd(cpu, i).pos = to;
            0
        }
        MCI_STOP => {
            stop(cpu, i);
            0
        }
        MCI_PAUSE => {
            cpu.bus.audio_catch_up();
            if holds(cpu, i) == Some(PlayState::Playing) {
                cpu.bus.cdaudio.stop();
                let at = cpu.bus.cdaudio.position();
                cd(cpu, i).pos = at;
            }
            cd(cpu, i).end_at = None;
            0
        }
        MCI_RESUME => {
            cpu.bus.audio_catch_up();
            if holds(cpu, i) == Some(PlayState::Paused) && cpu.bus.cdaudio.resume() {
                let left = cd(cpu, i).to.saturating_sub(cpu.bus.cdaudio.position());
                let now = cpu.bus.clock.now_micros();
                cd(cpu, i).end_at = Some(now + duration(left));
                super::super::schedule(cpu);
            }
            0
        }
        MCI_STATUS => status(cpu, i, flags, p),
        MCI_SET => set(cpu, i, flags, p),
        MCI_GETDEVCAPS => {
            if flags & MCI_GETDEVCAPS_ITEM == 0 {
                return MCIERR_MISSING_PARAMETER;
            }
            let (ret, kind) = match p.item {
                MCI_GETDEVCAPS_DEVICE_TYPE => (MCI_DEVTYPE_CD_AUDIO, Ret::Constant),
                MCI_GETDEVCAPS_HAS_AUDIO | MCI_GETDEVCAPS_CAN_EJECT | MCI_GETDEVCAPS_CAN_PLAY => {
                    (TRUE, Ret::Bool)
                }
                MCI_GETDEVCAPS_CAN_RECORD
                | MCI_GETDEVCAPS_HAS_VIDEO
                | MCI_GETDEVCAPS_USES_FILES
                | MCI_GETDEVCAPS_COMPOUND_DEVICE
                | MCI_GETDEVCAPS_CAN_SAVE => (FALSE, Ret::Bool),
                _ => return MCIERR_UNSUPPORTED_FUNCTION,
            };
            p.ret = ret;
            p.ret_kind = kind;
            0
        }
        MCI_INFO => {
            p.ret_kind = Ret::Text;
            if flags & MCI_INFO_PRODUCT != 0 {
                p.text = "CD Audio".to_string();
            } else if flags & MCI_INFO_MEDIA_IDENTITY != 0 {
                let Some(img) = image(cpu, i) else {
                    return MCIERR_DEVICE_NOT_READY;
                };
                // A number of the disc's table of contents.
                let id = img
                    .tracks()
                    .iter()
                    .fold(img.leadout(), |h, t| h.rotate_left(5) ^ t.start);
                p.text = format!("{:08x}", id);
            } else if flags & MCI_INFO_MEDIA_UPC != 0 {
                return MCIERR_NO_IDENTITY;
            } else if flags & MCI_INFO_FILE != 0 {
                return MCIERR_UNSUPPORTED_FUNCTION;
            } else {
                return MCIERR_MISSING_PARAMETER;
            }
            0
        }
        MCI_BREAK => 0,
        _ => MCIERR_UNSUPPORTED_FUNCTION,
    }
}

/// MCI_PLAY: from `from` (or where it is) to `to` (or the end).
fn play(cpu: &mut Cpu, i: usize, flags: u32, p: &mut Parms) -> u32 {
    let Some(img) = image(cpu, i) else {
        return MCIERR_DEVICE_NOT_READY;
    };
    let format = device(cpu, i).time_format;
    let end = img.leadout();
    let from = if flags & MCI_FROM != 0 {
        match to_sector(&img, format, p.from) {
            Ok(s) => s,
            Err(e) => return e,
        }
    } else {
        position(cpu, i)
    };
    let to = if flags & MCI_TO != 0 {
        match to_sector(&img, format, p.to) {
            Ok(s) => s,
            Err(e) => return e,
        }
    } else {
        end
    };
    if from >= end || to > end || to < from {
        return MCIERR_OUTOFRANGE;
    }
    stop(cpu, i);
    cd(cpu, i).pos = from;
    if to == from {
        return 0;
    }
    let drive = cd(cpu, i).drive;
    cpu.bus.cdaudio.play(drive, img, from, to - from);
    let now = cpu.bus.clock.now_micros();
    let c = cd(cpu, i);
    c.to = to;
    c.end_at = Some(now + duration(to - from));
    0
}

/// MCI_STATUS.
fn status(cpu: &mut Cpu, i: usize, flags: u32, p: &mut Parms) -> u32 {
    if flags & MCI_STATUS_ITEM == 0 {
        return MCIERR_MISSING_PARAMETER;
    }
    let format = device(cpu, i).time_format;
    let img = image(cpu, i);
    let (ret, kind) = match p.item {
        MCI_STATUS_MODE => {
            let mode = if cd(cpu, i).door_open {
                MCI_MODE_OPEN
            } else if img.is_none() {
                MCI_MODE_NOT_READY
            } else {
                match holds(cpu, i) {
                    Some(PlayState::Playing) => MCI_MODE_PLAY,
                    Some(PlayState::Paused) => MCI_MODE_PAUSE,
                    _ => MCI_MODE_STOP,
                }
            };
            (mode, Ret::Constant)
        }
        MCI_STATUS_MEDIA_PRESENT | MCI_STATUS_READY => (img.is_some() as u32, Ret::Bool),
        MCI_STATUS_TIME_FORMAT => (format, Ret::TimeFormat),
        item => {
            let Some(img) = img else {
                return MCIERR_DEVICE_NOT_READY;
            };
            let tracks = img.tracks();
            let track = || {
                tracks
                    .iter()
                    .find(|t| t.number as u32 == p.track)
                    .ok_or(MCIERR_OUTOFRANGE)
            };
            match item {
                MCI_STATUS_LENGTH => {
                    let sectors = if flags & MCI_TRACK != 0 {
                        match track() {
                            Ok(t) => t.end - t.start,
                            Err(e) => return e,
                        }
                    } else {
                        img.leadout() - tracks.first().map_or(0, |t| t.start)
                    };
                    length(format, sectors)
                }
                MCI_STATUS_POSITION => {
                    let at = if flags & MCI_TRACK != 0 {
                        match track() {
                            Ok(t) => t.start,
                            Err(e) => return e,
                        }
                    } else if flags & MCI_STATUS_START != 0 {
                        tracks.first().map_or(0, |t| t.start)
                    } else {
                        cpu.bus.audio_catch_up();
                        position(cpu, i)
                    };
                    from_sector(&img, format, at)
                }
                MCI_STATUS_NUMBER_OF_TRACKS => (tracks.len() as u32, Ret::Int),
                MCI_STATUS_CURRENT_TRACK => {
                    cpu.bus.audio_catch_up();
                    let at = position(cpu, i);
                    let t = tracks.iter().rev().find(|t| t.pregap_start <= at);
                    (t.map_or(1, |t| t.number as u32), Ret::Int)
                }
                MCI_CDA_STATUS_TYPE_TRACK => {
                    if flags & MCI_TRACK == 0 {
                        return MCIERR_MISSING_PARAMETER;
                    }
                    match track() {
                        Ok(t) if t.is_audio() => (MCI_CDA_TRACK_AUDIO, Ret::Constant),
                        Ok(_) => (MCI_CDA_TRACK_OTHER, Ret::Constant),
                        Err(e) => return e,
                    }
                }
                _ => return MCIERR_UNSUPPORTED_FUNCTION,
            }
        }
    };
    p.ret = ret;
    p.ret_kind = kind;
    0
}

/// MCI_SET: the time format, the door, and the channels.
fn set(cpu: &mut Cpu, i: usize, flags: u32, p: &mut Parms) -> u32 {
    if flags
        & (MCI_SET_TIME_FORMAT
            | MCI_SET_DOOR_OPEN
            | MCI_SET_DOOR_CLOSED
            | MCI_SET_AUDIO
            | MCI_SET_VIDEO)
        == 0
    {
        return MCIERR_MISSING_PARAMETER;
    }
    if flags & MCI_SET_VIDEO != 0 {
        return MCIERR_UNSUPPORTED_FUNCTION;
    }
    if flags & MCI_SET_TIME_FORMAT != 0 {
        match p.time_format {
            MCI_FORMAT_MILLISECONDS | MCI_FORMAT_MSF | MCI_FORMAT_TMSF => {
                device(cpu, i).time_format = p.time_format;
            }
            _ => return MCIERR_BAD_TIME_FORMAT,
        }
    }
    if flags & MCI_SET_DOOR_OPEN != 0 {
        stop(cpu, i);
        cd(cpu, i).door_open = true;
    }
    if flags & MCI_SET_DOOR_CLOSED != 0 {
        cd(cpu, i).door_open = false;
        let start = image(cpu, i).and_then(|img| img.tracks().first().map(|t| t.start));
        cd(cpu, i).pos = start.unwrap_or(0);
    }
    if flags & MCI_SET_AUDIO != 0 {
        if flags & (MCI_SET_ON | MCI_SET_OFF) == 0 {
            return MCIERR_MISSING_PARAMETER;
        }
        let volume = if flags & MCI_SET_ON != 0 { 0xFF } else { 0 };
        cpu.bus.audio_catch_up();
        let channels = &mut cpu.bus.cdaudio.channels;
        match p.audio {
            0 => {
                channels[0].1 = volume;
                channels[1].1 = volume;
            }
            1 => channels[0].1 = volume,
            2 => channels[1].1 = volume,
            _ => return MCIERR_OUTOFRANGE,
        }
    }
    0
}

/// The device looks at the time `now`: whether its play ended.
pub fn poll(cpu: &mut Cpu, i: usize, now: u64) -> bool {
    let Some(end_at) = cd(cpu, i).end_at else {
        return false;
    };
    if end_at > now {
        return false;
    }
    cpu.bus.audio_catch_up();
    let to = cd(cpu, i).to;
    if holds(cpu, i) == Some(PlayState::Playing) && cpu.bus.cdaudio.range().1 == to {
        // The player is a little behind the clock: a moment more.
        let left = to.saturating_sub(cpu.bus.cdaudio.position());
        cd(cpu, i).end_at = Some(now + duration(left.max(1)));
        return false;
    }
    let c = cd(cpu, i);
    c.end_at = None;
    c.pos = to;
    true
}

crate::state_fields!(Cd {
    drive,
    pos,
    end_at,
    to,
    door_open
});
