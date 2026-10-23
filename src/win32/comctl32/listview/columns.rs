//! The columns of the report view and the header control that shows
//! them: inserting, changing and deleting columns through the header's
//! messages, and what the header tells the list.

use super::layout;
use super::*;

const T_CREATE_HEADER: u16 = T_COLUMNS;
const T_HEADER_MADE: u16 = T_COLUMNS + 1;
pub(super) const T_SHOW_HEADER: u16 = T_COLUMNS + 2;
pub(super) const T_REPORT_HEADER: u16 = T_COLUMNS + 3;
const T_INSERT: u16 = T_COLUMNS + 4;
const T_INSERTED: u16 = T_COLUMNS + 5;
const T_SET: u16 = T_COLUMNS + 6;
const T_SET_DONE: u16 = T_COLUMNS + 7;
const T_DELETED: u16 = T_COLUMNS + 8;
const T_FORWARD: u16 = T_COLUMNS + 9;
const T_HEADER_CHANGED: u16 = T_COLUMNS + 10;
pub(super) const T_ORDER_CHANGED: u16 = T_COLUMNS + 11;
const T_WIDTH_SET: u16 = T_COLUMNS + 12;

const LVCF_FMT: u32 = 0x0001;
const LVCF_WIDTH: u32 = 0x0002;
const LVCF_TEXT: u32 = 0x0004;
const LVCF_SUBITEM: u32 = 0x0008;
const LVCF_IMAGE: u32 = 0x0010;
const LVCF_ORDER: u32 = 0x0020;

const LVCFMT_LEFT: u32 = 0x0000;
const LVCFMT_RIGHT: u32 = 0x0001;
const LVCFMT_CENTER: u32 = 0x0002;
pub(super) const LVCFMT_JUSTIFYMASK: u32 = 0x0003;
pub(super) const LVCFMT_IMAGE: u32 = 0x0800;
const LVCFMT_BITMAP_ON_RIGHT: u32 = 0x1000;
const LVCFMT_COL_HAS_IMAGES: u32 = 0x8000;

const LVSCW_AUTOSIZE: i32 = -1;
const LVSCW_AUTOSIZE_USEHEADER: i32 = -2;
const TRAILING_HEADER_PADDING: i32 = 11;

const HDS_HORZ: u32 = 0;
const WS_CHILD: u32 = user32::window::WS_CHILD;

/// An LVCOLUMN.
#[derive(Clone, Debug, Default)]
struct LvColumn {
    mask: u32,
    fmt: u32,
    cx: i32,
    text: u32,
    max: u32,
    sub: i32,
    image: i32,
    order: i32,
}

impl LvColumn {
    fn read(cpu: &mut Cpu, at: u32) -> Option<LvColumn> {
        let w = read_words(cpu, at, 8);
        if cpu.bus.guest_faulted() {
            return None;
        }
        Some(LvColumn {
            mask: w[0],
            fmt: w[1],
            cx: w[2] as i32,
            text: w[3],
            max: w[4],
            sub: w[5] as i32,
            image: w[6] as i32,
            order: w[7] as i32,
        })
    }
}

/// The steps that make the header if there is none
/// (LISTVIEW_CreateHeader).
pub(super) fn create_header_ops(lv: &ListView) -> Vec<Op> {
    if lv.header != 0 {
        return Vec::new();
    }
    vec![step(T_CREATE_HEADER, &[])]
}

const WS_EX_NOPARENTNOTIFY: u32 = 0x0000_0004;

fn create_header(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let Some(lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    if lv.header != 0 {
        return Vec::new();
    }
    let mut style = WS_CHILD | HDS_HORZ | header::HDS_FULLDRAG | header::HDS_DRAGDROP;
    if lv.style & LVS_NOSORTHEADER == 0 {
        style |= header::HDS_BUTTONS;
    }
    if lv.style & LVS_NOCOLUMNHEADER != 0 {
        style |= header::HDS_HIDDEN;
    }
    let instance = user32::user(cpu).window(c.hwnd).map_or(0, |w| w.instance);
    cpu.bus
        .guest_write_bytes(c.scratch + S_AUX, b"SysHeader32\0");
    // The header's WM_PARENTNOTIFY stops at the list view: the list
    // view's parent doesn't see it under COMCTL32 4.7x.
    let args = [
        Arg::V(WS_EX_NOPARENTNOTIFY),
        Arg::S(S_AUX),
        Arg::V(0),
        Arg::V(style),
        Arg::V(0),
        Arg::V(0),
        Arg::V(0),
        Arg::V(0),
        Arg::V(c.hwnd),
        Arg::V(0),
        Arg::V(instance),
        Arg::V(0),
    ];
    vec![user_op("CreateWindowExA", &args), step(T_HEADER_MADE, &[])]
}

fn header_made(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let hdr = c.last;
    let Some(mut lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    if hdr == 0 || lv.header != 0 {
        return Vec::new();
    }
    lv.header = hdr;
    let font = lv.hfont(cpu);
    put(cpu, c.hwnd, lv);
    vec![
        send_op(hdr, header::HDM_SETUNICODEFORMAT, Arg::V(1), Arg::V(0)),
        send_op(hdr, user32::message::WM_SETFONT, Arg::V(font), Arg::V(1)),
        step(layout::T_UPDATE_SIZE, &[]),
    ]
}

/// The header items' fields for an LVCOLUMN (column_fill_hditem):
/// the HDITEM's words, with its text.
fn fill_hditem(
    cpu: &mut Cpu,
    hwnd: u32,
    lv: &ListView,
    col: i32,
    lc: &LvColumn,
    wide: bool,
    hd: &mut [u32; 9],
) -> Option<Vec<u8>> {
    if lc.mask & LVCF_FMT != 0 {
        hd[0] |= header::HDI_FORMAT;
        let just = lc.fmt & LVCFMT_JUSTIFYMASK;
        if col == 0 || just == LVCFMT_LEFT {
            hd[5] |= header::HDF_LEFT;
        } else if just == LVCFMT_RIGHT {
            hd[5] |= header::HDF_RIGHT;
        } else if just == LVCFMT_CENTER {
            hd[5] |= header::HDF_CENTER;
        }
        if lc.fmt & LVCFMT_BITMAP_ON_RIGHT != 0 {
            hd[5] |= header::HDF_BITMAP_ON_RIGHT;
        }
        if lc.fmt & LVCFMT_COL_HAS_IMAGES != 0 {
            hd[5] |= header::HDF_IMAGE;
            hd[7] = I_IMAGECALLBACK as u32;
        }
    }
    if lc.mask & LVCF_WIDTH != 0 {
        hd[0] |= header::HDI_WIDTH;
        if lc.cx == LVSCW_AUTOSIZE_USEHEADER {
            let mut used = 0;
            for j in 0..(col - 1).max(0) {
                used += layout::column_rect(cpu, lv, j).width();
            }
            hd[1] = (super::super::client(cpu, hwnd).width() - used) as u32;
        } else {
            hd[1] = lc.cx as u32;
        }
    }
    let mut text = None;
    if lc.mask & LVCF_TEXT != 0 {
        hd[0] |= header::HDI_TEXT | header::HDI_FORMAT;
        hd[5] |= header::HDF_STRING;
        if lc.text != 0 && lc.text != LPSTR_TEXTCALLBACK {
            let t = header::read_text(cpu, lc.text, wide)?;
            hd[4] = t.len() as u32;
            text = Some(t);
        } else {
            hd[2] = lc.text;
        }
    }
    if lc.mask & LVCF_IMAGE != 0 {
        hd[0] |= header::HDI_IMAGE;
        hd[7] = lc.image as u32;
    }
    if lc.mask & LVCF_ORDER != 0 {
        hd[0] |= header::HDI_ORDER;
        hd[8] = lc.order as u32;
    }
    Some(text.unwrap_or_default())
}

/// Write an HDITEMA at `S_AUX` with its text at `S_TEXT`.
fn write_hditem(cpu: &mut Cpu, scratch: u32, mut hd: [u32; 9], text: &[u8], has_text: bool) {
    if has_text {
        let n = text.len().min(TEXT_MAX as usize - 1);
        let mut b = text[..n].to_vec();
        b.push(0);
        cpu.bus.guest_write_bytes(scratch + S_TEXT, &b);
        hd[2] = scratch + S_TEXT;
    }
    write_words(cpu, scratch + S_AUX, &hd);
}

/// LVM_INSERTCOLUMNA and LVM_INSERTCOLUMNW.
pub(super) fn insert_column(
    cpu: &mut Cpu,
    hwnd: u32,
    lv: ListView,
    col: i32,
    lp: u32,
    wide: bool,
) -> Option<u32> {
    if lp == 0 || col < 0 {
        return Some(-1i32 as u32);
    }
    LvColumn::read(cpu, lp)?;
    let mut ops = create_header_ops(&lv);
    let wstyle = super::super::style(cpu, hwnd);
    if lv.style & LVS_NOCOLUMNHEADER == 0
        && lv.view == LV_VIEW_DETAILS
        && wstyle & user32::window::WS_VISIBLE != 0
    {
        ops.push(step(T_SHOW_HEADER, &[]));
    }
    ops.push(step(T_INSERT, &[col as u32, lp, wide as u32]));
    run_ops(cpu, hwnd, ops, -1i32 as u32)
}

fn insert(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let (col, lp, wide) = (c.word(0) as i32, c.word(1), c.word(2) != 0);
    let Some(lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    if lv.header == 0 {
        return Vec::new();
    }
    let Some(lc) = LvColumn::read(cpu, lp) else {
        return Vec::new();
    };
    let col = col.min(lv.columns.len() as i32);
    let mut hd = [0u32; 9];
    let Some(text) = fill_hditem(cpu, c.hwnd, &lv, col, &lc, wide, &mut hd) else {
        return Vec::new();
    };
    if lc.mask & LVCF_WIDTH == 0 {
        hd[0] |= header::HDI_WIDTH;
        hd[1] = 10;
    }
    if lc.mask & LVCF_SUBITEM != 0 {
        hd[0] |= header::HDI_LPARAM;
        hd[6] = lc.sub as u32;
    }
    let has_text = lc.mask & LVCF_TEXT != 0 && hd[2] == 0;
    write_hditem(cpu, c.scratch, hd, &text, has_text);
    vec![
        send_op(
            lv.header,
            header::HDM_INSERTITEMA,
            Arg::V(col as u32),
            Arg::S(S_AUX),
        ),
        step(T_INSERTED, &[lp]),
    ]
}

fn inserted(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let n = c.last as i32;
    let lp = c.word(0);
    let Some(mut lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    if n < 0 {
        c.result = -1i32 as u32;
        return Vec::new();
    }
    let fmt = match LvColumn::read(cpu, lp) {
        Some(lc) if lc.mask & LVCF_FMT != 0 => lc.fmt,
        _ => 0,
    };
    let at = (n as usize).min(lv.columns.len());
    lv.columns.insert(at, Column { fmt });
    if !lv.ownerdata() {
        for item in lv.items.iter_mut() {
            for s in item.subs.iter_mut() {
                if s.sub >= n {
                    s.sub += 1;
                }
            }
            // The new column's subitem is a callback for its text and
            // its image, as set_sub_item makes it.
            if n > 0 {
                let pos = item
                    .subs
                    .iter()
                    .position(|s| s.sub > n)
                    .unwrap_or(item.subs.len());
                item.subs.insert(
                    pos,
                    SubItem {
                        sub: n,
                        image: I_IMAGECALLBACK,
                        callback: true,
                        ..SubItem::default()
                    },
                );
            }
        }
    }
    layout::update_item_size(cpu, c.hwnd, &mut lv);
    put(cpu, c.hwnd, lv.clone());
    layout::invalidate_list(cpu, c.hwnd, &lv);
    c.result = n as u32;
    layout::update_scroll_ops(&lv)
}

/// LVM_SETCOLUMNA and LVM_SETCOLUMNW.
pub(super) fn set_column(
    cpu: &mut Cpu,
    hwnd: u32,
    lv: &ListView,
    col: i32,
    lp: u32,
    wide: bool,
) -> Option<u32> {
    if lp == 0 || col < 0 || col as usize >= lv.columns.len() || lv.header == 0 {
        return Some(0);
    }
    LvColumn::read(cpu, lp)?;
    run_ops(
        cpu,
        hwnd,
        vec![step(T_SET, &[col as u32, lp, wide as u32])],
        0,
    )
}

fn set(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let (col, lp, wide) = (c.word(0) as i32, c.word(1), c.word(2) != 0);
    let Some(lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    let Some(lc) = LvColumn::read(cpu, lp) else {
        return Vec::new();
    };
    let mut hd = [0u32; 9];
    if lc.mask & LVCF_FMT != 0 {
        hd[0] |= header::HDI_FORMAT;
        let old = state(cpu)
            .header
            .headers
            .get(&lv.header)
            .and_then(|h| h.items.get(col as usize))
            .map_or(0, |it| it.fmt);
        hd[5] = old & header::HDF_STRING;
    }
    let Some(text) = fill_hditem(cpu, c.hwnd, &lv, col, &lc, wide, &mut hd) else {
        return Vec::new();
    };
    let has_text = lc.mask & LVCF_TEXT != 0 && hd[2] == 0;
    write_hditem(cpu, c.scratch, hd, &text, has_text);
    vec![
        send_op(
            lv.header,
            header::HDM_SETITEMA,
            Arg::V(col as u32),
            Arg::S(S_AUX),
        ),
        step(T_SET_DONE, &[col as u32, lp]),
    ]
}

fn set_done(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let (col, lp) = (c.word(0) as i32, c.word(1));
    if c.last == 0 {
        c.result = 0;
        return Vec::new();
    }
    c.result = 1;
    let Some(mut lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    let Some(lc) = LvColumn::read(cpu, lp) else {
        return Vec::new();
    };
    if lc.mask & LVCF_FMT != 0
        && let Some(column) = lv.columns.get_mut(col as usize)
    {
        let old = column.fmt;
        column.fmt = lc.fmt;
        put(cpu, c.hwnd, lv.clone());
        let changed = (old ^ lc.fmt) & (LVCFMT_JUSTIFYMASK | LVCFMT_IMAGE) != 0;
        if changed && lv.view == LV_VIEW_DETAILS {
            layout::invalidate_column(cpu, c.hwnd, &lv, col);
        }
    }
    Vec::new()
}

/// The header's item `col`.
fn header_item(cpu: &mut Cpu, lv: &ListView, col: i32) -> Option<header::Item> {
    state(cpu)
        .header
        .headers
        .get(&lv.header)
        .and_then(|h| h.items.get(col as usize))
        .cloned()
}

/// LVM_GETCOLUMNA and LVM_GETCOLUMNW.
pub(super) fn get_column(
    cpu: &mut Cpu,
    _hwnd: u32,
    lv: &ListView,
    col: i32,
    lp: u32,
    wide: bool,
) -> Option<u32> {
    if lp == 0 || col < 0 || col as usize >= lv.columns.len() {
        return Some(0);
    }
    let lc = LvColumn::read(cpu, lp)?;
    let Some(item) = header_item(cpu, lv, col) else {
        return Some(0);
    };
    if lc.mask & LVCF_TEXT != 0 && lc.text != 0 && lc.max > 0 {
        let text = item.text.clone().unwrap_or_default();
        header::write_text(cpu, lc.text, lc.max, &text, wide);
    }
    if lc.mask & LVCF_FMT != 0 {
        cpu.bus.guest_write_32(lp + 4, lv.columns[col as usize].fmt);
    }
    if lc.mask & LVCF_WIDTH != 0 {
        let w = layout::column_rect(cpu, lv, col).width();
        cpu.bus.guest_write_32(lp + 8, w as u32);
    }
    if lc.mask & LVCF_IMAGE != 0 {
        cpu.bus.guest_write_32(lp + 24, item.image as u32);
    }
    if lc.mask & LVCF_ORDER != 0 {
        cpu.bus.guest_write_32(lp + 28, item.order as u32);
    }
    if lc.mask & LVCF_SUBITEM != 0 {
        cpu.bus.guest_write_32(lp + 20, item.lparam);
    }
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(1)
}

/// LVM_GETCOLUMNWIDTH.
pub(super) fn get_column_width(cpu: &mut Cpu, lv: &ListView, col: i32) -> u32 {
    match lv.view {
        LV_VIEW_LIST => lv.item_width as u32,
        LV_VIEW_DETAILS => {
            if col < 0 {
                return 0;
            }
            header_item(cpu, lv, col).map_or(0, |it| it.cxy as u32)
        }
        _ => 0,
    }
}

/// LVM_DELETECOLUMN.
pub(super) fn delete_column(cpu: &mut Cpu, hwnd: u32, lv: ListView, col: i32) -> Option<u32> {
    if col < 0 || col as usize >= lv.columns.len() || lv.header == 0 {
        return Some(0);
    }
    let ops = vec![
        send_op(
            lv.header,
            header::HDM_DELETEITEM,
            Arg::V(col as u32),
            Arg::V(0),
        ),
        step(T_DELETED, &[col as u32]),
    ];
    run_ops(cpu, hwnd, ops, 0)
}

fn deleted(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let col = c.word(0) as i32;
    if c.last == 0 {
        return Vec::new();
    }
    c.result = 1;
    let Some(mut lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    if (col as usize) < lv.columns.len() {
        lv.columns.remove(col as usize);
    }
    if !lv.ownerdata() && col != 0 {
        for item in lv.items.iter_mut() {
            item.subs.retain(|s| s.sub != col);
            for s in item.subs.iter_mut() {
                if s.sub > col {
                    s.sub -= 1;
                }
            }
        }
    }
    layout::update_item_size(cpu, c.hwnd, &mut lv);
    put(cpu, c.hwnd, lv.clone());
    layout::invalidate_list(cpu, c.hwnd, &lv);
    layout::update_scroll_ops(&lv)
}

/// LVM_SETCOLUMNWIDTH (LISTVIEW_SetColumnWidth).
pub(super) fn set_column_width(
    cpu: &mut Cpu,
    hwnd: u32,
    lv: ListView,
    col: i32,
    lp: u32,
) -> Option<u32> {
    match width_ops(cpu, hwnd, lv, col, lp as u16 as i16 as i32) {
        Ok(ops) => run_ops(cpu, hwnd, ops, 0),
        Err(r) => Some(r),
    }
}

/// The steps that set a column's width: or what the message returns
/// without any.
fn width_ops(
    cpu: &mut Cpu,
    hwnd: u32,
    mut lv: ListView,
    col: i32,
    cx: i32,
) -> Result<Vec<Op>, u32> {
    let mut cx = cx;
    if lv.view != LV_VIEW_DETAILS && lv.view != LV_VIEW_LIST {
        return Err(0);
    }
    if lv.view == LV_VIEW_DETAILS && cx < -2 {
        cx = LVSCW_AUTOSIZE;
    } else if lv.view == LV_VIEW_LIST && cx < 1 {
        return Err(0);
    }
    if lv.view == LV_VIEW_LIST {
        lv.item_width = cx;
        put(cpu, hwnd, lv.clone());
        layout::invalidate_list(cpu, hwnd, &lv);
        return Err(1);
    }
    let n = lv.columns.len() as i32;
    if col < 0 || col >= n || lv.header == 0 {
        return Err(0);
    }
    let mut max_cx = 0;
    if cx == LVSCW_AUTOSIZE || (cx == LVSCW_AUTOSIZE_USEHEADER && col < n - 1) {
        for i in 0..lv.items.len() as i32 {
            let text = lv.text_of(i, col);
            max_cx = max_cx.max(layout::string_width(cpu, hwnd, &lv, &text));
        }
        let fmt = lv.columns[col as usize].fmt;
        if lv.himl_small != 0 && (col == 0 || fmt & LVCFMT_IMAGE != 0) {
            max_cx += lv.icon_size.0;
        }
        max_cx += layout::TRAILING_LABEL_PADDING;
    }
    if cx == LVSCW_AUTOSIZE {
        cx = max_cx;
    } else if cx == LVSCW_AUTOSIZE_USEHEADER {
        if col == n - 1 {
            let o = layout::origin(cpu, hwnd, &lv);
            let r = layout::column_rect(cpu, &lv, col);
            cx = lv.list.right - o.0 - r.left;
        } else {
            cx = 0;
            if let Some(item) = header_item(cpu, &lv, col) {
                let text = item.text.unwrap_or_default();
                let font = state(cpu)
                    .header
                    .headers
                    .get(&lv.header)
                    .map_or(0, |h| h.font);
                let hdc = user32::paint::get_dc(cpu, &[hwnd]).unwrap_or(0);
                let font = if font != 0 {
                    font
                } else {
                    gfx::stock(cpu, header::SYSTEM_FONT)
                };
                let old = gfx::select(cpu, hdc, font);
                cx = gfx::text_size(cpu, hdc, &text).0 + TRAILING_HEADER_PADDING;
                gfx::select(cpu, hdc, old);
                user32::paint::release_dc(cpu, &[hwnd, hdc]);
            }
            cx = cx.max(max_cx);
        }
    }
    if cx < 0 {
        return Err(0);
    }
    let words = vec![header::HDI_WIDTH, cx as u32, 0, 0, 0, 0, 0, 0, 0];
    Ok(vec![
        Op::Write { at: S_AUX, words },
        send_op(
            lv.header,
            header::HDM_SETITEMA,
            Arg::V(col as u32),
            Arg::S(S_AUX),
        ),
        step(T_WIDTH_SET, &[]),
    ])
}

/// WM_NOTIFY from the header (LISTVIEW_HeaderNotification).
pub(super) fn header_notification(cpu: &mut Cpu, hwnd: u32, lv: &ListView, lp: u32) -> Option<u32> {
    let w = read_words(cpu, lp, 6);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let (code, item) = (w[2] as i32, w[3] as i32);
    if item < 0 || item as usize >= lv.columns.len() {
        return Some(0);
    }
    let forward = step(T_FORWARD, &[lp]);
    let mut ops = Vec::new();
    let mut result = 0;
    match code {
        // HDN_TRACK, HDN_ENDTRACK: the track line isn't drawn.
        -328 | -308 => {}
        -327 | -307 => {
            let mut lv = lv.clone();
            lv.track_line = -1;
            put(cpu, hwnd, lv);
        }
        // HDN_BEGINDRAG.
        -310 => {
            ops.push(forward);
            result = (lv.ex_style & LVS_EX_HEADERDRAGDROP == 0) as u32;
        }
        // HDN_ENDDRAG.
        -311 => {
            layout::invalidate_list(cpu, hwnd, lv);
            ops.push(forward);
        }
        // HDN_ITEMCHANGING.
        -320 | -300 => {
            ops.push(forward);
            ops.push(step(T_RESULT_LAST, &[]));
        }
        // HDN_ITEMCHANGED.
        -321 | -301 => {
            ops.push(forward);
            ops.push(step(T_HEADER_CHANGED, &[]));
        }
        // HDN_ITEMCLICK.
        -322 | -302 => {
            let nm = nmlistview(-1, item, 0, 0, 0, 0);
            ops.extend(notify_ops(cpu, hwnd, lv, LVN_COLUMNCLICK, &nm));
            ops.push(forward);
        }
        // HDN_DIVIDERDBLCLICK.
        -325 | -305 => {
            ops.push(step(T_HEADER_CHANGED, &[1, item as u32]));
            ops.push(forward);
        }
        _ => {}
    }
    run_ops(cpu, hwnd, ops, result)
}

/// notify_forward_header: the header's notification to the list's
/// parent, made ANSI for an ANSI parent.
fn forward(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let lp = c.word(0);
    let Some(lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    let mut w = read_words(cpu, lp, 6);
    if cpu.bus.guest_faulted() {
        return Vec::new();
    }
    let code = w[2] as i32;
    if lv.format != header::NFR_UNICODE && (-339..=-320).contains(&code) {
        w[2] = (code + 20) as u32;
    }
    if w[5] != 0 {
        let mut hd = read_words(cpu, w[5], 9);
        if !cpu.bus.guest_faulted() {
            let wide_item = (-339..=-320).contains(&code);
            if lv.format != header::NFR_UNICODE
                && wide_item
                && hd[0] & header::HDI_TEXT != 0
                && hd[2] != 0
                && hd[2] != LPSTR_TEXTCALLBACK
                && let Some(t) = header::read_text(cpu, hd[2], true)
            {
                let n = t.len().min(TEXT_MAX as usize - 1);
                let mut b = t[..n].to_vec();
                b.push(0);
                cpu.bus.guest_write_bytes(c.scratch + S_TEXT, &b);
                hd[2] = c.scratch + S_TEXT;
            }
            write_words(cpu, c.scratch + S_AUX, &hd);
            w[5] = c.scratch + S_AUX;
        }
    }
    write_words(cpu, c.scratch + S_NM, &w);
    let msg = user32::message::WM_NOTIFY;
    vec![send_op(lv.notify, msg, Arg::V(w[1]), Arg::S(S_NM))]
}

/// The continuations.
pub(super) fn then(cpu: &mut Cpu, id: u16, c: &mut Ctx) -> Vec<Op> {
    match id {
        T_CREATE_HEADER => create_header(cpu, c),
        T_HEADER_MADE => header_made(cpu, c),
        T_SHOW_HEADER => {
            let Some(lv) = get(cpu, c.hwnd) else {
                return Vec::new();
            };
            if lv.header == 0 || lv.style & LVS_NOCOLUMNHEADER != 0 {
                return Vec::new();
            }
            vec![user_op("ShowWindow", &[Arg::V(lv.header), Arg::V(1)])]
        }
        T_REPORT_HEADER => vec![step(layout::T_UPDATE_SIZE, &[])],
        T_INSERT => insert(cpu, c),
        T_INSERTED => inserted(cpu, c),
        T_SET => set(cpu, c),
        T_SET_DONE => set_done(cpu, c),
        T_DELETED => deleted(cpu, c),
        T_FORWARD => forward(cpu, c),
        T_WIDTH_SET => {
            c.result = c.last;
            Vec::new()
        }
        T_HEADER_CHANGED | T_ORDER_CHANGED => {
            let Some(mut lv) = get(cpu, c.hwnd) else {
                return Vec::new();
            };
            if id == T_HEADER_CHANGED && c.word(0) == 1 {
                // A divider's double click sizes the column to fit.
                let col = c.word(1) as i32;
                let keep = c.result;
                let mut ops = width_ops(cpu, c.hwnd, lv, col, LVSCW_AUTOSIZE).unwrap_or_default();
                // The message's result stays the header's.
                ops.retain(|op| !matches!(op, Op::Then { id, .. } if *id == T_WIDTH_SET));
                c.result = keep;
                return ops;
            }
            layout::update_item_size(cpu, c.hwnd, &mut lv);
            put(cpu, c.hwnd, lv.clone());
            layout::invalidate_list(cpu, c.hwnd, &lv);
            layout::update_scroll_ops(&lv)
        }
        _ => Vec::new(),
    }
}
