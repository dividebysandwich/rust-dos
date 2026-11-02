//! Windows: making and destroying them, their place in the tree and on
//! the screen, showing and hiding, and what GetWindowLongA reads.
//!
//! A window's rectangles are kept in its parent's client coordinates, as
//! Windows keeps them: `rect` the whole window, `client` its client area.
//! Children are in z-order, the top one first. A new child goes to the
//! bottom of its siblings and a new top-level window to the top, so a
//! dialog's controls are in the order its template has them.
//!
//! CreateWindowExA, DestroyWindow, ShowWindow and SetWindowPos send the
//! window the messages Windows 95 sends, in its order, as tasks.

use super::message::*;
use super::{
    DESKTOP, ERROR_INVALID_INDEX, ERROR_INVALID_PARAMETER, ERROR_INVALID_WINDOW_HANDLE, FALSE,
    Name, State, TRUE, address_of, current_pid, current_tid, read_words, set_error, string_at,
    user, write_words,
};
use crate::cpu::Cpu;
use crate::win32::gdi32::{Rect, Region};
use crate::win32::task::{self, Next, Task};

pub const WS_OVERLAPPED: u32 = 0x0000_0000;
pub const WS_POPUP: u32 = 0x8000_0000;
pub const WS_CHILD: u32 = 0x4000_0000;
pub const WS_MINIMIZE: u32 = 0x2000_0000;
pub const WS_VISIBLE: u32 = 0x1000_0000;
pub const WS_DISABLED: u32 = 0x0800_0000;
pub const WS_CLIPSIBLINGS: u32 = 0x0400_0000;
pub const WS_CLIPCHILDREN: u32 = 0x0200_0000;
pub const WS_MAXIMIZE: u32 = 0x0100_0000;
pub const WS_CAPTION: u32 = 0x00C0_0000;
pub const WS_BORDER: u32 = 0x0080_0000;
pub const WS_DLGFRAME: u32 = 0x0040_0000;
pub const WS_VSCROLL: u32 = 0x0020_0000;
pub const WS_HSCROLL: u32 = 0x0010_0000;
pub const WS_SYSMENU: u32 = 0x0008_0000;
pub const WS_THICKFRAME: u32 = 0x0004_0000;
pub const WS_GROUP: u32 = 0x0002_0000;
pub const WS_TABSTOP: u32 = 0x0001_0000;
pub const WS_MINIMIZEBOX: u32 = 0x0002_0000;
pub const WS_MAXIMIZEBOX: u32 = 0x0001_0000;

pub const WS_EX_DLGMODALFRAME: u32 = 0x0000_0001;
pub const WS_EX_NOPARENTNOTIFY: u32 = 0x0000_0004;
pub const WS_EX_TOPMOST: u32 = 0x0000_0008;
pub const WS_EX_TRANSPARENT: u32 = 0x0000_0020;
pub const WS_EX_MDICHILD: u32 = 0x0000_0040;
pub const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
pub const WS_EX_WINDOWEDGE: u32 = 0x0000_0100;
pub const WS_EX_CLIENTEDGE: u32 = 0x0000_0200;
pub const WS_EX_CONTROLPARENT: u32 = 0x0001_0000;
pub const WS_EX_STATICEDGE: u32 = 0x0002_0000;
pub const WS_EX_APPWINDOW: u32 = 0x0004_0000;

/// SetWindowPos's flags.
pub const SWP_NOSIZE: u32 = 0x0001;
pub const SWP_NOMOVE: u32 = 0x0002;
pub const SWP_NOZORDER: u32 = 0x0004;
pub const SWP_NOREDRAW: u32 = 0x0008;
pub const SWP_NOACTIVATE: u32 = 0x0010;
pub const SWP_FRAMECHANGED: u32 = 0x0020;
pub const SWP_SHOWWINDOW: u32 = 0x0040;
pub const SWP_HIDEWINDOW: u32 = 0x0080;
pub const SWP_NOCOPYBITS: u32 = 0x0100;
pub const SWP_NOOWNERZORDER: u32 = 0x0200;
pub const SWP_NOSENDCHANGING: u32 = 0x0400;
pub const SWP_DEFERERASE: u32 = 0x2000;
pub const SWP_ASYNCWINDOWPOS: u32 = 0x4000;
/// Windows' own flags in WINDOWPOS: the client area didn't move or
/// change size, and the window's state (minimized, maximized) changed.
pub const SWP_NOCLIENTSIZE: u32 = 0x0800;
pub const SWP_NOCLIENTMOVE: u32 = 0x1000;
pub const SWP_STATECHANGED: u32 = 0x8000;

pub const HWND_TOP: u32 = 0;
pub const HWND_BOTTOM: u32 = 1;
pub const HWND_TOPMOST: u32 = 0xFFFF_FFFF;
pub const HWND_NOTOPMOST: u32 = 0xFFFF_FFFE;
pub const HWND_MESSAGE: u32 = 0xFFFF_FFFD;

/// ShowWindow's commands.
pub const SW_HIDE: u32 = 0;
pub const SW_SHOWNORMAL: u32 = 1;
pub const SW_SHOWMINIMIZED: u32 = 2;
pub const SW_SHOWMAXIMIZED: u32 = 3;
pub const SW_MAXIMIZE: u32 = 3;
pub const SW_SHOWNOACTIVATE: u32 = 4;
pub const SW_SHOW: u32 = 5;
pub const SW_MINIMIZE: u32 = 6;
pub const SW_SHOWMINNOACTIVE: u32 = 7;
pub const SW_SHOWNA: u32 = 8;
pub const SW_RESTORE: u32 = 9;
pub const SW_SHOWDEFAULT: u32 = 10;
pub const SW_FORCEMINIMIZE: u32 = 11;

pub const CW_USEDEFAULT: u32 = 0x8000_0000;

/// GetWindowLongA's indices.
pub const GWL_WNDPROC: i32 = -4;
pub const GWL_HINSTANCE: i32 = -6;
pub const GWL_HWNDPARENT: i32 = -8;
pub const GWL_ID: i32 = -12;
pub const GWL_STYLE: i32 = -16;
pub const GWL_EXSTYLE: i32 = -20;
pub const GWL_USERDATA: i32 = -21;

/// GetWindow's relations.
const GW_HWNDFIRST: u32 = 0;
const GW_HWNDLAST: u32 = 1;
const GW_HWNDNEXT: u32 = 2;
const GW_HWNDPREV: u32 = 3;
const GW_OWNER: u32 = 4;
const GW_CHILD: u32 = 5;
const GW_ENABLEDPOPUP: u32 = 6;

/// The window's own flags.
pub const NEED_SIZE: u32 = 0x01;
pub const DESTROYING: u32 = 0x02;
/// WM_NCCREATE has returned, so the window is in its parent's list.
pub const LINKED: u32 = 0x04;
/// Its DC (CS_OWNDC) belongs to it.
pub const HAS_DC: u32 = 0x08;
pub const IN_DESTROY_MSG: u32 = 0x10;
/// WM_NCDESTROY has been sent to it: what remains is to free it.
pub const NC_DESTROYED: u32 = 0x20;
/// Its last WM_SIZE was sent.
pub const SIZE_SENT: u32 = 0x40;

/// A window.
#[derive(Clone, Debug, Default)]
pub struct Window {
    /// Its class's place in `State::classes`.
    pub class: u32,
    pub style: u32,
    pub ex_style: u32,
    /// Its parent: the desktop for a top-level window.
    pub parent: u32,
    pub owner: u32,
    /// Its children, the top of the z-order first.
    pub children: Vec<u32>,
    /// The window and its client area, in the parent's client
    /// coordinates.
    pub rect: Rect,
    pub client: Rect,
    pub text: Vec<u8>,
    /// A child's id, or a top-level window's menu.
    pub id: u32,
    pub instance: u32,
    pub proc: u32,
    pub user_data: u32,
    /// Its extra bytes (the class's cbWndExtra).
    pub extra: Vec<u8>,
    /// The thread and process it belongs to.
    pub tid: u32,
    pub pid: u32,
    pub props: Vec<(String, u32)>,
    /// What needs painting, relative to the window's top left corner:
    /// the frame and client area, what lay in them before the window
    /// got smaller, and for a CS_PARENTDC child what of its parent's
    /// client area it was given. `paint`'s flags say how.
    pub update: Region,
    pub paint: u32,
    pub flags: u32,
    /// Where it goes when it is restored, in the parent's client
    /// coordinates, and where it goes when minimized.
    pub normal: Rect,
    pub icon_pos: (i32, i32),
    pub menu: u32,
    pub sysmenu: u32,
    /// Its scroll bars' state: horizontal, vertical.
    pub scroll: [super::controls::scrollbar::ScrollInfo; 2],
    /// Its own DC (CS_OWNDC).
    pub dc: u32,
    /// WM_SETICON's icons: big, small.
    pub icons: [u32; 2],
    /// SetWindowRgn's region, in window coordinates.
    pub region: Option<Region>,
    /// What a system class's window procedure keeps.
    pub control: super::controls::Control,
    /// The popup of it that was active last.
    pub last_popup: u32,
    pub help_id: u32,
}

/// WM_NCCALCSIZE's and AdjustWindowRectEx's frame: whether the style has
/// a thick, dialog or thin frame.
pub fn has_dlg_frame(style: u32, ex: u32) -> bool {
    ex & WS_EX_DLGMODALFRAME != 0 || (style & WS_DLGFRAME != 0 && style & WS_THICKFRAME == 0)
}

pub fn has_thick_frame(style: u32, _ex: u32) -> bool {
    style & WS_THICKFRAME != 0 && style & (WS_DLGFRAME | WS_BORDER) != WS_DLGFRAME
}

pub fn has_thin_frame(style: u32) -> bool {
    style & WS_BORDER != 0 || style & (WS_CHILD | WS_POPUP) == 0
}

/// The window rectangle around the client rectangle `r` for the style,
/// as AdjustWindowRectEx makes it.
pub fn adjust(r: Rect, style: u32, ex: u32, menu: bool) -> Rect {
    let m = super::sysparams::metric;
    let mut r = r;
    // The outer frame and caption.
    let (dx, dy) = if has_thick_frame(style, ex) {
        (m(32), m(33))
    } else if has_dlg_frame(style, ex) {
        (m(7), m(8))
    } else if has_thin_frame(style) {
        (m(5), m(6))
    } else {
        (0, 0)
    };
    r = r.inflate(dx, dy);
    if style & WS_CAPTION == WS_CAPTION {
        r.top -= if ex & WS_EX_TOOLWINDOW != 0 {
            m(51)
        } else {
            m(4)
        };
    }
    if menu {
        r.top -= m(15);
    }
    // The inner edges.
    if ex & WS_EX_CLIENTEDGE != 0 {
        r = r.inflate(m(45), m(46));
    }
    if ex & WS_EX_STATICEDGE != 0 {
        r = r.inflate(m(5), m(6));
    }
    r
}

/// The frame a maximized window puts off the screen: what
/// AdjustWindowRectEx puts around an empty rectangle, without the
/// caption. A WS_OVERLAPPEDWINDOW keeps its 4-pixel thick frame, so it
/// is maximized at (-4, -4), as on Windows 98.
///
/// A child window (an MDI child, say) takes WS_BORDER out of a
/// WS_CAPTION style first, leaving WS_DLGFRAME | WS_THICKFRAME, which
/// AdjustWindowRectEx gives no thick frame. Wine's user32 win.c tests
/// check that on Windows a maximized child with WS_CAPTION |
/// WS_THICKFRAME fills its parent's client area exactly.
fn max_frame(style: u32, ex: u32) -> Rect {
    if style & WS_CHILD != 0 {
        let framed = if style & WS_CAPTION == WS_CAPTION {
            style & !WS_BORDER
        } else {
            style
        };
        return adjust(Rect::default(), framed, ex, false);
    }
    let mut frame = adjust(Rect::default(), style, ex, false);
    if style & WS_CAPTION == WS_CAPTION {
        frame.top += if ex & WS_EX_TOOLWINDOW != 0 {
            super::sysparams::metric(51)
        } else {
            super::sysparams::metric(4)
        };
    }
    frame
}

/// The client area for the window rectangle `r`: what DefWindowProcA's
/// WM_NCCALCSIZE makes.
pub fn client_for(state: &State, hwnd: u32, r: Rect) -> Rect {
    let Some(w) = state.window(hwnd) else {
        return r;
    };
    let m = super::sysparams::metric;
    let (style, ex) = (w.style, w.ex_style);
    if super::class::is_class(state, hwnd, "#32768") {
        return r;
    }
    let menu = style & WS_CHILD == 0 && w.id != 0;
    let outer = adjust(Rect::default(), style, ex, false);
    let mut c = Rect::new(
        r.left - outer.left,
        r.top - outer.top,
        r.right - outer.right,
        r.bottom - outer.bottom,
    );
    if menu {
        c.top += m(15);
    }
    if style & WS_VSCROLL != 0 {
        c.right -= m(2);
    }
    if style & WS_HSCROLL != 0 {
        c.bottom -= m(3);
    }
    // A minimized window has no client area.
    if style & WS_MINIMIZE != 0 {
        c = Rect::new(r.left, r.top, r.left, r.top);
    }
    if c.right < c.left {
        c.right = c.left;
    }
    if c.bottom < c.top {
        c.bottom = c.top;
    }
    c
}

/// The styles CreateWindowExA gives a window whatever was asked: a
/// top-level window clips its siblings, an overlapped one has a caption,
/// and WS_EX_WINDOWEDGE follows the frame.
fn fix_styles(style: u32, ex: u32) -> (u32, u32) {
    let mut style = style;
    if style & WS_CHILD == 0 {
        style |= WS_CLIPSIBLINGS;
        if style & WS_POPUP == 0 {
            style |= WS_CAPTION;
        }
    }
    (style, fix_ex_style(style, ex))
}

/// The extended style for `style`: WS_EX_WINDOWEDGE goes with the
/// thicker frames.
fn fix_ex_style(style: u32, ex: u32) -> u32 {
    let mut ex = ex;
    if ex & WS_EX_DLGMODALFRAME != 0 {
        ex |= WS_EX_WINDOWEDGE;
    } else if style & (WS_DLGFRAME | WS_THICKFRAME) != 0 {
        if !(ex & WS_EX_STATICEDGE != 0 && style & (WS_CHILD | WS_POPUP) != 0) {
            ex |= WS_EX_WINDOWEDGE;
        }
    } else {
        ex &= !WS_EX_WINDOWEDGE;
    }
    ex
}

/// Bring the top-level windows `hwnd` owns, and those they own, to just
/// above it, in the order they had. Topmost ones stay where they are
/// when `hwnd` isn't topmost.
fn raise_owned(state: &mut State, hwnd: u32) {
    let Some(w) = state.window(hwnd) else {
        return;
    };
    if w.parent != DESKTOP {
        return;
    }
    let topmost = w.ex_style & WS_EX_TOPMOST != 0;
    let list = state
        .window(DESKTOP)
        .map(|d| d.children.clone())
        .unwrap_or_default();
    let owned_by = |h: u32| {
        let mut o = state.window(h).map_or(0, |w| w.owner);
        for _ in 0..64 {
            if o == 0 || o == hwnd {
                break;
            }
            o = state.window(o).map_or(0, |w| w.owner);
        }
        o == hwnd
            && (topmost
                || state
                    .window(h)
                    .is_some_and(|w| w.ex_style & WS_EX_TOPMOST == 0))
    };
    let (moving, mut rest): (Vec<u32>, Vec<u32>) = list.iter().partition(|&&h| owned_by(h));
    if moving.is_empty() {
        return;
    }
    let at = rest.iter().position(|&h| h == hwnd).unwrap_or(0);
    rest.splice(at..at, moving);
    if let Some(d) = state.window_mut(DESKTOP) {
        d.children = rest;
    }
}

/// Put `hwnd` in its parent's list of children, after `after` (HWND_TOP,
/// HWND_BOTTOM or a sibling). Topmost windows stay above the others.
pub fn link(state: &mut State, hwnd: u32, after: u32) {
    let Some(w) = state.window(hwnd) else {
        return;
    };
    let (parent, topmost) = (w.parent, w.ex_style & WS_EX_TOPMOST != 0);
    let is_topmost = |state: &State, h: u32| {
        state
            .window(h)
            .is_some_and(|w| w.ex_style & WS_EX_TOPMOST != 0)
    };
    let siblings: Vec<u32> = state
        .window(parent)
        .map(|p| p.children.clone())
        .unwrap_or_default();
    let mut list: Vec<u32> = siblings.into_iter().filter(|&h| h != hwnd).collect();
    let first_plain = list
        .iter()
        .position(|&h| !is_topmost(state, h))
        .unwrap_or(list.len());
    let at = match after {
        HWND_TOP | HWND_TOPMOST | HWND_NOTOPMOST => {
            if topmost || parent != DESKTOP {
                0
            } else {
                first_plain
            }
        }
        HWND_BOTTOM => {
            if topmost && parent == DESKTOP {
                first_plain
            } else {
                list.len()
            }
        }
        h => match list.iter().position(|&x| x == h) {
            Some(i) => i + 1,
            None => 0,
        },
    };
    let at = if parent == DESKTOP && !topmost {
        at.max(first_plain)
    } else if parent == DESKTOP && topmost {
        at.min(first_plain)
    } else {
        at
    };
    list.insert(at.min(list.len()), hwnd);
    if let Some(p) = state.window_mut(parent) {
        p.children = list;
    }
}

/// Take `hwnd` out of its parent's list.
pub fn unlink(state: &mut State, hwnd: u32) {
    let parent = state.parent(hwnd);
    if let Some(p) = state.window_mut(parent) {
        p.children.retain(|&h| h != hwnd);
    }
}

/// The desktop window, which every top-level window is a child of.
pub fn create_desktop(cpu: &mut Cpu) {
    let (w, h) = super::sysparams::screen_size(cpu);
    let class = super::class::find_system(user(cpu), "#32769").unwrap_or(0);
    let proc = user(cpu)
        .classes
        .get(class as usize)
        .and_then(|c| c.as_ref())
        .map_or(0, |c| c.proc);
    let desktop = Window {
        class,
        style: WS_POPUP | WS_VISIBLE | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
        rect: Rect::new(0, 0, w, h),
        client: Rect::new(0, 0, w, h),
        proc,
        flags: LINKED,
        ..Window::default()
    };
    let state = user(cpu);
    state.windows.clear();
    state.windows.push(Some(desktop));
    state.next_window = 1;
}

/// The places of the size in a CREATESTRUCT.
const CS_CY: u32 = 16;
const CS_CX: u32 = 20;

/// The scratch space of CreateWindowExA: the CREATESTRUCT, the hook's
/// CBT_CREATEWND, a MINMAXINFO and the rectangle of WM_NCCALCSIZE.
const CREATE_CS: u32 = 0;
const CREATE_CBT: u32 = 48;
const CREATE_MINMAX: u32 = 56;
const CREATE_RECT: u32 = 96;
/// An MDI child's MDICREATESTRUCT, which lpCreateParams points at.
const CREATE_MDI: u32 = 112;
const CREATE_SCRATCH: u32 = 148;

/// The variables of CreateWindowExA's task.
const C_HWND: usize = 0;
const C_STYLE: usize = 1;
const C_PARENT: usize = 2;
const C_NOTIFY: usize = 3;

/// CreateWindowExA(ex_style, class, name, style, x, y, width, height,
/// parent, menu, instance, param).
pub fn create_window_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[
        ex,
        class,
        name,
        style,
        x,
        y,
        cx,
        cy,
        parent,
        menu,
        instance,
        param,
    ] = args
    else {
        unreachable!()
    };
    super::init(cpu);
    let class_name = Name::read(cpu, class)?;
    let pid = current_pid(cpu);
    let Some(class_index) = super::class::find(user(cpu), &class_name, instance, pid) else {
        set_error(cpu, super::ERROR_CLASS_DOES_NOT_EXIST);
        return Some(0);
    };
    if name != 0 && string_at(cpu, name).is_none() {
        return None;
    }
    // An MDI child gets its style, place and id from its MDI client; its
    // MDICREATESTRUCT keeps what the program asked for.
    let asked = [style, x, y, cx, cy];
    let (style, x, y, cx, cy, menu) = if ex & WS_EX_MDICHILD != 0 {
        let place = (x as i32, y as i32, cx as i32, cy as i32);
        let Some((style, (x, y, cx, cy), id)) =
            super::controls::mdi::child_create(cpu, parent, style, place)
        else {
            return Some(0);
        };
        (style, x as u32, y as u32, cx as u32, cy as u32, id)
    } else {
        (style, x, y, cx, cy, menu)
    };
    let state = user(cpu);
    let mut parent = parent;
    let mut owner = 0;
    if parent == HWND_MESSAGE {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(0);
    }
    if style & WS_CHILD != 0 {
        if parent == 0 {
            set_error(cpu, super::ERROR_TLW_WITH_WSCHILD);
            return Some(0);
        }
        if !state.is_window(parent) {
            set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
            return Some(0);
        }
    } else {
        if parent != 0 {
            if !state.is_window(parent) {
                set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
                return Some(0);
            }
            if parent != DESKTOP {
                owner = state.top_level(parent);
            }
        }
        parent = DESKTOP;
    }
    // The CREATESTRUCT has the style as it was given, and the extended
    // style for that.
    let given = (style, fix_ex_style(style, ex));
    let (style, mut ex) = fix_styles(style, ex);
    // A window owned by a topmost one is topmost too.
    if state
        .window(owner)
        .is_some_and(|o| o.ex_style & WS_EX_TOPMOST != 0)
    {
        ex |= WS_EX_TOPMOST;
    }
    let class_info = state.classes[class_index as usize].clone().unwrap();
    // The window's procedure may be in COMCTL32's image.
    if !super::class::map_dll(cpu, class_index) {
        set_error(cpu, super::ERROR_NOT_ENOUGH_MEMORY);
        return Some(0);
    }
    // CW_USEDEFAULT: an overlapped window gets a place and three
    // quarters of the screen; a popup or child window none.
    let (sw, sh) = super::sysparams::screen_size(cpu);
    let (mut x, mut y, mut cx, mut cy) = (x as i32, y as i32, cx as i32, cy as i32);
    let default = CW_USEDEFAULT as i32;
    if style & (WS_CHILD | WS_POPUP) == 0 {
        if x == default {
            x = 0;
            y = 0;
        }
        if cx == default {
            cx = sw * 3 / 4 - x;
            cy = sh * 3 / 4 - y;
        }
    } else {
        if x == default {
            x = 0;
            y = 0;
        }
        if cx == default {
            cx = 0;
            cy = 0;
        }
    }
    let menu_handle = if style & WS_CHILD == 0 && menu == 0 && class_info.menu_name != 0 {
        // A top-level window without a menu of its own gets its class's.
        let args = [class_info.instance, class_info.menu_name];
        super::menu::load_menu(cpu, &args).unwrap_or(0)
    } else {
        menu
    };
    let rect = Rect::sized(x, y, cx, cy);
    let window = Window {
        class: class_index,
        // What the window gets besides is added after the CBT hook.
        style: given.0 & !WS_VISIBLE,
        ex_style: ex,
        parent,
        owner,
        rect,
        client: rect,
        id: menu_handle,
        instance,
        proc: class_info.proc,
        extra: vec![0; class_info.wnd_extra as usize],
        tid: current_tid(cpu),
        pid,
        normal: rect,
        icon_pos: (-1, -1),
        ..Window::default()
    };
    let state = user(cpu);
    let Some(hwnd) = state.new_window(window) else {
        set_error(cpu, super::ERROR_NOT_ENOUGH_MEMORY);
        return Some(0);
    };
    if let Some(c) = state.classes[class_index as usize].as_mut() {
        c.windows += 1;
    }
    super::controls::init(cpu, hwnd);
    super::paint::tree_changed(cpu);
    let vars = vec![hwnd, style, parent, 0];
    // The CREATESTRUCT the messages point at, in the scratch space.
    let words = [
        param,
        instance,
        menu,
        if parent == DESKTOP {
            if owner != 0 { owner } else { 0 }
        } else {
            parent
        },
        cy as u32,
        cx as u32,
        y as u32,
        x as u32,
        given.0,
        name,
        class,
        given.1,
    ];
    let vars = [vars, words.to_vec(), asked.to_vec()].concat();
    task::start(cpu, TASK_CREATE, vars, CREATE_SCRATCH)
}

/// The tasks of this module, by their places in `task::TASKS`.
const TASK_CREATE: u16 = 3;
const TASK_DESTROY: u16 = 4;
const TASK_SHOW: u16 = 5;
const TASK_SET_POS: u16 = 6;
const TASK_ENUM: u16 = 9;
const TASK_MISC: u16 = 15;

/// HCBT_ hook codes.
pub const HCBT_MOVESIZE: u32 = 0;
pub const HCBT_MINMAX: u32 = 1;
pub const HCBT_CREATEWND: u32 = 3;
pub const HCBT_DESTROYWND: u32 = 4;
pub const HCBT_ACTIVATE: u32 = 5;
pub const HCBT_SETFOCUS: u32 = 9;
pub const WH_CBT: u32 = 5;

/// Read the CREATESTRUCT back from the scratch space into the window:
/// a CBT hook may change it.
fn read_create_struct(cpu: &mut Cpu, hwnd: u32, cs: u32) {
    let w = read_words(cpu, cs, 12);
    let rect = Rect::sized(w[7] as i32, w[6] as i32, w[5] as i32, w[4] as i32);
    if let Some(win) = user(cpu).window_mut(hwnd) {
        win.rect = rect;
        win.client = rect;
        win.normal = rect;
    }
}

/// CreateWindowExA's steps.
pub fn create_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let hwnd = t.var(C_HWND);
    let cs = t.scratch + CREATE_CS;
    loop {
        match t.step {
            0 => {
                let mut words: Vec<u32> = t.vars[4..16].to_vec();
                if words[11] & WS_EX_MDICHILD != 0 {
                    // The MDICREATESTRUCT: class, title, instance, x,
                    // y, width, height, style and the parameter.
                    let mdi = t.scratch + CREATE_MDI;
                    let w = &words;
                    let a: Vec<u32> = (16..21).map(|i| t.var(i)).collect();
                    let mcs = [w[10], w[9], w[1], a[1], a[2], a[3], a[4], a[0], w[0]];
                    write_words(cpu, mdi, &mcs);
                    words[0] = mdi;
                }
                write_words(cpu, cs, &words);
                t.vars.truncate(4);
                if super::hook::has(cpu, WH_CBT) {
                    let cbt = t.scratch + CREATE_CBT;
                    write_words(cpu, cbt, &[cs, HWND_TOP]);
                    return super::hook::call(WH_CBT, HCBT_CREATEWND, hwnd, cbt, 1);
                }
                t.step = 1;
            }
            1 => {
                if value != 0 && t.step == 1 && super::hook::has(cpu, WH_CBT) {
                    free_window(cpu, hwnd);
                    return Next::Done(0);
                }
                if !user(cpu).is_window(hwnd) {
                    return Next::Done(0);
                }
                read_create_struct(cpu, hwnd, cs);
                let style = t.var(C_STYLE);
                if let Some(w) = user(cpu).window_mut(hwnd) {
                    w.style |= style & !WS_VISIBLE;
                }
                if style & WS_THICKFRAME != 0 || style & (WS_POPUP | WS_CHILD) == 0 {
                    let mm = t.scratch + CREATE_MINMAX;
                    write_min_max(cpu, hwnd, mm);
                    return send(hwnd, WM_GETMINMAXINFO, 0, mm, 2);
                }
                t.step = 3;
            }
            2 => {
                // The window's size is kept within what WM_GETMINMAXINFO
                // says; the CREATESTRUCT keeps the size asked for.
                let mm = read_words(cpu, t.scratch + CREATE_MINMAX, 10);
                let (min_x, min_y, max_x, max_y) =
                    (mm[6] as i32, mm[7] as i32, mm[8] as i32, mm[9] as i32);
                let cx = cpu.bus.guest_read_32(cs + CS_CX) as i32;
                let cy = cpu.bus.guest_read_32(cs + CS_CY) as i32;
                let cx = cx.min(max_x).max(min_x).max(0);
                let cy = cy.min(max_y).max(min_y).max(0);
                read_create_struct(cpu, hwnd, cs);
                if (cx != 0 || cy != 0)
                    && let Some(w) = user(cpu).window_mut(hwnd)
                {
                    let r = Rect::sized(w.rect.left, w.rect.top, cx, cy);
                    w.rect = r;
                    w.client = r;
                    w.normal = r;
                }
                t.step = 3;
            }
            3 => return send(hwnd, WM_NCCREATE, 0, cs, 4),
            4 => {
                if value == 0 || !user(cpu).is_window(hwnd) {
                    // The window goes, with WM_NCDESTROY.
                    t.set(C_NOTIFY, 0);
                    return send(hwnd, WM_NCDESTROY, 0, 0, 20);
                }
                // In its parent's list now.
                let state = user(cpu);
                let after = if t.var(C_STYLE) & WS_CHILD != 0 {
                    HWND_BOTTOM
                } else {
                    HWND_TOP
                };
                link(state, hwnd, after);
                if let Some(w) = state.window_mut(hwnd) {
                    w.flags |= LINKED;
                }
                super::paint::tree_changed(cpu);
                // WM_NCCALCSIZE without wParam has the rectangle on the
                // screen.
                let state = user(cpu);
                let rect = state.window(hwnd).map(|w| w.rect).unwrap_or_default();
                let origin = state.screen_client(t.var(C_PARENT));
                rect.offset(origin.left, origin.top)
                    .write(cpu, t.scratch + CREATE_RECT);
                return send(hwnd, WM_NCCALCSIZE, 0, t.scratch + CREATE_RECT, 5);
            }
            5 => {
                let origin = user(cpu).screen_client(t.var(C_PARENT));
                let client = Rect::read(cpu, t.scratch + CREATE_RECT);
                let client = client.offset(-origin.left, -origin.top);
                if let Some(w) = user(cpu).window_mut(hwnd) {
                    w.client = client;
                }
                return send(hwnd, WM_CREATE, 0, cs, 6);
            }
            6 => {
                if value as i32 == -1 || !user(cpu).is_window(hwnd) {
                    return Next::call(address_of("DestroyWindow"), &[hwnd], 21);
                }
                let style = t.var(C_STYLE);
                if style & (WS_CHILD | WS_POPUP) != 0 {
                    let client = user(cpu).window(hwnd).map(|w| w.client).unwrap_or_default();
                    let size = super::make_long(client.width(), client.height());
                    return send(hwnd, WM_SIZE, SIZE_RESTORED, size, 7);
                }
                if let Some(w) = user(cpu).window_mut(hwnd) {
                    w.flags |= NEED_SIZE;
                }
                t.step = 8;
            }
            7 => {
                let client = user(cpu).window(hwnd).map(|w| w.client).unwrap_or_default();
                let pos = super::make_long(client.left, client.top);
                return send(hwnd, WM_MOVE, 0, pos, 8);
            }
            8 => {
                // Created minimized or maximized: it becomes so now, and
                // comes to the top. The MDI client does this for its
                // children.
                let style = t.var(C_STYLE);
                let Some(w) = user(cpu).window_mut(hwnd) else {
                    return Next::Done(0);
                };
                if style & (WS_MINIMIZE | WS_MAXIMIZE) == 0 || w.ex_style & WS_EX_MDICHILD != 0 {
                    t.step = 9;
                    continue;
                }
                w.style &= !(WS_MINIMIZE | WS_MAXIMIZE);
                t.step = 30;
                if super::hook::has(cpu, WH_CBT) {
                    let cmd = if style & WS_MINIMIZE != 0 {
                        SW_MINIMIZE
                    } else {
                        SW_MAXIMIZE
                    };
                    return super::hook::call(WH_CBT, HCBT_MINMAX, hwnd, cmd, 30);
                }
                continue;
            }
            30 => {
                // The CBT hook may keep the window as it is.
                if value != 0 && super::hook::has(cpu, WH_CBT) {
                    t.step = 9;
                    continue;
                }
                if t.var(C_STYLE) & WS_MINIMIZE != 0 {
                    t.step = 31;
                    continue;
                }
                let mm = t.scratch + CREATE_MINMAX;
                write_min_max(cpu, hwnd, mm);
                return send(hwnd, WM_GETMINMAXINFO, 0, mm, 31);
            }
            31 => {
                let new = if t.var(C_STYLE) & WS_MINIMIZE != 0 {
                    minimize(cpu, hwnd)
                } else {
                    // Where and how big WM_GETMINMAXINFO says.
                    maximize(cpu, hwnd);
                    let mm = read_words(cpu, t.scratch + CREATE_MINMAX, 6);
                    let (cx, cy) = (mm[2] as i32, mm[3] as i32);
                    Rect::sized(mm[4] as i32, mm[5] as i32, cx, cy)
                };
                let swp = SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_STATECHANGED;
                let (x, y) = (new.left as u32, new.top as u32);
                let (cx, cy) = (new.width() as u32, new.height() as u32);
                let args = [hwnd, HWND_TOP, x, y, cx, cy, swp];
                return Next::call(address_of("SetWindowPos"), &args, 9);
            }
            9 => {
                // WM_PARENTNOTIFY goes to the parent, and on up through
                // parents that are child windows without
                // WS_EX_NOPARENTNOTIFY.
                let state = user(cpu);
                let Some(w) = state.window(hwnd) else {
                    return Next::Done(0);
                };
                if w.style & WS_CHILD != 0 && w.ex_style & WS_EX_NOPARENTNOTIFY == 0 {
                    let id = w.id;
                    let parent = w.parent;
                    t.set(C_NOTIFY, parent);
                    let wp = WM_CREATE | (id & 0xFFFF) << 16;
                    t.set(C_PARENT, wp);
                    return send(parent, WM_PARENTNOTIFY, wp, hwnd, 10);
                }
                t.step = 11;
            }
            10 => {
                let state = user(cpu);
                let at = t.var(C_NOTIFY);
                match state.window(at) {
                    Some(p)
                        if p.style & WS_CHILD != 0
                            && p.ex_style & WS_EX_NOPARENTNOTIFY == 0
                            && p.parent != DESKTOP =>
                    {
                        let next = p.parent;
                        t.set(C_NOTIFY, next);
                        return send(next, WM_PARENTNOTIFY, t.var(C_PARENT), hwnd, 10);
                    }
                    _ => t.step = 11,
                }
            }
            11 => {
                if !user(cpu).is_window(hwnd) {
                    return Next::Done(0);
                }
                let style = t.var(C_STYLE);
                if style & WS_VISIBLE != 0 {
                    let cmd = if style & WS_MINIMIZE != 0 {
                        SW_SHOWMINIMIZED
                    } else if style & WS_MAXIMIZE != 0 {
                        SW_SHOWMAXIMIZED
                    } else {
                        SW_SHOW
                    };
                    return Next::call(address_of("ShowWindow"), &[hwnd, cmd], 12);
                }
                t.step = 12;
            }
            12 | 13 => {
                // A visible MDI child: its client's Window menu, and
                // the child activated, which ShowWindow doesn't do.
                let w = user(cpu).window(hwnd);
                let mdi = w.filter(|w| {
                    t.var(C_STYLE) & WS_VISIBLE != 0 && w.ex_style & WS_EX_MDICHILD != 0
                });
                if let Some(client) = mdi.map(|w| w.parent) {
                    let calls = super::controls::mdi::child_shown(hwnd, client);
                    let (at, args) = &calls[(t.step - 12) as usize];
                    return Next::call(*at, args, t.step + 1);
                }
                t.step = 14;
            }
            14 => {
                return Next::Done(if user(cpu).is_window(hwnd) { hwnd } else { 0 });
            }
            20 => {
                free_window(cpu, hwnd);
                return Next::Done(0);
            }
            21 => return Next::Done(0),
            _ => return Next::Done(0),
        }
    }
}

/// Send `msg` to `hwnd` through SendMessageA, going on at `step`.
pub fn send(hwnd: u32, msg: u32, wp: u32, lp: u32, step: u16) -> Next {
    Next::call(address_of("SendMessageA"), &[hwnd, msg, wp, lp], step)
}

/// Fill the MINMAXINFO at `at` for `hwnd`, as WM_GETMINMAXINFO's sender
/// does: the maximized size and place, and the tracking limits.
pub fn write_min_max(cpu: &mut Cpu, hwnd: u32, at: u32) {
    let m = super::sysparams::metric;
    let (sw, sh) = super::sysparams::screen_size(cpu);
    // SM_CXMAXTRACK and SM_CYMAXTRACK.
    let max_track = [59, 60].map(|i| super::sysparams::screen_metric(cpu, i) as u32);
    let state = user(cpu);
    let Some(w) = state.window(hwnd) else {
        return;
    };
    let (style, ex) = (w.style, w.ex_style);
    // The maximized window's frame is off the screen.
    let frame = max_frame(style, ex);
    let (xinc, yinc) = (-frame.left, -frame.top);
    let (max_w, max_h) = if style & WS_CHILD != 0 {
        let parent = state.window(w.parent).map(|p| p.client).unwrap_or_default();
        (parent.width() + 2 * xinc, parent.height() + 2 * yinc)
    } else {
        (sw + 2 * xinc, sh + 2 * yinc)
    };
    // Without a border, a window can shrink to its frame.
    let (min_w, min_h) = if style & (WS_DLGFRAME | WS_BORDER) != 0 {
        (m(34), m(35))
    } else {
        (2 * xinc, 2 * yinc)
    };
    let words = [
        0,
        0,
        max_w as u32,
        max_h as u32,
        (-xinc) as u32,
        (-yinc) as u32,
        min_w as u32,
        min_h as u32,
        max_track[0],
        max_track[1],
    ];
    write_words(cpu, at, &words);
}

/// Free the window `hwnd` and what it holds, with no messages: its
/// children must be gone. Its timers stop and the focus, capture and
/// activation leave it.
pub fn free_window(cpu: &mut Cpu, hwnd: u32) {
    super::message::kill_window_timers(cpu, hwnd);
    crate::win32::opengl32::wgl::window_destroyed(cpu, hwnd);
    let state = user(cpu);
    let Some(slot) = super::slot(hwnd) else {
        return;
    };
    let Some(w) = state.windows.get(slot).and_then(|w| w.clone()) else {
        return;
    };
    for &child in &w.children {
        if let Some(c) = state.window_mut(child) {
            c.parent = DESKTOP;
        }
    }
    unlink(state, hwnd);
    for other in state.windows.iter_mut().flatten() {
        if other.owner == hwnd {
            other.owner = 0;
        }
        if other.last_popup == hwnd {
            other.last_popup = 0;
        }
    }
    if let Some(c) = state
        .classes
        .get_mut(w.class as usize)
        .and_then(|c| c.as_mut())
    {
        c.windows = c.windows.saturating_sub(1);
    }
    state.forget_activation(|h| h == hwnd);
    let input = &mut state.input;
    if input.capture == hwnd {
        input.capture = 0;
    }
    if state.caret.hwnd == hwnd {
        state.caret = Default::default();
    }
    state.windows[slot] = None;
    if w.style & WS_CHILD == 0 && w.id != 0 {
        super::menu::destroy(cpu, w.id);
    }
    if w.sysmenu != 0 {
        super::menu::destroy(cpu, w.sysmenu);
    }
    if w.dc != 0 {
        crate::win32::gdi32::dc::free_window_dc(cpu, w.dc);
    }
    crate::win32::gdi32::dc::window_gone(cpu, hwnd);
    super::paint::tree_changed(cpu);
    if w.style & WS_VISIBLE != 0 {
        super::paint::expose(cpu, &Region::from_rect(screen_rect_of(&w, cpu)));
    }
}

/// The window's rectangle on the screen, from what it was before it went.
fn screen_rect_of(w: &Window, cpu: &Cpu) -> Rect {
    let (x, y) = cpu.bus.win32.user.client_origin(w.parent);
    w.rect.offset(x, y)
}

/// The variables of DestroyWindow's task.
const D_HWND: usize = 0;
const D_CHILD: usize = 1;
const D_AT: usize = 2;
const D_LIST: usize = 3;

/// DestroyWindow(hwnd).
pub fn destroy_window(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hwnd = args[0];
    let state = user(cpu);
    let Some(w) = state.window(hwnd).cloned() else {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(FALSE);
    };
    if hwnd == DESKTOP {
        set_error(cpu, super::ERROR_INVALID_WINDOW_HANDLE);
        return Some(FALSE);
    }
    if w.tid != current_tid(cpu) {
        set_error(cpu, 5);
        return Some(FALSE);
    }
    let child = w.style & WS_CHILD != 0;
    task::start(cpu, TASK_DESTROY, vec![hwnd, child as u32, 0], 0)
}

/// DestroyWindow's steps.
pub fn destroy_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let hwnd = t.var(D_HWND);
    loop {
        match t.step {
            0 => {
                if super::hook::has(cpu, WH_CBT) {
                    return super::hook::call(WH_CBT, HCBT_DESTROYWND, hwnd, 0, 1);
                }
                t.step = 2;
            }
            1 => {
                if value != 0 {
                    return Next::Done(FALSE);
                }
                t.step = 2;
            }
            2 => {
                let state = user(cpu);
                let Some(w) = state.window(hwnd) else {
                    return Next::Done(TRUE);
                };
                if w.style & WS_CHILD != 0 && w.ex_style & WS_EX_NOPARENTNOTIFY == 0 {
                    let wp = WM_DESTROY | (w.id & 0xFFFF) << 16;
                    let parent = w.parent;
                    t.set(D_AT, parent);
                    return send(parent, WM_PARENTNOTIFY, wp, hwnd, 3);
                }
                t.step = 4;
            }
            3 => {
                let state = user(cpu);
                let at = t.var(D_AT);
                let id = state.window(hwnd).map_or(0, |w| w.id);
                match state.window(at) {
                    Some(p)
                        if p.style & WS_CHILD != 0
                            && p.ex_style & WS_EX_NOPARENTNOTIFY == 0
                            && p.parent != DESKTOP =>
                    {
                        let next = p.parent;
                        t.set(D_AT, next);
                        let wp = WM_DESTROY | (id & 0xFFFF) << 16;
                        return send(next, WM_PARENTNOTIFY, wp, hwnd, 3);
                    }
                    _ => t.step = 4,
                }
            }
            4 => {
                let state = user(cpu);
                let Some(w) = state.window(hwnd) else {
                    return Next::Done(TRUE);
                };
                if w.style & WS_VISIBLE != 0 {
                    if t.var(D_CHILD) != 0 {
                        return Next::call(address_of("ShowWindow"), &[hwnd, SW_HIDE], 5);
                    }
                    let flags =
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_HIDEWINDOW;
                    return Next::call(
                        address_of("SetWindowPos"),
                        &[hwnd, 0, 0, 0, 0, 0, flags],
                        5,
                    );
                }
                t.step = 5;
            }
            5 => {
                // Owned windows go first; those of other threads lose
                // their owner.
                let state = user(cpu);
                if !state.is_window(hwnd) {
                    return Next::Done(TRUE);
                }
                if t.var(D_CHILD) == 0 {
                    let tid = current_tid(cpu);
                    let state = user(cpu);
                    let owned = state
                        .windows
                        .iter()
                        .enumerate()
                        .filter_map(|(i, w)| w.as_ref().map(|w| (super::handle(i), w)))
                        .find(|(_, w)| w.owner == hwnd && w.flags & DESTROYING == 0)
                        .map(|(h, w)| (h, w.tid));
                    match owned {
                        Some((h, owner_tid)) if owner_tid == tid => {
                            if let Some(w) = state.window_mut(h) {
                                w.flags |= DESTROYING;
                            }
                            return Next::call(address_of("DestroyWindow"), &[h], 5);
                        }
                        Some((h, _)) => {
                            if let Some(w) = state.window_mut(h) {
                                w.owner = 0;
                            }
                            continue;
                        }
                        None => {}
                    }
                }
                // WM_DESTROY to the window, then to its children, each
                // before its own children.
                let list = pre_order(user(cpu), hwnd);
                for &h in &list {
                    if let Some(w) = user(cpu).window_mut(h) {
                        w.flags |= DESTROYING;
                    }
                }
                destroy_focus(cpu, &list);
                t.vars.truncate(D_LIST);
                t.set(D_AT, 0);
                t.vars.extend(list);
                t.step = 6;
            }
            6 => {
                let at = t.var(D_AT) as usize;
                let Some(&h) = t.vars.get(D_LIST + at) else {
                    t.step = 7;
                    // Children before their parents now.
                    let list: Vec<u32> = t.vars[D_LIST..].iter().rev().copied().collect();
                    t.vars.truncate(D_LIST);
                    t.vars.extend(list);
                    t.set(D_AT, 0);
                    continue;
                };
                t.set(D_AT, at as u32 + 1);
                let state = user(cpu);
                let fresh = state
                    .window(h)
                    .is_some_and(|w| w.flags & IN_DESTROY_MSG == 0);
                if state.is_window(h) && (h == hwnd || fresh) {
                    if let Some(w) = user(cpu).window_mut(h) {
                        w.flags |= IN_DESTROY_MSG;
                    }
                    super::input::destroy_caret_of(cpu, h);
                    return send(h, WM_DESTROY, 0, 0, 6);
                }
            }
            7 => {
                let at = t.var(D_AT) as usize;
                let Some(&h) = t.vars.get(D_LIST + at) else {
                    return Next::Done(TRUE);
                };
                t.set(D_AT, at as u32 + 1);
                let state = user(cpu);
                if let Some(w) = state.window_mut(h)
                    && w.flags & NC_DESTROYED == 0
                {
                    w.flags |= NC_DESTROYED;
                    return send(h, WM_NCDESTROY, 0, 0, 8);
                }
            }
            8 => {
                // Free the window WM_NCDESTROY went to.
                let at = t.var(D_AT) as usize;
                let h = t.vars[D_LIST + at - 1];
                if user(cpu).is_window(h) {
                    free_window(cpu, h);
                }
                t.step = 7;
            }
            _ => return Next::Done(TRUE),
        }
    }
}

/// The focus leaves windows that are going: no WM_KILLFOCUS, as on
/// Windows.
fn destroy_focus(cpu: &mut Cpu, list: &[u32]) {
    let state = user(cpu);
    for a in &mut state.input.queues {
        if list.contains(&a.focus) {
            a.focus = 0;
        }
    }
    if list.contains(&state.input.capture) {
        state.input.capture = 0;
    }
}

/// `hwnd` and its descendants, each before its own children.
pub fn pre_order(state: &State, hwnd: u32) -> Vec<u32> {
    let mut out = Vec::new();
    // Depth first: the children go on the stack last first, so the first
    // comes off next.
    let mut stack = vec![hwnd];
    while let Some(h) = stack.pop() {
        out.push(h);
        if let Some(w) = state.window(h) {
            stack.extend(w.children.iter().rev());
        }
    }
    out
}

/// The variables of ShowWindow's task.
const S_HWND: usize = 0;
const S_CMD: usize = 1;
const S_WAS: usize = 2;
const S_FLAGS: usize = 3;
const S_RECT: usize = 4;
/// Whether WM_GETMINMAXINFO filled the MINMAXINFO in the scratch space.
const S_MINMAX: usize = 8;
const SHOW_SCRATCH: u32 = 40;

/// ShowWindow(hwnd, command): whether it was visible.
pub fn show_window(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, cmd) = (args[0], args[1]);
    let Some(w) = user(cpu).window(hwnd) else {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(FALSE);
    };
    if hwnd == DESKTOP {
        return Some(FALSE);
    }
    let was = w.style & WS_VISIBLE != 0;
    let vars = vec![hwnd, cmd, was as u32, 0, 0, 0, 0, 0, 0];
    task::start(cpu, TASK_SHOW, vars, SHOW_SCRATCH)
}

/// ShowWindow's steps.
pub fn show_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let hwnd = t.var(S_HWND);
    let cmd = t.var(S_CMD);
    let was = t.var(S_WAS) != 0;
    loop {
        match t.step {
            0 => {
                // A change between minimized, maximized and restored goes
                // by the CBT hook, and a window to maximize is asked its
                // size.
                let style = user(cpu).window(hwnd).map_or(0, |w| w.style);
                let changes = match cmd {
                    SW_SHOWMINNOACTIVE | SW_MINIMIZE | SW_FORCEMINIMIZE | SW_SHOWMINIMIZED => {
                        style & WS_MINIMIZE == 0
                    }
                    SW_SHOWMAXIMIZED => style & WS_MAXIMIZE == 0,
                    SW_HIDE | SW_SHOWNA | SW_SHOW => false,
                    _ => style & (WS_MINIMIZE | WS_MAXIMIZE) != 0,
                };
                if !changes {
                    t.step = 30;
                    continue;
                }
                t.step = 20;
                if super::hook::has(cpu, WH_CBT) {
                    return super::hook::call(WH_CBT, HCBT_MINMAX, hwnd, cmd, 20);
                }
            }
            20 => {
                if value != 0 && super::hook::has(cpu, WH_CBT) {
                    return Next::Done(was as u32);
                }
                if cmd != SW_SHOWMAXIMIZED {
                    t.step = 30;
                    continue;
                }
                write_min_max(cpu, hwnd, t.scratch);
                t.set(S_MINMAX, 1);
                return send(hwnd, WM_GETMINMAXINFO, 0, t.scratch, 30);
            }
            30 => {
                let state = user(cpu);
                let Some(w) = state.window(hwnd) else {
                    return Next::Done(was as u32);
                };
                let (style, rect) = (w.style, w.rect);
                let child = style & WS_CHILD != 0;
                let active = state.is_active(hwnd);
                let mut swp = 0;
                let mut new = rect;
                let mut show = true;
                match cmd {
                    SW_HIDE => {
                        if !was {
                            return Next::Done(FALSE);
                        }
                        show = false;
                        swp |= SWP_HIDEWINDOW | SWP_NOSIZE | SWP_NOMOVE;
                        if !active {
                            swp |= SWP_NOACTIVATE | SWP_NOZORDER;
                        }
                    }
                    SW_SHOWMINNOACTIVE | SW_MINIMIZE | SW_FORCEMINIMIZE | SW_SHOWMINIMIZED => {
                        swp |= SWP_SHOWWINDOW | SWP_FRAMECHANGED;
                        if cmd != SW_SHOWMINIMIZED {
                            swp |= SWP_NOACTIVATE | SWP_NOZORDER;
                        }
                        if style & WS_MINIMIZE == 0 {
                            new = minimize(cpu, hwnd);
                            swp |= SWP_STATECHANGED;
                        } else {
                            swp |= SWP_NOSIZE | SWP_NOMOVE;
                        }
                    }
                    SW_SHOWMAXIMIZED | 3 if cmd == SW_SHOWMAXIMIZED => {
                        swp |= SWP_SHOWWINDOW | SWP_FRAMECHANGED;
                        if style & WS_MAXIMIZE == 0 {
                            new = maximize(cpu, hwnd);
                            if t.var(S_MINMAX) != 0 {
                                // Where and how big WM_GETMINMAXINFO says.
                                let mm = read_words(cpu, t.scratch, 6);
                                let (cx, cy) = (mm[2] as i32, mm[3] as i32);
                                new = Rect::sized(mm[4] as i32, mm[5] as i32, cx, cy);
                            }
                            swp |= SWP_STATECHANGED;
                        } else {
                            swp |= SWP_NOSIZE | SWP_NOMOVE;
                        }
                    }
                    SW_SHOWNA => {
                        swp |= SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOSIZE | SWP_NOMOVE;
                        if child {
                            swp |= SWP_NOZORDER;
                        }
                    }
                    SW_SHOW => {
                        if was {
                            return Next::Done(TRUE);
                        }
                        swp |= SWP_SHOWWINDOW | SWP_NOSIZE | SWP_NOMOVE;
                        if child {
                            swp |= SWP_NOACTIVATE | SWP_NOZORDER;
                        }
                    }
                    _ => {
                        // SW_SHOWNOACTIVATE, SW_RESTORE, SW_SHOWNORMAL,
                        // SW_SHOWDEFAULT and the rest.
                        if cmd == SW_SHOWNOACTIVATE {
                            swp |= SWP_NOACTIVATE | SWP_NOZORDER;
                        }
                        swp |= SWP_SHOWWINDOW;
                        if style & (WS_MINIMIZE | WS_MAXIMIZE) != 0 {
                            new = restore(cpu, hwnd);
                            swp |= SWP_FRAMECHANGED | SWP_STATECHANGED;
                        } else {
                            swp |= SWP_NOSIZE | SWP_NOMOVE;
                        }
                        if child {
                            swp |= SWP_NOACTIVATE | SWP_NOZORDER;
                        }
                    }
                }
                t.set(S_FLAGS, swp);
                t.set(S_RECT, new.left as u32);
                t.set(S_RECT + 1, new.top as u32);
                t.set(S_RECT + 2, new.width() as u32);
                t.set(S_RECT + 3, new.height() as u32);
                if (show != was || cmd == SW_SHOWNA)
                    && cmd != SW_SHOWMAXIMIZED
                    && swp & SWP_STATECHANGED == 0
                {
                    return send(hwnd, WM_SHOWWINDOW, show as u32, 0, 1);
                }
                t.step = 1;
            }
            1 => {
                if !user(cpu).is_window(hwnd) {
                    return Next::Done(was as u32);
                }
                let mut swp = t.var(S_FLAGS) & !SWP_STATECHANGED;
                let child = user(cpu)
                    .window(hwnd)
                    .is_some_and(|w| w.style & WS_CHILD != 0);
                if child && cmd != SW_MAXIMIZE {
                    swp |= SWP_NOACTIVATE | SWP_NOZORDER;
                }
                let state_changed = t.var(S_FLAGS) & SWP_STATECHANGED;
                let args = [
                    hwnd,
                    HWND_TOP,
                    t.var(S_RECT),
                    t.var(S_RECT + 1),
                    t.var(S_RECT + 2),
                    t.var(S_RECT + 3),
                    swp | state_changed,
                ];
                return Next::call(address_of("SetWindowPos"), &args, 2);
            }
            2 => {
                if cmd == SW_HIDE {
                    let focus = super::input::focus(cpu);
                    let state = user(cpu);
                    if focus == hwnd || state.is_descendant(hwnd, focus) {
                        let mut parent = state.parent(hwnd);
                        if parent == DESKTOP {
                            parent = 0;
                        }
                        t.step = 5;
                        return Next::call(address_of("SetFocus"), &[parent], 5);
                    }
                    return Next::Done(was as u32);
                }
                let state = user(cpu);
                let Some(w) = state.window_mut(hwnd) else {
                    return Next::Done(was as u32);
                };
                if w.flags & NEED_SIZE != 0 {
                    w.flags &= !NEED_SIZE;
                    let wp = if w.style & WS_MAXIMIZE != 0 {
                        SIZE_MAXIMIZED
                    } else if w.style & WS_MINIMIZE != 0 {
                        SIZE_MINIMIZED
                    } else {
                        SIZE_RESTORED
                    };
                    let size = super::make_long(w.client.width(), w.client.height());
                    return send(hwnd, WM_SIZE, wp, size, 3);
                }
                t.step = 4;
            }
            3 => {
                let client = user(cpu).window(hwnd).map(|w| w.client).unwrap_or_default();
                return send(
                    hwnd,
                    WM_MOVE,
                    0,
                    super::make_long(client.left, client.top),
                    4,
                );
            }
            _ => return Next::Done(was as u32),
        }
    }
}

/// Make the window minimized (its style; WM_GETMINMAXINFO isn't asked):
/// the rectangle it then has.
fn minimize(cpu: &mut Cpu, hwnd: u32) -> Rect {
    let m = super::sysparams::metric;
    let state = user(cpu);
    let Some(w) = state.window_mut(hwnd) else {
        return Rect::default();
    };
    if w.style & WS_MAXIMIZE == 0 {
        w.normal = w.rect;
    }
    w.style = (w.style | WS_MINIMIZE) & !WS_MAXIMIZE;
    let (x, y) = if w.icon_pos.0 >= 0 {
        w.icon_pos
    } else {
        let (_, sh) = super::sysparams::screen_size(cpu);
        (0, sh - m(58))
    };
    Rect::sized(x, y, m(57), m(58))
}

fn maximize(cpu: &mut Cpu, hwnd: u32) -> Rect {
    let (sw, sh) = super::sysparams::screen_size(cpu);
    let state = user(cpu);
    let Some(w) = state.window(hwnd) else {
        return Rect::default();
    };
    let (style, ex, parent) = (w.style, w.ex_style, w.parent);
    let frame = max_frame(style, ex);
    let area = if style & WS_CHILD != 0 {
        state.window(parent).map(|p| p.client).unwrap_or_default()
    } else {
        Rect::new(0, 0, sw, sh)
    };
    let area = Rect::sized(0, 0, area.width(), area.height());
    let w = state.window_mut(hwnd).unwrap();
    if w.style & WS_MINIMIZE == 0 {
        w.normal = w.rect;
    }
    w.style = (w.style | WS_MAXIMIZE) & !WS_MINIMIZE;
    Rect::new(
        area.left + frame.left,
        area.top + frame.top,
        area.right + frame.right,
        area.bottom + frame.bottom,
    )
}

fn restore(cpu: &mut Cpu, hwnd: u32) -> Rect {
    let Some(w) = user(cpu).window_mut(hwnd) else {
        return Rect::default();
    };
    w.style &= !(WS_MINIMIZE | WS_MAXIMIZE);
    w.normal
}

/// SetWindowPos's scratch space: the WINDOWPOS, then WM_NCCALCSIZE's
/// NCCALCSIZE_PARAMS (three rectangles and a pointer to the WINDOWPOS).
const P_WINPOS: u32 = 0;
const P_CALC: u32 = 28;
const P_SCRATCH: u32 = 28 + 52;

/// The variables of SetWindowPos's task: the window, its old rectangles
/// and its old style, and whether it was active.
const P_HWND: usize = 0;
const P_OLD: usize = 1;
const P_OLD_CLIENT: usize = 5;
const P_WAS_VISIBLE: usize = 9;

/// SetWindowPos(hwnd, after, x, y, cx, cy, flags).
pub fn set_window_pos(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, after, x, y, cx, cy, flags] = args else {
        unreachable!()
    };
    let state = user(cpu);
    let Some(w) = state.window(hwnd) else {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(FALSE);
    };
    if hwnd == DESKTOP {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    let (style, rect, client) = (w.style, w.rect, w.client);
    let mut flags = flags & !(SWP_NOCLIENTSIZE | SWP_NOCLIENTMOVE);
    let state_changed = flags & SWP_STATECHANGED;
    flags &= !SWP_STATECHANGED;
    let visible = style & WS_VISIBLE != 0;
    // What doesn't change isn't done.
    if visible {
        flags &= !SWP_SHOWWINDOW;
    } else {
        if flags & SWP_SHOWWINDOW == 0 {
            flags |= SWP_NOREDRAW;
        }
        flags &= !SWP_HIDEWINDOW;
    }
    let (cx, cy) = (cx as i32, cy as i32);
    let (cx, cy) = (cx.max(0), cy.max(0));
    if rect.width() == cx && rect.height() == cy {
        flags |= SWP_NOSIZE;
    }
    if rect.left == x as i32 && rect.top == y as i32 {
        flags |= SWP_NOMOVE;
    }
    let child = style & WS_CHILD != 0;
    // A window already active in the foreground isn't activated again.
    // The active window of a queue in the background is: the activation
    // brings it to the foreground if SetForegroundWindow may, as when a
    // program's message box made active behind the foreground (its
    // SetFocus came first) is then shown.
    if child || state.foreground_window() == hwnd {
        flags |= SWP_NOACTIVATE;
    }
    let mut after = after;
    if flags & SWP_NOZORDER == 0
        && after != HWND_TOP
        && after != HWND_BOTTOM
        && after != HWND_TOPMOST
        && after != HWND_NOTOPMOST
    {
        if !state.is_window(after) || state.parent(after) != state.parent(hwnd) {
            if !state.is_window(after) {
                set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
                return Some(FALSE);
            }
            after = HWND_TOP;
        }
        if after == hwnd {
            flags |= SWP_NOZORDER;
        }
    }
    let words = [
        hwnd,
        after,
        x,
        y,
        cx as u32,
        cy as u32,
        flags | state_changed,
    ];
    let mut vars = vec![hwnd];
    vars.extend([rect.left, rect.top, rect.right, rect.bottom].map(|v| v as u32));
    vars.extend([client.left, client.top, client.right, client.bottom].map(|v| v as u32));
    vars.push(visible as u32);
    vars.extend(words);
    task::start(cpu, TASK_SET_POS, vars, P_SCRATCH)
}

/// SetWindowPos's steps.
pub fn set_pos_step(cpu: &mut Cpu, t: &mut Task, _value: u32) -> Next {
    let hwnd = t.var(P_HWND);
    let winpos = t.scratch + P_WINPOS;
    loop {
        match t.step {
            0 => {
                let words: Vec<u32> = t.vars[P_WAS_VISIBLE + 1..].to_vec();
                t.vars.truncate(P_WAS_VISIBLE + 1);
                write_words(cpu, winpos, &words);
                if words[6] & SWP_NOSENDCHANGING == 0 {
                    // The flags it sees are without SWP_STATECHANGED's
                    // own place in them.
                    return send(hwnd, WM_WINDOWPOSCHANGING, 0, winpos, 1);
                }
                t.step = 1;
            }
            1 => {
                if !user(cpu).is_window(hwnd) {
                    return Next::Done(FALSE);
                }
                let p = read_words(cpu, winpos, 7);
                let flags = p[6];
                let old = rect_var(t, P_OLD);
                let (x, y, cx, cy) = (p[2] as i32, p[3] as i32, p[4] as i32, p[5] as i32);
                let mut new = old;
                if flags & SWP_NOMOVE == 0 {
                    new = new.offset(x - old.left, y - old.top);
                }
                if flags & SWP_NOSIZE == 0 {
                    new.right = new.left + cx.max(0);
                    new.bottom = new.top + cy.max(0);
                }
                t.vars
                    .extend([new.left, new.top, new.right, new.bottom].map(|v| v as u32));
                if flags & SWP_NOSIZE == 0 || flags & SWP_FRAMECHANGED != 0 {
                    // WM_NCCALCSIZE with the new rectangle, the old one,
                    // the old client area and the WINDOWPOS.
                    let calc = t.scratch + P_CALC;
                    let old_client = rect_var(t, P_OLD_CLIENT);
                    let mut bytes = Vec::new();
                    bytes.extend_from_slice(&new.bytes());
                    bytes.extend_from_slice(&old.bytes());
                    bytes.extend_from_slice(&old_client.bytes());
                    bytes.extend_from_slice(&winpos.to_le_bytes());
                    cpu.bus.guest_write_bytes(calc, &bytes);
                    return send(hwnd, WM_NCCALCSIZE, TRUE, calc, 2);
                }
                // The client area moves with the window.
                let old_client = rect_var(t, P_OLD_CLIENT);
                let client = old_client.offset(new.left - old.left, new.top - old.top);
                t.vars.extend(
                    [client.left, client.top, client.right, client.bottom].map(|v| v as u32),
                );
                t.step = 3;
            }
            2 => {
                let client = Rect::read(cpu, t.scratch + P_CALC);
                t.vars.extend(
                    [client.left, client.top, client.right, client.bottom].map(|v| v as u32),
                );
                t.step = 3;
            }
            3 => {
                // Move the window, change its place among its siblings,
                // show or hide it.
                let p = read_words(cpu, winpos, 7);
                let mut flags = p[6];
                let after = p[1];
                let new = rect_var(t, P_WAS_VISIBLE + 1);
                let client = rect_var(t, P_WAS_VISIBLE + 5);
                let old_client = rect_var(t, P_OLD_CLIENT);
                let old_screen = user(cpu).screen_rect(hwnd);
                let was_visible = user(cpu).is_visible(hwnd);
                let state = user(cpu);
                let under = super::input::under_cursor(state);
                // What siblings above it cover, which a raise shows.
                let was_covered = if was_visible && flags & (SWP_NOZORDER | SWP_NOREDRAW) == 0 {
                    super::paint::covered(state, hwnd)
                } else {
                    Region::new()
                };
                let Some(w) = state.window_mut(hwnd) else {
                    return Next::Done(FALSE);
                };
                w.rect = new;
                w.client = client;
                if w.style & (WS_MINIMIZE | WS_MAXIMIZE) == 0 {
                    w.normal = new;
                }
                if flags & SWP_SHOWWINDOW != 0 {
                    w.style |= WS_VISIBLE;
                }
                if flags & SWP_HIDEWINDOW != 0 {
                    w.style &= !WS_VISIBLE;
                }
                if flags & SWP_NOZORDER == 0 {
                    if after == HWND_TOPMOST {
                        w.ex_style |= WS_EX_TOPMOST;
                    } else if after == HWND_NOTOPMOST {
                        w.ex_style &= !WS_EX_TOPMOST;
                    }
                    link(state, hwnd, after);
                    raise_owned(state, hwnd);
                }
                if client.width() == old_client.width() && client.height() == old_client.height() {
                    flags |= SWP_NOCLIENTSIZE;
                }
                let client_moved = client.left != old_client.left || client.top != old_client.top;
                if !client_moved {
                    flags |= SWP_NOCLIENTMOVE;
                }
                super::paint::tree_changed(cpu);
                // What it uncovered and what it shows now need painting.
                let now_visible = user(cpu).is_visible(hwnd);
                if flags & SWP_NOREDRAW == 0 || flags & (SWP_SHOWWINDOW | SWP_HIDEWINDOW) != 0 {
                    super::paint::moved(cpu, hwnd, old_screen, was_visible, now_visible, flags);
                }
                // Moved or shown, it is painted whole; raised in place,
                // where the windows above it were.
                if now_visible
                    && flags & SWP_NOREDRAW == 0
                    && !was_covered.is_empty()
                    && user(cpu).screen_rect(hwnd) == old_screen
                {
                    super::paint::raised(cpu, hwnd, &was_covered);
                }
                if flags & SWP_HIDEWINDOW != 0 {
                    let state = user(cpu);
                    if state.input.capture == hwnd || state.is_descendant(hwnd, state.input.capture)
                    {
                        state.input.capture = 0;
                    }
                }
                // The window under the cursor gets a move if the window
                // moved, resized, showed or hid (`input::window_moved`
                // says which changes count). After paint::moved, which
                // puts the desktop on the screen as a program's first
                // window shows (`under` is 0 then, as nothing was under
                // the cursor), and after a hidden window lost the
                // capture, so the move goes to the window under the
                // cursor and not to the one hidden.
                // Its place counts as changed if its place on the screen
                // or its client area's did, not because the flags asked
                // for a move: a WM_WINDOWPOSCHANGING that keeps the window
                // where it is gives no move, which a program answering
                // the move by moving the window again would otherwise get
                // forever.
                let changed = user(cpu).screen_rect(hwnd) != old_screen
                    || client_moved
                    || was_visible != now_visible;
                super::input::window_moved(cpu, hwnd, under, changed);
                cpu.bus.guest_write_32(winpos + 24, flags);
                // Activate it.
                let child = user(cpu)
                    .window(hwnd)
                    .is_some_and(|w| w.style & WS_CHILD != 0);
                if flags & SWP_HIDEWINDOW != 0 && user(cpu).is_active(hwnd) {
                    t.step = 4;
                    return Next::call(address_of("ActivateOtherWindow"), &[hwnd], 4);
                }
                if flags & (SWP_NOACTIVATE | SWP_HIDEWINDOW) == 0 && !child {
                    return Next::call(address_of("SetForegroundWindow"), &[hwnd], 4);
                }
                if child && flags & SWP_NOACTIVATE == 0 && flags & SWP_SHOWWINDOW != 0 {
                    return send(hwnd, WM_CHILDACTIVATE, 0, 0, 4);
                }
                t.step = 4;
            }
            4 => {
                if !user(cpu).is_window(hwnd) {
                    return Next::Done(TRUE);
                }
                // A top-level window shown is erased now (WM_NCPAINT,
                // WM_ERASEBKGND), after its activation and before
                // WM_WINDOWPOSCHANGED and its WM_PAINT, unless
                // SWP_DEFERERASE says not to: a program that draws with
                // GetDC in WM_PAINT keeps what it drew, as on Windows. Its
                // children, and a child window shown, wait for their
                // BeginPaint.
                let flags = cpu.bus.guest_read_32(winpos + 24);
                let child = user(cpu)
                    .window(hwnd)
                    .is_some_and(|w| w.style & WS_CHILD != 0);
                if flags & SWP_SHOWWINDOW != 0
                    && flags & SWP_DEFERERASE == 0
                    && !child
                    && user(cpu).is_visible(hwnd)
                {
                    let erase = super::paint::RDW_ERASENOW | super::paint::RDW_NOCHILDREN;
                    return Next::call(address_of("RedrawWindow"), &[hwnd, 0, 0, erase], 5);
                }
                t.step = 5;
            }
            5 => {
                if !user(cpu).is_window(hwnd) {
                    return Next::Done(TRUE);
                }
                let flags = cpu.bus.guest_read_32(winpos + 24);
                const AGG: u32 = SWP_NOSIZE
                    | SWP_NOMOVE
                    | SWP_NOZORDER
                    | SWP_SHOWWINDOW
                    | SWP_HIDEWINDOW
                    | SWP_FRAMECHANGED
                    | SWP_NOCLIENTSIZE
                    | SWP_NOCLIENTMOVE;
                let nothing =
                    SWP_NOSIZE | SWP_NOMOVE | SWP_NOZORDER | SWP_NOCLIENTSIZE | SWP_NOCLIENTMOVE;
                if flags & AGG != nothing && flags & SWP_NOSENDCHANGING == 0 {
                    return send(hwnd, WM_WINDOWPOSCHANGED, 0, winpos, 6);
                }
                return Next::Done(TRUE);
            }
            _ => return Next::Done(TRUE),
        }
    }
}

fn rect_var(t: &Task, at: usize) -> Rect {
    Rect::new(
        t.var(at) as i32,
        t.var(at + 1) as i32,
        t.var(at + 2) as i32,
        t.var(at + 3) as i32,
    )
}

/// MoveWindow(hwnd, x, y, width, height, repaint).
pub fn move_window(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut flags = SWP_NOZORDER | SWP_NOACTIVATE;
    if args[5] == 0 {
        flags |= SWP_NOREDRAW;
    }
    set_window_pos(
        cpu,
        &[args[0], 0, args[1], args[2], args[3], args[4], flags],
    )
}

/// The window `hwnd`, or the last error set for a bad handle.
fn get(cpu: &mut Cpu, hwnd: u32) -> Option<&Window> {
    if !user(cpu).is_window(hwnd) {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return None;
    }
    user(cpu).window(hwnd)
}

pub fn is_window(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(user(cpu).is_window(args[0]) as u32)
}

pub fn is_window_visible(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(user(cpu).is_visible(args[0]) as u32)
}

pub fn is_window_enabled(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(get(cpu, args[0]).is_some_and(|w| w.style & WS_DISABLED == 0) as u32)
}

/// EnableWindow(hwnd, enable): whether it was disabled. A window that is
/// disabled loses the focus and the capture; WM_ENABLE tells it.
pub fn enable_window(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, enable) = (args[0], args[1] != 0);
    let Some(w) = get(cpu, hwnd) else {
        return Some(FALSE);
    };
    let was_disabled = w.style & WS_DISABLED != 0;
    if enable == !was_disabled {
        return Some(was_disabled as u32);
    }
    let focus = super::input::focus(cpu);
    let state = user(cpu);
    if let Some(w) = state.window_mut(hwnd) {
        if enable {
            w.style &= !WS_DISABLED;
        } else {
            w.style |= WS_DISABLED;
        }
    }
    if !enable && (focus == hwnd || state.is_descendant(hwnd, focus)) {
        // The focus goes nowhere, without messages here; WM_ENABLE
        // follows.
        task::start(
            cpu,
            TASK_MISC,
            vec![MISC_ENABLE, hwnd, enable as u32, was_disabled as u32, 1],
            0,
        )
    } else {
        task::start(
            cpu,
            TASK_MISC,
            vec![MISC_ENABLE, hwnd, enable as u32, was_disabled as u32, 0],
            0,
        )
    }
}

/// The small tasks of this module, by their first variable.
const MISC_ENABLE: u32 = 0;
const MISC_SET_TEXT: u32 = 1;
const MISC_SET_PARENT: u32 = 2;
const MISC_CLOSE: u32 = 3;
const MISC_SET_LONG: u32 = 4;
const MISC_PLACEMENT: u32 = 5;

/// The small tasks' steps.
pub fn misc_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    match t.var(0) {
        MISC_ENABLE => {
            let hwnd = t.var(1);
            match t.step {
                0 if t.var(4) != 0 => Next::call(address_of("SetFocus"), &[0], 1),
                0 | 1 => send(hwnd, WM_ENABLE, t.var(2), 0, 2),
                _ => Next::Done(t.var(3)),
            }
        }
        MISC_SET_TEXT => match t.step {
            // SetWindowTextA: WM_SETTEXT.
            0 => send(t.var(1), WM_SETTEXT, 0, t.var(2), 1),
            _ => Next::Done(value),
        },
        MISC_SET_PARENT => set_parent_step(cpu, t, value),
        MISC_CLOSE => match t.step {
            0 => send(t.var(1), WM_SYSCOMMAND, SC_MINIMIZE, 0, 1),
            _ => Next::Done(TRUE),
        },
        MISC_SET_LONG => match t.step {
            // SetWindowLongA of the style: WM_STYLECHANGING, the change,
            // WM_STYLECHANGED. The STYLESTRUCT is in the scratch space.
            0 => {
                let (hwnd, which) = (t.var(1), t.var(2));
                let ss = t.scratch;
                write_words(cpu, ss, &[t.var(3), t.var(4)]);
                send(hwnd, WM_STYLECHANGING, which, ss, 1)
            }
            1 => {
                let (hwnd, which) = (t.var(1), t.var(2));
                let ss = t.scratch;
                let new = cpu.bus.guest_read_32(ss + 4);
                let state = user(cpu);
                let Some(w) = state.window_mut(hwnd) else {
                    return Next::Done(0);
                };
                if which as i32 == GWL_STYLE {
                    // WS_VISIBLE changes the window's state without
                    // painting it.
                    w.style = new;
                } else {
                    w.ex_style = new;
                }
                super::paint::tree_changed(cpu);
                send(hwnd, WM_STYLECHANGED, which, ss, 2)
            }
            _ => Next::Done(t.var(3)),
        },
        MISC_PLACEMENT => match t.step {
            0 => {
                let (hwnd, cmd) = (t.var(1), t.var(2));
                Next::call(address_of("ShowWindow"), &[hwnd, cmd], 1)
            }
            _ => Next::Done(TRUE),
        },
        _ => Next::Done(0),
    }
}

/// IsChild(parent, hwnd).
pub fn is_child(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(user(cpu).is_descendant(args[0], args[1]) as u32)
}

pub fn is_iconic(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(
        user(cpu)
            .window(args[0])
            .is_some_and(|w| w.style & WS_MINIMIZE != 0) as u32,
    )
}

pub fn is_zoomed(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(
        user(cpu)
            .window(args[0])
            .is_some_and(|w| w.style & WS_MAXIMIZE != 0) as u32,
    )
}

/// GetParent: a child's parent, a popup's owner, or 0.
pub fn get_parent(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(parent_of(user(cpu), args[0]))
}

/// GetParent's answer: a popup's owner (even if it is a child too), a
/// child's parent, nothing for an overlapped window.
fn parent_of(state: &State, hwnd: u32) -> u32 {
    let Some(w) = state.window(hwnd) else {
        return 0;
    };
    if w.style & WS_POPUP != 0 {
        w.owner
    } else if w.style & WS_CHILD != 0 {
        w.parent
    } else {
        0
    }
}

/// GWL_HWNDPARENT: a window's parent, or the owner of one whose parent
/// is the desktop, whatever its style says.
fn hwnd_parent_of(w: &Window) -> u32 {
    if w.parent == DESKTOP || w.parent == 0 {
        w.owner
    } else {
        w.parent
    }
}

/// GetAncestor(hwnd, GA_PARENT, GA_ROOT or GA_ROOTOWNER).
pub fn get_ancestor(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, kind) = (args[0], args[1]);
    let state = user(cpu);
    if !state.is_window(hwnd) {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(0);
    }
    if hwnd == DESKTOP {
        return Some(0);
    }
    Some(match kind {
        1 => state.parent(hwnd),
        2 => {
            let mut h = hwnd;
            while state.parent(h) != DESKTOP && state.parent(h) != 0 {
                h = state.parent(h);
            }
            h
        }
        3 => {
            // Up GetParent's chain as far as it goes.
            let mut h = hwnd;
            loop {
                let parent = parent_of(state, h);
                if parent == 0 || parent == h {
                    break h;
                }
                h = parent;
            }
        }
        _ => 0,
    })
}

/// SetParent(hwnd, parent): the old parent. The window is hidden while it
/// moves, as Windows does, and keeps its place relative to its new
/// parent's client area.
pub fn set_parent(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, parent) = (args[0], args[1]);
    let state = user(cpu);
    if !state.is_window(hwnd) || hwnd == DESKTOP {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(0);
    }
    let parent = if parent == 0 { DESKTOP } else { parent };
    if !state.is_window(parent) {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(0);
    }
    if parent == hwnd || state.is_descendant(hwnd, parent) {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let old = state.parent(hwnd);
    let visible = state
        .window(hwnd)
        .is_some_and(|w| w.style & WS_VISIBLE != 0);
    task::start(
        cpu,
        TASK_MISC,
        vec![MISC_SET_PARENT, hwnd, parent, old, visible as u32],
        0,
    )
}

fn set_parent_step(cpu: &mut Cpu, t: &mut Task, _value: u32) -> Next {
    let (hwnd, parent, old, visible) = (t.var(1), t.var(2), t.var(3), t.var(4) != 0);
    match t.step {
        0 if visible => Next::call(address_of("ShowWindow"), &[hwnd, SW_HIDE], 1),
        0 | 1 => {
            let state = user(cpu);
            unlink(state, hwnd);
            if let Some(w) = state.window_mut(hwnd) {
                w.parent = parent;
                if parent == DESKTOP && w.style & WS_CHILD == 0 {
                    w.owner = 0;
                }
            }
            link(state, hwnd, HWND_TOP);
            super::paint::tree_changed(cpu);
            // It keeps its position relative to its new parent, at the
            // top of the z-order, and shows again if it showed.
            let flags = SWP_NOSIZE | SWP_NOMOVE | if visible { SWP_SHOWWINDOW } else { 0 };
            Next::call(
                address_of("SetWindowPos"),
                &[hwnd, HWND_TOP, 0, 0, 0, 0, flags],
                2,
            )
        }
        _ => Next::Done(if old == DESKTOP { DESKTOP } else { old }),
    }
}

/// GetWindow(hwnd, relation).
pub fn get_window(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, rel) = (args[0], args[1]);
    let state = user(cpu);
    let Some(w) = state.window(hwnd) else {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(0);
    };
    let siblings = state
        .window(w.parent)
        .map(|p| p.children.clone())
        .unwrap_or_default();
    let at = siblings.iter().position(|&h| h == hwnd);
    Some(match rel {
        GW_HWNDFIRST => {
            if hwnd == DESKTOP {
                0
            } else {
                siblings.first().copied().unwrap_or(0)
            }
        }
        GW_HWNDLAST => {
            if hwnd == DESKTOP {
                0
            } else {
                siblings.last().copied().unwrap_or(0)
            }
        }
        GW_HWNDNEXT => at.and_then(|i| siblings.get(i + 1)).copied().unwrap_or(0),
        GW_HWNDPREV => at
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| siblings.get(i))
            .copied()
            .unwrap_or(0),
        GW_OWNER => w.owner,
        GW_CHILD => w.children.first().copied().unwrap_or(0),
        GW_ENABLEDPOPUP => 0,
        _ => {
            set_error(cpu, super::ERROR_INVALID_FLAGS);
            0
        }
    })
}

pub fn get_top_window(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hwnd = if args[0] == 0 { DESKTOP } else { args[0] };
    let Some(w) = get(cpu, hwnd) else {
        return Some(0);
    };
    Some(w.children.first().copied().unwrap_or(0))
}

pub fn get_desktop_window(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    super::init(cpu);
    Some(DESKTOP)
}

/// The offsets of DWL_ values in a dialog's extra bytes.
pub const DWL_MSGRESULT: i32 = 0;
pub const DWL_DLGPROC: i32 = 4;
pub const DWL_USER: i32 = 8;

/// GetWindowLongA(hwnd, index).
pub fn get_window_long(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, index) = (args[0], args[1] as i32);
    let Some(w) = get(cpu, hwnd) else {
        return Some(0);
    };
    let value = match index {
        GWL_WNDPROC => w.proc,
        GWL_HINSTANCE => w.instance,
        GWL_HWNDPARENT => hwnd_parent_of(w),
        GWL_ID => w.id,
        GWL_STYLE => w.style,
        GWL_EXSTYLE => w.ex_style,
        GWL_USERDATA => w.user_data,
        i if i >= 0 && (i as usize + 4) <= w.extra.len() => {
            let i = i as usize;
            u32::from_le_bytes(w.extra[i..i + 4].try_into().unwrap())
        }
        _ => {
            set_error(cpu, ERROR_INVALID_INDEX);
            return Some(0);
        }
    };
    Some(value)
}

/// GetWindowWord(hwnd, index): the extra bytes, or GWL_ID and
/// GWL_HINSTANCE as words.
pub fn get_window_word(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, index) = (args[0], args[1] as i32);
    let Some(w) = get(cpu, hwnd) else {
        return Some(0);
    };
    let value = match index {
        GWL_ID => w.id & 0xFFFF,
        GWL_HINSTANCE => w.instance & 0xFFFF,
        GWL_HWNDPARENT => hwnd_parent_of(w),
        i if i >= 0 && (i as usize + 2) <= w.extra.len() => {
            let i = i as usize;
            u16::from_le_bytes(w.extra[i..i + 2].try_into().unwrap()) as u32
        }
        _ => {
            set_error(cpu, ERROR_INVALID_INDEX);
            return Some(0);
        }
    };
    Some(value)
}

/// SetWindowLongA(hwnd, index, value): the old value. The style goes
/// through WM_STYLECHANGING and WM_STYLECHANGED.
pub fn set_window_long(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, index, value) = (args[0], args[1] as i32, args[2]);
    let Some(w) = get(cpu, hwnd).cloned() else {
        return Some(0);
    };
    // The desktop is the system's, and another process's procedure stays.
    if hwnd == DESKTOP || (w.pid != current_pid(cpu) && index == GWL_WNDPROC) {
        set_error(cpu, 5);
        return Some(0);
    }
    if index == GWL_WNDPROC && value == 0 {
        // Windows 95 keeps the procedure, and says nothing.
        return Some(0);
    }
    let old = match index {
        GWL_STYLE | GWL_EXSTYLE => {
            let old = if index == GWL_STYLE {
                w.style
            } else {
                w.ex_style
            };
            let mut new = value;
            if index == GWL_EXSTYLE {
                // WS_EX_WINDOWEDGE follows the frame.
                let style = w.style;
                new = fix_styles(style | WS_CHILD, new).1;
                if style & WS_CHILD == 0 {
                    new = fix_styles(style, value).1;
                }
            }
            return task::start(
                cpu,
                TASK_MISC,
                vec![MISC_SET_LONG, hwnd, index as u32, old, new],
                8,
            );
        }
        GWL_WNDPROC => {
            let w = user(cpu).window_mut(hwnd).unwrap();
            std::mem::replace(&mut w.proc, value)
        }
        GWL_HINSTANCE => {
            let w = user(cpu).window_mut(hwnd).unwrap();
            std::mem::replace(&mut w.instance, value)
        }
        GWL_HWNDPARENT => {
            // The owner of a window on the desktop, else the parent.
            let w = user(cpu).window_mut(hwnd).unwrap();
            if w.parent != DESKTOP && w.parent != 0 {
                return set_parent(cpu, &[hwnd, value]);
            }
            std::mem::replace(&mut w.owner, value)
        }
        GWL_ID => {
            let w = user(cpu).window_mut(hwnd).unwrap();
            std::mem::replace(&mut w.id, value)
        }
        GWL_USERDATA => {
            let w = user(cpu).window_mut(hwnd).unwrap();
            std::mem::replace(&mut w.user_data, value)
        }
        i if i >= 0 && (i as usize + 4) <= w.extra.len() => {
            let w = user(cpu).window_mut(hwnd).unwrap();
            let i = i as usize;
            let old = u32::from_le_bytes(w.extra[i..i + 4].try_into().unwrap());
            w.extra[i..i + 4].copy_from_slice(&value.to_le_bytes());
            old
        }
        _ => {
            set_error(cpu, ERROR_INVALID_INDEX);
            return Some(0);
        }
    };
    Some(old)
}

/// SetWindowWord(hwnd, index, value): the extra bytes, or the id.
pub fn set_window_word(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, index, value) = (args[0], args[1] as i32, args[2] & 0xFFFF);
    let Some(w) = get(cpu, hwnd) else {
        return Some(0);
    };
    let len = w.extra.len();
    let w = user(cpu).window_mut(hwnd).unwrap();
    match index {
        GWL_ID => {
            let old = w.id & 0xFFFF;
            w.id = (w.id & 0xFFFF_0000) | value;
            Some(old)
        }
        GWL_HINSTANCE => {
            let old = w.instance & 0xFFFF;
            w.instance = (w.instance & 0xFFFF_0000) | value;
            Some(old)
        }
        i if i >= 0 && (i as usize + 2) <= len => {
            let i = i as usize;
            let old = u16::from_le_bytes(w.extra[i..i + 2].try_into().unwrap()) as u32;
            w.extra[i..i + 2].copy_from_slice(&(value as u16).to_le_bytes());
            Some(old)
        }
        _ => {
            set_error(cpu, ERROR_INVALID_INDEX);
            Some(0)
        }
    }
}

/// GetWindowRect(hwnd, rect): its rectangle on the screen.
pub fn get_window_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if get(cpu, args[0]).is_none() {
        return Some(FALSE);
    }
    let r = user(cpu).screen_rect(args[0]);
    r.write(cpu, args[1]);
    Some(TRUE)
}

/// GetClientRect(hwnd, rect): its client area's size.
pub fn get_client_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(w) = get(cpu, args[0]) else {
        return Some(FALSE);
    };
    let r = Rect::new(0, 0, w.client.width(), w.client.height());
    r.write(cpu, args[1]);
    Some(TRUE)
}

pub fn client_to_screen(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if get(cpu, args[0]).is_none() {
        return Some(FALSE);
    }
    let (x, y) = user(cpu).client_origin(args[0]);
    let px = cpu.bus.guest_read_32(args[1]) as i32;
    let py = cpu.bus.guest_read_32(args[1] + 4) as i32;
    write_words(cpu, args[1], &[(px + x) as u32, (py + y) as u32]);
    Some(TRUE)
}

pub fn screen_to_client(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if get(cpu, args[0]).is_none() {
        return Some(FALSE);
    }
    let (x, y) = user(cpu).client_origin(args[0]);
    let px = cpu.bus.guest_read_32(args[1]) as i32;
    let py = cpu.bus.guest_read_32(args[1] + 4) as i32;
    write_words(cpu, args[1], &[(px - x) as u32, (py - y) as u32]);
    Some(TRUE)
}

/// MapWindowPoints(from, to, points, count): the offset it moved them by,
/// as LOWORD x and HIWORD y.
pub fn map_window_points(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (from, to, points, count) = (args[0], args[1], args[2], args[3]);
    let state = user(cpu);
    for h in [from, to] {
        if h != 0 && !state.is_window(h) {
            set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
            return Some(0);
        }
    }
    let origin = |h: u32| {
        if h == 0 || h == DESKTOP {
            (0, 0)
        } else {
            state.client_origin(h)
        }
    };
    let (fx, fy) = origin(from);
    let (tx, ty) = origin(to);
    let (dx, dy) = (fx - tx, fy - ty);
    for i in 0..count {
        let at = points + 8 * i;
        let px = cpu.bus.guest_read_32(at) as i32;
        let py = cpu.bus.guest_read_32(at + 4) as i32;
        write_words(cpu, at, &[(px + dx) as u32, (py + dy) as u32]);
    }
    Some(super::make_long(dx, dy))
}

/// GetWindowTextA(hwnd, buffer, size): a window of this process's text
/// through WM_GETTEXT, another's from where USER keeps it.
pub fn get_window_text(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, buffer, size) = (args[0], args[1], args[2]);
    // Sent to another thread's window, the function runs again when the
    // reply comes, with the text in the buffer. It only takes the reply.
    if super::message::waiting_for_reply(cpu) {
        return send_message_now(cpu, hwnd, WM_GETTEXT, size, buffer);
    }
    let Some(w) = get(cpu, hwnd).cloned() else {
        if size > 0 && buffer != 0 {
            cpu.bus.guest_write_8(buffer, 0);
        }
        return Some(0);
    };
    if size as i32 <= 0 || buffer == 0 {
        return Some(0);
    }
    if w.pid != current_pid(cpu) {
        let text = w.text.clone();
        return Some(super::copy_out(cpu, buffer, size, &text));
    }
    cpu.bus.guest_write_8(buffer, 0);
    send_message_now(cpu, hwnd, WM_GETTEXT, size, buffer)
}

/// Send a message from a function's handler, which returns what the
/// window procedure returns: through SendMessageA's own task.
fn send_message_now(cpu: &mut Cpu, hwnd: u32, msg: u32, wp: u32, lp: u32) -> Option<u32> {
    super::message::send_message(cpu, &[hwnd, msg, wp, lp])
}

/// SetWindowTextA(hwnd, text): WM_SETTEXT.
pub fn set_window_text(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, text) = (args[0], args[1]);
    if get(cpu, hwnd).is_none() {
        return Some(FALSE);
    }
    task::start(cpu, TASK_MISC, vec![MISC_SET_TEXT, hwnd, text], 0)
}

/// GetWindowTextLengthA(hwnd): WM_GETTEXTLENGTH.
pub fn get_window_text_length(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hwnd = args[0];
    let Some(w) = get(cpu, hwnd).cloned() else {
        return Some(0);
    };
    if w.pid != current_pid(cpu) {
        return Some(w.text.len() as u32);
    }
    send_message_now(cpu, hwnd, WM_GETTEXTLENGTH, 0, 0)
}

/// The class name and window name a find matches: None matches any.
fn finds(state: &State, hwnd: u32, class: &Option<Name>, title: &Option<Vec<u8>>) -> bool {
    let Some(w) = state.window(hwnd) else {
        return false;
    };
    if let Some(class) = class
        && !super::class::matches(state, w.class, class)
    {
        return false;
    }
    if let Some(title) = title
        && !w.text.eq_ignore_ascii_case(title)
    {
        return false;
    }
    true
}

/// FindWindowA(class, title).
pub fn find_window(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    find_window_ex(cpu, &[0, 0, args[0], args[1]])
}

/// FindWindowExA(parent, after, class, title).
pub fn find_window_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (parent, after, class, title) = (args[0], args[1], args[2], args[3]);
    super::init(cpu);
    let class = if class == 0 {
        None
    } else {
        Some(Name::read(cpu, class)?)
    };
    let title = if title == 0 {
        None
    } else {
        Some(super::bytes_at(cpu, title)?)
    };
    let parent = if parent == 0 { DESKTOP } else { parent };
    // Program Manager's window is there for a program that looks for it
    // as well as for one that broadcasts WM_DDE_INITIATE.
    let progman = match &class {
        Some(Name::Text(name)) => name.eq_ignore_ascii_case(super::progman::CLASS),
        Some(Name::Id(_)) => false,
        None => title
            .as_ref()
            .is_some_and(|t| t.eq_ignore_ascii_case(super::progman::TITLE)),
    };
    if parent == DESKTOP && progman {
        super::progman::server(cpu);
    }
    let state = user(cpu);
    let Some(p) = state.window(parent) else {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(0);
    };
    let children = p.children.clone();
    let start = if after == 0 {
        0
    } else {
        match children.iter().position(|&h| h == after) {
            Some(i) => i + 1,
            None => return Some(0),
        }
    };
    let found = children[start..]
        .iter()
        .copied()
        .find(|&h| finds(state, h, &class, &title));
    if found.is_none() {
        set_error(cpu, 2);
    }
    Some(found.unwrap_or(0))
}

/// The enumerations' kinds.
const ENUM_WINDOWS: u32 = 0;
const ENUM_CHILDREN: u32 = 1;
const ENUM_THREAD: u32 = 2;
const ENUM_PROPS: u32 = 3;

/// The variables of an enumeration: its kind, the callback, its
/// argument, the next window's place, then the windows.
const E_KIND: usize = 0;
const E_PROC: usize = 1;
const E_PARAM: usize = 2;
const E_AT: usize = 3;
const E_LIST: usize = 4;

/// EnumWindows(proc, param): the top-level windows.
pub fn enum_windows(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    super::init(cpu);
    let list = user(cpu).top_levels();
    let mut vars = vec![ENUM_WINDOWS, args[0], args[1], 0];
    vars.extend(list);
    task::start(cpu, TASK_ENUM, vars, 0)
}

/// EnumChildWindows(parent, proc, param): its descendants, each before
/// its own children. FALSE, without a call, when it has none.
pub fn enum_child_windows(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    super::init(cpu);
    let parent = if args[0] == 0 { DESKTOP } else { args[0] };
    let state = user(cpu);
    if !state.is_window(parent) {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(FALSE);
    }
    let mut list = pre_order(state, parent);
    list.remove(0);
    if list.is_empty() {
        return Some(FALSE);
    }
    let mut vars = vec![ENUM_CHILDREN, args[1], args[2], 0];
    vars.extend(list);
    task::start(cpu, TASK_ENUM, vars, 0)
}

/// EnumThreadWindows(tid, proc, param): the thread's top-level windows.
pub fn enum_thread_windows(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    super::init(cpu);
    let tid = args[0];
    let state = user(cpu);
    let list: Vec<u32> = state
        .top_levels()
        .into_iter()
        .filter(|&h| state.window(h).is_some_and(|w| w.tid == tid))
        .collect();
    let mut vars = vec![ENUM_THREAD, args[1], args[2], 0];
    vars.extend(list);
    task::start(cpu, TASK_ENUM, vars, 0)
}

/// EnumPropsA(hwnd, proc): proc(hwnd, name, data) for each property.
pub fn enum_props(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    props(cpu, args[0], args[1], 0, false)
}

/// EnumPropsExA(hwnd, proc, param): proc(hwnd, name, data, param).
pub fn enum_props_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    props(cpu, args[0], args[1], args[2], true)
}

/// The enumeration of a window's properties: -1 when it has none. A
/// name is on the stack, in the scratch space, while the callback runs.
fn props(cpu: &mut Cpu, hwnd: u32, proc: u32, param: u32, ex: bool) -> Option<u32> {
    let Some(w) = get(cpu, hwnd) else {
        return Some(-1i32 as u32);
    };
    if w.props.is_empty() {
        return Some(-1i32 as u32);
    }
    let size = w
        .props
        .iter()
        .map(|(k, _)| k.len() as u32 + 1)
        .max()
        .unwrap_or(0);
    let vars = vec![ENUM_PROPS, proc, param, 0, hwnd, ex as u32];
    task::start(cpu, TASK_ENUM, vars, size.next_multiple_of(4))
}

/// The enumerations' steps: the callback for each window until one
/// returns FALSE.
pub fn enum_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    if t.step == 1 && value == 0 {
        return Next::Done(if t.var(E_KIND) == ENUM_PROPS {
            0
        } else {
            FALSE
        });
    }
    if t.var(E_KIND) == ENUM_PROPS {
        let hwnd = t.var(E_LIST);
        let at = t.var(E_AT) as usize;
        let props = user(cpu)
            .window(hwnd)
            .map(|w| w.props.clone())
            .unwrap_or_default();
        let Some((name, data)) = props.get(at).cloned() else {
            return Next::Done(value);
        };
        t.set(E_AT, at as u32 + 1);
        let key = match name.strip_prefix('#').and_then(|n| n.parse::<u32>().ok()) {
            Some(atom) => atom,
            None => {
                let mut bytes = crate::dosstr::to_bytes(&name);
                bytes.push(0);
                cpu.bus.guest_write_bytes(t.scratch, &bytes);
                t.scratch
            }
        };
        if t.var(E_LIST + 1) != 0 {
            return Next::call(t.var(E_PROC), &[hwnd, key, data, t.var(E_PARAM)], 1);
        }
        return Next::call(t.var(E_PROC), &[hwnd, key, data], 1);
    }
    loop {
        let at = t.var(E_AT) as usize;
        let Some(&hwnd) = t.vars.get(E_LIST + at) else {
            return Next::Done(TRUE);
        };
        t.set(E_AT, at as u32 + 1);
        if user(cpu).is_window(hwnd) {
            return Next::call(t.var(E_PROC), &[hwnd, t.var(E_PARAM)], 1);
        }
    }
}

/// GetWindowThreadProcessId(hwnd, pid).
pub fn get_window_thread_process_id(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(w) = get(cpu, args[0]) else {
        return Some(0);
    };
    let (tid, pid) = (w.tid, w.pid);
    let (tid, pid) = if args[0] == DESKTOP {
        (current_tid(cpu), current_pid(cpu))
    } else {
        (tid, pid)
    };
    if args[1] != 0 {
        cpu.bus.guest_write_32(args[1], pid);
    }
    Some(tid)
}

/// The visible, enabled window at the screen point (`x`, `y`): the
/// deepest child there. As WindowFromPoint and the mouse's messages do,
/// a child that answers WM_NCHITTEST with HTTRANSPARENT (a group box, a
/// static control) is passed over for the windows below it, so the
/// check boxes inside a group box get their clicks.
pub fn window_at(state: &State, x: i32, y: i32) -> u32 {
    let mut hwnd = DESKTOP;
    'down: loop {
        let Some(w) = state.window(hwnd) else {
            return hwnd;
        };
        for &child in &w.children {
            let Some(c) = state.window(child) else {
                continue;
            };
            if c.style & WS_VISIBLE == 0 {
                continue;
            }
            if !state.screen_rect(child).contains(x, y) {
                continue;
            }
            if c.ex_style & WS_EX_TRANSPARENT != 0 && c.style & WS_CHILD != 0 {
                continue;
            }
            if c.style & WS_CHILD != 0 && super::controls::hit_transparent(state, child) {
                continue;
            }
            hwnd = child;
            if c.style & WS_DISABLED != 0 || c.style & WS_MINIMIZE != 0 {
                return hwnd;
            }
            if state.screen_client(child).contains(x, y) {
                continue 'down;
            }
            return hwnd;
        }
        return hwnd;
    }
}

/// WindowFromPoint(x, y).
pub fn window_from_point(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    super::init(cpu);
    let state = user(cpu);
    let hwnd = window_at(state, args[0] as i32, args[1] as i32);
    Some(if hwnd == DESKTOP { DESKTOP } else { hwnd })
}

/// ChildWindowFromPoint(parent, x, y): the child of `parent` at the point
/// in its client coordinates, or the parent itself.
pub fn child_window_from_point(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    child_window_from_point_ex(cpu, &[args[0], args[1], args[2], 0])
}

/// ChildWindowFromPointEx(parent, x, y, flags): CWP_SKIPINVISIBLE (1),
/// CWP_SKIPDISABLED (2), CWP_SKIPTRANSPARENT (4).
pub fn child_window_from_point_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (parent, x, y, flags) = (args[0], args[1] as i32, args[2] as i32, args[3]);
    let Some(w) = get(cpu, parent).cloned() else {
        return Some(0);
    };
    let client = Rect::new(0, 0, w.client.width(), w.client.height());
    if !client.contains(x, y) {
        return Some(0);
    }
    let state = user(cpu);
    for &child in &w.children.clone() {
        let Some(c) = state.window(child) else {
            continue;
        };
        if flags & 1 != 0 && c.style & WS_VISIBLE == 0 {
            continue;
        }
        if flags & 2 != 0 && c.style & WS_DISABLED != 0 {
            continue;
        }
        if flags & 4 != 0 && c.ex_style & WS_EX_TRANSPARENT != 0 {
            continue;
        }
        if c.rect.contains(x, y) {
            return Some(child);
        }
    }
    Some(parent)
}

/// AdjustWindowRect(rect, style, menu).
pub fn adjust_window_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    adjust_window_rect_ex(cpu, &[args[0], args[1], args[2], 0])
}

/// AdjustWindowRectEx(rect, style, menu, ex_style).
pub fn adjust_window_rect_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = Rect::read(cpu, args[0]);
    let (style, ex) = (args[1], args[3]);
    // Windows 95 ignores WS_EX_WINDOWEDGE here as it adds it itself.
    let out = adjust(r, style, ex, args[2] != 0);
    out.write(cpu, args[0]);
    Some(TRUE)
}

/// GetWindowPlacement(hwnd, placement).
pub fn get_window_placement(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(w) = get(cpu, args[0]) else {
        return Some(FALSE);
    };
    let show = if w.style & WS_MINIMIZE != 0 {
        SW_SHOWMINIMIZED
    } else if w.style & WS_MAXIMIZE != 0 {
        SW_SHOWMAXIMIZED
    } else {
        SW_SHOWNORMAL
    };
    let normal = if w.style & (WS_MINIMIZE | WS_MAXIMIZE) != 0 {
        w.normal
    } else {
        w.rect
    };
    let icon = w.icon_pos;
    let words = [
        44,
        0,
        show,
        icon.0 as u32,
        icon.1 as u32,
        -1i32 as u32,
        -1i32 as u32,
    ];
    write_words(cpu, args[1], &words);
    normal.write(cpu, args[1] + 28);
    Some(TRUE)
}

/// SetWindowPlacement(hwnd, placement).
pub fn set_window_placement(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hwnd = args[0];
    if get(cpu, hwnd).is_none() {
        return Some(FALSE);
    }
    let words = read_words(cpu, args[1], 7);
    let normal = Rect::read(cpu, args[1] + 28);
    let state = user(cpu);
    let w = state.window_mut(hwnd).unwrap();
    w.normal = normal;
    w.icon_pos = (words[3] as i32, words[4] as i32);
    if w.style & (WS_MINIMIZE | WS_MAXIMIZE) == 0 {
        let flags = SWP_NOZORDER | SWP_NOACTIVATE;
        let r = normal;
        let cmd = words[2];
        if cmd == SW_SHOWNORMAL || cmd == SW_RESTORE {
            return set_window_pos(
                cpu,
                &[
                    hwnd,
                    0,
                    r.left as u32,
                    r.top as u32,
                    r.width() as u32,
                    r.height() as u32,
                    flags,
                ],
            );
        }
    }
    task::start(cpu, TASK_MISC, vec![MISC_PLACEMENT, hwnd, words[2]], 0)
}

/// BringWindowToTop(hwnd).
pub fn bring_window_to_top(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    set_window_pos(
        cpu,
        &[args[0], HWND_TOP, 0, 0, 0, 0, SWP_NOSIZE | SWP_NOMOVE],
    )
}

/// CloseWindow(hwnd): minimizes it.
pub fn close_window(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(w) = get(cpu, args[0]) else {
        return Some(FALSE);
    };
    if w.style & WS_CHILD != 0 {
        return Some(FALSE);
    }
    show_window(cpu, &[args[0], SW_MINIMIZE]).map(|_| TRUE)
}

pub fn open_icon(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(w) = get(cpu, args[0]) else {
        return Some(FALSE);
    };
    if w.style & WS_MINIMIZE == 0 {
        return Some(TRUE);
    }
    show_window(cpu, &[args[0], SW_SHOWNORMAL]).map(|_| TRUE)
}

/// A property's key: an atom as "#n", or its name.
fn prop_key(cpu: &mut Cpu, at: u32) -> Option<String> {
    if at < 0x1_0000 {
        return Some(format!("#{}", at));
    }
    Some(string_at(cpu, at)?.to_ascii_uppercase())
}

pub fn get_prop(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let key = prop_key(cpu, args[1])?;
    let Some(w) = user(cpu).window(args[0]) else {
        return Some(0);
    };
    Some(
        w.props
            .iter()
            .find(|(k, _)| *k == key)
            .map_or(0, |&(_, v)| v),
    )
}

pub fn set_prop(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let key = prop_key(cpu, args[1])?;
    let Some(w) = user(cpu).window_mut(args[0]) else {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(FALSE);
    };
    match w.props.iter_mut().find(|(k, _)| *k == key) {
        Some(p) => p.1 = args[2],
        None => w.props.insert(0, (key, args[2])),
    }
    Some(TRUE)
}

pub fn remove_prop(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let key = prop_key(cpu, args[1])?;
    let Some(w) = user(cpu).window_mut(args[0]) else {
        return Some(0);
    };
    match w.props.iter().position(|(k, _)| *k == key) {
        Some(i) => Some(w.props.remove(i).1),
        None => Some(0),
    }
}

pub fn get_last_active_popup(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(w) = get(cpu, args[0]) else {
        return Some(0);
    };
    let popup = w.last_popup;
    Some(if popup != 0 && user(cpu).is_window(popup) {
        popup
    } else {
        args[0]
    })
}

/// AnyPopup: whether a visible owned or popup top-level window exists.
pub fn any_popup(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let state = user(cpu);
    let any = state.top_levels().into_iter().any(|h| {
        state
            .window(h)
            .is_some_and(|w| w.style & WS_VISIBLE != 0 && (w.owner != 0 || w.style & WS_POPUP != 0))
    });
    Some(any as u32)
}

pub fn show_owned_popups(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if get(cpu, args[0]).is_none() {
        return Some(FALSE);
    }
    Some(TRUE)
}

/// DeferWindowPos's handles hold the calls to make, which
/// EndDeferWindowPos makes one after the other.
pub fn begin_defer_window_pos(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if (args[0] as i32) < 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let state = user(cpu);
    state.input.defers.push(Some(Vec::new()));
    Some(0xD000 + state.input.defers.len() as u32 - 1)
}

/// DeferWindowPos(hdwp, hwnd, after, x, y, cx, cy, flags).
pub fn defer_window_pos(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hdwp = args[0];
    if !user(cpu).is_window(args[1]) {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(0);
    }
    let state = user(cpu);
    let Some(Some(list)) = state
        .input
        .defers
        .get_mut(hdwp.wrapping_sub(0xD000) as usize)
    else {
        set_error(cpu, super::ERROR_INVALID_HANDLE);
        return Some(0);
    };
    list.retain(|c| c[0] != args[1]);
    list.push(args[1..8].to_vec());
    Some(hdwp)
}

/// EndDeferWindowPos(hdwp): the calls, made by a task.
pub fn end_defer_window_pos(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let state = user(cpu);
    let Some(list) = state
        .input
        .defers
        .get_mut(args[0].wrapping_sub(0xD000) as usize)
        .and_then(|l| l.take())
    else {
        set_error(cpu, super::ERROR_INVALID_HANDLE);
        return Some(FALSE);
    };
    let mut vars = vec![ENUM_DEFER, 0, 0, 0];
    for call in list {
        vars.extend(call);
    }
    task::start(cpu, TASK_ENUM_DEFER, vars, 0)
}

const ENUM_DEFER: u32 = 4;
const TASK_ENUM_DEFER: u16 = 16;

/// EndDeferWindowPos's steps: SetWindowPos for each.
pub fn defer_step(_cpu: &mut Cpu, t: &mut Task, _value: u32) -> Next {
    let at = t.var(E_AT) as usize;
    let Some(call) = t
        .vars
        .get(E_LIST + 7 * at..E_LIST + 7 * at + 7)
        .map(<[u32]>::to_vec)
    else {
        return Next::Done(TRUE);
    };
    t.set(E_AT, at as u32 + 1);
    Next::call(address_of("SetWindowPos"), &call, 1)
}

/// SetWindowRgn(hwnd, region, redraw): the window takes the region,
/// which the program mustn't use any more.
pub fn set_window_rgn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if get(cpu, args[0]).is_none() {
        return Some(FALSE);
    }
    let region = if args[1] == 0 {
        None
    } else {
        let r = crate::win32::gdi32::region_of(cpu, args[1]);
        crate::win32::gdi32::delete_object_now(cpu, args[1]);
        r
    };
    if let Some(w) = user(cpu).window_mut(args[0]) {
        w.region = region;
    }
    super::paint::tree_changed(cpu);
    Some(TRUE)
}

/// GetWindowRgn(hwnd, region): copies the window's region into `region`.
pub fn get_window_rgn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(w) = get(cpu, args[0]) else {
        return Some(0);
    };
    let Some(region) = w.region.clone() else {
        return Some(0);
    };
    let kind = region.kind();
    if !crate::win32::gdi32::set_region(cpu, args[1], region) {
        return Some(0);
    }
    Some(kind)
}

/// GetWindowModuleFileNameA(hwnd, buffer, size).
pub fn get_window_module_file_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(w) = get(cpu, args[0]) else {
        return Some(0);
    };
    let instance = w.instance;
    let path = crate::win32::running(cpu)
        .and_then(|p| p.modules.iter().find(|m| m.base == instance))
        .map(|m| m.path.clone())
        .unwrap_or_default();
    let bytes = crate::dosstr::to_bytes(&path);
    Some(super::copy_out(cpu, args[1], args[2], &bytes))
}

/// GetWindowInfo(hwnd, info).
pub fn get_window_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(w) = get(cpu, args[0]) else {
        return Some(FALSE);
    };
    let (style, ex, class) = (w.style, w.ex_style, w.class);
    let state = user(cpu);
    let window = state.screen_rect(args[0]);
    let client = state.screen_client(args[0]);
    let active = state.is_active(args[0]);
    let atom = state
        .classes
        .get(class as usize)
        .and_then(|c| c.as_ref())
        .map_or(0, |c| c.atom);
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&60u32.to_le_bytes());
    bytes.extend_from_slice(&window.bytes());
    bytes.extend_from_slice(&client.bytes());
    for v in [style, ex, if active { 1 } else { 0 }] {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    let border = (client.left - window.left) as u32;
    bytes.extend_from_slice(&border.to_le_bytes());
    bytes.extend_from_slice(&((client.top - window.top) as u32).to_le_bytes());
    bytes.extend_from_slice(&atom.to_le_bytes());
    bytes.extend_from_slice(&0x0400u16.to_le_bytes());
    cpu.bus.guest_write_bytes(args[1], &bytes);
    Some(TRUE)
}

crate::state_fields!(Window {
    class,
    style,
    ex_style,
    parent,
    owner,
    children,
    rect,
    client,
    text,
    id,
    instance,
    proc,
    user_data,
    extra,
    tid,
    pid,
    props,
    update,
    paint,
    flags,
    normal,
    icon_pos,
    menu,
    sysmenu,
    scroll,
    dc,
    icons,
    region,
    control,
    last_popup,
    help_id
});

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows 98 SE maximizes a WS_OVERLAPPEDWINDOW with a menu bar at
    /// (-4, -4), its 4-pixel thick frame off the screen, and its client
    /// area starts at (0, 38): Win32 and Win16 programs alike.
    #[test]
    fn a_maximized_overlapped_window_hides_its_thick_frame() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        let w = Window {
            style: WS_CAPTION | WS_SYSMENU | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX,
            id: 1,
            ..Window::default()
        };
        let hwnd = user(&mut cpu).new_window(w).expect("a window");
        let at = 0x8000;
        write_min_max(&mut cpu, hwnd, at);
        let mm = read_words(&mut cpu, at, 6);
        assert_eq!((mm[2], mm[3]), (648, 488), "the maximized size");
        assert_eq!(
            (mm[4] as i32, mm[5] as i32),
            (-4, -4),
            "the maximized place"
        );
        let c = client_for(user(&mut cpu), hwnd, Rect::new(-4, -4, 644, 484));
        assert_eq!((c.left, c.top, c.width(), c.height()), (0, 38, 640, 442));
    }

    /// A caption without a thick frame keeps its 3-pixel dialog frame
    /// off the screen.
    #[test]
    fn a_maximized_dialog_frame_window_hides_3_pixels() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        let w = Window {
            style: WS_CAPTION | WS_SYSMENU,
            ..Window::default()
        };
        let hwnd = user(&mut cpu).new_window(w).expect("a window");
        let at = 0x8000;
        write_min_max(&mut cpu, hwnd, at);
        let mm = read_words(&mut cpu, at, 6);
        assert_eq!(
            (mm[2], mm[3], mm[4] as i32, mm[5] as i32),
            (646, 486, -3, -3)
        );
    }

    /// A child with WS_CAPTION | WS_THICKFRAME, an MDI child say, is
    /// maximized to its parent's client area with no frame off it, as
    /// Wine's win.c tests check on Windows.
    #[test]
    fn a_maximized_thick_frame_child_fills_its_parent() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        let parent = Window {
            client: Rect::new(0, 0, 374, 235),
            ..Window::default()
        };
        let parent = user(&mut cpu).new_window(parent).expect("a parent");
        let w = Window {
            style: WS_CHILD | WS_CAPTION | WS_THICKFRAME,
            parent,
            ..Window::default()
        };
        let hwnd = user(&mut cpu).new_window(w).expect("a window");
        let at = 0x8000;
        write_min_max(&mut cpu, hwnd, at);
        let mm = read_words(&mut cpu, at, 6);
        assert_eq!(
            (mm[2], mm[3], mm[4] as i32, mm[5] as i32),
            (374, 235, 0, 0)
        );
    }
}
