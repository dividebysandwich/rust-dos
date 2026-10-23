//! Functions USER32 has from Windows 98 on (version.rs lists them, so
//! that Windows 95 doesn't): the multiple monitor functions, with the one
//! monitor the screen is, EnumDisplayDevicesA, AnimateWindow,
//! GetCursorInfo and TrackMouseEvent, and the metrics of the virtual
//! screen. What they give follows a Windows 98 SE install on a 640x480
//! screen: the monitor's handle is the one it had there, and its work
//! area is the screen, as SPI_GETWORKAREA has it here.

use super::message::{self, Msg};
use super::sysparams::screen_size;
use super::window::{SW_HIDE, SW_SHOW, SW_SHOWNA, WS_VISIBLE};
use super::{
    ERROR_INVALID_PARAMETER, ERROR_INVALID_WINDOW_HANDLE, FALSE, TRUE, address_of, read_words,
    set_error, user, write_words,
};
use crate::cpu::Cpu;
use crate::win32::gdi32::Rect;
use crate::win32::task::{self, Next, Task};

/// The monitor's handle, HMONITOR.
pub const MONITOR: u32 = 0x11A2;

/// MONITOR_DEFAULTTONULL: the functions' flags, which give no monitor
/// for a place off the screen; the others give the primary one.
const MONITOR_DEFAULTTONULL: u32 = 0;
const MONITORINFOF_PRIMARY: u32 = 1;

/// MONITORINFO's size, and MONITORINFOEXA's and MONITORINFOEXW's with
/// the device's name.
const MONITORINFO: u32 = 40;
const MONITORINFOEXA: u32 = 72;
const MONITORINFOEXW: u32 = 104;

/// The display's device name.
const DEVICE: &str = "\\\\.\\Display1";

/// The screen's rectangle, which the monitor is.
fn screen() -> Rect {
    let (w, h) = screen_size();
    Rect::new(0, 0, w, h)
}

/// GetSystemMetrics' values Windows 98 adds: SM_XVIRTUALSCREEN to
/// SM_CYVIRTUALSCREEN, SM_CMONITORS and SM_SAMEDISPLAYFORMAT.
pub fn metric(index: u32) -> Option<i32> {
    let (w, h) = screen_size();
    Some(match index {
        76 | 77 => 0,
        78 => w,
        79 => h,
        80 | 81 => 1,
        _ => return None,
    })
}

/// The monitor for a place that is or isn't on the screen.
fn monitor_for(on_screen: bool, flags: u32) -> u32 {
    if on_screen || flags != MONITOR_DEFAULTTONULL {
        MONITOR
    } else {
        0
    }
}

/// MonitorFromWindow(hwnd, flags): a window is on the screen's monitor.
pub fn monitor_from_window(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let on = user(cpu).is_window(args[0]);
    Some(monitor_for(on, args[1]))
}

/// MonitorFromPoint(point, flags), the point by value.
pub fn monitor_from_point(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    let on = screen().contains(args[0] as i32, args[1] as i32);
    Some(monitor_for(on, args[2]))
}

/// MonitorFromRect(rect, flags).
pub fn monitor_from_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = Rect::read(cpu, args[0]);
    let on = !r.intersect(&screen()).is_empty();
    Some(monitor_for(on, args[1]))
}

/// GetMonitorInfoA(monitor, info): the monitor's rectangle, its work
/// area and that it is the primary one, and in MONITORINFOEXA its
/// device's name. Another handle or size fails and leaves the last
/// error.
pub fn get_monitor_info_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    monitor_info(cpu, args, false)
}

/// GetMonitorInfoW(monitor, info), with MONITORINFOEXW's name.
pub fn get_monitor_info_w(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    monitor_info(cpu, args, true)
}

fn monitor_info(cpu: &mut Cpu, args: &[u32], wide: bool) -> Option<u32> {
    let (monitor, info) = (args[0], args[1]);
    if monitor != MONITOR || info == 0 {
        return Some(FALSE);
    }
    let size = cpu.bus.guest_read_32(info);
    let ex = if wide { MONITORINFOEXW } else { MONITORINFOEXA };
    if size != MONITORINFO && size != ex {
        return Some(FALSE);
    }
    let r = screen();
    let rect = [r.left, r.top, r.right, r.bottom].map(|v| v as u32);
    let mut words = vec![size];
    words.extend(rect);
    words.extend(rect);
    words.push(MONITORINFOF_PRIMARY);
    write_words(cpu, info, &words);
    if size == ex {
        let name: Vec<u8> = if wide {
            DEVICE
                .encode_utf16()
                .chain([0])
                .flat_map(|u| u.to_le_bytes())
                .collect()
        } else {
            DEVICE.bytes().chain([0]).collect()
        };
        cpu.bus.guest_write_bytes(info + MONITORINFO, &name);
    }
    Some(TRUE)
}

/// The task's kinds (its variable 0).
const ENUM_MONITORS: u32 = 0;
const ANIMATE: u32 = 1;

/// EnumDisplayMonitors(hdc, clip, proc, data): proc(monitor, hdc, rect,
/// data) for the monitor, if it shows. Without a DC the rectangle is the
/// monitor's on the screen, cut to `clip`; with one, the part of the
/// monitor the DC's visible area has, in the DC's coordinates, cut to
/// `clip`.
pub fn enum_display_monitors(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hdc, clip, proc, data] = args else {
        unreachable!()
    };
    let mut r = screen();
    if hdc != 0 {
        // GetDCOrgEx and GetClipBox, through the stack below the
        // arguments, which nothing uses then.
        let at = cpu.esp().wrapping_sub(64) & !3;
        let dc = crate::win32::gdi32::dc::get_dc_org_ex(cpu, &[hdc, at])?;
        if dc == FALSE {
            return Some(FALSE);
        }
        let origin = read_words(cpu, at, 2);
        crate::win32::gdi32::dc::get_clip_box(cpu, &[hdc, at + 8])?;
        let visible = Rect::read(cpu, at + 8);
        r = r
            .offset(-(origin[0] as i32), -(origin[1] as i32))
            .intersect(&visible);
    }
    if clip != 0 {
        r = r.intersect(&Rect::read(cpu, clip));
    }
    if r.is_empty() {
        return Some(TRUE);
    }
    let rect = [r.left, r.top, r.right, r.bottom].map(|v| v as u32);
    let mut vars = vec![ENUM_MONITORS, proc, data, hdc];
    vars.extend(rect);
    task::start(cpu, task::MONITOR, vars, 16)
}

/// AnimateWindow(hwnd, time, flags): the window shown or hidden at once,
/// as ShowWindow does, without the animation. It fails for a window
/// that is already as it would make it.
pub fn animate_window(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    const AW_HIDE: u32 = 0x1_0000;
    const AW_ACTIVATE: u32 = 0x2_0000;
    let (hwnd, flags) = (args[0], args[2]);
    let Some(w) = user(cpu).window(hwnd) else {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(FALSE);
    };
    let visible = w.style & WS_VISIBLE != 0;
    let hide = flags & AW_HIDE != 0;
    if visible != hide {
        return Some(FALSE);
    }
    let cmd = if hide {
        SW_HIDE
    } else if flags & AW_ACTIVATE != 0 {
        SW_SHOW
    } else {
        SW_SHOWNA
    };
    task::start(cpu, task::MONITOR, vec![ANIMATE, hwnd, cmd], 0)
}

/// The steps of EnumDisplayMonitors and AnimateWindow.
pub fn step(cpu: &mut Cpu, t: &mut Task, _value: u32) -> Next {
    if t.step == 1 {
        return Next::Done(TRUE);
    }
    match t.var(0) {
        ENUM_MONITORS => {
            let rect: Vec<u32> = (4..8).map(|i| t.var(i)).collect();
            write_words(cpu, t.scratch, &rect);
            Next::call(t.var(1), &[MONITOR, t.var(3), t.scratch, t.var(2)], 1)
        }
        _ => Next::call(address_of("ShowWindow"), &[t.var(1), t.var(2)], 1),
    }
}

/// DISPLAY_DEVICEA's size, without and with DeviceID and DeviceKey.
const DISPLAY_DEVICE_OLD: u32 = 168;
const DISPLAY_DEVICE: u32 = 424;

/// EnumDisplayDevicesA(device, number, info, flags): the display, the
/// only device, as number 0.
pub fn enum_display_devices(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[device, number, info, _] = args else {
        unreachable!()
    };
    if device != 0 || number != 0 || info == 0 {
        return Some(FALSE);
    }
    let size = cpu.bus.guest_read_32(info);
    if size < DISPLAY_DEVICE_OLD {
        return Some(FALSE);
    }
    // DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, PRIMARY_DEVICE and
    // VGA_COMPATIBLE.
    const FLAGS: u32 = 0x15;
    let mut data = vec![0u8; DISPLAY_DEVICE as usize];
    data[..4].copy_from_slice(&size.to_le_bytes());
    let mut put = |at: usize, s: &str| data[at..at + s.len()].copy_from_slice(s.as_bytes());
    put(4, DEVICE);
    put(36, "Standard PCI Graphics Adapter (VGA)");
    put(
        296,
        "System\\CurrentControlSet\\Services\\Class\\Display\\0000",
    );
    data[164..168].copy_from_slice(&FLAGS.to_le_bytes());
    let len = size.min(DISPLAY_DEVICE) as usize;
    cpu.bus.guest_write_bytes(info, &data[..len]);
    Some(TRUE)
}

/// GetCursorInfo(info): CURSORINFO, whether the cursor shows, its
/// handle and its place.
pub fn get_cursor_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    const CURSORINFO: u32 = 20;
    const CURSOR_SHOWING: u32 = 1;
    let info = args[0];
    if info == 0 || cpu.bus.guest_read_32(info) != CURSORINFO {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    let input = &user(cpu).input;
    let showing = if input.cursor_count >= 0 {
        CURSOR_SHOWING
    } else {
        0
    };
    let words = [
        CURSORINFO,
        showing,
        input.cursor_handle,
        input.cursor.0 as u32,
        input.cursor.1 as u32,
    ];
    write_words(cpu, info, &words);
    Some(TRUE)
}

/// TrackMouseEvent's flags.
const TME_HOVER: u32 = 1;
const TME_LEAVE: u32 = 2;
const TME_QUERY: u32 = 0x4000_0000;
const TME_CANCEL: u32 = 0x8000_0000;
const WM_MOUSELEAVE: u32 = 0x2A3;
/// HOVER_DEFAULT: the system's hover time, 400 ms.
const HOVER_DEFAULT: u32 = 0xFFFF_FFFF;
const HOVER_TIME: u32 = 400;

/// TrackMouseEvent(info): TRACKMOUSEEVENT. TME_LEAVE posts the window
/// WM_MOUSELEAVE when the cursor leaves its client area, at once if it
/// isn't there; TME_QUERY gives what is tracked and TME_CANCEL ends it.
/// TME_HOVER is kept for TME_QUERY, and WM_MOUSEHOVER isn't sent.
pub fn track_mouse_event(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    const TRACKMOUSEEVENT: u32 = 16;
    let info = args[0];
    let words = read_words(cpu, info, 4);
    if words[0] != TRACKMOUSEEVENT {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    let (flags, hwnd, hover) = (words[1], words[2], words[3]);
    let tracking = user(cpu).input.tracking;
    if flags & TME_QUERY != 0 {
        let flags = if tracking[0] == 0 { 0 } else { tracking[1] };
        write_words(cpu, info + 4, &[flags, tracking[0], tracking[2]]);
        return Some(TRUE);
    }
    if !user(cpu).is_window(hwnd) {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(FALSE);
    }
    let input = &mut user(cpu).input;
    if flags & TME_CANCEL != 0 {
        if input.tracking[0] == hwnd {
            input.tracking[1] &= !(flags & (TME_HOVER | TME_LEAVE));
            if input.tracking[1] == 0 {
                input.tracking = [0; 3];
            }
        }
        return Some(TRUE);
    }
    let hover = if hover == HOVER_DEFAULT {
        HOVER_TIME
    } else {
        hover
    };
    if flags & TME_LEAVE != 0 && !over_client(cpu, hwnd) {
        post_leave(cpu, hwnd);
        let rest = flags & TME_HOVER;
        user(cpu).input.tracking = if rest == 0 {
            [0; 3]
        } else {
            [hwnd, rest, hover]
        };
        return Some(TRUE);
    }
    user(cpu).input.tracking = [hwnd, flags & (TME_HOVER | TME_LEAVE), hover];
    Some(TRUE)
}

/// Whether the cursor is in the client area of `hwnd`, in no window
/// above it.
fn over_client(cpu: &mut Cpu, hwnd: u32) -> bool {
    let state = user(cpu);
    let (x, y) = state.input.cursor;
    super::window::window_at(state, x, y) == hwnd && state.screen_client(hwnd).contains(x, y)
}

fn post_leave(cpu: &mut Cpu, hwnd: u32) {
    let Some(tid) = user(cpu).window(hwnd).map(|w| w.tid) else {
        return;
    };
    let time = message::now_ms(cpu);
    let (x, y) = user(cpu).input.cursor;
    let msg = Msg {
        hwnd,
        message: WM_MOUSELEAVE,
        time,
        x,
        y,
        ..Msg::default()
    };
    message::post(cpu, tid, msg);
}

/// The mouse moved: a window that tracks the cursor's leaving and no
/// longer has it gets WM_MOUSELEAVE, and the tracking ends.
pub fn mouse_moved(cpu: &mut Cpu) {
    let [hwnd, flags, _] = user(cpu).input.tracking;
    if hwnd == 0 || flags & TME_LEAVE == 0 {
        return;
    }
    if !user(cpu).is_window(hwnd) {
        user(cpu).input.tracking = [0; 3];
        return;
    }
    if !over_client(cpu, hwnd) {
        post_leave(cpu, hwnd);
        user(cpu).input.tracking = [0; 3];
    }
}
