//! Input: the keyboard and the mouse of the emulated PC, as messages in
//! the queues of the threads whose windows they are for; the focus, the
//! active window and the capture; the caret, the cursor and the
//! clipboard.
//!
//! The devices are read when time passes (`poll`, from thread::wake)
//! and when the program looks at its input, while the desktop is on the
//! screen. A key that goes down or up is a WM_KEYDOWN or WM_KEYUP for
//! the focus (WM_SYSKEYDOWN with Alt held, or for the active window when
//! nothing has the focus); a keystroke the BIOS got for a key that was
//! already down is a repeat. The mouse's moves and buttons are messages
//! for the window under the cursor, or for the window that has the
//! capture.
//!
//! The focus and the active window are the system's, not each thread's:
//! GetFocus and GetActiveWindow give them to the thread they belong to.

use super::message::{
    self, Msg, QS_KEY, QS_MOUSEBUTTON, QS_MOUSEMOVE, WM_ACTIVATE, WM_ACTIVATEAPP, WM_CHAR,
    WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_MOUSEMOVE, WM_NCACTIVATE,
    WM_NCLBUTTONDOWN, WM_NCMOUSEMOVE, WM_SETFOCUS, WM_SYSCHAR, WM_SYSKEYDOWN, WM_SYSKEYUP,
};
use super::sysparams::{metric, screen_size};
use super::window::{
    self, HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WS_CAPTION, WS_CHILD, WS_DISABLED,
    WS_EX_CLIENTEDGE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_HSCROLL, WS_MAXIMIZEBOX, WS_MINIMIZE,
    WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU, WS_VISIBLE, WS_VSCROLL,
};
use super::{
    DESKTOP, ERROR_INVALID_PARAMETER, ERROR_INVALID_WINDOW_HANDLE, FALSE, State, TRUE, address_of,
    current_tid, read_words, set_error, string_at, user, write_words,
};
use crate::cpu::Cpu;
use crate::win32::gdi32::{self, Rect};
use crate::win32::task::{self, Next, Task};

const WM_CAPTURECHANGED: u32 = 0x0215;

const WA_INACTIVE: u32 = 0;
const WA_ACTIVE: u32 = 1;

const MK_LBUTTON: u32 = 0x01;
const MK_RBUTTON: u32 = 0x02;
const MK_SHIFT: u32 = 0x04;
const MK_CONTROL: u32 = 0x08;
const MK_MBUTTON: u32 = 0x10;

const VK_LBUTTON: usize = 0x01;
const VK_RBUTTON: usize = 0x02;
const VK_MBUTTON: usize = 0x04;
const VK_SHIFT: usize = 0x10;
const VK_CONTROL: usize = 0x11;
const VK_MENU: usize = 0x12;
const VK_CAPITAL: usize = 0x14;
const VK_F10: usize = 0x79;

/// Hit test codes, as WM_NCHITTEST returns them.
pub const HTERROR: u32 = -2i32 as u32;
pub const HTTRANSPARENT: u32 = -1i32 as u32;
pub const HTNOWHERE: u32 = 0;
pub const HTCLIENT: u32 = 1;
pub const HTCAPTION: u32 = 2;
pub const HTSYSMENU: u32 = 3;
pub const HTSIZE: u32 = 4;
pub const HTMENU: u32 = 5;
pub const HTHSCROLL: u32 = 6;
pub const HTVSCROLL: u32 = 7;
pub const HTMINBUTTON: u32 = 8;
pub const HTMAXBUTTON: u32 = 9;
pub const HTLEFT: u32 = 10;
pub const HTRIGHT: u32 = 11;
pub const HTTOP: u32 = 12;
pub const HTTOPLEFT: u32 = 13;
pub const HTTOPRIGHT: u32 = 14;
pub const HTBOTTOM: u32 = 15;
pub const HTBOTTOMLEFT: u32 = 16;
pub const HTBOTTOMRIGHT: u32 = 17;
pub const HTBORDER: u32 = 18;
pub const HTCLOSE: u32 = 20;

/// Milliseconds between the clicks of a double click.
const DOUBLE_CLICK_MS: u32 = 500;

const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_CLIPBOARD_NOT_OPEN: u32 = 1418;

/// The input's state.
#[derive(Clone, Debug, Default)]
pub struct Input {
    pub focus: u32,
    pub active: u32,
    pub capture: u32,
    pub foreground: u32,
    /// BeginDeferWindowPos's lists, by the handle's place.
    pub defers: Vec<Option<Vec<Vec<u32>>>>,
    /// Where the cursor is on the screen.
    pub cursor: (i32, i32),
    /// The key state of the messages taken (GetKeyState): 80h down, 1
    /// toggled.
    pub keys: Vec<u8>,
    /// The keys as the devices have them (GetAsyncKeyState): 80h down, 1
    /// pressed since the last call.
    pub async_keys: Vec<u8>,
    /// ClipCursor's rectangle.
    pub clip: Option<Rect>,
    /// SetCursor's cursor, and ShowCursor's count.
    pub cursor_handle: u32,
    pub cursor_count: i32,
    /// What the devices had when they were last read: the keys held, the
    /// mouse's place, its buttons and how often each was pressed and
    /// released.
    pub held: [u64; 8],
    pub device_pos: (i32, i32),
    pub buttons: u8,
    pub presses: [u16; 3],
    pub releases: [u16; 3],
    /// The last click: its message, window, time and place, for double
    /// clicks.
    pub last_click: [u32; 5],
    /// DefWindowProcA's menu keys: 1 when Alt went down alone, 2 when
    /// F10 did, so that their release opens the menu bar.
    pub menu_keys: u32,
    /// The key that last went down with Alt held, VK_MENU for Alt itself:
    /// Alt's release is WM_SYSKEYUP only after Alt alone.
    pub sys_key: usize,
    /// TrackMouseEvent's window, its TME_HOVER and TME_LEAVE flags and
    /// its hover time (monitor.rs).
    pub tracking: [u32; 3],
}

impl Input {
    pub fn key(&self, vk: usize) -> u8 {
        self.keys.get(vk).copied().unwrap_or(0)
    }

    fn async_down(&self, vk: usize) -> bool {
        self.async_keys.get(vk).is_some_and(|&k| k & 0x80 != 0)
    }

    fn set_key(&mut self, vk: usize, down: bool) {
        if self.keys.len() < 256 {
            self.keys.resize(256, 0);
        }
        let k = &mut self.keys[vk & 0xFF];
        if down {
            if *k & 0x80 == 0 {
                *k ^= 1;
            }
            *k |= 0x80;
        } else {
            *k &= !0x80;
        }
    }

    fn set_async(&mut self, vk: usize, down: bool) {
        if self.async_keys.len() < 256 {
            self.async_keys.resize(256, 0);
        }
        let k = &mut self.async_keys[vk & 0xFF];
        if down {
            *k |= 0x81;
        } else {
            *k &= !0x80;
        }
    }
}

/// The caret.
#[derive(Clone, Debug, Default)]
pub struct Caret {
    pub hwnd: u32,
    pub bitmap: u32,
    pub width: i32,
    pub height: i32,
    pub pos: (i32, i32),
    /// HideCaret's count: it shows at 0.
    pub hidden: i32,
    /// Whether it is drawn now.
    pub on: bool,
}

/// The clipboard.
#[derive(Clone, Debug, Default)]
pub struct Clipboard {
    /// The thread that opened it and the window it gave, if it is open.
    pub open_tid: u32,
    pub open_hwnd: u32,
    pub owner: u32,
    /// The formats and their data's handles, in the order they came.
    pub formats: Vec<(u32, u32)>,
}

/// The desktop came on the screen: the cursor starts in its middle,
/// where the mouse is.
pub fn desktop_started(cpu: &mut Cpu) {
    let (x, y) = (cpu.bus.mouse.x, cpu.bus.mouse.y);
    let mouse = &cpu.bus.mouse;
    let (buttons, presses, releases) = (mouse.buttons, mouse.press_count, mouse.release_count);
    let held = cpu.bus.kbd.held_keys();
    let input = &mut user(cpu).input;
    input.cursor = (x, y);
    input.device_pos = (x, y);
    input.buttons = buttons;
    input.presses = presses;
    input.releases = releases;
    input.held = held;
}

// The devices.

/// The virtual key of the key with scan code `scan` (E0 first when
/// `extended`), with Num Lock on or off.
fn vk_of(scan: u8, extended: bool, num_lock: bool) -> usize {
    const MAIN: [u8; 0x59] = [
        0, 0x1B, b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'0', 0xBD, 0xBB, 0x08,
        0x09, b'Q', b'W', b'E', b'R', b'T', b'Y', b'U', b'I', b'O', b'P', 0xDB, 0xDD, 0x0D, 0x11,
        b'A', b'S', b'D', b'F', b'G', b'H', b'J', b'K', b'L', 0xBA, 0xDE, 0xC0, 0x10, 0xDC, b'Z',
        b'X', b'C', b'V', b'B', b'N', b'M', 0xBC, 0xBE, 0xBF, 0x10, 0x6A, 0x12, 0x20, 0x14, 0x70,
        0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x90, 0x91, 0x24, 0x26, 0x21, 0x6D,
        0x25, 0x0C, 0x27, 0x6B, 0x23, 0x28, 0x22, 0x2D, 0x2E, 0, 0, 0xE2, 0x7A, 0x7B,
    ];
    if extended {
        return match scan {
            0x1C => 0x0D,
            0x1D => 0x11,
            0x35 => 0x6F,
            0x38 => 0x12,
            0x5B => 0x5B,
            0x5C => 0x5C,
            0x5D => 0x5D,
            0x47..=0x53 => MAIN[scan as usize] as usize,
            _ => 0,
        };
    }
    if num_lock {
        let pad = match scan {
            0x47 => 0x67,
            0x48 => 0x68,
            0x49 => 0x69,
            0x4B => 0x64,
            0x4C => 0x65,
            0x4D => 0x66,
            0x4F => 0x61,
            0x50 => 0x62,
            0x51 => 0x63,
            0x52 => 0x60,
            0x53 => 0x6E,
            _ => 0,
        };
        if pad != 0 {
            return pad;
        }
    }
    MAIN.get(scan as usize).copied().unwrap_or(0) as usize
}

/// The scan code of the virtual key `vk`, and whether it is extended.
fn scan_of(vk: usize) -> (u8, bool) {
    for extended in [false, true] {
        for scan in 1..0x59u8 {
            if vk_of(scan, extended, false) == vk || vk_of(scan, extended, true) == vk {
                return (scan, extended);
            }
        }
    }
    (0, false)
}

/// The left and right keys of Shift, Ctrl and Alt.
fn sided(vk: usize, scan: u8, extended: bool) -> Option<usize> {
    match vk {
        VK_SHIFT => Some(if scan == 0x36 { 0xA1 } else { 0xA0 }),
        VK_CONTROL => Some(if extended { 0xA3 } else { 0xA2 }),
        VK_MENU => Some(if extended { 0xA5 } else { 0xA4 }),
        _ => None,
    }
}

/// The character of `vk` with the shift state of `keys`, as ToAscii
/// makes it on the US keyboard.
fn char_of(vk: usize, keys: &dyn Fn(usize) -> u8) -> Option<u8> {
    let down = |v: usize| keys(v) & 0x80 != 0;
    let shift = down(VK_SHIFT);
    let ctrl = down(VK_CONTROL);
    let alt = down(VK_MENU);
    let caps = keys(VK_CAPITAL) & 1 != 0;
    if ctrl && alt {
        return None;
    }
    let c = match vk {
        0x41..=0x5A => {
            let upper = shift != caps;
            if ctrl {
                return Some(vk as u8 - 0x40);
            }
            if upper { vk as u8 } else { vk as u8 + 0x20 }
        }
        0x30..=0x39 => {
            if shift {
                b")!@#$%^&*("[vk - 0x30]
            } else {
                vk as u8
            }
        }
        0x60..=0x69 => b'0' + (vk - 0x60) as u8,
        0x6A => b'*',
        0x6B => b'+',
        0x6D => b'-',
        0x6E => b'.',
        0x6F => b'/',
        0x20 => b' ',
        0x0D => b'\r',
        0x08 => 0x08,
        0x09 => b'\t',
        0x1B => 0x1B,
        _ => {
            let (plain, shifted, control) = match vk {
                0xBA => (b';', b':', 0),
                0xBB => (b'=', b'+', 0),
                0xBC => (b',', b'<', 0),
                0xBD => (b'-', b'_', 0),
                0xBE => (b'.', b'>', 0),
                0xBF => (b'/', b'?', 0),
                0xC0 => (b'`', b'~', 0),
                0xDB => (b'[', b'{', 0x1B),
                0xDC => (b'\\', b'|', 0x1C),
                0xDD => (b']', b'}', 0x1D),
                0xDE => (b'\'', b'"', 0),
                _ => return None,
            };
            if ctrl {
                return (control != 0).then_some(control);
            }
            if shift { shifted } else { plain }
        }
    };
    if ctrl && !matches!(vk, 0x0D | 0x08 | 0x1B) {
        return None;
    }
    Some(c)
}

/// Read the keyboard and the mouse: what they did since they were last
/// read goes to the queues. DirectInput gets the keys that went down and
/// up, also while the desktop isn't on the screen, when USER takes none.
///
/// It runs when time passes (thread::wake) and whenever the program
/// looks at its input: when it takes or peeks at a message, asks what
/// its queue has or waits for it, and gets or sets the cursor or the
/// asynchronous keys. Windows' own input thread puts the devices' input
/// in the queues as it comes, so the program sees a move or a key before
/// it next asks, not only after the next interrupt.
pub fn poll(cpu: &mut Cpu) {
    let desktop = cpu.bus.win32.user.desktop;
    let taken = crate::win32::dinput::mouse_taken(cpu);
    if steers_with_mouse(cpu, taken) {
        let now = cpu.bus.clock.now_ns();
        cpu.bus.mouse.note_steering(now);
    }
    if desktop {
        poll_mouse(cpu, taken);
    }
    poll_keyboard(cpu, desktop);
}

/// Whether the program steers with the mouse's motion: DirectInput has
/// the mouse exclusively (`taken`), or it hid the cursor and has the
/// capture, as games do that put the cursor back in the middle after
/// each move (Unreal). The host's mouse then moves the emulated one by
/// its motion (`MouseState::note_steering`); where the host's pointer is
/// on the picture means nothing to such a program.
fn steers_with_mouse(cpu: &mut Cpu, taken: bool) -> bool {
    if taken {
        return true;
    }
    let state = user(cpu);
    let input = &state.input;
    input.cursor_count < 0 && input.capture != 0 && state.is_window(input.capture)
}

fn poll_keyboard(cpu: &mut Cpu, desktop: bool) {
    let held = cpu.bus.kbd.held_keys();
    let pressed = cpu.bus.kbd.take_pressed();
    let before = user(cpu).input.held;
    let mut events = Vec::new();
    // It runs at each wake of the threads and each message the program
    // looks for, many thousand times a second: most often no key
    // changed, and only the bits that did are visited.
    for (w, (&now, &was)) in held.iter().zip(&before).enumerate() {
        let mut changed = now ^ was;
        while changed != 0 {
            let bit = changed.trailing_zeros() as usize;
            changed &= changed - 1;
            events.push((w * 64 + bit, now >> bit & 1 != 0));
        }
    }
    user(cpu).input.held = held;
    for &(id, down) in &events {
        crate::win32::dinput::key(cpu, id as u8 & 0x7F, id >= 256, down);
    }
    if !desktop {
        // A key that went down and up again since the last read: USER
        // leaves the BIOS's keystrokes to the console, so the keyboard's
        // own record of it tells DirectInput.
        for (w, &went_down) in pressed.iter().enumerate() {
            let mut tapped = went_down & !(before[w] | held[w]);
            while tapped != 0 {
                let id = w * 64 + tapped.trailing_zeros() as usize;
                tapped &= tapped - 1;
                crate::win32::dinput::key(cpu, id as u8 & 0x7F, id >= 256, true);
                crate::win32::dinput::key(cpu, id as u8 & 0x7F, id >= 256, false);
            }
        }
        return;
    }
    // The keystrokes the BIOS got: each is taken, so the buffer doesn't
    // fill.
    let strokes = crate::win32::dos(cpu, |cpu| {
        let mut keys = Vec::new();
        while keys.len() < 64
            && let Some(key) = crate::keyboard::take_keystroke(&mut cpu.bus)
        {
            keys.push(key);
        }
        keys
    });
    let mut fresh: Vec<usize> = events.iter().filter(|e| e.1).map(|e| e.0).collect();
    for &(id, down) in &events {
        key_event(cpu, id as u8 & 0x7F, id >= 256, down, false);
    }
    for key in strokes {
        let scan = (key >> 8) as u8;
        if scan == 0 || scan > 0x58 {
            continue;
        }
        let mut id = scan as usize | ((key & 0xFF == 0xE0) as usize) << 8;
        // A grey key's keystroke can have 0 for its character where E0h
        // would say which key it was: the extended key of the scan code,
        // when that one went down or is held and the other isn't.
        let known = |id: usize| fresh.contains(&id) || held[id / 64] >> (id % 64) & 1 != 0;
        if key & 0xFF == 0 && !known(id) && known(id | 0x100) {
            id |= 0x100;
        }
        if let Some(i) = fresh.iter().position(|&f| f == id) {
            fresh.remove(i);
        } else if held[id / 64] >> (id % 64) & 1 != 0 {
            key_event(cpu, scan, id >= 256, true, true);
        } else {
            // Down and up again since the keyboard was last read.
            crate::win32::dinput::key(cpu, scan, id >= 256, true);
            crate::win32::dinput::key(cpu, scan, id >= 256, false);
            key_event(cpu, scan, id >= 256, true, false);
            key_event(cpu, scan, id >= 256, false, false);
        }
    }
}

/// The key `vk` (and `side`, its left or right one) goes down or up:
/// the message and lParam for it. With Alt held and Ctrl not, keys are
/// WM_SYSKEYDOWN; Alt's own release is WM_SYSKEYUP only when no other
/// key went down since it did. With no focus, and for F10, every key
/// message is a system one.
fn key_message(
    input: &mut Input,
    vk: usize,
    side: Option<usize>,
    scan: u8,
    extended: bool,
    down: bool,
    repeat: bool,
) -> (u32, u32) {
    let was_down = input.async_down(vk);
    let mut sys = input.focus == 0 || vk == VK_F10;
    if down {
        input.set_async(vk, true);
        if input.async_down(VK_MENU) && !input.async_down(VK_CONTROL) {
            sys = true;
            input.sys_key = vk;
        }
    } else {
        if input.async_down(VK_MENU)
            && (vk == VK_MENU || vk == VK_CONTROL || !input.async_down(VK_CONTROL))
        {
            sys |= input.sys_key == VK_MENU || vk != VK_MENU;
            input.sys_key = 0;
        }
        input.set_async(vk, false);
    }
    if let Some(side) = side {
        input.set_async(side, down);
    }
    let message = match (down, sys) {
        (true, false) => WM_KEYDOWN,
        (false, false) => WM_KEYUP,
        (true, true) => WM_SYSKEYDOWN,
        (false, true) => WM_SYSKEYUP,
    };
    let lparam = 1
        | (scan as u32) << 16
        | (extended as u32) << 24
        | (input.async_down(VK_MENU) as u32) << 29
        | ((!down || repeat || was_down) as u32) << 30
        | (!down as u32) << 31;
    (message, lparam)
}

/// A key went down or up: the message for the window that gets the
/// keyboard.
fn key_event(cpu: &mut Cpu, scan: u8, extended: bool, down: bool, repeat: bool) {
    let num_lock = crate::keyboard::lock_lights(&cpu.bus) & 2 != 0;
    let vk = vk_of(scan, extended, num_lock);
    if vk == 0 {
        return;
    }
    let time = message::now_ms(cpu);
    let state = user(cpu);
    let side = sided(vk, scan, extended);
    let (message, lparam) = key_message(&mut state.input, vk, side, scan, extended, down, repeat);
    let input = &state.input;
    let (focus, active, cursor) = (input.focus, input.active, input.cursor);
    let target = if focus != 0 { focus } else { active };
    let Some(tid) = state.window(target).map(|w| w.tid) else {
        return;
    };
    push_input(
        state,
        tid,
        Msg {
            hwnd: target,
            message,
            wparam: vk as u32,
            lparam,
            time,
            x: cursor.0,
            y: cursor.1,
        },
        QS_KEY,
    );
}

fn push_input(state: &mut State, tid: u32, msg: Msg, bit: u32) {
    let q = state.queue_mut(tid);
    // A move after a move of the same window replaces it.
    if (msg.message == WM_MOUSEMOVE || msg.message == WM_NCMOUSEMOVE)
        && let Some(last) = q.input.back_mut()
        && last.message == msg.message
        && last.hwnd == msg.hwnd
    {
        *last = msg;
        q.changed |= bit;
        return;
    }
    q.input.push_back(msg);
    q.changed |= bit;
}

/// Read the mouse; `taken` when DirectInput has it exclusively.
fn poll_mouse(cpu: &mut Cpu, taken: bool) {
    let mouse = &cpu.bus.mouse;
    let (x, y) = (mouse.x, mouse.y);
    let (presses, releases, buttons) = (mouse.press_count, mouse.release_count, mouse.buttons);
    if taken {
        // DirectInput has the mouse exclusively: the cursor stays and no
        // window gets its messages.
        let input = &mut user(cpu).input;
        input.device_pos = (x, y);
        input.presses = presses;
        input.releases = releases;
        input.buttons = buttons;
        return;
    }
    let input = &mut user(cpu).input;
    if (x, y) != input.device_pos {
        input.device_pos = (x, y);
        input.cursor = clipped(input, x, y);
        mouse_event_at(cpu, WM_MOUSEMOVE, 0);
    }
    for b in 0..3 {
        let input = &mut user(cpu).input;
        let mut down_count = presses[b].wrapping_sub(input.presses[b]);
        let mut up_count = releases[b].wrapping_sub(input.releases[b]);
        input.presses[b] = presses[b];
        input.releases[b] = releases[b];
        let mut down = input.buttons & 1 << b != 0;
        while down_count > 0 || up_count > 0 {
            if !down && down_count > 0 {
                down_count -= 1;
                down = true;
            } else if down && up_count > 0 {
                up_count -= 1;
                down = false;
            } else {
                break;
            }
            button_event(cpu, b, down);
        }
    }
}

/// The cursor at (`x`, `y`), kept on the screen and in ClipCursor's
/// rectangle.
fn clipped(input: &Input, x: i32, y: i32) -> (i32, i32) {
    let (w, h) = screen_size();
    let r = input.clip.unwrap_or(Rect::new(0, 0, w, h));
    (
        x.clamp(r.left, (r.right - 1).max(r.left)),
        y.clamp(r.top, (r.bottom - 1).max(r.top)),
    )
}

/// The mouse button `b` (left, right, middle) went down or up.
fn button_event(cpu: &mut Cpu, b: usize, down: bool) {
    let vk = [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON][b];
    let input = &mut user(cpu).input;
    input.set_async(vk, down);
    if down {
        input.buttons |= 1 << b;
    } else {
        input.buttons &= !(1 << b);
    }
    // WM_LBUTTONDOWN, WM_RBUTTONDOWN, WM_MBUTTONDOWN and their ups.
    let message = WM_LBUTTONDOWN + 3 * b as u32 + !down as u32;
    mouse_event_at(cpu, message, b as u32);
}

/// The MK_ flags of the buttons and keys as the devices have them.
fn mk_flags(input: &Input) -> u32 {
    let mut flags = 0;
    for (vk, mk) in [
        (VK_LBUTTON, MK_LBUTTON),
        (VK_RBUTTON, MK_RBUTTON),
        (VK_MBUTTON, MK_MBUTTON),
        (VK_SHIFT, MK_SHIFT),
        (VK_CONTROL, MK_CONTROL),
    ] {
        if input.async_down(vk) {
            flags |= mk;
        }
    }
    flags
}

/// A mouse message (a client one) at the cursor, for the window that has
/// the capture or the one under the cursor.
fn mouse_event_at(cpu: &mut Cpu, message: u32, _button: u32) {
    if message == WM_MOUSEMOVE {
        super::monitor::mouse_moved(cpu);
    }
    let time = message::now_ms(cpu);
    let state = user(cpu);
    let (x, y) = state.input.cursor;
    let capture = state.input.capture;
    let (hwnd, hit) = if capture != 0 && state.is_window(capture) {
        (capture, HTCLIENT)
    } else {
        let hwnd = window::window_at(state, x, y);
        if hwnd == DESKTOP || hwnd == 0 {
            return;
        }
        (hwnd, hit_test(state, hwnd, x, y))
    };
    // A disabled window, or one in a disabled window, gets no input.
    let mut h = hwnd;
    while let Some(w) = state.window(h) {
        if w.style & WS_DISABLED != 0 && message != WM_MOUSEMOVE {
            return;
        }
        if w.style & WS_CHILD == 0 {
            break;
        }
        h = w.parent;
    }
    let Some(w) = state.window(hwnd) else {
        return;
    };
    let tid = w.tid;
    let dbl_class = super::class::style_of(state, hwnd) & 0x0008 != 0;
    let mut message = message;
    let client = hit == HTCLIENT;
    let is_down = message != WM_MOUSEMOVE && (message - WM_LBUTTONDOWN).is_multiple_of(3);
    if is_down {
        let [last_msg, last_hwnd, last_time, lx, ly] = state.input.last_click;
        let (lx, ly) = (lx as i32, ly as i32);
        let near = (x - lx).abs() <= metric(36) / 2 && (y - ly).abs() <= metric(37) / 2;
        if last_msg == message
            && last_hwnd == hwnd
            && time.wrapping_sub(last_time) <= DOUBLE_CLICK_MS
            && near
            && (dbl_class || !client)
        {
            message += 2;
            state.input.last_click = [0; 5];
        } else {
            state.input.last_click = [message, hwnd, time, x as u32, y as u32];
        }
    }
    let (wparam, lparam, message) = if client {
        let (cx, cy) = state.client_origin(hwnd);
        (
            mk_flags(&state.input),
            super::make_long(x - cx, y - cy),
            message,
        )
    } else {
        (
            hit,
            super::make_long(x, y),
            message - WM_MOUSEMOVE + WM_NCMOUSEMOVE,
        )
    };
    let bit = if message == WM_MOUSEMOVE || message == WM_NCMOUSEMOVE {
        QS_MOUSEMOVE
    } else {
        QS_MOUSEBUTTON
    };
    push_input(
        state,
        tid,
        Msg {
            hwnd,
            message,
            wparam,
            lparam,
            time,
            x,
            y,
        },
        bit,
    );
}

/// What part of `hwnd` the screen point (`x`, `y`) is in, as
/// DefWindowProcA's WM_NCHITTEST has it.
pub fn hit_test(state: &State, hwnd: u32, x: i32, y: i32) -> u32 {
    let Some(w) = state.window(hwnd) else {
        return HTNOWHERE;
    };
    let mut r = state.screen_rect(hwnd);
    if !r.contains(x, y) {
        return HTNOWHERE;
    }
    let (style, ex) = (w.style, w.ex_style);
    if style & WS_MINIMIZE != 0 {
        return HTCAPTION;
    }
    if state.screen_client(hwnd).contains(x, y) {
        return HTCLIENT;
    }
    let m = metric;
    if window::has_thick_frame(style, ex) {
        let inner = r.inflate(-m(32), -m(33));
        if !inner.contains(x, y) {
            let corner = m(4);
            let left = x < r.left + corner;
            let right = x >= r.right - corner;
            let top = y < r.top + corner;
            let bottom = y >= r.bottom - corner;
            if y < inner.top {
                return if left {
                    HTTOPLEFT
                } else if right {
                    HTTOPRIGHT
                } else {
                    HTTOP
                };
            }
            if y >= inner.bottom {
                return if left {
                    HTBOTTOMLEFT
                } else if right {
                    HTBOTTOMRIGHT
                } else {
                    HTBOTTOM
                };
            }
            if x < inner.left {
                return if top {
                    HTTOPLEFT
                } else if bottom {
                    HTBOTTOMLEFT
                } else {
                    HTLEFT
                };
            }
            return if top {
                HTTOPRIGHT
            } else if bottom {
                HTBOTTOMRIGHT
            } else {
                HTRIGHT
            };
        }
        r = inner;
    } else if window::has_dlg_frame(style, ex) {
        let inner = r.inflate(-m(7), -m(8));
        if !inner.contains(x, y) {
            return HTBORDER;
        }
        r = inner;
    } else if window::has_thin_frame(style) {
        let inner = r.inflate(-m(5), -m(6));
        if !inner.contains(x, y) {
            return HTBORDER;
        }
        r = inner;
    }
    if style & WS_CAPTION == WS_CAPTION {
        let tool = ex & WS_EX_TOOLWINDOW != 0;
        let height = if tool { m(51) } else { m(4) };
        if y < r.top + height {
            let button = if tool { m(52) } else { m(30) };
            if style & WS_SYSMENU != 0 {
                if !tool && x < r.left + m(31) {
                    return HTSYSMENU;
                }
                let mut right = r.right;
                if x >= right - button {
                    return HTCLOSE;
                }
                right -= button;
                if !tool && style & (WS_MAXIMIZEBOX | WS_MINIMIZEBOX) != 0 {
                    if x >= right - button {
                        return HTMAXBUTTON;
                    }
                    if x >= right - 2 * button {
                        return HTMINBUTTON;
                    }
                }
            }
            return HTCAPTION;
        }
        r.top += height;
    }
    if style & WS_CHILD == 0 && w.id != 0 {
        if y < r.top + m(15) {
            return HTMENU;
        }
        r.top += m(15);
    }
    if ex & WS_EX_CLIENTEDGE != 0 {
        r = r.inflate(-m(45), -m(46));
    }
    if style & WS_VSCROLL != 0 && x >= r.right - m(2) {
        if style & WS_HSCROLL != 0 && y >= r.bottom - m(3) {
            return HTSIZE;
        }
        return HTVSCROLL;
    }
    if style & WS_HSCROLL != 0 && y >= r.bottom - m(3) {
        return HTHSCROLL;
    }
    HTNOWHERE
}

/// GetMessageA took `msg` from the input: the key state follows it.
pub fn message_taken(state: &mut State, msg: &Msg) {
    let input = &mut state.input;
    match msg.message {
        WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP => {
            let down = msg.message == WM_KEYDOWN || msg.message == WM_SYSKEYDOWN;
            let vk = msg.wparam as usize & 0xFF;
            input.set_key(vk, down);
            let scan = (msg.lparam >> 16) as u8;
            if let Some(side) = sided(vk, scan, msg.lparam & 1 << 24 != 0) {
                input.set_key(side, down);
            }
        }
        m if (WM_LBUTTONDOWN..=0x0209).contains(&m) => {
            let b = ((m - WM_LBUTTONDOWN) / 3) as usize;
            let down = (m - WM_LBUTTONDOWN) % 3 != 1;
            input.set_key([VK_LBUTTON, VK_RBUTTON, VK_MBUTTON][b], down);
        }
        m if (WM_NCLBUTTONDOWN..=0x00A9).contains(&m) => {
            let b = ((m - WM_NCLBUTTONDOWN) / 3) as usize;
            let down = (m - WM_NCLBUTTONDOWN) % 3 != 1;
            input.set_key([VK_LBUTTON, VK_RBUTTON, VK_MBUTTON][b.min(2)], down);
        }
        _ => {}
    }
}

/// Whether the button message `msg`, which GetMessageA takes, activates
/// a window first: the top-level window it is for, if that isn't active.
pub fn click_activates(cpu: &mut Cpu, msg: &Msg) -> Option<u32> {
    let down = match msg.message {
        m @ (WM_LBUTTONDOWN..=0x0209) => (m - WM_LBUTTONDOWN).is_multiple_of(3),
        m @ (WM_NCLBUTTONDOWN..=0x00A9) => (m - WM_NCLBUTTONDOWN).is_multiple_of(3),
        _ => false,
    };
    if !down {
        return None;
    }
    let state = user(cpu);
    let top = state.top_level(msg.hwnd);
    let w = state.window(top)?;
    if top == state.input.active || w.style & (WS_DISABLED | WS_VISIBLE) != WS_VISIBLE {
        return None;
    }
    Some(top)
}

// Focus and activation.

const TASK_INPUT: u16 = 19;

const K_FOCUS: u32 = 0;
const K_ACTIVATE: u32 = 1;
const K_CAPTURE: u32 = 2;
const K_OTHER: u32 = 3;

/// The window, if it belongs to the running thread.
fn own(cpu: &mut Cpu, hwnd: u32) -> u32 {
    let tid = current_tid(cpu);
    match user(cpu).window(hwnd) {
        Some(w) if w.tid == tid => hwnd,
        _ => 0,
    }
}

/// GetFocus.
pub fn get_focus(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let focus = user(cpu).input.focus;
    Some(own(cpu, focus))
}

/// GetActiveWindow.
pub fn get_active_window(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let active = user(cpu).input.active;
    Some(own(cpu, active))
}

/// GetForegroundWindow.
pub fn get_foreground_window(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(user(cpu).input.foreground)
}

/// GetCapture.
pub fn get_capture(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let capture = user(cpu).input.capture;
    Some(own(cpu, capture))
}

/// SetFocus(hwnd): the window that had the focus. Its top-level window
/// is activated first.
pub fn set_focus(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hwnd = args[0];
    let state = user(cpu);
    let previous = state.input.focus;
    if hwnd != 0 {
        if hwnd == previous {
            return Some(previous);
        }
        if !state.is_window(hwnd) {
            set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
            return Some(0);
        }
        // Not a minimized or disabled window, or one in one.
        let mut top = hwnd;
        loop {
            let Some(w) = state.window(top) else {
                return Some(0);
            };
            if w.style & (WS_MINIMIZE | WS_DISABLED) != 0 {
                return Some(0);
            }
            if w.parent == DESKTOP || w.parent == 0 || w.style & WS_CHILD == 0 {
                break;
            }
            top = w.parent;
        }
        let vars = vec![K_FOCUS, hwnd, previous, top];
        return task::start(cpu, TASK_INPUT, vars, 0);
    }
    if previous == 0 {
        return Some(0);
    }
    task::start(cpu, TASK_INPUT, vec![K_FOCUS, 0, previous, 0], 0)
}

fn focus_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let (hwnd, top) = (t.var(1), t.var(3));
    loop {
        match t.step {
            0 => {
                // The CBT hook may refuse the change.
                if super::hook::has(cpu, super::window::WH_CBT) {
                    let previous = t.var(2);
                    let (cbt, code) = (super::window::WH_CBT, super::window::HCBT_SETFOCUS);
                    return super::hook::call(cbt, code, hwnd, previous, 9);
                }
                t.step = 10;
            }
            9 => {
                if value != 0 {
                    return Next::Done(0);
                }
                t.step = 10;
            }
            10 => {
                if hwnd != 0 && top != user(cpu).input.active {
                    return Next::call(address_of("SetActiveWindow"), &[top], 1);
                }
                t.step = 1;
            }
            1 => {
                let state = user(cpu);
                if hwnd != 0 && (!state.is_window(hwnd) || top != state.input.active) {
                    return Next::Done(0);
                }
                let previous = state.input.focus;
                t.set(2, previous);
                if previous == hwnd {
                    return Next::Done(previous);
                }
                state.input.focus = hwnd;
                if state.is_window(previous) {
                    return activate_send(cpu, previous, WM_KILLFOCUS, hwnd, 0, 2);
                }
                t.step = 2;
            }
            2 => {
                let state = user(cpu);
                let previous = t.var(2);
                if state.input.focus != hwnd || !state.is_window(hwnd) {
                    return Next::Done(previous);
                }
                return activate_send(cpu, hwnd, WM_SETFOCUS, previous, 0, 3);
            }
            _ => return Next::Done(t.var(2)),
        }
    }
}

/// SetActiveWindow(hwnd): the window that was active.
pub fn set_active_window(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hwnd = args[0];
    let state = user(cpu);
    let previous = state.input.active;
    if hwnd != 0 {
        let Some(w) = state.window(hwnd) else {
            set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
            return Some(0);
        };
        if w.style & (WS_POPUP | WS_CHILD) == WS_CHILD {
            return get_active_window(cpu, &[]);
        }
    }
    if hwnd == previous {
        return Some(previous);
    }
    task::start(cpu, TASK_INPUT, vec![K_ACTIVATE, hwnd, previous, 0, 0], 0)
}

/// The places of SetActiveWindow's variables: the WM_ACTIVATEAPP
/// messages to send, three words each, start at `A_LIST`.
const A_AT: usize = 4;
const A_LIST: usize = 5;

fn iconic(state: &State, hwnd: u32) -> u32 {
    state
        .window(hwnd)
        .is_some_and(|w| w.style & WS_MINIMIZE != 0) as u32
}

/// An activation message for `hwnd`: sent, or to another thread's window
/// queued without waiting for it, as that thread may be waiting for this
/// one.
fn activate_send(cpu: &mut Cpu, hwnd: u32, msg: u32, wp: u32, lp: u32, step: u16) -> Next {
    let tid = user(cpu).window(hwnd).map_or(0, |w| w.tid);
    if tid != current_tid(cpu) {
        let args = [hwnd, msg, wp, lp];
        return Next::call(address_of("SendNotifyMessageA"), &args, step);
    }
    window::send(hwnd, msg, wp, lp, step)
}

fn activate_step(cpu: &mut Cpu, t: &mut Task, _value: u32) -> Next {
    let (hwnd, previous) = (t.var(1), t.var(2));
    loop {
        match t.step {
            0 => {
                if user(cpu).is_window(previous) {
                    return activate_send(cpu, previous, WM_NCACTIVATE, FALSE, hwnd, 1);
                }
                t.step = 2;
            }
            1 => {
                let state = user(cpu);
                if state.is_window(previous) {
                    let wp = WA_INACTIVE | iconic(state, previous) << 16;
                    return activate_send(cpu, previous, WM_ACTIVATE, wp, hwnd, 2);
                }
                t.step = 2;
            }
            2 => {
                let state = user(cpu);
                if hwnd != 0 && !state.is_window(hwnd) {
                    return Next::Done(previous);
                }
                state.input.active = hwnd;
                if hwnd != 0 {
                    state.input.foreground = hwnd;
                } else if state.input.foreground == previous {
                    state.input.foreground = 0;
                }
                // The window comes to the top.
                let first = state.top_levels().into_iter().find(|&h| {
                    state
                        .window(h)
                        .is_some_and(|w| w.ex_style & WS_EX_TOPMOST == 0 || h == hwnd)
                });
                let is_top = state.window(hwnd).is_some_and(|w| w.style & WS_CHILD == 0);
                if hwnd != 0 && is_top && first != Some(hwnd) {
                    let args = [
                        hwnd,
                        HWND_TOP,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE,
                    ];
                    return Next::call(address_of("SetWindowPos"), &args, 3);
                }
                t.step = 3;
            }
            3 => {
                // WM_ACTIVATEAPP when the active window's thread changes.
                let state = user(cpu);
                let old_tid = state.window(previous).map_or(0, |w| w.tid);
                let new_tid = state.window(hwnd).map_or(0, |w| w.tid);
                let mut list = Vec::new();
                if old_tid != new_tid {
                    for (tid, wp, lp) in [(old_tid, FALSE, new_tid), (new_tid, TRUE, old_tid)] {
                        if tid == 0 {
                            continue;
                        }
                        for top in state.top_levels() {
                            if state.window(top).is_some_and(|w| w.tid == tid) {
                                list.extend([top, wp, lp]);
                            }
                        }
                    }
                }
                for (i, v) in list.into_iter().enumerate() {
                    t.set(A_LIST + i, v);
                }
                t.set(A_AT, 0);
                t.step = 4;
            }
            4 => {
                let at = t.var(A_AT) as usize;
                if A_LIST + 3 * at + 2 < t.vars.len() {
                    t.set(A_AT, at as u32 + 1);
                    let (h, wp, lp) = (
                        t.var(A_LIST + 3 * at),
                        t.var(A_LIST + 3 * at + 1),
                        t.var(A_LIST + 3 * at + 2),
                    );
                    if user(cpu).is_window(h) {
                        return activate_send(cpu, h, WM_ACTIVATEAPP, wp, lp, 4);
                    }
                    continue;
                }
                t.step = 5;
            }
            5 => {
                let state = user(cpu);
                if state.is_window(hwnd) {
                    let foreground = (state.input.foreground == hwnd) as u32;
                    return activate_send(cpu, hwnd, WM_NCACTIVATE, foreground, previous, 6);
                }
                t.step = 7;
            }
            6 => {
                let state = user(cpu);
                if state.is_window(hwnd) {
                    let wp = WA_ACTIVE | iconic(state, hwnd) << 16;
                    return activate_send(cpu, hwnd, WM_ACTIVATE, wp, previous, 7);
                }
                t.step = 7;
            }
            7 => {
                // The focus goes into the active window.
                let state = user(cpu);
                let focus = state.input.focus;
                if state.input.active == hwnd
                    && (focus == 0 || hwnd == 0 || state.top_level(focus) != hwnd)
                    && (hwnd != 0 || focus != 0)
                {
                    return Next::call(address_of("SetFocus"), &[hwnd], 8);
                }
                return Next::Done(previous);
            }
            _ => return Next::Done(previous),
        }
    }
}

/// SetForegroundWindow(hwnd): it is activated.
pub fn set_foreground_window(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hwnd = args[0];
    if !user(cpu).is_window(hwnd) {
        return Some(FALSE);
    }
    user(cpu).input.foreground = hwnd;
    let top = user(cpu).top_level(hwnd);
    if top == user(cpu).input.active {
        return Some(TRUE);
    }
    task::start(cpu, TASK_INPUT, vec![K_OTHER, top, 1], 0)
}

/// ActivateOtherWindow(hwnd): `hwnd`, the active window, is going away:
/// its owner, or the first other top-level window that can be, becomes
/// active.
pub fn activate_other_window(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hwnd = args[0];
    let state = user(cpu);
    let usable = |h: u32| {
        h != hwnd
            && state.window(h).is_some_and(|w| {
                w.style & (WS_VISIBLE | WS_DISABLED | WS_MINIMIZE) == WS_VISIBLE
                    && w.ex_style & WS_EX_TOOLWINDOW == 0
            })
    };
    let owner = state.window(hwnd).map_or(0, |w| w.owner);
    let next = if owner != 0 && usable(owner) {
        owner
    } else {
        state
            .top_levels()
            .into_iter()
            .find(|&h| usable(h))
            .unwrap_or(0)
    };
    task::start(cpu, TASK_INPUT, vec![K_OTHER, next, 0], 0)
}

fn other_step(_cpu: &mut Cpu, t: &mut Task, _value: u32) -> Next {
    match t.step {
        0 => Next::call(address_of("SetActiveWindow"), &[t.var(1)], 1),
        _ => Next::Done(t.var(2)),
    }
}

/// SetCapture(hwnd): the window that had the capture.
pub fn set_capture(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hwnd = args[0];
    let state = user(cpu);
    if hwnd != 0 && !state.is_window(hwnd) {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(0);
    }
    let previous = state.input.capture;
    state.input.capture = hwnd;
    if previous != 0 && previous != hwnd && state.is_window(previous) {
        return task::start(
            cpu,
            TASK_INPUT,
            vec![K_CAPTURE, previous, hwnd, previous],
            0,
        );
    }
    Some(own(cpu, previous))
}

/// ReleaseCapture.
pub fn release_capture(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let state = user(cpu);
    let previous = state.input.capture;
    state.input.capture = 0;
    if previous != 0 && state.is_window(previous) {
        return task::start(cpu, TASK_INPUT, vec![K_CAPTURE, previous, 0, TRUE], 0);
    }
    Some(TRUE)
}

fn capture_step(_cpu: &mut Cpu, t: &mut Task, _value: u32) -> Next {
    match t.step {
        0 => window::send(t.var(1), WM_CAPTURECHANGED, 0, t.var(2), 1),
        _ => Next::Done(t.var(3)),
    }
}

/// The input functions' steps.
pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    match t.var(0) {
        K_FOCUS => focus_step(cpu, t, value),
        K_ACTIVATE => activate_step(cpu, t, value),
        K_CAPTURE => capture_step(cpu, t, value),
        _ => other_step(cpu, t, value),
    }
}

// The keyboard's functions.

/// GetKeyState(vk).
pub fn get_key_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let k = user(cpu).input.key(args[0] as usize & 0xFF);
    let mut v = (k & 1) as u32;
    if k & 0x80 != 0 {
        v |= 0xFFFF_FF80;
    }
    Some(v)
}

/// GetAsyncKeyState(vk): 8000h down, 1 pressed since the last call; the
/// devices as they are now.
pub fn get_async_key_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    poll(cpu);
    let vk = args[0] as usize & 0xFF;
    let input = &mut user(cpu).input;
    if input.async_keys.len() < 256 {
        input.async_keys.resize(256, 0);
    }
    let k = input.async_keys[vk];
    input.async_keys[vk] &= !1;
    let mut v = (k & 1) as u32;
    if k & 0x80 != 0 {
        v |= 0xFFFF_8000;
    }
    Some(v)
}

/// GetKeyboardState(buffer): the 256 keys' states.
pub fn get_keyboard_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut keys = user(cpu).input.keys.clone();
    keys.resize(256, 0);
    cpu.bus.guest_write_bytes(args[0], &keys);
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(TRUE)
}

/// SetKeyboardState(buffer).
pub fn set_keyboard_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut keys = vec![0u8; 256];
    cpu.bus.guest_read_bytes(args[0], &mut keys);
    if cpu.bus.guest_faulted() {
        return None;
    }
    user(cpu).input.keys = keys;
    Some(TRUE)
}

/// MapVirtualKeyA(code, type).
pub fn map_virtual_key(_cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (code, kind) = (args[0] as usize, args[1]);
    Some(match kind {
        0 => {
            let vk = match code {
                0xA0 | 0xA1 => VK_SHIFT,
                0xA2 | 0xA3 => VK_CONTROL,
                0xA4 | 0xA5 => VK_MENU,
                _ => code,
            };
            if code == 0xA1 {
                0x36
            } else {
                scan_of(vk).0 as u32
            }
        }
        1 | 3 => {
            let vk = vk_of(code as u8, false, false) as u32;
            if kind == 3 {
                match (vk as usize, code) {
                    (VK_SHIFT, 0x36) => 0xA1,
                    (VK_SHIFT, _) => 0xA0,
                    (VK_CONTROL, _) => 0xA2,
                    (VK_MENU, _) => 0xA4,
                    _ => vk,
                }
            } else {
                vk
            }
        }
        2 => {
            let none = |_: usize| 0u8;
            match char_of(code, &none) {
                Some(c) if c.is_ascii_lowercase() => c.to_ascii_uppercase() as u32,
                Some(c) => c as u32,
                None => 0,
            }
        }
        _ => 0,
    })
}

/// VkKeyScanA(char): the key and the shift state that type it.
pub fn vk_key_scan(_cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let c = args[0] as u8;
    for shift in [0u32, 1, 2] {
        let keys = |vk: usize| match (vk, shift) {
            (VK_SHIFT, 1) | (VK_CONTROL, 2) => 0x80,
            _ => 0,
        };
        for vk in 1..0xFF {
            if char_of(vk, &keys) == Some(c) {
                return Some(vk as u32 | shift << 8);
            }
        }
    }
    Some(0xFFFF)
}

/// OemKeyScan(char): the scan code and shift state.
pub fn oem_key_scan(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let v = vk_key_scan(cpu, args)?;
    if v == 0xFFFF {
        return Some(0xFFFF_FFFF);
    }
    let scan = scan_of(v as usize & 0xFF).0 as u32;
    Some(scan | (v >> 8) << 16)
}

/// The keys' names, by scan code.
pub fn key_name(scan: u8, extended: bool) -> Option<String> {
    let name = match (scan, extended) {
        (0x01, _) => "Esc",
        (0x0E, _) => "Backspace",
        (0x0F, _) => "Tab",
        (0x1C, false) => "Enter",
        (0x1C, true) => "Num Enter",
        (0x1D, false) => "Ctrl",
        (0x1D, true) => "Right Ctrl",
        (0x2A, _) => "Shift",
        (0x36, _) => "Right Shift",
        (0x37, _) => "Num *",
        (0x38, false) => "Alt",
        (0x38, true) => "Right Alt",
        (0x39, _) => "Space",
        (0x3A, _) => "Caps Lock",
        (0x45, _) => "Num Lock",
        (0x46, _) => "Scroll Lock",
        (0x47, true) => "Home",
        (0x48, true) => "Up",
        (0x49, true) => "Page Up",
        (0x4B, true) => "Left",
        (0x4D, true) => "Right",
        (0x4F, true) => "End",
        (0x50, true) => "Down",
        (0x51, true) => "Page Down",
        (0x52, true) => "Insert",
        (0x53, true) => "Delete",
        (0x35, true) => "Num /",
        (0x47, false) => "Num 7",
        (0x48, false) => "Num 8",
        (0x49, false) => "Num 9",
        (0x4A, _) => "Num -",
        (0x4B, false) => "Num 4",
        (0x4C, _) => "Num 5",
        (0x4D, false) => "Num 6",
        (0x4E, _) => "Num +",
        (0x4F, false) => "Num 1",
        (0x50, false) => "Num 2",
        (0x51, false) => "Num 3",
        (0x52, false) => "Num 0",
        (0x53, false) => "Num Del",
        (0x3B..=0x44, _) => return Some(format!("F{}", scan - 0x3A)),
        (0x57, _) => "F11",
        (0x58, _) => "F12",
        _ => {
            let vk = vk_of(scan, extended, false);
            let none = |_: usize| 0u8;
            let c = char_of(vk, &none)?;
            if !c.is_ascii_graphic() {
                return None;
            }
            return Some((c.to_ascii_uppercase() as char).to_string());
        }
    };
    Some(name.to_string())
}

/// GetKeyNameTextA(lParam, buffer, size).
pub fn get_key_name_text(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (lp, buffer, size) = (args[0], args[1], args[2]);
    let scan = (lp >> 16) as u8;
    let extended = lp & 1 << 24 != 0;
    let name = key_name(scan, extended).unwrap_or_default();
    Some(super::copy_out(cpu, buffer, size, name.as_bytes()))
}

/// ToAscii(vk, scan, keys, buffer, flags) and ToAsciiEx.
pub fn to_ascii(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (vk, scan, state, buffer) = (args[0] as usize & 0xFF, args[1], args[2], args[3]);
    if scan & 0x8000 != 0 {
        // A key going up types nothing.
        return Some(0);
    }
    let mut keys = vec![0u8; 256];
    if state != 0 {
        cpu.bus.guest_read_bytes(state, &mut keys);
        if cpu.bus.guest_faulted() {
            return None;
        }
    }
    let get = |v: usize| keys[v & 0xFF];
    match char_of(vk, &get) {
        Some(c) => {
            cpu.bus.guest_write_bytes(buffer, &[c, 0]);
            Some(1)
        }
        None => Some(0),
    }
}

/// TranslateMessage(msg): WM_KEYDOWN posts the WM_CHAR it types.
pub fn translate_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let m = Msg::read(cpu, args[0]);
    if cpu.bus.guest_faulted() {
        return None;
    }
    match m.message {
        WM_KEYDOWN | WM_SYSKEYDOWN => {}
        WM_KEYUP | WM_SYSKEYUP => return Some(TRUE),
        _ => return Some(FALSE),
    }
    let keys = user(cpu).input.keys.clone();
    let get = |v: usize| keys.get(v).copied().unwrap_or(0);
    let Some(c) = char_of(m.wparam as usize & 0xFF, &get) else {
        return Some(TRUE);
    };
    let Some(tid) = user(cpu).window(m.hwnd).map(|w| w.tid) else {
        return Some(TRUE);
    };
    let message = if m.message == WM_KEYDOWN {
        WM_CHAR
    } else {
        WM_SYSCHAR
    };
    message::post(
        cpu,
        tid,
        Msg {
            message,
            wparam: c as u32,
            ..m
        },
    );
    Some(TRUE)
}

/// GetKeyboardLayout(tid), ActivateKeyboardLayout and
/// LoadKeyboardLayoutA: US English.
pub fn get_keyboard_layout(_cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0x0409_0409)
}

/// GetKeyboardLayoutList(count, list).
pub fn get_keyboard_layout_list(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] > 0 && args[1] != 0 {
        cpu.bus.guest_write_32(args[1], 0x0409_0409);
    }
    Some(1)
}

/// GetKeyboardLayoutNameA(buffer).
pub fn get_keyboard_layout_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    cpu.bus.guest_write_bytes(args[0], b"00000409\0");
    Some(TRUE)
}

/// GetKeyboardType(which): an enhanced 101-key keyboard.
pub fn get_keyboard_type(_cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(match args[0] {
        0 => 4,
        1 => 0,
        2 => 12,
        _ => 0,
    })
}

/// GetKBCodePage.
pub fn get_kb_code_page(_cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(437)
}

/// keybd_event(vk, scan, flags, extra): a key as if it was typed.
pub fn keybd_event(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (vk, scan, flags) = (args[0] as usize & 0xFF, args[1] as u8, args[2]);
    let (scan, extended) = if scan != 0 {
        (scan, flags & 1 != 0)
    } else {
        let (s, e) = scan_of(vk);
        (s, e || flags & 1 != 0)
    };
    let down = flags & 2 == 0;
    let num_lock = crate::keyboard::lock_lights(&cpu.bus) & 2 != 0;
    if args[1] != 0 && vk_of(scan, extended, num_lock) == vk {
        key_event(cpu, scan, extended, down, false);
    } else {
        // The message has the scan code and extended flag as given,
        // 0 and all.
        let side = sided(vk, scan, extended);
        synthetic_key(cpu, vk, side, args[1] as u8, flags & 1 != 0, down);
    }
    crate::win32::thread::wake(cpu);
    Some(0)
}

/// A key message for `vk` whatever key the scan code is.
fn synthetic_key(
    cpu: &mut Cpu,
    vk: usize,
    side: Option<usize>,
    scan: u8,
    extended: bool,
    down: bool,
) {
    let time = message::now_ms(cpu);
    let state = user(cpu);
    let (message, lparam) = key_message(&mut state.input, vk, side, scan, extended, down, false);
    let (focus, active, cursor) = (state.input.focus, state.input.active, state.input.cursor);
    let target = if focus != 0 { focus } else { active };
    let Some(tid) = state.window(target).map(|w| w.tid) else {
        return;
    };
    let msg = Msg {
        hwnd: target,
        message,
        wparam: vk as u32,
        lparam,
        time,
        x: cursor.0,
        y: cursor.1,
    };
    push_input(state, tid, msg, QS_KEY);
}

/// SendInput(count, inputs, size): each INPUT as mouse_event or
/// keybd_event would give it. How many it gave.
pub fn send_input(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (count, at, size) = (args[0], args[1], args[2]);
    if size != 28 {
        set_error(cpu, super::ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    for i in 0..count {
        let w = read_words(cpu, at + i * 28, 7);
        if cpu.bus.guest_faulted() {
            return None;
        }
        match w[0] {
            // MOUSEINPUT: dx, dy, data, flags, time, extra.
            0 => mouse_event(cpu, &[w[4], w[1], w[2], w[3], w[6]])?,
            // KEYBDINPUT: the key and scan code, flags, time, extra.
            1 => keybd_event(cpu, &[w[1] & 0xFFFF, w[1] >> 16, w[2], w[4]])?,
            _ => 0,
        };
    }
    Some(count)
}

/// mouse_event(flags, dx, dy, data, extra).
pub fn mouse_event(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (flags, dx, dy) = (args[0], args[1] as i32, args[2] as i32);
    const MOVE: u32 = 0x0001;
    const ABSOLUTE: u32 = 0x8000;
    if flags & MOVE != 0 {
        let (w, h) = screen_size();
        let input = &mut user(cpu).input;
        let (x, y) = if flags & ABSOLUTE != 0 {
            (dx * w / 65536, dy * h / 65536)
        } else {
            (input.cursor.0 + dx, input.cursor.1 + dy)
        };
        input.cursor = clipped(input, x, y);
        mouse_event_at(cpu, WM_MOUSEMOVE, 0);
    }
    for (b, down_bit) in [(0usize, 0x0002u32), (1, 0x0008), (2, 0x0020)] {
        if flags & down_bit != 0 {
            button_event(cpu, b, true);
        }
        if flags & down_bit << 1 != 0 {
            button_event(cpu, b, false);
        }
    }
    crate::win32::thread::wake(cpu);
    Some(0)
}

// The cursor.

/// GetCursorPos(point): where the mouse is now.
pub fn get_cursor_pos(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    poll(cpu);
    let (x, y) = user(cpu).input.cursor;
    write_words(cpu, args[0], &[x as u32, y as u32]);
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(TRUE)
}

/// SetCursorPos(x, y): the mouse goes there too. What the mouse did
/// before is read first: a button that went down while it was elsewhere
/// went down there, not where the cursor is put.
pub fn set_cursor_pos(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    poll(cpu);
    let input = &mut user(cpu).input;
    let (x, y) = clipped(input, args[0] as i32, args[1] as i32);
    input.cursor = (x, y);
    cpu.bus.mouse.warp(x, y);
    let (mx, my) = (cpu.bus.mouse.x, cpu.bus.mouse.y);
    user(cpu).input.device_pos = (mx, my);
    if user(cpu).desktop {
        mouse_event_at(cpu, WM_MOUSEMOVE, 0);
    }
    Some(TRUE)
}

/// ClipCursor(rect): 0 frees it.
pub fn clip_cursor(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let clip = if args[0] == 0 {
        None
    } else {
        let w = read_words(cpu, args[0], 4);
        if cpu.bus.guest_faulted() {
            return None;
        }
        let (sw, sh) = screen_size();
        let r = Rect::new(w[0] as i32, w[1] as i32, w[2] as i32, w[3] as i32)
            .intersect(&Rect::new(0, 0, sw, sh));
        Some(r)
    };
    let input = &mut user(cpu).input;
    input.clip = clip;
    input.cursor = clipped(input, input.cursor.0, input.cursor.1);
    Some(TRUE)
}

/// GetClipCursor(rect).
pub fn get_clip_cursor(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (w, h) = screen_size();
    let r = user(cpu).input.clip.unwrap_or(Rect::new(0, 0, w, h));
    write_words(
        cpu,
        args[0],
        &[r.left as u32, r.top as u32, r.right as u32, r.bottom as u32],
    );
    Some(TRUE)
}

/// SetCursor(cursor): the cursor it had.
pub fn set_cursor(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let input = &mut user(cpu).input;
    Some(std::mem::replace(&mut input.cursor_handle, args[0]))
}

/// GetCursor.
pub fn get_cursor(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(user(cpu).input.cursor_handle)
}

/// ShowCursor(show): the count it then has.
pub fn show_cursor(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let input = &mut user(cpu).input;
    input.cursor_count += if args[0] != 0 { 1 } else { -1 };
    Some(input.cursor_count as u32)
}

/// GetDoubleClickTime.
pub fn get_double_click_time(_cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(DOUBLE_CLICK_MS)
}

// The caret.

/// Draw or take away the caret: it inverts what is under it.
fn toggle_caret(cpu: &mut Cpu) {
    let c = user(cpu).caret.clone();
    if c.hwnd == 0 || !user(cpu).is_visible(c.hwnd) {
        return;
    }
    let hdc = super::paint::window_dc(cpu, c.hwnd, 0, None);
    if hdc == 0 {
        return;
    }
    let r = Rect::new(c.pos.0, c.pos.1, c.pos.0 + c.width, c.pos.1 + c.height);
    if c.bitmap > 1 {
        let mem = gdi32::dc::create_compatible_dc(cpu, &[hdc]).unwrap_or(0);
        if mem != 0 {
            gdi32::dc::select_object(cpu, &[mem, c.bitmap]);
            let args = [
                hdc,
                r.left as u32,
                r.top as u32,
                c.width as u32,
                c.height as u32,
                mem,
                0,
                0,
                0x0066_0046,
            ];
            gdi32::draw::bit_blt(cpu, &args);
            gdi32::dc::delete_dc(cpu, &[mem]);
        }
    } else {
        gdi32::draw::invert_device_rect(cpu, hdc, r);
    }
    super::paint::release_dc(cpu, &[c.hwnd, hdc]);
    let caret = &mut user(cpu).caret;
    caret.on = !caret.on;
}

/// CreateCaret(hwnd, bitmap, width, height).
pub fn create_caret(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, bitmap, width, height) = (args[0], args[1], args[2] as i32, args[3] as i32);
    let tid = current_tid(cpu);
    match user(cpu).window(hwnd) {
        Some(w) if w.tid == tid => {}
        _ => {
            set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
            return Some(FALSE);
        }
    }
    destroy_caret(cpu, &[]);
    let (width, height) = if bitmap > 1 {
        gdi32::gdi(cpu)
            .bitmap(bitmap)
            .map_or((width, height), |b| (b.width, b.height))
    } else {
        (
            if width == 0 { metric(5) } else { width },
            if height == 0 { metric(6) } else { height },
        )
    };
    user(cpu).caret = Caret {
        hwnd,
        bitmap,
        width,
        height,
        pos: (0, 0),
        hidden: 1,
        on: false,
    };
    Some(TRUE)
}

/// DestroyCaret.
pub fn destroy_caret(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let c = user(cpu).caret.clone();
    if c.hwnd == 0 {
        return Some(FALSE);
    }
    if c.on {
        toggle_caret(cpu);
    }
    user(cpu).caret = Caret::default();
    Some(TRUE)
}

/// The window `hwnd` goes: its caret goes with it.
pub fn destroy_caret_of(cpu: &mut Cpu, hwnd: u32) {
    if user(cpu).caret.hwnd == hwnd {
        user(cpu).caret = Caret::default();
    }
}

/// ShowCaret(hwnd).
pub fn show_caret(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let c = user(cpu).caret.clone();
    if c.hwnd == 0 || (args[0] != 0 && args[0] != c.hwnd) {
        return Some(FALSE);
    }
    if c.hidden > 0 {
        user(cpu).caret.hidden -= 1;
        if c.hidden == 1 && !c.on {
            toggle_caret(cpu);
        }
    }
    Some(TRUE)
}

/// HideCaret(hwnd).
pub fn hide_caret(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let c = user(cpu).caret.clone();
    if c.hwnd == 0 || (args[0] != 0 && args[0] != c.hwnd) {
        return Some(FALSE);
    }
    if c.on {
        toggle_caret(cpu);
    }
    user(cpu).caret.hidden += 1;
    Some(TRUE)
}

/// SetCaretPos(x, y).
pub fn set_caret_pos(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let c = user(cpu).caret.clone();
    if c.hwnd == 0 {
        return Some(FALSE);
    }
    let pos = (args[0] as i32, args[1] as i32);
    if pos == c.pos {
        return Some(TRUE);
    }
    if c.on {
        toggle_caret(cpu);
    }
    user(cpu).caret.pos = pos;
    if c.on {
        toggle_caret(cpu);
    }
    Some(TRUE)
}

/// GetCaretPos(point).
pub fn get_caret_pos(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (x, y) = user(cpu).caret.pos;
    write_words(cpu, args[0], &[x as u32, y as u32]);
    Some(TRUE)
}

/// GetCaretBlinkTime.
pub fn get_caret_blink_time(_cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(530)
}

// The clipboard.

fn open_by_me(cpu: &mut Cpu) -> bool {
    let tid = current_tid(cpu);
    user(cpu).clipboard.open_tid == tid && tid != 0
}

/// OpenClipboard(hwnd).
pub fn open_clipboard(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let tid = current_tid(cpu);
    let clip = &mut user(cpu).clipboard;
    if clip.open_tid != 0 && clip.open_tid != tid {
        set_error(cpu, ERROR_ACCESS_DENIED);
        return Some(FALSE);
    }
    clip.open_tid = tid;
    clip.open_hwnd = args[0];
    Some(TRUE)
}

/// CloseClipboard.
pub fn close_clipboard(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    if !open_by_me(cpu) {
        set_error(cpu, ERROR_CLIPBOARD_NOT_OPEN);
        return Some(FALSE);
    }
    let clip = &mut user(cpu).clipboard;
    clip.open_tid = 0;
    clip.open_hwnd = 0;
    Some(TRUE)
}

/// EmptyClipboard: the window that opened it owns it.
pub fn empty_clipboard(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    if !open_by_me(cpu) {
        set_error(cpu, ERROR_CLIPBOARD_NOT_OPEN);
        return Some(FALSE);
    }
    let clip = &mut user(cpu).clipboard;
    clip.formats.clear();
    clip.owner = clip.open_hwnd;
    Some(TRUE)
}

/// SetClipboardData(format, data).
pub fn set_clipboard_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (format, data) = (args[0], args[1]);
    if !open_by_me(cpu) {
        set_error(cpu, ERROR_CLIPBOARD_NOT_OPEN);
        return Some(0);
    }
    let clip = &mut user(cpu).clipboard;
    match clip.formats.iter_mut().find(|f| f.0 == format) {
        Some(f) => f.1 = data,
        None => clip.formats.push((format, data)),
    }
    Some(if data == 0 { format } else { data })
}

/// GetClipboardData(format).
pub fn get_clipboard_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if !open_by_me(cpu) {
        set_error(cpu, ERROR_CLIPBOARD_NOT_OPEN);
        return Some(0);
    }
    let clip = &user(cpu).clipboard;
    Some(
        clip.formats
            .iter()
            .find(|f| f.0 == args[0])
            .map_or(0, |f| f.1),
    )
}

/// IsClipboardFormatAvailable(format).
pub fn is_clipboard_format_available(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let clip = &user(cpu).clipboard;
    Some(clip.formats.iter().any(|f| f.0 == args[0]) as u32)
}

/// GetClipboardOwner.
pub fn get_clipboard_owner(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(user(cpu).clipboard.owner)
}

/// GetOpenClipboardWindow.
pub fn get_open_clipboard_window(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(user(cpu).clipboard.open_hwnd)
}

/// CountClipboardFormats.
pub fn count_clipboard_formats(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(user(cpu).clipboard.formats.len() as u32)
}

/// EnumClipboardFormats(format): the one after it, 0 after the last.
pub fn enum_clipboard_formats(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if !open_by_me(cpu) {
        set_error(cpu, ERROR_CLIPBOARD_NOT_OPEN);
        return Some(0);
    }
    let formats = &user(cpu).clipboard.formats;
    let next = if args[0] == 0 {
        formats.first()
    } else {
        let i = formats.iter().position(|f| f.0 == args[0]);
        i.and_then(|i| formats.get(i + 1))
    };
    let next = next.map_or(0, |f| f.0);
    set_error(cpu, 0);
    Some(next)
}

/// RegisterClipboardFormatA(name): an atom, as RegisterWindowMessageA's.
pub fn register_clipboard_format(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = string_at(cpu, args[0])?;
    if name.is_empty() {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    Some(super::class::atom(user(cpu), &name) as u32)
}

/// GetClipboardFormatNameA(format, buffer, size).
pub fn get_clipboard_format_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (format, buffer, size) = (args[0], args[1], args[2]);
    let name = format
        .checked_sub(0xC000)
        .and_then(|i| user(cpu).registered.get(i as usize).cloned());
    match name {
        Some(name) => Some(super::copy_out(cpu, buffer, size, name.as_bytes())),
        None => {
            set_error(cpu, ERROR_INVALID_PARAMETER);
            Some(0)
        }
    }
}

/// SetLastErrorEx(code, type).
pub fn set_last_error_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    set_error(cpu, args[0]);
    Some(0)
}

crate::state_fields!(Input {
    focus,
    active,
    capture,
    foreground,
    defers,
    cursor,
    keys,
    async_keys,
    clip,
    cursor_handle,
    cursor_count,
    held,
    device_pos,
    buttons,
    presses,
    releases,
    last_click,
    menu_keys,
    sys_key,
    tracking
});
crate::state_fields!(Caret {
    hwnd,
    bitmap,
    width,
    height,
    pos,
    hidden,
    on
});
crate::state_fields!(Clipboard {
    open_tid,
    open_hwnd,
    owner,
    formats
});

#[cfg(test)]
mod tests {
    use super::*;

    /// A program that hid the cursor and took the capture steers with
    /// the mouse: the host's mouse then counts as in use and moving by
    /// its motion, for a second after the last poll that saw it.
    #[test]
    fn a_hidden_captured_cursor_steers_with_the_mouse() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        let now = cpu.bus.clock.now_ns();
        let hwnd = user(&mut cpu).new_window(window::Window::default());
        let hwnd = hwnd.expect("a window");
        user(&mut cpu).input.capture = hwnd;
        poll(&mut cpu);
        assert!(!cpu.bus.mouse.in_use(now), "the cursor shows");
        user(&mut cpu).input.cursor_count = -1;
        poll(&mut cpu);
        assert!(cpu.bus.mouse.in_use(now) && cpu.bus.mouse.moves_by_motion(now));
        assert!(!cpu.bus.mouse.moves_by_motion(now + 2_000_000_000));
        let mut other = Cpu::new(std::path::PathBuf::from("."));
        user(&mut other).input.cursor_count = -1;
        poll(&mut other);
        assert!(!other.bus.mouse.in_use(now), "no capture");
    }

    /// A grey arrow held down is one WM_KEYDOWN, extended, and stays
    /// down: its keystroke doesn't make another key go down and up.
    #[test]
    fn a_held_grey_arrow_stays_down() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        let hwnd = user(&mut cpu).new_window(window::Window::default());
        let hwnd = hwnd.expect("a window");
        user(&mut cpu).desktop = true;
        user(&mut cpu).input.focus = hwnd;
        crate::keyboard::key_event(&mut cpu.bus, 0x48, true, true, None);
        poll(&mut cpu);
        assert!(user(&mut cpu).input.async_down(0x26), "Up is down");
        let tid = user(&mut cpu).window(hwnd).map(|w| w.tid).unwrap_or(0);
        let keys: Vec<(u32, u32)> = user(&mut cpu)
            .queue_mut(tid)
            .input
            .iter()
            .map(|m| (m.message, m.lparam))
            .collect();
        assert_eq!(keys, [(WM_KEYDOWN, 0x0148_0001)]);
    }
}
