//! The tooltip control (tooltips_class32): a popup that shows a tool's
//! text when the mouse rests on it. A tool is a rectangle of a window,
//! or a whole window (TTF_IDISHWND); its text is a string, a string
//! resource, or asked of the tool's window with TTN_GETDISPINFOA
//! (LPSTR_TEXTCALLBACK). The control learns of the mouse through
//! TTM_RELAYEVENT, or by subclassing the tool's window (TTF_SUBCLASS),
//! shows itself after the initial delay on a timer, and hides after the
//! autopop delay, or when the mouse leaves the tool or is pressed.
//!
//! The toolbar keeps the tools of its own tooltip through the functions
//! at the end (`add_tool`, `del_tool`, `set_tool_rect`, `relay`).
//!
//! Ported from Wine's tooltips.c:
//!
//! Copyright 1998, 1999 Eric Kohl
//! Copyright 2004 Robert Shearman
//!
//! This library is free software; you can redistribute it and/or modify
//! it under the terms of the GNU Lesser General Public License as
//! published by the Free Software Foundation; either version 2.1 of the
//! License, or (at your option) any later version.

use super::gfx;
use super::script::{self, Arg, Ctx, Op, Script};
use super::{TOOLTIPS, invalidate, state};
use crate::cpu::Cpu;
use crate::win32::convert::CodePage;
use crate::win32::gdi32::Rect;
use crate::win32::user32::message::{
    WM_CREATE, WM_DESTROY, WM_ERASEBKGND, WM_GETFONT, WM_GETTEXT, WM_GETTEXTLENGTH, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_NCCREATE, WM_NCDESTROY,
    WM_NCHITTEST, WM_NOTIFY, WM_NOTIFYFORMAT, WM_PAINT, WM_PRINTCLIENT, WM_RBUTTONDOWN,
    WM_RBUTTONUP, WM_SETFONT, WM_TIMER,
};
use crate::win32::user32::window::{
    HWND_TOPMOST, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW, WS_BORDER,
    WS_CHILD, WS_CLIPSIBLINGS, WS_DLGFRAME, WS_EX_TOOLWINDOW, WS_POPUP,
};
use crate::win32::user32::{self, defwnd, read_words, write_words};
use std::collections::BTreeMap;

const TTM_ACTIVATE: u32 = 0x0401;
const TTM_SETDELAYTIME: u32 = 0x0403;
const TTM_ADDTOOLA: u32 = 0x0404;
const TTM_DELTOOLA: u32 = 0x0405;
const TTM_NEWTOOLRECTA: u32 = 0x0406;
const TTM_RELAYEVENT: u32 = 0x0407;
const TTM_GETTOOLINFOA: u32 = 0x0408;
const TTM_SETTOOLINFOA: u32 = 0x0409;
const TTM_HITTESTA: u32 = 0x040A;
const TTM_GETTEXTA: u32 = 0x040B;
const TTM_UPDATETIPTEXTA: u32 = 0x040C;
const TTM_GETTOOLCOUNT: u32 = 0x040D;
const TTM_ENUMTOOLSA: u32 = 0x040E;
const TTM_GETCURRENTTOOLA: u32 = 0x040F;
const TTM_WINDOWFROMPOINT: u32 = 0x0410;
const TTM_TRACKACTIVATE: u32 = 0x0411;
const TTM_TRACKPOSITION: u32 = 0x0412;
const TTM_SETTIPBKCOLOR: u32 = 0x0413;
const TTM_SETTIPTEXTCOLOR: u32 = 0x0414;
const TTM_GETDELAYTIME: u32 = 0x0415;
const TTM_GETTIPBKCOLOR: u32 = 0x0416;
const TTM_GETTIPTEXTCOLOR: u32 = 0x0417;
const TTM_SETMAXTIPWIDTH: u32 = 0x0418;
const TTM_GETMAXTIPWIDTH: u32 = 0x0419;
const TTM_SETMARGIN: u32 = 0x041A;
const TTM_GETMARGIN: u32 = 0x041B;
const TTM_POP: u32 = 0x041C;
const TTM_UPDATE: u32 = 0x041D;
const TTM_GETBUBBLESIZE: u32 = 0x041E;
const TTM_ADDTOOLW: u32 = 0x0432;
const TTM_DELTOOLW: u32 = 0x0433;
const TTM_NEWTOOLRECTW: u32 = 0x0434;
const TTM_GETTOOLINFOW: u32 = 0x0435;
const TTM_SETTOOLINFOW: u32 = 0x0436;
const TTM_HITTESTW: u32 = 0x0437;
const TTM_UPDATETIPTEXTW: u32 = 0x0439;
const TTM_ENUMTOOLSW: u32 = 0x043A;
const TTM_GETCURRENTTOOLW: u32 = 0x043B;

const TTS_ALWAYSTIP: u32 = 0x01;
const TTS_NOPREFIX: u32 = 0x02;

pub const TTF_IDISHWND: u32 = 0x0001;
const TTF_CENTERTIP: u32 = 0x0002;
const TTF_SUBCLASS: u32 = 0x0010;
const TTF_ABSOLUTE: u32 = 0x0080;
const TTF_TRANSPARENT: u32 = 0x0100;
const TTF_DI_SETITEM: u32 = 0x8000;

const TTDT_AUTOMATIC: u32 = 0;
const TTDT_RESHOW: u32 = 1;
const TTDT_AUTOPOP: u32 = 2;
const TTDT_INITIAL: u32 = 3;

pub const TTN_GETDISPINFOA: u32 = (-520i32) as u32;
pub const TTN_GETDISPINFOW: u32 = (-530i32) as u32;
const TTN_SHOW: u32 = (-521i32) as u32;
const TTN_POP: u32 = (-522i32) as u32;
const NM_CUSTOMDRAW: u32 = (-12i32) as u32;

const CDDS_PREPAINT: u32 = 0x0001;
const CDDS_POSTPAINT: u32 = 0x0002;
const CDRF_NOTIFYPOSTPAINT: u32 = 0x0010;

const NF_QUERY: u32 = 3;
const NFR_UNICODE: u32 = 2;
const HTTRANSPARENT: u32 = (-1i32) as u32;
const GW_OWNER: u32 = 4;
const SM_CXSCREEN: u32 = 0;
const SM_CYSCREEN: u32 = 1;

const DT_WORDBREAK: u32 = 0x0010;
const DT_EXTERNALLEADING: u32 = 0x0200;
const DT_CALCRECT: u32 = 0x0400;
const DT_NOPREFIX: u32 = 0x0800;
const TRANSPARENT: u32 = 1;

const ID_TIMERSHOW: u32 = 1;
const ID_TIMERPOP: u32 = 2;
const ID_TIMERLEAVE: u32 = 3;

const NORMAL_TEXT_MARGIN: i32 = 2;
/// The longest text a tip shows (INFOTIPSIZE).
pub const INFOTIPSIZE: u32 = 1024;
/// The sizes of TTTOOLINFOA without lParam (TTTOOLINFOA_V1_SIZE), with
/// it, and with lpReserved.
const V1_SIZE: u32 = 40;
const V2_SIZE: u32 = 44;
const V3_SIZE: u32 = 48;

/// What a tool's text is.
const TEXT_NONE: u32 = 0;
const TEXT_STRING: u32 = 1;
const TEXT_CALLBACK: u32 = 2;
const TEXT_RESOURCE: u32 = 3;

/// The scratch space of the control's own scripts: an NMTTDISPINFOA,
/// an NMHDR, an NMTTCUSTOMDRAW, a PAINTSTRUCT.
const S_DISP: u32 = 0;
const S_HDR: u32 = 112;
const S_CD: u32 = 128;
const S_PS: u32 = 184;
const S_SIZE: u32 = 248;

// The continuations.
const T_FORMAT: u16 = 1;
const T_DISP_ASK: u16 = 2;
const T_DISP_GOT: u16 = 3;
const T_SHOW: u16 = 4;
const T_SHOWN: u16 = 5;
const T_COPY_TEXT: u16 = 6;
const T_PAINT: u16 = 7;
const T_PAINT_DRAW: u16 = 8;

/// A tool.
#[derive(Clone, Debug, Default)]
pub struct Tool {
    pub flags: u32,
    pub hwnd: u32,
    pub id: u32,
    pub rect: Rect,
    pub inst: u32,
    /// TEXT_, and the text or the resource's id.
    pub kind: u32,
    pub text: Vec<u8>,
    pub res: u32,
    pub lparam: u32,
    pub unicode: bool,
}

crate::state_fields!(Tool {
    flags,
    hwnd,
    id,
    rect,
    inst,
    kind,
    text,
    res,
    lparam,
    unicode
});

/// What a tooltip control keeps.
#[derive(Clone, Debug, Default)]
pub struct Tooltip {
    /// The text shown, and the text fetched last for a tool.
    pub tip: Vec<u8>,
    pub fetched: Vec<u8>,
    pub active: bool,
    pub track_active: bool,
    pub bk: u32,
    pub fg: u32,
    pub font: u32,
    pub track_x: i32,
    pub track_y: i32,
    pub max_width: i32,
    /// The tool the mouse is on, the tool shown, the tool tracked; -1
    /// for none.
    pub tool: i32,
    pub current: i32,
    pub track: i32,
    pub reshow: i32,
    pub autopop: i32,
    pub initial: i32,
    pub margin: Rect,
    pub tools: Vec<Tool>,
}

crate::state_fields!(Tooltip {
    tip,
    fetched,
    active,
    track_active,
    bk,
    fg,
    font,
    track_x,
    track_y,
    max_width,
    tool,
    current,
    track,
    reshow,
    autopop,
    initial,
    margin,
    tools
});

/// A window a tooltip subclassed (TTF_SUBCLASS): the tooltip, and the
/// window's own procedure.
#[derive(Clone, Debug, Default)]
pub struct Subclass {
    pub tooltip: u32,
    pub proc: u32,
}

crate::state_fields!(Subclass { tooltip, proc });

/// What the module keeps, in COMCTL32's `State`.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub tips: BTreeMap<u32, Tooltip>,
    pub subclassed: BTreeMap<u32, Subclass>,
}

crate::state_fields!(State { tips, subclassed });

fn get(cpu: &mut Cpu, hwnd: u32) -> Option<Tooltip> {
    state(cpu).tooltips.tips.get(&hwnd).cloned()
}

fn put(cpu: &mut Cpu, hwnd: u32, t: Tooltip) {
    state(cpu).tooltips.tips.insert(hwnd, t);
}

/// Whether `hwnd` is a tooltip control.
pub fn exists(cpu: &mut Cpu, hwnd: u32) -> bool {
    state(cpu).tooltips.tips.contains_key(&hwnd)
}

fn new_script(hwnd: u32) -> Script {
    let mut s = Script::new(hwnd);
    s.alloc(S_SIZE);
    s
}

fn send_op(to: u32, msg: u32, wp: Arg, lp: Arg) -> Op {
    Op::Call {
        target: user32::address_of("SendMessageA"),
        args: vec![Arg::V(to), Arg::V(msg), wp, lp],
    }
}

fn user_op(name: &str, args: &[u32]) -> Op {
    Op::Call {
        target: user32::address_of(name),
        args: args.iter().map(|&a| Arg::V(a)).collect(),
    }
}

/// The program's string at `at`, at most `max` bytes of it: empty if
/// the memory faulted (the caller checks).
fn read_string(cpu: &mut Cpu, at: u32, max: usize) -> Vec<u8> {
    if at == 0 {
        return Vec::new();
    }
    cpu.bus.guest_read_asciiz(at, max)
}

/// The program's Unicode string at `at`, in ANSI.
pub fn read_wide(cpu: &mut Cpu, at: u32, max: u32) -> Vec<u8> {
    let mut units = Vec::new();
    if at == 0 {
        return Vec::new();
    }
    for i in 0..max {
        let c = cpu.bus.guest_read_16(at + 2 * i);
        if cpu.bus.guest_faulted() || c == 0 {
            break;
        }
        units.push(c);
    }
    wide_to_ansi(&units)
}

/// Unicode characters in ANSI, '?' for those it lacks.
pub fn wide_to_ansi(units: &[u16]) -> Vec<u8> {
    units
        .iter()
        .map(|&c| CodePage::Ansi.byte(c).unwrap_or(b'?'))
        .collect()
}

/// The string resource `id` of `inst`, in ANSI.
pub fn load_string(cpu: &mut Cpu, inst: u32, id: u32) -> Vec<u8> {
    match user32::resource::string_units(cpu, inst, id & 0xFFFF) {
        Some(Some(units)) => {
            let mut b = wide_to_ansi(&units);
            b.truncate(INFOTIPSIZE as usize - 1);
            b
        }
        _ => Vec::new(),
    }
}

/// TOOLTIPS_GetToolFromInfoT: the tool `hwnd` and `id` name, or -1.
fn tool_from_info(t: &Tooltip, hwnd: u32, id: u32) -> i32 {
    if let Some(i) = t
        .tools
        .iter()
        .position(|tl| tl.flags & TTF_IDISHWND == 0 && tl.hwnd == hwnd && tl.id == id)
    {
        return i as i32;
    }
    t.tools
        .iter()
        .position(|tl| tl.flags & TTF_IDISHWND != 0 && tl.id == id)
        .map_or(-1, |i| i as i32)
}

/// TOOLTIPS_GetToolFromPoint: the tool of `hwnd` at the point, or -1.
fn tool_from_point(t: &Tooltip, hwnd: u32, x: i32, y: i32) -> i32 {
    if let Some(i) = t
        .tools
        .iter()
        .position(|tl| tl.flags & TTF_IDISHWND == 0 && tl.hwnd == hwnd && tl.rect.contains(x, y))
    {
        return i as i32;
    }
    t.tools
        .iter()
        .position(|tl| tl.flags & TTF_IDISHWND != 0 && tl.id == hwnd)
        .map_or(-1, |i| i as i32)
}

/// The text a TTTOOLINFO's lpszText gives: kind, text, resource id.
fn text_of(cpu: &mut Cpu, ptr: u32, wide: bool) -> (u32, Vec<u8>, u32) {
    if ptr == 0 {
        return (TEXT_NONE, Vec::new(), 0);
    }
    if ptr >> 16 == 0 {
        return (TEXT_RESOURCE, Vec::new(), ptr);
    }
    if ptr == u32::MAX {
        return (TEXT_CALLBACK, Vec::new(), 0);
    }
    let text = if wide {
        read_wide(cpu, ptr, 0x1_0000)
    } else {
        read_string(cpu, ptr, 0x1_0000)
    };
    (TEXT_STRING, text, 0)
}

/// TTM_SETDELAYTIME's change of the delay `kind` to `time`, the
/// default for a negative one. Windows takes the defaults from
/// GetDoubleClickTime, 500 ms.
fn set_delay(t: &mut Tooltip, kind: u32, time: i32) {
    let dbl = 500;
    match kind {
        TTDT_AUTOMATIC => {
            let time = if time < 0 { dbl } else { time };
            t.reshow = time / 5;
            t.autopop = time * 10;
            t.initial = time;
        }
        TTDT_RESHOW => t.reshow = if time < 0 { dbl / 5 } else { time },
        TTDT_AUTOPOP => t.autopop = if time < 0 { dbl * 10 } else { time },
        TTDT_INITIAL => t.initial = if time < 0 { dbl } else { time },
        _ => {}
    }
}

/// The steps of TOOLTIPS_GetTipText for the tool `n`: the text ends in
/// the control's `fetched`, then the continuation `next` runs with
/// `data`.
fn fetch_text(cpu: &mut Cpu, hwnd: u32, n: i32, next: u16, data: &[u32]) -> Vec<Op> {
    let Some(mut t) = get(cpu, hwnd) else {
        return Vec::new();
    };
    let Some(tool) = t.tools.get(n as usize).cloned() else {
        return Vec::new();
    };
    match tool.kind {
        TEXT_CALLBACK => {
            let mut then_data = vec![hwnd, n as u32, next as u32];
            then_data.extend_from_slice(data);
            return vec![Op::Then {
                id: T_DISP_ASK,
                data: then_data,
            }];
        }
        TEXT_RESOURCE if tool.inst != 0 => t.fetched = load_string(cpu, tool.inst, tool.res),
        TEXT_STRING => {
            t.fetched = tool.text.clone();
            t.fetched.truncate(INFOTIPSIZE as usize - 1);
        }
        _ => t.fetched.clear(),
    }
    put(cpu, hwnd, t);
    vec![Op::Then {
        id: next,
        data: data.to_vec(),
    }]
}

/// TOOLTIPS_CalcTipSize: the size of the tip for the text shown.
fn tip_size(cpu: &mut Cpu, hwnd: u32, t: &Tooltip) -> (i32, i32) {
    let style = super::style(cpu, hwnd);
    let mut flags = DT_EXTERNALLEADING | DT_CALCRECT;
    let mut r = Rect::default();
    if t.max_width > -1 {
        r.right = t.max_width;
        flags |= DT_WORDBREAK;
    }
    if style & TTS_NOPREFIX != 0 {
        flags |= DT_NOPREFIX;
    }
    let hdc = user32::paint::get_dc(cpu, &[hwnd]).unwrap_or(0);
    if hdc != 0 {
        let old = gfx::select(cpu, hdc, t.font);
        gfx::draw_text(cpu, hdc, &t.tip, &mut r, flags);
        gfx::select(cpu, hdc, old);
        user32::paint::release_dc(cpu, &[hwnd, hdc]);
    }
    (
        r.width() + 2 * NORMAL_TEXT_MARGIN + t.margin.left + t.margin.right,
        r.height() + 2 * NORMAL_TEXT_MARGIN + t.margin.top + t.margin.bottom,
    )
}

/// The steps that tell the tool's window `code` (TTN_SHOW, TTN_POP),
/// with an NMHDR at the offset `at`.
fn tool_notify(hwnd: u32, tool: &Tool, code: u32, at: u32) -> Vec<Op> {
    vec![
        Op::Write {
            at,
            words: vec![hwnd, tool.id, code],
        },
        send_op(tool.hwnd, WM_NOTIFY, Arg::V(tool.id), Arg::S(at)),
    ]
}

/// The steps of TOOLTIPS_Hide, with an NMHDR at the offset `at`.
fn hide_ops(cpu: &mut Cpu, hwnd: u32, at: u32) -> Vec<Op> {
    let Some(mut t) = get(cpu, hwnd) else {
        return Vec::new();
    };
    if t.current == -1 {
        return Vec::new();
    }
    let Some(tool) = t.tools.get(t.current as usize).cloned() else {
        return Vec::new();
    };
    user32::message::kill_timer(cpu, &[hwnd, ID_TIMERPOP]);
    t.current = -1;
    put(cpu, hwnd, t);
    let mut ops = tool_notify(hwnd, &tool, TTN_POP, at);
    let flags = SWP_NOZORDER | SWP_HIDEWINDOW | SWP_NOACTIVATE;
    ops.push(user_op("SetWindowPos", &[hwnd, 0, 0, 0, 0, 0, flags]));
    ops
}

/// The steps of TOOLTIPS_TrackHide.
fn track_hide_ops(cpu: &mut Cpu, hwnd: u32) -> Vec<Op> {
    let Some(t) = get(cpu, hwnd) else {
        return Vec::new();
    };
    if t.track == -1 {
        return Vec::new();
    }
    let Some(tool) = t.tools.get(t.track as usize).cloned() else {
        return Vec::new();
    };
    let mut ops = tool_notify(hwnd, &tool, TTN_POP, S_HDR);
    let flags = SWP_NOZORDER | SWP_HIDEWINDOW | SWP_NOACTIVATE;
    ops.push(user_op("SetWindowPos", &[hwnd, 0, 0, 0, 0, 0, flags]));
    ops
}

/// The steps of TOOLTIPS_Show and TOOLTIPS_TrackShow: fetch the text,
/// then show it (`T_SHOW`).
fn show_ops(cpu: &mut Cpu, hwnd: u32, track: bool) -> Vec<Op> {
    let Some(t) = get(cpu, hwnd) else {
        return Vec::new();
    };
    let n = if track { t.track } else { t.tool };
    if n == -1 {
        return Vec::new();
    }
    fetch_text(cpu, hwnd, n, T_SHOW, &[hwnd, n as u32, track as u32])
}

/// A tool's rectangle on the screen.
fn tool_screen_rect(cpu: &mut Cpu, tool: &Tool) -> Rect {
    if tool.flags & TTF_IDISHWND != 0 {
        return user32::user(cpu).screen_rect(tool.id);
    }
    let (x, y) = user32::user(cpu).client_origin(tool.hwnd);
    tool.rect.offset(x, y)
}

/// TOOLTIPS_Show once the text is there: place and show the tip.
fn show_fetched(cpu: &mut Cpu, hwnd: u32, n: i32, track: bool) -> Vec<Op> {
    let Some(mut t) = get(cpu, hwnd) else {
        return Vec::new();
    };
    t.tip = t.fetched.clone();
    let Some(tool) = t.tools.get(n as usize).cloned() else {
        put(cpu, hwnd, t);
        return Vec::new();
    };
    if t.tip.is_empty() {
        put(cpu, hwnd, t);
        return Vec::new();
    }
    if !track {
        t.current = n;
    }
    put(cpu, hwnd, t.clone());
    let mut ops = tool_notify(hwnd, &tool, TTN_SHOW, S_HDR);
    let (cx, cy) = tip_size(cpu, hwnd, &t);
    let mut r = Rect::default();
    if track {
        r.left = t.track_x;
        r.top = t.track_y;
        if tool.flags & TTF_CENTERTIP != 0 {
            r.left -= cx / 2;
            r.top -= cy / 2;
        }
        if tool.flags & TTF_ABSOLUTE == 0 {
            let rc = tool_screen_rect(cpu, &tool);
            let over = r.left + cx > rc.left
                && r.left < rc.right
                && r.top + cy > rc.top
                && r.top < rc.bottom;
            if over {
                r.left = rc.right;
            }
        }
    } else if tool.flags & TTF_CENTERTIP != 0 {
        let rc = tool_screen_rect(cpu, &tool);
        r.left = (rc.left + rc.right - cx) / 2;
        r.top = rc.bottom + 2;
    } else {
        let (x, y) = user32::user(cpu).input.cursor;
        r.left = x;
        r.top = y + 20;
    }
    r.right = r.left + cx;
    r.bottom = r.top + cy;
    let screen_w = user32::sysparams::screen_metric(cpu, SM_CXSCREEN);
    let screen_h = user32::sysparams::screen_metric(cpu, SM_CYSCREEN);
    if r.right > screen_w {
        r.left -= r.right - screen_w + 2;
        r.right = screen_w - 2;
    }
    if r.left < 0 {
        r.right -= r.left;
        r.left = 0;
    }
    if r.bottom > screen_h {
        let rc = tool_screen_rect(cpu, &tool);
        r.bottom = rc.top - 2;
        r.top = r.bottom - cy;
    }
    if super::style(cpu, hwnd) & WS_BORDER != 0 {
        r = r.inflate(1, 1);
    }
    let flags = SWP_SHOWWINDOW | SWP_NOACTIVATE;
    ops.push(user_op(
        "SetWindowPos",
        &[
            hwnd,
            HWND_TOPMOST,
            r.left as u32,
            r.top as u32,
            r.width() as u32,
            r.height() as u32,
            flags,
        ],
    ));
    ops.push(Op::Then {
        id: T_SHOWN,
        data: vec![hwnd, track as u32],
    });
    ops
}

/// TOOLTIPS_CheckTool: the tool under the cursor, or -1.
fn check_tool(cpu: &mut Cpu, hwnd: u32, t: &Tooltip, show_test: bool) -> i32 {
    let (x, y) = user32::user(cpu).input.cursor;
    let under = user32::window::window_from_point(cpu, &[x as u32, y as u32]).unwrap_or(0);
    if under == 0 {
        return -1;
    }
    let (ox, oy) = user32::user(cpu).client_origin(under);
    let n = tool_from_point(t, under, x - ox, y - oy);
    if n == -1 {
        return -1;
    }
    if super::style(cpu, hwnd) & TTS_ALWAYSTIP == 0 && show_test {
        let owner = user32::window::get_window(cpu, &[hwnd, GW_OWNER]).unwrap_or(0);
        let active = user32::input::get_active_window(cpu, &[]).unwrap_or(0);
        let child = user32::window::is_child(cpu, &[active, owner]).unwrap_or(0) != 0;
        if active == 0 || (active != owner && !child) {
            return -1;
        }
    }
    n
}

/// TOOLTIPS_RelayEvent of the message `msg` of `from`, its steps added
/// to `s` (with scratch space of its own).
fn relay_into(cpu: &mut Cpu, hwnd: u32, from: u32, msg: u32, lp: u32, s: &mut Script) {
    use user32::message::{kill_timer, set_timer};
    match msg {
        WM_LBUTTONDOWN | WM_LBUTTONUP | WM_MBUTTONDOWN | WM_MBUTTONUP | WM_RBUTTONDOWN
        | WM_RBUTTONUP => {
            let at = s.alloc(12);
            let ops = hide_ops(cpu, hwnd, at);
            s.ops.extend(ops);
        }
        WM_MOUSEMOVE => {
            let Some(mut t) = get(cpu, hwnd) else {
                return;
            };
            let (x, y) = user32::point_of(lp);
            let old = t.tool;
            t.tool = tool_from_point(&t, from, x, y);
            let (tool, active, current) = (t.tool, t.active, t.current);
            let (initial, reshow, autopop) = (t.initial, t.reshow, t.autopop);
            put(cpu, hwnd, t);
            if tool != old {
                if old != -1 {
                    let at = s.alloc(12);
                    let ops = hide_ops(cpu, hwnd, at);
                    s.ops.extend(ops);
                    kill_timer(cpu, &[hwnd, ID_TIMERLEAVE]);
                }
                if tool != -1 && active {
                    let delay = if old == -1 { initial } else { reshow };
                    set_timer(cpu, &[hwnd, ID_TIMERSHOW, delay as u32, 0]);
                }
            } else if current != -1 {
                kill_timer(cpu, &[hwnd, ID_TIMERPOP]);
                set_timer(cpu, &[hwnd, ID_TIMERPOP, autopop as u32, 0]);
            } else if tool != -1 && active {
                set_timer(cpu, &[hwnd, ID_TIMERSHOW, initial as u32, 0]);
            }
        }
        _ => {}
    }
}

/// The DrawText flags the tip is drawn with.
fn draw_flags(cpu: &mut Cpu, hwnd: u32, t: &Tooltip) -> u32 {
    let mut flags = DT_EXTERNALLEADING;
    if t.max_width > -1 {
        flags |= DT_WORDBREAK;
    }
    if super::style(cpu, hwnd) & TTS_NOPREFIX != 0 {
        flags |= DT_NOPREFIX;
    }
    flags
}

/// TOOLTIPS_Refresh: paint the tip with the DrawText flags `flags`.
fn draw(cpu: &mut Cpu, hwnd: u32, hdc: u32, flags: u32) {
    let Some(t) = get(cpu, hwnd) else {
        return;
    };
    let mut r = super::client(cpu, hwnd);
    gfx::fill_rgb(cpu, hdc, r, t.bk);
    let old_mode = gfx::set_bk_mode(cpu, hdc, TRANSPARENT);
    let old_color = gfx::set_text_color(cpu, hdc, t.fg);
    let old_font = gfx::select(cpu, hdc, t.font);
    r.left += NORMAL_TEXT_MARGIN + t.margin.left;
    r.top += NORMAL_TEXT_MARGIN + t.margin.top;
    r.right -= NORMAL_TEXT_MARGIN + t.margin.right;
    r.bottom -= NORMAL_TEXT_MARGIN + t.margin.bottom;
    gfx::draw_text(cpu, hdc, &t.tip, &mut r, flags);
    gfx::select(cpu, hdc, old_font);
    gfx::set_text_color(cpu, hdc, old_color);
    gfx::set_bk_mode(cpu, hdc, old_mode);
}

/// WM_PAINT: BeginPaint, NM_CUSTOMDRAW's prepaint to the parent, the
/// drawing, the postpaint if the parent asks for it, EndPaint.
fn paint(hwnd: u32) -> Script {
    let mut s = new_script(hwnd);
    let begin = user32::address_of("BeginPaint");
    let end = user32::address_of("EndPaint");
    s.call(begin, &[Arg::V(hwnd), Arg::S(S_PS)])
        .then(T_PAINT, &[])
        .call(end, &[Arg::V(hwnd), Arg::S(S_PS)])
        .ret(0);
    s
}

/// The answer to TTN_GETDISPINFOA in the NMTTDISPINFOA at `nm`
/// (TOOLTIPS_GetDispInfoA).
fn got_disp_info(cpu: &mut Cpu, hwnd: u32, n: usize, nm: u32) {
    let w = read_words(cpu, nm, 27);
    let (ptr, inst, flags) = (w[3], w[24], w[25]);
    let set_item = flags & TTF_DI_SETITEM != 0;
    let Some(mut t) = get(cpu, hwnd) else {
        return;
    };
    if ptr == 0 {
        t.fetched.clear();
    } else if ptr >> 16 == 0 {
        t.fetched = load_string(cpu, inst, ptr);
        if set_item && let Some(tl) = t.tools.get_mut(n) {
            tl.kind = TEXT_RESOURCE;
            tl.inst = inst;
            tl.res = ptr;
        }
    } else if ptr != u32::MAX {
        let mut text = read_string(cpu, ptr, INFOTIPSIZE as usize);
        text.truncate(INFOTIPSIZE as usize - 1);
        if cpu.bus.guest_faulted() {
            text.clear();
        }
        if set_item && let Some(tl) = t.tools.get_mut(n) {
            tl.kind = TEXT_STRING;
            tl.inst = 0;
            tl.text = text.clone();
        }
        t.fetched = text;
    } else {
        t.fetched.clear();
    }
    put(cpu, hwnd, t);
}

/// The continuations.
pub fn then(cpu: &mut Cpu, id: u16, c: &mut Ctx) -> Vec<Op> {
    match id {
        T_FORMAT => {
            // After WM_NOTIFYFORMAT to the new tool's window.
            let (hwnd, n) = (c.word(0), c.word(1) as usize);
            if let Some(mut t) = get(cpu, hwnd)
                && let Some(tool) = t.tools.get_mut(n)
            {
                tool.unicode = c.last == NFR_UNICODE;
                put(cpu, hwnd, t);
            }
            Vec::new()
        }
        T_DISP_ASK => {
            // TTN_GETDISPINFOA to the tool's window.
            let (hwnd, n) = (c.word(0), c.word(1) as usize);
            let Some(tool) = get(cpu, hwnd).and_then(|t| t.tools.get(n).cloned()) else {
                return Vec::new();
            };
            let nm = c.scratch + S_DISP;
            let mut words = vec![0u32; 27];
            words[0] = hwnd;
            words[1] = tool.id;
            words[2] = TTN_GETDISPINFOA;
            words[3] = nm + 16;
            words[25] = tool.flags;
            words[26] = tool.lparam;
            vec![
                Op::Write { at: S_DISP, words },
                send_op(tool.hwnd, WM_NOTIFY, Arg::V(tool.id), Arg::S(S_DISP)),
                Op::Then {
                    id: T_DISP_GOT,
                    data: c.data.clone(),
                },
            ]
        }
        T_DISP_GOT => {
            let (hwnd, n) = (c.word(0), c.word(1) as usize);
            got_disp_info(cpu, hwnd, n, c.scratch + S_DISP);
            vec![Op::Then {
                id: c.word(2) as u16,
                data: c.data.get(3..).unwrap_or(&[]).to_vec(),
            }]
        }
        T_SHOW => show_fetched(cpu, c.word(0), c.word(1) as i32, c.word(2) != 0),
        T_SHOWN => {
            // The tip is up: paint it, and time its going.
            let (hwnd, track) = (c.word(0), c.word(1) != 0);
            invalidate(cpu, hwnd, true);
            if !track && let Some(t) = get(cpu, hwnd) {
                user32::message::set_timer(cpu, &[hwnd, ID_TIMERPOP, t.autopop as u32, 0]);
                user32::message::set_timer(cpu, &[hwnd, ID_TIMERLEAVE, t.reshow as u32, 0]);
            }
            vec![user_op("UpdateWindow", &[hwnd])]
        }
        T_COPY_TEXT => {
            // TTM_GETTEXTA: the text into the program's buffer.
            let (hwnd, at) = (c.word(0), c.word(1));
            let text = get(cpu, hwnd).map(|t| t.fetched).unwrap_or_default();
            user32::copy_out(cpu, at, INFOTIPSIZE, &text);
            Vec::new()
        }
        T_PAINT => {
            let (hwnd, hdc) = (c.hwnd, c.last);
            let Some(t) = get(cpu, hwnd) else {
                return Vec::new();
            };
            if hdc == 0 {
                return Vec::new();
            }
            let flags = draw_flags(cpu, hwnd, &t);
            let id = t
                .tools
                .get(t.current.max(0) as usize)
                .filter(|_| t.current != -1)
                .map_or(0, |tl| tl.id);
            let r = super::client(cpu, hwnd);
            let words = vec![
                hwnd,
                id,
                NM_CUSTOMDRAW,
                CDDS_PREPAINT,
                hdc,
                r.left as u32,
                r.top as u32,
                r.right as u32,
                r.bottom as u32,
                0,
                0,
                0,
                flags,
            ];
            let parent = super::parent(cpu, hwnd);
            vec![
                Op::Write { at: S_CD, words },
                send_op(parent, WM_NOTIFY, Arg::V(id), Arg::S(S_CD)),
                Op::Then {
                    id: T_PAINT_DRAW,
                    data: vec![hdc, parent, id],
                },
            ]
        }
        T_PAINT_DRAW => {
            let (hdc, parent, id) = (c.word(0), c.word(1), c.word(2));
            let flags = cpu.bus.guest_read_32(c.scratch + S_CD + 48);
            draw(cpu, c.hwnd, hdc, flags);
            if c.last & CDRF_NOTIFYPOSTPAINT != 0 {
                return vec![
                    Op::Write {
                        at: S_CD + 12,
                        words: vec![CDDS_POSTPAINT],
                    },
                    send_op(parent, WM_NOTIFY, Arg::V(id), Arg::S(S_CD)),
                ];
            }
            Vec::new()
        }
        _ => Vec::new(),
    }
}

/// The tool a TTTOOLINFO at `ti` names: None when the program's memory
/// faulted.
fn named_tool(cpu: &mut Cpu, t: &Tooltip, ti: u32) -> Option<i32> {
    let w = read_words(cpu, ti, 4);
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(tool_from_info(t, w[2], w[3]))
}

/// The window a tool with these flags and ids subclasses.
fn subclass_target(flags: u32, hwnd: u32, id: u32) -> u32 {
    if flags & TTF_IDISHWND != 0 { id } else { hwnd }
}

/// TTM_ADDTOOLA and TTM_ADDTOOLW. COMCTL32 4.70 takes the structure
/// whatever its cbSize says; lParam is there from TTTOOLINFOA_V2_SIZE.
fn add_tool_msg(cpu: &mut Cpu, hwnd: u32, mut t: Tooltip, ti: u32, wide: bool) -> Option<u32> {
    if ti == 0 {
        return Some(0);
    }
    let w = read_words(cpu, ti, 11);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let (kind, text, res) = text_of(cpu, w[9], wide);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let tool = Tool {
        flags: w[1],
        hwnd: w[2],
        id: w[3],
        rect: Rect::new(w[4] as i32, w[5] as i32, w[6] as i32, w[7] as i32),
        inst: w[8],
        kind,
        text,
        res,
        lparam: if w[0] >= V2_SIZE { w[10] } else { 0 },
        unicode: false,
    };
    if tool.flags & TTF_SUBCLASS != 0 {
        subclass(cpu, subclass_target(tool.flags, tool.hwnd, tool.id), hwnd);
    }
    let n = t.tools.len() as u32;
    let to = tool.hwnd;
    t.tools.push(tool);
    put(cpu, hwnd, t);
    if to == 0 {
        return Some(1);
    }
    let mut s = new_script(hwnd);
    s.send(to, WM_NOTIFYFORMAT, Arg::V(hwnd), Arg::V(NF_QUERY))
        .then(T_FORMAT, &[hwnd, n])
        .ret(1);
    script::run(cpu, TOOLTIPS, s)
}

/// Subclass `target` for the tooltip `tip`.
fn subclass(cpu: &mut Cpu, target: u32, tip: u32) {
    let ours = super::address_of("TooltipsSubclassProc");
    let Some(w) = user32::user(cpu).window_mut(target) else {
        return;
    };
    if w.proc == ours {
        if let Some(sc) = state(cpu).tooltips.subclassed.get_mut(&target) {
            sc.tooltip = tip;
        }
        return;
    }
    let proc = std::mem::replace(&mut w.proc, ours);
    let sc = Subclass { tooltip: tip, proc };
    state(cpu).tooltips.subclassed.insert(target, sc);
}

/// Give `target` its own procedure back, if `tip` subclassed it.
fn unsubclass(cpu: &mut Cpu, target: u32, tip: u32) {
    let subclassed = &mut state(cpu).tooltips.subclassed;
    if subclassed.get(&target).is_none_or(|sc| sc.tooltip != tip) {
        return;
    }
    let Some(sc) = subclassed.remove(&target) else {
        return;
    };
    let ours = super::address_of("TooltipsSubclassProc");
    if let Some(w) = user32::user(cpu).window_mut(target)
        && w.proc == ours
    {
        w.proc = sc.proc;
    }
}

/// The procedure a tool's window is subclassed with (TTF_SUBCLASS): the
/// mouse's messages are relayed to the tooltip on their way to the
/// window's own procedure.
pub fn subclass_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, msg, wp, lp] = args else {
        unreachable!()
    };
    let Some(sc) = state(cpu).tooltips.subclassed.get(&hwnd).cloned() else {
        return defwnd::def_window_proc(cpu, args);
    };
    let mut s = new_script(sc.tooltip);
    if exists(cpu, sc.tooltip) {
        relay_into(cpu, sc.tooltip, hwnd, msg, lp, &mut s);
    }
    if msg == WM_NCDESTROY {
        state(cpu).tooltips.subclassed.remove(&hwnd);
        if let Some(w) = user32::user(cpu).window_mut(hwnd) {
            w.proc = sc.proc;
        }
    }
    let f = user32::address_of("CallWindowProcA");
    let args = [sc.proc, hwnd, msg, wp, lp].map(Arg::V);
    s.call(f, &args).ret_last();
    script::run(cpu, TOOLTIPS, s)
}

/// Copy a tool into the TTTOOLINFO at `ti` of `size` bytes, as
/// TTM_GETTOOLINFO (without `ids`), TTM_ENUMTOOLS and TTM_HITTEST give
/// it. lpszText is given back as NULL.
fn write_tool(cpu: &mut Cpu, ti: u32, size: u32, tool: &Tool, ids: bool) {
    if ids {
        write_words(cpu, ti + 4, &[tool.flags, tool.hwnd, tool.id]);
    } else {
        write_words(cpu, ti + 4, &[tool.flags]);
    }
    let r = tool.rect;
    let words = [r.left, r.top, r.right, r.bottom].map(|v| v as u32);
    write_words(cpu, ti + 16, &words);
    write_words(cpu, ti + 32, &[tool.inst, 0]);
    if size >= V2_SIZE {
        write_words(cpu, ti + 40, &[tool.lparam]);
    }
}

/// Fix the tool indices after the tool `n` is removed.
fn forget_index(t: &mut Tooltip, n: i32) {
    for f in [&mut t.tool, &mut t.track, &mut t.current] {
        if *f == n {
            *f = -1;
        } else if *f > n {
            *f -= 1;
        }
    }
}

/// The tooltip control's window procedure.
pub fn proc_(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, msg, wp, lp] = args else {
        unreachable!()
    };
    if msg == WM_NCCREATE {
        if let Some(w) = user32::user(cpu).window_mut(hwnd) {
            w.style &= !(WS_CHILD | WS_BORDER | WS_DLGFRAME);
            w.style |= WS_POPUP | WS_BORDER | WS_CLIPSIBLINGS;
            w.ex_style |= WS_EX_TOOLWINDOW;
        }
        return defwnd::def_window_proc(cpu, args);
    }
    if msg == WM_CREATE {
        let mut t = Tooltip {
            active: true,
            max_width: -1,
            tool: -1,
            current: -1,
            track: -1,
            bk: gfx::sys_color(cpu, gfx::COLOR_INFOBK),
            fg: gfx::sys_color(cpu, gfx::COLOR_INFOTEXT),
            font: gfx::default_font(cpu),
            ..Tooltip::default()
        };
        set_delay(&mut t, TTDT_AUTOMATIC, -1);
        put(cpu, hwnd, t);
        let flags = SWP_NOZORDER | SWP_HIDEWINDOW | SWP_NOACTIVATE;
        let mut s = new_script(hwnd);
        s.user("SetWindowPos", &[hwnd, 0, 0, 0, 0, 0, flags]).ret(0);
        return script::run(cpu, TOOLTIPS, s);
    }
    let Some(mut t) = get(cpu, hwnd) else {
        return defwnd::def_window_proc(cpu, args);
    };
    let mut s = new_script(hwnd);
    let result = match msg {
        TTM_ACTIVATE => {
            t.active = wp != 0;
            let hide = !t.active && t.current != -1;
            put(cpu, hwnd, t);
            if hide {
                let ops = hide_ops(cpu, hwnd, S_HDR);
                s.ops.extend(ops);
            }
            0
        }
        TTM_ADDTOOLA | TTM_ADDTOOLW => {
            return add_tool_msg(cpu, hwnd, t, lp, msg == TTM_ADDTOOLW);
        }
        TTM_DELTOOLA | TTM_DELTOOLW => {
            if lp == 0 {
                return Some(0);
            }
            let size = cpu.bus.guest_read_32(lp);
            let n = named_tool(cpu, &t, lp)?;
            // TTM_DELTOOLW takes no size it doesn't know.
            if msg == TTM_DELTOOLW && size > V2_SIZE && size != V3_SIZE {
                return Some(0);
            }
            if n == -1 {
                return Some(0);
            }
            let ops = hide_ops(cpu, hwnd, S_HDR);
            s.ops.extend(ops);
            let Some(mut t) = get(cpu, hwnd) else {
                return Some(0);
            };
            let tool = t.tools.remove(n as usize);
            if tool.flags & TTF_SUBCLASS != 0 {
                unsubclass(cpu, subclass_target(tool.flags, tool.hwnd, tool.id), hwnd);
            }
            forget_index(&mut t, n);
            put(cpu, hwnd, t);
            0
        }
        TTM_ENUMTOOLSA | TTM_ENUMTOOLSW => {
            if lp == 0 {
                return Some(0);
            }
            let size = cpu.bus.guest_read_32(lp);
            if cpu.bus.guest_faulted() {
                return None;
            }
            if size < V1_SIZE {
                return Some(0);
            }
            let Some(tool) = t.tools.get(wp as usize).cloned() else {
                return Some(0);
            };
            write_tool(cpu, lp, size, &tool, true);
            if cpu.bus.guest_faulted() {
                return None;
            }
            1
        }
        TTM_GETBUBBLESIZE => {
            if lp == 0 {
                return Some(0);
            }
            let size = cpu.bus.guest_read_32(lp);
            let n = named_tool(cpu, &t, lp)?;
            if size < V1_SIZE || n == -1 {
                return Some(0);
            }
            let (cx, cy) = tip_size(cpu, hwnd, &t);
            (cx as u32 & 0xFFFF) | (cy as u32) << 16
        }
        TTM_GETCURRENTTOOLA | TTM_GETCURRENTTOOLW => {
            if lp == 0 {
                return Some((t.current != -1) as u32);
            }
            let size = cpu.bus.guest_read_32(lp);
            if cpu.bus.guest_faulted() {
                return None;
            }
            if size < V1_SIZE || t.current == -1 {
                return Some(0);
            }
            let Some(tool) = t.tools.get(t.current as usize).cloned() else {
                return Some(0);
            };
            write_tool(cpu, lp, size, &tool, true);
            if cpu.bus.guest_faulted() {
                return None;
            }
            1
        }
        TTM_GETDELAYTIME => match wp {
            TTDT_RESHOW => t.reshow as u32,
            TTDT_AUTOPOP => t.autopop as u32,
            TTDT_INITIAL | TTDT_AUTOMATIC => t.initial as u32,
            _ => u32::MAX,
        },
        TTM_GETMARGIN => {
            if lp != 0 {
                t.margin.write(cpu, lp);
                if cpu.bus.guest_faulted() {
                    return None;
                }
            }
            0
        }
        TTM_GETMAXTIPWIDTH => t.max_width as u32,
        TTM_GETTEXTA => {
            if lp == 0 {
                return Some(0);
            }
            let size = cpu.bus.guest_read_32(lp);
            let n = named_tool(cpu, &t, lp)?;
            let dest = cpu.bus.guest_read_32(lp + 36);
            if cpu.bus.guest_faulted() {
                return None;
            }
            if size < V1_SIZE || n == -1 || dest == 0 {
                return Some(0);
            }
            let ops = fetch_text(cpu, hwnd, n, T_COPY_TEXT, &[hwnd, dest]);
            s.ops.extend(ops);
            0
        }
        TTM_GETTIPBKCOLOR => t.bk,
        TTM_GETTIPTEXTCOLOR => t.fg,
        TTM_GETTOOLCOUNT => t.tools.len() as u32,
        TTM_GETTOOLINFOA | TTM_GETTOOLINFOW => {
            if lp == 0 {
                return Some(0);
            }
            let size = cpu.bus.guest_read_32(lp);
            let n = named_tool(cpu, &t, lp)?;
            if size < V1_SIZE || n == -1 {
                return Some(0);
            }
            let tool = t.tools[n as usize].clone();
            write_tool(cpu, lp, size, &tool, false);
            if cpu.bus.guest_faulted() {
                return None;
            }
            1
        }
        TTM_HITTESTA | TTM_HITTESTW => {
            if lp == 0 {
                return Some(0);
            }
            let w = read_words(cpu, lp, 4);
            if cpu.bus.guest_faulted() {
                return None;
            }
            let n = tool_from_point(&t, w[0], w[1] as i32, w[2] as i32);
            if n == -1 {
                return Some(0);
            }
            if w[3] >= V1_SIZE {
                let tool = t.tools[n as usize].clone();
                write_tool(cpu, lp + 12, w[3], &tool, true);
                if cpu.bus.guest_faulted() {
                    return None;
                }
            }
            1
        }
        TTM_NEWTOOLRECTA | TTM_NEWTOOLRECTW => {
            if lp == 0 {
                return Some(0);
            }
            let w = read_words(cpu, lp, 8);
            if cpu.bus.guest_faulted() {
                return None;
            }
            let n = tool_from_info(&t, w[2], w[3]);
            if w[0] >= V1_SIZE && n != -1 {
                let r = Rect::new(w[4] as i32, w[5] as i32, w[6] as i32, w[7] as i32);
                t.tools[n as usize].rect = r;
                put(cpu, hwnd, t);
            }
            0
        }
        TTM_POP => {
            let ops = hide_ops(cpu, hwnd, S_HDR);
            s.ops.extend(ops);
            0
        }
        TTM_RELAYEVENT => {
            if lp == 0 {
                return Some(0);
            }
            let m = read_words(cpu, lp, 4);
            if cpu.bus.guest_faulted() {
                return None;
            }
            relay_into(cpu, hwnd, m[0], m[1], m[3], &mut s);
            0
        }
        TTM_SETDELAYTIME => {
            set_delay(&mut t, wp, lp as u16 as i16 as i32);
            put(cpu, hwnd, t);
            0
        }
        TTM_SETMARGIN => {
            if lp != 0 {
                t.margin = Rect::read(cpu, lp);
                if cpu.bus.guest_faulted() {
                    return None;
                }
                put(cpu, hwnd, t);
            }
            0
        }
        TTM_SETMAXTIPWIDTH => {
            let old = t.max_width;
            t.max_width = lp as i32;
            put(cpu, hwnd, t);
            old as u32
        }
        TTM_SETTIPBKCOLOR => {
            t.bk = wp;
            put(cpu, hwnd, t);
            0
        }
        TTM_SETTIPTEXTCOLOR => {
            t.fg = wp;
            put(cpu, hwnd, t);
            0
        }
        TTM_SETTOOLINFOA | TTM_SETTOOLINFOW | TTM_UPDATETIPTEXTA | TTM_UPDATETIPTEXTW => {
            if lp == 0 {
                return Some(0);
            }
            let wide = msg == TTM_SETTOOLINFOW || msg == TTM_UPDATETIPTEXTW;
            let w = read_words(cpu, lp, 11);
            if cpu.bus.guest_faulted() {
                return None;
            }
            let n = tool_from_info(&t, w[2], w[3]);
            if w[0] < V1_SIZE || n == -1 {
                return Some(0);
            }
            let (kind, text, res) = text_of(cpu, w[9], wide);
            if cpu.bus.guest_faulted() {
                return None;
            }
            let update = msg == TTM_UPDATETIPTEXTA || msg == TTM_UPDATETIPTEXTW;
            let tool = &mut t.tools[n as usize];
            if !update {
                tool.flags = w[1];
                tool.hwnd = w[2];
                tool.id = w[3];
                tool.rect = Rect::new(w[4] as i32, w[5] as i32, w[6] as i32, w[7] as i32);
                if w[0] >= V2_SIZE {
                    tool.lparam = w[10];
                }
            }
            tool.inst = w[8];
            tool.kind = kind;
            tool.text = text;
            tool.res = res;
            let (current, active, track_active) = (t.current, t.active, t.track_active);
            put(cpu, hwnd, t);
            if current == n {
                // The tip shown changes with it.
                let track = update && !active && track_active;
                let ops = show_ops(cpu, hwnd, track);
                s.ops.extend(ops);
            }
            0
        }
        TTM_TRACKACTIVATE => {
            if wp != 0 {
                if lp == 0 {
                    return Some(0);
                }
                let size = cpu.bus.guest_read_32(lp);
                let n = named_tool(cpu, &t, lp)?;
                if size < V1_SIZE {
                    return Some(0);
                }
                t.track = n;
                t.track_active = n != -1;
                put(cpu, hwnd, t);
                if n != -1 {
                    let ops = show_ops(cpu, hwnd, true);
                    s.ops.extend(ops);
                }
            } else {
                let ops = track_hide_ops(cpu, hwnd);
                s.ops.extend(ops);
                t.track_active = false;
                t.track = -1;
                put(cpu, hwnd, t);
            }
            0
        }
        TTM_TRACKPOSITION => {
            t.track_x = lp as u16 as i32;
            t.track_y = (lp >> 16) as u16 as i32;
            let active = t.track_active;
            put(cpu, hwnd, t);
            if active {
                let ops = show_ops(cpu, hwnd, true);
                s.ops.extend(ops);
            }
            0
        }
        TTM_UPDATE => {
            if t.current != -1 {
                s.user("UpdateWindow", &[hwnd]);
            }
            0
        }
        TTM_WINDOWFROMPOINT => {
            let p = read_words(cpu, lp, 2);
            if cpu.bus.guest_faulted() {
                return None;
            }
            user32::window::window_from_point(cpu, &p).unwrap_or(0)
        }
        WM_DESTROY => {
            for tool in &t.tools {
                if tool.flags & TTF_SUBCLASS != 0 {
                    unsubclass(cpu, subclass_target(tool.flags, tool.hwnd, tool.id), hwnd);
                }
            }
            state(cpu).tooltips.tips.remove(&hwnd);
            0
        }
        WM_ERASEBKGND => 0,
        WM_GETFONT => t.font,
        WM_GETTEXT => {
            let n = user32::copy_out(cpu, lp, wp, &t.tip);
            if cpu.bus.guest_faulted() {
                return None;
            }
            n
        }
        WM_GETTEXTLENGTH => t.tip.len() as u32,
        WM_LBUTTONDOWN | WM_LBUTTONUP | WM_MBUTTONDOWN | WM_MBUTTONUP | WM_RBUTTONDOWN
        | WM_RBUTTONUP | WM_MOUSEMOVE => {
            let ops = hide_ops(cpu, hwnd, S_HDR);
            s.ops.extend(ops);
            0
        }
        WM_NCHITTEST => {
            let n = if t.track_active { t.track } else { t.tool };
            let transparent = n > -1
                && t.tools
                    .get(n as usize)
                    .is_some_and(|tl| tl.flags & TTF_TRANSPARENT != 0);
            if transparent {
                return Some(HTTRANSPARENT);
            }
            return defwnd::def_window_proc(cpu, args);
        }
        WM_NOTIFYFORMAT => 0,
        WM_PAINT | WM_PRINTCLIENT => {
            if wp != 0 {
                let flags = draw_flags(cpu, hwnd, &t);
                draw(cpu, hwnd, wp, flags);
                return Some(0);
            }
            return script::run(cpu, TOOLTIPS, paint(hwnd));
        }
        WM_SETFONT => {
            t.font = if wp != 0 { wp } else { gfx::default_font(cpu) };
            put(cpu, hwnd, t);
            0
        }
        WM_TIMER => {
            timer(cpu, hwnd, wp, &mut s);
            0
        }
        _ => return defwnd::def_window_proc(cpu, args),
    };
    s.result = result;
    s.ret(result);
    script::run(cpu, TOOLTIPS, s)
}

/// TOOLTIPS_Timer.
fn timer(cpu: &mut Cpu, hwnd: u32, id: u32, s: &mut Script) {
    use user32::message::{kill_timer, set_timer};
    let Some(mut t) = get(cpu, hwnd) else {
        return;
    };
    match id {
        ID_TIMERSHOW => {
            kill_timer(cpu, &[hwnd, ID_TIMERSHOW]);
            let old = t.tool;
            t.tool = check_tool(cpu, hwnd, &t, true);
            let same = t.tool == old && old != -1;
            put(cpu, hwnd, t);
            if same {
                let ops = show_ops(cpu, hwnd, false);
                s.ops.extend(ops);
            }
        }
        ID_TIMERPOP => {
            let ops = hide_ops(cpu, hwnd, S_HDR);
            s.ops.extend(ops);
        }
        ID_TIMERLEAVE => {
            let old = t.tool;
            t.tool = check_tool(cpu, hwnd, &t, false);
            let (tool, active, reshow) = (t.tool, t.active, t.reshow);
            put(cpu, hwnd, t);
            if tool != old {
                let ops = hide_ops(cpu, hwnd, S_HDR);
                s.ops.extend(ops);
                kill_timer(cpu, &[hwnd, ID_TIMERLEAVE]);
                if tool != -1 && active {
                    set_timer(cpu, &[hwnd, ID_TIMERSHOW, reshow as u32, 0]);
                }
            }
        }
        _ => {}
    }
}

// For the toolbar, which keeps the tools of its own tooltip.

/// Add a tool of `hwnd` with the id `id`, its text asked of `hwnd` with
/// TTN_GETDISPINFOA.
pub fn add_tool(cpu: &mut Cpu, tip: u32, hwnd: u32, id: u32, r: Rect) {
    if let Some(mut t) = get(cpu, tip) {
        t.tools.push(Tool {
            hwnd,
            id,
            rect: r,
            kind: TEXT_CALLBACK,
            ..Tool::default()
        });
        put(cpu, tip, t);
    }
}

/// Remove the tool of `hwnd` with the id `id`.
pub fn del_tool(cpu: &mut Cpu, tip: u32, hwnd: u32, id: u32) {
    let Some(mut t) = get(cpu, tip) else {
        return;
    };
    let n = tool_from_info(&t, hwnd, id);
    if n == -1 {
        return;
    }
    t.tools.remove(n as usize);
    forget_index(&mut t, n);
    put(cpu, tip, t);
}

/// TTM_NEWTOOLRECT for the tool of `hwnd` with the id `id`.
pub fn set_tool_rect(cpu: &mut Cpu, tip: u32, hwnd: u32, id: u32, r: Rect) {
    let Some(mut t) = get(cpu, tip) else {
        return;
    };
    let n = tool_from_info(&t, hwnd, id);
    if n != -1 {
        t.tools[n as usize].rect = r;
        put(cpu, tip, t);
    }
}

/// TTM_RELAYEVENT of the message `msg` of `hwnd`, its steps added to
/// `s`.
pub fn relay(cpu: &mut Cpu, tip: u32, hwnd: u32, msg: u32, lp: u32, s: &mut Script) {
    if exists(cpu, tip) {
        relay_into(cpu, tip, hwnd, msg, lp, s);
    }
}
