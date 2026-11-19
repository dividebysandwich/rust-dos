//! Drop targets: RegisterDragDrop and RevokeDragDrop, as Windows 98's
//! OLE32 has them.
//!
//! RegisterDragDrop keeps the window's IDropTarget, with a reference of
//! its own, in the window's property "OleDropTargetInterface", where
//! Windows keeps it too and programs read it with GetProp. Windows 98
//! puts the registering thread's OLE window beside it, in
//! "OleDropTargetMarshalHwnd", and its RevokeDragDrop fails without a
//! window there of a thread it can reach. Here a `Registration` stands
//! for that window: it says which thread registered the target. The
//! emulator has no such windows, so that property isn't there.
//!
//! A window destroyed while registered takes the property with it, and
//! the target keeps the reference, as on Windows. OleUninitialize
//! leaves the targets registered too.
//!
//! Nothing drags over the emulator's windows yet (DoDragDrop isn't
//! there), so a registered target is never called but for AddRef and
//! Release.

use super::compobj::{self, ADD_REF, RELEASE, RPC_E_WRONG_THREAD};
use super::state;
use crate::cpu::Cpu;
use crate::win32::com::{CO_E_NOTINITIALIZED, E_INVALIDARG, E_OUTOFMEMORY, S_OK};
use crate::win32::user32::message::thread_alive;
use crate::win32::user32::{current_pid, current_tid, user};

pub const DRAGDROP_E_NOTREGISTERED: u32 = 0x8004_0100;
pub const DRAGDROP_E_ALREADYREGISTERED: u32 = 0x8004_0101;
pub const DRAGDROP_E_INVALIDHWND: u32 = 0x8004_0102;

/// The property that holds a window's drop target, as `prop_key` of
/// USER32 stores names: in capitals.
const PROP: &str = "OLEDROPTARGETINTERFACE";

/// The thread that registered a window's drop target.
#[derive(Clone, Debug, Default)]
pub struct Registration {
    hwnd: u32,
    tid: u32,
    pid: u32,
}

/// Drag and drop's state.
#[derive(Clone, Debug, Default)]
pub struct DropTargets {
    /// The processes a thread of which called OleInitialize. Windows
    /// 98's OLE32 keeps what that sets up until the process ends, after
    /// the last OleUninitialize too, and RegisterDragDrop fails with
    /// E_OUTOFMEMORY in a process that never had it.
    ole_processes: Vec<u32>,
    registrations: Vec<Registration>,
}

/// A thread of the running process called OleInitialize.
pub fn ole_initialized(cpu: &mut Cpu) {
    let pid = current_pid(cpu);
    let drag_drop = &mut state(cpu).drag_drop;
    if !drag_drop.ole_processes.contains(&pid) {
        drag_drop.ole_processes.push(pid);
    }
}

/// A process ended: what OleInitialize set up for it and the targets it
/// registered go.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    let drag_drop = &mut state(cpu).drag_drop;
    drag_drop.ole_processes.retain(|&p| p != pid);
    drag_drop.registrations.retain(|r| r.pid != pid);
}

/// Where the drop target is among the window's properties: a property
/// of the name that holds 0 is no target.
fn target_index(props: &[(String, u32)]) -> Option<usize> {
    props.iter().position(|(k, v)| k == PROP && *v != 0)
}

/// RegisterDragDrop(hwnd, target), with Windows 98's checks in its
/// order: E_INVALIDARG for no target, DRAGDROP_E_INVALIDHWND for
/// something that isn't a window, E_OUTOFMEMORY in a process no thread
/// of which called OleInitialize (CoInitialize isn't enough),
/// DRAGDROP_E_ALREADYREGISTERED for a window that has a target, and
/// CO_E_NOTINITIALIZED for a thread that hasn't initialized COM. A
/// window of another thread or process can be registered.
pub fn register_drag_drop(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [hwnd, drop_target] = args[..] else {
        return Some(E_INVALIDARG);
    };
    if drop_target == 0 {
        return Some(E_INVALIDARG);
    }
    let Some(w) = user(cpu).window(hwnd) else {
        return Some(DRAGDROP_E_INVALIDHWND);
    };
    let registered = target_index(&w.props).is_some();
    let (tid, pid) = (current_tid(cpu), current_pid(cpu));
    if !state(cpu).drag_drop.ole_processes.contains(&pid) {
        return Some(E_OUTOFMEMORY);
    }
    if registered {
        return Some(DRAGDROP_E_ALREADYREGISTERED);
    }
    if !compobj::initialized(cpu) {
        return Some(CO_E_NOTINITIALIZED);
    }
    let w = user(cpu).window_mut(hwnd)?;
    w.props.retain(|(k, _)| k != PROP);
    w.props.insert(0, (PROP.to_string(), drop_target));
    // Registrations of windows destroyed since go, so that a new window
    // given one's handle isn't taken for it.
    let win32 = &mut cpu.bus.win32;
    let user = &win32.user;
    let registrations = &mut win32.ole.drag_drop.registrations;
    registrations.retain(|r| r.hwnd != hwnd && user.is_window(r.hwnd));
    registrations.push(Registration { hwnd, tid, pid });
    compobj::call_each(cpu, S_OK, ADD_REF, std::iter::once(drop_target))
}

/// RevokeDragDrop(hwnd): DRAGDROP_E_INVALIDHWND for something that
/// isn't a window, DRAGDROP_E_NOTREGISTERED for a window without a
/// target. Otherwise the property goes, and the target is released
/// where the thread that registered it can be reached: in this process
/// and still running. Any thread of the process may revoke it then,
/// after OleUninitialize too. Where the thread can't be reached, or the
/// property was set by other means than RegisterDragDrop, Windows 98
/// returns RPC_E_WRONG_THREAD and keeps the reference.
pub fn revoke_drag_drop(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hwnd = args[0];
    let Some(w) = user(cpu).window_mut(hwnd) else {
        return Some(DRAGDROP_E_INVALIDHWND);
    };
    let Some(i) = target_index(&w.props) else {
        return Some(DRAGDROP_E_NOTREGISTERED);
    };
    let drop_target = w.props.remove(i).1;
    let pid = current_pid(cpu);
    let registrations = &mut state(cpu).drag_drop.registrations;
    let registration = registrations
        .iter()
        .position(|r| r.hwnd == hwnd)
        .map(|i| registrations.remove(i));
    let reachable =
        registration.is_some_and(|r| r.pid == pid && thread_alive(&cpu.bus.win32, r.tid));
    if !reachable {
        return Some(RPC_E_WRONG_THREAD);
    }
    compobj::call_each(cpu, S_OK, RELEASE, std::iter::once(drop_target))
}

crate::state_fields!(Registration { hwnd, tid, pid });
crate::state_fields!(DropTargets {
    ole_processes,
    registrations
});
