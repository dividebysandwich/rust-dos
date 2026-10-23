//! COMCTL32.DLL: the common controls of Windows 95 and Internet
//! Explorer 3 (COMCTL32 4.70), each a window class with its procedure
//! here, in a module of its own; image lists, the dynamic arrays DPA and
//! DSA, property sheets, and the small helpers setup programs and
//! launchers call (CreateStatusWindowA, CreateToolbarEx,
//! CreateUpDownControl, the MRU lists).
//!
//! The classes are registered with USER32's when USER starts, as
//! COMCTL32 registers them as it loads on Windows 95, with COMCTL32 as
//! their module. What a control keeps of a window is in `State`, by its
//! handle, from WM_NCCREATE or WM_CREATE until WM_NCDESTROY. A window
//! procedure that calls into the program on its way, to notify its
//! parent or to paint through BeginPaint, returns a script (script.rs),
//! which the controls' task runs.
//!
//! The controls follow Wine's comctl32 (LGPL 2.1 or later, whose
//! copyright notices are in each module that follows one of its files)
//! and Microsoft's documentation, as Wine's conformance tests accept
//! Windows 95's and 98's answers.

pub mod dpa;
pub mod gfx;
pub mod header;
pub mod imagelist;
pub mod listview;
pub mod misc;
pub mod mru;
pub mod progress;
pub mod propsheet;
pub mod script;
pub mod status;
pub mod tab;
pub mod toolbar;
pub mod tooltips;
pub mod trackbar;
pub mod treeview;
pub mod updown;

use super::stubs::{Api, Conv, Dll};
use crate::cpu::Cpu;
use crate::win32::user32::{self, class};
use script::{Arg, Ctx, Op, Script};
use std::collections::BTreeMap;

pub static DLL: Dll = Dll {
    name: "COMCTL32.DLL",
    base: 0xBFC4_0000,
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

/// COMCTL32's functions, with the ordinals Windows 98's COMCTL32 has
/// them at; the window procedures (ordinal 0) aren't exported. Append
/// only (see `stubs`).
static APIS: &[Api] = &[
    stdcall("InitCommonControls", 17, 0, misc::init_common_controls),
    stdcall("InitCommonControlsEx", 83, 1, misc::init_common_controls_ex),
    stdcall("DllGetVersion", 25, 1, misc::dll_get_version),
    stdcall("Alloc", 71, 1, misc::alloc),
    stdcall("ReAlloc", 72, 2, misc::realloc),
    stdcall("Free", 73, 1, misc::free),
    stdcall("GetSize", 74, 1, misc::get_size),
    stdcall("Str_GetPtrA", 233, 3, misc::str_get_ptr),
    stdcall("Str_SetPtrA", 234, 2, misc::str_set_ptr),
    stdcall("ProgressWndProc", 0, 4, progress::proc_),
    stdcall("DPA_Create", 328, 1, dpa::dpa_create),
    stdcall("DPA_Destroy", 329, 1, dpa::dpa_destroy),
    stdcall("DPA_Grow", 330, 2, dpa::dpa_grow),
    stdcall("DPA_Clone", 331, 2, dpa::dpa_clone),
    stdcall("DPA_GetPtr", 332, 2, dpa::dpa_get_ptr),
    stdcall("DPA_GetPtrIndex", 333, 2, dpa::dpa_get_ptr_index),
    stdcall("DPA_InsertPtr", 334, 3, dpa::dpa_insert_ptr),
    stdcall("DPA_SetPtr", 335, 3, dpa::dpa_set_ptr),
    stdcall("DPA_DeletePtr", 336, 2, dpa::dpa_delete_ptr),
    stdcall("DPA_DeleteAllPtrs", 337, 1, dpa::dpa_delete_all_ptrs),
    stdcall("DPA_Sort", 338, 3, dpa::dpa_sort),
    stdcall("DPA_Search", 339, 6, dpa::dpa_search),
    stdcall("DPA_CreateEx", 340, 2, dpa::dpa_create_ex),
    stdcall("DPA_EnumCallback", 385, 3, dpa::dpa_enum_callback),
    stdcall("DPA_DestroyCallback", 386, 3, dpa::dpa_destroy_callback),
    stdcall("DSA_Create", 320, 2, dpa::dsa_create),
    stdcall("DSA_Destroy", 321, 1, dpa::dsa_destroy),
    stdcall("DSA_GetItem", 322, 3, dpa::dsa_get_item),
    stdcall("DSA_GetItemPtr", 323, 2, dpa::dsa_get_item_ptr),
    stdcall("DSA_InsertItem", 324, 3, dpa::dsa_insert_item),
    stdcall("DSA_SetItem", 325, 3, dpa::dsa_set_item),
    stdcall("DSA_DeleteItem", 326, 2, dpa::dsa_delete_item),
    stdcall("DSA_DeleteAllItems", 327, 1, dpa::dsa_delete_all_items),
    stdcall("DSA_EnumCallback", 387, 3, dpa::dsa_enum_callback),
    stdcall("DSA_DestroyCallback", 388, 3, dpa::dsa_destroy_callback),
    stdcall("ImageList_Add", 40, 3, imagelist::il_add),
    stdcall("ImageList_AddIcon", 41, 2, imagelist::il_add_icon),
    stdcall("ImageList_AddMasked", 42, 3, imagelist::il_add_masked),
    stdcall("ImageList_BeginDrag", 43, 4, imagelist::il_begin_drag),
    stdcall("ImageList_Copy", 44, 5, imagelist::il_copy),
    stdcall("ImageList_Create", 45, 5, imagelist::il_create),
    stdcall("ImageList_Destroy", 46, 1, imagelist::il_destroy),
    stdcall("ImageList_DragEnter", 47, 3, imagelist::il_drag_enter),
    stdcall("ImageList_DragLeave", 48, 1, imagelist::il_drag_leave),
    stdcall("ImageList_DragMove", 49, 2, imagelist::il_drag_move),
    stdcall("ImageList_DragShowNolock", 50, 1, imagelist::il_drag_show_nolock),
    stdcall("ImageList_Draw", 51, 6, imagelist::il_draw),
    stdcall("ImageList_DrawEx", 52, 10, imagelist::il_draw_ex),
    stdcall("ImageList_DrawIndirect", 53, 1, imagelist::il_draw_indirect),
    stdcall("ImageList_Duplicate", 54, 1, imagelist::il_duplicate),
    stdcall("ImageList_EndDrag", 55, 0, imagelist::il_end_drag),
    stdcall("ImageList_GetBkColor", 56, 1, imagelist::il_get_bk_color),
    stdcall("ImageList_GetDragImage", 57, 2, imagelist::il_get_drag_image),
    stdcall("ImageList_GetIcon", 58, 3, imagelist::il_get_icon),
    stdcall("ImageList_GetIconSize", 59, 3, imagelist::il_get_icon_size),
    stdcall("ImageList_GetImageCount", 60, 1, imagelist::il_get_image_count),
    stdcall("ImageList_GetImageInfo", 61, 3, imagelist::il_get_image_info),
    stdcall("ImageList_GetImageRect", 62, 3, imagelist::il_get_image_rect),
    stdcall("ImageList_LoadImage", 63, 7, imagelist::il_load_image),
    stdcall("ImageList_LoadImageA", 64, 7, imagelist::il_load_image),
    stdcall("ImageList_LoadImageW", 65, 7, imagelist::il_load_image_w),
    stdcall("ImageList_Merge", 66, 6, imagelist::il_merge),
    stdcall("ImageList_Read", 67, 1, imagelist::il_read),
    stdcall("ImageList_Remove", 68, 2, imagelist::il_remove),
    stdcall("ImageList_Replace", 69, 4, imagelist::il_replace),
    stdcall("ImageList_ReplaceIcon", 70, 3, imagelist::il_replace_icon),
    stdcall("ImageList_SetBkColor", 75, 2, imagelist::il_set_bk_color),
    stdcall("ImageList_SetDragCursorImage", 76, 4, imagelist::il_set_drag_cursor_image),
    stdcall("ImageList_SetFilter", 77, 3, imagelist::il_set_filter),
    stdcall("ImageList_SetFlags", 78, 2, imagelist::il_set_filter),
    stdcall("ImageList_SetIconSize", 79, 3, imagelist::il_set_icon_size),
    stdcall("ImageList_SetImageCount", 80, 2, imagelist::il_set_image_count),
    stdcall("ImageList_SetOverlayImage", 81, 3, imagelist::il_set_overlay_image),
    stdcall("ImageList_Write", 82, 2, imagelist::il_write),
    stdcall("UpDownWndProc", 0, 4, updown::proc_),
    stdcall("UpDownBuddyProc", 0, 4, updown::buddy_proc),
    stdcall("CreateUpDownControl", 16, 12, updown::create_up_down_control),
    stdcall("StatusWndProc", 0, 4, status::proc_),
    stdcall("MenuHelp", 2, 7, status::menu_help),
    stdcall("ShowHideMenuCtl", 3, 3, status::show_hide_menu_ctl),
    stdcall("GetEffectiveClientRect", 4, 3, status::get_effective_client_rect),
    stdcall("DrawStatusTextA", 5, 4, status::draw_status_text),
    stdcall("CreateStatusWindowA", 6, 4, status::create_status_window),
    stdcall("CreateStatusWindow", 21, 4, status::create_status_window),
    stdcall("DrawStatusText", 27, 4, status::draw_status_text),
    stdcall("TrackbarWndProc", 0, 4, trackbar::proc_),
    stdcall("TooltipsWndProc", 0, 4, tooltips::proc_),
    stdcall("ToolbarWndProc", 0, 4, toolbar::proc_),
    stdcall("CreateToolbar", 7, 8, toolbar::create_toolbar),
    stdcall("CreateMappedBitmap", 8, 5, toolbar::create_mapped_bitmap),
    stdcall("CreateToolbarEx", 23, 13, toolbar::create_toolbar_ex),
    stdcall("HeaderWndProc", 0, 4, header::proc_),
    stdcall("ListViewWndProc", 0, 4, listview::proc_),
    stdcall("TreeViewWndProc", 0, 4, treeview::proc_),
    stdcall("TabWndProc", 0, 4, tab::proc_),
    stdcall("CreatePropertySheetPage", 18, 1, propsheet::create_property_sheet_page),
    stdcall("CreatePropertySheetPageA", 19, 1, propsheet::create_property_sheet_page),
    stdcall("CreatePropertySheetPageW", 20, 1, propsheet::fail),
    stdcall("DestroyPropertySheetPage", 24, 1, propsheet::destroy_property_sheet_page),
    stdcall("PropertySheet", 86, 1, propsheet::property_sheet),
    stdcall("PropertySheetA", 87, 1, propsheet::property_sheet),
    stdcall("PropertySheetW", 88, 1, propsheet::fail_minus_one),
    stdcall("CreateMRUListA", 151, 1, mru::create_mru_list),
    stdcall("FreeMRUList", 152, 1, mru::free_mru_list),
    stdcall("AddMRUStringA", 153, 2, mru::add_mru_string),
    stdcall("EnumMRUListA", 154, 4, mru::enum_mru_list),
    stdcall("FindMRUStringA", 155, 3, mru::find_mru_string),
    stdcall("DelMRUString", 156, 2, mru::del_mru_string),
    stdcall("CreateMRUListLazyA", 157, 4, mru::create_mru_list_lazy),
    stdcall("AddMRUData", 167, 3, mru::add_mru_data),
    stdcall("FindMRUData", 169, 4, mru::find_mru_data),
    stdcall("TreeViewEditProc", 0, 4, treeview::edit_proc),
];

/// COMCTL32's place in `stubs::DLLS`.
fn dll_index() -> usize {
    super::stubs::DLLS
        .iter()
        .position(|d| d.name == DLL.name)
        .expect("COMCTL32 is a built-in DLL")
}

/// The address of COMCTL32's function `name`: a window procedure, for
/// its class.
pub fn address_of(name: &str) -> u32 {
    let index = APIS
        .iter()
        .position(|a| a.name == name)
        .unwrap_or_else(|| panic!("COMCTL32 has no {}", name));
    super::stubs::stub_address(dll_index(), index)
}

/// Call the built-in DLL `dll`'s function `name` from Rust, as the
/// program would call it: for functions that don't call into the
/// program themselves.
pub fn call_builtin(cpu: &mut Cpu, dll: &str, name: &str, args: &[u32]) -> Option<u32> {
    let d = super::stubs::DLLS
        .iter()
        .find(|d| d.name == dll)
        .unwrap_or_else(|| panic!("no built-in {}", dll));
    let api = d
        .apis
        .iter()
        .find(|a| a.name == name)
        .unwrap_or_else(|| panic!("{} has no {}", dll, name));
    (api.handler)(cpu, args)
}

/// KERNEL32's function `name`, called from Rust.
pub fn kernel32(cpu: &mut Cpu, name: &str, args: &[u32]) -> Option<u32> {
    call_builtin(cpu, "KERNEL32.DLL", name, args)
}

/// The modules whose scripts' continuations `dispatch_then` runs.
pub type Module = u32;
pub const PROGRESS: Module = 1;
pub const DPA: Module = 2;
pub const IMAGELIST: Module = 3;
pub const UPDOWN: Module = 4;
pub const STATUS: Module = 5;
pub const TRACKBAR: Module = 6;
pub const TOOLTIPS: Module = 7;
pub const TOOLBAR: Module = 8;
pub const HEADER: Module = 9;
pub const LISTVIEW: Module = 10;
pub const TREEVIEW: Module = 11;
pub const TAB: Module = 12;
pub const PROPSHEET: Module = 13;
pub const MRU: Module = 14;

/// The continuation that paints: the module's `draw`, with the DC
/// BeginPaint gave.
pub const DRAW: u16 = 0xFFFF;

/// Run the continuation `id` of the module `module`.
pub fn dispatch_then(cpu: &mut Cpu, module: u32, id: u16, c: &mut Ctx) -> Vec<Op> {
    match module {
        PROGRESS => progress::then(cpu, id, c),
        DPA => dpa::then_step(cpu, id, c),
        IMAGELIST => imagelist::then(cpu, id, c),
        UPDOWN => updown::then(cpu, id, c),
        STATUS => status::then(cpu, id, c),
        TRACKBAR => trackbar::then(cpu, id, c),
        TOOLTIPS => tooltips::then(cpu, id, c),
        TOOLBAR => toolbar::then(cpu, id, c),
        HEADER => header::then(cpu, id, c),
        LISTVIEW => listview::then(cpu, id, c),
        TREEVIEW => treeview::then(cpu, id, c),
        TAB => tab::then(cpu, id, c),
        PROPSHEET => propsheet::then(cpu, id, c),
        MRU => mru::then(cpu, id, c),
        _ => Vec::new(),
    }
}

/// The steps of the controls' task (`task::COMCTL32`).
pub fn step(
    cpu: &mut Cpu,
    t: &mut crate::win32::task::Task,
    value: u32,
) -> crate::win32::task::Next {
    script::step(cpu, t, value)
}

/// What the common controls keep, by window handle.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub progress: BTreeMap<u32, progress::Progress>,
    pub imagelists: BTreeMap<u32, imagelist::ImageList>,
    pub drag: imagelist::Drag,
    pub updown: BTreeMap<u32, updown::UpDown>,
    pub buddies: BTreeMap<u32, updown::Buddy>,
    pub status: status::State,
    pub trackbar: trackbar::State,
    pub tooltips: tooltips::State,
    pub toolbar: toolbar::State,
    pub header: header::State,
    pub listview: listview::State,
    pub treeview: treeview::State,
    pub tab: tab::State,
    pub propsheet: propsheet::State,
    pub mru: mru::State,
}

crate::state_fields!(State {
    progress,
    imagelists,
    drag,
    updown,
    buddies,
    status,
    trackbar,
    tooltips,
    toolbar,
    header,
    listview,
    treeview,
    tab,
    propsheet,
    mru,
});

/// COMCTL32's state.
pub fn state(cpu: &mut Cpu) -> &mut State {
    &mut cpu.bus.win32.comctl32
}

/// The classes: name, style, window procedure, extra bytes, cursor (an
/// IDC_ id), background (a COLOR_ index + 1, or 0).
const CLASSES: &[(&str, u32, &str, u32, u32, u32)] = &[
    (
        "msctls_progress32",
        class::CS_VREDRAW | class::CS_HREDRAW,
        "ProgressWndProc",
        4,
        32512,
        0,
    ),
    (
        "msctls_updown32",
        class::CS_VREDRAW | class::CS_HREDRAW,
        "UpDownWndProc",
        4,
        32512,
        16,
    ),
    (
        "msctls_statusbar32",
        class::CS_DBLCLKS | class::CS_VREDRAW | class::CS_HREDRAW,
        "StatusWndProc",
        4,
        32512,
        16,
    ),
    (
        "msctls_trackbar32",
        0,
        "TrackbarWndProc",
        4,
        32512,
        0,
    ),
    (
        "tooltips_class32",
        class::CS_DBLCLKS | class::CS_SAVEBITS,
        "TooltipsWndProc",
        4,
        32512,
        0,
    ),
    (
        "ToolbarWindow32",
        class::CS_DBLCLKS,
        "ToolbarWndProc",
        4,
        32512,
        16,
    ),
    (
        "SysHeader32",
        class::CS_DBLCLKS,
        "HeaderWndProc",
        4,
        32512,
        0,
    ),
    (
        "SysListView32",
        class::CS_DBLCLKS,
        "ListViewWndProc",
        4,
        32512,
        0,
    ),
    (
        "SysTreeView32",
        class::CS_DBLCLKS,
        "TreeViewWndProc",
        4,
        32512,
        0,
    ),
    (
        "SysTabControl32",
        class::CS_DBLCLKS | class::CS_VREDRAW | class::CS_HREDRAW,
        "TabWndProc",
        4,
        32512,
        16,
    ),
];

/// The classes Windows 95's COMCTL32 has that the built-in one doesn't
/// draw yet: windows that draw their background and nothing else, for
/// programs that make one they don't depend on.
const PLACEHOLDERS: &[(&str, u32)] = &[
    ("SysAnimate32", 0),
    ("msctls_hotkey32", 0),
    ("ComboBoxEx32", 0),
];

/// Register the common controls' classes, with USER32's own: USER32's
/// `class::register_system` calls this.
pub fn register_classes(cpu: &mut Cpu) {
    let add = |cpu: &mut Cpu, name: &str, style: u32, proc: u32, extra, cursor, background| {
        let atom = class::atom(user32::user(cpu), name);
        let c = class::Class {
            atom,
            name: name.to_string(),
            style: style | class::CS_GLOBALCLASS,
            proc,
            wnd_extra: extra,
            instance: DLL.base,
            cursor: user32::resource::system_cursor(cpu, cursor),
            background,
            global: true,
            system: true,
            ..class::Class::default()
        };
        user32::user(cpu).classes.push(Some(c));
    };
    for &(name, style, proc, extra, cursor, background) in CLASSES {
        add(
            cpu,
            name,
            style,
            address_of(proc),
            extra,
            cursor,
            background,
        );
    }
    for &(name, background) in PLACEHOLDERS {
        let proc = user32::address_of("DefWindowProcA");
        add(cpu, name, 0, proc, 4, 32512, background);
    }
}

// Helpers for the controls.

/// A control's GWL_STYLE, or 0.
pub fn style(cpu: &mut Cpu, hwnd: u32) -> u32 {
    user32::user(cpu).window(hwnd).map_or(0, |w| w.style)
}

/// A control's id (GWL_ID), or 0.
pub fn id_of(cpu: &mut Cpu, hwnd: u32) -> u32 {
    user32::user(cpu).window(hwnd).map_or(0, |w| w.id)
}

/// A control's client area, in client coordinates.
pub fn client(cpu: &mut Cpu, hwnd: u32) -> crate::win32::gdi32::Rect {
    let c = user32::user(cpu)
        .window(hwnd)
        .map(|w| w.client)
        .unwrap_or_default();
    crate::win32::gdi32::Rect::new(0, 0, c.width(), c.height())
}

/// GetParent(hwnd): a child's parent, a popup's owner.
pub fn parent(cpu: &mut Cpu, hwnd: u32) -> u32 {
    user32::window::get_parent(cpu, &[hwnd]).unwrap_or(0)
}

/// InvalidateRect(hwnd, NULL, erase).
pub fn invalidate(cpu: &mut Cpu, hwnd: u32, erase: bool) {
    user32::paint::invalidate_rect(cpu, &[hwnd, 0, erase as u32]);
}

/// A script that paints the control as WM_PAINT does without a DC:
/// BeginPaint, the module's `draw` with its DC, EndPaint; it returns 0.
pub fn paint_script(hwnd: u32) -> Script {
    let mut s = Script::new(hwnd);
    let ps = s.alloc(64);
    let begin = user32::address_of("BeginPaint");
    let end = user32::address_of("EndPaint");
    s.call(begin, &[Arg::V(hwnd), Arg::S(ps)])
        .then(DRAW, &[ps])
        .call(end, &[Arg::V(hwnd), Arg::S(ps)])
        .ret(0);
    s
}

/// Add to `s` WM_NOTIFY to `to` from the control `hwnd`: its NMHDR with
/// the code `code`, then `more`. The offset of the NMHDR in the scratch
/// space.
pub fn notify(cpu: &mut Cpu, s: &mut Script, hwnd: u32, to: u32, code: u32, more: &[u32]) -> u32 {
    let id = id_of(cpu, hwnd);
    let mut words = vec![hwnd, id, code];
    words.extend_from_slice(more);
    let at = s.data(&words);
    let msg = user32::message::WM_NOTIFY;
    s.send(to, msg, Arg::V(id), Arg::S(at));
    at
}
