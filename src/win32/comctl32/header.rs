//! The header control (SysHeader32): a row of column titles above a
//! list view's report, which the user can click, resize by their
//! dividers and, with HDS_DRAGDROP, drag into another order. Items
//! keep a width, a text, a bitmap or an image list's image, a format
//! and an lParam; an order array maps the places on screen to items.
//! The control asks its parent for text and images given as callbacks
//! (HDN_GETDISPINFO), lets it veto changes (HDN_ITEMCHANGING), draws
//! through NM_CUSTOMDRAW and, for HDF_OWNERDRAW items, WM_DRAWITEM.
//!
//! Notifications go out in the format the parent asks for with
//! WM_NOTIFYFORMAT: the W codes, with Unicode text, when it answers
//! NFR_UNICODE or after HDM_SETUNICODEFORMAT.
//!
//! Ported from Wine's header.c:
//!
//! Copyright 1998 Eric Kohl
//! Copyright 2000 Eric Kohl for CodeWeavers
//! Copyright 2003 Maxime Bellenge
//! Copyright 2006 Mikolaj Zalewski
//!
//! This library is free software; you can redistribute it and/or modify
//! it under the terms of the GNU Lesser General Public License as
//! published by the Free Software Foundation; either version 2.1 of the
//! License, or (at your option) any later version.

use super::gfx;
use super::script::{self, Arg, Ctx, Op, Script};
use super::{HEADER, id_of, state};
use crate::cpu::Cpu;
use crate::win32::gdi32::{self, Rect};
use crate::win32::user32::message::{
    WM_CREATE, WM_DRAWITEM, WM_ERASEBKGND, WM_GETDLGCODE, WM_GETFONT, WM_LBUTTONDBLCLK,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCDESTROY, WM_NOTIFY, WM_NOTIFYFORMAT, WM_PAINT,
    WM_PRINTCLIENT, WM_RBUTTONUP, WM_SETFONT, WM_SETREDRAW, WM_SIZE, WM_STYLECHANGED,
};
use crate::win32::user32::window::SWP_NOZORDER;
use crate::win32::user32::{self, defwnd, read_words, write_words};
use std::collections::BTreeMap;

const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_CONTEXTMENU: u32 = 0x007B;
const GWL_STYLE: u32 = (-16i32) as u32;

pub const HDM_GETITEMCOUNT: u32 = 0x1200;
pub const HDM_INSERTITEMA: u32 = 0x1201;
pub const HDM_DELETEITEM: u32 = 0x1202;
pub const HDM_GETITEMA: u32 = 0x1203;
pub const HDM_SETITEMA: u32 = 0x1204;
pub const HDM_LAYOUT: u32 = 0x1205;
pub const HDM_HITTEST: u32 = 0x1206;
pub const HDM_GETITEMRECT: u32 = 0x1207;
pub const HDM_SETIMAGELIST: u32 = 0x1208;
pub const HDM_GETIMAGELIST: u32 = 0x1209;
pub const HDM_INSERTITEMW: u32 = 0x120A;
pub const HDM_GETITEMW: u32 = 0x120B;
pub const HDM_SETITEMW: u32 = 0x120C;
pub const HDM_ORDERTOINDEX: u32 = 0x120F;
pub const HDM_CREATEDRAGIMAGE: u32 = 0x1210;
pub const HDM_GETORDERARRAY: u32 = 0x1211;
pub const HDM_SETORDERARRAY: u32 = 0x1212;
pub const HDM_SETHOTDIVIDER: u32 = 0x1213;
pub const HDM_SETBITMAPMARGIN: u32 = 0x1214;
pub const HDM_GETBITMAPMARGIN: u32 = 0x1215;
pub const HDM_SETUNICODEFORMAT: u32 = 0x2005;
pub const HDM_GETUNICODEFORMAT: u32 = 0x2006;

pub const HDS_BUTTONS: u32 = 0x0002;
pub const HDS_HOTTRACK: u32 = 0x0004;
pub const HDS_HIDDEN: u32 = 0x0008;
pub const HDS_DRAGDROP: u32 = 0x0040;
pub const HDS_FULLDRAG: u32 = 0x0080;
pub const HDS_FLAT: u32 = 0x0200;
pub const HDS_NOSIZING: u32 = 0x0800;

pub const HDI_WIDTH: u32 = 0x0001;
pub const HDI_TEXT: u32 = 0x0002;
pub const HDI_FORMAT: u32 = 0x0004;
pub const HDI_LPARAM: u32 = 0x0008;
pub const HDI_BITMAP: u32 = 0x0010;
pub const HDI_IMAGE: u32 = 0x0020;
pub const HDI_DI_SETITEM: u32 = 0x0040;
pub const HDI_ORDER: u32 = 0x0080;
const HDI_FILTER: u32 = 0x0100;
const HDI_SUPPORTED: u32 =
    HDI_WIDTH | HDI_TEXT | HDI_FORMAT | HDI_LPARAM | HDI_BITMAP | HDI_IMAGE | HDI_ORDER;
const HDI_UNKNOWN: u32 = !(HDI_SUPPORTED | HDI_FILTER | HDI_DI_SETITEM);
const HDI_COMCTL32_4_0: u32 = HDI_WIDTH | HDI_TEXT | HDI_FORMAT | HDI_LPARAM | HDI_BITMAP;

pub const HDF_LEFT: u32 = 0;
pub const HDF_RIGHT: u32 = 1;
pub const HDF_CENTER: u32 = 2;
pub const HDF_JUSTIFYMASK: u32 = 3;
pub const HDF_IMAGE: u32 = 0x0800;
pub const HDF_BITMAP_ON_RIGHT: u32 = 0x1000;
pub const HDF_BITMAP: u32 = 0x2000;
pub const HDF_STRING: u32 = 0x4000;
pub const HDF_OWNERDRAW: u32 = 0x8000;
const HDF_FIXEDWIDTH: u32 = 0x0100;

pub const HHT_NOWHERE: u32 = 0x0001;
pub const HHT_ONHEADER: u32 = 0x0002;
pub const HHT_ONDIVIDER: u32 = 0x0004;
pub const HHT_ONDIVOPEN: u32 = 0x0008;
pub const HHT_ABOVE: u32 = 0x0100;
pub const HHT_BELOW: u32 = 0x0200;
pub const HHT_TORIGHT: u32 = 0x0400;
pub const HHT_TOLEFT: u32 = 0x0800;

// The notifications, in their Unicode codes: the ANSI ones are 20 more.
pub const HDN_ITEMCHANGINGW: u32 = (-320i32) as u32;
pub const HDN_ITEMCHANGEDW: u32 = (-321i32) as u32;
pub const HDN_ITEMCLICKW: u32 = (-322i32) as u32;
pub const HDN_ITEMDBLCLICKW: u32 = (-323i32) as u32;
pub const HDN_DIVIDERDBLCLICKW: u32 = (-325i32) as u32;
pub const HDN_BEGINTRACKW: u32 = (-326i32) as u32;
pub const HDN_ENDTRACKW: u32 = (-327i32) as u32;
pub const HDN_TRACKW: u32 = (-328i32) as u32;
pub const HDN_GETDISPINFOW: u32 = (-329i32) as u32;
pub const HDN_BEGINDRAG: u32 = (-310i32) as u32;
pub const HDN_ENDDRAG: u32 = (-311i32) as u32;
const HDN_UNICODE_OFFSET: u32 = 20;

const NM_RCLICK: u32 = (-5i32) as u32;
const NM_CUSTOMDRAW: u32 = (-12i32) as u32;
const NM_RELEASEDCAPTURE: u32 = (-16i32) as u32;

const CDDS_PREPAINT: u32 = 1;
const CDDS_POSTPAINT: u32 = 2;
const CDDS_ITEM: u32 = 0x0001_0000;
const CDRF_SKIPDEFAULT: u32 = 0x04;
const CDRF_NOTIFYPOSTPAINT: u32 = 0x10;
const CDRF_NOTIFYITEMDRAW: u32 = 0x20;
const CDIS_SELECTED: u32 = 1;

pub const NFR_ANSI: u32 = 1;
pub const NFR_UNICODE: u32 = 2;
pub const NF_QUERY: u32 = 3;
pub const NF_REQUERY: u32 = 4;

const ODT_HEADER: u32 = 100;
const ODA_DRAWENTIRE: u32 = 1;
const ODS_SELECTED: u32 = 1;
const DLGC_WANTARROWS: u32 = 1;
const DLGC_WANTTAB: u32 = 2;

const I_IMAGECALLBACK: u32 = 0xFFFF_FFFF;
const LPSTR_TEXTCALLBACK: u32 = 0xFFFF_FFFF;

const VERT_BORDER: i32 = 4;
const DIVIDER_WIDTH: i32 = 10;
const HOT_DIVIDER_WIDTH: i32 = 2;
const MAX_HEADER_TEXT_LEN: u32 = 260;
pub const SYSTEM_FONT: u32 = 13;
const SM_CXEDGE: u32 = 45;
const SM_CXDOUBLECLK: u32 = 36;
const SM_CYDOUBLECLK: u32 = 37;

const BDR_RAISEDOUTER: u32 = 0x0001;
const EDGE_RAISED: u32 = 0x0005;
const EDGE_ETCHED: u32 = 0x0006;
const BF_LEFT: u32 = 1;
const BF_TOP: u32 = 2;
const BF_RIGHT: u32 = 4;
const BF_BOTTOM: u32 = 8;
const BF_RECT: u32 = 15;
const BF_MIDDLE: u32 = 0x0800;
const BF_SOFT: u32 = 0x1000;
const BF_ADJUST: u32 = 0x2000;
const BF_FLAT: u32 = 0x4000;

const DT_VCENTER: u32 = 0x0004;
const DT_SINGLELINE: u32 = 0x0020;
const DT_CALCRECT: u32 = 0x0400;
const DT_END_ELLIPSIS: u32 = 0x8000;
const TRANSPARENT: u32 = 1;
const R2_XORPEN: u32 = 7;
const BLACK_PEN: u32 = 7;
const SRCCOPY: u32 = 0x00CC_0020;

/// The scratch space of the control's scripts: a notification's
/// structure, an HDITEM it points to, a text buffer, a PAINTSTRUCT.
const S_NM: u32 = 0;
const S_ITEM: u32 = 64;
const S_TEXT: u32 = 112;
const S_PS: u32 = 640;
const S_SIZE: u32 = 704;

// The continuations.
const T_SETITEM: u16 = 1;
const T_SETITEM_CHANGING: u16 = 2;
const T_GETITEM: u16 = 3;
const T_GETITEM_DISP: u16 = 4;
const T_NOTIFY: u16 = 5;
const T_FORMAT: u16 = 6;
const T_PAINT: u16 = 7;
const T_PAINT_PRE: u16 = 8;
const T_PAINT_ITEM: u16 = 9;
const T_PAINT_ITEM_CD: u16 = 10;
const T_PAINT_ITEM_DISP: u16 = 11;
const T_PAINT_POST: u16 = 12;
const T_BEGIN_TRACK: u16 = 13;
const T_CHANGING_WIDTH: u16 = 14;
const T_BEGIN_DRAG: u16 = 15;
const T_END_DRAG: u16 = 16;
const T_RETURN_LAST: u16 = 17;

/// A header item.
#[derive(Clone, Debug, Default)]
pub struct Item {
    pub cxy: i32,
    pub hbm: u32,
    /// The text, ANSI; None when none was set or it is a callback.
    pub text: Option<Vec<u8>>,
    pub fmt: u32,
    pub lparam: u32,
    pub image: i32,
    pub order: i32,
    pub down: bool,
    pub rect: Rect,
    /// The HDI_ fields the parent gives with HDN_GETDISPINFO.
    pub callback: u32,
}

crate::state_fields!(Item {
    cxy,
    hbm,
    text,
    fmt,
    lparam,
    image,
    order,
    down,
    rect,
    callback
});

/// What a header control keeps.
#[derive(Clone, Debug, Default)]
pub struct Header {
    pub notify: u32,
    /// NFR_ANSI or NFR_UNICODE.
    pub format: u32,
    pub height: i32,
    pub font: u32,
    pub captured: bool,
    pub pressed: bool,
    pub dragging: bool,
    pub tracking: bool,
    pub down_at: (i32, i32),
    pub style: u32,
    pub move_item: i32,
    pub track_offset: i32,
    pub old_track: i32,
    pub hot_item: i32,
    pub hot_divider: i32,
    pub margin: i32,
    pub himl: u32,
    pub items: Vec<Item>,
    /// Items by their place on screen.
    pub order: Vec<i32>,
    pub rects_valid: bool,
    /// The font the DC had before painting selected the control's.
    pub old_font: u32,
}

crate::state_fields!(Header {
    notify,
    format,
    height,
    font,
    captured,
    pressed,
    dragging,
    tracking,
    down_at,
    style,
    move_item,
    track_offset,
    old_track,
    hot_item,
    hot_divider,
    margin,
    himl,
    items,
    order,
    rects_valid,
    old_font
});

/// What the module keeps, in COMCTL32's `State`.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub headers: BTreeMap<u32, Header>,
}

crate::state_fields!(State { headers });

fn get(cpu: &mut Cpu, hwnd: u32) -> Option<Header> {
    state(cpu).header.headers.get(&hwnd).cloned()
}

fn put(cpu: &mut Cpu, hwnd: u32, h: Header) {
    state(cpu).header.headers.insert(hwnd, h);
}

// Text and drawing helpers, for the header and the list view.

/// The zero-ended string at `at`, ANSI or Unicode, as ANSI bytes; None
/// if a page isn't there.
pub fn read_text(cpu: &mut Cpu, at: u32, wide: bool) -> Option<Vec<u8>> {
    if !wide {
        return user32::bytes_at(cpu, at);
    }
    let mut units = Vec::new();
    for i in 0..0x8000u32 {
        let u = cpu.bus.guest_read_16(at.wrapping_add(2 * i));
        if cpu.bus.guest_faulted() {
            return None;
        }
        if u == 0 {
            break;
        }
        units.push(u);
    }
    Some(wide_to_ansi(&units))
}

/// Unicode text as ANSI, by the ANSI code page; '?' for what it lacks.
pub fn wide_to_ansi(units: &[u16]) -> Vec<u8> {
    units
        .iter()
        .map(|&u| {
            if u < 0x80 {
                return u as u8;
            }
            (0x80..=0xFFu8)
                .find(|&b| crate::win32::kernel32::ansi_to_wide(&[b]).first() == Some(&u))
                .unwrap_or(b'?')
        })
        .collect()
}

/// Write `text` at `at` with its zero, as ANSI or Unicode, cut to fit
/// `max` characters with the zero: the characters written, without it.
pub fn write_text(cpu: &mut Cpu, at: u32, max: u32, text: &[u8], wide: bool) -> u32 {
    if max == 0 || at == 0 {
        return 0;
    }
    if !wide {
        return user32::copy_out(cpu, at, max, text);
    }
    let units = crate::win32::kernel32::ansi_to_wide(text);
    let n = units.len().min(max as usize - 1);
    let mut bytes: Vec<u8> = units[..n].iter().flat_map(|u| u.to_le_bytes()).collect();
    bytes.extend([0, 0]);
    cpu.bus.guest_write_bytes(at, &bytes);
    n as u32
}

/// InvalidateRect(hwnd, &r, erase).
pub fn invalidate_rect(cpu: &mut Cpu, hwnd: u32, r: Rect, erase: bool) {
    let rgn = crate::win32::gdi32::rgn::create_rect_rgn(
        cpu,
        &[r.left as u32, r.top as u32, r.right as u32, r.bottom as u32],
    )
    .unwrap_or(0);
    if rgn != 0 {
        user32::paint::invalidate_rgn(cpu, &[hwnd, rgn, erase as u32]);
        gdi32::delete_object(cpu, &[rgn]);
    }
}

/// The height of `font` (or the system font for 0), with its leading.
pub fn font_height_of(cpu: &mut Cpu, font: u32) -> i32 {
    let hdc = user32::paint::get_dc(cpu, &[0]).unwrap_or(0);
    let font = if font != 0 {
        font
    } else {
        gfx::stock(cpu, SYSTEM_FONT)
    };
    let old = gfx::select(cpu, hdc, font);
    let h = gfx::font_height(cpu, hdc);
    gfx::select(cpu, hdc, old);
    user32::paint::release_dc(cpu, &[0, hdc]);
    h
}

/// SendMessageA(to, msg, wp, lp) as a step.
pub fn send_op(to: u32, msg: u32, wp: Arg, lp: Arg) -> Op {
    Op::Call {
        target: user32::address_of("SendMessageA"),
        args: vec![Arg::V(to), Arg::V(msg), wp, lp],
    }
}

/// The ANSI code of a notification for an ANSI parent.
fn code_for(h: &Header, code: u32) -> u32 {
    let first = (-320i32) as u32;
    let last = (-399i32) as u32;
    if h.format != NFR_UNICODE && code >= last && code <= first {
        code.wrapping_add(HDN_UNICODE_OFFSET)
    } else {
        code
    }
}

impl Header {
    fn count(&self) -> i32 {
        self.items.len() as i32
    }

    pub fn order_to_index(&self, order: i32) -> i32 {
        if order < 0 || order >= self.count() {
            return order;
        }
        self.order[order as usize]
    }

    fn next_item(&self, i: i32) -> i32 {
        self.order_to_index(self.items[i as usize].order + 1)
    }

    fn prev_item(&self, i: i32) -> i32 {
        self.order_to_index(self.items[i as usize].order - 1)
    }

    fn is_fixed(&self, i: i32) -> bool {
        self.style & HDS_NOSIZING != 0
            || self
                .items
                .get(i as usize)
                .is_some_and(|it| it.fmt & HDF_FIXEDWIDTH != 0)
    }

    fn change_order(&mut self, item: i32, new: i32) {
        let old = self.items[item as usize].order;
        let (old_u, new_u) = (old as usize, new as usize);
        if old < new {
            self.order.copy_within(old_u + 1..=new_u, old_u);
        }
        if new < old {
            self.order.copy_within(new_u..old_u, new_u + 1);
        }
        self.order[new_u] = item;
        for i in old.min(new)..=old.max(new) {
            let idx = self.order[i as usize] as usize;
            self.items[idx].order = i;
        }
    }

    fn set_bounds(&mut self, client: Rect) {
        self.rects_valid = true;
        let mut x = client.left;
        for i in 0..self.items.len() {
            let idx = self.order[i] as usize;
            let it = &mut self.items[idx];
            it.rect = Rect::new(x, client.top, x + it.cxy.max(0), client.bottom);
            x = it.rect.right;
        }
    }

    fn hot_divider_rect(&self, client: Rect) -> Rect {
        let d = self.hot_divider;
        if self.items.is_empty() {
            return Rect::new(
                client.left,
                client.top,
                client.left + HOT_DIVIDER_WIDTH / 2,
                client.bottom,
            );
        }
        let (r, x) = if d < self.count() && d >= 0 {
            let r = self.items[d as usize].rect;
            (r, r.left)
        } else {
            let last = self.order_to_index(self.count() - 1) as usize;
            let r = self.items[last].rect;
            (r, r.right)
        };
        Rect::new(
            x - HOT_DIVIDER_WIDTH / 2,
            r.top,
            x + HOT_DIVIDER_WIDTH / 2,
            r.bottom,
        )
    }

    /// HEADER_InternalHitTest: the HHT_ flags and the item.
    fn hit_test(&self, client: Rect, x: i32, y: i32) -> (u32, i32) {
        if client.contains(x, y) {
            if self.items.is_empty() {
                return (HHT_NOWHERE, 1);
            }
            let mut no_width = false;
            for (i, it) in self.items.iter().enumerate() {
                let i = i as i32;
                let rect = it.rect;
                let width = rect.width();
                if width == 0 {
                    no_width = true;
                    continue;
                }
                if !rect.contains(x, y) {
                    continue;
                }
                if width <= 2 * DIVIDER_WIDTH {
                    return (HHT_ONHEADER, i);
                }
                if it.order > 0 {
                    let test =
                        Rect::new(rect.left, rect.top, rect.left + DIVIDER_WIDTH, rect.bottom);
                    if test.contains(x, y) {
                        let prev = self.prev_item(i);
                        if self.is_fixed(prev) {
                            return (HHT_ONHEADER, i);
                        }
                        let flag = if no_width {
                            HHT_ONDIVOPEN
                        } else {
                            HHT_ONDIVIDER
                        };
                        return (flag, prev);
                    }
                }
                let test = Rect::new(
                    rect.right - DIVIDER_WIDTH,
                    rect.top,
                    rect.right,
                    rect.bottom,
                );
                if !self.is_fixed(i) && test.contains(x, y) {
                    return (HHT_ONDIVIDER, i);
                }
                return (HHT_ONHEADER, i);
            }
            let last = self.count() - 1;
            if !self.is_fixed(last) {
                let r = self.items[last as usize].rect;
                let test = Rect::new(r.right, r.top, r.right + DIVIDER_WIDTH, r.bottom);
                if test.contains(x, y) {
                    let flag = if no_width {
                        HHT_ONDIVOPEN
                    } else {
                        HHT_ONDIVIDER
                    };
                    return (flag, last);
                }
            }
            return (HHT_NOWHERE, 1);
        }
        let mut flags = 0;
        if x < client.left {
            flags |= HHT_TOLEFT;
        } else if x > client.right {
            flags |= HHT_TORIGHT;
        }
        if y < client.top {
            flags |= HHT_ABOVE;
        } else if y > client.bottom {
            flags |= HHT_BELOW;
        }
        (flags, 1)
    }

    /// The item's rectangle inside its edge, as DrawEdge's BF_ADJUST
    /// leaves it.
    fn inner_rect(&self, i: i32) -> Rect {
        let it = &self.items[i as usize];
        let r = it.rect;
        if self.style & HDS_FLAT != 0 {
            r
        } else if self.style & HDS_BUTTONS != 0 {
            if it.down {
                r.inflate(-1, -1)
            } else {
                r.inflate(-2, -2)
            }
        } else {
            Rect::new(r.left, r.top, r.right - 2, r.bottom - 2)
        }
    }
}

/// The items' rectangles of the header `hwnd`, for the list view.
pub fn item_rects(cpu: &mut Cpu, hwnd: u32) -> Vec<Rect> {
    with_bounds(cpu, hwnd)
        .map(|h| h.items.iter().map(|it| it.rect).collect())
        .unwrap_or_default()
}

/// The order array of the header `hwnd`, for the list view.
pub fn order_of(cpu: &mut Cpu, hwnd: u32) -> Vec<i32> {
    get(cpu, hwnd).map(|h| h.order).unwrap_or_default()
}

/// The header `hwnd` with its items' rectangles worked out.
fn with_bounds(cpu: &mut Cpu, hwnd: u32) -> Option<Header> {
    let mut h = get(cpu, hwnd)?;
    if !h.rects_valid {
        let client = super::client(cpu, hwnd);
        h.set_bounds(client);
        put(cpu, hwnd, h.clone());
    }
    Some(h)
}

fn set_bounds(cpu: &mut Cpu, hwnd: u32, h: &mut Header) {
    let client = super::client(cpu, hwnd);
    h.set_bounds(client);
}

/// An HDITEM as read from the program.
#[derive(Clone, Debug, Default)]
struct HdItem {
    mask: u32,
    cxy: i32,
    text_ptr: u32,
    hbm: u32,
    text_max: u32,
    fmt: u32,
    lparam: u32,
    image: i32,
    order: i32,
    /// The text, when the mask has HDI_TEXT and it isn't a callback.
    text: Option<Vec<u8>>,
}

impl HdItem {
    fn read(cpu: &mut Cpu, at: u32, wide: bool) -> Option<HdItem> {
        let w = read_words(cpu, at, 9);
        if cpu.bus.guest_faulted() {
            return None;
        }
        let mut it = HdItem {
            mask: w[0],
            cxy: w[1] as i32,
            text_ptr: w[2],
            hbm: w[3],
            text_max: w[4],
            fmt: w[5],
            lparam: w[6],
            image: w[7] as i32,
            order: w[8] as i32,
            text: None,
        };
        if it.mask & HDI_TEXT != 0 && it.text_ptr != LPSTR_TEXTCALLBACK {
            it.text = Some(if it.text_ptr == 0 {
                Vec::new()
            } else {
                read_text(cpu, it.text_ptr, wide)?
            });
        }
        Some(it)
    }

    /// As words to pass to a continuation.
    fn to_words(&self) -> Vec<u32> {
        let mut v = vec![
            self.mask,
            self.cxy as u32,
            self.text_ptr,
            self.hbm,
            self.text_max,
            self.fmt,
            self.lparam,
            self.image as u32,
            self.order as u32,
        ];
        match &self.text {
            Some(t) => {
                v.push(1);
                v.push(t.len() as u32);
                v.extend(script::words_of(t));
            }
            None => v.push(0),
        }
        v
    }

    fn from_words(w: &[u32]) -> HdItem {
        let g = |i: usize| w.get(i).copied().unwrap_or(0);
        let mut it = HdItem {
            mask: g(0),
            cxy: g(1) as i32,
            text_ptr: g(2),
            hbm: g(3),
            text_max: g(4),
            fmt: g(5),
            lparam: g(6),
            image: g(7) as i32,
            order: g(8) as i32,
            text: None,
        };
        if g(9) == 1 {
            let len = g(10) as usize;
            let bytes: Vec<u8> = w
                .get(11..)
                .unwrap_or(&[])
                .iter()
                .flat_map(|x| x.to_le_bytes())
                .take(len)
                .collect();
            it.text = Some(bytes);
        }
        it
    }
}

/// HEADER_StoreHDItemInHeader.
fn store(item: &mut Item, mask: u32, hd: &HdItem) {
    if mask & HDI_BITMAP != 0 {
        item.hbm = hd.hbm;
    }
    if mask & HDI_FORMAT != 0 {
        item.fmt = hd.fmt;
    }
    if mask & HDI_LPARAM != 0 {
        item.lparam = hd.lparam;
    }
    if mask & HDI_WIDTH != 0 {
        item.cxy = hd.cxy;
    }
    if mask & HDI_IMAGE != 0 {
        item.image = hd.image;
        if hd.image as u32 == I_IMAGECALLBACK {
            item.callback |= HDI_IMAGE;
        } else {
            item.callback &= !HDI_IMAGE;
        }
    }
    if mask & HDI_TEXT != 0 {
        match &hd.text {
            Some(t) => {
                item.text = Some(t.clone());
                item.callback &= !HDI_TEXT;
            }
            None => {
                item.text = None;
                item.callback |= HDI_TEXT;
            }
        }
    }
}

/// Write an HDITEM at the scratch offset `S_ITEM` for a notification,
/// its text (if any) at `S_TEXT` in the parent's format: its address.
fn write_notify_item(cpu: &mut Cpu, scratch: u32, h: &Header, hd: &HdItem) -> u32 {
    let at = scratch + S_ITEM;
    let mut text_ptr = hd.text_ptr;
    if hd.mask & HDI_TEXT != 0
        && let Some(t) = &hd.text
    {
        let wide = h.format == NFR_UNICODE;
        write_text(cpu, scratch + S_TEXT, MAX_HEADER_TEXT_LEN, t, wide);
        text_ptr = scratch + S_TEXT;
    }
    write_words(
        cpu,
        at,
        &[
            hd.mask,
            hd.cxy as u32,
            text_ptr,
            hd.hbm,
            hd.text_max,
            hd.fmt,
            hd.lparam,
            hd.image as u32,
            hd.order as u32,
        ],
    );
    at
}

/// The steps that send the NMHEADER notification `code` (a W code) for
/// `item`, with `hd` as its HDITEM or none, from a continuation.
fn header_notify_ops(
    cpu: &mut Cpu,
    scratch: u32,
    hwnd: u32,
    h: &Header,
    code: u32,
    item: i32,
    hd: Option<&HdItem>,
) -> Vec<Op> {
    let pitem = match hd {
        Some(hd) => write_notify_item(cpu, scratch, h, hd),
        None => 0,
    };
    let id = id_of(cpu, hwnd);
    write_words(
        cpu,
        scratch + S_NM,
        &[hwnd, id, code_for(h, code), item as u32, 0, pitem],
    );
    vec![send_op(h.notify, WM_NOTIFY, Arg::V(id), Arg::S(S_NM))]
}

/// An HDITEM of the item's fields with one of them changed
/// (HEADER_SendNotifyWithIntFieldT).
fn int_field_item(h: &Header, item: i32, mask: u32, value: i32) -> HdItem {
    let it = h.items.get(item as usize).cloned().unwrap_or_default();
    let mut hd = HdItem {
        mask,
        cxy: it.cxy,
        text_ptr: 0,
        hbm: it.hbm,
        text_max: 0,
        fmt: it.fmt,
        lparam: it.lparam,
        image: it.image,
        order: it.order,
        text: None,
    };
    if mask == HDI_WIDTH {
        hd.cxy = value;
    } else if mask == HDI_ORDER {
        hd.order = value;
    }
    hd
}

/// Add to `s` the notification `code` for `item`, with an HDITEM of its
/// fields and `mask`'s set to `value` when `mask` isn't 0.
fn notify_later(s: &mut Script, code: u32, item: i32, mask: u32, value: i32) {
    s.then(T_NOTIFY, &[code, item as u32, mask, value as u32]);
}

fn simple_notify(cpu: &mut Cpu, s: &mut Script, hwnd: u32, h: &Header, code: u32) {
    super::notify(cpu, s, hwnd, h.notify, code, &[]);
}

fn new_script(hwnd: u32) -> Script {
    let mut s = Script::new(hwnd);
    s.alloc(S_SIZE);
    s
}

/// The header control's window procedure.
pub fn proc_(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, msg, wp, lp] = args else {
        unreachable!()
    };
    if msg == WM_CREATE {
        return create(cpu, hwnd, lp);
    }
    let Some(mut h) = get(cpu, hwnd) else {
        return defwnd::def_window_proc(cpu, args);
    };
    let unicode_msg = matches!(msg, HDM_GETITEMW | HDM_SETITEMW | HDM_INSERTITEMW);
    let result = match msg {
        HDM_CREATEDRAGIMAGE => create_drag_image(cpu, hwnd, wp as i32),
        HDM_DELETEITEM => {
            let i = wp as i32;
            if i < 0 || i >= h.count() {
                return Some(0);
            }
            let order = h.items[i as usize].order as usize;
            h.items.remove(i as usize);
            h.order.remove(order);
            for o in 0..h.order.len() {
                if h.order[o] > i {
                    h.order[o] -= 1;
                }
                if o >= order {
                    let idx = h.order[o] as usize;
                    h.items[idx].order = o as i32;
                }
            }
            set_bounds(cpu, hwnd, &mut h);
            put(cpu, hwnd, h);
            super::invalidate(cpu, hwnd, false);
            1
        }
        HDM_GETBITMAPMARGIN => h.margin as u32,
        HDM_SETBITMAPMARGIN => {
            let old = h.margin;
            h.margin = wp as i32;
            put(cpu, hwnd, h);
            old as u32
        }
        HDM_GETIMAGELIST => h.himl,
        HDM_SETIMAGELIST => {
            let old = h.himl;
            h.himl = lp;
            put(cpu, hwnd, h);
            old
        }
        HDM_GETITEMA | HDM_GETITEMW => {
            return get_item(cpu, hwnd, &h, wp as i32, lp, unicode_msg);
        }
        HDM_GETITEMCOUNT => h.count() as u32,
        HDM_GETITEMRECT => {
            let i = wp as i32;
            if i < 0 || i >= h.count() {
                return Some(0);
            }
            let h = with_bounds(cpu, hwnd)?;
            h.items[i as usize].rect.write(cpu, lp);
            if cpu.bus.guest_faulted() {
                return None;
            }
            1
        }
        HDM_GETORDERARRAY => {
            if (wp as usize) < h.items.len() {
                return Some(0);
            }
            let words: Vec<u32> = h.order.iter().map(|&o| o as u32).collect();
            write_words(cpu, lp, &words);
            if cpu.bus.guest_faulted() {
                return None;
            }
            1
        }
        HDM_SETORDERARRAY => {
            if wp as usize != h.items.len() {
                return Some(0);
            }
            let order = read_words(cpu, lp, wp as usize);
            if cpu.bus.guest_faulted() {
                return None;
            }
            set_order_array(&mut h, &order);
            set_bounds(cpu, hwnd, &mut h);
            put(cpu, hwnd, h);
            super::invalidate(cpu, hwnd, false);
            1
        }
        HDM_GETUNICODEFORMAT => (h.format == NFR_UNICODE) as u32,
        HDM_SETUNICODEFORMAT => {
            let old = h.format == NFR_UNICODE;
            h.format = if wp != 0 { NFR_UNICODE } else { NFR_ANSI };
            put(cpu, hwnd, h);
            old as u32
        }
        HDM_HITTEST => {
            let pt = read_words(cpu, lp, 2);
            if cpu.bus.guest_faulted() {
                return None;
            }
            let h = with_bounds(cpu, hwnd)?;
            let client = super::client(cpu, hwnd);
            let (flags, mut item) = h.hit_test(client, pt[0] as i32, pt[1] as i32);
            let outside = HHT_NOWHERE | HHT_ABOVE | HHT_BELOW | HHT_TOLEFT | HHT_TORIGHT;
            if flags & outside != 0 {
                item = -1;
            }
            write_words(cpu, lp + 8, &[flags, item as u32]);
            item as u32
        }
        HDM_INSERTITEMA | HDM_INSERTITEMW => {
            if lp == 0 {
                return Some(u32::MAX);
            }
            let hd = HdItem::read(cpu, lp, unicode_msg)?;
            insert_item(cpu, hwnd, &mut h, wp as i32, &hd) as u32
        }
        HDM_LAYOUT => {
            let w = read_words(cpu, lp, 2);
            if cpu.bus.guest_faulted() {
                return None;
            }
            let (prc, pwpos) = (w[0], w[1]);
            let mut r = Rect::read(cpu, prc);
            if cpu.bus.guest_faulted() {
                return None;
            }
            let cy = if h.style & HDS_HIDDEN != 0 {
                0
            } else {
                h.height
            };
            write_words(
                cpu,
                pwpos,
                &[
                    hwnd,
                    0,
                    r.left as u32,
                    r.top as u32,
                    r.width() as u32,
                    cy as u32,
                    SWP_NOZORDER,
                ],
            );
            if h.style & HDS_HIDDEN == 0 {
                r.top += h.height;
                r.write(cpu, prc);
            }
            if cpu.bus.guest_faulted() {
                return None;
            }
            h.rects_valid = false;
            put(cpu, hwnd, h);
            1
        }
        HDM_ORDERTOINDEX => h.order_to_index(wp as i32) as u32,
        HDM_SETHOTDIVIDER => {
            let r = set_hot_divider(cpu, hwnd, &mut h, wp != 0, lp);
            put(cpu, hwnd, h);
            r as u32
        }
        HDM_SETITEMA | HDM_SETITEMW => {
            if lp == 0 {
                return Some(0);
            }
            let i = wp as i32;
            if i < 0 || i >= h.count() {
                return Some(0);
            }
            let hd = HdItem::read(cpu, lp, unicode_msg)?;
            let mut s = new_script(hwnd);
            let mut data = vec![i as u32];
            data.extend(hd.to_words());
            s.then(T_SETITEM, &data);
            return script::run(cpu, HEADER, s);
        }
        WM_NCDESTROY => {
            state(cpu).header.headers.remove(&hwnd);
            return defwnd::def_window_proc(cpu, args);
        }
        WM_ERASEBKGND => 1,
        WM_GETDLGCODE => DLGC_WANTTAB | DLGC_WANTARROWS,
        WM_GETFONT => h.font,
        WM_LBUTTONDBLCLK => {
            let (x, y) = user32::point_of(lp);
            let h = with_bounds(cpu, hwnd)?;
            let client = super::client(cpu, hwnd);
            let (flags, item) = h.hit_test(client, x, y);
            let mut s = new_script(hwnd);
            if h.style & HDS_BUTTONS != 0 && flags == HHT_ONHEADER {
                notify_later(&mut s, HDN_ITEMDBLCLICKW, item, 0, 0);
            } else if flags == HHT_ONDIVIDER || flags == HHT_ONDIVOPEN {
                notify_later(&mut s, HDN_DIVIDERDBLCLICKW, item, 0, 0);
            }
            s.ret(0);
            return script::run(cpu, HEADER, s);
        }
        WM_LBUTTONDOWN => return button_down(cpu, hwnd, lp),
        WM_LBUTTONUP => return button_up(cpu, hwnd, lp),
        WM_MOUSELEAVE => {
            let old = h.hot_item;
            h.hot_item = -1;
            put(cpu, hwnd, h.clone());
            refresh_item(cpu, hwnd, &h, old);
            0
        }
        WM_MOUSEMOVE => return mouse_move(cpu, hwnd, lp),
        WM_NOTIFYFORMAT => match lp {
            NF_QUERY => h.format,
            NF_REQUERY => {
                let mut s = new_script(hwnd);
                s.send(wp, WM_NOTIFYFORMAT, Arg::V(hwnd), Arg::V(NF_QUERY))
                    .then(T_FORMAT, &[]);
                return script::run(cpu, HEADER, s);
            }
            _ => 0,
        },
        WM_SIZE => {
            set_bounds(cpu, hwnd, &mut h);
            put(cpu, hwnd, h);
            0
        }
        WM_PAINT | WM_PRINTCLIENT => {
            let mut s = new_script(hwnd);
            if wp != 0 {
                s.then(T_PAINT, &[wp]);
            } else {
                let begin = user32::address_of("BeginPaint");
                let end = user32::address_of("EndPaint");
                s.call(begin, &[Arg::V(hwnd), Arg::S(S_PS)])
                    .then(T_PAINT, &[0])
                    .call(end, &[Arg::V(hwnd), Arg::S(S_PS)]);
            }
            s.ret(0);
            return script::run(cpu, HEADER, s);
        }
        WM_RBUTTONUP => {
            let (x, y) = user32::point_of(lp);
            let (ox, oy) = user32::user(cpu).client_origin(hwnd);
            let mut s = new_script(hwnd);
            simple_notify(cpu, &mut s, hwnd, &h, NM_RCLICK);
            s.then(T_RETURN_LAST, &[]);
            let at = user32::make_long(x + ox, y + oy);
            s.send(hwnd, WM_CONTEXTMENU, Arg::V(hwnd), Arg::V(at));
            return script::run(cpu, HEADER, s);
        }
        WM_SETFONT => {
            h.font = wp;
            h.height = font_height_of(cpu, wp) + VERT_BORDER;
            h.rects_valid = false;
            put(cpu, hwnd, h);
            if lp & 0xFFFF != 0 {
                super::invalidate(cpu, hwnd, false);
            }
            0
        }
        WM_SETREDRAW => {
            let r = defwnd::def_window_proc(cpu, args);
            if wp != 0 {
                super::invalidate(cpu, hwnd, true);
            }
            return r;
        }
        WM_STYLECHANGED => {
            if wp == GWL_STYLE {
                let new = cpu.bus.guest_read_32(lp + 4);
                if cpu.bus.guest_faulted() {
                    return None;
                }
                h.style = new;
                put(cpu, hwnd, h);
            }
            0
        }
        _ => return defwnd::def_window_proc(cpu, args),
    };
    Some(result)
}

/// HDM_SETORDERARRAY: each item named in the array, in turn, is moved
/// from its place to the array's place for it; the places with an index
/// out of range are passed over. This is what Windows does, as the
/// answers in Wine's tests show (Wine's own header.c differs).
fn set_order_array(h: &mut Header, order: &[u32]) {
    let size = order.len() as i32;
    for (i, &v) in order.iter().enumerate() {
        let v = v as i32;
        if v < 0 || v >= size {
            continue;
        }
        if let Some(from) = h.order.iter().position(|&x| x == v) {
            h.order.remove(from);
            h.order.insert(i, v);
        }
    }
    for i in 0..order.len() {
        let idx = h.order[i] as usize;
        if let Some(it) = h.items.get_mut(idx) {
            it.order = i as i32;
        }
    }
}

/// HEADER_InsertItemT: the new item's index, or -1.
fn insert_item(cpu: &mut Cpu, hwnd: u32, h: &mut Header, at: i32, hd: &HdItem) -> i32 {
    if at < 0 || hd.mask == 0 {
        return -1;
    }
    let n = at.min(h.count());
    let order = if hd.mask & HDI_ORDER != 0 {
        hd.order
    } else {
        n
    };
    let order = order.clamp(0, h.count());
    h.items.insert(n as usize, Item::default());
    h.order.insert(order as usize, n);
    for i in 0..h.order.len() {
        if i != order as usize && h.order[i] >= n {
            h.order[i] += 1;
        }
        let idx = h.order[i] as usize;
        h.items[idx].order = i as i32;
    }
    let mut item = Item::default();
    // The width, format and lParam are kept even when the mask hasn't
    // them.
    store(&mut item, hd.mask | HDI_WIDTH | HDI_FORMAT | HDI_LPARAM, hd);
    item.order = order;
    if hd.mask & HDI_TEXT != 0 {
        item.fmt |= HDF_STRING;
    } else {
        item.fmt &= !HDF_STRING;
    }
    if item.hbm != 0 {
        item.fmt |= HDF_BITMAP;
    } else {
        item.fmt &= !HDF_BITMAP;
    }
    if hd.mask & HDI_IMAGE != 0 {
        item.fmt |= HDF_IMAGE;
    }
    h.items[n as usize] = item;
    set_bounds(cpu, hwnd, h);
    put(cpu, hwnd, h.clone());
    super::invalidate(cpu, hwnd, false);
    n
}

/// HDM_GETITEM: fill the program's HDITEM, asking the parent for the
/// fields that are callbacks first.
fn get_item(cpu: &mut Cpu, hwnd: u32, h: &Header, i: i32, at: u32, wide: bool) -> Option<u32> {
    if at == 0 {
        return Some(0);
    }
    let mut mask = cpu.bus.guest_read_32(at);
    if cpu.bus.guest_faulted() {
        return None;
    }
    if mask == 0 {
        return Some(1);
    }
    if i < 0 || i >= h.count() {
        return Some(0);
    }
    if mask & HDI_UNKNOWN != 0 {
        mask &= HDI_COMCTL32_4_0;
    }
    let item = &h.items[i as usize];
    if mask & item.callback == 0 {
        fill_item(cpu, item, mask, at, wide, None, item.image);
        if cpu.bus.guest_faulted() {
            return None;
        }
        return Some(1);
    }
    let mut s = new_script(hwnd);
    s.then(T_GETITEM, &[i as u32, at, mask, wide as u32]);
    script::run(cpu, HEADER, s)
}

/// Write the item's fields of `mask` into the HDITEM at `at`.
fn fill_item(
    cpu: &mut Cpu,
    item: &Item,
    mask: u32,
    at: u32,
    wide: bool,
    text: Option<&[u8]>,
    image: i32,
) {
    if mask & HDI_BITMAP != 0 {
        cpu.bus.guest_write_32(at + 12, item.hbm);
    }
    if mask & HDI_FORMAT != 0 {
        cpu.bus.guest_write_32(at + 20, item.fmt);
    }
    if mask & HDI_WIDTH != 0 {
        cpu.bus.guest_write_32(at + 4, item.cxy as u32);
    }
    if mask & HDI_LPARAM != 0 {
        cpu.bus.guest_write_32(at + 24, item.lparam);
    }
    if mask & HDI_IMAGE != 0 {
        cpu.bus.guest_write_32(at + 28, image as u32);
    }
    if mask & HDI_ORDER != 0 {
        cpu.bus.guest_write_32(at + 32, item.order as u32);
    }
    if mask & HDI_TEXT != 0 {
        let ptr = cpu.bus.guest_read_32(at + 8);
        let max = cpu.bus.guest_read_32(at + 16);
        let t = text.or(item.text.as_deref());
        if ptr != 0 && ptr != LPSTR_TEXTCALLBACK && max != 0 {
            write_text(cpu, ptr, max, t.unwrap_or(&[]), wide);
        }
    }
}

/// Write NMHDDISPINFO for the item's `mask` fields at `S_NM` and send it:
/// the steps, from a continuation.
fn disp_info_ops(cpu: &mut Cpu, scratch: u32, hwnd: u32, h: &Header, i: i32, mask: u32) -> Vec<Op> {
    let id = id_of(cpu, hwnd);
    let unicode = h.format == NFR_UNICODE;
    let code = if unicode {
        HDN_GETDISPINFOW
    } else {
        HDN_GETDISPINFOW.wrapping_add(HDN_UNICODE_OFFSET)
    };
    let lparam = h.items.get(i as usize).map_or(0, |it| it.lparam);
    let (text, max) = if mask & HDI_TEXT != 0 {
        cpu.bus.guest_write_32(scratch + S_TEXT, 0);
        (scratch + S_TEXT, MAX_HEADER_TEXT_LEN)
    } else {
        (0, 0)
    };
    write_words(
        cpu,
        scratch + S_NM,
        &[hwnd, id, code, i as u32, mask, text, max, 0, lparam],
    );
    vec![send_op(h.notify, WM_NOTIFY, Arg::V(id), Arg::S(S_NM))]
}

/// After HDN_GETDISPINFO: the text and image the parent gave, kept for
/// good if it asked with HDI_DI_SETITEM.
fn read_disp_info(
    cpu: &mut Cpu,
    scratch: u32,
    hwnd: u32,
    h: &mut Header,
    i: i32,
) -> (Option<Vec<u8>>, i32) {
    let w = read_words(cpu, scratch + S_NM, 9);
    let (mask, text_ptr, image) = (w[4], w[5], w[7] as i32);
    let unicode = h.format == NFR_UNICODE;
    let Some(item) = h.items.get_mut(i as usize) else {
        return (None, 0);
    };
    let asked = item.callback;
    let mut text = None;
    if asked & HDI_TEXT != 0 && text_ptr != 0 && text_ptr != LPSTR_TEXTCALLBACK {
        text = read_text(cpu, text_ptr, unicode);
    }
    let image = if asked & HDI_IMAGE != 0 {
        image
    } else {
        item.image
    };
    if mask & HDI_DI_SETITEM != 0 {
        if asked & HDI_TEXT != 0 {
            item.text = text.clone();
        }
        if asked & HDI_IMAGE != 0 {
            item.image = image;
        }
        item.callback &= !mask;
        let kept = h.clone();
        put(cpu, hwnd, kept);
    }
    (text, image)
}

/// HEADER_SetHotDivider: the divider set.
fn set_hot_divider(cpu: &mut Cpu, hwnd: u32, h: &mut Header, by_point: bool, lp: u32) -> i32 {
    let client = super::client(cpu, hwnd);
    if !h.rects_valid {
        h.set_bounds(client);
    }
    let mut divider;
    if by_point {
        let x = lp as u16 as i16 as i32;
        let (flags, item) = h.hit_test(client, x, 0);
        divider = item;
        if flags & HHT_TOLEFT != 0 {
            divider = 0;
        } else if flags & (HHT_NOWHERE | HHT_TORIGHT) != 0 {
            divider = h.count();
        } else if let Some(it) = h.items.get(divider as usize)
            && x > (it.rect.left + it.rect.right) / 2
        {
            divider = h.next_item(divider);
        }
    } else {
        divider = lp as i32;
    }
    if divider < -1 || divider > h.count() {
        return divider;
    }
    if divider != h.hot_divider {
        if h.hot_divider != -1 {
            let r = h.hot_divider_rect(client);
            invalidate_rect(cpu, hwnd, r, false);
        }
        h.hot_divider = divider;
        if divider != -1 {
            let r = h.hot_divider_rect(client);
            invalidate_rect(cpu, hwnd, r, false);
        }
    }
    divider
}

/// HDM_CREATEDRAGIMAGE: an image list with the item drawn in it. The
/// parent isn't asked for custom drawing here.
fn create_drag_image(cpu: &mut Cpu, hwnd: u32, i: i32) -> u32 {
    let Some(h) = with_bounds(cpu, hwnd) else {
        return 0;
    };
    if i < 0 || i >= h.count() {
        return 0;
    }
    let item = h.items[i as usize].clone();
    let (w, ht) = (item.rect.width(), item.rect.height());
    let screen = user32::paint::get_dc(cpu, &[0]).unwrap_or(0);
    let mem = gdi32::dc::create_compatible_dc(cpu, &[screen]).unwrap_or(0);
    let bmp =
        gdi32::bitmap::create_compatible_bitmap(cpu, &[screen, w as u32, ht as u32]).unwrap_or(0);
    user32::paint::release_dc(cpu, &[0, screen]);
    if mem == 0 || bmp == 0 {
        return 0;
    }
    let old = gfx::select(cpu, mem, bmp);
    let font = if h.font != 0 {
        h.font
    } else {
        gfx::stock(cpu, SYSTEM_FONT)
    };
    gfx::select(cpu, mem, font);
    let mut shifted = h.clone();
    shifted.items[i as usize].rect = item.rect.offset(-item.rect.left, -item.rect.top);
    let tc = gfx::sys_color(cpu, gfx::COLOR_BTNTEXT);
    gfx::set_text_color(cpu, mem, tc);
    let face = gfx::sys_color(cpu, gfx::COLOR_BTNFACE);
    gfx::set_bk_color(cpu, mem, face);
    draw_item_background(cpu, &shifted, mem, i);
    draw_item_content(cpu, &shifted, mem, i, item.text.as_deref(), item.image);
    gfx::select(cpu, mem, old);
    gdi32::dc::delete_dc(cpu, &[mem]);
    let himl = super::imagelist::create(cpu, w, ht, super::imagelist::ILC_COLORDDB, 1, 1);
    super::imagelist::il_add(cpu, &[himl, bmp, 0]);
    gdi32::delete_object(cpu, &[bmp]);
    himl
}

/// WM_CREATE: the control's state, then WM_NOTIFYFORMAT to the parent.
fn create(cpu: &mut Cpu, hwnd: u32, cs: u32) -> Option<u32> {
    let w = read_words(cpu, cs, 12);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let (parent, style) = (w[3], w[8]);
    let h = Header {
        notify: parent,
        format: NFR_ANSI,
        style,
        hot_item: -1,
        hot_divider: -1,
        margin: 3 * user32::sysparams::metric(SM_CXEDGE),
        height: font_height_of(cpu, 0) + VERT_BORDER,
        ..Header::default()
    };
    put(cpu, hwnd, h);
    let mut s = new_script(hwnd);
    s.send(parent, WM_NOTIFYFORMAT, Arg::V(hwnd), Arg::V(NF_QUERY))
        .then(T_FORMAT, &[])
        .ret(0);
    script::run(cpu, HEADER, s)
}

/// Whether the mouse moved far enough from where it was pressed to drag.
fn is_drag_distance(h: &Header, x: i32, y: i32) -> bool {
    (h.down_at.0 - x).abs() > user32::sysparams::metric(SM_CXDOUBLECLK)
        || (h.down_at.1 - y).abs() > user32::sysparams::metric(SM_CYDOUBLECLK)
}

/// A track line for resizing without HDS_FULLDRAG, drawn in XOR.
fn draw_track_line(cpu: &mut Cpu, hwnd: u32, x: i32) {
    let client = super::client(cpu, hwnd);
    let hdc = user32::paint::get_dc(cpu, &[hwnd]).unwrap_or(0);
    let pen = gfx::stock(cpu, BLACK_PEN);
    let old = gfx::select(cpu, hdc, pen);
    let rop = gdi32::dc::set_rop2(cpu, &[hdc, R2_XORPEN]).unwrap_or(0);
    gfx::line(cpu, hdc, (x, client.top), (x, client.bottom));
    gdi32::dc::set_rop2(cpu, &[hdc, rop]);
    gfx::select(cpu, hdc, old);
    user32::paint::release_dc(cpu, &[hwnd, hdc]);
}

fn refresh_item(cpu: &mut Cpu, hwnd: u32, h: &Header, i: i32) {
    if i >= 0
        && let Some(it) = h.items.get(i as usize)
    {
        invalidate_rect(cpu, hwnd, it.rect, false);
    }
}

/// WM_LBUTTONDOWN: press an item, or start resizing one.
fn button_down(cpu: &mut Cpu, hwnd: u32, lp: u32) -> Option<u32> {
    let (x, y) = user32::point_of(lp);
    let mut h = with_bounds(cpu, hwnd)?;
    let client = super::client(cpu, hwnd);
    let (flags, item) = h.hit_test(client, x, y);
    let mut s = new_script(hwnd);
    if h.style & HDS_BUTTONS != 0 && flags == HHT_ONHEADER {
        s.user("SetCapture", &[hwnd]);
        h.captured = true;
        h.pressed = true;
        h.dragging = false;
        h.move_item = item;
        h.down_at = (x, y);
        h.items[item as usize].down = true;
        refresh_item(cpu, hwnd, &h, item);
        put(cpu, hwnd, h);
    } else if flags == HHT_ONDIVIDER || flags == HHT_ONDIVOPEN {
        let width = h.items[item as usize].cxy;
        notify_later(&mut s, HDN_BEGINTRACKW, item, HDI_WIDTH, width);
        s.then(T_BEGIN_TRACK, &[item as u32, x as u32]);
    }
    s.ret(0);
    script::run(cpu, HEADER, s)
}

/// WM_LBUTTONUP: click, drop or finish resizing.
fn button_up(cpu: &mut Cpu, hwnd: u32, lp: u32) -> Option<u32> {
    let (x, y) = user32::point_of(lp);
    let mut h = with_bounds(cpu, hwnd)?;
    let mut s = new_script(hwnd);
    let mv = h.move_item;
    if h.pressed && mv >= 0 && mv < h.count() {
        h.items[mv as usize].down = false;
        if h.dragging {
            let new = if h.hot_divider == -1 {
                -1
            } else if h.hot_divider == h.count() {
                h.count() - 1
            } else {
                let mut o = h.items[h.hot_divider as usize].order;
                if o > h.items[mv as usize].order {
                    o -= 1;
                }
                o
            };
            if new != -1 {
                notify_later(&mut s, HDN_ENDDRAG, mv, HDI_ORDER, new);
                s.then(T_END_DRAG, &[mv as u32, new as u32]);
            } else {
                refresh_item(cpu, hwnd, &h, mv);
            }
            h.dragging = false;
            set_hot_divider(cpu, hwnd, &mut h, false, u32::MAX);
        } else {
            refresh_item(cpu, hwnd, &h, mv);
            if h.style & HDS_DRAGDROP == 0 || !is_drag_distance(&h, x, y) {
                notify_later(&mut s, HDN_ITEMCLICKW, mv, 0, 0);
            }
        }
        h.pressed = false;
    } else if h.tracking && mv >= 0 && mv < h.count() {
        let left = h.items[mv as usize].rect.left;
        let width = (x - left + h.track_offset).max(0);
        h.tracking = false;
        notify_later(&mut s, HDN_ENDTRACKW, mv, HDI_WIDTH, width);
        if h.style & HDS_FULLDRAG == 0 {
            draw_track_line(cpu, hwnd, h.old_track);
        }
        notify_later(&mut s, HDN_ITEMCHANGINGW, mv, HDI_WIDTH, width);
        s.then(T_CHANGING_WIDTH, &[mv as u32, width as u32, 1]);
    }
    if h.captured {
        h.captured = false;
        s.user("ReleaseCapture", &[]);
        simple_notify(cpu, &mut s, hwnd, &h, NM_RELEASEDCAPTURE);
    }
    put(cpu, hwnd, h);
    s.ret(0);
    script::run(cpu, HEADER, s)
}

/// WM_MOUSEMOVE: hot tracking, dragging and resizing.
fn mouse_move(cpu: &mut Cpu, hwnd: u32, lp: u32) -> Option<u32> {
    let (x, y) = user32::point_of(lp);
    let mut h = with_bounds(cpu, hwnd)?;
    let client = super::client(cpu, hwnd);
    let (flags, item) = h.hit_test(client, x, y);
    let hot_track = h.style & HDS_BUTTONS != 0 && h.style & HDS_HOTTRACK != 0;
    let old_hot = h.hot_item;
    if hot_track {
        h.hot_item = if flags & (HHT_ONHEADER | HHT_ONDIVIDER | HHT_ONDIVOPEN) != 0 {
            item
        } else {
            -1
        };
    }
    let mut s = new_script(hwnd);
    let mv = h.move_item;
    if h.captured && mv >= 0 && mv < h.count() {
        if h.pressed && !h.dragging && h.style & HDS_DRAGDROP != 0 && is_drag_distance(&h, x, y) {
            notify_later(&mut s, HDN_BEGINDRAG, mv, 0, 0);
            s.then(T_BEGIN_DRAG, &[lp]);
        }
        if h.dragging {
            set_hot_divider(cpu, hwnd, &mut h, true, lp);
        }
        if h.pressed && !h.dragging {
            let old = h.items[mv as usize].down;
            let down = item == mv && flags == HHT_ONHEADER;
            h.items[mv as usize].down = down;
            if old != down {
                refresh_item(cpu, hwnd, &h, mv);
            }
        } else if h.tracking {
            let left = h.items[mv as usize].rect.left;
            if h.style & HDS_FULLDRAG != 0 {
                let width = x - left + h.track_offset;
                notify_later(&mut s, HDN_ITEMCHANGINGW, mv, HDI_WIDTH, width);
                s.then(T_CHANGING_WIDTH, &[mv as u32, width as u32, 0]);
            } else {
                draw_track_line(cpu, hwnd, h.old_track);
                h.old_track = (x + h.track_offset).max(left);
                draw_track_line(cpu, hwnd, h.old_track);
                notify_later(&mut s, HDN_TRACKW, mv, HDI_WIDTH, h.old_track - left);
            }
        }
    }
    if hot_track && old_hot != h.hot_item && !h.dragging {
        refresh_item(cpu, hwnd, &h, old_hot);
        refresh_item(cpu, hwnd, &h, h.hot_item);
    }
    put(cpu, hwnd, h);
    s.ret(0);
    script::run(cpu, HEADER, s)
}

/// Draw the item's frame and background, the first half of
/// HEADER_DrawItem.
fn draw_item_background(cpu: &mut Cpu, h: &Header, hdc: u32, i: i32) {
    let item = &h.items[i as usize];
    let r = item.rect;
    if h.style & HDS_FLAT == 0 {
        if h.style & HDS_BUTTONS != 0 {
            if item.down {
                let f = BF_RECT | BF_FLAT | BF_MIDDLE | BF_ADJUST;
                gfx::edge(cpu, hdc, r, BDR_RAISEDOUTER, f);
            } else {
                let f = BF_RECT | BF_SOFT | BF_MIDDLE | BF_ADJUST;
                gfx::edge(cpu, hdc, r, EDGE_RAISED, f);
            }
        } else {
            gfx::edge(cpu, hdc, r, EDGE_ETCHED, BF_BOTTOM | BF_RIGHT | BF_ADJUST);
        }
    }
    let bk = gdi32::dc::get_bk_color(cpu, &[hdc]).unwrap_or(0);
    gfx::fill_rgb(cpu, hdc, h.inner_rect(i), bk);
}

/// Draw the item's text, image and bitmap, the rest of
/// HEADER_DrawItem, with `text` and `image` as the parent gave them
/// for callbacks.
fn draw_item_content(cpu: &mut Cpu, h: &Header, hdc: u32, i: i32, text: Option<&[u8]>, image: i32) {
    let item = &h.items[i as usize];
    let mut r = h.inner_rect(i);
    if item.down {
        r.left += 2;
        r.top += 2;
    }
    let margin = h.margin;
    let rw = r.width();
    let rh = r.height();
    let mut cw = 0;
    let mut iw = 0;
    let mut bw = 0;
    let (mut ix, mut bx) = (0, 0);
    let mut bmp_size = (0, 0);
    if item.fmt & HDF_STRING != 0 {
        let mut tr = Rect::default();
        let flags = DT_VCENTER | DT_SINGLELINE | DT_CALCRECT;
        gfx::draw_text(cpu, hdc, text.unwrap_or(&[]), &mut tr, flags);
        cw = tr.width() + 2 * margin;
    }
    let image_size = if h.himl != 0 {
        super::imagelist::icon_size(cpu, h.himl)
    } else {
        None
    };
    if item.fmt & HDF_IMAGE != 0
        && let Some((cx, _)) = image_size
    {
        iw = cx + 2 * margin;
    }
    if item.fmt & HDF_BITMAP != 0 && item.hbm != 0 {
        if let Some(b) = gdi32::gdi(cpu).bitmap(item.hbm) {
            bmp_size = (b.width, b.height);
        }
        bw = bmp_size.0 + 2 * margin;
    }
    let pic_w = if iw != 0 { iw } else { bw };
    if bw != 0 || iw != 0 {
        cw += pic_w;
    }
    let mut cx = match item.fmt & HDF_JUSTIFYMASK {
        HDF_LEFT => r.left,
        HDF_CENTER => r.left + rw / 2 - cw / 2,
        _ => r.right - cw,
    };
    if cx < r.left {
        cx = r.left;
    }
    if cx + cw > r.right {
        cw = r.right - cx;
    }
    let mut tx = cx + margin;
    let mut tw = cw - margin * 2;
    if iw != 0 || bw != 0 {
        tw -= pic_w;
        let px = if item.fmt & HDF_BITMAP_ON_RIGHT != 0 {
            cx + tw + margin * 3
        } else {
            tx += pic_w;
            cx + margin
        };
        if iw != 0 {
            ix = px;
        } else {
            bx = px;
        }
    }
    if iw != 0 && bw != 0 {
        bx = if item.fmt & HDF_JUSTIFYMASK == HDF_RIGHT {
            cx - bw + margin
        } else {
            cx + cw + margin
        };
    }
    if bw != 0 {
        let mem = gdi32::dc::create_compatible_dc(cpu, &[hdc]).unwrap_or(0);
        let old = gfx::select(cpu, mem, item.hbm);
        let (w, ht) = (bmp_size.0.min(r.right - bx).max(0), bmp_size.1);
        let y = r.top + (rh - ht) / 2;
        gdi32::draw::bit_blt(
            cpu,
            &[
                hdc, bx as u32, y as u32, w as u32, ht as u32, mem, 0, 0, SRCCOPY,
            ],
        );
        gfx::select(cpu, mem, old);
        gdi32::dc::delete_dc(cpu, &[mem]);
    }
    if iw != 0
        && let Some((icx, icy)) = image_size
    {
        let w = icx.min(r.right - ix).max(0);
        let y = r.top + (rh - icy) / 2;
        let dflt = gfx::CLR_DEFAULT;
        super::imagelist::draw_ex(cpu, h.himl, image, hdc, ix, y, w, icy, dflt, dflt, 0);
    }
    let explicit = HDF_OWNERDRAW | HDF_STRING | HDF_BITMAP | HDF_BITMAP_ON_RIGHT | HDF_IMAGE;
    if (item.fmt & HDF_STRING != 0 || item.fmt & explicit == 0)
        && let Some(t) = text
    {
        let old = gfx::set_bk_mode(cpu, hdc, TRANSPARENT);
        let mut tr = Rect::new(tx, r.top, tx + tw, r.bottom);
        let flags = DT_END_ELLIPSIS | DT_VCENTER | DT_SINGLELINE;
        gfx::draw_text(cpu, hdc, t, &mut tr, flags);
        if old != TRANSPARENT {
            gfx::set_bk_mode(cpu, hdc, old);
        }
    }
}

/// Write an NMCUSTOMDRAW at `S_NM` and send it: the steps.
#[allow(clippy::too_many_arguments)]
fn custom_draw_ops(
    cpu: &mut Cpu,
    scratch: u32,
    hwnd: u32,
    h: &Header,
    stage: u32,
    hdc: u32,
    r: Rect,
    spec: u32,
    item_state: u32,
    lparam: u32,
) -> Vec<Op> {
    let id = id_of(cpu, hwnd);
    write_words(
        cpu,
        scratch + S_NM,
        &[
            hwnd,
            id,
            NM_CUSTOMDRAW,
            stage,
            hdc,
            r.left as u32,
            r.top as u32,
            r.right as u32,
            r.bottom as u32,
            spec,
            item_state,
            lparam,
        ],
    );
    vec![send_op(h.notify, WM_NOTIFY, Arg::V(id), Arg::S(S_NM))]
}

/// Paint the items from the place `from` on (HEADER_Refresh's loop),
/// until one needs the parent: then the steps that ask it and come
/// back here.
fn paint_items(cpu: &mut Cpu, scratch: u32, hwnd: u32, hdc: u32, flags: u32, from: u32) -> Vec<Op> {
    let Some(h) = get(cpu, hwnd) else {
        return Vec::new();
    };
    let client = super::client(cpu, hwnd);
    let mut i = from as usize;
    while i < h.items.len() {
        let idx = h.order[i];
        let item = &h.items[idx as usize];
        if i > 0 && h.items[h.order[i - 1] as usize].rect.right > client.right {
            break;
        }
        if item.rect.width() == 0 {
            i += 1;
            continue;
        }
        let text_color = if h.hot_item == idx {
            gfx::COLOR_HIGHLIGHT
        } else {
            gfx::COLOR_BTNTEXT
        };
        let tc = gfx::sys_color(cpu, text_color);
        gfx::set_text_color(cpu, hdc, tc);
        let face = gfx::sys_color(cpu, gfx::COLOR_BTNFACE);
        gfx::set_bk_color(cpu, hdc, face);
        if flags & CDRF_NOTIFYITEMDRAW != 0 && item.fmt & HDF_OWNERDRAW == 0 {
            let st = if item.down { CDIS_SELECTED } else { 0 };
            let stage = CDDS_PREPAINT | CDDS_ITEM;
            let (r, lparam) = (item.rect, item.lparam);
            let mut ops = custom_draw_ops(
                cpu, scratch, hwnd, &h, stage, hdc, r, idx as u32, st, lparam,
            );
            ops.push(Op::Then {
                id: T_PAINT_ITEM_CD,
                data: vec![hdc, flags, i as u32],
            });
            return ops;
        }
        if let Some(ops) = paint_item_body(cpu, scratch, hwnd, &h, hdc, flags, i) {
            return ops;
        }
        i += 1;
    }
    paint_finish(cpu, scratch, hwnd, &h, hdc, flags)
}

/// An item's drawing after its custom draw: None when it was drawn
/// here, or the steps that ask the parent for it.
fn paint_item_body(
    cpu: &mut Cpu,
    scratch: u32,
    hwnd: u32,
    h: &Header,
    hdc: u32,
    flags: u32,
    place: usize,
) -> Option<Vec<Op>> {
    let idx = h.order[place];
    draw_item_background(cpu, h, hdc, idx);
    let item = &h.items[idx as usize];
    let next = Op::Then {
        id: T_PAINT_ITEM,
        data: vec![hdc, flags, place as u32 + 1],
    };
    if item.fmt & HDF_OWNERDRAW != 0 {
        let id = id_of(cpu, hwnd);
        let r = item.rect;
        let st = if item.down { ODS_SELECTED } else { 0 };
        write_words(
            cpu,
            scratch + S_NM,
            &[
                ODT_HEADER,
                id,
                idx as u32,
                ODA_DRAWENTIRE,
                st,
                hwnd,
                hdc,
                r.left as u32,
                r.top as u32,
                r.right as u32,
                r.bottom as u32,
                item.lparam,
            ],
        );
        gfx::set_bk_mode(cpu, hdc, TRANSPARENT);
        return Some(vec![
            send_op(h.notify, WM_DRAWITEM, Arg::V(id), Arg::S(S_NM)),
            next,
        ]);
    }
    let callback = item.callback & (HDI_TEXT | HDI_IMAGE);
    if callback != 0 {
        let mut ops = disp_info_ops(cpu, scratch, hwnd, h, idx, callback);
        ops.push(Op::Then {
            id: T_PAINT_ITEM_DISP,
            data: vec![hdc, flags, place as u32],
        });
        return Some(ops);
    }
    draw_item_content(cpu, h, hdc, idx, item.text.as_deref(), item.image);
    None
}

/// The end of HEADER_Refresh: the space after the items, the hot
/// divider, and CDDS_POSTPAINT if the parent asked for it.
fn paint_finish(
    cpu: &mut Cpu,
    scratch: u32,
    hwnd: u32,
    h: &Header,
    hdc: u32,
    flags: u32,
) -> Vec<Op> {
    let client = super::client(cpu, hwnd);
    let x = h
        .order
        .last()
        .map_or(client.left, |&i| h.items[i as usize].rect.right);
    let rest = Rect::new(x, client.top, client.right, client.bottom);
    if x <= client.right && !h.items.is_empty() {
        if h.style & HDS_FLAT != 0 {
            let b = gfx::sys_brush(cpu, gfx::COLOR_BTNFACE);
            gfx::fill(cpu, hdc, rest, b);
        } else if h.style & HDS_BUTTONS != 0 {
            let f = BF_TOP | BF_LEFT | BF_BOTTOM | BF_SOFT | BF_MIDDLE;
            gfx::edge(cpu, hdc, rest, EDGE_RAISED, f);
        } else {
            gfx::edge(cpu, hdc, rest, EDGE_ETCHED, BF_BOTTOM | BF_MIDDLE);
        }
    }
    if h.hot_divider != -1 {
        let r = h.hot_divider_rect(client);
        let c = gfx::sys_color(cpu, gfx::COLOR_HIGHLIGHT);
        gfx::fill_rgb(cpu, hdc, r, c);
    }
    let mut ops = vec![Op::Then {
        id: T_PAINT_POST,
        data: vec![hdc],
    }];
    if flags & CDRF_NOTIFYPOSTPAINT != 0 {
        ops.extend(custom_draw_ops(
            cpu,
            scratch,
            hwnd,
            h,
            CDDS_POSTPAINT,
            hdc,
            client,
            0,
            0,
            0,
        ));
    }
    ops
}

/// The continuations.
pub fn then(cpu: &mut Cpu, id: u16, c: &mut Ctx) -> Vec<Op> {
    let hwnd = c.hwnd;
    match id {
        T_SETITEM => {
            let Some(h) = get(cpu, hwnd) else {
                return vec![Op::Return(Arg::V(0))];
            };
            let i = c.word(0) as i32;
            let hd = HdItem::from_words(&c.data[1..]);
            let code = HDN_ITEMCHANGINGW;
            let mut ops = header_notify_ops(cpu, c.scratch, hwnd, &h, code, i, Some(&hd));
            ops.push(Op::Then {
                id: T_SETITEM_CHANGING,
                data: c.data.clone(),
            });
            ops
        }
        T_SETITEM_CHANGING => {
            if c.last != 0 {
                return vec![Op::Return(Arg::V(0))];
            }
            let Some(mut h) = get(cpu, hwnd) else {
                return vec![Op::Return(Arg::V(0))];
            };
            let i = c.word(0) as i32;
            if i < 0 || i >= h.count() {
                return vec![Op::Return(Arg::V(0))];
            }
            let hd = HdItem::from_words(&c.data[1..]);
            store(&mut h.items[i as usize], hd.mask, &hd);
            if hd.mask & HDI_ORDER != 0 && hd.order >= 0 && hd.order < h.count() {
                h.change_order(i, hd.order);
            }
            set_bounds(cpu, hwnd, &mut h);
            put(cpu, hwnd, h.clone());
            super::invalidate(cpu, hwnd, false);
            // The HDITEM sent with HDN_ITEMCHANGING goes again.
            let id = id_of(cpu, hwnd);
            let code = code_for(&h, HDN_ITEMCHANGEDW);
            write_words(cpu, c.scratch + S_NM, &[hwnd, id, code]);
            vec![
                send_op(h.notify, WM_NOTIFY, Arg::V(id), Arg::S(S_NM)),
                Op::Return(Arg::V(1)),
            ]
        }
        T_GETITEM => {
            let Some(h) = get(cpu, hwnd) else {
                return vec![Op::Return(Arg::V(0))];
            };
            let (i, mask) = (c.word(0) as i32, c.word(2));
            let callback = match h.items.get(i as usize) {
                Some(it) => mask & it.callback,
                None => return vec![Op::Return(Arg::V(0))],
            };
            let mut ops = disp_info_ops(cpu, c.scratch, hwnd, &h, i, callback);
            ops.push(Op::Then {
                id: T_GETITEM_DISP,
                data: c.data.clone(),
            });
            ops
        }
        T_GETITEM_DISP => {
            let Some(mut h) = get(cpu, hwnd) else {
                return vec![Op::Return(Arg::V(0))];
            };
            let (i, at, mask, wide) = (c.word(0) as i32, c.word(1), c.word(2), c.word(3) != 0);
            if i < 0 || i >= h.count() {
                return vec![Op::Return(Arg::V(0))];
            }
            let (text, image) = read_disp_info(cpu, c.scratch, hwnd, &mut h, i);
            let item = h.items[i as usize].clone();
            fill_item(cpu, &item, mask, at, wide, text.as_deref(), image);
            vec![Op::Return(Arg::V(1))]
        }
        T_NOTIFY => {
            let Some(h) = get(cpu, hwnd) else {
                return Vec::new();
            };
            let (code, item, mask, value) =
                (c.word(0), c.word(1) as i32, c.word(2), c.word(3) as i32);
            let hd = (mask != 0).then(|| int_field_item(&h, item, mask, value));
            header_notify_ops(cpu, c.scratch, hwnd, &h, code, item, hd.as_ref())
        }
        T_FORMAT => {
            if let Some(mut h) = get(cpu, hwnd) {
                h.format = c.last;
                put(cpu, hwnd, h);
            }
            c.result = c.last;
            Vec::new()
        }
        T_RETURN_LAST => {
            c.result = c.last;
            Vec::new()
        }
        T_PAINT => {
            let hdc = if c.word(0) != 0 { c.word(0) } else { c.last };
            let Some(mut h) = get(cpu, hwnd) else {
                return Vec::new();
            };
            if hdc == 0 {
                return Vec::new();
            }
            if !h.rects_valid {
                set_bounds(cpu, hwnd, &mut h);
                put(cpu, hwnd, h.clone());
            }
            let client = super::client(cpu, hwnd);
            let mut ops = custom_draw_ops(
                cpu,
                c.scratch,
                hwnd,
                &h,
                CDDS_PREPAINT,
                hdc,
                client,
                0,
                0,
                0,
            );
            ops.push(Op::Then {
                id: T_PAINT_PRE,
                data: vec![hdc],
            });
            ops
        }
        T_PAINT_PRE => {
            let (hdc, flags) = (c.word(0), c.last);
            let Some(mut h) = get(cpu, hwnd) else {
                return Vec::new();
            };
            let font = if h.font != 0 {
                h.font
            } else {
                gfx::stock(cpu, SYSTEM_FONT)
            };
            h.old_font = gfx::select(cpu, hdc, font);
            put(cpu, hwnd, h.clone());
            if h.items.is_empty() {
                let client = super::client(cpu, hwnd);
                let b = gfx::sys_brush(cpu, gfx::COLOR_BTNFACE);
                gfx::fill(cpu, hdc, client, b);
            }
            paint_items(cpu, c.scratch, hwnd, hdc, flags, 0)
        }
        T_PAINT_ITEM => paint_items(cpu, c.scratch, hwnd, c.word(0), c.word(1), c.word(2)),
        T_PAINT_ITEM_CD => {
            let (hdc, flags, place) = (c.word(0), c.word(1), c.word(2));
            if c.last & CDRF_SKIPDEFAULT == 0
                && let Some(h) = get(cpu, hwnd)
                && (place as usize) < h.items.len()
                && let Some(ops) =
                    paint_item_body(cpu, c.scratch, hwnd, &h, hdc, flags, place as usize)
            {
                return ops;
            }
            paint_items(cpu, c.scratch, hwnd, hdc, flags, place + 1)
        }
        T_PAINT_ITEM_DISP => {
            let (hdc, flags, place) = (c.word(0), c.word(1), c.word(2));
            if let Some(mut h) = get(cpu, hwnd)
                && (place as usize) < h.items.len()
            {
                let idx = h.order[place as usize];
                let (text, image) = read_disp_info(cpu, c.scratch, hwnd, &mut h, idx);
                let text = text.or_else(|| h.items[idx as usize].text.clone());
                draw_item_content(cpu, &h, hdc, idx, text.as_deref(), image);
            }
            paint_items(cpu, c.scratch, hwnd, hdc, flags, place + 1)
        }
        T_PAINT_POST => {
            if let Some(h) = get(cpu, hwnd) {
                gfx::select(cpu, c.word(0), h.old_font);
            }
            Vec::new()
        }
        T_BEGIN_TRACK => {
            if c.last != 0 {
                return Vec::new();
            }
            let Some(mut h) = get(cpu, hwnd) else {
                return Vec::new();
            };
            let (item, x) = (c.word(0) as i32, c.word(1) as i32);
            let Some(right) = h.items.get(item as usize).map(|it| it.rect.right) else {
                return Vec::new();
            };
            h.captured = true;
            h.tracking = true;
            h.move_item = item;
            h.track_offset = right - x;
            if h.style & HDS_FULLDRAG == 0 {
                h.old_track = right;
                draw_track_line(cpu, hwnd, right);
            }
            put(cpu, hwnd, h);
            vec![Op::Call {
                target: user32::address_of("SetCapture"),
                args: vec![Arg::V(hwnd)],
            }]
        }
        T_CHANGING_WIDTH => {
            if c.last != 0 {
                return Vec::new();
            }
            let Some(mut h) = get(cpu, hwnd) else {
                return Vec::new();
            };
            let (item, width, at_end) = (c.word(0) as i32, c.word(1) as i32, c.word(2) != 0);
            if item < 0 || item >= h.count() {
                return Vec::new();
            }
            h.items[item as usize].cxy = width.max(0);
            set_bounds(cpu, hwnd, &mut h);
            put(cpu, hwnd, h);
            super::invalidate(cpu, hwnd, at_end);
            let mut ops = Vec::new();
            if !at_end {
                ops.push(Op::Call {
                    target: user32::address_of("UpdateWindow"),
                    args: vec![Arg::V(hwnd)],
                });
            }
            ops.push(Op::Then {
                id: T_NOTIFY,
                data: vec![HDN_ITEMCHANGEDW, item as u32, HDI_WIDTH, width as u32],
            });
            ops
        }
        T_BEGIN_DRAG => {
            if c.last == 0
                && let Some(mut h) = get(cpu, hwnd)
            {
                h.dragging = true;
                set_hot_divider(cpu, hwnd, &mut h, true, c.word(0));
                put(cpu, hwnd, h);
            }
            Vec::new()
        }
        T_END_DRAG => {
            let Some(mut h) = get(cpu, hwnd) else {
                return Vec::new();
            };
            let (item, new) = (c.word(0) as i32, c.word(1) as i32);
            if c.last == 0 && item >= 0 && item < h.count() && new < h.count() && new >= 0 {
                h.change_order(item, new);
                h.rects_valid = false;
            }
            put(cpu, hwnd, h);
            super::invalidate(cpu, hwnd, false);
            Vec::new()
        }
        _ => Vec::new(),
    }
}
