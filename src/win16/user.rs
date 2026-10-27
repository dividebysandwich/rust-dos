//! USER: windows, messages, dialogs, menus and the rest of the user
//! interface, on USER32's. Most functions go on to USER32's of the same
//! name with their arguments widened; those whose structures, callbacks
//! or messages differ between Windows 3.1 and Win32 are converted here
//! (message.rs has the messages).

use super::builtin::{Api16, Def, cdecl, rust, win32};
use super::handles::{accel32, icon16, icon32, instance16, instance32, menu32};
use super::thunk::{self, KIND_DLGPROC, KIND_PLAIN, KIND_WNDPROC};
use super::{kernel, message};
use crate::cpu::{Cpu, Seg};
use crate::dpmi::Context;
use crate::win32::user32::text::ansi_case;

pub static MODULE: Def = Def {
    name: "USER",
    file: "USER.EXE",
    apis: APIS,
    library: false,
};

const U: &str = "USER32.DLL";
const K: &str = "KERNEL32.DLL";
const G: &str = "GDI32.DLL";

/// The functions. Append only: a function's place is its id's.
static APIS: &[Api16] = &[
    win32("MessageBox", 1, "wppw", U, "MessageBoxA", b'w'),
    rust("InitApp", 5, "w", one),
    win32("PostQuitMessage", 6, "s", U, "PostQuitMessage", b'w'),
    rust("ExitWindows", 7, "lw", zero),
    rust("SetTimer", 10, "wwwS", set_timer),
    rust("SetSystemTimer", 11, "wwwS", set_timer),
    win32("KillTimer", 12, "ww", U, "KillTimer", b'b'),
    win32("GetTickCount", 13, "", K, "GetTickCount", b'w'),
    rust("GetTimerResolution", 14, "", timer_resolution),
    win32("GetCurrentTime", 15, "", K, "GetTickCount", b'w'),
    win32("ClipCursor", 16, "R", U, "ClipCursor", b'b'),
    win32("GetCursorPos", 17, "P", U, "GetCursorPos", b'b'),
    win32("SetCapture", 18, "w", U, "SetCapture", b'w'),
    win32("ReleaseCapture", 19, "", U, "ReleaseCapture", b'b'),
    win32("SetDoubleClickTime", 20, "w", U, "SetDoubleClickTime", b'b'),
    win32("GetDoubleClickTime", 21, "", U, "GetDoubleClickTime", b'w'),
    win32("SetFocus", 22, "w", U, "SetFocus", b'w'),
    win32("GetFocus", 23, "", U, "GetFocus", b'w'),
    win32("RemoveProp", 24, "wr", U, "RemovePropA", b'w'),
    win32("GetProp", 25, "wr", U, "GetPropA", b'w'),
    win32("SetProp", 26, "wrw", U, "SetPropA", b'b'),
    win32("ClientToScreen", 28, "wP", U, "ClientToScreen", b'b'),
    win32("ScreenToClient", 29, "wP", U, "ScreenToClient", b'b'),
    rust("WindowFromPoint", 30, "l", window_from_point),
    win32("IsIconic", 31, "w", U, "IsIconic", b'b'),
    win32("GetWindowRect", 32, "wR", U, "GetWindowRect", b'b'),
    win32("GetClientRect", 33, "wR", U, "GetClientRect", b'b'),
    win32("EnableWindow", 34, "ww", U, "EnableWindow", b'b'),
    win32("IsWindowEnabled", 35, "w", U, "IsWindowEnabled", b'b'),
    win32("GetWindowText", 36, "wps", U, "GetWindowTextA", b'w'),
    win32("SetWindowText", 37, "wp", U, "SetWindowTextA", b'b'),
    win32(
        "GetWindowTextLength",
        38,
        "w",
        U,
        "GetWindowTextLengthA",
        b'w',
    ),
    rust("BeginPaint", 39, "wp", begin_paint),
    rust("EndPaint", 40, "wp", end_paint),
    rust("CreateWindow", 41, "rplsssswwnS", create_window),
    win32("ShowWindow", 42, "ws", U, "ShowWindow", b'b'),
    win32("CloseWindow", 43, "w", U, "CloseWindow", b'b'),
    win32("OpenIcon", 44, "w", U, "OpenIcon", b'b'),
    win32("BringWindowToTop", 45, "w", U, "BringWindowToTop", b'b'),
    win32("GetParent", 46, "w", U, "GetParent", b'w'),
    win32("IsWindow", 47, "w", U, "IsWindow", b'b'),
    win32("IsChild", 48, "ww", U, "IsChild", b'b'),
    win32("IsWindowVisible", 49, "w", U, "IsWindowVisible", b'b'),
    win32("FindWindow", 50, "rp", U, "FindWindowA", b'w'),
    win32("AnyPopup", 52, "", U, "AnyPopup", b'b'),
    win32("DestroyWindow", 53, "w", U, "DestroyWindow", b'b'),
    rust("EnumWindows", 54, "Sl", enum_windows),
    rust("EnumChildWindows", 55, "wSl", enum_child_windows),
    win32("MoveWindow", 56, "wsssss", U, "MoveWindow", b'b'),
    rust("RegisterClass", 57, "p", register_class),
    win32("GetClassName", 58, "wps", U, "GetClassNameA", b'w'),
    win32("SetActiveWindow", 59, "w", U, "SetActiveWindow", b'w'),
    win32("GetActiveWindow", 60, "", U, "GetActiveWindow", b'w'),
    win32("ScrollWindow", 61, "wssRR", U, "ScrollWindow", b'b'),
    win32("SetScrollPos", 62, "wwsw", U, "SetScrollPos", b'w'),
    win32("GetScrollPos", 63, "ww", U, "GetScrollPos", b'w'),
    win32("SetScrollRange", 64, "wwssw", U, "SetScrollRange", b'b'),
    rust("GetScrollRange", 65, "wwpp", get_scroll_range),
    win32("GetDC", 66, "w", U, "GetDC", b'w'),
    win32("GetWindowDC", 67, "w", U, "GetWindowDC", b'w'),
    win32("ReleaseDC", 68, "ww", U, "ReleaseDC", b'w'),
    win32("SetCursor", 69, "i", U, "SetCursor", b'i'),
    win32("SetCursorPos", 70, "ss", U, "SetCursorPos", b'b'),
    win32("ShowCursor", 71, "w", U, "ShowCursor", b'w'),
    win32("SetRect", 72, "Rssss", U, "SetRect", b'b'),
    win32("SetRectEmpty", 73, "R", U, "SetRectEmpty", b'b'),
    win32("CopyRect", 74, "RR", U, "CopyRect", b'b'),
    win32("IsRectEmpty", 75, "R", U, "IsRectEmpty", b'b'),
    rust("PtInRect", 76, "pl", pt_in_rect),
    win32("OffsetRect", 77, "Rss", U, "OffsetRect", b'b'),
    win32("InflateRect", 78, "Rss", U, "InflateRect", b'b'),
    win32("IntersectRect", 79, "RRR", U, "IntersectRect", b'b'),
    win32("UnionRect", 80, "RRR", U, "UnionRect", b'b'),
    win32("FillRect", 81, "wRw", U, "FillRect", b'w'),
    win32("InvertRect", 82, "wR", U, "InvertRect", b'b'),
    win32("FrameRect", 83, "wRw", U, "FrameRect", b'w'),
    win32("DrawIcon", 84, "wssi", U, "DrawIcon", b'b'),
    win32("DrawText", 85, "wpsRw", U, "DrawTextA", b'w'),
    rust("DialogBox", 87, "nrwS", dialog_box),
    win32("EndDialog", 88, "ws", U, "EndDialog", b'b'),
    rust("CreateDialog", 89, "nrwS", create_dialog),
    rust("IsDialogMessage", 90, "wp", is_dialog_message),
    win32("GetDlgItem", 91, "ww", U, "GetDlgItem", b'w'),
    win32("SetDlgItemText", 92, "wwp", U, "SetDlgItemTextA", b'b'),
    win32("GetDlgItemText", 93, "wwpw", U, "GetDlgItemTextA", b'w'),
    rust("SetDlgItemInt", 94, "wwww", set_dlg_item_int),
    rust("GetDlgItemInt", 95, "wwpw", get_dlg_item_int),
    win32("CheckRadioButton", 96, "wwww", U, "CheckRadioButton", b'b'),
    win32("CheckDlgButton", 97, "www", U, "CheckDlgButton", b'b'),
    win32(
        "IsDlgButtonChecked",
        98,
        "ww",
        U,
        "IsDlgButtonChecked",
        b'w',
    ),
    rust("DlgDirSelect", 99, "wpw", dlg_dir_select),
    win32("DlgDirList", 100, "wpwww", U, "DlgDirListA", b'w'),
    rust("SendDlgItemMessage", 101, "wwwwl", send_dlg_item_message),
    win32("AdjustWindowRect", 102, "Rlw", U, "AdjustWindowRect", b'b'),
    win32("IsMenu", 358, "m", U, "IsMenu", b'b'),
    win32(
        "AdjustWindowRectEx",
        454,
        "Rlwl",
        U,
        "AdjustWindowRectEx",
        b'b',
    ),
    win32("MapDialogRect", 103, "wR", U, "MapDialogRect", b'b'),
    win32("MessageBeep", 104, "w", U, "MessageBeep", b'b'),
    win32("FlashWindow", 105, "ww", U, "FlashWindow", b'b'),
    win32("GetKeyState", 106, "w", U, "GetKeyState", b'w'),
    rust("DefWindowProc", 107, "wwwl", def_window_proc),
    rust("GetMessage", 108, "pwww", get_message),
    rust("PeekMessage", 109, "pwwww", peek_message),
    rust("PostMessage", 110, "wwwl", message::post_message),
    rust("SendMessage", 111, "wwwl", message::send_message),
    win32("WaitMessage", 112, "", U, "WaitMessage", b'b'),
    rust("TranslateMessage", 113, "p", translate_message),
    rust("DispatchMessage", 114, "p", dispatch_message),
    win32("ReplyMessage", 115, "l", U, "ReplyMessage", b'b'),
    rust("PostAppMessage", 116, "wwwl", post_app_message),
    win32(
        "RegisterWindowMessage",
        118,
        "p",
        U,
        "RegisterWindowMessageA",
        b'w',
    ),
    win32("GetMessagePos", 119, "", U, "GetMessagePos", b'w'),
    win32("GetMessageTime", 120, "", U, "GetMessageTime", b'w'),
    rust("SetWindowsHook", 121, "sS", set_windows_hook),
    rust("CallWindowProc", 122, "Swwwl", call_window_proc),
    rust("CallMsgFilter", 123, "ps", zero),
    win32("UpdateWindow", 124, "w", U, "UpdateWindow", b'b'),
    win32("InvalidateRect", 125, "wRw", U, "InvalidateRect", b'b'),
    win32("InvalidateRgn", 126, "www", U, "InvalidateRgn", b'b'),
    win32("ValidateRect", 127, "wR", U, "ValidateRect", b'b'),
    win32("ValidateRgn", 128, "ww", U, "ValidateRgn", b'b'),
    rust("GetClassWord", 129, "ws", get_class_word),
    rust("SetClassWord", 130, "wsw", set_class_word),
    rust("GetClassLong", 131, "ws", get_class_long),
    rust("SetClassLong", 132, "wsl", set_class_long),
    rust("GetWindowWord", 133, "ws", get_window_word),
    rust("SetWindowWord", 134, "wsw", set_window_word),
    rust("GetWindowLong", 135, "ws", get_window_long),
    rust("SetWindowLong", 136, "wsl", set_window_long),
    win32("OpenClipboard", 137, "w", U, "OpenClipboard", b'b'),
    win32("CloseClipboard", 138, "", U, "CloseClipboard", b'b'),
    win32("EmptyClipboard", 139, "", U, "EmptyClipboard", b'b'),
    win32("GetClipboardOwner", 140, "", U, "GetClipboardOwner", b'w'),
    rust("SetClipboardData", 141, "ww", zero),
    rust("GetClipboardData", 142, "w", zero),
    win32(
        "CountClipboardFormats",
        143,
        "",
        U,
        "CountClipboardFormats",
        b'w',
    ),
    win32(
        "EnumClipboardFormats",
        144,
        "w",
        U,
        "EnumClipboardFormats",
        b'w',
    ),
    win32(
        "RegisterClipboardFormat",
        145,
        "p",
        U,
        "RegisterClipboardFormatA",
        b'w',
    ),
    win32(
        "GetClipboardFormatName",
        146,
        "wpw",
        U,
        "GetClipboardFormatNameA",
        b'w',
    ),
    win32(
        "SetClipboardViewer",
        147,
        "w",
        U,
        "SetClipboardViewer",
        b'w',
    ),
    win32("GetClipboardViewer", 148, "", U, "GetClipboardViewer", b'w'),
    win32(
        "ChangeClipboardChain",
        149,
        "ww",
        U,
        "ChangeClipboardChain",
        b'b',
    ),
    win32("LoadMenu", 150, "nr", U, "LoadMenuA", b'm'),
    win32("CreateMenu", 151, "", U, "CreateMenu", b'm'),
    win32("DestroyMenu", 152, "m", U, "DestroyMenu", b'b'),
    rust("ChangeMenu", 153, "wwSww", change_menu),
    rust("CheckMenuItem", 154, "www", check_menu_item),
    rust("EnableMenuItem", 155, "www", enable_menu_item),
    win32("GetSystemMenu", 156, "ww", U, "GetSystemMenu", b'm'),
    win32("GetMenu", 157, "w", U, "GetMenu", b'm'),
    win32("SetMenu", 158, "wm", U, "SetMenu", b'b'),
    win32("GetSubMenu", 159, "ms", U, "GetSubMenu", b'm'),
    win32("DrawMenuBar", 160, "w", U, "DrawMenuBar", b'b'),
    rust("GetMenuString", 161, "wwpsw", get_menu_string),
    rust("HiliteMenuItem", 162, "wwww", hilite_menu_item),
    win32("CreateCaret", 163, "wwss", U, "CreateCaret", b'b'),
    win32("DestroyCaret", 164, "", U, "DestroyCaret", b'b'),
    win32("SetCaretPos", 165, "ss", U, "SetCaretPos", b'b'),
    win32("HideCaret", 166, "w", U, "HideCaret", b'b'),
    win32("ShowCaret", 167, "w", U, "ShowCaret", b'b'),
    win32("SetCaretBlinkTime", 168, "w", U, "SetCaretBlinkTime", b'b'),
    win32("GetCaretBlinkTime", 169, "", U, "GetCaretBlinkTime", b'w'),
    win32(
        "ArrangeIconicWindows",
        170,
        "w",
        U,
        "ArrangeIconicWindows",
        b'w',
    ),
    win32("WinHelp", 171, "wpwl", U, "WinHelpA", b'b'),
    win32("LoadCursor", 173, "nr", U, "LoadCursorA", b'i'),
    win32("LoadIcon", 174, "nr", U, "LoadIconA", b'i'),
    win32("LoadBitmap", 175, "nr", U, "LoadBitmapA", b'w'),
    win32("LoadString", 176, "nwps", U, "LoadStringA", b'w'),
    win32("LoadAccelerators", 177, "nr", U, "LoadAcceleratorsA", b'a'),
    rust("TranslateAccelerator", 178, "wwp", translate_accelerator),
    rust("GetSystemMetrics", 179, "s", get_system_metrics),
    win32("GetSysColor", 180, "w", U, "GetSysColor", b'w'),
    rust("SetSysColors", 181, "wpp", zero),
    win32("GetCaretPos", 183, "P", U, "GetCaretPos", b'b'),
    rust("GrayString", 185, "wwSlsssss", gray_string),
    win32("SwapMouseButton", 186, "w", U, "SwapMouseButton", b'b'),
    win32("EndMenu", 187, "", U, "EndMenu", b'b'),
    rust("SetSysModalWindow", 188, "w", zero),
    rust("GetSysModalWindow", 189, "", zero),
    win32("GetUpdateRect", 190, "wRw", U, "GetUpdateRect", b'b'),
    rust("ChildWindowFromPoint", 191, "wl", child_window_from_point),
    win32(
        "IsClipboardFormatAvailable",
        193,
        "w",
        U,
        "IsClipboardFormatAvailable",
        b'b',
    ),
    win32(
        "DlgDirListComboBox",
        195,
        "wpwww",
        U,
        "DlgDirListComboBoxA",
        b'w',
    ),
    rust("TabbedTextOut", 196, "wsspssps", tabbed_text_out),
    rust("GetTabbedTextExtent", 197, "wpwwp", get_tabbed_text_extent),
    rust("OpenComm", 200, "pww", minus_one),
    rust("CloseComm", 207, "w", minus_one),
    rust("DialogBoxIndirect", 218, "nwwS", dialog_box_indirect),
    rust("CreateDialogIndirect", 219, "nSwS", create_dialog_indirect),
    win32("ScrollDC", 221, "wssRRwR", U, "ScrollDC", b'b'),
    win32("GetKeyboardState", 222, "p", U, "GetKeyboardState", b'b'),
    win32("SetKeyboardState", 223, "p", U, "SetKeyboardState", b'b'),
    rust("GetWindowTask", 224, "w", get_window_task),
    rust("EnumTaskWindows", 225, "wSl", enum_task_windows),
    win32(
        "GetNextDlgGroupItem",
        227,
        "www",
        U,
        "GetNextDlgGroupItem",
        b'w',
    ),
    win32(
        "GetNextDlgTabItem",
        228,
        "www",
        U,
        "GetNextDlgTabItem",
        b'w',
    ),
    win32("GetTopWindow", 229, "w", U, "GetTopWindow", b'w'),
    win32("GetNextWindow", 230, "ww", U, "GetNextWindow", b'w'),
    win32("SetWindowPos", 232, "whssssw", U, "SetWindowPos", b'b'),
    win32("SetParent", 233, "ww", U, "SetParent", b'w'),
    rust("UnhookWindowsHook", 234, "sS", one),
    rust("DefHookProc", 235, "swlp", zero),
    win32("GetCapture", 236, "", U, "GetCapture", b'w'),
    win32("GetUpdateRgn", 237, "www", U, "GetUpdateRgn", b'w'),
    win32("ExcludeUpdateRgn", 238, "ww", U, "ExcludeUpdateRgn", b'w'),
    rust("DialogBoxParam", 239, "nrwSl", dialog_box),
    rust("DialogBoxIndirectParam", 240, "nwwSl", dialog_box_indirect),
    rust("CreateDialogParam", 241, "nrwSl", create_dialog),
    rust(
        "CreateDialogIndirectParam",
        242,
        "nSwSl",
        create_dialog_indirect,
    ),
    win32("GetDialogBaseUnits", 243, "", U, "GetDialogBaseUnits", b'w'),
    win32("EqualRect", 244, "RR", U, "EqualRect", b'b'),
    win32("GetCursor", 247, "", U, "GetCursor", b'i'),
    win32("GetAsyncKeyState", 249, "w", U, "GetAsyncKeyState", b'w'),
    rust("GetMenuState", 250, "www", get_menu_state),
    rust("SendDriverMessage", 251, "wwll", zero),
    rust("OpenDriver", 252, "ppl", zero),
    rust("CloseDriver", 253, "wll", zero),
    rust("GetDriverModuleHandle", 254, "w", zero),
    rust("DefDriverProc", 255, "lwwll", zero),
    rust("MapWindowPoints", 258, "wwpw", map_window_points),
    win32(
        "BeginDeferWindowPos",
        259,
        "s",
        U,
        "BeginDeferWindowPos",
        b'w',
    ),
    win32("DeferWindowPos", 260, "wwhssssw", U, "DeferWindowPos", b'w'),
    win32("EndDeferWindowPos", 261, "w", U, "EndDeferWindowPos", b'b'),
    win32("GetWindow", 262, "ww", U, "GetWindow", b'w'),
    win32("GetMenuItemCount", 263, "m", U, "GetMenuItemCount", b'w'),
    win32("GetMenuItemID", 264, "ms", U, "GetMenuItemID", b'w'),
    win32("ShowOwnedPopups", 265, "ww", U, "ShowOwnedPopups", b'b'),
    rust("SetMessageQueue", 266, "w", one),
    win32("ShowScrollBar", 267, "www", U, "ShowScrollBar", b'b'),
    rust("GlobalAddAtom", 268, "S", kernel::add_atom),
    rust("GlobalDeleteAtom", 269, "w", kernel::delete_atom),
    rust("GlobalFindAtom", 270, "S", kernel::find_atom),
    rust("GlobalGetAtomName", 271, "wps", kernel::get_atom_name),
    win32("IsZoomed", 272, "w", U, "IsZoomed", b'b'),
    win32("GetDlgCtrlID", 277, "w", U, "GetDlgCtrlID", b'w'),
    win32("GetDesktopHwnd", 278, "", U, "GetDesktopWindow", b'w'),
    win32("SelectPalette", 282, "www", G, "SelectPalette", b'w'),
    win32("RealizePalette", 283, "w", G, "RealizePalette", b'w'),
    rust("GetFreeSystemResources", 284, "w", free_system_resources),
    win32("GetDesktopWindow", 286, "", U, "GetDesktopWindow", b'w'),
    win32(
        "GetLastActivePopup",
        287,
        "w",
        U,
        "GetLastActivePopup",
        b'w',
    ),
    win32("RedrawWindow", 290, "wRww", U, "RedrawWindow", b'b'),
    rust("SetWindowsHookEx", 291, "sSww", set_windows_hook),
    rust("UnhookWindowsHookEx", 292, "S", one),
    rust("CallNextHookEx", 293, "Sswl", zero),
    win32("LockWindowUpdate", 294, "w", U, "LockWindowUpdate", b'b'),
    rust("DefDlgProc", 308, "wwwl", def_dlg_proc),
    rust("GetWindowPlacement", 370, "wp", get_window_placement),
    rust("SetWindowPlacement", 371, "wp", set_window_placement),
    rust("UnregisterClass", 403, "Sn", unregister_class),
    rust("GetClassInfo", 404, "nSp", get_class_info),
    rust("CreateCursor", 406, "nsssspp", create_cursor),
    rust("CreateIcon", 407, "nsswwpp", create_icon),
    rust("InsertMenu", 410, "mwwwS", insert_menu),
    rust("AppendMenu", 411, "mwwS", append_menu),
    rust("RemoveMenu", 412, "www", remove_menu),
    rust("DeleteMenu", 413, "www", delete_menu),
    rust("ModifyMenu", 414, "mwwwS", modify_menu),
    win32("CreatePopupMenu", 415, "", U, "CreatePopupMenu", b'm'),
    win32("TrackPopupMenu", 416, "mwssswR", U, "TrackPopupMenu", b'b'),
    rust("GetMenuCheckMarkDimensions", 417, "", check_mark_dimensions),
    win32(
        "SetMenuItemBitmaps",
        418,
        "mwwww",
        U,
        "SetMenuItemBitmaps",
        b'b',
    ),
    cdecl("_wsprintf", 420, "SS", wsprintf),
    rust("wvsprintf", 421, "SSS", wvsprintf),
    win32("lstrcmp", 430, "pp", K, "lstrcmpA", b'w'),
    rust("AnsiUpper", 431, "S", ansi_upper),
    rust("AnsiLower", 432, "S", ansi_lower),
    win32("IsCharAlpha", 433, "w", U, "IsCharAlphaA", b'b'),
    win32(
        "IsCharAlphaNumeric",
        434,
        "w",
        U,
        "IsCharAlphaNumericA",
        b'b',
    ),
    win32("IsCharUpper", 435, "w", U, "IsCharUpperA", b'b'),
    win32("IsCharLower", 436, "w", U, "IsCharLowerA", b'b'),
    rust("AnsiUpperBuff", 437, "pw", ansi_upper_buff),
    rust("AnsiLowerBuff", 438, "pw", ansi_lower_buff),
    rust("DefFrameProc", 445, "wwwwl", def_frame_proc),
    rust("DefMDIChildProc", 447, "wwwl", def_mdi_child_proc),
    rust("TranslateMDISysAccel", 451, "wp", translate_mdi_sys_accel),
    rust("CreateWindowEx", 452, "lrplsssswwnS", create_window_ex),
    win32("DestroyIcon", 457, "i", U, "DestroyIcon", b'b'),
    win32("DestroyCursor", 458, "i", U, "DestroyCursor", b'b'),
    win32("DrawFocusRect", 466, "wR", U, "DrawFocusRect", b'b'),
    win32("lstrcmpi", 471, "pp", K, "lstrcmpiA", b'w'),
    rust("AnsiNext", 472, "S", ansi_next),
    rust("AnsiPrev", 473, "SS", ansi_prev),
    win32("EnableScrollBar", 482, "www", U, "EnableScrollBar", b'b'),
    rust("SystemParametersInfo", 483, "wwpw", system_parameters_info),
    rust("WNetGetConnection", 512, "ppp", wnet_error),
    win32("GetDCEx", 359, "wwl", U, "GetDCEx", b'w'),
    win32("DrawIconEx", 394, "wssisswww", U, "DrawIconEx", b'b'),
    win32("CopyIcon", 368, "ni", U, "CopyIcon", b'i'),
    win32("CopyCursor", 369, "ni", U, "CopyIcon", b'i'),
    win32("GetInputState", 335, "", U, "GetInputState", b'b'),
    win32("GetQueueStatus", 334, "w", U, "GetQueueStatus", b'w'),
];

fn zero(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

fn one(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(1)
}

fn minus_one(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0xFFFF)
}

/// GetTimerResolution: 1000, as Wine says.
/// GetSystemMetrics(index). Windows 98 gives Windows 3.x programs the
/// caption and menu bar heights Windows 3.1 counted: a caption one pixel
/// taller (SM_CYCAPTION 20) and a menu bar one pixel lower (SM_CYMENU
/// 18) than Win32 programs get. Their windows are drawn the same as
/// Win32 ones, with the client area 42 pixels below the window's top.
fn get_system_metrics(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    use crate::win32::user32::sysparams;
    let index = args[0];
    let value = sysparams::get_system_metrics(cpu, &[index])?;
    Some(
        match index {
            SM_CYCAPTION => value.wrapping_add(1),
            SM_CYMENU => value.wrapping_sub(1),
            _ => value,
        } & 0xFFFF,
    )
}

const SM_CYCAPTION: u32 = 4;
const SM_CYMENU: u32 = 15;

fn timer_resolution(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(1000)
}

fn wnet_error(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    // WN_NOT_SUPPORTED.
    Some(1)
}

fn free_system_resources(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(80)
}

fn check_mark_dimensions(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(13 << 16 | 13)
}

/// The address of USER32's function `name`.
fn api(name: &str) -> u32 {
    crate::win32::user32::address_of(name)
}

/// Go on to USER32's `name` with `args`; DX:AX what it returns.
fn go(cpu: &mut Cpu, name: &str, args: &[u32]) -> Option<u32> {
    thunk::call32(cpu, api(name), args, thunk::POST_PLAIN, Vec::new());
    None
}

/// Go on to USER32's `name` with `args`, `post` (this module's `post`)
/// seeing to what it returns with `data`.
fn go_post(cpu: &mut Cpu, name: &str, args: &[u32], data: Vec<u32>) -> Option<u32> {
    thunk::call32(cpu, api(name), args, thunk::POST_USER, data);
    None
}

/// `go_post` with the 32-bit stack below `top`, scratch space above it
/// that `thunk::scratch32` gave.
fn go_at(cpu: &mut Cpu, top: u32, name: &str, args: &[u32], data: Vec<u32>) -> Option<u32> {
    let ctx = Context::of(cpu);
    let post = if data.is_empty() {
        thunk::POST_PLAIN
    } else {
        thunk::POST_USER
    };
    thunk::enter32_at(cpu, ctx, top, api(name), args, post, data);
    None
}

/// What `post` does with what a USER32 function returned: `data[0]`.
const P_MSG: u32 = 1;
const P_PAINT: u32 = 2;
const P_CLASSINFO: u32 = 3;
const P_PROC16: u32 = 4;
const P_INSTANCE: u32 = 5;
pub(super) const P_WORDS: u32 = 6;
pub(super) const P_POINTS: u32 = 7;
const P_PLACEMENT: u32 = 8;
const P_ICON: u32 = 9;
const P_RECT: u32 = 10;
const P_PEEK: u32 = 11;
const P_SCROLL_RANGE: u32 = 12;
const P_MODULE: u32 = 13;
const P_DLG_INT: u32 = 14;
const P_CLASS_MENU: u32 = 15;
const P_EX_STYLE: u32 = 16;

/// The extended style USER32 gives captioned windows, which Windows 95
/// doesn't show programs for Windows 3.1.
pub(super) const WS_EX_WINDOWEDGE: u32 = 0x0100;
const GWL_EXSTYLE: u32 = (-20i32) as u32;
/// The kinds from here on are GDI's (gdi.rs).
pub(super) const P_GDI: u32 = 100;
/// And from here on misc.rs's.
pub(super) const P_MISC: u32 = 200;

/// PeekMessage's flag that keeps the task from yielding.
const PM_NOYIELD: u32 = 0x0002;

/// A USER32 function a function of this module went on to returned
/// `value`: the structures it filled converted to 16-bit ones, what it
/// returned as 16-bit code gets it.
pub fn post(cpu: &mut Cpu, value: u32, data: &[u32], ctx: &mut Context) -> Option<u32> {
    let d = |i: usize| data.get(i).copied().unwrap_or(0);
    if d(0) >= P_MISC {
        return super::misc::post(cpu, value, data);
    }
    if d(0) >= P_GDI {
        return super::gdi::post(cpu, value, data);
    }
    match d(0) {
        P_MSG => message::msg16(cpu, d(1), d(2)),
        P_PEEK => {
            if value != 0 {
                message::msg16(cpu, d(1), d(2));
            } else if d(3) & PM_NOYIELD == 0 {
                // With no message, Windows 3.1's PeekMessage lets the
                // other tasks run before it returns, so a task that waits
                // in a PeekMessage loop for another to end lets it run.
                let mut resume = *ctx;
                thunk::set_ax(&mut resume, 0);
                resume.gpr[thunk::EDX] &= 0xFFFF_0000;
                super::task::yield_now(cpu, resume);
                return None;
            }
        }
        P_PAINT => {
            let (ps, ps16) = (d(1), d(2));
            let mut bytes = Vec::new();
            for i in 0..2 {
                bytes.extend_from_slice(&(super::read_32(cpu, ps + 4 * i) as u16).to_le_bytes());
            }
            for i in 0..4 {
                bytes
                    .extend_from_slice(&(super::read_32(cpu, ps + 8 + 4 * i) as u16).to_le_bytes());
            }
            for i in 0..2 {
                bytes.extend_from_slice(
                    &(super::read_32(cpu, ps + 24 + 4 * i) as u16).to_le_bytes(),
                );
            }
            bytes.resize(32, 0);
            super::poke(cpu, ps16, &bytes);
        }
        P_CLASSINFO => {
            if value != 0 {
                class16(cpu, d(1), d(2), d(3), d(4));
            }
        }
        P_EX_STYLE => return Some(value & !WS_EX_WINDOWEDGE),
        // GetDlgItemInt: the text at d(1) taken, TRUE or FALSE at d(2).
        P_DLG_INT => {
            let text = super::string_bytes(cpu, d(1), DLG_INT_TEXT as usize);
            let found = dlg_int(&text, d(3) != 0);
            if d(2) != 0 {
                super::write_16(cpu, d(2), found.is_some() as u16);
            }
            return Some(found.unwrap_or(0) as u32);
        }
        // A class registered (d(1) its name, d(2) its menu's name) or, with
        // d(3), unregistered: what GetClassInfo gives as its menu's name.
        P_CLASS_MENU => {
            if value != 0 {
                let key = class_key(cpu, d(1));
                let menus = &mut cpu.bus.win32.win16.class_menus;
                menus.retain(|(k, _)| *k != key);
                if d(3) == 0 {
                    menus.push((key, d(2)));
                }
            }
            // RegisterClass's atom, or UnregisterClass's BOOL.
            return Some(if d(3) == 0 {
                value
            } else {
                (value != 0) as u32
            });
        }
        // A class's module: its instance's, USER's for the system's own.
        P_MODULE => {
            let win16 = &cpu.bus.win32.win16;
            let module = win16
                .modules
                .iter()
                .find(|m| m.resources32 == value)
                .or_else(|| win16.modules.iter().find(|m| m.name == "USER"));
            return Some(module.map_or(0, |m| m.handle) as u32);
        }
        P_PROC16 => {
            let kind = d(1) as u8;
            return Some(proc16_of(cpu, value, kind));
        }
        P_INSTANCE => return Some(instance16(cpu, value) as u32),
        P_ICON => return Some(icon16(value) as u32),
        P_WORDS => {
            // 32-bit integers at d(1) as 16-bit ones at d(2), d(3) of them.
            for i in 0..d(3) {
                let v = super::read_32(cpu, d(1) + 4 * i);
                super::write_16(cpu, d(2) + 2 * i, v as u16);
            }
        }
        P_POINTS => {
            for i in 0..2 * d(3) {
                let v = super::read_32(cpu, d(1) + 4 * i);
                super::write_16(cpu, d(2) + 2 * i, v as u16);
            }
        }
        P_RECT => {
            for i in 0..4 {
                let v = super::read_32(cpu, d(1) + 4 * i);
                super::write_16(cpu, d(2) + 2 * i, v as u16);
            }
        }
        P_SCROLL_RANGE => {
            // The two integers at d(1) to two separate 16-bit pointers.
            for (i, to) in [d(2), d(3)].into_iter().enumerate() {
                let v = super::read_32(cpu, d(1) + 4 * i as u32);
                super::write_16(cpu, to, v as u16);
            }
        }
        P_PLACEMENT if value != 0 => {
            for i in 0..11 {
                let v = super::read_32(cpu, d(1) + 4 * i);
                super::write_16(cpu, d(2) + 2 * i, v as u16);
            }
            super::write_16(cpu, d(2), 22);
        }
        _ => {}
    }
    Some(value)
}

/// The 16:16 procedure 16-bit code gets for the 32-bit `proc` of the
/// kind `kind` (a window procedure's slot, or the 16-bit one it stands
/// for).
fn proc16_of(cpu: &mut Cpu, proc: u32, kind: u8) -> u32 {
    thunk::proc16(cpu, proc, kind, 0, 10)
}

/// The 32-bit procedure 32-bit code gets for the 16-bit `proc`, a window
/// procedure of the instance `instance`.
fn proc32_of(cpu: &mut Cpu, proc: u32, kind: u8, instance: u16) -> u32 {
    thunk::proc32(cpu, proc, kind, instance as u32)
}

fn r16(cpu: &Cpu, at: u32) -> u32 {
    super::read_16(cpu, at) as u32
}

fn r32(cpu: &Cpu, at: u32) -> u32 {
    super::read_32(cpu, at)
}

fn sx(v: u32) -> u32 {
    v as u16 as i16 as i32 as u32
}

/// The WNDCLASSA at `wc` as the 16-bit WNDCLASS at `wc16`, with the
/// menu's name and the class's name the 16:16 pointers `menu` and
/// `class`.
fn class16(cpu: &mut Cpu, wc: u32, wc16: u32, menu: u32, class: u32) {
    let style = r32(cpu, wc);
    let proc = r32(cpu, wc + 4);
    let proc = proc16_of(cpu, proc, KIND_WNDPROC);
    let (cls, wnd) = (r32(cpu, wc + 8), r32(cpu, wc + 12));
    let instance = instance16(cpu, r32(cpu, wc + 16));
    let icon = icon16(r32(cpu, wc + 20));
    let cursor = icon16(r32(cpu, wc + 24));
    let brush = r32(cpu, wc + 28);
    let mut b = Vec::new();
    b.extend_from_slice(&(style as u16).to_le_bytes());
    b.extend_from_slice(&proc.to_le_bytes());
    for v in [cls as u16, wnd as u16, instance, icon, cursor, brush as u16] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(&menu.to_le_bytes());
    b.extend_from_slice(&class.to_le_bytes());
    super::poke(cpu, wc16, &b);
}

// Classes and windows.

/// RegisterClass(WNDCLASS16): USER32's RegisterClassA with the window
/// procedure's slot, the handles and the names widened.
fn register_class(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let wc16 = args[0];
    let instance = r16(cpu, wc16 + 10) as u16;
    let proc = r32(cpu, wc16 + 2);
    let proc = proc32_of(cpu, proc, KIND_WNDPROC, instance);
    let menu = r32(cpu, wc16 + 18);
    let class = r32(cpu, wc16 + 22);
    let name = |cpu: &Cpu, p: u32| {
        if p >> 16 == 0 {
            p
        } else {
            super::linear(cpu, p)
        }
    };
    let fields = [
        r16(cpu, wc16),
        proc,
        sx(r16(cpu, wc16 + 6)),
        sx(r16(cpu, wc16 + 8)),
        instance32(cpu, instance),
        icon32(r16(cpu, wc16 + 12) as u16),
        icon32(r16(cpu, wc16 + 14) as u16),
        r16(cpu, wc16 + 16),
        name(cpu, menu),
        name(cpu, class),
    ];
    let wc = thunk::scratch32(cpu, 40);
    for (i, f) in fields.into_iter().enumerate() {
        cpu.bus.guest_write_32(wc + 4 * i as u32, f);
    }
    go_at(
        cpu,
        wc,
        "RegisterClassA",
        &[wc],
        vec![P_CLASS_MENU, class, menu],
    )
}

/// UnregisterClass(name, instance): its menu's name forgotten too.
fn unregister_class(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = if args[0] >> 16 == 0 {
        args[0]
    } else {
        super::linear(cpu, args[0])
    };
    let instance = instance32(cpu, args[1] as u16);
    go_post(
        cpu,
        "UnregisterClassA",
        &[name, instance],
        vec![P_CLASS_MENU, args[0], 0, 1],
    )
}

/// A class's name (a 16:16 pointer, or an atom) as `class_menus` has it.
fn class_key(cpu: &Cpu, class: u32) -> String {
    if class >> 16 == 0 {
        format!("#{}", class)
    } else {
        super::string(cpu, super::linear(cpu, class)).to_ascii_uppercase()
    }
}

fn get_class_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let instance = instance32(cpu, args[0] as u16);
    let name32 = if args[1] >> 16 == 0 {
        args[1]
    } else {
        super::linear(cpu, args[1])
    };
    let key = class_key(cpu, args[1]);
    let menu = cpu
        .bus
        .win32
        .win16
        .class_menus
        .iter()
        .find(|(k, _)| *k == key)
        .map_or(0, |&(_, m)| m);
    let wc = thunk::scratch32(cpu, 40);
    go_at(
        cpu,
        wc,
        "GetClassInfoA",
        &[instance, name32, wc],
        vec![P_CLASSINFO, wc, args[2], menu, args[1]],
    )
}

/// CW_USEDEFAULT as Windows 3.1 has it, and as Win32 does.
const CW_USEDEFAULT16: u32 = 0xFFFF_8000;
const CW_USEDEFAULT: u32 = 0x8000_0000;
const WS_CHILD: u32 = 0x4000_0000;

fn create_window(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut all = vec![0];
    all.extend_from_slice(args);
    create_window_ex(cpu, &all)
}

/// CreateWindowEx(ex style, class, name, style, x, y, cx, cy, parent,
/// menu, instance, parameter).
fn create_window_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let style = args[3];
    let coord = |v: u32| {
        if v == CW_USEDEFAULT16 {
            CW_USEDEFAULT
        } else {
            v
        }
    };
    let menu = if style & WS_CHILD != 0 {
        args[9]
    } else {
        menu32(args[9] as u16)
    };
    let instance = instance32(cpu, args[10] as u16);
    // An MDI client's CLIENTCREATESTRUCT, which USER32 reads flat: the
    // window menu's handle and the first child's id, widened.
    let top = thunk::scratch32(cpu, 8);
    let mut params = args[11];
    if params != 0
        && args[1] >> 16 != 0
        && super::string(cpu, args[1]).eq_ignore_ascii_case("MDICLIENT")
    {
        let ccs = super::linear(cpu, params);
        let menu = menu32(super::read_16(cpu, ccs));
        let first = super::read_16(cpu, ccs + 2) as u32;
        super::write_32(cpu, top, menu);
        super::write_32(cpu, top + 4, first);
        params = top;
    }
    go_at(
        cpu,
        top,
        "CreateWindowExA",
        &[
            args[0],
            args[1],
            args[2],
            style,
            coord(args[4]),
            coord(args[5]),
            coord(args[6]),
            coord(args[7]),
            args[8],
            menu,
            instance,
            params,
        ],
        Vec::new(),
    )
}

/// The window long's indexes that hold procedures and handles.
const GWL_WNDPROC: u32 = (-4i32) as u32;
const GWL_HINSTANCE: u32 = (-6i32) as u32;
const DWL_DLGPROC: u32 = 4;
const GCL_WNDPROC: u32 = (-24i32) as u32;
const GCW_HCURSOR: u32 = (-12i32) as u32;
const GCW_HICON: u32 = (-14i32) as u32;
const GCW_HMODULE: u32 = (-16i32) as u32;

/// Whether `hwnd` is a dialog box (its DWL_DLGPROC a dialog procedure).
fn is_dialog(cpu: &Cpu, hwnd: u32) -> bool {
    crate::win32::user32::dialog::is_dialog(&cpu.bus.win32.user, hwnd)
}

fn get_window_long(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, index) = (args[0], args[1]);
    let data = match index {
        GWL_WNDPROC => vec![P_PROC16, KIND_WNDPROC as u32],
        DWL_DLGPROC if is_dialog(cpu, hwnd) => vec![P_PROC16, KIND_DLGPROC as u32],
        GWL_HINSTANCE => vec![P_INSTANCE],
        GWL_EXSTYLE => vec![P_EX_STYLE],
        _ => Vec::new(),
    };
    go_post(cpu, "GetWindowLongA", &[hwnd, index], data)
}

fn set_window_long(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, index, value) = (args[0], args[1], args[2]);
    let instance = window_instance(cpu, hwnd);
    let (value, data) = match index {
        GWL_WNDPROC => (
            proc32_of(cpu, value, KIND_WNDPROC, instance),
            vec![P_PROC16, KIND_WNDPROC as u32],
        ),
        DWL_DLGPROC if is_dialog(cpu, hwnd) => (
            proc32_of(cpu, value, KIND_DLGPROC, instance),
            vec![P_PROC16, KIND_DLGPROC as u32],
        ),
        GWL_HINSTANCE => (instance32(cpu, value as u16), vec![P_INSTANCE]),
        _ => (value, Vec::new()),
    };
    go_post(cpu, "SetWindowLongA", &[hwnd, index, value], data)
}

/// The 16-bit instance of the window `hwnd`, or the running task's.
fn window_instance(cpu: &Cpu, hwnd: u32) -> u16 {
    let instance = cpu
        .bus
        .win32
        .user
        .window(hwnd)
        .map_or(0, |w| instance16(cpu, w.instance));
    if instance != 0 {
        return instance;
    }
    super::current_task(cpu).map_or(0, |t| cpu.bus.win32.win16.tasks[t].instance)
}

fn get_window_word(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, index) = (args[0], args[1]);
    match index {
        GWL_HINSTANCE => go_post(cpu, "GetWindowLongA", &[hwnd, index], vec![P_INSTANCE]),
        i if i as i32 >= 0 => go(cpu, "GetWindowWord", &[hwnd, index]),
        _ => go(cpu, "GetWindowLongA", &[hwnd, index]),
    }
}

fn set_window_word(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, index, value) = (args[0], args[1], args[2]);
    match index {
        GWL_HINSTANCE => {
            let value = instance32(cpu, value as u16);
            go_post(
                cpu,
                "SetWindowLongA",
                &[hwnd, index, value],
                vec![P_INSTANCE],
            )
        }
        i if i as i32 >= 0 => go(cpu, "SetWindowWord", &[hwnd, index, value]),
        _ => go(cpu, "SetWindowLongA", &[hwnd, index, value]),
    }
}

fn get_class_long(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let data = if args[1] == GCL_WNDPROC {
        vec![P_PROC16, KIND_WNDPROC as u32]
    } else {
        Vec::new()
    };
    go_post(cpu, "GetClassLongA", &[args[0], args[1]], data)
}

fn set_class_long(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, index, mut value) = (args[0], args[1], args[2]);
    let mut data = Vec::new();
    if index == GCL_WNDPROC {
        let instance = window_instance(cpu, hwnd);
        value = proc32_of(cpu, value, KIND_WNDPROC, instance);
        data = vec![P_PROC16, KIND_WNDPROC as u32];
    }
    go_post(cpu, "SetClassLongA", &[hwnd, index, value], data)
}

fn get_class_word(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, index) = (args[0], args[1]);
    let data = match index {
        GCW_HICON | GCW_HCURSOR => vec![P_ICON],
        GCW_HMODULE => vec![P_MODULE],
        _ => Vec::new(),
    };
    if index as i32 >= 0 {
        return go(cpu, "GetClassWord", &[hwnd, index]);
    }
    go_post(cpu, "GetClassLongA", &[hwnd, index], data)
}

fn set_class_word(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, index, value) = (args[0], args[1], args[2]);
    if index as i32 >= 0 {
        return go(cpu, "SetClassWord", &[hwnd, index, value]);
    }
    let (value, data) = match index {
        GCW_HICON | GCW_HCURSOR => (icon32(value as u16), vec![P_ICON]),
        GCW_HMODULE => (instance32(cpu, value as u16), vec![P_INSTANCE]),
        _ => (value, Vec::new()),
    };
    go_post(cpu, "SetClassLongA", &[hwnd, index, value], data)
}

fn window_from_point(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    go(cpu, "WindowFromPoint", &[sx(args[0]), sx(args[0] >> 16)])
}

fn child_window_from_point(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    go(
        cpu,
        "ChildWindowFromPoint",
        &[args[0], sx(args[1]), sx(args[1] >> 16)],
    )
}

fn pt_in_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = args[0];
    let side = |i: u32| r16(cpu, r + 2 * i) as u16 as i16;
    let (x, y) = (args[1] as u16 as i16, (args[1] >> 16) as u16 as i16);
    Some((x >= side(0) && x < side(2) && y >= side(1) && y < side(3)) as u32)
}

/// EnumWindows' callbacks: (hwnd, lParam), a BOOL back in AX only (bit
/// 16).
const ENUM_PROC: u32 = 2 | 0b10 << 8 | 0x1_0000;

fn enum_windows(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let proc = thunk::proc32(cpu, args[0], KIND_PLAIN, ENUM_PROC);
    go(cpu, "EnumWindows", &[proc, args[1]])
}

/// CreateIcon(instance, width, height, planes, bits per pixel, AND
/// mask, XOR bits): planes and bits per pixel are bytes, whose words'
/// high bytes are whatever the caller had there.
fn create_icon(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut args32 = args.to_vec();
    args32[0] = instance32(cpu, args[0] as u16);
    args32[3] &= 0xFF;
    args32[4] &= 0xFF;
    thunk::call32(cpu, api("CreateIcon"), &args32, thunk::POST_AUTO, vec![b'i' as u32]);
    None
}

/// CreateCursor(instance, hot spot x and y, width, height, AND mask, XOR
/// mask).
fn create_cursor(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut args32 = args.to_vec();
    args32[0] = instance32(cpu, args[0] as u16);
    thunk::call32(cpu, api("CreateCursor"), &args32, thunk::POST_AUTO, vec![b'i' as u32]);
    None
}

fn enum_child_windows(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let proc = thunk::proc32(cpu, args[1], KIND_PLAIN, ENUM_PROC);
    go(cpu, "EnumChildWindows", &[args[0], proc, args[2]])
}

fn enum_task_windows(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let task = args[0] as u16;
    let tid = cpu
        .bus
        .win32
        .win16
        .tasks
        .iter()
        .find(|t| t.handle == task)
        .map_or(0, |t| t.tid);
    let proc = thunk::proc32(cpu, args[1], KIND_PLAIN, ENUM_PROC);
    go(cpu, "EnumThreadWindows", &[tid, proc, args[2]])
}

fn get_window_task(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let tid = cpu.bus.win32.user.window(args[0]).map_or(0, |w| w.tid);
    Some(
        cpu.bus
            .win32
            .win16
            .tasks
            .iter()
            .find(|t| t.tid == tid)
            .map_or(0, |t| t.handle) as u32,
    )
}

/// SetTimer's callback: (hwnd, msg, id, time).
const TIMER_PROC: u32 = 4 | 0b1000 << 8;

fn set_timer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let proc = thunk::proc32(cpu, args[3], KIND_PLAIN, TIMER_PROC);
    go(cpu, "SetTimer", &[args[0], args[1], args[2], proc])
}

/// SetWindowsHook and SetWindowsHookEx: 16-bit hooks aren't called; a
/// handle that UnhookWindowsHookEx takes.
fn set_windows_hook(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    cpu.bus.log_string(&format!(
        "[W16] A hook of kind {} isn't kept",
        args[0] as i32
    ));
    Some(0x0001_0001)
}

// Painting.

fn begin_paint(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let ps = thunk::scratch32(cpu, 64);
    go_at(
        cpu,
        ps,
        "BeginPaint",
        &[args[0], ps],
        vec![P_PAINT, ps, args[1]],
    )
}

fn end_paint(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let ps = thunk::scratch32(cpu, 64);
    let ps16 = args[1];
    let mut fields = vec![r16(cpu, ps16), r16(cpu, ps16 + 2)];
    for i in 0..4 {
        fields.push(sx(r16(cpu, ps16 + 4 + 2 * i)));
    }
    fields.extend([r16(cpu, ps16 + 12), r16(cpu, ps16 + 14)]);
    fields.resize(16, 0);
    for (i, f) in fields.into_iter().enumerate() {
        cpu.bus.guest_write_32(ps + 4 * i as u32, f);
    }
    go_at(cpu, ps, "EndPaint", &[args[0], ps], Vec::new())
}

fn get_scroll_range(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = thunk::scratch32(cpu, 8);
    let data = vec![P_SCROLL_RANGE, s, args[2], args[3]];
    go_at(
        cpu,
        s,
        "GetScrollRange",
        &[args[0], args[1], s, s + 4],
        data,
    )
}

fn map_window_points(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let n = args[3].min(0x400);
    let p = thunk::scratch32(cpu, 8 * n.max(1));
    for i in 0..2 * n {
        let v = sx(r16(cpu, args[2] + 2 * i));
        cpu.bus.guest_write_32(p + 4 * i, v);
    }
    go_at(
        cpu,
        p,
        "MapWindowPoints",
        &[args[0], args[1], p, n],
        vec![P_POINTS, p, args[2], n],
    )
}

fn gray_string(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, brush, proc, data) = (args[0], args[1], args[2], args[3]);
    let rest = [args[4], args[5], args[6], args[7], args[8]];
    let (proc, data) = if proc == 0 {
        (0, super::linear(cpu, data))
    } else {
        // (hdc, data, count), a BOOL back.
        (thunk::proc32(cpu, proc, KIND_PLAIN, 3 | 0b10 << 8), data)
    };
    let mut all = vec![hdc, brush, proc, data];
    all.extend(rest);
    go(cpu, "GrayStringA", &all)
}

/// Copy `n` 16-bit tab stops at `at16` to 32-bit ones on the 32-bit
/// stack: their address, 0 for none.
fn tabs32(cpu: &mut Cpu, at16: u32, n: u32) -> (u32, u32) {
    let n = n.min(0x100);
    let at = thunk::scratch32(cpu, 4 * n.max(1));
    if at16 == 0 || n == 0 {
        return (at, 0);
    }
    for i in 0..n {
        let v = sx(r16(cpu, at16 + 2 * i));
        cpu.bus.guest_write_32(at + 4 * i, v);
    }
    (at, at)
}

fn tabbed_text_out(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (top, tabs) = tabs32(cpu, args[6], args[5]);
    go_at(
        cpu,
        top,
        "TabbedTextOutA",
        &[
            args[0], args[1], args[2], args[3], args[4], args[5], tabs, args[7],
        ],
        Vec::new(),
    )
}

fn get_tabbed_text_extent(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (top, tabs) = tabs32(cpu, args[4], args[3]);
    go_at(
        cpu,
        top,
        "GetTabbedTextExtentA",
        &[args[0], args[1], sx(args[2]), args[3], tabs],
        Vec::new(),
    )
}

fn get_window_placement(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let wp = thunk::scratch32(cpu, 44);
    cpu.bus.guest_write_32(wp, 44);
    go_at(
        cpu,
        wp,
        "GetWindowPlacement",
        &[args[0], wp],
        vec![P_PLACEMENT, wp, args[1]],
    )
}

fn set_window_placement(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let wp = thunk::scratch32(cpu, 44);
    for i in 0..11 {
        let v = r16(cpu, args[1] + 2 * i);
        let v = if i >= 3 { sx(v) } else { v };
        cpu.bus.guest_write_32(wp + 4 * i, v);
    }
    cpu.bus.guest_write_32(wp, 44);
    go_at(cpu, wp, "SetWindowPlacement", &[args[0], wp], Vec::new())
}

/// SPI_GETWORKAREA, whose RECT is 16-bit.
const SPI_GETWORKAREA: u32 = 48;

fn system_parameters_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = thunk::scratch32(cpu, 64);
    let data = if args[0] == SPI_GETWORKAREA {
        vec![P_RECT, s, args[2]]
    } else {
        vec![P_WORDS, s, args[2], (args[2] != 0) as u32]
    };
    if args[2] != 0
        && let Some(bytes) = super::peek(cpu, args[2], 2)
    {
        let v = u16::from_le_bytes([bytes[0], bytes[1]]) as u32;
        cpu.bus.guest_write_32(s, v);
    }
    go_at(
        cpu,
        s,
        "SystemParametersInfoA",
        &[args[0], args[1], s, args[3]],
        data,
    )
}

// Dialogs.

fn dialog_box(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let instance = args[0] as u16;
    let proc = proc32_of(cpu, args[3], KIND_DLGPROC, instance);
    let instance32 = instance32(cpu, instance);
    let param = args.get(4).copied().unwrap_or(0);
    go(
        cpu,
        "DialogBoxParamA",
        &[instance32, args[1], args[2], proc, param],
    )
}

fn create_dialog(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let instance = args[0] as u16;
    let proc = proc32_of(cpu, args[3], KIND_DLGPROC, instance);
    let instance32 = instance32(cpu, instance);
    let param = args.get(4).copied().unwrap_or(0);
    go(
        cpu,
        "CreateDialogParamA",
        &[instance32, args[1], args[2], proc, param],
    )
}

/// A 16-bit dialog template at `at`, as a Win32 one on the 32-bit stack:
/// its address, which is the stack's top for the call.
fn template32(cpu: &mut Cpu, at: u32, max: u32) -> u32 {
    let bytes = super::peek_upto(cpu, at, max as usize);
    let converted = super::resource::dialog32(&bytes);
    let to = thunk::scratch32(cpu, converted.len() as u32 + 4);
    let at = (to + 3) & !3;
    for (i, b) in converted.into_iter().enumerate() {
        cpu.bus.guest_write_8(at + i as u32, b);
    }
    to
}

fn dialog_box_indirect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let instance = args[0] as u16;
    let block = super::global::handle_to_sel(args[1] as u16);
    let at = super::lin(cpu, block, 0);
    let size = super::global::size(cpu, args[1] as u16).clamp(1, 0x8000);
    let proc = proc32_of(cpu, args[3], KIND_DLGPROC, instance);
    let instance32 = instance32(cpu, instance);
    let template = template32(cpu, at, size);
    let param = args.get(4).copied().unwrap_or(0);
    go_at(
        cpu,
        template,
        "DialogBoxIndirectParamA",
        &[instance32, (template + 3) & !3, args[2], proc, param],
        Vec::new(),
    )
}

fn create_dialog_indirect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let instance = args[0] as u16;
    let proc = proc32_of(cpu, args[3], KIND_DLGPROC, instance);
    let instance32 = instance32(cpu, instance);
    // The template goes to the end of its segment at most.
    let (at, offset) = (super::linear(cpu, args[1]), args[1] & 0xFFFF);
    let size = super::selector_base(cpu, (args[1] >> 16) as u16)
        .map_or(0, |(_, limit, _)| (limit + 1).saturating_sub(offset))
        .min(0x8000);
    let template = template32(cpu, at, size);
    let param = args.get(4).copied().unwrap_or(0);
    go_at(
        cpu,
        template,
        "CreateDialogIndirectParamA",
        &[instance32, (template + 3) & !3, args[2], proc, param],
        Vec::new(),
    )
}

fn set_dlg_item_int(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let value = if args[3] != 0 { sx(args[2]) } else { args[2] };
    go(cpu, "SetDlgItemInt", &[args[0], args[1], value, args[3]])
}

/// GetDlgItemInt(dialog, id, translated, signed): the control's text
/// read with GetDlgItemTextA, then taken as Windows 3.1 does
/// (`dlg_int`).
fn get_dlg_item_int(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let buf = thunk::scratch32(cpu, DLG_INT_TEXT);
    go_at(
        cpu,
        buf,
        "GetDlgItemTextA",
        &[args[0], args[1], buf, DLG_INT_TEXT],
        vec![P_DLG_INT, buf, args[2], args[3]],
    )
}

/// The bytes of a control's text GetDlgItemInt reads.
const DLG_INT_TEXT: u32 = 64;

/// GetDlgItemInt's number in `text`: blanks around it, a sign if
/// `signed`, the digits, and a value that fits a 16-bit int (or word);
/// None if the text is anything else.
fn dlg_int(text: &[u8], signed: bool) -> Option<u16> {
    let text = text.trim_ascii();
    let (negative, digits) = match text.first() {
        Some(b'-') if signed => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut n: i64 = 0;
    for &d in digits {
        n = (n * 10 + (d - b'0') as i64).min(1 << 20);
    }
    let v = if negative { -n } else { n };
    let fits = if signed {
        (-0x8000..=0x7FFF).contains(&v)
    } else {
        (0..=0xFFFF).contains(&v)
    };
    fits.then_some(v as u16)
}

fn dlg_dir_select(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    go(cpu, "DlgDirSelectExA", &[args[0], args[1], 128, args[2]])
}

/// The child of `dialog` whose id is `id`.
fn dlg_item(cpu: &Cpu, dialog: u32, id: u32) -> u32 {
    let user = &cpu.bus.win32.user;
    user.window(dialog).map_or(0, |w| {
        w.children
            .iter()
            .copied()
            .find(|&c| {
                user.window(c)
                    .is_some_and(|cw| cw.id & 0xFFFF == id & 0xFFFF)
            })
            .unwrap_or(0)
    })
}

fn send_dlg_item_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hwnd = dlg_item(cpu, args[0], args[1]);
    if hwnd == 0 {
        return Some(0);
    }
    message::send32(cpu, api("SendMessageA"), hwnd, args[2], args[3], args[4]);
    None
}

fn def_window_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    message::send32_fn(cpu, "DefWindowProcA", args);
    None
}

fn def_dlg_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    message::send32_fn(cpu, "DefDlgProcA", args);
    None
}

fn def_mdi_child_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    message::send32_fn(cpu, "DefMDIChildProcA", args);
    None
}

fn def_frame_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    message::send32_with(
        cpu,
        api("DefFrameProcA"),
        args[0],
        &[args[1]],
        args[2],
        args[3],
        args[4],
    );
    None
}

/// CallWindowProc(proc, hwnd, msg, wParam, lParam): a 32-bit procedure
/// (a slot standing for one) gets the message translated; a 16-bit one
/// is called as it is, by jumping to it with the caller's frame in place
/// of this call's.
fn call_window_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let proc = args[0];
    if let Some(t) = thunk::thunk16_of(cpu, proc) {
        message::send32(cpu, t.target, args[1], args[2], args[3], args[4]);
        return None;
    }
    // The stack: the return address, lParam, wParam, msg, hwnd, proc.
    // The return address moves up over the procedure's place, and the
    // procedure returns to the caller, taking its 10 bytes off.
    let mut ctx = Context::of(cpu);
    let base = cpu.seg_cache(Seg::SS).base;
    let sp = cpu.sp();
    let at = |off: u16| base.wrapping_add(sp.wrapping_add(off) as u32);
    let ret = super::read_32(cpu, at(0));
    let frame: Vec<u8> = super::peek(cpu, at(4), 10).unwrap_or_default();
    super::write_32(cpu, at(4), ret);
    super::poke(cpu, at(8), &frame);
    thunk::set_sp(&mut ctx, sp.wrapping_add(4));
    ctx.eip = proc & 0xFFFF;
    ctx.seg[Seg::CS as usize] = (proc >> 16) as u16;
    let ds = ctx.seg[Seg::DS as usize];
    thunk::set_ax(&mut ctx, ds);
    crate::win32::system::load_context(cpu, &ctx);
    None
}

fn post_app_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let task = args[0] as u16;
    let tid = cpu
        .bus
        .win32
        .win16
        .tasks
        .iter()
        .find(|t| t.handle == task)
        .map_or(0, |t| t.tid);
    go(cpu, "PostThreadMessageA", &[tid, args[1], args[2], args[3]])
}

// The message loop: MSG16s.

fn get_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let m = thunk::scratch32(cpu, 28);
    go_at(
        cpu,
        m,
        "GetMessageA",
        &[m, args[1], args[2], args[3]],
        vec![P_MSG, m, args[0]],
    )
}

fn peek_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let m = thunk::scratch32(cpu, 28);
    go_at(
        cpu,
        m,
        "PeekMessageA",
        &[m, args[1], args[2], args[3], args[4]],
        vec![P_PEEK, m, args[0], args[4]],
    )
}

/// A MSG16 at `m16` as a MSG on the 32-bit stack: its address, and the
/// stack's top below the structures its message took (the top for
/// `go_at`).
fn msg_arg(cpu: &mut Cpu, m16: u32) -> (u32, u32) {
    let m = thunk::scratch32(cpu, 28);
    let top = message::msg32(cpu, m16, m);
    (m, top)
}

fn translate_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (m, top) = msg_arg(cpu, args[0]);
    go_at(cpu, top, "TranslateMessage", &[m], Vec::new())
}

fn dispatch_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (m, top) = msg_arg(cpu, args[0]);
    go_at(cpu, top, "DispatchMessageA", &[m], Vec::new())
}

fn is_dialog_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (m, top) = msg_arg(cpu, args[1]);
    go_at(cpu, top, "IsDialogMessageA", &[args[0], m], Vec::new())
}

fn translate_accelerator(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (m, top) = msg_arg(cpu, args[2]);
    let accel = accel32(args[1] as u16);
    go_at(
        cpu,
        top,
        "TranslateAcceleratorA",
        &[args[0], accel, m],
        Vec::new(),
    )
}

fn translate_mdi_sys_accel(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (m, top) = msg_arg(cpu, args[1]);
    go_at(cpu, top, "TranslateMDISysAccel", &[args[0], m], Vec::new())
}

// Menus.

const MF_BITMAP: u32 = 0x0004;
const MF_OWNERDRAW: u32 = 0x0100;
const MF_POPUP: u32 = 0x0010;
const MF_SEPARATOR: u32 = 0x0800;

/// A menu item's id (a popup's menu) and its content (a string's flat
/// address) as AppendMenuA takes them.
fn menu_item(cpu: &Cpu, flags: u32, id: u32, item: u32) -> (u32, u32) {
    let id = if flags & MF_POPUP != 0 {
        menu32(id as u16)
    } else {
        id
    };
    let item = if flags & (MF_BITMAP | MF_OWNERDRAW | MF_SEPARATOR) == 0 {
        super::linear(cpu, item)
    } else if flags & MF_BITMAP != 0 {
        item & 0xFFFF
    } else {
        item
    };
    (id, item)
}

fn append_menu(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, item) = menu_item(cpu, args[1], args[2], args[3]);
    go(
        cpu,
        "AppendMenuA",
        &[menu32(args[0] as u16), args[1], id, item],
    )
}

fn insert_menu(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, item) = menu_item(cpu, args[2], args[3], args[4]);
    go(
        cpu,
        "InsertMenuA",
        &[menu32(args[0] as u16), args[1], args[2], id, item],
    )
}

fn modify_menu(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, item) = menu_item(cpu, args[2], args[3], args[4]);
    go(
        cpu,
        "ModifyMenuA",
        &[menu32(args[0] as u16), args[1], args[2], id, item],
    )
}

/// A menu item's id or place in the menu `in_menu` for USER32, as
/// `flags` says which it is: by command, a popup's id is its menu, whose
/// 16-bit handle the program gives, when no item has that id itself.
fn item32(cpu: &Cpu, in_menu: u32, item: u32, flags: u32) -> u32 {
    use crate::win32::user32::menu::{exists, has_command};
    const MF_BYPOSITION: u32 = 0x0400;
    let menu = menu32(item as u16);
    let user = &cpu.bus.win32.user;
    if flags & MF_BYPOSITION == 0
        && menu != item
        && exists(user, menu)
        && !has_command(user, in_menu, item)
    {
        menu
    } else {
        item
    }
}

/// A USER32 function of (menu, item, flags, ...), the menu at `menu` in
/// `args` and the item and its flags at `item` and `flags`.
fn menu_item_call(
    cpu: &mut Cpu,
    name: &str,
    args: &[u32],
    menu: usize,
    item: usize,
    flags: usize,
) -> Option<u32> {
    let mut a = args.to_vec();
    a[menu] = menu32(a[menu] as u16);
    a[item] = item32(cpu, a[menu], a[item], a[flags]);
    go(cpu, name, &a)
}

fn check_menu_item(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    menu_item_call(cpu, "CheckMenuItem", args, 0, 1, 2)
}

fn enable_menu_item(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    menu_item_call(cpu, "EnableMenuItem", args, 0, 1, 2)
}

fn get_menu_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    menu_item_call(cpu, "GetMenuState", args, 0, 1, 2)
}

fn remove_menu(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    menu_item_call(cpu, "RemoveMenu", args, 0, 1, 2)
}

fn delete_menu(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    menu_item_call(cpu, "DeleteMenu", args, 0, 1, 2)
}

fn get_menu_string(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    menu_item_call(cpu, "GetMenuStringA", args, 0, 1, 4)
}

/// HiliteMenuItem(hwnd, menu, item, flags): it fails without a window or
/// a menu.
fn hilite_menu_item(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 || args[1] == 0 {
        return Some(0);
    }
    menu_item_call(cpu, "HiliteMenuItem", args, 1, 2, 3)
}

/// ChangeMenu(menu, position, item, id, flags), Windows 2's way to add,
/// change and remove items: the flag MF_APPEND, MF_DELETE, MF_CHANGE or
/// MF_REMOVE picks the function, inserting when there is none of them.
fn change_menu(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    const MF_CHANGE: u32 = 0x0080;
    const MF_APPEND: u32 = 0x0100;
    const MF_DELETE: u32 = 0x0200;
    const MF_REMOVE: u32 = 0x1000;
    const MF_BYPOSITION: u32 = 0x0400;
    let (menu, position, data, id, flags) = (args[0], args[1], args[2], args[3], args[4]);
    if flags & MF_APPEND != 0 {
        append_menu(cpu, &[menu, flags & !MF_APPEND, id, data])
    } else if flags & MF_DELETE != 0 {
        let flags = flags & !MF_DELETE;
        go(cpu, "DeleteMenu", &[menu32(menu as u16), position, flags])
    } else if flags & MF_CHANGE != 0 {
        modify_menu(cpu, &[menu, position, flags & !MF_CHANGE, id, data])
    } else if flags & MF_REMOVE != 0 {
        let item = if flags & MF_BYPOSITION != 0 {
            position
        } else {
            id
        };
        go(
            cpu,
            "RemoveMenu",
            &[menu32(menu as u16), item, flags & !MF_REMOVE],
        )
    } else {
        insert_menu(cpu, &[menu, position, flags, id, data])
    }
}

// Strings.

/// wsprintf(buffer, format, ...): Windows 3.1's, its arguments 16-bit
/// (`l` for a long, `s` a far pointer).
fn wsprintf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let base = cpu.seg_cache(Seg::SS).base;
    let list = base.wrapping_add(cpu.sp() as u32 + 12);
    Some(format16(cpu, args[0], args[1], list))
}

fn wvsprintf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let list = super::linear(cpu, args[2]);
    Some(format16(cpu, args[0], args[1], list))
}

/// Format the string `format` (16:16) with the arguments at the linear
/// `list` into `buffer` (16:16), at most 1024 bytes: its length.
fn format16(cpu: &mut Cpu, buffer: u32, format: u32, list: u32) -> u32 {
    let fmt = super::string_bytes(cpu, super::linear(cpu, format), 0x400);
    let mut out = Vec::new();
    let mut at = list;
    let mut i = 0;
    while i < fmt.len() {
        let c = fmt[i];
        i += 1;
        if c != b'%' {
            out.push(c);
            continue;
        }
        let (mut left, mut zero, mut prefix) = (false, false, false);
        while i < fmt.len() && matches!(fmt[i], b'-' | b'0' | b'#') {
            match fmt[i] {
                b'-' => left = true,
                b'0' => zero = true,
                _ => prefix = true,
            }
            i += 1;
        }
        let mut width = 0usize;
        while i < fmt.len() && fmt[i].is_ascii_digit() {
            // wsprintf's output is at most 1024 bytes anyway.
            width = (width * 10 + (fmt[i] - b'0') as usize).min(1024);
            i += 1;
        }
        let mut precision = None;
        if i < fmt.len() && fmt[i] == b'.' {
            i += 1;
            let mut p = 0usize;
            while i < fmt.len() && fmt[i].is_ascii_digit() {
                p = (p * 10 + (fmt[i] - b'0') as usize).min(1024);
                i += 1;
            }
            precision = Some(p);
        }
        let mut long = false;
        if i < fmt.len() && matches!(fmt[i], b'l' | b'L') {
            long = true;
            i += 1;
        } else if i < fmt.len() && fmt[i] == b'h' {
            i += 1;
        }
        let Some(&kind) = fmt.get(i) else {
            break;
        };
        i += 1;
        let mut next = |long: bool| {
            let v = if long { r32(cpu, at) } else { r16(cpu, at) };
            at += if long { 4 } else { 2 };
            v
        };
        let text: Vec<u8> = match kind {
            b'd' | b'i' => {
                let v = next(long);
                let v = if long {
                    v as i32
                } else {
                    v as u16 as i16 as i32
                };
                v.to_string().into_bytes()
            }
            b'u' => next(long).to_string().into_bytes(),
            b'x' | b'X' => {
                let v = next(long);
                let s = if kind == b'x' {
                    format!("{:x}", v)
                } else {
                    format!("{:X}", v)
                };
                let mut s = s.into_bytes();
                if prefix && v != 0 {
                    let mut p = if kind == b'x' {
                        b"0x".to_vec()
                    } else {
                        b"0X".to_vec()
                    };
                    p.append(&mut s);
                    s = p;
                }
                s
            }
            b'c' => vec![next(false) as u8],
            b's' => {
                let p = next(true);
                let mut s = super::string_bytes(cpu, super::linear(cpu, p), 0x400);
                if let Some(n) = precision {
                    s.truncate(n);
                }
                s
            }
            b'%' => vec![b'%'],
            other => vec![b'%', other],
        };
        let mut text = text;
        // Zeros go after a minus sign.
        let sign = usize::from(text.first() == Some(&b'-'));
        if matches!(kind, b'd' | b'i' | b'u' | b'x' | b'X')
            && let Some(p) = precision
        {
            while text.len() - sign < p {
                text.insert(sign, b'0');
            }
        }
        if text.len() < width {
            let pad = width - text.len();
            if left {
                text.extend(std::iter::repeat_n(b' ', pad));
            } else if zero {
                for _ in 0..pad {
                    text.insert(sign, b'0');
                }
            } else {
                for _ in 0..pad {
                    text.insert(0, b' ');
                }
            }
        }
        out.extend_from_slice(&text);
    }
    out.truncate(1023);
    let len = out.len() as u32;
    out.push(0);
    let to = super::linear(cpu, buffer);
    super::poke(cpu, to, &out);
    len
}

fn change_case(cpu: &mut Cpu, at: u32, len: usize, upper: bool) {
    let mut s = super::peek(cpu, at, len).unwrap_or_default();
    if let Some(n) = s.iter().position(|&b| b == 0) {
        s.truncate(n);
    }
    for b in s.iter_mut() {
        *b = ansi_case(*b, upper);
    }
    super::poke(cpu, at, &s);
}

fn ansi_upper(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] >> 16 == 0 {
        return Some(ansi_case(args[0] as u8, true) as u32);
    }
    let at = super::linear(cpu, args[0]);
    let len = super::string_bytes(cpu, at, 0x1_0000).len();
    change_case(cpu, at, len, true);
    Some(args[0])
}

fn ansi_lower(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] >> 16 == 0 {
        return Some(ansi_case(args[0] as u8, false) as u32);
    }
    let at = super::linear(cpu, args[0]);
    let len = super::string_bytes(cpu, at, 0x1_0000).len();
    change_case(cpu, at, len, false);
    Some(args[0])
}

fn ansi_upper_buff(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let len = if args[1] == 0 {
        0x1_0000
    } else {
        args[1] as usize
    };
    let mut s = super::peek(cpu, args[0], len).unwrap_or_default();
    s.iter_mut().for_each(|b| *b = ansi_case(*b, true));
    super::poke(cpu, args[0], &s);
    Some(args[1])
}

fn ansi_lower_buff(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let len = if args[1] == 0 {
        0x1_0000
    } else {
        args[1] as usize
    };
    let mut s = super::peek(cpu, args[0], len).unwrap_or_default();
    s.iter_mut().for_each(|b| *b = ansi_case(*b, false));
    super::poke(cpu, args[0], &s);
    Some(args[1])
}

fn ansi_next(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = super::linear(cpu, args[0]);
    let c = super::peek(cpu, at, 1).map_or(0, |b| b[0]);
    Some(if c == 0 {
        args[0]
    } else {
        args[0].wrapping_add(1)
    })
}

fn ansi_prev(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(if args[1] & 0xFFFF > args[0] & 0xFFFF {
        args[1] - 1
    } else {
        args[0]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What Windows 98 SE's GetSystemMetrics gives a Windows 3.1
    /// program: SM_CYMENU 18, SM_CYCAPTION 20 and SM_CYFRAME 4, where a
    /// Win32 program gets 19, 19 and 4.
    #[test]
    fn windows_3_programs_get_windows_3_caption_and_menu_heights() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        assert_eq!(get_system_metrics(&mut cpu, &[15]), Some(18), "SM_CYMENU");
        assert_eq!(get_system_metrics(&mut cpu, &[4]), Some(20), "SM_CYCAPTION");
        assert_eq!(get_system_metrics(&mut cpu, &[33]), Some(4), "SM_CYFRAME");
    }
}
