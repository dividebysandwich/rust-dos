//! The items: inserting, changing, reading and deleting them, their
//! states with the selection and the focus, searching and sorting.

use super::layout;
use super::*;

pub(super) const T_SET: u16 = T_ITEMS;
const T_SET_APPLY: u16 = T_ITEMS + 1;
const T_SEL_ADD: u16 = T_ITEMS + 2;
const T_FOCUS: u16 = T_ITEMS + 3;
pub(super) const T_GET: u16 = T_ITEMS + 4;
const T_GET_DONE: u16 = T_ITEMS + 5;
const T_DELETE: u16 = T_ITEMS + 6;
const T_DELETE_DONE: u16 = T_ITEMS + 7;
pub(super) const T_DELETE_ALL: u16 = T_ITEMS + 8;
const T_DELETE_ALL_ITEMS: u16 = T_ITEMS + 9;
const T_DELETE_ALL_ITEM: u16 = T_ITEMS + 10;
const T_DELETE_ALL_DROP: u16 = T_ITEMS + 11;
const T_DELETE_ALL_END: u16 = T_ITEMS + 12;
const T_SORT_STEP: u16 = T_ITEMS + 13;
const T_SORT_CMP: u16 = T_ITEMS + 14;
const T_OD_TEXT: u16 = T_ITEMS + 15;
const T_GET_STATE: u16 = T_ITEMS + 16;
const T_SET_MARK: u16 = T_ITEMS + 17;

/// What LVM_GETITEM is used for, which decides what the message
/// returns: whether it worked, the length of the text, the state, or
/// nothing (for the control's own use).
pub(super) const GET_ITEM: u32 = 0;
pub(super) const GET_TEXT: u32 = 1;
pub(super) const GET_STATE: u32 = 2;
pub(super) const GET_OWN: u32 = 3;

const LVNI_FOCUSED: u32 = 0x0001;
const LVNI_SELECTED: u32 = 0x0002;
const LVNI_CUT: u32 = 0x0004;
const LVNI_DROPHILITED: u32 = 0x0008;
const LVNI_ABOVE: u32 = 0x0100;
const LVNI_BELOW: u32 = 0x0200;
const LVNI_TOLEFT: u32 = 0x0400;
const LVNI_TORIGHT: u32 = 0x0800;

const LVFI_PARAM: u32 = 0x0001;
const LVFI_STRING: u32 = 0x0002;
const LVFI_SUBSTRING: u32 = 0x0004;
const LVFI_PARTIAL: u32 = 0x0008;
const LVFI_WRAP: u32 = 0x0020;
const LVFI_NEARESTXY: u32 = 0x0040;

const VK_PRIOR: u32 = 0x21;
const VK_NEXT: u32 = 0x22;
const VK_END: u32 = 0x23;
const VK_HOME: u32 = 0x24;
const VK_LEFT: u32 = 0x25;
const VK_UP: u32 = 0x26;
const VK_RIGHT: u32 = 0x27;
const VK_DOWN: u32 = 0x28;

const LVSICF_NOINVALIDATEALL: u32 = 0x0001;
const LVSICF_NOSCROLL: u32 = 0x0002;

/// Text given with an item: none (a NULL pointer), a callback, or the
/// text itself, as ANSI.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Text {
    #[default]
    Null,
    Callback,
    Some(Vec<u8>),
}

impl Text {
    /// The text at `ptr`: None if a page isn't there.
    pub fn read(cpu: &mut Cpu, ptr: u32, wide: bool) -> Option<Text> {
        Some(match ptr {
            0 => Text::Null,
            LPSTR_TEXTCALLBACK => Text::Callback,
            _ => Text::Some(header::read_text(cpu, ptr, wide)?),
        })
    }

    /// The text an item keeps.
    pub fn of(text: &Option<Vec<u8>>, callback: bool) -> Text {
        if callback {
            Text::Callback
        } else {
            text.clone().map_or(Text::Null, Text::Some)
        }
    }

    /// As an item keeps it: its text and whether it is a callback.
    pub fn stored(&self) -> (Option<Vec<u8>>, bool) {
        match self {
            Text::Null => (None, false),
            Text::Callback => (None, true),
            Text::Some(t) => (Some(t.clone()), false),
        }
    }

    fn encode(&self) -> Vec<u32> {
        match self {
            Text::Null => vec![0, 0],
            Text::Callback => vec![1, 0],
            Text::Some(t) => {
                let mut v = vec![2, t.len() as u32];
                v.extend(script::words_of(t));
                v
            }
        }
    }

    fn decode(w: &[u32]) -> Text {
        let get = |i: usize| w.get(i).copied().unwrap_or(0);
        match get(0) {
            1 => Text::Callback,
            2 => {
                let len = get(1) as usize;
                let bytes: Vec<u8> = w
                    .iter()
                    .skip(2)
                    .flat_map(|x| x.to_le_bytes())
                    .take(len)
                    .collect();
                Text::Some(bytes)
            }
            _ => Text::Null,
        }
    }
}

/// textcmpWT: whether setting `new` over `old` changes the text. Two
/// callbacks count as a change.
fn text_changes(old: &Text, new: &Text) -> bool {
    match (old, new) {
        (Text::Null, Text::Null) => false,
        (Text::Some(a), Text::Some(b)) => a != b,
        _ => true,
    }
}

/// A change to an item: the fields of an LVITEM, and whether its
/// failure makes the message fail.
#[derive(Clone, Debug, Default)]
pub struct Change {
    pub item: i32,
    pub sub: i32,
    pub mask: u32,
    pub state: u32,
    pub state_mask: u32,
    pub image: i32,
    pub lparam: u32,
    pub indent: i32,
    pub text: Text,
    pub counts: bool,
}

impl Change {
    /// A change of the state of `item`.
    pub fn state(item: i32, state: u32, mask: u32) -> Change {
        Change {
            item,
            mask: LVIF_STATE,
            state,
            state_mask: mask,
            ..Change::default()
        }
    }

    /// The LVITEM at `at`: None if a page isn't there.
    pub fn read(cpu: &mut Cpu, at: u32, wide: bool) -> Option<Change> {
        let w = read_words(cpu, at, 9);
        if cpu.bus.guest_faulted() {
            return None;
        }
        let mask = w[0];
        let indent = if mask & LVIF_INDENT != 0 {
            let v = cpu.bus.guest_read_32(at + 36);
            if cpu.bus.guest_faulted() {
                return None;
            }
            v as i32
        } else {
            0
        };
        let text = if mask & LVIF_TEXT != 0 {
            Text::read(cpu, w[5], wide)?
        } else {
            Text::Null
        };
        Some(Change {
            item: w[1] as i32,
            sub: w[2] as i32,
            mask,
            state: w[3],
            state_mask: w[4],
            image: w[7] as i32,
            lparam: w[8],
            indent,
            text,
            counts: false,
        })
    }

    fn encode(&self) -> Vec<u32> {
        let mut v = vec![
            self.item as u32,
            self.sub as u32,
            self.mask,
            self.state,
            self.state_mask,
            self.image as u32,
            self.lparam,
            self.indent as u32,
            self.counts as u32,
        ];
        v.extend(self.text.encode());
        v
    }

    fn decode(w: &[u32]) -> (Change, usize) {
        let get = |i: usize| w.get(i).copied().unwrap_or(0);
        let text = Text::decode(w.get(9..).unwrap_or(&[]));
        let used = 11 + (get(10) as usize).div_ceil(4);
        let used = if get(9) == 2 { used } else { 11 };
        let c = Change {
            item: get(0) as i32,
            sub: get(1) as i32,
            mask: get(2),
            state: get(3),
            state_mask: get(4),
            image: get(5) as i32,
            lparam: get(6),
            indent: get(7) as i32,
            counts: get(8) != 0,
            text,
        };
        (c, used)
    }
}

/// The step that makes `change`.
pub(super) fn set_op(change: &Change) -> Op {
    step(T_SET, &change.encode())
}

/// The state of item `i` as the control keeps it: the item's own bits
/// for the control's items, with the focus and the selection.
pub(super) fn item_state(lv: &ListView, i: i32) -> u32 {
    let mut s = if lv.ownerdata() {
        0
    } else {
        lv.items.get(i as usize).map_or(0, |it| it.state)
    };
    if lv.callback_mask & LVIS_FOCUSED == 0 {
        s &= !LVIS_FOCUSED;
        if lv.focused == i {
            s |= LVIS_FOCUSED;
        }
    }
    if lv.callback_mask & LVIS_SELECTED == 0 {
        s &= !LVIS_SELECTED;
        if lv.is_selected(i) {
            s |= LVIS_SELECTED;
        }
    }
    s
}

/// The steps that deselect every selected item but `skip`.
pub(super) fn deselect_all_ops(lv: &ListView, skip: i32) -> Vec<Op> {
    lv.selected_items()
        .into_iter()
        .filter(|&i| i != skip)
        .map(|i| set_op(&Change::state(i, 0, LVIS_SELECTED)))
        .collect()
}

/// The steps of LISTVIEW_SetSelection: `i` alone selected, focused and
/// the selection mark.
pub(super) fn set_selection_ops(lv: &ListView, i: i32) -> Vec<Op> {
    let mut ops = deselect_all_ops(lv, i);
    let both = LVIS_FOCUSED | LVIS_SELECTED;
    ops.push(set_op(&Change::state(i, both, both)));
    ops.push(step(T_SET_MARK, &[i as u32]));
    ops
}

/// The steps of LISTVIEW_SetItemFocus.
pub(super) fn set_focus_ops(lv: &ListView, i: i32) -> Vec<Op> {
    if i == lv.focused {
        return Vec::new();
    }
    if i == -1 {
        return vec![set_op(&Change::state(lv.focused, 0, LVIS_FOCUSED))];
    }
    vec![set_op(&Change::state(i, LVIS_FOCUSED, LVIS_FOCUSED))]
}

/// The step that sets the selection mark.
pub(super) fn set_mark_op(i: i32) -> Op {
    step(T_SET_MARK, &[i as u32])
}

/// Change an item or a subitem (LISTVIEW_SetItemT): whether it can be
/// done, and the steps that do it.
pub(super) fn set_item_ops(cpu: &mut Cpu, hwnd: u32, ch: &Change) -> (bool, Vec<Op>) {
    let Some(lv) = get(cpu, hwnd) else {
        return (false, Vec::new());
    };
    if ch.item < 0 || ch.item >= lv.count() {
        return (false, Vec::new());
    }
    if ch.mask & LVIF_TEXT != 0
        && ch.text == Text::Callback
        && lv.style & (LVS_SORTASCENDING | LVS_SORTDESCENDING) != 0
    {
        return (false, Vec::new());
    }
    if ch.sub != 0 {
        return (set_sub_item(cpu, hwnd, lv, ch), Vec::new());
    }
    set_main_item(cpu, hwnd, lv, ch)
}

/// set_sub_item: whether it worked.
fn set_sub_item(cpu: &mut Cpu, hwnd: u32, mut lv: ListView, ch: &Change) -> bool {
    if lv.ownerdata() || ch.sub < 0 || ch.sub as usize >= lv.columns.len() {
        return false;
    }
    if ch.mask & !(LVIF_TEXT | LVIF_IMAGE | LVIF_STATE) != 0 {
        return false;
    }
    if ch.mask & (LVIF_TEXT | LVIF_IMAGE | LVIF_STATE) == 0 {
        return true;
    }
    let Some(item) = lv.items.get_mut(ch.item as usize) else {
        return false;
    };
    let mut changed = false;
    let at = match item.subs.iter().position(|s| s.sub == ch.sub) {
        Some(at) => at,
        None => {
            let at = item
                .subs
                .iter()
                .position(|s| s.sub > ch.sub)
                .unwrap_or(item.subs.len());
            item.subs.insert(
                at,
                SubItem {
                    sub: ch.sub,
                    image: I_IMAGECALLBACK,
                    ..SubItem::default()
                },
            );
            changed = true;
            at
        }
    };
    let s = &mut item.subs[at];
    if ch.mask & LVIF_IMAGE != 0 && s.image != ch.image {
        s.image = ch.image;
        changed = true;
    }
    if ch.mask & LVIF_TEXT != 0 && text_changes(&Text::of(&s.text, s.callback), &ch.text) {
        (s.text, s.callback) = ch.text.stored();
        changed = true;
    }
    if ch.mask & LVIF_STATE != 0 {
        s.state = (s.state & !ch.state_mask) | (ch.state & ch.state_mask);
    }
    put(cpu, hwnd, lv.clone());
    if changed && !lv.drawing {
        if lv.view == LV_VIEW_DETAILS
            && lv.style & LVS_OWNERDRAWFIXED == 0
            && lv.ex_style & LVS_EX_FULLROWSELECT == 0
        {
            layout::invalidate_subitem(cpu, hwnd, &lv, ch.item, ch.sub);
        } else {
            layout::invalidate_item(cpu, hwnd, &lv, ch.item);
        }
    }
    true
}

/// The fields of the item `ch.item` that `ch` changes (LVIF_ bits).
fn changes_of(lv: &ListView, ch: &Change, old_state: u32, state_mask: u32) -> u32 {
    let mut changed = 0;
    if ch.mask & LVIF_STATE != 0 && (old_state ^ ch.state) & state_mask & !lv.callback_mask != 0 {
        changed |= LVIF_STATE;
    }
    if let Some(item) = lv.items.get(ch.item as usize) {
        if ch.mask & LVIF_IMAGE != 0 && item.image != ch.image {
            changed |= LVIF_IMAGE;
        }
        if ch.mask & LVIF_PARAM != 0 && item.lparam != ch.lparam {
            changed |= LVIF_PARAM;
        }
        if ch.mask & LVIF_INDENT != 0 && item.indent != ch.indent {
            changed |= LVIF_INDENT;
        }
        if ch.mask & LVIF_TEXT != 0 && text_changes(&Text::of(&item.text, item.callback), &ch.text)
        {
            changed |= LVIF_TEXT;
        }
    }
    changed
}

/// set_main_item for an item that is already there.
fn set_main_item(cpu: &mut Cpu, hwnd: u32, lv: ListView, ch: &Change) -> (bool, Vec<Op>) {
    if ch.mask == 0 {
        return (true, Vec::new());
    }
    if lv.ownerdata() && ch.mask & !LVIF_STATE != 0 {
        return (false, Vec::new());
    }
    let old = item_state(&lv, ch.item);
    let old_lparam = lv.items.get(ch.item as usize).map_or(0, |it| it.lparam);
    let changed = changes_of(&lv, ch, old, ch.state_mask);
    if changed == 0 {
        return (true, Vec::new());
    }
    let new = (old & !ch.state_mask) | (ch.state & ch.state_mask);
    let mut data = ch.encode();
    data.extend([old, old_lparam, changed]);
    if !lv.ownerdata() && lv.change_notify {
        let nm = nmlistview(ch.item, 0, new, old, changed, old_lparam);
        let mut ops = notify_ops(cpu, hwnd, &lv, LVN_ITEMCHANGING, &nm);
        ops.push(step(T_SET_APPLY, &data));
        return (true, ops);
    }
    (
        true,
        apply_main(cpu, hwnd, ch, old, old_lparam, changed, false),
    )
}

/// The rest of set_main_item once the change is allowed: the item
/// changed, and the steps that follow.
fn apply_main(
    cpu: &mut Cpu,
    hwnd: u32,
    ch: &Change,
    old: u32,
    old_lparam: u32,
    changed: u32,
    is_new: bool,
) -> Vec<Op> {
    let Some(mut lv) = get(cpu, hwnd) else {
        return Vec::new();
    };
    if ch.item < 0 || ch.item >= lv.count() {
        return Vec::new();
    }
    let i = ch.item;
    let state_mask = if is_new { !0 } else { ch.state_mask };
    let cbm = lv.callback_mask;
    let mut ops = Vec::new();
    if let Some(item) = lv.items.get_mut(i as usize) {
        if ch.mask & LVIF_TEXT != 0 {
            (item.text, item.callback) = ch.text.stored();
        }
        if ch.mask & LVIF_IMAGE != 0 {
            item.image = ch.image;
        }
        if ch.mask & LVIF_PARAM != 0 {
            item.lparam = ch.lparam;
        }
        if ch.mask & LVIF_INDENT != 0 {
            item.indent = ch.indent;
        }
    }
    let mut focus_to = None;
    if changed & LVIF_STATE != 0 {
        if !lv.ownerdata()
            && state_mask & !cbm != 0
            && let Some(item) = lv.items.get_mut(i as usize)
        {
            item.state = (item.state & !state_mask) | (ch.state & state_mask);
        }
        if ch.state & state_mask & !cbm & LVIS_SELECTED != 0 {
            if lv.style & LVS_SINGLESEL != 0 {
                let others = deselect_all_ops(&lv, i);
                if !others.is_empty() {
                    ops.extend(others);
                    ops.push(step(T_SEL_ADD, &[i as u32]));
                } else {
                    lv.select_range(i, i + 1);
                }
            } else {
                lv.select_range(i, i + 1);
            }
        } else if state_mask & LVIS_SELECTED != 0 {
            lv.deselect_range(i, i + 1);
        }
        if state_mask & !cbm & LVIS_FOCUSED != 0 {
            if ch.state & LVIS_FOCUSED != 0 {
                if lv.focused != -1 && lv.focused != i {
                    ops.push(set_op(&Change::state(lv.focused, 0, LVIS_FOCUSED)));
                }
                focus_to = Some(i);
            } else if lv.focused == i {
                lv.focused = -1;
            }
        }
    }
    put(cpu, hwnd, lv.clone());
    if let Some(f) = focus_to {
        ops.push(step(T_FOCUS, &[f as u32]));
    }
    if is_new {
        return ops;
    }
    let new = (old & !state_mask) | (ch.state & state_mask);
    let lparam = if ch.mask & LVIF_PARAM != 0 {
        ch.lparam
    } else {
        old_lparam
    };
    if lv.change_notify {
        let nm = nmlistview(i, 0, new, old, changed, lparam);
        ops.extend(notify_ops(cpu, hwnd, &lv, LVN_ITEMCHANGED, &nm));
    }
    if lv.ownerdata() && changed & LVIF_STATE != 0 {
        // COMCTL32 4.7x asks for the text of a virtual item whose state
        // changed, to redraw it.
        ops.push(step(T_OD_TEXT, &[i as u32]));
    }
    if !lv.drawing {
        layout::invalidate_item(cpu, hwnd, &lv, i);
    }
    ops
}

/// LVM_SETITEMA and LVM_SETITEMW.
pub(super) fn set_item_msg(cpu: &mut Cpu, hwnd: u32, lp: u32, wide: bool) -> Option<u32> {
    if lp == 0 {
        return Some(0);
    }
    // LVM_SETITEM is refused on a virtual list view.
    if get(cpu, hwnd).is_some_and(|lv| lv.ownerdata()) {
        return Some(0);
    }
    let mut ch = Change::read(cpu, lp, wide)?;
    ch.counts = true;
    let (ok, ops) = set_item_ops(cpu, hwnd, &ch);
    if !ok {
        return Some(0);
    }
    run_ops(cpu, hwnd, ops, 1)
}

/// LVM_SETITEMSTATE.
pub(super) fn set_item_state_msg(
    cpu: &mut Cpu,
    hwnd: u32,
    lv: &ListView,
    i: i32,
    state: u32,
    mask: u32,
) -> Option<u32> {
    if i != -1 {
        let mut ch = Change::state(i, state, mask);
        ch.counts = true;
        let (ok, ops) = set_item_ops(cpu, hwnd, &ch);
        if !ok {
            return Some(0);
        }
        return run_ops(cpu, hwnd, ops, 1);
    }
    if state & mask & LVIS_SELECTED != 0 && lv.style & LVS_SINGLESEL != 0 {
        return Some(0);
    }
    if state & mask & LVIS_FOCUSED != 0 {
        return Some(0);
    }
    if lv.ownerdata() {
        return set_all_virtual(cpu, hwnd, lv.clone(), state, mask);
    }
    let ops = (0..lv.count())
        .map(|i| {
            let mut ch = Change::state(i, state, mask);
            ch.counts = true;
            set_op(&ch)
        })
        .collect();
    run_ops(cpu, hwnd, ops, 1)
}

/// LVM_SETITEMSTATE of every item of a virtual list view: COMCTL32
/// 4.7x changes the selection at once and tells the parent with one
/// LVN_ITEMCHANGED for item -1, after LVN_ODCACHEHINT when it
/// deselects them all.
fn set_all_virtual(
    cpu: &mut Cpu,
    hwnd: u32,
    mut lv: ListView,
    state: u32,
    mask: u32,
) -> Option<u32> {
    let mut ops = Vec::new();
    if mask & LVIS_FOCUSED != 0 && lv.focused != -1 {
        ops.push(set_op(&Change::state(lv.focused, 0, LVIS_FOCUSED)));
    }
    if mask & LVIS_SELECTED != 0 {
        if state & LVIS_SELECTED != 0 {
            lv.selection.clear();
            let n = lv.count();
            lv.select_range(0, n);
        } else {
            lv.selection.clear();
            let last = (lv.count() - 1) as u32;
            ops.extend(notify_ops(cpu, hwnd, &lv, LVN_ODCACHEHINT, &[0, last]));
        }
        put(cpu, hwnd, lv.clone());
        layout::invalidate_list(cpu, hwnd, &lv);
    }
    let nm = nmlistview(-1, 0, state & mask, 0, LVIF_STATE, 0);
    ops.extend(notify_ops(cpu, hwnd, &lv, LVN_ITEMCHANGED, &nm));
    run_ops(cpu, hwnd, ops, 1)
}

/// LVM_SETITEMTEXTA and LVM_SETITEMTEXTW.
pub(super) fn set_item_text_msg(
    cpu: &mut Cpu,
    hwnd: u32,
    i: i32,
    lp: u32,
    wide: bool,
) -> Option<u32> {
    if lp == 0 {
        return Some(0);
    }
    let w = read_words(cpu, lp, 6);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let text = Text::read(cpu, w[5], wide)?;
    let ch = Change {
        item: i,
        sub: w[2] as i32,
        mask: LVIF_TEXT,
        text,
        counts: true,
        ..Change::default()
    };
    let (ok, ops) = set_item_ops(cpu, hwnd, &ch);
    if !ok {
        return Some(0);
    }
    run_ops(cpu, hwnd, ops, 1)
}

/// The id the next item gets.
fn next_id(lv: &ListView) -> u32 {
    lv.items.iter().map(|it| it.id + 1).max().unwrap_or(0)
}

/// Compare two item texts as lstrcmp does, letters before case.
pub(super) fn compare_text(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    let lower = |s: &[u8]| {
        s.iter()
            .map(|c| c.to_ascii_lowercase())
            .collect::<Vec<u8>>()
    };
    lower(a).cmp(&lower(b)).then_with(|| b.cmp(a))
}

/// shift_item.
fn shift_index(s: i32, i: i32, dir: i32, count: i32) -> i32 {
    if s < i {
        s
    } else if s > i || dir > 0 {
        s + dir
    } else {
        s.min(count - 1)
    }
}

/// LISTVIEW_ShiftIndices, once the count has changed.
fn shift_indices(lv: &mut ListView, i: i32, dir: i32) {
    let count = lv.count();
    lv.shift_selection(i, dir, count);
    lv.mark = shift_index(lv.mark, i, dir, count);
    lv.focused = shift_index(lv.focused, i, dir, count);
}

/// LVM_INSERTITEMA and LVM_INSERTITEMW.
pub(super) fn insert_item(
    cpu: &mut Cpu,
    hwnd: u32,
    mut lv: ListView,
    lp: u32,
    wide: bool,
) -> Option<u32> {
    if lv.ownerdata() {
        let n = lv.od_count;
        lv.od_count += 1;
        put(cpu, hwnd, lv);
        return Some(n as u32);
    }
    if lp == 0 {
        return Some(-1i32 as u32);
    }
    let mut ch = Change::read(cpu, lp, wide)?;
    if ch.sub != 0 {
        return Some(-1i32 as u32);
    }
    let sorting = lv.style & (LVS_SORTASCENDING | LVS_SORTDESCENDING) != 0;
    if ch.mask & LVIF_TEXT != 0 && ch.text == Text::Callback && sorting {
        return Some(-1i32 as u32);
    }
    let is_sorted = sorting && lv.style & LVS_OWNERDRAWFIXED == 0 && ch.text != Text::Callback;
    if ch.item < 0 && !is_sorted {
        return Some(-1i32 as u32);
    }
    let count = lv.count();
    let at = if is_sorted {
        let new = match &ch.text {
            Text::Some(t) => t.clone(),
            _ => Vec::new(),
        };
        let descending = lv.style & LVS_SORTDESCENDING != 0;
        lv.items
            .iter()
            .position(|it| {
                let cmp = match (&it.text, it.callback) {
                    (_, true) => std::cmp::Ordering::Less,
                    (None, _) => std::cmp::Ordering::Less,
                    (Some(t), _) => compare_text(t, &new),
                };
                let cmp = if descending { cmp.reverse() } else { cmp };
                cmp != std::cmp::Ordering::Less
            })
            .unwrap_or(lv.items.len()) as i32
    } else {
        ch.item.min(count)
    };
    let item = Item {
        id: next_id(&lv),
        ..Item::default()
    };
    lv.items.insert(at as usize, item);
    shift_indices(&mut lv, at, 1);
    ch.item = at;
    if lv.ex_style & LVS_EX_CHECKBOXES != 0 {
        ch.mask |= LVIF_STATE;
        ch.state_mask |= LVIS_STATEIMAGEMASK;
        ch.state = (ch.state & !LVIS_STATEIMAGEMASK) | (1 << 12);
    }
    put(cpu, hwnd, lv.clone());
    let changed = changes_of(&lv, &ch, 0, !0);
    let mut ops = apply_main(cpu, hwnd, &ch, 0, 0, changed, true);
    let mut lv = get(cpu, hwnd)?;
    if lv.is_icon_view() {
        let pos = if lv.style & LVS_ALIGNLEFT != 0 {
            layout::next_icon_pos_left(&mut lv)
        } else {
            layout::next_icon_pos_top(&mut lv)
        };
        if let Some(it) = lv.items.get_mut(at as usize) {
            it.pos = pos;
        }
        put(cpu, hwnd, lv.clone());
        layout::invalidate_item(cpu, hwnd, &lv, at);
    }
    let lparam = lv.items.get(at as usize).map_or(0, |it| it.lparam);
    ops.extend(notify_ops(
        cpu,
        hwnd,
        &lv,
        LVN_INSERTITEM,
        &nmlistview(at, 0, 0, 0, 0, lparam),
    ));
    ops.extend(layout::scroll_on_insert_ops(cpu, hwnd, at, 1));
    run_ops(cpu, hwnd, ops, at as u32)
}

/// LVM_DELETEITEM.
pub(super) fn delete_item(cpu: &mut Cpu, hwnd: u32, lv: &ListView, i: i32) -> Option<u32> {
    if i < 0 || i >= lv.count() {
        return Some(0);
    }
    let ops = vec![
        set_op(&Change::state(i, 0, LVIS_SELECTED | LVIS_FOCUSED)),
        step(T_DELETE, &[i as u32]),
    ];
    run_ops(cpu, hwnd, ops, 1)
}

/// LVM_DELETEALLITEMS, and WM_DESTROY (`destroy`).
pub(super) fn delete_all(cpu: &mut Cpu, hwnd: u32, destroy: bool) -> Option<u32> {
    run_ops(cpu, hwnd, vec![step(T_DELETE_ALL, &[destroy as u32])], 1)
}

/// LVM_SETITEMCOUNT.
pub(super) fn set_item_count(
    cpu: &mut Cpu,
    hwnd: u32,
    mut lv: ListView,
    n: i32,
    flags: u32,
) -> Option<u32> {
    if !lv.ownerdata() {
        return Some(1);
    }
    let old = lv.od_count;
    let mut ops = Vec::new();
    if n < old {
        lv.deselect_range(n, old);
        if lv.focused >= n {
            ops.extend(set_focus_ops(&lv, -1));
            lv.rc_focus = Rect::default();
        }
    }
    lv.od_count = n.max(0);
    put(cpu, hwnd, lv.clone());
    ops.extend(layout::update_scroll_ops(&lv));
    let flags = if lv.is_icon_view() { 0 } else { flags };
    if flags & LVSICF_NOSCROLL == 0 && lv.focused != -1 {
        ops.extend(layout::ensure_visible_ops(lv.focused, false));
    }
    if flags & LVSICF_NOINVALIDATEALL == 0 {
        layout::invalidate_list(cpu, hwnd, &lv);
    } else {
        layout::invalidate_count_change(cpu, hwnd, &lv, old, n);
    }
    run_ops(cpu, hwnd, ops, 1)
}

/// A request of LVM_GETITEM, read from its LVITEM.
#[derive(Clone, Copy, Debug, Default)]
struct Query {
    mask: u32,
    item: i32,
    sub: i32,
    state_mask: u32,
    text: u32,
    max: u32,
}

impl Query {
    fn read(cpu: &mut Cpu, at: u32) -> Option<Query> {
        let w = read_words(cpu, at, 7);
        if cpu.bus.guest_faulted() {
            return None;
        }
        Some(Query {
            mask: w[0],
            item: w[1] as i32,
            sub: w[2] as i32,
            state_mask: w[4],
            text: w[5],
            max: w[6],
        })
    }
}

/// The NMLVDISPINFO's item that asks the parent: mask, state mask,
/// image and indent.
#[derive(Clone, Copy, Debug, Default)]
struct Ask {
    mask: u32,
    state_mask: u32,
    image: i32,
    indent: i32,
}

/// What LVM_GETITEM has to ask the parent with LVN_GETDISPINFO.
fn ask_of(lv: &ListView, q: &Query) -> Ask {
    let mut a = Ask::default();
    if lv.ownerdata() {
        let needed = q.mask & !(LVIF_STATE | LVIF_PARAM) != 0
            || (q.mask & LVIF_STATE != 0 && lv.callback_mask & q.state_mask != 0);
        if !needed {
            return a;
        }
        a.mask = q.mask & !LVIF_PARAM;
        if q.mask & LVIF_TEXT != 0 && q.mask & LVIF_NORECOMPUTE != 0 {
            a.mask &= !(LVIF_TEXT | LVIF_NORECOMPUTE);
        }
        if q.mask & LVIF_STATE != 0 {
            a.state_mask = q.state_mask & lv.callback_mask;
        }
        return a;
    }
    let Some(item) = lv.items.get(q.item as usize) else {
        return a;
    };
    let sub = if q.sub != 0 {
        item.subs.iter().find(|s| s.sub == q.sub)
    } else {
        None
    };
    // An item without the subitem asked for answers as the main item
    // with callbacks for its text and image.
    let isub = if sub.is_some() { q.sub } else { 0 };
    let (image, text) = match sub {
        Some(s) => (s.image, Text::of(&s.text, s.callback)),
        None if q.sub != 0 => (I_IMAGECALLBACK, Text::Callback),
        None => (item.image, Text::of(&item.text, item.callback)),
    };
    if q.mask & LVIF_STATE != 0 && lv.callback_mask != 0 && isub == 0 {
        a.mask |= LVIF_STATE;
        a.state_mask = lv.callback_mask;
    }
    if q.mask & LVIF_IMAGE != 0
        && image == I_IMAGECALLBACK
        && (isub == 0 || lv.ex_style & LVS_EX_SUBITEMIMAGES != 0)
    {
        a.mask |= LVIF_IMAGE;
        a.image = I_IMAGECALLBACK;
    }
    if q.mask & LVIF_INDENT != 0 && item.indent == I_INDENTCALLBACK && isub == 0 {
        a.mask |= LVIF_INDENT;
        a.indent = I_INDENTCALLBACK;
    }
    if q.mask & LVIF_TEXT != 0 && q.mask & LVIF_NORECOMPUTE == 0 && !matches!(text, Text::Some(_)) {
        a.mask |= LVIF_TEXT;
    }
    a
}

/// The length of the zero-ended text at `at`.
fn text_len(cpu: &mut Cpu, at: u32, wide: bool) -> u32 {
    if at == 0 || at == LPSTR_TEXTCALLBACK {
        return 0;
    }
    header::read_text(cpu, at, wide).map_or(0, |t| {
        if wide {
            crate::win32::kernel32::ansi_to_wide(&t).len() as u32
        } else {
            t.len() as u32
        }
    })
}

/// LVM_GETITEM's work on the LVITEM at `at` (LISTVIEW_GetItemExtT),
/// for `mode`. With `scratch`, the script's scratch space, it asks the
/// parent itself; without, it returns None if it has to.
fn get_item(
    cpu: &mut Cpu,
    hwnd: u32,
    at: u32,
    wide: bool,
    mode: u32,
    scratch: Option<u32>,
) -> Result<u32, Vec<Op>> {
    let Some(lv) = get(cpu, hwnd) else {
        return Ok(0);
    };
    let Some(q) = Query::read(cpu, at) else {
        return Ok(0);
    };
    if q.item < 0 || q.item >= lv.count() {
        return Ok(0);
    }
    if q.mask == 0 {
        return Ok(finish_result(cpu, at, wide, mode, true));
    }
    if q.mask == LVIF_STATE && q.state_mask == LVIS_FOCUSED && lv.callback_mask & LVIS_FOCUSED == 0
    {
        let s = if lv.focused == q.item {
            LVIS_FOCUSED
        } else {
            0
        };
        cpu.bus.guest_write_32(at + 12, s);
        return Ok(finish_result(cpu, at, wide, mode, true));
    }
    let ask = ask_of(&lv, &q);
    if ask.mask == 0 {
        fill_item(cpu, hwnd, &lv, at, wide, &q, None);
        return Ok(finish_result(cpu, at, wide, mode, true));
    }
    let Some(scratch) = scratch else {
        return Err(vec![step(T_GET, &[at, wide as u32, mode])]);
    };
    // The parent answers in its own format: the caller's buffer serves
    // when that is the caller's.
    let parent_wide = lv.format == header::NFR_UNICODE;
    let (buf, max, converted) = if ask.mask & LVIF_TEXT == 0 {
        (0, 0, false)
    } else if parent_wide == wide {
        (q.text, q.max, false)
    } else {
        (scratch + S_TEXT2, q.max.min(TEXT_MAX), true)
    };
    if buf != 0 && max > 0 && buf != LPSTR_TEXTCALLBACK {
        if parent_wide {
            cpu.bus.guest_write_16(buf, 0);
        } else {
            cpu.bus.guest_write_8(buf, 0);
        }
    }
    let lparam = if lv.ownerdata() {
        0
    } else {
        lv.items.get(q.item as usize).map_or(0, |it| it.lparam)
    };
    let id = id_of(cpu, hwnd);
    let code = code_for(&lv, LVN_GETDISPINFOW);
    let words = [
        hwnd,
        id,
        code,
        ask.mask,
        q.item as u32,
        q.sub as u32,
        0,
        ask.state_mask,
        buf,
        max,
        ask.image as u32,
        lparam,
        ask.indent as u32,
    ];
    write_words(cpu, scratch + S_NM, &words);
    let msg = user32::message::WM_NOTIFY;
    Err(vec![
        send_op(lv.notify, msg, Arg::V(id), Arg::S(S_NM)),
        step(
            T_GET_DONE,
            &[
                at,
                wide as u32,
                mode,
                buf,
                converted as u32,
                parent_wide as u32,
            ],
        ),
    ])
}

/// What GETITEM returns for `mode`, the item read.
fn finish_result(cpu: &mut Cpu, at: u32, wide: bool, mode: u32, ok: bool) -> u32 {
    if !ok {
        return 0;
    }
    match mode {
        GET_TEXT => {
            let ptr = cpu.bus.guest_read_32(at + 20);
            text_len(cpu, ptr, wide)
        }
        GET_STATE => {
            let w = read_words(cpu, at, 5);
            w[3] & w[4]
        }
        _ => 1,
    }
}

/// The parent's answer to LVN_GETDISPINFO, read back from the
/// NMLVDISPINFO at `nm`.
#[derive(Clone, Copy, Debug, Default)]
struct Answer {
    mask: u32,
    state: u32,
    state_mask: u32,
    text: u32,
    image: i32,
    lparam: u32,
    indent: i32,
}

/// Fill the LVITEM at `at` for the query `q`, with the parent's answer.
fn fill_item(
    cpu: &mut Cpu,
    hwnd: u32,
    lv: &ListView,
    at: u32,
    wide: bool,
    q: &Query,
    ans: Option<Answer>,
) {
    let mut lv = lv.clone();
    let a = ans.unwrap_or_default();
    if lv.ownerdata() {
        if ans.is_some() {
            // The answer's fields, up to the indent unless that was
            // asked for, with the caller's mask.
            write_words(cpu, at + 12, &[a.state, q.state_mask, a.text, q.max]);
            write_words(cpu, at + 28, &[a.image as u32, a.lparam]);
            if q.mask & LVIF_INDENT != 0 {
                cpu.bus.guest_write_32(at + 36, a.indent as u32);
            }
        }
        if q.mask & LVIF_PARAM != 0 {
            cpu.bus.guest_write_32(at + 32, 0);
        }
        if q.mask & LVIF_TEXT != 0 && q.mask & LVIF_NORECOMPUTE != 0 {
            cpu.bus.guest_write_32(at + 20, LPSTR_TEXTCALLBACK);
        }
        if q.mask & LVIF_STATE == 0 || q.sub != 0 {
            return;
        }
        let mut s = if ans.is_some() {
            a.state
        } else {
            cpu.bus.guest_read_32(at + 12)
        };
        if q.state_mask & !lv.callback_mask & LVIS_FOCUSED != 0 {
            s &= !LVIS_FOCUSED;
            if lv.focused == q.item {
                s |= LVIS_FOCUSED;
            }
        }
        if q.state_mask & !lv.callback_mask & LVIS_SELECTED != 0 {
            s &= !LVIS_SELECTED;
            if lv.is_selected(q.item) {
                s |= LVIS_SELECTED;
            }
        }
        cpu.bus.guest_write_32(at + 12, s);
        return;
    }
    let Some(item) = lv.items.get(q.item as usize).cloned() else {
        return;
    };
    let sub_at = if q.sub != 0 {
        item.subs.iter().position(|s| s.sub == q.sub)
    } else {
        None
    };
    let isub = if sub_at.is_some() { q.sub } else { 0 };
    let (image, text) = match sub_at {
        Some(k) => (
            item.subs[k].image,
            Text::of(&item.subs[k].text, item.subs[k].callback),
        ),
        None if q.sub != 0 => (I_IMAGECALLBACK, Text::Callback),
        None => (item.image, Text::of(&item.text, item.callback)),
    };
    let mut amask = a.mask;
    if isub != 0 {
        amask &= !LVIF_DI_SETITEM;
    }
    let set = amask & LVIF_DI_SETITEM != 0;
    // The image.
    if amask & LVIF_IMAGE != 0 {
        cpu.bus.guest_write_32(at + 28, a.image as u32);
        if set && image == I_IMAGECALLBACK {
            store_image(&mut lv, q.item, sub_at, a.image);
        }
    } else if q.mask & LVIF_IMAGE != 0 {
        let v = if isub == 0 || lv.ex_style & LVS_EX_SUBITEMIMAGES != 0 {
            image
        } else {
            0
        };
        cpu.bus.guest_write_32(at + 28, v as u32);
    }
    // The text.
    if amask & LVIF_TEXT != 0 {
        cpu.bus.guest_write_32(at + 20, a.text);
    } else if q.mask & LVIF_TEXT != 0 {
        match &text {
            Text::Some(t) => {
                if q.text != 0 && q.max > 0 {
                    header::write_text(cpu, q.text, q.max, t, wide);
                }
            }
            Text::Null => cpu.bus.guest_write_32(at + 20, 0),
            Text::Callback => cpu.bus.guest_write_32(at + 20, LPSTR_TEXTCALLBACK),
        }
    }
    // The lParam.
    if q.mask & LVIF_PARAM != 0 {
        let v = if amask & LVIF_PARAM != 0 {
            a.lparam
        } else {
            item.lparam
        };
        cpu.bus.guest_write_32(at + 32, v);
    }
    if isub != 0 && q.mask & LVIF_STATE != 0 {
        let state = sub_at.map_or(0, |k| item.subs[k].state);
        cpu.bus.guest_write_32(at + 12, state & q.state_mask);
    }
    if isub == 0 {
        if q.mask & LVIF_STATE != 0 {
            let mut s = item.state & q.state_mask;
            if amask & LVIF_STATE != 0 {
                s &= !a.state_mask;
                s |= a.state & a.state_mask;
            }
            if q.state_mask & !lv.callback_mask & LVIS_FOCUSED != 0 {
                s &= !LVIS_FOCUSED;
                if lv.focused == q.item {
                    s |= LVIS_FOCUSED;
                }
            }
            if q.state_mask & !lv.callback_mask & LVIS_SELECTED != 0 {
                s &= !LVIS_SELECTED;
                if lv.is_selected(q.item) {
                    s |= LVIS_SELECTED;
                }
            }
            cpu.bus.guest_write_32(at + 12, s);
        }
        if amask & LVIF_INDENT != 0 {
            cpu.bus.guest_write_32(at + 36, a.indent as u32);
            if set
                && item.indent == I_INDENTCALLBACK
                && let Some(it) = lv.items.get_mut(q.item as usize)
            {
                it.indent = a.indent;
            }
        } else if q.mask & LVIF_INDENT != 0 {
            cpu.bus.guest_write_32(at + 36, item.indent as u32);
        }
    }
    if set {
        put(cpu, hwnd, lv);
    }
}

fn store_image(lv: &mut ListView, i: i32, sub_at: Option<usize>, image: i32) {
    if let Some(it) = lv.items.get_mut(i as usize) {
        match sub_at {
            Some(k) => it.subs[k].image = image,
            None => it.image = image,
        }
    }
}

/// The rest of LVM_GETITEM once the parent answered: the answer stored
/// in the caller's LVITEM, its text copied to the caller's buffer.
fn get_item_done(cpu: &mut Cpu, c: &mut Ctx) {
    let (at, wide, mode, buf) = (c.word(0), c.word(1) != 0, c.word(2), c.word(3));
    let (converted, parent_wide) = (c.word(4) != 0, c.word(5) != 0);
    let Some(lv) = get(cpu, c.hwnd) else {
        return;
    };
    let Some(q) = Query::read(cpu, at) else {
        return;
    };
    let w = read_words(cpu, c.scratch + S_NM + 12, 10);
    let a = Answer {
        mask: w[0],
        state: w[3],
        state_mask: w[4],
        text: w[5],
        image: w[7] as i32,
        lparam: w[8],
        indent: w[9] as i32,
    };
    // Where the text is, in the caller's format.
    let mut answer = a;
    if a.mask & LVIF_TEXT != 0
        && a.text != LPSTR_TEXTCALLBACK
        && a.text != 0
        && (converted || a.text != buf || a.text != q.text)
    {
        let text = header::read_text(cpu, a.text, parent_wide).unwrap_or_default();
        if q.text != 0 && q.text != LPSTR_TEXTCALLBACK && q.max > 0 {
            header::write_text(cpu, q.text, q.max, &text, wide);
        }
        answer.text = q.text;
    }
    let lv_mask_text = a.mask & LVIF_TEXT != 0;
    fill_item(cpu, c.hwnd, &lv, at, wide, &q, Some(answer));
    // LISTVIEW_GetItemExtT: the caller's buffer, unless the parent made
    // the text a callback.
    if lv_mask_text && answer.text != LPSTR_TEXTCALLBACK {
        cpu.bus.guest_write_32(at + 20, q.text);
    }
    if lv.ownerdata() {
        cpu.bus.guest_write_32(at, q.mask);
    }
    c.result = finish_result(cpu, at, wide, mode, true);
}

/// LVM_GETITEMA and LVM_GETITEMW.
pub(super) fn get_item_msg(cpu: &mut Cpu, hwnd: u32, lp: u32, wide: bool) -> Option<u32> {
    if lp == 0 {
        return Some(0);
    }
    match get_item(cpu, hwnd, lp, wide, GET_ITEM, None) {
        Ok(r) => Some(r),
        Err(ops) => run_ops(cpu, hwnd, ops, 0),
    }
}

/// LVM_GETITEMTEXTA and LVM_GETITEMTEXTW: the length of the text.
pub(super) fn get_item_text_msg(
    cpu: &mut Cpu,
    hwnd: u32,
    lv: &ListView,
    i: i32,
    lp: u32,
    wide: bool,
) -> Option<u32> {
    if lp == 0 || i < 0 || i >= lv.count() {
        return Some(0);
    }
    write_words(cpu, lp, &[LVIF_TEXT, i as u32]);
    if cpu.bus.guest_faulted() {
        return None;
    }
    match get_item(cpu, hwnd, lp, wide, GET_TEXT, None) {
        Ok(r) => Some(r),
        Err(ops) => run_ops(cpu, hwnd, ops, 0),
    }
}

/// LVM_GETITEMSTATE.
pub(super) fn get_item_state_msg(
    cpu: &mut Cpu,
    hwnd: u32,
    lv: &ListView,
    i: i32,
    mask: u32,
) -> Option<u32> {
    if i < 0 || i >= lv.count() {
        return Some(0);
    }
    if lv.callback_mask & mask == 0 || (mask == LVIS_FOCUSED) {
        return Some(item_state(lv, i) & mask);
    }
    run_ops(cpu, hwnd, vec![step(T_GET_STATE, &[i as u32, mask])], 0)
}

/// The steps that read item `i`'s subitem `sub` into the LVITEM at
/// `S_ITEM` of the scratch space, with its text at `S_TEXT`, asking the
/// parent what it gives (for drawing): `mask` says what to read.
pub(super) fn read_own_ops(
    cpu: &mut Cpu,
    c: &Ctx,
    i: i32,
    sub: i32,
    mask: u32,
    state_mask: u32,
) -> Vec<Op> {
    let at = c.scratch + S_ITEM;
    write_words(
        cpu,
        at,
        &[
            mask,
            i as u32,
            sub as u32,
            0,
            state_mask,
            c.scratch + S_TEXT,
            TEXT_MAX,
            0,
            0,
            0,
        ],
    );
    cpu.bus.guest_write_8(c.scratch + S_TEXT, 0);
    match get_item(cpu, c.hwnd, at, false, GET_OWN, Some(c.scratch)) {
        Ok(_) => Vec::new(),
        Err(ops) => ops,
    }
}

/// What LVM_GETITEM gave at `S_ITEM` for the control's own use: the
/// state, the image, the lParam, the indent and the text.
#[derive(Clone, Debug, Default)]
pub(super) struct Got {
    pub state: u32,
    pub image: i32,
    pub lparam: u32,
    pub indent: i32,
    pub text: Option<Vec<u8>>,
}

pub(super) fn read_own(cpu: &mut Cpu, scratch: u32) -> Got {
    let w = read_words(cpu, scratch + S_ITEM, 10);
    let text = match w[5] {
        0 => None,
        LPSTR_TEXTCALLBACK => Some(b"(callback)".to_vec()),
        p => header::read_text(cpu, p, false),
    };
    Got {
        state: w[3],
        image: w[7] as i32,
        lparam: w[8],
        indent: w[9] as i32,
        text,
    }
}

/// LVM_GETNEXTITEM.
pub(super) fn next_item(cpu: &mut Cpu, hwnd: u32, lv: &ListView, start: i32, flags: u32) -> i32 {
    let count = lv.count();
    if start < -1 || start >= count {
        return -1;
    }
    let mut mask = 0;
    if flags & LVNI_CUT != 0 {
        mask |= LVIS_CUT;
    }
    if flags & LVNI_DROPHILITED != 0 {
        mask |= LVIS_DROPHILITED;
    }
    if flags & LVNI_FOCUSED != 0 {
        mask |= LVIS_FOCUSED;
    }
    if flags & LVNI_SELECTED != 0 {
        mask |= LVIS_SELECTED;
    }
    let has = |i: i32| i >= 0 && i < count && item_state(lv, i) & mask == mask;
    if flags & LVNI_FOCUSED != 0 {
        if !has(lv.focused) {
            return -1;
        }
        return if lv.focused == start { -1 } else { lv.focused };
    }
    let mut n = start;
    let list_or_report = lv.view == LV_VIEW_LIST || lv.view == LV_VIEW_DETAILS;
    let nearest = |cpu: &mut Cpu, n: i32, vk: u32| -> Vec<i32> {
        let mut found = Vec::new();
        let mut from = n;
        let pt = layout::item_position(cpu, hwnd, lv, n);
        loop {
            let next = find_nearest(cpu, hwnd, lv, from, pt, vk);
            if next == -1 || found.contains(&next) {
                break;
            }
            found.push(next);
            from = next;
        }
        found
    };
    if flags & LVNI_ABOVE != 0 {
        if list_or_report {
            while n >= 0 {
                n -= 1;
                if has(n) {
                    return n;
                }
            }
        } else if lv.auto_arranged() {
            let per_row = layout::count_per_row(lv);
            while n - per_row >= 0 {
                n -= per_row;
                if has(n) {
                    return n;
                }
            }
            return -1;
        } else {
            return nearest(cpu, n, VK_UP)
                .into_iter()
                .find(|&i| has(i))
                .unwrap_or(-1);
        }
    } else if flags & LVNI_BELOW != 0 {
        if list_or_report {
            while n < count {
                n += 1;
                if has(n) {
                    return n;
                }
            }
        } else if lv.auto_arranged() {
            let per_row = layout::count_per_row(lv);
            while n + per_row < count {
                n += per_row;
                if has(n) {
                    return n;
                }
            }
            return -1;
        } else {
            return nearest(cpu, n, VK_DOWN)
                .into_iter()
                .find(|&i| has(i))
                .unwrap_or(-1);
        }
    } else if flags & LVNI_TOLEFT != 0 {
        if lv.view == LV_VIEW_LIST {
            let per_col = layout::count_per_column(lv);
            while n - per_col >= 0 {
                n -= per_col;
                if has(n) {
                    return n;
                }
            }
        } else if lv.is_icon_view() {
            if lv.auto_arranged() {
                let per_row = layout::count_per_row(lv);
                while n % per_row > 0 {
                    n -= 1;
                    if has(n) {
                        return n;
                    }
                }
                return -1;
            }
            return nearest(cpu, n, VK_LEFT)
                .into_iter()
                .find(|&i| has(i))
                .unwrap_or(-1);
        }
    } else if flags & LVNI_TORIGHT != 0 {
        if lv.view == LV_VIEW_LIST {
            let per_col = layout::count_per_column(lv);
            while n + per_col < count {
                n += per_col;
                if has(n) {
                    return n;
                }
            }
        } else if lv.is_icon_view() {
            if lv.auto_arranged() {
                let per_row = layout::count_per_row(lv);
                while n % per_row < per_row - 1 {
                    n += 1;
                    if has(n) {
                        return n;
                    }
                }
                return -1;
            }
            return nearest(cpu, n, VK_RIGHT)
                .into_iter()
                .find(|&i| has(i))
                .unwrap_or(-1);
        }
    } else {
        return (start + 1..count).find(|&i| has(i)).unwrap_or(-1);
    }
    -1
}

/// LVFI_NEARESTXY's search from item `start`: the item nearest the
/// point `pt` moved a step in the direction `vk`.
fn find_nearest(
    cpu: &mut Cpu,
    hwnd: u32,
    lv: &ListView,
    start: i32,
    pt: (i32, i32),
    vk: u32,
) -> i32 {
    let fi = Find {
        flags: LVFI_NEARESTXY,
        pt,
        vk,
        ..Find::default()
    };
    find(cpu, hwnd, lv, start, &fi)
}

/// An LVFINDINFO.
#[derive(Clone, Debug, Default)]
struct Find {
    flags: u32,
    text: Vec<u8>,
    lparam: u32,
    pt: (i32, i32),
    vk: u32,
}

/// LISTVIEW_FindItemW for the control's own items.
fn find(cpu: &mut Cpu, hwnd: u32, lv: &ListView, start: i32, fi: &Find) -> i32 {
    let count = lv.count();
    let mut n = start + 1;
    if n < 0 {
        return -1;
    }
    let mut want_text = fi.flags & (LVFI_STRING | LVFI_PARTIAL | LVFI_SUBSTRING) != 0;
    let mut wrap = fi.flags & LVFI_WRAP != 0;
    let mut nearest = false;
    let mut dest = (0, 0);
    if fi.flags & LVFI_NEARESTXY != 0 && lv.is_icon_view() {
        let origin = layout::origin(cpu, hwnd, lv);
        dest = (fi.pt.0 - origin.0, fi.pt.1 - origin.1);
        let list_h = lv.list.height();
        match fi.vk {
            VK_DOWN => dest.1 += lv.item_height,
            VK_UP => dest.1 -= lv.item_height,
            VK_RIGHT => dest.0 += lv.item_width,
            VK_LEFT => dest.0 -= lv.item_width,
            VK_HOME => dest = (0, 0),
            VK_NEXT => dest.1 += list_h,
            VK_PRIOR => dest.1 -= list_h,
            VK_END => {
                let area = layout::area_rect(lv);
                dest = (area.right, area.bottom);
            }
            _ => {}
        }
        nearest = true;
    }
    let by_param = fi.flags & LVFI_PARAM != 0;
    if by_param {
        nearest = false;
        want_text = false;
    }
    let mut last = count;
    let mut best = -1;
    let mut min = u32::MAX >> 1;
    loop {
        while n < last {
            let i = n;
            n += 1;
            let Some(item) = lv.items.get(i as usize) else {
                continue;
            };
            if by_param {
                if item.lparam == fi.lparam {
                    return i;
                }
                continue;
            }
            if want_text {
                let text = item.text.clone().unwrap_or_default();
                let lower = |s: &[u8]| {
                    s.iter()
                        .map(|c| c.to_ascii_lowercase())
                        .collect::<Vec<u8>>()
                };
                let (t, f) = (lower(&text), lower(&fi.text));
                if fi.flags & (LVFI_PARTIAL | LVFI_SUBSTRING) != 0 {
                    if !t.starts_with(&f) {
                        continue;
                    }
                } else if t != f {
                    continue;
                }
            }
            if !nearest {
                return i;
            }
            let p = item.pos;
            let dx = dest.0.wrapping_sub(p.0) as u32;
            let dy = dest.1.wrapping_sub(p.1) as u32;
            let dist = dx.wrapping_mul(dx).wrapping_add(dy.wrapping_mul(dy));
            if dist < min {
                min = dist;
                best = i;
            }
        }
        if !wrap {
            break;
        }
        n = 0;
        last = (start + 1).min(count);
        wrap = false;
    }
    best
}

/// LVM_FINDITEMA and LVM_FINDITEMW.
pub(super) fn find_item(
    cpu: &mut Cpu,
    hwnd: u32,
    lv: &ListView,
    start: i32,
    lp: u32,
    wide: bool,
) -> Option<u32> {
    if lp == 0 {
        return Some(-1i32 as u32);
    }
    let w = read_words(cpu, lp, 6);
    if cpu.bus.guest_faulted() {
        return None;
    }
    if lv.ownerdata() {
        let mut more = vec![start as u32];
        more.extend_from_slice(&w);
        let code = code_for(lv, LVN_ODFINDITEMW);
        let mut ops = notify_ops(cpu, hwnd, lv, code, &more);
        ops.push(Op::Return(Arg::Last));
        return run_ops(cpu, hwnd, ops, -1i32 as u32);
    }
    let flags = w[0];
    // COMCTL32 4.7x has no LVFI_SUBSTRING.
    if flags & LVFI_SUBSTRING != 0 && flags & (LVFI_STRING | LVFI_PARTIAL | LVFI_PARAM) == 0 {
        return Some(-1i32 as u32);
    }
    let text = if flags & (LVFI_STRING | LVFI_PARTIAL | LVFI_SUBSTRING) != 0 && w[1] != 0 {
        header::read_text(cpu, w[1], wide)?
    } else {
        Vec::new()
    };
    let fi = Find {
        flags,
        text,
        lparam: w[2],
        pt: (w[3] as i32, w[4] as i32),
        vk: w[5],
    };
    Some(find(cpu, hwnd, lv, start, &fi) as u32)
}

/// LVM_GETSELECTEDCOUNT.
pub(super) fn selected_count(lv: &ListView) -> u32 {
    if lv.callback_mask & LVIS_SELECTED != 0 {
        return (0..lv.count())
            .filter(|&i| item_state(lv, i) & LVIS_SELECTED != 0)
            .count() as u32;
    }
    lv.selected_count() as u32
}

/// LVM_SORTITEMS: a binary insertion sort, each comparison a call of
/// the program's function.
pub(super) fn sort_items(
    cpu: &mut Cpu,
    hwnd: u32,
    mut lv: ListView,
    compare: u32,
    lparam: u32,
) -> Option<u32> {
    if lv.ownerdata() || compare == 0 {
        return Some(0);
    }
    if lv.count() < 2 {
        return Some(1);
    }
    lv.sorting = vec![0];
    lv.sort_compare = compare;
    lv.sort_lparam = lparam;
    let n = lv.count() as u32;
    put(cpu, hwnd, lv);
    run_ops(cpu, hwnd, vec![step(T_SORT_STEP, &[1, 0, 1, n])], 1)
}

/// The step of the sort placing item `k` in the sorted `lo..hi`.
fn sort_step(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let (k, lo, hi, n) = (c.word(0), c.word(1), c.word(2), c.word(3));
    let Some(mut lv) = get(cpu, c.hwnd) else {
        return Vec::new();
    };
    if lv.count() as u32 != n {
        lv.sorting.clear();
        put(cpu, c.hwnd, lv);
        return Vec::new();
    }
    if lo < hi {
        let mid = (lo + hi) / 2;
        let other = lv.sorting[mid as usize];
        let a = lv.items[k as usize].lparam;
        let b = lv.items[other as usize].lparam;
        return vec![
            Op::Call {
                target: lv.sort_compare,
                args: vec![Arg::V(a), Arg::V(b), Arg::V(lv.sort_lparam)],
            },
            step(T_SORT_CMP, &[k, lo, hi, n, mid]),
        ];
    }
    lv.sorting.insert(lo as usize, k);
    if k + 1 < n {
        let len = lv.sorting.len() as u32;
        put(cpu, c.hwnd, lv);
        return vec![step(T_SORT_STEP, &[k + 1, 0, len, n])];
    }
    // Sorted: the items in their new order, the selection rebuilt from
    // their states, the mark and the focus following their items.
    let order = std::mem::take(&mut lv.sorting);
    let old = std::mem::take(&mut lv.items);
    let place = |i: i32| {
        order
            .iter()
            .position(|&o| o as i32 == i)
            .map_or(-1, |p| p as i32)
    };
    lv.mark = if lv.mark >= 0 { place(lv.mark) } else { -1 };
    lv.focused = if lv.focused >= 0 {
        place(lv.focused)
    } else {
        -1
    };
    lv.items = order.iter().map(|&o| old[o as usize].clone()).collect();
    lv.selection.clear();
    for i in 0..lv.items.len() {
        if lv.items[i].state & LVIS_SELECTED != 0 {
            lv.select_range(i as i32, i as i32 + 1);
        }
    }
    put(cpu, c.hwnd, lv.clone());
    if !lv.is_icon_view() {
        layout::invalidate_list(cpu, c.hwnd, &lv);
    }
    Vec::new()
}

/// The continuations.
pub(super) fn then(cpu: &mut Cpu, id: u16, c: &mut Ctx) -> Vec<Op> {
    match id {
        T_SET => {
            let (ch, _) = Change::decode(&c.data);
            let (ok, ops) = set_item_ops(cpu, c.hwnd, &ch);
            if !ok && ch.counts {
                c.result = 0;
            }
            ops
        }
        T_SET_APPLY => {
            let (ch, used) = Change::decode(&c.data);
            let (old, old_lparam, changed) = (c.word(used), c.word(used + 1), c.word(used + 2));
            if c.last != 0 {
                if ch.counts {
                    c.result = 0;
                }
                return Vec::new();
            }
            apply_main(cpu, c.hwnd, &ch, old, old_lparam, changed, false)
        }
        T_SEL_ADD => {
            let i = c.word(0) as i32;
            if let Some(mut lv) = get(cpu, c.hwnd)
                && i >= 0
                && i < lv.count()
            {
                lv.select_range(i, i + 1);
                put(cpu, c.hwnd, lv);
            }
            Vec::new()
        }
        T_FOCUS => {
            let i = c.word(0) as i32;
            let Some(mut lv) = get(cpu, c.hwnd) else {
                return Vec::new();
            };
            if i < 0 || i >= lv.count() {
                return Vec::new();
            }
            lv.focused = i;
            put(cpu, c.hwnd, lv.clone());
            layout::ensure_visible_ops(i, lv.view == LV_VIEW_LIST)
        }
        T_SET_MARK => {
            if let Some(mut lv) = get(cpu, c.hwnd) {
                lv.mark = c.word(0) as i32;
                put(cpu, c.hwnd, lv);
            }
            Vec::new()
        }
        T_GET => {
            let (at, wide, mode) = (c.word(0), c.word(1) != 0, c.word(2));
            match get_item(cpu, c.hwnd, at, wide, mode, Some(c.scratch)) {
                Ok(r) => {
                    if mode != GET_OWN {
                        c.result = r;
                    }
                    Vec::new()
                }
                Err(ops) => ops,
            }
        }
        T_GET_DONE => {
            let mode = c.word(2);
            let keep = c.result;
            get_item_done(cpu, c);
            if mode == GET_OWN {
                c.result = keep;
            }
            Vec::new()
        }
        T_GET_STATE => {
            let (i, mask) = (c.word(0), c.word(1));
            let at = c.scratch + S_ITEM;
            write_words(cpu, at, &[LVIF_STATE, i, 0, 0, mask, 0, 0, 0, 0, 0]);
            match get_item(cpu, c.hwnd, at, false, GET_STATE, Some(c.scratch)) {
                Ok(r) => {
                    c.result = r;
                    Vec::new()
                }
                Err(ops) => ops,
            }
        }
        T_OD_TEXT => {
            let i = c.word(0) as i32;
            let Some(lv) = get(cpu, c.hwnd) else {
                return Vec::new();
            };
            if i < 0 || i >= lv.count() {
                return Vec::new();
            }
            let id = id_of(cpu, c.hwnd);
            let code = code_for(&lv, LVN_GETDISPINFOW);
            let buf = c.scratch + S_TEXT;
            cpu.bus.guest_write_16(buf, 0);
            let words = [
                c.hwnd, id, code, LVIF_TEXT, i as u32, 0, 0, 0, buf, TEXT_MAX, 0, 0, 0,
            ];
            write_words(cpu, c.scratch + S_NM, &words);
            let msg = user32::message::WM_NOTIFY;
            vec![send_op(lv.notify, msg, Arg::V(id), Arg::S(S_NM))]
        }
        T_DELETE => {
            let i = c.word(0) as i32;
            let Some(lv) = get(cpu, c.hwnd) else {
                return Vec::new();
            };
            if i < 0 || i >= lv.count() {
                c.result = 0;
                return Vec::new();
            }
            let lparam = lv.items.get(i as usize).map_or(0, |it| it.lparam);
            let nm = nmlistview(i, 0, 0, 0, 0, lparam);
            let mut ops = notify_ops(cpu, c.hwnd, &lv, LVN_DELETEITEM, &nm);
            ops.push(step(T_DELETE_DONE, &[i as u32]));
            ops
        }
        T_DELETE_DONE => {
            let i = c.word(0) as i32;
            let Some(mut lv) = get(cpu, c.hwnd) else {
                return Vec::new();
            };
            if i < 0 || i >= lv.count() {
                return Vec::new();
            }
            let is_icon = lv.is_icon_view();
            if is_icon {
                layout::invalidate_item(cpu, c.hwnd, &lv, i);
            }
            if lv.ownerdata() {
                lv.od_count -= 1;
            } else {
                lv.items.remove(i as usize);
            }
            shift_indices(&mut lv, i, -1);
            put(cpu, c.hwnd, lv);
            if is_icon {
                return Vec::new();
            }
            layout::scroll_on_insert_ops(cpu, c.hwnd, i, -1)
        }
        T_DELETE_ALL => {
            let Some(mut lv) = get(cpu, c.hwnd) else {
                return Vec::new();
            };
            lv.selection.clear();
            lv.mark = -1;
            lv.focused = -1;
            lv.rc_focus = Rect::default();
            put(cpu, c.hwnd, lv.clone());
            let nm = nmlistview(-1, 0, 0, 0, 0, 0);
            let mut ops = notify_ops(cpu, c.hwnd, &lv, LVN_DELETEALLITEMS, &nm);
            ops.push(step(T_DELETE_ALL_ITEMS, &[c.word(0)]));
            ops
        }
        T_DELETE_ALL_ITEMS => {
            let suppress = c.last != 0;
            let Some(lv) = get(cpu, c.hwnd) else {
                return Vec::new();
            };
            let mut ops = Vec::new();
            for i in (0..lv.count()).rev() {
                if !lv.ownerdata() && !suppress {
                    ops.push(step(T_DELETE_ALL_ITEM, &[i as u32]));
                }
                ops.push(step(T_DELETE_ALL_DROP, &[i as u32]));
            }
            ops.push(step(T_DELETE_ALL_END, &[c.word(0)]));
            ops
        }
        T_DELETE_ALL_ITEM => {
            let i = c.word(0) as i32;
            let Some(lv) = get(cpu, c.hwnd) else {
                return Vec::new();
            };
            let Some(item) = lv.items.get(i as usize) else {
                return Vec::new();
            };
            let nm = nmlistview(i, 0, 0, 0, 0, item.lparam);
            notify_ops(cpu, c.hwnd, &lv, LVN_DELETEITEM, &nm)
        }
        T_DELETE_ALL_DROP => {
            let i = c.word(0) as usize;
            if let Some(mut lv) = get(cpu, c.hwnd) {
                if lv.ownerdata() {
                    lv.od_count = lv.od_count.min(i as i32);
                } else {
                    lv.items.truncate(i);
                }
                put(cpu, c.hwnd, lv);
            }
            Vec::new()
        }
        T_DELETE_ALL_END => {
            let destroy = c.word(0) != 0;
            let Some(mut lv) = get(cpu, c.hwnd) else {
                return Vec::new();
            };
            lv.items.clear();
            lv.od_count = 0;
            let mut ops = Vec::new();
            if !destroy {
                layout::arrange(cpu, c.hwnd, &mut lv, 0);
                put(cpu, c.hwnd, lv.clone());
                ops.extend(layout::update_scroll_ops(&lv));
            } else {
                put(cpu, c.hwnd, lv.clone());
            }
            layout::invalidate_list(cpu, c.hwnd, &lv);
            ops
        }
        T_SORT_STEP => sort_step(cpu, c),
        T_SORT_CMP => {
            let (k, lo, hi, n, mid) = (c.word(0), c.word(1), c.word(2), c.word(3), c.word(4));
            let (lo, hi) = if (c.last as i32) < 0 {
                (lo, mid)
            } else {
                (mid + 1, hi)
            };
            vec![step(T_SORT_STEP, &[k, lo, hi, n])]
        }
        _ => Vec::new(),
    }
}
