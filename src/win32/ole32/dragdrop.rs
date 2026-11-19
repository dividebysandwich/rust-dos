//! Drop targets: RegisterDragDrop and RevokeDragDrop, as Windows 98's
//! OLE32 has them.
//!
//! RegisterDragDrop keeps the window's IDropTarget, with a reference of
//! its own, in the window's property "OleDropTargetInterface", where
//! Windows keeps it too and programs read it with GetProp.
//! RevokeDragDrop removes the property and releases the target. A
//! window destroyed while registered takes the property with it, and
//! the target keeps the reference, as on Windows.
//!
//! Nothing drags over the emulator's windows yet (DoDragDrop isn't
//! there), so a registered target is never called but for AddRef and
//! Release.

use super::compobj::{self, ADD_REF, RELEASE};
use crate::cpu::Cpu;
use crate::win32::com::{E_INVALIDARG, E_OUTOFMEMORY, S_OK};
use crate::win32::user32::{current_pid, user};

pub const DRAGDROP_E_NOTREGISTERED: u32 = 0x8004_0100;
pub const DRAGDROP_E_ALREADYREGISTERED: u32 = 0x8004_0101;
pub const DRAGDROP_E_INVALIDHWND: u32 = 0x8004_0102;

/// The property that holds a window's drop target, as `prop_key` of
/// USER32 stores names: in capitals.
const PROP: &str = "OLEDROPTARGETINTERFACE";

/// Where the drop target is among the window's properties: a property
/// of the name that holds 0 is no target.
fn target_index(props: &[(String, u32)]) -> Option<usize> {
    props.iter().position(|(k, v)| k == PROP && *v != 0)
}

/// RegisterDragDrop(hwnd, target): E_OUTOFMEMORY for a thread that
/// hasn't called OleInitialize (CoInitialize isn't enough), then
/// E_INVALIDARG for no target, DRAGDROP_E_INVALIDHWND for something
/// that isn't a window of this process and DRAGDROP_E_ALREADYREGISTERED
/// for a window that has a target.
pub fn register_drag_drop(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [hwnd, drop_target] = args[..] else {
        return Some(E_INVALIDARG);
    };
    if !compobj::ole_initialized(cpu) {
        return Some(E_OUTOFMEMORY);
    }
    if drop_target == 0 {
        return Some(E_INVALIDARG);
    }
    let pid = current_pid(cpu);
    let Some(w) = user(cpu).window_mut(hwnd).filter(|w| w.pid == pid) else {
        return Some(DRAGDROP_E_INVALIDHWND);
    };
    if target_index(&w.props).is_some() {
        return Some(DRAGDROP_E_ALREADYREGISTERED);
    }
    w.props.retain(|(k, _)| k != PROP);
    w.props.insert(0, (PROP.to_string(), drop_target));
    compobj::call_each(cpu, S_OK, ADD_REF, std::iter::once(drop_target))
}

/// RevokeDragDrop(hwnd): DRAGDROP_E_INVALIDHWND for something that
/// isn't a window, DRAGDROP_E_NOTREGISTERED for a window without a
/// target. It works after OleUninitialize too.
pub fn revoke_drag_drop(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hwnd = args[0];
    let Some(w) = user(cpu).window_mut(hwnd) else {
        return Some(DRAGDROP_E_INVALIDHWND);
    };
    let Some(i) = target_index(&w.props) else {
        return Some(DRAGDROP_E_NOTREGISTERED);
    };
    let drop_target = w.props.remove(i).1;
    compobj::call_each(cpu, S_OK, RELEASE, std::iter::once(drop_target))
}
