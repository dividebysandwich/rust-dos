//! USER32.DLL: windows, their classes and messages, input, painting,
//! dialogs and the basic controls, on the desktop GDI32 draws.
//!
//! USER's state is the system's, as on Windows 95, where the windows of
//! all programs are in one tree: `State`, in `Win32::user`. Window, menu
//! and icon handles are small numbers, as USER's 16-bit heap made them.
//! Functions that send messages or call the program's callbacks are
//! tasks (task.rs). Like Windows 95's, the Unicode (W) functions are
//! stubs that fail with ERROR_CALL_NOT_IMPLEMENTED, but for the few
//! Windows 95 has.

pub mod class;
pub mod controls;
pub mod cursor;
pub mod ddeml;
pub mod defwnd;
pub mod dialog;
pub mod hook;
pub mod input;
pub mod menu;
pub mod message;
pub mod monitor;
pub mod notepad;
pub mod paint;
pub mod progman;
pub mod rect;
pub mod resource;
pub mod saver;
pub mod session;
pub mod sysparams;
pub mod text;
pub mod window;
pub mod winhelp;

use super::gdi32::Rect;
use super::stubs::{Api, Conv, Dll};
use crate::cpu::Cpu;

pub use class::Class;
pub use message::{Msg, Queue};
pub use window::Window;

pub static DLL: Dll = Dll {
    name: "USER32.DLL",
    base: 0xBFF5_0000,
    apis: APIS,
    vars: &[],
    init: None,
    term: None,
    vtables: &[],
};

const fn stdcall(
    name: &'static str,
    ordinal: u16,
    args: u8,
    handler: super::stubs::Handler,
) -> Api {
    Api {
        name,
        ordinal,
        args,
        conv: Conv::Stdcall,
        handler,
    }
}

const fn cdecl(name: &'static str, ordinal: u16, args: u8, handler: super::stubs::Handler) -> Api {
    Api {
        name,
        ordinal,
        args,
        conv: Conv::Cdecl,
        handler,
    }
}

pub const TRUE: u32 = 1;
pub const FALSE: u32 = 0;

/// Error codes (GetLastError).
pub const ERROR_INVALID_HANDLE: u32 = 6;
pub const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
pub const ERROR_INVALID_PARAMETER: u32 = 87;
pub const ERROR_CALL_NOT_IMPLEMENTED: u32 = 120;
pub const ERROR_INVALID_WINDOW_HANDLE: u32 = 1400;
pub const ERROR_INVALID_MENU_HANDLE: u32 = 1401;
pub const ERROR_INVALID_CURSOR_HANDLE: u32 = 1402;
pub const ERROR_INVALID_ACCEL_HANDLE: u32 = 1403;
pub const ERROR_INVALID_HOOK_HANDLE: u32 = 1404;
pub const ERROR_WINDOW_OF_OTHER_THREAD: u32 = 1408;
pub const ERROR_CHILD_WINDOW_MENU: u32 = 1436;
pub const ERROR_NO_SCROLLBARS: u32 = 1447;
pub const ERROR_CLASS_ALREADY_EXISTS: u32 = 1410;
pub const ERROR_CLASS_DOES_NOT_EXIST: u32 = 1411;
pub const ERROR_CLASS_HAS_WINDOWS: u32 = 1412;
pub const ERROR_INVALID_INDEX: u32 = 1413;
pub const ERROR_INVALID_ICON_HANDLE: u32 = 1414;
pub const ERROR_TLW_WITH_WSCHILD: u32 = 1406;
pub const ERROR_RESOURCE_NAME_NOT_FOUND: u32 = 1814;
pub const ERROR_INVALID_FLAGS: u32 = 1004;
pub const ERROR_NOT_ENOUGH_QUOTA: u32 = 1816;

/// Where window handles start, and how far apart they are.
const HWND_BASE: u32 = 0x0400;
const HWND_STEP: u32 = 4;
/// The most windows there are at once.
const MAX_WINDOWS: usize = 0x3000;

/// The desktop's handle: the first window's.
pub const DESKTOP: u32 = HWND_BASE;

/// USER's state: the system's.
#[derive(Debug, Default)]
pub struct State {
    /// The windows, by their handles' places; the desktop first.
    pub windows: Vec<Option<Window>>,
    /// Where the search for a free place for a new window starts, so
    /// that a handle isn't given out again at once.
    pub next_window: usize,
    pub classes: Vec<Option<Class>>,
    /// The last class atom given out.
    pub last_atom: u16,
    pub queues: Vec<Queue>,
    pub hooks: Vec<hook::Hook>,
    pub timers: Vec<message::Timer>,
    pub menus: Vec<Option<menu::Menu>>,
    pub icons: Vec<Option<resource::Icon>>,
    pub accels: Vec<Option<Vec<resource::Accel>>>,
    pub input: input::Input,
    /// The colors of the parts of the screen (GetSysColor), and their
    /// brushes.
    pub colors: Vec<u32>,
    pub color_brushes: Vec<u32>,
    /// Whether the desktop is on the screen: the display mode for it is
    /// set when the first window shows or a program draws on the screen.
    pub desktop: bool,
    /// The last id given to a message sent to another thread.
    pub last_sent: u32,
    pub clipboard: input::Clipboard,
    /// The caret.
    pub caret: input::Caret,
    /// The values SystemParametersInfoA changed.
    pub params: Vec<(u32, u32)>,
    /// The message ids RegisterWindowMessageA gave out, by name.
    pub registered: Vec<String>,
    /// The threads running hooks, the innermost last: each thread and
    /// the hooks' kind; and the number of the last hook set.
    pub in_hooks: Vec<(u32, i32)>,
    pub last_hook: u32,
    /// The screens kept under the windows of classes with CS_SAVEBITS
    /// that show (`paint::SavedBits`).
    pub saved_bits: Vec<paint::SavedBits>,
    /// The help viewer (winhelp), which save states leave out.
    pub help: winhelp::Help,
}

impl State {
    /// The window `hwnd`, if it is one.
    pub fn window(&self, hwnd: u32) -> Option<&Window> {
        self.windows.get(slot(hwnd)?)?.as_ref()
    }

    pub fn window_mut(&mut self, hwnd: u32) -> Option<&mut Window> {
        self.windows.get_mut(slot(hwnd)?)?.as_mut()
    }

    /// Whether `hwnd` is a window (and not one being taken apart).
    pub fn is_window(&self, hwnd: u32) -> bool {
        self.window(hwnd).is_some()
    }

    /// A place for a new window: its handle, or None when there are too
    /// many.
    pub(crate) fn new_window(&mut self, window: Window) -> Option<u32> {
        let len = self.windows.len();
        let free = (0..len)
            .map(|i| (self.next_window + i) % len.max(1))
            .find(|&i| i > 0 && self.windows[i].is_none());
        let i = match free {
            Some(i) if len >= 64 => i,
            _ if len < MAX_WINDOWS => {
                self.windows.push(None);
                len
            }
            Some(i) => i,
            None => return None,
        };
        self.next_window = i + 1;
        self.windows[i] = Some(window);
        Some(handle(i))
    }

    pub fn queue(&self, tid: u32) -> Option<&Queue> {
        self.queues.iter().find(|q| q.tid == tid)
    }

    /// The queue of the thread `tid`, made if it has none.
    pub fn queue_mut(&mut self, tid: u32) -> &mut Queue {
        match self.queues.iter().position(|q| q.tid == tid) {
            Some(i) => &mut self.queues[i],
            None => {
                self.queues.push(Queue {
                    tid,
                    ..Queue::default()
                });
                self.queues.last_mut().unwrap()
            }
        }
    }

    /// The windows of the thread `tid`, in the tree's order (parents
    /// before their children).
    pub fn windows_of(&self, tid: u32) -> Vec<u32> {
        self.windows
            .iter()
            .enumerate()
            .filter_map(|(i, w)| w.as_ref().filter(|w| w.tid == tid).map(|_| handle(i)))
            .collect()
    }

    /// The top-level windows, top of the z-order first.
    pub fn top_levels(&self) -> Vec<u32> {
        self.window(DESKTOP)
            .map(|d| d.children.clone())
            .unwrap_or_default()
    }

    /// The parent of `hwnd`, as GetAncestor(GA_PARENT) has it: the
    /// desktop for a top-level window.
    pub fn parent(&self, hwnd: u32) -> u32 {
        self.window(hwnd).map_or(0, |w| w.parent)
    }

    /// Whether `hwnd` is a child window (WS_CHILD, with a parent that
    /// isn't the desktop).
    pub fn is_child_window(&self, hwnd: u32) -> bool {
        self.window(hwnd)
            .is_some_and(|w| w.style & window::WS_CHILD != 0 && w.parent != DESKTOP)
    }

    /// The top-level window `hwnd` is in, or itself.
    pub fn top_level(&self, mut hwnd: u32) -> u32 {
        while let Some(w) = self.window(hwnd) {
            if w.parent == DESKTOP || w.parent == 0 || w.style & window::WS_CHILD == 0 {
                return hwnd;
            }
            hwnd = w.parent;
        }
        hwnd
    }

    /// Whether `child` is a descendant of `parent`, as IsChild has it:
    /// through parents that are child windows.
    pub fn is_descendant(&self, parent: u32, child: u32) -> bool {
        let mut hwnd = child;
        while let Some(w) = self.window(hwnd) {
            if w.style & window::WS_CHILD == 0 {
                return false;
            }
            if w.parent == parent {
                return true;
            }
            hwnd = w.parent;
        }
        false
    }

    /// The top-left corner of the client area of `hwnd` on the screen.
    pub fn client_origin(&self, hwnd: u32) -> (i32, i32) {
        let Some(w) = self.window(hwnd) else {
            return (0, 0);
        };
        let (px, py) = if hwnd == DESKTOP {
            (0, 0)
        } else {
            self.client_origin(w.parent)
        };
        (px + w.client.left, py + w.client.top)
    }

    /// The top-left corner of `hwnd` on the screen.
    pub fn window_origin(&self, hwnd: u32) -> (i32, i32) {
        let Some(w) = self.window(hwnd) else {
            return (0, 0);
        };
        let (px, py) = if hwnd == DESKTOP {
            (0, 0)
        } else {
            self.client_origin(w.parent)
        };
        (px + w.rect.left, py + w.rect.top)
    }

    /// The window's rectangle on the screen.
    pub fn screen_rect(&self, hwnd: u32) -> Rect {
        let Some(w) = self.window(hwnd) else {
            return Rect::default();
        };
        let (x, y) = self.window_origin(hwnd);
        Rect::sized(x, y, w.rect.width(), w.rect.height())
    }

    /// Its client area on the screen.
    pub fn screen_client(&self, hwnd: u32) -> Rect {
        let Some(w) = self.window(hwnd) else {
            return Rect::default();
        };
        let (x, y) = self.client_origin(hwnd);
        Rect::sized(x, y, w.client.width(), w.client.height())
    }

    /// Whether the window and all its parents are visible.
    pub fn is_visible(&self, mut hwnd: u32) -> bool {
        loop {
            let Some(w) = self.window(hwnd) else {
                return false;
            };
            if w.style & window::WS_VISIBLE == 0 {
                return false;
            }
            if hwnd == DESKTOP || w.parent == 0 {
                return true;
            }
            hwnd = w.parent;
        }
    }

    pub fn color(&self, index: u32) -> u32 {
        self.colors.get(index as usize).copied().unwrap_or(0)
    }
}

/// A window handle's place in `State::windows`.
fn slot(hwnd: u32) -> Option<usize> {
    let offset = hwnd.checked_sub(HWND_BASE)?;
    (offset % HWND_STEP == 0 && hwnd < 0x1_0000).then_some((offset / HWND_STEP) as usize)
}

fn handle(slot: usize) -> u32 {
    HWND_BASE + HWND_STEP * slot as u32
}

/// USER's state.
pub fn user(cpu: &mut Cpu) -> &mut State {
    &mut cpu.bus.win32.user
}

/// The id of the running thread, or 0 in the idle loop.
pub fn current_tid(cpu: &Cpu) -> u32 {
    super::running(cpu)
        .and_then(|p| p.current_thread())
        .map_or(0, |t| t.id)
}

pub fn current_pid(cpu: &Cpu) -> u32 {
    super::running(cpu).map_or(0, |p| p.id)
}

pub fn set_error(cpu: &mut Cpu, code: u32) {
    super::process(cpu).last_error = code;
}

/// A function Windows 95 has only as a stub that fails.
pub fn not_implemented(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    set_error(cpu, ERROR_CALL_NOT_IMPLEMENTED);
    Some(FALSE)
}

/// A function that does nothing and succeeds.
fn succeed(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(TRUE)
}

/// A function that does nothing and returns 0.
fn nothing(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// The string at `at`, one char per byte. None if a page isn't there.
pub fn string_at(cpu: &mut Cpu, at: u32) -> Option<String> {
    let bytes = cpu.bus.guest_read_asciiz(at, 0x1_0000);
    (!cpu.bus.guest_faulted()).then(|| crate::dosstr::from_bytes(&bytes))
}

/// The bytes of the string at `at`.
pub fn bytes_at(cpu: &mut Cpu, at: u32) -> Option<Vec<u8>> {
    let bytes = cpu.bus.guest_read_asciiz(at, 0x1_0000);
    (!cpu.bus.guest_faulted()).then_some(bytes)
}

/// A name or a number (MAKEINTRESOURCE, MAKEINTATOM): numbers are below
/// 10000h.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Name {
    Id(u16),
    Text(String),
}

impl Name {
    /// The name at `at`: a number when it is below 10000h, or "#123".
    pub fn read(cpu: &mut Cpu, at: u32) -> Option<Name> {
        if at < 0x1_0000 {
            return Some(Name::Id(at as u16));
        }
        let text = string_at(cpu, at)?;
        if let Some(n) = text.strip_prefix('#').and_then(|n| n.parse::<u16>().ok()) {
            return Some(Name::Id(n));
        }
        Some(Name::Text(text))
    }
}

/// Copy the bytes `text` to the buffer at `at` of `size` bytes, cut to
/// fit with a 0 after them: how many were copied, without the 0.
pub fn copy_out(cpu: &mut Cpu, at: u32, size: u32, text: &[u8]) -> u32 {
    if size == 0 || at == 0 {
        return 0;
    }
    let n = text.len().min(size as usize - 1);
    let mut bytes = text[..n].to_vec();
    bytes.push(0);
    cpu.bus.guest_write_bytes(at, &bytes);
    n as u32
}

/// The words of a structure at `at`.
pub fn read_words(cpu: &mut Cpu, at: u32, n: usize) -> Vec<u32> {
    (0..n as u32)
        .map(|i| cpu.bus.guest_read_32(at + 4 * i))
        .collect()
}

pub fn write_words(cpu: &mut Cpu, at: u32, words: &[u32]) {
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    cpu.bus.guest_write_bytes(at, &bytes);
}

/// LOWORD and HIWORD as signed numbers, as points in lParam are.
pub fn point_of(value: u32) -> (i32, i32) {
    (
        value as u16 as i16 as i32,
        (value >> 16) as u16 as i16 as i32,
    )
}

pub fn make_long(lo: i32, hi: i32) -> u32 {
    (lo as u16 as u32) | (hi as u16 as u32) << 16
}

/// USER32's place in `stubs::DLLS`.
fn dll_index() -> usize {
    super::stubs::DLLS
        .iter()
        .position(|d| d.name == DLL.name)
        .expect("USER32 is a built-in DLL")
}

/// The address of USER32's function `name`, for tasks that call it as
/// the program would and for window procedures of the system's classes.
pub fn address_of(name: &str) -> u32 {
    let index = APIS
        .iter()
        .position(|a| a.name == name)
        .unwrap_or_else(|| panic!("USER32 has no {}", name));
    super::stubs::stub_address(dll_index(), index)
}

/// The system starts: the desktop window, the system's classes, the
/// arrow as the cursor, which Windows shows from the start until a
/// program sets another, and the screen saver's settings.
pub fn init(cpu: &mut Cpu) {
    if !user(cpu).windows.is_empty() {
        return;
    }
    saver::init(cpu);
    sysparams::init_colors(cpu);
    class::register_system(cpu);
    window::create_desktop(cpu);
    let arrow = resource::system_cursor(cpu, resource::IDC_ARROW);
    cursor::set(cpu, arrow);
    // The cursor starts in the middle of the screen, where the desktop puts
    // it as it comes on the screen (`paint::ensure_desktop`). A DirectDraw
    // mode may already be set when USER32 starts. Then the screen is that
    // mode's, so the middle is the mode's too: the desktop's middle can lie
    // off a smaller mode, where the cursor can't be.
    let (w, h) = sysparams::screen_size(cpu);
    user(cpu).input.cursor = (w / 2, h / 2);
}

/// The process `pid` ended: its windows, classes, hooks, timers, menus,
/// icons and accelerator tables go. With no window left, the screen goes
/// back to text.
pub fn process_ended(cpu: &mut Cpu, pid: u32, tids: &[u32]) {
    if user(cpu).windows.is_empty() {
        return;
    }
    forget_windows(cpu, |w| w.pid == pid, tids);
    let state = user(cpu);
    state
        .classes
        .retain(|c| c.as_ref().is_none_or(|c| c.pid != pid));
    state.hooks.retain(|h| h.pid != pid);
    for m in state.menus.iter_mut() {
        if m.as_ref().is_some_and(|m| m.pid == pid) {
            *m = None;
        }
    }
    resource::process_ended(cpu, pid);
    // The cursor was the process's own and has gone with it: the arrow,
    // which the desktop sets as the mouse moves over it.
    let current = user(cpu).input.cursor_handle;
    if current != 0 && resource::icon(cpu, current).is_none() {
        let arrow = resource::system_cursor(cpu, resource::IDC_ARROW);
        cursor::set(cpu, arrow);
    }
    redraw_after(cpu);
}

/// A Win16 task, a thread of the Win16 process, ended: its windows go,
/// with its timers and its queue. The process's classes and menus stay
/// for its other tasks.
pub fn destroy_thread_windows(cpu: &mut Cpu, tid: u32) {
    if user(cpu).windows.is_empty() {
        return;
    }
    forget_windows(cpu, |w| w.tid == tid, &[tid]);
    redraw_after(cpu);
}

/// Drop the windows `gone` says, without the messages DestroyWindow
/// sends, and the timers and queues of the threads `tids`.
fn forget_windows(cpu: &mut Cpu, gone: impl Fn(&Window) -> bool, tids: &[u32]) {
    let state = user(cpu);
    let gone: Vec<u32> = (1..state.windows.len())
        .filter(|&i| state.windows[i].as_ref().is_some_and(&gone))
        .map(handle)
        .collect();
    for &hwnd in &gone {
        window::unlink(state, hwnd);
    }
    for &hwnd in &gone {
        if let Some(i) = slot(hwnd) {
            state.windows[i] = None;
        }
    }
    state.saved_bits.retain(|s| !gone.contains(&s.hwnd));
    state.timers.retain(|t| !tids.contains(&t.tid));
    state.queues.retain(|q| !tids.contains(&q.tid));
    let windows = &state.windows;
    let alive = |hwnd: u32| slot(hwnd).is_some_and(|i| windows.get(i).is_some_and(|w| w.is_some()));
    let had_foreground = state.foreground_window() != 0;
    let input = &mut state.input;
    for a in &mut input.queues {
        if !alive(a.active) {
            a.active = 0;
        }
        if !alive(a.focus) {
            a.focus = 0;
        }
    }
    input.queues.retain(|a| !tids.contains(&a.queue));
    if tids.contains(&input.foreground) {
        input.foreground = 0;
    }
    if !alive(input.capture) {
        input.capture = 0;
    }
    if !alive(state.caret.hwnd) {
        state.caret = input::Caret::default();
    }
    if state.clipboard.owner != 0 && !alive(state.clipboard.owner) {
        state.clipboard.owner = 0;
    }
    // The foreground window went with them: the next one comes there.
    if had_foreground && state.foreground_window() == 0 {
        input::foreground_gone(cpu);
    }
}

/// Windows went: the desktop left when none are left, redrawn when some
/// are.
fn redraw_after(cpu: &mut Cpu) {
    let state = user(cpu);
    let empty = state.windows.iter().skip(1).all(|w| w.is_none());
    if state.desktop {
        if empty {
            paint::leave_desktop(cpu);
        } else {
            paint::redraw_screen(cpu);
        }
    }
}

/// The functions. Append only (see `stubs`).
static APIS: &[Api] = &[
    // Windows and their classes.
    stdcall("RegisterClassA", 472, 1, class::register_class),
    stdcall("RegisterClassExA", 473, 1, class::register_class_ex),
    stdcall("UnregisterClassA", 614, 2, class::unregister_class),
    stdcall("GetClassInfoA", 226, 3, class::get_class_info),
    stdcall("GetClassInfoExA", 227, 3, class::get_class_info_ex),
    stdcall("GetClassLongA", 230, 2, class::get_class_long),
    stdcall("SetClassLongA", 515, 3, class::set_class_long),
    stdcall("GetClassWord", 234, 2, class::get_class_word),
    stdcall("SetClassWord", 517, 3, class::set_class_word),
    stdcall("GetClassNameA", 232, 3, class::get_class_name),
    stdcall("CreateWindowExA", 91, 12, window::create_window_ex),
    stdcall("DestroyWindow", 142, 1, window::destroy_window),
    stdcall("ShowWindow", 582, 2, window::show_window),
    stdcall("SetWindowPos", 569, 7, window::set_window_pos),
    stdcall("MoveWindow", 441, 6, window::move_window),
    stdcall("IsWindow", 391, 1, window::is_window),
    stdcall("IsWindowVisible", 394, 1, window::is_window_visible),
    stdcall("IsWindowEnabled", 392, 1, window::is_window_enabled),
    stdcall("EnableWindow", 181, 2, window::enable_window),
    stdcall("IsChild", 381, 2, window::is_child),
    stdcall("IsIconic", 388, 1, window::is_iconic),
    stdcall("IsZoomed", 395, 1, window::is_zoomed),
    stdcall("GetParent", 303, 1, window::get_parent),
    stdcall("SetParent", 544, 2, window::set_parent),
    stdcall("GetWindow", 330, 2, window::get_window),
    stdcall("GetTopWindow", 324, 1, window::get_top_window),
    stdcall("GetDesktopWindow", 249, 0, window::get_desktop_window),
    stdcall("GetWindowLongA", 334, 2, window::get_window_long),
    stdcall("SetWindowLongA", 566, 3, window::set_window_long),
    stdcall("GetWindowWord", 346, 2, window::get_window_word),
    stdcall("SetWindowWord", 573, 3, window::set_window_word),
    stdcall("GetWindowRect", 339, 2, window::get_window_rect),
    stdcall("GetClientRect", 235, 2, window::get_client_rect),
    stdcall("ClientToScreen", 61, 2, window::client_to_screen),
    stdcall("ScreenToClient", 494, 2, window::screen_to_client),
    stdcall("MapWindowPoints", 426, 4, window::map_window_points),
    stdcall("GetWindowTextA", 341, 3, window::get_window_text),
    stdcall("SetWindowTextA", 571, 2, window::set_window_text),
    stdcall(
        "GetWindowTextLengthA",
        342,
        1,
        window::get_window_text_length,
    ),
    stdcall("FindWindowA", 211, 2, window::find_window),
    stdcall("FindWindowExA", 212, 4, window::find_window_ex),
    stdcall("EnumWindows", 206, 2, window::enum_windows),
    stdcall("EnumChildWindows", 187, 3, window::enum_child_windows),
    stdcall("EnumThreadWindows", 203, 3, window::enum_thread_windows),
    stdcall(
        "GetWindowThreadProcessId",
        345,
        2,
        window::get_window_thread_process_id,
    ),
    stdcall("WindowFromPoint", 640, 2, window::window_from_point),
    stdcall(
        "ChildWindowFromPoint",
        58,
        3,
        window::child_window_from_point,
    ),
    stdcall(
        "ChildWindowFromPointEx",
        59,
        4,
        window::child_window_from_point_ex,
    ),
    stdcall("AdjustWindowRect", 2, 3, window::adjust_window_rect),
    stdcall("AdjustWindowRectEx", 3, 4, window::adjust_window_rect_ex),
    stdcall("GetWindowPlacement", 338, 2, window::get_window_placement),
    stdcall("SetWindowPlacement", 568, 2, window::set_window_placement),
    stdcall("BringWindowToTop", 14, 1, window::bring_window_to_top),
    stdcall("CloseWindow", 65, 1, window::close_window),
    stdcall("OpenIcon", 454, 1, window::open_icon),
    stdcall("GetPropA", 307, 2, window::get_prop),
    stdcall("SetPropA", 547, 3, window::set_prop),
    stdcall("RemovePropA", 490, 2, window::remove_prop),
    stdcall("EnumPropsA", 199, 2, window::enum_props),
    stdcall("GetLastActivePopup", 275, 1, window::get_last_active_popup),
    stdcall("GetNextWindow", 649, 2, window::get_window),
    stdcall("AnyPopup", 6, 0, window::any_popup),
    stdcall("ShowOwnedPopups", 580, 2, window::show_owned_popups),
    stdcall("BeginDeferWindowPos", 11, 1, window::begin_defer_window_pos),
    stdcall("DeferWindowPos", 135, 8, window::defer_window_pos),
    stdcall("EndDeferWindowPos", 182, 1, window::end_defer_window_pos),
    stdcall("ArrangeIconicWindows", 9, 1, nothing),
    stdcall("GetWindowContextHelpId", 331, 1, nothing),
    stdcall("SetWindowContextHelpId", 564, 2, succeed),
    stdcall("ShowWindowAsync", 583, 2, window::show_window),
    stdcall("FlashWindow", 215, 2, nothing),
    stdcall("GetAncestor", 221, 2, window::get_ancestor),
    stdcall(
        "RealChildWindowFromPoint",
        469,
        3,
        window::child_window_from_point,
    ),
    stdcall("SetWindowRgn", 570, 3, window::set_window_rgn),
    stdcall("GetWindowRgn", 340, 2, window::get_window_rgn),
    stdcall("TileWindows", 594, 5, nothing),
    stdcall("CascadeWindows", 26, 5, nothing),
    stdcall("GetClassNameW", 233, 3, not_implemented),
    stdcall("RegisterClassW", 475, 1, not_implemented),
    stdcall("RegisterClassExW", 474, 1, not_implemented),
    stdcall("UnregisterClassW", 615, 2, not_implemented),
    stdcall("GetClassInfoW", 229, 3, not_implemented),
    stdcall("GetClassInfoExW", 228, 3, not_implemented),
    stdcall("GetClassLongW", 231, 2, not_implemented),
    stdcall("SetClassLongW", 516, 3, not_implemented),
    stdcall("CreateWindowExW", 92, 12, not_implemented),
    stdcall("GetWindowLongW", 335, 2, not_implemented),
    stdcall("SetWindowLongW", 567, 3, not_implemented),
    stdcall("GetWindowTextW", 344, 3, not_implemented),
    stdcall("SetWindowTextW", 572, 2, not_implemented),
    stdcall("GetWindowTextLengthW", 343, 1, not_implemented),
    stdcall("FindWindowW", 214, 2, not_implemented),
    stdcall("FindWindowExW", 213, 4, not_implemented),
    stdcall("IsWindowUnicode", 393, 1, nothing),
    stdcall("GetPropW", 308, 2, not_implemented),
    stdcall("SetPropW", 548, 3, not_implemented),
    stdcall("RemovePropW", 491, 2, not_implemented),
    stdcall("InternalGetWindowText", 368, 3, window::get_window_text),
    // Messages.
    stdcall("SendMessageA", 503, 4, message::send_message),
    stdcall("PostMessageA", 463, 4, message::post_message),
    stdcall("PostThreadMessageA", 466, 4, message::post_thread_message),
    stdcall("PostQuitMessage", 465, 1, message::post_quit_message),
    stdcall("GetMessageA", 291, 4, message::get_message),
    stdcall("PeekMessageA", 460, 5, message::peek_message),
    stdcall("DispatchMessageA", 147, 1, message::dispatch_message),
    stdcall("TranslateMessage", 606, 1, input::translate_message),
    stdcall("WaitMessage", 635, 0, message::wait_message),
    stdcall("GetQueueStatus", 309, 1, message::get_queue_status),
    stdcall("GetInputState", 263, 0, message::get_input_state),
    stdcall("GetMessagePos", 293, 0, message::get_message_pos),
    stdcall("GetMessageTime", 294, 0, message::get_message_time),
    stdcall("GetMessageExtraInfo", 292, 0, nothing),
    stdcall("SetMessageExtraInfo", 542, 1, nothing),
    stdcall("InSendMessage", 359, 0, message::in_send_message),
    stdcall("ReplyMessage", 492, 1, message::reply_message),
    stdcall("SendMessageTimeoutA", 506, 7, message::send_message_timeout),
    stdcall("SendNotifyMessageA", 509, 4, message::send_notify_message),
    stdcall(
        "SendMessageCallbackA",
        504,
        6,
        message::send_message_callback,
    ),
    stdcall("CallWindowProcA", 23, 5, message::call_window_proc),
    stdcall("DefWindowProcA", 133, 4, defwnd::def_window_proc),
    stdcall(
        "RegisterWindowMessageA",
        485,
        1,
        message::register_window_message,
    ),
    stdcall(
        "MsgWaitForMultipleObjects",
        442,
        5,
        message::msg_wait_for_multiple_objects,
    ),
    stdcall(
        "MsgWaitForMultipleObjectsEx",
        443,
        5,
        message::msg_wait_for_multiple_objects_ex,
    ),
    stdcall("SetTimer", 559, 4, message::set_timer),
    stdcall("KillTimer", 396, 2, message::kill_timer),
    stdcall("SetSystemTimer", 650, 4, message::set_system_timer),
    stdcall("KillSystemTimer", 651, 2, message::kill_system_timer),
    stdcall("WaitForInputIdle", 634, 2, message::wait_for_input_idle),
    stdcall("BroadcastSystemMessage", 15, 5, nothing),
    stdcall("SetMessageQueue", 543, 1, succeed),
    stdcall("SendMessageW", 508, 4, not_implemented),
    stdcall("PostMessageW", 464, 4, not_implemented),
    stdcall("PostThreadMessageW", 467, 4, not_implemented),
    stdcall("GetMessageW", 295, 4, not_implemented),
    stdcall("PeekMessageW", 461, 5, not_implemented),
    stdcall("DispatchMessageW", 148, 1, not_implemented),
    stdcall("SendMessageTimeoutW", 507, 7, not_implemented),
    stdcall("SendNotifyMessageW", 510, 4, not_implemented),
    stdcall("SendMessageCallbackW", 505, 6, not_implemented),
    stdcall("CallWindowProcW", 24, 5, not_implemented),
    stdcall("DefWindowProcW", 134, 4, not_implemented),
    stdcall("RegisterWindowMessageW", 486, 1, not_implemented),
    // Hooks.
    stdcall("SetWindowsHookExA", 575, 4, hook::set_windows_hook_ex),
    stdcall("SetWindowsHookA", 574, 2, hook::set_windows_hook),
    stdcall("UnhookWindowsHookEx", 609, 1, hook::unhook_windows_hook_ex),
    stdcall("UnhookWindowsHook", 608, 2, hook::unhook_windows_hook),
    stdcall("CallNextHookEx", 22, 4, hook::call_next_hook_ex),
    stdcall("SetWindowsHookExW", 576, 4, not_implemented),
    stdcall("CallMsgFilterA", 20, 2, hook::call_msg_filter),
    // Input: focus, activation, keyboard, mouse.
    stdcall("SetFocus", 529, 1, input::set_focus),
    stdcall("GetFocus", 257, 0, input::get_focus),
    stdcall("SetActiveWindow", 511, 1, input::set_active_window),
    stdcall("GetActiveWindow", 219, 0, input::get_active_window),
    stdcall("SetForegroundWindow", 530, 1, input::set_foreground_window),
    stdcall("GetForegroundWindow", 258, 0, input::get_foreground_window),
    stdcall("SetCapture", 512, 1, input::set_capture),
    stdcall("ReleaseCapture", 487, 0, input::release_capture),
    stdcall("GetCapture", 223, 0, input::get_capture),
    stdcall("GetKeyState", 268, 1, input::get_key_state),
    stdcall("GetAsyncKeyState", 222, 1, input::get_async_key_state),
    stdcall("GetKeyboardState", 273, 1, input::get_keyboard_state),
    stdcall("SetKeyboardState", 532, 1, input::set_keyboard_state),
    stdcall("MapVirtualKeyA", 422, 2, input::map_virtual_key),
    stdcall("MapVirtualKeyExA", 423, 3, input::map_virtual_key),
    stdcall("VkKeyScanA", 626, 1, input::vk_key_scan),
    stdcall("GetKeyNameTextA", 266, 3, input::get_key_name_text),
    stdcall("ToAscii", 595, 5, input::to_ascii),
    stdcall("ToAsciiEx", 596, 6, input::to_ascii),
    stdcall("ToUnicode", 597, 6, not_implemented),
    stdcall("GetKeyboardLayout", 269, 1, input::get_keyboard_layout),
    stdcall(
        "GetKeyboardLayoutNameA",
        271,
        1,
        input::get_keyboard_layout_name,
    ),
    stdcall("GetKeyboardType", 274, 1, input::get_keyboard_type),
    stdcall("keybd_event", 643, 4, input::keybd_event),
    stdcall("mouse_event", 644, 5, input::mouse_event),
    stdcall("GetCursorPos", 246, 1, input::get_cursor_pos),
    stdcall("SetCursorPos", 521, 2, input::set_cursor_pos),
    stdcall("ClipCursor", 62, 1, input::clip_cursor),
    stdcall("GetClipCursor", 236, 1, input::get_clip_cursor),
    stdcall("SetCursor", 520, 1, input::set_cursor),
    stdcall("GetCursor", 244, 0, input::get_cursor),
    stdcall("ShowCursor", 579, 1, input::show_cursor),
    stdcall("GetDoubleClickTime", 256, 0, input::get_double_click_time),
    stdcall("SetDoubleClickTime", 528, 1, succeed),
    stdcall("SwapMouseButton", 585, 1, nothing),
    stdcall("DragDetect", 157, 3, nothing),
    stdcall("CreateCaret", 75, 4, input::create_caret),
    stdcall("DestroyCaret", 138, 0, input::destroy_caret),
    stdcall("ShowCaret", 578, 1, input::show_caret),
    stdcall("HideCaret", 350, 1, input::hide_caret),
    stdcall("SetCaretPos", 514, 2, input::set_caret_pos),
    stdcall("GetCaretPos", 225, 1, input::get_caret_pos),
    stdcall("GetCaretBlinkTime", 224, 0, input::get_caret_blink_time),
    stdcall("SetCaretBlinkTime", 513, 1, succeed),
    stdcall("EnableMenuItem", 179, 3, menu::enable_menu_item),
    stdcall("ActivateKeyboardLayout", 1, 2, input::get_keyboard_layout),
    stdcall("LoadKeyboardLayoutA", 409, 2, input::get_keyboard_layout),
    stdcall(
        "GetKeyboardLayoutList",
        270,
        2,
        input::get_keyboard_layout_list,
    ),
    stdcall("OemKeyScan", 445, 1, input::oem_key_scan),
    stdcall("VkKeyScanExA", 627, 2, input::vk_key_scan),
    stdcall("MapVirtualKeyW", 425, 2, not_implemented),
    stdcall("GetKeyNameTextW", 267, 3, not_implemented),
    stdcall("GetKeyboardLayoutNameW", 272, 1, not_implemented),
    stdcall("VkKeyScanW", 629, 1, not_implemented),
    stdcall("SendInput", 502, 3, input::send_input),
    stdcall("TrackMouseEvent", 599, 1, monitor::track_mouse_event),
    stdcall("RegisterHotKey", 480, 4, succeed),
    stdcall("UnregisterHotKey", 617, 2, succeed),
    // Painting.
    stdcall("GetDC", 247, 1, paint::get_dc),
    stdcall("GetDCEx", 248, 3, paint::get_dc_ex),
    stdcall("GetWindowDC", 332, 1, paint::get_window_dc),
    stdcall("ReleaseDC", 488, 2, paint::release_dc),
    stdcall("BeginPaint", 12, 2, paint::begin_paint),
    stdcall("EndPaint", 185, 2, paint::end_paint),
    stdcall("InvalidateRect", 370, 3, paint::invalidate_rect),
    stdcall("InvalidateRgn", 371, 3, paint::invalidate_rgn),
    stdcall("ValidateRect", 624, 2, paint::validate_rect),
    stdcall("ValidateRgn", 625, 2, paint::validate_rgn),
    stdcall("UpdateWindow", 618, 1, paint::update_window),
    stdcall("RedrawWindow", 471, 4, paint::redraw_window),
    stdcall("GetUpdateRect", 325, 3, paint::get_update_rect),
    stdcall("GetUpdateRgn", 326, 3, paint::get_update_rgn),
    stdcall("ExcludeUpdateRgn", 208, 2, paint::exclude_update_rgn),
    stdcall("ScrollWindow", 496, 5, paint::scroll_window),
    stdcall("ScrollWindowEx", 497, 8, paint::scroll_window_ex),
    stdcall("ScrollDC", 495, 7, paint::scroll_dc),
    stdcall("LockWindowUpdate", 418, 1, succeed),
    stdcall("WindowFromDC", 639, 1, paint::window_from_dc),
    stdcall("FillRect", 210, 3, paint::fill_rect),
    stdcall("FrameRect", 217, 3, paint::frame_rect),
    stdcall("InvertRect", 372, 2, paint::invert_rect),
    stdcall("DrawFocusRect", 164, 2, paint::draw_focus_rect),
    stdcall("DrawEdge", 163, 4, paint::draw_edge),
    stdcall("DrawFrameControl", 166, 4, paint::draw_frame_control),
    stdcall("DrawIcon", 167, 4, paint::draw_icon),
    stdcall("DrawIconEx", 168, 9, paint::draw_icon_ex),
    stdcall("GetSysColor", 316, 1, sysparams::get_sys_color),
    stdcall("GetSysColorBrush", 317, 1, sysparams::get_sys_color_brush),
    stdcall("SetSysColors", 555, 3, sysparams::set_sys_colors),
    stdcall("GetSystemMetrics", 319, 1, sysparams::get_system_metrics),
    stdcall(
        "SystemParametersInfoA",
        589,
        4,
        sysparams::system_parameters_info,
    ),
    stdcall("SystemParametersInfoW", 590, 4, not_implemented),
    stdcall("DrawTextA", 173, 5, text::draw_text),
    stdcall("DrawTextExA", 174, 6, text::draw_text_ex),
    stdcall("DrawTextW", 176, 5, not_implemented),
    stdcall("DrawTextExW", 175, 6, not_implemented),
    stdcall("TabbedTextOutA", 591, 8, text::tabbed_text_out),
    stdcall("GetTabbedTextExtentA", 320, 5, text::get_tabbed_text_extent),
    stdcall("GrayStringA", 347, 9, text::gray_string),
    stdcall("DrawStateA", 171, 10, text::draw_state),
    cdecl("wsprintfA", 645, 0, text::wsprintf),
    stdcall("wvsprintfA", 647, 3, text::wvsprintf),
    cdecl("wsprintfW", 646, 0, text::wsprintf_w),
    stdcall("wvsprintfW", 648, 3, text::wvsprintf_w),
    stdcall("CharUpperA", 50, 1, text::char_upper),
    stdcall("CharLowerA", 34, 1, text::char_lower),
    stdcall("CharUpperBuffA", 51, 2, text::char_upper_buff),
    stdcall("CharLowerBuffA", 35, 2, text::char_lower_buff),
    stdcall("CharNextA", 38, 1, text::char_next),
    stdcall("CharPrevA", 42, 2, text::char_prev),
    stdcall("IsCharAlphaA", 373, 1, text::is_char_alpha),
    stdcall("IsCharAlphaNumericA", 374, 1, text::is_char_alpha_numeric),
    stdcall("IsCharUpperA", 379, 1, text::is_char_upper),
    stdcall("IsCharLowerA", 377, 1, text::is_char_lower),
    stdcall("CharToOemA", 46, 2, text::char_to_oem),
    stdcall("OemToCharA", 446, 2, text::oem_to_char),
    stdcall("CharToOemBuffA", 47, 3, text::char_to_oem_buff),
    stdcall("OemToCharBuffA", 447, 3, text::oem_to_char_buff),
    stdcall("CharNextExA", 39, 3, text::char_next_ex),
    stdcall("CharPrevExA", 43, 4, text::char_prev_ex),
    stdcall("MessageBeep", 428, 1, succeed),
    stdcall("MessageBoxA", 429, 4, dialog::message_box),
    stdcall("MessageBoxExA", 430, 5, dialog::message_box),
    stdcall("MessageBoxIndirectA", 432, 1, dialog::message_box_indirect),
    // Rectangles.
    stdcall("SetRect", 549, 5, rect::set_rect),
    stdcall("SetRectEmpty", 550, 1, rect::set_rect_empty),
    stdcall("CopyRect", 71, 2, rect::copy_rect),
    stdcall("IsRectEmpty", 390, 1, rect::is_rect_empty),
    stdcall("EqualRect", 207, 2, rect::equal_rect),
    stdcall("IntersectRect", 369, 3, rect::intersect_rect),
    stdcall("UnionRect", 610, 3, rect::union_rect),
    stdcall("SubtractRect", 584, 3, rect::subtract_rect),
    stdcall("OffsetRect", 450, 3, rect::offset_rect),
    stdcall("InflateRect", 361, 3, rect::inflate_rect),
    stdcall("PtInRect", 468, 3, rect::pt_in_rect),
    // Dialogs.
    stdcall("CreateDialogParamA", 81, 5, dialog::create_dialog_param),
    stdcall(
        "CreateDialogIndirectParamA",
        79,
        5,
        dialog::create_dialog_indirect_param,
    ),
    stdcall("DialogBoxParamA", 145, 5, dialog::dialog_box_param),
    stdcall(
        "DialogBoxIndirectParamA",
        143,
        5,
        dialog::dialog_box_indirect_param,
    ),
    stdcall("EndDialog", 183, 2, dialog::end_dialog),
    stdcall("DefDlgProcA", 127, 4, dialog::def_dlg_proc),
    stdcall("IsDialogMessageA", 384, 2, dialog::is_dialog_message),
    stdcall("GetDlgItem", 252, 2, dialog::get_dlg_item),
    stdcall("GetDlgCtrlID", 251, 1, dialog::get_dlg_ctrl_id),
    stdcall("SetDlgItemTextA", 526, 3, dialog::set_dlg_item_text),
    stdcall("GetDlgItemTextA", 254, 4, dialog::get_dlg_item_text),
    stdcall("SetDlgItemInt", 525, 4, dialog::set_dlg_item_int),
    stdcall("GetDlgItemInt", 253, 4, dialog::get_dlg_item_int),
    stdcall("SendDlgItemMessageA", 498, 5, dialog::send_dlg_item_message),
    stdcall("CheckDlgButton", 54, 3, dialog::check_dlg_button),
    stdcall("IsDlgButtonChecked", 386, 2, dialog::is_dlg_button_checked),
    stdcall("CheckRadioButton", 57, 4, dialog::check_radio_button),
    stdcall("GetNextDlgTabItem", 300, 3, dialog::get_next_dlg_tab_item),
    stdcall(
        "GetNextDlgGroupItem",
        299,
        3,
        dialog::get_next_dlg_group_item,
    ),
    stdcall("MapDialogRect", 421, 2, dialog::map_dialog_rect),
    stdcall("GetDialogBaseUnits", 250, 0, dialog::get_dialog_base_units),
    stdcall("CreateDialogIndirectParamW", 80, 5, not_implemented),
    stdcall("CreateDialogParamW", 82, 5, not_implemented),
    stdcall("DialogBoxParamW", 146, 5, not_implemented),
    stdcall("DialogBoxIndirectParamW", 144, 5, not_implemented),
    stdcall("SendDlgItemMessageW", 499, 5, not_implemented),
    stdcall("DefDlgProcW", 128, 4, not_implemented),
    stdcall("IsDialogMessage", 383, 2, dialog::is_dialog_message),
    // Resources.
    stdcall("LoadStringA", 415, 4, resource::load_string),
    stdcall("LoadStringW", 416, 4, resource::load_string_w),
    stdcall("LoadIconA", 405, 2, resource::load_icon),
    stdcall("LoadCursorA", 401, 2, resource::load_cursor),
    stdcall("LoadBitmapA", 399, 2, resource::load_bitmap),
    stdcall("LoadImageA", 407, 6, resource::load_image),
    stdcall("LoadMenuA", 411, 2, menu::load_menu),
    stdcall("LoadMenuIndirectA", 412, 1, menu::load_menu_indirect),
    stdcall("LoadAcceleratorsA", 397, 2, resource::load_accelerators),
    stdcall(
        "CreateAcceleratorTableA",
        73,
        2,
        resource::create_accelerator_table,
    ),
    stdcall(
        "DestroyAcceleratorTable",
        137,
        1,
        resource::destroy_accelerator_table,
    ),
    stdcall(
        "CopyAcceleratorTableA",
        67,
        3,
        resource::copy_accelerator_table,
    ),
    stdcall(
        "TranslateAcceleratorA",
        603,
        3,
        resource::translate_accelerator,
    ),
    stdcall("CreateIcon", 83, 7, resource::create_icon),
    stdcall("CreateIconIndirect", 86, 1, resource::create_icon_indirect),
    stdcall("CreateCursor", 76, 7, resource::create_cursor),
    stdcall("DestroyIcon", 140, 1, resource::destroy_icon),
    stdcall("DestroyCursor", 139, 1, resource::destroy_icon),
    stdcall("CopyIcon", 69, 1, resource::copy_icon),
    stdcall("GetIconInfo", 261, 2, resource::get_icon_info),
    stdcall("LoadIconW", 406, 2, resource::load_icon),
    stdcall("LoadCursorW", 404, 2, resource::load_cursor),
    stdcall("CopyImage", 70, 5, resource::copy_image),
    stdcall("LoadCursorFromFileA", 402, 1, nothing),
    stdcall(
        "CreateIconFromResource",
        84,
        4,
        resource::create_icon_from_resource,
    ),
    stdcall(
        "CreateIconFromResourceEx",
        85,
        7,
        resource::create_icon_from_resource_ex,
    ),
    stdcall(
        "LookupIconIdFromDirectory",
        419,
        2,
        resource::lookup_icon_id_from_directory,
    ),
    stdcall(
        "LookupIconIdFromDirectoryEx",
        420,
        5,
        resource::lookup_icon_id_from_directory_ex,
    ),
    stdcall("LoadImageW", 408, 6, not_implemented),
    stdcall("LoadBitmapW", 400, 2, not_implemented),
    stdcall("LoadMenuW", 414, 2, not_implemented),
    stdcall("LoadAcceleratorsW", 398, 2, not_implemented),
    // Menus.
    stdcall("CreateMenu", 89, 0, menu::create_menu),
    stdcall("CreatePopupMenu", 90, 0, menu::create_popup_menu),
    stdcall("DestroyMenu", 141, 1, menu::destroy_menu),
    stdcall("IsMenu", 389, 1, menu::is_menu),
    stdcall("GetMenu", 277, 1, menu::get_menu),
    stdcall("SetMenu", 535, 2, menu::set_menu),
    stdcall("AppendMenuA", 7, 4, menu::append_menu),
    stdcall("InsertMenuA", 364, 5, menu::insert_menu),
    stdcall("ModifyMenuA", 436, 5, menu::modify_menu),
    stdcall("RemoveMenu", 489, 3, menu::remove_menu),
    stdcall("DeleteMenu", 136, 3, menu::delete_menu),
    stdcall("GetMenuItemCount", 283, 1, menu::get_menu_item_count),
    stdcall("GetMenuItemID", 284, 2, menu::get_menu_item_id),
    stdcall("GetMenuState", 288, 3, menu::get_menu_state),
    stdcall("GetMenuStringA", 289, 5, menu::get_menu_string),
    stdcall("GetSubMenu", 315, 2, menu::get_sub_menu),
    stdcall("CheckMenuItem", 55, 3, menu::check_menu_item),
    stdcall("CheckMenuRadioItem", 56, 5, menu::check_menu_radio_item),
    stdcall("DrawMenuBar", 169, 1, menu::draw_menu_bar),
    stdcall("GetSystemMenu", 318, 2, menu::get_system_menu),
    stdcall("TrackPopupMenu", 600, 7, menu::track_popup_menu),
    stdcall("TrackPopupMenuEx", 601, 6, menu::track_popup_menu_ex),
    stdcall("GetMenuItemInfoA", 285, 4, menu::get_menu_item_info),
    stdcall("SetMenuItemInfoA", 540, 4, menu::set_menu_item_info),
    stdcall("InsertMenuItemA", 365, 4, menu::insert_menu_item),
    stdcall("GetMenuItemRect", 287, 4, menu::get_menu_item_rect),
    stdcall("HiliteMenuItem", 351, 4, succeed),
    stdcall(
        "GetMenuCheckMarkDimensions",
        279,
        0,
        menu::get_menu_check_mark_dimensions,
    ),
    stdcall("SetMenuItemBitmaps", 539, 5, succeed),
    stdcall("GetMenuDefaultItem", 281, 3, menu::get_menu_default_item),
    stdcall("SetMenuDefaultItem", 537, 3, menu::set_menu_default_item),
    stdcall("EndMenu", 184, 0, succeed),
    stdcall("MenuItemFromPoint", 427, 4, menu::menu_item_from_point),
    stdcall("GetMenuContextHelpId", 280, 1, nothing),
    stdcall("SetMenuContextHelpId", 536, 2, succeed),
    stdcall("AppendMenuW", 8, 4, not_implemented),
    stdcall("InsertMenuW", 367, 5, not_implemented),
    stdcall("ModifyMenuW", 437, 5, not_implemented),
    stdcall("GetMenuStringW", 290, 5, not_implemented),
    stdcall("GetMenuItemInfoW", 286, 4, not_implemented),
    stdcall("SetMenuItemInfoW", 541, 4, not_implemented),
    stdcall("InsertMenuItemW", 366, 4, not_implemented),
    stdcall("GetMenuInfo", 282, 2, not_implemented),
    stdcall("SetMenuInfo", 538, 2, not_implemented),
    // Scroll bars.
    stdcall(
        "SetScrollInfo",
        551,
        4,
        controls::scrollbar::set_scroll_info,
    ),
    stdcall(
        "GetScrollInfo",
        311,
        3,
        controls::scrollbar::get_scroll_info,
    ),
    stdcall("SetScrollPos", 552, 4, controls::scrollbar::set_scroll_pos),
    stdcall("GetScrollPos", 312, 2, controls::scrollbar::get_scroll_pos),
    stdcall(
        "SetScrollRange",
        553,
        5,
        controls::scrollbar::set_scroll_range,
    ),
    stdcall(
        "GetScrollRange",
        313,
        4,
        controls::scrollbar::get_scroll_range,
    ),
    stdcall(
        "ShowScrollBar",
        581,
        3,
        controls::scrollbar::show_scroll_bar,
    ),
    stdcall(
        "EnableScrollBar",
        180,
        3,
        controls::scrollbar::enable_scroll_bar,
    ),
    // The system classes' window procedures.
    stdcall("ButtonWndProc", 0, 4, controls::button_proc),
    stdcall("StaticWndProc", 0, 4, controls::static_proc),
    stdcall("EditWndProc", 0, 4, controls::edit_proc),
    stdcall("ListBoxWndProc", 0, 4, controls::listbox_proc),
    stdcall("ComboBoxWndProc", 0, 4, controls::combobox_proc),
    stdcall("ScrollBarWndProc", 0, 4, controls::scrollbar_proc),
    stdcall("MDIClientWndProc", 0, 4, controls::mdiclient_proc),
    stdcall("DesktopWndProc", 0, 4, defwnd::desktop_proc),
    stdcall("MenuWndProc", 0, 4, menu::popup_proc),
    stdcall("IconTitleWndProc", 0, 4, defwnd::def_window_proc),
    stdcall("ComboLBoxWndProc", 0, 4, controls::listbox_proc),
    stdcall(
        "MciVideoWndProc",
        0,
        4,
        crate::win32::winmm::mci::video::window_proc,
    ),
    // The clipboard.
    stdcall("OpenClipboard", 451, 1, input::open_clipboard),
    stdcall("CloseClipboard", 63, 0, input::close_clipboard),
    stdcall("EmptyClipboard", 178, 0, input::empty_clipboard),
    stdcall("GetClipboardData", 237, 1, input::get_clipboard_data),
    stdcall("SetClipboardData", 518, 2, input::set_clipboard_data),
    stdcall(
        "IsClipboardFormatAvailable",
        382,
        1,
        input::is_clipboard_format_available,
    ),
    stdcall("GetClipboardOwner", 240, 0, input::get_clipboard_owner),
    stdcall("GetClipboardViewer", 242, 0, nothing),
    stdcall("SetClipboardViewer", 519, 1, nothing),
    stdcall("ChangeClipboardChain", 27, 2, nothing),
    stdcall(
        "CountClipboardFormats",
        72,
        0,
        input::count_clipboard_formats,
    ),
    stdcall(
        "EnumClipboardFormats",
        188,
        1,
        input::enum_clipboard_formats,
    ),
    stdcall(
        "RegisterClipboardFormatA",
        476,
        1,
        input::register_clipboard_format,
    ),
    stdcall(
        "GetClipboardFormatNameA",
        238,
        3,
        input::get_clipboard_format_name,
    ),
    stdcall(
        "GetOpenClipboardWindow",
        302,
        0,
        input::get_open_clipboard_window,
    ),
    // MDI.
    stdcall("DefFrameProcA", 129, 5, controls::def_frame_proc),
    stdcall("DefMDIChildProcA", 131, 4, controls::def_mdi_child_proc),
    stdcall("TranslateMDISysAccel", 605, 2, nothing),
    stdcall("CreateMDIWindowA", 87, 10, controls::create_mdi_window),
    stdcall("CreateMDIWindowW", 88, 10, not_implemented),
    stdcall("DefFrameProcW", 130, 5, not_implemented),
    stdcall("DefMDIChildProcW", 132, 4, not_implemented),
    // The rest.
    stdcall("GetShellWindow", 314, 0, nothing),
    stdcall(
        "EnumDisplaySettingsA",
        195,
        3,
        sysparams::enum_display_settings,
    ),
    stdcall(
        "ChangeDisplaySettingsA",
        28,
        2,
        sysparams::change_display_settings,
    ),
    stdcall("ExitWindowsEx", 209, 2, session::exit_windows_ex),
    stdcall("GetGuiResources", 260, 2, nothing),
    stdcall(
        "GetProcessWindowStation",
        306,
        0,
        sysparams::get_process_window_station,
    ),
    stdcall("GetThreadDesktop", 322, 1, sysparams::get_thread_desktop),
    stdcall("GetUserObjectInformationA", 327, 5, not_implemented),
    stdcall("WinHelpA", 636, 4, winhelp::win_help),
    stdcall("GetSystemDefaultLangID", 652, 0, nothing),
    stdcall("EnumDisplayDevicesA", 192, 4, monitor::enum_display_devices),
    // No monitor functions (430 to 433): they came with Windows 98, and
    // programs and Wine's tests look them up to tell Windows 95 apart.
    stdcall("SetWindowsHookW", 577, 2, not_implemented),
    stdcall("PaintDesktop", 459, 1, paint::paint_desktop),
    stdcall("GetDlgItemTextW", 255, 4, not_implemented),
    stdcall("SetDlgItemTextW", 527, 3, not_implemented),
    stdcall("MessageBoxW", 434, 4, dialog::message_box_w),
    stdcall("MessageBoxExW", 431, 5, dialog::message_box_w),
    stdcall("CharUpperW", 53, 1, text::char_upper_w),
    stdcall("CharLowerW", 37, 1, text::char_lower_w),
    stdcall("CharNextW", 41, 1, text::char_next_w),
    stdcall("CharPrevW", 45, 2, text::char_prev_w),
    stdcall("CharUpperBuffW", 52, 2, text::char_upper_buff_w),
    stdcall("CharLowerBuffW", 36, 2, text::char_lower_buff_w),
    stdcall("IsCharAlphaW", 376, 1, text::is_char_alpha_w),
    stdcall("IsCharAlphaNumericW", 375, 1, text::is_char_alpha_numeric_w),
    stdcall("IsCharUpperW", 380, 1, text::is_char_upper_w),
    stdcall("IsCharLowerW", 378, 1, text::is_char_lower_w),
    stdcall("SetLastErrorEx", 533, 2, input::set_last_error_ex),
    stdcall(
        "GetWindowModuleFileNameA",
        336,
        3,
        window::get_window_module_file_name,
    ),
    stdcall("CallMsgFilter", 19, 2, hook::call_msg_filter),
    stdcall("CallHooks", 0, 4, hook::call_hooks),
    stdcall("DefSysCommand", 0, 3, defwnd::def_sys_command),
    stdcall("ActivateOtherWindow", 0, 1, input::activate_other_window),
    stdcall("IsHungAppWindow", 653, 1, nothing),
    stdcall("GetAltTabInfo", 220, 5, not_implemented),
    stdcall("GetComboBoxInfo", 243, 2, not_implemented),
    stdcall("GetTitleBarInfo", 323, 2, not_implemented),
    stdcall("GetWindowInfo", 333, 2, window::get_window_info),
    stdcall("GetGUIThreadInfo", 259, 2, not_implemented),
    stdcall("EnumPropsExA", 200, 3, window::enum_props_ex),
    stdcall("GetKBCodePage", 265, 0, input::get_kb_code_page),
    stdcall("DrawCaption", 160, 4, paint::draw_caption),
    stdcall("DrawAnimatedRects", 159, 4, succeed),
    stdcall("ReuseDDElParam", 493, 5, ddeml::reuse_dde_lparam),
    stdcall("UnpackDDElParam", 613, 4, ddeml::unpack_dde_lparam),
    stdcall("PackDDElParam", 458, 3, ddeml::pack_dde_lparam),
    stdcall("FreeDDElParam", 218, 2, ddeml::free_dde_lparam),
    stdcall("ImpersonateDdeClientWindow", 358, 2, not_implemented),
    stdcall("DlgDirListA", 149, 5, nothing),
    stdcall("DlgDirSelectExA", 155, 4, nothing),
    stdcall("DlgDirListComboBoxA", 150, 5, nothing),
    stdcall("DlgDirSelectComboBoxExA", 153, 4, nothing),
    stdcall("MonitorFromWindow", 440, 2, monitor::monitor_from_window),
    stdcall("MonitorFromPoint", 438, 3, monitor::monitor_from_point),
    stdcall("MonitorFromRect", 439, 2, monitor::monitor_from_rect),
    stdcall("GetMonitorInfoA", 296, 2, monitor::get_monitor_info_a),
    stdcall("GetMonitorInfoW", 297, 2, monitor::get_monitor_info_w),
    stdcall("EnumDisplayMonitors", 194, 4, monitor::enum_display_monitors),
    stdcall("AnimateWindow", 5, 3, monitor::animate_window),
    stdcall("GetCursorInfo", 245, 1, monitor::get_cursor_info),
    // DDEML (ddeml.rs), numbered from 600 on.
    stdcall("DdeInitializeA", 113, 4, ddeml::dde_initialize),
    stdcall("DdeInitializeW", 114, 4, ddeml::dde_initialize),
    stdcall("DdeUninitialize", 126, 1, ddeml::dde_uninitialize),
    stdcall("DdeGetLastError", 111, 1, ddeml::dde_get_last_error),
    stdcall(
        "DdeCreateStringHandleA",
        103,
        3,
        ddeml::dde_create_string_handle,
    ),
    stdcall(
        "DdeCreateStringHandleW",
        104,
        3,
        ddeml::dde_create_string_handle_w,
    ),
    stdcall("DdeFreeStringHandle", 109, 2, ddeml::dde_free_string_handle),
    stdcall("DdeKeepStringHandle", 115, 2, ddeml::dde_keep_string_handle),
    stdcall("DdeQueryStringA", 120, 5, ddeml::dde_query_string),
    stdcall("DdeCmpStringHandles", 99, 2, ddeml::dde_cmp_string_handles),
    stdcall("DdeCreateDataHandle", 102, 7, ddeml::dde_create_data_handle),
    stdcall("DdeAddData", 97, 4, ddeml::dde_add_data),
    stdcall("DdeGetData", 110, 4, ddeml::dde_get_data),
    stdcall("DdeAccessData", 96, 2, ddeml::dde_access_data),
    stdcall("DdeUnaccessData", 125, 1, ddeml::dde_unaccess_data),
    stdcall("DdeFreeDataHandle", 108, 1, ddeml::dde_free_data_handle),
    stdcall("DdeNameService", 116, 4, ddeml::dde_name_service),
    stdcall("DdeConnect", 100, 4, ddeml::dde_connect),
    stdcall("DdeDisconnect", 105, 1, ddeml::dde_disconnect),
    stdcall(
        "DdeClientTransaction",
        98,
        8,
        ddeml::dde_client_transaction,
    ),
    stdcall("DdeAbandonTransaction", 95, 3, succeed),
    stdcall("DdeEnableCallback", 107, 3, succeed),
    stdcall("DdePostAdvise", 117, 3, succeed),
    stdcall("DdeSetUserHandle", 124, 3, succeed),
    // The help viewer's window procedures (winhelp).
    stdcall("WinHelpWndProc", 0, 4, winhelp::frame_proc),
    stdcall("WinHelpTopicWndProc", 0, 4, winhelp::topic_proc),
    // Program Manager's window, the raw DDE messages' server.
    stdcall("ProgmanWndProc", 0, 4, progman::wnd_proc),
    // The activation of an input queue's windows in its own thread.
    stdcall("ActivateInQueue", 0, 3, input::activate_in_queue),
    stdcall("DeactivateQueue", 0, 1, input::deactivate_queue),
    stdcall("ActivateByClick", 0, 1, input::activate_by_click),
    // Windows 98's does nothing: there is no debug USER32 to tell.
    stdcall("SetDebugErrorLevel", 522, 1, |_, _| Some(0)),
    stdcall("SetSystemCursor", 557, 2, resource::set_system_cursor),
    // Notepad, C:\WINDOWS\NOTEPAD.EXE: its start and its window's
    // procedure (notepad.rs).
    stdcall("NotepadMain", 0, 0, notepad::main),
    stdcall("NotepadWndProc", 0, 4, notepad::wnd_proc),
];

crate::state_fields!(State {
    windows,
    next_window,
    classes,
    last_atom,
    queues,
    hooks,
    timers,
    menus,
    icons,
    accels,
    input,
    colors,
    color_brushes,
    desktop,
    last_sent,
    clipboard,
    caret,
    params,
    registered,
    in_hooks,
    last_hook,
    saved_bits
} skip {
    help
});
