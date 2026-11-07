//! OLEDLG.DLL: the OLE user interface dialogs, such as Insert Object,
//! Paste Special and the Server Busy box. Windows 95 and 98 both have
//! it, and a program linked with MFC's OLE support imports it:
//! COleMessageFilter imports OleUIBusyA by its ordinal, 8. Without the
//! DLL such a program, Discworld Noir's setup among them, doesn't start.
//!
//! None of the dialogs is written yet. Each function is in the table
//! with Windows 98 SE's ordinal, so a program that imports it by name or
//! by ordinal loads, and calling it does what calling a missing function
//! does (`super::not_written`): the program ends with a message naming
//! it. MFC asks OleUIBusy only when a call to another process's OLE
//! server is refused as busy, which the built-in OLE32, with no such
//! servers, never does.

use super::stubs::{Api, Conv, Dll};

pub static DLL: Dll = Dll {
    name: "OLEDLG.DLL",
    base: 0xBF5E_0000,
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

/// The functions, with Windows 98 SE's ordinals. Append only (see
/// `stubs`).
static APIS: &[Api] = &[
    stdcall("OleUIAddVerbMenuA", 1, 9, add_verb_menu_a),
    stdcall("OleUICanConvertOrActivateAs", 2, 3, can_convert),
    stdcall("OleUIInsertObjectA", 3, 1, insert_object_a),
    stdcall("OleUIPasteSpecialA", 4, 1, paste_special_a),
    stdcall("OleUIEditLinksA", 5, 1, edit_links_a),
    stdcall("OleUIChangeIconA", 6, 1, change_icon_a),
    stdcall("OleUIConvertA", 7, 1, convert_a),
    stdcall("OleUIBusyA", 8, 1, busy_a),
    stdcall("OleUIUpdateLinksA", 9, 4, update_links_a),
    Api {
        name: "OleUIPromptUserA",
        ordinal: 10,
        args: 0,
        conv: Conv::Cdecl,
        handler: prompt_user_a,
    },
    stdcall("OleUIObjectPropertiesA", 11, 1, object_properties_a),
    stdcall("OleUIChangeSourceA", 12, 1, change_source_a),
    Api {
        name: "OleUIPromptUserW",
        ordinal: 13,
        args: 0,
        conv: Conv::Cdecl,
        handler: prompt_user_w,
    },
    stdcall("OleUIAddVerbMenuW", 14, 9, add_verb_menu_w),
    stdcall("OleUIBusyW", 15, 1, busy_w),
    stdcall("OleUIChangeIconW", 16, 1, change_icon_w),
    stdcall("OleUIChangeSourceW", 17, 1, change_source_w),
    stdcall("OleUIConvertW", 18, 1, convert_w),
    stdcall("OleUIEditLinksW", 19, 1, edit_links_w),
    stdcall("OleUIInsertObjectW", 20, 1, insert_object_w),
    stdcall("OleUIObjectPropertiesW", 21, 1, object_properties_w),
    stdcall("OleUIPasteSpecialW", 22, 1, paste_special_w),
    stdcall("OleUIUpdateLinksW", 23, 4, update_links_w),
];

// Each returns a UINT or a BOOL, and fails from guest code with 0
// (OLEUI_FALSE, FALSE).
super::not_written_handlers! {
    DLL.name;
    add_verb_menu_a: "OleUIAddVerbMenuA" => 0,
    can_convert: "OleUICanConvertOrActivateAs" => 0,
    insert_object_a: "OleUIInsertObjectA" => 0,
    paste_special_a: "OleUIPasteSpecialA" => 0,
    edit_links_a: "OleUIEditLinksA" => 0,
    change_icon_a: "OleUIChangeIconA" => 0,
    convert_a: "OleUIConvertA" => 0,
    busy_a: "OleUIBusyA" => 0,
    update_links_a: "OleUIUpdateLinksA" => 0,
    prompt_user_a: "OleUIPromptUserA" => 0,
    object_properties_a: "OleUIObjectPropertiesA" => 0,
    change_source_a: "OleUIChangeSourceA" => 0,
    prompt_user_w: "OleUIPromptUserW" => 0,
    add_verb_menu_w: "OleUIAddVerbMenuW" => 0,
    busy_w: "OleUIBusyW" => 0,
    change_icon_w: "OleUIChangeIconW" => 0,
    change_source_w: "OleUIChangeSourceW" => 0,
    convert_w: "OleUIConvertW" => 0,
    edit_links_w: "OleUIEditLinksW" => 0,
    insert_object_w: "OleUIInsertObjectW" => 0,
    object_properties_w: "OleUIObjectPropertiesW" => 0,
    paste_special_w: "OleUIPasteSpecialW" => 0,
    update_links_w: "OleUIUpdateLinksW" => 0,
}
