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
pub mod paint;
pub mod progman;
pub mod rect;
pub mod resource;
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
    fn new_window(&mut self, window: Window) -> Option<u32> {
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

/// The system starts: the desktop window, the system's classes, and the
/// arrow as the cursor, which Windows shows from the start until a
/// program sets another.
pub fn init(cpu: &mut Cpu) {
    if !user(cpu).windows.is_empty() {
        return;
    }
    sysparams::init_colors(cpu);
    class::register_system(cpu);
    window::create_desktop(cpu);
    let arrow = resource::system_cursor(cpu, resource::IDC_ARROW);
    cursor::set(cpu, arrow);
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
    stdcall("RegisterClassA", 1, 1, class::register_class),
    stdcall("RegisterClassExA", 2, 1, class::register_class_ex),
    stdcall("UnregisterClassA", 3, 2, class::unregister_class),
    stdcall("GetClassInfoA", 4, 3, class::get_class_info),
    stdcall("GetClassInfoExA", 5, 3, class::get_class_info_ex),
    stdcall("GetClassLongA", 6, 2, class::get_class_long),
    stdcall("SetClassLongA", 7, 3, class::set_class_long),
    stdcall("GetClassWord", 8, 2, class::get_class_word),
    stdcall("SetClassWord", 9, 3, class::set_class_word),
    stdcall("GetClassNameA", 10, 3, class::get_class_name),
    stdcall("CreateWindowExA", 11, 12, window::create_window_ex),
    stdcall("DestroyWindow", 12, 1, window::destroy_window),
    stdcall("ShowWindow", 13, 2, window::show_window),
    stdcall("SetWindowPos", 14, 7, window::set_window_pos),
    stdcall("MoveWindow", 15, 6, window::move_window),
    stdcall("IsWindow", 16, 1, window::is_window),
    stdcall("IsWindowVisible", 17, 1, window::is_window_visible),
    stdcall("IsWindowEnabled", 18, 1, window::is_window_enabled),
    stdcall("EnableWindow", 19, 2, window::enable_window),
    stdcall("IsChild", 20, 2, window::is_child),
    stdcall("IsIconic", 21, 1, window::is_iconic),
    stdcall("IsZoomed", 22, 1, window::is_zoomed),
    stdcall("GetParent", 23, 1, window::get_parent),
    stdcall("SetParent", 24, 2, window::set_parent),
    stdcall("GetWindow", 25, 2, window::get_window),
    stdcall("GetTopWindow", 26, 1, window::get_top_window),
    stdcall("GetDesktopWindow", 27, 0, window::get_desktop_window),
    stdcall("GetWindowLongA", 28, 2, window::get_window_long),
    stdcall("SetWindowLongA", 29, 3, window::set_window_long),
    stdcall("GetWindowWord", 30, 2, window::get_window_word),
    stdcall("SetWindowWord", 31, 3, window::set_window_word),
    stdcall("GetWindowRect", 32, 2, window::get_window_rect),
    stdcall("GetClientRect", 33, 2, window::get_client_rect),
    stdcall("ClientToScreen", 34, 2, window::client_to_screen),
    stdcall("ScreenToClient", 35, 2, window::screen_to_client),
    stdcall("MapWindowPoints", 36, 4, window::map_window_points),
    stdcall("GetWindowTextA", 37, 3, window::get_window_text),
    stdcall("SetWindowTextA", 38, 2, window::set_window_text),
    stdcall(
        "GetWindowTextLengthA",
        39,
        1,
        window::get_window_text_length,
    ),
    stdcall("FindWindowA", 40, 2, window::find_window),
    stdcall("FindWindowExA", 41, 4, window::find_window_ex),
    stdcall("EnumWindows", 42, 2, window::enum_windows),
    stdcall("EnumChildWindows", 43, 3, window::enum_child_windows),
    stdcall("EnumThreadWindows", 44, 3, window::enum_thread_windows),
    stdcall(
        "GetWindowThreadProcessId",
        45,
        2,
        window::get_window_thread_process_id,
    ),
    stdcall("WindowFromPoint", 46, 2, window::window_from_point),
    stdcall(
        "ChildWindowFromPoint",
        47,
        3,
        window::child_window_from_point,
    ),
    stdcall(
        "ChildWindowFromPointEx",
        48,
        4,
        window::child_window_from_point_ex,
    ),
    stdcall("AdjustWindowRect", 49, 3, window::adjust_window_rect),
    stdcall("AdjustWindowRectEx", 50, 4, window::adjust_window_rect_ex),
    stdcall("GetWindowPlacement", 51, 2, window::get_window_placement),
    stdcall("SetWindowPlacement", 52, 2, window::set_window_placement),
    stdcall("BringWindowToTop", 53, 1, window::bring_window_to_top),
    stdcall("CloseWindow", 54, 1, window::close_window),
    stdcall("OpenIcon", 55, 1, window::open_icon),
    stdcall("GetPropA", 56, 2, window::get_prop),
    stdcall("SetPropA", 57, 3, window::set_prop),
    stdcall("RemovePropA", 58, 2, window::remove_prop),
    stdcall("EnumPropsA", 59, 2, window::enum_props),
    stdcall("GetLastActivePopup", 60, 1, window::get_last_active_popup),
    stdcall("GetNextWindow", 61, 2, window::get_window),
    stdcall("AnyPopup", 62, 0, window::any_popup),
    stdcall("ShowOwnedPopups", 63, 2, window::show_owned_popups),
    stdcall("BeginDeferWindowPos", 64, 1, window::begin_defer_window_pos),
    stdcall("DeferWindowPos", 65, 8, window::defer_window_pos),
    stdcall("EndDeferWindowPos", 66, 1, window::end_defer_window_pos),
    stdcall("ArrangeIconicWindows", 67, 1, nothing),
    stdcall("GetWindowContextHelpId", 68, 1, nothing),
    stdcall("SetWindowContextHelpId", 69, 2, succeed),
    stdcall("ShowWindowAsync", 70, 2, window::show_window),
    stdcall("FlashWindow", 71, 2, nothing),
    stdcall("GetAncestor", 72, 2, window::get_ancestor),
    stdcall(
        "RealChildWindowFromPoint",
        73,
        3,
        window::child_window_from_point,
    ),
    stdcall("SetWindowRgn", 74, 3, window::set_window_rgn),
    stdcall("GetWindowRgn", 75, 2, window::get_window_rgn),
    stdcall("TileWindows", 76, 5, nothing),
    stdcall("CascadeWindows", 77, 5, nothing),
    stdcall("GetClassNameW", 78, 3, not_implemented),
    stdcall("RegisterClassW", 79, 1, not_implemented),
    stdcall("RegisterClassExW", 80, 1, not_implemented),
    stdcall("UnregisterClassW", 81, 2, not_implemented),
    stdcall("GetClassInfoW", 82, 3, not_implemented),
    stdcall("GetClassInfoExW", 83, 3, not_implemented),
    stdcall("GetClassLongW", 84, 2, not_implemented),
    stdcall("SetClassLongW", 85, 3, not_implemented),
    stdcall("CreateWindowExW", 86, 12, not_implemented),
    stdcall("GetWindowLongW", 87, 2, not_implemented),
    stdcall("SetWindowLongW", 88, 3, not_implemented),
    stdcall("GetWindowTextW", 89, 3, not_implemented),
    stdcall("SetWindowTextW", 90, 2, not_implemented),
    stdcall("GetWindowTextLengthW", 91, 1, not_implemented),
    stdcall("FindWindowW", 92, 2, not_implemented),
    stdcall("FindWindowExW", 93, 4, not_implemented),
    stdcall("IsWindowUnicode", 94, 1, nothing),
    stdcall("GetPropW", 95, 2, not_implemented),
    stdcall("SetPropW", 96, 3, not_implemented),
    stdcall("RemovePropW", 97, 2, not_implemented),
    stdcall("InternalGetWindowText", 98, 3, window::get_window_text),
    // Messages.
    stdcall("SendMessageA", 99, 4, message::send_message),
    stdcall("PostMessageA", 100, 4, message::post_message),
    stdcall("PostThreadMessageA", 101, 4, message::post_thread_message),
    stdcall("PostQuitMessage", 102, 1, message::post_quit_message),
    stdcall("GetMessageA", 103, 4, message::get_message),
    stdcall("PeekMessageA", 104, 5, message::peek_message),
    stdcall("DispatchMessageA", 105, 1, message::dispatch_message),
    stdcall("TranslateMessage", 106, 1, input::translate_message),
    stdcall("WaitMessage", 107, 0, message::wait_message),
    stdcall("GetQueueStatus", 108, 1, message::get_queue_status),
    stdcall("GetInputState", 109, 0, message::get_input_state),
    stdcall("GetMessagePos", 110, 0, message::get_message_pos),
    stdcall("GetMessageTime", 111, 0, message::get_message_time),
    stdcall("GetMessageExtraInfo", 112, 0, nothing),
    stdcall("SetMessageExtraInfo", 113, 1, nothing),
    stdcall("InSendMessage", 114, 0, message::in_send_message),
    stdcall("ReplyMessage", 115, 1, message::reply_message),
    stdcall("SendMessageTimeoutA", 116, 7, message::send_message_timeout),
    stdcall("SendNotifyMessageA", 117, 4, message::send_notify_message),
    stdcall(
        "SendMessageCallbackA",
        118,
        6,
        message::send_message_callback,
    ),
    stdcall("CallWindowProcA", 119, 5, message::call_window_proc),
    stdcall("DefWindowProcA", 120, 4, defwnd::def_window_proc),
    stdcall(
        "RegisterWindowMessageA",
        121,
        1,
        message::register_window_message,
    ),
    stdcall(
        "MsgWaitForMultipleObjects",
        122,
        5,
        message::msg_wait_for_multiple_objects,
    ),
    stdcall(
        "MsgWaitForMultipleObjectsEx",
        123,
        5,
        message::msg_wait_for_multiple_objects_ex,
    ),
    stdcall("SetTimer", 124, 4, message::set_timer),
    stdcall("KillTimer", 125, 2, message::kill_timer),
    stdcall("SetSystemTimer", 126, 4, message::set_system_timer),
    stdcall("KillSystemTimer", 127, 2, message::kill_system_timer),
    stdcall("WaitForInputIdle", 128, 2, message::wait_for_input_idle),
    stdcall("BroadcastSystemMessage", 129, 5, nothing),
    stdcall("SetMessageQueue", 130, 1, succeed),
    stdcall("SendMessageW", 131, 4, not_implemented),
    stdcall("PostMessageW", 132, 4, not_implemented),
    stdcall("PostThreadMessageW", 133, 4, not_implemented),
    stdcall("GetMessageW", 134, 4, not_implemented),
    stdcall("PeekMessageW", 135, 5, not_implemented),
    stdcall("DispatchMessageW", 136, 1, not_implemented),
    stdcall("SendMessageTimeoutW", 137, 7, not_implemented),
    stdcall("SendNotifyMessageW", 138, 4, not_implemented),
    stdcall("SendMessageCallbackW", 139, 6, not_implemented),
    stdcall("CallWindowProcW", 140, 5, not_implemented),
    stdcall("DefWindowProcW", 141, 4, not_implemented),
    stdcall("RegisterWindowMessageW", 142, 1, not_implemented),
    // Hooks.
    stdcall("SetWindowsHookExA", 143, 4, hook::set_windows_hook_ex),
    stdcall("SetWindowsHookA", 144, 2, hook::set_windows_hook),
    stdcall("UnhookWindowsHookEx", 145, 1, hook::unhook_windows_hook_ex),
    stdcall("UnhookWindowsHook", 146, 2, hook::unhook_windows_hook),
    stdcall("CallNextHookEx", 147, 4, hook::call_next_hook_ex),
    stdcall("SetWindowsHookExW", 148, 4, not_implemented),
    stdcall("CallMsgFilterA", 149, 2, hook::call_msg_filter),
    // Input: focus, activation, keyboard, mouse.
    stdcall("SetFocus", 150, 1, input::set_focus),
    stdcall("GetFocus", 151, 0, input::get_focus),
    stdcall("SetActiveWindow", 152, 1, input::set_active_window),
    stdcall("GetActiveWindow", 153, 0, input::get_active_window),
    stdcall("SetForegroundWindow", 154, 1, input::set_foreground_window),
    stdcall("GetForegroundWindow", 155, 0, input::get_foreground_window),
    stdcall("SetCapture", 156, 1, input::set_capture),
    stdcall("ReleaseCapture", 157, 0, input::release_capture),
    stdcall("GetCapture", 158, 0, input::get_capture),
    stdcall("GetKeyState", 159, 1, input::get_key_state),
    stdcall("GetAsyncKeyState", 160, 1, input::get_async_key_state),
    stdcall("GetKeyboardState", 161, 1, input::get_keyboard_state),
    stdcall("SetKeyboardState", 162, 1, input::set_keyboard_state),
    stdcall("MapVirtualKeyA", 163, 2, input::map_virtual_key),
    stdcall("MapVirtualKeyExA", 164, 3, input::map_virtual_key),
    stdcall("VkKeyScanA", 165, 1, input::vk_key_scan),
    stdcall("GetKeyNameTextA", 166, 3, input::get_key_name_text),
    stdcall("ToAscii", 167, 5, input::to_ascii),
    stdcall("ToAsciiEx", 168, 6, input::to_ascii),
    stdcall("ToUnicode", 169, 6, not_implemented),
    stdcall("GetKeyboardLayout", 170, 1, input::get_keyboard_layout),
    stdcall(
        "GetKeyboardLayoutNameA",
        171,
        1,
        input::get_keyboard_layout_name,
    ),
    stdcall("GetKeyboardType", 172, 1, input::get_keyboard_type),
    stdcall("keybd_event", 173, 4, input::keybd_event),
    stdcall("mouse_event", 174, 5, input::mouse_event),
    stdcall("GetCursorPos", 175, 1, input::get_cursor_pos),
    stdcall("SetCursorPos", 176, 2, input::set_cursor_pos),
    stdcall("ClipCursor", 177, 1, input::clip_cursor),
    stdcall("GetClipCursor", 178, 1, input::get_clip_cursor),
    stdcall("SetCursor", 179, 1, input::set_cursor),
    stdcall("GetCursor", 180, 0, input::get_cursor),
    stdcall("ShowCursor", 181, 1, input::show_cursor),
    stdcall("GetDoubleClickTime", 182, 0, input::get_double_click_time),
    stdcall("SetDoubleClickTime", 183, 1, succeed),
    stdcall("SwapMouseButton", 184, 1, nothing),
    stdcall("DragDetect", 185, 3, nothing),
    stdcall("CreateCaret", 186, 4, input::create_caret),
    stdcall("DestroyCaret", 187, 0, input::destroy_caret),
    stdcall("ShowCaret", 188, 1, input::show_caret),
    stdcall("HideCaret", 189, 1, input::hide_caret),
    stdcall("SetCaretPos", 190, 2, input::set_caret_pos),
    stdcall("GetCaretPos", 191, 1, input::get_caret_pos),
    stdcall("GetCaretBlinkTime", 192, 0, input::get_caret_blink_time),
    stdcall("SetCaretBlinkTime", 193, 1, succeed),
    stdcall("EnableMenuItem", 194, 3, menu::enable_menu_item),
    stdcall("ActivateKeyboardLayout", 195, 2, input::get_keyboard_layout),
    stdcall("LoadKeyboardLayoutA", 196, 2, input::get_keyboard_layout),
    stdcall(
        "GetKeyboardLayoutList",
        197,
        2,
        input::get_keyboard_layout_list,
    ),
    stdcall("OemKeyScan", 198, 1, input::oem_key_scan),
    stdcall("VkKeyScanExA", 199, 2, input::vk_key_scan),
    stdcall("MapVirtualKeyW", 200, 2, not_implemented),
    stdcall("GetKeyNameTextW", 201, 3, not_implemented),
    stdcall("GetKeyboardLayoutNameW", 202, 1, not_implemented),
    stdcall("VkKeyScanW", 203, 1, not_implemented),
    stdcall("SendInput", 204, 3, input::send_input),
    stdcall("TrackMouseEvent", 205, 1, monitor::track_mouse_event),
    stdcall("RegisterHotKey", 206, 4, succeed),
    stdcall("UnregisterHotKey", 207, 2, succeed),
    // Painting.
    stdcall("GetDC", 208, 1, paint::get_dc),
    stdcall("GetDCEx", 209, 3, paint::get_dc_ex),
    stdcall("GetWindowDC", 210, 1, paint::get_window_dc),
    stdcall("ReleaseDC", 211, 2, paint::release_dc),
    stdcall("BeginPaint", 212, 2, paint::begin_paint),
    stdcall("EndPaint", 213, 2, paint::end_paint),
    stdcall("InvalidateRect", 214, 3, paint::invalidate_rect),
    stdcall("InvalidateRgn", 215, 3, paint::invalidate_rgn),
    stdcall("ValidateRect", 216, 2, paint::validate_rect),
    stdcall("ValidateRgn", 217, 2, paint::validate_rgn),
    stdcall("UpdateWindow", 218, 1, paint::update_window),
    stdcall("RedrawWindow", 219, 4, paint::redraw_window),
    stdcall("GetUpdateRect", 220, 3, paint::get_update_rect),
    stdcall("GetUpdateRgn", 221, 3, paint::get_update_rgn),
    stdcall("ExcludeUpdateRgn", 222, 2, paint::exclude_update_rgn),
    stdcall("ScrollWindow", 223, 5, paint::scroll_window),
    stdcall("ScrollWindowEx", 224, 8, paint::scroll_window_ex),
    stdcall("ScrollDC", 225, 7, paint::scroll_dc),
    stdcall("LockWindowUpdate", 226, 1, succeed),
    stdcall("WindowFromDC", 227, 1, paint::window_from_dc),
    stdcall("FillRect", 228, 3, paint::fill_rect),
    stdcall("FrameRect", 229, 3, paint::frame_rect),
    stdcall("InvertRect", 230, 2, paint::invert_rect),
    stdcall("DrawFocusRect", 231, 2, paint::draw_focus_rect),
    stdcall("DrawEdge", 232, 4, paint::draw_edge),
    stdcall("DrawFrameControl", 233, 4, paint::draw_frame_control),
    stdcall("DrawIcon", 234, 4, paint::draw_icon),
    stdcall("DrawIconEx", 235, 9, paint::draw_icon_ex),
    stdcall("GetSysColor", 236, 1, sysparams::get_sys_color),
    stdcall("GetSysColorBrush", 237, 1, sysparams::get_sys_color_brush),
    stdcall("SetSysColors", 238, 3, sysparams::set_sys_colors),
    stdcall("GetSystemMetrics", 239, 1, sysparams::get_system_metrics),
    stdcall(
        "SystemParametersInfoA",
        240,
        4,
        sysparams::system_parameters_info,
    ),
    stdcall("SystemParametersInfoW", 241, 4, not_implemented),
    stdcall("DrawTextA", 242, 5, text::draw_text),
    stdcall("DrawTextExA", 243, 6, text::draw_text_ex),
    stdcall("DrawTextW", 244, 5, not_implemented),
    stdcall("DrawTextExW", 245, 6, not_implemented),
    stdcall("TabbedTextOutA", 246, 8, text::tabbed_text_out),
    stdcall("GetTabbedTextExtentA", 247, 5, text::get_tabbed_text_extent),
    stdcall("GrayStringA", 248, 9, text::gray_string),
    stdcall("DrawStateA", 249, 10, text::draw_state),
    cdecl("wsprintfA", 250, 0, text::wsprintf),
    stdcall("wvsprintfA", 251, 3, text::wvsprintf),
    cdecl("wsprintfW", 252, 0, text::wsprintf_w),
    stdcall("wvsprintfW", 253, 3, text::wvsprintf_w),
    stdcall("CharUpperA", 254, 1, text::char_upper),
    stdcall("CharLowerA", 255, 1, text::char_lower),
    stdcall("CharUpperBuffA", 256, 2, text::char_upper_buff),
    stdcall("CharLowerBuffA", 257, 2, text::char_lower_buff),
    stdcall("CharNextA", 258, 1, text::char_next),
    stdcall("CharPrevA", 259, 2, text::char_prev),
    stdcall("IsCharAlphaA", 260, 1, text::is_char_alpha),
    stdcall("IsCharAlphaNumericA", 261, 1, text::is_char_alpha_numeric),
    stdcall("IsCharUpperA", 262, 1, text::is_char_upper),
    stdcall("IsCharLowerA", 263, 1, text::is_char_lower),
    stdcall("CharToOemA", 264, 2, text::char_to_oem),
    stdcall("OemToCharA", 265, 2, text::oem_to_char),
    stdcall("CharToOemBuffA", 266, 3, text::char_to_oem_buff),
    stdcall("OemToCharBuffA", 267, 3, text::oem_to_char_buff),
    stdcall("CharNextExA", 268, 3, text::char_next_ex),
    stdcall("CharPrevExA", 269, 4, text::char_prev_ex),
    stdcall("MessageBeep", 270, 1, succeed),
    stdcall("MessageBoxA", 271, 4, dialog::message_box),
    stdcall("MessageBoxExA", 272, 5, dialog::message_box),
    stdcall("MessageBoxIndirectA", 273, 1, dialog::message_box_indirect),
    // Rectangles.
    stdcall("SetRect", 274, 5, rect::set_rect),
    stdcall("SetRectEmpty", 275, 1, rect::set_rect_empty),
    stdcall("CopyRect", 276, 2, rect::copy_rect),
    stdcall("IsRectEmpty", 277, 1, rect::is_rect_empty),
    stdcall("EqualRect", 278, 2, rect::equal_rect),
    stdcall("IntersectRect", 279, 3, rect::intersect_rect),
    stdcall("UnionRect", 280, 3, rect::union_rect),
    stdcall("SubtractRect", 281, 3, rect::subtract_rect),
    stdcall("OffsetRect", 282, 3, rect::offset_rect),
    stdcall("InflateRect", 283, 3, rect::inflate_rect),
    stdcall("PtInRect", 284, 3, rect::pt_in_rect),
    // Dialogs.
    stdcall("CreateDialogParamA", 285, 5, dialog::create_dialog_param),
    stdcall(
        "CreateDialogIndirectParamA",
        286,
        5,
        dialog::create_dialog_indirect_param,
    ),
    stdcall("DialogBoxParamA", 287, 5, dialog::dialog_box_param),
    stdcall(
        "DialogBoxIndirectParamA",
        288,
        5,
        dialog::dialog_box_indirect_param,
    ),
    stdcall("EndDialog", 289, 2, dialog::end_dialog),
    stdcall("DefDlgProcA", 290, 4, dialog::def_dlg_proc),
    stdcall("IsDialogMessageA", 291, 2, dialog::is_dialog_message),
    stdcall("GetDlgItem", 292, 2, dialog::get_dlg_item),
    stdcall("GetDlgCtrlID", 293, 1, dialog::get_dlg_ctrl_id),
    stdcall("SetDlgItemTextA", 294, 3, dialog::set_dlg_item_text),
    stdcall("GetDlgItemTextA", 295, 4, dialog::get_dlg_item_text),
    stdcall("SetDlgItemInt", 296, 4, dialog::set_dlg_item_int),
    stdcall("GetDlgItemInt", 297, 4, dialog::get_dlg_item_int),
    stdcall("SendDlgItemMessageA", 298, 5, dialog::send_dlg_item_message),
    stdcall("CheckDlgButton", 299, 3, dialog::check_dlg_button),
    stdcall("IsDlgButtonChecked", 300, 2, dialog::is_dlg_button_checked),
    stdcall("CheckRadioButton", 301, 4, dialog::check_radio_button),
    stdcall("GetNextDlgTabItem", 302, 3, dialog::get_next_dlg_tab_item),
    stdcall(
        "GetNextDlgGroupItem",
        303,
        3,
        dialog::get_next_dlg_group_item,
    ),
    stdcall("MapDialogRect", 304, 2, dialog::map_dialog_rect),
    stdcall("GetDialogBaseUnits", 305, 0, dialog::get_dialog_base_units),
    stdcall("CreateDialogIndirectParamW", 306, 5, not_implemented),
    stdcall("CreateDialogParamW", 307, 5, not_implemented),
    stdcall("DialogBoxParamW", 308, 5, not_implemented),
    stdcall("DialogBoxIndirectParamW", 309, 5, not_implemented),
    stdcall("SendDlgItemMessageW", 310, 5, not_implemented),
    stdcall("DefDlgProcW", 311, 4, not_implemented),
    stdcall("IsDialogMessage", 312, 2, dialog::is_dialog_message),
    // Resources.
    stdcall("LoadStringA", 313, 4, resource::load_string),
    stdcall("LoadStringW", 314, 4, resource::load_string_w),
    stdcall("LoadIconA", 315, 2, resource::load_icon),
    stdcall("LoadCursorA", 316, 2, resource::load_cursor),
    stdcall("LoadBitmapA", 317, 2, resource::load_bitmap),
    stdcall("LoadImageA", 318, 6, resource::load_image),
    stdcall("LoadMenuA", 319, 2, menu::load_menu),
    stdcall("LoadMenuIndirectA", 320, 1, menu::load_menu_indirect),
    stdcall("LoadAcceleratorsA", 321, 2, resource::load_accelerators),
    stdcall(
        "CreateAcceleratorTableA",
        322,
        2,
        resource::create_accelerator_table,
    ),
    stdcall(
        "DestroyAcceleratorTable",
        323,
        1,
        resource::destroy_accelerator_table,
    ),
    stdcall(
        "CopyAcceleratorTableA",
        324,
        3,
        resource::copy_accelerator_table,
    ),
    stdcall(
        "TranslateAcceleratorA",
        325,
        3,
        resource::translate_accelerator,
    ),
    stdcall("CreateIcon", 326, 7, resource::create_icon),
    stdcall("CreateIconIndirect", 327, 1, resource::create_icon_indirect),
    stdcall("CreateCursor", 328, 7, resource::create_cursor),
    stdcall("DestroyIcon", 329, 1, resource::destroy_icon),
    stdcall("DestroyCursor", 330, 1, resource::destroy_icon),
    stdcall("CopyIcon", 331, 1, resource::copy_icon),
    stdcall("GetIconInfo", 332, 2, resource::get_icon_info),
    stdcall("LoadIconW", 333, 2, resource::load_icon),
    stdcall("LoadCursorW", 334, 2, resource::load_cursor),
    stdcall("CopyImage", 335, 5, resource::copy_image),
    stdcall("LoadCursorFromFileA", 336, 1, nothing),
    stdcall(
        "CreateIconFromResource",
        337,
        4,
        resource::create_icon_from_resource,
    ),
    stdcall(
        "CreateIconFromResourceEx",
        338,
        7,
        resource::create_icon_from_resource_ex,
    ),
    stdcall(
        "LookupIconIdFromDirectory",
        339,
        2,
        resource::lookup_icon_id_from_directory,
    ),
    stdcall(
        "LookupIconIdFromDirectoryEx",
        340,
        5,
        resource::lookup_icon_id_from_directory_ex,
    ),
    stdcall("LoadImageW", 341, 6, not_implemented),
    stdcall("LoadBitmapW", 342, 2, not_implemented),
    stdcall("LoadMenuW", 343, 2, not_implemented),
    stdcall("LoadAcceleratorsW", 344, 2, not_implemented),
    // Menus.
    stdcall("CreateMenu", 345, 0, menu::create_menu),
    stdcall("CreatePopupMenu", 346, 0, menu::create_popup_menu),
    stdcall("DestroyMenu", 347, 1, menu::destroy_menu),
    stdcall("IsMenu", 348, 1, menu::is_menu),
    stdcall("GetMenu", 349, 1, menu::get_menu),
    stdcall("SetMenu", 350, 2, menu::set_menu),
    stdcall("AppendMenuA", 351, 4, menu::append_menu),
    stdcall("InsertMenuA", 352, 5, menu::insert_menu),
    stdcall("ModifyMenuA", 353, 5, menu::modify_menu),
    stdcall("RemoveMenu", 354, 3, menu::remove_menu),
    stdcall("DeleteMenu", 355, 3, menu::delete_menu),
    stdcall("GetMenuItemCount", 356, 1, menu::get_menu_item_count),
    stdcall("GetMenuItemID", 357, 2, menu::get_menu_item_id),
    stdcall("GetMenuState", 358, 3, menu::get_menu_state),
    stdcall("GetMenuStringA", 359, 5, menu::get_menu_string),
    stdcall("GetSubMenu", 360, 2, menu::get_sub_menu),
    stdcall("CheckMenuItem", 361, 3, menu::check_menu_item),
    stdcall("CheckMenuRadioItem", 362, 5, menu::check_menu_radio_item),
    stdcall("DrawMenuBar", 363, 1, menu::draw_menu_bar),
    stdcall("GetSystemMenu", 364, 2, menu::get_system_menu),
    stdcall("TrackPopupMenu", 365, 7, menu::track_popup_menu),
    stdcall("TrackPopupMenuEx", 366, 6, menu::track_popup_menu_ex),
    stdcall("GetMenuItemInfoA", 367, 4, menu::get_menu_item_info),
    stdcall("SetMenuItemInfoA", 368, 4, menu::set_menu_item_info),
    stdcall("InsertMenuItemA", 369, 4, menu::insert_menu_item),
    stdcall("GetMenuItemRect", 370, 4, menu::get_menu_item_rect),
    stdcall("HiliteMenuItem", 371, 4, succeed),
    stdcall(
        "GetMenuCheckMarkDimensions",
        372,
        0,
        menu::get_menu_check_mark_dimensions,
    ),
    stdcall("SetMenuItemBitmaps", 373, 5, succeed),
    stdcall("GetMenuDefaultItem", 374, 3, menu::get_menu_default_item),
    stdcall("SetMenuDefaultItem", 375, 3, menu::set_menu_default_item),
    stdcall("EndMenu", 376, 0, succeed),
    stdcall("MenuItemFromPoint", 377, 4, menu::menu_item_from_point),
    stdcall("GetMenuContextHelpId", 378, 1, nothing),
    stdcall("SetMenuContextHelpId", 379, 2, succeed),
    stdcall("AppendMenuW", 380, 4, not_implemented),
    stdcall("InsertMenuW", 381, 5, not_implemented),
    stdcall("ModifyMenuW", 382, 5, not_implemented),
    stdcall("GetMenuStringW", 383, 5, not_implemented),
    stdcall("GetMenuItemInfoW", 384, 4, not_implemented),
    stdcall("SetMenuItemInfoW", 385, 4, not_implemented),
    stdcall("InsertMenuItemW", 386, 4, not_implemented),
    stdcall("GetMenuInfo", 387, 2, not_implemented),
    stdcall("SetMenuInfo", 388, 2, not_implemented),
    // Scroll bars.
    stdcall(
        "SetScrollInfo",
        389,
        4,
        controls::scrollbar::set_scroll_info,
    ),
    stdcall(
        "GetScrollInfo",
        390,
        3,
        controls::scrollbar::get_scroll_info,
    ),
    stdcall("SetScrollPos", 391, 4, controls::scrollbar::set_scroll_pos),
    stdcall("GetScrollPos", 392, 2, controls::scrollbar::get_scroll_pos),
    stdcall(
        "SetScrollRange",
        393,
        5,
        controls::scrollbar::set_scroll_range,
    ),
    stdcall(
        "GetScrollRange",
        394,
        4,
        controls::scrollbar::get_scroll_range,
    ),
    stdcall(
        "ShowScrollBar",
        395,
        3,
        controls::scrollbar::show_scroll_bar,
    ),
    stdcall(
        "EnableScrollBar",
        396,
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
    // The clipboard.
    stdcall("OpenClipboard", 397, 1, input::open_clipboard),
    stdcall("CloseClipboard", 398, 0, input::close_clipboard),
    stdcall("EmptyClipboard", 399, 0, input::empty_clipboard),
    stdcall("GetClipboardData", 400, 1, input::get_clipboard_data),
    stdcall("SetClipboardData", 401, 2, input::set_clipboard_data),
    stdcall(
        "IsClipboardFormatAvailable",
        402,
        1,
        input::is_clipboard_format_available,
    ),
    stdcall("GetClipboardOwner", 403, 0, input::get_clipboard_owner),
    stdcall("GetClipboardViewer", 404, 0, nothing),
    stdcall("SetClipboardViewer", 405, 1, nothing),
    stdcall("ChangeClipboardChain", 406, 2, nothing),
    stdcall(
        "CountClipboardFormats",
        407,
        0,
        input::count_clipboard_formats,
    ),
    stdcall(
        "EnumClipboardFormats",
        408,
        1,
        input::enum_clipboard_formats,
    ),
    stdcall(
        "RegisterClipboardFormatA",
        409,
        1,
        input::register_clipboard_format,
    ),
    stdcall(
        "GetClipboardFormatNameA",
        410,
        3,
        input::get_clipboard_format_name,
    ),
    stdcall(
        "GetOpenClipboardWindow",
        411,
        0,
        input::get_open_clipboard_window,
    ),
    // MDI.
    stdcall("DefFrameProcA", 412, 5, controls::def_frame_proc),
    stdcall("DefMDIChildProcA", 413, 4, controls::def_mdi_child_proc),
    stdcall("TranslateMDISysAccel", 414, 2, nothing),
    stdcall("CreateMDIWindowA", 415, 10, controls::create_mdi_window),
    stdcall("CreateMDIWindowW", 416, 10, not_implemented),
    stdcall("DefFrameProcW", 417, 5, not_implemented),
    stdcall("DefMDIChildProcW", 418, 4, not_implemented),
    // The rest.
    stdcall("GetShellWindow", 419, 0, nothing),
    stdcall(
        "EnumDisplaySettingsA",
        420,
        3,
        sysparams::enum_display_settings,
    ),
    stdcall(
        "ChangeDisplaySettingsA",
        421,
        2,
        sysparams::change_display_settings,
    ),
    stdcall("ExitWindowsEx", 422, 2, nothing),
    stdcall("GetGuiResources", 423, 2, nothing),
    stdcall(
        "GetProcessWindowStation",
        424,
        0,
        sysparams::get_process_window_station,
    ),
    stdcall("GetThreadDesktop", 425, 1, sysparams::get_thread_desktop),
    stdcall("GetUserObjectInformationA", 426, 5, not_implemented),
    stdcall("WinHelpA", 427, 4, winhelp::win_help),
    stdcall("GetSystemDefaultLangID", 428, 0, nothing),
    stdcall("EnumDisplayDevicesA", 429, 4, monitor::enum_display_devices),
    // No monitor functions (430 to 433): they came with Windows 98, and
    // programs and Wine's tests look them up to tell Windows 95 apart.
    stdcall("SetWindowsHookW", 434, 2, not_implemented),
    stdcall("PaintDesktop", 435, 1, paint::paint_desktop),
    stdcall("GetDlgItemTextW", 436, 4, not_implemented),
    stdcall("SetDlgItemTextW", 437, 3, not_implemented),
    stdcall("MessageBoxW", 438, 4, dialog::message_box_w),
    stdcall("MessageBoxExW", 439, 5, dialog::message_box_w),
    stdcall("CharUpperW", 440, 1, text::char_upper_w),
    stdcall("CharLowerW", 441, 1, text::char_lower_w),
    stdcall("CharNextW", 442, 1, text::char_next_w),
    stdcall("CharPrevW", 443, 2, text::char_prev_w),
    stdcall("CharUpperBuffW", 444, 2, text::char_upper_buff_w),
    stdcall("CharLowerBuffW", 445, 2, text::char_lower_buff_w),
    stdcall("IsCharAlphaW", 446, 1, text::is_char_alpha_w),
    stdcall("IsCharAlphaNumericW", 447, 1, text::is_char_alpha_numeric_w),
    stdcall("IsCharUpperW", 448, 1, text::is_char_upper_w),
    stdcall("IsCharLowerW", 449, 1, text::is_char_lower_w),
    stdcall("SetLastErrorEx", 450, 2, input::set_last_error_ex),
    stdcall(
        "GetWindowModuleFileNameA",
        451,
        3,
        window::get_window_module_file_name,
    ),
    stdcall("CallMsgFilter", 452, 2, hook::call_msg_filter),
    stdcall("CallHooks", 0, 4, hook::call_hooks),
    stdcall("DefSysCommand", 0, 3, defwnd::def_sys_command),
    stdcall("ActivateOtherWindow", 0, 1, input::activate_other_window),
    stdcall("IsHungAppWindow", 453, 1, nothing),
    stdcall("GetAltTabInfo", 454, 5, not_implemented),
    stdcall("GetComboBoxInfo", 455, 2, not_implemented),
    stdcall("GetTitleBarInfo", 456, 2, not_implemented),
    stdcall("GetWindowInfo", 457, 2, window::get_window_info),
    stdcall("GetGUIThreadInfo", 458, 2, not_implemented),
    stdcall("EnumPropsExA", 459, 3, window::enum_props_ex),
    stdcall("GetKBCodePage", 460, 0, input::get_kb_code_page),
    stdcall("DrawCaption", 461, 4, paint::draw_caption),
    stdcall("DrawAnimatedRects", 462, 4, succeed),
    stdcall("ReuseDDElParam", 463, 5, ddeml::reuse_dde_lparam),
    stdcall("UnpackDDElParam", 464, 4, ddeml::unpack_dde_lparam),
    stdcall("PackDDElParam", 465, 3, ddeml::pack_dde_lparam),
    stdcall("FreeDDElParam", 466, 2, ddeml::free_dde_lparam),
    stdcall("ImpersonateDdeClientWindow", 467, 2, not_implemented),
    stdcall("DlgDirListA", 468, 5, nothing),
    stdcall("DlgDirSelectExA", 469, 4, nothing),
    stdcall("DlgDirListComboBoxA", 470, 5, nothing),
    stdcall("DlgDirSelectComboBoxExA", 471, 4, nothing),
    stdcall("MonitorFromWindow", 472, 2, monitor::monitor_from_window),
    stdcall("MonitorFromPoint", 473, 3, monitor::monitor_from_point),
    stdcall("MonitorFromRect", 474, 2, monitor::monitor_from_rect),
    stdcall("GetMonitorInfoA", 475, 2, monitor::get_monitor_info_a),
    stdcall("GetMonitorInfoW", 476, 2, monitor::get_monitor_info_w),
    stdcall("EnumDisplayMonitors", 477, 4, monitor::enum_display_monitors),
    stdcall("AnimateWindow", 478, 3, monitor::animate_window),
    stdcall("GetCursorInfo", 479, 1, monitor::get_cursor_info),
    // DDEML (ddeml.rs), numbered from 600 on.
    stdcall("DdeInitializeA", 600, 4, ddeml::dde_initialize),
    stdcall("DdeInitializeW", 601, 4, ddeml::dde_initialize),
    stdcall("DdeUninitialize", 602, 1, ddeml::dde_uninitialize),
    stdcall("DdeGetLastError", 603, 1, ddeml::dde_get_last_error),
    stdcall(
        "DdeCreateStringHandleA",
        604,
        3,
        ddeml::dde_create_string_handle,
    ),
    stdcall(
        "DdeCreateStringHandleW",
        605,
        3,
        ddeml::dde_create_string_handle_w,
    ),
    stdcall("DdeFreeStringHandle", 606, 2, ddeml::dde_free_string_handle),
    stdcall("DdeKeepStringHandle", 607, 2, ddeml::dde_keep_string_handle),
    stdcall("DdeQueryStringA", 608, 5, ddeml::dde_query_string),
    stdcall("DdeCmpStringHandles", 609, 2, ddeml::dde_cmp_string_handles),
    stdcall("DdeCreateDataHandle", 610, 7, ddeml::dde_create_data_handle),
    stdcall("DdeAddData", 611, 4, ddeml::dde_add_data),
    stdcall("DdeGetData", 612, 4, ddeml::dde_get_data),
    stdcall("DdeAccessData", 613, 2, ddeml::dde_access_data),
    stdcall("DdeUnaccessData", 614, 1, ddeml::dde_unaccess_data),
    stdcall("DdeFreeDataHandle", 615, 1, ddeml::dde_free_data_handle),
    stdcall("DdeNameService", 616, 4, ddeml::dde_name_service),
    stdcall("DdeConnect", 617, 4, ddeml::dde_connect),
    stdcall("DdeDisconnect", 618, 1, ddeml::dde_disconnect),
    stdcall(
        "DdeClientTransaction",
        619,
        8,
        ddeml::dde_client_transaction,
    ),
    stdcall("DdeAbandonTransaction", 620, 3, succeed),
    stdcall("DdeEnableCallback", 621, 3, succeed),
    stdcall("DdePostAdvise", 622, 3, succeed),
    stdcall("DdeSetUserHandle", 623, 3, succeed),
    // The help viewer's window procedures (winhelp).
    stdcall("WinHelpWndProc", 0, 4, winhelp::frame_proc),
    stdcall("WinHelpTopicWndProc", 0, 4, winhelp::topic_proc),
    // Program Manager's window, the raw DDE messages' server.
    stdcall("ProgmanWndProc", 0, 4, progman::wnd_proc),
    // The activation of an input queue's windows in its own thread.
    stdcall("ActivateInQueue", 0, 3, input::activate_in_queue),
    stdcall("DeactivateQueue", 0, 1, input::deactivate_queue),
    stdcall("ActivateByClick", 0, 1, input::activate_by_click),
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
    last_hook
} skip {
    help
});
