//! Where the items are: their origins and rectangles in each view, the
//! size of an item, the list's area under the header, hit testing,
//! invalidation, and the scroll bars.

use super::*;
use crate::win32::user32::controls::scrollbar::{SB_HORZ, SB_VERT};
use crate::win32::user32::window::{
    SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW, WS_HSCROLL,
};

pub(super) const T_UPDATE_SIZE: u16 = T_LAYOUT;
const T_SCROLL: u16 = T_LAYOUT + 1;
const T_SCROLL_V: u16 = T_LAYOUT + 2;
const T_SCROLL_DONE: u16 = T_LAYOUT + 3;
const T_VSCROLL: u16 = T_LAYOUT + 4;
const T_HSCROLL: u16 = T_LAYOUT + 5;
const T_SCROLLED: u16 = T_LAYOUT + 6;
const T_ENSURE: u16 = T_LAYOUT + 7;
const T_ITEM_POSITION: u16 = T_LAYOUT + 8;
const T_SIZED: u16 = T_LAYOUT + 9;

const ICON_TOP_PADDING: i32 = 4;
const ICON_TOP_PADDING_HITABLE: i32 = 2;
const ICON_BOTTOM_PADDING: i32 = 4;
pub(super) const WIDTH_PADDING: i32 = 12;
pub(super) const HEIGHT_PADDING: i32 = 1;
pub(super) const REPORT_MARGINX: i32 = 2;
const MAX_EMPTYTEXT_SELECT_WIDTH: i32 = 80;
const DEFAULT_COLUMN_WIDTH: i32 = 96;
const SCROLL_ICON_LINE_SIZE: i32 = 37;
pub(super) const TRAILING_LABEL_PADDING: i32 = 12;

pub(super) const DT_CENTER: u32 = 0x0001;
pub(super) const DT_VCENTER: u32 = 0x0004;
pub(super) const DT_WORDBREAK: u32 = 0x0010;
pub(super) const DT_SINGLELINE: u32 = 0x0020;
pub(super) const DT_NOCLIP: u32 = 0x0100;
pub(super) const DT_CALCRECT: u32 = 0x0400;
pub(super) const DT_NOPREFIX: u32 = 0x0800;
pub(super) const DT_EDITCONTROL: u32 = 0x2000;
pub(super) const DT_END_ELLIPSIS: u32 = 0x8000;
pub(super) const DT_WORD_ELLIPSIS: u32 = 0x0004_0000;
/// How labels are drawn: several lines, a focused icon's full label,
/// and one line.
pub(super) const LV_ML_DT_FLAGS: u32 =
    DT_NOPREFIX | DT_EDITCONTROL | DT_CENTER | DT_WORDBREAK | DT_WORD_ELLIPSIS | DT_END_ELLIPSIS;
pub(super) const LV_FL_DT_FLAGS: u32 =
    DT_NOPREFIX | DT_EDITCONTROL | DT_CENTER | DT_WORDBREAK | DT_NOCLIP;
pub(super) const LV_SL_DT_FLAGS: u32 =
    DT_VCENTER | DT_NOPREFIX | DT_EDITCONTROL | DT_SINGLELINE | DT_WORD_ELLIPSIS | DT_END_ELLIPSIS;

const LVIR_BOUNDS: i32 = 0;
const LVIR_ICON: i32 = 1;
const LVIR_LABEL: i32 = 2;
const LVIR_SELECTBOUNDS: i32 = 3;

pub(super) const LVHT_NOWHERE: u32 = 0x0001;
pub(super) const LVHT_ONITEMICON: u32 = 0x0002;
pub(super) const LVHT_ONITEMLABEL: u32 = 0x0004;
pub(super) const LVHT_ONITEMSTATEICON: u32 = 0x0008;
pub(super) const LVHT_ONITEM: u32 = 0x000E;
const LVHT_ABOVE: u32 = 0x0008;
const LVHT_BELOW: u32 = 0x0010;
const LVHT_TORIGHT: u32 = 0x0020;
const LVHT_TOLEFT: u32 = 0x0040;

const LVA_DEFAULT: u32 = 0;
const LVA_ALIGNLEFT: u32 = 1;
const LVA_ALIGNTOP: u32 = 2;
const LVA_SNAPTOGRID: u32 = 5;

/// SB_ codes, and the one the control uses for its own scrolling.
pub(super) const SB_INTERNAL: u32 = 0xFFFF_FFFF;
const SB_LINEUP: u32 = 0;
const SB_LINEDOWN: u32 = 1;
const SB_PAGEUP: u32 = 2;
const SB_PAGEDOWN: u32 = 3;
const SB_THUMBPOSITION: u32 = 4;
const SB_THUMBTRACK: u32 = 5;

const SIF_RANGE: u32 = 0x0001;
const SIF_PAGE: u32 = 0x0002;
const SIF_POS: u32 = 0x0004;
const SIF_TRACKPOS: u32 = 0x0010;
const SIF_ALL: u32 = 0x0017;

const SM_CXICON: u32 = 11;
const SM_CYICON: u32 = 12;
const SM_CYHSCROLL: u32 = 3;
const SM_CXSMICON: u32 = 49;
const SM_CYSMICON: u32 = 50;

/// The scroll position of the bar `bar`.
fn scroll_pos(cpu: &mut Cpu, hwnd: u32, bar: u32) -> i32 {
    user32::user(cpu)
        .window(hwnd)
        .map_or(0, |w| w.scroll[bar as usize].pos)
}

/// LISTVIEW_GetOrigin: where the view's corner is in the client area.
pub(super) fn origin(cpu: &mut Cpu, hwnd: u32, lv: &ListView) -> (i32, i32) {
    let mut h = scroll_pos(cpu, hwnd, SB_HORZ);
    let mut v = scroll_pos(cpu, hwnd, SB_VERT);
    if lv.view == LV_VIEW_LIST {
        h *= lv.item_width;
    } else if lv.view == LV_VIEW_DETAILS {
        v *= lv.item_height;
    }
    (lv.list.left - h, lv.list.top - v)
}

/// The rectangle of column `col` in the header.
pub(super) fn column_rect(cpu: &mut Cpu, lv: &ListView, col: i32) -> Rect {
    if lv.header == 0 || col < 0 {
        return Rect::default();
    }
    header::item_rects(cpu, lv.header)
        .get(col as usize)
        .copied()
        .unwrap_or_default()
}

/// The right end of the columns: the right of the last in order.
fn columns_right(cpu: &mut Cpu, lv: &ListView) -> Option<i32> {
    if lv.columns.is_empty() || lv.header == 0 {
        return None;
    }
    let order = header::order_of(cpu, lv.header);
    let last = *order.last()?;
    Some(column_rect(cpu, lv, last).right)
}

/// LISTVIEW_GetItemOrigin: item `i`'s corner, from the view's.
pub(super) fn item_origin(cpu: &mut Cpu, lv: &ListView, i: i32) -> (i32, i32) {
    match lv.view {
        LV_VIEW_ICON | LV_VIEW_SMALLICON => lv.items.get(i as usize).map_or((0, 0), |it| it.pos),
        LV_VIEW_LIST => {
            let per = count_per_column(lv);
            (i / per * lv.item_width, i % per * lv.item_height)
        }
        _ => {
            let mut x = REPORT_MARGINX;
            if !lv.columns.is_empty() {
                x += column_rect(cpu, lv, 0).left;
            }
            (x, i * lv.item_height)
        }
    }
}

/// LISTVIEW_GetItemPosition.
pub(super) fn item_position(cpu: &mut Cpu, hwnd: u32, lv: &ListView, i: i32) -> (i32, i32) {
    let o = origin(cpu, hwnd, lv);
    let mut p = item_origin(cpu, lv, i);
    if lv.view == LV_VIEW_ICON {
        p.0 += (lv.item_width - lv.icon_size.0) / 2;
        p.1 += ICON_TOP_PADDING;
    }
    (p.0 + o.0, p.1 + o.1)
}

pub(super) fn count_per_row(lv: &ListView) -> i32 {
    let w = lv.list.width();
    (w / lv.item_width.max(1)).max(1)
}

pub(super) fn count_per_column(lv: &ListView) -> i32 {
    let h = lv.list.height();
    (h / lv.item_height.max(1)).max(1)
}

pub(super) fn count_per_page(lv: &ListView) -> i32 {
    match lv.view {
        LV_VIEW_DETAILS => count_per_column(lv),
        LV_VIEW_LIST => count_per_row(lv) * count_per_column(lv),
        _ => lv.count(),
    }
}

/// LISTVIEW_GetTopIndex.
pub(super) fn top_index(cpu: &mut Cpu, hwnd: u32, lv: &ListView) -> i32 {
    match lv.view {
        LV_VIEW_LIST => scroll_pos(cpu, hwnd, SB_HORZ) * count_per_column(lv),
        LV_VIEW_DETAILS => scroll_pos(cpu, hwnd, SB_VERT),
        _ => count_per_row(lv) * (scroll_pos(cpu, hwnd, SB_VERT) / lv.item_height.max(1)),
    }
}

/// What an item's metrics depend on.
#[derive(Clone, Debug, Default)]
pub(super) struct Shape {
    pub sub: i32,
    /// Whether it is an icon with the focus, whose label shows whole.
    pub focused: bool,
    pub indent: i32,
    pub image: i32,
    pub text: Vec<u8>,
}

impl Shape {
    /// Item `i`'s, as the control keeps it.
    pub fn of(lv: &ListView, i: i32, sub: i32) -> Shape {
        let item = lv.items.get(i as usize);
        let indent = item.map_or(0, |it| it.indent).max(0);
        let image = match item {
            Some(it) if sub == 0 => it.image,
            Some(it) => it
                .subs
                .iter()
                .find(|s| s.sub == sub)
                .map_or(I_IMAGECALLBACK, |s| s.image),
            None => 0,
        };
        let focused = lv.view == LV_VIEW_ICON && lv.focus && lv.focused == i && sub == 0;
        Shape {
            sub,
            focused,
            indent,
            image,
            text: lv.text_of(i, sub),
        }
    }
}

/// An item's rectangles, from its corner (LISTVIEW_GetItemMetrics).
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Metrics {
    pub bounds: Rect,
    pub select: Rect,
    pub icon: Rect,
    pub state: Rect,
    pub label: Rect,
}

/// The rectangle DT_CALCRECT gives `text` in `r` with `flags`.
fn calc_text(cpu: &mut Cpu, hwnd: u32, lv: &ListView, text: &[u8], r: Rect, flags: u32) -> Rect {
    let hdc = user32::paint::get_dc(cpu, &[hwnd]).unwrap_or(0);
    let font = lv.hfont(cpu);
    let old = gfx::select(cpu, hdc, font);
    let mut r = r;
    gfx::draw_text(cpu, hdc, text, &mut r, flags | DT_CALCRECT);
    gfx::select(cpu, hdc, old);
    user32::paint::release_dc(cpu, &[hwnd, hdc]);
    r
}

/// LISTVIEW_GetItemMetrics.
pub(super) fn metrics(cpu: &mut Cpu, hwnd: u32, lv: &ListView, s: &Shape) -> Metrics {
    let report = lv.view == LV_VIEW_DETAILS;
    let col = if s.sub != 0 || report {
        Some(column_rect(cpu, lv, s.sub))
    } else {
        None
    };
    let col_fmt = lv.columns.get(s.sub as usize).map_or(0, |c| c.fmt);
    let mut bx = match col {
        Some(c) if s.sub != 0 => c,
        _ => Rect::new(0, 0, lv.item_width, 0),
    };
    bx.top = 0;
    bx.bottom = lv.item_height;

    // The icon and the state icon.
    let state_width = if lv.himl_state != 0 && s.sub == 0 {
        lv.icon_state_size.0
    } else {
        0
    };
    let mut icon;
    if lv.view == LV_VIEW_ICON {
        let mut left = bx.left + state_width;
        if lv.himl_normal != 0 {
            left += (lv.item_width - lv.icon_size.0 - state_width) / 2;
        }
        let top = bx.top + ICON_TOP_PADDING;
        icon = Rect::new(left, top, left, top);
        if lv.himl_normal != 0 {
            icon.right += lv.icon_size.0;
            icon.bottom += lv.icon_size.1;
        }
    } else {
        let mut left = bx.left + state_width;
        if report && s.sub == 0 {
            left += lv.icon_size.0 * s.indent + REPORT_MARGINX;
        }
        icon = Rect::new(left, bx.top, left, bx.top + lv.icon_size.1);
        const LVCFMT_IMAGE: u32 = 0x0800;
        if lv.himl_small != 0
            && (col.is_none()
                || s.sub == 0
                || col_fmt & LVCFMT_IMAGE != 0
                || (lv.ex_style & LVS_EX_SUBITEMIMAGES != 0 && s.image != I_IMAGECALLBACK))
        {
            icon.right += lv.icon_size.0;
        }
    }
    let state = Rect::new(
        icon.left - state_width,
        icon.top,
        icon.left,
        icon.top + lv.icon_size.1,
    );

    // The label.
    let mut label = Rect::new(0, 0, bx.right, 0);
    if report && s.sub == 0 {
        let c = col.unwrap_or_default();
        label = c.offset(-c.left, 0);
    }
    let mut size = (0, 0);
    if s.sub != 0 || (lv.style & LVS_OWNERDRAWFIXED != 0 && report) {
        size = (lv.item_width, lv.item_height);
    } else if !s.text.is_empty() {
        let mut r = Rect::new(0, 0, lv.item_width - TRAILING_LABEL_PADDING, lv.item_height);
        if lv.view == LV_VIEW_ICON {
            r.bottom -= ICON_TOP_PADDING + lv.icon_size.1 + ICON_BOTTOM_PADDING;
        }
        let flags = if lv.view == LV_VIEW_ICON {
            if s.focused {
                LV_FL_DT_FLAGS
            } else {
                LV_ML_DT_FLAGS
            }
        } else {
            LV_SL_DT_FLAGS
        };
        let r = calc_text(cpu, hwnd, lv, &s.text, r, flags);
        if r.right != r.left {
            size.0 = (r.width() + TRAILING_LABEL_PADDING).min(lv.item_width);
        }
        size.1 = r.height();
    }
    if lv.view == LV_VIEW_ICON {
        label.left = bx.left + (lv.item_width - size.0) / 2;
        label.top = bx.top + ICON_TOP_PADDING_HITABLE + lv.icon_size.1 + ICON_BOTTOM_PADDING;
        label.right = label.left + size.0;
        let th = lv.text_height.max(1);
        if !s.focused && size.1 > th {
            size.1 = size.1.min(bx.bottom - label.top);
            size.1 = (size.1 / th).max(1) * th;
        }
        label.bottom = label.top + size.1 + HEIGHT_PADDING;
    } else if report {
        let c = col.unwrap_or_default();
        label.left = icon.right;
        label.top = bx.top;
        label.right = if s.sub != 0 { c.right } else { c.width() };
        label.bottom = label.top + lv.item_height;
    } else {
        label.left = icon.right;
        label.top = bx.top;
        label.right = (label.left + size.0).min(label.right);
        label.bottom = label.top + lv.item_height;
    }

    // The selection's rectangle.
    let select = if report {
        let right = if size.0 != 0 {
            (label.left + size.0).min(label.right)
        } else {
            (label.left + MAX_EMPTYTEXT_SELECT_WIDTH).min(label.right)
        };
        Rect::new(icon.left, bx.top, right, bx.bottom)
    } else {
        icon.union(&label)
    };
    let bounds = if s.focused { bx.union(&label) } else { bx };
    Metrics {
        bounds,
        select,
        icon,
        state,
        label,
    }
}

/// LISTVIEW_GetItemBox: item `i`'s box in the client area.
pub(super) fn item_box(cpu: &mut Cpu, hwnd: u32, lv: &ListView, i: i32) -> Rect {
    let o = origin(cpu, hwnd, lv);
    let p = item_origin(cpu, lv, i);
    let mut s = Shape::of(lv, i, 0);
    if !s.focused {
        s.text.clear();
    }
    let b = metrics(cpu, hwnd, lv, &s).bounds;
    let full_row = lv.view == LV_VIEW_DETAILS
        && lv.ex_style & LVS_EX_FULLROWSELECT != 0
        && header::order_of(cpu, lv.header)
            .first()
            .copied()
            .unwrap_or(0)
            != 0;
    if full_row {
        b.offset(o.0, p.1 + o.1)
    } else {
        b.offset(p.0 + o.0, p.1 + o.1)
    }
}

/// LISTVIEW_GetItemRect for `code` (an LVIR_ value).
pub(super) fn item_rect(
    cpu: &mut Cpu,
    hwnd: u32,
    lv: &ListView,
    i: i32,
    code: i32,
) -> Option<Rect> {
    if i < 0 || i >= lv.count() {
        return None;
    }
    let o = origin(cpu, hwnd, lv);
    let p = item_origin(cpu, lv, i);
    let mut s = Shape::of(lv, i, 0);
    s.focused = lv.view == LV_VIEW_ICON && code != LVIR_ICON && lv.focus && lv.focused == i;
    let report = lv.view == LV_VIEW_DETAILS;
    let mut code = code;
    if report && lv.ex_style & LVS_EX_FULLROWSELECT != 0 && code == LVIR_SELECTBOUNDS {
        code = LVIR_BOUNDS;
    }
    let m = metrics(cpu, hwnd, lv, &s);
    let r = match code {
        LVIR_ICON => m.icon,
        LVIR_LABEL => m.label,
        LVIR_BOUNDS => m.bounds,
        LVIR_SELECTBOUNDS => m.select,
        _ => return None,
    };
    Some(if report {
        if code != LVIR_BOUNDS {
            let left = column_rect(cpu, lv, 0).left;
            r.offset(o.0 + left, p.1 + o.1)
        } else {
            r.offset(o.0, p.1 + o.1)
        }
    } else {
        r.offset(p.0 + o.0, p.1 + o.1)
    })
}

/// LVM_GETITEMRECT.
pub(super) fn get_item_rect_msg(
    cpu: &mut Cpu,
    hwnd: u32,
    lv: &ListView,
    i: i32,
    lp: u32,
) -> Option<u32> {
    if lp == 0 {
        return Some(0);
    }
    let code = cpu.bus.guest_read_32(lp) as i32;
    if cpu.bus.guest_faulted() {
        return None;
    }
    let Some(r) = item_rect(cpu, hwnd, lv, i, code) else {
        return Some(0);
    };
    r.write(cpu, lp);
    Some(1)
}

/// LVM_GETSUBITEMRECT.
pub(super) fn get_subitem_rect_msg(
    cpu: &mut Cpu,
    hwnd: u32,
    lv: &ListView,
    i: i32,
    lp: u32,
) -> Option<u32> {
    if lp == 0 {
        return Some(0);
    }
    let w = read_words(cpu, lp, 2);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let (code, col) = (w[0] as i32, w[1] as i32);
    if col == 0 {
        return get_item_rect_msg(cpu, hwnd, lv, i, lp);
    }
    if lv.view != LV_VIEW_DETAILS {
        return Some(0);
    }
    if i == -1 {
        if code != LVIR_BOUNDS {
            return Some(0);
        }
        let r = if lv.header != 0 {
            if col < 0 || col as usize >= header::item_rects(cpu, lv.header).len() {
                return Some(0);
            }
            column_rect(cpu, lv, col)
        } else {
            Rect::default()
        };
        r.write(cpu, lp);
        return Some(1);
    }
    // COMCTL32 4.7x gives a subitem's rectangle for any row.
    if i < 0 {
        return Some(0);
    }
    let pos = item_position(cpu, hwnd, lv, i);
    let o = origin(cpu, hwnd, lv);
    if col < 0 || col as usize >= lv.columns.len() {
        return Some(0);
    }
    let s = Shape {
        sub: col,
        ..Shape::default()
    };
    let m = metrics(cpu, hwnd, lv, &s);
    let r = match code {
        LVIR_ICON => m.icon,
        LVIR_LABEL | LVIR_BOUNDS => m.bounds,
        _ => return Some(0),
    };
    r.offset(o.0, pos.1).write(cpu, lp);
    Some(1)
}

/// LVM_GETITEMPOSITION. In a report with columns COMCTL32 asks the
/// header where the first column is.
pub(super) fn get_item_position_msg(
    cpu: &mut Cpu,
    hwnd: u32,
    lv: &ListView,
    i: i32,
    lp: u32,
) -> Option<u32> {
    if lp == 0 || i < 0 || i >= lv.count() {
        return Some(0);
    }
    if lv.view == LV_VIEW_DETAILS && !lv.columns.is_empty() && lv.header != 0 {
        let ops = vec![
            send_op(
                lv.header,
                header::HDM_GETITEMRECT,
                Arg::V(0),
                Arg::S(S_RECT),
            ),
            step(T_ITEM_POSITION, &[i as u32, lp]),
        ];
        return run_ops(cpu, hwnd, ops, 1);
    }
    let p = item_position(cpu, hwnd, lv, i);
    write_words(cpu, lp, &[p.0 as u32, p.1 as u32]);
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(1)
}

/// LVM_GETITEMSPACING.
pub(super) fn item_spacing(_cpu: &mut Cpu, lv: &ListView, small: bool) -> u32 {
    let (x, y) = if !small {
        lv.icon_spacing
    } else if lv.view == LV_VIEW_ICON {
        (
            DEFAULT_COLUMN_WIDTH,
            user32::sysparams::metric(SM_CXSMICON) + HEIGHT_PADDING,
        )
    } else {
        (lv.item_width, lv.item_height)
    };
    (x as u32 & 0xFFFF) | ((y as u32 & 0xFFFF) << 16)
}

/// LISTVIEW_GetStringWidthT.
pub(super) fn string_width(cpu: &mut Cpu, hwnd: u32, lv: &ListView, text: &[u8]) -> i32 {
    if text.is_empty() {
        return 0;
    }
    let hdc = user32::paint::get_dc(cpu, &[hwnd]).unwrap_or(0);
    let font = lv.hfont(cpu);
    let old = gfx::select(cpu, hdc, font);
    let (w, _) = gfx::text_size(cpu, hdc, text);
    gfx::select(cpu, hdc, old);
    user32::paint::release_dc(cpu, &[hwnd, hdc]);
    w
}

/// LISTVIEW_SaveTextMetrics.
pub(super) fn save_text_metrics(cpu: &mut Cpu, hwnd: u32, lv: &mut ListView) {
    let hdc = user32::paint::get_dc(cpu, &[hwnd]).unwrap_or(0);
    let font = lv.hfont(cpu);
    let old = gfx::select(cpu, hdc, font);
    lv.text_height = gfx::font_height(cpu, hdc);
    lv.max_char_width = gfx::text_size(cpu, hdc, b"W").0;
    lv.ellipsis_width = gfx::text_size(cpu, hdc, b"...").0;
    gfx::select(cpu, hdc, old);
    user32::paint::release_dc(cpu, &[hwnd, hdc]);
}

/// LISTVIEW_CalculateItemWidth.
fn calc_item_width(cpu: &mut Cpu, hwnd: u32, lv: &ListView) -> i32 {
    match lv.view {
        LV_VIEW_ICON => lv.icon_spacing.0,
        LV_VIEW_DETAILS => columns_right(cpu, lv).unwrap_or(0),
        _ => {
            let mut w = 0;
            for i in 0..lv.items.len() as i32 {
                let text = lv.text_of(i, 0);
                w = w.max(string_width(cpu, hwnd, lv, &text));
            }
            if lv.himl_small != 0 {
                w += lv.icon_size.0;
            }
            if lv.himl_state != 0 {
                w += lv.icon_state_size.0;
            }
            DEFAULT_COLUMN_WIDTH.max(w + WIDTH_PADDING)
        }
    }
}

/// LISTVIEW_CalculateItemHeight.
pub(super) fn calc_item_height(lv: &ListView) -> i32 {
    let h = if lv.view == LV_VIEW_ICON {
        lv.icon_spacing.1
    } else {
        let mut h = lv.text_height;
        if lv.view == LV_VIEW_DETAILS && lv.ex_style & LVS_EX_GRIDLINES != 0 {
            h += 1;
        }
        if lv.himl_state != 0 {
            h = h.max(lv.icon_state_size.1);
        }
        if lv.himl_small != 0 {
            h = h.max(lv.icon_size.1);
        }
        if lv.himl_state != 0 || lv.himl_small != 0 {
            h += HEIGHT_PADDING;
        }
        if lv.measure_height > 0 {
            h = lv.measure_height;
        }
        h
    };
    h.max(1)
}

/// LISTVIEW_UpdateItemSize.
pub(super) fn update_item_size(cpu: &mut Cpu, hwnd: u32, lv: &mut ListView) {
    lv.item_width = calc_item_width(cpu, hwnd, lv);
    lv.item_height = calc_item_height(lv);
}

/// set_icon_size: an image list's icon size, or the system's.
pub(super) fn icon_size_of(cpu: &mut Cpu, himl: u32, small: bool) -> (i32, i32) {
    if himl != 0
        && let Some(s) = super::super::imagelist::icon_size(cpu, himl)
    {
        return s;
    }
    if small {
        (
            user32::sysparams::metric(SM_CXSMICON),
            user32::sysparams::metric(SM_CYSMICON),
        )
    } else {
        (
            user32::sysparams::metric(SM_CXICON),
            user32::sysparams::metric(SM_CYICON),
        )
    }
}

/// The next place for an icon, row by row.
pub(super) fn next_icon_pos_top(lv: &mut ListView) -> (i32, i32) {
    let w = lv.list.width();
    let p = lv.icon_pos;
    lv.icon_pos.0 += lv.item_width;
    if lv.icon_pos.0 + lv.item_width <= w {
        return p;
    }
    lv.icon_pos.0 = 0;
    lv.icon_pos.1 += lv.item_height;
    p
}

/// The next place for an icon, column by column.
pub(super) fn next_icon_pos_left(lv: &mut ListView) -> (i32, i32) {
    let h = lv.list.height();
    let p = lv.icon_pos;
    lv.icon_pos.1 += lv.item_height;
    if lv.icon_pos.1 + lv.item_height <= h {
        return p;
    }
    lv.icon_pos.0 += lv.item_width;
    lv.icon_pos.1 = 0;
    p
}

/// LISTVIEW_MoveIconTo.
fn move_icon_to(cpu: &mut Cpu, hwnd: u32, lv: &mut ListView, i: i32, p: (i32, i32)) {
    let Some(old) = lv.items.get(i as usize).map(|it| it.pos) else {
        return;
    };
    if old == p {
        return;
    }
    invalidate_item(cpu, hwnd, lv, i);
    lv.items[i as usize].pos = p;
    invalidate_item(cpu, hwnd, lv, i);
}

/// LISTVIEW_Arrange.
pub(super) fn arrange(cpu: &mut Cpu, hwnd: u32, lv: &mut ListView, code: u32) -> bool {
    if !lv.is_icon_view() {
        return false;
    }
    let code = if code == LVA_DEFAULT {
        if lv.style & LVS_ALIGNLEFT != 0 {
            LVA_ALIGNLEFT
        } else {
            LVA_ALIGNTOP
        }
    } else {
        code
    };
    let left = match code {
        LVA_ALIGNLEFT => true,
        LVA_ALIGNTOP | LVA_SNAPTOGRID => false,
        _ => return false,
    };
    lv.auto_arrange = true;
    lv.icon_pos = (0, 0);
    for i in 0..lv.items.len() as i32 {
        let p = if left {
            next_icon_pos_left(lv)
        } else {
            next_icon_pos_top(lv)
        };
        move_icon_to(cpu, hwnd, lv, i, p);
    }
    true
}

/// LISTVIEW_GetAreaRect.
pub(super) fn area_rect(lv: &ListView) -> Rect {
    let mut r = Rect::default();
    match lv.view {
        LV_VIEW_ICON | LV_VIEW_SMALLICON => {
            for it in &lv.items {
                r.right = r.right.max(it.pos.0);
                r.bottom = r.bottom.max(it.pos.1);
            }
            if !lv.items.is_empty() {
                r.right += lv.item_width;
                r.bottom += lv.item_height;
            }
        }
        LV_VIEW_LIST => {
            let y = count_per_column(lv);
            let mut x = lv.count() / y;
            if lv.count() % y != 0 {
                x += 1;
            }
            r.right = x * lv.item_width;
            r.bottom = y * lv.item_height;
        }
        _ => {}
    }
    r
}

/// LISTVIEW_GetViewRect.
pub(super) fn view_rect(cpu: &mut Cpu, hwnd: u32, lv: &ListView) -> Rect {
    let r = area_rect(lv);
    if lv.view != LV_VIEW_DETAILS {
        let o = origin(cpu, hwnd, lv);
        return r.offset(o.0, o.1);
    }
    r
}

/// LVM_APPROXIMATEVIEWRECT.
pub(super) fn approximate_view_rect(
    cpu: &mut Cpu,
    hwnd: u32,
    lv: &ListView,
    count: i32,
    cx: i32,
    cy: i32,
) -> u32 {
    let count = if count == -1 { lv.count() } else { count };
    let (mut w, mut h) = (cx & 0xFFFF, cy & 0xFFFF);
    match lv.view {
        LV_VIEW_LIST => {
            if h == 0xFFFF {
                h = lv.list.height();
            }
            h = h.max(lv.item_height);
            let mut per_col = 1;
            let mut cols = 0;
            if count > 0 && lv.item_height > 0 {
                per_col = (h / lv.item_height).max(1);
                cols = if count % per_col != 0 {
                    count / per_col
                } else {
                    count / per_col + 1
                };
            }
            h = per_col * lv.item_height + 2;
            w = cols * lv.item_width + 2;
        }
        LV_VIEW_DETAILS => {
            if lv.count() > 0 {
                let b = item_box(cpu, hwnd, lv, 0);
                w = b.width();
                h = b.height() * count;
            } else {
                if h == 0xFFFF {
                    h = lv.list.height();
                }
                if w == 0xFFFF {
                    w = lv.list.width();
                }
            }
        }
        LV_VIEW_ICON => {
            let (iw, ih) = lv.icon_spacing;
            if w == 0xFFFF {
                w = lv.list.width();
            }
            w = w.max(iw);
            let cols = (w / iw.max(1)).min(count).max(1);
            let rows = if count != 0 {
                count / cols + (count % cols != 0) as i32
            } else {
                0
            };
            h = ih * rows + 2;
            w = iw * cols + 2;
        }
        _ => return 0,
    }
    (w as u32 & 0xFFFF) | ((h as u32 & 0xFFFF) << 16)
}

/// The items whose boxes meet `frame`, in client coordinates
/// (iterator_frameditems).
pub(super) fn items_in(cpu: &mut Cpu, hwnd: u32, lv: &ListView, frame: Rect) -> Vec<i32> {
    let o = origin(cpu, hwnd, lv);
    let f = frame.offset(-o.0, -o.1);
    let count = lv.count();
    let (w, h) = (lv.item_width, lv.item_height.max(1));
    let mut out = Vec::new();
    match lv.view {
        LV_VIEW_ICON | LV_VIEW_SMALLICON => {
            for (i, it) in lv.items.iter().enumerate() {
                let r = Rect::new(it.pos.0, it.pos.1, it.pos.0 + w, it.pos.1 + h);
                if !r.intersect(&f).is_empty() {
                    out.push(i as i32);
                }
            }
            if lv.view == LV_VIEW_ICON && lv.focused >= 0 && !out.contains(&lv.focused) {
                let b = item_box(cpu, hwnd, lv, lv.focused);
                if !b.intersect(&frame).is_empty() {
                    out.push(lv.focused);
                }
            }
        }
        LV_VIEW_DETAILS => {
            if f.left >= w || f.top >= h * count {
                return out;
            }
            let lo = (f.top / h).max(0);
            let hi = ((f.bottom - 1) / h).min(count - 1) + 1;
            out.extend(lo..hi);
        }
        _ => {
            let per = count_per_column(lv);
            let first_row = (f.top / h).max(0);
            let last_row = ((f.bottom - 1) / h).min(per - 1);
            let last = (count + per - 1) / per;
            let (first_col, last_col) = if w != 0 {
                ((f.left / w).max(0), ((f.right - 1) / w).min(last))
            } else {
                (f.left.max(0), (f.right - 1).min(last))
            };
            if last_col < first_col || last_row < first_row {
                return out;
            }
            for col in first_col..=last_col {
                let lo = col * per + first_row;
                if lo >= count {
                    break;
                }
                let hi = (col * per + last_row + 1).min(count);
                out.extend(lo..hi);
            }
        }
    }
    out
}

/// What a hit test found.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Hit {
    pub flags: u32,
    pub item: i32,
    pub sub: i32,
}

/// LISTVIEW_HitTest at (x, y).
pub(super) fn hit_test(
    cpu: &mut Cpu,
    hwnd: u32,
    lv: &ListView,
    x: i32,
    y: i32,
    subitem: bool,
    select: bool,
) -> Hit {
    let mut hit = Hit {
        flags: 0,
        item: -1,
        sub: 0,
    };
    let o = origin(cpu, hwnd, lv);
    let report = lv.view == LV_VIEW_DETAILS;
    let list = lv.list;
    if subitem && report {
        if list.left > x && o.0 < x {
            hit.flags |= LVHT_TOLEFT;
        }
        let oy = if y < list.top && y >= 0 {
            y + list.top
        } else {
            y
        };
        if list.bottom < oy {
            hit.flags |= LVHT_BELOW;
        }
    } else {
        if list.left > x {
            hit.flags |= LVHT_TOLEFT;
        } else if list.right < x {
            hit.flags |= LVHT_TORIGHT;
        }
        // In the report view the header's rows count as the first
        // item's.
        let header_row = report && y >= 0 && y < list.top;
        if list.top > y && !header_row {
            hit.flags |= LVHT_ABOVE;
        } else if list.bottom < y {
            hit.flags |= LVHT_BELOW;
        }
    }
    if report && subitem {
        let ox = x - o.0;
        hit.sub = -1;
        for j in 0..lv.columns.len() as i32 {
            let r = column_rect(cpu, lv, j);
            if ox >= r.left && ox < r.right {
                hit.sub = j;
                break;
            }
        }
        if hit.sub == -1 {
            hit.item = -1;
            hit.flags = LVHT_NOWHERE;
            return hit;
        }
    }
    if hit.flags != 0 {
        return hit;
    }
    hit.flags |= LVHT_NOWHERE;
    let Some(&item) = items_in(cpu, hwnd, lv, Rect::new(x, y, x + 1, y + 1)).first() else {
        return hit;
    };
    let sub = if subitem { hit.sub } else { 0 };
    let mut shape = Shape::of(lv, item, sub);
    if !lv.focus {
        shape.focused = false;
    }
    let m = metrics(cpu, hwnd, lv, &shape);
    let p = item_origin(cpu, lv, item);
    let mut ox = x - p.0 - o.0;
    let oy = if y < list.top && y >= 0 {
        y - p.1 - o.1 + list.top
    } else {
        y - p.1 - o.1
    };
    let mut bounds = if report {
        if lv.ex_style & LVS_EX_FULLROWSELECT != 0 {
            ox = x - o.0;
        }
        m.bounds
    } else {
        m.icon.union(&m.label).union(&m.state)
    };
    if !bounds.contains(ox, oy) {
        return hit;
    }
    if m.icon.contains(ox, oy) {
        hit.flags |= LVHT_ONITEMICON;
    } else if m.label.contains(ox, oy) {
        hit.flags |= LVHT_ONITEMLABEL;
    } else if lv.himl_state != 0 && m.state.contains(ox, oy) {
        hit.flags |= LVHT_ONITEMSTATEICON;
    }
    if report && lv.ex_style & LVS_EX_FULLROWSELECT != 0 && hit.flags & LVHT_ONITEM == 0 {
        hit.flags = LVHT_ONITEM | LVHT_ABOVE;
    }
    if hit.flags & LVHT_ONITEM != 0 {
        hit.flags &= !LVHT_NOWHERE;
    }
    let mut found = item;
    let whole_row =
        report && (lv.ex_style & LVS_EX_FULLROWSELECT != 0 || lv.style & LVS_OWNERDRAWFIXED != 0);
    if select && !whole_row {
        if report {
            let mut main = Shape::of(lv, item, 0);
            main.focused = false;
            let m = metrics(cpu, hwnd, lv, &main);
            bounds = m.icon.union(&m.label).union(&m.state);
        }
        if !bounds.contains(ox, oy) {
            found = -1;
        }
    }
    hit.item = found;
    hit
}

/// LVM_HITTEST and LVM_SUBITEMHITTEST.
pub(super) fn hit_test_msg(
    cpu: &mut Cpu,
    hwnd: u32,
    lv: &ListView,
    lp: u32,
    subitem: bool,
) -> Option<u32> {
    if lp == 0 {
        return Some(-1i32 as u32);
    }
    let w = read_words(cpu, lp, 2);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let hit = hit_test(cpu, hwnd, lv, w[0] as i32, w[1] as i32, subitem, !subitem);
    cpu.bus.guest_write_32(lp + 8, hit.flags);
    cpu.bus.guest_write_32(lp + 12, hit.item as u32);
    if subitem {
        cpu.bus.guest_write_32(lp + 16, hit.sub as u32);
    }
    Some(hit.item as u32)
}

// Invalidation.

pub(super) fn invalidate_rect(cpu: &mut Cpu, hwnd: u32, lv: &ListView, r: Rect) {
    if lv.redraw {
        header::invalidate_rect(cpu, hwnd, r, true);
    }
}

pub(super) fn invalidate_list(cpu: &mut Cpu, hwnd: u32, lv: &ListView) {
    if lv.redraw {
        super::super::invalidate(cpu, hwnd, true);
    }
}

pub(super) fn invalidate_item(cpu: &mut Cpu, hwnd: u32, lv: &ListView, i: i32) {
    if !lv.redraw || i < 0 || i >= lv.count() {
        return;
    }
    let b = item_box(cpu, hwnd, lv, i);
    invalidate_rect(cpu, hwnd, lv, b);
}

pub(super) fn invalidate_subitem(cpu: &mut Cpu, hwnd: u32, lv: &ListView, i: i32, sub: i32) {
    if !lv.redraw {
        return;
    }
    let o = origin(cpu, hwnd, lv);
    let p = item_origin(cpu, lv, i);
    let mut r = column_rect(cpu, lv, sub);
    r.top = 0;
    r.bottom = lv.item_height;
    let r = r.offset(o.0 + p.0, o.1 + p.1);
    invalidate_rect(cpu, hwnd, lv, r);
}

pub(super) fn invalidate_column(cpu: &mut Cpu, hwnd: u32, lv: &ListView, col: i32) {
    if !lv.redraw {
        return;
    }
    let mut r = column_rect(cpu, lv, col);
    r.top = lv.list.top;
    r.bottom = lv.list.bottom;
    invalidate_rect(cpu, hwnd, lv, r);
}

/// LVM_SETITEMCOUNT's invalidation with LVSICF_NOINVALIDATEALL.
pub(super) fn invalidate_count_change(
    cpu: &mut Cpu,
    hwnd: u32,
    lv: &ListView,
    _old: i32,
    _new: i32,
) {
    invalidate_list(cpu, hwnd, lv);
}

// Sizes and scroll bars.

/// LVM_SETICONSPACING (LISTVIEW_SetIconSpacing): the spacing before.
pub(super) fn set_icon_spacing(cpu: &mut Cpu, hwnd: u32, mut lv: ListView, lp: u32) -> u32 {
    let old = (lv.icon_spacing.0 as u32 & 0xFFFF) | ((lv.icon_spacing.1 as u32 & 0xFFFF) << 16);
    let (cx, cy) = (lp as u16 as i16 as i32, (lp >> 16) as u16 as i16 as i32);
    set_spacing(cpu, hwnd, &mut lv, cx, cy);
    put(cpu, hwnd, lv);
    old
}

fn set_spacing(cpu: &mut Cpu, hwnd: u32, lv: &mut ListView, cx: i32, cy: i32) {
    if lv.view != LV_VIEW_ICON {
        return;
    }
    if cx == -1 && cy == -1 {
        lv.custom_spacing = false;
    } else if cx != 0 || cy != 0 {
        lv.custom_spacing = true;
    }
    if !lv.custom_spacing {
        lv.icon_spacing = default_spacing(cpu, lv);
        update_item_size(cpu, hwnd, lv);
        return;
    }
    let (dx, dy) = default_spacing(cpu, lv);
    let mut cx = if cx == -1 { dx } else { cx };
    let mut cy = if cy == -1 { dy } else { cy };
    if cx == 0 {
        cx = lv.icon_spacing.0;
    }
    if cy == 0 {
        cy = lv.icon_spacing.1;
    }
    lv.icon_spacing = (cx, cy);
    update_item_size(cpu, hwnd, lv);
}

/// The icon spacing COMCTL32 4.7x starts with: the system's spacing
/// less the system's icon, plus the icon of the normal image list.
pub(super) fn default_spacing(cpu: &mut Cpu, lv: &ListView) -> (i32, i32) {
    let icon = if lv.himl_normal != 0 {
        super::super::imagelist::icon_size(cpu, lv.himl_normal).unwrap_or((0, 0))
    } else {
        (0, 0)
    };
    let m = user32::sysparams::metric;
    (m(38) - m(SM_CXICON) + icon.0, m(39) - m(SM_CYICON) + icon.1)
}

/// LVM_SETIMAGELIST: the list before.
pub(super) fn set_image_list(
    cpu: &mut Cpu,
    hwnd: u32,
    mut lv: ListView,
    kind: u32,
    himl: u32,
) -> Option<u32> {
    let old_height = lv.item_height;
    let old = match kind {
        0 => {
            let old = lv.himl_normal;
            lv.himl_normal = himl;
            if lv.view == LV_VIEW_ICON {
                lv.icon_size = icon_size_of(cpu, himl, false);
            }
            set_spacing(cpu, hwnd, &mut lv, 0, 0);
            old
        }
        1 => {
            let old = lv.himl_small;
            lv.himl_small = himl;
            if lv.view != LV_VIEW_ICON {
                lv.icon_size = icon_size_of(cpu, himl, true);
            }
            old
        }
        2 => {
            let old = lv.himl_state;
            lv.himl_state = himl;
            lv.icon_state_size = icon_size_of(cpu, himl, true);
            if himl != 0 {
                super::super::imagelist::il_set_bk_color(cpu, &[himl, gfx::CLR_NONE]);
            }
            old
        }
        _ => return Some(0),
    };
    lv.item_height = calc_item_height(&lv);
    put(cpu, hwnd, lv.clone());
    let ops = if lv.item_height != old_height {
        update_scroll_ops(&lv)
    } else {
        Vec::new()
    };
    run_ops(cpu, hwnd, ops, old)
}

/// The state image list LVS_EX_CHECKBOXES makes: an empty box and a
/// checked one (LISTVIEW_CreateCheckBoxIL).
pub(super) fn check_box_image_list(cpu: &mut Cpu, hwnd: u32, _lv: &ListView) -> u32 {
    use super::super::imagelist;
    use crate::win32::gdi32;
    let cx = user32::sysparams::metric(SM_CXSMICON);
    let cy = user32::sysparams::metric(SM_CYSMICON);
    let himl = imagelist::create(
        cpu,
        cx,
        cy,
        imagelist::ILC_COLOR | imagelist::ILC_MASK,
        2,
        2,
    );
    let wnd_dc = user32::paint::get_dc(cpu, &[hwnd]).unwrap_or(0);
    let hdc = gdi32::dc::create_compatible_dc(cpu, &[wnd_dc]).unwrap_or(0);
    let im =
        gdi32::bitmap::create_compatible_bitmap(cpu, &[wnd_dc, cx as u32, cy as u32]).unwrap_or(0);
    let mask = gdi32::bitmap::create_bitmap(cpu, &[cx as u32, cy as u32, 1, 1, 0]).unwrap_or(0);
    user32::paint::release_dc(cpu, &[hwnd, wnd_dc]);
    let full = Rect::new(0, 0, cx, cy);
    let orig = gfx::select(cpu, hdc, mask);
    let white = gfx::stock(cpu, 0);
    gfx::fill(cpu, hdc, full, white);
    let inner = full.inflate(-2, -2);
    let black = gfx::stock(cpu, 4);
    gfx::fill(cpu, hdc, inner, black);
    const DFC_BUTTON: u32 = 4;
    const DFCS_BUTTONCHECK: u32 = 0;
    const DFCS_CHECKED: u32 = 0x0400;
    const DFCS_MONO: u32 = 0x8000;
    for checked in [0, DFCS_CHECKED] {
        gfx::select(cpu, hdc, im);
        let brush = gfx::sys_brush(cpu, gfx::COLOR_WINDOW);
        gfx::fill(cpu, hdc, full, brush);
        let state = DFCS_BUTTONCHECK | DFCS_MONO | checked;
        user32::paint::frame_control(cpu, hdc, inner, DFC_BUTTON, state);
        gfx::select(cpu, hdc, orig);
        imagelist::il_add(cpu, &[himl, im, mask]);
    }
    gdi32::delete_object(cpu, &[mask]);
    gdi32::delete_object(cpu, &[im]);
    gdi32::dc::delete_dc(cpu, &[hdc]);
    himl
}

/// LVM_SETITEMPOSITION.
pub(super) fn set_item_position(
    cpu: &mut Cpu,
    hwnd: u32,
    mut lv: ListView,
    i: i32,
    x: i32,
    y: i32,
) -> bool {
    if i < 0 || i >= lv.count() || !lv.is_icon_view() || lv.ownerdata() {
        return false;
    }
    let o = origin(cpu, hwnd, &lv);
    let (mut x, mut y) = if x == -1 && y == -1 { o } else { (x, y) };
    if lv.view == LV_VIEW_ICON {
        x -= (lv.item_width - lv.icon_size.0) / 2;
        y -= ICON_TOP_PADDING;
    }
    lv.auto_arrange = false;
    move_icon_to(cpu, hwnd, &mut lv, i, (x - o.0, y - o.1));
    put(cpu, hwnd, lv);
    true
}

/// LVM_CREATEDRAGIMAGE: an image list with the item's picture, and
/// where its corner is.
pub(super) fn create_drag_image(
    cpu: &mut Cpu,
    hwnd: u32,
    lv: &ListView,
    i: i32,
    lp: u32,
) -> Option<u32> {
    use super::super::imagelist;
    use crate::win32::gdi32;
    if lp == 0 {
        return Some(0);
    }
    let Some(r) = item_rect(cpu, hwnd, lv, i, LVIR_BOUNDS) else {
        return Some(0);
    };
    write_words(cpu, lp, &[r.left as u32, r.top as u32]);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let (w, h) = (r.width().max(1), r.height().max(1));
    let himl = imagelist::create(cpu, w, h, imagelist::ILC_COLOR, 1, 1);
    let wnd_dc = user32::paint::get_dc(cpu, &[hwnd]).unwrap_or(0);
    let hdc = gdi32::dc::create_compatible_dc(cpu, &[wnd_dc]).unwrap_or(0);
    let bm =
        gdi32::bitmap::create_compatible_bitmap(cpu, &[wnd_dc, w as u32, h as u32]).unwrap_or(0);
    user32::paint::release_dc(cpu, &[hwnd, wnd_dc]);
    let orig = gfx::select(cpu, hdc, bm);
    let bk = if lv.clr_bk == gfx::CLR_NONE {
        gfx::sys_color(cpu, gfx::COLOR_WINDOW)
    } else {
        lv.clr_bk
    };
    gfx::fill_rgb(cpu, hdc, Rect::new(0, 0, w, h), bk);
    gfx::select(cpu, hdc, orig);
    imagelist::il_add(cpu, &[himl, bm, 0]);
    gdi32::delete_object(cpu, &[bm]);
    gdi32::dc::delete_dc(cpu, &[hdc]);
    Some(himl)
}

/// The steps of LISTVIEW_UpdateSize: the list's area, and the header
/// laid out at its top.
fn update_size(cpu: &mut Cpu, hwnd: u32) -> Vec<Op> {
    let Some(mut lv) = get(cpu, hwnd) else {
        return Vec::new();
    };
    let mut list = super::super::client(cpu, hwnd);
    let wstyle = super::super::style(cpu, hwnd);
    if lv.view == LV_VIEW_LIST {
        if wstyle & WS_HSCROLL == 0 {
            list.bottom -= user32::sysparams::metric(SM_CYHSCROLL);
        }
        list.bottom = (list.bottom - 2).max(0);
    }
    let mut ops = Vec::new();
    if lv.header != 0 {
        let hstyle = super::super::style(cpu, lv.header);
        let height = state(cpu)
            .header
            .headers
            .get(&lv.header)
            .map_or(0, |h| h.height);
        let mut cy = if hstyle & header::HDS_HIDDEN != 0 {
            0
        } else {
            height
        };
        let mut flags = SWP_NOZORDER;
        if lv.header_enabled() {
            flags |= SWP_SHOWWINDOW;
        } else {
            flags |= SWP_HIDEWINDOW;
            cy = 0;
        }
        let want = Rect::new(list.left, list.top, list.right, list.top + cy);
        ops.extend(place_header(cpu, lv.header, want, flags));
        list.top = cy.max(0);
    }
    if lv.view == LV_VIEW_DETAILS && lv.ex_style & LVS_EX_GRIDLINES != 0 {
        list.top += 2;
    }
    lv.list = list;
    put(cpu, hwnd, lv);
    ops
}

/// SetWindowPos for the header, unless it is there already.
fn place_header(cpu: &mut Cpu, hdr: u32, want: Rect, flags: u32) -> Vec<Op> {
    let (rect, style) = user32::user(cpu)
        .window(hdr)
        .map_or((Rect::default(), 0), |w| (w.rect, w.style));
    let visible = style & user32::window::WS_VISIBLE != 0;
    let show_ok =
        (flags & SWP_SHOWWINDOW == 0 || visible) && (flags & SWP_HIDEWINDOW == 0 || !visible);
    if rect == want && show_ok {
        return Vec::new();
    }
    let args = [
        Arg::V(hdr),
        Arg::V(0),
        Arg::V(want.left as u32),
        Arg::V(want.top as u32),
        Arg::V(want.width() as u32),
        Arg::V(want.height() as u32),
        Arg::V(flags),
    ];
    vec![user_op("SetWindowPos", &args)]
}

/// The steps of LISTVIEW_UpdateScroll.
pub(super) fn update_scroll_ops(lv: &ListView) -> Vec<Op> {
    if lv.style & LVS_NOSCROLL != 0 || !lv.redraw {
        return Vec::new();
    }
    vec![step(T_SCROLL, &[])]
}

/// Write a SCROLLINFO at `S_SCROLL`.
fn write_scroll_info(cpu: &mut Cpu, scratch: u32, mask: u32, max: i32, page: i32, pos: i32) {
    write_words(
        cpu,
        scratch + S_SCROLL,
        &[28, mask, 0, max as u32, page as u32, pos as u32, 0],
    );
}

fn set_scroll_op(hwnd: u32, bar: u32) -> Op {
    user_op(
        "SetScrollInfo",
        &[Arg::V(hwnd), Arg::V(bar), Arg::S(S_SCROLL), Arg::V(1)],
    )
}

/// The horizontal bar's range and page.
fn horz_info(cpu: &mut Cpu, hwnd: u32, lv: &ListView) -> (i32, i32) {
    let mut page = lv.list.width();
    let mut max;
    match lv.view {
        LV_VIEW_LIST => {
            let per = count_per_column(lv);
            max = (lv.count() + per - 1) / per;
            if page < lv.item_width {
                page = lv.item_width;
            }
            if lv.item_width != 0 {
                page /= lv.item_width;
            }
        }
        LV_VIEW_DETAILS => max = lv.item_width,
        _ => {
            let r = view_rect(cpu, hwnd, lv);
            max = r.width();
        }
    }
    if lv.header_enabled()
        && let Some(right) = columns_right(cpu, lv)
    {
        max = right;
    }
    ((max - 1).max(0), page)
}

/// The vertical bar's range and page.
fn vert_info(cpu: &mut Cpu, hwnd: u32, lv: &ListView) -> (i32, i32) {
    let mut page = lv.list.height();
    let mut max = 0;
    if lv.view == LV_VIEW_DETAILS {
        max = lv.count();
        if page < lv.item_height {
            page = lv.item_height;
        }
        if lv.item_height > 0 {
            page /= lv.item_height;
        }
    } else if lv.view != LV_VIEW_LIST {
        max = view_rect(cpu, hwnd, lv).height();
    }
    ((max - 1).max(0), page)
}

/// LISTVIEW_UpdateHeaderSize: the header follows the horizontal
/// scroll position.
fn header_size_ops(cpu: &mut Cpu, lv: &ListView, pos: i32) -> Vec<Op> {
    if lv.header == 0 {
        return Vec::new();
    }
    let rect = user32::user(cpu)
        .window(lv.header)
        .map_or(Rect::default(), |w| w.rect);
    let (x, y) = (-pos, rect.top);
    let (cx, cy) = (rect.right + pos, rect.bottom);
    let flags = if lv.style & LVS_NOCOLUMNHEADER != 0 {
        SWP_HIDEWINDOW
    } else {
        SWP_SHOWWINDOW | SWP_NOZORDER | SWP_NOACTIVATE
    };
    place_header(cpu, lv.header, Rect::new(x, y, x + cx, y + cy), flags)
}

/// The steps of LISTVIEW_VScroll and LISTVIEW_HScroll.
pub(super) fn vscroll_ops(code: u32, diff: i32) -> Vec<Op> {
    vec![step(T_VSCROLL, &[code, diff as u32])]
}

pub(super) fn hscroll_ops(code: u32, diff: i32) -> Vec<Op> {
    vec![step(T_HSCROLL, &[code, diff as u32])]
}

/// The steps of LISTVIEW_MouseWheel.
pub(super) fn wheel_ops(lv: &ListView, delta: i32) -> Vec<Op> {
    let d = -delta;
    match lv.view {
        LV_VIEW_ICON | LV_VIEW_SMALLICON => {
            let diff = if d < 0 {
                -SCROLL_ICON_LINE_SIZE
            } else {
                SCROLL_ICON_LINE_SIZE
            };
            vscroll_ops(SB_INTERNAL, diff)
        }
        LV_VIEW_DETAILS => {
            if d.abs() >= 120 {
                let lines = count_per_column(lv).min(3) * (d / 120);
                vscroll_ops(SB_INTERNAL, lines)
            } else {
                Vec::new()
            }
        }
        _ => hscroll_ops(if d < 0 { SB_LINEUP } else { SB_LINEDOWN }, 0),
    }
}

/// LVM_SCROLL.
pub(super) fn scroll_msg(cpu: &mut Cpu, hwnd: u32, lv: &ListView, dx: i32, dy: i32) -> Option<u32> {
    let (mut dx, mut dy) = (dx, dy);
    match lv.view {
        LV_VIEW_DETAILS => {
            let h = lv.item_height.max(1);
            dy += (if dy < 0 { -1 } else { 1 }) * h / 2;
            dy /= h;
        }
        LV_VIEW_LIST => {
            if dy != 0 {
                return Some(0);
            }
        }
        _ => dx = 0,
    }
    let mut ops = Vec::new();
    if dx != 0 {
        ops.extend(hscroll_ops(SB_INTERNAL, dx));
    }
    if dy != 0 {
        ops.extend(vscroll_ops(SB_INTERNAL, dy));
    }
    run_ops(cpu, hwnd, ops, 1)
}

/// The steps of LISTVIEW_EnsureVisible.
pub(super) fn ensure_visible_ops(i: i32, partial: bool) -> Vec<Op> {
    vec![step(T_ENSURE, &[i as u32, partial as u32])]
}

fn ensure_visible(cpu: &mut Cpu, hwnd: u32, i: i32, partial: bool) -> Vec<Op> {
    let Some(lv) = get(cpu, hwnd) else {
        return Vec::new();
    };
    let Some(r) = item_rect(cpu, hwnd, &lv, i, LVIR_BOUNDS) else {
        return Vec::new();
    };
    let list = lv.list;
    if partial && !r.intersect(&list).is_empty() {
        return Vec::new();
    }
    let (mut pos_w, mut pos_h) = (0, 0);
    let (mut h_adj, mut v_adj, mut h_diff, mut v_diff) = (0, 0, 0, 0);
    if r.left < list.left || r.right > list.right {
        if lv.view == LV_VIEW_LIST {
            pos_w = lv.item_width;
        } else if lv.is_icon_view() {
            pos_w = 1;
        }
        if r.left < list.left {
            h_adj = -1;
            if lv.view != LV_VIEW_DETAILS {
                h_diff = r.left - list.left;
            }
        } else {
            h_adj = 1;
            if lv.view != LV_VIEW_DETAILS {
                h_diff = r.right - list.right;
            }
        }
    }
    if r.top < list.top || r.bottom > list.bottom {
        if lv.view == LV_VIEW_DETAILS {
            pos_h = lv.item_height;
        } else if lv.is_icon_view() {
            pos_h = 1;
        }
        if r.top < list.top {
            v_adj = -1;
            if lv.view != LV_VIEW_LIST {
                v_diff = r.top - list.top;
            }
        } else {
            v_adj = 1;
            if lv.view != LV_VIEW_LIST {
                v_diff = r.bottom - list.bottom;
            }
        }
    }
    let mut ops = Vec::new();
    if pos_w != 0 {
        let mut diff = h_diff / pos_w;
        if h_diff % pos_w != 0 {
            diff += h_adj;
        }
        ops.extend(hscroll_ops(SB_INTERNAL, diff));
    }
    if pos_h != 0 {
        let mut diff = v_diff / pos_h;
        if v_diff % pos_h != 0 {
            diff += v_adj;
        }
        ops.extend(vscroll_ops(SB_INTERNAL, diff));
    }
    ops
}

/// LISTVIEW_VScroll and LISTVIEW_HScroll up to SetScrollInfo.
fn scroll(cpu: &mut Cpu, c: &mut Ctx, bar: u32) -> Vec<Op> {
    let (code, diff) = (c.word(0), c.word(1) as i32);
    let Some(lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    let mut ops = Vec::new();
    if lv.edit != 0 {
        let msg = user32::message::WM_KILLFOCUS;
        ops.push(send_op(lv.edit, msg, Arg::V(0), Arg::V(0)));
    }
    let at = c.scratch + S_SCROLL;
    write_words(cpu, at, &[28, SIF_ALL | SIF_TRACKPOS]);
    let got = user32::controls::scrollbar::get_scroll_info(cpu, &[c.hwnd, bar, at]).unwrap_or(0);
    if got == 0 {
        c.result = 1;
        return ops;
    }
    let w = read_words(cpu, at, 7);
    let (min, max, page, pos, track) = (
        w[2] as i32,
        w[3] as i32,
        w[4] as i32,
        w[5] as i32,
        w[6] as i32,
    );
    let icon = lv.is_icon_view();
    let diff = match code {
        SB_INTERNAL => diff,
        SB_LINEUP => {
            if bar == SB_VERT && icon {
                -SCROLL_ICON_LINE_SIZE
            } else {
                -1
            }
        }
        SB_LINEDOWN => {
            if bar == SB_VERT && icon {
                SCROLL_ICON_LINE_SIZE
            } else {
                1
            }
        }
        SB_PAGEUP => -page,
        SB_PAGEDOWN => page,
        SB_THUMBPOSITION | SB_THUMBTRACK => track - pos,
        _ => 0,
    };
    if diff == 0 {
        return ops;
    }
    let mut new = pos.wrapping_add(diff);
    if diff > 0 {
        if new < pos || new > max {
            new = max;
        }
    } else if new > pos || new < min {
        new = min;
    }
    write_scroll_info(cpu, c.scratch, SIF_POS, 0, 0, new);
    ops.push(set_scroll_op(c.hwnd, bar));
    ops.push(step(T_SCROLLED, &[bar, pos as u32]));
    ops
}

/// The rest once the bar has its position: the list moves.
fn scrolled(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let (bar, old) = (c.word(0), c.word(1) as i32);
    let new = c.last as i32;
    let Some(mut lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    if new == old {
        return Vec::new();
    }
    let mut ops = Vec::new();
    let mut diff = old - new;
    let (dx, dy) = if bar == SB_VERT {
        if lv.view == LV_VIEW_DETAILS {
            diff *= lv.item_height;
        }
        (0, diff)
    } else {
        ops.extend(header_size_ops(cpu, &lv, new));
        if lv.view == LV_VIEW_LIST {
            diff *= lv.item_width;
        }
        (diff, 0)
    };
    // scroll_list: the list is drawn again where it moved.
    let list = lv.list;
    invalidate_rect(cpu, c.hwnd, &lv, list);
    lv.rc_focus = lv.rc_focus.offset(dx, dy);
    put(cpu, c.hwnd, lv);
    ops.push(user_op("UpdateWindow", &[Arg::V(c.hwnd)]));
    ops
}

/// The steps of LISTVIEW_ScrollOnInsert, after item `i` was inserted
/// (`dir` 1) or deleted (-1).
pub(super) fn scroll_on_insert_ops(cpu: &mut Cpu, hwnd: u32, i: i32, dir: i32) -> Vec<Op> {
    let Some(mut lv) = get(cpu, hwnd) else {
        return Vec::new();
    };
    if !lv.redraw {
        return Vec::new();
    }
    if lv.auto_arranged() {
        let count = lv.count();
        let skip = (dir < 0 && i >= count) || (dir > 0 && i == count - 1);
        if !skip {
            arrange(cpu, hwnd, &mut lv, 0);
            put(cpu, hwnd, lv.clone());
        }
    }
    let ops = update_scroll_ops(&lv);
    let per = match lv.view {
        LV_VIEW_DETAILS => lv.count() + 1,
        LV_VIEW_LIST => count_per_column(&lv),
        _ => return ops,
    };
    let (col, row) = (i / per, i % per);
    let o = origin(cpu, hwnd, &lv);
    let w = lv.item_width;
    let h = lv.item_height;
    let r = Rect::new(col * w, row * h, col * w + w, per * h).offset(o.0, o.1);
    let r = r.intersect(&lv.list);
    if !r.is_empty() {
        invalidate_rect(cpu, hwnd, &lv, r);
    }
    if lv.view == LV_VIEW_LIST {
        let right = (lv.count() / per + 1) * w;
        let r = Rect::new((col + 1) * w, 0, right, per * h).offset(o.0, o.1);
        let r = r.intersect(&lv.list);
        if !r.is_empty() {
            invalidate_rect(cpu, hwnd, &lv, r);
        }
    }
    ops
}

/// WM_SIZE (LISTVIEW_Size).
pub(super) fn size(cpu: &mut Cpu, hwnd: u32, lv: ListView) -> Option<u32> {
    let old = lv.list;
    let words = [old.left, old.top, old.right, old.bottom].map(|v| v as u32);
    let ops = vec![step(T_UPDATE_SIZE, &[]), step(T_SIZED, &words)];
    run_ops(cpu, hwnd, ops, 0)
}

/// What WM_SIZE does once the area is known.
fn sized(cpu: &mut Cpu, hwnd: u32, old: Rect) -> Vec<Op> {
    let Some(mut lv) = get(cpu, hwnd) else {
        return Vec::new();
    };
    if lv.list == old || !lv.redraw {
        return Vec::new();
    }
    if lv.auto_arranged() {
        arrange(cpu, hwnd, &mut lv, 0);
        put(cpu, hwnd, lv.clone());
    }
    let ops = update_scroll_ops(&lv);
    let h = lv.item_height.max(1);
    if lv.view == LV_VIEW_LIST && old.height() / h != lv.list.height() / h {
        invalidate_list(cpu, hwnd, &lv);
    }
    ops
}

/// WM_STYLECHANGED (LISTVIEW_StyleChanged).
pub(super) fn style_changed(
    cpu: &mut Cpu,
    hwnd: u32,
    mut lv: ListView,
    wp: u32,
    lp: u32,
) -> Option<u32> {
    if wp != (-16i32) as u32 {
        return Some(0);
    }
    let w = read_words(cpu, lp, 2);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let (old, new) = (w[0], w[1]);
    let (old_view, new_view) = (old & LVS_TYPEMASK, new & LVS_TYPEMASK);
    lv.style = new;
    lv.map_view();
    let mut ops = Vec::new();
    let show_bar = |bar: u32| user_op("ShowScrollBar", &[Arg::V(hwnd), Arg::V(bar), Arg::V(0)]);
    if old & WS_HSCROLL != 0 && new & WS_HSCROLL == 0 {
        ops.push(show_bar(SB_HORZ));
    }
    if old & user32::window::WS_VSCROLL != 0 && new & user32::window::WS_VSCROLL == 0 {
        ops.push(show_bar(SB_VERT));
    }
    if new_view != old_view {
        if lv.edit != 0 {
            let msg = user32::message::WM_KILLFOCUS;
            ops.push(send_op(lv.edit, msg, Arg::V(0), Arg::V(0)));
        }
        if lv.header != 0 {
            ops.push(user_op("ShowWindow", &[Arg::V(lv.header), Arg::V(0)]));
        }
        ops.push(show_bar(3));
        lv.rc_focus = Rect::default();
        let old_size = lv.icon_size;
        let himl = if new_view == LVS_ICON {
            lv.himl_normal
        } else {
            lv.himl_small
        };
        lv.icon_size = icon_size_of(cpu, himl, new_view != LVS_ICON);
        if new_view == LVS_ICON {
            if lv.icon_size != old_size {
                set_spacing(cpu, hwnd, &mut lv, 0, 0);
            }
        } else if new_view == LVS_REPORT {
            ops.extend(super::columns::create_header_ops(&lv));
            ops.push(step(super::columns::T_REPORT_HEADER, &[]));
        }
        update_item_size(cpu, hwnd, &mut lv);
    }
    if new_view == LVS_REPORT && (old ^ new) & LVS_NOCOLUMNHEADER != 0 && lv.header != 0 {
        let hs = super::super::style(cpu, lv.header);
        if new & LVS_NOCOLUMNHEADER != 0 {
            if let Some(w) = user32::user(cpu).window_mut(lv.header) {
                w.style = hs | header::HDS_HIDDEN;
            }
        } else if hs & header::HDS_HIDDEN != 0
            && let Some(w) = user32::user(cpu).window_mut(lv.header)
        {
            w.style = (hs & !header::HDS_HIDDEN) | user32::window::WS_VISIBLE;
        }
    }
    if (new_view == LVS_ICON || new_view == 2)
        && (new_view != old_view || (new ^ old) & LVS_ALIGNMASK != 0)
    {
        arrange(cpu, hwnd, &mut lv, 0);
    }
    put(cpu, hwnd, lv.clone());
    ops.push(step(T_UPDATE_SIZE, &[]));
    ops.extend(update_scroll_ops(&lv));
    invalidate_list(cpu, hwnd, &lv);
    run_ops(cpu, hwnd, ops, 0)
}

/// The continuations.
pub(super) fn then(cpu: &mut Cpu, id: u16, c: &mut Ctx) -> Vec<Op> {
    match id {
        T_UPDATE_SIZE => update_size(cpu, c.hwnd),
        T_SCROLL => {
            let Some(lv) = get(cpu, c.hwnd) else {
                return Vec::new();
            };
            if lv.style & LVS_NOSCROLL != 0 || !lv.redraw {
                return Vec::new();
            }
            let (max, page) = horz_info(cpu, c.hwnd, &lv);
            write_scroll_info(cpu, c.scratch, SIF_RANGE | SIF_PAGE, max, page, 0);
            let old = scroll_pos(cpu, c.hwnd, SB_HORZ);
            vec![
                set_scroll_op(c.hwnd, SB_HORZ),
                step(T_SCROLL_V, &[old as u32]),
            ]
        }
        T_SCROLL_V => {
            let dx = c.word(0) as i32 - c.last as i32;
            let Some(lv) = get(cpu, c.hwnd) else {
                return Vec::new();
            };
            let (max, page) = vert_info(cpu, c.hwnd, &lv);
            write_scroll_info(cpu, c.scratch, SIF_RANGE | SIF_PAGE, max, page, 0);
            let old = scroll_pos(cpu, c.hwnd, SB_VERT);
            vec![
                set_scroll_op(c.hwnd, SB_VERT),
                step(T_SCROLL_DONE, &[dx as u32, old as u32]),
            ]
        }
        T_SCROLL_DONE => {
            let dx = c.word(0) as i32;
            let dy = c.word(1) as i32 - c.last as i32;
            let Some(lv) = get(cpu, c.hwnd) else {
                return Vec::new();
            };
            if dx != 0 || dy != 0 {
                let list = lv.list;
                invalidate_rect(cpu, c.hwnd, &lv, list);
            }
            let pos = scroll_pos(cpu, c.hwnd, SB_HORZ);
            header_size_ops(cpu, &lv, pos)
        }
        T_SIZED => {
            let old = Rect::new(
                c.word(0) as i32,
                c.word(1) as i32,
                c.word(2) as i32,
                c.word(3) as i32,
            );
            sized(cpu, c.hwnd, old)
        }
        T_VSCROLL => scroll(cpu, c, SB_VERT),
        T_HSCROLL => scroll(cpu, c, SB_HORZ),
        T_SCROLLED => scrolled(cpu, c),
        T_ENSURE => ensure_visible(cpu, c.hwnd, c.word(0) as i32, c.word(1) != 0),
        T_ITEM_POSITION => {
            let (i, at) = (c.word(0) as i32, c.word(1));
            let Some(lv) = get(cpu, c.hwnd) else {
                return Vec::new();
            };
            if i >= lv.count() {
                c.result = 0;
                return Vec::new();
            }
            let p = item_position(cpu, c.hwnd, &lv, i);
            write_words(cpu, at, &[p.0 as u32, p.1 as u32]);
            Vec::new()
        }
        _ => Vec::new(),
    }
}
