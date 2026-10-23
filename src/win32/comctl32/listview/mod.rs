//! The list view (SysListView32): items with a text, an image, a state
//! and subitems, shown as large icons, small icons, a list or a report
//! with a header control (header.rs) for its columns. The control keeps
//! the selection and the focus, sorts and searches its items, lets the
//! user edit labels in an edit control, and tells its parent about
//! every change with WM_NOTIFY (LVN_ITEMCHANGING, LVN_ITEMCHANGED,
//! LVN_GETDISPINFO for callback texts and images, NM_CUSTOMDRAW while
//! it paints). With LVS_OWNERDATA the program keeps the items and the
//! control only their count and the selection.
//!
//! The control is COMCTL32 4.72's: the messages up to LVM_GETTOOLTIPS
//! and the extended styles up to LVS_EX_BORDERSELECT; LVM_SORTITEMSEX
//! and the views of COMCTL32 6 fail.
//!
//! Notifications go out in the format the parent asks for with
//! WM_NOTIFYFORMAT, with Unicode text and the W codes when it answers
//! NFR_UNICODE.
//!
//! The control is split in modules: the items and their states
//! (items.rs), where they are and the scroll bars (layout.rs), the
//! columns and the header (columns.rs), painting (paint.rs), and the
//! mouse, the keyboard and label editing (input.rs).
//!
//! Ported from Wine's listview.c:
//!
//! Copyright 1998, 1999 Eric Kohl
//! Copyright 1999 Luc Tourangeau
//! Copyright 2000 Jason Mawdsley
//! Copyright 2001 CodeWeavers Inc.
//! Copyright 2002 Dimitrie O. Paun
//! Copyright 2009 Nikolay Sivov
//! Copyright 2009 Owen Rudge for CodeWeavers
//!
//! This library is free software; you can redistribute it and/or modify
//! it under the terms of the GNU Lesser General Public License as
//! published by the Free Software Foundation; either version 2.1 of the
//! License, or (at your option) any later version.

mod columns;
mod input;
mod items;
mod layout;
mod paint;

use super::gfx;
use super::header;
use super::script::{self, Arg, Ctx, Op, Script};
use super::{LISTVIEW, id_of, state};
use crate::cpu::Cpu;
use crate::win32::gdi32::Rect;
use crate::win32::user32::message::{
    WM_CHAR, WM_COMMAND, WM_CREATE, WM_DESTROY, WM_ENABLE, WM_ERASEBKGND, WM_GETDLGCODE,
    WM_GETFONT, WM_HSCROLL, WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MEASUREITEM, WM_MOUSEMOVE, WM_NCCREATE, WM_NCDESTROY, WM_NOTIFY,
    WM_NOTIFYFORMAT, WM_PAINT, WM_PRINTCLIENT, WM_RBUTTONDBLCLK, WM_RBUTTONDOWN, WM_RBUTTONUP,
    WM_SETFOCUS, WM_SETFONT, WM_SETREDRAW, WM_SHOWWINDOW, WM_SIZE, WM_STYLECHANGED,
    WM_STYLECHANGING, WM_TIMER, WM_VSCROLL, WM_WINDOWPOSCHANGED,
};
use crate::win32::user32::{self, defwnd, read_words, write_words};
use std::collections::BTreeMap;

const WM_MOUSEWHEEL: u32 = 0x020A;
const WM_MOUSEHOVER: u32 = 0x02A1;
const GWL_STYLE: u32 = (-16i32) as u32;

const LVM_FIRST: u32 = 0x1000;
const LVM_GETBKCOLOR: u32 = LVM_FIRST;
const LVM_SETBKCOLOR: u32 = LVM_FIRST + 1;
const LVM_GETIMAGELIST: u32 = LVM_FIRST + 2;
const LVM_SETIMAGELIST: u32 = LVM_FIRST + 3;
const LVM_GETITEMCOUNT: u32 = LVM_FIRST + 4;
const LVM_GETITEMA: u32 = LVM_FIRST + 5;
const LVM_SETITEMA: u32 = LVM_FIRST + 6;
const LVM_INSERTITEMA: u32 = LVM_FIRST + 7;
const LVM_DELETEITEM: u32 = LVM_FIRST + 8;
const LVM_DELETEALLITEMS: u32 = LVM_FIRST + 9;
const LVM_GETCALLBACKMASK: u32 = LVM_FIRST + 10;
const LVM_SETCALLBACKMASK: u32 = LVM_FIRST + 11;
const LVM_GETNEXTITEM: u32 = LVM_FIRST + 12;
const LVM_FINDITEMA: u32 = LVM_FIRST + 13;
const LVM_GETITEMRECT: u32 = LVM_FIRST + 14;
const LVM_SETITEMPOSITION: u32 = LVM_FIRST + 15;
const LVM_GETITEMPOSITION: u32 = LVM_FIRST + 16;
const LVM_GETSTRINGWIDTHA: u32 = LVM_FIRST + 17;
const LVM_HITTEST: u32 = LVM_FIRST + 18;
const LVM_ENSUREVISIBLE: u32 = LVM_FIRST + 19;
const LVM_SCROLL: u32 = LVM_FIRST + 20;
const LVM_REDRAWITEMS: u32 = LVM_FIRST + 21;
const LVM_ARRANGE: u32 = LVM_FIRST + 22;
const LVM_EDITLABELA: u32 = LVM_FIRST + 23;
const LVM_GETEDITCONTROL: u32 = LVM_FIRST + 24;
const LVM_GETCOLUMNA: u32 = LVM_FIRST + 25;
const LVM_SETCOLUMNA: u32 = LVM_FIRST + 26;
const LVM_INSERTCOLUMNA: u32 = LVM_FIRST + 27;
const LVM_DELETECOLUMN: u32 = LVM_FIRST + 28;
const LVM_GETCOLUMNWIDTH: u32 = LVM_FIRST + 29;
const LVM_SETCOLUMNWIDTH: u32 = LVM_FIRST + 30;
const LVM_GETHEADER: u32 = LVM_FIRST + 31;
const LVM_CREATEDRAGIMAGE: u32 = LVM_FIRST + 33;
const LVM_GETVIEWRECT: u32 = LVM_FIRST + 34;
const LVM_GETTEXTCOLOR: u32 = LVM_FIRST + 35;
const LVM_SETTEXTCOLOR: u32 = LVM_FIRST + 36;
const LVM_GETTEXTBKCOLOR: u32 = LVM_FIRST + 37;
const LVM_SETTEXTBKCOLOR: u32 = LVM_FIRST + 38;
const LVM_GETTOPINDEX: u32 = LVM_FIRST + 39;
const LVM_GETCOUNTPERPAGE: u32 = LVM_FIRST + 40;
const LVM_GETORIGIN: u32 = LVM_FIRST + 41;
const LVM_UPDATE: u32 = LVM_FIRST + 42;
const LVM_SETITEMSTATE: u32 = LVM_FIRST + 43;
const LVM_GETITEMSTATE: u32 = LVM_FIRST + 44;
const LVM_GETITEMTEXTA: u32 = LVM_FIRST + 45;
const LVM_SETITEMTEXTA: u32 = LVM_FIRST + 46;
const LVM_SETITEMCOUNT: u32 = LVM_FIRST + 47;
const LVM_SORTITEMS: u32 = LVM_FIRST + 48;
const LVM_SETITEMPOSITION32: u32 = LVM_FIRST + 49;
const LVM_GETSELECTEDCOUNT: u32 = LVM_FIRST + 50;
const LVM_GETITEMSPACING: u32 = LVM_FIRST + 51;
const LVM_GETISEARCHSTRINGA: u32 = LVM_FIRST + 52;
const LVM_SETICONSPACING: u32 = LVM_FIRST + 53;
const LVM_SETEXTENDEDLISTVIEWSTYLE: u32 = LVM_FIRST + 54;
const LVM_GETEXTENDEDLISTVIEWSTYLE: u32 = LVM_FIRST + 55;
const LVM_GETSUBITEMRECT: u32 = LVM_FIRST + 56;
const LVM_SUBITEMHITTEST: u32 = LVM_FIRST + 57;
const LVM_SETCOLUMNORDERARRAY: u32 = LVM_FIRST + 58;
const LVM_GETCOLUMNORDERARRAY: u32 = LVM_FIRST + 59;
const LVM_SETHOTITEM: u32 = LVM_FIRST + 60;
const LVM_GETHOTITEM: u32 = LVM_FIRST + 61;
const LVM_SETHOTCURSOR: u32 = LVM_FIRST + 62;
const LVM_GETHOTCURSOR: u32 = LVM_FIRST + 63;
const LVM_APPROXIMATEVIEWRECT: u32 = LVM_FIRST + 64;
const LVM_SETWORKAREAS: u32 = LVM_FIRST + 65;
const LVM_GETSELECTIONMARK: u32 = LVM_FIRST + 66;
const LVM_SETSELECTIONMARK: u32 = LVM_FIRST + 67;
const LVM_GETWORKAREAS: u32 = LVM_FIRST + 70;
const LVM_SETHOVERTIME: u32 = LVM_FIRST + 71;
const LVM_GETHOVERTIME: u32 = LVM_FIRST + 72;
const LVM_GETNUMBEROFWORKAREAS: u32 = LVM_FIRST + 73;
const LVM_SETTOOLTIPS: u32 = LVM_FIRST + 74;
const LVM_GETITEMW: u32 = LVM_FIRST + 75;
const LVM_SETITEMW: u32 = LVM_FIRST + 76;
const LVM_INSERTITEMW: u32 = LVM_FIRST + 77;
const LVM_GETTOOLTIPS: u32 = LVM_FIRST + 78;
const LVM_FINDITEMW: u32 = LVM_FIRST + 83;
const LVM_GETSTRINGWIDTHW: u32 = LVM_FIRST + 87;
const LVM_GETCOLUMNW: u32 = LVM_FIRST + 95;
const LVM_SETCOLUMNW: u32 = LVM_FIRST + 96;
const LVM_INSERTCOLUMNW: u32 = LVM_FIRST + 97;
const LVM_GETITEMTEXTW: u32 = LVM_FIRST + 115;
const LVM_SETITEMTEXTW: u32 = LVM_FIRST + 116;
const LVM_GETISEARCHSTRINGW: u32 = LVM_FIRST + 117;
const LVM_EDITLABELW: u32 = LVM_FIRST + 118;
const LVM_SETUNICODEFORMAT: u32 = 0x2005;
const LVM_GETUNICODEFORMAT: u32 = 0x2006;

pub(super) const LVS_ICON: u32 = 0x0000;
pub(super) const LVS_REPORT: u32 = 0x0001;
pub(super) const LVS_TYPEMASK: u32 = 0x0003;
pub(super) const LVS_SINGLESEL: u32 = 0x0004;
pub(super) const LVS_SHOWSELALWAYS: u32 = 0x0008;
pub(super) const LVS_SORTASCENDING: u32 = 0x0010;
pub(super) const LVS_SORTDESCENDING: u32 = 0x0020;
pub(super) const LVS_SHAREIMAGELISTS: u32 = 0x0040;
pub(super) const LVS_AUTOARRANGE: u32 = 0x0100;
pub(super) const LVS_EDITLABELS: u32 = 0x0200;
pub(super) const LVS_OWNERDRAWFIXED: u32 = 0x0400;
pub(super) const LVS_ALIGNLEFT: u32 = 0x0800;
pub(super) const LVS_ALIGNMASK: u32 = 0x0C00;
pub(super) const LVS_OWNERDATA: u32 = 0x1000;
pub(super) const LVS_NOSCROLL: u32 = 0x2000;
pub(super) const LVS_NOCOLUMNHEADER: u32 = 0x4000;
pub(super) const LVS_NOSORTHEADER: u32 = 0x8000;

pub(super) const LV_VIEW_ICON: u32 = 0;
pub(super) const LV_VIEW_DETAILS: u32 = 1;
pub(super) const LV_VIEW_SMALLICON: u32 = 2;
pub(super) const LV_VIEW_LIST: u32 = 3;

pub(super) const LVS_EX_GRIDLINES: u32 = 0x0001;
pub(super) const LVS_EX_SUBITEMIMAGES: u32 = 0x0002;
pub(super) const LVS_EX_CHECKBOXES: u32 = 0x0004;
pub(super) const LVS_EX_TRACKSELECT: u32 = 0x0008;
pub(super) const LVS_EX_HEADERDRAGDROP: u32 = 0x0010;
pub(super) const LVS_EX_FULLROWSELECT: u32 = 0x0020;
pub(super) const LVS_EX_ONECLICKACTIVATE: u32 = 0x0040;
pub(super) const LVS_EX_TWOCLICKACTIVATE: u32 = 0x0080;
/// The extended styles of COMCTL32 4.72.
const LVS_EX_KNOWN: u32 = 0xFFFF;

pub(super) const LVIF_TEXT: u32 = 0x0001;
pub(super) const LVIF_IMAGE: u32 = 0x0002;
pub(super) const LVIF_PARAM: u32 = 0x0004;
pub(super) const LVIF_STATE: u32 = 0x0008;
pub(super) const LVIF_INDENT: u32 = 0x0010;
pub(super) const LVIF_NORECOMPUTE: u32 = 0x0800;
pub(super) const LVIF_DI_SETITEM: u32 = 0x1000;

pub(super) const LVIS_FOCUSED: u32 = 0x0001;
pub(super) const LVIS_SELECTED: u32 = 0x0002;
pub(super) const LVIS_CUT: u32 = 0x0004;
pub(super) const LVIS_DROPHILITED: u32 = 0x0008;
pub(super) const LVIS_STATEIMAGEMASK: u32 = 0xF000;

pub(super) const LPSTR_TEXTCALLBACK: u32 = 0xFFFF_FFFF;
pub(super) const I_IMAGECALLBACK: i32 = -1;
pub(super) const I_INDENTCALLBACK: i32 = -1;

const LVSIL_NORMAL: u32 = 0;
const LVSIL_SMALL: u32 = 1;
const LVSIL_STATE: u32 = 2;

/// The notifications, in their Unicode codes where they have two.
const fn nc(n: i32) -> u32 {
    n as u32
}
pub(super) const NM_CLICK: u32 = nc(-2);
pub(super) const NM_DBLCLK: u32 = nc(-3);
pub(super) const NM_RETURN: u32 = nc(-4);
pub(super) const NM_RCLICK: u32 = nc(-5);
pub(super) const NM_RDBLCLK: u32 = nc(-6);
pub(super) const NM_SETFOCUS: u32 = nc(-7);
pub(super) const NM_KILLFOCUS: u32 = nc(-8);
pub(super) const NM_CUSTOMDRAW: u32 = nc(-12);
pub(super) const NM_HOVER: u32 = nc(-13);
pub(super) const NM_RELEASEDCAPTURE: u32 = nc(-16);
pub(super) const LVN_ITEMCHANGING: u32 = nc(-100);
pub(super) const LVN_ITEMCHANGED: u32 = nc(-101);
pub(super) const LVN_INSERTITEM: u32 = nc(-102);
pub(super) const LVN_DELETEITEM: u32 = nc(-103);
pub(super) const LVN_DELETEALLITEMS: u32 = nc(-104);
pub(super) const LVN_COLUMNCLICK: u32 = nc(-108);
pub(super) const LVN_BEGINDRAG: u32 = nc(-109);
pub(super) const LVN_ODCACHEHINT: u32 = nc(-113);
pub(super) const LVN_ITEMACTIVATE: u32 = nc(-114);
pub(super) const LVN_KEYDOWN: u32 = nc(-155);
pub(super) const LVN_BEGINLABELEDITW: u32 = nc(-175);
pub(super) const LVN_ENDLABELEDITW: u32 = nc(-176);
pub(super) const LVN_GETDISPINFOW: u32 = nc(-177);
pub(super) const LVN_ODFINDITEMW: u32 = nc(-179);

/// Where a script keeps its structures, in its scratch space.
pub(super) const S_NM: u32 = 0;
pub(super) const S_ITEM: u32 = 128;
pub(super) const S_AUX: u32 = 192;
pub(super) const S_PS: u32 = 256;
pub(super) const S_RECT: u32 = 320;
pub(super) const S_SCROLL: u32 = 352;
pub(super) const S_TEXT: u32 = 384;
/// Characters a text buffer holds.
pub(super) const TEXT_MAX: u32 = 512;
pub(super) const S_TEXT2: u32 = S_TEXT + 2 * TEXT_MAX + 16;
const S_SIZE: u32 = S_TEXT2 + 2 * TEXT_MAX + 16;

// The continuations: this module's, then each submodule's from its
// first.
const T_RESULT_LAST: u16 = 1;
const T_FORMAT: u16 = 2;
const T_REQUERY: u16 = 3;
const T_MEASURE: u16 = 4;
pub(super) const T_ITEMS: u16 = 100;
pub(super) const T_LAYOUT: u16 = 200;
pub(super) const T_COLUMNS: u16 = 250;
pub(super) const T_PAINT: u16 = 300;
pub(super) const T_INPUT: u16 = 400;

/// A subitem.
#[derive(Clone, Debug, Default)]
pub struct SubItem {
    pub sub: i32,
    pub image: i32,
    pub text: Option<Vec<u8>>,
    pub callback: bool,
    pub state: u32,
}

crate::state_fields!(SubItem {
    sub,
    image,
    text,
    callback,
    state
});

/// An item the control keeps.
#[derive(Clone, Debug, Default)]
pub struct Item {
    pub id: u32,
    /// The LVIS_ bits, the selection's in step with `selection`.
    pub state: u32,
    pub image: i32,
    pub lparam: u32,
    pub indent: i32,
    /// The text, ANSI; None when none was set or it is a callback.
    pub text: Option<Vec<u8>>,
    pub callback: bool,
    /// The subitems with something set, by their column.
    pub subs: Vec<SubItem>,
    /// Where the item is in the icon views, from the view's corner.
    pub pos: (i32, i32),
}

crate::state_fields!(Item {
    id,
    state,
    image,
    lparam,
    indent,
    text,
    callback,
    subs,
    pos
});

/// A column: its rectangle is the header item's.
#[derive(Clone, Debug, Default)]
pub struct Column {
    pub fmt: u32,
}

crate::state_fields!(Column { fmt });

/// What a list view keeps.
#[derive(Clone, Debug, Default)]
pub struct ListView {
    pub notify: u32,
    /// NFR_ANSI or NFR_UNICODE.
    pub format: u32,
    pub style: u32,
    pub ex_style: u32,
    pub view: u32,
    pub header: u32,
    pub items: Vec<Item>,
    /// The number of items of a virtual list view.
    pub od_count: i32,
    pub columns: Vec<Column>,
    /// The selected items, as ranges `lo..hi`, in order.
    pub selection: Vec<(i32, i32)>,
    pub focused: i32,
    pub mark: i32,
    pub hot: i32,
    pub callback_mask: u32,
    pub himl_normal: u32,
    pub himl_small: u32,
    pub himl_state: u32,
    pub icon_size: (i32, i32),
    pub icon_state_size: (i32, i32),
    pub icon_spacing: (i32, i32),
    /// Whether LVM_SETICONSPACING set the spacing.
    pub custom_spacing: bool,
    pub item_width: i32,
    pub item_height: i32,
    /// The client area without the header.
    pub list: Rect,
    pub font: u32,
    pub text_height: i32,
    pub max_char_width: i32,
    pub ellipsis_width: i32,
    pub clr_bk: u32,
    pub clr_text: u32,
    pub clr_text_bk: u32,
    pub redraw: bool,
    pub no_item_metrics: bool,
    pub change_notify: bool,
    pub drawing: bool,
    pub focus: bool,
    pub auto_arrange: bool,
    pub hover_time: u32,
    pub hot_cursor: u32,
    pub tooltips: u32,
    pub track_line: i32,
    pub measure_height: i32,
    pub icon_pos: (i32, i32),
    pub rc_focus: Rect,
    /// A sort under way: the items placed so far, by their old index.
    pub sorting: Vec<u32>,
    pub sort_compare: u32,
    pub sort_lparam: u32,
    /// The label being edited: its edit control, item and the edit
    /// control's own procedure.
    pub edit: u32,
    pub edit_item: i32,
    pub edit_proc: u32,
    pub ending_edit: bool,
    pub lbutton_down: bool,
    pub lbutton_item: i32,
    pub click_pos: (i32, i32),
    /// What painting keeps between its steps.
    pub paint: paint::Painting,
}

crate::state_fields!(ListView {
    notify,
    format,
    style,
    ex_style,
    view,
    header,
    items,
    od_count,
    columns,
    selection,
    focused,
    mark,
    hot,
    callback_mask,
    himl_normal,
    himl_small,
    himl_state,
    icon_size,
    icon_state_size,
    icon_spacing,
    custom_spacing,
    item_width,
    item_height,
    list,
    font,
    text_height,
    max_char_width,
    ellipsis_width,
    clr_bk,
    clr_text,
    clr_text_bk,
    redraw,
    no_item_metrics,
    change_notify,
    drawing,
    focus,
    auto_arrange,
    hover_time,
    hot_cursor,
    tooltips,
    track_line,
    measure_height,
    icon_pos,
    rc_focus,
    sorting,
    sort_compare,
    sort_lparam,
    edit,
    edit_item,
    edit_proc,
    ending_edit,
    lbutton_down,
    lbutton_item,
    click_pos,
    paint
});

impl ListView {
    pub fn ownerdata(&self) -> bool {
        self.style & LVS_OWNERDATA != 0
    }

    pub fn count(&self) -> i32 {
        if self.ownerdata() {
            self.od_count
        } else {
            self.items.len() as i32
        }
    }

    pub fn is_icon_view(&self) -> bool {
        self.view == LV_VIEW_ICON || self.view == LV_VIEW_SMALLICON
    }

    pub fn auto_arranged(&self) -> bool {
        (self.style & LVS_AUTOARRANGE != 0 || self.auto_arrange) && self.is_icon_view()
    }

    /// map_style_view.
    pub fn map_view(&mut self) {
        self.view = match self.style & LVS_TYPEMASK {
            LVS_REPORT => LV_VIEW_DETAILS,
            2 => LV_VIEW_SMALLICON,
            3 => LV_VIEW_LIST,
            _ => LV_VIEW_ICON,
        };
    }

    pub fn is_selected(&self, i: i32) -> bool {
        self.selection.iter().any(|&(lo, hi)| lo <= i && i < hi)
    }

    pub fn selected_count(&self) -> i32 {
        self.selection.iter().map(|&(lo, hi)| hi - lo).sum()
    }

    pub fn selected_items(&self) -> Vec<i32> {
        self.selection.iter().flat_map(|&(lo, hi)| lo..hi).collect()
    }

    /// Add `lo..hi` to the selection, and the items' SELECTED bits.
    pub fn select_range(&mut self, lo: i32, hi: i32) {
        if lo >= hi {
            return;
        }
        let mut ranges = std::mem::take(&mut self.selection);
        ranges.push((lo, hi));
        ranges.sort();
        for r in ranges {
            match self.selection.last_mut() {
                Some(last) if r.0 <= last.1 => last.1 = last.1.max(r.1),
                _ => self.selection.push(r),
            }
        }
        if !self.ownerdata() {
            for i in lo.max(0)..hi.min(self.items.len() as i32) {
                self.items[i as usize].state |= LVIS_SELECTED;
            }
        }
    }

    /// Take `lo..hi` out of the selection, and the items' SELECTED bits.
    pub fn deselect_range(&mut self, lo: i32, hi: i32) {
        if lo >= hi {
            return;
        }
        let mut out = Vec::new();
        for &(a, b) in &self.selection {
            if b <= lo || a >= hi {
                out.push((a, b));
                continue;
            }
            if a < lo {
                out.push((a, lo));
            }
            if b > hi {
                out.push((hi, b));
            }
        }
        self.selection = out;
        if !self.ownerdata() {
            for i in lo.max(0)..hi.min(self.items.len() as i32) {
                self.items[i as usize].state &= !LVIS_SELECTED;
            }
        }
    }

    /// ranges_shift: the selection after an item was inserted at `i`
    /// (`dir` 1) or taken out (-1); `count` is the new count.
    pub fn shift_selection(&mut self, i: i32, dir: i32, count: i32) {
        let upper = count;
        let mut out: Vec<(i32, i32)> = Vec::new();
        for &(mut lo, mut hi) in &self.selection {
            if lo >= i {
                lo = (lo + dir).min(upper - 1).max(0);
            }
            if hi > i {
                hi = (hi + dir).min(upper).max(0);
            }
            if lo < hi {
                out.push((lo, hi));
            }
        }
        self.selection.clear();
        for (lo, hi) in out {
            match self.selection.last_mut() {
                Some(last) if lo <= last.1 => last.1 = last.1.max(hi),
                _ => self.selection.push((lo, hi)),
            }
        }
    }

    pub fn header_enabled(&self) -> bool {
        self.view == LV_VIEW_DETAILS && self.style & LVS_NOCOLUMNHEADER == 0
    }

    /// The text of item `i`'s subitem `sub` as the control keeps it,
    /// for its own measures: callbacks count as no text.
    pub fn text_of(&self, i: i32, sub: i32) -> Vec<u8> {
        let Some(item) = self.items.get(i as usize) else {
            return Vec::new();
        };
        if sub == 0 {
            return item.text.clone().unwrap_or_default();
        }
        item.subs
            .iter()
            .find(|s| s.sub == sub)
            .and_then(|s| s.text.clone())
            .unwrap_or_default()
    }

    /// The control's font.
    pub fn hfont(&self, cpu: &mut Cpu) -> u32 {
        if self.font != 0 {
            self.font
        } else {
            gfx::default_font(cpu)
        }
    }
}

/// What the module keeps, in COMCTL32's `State`.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub lists: BTreeMap<u32, ListView>,
}

crate::state_fields!(State { lists });

pub(super) fn get(cpu: &mut Cpu, hwnd: u32) -> Option<ListView> {
    state(cpu).listview.lists.get(&hwnd).cloned()
}

pub(super) fn put(cpu: &mut Cpu, hwnd: u32, lv: ListView) {
    state(cpu).listview.lists.insert(hwnd, lv);
}

/// The step that runs the continuation `id` with `data`.
pub(super) fn step(id: u16, data: &[u32]) -> Op {
    Op::Then {
        id,
        data: data.to_vec(),
    }
}

/// A script for the control, with its scratch space.
fn new_script(hwnd: u32) -> Script {
    let mut s = Script::new(hwnd);
    s.alloc(S_SIZE);
    s
}

/// Run `ops`, returning `result` unless a step changes it.
pub(super) fn run_ops(cpu: &mut Cpu, hwnd: u32, ops: Vec<Op>, result: u32) -> Option<u32> {
    if ops.is_empty() {
        return Some(result);
    }
    let mut s = new_script(hwnd);
    s.ops = ops;
    s.result = result;
    script::run(cpu, LISTVIEW, s)
}

/// SendMessageA(to, msg, wp, lp) as a step.
pub(super) fn send_op(to: u32, msg: u32, wp: Arg, lp: Arg) -> Op {
    header::send_op(to, msg, wp, lp)
}

/// USER32's function `name` as a step.
pub(super) fn user_op(name: &str, args: &[Arg]) -> Op {
    Op::Call {
        target: user32::address_of(name),
        args: args.to_vec(),
    }
}

/// The code of a notification as the parent wants it: the ANSI one
/// for an ANSI parent.
pub(super) fn code_for(lv: &ListView, code: u32) -> u32 {
    if lv.format == header::NFR_UNICODE {
        return code;
    }
    match code as i32 {
        -175 => nc(-105),
        -176 => nc(-106),
        -177 => nc(-150),
        -178 => nc(-151),
        -179 => nc(-152),
        -158 => nc(-157),
        _ => code,
    }
}

/// The steps of WM_NOTIFY to the parent: an NMHDR with `code` (made
/// ANSI for an ANSI parent), then `more`, at `S_NM`.
pub(super) fn notify_ops(
    cpu: &mut Cpu,
    hwnd: u32,
    lv: &ListView,
    code: u32,
    more: &[u32],
) -> Vec<Op> {
    let id = id_of(cpu, hwnd);
    let mut words = vec![hwnd, id, code_for(lv, code)];
    words.extend_from_slice(more);
    vec![
        Op::Write { at: S_NM, words },
        send_op(lv.notify, WM_NOTIFY, Arg::V(id), Arg::S(S_NM)),
    ]
}

/// An NMLISTVIEW after its NMHDR.
pub(super) fn nmlistview(
    item: i32,
    sub: i32,
    new: u32,
    old: u32,
    changed: u32,
    lparam: u32,
) -> Vec<u32> {
    vec![item as u32, sub as u32, new, old, changed, 0, 0, lparam]
}

/// The window procedure.
pub fn proc_(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, msg, wp, lp] = args else {
        unreachable!()
    };
    if msg == WM_NCCREATE {
        nc_create(cpu, hwnd, lp)?;
        return defwnd::def_window_proc(cpu, args);
    }
    let Some(mut lv) = get(cpu, hwnd) else {
        return defwnd::def_window_proc(cpu, args);
    };
    let wide = matches!(
        msg,
        LVM_GETITEMW
            | LVM_SETITEMW
            | LVM_INSERTITEMW
            | LVM_FINDITEMW
            | LVM_GETSTRINGWIDTHW
            | LVM_GETCOLUMNW
            | LVM_SETCOLUMNW
            | LVM_INSERTCOLUMNW
            | LVM_GETITEMTEXTW
            | LVM_SETITEMTEXTW
            | LVM_EDITLABELW
    );
    let result = match msg {
        LVM_APPROXIMATEVIEWRECT => {
            let (cx, cy) = (lp as u16 as i16 as i32, (lp >> 16) as u16 as i16 as i32);
            layout::approximate_view_rect(cpu, hwnd, &lv, wp as i32, cx, cy)
        }
        LVM_ARRANGE => {
            let ok = layout::arrange(cpu, hwnd, &mut lv, wp);
            put(cpu, hwnd, lv);
            ok as u32
        }
        LVM_CREATEDRAGIMAGE => layout::create_drag_image(cpu, hwnd, &lv, wp as i32, lp)?,
        LVM_DELETEALLITEMS => return items::delete_all(cpu, hwnd, false),
        LVM_DELETECOLUMN => return columns::delete_column(cpu, hwnd, lv, wp as i32),
        LVM_DELETEITEM => return items::delete_item(cpu, hwnd, &lv, wp as i32),
        LVM_EDITLABELA | LVM_EDITLABELW => return input::edit_label(cpu, hwnd, &lv, wp as i32),
        LVM_ENSUREVISIBLE => {
            if (wp as i32) < 0 || wp as i32 >= lv.count() {
                return Some(0);
            }
            return run_ops(cpu, hwnd, layout::ensure_visible_ops(wp as i32, lp != 0), 1);
        }
        LVM_FINDITEMA | LVM_FINDITEMW => {
            return items::find_item(cpu, hwnd, &lv, wp as i32, lp, wide);
        }
        LVM_GETBKCOLOR => lv.clr_bk,
        LVM_GETCALLBACKMASK => lv.callback_mask,
        LVM_GETCOLUMNA | LVM_GETCOLUMNW => {
            columns::get_column(cpu, hwnd, &lv, wp as i32, lp, wide)?
        }
        LVM_GETCOLUMNORDERARRAY => {
            if lv.header == 0 {
                return Some(0);
            }
            let msg = header::HDM_GETORDERARRAY;
            let ops = vec![
                send_op(lv.header, msg, Arg::V(wp), Arg::V(lp)),
                Op::Return(Arg::Last),
            ];
            return run_ops(cpu, hwnd, ops, 0);
        }
        LVM_GETCOLUMNWIDTH => columns::get_column_width(cpu, &lv, wp as i32),
        LVM_GETCOUNTPERPAGE => layout::count_per_page(&lv) as u32,
        LVM_GETEDITCONTROL => lv.edit,
        LVM_GETEXTENDEDLISTVIEWSTYLE => lv.ex_style,
        LVM_GETHEADER => lv.header,
        LVM_GETHOTCURSOR => lv.hot_cursor,
        LVM_GETHOTITEM => lv.hot as u32,
        LVM_GETHOVERTIME => lv.hover_time,
        LVM_GETIMAGELIST => match wp {
            LVSIL_NORMAL => lv.himl_normal,
            LVSIL_SMALL => lv.himl_small,
            LVSIL_STATE => lv.himl_state,
            _ => 0,
        },
        LVM_GETISEARCHSTRINGA | LVM_GETISEARCHSTRINGW => 0,
        LVM_GETITEMA | LVM_GETITEMW => return items::get_item_msg(cpu, hwnd, lp, wide),
        LVM_GETITEMCOUNT => lv.count() as u32,
        LVM_GETITEMPOSITION => return layout::get_item_position_msg(cpu, hwnd, &lv, wp as i32, lp),
        LVM_GETITEMRECT => layout::get_item_rect_msg(cpu, hwnd, &lv, wp as i32, lp)?,
        LVM_GETITEMSPACING => layout::item_spacing(cpu, &lv, wp != 0),
        LVM_GETITEMSTATE => return items::get_item_state_msg(cpu, hwnd, &lv, wp as i32, lp),
        LVM_GETITEMTEXTA | LVM_GETITEMTEXTW => {
            return items::get_item_text_msg(cpu, hwnd, &lv, wp as i32, lp, wide);
        }
        LVM_GETNEXTITEM => items::next_item(cpu, hwnd, &lv, wp as i32, lp) as u32,
        LVM_GETNUMBEROFWORKAREAS => 1,
        LVM_GETWORKAREAS | LVM_SETWORKAREAS => 0,
        LVM_GETORIGIN => {
            if lp == 0 || !lv.is_icon_view() {
                return Some(0);
            }
            let (x, y) = layout::origin(cpu, hwnd, &lv);
            write_words(cpu, lp, &[x as u32, y as u32]);
            if cpu.bus.guest_faulted() {
                return None;
            }
            1
        }
        LVM_GETSELECTEDCOUNT => items::selected_count(&lv),
        LVM_GETSELECTIONMARK => lv.mark as u32,
        LVM_GETSTRINGWIDTHA | LVM_GETSTRINGWIDTHW => {
            if lp == 0 {
                return Some(0);
            }
            let text = header::read_text(cpu, lp, wide)?;
            layout::string_width(cpu, hwnd, &lv, &text) as u32
        }
        LVM_GETSUBITEMRECT => layout::get_subitem_rect_msg(cpu, hwnd, &lv, wp as i32, lp)?,
        LVM_GETTEXTBKCOLOR => lv.clr_text_bk,
        LVM_GETTEXTCOLOR => lv.clr_text,
        LVM_GETTOOLTIPS => lv.tooltips,
        LVM_GETTOPINDEX => layout::top_index(cpu, hwnd, &lv) as u32,
        LVM_GETUNICODEFORMAT => (lv.format == header::NFR_UNICODE) as u32,
        LVM_GETVIEWRECT => {
            if lp == 0 {
                return Some(0);
            }
            let r = layout::view_rect(cpu, hwnd, &lv);
            r.write(cpu, lp);
            if cpu.bus.guest_faulted() {
                return None;
            }
            1
        }
        LVM_HITTEST | LVM_SUBITEMHITTEST => {
            return layout::hit_test_msg(cpu, hwnd, &lv, lp, msg == LVM_SUBITEMHITTEST);
        }
        LVM_INSERTCOLUMNA | LVM_INSERTCOLUMNW => {
            return columns::insert_column(cpu, hwnd, lv, wp as i32, lp, wide);
        }
        LVM_INSERTITEMA | LVM_INSERTITEMW => return items::insert_item(cpu, hwnd, lv, lp, wide),
        LVM_REDRAWITEMS => {
            let (first, last) = (wp as i32, lp as i32);
            if last < first || first < 0 || last >= lv.count() {
                return Some(0);
            }
            for i in first..=last {
                layout::invalidate_item(cpu, hwnd, &lv, i);
            }
            1
        }
        LVM_SCROLL => return layout::scroll_msg(cpu, hwnd, &lv, wp as i32, lp as i32),
        LVM_SETBKCOLOR => {
            if lv.clr_bk != lp {
                lv.clr_bk = lp;
                put(cpu, hwnd, lv.clone());
                layout::invalidate_list(cpu, hwnd, &lv);
            }
            1
        }
        LVM_SETCALLBACKMASK => {
            lv.callback_mask = wp;
            put(cpu, hwnd, lv);
            1
        }
        LVM_SETCOLUMNA | LVM_SETCOLUMNW => {
            return columns::set_column(cpu, hwnd, &lv, wp as i32, lp, wide);
        }
        LVM_SETCOLUMNORDERARRAY => {
            if lv.header == 0 {
                return Some(0);
            }
            let msg = header::HDM_SETORDERARRAY;
            let ops = vec![
                send_op(lv.header, msg, Arg::V(wp), Arg::V(lp)),
                step(T_RESULT_LAST, &[]),
                step(columns::T_ORDER_CHANGED, &[]),
            ];
            return run_ops(cpu, hwnd, ops, 0);
        }
        LVM_SETCOLUMNWIDTH => return columns::set_column_width(cpu, hwnd, lv, wp as i32, lp),
        LVM_SETEXTENDEDLISTVIEWSTYLE => return set_ex_style(cpu, hwnd, lv, wp, lp),
        LVM_SETHOTCURSOR => {
            let old = lv.hot_cursor;
            lv.hot_cursor = lp;
            put(cpu, hwnd, lv);
            old
        }
        LVM_SETHOTITEM => {
            let old = lv.hot;
            lv.hot = wp as i32;
            put(cpu, hwnd, lv);
            old as u32
        }
        LVM_SETHOVERTIME => {
            let old = lv.hover_time;
            lv.hover_time = lp;
            put(cpu, hwnd, lv);
            old
        }
        LVM_SETICONSPACING => layout::set_icon_spacing(cpu, hwnd, lv, lp),
        LVM_SETIMAGELIST => return layout::set_image_list(cpu, hwnd, lv, wp, lp),
        LVM_SETITEMA | LVM_SETITEMW => return items::set_item_msg(cpu, hwnd, lp, wide),
        LVM_SETITEMCOUNT => return items::set_item_count(cpu, hwnd, lv, wp as i32, lp),
        LVM_SETITEMPOSITION => {
            let (x, y) = (lp as u16 as i16 as i32, (lp >> 16) as u16 as i16 as i32);
            layout::set_item_position(cpu, hwnd, lv, wp as i32, x, y) as u32
        }
        LVM_SETITEMPOSITION32 => {
            if lp == 0 {
                return Some(0);
            }
            let p = read_words(cpu, lp, 2);
            if cpu.bus.guest_faulted() {
                return None;
            }
            layout::set_item_position(cpu, hwnd, lv, wp as i32, p[0] as i32, p[1] as i32) as u32
        }
        LVM_SETITEMSTATE => {
            if lp == 0 {
                return Some(0);
            }
            let w = read_words(cpu, lp, 5);
            if cpu.bus.guest_faulted() {
                return None;
            }
            return items::set_item_state_msg(cpu, hwnd, &lv, wp as i32, w[3], w[4]);
        }
        LVM_SETITEMTEXTA | LVM_SETITEMTEXTW => {
            return items::set_item_text_msg(cpu, hwnd, wp as i32, lp, wide);
        }
        LVM_SETSELECTIONMARK => {
            let old = lv.mark;
            lv.mark = lp as i32;
            put(cpu, hwnd, lv);
            old as u32
        }
        LVM_SETTEXTBKCOLOR => {
            lv.clr_text_bk = lp;
            put(cpu, hwnd, lv);
            1
        }
        LVM_SETTEXTCOLOR => {
            lv.clr_text = lp;
            put(cpu, hwnd, lv);
            1
        }
        LVM_SETTOOLTIPS => {
            let old = lv.tooltips;
            lv.tooltips = wp;
            put(cpu, hwnd, lv);
            old
        }
        LVM_SETUNICODEFORMAT => {
            let old = lv.format == header::NFR_UNICODE;
            lv.format = if wp != 0 {
                header::NFR_UNICODE
            } else {
                header::NFR_ANSI
            };
            put(cpu, hwnd, lv);
            old as u32
        }
        LVM_SORTITEMS => return items::sort_items(cpu, hwnd, lv, lp, wp),
        LVM_UPDATE => {
            if (wp as i32) < 0 || wp as i32 >= lv.count() {
                return Some(0);
            }
            if lv.auto_arranged() {
                layout::arrange(cpu, hwnd, &mut lv, 0);
                put(cpu, hwnd, lv.clone());
            } else {
                layout::invalidate_item(cpu, hwnd, &lv, wp as i32);
            }
            1
        }
        WM_CHAR => return input::char_(cpu, hwnd, &lv, wp),
        WM_COMMAND => return input::command(cpu, hwnd, &lv, wp, lp),
        WM_CREATE => return create(cpu, hwnd, lv, lp),
        WM_DESTROY => {
            let ops = vec![step(items::T_DELETE_ALL, &[1])];
            return run_ops(cpu, hwnd, ops, 0);
        }
        WM_ENABLE => {
            if lv.style & LVS_OWNERDRAWFIXED != 0 {
                super::invalidate(cpu, hwnd, true);
            }
            1
        }
        WM_ERASEBKGND => return paint::erase_bkgnd(cpu, hwnd, &lv, wp),
        WM_GETDLGCODE => 0x0001 | 0x0080,
        WM_GETFONT => lv.font,
        WM_HSCROLL => return run_ops(cpu, hwnd, layout::hscroll_ops(wp & 0xFFFF, 0), 0),
        WM_VSCROLL => return run_ops(cpu, hwnd, layout::vscroll_ops(wp & 0xFFFF, 0), 0),
        WM_MOUSEWHEEL => {
            if wp & (0x0004 | 0x0008) != 0 {
                return defwnd::def_window_proc(cpu, args);
            }
            let delta = (wp >> 16) as u16 as i16 as i32;
            return run_ops(cpu, hwnd, layout::wheel_ops(&lv, delta), 0);
        }
        WM_KEYDOWN => return input::key_down(cpu, hwnd, &lv, wp, lp),
        WM_KILLFOCUS => return input::kill_focus(cpu, hwnd, lv),
        WM_SETFOCUS => return input::set_focus(cpu, hwnd, lv),
        WM_LBUTTONDBLCLK => return input::lbutton_dblclk(cpu, hwnd, &lv, wp, lp),
        WM_LBUTTONDOWN => return input::lbutton_down(cpu, hwnd, lv, wp, lp),
        WM_LBUTTONUP => return input::lbutton_up(cpu, hwnd, lv, wp, lp),
        WM_RBUTTONDBLCLK => return input::rbutton_dblclk(cpu, hwnd, &lv, lp),
        WM_RBUTTONDOWN => return input::rbutton_down(cpu, hwnd, lv, wp, lp),
        WM_RBUTTONUP => return input::rbutton_up(cpu, hwnd, &lv, wp, lp),
        WM_MOUSEMOVE => return input::mouse_move(cpu, hwnd, lv, wp, lp),
        WM_MOUSEHOVER => return input::mouse_hover(cpu, hwnd, &lv, lp),
        WM_TIMER => return defwnd::def_window_proc(cpu, args),
        WM_NCDESTROY => {
            nc_destroy(cpu, hwnd, &lv);
            return defwnd::def_window_proc(cpu, args);
        }
        WM_NOTIFY => {
            if lp == 0 {
                return Some(0);
            }
            let from = cpu.bus.guest_read_32(lp);
            if cpu.bus.guest_faulted() {
                return None;
            }
            if from != lv.header || lv.header == 0 {
                return Some(0);
            }
            return columns::header_notification(cpu, hwnd, &lv, lp);
        }
        WM_NOTIFYFORMAT => {
            if lp == header::NF_REQUERY {
                let ops = vec![
                    send_op(
                        lv.notify,
                        WM_NOTIFYFORMAT,
                        Arg::V(hwnd),
                        Arg::V(header::NF_QUERY),
                    ),
                    step(T_REQUERY, &[]),
                ];
                return run_ops(cpu, hwnd, ops, 0);
            }
            lv.format
        }
        WM_PAINT => return paint::paint(cpu, hwnd, &lv, wp, args),
        WM_PRINTCLIENT => return paint::print_client(cpu, hwnd, &lv, wp, lp),
        WM_SETFONT => return set_font(cpu, hwnd, lv, wp, lp),
        WM_SETREDRAW => {
            let on = wp != 0;
            if lv.redraw == on {
                return Some(0);
            }
            lv.redraw = on;
            if !on {
                put(cpu, hwnd, lv);
                return Some(0);
            }
            if lv.auto_arranged() {
                layout::arrange(cpu, hwnd, &mut lv, 0);
            }
            put(cpu, hwnd, lv.clone());
            let ops = layout::update_scroll_ops(&lv);
            layout::invalidate_list(cpu, hwnd, &lv);
            return run_ops(cpu, hwnd, ops, 0);
        }
        WM_SHOWWINDOW => {
            let mut ops = Vec::new();
            if lv.view == LV_VIEW_DETAILS && wp != 0 {
                ops.extend(columns::create_header_ops(&lv));
                ops.push(step(columns::T_SHOW_HEADER, &[]));
            }
            ops.push(user_op(
                "DefWindowProcA",
                &[Arg::V(hwnd), Arg::V(msg), Arg::V(wp), Arg::V(lp)],
            ));
            ops.push(Op::Return(Arg::Last));
            return run_ops(cpu, hwnd, ops, 0);
        }
        WM_SIZE => return layout::size(cpu, hwnd, lv),
        WM_WINDOWPOSCHANGED => {
            let flags = cpu.bus.guest_read_32(lp + 24);
            if cpu.bus.guest_faulted() {
                return None;
            }
            let mut ops = Vec::new();
            if flags & user32::window::SWP_NOSIZE == 0 {
                ops.push(step(layout::T_UPDATE_SIZE, &[]));
                ops.extend(layout::update_scroll_ops(&lv));
            }
            ops.push(user_op(
                "DefWindowProcA",
                &[Arg::V(hwnd), Arg::V(msg), Arg::V(wp), Arg::V(lp)],
            ));
            ops.push(Op::Return(Arg::Last));
            return run_ops(cpu, hwnd, ops, 0);
        }
        WM_STYLECHANGED => return layout::style_changed(cpu, hwnd, lv, wp, lp),
        WM_STYLECHANGING => {
            if wp == GWL_STYLE {
                let w = read_words(cpu, lp, 2);
                if cpu.bus.guest_faulted() {
                    return None;
                }
                let (old, mut new) = (w[0], w[1]);
                if (old ^ new) & LVS_OWNERDATA != 0 {
                    if old & LVS_OWNERDATA != 0 {
                        new |= LVS_OWNERDATA;
                    } else {
                        new &= !LVS_OWNERDATA;
                    }
                    cpu.bus.guest_write_32(lp + 4, new);
                }
            }
            0
        }
        _ => return defwnd::def_window_proc(cpu, args),
    };
    Some(result)
}

/// WM_NCCREATE: the control's defaults (LISTVIEW_NCCreate).
fn nc_create(cpu: &mut Cpu, hwnd: u32, lp: u32) -> Option<()> {
    let w = read_words(cpu, lp, 11);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let (parent, style) = (w[3], w[10]);
    let mut lv = ListView {
        notify: parent,
        format: header::NFR_ANSI,
        style,
        clr_bk: gfx::sys_color(cpu, gfx::COLOR_WINDOW),
        clr_text: gfx::CLR_DEFAULT,
        clr_text_bk: gfx::CLR_DEFAULT,
        focused: -1,
        mark: -1,
        hot: -1,
        redraw: true,
        no_item_metrics: true,
        change_notify: true,
        edit_item: -1,
        lbutton_item: -1,
        hover_time: 0xFFFF_FFFF,
        track_line: -1,
        font: gfx::default_font(cpu),
        ..ListView::default()
    };
    lv.map_view();
    lv.icon_spacing = layout::default_spacing(cpu, &lv);
    layout::save_text_metrics(cpu, hwnd, &mut lv);
    put(cpu, hwnd, lv);
    Some(())
}

/// WM_CREATE (LISTVIEW_Create): asks the parent for the format of the
/// notifications first.
fn create(cpu: &mut Cpu, hwnd: u32, mut lv: ListView, lp: u32) -> Option<u32> {
    let style = cpu.bus.guest_read_32(lp + 32);
    if cpu.bus.guest_faulted() {
        return None;
    }
    lv.style = style;
    lv.map_view();
    put(cpu, hwnd, lv.clone());
    let ops = vec![
        send_op(
            lv.notify,
            WM_NOTIFYFORMAT,
            Arg::V(hwnd),
            Arg::V(header::NF_QUERY),
        ),
        step(T_FORMAT, &[style]),
    ];
    run_ops(cpu, hwnd, ops, 0)
}

/// The rest of WM_CREATE, with the format.
fn created(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let style = c.word(0);
    let Some(mut lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    lv.format = if c.last == 0 {
        header::NFR_ANSI
    } else {
        c.last
    };
    let mut ops = Vec::new();
    let report = lv.view == LV_VIEW_DETAILS;
    if report && style & user32::window::WS_VISIBLE != 0 {
        ops.extend(columns::create_header_ops(&lv));
    }
    layout::update_item_size(cpu, c.hwnd, &mut lv);
    if report {
        if style & LVS_NOCOLUMNHEADER == 0 && style & user32::window::WS_VISIBLE != 0 {
            ops.push(step(columns::T_SHOW_HEADER, &[]));
        }
        ops.extend(layout::update_scroll_ops(&lv));
        if lv.style & LVS_OWNERDRAWFIXED != 0 {
            ops.extend(measure_item_ops(cpu, c.hwnd, &lv, c.scratch));
        }
    }
    let small = lv.view != LV_VIEW_ICON;
    lv.icon_size = layout::icon_size_of(cpu, lv.himl_normal, small);
    lv.icon_state_size = layout::icon_size_of(cpu, lv.himl_state, true);
    put(cpu, c.hwnd, lv);
    ops
}

/// The steps of WM_MEASUREITEM to the parent (notify_measureitem).
fn measure_item_ops(cpu: &mut Cpu, hwnd: u32, lv: &ListView, scratch: u32) -> Vec<Op> {
    let id = id_of(cpu, hwnd);
    let words = [102, id, 0, 0, lv.text_height as u32, 0];
    write_words(cpu, scratch + S_AUX, &words);
    vec![
        send_op(lv.notify, WM_MEASUREITEM, Arg::V(id), Arg::S(S_AUX)),
        step(T_MEASURE, &[]),
    ]
}

/// WM_NCDESTROY: the image lists go with the control unless they are
/// shared.
fn nc_destroy(cpu: &mut Cpu, hwnd: u32, lv: &ListView) {
    if lv.style & LVS_SHAREIMAGELISTS == 0 {
        for himl in [lv.himl_normal, lv.himl_small, lv.himl_state] {
            if himl != 0 {
                super::imagelist::destroy(cpu, himl);
            }
        }
    }
    state(cpu).listview.lists.remove(&hwnd);
}

/// WM_SETFONT.
fn set_font(cpu: &mut Cpu, hwnd: u32, mut lv: ListView, font: u32, redraw: u32) -> Option<u32> {
    let old = lv.font;
    lv.font = if font == 0 {
        gfx::default_font(cpu)
    } else {
        font
    };
    if lv.font == old {
        return Some(0);
    }
    layout::save_text_metrics(cpu, hwnd, &mut lv);
    put(cpu, hwnd, lv.clone());
    let mut ops = Vec::new();
    if lv.view == LV_VIEW_DETAILS && lv.header != 0 {
        let lp = redraw & 0xFFFF;
        ops.push(send_op(lv.header, WM_SETFONT, Arg::V(lv.font), Arg::V(lp)));
        ops.push(step(layout::T_UPDATE_SIZE, &[]));
        ops.extend(layout::update_scroll_ops(&lv));
    }
    if redraw & 0xFFFF != 0 {
        layout::invalidate_list(cpu, hwnd, &lv);
    }
    run_ops(cpu, hwnd, ops, 0)
}

/// LVM_SETEXTENDEDLISTVIEWSTYLE (LISTVIEW_SetExtendedListViewStyle):
/// the styles before.
fn set_ex_style(cpu: &mut Cpu, hwnd: u32, mut lv: ListView, mask: u32, style: u32) -> Option<u32> {
    let old = lv.ex_style;
    let mask = if mask == 0 { !0 } else { mask } & LVS_EX_KNOWN;
    lv.ex_style = (old & !mask) | (style & mask);
    let changed = old ^ lv.ex_style;
    if changed & LVS_EX_CHECKBOXES != 0 {
        if lv.ex_style & LVS_EX_CHECKBOXES != 0 {
            let himl = layout::check_box_image_list(cpu, hwnd, &lv);
            let old_himl = lv.himl_state;
            lv.himl_state = himl;
            lv.icon_state_size = layout::icon_size_of(cpu, himl, true);
            if old_himl != 0 && old_himl != himl && lv.style & LVS_SHAREIMAGELISTS == 0 {
                super::imagelist::destroy(cpu, old_himl);
            }
            // Every item gets the unchecked box.
            for item in lv.items.iter_mut() {
                item.state = (item.state & !LVIS_STATEIMAGEMASK) | (1 << 12);
            }
        } else {
            if lv.himl_state != 0 && lv.style & LVS_SHAREIMAGELISTS == 0 {
                super::imagelist::destroy(cpu, lv.himl_state);
            }
            lv.himl_state = 0;
            lv.icon_state_size = (0, 0);
        }
    }
    if changed & LVS_EX_HEADERDRAGDROP != 0 && lv.header != 0 {
        let hstyle = super::style(cpu, lv.header);
        let new = if lv.ex_style & LVS_EX_HEADERDRAGDROP != 0 {
            hstyle | header::HDS_DRAGDROP
        } else {
            hstyle & !header::HDS_DRAGDROP
        };
        if let Some(w) = user32::user(cpu).window_mut(lv.header) {
            w.style = new;
        }
    }
    layout::update_item_size(cpu, hwnd, &mut lv);
    put(cpu, hwnd, lv.clone());
    layout::invalidate_list(cpu, hwnd, &lv);
    let mut ops = Vec::new();
    if changed & LVS_EX_GRIDLINES != 0 {
        ops.push(step(layout::T_UPDATE_SIZE, &[]));
    }
    ops.extend(layout::update_scroll_ops(&lv));
    run_ops(cpu, hwnd, ops, old)
}

/// The continuations of its scripts.
pub fn then(cpu: &mut Cpu, id: u16, c: &mut Ctx) -> Vec<Op> {
    match id {
        T_RESULT_LAST => {
            c.result = c.last;
            Vec::new()
        }
        T_FORMAT => created(cpu, c),
        T_REQUERY => {
            if let Some(mut lv) = get(cpu, c.hwnd) {
                lv.format = c.last;
                put(cpu, c.hwnd, lv);
            }
            c.result = c.last;
            Vec::new()
        }
        T_MEASURE => {
            let h = cpu.bus.guest_read_32(c.scratch + S_AUX + 16) as i32;
            if let Some(mut lv) = get(cpu, c.hwnd)
                && h != lv.measure_height
            {
                lv.measure_height = h.max(1);
                layout::update_item_size(cpu, c.hwnd, &mut lv);
                put(cpu, c.hwnd, lv);
            }
            Vec::new()
        }
        T_ITEMS..T_LAYOUT => items::then(cpu, id, c),
        T_LAYOUT..T_COLUMNS => layout::then(cpu, id, c),
        T_COLUMNS..T_PAINT => columns::then(cpu, id, c),
        T_PAINT..T_INPUT => paint::then(cpu, id, c),
        T_INPUT.. => input::then(cpu, id, c),
        _ => Vec::new(),
    }
}

/// The edit control's procedure while it edits a label.
pub fn edit_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    input::edit_proc(cpu, args)
}
