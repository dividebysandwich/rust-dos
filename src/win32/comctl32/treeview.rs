//! The tree view (SysTreeView32): a hierarchy of items, each with a
//! label, images and state, that expand and collapse under their
//! parents, drawn with lines and buttons as its styles ask, scrolled a
//! whole item at a time, and edited in place through an edit control.
//!
//! The control keeps its items in Rust, by handle. HTREEITEMs are
//! numbers the control hands out, unique across the trees. Each change
//! the parent may veto or watch (TVN_SELCHANGING, TVN_ITEMEXPANDING,
//! TVN_GETDISPINFO, NM_CUSTOMDRAW and the rest) goes to it through
//! WM_NOTIFY, which calls into the program; a message whose work sends
//! one runs as steps (`Step`), kept as words between the calls in the
//! script's continuation, so that what the parent sees and does in a
//! notification finds the tree as Windows has it at that point.
//!
//! What it leaves out: Unicode messages (the W messages fail and the A
//! notifications are sent), the line color (TVM_SETLINECOLOR is
//! COMCTL32 5.80's), TVS_SINGLEEXPAND, TVS_INFOTIP and the tooltips of
//! labels cut off, and smooth scrolling. Sorting and measuring use the
//! label the parent gave last for items whose labels are callbacks.
//!
//! Ported from Wine's treeview.c:
//!
//! Copyright 1998 Eric Kohl
//! Copyright 1998, 1999 Alex Priem
//! Copyright 1999 Sylvain St-Germain
//! Copyright 2002 CodeWeavers, Aric Stewart
//!
//! This library is free software; you can redistribute it and/or modify
//! it under the terms of the GNU Lesser General Public License as
//! published by the Free Software Foundation; either version 2.1 of the
//! License, or (at your option) any later version.

use super::gfx::{
    self, CLR_DEFAULT, CLR_NONE, COLOR_BTNFACE, COLOR_BTNTEXT, COLOR_GRAYTEXT, COLOR_HIGHLIGHT,
    COLOR_HIGHLIGHTTEXT, COLOR_WINDOW, COLOR_WINDOWTEXT,
};
use super::script::{self, Arg, Ctx, Op, Script};
use super::{TREEVIEW, imagelist, invalidate, state};
use crate::cpu::Cpu;
use crate::win32::gdi32::{self, Rect};
use crate::win32::user32::message::{
    WM_CHAR, WM_COMMAND, WM_CONTEXTMENU, WM_CREATE, WM_DESTROY, WM_ERASEBKGND, WM_GETDLGCODE,
    WM_GETFONT, WM_HSCROLL, WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCDESTROY, WM_NOTIFY, WM_NOTIFYFORMAT, WM_PAINT, WM_PRINTCLIENT,
    WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETCURSOR, WM_SETFOCUS, WM_SETFONT, WM_SETREDRAW, WM_SIZE,
    WM_STYLECHANGED, WM_TIMER, WM_VSCROLL,
};
use crate::win32::user32::{self, defwnd, read_words, write_words};
use std::cmp::Ordering;
use std::collections::{BTreeMap, VecDeque};

const WM_MOUSEWHEEL: u32 = 0x020A;
const WM_CAPTURECHANGED: u32 = 0x0215;

const TVM_INSERTITEMA: u32 = 0x1100;
const TVM_DELETEITEM: u32 = 0x1101;
const TVM_EXPAND: u32 = 0x1102;
const TVM_GETITEMRECT: u32 = 0x1104;
const TVM_GETCOUNT: u32 = 0x1105;
const TVM_GETINDENT: u32 = 0x1106;
const TVM_SETINDENT: u32 = 0x1107;
const TVM_GETIMAGELIST: u32 = 0x1108;
const TVM_SETIMAGELIST: u32 = 0x1109;
const TVM_GETNEXTITEM: u32 = 0x110A;
const TVM_SELECTITEM: u32 = 0x110B;
const TVM_GETITEMA: u32 = 0x110C;
const TVM_SETITEMA: u32 = 0x110D;
const TVM_EDITLABELA: u32 = 0x110E;
const TVM_GETEDITCONTROL: u32 = 0x110F;
const TVM_GETVISIBLECOUNT: u32 = 0x1110;
const TVM_HITTEST: u32 = 0x1111;
const TVM_CREATEDRAGIMAGE: u32 = 0x1112;
const TVM_SORTCHILDREN: u32 = 0x1113;
const TVM_ENSUREVISIBLE: u32 = 0x1114;
const TVM_SORTCHILDRENCB: u32 = 0x1115;
const TVM_ENDEDITLABELNOW: u32 = 0x1116;
const TVM_SETTOOLTIPS: u32 = 0x1118;
const TVM_GETTOOLTIPS: u32 = 0x1119;
const TVM_SETINSERTMARK: u32 = 0x111A;
const TVM_SETITEMHEIGHT: u32 = 0x111B;
const TVM_GETITEMHEIGHT: u32 = 0x111C;
const TVM_SETBKCOLOR: u32 = 0x111D;
const TVM_SETTEXTCOLOR: u32 = 0x111E;
const TVM_GETBKCOLOR: u32 = 0x111F;
const TVM_GETTEXTCOLOR: u32 = 0x1120;
const TVM_SETSCROLLTIME: u32 = 0x1121;
const TVM_GETSCROLLTIME: u32 = 0x1122;
const TVM_SETINSERTMARKCOLOR: u32 = 0x1125;
const TVM_GETINSERTMARKCOLOR: u32 = 0x1126;
const TVM_GETITEMSTATE: u32 = 0x1127;
const TVM_SETUNICODEFORMAT: u32 = 0x2005;
const TVM_GETUNICODEFORMAT: u32 = 0x2006;

const TVIF_TEXT: u32 = 0x0001;
const TVIF_IMAGE: u32 = 0x0002;
const TVIF_PARAM: u32 = 0x0004;
const TVIF_STATE: u32 = 0x0008;
const TVIF_HANDLE: u32 = 0x0010;
const TVIF_SELECTEDIMAGE: u32 = 0x0020;
const TVIF_CHILDREN: u32 = 0x0040;
const TVIF_INTEGRAL: u32 = 0x0080;
const TVIF_DI_SETITEM: u32 = 0x1000;
const CALLBACK_MASK_ALL: u32 = TVIF_TEXT | TVIF_CHILDREN | TVIF_IMAGE | TVIF_SELECTEDIMAGE;
/// What TVN_ITEMEXPANDING and TVN_ITEMEXPANDED give of the item.
const EXPAND_MASK: u32 = TVIF_HANDLE | TVIF_STATE | TVIF_PARAM | TVIF_IMAGE | TVIF_SELECTEDIMAGE;

const TVIS_SELECTED: u32 = 0x0002;
const TVIS_DROPHILITED: u32 = 0x0008;
const TVIS_BOLD: u32 = 0x0010;
const TVIS_EXPANDED: u32 = 0x0020;
const TVIS_EXPANDEDONCE: u32 = 0x0040;
const TVIS_OVERLAYMASK: u32 = 0x0F00;
const TVIS_STATEIMAGEMASK: u32 = 0xF000;

const TVS_HASBUTTONS: u32 = 0x0001;
const TVS_HASLINES: u32 = 0x0002;
const TVS_LINESATROOT: u32 = 0x0004;
const TVS_EDITLABELS: u32 = 0x0008;
const TVS_DISABLEDRAGDROP: u32 = 0x0010;
const TVS_SHOWSELALWAYS: u32 = 0x0020;
const TVS_NOTOOLTIPS: u32 = 0x0080;
const TVS_CHECKBOXES: u32 = 0x0100;
const TVS_TRACKSELECT: u32 = 0x0200;
const TVS_NOSCROLL: u32 = 0x2000;
const TVS_NONEVENHEIGHT: u32 = 0x4000;
const TVS_NOHSCROLL: u32 = 0x8000;

const TVGN_ROOT: u32 = 0x0;
const TVGN_NEXT: u32 = 0x1;
const TVGN_PREVIOUS: u32 = 0x2;
const TVGN_PARENT: u32 = 0x3;
const TVGN_CHILD: u32 = 0x4;
const TVGN_FIRSTVISIBLE: u32 = 0x5;
const TVGN_NEXTVISIBLE: u32 = 0x6;
const TVGN_PREVIOUSVISIBLE: u32 = 0x7;
const TVGN_DROPHILITE: u32 = 0x8;
const TVGN_CARET: u32 = 0x9;
const TVGN_LASTVISIBLE: u32 = 0xA;

const TVE_COLLAPSE: u32 = 0x0001;
const TVE_EXPAND: u32 = 0x0002;
const TVE_TOGGLE: u32 = 0x0003;
const TVE_EXPANDPARTIAL: u32 = 0x4000;
const TVE_COLLAPSERESET: u32 = 0x8000;

const TVC_UNKNOWN: u32 = 0;
const TVC_BYMOUSE: u32 = 1;
const TVC_BYKEYBOARD: u32 = 2;

const TVI_ROOT: u32 = 0xFFFF_0000;
const TVI_FIRST: u32 = 0xFFFF_0001;
const TVI_LAST: u32 = 0xFFFF_0002;
const TVI_SORT: u32 = 0xFFFF_0003;

const TVSIL_NORMAL: u32 = 0;
const TVSIL_STATE: u32 = 2;

const TVHT_NOWHERE: u32 = 0x0001;
const TVHT_ONITEMICON: u32 = 0x0002;
const TVHT_ONITEMLABEL: u32 = 0x0004;
const TVHT_ONITEMINDENT: u32 = 0x0008;
const TVHT_ONITEMBUTTON: u32 = 0x0010;
const TVHT_ONITEMRIGHT: u32 = 0x0020;
const TVHT_ONITEMSTATEICON: u32 = 0x0040;
const TVHT_ONITEM: u32 = TVHT_ONITEMICON | TVHT_ONITEMLABEL | TVHT_ONITEMSTATEICON;
const TVHT_ABOVE: u32 = 0x0100;
const TVHT_BELOW: u32 = 0x0200;
const TVHT_TORIGHT: u32 = 0x0400;
const TVHT_TOLEFT: u32 = 0x0800;

const fn code(n: i32) -> u32 {
    n as u32
}
const TVN_SELCHANGINGA: u32 = code(-401);
const TVN_SELCHANGEDA: u32 = code(-402);
const TVN_GETDISPINFOA: u32 = code(-403);
const TVN_ITEMEXPANDINGA: u32 = code(-405);
const TVN_ITEMEXPANDEDA: u32 = code(-406);
const TVN_BEGINDRAGA: u32 = code(-407);
const TVN_BEGINRDRAGA: u32 = code(-408);
const TVN_DELETEITEMA: u32 = code(-409);
const TVN_BEGINLABELEDITA: u32 = code(-410);
const TVN_ENDLABELEDITA: u32 = code(-411);
const TVN_KEYDOWN: u32 = code(-412);
const NM_CLICK: u32 = code(-2);
const NM_DBLCLK: u32 = code(-3);
const NM_RCLICK: u32 = code(-5);
const NM_SETFOCUS: u32 = code(-7);
const NM_KILLFOCUS: u32 = code(-8);
const NM_CUSTOMDRAW: u32 = code(-12);
const NM_SETCURSOR: u32 = code(-17);

const CDDS_PREPAINT: u32 = 0x0001;
const CDDS_POSTPAINT: u32 = 0x0002;
const CDDS_ITEMPREPAINT: u32 = 0x0001_0001;
const CDDS_ITEMPOSTPAINT: u32 = 0x0001_0002;
const CDRF_NEWFONT: u32 = 0x0002;
const CDRF_SKIPDEFAULT: u32 = 0x0004;
const CDRF_NOTIFYPOSTPAINT: u32 = 0x0010;
const CDRF_NOTIFYITEMDRAW: u32 = 0x0020;
const CDIS_SELECTED: u32 = 0x0001;
const CDIS_FOCUS: u32 = 0x0010;
const CDIS_HOT: u32 = 0x0040;

const NF_QUERY: u32 = 3;
const NF_REQUERY: u32 = 4;
const NFR_ANSI: u32 = 1;
const NFR_UNICODE: u32 = 2;

const EN_KILLFOCUS: u32 = 0x0200;
const EN_UPDATE: u32 = 0x0400;
const EM_SETSEL: u32 = 0x00B1;

const SB_HORZ: u32 = 0;
const SB_VERT: u32 = 1;
const SB_LINEUP: u32 = 0;
const SB_LINEDOWN: u32 = 1;
const SB_PAGEUP: u32 = 2;
const SB_PAGEDOWN: u32 = 3;
const SB_THUMBPOSITION: u32 = 4;
const SB_THUMBTRACK: u32 = 5;
const SB_TOP: u32 = 6;
const SB_BOTTOM: u32 = 7;
const SB_ENDSCROLL: u32 = 8;
const SIF_RANGE: u32 = 0x1;
const SIF_PAGE: u32 = 0x2;
const SIF_POS: u32 = 0x4;
const SM_CXVSCROLL: u32 = 2;
const SM_CYHSCROLL: u32 = 3;
const SM_CXDRAG: u32 = 68;
const SM_CYDRAG: u32 = 69;

const VK_BACK: u32 = 0x08;
const VK_RETURN: u32 = 0x0D;
const VK_CONTROL: u32 = 0x11;
const VK_ESCAPE: u32 = 0x1B;
const VK_SPACE: u32 = 0x20;
const VK_PRIOR: u32 = 0x21;
const VK_NEXT: u32 = 0x22;
const VK_END: u32 = 0x23;
const VK_HOME: u32 = 0x24;
const VK_LEFT: u32 = 0x25;
const VK_UP: u32 = 0x26;
const VK_RIGHT: u32 = 0x27;
const VK_DOWN: u32 = 0x28;
const VK_MULTIPLY: u32 = 0x6A;
const VK_ADD: u32 = 0x6B;
const VK_SUBTRACT: u32 = 0x6D;

const MK_SHIFT: u32 = 0x0004;
const MK_CONTROL: u32 = 0x0008;

const WS_CHILD: u32 = 0x4000_0000;
const WS_POPUP: u32 = 0x8000_0000;
const WS_BORDER: u32 = 0x0080_0000;
const WS_CLIPSIBLINGS: u32 = 0x0400_0000;
const WS_EX_CLIENTEDGE: u32 = 0x0000_0200;
const ES_AUTOHSCROLL: u32 = 0x0080;
const ES_WANTRETURN: u32 = 0x1000;
const GWL_EXSTYLE: u32 = (-20i32) as u32;
const GWL_STYLE: u32 = (-16i32) as u32;
const CW_USEDEFAULT: u32 = 0x8000_0000;
const SW_HIDE: u32 = 0;
const SW_SHOW: u32 = 5;
const SWP_NOMOVE: u32 = 0x0002;
const SWP_DRAWFRAME: u32 = 0x0020;
const SIZE_RESTORED: u32 = 0;
const DLGC_WANTARROWS: u32 = 0x0001;
const DLGC_WANTALLKEYS: u32 = 0x0004;
const DLGC_WANTCHARS: u32 = 0x0080;
const PRF_CHECKVISIBLE: u32 = 0x0001;
const PRF_CLIENT: u32 = 0x0004;
const PRF_ERASEBKGND: u32 = 0x0008;
const ILD_NORMAL: u32 = 0;
const ILC_COLOR: u32 = 0;
const ILC_MASK: u32 = 1;
const DT_LEFT: u32 = 0;
const DT_SINGLELINE: u32 = 0x20;
const DT_NOPREFIX: u32 = 0x800;
const TRANSPARENT: u32 = 1;
const PS_SOLID: u32 = 0;
const DFC_BUTTON: u32 = 4;
const DFCS_BUTTONCHECK: u32 = 0;
const DFCS_CHECKED: u32 = 0x0400;
const DFCS_FLAT: u32 = 0x4000;
const IDC_HAND: u32 = 32649;

/// The scroll bars showing (`status`).
const TV_HSCROLL: u32 = 0x01;
const TV_VSCROLL: u32 = 0x02;

const TV_EDIT_TIMER: u32 = 2;
const TEXT_CALLBACK_SIZE: u32 = 260;
const MINIMUM_INDENT: u32 = 19;
const TVHEIGHT_MIN: i32 = 16;
const TVHEIGHT_FONT_ADJUST: i32 = 3;
const KEY_DELAY: u32 = 450;
/// The handle of the first item; the next ones follow it.
const FIRST_ITEM: u32 = 0x0080_0010;
/// The bytes between items' handles, as if they were blocks of a heap.
const ITEM_STRIDE: u32 = 0x40;

/// The scratch space of the control's scripts: a notification, two
/// labels for it, a SCROLLINFO, a PAINTSTRUCT, a class name and the
/// label of an edit control.
const S_NM: u32 = 0;
const S_TEXT1: u32 = 0x80;
const S_TEXT2: u32 = 0x188;
const TEXT_BUF: u32 = 0x108;
const S_SI: u32 = 0x290;
const S_PS: u32 = 0x2B0;
const S_CLASS: u32 = 0x2F0;
const S_EDIT: u32 = 0x308;
const EDIT_BUF: u32 = 1024;
const S_SIZE: u32 = S_EDIT + EDIT_BUF;

/// The continuation that runs the steps.
const T_RUN: u16 = 1;

/// An item of a tree.
#[derive(Clone, Debug, Default)]
pub struct Item {
    /// The fields the parent gives through TVN_GETDISPINFO.
    pub callback: u32,
    pub state: u32,
    /// The label; None for an item without one.
    pub text: Option<Vec<u8>>,
    pub image: i32,
    pub selected_image: i32,
    pub children: i32,
    pub lparam: u32,
    /// The height in rows.
    pub integral: i32,
    /// 0 for the items at the root.
    pub level: i32,
    pub parent: u32,
    pub first_child: u32,
    pub last_child: u32,
    pub prev: u32,
    pub next: u32,
    pub rect: Rect,
    pub lines_offset: i32,
    pub state_offset: i32,
    pub image_offset: i32,
    pub text_offset: i32,
    /// 0 until measured.
    pub text_width: i32,
    /// The row of the item among those showing, or -1 when its parent
    /// is collapsed.
    pub visible_order: i32,
}

crate::state_fields!(Item {
    callback,
    state,
    text,
    image,
    selected_image,
    children,
    lparam,
    integral,
    level,
    parent,
    first_child,
    last_child,
    prev,
    next,
    rect,
    lines_offset,
    state_offset,
    image_offset,
    text_offset,
    text_width,
    visible_order
});

/// What a tree view keeps.
#[derive(Clone, Debug, Default)]
pub struct Tree {
    pub hwnd: u32,
    pub notify: u32,
    pub style: u32,
    pub items: BTreeMap<u32, Item>,
    /// The hidden item whose children are the items at the root.
    pub root: u32,
    pub status: u32,
    pub edit_timer: bool,
    pub count: u32,
    pub cdmode: u32,
    pub scroll_time: u32,
    pub redraw: bool,
    pub item_height: i32,
    pub height_set: bool,
    pub client_width: i32,
    pub client_height: i32,
    pub tree_width: i32,
    pub tree_height: i32,
    pub indent: u32,
    pub selected: u32,
    pub hot: u32,
    pub focused: u32,
    pub edit_item: u32,
    pub first_visible: u32,
    pub max_visible_order: i32,
    pub drop_item: u32,
    pub insert_mark: u32,
    pub insert_after: bool,
    pub scroll_x: i32,
    pub clr_bk: u32,
    pub clr_text: u32,
    pub clr_line: u32,
    pub clr_insert: u32,
    pub font: u32,
    pub default_font: u32,
    pub bold_font: u32,
    pub underline_font: u32,
    pub tooltip: u32,
    pub edit: u32,
    pub ignore_kill: bool,
    pub label_changed: bool,
    pub unicode: bool,
    pub himl_normal: u32,
    pub normal_width: i32,
    pub normal_height: i32,
    pub himl_state: u32,
    pub state_width: i32,
    pub state_height: i32,
    /// A press that may become a drag: the button (1 left, 2 right),
    /// the item and hit flags, whether it would edit, and where.
    pub press: u32,
    pub press_item: u32,
    pub press_flags: u32,
    pub press_edit: bool,
    pub press_x: i32,
    pub press_y: i32,
    /// TVM_SORTCHILDRENCB under way: the parent, the program's function
    /// and its value, the children, those sorted so far, and the
    /// search for the place of the next.
    pub sort_parent: u32,
    pub sort_func: u32,
    pub sort_param: u32,
    pub sort_items: Vec<u32>,
    pub sort_done: Vec<u32>,
    pub sort_lo: u32,
    pub sort_hi: u32,
    /// Typing an item's label to find it.
    pub last_key: u32,
    pub char_code: u32,
    pub search: Vec<u8>,
}

crate::state_fields!(Tree {
    hwnd,
    notify,
    style,
    items,
    root,
    status,
    edit_timer,
    count,
    cdmode,
    scroll_time,
    redraw,
    item_height,
    height_set,
    client_width,
    client_height,
    tree_width,
    tree_height,
    indent,
    selected,
    hot,
    focused,
    edit_item,
    first_visible,
    max_visible_order,
    drop_item,
    insert_mark,
    insert_after,
    scroll_x,
    clr_bk,
    clr_text,
    clr_line,
    clr_insert,
    font,
    default_font,
    bold_font,
    underline_font,
    tooltip,
    edit,
    ignore_kill,
    label_changed,
    unicode,
    himl_normal,
    normal_width,
    normal_height,
    himl_state,
    state_width,
    state_height,
    press,
    press_item,
    press_flags,
    press_edit,
    press_x,
    press_y,
    sort_parent,
    sort_func,
    sort_param,
    sort_items,
    sort_done,
    sort_lo,
    sort_hi,
    last_key,
    char_code,
    search
});

/// An edit control a tree subclassed to edit a label: its tree and its
/// own procedure.
#[derive(Clone, Debug, Default)]
pub struct Edit {
    pub tree: u32,
    pub proc: u32,
}

crate::state_fields!(Edit { tree, proc });

/// What the module keeps, in COMCTL32's `State`.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub trees: BTreeMap<u32, Tree>,
    pub edits: BTreeMap<u32, Edit>,
    /// The handle the next item gets.
    pub next_item: u32,
}

crate::state_fields!(State {
    trees,
    edits,
    next_item
});

fn state_image_index(state: u32) -> u32 {
    (state >> 12) & 0x0F
}

// The tree's structure.

impl Tree {
    fn item(&self, h: u32) -> &Item {
        &self.items[&h]
    }

    fn item_mut(&mut self, h: u32) -> &mut Item {
        self.items.get_mut(&h).expect("an item of the tree")
    }

    /// Whether `h` is an item of this tree (TREEVIEW_ValidItem).
    fn valid(&self, h: u32) -> bool {
        h != 0 && self.items.contains_key(&h)
    }

    /// Whether `h` is an item the program may name: not the root.
    fn user_item(&self, h: u32) -> bool {
        self.valid(h) && h != self.root
    }

    fn shown(&self, h: u32) -> bool {
        self.items.get(&h).is_some_and(|i| i.visible_order >= 0)
    }

    fn expanded(&self, h: u32) -> bool {
        self.item(h).state & TVIS_EXPANDED != 0
    }

    /// The last item showing in `h`'s subtree (TREEVIEW_GetLastListItem).
    fn last_list(&self, h: u32) -> u32 {
        if h == 0 {
            return 0;
        }
        let mut h = h;
        while self.item(h).last_child != 0 && self.expanded(h) {
            h = self.item(h).last_child;
        }
        if h == self.root { 0 } else { h }
    }

    /// The item showing before `h` (TREEVIEW_GetPrevListItem).
    fn prev_list(&self, h: u32) -> u32 {
        let i = self.item(h);
        if i.prev != 0 {
            let up = self.item(i.prev);
            if up.state & TVIS_EXPANDED != 0 && up.last_child != 0 {
                return self.last_list(up.last_child);
            }
            return i.prev;
        }
        if i.parent != self.root { i.parent } else { 0 }
    }

    /// The item showing after `h` (TREEVIEW_GetNextListItem).
    fn next_list(&self, h: u32) -> u32 {
        let i = self.item(h);
        if i.state & TVIS_EXPANDED != 0 && i.first_child != 0 {
            return i.first_child;
        }
        if i.next != 0 {
            return i.next;
        }
        let mut p = i.parent;
        while p != 0 {
            let pi = self.item(p);
            if pi.next != 0 {
                return pi.next;
            }
            p = pi.parent;
        }
        0
    }

    /// The item `count` places after `h` (before for a negative count),
    /// or the last one there is (TREEVIEW_GetListItem).
    fn list_item(&self, h: u32, count: i32) -> u32 {
        if count == 0 || h == 0 {
            return h;
        }
        let mut h = h;
        let mut prev = h;
        for _ in 0..count.unsigned_abs() {
            if h == 0 {
                break;
            }
            prev = h;
            h = if count > 0 {
                self.next_list(h)
            } else {
                self.prev_list(h)
            };
        }
        if h != 0 { h } else { prev }
    }

    /// Whether `child` is under `parent` (an item isn't under itself).
    fn is_child_of(&self, parent: u32, child: u32) -> bool {
        let mut c = child;
        while c != 0 {
            c = self.items.get(&c).map_or(0, |i| i.parent);
            if c == parent {
                return true;
            }
        }
        false
    }

    fn visible_count(&self) -> i32 {
        if self.item_height <= 0 {
            return 0;
        }
        self.client_height / self.item_height
    }

    fn first_child(&self) -> u32 {
        self.item(self.root).first_child
    }

    fn order(&self, h: u32) -> i32 {
        self.items.get(&h).map_or(0, |i| i.visible_order)
    }

    /// The offsets of an item's parts
    /// (TREEVIEW_ComputeItemInternalMetrics).
    fn compute_metrics(&mut self, h: u32) {
        let lar =
            (self.style & (TVS_LINESATROOT | TVS_HASLINES | TVS_HASBUTTONS)) > TVS_LINESATROOT;
        let indent = self.indent as i32;
        let (scroll_x, state_w, normal_w) = (self.scroll_x, self.state_width, self.normal_width);
        let i = self.item_mut(h);
        let level = if lar { i.level } else { i.level - 1 };
        i.lines_offset = indent * level - scroll_x;
        i.state_offset = i.lines_offset + indent;
        i.image_offset = i.state_offset
            + if state_image_index(i.state) != 0 {
                state_w
            } else {
                0
            };
        i.text_offset = i.image_offset + normal_w;
    }

    fn compute_rect(&mut self, h: u32) {
        let first = self.order(self.first_visible);
        let (height, width) = (self.item_height, self.client_width);
        let i = self.item_mut(h);
        let top = height * (i.visible_order - first);
        i.rect = Rect::new(0, top, width, top + height * i.integral - 1);
    }

    /// Number the items showing from `start` on, or from the first
    /// (TREEVIEW_RecalculateVisibleOrder).
    fn recalc_order(&mut self, start: u32) {
        let (first, mut order) = if start == 0 || !self.valid(start) || start == self.root {
            (self.first_child(), 0)
        } else {
            (start, self.order(start).max(0))
        };
        let mut h = first;
        while h != 0 {
            if !self.shown(h) && order > 0 {
                self.compute_metrics(h);
            }
            let i = self.item_mut(h);
            i.visible_order = order;
            order += i.integral;
            h = self.next_list(h);
        }
        self.max_visible_order = order;
        let mut h = self.first_child();
        while h != 0 {
            self.compute_rect(h);
            h = self.next_list(h);
        }
    }

    /// The font of an item's label.
    fn font_for(&self, h: u32) -> u32 {
        if self.style & TVS_TRACKSELECT != 0 && h == self.hot {
            return self.underline_font;
        }
        if self.items.get(&h).is_some_and(|i| i.state & TVIS_BOLD != 0) {
            return self.bold_font;
        }
        self.font
    }

    /// Clear what refers to `h`, which goes (TREEVIEW_FreeItem).
    fn forget(&mut self, h: u32) {
        self.items.remove(&h);
        for r in [
            &mut self.selected,
            &mut self.hot,
            &mut self.focused,
            &mut self.first_visible,
            &mut self.drop_item,
            &mut self.insert_mark,
            &mut self.edit_item,
        ] {
            if *r == h {
                *r = 0;
            }
        }
    }

    /// Take `h` out of its parent's list of children
    /// (TREEVIEW_UnlinkItem).
    fn unlink(&mut self, h: u32) {
        let (parent, prev, next) = {
            let i = self.item(h);
            (i.parent, i.prev, i.next)
        };
        let p = self.item_mut(parent);
        if p.first_child == h {
            p.first_child = next;
        }
        if p.last_child == h {
            p.last_child = prev;
        }
        if prev != 0 {
            self.item_mut(prev).next = next;
        }
        if next != 0 {
            self.item_mut(next).prev = prev;
        }
    }

    /// Remove `h` and its subtree (TREEVIEW_RemoveItem, without the
    /// notifications, which have gone already).
    fn remove(&mut self, h: u32) {
        self.remove_children(h);
        self.unlink(h);
        self.count = self.count.saturating_sub(1);
        self.forget(h);
    }

    fn remove_children(&mut self, h: u32) {
        let mut child = self.item(h).first_child;
        while child != 0 {
            let next = self.item(child).next;
            self.remove(child);
            child = next;
        }
    }

    /// `h` and its subtree, the order in which they go.
    fn subtree(&self, h: u32, out: &mut Vec<u32>) {
        out.push(h);
        let mut child = self.item(h).first_child;
        while child != 0 {
            self.subtree(child, out);
            child = self.item(child).next;
        }
    }

    fn insert_before(&mut self, new: u32, sibling: u32, parent: u32) {
        if sibling != 0 {
            let prev = self.item(sibling).prev;
            if prev != 0 {
                self.item_mut(prev).next = new;
            }
            self.item_mut(new).prev = prev;
            self.item_mut(sibling).prev = new;
        } else {
            self.item_mut(new).prev = 0;
        }
        self.item_mut(new).next = sibling;
        let p = self.item_mut(parent);
        if p.first_child == sibling {
            p.first_child = new;
        }
        if p.last_child == 0 {
            p.last_child = new;
        }
    }

    fn insert_after_item(&mut self, new: u32, sibling: u32, parent: u32) {
        if sibling != 0 {
            let next = self.item(sibling).next;
            if next != 0 {
                self.item_mut(next).prev = new;
            }
            self.item_mut(new).next = next;
            self.item_mut(sibling).next = new;
        } else {
            self.item_mut(new).next = 0;
        }
        self.item_mut(new).prev = sibling;
        let p = self.item_mut(parent);
        if p.last_child == sibling {
            p.last_child = new;
        }
        if p.first_child == 0 {
            p.first_child = new;
        }
    }

    fn hit_test_point(&self, y: i32) -> u32 {
        if !self.valid(self.first_visible) || self.item_height <= 0 {
            return 0;
        }
        let row = y.div_euclid(self.item_height) + self.order(self.first_visible);
        let mut h = self.first_visible;
        while h != 0 {
            let i = self.item(h);
            if row >= i.visible_order && row < i.visible_order + i.integral {
                return h;
            }
            h = self.next_list(h);
        }
        0
    }

    fn colors(&self, cpu: &mut Cpu) -> (u32, u32) {
        let bk = if self.clr_bk == CLR_NONE {
            gfx::sys_color(cpu, COLOR_WINDOW)
        } else {
            self.clr_bk
        };
        let text = if self.clr_text == CLR_NONE {
            gfx::sys_color(cpu, COLOR_WINDOWTEXT)
        } else {
            self.clr_text
        };
        (bk, text)
    }
}

// Taking a tree out of the state while working on it.

fn take(cpu: &mut Cpu, hwnd: u32) -> Option<Tree> {
    state(cpu).treeview.trees.remove(&hwnd)
}

fn put(cpu: &mut Cpu, t: Tree) {
    state(cpu).treeview.trees.insert(t.hwnd, t);
}

/// Run `f` on the tree of `hwnd`, if it has one.
fn with<R>(cpu: &mut Cpu, hwnd: u32, f: impl FnOnce(&mut Cpu, &mut Tree) -> R) -> Option<R> {
    let mut t = take(cpu, hwnd)?;
    let r = f(cpu, &mut t);
    put(cpu, t);
    Some(r)
}

/// Read the tree of `hwnd`.
fn peek<R>(cpu: &mut Cpu, hwnd: u32, f: impl FnOnce(&Tree) -> R) -> Option<R> {
    state(cpu).treeview.trees.get(&hwnd).map(f)
}

// Measuring.

fn get_dc(cpu: &mut Cpu, hwnd: u32) -> u32 {
    user32::paint::get_dc(cpu, &[hwnd]).unwrap_or(0)
}

fn release_dc(cpu: &mut Cpu, hwnd: u32, hdc: u32) {
    user32::paint::release_dc(cpu, &[hwnd, hdc]);
}

/// The width of `h`'s label in its font (TREEVIEW_ComputeTextWidth),
/// on `hdc` or on a DC of the tree's own.
fn measure(cpu: &mut Cpu, t: &mut Tree, h: u32, hdc: u32) {
    let Some(text) = t.item(h).text.clone() else {
        t.item_mut(h).text_width = 0;
        return;
    };
    let own = hdc == 0;
    let hdc = if own { get_dc(cpu, t.hwnd) } else { hdc };
    if hdc == 0 {
        return;
    }
    let old = if own {
        gfx::select(cpu, hdc, t.font_for(h))
    } else {
        0
    };
    let (w, _) = gfx::text_size(cpu, hdc, &text);
    t.item_mut(h).text_width = w;
    if own {
        gfx::select(cpu, hdc, old);
        release_dc(cpu, t.hwnd, hdc);
    }
}

/// The metrics and label widths of the items showing under `root`
/// (TREEVIEW_UpdateSubTree).
fn update_subtree(cpu: &mut Cpu, t: &mut Tree, root: u32) {
    if !t.valid(root) || t.item(root).first_child == 0 || !t.expanded(root) {
        return;
    }
    t.item_mut(root).state &= !TVIS_EXPANDED;
    let sibling = t.next_list(root);
    t.item_mut(root).state |= TVIS_EXPANDED;
    let hdc = get_dc(cpu, t.hwnd);
    let old = gfx::select(cpu, hdc, t.font);
    let mut h = if root == t.root {
        t.first_child()
    } else {
        root
    };
    while h != sibling && h != 0 {
        t.compute_metrics(h);
        if t.item(h).text_width == 0 {
            gfx::select(cpu, hdc, t.font_for(h));
            measure(cpu, t, h, hdc);
        }
        h = t.next_list(h);
    }
    gfx::select(cpu, hdc, old);
    release_dc(cpu, t.hwnd, hdc);
}

/// The height of items for the font and the images
/// (TREEVIEW_NaturalHeight).
fn natural_height(cpu: &mut Cpu, t: &Tree) -> i32 {
    let hdc = get_dc(cpu, 0);
    let old = gfx::select(cpu, hdc, t.font);
    let text = gfx::font_height(cpu, hdc);
    gfx::select(cpu, hdc, old);
    release_dc(cpu, 0, hdc);
    let mut height = TVHEIGHT_MIN
        .max(text + TVHEIGHT_FONT_ADJUST)
        .max(t.normal_height);
    if t.style & TVS_NONEVENHEIGHT == 0 {
        height &= !1;
    }
    height
}

/// The widest character of `font` and the width of `text` in it.
fn text_extent(cpu: &mut Cpu, hwnd: u32, font: u32, text: &[u8]) -> (i32, i32) {
    let hdc = get_dc(cpu, hwnd);
    let old = gfx::select(cpu, hdc, font);
    let (w, _) = gfx::text_size(cpu, hdc, text);
    let max = gdi32::font::font_of(cpu, hdc).map_or(8, |(_, f)| f.max_width());
    gfx::select(cpu, hdc, old);
    release_dc(cpu, hwnd, hdc);
    (max, w)
}

/// A font like `font` in bold, or underlined.
fn font_like(cpu: &mut Cpu, font: u32, bold: bool) -> u32 {
    let mut log = gdi32::gdi(cpu)
        .font(font)
        .map(|f| f.log.clone())
        .unwrap_or_else(|| user32::sysparams::sans_serif(false));
    if log.len() < 60 {
        log.resize(60, 0);
    }
    if bold {
        log[16..20].copy_from_slice(&700i32.to_le_bytes());
    } else {
        log[21] = 1;
    }
    gdi32::add(
        cpu,
        gdi32::Kind::Font {
            font: gdi32::Font { log },
        },
    )
}

// What the program is called for, made into ops.

fn call(name: &str, args: &[Arg]) -> Op {
    Op::Call {
        target: user32::address_of(name),
        args: args.to_vec(),
    }
}

fn user(name: &str, args: &[u32]) -> Op {
    call(name, &args.iter().map(|&a| Arg::V(a)).collect::<Vec<_>>())
}

fn send(hwnd: u32, msg: u32, wp: u32, lp: Arg) -> Op {
    call("SendMessageA", &[Arg::V(hwnd), Arg::V(msg), Arg::V(wp), lp])
}

fn write(at: u32, words: Vec<u32>) -> Op {
    Op::Write { at, words }
}

fn then_steps(steps: &[Step]) -> Op {
    Op::Then {
        id: T_RUN,
        data: steps.iter().flat_map(|s| s.iter().copied()).collect(),
    }
}

/// A string and its 0, cut to fit `size` bytes, as words.
fn text_words(text: &[u8], size: u32) -> Vec<u32> {
    let n = text.len().min(size as usize - 1);
    let mut b = text[..n].to_vec();
    b.push(0);
    script::words_of(&b)
}

/// WM_NOTIFY to the tree's parent with the NMHDR of `code` and `body`
/// after it.
fn notify(cpu: &mut Cpu, t: &Tree, code: u32, body: &[u32]) -> Vec<Op> {
    let id = super::id_of(cpu, t.hwnd);
    let mut words = vec![t.hwnd, id, code];
    words.extend_from_slice(body);
    vec![
        write(S_NM, words),
        send(t.notify, WM_NOTIFY, id, Arg::S(S_NM)),
    ]
}

/// A TVITEM for a notification (TREEVIEW_TVItemFromItem), with its
/// label at `text` when the mask has TVIF_TEXT.
fn tv_item(t: &Tree, h: u32, mask: u32, text: u32) -> [u32; 10] {
    if !t.valid(h) {
        return [0; 10];
    }
    let i = t.item(h);
    let (psz, cch) = if mask & TVIF_TEXT != 0 {
        let len = i
            .text
            .as_ref()
            .map_or(0, |s| s.len().min(TEXT_BUF as usize - 1) as u32 + 1);
        (text, len)
    } else {
        (0, 0)
    };
    [
        mask,
        h,
        i.state,
        0xFFFF,
        psz,
        cch,
        i.image as u32,
        i.selected_image as u32,
        i.children as u32,
        i.lparam,
    ]
}

/// TVN_* with an NMTREEVIEW of `old` and `new`
/// (TREEVIEW_SendTreeviewNotify). Labels need the scratch space's
/// address.
#[allow(clippy::too_many_arguments)]
fn tv_notify(
    cpu: &mut Cpu,
    t: &Tree,
    code: u32,
    action: u32,
    mask: u32,
    old: u32,
    new: u32,
    scratch: u32,
) -> Vec<Op> {
    let mut ops = Vec::new();
    let label = |h: u32| {
        t.items
            .get(&h)
            .and_then(|i| i.text.clone())
            .unwrap_or_default()
    };
    if mask & TVIF_TEXT != 0 {
        ops.push(write(S_TEXT1, text_words(&label(old), TEXT_BUF)));
        ops.push(write(S_TEXT2, text_words(&label(new), TEXT_BUF)));
    }
    let mut body = vec![action];
    body.extend(tv_item(t, old, mask, scratch + S_TEXT1));
    body.extend(tv_item(t, new, mask, scratch + S_TEXT2));
    body.extend([0, 0]);
    ops.extend(notify(cpu, t, code, &body));
    ops
}

// Scrolling.

/// The tree's size and its scroll bars (TREEVIEW_UpdateScrollBars).
fn update_scroll_bars(cpu: &mut Cpu, t: &mut Tree, ops: &mut Vec<Op>) {
    t.tree_width = 0;
    t.tree_height = 0;
    let mut h = t.first_child();
    while h != 0 {
        let i = t.item(h);
        if i.visible_order >= 0 {
            let (right, bottom) = (i.text_offset + i.text_width + 2, i.rect.bottom);
            t.tree_width = t.tree_width.max(right);
            t.tree_height = bottom;
        }
        h = t.next_list(h);
    }
    let first = t.first_child();
    if first != 0 {
        t.tree_height -= t.item(first).rect.top;
    }
    t.tree_width += t.scroll_x;
    if t.style & TVS_NOSCROLL != 0 {
        return;
    }
    let metric = |cpu: &mut Cpu, n: u32| {
        user32::sysparams::get_system_metrics(cpu, &[n]).unwrap_or(0) as i32
    };
    let mut vert = false;
    let mut horz = false;
    if t.tree_height > t.client_height {
        vert = true;
        if t.tree_width > t.client_width - metric(cpu, SM_CXVSCROLL) {
            horz = true;
        }
    } else if t.tree_width > t.client_width || t.scroll_x > 0 {
        horz = true;
    }
    if !vert && horz && t.tree_height > t.client_height - metric(cpu, SM_CYHSCROLL) {
        vert = true;
    }
    if horz && t.style & TVS_NOHSCROLL != 0 {
        horz = false;
    }
    let hwnd = t.hwnd;
    let set_info = |bar: u32, page: i32, pos: i32, max: i32| {
        let mask = SIF_POS | SIF_RANGE | SIF_PAGE;
        vec![
            write(
                S_SI,
                vec![28, mask, 0, max as u32, page as u32, pos as u32, 0],
            ),
            call(
                "SetScrollInfo",
                &[Arg::V(hwnd), Arg::V(bar), Arg::S(S_SI), Arg::V(1)],
            ),
        ]
    };
    if vert && t.visible_count() != 0 && t.valid(t.first_visible) {
        let pos = t.order(t.first_visible);
        ops.extend(set_info(
            SB_VERT,
            t.visible_count(),
            pos,
            t.max_visible_order - 1,
        ));
        if t.status & TV_VSCROLL == 0 {
            ops.push(user("ShowScrollBar", &[hwnd, SB_VERT, 1]));
        }
        t.status |= TV_VSCROLL;
    } else {
        if t.status & TV_VSCROLL != 0 {
            ops.push(user("ShowScrollBar", &[hwnd, SB_VERT, 0]));
        }
        t.status &= !TV_VSCROLL;
    }
    if horz {
        let page = t.client_width;
        let max = t.tree_width - 1;
        let pos = t.scroll_x.min(max - (page - 1).max(0)).max(0);
        if t.status & TV_HSCROLL == 0 {
            ops.push(user("ShowScrollBar", &[hwnd, SB_HORZ, 1]));
        }
        t.status |= TV_HSCROLL;
        ops.extend(set_info(SB_HORZ, page, pos, max));
        hscroll(cpu, t, SB_THUMBPOSITION, pos, ops);
    } else {
        if t.status & TV_HSCROLL != 0 {
            ops.push(user("ShowScrollBar", &[hwnd, SB_HORZ, 0]));
        }
        t.status &= !TV_HSCROLL;
    }
}

/// WM_HSCROLL's work (TREEVIEW_HScroll); `pos` is SB_THUMBPOSITION's.
fn hscroll(cpu: &mut Cpu, t: &mut Tree, code: u32, pos: i32, ops: &mut Vec<Op>) {
    if t.status & TV_HSCROLL == 0 {
        return;
    }
    let max = t.tree_width - t.client_width;
    let mut x = t.scroll_x;
    if max <= 0 {
        x = 0;
    } else {
        match code {
            SB_LINEUP => x -= t.item_height,
            SB_LINEDOWN => x += t.item_height,
            SB_PAGEUP => x -= t.client_width,
            SB_PAGEDOWN => x += t.client_width,
            SB_THUMBTRACK | SB_THUMBPOSITION => x = pos,
            SB_ENDSCROLL => return,
            _ => {}
        }
        x = x.clamp(0, max);
    }
    if x != t.scroll_x {
        let delta = t.scroll_x - x;
        let mut h = t.first_child();
        while h != 0 {
            let i = t.item_mut(h);
            i.lines_offset += delta;
            i.state_offset += delta;
            i.image_offset += delta;
            i.text_offset += delta;
            h = t.next_list(h);
        }
        t.scroll_x = x;
        invalidate(cpu, t.hwnd, true);
    }
    if code != SB_THUMBTRACK {
        ops.push(user("SetScrollPos", &[t.hwnd, SB_HORZ, x as u32, 1]));
    }
}

/// Make `new` the first item showing (TREEVIEW_SetFirstVisible).
fn set_first_visible(cpu: &mut Cpu, t: &mut Tree, new: u32, update_pos: bool, ops: &mut Vec<Op>) {
    let mut new = new;
    if t.valid(new) {
        let gap = t.visible_count() - t.max_visible_order + t.order(new);
        if gap > 0 {
            new = t.list_item(new, -gap);
            if new == 0 {
                new = t.first_child();
            }
        }
    }
    if t.first_visible == new {
        return;
    }
    if !t.valid(t.first_visible) || !t.valid(new) {
        t.first_visible = new;
        invalidate(cpu, t.hwnd, true);
        return;
    }
    let scroll = t.item_height * (t.order(t.first_visible) - t.order(new));
    t.first_visible = new;
    let mut h = t.first_child();
    while h != 0 {
        let i = t.item_mut(h);
        i.rect = i.rect.offset(0, scroll);
        h = t.next_list(h);
    }
    if update_pos {
        let pos = t.order(new) as u32;
        ops.push(user("SetScrollPos", &[t.hwnd, SB_VERT, pos, 1]));
    }
    invalidate(cpu, t.hwnd, true);
}

/// WM_VSCROLL's work (TREEVIEW_VScroll).
fn vscroll(cpu: &mut Cpu, t: &mut Tree, code: u32, pos: i32, ops: &mut Vec<Op>) {
    if t.status & TV_VSCROLL == 0 || !t.valid(t.first_visible) {
        return;
    }
    let old = t.first_visible;
    let page = t.visible_count().max(1);
    let new = match code {
        SB_TOP => t.first_child(),
        SB_BOTTOM => t.last_list(t.root),
        SB_LINEUP => t.prev_list(old),
        SB_LINEDOWN => t.next_list(old),
        SB_PAGEUP => t.list_item(old, -page),
        SB_PAGEDOWN => t.list_item(old, page),
        SB_THUMBTRACK | SB_THUMBPOSITION => t.list_item(t.first_child(), pos),
        _ => 0,
    };
    if new == 0 {
        return;
    }
    if new != old {
        set_first_visible(cpu, t, new, code != SB_THUMBTRACK, ops);
    } else if code == SB_THUMBPOSITION {
        let pos = t.order(new) as u32;
        ops.push(user("SetScrollPos", &[t.hwnd, SB_VERT, pos, 1]));
    }
}

// The steps.

const STEP: usize = 6;
type Step = [u32; STEP];

fn st(tag: u32, args: &[u32]) -> Step {
    let mut s = [0; STEP];
    s[0] = tag;
    s[1..1 + args.len()].copy_from_slice(args);
    s
}

// Expanding: the item and flags (EX_*).
const W_EXPAND: u32 = 1;
const W_EXPAND_ASKED: u32 = 2;
const W_EXPAND_TOLD: u32 = 3;
const W_EXPAND_SHOW: u32 = 4;
// Collapsing: the item and flags.
const W_COLLAPSE: u32 = 5;
const W_COLLAPSE_ASKED: u32 = 6;
const W_COLLAPSE_TOLD: u32 = 7;
const W_COLLAPSE_DONE: u32 = 8;
// Selecting: the action, the item, the cause and whether it returns.
const W_SELECT: u32 = 9;
const W_SELECT_ASKED: u32 = 10;
const W_SELECT_TELL: u32 = 11;
// Showing an item: it, whether to scroll across, whether it returns.
const W_ENSURE: u32 = 12;
const W_ENSURE_SCROLL: u32 = 13;
// TVN_GETDISPINFO: the item and the fields.
const W_DISPINFO: u32 = 14;
const W_DISPINFO_GOT: u32 = 15;
// TVM_GETITEMA: the TVITEM.
const W_GETITEM: u32 = 16;
// Deleting.
const W_DELETE: u32 = 17;
const W_DELETE_GONE: u32 = 18;
const W_DELETE_SHOW: u32 = 19;
// Editing a label.
const W_EDIT: u32 = 20;
const W_EDIT_CREATE: u32 = 21;
const W_EDIT_CREATED: u32 = 22;
const W_EDIT_BEGUN: u32 = 23;
const W_END_EDIT: u32 = 24;
const W_END_EDIT_TEXT: u32 = 25;
const W_END_EDIT_TOLD: u32 = 26;
const W_IGNORE_KILL: u32 = 27;
const W_EDIT_RESIZE: u32 = 28;
// Painting: the DC, then the rectangle.
const W_PAINT_BEGIN: u32 = 29;
const W_PAINT: u32 = 30;
const W_PAINT_ITEMS: u32 = 31;
const W_DRAW_ITEM: u32 = 32;
const W_DRAW_ITEM_ASKED: u32 = 33;
const W_PAINT_END: u32 = 34;
const W_END_PAINT: u32 = 35;
// The mouse.
const W_CLICK: u32 = 36;
const W_CLICK_ASKED: u32 = 37;
const W_PRESS: u32 = 38;
const W_RELEASE: u32 = 39;
const W_RELEASE_ASKED: u32 = 40;
const W_SET_FOCUS: u32 = 41;
const W_DBLCLK_ASKED: u32 = 42;
const W_TOGGLE: u32 = 43;
const W_EXPAND_ALL: u32 = 44;
const W_FOCUS_TELL: u32 = 45;
// TVM_SORTCHILDRENCB's comparisons.
const W_SORT: u32 = 46;
const W_SORT_GOT: u32 = 47;
const W_FIRST_VISIBLE: u32 = 48;
const W_NOTIFY_FORMAT: u32 = 49;
const W_SETCURSOR_ASKED: u32 = 50;
const W_RESULT: u32 = 51;
const W_RCLICK: u32 = 52;
const W_KEY: u32 = 53;
const W_TOOLTIP: u32 = 54;

/// Expanding's and collapsing's flags.
const EX_USER: u32 = 2;
const EX_RET: u32 = 4;
const EX_RESET: u32 = 8;
const EX_NOTIFY: u32 = 16;

/// What the steps run with.
struct Run {
    /// The scratch space's address, once the script runs in a task.
    scratch: Option<u32>,
    /// What the call before returned.
    last: u32,
    /// What the message returns.
    result: u32,
}

/// What a step did.
#[derive(Default)]
struct Out {
    /// Calls to make before the steps go on.
    ops: Vec<Op>,
    /// Steps to run next, before the rest.
    push: Vec<Step>,
    /// The step needs the scratch space's address: run it in a task.
    defer: bool,
}

fn encode(work: &VecDeque<Step>) -> Vec<u32> {
    work.iter().flat_map(|s| s.iter().copied()).collect()
}

fn decode(data: &[u32]) -> VecDeque<Step> {
    data.chunks(STEP)
        .filter(|c| c.len() == STEP)
        .map(|c| {
            let mut s = [0; STEP];
            s.copy_from_slice(c);
            s
        })
        .collect()
}

/// Run the steps until one calls into the program: the ops to run, with
/// the rest of the steps in a continuation after them.
fn drive(cpu: &mut Cpu, hwnd: u32, steps: VecDeque<Step>, r: &mut Run) -> Vec<Op> {
    let mut work = steps;
    while let Some(w) = work.pop_front() {
        let mut out = Out::default();
        step(cpu, hwnd, w, r, &mut out);
        if out.defer {
            work.push_front(w);
            return vec![Op::Then {
                id: T_RUN,
                data: encode(&work),
            }];
        }
        for p in out.push.into_iter().rev() {
            work.push_front(p);
        }
        // What a step learns from its call comes in `last`; the steps
        // that make no call see 0 there.
        r.last = 0;
        if !out.ops.is_empty() {
            let mut ops = out.ops;
            if !work.is_empty() {
                ops.push(Op::Then {
                    id: T_RUN,
                    data: encode(&work),
                });
            }
            return ops;
        }
    }
    Vec::new()
}

fn new_script(hwnd: u32) -> Script {
    let mut s = Script::new(hwnd);
    s.alloc(S_SIZE);
    s
}

/// Do `steps` for the message, which returns `result` unless they say
/// otherwise.
fn start(cpu: &mut Cpu, hwnd: u32, steps: Vec<Step>, result: u32) -> Option<u32> {
    let mut r = Run {
        scratch: None,
        last: 0,
        result,
    };
    let ops = drive(cpu, hwnd, steps.into(), &mut r);
    let mut s = new_script(hwnd);
    s.ops = ops;
    s.result = r.result;
    script::run(cpu, TREEVIEW, s)
}

/// Make the calls `ops` and return `result`.
fn finish(cpu: &mut Cpu, hwnd: u32, ops: Vec<Op>, result: u32) -> Option<u32> {
    if ops.is_empty() {
        return Some(result);
    }
    let mut s = new_script(hwnd);
    s.ops = ops;
    s.result = result;
    script::run(cpu, TREEVIEW, s)
}

/// The continuations of its scripts.
pub fn then(cpu: &mut Cpu, id: u16, c: &mut Ctx) -> Vec<Op> {
    if id != T_RUN {
        return Vec::new();
    }
    let mut r = Run {
        scratch: Some(c.scratch),
        last: c.last,
        result: c.result,
    };
    let ops = drive(cpu, c.hwnd, decode(&c.data), &mut r);
    c.result = r.result;
    ops
}

fn step(cpu: &mut Cpu, hwnd: u32, w: Step, r: &mut Run, out: &mut Out) {
    let Some(mut t) = take(cpu, hwnd) else {
        // The tree went while its steps waited; the painting ones still
        // end their painting.
        if w[0] == W_END_PAINT {
            out.ops
                .push(call("EndPaint", &[Arg::V(hwnd), Arg::S(S_PS)]));
        }
        return;
    };
    step_on(cpu, &mut t, w, r, out);
    put(cpu, t);
}

fn step_on(cpu: &mut Cpu, t: &mut Tree, w: Step, r: &mut Run, out: &mut Out) {
    let [tag, a, b, c, d, e] = w;
    match tag {
        W_EXPAND => expand(cpu, t, a, b, r, out),
        W_EXPAND_ASKED => {
            if r.last != 0 || !t.valid(a) || t.item(a).first_child == 0 {
                if b & EX_RET != 0 {
                    r.result = 0;
                }
                return;
            }
            t.item_mut(a).state |= TVIS_EXPANDED;
            // The children show by the time the parent hears of it, so
            // that it can ask for their rectangles.
            if t.shown(a) {
                t.recalc_order(a);
                update_subtree(cpu, t, a);
            }
            out.ops = tv_notify(cpu, t, TVN_ITEMEXPANDEDA, TVE_EXPAND, EXPAND_MASK, 0, a, 0);
            out.push.push(st(W_EXPAND_TOLD, &[a, b]));
        }
        W_EXPAND_TOLD => {
            if t.valid(a) {
                t.item_mut(a).state |= TVIS_EXPANDEDONCE;
                out.push.push(st(W_EXPAND_SHOW, &[a, b]));
            }
        }
        W_EXPAND_SHOW => expand_show(cpu, t, a, b, r, out),
        W_COLLAPSE => {
            if !t.valid(a) || !t.expanded(a) {
                if b & EX_RET != 0 {
                    r.result = 0;
                }
                return;
            }
            let once = t.item(a).state & TVIS_EXPANDEDONCE != 0;
            let flags = if b & EX_USER != 0 || !once {
                b | EX_NOTIFY
            } else {
                b
            };
            if flags & EX_NOTIFY != 0 {
                let action = collapse_action(flags);
                out.ops = tv_notify(cpu, t, TVN_ITEMEXPANDINGA, action, EXPAND_MASK, 0, a, 0);
            }
            out.push.push(st(W_COLLAPSE_ASKED, &[a, flags]));
        }
        W_COLLAPSE_ASKED => {
            if !t.valid(a) {
                return;
            }
            if t.item(a).first_child == 0 {
                if b & EX_RET != 0 {
                    r.result = 0;
                }
                return;
            }
            t.item_mut(a).state &= !TVIS_EXPANDED;
            if b & EX_NOTIFY != 0 {
                let action = collapse_action(b);
                out.ops = tv_notify(cpu, t, TVN_ITEMEXPANDEDA, action, EXPAND_MASK, 0, a, 0);
            }
            out.push.push(st(W_COLLAPSE_TOLD, &[a, b]));
        }
        W_COLLAPSE_TOLD => {
            if !t.valid(a) {
                return;
            }
            let set_sel = t.selected != 0 && t.is_child_of(a, t.selected);
            let set_first = t.first_visible != 0 && t.is_child_of(a, t.first_visible);
            if b & EX_RESET != 0 {
                t.item_mut(a).state &= !TVIS_EXPANDEDONCE;
                let mut gone = Vec::new();
                let mut child = t.item(a).first_child;
                while child != 0 {
                    t.subtree(child, &mut gone);
                    child = t.item(child).next;
                }
                for h in gone {
                    out.ops.extend(delete_notify(cpu, t, h));
                }
            }
            out.push.push(st(
                W_COLLAPSE_DONE,
                &[a, b, set_sel as u32, set_first as u32],
            ));
        }
        W_COLLAPSE_DONE => collapse_done(cpu, t, a, b, c != 0, d != 0, r, out),
        W_SELECT => select(cpu, t, a, b, c, d != 0, r, out),
        W_SELECT_ASKED => {
            if r.last != 0 {
                if d != 0 {
                    r.result = 0;
                }
                return;
            }
            let new = if t.valid(a) { a } else { 0 };
            let prev = if t.valid(b) { b } else { 0 };
            if prev != 0 {
                t.item_mut(prev).state &= !TVIS_SELECTED;
            }
            if new != 0 {
                t.item_mut(new).state |= TVIS_SELECTED;
            }
            t.selected = new;
            out.push.push(st(W_ENSURE, &[new, 0, 0]));
            out.push.push(st(W_SELECT_TELL, &[new, prev, c]));
        }
        W_SELECT_TELL => {
            let Some(scratch) = r.scratch else {
                out.defer = true;
                return;
            };
            invalidate(cpu, t.hwnd, true);
            let mask = TVIF_TEXT | TVIF_HANDLE | TVIF_STATE | TVIF_PARAM;
            out.ops = tv_notify(cpu, t, TVN_SELCHANGEDA, c, mask, b, a, scratch);
            // Windows paints the new selection before it returns.
            out.ops.push(user("UpdateWindow", &[t.hwnd]));
        }
        W_ENSURE => {
            if !t.user_item(a) {
                if c != 0 {
                    r.result = 0;
                }
                return;
            }
            if !t.shown(a) {
                let mut p = t.item(a).parent;
                while p != t.root && p != 0 {
                    out.push.push(st(W_EXPAND, &[p, 0]));
                    p = t.item(p).parent;
                }
                out.push.reverse();
            }
            out.push.push(st(W_ENSURE_SCROLL, &[a, b, c]));
        }
        W_ENSURE_SCROLL => ensure_scroll(cpu, t, a, b != 0, c != 0, r, out),
        W_DISPINFO => {
            if !t.valid(a) {
                return;
            }
            let mask = b & t.item(a).callback;
            if mask == 0 {
                return;
            }
            let Some(scratch) = r.scratch else {
                out.defer = true;
                return;
            };
            if mask & TVIF_TEXT != 0 {
                t.item_mut(a).text_width = 0;
            }
            let i = t.item(a);
            let text = i.text.clone().unwrap_or_default();
            out.ops.push(write(S_TEXT1, text_words(&text, TEXT_BUF)));
            let body = [
                mask,
                a,
                i.state,
                0,
                scratch + S_TEXT1,
                TEXT_CALLBACK_SIZE.min(TEXT_BUF),
                i.image as u32,
                i.selected_image as u32,
                i.children as u32,
                i.lparam,
            ];
            out.ops.extend(notify(cpu, t, TVN_GETDISPINFOA, &body));
            out.push.push(st(W_DISPINFO_GOT, &[a, mask]));
        }
        W_DISPINFO_GOT => {
            if let Some(scratch) = r.scratch
                && t.valid(a)
            {
                dispinfo_got(cpu, t, a, b, scratch);
            }
        }
        W_GETITEM => get_item(cpu, t, a, r),
        W_DELETE => delete(cpu, t, a, out),
        W_DELETE_GONE => delete_gone(t, a, b, c, d, e, out),
        W_DELETE_SHOW => {
            let (parent, first, prev, visible) = (a, b, c, d != 0);
            if !t.redraw {
                return;
            }
            if visible {
                let first = if t.valid(first) {
                    first
                } else {
                    t.first_child()
                };
                let prev = if t.valid(prev) { prev } else { 0 };
                t.recalc_order(prev);
                set_first_visible(cpu, t, first, true, &mut out.ops);
                update_scroll_bars(cpu, t, &mut out.ops);
                invalidate(cpu, t.hwnd, true);
            } else if t.shown(parent) || parent == t.root {
                invalidate(cpu, t.hwnd, true);
            }
        }
        W_EDIT => {
            if !t.user_item(a) {
                r.result = 0;
                return;
            }
            if t.edit != 0 {
                r.result = t.edit;
                return;
            }
            t.label_changed = false;
            out.push.push(st(W_ENSURE, &[a, 1, 0]));
            out.push.push(st(W_DISPINFO, &[a, TVIF_TEXT]));
            out.push.push(st(W_EDIT_CREATE, &[a]));
        }
        W_EDIT_CREATE => edit_create(cpu, t, a, r, out),
        W_EDIT_CREATED => edit_created(cpu, t, a, r, out),
        W_EDIT_BEGUN => {
            let edit = b;
            if r.last != 0 {
                if let Some(e) = state(cpu).treeview.edits.remove(&edit)
                    && let Some(w) = user32::user(cpu).window_mut(edit)
                {
                    w.proc = e.proc;
                }
                t.edit = 0;
                t.edit_item = 0;
                out.ops.push(user("DestroyWindow", &[edit]));
                r.result = 0;
                return;
            }
            out.ops.push(user("SetFocus", &[edit]));
            out.ops.push(send(edit, EM_SETSEL, 0, Arg::V(u32::MAX)));
            out.ops.push(user("ShowWindow", &[edit, SW_SHOW]));
            r.result = edit;
        }
        W_END_EDIT => {
            let edit = t.edit;
            if edit == 0 || !user32::user(cpu).is_window(edit) || !t.valid(t.edit_item) {
                t.edit = 0;
                t.edit_item = 0;
                if b != 0 {
                    r.result = 0;
                }
                return;
            }
            if a == 0 {
                let size = Arg::V(EDIT_BUF - 1);
                out.ops.push(write(S_EDIT, vec![0]));
                out.ops.push(call(
                    "GetWindowTextA",
                    &[Arg::V(edit), Arg::S(S_EDIT), size],
                ));
            }
            out.push.push(st(W_END_EDIT_TEXT, &[a, b]));
        }
        W_END_EDIT_TEXT => {
            let Some(scratch) = r.scratch else {
                out.defer = true;
                return;
            };
            let h = t.edit_item;
            if !t.valid(h) {
                return;
            }
            let i = t.item(h);
            let (mask, psz, cch) = if a == 0 {
                (TVIF_TEXT, scratch + S_EDIT, r.last.min(EDIT_BUF - 1) + 1)
            } else {
                (0, 0, 0)
            };
            let body = [mask, h, i.state, 0xFFFF, psz, cch, 0, 0, 0, i.lparam];
            out.ops = notify(cpu, t, TVN_ENDLABELEDITA, &body);
            out.push.push(st(W_END_EDIT_TOLD, &[a, b]));
        }
        W_END_EDIT_TOLD => {
            let Some(scratch) = r.scratch else {
                return;
            };
            let h = t.edit_item;
            let edit = t.edit;
            if a == 0 && r.last != 0 && t.valid(h) {
                let text = read_text(cpu, scratch + S_EDIT, EDIT_BUF as usize - 1);
                if t.item(h).text.as_deref() != Some(&text[..]) {
                    t.item_mut(h).text = Some(text);
                    t.item_mut(h).callback &= !TVIF_TEXT;
                    measure(cpu, t, h, 0);
                }
            }
            // The edit control loses the focus as it goes, and tells the
            // tree, whose edit control it isn't by then.
            t.edit = 0;
            t.edit_item = 0;
            out.ops.push(user("ShowWindow", &[edit, SW_HIDE]));
            out.ops.push(user("DestroyWindow", &[edit]));
            invalidate(cpu, t.hwnd, true);
            if b != 0 {
                r.result = 1;
            }
        }
        W_IGNORE_KILL => t.ignore_kill = a != 0,
        W_EDIT_RESIZE => {
            let Some(scratch) = r.scratch else {
                return;
            };
            let h = t.edit_item;
            if t.edit == 0 || !t.valid(h) {
                return;
            }
            let text = read_text(cpu, scratch + S_EDIT, EDIT_BUF as usize - 1);
            let (maxw, w) = text_extent(cpu, t.edit, t.font_for(h), &text);
            let i = t.item(h);
            let cx = (w + maxw * 2)
                .max(maxw * 3)
                .min(t.client_width - i.text_offset + 2);
            let cy = i.rect.bottom - i.rect.top + 3;
            let flags = SWP_NOMOVE | SWP_DRAWFRAME;
            out.ops.push(user(
                "SetWindowPos",
                &[t.edit, 0, 0, 0, cx as u32, cy as u32, flags],
            ));
        }
        W_PAINT_BEGIN => {
            let hdc = r.last;
            let Some(scratch) = r.scratch else {
                return;
            };
            if hdc == 0 {
                return;
            }
            let ps = read_words(cpu, scratch + S_PS, 6);
            if t.redraw {
                out.push
                    .push(st(W_PAINT, &[hdc, ps[2], ps[3], ps[4], ps[5]]));
            }
        }
        W_PAINT => {
            if t.client_height == 0 || t.client_width == 0 {
                return;
            }
            let body = custom_draw_body(t, CDDS_PREPAINT, a, [b, c, d, e], 0, 0, 0);
            out.ops = notify(cpu, t, NM_CUSTOMDRAW, &body);
            out.push.push(st(W_PAINT_ITEMS, &[a, b, c, d, e]));
        }
        W_PAINT_ITEMS => {
            t.cdmode = r.last;
            if t.cdmode == CDRF_SKIPDEFAULT {
                return;
            }
            let (top, bottom) = (c as i32, e as i32);
            let mut h = t.first_child();
            while h != 0 {
                let i = t.item(h);
                if i.visible_order >= 0 {
                    if i.rect.top > bottom {
                        break;
                    }
                    if i.rect.bottom >= top {
                        out.push.push(st(W_DRAW_ITEM, &[a, h]));
                    }
                }
                h = t.next_list(h);
            }
            out.push.push(st(W_PAINT_END, &[a, b, c, d, e]));
        }
        W_DRAW_ITEM => {
            if !t.valid(b) {
                return;
            }
            if t.item(b).callback & CALLBACK_MASK_ALL != 0 && c == 0 {
                out.push.push(st(W_DISPINFO, &[b, CALLBACK_MASK_ALL]));
                out.push.push(st(W_DRAW_ITEM, &[a, b, 1]));
                return;
            }
            draw_item_start(cpu, t, a, b, out);
        }
        W_DRAW_ITEM_ASKED => {
            let Some(scratch) = r.scratch else {
                return;
            };
            if !t.valid(b) {
                return;
            }
            let cditem = if c != 0 { r.last } else { 0 };
            if cditem & CDRF_SKIPDEFAULT == 0 {
                let colors = read_words(cpu, scratch + S_NM + 48, 2);
                if cditem & CDRF_NEWFONT != 0 {
                    measure(cpu, t, b, a);
                }
                draw_item(cpu, t, a, b, colors[0], colors[1]);
                if cditem & CDRF_NOTIFYPOSTPAINT != 0 {
                    let i = t.item(b);
                    let rc = rect_words(i.rect);
                    let mut body = custom_draw_body(
                        t,
                        CDDS_ITEMPOSTPAINT,
                        a,
                        rc,
                        b,
                        item_state(t, b),
                        i.lparam,
                    );
                    body[9] = colors[0];
                    body[10] = colors[1];
                    body[11] = i.level as u32;
                    out.ops = notify(cpu, t, NM_CUSTOMDRAW, &body);
                }
            }
            gfx::select(cpu, a, d);
        }
        W_PAINT_END => {
            update_scroll_bars(cpu, t, &mut out.ops);
            if t.cdmode & CDRF_NOTIFYPOSTPAINT != 0 {
                let body = custom_draw_body(t, CDDS_POSTPAINT, a, [b, c, d, e], 0, 0, 0);
                out.ops.extend(notify(cpu, t, NM_CUSTOMDRAW, &body));
            }
        }
        W_END_PAINT => {
            out.ops
                .push(call("EndPaint", &[Arg::V(t.hwnd), Arg::S(S_PS)]));
        }
        W_CLICK => {
            out.ops = notify(cpu, t, NM_CLICK, &[]);
            out.push.push(st(W_CLICK_ASKED, &[a, b, c]));
        }
        W_CLICK_ASKED => {
            if r.last != 0 {
                out.push.push(st(W_SET_FOCUS, &[]));
                return;
            }
            press(cpu, t, a, b, c, out);
        }
        W_PRESS => press(cpu, t, a, b, c, out),
        W_RELEASE => {
            // The button went up without a drag.
            out.ops = notify(cpu, t, NM_CLICK, &[]);
            out.push.push(st(W_RELEASE_ASKED, &[a, b, c]));
        }
        W_RELEASE_ASKED => {
            if r.last != 0 {
                out.push.push(st(W_SET_FOCUS, &[]));
                return;
            }
            clicked(cpu, t, a, b, c != 0, out);
        }
        W_SET_FOCUS => out.ops.push(user("SetFocus", &[t.hwnd])),
        W_DBLCLK_ASKED => {
            if r.last != 0 || !t.valid(a) {
                return;
            }
            match b {
                TVHT_ONITEMINDENT => {
                    if t.style & TVS_HASLINES == 0 {
                        return;
                    }
                    let mut level = c as i32 / t.indent.max(1) as i32;
                    if t.style & TVS_LINESATROOT == 0 {
                        level += 1;
                    }
                    let mut h = a;
                    while t.item(h).level > level && t.item(h).parent != t.root {
                        h = t.item(h).parent;
                    }
                    out.push.push(st(W_TOGGLE, &[h]));
                }
                TVHT_ONITEMLABEL | TVHT_ONITEMICON | TVHT_ONITEMBUTTON => {
                    out.push.push(st(W_TOGGLE, &[a]));
                }
                TVHT_ONITEMSTATEICON => {
                    if t.style & TVS_CHECKBOXES != 0 {
                        toggle_state(cpu, t, a);
                    } else {
                        out.push.push(st(W_TOGGLE, &[a]));
                    }
                }
                _ => {}
            }
        }
        W_TOGGLE => {
            if !t.valid(a) {
                if b & EX_RET != 0 {
                    r.result = 0;
                }
                return;
            }
            let flags = EX_USER | b;
            if t.expanded(a) {
                out.push.push(st(W_COLLAPSE, &[a, flags]));
            } else {
                out.push.push(st(W_EXPAND, &[a, flags]));
            }
        }
        W_EXPAND_ALL => {
            if !t.valid(a) {
                return;
            }
            out.push.push(st(W_EXPAND, &[a, EX_USER]));
            let mut child = t.item(a).first_child;
            while child != 0 {
                if t.item(child).children > 0 {
                    out.push.push(st(W_EXPAND_ALL, &[child]));
                }
                child = t.item(child).next;
            }
        }
        W_FOCUS_TELL => {
            invalidate(cpu, t.hwnd, true);
            out.ops = notify(cpu, t, a, &[]);
        }
        W_SORT => sort_step(cpu, t, r, out),
        W_SORT_GOT => {
            if (r.last as i32) < 0 {
                t.sort_hi = a;
            } else {
                t.sort_lo = a + 1;
            }
            out.push.push(st(W_SORT, &[]));
        }
        W_FIRST_VISIBLE => {
            if t.valid(a) {
                set_first_visible(cpu, t, a, true, &mut out.ops);
            }
        }
        W_NOTIFY_FORMAT => {
            let format = r.last;
            if format == NFR_ANSI || format == NFR_UNICODE {
                t.unicode = format == NFR_UNICODE;
                r.result = format;
            } else {
                r.result = 0;
            }
        }
        W_SETCURSOR_ASKED => {
            if r.last != 0 {
                r.result = 1;
                return;
            }
            if a != 0 && t.style & TVS_TRACKSELECT != 0 {
                let hand = user32::resource::system_cursor(cpu, IDC_HAND);
                out.ops.push(user("SetCursor", &[hand]));
                r.result = 0;
            } else {
                out.ops
                    .push(user("DefWindowProcA", &[t.hwnd, WM_SETCURSOR, b, c]));
                out.push.push(st(W_RESULT, &[]));
            }
        }
        W_RESULT => r.result = r.last,
        W_RCLICK => {
            out.ops = notify(cpu, t, NM_RCLICK, &[]);
        }
        W_KEY => key_down(cpu, t, a, out),
        W_TOOLTIP if r.last != 0 => t.tooltip = r.last,
        _ => {}
    }
}

fn collapse_action(flags: u32) -> u32 {
    TVE_COLLAPSE
        | if flags & EX_RESET != 0 {
            TVE_COLLAPSERESET
        } else {
            0
        }
}

fn rect_words(r: Rect) -> [u32; 4] {
    [r.left as u32, r.top as u32, r.right as u32, r.bottom as u32]
}

// Expanding and collapsing.

/// TREEVIEW_Expand, up to TVN_ITEMEXPANDING.
fn expand(cpu: &mut Cpu, t: &mut Tree, h: u32, flags: u32, r: &mut Run, out: &mut Out) {
    if !t.user_item(h) {
        if flags & EX_RET != 0 {
            r.result = 0;
        }
        return;
    }
    if t.expanded(h) {
        if flags & EX_RET != 0 {
            r.result = 1;
        }
        return;
    }
    let i = t.item(h);
    if flags & EX_USER != 0 || i.state & TVIS_EXPANDEDONCE == 0 {
        out.ops = tv_notify(cpu, t, TVN_ITEMEXPANDINGA, TVE_EXPAND, EXPAND_MASK, 0, h, 0);
        out.push.push(st(W_EXPAND_ASKED, &[h, flags]));
        return;
    }
    if i.first_child == 0 {
        if flags & EX_RET != 0 {
            r.result = 0;
        }
        return;
    }
    t.item_mut(h).state |= TVIS_EXPANDED;
    if t.shown(h) {
        t.recalc_order(h);
        update_subtree(cpu, t, h);
    }
    out.push.push(st(W_EXPAND_SHOW, &[h, flags]));
}

/// The rest of TREEVIEW_Expand: the children showing, and scrolled to.
fn expand_show(cpu: &mut Cpu, t: &mut Tree, h: u32, flags: u32, r: &mut Run, out: &mut Out) {
    if !t.valid(h) {
        return;
    }
    if t.shown(h) {
        update_scroll_bars(cpu, t, &mut out.ops);
        invalidate(cpu, t.hwnd, true);
        let i = t.item(h);
        if i.first_child != 0 && t.valid(t.first_visible) {
            let children = t.order(t.last_list(h).max(i.first_child)) - t.order(i.first_child) + 1;
            let visible_pos = i.visible_order - t.order(t.first_visible);
            let rows_below = t.visible_count() - visible_pos - 1;
            if visible_pos > 0 && children > rows_below {
                let scroll = (children - rows_below).min(visible_pos);
                if scroll > 0 {
                    let new = t.list_item(t.first_visible, scroll);
                    set_first_visible(cpu, t, new, true, &mut out.ops);
                }
            }
        }
    }
    if flags & EX_RET != 0 {
        r.result = 1;
    }
}

/// The rest of TREEVIEW_Collapse, once the parent knows.
#[allow(clippy::too_many_arguments)]
fn collapse_done(
    cpu: &mut Cpu,
    t: &mut Tree,
    h: u32,
    flags: u32,
    set_sel: bool,
    set_first: bool,
    r: &mut Run,
    out: &mut Out,
) {
    if !t.valid(h) {
        return;
    }
    if flags & EX_RESET != 0 {
        let children = t.item(h).children;
        t.remove_children(h);
        t.item_mut(h).children = children;
    }
    // The children, and what shows under them, are hidden now.
    let mut gone = Vec::new();
    let mut child = t.item(h).first_child;
    while child != 0 {
        t.subtree(child, &mut gone);
        child = t.item(child).next;
    }
    for g in gone {
        t.item_mut(g).visible_order = -1;
    }
    t.recalc_order(h);
    if set_sel {
        if t.valid(t.selected) {
            let s = t.selected;
            t.item_mut(s).state &= !TVIS_SELECTED;
        }
        t.item_mut(h).state |= TVIS_SELECTED;
        t.selected = h;
    }
    update_scroll_bars(cpu, t, &mut out.ops);
    invalidate(cpu, t.hwnd, true);
    let first = if set_first { h } else { t.first_visible };
    set_first_visible(cpu, t, first, true, &mut out.ops);
    if flags & EX_RET != 0 {
        r.result = 1;
    }
}

// Selecting and showing.

/// TREEVIEW_DoSelectItem, up to TVN_SELCHANGING.
#[allow(clippy::too_many_arguments)]
fn select(
    cpu: &mut Cpu,
    t: &mut Tree,
    action: u32,
    h: u32,
    cause: u32,
    ret: bool,
    r: &mut Run,
    out: &mut Out,
) {
    let h = if t.user_item(h) { h } else { 0 };
    if ret {
        r.result = 1;
    }
    if t.focused != 0 {
        invalidate(cpu, t.hwnd, true);
        t.focused = 0;
    }
    match action {
        TVGN_CARET => {
            let prev = t.selected;
            if prev == h {
                out.push.push(st(W_ENSURE, &[prev, 0, 0]));
                return;
            }
            let Some(scratch) = r.scratch else {
                out.defer = true;
                return;
            };
            let mask = TVIF_TEXT | TVIF_HANDLE | TVIF_STATE | TVIF_PARAM;
            out.ops = tv_notify(cpu, t, TVN_SELCHANGINGA, cause, mask, prev, h, scratch);
            out.push
                .push(st(W_SELECT_ASKED, &[h, prev, cause, ret as u32]));
        }
        TVGN_DROPHILITE => {
            let prev = t.drop_item;
            if t.valid(prev) {
                t.item_mut(prev).state &= !TVIS_DROPHILITED;
            }
            t.drop_item = h;
            if h != 0 {
                t.item_mut(h).state |= TVIS_DROPHILITED;
            }
            invalidate(cpu, t.hwnd, true);
        }
        TVGN_FIRSTVISIBLE if h != 0 => {
            out.push.push(st(W_ENSURE, &[h, 0, 0]));
            out.push.push(st(W_FIRST_VISIBLE, &[h]));
        }
        _ => {}
    }
}

/// The rest of TREEVIEW_EnsureVisible, once the parents are expanded.
fn ensure_scroll(
    cpu: &mut Cpu,
    t: &mut Tree,
    h: u32,
    across: bool,
    ret: bool,
    r: &mut Run,
    out: &mut Out,
) {
    if ret {
        r.result = 0;
    }
    if !t.valid(h) || !t.shown(h) {
        return;
    }
    let count = t.visible_count();
    let pos = if t.valid(t.first_visible) {
        t.order(h) - t.order(t.first_visible)
    } else {
        -1
    };
    let mut new = 0;
    if pos < 0 {
        new = h;
    } else if pos >= count && pos > 0 && t.style & TVS_NOSCROLL == 0 {
        new = t.list_item(t.first_visible, pos - count + 1);
    }
    if across {
        let (maxw, _) = text_extent(cpu, t.hwnd, t.font, b"");
        let i = t.item(h);
        let x = (i.text_width + maxw * 2).max(maxw * 3);
        let p = if i.text_offset < 0 {
            i.text_offset
        } else if i.text_offset + x > t.client_width {
            if x > t.client_width {
                i.text_offset
            } else {
                i.text_offset + x - t.client_width
            }
        } else {
            0
        };
        let to = t.scroll_x + p;
        hscroll(cpu, t, SB_THUMBPOSITION, to, &mut out.ops);
    }
    if new != 0 && new != t.first_visible {
        set_first_visible(cpu, t, new, true, &mut out.ops);
        if ret {
            r.result = 1;
        }
    }
}

// Getting and setting items.

/// The parent's answer to TVN_GETDISPINFO (the rest of
/// TREEVIEW_UpdateDispInfo).
fn dispinfo_got(cpu: &mut Cpu, t: &mut Tree, h: u32, mask: u32, scratch: u32) {
    let nm = read_words(cpu, scratch + S_NM + 12, 10);
    if cpu.bus.guest_faulted() {
        cpu.bus.guest_fault = None;
        return;
    }
    let mask = mask & t.item(h).callback;
    if mask & TVIF_TEXT != 0 {
        let text = read_text(cpu, nm[4], 0x1_0000);
        t.item_mut(h).text = Some(text);
    }
    let i = t.item_mut(h);
    if mask & TVIF_IMAGE != 0 {
        i.image = nm[6] as i32;
    }
    if mask & TVIF_SELECTEDIMAGE != 0 {
        i.selected_image = nm[7] as i32;
    }
    if mask & TVIF_CHILDREN != 0 {
        i.children = nm[8] as i32;
    }
    if nm[0] & TVIF_DI_SETITEM != 0 {
        i.callback &= !nm[0];
    }
}

/// The string at `at`, or nothing for a pointer the program can't
/// read: a continuation can't have the instruction run again.
fn read_text(cpu: &mut Cpu, at: u32, max: usize) -> Vec<u8> {
    if at == 0 || at == u32::MAX {
        return Vec::new();
    }
    let text = cpu.bus.guest_read_asciiz(at, max);
    if cpu.bus.guest_faulted() {
        cpu.bus.guest_fault = None;
        return Vec::new();
    }
    text
}

/// TVM_GETITEMA's filling of the TVITEM at `at` (TREEVIEW_GetItemT).
fn get_item(cpu: &mut Cpu, t: &mut Tree, at: u32, r: &mut Run) {
    let w = read_words(cpu, at, 10);
    if cpu.bus.guest_faulted() {
        cpu.bus.guest_fault = None;
        r.result = 0;
        return;
    }
    let (mask, h) = (w[0], w[1]);
    if !t.user_item(h) {
        r.result = 0;
        return;
    }
    let i = t.item(h).clone();
    if mask & TVIF_CHILDREN != 0 {
        cpu.bus.guest_write_32(at + 32, i.children as u32);
    }
    if mask & TVIF_IMAGE != 0 {
        cpu.bus.guest_write_32(at + 24, i.image as u32);
    }
    if mask & TVIF_INTEGRAL != 0 {
        cpu.bus.guest_write_32(at + 40, i.integral as u32);
    }
    // lParam whatever the mask says, as Windows does.
    cpu.bus.guest_write_32(at + 36, i.lparam);
    if mask & TVIF_SELECTEDIMAGE != 0 {
        cpu.bus.guest_write_32(at + 28, i.selected_image as u32);
    }
    if mask & TVIF_STATE != 0 {
        // The whole state, whatever stateMask asks for.
        cpu.bus.guest_write_32(at + 8, i.state);
    }
    if mask & TVIF_TEXT != 0 {
        let (psz, cch) = (w[4], w[5]);
        if cch as i32 > 0 && psz != 0 {
            match &i.text {
                None => cpu.bus.guest_write_8(psz, 0),
                Some(text) => {
                    user32::copy_out(cpu, psz, cch, text);
                }
            }
        }
    }
    cpu.bus.guest_fault = None;
    r.result = 1;
}

/// The fields of a TVITEM(EX) a program passes.
struct TvItem {
    mask: u32,
    h: u32,
    state: u32,
    state_mask: u32,
    /// The label, None for a callback.
    text: Option<Option<Vec<u8>>>,
    image: i32,
    selected_image: i32,
    children: i32,
    lparam: u32,
    integral: i32,
}

/// Read the TVITEM at `at`: None when its memory isn't there.
fn read_tv_item(cpu: &mut Cpu, at: u32) -> Option<TvItem> {
    let w = read_words(cpu, at, 10);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let mask = w[0];
    let integral = if mask & TVIF_INTEGRAL != 0 {
        let v = cpu.bus.guest_read_32(at + 40);
        if cpu.bus.guest_faulted() {
            return None;
        }
        v as i32
    } else {
        1
    };
    let text = if mask & TVIF_TEXT != 0 {
        // A NULL label is a callback, as LPSTR_TEXTCALLBACK is.
        if w[4] == u32::MAX || w[4] == 0 {
            Some(None)
        } else {
            Some(Some(user32::bytes_at(cpu, w[4])?))
        }
    } else {
        None
    };
    Some(TvItem {
        mask,
        h: w[1],
        state: w[2],
        state_mask: w[3],
        text,
        image: w[6] as i32,
        selected_image: w[7] as i32,
        children: w[8] as i32,
        lparam: w[9],
        integral,
    })
}

/// Set an item's fields from a TVITEM (TREEVIEW_DoSetItemT).
fn do_set_item(t: &mut Tree, h: u32, v: &TvItem) {
    let mut set = 0;
    let mut clear = 0;
    let i = t.item_mut(h);
    if let Some(text) = &v.text {
        i.text_width = 0;
        match text {
            Some(text) => {
                clear |= TVIF_TEXT;
                i.text = Some(text.clone());
            }
            None => {
                set |= TVIF_TEXT;
                let mut old = i.text.take().unwrap_or_default();
                old.truncate(TEXT_CALLBACK_SIZE as usize - 1);
                i.text = Some(old);
            }
        }
    }
    if v.mask & TVIF_CHILDREN != 0 {
        i.children = v.children;
        if i.children == -1 {
            set |= TVIF_CHILDREN;
        } else {
            clear |= TVIF_CHILDREN;
        }
    }
    if v.mask & TVIF_IMAGE != 0 {
        i.image = v.image;
        if i.image == -1 {
            set |= TVIF_IMAGE;
        } else {
            clear |= TVIF_IMAGE;
        }
    }
    if v.mask & TVIF_SELECTEDIMAGE != 0 {
        i.selected_image = v.selected_image;
        if i.selected_image == -1 {
            set |= TVIF_SELECTEDIMAGE;
        } else {
            clear |= TVIF_SELECTEDIMAGE;
        }
    }
    if v.mask & TVIF_PARAM != 0 {
        i.lparam = v.lparam;
    }
    if v.mask & TVIF_INTEGRAL != 0 {
        i.integral = v.integral.max(1);
    }
    if v.mask & TVIF_STATE != 0 {
        i.state &= !v.state_mask;
        i.state |= v.state & v.state_mask;
    }
    i.callback |= set;
    i.callback &= !clear;
}

/// A new item's handle.
fn new_handle(cpu: &mut Cpu) -> u32 {
    let s = &mut state(cpu).treeview;
    if s.next_item < FIRST_ITEM {
        s.next_item = FIRST_ITEM;
    }
    let h = s.next_item;
    s.next_item = s.next_item.wrapping_add(ITEM_STRIDE);
    if s.next_item >= TVI_ROOT {
        s.next_item = FIRST_ITEM;
    }
    h
}

/// Case-insensitive comparison of labels, as lstrcmpi compares them.
fn compare_i(a: &[u8], b: &[u8]) -> Ordering {
    let low = |s: &[u8]| s.iter().map(|c| c.to_ascii_lowercase()).collect::<Vec<_>>();
    low(a).cmp(&low(b))
}

/// The labels' order as lstrcmp gives it: without case first, then
/// lower case before upper.
fn compare(a: &[u8], b: &[u8]) -> Ordering {
    compare_i(a, b).then_with(|| {
        let swap = |s: &[u8]| {
            s.iter()
                .map(|c| {
                    if c.is_ascii_alphabetic() {
                        c ^ 0x20
                    } else {
                        *c
                    }
                })
                .collect::<Vec<_>>()
        };
        swap(a).cmp(&swap(b))
    })
}

/// TVM_INSERTITEMA (TREEVIEW_InsertItemT): the new item, or 0.
fn insert_item(cpu: &mut Cpu, hwnd: u32, at: u32) -> Option<u32> {
    let w = read_words(cpu, at, 2);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let v = read_tv_item(cpu, at + 8)?;
    let h = new_handle(cpu);
    let mut ops = Vec::new();
    let result = with(cpu, hwnd, |cpu, t| {
        let (parent_in, after_in) = (w[0], w[1]);
        let parent = if parent_in == TVI_ROOT || parent_in == 0 {
            t.root
        } else if t.valid(parent_in) {
            parent_in
        } else {
            return 0;
        };
        let mut after = after_in;
        if !matches!(after, TVI_FIRST | TVI_LAST | TVI_SORT)
            && (!t.valid(after) || t.item(after).parent != parent)
        {
            after = TVI_LAST;
        }
        let item = Item {
            parent,
            integral: 1,
            visible_order: -1,
            ..Item::default()
        };
        t.items.insert(h, item);
        do_set_item(t, h, &v);
        t.count += 1;
        match after {
            TVI_FIRST => {
                let first = t.item(parent).first_child;
                t.insert_before(h, first, parent);
                if t.first_visible == first && first != 0 {
                    set_first_visible(cpu, t, h, true, &mut ops);
                }
            }
            TVI_LAST => {
                let last = t.item(parent).last_child;
                t.insert_after_item(h, last, parent);
            }
            TVI_SORT => {
                let original = t.item(parent).first_child;
                let text = t.item(h).text.clone().unwrap_or_default();
                let mut child = original;
                let mut previous = 0;
                let mut inserted = false;
                while child != 0 {
                    let other = t.item(child).text.clone().unwrap_or_default();
                    match compare(&text, &other) {
                        Ordering::Less => {
                            t.insert_before(h, child, parent);
                            if t.first_visible == original && child == original {
                                set_first_visible(cpu, t, h, true, &mut ops);
                            }
                            inserted = true;
                            break;
                        }
                        Ordering::Greater => {
                            previous = child;
                            child = t.item(child).next;
                        }
                        Ordering::Equal => {
                            t.insert_after_item(h, child, parent);
                            inserted = true;
                            break;
                        }
                    }
                }
                if !inserted {
                    t.insert_after_item(h, previous, parent);
                }
            }
            _ => t.insert_after_item(h, after, parent),
        }
        let level = t.item(parent).level + 1;
        t.item_mut(h).level = level;
        if t.item(parent).children == 0 {
            t.item_mut(parent).children = 1;
        }
        if t.style & TVS_CHECKBOXES != 0 && state_image_index(t.item(h).state) == 0 {
            t.item_mut(h).state |= 1 << 12;
        }
        if t.first_visible == 0 {
            t.first_visible = h;
        }
        if !t.redraw {
            return h;
        }
        if t.item(parent).state & TVIS_EXPANDED != 0 && (parent == t.root || t.shown(parent)) {
            let prev = t.prev_list(h);
            t.recalc_order(prev);
            t.compute_metrics(h);
            measure(cpu, t, h, 0);
            update_scroll_bars(cpu, t, &mut ops);
            invalidate(cpu, hwnd, true);
        } else if t.shown(parent) && t.item(h).prev == t.item(h).next {
            invalidate(cpu, hwnd, true);
        }
        h
    })
    .unwrap_or(0);
    finish(cpu, hwnd, ops, result)
}

/// TVM_SETITEMA (TREEVIEW_SetItemT).
fn set_item(cpu: &mut Cpu, hwnd: u32, at: u32) -> Option<u32> {
    let v = read_tv_item(cpu, at)?;
    let mut ops = Vec::new();
    let result = with(cpu, hwnd, |cpu, t| {
        let h = v.h;
        if !t.user_item(h) {
            return 0;
        }
        let original = t.item(h).clone();
        do_set_item(t, h, &v);
        let shown = t.shown(h);
        let bold = v.mask & TVIF_STATE != 0 && v.state_mask & TVIS_BOLD != 0;
        if (v.mask & TVIF_TEXT != 0 || bold) && shown {
            measure(cpu, t, h, 0);
        }
        if v.mask != 0 && shown {
            t.compute_metrics(h);
            let new = t.item(h);
            let changed = (v.mask & TVIF_CHILDREN != 0 && original.children != new.children)
                || (v.mask & TVIF_IMAGE != 0 && original.image != new.image)
                || (v.mask & TVIF_SELECTEDIMAGE != 0
                    && original.selected_image != new.selected_image)
                || v.mask & TVIF_TEXT != 0
                || (v.mask & TVIF_INTEGRAL != 0 && original.integral != new.integral)
                || (v.mask & TVIF_STATE != 0 && (original.state ^ new.state) & v.state_mask != 0);
            if changed {
                if v.mask & TVIF_INTEGRAL != 0 {
                    t.recalc_order(h);
                }
                update_scroll_bars(cpu, t, &mut ops);
                invalidate(cpu, hwnd, true);
            }
        }
        1
    })
    .unwrap_or(0);
    finish(cpu, hwnd, ops, result)
}

/// TVN_DELETEITEM for `h`.
fn delete_notify(cpu: &mut Cpu, t: &Tree, h: u32) -> Vec<Op> {
    let mask = TVIF_HANDLE | TVIF_PARAM;
    tv_notify(cpu, t, TVN_DELETEITEMA, TVC_UNKNOWN, mask, h, 0, 0)
}

/// TREEVIEW_DeleteItem, up to the notifications.
fn delete(cpu: &mut Cpu, t: &mut Tree, h: u32, out: &mut Out) {
    let mut gone = Vec::new();
    if h == TVI_ROOT || h == 0 {
        let mut child = t.first_child();
        while child != 0 {
            t.subtree(child, &mut gone);
            child = t.item(child).next;
        }
        for g in &gone {
            out.ops.extend(delete_notify(cpu, t, *g));
        }
        out.push.push(st(W_DELETE_GONE, &[TVI_ROOT, 0, 0, 0, 1]));
        return;
    }
    if !t.user_item(h) {
        return;
    }
    let (prev, visible) = if t.shown(h) {
        (t.prev_list(h), true)
    } else {
        (0, false)
    };
    let mut new_sel = 0;
    if t.selected != 0 && (h == t.selected || t.is_child_of(h, t.selected)) {
        let i = t.item(h);
        new_sel = if i.next != 0 {
            i.next
        } else if i.parent != t.root {
            i.parent
        } else {
            i.prev
        };
    }
    let new_first = if t.first_visible == h || t.is_child_of(h, t.first_visible) {
        let i = t.item(h);
        if i.next != 0 {
            i.next
        } else if i.prev != 0 {
            i.prev
        } else if i.parent != t.root {
            i.parent
        } else {
            0
        }
    } else {
        t.first_visible
    };
    t.subtree(h, &mut gone);
    for g in &gone {
        out.ops.extend(delete_notify(cpu, t, *g));
    }
    out.push.push(st(
        W_DELETE_GONE,
        &[h, new_sel, new_first, prev, visible as u32],
    ));
}

/// The rest of TREEVIEW_DeleteItem, once the parent knows.
fn delete_gone(
    t: &mut Tree,
    h: u32,
    new_sel: u32,
    new_first: u32,
    prev: u32,
    visible: u32,
    out: &mut Out,
) {
    let parent;
    if h == TVI_ROOT {
        parent = t.root;
        let root = t.root;
        t.remove_children(root);
        t.first_visible = 0;
    } else {
        if !t.valid(h) {
            return;
        }
        parent = t.item(h).parent;
        let was_first = t.first_visible;
        t.remove(h);
        if was_first != 0 && !t.valid(was_first) {
            t.first_visible = 0;
        }
    }
    if parent != t.root && t.valid(parent) && t.item(parent).first_child == 0 {
        // The parent of no more children loses its button, and is
        // collapsed.
        let p = t.item_mut(parent);
        p.state &= !(TVIS_EXPANDED | TVIS_EXPANDEDONCE);
        if p.children > 0 {
            p.children = 0;
        }
    }
    if t.selected == 0 && t.valid(new_sel) {
        out.push
            .push(st(W_SELECT, &[TVGN_CARET, new_sel, TVC_UNKNOWN, 0]));
    }
    out.push
        .push(st(W_DELETE_SHOW, &[parent, new_first, prev, visible]));
}

// Label editing.

fn edit_create(cpu: &mut Cpu, t: &mut Tree, h: u32, r: &mut Run, out: &mut Out) {
    if !t.valid(h) {
        r.result = 0;
        return;
    }
    if r.scratch.is_none() {
        out.defer = true;
        return;
    }
    let text = t.item(h).text.clone().unwrap_or_default();
    let (maxw, w) = text_extent(cpu, t.hwnd, t.font, &text);
    let i = t.item(h);
    let cx = (w + maxw * 2)
        .max(maxw * 3)
        .min(t.client_width - i.text_offset + 2);
    t.edit_item = h;
    let instance = user32::user(cpu).window(t.hwnd).map_or(0, |w| w.instance);
    let style = WS_CHILD | WS_BORDER | ES_AUTOHSCROLL | WS_CLIPSIBLINGS | ES_WANTRETURN;
    let i = t.item(h);
    out.ops.push(write(S_CLASS, script::words_of(b"Edit\0")));
    // The label goes in as the window's name, so the edit control tells
    // the tree EN_UPDATE and EN_CHANGE before WM_PARENTNOTIFY.
    out.ops.push(write(S_EDIT, text_words(&text, EDIT_BUF)));
    out.ops.push(call(
        "CreateWindowExA",
        &[
            Arg::V(0),
            Arg::S(S_CLASS),
            Arg::S(S_EDIT),
            Arg::V(style),
            Arg::V((i.text_offset - 2) as u32),
            Arg::V((i.rect.top - 1) as u32),
            Arg::V((cx + 3) as u32),
            Arg::V((i.rect.bottom - i.rect.top + 3) as u32),
            Arg::V(t.hwnd),
            Arg::V(0),
            Arg::V(instance),
            Arg::V(0),
        ],
    ));
    out.push.push(st(W_EDIT_CREATED, &[h]));
}

fn edit_created(cpu: &mut Cpu, t: &mut Tree, h: u32, r: &mut Run, out: &mut Out) {
    let edit = r.last;
    let Some(scratch) = r.scratch else {
        return;
    };
    if edit == 0 {
        t.edit_item = 0;
        r.result = 0;
        return;
    }
    t.edit = edit;
    let (style, ex) = user32::user(cpu)
        .window(edit)
        .map_or((0, 0), |w| (w.style, w.ex_style));
    out.ops.push(user(
        "SetWindowLongA",
        &[edit, GWL_EXSTYLE, ex & !WS_EX_CLIENTEDGE],
    ));
    out.ops.push(user(
        "SetWindowLongA",
        &[edit, GWL_STYLE, style | WS_BORDER],
    ));
    let font = t.font_for(h);
    out.ops.push(send(edit, WM_SETFONT, font, Arg::V(0)));
    // The edit control's own procedure goes on behind the tree's, which
    // ends the editing on Enter and Esc.
    let ours = super::address_of("TreeViewEditProc");
    let proc = user32::user(cpu)
        .window_mut(edit)
        .map(|w| std::mem::replace(&mut w.proc, ours));
    if let Some(proc) = proc {
        let tree = t.hwnd;
        state(cpu).treeview.edits.insert(edit, Edit { tree, proc });
    }
    if !t.valid(h) {
        return;
    }
    let mask = TVIF_HANDLE | TVIF_STATE | TVIF_PARAM | TVIF_TEXT;
    let text = t.item(h).text.clone().unwrap_or_default();
    out.ops.push(write(S_TEXT1, text_words(&text, TEXT_BUF)));
    let body = tv_item(t, h, mask, scratch + S_TEXT1);
    out.ops.extend(notify(cpu, t, TVN_BEGINLABELEDITA, &body));
    out.push.push(st(W_EDIT_BEGUN, &[h, edit]));
}

/// The procedure of the edit control of a label being edited
/// (TREEVIEW_Edit_SubclassProc).
pub fn edit_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, msg, wp, lp] = args else {
        return Some(0);
    };
    let Some(e) = state(cpu).treeview.edits.get(&hwnd).cloned() else {
        return defwnd::def_window_proc(cpu, args);
    };
    let forward = |cpu: &mut Cpu| {
        let mut s = Script::new(e.tree);
        let args = [
            Arg::V(e.proc),
            Arg::V(hwnd),
            Arg::V(msg),
            Arg::V(wp),
            Arg::V(lp),
        ];
        s.call(user32::address_of("CallWindowProcA"), &args)
            .ret_last();
        script::run(cpu, TREEVIEW, s)
    };
    let flags = peek(cpu, e.tree, |t| (t.ignore_kill, t.label_changed));
    let cancel = match msg {
        WM_KILLFOCUS => {
            if flags.is_some_and(|(ignore, _)| ignore) {
                return Some(1);
            }
            return forward(cpu);
        }
        WM_DESTROY => {
            state(cpu).treeview.edits.remove(&hwnd);
            if let Some(w) = user32::user(cpu).window_mut(hwnd) {
                w.proc = e.proc;
            }
            return forward(cpu);
        }
        WM_GETDLGCODE => return Some(DLGC_WANTARROWS | DLGC_WANTALLKEYS),
        WM_KEYDOWN if wp == VK_ESCAPE => true,
        WM_KEYDOWN if wp == VK_RETURN => false,
        _ => return forward(cpu),
    };
    let Some((_, changed)) = flags else {
        return forward(cpu);
    };
    let steps = vec![
        st(W_IGNORE_KILL, &[1]),
        st(W_END_EDIT, &[(cancel || !changed) as u32, 0]),
        st(W_IGNORE_KILL, &[0]),
    ];
    start(cpu, e.tree, steps, 0)
}

/// WM_COMMAND from the edit control (TREEVIEW_Command).
fn command(cpu: &mut Cpu, hwnd: u32, wp: u32, lp: u32) -> Option<u32> {
    let Some((edit, notify_to)) = peek(cpu, hwnd, |t| (t.edit, t.notify)) else {
        return Some(0);
    };
    match wp >> 16 {
        EN_UPDATE if edit != 0 && edit == lp => {
            if let Some(t) = state(cpu).treeview.trees.get_mut(&hwnd) {
                t.label_changed = true;
            }
            let mut s = new_script(hwnd);
            s.ops.push(write(S_EDIT, vec![0]));
            s.call(
                user32::address_of("GetWindowTextA"),
                &[Arg::V(edit), Arg::S(S_EDIT), Arg::V(EDIT_BUF - 1)],
            );
            s.ops.push(then_steps(&[st(W_EDIT_RESIZE, &[])]));
            s.ret(0);
            script::run(cpu, TREEVIEW, s)
        }
        EN_KILLFOCUS => {
            if edit == 0 || edit != lp {
                return Some(0);
            }
            start(cpu, hwnd, vec![st(W_END_EDIT, &[0, 0])], 0)
        }
        _ => {
            let mut s = Script::new(hwnd);
            s.ops.push(send(notify_to, WM_COMMAND, wp, Arg::V(lp)));
            s.ret_last();
            script::run(cpu, TREEVIEW, s)
        }
    }
}

// Painting.

/// An NMTVCUSTOMDRAW's words after its NMHDR.
fn custom_draw_body(
    t: &Tree,
    stage: u32,
    hdc: u32,
    rc: [u32; 4],
    spec: u32,
    item_state: u32,
    lparam: u32,
) -> Vec<u32> {
    vec![
        stage, hdc, rc[0], rc[1], rc[2], rc[3], spec, item_state, lparam, t.clr_text, t.clr_bk, 0,
    ]
}

fn in_focus(cpu: &mut Cpu, t: &Tree) -> bool {
    user32::input::get_focus(cpu, &[]) == Some(t.hwnd)
}

fn item_state(t: &Tree, h: u32) -> u32 {
    let mut s = 0;
    if t.item(h).state & TVIS_SELECTED != 0 {
        s |= CDIS_SELECTED;
    }
    if h == t.selected {
        s |= CDIS_FOCUS;
    }
    if h == t.hot {
        s |= CDIS_HOT;
    }
    s
}

/// TREEVIEW_DrawItem up to its NM_CUSTOMDRAW: the colors, the font, the
/// label's width.
fn draw_item_start(cpu: &mut Cpu, t: &mut Tree, hdc: u32, h: u32, out: &mut Out) {
    let focus = in_focus(cpu, t);
    let (bk, text) = t.colors(cpu);
    let i = t.item(h);
    let highlit = i.state & TVIS_DROPHILITED != 0
        || (h == t.focused && i.state & TVIS_SELECTED == 0)
        || (i.state & TVIS_SELECTED != 0
            && t.focused == 0
            && (focus || t.style & TVS_SHOWSELALWAYS != 0));
    let (clr_text, clr_bk) = if highlit {
        if i.state & TVIS_DROPHILITED != 0 || focus {
            (
                gfx::sys_color(cpu, COLOR_HIGHLIGHTTEXT),
                gfx::sys_color(cpu, COLOR_HIGHLIGHT),
            )
        } else {
            (text, gfx::sys_color(cpu, COLOR_BTNFACE))
        }
    } else if t.style & TVS_TRACKSELECT != 0 && h == t.hot {
        (gfx::sys_color(cpu, COLOR_HIGHLIGHT), bk)
    } else {
        (text, bk)
    };
    let old_font = gfx::select(cpu, hdc, t.font_for(h));
    if t.item(h).text_width == 0 {
        measure(cpu, t, h, hdc);
    }
    let i = t.item(h);
    let mut body = custom_draw_body(
        t,
        CDDS_ITEMPREPAINT,
        hdc,
        rect_words(i.rect),
        h,
        item_state(t, h),
        i.lparam,
    );
    body[9] = clr_text;
    body[10] = clr_bk;
    body[11] = i.level as u32;
    if t.cdmode & CDRF_NOTIFYITEMDRAW != 0 {
        out.ops = notify(cpu, t, NM_CUSTOMDRAW, &body);
        out.push.push(st(W_DRAW_ITEM_ASKED, &[hdc, h, 1, old_font]));
    } else {
        // The colors wait in the scratch space as the parent would have
        // left them, for the step after.
        let id = super::id_of(cpu, t.hwnd);
        let mut words = vec![t.hwnd, id, NM_CUSTOMDRAW];
        words.extend(body);
        out.ops.push(write(S_NM, words));
        out.ops
            .push(then_steps(&[st(W_DRAW_ITEM_ASKED, &[hdc, h, 0, old_font])]));
    }
}

/// A line of every other pixel from `a` to `b`, without `b`, as Wine
/// draws the tree's lines.
fn dotted(cpu: &mut Cpu, hdc: u32, a: (i32, i32), b: (i32, i32), color: u32) {
    let points: Vec<(i32, i32)> = gdi32::draw::line_points(a.0, a.1, b.0, b.1).collect();
    for (n, (x, y)) in points.into_iter().enumerate() {
        if (x, y) != b && n % 2 == 0 {
            gdi32::draw::set_pixel(cpu, &[hdc, x as u32, y as u32, color]);
        }
    }
}

fn rectangle(cpu: &mut Cpu, hdc: u32, l: i32, t: i32, r: i32, b: i32) {
    gdi32::draw::rectangle(cpu, &[hdc, l as u32, t as u32, r as u32, b as u32]);
}

fn line_color(cpu: &mut Cpu, t: &Tree) -> u32 {
    if t.clr_line == CLR_DEFAULT {
        gfx::sys_color(cpu, COLOR_GRAYTEXT)
    } else {
        t.clr_line
    }
}

/// The lines and the button of an item (TREEVIEW_DrawItemLines).
fn draw_lines(cpu: &mut Cpu, t: &Tree, hdc: u32, h: u32) {
    let lar = (t.style & (TVS_LINESATROOT | TVS_HASLINES | TVS_HASBUTTONS)) > TVS_LINESATROOT;
    let i = t.item(h);
    if !lar && i.level == 0 {
        return;
    }
    let (bk, text) = t.colors(cpu);
    let cx = (i.lines_offset + i.state_offset) / 2;
    let mut cy = (i.rect.top + i.rect.bottom) / 2;
    let color = line_color(cpu, t);
    if t.style & TVS_HASLINES != 0 {
        cy = (cy + 2) & !1;
        dotted(cpu, hdc, (i.state_offset, cy), (cx - 1, cy), color);
        if i.prev != 0 || i.parent != t.root {
            dotted(cpu, hdc, (cx, i.rect.top), (cx, cy), color);
        }
        if i.next != 0 {
            dotted(cpu, hdc, (cx, cy), (cx, i.rect.bottom + 1), color);
        }
        let mut p = i.parent;
        while p != t.root && p != 0 {
            let pi = t.item(p);
            let pcx = (pi.lines_offset + pi.state_offset) / 2;
            if pi.next != 0 && pi.state_offset > pi.lines_offset {
                dotted(cpu, hdc, (pcx, i.rect.top), (pcx, i.rect.bottom + 1), color);
            }
            p = pi.parent;
        }
    }
    if t.style & TVS_HASBUTTONS != 0 && i.children != 0 {
        let brush = gdi32::create_solid_brush(cpu, &[bk]).unwrap_or(0);
        let old_brush = gfx::select(cpu, hdc, brush);
        let height = i.rect.bottom - i.rect.top;
        let width = i.state_offset - i.lines_offset;
        let size = height.min(width) / 4;
        let plus = (size + 1) * 3 / 4;
        let pen = gdi32::create_pen(cpu, &[PS_SOLID, 0, color]).unwrap_or(0);
        let old_pen = gfx::select(cpu, hdc, pen);
        rectangle(
            cpu,
            hdc,
            cx - size - 1,
            cy - size - 1,
            cx + size + 2,
            cy + size + 2,
        );
        gfx::select(cpu, hdc, old_pen);
        gdi32::delete_object(cpu, &[pen]);
        let pen = gdi32::create_pen(cpu, &[PS_SOLID, 0, text]).unwrap_or(0);
        let old_pen = gfx::select(cpu, hdc, pen);
        let expanded = i.state & TVIS_EXPANDED != 0;
        gfx::line(cpu, hdc, (cx - plus + 1, cy), (cx + plus, cy));
        if !expanded {
            gfx::line(cpu, hdc, (cx, cy - plus + 1), (cx, cy + plus));
        }
        gfx::select(cpu, hdc, old_pen);
        gdi32::delete_object(cpu, &[pen]);
        gfx::select(cpu, hdc, old_brush);
        gdi32::delete_object(cpu, &[brush]);
    }
}

/// The rest of TREEVIEW_DrawItem: lines, images, label, insert mark.
fn draw_item(cpu: &mut Cpu, t: &mut Tree, hdc: u32, h: u32, clr_text: u32, clr_bk: u32) {
    draw_lines(cpu, t, hdc, h);
    let old_text = gfx::set_text_color(cpu, hdc, clr_text);
    let old_bk = gfx::set_bk_color(cpu, hdc, clr_bk);
    let i = t.item(h).clone();
    let cy = (i.rect.top + i.rect.bottom) / 2;
    let state_index = state_image_index(i.state);
    if t.himl_state != 0 && state_index != 0 {
        let y = cy - t.state_height / 2;
        let index = state_index as i32;
        imagelist::draw(cpu, t.himl_state, index, hdc, i.state_offset, y, ILD_NORMAL);
    }
    let index = if i.state & TVIS_SELECTED != 0 && i.selected_image >= 0 {
        i.selected_image
    } else {
        i.image
    };
    if t.himl_normal != 0 {
        let y = cy - t.normal_height / 2;
        let style = ILD_NORMAL | (i.state & TVIS_OVERLAYMASK);
        imagelist::draw(cpu, t.himl_normal, index, hdc, i.image_offset, y, style);
    }
    if (t.edit == 0 || t.edit_item != h)
        && let Some(text) = &i.text
    {
        let rc = Rect::new(
            i.text_offset,
            i.rect.top,
            i.text_offset + i.text_width + 4,
            i.rect.bottom,
        );
        gfx::fill_rgb(cpu, hdc, rc, clr_bk);
        let mode = gfx::set_bk_mode(cpu, hdc, TRANSPARENT);
        let mut at = Rect::new(rc.left + 2, rc.top + 1, rc.right, rc.bottom);
        gfx::draw_text(
            cpu,
            hdc,
            text,
            &mut at,
            DT_LEFT | DT_SINGLELINE | DT_NOPREFIX,
        );
        gfx::set_bk_mode(cpu, hdc, mode);
        if h == t.selected && in_focus(cpu, t) {
            focus_rect(cpu, hdc, rc);
        }
    }
    if h == t.insert_mark {
        let color = if t.clr_insert == CLR_DEFAULT {
            gfx::sys_color(cpu, COLOR_BTNTEXT)
        } else {
            t.clr_insert
        };
        let pen = gdi32::create_pen(cpu, &[PS_SOLID, 2, color]).unwrap_or(0);
        let old = gfx::select(cpu, hdc, pen);
        let y = if t.insert_after {
            i.rect.bottom - 1
        } else {
            i.rect.top + 1
        };
        let left = i.text_offset - 2;
        let right = i.text_offset + i.text_width + 2;
        gfx::line(cpu, hdc, (left, y - 3), (left, y + 4));
        gfx::line(cpu, hdc, (left, y), (right + 1, y));
        gfx::line(cpu, hdc, (right, y + 3), (right, y - 4));
        gfx::select(cpu, hdc, old);
        gdi32::delete_object(cpu, &[pen]);
    }
    gfx::set_text_color(cpu, hdc, old_text);
    gfx::set_bk_color(cpu, hdc, old_bk);
}

/// DrawFocusRect on `hdc`.
fn focus_rect(cpu: &mut Cpu, hdc: u32, r: Rect) {
    let Some(dc) = gdi32::gdi(cpu).dc(hdc).cloned() else {
        return;
    };
    let (l, t) = dc.to_device(r.left, r.top);
    let (rr, b) = dc.to_device(r.right, r.bottom);
    gdi32::draw::focus_device_rect(cpu, hdc, Rect::new(l, t, rr, b));
}

/// WM_ERASEBKGND: the background in the tree's color.
fn erase(cpu: &mut Cpu, hwnd: u32, hdc: u32) -> u32 {
    let Some(t) = take(cpu, hwnd) else {
        return 1;
    };
    let (bk, _) = t.colors(cpu);
    let rc = super::client(cpu, hwnd);
    gfx::fill_rgb(cpu, hdc, rc, bk);
    put(cpu, t);
    1
}

/// WM_PAINT and WM_PRINTCLIENT: on `hdc`, or through BeginPaint.
fn paint(cpu: &mut Cpu, hwnd: u32, hdc: u32) -> Option<u32> {
    if hdc != 0 {
        if !peek(cpu, hwnd, |t| t.redraw).unwrap_or(false) {
            return Some(0);
        }
        let [l, t, r, b] = rect_words(super::client(cpu, hwnd));
        return start(cpu, hwnd, vec![st(W_PAINT, &[hdc, l, t, r, b])], 0);
    }
    let mut s = new_script(hwnd);
    s.call(
        user32::address_of("BeginPaint"),
        &[Arg::V(hwnd), Arg::S(S_PS)],
    );
    s.ops
        .push(then_steps(&[st(W_PAINT_BEGIN, &[]), st(W_END_PAINT, &[])]));
    s.ret(0);
    script::run(cpu, TREEVIEW, s)
}

// Sorting.

fn children_of(t: &Tree, parent: u32) -> Vec<u32> {
    let mut list = Vec::new();
    let mut child = t.item(parent).first_child;
    while child != 0 {
        list.push(child);
        child = t.item(child).next;
    }
    list
}

/// TVM_SORTCHILDREN (TREEVIEW_Sort on labels).
fn sort_children(cpu: &mut Cpu, t: &mut Tree, parent: u32) -> u32 {
    let parent = if parent == TVI_ROOT || parent == 0 {
        t.root
    } else {
        parent
    };
    if !t.valid(parent) {
        return 0;
    }
    let mut list = children_of(t, parent);
    if list.len() < 2 {
        return 0;
    }
    list.sort_by(|&a, &b| {
        let (ta, tb) = (&t.item(a).text, &t.item(b).text);
        match (ta, tb) {
            (Some(x), Some(y)) => compare(x, y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    });
    relink(cpu, t, parent, &list);
    1
}

/// Chain `parent`'s children in the order of `list` and show them so.
fn relink(cpu: &mut Cpu, t: &mut Tree, parent: u32, list: &[u32]) {
    for (n, &h) in list.iter().enumerate() {
        let prev = if n > 0 { list[n - 1] } else { 0 };
        let next = list.get(n + 1).copied().unwrap_or(0);
        let i = t.item_mut(h);
        i.prev = prev;
        i.next = next;
    }
    let p = t.item_mut(parent);
    p.first_child = list[0];
    p.last_child = list[list.len() - 1];
    if t.expanded(parent) {
        let order = t.order(t.first_visible);
        t.recalc_order(0);
        if t.is_child_of(parent, t.first_visible) {
            let mut h = t.first_child();
            while h != 0 && t.order(h) != order {
                h = t.next_list(h);
            }
            if h == 0 {
                h = t.item(parent).first_child;
            }
            let mut ops = Vec::new();
            set_first_visible(cpu, t, h, false, &mut ops);
        }
        invalidate(cpu, t.hwnd, true);
    }
}

/// A step of TVM_SORTCHILDRENCB: compare the next child with one of
/// those sorted, or put it in its place.
fn sort_step(cpu: &mut Cpu, t: &mut Tree, r: &mut Run, out: &mut Out) {
    loop {
        let done = t.sort_done.len();
        if done >= t.sort_items.len() {
            let list = std::mem::take(&mut t.sort_done);
            let items = std::mem::take(&mut t.sort_items);
            let parent = t.sort_parent;
            t.sort_parent = 0;
            // Items gone or added while the program compared: the order
            // stays as it is.
            let now = if t.valid(parent) {
                children_of(t, parent)
            } else {
                Vec::new()
            };
            if now == items && !list.is_empty() {
                relink(cpu, t, parent, &list);
                r.result = 1;
            } else {
                r.result = 0;
            }
            return;
        }
        if t.sort_lo < t.sort_hi {
            let mid = (t.sort_lo + t.sort_hi) / 2;
            let a = t.sort_items[done];
            let b = t.sort_done[mid as usize];
            let la = t.items.get(&a).map_or(0, |i| i.lparam);
            let lb = t.items.get(&b).map_or(0, |i| i.lparam);
            out.ops.push(Op::Call {
                target: t.sort_func,
                args: vec![Arg::V(la), Arg::V(lb), Arg::V(t.sort_param)],
            });
            out.push.push(st(W_SORT_GOT, &[mid]));
            return;
        }
        let a = t.sort_items[done];
        let at = t.sort_lo as usize;
        t.sort_done.insert(at, a);
        t.sort_lo = 0;
        t.sort_hi = t.sort_done.len() as u32;
    }
}

// The mouse.

/// The item and part at (x, y) (TREEVIEW_HitTest).
fn hit_test(cpu: &mut Cpu, t: &Tree, x: i32, y: i32) -> (u32, u32) {
    let rc = super::client(cpu, t.hwnd);
    let mut status = 0;
    if x < rc.left {
        status |= TVHT_TOLEFT;
    } else if x > rc.right {
        status |= TVHT_TORIGHT;
    }
    if y < rc.top {
        status |= TVHT_ABOVE;
    } else if y > rc.bottom {
        status |= TVHT_BELOW;
    }
    if status != 0 {
        return (0, status);
    }
    let h = t.hit_test_point(y);
    if h == 0 {
        return (0, TVHT_NOWHERE);
    }
    let i = t.item(h);
    let flags = if x >= i.text_offset + i.text_width {
        TVHT_ONITEMRIGHT
    } else if x >= i.text_offset {
        TVHT_ONITEMLABEL
    } else if x >= i.image_offset {
        TVHT_ONITEMICON
    } else if x >= i.state_offset {
        TVHT_ONITEMSTATEICON
    } else if x >= i.lines_offset && t.style & TVS_HASBUTTONS != 0 {
        TVHT_ONITEMBUTTON
    } else {
        TVHT_ONITEMINDENT
    };
    (h, flags)
}

fn toggle_state(cpu: &mut Cpu, t: &mut Tree, h: u32) {
    if t.style & TVS_CHECKBOXES == 0 || !t.valid(h) {
        return;
    }
    let i = t.item_mut(h);
    let mut s = state_image_index(i.state);
    i.state &= !TVIS_STATEIMAGEMASK;
    if s < 3 {
        s = [0, 2, 1][s as usize];
    }
    i.state |= s << 12;
    invalidate(cpu, t.hwnd, true);
}

/// WM_LBUTTONDOWN after NM_CLICK (when it comes first): the button, a
/// drag, or the rest of the click.
fn press(cpu: &mut Cpu, t: &mut Tree, h: u32, flags: u32, packed: u32, out: &mut Out) {
    let track = packed & 1 != 0;
    let edit = packed & 2 != 0;
    if flags & TVHT_ONITEMBUTTON != 0 {
        out.push.push(st(W_TOGGLE, &[h]));
        out.push.push(st(W_SET_FOCUS, &[]));
        return;
    }
    if track {
        // Windows waits here for the mouse to leave a small square or
        // the button to go up; the tree waits for those messages.
        t.press = 1;
        t.press_item = h;
        t.press_flags = flags;
        t.press_edit = edit;
        out.ops.push(user("SetCapture", &[t.hwnd]));
        return;
    }
    clicked(cpu, t, h, flags, edit, out);
}

/// The end of a click (the rest of TREEVIEW_LButtonDown).
fn clicked(cpu: &mut Cpu, t: &mut Tree, h: u32, flags: u32, edit: bool, out: &mut Out) {
    if edit {
        let time = user32::input::get_double_click_time(cpu, &[]).unwrap_or(500);
        user32::message::kill_timer(cpu, &[t.hwnd, TV_EDIT_TIMER]);
        user32::message::set_timer(cpu, &[t.hwnd, TV_EDIT_TIMER, time, 0]);
        t.edit_timer = true;
    } else if flags & (TVHT_ONITEMICON | TVHT_ONITEMLABEL) != 0 {
        out.push
            .push(st(W_SELECT, &[TVGN_CARET, h, TVC_BYMOUSE, 0]));
    } else if flags & TVHT_ONITEMSTATEICON != 0 {
        toggle_state(cpu, t, h);
    }
    out.push.push(st(W_SET_FOCUS, &[]));
}

fn lbutton_down(cpu: &mut Cpu, hwnd: u32, lp: u32) -> Option<u32> {
    let (x, y) = user32::point_of(lp);
    let steps = with(cpu, hwnd, |cpu, t| {
        if t.edit != 0 {
            return vec![st(W_SET_FOCUS, &[])];
        }
        let (h, flags) = hit_test(cpu, t, x, y);
        if h != 0 && flags & TVHT_ONITEM != 0 {
            t.focused = h;
            invalidate(cpu, t.hwnd, true);
        }
        let track = flags & TVHT_ONITEM != 0 && t.style & TVS_DISABLEDRAGDROP == 0;
        let edit =
            t.style & TVS_EDITLABELS != 0 && flags & TVHT_ONITEMLABEL != 0 && t.selected == h;
        t.press_x = x;
        t.press_y = y;
        let packed = track as u32 | (edit as u32) << 1;
        if track {
            vec![st(W_PRESS, &[h, flags, packed])]
        } else {
            vec![st(W_CLICK, &[h, flags, packed])]
        }
    })
    .unwrap_or_default();
    start(cpu, hwnd, steps, 0)
}

/// A button's release or a move while a press may become a drag.
fn pressed_mouse(cpu: &mut Cpu, hwnd: u32, msg: u32, lp: u32) -> Option<Option<u32>> {
    if !matches!(
        msg,
        WM_MOUSEMOVE | WM_LBUTTONUP | WM_RBUTTONUP | WM_CAPTURECHANGED
    ) {
        return None;
    }
    let (x, y) = user32::point_of(lp);
    let (button, h, flags, edit, px, py) = peek(cpu, hwnd, |t| {
        (
            t.press,
            t.press_item,
            t.press_flags,
            t.press_edit,
            t.press_x,
            t.press_y,
        )
    })?;
    if button == 0 {
        return None;
    }
    let metric = |cpu: &mut Cpu, n: u32| {
        user32::sysparams::get_system_metrics(cpu, &[n]).unwrap_or(4) as i32
    };
    let (cx, cy) = (metric(cpu, SM_CXDRAG), metric(cpu, SM_CYDRAG));
    if msg == WM_MOUSEMOVE {
        let inside = (px - cx..px + cx).contains(&x) && (py - cy..py + cy).contains(&y);
        if inside {
            return Some(Some(0));
        }
        let ops = with(cpu, hwnd, |cpu, t| {
            t.press = 0;
            let mut ops = vec![user("ReleaseCapture", &[])];
            if !t.valid(h) {
                return ops;
            }
            let code = if button == 1 {
                TVN_BEGINDRAGA
            } else {
                TVN_BEGINRDRAGA
            };
            let i = t.item(h);
            let mut body = vec![TVC_UNKNOWN];
            body.extend([0; 10]);
            let mask = TVIF_STATE | TVIF_PARAM | TVIF_HANDLE;
            body.extend([mask, h, i.state, 0xFFFF, 0, 0, 0, 0, 0, i.lparam]);
            body.extend([px as u32, py as u32]);
            ops.extend(notify(cpu, t, code, &body));
            if button == 1 && t.focused != 0 {
                t.focused = 0;
                invalidate(cpu, t.hwnd, true);
            }
            ops
        })
        .unwrap_or_default();
        return Some(finish(cpu, hwnd, ops, 0));
    }
    if let Some(t) = state(cpu).treeview.trees.get_mut(&hwnd) {
        t.press = 0;
    }
    let mut ops = if msg == WM_CAPTURECHANGED {
        Vec::new()
    } else {
        vec![user("ReleaseCapture", &[])]
    };
    if button == 2 {
        if msg == WM_RBUTTONUP {
            ops.push(user("SetFocus", &[hwnd]));
            ops.push(then_steps(&[st(W_RCLICK, &[])]));
            // WM_CONTEXTMENU follows when the parent leaves NM_RCLICK
            // to the tree; the point is on the screen.
            let screen = user32::user(cpu).screen_client(hwnd);
            let pt = user32::make_long(x + screen.left, y + screen.top);
            ops.push(send(hwnd, WM_CONTEXTMENU, hwnd, Arg::V(pt)));
        }
        return Some(finish(cpu, hwnd, ops, 0));
    }
    if msg == WM_LBUTTONUP {
        ops.push(then_steps(&[st(W_RELEASE, &[h, flags, edit as u32])]));
    }
    Some(finish(cpu, hwnd, ops, 0))
}

fn rbutton_down(cpu: &mut Cpu, hwnd: u32, lp: u32) -> Option<u32> {
    let (x, y) = user32::point_of(lp);
    let ops = with(cpu, hwnd, |cpu, t| {
        if t.edit != 0 {
            return vec![user("SetFocus", &[t.hwnd])];
        }
        let (h, _) = hit_test(cpu, t, x, y);
        t.press = 2;
        t.press_item = h;
        t.press_flags = 0;
        t.press_edit = false;
        t.press_x = x;
        t.press_y = y;
        vec![user("SetCapture", &[t.hwnd])]
    })
    .unwrap_or_default();
    finish(cpu, hwnd, ops, 0)
}

fn lbutton_dblclk(cpu: &mut Cpu, hwnd: u32, lp: u32) -> Option<u32> {
    let (x, y) = user32::point_of(lp);
    let mut ops = vec![user("SetFocus", &[hwnd])];
    with(cpu, hwnd, |cpu, t| {
        if t.edit_timer {
            user32::message::kill_timer(cpu, &[t.hwnd, TV_EDIT_TIMER]);
            t.edit_timer = false;
        }
        let (h, flags) = hit_test(cpu, t, x, y);
        if h != 0 {
            ops.extend(notify(cpu, t, NM_DBLCLK, &[]));
            ops.push(then_steps(&[st(W_DBLCLK_ASKED, &[h, flags, x as u32])]));
        }
    });
    finish(cpu, hwnd, ops, 0)
}

fn mouse_move(cpu: &mut Cpu, hwnd: u32, lp: u32) -> Option<u32> {
    let (_, y) = user32::point_of(lp);
    with(cpu, hwnd, |cpu, t| {
        if t.style & TVS_TRACKSELECT == 0 {
            return;
        }
        let h = t.hit_test_point(y);
        if h != t.hot {
            t.hot = h;
            invalidate(cpu, t.hwnd, true);
        }
    });
    Some(0)
}

fn set_cursor(cpu: &mut Cpu, hwnd: u32, wp: u32, lp: u32) -> Option<u32> {
    let (_, cursor_y) = user32::user(cpu).input.cursor;
    let screen = user32::user(cpu).screen_client(hwnd);
    let y = cursor_y - screen.top;
    let ops = with(cpu, hwnd, |cpu, t| {
        let h = t.hit_test_point(y);
        let lparam = t.items.get(&h).map_or(0, |i| i.lparam);
        let mut ops = notify(cpu, t, NM_SETCURSOR, &[h, lparam, 0, 0, lp]);
        ops.push(then_steps(&[st(W_SETCURSOR_ASKED, &[h, wp, lp])]));
        ops
    })
    .unwrap_or_default();
    finish(cpu, hwnd, ops, 0)
}

// The keyboard.

/// WM_KEYDOWN's work, after TVN_KEYDOWN (TREEVIEW_KeyDown).
fn key_down(cpu: &mut Cpu, t: &mut Tree, key: u32, out: &mut Out) {
    let ctrl = user32::input::get_async_key_state(cpu, &[VK_CONTROL]).unwrap_or(0) & 0x8000 != 0;
    if ctrl {
        let (bar, code) = match key {
            VK_PRIOR => (SB_VERT, SB_PAGEUP),
            VK_NEXT => (SB_VERT, SB_PAGEDOWN),
            VK_END => (SB_VERT, SB_BOTTOM),
            VK_HOME => (SB_VERT, SB_TOP),
            VK_LEFT => (SB_HORZ, SB_LINEUP),
            VK_UP => (SB_VERT, SB_LINEUP),
            VK_RIGHT => (SB_HORZ, SB_LINEDOWN),
            VK_DOWN => (SB_VERT, SB_LINEDOWN),
            _ => return,
        };
        if bar == SB_HORZ {
            hscroll(cpu, t, code, 0, &mut out.ops);
        } else {
            vscroll(cpu, t, code, 0, &mut out.ops);
        }
        return;
    }
    let prev = t.selected;
    if !t.valid(prev) {
        return;
    }
    let mut new = 0;
    match key {
        VK_UP => {
            new = t.prev_list(prev);
            if new == 0 {
                new = t.first_child();
            }
        }
        VK_DOWN => new = t.next_list(prev),
        VK_HOME => new = t.first_child(),
        VK_END => new = t.last_list(t.root),
        VK_LEFT => {
            if t.expanded(prev) && t.item(prev).first_child != 0 {
                out.push.push(st(W_COLLAPSE, &[prev, EX_USER]));
            } else if t.item(prev).parent != t.root {
                new = t.item(prev).parent;
            }
        }
        VK_RIGHT => {
            if t.item(prev).children > 0 {
                if !t.expanded(prev) {
                    out.push.push(st(W_EXPAND, &[prev, EX_USER]));
                } else {
                    new = t.item(prev).first_child;
                }
            }
        }
        VK_MULTIPLY => out.push.push(st(W_EXPAND_ALL, &[prev])),
        VK_ADD => {
            if !t.expanded(prev) {
                out.push.push(st(W_EXPAND, &[prev, EX_USER]));
            }
        }
        VK_SUBTRACT => {
            if t.expanded(prev) {
                out.push.push(st(W_COLLAPSE, &[prev, EX_USER]));
            }
        }
        VK_PRIOR => new = t.list_item(prev, -t.visible_count()),
        VK_NEXT => new = t.list_item(prev, t.visible_count()),
        VK_BACK => {
            new = t.item(prev).parent;
            if new == t.root {
                new = 0;
            }
        }
        VK_SPACE => toggle_state(cpu, t, prev),
        _ => {}
    }
    if new != 0 && new != prev {
        out.push
            .push(st(W_SELECT, &[TVGN_CARET, new, TVC_BYKEYBOARD, 0]));
    }
}

fn keydown(cpu: &mut Cpu, hwnd: u32, wp: u32, lp: u32) -> Option<u32> {
    // The parent hears of the key first.
    let ops = with(cpu, hwnd, |cpu, t| {
        let mut ops = notify(cpu, t, TVN_KEYDOWN, &[wp & 0xFFFF, lp]);
        ops.push(then_steps(&[st(W_KEY, &[wp])]));
        ops
    })
    .unwrap_or_default();
    finish(cpu, hwnd, ops, 0)
}

/// WM_CHAR: find the item whose label starts with what was typed
/// (TREEVIEW_ProcessLetterKeys).
fn letter(cpu: &mut Cpu, hwnd: u32, ch: u32, data: u32) -> Option<u32> {
    if ch == 0 || data == 0 || ch > 0xFF {
        return Some(0);
    }
    let c = ch as u8;
    if !c.is_ascii_alphanumeric() && !b".`!@#$%^&*()-_+=\\]}[{/?><,~".contains(&c) {
        return Some(0);
    }
    let now = super::kernel32(cpu, "GetTickCount", &[]).unwrap_or(0);
    let steps = with(cpu, hwnd, |_, t| {
        let elapsed = now.wrapping_sub(t.last_key);
        t.last_key = now;
        let mut ch = ch;
        if elapsed < KEY_DELAY {
            if t.search.len() < 260 {
                t.search.push(c);
            }
            if t.char_code != ch {
                t.char_code = 0;
                ch = 0;
            }
        } else {
            t.char_code = ch;
            t.search = vec![c];
            ch = 0;
        }
        let (end, mut idx) = if t.valid(t.selected) {
            let end = t.selected;
            let idx = if t.search.len() == 1 {
                t.next_list(end)
            } else {
                end
            };
            (end, idx)
        } else {
            (0, t.first_child())
        };
        let mut found = 0;
        let n = t.search.len();
        let starts = |text: &[u8], n: usize, search: &[u8]| {
            text.len() >= n && compare_i(&text[..n], &search[..n]) == Ordering::Equal
        };
        loop {
            if idx == 0 {
                if end == 0 {
                    break;
                }
                idx = t.first_child();
                if idx == end || idx == 0 {
                    break;
                }
            }
            let text = t.item(idx).text.clone().unwrap_or_default();
            if starts(&text, n, &t.search) {
                found = idx;
                break;
            } else if ch != 0 && found == 0 && starts(&text, 1, &t.search) {
                found = idx;
            }
            idx = t.next_list(idx);
            if idx == end {
                break;
            }
        }
        if found != 0 {
            vec![st(W_SELECT, &[TVGN_CARET, found, TVC_BYKEYBOARD, 0])]
        } else {
            Vec::new()
        }
    })
    .unwrap_or_default();
    start(cpu, hwnd, steps, 0)
}

// Creating and destroying.

/// The image list of check boxes TVS_CHECKBOXES gives the state images
/// (initialize_checkboxes).
fn checkboxes(cpu: &mut Cpu, t: &mut Tree) {
    let himl = imagelist::create(cpu, 16, 16, ILC_COLOR | ILC_MASK, 3, 0);
    let gdi = |cpu: &mut Cpu, name: &str, args: &[u32]| {
        super::call_builtin(cpu, "GDI32.DLL", name, args).unwrap_or(0)
    };
    let screen = get_dc(cpu, 0);
    let hdc = gdi(cpu, "CreateCompatibleDC", &[screen]);
    let bmp = gdi(cpu, "CreateCompatibleBitmap", &[screen, 48, 16]);
    let old = gfx::select(cpu, hdc, bmp);
    let white = gfx::sys_color(cpu, COLOR_WINDOW);
    gfx::fill_rgb(cpu, hdc, Rect::new(0, 0, 48, 16), white);
    let flat = DFCS_BUTTONCHECK | DFCS_FLAT;
    let _ = user32::paint::frame_control(cpu, hdc, Rect::new(18, 2, 30, 14), DFC_BUTTON, flat);
    let checked = flat | DFCS_CHECKED;
    let _ = user32::paint::frame_control(cpu, hdc, Rect::new(34, 2, 46, 14), DFC_BUTTON, checked);
    gfx::select(cpu, hdc, old);
    imagelist::add_masked(cpu, himl, bmp, white);
    gdi32::delete_object(cpu, &[bmp]);
    gdi(cpu, "DeleteDC", &[hdc]);
    release_dc(cpu, 0, screen);
    t.himl_state = himl;
    t.state_width = 16;
    t.state_height = 16;
}

/// WM_CREATE (TREEVIEW_Create).
fn create(cpu: &mut Cpu, hwnd: u32, cs: u32) -> Option<u32> {
    let w = read_words(cpu, cs, 12);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let parent = w[3];
    let style = super::style(cpu, hwnd);
    let rc = super::client(cpu, hwnd);
    let log = user32::sysparams::sans_serif(false);
    let default_font = gdi32::add(
        cpu,
        gdi32::Kind::Font {
            font: gdi32::Font { log },
        },
    );
    let root = new_handle(cpu);
    let mut t = Tree {
        hwnd,
        notify: parent,
        style,
        root,
        scroll_time: 300,
        redraw: true,
        client_width: rc.width(),
        client_height: rc.height(),
        indent: MINIMUM_INDENT,
        clr_bk: CLR_NONE,
        clr_text: CLR_NONE,
        clr_line: CLR_DEFAULT,
        clr_insert: CLR_DEFAULT,
        font: default_font,
        default_font,
        ..Tree::default()
    };
    t.bold_font = font_like(cpu, default_font, true);
    t.underline_font = font_like(cpu, default_font, false);
    t.item_height = natural_height(cpu, &t);
    t.items.insert(
        root,
        Item {
            state: TVIS_EXPANDED,
            level: -1,
            visible_order: -1,
            integral: 1,
            ..Item::default()
        },
    );
    if style & TVS_CHECKBOXES != 0 {
        checkboxes(cpu, &mut t);
    }
    put(cpu, t);
    let mut s = new_script(hwnd);
    s.send(parent, WM_NOTIFYFORMAT, Arg::V(hwnd), Arg::V(NF_QUERY));
    s.ops.push(then_steps(&[st(W_NOTIFY_FORMAT, &[])]));
    if style & TVS_NOTOOLTIPS == 0 {
        s.ops
            .push(write(S_CLASS, script::words_of(b"tooltips_class32\0")));
        let d = CW_USEDEFAULT;
        s.call(
            user32::address_of("CreateWindowExA"),
            &[
                Arg::V(0),
                Arg::S(S_CLASS),
                Arg::V(0),
                Arg::V(WS_POPUP),
                Arg::V(d),
                Arg::V(d),
                Arg::V(d),
                Arg::V(d),
                Arg::V(hwnd),
                Arg::V(0),
                Arg::V(0),
                Arg::V(0),
            ],
        );
        s.ops.push(then_steps(&[st(W_TOOLTIP, &[])]));
    }
    s.user("ShowScrollBar", &[hwnd, SB_VERT, 0]);
    s.user("ShowScrollBar", &[hwnd, SB_HORZ, 0]);
    s.ret(0);
    script::run(cpu, TREEVIEW, s)
}

/// WM_DESTROY (TREEVIEW_Destroy).
fn destroy(cpu: &mut Cpu, hwnd: u32) {
    let Some(t) = take(cpu, hwnd) else {
        return;
    };
    if t.edit != 0
        && let Some(e) = state(cpu).treeview.edits.remove(&t.edit)
        && let Some(w) = user32::user(cpu).window_mut(t.edit)
    {
        w.proc = e.proc;
    }
    if t.edit_timer {
        user32::message::kill_timer(cpu, &[hwnd, TV_EDIT_TIMER]);
    }
    for f in [t.default_font, t.bold_font, t.underline_font] {
        if f != 0 {
            gdi32::delete_object(cpu, &[f]);
        }
    }
}

/// TVM_SETIMAGELIST (TREEVIEW_SetImageList).
fn set_image_list(cpu: &mut Cpu, t: &mut Tree, which: u32, himl: u32, ops: &mut Vec<Op>) -> u32 {
    let (old_w, old_h) = (t.normal_width, t.normal_height);
    let size = if himl != 0 {
        imagelist::icon_size(cpu, himl).unwrap_or((0, 0))
    } else {
        (0, 0)
    };
    let old = match which {
        TVSIL_NORMAL => {
            (t.normal_width, t.normal_height) = size;
            std::mem::replace(&mut t.himl_normal, himl)
        }
        TVSIL_STATE => {
            (t.state_width, t.state_height) = size;
            std::mem::replace(&mut t.himl_state, himl)
        }
        _ => return 0,
    };
    if old_w != t.normal_width || old_h != t.normal_height {
        let mut recalc = false;
        if old_h != t.normal_height && !t.height_set {
            t.item_height = natural_height(cpu, t);
            recalc = true;
        }
        if t.normal_width > MINIMUM_INDENT as i32 && t.normal_width as u32 != t.indent {
            t.indent = t.normal_width as u32;
            recalc = true;
        }
        if recalc {
            t.recalc_order(0);
        }
    }
    let root = t.root;
    update_subtree(cpu, t, root);
    update_scroll_bars(cpu, t, ops);
    invalidate(cpu, t.hwnd, true);
    old
}

/// WM_SETFONT (TREEVIEW_SetFont).
fn set_font(cpu: &mut Cpu, t: &mut Tree, font: u32, redraw: bool, ops: &mut Vec<Op>) {
    let height = t.item_height;
    t.font = if font != 0 { font } else { t.default_font };
    for f in [t.bold_font, t.underline_font] {
        if f != 0 {
            gdi32::delete_object(cpu, &[f]);
        }
    }
    t.bold_font = font_like(cpu, t.font, true);
    t.underline_font = font_like(cpu, t.font, false);
    if !t.height_set {
        t.item_height = natural_height(cpu, t);
    }
    if height != t.item_height {
        t.recalc_order(0);
    }
    for i in t.items.values_mut() {
        i.text_width = 0;
    }
    let root = t.root;
    update_subtree(cpu, t, root);
    update_scroll_bars(cpu, t, ops);
    if redraw {
        invalidate(cpu, t.hwnd, true);
    }
}

/// TVM_CREATEDRAGIMAGE: an image list of the item's image and label.
fn create_drag_image(cpu: &mut Cpu, t: &mut Tree, h: u32) -> u32 {
    if t.himl_normal == 0 || !t.user_item(h) {
        return 0;
    }
    let gdi = |cpu: &mut Cpu, name: &str, args: &[u32]| {
        super::call_builtin(cpu, "GDI32.DLL", name, args).unwrap_or(0)
    };
    let screen = get_dc(cpu, 0);
    let hdc = gdi(cpu, "CreateCompatibleDC", &[screen]);
    let old_font = gfx::select(cpu, hdc, t.font);
    let text = t.item(h).text.clone().unwrap_or_default();
    let (tw, th) = gfx::text_size(cpu, hdc, &text);
    let (cx, cy) = imagelist::icon_size(cpu, t.himl_normal).unwrap_or((0, 0));
    let (w, height) = (tw + cx, th.max(cy));
    let bmp = gdi(
        cpu,
        "CreateCompatibleBitmap",
        &[screen, w as u32, height as u32],
    );
    let old = gfx::select(cpu, hdc, bmp);
    let list = imagelist::create(cpu, w, height, ILC_COLOR, 10, 10);
    imagelist::draw(cpu, t.himl_normal, t.item(h).image, hdc, 0, 0, ILD_NORMAL);
    let mut rc = Rect::new(cx, 0, w, height);
    gfx::draw_text(cpu, hdc, &text, &mut rc, DT_LEFT);
    gfx::select(cpu, hdc, old_font);
    gfx::select(cpu, hdc, old);
    imagelist::il_add(cpu, &[list, bmp, 0]);
    gdi(cpu, "DeleteDC", &[hdc]);
    gdi32::delete_object(cpu, &[bmp]);
    release_dc(cpu, 0, screen);
    list
}

/// TVM_GETNEXTITEM (TREEVIEW_GetNextItem).
fn get_next_item(t: &Tree, which: u32, h: u32) -> u32 {
    match which {
        TVGN_ROOT => return t.first_child(),
        TVGN_CARET => return t.selected,
        TVGN_FIRSTVISIBLE => return t.first_visible,
        TVGN_DROPHILITE => return t.drop_item,
        TVGN_LASTVISIBLE => return t.last_list(t.root),
        TVGN_CHILD if h == 0 || h == TVI_ROOT => return t.first_child(),
        _ => {}
    }
    if !t.user_item(h) {
        return 0;
    }
    let i = t.item(h);
    match which {
        TVGN_NEXT => i.next,
        TVGN_PREVIOUS => i.prev,
        TVGN_PARENT => {
            if i.parent != t.root {
                i.parent
            } else {
                0
            }
        }
        TVGN_CHILD => i.first_child,
        TVGN_NEXTVISIBLE if t.shown(h) => t.next_list(h),
        TVGN_PREVIOUSVISIBLE if t.shown(h) => t.prev_list(h),
        _ => 0,
    }
}

// The window procedure.

/// Whether `h` is an item of the tree of `hwnd` the program may name.
fn has_item(cpu: &mut Cpu, hwnd: u32, h: u32) -> bool {
    peek(cpu, hwnd, |t| t.user_item(h)).unwrap_or(false)
}

/// The window procedure.
pub fn proc_(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, msg, wp, lp] = args else {
        return Some(0);
    };
    if msg == WM_CREATE {
        if state(cpu).treeview.trees.contains_key(&hwnd) {
            return Some(0);
        }
        return create(cpu, hwnd, lp);
    }
    if !state(cpu).treeview.trees.contains_key(&hwnd) {
        return defwnd::def_window_proc(cpu, args);
    }
    if let Some(done) = pressed_mouse(cpu, hwnd, msg, lp) {
        return done;
    }
    // Messages that only read or change the tree.
    let simple = |cpu: &mut Cpu, f: &mut dyn FnMut(&mut Cpu, &mut Tree) -> u32| {
        with(cpu, hwnd, |cpu, t| f(cpu, t)).unwrap_or(0)
    };
    // What they make the program do (the scroll bars).
    let mut ops = Vec::new();
    let result = match msg {
        TVM_INSERTITEMA => return insert_item(cpu, hwnd, lp),
        TVM_SETITEMA => return set_item(cpu, hwnd, lp),
        TVM_GETITEMA => {
            let w = read_words(cpu, lp, 2);
            if cpu.bus.guest_faulted() {
                return None;
            }
            let steps = vec![st(W_DISPINFO, &[w[1], w[0]]), st(W_GETITEM, &[lp])];
            return start(cpu, hwnd, steps, 0);
        }
        TVM_DELETEITEM => {
            if lp != TVI_ROOT && lp != 0 && !has_item(cpu, hwnd, lp) {
                return Some(0);
            }
            return start(cpu, hwnd, vec![st(W_DELETE, &[lp])], 1);
        }
        TVM_EXPAND => {
            if !has_item(cpu, hwnd, lp) {
                return Some(0);
            }
            let step = match wp & TVE_TOGGLE {
                TVE_COLLAPSE => {
                    let reset = if wp & TVE_COLLAPSERESET != 0 {
                        EX_RESET
                    } else {
                        0
                    };
                    st(W_COLLAPSE, &[lp, EX_RET | reset])
                }
                TVE_EXPAND if wp & TVE_EXPANDPARTIAL != 0 => {
                    // Windows 95's COMCTL32 doesn't know partial
                    // expanding; it shows the children as the plain one.
                    st(W_EXPAND, &[lp, EX_RET])
                }
                TVE_EXPAND => st(W_EXPAND, &[lp, EX_RET]),
                TVE_TOGGLE => st(W_TOGGLE, &[lp, EX_RET]),
                _ => return Some(0),
            };
            return start(cpu, hwnd, vec![step], 0);
        }
        TVM_SELECTITEM => {
            if lp != 0 && !has_item(cpu, hwnd, lp) {
                return Some(0);
            }
            return start(cpu, hwnd, vec![st(W_SELECT, &[wp, lp, TVC_UNKNOWN, 1])], 1);
        }
        TVM_ENSUREVISIBLE => return start(cpu, hwnd, vec![st(W_ENSURE, &[lp, 1, 1])], 0),
        TVM_EDITLABELA => return start(cpu, hwnd, vec![st(W_EDIT, &[lp])], 0),
        TVM_ENDEDITLABELNOW => {
            let step = st(W_END_EDIT, &[(wp != 0) as u32, 1]);
            return start(cpu, hwnd, vec![step], 0);
        }
        TVM_SORTCHILDRENCB => {
            let w = read_words(cpu, lp, 3);
            if cpu.bus.guest_faulted() {
                return None;
            }
            let go = with(cpu, hwnd, |_, t| {
                let parent = if w[0] == TVI_ROOT || w[0] == 0 {
                    t.root
                } else {
                    w[0]
                };
                if !t.valid(parent) || t.sort_parent != 0 || w[1] == 0 {
                    return false;
                }
                let list = children_of(t, parent);
                if list.len() < 2 {
                    return false;
                }
                t.sort_parent = parent;
                t.sort_func = w[1];
                t.sort_param = w[2];
                t.sort_done = vec![list[0]];
                t.sort_items = list;
                t.sort_lo = 0;
                t.sort_hi = 1;
                true
            })
            .unwrap_or(false);
            if !go {
                return Some(0);
            }
            return start(cpu, hwnd, vec![st(W_SORT, &[])], 0);
        }
        TVM_SORTCHILDREN => simple(cpu, &mut |cpu, t| sort_children(cpu, t, lp)),
        TVM_GETITEMRECT => {
            if lp == 0 {
                return Some(0);
            }
            let h = cpu.bus.guest_read_32(lp);
            if cpu.bus.guest_faulted() {
                return None;
            }
            simple(cpu, &mut |cpu, t| {
                if !t.user_item(h) || !t.shown(h) {
                    return 0;
                }
                if wp != 0 && t.item(h).text_width == 0 {
                    measure(cpu, t, h, 0);
                }
                let i = t.item(h);
                let rc = if wp != 0 {
                    Rect::new(
                        i.text_offset,
                        i.rect.top,
                        i.text_offset + i.text_width + 4,
                        i.rect.bottom,
                    )
                } else {
                    i.rect
                };
                rc.write(cpu, lp);
                1
            })
        }
        TVM_GETCOUNT => simple(cpu, &mut |_, t| t.count),
        TVM_GETINDENT => simple(cpu, &mut |_, t| t.indent),
        TVM_SETINDENT => {
            let new = wp.max(MINIMUM_INDENT);
            with(cpu, hwnd, |cpu, t| {
                if t.indent != new {
                    t.indent = new;
                    let root = t.root;
                    update_subtree(cpu, t, root);
                    update_scroll_bars(cpu, t, &mut ops);
                    invalidate(cpu, hwnd, true);
                }
            });
            0
        }
        TVM_GETIMAGELIST => simple(cpu, &mut |_, t| match wp {
            TVSIL_NORMAL => t.himl_normal,
            TVSIL_STATE => t.himl_state,
            _ => 0,
        }),
        TVM_SETIMAGELIST => {
            with(cpu, hwnd, |cpu, t| set_image_list(cpu, t, wp, lp, &mut ops)).unwrap_or(0)
        }
        TVM_GETNEXTITEM => simple(cpu, &mut |_, t| get_next_item(t, wp, lp)),
        TVM_GETEDITCONTROL => simple(cpu, &mut |_, t| t.edit),
        TVM_GETVISIBLECOUNT => simple(cpu, &mut |_, t| t.visible_count() as u32),
        TVM_HITTEST => {
            let w = read_words(cpu, lp, 4);
            if cpu.bus.guest_faulted() {
                return None;
            }
            simple(cpu, &mut |cpu, t| {
                let (h, flags) = hit_test(cpu, t, w[0] as i32, w[1] as i32);
                write_words(cpu, lp + 8, &[flags, h]);
                h
            })
        }
        TVM_CREATEDRAGIMAGE => simple(cpu, &mut |cpu, t| create_drag_image(cpu, t, lp)),
        TVM_SETTOOLTIPS => simple(cpu, &mut |_, t| std::mem::replace(&mut t.tooltip, wp)),
        TVM_GETTOOLTIPS => simple(cpu, &mut |_, t| t.tooltip),
        TVM_SETINSERTMARK => simple(cpu, &mut |cpu, t| {
            if lp != 0 && !t.user_item(lp) {
                return 0;
            }
            t.insert_after = wp != 0;
            t.insert_mark = lp;
            invalidate(cpu, hwnd, true);
            1
        }),
        TVM_SETITEMHEIGHT => with(cpu, hwnd, |cpu, t| {
            let prev = t.item_height;
            let new = wp as i16 as i32;
            if new == -1 {
                t.item_height = natural_height(cpu, t);
                t.height_set = false;
            } else if new != 0 {
                t.item_height = new;
                t.height_set = true;
            }
            if t.style & TVS_NONEVENHEIGHT == 0 && t.item_height > 1 {
                t.item_height &= !1;
            }
            if t.item_height != prev {
                t.recalc_order(0);
                update_scroll_bars(cpu, t, &mut ops);
                invalidate(cpu, hwnd, true);
            }
            prev as u32
        })
        .unwrap_or(0),
        TVM_GETITEMHEIGHT => simple(cpu, &mut |_, t| t.item_height as u32),
        TVM_SETBKCOLOR => simple(cpu, &mut |cpu, t| {
            let old = std::mem::replace(&mut t.clr_bk, lp);
            if old != lp {
                invalidate(cpu, hwnd, true);
            }
            old
        }),
        TVM_GETBKCOLOR => simple(cpu, &mut |_, t| t.clr_bk),
        TVM_SETTEXTCOLOR => simple(cpu, &mut |cpu, t| {
            let old = std::mem::replace(&mut t.clr_text, lp);
            if old != lp {
                invalidate(cpu, hwnd, true);
            }
            old
        }),
        TVM_GETTEXTCOLOR => simple(cpu, &mut |_, t| t.clr_text),
        TVM_SETSCROLLTIME => simple(cpu, &mut |_, t| {
            let old = t.scroll_time;
            t.scroll_time = wp.min(100);
            old
        }),
        TVM_GETSCROLLTIME => simple(cpu, &mut |_, t| t.scroll_time),
        TVM_SETINSERTMARKCOLOR => simple(cpu, &mut |_, t| std::mem::replace(&mut t.clr_insert, lp)),
        TVM_GETINSERTMARKCOLOR => simple(cpu, &mut |_, t| t.clr_insert),
        TVM_GETITEMSTATE => simple(cpu, &mut |_, t| {
            if t.user_item(wp) {
                t.item(wp).state & lp
            } else {
                0
            }
        }),
        TVM_SETUNICODEFORMAT => simple(cpu, &mut |_, t| {
            std::mem::replace(&mut t.unicode, wp != 0) as u32
        }),
        TVM_GETUNICODEFORMAT => simple(cpu, &mut |_, t| t.unicode as u32),
        WM_CHAR => return letter(cpu, hwnd, wp, lp),
        WM_COMMAND => return command(cpu, hwnd, wp, lp),
        WM_DESTROY => {
            destroy(cpu, hwnd);
            0
        }
        WM_NCDESTROY => {
            destroy(cpu, hwnd);
            return defwnd::def_window_proc(cpu, args);
        }
        WM_ERASEBKGND => erase(cpu, hwnd, wp),
        WM_GETDLGCODE => DLGC_WANTARROWS | DLGC_WANTCHARS,
        WM_GETFONT => simple(cpu, &mut |_, t| t.font),
        WM_HSCROLL => {
            with(cpu, hwnd, |cpu, t| {
                hscroll(cpu, t, wp & 0xFFFF, (wp >> 16) as i16 as i32, &mut ops)
            });
            0
        }
        WM_VSCROLL => {
            with(cpu, hwnd, |cpu, t| {
                vscroll(cpu, t, wp & 0xFFFF, (wp >> 16) as i16 as i32, &mut ops)
            });
            0
        }
        WM_MOUSEWHEEL => {
            if wp & (MK_SHIFT | MK_CONTROL) != 0 {
                return defwnd::def_window_proc(cpu, args);
            }
            with(cpu, hwnd, |cpu, t| {
                if !t.valid(t.first_visible) {
                    return;
                }
                let delta = -((wp >> 16) as i16 as i32);
                let lines = 3 * (delta / 120);
                if lines != 0 {
                    let top = t.max_visible_order.max(0);
                    let new = (t.order(t.first_visible) + lines).clamp(0, top);
                    vscroll(cpu, t, SB_THUMBPOSITION, new, &mut ops);
                }
            });
            1
        }
        WM_KEYDOWN => return keydown(cpu, hwnd, wp, lp),
        WM_KILLFOCUS => {
            let selected = simple(cpu, &mut |_, t| t.selected);
            let mut s = new_script(hwnd);
            if selected != 0 {
                invalidate(cpu, hwnd, true);
                s.user("UpdateWindow", &[hwnd]);
            }
            s.ops.push(then_steps(&[st(W_FOCUS_TELL, &[NM_KILLFOCUS])]));
            s.ret(0);
            return script::run(cpu, TREEVIEW, s);
        }
        WM_SETFOCUS => {
            let first = simple(cpu, &mut |_, t| {
                if t.selected == 0 { t.first_visible } else { 0 }
            });
            let mut steps = Vec::new();
            if first != 0 {
                steps.push(st(W_SELECT, &[TVGN_CARET, first, TVC_UNKNOWN, 0]));
            }
            steps.push(st(W_FOCUS_TELL, &[NM_SETFOCUS]));
            return start(cpu, hwnd, steps, 0);
        }
        WM_LBUTTONDBLCLK => return lbutton_dblclk(cpu, hwnd, lp),
        WM_LBUTTONDOWN => return lbutton_down(cpu, hwnd, lp),
        WM_RBUTTONDOWN => return rbutton_down(cpu, hwnd, lp),
        WM_MOUSEMOVE => return mouse_move(cpu, hwnd, lp),
        WM_NOTIFYFORMAT => {
            if lp != NF_REQUERY {
                return Some(0);
            }
            let mut s = new_script(hwnd);
            s.send(wp, WM_NOTIFYFORMAT, Arg::V(hwnd), Arg::V(NF_QUERY));
            s.ops.push(then_steps(&[st(W_NOTIFY_FORMAT, &[])]));
            return script::run(cpu, TREEVIEW, s);
        }
        WM_PRINTCLIENT => {
            if lp & PRF_CHECKVISIBLE != 0 && !user32::user(cpu).is_visible(hwnd) {
                return Some(0);
            }
            if lp & PRF_ERASEBKGND != 0 {
                erase(cpu, hwnd, wp);
            }
            if lp & PRF_CLIENT != 0 {
                return paint(cpu, hwnd, wp);
            }
            0
        }
        WM_PAINT => return paint(cpu, hwnd, wp),
        WM_SETCURSOR => return set_cursor(cpu, hwnd, wp, lp),
        WM_SETFONT => {
            with(cpu, hwnd, |cpu, t| set_font(cpu, t, wp, lp != 0, &mut ops));
            0
        }
        WM_SETREDRAW => {
            with(cpu, hwnd, |cpu, t| {
                t.redraw = wp != 0;
                if t.redraw {
                    let root = t.root;
                    update_subtree(cpu, t, root);
                    t.recalc_order(0);
                    update_scroll_bars(cpu, t, &mut ops);
                    invalidate(cpu, hwnd, true);
                }
            });
            0
        }
        WM_SIZE => {
            with(cpu, hwnd, |cpu, t| {
                if wp == SIZE_RESTORED {
                    t.client_width = lp as i16 as i32;
                    t.client_height = (lp >> 16) as i16 as i32;
                    t.recalc_order(0);
                    let first = t.first_visible;
                    set_first_visible(cpu, t, first, true, &mut ops);
                    update_scroll_bars(cpu, t, &mut ops);
                }
                invalidate(cpu, hwnd, true);
            });
            0
        }
        WM_STYLECHANGED => {
            if wp == GWL_STYLE {
                let new = cpu.bus.guest_read_32(lp + 4);
                if cpu.bus.guest_faulted() {
                    return None;
                }
                with(cpu, hwnd, |cpu, t| {
                    if (t.style ^ new) & TVS_CHECKBOXES != 0 && new & TVS_CHECKBOXES != 0 {
                        checkboxes(cpu, t);
                    }
                    if (t.style ^ new) & TVS_NOTOOLTIPS != 0
                        && new & TVS_NOTOOLTIPS != 0
                        && t.tooltip != 0
                    {
                        ops.push(user("DestroyWindow", &[t.tooltip]));
                        t.tooltip = 0;
                    }
                    t.style = new;
                });
            }
            with(cpu, hwnd, |cpu, t| {
                let root = t.root;
                update_subtree(cpu, t, root);
                update_scroll_bars(cpu, t, &mut ops);
                invalidate(cpu, hwnd, true);
            });
            0
        }
        WM_TIMER => {
            if wp != TV_EDIT_TIMER {
                return Some(1);
            }
            user32::message::kill_timer(cpu, &[hwnd, TV_EDIT_TIMER]);
            let selected = simple(cpu, &mut |_, t| {
                t.edit_timer = false;
                t.selected
            });
            return start(cpu, hwnd, vec![st(W_EDIT, &[selected])], 0);
        }
        // Unicode messages (TVM_INSERTITEMW and the rest) and what the
        // tree doesn't do itself.
        _ => return defwnd::def_window_proc(cpu, args),
    };
    finish(cpu, hwnd, ops, result)
}
