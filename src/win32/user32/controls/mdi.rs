//! The MDIClient class, DefFrameProcA, DefMDIChildProcA and
//! CreateMDIWindowA, as Wine's mdi.c has them.
//!
//! The MDI functions call one function after another into the program
//! and USER32 (SendMessageA, SetWindowPos, the menu functions), with
//! what comes next often depending on the state those calls leave. They
//! run as a list of operations in a task: an operation is a call, or a
//! step of the MDI code here, which looks at the state as it is when it
//! runs and puts the operations it calls for in front of the rest.

use super::button::parent_of;
use super::scrollbar::{SB_BOTH, SB_HORZ, SB_VERT};
use super::{MDI, start};
use crate::cpu::Cpu;
use crate::win32::gdi32::{self, Rect};
use crate::win32::task::{Next, Task};
use crate::win32::user32::message::{
    self, SC_CLOSE, SC_KEYMENU, SC_MAXIMIZE, SC_MINIMIZE, SC_MOVE, SC_RESTORE, SC_SIZE,
    SIZE_MAXIMIZED, SIZE_MINIMIZED, WM_CHILDACTIVATE, WM_CLOSE, WM_COMMAND, WM_CREATE, WM_DESTROY,
    WM_GETMINMAXINFO, WM_HSCROLL, WM_LBUTTONDOWN, WM_MDIACTIVATE, WM_MDICREATE, WM_MDIDESTROY,
    WM_MDIGETACTIVE, WM_MENUCHAR, WM_NCACTIVATE, WM_NCCREATE, WM_PARENTNOTIFY, WM_SETFOCUS,
    WM_SETREDRAW, WM_SETTEXT, WM_SHOWWINDOW, WM_SIZE, WM_SYSCHAR, WM_SYSCOMMAND, WM_VSCROLL,
};
use crate::win32::user32::paint::{self, DI_NORMAL};
use crate::win32::user32::sysparams::{self, COLOR_MENU};
use crate::win32::user32::window::{
    self, CW_USEDEFAULT, HWND_TOP, SW_HIDE, SW_MAXIMIZE, SW_RESTORE, SW_SHOWNOACTIVATE,
    SW_SHOWNORMAL, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    WS_CAPTION, WS_CHILD, WS_CLIPSIBLINGS, WS_DISABLED, WS_EX_MDICHILD, WS_MAXIMIZE,
    WS_MAXIMIZEBOX, WS_MINIMIZE, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU, WS_THICKFRAME, WS_VISIBLE,
};
use crate::win32::user32::{address_of, class, defwnd, read_words, resource, user, write_words};

/// What an MDI client keeps (MDICLIENTINFO).
#[derive(Clone, Debug, Default)]
pub struct Mdi {
    /// Whether the window is an MDI client (WIN_ISMDICLIENT).
    pub client: bool,
    /// The children it manages, in the order they were made.
    pub children: Vec<u32>,
    pub active: u32,
    pub maximized: u32,
    pub frame_menu: u32,
    pub window_menu: u32,
    pub first_id: u32,
    /// The frame's own title, if it is known yet.
    pub title: Option<Vec<u8>>,
    /// How many children were ever made, for their default places.
    pub total: u32,
    pub flags: u32,
    /// Which scroll bars to set again (SB_* + 1), or 0.
    pub recalc: u32,
}

const WM_MDIRESTORE: u32 = 0x0223;
const WM_MDINEXT: u32 = 0x0224;
const WM_MDIMAXIMIZE: u32 = 0x0225;
const WM_MDITILE: u32 = 0x0226;
const WM_MDICASCADE: u32 = 0x0227;
const WM_MDIICONARRANGE: u32 = 0x0228;
const WM_MDISETMENU: u32 = 0x0230;
const WM_MDIREFRESHMENU: u32 = 0x0234;
/// What Windows posts itself to set the client's scroll bars again.
const WM_MDICALCCHILDSCROLL: u32 = 0x10AC;
const WM_SETVISIBLE: u32 = 0x0009;
const WM_NEXTMENU: u32 = 0x0213;

const SC_NEXTWINDOW: u32 = 0xF040;
const SC_PREVWINDOW: u32 = 0xF050;

const MDIS_ALLCHILDSTYLES: u32 = 0x0001;
const MDITILE_HORIZONTAL: u32 = 0x0001;
const MDITILE_SKIPDISABLED: u32 = 0x0002;

const MF_BYPOSITION: u32 = 0x0400;
const MF_SEPARATOR: u32 = 0x0800;
const MF_CHECKED: u32 = 0x0008;
const MF_GRAYED: u32 = 0x0001;
const MF_DISABLED: u32 = 0x0002;
const MF_HELP: u32 = 0x4000;
const MF_BITMAP: u32 = 0x0004;
const MF_POPUP: u32 = 0x0010;
const MIIM_ID: u32 = 0x0002;
const MIIM_TYPE: u32 = 0x0010;
const MIIM_DATA: u32 = 0x0020;
const HBMMENU_MBAR_RESTORE: u32 = 2;
const HBMMENU_MBAR_MINIMIZE: u32 = 3;
const HBMMENU_MBAR_CLOSE: u32 = 5;
const HBMMENU_MBAR_CLOSE_D: u32 = 6;

const CS_NOCLOSE: u32 = 0x0200;
const SWP_DRAWFRAME: u32 = SWP_FRAMECHANGED;
const SW_INVALIDATE: u32 = 0x0002;
const SW_ERASE: u32 = 0x0004;
const SW_SCROLLCHILDREN: u32 = 0x0001;
const VK_SPACE: u32 = 0x20;
const VK_LEFT: u32 = 0x25;
const VK_RIGHT: u32 = 0x27;
const SIF_RANGE: u32 = 0x01;
const SIF_POS: u32 = 0x04;
const SB_LINEUP: u32 = 0;
const SB_LINEDOWN: u32 = 1;
const SB_PAGEUP: u32 = 2;
const SB_PAGEDOWN: u32 = 3;
const SB_THUMBPOSITION: u32 = 4;
const SB_THUMBTRACK: u32 = 5;
const SB_TOP: u32 = 6;
const SB_BOTTOM: u32 = 7;
const SB_ENDSCROLL: u32 = 8;
const IDI_WINLOGO: u32 = 32517;

const SM_CXVSCROLL: u32 = 2;
const SM_CYHSCROLL: u32 = 3;
const SM_CYCAPTION: u32 = 4;
const SM_CYICON: u32 = 12;
const SM_CYFRAME: u32 = 33;
const SM_CYICONSPACING: u32 = 39;
const SM_CXSMICON: u32 = 49;
const SM_CYSMICON: u32 = 50;

/// The client is to set its scroll bars again.
const NEEDUPDATE: u32 = 0x0001;
/// After this many children, the Window menu ends with "More
/// Windows...".
const MORE_WINDOWS: usize = 9;
const MAX_TITLE: usize = 0xA1;

fn metric(index: u32) -> i32 {
    sysparams::metric(index)
}

/// The MDI client `client`'s state, if it is one.
fn info(cpu: &mut Cpu, client: u32) -> Option<Mdi> {
    let w = user(cpu).window(client)?;
    w.control.mdi.client.then(|| w.control.mdi.clone())
}

fn with<R>(cpu: &mut Cpu, client: u32, f: impl FnOnce(&mut Mdi) -> R) -> Option<R> {
    let w = user(cpu).window_mut(client)?;
    w.control.mdi.client.then(|| f(&mut w.control.mdi))
}

fn style_of(cpu: &mut Cpu, hwnd: u32) -> u32 {
    user(cpu).window(hwnd).map_or(0, |w| w.style)
}

fn zoomed(cpu: &mut Cpu, hwnd: u32) -> bool {
    hwnd != 0 && style_of(cpu, hwnd) & WS_MAXIMIZE != 0
}

fn client_rect(cpu: &mut Cpu, hwnd: u32) -> Rect {
    let c = user(cpu).window(hwnd).map(|w| w.client).unwrap_or_default();
    Rect::new(0, 0, c.width(), c.height())
}

/// The parent of a child window, its frame for an MDI client.
fn parent(cpu: &mut Cpu, hwnd: u32) -> u32 {
    user(cpu).window(hwnd).map_or(0, |w| w.parent)
}

/// An MDI child's next (`next`) or previous window to activate that
/// isn't `hwnd` (the active one for 0) (MDI_GetWindow): visible,
/// enabled, not owned, and without the styles `mask`.
fn next_window(cpu: &mut Cpu, ci: &Mdi, hwnd: u32, next: bool, mask: u32) -> u32 {
    let mask = mask | WS_DISABLED | WS_VISIBLE;
    let hwnd = if hwnd == 0 { ci.active } else { hwnd };
    let state = user(cpu);
    let Some(list) = state
        .window(hwnd)
        .and_then(|w| state.window(w.parent))
        .map(|p| p.children.clone())
    else {
        return 0;
    };
    let at = list
        .iter()
        .position(|&h| h == hwnd)
        .map_or(list.len(), |i| i + 1);
    let order = list[at.min(list.len())..]
        .iter()
        .chain(list[..at.saturating_sub(1).min(list.len())].iter());
    let mut last = 0;
    for &h in order {
        let Some(w) = state.window(h) else {
            continue;
        };
        if w.owner != 0 || w.style & mask != WS_VISIBLE {
            continue;
        }
        last = h;
        if next {
            break;
        }
    }
    last
}

/// The place and size of the `total`th child (MDI_CalcDefaultChildPos):
/// down a third of the client from the place before, about two thirds
/// of it in size.
fn default_place(cpu: &mut Cpu, client: u32, total: u32, delta: i32) -> ((i32, i32), (i32, i32)) {
    let spacing = metric(SM_CYCAPTION) + metric(SM_CYFRAME) - 1;
    let mut r = client_rect(cpu, client);
    if r.height() - delta >= spacing {
        r.bottom -= delta;
    }
    let stagger = r.height() / (3 * spacing);
    let size = (
        r.width() - stagger * spacing,
        r.height() - stagger * spacing,
    );
    let at = spacing * (total as i32 % (stagger + 1));
    ((at, at), size)
}

/// A window's x, y, width and height.
pub type Place = (i32, i32, i32, i32);

/// CreateWindowExA of a window with WS_EX_MDICHILD in `client`: its
/// style, place and id (or menu), or None when it can't be made there.
pub fn child_create(
    cpu: &mut Cpu,
    client: u32,
    style: u32,
    place: Place,
) -> Option<(u32, Place, u32)> {
    let ci = info(cpu, client)?;
    let all = style_of(cpu, client) & MDIS_ALLCHILDSTYLES != 0;
    let style = if all {
        if style & WS_POPUP != 0 {
            return None;
        }
        style | WS_CHILD | WS_CLIPSIBLINGS
    } else {
        (style & !WS_POPUP)
            | WS_CHILD
            | WS_VISIBLE
            | WS_CLIPSIBLINGS
            | WS_CAPTION
            | WS_SYSMENU
            | WS_THICKFRAME
            | WS_MINIMIZEBOX
            | WS_MAXIMIZEBOX
    };
    let ((x0, y0), (w0, h0)) = default_place(cpu, client, ci.total, 0);
    let default = |v: i32| v == CW_USEDEFAULT as i32 || v == -0x8000;
    let (mut x, mut y, mut cx, mut cy) = place;
    if default(x) {
        x = x0;
        y = y0;
    }
    if default(cx) || cx == 0 {
        cx = w0;
    }
    if default(cy) || cy == 0 {
        cy = h0;
    }
    let id = ci.first_id + ci.children.len() as u32;
    Some((style, (x, y, cx, cy), id))
}

/// After CreateWindowExA showed a visible MDI child: the calls that
/// follow (the client's Window menu, the child activated), for
/// CreateWindowExA's task to make.
pub fn child_shown(hwnd: u32, client: u32) -> [(u32, Vec<u32>); 2] {
    let flags = window::SWP_SHOWWINDOW | SWP_NOMOVE | SWP_NOSIZE;
    [
        (
            address_of("SendMessageA"),
            vec![client, WM_MDIREFRESHMENU, 0, 0],
        ),
        (
            address_of("SetWindowPos"),
            vec![hwnd, HWND_TOP, 0, 0, 0, 0, flags],
        ),
    ]
}

/// Post WM_MDICALCCHILDSCROLL to the client once, to set its scroll
/// bars `recalc` (SB_* + 1) again (MDI_PostUpdate).
fn post_update(cpu: &mut Cpu, client: u32, recalc: u32) {
    let post = with(cpu, client, |ci| {
        let post = ci.flags & NEEDUPDATE == 0;
        ci.flags |= NEEDUPDATE;
        ci.recalc = recalc;
        post
    });
    if post == Some(true) {
        message::post_message(cpu, &[client, WM_MDICALCCHILDSCROLL, 0, 0]);
    }
}

// The operations.

/// A list of operations: a call is the function's address, the number
/// of arguments and the arguments; a step of the MDI code is its
/// number (`I_*`), the number of arguments and the arguments.
#[derive(Default)]
struct Ops(Vec<u32>);

impl Ops {
    fn call(&mut self, name: &str, args: &[u32]) -> &mut Ops {
        self.0.push(address_of(name));
        self.0.push(args.len() as u32);
        self.0.extend_from_slice(args);
        self
    }

    fn send(&mut self, hwnd: u32, msg: u32, wp: u32, lp: u32) -> &mut Ops {
        if hwnd != 0 {
            self.call("SendMessageA", &[hwnd, msg, wp, lp]);
        }
        self
    }

    fn op(&mut self, id: u32, args: &[u32]) -> &mut Ops {
        self.0.push(id);
        self.0.push(args.len() as u32);
        self.0.extend_from_slice(args);
        self
    }

    /// DefWindowProcA, whose value the function returns.
    fn def_proc(&mut self, args: &[u32]) -> &mut Ops {
        self.call("DefWindowProcA", args).op(I_RESULT_LAST, &[])
    }
}

/// The task's variables: what it returns, what the last call returned,
/// where the next operation is, and the operations from there on.
const R_RESULT: usize = 1;
const R_LAST: usize = 2;
const R_AT: usize = 3;
const R_OPS: usize = 4;

/// The scratch space: a MENUITEMINFO, then a string or a SCROLLINFO.
const SCRATCH: u32 = 256;
const MII: u32 = 0;
const TEXT: u32 = 64;

const K_OPS: u32 = 0;

/// Run the operations; the function returns `result`, unless an
/// operation sets it.
fn run(cpu: &mut Cpu, ops: Ops, result: u32) -> Option<u32> {
    if ops.0.is_empty() {
        return Some(result);
    }
    let mut vars = vec![result, 0, R_OPS as u32];
    vars.extend(ops.0);
    start(cpu, MDI, K_OPS, vars, SCRATCH)
}

/// The steps of MDI's task.
pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    if t.step != 0 {
        t.set(R_LAST, value);
    }
    loop {
        let at = t.var(R_AT) as usize;
        if at + 1 >= t.vars.len() {
            return Next::Done(t.var(R_RESULT));
        }
        let (code, n) = (t.vars[at], t.vars[at + 1] as usize);
        let end = (at + 2 + n).min(t.vars.len());
        let args = t.vars[at + 2..end].to_vec();
        t.vars.drain(R_OPS..end);
        t.set(R_AT, R_OPS as u32);
        if code >= 0x1_0000 {
            return Next::call(code, &args, 1);
        }
        let more = operation(cpu, t, code, &args);
        let tail = t.vars.split_off(R_OPS);
        t.vars.extend(more.0);
        t.vars.extend(tail);
    }
}

// The steps of the MDI code.

/// The function returns what the last call returned.
const I_RESULT_LAST: u32 = 1;
/// The function returns the argument.
const I_RESULT: u32 = 2;
/// After SetFocus(client): if the focus was on the client already,
/// send it WM_SETFOCUS itself.
const I_FOCUS_WAS: u32 = 3;
const I_CHILD_ACTIVATE: u32 = 4;
const I_SWITCH: u32 = 5;
const I_SET_ACTIVE: u32 = 6;
/// The Window menu: client, menu, with the children (1) or none.
const I_REFRESH: u32 = 7;
const I_REFRESH_COUNT: u32 = 8;
const I_REFRESH_SCAN: u32 = 9;
const I_REFRESH_TYPE: u32 = 10;
const I_REFRESH_ID: u32 = 11;
const I_REFRESH_REMOVE: u32 = 12;
const I_REFRESH_ADD: u32 = 13;
const I_APPEND: u32 = 14;
/// The Window menu as the client has it now.
const I_REFRESH_NOW: u32 = 15;
const I_SET_MENU: u32 = 16;
const I_AUGMENT: u32 = 17;
const I_AUGMENT_MENU: u32 = 18;
const I_AUGMENT_SYS: u32 = 19;
const I_AUGMENT_STATE: u32 = 20;
const I_RESTORE_MENU: u32 = 21;
const I_RESTORE_COUNT: u32 = 22;
const I_RESTORE_ID: u32 = 23;
const I_RESTORE_INFO: u32 = 24;
const I_DELETE_OBJECT: u32 = 25;
const I_FRAME_TEXT: u32 = 26;
const I_DESTROY_CHILD: u32 = 27;
const I_REMOVE_CHILD: u32 = 28;
const I_SET_MAXIMIZED: u32 = 29;
const I_CASCADE: u32 = 30;
const I_TILE: u32 = 31;
const I_CALC_SCROLL: u32 = 32;
const I_SET_SCROLL: u32 = 33;
const I_SCROLL_CHILDREN: u32 = 34;
const I_CLEAR_UPDATE: u32 = 35;
const I_FRAME_MENU: u32 = 36;
/// Write what the last call returned at the address.
const I_WRITE_LAST: u32 = 37;
const I_NEXT_MENU: u32 = 38;
const I_NEXT_MENU_SYS: u32 = 39;

fn operation(cpu: &mut Cpu, t: &mut Task, code: u32, a: &[u32]) -> Ops {
    let last = t.var(R_LAST);
    let arg = |i: usize| a.get(i).copied().unwrap_or(0);
    let mut ops = Ops::default();
    match code {
        I_RESULT_LAST => t.set(R_RESULT, last),
        I_RESULT => t.set(R_RESULT, arg(0)),
        I_FOCUS_WAS => {
            if last == arg(0) {
                ops.send(arg(0), WM_SETFOCUS, arg(0), 0);
            }
        }
        I_CHILD_ACTIVATE => return child_activate(cpu, t, arg(0), arg(1)),
        I_SWITCH => return switch_child(cpu, arg(0), arg(1), arg(2) != 0),
        I_SET_ACTIVE => {
            with(cpu, arg(0), |ci| ci.active = arg(1));
        }
        I_REFRESH_NOW => {
            if let Some(ci) = info(cpu, arg(0)) {
                ops.op(I_REFRESH, &[arg(0), ci.window_menu, 1]);
            }
        }
        I_REFRESH => {
            if arg(1) != 0 {
                ops.call("IsMenu", &[arg(1)]).op(I_REFRESH_COUNT, a);
            }
        }
        I_REFRESH_COUNT => {
            if last != 0 {
                ops.call("GetMenuItemCount", &[arg(1)]);
                ops.op(I_REFRESH_SCAN, &[arg(0), arg(1), arg(2), u32::MAX, 0]);
            }
        }
        I_REFRESH_SCAN => {
            // Arguments: client, menu, children, count, item.
            let count = if arg(3) == u32::MAX { last } else { arg(3) };
            let item = arg(4);
            if count as i32 <= 0 || item >= count {
                ops.op(I_REFRESH_ADD, &[arg(0), arg(1), arg(2)]);
            } else {
                let mii = t.scratch + MII;
                write_words(cpu, mii, &[44, MIIM_TYPE, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
                ops.call("GetMenuItemInfoA", &[arg(1), item, 1, mii]);
                ops.op(I_REFRESH_TYPE, &[arg(0), arg(1), arg(2), count, item]);
            }
        }
        I_REFRESH_TYPE => {
            let kind = cpu.bus.guest_read_32(t.scratch + MII + 8);
            let (count, item) = (arg(3), arg(4));
            if last != 0 && kind & MF_SEPARATOR != 0 {
                // Windows looks only at the id of the item after it.
                let mii = t.scratch + MII;
                write_words(cpu, mii, &[44, MIIM_ID, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
                ops.call("GetMenuItemInfoA", &[arg(1), item + 1, 1, mii]);
                ops.op(I_REFRESH_ID, a);
            } else {
                ops.op(I_REFRESH_SCAN, &[arg(0), arg(1), arg(2), count, item + 1]);
            }
        }
        I_REFRESH_ID => {
            let id = cpu.bus.guest_read_32(t.scratch + MII + 16);
            let first = info(cpu, arg(0)).map_or(0, |ci| ci.first_id);
            let (count, item) = (arg(3), arg(4));
            if last != 0 && id == first {
                ops.call("RemoveMenu", &[arg(1), item, MF_BYPOSITION]);
                ops.op(I_REFRESH_REMOVE, a);
            } else {
                ops.op(I_REFRESH_SCAN, &[arg(0), arg(1), arg(2), count, item + 1]);
            }
        }
        I_REFRESH_REMOVE => {
            if last != 0 {
                ops.call("RemoveMenu", &[arg(1), arg(4), MF_BYPOSITION]);
                ops.op(I_REFRESH_REMOVE, a);
            } else {
                ops.op(I_REFRESH_ADD, &[arg(0), arg(1), arg(2)]);
            }
        }
        I_REFRESH_ADD => {
            let Some(ci) = info(cpu, arg(0)) else {
                return ops;
            };
            if arg(2) == 0 {
                return ops;
            }
            let menu = arg(1);
            let mut visible = 0;
            for &child in &ci.children {
                if style_of(cpu, child) & WS_VISIBLE == 0 {
                    continue;
                }
                let id = ci.first_id + visible as u32;
                if visible == MORE_WINDOWS {
                    ops.op(I_APPEND, &[menu, id, 0, 0]);
                    break;
                }
                if visible == 0 {
                    // Visio expects the separator's id to be 0.
                    ops.call("AppendMenuA", &[menu, MF_SEPARATOR, 0, 0]);
                }
                visible += 1;
                if let Some(w) = user(cpu).window_mut(child) {
                    w.id = id;
                }
                ops.op(I_APPEND, &[menu, id, child, visible as u32]);
                if child == ci.active {
                    ops.call("CheckMenuItem", &[menu, id, MF_CHECKED]);
                }
            }
        }
        I_APPEND => {
            // Arguments: menu, id, child, number (0: "More Windows...").
            let text = if arg(2) == 0 {
                b"&More Windows...".to_vec()
            } else {
                let title = user(cpu)
                    .window(arg(2))
                    .map(|w| w.text.clone())
                    .unwrap_or_default();
                let mut text = vec![b'&', b'0' + arg(3) as u8, b' '];
                text.extend(title.iter().take(MAX_TITLE - 4));
                text
            };
            let at = t.scratch + TEXT;
            cpu.bus.guest_write_bytes(at, &text);
            cpu.bus.guest_write_bytes(at + text.len() as u32, &[0]);
            ops.call("AppendMenuA", &[arg(0), 0, arg(1), at]);
        }
        I_SET_MENU => return set_menu(cpu, t, arg(0), arg(1), arg(2)),
        I_AUGMENT => {
            ops.call("GetMenu", &[arg(0)]).op(I_AUGMENT_MENU, a);
        }
        I_AUGMENT_MENU => {
            if last != 0 {
                ops.call("GetSystemMenu", &[arg(1), 0]);
                ops.op(I_AUGMENT_SYS, &[arg(0), arg(1), last]);
            }
        }
        I_AUGMENT_SYS => {
            if last != 0 {
                ops.call("GetMenuState", &[last, SC_CLOSE, 0]);
                ops.op(I_AUGMENT_STATE, &[arg(0), arg(1), arg(2), last]);
            }
        }
        I_AUGMENT_STATE => return augment(cpu, arg(0), arg(1), arg(2), arg(3), last),
        I_RESTORE_MENU => {
            ops.call("GetMenu", &[arg(0)])
                .op(I_RESTORE_COUNT, &[arg(0)]);
        }
        I_RESTORE_COUNT => {
            if last != 0 {
                ops.call("GetMenuItemCount", &[last]);
                ops.op(I_RESTORE_ID, &[arg(0), last]);
            }
        }
        I_RESTORE_ID => {
            let n = last.wrapping_sub(1);
            ops.call("GetMenuItemID", &[arg(1), n]);
            ops.op(I_RESTORE_INFO, &[arg(0), arg(1), 0]);
        }
        I_RESTORE_INFO => {
            let (frame, menu) = (arg(0), arg(1));
            if arg(2) == 0 {
                // The last item: the system's buttons, if there are any.
                if last != SC_RESTORE && last != SC_CLOSE {
                    return ops;
                }
                let mii = t.scratch + MII;
                write_words(
                    cpu,
                    mii,
                    &[44, MIIM_DATA | MIIM_TYPE, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                );
                ops.call("GetMenuItemInfoA", &[menu, 0, 1, mii]);
                ops.op(I_RESTORE_INFO, &[frame, menu, 1]);
                return ops;
            }
            let w = read_words(cpu, t.scratch + MII, 11);
            let (kind, data) = (w[2], w[9]);
            ops.call("RemoveMenu", &[menu, 0, MF_BYPOSITION]);
            // The child's icon, made into a bitmap for the menu.
            if kind & MF_BITMAP != 0 && data & 0xFFFF > 0x10 {
                ops.op(I_DELETE_OBJECT, &[data]);
            }
            ops.call("DeleteMenu", &[menu, SC_CLOSE, 0]);
            ops.call("DeleteMenu", &[menu, SC_RESTORE, 0]);
            ops.call("DeleteMenu", &[menu, SC_MINIMIZE, 0]);
            ops.call("DrawMenuBar", &[frame]);
        }
        I_DELETE_OBJECT => {
            gdi32::delete_object(cpu, &[arg(0)]);
        }
        I_FRAME_TEXT => return frame_text(cpu, t, arg(0), arg(1), arg(2) != 0, arg(3)),
        I_DESTROY_CHILD => return destroy_child(cpu, arg(0), arg(1), arg(2) != 0),
        I_REMOVE_CHILD => {
            with(cpu, arg(0), |ci| ci.children.retain(|&h| h != arg(1)));
        }
        I_SET_MAXIMIZED => {
            with(cpu, arg(0), |ci| ci.maximized = arg(1));
        }
        I_CASCADE => return cascade(cpu, arg(0)),
        I_TILE => return tile(cpu, arg(0), arg(1)),
        I_CALC_SCROLL => return calc_scroll(cpu, arg(0), arg(1)),
        I_SET_SCROLL => {
            // Arguments: window, bar, min, max, position.
            let at = t.scratch + TEXT;
            write_words(
                cpu,
                at,
                &[28, SIF_RANGE | SIF_POS, arg(2), arg(3), 0, arg(4), 0],
            );
            ops.call("SetScrollInfo", &[arg(0), arg(1), at, 1]);
        }
        I_SCROLL_CHILDREN => return scroll_children(cpu, arg(0), arg(1), arg(2), arg(3)),
        I_CLEAR_UPDATE => {
            with(cpu, arg(0), |ci| ci.flags &= !NEEDUPDATE);
        }
        I_FRAME_MENU => {
            with(cpu, arg(0), |ci| ci.frame_menu = last);
        }
        I_WRITE_LAST => cpu.bus.guest_write_32(arg(0), last),
        I_NEXT_MENU => {
            // Arguments: frame, MDINEXTMENU, key, active child. The
            // child's system menu sits between the frame's system menu
            // and its menu bar.
            let (frame, next, key, child) = (arg(0), arg(1), arg(2), arg(3));
            let menu_in = cpu.bus.guest_read_32(next);
            let child_sys = user(cpu).window(child).map_or(0, |w| w.sysmenu);
            if key == VK_LEFT && last == menu_in {
                ops.call("GetSubMenu", &[child_sys, 0])
                    .op(I_WRITE_LAST, &[next + 4]);
                cpu.bus.guest_write_32(next + 8, child);
            } else if key == VK_RIGHT {
                let sys = user(cpu).window(frame).map_or(0, |w| w.sysmenu);
                ops.call("GetSubMenu", &[sys, 0]);
                ops.op(I_NEXT_MENU_SYS, &[next, child, child_sys]);
            }
        }
        I_NEXT_MENU_SYS => {
            let (next, child, child_sys) = (arg(0), arg(1), arg(2));
            if last == cpu.bus.guest_read_32(next) {
                ops.call("GetSubMenu", &[child_sys, 0])
                    .op(I_WRITE_LAST, &[next + 4]);
                cpu.bus.guest_write_32(next + 8, child);
            }
        }
        _ => {}
    }
    ops
}

/// MDI_ChildActivate: `child` (0 for none) becomes the active child.
fn child_activate(cpu: &mut Cpu, t: &mut Task, client: u32, child: u32) -> Ops {
    let mut ops = Ops::default();
    let Some(ci) = info(cpu, client) else {
        return ops;
    };
    if ci.active == child {
        t.set(R_RESULT, 0);
        return ops;
    }
    let frame = parent(cpu, client);
    let active_frame = user(cpu).is_active(frame);
    let prev = ci.active;
    if prev != 0 {
        ops.send(prev, WM_NCACTIVATE, 0, 0);
        ops.send(prev, WM_MDIACTIVATE, prev, child);
    }
    ops.op(I_SWITCH, &[client, child, 0]);
    ops.op(I_SET_ACTIVE, &[client, child]);
    ops.op(I_REFRESH_NOW, &[client]);
    if active_frame {
        ops.send(child, WM_NCACTIVATE, 1, 0);
        // The client hands the focus on to the child; if the focus is
        // on the client already, SetFocus does nothing, and Windows
        // sends WM_SETFOCUS itself.
        ops.call("SetFocus", &[client]).op(I_FOCUS_WAS, &[client]);
    }
    ops.send(child, WM_MDIACTIVATE, prev, child);
    ops.op(I_RESULT, &[1]);
    ops
}

/// MDI_SwitchActiveChild: bring `to` to the top, and maximize it if
/// the active child was.
fn switch_child(cpu: &mut Cpu, client: u32, to: u32, activate: bool) -> Ops {
    let mut ops = Ops::default();
    let Some(ci) = info(cpu, client) else {
        return ops;
    };
    let prev = ci.active;
    if to == prev || to == 0 {
        return ops;
    }
    let flags = SWP_NOMOVE | SWP_NOSIZE;
    if zoomed(cpu, prev) {
        ops.send(prev, WM_SETREDRAW, 0, 0);
        ops.call("ShowWindow", &[prev, SW_RESTORE]);
        ops.send(prev, WM_SETREDRAW, 1, 0);
        ops.call("SetWindowPos", &[to, HWND_TOP, 0, 0, 0, 0, flags]);
        ops.call("ShowWindow", &[to, SW_MAXIMIZE]);
    }
    let flags = flags | if activate { 0 } else { SWP_NOACTIVATE };
    ops.call("SetWindowPos", &[to, HWND_TOP, 0, 0, 0, 0, flags]);
    ops
}

/// MDISetMenu: WM_MDISETMENU's frame menu and Window menu.
fn set_menu(cpu: &mut Cpu, t: &mut Task, client: u32, frame_menu: u32, window_menu: u32) -> Ops {
    let mut ops = Ops::default();
    let Some(ci) = info(cpu, client) else {
        return ops;
    };
    let frame = parent(cpu, client);
    t.set(R_RESULT, 0);
    if frame_menu != 0 {
        if frame_menu == ci.frame_menu {
            t.set(R_RESULT, frame_menu);
            return ops;
        }
        if ci.maximized != 0 {
            ops.op(I_RESTORE_MENU, &[frame, ci.maximized]);
        }
    }
    if window_menu != 0 && window_menu != ci.window_menu {
        if ci.window_menu != 0 && !ci.children.is_empty() {
            // The children move from the old Window menu to the new.
            ops.op(I_REFRESH, &[client, ci.window_menu, 0]);
            ops.op(I_REFRESH, &[client, window_menu, 1]);
        }
        with(cpu, client, |ci| ci.window_menu = window_menu);
    }
    if frame_menu != 0 {
        ops.call("SetMenu", &[frame, frame_menu]);
        with(cpu, client, |ci| ci.frame_menu = frame_menu);
        if ci.maximized != 0 {
            ops.op(I_AUGMENT, &[frame, ci.maximized]);
        }
        t.set(R_RESULT, ci.frame_menu);
    } else if ci.maximized != 0 {
        ops.op(I_AUGMENT, &[frame, ci.maximized]);
    }
    ops
}

/// The icon of a window for its maximized system menu: its small one,
/// its big one, its class's, or the system's.
fn icon_of(cpu: &mut Cpu, hwnd: u32) -> u32 {
    let w = user(cpu).window(hwnd);
    let own = w.map_or(0, |w| {
        if w.icons[1] != 0 {
            w.icons[1]
        } else {
            w.icons[0]
        }
    });
    if own != 0 {
        return own;
    }
    let class = class::of(user(cpu), hwnd).map_or(0, |c| {
        if c.icon_small != 0 {
            c.icon_small
        } else {
            c.icon
        }
    });
    if class != 0 {
        return class;
    }
    resource::load_icon(cpu, &[0, IDI_WINLOGO]).unwrap_or(0)
}

/// The child's icon on the menu's color, in a small bitmap.
fn icon_bitmap(cpu: &mut Cpu, child: u32) -> u32 {
    let icon = icon_of(cpu, child);
    if icon == 0 {
        return 0;
    }
    let (cx, cy) = (metric(SM_CXSMICON), metric(SM_CYSMICON));
    let Some(screen) = paint::get_dc(cpu, &[0]).filter(|&h| h != 0) else {
        return 0;
    };
    let args = [screen, cx as u32, cy as u32];
    let bitmap = gdi32::bitmap::create_compatible_bitmap(cpu, &args).unwrap_or(0);
    let mem = gdi32::dc::create_compatible_dc(cpu, &[screen]).unwrap_or(0);
    if bitmap != 0 && mem != 0 {
        let old = gdi32::dc::select_object(cpu, &[mem, bitmap]).unwrap_or(0);
        paint::fill_color(cpu, mem, Rect::new(0, 0, cx, cy), COLOR_MENU);
        paint::draw_icon_at(cpu, mem, (0, 0), icon, (cx, cy), DI_NORMAL);
        if old != 0 {
            gdi32::dc::select_object(cpu, &[mem, old]);
        }
    }
    if mem != 0 {
        gdi32::dc::delete_dc(cpu, &[mem]);
    }
    paint::release_dc(cpu, &[0, screen]);
    bitmap
}

/// MDI_AugmentFrameMenu, once the frame's menu, the child's system menu
/// and the state of its Close are known: the child's system menu, as
/// its icon, goes first on the frame's menu bar, and its buttons last.
fn augment(cpu: &mut Cpu, frame: u32, child: u32, menu: u32, sys: u32, close: u32) -> Ops {
    let mut ops = Ops::default();
    let no_close = class::style_of(user(cpu), child) & CS_NOCLOSE != 0;
    let enabled = !no_close && close != u32::MAX && close & (MF_DISABLED | MF_GRAYED) == 0;
    let close = if enabled {
        HBMMENU_MBAR_CLOSE
    } else {
        HBMMENU_MBAR_CLOSE_D
    };
    let flags = MF_HELP | MF_BITMAP;
    ops.call("AppendMenuA", &[menu, flags, SC_CLOSE, close]);
    ops.call(
        "AppendMenuA",
        &[menu, flags, SC_RESTORE, HBMMENU_MBAR_RESTORE],
    );
    ops.call(
        "AppendMenuA",
        &[menu, flags, SC_MINIMIZE, HBMMENU_MBAR_MINIMIZE],
    );
    let bitmap = icon_bitmap(cpu, child);
    let flags = MF_BYPOSITION | MF_BITMAP | MF_POPUP;
    ops.call("InsertMenuA", &[menu, 0, flags, sys, bitmap]);
    ops.call("EnableMenuItem", &[sys, SC_SIZE, MF_GRAYED]);
    ops.call("EnableMenuItem", &[sys, SC_MOVE, MF_GRAYED]);
    ops.call("EnableMenuItem", &[sys, SC_MAXIMIZE, MF_GRAYED]);
    ops.call("SetMenuDefaultItem", &[sys, SC_CLOSE, 0]);
    ops.call("DrawMenuBar", &[frame]);
    ops
}

/// MDI_UpdateFrameText: the frame's title, with the maximized child's
/// in brackets. `title` is a new title for the frame, or 0.
fn frame_text(
    cpu: &mut Cpu,
    t: &mut Task,
    frame: u32,
    client: u32,
    repaint: bool,
    title: u32,
) -> Ops {
    let mut ops = Ops::default();
    let Some(ci) = info(cpu, client) else {
        return ops;
    };
    let mut own = ci.title.clone();
    if title != 0 {
        own = Some(cpu.bus.guest_read_asciiz(title, 0x1_0000));
    } else if own.is_none() {
        // The first time: the frame's title as it is.
        own = Some(
            user(cpu)
                .window(frame)
                .map(|w| w.text.clone())
                .unwrap_or_default(),
        );
    }
    with(cpu, client, |ci| ci.title = own.clone());
    let own = own.unwrap_or_default();
    let mut text: Vec<u8> = own.iter().copied().take(MAX_TITLE - 1).collect();
    if ci.maximized != 0 && own.len() + 6 < MAX_TITLE {
        let child = user(cpu)
            .window(ci.active)
            .map(|w| w.text.clone())
            .unwrap_or_default();
        if !child.is_empty() {
            text.extend_from_slice(b" - [");
            let room = MAX_TITLE - own.len() - 5;
            text.extend(child.iter().take(room.saturating_sub(1)));
            text.push(b']');
        }
    }
    let at = t.scratch + TEXT;
    cpu.bus.guest_write_bytes(at, &text);
    cpu.bus.guest_write_bytes(at + text.len() as u32, &[0]);
    ops.call("DefWindowProcA", &[frame, WM_SETTEXT, 0, at]);
    if repaint {
        let flags = SWP_FRAMECHANGED | SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE | SWP_NOZORDER;
        ops.call("SetWindowPos", &[frame, 0, 0, 0, 0, 0, flags]);
    }
    ops
}

/// MDIDestroyChild: `child` goes from the client's list; with
/// `destroy`, it is destroyed too (WM_MDIDESTROY rather than its
/// WM_PARENTNOTIFY).
fn destroy_child(cpu: &mut Cpu, client: u32, child: u32, destroy: bool) -> Ops {
    let mut ops = Ops::default();
    let Some(ci) = info(cpu, client) else {
        return ops;
    };
    if child == ci.active {
        let next = next_window(cpu, &ci, child, true, 0);
        if destroy && next != 0 {
            ops.op(I_SWITCH, &[client, next, 1]);
        } else {
            ops.call("ShowWindow", &[child, SW_HIDE]);
            if child == ci.maximized {
                let frame = parent(cpu, client);
                ops.op(I_RESTORE_MENU, &[frame, child]);
                ops.op(I_SET_MAXIMIZED, &[client, 0]);
                ops.op(I_FRAME_TEXT, &[frame, client, 1, 0]);
            }
            if destroy {
                ops.op(I_CHILD_ACTIVATE, &[client, 0]);
            }
        }
    }
    ops.op(I_REMOVE_CHILD, &[client, child]);
    if destroy {
        ops.send(client, WM_MDIREFRESHMENU, 0, 0);
        post_update(cpu, client, SB_BOTH + 1);
        ops.call("DestroyWindow", &[child]);
    }
    ops.op(I_RESULT, &[0]);
    ops
}

/// The client's children that cascading and tiling move: visible, not
/// owned, not minimized; and whether some are minimized.
fn arranged(cpu: &mut Cpu, client: u32, skip_disabled: bool) -> (Vec<u32>, bool) {
    let state = user(cpu);
    let list = state
        .window(client)
        .map(|w| w.children.clone())
        .unwrap_or_default();
    let mut icons = false;
    let mut out = Vec::new();
    for h in list {
        if !state.is_visible(h) {
            continue;
        }
        let Some(w) = state.window(h) else {
            continue;
        };
        if w.owner != 0 {
            continue;
        }
        if w.style & WS_MINIMIZE != 0 {
            icons = true;
            continue;
        }
        if skip_disabled && w.style & WS_DISABLED != 0 {
            continue;
        }
        out.push(h);
    }
    (out, icons)
}

/// WM_MDICASCADE (MDICascade).
fn cascade(cpu: &mut Cpu, client: u32) -> Ops {
    let mut ops = Ops::default();
    let Some(ci) = info(cpu, client) else {
        return ops;
    };
    if ci.maximized != 0 {
        ops.send(client, WM_MDIRESTORE, ci.maximized, 0);
        ops.op(I_CASCADE, &[client]);
        with(cpu, client, |ci| ci.maximized = 0);
        return ops;
    }
    if ci.children.is_empty() {
        return ops;
    }
    let (list, icons) = arranged(cpu, client, false);
    let delta = if icons {
        metric(SM_CYICONSPACING) + metric(SM_CYICON)
    } else {
        0
    };
    for (n, &h) in list.iter().rev().enumerate() {
        let ((x, y), (cx, cy)) = default_place(cpu, client, n as u32, delta);
        let mut flags = SWP_DRAWFRAME | SWP_NOACTIVATE | SWP_NOZORDER;
        if style_of(cpu, h) & WS_THICKFRAME == 0 {
            flags |= SWP_NOSIZE;
        }
        let args = [h, 0, x as u32, y as u32, cx as u32, cy as u32, flags];
        ops.call("SetWindowPos", &args);
    }
    if icons {
        ops.call("ArrangeIconicWindows", &[client]);
    }
    ops
}

/// WM_MDITILE (MDITile).
fn tile(cpu: &mut Cpu, client: u32, how: u32) -> Ops {
    let mut ops = Ops::default();
    let Some(ci) = info(cpu, client) else {
        return ops;
    };
    if ci.maximized != 0 {
        ops.send(client, WM_MDIRESTORE, ci.maximized, 0);
        ops.op(I_TILE, &[client, how]);
        with(cpu, client, |ci| ci.maximized = 0);
        return ops;
    }
    if ci.children.is_empty() {
        return ops;
    }
    let (list, icons) = arranged(cpu, client, how & MDITILE_SKIPDISABLED != 0);
    let total = list.len() as i32;
    if total > 0 {
        let mut r = client_rect(cpu, client);
        let mut rows = (total as f64).sqrt() as i32;
        let mut columns = total / rows;
        if how & MDITILE_HORIZONTAL != 0 {
            std::mem::swap(&mut rows, &mut columns);
        }
        if icons {
            let y = r.bottom - 2 * metric(SM_CYICONSPACING) - metric(SM_CYICON);
            if y - metric(SM_CYICON) >= r.top {
                r.bottom = y;
            }
        }
        let mut ysize = r.bottom / rows;
        let xsize = r.right / columns;
        let (mut x, mut i) = (0, 0);
        let mut c = 1;
        while c <= columns && (i as usize) < list.len() {
            if c == columns {
                rows = total - i;
                ysize = r.bottom / rows;
            }
            let mut y = 0;
            let mut row = 1;
            while row <= rows && (i as usize) < list.len() {
                let h = list[i as usize];
                let mut flags = SWP_DRAWFRAME | SWP_NOACTIVATE | SWP_NOZORDER;
                if style_of(cpu, h) & WS_THICKFRAME == 0 {
                    flags |= SWP_NOSIZE;
                }
                let args = [h, 0, x as u32, y as u32, xsize as u32, ysize as u32, flags];
                ops.call("SetWindowPos", &args);
                y += ysize;
                row += 1;
                i += 1;
            }
            x += xsize;
            c += 1;
        }
    }
    if icons {
        ops.call("ArrangeIconicWindows", &[client]);
    }
    ops
}

/// CalcChildScroll: the client's scroll bars cover its children and its
/// client area; none with a maximized child.
fn calc_scroll(cpu: &mut Cpu, client: u32, bar: u32) -> Ops {
    let mut ops = Ops::default();
    let state = user(cpu);
    let Some(w) = state.window(client) else {
        return ops;
    };
    let area = Rect::new(0, 0, w.client.width(), w.client.height());
    let mut children = Rect::default();
    for &h in &w.children {
        let Some(c) = state.window(h) else {
            continue;
        };
        if c.style & WS_MAXIMIZE != 0 {
            ops.call("ShowScrollBar", &[client, SB_BOTH, 0]);
            return ops;
        }
        if c.style & WS_VISIBLE != 0 {
            children = if children.is_empty() {
                c.rect
            } else {
                children.union(&c.rect)
            };
        }
    }
    let all = if children.is_empty() {
        area
    } else {
        children.union(&area)
    };
    if bar == SB_BOTH || bar == SB_HORZ {
        let args = [
            client,
            SB_HORZ,
            all.left as u32,
            (all.right - area.right) as u32,
            (area.left - all.left) as u32,
        ];
        ops.op(I_SET_SCROLL, &args);
    }
    if bar == SB_BOTH || bar == SB_VERT {
        let args = [
            client,
            SB_VERT,
            all.top as u32,
            (all.bottom - area.bottom) as u32,
            (area.top - all.top) as u32,
        ];
        ops.op(I_SET_SCROLL, &args);
    }
    ops
}

/// ScrollChildren: the client's WM_HSCROLL and WM_VSCROLL.
fn scroll_children(cpu: &mut Cpu, client: u32, msg: u32, wp: u32, lp: u32) -> Ops {
    let mut ops = Ops::default();
    let r = client_rect(cpu, client);
    let (bar, length, shift) = match msg {
        WM_HSCROLL => (SB_HORZ, r.width() / 2, metric(SM_CYHSCROLL)),
        WM_VSCROLL => (SB_VERT, r.height() / 2, metric(SM_CXVSCROLL)),
        _ => return ops,
    };
    let si = user(cpu)
        .window(client)
        .map(|w| w.scroll[bar as usize])
        .unwrap_or_default();
    let (min, max, current) = if si.made {
        (si.min, si.max, si.pos)
    } else {
        (0, 0, 0)
    };
    let new = match wp & 0xFFFF {
        SB_LINEUP => current - shift,
        SB_LINEDOWN => current + shift,
        SB_PAGEUP => current - length,
        SB_PAGEDOWN => current + length,
        SB_THUMBPOSITION => (lp & 0xFFFF) as i32,
        SB_THUMBTRACK => return ops,
        SB_TOP => min,
        SB_BOTTOM => max,
        SB_ENDSCROLL => {
            ops.op(I_CALC_SCROLL, &[client, bar]);
            return ops;
        }
        _ => -1,
    };
    let new = if new > max { max } else { new.max(min) };
    ops.call("SetScrollPos", &[client, bar, new as u32, 1]);
    let (dx, dy) = if msg == WM_VSCROLL {
        (0, current - new)
    } else {
        (current - new, 0)
    };
    let flags = SW_INVALIDATE | SW_ERASE | SW_SCROLLCHILDREN;
    let args = [client, dx as u32, dy as u32, 0, 0, 0, 0, flags];
    ops.call("ScrollWindowEx", &args);
    ops
}

// The window procedures.

/// The MDIClient class's window procedure.
pub fn mdiclient_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, msg, wp, lp] = args else {
        unreachable!()
    };
    let Some(ci) = info(cpu, hwnd) else {
        if msg == WM_NCCREATE
            && let Some(w) = user(cpu).window_mut(hwnd)
        {
            w.control.mdi.client = true;
        }
        return defwnd::def_window_proc(cpu, args);
    };
    let mut ops = Ops::default();
    match msg {
        WM_CREATE => {
            let words = read_words(cpu, lp, 4);
            let (create, frame) = (words[0], words[3]);
            let (window_menu, first_id) = if create != 0 {
                let ccs = read_words(cpu, create, 2);
                (ccs[0], ccs[1])
            } else {
                (0, 0)
            };
            with(cpu, hwnd, |ci| {
                *ci = Mdi {
                    client: true,
                    window_menu,
                    first_id,
                    ..Mdi::default()
                };
            });
            ops.call("GetMenu", &[frame]).op(I_FRAME_MENU, &[hwnd]);
            run(cpu, ops, 0)
        }
        WM_DESTROY => {
            if ci.maximized != 0 {
                let frame = parent(cpu, hwnd);
                ops.op(I_RESTORE_MENU, &[frame, ci.maximized]);
            }
            ops.op(I_REFRESH, &[hwnd, ci.window_menu, 0]);
            with(cpu, hwnd, |ci| {
                ci.children.clear();
                ci.title = None;
            });
            run(cpu, ops, 0)
        }
        WM_MDIACTIVATE => {
            if ci.active != wp {
                let flags = SWP_NOSIZE | SWP_NOMOVE;
                ops.call("SetWindowPos", &[wp, 0, 0, 0, 0, 0, flags]);
            }
            run(cpu, ops, 0)
        }
        WM_MDICASCADE => {
            let ops = cascade(cpu, hwnd);
            run(cpu, ops, 0)
        }
        WM_MDICREATE => {
            if lp == 0 {
                return Some(0);
            }
            let cs = read_words(cpu, lp, 9);
            let args = [
                WS_EX_MDICHILD,
                cs[0],
                cs[1],
                cs[7],
                cs[3],
                cs[4],
                cs[5],
                cs[6],
                hwnd,
                0,
                cs[2],
                cs[8],
            ];
            ops.call("CreateWindowExA", &args).op(I_RESULT_LAST, &[]);
            run(cpu, ops, 0)
        }
        WM_MDIDESTROY => {
            let ops = destroy_child(cpu, hwnd, wp, true);
            run(cpu, ops, 0)
        }
        WM_MDIGETACTIVE => {
            if lp != 0 {
                let z = zoomed(cpu, ci.active) as u32;
                cpu.bus.guest_write_32(lp, z);
            }
            Some(ci.active)
        }
        WM_MDIICONARRANGE => {
            with(cpu, hwnd, |ci| {
                ci.flags |= NEEDUPDATE;
                ci.recalc = SB_BOTH + 1;
            });
            ops.call("ArrangeIconicWindows", &[hwnd]);
            ops.send(hwnd, WM_MDICALCCHILDSCROLL, 0, 0);
            run(cpu, ops, 0)
        }
        WM_MDIMAXIMIZE => {
            ops.call("ShowWindow", &[wp, SW_MAXIMIZE]);
            run(cpu, ops, 0)
        }
        WM_MDINEXT => {
            // lParam set: the previous window.
            let next = next_window(cpu, &ci, wp, lp == 0, 0);
            let ops = switch_child(cpu, hwnd, next, true);
            run(cpu, ops, 0)
        }
        WM_MDIRESTORE => {
            ops.call("ShowWindow", &[wp, SW_SHOWNORMAL]);
            run(cpu, ops, 0)
        }
        WM_MDISETMENU => {
            ops.op(I_SET_MENU, &[hwnd, wp, lp]);
            run(cpu, ops, 0)
        }
        WM_MDIREFRESHMENU => {
            ops.op(I_REFRESH, &[hwnd, ci.window_menu, 1]);
            run(cpu, ops, ci.frame_menu)
        }
        WM_MDITILE => {
            with(cpu, hwnd, |ci| ci.flags |= NEEDUPDATE);
            ops.call("ShowScrollBar", &[hwnd, SB_BOTH, 0]);
            ops.op(I_TILE, &[hwnd, wp]);
            ops.op(I_CLEAR_UPDATE, &[hwnd]);
            run(cpu, ops, 0)
        }
        WM_VSCROLL | WM_HSCROLL => {
            with(cpu, hwnd, |ci| ci.flags |= NEEDUPDATE);
            ops.op(I_SCROLL_CHILDREN, &[hwnd, msg, wp, lp]);
            ops.op(I_CLEAR_UPDATE, &[hwnd]);
            run(cpu, ops, 0)
        }
        WM_SETFOCUS => {
            if ci.active != 0 && style_of(cpu, ci.active) & WS_MINIMIZE == 0 {
                ops.call("SetFocus", &[ci.active]);
            }
            run(cpu, ops, 0)
        }
        WM_NCACTIVATE => {
            ops.send(ci.active, msg, wp, lp);
            ops.def_proc(args);
            run(cpu, ops, 0)
        }
        WM_PARENTNOTIFY => match wp & 0xFFFF {
            WM_CREATE => {
                let mdi = user(cpu)
                    .window(lp)
                    .is_some_and(|w| w.ex_style & WS_EX_MDICHILD != 0);
                if mdi {
                    with(cpu, hwnd, |ci| {
                        ci.total += 1;
                        ci.children.push(lp);
                    });
                }
                Some(0)
            }
            WM_LBUTTONDOWN => {
                let (x, y) = (lp as u16 as i16 as i32, (lp >> 16) as u16 as i16 as i32);
                let point = [hwnd, x as u32, y as u32];
                let child = window::child_window_from_point(cpu, &point).unwrap_or(0);
                if child != 0 && child != hwnd && child != ci.active {
                    let flags = SWP_NOSIZE | SWP_NOMOVE;
                    ops.call("SetWindowPos", &[child, 0, 0, 0, 0, 0, flags]);
                }
                run(cpu, ops, 0)
            }
            WM_DESTROY => {
                let ops = destroy_child(cpu, hwnd, lp, false);
                run(cpu, ops, 0)
            }
            _ => Some(0),
        },
        WM_SIZE => {
            if ci.active != 0 && zoomed(cpu, ci.active) {
                let w = user(cpu).window(ci.active);
                let (style, ex) = w.map_or((0, 0), |w| (w.style, w.ex_style));
                let r = Rect::new(0, 0, (lp & 0xFFFF) as i32, (lp >> 16) as i32);
                let r = window::adjust(r, style, ex, false);
                let args = [
                    ci.active,
                    r.left as u32,
                    r.top as u32,
                    r.width() as u32,
                    r.height() as u32,
                    1,
                ];
                ops.call("MoveWindow", &args);
            } else {
                post_update(cpu, hwnd, SB_BOTH + 1);
            }
            ops.def_proc(args);
            run(cpu, ops, 0)
        }
        WM_MDICALCCHILDSCROLL => {
            if ci.flags & NEEDUPDATE != 0 && ci.recalc != 0 {
                ops.op(I_CALC_SCROLL, &[hwnd, ci.recalc - 1]);
                with(cpu, hwnd, |ci| {
                    ci.recalc = 0;
                    ci.flags &= !NEEDUPDATE;
                });
            }
            run(cpu, ops, 0)
        }
        _ => defwnd::def_window_proc(cpu, args),
    }
}

/// DefFrameProcA(hwnd, client, msg, wParam, lParam).
pub fn def_frame_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, client, msg, wp, lp] = args else {
        unreachable!()
    };
    let rest = [hwnd, msg, wp, lp];
    let ci = if client != 0 { info(cpu, client) } else { None };
    let Some(ci) = ci else {
        return defwnd::def_window_proc(cpu, &rest);
    };
    let mut ops = Ops::default();
    match msg {
        WM_SETTEXT => {
            ops.op(I_FRAME_TEXT, &[hwnd, client, 0, lp]);
            return run(cpu, ops, 1);
        }
        WM_COMMAND => {
            let id = wp & 0xFFFF;
            let count = ci.children.len() as u32;
            if id < ci.first_id || id >= ci.first_id + count {
                // The maximized child's system commands, from its
                // buttons on the frame's menu bar.
                let system = (id.wrapping_sub(0xF000)) & 0xF00F == 0;
                let closes =
                    id != SC_CLOSE || class::style_of(user(cpu), ci.active) & CS_NOCLOSE == 0;
                let command = matches!(
                    id,
                    SC_SIZE
                        | SC_MOVE
                        | SC_MINIMIZE
                        | SC_MAXIMIZE
                        | SC_NEXTWINDOW
                        | SC_PREVWINDOW
                        | SC_RESTORE
                        | SC_CLOSE
                );
                if system && ci.maximized != 0 && command && closes {
                    ops.send(ci.maximized, WM_SYSCOMMAND, wp, lp);
                    ops.op(I_RESULT_LAST, &[]);
                    return run(cpu, ops, 0);
                }
            } else if (id - ci.first_id) as usize != MORE_WINDOWS {
                // One of the children on the Window menu.
                let child = ci
                    .children
                    .iter()
                    .copied()
                    .find(|&h| user(cpu).window(h).is_some_and(|w| w.id == id));
                if let Some(child) = child {
                    ops.send(client, WM_MDIACTIVATE, child, 0);
                }
            }
        }
        WM_NCACTIVATE => {
            ops.send(client, msg, wp, lp);
        }
        WM_SETFOCUS => {
            ops.call("SetFocus", &[client]);
        }
        WM_SIZE => {
            let (cx, cy) = (lp & 0xFFFF, lp >> 16);
            ops.call("MoveWindow", &[client, 0, 0, cx, cy, 1]);
        }
        WM_NEXTMENU => {
            let iconic = style_of(cpu, hwnd) & WS_MINIMIZE != 0;
            if !iconic && ci.active != 0 && !zoomed(cpu, ci.active) {
                ops.call("GetMenu", &[hwnd]);
                ops.op(I_NEXT_MENU, &[hwnd, lp, wp, ci.active]);
            }
            return run(cpu, ops, 0);
        }
        _ => {}
    }
    ops.def_proc(&rest);
    run(cpu, ops, 0)
}

/// DefMDIChildProcA(hwnd, msg, wParam, lParam).
pub fn def_mdi_child_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, msg, wp, lp] = args else {
        unreachable!()
    };
    let client = parent_of(cpu, hwnd);
    let Some(ci) = info(cpu, client) else {
        return defwnd::def_window_proc(cpu, args);
    };
    let frame = parent(cpu, client);
    let mut ops = Ops::default();
    match msg {
        WM_SETTEXT => {
            ops.call("DefWindowProcA", args);
            if ci.maximized == hwnd {
                ops.op(I_FRAME_TEXT, &[frame, client, 1, 0]);
            }
            return run(cpu, ops, 1);
        }
        WM_GETMINMAXINFO => {
            // The maximized child's client area is the client's.
            let w = user(cpu).window(hwnd);
            let (style, ex) = w.map_or((0, 0), |w| (w.style, w.ex_style));
            let r = window::adjust(client_rect(cpu, client), style, ex, false);
            let words = [
                r.width() as u32,
                r.height() as u32,
                r.left as u32,
                r.top as u32,
            ];
            write_words(cpu, lp + 8, &words);
            return Some(0);
        }
        WM_MENUCHAR => return Some(0x0001_0000),
        WM_CLOSE => {
            ops.send(client, WM_MDIDESTROY, hwnd, 0);
            return run(cpu, ops, 0);
        }
        WM_SETFOCUS => {
            if ci.active != hwnd {
                ops.op(I_CHILD_ACTIVATE, &[client, hwnd]);
                ops.op(I_RESULT, &[0]);
            }
        }
        WM_CHILDACTIVATE => {
            ops.op(I_CHILD_ACTIVATE, &[client, hwnd]).op(I_RESULT, &[0]);
            return run(cpu, ops, 0);
        }
        WM_SYSCOMMAND => match wp & 0xFFF0 {
            SC_MOVE if ci.maximized == hwnd => return Some(0),
            SC_MAXIMIZE if ci.maximized == hwnd => {
                ops.send(frame, msg, wp, lp).op(I_RESULT_LAST, &[]);
                return run(cpu, ops, 0);
            }
            SC_NEXTWINDOW => {
                ops.send(client, WM_MDINEXT, 0, 0);
                return run(cpu, ops, 0);
            }
            SC_PREVWINDOW => {
                ops.send(client, WM_MDINEXT, 0, 1);
                return run(cpu, ops, 0);
            }
            _ => {}
        },
        WM_SHOWWINDOW | WM_SETVISIBLE => {
            if ci.maximized != 0 {
                with(cpu, client, |ci| ci.flags &= !NEEDUPDATE);
            } else {
                post_update(cpu, client, SB_BOTH + 1);
            }
        }
        WM_SIZE => {
            // The only place a child becomes maximized or stops being.
            if ci.maximized == hwnd && wp != SIZE_MAXIMIZED {
                with(cpu, client, |ci| ci.maximized = 0);
                ops.op(I_RESTORE_MENU, &[frame, hwnd]);
                ops.op(I_FRAME_TEXT, &[frame, client, 1, 0]);
            }
            let mut update = true;
            if wp == SIZE_MAXIMIZED {
                let old = ci.maximized;
                if old == hwnd {
                    update = false;
                } else {
                    if old != 0 {
                        ops.send(old, WM_SETREDRAW, 0, 0);
                        ops.op(I_RESTORE_MENU, &[frame, old]);
                        ops.call("ShowWindow", &[old, SW_SHOWNOACTIVATE]);
                        ops.send(old, WM_SETREDRAW, 1, 0);
                    }
                    ops.op(I_SET_MAXIMIZED, &[client, hwnd]);
                    ops.op(I_AUGMENT, &[frame, hwnd]);
                    ops.op(I_FRAME_TEXT, &[frame, client, 1, 0]);
                }
            }
            if update {
                if wp == SIZE_MINIMIZED {
                    let next = next_window(cpu, &ci, hwnd, true, WS_MINIMIZE);
                    let to = if next != 0 { next } else { hwnd };
                    ops.send(to, WM_CHILDACTIVATE, 0, 0);
                }
                post_update(cpu, client, SB_BOTH + 1);
            }
        }
        WM_NEXTMENU => {
            // VK_LEFT: the frame's system menu; VK_RIGHT: its menu bar.
            if wp == VK_LEFT {
                let sys = user(cpu).window(frame).map_or(0, |w| w.sysmenu);
                ops.call("GetSubMenu", &[sys, 0])
                    .op(I_WRITE_LAST, &[lp + 4]);
            } else if wp == VK_RIGHT {
                ops.call("GetMenu", &[frame]).op(I_WRITE_LAST, &[lp + 4]);
            }
            cpu.bus.guest_write_32(lp + 8, frame);
            return run(cpu, ops, 0);
        }
        WM_SYSCHAR => {
            if wp == b'-' as u32 {
                ops.send(hwnd, WM_SYSCOMMAND, SC_KEYMENU, VK_SPACE);
                return run(cpu, ops, 0);
            }
        }
        WM_DESTROY => {
            // It goes from the Window menu.
            ops.op(I_REFRESH, &[client, ci.window_menu, 1]);
        }
        _ => {}
    }
    ops.def_proc(args);
    run(cpu, ops, 0)
}

/// CreateMDIWindowA(class, name, style, x, y, width, height, parent,
/// instance, lParam).
pub fn create_mdi_window(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[class, name, style, x, y, cx, cy, parent, instance, lp] = args else {
        unreachable!()
    };
    let args = [
        WS_EX_MDICHILD,
        class,
        name,
        style,
        x,
        y,
        cx,
        cy,
        parent,
        0,
        instance,
        lp,
    ];
    window::create_window_ex(cpu, &args)
}

crate::state_fields!(Mdi {
    client,
    children,
    active,
    maximized,
    frame_menu,
    window_menu,
    first_id,
    title,
    total,
    flags,
    recalc
});
