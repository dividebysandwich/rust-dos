//! DefWindowProcA: what a window does with a message its procedure
//! leaves, as Windows 95 does it. The non-client area is drawn here: the
//! frame, the caption and its buttons, the menu bar (menu.rs), the edges
//! and the scroll bars (controls/scrollbar.rs). A click on the caption's
//! buttons, a move or a resize by the frame runs a loop of its own, as a
//! task that takes the mouse's messages until the button goes up.
//!
//! The desktop window's procedure is here too: it paints the desktop.

use super::input::{
    HTBORDER, HTBOTTOMRIGHT, HTCAPTION, HTCLIENT, HTCLOSE, HTERROR, HTHSCROLL, HTLEFT, HTMAXBUTTON,
    HTMENU, HTMINBUTTON, HTSYSMENU, HTVSCROLL, MA_ACTIVATE,
};
use super::message::{
    WM_ACTIVATE, WM_CANCELMODE, WM_CHARTOITEM, WM_CLOSE, WM_CONTEXTMENU, WM_CTLCOLOR,
    WM_CTLCOLORMSGBOX, WM_CTLCOLORSTATIC, WM_ERASEBKGND, WM_GETDLGCODE, WM_GETICON,
    WM_GETMINMAXINFO, WM_GETTEXT, WM_GETTEXTLENGTH, WM_HELP, WM_ICONERASEBKGND, WM_KEYDOWN,
    WM_KEYUP, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_MOUSEACTIVATE, WM_MOUSEMOVE,
    WM_MOVE, WM_NCACTIVATE, WM_NCCALCSIZE, WM_NCCREATE, WM_NCDESTROY, WM_NCHITTEST,
    WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN, WM_NCPAINT, WM_NCRBUTTONDOWN, WM_NOTIFYFORMAT, WM_PAINT,
    WM_PRINT, WM_QUERYDRAGICON, WM_QUERYENDSESSION, WM_QUERYOPEN, WM_RBUTTONDOWN, WM_SETCURSOR,
    WM_SETICON, WM_SETREDRAW, WM_SETTEXT, WM_SHOWWINDOW, WM_SIZE, WM_SYSCHAR, WM_SYSCOMMAND,
    WM_SYSKEYDOWN, WM_SYSKEYUP, WM_VKEYTOITEM, WM_WINDOWPOSCHANGED, WM_WINDOWPOSCHANGING,
};
use super::paint::{
    self, BF_ADJUST, BF_RECT, DFC_CAPTION, DFCS_CAPTIONCLOSE, DFCS_CAPTIONMAX, DFCS_CAPTIONMIN,
    DFCS_CAPTIONRESTORE, DFCS_INACTIVE, DFCS_PUSHED, EDGE_RAISED, EDGE_SUNKEN,
};
use super::sysparams::{self, metric};
use super::window::{
    self, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_CAPTION, WS_CHILD, WS_DISABLED,
    WS_EX_CLIENTEDGE, WS_EX_STATICEDGE, WS_EX_TOOLWINDOW, WS_HSCROLL, WS_MAXIMIZE, WS_MAXIMIZEBOX,
    WS_MINIMIZE, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU, WS_THICKFRAME, WS_VISIBLE, WS_VSCROLL,
};
use super::{
    DESKTOP, FALSE, State, TRUE, address_of, bytes_at, copy_out, read_words, user, write_words,
};
use crate::cpu::Cpu;
use crate::win32::gdi32::dc::{DCX_CACHE, DCX_CLIPSIBLINGS, DCX_INTERSECTRGN, DCX_WINDOW};
use crate::win32::gdi32::{self, Rect, Region};
use crate::win32::task::{self, Next, Task};

const WM_PAINTICON: u32 = 0x0026;
const WM_ISACTIVEICON: u32 = 0x0035;
const WM_SYNCPAINT: u32 = 0x0088;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_MOUSEFIRST: u32 = 0x0200;
const WM_MOUSELAST: u32 = 0x0209;
const WM_ENTERSIZEMOVE: u32 = 0x0231;
const WM_EXITSIZEMOVE: u32 = 0x0232;

const SC_SIZE: u32 = 0xF000;
const SC_MOVE: u32 = 0xF010;
const SC_MINIMIZE: u32 = 0xF020;
const SC_MAXIMIZE: u32 = 0xF030;
const SC_CLOSE: u32 = 0xF060;
const SC_VSCROLL: u32 = 0xF070;
const SC_HSCROLL: u32 = 0xF080;
const SC_MOUSEMENU: u32 = 0xF090;
const SC_KEYMENU: u32 = 0xF100;
const SC_RESTORE: u32 = 0xF120;

const SW_MINIMIZE: u32 = 6;
const SW_MAXIMIZE: u32 = 3;
const SW_RESTORE: u32 = 9;

const ICON_SMALL: u32 = 0;
const ICON_BIG: u32 = 1;
const ICON_SMALL2: u32 = 2;

const VK_F4: u32 = 0x73;
const VK_F10: u32 = 0x79;
const VK_MENU: u32 = 0x12;
const VK_LMENU: u32 = 0xA4;
const VK_RMENU: u32 = 0xA5;
const VK_ESCAPE: u32 = 0x1B;

/// The KF_ALTDOWN bit of a key message's lParam.
const KEYDATA_ALT: u32 = 1 << 29;

/// Window::flags: its caption is drawn active (WM_NCACTIVATE).
const NC_ACTIVE: u32 = 0x80;

const BDR_SUNKENOUTER: u32 = 0x0002;

const CTLCOLOR_EDIT: u32 = 1;
const CTLCOLOR_LISTBOX: u32 = 2;
const CTLCOLOR_SCROLLBAR: u32 = 5;

const TASK_DEFWND: u16 = 7;

/// The kinds of DefWindowProcA's task.
const K_SCRIPT: u32 = 0;
const K_PAINT: u32 = 1;
const K_POS_CHANGING: u32 = 2;
const K_TRACK_BUTTON: u32 = 3;
const K_MOVE_SIZE: u32 = 4;
const K_SET_CURSOR: u32 = 5;
const K_SYS_COMMAND: u32 = 6;

/// What a script returns: a fixed value, what its last call returned, or
/// that unless it is 0.
const R_FIXED: u32 = 0;
const R_LAST: u32 = 1;
const R_LAST_OR: u32 = 2;

/// Make the calls `calls` (a function and its arguments each) one after
/// the other, then return what `mode` says.
fn script(cpu: &mut Cpu, mode: u32, fixed: u32, calls: &[(u32, Vec<u32>)]) -> Option<u32> {
    let mut vars = vec![K_SCRIPT, mode, fixed, 4];
    for (target, args) in calls {
        vars.push(*target);
        vars.push(args.len() as u32);
        vars.extend(args);
    }
    task::start(cpu, TASK_DEFWND, vars, 0)
}

fn send_call(hwnd: u32, msg: u32, wp: u32, lp: u32) -> (u32, Vec<u32>) {
    (address_of("SendMessageA"), vec![hwnd, msg, wp, lp])
}

fn call(name: &str, args: &[u32]) -> (u32, Vec<u32>) {
    (address_of(name), args.to_vec())
}

fn script_step(_cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let at = t.var(3) as usize;
    if at < t.vars.len() {
        let target = t.var(at);
        let n = t.var(at + 1) as usize;
        let args: Vec<u32> = (0..n).map(|i| t.var(at + 2 + i)).collect();
        t.set(3, (at + 2 + n) as u32);
        return Next::call(target, &args, 1);
    }
    Next::Done(match t.var(1) {
        R_LAST => value,
        R_LAST_OR if value != 0 => value,
        _ => t.var(2),
    })
}

/// DefWindowProcA's task.
pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    match t.var(0) {
        K_SCRIPT => script_step(cpu, t, value),
        K_PAINT => paint_step(cpu, t, value),
        K_POS_CHANGING => pos_changing_step(cpu, t, value),
        K_TRACK_BUTTON => track_button_step(cpu, t, value),
        K_MOVE_SIZE => move_size_step(cpu, t, value),
        K_SYS_COMMAND => sys_command_step(t, value),
        _ => set_cursor_step(cpu, t, value),
    }
}

fn style_of(state: &State, hwnd: u32) -> u32 {
    state.window(hwnd).map_or(0, |w| w.style)
}

/// DefWindowProcA(hwnd, message, wParam, lParam).
pub fn def_window_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, msg, wp, lp) = (args[0], args[1], args[2], args[3]);
    if !user(cpu).is_window(hwnd) {
        return Some(0);
    }
    match msg {
        WM_NCCREATE => {
            if lp != 0 {
                let name = cpu.bus.guest_read_32(lp + 36);
                if name >= 0x1_0000 {
                    let text = bytes_at(cpu, name)?;
                    if let Some(w) = user(cpu).window_mut(hwnd) {
                        w.text = text;
                    }
                }
                return Some(TRUE);
            }
            Some(0)
        }
        WM_NCCALCSIZE => {
            if lp != 0 {
                let r = Rect::read(cpu, lp);
                let client = window::client_for(user(cpu), hwnd, r);
                client.write(cpu, lp);
            }
            Some(0)
        }
        WM_NCHITTEST => {
            let (x, y) = super::point_of(lp);
            Some(super::input::hit_test(user(cpu), hwnd, x, y))
        }
        WM_NCPAINT => {
            nc_paint(cpu, hwnd, wp);
            Some(0)
        }
        WM_NCACTIVATE => {
            let state = user(cpu);
            if let Some(w) = state.window_mut(hwnd) {
                if wp != 0 {
                    w.flags |= NC_ACTIVE;
                } else {
                    w.flags &= !NC_ACTIVE;
                }
            }
            if state.is_visible(hwnd) {
                nc_paint(cpu, hwnd, 1);
            }
            Some(TRUE)
        }
        WM_NCDESTROY => {
            if let Some(w) = user(cpu).window_mut(hwnd) {
                w.text.clear();
            }
            Some(0)
        }
        WM_GETTEXTLENGTH => Some(user(cpu).window(hwnd).map_or(0, |w| w.text.len() as u32)),
        WM_GETTEXT => {
            if wp == 0 {
                return Some(0);
            }
            let text = user(cpu)
                .window(hwnd)
                .map(|w| w.text.clone())
                .unwrap_or_default();
            Some(copy_out(cpu, lp, wp, &text))
        }
        WM_SETTEXT => {
            let text = if lp == 0 {
                Vec::new()
            } else {
                bytes_at(cpu, lp)?
            };
            let state = user(cpu);
            if let Some(w) = state.window_mut(hwnd) {
                w.text = text;
            }
            if style_of(state, hwnd) & WS_CAPTION == WS_CAPTION && state.is_visible(hwnd) {
                nc_paint(cpu, hwnd, 1);
            }
            Some(TRUE)
        }
        WM_WINDOWPOSCHANGING => {
            let style = style_of(user(cpu), hwnd);
            let flags = cpu.bus.guest_read_32(lp + 24);
            // As Wine and Windows have it: an overlapped window asks even
            // when it isn't sized.
            if (flags & SWP_NOSIZE == 0 && style & WS_THICKFRAME != 0)
                || style & (WS_POPUP | WS_CHILD) == 0
            {
                return task::start(cpu, TASK_DEFWND, vec![K_POS_CHANGING, hwnd, lp], 40);
            }
            Some(0)
        }
        WM_WINDOWPOSCHANGED => {
            let flags = cpu.bus.guest_read_32(lp + 24);
            let state = user(cpu);
            let Some(w) = state.window(hwnd) else {
                return Some(0);
            };
            let client = w.client;
            let mut calls = Vec::new();
            if flags & window::SWP_NOCLIENTMOVE == 0 {
                let pos = super::make_long(client.left, client.top);
                calls.push(send_call(hwnd, WM_MOVE, 0, pos));
            }
            if flags & window::SWP_NOCLIENTSIZE == 0 || flags & window::SWP_STATECHANGED != 0 {
                let (wp, size) = if w.style & WS_MINIMIZE != 0 {
                    (1, 0)
                } else {
                    let wp = if w.style & WS_MAXIMIZE != 0 { 2 } else { 0 };
                    (wp, super::make_long(client.width(), client.height()))
                };
                calls.push(send_call(hwnd, WM_SIZE, wp, size));
            }
            if calls.is_empty() {
                return Some(0);
            }
            script(cpu, R_FIXED, 0, &calls)
        }
        WM_GETMINMAXINFO => Some(0),
        WM_PAINT | WM_PAINTICON => {
            let state = user(cpu);
            let icon = match state.window(hwnd) {
                Some(w) if w.style & WS_MINIMIZE != 0 => {
                    super::class::of(state, hwnd).map_or(0, |c| c.icon)
                }
                _ => 0,
            };
            task::start(cpu, TASK_DEFWND, vec![K_PAINT, hwnd, icon], 64)
        }
        WM_SYNCPAINT => {
            let flags = paint::RDW_ERASENOW | paint::RDW_ERASE | paint::RDW_ALLCHILDREN;
            paint::redraw_window(cpu, &[hwnd, 0, 0, flags])
        }
        WM_SETREDRAW => {
            if wp != 0 {
                if let Some(w) = user(cpu).window_mut(hwnd) {
                    w.style |= WS_VISIBLE;
                }
                paint::tree_changed(cpu);
                return Some(0);
            }
            let flags = paint::RDW_ALLCHILDREN | paint::RDW_VALIDATE;
            paint::redraw_window(cpu, &[hwnd, 0, 0, flags]);
            if let Some(w) = user(cpu).window_mut(hwnd) {
                w.style &= !WS_VISIBLE;
            }
            paint::tree_changed(cpu);
            Some(0)
        }
        WM_ERASEBKGND | WM_ICONERASEBKGND => {
            let state = user(cpu);
            let Some(class) = super::class::of(state, hwnd) else {
                return Some(0);
            };
            let (brush, class_style) = (class.background, class.style);
            if brush == 0 {
                return Some(0);
            }
            let hdc = wp;
            let r = if class_style & super::class::CS_PARENTDC != 0 {
                let c = state.window(hwnd).map(|w| w.client).unwrap_or_default();
                Rect::new(0, 0, c.width(), c.height())
            } else {
                clip_box(cpu, hdc)
            };
            paint::fill(cpu, hdc, r, brush);
            Some(1)
        }
        WM_CLOSE => script(cpu, R_FIXED, 0, &[call("DestroyWindow", &[hwnd])]),
        WM_MOUSEACTIVATE => {
            let state = user(cpu);
            if style_of(state, hwnd) & WS_CHILD != 0 {
                let parent = state.parent(hwnd);
                let ask = send_call(parent, WM_MOUSEACTIVATE, wp, lp);
                return script(cpu, R_LAST_OR, MA_ACTIVATE, &[ask]);
            }
            Some(MA_ACTIVATE)
        }
        WM_ACTIVATE => {
            if wp & 0xFFFF != 0 && style_of(user(cpu), hwnd) & WS_MINIMIZE == 0 {
                return script(cpu, R_FIXED, 0, &[call("SetFocus", &[hwnd])]);
            }
            Some(0)
        }
        WM_GETDLGCODE => Some(0),
        m if (WM_CTLCOLORMSGBOX..=WM_CTLCOLORSTATIC).contains(&m) => {
            Some(control_color(cpu, hwnd, wp, m - WM_CTLCOLORMSGBOX))
        }
        WM_CTLCOLOR => Some(control_color(cpu, hwnd, wp, lp >> 16)),
        WM_SETCURSOR => {
            let state = user(cpu);
            let hit = lp & 0xFFFF;
            let sizing = (super::input::HTLEFT..=HTBOTTOMRIGHT).contains(&hit);
            let parent = state.parent(hwnd);
            if style_of(state, hwnd) & WS_CHILD != 0 && !sizing && parent != DESKTOP {
                return task::start(
                    cpu,
                    TASK_DEFWND,
                    vec![K_SET_CURSOR, hwnd, wp, lp, parent],
                    0,
                );
            }
            Some(set_cursor_for(cpu, hwnd, lp))
        }
        WM_SYSCOMMAND => {
            // The CBT hook may refuse the command.
            if super::hook::has(cpu, window::WH_CBT) {
                let vars = vec![K_SYS_COMMAND, hwnd, wp, lp];
                return task::start(cpu, TASK_DEFWND, vars, 0);
            }
            sys_command(cpu, hwnd, wp, lp)
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
            user(cpu).input.menu_keys = 0;
            Some(0)
        }
        WM_NCLBUTTONDOWN => nc_button_down(cpu, hwnd, wp, lp),
        WM_LBUTTONDBLCLK => nc_double_click(cpu, hwnd, HTCLIENT, lp),
        WM_NCLBUTTONDBLCLK => nc_double_click(cpu, hwnd, wp, lp),
        WM_NCRBUTTONDOWN => {
            if wp == HTCAPTION {
                return script(cpu, R_FIXED, 0, &[call("SetCapture", &[hwnd])]);
            }
            Some(0)
        }
        WM_RBUTTONUP => {
            let state = user(cpu);
            let mut calls = Vec::new();
            if state.input.capture == hwnd {
                calls.push(call("ReleaseCapture", &[]));
            }
            let (x, y) = super::point_of(lp);
            let (cx, cy) = state.client_origin(hwnd);
            let pt = super::make_long(x + cx, y + cy);
            calls.push(send_call(hwnd, WM_CONTEXTMENU, hwnd, pt));
            script(cpu, R_FIXED, 0, &calls)
        }
        WM_CONTEXTMENU => {
            let state = user(cpu);
            if style_of(state, hwnd) & WS_CHILD != 0 {
                let parent = state.parent(hwnd);
                return script(cpu, R_FIXED, 0, &[send_call(parent, msg, wp, lp)]);
            }
            let has_menu = state
                .window(hwnd)
                .is_some_and(|w| w.style & WS_SYSMENU != 0);
            let (x, y) = super::point_of(lp);
            let hit = super::input::hit_test(state, hwnd, x, y);
            if has_menu && (hit == HTCAPTION || hit == HTSYSMENU) {
                let menu = super::menu::get_system_menu(cpu, &[hwnd, FALSE]).unwrap_or(0);
                if menu != 0 {
                    let args = [menu, 0x0002, x as u32, y as u32, 0, hwnd, 0];
                    return script(cpu, R_FIXED, 0, &[call("TrackPopupMenu", &args)]);
                }
            }
            Some(0)
        }
        WM_KEYDOWN => {
            if wp == VK_F10 {
                user(cpu).input.menu_keys = 2;
            }
            Some(0)
        }
        WM_SYSKEYDOWN => sys_key_down(cpu, hwnd, wp, lp),
        WM_KEYUP | WM_SYSKEYUP => {
            let keys = std::mem::take(&mut user(cpu).input.menu_keys);
            let alt = matches!(wp, VK_MENU | VK_LMENU | VK_RMENU);
            if (alt && keys & 1 != 0) || (wp == VK_F10 && keys & 2 != 0) {
                let top = user(cpu).top_level(hwnd);
                return script(
                    cpu,
                    R_FIXED,
                    0,
                    &[send_call(top, WM_SYSCOMMAND, SC_KEYMENU, 0)],
                );
            }
            Some(0)
        }
        WM_SYSCHAR => sys_char(cpu, hwnd, wp, lp),
        WM_SHOWWINDOW | WM_PRINT => Some(0),
        WM_CANCELMODE => {
            user(cpu).input.menu_keys = 0;
            if user(cpu).input.capture == hwnd {
                return script(cpu, R_FIXED, 0, &[call("ReleaseCapture", &[])]);
            }
            Some(0)
        }
        WM_VKEYTOITEM | WM_CHARTOITEM => Some(-1i32 as u32),
        WM_QUERYDRAGICON => {
            let state = user(cpu);
            let icon = super::class::of(state, hwnd).map_or(0, |c| c.icon);
            if icon != 0 {
                return Some(icon);
            }
            super::resource::load_icon(cpu, &[0, 32512])
        }
        WM_ISACTIVEICON => {
            let state = user(cpu);
            Some(state.window(hwnd).is_some_and(|w| w.flags & NC_ACTIVE != 0) as u32)
        }
        WM_NOTIFYFORMAT => Some(1),
        WM_QUERYOPEN | WM_QUERYENDSESSION => Some(1),
        WM_SETICON => {
            let state = user(cpu);
            let Some(w) = state.window_mut(hwnd) else {
                return Some(0);
            };
            let old = match wp {
                ICON_SMALL => std::mem::replace(&mut w.icons[1], lp),
                ICON_BIG => std::mem::replace(&mut w.icons[0], lp),
                _ => 0,
            };
            if w.style & WS_CAPTION == WS_CAPTION && state.is_visible(hwnd) {
                nc_paint(cpu, hwnd, 1);
            }
            Some(old)
        }
        WM_GETICON => {
            let state = user(cpu);
            let Some(w) = state.window(hwnd) else {
                return Some(0);
            };
            Some(match wp {
                ICON_SMALL => w.icons[1],
                ICON_BIG => w.icons[0],
                ICON_SMALL2 if w.icons[1] != 0 => w.icons[1],
                ICON_SMALL2 => super::class::of(state, hwnd).map_or(0, |c| c.icon_small),
                _ => 0,
            })
        }
        WM_HELP => {
            let parent = user(cpu).parent(hwnd);
            script(cpu, R_FIXED, 0, &[send_call(parent, msg, wp, lp)])
        }
        _ => Some(0),
    }
}

/// The bounds of what `hdc` may draw on, in logical units.
fn clip_box(cpu: &mut Cpu, hdc: u32) -> Rect {
    let Some(dc) = gdi32::gdi(cpu).dc(hdc).cloned() else {
        return Rect::default();
    };
    let (region, _) = gdi32::dc::effective_clip(cpu, &dc);
    let b = region.bounds();
    let (l, t) = dc.to_logical(b.left, b.top);
    let (r, bt) = dc.to_logical(b.right, b.bottom);
    Rect::new(l, t, r, bt).ordered()
}

/// The colors a control of type `kind` (CTLCOLOR_*) is drawn with: they
/// go to `hdc`, and the brush is the result. As on Windows 95, the window
/// `hwnd` of a Windows 3.x program gets Windows 3.1's colors, the
/// window's for every kind but scroll bars, so its dialogs are white.
pub(crate) fn control_color(cpu: &mut Cpu, hwnd: u32, hdc: u32, kind: u32) -> u32 {
    let state = user(cpu);
    if kind == CTLCOLOR_SCROLLBAR {
        let (text, back) = (state.color(sysparams::COLOR_BTNFACE), state.color(20));
        gdi32::dc::set_text_color(cpu, &[hdc, text]);
        gdi32::dc::set_bk_color(cpu, &[hdc, back]);
        return sysparams::brush(cpu, sysparams::COLOR_SCROLLBAR);
    }
    let text = state.color(sysparams::COLOR_WINDOWTEXT);
    gdi32::dc::set_text_color(cpu, &[hdc, text]);
    let owner = user(cpu).window(hwnd).map(|w| (w.pid, w.instance));
    let win3 = || owner.is_some_and(|(pid, inst)| crate::win16::expects_win3(cpu, pid, inst));
    let index = if kind == CTLCOLOR_EDIT || kind == CTLCOLOR_LISTBOX || win3() {
        sysparams::COLOR_WINDOW
    } else {
        sysparams::COLOR_BTNFACE
    };
    let back = user(cpu).color(index);
    gdi32::dc::set_bk_color(cpu, &[hdc, back]);
    sysparams::brush(cpu, index)
}

/// WM_SETCURSOR's own part: the class's cursor in the client area, the
/// arrow elsewhere. Whether it set one.
fn set_cursor_for(cpu: &mut Cpu, hwnd: u32, lp: u32) -> u32 {
    let hit = lp & 0xFFFF;
    let cursor = match hit {
        HTCLIENT => {
            let c = super::class::of(user(cpu), hwnd).map_or(0, |c| c.cursor);
            if c == 0 {
                return FALSE;
            }
            c
        }
        h if h == HTERROR & 0xFFFF => return FALSE,
        h if (HTLEFT..=HTBOTTOMRIGHT).contains(&h) => {
            // IDC_SIZEWE, IDC_SIZENS, IDC_SIZENWSE, IDC_SIZENESW.
            let id = match h {
                10 | 11 => 32644,
                12 | 15 => 32645,
                13 | 17 => 32642,
                _ => 32643,
            };
            super::resource::system_cursor(cpu, id)
        }
        _ => super::resource::system_cursor(cpu, super::resource::IDC_ARROW),
    };
    super::cursor::set(cpu, cursor);
    TRUE
}

fn sys_command_step(t: &mut Task, value: u32) -> Next {
    let (hwnd, wp, lp) = (t.var(1), t.var(2), t.var(3));
    match t.step {
        0 => super::hook::call(window::WH_CBT, HCBT_SYSCOMMAND, wp, lp, 1),
        1 if value == 0 => Next::call(address_of("DefSysCommand"), &[hwnd, wp, lp], 2),
        1 => Next::Done(0),
        _ => Next::Done(value),
    }
}

/// HCBT_SYSCOMMAND, the CBT hook's code for WM_SYSCOMMAND.
const HCBT_SYSCOMMAND: u32 = 8;

/// DefSysCommand(hwnd, wParam, lParam): USER's own function, the
/// WM_SYSCOMMAND of DefWindowProcA after the CBT hook allowed it.
pub fn def_sys_command(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    sys_command(cpu, args[0], args[1], args[2])
}

fn set_cursor_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let (hwnd, wp, lp, parent) = (t.var(1), t.var(2), t.var(3), t.var(4));
    match t.step {
        0 => window::send(parent, WM_SETCURSOR, wp, lp, 1),
        _ => {
            if value != 0 {
                return Next::Done(TRUE);
            }
            Next::Done(set_cursor_for(cpu, hwnd, lp))
        }
    }
}

// The non-client area.

/// The caption's buttons of `hwnd` that it has: their hit test codes
/// and rectangles in window coordinates.
fn caption_buttons(state: &State, hwnd: u32) -> Vec<(u32, Rect)> {
    let Some(w) = state.window(hwnd) else {
        return Vec::new();
    };
    let (style, ex) = (w.style, w.ex_style);
    if style & WS_CAPTION != WS_CAPTION || style & WS_SYSMENU == 0 {
        return Vec::new();
    }
    let Some(cap) = caption_rect(state, hwnd) else {
        return Vec::new();
    };
    let tool = ex & WS_EX_TOOLWINDOW != 0;
    let (bw, bh) = if tool {
        (metric(52) - 2, metric(53) - 4)
    } else {
        (metric(30) - 2, metric(31) - 4)
    };
    let top = cap.top + 2;
    let mut right = cap.right - 2;
    let mut buttons = vec![(HTCLOSE, Rect::new(right - bw, top, right, top + bh))];
    right -= bw + 2;
    if !tool && style & (WS_MAXIMIZEBOX | WS_MINIMIZEBOX) != 0 {
        buttons.push((HTMAXBUTTON, Rect::new(right - bw, top, right, top + bh)));
        right -= bw;
        buttons.push((HTMINBUTTON, Rect::new(right - bw, top, right, top + bh)));
    }
    buttons
}

/// The frame's width and height, as AdjustWindowRectEx counts them.
fn frame_size(style: u32, ex: u32) -> (i32, i32) {
    if window::has_thick_frame(style, ex) {
        (metric(32), metric(33))
    } else if window::has_dlg_frame(style, ex) {
        (metric(7), metric(8))
    } else if window::has_thin_frame(style) {
        (metric(5), metric(6))
    } else {
        (0, 0)
    }
}

/// The caption's rectangle in window coordinates, with the line under
/// it.
fn caption_rect(state: &State, hwnd: u32) -> Option<Rect> {
    let w = state.window(hwnd)?;
    let (style, ex) = (w.style, w.ex_style);
    if style & WS_CAPTION != WS_CAPTION {
        return None;
    }
    let r = Rect::new(0, 0, w.rect.width(), w.rect.height());
    let (fx, fy) = frame_size(style, ex);
    let mut r = r.inflate(-fx, -fy);
    if ex & WS_EX_STATICEDGE != 0 {
        r = r.inflate(-metric(5), -metric(6));
    }
    let height = if ex & WS_EX_TOOLWINDOW != 0 {
        metric(51)
    } else {
        metric(4)
    };
    Some(Rect::new(r.left, r.top, r.right, r.top + height))
}

/// Draw the caption button `hit` of `hwnd`, up or pushed.
fn draw_caption_button(cpu: &mut Cpu, hwnd: u32, hdc: u32, hit: u32, r: Rect, pushed: bool) {
    let state = user(cpu);
    let style = style_of(state, hwnd);
    let no_close = super::class::style_of(state, hwnd) & super::class::CS_NOCLOSE != 0;
    let mut kind = match hit {
        HTCLOSE => DFCS_CAPTIONCLOSE | if no_close { DFCS_INACTIVE } else { 0 },
        HTMAXBUTTON => {
            let k = if style & WS_MAXIMIZE != 0 {
                DFCS_CAPTIONRESTORE
            } else {
                DFCS_CAPTIONMAX
            };
            k | if style & WS_MAXIMIZEBOX == 0 {
                DFCS_INACTIVE
            } else {
                0
            }
        }
        _ => {
            let k = if style & WS_MINIMIZE != 0 {
                DFCS_CAPTIONRESTORE
            } else {
                DFCS_CAPTIONMIN
            };
            k | if style & WS_MINIMIZEBOX == 0 {
                DFCS_INACTIVE
            } else {
                0
            }
        }
    };
    if pushed {
        kind |= DFCS_PUSHED;
    }
    paint::frame_control(cpu, hdc, r, DFC_CAPTION, kind);
}

/// A DC of the whole window that draws only on its non-client area,
/// within the region `rgn` (1 for all of it) on the screen; 0 if there
/// is nothing to draw.
fn nc_dc(cpu: &mut Cpu, hwnd: u32, rgn: u32) -> u32 {
    let state = user(cpu);
    if !state.is_visible(hwnd) {
        return 0;
    }
    let screen = state.screen_rect(hwnd);
    let client = state.screen_client(hwnd);
    let mut area = Region::from_rect(screen).diff_rect(&client);
    if rgn > 1
        && let Some(r) = gdi32::region_of(cpu, rgn)
    {
        area = area.and(&r);
    }
    if area.is_empty() {
        return 0;
    }
    let flags = DCX_WINDOW | DCX_CACHE | DCX_CLIPSIBLINGS | DCX_INTERSECTRGN;
    paint::window_dc(cpu, hwnd, flags, Some(area))
}

/// Draw the non-client area of `hwnd` where the region `rgn` (screen
/// coordinates; 1 for all) is: WM_NCPAINT.
pub fn nc_paint(cpu: &mut Cpu, hwnd: u32, rgn: u32) {
    if hwnd == DESKTOP {
        return;
    }
    let Some(w) = user(cpu).window(hwnd).cloned() else {
        return;
    };
    let hdc = nc_dc(cpu, hwnd, rgn);
    if hdc == 0 {
        return;
    }
    let (style, ex) = (w.style, w.ex_style);
    let active = w.flags & NC_ACTIVE != 0;
    let m = metric;
    let mut r = Rect::new(0, 0, w.rect.width(), w.rect.height());
    // The frame.
    let thick = window::has_thick_frame(style, ex);
    if thick || window::has_dlg_frame(style, ex) {
        r = paint::edge(cpu, hdc, r, EDGE_RAISED, BF_RECT | BF_ADJUST).1;
        let (fx, fy) = frame_size(style, ex);
        let (bx, by) = (fx - m(45), fy - m(46));
        let color = if !thick {
            sysparams::COLOR_BTNFACE
        } else if active {
            sysparams::COLOR_ACTIVEBORDER
        } else {
            11
        };
        if bx > 0 || by > 0 {
            let inner = r.inflate(-bx, -by);
            fill_band(cpu, hdc, r, inner, color);
            r = inner;
        }
    } else if window::has_thin_frame(style) {
        let inner = r.inflate(-m(5), -m(6));
        fill_band(cpu, hdc, r, inner, sysparams::COLOR_WINDOWFRAME);
        r = inner;
    }
    if ex & WS_EX_STATICEDGE != 0 {
        r = paint::edge(cpu, hdc, r, BDR_SUNKENOUTER, BF_RECT | BF_ADJUST).1;
    }
    // The caption.
    if style & WS_CAPTION == WS_CAPTION {
        let tool = ex & WS_EX_TOOLWINDOW != 0;
        let height = if tool { m(51) } else { m(4) };
        let cap = Rect::new(r.left, r.top, r.right, r.top + height);
        r.top += height;
        // The line under it.
        let line = Rect::new(cap.left, cap.bottom - 1, cap.right, cap.bottom);
        paint::fill_color(cpu, hdc, line, sysparams::COLOR_BTNFACE);
        let bar = Rect::new(cap.left, cap.top, cap.right, cap.bottom - 1);
        let color = if active {
            sysparams::COLOR_ACTIVECAPTION
        } else {
            sysparams::COLOR_INACTIVECAPTION
        };
        paint::fill_color(cpu, hdc, bar, color);
        let buttons = caption_buttons(user(cpu), hwnd);
        let text_right = buttons.iter().map(|b| b.1.left).min().unwrap_or(bar.right);
        let mut flags = paint::DC_TEXT;
        if active {
            flags |= paint::DC_ACTIVE;
        }
        if tool {
            flags |= paint::DC_SMALLCAP;
        } else if style & WS_SYSMENU != 0 {
            flags |= paint::DC_ICON;
        }
        let text = Rect::new(bar.left, bar.top, text_right, bar.bottom);
        paint::caption(cpu, hwnd, hdc, text, flags);
        for (hit, br) in buttons {
            draw_caption_button(cpu, hwnd, hdc, hit, br, false);
        }
    }
    // The menu bar.
    if style & WS_CHILD == 0 && w.id != 0 && style & WS_MINIMIZE == 0 {
        let bar = Rect::new(r.left, r.top, r.right, r.top + m(15));
        r.top += m(15);
        super::menu::draw_bar(cpu, hwnd, hdc, bar);
    }
    if ex & WS_EX_CLIENTEDGE != 0 {
        r = paint::edge(cpu, hdc, r, EDGE_SUNKEN, BF_RECT | BF_ADJUST).1;
    }
    // The scroll bars, and the box where they meet.
    let (vs, hs) = (style & WS_VSCROLL != 0, style & WS_HSCROLL != 0);
    if vs {
        let bottom = if hs { r.bottom - m(3) } else { r.bottom };
        let vr = Rect::new(r.right - m(2), r.top, r.right, bottom);
        super::controls::scrollbar::draw_nc(cpu, hwnd, hdc, 1, vr);
    }
    if hs {
        let right = if vs { r.right - m(2) } else { r.right };
        let hr = Rect::new(r.left, r.bottom - m(3), right, r.bottom);
        super::controls::scrollbar::draw_nc(cpu, hwnd, hdc, 0, hr);
    }
    if vs && hs {
        let corner = Rect::new(r.right - m(2), r.bottom - m(3), r.right, r.bottom);
        paint::fill_color(cpu, hdc, corner, sysparams::COLOR_BTNFACE);
    }
    paint::release_dc(cpu, &[hwnd, hdc]);
}

/// Fill what is between `outer` and `inner` with a system color.
fn fill_band(cpu: &mut Cpu, hdc: u32, outer: Rect, inner: Rect, color: u32) {
    let parts = [
        Rect::new(outer.left, outer.top, outer.right, inner.top),
        Rect::new(outer.left, inner.bottom, outer.right, outer.bottom),
        Rect::new(outer.left, inner.top, inner.left, inner.bottom),
        Rect::new(inner.right, inner.top, outer.right, inner.bottom),
    ];
    for p in parts {
        if !p.is_empty() {
            paint::fill_color(cpu, hdc, p, color);
        }
    }
}

// The mouse on the non-client area, and the system commands.

/// WM_NCLBUTTONDOWN.
fn nc_button_down(cpu: &mut Cpu, hwnd: u32, hit: u32, lp: u32) -> Option<u32> {
    let state = user(cpu);
    let style = style_of(state, hwnd);
    let command = |sc: u32| send_call(hwnd, WM_SYSCOMMAND, sc, lp);
    match hit {
        HTCAPTION => {
            let top = state.top_level(hwnd);
            if state.is_active(top) || top == hwnd {
                return script(cpu, R_FIXED, 0, &[command(SC_MOVE + HTCAPTION)]);
            }
            Some(0)
        }
        HTSYSMENU => {
            if style & WS_SYSMENU != 0 {
                return script(cpu, R_FIXED, 0, &[command(SC_MOUSEMENU + HTSYSMENU)]);
            }
            Some(0)
        }
        HTMENU => script(cpu, R_FIXED, 0, &[command(SC_MOUSEMENU + HTMENU)]),
        HTHSCROLL => script(cpu, R_FIXED, 0, &[command(SC_HSCROLL + HTHSCROLL)]),
        HTVSCROLL => script(cpu, R_FIXED, 0, &[command(SC_VSCROLL + HTVSCROLL)]),
        HTMINBUTTON | HTMAXBUTTON | HTCLOSE => {
            let sc = match hit {
                HTMINBUTTON if style & WS_MINIMIZE != 0 => SC_RESTORE,
                HTMINBUTTON => SC_MINIMIZE,
                HTMAXBUTTON if style & WS_MAXIMIZE != 0 => SC_RESTORE,
                HTMAXBUTTON => SC_MAXIMIZE,
                _ => SC_CLOSE,
            };
            let usable = match hit {
                HTMINBUTTON => style & WS_MINIMIZEBOX != 0,
                HTMAXBUTTON => style & WS_MAXIMIZEBOX != 0,
                _ => super::class::style_of(state, hwnd) & super::class::CS_NOCLOSE == 0,
            };
            if !usable {
                return Some(0);
            }
            let vars = vec![K_TRACK_BUTTON, hwnd, hit, sc, lp, 1];
            task::start(cpu, TASK_DEFWND, vars, 28)
        }
        h if (HTLEFT..=HTBOTTOMRIGHT).contains(&h) => {
            // WMSZ_LEFT and the rest follow the hit test codes' order.
            let sc = SC_SIZE + (h - HTLEFT + 1);
            script(cpu, R_FIXED, 0, &[command(sc)])
        }
        HTBORDER => Some(0),
        _ => Some(0),
    }
}

/// WM_NCLBUTTONDBLCLK, and WM_LBUTTONDBLCLK as HTCLIENT.
fn nc_double_click(cpu: &mut Cpu, hwnd: u32, hit: u32, lp: u32) -> Option<u32> {
    let state = user(cpu);
    let style = style_of(state, hwnd);
    let command = |sc: u32| send_call(hwnd, WM_SYSCOMMAND, sc, lp);
    match hit {
        HTCLIENT if style & WS_MINIMIZE != 0 => script(cpu, R_FIXED, 0, &[command(SC_RESTORE)]),
        HTCAPTION if style & WS_MAXIMIZEBOX != 0 => {
            let sc = if style & (WS_MAXIMIZE | WS_MINIMIZE) != 0 {
                SC_RESTORE
            } else {
                SC_MAXIMIZE
            };
            script(cpu, R_FIXED, 0, &[command(sc)])
        }
        HTSYSMENU => {
            let no_close = super::class::style_of(state, hwnd) & super::class::CS_NOCLOSE != 0;
            if style & WS_SYSMENU != 0 && !no_close {
                return script(cpu, R_FIXED, 0, &[command(SC_CLOSE)]);
            }
            Some(0)
        }
        HTHSCROLL => script(cpu, R_FIXED, 0, &[command(SC_HSCROLL + hit)]),
        HTVSCROLL => script(cpu, R_FIXED, 0, &[command(SC_VSCROLL + hit)]),
        _ => Some(0),
    }
}

/// WM_SYSCOMMAND.
fn sys_command(cpu: &mut Cpu, hwnd: u32, wp: u32, lp: u32) -> Option<u32> {
    let state = user(cpu);
    if style_of(state, hwnd) & WS_DISABLED != 0 {
        return Some(0);
    }
    match wp & 0xFFF0 {
        SC_SIZE | SC_MOVE => {
            // Only with the mouse: a move or size by the keyboard isn't
            // done.
            if lp == 0 {
                return Some(0);
            }
            let w = state.window(hwnd)?;
            if w.style & (WS_MAXIMIZE | WS_MINIMIZE) != 0 && wp & 0xFFF0 == SC_SIZE {
                return Some(0);
            }
            let rect = w.rect;
            let (x, y) = state.input.cursor;
            let edges = if wp & 0xFFF0 == SC_MOVE {
                0
            } else {
                // WMSZ_LEFT 1 ... WMSZ_BOTTOMRIGHT 8: which edges move.
                match wp & 0xF {
                    1 => 1,
                    2 => 4,
                    3 => 2,
                    4 => 3,
                    5 => 6,
                    6 => 8,
                    7 => 9,
                    8 => 12,
                    _ => return Some(0),
                }
            };
            let vars = vec![
                K_MOVE_SIZE,
                hwnd,
                edges,
                x as u32,
                y as u32,
                rect.left as u32,
                rect.top as u32,
                rect.right as u32,
                rect.bottom as u32,
            ];
            task::start(cpu, TASK_DEFWND, vars, 28)
        }
        SC_MINIMIZE => script(cpu, R_FIXED, 0, &[call("ShowWindow", &[hwnd, SW_MINIMIZE])]),
        SC_MAXIMIZE => script(cpu, R_FIXED, 0, &[call("ShowWindow", &[hwnd, SW_MAXIMIZE])]),
        SC_RESTORE => script(cpu, R_FIXED, 0, &[call("ShowWindow", &[hwnd, SW_RESTORE])]),
        SC_CLOSE => script(cpu, R_FIXED, 0, &[send_call(hwnd, WM_CLOSE, 0, 0)]),
        super::saver::SC_SCREENSAVE => {
            super::saver::start(cpu);
            Some(0)
        }
        SC_VSCROLL | SC_HSCROLL => {
            let bar = (wp & 0xFFF0 == SC_VSCROLL) as u32;
            super::controls::scrollbar::track(cpu, &[hwnd, bar, lp])
        }
        SC_MOUSEMENU | SC_KEYMENU => super::menu::track_bar(cpu, &[hwnd, wp, lp]),
        _ => Some(0),
    }
}

/// WM_SYSKEYDOWN.
fn sys_key_down(cpu: &mut Cpu, hwnd: u32, wp: u32, lp: u32) -> Option<u32> {
    let input = &mut user(cpu).input;
    if lp & KEYDATA_ALT != 0 {
        let alt = matches!(wp, VK_MENU | VK_LMENU | VK_RMENU);
        input.menu_keys = (alt && input.menu_keys & 1 == 0) as u32;
        if wp == VK_F4 {
            let top = user(cpu).top_level(hwnd);
            if super::class::style_of(user(cpu), top) & super::class::CS_NOCLOSE == 0 {
                super::message::post_message(cpu, &[top, WM_SYSCOMMAND, SC_CLOSE, 0]);
            }
        }
        return Some(0);
    }
    if wp == VK_F10 {
        input.menu_keys = 2;
        if input.key(0x10) & 0x80 != 0 {
            return script(
                cpu,
                R_FIXED,
                0,
                &[send_call(hwnd, WM_CONTEXTMENU, hwnd, u32::MAX)],
            );
        }
    } else if wp == VK_ESCAPE && input.key(0x10) & 0x80 != 0 {
        return script(
            cpu,
            R_FIXED,
            0,
            &[send_call(hwnd, WM_SYSCOMMAND, SC_KEYMENU, 0x20)],
        );
    }
    Some(0)
}

/// WM_SYSCHAR.
fn sys_char(cpu: &mut Cpu, hwnd: u32, wp: u32, lp: u32) -> Option<u32> {
    let state = user(cpu);
    state.input.menu_keys = 0;
    let style = style_of(state, hwnd);
    if wp == u32::from(b'\r') && style & WS_MINIMIZE != 0 {
        super::message::post_message(cpu, &[hwnd, WM_SYSCOMMAND, SC_RESTORE, 0]);
        return Some(0);
    }
    if lp & KEYDATA_ALT != 0 && wp != 0 {
        if wp == u32::from(b'\t') || wp == 0x1B {
            return Some(0);
        }
        if wp == u32::from(b' ') && style & WS_CHILD != 0 {
            let parent = state.parent(hwnd);
            return script(cpu, R_FIXED, 0, &[send_call(parent, WM_SYSCHAR, wp, lp)]);
        }
        return script(
            cpu,
            R_FIXED,
            0,
            &[send_call(hwnd, WM_SYSCOMMAND, SC_KEYMENU, wp)],
        );
    }
    Some(0)
}

// The steps.

fn paint_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let (hwnd, icon) = (t.var(1), t.var(2));
    let ps = t.scratch;
    match t.step {
        0 => Next::call(address_of("BeginPaint"), &[hwnd, ps], 1),
        1 => {
            let hdc = value;
            if hdc != 0 && icon != 0 {
                let c = user(cpu).window(hwnd).map(|w| w.client).unwrap_or_default();
                let (cx, cy) = (metric(11), metric(12));
                let at = ((c.width() - cx) / 2, (c.height() - cy) / 2);
                paint::draw_icon_at(cpu, hdc, at, icon, (cx, cy), paint::DI_NORMAL);
            }
            Next::call(address_of("EndPaint"), &[hwnd, ps], 2)
        }
        _ => Next::Done(0),
    }
}

fn pos_changing_step(cpu: &mut Cpu, t: &mut Task, _value: u32) -> Next {
    let (hwnd, winpos) = (t.var(1), t.var(2));
    let mm = t.scratch;
    match t.step {
        0 => {
            window::write_min_max(cpu, hwnd, mm);
            window::send(hwnd, WM_GETMINMAXINFO, 0, mm, 1)
        }
        _ => {
            let m = read_words(cpu, mm, 10);
            let (max_x, max_y) = (m[2] as i32, m[3] as i32);
            let (min_x, min_y) = (m[6] as i32, m[7] as i32);
            let p = read_words(cpu, winpos, 7);
            let (mut cx, mut cy) = (p[4] as i32, p[5] as i32);
            cx = cx.min(max_x);
            cy = cy.min(max_y);
            if style_of(user(cpu), hwnd) & WS_MINIMIZE == 0 {
                cx = cx.max(min_x);
                cy = cy.max(min_y);
            }
            write_words(cpu, winpos + 16, &[cx as u32, cy as u32]);
            Next::Done(0)
        }
    }
}

/// The variables of the caption button's loop: the window, the button,
/// the command it gives, the click's lParam and whether it shows pushed.
fn track_button_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let (hwnd, hit, sc, lp) = (t.var(1), t.var(2), t.var(3), t.var(4));
    let msg = t.scratch;
    loop {
        match t.step {
            0 => {
                redraw_button(cpu, hwnd, hit, true);
                return Next::call(address_of("SetCapture"), &[hwnd], 1);
            }
            1 => {
                let args = [msg, 0, WM_MOUSEFIRST, WM_MOUSELAST];
                return Next::call(address_of("GetMessageA"), &args, 2);
            }
            2 => {
                let m = read_words(cpu, msg, 7);
                let (x, y) = (m[5] as i32, m[6] as i32);
                let over = super::input::hit_test(user(cpu), hwnd, x, y) == hit;
                if value == 0 || m[1] == WM_LBUTTONUP {
                    t.set(5, over as u32);
                    t.step = 3;
                    continue;
                }
                if m[1] == WM_MOUSEMOVE && over != (t.var(5) != 0) {
                    t.set(5, over as u32);
                    redraw_button(cpu, hwnd, hit, over);
                }
                t.step = 1;
            }
            3 => return Next::call(address_of("ReleaseCapture"), &[], 4),
            4 => {
                redraw_button(cpu, hwnd, hit, false);
                if t.var(5) != 0 && user(cpu).is_window(hwnd) {
                    return window::send(hwnd, WM_SYSCOMMAND, sc, lp, 5);
                }
                return Next::Done(0);
            }
            _ => return Next::Done(0),
        }
    }
}

fn redraw_button(cpu: &mut Cpu, hwnd: u32, hit: u32, pushed: bool) {
    let Some(&(_, r)) = caption_buttons(user(cpu), hwnd).iter().find(|b| b.0 == hit) else {
        return;
    };
    let hdc = nc_dc(cpu, hwnd, 1);
    if hdc == 0 {
        return;
    }
    draw_caption_button(cpu, hwnd, hdc, hit, r, pushed);
    paint::release_dc(cpu, &[hwnd, hdc]);
}

/// The variables of a move or resize by the mouse: the window, the edges
/// that move (left 1, top 2, right 4, bottom 8; none for a move), where
/// the mouse started, and the window's rectangle then.
fn move_size_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let (hwnd, edges) = (t.var(1), t.var(2));
    let msg = t.scratch;
    loop {
        match t.step {
            0 => return window::send(hwnd, WM_ENTERSIZEMOVE, 0, 0, 1),
            1 => return Next::call(address_of("SetCapture"), &[hwnd], 2),
            2 => {
                let args = [msg, 0, WM_MOUSEFIRST, WM_MOUSELAST];
                return Next::call(address_of("GetMessageA"), &args, 3);
            }
            3 => {
                if !user(cpu).is_window(hwnd) {
                    return Next::Done(0);
                }
                let m = read_words(cpu, msg, 7);
                if value == 0 || m[1] == WM_LBUTTONUP {
                    t.step = 4;
                    continue;
                }
                if m[1] != WM_MOUSEMOVE {
                    t.step = 2;
                    continue;
                }
                let (dx, dy) = (m[5] as i32 - t.var(3) as i32, m[6] as i32 - t.var(4) as i32);
                let old = Rect::new(
                    t.var(5) as i32,
                    t.var(6) as i32,
                    t.var(7) as i32,
                    t.var(8) as i32,
                );
                let mut r = old;
                if edges == 0 {
                    r = old.offset(dx, dy);
                } else {
                    if edges & 1 != 0 {
                        r.left = (old.left + dx).min(old.right - metric(34));
                    }
                    if edges & 2 != 0 {
                        r.top = (old.top + dy).min(old.bottom - metric(35));
                    }
                    if edges & 4 != 0 {
                        r.right = (old.right + dx).max(old.left + metric(34));
                    }
                    if edges & 8 != 0 {
                        r.bottom = (old.bottom + dy).max(old.top + metric(35));
                    }
                }
                let mut flags = SWP_NOZORDER | SWP_NOACTIVATE;
                if edges == 0 {
                    flags |= SWP_NOSIZE;
                }
                let args = [
                    hwnd,
                    0,
                    r.left as u32,
                    r.top as u32,
                    r.width() as u32,
                    r.height() as u32,
                    flags,
                ];
                return Next::call(address_of("SetWindowPos"), &args, 5);
            }
            5 => {
                // Paint what the move showed before the next one.
                return Next::call(address_of("UpdateWindow"), &[hwnd], 2);
            }
            4 => return Next::call(address_of("ReleaseCapture"), &[], 6),
            6 => return window::send(hwnd, WM_EXITSIZEMOVE, 0, 0, 7),
            _ => return Next::Done(0),
        }
    }
}

// The desktop.

/// The desktop window's procedure: it paints the desktop.
pub fn desktop_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, msg, wp) = (args[0], args[1], args[2]);
    match msg {
        WM_ERASEBKGND => {
            paint::paint_desktop(cpu, &[wp]);
            Some(1)
        }
        WM_PAINT => task::start(cpu, TASK_DEFWND, vec![K_PAINT, hwnd, 0], 64),
        WM_NCCREATE => Some(TRUE),
        WM_NCCALCSIZE | WM_NCPAINT | WM_NCACTIVATE | WM_SETCURSOR => Some(TRUE),
        _ => {
            let _ = SWP_NOMOVE;
            Some(0)
        }
    }
}
