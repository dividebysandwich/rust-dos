//! QUARTZ.DLL's filter graph manager (CLSID_FilterGraph): IGraphBuilder
//! renders a file, and IMediaControl, IMediaEventEx, IMediaPosition,
//! IMediaSeeking, IVideoWindow, IBasicVideo and IBasicAudio play it, as
//! the graph a program builds with RenderFile plays on Windows: the
//! pictures in the video renderer's window, "ActiveMovie Window", and
//! the sound on the sound device.
//!
//! A rendered graph plays through an MPEGVideo device of its own
//! (winmm/mci/video.rs), the player MCI's MCIQTZ has on Windows too:
//! its window is the video renderer's, made hidden when the file is
//! rendered and shown when the graph runs, unless AutoShow is off.
//! IVideoWindow's owner makes it a child of the owner's window, as the
//! renderer's put_Owner does, and its position and style are the
//! window's own. The play's end (`play_ended`) queues EC_COMPLETE,
//! which sets the graph's event and posts its notification message.
//!
//! The graph has no filters a program can see: the methods that add,
//! connect or list filters return E_NOTIMPL, and so do IDispatch's,
//! but for GetTypeInfoCount, which counts no type information.
//! Positions are in milliseconds within, in 100-nanosecond units
//! (TIME_FORMAT_MEDIA_TIME) through IMediaSeeking and in seconds
//! through IMediaPosition. The play's rate is always 1.

use super::{
    E_ABORT, E_FAIL, E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, E_POINTER, FILE_NOT_FOUND, Kind,
    S_FALSE, S_OK, TIME_FORMAT_MEDIA_TIME, VFW_E_NO_PALETTE_AVAILABLE, VFW_E_UNKNOWN_FILE_TYPE,
    VFW_E_WRONG_STATE, get64, put32, put64,
};
use crate::cpu::Cpu;
use crate::win32::com::Guid;
use crate::win32::thread::{self, Wait};
use crate::win32::user32;
use crate::win32::winmm::mci::{
    self, MCI_FROM, MCI_OPEN_ALIAS, MCI_OPEN_ELEMENT, MCI_OPEN_TYPE, MCI_PAUSE, MCI_PLAY, MCI_SEEK,
    MCI_STOP, MCI_TO, MCIERR_FILE_NOT_FOUND, MCIERR_INVALID_FILE, Parms, media::Media, video,
};
use std::sync::Arc;

/// IMediaControl's states.
const STATE_STOPPED: u32 = 0;
const STATE_PAUSED: u32 = 1;
const STATE_RUNNING: u32 = 2;

/// The events.
pub const EC_COMPLETE: u32 = 0x01;

/// IMediaEventEx's flag that stops the notifications.
const AM_MEDIAEVENT_NONOTIFY: u32 = 0x01;

/// IMediaSeeking's capabilities: seeking to a time, forward and back;
/// the current and stop positions and the duration.
const SEEKING_CAPS: u32 = 0x3F;

/// SetPositions' flags.
const AM_SEEKING_POSITIONING_MASK: u32 = 0x03;
const AM_SEEKING_ABSOLUTE: u32 = 0x01;
const AM_SEEKING_RELATIVE: u32 = 0x02;
const AM_SEEKING_INCREMENTAL: u32 = 0x03;
const AM_SEEKING_RETURN_TIME: u32 = 0x08;

/// OLE's TRUE and FALSE.
const OATRUE: u32 = 0xFFFF_FFFF;
const OAFALSE: u32 = 0;

const INFINITE: u32 = 0xFFFF_FFFF;
const VFW_E_NOT_STOPPED: u32 = 0x8004_0224;
const VFW_E_NOT_FOUND: u32 = 0x8004_0216;

/// 100-nanosecond units a millisecond.
const UNITS_MS: i64 = 10_000;

/// GWL_STYLE and GWL_EXSTYLE, and the styles put_Owner changes.
const GWL_STYLE: u32 = 0xFFFF_FFF0;
const GWL_EXSTYLE: u32 = 0xFFFF_FFEC;
const WS_CHILD: u32 = 0x4000_0000;
const WS_POPUP: u32 = 0x8000_0000;
const WS_MINIMIZE: u32 = 0x2000_0000;
const WS_MAXIMIZE: u32 = 0x0100_0000;

/// ShowWindow's commands.
const SW_HIDE: u32 = 0;
const SW_SHOWNORMAL: u32 = 1;
const SW_SHOWMINIMIZED: u32 = 2;
const SW_SHOWMAXIMIZED: u32 = 3;
const SW_SHOW: u32 = 5;

/// A filter graph's state, in its `Kind::Graph`.
#[derive(Clone, Debug, Default)]
pub struct Graph {
    /// The file it renders, or none.
    pub file: String,
    /// IMediaControl's state.
    pub state: u32,
    /// The events queued: their codes and two parameters.
    pub events: Vec<(u32, u32, u32)>,
    /// Its event, set while events are queued: the handle the program
    /// gets and the object.
    pub event: (u32, u32),
    /// SetNotifyWindow's window, message and data, and SetNotifyFlags'.
    pub notify: (u32, u32, u32),
    pub notify_flags: u32,
    /// IVideoWindow's settings that aren't the window's own: whether
    /// AutoShow is off, the message drain, the border colour, whether
    /// the background palette and the hidden cursor are asked for.
    pub manual_show: bool,
    pub drain: u32,
    pub border_color: u32,
    pub background_palette: bool,
    pub cursor_hidden: bool,
    /// Whether a wait (GetEvent, WaitForCompletion) took the play's
    /// EC_COMPLETE before the play's end could queue it.
    pub complete_taken: bool,
}

/// CoCreateInstance's CLSID_FilterGraph: a graph with nothing in it.
pub fn create(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    let id = new(cpu);
    super::give_new(cpu, id, iid, out)
}

/// A new graph with nothing in it, with one reference: its id. amstream
/// makes its multimedia stream's graph with it.
pub fn new(cpu: &mut Cpu) -> u32 {
    let id = super::add(
        cpu,
        Kind::Graph {
            g: Graph::default(),
        },
    );
    let event = super::new_event(cpu);
    if let Some(g) = graph(cpu, id) {
        g.event = event;
    }
    id
}

/// The graph `id` goes: its device closes, its event too.
pub fn close(cpu: &mut Cpu, id: u32) {
    if let Some(i) = mci::graph_device(cpu, id) {
        mci::close_device(cpu, i);
    }
    let event = graph(cpu, id).map_or((0, 0), |g| std::mem::take(&mut g.event));
    super::free_event(cpu, event);
}

fn graph(cpu: &mut Cpu, id: u32) -> Option<&mut Graph> {
    match super::kind(cpu, id)? {
        Kind::Graph { g } => Some(g),
        _ => None,
    }
}

/// The graph `this` is an interface of.
fn graph_of(cpu: &mut Cpu, this: u32) -> Result<u32, u32> {
    let id = super::object_of(cpu, this).ok_or(E_POINTER)?;
    graph(cpu, id).ok_or(E_POINTER)?;
    Ok(id)
}

/// The graph `this` is, and the device it plays through: E_NOINTERFACE
/// with no file rendered, as a graph without a renderer answers.
fn player(cpu: &mut Cpu, this: u32) -> Result<(u32, usize), u32> {
    let id = graph_of(cpu, this)?;
    let i = mci::graph_device(cpu, id).ok_or(E_NOINTERFACE)?;
    Ok((id, i))
}

/// The file the device `i` plays.
fn media(cpu: &mut Cpu, i: usize) -> Result<Arc<Media>, u32> {
    video::media(cpu, i).ok_or(E_FAIL)
}

/// The graph `this`, and the device that shows its pictures: for
/// IVideoWindow and IBasicVideo, which a file without pictures doesn't
/// have.
fn renderer(cpu: &mut Cpu, this: u32) -> Result<(u32, usize, Arc<Media>), u32> {
    let (id, i) = player(cpu, this)?;
    let m = media(cpu, i)?;
    if m.frames == 0 {
        return Err(E_NOINTERFACE);
    }
    Ok((id, i, m))
}

/// The video renderer's window: the device's own.
fn own(cpu: &mut Cpu, i: usize) -> u32 {
    mci::device(cpu, i).video.own
}

/// Run a method that may call into USER32 (video.rs `made`): the call
/// is made and the method runs again.
fn with_calls(cpu: &mut Cpu, f: impl FnOnce(&mut Cpu) -> Result<u32, u32>) -> Option<u32> {
    mci::with_calls(cpu, |cpu| Some(f(cpu).unwrap_or_else(|e| e)))
}

/// A USER32 call the method makes before it goes on, or `NEEDS_CALL`.
fn call(cpu: &mut Cpu, api: &str, args: &[u32], strings: &[(u32, &str)]) -> Result<u32, u32> {
    video::made(cpu, api, args, strings).ok_or(video::NEEDS_CALL)
}

/// Where the play is, in milliseconds, and where it stops.
fn positions(cpu: &mut Cpu, i: usize) -> (u32, u32) {
    (video::position_ms(cpu, i), mci::device(cpu, i).video.to)
}

/// An MCI command of the device `i`, as a method's result. It goes
/// through MCI's dispatch, as a program's command does, so that a play
/// wakes the multimedia thread, which draws its pictures and sees its
/// end (`play_ended`).
fn command(cpu: &mut Cpu, i: usize, msg: u32, flags: u32, from: u32, to: u32) -> Result<(), u32> {
    let mut p = Parms {
        from,
        to,
        ..Parms::default()
    };
    match mci::device_command(cpu, i, msg, flags, &mut p) {
        0 => Ok(()),
        video::NEEDS_CALL => Err(video::NEEDS_CALL),
        _ => Err(E_FAIL),
    }
}

// IGraphBuilder.

/// IGraphBuilder::RenderFile(this, file, play list).
pub fn render_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (this, file) = (args[0], args[1]);
    with_calls(cpu, |cpu| render(cpu, this, file))
}

/// IMediaControl::RenderFile(this, file): the same, with a BSTR.
pub fn control_render_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    render_file(cpu, &[args[0], args[1], 0])
}

/// Render the file named by the wide string at `file`: open a device of
/// the graph's for it, its window hidden. A file rendered before is let
/// go.
fn render(cpu: &mut Cpu, this: u32, file: u32) -> Result<u32, u32> {
    let id = graph_of(cpu, this)?;
    if file == 0 {
        return Err(E_POINTER);
    }
    if graph(cpu, id).is_some_and(|g| g.state != STATE_STOPPED) {
        return Err(VFW_E_NOT_STOPPED);
    }
    let path = crate::win32::dmusic::read_wide(cpu, file, 260);
    if let Some(i) = mci::graph_device(cpu, id) {
        mci::close_device(cpu, i);
    }
    let mut p = Parms {
        open_type: "MPEGVideo".to_string(),
        element: path.clone(),
        alias: format!("DirectShow graph {id}"),
        ..Parms::default()
    };
    let flags = MCI_OPEN_TYPE | MCI_OPEN_ELEMENT | MCI_OPEN_ALIAS;
    match mci::open(cpu, flags, &mut p, false) {
        0 => {}
        video::NEEDS_CALL => return Err(video::NEEDS_CALL),
        MCIERR_FILE_NOT_FOUND => return Err(FILE_NOT_FOUND),
        MCIERR_INVALID_FILE => return Err(VFW_E_UNKNOWN_FILE_TYPE),
        _ => return Err(E_FAIL),
    }
    let i = mci::give_to_graph(cpu, p.ret, id).ok_or(E_FAIL)?;
    let length = video::media(cpu, i).map_or(0, |m| m.length_ms());
    let manual = graph(cpu, id).is_some_and(|g| g.manual_show);
    let v = &mut mci::device(cpu, i).video;
    v.to = length;
    v.manual_show = manual;
    if let Some(g) = graph(cpu, id) {
        g.file = path;
        g.complete_taken = false;
    }
    Ok(S_OK)
}

/// IGraphBuilder::FindFilterByName(this, name, out): there are none to
/// find.
pub fn find_filter_by_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    crate::win32::com::write_out(cpu, args[2], 0);
    Some(VFW_E_NOT_FOUND)
}

/// IDispatch::GetTypeInfoCount(this, out): no type information, which
/// is a count of 0 and S_OK.
pub fn get_type_info_count(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(put32(cpu, args[1], 0).map_or_else(|e| e, |_| S_OK))
}

// IMediaControl.

/// IMediaControl::Run(this): the play goes on from where it is, to the
/// stop position. One there already completes at once.
pub fn run(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let this = args[0];
    with_calls(cpu, |cpu| {
        let id = graph_of(cpu, this)?;
        let Some(i) = mci::graph_device(cpu, id) else {
            set_state(cpu, id, STATE_RUNNING);
            return Ok(S_OK);
        };
        if graph(cpu, id).is_some_and(|g| g.state == STATE_RUNNING) {
            return Ok(S_OK);
        }
        let (pos, to) = positions(cpu, i);
        if pos >= to {
            set_state(cpu, id, STATE_RUNNING);
            queue(cpu, id, EC_COMPLETE, S_OK, 0);
            return Ok(S_OK);
        }
        command(cpu, i, MCI_PLAY, MCI_FROM | MCI_TO, pos, to)?;
        set_state(cpu, id, STATE_RUNNING);
        Ok(S_OK)
    })
}

/// IMediaControl::Pause(this): the play stops where it is, its picture
/// shown.
pub fn pause(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let this = args[0];
    with_calls(cpu, |cpu| {
        let id = graph_of(cpu, this)?;
        if let Some(i) = mci::graph_device(cpu, id) {
            if mci::device(cpu, i).video.started.is_some() {
                command(cpu, i, MCI_PAUSE, 0, 0, 0)?;
            } else {
                show(cpu, id, i)?;
                let (pos, _) = positions(cpu, i);
                command(cpu, i, MCI_SEEK, MCI_TO, 0, pos)?;
            }
        }
        set_state(cpu, id, STATE_PAUSED);
        Ok(S_OK)
    })
}

/// The video renderer's window shows, as it does when its graph pauses
/// or runs, unless AutoShow is off.
fn show(cpu: &mut Cpu, id: u32, i: usize) -> Result<(), u32> {
    let own = own(cpu, i);
    let manual = graph(cpu, id).is_some_and(|g| g.manual_show);
    let state = user32::user(cpu);
    if !manual && state.is_window(own) && !state.is_visible(own) {
        call(cpu, "ShowWindow", &[own, user32::window::SW_SHOWNA], &[])?;
    }
    Ok(())
}

/// IMediaControl::Stop(this): the play stops where it is.
pub fn stop(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let this = args[0];
    with_calls(cpu, |cpu| {
        let id = graph_of(cpu, this)?;
        if let Some(i) = mci::graph_device(cpu, id) {
            command(cpu, i, MCI_STOP, 0, 0, 0)?;
        }
        set_state(cpu, id, STATE_STOPPED);
        Ok(S_OK)
    })
}

/// IMediaControl::StopWhenReady(this): paused, then stopped.
pub fn stop_when_ready(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    stop(cpu, args)
}

fn set_state(cpu: &mut Cpu, id: u32, state: u32) {
    if let Some(g) = graph(cpu, id) {
        g.state = state;
        if state != STATE_RUNNING {
            g.complete_taken = false;
        }
    }
}

/// IMediaControl::GetState(this, timeout, out).
pub fn get_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(graph_of(cpu, args[0]));
    let state = graph(cpu, id).map_or(STATE_STOPPED, |g| g.state);
    Some(put32(cpu, args[2], state).map_or_else(|e| e, |_| S_OK))
}

// IMediaEventEx.

/// Queue the event `code`: the graph's event is set and its window
/// told, unless the notifications are off.
fn queue(cpu: &mut Cpu, id: u32, code: u32, p1: u32, p2: u32) {
    let Some(g) = graph(cpu, id) else {
        return;
    };
    g.events.push((code, p1, p2));
    if g.notify_flags & AM_MEDIAEVENT_NONOTIFY != 0 {
        return;
    }
    let (event, (hwnd, msg, data)) = (g.event.1, g.notify);
    super::set_event(cpu, event);
    if hwnd != 0 {
        user32::message::post_message(cpu, &[hwnd, msg, 0, data]);
    }
}

/// The play of the graph `id` reached its end (winmm/mci): EC_COMPLETE,
/// unless a wait has taken it already.
pub fn play_ended(cpu: &mut Cpu, id: u32) {
    let Some(g) = graph(cpu, id) else {
        return;
    };
    if std::mem::take(&mut g.complete_taken) {
        return;
    }
    queue(cpu, id, EC_COMPLETE, S_OK, 0);
}

/// When the play of the graph `id` ends, in the clock's microseconds, if
/// it plays and nothing has taken its EC_COMPLETE.
fn ends_at(cpu: &mut Cpu, id: u32) -> Option<u64> {
    if graph(cpu, id)?.complete_taken {
        return None;
    }
    let i = mci::graph_device(cpu, id)?;
    video::end_at(mci::device(cpu, i))
}

/// IMediaEvent::GetEventHandle(this, out).
pub fn get_event_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(graph_of(cpu, args[0]));
    let handle = graph(cpu, id).map_or(0, |g| g.event.0);
    Some(put32(cpu, args[1], handle).map_or_else(|e| e, |_| S_OK))
}

/// IMediaEvent::GetEvent(this, code, param1, param2, timeout): the
/// first event queued, or the play's EC_COMPLETE when it comes before
/// the timeout; E_ABORT after the timeout without one.
pub fn get_event(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, code, p1, p2, timeout] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let id = hr!(graph_of(cpu, this));
    for at in [code, p1, p2] {
        if at == 0 || !cpu.bus.guest_probe(at, 4, true) {
            return Some(E_POINTER);
        }
    }
    let Some(g) = graph(cpu, id) else {
        return Some(E_POINTER);
    };
    if !g.events.is_empty() {
        let (c, a, b) = g.events.remove(0);
        let (empty, event) = (g.events.is_empty(), g.event.1);
        if empty {
            super::reset_event(cpu, event);
        }
        write3(cpu, [code, p1, p2], [c, a, b]);
        return Some(S_OK);
    }
    write3(cpu, [code, p1, p2], [0, 0, 0]);
    wait_for(cpu, id, timeout, |cpu| {
        write3(cpu, [code, p1, p2], [EC_COMPLETE, S_OK, 0]);
    })
}

fn write3(cpu: &mut Cpu, at: [u32; 3], values: [u32; 3]) {
    for (a, v) in at.into_iter().zip(values) {
        cpu.bus.guest_write_32(a, v);
    }
}

/// Wait up to `timeout` milliseconds for the play's end: when it comes
/// first, `took` writes EC_COMPLETE, the play's end won't queue it, and
/// the thread waits until then for S_OK; else it waits for the timeout,
/// for E_ABORT.
fn wait_for(cpu: &mut Cpu, id: u32, timeout: u32, took: impl FnOnce(&mut Cpu)) -> Option<u32> {
    if timeout == 0 {
        return Some(E_ABORT);
    }
    let now = cpu.bus.clock.now_micros();
    let limit = (timeout != INFINITE).then(|| now + timeout as u64 * 1000);
    let end = ends_at(cpu, id);
    let (deadline, result) = match end {
        Some(end) if limit.is_none_or(|l| end <= l) => {
            took(cpu);
            if let Some(g) = graph(cpu, id) {
                g.complete_taken = true;
            }
            (Some(end), S_OK)
        }
        _ => (limit, E_ABORT),
    };
    cpu.set_eax(result);
    thread::block(
        cpu,
        Wait {
            deadline,
            timeout_result: result,
            ..Wait::default()
        },
    );
    None
}

/// IMediaEvent::WaitForCompletion(this, timeout, code): until the play
/// ends, its EC_COMPLETE taken with the events before it.
pub fn wait_for_completion(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, timeout, code] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let id = hr!(graph_of(cpu, this));
    hr!(put32(cpu, code, 0));
    let Some(g) = graph(cpu, id) else {
        return Some(E_POINTER);
    };
    if g.state != STATE_RUNNING {
        return Some(VFW_E_WRONG_STATE);
    }
    if let Some(k) = g.events.iter().position(|e| e.0 == EC_COMPLETE) {
        g.events.drain(..=k);
        let (empty, event) = (g.events.is_empty(), g.event.1);
        if empty {
            super::reset_event(cpu, event);
        }
        cpu.bus.guest_write_32(code, EC_COMPLETE);
        return Some(S_OK);
    }
    wait_for(cpu, id, timeout, |cpu| {
        cpu.bus.guest_write_32(code, EC_COMPLETE)
    })
}

/// IMediaEventEx::SetNotifyWindow(this, window, message, data).
pub fn set_notify_window(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(graph_of(cpu, args[0]));
    if args[1] != 0 && !user32::user(cpu).is_window(args[1]) {
        return Some(E_INVALIDARG);
    }
    if let Some(g) = graph(cpu, id) {
        g.notify = (args[1], args[2], args[3]);
    }
    Some(S_OK)
}

/// IMediaEventEx::SetNotifyFlags(this, flags).
pub fn set_notify_flags(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(graph_of(cpu, args[0]));
    if args[1] & !AM_MEDIAEVENT_NONOTIFY != 0 {
        return Some(E_INVALIDARG);
    }
    let Some(g) = graph(cpu, id) else {
        return Some(E_POINTER);
    };
    g.notify_flags = args[1];
    let (queued, event) = (!g.events.is_empty(), g.event.1);
    if args[1] & AM_MEDIAEVENT_NONOTIFY != 0 {
        super::reset_event(cpu, event);
    } else if queued {
        super::set_event(cpu, event);
    }
    Some(S_OK)
}

/// IMediaEventEx::GetNotifyFlags(this, out).
pub fn get_notify_flags(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(graph_of(cpu, args[0]));
    let flags = graph(cpu, id).map_or(0, |g| g.notify_flags);
    Some(put32(cpu, args[1], flags).map_or_else(|e| e, |_| S_OK))
}

// Positions: IMediaPosition and IMediaSeeking.

/// Move the play of the graph `id` to `ms`: a running graph plays on
/// from there, another shows the picture there.
fn seek_to(cpu: &mut Cpu, id: u32, i: usize, ms: u32) -> Result<(), u32> {
    let length = media(cpu, i)?.length_ms();
    let ms = ms.min(length);
    let running = graph(cpu, id).is_some_and(|g| g.state == STATE_RUNNING);
    if let Some(g) = graph(cpu, id) {
        g.complete_taken = false;
    }
    let to = mci::device(cpu, i).video.to;
    if running && ms < to {
        command(cpu, i, MCI_PLAY, MCI_FROM | MCI_TO, ms, to)
    } else {
        command(cpu, i, MCI_SEEK, MCI_TO, 0, ms)?;
        if running {
            queue(cpu, id, EC_COMPLETE, S_OK, 0);
        }
        Ok(())
    }
}

/// The stop position of the graph's play: where a play ends.
fn set_stop(cpu: &mut Cpu, i: usize, ms: u32) -> Result<(), u32> {
    let length = media(cpu, i)?.length_ms();
    mci::device(cpu, i).video.to = ms.min(length);
    crate::win32::winmm::schedule(cpu);
    Ok(())
}

/// A double the program passed as two arguments.
fn arg_f64(lo: u32, hi: u32) -> f64 {
    f64::from_bits((hi as u64) << 32 | lo as u64)
}

/// Seconds (REFTIME) as milliseconds, 0 for less.
fn seconds_ms(s: f64) -> u32 {
    (s * 1000.0).round().clamp(0.0, u32::MAX as f64) as u32
}

fn put_seconds(cpu: &mut Cpu, at: u32, ms: u32) -> Option<u32> {
    let s = ms as f64 / 1000.0;
    Some(put64(cpu, at, s.to_bits()).map_or_else(|e| e, |_| S_OK))
}

/// IMediaPosition::get_Duration(this, out).
pub fn get_duration_seconds(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, i) = hr!(player(cpu, args[0]));
    let ms = hr!(media(cpu, i)).length_ms();
    put_seconds(cpu, args[1], ms)
}

/// IMediaPosition::get_CurrentPosition(this, out).
pub fn get_current_seconds(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, i) = hr!(player(cpu, args[0]));
    let (pos, _) = positions(cpu, i);
    put_seconds(cpu, args[1], pos)
}

/// IMediaPosition::put_CurrentPosition(this, seconds).
pub fn put_current_seconds(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (this, ms) = (args[0], seconds_ms(arg_f64(args[1], args[2])));
    with_calls(cpu, |cpu| {
        let (id, i) = player(cpu, this)?;
        seek_to(cpu, id, i, ms)?;
        Ok(S_OK)
    })
}

/// IMediaPosition::get_StopTime(this, out).
pub fn get_stop_seconds(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, i) = hr!(player(cpu, args[0]));
    let (_, to) = positions(cpu, i);
    put_seconds(cpu, args[1], to)
}

/// IMediaPosition::put_StopTime(this, seconds).
pub fn put_stop_seconds(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, i) = hr!(player(cpu, args[0]));
    hr!(set_stop(cpu, i, seconds_ms(arg_f64(args[1], args[2]))));
    Some(S_OK)
}

/// IMediaPosition::get_PrerollTime(this, out): none.
pub fn get_zero_seconds(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(player(cpu, args[0]));
    put_seconds(cpu, args[1], 0)
}

/// IMediaPosition::put_Rate and IMediaSeeking::SetRate(this, rate):
/// only 1.
pub fn put_rate(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(player(cpu, args[0]));
    Some(if arg_f64(args[1], args[2]) == 1.0 {
        S_OK
    } else {
        E_NOTIMPL
    })
}

/// IMediaPosition::get_Rate and IMediaSeeking::GetRate(this, out): 1.
pub fn get_rate(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(player(cpu, args[0]));
    Some(put64(cpu, args[1], 1f64.to_bits()).map_or_else(|e| e, |_| S_OK))
}

/// IMediaPosition::CanSeekForward and CanSeekBackward(this, out).
pub fn can_seek(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(player(cpu, args[0]));
    Some(put32(cpu, args[1], OATRUE).map_or_else(|e| e, |_| S_OK))
}

/// IMediaSeeking::GetCapabilities(this, out).
pub fn get_capabilities(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(player(cpu, args[0]));
    Some(put32(cpu, args[1], SEEKING_CAPS).map_or_else(|e| e, |_| S_OK))
}

/// IMediaSeeking::CheckCapabilities(this, caps): those of them the
/// graph has; S_FALSE for some, E_FAIL for none.
pub fn check_capabilities(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(player(cpu, args[0]));
    if args[1] == 0 || !cpu.bus.guest_probe(args[1], 4, true) {
        return Some(E_POINTER);
    }
    let asked = cpu.bus.guest_read_32(args[1]);
    let has = asked & SEEKING_CAPS;
    cpu.bus.guest_write_32(args[1], has);
    Some(if has == asked {
        S_OK
    } else if has != 0 {
        S_FALSE
    } else {
        E_FAIL
    })
}

/// IMediaSeeking::IsFormatSupported and IsUsingTimeFormat(this, format):
/// media time.
pub fn is_format_supported(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(player(cpu, args[0]));
    match Guid::read(cpu, args[1]) {
        Some(g) if g == TIME_FORMAT_MEDIA_TIME => Some(S_OK),
        Some(_) => Some(S_FALSE),
        None => Some(E_POINTER),
    }
}

/// IMediaSeeking::GetTimeFormat and QueryPreferredFormat(this, out).
pub fn get_time_format(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(player(cpu, args[0]));
    Some(if TIME_FORMAT_MEDIA_TIME.write(cpu, args[1]) {
        S_OK
    } else {
        E_POINTER
    })
}

/// IMediaSeeking::SetTimeFormat(this, format): media time only.
pub fn set_time_format(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _) = hr!(player(cpu, args[0]));
    if graph(cpu, id).is_some_and(|g| g.state != STATE_STOPPED) {
        return Some(VFW_E_WRONG_STATE);
    }
    match Guid::read(cpu, args[1]) {
        Some(g) if g == TIME_FORMAT_MEDIA_TIME => Some(S_OK),
        Some(_) => Some(E_INVALIDARG),
        None => Some(E_POINTER),
    }
}

fn put_units(cpu: &mut Cpu, at: u32, ms: u32) -> Result<(), u32> {
    put64(cpu, at, (ms as i64 * UNITS_MS) as u64)
}

/// IMediaSeeking::GetDuration(this, out).
pub fn get_duration(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, i) = hr!(player(cpu, args[0]));
    let ms = hr!(media(cpu, i)).length_ms();
    Some(put_units(cpu, args[1], ms).map_or_else(|e| e, |_| S_OK))
}

/// IMediaSeeking::GetStopPosition(this, out).
pub fn get_stop_position(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, i) = hr!(player(cpu, args[0]));
    let (_, to) = positions(cpu, i);
    Some(put_units(cpu, args[1], to).map_or_else(|e| e, |_| S_OK))
}

/// IMediaSeeking::GetCurrentPosition(this, out).
pub fn get_current_position(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, i) = hr!(player(cpu, args[0]));
    let (pos, _) = positions(cpu, i);
    Some(put_units(cpu, args[1], pos).map_or_else(|e| e, |_| S_OK))
}

/// IMediaSeeking::GetPositions(this, current, stop): either may be NULL.
pub fn get_positions(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, i) = hr!(player(cpu, args[0]));
    let (pos, to) = positions(cpu, i);
    for (at, ms) in [(args[1], pos), (args[2], to)] {
        if at != 0 {
            hr!(put_units(cpu, at, ms));
        }
    }
    Some(S_OK)
}

/// IMediaSeeking::GetAvailable(this, earliest, latest): all of it.
pub fn get_available(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, i) = hr!(player(cpu, args[0]));
    let length = hr!(media(cpu, i)).length_ms();
    for (at, ms) in [(args[1], 0), (args[2], length)] {
        if at != 0 {
            hr!(put_units(cpu, at, ms));
        }
    }
    Some(S_OK)
}

/// IMediaSeeking::GetPreroll(this, out): none.
pub fn get_preroll(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(player(cpu, args[0]));
    Some(put64(cpu, args[1], 0).map_or_else(|e| e, |_| S_OK))
}

/// IMediaSeeking::ConvertTimeFormat(this, target, target format, source
/// (64 bits), source format): media time to media time. A NULL format
/// is the one the graph uses.
pub fn convert_time_format(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, target, to_format, lo, hi, from_format] = args[..] else {
        return Some(E_INVALIDARG);
    };
    hr!(player(cpu, this));
    for f in [to_format, from_format] {
        if f != 0 && Guid::read(cpu, f) != Some(TIME_FORMAT_MEDIA_TIME) {
            return Some(E_INVALIDARG);
        }
    }
    let value = (hi as u64) << 32 | lo as u64;
    Some(put64(cpu, target, value).map_or_else(|e| e, |_| S_OK))
}

/// IMediaSeeking::SetPositions(this, current, its flags, stop, its
/// flags): each absolute, relative to where it is, or (the stop)
/// relative to the new current position; with AM_SEEKING_ReturnTime
/// the position set is written back.
pub fn set_positions(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, current, current_flags, stop_at, stop_flags] = args[..] else {
        return Some(E_INVALIDARG);
    };
    with_calls(cpu, |cpu| {
        let (id, i) = player(cpu, this)?;
        let (pos, to) = positions(cpu, i);
        let ms =
            |cpu: &mut Cpu, at: u32| -> Result<i64, u32> { Ok(get64(cpu, at)? as i64 / UNITS_MS) };
        let clamp = |v: i64| v.clamp(0, u32::MAX as i64) as u32;
        let mut new_pos = None;
        match current_flags & AM_SEEKING_POSITIONING_MASK {
            AM_SEEKING_ABSOLUTE => new_pos = Some(clamp(ms(cpu, current)?)),
            AM_SEEKING_RELATIVE => new_pos = Some(clamp(pos as i64 + ms(cpu, current)?)),
            0 => {}
            _ => return Err(E_INVALIDARG),
        }
        let base = new_pos.unwrap_or(pos) as i64;
        let new_stop = match stop_flags & AM_SEEKING_POSITIONING_MASK {
            AM_SEEKING_ABSOLUTE => Some(clamp(ms(cpu, stop_at)?)),
            AM_SEEKING_RELATIVE => Some(clamp(to as i64 + ms(cpu, stop_at)?)),
            AM_SEEKING_INCREMENTAL => Some(clamp(base + ms(cpu, stop_at)?)),
            _ => None,
        };
        if let Some(s) = new_stop {
            set_stop(cpu, i, s)?;
        }
        if let Some(p) = new_pos {
            seek_to(cpu, id, i, p)?;
        }
        let (pos, to) = positions(cpu, i);
        if current_flags & AM_SEEKING_RETURN_TIME != 0 && new_pos.is_some() {
            put_units(cpu, current, pos)?;
        }
        if stop_flags & AM_SEEKING_RETURN_TIME != 0 && new_stop.is_some() {
            put_units(cpu, stop_at, to)?;
        }
        Ok(S_OK)
    })
}

// IVideoWindow.

/// The window's rectangle in its parent's client area: left, top,
/// width, height.
fn window_rect(cpu: &mut Cpu, hwnd: u32) -> [i32; 4] {
    user32::user(cpu).window(hwnd).map_or([0; 4], |w| {
        let r = w.rect;
        [r.left, r.top, r.width(), r.height()]
    })
}

fn put_window_rect(cpu: &mut Cpu, this: u32, f: impl Fn([i32; 4]) -> [i32; 4]) -> Option<u32> {
    with_calls(cpu, |cpu| {
        let (_, i, _) = renderer(cpu, this)?;
        let own = own(cpu, i);
        let [x, y, w, h] = f(window_rect(cpu, own));
        let args = [own, x as u32, y as u32, w as u32, h as u32, 1];
        call(cpu, "MoveWindow", &args, &[])?;
        Ok(S_OK)
    })
}

fn get_window_value(cpu: &mut Cpu, args: &[u32], k: usize) -> Option<u32> {
    let (_, i, _) = hr!(renderer(cpu, args[0]));
    let own = own(cpu, i);
    let value = window_rect(cpu, own)[k];
    Some(put32(cpu, args[1], value as u32).map_or_else(|e| e, |_| S_OK))
}

/// IVideoWindow::put_Left, put_Top, put_Width, put_Height(this, value).
pub fn put_left(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let v = args[1] as i32;
    put_window_rect(cpu, args[0], |[_, y, w, h]| [v, y, w, h])
}

pub fn put_top(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let v = args[1] as i32;
    put_window_rect(cpu, args[0], |[x, _, w, h]| [x, v, w, h])
}

pub fn put_width(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let v = args[1] as i32;
    put_window_rect(cpu, args[0], |[x, y, _, h]| [x, y, v, h])
}

pub fn put_height(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let v = args[1] as i32;
    put_window_rect(cpu, args[0], |[x, y, w, _]| [x, y, w, v])
}

/// IVideoWindow::get_Left, get_Top, get_Width, get_Height(this, out).
pub fn get_left(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_window_value(cpu, args, 0)
}

pub fn get_top(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_window_value(cpu, args, 1)
}

pub fn get_width(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_window_value(cpu, args, 2)
}

pub fn get_height(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_window_value(cpu, args, 3)
}

/// IVideoWindow::SetWindowPosition(this, left, top, width, height).
pub fn set_window_position(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = [args[1], args[2], args[3], args[4]].map(|v| v as i32);
    put_window_rect(cpu, args[0], |_| r)
}

/// IVideoWindow::GetWindowPosition and GetRestorePosition(this, left,
/// top, width, height).
pub fn get_window_position(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, i, _) = hr!(renderer(cpu, args[0]));
    let own = own(cpu, i);
    let r = window_rect(cpu, own);
    for (at, v) in args[1..5].iter().zip(r) {
        hr!(put32(cpu, *at, v as u32));
    }
    Some(S_OK)
}

/// IVideoWindow::put_Caption(this, text).
pub fn put_caption(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (this, text) = (args[0], args[1]);
    with_calls(cpu, |cpu| {
        let (_, i, _) = renderer(cpu, this)?;
        let own = own(cpu, i);
        let text = crate::win32::dmusic::read_wide(cpu, text, 1024);
        call(cpu, "SetWindowTextA", &[own, 0], &[(1, &text)])?;
        Ok(S_OK)
    })
}

/// IVideoWindow::get_Caption(this, out): a new BSTR.
pub fn get_caption(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, i, _) = hr!(renderer(cpu, args[0]));
    let own = own(cpu, i);
    let text = user32::user(cpu)
        .window(own)
        .map(|w| String::from_utf8_lossy(&w.text).into_owned())
        .unwrap_or_default();
    let bstr = crate::win32::oleaut32::new_bstr(cpu, &text);
    if bstr == 0 {
        return Some(super::E_OUTOFMEMORY);
    }
    Some(put32(cpu, args[1], bstr).map_or_else(|e| e, |_| S_OK))
}

fn set_long(cpu: &mut Cpu, this: u32, index: u32, value: u32) -> Option<u32> {
    with_calls(cpu, |cpu| {
        let (_, i, _) = renderer(cpu, this)?;
        let own = own(cpu, i);
        call(cpu, "SetWindowLongA", &[own, index, value], &[])?;
        Ok(S_OK)
    })
}

/// IVideoWindow::put_WindowStyle and put_WindowStyleEx(this, style).
pub fn put_window_style(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    set_long(cpu, args[0], GWL_STYLE, args[1])
}

pub fn put_window_style_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    set_long(cpu, args[0], GWL_EXSTYLE, args[1])
}

/// IVideoWindow::get_WindowStyle and get_WindowStyleEx(this, out).
pub fn get_window_style(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, i, _) = hr!(renderer(cpu, args[0]));
    let own = own(cpu, i);
    let style = user32::user(cpu).window(own).map_or(0, |w| w.style);
    Some(put32(cpu, args[1], style).map_or_else(|e| e, |_| S_OK))
}

pub fn get_window_style_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, i, _) = hr!(renderer(cpu, args[0]));
    let own = own(cpu, i);
    let style = user32::user(cpu).window(own).map_or(0, |w| w.ex_style);
    Some(put32(cpu, args[1], style).map_or_else(|e| e, |_| S_OK))
}

/// IVideoWindow::put_AutoShow(this, on): whether running or pausing
/// the graph shows the window.
pub fn put_auto_show(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, i, _) = hr!(renderer(cpu, args[0]));
    let manual = args[1] == OAFALSE;
    if let Some(g) = graph(cpu, id) {
        g.manual_show = manual;
    }
    mci::device(cpu, i).video.manual_show = manual;
    Some(S_OK)
}

pub fn get_auto_show(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _, _) = hr!(renderer(cpu, args[0]));
    let manual = graph(cpu, id).is_some_and(|g| g.manual_show);
    let value = if manual { OAFALSE } else { OATRUE };
    Some(put32(cpu, args[1], value).map_or_else(|e| e, |_| S_OK))
}

/// IVideoWindow::put_WindowState(this, SW_*).
pub fn put_window_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (this, cmd) = (args[0], args[1]);
    with_calls(cpu, |cpu| {
        let (_, i, _) = renderer(cpu, this)?;
        let own = own(cpu, i);
        call(cpu, "ShowWindow", &[own, cmd], &[])?;
        Ok(S_OK)
    })
}

/// IVideoWindow::get_WindowState(this, out): hidden, minimized,
/// maximized or shown.
pub fn get_window_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, i, _) = hr!(renderer(cpu, args[0]));
    let own = own(cpu, i);
    let state = user32::user(cpu);
    let style = state.window(own).map_or(0, |w| w.style);
    let value = if !state.is_visible(own) {
        SW_HIDE
    } else if style & WS_MINIMIZE != 0 {
        SW_SHOWMINIMIZED
    } else if style & WS_MAXIMIZE != 0 {
        SW_SHOWMAXIMIZED
    } else {
        SW_SHOWNORMAL
    };
    Some(put32(cpu, args[1], value).map_or_else(|e| e, |_| S_OK))
}

/// IVideoWindow::put_BackgroundPalette(this, on).
pub fn put_background_palette(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _, _) = hr!(renderer(cpu, args[0]));
    if let Some(g) = graph(cpu, id) {
        g.background_palette = args[1] != OAFALSE;
    }
    Some(S_OK)
}

pub fn get_background_palette(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _, _) = hr!(renderer(cpu, args[0]));
    let on = graph(cpu, id).is_some_and(|g| g.background_palette);
    Some(put32(cpu, args[1], if on { OATRUE } else { OAFALSE }).map_or_else(|e| e, |_| S_OK))
}

/// IVideoWindow::put_Visible(this, on).
pub fn put_visible(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let cmd = if args[1] == OAFALSE { SW_HIDE } else { SW_SHOW };
    put_window_state(cpu, &[args[0], cmd])
}

pub fn get_visible(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, i, _) = hr!(renderer(cpu, args[0]));
    let own = own(cpu, i);
    let on = user32::user(cpu).is_visible(own);
    Some(put32(cpu, args[1], if on { OATRUE } else { OAFALSE }).map_or_else(|e| e, |_| S_OK))
}

/// IVideoWindow::put_Owner(this, window): the video window becomes a
/// child of `window`, or with 0 a top-level window again, as the video
/// renderer's put_Owner makes it: WS_CHILD set or cleared, then the new
/// parent.
pub fn put_owner(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (this, owner) = (args[0], args[1]);
    with_calls(cpu, |cpu| {
        let (_, i, _) = renderer(cpu, this)?;
        let own = own(cpu, i);
        if owner != 0 && !user32::user(cpu).is_window(owner) {
            return Err(E_INVALIDARG);
        }
        let style = user32::user(cpu).window(own).map_or(0, |w| w.style);
        let style = if owner != 0 {
            (style | WS_CHILD) & !WS_POPUP
        } else {
            style & !WS_CHILD
        };
        call(cpu, "SetWindowLongA", &[own, GWL_STYLE, style], &[])?;
        call(cpu, "SetParent", &[own, owner], &[])?;
        Ok(S_OK)
    })
}

/// IVideoWindow::get_Owner(this, out): the window it is a child of.
pub fn get_owner(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, i, _) = hr!(renderer(cpu, args[0]));
    let own = own(cpu, i);
    let state = user32::user(cpu);
    let owner = state
        .window(own)
        .filter(|w| w.style & WS_CHILD != 0)
        .map_or(0, |w| w.parent);
    Some(put32(cpu, args[1], owner).map_or_else(|e| e, |_| S_OK))
}

/// IVideoWindow::put_MessageDrain(this, window): kept. The renderer's
/// window gets no keyboard or mouse messages of its own to pass on.
pub fn put_message_drain(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _, _) = hr!(renderer(cpu, args[0]));
    if let Some(g) = graph(cpu, id) {
        g.drain = args[1];
    }
    Some(S_OK)
}

pub fn get_message_drain(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _, _) = hr!(renderer(cpu, args[0]));
    let drain = graph(cpu, id).map_or(0, |g| g.drain);
    Some(put32(cpu, args[1], drain).map_or_else(|e| e, |_| S_OK))
}

/// IVideoWindow::put_BorderColor and get_BorderColor: kept.
pub fn put_border_color(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _, _) = hr!(renderer(cpu, args[0]));
    if let Some(g) = graph(cpu, id) {
        g.border_color = args[1];
    }
    Some(S_OK)
}

pub fn get_border_color(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _, _) = hr!(renderer(cpu, args[0]));
    let color = graph(cpu, id).map_or(0, |g| g.border_color);
    Some(put32(cpu, args[1], color).map_or_else(|e| e, |_| S_OK))
}

/// IVideoWindow::get_FullScreenMode(this, out): never.
pub fn get_full_screen_mode(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(renderer(cpu, args[0]));
    Some(put32(cpu, args[1], OAFALSE).map_or_else(|e| e, |_| S_OK))
}

/// IVideoWindow::put_FullScreenMode(this, on): the full-screen renderer
/// isn't there, so only off.
pub fn put_full_screen_mode(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(renderer(cpu, args[0]));
    Some(if args[1] == OAFALSE { S_OK } else { E_NOTIMPL })
}

/// IVideoWindow::SetWindowForeground(this, focus).
pub fn set_window_foreground(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let this = args[0];
    with_calls(cpu, |cpu| {
        let (_, i, _) = renderer(cpu, this)?;
        let own = own(cpu, i);
        call(cpu, "SetForegroundWindow", &[own], &[])?;
        Ok(S_OK)
    })
}

/// IVideoWindow::NotifyOwnerMessage(this, window, message, wparam,
/// lparam): the renderer has nothing to do with the owner's palette and
/// display changes.
pub fn notify_owner_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(renderer(cpu, args[0]));
    Some(S_OK)
}

/// IVideoWindow::GetMinIdealImageSize and GetMaxIdealImageSize(this,
/// width, height): the picture's own size.
pub fn get_ideal_image_size(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, _, m) = hr!(renderer(cpu, args[0]));
    hr!(put32(cpu, args[1], m.width));
    hr!(put32(cpu, args[2], m.height));
    Some(S_OK)
}

/// IVideoWindow::HideCursor and IsCursorHidden: kept.
pub fn hide_cursor(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _, _) = hr!(renderer(cpu, args[0]));
    if let Some(g) = graph(cpu, id) {
        g.cursor_hidden = args[1] != OAFALSE;
    }
    Some(S_OK)
}

pub fn is_cursor_hidden(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _, _) = hr!(renderer(cpu, args[0]));
    let on = graph(cpu, id).is_some_and(|g| g.cursor_hidden);
    Some(put32(cpu, args[1], if on { OATRUE } else { OAFALSE }).map_or_else(|e| e, |_| S_OK))
}

// IBasicVideo.

/// IBasicVideo::get_AvgTimePerFrame(this, out): seconds.
pub fn get_avg_time_per_frame(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, _, m) = hr!(renderer(cpu, args[0]));
    let (num, den) = m.rate;
    let s = den as f64 / num.max(1) as f64;
    Some(put64(cpu, args[1], s.to_bits()).map_or_else(|e| e, |_| S_OK))
}

/// IBasicVideo::get_BitRate(this, out): the pictures' bits a second.
pub fn get_bit_rate(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, _, m) = hr!(renderer(cpu, args[0]));
    let rate = (m.video_bytes() as u64 * 8 * 1000 / m.length_ms().max(1) as u64) as u32;
    Some(put32(cpu, args[1], rate).map_or_else(|e| e, |_| S_OK))
}

/// IBasicVideo::get_BitErrorRate(this, out): none.
pub fn get_bit_error_rate(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(renderer(cpu, args[0]));
    Some(put32(cpu, args[1], 0).map_or_else(|e| e, |_| S_OK))
}

/// IBasicVideo::get_VideoWidth and get_VideoHeight(this, out).
pub fn get_video_width(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, _, m) = hr!(renderer(cpu, args[0]));
    Some(put32(cpu, args[1], m.width).map_or_else(|e| e, |_| S_OK))
}

pub fn get_video_height(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, _, m) = hr!(renderer(cpu, args[0]));
    Some(put32(cpu, args[1], m.height).map_or_else(|e| e, |_| S_OK))
}

/// IBasicVideo::GetVideoSize(this, width, height).
pub fn get_video_size(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_ideal_image_size(cpu, args)
}

/// The source rectangle (`source`) or the destination one: left, top,
/// width, height. The destination is the window's client area unless
/// one was set.
fn video_rect(cpu: &mut Cpu, i: usize, source: bool) -> [i32; 4] {
    let v = &mci::device(cpu, i).video;
    if source {
        return v.source;
    }
    if let Some(d) = v.dest {
        return d;
    }
    let own = v.hwnd;
    user32::user(cpu)
        .window(own)
        .map_or([0; 4], |w| [0, 0, w.client.width(), w.client.height()])
}

/// Set a rectangle of the graph `this`: the source must be inside the
/// picture, and neither may be empty.
fn set_video_rect(
    cpu: &mut Cpu,
    this: u32,
    source: bool,
    f: impl Fn([i32; 4]) -> [i32; 4],
) -> Option<u32> {
    let (_, i, m) = hr!(renderer(cpu, this));
    let r = f(video_rect(cpu, i, source));
    let [x, y, w, h] = r;
    if w <= 0 || h <= 0 {
        return Some(E_INVALIDARG);
    }
    if source && (x < 0 || y < 0 || x + w > m.width as i32 || y + h > m.height as i32) {
        return Some(E_INVALIDARG);
    }
    let v = &mut mci::device(cpu, i).video;
    if source {
        v.source = r;
    } else {
        v.dest = Some(r);
    }
    v.drawn = None;
    Some(S_OK)
}

fn get_video_value(cpu: &mut Cpu, args: &[u32], source: bool, k: usize) -> Option<u32> {
    let (_, i, _) = hr!(renderer(cpu, args[0]));
    let value = video_rect(cpu, i, source)[k];
    Some(put32(cpu, args[1], value as u32).map_or_else(|e| e, |_| S_OK))
}

/// IBasicVideo's put_Source* and put_Destination*(this, value), and
/// their get_*(this, out): one side of a rectangle each.
macro_rules! rect_value {
    ($put:ident, $get:ident, $source:expr, $k:expr) => {
        pub fn $put(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
            let v = args[1] as i32;
            set_video_rect(cpu, args[0], $source, |mut r| {
                r[$k] = v;
                r
            })
        }

        pub fn $get(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
            get_video_value(cpu, args, $source, $k)
        }
    };
}

rect_value!(put_source_left, get_source_left, true, 0);
rect_value!(put_source_top, get_source_top, true, 1);
rect_value!(put_source_width, get_source_width, true, 2);
rect_value!(put_source_height, get_source_height, true, 3);
rect_value!(put_dest_left, get_dest_left, false, 0);
rect_value!(put_dest_top, get_dest_top, false, 1);
rect_value!(put_dest_width, get_dest_width, false, 2);
rect_value!(put_dest_height, get_dest_height, false, 3);

/// IBasicVideo::SetSourcePosition and SetDestinationPosition(this,
/// left, top, width, height).
pub fn set_source_position(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = [args[1], args[2], args[3], args[4]].map(|v| v as i32);
    set_video_rect(cpu, args[0], true, |_| r)
}

pub fn set_dest_position(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = [args[1], args[2], args[3], args[4]].map(|v| v as i32);
    set_video_rect(cpu, args[0], false, |_| r)
}

fn get_video_position(cpu: &mut Cpu, args: &[u32], source: bool) -> Option<u32> {
    let (_, i, _) = hr!(renderer(cpu, args[0]));
    let r = video_rect(cpu, i, source);
    for (at, v) in args[1..5].iter().zip(r) {
        hr!(put32(cpu, *at, v as u32));
    }
    Some(S_OK)
}

/// IBasicVideo::GetSourcePosition and GetDestinationPosition(this,
/// left, top, width, height).
pub fn get_source_position(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_video_position(cpu, args, true)
}

pub fn get_dest_position(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_video_position(cpu, args, false)
}

/// IBasicVideo::SetDefaultSourcePosition(this): the whole picture.
pub fn set_default_source(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, i, m) = hr!(renderer(cpu, args[0]));
    let v = &mut mci::device(cpu, i).video;
    v.source = [0, 0, m.width as i32, m.height as i32];
    v.drawn = None;
    Some(S_OK)
}

/// IBasicVideo::SetDefaultDestinationPosition(this): the client area.
pub fn set_default_dest(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, i, _) = hr!(renderer(cpu, args[0]));
    let v = &mut mci::device(cpu, i).video;
    v.dest = None;
    v.drawn = None;
    Some(S_OK)
}

/// IBasicVideo::IsUsingDefaultSource(this): S_OK or S_FALSE.
pub fn is_using_default_source(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, i, m) = hr!(renderer(cpu, args[0]));
    let whole = [0, 0, m.width as i32, m.height as i32];
    Some(if mci::device(cpu, i).video.source == whole {
        S_OK
    } else {
        S_FALSE
    })
}

/// IBasicVideo::IsUsingDefaultDestination(this).
pub fn is_using_default_dest(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, i, _) = hr!(renderer(cpu, args[0]));
    Some(if mci::device(cpu, i).video.dest.is_none() {
        S_OK
    } else {
        S_FALSE
    })
}

/// IBasicVideo::GetVideoPaletteEntries: the pictures are never of a
/// palette's colours here (24-bit DIBs).
pub fn get_video_palette_entries(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(renderer(cpu, args[0]));
    Some(VFW_E_NO_PALETTE_AVAILABLE)
}

/// IBasicVideo::GetCurrentImage(this, size, image): the picture the
/// play is at, as a BITMAPINFOHEADER and a 24-bit bottom-up DIB. With a
/// NULL image, the size it needs.
pub fn get_current_image(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, size_at, image] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let (_, i, m) = hr!(renderer(cpu, this));
    let (w, h) = (m.width as i32, m.height as i32);
    let stride = crate::win32::gdi32::surface::dib_stride(w, 24);
    let need = 40 + stride as u32 * h as u32;
    if size_at == 0 || !cpu.bus.guest_probe(size_at, 4, true) {
        return Some(E_POINTER);
    }
    if image == 0 {
        cpu.bus.guest_write_32(size_at, need);
        return Some(S_OK);
    }
    if cpu.bus.guest_read_32(size_at) < need || !cpu.bus.guest_probe(image, need as usize, true) {
        return Some(E_OUTOFMEMORY_IMAGE);
    }
    let (pos, _) = positions(cpu, i);
    let n = m.frame_at(pos as u64);
    let rows = {
        let v = &mut mci::device(cpu, i).video;
        match v.pictures.rows(&m, n, stride) {
            Some(r) => r,
            None => return Some(E_FAIL),
        }
    };
    let mut out = Vec::with_capacity(need as usize);
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(stride as u32 * h as u32).to_le_bytes());
    out.extend_from_slice(&[0; 16]);
    for row in rows.chunks_exact(stride).rev() {
        out.extend_from_slice(row);
    }
    cpu.bus.guest_write_bytes(image, &out);
    cpu.bus.guest_write_32(size_at, need);
    Some(S_OK)
}

/// GetCurrentImage's error for a buffer too small: E_OUTOFMEMORY.
const E_OUTOFMEMORY_IMAGE: u32 = super::E_OUTOFMEMORY;

// IBasicAudio.

/// The graph `this` and its device, if its file has sound: IBasicAudio
/// is the audio renderer's, which a file without sound doesn't have.
fn audio(cpu: &mut Cpu, this: u32) -> Result<usize, u32> {
    let (_, i) = player(cpu, this)?;
    if media(cpu, i)?.audio.is_none() {
        return Err(E_NOINTERFACE);
    }
    Ok(i)
}

/// IBasicAudio::put_Volume(this, volume): -10000 (silence) to 0 (full),
/// in hundredths of a decibel.
pub fn put_volume(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let i = hr!(audio(cpu, args[0]));
    let volume = args[1] as i32;
    if !(-10000..=0).contains(&volume) {
        return Some(E_INVALIDARG);
    }
    let balance = mci::device(cpu, i).video.balance;
    video::set_gain(cpu, i, volume, balance);
    Some(S_OK)
}

pub fn get_volume(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let i = hr!(audio(cpu, args[0]));
    let volume = mci::device(cpu, i).video.volume;
    Some(put32(cpu, args[1], volume as u32).map_or_else(|e| e, |_| S_OK))
}

/// IBasicAudio::put_Balance(this, balance): -10000 (left only) to 10000
/// (right only).
pub fn put_balance(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let i = hr!(audio(cpu, args[0]));
    let balance = args[1] as i32;
    if !(-10000..=10000).contains(&balance) {
        return Some(E_INVALIDARG);
    }
    let volume = mci::device(cpu, i).video.volume;
    video::set_gain(cpu, i, volume, balance);
    Some(S_OK)
}

pub fn get_balance(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let i = hr!(audio(cpu, args[0]));
    let balance = mci::device(cpu, i).video.balance;
    Some(put32(cpu, args[1], balance as u32).map_or_else(|e| e, |_| S_OK))
}

// The DLL's own functions.

/// The texts AMGetErrorText gives, for the errors the graph returns.
const ERROR_TEXTS: &[(u32, &str)] = &[
    (
        VFW_E_UNKNOWN_FILE_TYPE,
        "The media type of this file is not recognized.",
    ),
    (VFW_E_NOT_FOUND, "An object or name was not found."),
    (
        VFW_E_NOT_STOPPED,
        "The operation could not be performed because the filter is not stopped.",
    ),
    (
        VFW_E_WRONG_STATE,
        "The operation cannot be performed at this time.",
    ),
    (VFW_E_NO_PALETTE_AVAILABLE, "No palette is available."),
];

fn error_text(hr: u32) -> Option<&'static str> {
    ERROR_TEXTS.iter().find(|(h, _)| *h == hr).map(|(_, t)| *t)
}

/// AMGetErrorTextA(hr, buffer, characters): the text's length, or 0 for
/// an error it has no text for or a buffer too small.
pub fn am_get_error_text_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [hr, buf, max] = args[..] else {
        return Some(0);
    };
    let Some(text) = error_text(hr) else {
        return Some(0);
    };
    let n = text.len() as u32;
    if buf == 0 || max <= n || !cpu.bus.guest_probe(buf, n as usize + 1, true) {
        return Some(0);
    }
    let mut bytes = text.as_bytes().to_vec();
    bytes.push(0);
    cpu.bus.guest_write_bytes(buf, &bytes);
    Some(n)
}

/// AMGetErrorTextW(hr, buffer, characters).
pub fn am_get_error_text_w(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [hr, buf, max] = args[..] else {
        return Some(0);
    };
    let Some(text) = error_text(hr) else {
        return Some(0);
    };
    let n = text.len() as u32;
    if buf == 0 || max <= n || !cpu.bus.guest_probe(buf, 2 * (n as usize + 1), true) {
        return Some(0);
    }
    crate::win32::dmusic::write_wide(cpu, buf, text, n as usize + 1);
    Some(n)
}

/// AmpFactorToDB(factor): an amplitude, 65536 for 1, in hundredths of a
/// decibel, -10000 at the least.
pub fn amp_factor_to_db(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(to_db(args[0] as i32) as u32)
}

fn to_db(factor: i32) -> i32 {
    if factor <= 0 {
        return -10000;
    }
    let db = 2000.0 * (factor as f64 / 65536.0).log10();
    (db.round() as i32).max(-10000)
}

/// DBToAmpFactor(db): the amplitude of hundredths of a decibel, 65536 for
/// 0.
pub fn db_to_amp_factor(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(to_amp(args[0] as i32))
}

fn to_amp(db: i32) -> u32 {
    let db = db.min(0);
    if db <= -10000 {
        return 0;
    }
    (65536.0 * 10f64.powf(db as f64 / 2000.0)).round() as u32
}

crate::state_fields!(Graph {
    file,
    state,
    events,
    event,
    notify,
    notify_flags,
    manual_show,
    drain,
    border_color,
    background_palette,
    cursor_hidden,
    complete_taken
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amplitudes_and_decibels_convert_both_ways() {
        assert_eq!(to_db(65536), 0);
        assert_eq!(to_db(6554), -2000);
        assert_eq!(to_db(0), -10000);
        assert_eq!(to_amp(0), 65536);
        assert_eq!(to_amp(-2000), 6554);
        assert_eq!(to_amp(-10000), 0);
        assert_eq!(to_amp(500), 65536);
    }

    #[test]
    fn seconds_round_to_milliseconds() {
        assert_eq!(seconds_ms(1.5), 1500);
        assert_eq!(seconds_ms(-3.0), 0);
        let b = 2.25f64.to_bits();
        assert_eq!(arg_f64(b as u32, (b >> 32) as u32), 2.25);
    }

    #[test]
    fn every_error_the_graph_returns_has_a_text() {
        for hr in [
            VFW_E_UNKNOWN_FILE_TYPE,
            VFW_E_NOT_STOPPED,
            VFW_E_WRONG_STATE,
        ] {
            assert!(error_text(hr).is_some(), "{hr:08X}");
        }
        assert!(error_text(0x8000_0001).is_none());
    }
}
