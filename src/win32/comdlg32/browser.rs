//! The shell objects of the Explorer-style file dialog: the
//! IShellBrowser its WM_GETISHELLBROWSER (WM_USER + 7) gives a hook, and
//! the IShellView its QueryActiveShellView gives. The dialog's view is a
//! list box of COMDLG32's, which no shell view object makes, so these
//! answer what tells a hook about the dialog (its windows) and fail the
//! rest with E_NOTIMPL. The view has no IShellView2, as Windows 95's
//! shell's views have none.

use super::filedlg::{FileDlg, dialog};
use crate::cpu::Cpu;
use crate::win32::com::{self, Guid, IID_IUNKNOWN};

const S_OK: u32 = 0;
const E_NOTIMPL: u32 = 0x8000_4001;
const E_NOINTERFACE: u32 = 0x8000_4002;
const E_POINTER: u32 = 0x8000_4003;

const IID_IOLEWINDOW: Guid = Guid::new(0x0000_0114, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_ISHELLBROWSER: Guid = Guid::new(0x0002_14E2, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_ISHELLVIEW: Guid = Guid::new(0x0002_14E3, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);

/// The vtables' places in COMDLG32's list.
const BROWSER: usize = 0;
const VIEW: usize = 1;

/// Make the dialog's objects, whose id is the dialog's window.
pub fn make(cpu: &mut Cpu, hwnd: u32) -> (u32, u32) {
    let dll = super::dll_index();
    let browser = com::new_interface(cpu, dll, BROWSER, hwnd, 0).unwrap_or(0);
    let view = com::new_interface(cpu, dll, VIEW, hwnd, 0).unwrap_or(0);
    (browser, view)
}

/// Free the dialog's objects as it ends.
pub fn free(cpu: &mut Cpu, d: &FileDlg) {
    for this in [d.browser, d.shell_view] {
        if this != 0 {
            com::free_interface(cpu, this);
        }
    }
}

/// The object `this` is: which vtable, and its dialog.
fn object(cpu: &mut Cpu, this: u32) -> Option<(usize, u32)> {
    com::interface_of(cpu, super::dll_index(), this)
}

fn query(cpu: &mut Cpu, args: &[u32], iids: &[Guid]) -> Option<u32> {
    let [this, riid, out] = args[..] else {
        return Some(E_POINTER);
    };
    let iid = Guid::read(cpu, riid)?;
    if out == 0 {
        return Some(E_POINTER);
    }
    if iid == IID_IUNKNOWN || iid == IID_IOLEWINDOW || iids.contains(&iid) {
        com::write_out(cpu, out, this);
        return Some(S_OK);
    }
    com::write_out(cpu, out, 0);
    Some(E_NOINTERFACE)
}

/// IShellBrowser::QueryInterface(riid, out).
pub fn query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    query(cpu, args, &[IID_ISHELLBROWSER])
}

/// IShellView::QueryInterface(riid, out): no IShellView2.
pub fn view_query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    query(cpu, args, &[IID_ISHELLVIEW])
}

/// AddRef and Release: the objects live as long as the dialog.
pub fn add_ref(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(1)
}

/// IOleWindow::GetWindow(out): the dialog, or the view.
pub fn get_window(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((kind, hwnd)) = object(cpu, args[0]) else {
        return Some(E_POINTER);
    };
    let window = if kind == VIEW {
        dialog(cpu, hwnd).map_or(0, |d| d.view)
    } else {
        hwnd
    };
    com::write_out(cpu, args[1], window);
    Some(S_OK)
}

/// IShellBrowser::GetControlWindow(id, out): the toolbar for FCW_TOOLBAR.
pub fn get_control_window(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((_, hwnd)) = object(cpu, args[0]) else {
        return Some(E_POINTER);
    };
    let window = match args[1] {
        // FCW_TOOLBAR
        2 => dialog(cpu, hwnd).map_or(0, |d| d.toolbar),
        _ => 0,
    };
    com::write_out(cpu, args[2], window);
    Some(if window != 0 { S_OK } else { E_NOTIMPL })
}

/// IShellBrowser::QueryActiveShellView(out).
pub fn query_active_shell_view(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((_, hwnd)) = object(cpu, args[0]) else {
        return Some(E_POINTER);
    };
    let view = dialog(cpu, hwnd).map_or(0, |d| d.shell_view);
    com::write_out(cpu, args[1], view);
    Some(if view != 0 { S_OK } else { E_NOINTERFACE })
}

/// The methods that do nothing and succeed.
pub fn ok(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(S_OK)
}

/// The methods the dialog doesn't have.
pub fn not_impl(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(E_NOTIMPL)
}
