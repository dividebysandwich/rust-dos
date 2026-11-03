//! Messages: each thread's queue, sending and posting, GetMessageA and
//! PeekMessageA, DispatchMessageA, and timers.
//!
//! A message sent to a window of the thread that sends it is a call of
//! its window procedure, with the WH_CALLWNDPROC hooks around it. One
//! sent to another thread's window waits in that thread's queue until the
//! thread gets messages (or waits for a reply of its own), while the
//! sender waits for the reply, getting the messages sent to it meanwhile,
//! as on Windows.
//!
//! GetMessageA returns, in this order, the posted messages, WM_QUIT, the
//! input messages, WM_PAINT for a window that needs painting and
//! WM_TIMER for a timer that is due; with none, the thread waits, and
//! the function runs again when something comes (task::Task::retry).

use super::{
    DESKTOP, ERROR_INVALID_PARAMETER, ERROR_INVALID_WINDOW_HANDLE, FALSE, TRUE, address_of,
    current_tid, read_words, set_error, string_at, user, write_words,
};
use crate::cpu::Cpu;
use crate::win32::object::{self, Kind};
use crate::win32::task::{self, Next, Task};
use crate::win32::thread::{self, Wait};
use std::collections::VecDeque;

pub const WM_NULL: u32 = 0x0000;
pub const WM_CREATE: u32 = 0x0001;
pub const WM_DESTROY: u32 = 0x0002;
pub const WM_MOVE: u32 = 0x0003;
pub const WM_SIZE: u32 = 0x0005;
pub const WM_ACTIVATE: u32 = 0x0006;
pub const WM_SETFOCUS: u32 = 0x0007;
pub const WM_KILLFOCUS: u32 = 0x0008;
pub const WM_ENABLE: u32 = 0x000A;
pub const WM_SETREDRAW: u32 = 0x000B;
pub const WM_SETTEXT: u32 = 0x000C;
pub const WM_GETTEXT: u32 = 0x000D;
pub const WM_GETTEXTLENGTH: u32 = 0x000E;
pub const WM_PAINT: u32 = 0x000F;
pub const WM_CLOSE: u32 = 0x0010;
pub const WM_QUERYENDSESSION: u32 = 0x0011;
pub const WM_QUIT: u32 = 0x0012;
pub const WM_QUERYOPEN: u32 = 0x0013;
pub const WM_ERASEBKGND: u32 = 0x0014;
pub const WM_SYSCOLORCHANGE: u32 = 0x0015;
pub const WM_ENDSESSION: u32 = 0x0016;
pub const WM_SHOWWINDOW: u32 = 0x0018;
pub const WM_CTLCOLOR: u32 = 0x0019;
pub const WM_SETTINGCHANGE: u32 = 0x001A;
pub const WM_ACTIVATEAPP: u32 = 0x001C;
pub const WM_FONTCHANGE: u32 = 0x001D;
pub const WM_CANCELMODE: u32 = 0x001F;
pub const WM_SETCURSOR: u32 = 0x0020;
pub const WM_MOUSEACTIVATE: u32 = 0x0021;
pub const WM_CHILDACTIVATE: u32 = 0x0022;
pub const WM_GETMINMAXINFO: u32 = 0x0024;
pub const WM_ICONERASEBKGND: u32 = 0x0027;
pub const WM_NEXTDLGCTL: u32 = 0x0028;
pub const WM_DRAWITEM: u32 = 0x002B;
pub const WM_MEASUREITEM: u32 = 0x002C;
pub const WM_DELETEITEM: u32 = 0x002D;
pub const WM_VKEYTOITEM: u32 = 0x002E;
pub const WM_CHARTOITEM: u32 = 0x002F;
pub const WM_SETFONT: u32 = 0x0030;
pub const WM_GETFONT: u32 = 0x0031;
pub const WM_QUERYDRAGICON: u32 = 0x0037;
pub const WM_COMPAREITEM: u32 = 0x0039;
pub const WM_WINDOWPOSCHANGING: u32 = 0x0046;
pub const WM_WINDOWPOSCHANGED: u32 = 0x0047;
pub const WM_NOTIFY: u32 = 0x004E;
pub const WM_HELP: u32 = 0x0053;
pub const WM_NOTIFYFORMAT: u32 = 0x0055;
pub const WM_CONTEXTMENU: u32 = 0x007B;
pub const WM_STYLECHANGING: u32 = 0x007C;
pub const WM_STYLECHANGED: u32 = 0x007D;
pub const WM_GETICON: u32 = 0x007F;
pub const WM_SETICON: u32 = 0x0080;
pub const WM_NCCREATE: u32 = 0x0081;
pub const WM_NCDESTROY: u32 = 0x0082;
pub const WM_NCCALCSIZE: u32 = 0x0083;
pub const WM_NCHITTEST: u32 = 0x0084;
pub const WM_NCPAINT: u32 = 0x0085;
pub const WM_NCACTIVATE: u32 = 0x0086;
pub const WM_GETDLGCODE: u32 = 0x0087;
pub const WM_NCMOUSEMOVE: u32 = 0x00A0;
pub const WM_NCLBUTTONDOWN: u32 = 0x00A1;
pub const WM_NCLBUTTONUP: u32 = 0x00A2;
pub const WM_NCLBUTTONDBLCLK: u32 = 0x00A3;
pub const WM_NCRBUTTONDOWN: u32 = 0x00A4;
pub const WM_NCRBUTTONUP: u32 = 0x00A5;
pub const WM_KEYDOWN: u32 = 0x0100;
pub const WM_KEYUP: u32 = 0x0101;
pub const WM_CHAR: u32 = 0x0102;
pub const WM_DEADCHAR: u32 = 0x0103;
pub const WM_SYSKEYDOWN: u32 = 0x0104;
pub const WM_SYSKEYUP: u32 = 0x0105;
pub const WM_SYSCHAR: u32 = 0x0106;
pub const WM_SYSDEADCHAR: u32 = 0x0107;
pub const WM_INITDIALOG: u32 = 0x0110;
pub const WM_COMMAND: u32 = 0x0111;
pub const WM_SYSCOMMAND: u32 = 0x0112;
pub const WM_TIMER: u32 = 0x0113;
pub const WM_HSCROLL: u32 = 0x0114;
pub const WM_VSCROLL: u32 = 0x0115;
pub const WM_INITMENU: u32 = 0x0116;
pub const WM_INITMENUPOPUP: u32 = 0x0117;
pub const WM_SYSTIMER: u32 = 0x0118;
pub const WM_MENUSELECT: u32 = 0x011F;
pub const WM_MENUCHAR: u32 = 0x0120;
pub const WM_ENTERIDLE: u32 = 0x0121;
pub const WM_CTLCOLORMSGBOX: u32 = 0x0132;
pub const WM_CTLCOLOREDIT: u32 = 0x0133;
pub const WM_CTLCOLORLISTBOX: u32 = 0x0134;
pub const WM_CTLCOLORBTN: u32 = 0x0135;
pub const WM_CTLCOLORDLG: u32 = 0x0136;
pub const WM_CTLCOLORSCROLLBAR: u32 = 0x0137;
pub const WM_CTLCOLORSTATIC: u32 = 0x0138;
pub const WM_MOUSEMOVE: u32 = 0x0200;
pub const WM_LBUTTONDOWN: u32 = 0x0201;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_LBUTTONDBLCLK: u32 = 0x0203;
pub const WM_RBUTTONDOWN: u32 = 0x0204;
pub const WM_RBUTTONUP: u32 = 0x0205;
pub const WM_RBUTTONDBLCLK: u32 = 0x0206;
pub const WM_MBUTTONDOWN: u32 = 0x0207;
pub const WM_MBUTTONUP: u32 = 0x0208;
pub const WM_MBUTTONDBLCLK: u32 = 0x0209;
pub const WM_PARENTNOTIFY: u32 = 0x0210;
pub const WM_ENTERMENULOOP: u32 = 0x0211;
pub const WM_EXITMENULOOP: u32 = 0x0212;
pub const WM_MDICREATE: u32 = 0x0220;
pub const WM_MDIDESTROY: u32 = 0x0221;
pub const WM_MDIACTIVATE: u32 = 0x0222;
pub const WM_MDIGETACTIVE: u32 = 0x0229;
pub const WM_ENTERSIZEMOVE: u32 = 0x0231;
pub const WM_EXITSIZEMOVE: u32 = 0x0232;
pub const WM_CUT: u32 = 0x0300;
pub const WM_COPY: u32 = 0x0301;
pub const WM_PASTE: u32 = 0x0302;
pub const WM_CLEAR: u32 = 0x0303;
pub const WM_UNDO: u32 = 0x0304;
pub const WM_QUERYNEWPALETTE: u32 = 0x030F;
pub const WM_PALETTEISCHANGING: u32 = 0x0310;
pub const WM_PALETTECHANGED: u32 = 0x0311;
pub const WM_PRINT: u32 = 0x0317;
pub const WM_PRINTCLIENT: u32 = 0x0318;
pub const WM_USER: u32 = 0x0400;
pub const WM_APP: u32 = 0x8000;

pub const SIZE_RESTORED: u32 = 0;
pub const SIZE_MINIMIZED: u32 = 1;
pub const SIZE_MAXIMIZED: u32 = 2;

pub const SC_SIZE: u32 = 0xF000;
pub const SC_MOVE: u32 = 0xF010;
pub const SC_MINIMIZE: u32 = 0xF020;
pub const SC_MAXIMIZE: u32 = 0xF030;
pub const SC_CLOSE: u32 = 0xF060;
pub const SC_KEYMENU: u32 = 0xF100;
pub const SC_RESTORE: u32 = 0xF120;

/// The queue status bits.
pub const QS_KEY: u32 = 0x0001;
pub const QS_MOUSEMOVE: u32 = 0x0002;
pub const QS_MOUSEBUTTON: u32 = 0x0004;
pub const QS_POSTMESSAGE: u32 = 0x0008;
pub const QS_TIMER: u32 = 0x0010;
pub const QS_PAINT: u32 = 0x0020;
pub const QS_SENDMESSAGE: u32 = 0x0040;
pub const QS_HOTKEY: u32 = 0x0080;
pub const QS_MOUSE: u32 = QS_MOUSEMOVE | QS_MOUSEBUTTON;
pub const QS_INPUT: u32 = QS_MOUSE | QS_KEY;
pub const QS_ALLINPUT: u32 = 0x00FF;

/// PeekMessageA's flags.
const PM_REMOVE: u32 = 0x0001;
const PM_NOYIELD: u32 = 0x0002;

pub const HWND_BROADCAST: u32 = 0xFFFF;
const ERROR_TIMEOUT: u32 = 1460;

/// A message in a queue, as MSG has it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Msg {
    pub hwnd: u32,
    pub message: u32,
    pub wparam: u32,
    pub lparam: u32,
    pub time: u32,
    pub x: i32,
    pub y: i32,
}

impl Msg {
    pub fn words(&self) -> [u32; 7] {
        [
            self.hwnd,
            self.message,
            self.wparam,
            self.lparam,
            self.time,
            self.x as u32,
            self.y as u32,
        ]
    }

    pub fn read(cpu: &mut Cpu, at: u32) -> Msg {
        let w = read_words(cpu, at, 7);
        Msg {
            hwnd: w[0],
            message: w[1],
            wparam: w[2],
            lparam: w[3],
            time: w[4],
            x: w[5] as i32,
            y: w[6] as i32,
        }
    }
}

/// A message another thread sent: what it waits for the reply of
/// (`kind` 0), or a notification it doesn't (1), or one whose reply goes
/// to a callback of the sender (2: `callback` and its data).
#[derive(Clone, Debug, Default)]
pub struct Sent {
    pub id: u32,
    pub from: u32,
    pub hwnd: u32,
    pub message: u32,
    pub wparam: u32,
    pub lparam: u32,
    pub kind: u32,
    pub callback: (u32, u32),
}

/// A callback of SendMessageCallbackA to call in the sender's thread:
/// the callback, the window, the message, its data and the result.
#[derive(Clone, Debug, Default)]
pub struct Callback {
    pub proc: u32,
    pub hwnd: u32,
    pub message: u32,
    pub data: u32,
    pub result: u32,
}

/// A thread's message queue.
#[derive(Clone, Debug, Default)]
pub struct Queue {
    pub tid: u32,
    pub posted: VecDeque<Msg>,
    /// Keyboard and mouse messages for the thread's windows.
    pub input: VecDeque<Msg>,
    /// Messages other threads sent that the thread hasn't taken yet.
    pub sent: VecDeque<Sent>,
    pub callbacks: VecDeque<Callback>,
    /// PostQuitMessage's exit code.
    pub quit: Option<u32>,
    /// Replies to the messages the thread sent, by id.
    pub replies: Vec<(u32, u32)>,
    /// The messages the thread waits for the replies of, the innermost
    /// last, with when they time out; 0 marks where it got a message
    /// sent to it meanwhile.
    pub waiting: Vec<(u32, u64)>,
    /// The messages other threads sent it that it handles, the innermost
    /// last; None for a callback of SendMessageCallbackA. Windows takes a
    /// sent message off the queue when the receiver gets it, so one being
    /// handled is here and no longer in `sent`, where a wait of the
    /// thread's own (for a reply, or a modal loop) would take it again.
    pub in_send: Vec<Option<Sent>>,
    /// The queue status bits that came since GetQueueStatus last looked.
    pub changed: u32,
    /// The last message's time and place, for GetMessageTime and
    /// GetMessagePos.
    pub last_time: u32,
    pub last_pos: (i32, i32),
    /// What the thread is to do about the foreground when it next looks
    /// for messages (input::take_activation): what, the window, the
    /// thread on the other side and the input queue; zeros for nothing.
    pub activation: [u32; 4],
}

/// A timer of SetTimer.
#[derive(Clone, Debug, Default)]
pub struct Timer {
    pub hwnd: u32,
    pub id: u32,
    pub tid: u32,
    /// Milliseconds between its messages, and when the next is due, by
    /// the clock's microseconds.
    pub elapse: u32,
    pub due: u64,
    pub proc: u32,
    /// A system timer (SetSystemTimer: WM_SYSTIMER).
    pub system: bool,
}

/// A message filter: the window (and its children), and the range of
/// messages (0, 0 for all).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    pub hwnd: u32,
    pub min: u32,
    pub max: u32,
}

impl Filter {
    fn wants(&self, state: &super::State, msg: &Msg) -> bool {
        let in_range = if self.min == 0 && self.max == 0 {
            true
        } else if self.min <= self.max {
            (self.min..=self.max).contains(&msg.message)
        } else {
            // A reversed range takes what is outside it.
            msg.message >= self.min || msg.message <= self.max
        };
        let window = self.hwnd == 0
            || msg.hwnd == self.hwnd
            || (self.hwnd == 0xFFFF_FFFF && msg.hwnd == 0)
            || state.is_descendant(self.hwnd, msg.hwnd);
        in_range && window
    }

    fn wants_message(&self, message: u32) -> bool {
        (self.min == 0 && self.max == 0)
            || if self.min <= self.max {
                (self.min..=self.max).contains(&message)
            } else {
                message >= self.min || message <= self.max
            }
    }
}

/// What a waiting thread's queue must have for it to go on: the queue
/// status bits it waits for, the filter of GetMessageA, and the reply it
/// waits for, if any.
#[derive(Clone, Debug, Default)]
pub struct QueueWait {
    pub mask: u32,
    pub filter: Filter,
    pub reply: u32,
}

/// The milliseconds of the clock, as GetTickCount and message times
/// count them.
pub fn now_ms(cpu: &Cpu) -> u32 {
    (cpu.bus.clock.now_micros() / 1000) as u32
}

/// Whether the queue of `tid` has what `wait` waits for, at the time
/// `now`.
pub fn wait_ready(win32: &crate::win32::Win32, tid: u32, wait: &QueueWait, now: u64) -> bool {
    let state = &win32.user;
    let Some(q) = state.queue(tid) else {
        return false;
    };
    if wait.reply != 0 && q.replies.iter().any(|&(id, _)| id == wait.reply) {
        return true;
    }
    if wait.reply != 0
        && q.waiting
            .iter()
            .any(|&(id, deadline)| id == wait.reply && deadline != 0 && now >= deadline)
    {
        return true;
    }
    let mask = wait.mask;
    if mask & QS_SENDMESSAGE != 0 && (!q.sent.is_empty() || !q.callbacks.is_empty()) {
        return true;
    }
    // The foreground came or went: a thread that waits for posted
    // messages looks for them, and does what it has to on the way. (One
    // that waits only for sent ones, as WaitForInputIdle does, wouldn't
    // look, and would wake again at once.)
    if mask & QS_POSTMESSAGE != 0 && wait.reply == 0 && q.activation[0] != 0 {
        return true;
    }
    let f = &wait.filter;
    if mask & QS_POSTMESSAGE != 0
        && (q.posted.iter().any(|m| f.wants(state, m)) || q.quit.is_some())
    {
        return true;
    }
    if mask & QS_INPUT != 0
        && q.input
            .iter()
            .any(|m| f.wants(state, m) && mask & qs_of(m.message) != 0)
    {
        return true;
    }
    if mask & QS_PAINT != 0
        && f.wants_message(WM_PAINT)
        && paint_window(state, tid, f.hwnd).is_some()
    {
        return true;
    }
    if mask & QS_TIMER != 0
        && state
            .timers
            .iter()
            .any(|t| t.tid == tid && t.due <= now && timer_wanted(state, t, f))
    {
        return true;
    }
    false
}

/// The queue status bit of an input message.
fn qs_of(message: u32) -> u32 {
    match message {
        WM_KEYDOWN..=WM_SYSDEADCHAR => QS_KEY,
        WM_MOUSEMOVE | WM_NCMOUSEMOVE => QS_MOUSEMOVE,
        _ => QS_MOUSEBUTTON,
    }
}

fn timer_wanted(state: &super::State, t: &Timer, f: &Filter) -> bool {
    let message = if t.system { WM_SYSTIMER } else { WM_TIMER };
    f.wants_message(message)
        && (f.hwnd == 0 || t.hwnd == f.hwnd || state.is_descendant(f.hwnd, t.hwnd))
}

/// When the next timer of `tid` is due, if it has one.
fn next_timer(state: &super::State, tid: u32) -> Option<u64> {
    state
        .timers
        .iter()
        .filter(|t| t.tid == tid)
        .map(|t| t.due)
        .min()
}

/// The first window of `tid` (under `hwnd`, if it isn't 0) that needs
/// painting: parents before their children, the top of the z-order
/// first.
pub fn paint_window(state: &super::State, tid: u32, hwnd: u32) -> Option<u32> {
    let root = if hwnd == 0 || hwnd == 0xFFFF_FFFF {
        DESKTOP
    } else {
        hwnd
    };
    let order = super::window::pre_order(state, root);
    order.into_iter().find(|&h| {
        h != DESKTOP
            && state.window(h).is_some_and(|w| {
                w.tid == tid
                    && w.style & super::window::WS_VISIBLE != 0
                    && super::paint::needs_paint(w)
            })
            && state.is_visible(h)
    })
}

/// Whether the window procedure `proc` may be called for a window: an
/// address, not 0.
fn callable(proc: u32) -> bool {
    proc >= 0x1_0000
}

/// The variables of SendMessageA's task.
const SEND_HWND: usize = 0;
const SEND_MSG: usize = 1;
const SEND_WP: usize = 2;
const SEND_LP: usize = 3;
const SEND_RESULT: usize = 4;
/// What it is: a send within the thread (0), the wait for another
/// thread's reply (1), a send with a timeout's place for the result (2).
const SEND_KIND: usize = 5;
const SEND_ID: usize = 6;
const SEND_OUT: usize = 7;

const TASK_SEND: u16 = 0;
const TASK_DISPATCH: u16 = 1;
const TASK_GET: u16 = 2;
const TASK_BROADCAST: u16 = 12;
const TASK_IDLE: u16 = task::INPUT_IDLE;

pub const WH_CALLWNDPROC: u32 = 4;
pub const WH_CALLWNDPROCRET: u32 = 12;
pub const WH_GETMESSAGE: u32 = 3;

/// SendMessageA(hwnd, message, wParam, lParam).
pub fn send_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    send(cpu, args[0], args[1], args[2], args[3], None)
}

/// Send a message, waiting for the reply of another thread until
/// `timeout` (milliseconds) passes if it is given: then the result goes
/// to `out` too.
fn send(
    cpu: &mut Cpu,
    hwnd: u32,
    msg: u32,
    wp: u32,
    lp: u32,
    timeout: Option<(u32, u32)>,
) -> Option<u32> {
    if hwnd == HWND_BROADCAST || hwnd == 0xFFFF_FFFF {
        // Program Manager answers DDE from its first broadcast on.
        if msg == super::progman::WM_DDE_INITIATE {
            super::progman::server(cpu);
        }
        let mut vars = vec![msg, wp, lp, 0, 0];
        vars.extend(user(cpu).top_levels());
        return task::start(cpu, TASK_BROADCAST, vars, 0);
    }
    let state = user(cpu);
    let Some(w) = state.window(hwnd) else {
        if let Some(next) = resume_wait(cpu, hwnd, msg, wp, lp, timeout) {
            return next;
        }
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(0);
    };
    let (tid, proc) = (w.tid, w.proc);
    let me = current_tid(cpu);
    if tid != me && hwnd != DESKTOP && tid != 0 {
        return send_other(cpu, hwnd, tid, msg, wp, lp, timeout);
    }
    if !callable(proc) {
        return Some(0);
    }
    let hooks = super::hook::has(cpu, WH_CALLWNDPROC) || super::hook::has(cpu, WH_CALLWNDPROCRET);
    // SendMessageTimeoutA's result goes to its `out` too, and it returns
    // TRUE.
    let vars = match timeout {
        Some((_, out)) => vec![hwnd, msg, wp, lp, 0, 2, 0, out],
        None => vec![hwnd, msg, wp, lp, 0, 0],
    };
    task::start(cpu, TASK_SEND, vars, if hooks { 20 } else { 0 })
}

/// What SendMessageA's task returns for the window procedure's `result`.
fn send_result(cpu: &mut Cpu, t: &Task, result: u32) -> u32 {
    if t.var(SEND_KIND) == 2 {
        if t.var(SEND_OUT) != 0 {
            cpu.bus.guest_write_32(t.var(SEND_OUT), result);
        }
        return TRUE;
    }
    result
}

/// Send a message to the window of another thread, `tid`: queued there,
/// and the thread waits for the reply. A function that runs again after
/// its wait finds its own message among those it waits for.
fn send_other(
    cpu: &mut Cpu,
    hwnd: u32,
    tid: u32,
    msg: u32,
    wp: u32,
    lp: u32,
    timeout: Option<(u32, u32)>,
) -> Option<u32> {
    let me = current_tid(cpu);
    let now = cpu.bus.clock.now_micros();
    let state = user(cpu);
    let waiting = state.queue_mut(me).waiting.last().copied();
    let id = match waiting {
        Some((id, _)) if id != 0 => id,
        _ if !thread_alive(&cpu.bus.win32, tid) => {
            // The thread whose window it is has ended: nothing replies.
            return Some(0);
        }
        _ => {
            let state = user(cpu);
            state.last_sent += 1;
            let id = state.last_sent;
            let deadline = timeout.map_or(0, |(ms, _)| now + ms as u64 * 1000);
            state
                .queue_mut(me)
                .waiting
                .push((id, deadline.max(1) * (timeout.is_some() as u64)));
            state.queue_mut(tid).sent.push_back(Sent {
                id,
                from: me,
                hwnd,
                message: msg,
                wparam: wp,
                lparam: lp,
                kind: 0,
                callback: (0, 0),
            });
            state.queue_mut(tid).changed |= QS_SENDMESSAGE;
            thread::wake(cpu);
            id
        }
    };
    let out = timeout.map_or(0, |(_, out)| out);
    task::start(cpu, TASK_SEND, vec![hwnd, msg, wp, lp, 0, 1, id, out], 0)
}

/// Whether the current thread's function runs again after a send to
/// another thread: `send` then takes up its wait for the reply.
pub fn waiting_for_reply(cpu: &mut Cpu) -> bool {
    awaited_reply(cpu).is_some()
}

/// The message the current thread waits for the reply of, if that is
/// where it is: not while it handles one sent to it meanwhile.
fn awaited_reply(cpu: &mut Cpu) -> Option<u32> {
    let me = current_tid(cpu);
    let q = user(cpu).queues.iter().find(|q| q.tid == me)?;
    q.waiting.last().map(|&(id, _)| id).filter(|&id| id != 0)
}

/// Whether the thread `tid` is there and hasn't ended.
pub fn thread_alive(win32: &crate::win32::Win32, tid: u32) -> bool {
    thread::find(win32, tid).is_some_and(|(p, t)| win32.processes[p].threads[t].exit_code.is_none())
}

/// A send that waited for another thread's reply runs again, and the
/// window is gone: the other thread destroyed it while it handled the
/// message. The send waits on for that reply, as Windows returns it,
/// instead of failing and leaving its wait on top of the thread's list,
/// where the thread's next send would take the reply for its own.
fn resume_wait(
    cpu: &mut Cpu,
    hwnd: u32,
    msg: u32,
    wp: u32,
    lp: u32,
    timeout: Option<(u32, u32)>,
) -> Option<Option<u32>> {
    let id = awaited_reply(cpu)?;
    let out = timeout.map_or(0, |(_, out)| out);
    Some(task::start(
        cpu,
        TASK_SEND,
        vec![hwnd, msg, wp, lp, 0, 1, id, out],
        0,
    ))
}

/// The thread `tid` ended: the messages other threads sent it and wait
/// for the reply of get 0, so that they don't wait for ever, those it
/// was handling as well as those it hadn't taken.
pub fn thread_ended(win32: &mut crate::win32::Win32, tid: u32) {
    let state = &mut win32.user;
    let Some(q) = state.queues.iter_mut().find(|q| q.tid == tid) else {
        return;
    };
    let handled = q.in_send.drain(..).flatten();
    let pending: Vec<(u32, u32)> = handled
        .chain(q.sent.drain(..))
        .filter(|s| s.kind == 0)
        .map(|s| (s.from, s.id))
        .collect();
    for (from, id) in pending {
        state.queue_mut(from).replies.push((id, 0));
    }
}

/// SendMessageA's steps.
pub fn send_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    if t.var(SEND_KIND) == 1 {
        return reply_step(cpu, t, value);
    }
    let hwnd = t.var(SEND_HWND);
    let args = [hwnd, t.var(SEND_MSG), t.var(SEND_WP), t.var(SEND_LP)];
    loop {
        match t.step {
            0 => {
                if t.scratch != 0 && super::hook::has(cpu, WH_CALLWNDPROC) {
                    write_words(cpu, t.scratch, &[args[3], args[2], args[1], args[0]]);
                    return super::hook::call(WH_CALLWNDPROC, 0, 1, t.scratch, 1);
                }
                t.step = 1;
            }
            1 => {
                let Some(w) = user(cpu).window(hwnd) else {
                    return Next::Done(0);
                };
                let proc = w.proc;
                if !callable(proc) {
                    return Next::Done(send_result(cpu, t, 0));
                }
                return Next::call(proc, &args, 2);
            }
            2 => {
                t.set(SEND_RESULT, value);
                if t.scratch != 0 && super::hook::has(cpu, WH_CALLWNDPROCRET) {
                    write_words(cpu, t.scratch, &[value, args[3], args[2], args[1], args[0]]);
                    return super::hook::call(WH_CALLWNDPROCRET, 0, 1, t.scratch, 3);
                }
                return Next::Done(send_result(cpu, t, value));
            }
            _ => return Next::Done(send_result(cpu, t, t.var(SEND_RESULT))),
        }
    }
}

/// The wait for another thread's reply: the reply, or a message sent to
/// this thread meanwhile, which it handles before it waits on.
fn reply_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let me = current_tid(cpu);
    let id = t.var(SEND_ID);
    loop {
        match t.step {
            0 => {
                let now = cpu.bus.clock.now_micros();
                let state = user(cpu);
                let q = state.queue_mut(me);
                if let Some(i) = q.replies.iter().position(|&(r, _)| r == id) {
                    let (_, result) = q.replies.remove(i);
                    q.waiting.retain(|&(w, _)| w != id);
                    if t.var(SEND_OUT) != 0 {
                        cpu.bus.guest_write_32(t.var(SEND_OUT), result);
                        return Next::Done(TRUE);
                    }
                    return Next::Done(result);
                }
                let deadline = q
                    .waiting
                    .iter()
                    .find(|&&(w, _)| w == id)
                    .map_or(0, |&(_, d)| d);
                if deadline != 0 && now >= deadline {
                    q.waiting.retain(|&(w, _)| w != id);
                    // The reply, if it comes, goes nowhere, whether the
                    // other thread has taken the message or not.
                    for q in state.queues.iter_mut() {
                        let handled = q.in_send.iter_mut().flatten();
                        for s in q.sent.iter_mut().chain(handled) {
                            if s.id == id {
                                s.kind = 1;
                            }
                        }
                    }
                    set_error(cpu, ERROR_TIMEOUT);
                    return Next::Done(0);
                }
                if let Some(next) = handle_sent(cpu, 1) {
                    return next;
                }
                // Wait for the reply, or for a message sent to it.
                let wait = Wait {
                    deadline: (deadline != 0).then_some(deadline),
                    timeout_result: thread::WAIT_TIMEOUT,
                    queue: Some(QueueWait {
                        mask: QS_SENDMESSAGE,
                        filter: Filter::default(),
                        reply: id,
                    }),
                    ..Wait::default()
                };
                t.retry(cpu);
                thread::block(cpu, wait);
                return Next::Stop;
            }
            1 => {
                // A message sent to this thread was handled: reply.
                finish_sent(cpu, value);
                let q = user(cpu).queue_mut(me);
                if q.waiting.last() == Some(&(0, 0)) {
                    q.waiting.pop();
                }
                t.step = 0;
            }
            _ => return Next::Done(0),
        }
    }
}

/// Start handling the first message another thread sent to this one, or
/// the first callback of SendMessageCallbackA due, taking it off the
/// queue: a call, going on at `step` with its result. None when there is
/// none.
fn handle_sent(cpu: &mut Cpu, step: u16) -> Option<Next> {
    let me = current_tid(cpu);
    let state = user(cpu);
    let q = state.queue_mut(me);
    let (call, args) = if let Some(cb) = q.callbacks.pop_front() {
        q.in_send.push(None);
        (cb.proc, [cb.hwnd, cb.message, cb.data, cb.result])
    } else {
        let sent = q.sent.pop_front()?;
        let args = [sent.hwnd, sent.message, sent.wparam, sent.lparam];
        q.in_send.push(Some(sent));
        (address_of("SendMessageA"), args)
    };
    q.waiting.push((0, 0));
    // With none left to handle, QS_SENDMESSAGE no longer counts as new.
    // Windows (and Wine's receive_message) clears it when the receiver
    // gets the last one, not when it replies: else
    // MsgWaitForMultipleObjects would return at once while the window
    // procedure handles it, and for ever after a message another thread
    // sent.
    if q.sent.is_empty() && q.callbacks.is_empty() {
        q.changed &= !QS_SENDMESSAGE;
    }
    Some(Next::call(call, &args, step))
}

/// The message `handle_sent` started handling returned `result`: the
/// sender gets the reply.
fn finish_sent(cpu: &mut Cpu, result: u32) {
    let me = current_tid(cpu);
    let state = user(cpu);
    // A callback ran when it is None.
    let Some(Some(sent)) = state.queue_mut(me).in_send.pop() else {
        return;
    };
    match sent.kind {
        0 => {
            state.queue_mut(sent.from).replies.push((sent.id, result));
        }
        2 => {
            // As in Wine, the result comes to the sender as a sent
            // message does, and wakes a MsgWaitForMultipleObjects that
            // waits for one.
            let q = state.queue_mut(sent.from);
            q.callbacks.push_back(Callback {
                proc: sent.callback.0,
                hwnd: sent.hwnd,
                message: sent.message,
                data: sent.callback.1,
                result,
            });
            q.changed |= QS_SENDMESSAGE;
        }
        _ => {}
    }
    thread::wake(cpu);
}

/// SendMessageTimeoutA(hwnd, message, wParam, lParam, flags, timeout,
/// result): TRUE, with the result in `result`, or 0 when it timed out.
pub fn send_message_timeout(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, msg, wp, lp, _flags, timeout, out] = args else {
        unreachable!()
    };
    if hwnd != HWND_BROADCAST && hwnd != 0xFFFF_FFFF && !user(cpu).is_window(hwnd) {
        if let Some(next) = resume_wait(cpu, hwnd, msg, wp, lp, Some((timeout, out))) {
            return next;
        }
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(0);
    }
    send(cpu, hwnd, msg, wp, lp, Some((timeout, out)))
}

/// SendNotifyMessageA: within the thread a send; to another thread's
/// window queued without a wait for the reply.
pub fn send_notify_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, msg, wp, lp] = args else {
        unreachable!()
    };
    if hwnd == HWND_BROADCAST {
        return post_message(cpu, args);
    }
    let Some(w) = user(cpu).window(hwnd) else {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(FALSE);
    };
    let tid = w.tid;
    if tid == current_tid(cpu) {
        return send(cpu, hwnd, msg, wp, lp, None).map(|_| TRUE);
    }
    let state = user(cpu);
    state.last_sent += 1;
    let id = state.last_sent;
    let from = current_tid(cpu);
    let state = user(cpu);
    state.queue_mut(tid).sent.push_back(Sent {
        id,
        from,
        hwnd,
        message: msg,
        wparam: wp,
        lparam: lp,
        kind: 1,
        callback: (0, 0),
    });
    state.queue_mut(tid).changed |= QS_SENDMESSAGE;
    thread::wake(cpu);
    Some(TRUE)
}

/// SendMessageCallbackA(hwnd, message, wParam, lParam, callback, data):
/// the callback gets the result in the sender's thread, when it next
/// gets messages. To a window of the sender's own thread (or one that
/// SendMessageA handles in place, as the desktop) the message is sent at
/// once and the callback called as soon as the window procedure returns,
/// as Windows documents and Wine does. The message is never queued, so
/// InSendMessage is FALSE while it is handled, and a callback due from
/// an earlier send doesn't run in its place.
pub fn send_message_callback(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, msg, wp, lp, callback, data] = args else {
        unreachable!()
    };
    let Some(w) = user(cpu).window(hwnd) else {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(FALSE);
    };
    let tid = w.tid;
    let me = current_tid(cpu);
    if tid == me || hwnd == DESKTOP || tid == 0 {
        let vars = vec![DISPATCH_CALLBACK_NOW, hwnd, msg, wp, lp, callback, data];
        return task::start(cpu, TASK_DISPATCH, vars, 0);
    }
    let state = user(cpu);
    state.last_sent += 1;
    let id = state.last_sent;
    state.queue_mut(tid).sent.push_back(Sent {
        id,
        from: me,
        hwnd,
        message: msg,
        wparam: wp,
        lparam: lp,
        kind: 2,
        callback: (callback, data),
    });
    state.queue_mut(tid).changed |= QS_SENDMESSAGE;
    thread::wake(cpu);
    Some(TRUE)
}

/// Post `msg` to the queue of the thread `tid`.
pub fn post(cpu: &mut Cpu, tid: u32, msg: Msg) {
    let state = user(cpu);
    let q = state.queue_mut(tid);
    q.posted.push_back(msg);
    q.changed |= QS_POSTMESSAGE;
    thread::wake(cpu);
}

/// The time and cursor position a message posted now has.
fn stamp(cpu: &Cpu, mut msg: Msg) -> Msg {
    msg.time = now_ms(cpu);
    let (x, y) = cpu.bus.win32.user.input.cursor;
    msg.x = x;
    msg.y = y;
    msg
}

/// PostMessageA(hwnd, message, wParam, lParam): 0 for the window goes to
/// the thread's own queue.
pub fn post_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, msg, wp, lp] = args else {
        unreachable!()
    };
    let m = stamp(
        cpu,
        Msg {
            hwnd,
            message: msg,
            wparam: wp,
            lparam: lp,
            ..Msg::default()
        },
    );
    if hwnd == 0 {
        let tid = current_tid(cpu);
        post(cpu, tid, m);
        return Some(TRUE);
    }
    if hwnd == HWND_BROADCAST || hwnd == 0xFFFF_FFFF {
        for h in user(cpu).top_levels() {
            // Program Manager's window has no thread whose queue is read.
            let Some(tid) = user(cpu)
                .window(h)
                .filter(|w| !super::progman::is_server(w))
                .map(|w| w.tid)
            else {
                continue;
            };
            post(
                cpu,
                tid,
                Msg {
                    hwnd: h,
                    ..m.clone()
                },
            );
        }
        return Some(TRUE);
    }
    let Some(w) = user(cpu).window(hwnd) else {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(FALSE);
    };
    // Program Manager's window, of no thread, handles what is posted to
    // it now.
    if super::progman::is_server(w) {
        return super::progman::posted(cpu, hwnd, msg, wp, lp);
    }
    let tid = w.tid;
    post(cpu, tid, m);
    Some(TRUE)
}

/// PostThreadMessageA(tid, message, wParam, lParam).
pub fn post_thread_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[tid, msg, wp, lp] = args else {
        unreachable!()
    };
    if thread::find(&cpu.bus.win32, tid).is_none() {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    let m = stamp(
        cpu,
        Msg {
            hwnd: 0,
            message: msg,
            wparam: wp,
            lparam: lp,
            ..Msg::default()
        },
    );
    post(cpu, tid, m);
    Some(TRUE)
}

/// PostQuitMessage(code): GetMessageA returns WM_QUIT once the queue has
/// no posted messages left.
pub fn post_quit_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let tid = current_tid(cpu);
    let q = user(cpu).queue_mut(tid);
    q.quit = Some(args[0]);
    q.changed |= QS_POSTMESSAGE;
    Some(0)
}

/// The variables of GetMessageA's task.
const GET_MSG: usize = 0;
const GET_HWND: usize = 1;
const GET_MIN: usize = 2;
const GET_MAX: usize = 3;
/// PM_REMOVE and the like for PeekMessageA; for GetMessageA, PM_REMOVE
/// and that it waits.
const GET_FLAGS: usize = 4;
const GET_RESULT: usize = 5;
const GET_REMOVE: usize = 6;
/// The window to send WM_SETCURSOR to (0 for none), and its lParam.
const GET_CURSOR_HWND: usize = 7;
const GET_CURSOR_LP: usize = 8;
/// The window a click activates, while WM_MOUSEACTIVATE is asked; then
/// whether the click is eaten.
const GET_ACTIVATE: usize = 9;
const WAITS: u32 = 0x8000_0000;

/// GetMessageA(msg, hwnd, min, max).
pub fn get_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[msg, hwnd, min, max] = args else {
        unreachable!()
    };
    if hwnd != 0 && hwnd != 0xFFFF_FFFF && !user(cpu).is_window(hwnd) {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(-1i32 as u32);
    }
    task::start(
        cpu,
        TASK_GET,
        vec![msg, hwnd, min, max, PM_REMOVE | WAITS, 0],
        0,
    )
}

/// PeekMessageA(msg, hwnd, min, max, flags).
pub fn peek_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[msg, hwnd, min, max, flags] = args else {
        unreachable!()
    };
    if hwnd != 0 && hwnd != 0xFFFF_FFFF && !user(cpu).is_window(hwnd) {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(FALSE);
    }
    let before = cpu.bus.observe.on.then(|| quiet(cpu)).flatten();
    let result = task::start(
        cpu,
        TASK_GET,
        vec![msg, hwnd, min, max, flags & !WAITS, 0],
        0,
    );
    // Found nothing and changed nothing: an idle proof goes through it,
    // up to when the thread's next timer is due.
    if let Some(before) = before
        && result == Some(FALSE)
        && quiet(cpu).is_some_and(|after| after == before)
    {
        let tid = current_tid(cpu);
        let now = cpu.bus.clock.now_micros();
        let due = user(cpu)
            .timers
            .iter()
            .filter(|t| t.tid == tid && t.due > now)
            .map(|t| t.due)
            .min();
        let until = due.map(|due| cpu.bus.clock.icount_at_micros(due));
        crate::idle::pure_call(cpu, until);
    }
    result
}

/// What a PeekMessage that finds nothing may change, compared before and
/// after it during an idle proof: the queue's own state, what reading
/// the devices changes (`input::seen`) and the idle event that
/// WaitForInputIdle waits for. None when a read of the devices changes
/// something whatever it finds.
#[derive(PartialEq)]
struct Quiet {
    activation: [u32; 4],
    changed: u32,
    lengths: [usize; 7],
    seen: super::input::Seen,
    idle: Option<bool>,
}

fn quiet(cpu: &mut Cpu) -> Option<Quiet> {
    let seen = super::input::seen(cpu)?;
    let idle = crate::win32::running(cpu).and_then(|p| p.idle_event);
    let idle = idle.and_then(|id| match cpu.bus.win32.objects.get(id).map(|o| &o.kind) {
        Some(Kind::Event { signaled, .. }) => Some(*signaled),
        _ => None,
    });
    let tid = current_tid(cpu);
    let q = user(cpu).queue(tid)?;
    Some(Quiet {
        activation: q.activation,
        changed: q.changed,
        lengths: [
            q.posted.len(),
            q.input.len(),
            q.sent.len(),
            q.callbacks.len(),
            q.in_send.len(),
            q.waiting.len(),
            q.replies.len(),
        ],
        seen,
        idle,
    })
}

/// WaitMessage: the thread waits for a message to come.
pub fn wait_message(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    super::input::poll(cpu);
    input_idle(cpu);
    let tid = current_tid(cpu);
    let now = cpu.bus.clock.now_micros();
    let state = user(cpu);
    let q = state.queue_mut(tid);
    if std::mem::take(&mut q.changed) != 0 {
        return Some(TRUE);
    }
    let wait = QueueWait {
        mask: QS_ALLINPUT,
        filter: Filter::default(),
        reply: 0,
    };
    if wait_ready(&cpu.bus.win32, tid, &wait, now) {
        return Some(TRUE);
    }
    let deadline = next_timer(user(cpu), tid);
    thread::block(
        cpu,
        Wait {
            deadline,
            timeout_result: TRUE,
            queue: Some(wait),
            ..Wait::default()
        },
    );
    // What the wait returns doesn't matter: TRUE.
    None
}

/// Take the message the filter `f` wants from the thread `tid`'s queue,
/// if one is there; `remove` takes it out.
fn take_message(cpu: &mut Cpu, tid: u32, f: Filter, remove: bool) -> Option<Msg> {
    // What the devices did since they were last read comes first: a move
    // replaces the one SetCursorPos queued, which the program's next
    // SetCursorPos would otherwise undo before any poll saw it.
    super::input::poll(cpu);
    let now = cpu.bus.clock.now_micros();
    let pid = super::current_pid(cpu);
    let state = user(cpu);
    // Posted messages.
    let q = state.queue(tid)?;
    if let Some(i) = q.posted.iter().position(|m| f.wants(state, m)) {
        let q = state.queue_mut(tid);
        let m = if remove {
            q.posted.remove(i).unwrap()
        } else {
            q.posted[i].clone()
        };
        return Some(m);
    }
    // WM_QUIT.
    if let Some(code) = q.quit
        && f.wants_message(WM_QUIT)
    {
        if remove {
            state.queue_mut(tid).quit = None;
        }
        return Some(Msg {
            message: WM_QUIT,
            wparam: code,
            time: (now / 1000) as u32,
            ..Msg::default()
        });
    }
    // Input.
    let q = state.queue(tid)?;
    if let Some(i) = q.input.iter().position(|m| f.wants(state, m)) {
        let q = state.queue_mut(tid);
        let m = if remove {
            q.input.remove(i).unwrap()
        } else {
            q.input[i].clone()
        };
        if remove {
            super::input::message_taken(state, &m);
            // A key or button the user pressed: its process may bring a
            // window to the foreground. A message a program posts, even
            // one of these, doesn't count.
            if is_input(m.message) {
                state.input.last_input = pid;
            }
        }
        return Some(m);
    }
    // WM_PAINT.
    if f.wants_message(WM_PAINT)
        && let Some(hwnd) = paint_window(state, tid, f.hwnd)
    {
        let (x, y) = state.input.cursor;
        return Some(Msg {
            hwnd,
            message: WM_PAINT,
            time: (now / 1000) as u32,
            x,
            y,
            ..Msg::default()
        });
    }
    // WM_TIMER.
    let i = state
        .timers
        .iter()
        .position(|t| t.tid == tid && t.due <= now && timer_wanted(state, t, &f))?;
    let t = &mut state.timers[i];
    if remove {
        t.due = now + t.elapse as u64 * 1000;
    }
    let (x, y) = state.input.cursor;
    Some(Msg {
        hwnd: t.hwnd,
        message: if t.system { WM_SYSTIMER } else { WM_TIMER },
        wparam: t.id,
        lparam: t.proc,
        time: (now / 1000) as u32,
        x,
        y,
    })
}

/// Whether `message` is a key or a mouse button going down: the input
/// that lets the thread that takes it bring a window to the foreground.
fn is_input(message: u32) -> bool {
    // The downs and double clicks of the three buttons.
    let button = |first: u32| (first..first + 9).contains(&message) && (message - first) % 3 != 1;
    let key = matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN);
    key || button(WM_LBUTTONDOWN) || button(WM_NCLBUTTONDOWN)
}

/// GetMessageA's and PeekMessageA's steps.
pub fn get_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let tid = current_tid(cpu);
    loop {
        match t.step {
            0 => {
                // Messages other threads sent come first.
                if let Some(next) = handle_sent(cpu, 1) {
                    return next;
                }
                // So does what the queue has to do as the foreground came
                // or went.
                if let Some(next) = super::input::take_activation(cpu, 4) {
                    return next;
                }
                let f = Filter {
                    hwnd: t.var(GET_HWND),
                    min: t.var(GET_MIN),
                    max: t.var(GET_MAX),
                };
                let flags = t.var(GET_FLAGS);
                let remove = flags & PM_REMOVE != 0;
                if let Some(m) = take_message(cpu, tid, f, remove) {
                    if matches!(m.message, WM_TIMER | WM_SYSTIMER) && flags & PM_NOYIELD == 0 {
                        input_idle(cpu);
                    }
                    let state = user(cpu);
                    let q = state.queue_mut(tid);
                    q.last_time = m.time;
                    q.last_pos = (m.x, m.y);
                    q.changed = 0;
                    write_words(cpu, t.var(GET_MSG), &m.words());
                    let result = if flags & WAITS != 0 {
                        (m.message != WM_QUIT) as u32
                    } else {
                        TRUE
                    };
                    t.set(GET_RESULT, result);
                    t.set(GET_REMOVE, remove as u32);
                    let cursor = super::input::sets_cursor(cpu, &m).filter(|_| remove);
                    let (hwnd, lp) = cursor.unwrap_or((0, 0));
                    t.set(GET_CURSOR_HWND, hwnd);
                    t.set(GET_CURSOR_LP, lp);
                    // A click on a window that isn't the active one in the
                    // foreground: WM_MOUSEACTIVATE asks the window under
                    // the cursor whether it activates its top-level
                    // window, and whether the click is still delivered.
                    if remove && let Some((top, lp)) = super::input::click_activates(cpu, &m) {
                        t.set(GET_ACTIVATE, top);
                        return super::window::send(m.hwnd, WM_MOUSEACTIVATE, top, lp, 5);
                    }
                    t.step = 3;
                    continue;
                }
                if flags & PM_NOYIELD == 0 {
                    input_idle(cpu);
                }
                if flags & WAITS == 0 {
                    // Peeking finds nothing.
                    return Next::Done(FALSE);
                }
                let deadline = next_timer(user(cpu), tid);
                let wait = Wait {
                    deadline,
                    timeout_result: 0,
                    queue: Some(QueueWait {
                        mask: QS_ALLINPUT,
                        filter: f,
                        reply: 0,
                    }),
                    ..Wait::default()
                };
                t.retry(cpu);
                thread::block(cpu, wait);
                return Next::Stop;
            }
            1 => {
                finish_sent(cpu, value);
                let q = user(cpu).queue_mut(tid);
                if q.waiting.last() == Some(&(0, 0)) {
                    q.waiting.pop();
                }
                t.step = 0;
            }
            4 => t.step = 0,
            5 => {
                // The answer to WM_MOUSEACTIVATE: 0 activates as
                // MA_ACTIVATE does. A click that is eaten isn't returned:
                // the thread looks for the next message.
                use super::input::{MA_ACTIVATE, MA_ACTIVATEANDEAT, MA_NOACTIVATEANDEAT};
                let top = t.var(GET_ACTIVATE);
                let eat = value == MA_ACTIVATEANDEAT || value == MA_NOACTIVATEANDEAT;
                t.set(GET_ACTIVATE, eat as u32);
                t.step = 6;
                let activates = matches!(value, 0 | MA_ACTIVATE | MA_ACTIVATEANDEAT);
                if activates && user(cpu).is_window(top) {
                    return Next::call(address_of("ActivateByClick"), &[top], 6);
                }
            }
            6 => {
                t.step = if t.var(GET_ACTIVATE) != 0 { 0 } else { 3 };
            }
            3 => {
                // Windows sends WM_SETCURSOR for the mouse message, and
                // the window's answer sets the cursor.
                let hwnd = t.var(GET_CURSOR_HWND);
                if hwnd != 0 {
                    t.set(GET_CURSOR_HWND, 0);
                    if user(cpu).is_window(hwnd) {
                        let lp = t.var(GET_CURSOR_LP);
                        return super::window::send(hwnd, WM_SETCURSOR, hwnd, lp, 3);
                    }
                }
                if super::hook::has(cpu, WH_GETMESSAGE) {
                    let remove = t.var(GET_REMOVE);
                    return super::hook::call(WH_GETMESSAGE, 0, remove, t.var(GET_MSG), 2);
                }
                return Next::Done(t.var(GET_RESULT));
            }
            _ => return Next::Done(t.var(GET_RESULT)),
        }
    }
}

/// The variables of DispatchMessageA's task.
const DISPATCH_KIND: usize = 0;
const DISPATCH_MSG: usize = 1;
const DISPATCH_NORMAL: u32 = 0;
/// SendMessageCallbackA within the thread: send the message, then call
/// the callback with its result. Its variables after the kind: the
/// window, the message, its parameters, the callback and its data.
const DISPATCH_CALLBACK_NOW: u32 = 1;

/// DispatchMessageA(msg): the window procedure's result.
pub fn dispatch_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let m = Msg::read(cpu, args[0]);
    if cpu.bus.guest_faulted() {
        return None;
    }
    if (m.message == WM_TIMER || m.message == WM_SYSTIMER) && m.lparam != 0 {
        // A timer's callback, if the timer is there.
        let state = user(cpu);
        let found = state
            .timers
            .iter()
            .any(|t| t.proc == m.lparam && t.id == m.wparam && t.hwnd == m.hwnd);
        if !found {
            return Some(0);
        }
        let time = now_ms(cpu);
        return task::start(
            cpu,
            TASK_DISPATCH,
            vec![DISPATCH_NORMAL, m.lparam, m.hwnd, m.message, m.wparam, time],
            0,
        );
    }
    if m.hwnd == 0 {
        return Some(0);
    }
    let tid = current_tid(cpu);
    let Some(w) = user(cpu).window(m.hwnd) else {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(0);
    };
    if w.tid != tid {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(0);
    }
    let proc = w.proc;
    if !callable(proc) {
        return Some(0);
    }
    task::start(
        cpu,
        TASK_DISPATCH,
        vec![DISPATCH_NORMAL, proc, m.hwnd, m.message, m.wparam, m.lparam],
        0,
    )
}

/// DispatchMessageA's steps.
pub fn dispatch_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    if t.var(DISPATCH_KIND) == DISPATCH_CALLBACK_NOW {
        let [hwnd, msg, wp, lp, callback, data] = [1, 2, 3, 4, 5, 6].map(|i| t.var(i));
        return match t.step {
            0 => Next::call(address_of("SendMessageA"), &[hwnd, msg, wp, lp], 1),
            1 if callable(callback) => Next::call(callback, &[hwnd, msg, data, value], 2),
            _ => Next::Done(TRUE),
        };
    }
    let args: Vec<u32> = t.vars[DISPATCH_MSG + 1..DISPATCH_MSG + 5].to_vec();
    match t.step {
        0 => Next::call(t.var(DISPATCH_MSG), &args, 1),
        1 => {
            let (hwnd, msg) = (args[0], args[1]);
            if msg == WM_PAINT {
                // A window that painted without BeginPaint still gets
                // WM_NCPAINT and WM_ERASEBKGND it needs.
                t.set(DISPATCH_MSG + 6, value);
                if let Some(next) = super::paint::pending_erase(cpu, hwnd, 2) {
                    return next;
                }
            }
            Next::Done(value)
        }
        _ => Next::Done(t.var(DISPATCH_MSG + 6)),
    }
}

/// The variables of a broadcast: the message, its parameters, the
/// result, the next window's place, then the windows.
pub fn broadcast_step(cpu: &mut Cpu, t: &mut Task, _value: u32) -> Next {
    loop {
        let at = t.var(4) as usize;
        let Some(&hwnd) = t.vars.get(5 + at) else {
            return Next::Done(TRUE);
        };
        t.set(4, at as u32 + 1);
        if user(cpu).is_window(hwnd) {
            let args = [hwnd, t.var(0), t.var(1), t.var(2)];
            return Next::call(address_of("SendMessageA"), &args, 1);
        }
    }
}

/// GetQueueStatus(flags): what is in the queue (the high word) and what
/// came since it was last asked (the low word).
pub fn get_queue_status(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    super::input::poll(cpu);
    let tid = current_tid(cpu);
    let now = cpu.bus.clock.now_micros();
    let mask = args[0];
    let mut have = 0;
    for bit in [
        QS_KEY,
        QS_MOUSEMOVE,
        QS_MOUSEBUTTON,
        QS_POSTMESSAGE,
        QS_TIMER,
        QS_PAINT,
        QS_SENDMESSAGE,
    ] {
        let wait = QueueWait {
            mask: bit,
            filter: Filter::default(),
            reply: 0,
        };
        if wait_ready(&cpu.bus.win32, tid, &wait, now) {
            have |= bit;
        }
    }
    let q = user(cpu).queue_mut(tid);
    let changed = q.changed & mask;
    q.changed &= !mask;
    Some((have & mask) << 16 | changed)
}

/// GetInputState: whether keyboard or mouse input waits.
pub fn get_input_state(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    super::input::poll(cpu);
    let tid = current_tid(cpu);
    let any = user(cpu).queue(tid).is_some_and(|q| !q.input.is_empty());
    Some(any as u32)
}

pub fn get_message_pos(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let tid = current_tid(cpu);
    let (x, y) = user(cpu).queue_mut(tid).last_pos;
    Some(super::make_long(x, y))
}

pub fn get_message_time(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let tid = current_tid(cpu);
    Some(user(cpu).queue_mut(tid).last_time)
}

/// InSendMessage: whether the thread handles a message another thread
/// sent.
pub fn in_send_message(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let tid = current_tid(cpu);
    let in_send = user(cpu)
        .queue(tid)
        .is_some_and(|q| q.in_send.last().is_some_and(Option::is_some));
    Some(in_send as u32)
}

/// ReplyMessage(result): the sender gets `result` now, and the window
/// procedure's own result later goes nowhere.
pub fn reply_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let tid = current_tid(cpu);
    let state = user(cpu);
    let q = state.queue_mut(tid);
    let Some(Some(sent)) = q.in_send.last_mut() else {
        return Some(FALSE);
    };
    if sent.kind != 0 {
        return Some(FALSE);
    }
    sent.kind = 1;
    let (from, id) = (sent.from, sent.id);
    state.queue_mut(from).replies.push((id, args[0]));
    thread::wake(cpu);
    Some(TRUE)
}

/// CallWindowProcA(proc, hwnd, message, wParam, lParam).
pub fn call_window_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if !callable(args[0]) {
        return Some(0);
    }
    task::start(
        cpu,
        TASK_DISPATCH,
        vec![DISPATCH_NORMAL, args[0], args[1], args[2], args[3], args[4]],
        0,
    )
}

/// RegisterWindowMessageA(name): a message id from USER's atom table.
pub fn register_window_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = string_at(cpu, args[0])?;
    if name.is_empty() {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    Some(super::class::atom(user(cpu), &name) as u32)
}

/// MsgWaitForMultipleObjects(count, handles, all, timeout, mask).
pub fn msg_wait_for_multiple_objects(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    msg_wait(cpu, args[0], args[1], args[2] != 0, args[3], args[4])
}

/// MsgWaitForMultipleObjectsEx(count, handles, timeout, mask, flags).
pub fn msg_wait_for_multiple_objects_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let all = args[4] & 1 != 0;
    msg_wait(cpu, args[0], args[1], all, args[2], args[3])
}

fn msg_wait(
    cpu: &mut Cpu,
    count: u32,
    handles: u32,
    all: bool,
    timeout: u32,
    mask: u32,
) -> Option<u32> {
    super::input::poll(cpu);
    input_idle(cpu);
    let tid = current_tid(cpu);
    let now = cpu.bus.clock.now_micros();
    let wait = QueueWait {
        mask,
        filter: Filter::default(),
        reply: 0,
    };
    // Input that came since the queue was last looked at ends it at once,
    // as do a window to paint and a timer due, which Windows counts as
    // new whenever they are there.
    let changed = user(cpu).queue(tid).map_or(0, |q| q.changed) & mask;
    let standing = QueueWait {
        mask: mask & (QS_SENDMESSAGE | QS_PAINT | QS_TIMER),
        ..wait.clone()
    };
    if changed != 0 || (standing.mask != 0 && wait_ready(&cpu.bus.win32, tid, &standing, now)) {
        return Some(count);
    }
    let mut objects = Vec::new();
    for i in 0..count {
        let handle = cpu.bus.guest_read_32(handles + 4 * i);
        match crate::win32::object::resolve(cpu, handle) {
            Some(id) => objects.push(id),
            None => {
                set_error(cpu, super::ERROR_INVALID_HANDLE);
                return Some(0xFFFF_FFFF);
            }
        }
    }
    let mut w = Wait {
        objects,
        all,
        deadline: None,
        timeout_result: thread::WAIT_TIMEOUT,
        ..Wait::default()
    };
    if let Some(result) = thread::wait_now(cpu, &w) {
        return Some(result);
    }
    if timeout == 0 {
        return Some(thread::WAIT_TIMEOUT);
    }
    if timeout != 0xFFFF_FFFF {
        w.deadline = Some(now + timeout as u64 * 1000);
    }
    w.queue = Some(wait);
    thread::block(cpu, w);
    None
}

/// The running thread looks for input and finds none, or waits for it:
/// its process is idle. Its input idle event is signaled, which ends
/// WaitForInputIdle in the process that started it. As in Wine, it stays
/// signaled after that, and getting a timer's message counts too. Unlike
/// Wine, so does a PeekMessageA that finds nothing, as on older Windows
/// (the answers Wine's tests take as theirs) and ReactOS: else a program
/// that only peeks would make WinExec wait out its time-out.
fn input_idle(cpu: &mut Cpu) {
    let Some(id) = crate::win32::running(cpu).and_then(|p| p.idle_event) else {
        return;
    };
    let win32 = &mut cpu.bus.win32;
    match win32.objects.get_mut(id).map(|o| &mut o.kind) {
        Some(Kind::Event { signaled, .. }) if !*signaled => *signaled = true,
        _ => return,
    }
    // A thread that waits for it goes on now, not at the next tick.
    let waited = win32
        .processes
        .iter()
        .flat_map(|p| &p.threads)
        .any(|t| t.wait.as_ref().is_some_and(|w| w.objects.contains(&id)));
    if waited {
        thread::wake(cpu);
    }
}

const WAIT_FAILED: u32 = 0xFFFF_FFFF;
const INFINITE: u32 = 0xFFFF_FFFF;

/// The variables of WaitForInputIdle's task: the process's handle, a
/// handle of its idle event, the time-out and when the wait started, in
/// milliseconds.
const IDLE_PROCESS: usize = 0;
const IDLE_EVENT: usize = 1;
const IDLE_MS: usize = 2;
const IDLE_START: usize = 3;

/// WaitForInputIdle(process, ms): wait until the process is idle (see
/// `input_idle`) or ends, handling the messages other threads send
/// meanwhile, as Wine does. 0 then, or WAIT_TIMEOUT once `ms` passed.
/// WAIT_FAILED at once for a process that has no idle event, a console
/// one or one that ended, and for a handle that isn't a process's.
pub fn wait_for_input_idle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, ms] = args else { unreachable!() };
    let Some(Kind::Process { id, .. }) = object::kind_of(cpu, handle) else {
        set_error(cpu, super::ERROR_INVALID_HANDLE);
        return Some(WAIT_FAILED);
    };
    let processes = &cpu.bus.win32.processes;
    let Some(idle) = processes
        .iter()
        .find(|p| p.id == id)
        .and_then(|p| p.idle_event)
    else {
        return Some(WAIT_FAILED);
    };
    object::objects(cpu).hold(idle);
    let event = object::new_handle(cpu, idle);
    let start = now_ms(cpu);
    task::start(cpu, TASK_IDLE, vec![handle, event, ms, start], 8)
}

/// WaitForInputIdle's steps: MsgWaitForMultipleObjects for the process
/// and its idle event, and for messages sent to the thread, which it
/// handles before it waits on.
pub fn idle_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    loop {
        match t.step {
            0 => {
                let ms = t.var(IDLE_MS);
                let mut left = ms;
                if ms != INFINITE {
                    let elapsed = now_ms(cpu).wrapping_sub(t.var(IDLE_START));
                    if elapsed > ms {
                        return idle_done(cpu, t, thread::WAIT_TIMEOUT);
                    }
                    left = ms - elapsed;
                }
                write_words(cpu, t.scratch, &[t.var(IDLE_PROCESS), t.var(IDLE_EVENT)]);
                let args = [2, t.scratch, FALSE, left, QS_SENDMESSAGE];
                return Next::call(address_of("MsgWaitForMultipleObjects"), &args, 1);
            }
            1 => match value {
                2 => {
                    if let Some(next) = handle_sent(cpu, 2) {
                        return next;
                    }
                    // Nothing was sent after all: that it changed no
                    // longer ends the wait at once.
                    let tid = current_tid(cpu);
                    if let Some(q) = user(cpu).queues.iter_mut().find(|q| q.tid == tid) {
                        q.changed &= !QS_SENDMESSAGE;
                    }
                    t.step = 0;
                }
                thread::WAIT_TIMEOUT | WAIT_FAILED => return idle_done(cpu, t, value),
                _ => return idle_done(cpu, t, 0),
            },
            2 => {
                finish_sent(cpu, value);
                let tid = current_tid(cpu);
                let q = user(cpu).queue_mut(tid);
                if q.waiting.last() == Some(&(0, 0)) {
                    q.waiting.pop();
                }
                t.step = 0;
            }
            _ => return idle_done(cpu, t, 0),
        }
    }
}

/// WaitForInputIdle is done: it returns `result`, having closed its
/// handle of the idle event.
fn idle_done(cpu: &mut Cpu, t: &Task, result: u32) -> Next {
    object::close(cpu, t.var(IDLE_EVENT));
    Next::Done(result)
}

/// The smallest time between a timer's messages, in milliseconds.
const MIN_ELAPSE: u32 = 10;

/// SetTimer(hwnd, id, elapse, proc): the timer's id.
pub fn set_timer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    timer(cpu, args, false)
}

pub fn set_system_timer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    timer(cpu, args, true)
}

fn timer(cpu: &mut Cpu, args: &[u32], system: bool) -> Option<u32> {
    let &[hwnd, id, elapse, proc] = args else {
        unreachable!()
    };
    let tid = current_tid(cpu);
    let now = cpu.bus.clock.now_micros();
    let state = user(cpu);
    if hwnd != 0 {
        let Some(w) = state.window(hwnd) else {
            set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
            return Some(0);
        };
        if w.tid != tid {
            set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
            return Some(0);
        }
    }
    let elapse = elapse.clamp(MIN_ELAPSE, 0x7FFF_FFFF);
    let mine = |t: &Timer| t.hwnd == 0 && t.id == id && t.tid == tid && t.system == system;
    let id = if hwnd == 0 && (id == 0 || !state.timers.iter().any(mine)) {
        // A timer without a window gets an id of its own, unless `id` is
        // one of the thread's, which it replaces.
        let mut new = 1;
        while state.timers.iter().any(|t| t.hwnd == 0 && t.id == new) {
            new += 1;
        }
        new
    } else {
        id
    };
    let timer = Timer {
        hwnd,
        id,
        tid,
        elapse,
        due: now + elapse as u64 * 1000,
        proc,
        system,
    };
    match state
        .timers
        .iter_mut()
        .find(|t| t.hwnd == hwnd && t.id == id && t.system == system && (hwnd != 0 || t.tid == tid))
    {
        Some(t) => *t = timer,
        None => state.timers.push(timer),
    }
    // A wait for messages wakes in time for it.
    thread::wake(cpu);
    // With a window, a timer whose id is 0 still returns nonzero.
    Some(if hwnd == 0 || id != 0 { id } else { 1 })
}

/// KillTimer(hwnd, id).
pub fn kill_timer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    kill(cpu, args[0], args[1], false)
}

pub fn kill_system_timer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    kill(cpu, args[0], args[1], true)
}

fn kill(cpu: &mut Cpu, hwnd: u32, id: u32, system: bool) -> Option<u32> {
    let tid = current_tid(cpu);
    let state = user(cpu);
    let before = state.timers.len();
    state.timers.retain(|t| {
        !(t.hwnd == hwnd && t.id == id && t.system == system && (hwnd != 0 || t.tid == tid))
    });
    if state.timers.len() == before {
        return Some(FALSE);
    }
    // Its WM_TIMER messages already posted stay; none are, here.
    Some(TRUE)
}

/// The timers of a window that goes stop.
pub fn kill_window_timers(cpu: &mut Cpu, hwnd: u32) {
    let state = user(cpu);
    state.timers.retain(|t| t.hwnd != hwnd);
    for q in state.queues.iter_mut() {
        q.posted.retain(|m| m.hwnd != hwnd);
        q.input.retain(|m| m.hwnd != hwnd);
    }
}

crate::state_fields!(Msg {
    hwnd,
    message,
    wparam,
    lparam,
    time,
    x,
    y
});
crate::state_fields!(Sent {
    id,
    from,
    hwnd,
    message,
    wparam,
    lparam,
    kind,
    callback
});
crate::state_fields!(Callback {
    proc,
    hwnd,
    message,
    data,
    result
});
crate::state_fields!(Queue {
    tid,
    posted,
    input,
    sent,
    callbacks,
    quit,
    replies,
    waiting,
    in_send,
    changed,
    last_time,
    last_pos,
    activation
});
crate::state_fields!(Timer {
    hwnd,
    id,
    tid,
    elapse,
    due,
    proc,
    system
});
crate::state_fields!(Filter { hwnd, min, max });
crate::state_fields!(QueueWait {
    mask,
    filter,
    reply
});

#[cfg(test)]
mod tests {
    use super::super::input;
    use super::*;

    /// A desktop with one window, which has the capture: the thread that
    /// gets its mouse messages.
    fn captured(cpu: &mut Cpu) -> u32 {
        let hwnd = user(cpu).new_window(super::super::window::Window::default());
        let hwnd = hwnd.expect("a window");
        user(cpu).desktop = true;
        user(cpu).input.capture = hwnd;
        user(cpu).window(hwnd).map(|w| w.tid).unwrap_or(0)
    }

    /// The next message for `tid`, taken: its message and lParam.
    fn take(cpu: &mut Cpu, tid: u32) -> Option<(u32, u32)> {
        take_message(cpu, tid, Filter::default(), true).map(|m| (m.message, m.lparam))
    }

    /// A program that puts the cursor back in the middle after each move
    /// (Xenocracy's dialogs in flight) gets the move the mouse made while
    /// it drew its frame, not the one its SetCursorPos queued.
    #[test]
    fn a_move_between_frames_outlives_set_cursor_pos() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        let tid = captured(&mut cpu);
        input::set_cursor_pos(&mut cpu, &[100, 100]);
        assert_eq!(take(&mut cpu, tid), Some((WM_MOUSEMOVE, 100 << 16 | 100)));
        input::set_cursor_pos(&mut cpu, &[100, 100]);
        // The frame: the mouse moves before the program asks again.
        cpu.bus.mouse.move_by(50.0, 0.0);
        assert_eq!(take(&mut cpu, tid), Some((WM_MOUSEMOVE, 100 << 16 | 150)));
        assert_eq!(take(&mut cpu, tid), None);
    }

    /// A click the mouse made before the program put the cursor back is
    /// where the mouse was, not where the cursor went.
    #[test]
    fn a_click_before_set_cursor_pos_stays_where_it_was() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        let tid = captured(&mut cpu);
        input::set_cursor_pos(&mut cpu, &[100, 100]);
        assert_eq!(take(&mut cpu, tid), Some((WM_MOUSEMOVE, 100 << 16 | 100)));
        cpu.bus.mouse.move_by(50.0, 0.0);
        cpu.bus.mouse.button_down(0);
        input::set_cursor_pos(&mut cpu, &[100, 100]);
        assert_eq!(take(&mut cpu, tid), Some((WM_MOUSEMOVE, 100 << 16 | 150)));
        assert_eq!(take(&mut cpu, tid), Some((WM_LBUTTONDOWN, 100 << 16 | 150)));
        assert_eq!(take(&mut cpu, tid), Some((WM_MOUSEMOVE, 100 << 16 | 100)));
        assert_eq!(take(&mut cpu, tid), None);
    }

    /// A thread that handles a message another thread sent, and sends
    /// one back to that thread from its window procedure, waits for the
    /// reply without taking the message it handles again: Windows took
    /// it off the queue when the thread got it. Sierra's ISDEL.EXE does
    /// this when INSTALL.EXE broadcasts that it ends; taken again, the
    /// message called the window procedure until the stack ran out.
    #[test]
    fn a_message_being_handled_is_not_taken_again() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        let me = current_tid(&cpu);
        let sent = |id| Sent {
            id,
            from: 7,
            hwnd: 0x418,
            message: 0x1D96,
            ..Sent::default()
        };
        user(&mut cpu).queue_mut(me).sent.push_back(sent(1));
        let send_wait = QueueWait {
            mask: QS_SENDMESSAGE,
            filter: Filter::default(),
            reply: 2,
        };
        user(&mut cpu).queue_mut(me).changed |= QS_SENDMESSAGE;
        assert!(wait_ready(&cpu.bus.win32, me, &send_wait, 0));
        assert!(handle_sent(&mut cpu, 1).is_some());
        // Its window procedure sends to thread 7 and waits for the reply:
        // nothing else was sent to it, and nothing counts as new.
        assert!(!wait_ready(&cpu.bus.win32, me, &send_wait, 0));
        assert_eq!(user(&mut cpu).queue_mut(me).changed & QS_SENDMESSAGE, 0);
        assert!(handle_sent(&mut cpu, 1).is_none());
        // A message sent meanwhile is handled within the wait.
        user(&mut cpu).queue_mut(me).sent.push_back(sent(3));
        assert!(wait_ready(&cpu.bus.win32, me, &send_wait, 0));
        assert!(handle_sent(&mut cpu, 1).is_some());
        finish_sent(&mut cpu, 0);
        finish_sent(&mut cpu, 5);
        let q = user(&mut cpu).queue_mut(me);
        assert!(q.sent.is_empty() && q.in_send.is_empty());
        let replies = &user(&mut cpu).queue_mut(7).replies;
        assert_eq!(replies, &vec![(3, 0), (1, 5)]);
    }

    /// A message the thread took off its queue is still the one
    /// InSendMessage and ReplyMessage see, and its sender still gets a
    /// reply when the thread ends while it handles it.
    #[test]
    fn a_message_being_handled_is_still_replied_to() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        let me = current_tid(&cpu);
        let sent = |id| Sent {
            id,
            from: 7,
            hwnd: 0x418,
            ..Sent::default()
        };
        user(&mut cpu).queue_mut(me).sent.push_back(sent(1));
        assert!(handle_sent(&mut cpu, 1).is_some());
        assert_eq!(in_send_message(&mut cpu, &[]), Some(TRUE));
        assert_eq!(reply_message(&mut cpu, &[9]), Some(TRUE));
        // A second ReplyMessage, and the window procedure's own result,
        // go nowhere.
        assert_eq!(reply_message(&mut cpu, &[10]), Some(FALSE));
        finish_sent(&mut cpu, 11);
        assert_eq!(in_send_message(&mut cpu, &[]), Some(FALSE));
        // The thread ends while it handles the next one.
        user(&mut cpu).queue_mut(me).sent.push_back(sent(2));
        assert!(handle_sent(&mut cpu, 1).is_some());
        thread_ended(&mut cpu.bus.win32, me);
        assert!(user(&mut cpu).queue_mut(me).in_send.is_empty());
        let replies = &user(&mut cpu).queue_mut(7).replies;
        assert_eq!(replies, &vec![(1, 9), (2, 0)]);
    }

    /// A callback that another thread sent (SendMessageCallbackA) is a sent
    /// message, so InSendMessage is TRUE while its window procedure runs,
    /// as MSDN describes InSendMessage. The callback itself runs after it,
    /// when the reply is finished, and InSendMessage is FALSE again.
    #[test]
    fn a_cross_thread_callback_message_is_in_send() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        let me = current_tid(&cpu);
        let sent = Sent {
            id: 1,
            from: 7,
            hwnd: 0x418,
            kind: 2,
            callback: (0x1000, 0x2000),
            ..Sent::default()
        };
        user(&mut cpu).queue_mut(me).sent.push_back(sent);
        assert!(handle_sent(&mut cpu, 1).is_some());
        assert_eq!(in_send_message(&mut cpu, &[]), Some(TRUE));
        finish_sent(&mut cpu, 2);
        assert_eq!(in_send_message(&mut cpu, &[]), Some(FALSE));
    }

    /// GetQueueStatus and GetInputState see a move the mouse made since
    /// the devices were last read, as a program asks before it peeks.
    #[test]
    fn queue_status_sees_a_fresh_move() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        captured(&mut cpu);
        assert_eq!(get_input_state(&mut cpu, &[]), Some(0));
        cpu.bus.mouse.move_by(10.0, 0.0);
        let status = get_queue_status(&mut cpu, &[QS_MOUSEMOVE]);
        assert_eq!(status, Some(QS_MOUSEMOVE << 16 | QS_MOUSEMOVE));
        assert_eq!(get_input_state(&mut cpu, &[]), Some(1));
    }
}
