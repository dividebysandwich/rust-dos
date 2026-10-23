//! The toolbar (ToolbarWindow32): a row of buttons, each with an image
//! of an image list and a text of its own or of the toolbar's string
//! pool, that send WM_COMMAND to the toolbar's parent when clicked.
//! Buttons can be check boxes, members of a radio group, separators or
//! drop-down buttons; the toolbar wraps them in rows (TBSTYLE_WRAPABLE
//! or TBSTATE_WRAP), sizes itself to its parent's width
//! (TB_AUTOSIZE), tracks the hot button of a flat toolbar, and shows
//! the buttons' tooltips through a tooltip control of its own
//! (TBSTYLE_TOOLTIPS), whose texts it asks of its parent with
//! TBN_GETINFOTIPA and TTN_GETDISPINFOA. Also here: CreateToolbarEx,
//! CreateToolbar and CreateMappedBitmap.
//!
//! The standard bitmaps (HINST_COMMCTRL) are Wine's, in `res/`, made
//! 8 bits a pixel with the button face's gray where they are
//! transparent.
//!
//! The toolbar follows Wine's, which follows COMCTL32 5.80: the
//! messages and notifications COMCTL32 4.71 and 5.80 added
//! (TB_GETSTRING, TBN_HOTITEMCHANGE, TBN_GETINFOTIP, TBN_GETDISPINFO
//! and others) are answered too. Left out: the customization dialog
//! (TB_CUSTOMIZE answers 0), TB_SAVERESTORE (0), NM_CUSTOMDRAW, the
//! drag of buttons with Alt (TBSTYLE_ALTDRAG; Shift works), the pager
//! notifications. A flat toolbar learns that the mouse left it from a
//! timer, as there is no TrackMouseEvent.
//!
//! Ported from Wine's toolbar.c and commctrl.c:
//!
//! Copyright 1998, 1999 Eric Kohl
//! Copyright 2000 Eric Kohl for CodeWeavers
//! Copyright 2004 Robert Shearman
//! Copyright 1997 Dimitrie O. Paun
//! Copyright 1998, 2000 Eric Kohl
//!
//! This library is free software; you can redistribute it and/or modify
//! it under the terms of the GNU Lesser General Public License as
//! published by the Free Software Foundation; either version 2.1 of the
//! License, or (at your option) any later version.

use super::misc::{heap_alloc, heap_free, process_heap};
use super::script::{self, Arg, Ctx, Op, Script};
use super::{DRAW, TOOLBAR, call_builtin, gfx, imagelist, state, tooltips};
use crate::cpu::Cpu;
use crate::win32::gdi32::{self, Rect};
use crate::win32::kernel32;
use crate::win32::user32::message::{
    WM_CHARTOITEM, WM_COMMAND, WM_CREATE, WM_DESTROY, WM_DRAWITEM, WM_ERASEBKGND, WM_GETFONT,
    WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MEASUREITEM, WM_MOUSEMOVE,
    WM_NCCALCSIZE, WM_NCCREATE, WM_NCPAINT, WM_NOTIFY, WM_NOTIFYFORMAT, WM_PAINT, WM_PRINTCLIENT,
    WM_RBUTTONDBLCLK, WM_RBUTTONUP, WM_SETFOCUS, WM_SETFONT, WM_SETREDRAW, WM_SIZE,
    WM_STYLECHANGED, WM_TIMER, WM_USER, WM_VKEYTOITEM,
};
use crate::win32::user32::window::{
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, WS_BORDER, WS_CHILD, WS_MINIMIZE, WS_POPUP,
};
use crate::win32::user32::{self, defwnd, read_words, sysparams, write_words};
use std::collections::BTreeMap;

// The messages.
const TB_ENABLEBUTTON: u32 = WM_USER + 1;
const TB_CHECKBUTTON: u32 = WM_USER + 2;
const TB_PRESSBUTTON: u32 = WM_USER + 3;
const TB_HIDEBUTTON: u32 = WM_USER + 4;
const TB_INDETERMINATE: u32 = WM_USER + 5;
const TB_MARKBUTTON: u32 = WM_USER + 6;
const TB_ISBUTTONENABLED: u32 = WM_USER + 9;
const TB_ISBUTTONCHECKED: u32 = WM_USER + 10;
const TB_ISBUTTONPRESSED: u32 = WM_USER + 11;
const TB_ISBUTTONHIDDEN: u32 = WM_USER + 12;
const TB_ISBUTTONINDETERMINATE: u32 = WM_USER + 13;
const TB_ISBUTTONHIGHLIGHTED: u32 = WM_USER + 14;
const TB_SETSTATE: u32 = WM_USER + 17;
const TB_GETSTATE: u32 = WM_USER + 18;
const TB_ADDBITMAP: u32 = WM_USER + 19;
const TB_ADDBUTTONSA: u32 = WM_USER + 20;
const TB_INSERTBUTTONA: u32 = WM_USER + 21;
const TB_DELETEBUTTON: u32 = WM_USER + 22;
const TB_GETBUTTON: u32 = WM_USER + 23;
const TB_BUTTONCOUNT: u32 = WM_USER + 24;
const TB_COMMANDTOINDEX: u32 = WM_USER + 25;
const TB_SAVERESTOREA: u32 = WM_USER + 26;
const TB_CUSTOMIZE: u32 = WM_USER + 27;
const TB_ADDSTRINGA: u32 = WM_USER + 28;
const TB_GETITEMRECT: u32 = WM_USER + 29;
const TB_BUTTONSTRUCTSIZE: u32 = WM_USER + 30;
const TB_SETBUTTONSIZE: u32 = WM_USER + 31;
const TB_SETBITMAPSIZE: u32 = WM_USER + 32;
const TB_AUTOSIZE: u32 = WM_USER + 33;
const TB_GETTOOLTIPS: u32 = WM_USER + 35;
const TB_SETTOOLTIPS: u32 = WM_USER + 36;
const TB_SETPARENT: u32 = WM_USER + 37;
const TB_SETROWS: u32 = WM_USER + 39;
const TB_GETROWS: u32 = WM_USER + 40;
const TB_GETBITMAPFLAGS: u32 = WM_USER + 41;
const TB_SETCMDID: u32 = WM_USER + 42;
const TB_CHANGEBITMAP: u32 = WM_USER + 43;
const TB_GETBITMAP: u32 = WM_USER + 44;
const TB_GETBUTTONTEXTA: u32 = WM_USER + 45;
const TB_REPLACEBITMAP: u32 = WM_USER + 46;
const TB_SETINDENT: u32 = WM_USER + 47;
const TB_SETIMAGELIST: u32 = WM_USER + 48;
const TB_GETIMAGELIST: u32 = WM_USER + 49;
const TB_LOADIMAGES: u32 = WM_USER + 50;
const TB_GETRECT: u32 = WM_USER + 51;
const TB_SETHOTIMAGELIST: u32 = WM_USER + 52;
const TB_GETHOTIMAGELIST: u32 = WM_USER + 53;
const TB_SETDISABLEDIMAGELIST: u32 = WM_USER + 54;
const TB_GETDISABLEDIMAGELIST: u32 = WM_USER + 55;
const TB_SETSTYLE: u32 = WM_USER + 56;
const TB_GETSTYLE: u32 = WM_USER + 57;
const TB_GETBUTTONSIZE: u32 = WM_USER + 58;
const TB_SETBUTTONWIDTH: u32 = WM_USER + 59;
const TB_SETMAXTEXTROWS: u32 = WM_USER + 60;
const TB_GETTEXTROWS: u32 = WM_USER + 61;
const TB_GETBUTTONINFOW: u32 = WM_USER + 63;
const TB_SETBUTTONINFOW: u32 = WM_USER + 64;
const TB_GETBUTTONINFOA: u32 = WM_USER + 65;
const TB_SETBUTTONINFOA: u32 = WM_USER + 66;
const TB_INSERTBUTTONW: u32 = WM_USER + 67;
const TB_ADDBUTTONSW: u32 = WM_USER + 68;
const TB_HITTEST: u32 = WM_USER + 69;
const TB_SETDRAWTEXTFLAGS: u32 = WM_USER + 70;
const TB_GETHOTITEM: u32 = WM_USER + 71;
const TB_SETHOTITEM: u32 = WM_USER + 72;
const TB_SETANCHORHIGHLIGHT: u32 = WM_USER + 73;
const TB_GETANCHORHIGHLIGHT: u32 = WM_USER + 74;
const TB_GETBUTTONTEXTW: u32 = WM_USER + 75;
const TB_SAVERESTOREW: u32 = WM_USER + 76;
const TB_ADDSTRINGW: u32 = WM_USER + 77;
const TB_MAPACCELERATORA: u32 = WM_USER + 78;
const TB_GETINSERTMARK: u32 = WM_USER + 79;
const TB_SETINSERTMARK: u32 = WM_USER + 80;
const TB_MOVEBUTTON: u32 = WM_USER + 82;
const TB_GETMAXSIZE: u32 = WM_USER + 83;
const TB_SETEXTENDEDSTYLE: u32 = WM_USER + 84;
const TB_GETEXTENDEDSTYLE: u32 = WM_USER + 85;
const TB_GETPADDING: u32 = WM_USER + 86;
const TB_SETPADDING: u32 = WM_USER + 87;
const TB_SETINSERTMARKCOLOR: u32 = WM_USER + 88;
const TB_GETINSERTMARKCOLOR: u32 = WM_USER + 89;
const TB_MAPACCELERATORW: u32 = WM_USER + 90;
const TB_GETSTRINGW: u32 = WM_USER + 91;
const TB_GETSTRINGA: u32 = WM_USER + 92;
const TB_SETHOTITEM2: u32 = WM_USER + 94;
const TB_SETLISTGAP: u32 = WM_USER + 96;
const TB_GETIMAGELISTCOUNT: u32 = WM_USER + 98;
const TB_GETIDEALSIZE: u32 = WM_USER + 99;

const CCM_SETCOLORSCHEME: u32 = 0x2002;
const CCM_GETCOLORSCHEME: u32 = 0x2003;
const CCM_SETUNICODEFORMAT: u32 = 0x2005;
const CCM_GETUNICODEFORMAT: u32 = 0x2006;
const CCM_SETVERSION: u32 = 0x2007;
const CCM_GETVERSION: u32 = 0x2008;

const WM_CAPTURECHANGED: u32 = 0x0215;

// The toolbar's styles, and the common controls' (CCS_).
const TBSTYLE_TOOLTIPS: u32 = 0x0100;
const TBSTYLE_WRAPABLE: u32 = 0x0200;
const TBSTYLE_ALTDRAG: u32 = 0x0400;
const TBSTYLE_FLAT: u32 = 0x0800;
const TBSTYLE_LIST: u32 = 0x1000;
const TBSTYLE_TRANSPARENT: u32 = 0x8000;
const CCS_TOP: u32 = 0x01;
const CCS_NOMOVEY: u32 = 0x02;
const CCS_BOTTOM: u32 = 0x03;
const CCS_NORESIZE: u32 = 0x04;
const CCS_NOPARENTALIGN: u32 = 0x08;
const CCS_ADJUSTABLE: u32 = 0x20;
const CCS_NODIVIDER: u32 = 0x40;
const CCS_VERT: u32 = 0x80;
const COMMON_STYLES: u32 = 0xEF;

const TBSTYLE_EX_DRAWDDARROWS: u32 = 0x01;
const TBSTYLE_EX_UNDOC1: u32 = 0x04;
const TBSTYLE_EX_MIXEDBUTTONS: u32 = 0x08;
const TBSTYLE_EX_HIDECLIPPEDBUTTONS: u32 = 0x10;

// The buttons' styles and states.
const BTNS_SEP: u32 = 0x01;
const BTNS_CHECK: u32 = 0x02;
const BTNS_GROUP: u32 = 0x04;
const BTNS_CHECKGROUP: u32 = BTNS_GROUP | BTNS_CHECK;
const BTNS_DROPDOWN: u32 = 0x08;
const BTNS_AUTOSIZE: u32 = 0x10;
const BTNS_NOPREFIX: u32 = 0x20;
const BTNS_SHOWTEXT: u32 = 0x40;
const BTNS_WHOLEDROPDOWN: u32 = 0x80;

const TBSTATE_CHECKED: u32 = 0x01;
const TBSTATE_PRESSED: u32 = 0x02;
const TBSTATE_ENABLED: u32 = 0x04;
const TBSTATE_HIDDEN: u32 = 0x08;
const TBSTATE_INDETERMINATE: u32 = 0x10;
const TBSTATE_WRAP: u32 = 0x20;
const TBSTATE_MARKED: u32 = 0x80;

const TBIF_IMAGE: u32 = 0x01;
const TBIF_TEXT: u32 = 0x02;
const TBIF_STATE: u32 = 0x04;
const TBIF_STYLE: u32 = 0x08;
const TBIF_LPARAM: u32 = 0x10;
const TBIF_COMMAND: u32 = 0x20;
const TBIF_SIZE: u32 = 0x40;
const TBIF_BYINDEX: u32 = 0x8000_0000;
/// TBBUTTONINFO's size, the only one TB_GETBUTTONINFO takes.
const TBBUTTONINFO_SIZE: u32 = 32;
/// TBBUTTON's size, whatever TB_BUTTONSTRUCTSIZE says.
const TBBUTTON_SIZE: u32 = 20;

const TBNF_IMAGE: u32 = 0x01;
const TBNF_DI_SETITEM: u32 = 0x1000_0000;
const TBIMHT_AFTER: u32 = 0x01;
const TBDDRET_TREATPRESSED: u32 = 2;

const I_IMAGECALLBACK: i32 = -1;
const I_IMAGENONE: i32 = -2;

// The notifications.
const TBN_BEGINDRAG: u32 = (-701i32) as u32;
const TBN_ENDDRAG: u32 = (-702i32) as u32;
const TBN_TOOLBARCHANGE: u32 = (-708i32) as u32;
const TBN_DROPDOWN: u32 = (-710i32) as u32;
const TBN_HOTITEMCHANGE: u32 = (-713i32) as u32;
const TBN_DRAGOUT: u32 = (-714i32) as u32;
const TBN_DELETINGBUTTON: u32 = (-715i32) as u32;
const TBN_GETDISPINFOW: u32 = (-717i32) as u32;
const TBN_GETINFOTIPA: u32 = (-718i32) as u32;
const TBN_GETINFOTIPW: u32 = (-719i32) as u32;
const NM_CLICK: u32 = (-2i32) as u32;
const NM_RCLICK: u32 = (-5i32) as u32;
const NM_RDBLCLK: u32 = (-6i32) as u32;
const NM_KEYDOWN: u32 = (-15i32) as u32;
const NM_RELEASEDCAPTURE: u32 = (-16i32) as u32;
const NM_TOOLTIPSCREATED: u32 = (-19i32) as u32;
const NM_LDOWN: u32 = (-20i32) as u32;

const HICF_OTHER: u32 = 0x00;
const HICF_MOUSE: u32 = 0x01;
const HICF_ARROWKEYS: u32 = 0x02;
const HICF_ENTERING: u32 = 0x10;
const HICF_LEAVING: u32 = 0x20;
const HICF_LMOUSE: u32 = 0x80;

// The custom draw item states the drawing works with.
const CDIS_SELECTED: u32 = 0x0001;
const CDIS_DISABLED: u32 = 0x0004;
const CDIS_CHECKED: u32 = 0x0008;
const CDIS_HOT: u32 = 0x0040;
const CDIS_MARKED: u32 = 0x0080;
const CDIS_INDETERMINATE: u32 = 0x0100;

const SEPARATOR_WIDTH: i32 = 8;
const TOP_BORDER: i32 = 2;
const BOTTOM_BORDER: i32 = 2;
const DDARROW_WIDTH: i32 = 11;
const ARROW_HEIGHT: i32 = 3;
const INSERTMARK_WIDTH: i32 = 2;
const DEFPAD_CX: i32 = 7;
const DEFPAD_CY: i32 = 6;
const DEFLISTGAP: i32 = 4;
const LISTPAD_CY: i32 = 9;
const NONLIST_NOTEXT_OFFSET: i32 = 2;

const HINST_COMMCTRL: u32 = 0xFFFF_FFFF;
/// The standard bitmaps' resource ids in COMCTL32 (IDB_STD_SMALL and
/// on), which TB_ADDBITMAP's HINST_COMMCTRL ids map to.
const IDB_STD_SMALL: u32 = 120;
const IDB_STD_LARGE: u32 = 121;
const IDB_VIEW_SMALL: u32 = 124;
const IDB_VIEW_LARGE: u32 = 125;
const IDB_HIST_SMALL: u32 = 130;
const IDB_HIST_LARGE: u32 = 131;

static STD_SMALL: &[u8] = include_bytes!("res/idb_std_small.dib");
static STD_LARGE: &[u8] = include_bytes!("res/idb_std_large.dib");
static VIEW_SMALL: &[u8] = include_bytes!("res/idb_view_small.dib");
static VIEW_LARGE: &[u8] = include_bytes!("res/idb_view_large.dib");
static HIST_SMALL: &[u8] = include_bytes!("res/idb_hist_small.dib");
static HIST_LARGE: &[u8] = include_bytes!("res/idb_hist_large.dib");

const ILC_MASK: u32 = 0x0001;
const ILC_COLOR32: u32 = 0x0020;
const ILD_TRANSPARENT: u32 = 0x0001;
const ILD_BLEND50: u32 = 0x0004;
const ILD_MASK: u32 = 0x0010;

const NF_QUERY: u32 = 3;
const NF_REQUERY: u32 = 4;
const NFR_UNICODE: u32 = 2;
const GWL_STYLE: u32 = (-16i32) as u32;
const CW_USEDEFAULT: u32 = 0x8000_0000;
const MK_SHIFT: u32 = 0x0004;
const VK_RETURN: u32 = 0x0D;
const VK_SPACE: u32 = 0x20;
const VK_LEFT: u32 = 0x25;
const VK_UP: u32 = 0x26;
const VK_RIGHT: u32 = 0x27;
const VK_DOWN: u32 = 0x28;
const BN_CLICKED: u32 = 0;

const SM_CXBORDER: u32 = 5;
const SM_CYBORDER: u32 = 6;
const SM_CXEDGE: u32 = 45;
const SM_CYEDGE: u32 = 46;

const DT_CENTER: u32 = 0x0001;
const DT_VCENTER: u32 = 0x0004;
const DT_SINGLELINE: u32 = 0x0020;
const DT_CALCRECT: u32 = 0x0400;
const DT_NOPREFIX: u32 = 0x0800;
const DT_END_ELLIPSIS: u32 = 0x8000;
const TRANSPARENT: u32 = 1;
const OPAQUE: u32 = 2;

const BDR_SUNKENOUTER: u32 = 0x0002;
const BDR_RAISEDINNER: u32 = 0x0004;
const EDGE_RAISED: u32 = 0x0005;
const EDGE_SUNKEN: u32 = 0x000A;
const EDGE_ETCHED: u32 = 0x0006;
const BF_TOP: u32 = 0x0002;
const BF_RECT: u32 = 0x000F;
const BF_MIDDLE: u32 = 0x0800;
const BF_SOFT: u32 = 0x1000;
const DCX_WINDOW: u32 = 0x0001;
const DCX_USESTYLE: u32 = 0x0001_0000;

const SRCCOPY: u32 = 0x00CC_0020;
const PATCOPY: u32 = 0x00F0_0021;
const NOTSRCERASE: u32 = 0x0011_00A6;
/// The ROP that puts the pattern where the source is black: dst ^ (src
/// & (pat ^ dst)).
const ROP_ETCH: u32 = 0x00E2_0746;

/// The timer that watches for the mouse leaving a flat toolbar.
const ID_LEAVE: u32 = 0x7B01;
const LEAVE_MS: u32 = 100;

/// The scratch space every script starts with, which continuations use
/// for their own structures: at its start the NMTBGETINFOTIP, then its
/// text buffer (INFOTIPSIZE + 1 wide characters), then an
/// NMTTDISPINFOA.
const S_CONT: u32 = 2304;
const O_TIP_NM: u32 = 0;
const O_TIP_BUF: u32 = 32;
const O_DISP_A: u32 = 2088;
/// NMTTDISPINFO's fields; the W one's szText is longer, which moves
/// the fields after it.
const DISP_TEXT: u32 = 12;
const DISP_SZ: u32 = 16;
const DISPA_HINST: u32 = 96;
const DISPA_FLAGS: u32 = 100;
const DISPA_LPARAM: u32 = 104;
const DISPW_HINST: u32 = 176;
/// szText's length in characters.
const DISP_CHARS: usize = 80;

// The continuations.
const T_FORMAT: u16 = 1;
const T_HOT: u16 = 2;
const T_TIP_MADE: u16 = 3;
const T_MOVE: u16 = 4;
const T_DROPDOWN: u16 = 5;
const T_DEFAULT: u16 = 6;
const T_TIP_ASK: u16 = 7;
const T_TIP_GOT: u16 = 8;
const T_DISP_BACK: u16 = 9;
const T_DISP_IMAGE: u16 = 10;
const T_DRAW_DC: u16 = 11;
const T_ERASED: u16 = 12;
const T_KEY: u16 = 13;
const T_EX_MADE: u16 = 14;
const T_CLICKED: u16 = 15;

/// A button.
#[derive(Clone, Debug, Default)]
pub struct Button {
    pub bitmap: i32,
    pub command: i32,
    pub state: u32,
    pub style: u32,
    pub hot: bool,
    pub dropdown_pressed: bool,
    pub data: u32,
    /// iString as the program gave it: an index in the string pool, or
    /// the address of its text, which `text` has a copy of.
    pub string: u32,
    pub text: Option<Vec<u8>>,
    pub row: i32,
    pub rect: Rect,
    /// The width the program set (TBIF_SIZE), or 0.
    pub cx: i32,
    /// The image TBN_GETDISPINFOW gave an I_IMAGECALLBACK button.
    pub disp_image: i32,
}

crate::state_fields!(Button {
    bitmap,
    command,
    state,
    style,
    hot,
    dropdown_pressed,
    data,
    string,
    text,
    row,
    rect,
    cx,
    disp_image
});

/// A bitmap TB_ADDBITMAP added: its count of images, and where it came
/// from (a module and resource, or 0 and a bitmap's handle).
#[derive(Clone, Debug, Default)]
pub struct BitmapInfo {
    pub buttons: i32,
    pub inst: u32,
    pub id: u32,
}

crate::state_fields!(BitmapInfo { buttons, inst, id });

/// An image list of the toolbar, by its id (TB_SETIMAGELIST's wParam
/// from version 5).
#[derive(Clone, Debug, Default)]
pub struct ImlEntry {
    pub id: i32,
    pub himl: u32,
}

crate::state_fields!(ImlEntry { id, himl });

/// What a toolbar keeps: Wine's TOOLBAR_INFO.
#[derive(Clone, Debug, Default)]
pub struct Toolbar {
    pub struct_size: u32,
    pub width: i32,
    pub client: Rect,
    pub bound: Rect,
    pub button_h: i32,
    pub button_w: i32,
    pub bitmap_h: i32,
    pub bitmap_w: i32,
    pub indent: i32,
    pub rows: i32,
    pub max_text_rows: i32,
    pub cx_min: i32,
    pub cx_max: i32,
    pub buttons: Vec<Button>,
    pub strings: Vec<Vec<u8>>,
    pub bitmaps: Vec<BitmapInfo>,
    pub num_bitmaps: i32,
    pub button_down: i32,
    pub button_drag: i32,
    pub old_hit: i32,
    pub hot_item: i32,
    pub pad_cx: i32,
    pub pad_cy: i32,
    pub top_margin: i32,
    pub list_gap: i32,
    pub default_font: u32,
    pub font: u32,
    /// The image list the toolbar made itself (TB_ADDBITMAP), or 0.
    pub him_int: u32,
    pub def: Vec<ImlEntry>,
    pub hot_lists: Vec<ImlEntry>,
    pub dis_lists: Vec<ImlEntry>,
    pub tooltip: u32,
    pub tip_creating: bool,
    pub notify: u32,
    pub anchor: bool,
    pub redraw: bool,
    pub drag_out_sent: bool,
    pub unicode: bool,
    pub captured: bool,
    pub style: u32,
    pub ex_style: u32,
    pub dt_flags: u32,
    pub mark_color: u32,
    pub clr_highlight: u32,
    pub clr_shadow: u32,
    pub version: i32,
    /// A buffer of the program's heap holding a tooltip text too long
    /// for szText, or 0.
    pub tip_text: u32,
    pub mark_button: i32,
    pub mark_flags: u32,
    pub leave_timer: bool,
}

crate::state_fields!(Toolbar {
    struct_size,
    width,
    client,
    bound,
    button_h,
    button_w,
    bitmap_h,
    bitmap_w,
    indent,
    rows,
    max_text_rows,
    cx_min,
    cx_max,
    buttons,
    strings,
    bitmaps,
    num_bitmaps,
    button_down,
    button_drag,
    old_hit,
    hot_item,
    pad_cx,
    pad_cy,
    top_margin,
    list_gap,
    default_font,
    font,
    him_int,
    def,
    hot_lists,
    dis_lists,
    tooltip,
    tip_creating,
    notify,
    anchor,
    redraw,
    drag_out_sent,
    unicode,
    captured,
    style,
    ex_style,
    dt_flags,
    mark_color,
    clr_highlight,
    clr_shadow,
    version,
    tip_text,
    mark_button,
    mark_flags,
    leave_timer
});

/// What the module keeps, in COMCTL32's `State`.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub bars: BTreeMap<u32, Toolbar>,
}

crate::state_fields!(State { bars });

fn get(cpu: &mut Cpu, hwnd: u32) -> Option<Toolbar> {
    state(cpu).toolbar.bars.get(&hwnd).cloned()
}

fn put(cpu: &mut Cpu, hwnd: u32, t: Toolbar) {
    state(cpu).toolbar.bars.insert(hwnd, t);
}

/// A script with the continuations' scratch space at its start.
fn new_script(hwnd: u32) -> Script {
    let mut s = Script::new(hwnd);
    s.alloc(S_CONT);
    s
}

/// A script a continuation adds steps with: its scratch space is the
/// continuations' part of the script's.
fn cont_script(hwnd: u32) -> Script {
    Script::new(hwnd)
}

fn gdi(cpu: &mut Cpu, name: &str, args: &[u32]) -> u32 {
    call_builtin(cpu, "GDI32.DLL", name, args).unwrap_or(0)
}

fn metric(index: u32) -> i32 {
    sysparams::metric(index)
}

/// The program's ANSI string at `at`, at most `max` bytes; None if the
/// memory faulted.
fn read_ansi(cpu: &mut Cpu, at: u32, max: usize) -> Option<Vec<u8>> {
    if at == 0 {
        return Some(Vec::new());
    }
    let s = cpu.bus.guest_read_asciiz(at, max);
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(s)
}

/// The program's string at `at`, ANSI or Unicode, in ANSI.
fn read_text(cpu: &mut Cpu, at: u32, wide: bool, max: usize) -> Option<Vec<u8>> {
    if wide {
        let s = tooltips::read_wide(cpu, at, max as u32);
        if cpu.bus.guest_faulted() {
            return None;
        }
        Some(s)
    } else {
        read_ansi(cpu, at, max)
    }
}

/// Write `text` and a 0 after it at `at`, ANSI or Unicode, at most
/// `chars` characters with the 0.
fn write_text(cpu: &mut Cpu, at: u32, text: &[u8], wide: bool, chars: usize) {
    if at == 0 || chars == 0 {
        return;
    }
    let n = text.len().min(chars - 1);
    if wide {
        let mut units = kernel32::ansi_to_wide(&text[..n]);
        units.push(0);
        let bytes: Vec<u8> = units.iter().flat_map(|u| u.to_le_bytes()).collect();
        cpu.bus.guest_write_bytes(at, &bytes);
    } else {
        user32::copy_out(cpu, at, n as u32 + 1, &text[..n]);
    }
}

/// InvalidateRect(hwnd, r, erase).
fn invalidate_rect(cpu: &mut Cpu, hwnd: u32, r: Rect, erase: bool) {
    let heap = process_heap(cpu);
    let at = heap_alloc(cpu, heap, 16);
    if at == 0 {
        super::invalidate(cpu, hwnd, erase);
        return;
    }
    r.write(cpu, at);
    user32::paint::invalidate_rect(cpu, &[hwnd, at, erase as u32]);
    heap_free(cpu, heap, at);
}

fn default_top_margin(t: &Toolbar) -> i32 {
    if t.style & TBSTYLE_FLAT != 0 {
        0
    } else {
        TOP_BORDER
    }
}

/// GETIBITMAP and GETHIMLID: an image's index and its list's id.
fn image_index(t: &Toolbar, i: i32) -> i32 {
    if t.version >= 5 { i & 0xFFFF } else { i }
}

fn list_id(t: &Toolbar, i: i32) -> i32 {
    if t.version >= 5 {
        (i as u32 >> 16) as i32
    } else {
        0
    }
}

fn list_of(lists: &[ImlEntry], id: i32) -> u32 {
    lists.iter().find(|e| e.id == id).map_or(0, |e| e.himl)
}

/// TOOLBAR_InsertImageList: the list that was there.
fn insert_list(lists: &mut Vec<ImlEntry>, himl: u32, id: i32) -> u32 {
    if let Some(e) = lists.iter_mut().find(|e| e.id == id) {
        return std::mem::replace(&mut e.himl, himl);
    }
    lists.push(ImlEntry { id, himl });
    0
}

fn def_list(t: &Toolbar, id: i32) -> u32 {
    list_of(&t.def, id)
}

/// TOOLBAR_GetText: a button's text, its own or the pool's.
fn text_of(t: &Toolbar, b: &Button) -> Option<Vec<u8>> {
    if let Some(text) = &b.text {
        return Some(text.clone());
    }
    let i = b.string as i32;
    if b.string < 0x10000 && i >= 0 && (i as usize) < t.strings.len() {
        Some(t.strings[i as usize].clone())
    } else {
        None
    }
}

fn is_valid_bitmap_index(cpu: &mut Cpu, t: &Toolbar, index: i32) -> bool {
    let himl = def_list(t, list_id(t, index));
    let i = image_index(t, index);
    (himl != 0 && i >= 0 && i < imagelist::count(cpu, himl)) || index == I_IMAGECALLBACK
}

fn is_valid_image_list(cpu: &mut Cpu, t: &Toolbar, index: i32) -> bool {
    let himl = def_list(t, list_id(t, index));
    himl != 0 && imagelist::count(cpu, himl) > 0
}

/// TOOLBAR_MeasureString: the size of a button's text in the DC.
fn measure_string(cpu: &mut Cpu, t: &Toolbar, b: &Button, hdc: u32) -> (i32, i32) {
    let shown = t.ex_style & TBSTYLE_EX_MIXEDBUTTONS == 0 || b.style & BTNS_SHOWTEXT != 0;
    if t.max_text_rows <= 0 || b.state & TBSTATE_HIDDEN != 0 || !shown {
        return (0, 0);
    }
    let Some(text) = text_of(t, b) else {
        return (0, 0);
    };
    let (cx, cy) = gfx::text_size(cpu, hdc, &text);
    let mut r = Rect::new(0, 0, cx, cy);
    let prefix = if b.style & BTNS_NOPREFIX != 0 {
        DT_NOPREFIX
    } else {
        0
    };
    let flags = DT_VCENTER | DT_SINGLELINE | DT_CALCRECT | prefix;
    gfx::draw_text(cpu, hdc, &text, &mut r, flags);
    (r.right, r.bottom)
}

/// A DC of the toolbar with its font selected: the DC and the font
/// that was.
fn font_dc(cpu: &mut Cpu, hwnd: u32, t: &Toolbar) -> (u32, u32) {
    let hdc = user32::paint::get_dc(cpu, &[hwnd]).unwrap_or(0);
    let old = gfx::select(cpu, hdc, t.font);
    (hdc, old)
}

fn release_font_dc(cpu: &mut Cpu, hwnd: u32, (hdc, old): (u32, u32)) {
    gfx::select(cpu, hdc, old);
    user32::paint::release_dc(cpu, &[hwnd, hdc]);
}

/// TOOLBAR_CalcStrings: the largest text's width and height.
fn calc_strings(cpu: &mut Cpu, hwnd: u32, t: &Toolbar) -> (i32, i32) {
    let mut size = (0, 0);
    if t.max_text_rows == 0 {
        return size;
    }
    let dc = font_dc(cpu, hwnd, t);
    if t.buttons.is_empty() && !t.strings.is_empty() {
        size.1 = gfx::font_height(cpu, dc.0);
    }
    for b in &t.buttons {
        if text_of(t, b).is_some() {
            let (cx, cy) = measure_string(cpu, t, b, dc.0);
            size.0 = size.0.max(cx);
            size.1 = size.1.max(cy);
        }
    }
    release_font_dc(cpu, hwnd, dc);
    size
}

/// TOOLBAR_WrapToolbar: the TBSTATE_WRAP flags of a TBSTYLE_WRAPABLE
/// toolbar, from its parent's width.
fn wrap(cpu: &mut Cpu, hwnd: u32, t: &mut Toolbar) {
    if t.style & TBSTYLE_WRAPABLE == 0 && t.ex_style & TBSTYLE_EX_UNDOC1 == 0 {
        return;
    }
    let parent = super::parent(cpu, hwnd);
    t.width = if parent != 0 {
        super::client(cpu, parent).width()
    } else {
        user32::user(cpu).screen_rect(hwnd).width()
    };
    let n = t.buttons.len();
    let sep = |t: &Toolbar, i: usize| t.buttons[i].style & BTNS_SEP != 0;
    let hidden = |t: &Toolbar, i: usize| t.buttons[i].state & TBSTATE_HIDDEN != 0;
    let mut x = t.indent;
    let mut button_wrap = false;
    let mut i = 0;
    while i < n {
        t.buttons[i].state &= !TBSTATE_WRAP;
        if hidden(t, i) {
            i += 1;
            continue;
        }
        let b = &t.buttons[i];
        let cx = if b.cx > 0 {
            b.cx
        } else if sep(t, i) && t.style & CCS_VERT == 0 {
            if b.bitmap > 0 {
                b.bitmap
            } else {
                SEPARATOR_WIDTH
            }
        } else {
            t.button_w
        };
        // Two or more separators in a row are a group, the first of
        // which wraps if the wrap before was on a button.
        if button_wrap && sep(t, i) && i + 1 < n && sep(t, i + 1) {
            t.buttons[i].state |= TBSTATE_WRAP;
            x = t.indent;
            button_wrap = false;
            i += 2;
            continue;
        }
        if x + cx - (t.button_w - t.bitmap_w) / 2 > t.width || (x == t.indent && cx > t.width) {
            let mut found = false;
            // Past separators and hidden buttons: the last separator
            // before a button wraps.
            while i < n && ((sep(t, i) && t.buttons[i].style & BTNS_DROPDOWN == 0) || hidden(t, i))
            {
                i += 1;
                found = true;
            }
            if found && i < n {
                i -= 1;
                t.buttons[i].state |= TBSTATE_WRAP;
                x = t.indent;
                button_wrap = false;
                i += 1;
                continue;
            } else if i >= n {
                break;
            }
            // A button: the last separator before it wraps.
            let mut j = i as isize - 1;
            while j >= 0 && t.buttons[j as usize].state & TBSTATE_WRAP == 0 {
                let ju = j as usize;
                if sep(t, ju) && !hidden(t, ju) {
                    found = true;
                    i = ju;
                    x = t.indent;
                    t.buttons[ju].state |= TBSTATE_WRAP;
                    button_wrap = false;
                    break;
                }
                j -= 1;
            }
            // No separator: the button before it that isn't hidden.
            if !found {
                let mut j = i as isize - 1;
                while j >= 0 && t.buttons[j as usize].state & TBSTATE_WRAP == 0 {
                    let ju = j as usize;
                    if hidden(t, ju) {
                        j -= 1;
                        continue;
                    }
                    found = true;
                    i = ju;
                    x = t.indent;
                    t.buttons[ju].state |= TBSTATE_WRAP;
                    button_wrap = true;
                    break;
                }
            }
            // Else the button itself.
            if !found {
                t.buttons[i].state |= TBSTATE_WRAP;
                x = t.indent;
                button_wrap = !sep(t, i);
            }
        } else {
            x += cx;
        }
        i += 1;
    }
}

/// TOOLBAR_MeasureButton: the size a button needs for a text of
/// `text` size, with or without an image.
fn measure_button(t: &Toolbar, text: (i32, i32), has_bitmap: bool, valid_list: bool) -> (i32, i32) {
    let cxedge = metric(SM_CXEDGE);
    if t.style & TBSTYLE_LIST != 0 {
        let mut cy = text.1.max(if has_bitmap { t.bitmap_h } else { 0 });
        cy += if valid_list {
            if has_bitmap { DEFPAD_CY } else { LISTPAD_CY }
        } else {
            t.pad_cy
        };
        let mut cx = 2 * cxedge + t.bitmap_w + t.list_gap;
        if text.0 > 0 {
            cx += text.0 + t.pad_cx;
        }
        (cx, cy)
    } else if has_bitmap {
        let mut cy = t.bitmap_h + DEFPAD_CY;
        if text.1 > 0 {
            cy += 1 + text.1;
        }
        (t.pad_cx + text.0.max(t.bitmap_w), cy)
    } else {
        let cy = text.1 + t.pad_cy + NONLIST_NOTEXT_OFFSET;
        (t.pad_cx + (2 * cxedge + text.0).max(t.bitmap_w), cy)
    }
}

/// TOOLBAR_CalcToolbar: the buttons' size from the texts and images,
/// then the layout.
fn calc(cpu: &mut Cpu, hwnd: u32, t: &mut Toolbar) {
    let text = calc_strings(cpu, hwnd, t);
    let valid = is_valid_image_list(cpu, t, 0);
    let (w, h) = measure_button(t, text, true, valid);
    t.button_w = w;
    t.button_h = h;
    t.top_margin = default_top_margin(t);
    if t.cx_min >= 0 && t.button_w < t.cx_min {
        t.button_w = t.cx_min;
    }
    if t.cx_max > 0 && t.button_w > t.cx_max {
        t.button_w = t.cx_max;
    }
    layout(cpu, hwnd, t);
}

/// TOOLBAR_LayoutToolbar: the buttons' rectangles and rows.
fn layout(cpu: &mut Cpu, hwnd: u32, t: &mut Toolbar) {
    let valid = is_valid_image_list(cpu, t, 0);
    let dd_arrows = t.ex_style & TBSTYLE_EX_DRAWDDARROWS != 0;
    wrap(cpu, hwnd, t);
    let mut x = t.indent;
    let mut y = t.top_margin;
    let (mut rows, mut sep_rows) = (0, 0);
    t.bound = Rect::new(x, y, x, y + t.button_h);
    let n = t.buttons.len();
    for i in 0..n {
        if t.buttons[i].state & TBSTATE_HIDDEN != 0 {
            t.buttons[i].rect = Rect::default();
            continue;
        }
        let mut cy = t.button_h;
        let b = t.buttons[i].clone();
        let cx;
        if b.style & BTNS_SEP != 0 {
            let w = if b.bitmap > 0 {
                b.bitmap
            } else {
                SEPARATOR_WIDTH
            };
            if t.style & CCS_VERT != 0 {
                cy = w;
                cx = if b.cx > 0 { b.cx } else { t.width };
            } else {
                cx = if b.cx > 0 { b.cx } else { w };
            }
        } else {
            let mut w = if b.cx != 0 {
                b.cx
            } else if t.ex_style & TBSTYLE_EX_MIXEDBUTTONS != 0 || b.style & BTNS_AUTOSIZE != 0 {
                let dc = font_dc(cpu, hwnd, t);
                let size = measure_string(cpu, t, &b, dc.0);
                release_font_dc(cpu, hwnd, dc);
                let has_bitmap = is_valid_bitmap_index(cpu, t, b.bitmap);
                measure_button(t, size, has_bitmap, valid).0
            } else {
                t.button_w
            };
            // A size the program set gets no room for the arrow.
            if b.cx == 0 && dd_arrows && b.style & (BTNS_DROPDOWN | BTNS_WHOLEDROPDOWN) != 0 {
                w += DDARROW_WIDTH;
            }
            cx = w;
        }
        let wraps = b.state & TBSTATE_WRAP != 0;
        let r = Rect::new(x, y, x + cx, y + cy);
        t.buttons[i].rect = r;
        t.bound.left = t.bound.left.min(x);
        t.bound.right = t.bound.right.max(x + cx);
        t.bound.bottom = t.bound.bottom.max(y + cy);
        if t.tooltip != 0 && b.style & BTNS_SEP == 0 {
            tooltips::set_tool_rect(cpu, t.tooltip, hwnd, b.command as u32, r);
        }
        // The space between rows counts as a row.
        t.buttons[i].row = rows + sep_rows;
        if wraps {
            if b.style & BTNS_SEP == 0 {
                y += cy;
            } else {
                if t.style & CCS_VERT == 0 {
                    let w = if b.cx > 0 { b.cx } else { SEPARATOR_WIDTH };
                    y += cy + w * 2 / 3;
                } else {
                    y += cy;
                }
                sep_rows += 1;
            }
            x = t.indent;
            if i != n - 1 {
                rows += 1;
            }
        } else {
            x += cx;
        }
    }
    t.rows = rows + sep_rows + 1;
}

/// TOOLBAR_InternalHitTest: the button at (x, y), minus the index of a
/// separator, or -1.
fn hit_test(t: &Toolbar, x: i32, y: i32) -> i32 {
    for (i, b) in t.buttons.iter().enumerate() {
        if b.state & TBSTATE_HIDDEN != 0 || !b.rect.contains(x, y) {
            continue;
        }
        return if b.style & BTNS_SEP != 0 {
            -(i as i32)
        } else {
            i as i32
        };
    }
    -1
}

/// TOOLBAR_GetButtonIndex: the index of the button with the command
/// `id`, or of the index `id`; -1 for none.
fn button_index(t: &Toolbar, id: i32, by_index: bool) -> i32 {
    if by_index {
        return if id >= t.buttons.len() as i32 { -1 } else { id };
    }
    t.buttons
        .iter()
        .position(|b| b.command == id)
        .map_or(-1, |i| i as i32)
}

/// TOOLBAR_GetCheckedGroupButtonIndex: the checked button of the group
/// of the button `index`, or -1.
fn checked_group_index(t: &Toolbar, index: i32) -> i32 {
    let n = t.buttons.len() as i32;
    if index < 0 || index >= n {
        return -1;
    }
    let b = &t.buttons[index as usize];
    if b.style & BTNS_CHECKGROUP == BTNS_CHECKGROUP && b.state & TBSTATE_CHECKED != 0 {
        return index;
    }
    let mut i = index - 1;
    while i >= 0 {
        let b = &t.buttons[i as usize];
        if b.style & BTNS_GROUP != BTNS_GROUP {
            break;
        }
        if b.state & TBSTATE_CHECKED != 0 {
            return i;
        }
        i -= 1;
    }
    let mut i = index + 1;
    while i < n {
        let b = &t.buttons[i as usize];
        if b.style & BTNS_GROUP != BTNS_GROUP {
            break;
        }
        if b.state & TBSTATE_CHECKED != 0 {
            return i;
        }
        i += 1;
    }
    -1
}

/// TOOLBAR_AutoSize: the toolbar's place at the top (or bottom) of its
/// parent, as wide as the parent's client area, by SetWindowPos.
/// `if_changed` leaves out a SetWindowPos that would change nothing,
/// as WM_SIZE does to end the recursion.
fn auto_size(cpu: &mut Cpu, hwnd: u32, t: &mut Toolbar, s: &mut Script, if_changed: bool) {
    let parent = super::parent(cpu, hwnd);
    if parent == 0 || !t.redraw {
        return;
    }
    let pr = super::client(cpu, parent);
    let (x, mut y) = (pr.left, pr.top);
    let mut cy = TOP_BORDER + t.rows * t.button_h + BOTTOM_BORDER;
    let mut cx = pr.width();
    if t.style & TBSTYLE_WRAPABLE != 0 || t.ex_style & TBSTYLE_EX_UNDOC1 != 0 {
        layout(cpu, hwnd, t);
        super::invalidate(cpu, hwnd, true);
    }
    if t.style & CCS_NORESIZE != 0 {
        return;
    }
    let mut flags = SWP_NOZORDER | SWP_NOACTIVATE;
    let window = user32::user(cpu).screen_rect(hwnd);
    if t.style & CCS_BOTTOM == CCS_NOMOVEY {
        let (_, oy) = user32::user(cpu).client_origin(parent);
        y = window.top - oy;
    }
    if t.style & CCS_BOTTOM == CCS_BOTTOM {
        y = pr.bottom - window.height();
    }
    if t.style & CCS_NOPARENTALIGN != 0 {
        flags |= SWP_NOMOVE;
    }
    if t.style & CCS_NODIVIDER == 0 {
        cy += metric(SM_CYEDGE);
    }
    if t.style & WS_BORDER != 0 {
        cy += 2 * metric(SM_CXBORDER);
        cx += 2 * metric(SM_CYBORDER);
    }
    if if_changed {
        let now = user32::user(cpu)
            .window(hwnd)
            .map(|w| w.rect)
            .unwrap_or_default();
        let same_size = now.width() == cx && now.height() == cy;
        let same_place = flags & SWP_NOMOVE != 0 || (now.left == x && now.top == y);
        if same_size && same_place {
            return;
        }
    }
    let args = [hwnd, 0, x as u32, y as u32, cx as u32, cy as u32, flags];
    s.user("SetWindowPos", &args);
}

/// The TBBUTTONs at `at`, or None if the memory faulted.
fn read_tbbuttons(cpu: &mut Cpu, at: u32, n: u32, wide: bool) -> Option<Vec<Button>> {
    let mut v = Vec::new();
    for i in 0..n {
        let w = read_words(cpu, at + i * TBBUTTON_SIZE, 5);
        if cpu.bus.guest_faulted() {
            return None;
        }
        let style = (w[2] >> 8) & 0xFF;
        let mut b = Button {
            bitmap: w[0] as i32,
            command: w[1] as i32,
            state: w[2] & 0xFF,
            style,
            data: w[3],
            string: w[4],
            disp_image: I_IMAGENONE,
            ..Button::default()
        };
        if style & BTNS_SEP != 0 {
            b.string = u32::MAX;
        } else if w[4] >= 0x10000 && w[4] != u32::MAX {
            b.text = Some(read_text(cpu, w[4], wide, 4096)?);
        }
        v.push(b);
    }
    Some(v)
}

/// TOOLBAR_InternalInsertButtonsT.
#[allow(clippy::too_many_arguments)]
fn insert_buttons(
    cpu: &mut Cpu,
    hwnd: u32,
    t: &mut Toolbar,
    s: &mut Script,
    index: i32,
    n: u32,
    at: u32,
    wide: bool,
) -> u32 {
    if n > 0 && at == 0 {
        return 0;
    }
    let Some(new) = read_tbbuttons(cpu, at, n.min(0x10000), wide) else {
        return 0;
    };
    let index = if index < 0 || index as usize > t.buttons.len() {
        t.buttons.len()
    } else {
        index as usize
    };
    let has_string = new.iter().any(|b| b.text.is_some());
    for (k, b) in new.into_iter().enumerate() {
        if t.tooltip != 0 && b.style & BTNS_SEP == 0 {
            tooltips::add_tool(cpu, t.tooltip, hwnd, b.command as u32, Rect::default());
        }
        t.buttons.insert(index + k, b);
    }
    if !t.strings.is_empty() || has_string {
        calc(cpu, hwnd, t);
    } else {
        layout(cpu, hwnd, t);
    }
    auto_size(cpu, hwnd, t, s, false);
    super::invalidate(cpu, hwnd, true);
    1
}

/// A bitmap of the screen's format with the packed DIB at `dib` in it,
/// its colors `map` changed (CreateMappedBitmap's work): its handle,
/// or 0.
fn mapped_bitmap(cpu: &mut Cpu, dib: u32, map: &[(u32, u32)]) -> u32 {
    let w = read_words(cpu, dib, 10);
    if cpu.bus.guest_faulted() {
        return 0;
    }
    let size = w[0];
    let (width, height) = (w[1] as i32, w[2] as i32);
    let bpp = w[3] >> 16;
    let used = w[8];
    if !(12..=124).contains(&size) || width <= 0 || height == 0 {
        return 0;
    }
    let colors = if size >= 40 && used != 0 {
        used.min(256)
    } else if bpp <= 8 {
        1 << bpp
    } else {
        0
    };
    let n = size + 4 * colors;
    let heap = process_heap(cpu);
    let info = heap_alloc(cpu, heap, n + 4);
    if info == 0 {
        return 0;
    }
    let words = read_words(cpu, dib, (n as usize).div_ceil(4));
    if cpu.bus.guest_faulted() {
        heap_free(cpu, heap, info);
        return 0;
    }
    write_words(cpu, info, &words);
    for c in 0..colors {
        let at = info + size + 4 * c;
        let quad = cpu.bus.guest_read_32(at);
        let rgb = ((quad >> 16) & 0xFF) | (quad & 0xFF00) | ((quad & 0xFF) << 16);
        if let Some(&(_, to)) = map.iter().find(|&&(from, _)| from == rgb) {
            let q = ((to >> 16) & 0xFF) | (to & 0xFF00) | ((to & 0xFF) << 16);
            cpu.bus.guest_write_32(at, q);
        }
    }
    let h = height.abs();
    let screen = user32::paint::get_dc(cpu, &[0]).unwrap_or(0);
    let hbm = gdi(
        cpu,
        "CreateCompatibleBitmap",
        &[screen, width as u32, h as u32],
    );
    if hbm != 0 {
        let dst = gdi(cpu, "CreateCompatibleDC", &[screen]);
        let old = gdi(cpu, "SelectObject", &[dst, hbm]);
        let (wu, hu) = (width as u32, h as u32);
        let args = [dst, 0, 0, wu, hu, 0, 0, wu, hu, dib + n, info, 0, SRCCOPY];
        gdi(cpu, "StretchDIBits", &args);
        gdi(cpu, "SelectObject", &[dst, old]);
        gdi(cpu, "DeleteDC", &[dst]);
    }
    user32::paint::release_dc(cpu, &[0, screen]);
    heap_free(cpu, heap, info);
    hbm
}

/// CreateMappedBitmap's colors when the program gives none.
fn default_map(cpu: &mut Cpu) -> Vec<(u32, u32)> {
    vec![
        (0x00_0000, gfx::sys_color(cpu, gfx::COLOR_BTNTEXT)),
        (0x80_8080, gfx::sys_color(cpu, gfx::COLOR_BTNSHADOW)),
        (0xC0_C0C0, gfx::sys_color(cpu, gfx::COLOR_BTNFACE)),
        (0xFF_FFFF, gfx::sys_color(cpu, gfx::COLOR_BTNHIGHLIGHT)),
    ]
}

/// CreateMappedBitmap(inst, id, 0, map, n) from Rust.
fn create_mapped(cpu: &mut Cpu, inst: u32, id: u32, map: &[(u32, u32)]) -> u32 {
    match kernel32::resource_data(cpu, inst, 2, id) {
        Some(Ok((at, _))) => mapped_bitmap(cpu, at, map),
        _ => 0,
    }
}

/// A standard bitmap of COMCTL32's, with its gray the button face's
/// color.
fn own_bitmap(cpu: &mut Cpu, id: u32) -> u32 {
    let dib = match id {
        IDB_STD_SMALL => STD_SMALL,
        IDB_STD_LARGE => STD_LARGE,
        IDB_VIEW_SMALL => VIEW_SMALL,
        IDB_VIEW_LARGE => VIEW_LARGE,
        IDB_HIST_SMALL => HIST_SMALL,
        IDB_HIST_LARGE => HIST_LARGE,
        _ => return 0,
    };
    let heap = process_heap(cpu);
    let at = heap_alloc(cpu, heap, dib.len() as u32 + 4);
    if at == 0 {
        return 0;
    }
    cpu.bus.guest_write_bytes(at, dib);
    let face = gfx::sys_color(cpu, gfx::COLOR_BTNFACE);
    let hbm = mapped_bitmap(cpu, at, &[(0xC0_C0C0, face)]);
    heap_free(cpu, heap, at);
    hbm
}

/// COMCTL32_EnsureBitmapSize: the bitmap made at least `min_w` by
/// `min_h`, the new part in `bk`: the bitmap's handle, maybe a new one.
fn ensure_bitmap_size(cpu: &mut Cpu, hbm: u32, min_w: i32, min_h: i32, bk: u32) -> u32 {
    let Some((w, h, planes, bpp)) = gdi32::gdi(cpu)
        .bitmap(hbm)
        .map(|b| (b.width, b.height, b.planes as u32, b.bpp as u32))
    else {
        return hbm;
    };
    let (nw, nh) = (min_w.max(w), min_h.max(h));
    if nw == w && nh == h {
        return hbm;
    }
    let new_dc = gdi(cpu, "CreateCompatibleDC", &[0]);
    let new = gdi(cpu, "CreateBitmap", &[nw as u32, nh as u32, planes, bpp, 0]);
    let old_new = gdi(cpu, "SelectObject", &[new_dc, new]);
    let brush = gdi(cpu, "CreateSolidBrush", &[bk]);
    let old_brush = gdi(cpu, "SelectObject", &[new_dc, brush]);
    let old_dc = gdi(cpu, "CreateCompatibleDC", &[0]);
    let old_old = gdi(cpu, "SelectObject", &[old_dc, hbm]);
    let (wu, hu, nwu, nhu) = (w as u32, h as u32, nw as u32, nh as u32);
    gdi(
        cpu,
        "BitBlt",
        &[new_dc, 0, 0, wu, hu, old_dc, 0, 0, SRCCOPY],
    );
    if w < min_w {
        gdi(cpu, "PatBlt", &[new_dc, wu, 0, nwu, hu, PATCOPY]);
    }
    if h < min_h {
        gdi(cpu, "PatBlt", &[new_dc, 0, hu, wu, nhu, PATCOPY]);
    }
    if w < min_w && h < min_h {
        gdi(cpu, "PatBlt", &[new_dc, wu, hu, nwu, nhu, PATCOPY]);
    }
    gdi(cpu, "SelectObject", &[new_dc, old_new]);
    gdi(cpu, "SelectObject", &[new_dc, old_brush]);
    gdi(cpu, "DeleteObject", &[brush]);
    gdi(cpu, "DeleteDC", &[new_dc]);
    gdi(cpu, "SelectObject", &[old_dc, old_old]);
    gdi(cpu, "DeleteDC", &[old_dc]);
    gdi(cpu, "DeleteObject", &[hbm]);
    new
}

/// TOOLBAR_AddBitmapToImageList.
fn add_bitmap_to_list(cpu: &mut Cpu, t: &mut Toolbar, himl: u32, info: &BitmapInfo) -> bool {
    let before = imagelist::count(cpu, himl);
    let own = super::DLL.base;
    let mut hbm = if info.inst == 0 {
        call_builtin(cpu, "USER32.DLL", "CopyImage", &[info.id, 0, 0, 0, 0]).unwrap_or(0)
    } else if info.inst == own {
        own_bitmap(cpu, info.id)
    } else {
        let map = default_map(cpu);
        create_mapped(cpu, info.inst, info.id, &map)
    };
    if hbm == 0 {
        return false;
    }
    let face = gfx::sys_color(cpu, gfx::COLOR_BTNFACE);
    let (cx, cy) = imagelist::icon_size(cpu, himl).unwrap_or((0, 0));
    if info.inst != own {
        hbm = ensure_bitmap_size(cpu, hbm, cx.saturating_mul(info.buttons), cy, face);
    }
    let index = imagelist::add_masked(cpu, himl, hbm, face);
    gdi(cpu, "DeleteObject", &[hbm]);
    if index == -1 {
        return false;
    }
    let added = imagelist::count(cpu, himl) - before;
    if info.buttons == 0 {
        // wParam 0 adds one image only.
        imagelist::il_set_image_count(cpu, &[himl, (before + 1) as u32]);
    }
    t.num_bitmaps += added;
    true
}

/// TOOLBAR_CheckImageListIconSize: the toolbar's own image list made
/// again in the bitmap size, if that changed.
fn check_icon_size(cpu: &mut Cpu, t: &mut Toolbar) {
    let old = def_list(t, 0);
    if old == 0 || old != t.him_int {
        return;
    }
    let Some((cx, cy)) = imagelist::icon_size(cpu, old) else {
        return;
    };
    if cx == t.bitmap_w && cy == t.bitmap_h {
        return;
    }
    let flags = ILC_COLOR32 | ILC_MASK;
    let new = imagelist::create(cpu, t.bitmap_w, t.bitmap_h, flags, 8, 2);
    for info in t.bitmaps.clone() {
        add_bitmap_to_list(cpu, t, new, &info);
    }
    insert_list(&mut t.def, new, 0);
    t.him_int = new;
    t.num_bitmaps -= imagelist::count(cpu, old);
    imagelist::destroy(cpu, old);
}

/// TOOLBAR_AddBitmap: the index of the bitmap's first image.
fn add_bitmap(cpu: &mut Cpu, hwnd: u32, t: &mut Toolbar, count: i32, inst: u32, id: u32) -> i32 {
    let info = if inst == HINST_COMMCTRL {
        let (buttons, own_id) = match id {
            0 => (15, IDB_STD_SMALL),
            1 => (15, IDB_STD_LARGE),
            4 => (12, IDB_VIEW_SMALL),
            5 => (12, IDB_VIEW_LARGE),
            8 => (5, IDB_HIST_SMALL),
            9 => (5, IDB_HIST_LARGE),
            _ => return -1,
        };
        // The buttons take the size of the standard images.
        let size = if id & 1 != 0 { 24 } else { 16 };
        set_bitmap_size(cpu, hwnd, t, size | (size << 16));
        calc(cpu, hwnd, t);
        BitmapInfo {
            buttons,
            inst: super::DLL.base,
            id: own_id,
        }
    } else {
        BitmapInfo {
            buttons: count,
            inst,
            id,
        }
    };
    // A bitmap added before gives its first index again.
    let mut sum = 0i32;
    for b in &t.bitmaps {
        if b.inst == info.inst && b.id == info.id {
            return sum;
        }
        sum = sum.wrapping_add(b.buttons);
    }
    let himl = if t.def.is_empty() {
        let flags = ILC_COLOR32 | ILC_MASK;
        let initial = info.buttons.clamp(0, 0x10000);
        let h = imagelist::create(cpu, t.bitmap_w, t.bitmap_h, flags, initial, 2);
        insert_list(&mut t.def, h, 0);
        t.him_int = h;
        h
    } else {
        def_list(t, 0)
    };
    if himl == 0 || !add_bitmap_to_list(cpu, t, himl, &info) {
        return -1;
    }
    t.bitmaps.push(info);
    super::invalidate(cpu, hwnd, true);
    sum
}

/// TOOLBAR_AddStringA and W: the index of the first string added.
fn add_string(cpu: &mut Cpu, hwnd: u32, t: &mut Toolbar, inst: u32, lp: u32, wide: bool) -> i32 {
    let first = t.strings.is_empty();
    let index = t.strings.len() as i32;
    if inst != 0 && lp < 0x10000 {
        // A string resource, its first character the delimiter of the
        // strings in it.
        let mut units = match user32::resource::string_units(cpu, inst, lp) {
            Some(Some(units)) => units,
            _ => Vec::new(),
        };
        units.truncate(511);
        if units.len() <= 1 {
            return index;
        }
        let delimiter = units[0];
        let mut p = 1;
        while let Some(off) = units[p..].iter().position(|&c| c == delimiter) {
            let next = p + off;
            // A delimiter at the end ends the list.
            if next + 1 >= units.len() {
                break;
            }
            t.strings.push(tooltips::wide_to_ansi(&units[p..next]));
            p = next + 1;
        }
    } else {
        if lp == 0 {
            return -1;
        }
        let mut new = Vec::new();
        let mut at = lp;
        loop {
            let Some(s) = read_text(cpu, at, wide, 4096) else {
                return -1;
            };
            if s.is_empty() || new.len() >= 0x10000 {
                break;
            }
            // The length in the program's characters.
            let chars = if wide {
                (0..)
                    .take_while(|&i| cpu.bus.guest_read_16(at + 2 * i) != 0)
                    .count() as u32
            } else {
                s.len() as u32
            };
            at += (chars + 1) * if wide { 2 } else { 1 };
            new.push(s);
        }
        t.strings.extend(new);
    }
    if first {
        calc(cpu, hwnd, t);
    }
    index
}

/// TOOLBAR_SetBitmapSize.
fn set_bitmap_size(cpu: &mut Cpu, hwnd: u32, t: &mut Toolbar, lp: u32) -> u32 {
    let mut w = lp as u16 as i16 as i32;
    let mut h = (lp >> 16) as u16 as i16 as i32;
    if w == 0 {
        w = 1;
    }
    if h == 0 {
        h = 1;
    }
    if w < -1 || h < -1 {
        return 0;
    }
    if w != -1 {
        t.bitmap_w = w;
    }
    if h != -1 {
        t.bitmap_h = h;
    }
    let himl = def_list(t, 0);
    if himl == t.him_int && himl != 0 && imagelist::count(cpu, himl) == 0 {
        imagelist::il_set_icon_size(cpu, &[himl, t.bitmap_w as u32, t.bitmap_h as u32]);
    }
    calc(cpu, hwnd, t);
    super::invalidate(cpu, hwnd, false);
    1
}

/// TOOLBAR_SetButtonSize.
fn set_button_size(cpu: &mut Cpu, hwnd: u32, t: &mut Toolbar, lp: u32) -> u32 {
    let mut cx = lp as u16 as i16 as i32;
    let mut cy = (lp >> 16) as u16 as i16 as i32;
    if cx < 0 || cy < 0 {
        return 0;
    }
    if cx == 0 {
        cx = 24;
    }
    if cy == 0 {
        cy = 22;
    }
    t.button_w = cx.max(t.pad_cx + t.bitmap_w);
    t.button_h = cy.max(t.pad_cy + t.bitmap_h);
    t.top_margin = default_top_margin(t);
    layout(cpu, hwnd, t);
    1
}

/// TOOLBAR_SetImageList: the list that was.
fn set_image_list(cpu: &mut Cpu, hwnd: u32, t: &mut Toolbar, wp: u32, himl: u32) -> u32 {
    let (old_button_w, old_w, old_h) = (t.button_w, t.bitmap_w, t.bitmap_h);
    let id = if t.version >= 5 { wp as i32 } else { 0 };
    let old = insert_list(&mut t.def, himl, id);
    let lists: Vec<u32> = t.def.iter().map(|e| e.himl).collect();
    t.num_bitmaps = lists.iter().map(|&h| imagelist::count(cpu, h).max(0)).sum();
    (t.bitmap_w, t.bitmap_h) = imagelist::icon_size(cpu, himl).unwrap_or((1, 1));
    if old_w != t.bitmap_w || old_h != t.bitmap_h {
        calc(cpu, hwnd, t);
        if t.button_w < old_button_w {
            let lp = (old_button_w as u32 & 0xFFFF) | ((t.button_h as u32 & 0xFFFF) << 16);
            set_button_size(cpu, hwnd, t, lp);
        }
    }
    super::invalidate(cpu, hwnd, true);
    old
}

/// Add to `s` WM_NOTIFY `code` from the toolbar to its notify window.
fn notify(cpu: &mut Cpu, s: &mut Script, hwnd: u32, t: &Toolbar, code: u32, more: &[u32]) -> u32 {
    super::notify(cpu, s, hwnd, t.notify, code, more)
}

/// An NMTOOLBARA's words after its header, for the button `b`.
fn nmtoolbar(id: i32, b: Option<&Button>, rect: Rect) -> Vec<u32> {
    let mut v = vec![id as u32];
    match b {
        Some(b) => v.extend([
            b.bitmap as u32,
            b.command as u32,
            b.state | (b.style << 8),
            b.data,
            b.string,
        ]),
        None => v.extend([0; 5]),
    }
    v.extend([0, 0]);
    v.extend([
        rect.left as u32,
        rect.top as u32,
        rect.right as u32,
        rect.bottom as u32,
    ]);
    v
}

/// TOOLBAR_SetHotItemEx: TBN_HOTITEMCHANGE, then the hot button
/// changes if the parent lets it.
fn set_hot_item_ex(cpu: &mut Cpu, hwnd: u32, t: &Toolbar, s: &mut Script, hit: i32, reason: u32) {
    if t.hot_item == hit {
        return;
    }
    let mut flags = reason;
    let n = t.buttons.len() as i32;
    let id_old = if t.hot_item >= 0 && t.hot_item < n {
        t.buttons[t.hot_item as usize].command as u32
    } else {
        flags |= HICF_ENTERING;
        0
    };
    let id_new = if hit >= 0 && hit < n {
        t.buttons[hit as usize].command as u32
    } else {
        flags |= HICF_LEAVING;
        0
    };
    notify(cpu, s, hwnd, t, TBN_HOTITEMCHANGE, &[id_old, id_new, flags]);
    s.then(T_HOT, &[hit as u32]);
}

/// The hot button changes to `hit` (T_HOT).
fn change_hot(cpu: &mut Cpu, hwnd: u32, hit: i32) {
    let Some(mut t) = get(cpu, hwnd) else {
        return;
    };
    let n = t.buttons.len() as i32;
    if t.hot_item >= 0 && t.hot_item < n {
        let b = &mut t.buttons[t.hot_item as usize];
        b.hot = false;
        let r = b.rect;
        invalidate_rect(cpu, hwnd, r, true);
    }
    // A disabled button doesn't become hot, though the parent heard
    // of it.
    if hit >= 0 && hit < n && t.buttons[hit as usize].state & TBSTATE_ENABLED != 0 {
        let b = &mut t.buttons[hit as usize];
        b.hot = true;
        let r = b.rect;
        invalidate_rect(cpu, hwnd, r, true);
        t.hot_item = hit;
    } else {
        t.hot_item = -1;
    }
    put(cpu, hwnd, t);
}

/// TOOLBAR_DeleteButton.
fn delete_button(cpu: &mut Cpu, hwnd: u32, t: &mut Toolbar, s: &mut Script, index: i32) -> u32 {
    if index < 0 || index as usize >= t.buttons.len() {
        return 0;
    }
    let b = t.buttons.remove(index as usize);
    let more = nmtoolbar(b.command, Some(&b), Rect::default());
    notify(cpu, s, hwnd, t, TBN_DELETINGBUTTON, &more);
    if t.tooltip != 0 && b.style & BTNS_SEP == 0 {
        tooltips::del_tool(cpu, t.tooltip, hwnd, b.command as u32);
    }
    let n = t.buttons.len() as i32;
    for i in [
        &mut t.hot_item,
        &mut t.button_down,
        &mut t.button_drag,
        &mut t.old_hit,
        &mut t.mark_button,
    ] {
        if *i >= n {
            *i = -1;
        }
    }
    layout(cpu, hwnd, t);
    super::invalidate(cpu, hwnd, true);
    1
}

/// TOOLBAR_MoveButton.
fn move_button(cpu: &mut Cpu, hwnd: u32, t: &mut Toolbar, s: &mut Script, id: i32, to: i32) -> u32 {
    let index = button_index(t, id, true);
    if index < 0 || to < 0 {
        return 0;
    }
    let to = to.min(t.buttons.len() as i32 - 1);
    let fix = |i: &mut i32| {
        if index < to {
            if *i > index && *i <= to {
                *i -= 1;
            } else if *i == index {
                *i = to;
            }
        } else if index > to {
            if *i >= to && *i < index {
                *i += 1;
            } else if *i == index {
                *i = to;
            }
        }
    };
    let b = t.buttons.remove(index as usize);
    t.buttons.insert(to as usize, b);
    fix(&mut t.button_down);
    fix(&mut t.button_drag);
    fix(&mut t.old_hit);
    fix(&mut t.hot_item);
    layout(cpu, hwnd, t);
    auto_size(cpu, hwnd, t, s, false);
    super::invalidate(cpu, hwnd, true);
    1
}

/// TOOLBAR_SetRows.
fn set_rows(cpu: &mut Cpu, hwnd: u32, t: &mut Toolbar, s: &mut Script, wp: u32, lp: u32) -> u32 {
    let rows = (wp & 0xFFFF) as i32;
    let larger = wp >> 16 != 0;
    if t.rows != rows {
        let n = t.buttons.len() as i32;
        let mut hidden = 0;
        for b in t.buttons.iter_mut() {
            b.state &= !TBSTATE_WRAP;
            if b.state & TBSTATE_HIDDEN != 0 {
                hidden += 1;
            }
        }
        // Round up: more buttons a row, fewer rows.
        let mut ideal = (n - hidden + (rows - 1)) / if rows != 0 { rows } else { 1 };
        if larger && ideal > 1 {
            let res_rows = (n + (ideal - 1)) / ideal;
            let more_rows = (n + (ideal - 2)) / (ideal - 1);
            if res_rows < rows && more_rows > rows {
                ideal -= 1;
            }
        }
        let mut column = 0;
        let mut wrapped = false;
        for b in t.buttons.iter_mut() {
            if b.state & TBSTATE_HIDDEN != 0 {
                continue;
            }
            if !wrapped {
                column += 1;
            } else {
                wrapped = false;
                column = 1;
            }
            if column > ideal - 1 {
                wrapped = true;
                b.state |= TBSTATE_WRAP;
            }
        }
        calc(cpu, hwnd, t);
        if t.style & CCS_NORESIZE != 0 {
            let (w, h) = (t.bound.width() as u32, t.bound.height() as u32);
            let flags = SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE;
            s.user("SetWindowPos", &[hwnd, 0, 0, 0, w, h, flags]);
        }
        super::invalidate(cpu, hwnd, true);
    }
    if lp != 0 {
        t.bound.write(cpu, lp);
    }
    0
}

/// TOOLBAR_GetButtonInfoT.
fn get_button_info(cpu: &mut Cpu, t: &Toolbar, id: u32, lp: u32, wide: bool) -> u32 {
    if lp == 0 {
        return u32::MAX;
    }
    let w = read_words(cpu, lp, 8);
    if cpu.bus.guest_faulted() || w[0] != TBBUTTONINFO_SIZE {
        return u32::MAX;
    }
    let mask = w[1];
    let index = button_index(t, id as i32, mask & TBIF_BYINDEX != 0);
    if index < 0 {
        return u32::MAX;
    }
    let b = &t.buttons[index as usize];
    if mask & TBIF_COMMAND != 0 {
        cpu.bus.guest_write_32(lp + 8, b.command as u32);
    }
    if mask & TBIF_IMAGE != 0 {
        cpu.bus.guest_write_32(lp + 12, b.bitmap as u32);
    }
    if mask & TBIF_LPARAM != 0 {
        cpu.bus.guest_write_32(lp + 20, b.data);
    }
    if mask & TBIF_SIZE != 0 {
        // A separator's width is the one the program set.
        let cx = if b.style & BTNS_SEP != 0 {
            b.cx as u16
        } else {
            b.rect.width() as u16
        };
        cpu.bus.guest_write_16(lp + 18, cx);
    }
    if mask & TBIF_STATE != 0 {
        cpu.bus.guest_write_8(lp + 16, b.state as u8);
    }
    if mask & TBIF_STYLE != 0 {
        cpu.bus.guest_write_8(lp + 17, b.style as u8);
    }
    if mask & TBIF_TEXT != 0 {
        // Only the button's own text: not the pool's.
        let (at, chars) = (w[6], w[7] as usize);
        match &b.text {
            Some(text) => write_text(cpu, at, text, wide, chars),
            None if at != 0 => write_text(cpu, at, b"", wide, 1),
            None => {}
        }
    }
    index as u32
}

/// TOOLBAR_SetButtonInfo.
fn set_button_info(cpu: &mut Cpu, hwnd: u32, t: &mut Toolbar, id: u32, lp: u32, wide: bool) -> u32 {
    if lp == 0 {
        return 0;
    }
    let w = read_words(cpu, lp, 8);
    if cpu.bus.guest_faulted() || w[0] < TBBUTTONINFO_SIZE {
        return 0;
    }
    let mask = w[1];
    let index = button_index(t, id as i32, mask & TBIF_BYINDEX != 0);
    if index < 0 {
        return 0;
    }
    let text = if mask & TBIF_TEXT != 0 && w[6] != u32::MAX && w[6] != 0 {
        let Some(text) = read_text(cpu, w[6], wide, 4096) else {
            return 0;
        };
        Some(text)
    } else {
        None
    };
    let b = &mut t.buttons[index as usize];
    if mask & TBIF_COMMAND != 0 {
        b.command = w[2] as i32;
    }
    if mask & TBIF_IMAGE != 0 {
        b.bitmap = w[3] as i32;
    }
    if mask & TBIF_LPARAM != 0 {
        b.data = w[5];
    }
    if mask & TBIF_SIZE != 0 {
        b.cx = (w[4] >> 16) as u16 as i32;
    }
    if mask & TBIF_STATE != 0 {
        b.state = w[4] & 0xFF;
    }
    if mask & TBIF_STYLE != 0 {
        b.style = (w[4] >> 8) & 0xFF;
    }
    if mask & TBIF_TEXT != 0 && w[6] != u32::MAX {
        b.string = w[6];
        // A null text makes the button the pool's first string's.
        b.text = text;
    }
    let old = b.rect;
    layout(cpu, hwnd, t);
    let new = t.buttons[index as usize].rect;
    if old != new {
        super::invalidate(cpu, hwnd, true);
    } else {
        invalidate_rect(cpu, hwnd, new, true);
    }
    1
}

/// TB_GETBUTTONTEXTA and W.
fn get_button_text(cpu: &mut Cpu, t: &Toolbar, id: u32, lp: u32, wide: bool) -> u32 {
    let index = button_index(t, id as i32, false);
    if index < 0 {
        return u32::MAX;
    }
    match text_of(t, &t.buttons[index as usize]) {
        Some(text) => {
            if lp != 0 {
                write_text(cpu, lp, &text, wide, text.len() + 1);
            }
            text.len() as u32
        }
        // Wine's ANSI answer: WideCharToMultiByte of no text, less one.
        None if !wide => u32::MAX,
        None => 0,
    }
}

/// TB_GETSTRINGA and W.
fn get_string(cpu: &mut Cpu, t: &Toolbar, wp: u32, lp: u32, wide: bool) -> u32 {
    let index = (wp >> 16) as usize;
    let size = wp & 0xFFFF;
    let Some(text) = t.strings.get(index) else {
        return u32::MAX;
    };
    let len = text.len();
    if wide {
        let max = ((size / 2) as u16).wrapping_sub(1) as usize;
        let n = max.min(len);
        if lp != 0 {
            write_text(cpu, lp, &text[..n], true, n + 1);
        }
        n as u32
    } else if lp == 0 || size == 0 {
        len as u32
    } else if (size as usize) > len {
        write_text(cpu, lp, text, false, len + 1);
        len as u32
    } else {
        // It doesn't fit: as much as does, and no 0.
        cpu.bus.guest_write_bytes(lp, &text[..size as usize]);
        u32::MAX
    }
}

/// The steps that make the tooltip control
/// (TOOLBAR_TooltipCreateControl); with `answer`, the script returns
/// its handle.
fn create_tooltip(t: &mut Toolbar, hwnd: u32, s: &mut Script, answer: bool) {
    t.tip_creating = true;
    let class = s.string(b"tooltips_class32");
    let f = user32::address_of("CreateWindowExA");
    let v = Arg::V;
    let args = [
        v(0),
        Arg::S(class),
        v(0),
        v(WS_POPUP),
        v(CW_USEDEFAULT),
        v(CW_USEDEFAULT),
        v(CW_USEDEFAULT),
        v(CW_USEDEFAULT),
        v(hwnd),
        v(0),
        v(0),
        v(0),
    ];
    s.call(f, &args);
    s.then(T_TIP_MADE, &[answer as u32]);
}

/// The tooltip control is made (T_TIP_MADE): the tools of the buttons,
/// and NM_TOOLTIPSCREATED.
fn tooltip_made(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let hwnd = c.hwnd;
    let tip = c.last;
    if c.word(0) != 0 {
        c.result = tip;
    }
    let Some(mut t) = get(cpu, hwnd) else {
        return Vec::new();
    };
    t.tip_creating = false;
    t.tooltip = tip;
    let mut s = cont_script(hwnd);
    if tip != 0 {
        notify(cpu, &mut s, hwnd, &t, NM_TOOLTIPSCREATED, &[tip]);
        for b in &t.buttons {
            if b.style & BTNS_SEP == 0 {
                tooltips::add_tool(cpu, tip, hwnd, b.command as u32, b.rect);
            }
        }
    }
    put(cpu, hwnd, t);
    s.ops
}

/// The NMMOUSE words after its header for the button `hit`, at (x, y)
/// of the toolbar.
fn nmmouse(cpu: &mut Cpu, hwnd: u32, t: &Toolbar, hit: i32, x: i32, y: i32) -> Vec<u32> {
    let (spec, data) = if hit < 0 {
        (u32::MAX, 0)
    } else {
        let b = &t.buttons[hit as usize];
        (b.command as u32, b.data)
    };
    let (ox, oy) = user32::user(cpu).client_origin(hwnd);
    vec![spec, data, (x + ox) as u32, (y + oy) as u32, hit as u32]
}

/// The steps of a pressed button (the end of TOOLBAR_LButtonDown's
/// button part).
fn press(cpu: &mut Cpu, hwnd: u32, t: &mut Toolbar, s: &mut Script, hit: i32) {
    t.captured = true;
    t.button_down = hit;
    t.drag_out_sent = false;
    let b = &mut t.buttons[hit as usize];
    b.state |= TBSTATE_PRESSED;
    let (enabled, r) = (b.state & TBSTATE_ENABLED != 0, b.rect);
    set_hot_item_ex(cpu, hwnd, t, s, hit, HICF_MOUSE | HICF_LMOUSE);
    if enabled {
        invalidate_rect(cpu, hwnd, r, true);
    }
    s.user("UpdateWindow", &[hwnd]);
    s.user("SetCapture", &[hwnd]);
}

/// The end of TOOLBAR_LButtonDown: TBN_BEGINDRAG, NM_LDOWN.
#[allow(clippy::too_many_arguments)]
fn button_down_end(
    cpu: &mut Cpu,
    hwnd: u32,
    t: &Toolbar,
    s: &mut Script,
    hit: i32,
    wp: u32,
    lp: u32,
) {
    if hit >= 0 {
        let id = t.buttons[hit as usize].command;
        let more = nmtoolbar(id, None, Rect::default());
        notify(cpu, s, hwnd, t, TBN_BEGINDRAG, &more);
    }
    let (x, y) = user32::point_of(lp);
    let more = nmmouse(cpu, hwnd, t, hit, x, y);
    notify(cpu, s, hwnd, t, NM_LDOWN, &more);
    s.then(T_DEFAULT, &[WM_LBUTTONDOWN, wp, lp]);
}

/// TOOLBAR_LButtonDown.
fn l_button_down(cpu: &mut Cpu, hwnd: u32, t: &mut Toolbar, s: &mut Script, wp: u32, lp: u32) {
    // TBSTYLE_ALTDRAG's Alt isn't known here: Shift drags.
    let drag_key = t.style & TBSTYLE_ALTDRAG == 0 && wp & MK_SHIFT != 0;
    if t.tooltip != 0 {
        tooltips::relay(cpu, t.tooltip, hwnd, WM_LBUTTONDOWN, lp, s);
    }
    let (x, y) = user32::point_of(lp);
    let hit = hit_test(t, x, y);
    if hit >= 0 && drag_key && t.style & CCS_ADJUSTABLE != 0 {
        t.button_drag = hit;
        s.user("SetCapture", &[hwnd]);
    } else if hit >= 0 {
        t.old_hit = hit;
        let b = t.buttons[hit as usize].clone();
        let mut arrow = b.rect;
        arrow.left = b.rect.left.max(b.rect.right - DDARROW_WIDTH);
        let dd_arrows = t.ex_style & TBSTYLE_EX_DRAWDDARROWS != 0;
        let drops = b.style & BTNS_WHOLEDROPDOWN != 0
            || (b.style & BTNS_DROPDOWN != 0 && (!dd_arrows || arrow.contains(x, y)));
        if b.state & TBSTATE_ENABLED != 0 && drops {
            let bb = &mut t.buttons[hit as usize];
            if b.style & BTNS_WHOLEDROPDOWN != 0 {
                bb.state |= TBSTATE_PRESSED;
            } else {
                bb.dropdown_pressed = true;
            }
            invalidate_rect(cpu, hwnd, b.rect, true);
            s.user("UpdateWindow", &[hwnd]);
            let more = nmtoolbar(b.command, None, b.rect);
            notify(cpu, s, hwnd, t, TBN_DROPDOWN, &more);
            s.then(T_DROPDOWN, &[hit as u32, wp, lp]);
            return;
        }
        press(cpu, hwnd, t, s, hit);
    }
    button_down_end(cpu, hwnd, t, s, hit, wp, lp);
}

/// TBN_DROPDOWN's answer (T_DROPDOWN).
fn dropped_down(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let hwnd = c.hwnd;
    let (hit, wp, lp) = (c.word(0) as i32, c.word(1), c.word(2));
    let Some(mut t) = get(cpu, hwnd) else {
        return Vec::new();
    };
    if hit < 0 || hit as usize >= t.buttons.len() {
        return Vec::new();
    }
    let mut s = cont_script(hwnd);
    if c.last != TBDDRET_TREATPRESSED {
        let b = &mut t.buttons[hit as usize];
        if b.style & BTNS_WHOLEDROPDOWN != 0 {
            b.state &= !TBSTATE_PRESSED;
        } else {
            b.dropdown_pressed = false;
        }
        let r = b.rect;
        invalidate_rect(cpu, hwnd, r, true);
        let (cx, cy) = user32::user(cpu).input.cursor;
        let (ox, oy) = user32::user(cpu).client_origin(hwnd);
        let now = hit_test(&t, cx - ox, cy - oy);
        if !t.anchor || now >= 0 {
            set_hot_item_ex(cpu, hwnd, &t, &mut s, now, HICF_MOUSE | HICF_LMOUSE);
        }
        c.result = 0;
    } else {
        press(cpu, hwnd, &mut t, &mut s, hit);
        button_down_end(cpu, hwnd, &t, &mut s, hit, wp, lp);
    }
    put(cpu, hwnd, t);
    s.ops
}

/// TOOLBAR_LButtonUp.
fn l_button_up(cpu: &mut Cpu, hwnd: u32, t: &mut Toolbar, s: &mut Script, lp: u32) {
    if t.tooltip != 0 {
        tooltips::relay(cpu, t.tooltip, hwnd, WM_LBUTTONUP, lp, s);
    }
    let (x, y) = user32::point_of(lp);
    let hit = hit_test(t, x, y);
    if !t.anchor || hit >= 0 {
        set_hot_item_ex(cpu, hwnd, t, s, hit, HICF_MOUSE | HICF_LMOUSE);
    }
    let n = t.buttons.len() as i32;
    if t.button_drag >= 0 && t.button_drag < n {
        let drag = t.button_drag;
        let br = t.buttons[drag as usize].rect;
        s.user("ReleaseCapture", &[]);
        let client = super::client(cpu, hwnd);
        if client.contains(x, y) {
            let target = if hit >= 0 {
                hit
            } else if hit < -1 {
                -hit
            } else {
                -1
            };
            if target == drag {
                // Moved a little left: the separator before it goes;
                // right: a separator comes before it.
                if x < br.left + br.width() / 2 {
                    if target > 0 && t.buttons[target as usize - 1].style & BTNS_SEP != 0 {
                        delete_button(cpu, hwnd, t, s, target - 1);
                    }
                } else {
                    let sep = Button {
                        style: BTNS_SEP,
                        string: u32::MAX,
                        disp_image: I_IMAGENONE,
                        ..Button::default()
                    };
                    t.buttons.insert(target as usize, sep);
                    layout(cpu, hwnd, t);
                    auto_size(cpu, hwnd, t, s, false);
                    super::invalidate(cpu, hwnd, true);
                }
            } else if target == -1 {
                let first_left = t.buttons.first().map_or(0, |b| b.rect.left);
                let to = if n > 0 && x < first_left { 0 } else { n };
                move_button(cpu, hwnd, t, s, drag, to);
            } else {
                move_button(cpu, hwnd, t, s, drag, target);
            }
        } else {
            delete_button(cpu, hwnd, t, s, drag);
        }
        set_hot_item_ex(cpu, hwnd, t, s, hit, HICF_MOUSE | HICF_LMOUSE);
        t.button_drag = -1;
        notify(cpu, s, hwnd, t, TBN_TOOLBARCHANGE, &[]);
    } else if t.button_down >= 0 && t.button_down < n {
        let down = t.button_down;
        let mut old_index = -1;
        // A button counts as clicked when the mouse is let go on it.
        let clicked = hit == down;
        let b = &mut t.buttons[down as usize];
        b.state &= !TBSTATE_PRESSED;
        let style = b.style;
        if clicked && style & BTNS_CHECK != 0 {
            if style & BTNS_GROUP != 0 {
                old_index = checked_group_index(t, hit);
                if old_index != hit && old_index != -1 {
                    t.buttons[old_index as usize].state &= !TBSTATE_CHECKED;
                }
                t.buttons[down as usize].state |= TBSTATE_CHECKED;
            } else {
                t.buttons[down as usize].state ^= TBSTATE_CHECKED;
            }
        }
        if old_index != -1 {
            let r = t.buttons[old_index as usize].rect;
            invalidate_rect(cpu, hwnd, r, true);
        }
        let r = t.buttons[down as usize].rect;
        invalidate_rect(cpu, hwnd, r, true);
        if t.captured {
            s.user("ReleaseCapture", &[]);
        }
        t.captured = false;
        t.old_hit = -1;
        t.button_down = -1;
        notify(cpu, s, hwnd, t, NM_RELEASEDCAPTURE, &[]);
        let b = t.buttons[down as usize].clone();
        let more = nmtoolbar(b.command, None, Rect::default());
        notify(cpu, s, hwnd, t, TBN_ENDDRAG, &more);
        if clicked && b.state & TBSTATE_ENABLED != 0 {
            let wp = (b.command as u32 & 0xFFFF) | (BN_CLICKED << 16);
            s.send(t.notify, WM_COMMAND, Arg::V(wp), Arg::V(hwnd));
            // The program may have destroyed the toolbar.
            s.then(T_CLICKED, &[hit as u32, x as u32, y as u32, lp]);
            return;
        }
    }
    click_end(cpu, hwnd, t, s, hit, (x, y), lp);
}

/// The end of TOOLBAR_LButtonUp: NM_CLICK.
fn click_end(
    cpu: &mut Cpu,
    hwnd: u32,
    t: &Toolbar,
    s: &mut Script,
    hit: i32,
    (x, y): (i32, i32),
    lp: u32,
) {
    let hit = if hit >= 0 && hit as usize >= t.buttons.len() {
        -1
    } else {
        hit
    };
    let more = nmmouse(cpu, hwnd, t, hit, x, y);
    notify(cpu, s, hwnd, t, NM_CLICK, &more);
    s.then(T_DEFAULT, &[WM_LBUTTONUP, 0, lp]);
}

/// TOOLBAR_MouseLeave.
fn mouse_leave(cpu: &mut Cpu, hwnd: u32, t: &mut Toolbar, s: &mut Script) {
    let n = t.buttons.len() as i32;
    if t.hot_item >= 0
        && t.hot_item < n
        && !t.anchor
        && !t.buttons[t.hot_item as usize].dropdown_pressed
    {
        set_hot_item_ex(cpu, hwnd, t, s, -1, HICF_MOUSE);
    }
    if t.old_hit < 0 {
        return;
    }
    if t.old_hit == t.button_down && t.button_down >= 0 && t.button_down < n {
        let b = &mut t.buttons[t.button_down as usize];
        b.state &= !TBSTATE_PRESSED;
        let r = b.rect.inflate(1, 1);
        invalidate_rect(cpu, hwnd, r, true);
    }
    if t.captured && !t.drag_out_sent && t.button_down >= 0 && t.button_down < n {
        let id = t.buttons[t.button_down as usize].command;
        let more = nmtoolbar(id, None, Rect::default());
        notify(cpu, s, hwnd, t, TBN_DRAGOUT, &more);
        t.drag_out_sent = true;
    }
    t.old_hit = -1;
}

/// TOOLBAR_MouseMove, after the tooltip control is made.
fn mouse_move(cpu: &mut Cpu, hwnd: u32, t: &mut Toolbar, s: &mut Script, lp: u32) {
    if t.style & TBSTYLE_FLAT != 0 && !t.leave_timer {
        // No TrackMouseEvent: a timer looks for the mouse leaving.
        user32::message::set_timer(cpu, &[hwnd, ID_LEAVE, LEAVE_MS, 0]);
        t.leave_timer = true;
    }
    if t.tooltip != 0 {
        tooltips::relay(cpu, t.tooltip, hwnd, WM_MOUSEMOVE, lp, s);
    }
    let (x, y) = user32::point_of(lp);
    let hit = hit_test(t, x, y);
    if t.style & TBSTYLE_FLAT != 0 && (!t.anchor || hit >= 0) {
        set_hot_item_ex(cpu, hwnd, t, s, hit, HICF_MOUSE);
    }
    let n = t.buttons.len() as i32;
    if t.old_hit != hit && t.captured && t.button_down >= 0 && t.button_down < n {
        let down = t.button_down;
        if !t.drag_out_sent {
            let id = t.buttons[down as usize].command;
            let more = nmtoolbar(id, None, Rect::default());
            notify(cpu, s, hwnd, t, TBN_DRAGOUT, &more);
            t.drag_out_sent = true;
        }
        let old_hit = t.old_hit;
        let b = &mut t.buttons[down as usize];
        if old_hit == down {
            b.state &= !TBSTATE_PRESSED;
            let r = b.rect;
            invalidate_rect(cpu, hwnd, r, true);
        } else if hit == down {
            b.state |= TBSTATE_PRESSED;
            let r = b.rect;
            invalidate_rect(cpu, hwnd, r, true);
        }
        t.old_hit = hit;
    }
}

/// TOOLBAR_KeyDown, after NM_KEYDOWN (T_KEY).
fn key_down(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let hwnd = c.hwnd;
    let (wp, lp) = (c.word(0), c.word(1));
    if c.last != 0 {
        c.result = defwnd::def_window_proc(cpu, &[hwnd, WM_KEYDOWN, wp, lp]).unwrap_or(0);
        return Vec::new();
    }
    c.result = 0;
    let Some(t) = get(cpu, hwnd) else {
        return Vec::new();
    };
    let mut s = cont_script(hwnd);
    let n = t.buttons.len() as i32;
    match wp {
        VK_LEFT | VK_UP | VK_RIGHT | VK_DOWN if n > 0 => {
            // TOOLBAR_SetRelativeHotItem, without TBN_WRAPHOTITEM.
            let dir = if wp == VK_LEFT || wp == VK_UP { -1 } else { 1 };
            let mut hot = t.hot_item;
            for _ in 0..n {
                hot = ((hot + dir) % n + n) % n;
                let b = &t.buttons[hot as usize];
                if b.state & TBSTATE_ENABLED != 0 && b.style & BTNS_SEP == 0 {
                    set_hot_item_ex(cpu, hwnd, &t, &mut s, hot, HICF_ARROWKEYS);
                    break;
                }
            }
        }
        VK_SPACE | VK_RETURN => {
            let hot = t.hot_item;
            if hot >= 0 && hot < n && t.buttons[hot as usize].state & TBSTATE_ENABLED != 0 {
                let id = t.buttons[hot as usize].command as u32 & 0xFFFF;
                let wp = id | (BN_CLICKED << 16);
                s.send(t.notify, WM_COMMAND, Arg::V(wp), Arg::V(hwnd));
            }
        }
        _ => {}
    }
    s.ops
}

/// TOOLBAR_TTGetDispInfo: the text of the tooltip of the button the
/// NMTTDISPINFOA or W at `lp` asks for: TBN_GETINFOTIP to the parent,
/// then the button's text, then the parent's TTN_GETDISPINFOA.
fn tt_get_disp_info(cpu: &mut Cpu, t: &mut Toolbar, s: &mut Script, lp: u32) -> u32 {
    let hdr = read_words(cpu, lp, 3);
    if cpu.bus.guest_faulted() {
        return 0;
    }
    if t.tip_text != 0 {
        let heap = process_heap(cpu);
        heap_free(cpu, heap, t.tip_text);
        t.tip_text = 0;
    }
    let index = button_index(t, hdr[1] as i32, false);
    if index < 0 {
        return 0;
    }
    s.then(T_TIP_ASK, &[lp, hdr[1], index as u32, hdr[2]]);
    0
}

/// TBN_GETINFOTIP to the parent (T_TIP_ASK).
fn tip_ask(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let hwnd = c.hwnd;
    let Some(t) = get(cpu, hwnd) else {
        return Vec::new();
    };
    let index = c.word(2) as usize;
    let Some(b) = t.buttons.get(index) else {
        return Vec::new();
    };
    let code = if t.unicode {
        TBN_GETINFOTIPW
    } else {
        TBN_GETINFOTIPA
    };
    let id = super::id_of(cpu, hwnd);
    let words = vec![
        hwnd,
        id,
        code,
        c.scratch + O_TIP_BUF,
        tooltips::INFOTIPSIZE,
        c.word(1),
        b.data,
    ];
    let mut s = cont_script(hwnd);
    s.ops.push(Op::Write {
        at: O_TIP_BUF,
        words: vec![0],
    });
    s.ops.push(Op::Write {
        at: O_TIP_NM,
        words,
    });
    s.send(t.notify, WM_NOTIFY, Arg::V(id), Arg::S(O_TIP_NM));
    s.then(T_TIP_GOT, &c.data);
    s.ops
}

/// Put `text` in the NMTTDISPINFO at `lp` (W or A): in its szText, or
/// a buffer of the toolbar's if it is too long.
fn give_tip_text(cpu: &mut Cpu, t: &mut Toolbar, lp: u32, text: &[u8], wide: bool) {
    if text.len() >= DISP_CHARS {
        let heap = process_heap(cpu);
        let size = (text.len() as u32 + 1) * if wide { 2 } else { 1 };
        let buf = heap_alloc(cpu, heap, size);
        if buf != 0 {
            t.tip_text = buf;
            write_text(cpu, buf, text, wide, text.len() + 1);
            cpu.bus.guest_write_32(lp + DISP_TEXT, buf);
            return;
        }
    }
    let mut at = cpu.bus.guest_read_32(lp + DISP_TEXT);
    if at < 0x10000 {
        at = lp + DISP_SZ;
        cpu.bus.guest_write_32(lp + DISP_TEXT, at);
    }
    write_text(cpu, at, text, wide, DISP_CHARS);
}

/// TBN_GETINFOTIP's answer (T_TIP_GOT).
fn tip_got(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let hwnd = c.hwnd;
    let (lp, id_from, index, code) = (c.word(0), c.word(1), c.word(2) as usize, c.word(3));
    let wide_disp = code == tooltips::TTN_GETDISPINFOW;
    let Some(mut t) = get(cpu, hwnd) else {
        return Vec::new();
    };
    let at = cpu.bus.guest_read_32(c.scratch + O_TIP_NM + 12);
    let text = read_text(cpu, at, t.unicode, 1024).unwrap_or_default();
    c.result = 0;
    if !text.is_empty() {
        give_tip_text(cpu, &mut t, lp, &text, wide_disp);
        put(cpu, hwnd, t);
        return Vec::new();
    }
    // A text that isn't shown is the tooltip.
    if let Some(b) = t.buttons.get(index)
        && t.ex_style & TBSTYLE_EX_MIXEDBUTTONS != 0
        && b.style & BTNS_SHOWTEXT == 0
    {
        let text = text_of(&t, b).unwrap_or_default();
        if !text.is_empty() {
            give_tip_text(cpu, &mut t, lp, &text, wide_disp);
            put(cpu, hwnd, t);
            return Vec::new();
        }
    }
    put(cpu, hwnd, t.clone());
    // Last, the parent: TTN_GETDISPINFOA from an ANSI toolbar, as
    // Windows sends it.
    let mut s = cont_script(hwnd);
    if !wide_disp || t.unicode {
        s.send(t.notify, WM_NOTIFY, Arg::V(id_from), Arg::V(lp));
        s.ret_last();
        return s.ops;
    }
    let w = read_words(cpu, lp, 3);
    let more = read_words(cpu, lp + DISPW_HINST, 3);
    if cpu.bus.guest_faulted() {
        return Vec::new();
    }
    let mut words = vec![w[0], w[1], tooltips::TTN_GETDISPINFOA];
    words.push(c.scratch + O_DISP_A + DISP_SZ);
    words.extend([0; 20]);
    words.extend(more);
    s.ops.push(Op::Write {
        at: O_DISP_A,
        words,
    });
    s.send(t.notify, WM_NOTIFY, Arg::V(id_from), Arg::S(O_DISP_A));
    s.then(T_DISP_BACK, &[lp]);
    s.ops
}

/// The parent's TTN_GETDISPINFOA answer, put in the NMTTDISPINFOW the
/// toolbar was asked with (T_DISP_BACK).
fn disp_back(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let hwnd = c.hwnd;
    let lp = c.word(0);
    c.result = c.last;
    let a = c.scratch + O_DISP_A;
    let text_at = cpu.bus.guest_read_32(a + DISP_TEXT);
    let inst = cpu.bus.guest_read_32(a + DISPA_HINST);
    let flags = cpu.bus.guest_read_32(a + DISPA_FLAGS);
    let param = cpu.bus.guest_read_32(a + DISPA_LPARAM);
    write_words(cpu, lp + DISPW_HINST, &[inst, flags, param]);
    if text_at != 0 && text_at < 0x10000 {
        // A string resource's id.
        cpu.bus.guest_write_32(lp + DISP_TEXT, text_at);
        return Vec::new();
    }
    let Some(mut t) = get(cpu, hwnd) else {
        return Vec::new();
    };
    let text = read_ansi(cpu, text_at, 1024).unwrap_or_default();
    // The program's own szText pointer is replaced by the toolbar's.
    cpu.bus.guest_write_32(lp + DISP_TEXT, lp + DISP_SZ);
    give_tip_text(cpu, &mut t, lp, &text, true);
    put(cpu, hwnd, t);
    Vec::new()
}

/// TOOLBAR_EraseBackground: a transparent toolbar has its parent paint
/// the background, through the DC moved to the parent's coordinates.
fn erase_background(
    cpu: &mut Cpu,
    hwnd: u32,
    t: &Toolbar,
    s: &mut Script,
    wp: u32,
    lp: u32,
) -> u32 {
    if !t.redraw {
        return 0;
    }
    if t.style & TBSTYLE_TRANSPARENT != 0 {
        let parent = super::parent(cpu, hwnd);
        let (ox, oy) = user32::user(cpu).client_origin(hwnd);
        let (px, py) = user32::user(cpu).client_origin(parent);
        let (dx, dy) = (ox - px, oy - py);
        let heap = process_heap(cpu);
        let at = heap_alloc(cpu, heap, 8);
        gdi(cpu, "OffsetWindowOrgEx", &[wp, dx as u32, dy as u32, at]);
        let (x, y) = if at != 0 {
            let w = read_words(cpu, at, 2);
            heap_free(cpu, heap, at);
            (w[0], w[1])
        } else {
            (0, 0)
        };
        s.send(parent, WM_ERASEBKGND, Arg::V(wp), Arg::V(lp));
        s.then(T_ERASED, &[wp, x, y, lp]);
        return 0;
    }
    defwnd::def_window_proc(cpu, &[hwnd, WM_ERASEBKGND, wp, lp]).unwrap_or(0)
}

/// The parent's WM_ERASEBKGND answer (T_ERASED).
fn erased(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let (hdc, x, y, lp) = (c.word(0), c.word(1), c.word(2), c.word(3));
    gdi(cpu, "SetWindowOrgEx", &[hdc, x, y, 0]);
    c.result = c.last;
    if c.last == 0 {
        c.result = defwnd::def_window_proc(cpu, &[c.hwnd, WM_ERASEBKGND, hdc, lp]).unwrap_or(0);
    }
    Vec::new()
}

/// TOOLBAR_NCPaint: the etched line at the top, unless CCS_NODIVIDER.
fn nc_paint(cpu: &mut Cpu, hwnd: u32, wp: u32, lp: u32) -> u32 {
    let style = super::style(cpu, hwnd);
    if style & WS_MINIMIZE != 0 {
        return 0;
    }
    defwnd::def_window_proc(cpu, &[hwnd, WM_NCPAINT, wp, lp]);
    if style & CCS_NODIVIDER != 0 {
        return 0;
    }
    let hdc = user32::paint::get_dc_ex(cpu, &[hwnd, 0, DCX_USESTYLE | DCX_WINDOW]).unwrap_or(0);
    if hdc == 0 {
        return 0;
    }
    let w = user32::user(cpu)
        .window(hwnd)
        .map(|w| w.rect)
        .unwrap_or_default();
    let mut r = Rect::new(0, 0, w.width(), w.height());
    if style & WS_BORDER != 0 {
        r = r.inflate(-1, -1);
    }
    gfx::edge(cpu, hdc, r, EDGE_ETCHED, BF_TOP);
    user32::paint::release_dc(cpu, &[hwnd, hdc]);
    0
}

/// The steps before painting: TBN_GETDISPINFOW for the images of the
/// I_IMAGECALLBACK buttons (Windows sends it to ANSI toolbars too).
fn ask_images(cpu: &mut Cpu, hwnd: u32, t: &Toolbar, s: &mut Script) {
    for (i, b) in t.buttons.iter().enumerate() {
        let hidden = b.state & TBSTATE_HIDDEN != 0;
        if b.bitmap != I_IMAGECALLBACK || b.style & BTNS_SEP != 0 || hidden {
            continue;
        }
        let more = [TBNF_IMAGE, b.command as u32, b.data, u32::MAX, 0, 0];
        let at = notify(cpu, s, hwnd, t, TBN_GETDISPINFOW, &more);
        s.then(T_DISP_IMAGE, &[i as u32, at]);
    }
}

/// TBN_GETDISPINFOW's answer (T_DISP_IMAGE).
fn image_got(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let (i, at) = (c.word(0) as usize, c.scratch + c.word(1));
    let mask = cpu.bus.guest_read_32(at + 12);
    let image = cpu.bus.guest_read_32(at + 24) as i32;
    if let Some(mut t) = get(cpu, c.hwnd) {
        if let Some(b) = t.buttons.get_mut(i) {
            b.disp_image = image;
            if mask & TBNF_DI_SETITEM != 0 {
                b.bitmap = image;
            }
        }
        put(cpu, c.hwnd, t);
    }
    Vec::new()
}

/// TOOLBAR_TranslateState: a button's custom draw state.
fn draw_state(b: &Button) -> u32 {
    let mut s = 0;
    if b.state & TBSTATE_CHECKED != 0 {
        s |= CDIS_CHECKED;
    }
    if b.state & TBSTATE_PRESSED != 0 {
        s |= CDIS_SELECTED;
    }
    if b.state & TBSTATE_ENABLED == 0 {
        s |= CDIS_DISABLED;
    }
    if b.state & TBSTATE_MARKED != 0 {
        s |= CDIS_MARKED;
    }
    if b.hot {
        s |= CDIS_HOT;
    }
    let ei = TBSTATE_ENABLED | TBSTATE_INDETERMINATE;
    if b.state & ei == ei {
        s |= CDIS_INDETERMINATE;
    }
    s
}

fn shadow(cpu: &mut Cpu, t: &Toolbar) -> u32 {
    if t.clr_shadow == gfx::CLR_DEFAULT {
        gfx::sys_color(cpu, gfx::COLOR_BTNSHADOW)
    } else {
        t.clr_shadow
    }
}

fn highlight(cpu: &mut Cpu, t: &Toolbar) -> u32 {
    if t.clr_highlight == gfx::CLR_DEFAULT {
        gfx::sys_color(cpu, gfx::COLOR_BTNHIGHLIGHT)
    } else {
        t.clr_highlight
    }
}

/// TOOLBAR_DrawArrow: the drop-down arrow at (left, top).
fn draw_arrow(cpu: &mut Cpu, hdc: u32, left: i32, top: i32, color: u32) {
    for (dx, w) in [(2, 5), (3, 3), (4, 1)] {
        let y = top + dx - 2;
        gfx::fill_rgb(
            cpu,
            hdc,
            Rect::new(left + dx, y, left + dx + w, y + 1),
            color,
        );
    }
}

/// The image list of the kind `kind` (0 default, 1 hot, 2 disabled)
/// and the image to draw a button with
/// (TOOLBAR_GetImageListForDrawing).
fn list_for_drawing(cpu: &mut Cpu, t: &Toolbar, b: &Button, kind: u32) -> Option<(u32, i32)> {
    if !is_valid_bitmap_index(cpu, t, b.bitmap) {
        return None;
    }
    let raw = if b.bitmap == I_IMAGECALLBACK {
        b.disp_image
    } else {
        b.bitmap
    };
    let index = if raw != I_IMAGENONE {
        image_index(t, raw)
    } else {
        raw
    };
    if index < 0 {
        return None;
    }
    let id = list_id(t, b.bitmap);
    let himl = match kind {
        0 => def_list(t, id),
        1 => list_of(&t.hot_lists, id),
        _ => list_of(&t.dis_lists, id),
    };
    (himl != 0).then_some((himl, index))
}

/// TOOLBAR_DrawMasked: the image etched, for a disabled button with no
/// disabled image list.
fn draw_masked(cpu: &mut Cpu, himl: u32, index: i32, hdc: u32, x: i32, y: i32, flags: u32) {
    let Some((cx, cy)) = imagelist::icon_size(cpu, himl) else {
        return;
    };
    let (cxu, cyu) = (cx as u32, cy as u32);
    let image_dc = gdi(cpu, "CreateCompatibleDC", &[hdc]);
    let image = gdi(cpu, "CreateCompatibleBitmap", &[hdc, cxu, cyu]);
    let old_image = gdi(cpu, "SelectObject", &[image_dc, image]);
    imagelist::draw_ex(
        cpu, himl, index, image_dc, 0, 0, cx, cy, 0xFF_FFFF, 0, flags,
    );
    let mask_dc = gdi(cpu, "CreateCompatibleDC", &[0]);
    let mask = gdi(cpu, "CreateBitmap", &[cxu, cyu, 1, 1, 0]);
    let old_mask = gdi(cpu, "SelectObject", &[mask_dc, mask]);
    imagelist::draw_ex(
        cpu, himl, index, mask_dc, 0, 0, cx, cy, 0xFF_FFFF, 0, ILD_MASK,
    );
    // White and the background become white in the mask too.
    gfx::set_bk_color(cpu, image_dc, 0xFF_FFFF);
    let args = [mask_dc, 0, 0, cxu, cyu, image_dc, 0, 0, NOTSRCERASE];
    gdi(cpu, "BitBlt", &args);
    let old_bk = gfx::set_bk_color(cpu, hdc, 0xFF_FFFF);
    let light = gfx::sys_brush(cpu, gfx::COLOR_BTNHIGHLIGHT);
    let old_brush = gfx::select(cpu, hdc, light);
    let (xu, yu) = (x as u32, y as u32);
    let args = [hdc, xu + 1, yu + 1, cxu, cyu, mask_dc, 0, 0, ROP_ETCH];
    gdi(cpu, "BitBlt", &args);
    let dark = gfx::sys_brush(cpu, gfx::COLOR_BTNSHADOW);
    gfx::select(cpu, hdc, dark);
    gdi(
        cpu,
        "BitBlt",
        &[hdc, xu, yu, cxu, cyu, mask_dc, 0, 0, ROP_ETCH],
    );
    gfx::select(cpu, hdc, old_brush);
    gfx::set_bk_color(cpu, hdc, old_bk);
    gdi(cpu, "SelectObject", &[image_dc, old_image]);
    gdi(cpu, "SelectObject", &[mask_dc, old_mask]);
    gdi(cpu, "DeleteObject", &[image]);
    gdi(cpu, "DeleteDC", &[image_dc]);
    gdi(cpu, "DeleteObject", &[mask]);
    gdi(cpu, "DeleteDC", &[mask_dc]);
}

/// TOOLBAR_DrawImage.
fn draw_image(cpu: &mut Cpu, t: &Toolbar, b: &Button, hdc: u32, x: i32, y: i32, state: u32) {
    let mut masked = false;
    let found = if state & (CDIS_DISABLED | CDIS_INDETERMINATE) != 0 {
        list_for_drawing(cpu, t, b, 2).or_else(|| {
            masked = true;
            list_for_drawing(cpu, t, b, 0)
        })
    } else if state & CDIS_CHECKED != 0 || (state & CDIS_HOT != 0 && t.style & TBSTYLE_FLAT != 0) {
        list_for_drawing(cpu, t, b, 1).or_else(|| list_for_drawing(cpu, t, b, 0))
    } else {
        list_for_drawing(cpu, t, b, 0)
    };
    let Some((himl, index)) = found else {
        return;
    };
    let offset = (state & (CDIS_SELECTED | CDIS_CHECKED) != 0) as i32;
    let mut flags = ILD_TRANSPARENT;
    if state & CDIS_MARKED != 0 {
        flags |= ILD_BLEND50;
    }
    if masked {
        draw_masked(cpu, himl, index, hdc, x + offset, y + offset, flags);
    } else {
        imagelist::draw(cpu, himl, index, hdc, x + offset, y + offset, flags);
    }
}

/// TOOLBAR_DrawString.
fn draw_string(cpu: &mut Cpu, t: &Toolbar, hdc: u32, mut r: Rect, text: &[u8], state: u32) {
    let old_font = gfx::select(cpu, hdc, t.font);
    let shadow = gfx::sys_color(cpu, gfx::COLOR_BTNSHADOW);
    let mut old_bk = None;
    let old_color = if state & CDIS_DISABLED != 0 {
        let light = gfx::sys_color(cpu, gfx::COLOR_BTNHIGHLIGHT);
        let old = gfx::set_text_color(cpu, hdc, light);
        let mut moved = r.offset(1, 1);
        gfx::draw_text(cpu, hdc, text, &mut moved, t.dt_flags);
        gfx::set_text_color(cpu, hdc, shadow);
        old
    } else if state & CDIS_INDETERMINATE != 0 {
        gfx::set_text_color(cpu, hdc, shadow)
    } else if state & CDIS_MARKED != 0 {
        let fg = gfx::sys_color(cpu, gfx::COLOR_HIGHLIGHTTEXT);
        let bk = gfx::sys_color(cpu, gfx::COLOR_HIGHLIGHT);
        let old = gfx::set_text_color(cpu, hdc, fg);
        let b = gfx::set_bk_color(cpu, hdc, bk);
        let m = gfx::set_bk_mode(cpu, hdc, OPAQUE);
        old_bk = Some((b, m));
        old
    } else {
        let fg = gfx::sys_color(cpu, gfx::COLOR_BTNTEXT);
        gfx::set_text_color(cpu, hdc, fg)
    };
    gfx::draw_text(cpu, hdc, text, &mut r, t.dt_flags);
    gfx::set_text_color(cpu, hdc, old_color);
    if let Some((b, m)) = old_bk {
        gfx::set_bk_color(cpu, hdc, b);
        gfx::set_bk_mode(cpu, hdc, m);
    }
    gfx::select(cpu, hdc, old_font);
}

/// TOOLBAR_DrawFrame and TOOLBAR_DrawSepDDArrow's edges.
fn draw_frame(cpu: &mut Cpu, t: &Toolbar, hdc: u32, r: Rect, state: u32, pressed: bool) {
    let inactive = state & (CDIS_DISABLED | CDIS_INDETERMINATE) != 0;
    let pressed = pressed && !inactive;
    if t.style & TBSTYLE_FLAT != 0 {
        if pressed {
            gfx::edge(cpu, hdc, r, BDR_SUNKENOUTER, BF_RECT);
        } else if state & CDIS_HOT != 0 && !inactive {
            gfx::edge(cpu, hdc, r, BDR_RAISEDINNER, BF_RECT);
        }
    } else if pressed {
        gfx::edge(cpu, hdc, r, EDGE_SUNKEN, BF_RECT | BF_MIDDLE);
    } else {
        gfx::edge(cpu, hdc, r, EDGE_RAISED, BF_SOFT | BF_RECT | BF_MIDDLE);
    }
}

/// The checked look: the 55AA pattern in the highlight and face colors
/// (TOOLBAR_DrawPattern).
fn draw_pattern(cpu: &mut Cpu, hdc: u32, r: Rect, brush: u32) {
    let (cxe, cye) = (metric(SM_CXEDGE), metric(SM_CYEDGE));
    let old = gfx::select(cpu, hdc, brush);
    let light = gfx::sys_color(cpu, gfx::COLOR_BTNHIGHLIGHT);
    let face = gfx::sys_color(cpu, gfx::COLOR_BTNFACE);
    let old_fg = gfx::set_text_color(cpu, hdc, light);
    let old_bk = gfx::set_bk_color(cpu, hdc, face);
    let (x, y) = ((r.left + cxe) as u32, (r.top + cye) as u32);
    let (w, h) = ((r.width() - 2 * cxe) as u32, (r.height() - 2 * cye) as u32);
    gdi(cpu, "PatBlt", &[hdc, x, y, w, h, PATCOPY]);
    gfx::set_bk_color(cpu, hdc, old_bk);
    gfx::set_text_color(cpu, hdc, old_fg);
    gfx::select(cpu, hdc, old);
}

/// Draw the arrow of a drop-down button, grayed when disabled.
fn draw_dd_arrow(cpu: &mut Cpu, hdc: u32, arrow: Rect, state: u32, pressed: bool) {
    let top = arrow.top + (arrow.height() - ARROW_HEIGHT) / 2;
    if state & (CDIS_DISABLED | CDIS_INDETERMINATE) != 0 {
        let light = gfx::sys_color(cpu, gfx::COLOR_BTNHIGHLIGHT);
        let dark = gfx::sys_color(cpu, gfx::COLOR_BTNSHADOW);
        draw_arrow(cpu, hdc, arrow.left + 1, top + 1, light);
        draw_arrow(cpu, hdc, arrow.left, top, dark);
    } else {
        let color = gfx::sys_color(cpu, gfx::COLOR_BTNTEXT);
        let off = pressed as i32;
        draw_arrow(cpu, hdc, arrow.left + off, top + off, color);
    }
}

/// A flat toolbar's separator: a line.
fn draw_separator(cpu: &mut Cpu, t: &Toolbar, hdc: u32, rc: Rect) {
    let (sh, hi) = (shadow(cpu, t), highlight(cpu, t));
    if t.style & CCS_VERT != 0 {
        let top = rc.top + (rc.height() - 2) / 2;
        let line = Rect::new(rc.left + 2, top, rc.right - 2, top + 1);
        gfx::fill_rgb(cpu, hdc, line, sh);
        gfx::fill_rgb(cpu, hdc, line.offset(0, 1), hi);
    } else {
        let left = (rc.left + rc.right) / 2 - 1;
        let line = Rect::new(left, rc.top + 2, left + 1, rc.bottom - 2);
        gfx::fill_rgb(cpu, hdc, line, sh);
        gfx::fill_rgb(cpu, hdc, line.offset(1, 0), hi);
    }
}

/// TOOLBAR_DrawButton, without custom draw.
fn draw_button(cpu: &mut Cpu, t: &Toolbar, b: &Button, hdc: u32, pattern: u32) {
    let style = t.style;
    let flat = style & TBSTYLE_FLAT != 0;
    if b.style & BTNS_SEP != 0 {
        // A flat toolbar's separators are lines; the others' are space.
        if flat {
            draw_separator(cpu, t, hdc, b.rect);
        }
        return;
    }
    let has_dd = (t.ex_style & TBSTYLE_EX_DRAWDDARROWS != 0 && b.style & BTNS_DROPDOWN != 0)
        || b.style & BTNS_WHOLEDROPDOWN != 0;
    let sep_dd = has_dd && b.style & BTNS_WHOLEDROPDOWN == 0;
    let mut rc = b.rect;
    let mut arrow = rc;
    let text = text_of(t, b);
    if has_dd {
        let right = if flat {
            rc.left.max(rc.right - DDARROW_WIDTH)
        } else {
            rc.left.max(rc.right - DDARROW_WIDTH - 2)
        };
        if sep_dd {
            rc.right = right;
        }
        arrow.left = right;
    }
    let mut rc_text = rc;
    let mut rc_bitmap = rc;
    let cxedge = metric(SM_CXEDGE);
    let shows_text = t.ex_style & TBSTYLE_EX_MIXEDBUTTONS == 0 || b.style & BTNS_SHOWTEXT != 0;
    if style & TBSTYLE_LIST != 0 {
        if text.is_some() && t.max_text_rows > 0 && shows_text {
            rc_bitmap.left += cxedge + t.pad_cx / 2;
        } else {
            rc_bitmap.left += cxedge + t.list_gap / 2;
        }
    } else {
        rc_bitmap.left += (rc.width() - t.bitmap_w) / 2;
    }
    rc_bitmap.top += t.pad_cy / 2;
    if text.is_some() {
        rc_text.left += cxedge;
        rc_text.right -= cxedge;
        if style & TBSTYLE_LIST != 0 {
            rc_text.left += t.bitmap_w + t.list_gap + 2;
        } else {
            let himl = def_list(t, 0);
            if himl != 0 && imagelist::count(cpu, himl) > 0 {
                rc_text.top += t.pad_cy / 2 + t.bitmap_h + 1;
            } else {
                rc_text.top += t.pad_cy / 2 + 2;
            }
        }
    }
    let state = draw_state(b);
    if b.state & (TBSTATE_PRESSED | TBSTATE_CHECKED) != 0 {
        rc_text = rc_text.offset(1, 1);
    }
    if state & CDIS_HOT == 0 && state & (CDIS_CHECKED | CDIS_INDETERMINATE) != 0 {
        draw_pattern(cpu, hdc, rc, pattern);
    }
    let pressed = state & (CDIS_SELECTED | CDIS_CHECKED) != 0;
    draw_frame(cpu, t, hdc, rc, state, pressed);
    if sep_dd {
        let pressed = b.dropdown_pressed || pressed;
        draw_frame(cpu, t, hdc, arrow, state & !CDIS_INDETERMINATE, pressed);
        draw_dd_arrow(cpu, hdc, arrow, state, pressed);
    }
    let old_mode = gfx::set_bk_mode(cpu, hdc, TRANSPARENT);
    if shows_text && let Some(text) = &text {
        draw_string(cpu, t, hdc, rc_text, text, state);
    }
    gfx::set_bk_mode(cpu, hdc, old_mode);
    draw_image(cpu, t, b, hdc, rc_bitmap.left, rc_bitmap.top, state);
    if has_dd && !sep_dd {
        draw_dd_arrow(cpu, hdc, arrow, state, pressed);
    }
}

/// The 8 by 8 brush of alternate pixels a checked button is filled
/// with.
fn pattern_brush(cpu: &mut Cpu) -> u32 {
    let rows: Vec<u8> = (0..8)
        .flat_map(|i| [if i % 2 == 0 { 0x55 } else { 0xAA }, 0])
        .collect();
    let heap = process_heap(cpu);
    let at = heap_alloc(cpu, heap, 16);
    if at == 0 {
        return 0;
    }
    cpu.bus.guest_write_bytes(at, &rows);
    let hbm = gdi(cpu, "CreateBitmap", &[8, 8, 1, 1, at]);
    heap_free(cpu, heap, at);
    let brush = gdi(cpu, "CreatePatternBrush", &[hbm]);
    gdi(cpu, "DeleteObject", &[hbm]);
    brush
}

/// TOOLBAR_Refresh, without custom draw: the buttons in `paint`.
fn refresh(cpu: &mut Cpu, hwnd: u32, hdc: u32, paint: Option<Rect>) {
    let Some(mut t) = get(cpu, hwnd) else {
        return;
    };
    if !t.redraw {
        return;
    }
    // The program may change an image list of its own.
    if def_list(&t, 0) != t.him_int {
        let lists: Vec<u32> = t.def.iter().map(|e| e.himl).collect();
        t.num_bitmaps = lists.iter().map(|&h| imagelist::count(cpu, h).max(0)).sum();
    }
    check_icon_size(cpu, &mut t);
    put(cpu, hwnd, t.clone());
    let paint = paint.unwrap_or(t.bound);
    let client = super::client(cpu, hwnd);
    let pattern = pattern_brush(cpu);
    for b in &t.buttons {
        if b.state & TBSTATE_HIDDEN != 0 {
            continue;
        }
        let clipped = client.intersect(&b.rect) != b.rect;
        if t.ex_style & TBSTYLE_EX_HIDECLIPPEDBUTTONS != 0 && clipped {
            continue;
        }
        if paint.intersect(&b.rect).is_empty() {
            continue;
        }
        draw_button(cpu, &t, b, hdc, pattern);
    }
    if pattern != 0 {
        gdi(cpu, "DeleteObject", &[pattern]);
    }
    if t.mark_button >= 0 && (t.mark_button as usize) < t.buttons.len() {
        let r = t.buttons[t.mark_button as usize].rect;
        let x = if t.mark_flags & TBIMHT_AFTER != 0 {
            r.right
        } else {
            r.left - INSERTMARK_WIDTH
        };
        let mark = Rect::new(x, r.top, x + INSERTMARK_WIDTH, r.bottom);
        gfx::fill_rgb(cpu, hdc, mark, t.mark_color);
    }
}

/// The steps of WM_PAINT and WM_PRINTCLIENT.
fn paint(cpu: &mut Cpu, hwnd: u32, t: &Toolbar, s: &mut Script, hdc: u32) {
    ask_images(cpu, hwnd, t, s);
    if hdc != 0 {
        s.then(T_DRAW_DC, &[hdc]);
        return;
    }
    let ps = s.alloc(64);
    let begin = user32::address_of("BeginPaint");
    let end = user32::address_of("EndPaint");
    s.call(begin, &[Arg::V(hwnd), Arg::S(ps)])
        .then(DRAW, &[ps])
        .call(end, &[Arg::V(hwnd), Arg::S(ps)]);
}

/// WM_NCCREATE: TOOLBAR_NCCreate.
fn nc_create(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, lp) = (args[0], args[3]);
    let cs = read_words(cpu, lp, 12);
    if cpu.bus.guest_faulted() {
        return Some(0);
    }
    let (parent, style) = (cs[3], cs[8]);
    let t = Toolbar {
        struct_size: TBBUTTON_SIZE,
        rows: 1,
        button_w: 23,
        button_h: 22,
        bitmap_h: 16,
        bitmap_w: 16,
        max_text_rows: 1,
        cx_min: -1,
        cx_max: -1,
        button_down: -1,
        button_drag: -1,
        old_hit: -1,
        hot_item: -1,
        notify: parent,
        dt_flags: if style & TBSTYLE_LIST != 0 {
            DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS
        } else {
            DT_CENTER | DT_END_ELLIPSIS
        },
        redraw: true,
        clr_highlight: gfx::CLR_DEFAULT,
        clr_shadow: gfx::CLR_DEFAULT,
        pad_cx: DEFPAD_CX,
        pad_cy: DEFPAD_CY,
        list_gap: DEFLISTGAP,
        top_margin: if style & TBSTYLE_FLAT != 0 {
            0
        } else {
            TOP_BORDER
        },
        style,
        mark_button: -1,
        ..Toolbar::default()
    };
    put(cpu, hwnd, t);
    let user = user32::user(cpu);
    // The instance of a toolbar CreateToolbarEx made is its parent's.
    let parent_inst = user.window(parent).map_or(0, |w| w.instance);
    if let Some(w) = user.window_mut(hwnd) {
        if w.instance == 0 {
            w.instance = parent_inst;
        }
        // As Windows does: a toolbar is CCS_TOP unless it says where
        // it goes, and a flat one at the top is transparent.
        if style & (CCS_TOP | CCS_NOMOVEY) == 0 {
            let mut add = CCS_TOP;
            if style & TBSTYLE_FLAT != 0 {
                add |= TBSTYLE_TRANSPARENT;
            }
            w.style = style | add;
        }
    }
    defwnd::def_window_proc(cpu, args)
}

/// WM_CREATE: TOOLBAR_Create.
fn create(cpu: &mut Cpu, hwnd: u32, t: &mut Toolbar, s: &mut Script) -> u32 {
    t.style = super::style(cpu, hwnd);
    t.client = super::client(cpu, hwnd);
    t.tooltip = 0;
    t.default_font = gfx::default_font(cpu);
    t.font = t.default_font;
    if t.notify != 0 {
        s.send(t.notify, WM_NOTIFYFORMAT, Arg::V(hwnd), Arg::V(NF_QUERY));
        s.then(T_FORMAT, &[0]);
    }
    0
}

/// The toolbar's window procedure.
pub fn proc_(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, msg, wp, lp) = (args[0], args[1], args[2], args[3]);
    if msg == WM_NCCREATE {
        return nc_create(cpu, args);
    }
    let Some(mut t) = get(cpu, hwnd) else {
        return defwnd::def_window_proc(cpu, args);
    };
    let mut s = new_script(hwnd);
    let result = message(cpu, hwnd, &mut t, &mut s, msg, wp, lp);
    if msg == WM_DESTROY {
        state(cpu).toolbar.bars.remove(&hwnd);
    } else {
        put(cpu, hwnd, t);
    }
    match result {
        Some(r) => {
            s.result = r;
            script::run(cpu, TOOLBAR, s)
        }
        None => defwnd::def_window_proc(cpu, args),
    }
}

/// A message: what it returns, or None for DefWindowProc's answer.
fn message(
    cpu: &mut Cpu,
    hwnd: u32,
    t: &mut Toolbar,
    s: &mut Script,
    msg: u32,
    wp: u32,
    lp: u32,
) -> Option<u32> {
    let w = wp as i32;
    let r = match msg {
        TB_ADDBITMAP => {
            if lp == 0 {
                return Some(u32::MAX);
            }
            let ab = read_words(cpu, lp, 2);
            if cpu.bus.guest_faulted() {
                return Some(u32::MAX);
            }
            add_bitmap(cpu, hwnd, t, w, ab[0], ab[1]) as u32
        }
        TB_ADDBUTTONSA | TB_ADDBUTTONSW => {
            insert_buttons(cpu, hwnd, t, s, -1, wp, lp, msg == TB_ADDBUTTONSW)
        }
        TB_ADDSTRINGA | TB_ADDSTRINGW => {
            add_string(cpu, hwnd, t, wp, lp, msg == TB_ADDSTRINGW) as u32
        }
        TB_AUTOSIZE => {
            auto_size(cpu, hwnd, t, s, false);
            0
        }
        TB_BUTTONCOUNT => t.buttons.len() as u32,
        TB_BUTTONSTRUCTSIZE => {
            t.struct_size = wp;
            0
        }
        TB_CHANGEBITMAP => {
            let i = button_index(t, w, false);
            if i < 0 {
                return Some(0);
            }
            let b = &mut t.buttons[i as usize];
            b.bitmap = lp as u16 as i32;
            let r = b.rect;
            invalidate_rect(cpu, hwnd, r, true);
            1
        }
        TB_CHECKBUTTON => {
            let i = button_index(t, w, false);
            if i < 0 {
                return Some(0);
            }
            let check = lp & 0xFFFF != 0;
            let was = t.buttons[i as usize].state & TBSTATE_CHECKED != 0;
            let mut old = -1;
            if !check {
                t.buttons[i as usize].state &= !TBSTATE_CHECKED;
            } else {
                if t.buttons[i as usize].style & BTNS_GROUP != 0 {
                    old = checked_group_index(t, i);
                    if old == i {
                        return Some(0);
                    }
                    if old != -1 {
                        t.buttons[old as usize].state &= !TBSTATE_CHECKED;
                    }
                }
                t.buttons[i as usize].state |= TBSTATE_CHECKED;
            }
            if was != check {
                if old != -1 {
                    let r = t.buttons[old as usize].rect;
                    invalidate_rect(cpu, hwnd, r, true);
                }
                let r = t.buttons[i as usize].rect;
                invalidate_rect(cpu, hwnd, r, true);
            }
            1
        }
        TB_COMMANDTOINDEX => button_index(t, w, false) as u32,
        // No customization dialog.
        TB_CUSTOMIZE => 0,
        TB_DELETEBUTTON => delete_button(cpu, hwnd, t, s, w),
        TB_ENABLEBUTTON => {
            let i = button_index(t, w, false);
            if i < 0 {
                return Some(0);
            }
            let b = &mut t.buttons[i as usize];
            let was = b.state & TBSTATE_ENABLED;
            if lp & 0xFFFF == 0 {
                b.state &= !(TBSTATE_ENABLED | TBSTATE_PRESSED);
            } else {
                b.state |= TBSTATE_ENABLED;
            }
            if was != b.state & TBSTATE_ENABLED {
                let r = b.rect;
                invalidate_rect(cpu, hwnd, r, true);
            }
            1
        }
        TB_GETANCHORHIGHLIGHT => t.anchor as u32,
        TB_GETBITMAP => {
            let i = button_index(t, w, false);
            if i < 0 {
                return Some(u32::MAX);
            }
            t.buttons[i as usize].bitmap as u32
        }
        TB_GETBITMAPFLAGS => 0,
        TB_GETBUTTON => {
            if lp == 0 || w < 0 || w as usize >= t.buttons.len() {
                return Some(0);
            }
            let b = &t.buttons[w as usize];
            let words = [
                b.bitmap as u32,
                b.command as u32,
                b.state | (b.style << 8),
                b.data,
                b.string,
            ];
            write_words(cpu, lp, &words);
            1
        }
        TB_GETBUTTONINFOA | TB_GETBUTTONINFOW => {
            get_button_info(cpu, t, wp, lp, msg == TB_GETBUTTONINFOW)
        }
        TB_GETBUTTONSIZE => (t.button_w as u32 & 0xFFFF) | ((t.button_h as u32 & 0xFFFF) << 16),
        TB_GETBUTTONTEXTA | TB_GETBUTTONTEXTW => {
            get_button_text(cpu, t, wp, lp, msg == TB_GETBUTTONTEXTW)
        }
        TB_GETDISABLEDIMAGELIST => list_of(&t.dis_lists, w),
        TB_GETEXTENDEDSTYLE => t.ex_style,
        TB_GETHOTIMAGELIST => list_of(&t.hot_lists, w),
        TB_GETHOTITEM => {
            if t.style & TBSTYLE_FLAT == 0 || t.hot_item < 0 {
                u32::MAX
            } else {
                t.hot_item as u32
            }
        }
        TB_GETIMAGELIST => def_list(t, w),
        TB_GETINSERTMARK => {
            if lp != 0 {
                write_words(cpu, lp, &[t.mark_button as u32, t.mark_flags]);
            }
            0
        }
        TB_GETINSERTMARKCOLOR => t.mark_color,
        TB_GETITEMRECT | TB_GETRECT => {
            let i = if msg == TB_GETRECT {
                button_index(t, w, false)
            } else {
                w
            };
            if i < 0 || i as usize >= t.buttons.len() || lp == 0 {
                return Some(0);
            }
            let b = &t.buttons[i as usize];
            if msg == TB_GETITEMRECT && b.state & TBSTATE_HIDDEN != 0 {
                return Some(0);
            }
            b.rect.write(cpu, lp);
            1
        }
        TB_GETMAXSIZE => {
            if lp == 0 {
                return Some(0);
            }
            let size = [t.bound.width() as u32, t.bound.height() as u32];
            write_words(cpu, lp, &size);
            1
        }
        TB_GETPADDING => (t.pad_cx as u32 & 0xFFFF) | ((t.pad_cy as u32 & 0xFFFF) << 16),
        TB_GETROWS => t.rows as u32,
        TB_GETSTATE => {
            let i = button_index(t, w, false);
            if i < 0 {
                return Some(u32::MAX);
            }
            t.buttons[i as usize].state
        }
        TB_GETSTRINGA | TB_GETSTRINGW => get_string(cpu, t, wp, lp, msg == TB_GETSTRINGW),
        TB_GETSTYLE => t.style,
        TB_GETTEXTROWS => t.max_text_rows as u32,
        TB_GETTOOLTIPS => {
            if t.style & TBSTYLE_TOOLTIPS != 0 && t.tooltip == 0 && !t.tip_creating {
                create_tooltip(t, hwnd, s, true);
                return Some(0);
            }
            t.tooltip
        }
        CCM_GETUNICODEFORMAT => t.unicode as u32,
        TB_HIDEBUTTON => {
            let i = button_index(t, w, false);
            if i < 0 {
                return Some(0);
            }
            let b = &mut t.buttons[i as usize];
            let old = b.state;
            if lp & 0xFFFF != 0 {
                b.state |= TBSTATE_HIDDEN;
            } else {
                b.state &= !TBSTATE_HIDDEN;
            }
            if old != b.state {
                layout(cpu, hwnd, t);
                super::invalidate(cpu, hwnd, true);
            }
            1
        }
        TB_HITTEST => {
            if lp == 0 {
                return Some(u32::MAX);
            }
            let p = read_words(cpu, lp, 2);
            hit_test(t, p[0] as i32, p[1] as i32) as u32
        }
        TB_INDETERMINATE | TB_MARKBUTTON | TB_PRESSBUTTON => {
            let i = button_index(t, w, false);
            if i < 0 {
                return Some(0);
            }
            let flag = match msg {
                TB_INDETERMINATE => TBSTATE_INDETERMINATE,
                TB_MARKBUTTON => TBSTATE_MARKED,
                _ => TBSTATE_PRESSED,
            };
            let b = &mut t.buttons[i as usize];
            let old = b.state;
            if lp & 0xFFFF != 0 {
                b.state |= flag;
            } else {
                b.state &= !flag;
            }
            if old != b.state {
                let r = b.rect;
                invalidate_rect(cpu, hwnd, r, true);
            }
            1
        }
        TB_INSERTBUTTONA | TB_INSERTBUTTONW => {
            if lp == 0 {
                return Some(0);
            }
            let index = if w == -1 {
                t.buttons.len() as i32
            } else if w < 0 {
                return Some(0);
            } else {
                w.min(t.buttons.len() as i32)
            };
            insert_buttons(cpu, hwnd, t, s, index, 1, lp, msg == TB_INSERTBUTTONW)
        }
        TB_ISBUTTONCHECKED
        | TB_ISBUTTONENABLED
        | TB_ISBUTTONHIDDEN
        | TB_ISBUTTONHIGHLIGHTED
        | TB_ISBUTTONINDETERMINATE
        | TB_ISBUTTONPRESSED => {
            let i = button_index(t, w, false);
            if i < 0 {
                return Some(u32::MAX);
            }
            let flag = match msg {
                TB_ISBUTTONCHECKED => TBSTATE_CHECKED,
                TB_ISBUTTONENABLED => TBSTATE_ENABLED,
                TB_ISBUTTONHIDDEN => TBSTATE_HIDDEN,
                TB_ISBUTTONHIGHLIGHTED => TBSTATE_MARKED,
                TB_ISBUTTONINDETERMINATE => TBSTATE_INDETERMINATE,
                _ => TBSTATE_PRESSED,
            };
            t.buttons[i as usize].state & flag
        }
        TB_LOADIMAGES => add_bitmap(cpu, hwnd, t, 0, lp, wp) as u32,
        TB_MAPACCELERATORA | TB_MAPACCELERATORW => map_accelerator(cpu, t, wp, lp),
        TB_MOVEBUTTON => move_button(cpu, hwnd, t, s, w, lp as i32),
        TB_REPLACEBITMAP => replace_bitmap(cpu, hwnd, t, lp),
        TB_SAVERESTOREA | TB_SAVERESTOREW => 0,
        TB_SETANCHORHIGHLIGHT => {
            let old = t.anchor;
            t.anchor = wp != 0;
            old as u32
        }
        TB_SETBITMAPSIZE => set_bitmap_size(cpu, hwnd, t, lp),
        TB_SETBUTTONINFOA | TB_SETBUTTONINFOW => {
            set_button_info(cpu, hwnd, t, wp, lp, msg == TB_SETBUTTONINFOW)
        }
        TB_SETBUTTONSIZE => set_button_size(cpu, hwnd, t, lp),
        TB_SETBUTTONWIDTH => {
            let (lo, hi) = (lp as u16 as i16 as i32, (lp >> 16) as u16 as i16 as i32);
            if t.cx_min != lo || t.cx_max != hi {
                t.cx_min = lo;
                t.cx_max = hi;
                calc(cpu, hwnd, t);
                super::invalidate(cpu, hwnd, true);
            }
            1
        }
        TB_SETCMDID => {
            if w < 0 || w as usize >= t.buttons.len() {
                return Some(0);
            }
            t.buttons[w as usize].command = lp as i32;
            1
        }
        TB_SETDISABLEDIMAGELIST | TB_SETHOTIMAGELIST => {
            let id = if t.version >= 5 { w } else { 0 };
            let lists = if msg == TB_SETHOTIMAGELIST {
                &mut t.hot_lists
            } else {
                &mut t.dis_lists
            };
            insert_list(lists, lp, id)
        }
        TB_SETDRAWTEXTFLAGS => {
            let old = t.dt_flags;
            t.dt_flags = (t.dt_flags & !wp) | (lp & wp);
            old
        }
        TB_SETEXTENDEDSTYLE => {
            let old = t.ex_style;
            t.ex_style = lp;
            if (old ^ lp) & TBSTYLE_EX_MIXEDBUTTONS != 0 {
                calc(cpu, hwnd, t);
            } else {
                layout(cpu, hwnd, t);
            }
            auto_size(cpu, hwnd, t, s, false);
            super::invalidate(cpu, hwnd, true);
            old
        }
        TB_SETHOTITEM | TB_SETHOTITEM2 => {
            let old = t.hot_item;
            let n = t.buttons.len() as i32;
            let mut hot = w;
            if msg == TB_SETHOTITEM {
                if hot >= n {
                    return Some(t.hot_item as u32);
                }
                if hot < 0 {
                    hot = -1;
                }
                set_hot_item_ex(cpu, hwnd, t, s, hot, HICF_OTHER);
            } else {
                if hot < 0 || hot > n {
                    hot = -1;
                }
                set_hot_item_ex(cpu, hwnd, t, s, hot, lp);
            }
            if old < 0 { u32::MAX } else { old as u32 }
        }
        TB_SETIMAGELIST => set_image_list(cpu, hwnd, t, wp, lp),
        // As Wine and Windows do, TB_SETINDENT changes nothing that is
        // laid out already.
        TB_SETINDENT => {
            t.indent = w;
            1
        }
        TB_SETINSERTMARK => {
            let m = read_words(cpu, lp, 2);
            if cpu.bus.guest_faulted() || m[1] & !TBIMHT_AFTER != 0 {
                return Some(0);
            }
            let b = m[0] as i32;
            if b == -1 || (b >= 0 && (b as usize) < t.buttons.len()) {
                t.mark_button = b;
                t.mark_flags = m[1];
                super::invalidate(cpu, hwnd, true);
            }
            0
        }
        TB_SETINSERTMARKCOLOR => {
            t.mark_color = lp;
            super::invalidate(cpu, hwnd, true);
            0
        }
        TB_SETMAXTEXTROWS => {
            t.max_text_rows = w;
            calc(cpu, hwnd, t);
            1
        }
        TB_SETPADDING => {
            let old = (t.pad_cx as u32 & 0xFFFF) | ((t.pad_cy as u32 & 0xFFFF) << 16);
            t.pad_cx = (lp & 0xFFFF) as i32;
            t.pad_cy = (lp >> 16) as i32;
            old
        }
        TB_SETPARENT => std::mem::replace(&mut t.notify, wp),
        TB_SETROWS => set_rows(cpu, hwnd, t, s, wp, lp),
        TB_SETSTATE => {
            let i = button_index(t, w, false);
            if i < 0 {
                return Some(0);
            }
            let new = lp & 0xFFFF;
            let b = &mut t.buttons[i as usize];
            if (b.state ^ new) & TBSTATE_HIDDEN != 0 {
                b.state = new;
                calc(cpu, hwnd, t);
                super::invalidate(cpu, hwnd, true);
                return Some(1);
            }
            if b.state != new {
                b.state = new;
                let r = b.rect;
                invalidate_rect(cpu, hwnd, r, true);
            }
            1
        }
        TB_SETSTYLE => {
            s.user("SetWindowLongA", &[hwnd, GWL_STYLE, lp]);
            1
        }
        TB_SETTOOLTIPS => {
            t.tooltip = wp;
            0
        }
        CCM_SETUNICODEFORMAT => {
            let old = t.unicode;
            t.unicode = wp != 0;
            old as u32
        }
        TB_SETLISTGAP => {
            t.list_gap = w;
            super::invalidate(cpu, hwnd, true);
            0
        }
        TB_GETIMAGELISTCOUNT => t.def.len().max(t.hot_lists.len()).max(t.dis_lists.len()) as u32,
        TB_GETIDEALSIZE => {
            if lp == 0 {
                return Some(0);
            }
            match wp {
                0 => cpu.bus.guest_write_32(lp, t.bound.width() as u32),
                1 => cpu.bus.guest_write_32(lp + 4, t.bound.height() as u32),
                _ => return Some(0),
            }
            1
        }
        CCM_GETCOLORSCHEME => {
            if lp != 0 {
                let (hi, sh) = (highlight(cpu, t), shadow(cpu, t));
                write_words(cpu, lp + 4, &[hi, sh]);
            }
            1
        }
        CCM_SETCOLORSCHEME => {
            let c = read_words(cpu, lp, 3);
            if !cpu.bus.guest_faulted() {
                t.clr_highlight = c[1];
                t.clr_shadow = c[2];
                super::invalidate(cpu, hwnd, true);
            }
            0
        }
        CCM_GETVERSION => t.version as u32,
        CCM_SETVERSION => {
            let old = t.version;
            t.version = w;
            if t.version >= 5 {
                t.unicode = true;
            }
            old as u32
        }
        WM_CREATE => create(cpu, hwnd, t, s),
        WM_DESTROY => {
            if t.tooltip != 0 {
                s.user("DestroyWindow", &[t.tooltip]);
            }
            if t.tip_text != 0 {
                let heap = process_heap(cpu);
                heap_free(cpu, heap, t.tip_text);
            }
            if t.him_int != 0 {
                imagelist::destroy(cpu, t.him_int);
            }
            if t.leave_timer {
                user32::message::kill_timer(cpu, &[hwnd, ID_LEAVE]);
            }
            0
        }
        WM_ERASEBKGND => erase_background(cpu, hwnd, t, s, wp, lp),
        WM_GETFONT => t.font,
        WM_KEYDOWN => {
            notify(cpu, s, hwnd, t, NM_KEYDOWN, &[wp, lp >> 16]);
            s.then(T_KEY, &[wp, lp]);
            0
        }
        WM_LBUTTONDBLCLK => {
            let (x, y) = user32::point_of(lp);
            if hit_test(t, x, y) >= 0 {
                l_button_down(cpu, hwnd, t, s, wp, lp);
            }
            0
        }
        WM_LBUTTONDOWN => {
            l_button_down(cpu, hwnd, t, s, wp, lp);
            0
        }
        WM_LBUTTONUP => {
            l_button_up(cpu, hwnd, t, s, lp);
            0
        }
        WM_RBUTTONUP => {
            let (x, y) = user32::point_of(lp);
            let hit = hit_test(t, x, y);
            let hit = if hit < 0 { -1 } else { hit };
            let more = nmmouse(cpu, hwnd, t, hit, x, y);
            notify(cpu, s, hwnd, t, NM_RCLICK, &more);
            s.then(T_DEFAULT, &[msg, wp, lp]);
            0
        }
        WM_RBUTTONDBLCLK => {
            notify(cpu, s, hwnd, t, NM_RDBLCLK, &[]);
            s.then(T_DEFAULT, &[msg, wp, lp]);
            0
        }
        WM_MOUSEMOVE => {
            if t.style & TBSTYLE_TOOLTIPS != 0 && t.tooltip == 0 && !t.tip_creating {
                create_tooltip(t, hwnd, s, false);
                s.then(T_MOVE, &[wp, lp]);
                return Some(0);
            }
            mouse_move(cpu, hwnd, t, s, lp);
            0
        }
        WM_TIMER if wp == ID_LEAVE => {
            let (cx, cy) = user32::user(cpu).input.cursor;
            let over = user32::window::window_from_point(cpu, &[cx as u32, cy as u32]);
            if over != Some(hwnd) {
                user32::message::kill_timer(cpu, &[hwnd, ID_LEAVE]);
                t.leave_timer = false;
                mouse_leave(cpu, hwnd, t, s);
            }
            0
        }
        WM_CAPTURECHANGED => {
            t.captured = false;
            let n = t.buttons.len() as i32;
            if t.button_down >= 0 && t.button_down < n {
                let b = &mut t.buttons[t.button_down as usize];
                b.state &= !TBSTATE_PRESSED;
                let (enabled, r) = (b.state & TBSTATE_ENABLED != 0, b.rect);
                t.old_hit = -1;
                if enabled {
                    invalidate_rect(cpu, hwnd, r, true);
                }
            }
            0
        }
        WM_NCCALCSIZE => {
            if super::style(cpu, hwnd) & CCS_NODIVIDER == 0 && lp != 0 {
                let top = cpu.bus.guest_read_32(lp + 4) as i32;
                cpu.bus
                    .guest_write_32(lp + 4, (top + metric(SM_CYEDGE)) as u32);
            }
            return None;
        }
        WM_NCPAINT => nc_paint(cpu, hwnd, wp, lp),
        WM_NOTIFY => {
            let code = cpu.bus.guest_read_32(lp + 8);
            if code == tooltips::TTN_GETDISPINFOA || code == tooltips::TTN_GETDISPINFOW {
                tt_get_disp_info(cpu, t, s, lp)
            } else {
                0
            }
        }
        WM_NOTIFYFORMAT => match lp {
            NF_QUERY => NFR_UNICODE,
            NF_REQUERY => {
                s.send(t.notify, WM_NOTIFYFORMAT, Arg::V(hwnd), Arg::V(NF_QUERY));
                s.then(T_FORMAT, &[1]);
                0
            }
            _ => 0,
        },
        WM_PAINT | WM_PRINTCLIENT => {
            paint(cpu, hwnd, t, s, wp);
            0
        }
        WM_SETFOCUS => {
            if !t.buttons.is_empty() {
                set_hot_item_ex(cpu, hwnd, t, s, 0, HICF_OTHER);
            }
            0
        }
        WM_SETFONT => {
            t.font = if wp == 0 { t.default_font } else { wp };
            calc(cpu, hwnd, t);
            if lp & 0xFFFF != 0 {
                super::invalidate(cpu, hwnd, true);
            }
            1
        }
        WM_SETREDRAW => {
            let old = t.redraw;
            t.redraw = wp != 0;
            if wp != 0 {
                super::invalidate(cpu, hwnd, true);
            }
            old as u32
        }
        WM_SIZE => {
            let client = super::client(cpu, hwnd);
            if t.ex_style & TBSTYLE_EX_HIDECLIPPEDBUTTONS != 0 {
                // The buttons the change uncovers or cuts are drawn
                // again.
                let (old, new) = (t.client, client);
                let (min_x, max_x) = (old.right.min(new.right), old.right.max(new.right));
                let (min_y, max_y) = (old.bottom.min(new.bottom), old.bottom.max(new.bottom));
                let dw = Rect::new(min_x, 0, max_x, min_y);
                let dh = Rect::new(0, min_y, max_x, max_y);
                for b in &t.buttons {
                    if !dw.intersect(&b.rect).is_empty() || !dh.intersect(&b.rect).is_empty() {
                        invalidate_rect(cpu, hwnd, b.rect, true);
                    }
                }
            }
            t.client = client;
            auto_size(cpu, hwnd, t, s, true);
            0
        }
        WM_STYLECHANGED => {
            if wp == GWL_STYLE {
                let new = cpu.bus.guest_read_32(lp + 4);
                let old = t.style;
                t.dt_flags = if new & TBSTYLE_LIST != 0 {
                    DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS
                } else {
                    DT_CENTER | DT_END_ELLIPSIS
                };
                t.style = new;
                if (old ^ new) & (TBSTYLE_WRAPABLE | CCS_VERT) != 0 {
                    layout(cpu, hwnd, t);
                }
                if (old ^ new) & COMMON_STYLES != 0 {
                    auto_size(cpu, hwnd, t, s, false);
                    super::invalidate(cpu, hwnd, true);
                }
            }
            0
        }
        WM_CHARTOITEM | WM_COMMAND | WM_DRAWITEM | WM_MEASUREITEM | WM_VKEYTOITEM => {
            s.send(t.notify, msg, Arg::V(wp), Arg::V(lp));
            s.ret_last();
            0
        }
        _ => return None,
    };
    Some(r)
}

/// TB_MAPACCELERATOR: the command of the button whose text has `wp`
/// after an ampersand.
fn map_accelerator(cpu: &mut Cpu, t: &Toolbar, wp: u32, lp: u32) -> u32 {
    let key = (wp as u8).to_ascii_lowercase();
    for b in &t.buttons {
        if b.style & BTNS_NOPREFIX != 0 || b.state & TBSTATE_HIDDEN != 0 {
            continue;
        }
        let Some(text) = text_of(t, b) else {
            continue;
        };
        let mut i = 0;
        while i + 1 < text.len() {
            if text[i] == b'&' && text[i + 1] == b'&' {
                i += 2;
                continue;
            }
            if text[i] == b'&' && text[i + 1].to_ascii_lowercase() == key {
                if lp != 0 {
                    cpu.bus.guest_write_32(lp, b.command as u32);
                }
                return 1;
            }
            i += 1;
        }
    }
    0
}

/// TOOLBAR_ReplaceBitmap.
fn replace_bitmap(cpu: &mut Cpu, hwnd: u32, t: &mut Toolbar, lp: u32) -> u32 {
    if lp == 0 {
        return 0;
    }
    let r = read_words(cpu, lp, 5);
    if cpu.bus.guest_faulted() {
        return 0;
    }
    let (inst_old, id_old, inst_new, id_new, buttons) = (r[0], r[1], r[2], r[3], r[4] as i32);
    if inst_old == HINST_COMMCTRL {
        return 0;
    }
    let mut pos = 0;
    let mut old_buttons = 0;
    for b in t.bitmaps.iter_mut() {
        if b.inst == inst_old && b.id == id_old {
            old_buttons = b.buttons;
            b.buttons = buttons;
            b.inst = inst_new;
            b.id = id_new;
            break;
        }
        pos += b.buttons;
    }
    if old_buttons == 0 {
        return 0;
    }
    let info = BitmapInfo {
        buttons,
        inst: inst_new,
        id: id_new,
    };
    let himl = def_list(t, 0);
    let before = imagelist::count(cpu, himl);
    let mut i = (pos + old_buttons - 1).min(before - 1);
    while i >= pos {
        imagelist::il_remove(cpu, &[himl, i as u32]);
        i -= 1;
    }
    t.num_bitmaps -= before - imagelist::count(cpu, himl);
    // The new images go at the end of the list.
    add_bitmap_to_list(cpu, t, himl, &info);
    super::invalidate(cpu, hwnd, true);
    1
}

/// The continuations of the toolbar's scripts.
pub fn then(cpu: &mut Cpu, id: u16, c: &mut Ctx) -> Vec<Op> {
    let hwnd = c.hwnd;
    match id {
        DRAW => {
            let ps = c.word(0);
            let r = Rect::read(cpu, c.scratch + ps + 8);
            refresh(cpu, hwnd, c.last, Some(r));
            Vec::new()
        }
        T_DRAW_DC => {
            refresh(cpu, hwnd, c.word(0), None);
            Vec::new()
        }
        T_FORMAT => {
            let unicode = c.last == NFR_UNICODE;
            if c.word(0) != 0 {
                // NF_REQUERY's answer.
                c.result = if unicode { NFR_UNICODE } else { 1 };
            }
            if let Some(mut t) = get(cpu, hwnd) {
                t.unicode = unicode;
                put(cpu, hwnd, t);
            }
            Vec::new()
        }
        T_HOT => {
            if c.last == 0 {
                change_hot(cpu, hwnd, c.word(0) as i32);
            }
            Vec::new()
        }
        T_TIP_MADE => tooltip_made(cpu, c),
        T_MOVE => {
            let Some(mut t) = get(cpu, hwnd) else {
                return Vec::new();
            };
            let mut s = cont_script(hwnd);
            mouse_move(cpu, hwnd, &mut t, &mut s, c.word(1));
            put(cpu, hwnd, t);
            s.ops
        }
        T_DROPDOWN => dropped_down(cpu, c),
        T_DEFAULT => {
            if c.last == 0 {
                let args = [hwnd, c.word(0), c.word(1), c.word(2)];
                c.result = defwnd::def_window_proc(cpu, &args).unwrap_or(0);
            } else {
                c.result = 0;
            }
            Vec::new()
        }
        T_CLICKED => {
            c.result = 0;
            if !user32::user(cpu).is_window(hwnd) {
                return Vec::new();
            }
            let Some(t) = get(cpu, hwnd) else {
                return Vec::new();
            };
            let (hit, x, y, lp) = (c.word(0) as i32, c.word(1), c.word(2), c.word(3));
            let mut s = cont_script(hwnd);
            click_end(cpu, hwnd, &t, &mut s, hit, (x as i32, y as i32), lp);
            s.ops
        }
        T_TIP_ASK => tip_ask(cpu, c),
        T_TIP_GOT => tip_got(cpu, c),
        T_DISP_BACK => disp_back(cpu, c),
        T_DISP_IMAGE => image_got(cpu, c),
        T_ERASED => erased(cpu, c),
        T_KEY => key_down(cpu, c),
        T_EX_MADE => toolbar_ex_made(c),
        _ => Vec::new(),
    }
}

/// The toolbar CreateToolbarEx made is there (T_EX_MADE): its sizes,
/// bitmap and buttons.
fn toolbar_ex_made(c: &mut Ctx) -> Vec<Op> {
    let tb = c.last;
    c.result = tb;
    if tb == 0 {
        return Vec::new();
    }
    let [
        bitmaps,
        inst,
        bmid,
        buttons,
        n,
        dx_button,
        dy_button,
        dx_bitmap,
        dy_bitmap,
        size,
    ] = std::array::from_fn(|i| c.word(i));
    let mut s = cont_script(0);
    let v = Arg::V;
    s.send(tb, TB_BUTTONSTRUCTSIZE, v(size), v(0));
    let (mut bw, mut bh) = (dx_bitmap as i32, dy_bitmap as i32);
    if bw < 0 {
        bw = 16;
    }
    if bh < 0 {
        bh = 16;
    }
    if bw == 0 || bh == 0 {
        bw = 16;
        bh = 16;
    }
    let pack = |w: i32, h: i32| (w as u32 & 0xFFFF) | ((h as u32 & 0xFFFF) << 16);
    s.send(tb, TB_SETBITMAPSIZE, v(0), v(pack(bw, bh)));
    let (mut dx, mut dy) = (dx_button as i32, dy_button as i32);
    if dx < 0 {
        dx = bw;
    }
    if dy < 0 {
        dy = bh;
    }
    // As Windows does, the button size goes to TB_SETBITMAPSIZE.
    if dx != 0 && dy != 0 {
        s.send(tb, TB_SETBITMAPSIZE, v(0), v(pack(dx, dy)));
    }
    if (bitmaps as i32) > 0 || inst == HINST_COMMCTRL {
        // The continuations' scratch space: no TBADDBITMAP was there.
        s.ops.push(Op::Write {
            at: O_DISP_A,
            words: vec![inst, bmid],
        });
        s.send(tb, TB_ADDBITMAP, v(bitmaps), Arg::S(O_DISP_A));
    }
    if (n as i32) > 0 {
        s.send(tb, TB_ADDBUTTONSA, v(n), v(buttons));
    }
    s.ops
}

/// CreateToolbarEx(hwnd, style, id, bitmaps, inst, bitmap id, buttons,
/// count, button cx, cy, bitmap cx, cy, struct size).
pub fn create_toolbar_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut s = new_script(0);
    let class = s.string(b"ToolbarWindow32");
    let f = user32::address_of("CreateWindowExA");
    let v = Arg::V;
    let call = [
        v(0),
        Arg::S(class),
        v(0),
        v(args[1] | WS_CHILD),
        v(0),
        v(0),
        v(100),
        v(30),
        v(args[0]),
        v(args[2]),
        v(super::DLL.base),
        v(0),
    ];
    s.call(f, &call);
    s.then(T_EX_MADE, &args[3..13]);
    script::run(cpu, TOOLBAR, s)
}

/// CreateToolbar(hwnd, style, id, bitmaps, inst, bitmap id, buttons,
/// count): CreateToolbarEx with CCS_NODIVIDER and the default sizes.
pub fn create_toolbar(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let a = [
        args[0],
        args[1] | CCS_NODIVIDER,
        args[2],
        args[3],
        args[4],
        args[5],
        args[6],
        args[7],
        0,
        0,
        0,
        0,
        TBBUTTON_SIZE,
    ];
    create_toolbar_ex(cpu, &a)
}

/// CreateMappedBitmap(inst, id, flags, color map, count).
pub fn create_mapped_bitmap(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (inst, id, map_at, n) = (args[0], args[1], args[3], args[4]);
    let map = if map_at != 0 {
        let words = read_words(cpu, map_at, 2 * n.min(256) as usize);
        if cpu.bus.guest_faulted() {
            return Some(0);
        }
        words.chunks(2).map(|c| (c[0], c[1])).collect()
    } else {
        default_map(cpu)
    };
    Some(create_mapped(cpu, inst, id, &map))
}
