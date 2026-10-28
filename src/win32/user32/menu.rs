//! Menus: a window's menu bar, popup menus and the system menu, the
//! functions that build and change them, and the loop that tracks one
//! while the user picks from it.
//!
//! A menu is a list of items in `State::menus`, its handle 0x0002_0000
//! plus four times its place. An item has a type and a state (MFT_ and
//! MFS_ bits, with MF_POPUP in the type when it opens a submenu), its
//! command id, text, bitmaps and data, as Windows keeps them. Where an
//! item is drawn is worked out when it is drawn: a menu bar's items in
//! the coordinates of its window, a popup menu's in those of its #32768
//! window.
//!
//! TrackPopupMenu and the menu bar (DefWindowProcA's SC_MOUSEMENU and
//! SC_KEYMENU) run the same loop, a task with its own GetMessageA loop.
//! It shows the popups as #32768 windows, takes the keyboard and mouse
//! messages for itself and dispatches the others, and posts the command
//! picked to the owner, or returns it with TPM_RETURNCMD. Owner-drawn
//! items are laid out at a fixed size and not drawn: WM_MEASUREITEM and
//! WM_DRAWITEM aren't sent.

use super::message::{
    Msg, SC_CLOSE, SC_MAXIMIZE, SC_MINIMIZE, SC_MOVE, SC_RESTORE, SC_SIZE, WM_CANCELMODE, WM_CHAR,
    WM_COMMAND, WM_CREATE, WM_ENTERMENULOOP, WM_ERASEBKGND, WM_EXITMENULOOP, WM_INITMENU,
    WM_INITMENUPOPUP, WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDBLCLK,
    WM_MENUCHAR, WM_MENUSELECT, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_NCCREATE, WM_NCPAINT, WM_PAINT,
    WM_QUIT, WM_RBUTTONDBLCLK, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSCHAR, WM_SYSCOMMAND,
    WM_SYSDEADCHAR, WM_SYSKEYDOWN,
};
use super::sysparams::{
    self, COLOR_BTNHIGHLIGHT, COLOR_BTNSHADOW, COLOR_GRAYTEXT, COLOR_HIGHLIGHT,
    COLOR_HIGHLIGHTTEXT, COLOR_MENU, COLOR_MENUTEXT, metric,
};
use super::window::{
    HWND_TOPMOST, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SWP_SHOWWINDOW, WS_CHILD, WS_EX_CLIENTEDGE, WS_EX_STATICEDGE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_MAXIMIZE, WS_MAXIMIZEBOX, WS_MINIMIZE, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
    Window,
};
use super::{
    DESKTOP, ERROR_CHILD_WINDOW_MENU, ERROR_INVALID_MENU_HANDLE, ERROR_INVALID_PARAMETER,
    ERROR_INVALID_WINDOW_HANDLE, ERROR_RESOURCE_NAME_NOT_FOUND, FALSE, State, TRUE, address_of,
    bytes_at, current_pid, make_long, paint, point_of, read_words, set_error, text, user,
    write_words,
};
use crate::cpu::Cpu;
use crate::win32::convert::CodePage;
use crate::win32::gdi32::dc::{DCX_USESTYLE, DCX_WINDOW, TRANSPARENT};
use crate::win32::gdi32::{self, Rect, Region, font};
use crate::win32::kernel32;
use crate::win32::task::{self, Next, Task};

/// Menu flags (MF_) and the item types (MFT_) and states (MFS_) they
/// share their bits with.
pub const MF_BYCOMMAND: u32 = 0x0000;
pub const MF_ENABLED: u32 = 0x0000;
pub const MF_GRAYED: u32 = 0x0001;
pub const MF_DISABLED: u32 = 0x0002;
pub const MF_BITMAP: u32 = 0x0004;
pub const MF_CHECKED: u32 = 0x0008;
pub const MF_POPUP: u32 = 0x0010;
pub const MF_MENUBARBREAK: u32 = 0x0020;
pub const MF_MENUBREAK: u32 = 0x0040;
pub const MF_HILITE: u32 = 0x0080;
pub const MF_END: u32 = 0x0080;
pub const MF_OWNERDRAW: u32 = 0x0100;
pub const MF_BYPOSITION: u32 = 0x0400;
pub const MF_SEPARATOR: u32 = 0x0800;
pub const MF_DEFAULT: u32 = 0x1000;
pub const MF_SYSMENU: u32 = 0x2000;
pub const MF_HELP: u32 = 0x4000;
pub const MF_RIGHTJUSTIFY: u32 = 0x4000;
pub const MFT_BITMAP: u32 = MF_BITMAP;
pub const MFT_OWNERDRAW: u32 = MF_OWNERDRAW;
pub const MFT_SEPARATOR: u32 = MF_SEPARATOR;
pub const MFT_RADIOCHECK: u32 = 0x0200;
pub const MFS_CHECKED: u32 = MF_CHECKED;
pub const MFS_DEFAULT: u32 = MF_DEFAULT;
/// The bits of an item's type and state.
const TYPE_MASK: u32 = 0x6B64;
const STATE_MASK: u32 = 0x108B;

/// MENUITEMINFO's fMask bits.
const MIIM_STATE: u32 = 0x0001;
const MIIM_ID: u32 = 0x0002;
const MIIM_SUBMENU: u32 = 0x0004;
const MIIM_CHECKMARKS: u32 = 0x0008;
const MIIM_TYPE: u32 = 0x0010;
const MIIM_DATA: u32 = 0x0020;
const MIIM_STRING: u32 = 0x0040;
const MIIM_BITMAP: u32 = 0x0080;
const MIIM_FTYPE: u32 = 0x0100;
/// MENUITEMINFO's sizes: with hbmpItem, and Windows 95's without it.
const MII_SIZE: u32 = 48;
const MII_SIZE_95: u32 = 44;

/// TrackPopupMenu's flags.
pub const TPM_RIGHTBUTTON: u32 = 0x0002;
pub const TPM_CENTERALIGN: u32 = 0x0004;
pub const TPM_RIGHTALIGN: u32 = 0x0008;
pub const TPM_VCENTERALIGN: u32 = 0x0010;
pub const TPM_BOTTOMALIGN: u32 = 0x0020;
pub const TPM_NONOTIFY: u32 = 0x0080;
pub const TPM_RETURNCMD: u32 = 0x0100;

/// GetMenuDefaultItem's flags.
const GMDI_USEDISABLED: u32 = 0x0001;
const GMDI_GOINTOPOPUPS: u32 = 0x0002;

pub const SC_MOUSEMENU: u32 = 0xF090;
const SC_TASKLIST: u32 = 0xF130;
const HTSYSMENU: u32 = 3;
const MA_NOACTIVATE: u32 = 3;
const RT_MENU: u32 = 4;
const ERROR_MENU_ITEM_NOT_FOUND: u32 = 1456;
const ERROR_POPUP_ALREADY_ACTIVE: u32 = 1446;

const VK_MENU: u32 = 0x12;
const VK_ESCAPE: u32 = 0x1B;
const VK_END: u32 = 0x23;
const VK_HOME: u32 = 0x24;
const VK_LEFT: u32 = 0x25;
const VK_UP: u32 = 0x26;
const VK_RIGHT: u32 = 0x27;
const VK_DOWN: u32 = 0x28;
const VK_F10: u32 = 0x79;

/// Where menu handles start, and the most menus there are at once.
const MENU_BASE: u32 = 0x0002_0000;
const MAX_MENUS: usize = 0x4000;
/// No item.
const NONE: u32 = u32::MAX;
/// The atom of the popup menus' class, #32768.
const POPUP_ATOM: u32 = 0x8000;
/// The edge around a popup menu's items.
const POPUP_BORDER: i32 = 3;
/// How deep submenus go where a function looks through them.
const MAX_DEPTH: u32 = 32;
/// BDR_SUNKENOUTER: the edge of the selected item of a menu bar.
const BDR_SUNKENOUTER: u32 = 0x0002;

/// The menu tasks' place in `task::TASKS`.
const TASK_MENU: u16 = 20;

/// An item of a menu.
#[derive(Clone, Debug, Default)]
pub struct Item {
    /// MFT_ bits, and MF_POPUP when it opens `submenu`.
    pub kind: u32,
    /// MFS_ bits.
    pub state: u32,
    pub id: u32,
    pub submenu: u32,
    /// The bitmaps beside it when it is checked and when it isn't.
    pub checked: u32,
    pub unchecked: u32,
    pub data: u32,
    /// Its text: None for a separator, a bitmap or an owner-drawn item.
    pub text: Option<Vec<u8>>,
    /// The bitmap it shows (MF_BITMAP, MIIM_BITMAP).
    pub bitmap: u32,
    /// Where it was laid out last, and where its text after a tab
    /// starts.
    pub rect: Rect,
    pub tab: i32,
}

/// A menu.
#[derive(Clone, Debug, Default)]
pub struct Menu {
    pub pid: u32,
    /// MF_POPUP for a popup menu, MF_SYSMENU for a system menu.
    pub flags: u32,
    pub items: Vec<Item>,
    /// Its size as it was laid out last.
    pub width: i32,
    pub height: i32,
    /// The window it belongs to: a menu bar's window, or a popup
    /// menu's #32768 window while it shows.
    pub hwnd: u32,
    /// The selected item while it is tracked, or NONE.
    pub focus: u32,
    pub help_id: u32,
}

// The menus.

fn slot(h: u32) -> Option<usize> {
    let offset = h.checked_sub(MENU_BASE)?;
    (offset % 4 == 0).then_some((offset / 4) as usize)
}

fn get(state: &State, h: u32) -> Option<&Menu> {
    state.menus.get(slot(h)?)?.as_ref()
}

/// Whether `h` is a menu.
pub(crate) fn exists(state: &State, h: u32) -> bool {
    get(state, h).is_some()
}

/// Whether the menu `h` (or one of its popups) has an item whose id is
/// `id`.
pub(crate) fn has_command(state: &State, h: u32, id: u32) -> bool {
    find(state, h, id, false).is_some()
}

fn get_mut(state: &mut State, h: u32) -> Option<&mut Menu> {
    state.menus.get_mut(slot(h)?)?.as_mut()
}

fn item(state: &State, (h, pos): (u32, usize)) -> Option<&Item> {
    get(state, h)?.items.get(pos)
}

fn item_mut(state: &mut State, (h, pos): (u32, usize)) -> Option<&mut Item> {
    get_mut(state, h)?.items.get_mut(pos)
}

/// A new, empty menu: its handle, or 0 when there are too many.
fn new_menu(state: &mut State, flags: u32, pid: u32) -> u32 {
    let menu = Menu {
        pid,
        flags,
        focus: NONE,
        ..Menu::default()
    };
    let i = match state.menus.iter().position(|m| m.is_none()) {
        Some(i) => i,
        None if state.menus.len() < MAX_MENUS => {
            state.menus.push(None);
            state.menus.len() - 1
        }
        None => return 0,
    };
    state.menus[i] = Some(menu);
    MENU_BASE + 4 * i as u32
}

/// Free the menu `h` and its submenus.
fn free(state: &mut State, h: u32, depth: u32) {
    let Some(menu) = slot(h)
        .and_then(|i| state.menus.get_mut(i))
        .and_then(Option::take)
    else {
        return;
    };
    if depth < MAX_DEPTH {
        for it in menu.items {
            if it.kind & MF_POPUP != 0 {
                free(state, it.submenu, depth + 1);
            }
        }
    }
}

/// The menu `h` goes, with its submenus: for DestroyWindow.
pub fn destroy(cpu: &mut Cpu, h: u32) {
    free(user(cpu), h, 0);
}

/// The item `pos` of the menu `h`, by its place or by its command. By
/// command, the submenus are looked through first; a popup item with
/// that id is what is found when nothing else is.
fn find(state: &State, h: u32, pos: u32, by_position: bool) -> Option<(u32, usize)> {
    find_in(state, h, pos, by_position, 0)
}

fn find_in(state: &State, h: u32, pos: u32, by_position: bool, depth: u32) -> Option<(u32, usize)> {
    if h == 0xFFFF {
        return None;
    }
    let m = get(state, h)?;
    if by_position {
        return ((pos as usize) < m.items.len()).then_some((h, pos as usize));
    }
    let mut fallback = None;
    for (i, it) in m.items.iter().enumerate() {
        if it.kind & MF_POPUP != 0 {
            if depth < MAX_DEPTH
                && let Some(found) = find_in(state, it.submenu, pos, false, depth + 1)
            {
                return Some(found);
            }
            if it.id == pos {
                fallback = Some((h, i));
            }
        } else if it.id == pos {
            return Some((h, i));
        }
    }
    fallback
}

/// `find`, by MF_BYPOSITION in `flags`, failing with
/// ERROR_INVALID_MENU_HANDLE when `h` isn't a menu.
fn lookup(cpu: &mut Cpu, h: u32, pos: u32, flags: u32) -> Option<(u32, usize)> {
    let state = user(cpu);
    if get(state, h).is_none() {
        set_error(cpu, ERROR_INVALID_MENU_HANDLE);
        return None;
    }
    find(state, h, pos, flags & MF_BYPOSITION != 0)
}

/// A new item in `h` before the item `pos` (by MF_BYPOSITION in
/// `flags`, or by command, at the end when there is no such item):
/// where it is, which may be in a submenu.
fn insert_at(state: &mut State, h: u32, pos: u32, flags: u32) -> Option<(u32, usize)> {
    let count = get(state, h)?.items.len();
    let at = if flags & MF_BYPOSITION != 0 {
        (h, (pos as usize).min(count))
    } else {
        find(state, h, pos, false).unwrap_or((h, count))
    };
    get_mut(state, at.0)?.items.insert(at.1, Item::default());
    Some(at)
}

fn remove_at(state: &mut State, (h, pos): (u32, usize)) -> Option<Item> {
    let m = get_mut(state, h)?;
    (pos < m.items.len()).then(|| m.items.remove(pos))
}

/// Unselect the selected item of `m`.
fn clear_focus(m: &mut Menu) {
    if let Some(it) = m.items.get_mut(m.focus as usize) {
        it.state &= !MF_HILITE;
    }
    m.focus = NONE;
}

fn focus(state: &State, h: u32) -> u32 {
    get(state, h).map_or(NONE, |m| m.focus)
}

/// LOWORD and HIWORD of a wParam.
fn make_wp(lo: u32, hi: u32) -> u32 {
    (lo & 0xFFFF) | (hi & 0xFFFF) << 16
}

// What the functions change an item by, as MENUITEMINFO has it.

#[derive(Clone, Debug, Default)]
struct Info {
    mask: u32,
    kind: u32,
    state: u32,
    id: u32,
    submenu: u32,
    checked: u32,
    unchecked: u32,
    data: u32,
    text: Option<Vec<u8>>,
    bitmap: u32,
}

fn is_string(flags: u32) -> bool {
    flags & (MF_BITMAP | MF_OWNERDRAW | MF_SEPARATOR) == 0
}

/// What InsertMenuA, AppendMenuA and ModifyMenuA make of their flags, id
/// and string: `raw` is the string argument, `text` the string it points
/// to when it is a string item.
fn info_of(flags: u32, id: u32, raw: u32, text: Option<Vec<u8>>) -> Info {
    let mut flags = flags;
    let mut info = Info {
        mask: MIIM_STATE | MIIM_ID | MIIM_FTYPE,
        id,
        ..Info::default()
    };
    if is_string(flags) {
        info.mask |= MIIM_STRING | MIIM_BITMAP;
        match text {
            None => flags |= MF_SEPARATOR,
            Some(mut t) => {
                // A backspace first makes a help item.
                if t.first() == Some(&8) {
                    flags |= MF_HELP;
                    t.remove(0);
                }
                info.text = Some(t);
            }
        }
    } else if flags & MF_BITMAP != 0 {
        info.mask |= MIIM_BITMAP | MIIM_STRING;
        info.bitmap = raw;
    }
    if flags & MF_OWNERDRAW != 0 {
        info.mask |= MIIM_DATA;
        info.data = raw;
    }
    if flags & MF_POPUP != 0 {
        info.mask |= MIIM_SUBMENU;
        info.submenu = id;
    }
    if flags & MF_SEPARATOR != 0 {
        flags |= MF_GRAYED | MF_DISABLED;
    }
    info.state = flags & STATE_MASK & !MFS_DEFAULT;
    info.kind = flags & TYPE_MASK;
    info
}

/// `info_of` for a function's arguments.
fn info_from(cpu: &mut Cpu, flags: u32, id: u32, raw: u32) -> Option<Info> {
    let text = if is_string(flags) && raw != 0 {
        Some(bytes_at(cpu, raw)?)
    } else {
        None
    };
    Some(info_of(flags, id, raw, text))
}

/// The MENUITEMINFOA at `at`, with MIIM_TYPE turned into the bits it
/// stands for; or the error for a wrong size or mix of bits.
fn read_info(cpu: &mut Cpu, at: u32) -> Option<Result<Info, u32>> {
    let size = cpu.bus.guest_read_32(at);
    if cpu.bus.guest_faulted() {
        return None;
    }
    if size != MII_SIZE && size != MII_SIZE_95 {
        return Some(Err(ERROR_INVALID_PARAMETER));
    }
    let w = read_words(cpu, at, size as usize / 4);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let mut info = Info {
        mask: w[1],
        kind: w[2],
        state: w[3],
        id: w[4],
        submenu: w[5],
        checked: w[6],
        unchecked: w[7],
        data: w[8],
        text: None,
        bitmap: w.get(11).copied().unwrap_or(0),
    };
    let mask = info.mask;
    if (mask & MIIM_TYPE != 0 && mask & (MIIM_STRING | MIIM_FTYPE | MIIM_BITMAP) != 0)
        || (mask & MIIM_FTYPE != 0 && info.kind & MFT_BITMAP != 0)
    {
        return Some(Err(ERROR_INVALID_PARAMETER));
    }
    if mask & MIIM_TYPE != 0 {
        info.mask |= MIIM_FTYPE;
        if is_string(info.kind) {
            info.mask |= MIIM_STRING;
        } else if info.kind & MFT_BITMAP != 0 {
            info.mask |= MIIM_BITMAP;
            info.bitmap = w[9];
        }
    }
    if info.mask & MIIM_STRING != 0 && w[9] != 0 {
        info.text = Some(bytes_at(cpu, w[9])?);
    }
    Some(Ok(info))
}

/// Change the item at `at` by `info`. An item left with no text, no
/// bitmap and not owner-drawn becomes a separator.
fn set_info(state: &mut State, at: (u32, usize), info: &Info) -> Result<(), u32> {
    if item(state, at).is_none() {
        return Err(ERROR_MENU_ITEM_NOT_FOUND);
    }
    if info.mask & MIIM_SUBMENU != 0 && info.submenu != 0 {
        let Some(sub) = get_mut(state, info.submenu) else {
            return Err(ERROR_INVALID_PARAMETER);
        };
        sub.flags |= MF_POPUP;
    }
    let it = item_mut(state, at).unwrap();
    if info.mask & MIIM_FTYPE != 0 {
        it.kind = (it.kind & !TYPE_MASK) | (info.kind & TYPE_MASK);
    }
    if info.mask & MIIM_STRING != 0 {
        it.text = info.text.clone();
    }
    if info.mask & MIIM_STATE != 0 {
        it.state = info.state & STATE_MASK;
    }
    if info.mask & MIIM_ID != 0 {
        it.id = info.id;
    }
    if info.mask & MIIM_SUBMENU != 0 {
        it.submenu = info.submenu;
        if info.submenu != 0 {
            it.kind |= MF_POPUP;
        } else {
            it.kind &= !MF_POPUP;
        }
    }
    if info.mask & MIIM_CHECKMARKS != 0 {
        it.checked = info.checked;
        it.unchecked = info.unchecked;
    }
    if info.mask & MIIM_DATA != 0 {
        it.data = info.data;
    }
    if info.mask & MIIM_BITMAP != 0 {
        it.bitmap = info.bitmap;
    }
    if it.text.is_none() && it.kind & MFT_OWNERDRAW == 0 && it.bitmap == 0 {
        it.kind |= MFT_SEPARATOR;
    }
    Ok(())
}

/// Insert an item made by `info` (InsertMenuA's way): whether it was.
fn insert_item(cpu: &mut Cpu, h: u32, pos: u32, flags: u32, info: &Info) -> u32 {
    let state = user(cpu);
    let Some(at) = insert_at(state, h, pos, flags) else {
        set_error(cpu, ERROR_INVALID_MENU_HANDLE);
        return FALSE;
    };
    if let Err(e) = set_info(state, at, info) {
        remove_at(state, at);
        set_error(cpu, e);
        return FALSE;
    }
    if let Some(it) = item_mut(state, at) {
        it.checked = 0;
        it.unchecked = 0;
    }
    TRUE
}

// Making and destroying menus.

pub fn create_menu(cpu: &mut Cpu, _args: &[u32]) -> Option<u32> {
    let pid = current_pid(cpu);
    Some(new_menu(user(cpu), 0, pid))
}

pub fn create_popup_menu(cpu: &mut Cpu, _args: &[u32]) -> Option<u32> {
    let pid = current_pid(cpu);
    Some(new_menu(user(cpu), MF_POPUP, pid))
}

/// DestroyMenu(hmenu): with its submenus.
pub fn destroy_menu(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let state = user(cpu);
    if get(state, args[0]).is_none() {
        set_error(cpu, ERROR_INVALID_MENU_HANDLE);
        return Some(FALSE);
    }
    free(state, args[0], 0);
    Some(TRUE)
}

pub fn is_menu(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if get(user(cpu), args[0]).is_none() {
        set_error(cpu, ERROR_INVALID_MENU_HANDLE);
        return Some(FALSE);
    }
    Some(TRUE)
}

/// LoadMenuA(instance, name).
pub fn load_menu(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (at, _) = match kernel32::resource_data(cpu, args[0], RT_MENU, args[1])? {
        Ok(found) => found,
        Err(_) => {
            set_error(cpu, ERROR_RESOURCE_NAME_NOT_FOUND);
            return Some(0);
        }
    };
    parse_template(cpu, at)
}

/// LoadMenuIndirectA(template): a MENU or MENUEX template, whose strings
/// are UTF-16 in either case.
pub fn load_menu_indirect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    parse_template(cpu, args[0])
}

fn parse_template(cpu: &mut Cpu, at: u32) -> Option<u32> {
    let version = cpu.bus.guest_read_16(at);
    let offset = cpu.bus.guest_read_16(at + 2) as u32;
    if cpu.bus.guest_faulted() {
        return None;
    }
    if version > 1 {
        return Some(0);
    }
    let pid = current_pid(cpu);
    let h = new_menu(user(cpu), 0, pid);
    if h == 0 {
        return Some(0);
    }
    let p = at + 4 + offset;
    let parsed = if version == 0 {
        parse_items(cpu, p, h, 0)
    } else {
        parse_items_ex(cpu, p, h, 0)
    };
    if cpu.bus.guest_faulted() {
        free(user(cpu), h, 0);
        return None;
    }
    if parsed.is_none() {
        free(user(cpu), h, 0);
        return Some(0);
    }
    Some(h)
}

/// The UTF-16 string at `p`, in the ANSI code page, and what follows it.
fn read_wide(cpu: &mut Cpu, p: u32) -> (Vec<u8>, u32) {
    let mut text = Vec::new();
    let mut p = p;
    for _ in 0..0x1000 {
        let c = cpu.bus.guest_read_16(p);
        p += 2;
        if c == 0 || cpu.bus.guest_faulted() {
            break;
        }
        text.push(CodePage::Ansi.byte(c).unwrap_or(b'?'));
    }
    (text, p)
}

/// The items of a MENU template at `p` into `h`: where they end.
fn parse_items(cpu: &mut Cpu, p: u32, h: u32, depth: u32) -> Option<u32> {
    let mut p = p;
    for _ in 0..0x1000 {
        let mut flags = cpu.bus.guest_read_16(p) as u32;
        p += 2;
        let end = flags & MF_END != 0;
        flags &= !MF_END;
        let mut id = 0;
        if flags & MF_POPUP == 0 {
            id = cpu.bus.guest_read_16(p) as u32;
            p += 2;
        }
        let (text, next) = read_wide(cpu, p);
        p = next;
        if cpu.bus.guest_faulted() {
            return None;
        }
        let info = if flags & MF_POPUP != 0 {
            if depth >= MAX_DEPTH {
                return None;
            }
            let pid = current_pid(cpu);
            let sub = new_menu(user(cpu), MF_POPUP, pid);
            if sub == 0 {
                return None;
            }
            match parse_items(cpu, p, sub, depth + 1) {
                Some(next) => p = next,
                None => {
                    free(user(cpu), sub, 0);
                    return None;
                }
            }
            info_of(flags, sub, 1, is_string(flags).then_some(text))
        } else if flags & MF_SEPARATOR != 0 || text.is_empty() {
            info_of(MF_SEPARATOR, id, 0, None)
        } else {
            info_of(flags, id, 1, is_string(flags).then_some(text))
        };
        insert_item(cpu, h, NONE, MF_BYPOSITION, &info);
        if end {
            return Some(p);
        }
    }
    None
}

/// The items of a MENUEX template at `p` into `h`: where they end.
fn parse_items_ex(cpu: &mut Cpu, p: u32, h: u32, depth: u32) -> Option<u32> {
    let mut p = p;
    for _ in 0..0x1000 {
        let w = read_words(cpu, p, 3);
        let resinfo = cpu.bus.guest_read_16(p + 12) as u32;
        // The text is aligned to a word, what follows to a doubleword.
        p = (p + 14 + 1) & !1;
        let (text, next) = read_wide(cpu, p);
        p = (next + 3) & !3;
        if cpu.bus.guest_faulted() {
            return None;
        }
        let mut info = Info {
            mask: MIIM_STATE | MIIM_ID | MIIM_FTYPE,
            kind: w[0],
            state: w[1],
            id: w[2],
            ..Info::default()
        };
        if resinfo & 1 != 0 {
            let help_id = cpu.bus.guest_read_32(p);
            p += 4;
            if depth >= MAX_DEPTH {
                return None;
            }
            let pid = current_pid(cpu);
            let sub = new_menu(user(cpu), MF_POPUP, pid);
            if sub == 0 {
                return None;
            }
            if let Some(m) = get_mut(user(cpu), sub) {
                m.help_id = help_id;
            }
            match parse_items_ex(cpu, p, sub, depth + 1) {
                Some(next) => p = next,
                None => {
                    free(user(cpu), sub, 0);
                    return None;
                }
            }
            info.mask |= MIIM_SUBMENU;
            info.submenu = sub;
        } else if text.is_empty() && info.kind & MF_SEPARATOR == 0 {
            info.kind |= MF_SEPARATOR;
        }
        if is_string(info.kind) {
            info.mask |= MIIM_STRING;
            info.text = Some(text);
        }
        insert_item(cpu, h, NONE, MF_BYPOSITION, &info);
        if resinfo & MF_END != 0 {
            return Some(p);
        }
    }
    None
}

// Changing menus.

/// InsertMenuA(hmenu, position, flags, id, item).
pub fn insert_menu(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (h, pos, flags, id, raw) = (args[0], args[1], args[2], args[3], args[4]);
    let info = info_from(cpu, flags, id, raw)?;
    Some(insert_item(cpu, h, pos, flags, &info))
}

/// AppendMenuA(hmenu, flags, id, item).
pub fn append_menu(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (h, flags, id, raw) = (args[0], args[1], args[2], args[3]);
    insert_menu(cpu, &[h, NONE, flags | MF_BYPOSITION, id, raw])
}

/// ModifyMenuA(hmenu, position, flags, id, item).
pub fn modify_menu(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (h, pos, flags, id, raw) = (args[0], args[1], args[2], args[3], args[4]);
    let info = info_from(cpu, flags, id, raw)?;
    let Some(at) = lookup(cpu, h, pos, flags) else {
        // Word 95 changes the task list item, which isn't there.
        return Some((pos == SC_TASKLIST && flags & MF_BYPOSITION == 0) as u32);
    };
    match set_info(user(cpu), at, &info) {
        Ok(()) => Some(TRUE),
        Err(e) => {
            set_error(cpu, e);
            Some(FALSE)
        }
    }
}

/// RemoveMenu(hmenu, position, flags): the item goes, its submenu stays.
pub fn remove_menu(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(at) = lookup(cpu, args[0], args[1], args[2]) else {
        return Some(FALSE);
    };
    remove_at(user(cpu), at);
    Some(TRUE)
}

/// DeleteMenu(hmenu, position, flags): the item goes with its submenu.
pub fn delete_menu(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(at) = lookup(cpu, args[0], args[1], args[2]) else {
        return Some(FALSE);
    };
    let state = user(cpu);
    if let Some(it) = remove_at(state, at)
        && it.kind & MF_POPUP != 0
    {
        free(state, it.submenu, 0);
    }
    Some(TRUE)
}

/// InsertMenuItemA(hmenu, item, by_position, info).
pub fn insert_menu_item(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (h, pos, by_position) = (args[0], args[1], args[2]);
    let info = match read_info(cpu, args[3])? {
        Ok(info) => info,
        Err(e) => {
            set_error(cpu, e);
            return Some(FALSE);
        }
    };
    let flags = if by_position != 0 { MF_BYPOSITION } else { 0 };
    let state = user(cpu);
    let Some(at) = insert_at(state, h, pos, flags) else {
        set_error(cpu, ERROR_INVALID_MENU_HANDLE);
        return Some(FALSE);
    };
    match set_info(state, at, &info) {
        Ok(()) => Some(TRUE),
        Err(e) => {
            set_error(cpu, e);
            Some(FALSE)
        }
    }
}

/// SetMenuItemInfoA(hmenu, item, by_position, info).
pub fn set_menu_item_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (h, pos, by_position) = (args[0], args[1], args[2]);
    let info = match read_info(cpu, args[3])? {
        Ok(info) => info,
        Err(e) => {
            set_error(cpu, e);
            return Some(FALSE);
        }
    };
    let flags = if by_position != 0 { MF_BYPOSITION } else { 0 };
    let Some(at) = lookup(cpu, h, pos, flags) else {
        set_error(cpu, ERROR_MENU_ITEM_NOT_FOUND);
        return Some(FALSE);
    };
    match set_info(user(cpu), at, &info) {
        Ok(()) => Some(TRUE),
        Err(e) => {
            set_error(cpu, e);
            Some(FALSE)
        }
    }
}

/// GetMenuItemInfoA(hmenu, item, by_position, info): what fMask asks
/// for, as Windows 95 gives it (hSubMenu is left as it is without
/// MIIM_SUBMENU).
pub fn get_menu_item_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (h, pos, by_position, at) = (args[0], args[1], args[2], args[3]);
    let size = cpu.bus.guest_read_32(at);
    if cpu.bus.guest_faulted() {
        return None;
    }
    if size != MII_SIZE && size != MII_SIZE_95 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    let flags = if by_position != 0 { MF_BYPOSITION } else { 0 };
    let Some(found) = lookup(cpu, h, pos, flags) else {
        set_error(cpu, ERROR_MENU_ITEM_NOT_FOUND);
        return Some(FALSE);
    };
    let it = item(user(cpu), found).cloned().unwrap_or_default();
    let mut w = read_words(cpu, at, size as usize / 4);
    let mask = w[1];
    if mask & MIIM_TYPE != 0 {
        w[2] = it.kind & TYPE_MASK;
        if it.bitmap != 0 {
            w[2] |= MFT_BITMAP;
        }
        if w[2] & MFT_BITMAP != 0 {
            w[9] = it.bitmap;
            w[10] = 0;
        }
    }
    if mask & (MIIM_TYPE | MIIM_STRING) != 0 {
        let (buffer, cch) = (w[9], w[10]);
        match &it.text {
            None => {
                if buffer != 0 && cch != 0 {
                    w[10] = 0;
                    cpu.bus.guest_write_bytes(buffer, &[0]);
                }
            }
            Some(text) => {
                let len = text.len() as u32;
                if buffer != 0 && cch != 0 {
                    super::copy_out(cpu, buffer, cch, text);
                    w[10] = if cch <= len + 1 { cch - 1 } else { len };
                } else {
                    w[10] = len;
                }
            }
        }
    }
    if mask & MIIM_FTYPE != 0 {
        w[2] = it.kind & TYPE_MASK;
    }
    if mask & MIIM_BITMAP != 0 && w.len() > 11 {
        w[11] = it.bitmap;
    }
    if mask & MIIM_STATE != 0 {
        w[3] = it.state & STATE_MASK;
    }
    if mask & MIIM_ID != 0 {
        w[4] = it.id;
    }
    if mask & MIIM_SUBMENU != 0 {
        w[5] = it.submenu;
    }
    if mask & MIIM_CHECKMARKS != 0 {
        w[6] = it.checked;
        w[7] = it.unchecked;
    }
    if mask & MIIM_DATA != 0 {
        w[8] = it.data;
    }
    write_words(cpu, at, &w);
    Some(TRUE)
}

/// EnableMenuItem(hmenu, item, flags): the item's MF_GRAYED and
/// MF_DISABLED before, or -1 when there is no such item.
pub fn enable_menu_item(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (h, pos, flags) = (args[0], args[1], args[2]);
    let Some(at) = lookup(cpu, h, pos, flags) else {
        return Some(NONE);
    };
    let it = item_mut(user(cpu), at).unwrap();
    let old = it.state & (MF_GRAYED | MF_DISABLED);
    it.state ^= (old ^ flags) & (MF_GRAYED | MF_DISABLED);
    Some(old)
}

/// CheckMenuItem(hmenu, item, flags): MF_CHECKED as it was, or -1.
pub fn check_menu_item(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (h, pos, flags) = (args[0], args[1], args[2]);
    let Some(at) = lookup(cpu, h, pos, flags) else {
        return Some(NONE);
    };
    let it = item_mut(user(cpu), at).unwrap();
    let old = it.state & MF_CHECKED;
    if flags & MF_CHECKED != 0 {
        it.state |= MF_CHECKED;
    } else {
        it.state &= !MF_CHECKED;
    }
    Some(old)
}

/// CheckMenuRadioItem(hmenu, first, last, check, flags): `check` gets a
/// radio check and the others of the range in the same menu none.
/// Whether `check` was found.
pub fn check_menu_radio_item(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (h, first, last, check, flags) = (args[0], args[1], args[2], args[3], args[4]);
    let state = user(cpu);
    if get(state, h).is_none() {
        set_error(cpu, ERROR_INVALID_MENU_HANDLE);
        return Some(FALSE);
    }
    let by_position = flags & MF_BYPOSITION != 0;
    let mut first_menu = None;
    let mut done = false;
    for i in (first as u64..=last as u64).take(0x1_0000) {
        let i = i as u32;
        let Some(at) = find(state, h, i, by_position) else {
            continue;
        };
        match first_menu {
            None => first_menu = Some(at.0),
            Some(m) if m != at.0 => continue,
            _ => {}
        }
        let it = item_mut(state, at).unwrap();
        if it.kind == MFT_SEPARATOR {
            continue;
        }
        if i == check {
            it.kind |= MFT_RADIOCHECK;
            it.state |= MFS_CHECKED;
            done = true;
        } else {
            it.state &= !MFS_CHECKED;
        }
    }
    Some(done as u32)
}

/// SetMenuDefaultItem(hmenu, item, by_position): -1 for none.
pub fn set_menu_default_item(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (h, pos, by_position) = (args[0], args[1], args[2]);
    let Some(m) = get_mut(user(cpu), h) else {
        set_error(cpu, ERROR_INVALID_MENU_HANDLE);
        return Some(FALSE);
    };
    for it in m.items.iter_mut() {
        it.state &= !MFS_DEFAULT;
    }
    if pos == NONE {
        return Some(TRUE);
    }
    let found = if by_position != 0 {
        m.items.get_mut(pos as usize)
    } else {
        m.items.iter_mut().find(|it| it.id == pos)
    };
    match found {
        Some(it) => {
            it.state |= MFS_DEFAULT;
            Some(TRUE)
        }
        None => Some(FALSE),
    }
}

/// GetMenuDefaultItem(hmenu, by_position, flags): the default item's
/// place or id, or -1.
pub fn get_menu_default_item(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (h, by_position, flags) = (args[0], args[1], args[2]);
    if get(user(cpu), h).is_none() {
        set_error(cpu, ERROR_INVALID_MENU_HANDLE);
        return Some(NONE);
    }
    Some(default_item(user(cpu), h, by_position != 0, flags, 0))
}

fn default_item(state: &State, h: u32, by_position: bool, flags: u32, depth: u32) -> u32 {
    let Some(m) = get(state, h) else {
        return NONE;
    };
    let Some(i) = m.items.iter().position(|it| it.state & MFS_DEFAULT != 0) else {
        return NONE;
    };
    let it = &m.items[i];
    if flags & GMDI_USEDISABLED == 0 && it.state & (MF_GRAYED | MF_DISABLED) != 0 {
        return NONE;
    }
    if it.kind & MF_POPUP != 0 && flags & GMDI_GOINTOPOPUPS != 0 && depth < MAX_DEPTH {
        let found = default_item(state, it.submenu, by_position, flags, depth + 1);
        if found != NONE {
            return found;
        }
    }
    if by_position { i as u32 } else { it.id }
}

// Reading menus.

pub fn get_menu_item_count(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match get(user(cpu), args[0]) {
        Some(m) => Some(m.items.len() as u32),
        None => {
            set_error(cpu, ERROR_INVALID_MENU_HANDLE);
            Some(NONE)
        }
    }
}

/// GetMenuItemID(hmenu, position): -1 for a popup item.
pub fn get_menu_item_id(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(at) = lookup(cpu, args[0], args[1], MF_BYPOSITION) else {
        return Some(NONE);
    };
    let it = item(user(cpu), at).unwrap();
    Some(if it.kind & MF_POPUP != 0 { NONE } else { it.id })
}

/// GetMenuState(hmenu, item, flags): the type and state; for a popup
/// item, its submenu's count of items in the high byte.
pub fn get_menu_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(at) = lookup(cpu, args[0], args[1], args[2]) else {
        return Some(NONE);
    };
    let state = user(cpu);
    let it = item(state, at).unwrap();
    if it.kind & MF_POPUP != 0 {
        return Some(match get(state, it.submenu) {
            Some(sub) => (sub.items.len() as u32) << 8 | ((it.state | it.kind) & 0xFF),
            None => NONE,
        });
    }
    Some(it.kind | it.state)
}

/// GetMenuStringA(hmenu, item, buffer, size, flags): the length of what
/// it copied, or of the text without a buffer.
pub fn get_menu_string(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (h, pos, buffer, size, flags) = (args[0], args[1], args[2], args[3], args[4]);
    if buffer != 0 && size != 0 {
        cpu.bus.guest_write_bytes(buffer, &[0]);
    }
    let Some(at) = lookup(cpu, h, pos, flags) else {
        set_error(cpu, ERROR_MENU_ITEM_NOT_FOUND);
        return Some(0);
    };
    let Some(text) = item(user(cpu), at).and_then(|it| it.text.clone()) else {
        return Some(0);
    };
    if buffer == 0 || size == 0 {
        return Some(text.len() as u32);
    }
    Some(super::copy_out(cpu, buffer, size, &text))
}

/// GetSubMenu(hmenu, position).
pub fn get_sub_menu(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(at) = lookup(cpu, args[0], args[1], MF_BYPOSITION) else {
        return Some(0);
    };
    let it = item(user(cpu), at).unwrap();
    Some(if it.kind & MF_POPUP != 0 {
        it.submenu
    } else {
        0
    })
}

pub fn get_menu_check_mark_dimensions(_cpu: &mut Cpu, _args: &[u32]) -> Option<u32> {
    Some(make_long(metric(71), metric(72)))
}

/// GetMenuItemRect(hwnd, hmenu, position, rect): on the screen.
pub fn get_menu_item_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, h, pos, at) = (args[0], args[1], args[2], args[3]);
    if at == 0 {
        return Some(FALSE);
    }
    let Some(found) = find(user(cpu), h, pos, true) else {
        return Some(FALSE);
    };
    let (flags, owner) = get(user(cpu), found.0).map_or((0, 0), |m| (m.flags, m.hwnd));
    if flags & MF_POPUP == 0 && owner != 0 {
        layout_bar_now(cpu, owner);
    }
    let window = if hwnd != 0 { hwnd } else { owner };
    let state = user(cpu);
    if window == 0 || !state.is_window(window) {
        return Some(FALSE);
    }
    let r = item(state, found).map_or(Rect::default(), |it| it.rect);
    let (x, y) = state.window_origin(window);
    r.offset(x, y).write(cpu, at);
    Some(TRUE)
}

/// MenuItemFromPoint(hwnd, hmenu, point): the item at the point on the
/// screen, or -1.
pub fn menu_item_from_point(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, h, x, y) = (args[0], args[1], args[2] as i32, args[3] as i32);
    let state = user(cpu);
    let Some(m) = get(state, h) else {
        set_error(cpu, ERROR_INVALID_MENU_HANDLE);
        return Some(NONE);
    };
    let window = if m.hwnd != 0 { m.hwnd } else { hwnd };
    if !state.is_window(window) {
        return Some(NONE);
    }
    let (wx, wy) = state.window_origin(window);
    Some(item_at(state, h, x - wx, y - wy).unwrap_or(NONE))
}

/// The item of `h` at (`x`, `y`) where it was laid out.
fn item_at(state: &State, h: u32, x: i32, y: i32) -> Option<u32> {
    get(state, h)?
        .items
        .iter()
        .position(|it| it.rect.contains(x, y))
        .map(|i| i as u32)
}

// Windows' menus.

/// The menu bar of `hwnd`, if it has one.
fn bar_menu(state: &State, hwnd: u32) -> u32 {
    state
        .window(hwnd)
        .filter(|w| w.style & WS_CHILD == 0 && get(state, w.id).is_some())
        .map_or(0, |w| w.id)
}

/// The frame and caption around a window's menu bar: the rectangle
/// AdjustWindowRectEx puts around an empty one, without the client
/// edges.
fn frame(w: &Window) -> Rect {
    let ex = w.ex_style & !(WS_EX_CLIENTEDGE | WS_EX_STATICEDGE);
    super::window::adjust(Rect::default(), w.style, ex, false)
}

/// Where the menu bar of `hwnd` is, in its window's coordinates.
fn bar_rect(state: &State, hwnd: u32) -> Option<Rect> {
    let w = state.window(hwnd)?;
    if w.style & (WS_CHILD | WS_MINIMIZE) != 0 || w.id == 0 {
        return None;
    }
    let o = frame(w);
    Some(Rect::new(
        -o.left,
        -o.top,
        w.rect.width() - o.right,
        -o.top + metric(15),
    ))
}

/// The system menu button of `hwnd` on the screen.
fn sys_button(state: &State, hwnd: u32) -> Option<Rect> {
    let w = state.window(hwnd)?;
    let o = frame(w);
    let cap = if w.ex_style & WS_EX_TOOLWINDOW != 0 {
        metric(51)
    } else {
        metric(4)
    };
    let (x, y) = state.window_origin(hwnd);
    Some(Rect::new(
        x - o.left,
        y - o.top - cap,
        x - o.left + cap,
        y - o.top,
    ))
}

/// GetMenu(hwnd).
pub fn get_menu(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match user(cpu).window(args[0]) {
        Some(w) => Some(w.id),
        None => {
            set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
            Some(0)
        }
    }
}

/// SetMenu(hwnd, hmenu): the window's frame is worked out again for it.
pub fn set_menu(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, h) = (args[0], args[1]);
    let state = user(cpu);
    if h != 0 && get(state, h).is_none() {
        set_error(cpu, ERROR_INVALID_MENU_HANDLE);
        return Some(FALSE);
    }
    let Some(w) = state.window(hwnd) else {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(FALSE);
    };
    if w.style & (WS_CHILD | WS_POPUP) == WS_CHILD {
        set_error(cpu, ERROR_CHILD_WINDOW_MENU);
        return Some(FALSE);
    }
    if let Some(m) = get_mut(state, h) {
        m.hwnd = hwnd;
    }
    if let Some(w) = state.window_mut(hwnd) {
        w.id = h;
    }
    task::start(cpu, TASK_MENU, vec![K_SET_POS, hwnd], 0)
}

/// DrawMenuBar(hwnd).
pub fn draw_menu_bar(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hwnd = args[0];
    let state = user(cpu);
    let Some(w) = state.window(hwnd) else {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(FALSE);
    };
    if w.style & (WS_CHILD | WS_POPUP) == WS_CHILD {
        return Some(TRUE);
    }
    let h = bar_menu(state, hwnd);
    let Some(m) = get_mut(state, h) else {
        return Some(TRUE);
    };
    m.hwnd = hwnd;
    task::start(cpu, TASK_MENU, vec![K_SET_POS, hwnd], 0)
}

/// GetSystemMenu(hwnd, revert): the popup of the window's system menu,
/// made when first asked for; `revert` puts back the standard one and
/// returns 0.
pub fn get_system_menu(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, revert) = (args[0], args[1] != 0);
    if hwnd == DESKTOP {
        return Some(0);
    }
    let state = user(cpu);
    let Some(w) = state.window(hwnd) else {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(0);
    };
    if revert && w.sysmenu != 0 {
        let old = w.sysmenu;
        free(state, old, 0);
        if let Some(w) = state.window_mut(hwnd) {
            w.sysmenu = 0;
        }
    }
    let bar = system_bar(cpu, hwnd);
    let popup = get(user(cpu), bar)
        .and_then(|m| m.items.first())
        .map_or(0, |it| it.submenu);
    Some(if revert { 0 } else { popup })
}

/// The system menu of `hwnd`, a menu bar of one popup item, made when
/// first asked for: 0 for a window without WS_SYSMENU.
fn system_bar(cpu: &mut Cpu, hwnd: u32) -> u32 {
    let state = user(cpu);
    let Some(w) = state.window(hwnd) else {
        return 0;
    };
    if w.sysmenu != 0 && get(state, w.sysmenu).is_some() {
        return w.sysmenu;
    }
    if w.style & WS_SYSMENU == 0 {
        return 0;
    }
    let pid = w.pid;
    let no_close = super::class::style_of(state, hwnd) & super::class::CS_NOCLOSE != 0;
    let popup = new_menu(state, MF_POPUP | MF_SYSMENU, pid);
    if popup == 0 {
        return 0;
    }
    let mut items = Vec::new();
    for (id, label) in [
        (SC_RESTORE, &b"&Restore"[..]),
        (SC_MOVE, b"&Move"),
        (SC_SIZE, b"&Size"),
        (SC_MINIMIZE, b"Mi&nimize"),
        (SC_MAXIMIZE, b"Ma&ximize"),
    ] {
        items.push(Item {
            id,
            text: Some(label.to_vec()),
            ..Item::default()
        });
    }
    items.push(Item {
        kind: MF_SEPARATOR,
        state: MF_GRAYED | MF_DISABLED,
        ..Item::default()
    });
    if !no_close {
        items.push(Item {
            id: SC_CLOSE,
            state: MFS_DEFAULT,
            text: Some(b"&Close\tAlt+F4".to_vec()),
            ..Item::default()
        });
    }
    get_mut(state, popup).unwrap().items = items;
    let bar = new_menu(state, MF_SYSMENU, pid);
    if bar == 0 {
        free(state, popup, 0);
        return 0;
    }
    let m = get_mut(state, bar).unwrap();
    m.hwnd = hwnd;
    m.items.push(Item {
        kind: MF_SYSMENU | MF_POPUP,
        submenu: popup,
        ..Item::default()
    });
    if let Some(w) = state.window_mut(hwnd) {
        w.sysmenu = bar;
    }
    bar
}

/// Gray the items of a system menu the window's style has no use for,
/// before it shows.
fn init_sys_popup(cpu: &mut Cpu, h: u32, hwnd: u32) {
    let state = user(cpu);
    let Some(w) = state.window(hwnd) else {
        return;
    };
    let style = w.style;
    let no_close = super::class::style_of(state, hwnd) & super::class::CS_NOCLOSE != 0;
    let sized = style & (WS_MAXIMIZE | WS_MINIMIZE) != 0;
    let grays = [
        (SC_SIZE, style & WS_THICKFRAME == 0 || sized),
        (SC_MOVE, style & WS_MAXIMIZE != 0),
        (
            SC_MINIMIZE,
            style & WS_MINIMIZEBOX == 0 || style & WS_MINIMIZE != 0,
        ),
        (
            SC_MAXIMIZE,
            style & WS_MAXIMIZEBOX == 0 || style & WS_MAXIMIZE != 0,
        ),
        (SC_RESTORE, !sized),
    ];
    for (cmd, gray) in grays {
        if let Some(at) = find(state, h, cmd, false) {
            let it = item_mut(state, at).unwrap();
            it.state &= !(MF_GRAYED | MF_DISABLED);
            if gray {
                it.state |= MF_GRAYED;
            }
        }
    }
    if no_close && let Some(at) = find(state, h, SC_CLOSE, false) {
        let it = item_mut(state, at).unwrap();
        it.state = (it.state & !MF_DISABLED) | MF_GRAYED;
    }
}

// Laying out and drawing.

/// Run `f` with the menu font on `hdc`, drawing text without a
/// background. The menu font is the one SPI_GETNONCLIENTMETRICS gives:
/// 8-point MS Sans Serif, regular.
fn with_font<T>(cpu: &mut Cpu, hdc: u32, f: impl FnOnce(&mut Cpu) -> T) -> T {
    let log = super::sysparams::sans_serif(false);
    let font = gdi32::add(
        cpu,
        gdi32::Kind::Font {
            font: gdi32::Font { log },
        },
    );
    let saved = gdi32::dc::with(cpu, hdc, |d| {
        let old = (d.attrs.font, d.attrs.text_color, d.attrs.bk_mode);
        d.attrs.font = font;
        d.attrs.bk_mode = TRANSPARENT;
        old
    });
    let result = f(cpu);
    if let Some(old) = saved {
        gdi32::dc::with(cpu, hdc, |d| {
            d.attrs.font = old.0;
            d.attrs.text_color = old.1;
            d.attrs.bk_mode = old.2;
        });
    }
    gdi32::delete_object(cpu, &[font]);
    result
}

/// The width and height of a line of `text`, without its prefixes.
fn text_size(cpu: &mut Cpu, hdc: u32, text: &[u8]) -> (i32, i32) {
    let mut r = Rect::default();
    let flags = text::DT_SINGLELINE | text::DT_CALCRECT;
    let h = text::draw_text_bytes(cpu, hdc, text, &mut r, flags);
    (r.width(), h)
}

/// An item's text before its tab, and after it.
fn split_tab(text: &[u8]) -> (&[u8], Option<&[u8]>) {
    match text.iter().position(|&c| c == b'\t') {
        Some(i) => (&text[..i], Some(&text[i + 1..])),
        None => (text, None),
    }
}

/// Whether a bitmap handle is one of HBMMENU_'s, which aren't drawn.
fn is_special_bitmap(b: u32) -> bool {
    b <= 11 || b == NONE
}

fn bitmap_size(cpu: &mut Cpu, b: u32) -> (i32, i32) {
    if is_special_bitmap(b) {
        let size = metric(15) - 4;
        return (size, size);
    }
    gdi32::gdi(cpu)
        .bitmap(b)
        .map_or((0, 0), |b| (b.width, b.height))
}

/// Where a popup item's text starts, from its left. Windows 98 lays out
/// "Save As...\tCtrl+S" in 8-point MS Sans Serif as 22 pixels, the text
/// (49), 8, "Ctrl+S" (28) and 18: an item 125 wide.
fn text_x() -> i32 {
    metric(71) + 9
}

/// The space between a popup item's text and the text after its tab.
const TAB_SPACE: i32 = 8;

/// The space a popup item has after its text, for a submenu's arrow.
fn arrow_space() -> i32 {
    metric(71) + 5
}

/// The space a menu bar item has around its text, as Windows 98 lays
/// it out: "File" in 8-point MS Sans Serif is 16 wide, its item 28.
const BAR_ITEM_SPACE: i32 = 12;

/// The size an item needs: the width of its text before a tab and after
/// it, and its height. A menu bar item is one pixel lower than the bar
/// (SM_CYMENU), and a popup item with text is 4 pixels taller than it:
/// 18 and 17 on Windows 98.
fn measure(cpu: &mut Cpu, hdc: u32, it: &Item, bar: bool) -> (i32, i32, i32) {
    let fm = font::metrics(cpu, hdc).unwrap_or_default();
    let (th, avg) = (fm.height, fm.average_width.max(1));
    let line = (th + 4).max(metric(55));
    let bar_h = th.max(metric(15) - 1);
    if it.kind & MF_SEPARATOR != 0 {
        return (0, 0, if bar { 0 } else { metric(55) / 2 });
    }
    if it.kind & MF_OWNERDRAW != 0 {
        return if bar {
            (2 * avg, 0, bar_h)
        } else {
            (0, 0, line)
        };
    }
    if let Some(text) = &it.text {
        if bar {
            let (w, _) = text_size(cpu, hdc, text);
            return (w + BAR_ITEM_SPACE, 0, bar_h);
        }
        let (left, right) = split_tab(text);
        let (lw, lh) = text_size(cpu, hdc, left);
        let rw = right.map_or(0, |r| text_size(cpu, hdc, r).0);
        return (lw, rw, lh.max(th) + 4);
    }
    if it.bitmap != 0 {
        let (bw, bh) = bitmap_size(cpu, it.bitmap);
        return if bar {
            (bw + 4, 0, bh.max(bar_h))
        } else {
            (bw, 0, (bh + 2).max(line))
        };
    }
    (0, 0, line)
}

/// Lay out the menu bar `h` of `hwnd` in `rect` (window coordinates) of
/// `hdc`, on one line. The items from the first with MF_RIGHTJUSTIFY on
/// go to the right end.
fn layout_bar(cpu: &mut Cpu, h: u32, hwnd: u32, hdc: u32, rect: Rect) {
    let Some(items) = get(user(cpu), h).map(|m| m.items.clone()) else {
        return;
    };
    let mut rects = Vec::with_capacity(items.len());
    let mut x = rect.left;
    let mut right_from = None;
    for (i, it) in items.iter().enumerate() {
        if right_from.is_none() && it.kind & MF_RIGHTJUSTIFY != 0 {
            right_from = Some(i);
        }
        let (w, _, h) = measure(cpu, hdc, it, true);
        rects.push(Rect::new(x, rect.top, x + w, rect.top + h));
        x += w;
    }
    if let Some(i) = right_from {
        let shift = rect.right - x;
        if shift > 0 {
            for r in &mut rects[i..] {
                *r = r.offset(shift, 0);
            }
        }
    }
    if let Some(m) = get_mut(user(cpu), h) {
        for (it, r) in m.items.iter_mut().zip(rects) {
            it.rect = r;
            it.tab = 0;
        }
        m.width = rect.width();
        m.height = rect.height();
        m.hwnd = hwnd;
    }
}

/// Lay out the menu bar of `hwnd` where it is, measuring on a DC of its
/// own.
fn layout_bar_now(cpu: &mut Cpu, hwnd: u32) {
    let state = user(cpu);
    let h = bar_menu(state, hwnd);
    let Some(rect) = bar_rect(state, hwnd).filter(|_| h != 0) else {
        return;
    };
    let Some(mem) = gdi32::dc::create_compatible_dc(cpu, &[0]).filter(|&d| d != 0) else {
        return;
    };
    with_font(cpu, mem, |cpu| layout_bar(cpu, h, hwnd, mem, rect));
    gdi32::dc::delete_dc(cpu, &[mem]);
}

/// Lay out the popup menu `h` in columns, broken at MF_MENUBREAK and
/// MF_MENUBARBREAK: the size of its window.
fn layout_popup(cpu: &mut Cpu, h: u32) -> (i32, i32) {
    let Some(items) = get(user(cpu), h).map(|m| m.items.clone()) else {
        return (0, 0);
    };
    let Some(mem) = gdi32::dc::create_compatible_dc(cpu, &[0]).filter(|&d| d != 0) else {
        return (0, 0);
    };
    let n = items.len();
    let (rects, tabs, size) = with_font(cpu, mem, |cpu| {
        let sizes: Vec<_> = items
            .iter()
            .map(|it| measure(cpu, mem, it, false))
            .collect();
        let mut rects = vec![Rect::default(); n];
        let mut tabs = vec![0; n];
        let mut x = POPUP_BORDER;
        let mut bottom = POPUP_BORDER;
        let mut i = 0;
        while i < n {
            let start = i;
            i += 1;
            while i < n && items[i].kind & (MF_MENUBREAK | MF_MENUBARBREAK) == 0 {
                i += 1;
            }
            let left = sizes[start..i].iter().map(|s| s.0).max().unwrap_or(0);
            let right = sizes[start..i].iter().map(|s| s.1).max().unwrap_or(0);
            let gap = if right > 0 { TAB_SPACE + right } else { 0 };
            let width = text_x() + left + gap + arrow_space();
            let mut y = POPUP_BORDER;
            for j in start..i {
                rects[j] = Rect::new(x, y, x + width, y + sizes[j].2);
                tabs[j] = x + text_x() + left + TAB_SPACE;
                y += sizes[j].2;
            }
            bottom = bottom.max(y);
            x += width;
        }
        (rects, tabs, (x + POPUP_BORDER, bottom + POPUP_BORDER))
    });
    gdi32::dc::delete_dc(cpu, &[mem]);
    if let Some(m) = get_mut(user(cpu), h) {
        for ((it, r), tab) in m.items.iter_mut().zip(rects).zip(tabs) {
            it.rect = r;
            it.tab = tab;
        }
        m.width = size.0;
        m.height = size.1;
    }
    size
}

fn set_text_color(cpu: &mut Cpu, hdc: u32, color: u32) {
    let rgb = user(cpu).color(color);
    gdi32::dc::with(cpu, hdc, |d| d.attrs.text_color = rgb);
}

/// Draw `text` in `r` by DrawTextA's `flags`: in `color`, or embossed
/// when `gray`.
fn label(cpu: &mut Cpu, hdc: u32, text: &[u8], r: Rect, flags: u32, color: u32, gray: bool) {
    if gray {
        set_text_color(cpu, hdc, COLOR_BTNHIGHLIGHT);
        let mut shadow = r.offset(1, 1);
        text::draw_text_bytes(cpu, hdc, text, &mut shadow, flags);
    }
    set_text_color(cpu, hdc, color);
    let mut r = r;
    text::draw_text_bytes(cpu, hdc, text, &mut r, flags);
}

/// Draw an item where it was laid out: on a menu bar (`bar`) or in a
/// popup menu.
fn draw_item(cpu: &mut Cpu, hdc: u32, it: &Item, bar: bool) {
    let r = it.rect;
    if r.is_empty() {
        return;
    }
    if it.kind & MF_SEPARATOR != 0 {
        if !bar {
            let y = r.top + r.height() / 2 - 1;
            let line = Rect::new(r.left + 1, y, r.right - 1, y + 1);
            paint::fill_color(cpu, hdc, line, COLOR_BTNSHADOW);
            paint::fill_color(cpu, hdc, line.offset(0, 1), COLOR_BTNHIGHLIGHT);
        }
        return;
    }
    let hilite = it.state & MF_HILITE != 0;
    let grayed = it.state & MF_GRAYED != 0;
    if bar {
        paint::fill_color(cpu, hdc, r, COLOR_MENU);
        if hilite {
            paint::edge(cpu, hdc, r, BDR_SUNKENOUTER, paint::BF_RECT);
        }
    } else {
        let back = if hilite { COLOR_HIGHLIGHT } else { COLOR_MENU };
        paint::fill_color(cpu, hdc, r, back);
    }
    let fore = if hilite && !bar {
        COLOR_HIGHLIGHTTEXT
    } else {
        COLOR_MENUTEXT
    };
    let color = if grayed { COLOR_GRAYTEXT } else { fore };
    if !bar {
        let check = metric(71);
        let cr = Rect::new(r.left + 2, r.top, r.left + 4 + check, r.bottom);
        let checked = it.state & MF_CHECKED != 0;
        let bitmap = if checked { it.checked } else { it.unchecked };
        if bitmap != 0 && !is_special_bitmap(bitmap) {
            let (bw, bh) = bitmap_size(cpu, bitmap);
            let at = (
                cr.left + (cr.width() - bw) / 2,
                r.top + (r.height() - bh) / 2,
            );
            text::draw_bitmap(cpu, hdc, bitmap, at, (0, 0));
        } else if checked && it.kind & MFT_RADIOCHECK != 0 {
            let center = (cr.left + cr.width() / 2, r.top + r.height() / 2);
            paint::disc(cpu, hdc, center, 3, color);
        } else if checked {
            paint::check_mark(cpu, hdc, cr, color);
        }
        if it.kind & MF_POPUP != 0 {
            let ar = Rect::new(r.right - check - 2, r.top, r.right - 2, r.bottom);
            paint::arrow(cpu, hdc, ar, 3, color);
        }
    }
    let flags = text::DT_SINGLELINE | text::DT_VCENTER;
    match &it.text {
        Some(text) if bar => {
            label(
                cpu,
                hdc,
                text,
                r,
                flags | text::DT_CENTER,
                color,
                grayed && !hilite,
            );
        }
        Some(text) => {
            let (left, right) = split_tab(text);
            let gray = grayed && !hilite;
            let tr = Rect::new(r.left + text_x(), r.top, r.right, r.bottom);
            label(cpu, hdc, left, tr, flags, color, gray);
            if let Some(right) = right {
                let tr = Rect::new(it.tab, r.top, r.right, r.bottom);
                label(cpu, hdc, right, tr, flags, color, gray);
            }
        }
        None if it.bitmap != 0 && !is_special_bitmap(it.bitmap) => {
            let (bw, bh) = bitmap_size(cpu, it.bitmap);
            let x = if bar {
                r.left + (r.width() - bw) / 2
            } else {
                r.left + text_x()
            };
            let y = r.top + (r.height() - bh) / 2;
            text::draw_bitmap(cpu, hdc, it.bitmap, (x, y), (0, 0));
        }
        None => {}
    }
}

/// Draw the menu bar of `hwnd` in `rect` of `hdc`, a DC of the whole
/// window (window coordinates): one line, metric(15) high.
pub fn draw_bar(cpu: &mut Cpu, hwnd: u32, hdc: u32, rect: Rect) {
    let h = bar_menu(user(cpu), hwnd);
    with_font(cpu, hdc, |cpu| {
        paint::fill_color(cpu, hdc, rect, COLOR_MENU);
        if h == 0 {
            return;
        }
        layout_bar(cpu, h, hwnd, hdc, rect);
        let items = get(user(cpu), h)
            .map(|m| m.items.clone())
            .unwrap_or_default();
        for it in &items {
            let mut it = it.clone();
            it.rect = it.rect.intersect(&rect);
            draw_item(cpu, hdc, &it, true);
        }
    });
}

/// Draw the menu bar of `hwnd` now, on a DC of its own.
fn redraw_bar(cpu: &mut Cpu, hwnd: u32) {
    let Some(rect) = bar_rect(user(cpu), hwnd) else {
        return;
    };
    let hdc = paint::window_dc(cpu, hwnd, DCX_USESTYLE | DCX_WINDOW, None);
    if hdc == 0 {
        return;
    }
    draw_bar(cpu, hwnd, hdc, rect);
    paint::release_dc(cpu, &[hwnd, hdc]);
}

/// Draw the popup menu `h` in its window `hwnd`.
fn draw_popup(cpu: &mut Cpu, h: u32, hwnd: u32) {
    let Some(r) = user(cpu)
        .window(hwnd)
        .map(|w| Rect::new(0, 0, w.rect.width(), w.rect.height()))
    else {
        return;
    };
    let hdc = paint::window_dc(cpu, hwnd, DCX_USESTYLE | DCX_WINDOW, None);
    if hdc == 0 {
        return;
    }
    let items = get(user(cpu), h)
        .map(|m| m.items.clone())
        .unwrap_or_default();
    with_font(cpu, hdc, |cpu| {
        paint::fill_color(cpu, hdc, r, COLOR_MENU);
        paint::edge(cpu, hdc, r, paint::EDGE_RAISED, paint::BF_RECT);
        for it in &items {
            draw_item(cpu, hdc, it, false);
        }
    });
    paint::release_dc(cpu, &[hwnd, hdc]);
}

/// The popup menu showing in the #32768 window `hwnd`.
fn popup_menu_of(state: &State, hwnd: u32) -> u32 {
    state
        .menus
        .iter()
        .position(|m| {
            m.as_ref()
                .is_some_and(|m| m.flags & MF_POPUP != 0 && m.hwnd == hwnd)
        })
        .map_or(0, |i| MENU_BASE + 4 * i as u32)
}

/// The window procedure of popup menus (class #32768).
pub fn popup_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, msg) = (args[0], args[1]);
    match msg {
        WM_NCCREATE => Some(TRUE),
        WM_CREATE => Some(0),
        WM_MOUSEACTIVATE => Some(MA_NOACTIVATE),
        WM_ERASEBKGND => Some(1),
        // A popup menu has no frame: its edge is drawn with its items.
        WM_NCPAINT => Some(0),
        WM_PAINT => {
            let h = popup_menu_of(user(cpu), hwnd);
            if h != 0 {
                draw_popup(cpu, h, hwnd);
            }
            if let Some(w) = user(cpu).window_mut(hwnd) {
                w.update = Region::new();
                w.paint = 0;
            }
            Some(0)
        }
        _ => super::defwnd::def_window_proc(cpu, args),
    }
}

// Tracking.

/// The kinds of the menu tasks: SetMenu's and DrawMenuBar's
/// SetWindowPos, and tracking.
const K_SET_POS: u32 = 0;
const K_TRACK: u32 = 1;

/// The steps of tracking: going on with what is to do, and going on after
/// CreateWindowExA, showing a popup, GetMessageA and WM_MENUCHAR.
const S_RUN: u16 = 1;
const S_CREATED: u16 = 2;
const S_SHOWN: u16 = 3;
const S_GOT: u16 = 4;
const S_MENUCHAR: u16 = 5;

/// What is tracked: a popup menu of TrackPopupMenu, the system menu
/// (else the menu bar); and the tracking's state: it ends, the top item
/// is clicked a second time to close it, WM_MENUCHAR answers the key
/// that started it.
const M_POPUP: u32 = 0x01;
const M_SYS: u32 = 0x02;
const M_END: u32 = 0x04;
const M_HIDE: u32 = 0x08;
const M_KEY_START: u32 = 0x10;

/// Where a menu that opens is: WM_INITMENUPOPUP is to be sent, its
/// window to be made, to be shown, or it shows.
const L_INIT: u32 = 0;
const L_WINDOW: u32 = 1;
const L_SHOW: u32 = 2;
const L_SHOWN: u32 = 3;

/// The scratch space of tracking: the MSG of its GetMessageA.
const MSG_SCRATCH: u32 = 32;

/// A menu open while tracking: the top one (the menu bar, the system
/// menu or TrackPopupMenu's) and the submenus open under it.
#[derive(Clone, Copy, Debug, Default)]
struct Level {
    menu: u32,
    /// Its #32768 window, or 0.
    hwnd: u32,
    state: u32,
    /// Whether its first item is selected when it shows.
    first: u32,
}

/// Tracking's state, kept in the task's variables.
#[derive(Clone, Debug, Default)]
struct Track {
    owner: u32,
    /// TPM_ flags.
    flags: u32,
    mode: u32,
    /// The command picked, or NONE.
    result: u32,
    /// Where TrackPopupMenu shows its menu; then the last message's
    /// point.
    x: i32,
    y: i32,
    /// A character WM_MENUCHAR is to be sent for.
    key: u32,
    levels: Vec<Level>,
    /// Windows to destroy, and messages to send to the owner.
    destroy: Vec<u32>,
    sends: Vec<[u32; 4]>,
}

impl Track {
    fn encode(&self) -> Vec<u32> {
        let mut v = vec![
            self.owner,
            self.flags,
            self.mode,
            self.result,
            self.x as u32,
            self.y as u32,
            self.key,
            self.levels.len() as u32,
            self.destroy.len() as u32,
            self.sends.len() as u32,
        ];
        for l in &self.levels {
            v.extend([l.menu, l.hwnd, l.state, l.first]);
        }
        v.extend(&self.destroy);
        for s in &self.sends {
            v.extend(s);
        }
        v
    }

    fn decode(v: &[u32]) -> Track {
        let at = |i: usize| v.get(i).copied().unwrap_or(0);
        let (nl, nd, ns) = (at(7) as usize, at(8) as usize, at(9) as usize);
        let mut p = 10;
        let levels = (0..nl)
            .map(|i| {
                let b = p + 4 * i;
                Level {
                    menu: at(b),
                    hwnd: at(b + 1),
                    state: at(b + 2),
                    first: at(b + 3),
                }
            })
            .collect();
        p += 4 * nl;
        let destroy = (0..nd).map(|i| at(p + i)).collect();
        p += nd;
        let sends = (0..ns)
            .map(|i| {
                let b = p + 4 * i;
                [at(b), at(b + 1), at(b + 2), at(b + 3)]
            })
            .collect();
        Track {
            owner: at(0),
            flags: at(1),
            mode: at(2),
            result: at(3),
            x: at(4) as i32,
            y: at(5) as i32,
            key: at(6),
            levels,
            destroy,
            sends,
        }
    }

    /// The innermost menu open.
    fn current(&self) -> usize {
        self.levels.len().saturating_sub(1)
    }

    fn queue(&mut self, msg: u32, wp: u32, lp: u32) {
        self.sends.push([self.owner, msg, wp, lp]);
    }

    /// Tracking starts: WM_ENTERMENULOOP and WM_INITMENU.
    fn enter(&mut self, top: u32) {
        if self.flags & TPM_NONOTIFY == 0 {
            self.queue(WM_ENTERMENULOOP, (self.mode & M_POPUP != 0) as u32, 0);
            self.queue(WM_INITMENU, top, 0);
        }
    }
}

/// DefWindowProcA's WM_SYSCOMMAND with SC_MOUSEMENU or SC_KEYMENU: args
/// are the window, wParam and lParam. SC_MOUSEMENU's lParam is the point
/// on the screen, and its wParam has HTMENU or HTSYSMENU in the low bits;
/// SC_KEYMENU's lParam is the character pressed with Alt, 0 for Alt or
/// F10 alone.
pub fn track_bar(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (mut hwnd, wparam, lparam) = (args[0], args[1], args[2]);
    let mut tr = Track {
        result: NONE,
        ..Track::default()
    };
    if wparam & 0xFFF0 == SC_MOUSEMENU {
        let sys = wparam & 0xF == HTSYSMENU;
        let top = if sys {
            system_bar(cpu, hwnd)
        } else {
            bar_menu(user(cpu), hwnd)
        };
        if get(user(cpu), top).is_none() {
            return Some(0);
        }
        tr.owner = hwnd;
        tr.mode = if sys { M_SYS } else { 0 };
        (tr.x, tr.y) = point_of(lparam);
        open_top(cpu, &mut tr, top);
        if !sys {
            layout_bar_now(cpu, hwnd);
        }
        match hit(cpu, &tr, (tr.x, tr.y)) {
            Some((0, Some(i))) => {
                select(cpu, &mut tr, 0, i, true, false);
                show_sub_popup(cpu, &mut tr, 0, false);
            }
            _ => end(cpu, &mut tr),
        }
        return start(cpu, tr);
    }
    let ch = lparam & 0xFFFF;
    // The window whose menu it is: a child window has none.
    let state = user(cpu);
    while state
        .window(hwnd)
        .is_some_and(|w| w.style & (WS_CHILD | WS_POPUP) == WS_CHILD)
    {
        hwnd = state.parent(hwnd);
    }
    let Some(w) = state.window(hwnd).filter(|_| hwnd != DESKTOP) else {
        return Some(0);
    };
    let (style, menu) = (w.style, w.id);
    let mut top = menu;
    let mut sys = false;
    if menu == 0 || style & WS_MINIMIZE != 0 || ch == b' ' as u32 {
        if style & WS_SYSMENU == 0 {
            return Some(0);
        }
        top = system_bar(cpu, hwnd);
        sys = true;
    }
    if get(user(cpu), top).is_none() {
        return Some(0);
    }
    tr.owner = hwnd;
    tr.mode = if sys { M_SYS } else { 0 };
    open_top(cpu, &mut tr, top);
    if ch != 0 && ch != b' ' as u32 {
        let found = if sys {
            None
        } else {
            find_by_key(user(cpu), top, ch)
        };
        match found {
            Some(i) => {
                select(cpu, &mut tr, 0, i, true, false);
                let r = exec_focused(cpu, &mut tr, 0);
                if r != -2 {
                    finish(cpu, &mut tr, r);
                }
            }
            None => {
                tr.key = ch;
                tr.mode |= M_KEY_START;
            }
        }
    } else if sys {
        select(cpu, &mut tr, 0, 0, true, false);
        if ch == b' ' as u32 {
            show_sub_popup(cpu, &mut tr, 0, true);
        }
    } else {
        move_selection(cpu, &mut tr, 0, 1);
    }
    start(cpu, tr)
}

/// The top menu of the menu bar or system menu is open.
fn open_top(cpu: &mut Cpu, tr: &mut Track, top: u32) {
    if let Some(m) = get_mut(user(cpu), top) {
        clear_focus(m);
        m.hwnd = tr.owner;
    }
    tr.levels.push(Level {
        menu: top,
        state: L_SHOWN,
        ..Level::default()
    });
    tr.enter(top);
}

/// TrackPopupMenu(hmenu, flags, x, y, reserved, hwnd, rect).
pub fn track_popup_menu(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    track_popup(
        cpu,
        args[0],
        args[1],
        (args[2] as i32, args[3] as i32),
        args[5],
    )
}

/// TrackPopupMenuEx(hmenu, flags, x, y, hwnd, params).
pub fn track_popup_menu_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    track_popup(
        cpu,
        args[0],
        args[1],
        (args[2] as i32, args[3] as i32),
        args[4],
    )
}

fn track_popup(cpu: &mut Cpu, h: u32, flags: u32, (x, y): (i32, i32), hwnd: u32) -> Option<u32> {
    let state = user(cpu);
    let Some(m) = get(state, h) else {
        set_error(cpu, ERROR_INVALID_MENU_HANDLE);
        return Some(FALSE);
    };
    if m.flags & MF_POPUP != 0 && m.hwnd != 0 && state.is_window(m.hwnd) {
        set_error(cpu, ERROR_POPUP_ALREADY_ACTIVE);
        return Some(FALSE);
    }
    if !state.is_window(hwnd) {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(FALSE);
    }
    let mut tr = Track {
        owner: hwnd,
        flags,
        mode: M_POPUP,
        result: NONE,
        x,
        y,
        ..Track::default()
    };
    tr.enter(h);
    if flags & TPM_NONOTIFY == 0 {
        tr.queue(WM_INITMENUPOPUP, h, 0);
    }
    tr.levels.push(Level {
        menu: h,
        state: L_WINDOW,
        ..Level::default()
    });
    start(cpu, tr)
}

/// Start tracking's task, with the mouse captured for it.
fn start(cpu: &mut Cpu, tr: Track) -> Option<u32> {
    if tr.mode & M_END == 0 {
        user(cpu).input.capture = tr.owner;
    }
    let mut vars = vec![K_TRACK];
    vars.extend(tr.encode());
    task::start(cpu, TASK_MENU, vars, MSG_SCRATCH)
}

/// Draw a menu open while tracking again, as its selection changed.
fn redraw_level(cpu: &mut Cpu, tr: &Track, lv: usize) {
    let l = tr.levels[lv];
    if l.hwnd != 0 {
        draw_popup(cpu, l.menu, l.hwnd);
    } else if lv == 0 && tr.mode & (M_POPUP | M_SYS) == 0 {
        redraw_bar(cpu, tr.owner);
    }
}

/// Select the item `index` (or NONE) of the open menu `lv`, telling the
/// owner with WM_MENUSELECT when `send`. With nothing selected, `parent`
/// tells it of the item that opened the menu.
fn select(cpu: &mut Cpu, tr: &mut Track, lv: usize, index: u32, send: bool, parent: bool) {
    let h = tr.levels[lv].menu;
    let state = user(cpu);
    let Some(m) = get_mut(state, h) else {
        return;
    };
    if m.items.is_empty() || m.focus == index {
        return;
    }
    if index != NONE && index as usize >= m.items.len() {
        return;
    }
    clear_focus(m);
    m.focus = index;
    let sys = m.flags & MF_SYSMENU;
    let mut note = None;
    if let Some(it) = m.items.get_mut(index as usize) {
        if it.kind & MF_SEPARATOR == 0 {
            it.state |= MF_HILITE;
        }
        let low = if it.kind & MF_POPUP != 0 {
            index
        } else {
            it.id
        };
        note = Some((make_wp(low, it.kind | it.state | sys), h));
    } else if parent && lv > 0 {
        let p = tr.levels[lv - 1].menu;
        if let Some(pm) = get(state, p)
            && let Some(it) = pm.items.get(pm.focus as usize)
        {
            let hi = it.kind | it.state | (pm.flags & MF_SYSMENU);
            note = Some((make_wp(pm.focus, hi), p));
        }
    }
    redraw_level(cpu, tr, lv);
    if send && let Some((wp, lp)) = note {
        tr.queue(WM_MENUSELECT, wp, lp);
    }
}

/// Select the next item (`offset` 1) or the one before (-1) of the open
/// menu `lv` that isn't a separator, going round.
fn move_selection(cpu: &mut Cpu, tr: &mut Track, lv: usize, offset: i64) {
    let Some(m) = get(user(cpu), tr.levels[lv].menu) else {
        return;
    };
    let seps: Vec<bool> = m
        .items
        .iter()
        .map(|it| it.kind & MF_SEPARATOR != 0)
        .collect();
    let count = seps.len() as i64;
    let focus = m.focus;
    if count == 0 {
        return;
    }
    if focus != NONE {
        if count == 1 {
            return;
        }
        let mut i = focus as i64 + offset;
        while (0..count).contains(&i) {
            if !seps[i as usize] {
                select(cpu, tr, lv, i as u32, true, false);
                return;
            }
            i += offset;
        }
    }
    let mut i = if offset > 0 { 0 } else { count - 1 };
    while (0..count).contains(&i) {
        if !seps[i as usize] {
            select(cpu, tr, lv, i as u32, true, false);
            return;
        }
        i += offset;
    }
}

/// Close the menus open under the open menu `lv`.
fn close_above(cpu: &mut Cpu, tr: &mut Track, lv: usize) {
    while tr.levels.len() > lv + 1 {
        let l = tr.levels.pop().unwrap();
        if l.hwnd != 0 {
            tr.destroy.push(l.hwnd);
        }
        if let Some(m) = get_mut(user(cpu), l.menu) {
            if l.hwnd != 0 && m.hwnd == l.hwnd {
                m.hwnd = 0;
            }
            clear_focus(m);
        }
    }
}

/// Open the submenu of the selected item of the open menu `lv`, with its
/// first item selected when `first`: whether there is one to open.
fn show_sub_popup(cpu: &mut Cpu, tr: &mut Track, lv: usize, first: bool) -> bool {
    let state = user(cpu);
    let Some(m) = get(state, tr.levels[lv].menu) else {
        return false;
    };
    let Some(it) = m.items.get(m.focus as usize) else {
        return false;
    };
    if it.kind & MF_POPUP == 0
        || it.state & (MF_GRAYED | MF_DISABLED) != 0
        || get(state, it.submenu).is_none()
    {
        return false;
    }
    let sub = it.submenu;
    close_above(cpu, tr, lv);
    tr.levels.push(Level {
        menu: sub,
        hwnd: 0,
        state: L_INIT,
        first: first as u32,
    });
    true
}

/// Carry out the selected item of the open menu `lv`: its command id when
/// it is posted (or to be returned), -1 when there is none to carry out,
/// -2 when it opened a submenu.
fn exec_focused(cpu: &mut Cpu, tr: &mut Track, lv: usize) -> i64 {
    let state = user(cpu);
    let Some(m) = get(state, tr.levels[lv].menu) else {
        return -1;
    };
    let Some(it) = m.items.get(m.focus as usize) else {
        return -1;
    };
    if it.kind & MF_POPUP != 0 {
        show_sub_popup(cpu, tr, lv, true);
        return -2;
    }
    if it.state & (MF_GRAYED | MF_DISABLED) != 0 || it.kind & MF_SEPARATOR != 0 {
        return -1;
    }
    let (id, sys) = (it.id, m.flags & MF_SYSMENU != 0);
    if tr.flags & TPM_RETURNCMD == 0 {
        let (msg, lp) = if sys {
            (WM_SYSCOMMAND, make_long(tr.x, tr.y))
        } else {
            (WM_COMMAND, 0)
        };
        let tid = state.window(tr.owner).map_or(0, |w| w.tid);
        let (x, y) = state.input.cursor;
        let time = super::message::now_ms(cpu);
        let msg = Msg {
            hwnd: tr.owner,
            message: msg,
            wparam: id,
            lparam: lp,
            time,
            x,
            y,
        };
        super::message::post(cpu, tid, msg);
    }
    id as i64
}

/// Tracking ends with what `exec_focused` returned.
fn finish(cpu: &mut Cpu, tr: &mut Track, r: i64) {
    if r >= 0 {
        tr.result = r as u32;
    }
    end(cpu, tr);
}

/// Tracking ends: the popups go, the menu bar is unselected, and the
/// owner gets WM_MENUSELECT for no menu and WM_EXITMENULOOP.
fn end(cpu: &mut Cpu, tr: &mut Track) {
    if tr.mode & M_END != 0 {
        return;
    }
    tr.mode |= M_END;
    tr.key = 0;
    if !tr.levels.is_empty() {
        close_above(cpu, tr, 0);
        let l = tr.levels.pop().unwrap();
        if let Some(m) = get_mut(user(cpu), l.menu) {
            clear_focus(m);
            if l.hwnd != 0 && m.hwnd == l.hwnd {
                m.hwnd = 0;
            }
        }
        if l.hwnd != 0 {
            tr.destroy.push(l.hwnd);
        } else if tr.mode & (M_POPUP | M_SYS) == 0 {
            redraw_bar(cpu, tr.owner);
        }
    }
    let state = user(cpu);
    if state.input.capture == tr.owner {
        state.input.capture = 0;
    }
    if state.is_window(tr.owner) {
        tr.queue(WM_MENUSELECT, make_wp(0, 0xFFFF), 0);
        tr.queue(WM_EXITMENULOOP, (tr.mode & M_POPUP != 0) as u32, 0);
    }
}

/// The open menu and item at the point (`x`, `y`) on the screen: the
/// innermost first, and on the menu bar or the system menu button for
/// the top one.
fn hit(cpu: &mut Cpu, tr: &Track, (x, y): (i32, i32)) -> Option<(usize, Option<u32>)> {
    let state = user(cpu);
    for (lv, l) in tr.levels.iter().enumerate().rev() {
        if l.hwnd != 0 {
            let r = state.screen_rect(l.hwnd);
            if r.contains(x, y) {
                return Some((lv, item_at(state, l.menu, x - r.left, y - r.top)));
            }
        } else if lv == 0 && tr.mode & M_SYS != 0 {
            if sys_button(state, tr.owner).is_some_and(|r| r.contains(x, y)) {
                return Some((0, Some(0)));
            }
        } else if lv == 0 && tr.mode & M_POPUP == 0 {
            let (wx, wy) = state.window_origin(tr.owner);
            if bar_rect(state, tr.owner).is_some_and(|r| r.contains(x - wx, y - wy)) {
                return Some((0, item_at(state, l.menu, x - wx, y - wy)));
            }
        }
    }
    None
}

/// Select the item `i` of the open menu `lv`, closing what was open under
/// it.
fn switch_to(cpu: &mut Cpu, tr: &mut Track, lv: usize, i: u32) {
    close_above(cpu, tr, lv);
    select(cpu, tr, lv, i, true, false);
}

fn mouse(cpu: &mut Cpu, tr: &mut Track, msg: &Msg) {
    let found = hit(cpu, tr, (msg.x, msg.y));
    let right = tr.flags & TPM_RIGHTBUTTON != 0;
    match msg.message {
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => button_down(cpu, tr, found),
        WM_RBUTTONDOWN | WM_RBUTTONDBLCLK if right => button_down(cpu, tr, found),
        WM_LBUTTONUP => button_up(cpu, tr, found),
        WM_RBUTTONUP if right => button_up(cpu, tr, found),
        WM_MOUSEMOVE => match found {
            Some((lv, Some(i))) => {
                if focus(user(cpu), tr.levels[lv].menu) != i {
                    switch_to(cpu, tr, lv, i);
                    show_sub_popup(cpu, tr, lv, false);
                }
            }
            _ => {
                let cur = tr.current();
                select(cpu, tr, cur, NONE, true, true);
            }
        },
        _ => {}
    }
}

/// A button goes down: on an item it is selected and its submenu opens;
/// anywhere else tracking ends.
fn button_down(cpu: &mut Cpu, tr: &mut Track, found: Option<(usize, Option<u32>)>) {
    let Some((lv, Some(i))) = found else {
        end(cpu, tr);
        return;
    };
    if focus(user(cpu), tr.levels[lv].menu) != i {
        switch_to(cpu, tr, lv, i);
    }
    if tr.levels.len() == lv + 1 {
        show_sub_popup(cpu, tr, lv, false);
    }
}

/// A button goes up: on the selected item its command is carried out;
/// on the top item a second time, its submenu closes and tracking ends.
fn button_up(cpu: &mut Cpu, tr: &mut Track, found: Option<(usize, Option<u32>)>) {
    let Some((lv, index)) = found else {
        if tr.mode & M_POPUP == 0 {
            end(cpu, tr);
        }
        return;
    };
    let h = tr.levels[lv].menu;
    if let Some(i) = index
        && focus(user(cpu), h) == i
    {
        let popup = item(user(cpu), (h, i as usize)).is_some_and(|it| it.kind & MF_POPUP != 0);
        if !popup {
            let r = exec_focused(cpu, tr, lv);
            if r >= 0 {
                finish(cpu, tr, r);
            }
            return;
        }
        if lv == 0 && tr.mode & M_HIDE != 0 {
            end(cpu, tr);
            return;
        }
    }
    if lv == 0 {
        tr.mode |= M_HIDE;
    }
}

/// A key goes down: what isn't for the menu is translated, for its
/// character.
fn key(cpu: &mut Cpu, tr: &mut Track, vk: u32, scratch: u32) -> Option<Next> {
    let cur = tr.current();
    let bar = tr.mode & M_POPUP == 0;
    match vk {
        VK_MENU | VK_F10 => end(cpu, tr),
        VK_HOME | VK_END => {
            select(cpu, tr, cur, NONE, false, false);
            move_selection(cpu, tr, cur, if vk == VK_HOME { 1 } else { -1 });
        }
        VK_UP | VK_DOWN => {
            if cur == 0 && bar {
                show_sub_popup(cpu, tr, 0, true);
            } else {
                move_selection(cpu, tr, cur, if vk == VK_UP { -1 } else { 1 });
            }
        }
        VK_LEFT => {
            let prev = cur.saturating_sub(1);
            close_above(cpu, tr, prev);
            if prev == 0 && bar {
                move_selection(cpu, tr, 0, -1);
                if cur > 0 {
                    show_sub_popup(cpu, tr, 0, true);
                }
            }
        }
        VK_RIGHT => {
            if (!bar || cur > 0) && show_sub_popup(cpu, tr, cur, true) {
                return None;
            }
            if bar {
                close_above(cpu, tr, 0);
                move_selection(cpu, tr, 0, 1);
                if cur > 0 {
                    show_sub_popup(cpu, tr, 0, true);
                }
            }
        }
        VK_ESCAPE => {
            if cur > 0 {
                close_above(cpu, tr, cur - 1);
            } else {
                end(cpu, tr);
            }
        }
        _ => {
            return Some(Next::call(
                address_of("TranslateMessage"),
                &[scratch],
                S_RUN,
            ));
        }
    }
    None
}

/// The item of `h` whose mnemonic (the character after a single '&') is
/// `ch`, in either case.
fn find_by_key(state: &State, h: u32, ch: u32) -> Option<u32> {
    let up = (ch as u8).to_ascii_uppercase();
    let m = get(state, h)?;
    m.items
        .iter()
        .position(|it| {
            let Some(t) = &it.text else {
                return false;
            };
            let mut j = 0;
            while j + 1 < t.len() {
                if t[j] == b'&' {
                    if t[j + 1] == b'&' {
                        j += 2;
                        continue;
                    }
                    return t[j + 1].to_ascii_uppercase() == up;
                }
                j += 1;
            }
            false
        })
        .map(|i| i as u32)
}

/// A character: Enter and Space carry out the selected item, a mnemonic
/// its item; for another, the owner is asked with WM_MENUCHAR.
fn character(cpu: &mut Cpu, tr: &mut Track, ch: u32) {
    let cur = tr.current();
    if ch == b'\r' as u32 || ch == b' ' as u32 {
        let r = exec_focused(cpu, tr, cur);
        if r != -2 {
            finish(cpu, tr, r);
        }
        return;
    }
    if ch < 32 {
        return;
    }
    match find_by_key(user(cpu), tr.levels[cur].menu, ch) {
        Some(i) => {
            select(cpu, tr, cur, i, true, false);
            let r = exec_focused(cpu, tr, cur);
            if r != -2 {
                finish(cpu, tr, r);
            }
        }
        None => tr.key = ch,
    }
}

/// What WM_MENUCHAR returned: 2 in the high word picks the item in the
/// low word, 1 ends tracking.
fn menu_char(cpu: &mut Cpu, tr: &mut Track, value: u32) {
    let started = tr.mode & M_KEY_START != 0;
    tr.mode &= !M_KEY_START;
    tr.key = 0;
    if tr.mode & M_END != 0 {
        return;
    }
    let cur = tr.current();
    match value >> 16 {
        2 => {
            select(cpu, tr, cur, value & 0xFFFF, true, false);
            let r = exec_focused(cpu, tr, cur);
            if r != -2 {
                finish(cpu, tr, r);
            }
        }
        1 => end(cpu, tr),
        _ if started => end(cpu, tr),
        _ => {}
    }
}

/// A message GetMessageA took while tracking.
fn got(cpu: &mut Cpu, tr: &mut Track, value: u32, scratch: u32) -> Next {
    let msg = Msg::read(cpu, scratch);
    tr.x = msg.x;
    tr.y = msg.y;
    if value == 0 || value == NONE {
        // WM_QUIT ends tracking and stays for the program's loop.
        if msg.message == WM_QUIT {
            super::message::post_quit_message(cpu, &[msg.wparam]);
        }
        end(cpu, tr);
        return reconcile(cpu, tr, scratch);
    }
    match msg.message {
        WM_CANCELMODE => end(cpu, tr),
        WM_MOUSEMOVE..=WM_MBUTTONDBLCLK => mouse(cpu, tr, &msg),
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            if let Some(next) = key(cpu, tr, msg.wparam, scratch) {
                return next;
            }
        }
        WM_CHAR | WM_SYSCHAR => character(cpu, tr, msg.wparam),
        WM_KEYDOWN..=WM_SYSDEADCHAR => {}
        _ => return Next::call(address_of("DispatchMessageA"), &[scratch], S_RUN),
    }
    reconcile(cpu, tr, scratch)
}

/// Where the open menu `lv`, `w` by `h`, goes on the screen: under its
/// menu bar item or the system menu button, or beside its item in the
/// menu it opens from; on the other side when there is no room.
fn popup_pos(cpu: &mut Cpu, tr: &Track, lv: usize, (w, h): (i32, i32)) -> (i32, i32) {
    let (sw, sh) = sysparams::screen_size(cpu);
    let state = user(cpu);
    let (mut x, mut y);
    let mut other_x = None;
    let mut other_y = None;
    if lv == 0 {
        (x, y) = (tr.x, tr.y);
        if tr.flags & TPM_RIGHTALIGN != 0 {
            x -= w;
        } else if tr.flags & TPM_CENTERALIGN != 0 {
            x -= w / 2;
        }
        if tr.flags & TPM_BOTTOMALIGN != 0 {
            y -= h;
        } else if tr.flags & TPM_VCENTERALIGN != 0 {
            y -= h / 2;
        }
    } else {
        let parent = tr.levels[lv - 1];
        let r = get(state, parent.menu)
            .and_then(|m| m.items.get(m.focus as usize))
            .map_or(Rect::default(), |it| it.rect);
        if parent.hwnd != 0 {
            let pr = state.screen_rect(parent.hwnd);
            x = pr.left + r.right;
            y = pr.top + r.top - POPUP_BORDER;
            other_x = Some(pr.left + r.left - w);
        } else if tr.mode & M_SYS != 0 {
            let b = sys_button(state, tr.owner).unwrap_or_default();
            (x, y) = (b.left, b.bottom);
            other_y = Some(b.top - h);
        } else {
            let (wx, wy) = state.window_origin(tr.owner);
            x = wx + r.left;
            y = wy + r.bottom;
            other_y = Some(wy + r.top - h);
        }
    }
    if x + w > sw {
        x = other_x.filter(|&o| o >= 0).unwrap_or(sw - w);
    }
    if y + h > sh {
        y = other_y.filter(|&o| o >= 0).unwrap_or(sh - h);
    }
    (x.max(0), y.max(0))
}

/// Do the next thing tracking has to do: destroy the windows of menus
/// that closed, send the owner what it is to be told, open the menus to
/// open; then end, or take the next message.
fn reconcile(cpu: &mut Cpu, tr: &mut Track, scratch: u32) -> Next {
    loop {
        if tr.mode & M_END == 0 {
            let state = user(cpu);
            let gone = tr.levels.iter().any(|l| get(state, l.menu).is_none());
            if gone || !state.is_window(tr.owner) {
                end(cpu, tr);
            }
        }
        if !tr.destroy.is_empty() {
            let hwnd = tr.destroy.remove(0);
            if user(cpu).is_window(hwnd) {
                return Next::call(address_of("DestroyWindow"), &[hwnd], S_RUN);
            }
            continue;
        }
        if !tr.sends.is_empty() {
            let [hwnd, msg, wp, lp] = tr.sends.remove(0);
            return super::window::send(hwnd, msg, wp, lp, S_RUN);
        }
        if tr.key != 0 && tr.mode & M_END == 0 {
            let h = tr.levels[tr.current()].menu;
            let flags = get(user(cpu), h).map_or(0, |m| m.flags);
            let wp = make_wp(tr.key, flags);
            return super::window::send(tr.owner, WM_MENUCHAR, wp, h, S_MENUCHAR);
        }
        if let Some(lv) = tr.levels.iter().position(|l| l.state != L_SHOWN) {
            let l = tr.levels[lv];
            match l.state {
                L_INIT => {
                    tr.levels[lv].state = L_WINDOW;
                    if tr.flags & TPM_NONOTIFY == 0 && lv > 0 {
                        let parent = tr.levels[lv - 1].menu;
                        let (pos, sys) = get(user(cpu), parent)
                            .map_or((0, 0), |m| (m.focus, (m.flags & MF_SYSMENU != 0) as u32));
                        let lp = make_wp(pos, sys);
                        return super::window::send(tr.owner, WM_INITMENUPOPUP, l.menu, lp, S_RUN);
                    }
                }
                L_WINDOW => {
                    if lv > 0 && tr.mode & M_SYS != 0 && lv == 1 {
                        init_sys_popup(cpu, l.menu, tr.owner);
                    }
                    if let Some(m) = get_mut(user(cpu), l.menu) {
                        clear_focus(m);
                    }
                    let size = layout_popup(cpu, l.menu);
                    let (x, y) = popup_pos(cpu, tr, lv, size);
                    tr.levels[lv].state = L_SHOW;
                    let instance = user(cpu).window(tr.owner).map_or(0, |w| w.instance);
                    let ex = WS_EX_TOOLWINDOW | WS_EX_TOPMOST;
                    let args = [
                        ex,
                        POPUP_ATOM,
                        0,
                        WS_POPUP,
                        x as u32,
                        y as u32,
                        size.0 as u32,
                        size.1 as u32,
                        tr.owner,
                        0,
                        instance,
                        l.menu,
                    ];
                    return Next::call(address_of("CreateWindowExA"), &args, S_CREATED);
                }
                _ => {
                    tr.levels[lv].state = L_SHOWN;
                    let flags = SWP_SHOWWINDOW | SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE;
                    let args = [l.hwnd, HWND_TOPMOST, 0, 0, 0, 0, flags];
                    return Next::call(address_of("SetWindowPos"), &args, S_SHOWN);
                }
            }
            continue;
        }
        if tr.mode & M_END != 0 {
            let result = if tr.mode & M_POPUP == 0 {
                0
            } else if tr.flags & TPM_RETURNCMD == 0 {
                TRUE
            } else if tr.result == NONE {
                0
            } else {
                tr.result
            };
            return Next::Done(result);
        }
        return Next::call(address_of("GetMessageA"), &[scratch, 0, 0, 0], S_GOT);
    }
}

/// The steps of the menu tasks.
pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    if t.var(0) == K_SET_POS {
        return match t.step {
            0 => {
                let flags =
                    SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE | SWP_NOZORDER | SWP_FRAMECHANGED;
                Next::call(
                    address_of("SetWindowPos"),
                    &[t.var(1), 0, 0, 0, 0, 0, flags],
                    1,
                )
            }
            _ => Next::Done(TRUE),
        };
    }
    let mut tr = Track::decode(t.vars.get(1..).unwrap_or(&[]));
    let next = match t.step {
        S_CREATED => {
            if let Some(lv) = tr.levels.iter().position(|l| l.state == L_SHOW) {
                if value == 0 {
                    tr.levels.truncate(lv);
                    end(cpu, &mut tr);
                } else {
                    tr.levels[lv].hwnd = value;
                    if let Some(m) = get_mut(user(cpu), tr.levels[lv].menu) {
                        m.hwnd = value;
                    }
                }
            }
            reconcile(cpu, &mut tr, t.scratch)
        }
        S_SHOWN => {
            if let Some(lv) = tr.levels.iter().rposition(|l| l.hwnd != 0) {
                let l = tr.levels[lv];
                draw_popup(cpu, l.menu, l.hwnd);
                if l.first != 0 {
                    tr.levels[lv].first = 0;
                    move_selection(cpu, &mut tr, lv, 1);
                }
            }
            reconcile(cpu, &mut tr, t.scratch)
        }
        S_GOT => got(cpu, &mut tr, value, t.scratch),
        S_MENUCHAR => {
            menu_char(cpu, &mut tr, value);
            reconcile(cpu, &mut tr, t.scratch)
        }
        _ => reconcile(cpu, &mut tr, t.scratch),
    };
    t.vars.truncate(1);
    t.vars.extend(tr.encode());
    next
}

crate::state_fields!(Item {
    kind,
    state,
    id,
    submenu,
    checked,
    unchecked,
    data,
    text,
    bitmap,
    rect,
    tab
});

crate::state_fields!(Menu {
    pid,
    flags,
    items,
    width,
    height,
    hwnd,
    focus,
    help_id
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::win32::user32::window::WS_CAPTION;

    /// Windows 98 SE's menu bar on a WS_OVERLAPPEDWINDOW at (20, 20),
    /// with "File", "Special", "Window" and "Help" in 8-point MS Sans
    /// Serif: GetMenuItemRect gives (24, 43)-(52, 61) for "File" and
    /// (150, 43)-(184, 61) for "Help", items 18 high on a bar 19 high.
    #[test]
    fn bar_items_are_laid_out_as_windows_98_does() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        super::super::init(&mut cpu);
        let h = new_menu(user(&mut cpu), 0, 0);
        for (i, text) in ["&File", "&Special", "&Window", "&Help"].iter().enumerate() {
            let info = info_of(0, i as u32 + 1, 0, Some(text.as_bytes().to_vec()));
            insert_item(&mut cpu, h, NONE, MF_BYPOSITION, &info);
        }
        let w = Window {
            style: WS_CAPTION | WS_SYSMENU | WS_THICKFRAME,
            rect: Rect::new(20, 20, 440, 220),
            id: h,
            ..Window::default()
        };
        let hwnd = user(&mut cpu).new_window(w).expect("a window");
        layout_bar_now(&mut cpu, hwnd);
        let state = user(&mut cpu);
        let (x, y) = state.window_origin(hwnd);
        let rect = |pos| item(state, (h, pos)).unwrap().rect.offset(x, y);
        assert_eq!(rect(0), Rect::new(24, 43, 52, 61), "File");
        assert_eq!(rect(3), Rect::new(150, 43, 184, 61), "Help");
    }

    /// Windows 98 SE's File popup from the same window: at (24, 61), 131
    /// by 66, items 17 high and a separator 9 high, item text 22 pixels
    /// in and "Ctrl+S" at x = 106 on the screen.
    #[test]
    fn popup_items_are_laid_out_as_windows_98_does() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        super::super::init(&mut cpu);
        let h = new_menu(user(&mut cpu), MF_POPUP, 0);
        let items: [(u32, Option<&str>); 4] = [
            (0, Some("&Open...")),
            (0, Some("Save &As...\tCtrl+S")),
            (MF_SEPARATOR, None),
            (0, Some("E&xit")),
        ];
        for (i, (flags, text)) in items.iter().enumerate() {
            let text = text.map(|t| t.as_bytes().to_vec());
            let info = info_of(*flags, i as u32 + 1, 0, text);
            insert_item(&mut cpu, h, NONE, MF_BYPOSITION, &info);
        }
        assert_eq!(layout_popup(&mut cpu, h), (131, 66));
        let state = user(&mut cpu);
        let rect = |pos| item(state, (h, pos)).unwrap().rect;
        assert_eq!(rect(0), Rect::new(3, 3, 128, 20), "Open");
        assert_eq!(rect(1), Rect::new(3, 20, 128, 37), "Save As");
        assert_eq!(rect(2), Rect::new(3, 37, 128, 46), "the separator");
        assert_eq!(rect(3), Rect::new(3, 46, 128, 63), "Exit");
        assert_eq!(item(state, (h, 1)).unwrap().tab + 24, 106, "Ctrl+S");
        assert_eq!(text_x(), 22);
    }
}
