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
use crate::cpu::Cpu;

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

/// Handlers for functions that aren't written yet, one a function, each
/// of which reports its own name.
macro_rules! not_written {
    ($($handler:ident: $name:literal),* $(,)?) => {
        $(
            fn $handler(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
                super::not_written(cpu, DLL.name, $name)
            }
        )*
    };
}

/// The functions, with Windows 98 SE's ordinals. Append only (see
/// `stubs`).
static APIS: &[Api] = &[
    stdcall("OleUIAddVerbMenuA", 1, 9, add_verb_menu_a),
    stdcall("OleUICanConvertOrActivateAs", 2, 3, can_convert_or_activate_as),
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

not_written! {
    add_verb_menu_a: "OleUIAddVerbMenuA",
    can_convert_or_activate_as: "OleUICanConvertOrActivateAs",
    insert_object_a: "OleUIInsertObjectA",
    paste_special_a: "OleUIPasteSpecialA",
    edit_links_a: "OleUIEditLinksA",
    change_icon_a: "OleUIChangeIconA",
    convert_a: "OleUIConvertA",
    busy_a: "OleUIBusyA",
    update_links_a: "OleUIUpdateLinksA",
    prompt_user_a: "OleUIPromptUserA",
    object_properties_a: "OleUIObjectPropertiesA",
    change_source_a: "OleUIChangeSourceA",
    prompt_user_w: "OleUIPromptUserW",
    add_verb_menu_w: "OleUIAddVerbMenuW",
    busy_w: "OleUIBusyW",
    change_icon_w: "OleUIChangeIconW",
    change_source_w: "OleUIChangeSourceW",
    convert_w: "OleUIConvertW",
    edit_links_w: "OleUIEditLinksW",
    insert_object_w: "OleUIInsertObjectW",
    object_properties_w: "OleUIObjectPropertiesW",
    paste_special_w: "OleUIPasteSpecialW",
    update_links_w: "OleUIUpdateLinksW",
}
