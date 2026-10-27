//! Program Manager's window, which answers the DDE messages setup
//! programs send without DDEML, as Windows 98's Explorer answers them
//! for Program Manager: WM_DDE_INITIATE broadcast for the application
//! and topic "PROGMAN", then WM_DDE_EXECUTE with the commands that make
//! groups and items (SHELL32's progman.rs), WM_DDE_REQUEST for the list
//! of groups or a group's items, and WM_DDE_TERMINATE.
//!
//! The window is made the first time a program broadcasts
//! WM_DDE_INITIATE. It is the system's, of no thread: a message sent to
//! it is handled in the sender's thread, and one posted to it is handled
//! as it is posted, by the poster, whose window gets the reply in its
//! queue. A 16-bit program's messages have Windows 3.1's lParam, the
//! global handle and the atom or status in its two words; a 32-bit
//! program's have Win32's, WM_DDE_ACK's and WM_DDE_DATA's two values in
//! a global block (PackDDElParam). Their atoms are in the global atom
//! table of their kind, Win16's or KERNEL32's, which are apart here.
//! The other DDE messages (WM_DDE_POKE, WM_DDE_ADVISE and
//! WM_DDE_UNADVISE) get a negative WM_DDE_ACK, and the topics Explorer
//! has besides Program Manager's (the "Shell" and "AppProperties"
//! topics, the "Folders" application) get no answer.

use super::class::{self, CS_GLOBALCLASS, Class};
use super::message::{self, Msg};
use super::window::{self, HWND_BOTTOM, LINKED, WS_POPUP, Window};
use super::{DESKTOP, address_of, current_tid, user};
use crate::cpu::Cpu;
use crate::win32::task::{self, Next, Task};

use super::ddeml::{
    CF_TEXT, DDE_FACK, WM_DDE_ACK, WM_DDE_ADVISE, WM_DDE_DATA, WM_DDE_EXECUTE, WM_DDE_POKE,
};

pub const WM_DDE_INITIATE: u32 = 0x3E0;
pub const WM_DDE_TERMINATE: u32 = 0x3E1;
const WM_DDE_UNADVISE: u32 = 0x3E3;
const WM_DDE_REQUEST: u32 = 0x3E6;

/// DDEDATA's flags for data answering a request: fResponse, and
/// fRelease, the client freeing it.
const DDE_FRESPONSE: u16 = 0x1000;
const DDE_FRELEASE: u16 = 0x2000;
const GMEM_MOVEABLE: u32 = 0x0002;
const GMEM_DDESHARE: u32 = 0x2000;
/// The most bytes of commands read from an execute's block.
const MAX_COMMANDS: u32 = 0x1_0000;

/// The window's class and title, as Explorer's Program Manager window
/// has them.
pub const CLASS: &str = "Progman";
pub const TITLE: &[u8] = b"Program Manager";
/// The application and topic it answers to.
const NAME: &str = "PROGMAN";

fn proc_address() -> u32 {
    address_of("ProgmanWndProc")
}

/// Whether `w` is Program Manager's window.
pub fn is_server(w: &Window) -> bool {
    w.pid == 0 && w.proc == proc_address()
}

/// Program Manager's window, made if it isn't there yet: its handle, or
/// 0 when there is no room for it.
pub fn server(cpu: &mut Cpu) -> u32 {
    super::init(cpu);
    let state = user(cpu);
    let found = state
        .top_levels()
        .into_iter()
        .find(|&h| state.window(h).is_some_and(is_server));
    if let Some(hwnd) = found {
        return hwnd;
    }
    let proc = proc_address();
    let class = match class::find_system(state, CLASS) {
        Some(c) => c,
        None => {
            let atom = class::atom(state, CLASS);
            state.classes.push(Some(Class {
                atom,
                name: CLASS.to_string(),
                style: CS_GLOBALCLASS,
                proc,
                instance: super::DLL.base,
                global: true,
                system: true,
                ..Class::default()
            }));
            state.classes.len() as u32 - 1
        }
    };
    let window = Window {
        class,
        style: WS_POPUP,
        parent: DESKTOP,
        text: TITLE.to_vec(),
        instance: super::DLL.base,
        proc,
        flags: LINKED,
        icon_pos: (-1, -1),
        ..Window::default()
    };
    let Some(hwnd) = state.new_window(window) else {
        return 0;
    };
    if let Some(c) = state.classes[class as usize].as_mut() {
        c.windows += 1;
    }
    window::link(state, hwnd, HWND_BOTTOM);
    hwnd
}

/// ProgmanWndProc(hwnd, message, wParam, lParam): the window's
/// procedure, for the messages sent to it.
pub fn wnd_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, msg, wp, lp] = args else {
        return Some(0);
    };
    match msg {
        WM_DDE_INITIATE => initiate(cpu, hwnd, wp, lp),
        WM_DDE_TERMINATE..=WM_DDE_EXECUTE => posted(cpu, hwnd, msg, wp, lp),
        _ => super::defwnd::def_window_proc(cpu, args),
    }
}

/// The name of the global atom `a`, from the table of 16-bit programs'
/// atoms or of 32-bit ones'.
fn atom_name(cpu: &mut Cpu, a: u32, sixteen: bool) -> Option<String> {
    if sixteen {
        crate::win16::atom_name(cpu, a as u16)
    } else {
        crate::win32::kernel32::atom::name_of(cpu, a as u16)
    }
}

/// A global atom of `name` for a 16-bit or 32-bit program.
fn add_atom(cpu: &mut Cpu, name: &str, sixteen: bool) -> u32 {
    if sixteen {
        crate::win16::atom_of(cpu, name.to_string(), true) as u32
    } else {
        crate::win32::kernel32::atom::add(cpu, name) as u32
    }
}

/// Whether the name the atom `a` has is Program Manager's; 0 stands for
/// any.
fn is_progman(cpu: &mut Cpu, a: u32, sixteen: bool) -> bool {
    a == 0 || atom_name(cpu, a, sixteen).is_some_and(|n| n.eq_ignore_ascii_case(NAME))
}

/// WM_DDE_INITIATE from `client` for the application and topic whose
/// atoms `lp` has: Program Manager's (or any) is acknowledged with
/// WM_DDE_ACK, sent back with atoms of its own, as the server sends it.
fn initiate(cpu: &mut Cpu, hwnd: u32, client: u32, lp: u32) -> Option<u32> {
    let sixteen = is_16bit(cpu, client);
    if client == hwnd
        || !user(cpu).is_window(client)
        || !is_progman(cpu, lp & 0xFFFF, sixteen)
        || !is_progman(cpu, lp >> 16, sixteen)
    {
        return Some(0);
    }
    cpu.bus.log_string(&format!(
        "[W32] Program Manager: DDE conversation with {:04X}",
        client
    ));
    let app = add_atom(cpu, NAME, sixteen);
    let topic = add_atom(cpu, NAME, sixteen);
    let vars = vec![client, WM_DDE_ACK, hwnd, app | topic << 16];
    task::start(cpu, task::PROGMAN, vars, 0)
}

/// The task that sends a message: variables 0 to 3 its window, message,
/// wParam and lParam.
pub fn step(_: &mut Cpu, t: &mut Task, _value: u32) -> Next {
    if t.step == 0 {
        let args = [t.var(0), t.var(1), t.var(2), t.var(3)];
        return Next::call(address_of("SendMessageA"), &args, 1);
    }
    Next::Done(0)
}

/// Whether the window `client` is a 16-bit program's, whose DDE
/// messages have Windows 3.1's lParam.
fn is_16bit(cpu: &mut Cpu, client: u32) -> bool {
    let tid = match user(cpu).window(client) {
        Some(w) => w.tid,
        None => current_tid(cpu),
    };
    crate::win16::is_task(&cpu.bus.win32, tid)
}

/// The two values of a DDE message's lParam that has them in a global
/// block for a 32-bit program, which the block is freed of.
fn unpack(cpu: &mut Cpu, lp: u32) -> (u32, u32) {
    let Some(values) = super::ddeml::unpacked(cpu, lp) else {
        return (0, 0);
    };
    crate::win32::kernel32::global_free(cpu, lp);
    values
}

/// Post `msg` from the server to `client`, its lParam `lo` and `hi`
/// as a 16-bit or a 32-bit program takes them.
fn reply(cpu: &mut Cpu, server: u32, client: u32, msg: u32, lo: u32, hi: u32, sixteen: bool) {
    let Some(tid) = user(cpu).window(client).map(|w| w.tid) else {
        return;
    };
    let lp = if sixteen || !matches!(msg, WM_DDE_ACK | WM_DDE_DATA) {
        (lo & 0xFFFF) | hi << 16
    } else {
        super::ddeml::pack_dde_lparam(cpu, &[msg, lo, hi]).unwrap_or(0)
    };
    let msg = Msg {
        hwnd: client,
        message: msg,
        wparam: server,
        lparam: lp,
        time: message::now_ms(cpu),
        ..Msg::default()
    };
    message::post(cpu, tid, msg);
}

/// The commands of an execute's global block `handle`, up to their
/// zero.
fn commands(cpu: &mut Cpu, handle: u32, sixteen: bool) -> Option<String> {
    let bytes = if sixteen {
        crate::win16::global_bytes(cpu, handle as u16)?
    } else {
        let at = crate::win32::kernel32::global_address(cpu, handle);
        let size = crate::win32::kernel32::global_size(cpu, handle).min(MAX_COMMANDS);
        if at == 0 || size == 0 || !cpu.bus.guest_probe(at, size as usize, false) {
            return None;
        }
        let mut bytes = vec![0u8; size as usize];
        cpu.bus.guest_read_bytes(at, &mut bytes);
        bytes
    };
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    Some(crate::dosstr::from_bytes(&bytes[..end]))
}

/// A global block of `bytes` for `client` to free: its handle, or 0.
fn block(cpu: &mut Cpu, bytes: &[u8], sixteen: bool) -> u32 {
    if sixteen {
        return crate::win16::global_block_of(cpu, bytes) as u32;
    }
    let flags = GMEM_MOVEABLE | GMEM_DDESHARE;
    let handle = crate::win32::kernel32::global_alloc(cpu, flags, bytes.len() as u32);
    let at = crate::win32::kernel32::global_address(cpu, handle);
    if at == 0 {
        if handle != 0 {
            crate::win32::kernel32::global_free(cpu, handle);
        }
        return 0;
    }
    cpu.bus.guest_write_bytes(at, bytes);
    handle
}

/// A DDE message `client` posted (or sent) to the server `hwnd`: what
/// PostMessageA returns.
pub fn posted(cpu: &mut Cpu, hwnd: u32, msg: u32, client: u32, lp: u32) -> Option<u32> {
    let sixteen = is_16bit(cpu, client);
    match msg {
        WM_DDE_EXECUTE => {
            let handle = if sixteen { lp >> 16 } else { lp };
            let done = match commands(cpu, handle, sixteen) {
                Some(text) => crate::win32::shell32::progman_execute(cpu, &text),
                None => None,
            };
            let status = if done.is_some() { DDE_FACK } else { 0 };
            reply(cpu, hwnd, client, WM_DDE_ACK, status, handle, sixteen);
            match done {
                Some(windows) => super::ddeml::open_windows(cpu, windows),
                None => Some(1),
            }
        }
        WM_DDE_REQUEST => {
            // A client that is gone would never free the data's block.
            if !user(cpu).is_window(client) {
                return Some(1);
            }
            let (format, item) = (lp & 0xFFFF, lp >> 16);
            let name = atom_name(cpu, item, sixteen).unwrap_or_default();
            let text = match format {
                CF_TEXT => crate::win32::shell32::progman_request(cpu, &name),
                _ => None,
            };
            let data = text.map(|text| {
                let mut bytes = (DDE_FRESPONSE | DDE_FRELEASE).to_le_bytes().to_vec();
                bytes.extend((CF_TEXT as u16).to_le_bytes());
                bytes.extend(crate::dosstr::to_bytes(&text));
                bytes.push(0);
                block(cpu, &bytes, sixteen)
            });
            match data {
                Some(h) if h != 0 => reply(cpu, hwnd, client, WM_DDE_DATA, h, item, sixteen),
                _ => reply(cpu, hwnd, client, WM_DDE_ACK, 0, item, sixteen),
            }
            Some(1)
        }
        WM_DDE_ADVISE | WM_DDE_POKE | WM_DDE_UNADVISE => {
            let item = if sixteen || msg == WM_DDE_UNADVISE {
                lp >> 16
            } else {
                unpack(cpu, lp).1
            };
            reply(cpu, hwnd, client, WM_DDE_ACK, 0, item, sixteen);
            Some(1)
        }
        WM_DDE_TERMINATE => {
            reply(cpu, hwnd, client, WM_DDE_TERMINATE, 0, 0, sixteen);
            Some(1)
        }
        // A client's acknowledgement or data: the server asked for none.
        // A 32-bit client's packed lParam is the receiver's to free.
        WM_DDE_ACK | WM_DDE_DATA if !sixteen => {
            unpack(cpu, lp);
            Some(1)
        }
        _ => Some(1),
    }
}
