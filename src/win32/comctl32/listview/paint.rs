//! Painting: the background, then each visible item and subitem with
//! NM_CUSTOMDRAW around it (LISTVIEW_Refresh and LISTVIEW_DrawItem), or
//! WM_DRAWITEM for an owner drawn report. Each notification is a step
//! of the script, and what painting keeps between them is in
//! `Painting`.

use super::items;
use super::layout::{self, Shape};
use super::*;
use crate::win32::gdi32;

const T_REFRESH: u16 = T_PAINT;
const T_REFRESH_PRE: u16 = T_PAINT + 1;
const T_DRAW_NEXT: u16 = T_PAINT + 2;
const T_DRAW_ITEM: u16 = T_PAINT + 3;
const T_DRAW_ITEM2: u16 = T_PAINT + 4;
const T_DRAW_ITEM3: u16 = T_PAINT + 5;
const T_OWNER_DRAW: u16 = T_PAINT + 6;
const T_OWNER_DRAW2: u16 = T_PAINT + 7;
const T_REFRESH_END: u16 = T_PAINT + 8;
const T_REFRESH_DONE: u16 = T_PAINT + 9;
const T_ERASED: u16 = T_PAINT + 10;

const CDDS_PREPAINT: u32 = 0x0001;
const CDDS_POSTPAINT: u32 = 0x0002;
const CDDS_ITEM: u32 = 0x0001_0000;
const CDDS_SUBITEM: u32 = 0x0002_0000;
const CDRF_SKIPDEFAULT: u32 = 0x0004;
const CDRF_NOTIFYPOSTPAINT: u32 = 0x0010;
const CDRF_NOTIFYITEMDRAW: u32 = 0x0020;
const CDIS_SELECTED: u32 = 0x0001;
const CDIS_FOCUS: u32 = 0x0010;
const CDIS_HOT: u32 = 0x0040;

const ODT_LISTVIEW: u32 = 102;
const ODA_DRAWENTIRE: u32 = 0x0001;
const ODS_SELECTED: u32 = 0x0001;
const ODS_FOCUS: u32 = 0x0010;

const OPAQUE: u32 = 2;
const TRANSPARENT: u32 = 1;
const IMAGE_PADDING: i32 = 2;
const LABEL_HOR_PADDING: i32 = 5;
const DT_RIGHT: u32 = 0x0002;
const PRF_CHECKVISIBLE: u32 = 0x0001;
const PRF_CLIENT: u32 = 0x0004;
const PRF_ERASEBKGND: u32 = 0x0008;
const PS_SOLID: u32 = 0;

/// What painting keeps between its steps.
#[derive(Clone, Debug, Default)]
pub struct Painting {
    pub hdc: u32,
    pub cdmode: u32,
    pub cditemmode: u32,
    /// Item and subitem pairs left to draw.
    pub queue: Vec<(i32, i32)>,
    /// The first and the last item drawn.
    pub range: (i32, i32),
    pub old_font: u32,
    pub old_bk_mode: u32,
    pub old_bk: u32,
    pub old_text: u32,
    pub saved_text_bk: u32,
    pub saved_text: u32,
    /// The item being drawn: its subitem, state, image and text, its
    /// rectangles, and the custom draw answer for it.
    pub item: i32,
    pub sub: i32,
    pub state: u32,
    pub image: i32,
    pub lparam: u32,
    pub text: Vec<u8>,
    pub rc_box: Rect,
    pub rc_select: Rect,
    pub rc_icon: Rect,
    pub rc_state: Rect,
    pub rc_label: Rect,
    pub cdsub: u32,
    pub item_bk: u32,
}

crate::state_fields!(Painting {
    hdc,
    cdmode,
    cditemmode,
    queue,
    range,
    old_font,
    old_bk_mode,
    old_bk,
    old_text,
    saved_text_bk,
    saved_text,
    item,
    sub,
    state,
    image,
    lparam,
    text,
    rc_box,
    rc_select,
    rc_icon,
    rc_state,
    rc_label,
    cdsub,
    item_bk
});

/// WM_PAINT (LISTVIEW_WMPaint and LISTVIEW_Paint).
pub(super) fn paint(
    cpu: &mut Cpu,
    hwnd: u32,
    lv: &ListView,
    hdc: u32,
    args: &[u32],
) -> Option<u32> {
    if !lv.redraw {
        return defwnd::def_window_proc(cpu, args);
    }
    let ops = paint_ops(cpu, hwnd, lv, hdc);
    run_ops(cpu, hwnd, ops, 0)
}

fn paint_ops(cpu: &mut Cpu, hwnd: u32, lv: &ListView, hdc: u32) -> Vec<Op> {
    let mut ops = Vec::new();
    let mut lv = lv.clone();
    if lv.no_item_metrics && lv.count() > 0 {
        lv.no_item_metrics = false;
        layout::update_item_size(cpu, hwnd, &mut lv);
        if lv.is_icon_view() {
            layout::arrange(cpu, hwnd, &mut lv, 0);
        }
        put(cpu, hwnd, lv.clone());
        ops.extend(layout::update_scroll_ops(&lv));
    }
    if lv.header != 0 {
        ops.push(user_op("UpdateWindow", &[Arg::V(lv.header)]));
    }
    if hdc != 0 {
        ops.push(step(T_REFRESH, &[hdc, 0]));
    } else {
        ops.push(user_op("BeginPaint", &[Arg::V(hwnd), Arg::S(S_PS)]));
        ops.push(step(T_REFRESH, &[0, 1]));
        ops.push(user_op("EndPaint", &[Arg::V(hwnd), Arg::S(S_PS)]));
    }
    ops
}

/// WM_PRINTCLIENT.
pub(super) fn print_client(
    cpu: &mut Cpu,
    hwnd: u32,
    lv: &ListView,
    hdc: u32,
    options: u32,
) -> Option<u32> {
    let visible = super::super::style(cpu, hwnd) & user32::window::WS_VISIBLE != 0;
    if options & PRF_CHECKVISIBLE != 0 && !visible {
        return Some(0);
    }
    let mut ops = Vec::new();
    if options & PRF_ERASEBKGND != 0 {
        ops.extend(erase_ops(cpu, hwnd, lv, hdc).0);
    }
    if options & PRF_CLIENT != 0 {
        ops.extend(paint_ops(cpu, hwnd, lv, hdc));
    }
    run_ops(cpu, hwnd, ops, 0)
}

/// WM_ERASEBKGND (LISTVIEW_EraseBkgnd): without a background colour the
/// parent erases.
pub(super) fn erase_bkgnd(cpu: &mut Cpu, hwnd: u32, lv: &ListView, hdc: u32) -> Option<u32> {
    let (ops, result) = erase_ops(cpu, hwnd, lv, hdc);
    run_ops(cpu, hwnd, ops, result)
}

fn erase_ops(cpu: &mut Cpu, hwnd: u32, lv: &ListView, hdc: u32) -> (Vec<Op>, u32) {
    if lv.clr_bk == gfx::CLR_NONE {
        let msg = user32::message::WM_ERASEBKGND;
        let ops = vec![
            send_op(lv.notify, msg, Arg::V(hdc), Arg::V(0)),
            step(T_ERASED, &[]),
        ];
        return (ops, 0);
    }
    let r = super::super::client(cpu, hwnd);
    gfx::fill_rgb(cpu, hdc, r, lv.clr_bk);
    (Vec::new(), 1)
}

/// The NMLVCUSTOMDRAW at `S_NM` (customdraw_fill): `item` is None for
/// the control's.
fn custom_draw_words(
    cpu: &mut Cpu,
    hwnd: u32,
    lv: &ListView,
    stage: u32,
    rc: Rect,
    item: Option<(i32, i32, u32, u32)>,
) -> Vec<u32> {
    let id = id_of(cpu, hwnd);
    let hdc = lv.paint.hdc;
    let (mut spec, mut istate, mut lparam, mut sub) = (0, 0, 0, 0);
    let mut stage = stage;
    if let Some((i, s, state, lp)) = item {
        spec = i as u32;
        sub = s;
        lparam = lp;
        if state & LVIS_SELECTED != 0 {
            istate |= CDIS_SELECTED;
        }
        if state & LVIS_FOCUSED != 0 {
            istate |= CDIS_FOCUS;
        }
        if i == lv.hot {
            istate |= CDIS_HOT;
        }
        stage |= CDDS_ITEM;
        if s != 0 {
            stage |= CDDS_SUBITEM;
        }
    }
    vec![
        hwnd,
        id,
        NM_CUSTOMDRAW,
        stage,
        hdc,
        rc.left as u32,
        rc.top as u32,
        rc.right as u32,
        rc.bottom as u32,
        spec,
        istate,
        lparam,
        lv.clr_text,
        lv.clr_text_bk,
        sub as u32,
    ]
}

/// The steps of NM_CUSTOMDRAW with `words`.
fn custom_draw_ops(lv: &ListView, words: Vec<u32>) -> Vec<Op> {
    let id = words[1];
    vec![
        Op::Write { at: S_NM, words },
        send_op(
            lv.notify,
            user32::message::WM_NOTIFY,
            Arg::V(id),
            Arg::S(S_NM),
        ),
    ]
}

/// The colours the custom draw structure at `S_NM` has now.
fn custom_draw_colors(cpu: &mut Cpu, scratch: u32) -> (u32, u32) {
    let w = read_words(cpu, scratch + S_NM + 48, 2);
    (w[0], w[1])
}

/// prepaint_setup: the colours of the item's text, set in the DC
/// unless `colors_only`.
fn prepaint_setup(
    cpu: &mut Cpu,
    lv: &ListView,
    hdc: u32,
    text: u32,
    text_bk: u32,
    subitem: bool,
    colors_only: bool,
) -> (u32, u32) {
    let mut text = text;
    let mut bk = text_bk;
    if bk == gfx::CLR_DEFAULT {
        bk = gfx::sys_color(cpu, gfx::COLOR_WINDOW);
    }
    if text == gfx::CLR_DEFAULT {
        text = gfx::sys_color(cpu, gfx::COLOR_WINDOWTEXT);
    }
    if !subitem && lv.paint.state & LVIS_SELECTED != 0 && lv.paint.item >= 0 {
        if lv.focus {
            bk = gfx::sys_color(cpu, gfx::COLOR_HIGHLIGHT);
            text = gfx::sys_color(cpu, gfx::COLOR_HIGHLIGHTTEXT);
        } else if lv.style & LVS_SHOWSELALWAYS != 0 {
            bk = gfx::sys_color(cpu, gfx::COLOR_BTNFACE);
            text = gfx::sys_color(cpu, gfx::COLOR_BTNTEXT);
        }
    }
    if colors_only {
        return (text, bk);
    }
    if bk != gfx::CLR_NONE {
        gfx::set_bk_mode(cpu, hdc, OPAQUE);
        gfx::set_bk_color(cpu, hdc, bk);
    } else {
        gfx::set_bk_mode(cpu, hdc, TRANSPARENT);
    }
    gfx::set_text_color(cpu, hdc, text);
    (text, bk)
}

/// The start of LISTVIEW_Refresh.
fn refresh(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let Some(mut lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    let from_ps = c.word(1) != 0;
    let hdc = if from_ps { c.last } else { c.word(0) };
    if hdc == 0 {
        if from_ps {
            c.result = 1;
        }
        return Vec::new();
    }
    let font = lv.hfont(cpu);
    lv.paint = Painting {
        hdc,
        old_font: gfx::select(cpu, hdc, font),
        old_bk_mode: gdi32::dc::get_bk_mode(cpu, &[hdc]).unwrap_or(0),
        old_bk: gdi32::dc::get_bk_color(cpu, &[hdc]).unwrap_or(0),
        old_text: gdi32::dc::get_text_color(cpu, &[hdc]).unwrap_or(0),
        saved_text_bk: lv.clr_text_bk,
        saved_text: lv.clr_text,
        item: -1,
        ..Painting::default()
    };
    lv.drawing = true;
    if from_ps {
        let w = read_words(cpu, c.scratch + S_PS, 6);
        if w[1] != 0 && lv.clr_bk != gfx::CLR_NONE {
            let r = Rect::new(w[2] as i32, w[3] as i32, w[4] as i32, w[5] as i32);
            gfx::fill_rgb(cpu, hdc, r, lv.clr_bk);
        }
    }
    put(cpu, c.hwnd, lv.clone());
    let rc = super::super::client(cpu, c.hwnd);
    let words = custom_draw_words(cpu, c.hwnd, &lv, CDDS_PREPAINT, rc, None);
    let mut ops = custom_draw_ops(&lv, words);
    ops.push(step(T_REFRESH_PRE, &[]));
    ops
}

/// After the control's CDDS_PREPAINT: what to draw.
fn refresh_pre(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let Some(mut lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    lv.paint.cdmode = c.last;
    if c.last & CDRF_SKIPDEFAULT != 0 {
        put(cpu, c.hwnd, lv);
        return vec![step(T_REFRESH_END, &[0])];
    }
    // The parent's colours, made real; the DC keeps its background
    // colour, as COMCTL32 4.7x leaves it to the items.
    let (text, bk) = custom_draw_colors(cpu, c.scratch);
    let hdc = lv.paint.hdc;
    let (text, bk) = prepaint_setup(cpu, &lv, hdc, text, bk, false, true);
    lv.clr_text = text;
    lv.clr_text_bk = bk;
    if lv.count() == 0 {
        put(cpu, c.hwnd, lv);
        return vec![step(T_REFRESH_END, &[0])];
    }
    let at = c.scratch + S_RECT;
    let kind = gdi32::dc::get_clip_box(cpu, &[hdc, at]).unwrap_or(0);
    if kind == 1 {
        // NULLREGION.
        put(cpu, c.hwnd, lv);
        return vec![step(T_REFRESH_END, &[0])];
    }
    let clip = Rect::read(cpu, at);
    let mut visible = layout::items_in(cpu, c.hwnd, &lv, clip);
    visible.sort();
    visible.dedup();
    let range = match (visible.first(), visible.last()) {
        (Some(&a), Some(&b)) => (a, b),
        _ => (0, -1),
    };
    lv.paint.range = range;
    let mut ops = Vec::new();
    if lv.ownerdata() && range.1 >= range.0 {
        ops.extend(notify_ops(
            cpu,
            c.hwnd,
            &lv,
            LVN_ODCACHEHINT,
            &[range.0 as u32, range.1 as u32],
        ));
    }
    let owner_draw = lv.style & LVS_OWNERDRAWFIXED != 0 && lv.view == LV_VIEW_DETAILS;
    let mut queue = Vec::new();
    if owner_draw {
        queue = visible.iter().map(|&i| (i, -1)).collect();
    } else if lv.view == LV_VIEW_DETAILS {
        let o = layout::origin(cpu, c.hwnd, &lv);
        let mut cols = Vec::new();
        for col in 0..lv.columns.len() as i32 {
            let r = layout::column_rect(cpu, &lv, col);
            if r.right + o.0 >= clip.left && r.left + o.0 < clip.right {
                cols.push(col);
            }
        }
        if lv.ex_style & LVS_EX_FULLROWSELECT != 0 && !lv.columns.is_empty() && !cols.contains(&0) {
            cols.insert(0, 0);
        }
        for &i in &visible {
            for &col in &cols {
                queue.push((i, col));
            }
        }
    } else {
        queue = visible.iter().rev().map(|&i| (i, 0)).collect();
    }
    lv.paint.queue = queue;
    put(cpu, c.hwnd, lv);
    ops.push(step(T_DRAW_NEXT, &[]));
    ops
}

/// The next item to draw.
fn draw_next(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let Some(mut lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    if lv.paint.queue.is_empty() {
        return vec![step(T_REFRESH_END, &[1])];
    }
    let (item, sub) = lv.paint.queue.remove(0);
    put(cpu, c.hwnd, lv.clone());
    if item < 0 || item >= lv.count() {
        return vec![step(T_DRAW_NEXT, &[])];
    }
    if sub < 0 {
        return owner_draw(cpu, c, &lv, item);
    }
    let mut mask = LVIF_TEXT | LVIF_IMAGE | LVIF_PARAM;
    if sub == 0 {
        mask |= LVIF_STATE;
    }
    if lv.view == LV_VIEW_DETAILS {
        mask |= LVIF_INDENT;
    }
    let state_mask = LVIS_SELECTED | LVIS_FOCUSED | LVIS_STATEIMAGEMASK | LVIS_CUT;
    let mut ops = items::read_own_ops(cpu, c, item, sub, mask, state_mask);
    ops.push(step(T_DRAW_ITEM, &[item as u32, sub as u32]));
    ops
}

/// LISTVIEW_DrawItem up to the item's CDDS_ITEMPREPAINT.
fn draw_item(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let (item, sub) = (c.word(0) as i32, c.word(1) as i32);
    let Some(mut lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    if item >= lv.count() {
        return vec![step(T_DRAW_NEXT, &[])];
    }
    let got = items::read_own(cpu, c.scratch);
    let mut state = if sub == 0 { got.state } else { 0 };
    if sub > 0 && lv.ex_style & LVS_EX_FULLROWSELECT != 0 {
        state = items::item_state(&lv, item) & LVIS_SELECTED;
    }
    let focus_rect = lv.focus && state & LVIS_FOCUSED != 0;
    if !focus_rect {
        state &= !LVIS_FOCUSED;
    }
    let text = got.text.unwrap_or_default();
    let shape = Shape {
        sub,
        focused: lv.view == LV_VIEW_ICON && focus_rect,
        indent: if lv.view == LV_VIEW_DETAILS && sub == 0 {
            got.indent.max(0)
        } else {
            0
        },
        image: got.image,
        text: text.clone(),
    };
    let m = layout::metrics(cpu, c.hwnd, &lv, &shape);
    let o = layout::origin(cpu, c.hwnd, &lv);
    let p = layout::item_origin(cpu, &lv, item);
    let pos = if lv.view == LV_VIEW_DETAILS {
        let x = if sub == 0 {
            layout::column_rect(cpu, &lv, 0).left + o.0
        } else {
            o.0
        };
        (x, p.1 + o.1)
    } else {
        (p.0 + o.0, p.1 + o.1)
    };
    let off = |r: Rect| r.offset(pos.0, pos.1);
    let pt = &mut lv.paint;
    pt.item = item;
    pt.sub = sub;
    pt.state = state;
    pt.image = got.image;
    pt.lparam = got.lparam;
    pt.text = text;
    pt.rc_box = off(m.bounds);
    pt.rc_select = off(m.select);
    pt.rc_icon = off(m.icon);
    pt.rc_state = off(m.state);
    pt.rc_label = off(m.label);
    pt.cdsub = 0;
    put(cpu, c.hwnd, lv.clone());
    let cdmode = if sub > 0 {
        lv.paint.cditemmode
    } else {
        lv.paint.cdmode
    };
    let rc = lv.paint.rc_box;
    let words = custom_draw_words(
        cpu,
        c.hwnd,
        &lv,
        CDDS_PREPAINT,
        rc,
        Some((item, sub, state, got.lparam)),
    );
    if cdmode & CDRF_SKIPDEFAULT != 0 {
        // Straight to the postpaint, which this mode doesn't ask for.
        return vec![step(T_DRAW_NEXT, &[])];
    }
    if cdmode & CDRF_NOTIFYITEMDRAW != 0 {
        let mut ops = custom_draw_ops(&lv, words);
        ops.push(step(T_DRAW_ITEM2, &[1]));
        return ops;
    }
    write_words(cpu, c.scratch + S_NM, &words);
    vec![step(T_DRAW_ITEM2, &[0])]
}

/// After the item's CDDS_ITEMPREPAINT.
fn draw_item2(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let Some(mut lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    let cdsub = if c.word(0) != 0 { c.last } else { 0 };
    lv.paint.cdsub = cdsub;
    if lv.paint.sub == 0 {
        lv.paint.cditemmode = cdsub;
    }
    put(cpu, c.hwnd, lv.clone());
    if cdsub & CDRF_SKIPDEFAULT != 0 {
        return post_item_ops(cpu, c, &lv);
    }
    if lv.paint.sub == 0 && cdsub == CDRF_NOTIFYITEMDRAW {
        let pt = &lv.paint;
        let item = Some((pt.item, 0, pt.state, pt.lparam));
        let rc = pt.rc_box;
        let mut words = custom_draw_words(cpu, c.hwnd, &lv, CDDS_PREPAINT, rc, item);
        words[3] |= CDDS_SUBITEM;
        let mut ops = custom_draw_ops(&lv, words);
        ops.push(step(T_DRAW_ITEM3, &[1]));
        return ops;
    }
    vec![step(T_DRAW_ITEM3, &[0])]
}

/// The item drawn, then its CDDS_ITEMPOSTPAINT.
fn draw_item3(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let Some(mut lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    if c.word(0) != 0 {
        lv.paint.cdsub = c.last;
        put(cpu, c.hwnd, lv.clone());
        if c.last & CDRF_SKIPDEFAULT != 0 {
            return post_item_ops(cpu, c, &lv);
        }
    }
    let hdc = lv.paint.hdc;
    let sub = lv.paint.sub;
    let item = lv.paint.item;
    let cdmode = if sub > 0 {
        lv.paint.cditemmode
    } else {
        lv.paint.cdmode
    };
    // The DC's background colour comes back after the item, for the
    // parent's postpaint.
    lv.paint.item_bk = gdi32::dc::get_bk_color(cpu, &[hdc]).unwrap_or(0);
    let (text, bk) = custom_draw_colors(cpu, c.scratch);
    let full_row = lv.ex_style & LVS_EX_FULLROWSELECT != 0;
    let mut text_bk = bk;
    if sub == 0 || cdmode & CDRF_NOTIFYITEMDRAW != 0 {
        text_bk = prepaint_setup(cpu, &lv, hdc, text, bk, false, false).1;
    } else if !full_row {
        text_bk = prepaint_setup(cpu, &lv, hdc, text, bk, true, false).1;
    }
    if sub != 0 && lv.view == LV_VIEW_DETAILS && full_row {
        text_bk = gfx::CLR_NONE;
    }
    let pt = lv.paint.clone();
    let mut rc_select = pt.rc_select;
    rc_select.left = pt.rc_label.left;
    if sub == 0 {
        if lv.view == LV_VIEW_ICON {
            rc_select = pt.rc_label;
        }
        if lv.view == LV_VIEW_DETAILS && full_row {
            let mut rc_box = pt.rc_box;
            let order = header::order_of(cpu, lv.header);
            if let (Some(&leftmost), Some(&last)) = (order.first(), order.last())
                && leftmost != 0
            {
                let ox = pt.rc_box.left - layout::column_rect(cpu, &lv, 0).left;
                rc_box.right = layout::column_rect(cpu, &lv, last).right + ox;
                rc_select.left = layout::column_rect(cpu, &lv, leftmost).left + ox;
            }
            rc_select.right = rc_box.right;
        }
        if text_bk != gfx::CLR_NONE {
            gfx::fill_rgb(cpu, hdc, rc_select, text_bk);
        }
        if lv.focused == item {
            lv.rc_focus = rc_select;
        }
    }
    // The state image.
    let state_image = (pt.state >> 12) & 0xF;
    if lv.himl_state != 0 && state_image != 0 && sub == 0 {
        super::super::imagelist::draw(
            cpu,
            lv.himl_state,
            state_image as i32 - 1,
            hdc,
            pt.rc_state.left,
            pt.rc_state.top,
            0,
        );
    }
    // The icon.
    let himl = if lv.view == LV_VIEW_ICON {
        lv.himl_normal
    } else {
        lv.himl_small
    };
    let has_icon = himl != 0 && pt.image >= 0 && !pt.rc_icon.is_empty();
    if has_icon {
        let style = if pt.state & (LVIS_SELECTED | LVIS_CUT) != 0 && lv.focus {
            super::super::imagelist::ILD_SELECTED
        } else {
            super::super::imagelist::ILD_NORMAL
        };
        let fg = if pt.state & LVIS_CUT != 0 {
            0x00FF_FFFF
        } else {
            gfx::CLR_DEFAULT
        };
        let r = pt.rc_icon;
        super::super::imagelist::draw_ex(
            cpu,
            himl,
            pt.image,
            hdc,
            r.left,
            r.top,
            r.width(),
            r.height(),
            lv.clr_bk,
            fg,
            style,
        );
    }
    // The label, unless it is being edited.
    let editing = lv.edit != 0 && item == lv.edit_item && sub == 0;
    if !editing {
        let focus_rect = lv.focus && pt.state & LVIS_FOCUSED != 0;
        let mut format = if lv.view == LV_VIEW_ICON {
            if focus_rect {
                layout::LV_FL_DT_FLAGS
            } else {
                layout::LV_ML_DT_FLAGS
            }
        } else {
            layout::LV_SL_DT_FLAGS
        };
        if lv.view != LV_VIEW_ICON && sub != 0 {
            let fmt = lv.columns.get(sub as usize).map_or(0, |c| c.fmt);
            match fmt & super::columns::LVCFMT_JUSTIFYMASK {
                1 => format |= DT_RIGHT,
                2 => format |= layout::DT_CENTER,
                _ => {}
            }
        }
        let mut label = pt.rc_label;
        if format & (DT_RIGHT | layout::DT_CENTER) == 0 {
            label.left += if has_icon {
                IMAGE_PADDING
            } else {
                LABEL_HOR_PADDING
            };
        } else if format & DT_RIGHT != 0 {
            label.right -= LABEL_HOR_PADDING;
        }
        if lv.view == LV_VIEW_DETAILS && lv.ex_style & LVS_EX_GRIDLINES != 0 {
            label.bottom -= 1;
        }
        gfx::draw_text(cpu, hdc, &pt.text, &mut label, format);
    }
    gfx::set_bk_color(cpu, hdc, lv.paint.item_bk);
    put(cpu, c.hwnd, lv.clone());
    post_item_ops(cpu, c, &lv)
}

/// The item's CDDS_ITEMPOSTPAINT if it asked for it, then the next.
fn post_item_ops(cpu: &mut Cpu, c: &mut Ctx, lv: &ListView) -> Vec<Op> {
    let mut ops = Vec::new();
    if lv.paint.cdsub & CDRF_NOTIFYPOSTPAINT != 0 {
        let pt = &lv.paint;
        let item = Some((pt.item, pt.sub, pt.state, pt.lparam));
        let mut words = custom_draw_words(cpu, c.hwnd, lv, CDDS_POSTPAINT, pt.rc_box, item);
        // The colours as the parent left them.
        let (text, bk) = custom_draw_colors(cpu, c.scratch);
        words[12] = text;
        words[13] = bk;
        ops.extend(custom_draw_ops(lv, words));
    }
    ops.push(step(T_DRAW_NEXT, &[]));
    ops
}

/// LISTVIEW_RefreshOwnerDraw for item `item`.
fn owner_draw(cpu: &mut Cpu, c: &mut Ctx, lv: &ListView, item: i32) -> Vec<Op> {
    let state = items::item_state(lv, item) & (LVIS_SELECTED | LVIS_FOCUSED);
    let lparam = lv.items.get(item as usize).map_or(0, |it| it.lparam);
    let o = layout::origin(cpu, c.hwnd, lv);
    let p = layout::item_origin(cpu, lv, item);
    let left = p.0 + o.0;
    let top = p.1 + o.1;
    let rc = Rect::new(left, top, left + lv.item_width, top + lv.item_height);
    let mut ods = 0;
    if state & LVIS_SELECTED != 0 {
        ods |= ODS_SELECTED;
    }
    if lv.focus && state & LVIS_FOCUSED != 0 {
        ods |= ODS_FOCUS;
    }
    let id = id_of(cpu, c.hwnd);
    let dis = [
        ODT_LISTVIEW,
        id,
        item as u32,
        ODA_DRAWENTIRE,
        ods,
        c.hwnd,
        lv.paint.hdc,
        rc.left as u32,
        rc.top as u32,
        rc.right as u32,
        rc.bottom as u32,
        lparam,
    ];
    write_words(cpu, c.scratch + S_AUX, &dis);
    let mut lv = lv.clone();
    lv.paint.item = item;
    lv.paint.state = state;
    lv.paint.lparam = lparam;
    lv.paint.rc_box = rc;
    put(cpu, c.hwnd, lv.clone());
    let words = custom_draw_words(
        cpu,
        c.hwnd,
        &lv,
        CDDS_PREPAINT,
        rc,
        Some((item, 0, state, lparam)),
    );
    if lv.paint.cdmode & CDRF_NOTIFYITEMDRAW != 0 {
        let mut ops = custom_draw_ops(&lv, words);
        ops.push(step(T_OWNER_DRAW, &[1]));
        return ops;
    }
    write_words(cpu, c.scratch + S_NM, &words);
    vec![step(T_OWNER_DRAW, &[0])]
}

fn owner_draw2(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let Some(mut lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    let mode = if c.word(0) != 0 { c.last } else { 0 };
    lv.paint.cdsub = mode;
    put(cpu, c.hwnd, lv.clone());
    let mut ops = Vec::new();
    if mode & CDRF_SKIPDEFAULT == 0 {
        let (text, bk) = custom_draw_colors(cpu, c.scratch);
        let hdc = lv.paint.hdc;
        prepaint_setup(cpu, &lv, hdc, text, bk, false, false);
        let id = id_of(cpu, c.hwnd);
        let msg = user32::message::WM_DRAWITEM;
        ops.push(send_op(lv.notify, msg, Arg::V(id), Arg::S(S_AUX)));
    }
    ops.push(step(T_OWNER_DRAW2, &[]));
    ops
}

/// The end of LISTVIEW_Refresh: the focus rectangle, the grid lines
/// and the control's postpaint.
fn refresh_end(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let drew = c.word(0) != 0;
    let Some(lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    let hdc = lv.paint.hdc;
    let owner_draw = lv.style & LVS_OWNERDRAWFIXED != 0 && lv.view == LV_VIEW_DETAILS;
    let (lo, hi) = lv.paint.range;
    if drew
        && !owner_draw
        && lv.focus
        && lo <= lv.focused
        && hi >= lv.focused
        && lv.rc_focus.left < lv.rc_focus.right
    {
        lv.rc_focus.write(cpu, c.scratch + S_RECT);
        user32::paint::draw_focus_rect(cpu, &[hdc, c.scratch + S_RECT]);
    }
    if lv.view == LV_VIEW_DETAILS && lv.ex_style & LVS_EX_GRIDLINES != 0 {
        draw_grid(cpu, c.hwnd, &lv, hdc);
    }
    let mut ops = Vec::new();
    if lv.paint.cdmode & CDRF_NOTIFYPOSTPAINT != 0 {
        let rc = super::super::client(cpu, c.hwnd);
        let words = custom_draw_words(cpu, c.hwnd, &lv, CDDS_POSTPAINT, rc, None);
        ops.extend(custom_draw_ops(&lv, words));
    }
    ops.push(step(T_REFRESH_DONE, &[]));
    ops
}

/// LISTVIEW_RefreshReportGrid.
fn draw_grid(cpu: &mut Cpu, hwnd: u32, lv: &ListView, hdc: u32) {
    let o = layout::origin(cpu, hwnd, lv);
    let face = gfx::sys_color(cpu, gfx::COLOR_BTNFACE);
    let pen = gdi32::create_pen(cpu, &[PS_SOLID, 1, face]).unwrap_or(0);
    if pen == 0 {
        return;
    }
    let old = gfx::select(cpu, hdc, pen);
    let list = lv.list;
    for col in 0..lv.columns.len() as i32 {
        let r = layout::column_rect(cpu, lv, col);
        if r.left == 0 {
            continue;
        }
        gfx::line(
            cpu,
            hdc,
            (r.left + o.0, list.top),
            (r.left + o.0, list.bottom),
        );
    }
    let order = header::order_of(cpu, lv.header);
    if let Some(&last) = order.last() {
        let r = layout::column_rect(cpu, lv, last);
        gfx::line(
            cpu,
            hdc,
            (r.right + o.0, list.top),
            (r.right + o.0, list.bottom),
        );
    }
    let h = layout::calc_item_height(lv);
    gfx::line(cpu, hdc, (list.left, o.1 - 1), (list.right, o.1 - 1));
    let mut y = h - 1 + o.1;
    while y <= list.bottom {
        gfx::line(cpu, hdc, (list.left, y), (list.right, y));
        y += h.max(1);
    }
    gfx::select(cpu, hdc, old);
    gdi32::delete_object(cpu, &[pen]);
}

/// The continuations.
pub(super) fn then(cpu: &mut Cpu, id: u16, c: &mut Ctx) -> Vec<Op> {
    match id {
        T_REFRESH => refresh(cpu, c),
        T_REFRESH_PRE => refresh_pre(cpu, c),
        T_DRAW_NEXT => draw_next(cpu, c),
        T_DRAW_ITEM => draw_item(cpu, c),
        T_DRAW_ITEM2 => draw_item2(cpu, c),
        T_DRAW_ITEM3 => draw_item3(cpu, c),
        T_OWNER_DRAW => owner_draw2(cpu, c),
        T_OWNER_DRAW2 => {
            let Some(lv) = get(cpu, c.hwnd) else {
                return Vec::new();
            };
            let mut ops = Vec::new();
            if lv.paint.cdsub & CDRF_NOTIFYPOSTPAINT != 0 {
                let pt = &lv.paint;
                let item = Some((pt.item, 0, pt.state, pt.lparam));
                let words = custom_draw_words(cpu, c.hwnd, &lv, CDDS_POSTPAINT, pt.rc_box, item);
                ops.extend(custom_draw_ops(&lv, words));
            }
            ops.push(step(T_DRAW_NEXT, &[]));
            ops
        }
        T_REFRESH_END => refresh_end(cpu, c),
        T_REFRESH_DONE => {
            let Some(mut lv) = get(cpu, c.hwnd) else {
                return Vec::new();
            };
            let pt = lv.paint.clone();
            lv.clr_text_bk = pt.saved_text_bk;
            lv.clr_text = pt.saved_text;
            gfx::select(cpu, pt.hdc, pt.old_font);
            gfx::set_bk_mode(cpu, pt.hdc, pt.old_bk_mode);
            gfx::set_bk_color(cpu, pt.hdc, pt.old_bk);
            gfx::set_text_color(cpu, pt.hdc, pt.old_text);
            lv.drawing = false;
            lv.paint = Painting::default();
            put(cpu, c.hwnd, lv);
            Vec::new()
        }
        T_ERASED => {
            c.result = c.last;
            Vec::new()
        }
        _ => Vec::new(),
    }
}
