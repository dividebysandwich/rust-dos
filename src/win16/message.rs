//! Messages between 16-bit and 32-bit code: a 16-bit window procedure
//! USER32 calls (through its 32-bit slot, thunk.rs), a 32-bit one 16-bit
//! code calls, and the messages 16-bit programs send and post. Windows
//! 3.1's messages pack some parameters differently (WM_COMMAND,
//! WM_ACTIVATE, the scroll bars' and menus' messages), have one
//! WM_CTLCOLOR for Win32's seven, number the controls' messages from
//! WM_USER, and point to structures of 16-bit fields; each is
//! translated, the structures copied to the other side's stack and
//! back.
//!
//! The translations follow Wine's user.exe16 message.c (LGPL 2.1 or
//! later, copyright 1993-1996 Alexandre Julliard and others).

use super::Thunk;
use super::handles::{icon16, icon32, menu16, menu32};
use super::thunk::{self, ESP, KIND_DLGPROC};
use crate::cpu::{Cpu, Seg};
use crate::dpmi::Context;

const WM_CREATE: u32 = 0x0001;
const WM_ACTIVATE: u32 = 0x0006;
const WM_SETTEXT: u32 = 0x000C;
const WM_GETTEXT: u32 = 0x000D;
const WM_CTLCOLOR16: u32 = 0x0019;
const WM_WININICHANGE: u32 = 0x001A;
const WM_DEVMODECHANGE: u32 = 0x001B;
const WM_ACTIVATEAPP: u32 = 0x001C;
const WM_GETMINMAXINFO: u32 = 0x0024;
const WM_DRAWITEM: u32 = 0x002B;
const WM_MEASUREITEM: u32 = 0x002C;
const WM_DELETEITEM: u32 = 0x002D;
const WM_VKEYTOITEM: u32 = 0x002E;
const WM_CHARTOITEM: u32 = 0x002F;
const WM_GETFONT: u32 = 0x0031;
const WM_QUERYDRAGICON: u32 = 0x0037;
const WM_COMPAREITEM: u32 = 0x0039;
const WM_WINDOWPOSCHANGING: u32 = 0x0046;
const WM_WINDOWPOSCHANGED: u32 = 0x0047;
const WM_GETICON: u32 = 0x007F;
const WM_SETICON: u32 = 0x0080;
const WM_NCCREATE: u32 = 0x0081;
const WM_NCCALCSIZE: u32 = 0x0083;
const WM_NCHITTEST: u32 = 0x0084;
const WM_COMMAND: u32 = 0x0111;
const WM_HSCROLL: u32 = 0x0114;
const WM_VSCROLL: u32 = 0x0115;
const WM_INITMENU: u32 = 0x0116;
const WM_INITMENUPOPUP: u32 = 0x0117;
const WM_MENUSELECT: u32 = 0x011F;
const WM_MENUCHAR: u32 = 0x0120;
const WM_CTLCOLORMSGBOX: u32 = 0x0132;
const WM_CTLCOLORSTATIC: u32 = 0x0138;
const WM_PARENTNOTIFY: u32 = 0x0210;
const WM_MDICREATE: u32 = 0x0220;
const WS_EX_MDICHILD: u32 = 0x0040;
const WM_DESTROY: u32 = 0x0002;
const WM_USER: u32 = 0x0400;

/// The 16-bit and 32-bit numbers of the controls' messages: Windows 3.1
/// numbers them from WM_USER, Win32 below it.
const EM_GETSEL: u32 = 0x00B0;
const LB_ADDSTRING: u32 = 0x0180;
const CB_GETEDITSEL: u32 = 0x0140;
const BM_GETCHECK: u32 = 0x00F0;
const STM_SETICON: u32 = 0x0170;
const STM_GETICON: u32 = 0x0171;

/// The controls' messages that take strings and other pointers.
const EM_GETRECT: u32 = 0x00B2;
const EM_SETRECT: u32 = 0x00B3;
const EM_SETRECTNP: u32 = 0x00B4;
const EM_GETLINE: u32 = 0x00C4;
const EM_REPLACESEL: u32 = 0x00C2;
const EM_SETTABSTOPS: u32 = 0x00CB;
const LB_INSERTSTRING: u32 = 0x0181;
const LB_SELECTSTRING: u32 = 0x018C;
const LB_DIR: u32 = 0x018D;
const LB_FINDSTRING: u32 = 0x018F;
const LB_GETTEXT: u32 = 0x0189;
const LB_GETSELITEMS: u32 = 0x0191;
const LB_SETTABSTOPS: u32 = 0x0192;
const LB_GETITEMRECT: u32 = 0x0198;
const LB_FINDSTRINGEXACT: u32 = 0x01A2;
const LB_ADDFILE: u32 = 0x0196;
const CB_ADDSTRING: u32 = 0x0143;
const CB_DIR: u32 = 0x0145;
const CB_FINDSTRING: u32 = 0x014C;
const CB_GETLBTEXT: u32 = 0x0148;
const CB_INSERTSTRING: u32 = 0x014A;
const CB_SELECTSTRING: u32 = 0x014D;
const CB_GETDROPPEDCONTROLRECT: u32 = 0x0152;
const CB_FINDSTRINGEXACT: u32 = 0x0158;

/// What is written back when a call returns, and how what it returned
/// is converted: `Fix` values in `data` after the message's number.
const FIX_NONE: u32 = 0;
/// A structure copied to the 16-bit side: its kind, the 32-bit and the
/// 16-bit addresses.
const FIX_MINMAX: u32 = 1;
const FIX_WINDOWPOS: u32 = 2;
const FIX_RECT: u32 = 3;
const FIX_NCCALC: u32 = 4;
const FIX_MEASURE: u32 = 5;
/// A buffer the callee filled: its length is what the call returned.
const FIX_TEXT: u32 = 6;
/// A buffer with a NUL-terminated string the callee filled.
const FIX_STRING: u32 = 7;
/// An array of `len` integers the callee filled, 16-bit and 32-bit.
const FIX_INTS: u32 = 8;

fn lo(v: u32) -> u32 {
    v & 0xFFFF
}

fn hi(v: u32) -> u32 {
    v >> 16
}

fn make_long(low: u32, high: u32) -> u32 {
    (high & 0xFFFF) << 16 | low & 0xFFFF
}

fn sx(v: u32) -> u32 {
    v as u16 as i16 as i32 as u32
}

/// Space on the 16-bit stack, below where a call's frame goes, for the
/// structures a 16-bit procedure is passed.
pub(super) struct Stack16 {
    pub(super) ss: u16,
    base: u32,
    pub(super) sp: u16,
}

impl Stack16 {
    pub(super) fn new(cpu: &Cpu, (ss, sp): (u16, u16)) -> Stack16 {
        let base = super::selector_base(cpu, ss).map_or(0, |(b, _, _)| b);
        Stack16 { ss, base, sp }
    }

    /// `len` bytes: their 16:16 address and their linear one.
    pub(super) fn alloc(&mut self, len: u32) -> (u32, u32) {
        self.sp = self.sp.wrapping_sub((len as u16 + 3) & !3);
        let lin = self.base.wrapping_add(self.sp as u32);
        ((self.ss as u32) << 16 | self.sp as u32, lin)
    }
}

/// Space on the 32-bit stack, below where a call goes.
struct Stack32 {
    top: u32,
}

impl Stack32 {
    fn alloc(&mut self, len: u32) -> u32 {
        self.top = (self.top - len.next_multiple_of(4)) & !3;
        self.top
    }
}

fn r16(cpu: &Cpu, at: u32) -> u32 {
    super::read_16(cpu, at) as u32
}

fn r32(cpu: &Cpu, at: u32) -> u32 {
    super::read_32(cpu, at)
}

fn w16(cpu: &mut Cpu, at: u32, v: u32) {
    super::write_16(cpu, at, v as u16);
}

fn w32(cpu: &mut Cpu, at: u32, v: u32) {
    super::write_32(cpu, at, v);
}

/// Copy `n` 32-bit integers at `from` to 16-bit ones at `to`, or back
/// (sign-extended) with `widen`.
fn copy_ints(cpu: &mut Cpu, from: u32, to: u32, n: u32, widen: bool) {
    for i in 0..n {
        if widen {
            let v = sx(r16(cpu, from + 2 * i));
            w32(cpu, to + 4 * i, v);
        } else {
            let v = r32(cpu, from + 4 * i);
            w16(cpu, to + 2 * i, v);
        }
    }
}

/// Copy `n` 16-bit RECTs at `from` to 32-bit ones at `to`: the left and
/// top edges sign-extended, the right and bottom ones that far from them.
/// A window near x = 32767 has a right edge that wraps to a negative
/// number in 16 bits; widened alone, it would make the rectangle empty.
fn widen_rects(cpu: &mut Cpu, from: u32, to: u32, n: u32) {
    for k in 0..n {
        for j in 0..2 {
            let (at16, at) = (from + 8 * k + 2 * j, to + 16 * k + 4 * j);
            let first = r16(cpu, at16);
            let second = r16(cpu, at16 + 4);
            let size = (second as u16).wrapping_sub(first as u16) as i16 as i32 as u32;
            w32(cpu, at, sx(first));
            w32(cpu, at + 8, sx(first).wrapping_add(size));
        }
    }
}

/// Copy `n` 16-bit integers at `from` back over the 32-bit ones at `to`
/// that they were made from, each widened to the value nearest the one
/// it replaces. A 16-bit procedure works on coordinates modulo 64K, as
/// Windows 3.1's USER does: a window moved to x = 32700 has a right
/// edge that wraps to a negative number, and the client rectangle its
/// WM_NCCALCSIZE gives back has to keep the 32-bit edges it came from,
/// or the client area stays empty after the window moves back.
fn copy_ints_back(cpu: &mut Cpu, from: u32, to: u32, n: u32) {
    for i in 0..n {
        let old = r32(cpu, to + 4 * i);
        let delta = (r16(cpu, from + 2 * i) as u16).wrapping_sub(old as u16) as i16;
        w32(cpu, to + 4 * i, old.wrapping_add(delta as i32 as u32));
    }
}

/// Copy a string (with its NUL) at the linear `from` to `to`.
fn copy_string(cpu: &mut Cpu, from: u32, to: u32) {
    let mut s = super::string_bytes(cpu, from, 0x1_0000);
    s.push(0);
    super::poke(cpu, to, &s);
}

/// The class of the window `hwnd`, by the names USER32 gives the
/// system's, lower case.
fn class_name(cpu: &Cpu, hwnd: u32) -> String {
    let user = &cpu.bus.win32.user;
    user.window(hwnd)
        .and_then(|w| user.classes.get(w.class as usize))
        .and_then(|c| c.as_ref())
        .map_or(String::new(), |c| c.name.to_ascii_lowercase())
}

/// A 16-bit program's message to the window `hwnd` as Win32 numbers it:
/// the controls' messages of the system's classes moved down from
/// WM_USER.
fn control_msg32(cpu: &Cpu, hwnd: u32, msg: u32) -> u32 {
    if !(WM_USER..WM_USER + 0x40).contains(&msg) {
        return msg;
    }
    let n = msg - WM_USER;
    match class_name(cpu, hwnd).as_str() {
        "edit" if n <= 0x29 => EM_GETSEL + n,
        "listbox" | "combolbox" if (1..=0x2A).contains(&n) => LB_ADDSTRING + n - 1,
        "combobox" if n <= 0x1C => CB_GETEDITSEL + n,
        "button" if n <= 4 => BM_GETCHECK + n,
        "static" if n <= 3 => STM_SETICON + n,
        _ => msg,
    }
}

/// Win32's number of a control's message back to Windows 3.1's, for the
/// window `hwnd`.
fn control_msg16(cpu: &Cpu, hwnd: u32, msg: u32) -> u32 {
    let class = class_name(cpu, hwnd);
    match class.as_str() {
        "edit" if (EM_GETSEL..=EM_GETSEL + 0x29).contains(&msg) => msg - EM_GETSEL + WM_USER,
        "listbox" | "combolbox" if (LB_ADDSTRING..=LB_ADDSTRING + 0x29).contains(&msg) => {
            msg - LB_ADDSTRING + WM_USER + 1
        }
        "combobox" if (CB_GETEDITSEL..=CB_GETEDITSEL + 0x1C).contains(&msg) => {
            msg - CB_GETEDITSEL + WM_USER
        }
        "button" if (BM_GETCHECK..=BM_GETCHECK + 4).contains(&msg) => msg - BM_GETCHECK + WM_USER,
        "static" if (STM_SETICON..=STM_SETICON + 3).contains(&msg) => msg - STM_SETICON + WM_USER,
        _ => msg,
    }
}

/// A message 16-bit code sends or passes on, as 32-bit code gets it:
/// (msg, wParam, lParam), the structures it points to copied to `stack`,
/// and what to write back when it returns (`data` grows).
fn to32(
    cpu: &mut Cpu,
    hwnd: u32,
    msg16: u32,
    wp: u32,
    lp: u32,
    stack: &mut Stack32,
    data: &mut Vec<u32>,
) -> (u32, u32, u32) {
    let msg = control_msg32(cpu, hwnd, msg16);
    let flat = |cpu: &Cpu, lp: u32| super::linear(cpu, lp);
    data.push(msg);

    match msg {
        WM_CREATE | WM_NCCREATE => {
            let cs16 = flat(cpu, lp);
            let cs = stack.alloc(48);
            if cs16 != 0 {
                let instance = super::handles::instance32(cpu, r16(cpu, cs16 + 4) as u16);
                let menu = menu32(r16(cpu, cs16 + 6) as u16);
                let name = r32(cpu, cs16 + 22);
                let class = r32(cpu, cs16 + 26);
                let fields = [
                    r32(cpu, cs16),
                    instance,
                    menu,
                    r16(cpu, cs16 + 8),
                    sx(r16(cpu, cs16 + 10)),
                    sx(r16(cpu, cs16 + 12)),
                    sx(r16(cpu, cs16 + 14)),
                    sx(r16(cpu, cs16 + 16)),
                    r32(cpu, cs16 + 18),
                    flat(cpu, name),
                    if hi(class) == 0 {
                        class
                    } else {
                        flat(cpu, class)
                    },
                    r32(cpu, cs16 + 30),
                ];
                for (i, f) in fields.into_iter().enumerate() {
                    w32(cpu, cs + 4 * i as u32, f);
                }
            }
            data.push(FIX_NONE);
            (msg, wp, if cs16 == 0 { 0 } else { cs })
        }
        WM_MDICREATE => {
            // MDICREATESTRUCT: the names flat, the owner an instance of
            // 32-bit code, the position and size widened (CW_USEDEFAULT
            // to Win32's).
            let mc16 = flat(cpu, lp);
            let mc = stack.alloc(36);
            if mc16 != 0 {
                let class = r32(cpu, mc16);
                let coord = |v: u32| if v == 0x8000 { 0x8000_0000 } else { sx(v) };
                let fields = [
                    if hi(class) == 0 {
                        class
                    } else {
                        flat(cpu, class)
                    },
                    flat(cpu, r32(cpu, mc16 + 4)),
                    super::handles::instance32(cpu, r16(cpu, mc16 + 8) as u16),
                    coord(r16(cpu, mc16 + 10)),
                    coord(r16(cpu, mc16 + 12)),
                    coord(r16(cpu, mc16 + 14)),
                    coord(r16(cpu, mc16 + 16)),
                    r32(cpu, mc16 + 18),
                    r32(cpu, mc16 + 22),
                ];
                for (i, f) in fields.into_iter().enumerate() {
                    w32(cpu, mc + 4 * i as u32, f);
                }
            }
            data.push(FIX_NONE);
            (msg, wp, if mc16 == 0 { 0 } else { mc })
        }
        WM_GETMINMAXINFO => {
            let p16 = flat(cpu, lp);
            let p = stack.alloc(40);
            copy_ints(cpu, p16, p, 10, true);
            data.extend([FIX_MINMAX, p, p16]);
            (msg, wp, p)
        }
        WM_WINDOWPOSCHANGING | WM_WINDOWPOSCHANGED => {
            let p16 = flat(cpu, lp);
            let p = stack.alloc(28);
            windowpos32(cpu, p16, p);
            data.extend([FIX_WINDOWPOS, p, p16]);
            (msg, wp, p)
        }
        WM_NCCALCSIZE => {
            let p16 = flat(cpu, lp);
            if wp != 0 {
                let p = stack.alloc(52);
                widen_rects(cpu, p16, p, 3);
                let pos16 = flat(cpu, r32(cpu, p16 + 24));
                let pos = stack.alloc(28);
                if pos16 != 0 {
                    windowpos32(cpu, pos16, pos);
                }
                w32(cpu, p + 48, if pos16 == 0 { 0 } else { pos });
                data.extend([FIX_NCCALC, p, p16]);
                (msg, wp, p)
            } else {
                let p = stack.alloc(16);
                widen_rects(cpu, p16, p, 1);
                data.extend([FIX_RECT, p, p16]);
                (msg, wp, p)
            }
        }
        WM_DRAWITEM => {
            let d16 = flat(cpu, lp);
            let d = stack.alloc(48);
            for i in 0..7 {
                let v = r16(cpu, d16 + 2 * i);
                w32(cpu, d + 4 * i, v);
            }
            copy_ints(cpu, d16 + 14, d + 28, 4, true);
            let v = r32(cpu, d16 + 22);
            w32(cpu, d + 44, v);
            data.push(FIX_NONE);
            (msg, wp, d)
        }
        WM_MEASUREITEM => {
            let m16 = flat(cpu, lp);
            let m = stack.alloc(24);
            for i in 0..5 {
                let v = r16(cpu, m16 + 2 * i);
                w32(cpu, m + 4 * i, v);
            }
            let v = r32(cpu, m16 + 10);
            w32(cpu, m + 20, v);
            data.extend([FIX_MEASURE, m, m16]);
            (msg, wp, m)
        }
        WM_DELETEITEM => {
            let d16 = flat(cpu, lp);
            let d = stack.alloc(20);
            for i in 0..4 {
                let v = r16(cpu, d16 + 2 * i);
                w32(cpu, d + 4 * i, v);
            }
            let v = r32(cpu, d16 + 8);
            w32(cpu, d + 16, v);
            data.push(FIX_NONE);
            (msg, wp, d)
        }
        WM_COMPAREITEM => {
            let c16 = flat(cpu, lp);
            let c = stack.alloc(32);
            let fields = [
                r16(cpu, c16),
                r16(cpu, c16 + 2),
                r16(cpu, c16 + 4),
                r16(cpu, c16 + 6),
                r32(cpu, c16 + 8),
                r16(cpu, c16 + 12),
                r32(cpu, c16 + 14),
                0,
            ];
            for (i, f) in fields.into_iter().enumerate() {
                w32(cpu, c + 4 * i as u32, f);
            }
            data.push(FIX_NONE);
            (msg, wp, c)
        }
        WM_COMMAND => {
            data.push(FIX_NONE);
            (msg, make_long(wp, hi(lp)), lo(lp))
        }
        WM_ACTIVATE | WM_CHARTOITEM | WM_VKEYTOITEM => {
            data.push(FIX_NONE);
            (msg, make_long(wp, hi(lp)), lo(lp))
        }
        WM_HSCROLL | WM_VSCROLL => {
            data.push(FIX_NONE);
            (msg, make_long(wp, lo(lp)), hi(lp))
        }
        WM_MENUSELECT => {
            data.push(FIX_NONE);
            let flags = lo(lp);
            if flags == 0xFFFF && hi(lp) == 0 {
                (msg, 0xFFFF_0000, 0)
            } else {
                (msg, make_long(wp, flags), menu32(hi(lp) as u16))
            }
        }
        WM_MENUCHAR => {
            data.push(FIX_NONE);
            (msg, make_long(wp, lo(lp)), menu32(hi(lp) as u16))
        }
        WM_INITMENU | WM_INITMENUPOPUP => {
            data.push(FIX_NONE);
            (msg, menu32(wp as u16), lp)
        }
        WM_CTLCOLOR16 => {
            let last = data.len() - 1;
            data[last] = WM_CTLCOLORMSGBOX + hi(lp).min(6);
            data.push(FIX_NONE);
            (WM_CTLCOLORMSGBOX + hi(lp).min(6), wp, lo(lp))
        }
        WM_PARENTNOTIFY => {
            data.push(FIX_NONE);
            if wp == WM_CREATE || wp == WM_DESTROY {
                (msg, make_long(wp, hi(lp)), lo(lp))
            } else {
                (msg, wp, lp)
            }
        }
        WM_SETTEXT | WM_WININICHANGE | WM_DEVMODECHANGE | WM_GETTEXT | EM_REPLACESEL
        | LB_ADDSTRING | LB_INSERTSTRING | LB_FINDSTRING | LB_FINDSTRINGEXACT | LB_SELECTSTRING
        | LB_DIR | LB_ADDFILE | LB_GETTEXT | CB_ADDSTRING | CB_INSERTSTRING | CB_FINDSTRING
        | CB_FINDSTRINGEXACT | CB_SELECTSTRING | CB_DIR | CB_GETLBTEXT | EM_GETLINE => {
            data.push(FIX_NONE);
            // A list box without strings has item data, not pointers.
            let strings = !matches!(
                msg,
                LB_ADDSTRING
                    | LB_INSERTSTRING
                    | LB_FINDSTRING
                    | LB_FINDSTRINGEXACT
                    | LB_SELECTSTRING
                    | CB_ADDSTRING
                    | CB_INSERTSTRING
                    | CB_FINDSTRING
                    | CB_FINDSTRINGEXACT
                    | CB_SELECTSTRING
            ) || has_strings(cpu, hwnd);
            (
                msg,
                sx_wparam(msg, wp),
                if strings { flat(cpu, lp) } else { lp },
            )
        }
        EM_GETRECT | LB_GETITEMRECT | CB_GETDROPPEDCONTROLRECT => {
            let r16 = flat(cpu, lp);
            let r = stack.alloc(16);
            data.extend([FIX_RECT, r, r16]);
            (msg, sx_wparam(msg, wp), r)
        }
        EM_SETRECT | EM_SETRECTNP => {
            let r16 = flat(cpu, lp);
            let r = stack.alloc(16);
            copy_ints(cpu, r16, r, 4, true);
            data.push(FIX_NONE);
            (msg, wp, r)
        }
        EM_SETTABSTOPS | LB_SETTABSTOPS => {
            let a16 = flat(cpu, lp);
            let n = wp.min(64);
            let a = stack.alloc(4 * n.max(1));
            copy_ints(cpu, a16, a, n, true);
            data.push(FIX_NONE);
            (msg, wp, if a16 == 0 { 0 } else { a })
        }
        LB_GETSELITEMS => {
            let a16 = flat(cpu, lp);
            let n = wp.min(0x1000);
            let a = stack.alloc(4 * n.max(1));
            data.extend([FIX_INTS, a, a16]);
            (msg, wp, a)
        }
        WM_SETICON => {
            data.push(FIX_NONE);
            (msg, wp, icon32(lp as u16))
        }
        STM_SETICON => {
            data.push(FIX_NONE);
            (msg, icon32(wp as u16), lp)
        }
        _ => {
            data.push(FIX_NONE);
            (msg, sx_wparam(msg, wp), lp)
        }
    }
}

/// The 16-bit wParam widened: the controls' indexes are signed.
fn sx_wparam(msg: u32, wp: u32) -> u32 {
    match msg {
        LB_GETTEXT | LB_INSERTSTRING | LB_FINDSTRING | LB_FINDSTRINGEXACT | LB_SELECTSTRING
        | CB_GETLBTEXT | CB_INSERTSTRING | CB_FINDSTRING | CB_FINDSTRINGEXACT | CB_SELECTSTRING
        | LB_GETITEMRECT => sx(wp),
        _ => wp & 0xFFFF,
    }
}

/// Whether the list box or combo box `hwnd` keeps strings.
fn has_strings(cpu: &Cpu, hwnd: u32) -> bool {
    let style = cpu.bus.win32.user.window(hwnd).map_or(0, |w| w.style);
    let class = class_name(cpu, hwnd);
    let (owner_draw, strings) = if class == "combobox" {
        (0x0030, 0x0200)
    } else {
        (0x0030, 0x0040)
    };
    style & owner_draw == 0 || style & strings != 0
}

/// A WINDOWPOS16 at `p16` as a WINDOWPOS at `p`.
fn windowpos32(cpu: &mut Cpu, p16: u32, p: u32) {
    let fields = [
        r16(cpu, p16),
        r16(cpu, p16 + 2),
        sx(r16(cpu, p16 + 4)),
        sx(r16(cpu, p16 + 6)),
        sx(r16(cpu, p16 + 8)),
        sx(r16(cpu, p16 + 10)),
        r16(cpu, p16 + 12),
    ];
    for (i, f) in fields.into_iter().enumerate() {
        w32(cpu, p + 4 * i as u32, f);
    }
}

/// A WINDOWPOS at `p` as a WINDOWPOS16 at `p16`.
fn windowpos16(cpu: &mut Cpu, p: u32, p16: u32) {
    for i in 0..7 {
        let v = r32(cpu, p + 4 * i);
        w16(cpu, p16 + 2 * i, v);
    }
}

/// What 32-bit code returned for a message 16-bit code sent (`data`,
/// from `to32`): the structures written back to the 16-bit side, the
/// value as 16-bit code gets it.
fn returned16(cpu: &mut Cpu, value: u32, data: &[u32]) -> u32 {
    let msg = data.first().copied().unwrap_or(0);
    let fix = &data[1.min(data.len())..];
    if let [kind, p, p16, ..] = *fix {
        match kind {
            FIX_MINMAX => copy_ints(cpu, p, p16, 10, false),
            FIX_WINDOWPOS => windowpos16(cpu, p, p16),
            FIX_RECT => copy_ints(cpu, p, p16, 4, false),
            FIX_NCCALC => copy_ints(cpu, p, p16, 12, false),
            FIX_MEASURE => {
                for i in 3..5 {
                    let v = r32(cpu, p + 4 * i);
                    w16(cpu, p16 + 2 * i, v);
                }
            }
            FIX_INTS => {
                let n = (value as i32).max(0) as u32;
                copy_ints(cpu, p, p16, n.min(0x1000), false);
            }
            _ => {}
        }
    }
    match msg {
        WM_GETICON | WM_QUERYDRAGICON | STM_SETICON | STM_GETICON => icon16(value) as u32,
        _ => value,
    }
}

/// A message 32-bit code sends a 16-bit window procedure, as that gets
/// it: (msg, wParam, lParam), the structures copied to `stack`, what to
/// write back after (`data` grows).
pub(super) fn to16(
    cpu: &mut Cpu,
    hwnd: u32,
    msg: u32,
    wp: u32,
    lp: u32,
    stack: &mut Stack16,
    data: &mut Vec<u32>,
) -> (u32, u32, u32) {
    data.push(msg);
    match msg {
        WM_CREATE | WM_NCCREATE => {
            data.push(FIX_NONE);
            (msg, wp, create_struct16(cpu, lp, stack))
        }
        WM_GETMINMAXINFO => {
            let (seg, p16) = stack.alloc(20);
            copy_ints(cpu, lp, p16, 10, false);
            data.extend([FIX_MINMAX, lp, p16]);
            (msg, wp, seg)
        }
        WM_WINDOWPOSCHANGING | WM_WINDOWPOSCHANGED => {
            let (seg, p16) = stack.alloc(14);
            windowpos16(cpu, lp, p16);
            data.extend([FIX_WINDOWPOS, lp, p16]);
            (msg, wp, seg)
        }
        WM_NCCALCSIZE => {
            if wp != 0 {
                let (seg, p16) = stack.alloc(28);
                copy_ints(cpu, lp, p16, 12, false);
                let pos = r32(cpu, lp + 48);
                let (pos_seg, pos16) = stack.alloc(14);
                if pos != 0 {
                    windowpos16(cpu, pos, pos16);
                }
                w32(cpu, p16 + 24, if pos == 0 { 0 } else { pos_seg });
                data.extend([FIX_NCCALC, lp, p16]);
                (msg, wp, seg)
            } else {
                let (seg, p16) = stack.alloc(8);
                copy_ints(cpu, lp, p16, 4, false);
                data.extend([FIX_RECT, lp, p16]);
                (msg, wp, seg)
            }
        }
        WM_DRAWITEM => {
            let (seg, d16) = stack.alloc(26);
            for i in 0..7 {
                let v = r32(cpu, lp + 4 * i);
                w16(cpu, d16 + 2 * i, v);
            }
            copy_ints(cpu, lp + 28, d16 + 14, 4, false);
            let v = r32(cpu, lp + 44);
            w32(cpu, d16 + 22, v);
            data.push(FIX_NONE);
            (msg, wp, seg)
        }
        WM_MEASUREITEM => {
            let (seg, m16) = stack.alloc(14);
            for i in 0..5 {
                let v = r32(cpu, lp + 4 * i);
                w16(cpu, m16 + 2 * i, v);
            }
            let v = r32(cpu, lp + 20);
            w32(cpu, m16 + 10, v);
            data.extend([FIX_MEASURE, lp, m16]);
            (msg, wp, seg)
        }
        WM_DELETEITEM => {
            let (seg, d16) = stack.alloc(12);
            for i in 0..4 {
                let v = r32(cpu, lp + 4 * i);
                w16(cpu, d16 + 2 * i, v);
            }
            let v = r32(cpu, lp + 16);
            w32(cpu, d16 + 8, v);
            data.push(FIX_NONE);
            (msg, wp, seg)
        }
        WM_COMPAREITEM => {
            let (seg, c16) = stack.alloc(18);
            for (i, at) in [0u32, 4, 8, 12].into_iter().enumerate() {
                let v = r32(cpu, lp + at);
                w16(cpu, c16 + 2 * i as u32, v);
            }
            let v = r32(cpu, lp + 16);
            w32(cpu, c16 + 8, v);
            let v = r32(cpu, lp + 20);
            w16(cpu, c16 + 12, v);
            let v = r32(cpu, lp + 24);
            w32(cpu, c16 + 14, v);
            data.push(FIX_NONE);
            (msg, wp, seg)
        }
        WM_GETTEXT => {
            let size = wp.min(0x800);
            let (seg, b16) = stack.alloc(size.max(1));
            data.extend([FIX_TEXT, lp, b16, size]);
            (msg, size, seg)
        }
        WM_SETTEXT | WM_WININICHANGE | WM_DEVMODECHANGE => {
            let seg = if lp == 0 { 0 } else { string16(cpu, stack, lp) };
            data.push(FIX_NONE);
            (msg, wp, seg)
        }
        WM_COMMAND => {
            data.push(FIX_NONE);
            (msg, lo(wp), make_long(lp, hi(wp)))
        }
        WM_ACTIVATE | WM_CHARTOITEM | WM_VKEYTOITEM => {
            data.push(FIX_NONE);
            (msg, lo(wp), make_long(lp, hi(wp)))
        }
        WM_HSCROLL | WM_VSCROLL => {
            data.push(FIX_NONE);
            (msg, lo(wp), make_long(hi(wp), lp))
        }
        WM_MENUSELECT => {
            data.push(FIX_NONE);
            if hi(wp) == 0xFFFF && lp == 0 {
                (msg, 0, make_long(0xFFFF, 0))
            } else {
                (msg, lo(wp), make_long(hi(wp), menu16(lp) as u32))
            }
        }
        WM_MENUCHAR => {
            data.push(FIX_NONE);
            (msg, lo(wp), make_long(hi(wp), menu16(lp) as u32))
        }
        WM_INITMENU | WM_INITMENUPOPUP => {
            data.push(FIX_NONE);
            (msg, menu16(wp) as u32, lp)
        }
        WM_CTLCOLORMSGBOX..=WM_CTLCOLORSTATIC => {
            data.push(FIX_NONE);
            (
                WM_CTLCOLOR16,
                lo(wp),
                make_long(lp, msg - WM_CTLCOLORMSGBOX),
            )
        }
        WM_PARENTNOTIFY => {
            data.push(FIX_NONE);
            if lo(wp) == WM_CREATE || lo(wp) == WM_DESTROY {
                (msg, lo(wp), make_long(lp, hi(wp)))
            } else {
                (msg, lo(wp), lp)
            }
        }
        WM_ACTIVATEAPP => {
            data.push(FIX_NONE);
            let task = cpu
                .bus
                .win32
                .win16
                .tasks
                .iter()
                .find(|t| t.tid == lp)
                .map_or(0, |t| t.handle);
            (msg, lo(wp), task as u32)
        }
        WM_SETICON => {
            data.push(FIX_NONE);
            (msg, lo(wp), icon16(lp) as u32)
        }
        STM_SETICON => {
            data.push(FIX_NONE);
            (control_msg16(cpu, hwnd, msg), icon16(wp) as u32, lp)
        }
        _ => {
            data.push(FIX_NONE);
            let msg16 = control_msg16(cpu, hwnd, msg);
            (msg16, lo(wp), lp)
        }
    }
}

/// A copy of the string at the linear `at` on the 16-bit stack: its
/// 16:16 address.
fn string16(cpu: &mut Cpu, stack: &mut Stack16, at: u32) -> u32 {
    let mut s = super::string_bytes(cpu, at, 0x800);
    s.push(0);
    let (seg, lin) = stack.alloc(s.len() as u32);
    super::poke(cpu, lin, &s);
    seg
}

/// The MDICREATESTRUCT at `mc`, which an MDI child's CREATESTRUCT
/// points at, as a 16-bit one on `stack`: its 16:16 address. The names
/// are copies, the owner a 16-bit instance, the position and size
/// words (Win32's CW_USEDEFAULT the 16-bit one).
fn mdi_create16(cpu: &mut Cpu, stack: &mut Stack16, mc: u32) -> u32 {
    let class = r32(cpu, mc);
    let title = r32(cpu, mc + 4);
    let class16 = if class < 0x1_0000 {
        class
    } else {
        string16(cpu, stack, class)
    };
    let title16 = if title == 0 {
        0
    } else {
        string16(cpu, stack, title)
    };
    let owner = super::handles::instance16(cpu, r32(cpu, mc + 8));
    let (seg, mc16) = stack.alloc(26);
    w32(cpu, mc16, class16);
    w32(cpu, mc16 + 4, title16);
    w16(cpu, mc16 + 8, owner as u32);
    for i in 0..4 {
        let v = r32(cpu, mc + 12 + 4 * i);
        let v = if v == 0x8000_0000 { 0x8000 } else { v };
        w16(cpu, mc16 + 10 + 2 * i, v);
    }
    w32(cpu, mc16 + 18, r32(cpu, mc + 28));
    w32(cpu, mc16 + 22, r32(cpu, mc + 32));
    seg
}

/// What a 16-bit window procedure returned in DX:AX for a message 32-bit
/// code sent (`data`, from `to16`): the structures written back, the
/// value as 32-bit code gets it.
fn returned32(cpu: &mut Cpu, value: u32, data: &[u32], dialog: bool) -> u32 {
    let msg = data.first().copied().unwrap_or(0);
    let fix = &data[1.min(data.len())..];
    if let [kind, p, p16, ..] = *fix {
        match kind {
            FIX_MINMAX => copy_ints_back(cpu, p16, p, 10),
            FIX_WINDOWPOS => windowpos32(cpu, p16, p),
            FIX_RECT => copy_ints_back(cpu, p16, p, 4),
            FIX_NCCALC => copy_ints_back(cpu, p16, p, 12),
            FIX_MEASURE => {
                for i in 3..5 {
                    let v = r16(cpu, p16 + 2 * i);
                    w32(cpu, p + 4 * i, v);
                }
            }
            FIX_TEXT => {
                let size = fix.get(3).copied().unwrap_or(0);
                if size > 0 {
                    let len = (value & 0xFFFF).min(size - 1);
                    let mut text = super::peek(cpu, p16, len as usize).unwrap_or_default();
                    text.push(0);
                    super::poke(cpu, p, &text);
                }
            }
            FIX_STRING => copy_string(cpu, p16, p),
            _ => {}
        }
    }
    if dialog {
        return value & 0xFFFF;
    }
    match msg {
        WM_NCHITTEST => sx(value),
        WM_CTLCOLORMSGBOX..=WM_CTLCOLORSTATIC | WM_GETFONT => value & 0xFFFF,
        WM_QUERYDRAGICON | WM_GETICON | STM_SETICON | STM_GETICON => icon32(value as u16),
        WM_GETTEXT => value & 0xFFFF,
        _ => value,
    }
}

/// USER32 (or other 32-bit code) called the 32-bit slot of a 16-bit
/// window or dialog procedure: (hwnd, msg, wParam, lParam), stdcall.
pub fn call_wndproc16(cpu: &mut Cpu, thunk: &Thunk) {
    let esp = cpu.esp();
    let ret = cpu.bus.guest_read_32(esp);
    let args: Vec<u32> = (0..4)
        .map(|i| cpu.bus.guest_read_32(esp + 4 + 4 * i))
        .collect();
    let (hwnd, msg, wp, lp) = (args[0], args[1], args[2], args[3]);
    let mut resume = Context::of(cpu);
    resume.eip = ret;
    resume.gpr[ESP] = esp + 20;
    let at = thunk::stack16_for(cpu, &resume);
    let mut stack = Stack16::new(cpu, at);
    let mut data = vec![thunk.kind as u32];
    let (msg16, wp16, lp16) = to16(cpu, hwnd, msg, wp, lp, &mut stack, &mut data);
    let ds = window_ds(cpu, hwnd, thunk);
    thunk::call16_at(
        cpu,
        thunk.target,
        &[
            hwnd as u16,
            msg16 as u16,
            wp16 as u16,
            (lp16 >> 16) as u16,
            lp16 as u16,
        ],
        ds,
        resume,
        thunk::THEN_MESSAGE,
        data,
        (stack.ss, stack.sp),
    );
}

/// The data segment a 16-bit window procedure of `hwnd` gets in AX and
/// DS: its window's instance's, else its thunk's.
fn window_ds(cpu: &Cpu, hwnd: u32, thunk: &Thunk) -> u16 {
    let instance = cpu
        .bus
        .win32
        .user
        .window(hwnd)
        .map_or(0, |w| super::handles::instance16(cpu, w.instance));
    let instance = if instance != 0 {
        instance
    } else {
        thunk.extra as u16
    };
    if instance == 0 {
        return 0;
    }
    let sel = super::global::handle_to_sel(instance);
    if super::selector_base(cpu, sel).is_some() {
        sel
    } else {
        0
    }
}

/// A 16-bit window procedure returned (`call_wndproc16`): DX:AX
/// converted, the structures written back.
pub fn then(cpu: &mut Cpu, value: u32, data: &[u32], _: &mut Context) -> Option<u32> {
    let dialog = data.first() == Some(&(KIND_DLGPROC as u32));
    Some(returned32(cpu, value, &data[1..], dialog))
}

/// 16-bit code called the 16-bit slot of a 32-bit window procedure, as
/// CallWindowProc does with one: (hwnd, msg, wParam, lParam), Pascal's.
pub fn call_wndproc32(cpu: &mut Cpu, thunk: &Thunk) {
    let base = cpu.seg_cache(Seg::SS).base;
    let sp = cpu.sp() as u32;
    let at = |i: u32| base.wrapping_add((sp + i) & 0xFFFF);
    let lp = cpu.bus.guest_read_32(at(4));
    let wp = cpu.bus.guest_read_16(at(8)) as u32;
    let msg = cpu.bus.guest_read_16(at(10)) as u32;
    let hwnd = cpu.bus.guest_read_16(at(12)) as u32;
    send32(cpu, thunk.target, hwnd, msg, wp, lp);
}

/// Call the 32-bit procedure `target` with the message 16-bit code
/// passes (`msg`, its wParam and lParam), translated; DX:AX what it
/// returns, translated back.
pub fn send32(cpu: &mut Cpu, target: u32, hwnd: u32, msg: u32, wp: u32, lp: u32) {
    send32_with(cpu, target, hwnd, &[], msg, wp, lp);
}

/// `send32` for a procedure that takes `extra` after the window (as
/// DefFrameProc takes the MDI client).
pub fn send32_with(
    cpu: &mut Cpu,
    target: u32,
    hwnd: u32,
    extra: &[u32],
    msg: u32,
    wp: u32,
    lp: u32,
) {
    let ctx = Context::of(cpu);
    let mut stack = Stack32 {
        top: thunk::scratch32(cpu, 0x400),
    };
    let mut data = Vec::new();
    let (msg32, wp32, lp32) = to32(cpu, hwnd, msg, wp, lp, &mut stack, &mut data);
    let top = stack.top;
    let mut args = vec![hwnd];
    args.extend_from_slice(extra);
    args.extend([msg32, wp32, lp32]);
    thunk::enter32_at(cpu, ctx, top, target, &args, thunk::POST_MESSAGE, data);
}

/// A 32-bit function 16-bit code sent a message through returned: the
/// structures written back, the value as 16-bit code gets it.
pub fn post(cpu: &mut Cpu, value: u32, data: &[u32], _: &mut Context) -> Option<u32> {
    Some(returned16(cpu, value, data))
}

/// The address of USER32's function `name`.
fn user32(name: &str) -> u32 {
    crate::win32::user32::address_of(name)
}

/// SendMessage(hwnd, msg, wParam, lParam) of 16-bit code: USER32's
/// SendMessageA with the message translated.
pub fn send_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    send32_fn(cpu, "SendMessageA", args);
    None
}

/// DefWindowProc, DefDlgProc and the others that take a message.
pub fn send32_fn(cpu: &mut Cpu, name: &str, args: &[u32]) {
    let target = user32(name);
    send32(cpu, target, args[0], args[1], args[2], args[3]);
}

/// PostMessage and PostAppMessage: USER32's with the message translated
/// (its structures can't be copied, as it returns at once).
pub fn post_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut data = Vec::new();
    let mut stack = Stack32 {
        top: thunk::scratch32(cpu, 0x400),
    };
    let (msg, wp, lp) = to32(
        cpu, args[0], args[1], args[2], args[3], &mut stack, &mut data,
    );
    let target = user32("PostMessageA");
    let ctx = Context::of(cpu);
    thunk::enter32_at(
        cpu,
        ctx,
        stack.top,
        target,
        &[args[0], msg, wp, lp],
        thunk::POST_PLAIN,
        Vec::new(),
    );
    None
}

/// The CREATESTRUCT at `lp` as a CREATESTRUCT16 on `stack`, with its
/// strings: its 16:16 address, or 0 for none.
pub(super) fn create_struct16(cpu: &mut Cpu, lp: u32, stack: &mut Stack16) -> u32 {
    if lp == 0 {
        return 0;
    }
    let (seg, cs16) = stack.alloc(34);
    let name = r32(cpu, lp + 36);
    let class = r32(cpu, lp + 40);
    let name16 = if name == 0 {
        0
    } else {
        string16(cpu, stack, name)
    };
    let class16 = if class < 0x1_0000 {
        class
    } else {
        string16(cpu, stack, class)
    };
    let instance = super::handles::instance16(cpu, r32(cpu, lp + 4));
    let mut params = r32(cpu, lp);
    if r32(cpu, lp + 44) & WS_EX_MDICHILD != 0 && params != 0 {
        params = mdi_create16(cpu, stack, params);
    }
    w32(cpu, cs16, params);
    w16(cpu, cs16 + 4, instance as u32);
    let menu = menu16(r32(cpu, lp + 8)) as u32;
    w16(cpu, cs16 + 6, menu);
    for (i, at) in [12u32, 16, 20, 24, 28].into_iter().enumerate() {
        let v = r32(cpu, lp + at);
        w16(cpu, cs16 + 8 + 2 * i as u32, v);
    }
    let style = r32(cpu, lp + 32);
    w32(cpu, cs16 + 18, style);
    w32(cpu, cs16 + 22, name16);
    w32(cpu, cs16 + 26, class16);
    let ex = r32(cpu, lp + 44) & !super::user::WS_EX_WINDOWEDGE;
    w32(cpu, cs16 + 30, ex);
    seg
}

/// A MSG16 at `m16` as a MSG at `m`, the message translated as one
/// posted. The structures a message takes go on the 32-bit stack below
/// `m`: the stack's top below them.
pub fn msg32(cpu: &mut Cpu, m16: u32, m: u32) -> u32 {
    let hwnd = r16(cpu, m16);
    let msg = r16(cpu, m16 + 2);
    let wp = r16(cpu, m16 + 4);
    let lp = r32(cpu, m16 + 6);
    let mut data = Vec::new();
    let mut stack = Stack32 { top: m };
    let (msg, wp, lp) = to32(cpu, hwnd, msg, wp, lp, &mut stack, &mut data);
    let time = r32(cpu, m16 + 10);
    let x = sx(r16(cpu, m16 + 14));
    let y = sx(r16(cpu, m16 + 16));
    for (i, v) in [hwnd, msg, wp, lp, time, x, y].into_iter().enumerate() {
        w32(cpu, m + 4 * i as u32, v);
    }
    stack.top
}

/// A MSG at `m` as a MSG16 at `m16`.
pub fn msg16(cpu: &mut Cpu, m: u32, m16: u32) {
    let hwnd = r32(cpu, m);
    let msg = r32(cpu, m + 4);
    let wp = r32(cpu, m + 8);
    let lp = r32(cpu, m + 12);
    let (msg, wp, lp) = posted16(cpu, hwnd, msg, wp, lp);
    w16(cpu, m16, hwnd);
    w16(cpu, m16 + 2, msg);
    w16(cpu, m16 + 4, wp);
    w32(cpu, m16 + 6, lp);
    let time = r32(cpu, m + 16);
    w32(cpu, m16 + 10, time);
    let x = r32(cpu, m + 20);
    w16(cpu, m16 + 14, x);
    let y = r32(cpu, m + 24);
    w16(cpu, m16 + 16, y);
}

/// A posted message (no structures) as 16-bit code gets it.
pub(super) fn posted16(
    cpu: &mut Cpu,
    hwnd: u32,
    msg: u32,
    wp: u32,
    lp: u32,
) -> (u32, u32, u32) {
    match msg {
        WM_COMMAND | WM_ACTIVATE | WM_CHARTOITEM | WM_VKEYTOITEM | WM_HSCROLL | WM_VSCROLL
        | WM_MENUSELECT | WM_MENUCHAR | WM_INITMENU | WM_INITMENUPOPUP | WM_PARENTNOTIFY => {
            let mut stack = Stack16 {
                ss: 0,
                base: 0,
                sp: 0,
            };
            let mut data = Vec::new();
            to16(cpu, hwnd, msg, wp, lp, &mut stack, &mut data)
        }
        // The wave header played: the program's own.
        MM_WOM_DONE => (msg, lo(wp), super::misc::wave_header16(cpu, lp)),
        _ => (control_msg16(cpu, hwnd, msg), lo(wp), lp),
    }
}

const MM_WOM_DONE: u32 = 0x3BD;
