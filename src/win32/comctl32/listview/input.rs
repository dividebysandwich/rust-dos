//! The keyboard, the mouse, the focus and label editing: the control's
//! side of LISTVIEW_KeyDown, LISTVIEW_LButtonDown and the others, and
//! the edit control it puts over a label (LISTVIEW_EditLabelT,
//! LISTVIEW_EndEditLabelT and EditLblWndProcT).

use super::items::{self, Change, Text};
use super::layout;
use super::*;

const T_KEY: u16 = T_INPUT;
const T_FOCUS_SET: u16 = T_INPUT + 1;
const T_HOVERED: u16 = T_INPUT + 2;
const T_EDIT_BEGIN: u16 = T_INPUT + 3;
const T_EDIT_READ: u16 = T_INPUT + 4;
const T_EDIT_CREATE: u16 = T_INPUT + 5;
const T_EDIT_MADE: u16 = T_INPUT + 6;
const T_EDIT_BEGUN: u16 = T_INPUT + 7;
const T_FORGET_EDIT: u16 = T_INPUT + 8;
const T_END: u16 = T_INPUT + 9;
const T_END_TEXT: u16 = T_INPUT + 10;
const T_END_GOT: u16 = T_INPUT + 11;
const T_END_DONE: u16 = T_INPUT + 12;
const T_LBUTTON_DOWN: u16 = T_INPUT + 13;
const T_LBUTTON_UP: u16 = T_INPUT + 14;
const T_CONTEXT: u16 = T_INPUT + 15;
const T_CONTEXT_MENU: u16 = T_INPUT + 16;

const WM_CLOSE: u32 = 0x0010;
const WM_CONTEXTMENU: u32 = 0x007B;
const EM_SETSEL: u32 = 0x00B1;
const EN_KILLFOCUS: u32 = 0x0200;
const EN_UPDATE: u32 = 0x0400;
const DLGC_WANTARROWS: u32 = 0x0001;
const DLGC_WANTALLKEYS: u32 = 0x0004;

const VK_RETURN: u32 = 0x0D;
const VK_SHIFT: u32 = 0x10;
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

const LVNI_ABOVE: u32 = 0x0100;
const LVNI_BELOW: u32 = 0x0200;
const LVNI_TOLEFT: u32 = 0x0400;
const LVNI_TORIGHT: u32 = 0x0800;
const LVIR_LABEL: i32 = 2;

const MK_SHIFT: u32 = 0x0004;
const MK_CONTROL: u32 = 0x0008;

/// The style of the edit control over a label.
const EDIT_STYLE: u32 = 0x4000_0000 | 0x0400_0000 | 0x0080 | 0x0080_0000 | 0x1000_0000;

fn point(lp: u32) -> (i32, i32) {
    (lp as u16 as i16 as i32, (lp >> 16) as u16 as i16 as i32)
}

fn key_down_now(cpu: &mut Cpu, vk: u32) -> bool {
    let v = user32::input::get_key_state(cpu, &[vk]).unwrap_or(0);
    v & 0x8000 != 0
}

/// The steps of a notification with an NMITEMACTIVATE (notify_click
/// and notify_itemactivate).
fn click_ops(
    cpu: &mut Cpu,
    hwnd: u32,
    lv: &ListView,
    code: u32,
    hit: &layout::Hit,
    pt: (i32, i32),
    keys: u32,
) -> Vec<Op> {
    let lparam = if lv.ownerdata() {
        0
    } else {
        lv.items.get(hit.item as usize).map_or(0, |it| it.lparam)
    };
    let more = [
        hit.item as u32,
        hit.sub as u32,
        0,
        0,
        0,
        pt.0 as u32,
        pt.1 as u32,
        lparam,
        keys,
    ];
    notify_ops(cpu, hwnd, lv, code, &more)
}

/// WM_SETFOCUS (LISTVIEW_SetFocus).
pub(super) fn set_focus(cpu: &mut Cpu, hwnd: u32, lv: ListView) -> Option<u32> {
    if lv.focus {
        return Some(0);
    }
    let mut ops = notify_ops(cpu, hwnd, &lv, NM_SETFOCUS, &[]);
    ops.push(step(T_FOCUS_SET, &[1]));
    run_ops(cpu, hwnd, ops, 0)
}

/// WM_KILLFOCUS (LISTVIEW_KillFocus).
pub(super) fn kill_focus(cpu: &mut Cpu, hwnd: u32, lv: ListView) -> Option<u32> {
    if !lv.focus {
        return Some(0);
    }
    let mut ops = notify_ops(cpu, hwnd, &lv, NM_KILLFOCUS, &[]);
    ops.push(step(T_FOCUS_SET, &[0]));
    run_ops(cpu, hwnd, ops, 0)
}

/// The focus flag changed: the focused and the selected items look
/// different.
fn focus_set(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let Some(mut lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    lv.focus = c.word(0) != 0;
    put(cpu, c.hwnd, lv.clone());
    if lv.focused >= 0 {
        layout::invalidate_item(cpu, c.hwnd, &lv, lv.focused);
    }
    for i in lv.selected_items() {
        layout::invalidate_item(cpu, c.hwnd, &lv, i);
    }
    Vec::new()
}

/// WM_CHAR: the incremental search isn't there.
pub(super) fn char_(_cpu: &mut Cpu, _hwnd: u32, _lv: &ListView, _ch: u32) -> Option<u32> {
    Some(0)
}

/// WM_COMMAND (LISTVIEW_Command): only while a label is edited.
pub(super) fn command(cpu: &mut Cpu, hwnd: u32, lv: &ListView, wp: u32, lp: u32) -> Option<u32> {
    if lv.edit == 0 {
        return Some(0);
    }
    match wp >> 16 {
        EN_UPDATE => Some(0),
        EN_KILLFOCUS => run_ops(cpu, hwnd, cancel_edit_ops(lv), 0),
        _ => {
            let ops = vec![
                send_op(lv.notify, WM_COMMAND, Arg::V(wp), Arg::V(lp)),
                Op::Return(Arg::Last),
            ];
            run_ops(cpu, hwnd, ops, 0)
        }
    }
}

/// LISTVIEW_CancelEditLabel: the edit ends, keeping its text.
fn cancel_edit_ops(lv: &ListView) -> Vec<Op> {
    if lv.edit == 0 {
        return Vec::new();
    }
    vec![
        step(T_END, &[1, lv.edit]),
        send_op(lv.edit, WM_CLOSE, Arg::V(0), Arg::V(0)),
    ]
}

/// WM_KEYDOWN (LISTVIEW_KeyDown).
pub(super) fn key_down(cpu: &mut Cpu, hwnd: u32, lv: &ListView, vk: u32, _lp: u32) -> Option<u32> {
    let mut ops = notify_ops(cpu, hwnd, lv, LVN_KEYDOWN, &[vk & 0xFFFF, 0]);
    ops.push(step(T_KEY, &[vk]));
    run_ops(cpu, hwnd, ops, 0)
}

fn key(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let vk = c.word(0);
    let Some(lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    let count = lv.count();
    let focused = lv.focused;
    let mut ops = Vec::new();
    let next = |cpu: &mut Cpu, flags| items::next_item(cpu, c.hwnd, &lv, focused, flags);
    let item = match vk {
        VK_SPACE => {
            if lv.ex_style & LVS_EX_CHECKBOXES != 0 && focused >= 0 {
                let state = (items::item_state(&lv, focused) & LVIS_STATEIMAGEMASK) >> 12;
                if state == 1 || state == 2 {
                    let ch = Change::state(focused, (state ^ 3) << 12, LVIS_STATEIMAGEMASK);
                    ops.push(items::set_op(&ch));
                }
            }
            focused
        }
        VK_RETURN => {
            if count > 0 && focused != -1 {
                ops.extend(notify_ops(cpu, c.hwnd, &lv, NM_RETURN, &[]));
                ops.extend(notify_ops(cpu, c.hwnd, &lv, LVN_ITEMACTIVATE, &[]));
            }
            -1
        }
        VK_HOME => {
            if count > 0 {
                0
            } else {
                -1
            }
        }
        VK_END => {
            if count > 0 {
                count - 1
            } else {
                -1
            }
        }
        VK_LEFT => next(cpu, LVNI_TOLEFT),
        VK_UP => next(cpu, LVNI_ABOVE),
        VK_RIGHT => next(cpu, LVNI_TORIGHT),
        VK_DOWN => next(cpu, LVNI_BELOW),
        VK_PRIOR => {
            let per_col = layout::count_per_column(&lv);
            let i = if lv.view == LV_VIEW_DETAILS {
                let top = layout::top_index(cpu, c.hwnd, &lv);
                if focused == top {
                    top - per_col + 1
                } else {
                    top
                }
            } else {
                focused - per_col * layout::count_per_row(&lv)
            };
            i.max(0)
        }
        VK_NEXT => {
            let per_col = layout::count_per_column(&lv);
            let i = if lv.view == LV_VIEW_DETAILS {
                let top = layout::top_index(cpu, c.hwnd, &lv);
                if focused == top + per_col - 1 {
                    focused + per_col - 1
                } else {
                    top + per_col - 1
                }
            } else {
                focused + per_col * layout::count_per_row(&lv)
            };
            i.min(count - 1)
        }
        _ => -1,
    };
    if item != -1 && (item != focused || vk == VK_SPACE) {
        ops.extend(key_selection(cpu, c.hwnd, &lv, item, vk == VK_SPACE));
    }
    ops
}

/// LISTVIEW_KeySelection.
fn key_selection(cpu: &mut Cpu, hwnd: u32, lv: &ListView, i: i32, space: bool) -> Vec<Op> {
    let shift = key_down_now(cpu, VK_SHIFT);
    let ctrl = key_down_now(cpu, VK_CONTROL);
    let mut ops = Vec::new();
    if i >= 0 && i < lv.count() {
        if lv.style & LVS_SINGLESEL != 0 || (!shift && !ctrl) {
            ops.extend(items::set_selection_ops(lv, i));
        } else if shift {
            ops.extend(group_selection_ops(lv, i));
        } else {
            if space {
                let selected = items::item_state(lv, i) & LVIS_SELECTED;
                let new = !selected & LVIS_SELECTED;
                ops.push(items::set_op(&Change::state(i, new, LVIS_SELECTED)));
                if new != 0 {
                    ops.push(items::set_mark_op(i));
                }
            }
            ops.extend(items::set_focus_ops(lv, i));
        }
        ops.extend(layout::ensure_visible_ops(i, false));
    }
    ops.push(user_op("UpdateWindow", &[Arg::V(hwnd)]));
    ops
}

/// LISTVIEW_SetGroupSelection: the items from the selection mark to
/// `i` selected, the others not. In the icon views this takes the
/// items between the two by their index.
fn group_selection_ops(lv: &ListView, i: i32) -> Vec<Op> {
    let mut ops = Vec::new();
    let (lo, hi) = if lv.mark == -1 {
        ops.push(items::set_mark_op(i));
        (i, i + 1)
    } else {
        (lv.mark.min(i), lv.mark.max(i) + 1)
    };
    for s in lv.selected_items() {
        if s < lo || s >= hi {
            ops.push(items::set_op(&Change::state(s, 0, LVIS_SELECTED)));
        }
    }
    for s in lo..hi {
        ops.push(items::set_op(&Change::state(
            s,
            LVIS_SELECTED,
            LVIS_SELECTED,
        )));
    }
    ops.extend(items::set_focus_ops(lv, i));
    ops
}

/// WM_MOUSEHOVER (LISTVIEW_MouseHover): COMCTL32 4.7x asks the parent
/// for WM_GETDLGCODE first.
pub(super) fn mouse_hover(cpu: &mut Cpu, hwnd: u32, lv: &ListView, lp: u32) -> Option<u32> {
    let mut ops = vec![send_op(lv.notify, WM_GETDLGCODE, Arg::V(0), Arg::V(0))];
    ops.extend(notify_ops(cpu, hwnd, lv, NM_HOVER, &[]));
    ops.push(step(T_HOVERED, &[lp]));
    run_ops(cpu, hwnd, ops, 0)
}

fn hovered(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    if c.last != 0 {
        return Vec::new();
    }
    let Some(lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    let tracking = LVS_EX_TRACKSELECT | LVS_EX_ONECLICKACTIVATE | LVS_EX_TWOCLICKACTIVATE;
    if lv.ex_style & tracking == 0 {
        return Vec::new();
    }
    let (x, y) = point(c.word(0));
    let hit = layout::hit_test(cpu, c.hwnd, &lv, x, y, false, true);
    let mut ops = Vec::new();
    if hit.item >= 0 && hit.item < lv.count() {
        ops.extend(items::set_selection_ops(&lv, hit.item));
    }
    ops.push(user_op("SetFocus", &[Arg::V(c.hwnd)]));
    ops
}

/// WM_MOUSEMOVE: a drag starts when the left button moved far enough
/// from where it went down on an item.
pub(super) fn mouse_move(
    cpu: &mut Cpu,
    hwnd: u32,
    mut lv: ListView,
    _wp: u32,
    lp: u32,
) -> Option<u32> {
    if !lv.lbutton_down || lv.lbutton_item < 0 {
        return Some(0);
    }
    let (x, y) = point(lp);
    let (cx, cy) = (user32::sysparams::metric(68), user32::sysparams::metric(69));
    if (x - lv.click_pos.0).abs() <= cx && (y - lv.click_pos.1).abs() <= cy {
        return Some(0);
    }
    let item = lv.lbutton_item;
    let lparam = lv.items.get(item as usize).map_or(0, |it| it.lparam);
    lv.lbutton_item = -1;
    put(cpu, hwnd, lv.clone());
    let mut more = nmlistview(item, 0, 0, 0, 0, lparam);
    more[5] = lv.click_pos.0 as u32;
    more[6] = lv.click_pos.1 as u32;
    let ops = notify_ops(cpu, hwnd, &lv, LVN_BEGINDRAG, &more);
    run_ops(cpu, hwnd, ops, 0)
}

/// WM_LBUTTONDOWN (LISTVIEW_LButtonDown).
pub(super) fn lbutton_down(
    cpu: &mut Cpu,
    hwnd: u32,
    lv: ListView,
    wp: u32,
    lp: u32,
) -> Option<u32> {
    let mut ops = notify_ops(cpu, hwnd, &lv, NM_RELEASEDCAPTURE, &[]);
    ops.push(step(T_LBUTTON_DOWN, &[wp, lp]));
    run_ops(cpu, hwnd, ops, 0)
}

fn lbutton_down_step(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let (keys, lp) = (c.word(0), c.word(1));
    let Some(mut lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    let (x, y) = point(lp);
    lv.lbutton_down = true;
    lv.click_pos = (x, y);
    let hit = layout::hit_test(cpu, c.hwnd, &lv, x, y, false, true);
    let i = hit.item;
    let mut ops = Vec::new();
    if i >= 0 && i < lv.count() {
        lv.lbutton_item = i;
        if lv.ex_style & LVS_EX_CHECKBOXES != 0 && hit.flags & layout::LVHT_ONITEMSTATEICON != 0 {
            put(cpu, c.hwnd, lv.clone());
            let state = (items::item_state(&lv, i) & LVIS_STATEIMAGEMASK) >> 12;
            if state == 1 || state == 2 {
                let ch = Change::state(i, (state ^ 3) << 12, LVIS_STATEIMAGEMASK);
                ops.push(items::set_op(&ch));
            }
            return ops;
        }
        let selected = items::item_state(&lv, i) & LVIS_SELECTED != 0;
        let both = LVIS_SELECTED | LVIS_FOCUSED;
        if lv.style & LVS_SINGLESEL != 0 {
            if !selected {
                ops.extend(items::set_selection_ops(&lv, i));
            }
        } else if keys & MK_CONTROL != 0 && keys & MK_SHIFT != 0 {
            ops.push(items::set_op(&Change::state(i, both, both)));
            ops.push(items::set_mark_op(i));
        } else if keys & MK_CONTROL != 0 {
            let new = if selected { 0 } else { LVIS_SELECTED };
            ops.push(items::set_op(&Change::state(i, new | LVIS_FOCUSED, both)));
            ops.push(items::set_mark_op(i));
        } else if keys & MK_SHIFT != 0 {
            ops.extend(group_selection_ops(&lv, i));
        } else if selected {
            ops.extend(items::set_focus_ops(&lv, i));
        } else {
            ops.extend(items::set_selection_ops(&lv, i));
        }
        if lv.ex_style & LVS_EX_ONECLICKACTIVATE != 0 {
            ops.extend(click_ops(
                cpu,
                c.hwnd,
                &lv,
                LVN_ITEMACTIVATE,
                &hit,
                (x, y),
                keys,
            ));
        }
    } else {
        lv.lbutton_item = -1;
        if !lv.focus {
            ops.push(user_op("SetFocus", &[Arg::V(c.hwnd)]));
        }
        if keys & (MK_CONTROL | MK_SHIFT) == 0 {
            ops.extend(items::deselect_all_ops(&lv, -1));
        }
        ops.push(user_op("ReleaseCapture", &[]));
    }
    put(cpu, c.hwnd, lv);
    ops
}

/// WM_LBUTTONUP (LISTVIEW_LButtonUp).
pub(super) fn lbutton_up(cpu: &mut Cpu, hwnd: u32, lv: ListView, wp: u32, lp: u32) -> Option<u32> {
    if !lv.lbutton_down {
        return Some(0);
    }
    let (x, y) = point(lp);
    let hit = layout::hit_test(cpu, hwnd, &lv, x, y, false, false);
    let mut ops = click_ops(cpu, hwnd, &lv, NM_CLICK, &hit, (x, y), wp);
    ops.push(step(T_LBUTTON_UP, &[hit.item as u32]));
    run_ops(cpu, hwnd, ops, 0)
}

fn lbutton_up_step(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let item = c.word(0) as i32;
    let Some(mut lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    lv.lbutton_down = false;
    let mut ops = Vec::new();
    if item != -1 && item == lv.lbutton_item {
        ops.extend(items::set_selection_ops(&lv, item));
    }
    lv.lbutton_item = -1;
    if !lv.focus {
        ops.push(user_op("SetFocus", &[Arg::V(c.hwnd)]));
    }
    put(cpu, c.hwnd, lv);
    ops
}

/// WM_LBUTTONDBLCLK (LISTVIEW_LButtonDblClk).
pub(super) fn lbutton_dblclk(
    cpu: &mut Cpu,
    hwnd: u32,
    lv: &ListView,
    wp: u32,
    lp: u32,
) -> Option<u32> {
    let (x, y) = point(lp);
    let hit = layout::hit_test(cpu, hwnd, lv, x, y, false, false);
    let mut ops = notify_ops(cpu, hwnd, lv, NM_RELEASEDCAPTURE, &[]);
    ops.extend(click_ops(cpu, hwnd, lv, NM_DBLCLK, &hit, (x, y), wp));
    if hit.item != -1 {
        ops.extend(click_ops(cpu, hwnd, lv, LVN_ITEMACTIVATE, &hit, (x, y), wp));
    }
    run_ops(cpu, hwnd, ops, 0)
}

/// WM_RBUTTONDBLCLK (LISTVIEW_RButtonDblClk).
pub(super) fn rbutton_dblclk(cpu: &mut Cpu, hwnd: u32, lv: &ListView, lp: u32) -> Option<u32> {
    let (x, y) = point(lp);
    let hit = layout::hit_test(cpu, hwnd, lv, x, y, false, false);
    let mut ops = notify_ops(cpu, hwnd, lv, NM_RELEASEDCAPTURE, &[]);
    ops.extend(click_ops(cpu, hwnd, lv, NM_RDBLCLK, &hit, (x, y), 0));
    run_ops(cpu, hwnd, ops, 0)
}

/// WM_RBUTTONDOWN (LISTVIEW_RButtonDown).
pub(super) fn rbutton_down(
    cpu: &mut Cpu,
    hwnd: u32,
    lv: ListView,
    wp: u32,
    lp: u32,
) -> Option<u32> {
    let (x, y) = point(lp);
    let mut ops = notify_ops(cpu, hwnd, &lv, NM_RELEASEDCAPTURE, &[]);
    if !lv.focus {
        ops.push(user_op("SetFocus", &[Arg::V(hwnd)]));
    }
    let hit = layout::hit_test(cpu, hwnd, &lv, x, y, false, true);
    let i = hit.item;
    if i >= 0 && i < lv.count() {
        ops.extend(items::set_focus_ops(&lv, i));
        let selected = items::item_state(&lv, i) & LVIS_SELECTED != 0;
        if wp & (MK_SHIFT | MK_CONTROL) == 0 && !selected {
            ops.extend(items::set_selection_ops(&lv, i));
        }
    } else {
        ops.extend(items::deselect_all_ops(&lv, -1));
    }
    run_ops(cpu, hwnd, ops, 0)
}

/// WM_RBUTTONUP (LISTVIEW_RButtonUp): NM_RCLICK, then WM_CONTEXTMENU
/// unless the parent handled the click.
pub(super) fn rbutton_up(cpu: &mut Cpu, hwnd: u32, lv: &ListView, wp: u32, lp: u32) -> Option<u32> {
    let (x, y) = point(lp);
    let hit = layout::hit_test(cpu, hwnd, lv, x, y, false, false);
    let mut ops = click_ops(cpu, hwnd, lv, NM_RCLICK, &hit, (x, y), wp);
    ops.push(step(T_CONTEXT, &[lp]));
    run_ops(cpu, hwnd, ops, 0)
}

fn context(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    if c.last != 0 {
        return Vec::new();
    }
    let (x, y) = point(c.word(0));
    write_words(cpu, c.scratch + S_AUX, &[x as u32, y as u32]);
    vec![
        user_op("ClientToScreen", &[Arg::V(c.hwnd), Arg::S(S_AUX)]),
        step(T_CONTEXT_MENU, &[]),
    ]
}

fn context_menu(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let p = read_words(cpu, c.scratch + S_AUX, 2);
    let lp = (p[0] & 0xFFFF) | ((p[1] & 0xFFFF) << 16);
    vec![send_op(c.hwnd, WM_CONTEXTMENU, Arg::V(c.hwnd), Arg::V(lp))]
}

/// LVM_EDITLABEL (LISTVIEW_EditLabelT): the edit control, or 0.
pub(super) fn edit_label(cpu: &mut Cpu, hwnd: u32, lv: &ListView, i: i32) -> Option<u32> {
    if lv.style & LVS_EDITLABELS == 0 {
        return Some(0);
    }
    let mut ops = Vec::new();
    if lv.edit != 0 {
        // The edit loses the focus, which ends it.
        ops.push(user_op("SetFocus", &[Arg::V(hwnd)]));
        ops.push(step(T_FORGET_EDIT, &[]));
    }
    ops.push(step(T_EDIT_BEGIN, &[i as u32]));
    run_ops(cpu, hwnd, ops, 0)
}

fn edit_begin(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let i = c.word(0) as i32;
    let Some(mut lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    if i < 0 || i >= lv.count() {
        return Vec::new();
    }
    // Without the focus there is no edit.
    let focus = user32::user(cpu).input.focus;
    if focus != c.hwnd && (focus == 0 || super::super::parent(cpu, focus) != c.hwnd) {
        return Vec::new();
    }
    lv.edit_item = i;
    put(cpu, c.hwnd, lv.clone());
    let mut ops = items::set_selection_ops(&lv, i);
    ops.push(step(T_EDIT_READ, &[i as u32]));
    ops
}

fn edit_read(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let i = c.word(0) as i32;
    if let Some(lv) = get(cpu, c.hwnd) {
        layout::invalidate_item(cpu, c.hwnd, &lv, i);
    }
    let mask = LVIF_PARAM | LVIF_STATE | LVIF_TEXT;
    let mut ops = items::read_own_ops(cpu, c, i, 0, mask, !0);
    ops.push(step(T_EDIT_CREATE, &[i as u32]));
    ops
}

/// The text a label has, at `S_TEXT2`, NUL-terminated.
fn put_text2(cpu: &mut Cpu, scratch: u32, text: &[u8]) {
    let n = text.len().min(TEXT_MAX as usize - 1);
    for (k, &b) in text[..n].iter().enumerate() {
        cpu.bus.guest_write_8(scratch + S_TEXT2 + k as u32, b);
    }
    cpu.bus.guest_write_8(scratch + S_TEXT2 + n as u32, 0);
}

fn text2(cpu: &mut Cpu, scratch: u32) -> Vec<u8> {
    let mut out = Vec::new();
    for k in 0..TEXT_MAX - 1 {
        let b = cpu.bus.guest_read_8(scratch + S_TEXT2 + k);
        if b == 0 {
            break;
        }
        out.push(b);
    }
    out
}

fn edit_create(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let i = c.word(0) as i32;
    let Some(lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    let Some(r) = layout::item_rect(cpu, c.hwnd, &lv, i, LVIR_LABEL) else {
        return Vec::new();
    };
    let got = items::read_own(cpu, c.scratch);
    put_text2(cpu, c.scratch, &got.text.unwrap_or_default());
    let class = c.scratch + S_AUX;
    for (k, &b) in b"Edit\0".iter().enumerate() {
        cpu.bus.guest_write_8(class + k as u32, b);
    }
    let hinst = user32::user(cpu).window(c.hwnd).map_or(0, |w| w.instance);
    // As with the header, the parent doesn't hear of the edit control.
    let args = [
        Arg::V(0x0000_0004),
        Arg::S(S_AUX),
        Arg::S(S_TEXT2),
        Arg::V(EDIT_STYLE),
        Arg::V(0),
        Arg::V(0),
        Arg::V(0),
        Arg::V(0),
        Arg::V(c.hwnd),
        Arg::V(0),
        Arg::V(hinst),
        Arg::V(0),
    ];
    let data = [
        i as u32,
        got.state,
        got.lparam,
        r.left as u32,
        r.top as u32,
        r.right as u32,
        r.bottom as u32,
    ];
    vec![user_op("CreateWindowExA", &args), step(T_EDIT_MADE, &data)]
}

fn edit_made(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let edit = c.last;
    let Some(mut lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    if edit == 0 {
        return Vec::new();
    }
    let (i, state, lparam) = (c.word(0) as i32, c.word(1), c.word(2));
    let ours = super::super::address_of("ListViewEditProc");
    if let Some(w) = user32::user(cpu).window_mut(edit) {
        lv.edit_proc = std::mem::replace(&mut w.proc, ours);
    }
    lv.edit = edit;
    put(cpu, c.hwnd, lv.clone());
    let font = lv.hfont(cpu);
    let mut ops = vec![send_op(edit, WM_SETFONT, Arg::V(font), Arg::V(0))];
    let mask = LVIF_PARAM | LVIF_STATE | LVIF_TEXT;
    let item = [
        mask,
        i as u32,
        0,
        state,
        !0,
        c.scratch + S_TEXT2,
        TEXT_MAX,
        0,
        lparam,
        0,
    ];
    ops.extend(notify_ops(cpu, c.hwnd, &lv, LVN_BEGINLABELEDITW, &item));
    let mut data = c.data.clone();
    data.push(edit);
    ops.push(step(T_EDIT_BEGUN, &data));
    ops
}

fn edit_begun(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let edit = c.word(7);
    let Some(mut lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    if c.last != 0 {
        lv.edit = 0;
        put(cpu, c.hwnd, lv);
        return vec![send_op(edit, WM_CLOSE, Arg::V(0), Arg::V(0))];
    }
    let r = Rect::new(
        c.word(3) as i32,
        c.word(4) as i32,
        c.word(5) as i32,
        c.word(6) as i32,
    );
    let text = text2(cpu, c.scratch);
    let cx = layout::string_width(cpu, c.hwnd, &lv, &text) + lv.max_char_width * 2;
    let args = [
        Arg::V(edit),
        Arg::V((r.left - 2) as u32),
        Arg::V((r.top - 1) as u32),
        Arg::V(cx as u32),
        Arg::V((r.height() + 2) as u32),
        Arg::V(0),
    ];
    vec![
        user_op("MoveWindow", &args),
        user_op("ShowWindow", &[Arg::V(edit), Arg::V(1)]),
        user_op("SetFocus", &[Arg::V(edit)]),
        send_op(edit, EM_SETSEL, Arg::V(0), Arg::V(0xFFFF_FFFF)),
        Op::Return(Arg::V(edit)),
    ]
}

/// LISTVIEW_EndEditLabelT: `c.word(0)` says whether the edit's text
/// is kept, `c.word(1)` is the edit control.
fn end(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let (save, edit) = (c.word(0), c.word(1));
    put_text2(cpu, c.scratch, b"");
    if save != 0 {
        let args = [Arg::V(edit), Arg::S(S_TEXT2), Arg::V(TEXT_MAX)];
        vec![user_op("GetWindowTextA", &args), step(T_END_TEXT, &[1])]
    } else {
        vec![step(T_END_TEXT, &[0])]
    }
}

fn end_text(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let len = if c.word(0) != 0 { c.last } else { 0 };
    let Some(mut lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    let item = lv.edit_item;
    lv.edit_item = -1;
    lv.edit = 0;
    put(cpu, c.hwnd, lv);
    let mask = LVIF_PARAM | LVIF_STATE | LVIF_TEXT;
    let mut ops = items::read_own_ops(cpu, c, item, 0, mask, !0);
    ops.push(step(T_END_GOT, &[item as u32, len]));
    ops
}

fn end_got(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let (item, len) = (c.word(0) as i32, c.word(1));
    let Some(lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    if item < 0 || item >= lv.count() {
        return Vec::new();
    }
    let got = items::read_own(cpu, c.scratch);
    let new = text2(cpu, c.scratch);
    let has = len != 0;
    let same = got.text.as_deref().unwrap_or(b"") == new.as_slice();
    let (ptr, max) = if same || !has {
        (0, 0)
    } else {
        (c.scratch + S_TEXT2, new.len() as u32)
    };
    let mask = LVIF_PARAM | LVIF_STATE | LVIF_TEXT;
    let words = [
        mask,
        item as u32,
        0,
        got.state,
        !0,
        ptr,
        max,
        got.image as u32,
        got.lparam,
        got.indent as u32,
    ];
    let mut ops = notify_ops(cpu, c.hwnd, &lv, LVN_ENDLABELEDITW, &words);
    ops.push(step(T_END_DONE, &[item as u32, has as u32, same as u32]));
    // COMCTL32 4.7x paints the label before the edit control goes.
    ops.push(user_op("UpdateWindow", &[Arg::V(c.hwnd)]));
    ops
}

fn end_done(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let (item, has, same) = (c.word(0) as i32, c.word(1) != 0, c.word(2) != 0);
    if c.last == 0 || !has || same {
        return Vec::new();
    }
    let Some(lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    if !lv.ownerdata() && lv.items.get(item as usize).is_some_and(|it| it.callback) {
        layout::invalidate_item(cpu, c.hwnd, &lv, item);
        return Vec::new();
    }
    let text = text2(cpu, c.scratch);
    let ch = Change {
        item,
        mask: LVIF_TEXT,
        text: Text::Some(text),
        ..Change::default()
    };
    items::set_item_ops(cpu, c.hwnd, &ch).1
}

/// The edit control's procedure while it edits a label
/// (EditLblWndProcT): Enter keeps the text, Escape drops it, and both
/// end the edit.
pub(super) fn edit_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[edit, msg, wp, lp] = args else {
        unreachable!()
    };
    let lvh = super::super::parent(cpu, edit);
    let Some(mut lv) = get(cpu, lvh) else {
        return defwnd::def_window_proc(cpu, args);
    };
    let old = lv.edit_proc;
    if old == 0 {
        return defwnd::def_window_proc(cpu, args);
    }
    let call_old = |old: u32| {
        vec![
            user_op(
                "CallWindowProcA",
                &[
                    Arg::V(old),
                    Arg::V(edit),
                    Arg::V(msg),
                    Arg::V(wp),
                    Arg::V(lp),
                ],
            ),
            Op::Return(Arg::Last),
        ]
    };
    match msg {
        WM_GETDLGCODE => Some(DLGC_WANTARROWS | DLGC_WANTALLKEYS),
        WM_DESTROY => {
            if let Some(w) = user32::user(cpu).window_mut(edit) {
                w.proc = old;
            }
            lv.edit_proc = 0;
            put(cpu, lvh, lv);
            run_ops(cpu, lvh, call_old(old), 0)
        }
        WM_KEYDOWN if wp == VK_ESCAPE || wp == VK_RETURN => {
            let mut ops = Vec::new();
            if lv.edit != 0 {
                ops.push(step(T_END, &[(wp == VK_RETURN) as u32, edit]));
            }
            ops.push(send_op(edit, WM_CLOSE, Arg::V(0), Arg::V(0)));
            ops.push(Op::Return(Arg::V(0)));
            run_ops(cpu, lvh, ops, 0)
        }
        _ => run_ops(cpu, lvh, call_old(old), 0),
    }
}

/// The continuations.
pub(super) fn then(cpu: &mut Cpu, id: u16, c: &mut Ctx) -> Vec<Op> {
    match id {
        T_KEY => key(cpu, c),
        T_FOCUS_SET => focus_set(cpu, c),
        T_HOVERED => hovered(cpu, c),
        T_EDIT_BEGIN => edit_begin(cpu, c),
        T_EDIT_READ => edit_read(cpu, c),
        T_EDIT_CREATE => edit_create(cpu, c),
        T_EDIT_MADE => edit_made(cpu, c),
        T_EDIT_BEGUN => edit_begun(cpu, c),
        T_FORGET_EDIT => {
            if let Some(mut lv) = get(cpu, c.hwnd) {
                lv.edit = 0;
                put(cpu, c.hwnd, lv);
            }
            Vec::new()
        }
        T_END => end(cpu, c),
        T_END_TEXT => end_text(cpu, c),
        T_END_GOT => end_got(cpu, c),
        T_END_DONE => end_done(cpu, c),
        T_LBUTTON_DOWN => lbutton_down_step(cpu, c),
        T_LBUTTON_UP => lbutton_up_step(cpu, c),
        T_CONTEXT => context(cpu, c),
        T_CONTEXT_MENU => context_menu(cpu, c),
        _ => Vec::new(),
    }
}
