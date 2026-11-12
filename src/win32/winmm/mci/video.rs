//! MCI's digitalvideo devices: MPEGVideo, which plays MPEG-1 and AVI
//! files as Windows 98's MCIQTZ does with ActiveMovie, and AVIVideo,
//! which plays AVI files as its MCIAVI does (media.rs). They show the
//! pictures in a window and play the sound on the built-in sound
//! device, and differ in the files they take and their window's class
//! and caption.
//!
//! The device paints a window of its own, made when it opens: a child of
//! the window MCI_DGV_OPEN_PARENT names, with MCI_DGV_OPEN_WS's style,
//! or else an overlapped window, which shows when a play starts.
//! MCI_WINDOW's handle has it paint another window instead. The picture
//! is stretched from the source rectangle to the destination one, the
//! window's client area unless MCI_PUT sets it. MCI_WHERE and MCI_PUT
//! give and take a rectangle's width and height where RECT has its
//! right and bottom, as MCI does. A play with MCIAVI's "fullscreen"
//! draws into a window that covers the screen while it plays
//! (`full_screen`).
//!
//! A play shows the frame its emulated time is at on the process's
//! multimedia thread (`poll`), drawn as StretchDIBits draws, and the
//! sound plays as one sound from where the play starts. Positions are
//! in milliseconds or frames. The device's own window draws its frame
//! again when it gets WM_PAINT (`window_proc`).
//!
//! The window's calls into USER32 (CreateWindowExA, ShowWindow,
//! SetWindowTextA, MoveWindow) send its window procedures messages, so
//! a command that makes one returns `NEEDS_CALL`; the function that ran
//! it makes the call (`call_step`) and runs again, and the command then
//! finds what the call returned (`made`). Each thread keeps its own
//! calls. Closing posts the window WM_CLOSE, which destroys it.
//!
//! A save state keeps where the device is, not the file: it is read
//! again when it is next wanted.

use super::media::{Kind as MediaKind, Media, Pictures};
use super::*;
use crate::win32::gdi32::draw::{self, Source};
use crate::win32::gdi32::palette;
use crate::win32::gdi32::surface::{Surface, default_masks, dib_stride};
use crate::win32::task::{self, Next, Task};
use crate::win32::user32::{self, window};
use crate::win32::winmm::{wave, write_text};
use std::sync::Arc;

/// The device type, and MPEGVideo's kind among MCI's devices.
pub const MCI_DEVTYPE_DIGITAL_VIDEO: u32 = 520;
/// AVIVideo's kind: the same device type, with a flag above it that
/// tells the two apart.
pub const AVI_VIDEO: u32 = 0x1_0000 | MCI_DEVTYPE_DIGITAL_VIDEO;

/// The digitalvideo commands.
pub const MCI_REALIZE: u32 = 0x0840;
pub const MCI_WINDOW: u32 = 0x0841;
pub const MCI_PUT: u32 = 0x0842;
pub const MCI_WHERE: u32 = 0x0843;
pub const MCI_UPDATE: u32 = 0x0854;

/// MCI_OPEN's flags of digitalvideo: the window's style and parent.
pub const MCI_DGV_OPEN_WS: u32 = 0x0001_0000;
pub const MCI_DGV_OPEN_PARENT: u32 = 0x0002_0000;

/// MCI_PLAY's.
const MCI_DGV_PLAY_REPEAT: u32 = 0x0001_0000;
const MCI_DGV_PLAY_REVERSE: u32 = 0x0002_0000;
/// MCIAVI's (mciavi.h), which MCIQTZ takes too: the play shows in the
/// device's window, or covers the screen (`full_screen`), the picture
/// twice its size with FULLBY2.
pub const MCI_MCIAVI_PLAY_WINDOW: u32 = 0x0100_0000;
pub const MCI_MCIAVI_PLAY_FULLSCREEN: u32 = 0x0200_0000;
pub const MCI_MCIAVI_PLAY_FULLBY2: u32 = 0x0400_0000;

/// MCI_WINDOW's.
pub const MCI_DGV_WINDOW_HWND: u32 = 0x0001_0000;
pub const MCI_DGV_WINDOW_STATE: u32 = 0x0004_0000;
pub const MCI_DGV_WINDOW_TEXT: u32 = 0x0008_0000;
const MCI_DGV_WINDOW_DEFAULT: u32 = 0;

/// MCI_REALIZE's.
pub const MCI_DGV_REALIZE_NORM: u32 = 0x0001_0000;
pub const MCI_DGV_REALIZE_BKGD: u32 = 0x0002_0000;

/// MCI_PUT's and MCI_WHERE's, and MCI_UPDATE's.
pub const MCI_DGV_RECT: u32 = 0x0001_0000;
pub const MCI_DGV_PUT_SOURCE: u32 = 0x0002_0000;
pub const MCI_DGV_PUT_DESTINATION: u32 = 0x0004_0000;
const MCI_DGV_PUT_FRAME: u32 = 0x0008_0000;
const MCI_DGV_PUT_VIDEO: u32 = 0x0010_0000;
pub const MCI_DGV_PUT_WINDOW: u32 = 0x0020_0000;
const MCI_DGV_WHERE_MAX: u32 = 0x0040_0000;
pub const MCI_DGV_UPDATE_HDC: u32 = 0x0002_0000;

/// MCI_STATUS's items: the window; whether it shows, is minimized, is
/// maximized.
pub const MCI_DGV_STATUS_HWND: u32 = 0x4001;
pub const MCI_DGV_STATUS_WINDOW_VISIBLE: u32 = 0x4021;
pub const MCI_DGV_STATUS_WINDOW_MINIMIZED: u32 = 0x4022;
pub const MCI_DGV_STATUS_WINDOW_MAXIMIZED: u32 = 0x4023;

/// MCI_STEP's.
const MCI_DGV_STEP_REVERSE: u32 = 0x0001_0000;
const MCI_DGV_STEP_FRAMES: u32 = 0x0002_0000;

/// The time formats besides milliseconds.
const MCI_FORMAT_FRAMES: u32 = 3;

const MCIERR_NONAPPLICABLE_FUNCTION: u32 = 302;
const MCIERR_NO_WINDOW: u32 = 346;

/// What a command returns when it needs a call into USER32 first.
pub const NEEDS_CALL: u32 = 0xFFFF_0001;

/// The class of MPEGVideo's window, and its caption; AVIVideo's class,
/// whose windows have the file's name as their caption.
pub const CLASS: &str = "VideoRenderer";
const CAPTION: &str = "ActiveMovie Window";
pub const AVI_CLASS: &str = "AVIWnd32";

/// Whether the device `i` is MPEGVideo.
fn is_mpeg(cpu: &mut Cpu, i: usize) -> bool {
    device(cpu, i).kind == MCI_DEVTYPE_DIGITAL_VIDEO
}

const WM_CLOSE: u32 = 0x0010;
const SRCCOPY: u32 = 0x00CC_0020;
const BLACKNESS: u32 = 0x0000_0042;

/// The device's state, in its `Device`.
#[derive(Clone, Debug, Default)]
pub struct Video {
    /// The window it paints, and the one it made, which closes with it.
    pub hwnd: u32,
    pub own: u32,
    /// The source and destination rectangles, as left, top, width and
    /// height: the whole picture, and the client area for None.
    pub source: [i32; 4],
    pub dest: Option<[i32; 4]>,
    /// Where the device is, in milliseconds: where a play started, while
    /// it plays. Where the play ends.
    pub pos: u32,
    pub to: u32,
    /// When the play started, in the clock's microseconds, while it
    /// plays; whether it plays again from the start when it ends; its
    /// sound.
    pub started: Option<u64>,
    pub repeat: bool,
    pub sound: u32,
    pub paused: bool,
    /// MCI_SET_AUDIO's off.
    pub muted: bool,
    /// The sound's volume and balance, as a DirectShow graph that plays
    /// through the device sets them (IBasicAudio): hundredths of a
    /// decibel, 0 for full, and -10000 for the left channel's silence to
    /// 10000 for the right's. MCI leaves them at 0.
    pub volume: i32,
    pub balance: i32,
    /// Whether a play leaves the device's own window hidden: a graph's
    /// with IVideoWindow's AutoShow off.
    pub manual_show: bool,
    /// The frame drawn last.
    pub drawn: Option<u32>,
    /// The file, which a save state doesn't keep, and its decoder.
    pub media: Option<Arc<Media>>,
    pub pictures: Pictures,
    /// On a palette device, the palette the pictures are drawn with,
    /// once one was drawn; 0 before.
    pub palette: u32,
    /// Whether the palette is realized as a background palette: the
    /// last MCI_REALIZE had MCI_DGV_REALIZE_BKGD.
    pub background: bool,
    /// While a full-screen play goes on, the window that covers the
    /// screen (`full_screen`), or 0; whether the picture is twice its
    /// size.
    pub cover: u32,
    pub by_two: bool,
}

/// A call into USER32: the function, its arguments, and the strings it
/// passes, each with the place of the argument that points to it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Call {
    pub target: u32,
    pub args: Vec<u32>,
    pub strings: Vec<(u32, String)>,
}

/// A thread's command's calls into USER32, which the thread makes before
/// the command runs again: those made, with what they returned, and the
/// one it asks for next. Each thread has its own, so that commands two
/// threads send at once don't take each other's calls.
#[derive(Clone, Debug, Default)]
pub struct Calls {
    pub tid: u32,
    pub made: Vec<(Call, u32)>,
    pub next: Option<Call>,
    /// The file the command read, so that it isn't read again.
    pub media: Option<(String, Arc<Media>)>,
}

/// The bytes of a call's scratch space for its strings.
const SCRATCH: u32 = 1024;

fn video(cpu: &mut Cpu, i: usize) -> &mut Video {
    &mut device(cpu, i).video
}

/// The running thread's calls, made empty if it has none.
fn calls(cpu: &mut Cpu) -> &mut Calls {
    let tid = user32::current_tid(cpu);
    let list = &mut mci(cpu).calls;
    let k = match list.iter().position(|c| c.tid == tid) {
        Some(k) => k,
        None => {
            list.push(Calls {
                tid,
                ..Calls::default()
            });
            list.len() - 1
        }
    };
    &mut list[k]
}

/// What the call `api(args)` returned, if the command made it; else it
/// asks for the call, and the command returns NEEDS_CALL. `strings` are
/// the strings the call passes, each with the place of its argument.
pub fn made(cpu: &mut Cpu, api: &str, args: &[u32], strings: &[(u32, &str)]) -> Option<u32> {
    let call = Call {
        target: user32::address_of(api),
        args: args.to_vec(),
        strings: strings.iter().map(|&(k, s)| (k, s.to_string())).collect(),
    };
    let calls = calls(cpu);
    if let Some((_, value)) = calls.made.iter().find(|(c, _)| *c == call) {
        return Some(*value);
    }
    calls.next = Some(call);
    None
}

/// The running thread's command is done: the calls it made are
/// forgotten.
pub fn command_done(cpu: &mut Cpu) {
    let tid = user32::current_tid(cpu);
    mci(cpu).calls.retain(|c| c.tid != tid);
}

/// The function that ran a command that returned NEEDS_CALL makes the
/// call (its handler returns this), and then runs again.
pub fn make_call(cpu: &mut Cpu) -> Option<u32> {
    task::start(cpu, task::MCI_CALL, Vec::new(), SCRATCH)
}

/// The task of `make_call`.
pub fn call_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    match t.step {
        0 => {
            let Some(call) = calls(cpu).next.clone() else {
                command_done(cpu);
                return Next::Done(MCIERR_INTERNAL);
            };
            // The strings go to the scratch space, one after another.
            let mut args = call.args;
            let mut at = t.scratch;
            let room = SCRATCH / call.strings.len().max(1) as u32;
            for (k, s) in &call.strings {
                let len = (s.len() as u32 + 1).min(room);
                if write_text(cpu, at, len, s).is_none() {
                    command_done(cpu);
                    return Next::Done(MCIERR_INTERNAL);
                }
                if let Some(a) = args.get_mut(*k as usize) {
                    *a = at;
                }
                at += len;
            }
            Next::call(call.target, &args, 1)
        }
        _ => {
            let calls = calls(cpu);
            if let Some(call) = calls.next.take() {
                calls.made.push((call, value));
            }
            t.retry(cpu);
            Next::Stop
        }
    }
}

/// The open device `i`'s file, read again after a restore.
pub fn media(cpu: &mut Cpu, i: usize) -> Option<Arc<Media>> {
    if let Some(m) = &video(cpu, i).media {
        return Some(m.clone());
    }
    let element = device(cpu, i).element.clone();
    if element.is_empty() {
        return None;
    }
    let m = Arc::new(read(cpu, i, &element).ok()?);
    video(cpu, i).media = Some(m.clone());
    Some(m)
}

/// Reads the file `path`, for the device `i`.
fn read(cpu: &mut Cpu, i: usize, path: &str) -> Result<Media, u32> {
    let bytes = super::media::read_file(cpu, path).ok_or(MCIERR_FILE_NOT_FOUND)?;
    Media::parse(&bytes, is_mpeg(cpu, i)).ok_or(MCIERR_INVALID_FILE)
}

/// The driver: the command `msg` of the device `i`.
pub fn command(cpu: &mut Cpu, i: usize, msg: u32, flags: u32, p: &mut Parms) -> u32 {
    match msg {
        MCI_OPEN => open(cpu, i, flags, p),
        MCI_CLOSE => {
            halt(cpu, i);
            leave_full_screen(cpu, i);
            // The device's palette is the system's (`add_system`), so it
            // is still there when a process that ended closes its
            // devices. Only a palette is deleted: DeleteObject of another
            // handle sets the last error, of a process that may be gone.
            let palette = std::mem::take(&mut video(cpu, i).palette);
            if crate::win32::gdi32::gdi(cpu).palette(palette).is_some() {
                crate::win32::gdi32::delete_object(cpu, &[palette]);
            }
            // A process that ended has no windows left, and no process
            // to post to: its devices close after USER32's windows go.
            let own = video(cpu, i).own;
            if own != 0 && user32::user(cpu).is_window(own) {
                user32::message::post_message(cpu, &[own, WM_CLOSE, 0, 0]);
            }
            0
        }
        MCI_PLAY => play(cpu, i, flags, p),
        MCI_STOP => {
            halt(cpu, i);
            leave_full_screen(cpu, i);
            video(cpu, i).paused = false;
            0
        }
        MCI_PAUSE => {
            if video(cpu, i).started.is_some() {
                halt(cpu, i);
                leave_full_screen(cpu, i);
                video(cpu, i).paused = true;
            }
            0
        }
        MCI_RESUME => {
            if video(cpu, i).paused {
                video(cpu, i).paused = false;
                start(cpu, i);
            }
            0
        }
        MCI_CUE => 0,
        MCI_SEEK => seek(cpu, i, flags, p),
        MCI_STEP => step(cpu, i, flags, p),
        MCI_STATUS => status(cpu, i, flags, p),
        MCI_SET => set(cpu, i, flags, p),
        MCI_GETDEVCAPS => capabilities(flags, p),
        MCI_INFO => {
            if flags & MCI_INFO_PRODUCT != 0 {
                p.text = if is_mpeg(cpu, i) {
                    "Built-in Windows MPEG video"
                } else {
                    "Built-in Windows AVI video"
                }
                .to_string();
            } else if flags & MCI_INFO_FILE != 0 {
                p.text = device(cpu, i).element.clone();
            } else {
                return MCIERR_MISSING_PARAMETER;
            }
            p.ret_kind = Ret::Text;
            0
        }
        MCI_REALIZE => realize(cpu, i, flags),
        MCI_WINDOW => set_window(cpu, i, flags, p),
        MCI_PUT => put(cpu, i, flags, p),
        MCI_WHERE => place(cpu, i, flags, p),
        MCI_UPDATE => update(cpu, i, flags, p),
        MCI_RECORD | MCI_SAVE => MCIERR_UNSUPPORTED_FUNCTION,
        _ => MCIERR_UNRECOGNIZED_COMMAND,
    }
}

/// MCI_OPEN: the file, and the window.
fn open(cpu: &mut Cpu, i: usize, flags: u32, p: &Parms) -> u32 {
    let element = device(cpu, i).element.clone();
    let media = if element.is_empty() {
        None
    } else {
        let cached = calls(cpu)
            .media
            .as_ref()
            .filter(|(e, _)| *e == element)
            .map(|(_, m)| m.clone());
        let m = match cached {
            Some(m) => m,
            None => match read(cpu, i, &element) {
                Ok(m) => Arc::new(m),
                Err(e) => return e,
            },
        };
        Some(m)
    };
    let (w, h) = media
        .as_ref()
        .filter(|m| m.frames > 0)
        .map_or((0, 0), |m| (m.width as i32, m.height as i32));
    // The window: a child of the parent, or an overlapped one, the size
    // of the picture.
    let parent = if flags & MCI_DGV_OPEN_PARENT != 0 {
        p.parent
    } else {
        0
    };
    let mut style = if flags & MCI_DGV_OPEN_WS != 0 {
        p.style
    } else {
        WS_OVERLAPPEDWINDOW
    };
    if parent != 0 && style & window::WS_POPUP == 0 {
        style |= window::WS_CHILD;
    }
    let (x, y, cx, cy) = if style & (window::WS_CHILD | window::WS_POPUP) != 0 {
        (0, 0, w, h)
    } else {
        let r = window::adjust(
            crate::win32::gdi32::Rect::sized(0, 0, w, h),
            style,
            0,
            false,
        );
        let d = CW_USEDEFAULT;
        (d, d, r.width(), r.height())
    };
    if p.windowless {
        let v = video(cpu, i);
        v.source = [0, 0, w, h];
        v.media = media;
        return 0;
    }
    if let Some(m) = &media {
        calls(cpu).media = Some((element.clone(), m.clone()));
    }
    // The class and the caption are the second and third arguments.
    let args = [
        0,
        0,
        0,
        style,
        x as u32,
        y as u32,
        cx as u32,
        cy as u32,
        parent,
        0,
        super::super::DLL.base,
        0,
    ];
    let title = element.rsplit(['\\', '/', ':']).next().unwrap_or_default();
    let (class, caption) = if is_mpeg(cpu, i) {
        (CLASS, CAPTION)
    } else {
        (AVI_CLASS, title)
    };
    let names = [(1, class), (2, caption)];
    let Some(hwnd) = made(cpu, "CreateWindowExA", &args, &names) else {
        return NEEDS_CALL;
    };
    if hwnd == 0 {
        return MCIERR_NO_WINDOW;
    }
    let v = video(cpu, i);
    v.hwnd = hwnd;
    v.own = hwnd;
    v.source = [0, 0, w, h];
    v.media = media;
    0
}

/// The device's length in milliseconds.
fn length(cpu: &mut Cpu, i: usize) -> u32 {
    media(cpu, i).map_or(0, |m| m.length_ms())
}

/// A position in the device's time format, in milliseconds.
fn to_ms(cpu: &mut Cpu, i: usize, value: u32) -> u32 {
    if device(cpu, i).time_format != MCI_FORMAT_FRAMES {
        return value;
    }
    media(cpu, i).map_or(0, |m| m.frame_ms(value))
}

/// A position in milliseconds, in the device's time format.
fn from_ms(cpu: &mut Cpu, i: usize, ms: u32) -> u32 {
    if device(cpu, i).time_format != MCI_FORMAT_FRAMES {
        return ms;
    }
    media(cpu, i).map_or(0, |m| {
        if ms >= m.length_ms() {
            m.frames
        } else {
            m.frame_at(ms as u64)
        }
    })
}

/// Where the device is, in milliseconds: where its play has got to.
fn position(cpu: &mut Cpu, i: usize) -> u32 {
    let now = cpu.bus.clock.now_micros();
    let v = video(cpu, i);
    let Some(started) = v.started else {
        return v.pos;
    };
    let at = v.pos as u64 + now.saturating_sub(started) / 1000;
    at.min(v.to as u64) as u32
}

/// When the play of `d` ends, in the clock's microseconds, if it plays.
pub fn end_at(d: &Device) -> Option<u64> {
    let v = &d.video;
    Some(v.started? + (v.to.saturating_sub(v.pos) as u64) * 1000)
}

/// When the device `d` next shows a frame, or its play ends.
pub fn next_due(d: &Device) -> Option<u64> {
    let v = &d.video;
    let started = v.started?;
    let end = end_at(d)?;
    if v.hwnd == 0 {
        return Some(end);
    }
    // A file not read yet since a restore is read at once, by the next
    // draw; one without pictures (MPEG audio) has only its end to wait
    // for.
    let Some(m) = v.media.as_ref() else {
        return Some(started.min(end));
    };
    if m.frames == 0 {
        return Some(end);
    }
    // The next frame's start, from the frame drawn last.
    let next = v.drawn.map_or(0, |n| n + 1);
    let at = m.frame_ms(next) as u64;
    let due = started + at.saturating_sub(v.pos as u64) * 1000;
    Some(due.min(end))
}

/// The open device `i` at the time `now`: its frame drawn, and true when
/// its play ended.
pub fn poll(cpu: &mut Cpu, i: usize, now: u64) -> bool {
    if video(cpu, i).started.is_none() {
        return false;
    }
    let ended = end_at(device(cpu, i)).is_some_and(|end| end <= now);
    draw(cpu, i, None);
    if !ended {
        return false;
    }
    let repeat = video(cpu, i).repeat;
    halt(cpu, i);
    if !repeat {
        leave_full_screen(cpu, i);
    }
    let v = video(cpu, i);
    if repeat {
        // The frames come again from the first, which `next_due` waits
        // for from the frame drawn last.
        v.pos = 0;
        v.drawn = None;
        start(cpu, i);
        return false;
    }
    true
}

/// The play stops where it is.
fn halt(cpu: &mut Cpu, i: usize) {
    if video(cpu, i).started.is_none() {
        return;
    }
    let at = position(cpu, i);
    let v = video(cpu, i);
    v.pos = at;
    v.started = None;
    let sound = std::mem::take(&mut v.sound);
    if sound != 0 {
        wave::stop_sound(cpu, sound);
    }
}

/// The play goes from `pos` to `to`, from now.
fn start(cpu: &mut Cpu, i: usize) {
    let (pos, to) = (video(cpu, i).pos, video(cpu, i).to);
    if pos >= to {
        return;
    }
    let audio = media(cpu, i).and_then(|m| {
        let (f, data) = m.audio.as_ref()?;
        let block = f.block() as u64;
        let at =
            |ms: u32| ((ms as u64 * f.bytes_per_second() as u64 / 1000) / block * block) as usize;
        let (a, b) = (at(pos).min(data.len()), at(to).min(data.len()));
        (a < b).then(|| (*f, data[a..b].to_vec()))
    });
    let (volume, balance) = (video(cpu, i).volume, video(cpu, i).balance);
    let sound = match audio {
        Some((f, mut samples)) if !video(cpu, i).muted && volume > -10000 => {
            attenuate(&f, &mut samples, volume, balance);
            wave::play_wav(cpu, f, samples, false)
        }
        _ => 0,
    };
    let now = cpu.bus.clock.now_micros();
    let v = video(cpu, i);
    v.sound = sound;
    v.started = Some(now);
}

/// Scale PCM `samples` of the format `f` by a volume and a balance in
/// hundredths of a decibel (`Video::volume`): a channel turned down by
/// 20 dB has a tenth of its amplitude.
pub fn attenuate(
    f: &crate::win32::dsound::device::Format,
    samples: &mut [u8],
    volume: i32,
    balance: i32,
) {
    if volume == 0 && balance == 0 {
        return;
    }
    let gain = |db: i32| 10f32.powf(db.clamp(-10000, 0) as f32 / 2000.0);
    let left = gain(volume - balance.max(0));
    let right = gain(volume + balance.min(0));
    let channels = f.channels.max(1) as usize;
    let scale = |k: usize| if channels == 2 && k % 2 == 1 { right } else { left };
    let mono = (left + right) / 2.0;
    let pick = |k: usize| if channels == 1 { mono } else { scale(k) };
    match f.bits {
        8 => {
            for (k, s) in samples.iter_mut().enumerate() {
                let v = (*s as f32 - 128.0) * pick(k);
                *s = (v.round() + 128.0).clamp(0.0, 255.0) as u8;
            }
        }
        16 => {
            for (k, s) in samples.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                let v = i16::from_le_bytes(*s) as f32 * pick(k);
                *s = (v.round() as i16).to_le_bytes();
            }
        }
        _ => {}
    }
}

/// MCI_PLAY: from where the device is, or `from`, to its end, or `to`.
/// A window of its own that is hidden shows, when it is the one the
/// device paints. With MCI_MCIAVI_PLAY_FULLSCREEN or _FULLBY2 the
/// pictures cover the screen instead (`full_screen`); a play without
/// them, or with MCI_MCIAVI_PLAY_WINDOW, shows in the window.
fn play(cpu: &mut Cpu, i: usize, flags: u32, p: &Parms) -> u32 {
    if flags & MCI_DGV_PLAY_REVERSE != 0 {
        return MCIERR_UNSUPPORTED_FUNCTION;
    }
    let full = flags & (MCI_MCIAVI_PLAY_FULLSCREEN | MCI_MCIAVI_PLAY_FULLBY2) != 0;
    if full && flags & MCI_MCIAVI_PLAY_WINDOW != 0 {
        return MCIERR_FLAGS_NOT_COMPATIBLE;
    }
    let Some(m) = media(cpu, i) else {
        return MCIERR_NONAPPLICABLE_FUNCTION;
    };
    let length = length(cpu, i);
    let from = if flags & MCI_FROM != 0 {
        to_ms(cpu, i, p.from)
    } else {
        position(cpu, i)
    };
    let to = if flags & MCI_TO != 0 {
        to_ms(cpu, i, p.to)
    } else {
        length
    };
    if from > length || to > length {
        return MCIERR_OUTOFRANGE;
    }
    let cover = if full && m.frames > 0 {
        match full_screen(cpu, i) {
            Some(cover) => cover,
            None => return NEEDS_CALL,
        }
    } else {
        leave_full_screen(cpu, i);
        let (own, hwnd) = (video(cpu, i).own, video(cpu, i).hwnd);
        let manual = video(cpu, i).manual_show;
        let state = user32::user(cpu);
        if own != 0 && hwnd == own && !manual && state.is_window(own) && !state.is_visible(own) {
            let shown = window::SW_SHOWNA;
            if made(cpu, "ShowWindow", &[own, shown], &[]).is_none() {
                return NEEDS_CALL;
            }
        }
        0
    };
    halt(cpu, i);
    // A position at the end plays from the start.
    let from = if from >= to && flags & MCI_FROM == 0 {
        0
    } else {
        from
    };
    let v = video(cpu, i);
    v.pos = from;
    v.to = to;
    v.paused = false;
    v.repeat = flags & MCI_DGV_PLAY_REPEAT != 0;
    v.drawn = None;
    v.cover = cover;
    v.by_two = flags & MCI_MCIAVI_PLAY_FULLBY2 != 0;
    start(cpu, i);
    0
}

/// The window a full-screen play covers the screen with: the one it has,
/// or a new one, which is made and shown. None while a call into USER32
/// is to be made first (`made`).
///
/// MCIAVI shows a full-screen play in a display mode of its own, 320 by
/// 240, and gives the screen back when the play stops. The device's
/// window stays as it is: its handle, place and visibility don't change,
/// and it shows the frame where the play stopped. Here the play draws
/// into a window of the device's class that covers the screen, a
/// topmost popup with nothing around the picture but black (`cover`).
/// It isn't activated, so the program's window keeps the keyboard, as
/// it does when MCIAVI's play takes the screen. The window goes when the
/// play stops, whatever stops it (`leave_full_screen`), so a play that
/// is paused, or resumed after a pause, shows in the device's window.
fn full_screen(cpu: &mut Cpu, i: usize) -> Option<u32> {
    let cover = video(cpu, i).cover;
    if cover != 0 && user32::user(cpu).is_window(cover) {
        return Some(cover);
    }
    let (w, h) = user32::sysparams::screen_size(cpu);
    let args = [
        window::WS_EX_TOPMOST,
        0,
        0,
        window::WS_POPUP,
        0,
        0,
        w as u32,
        h as u32,
        0,
        0,
        super::super::DLL.base,
        0,
    ];
    let class = if is_mpeg(cpu, i) { CLASS } else { AVI_CLASS };
    let cover = made(cpu, "CreateWindowExA", &args, &[(1, class), (2, "")])?;
    if cover != 0 {
        made(cpu, "ShowWindow", &[cover, window::SW_SHOWNA], &[])?;
    }
    Some(cover)
}

/// A full-screen play stopped: the window that covered the screen goes,
/// and the screen shows the windows under it again.
fn leave_full_screen(cpu: &mut Cpu, i: usize) {
    let cover = std::mem::take(&mut video(cpu, i).cover);
    if cover == 0 {
        return;
    }
    // The device's window has the frame drawn before the play took the
    // screen: the next draw gives it the one the play got to.
    video(cpu, i).drawn = None;
    if user32::user(cpu).is_window(cover) {
        user32::message::post_message(cpu, &[cover, WM_CLOSE, 0, 0]);
    }
}

/// Where a full-screen play shows the picture `w` by `h` in the window
/// that covers the screen, as left, top, width and height: its place in
/// MCIAVI's 320 by 240 mode, at the middle, at its size or twice it,
/// made as big as the screen is to that mode. A picture bigger than the
/// screen is made smaller to fit, keeping its shape.
fn full_screen_rect(cpu: &mut Cpu, w: i32, h: i32, by_two: bool) -> [i32; 4] {
    let (sw, sh) = user32::sysparams::screen_size(cpu);
    let mode = (sw as f64 / 320.0).min(sh as f64 / 240.0);
    let mut scale = mode * if by_two { 2.0 } else { 1.0 };
    scale = scale.min(sw as f64 / w.max(1) as f64);
    scale = scale.min(sh as f64 / h.max(1) as f64);
    let (pw, ph) = (
        (w as f64 * scale).round() as i32,
        (h as f64 * scale).round() as i32,
    );
    [(sw - pw) / 2, (sh - ph) / 2, pw, ph]
}

/// The sound's volume and balance (`Video::volume`): a play goes on from
/// where it is with them.
pub fn set_gain(cpu: &mut Cpu, i: usize, volume: i32, balance: i32) {
    let playing = video(cpu, i).started.is_some();
    halt(cpu, i);
    let v = video(cpu, i);
    v.volume = volume;
    v.balance = balance;
    if playing {
        start(cpu, i);
    }
}

/// Where the device `i` is, in milliseconds.
pub fn position_ms(cpu: &mut Cpu, i: usize) -> u32 {
    position(cpu, i)
}

/// MCI_SEEK: to the start, the end or a position; it stops the play,
/// and the frame there is drawn.
fn seek(cpu: &mut Cpu, i: usize, flags: u32, p: &Parms) -> u32 {
    let ways = [MCI_SEEK_TO_START, MCI_SEEK_TO_END, MCI_TO]
        .iter()
        .filter(|&&f| flags & f != 0)
        .count();
    if ways > 1 {
        return MCIERR_FLAGS_NOT_COMPATIBLE;
    }
    let length = length(cpu, i);
    let to = if flags & MCI_SEEK_TO_START != 0 {
        0
    } else if flags & MCI_SEEK_TO_END != 0 {
        length
    } else if flags & MCI_TO != 0 {
        let to = to_ms(cpu, i, p.to);
        if to > length {
            return MCIERR_OUTOFRANGE;
        }
        to
    } else {
        return MCIERR_MISSING_PARAMETER;
    };
    halt(cpu, i);
    leave_full_screen(cpu, i);
    let v = video(cpu, i);
    v.pos = to;
    v.paused = false;
    draw(cpu, i, None);
    0
}

/// MCI_STEP: frames on, or back.
fn step(cpu: &mut Cpu, i: usize, flags: u32, p: &Parms) -> u32 {
    let Some(m) = media(cpu, i) else {
        return MCIERR_NONAPPLICABLE_FUNCTION;
    };
    let frames = if flags & MCI_DGV_STEP_FRAMES != 0 {
        p.number
    } else {
        1
    };
    halt(cpu, i);
    leave_full_screen(cpu, i);
    let at = m.frame_at(video(cpu, i).pos as u64);
    let n = if flags & MCI_DGV_STEP_REVERSE != 0 {
        at.saturating_sub(frames)
    } else {
        at.saturating_add(frames).min(m.frames.saturating_sub(1))
    };
    video(cpu, i).pos = m.frame_ms(n);
    draw(cpu, i, None);
    0
}

fn status(cpu: &mut Cpu, i: usize, flags: u32, p: &mut Parms) -> u32 {
    if flags & MCI_STATUS_ITEM == 0 {
        return MCIERR_MISSING_PARAMETER;
    }
    let (ret, kind) = match p.item {
        MCI_STATUS_LENGTH => {
            let length = length(cpu, i);
            (from_ms(cpu, i, length), Ret::Int)
        }
        MCI_STATUS_POSITION => {
            let at = if flags & MCI_STATUS_START != 0 {
                0
            } else {
                position(cpu, i)
            };
            (from_ms(cpu, i, at), Ret::Int)
        }
        MCI_STATUS_NUMBER_OF_TRACKS | MCI_STATUS_CURRENT_TRACK => (1, Ret::Int),
        MCI_STATUS_MODE => {
            let v = video(cpu, i);
            let mode = if v.started.is_some() {
                MCI_MODE_PLAY
            } else if v.paused {
                MCI_MODE_PAUSE
            } else {
                MCI_MODE_STOP
            };
            (mode, Ret::Constant)
        }
        MCI_STATUS_MEDIA_PRESENT | MCI_STATUS_READY => (TRUE, Ret::Bool),
        MCI_STATUS_TIME_FORMAT => (device(cpu, i).time_format, Ret::TimeFormat),
        MCI_DGV_STATUS_HWND => (video(cpu, i).hwnd, Ret::Int),
        MCI_DGV_STATUS_WINDOW_VISIBLE
        | MCI_DGV_STATUS_WINDOW_MINIMIZED
        | MCI_DGV_STATUS_WINDOW_MAXIMIZED => {
            let hwnd = video(cpu, i).hwnd;
            let state = user32::user(cpu);
            let Some(style) = state.window(hwnd).map(|w| w.style) else {
                return MCIERR_NO_WINDOW;
            };
            let on = match p.item {
                MCI_DGV_STATUS_WINDOW_VISIBLE => state.is_visible(hwnd),
                MCI_DGV_STATUS_WINDOW_MINIMIZED => style & window::WS_MINIMIZE != 0,
                _ => style & window::WS_MAXIMIZE != 0,
            };
            (on as u32, Ret::Bool)
        }
        _ => return MCIERR_UNSUPPORTED_FUNCTION,
    };
    p.ret = ret;
    p.ret_kind = kind;
    0
}

/// MCI_SET: the time format, and the sound on or off.
fn set(cpu: &mut Cpu, i: usize, flags: u32, p: &Parms) -> u32 {
    if flags & MCI_SET_TIME_FORMAT != 0 {
        if !matches!(p.time_format, MCI_FORMAT_MILLISECONDS | MCI_FORMAT_FRAMES) {
            return MCIERR_BAD_TIME_FORMAT;
        }
        device(cpu, i).time_format = p.time_format;
    }
    if flags & MCI_SET_AUDIO != 0 && flags & (MCI_SET_ON | MCI_SET_OFF) != 0 {
        let muted = flags & MCI_SET_OFF != 0;
        if video(cpu, i).muted != muted {
            let playing = video(cpu, i).started.is_some();
            halt(cpu, i);
            video(cpu, i).muted = muted;
            if playing {
                start(cpu, i);
            }
        }
    }
    0
}

/// MCI_GETDEVCAPS.
fn capabilities(flags: u32, p: &mut Parms) -> u32 {
    if flags & MCI_GETDEVCAPS_ITEM == 0 {
        return MCIERR_MISSING_PARAMETER;
    }
    let (ret, kind) = match p.item {
        MCI_GETDEVCAPS_DEVICE_TYPE => (MCI_DEVTYPE_DIGITAL_VIDEO, Ret::Constant),
        MCI_GETDEVCAPS_HAS_AUDIO
        | MCI_GETDEVCAPS_HAS_VIDEO
        | MCI_GETDEVCAPS_USES_FILES
        | MCI_GETDEVCAPS_COMPOUND_DEVICE
        | MCI_GETDEVCAPS_CAN_PLAY => (TRUE, Ret::Bool),
        MCI_GETDEVCAPS_CAN_RECORD | MCI_GETDEVCAPS_CAN_EJECT | MCI_GETDEVCAPS_CAN_SAVE => {
            (FALSE, Ret::Bool)
        }
        _ => return MCIERR_UNSUPPORTED_FUNCTION,
    };
    p.ret = ret;
    p.ret_kind = kind;
    0
}

/// MCI_WINDOW: the window it paints, how that shows, its caption.
fn set_window(cpu: &mut Cpu, i: usize, flags: u32, p: &Parms) -> u32 {
    if flags & MCI_DGV_WINDOW_HWND != 0 {
        let hwnd = if p.hwnd == MCI_DGV_WINDOW_DEFAULT {
            video(cpu, i).own
        } else if user32::user(cpu).is_window(p.hwnd) {
            p.hwnd
        } else {
            return MCIERR_NO_WINDOW;
        };
        let v = video(cpu, i);
        v.hwnd = hwnd;
        v.drawn = None;
    }
    let hwnd = video(cpu, i).hwnd;
    if flags & (MCI_DGV_WINDOW_STATE | MCI_DGV_WINDOW_TEXT) != 0 && hwnd == 0 {
        return MCIERR_NO_WINDOW;
    }
    if flags & MCI_DGV_WINDOW_STATE != 0
        && made(cpu, "ShowWindow", &[hwnd, p.cmd_show], &[]).is_none()
    {
        return NEEDS_CALL;
    }
    if flags & MCI_DGV_WINDOW_TEXT != 0 {
        let text = p.window_text.clone();
        if made(cpu, "SetWindowTextA", &[hwnd, 0], &[(1, &text)]).is_none() {
            return NEEDS_CALL;
        }
    }
    0
}

/// The client area of the window the device paints, as left, top, width
/// and height.
fn client(cpu: &mut Cpu, i: usize) -> [i32; 4] {
    let hwnd = video(cpu, i).hwnd;
    user32::user(cpu)
        .window(hwnd)
        .map_or([0; 4], |w| [0, 0, w.client.width(), w.client.height()])
}

/// MCI_PUT: the source or destination rectangle, or where the window is.
fn put(cpu: &mut Cpu, i: usize, flags: u32, p: &Parms) -> u32 {
    if flags & (MCI_DGV_PUT_FRAME | MCI_DGV_PUT_VIDEO) != 0 {
        return MCIERR_UNSUPPORTED_FUNCTION;
    }
    let rect = (flags & MCI_DGV_RECT != 0).then_some(p.rect);
    if flags & MCI_DGV_PUT_WINDOW != 0 {
        let hwnd = video(cpu, i).hwnd;
        let Some([x, y, w, h]) = rect else {
            return MCIERR_MISSING_PARAMETER;
        };
        let args = [hwnd, x as u32, y as u32, w as u32, h as u32, TRUE];
        if made(cpu, "MoveWindow", &args, &[]).is_none() {
            return NEEDS_CALL;
        }
    }
    if flags & MCI_DGV_PUT_SOURCE != 0 {
        let whole = media(cpu, i).map_or([0; 4], |m| [0, 0, m.width as i32, m.height as i32]);
        video(cpu, i).source = rect.unwrap_or(whole);
    }
    if flags & MCI_DGV_PUT_DESTINATION != 0 {
        video(cpu, i).dest = rect;
    }
    video(cpu, i).drawn = None;
    0
}

/// MCI_WHERE: the source or destination rectangle, or where the window
/// is.
fn place(cpu: &mut Cpu, i: usize, flags: u32, p: &mut Parms) -> u32 {
    p.rect = if flags & MCI_DGV_PUT_SOURCE != 0 {
        if flags & MCI_DGV_WHERE_MAX != 0 {
            media(cpu, i).map_or([0; 4], |m| [0, 0, m.width as i32, m.height as i32])
        } else {
            video(cpu, i).source
        }
    } else if flags & MCI_DGV_PUT_DESTINATION != 0 {
        match video(cpu, i).dest {
            Some(r) if flags & MCI_DGV_WHERE_MAX == 0 => r,
            _ => client(cpu, i),
        }
    } else if flags & MCI_DGV_PUT_WINDOW != 0 {
        let hwnd = video(cpu, i).hwnd;
        let state = user32::user(cpu);
        let Some(w) = state.window(hwnd) else {
            return MCIERR_NO_WINDOW;
        };
        let r = w.rect;
        [r.left, r.top, r.width(), r.height()]
    } else if flags & (MCI_DGV_PUT_FRAME | MCI_DGV_PUT_VIDEO) != 0 {
        return MCIERR_UNSUPPORTED_FUNCTION;
    } else {
        return MCIERR_MISSING_PARAMETER;
    };
    p.ret_kind = Ret::Rect;
    0
}

/// MCI_UPDATE: the frame drawn again, to a DC.
fn update(cpu: &mut Cpu, i: usize, flags: u32, p: &Parms) -> u32 {
    if flags & MCI_DGV_UPDATE_HDC == 0 {
        return MCIERR_MISSING_PARAMETER;
    }
    video(cpu, i).drawn = None;
    let rect = (flags & MCI_DGV_RECT != 0).then_some(p.rect);
    draw(cpu, i, Some((p.hdc, rect)));
    0
}

/// Draws the frame the device is at: to the window it paints, or the
/// one a full-screen play covers the screen with, or to the DC `to` (in
/// the rectangle, or the destination one), unless it was drawn last.
fn draw(cpu: &mut Cpu, i: usize, to: Option<(u32, Option<[i32; 4]>)>) {
    let Some(m) = media(cpu, i) else {
        return;
    };
    if m.frames == 0 {
        return;
    }
    let n = m.frame_at(position(cpu, i) as u64);
    let cover = video(cpu, i).cover;
    let hwnd = if cover != 0 {
        cover
    } else {
        video(cpu, i).hwnd
    };
    // A window that covers the screen is black around the picture: all
    // of it is drawn black when nothing was drawn in it yet, or when it
    // is painted (`window_proc`).
    let blacken = cover != 0 && video(cpu, i).drawn.is_none();
    if to.is_none() {
        if video(cpu, i).drawn == Some(n) {
            return;
        }
        // Done with this frame even when it can't be drawn (its window
        // is gone, or it doesn't decode): `next_due` waits for the next
        // one rather than for this one again.
        video(cpu, i).drawn = Some(n);
        if !user32::user(cpu).is_window(hwnd) {
            return;
        }
    }
    let (w, h) = (m.width as i32, m.height as i32);
    let stride = dib_stride(w, 24);
    let rows = {
        let v = video(cpu, i);
        let Some(rows) = v.pictures.rows(&m, n, stride) else {
            return;
        };
        rows
    };
    video(cpu, i).drawn = Some(n);
    let mut surface = Surface::memory(w, h, 24, Vec::new(), default_masks(24));
    surface.data = rows;
    let [sx, sy, sw, sh] = video(cpu, i).source;
    let source = Source {
        surface,
        bk_color: 0xFF_FFFF,
        origin: (0, 0),
    };
    let (hdc, dest) = match to {
        Some((hdc, rect)) => (hdc, rect.or(video(cpu, i).dest).unwrap_or([0, 0, w, h])),
        None => {
            let dest = if cover != 0 {
                let by_two = video(cpu, i).by_two;
                full_screen_rect(cpu, w, h, by_two)
            } else {
                video(cpu, i).dest.unwrap_or_else(|| client(cpu, i))
            };
            let flags = crate::win32::gdi32::dc::DCX_USESTYLE | crate::win32::gdi32::dc::DCX_CACHE;
            (user32::paint::window_dc(cpu, hwnd, flags, None), dest)
        }
    };
    let Some(dc) = crate::win32::gdi32::gdi(cpu).dc(hdc).cloned() else {
        return;
    };
    let [dx, dy, dw, dh] = dest;
    let (x0, y0) = dc.to_device(dx, dy);
    let (x1, y1) = dc.to_device(dx + dw, dy + dh);
    let src = (sx, sy, sx + sw, sy + sh);
    if blacken && to.is_none() {
        let (cw, ch) = user32::user(cpu)
            .window(hwnd)
            .map_or((0, 0), |w| (w.client.width(), w.client.height()));
        draw::blit_from(cpu, hdc, (0, 0, cw, ch), None, BLACKNESS);
    }
    let old = select_movie_palette(cpu, i, &m, hdc);
    draw::blit_from(cpu, hdc, (x0, y0, x1, y1), Some((source, src)), SRCCOPY);
    if let Some(old) = old {
        palette::select_palette(cpu, &[hdc, old, 1]);
    }
    if to.is_none() {
        user32::paint::release_dc(cpu, &[hwnd, hdc]);
    }
}

/// MCI_REALIZE: the movie's palette is realized in the device's window
/// now, as a program does when its window gets WM_QUERYNEWPALETTE or
/// WM_PALETTECHANGED. With MCI_DGV_REALIZE_BKGD it is a background
/// palette, for this and the frames drawn after it, until an
/// MCI_REALIZE without it.
fn realize(cpu: &mut Cpu, i: usize, flags: u32) -> u32 {
    if flags & MCI_TEST != 0 {
        return 0;
    }
    video(cpu, i).background = flags & MCI_DGV_REALIZE_BKGD != 0;
    let hwnd = video(cpu, i).hwnd;
    if !palette::palette_device(cpu) || !user32::user(cpu).is_window(hwnd) {
        return 0;
    }
    let Some(m) = media(cpu, i) else {
        return 0;
    };
    let flags = crate::win32::gdi32::dc::DCX_USESTYLE | crate::win32::gdi32::dc::DCX_CACHE;
    let hdc = user32::paint::window_dc(cpu, hwnd, flags, None);
    if let Some(old) = select_movie_palette(cpu, i, &m, hdc) {
        palette::select_palette(cpu, &[hdc, old, 1]);
    }
    user32::paint::release_dc(cpu, &[hwnd, hdc]);
    0
}

/// On a palette device, select the movie's palette into `hdc` and
/// realize it, as MCIAVI's DrawDib does before it draws: the movie's own
/// colours for an 8-bit movie, the halftone palette for one of more
/// bits. Unless MCI_REALIZE asked for the background, it is selected as
/// a foreground palette, so in the foreground window or a child of it,
/// its colours take the system palette's free entries. Returns the
/// palette it replaced, which goes back in after the picture is drawn.
fn select_movie_palette(cpu: &mut Cpu, i: usize, m: &Media, hdc: u32) -> Option<u32> {
    if !palette::palette_device(cpu) {
        return None;
    }
    let mut hpal = video(cpu, i).palette;
    if crate::win32::gdi32::gdi(cpu).palette(hpal).is_none() {
        let mut colors = match &m.video {
            MediaKind::Avi(v) => v.colors(),
            _ => Vec::new(),
        };
        if colors.is_empty() {
            colors = palette::halftone_colors();
        }
        hpal = palette::system_palette_of(cpu, &colors);
        video(cpu, i).palette = hpal;
    }
    let background = video(cpu, i).background as u32;
    let old = palette::select_palette(cpu, &[hdc, hpal, background]).filter(|&o| o != 0)?;
    palette::realize_posted(cpu, hdc);
    Some(old)
}

/// The window procedure of the devices' windows (`CLASS`, `AVI_CLASS`),
/// and of those that full-screen plays cover the screen with: WM_PAINT
/// draws the frame the device is at again, as MCIAVI's and MCIQTZ's
/// windows do when they are uncovered or the play is stopped, and the
/// rest goes to DefWindowProcA.
pub fn window_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, msg) = (args[0], args[1]);
    if msg == user32::message::WM_PAINT {
        let painted = mci(cpu).devices.iter().position(|d| {
            let v = &d.video;
            let shows = if v.cover != 0 { v.cover } else { v.hwnd };
            matches!(d.kind, MCI_DEVTYPE_DIGITAL_VIDEO | AVI_VIDEO) && shows == hwnd
        });
        if let Some(i) = painted {
            video(cpu, i).drawn = None;
            draw(cpu, i, None);
        }
    }
    user32::defwnd::def_window_proc(cpu, args)
}

/// CreateWindowExA's default place.
const CW_USEDEFAULT: i32 = 0x8000_0000_u32 as i32;
/// An overlapped window with a caption, a frame and its boxes.
const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;

crate::state_fields!(Video {
    hwnd,
    own,
    source,
    dest,
    pos,
    to,
    started,
    repeat,
    sound,
    paused,
    muted,
    volume,
    balance,
    manual_show,
    drawn,
    palette,
    background,
    cover,
    by_two
} skip {
    media,
    pictures
});

crate::state_fields!(Call {
    target,
    args,
    strings
});

crate::state_fields!(Calls {
    tid,
    made,
    next
} skip {
    media
});
