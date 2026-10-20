//! The ListBox and ComboLBox classes, after Wine's listbox.c, as Windows
//! 95 has them, with the ANSI messages.
//!
//! As in the edit control, a message does its work on the state at once
//! and leaves what it then calls in `Calls`. A change invalidates the
//! window, and WM_PAINT draws all of it; an owner-drawn list asks its
//! owner for each item in the WM_PAINT task. A sorted owner-drawn list
//! without strings finds an item's place with WM_COMPAREITEM in a task of
//! its own, and LBS_WANTKEYBOARDINPUT asks the owner about a key in one.
//!
//! A ComboLBox is the list of a combo box: the combo box is its owner,
//! and the two use each other's state.

use super::super::defwnd::def_window_proc;
use super::super::message::{
    WM_CHAR, WM_CHARTOITEM, WM_COMMAND, WM_COMPAREITEM, WM_CREATE, WM_CTLCOLORLISTBOX,
    WM_DELETEITEM, WM_DESTROY, WM_DRAWITEM, WM_ENABLE, WM_ERASEBKGND, WM_GETDLGCODE, WM_GETFONT,
    WM_HSCROLL, WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MEASUREITEM, WM_MOUSEMOVE, WM_NCACTIVATE, WM_NCDESTROY, WM_NCLBUTTONDOWN, WM_PAINT,
    WM_PRINTCLIENT, WM_SETFOCUS, WM_SETFONT, WM_SETREDRAW, WM_SIZE, WM_VKEYTOITEM, WM_VSCROLL,
};
use super::super::sysparams::{
    self, COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_WINDOW,
};
use super::super::window::{
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, WS_DISABLED, WS_HSCROLL, WS_VISIBLE, WS_VSCROLL,
};
use super::super::{bytes_at, make_long, paint, read_words, text, user, write_words};
use super::combobox::{
    self, CBF_DROPPED, CBF_EUI, CBF_NOROLLUP, CBF_SELCHANGE, CBS_DROPDOWNLIST, CBS_SIMPLE,
};
use super::edit::{
    Calls, DLGC_WANTARROWS, DLGC_WANTCHARS, K_CALLS, K_PAINT, MK_CONTROL, MK_SHIFT, P_BRUSH, P_HDC,
    P_HWND, P_OWN, SB_BOTTOM, SB_HORZ, SB_LINEDOWN, SB_LINEUP, SB_PAGEDOWN, SB_PAGEUP,
    SB_THUMBPOSITION, SB_THUMBTRACK, SB_TOP, SB_VERT, SCRATCH, SCRATCH_DATA, SIF_DISABLENOSCROLL,
    SIF_PAGE, SIF_POS, SIF_RANGE, VK_DOWN, VK_END, VK_F4, VK_HOME, VK_LEFT, VK_NEXT, VK_PRIOR,
    VK_RIGHT, VK_SHIFT, VK_SPACE, VK_UP, calls_step, client_of, default_brush, key_down, measure,
    ops_push, ops_step, paint_end, paint_head, paint_start, run, select_font,
};
use super::{LISTBOX, start};
use crate::cpu::Cpu;
use crate::win32::gdi32::{Rect, dc, font};
use crate::win32::task::{Next, Task};
use std::cmp::Ordering;

// The styles.
pub const LBS_NOTIFY: u32 = 0x0001;
pub const LBS_SORT: u32 = 0x0002;
pub const LBS_NOREDRAW: u32 = 0x0004;
pub const LBS_MULTIPLESEL: u32 = 0x0008;
pub const LBS_OWNERDRAWFIXED: u32 = 0x0010;
pub const LBS_OWNERDRAWVARIABLE: u32 = 0x0020;
pub const LBS_HASSTRINGS: u32 = 0x0040;
pub const LBS_USETABSTOPS: u32 = 0x0080;
pub const LBS_NOINTEGRALHEIGHT: u32 = 0x0100;
pub const LBS_MULTICOLUMN: u32 = 0x0200;
pub const LBS_WANTKEYBOARDINPUT: u32 = 0x0400;
pub const LBS_EXTENDEDSEL: u32 = 0x0800;
pub const LBS_DISABLENOSCROLL: u32 = 0x1000;
pub const LBS_NOSEL: u32 = 0x4000;
pub const LBS_COMBOBOX: u32 = 0x8000;

// The messages.
pub const LB_ADDSTRING: u32 = 0x0180;
pub const LB_INSERTSTRING: u32 = 0x0181;
pub const LB_DELETESTRING: u32 = 0x0182;
pub const LB_SELITEMRANGEEX: u32 = 0x0183;
pub const LB_RESETCONTENT: u32 = 0x0184;
pub const LB_SETSEL: u32 = 0x0185;
pub const LB_SETCURSEL: u32 = 0x0186;
pub const LB_GETSEL: u32 = 0x0187;
pub const LB_GETCURSEL: u32 = 0x0188;
pub const LB_GETTEXT: u32 = 0x0189;
pub const LB_GETTEXTLEN: u32 = 0x018A;
pub const LB_GETCOUNT: u32 = 0x018B;
pub const LB_SELECTSTRING: u32 = 0x018C;
pub const LB_DIR: u32 = 0x018D;
pub const LB_GETTOPINDEX: u32 = 0x018E;
pub const LB_FINDSTRING: u32 = 0x018F;
pub const LB_GETSELCOUNT: u32 = 0x0190;
pub const LB_GETSELITEMS: u32 = 0x0191;
pub const LB_SETTABSTOPS: u32 = 0x0192;
pub const LB_GETHORIZONTALEXTENT: u32 = 0x0193;
pub const LB_SETHORIZONTALEXTENT: u32 = 0x0194;
pub const LB_SETCOLUMNWIDTH: u32 = 0x0195;
pub const LB_ADDFILE: u32 = 0x0196;
pub const LB_SETTOPINDEX: u32 = 0x0197;
pub const LB_GETITEMRECT: u32 = 0x0198;
pub const LB_GETITEMDATA: u32 = 0x0199;
pub const LB_SETITEMDATA: u32 = 0x019A;
pub const LB_SELITEMRANGE: u32 = 0x019B;
pub const LB_SETANCHORINDEX: u32 = 0x019C;
pub const LB_GETANCHORINDEX: u32 = 0x019D;
pub const LB_SETCARETINDEX: u32 = 0x019E;
pub const LB_GETCARETINDEX: u32 = 0x019F;
pub const LB_SETITEMHEIGHT: u32 = 0x01A0;
pub const LB_GETITEMHEIGHT: u32 = 0x01A1;
pub const LB_FINDSTRINGEXACT: u32 = 0x01A2;
pub const LB_CARETON: u32 = 0x01A3;
pub const LB_CARETOFF: u32 = 0x01A4;
pub const LB_SETLOCALE: u32 = 0x01A5;
pub const LB_GETLOCALE: u32 = 0x01A6;
pub const LB_SETCOUNT: u32 = 0x01A7;
pub const LB_INITSTORAGE: u32 = 0x01A8;
pub const LB_ITEMFROMPOINT: u32 = 0x01A9;

// The notifications.
pub const LBN_ERRSPACE: u32 = 0xFFFE;
pub const LBN_SELCHANGE: u32 = 1;
pub const LBN_DBLCLK: u32 = 2;
pub const LBN_SELCANCEL: u32 = 3;
pub const LBN_SETFOCUS: u32 = 4;
pub const LBN_KILLFOCUS: u32 = 5;

const LB_OKAY: i32 = 0;
const LB_ERR: i32 = -1;

const WM_LBTRACKPOINT: u32 = 0x0131;
const HTHSCROLL: u32 = 6;
const HTVSCROLL: u32 = 7;
const SM_CXVSCROLL: u32 = 2;
const SM_CYHSCROLL: u32 = 3;

// The owner-draw structures.
const ODT_LISTBOX: u32 = 2;
const ODA_DRAWENTIRE: u32 = 1;
const ODA_FOCUS: u32 = 4;
const ODS_SELECTED: u32 = 0x0001;
const ODS_DISABLED: u32 = 0x0004;
const ODS_FOCUS: u32 = 0x0010;

// What the list keeps besides its style.
/// WM_CREATE has been, and WM_DESTROY not yet.
const LF_CREATED: u32 = 0x01;
/// The focus rectangle shows (a combo box's list has it when dropped).
const LF_CARET_ON: u32 = 0x02;
const LF_IN_FOCUS: u32 = 0x04;
/// The mouse button went down in the list and is tracked.
const LF_CAPTURED: u32 = 0x08;
/// It changed under WM_SETREDRAW FALSE.
const LF_CHANGED: u32 = 0x10;

// The task kinds besides the shared ones.
const K_SORT: u32 = 2;
const K_KEY: u32 = 3;

/// An item.
#[derive(Clone, Debug, Default)]
pub struct Item {
    pub text: Vec<u8>,
    pub data: u32,
    pub selected: bool,
    /// The height of an item of an LBS_OWNERDRAWVARIABLE list.
    pub height: i32,
}

/// What a list box keeps.
#[derive(Clone, Debug, Default)]
pub struct ListBox {
    /// The style as the list has it: CreateWindow's, fixed up.
    pub style: u32,
    /// The window the notifications go to.
    pub owner: u32,
    /// The combo box of a ComboLBox, or 0.
    pub combo: u32,
    pub flags: u32,
    pub items: Vec<Item>,
    pub top: i32,
    pub selected: i32,
    pub focus: i32,
    pub anchor: i32,
    pub item_height: i32,
    pub page: i32,
    pub column_width: i32,
    /// The horizontal extent, 0 without WS_HSCROLL, and the scroll.
    pub horz_extent: i32,
    pub horz_pos: i32,
    /// LB_SETTABSTOPS's stops, in pixels.
    pub tabs: Vec<i32>,
    pub avg_char_width: i32,
    pub font: u32,
    pub locale: u32,
    /// The client area's size.
    pub width: i32,
    pub height: i32,
    /// The scroll bars' last range, page and position sent.
    pub bars: Vec<i32>,
}

impl ListBox {
    fn owner_draw(&self) -> bool {
        self.style & (LBS_OWNERDRAWFIXED | LBS_OWNERDRAWVARIABLE) != 0
    }

    fn has_strings(&self) -> bool {
        !self.owner_draw() || self.style & LBS_HASSTRINGS != 0
    }

    fn multi_select(&self) -> bool {
        self.style & (LBS_MULTIPLESEL | LBS_EXTENDEDSEL) != 0 && self.style & LBS_NOSEL == 0
    }

    fn count(&self) -> i32 {
        self.items.len() as i32
    }

    fn caret_shows(&self) -> bool {
        self.flags & (LF_CARET_ON | LF_IN_FOCUS) == LF_CARET_ON | LF_IN_FOCUS
    }

    fn current_page_size(&self) -> i32 {
        if self.style & LBS_OWNERDRAWVARIABLE == 0 {
            return self.page;
        }
        let mut height = 0;
        let mut i = self.top;
        while i < self.count() {
            height += self.items[i as usize].height;
            if height > self.height {
                break;
            }
            i += 1;
        }
        if i == self.top { 1 } else { i - self.top }
    }

    fn max_top_index(&self) -> i32 {
        let n = self.count();
        let max = if self.style & LBS_OWNERDRAWVARIABLE != 0 {
            let mut page = self.height;
            let mut max = n - 1;
            while max >= 0 {
                page -= self.items[max as usize].height;
                if page < 0 {
                    break;
                }
                max -= 1;
            }
            if max < n - 1 { max + 1 } else { max }
        } else if self.style & LBS_MULTICOLUMN != 0 {
            let page = (self.width / self.column_width.max(1)).max(1);
            let cols = (n + self.page - 1) / self.page.max(1);
            (cols - page) * self.page
        } else {
            n - self.page
        };
        max.max(0)
    }

    /// The rectangle of an item in client coordinates, and whether it
    /// shows: 1 if it does, 0 if not, -1 for no such item.
    fn item_rect(&self, index: i32) -> (i32, Rect) {
        if index != 0 && index >= self.count() {
            return (LB_ERR, Rect::default());
        }
        let mut r = Rect::new(0, 0, self.width, self.height);
        if self.style & LBS_MULTICOLUMN != 0 {
            let page = self.page.max(1);
            let col = index / page - self.top / page;
            r.left += col * self.column_width;
            r.right = r.left + self.column_width;
            r.top += index % page * self.item_height;
            r.bottom = r.top + self.item_height;
        } else if self.style & LBS_OWNERDRAWVARIABLE != 0 {
            r.right += self.horz_pos;
            if index >= 0 && index < self.count() {
                if index < self.top {
                    for i in index..self.top {
                        r.top -= self.items[i as usize].height;
                    }
                } else {
                    for i in self.top..index {
                        r.top += self.items[i as usize].height;
                    }
                }
                r.bottom = r.top + self.items[index as usize].height;
            }
        } else {
            r.top += (index - self.top) * self.item_height;
            r.bottom = r.top + self.item_height;
            r.right += self.horz_pos;
        }
        let shows = r.left < self.width && r.right > 0 && r.top < self.height && r.bottom > 0;
        (shows as i32, r)
    }

    /// The item nearest to (`x`, `y`), or -1.
    fn item_from_point(&self, x: i32, y: i32) -> i32 {
        let n = self.count();
        if n == 0 {
            return -1;
        }
        let mut index = self.top;
        if self.style & LBS_OWNERDRAWVARIABLE != 0 {
            let mut pos = 0;
            if y >= 0 {
                while index < n {
                    pos += self.items[index as usize].height;
                    if pos > y {
                        break;
                    }
                    index += 1;
                }
            } else {
                while index > 0 {
                    index -= 1;
                    pos -= self.items[index as usize].height;
                    if pos <= y {
                        break;
                    }
                }
            }
        } else if self.style & LBS_MULTICOLUMN != 0 {
            if y >= self.item_height * self.page {
                return -1;
            }
            if y >= 0 {
                index += y / self.item_height.max(1);
            }
            let cw = self.column_width.max(1);
            if x >= 0 {
                index += x / cw * self.page;
            } else {
                index -= ((x + 1) / cw - 1) * self.page;
            }
        } else {
            index += y.div_euclid(self.item_height.max(1));
        }
        if index < 0 {
            return 0;
        }
        if index >= n {
            return -1;
        }
        index
    }

    /// LB_FINDSTRING's and LB_FINDSTRINGEXACT's search of a list with
    /// strings, or of one without by its data, from after `start`. None
    /// for a sorted list without strings and `exact`, which asks the
    /// owner.
    fn find_string(&self, start: i32, s: &[u8], data: u32, exact: bool) -> Option<i32> {
        let n = self.count();
        let start = if start >= n { -1 } else { start };
        let order = ((start + 1).max(0)..n).chain(0..=start.min(n - 1));
        if self.has_strings() {
            if s.is_empty() {
                return Some(LB_ERR);
            }
            let prefix = |t: &[u8], at: usize| {
                t.len() >= at + s.len() && t[at..at + s.len()].eq_ignore_ascii_case(s)
            };
            for i in order {
                let t = &self.items[i as usize].text;
                if exact {
                    if crate::win32::kernel32::compare_nocase(s, t) == Ordering::Equal {
                        return Some(i);
                    }
                } else if prefix(t, 0)
                    || t.first() == Some(&b'[')
                        && (prefix(t, 1) || t.get(1) == Some(&b'-') && prefix(t, 2))
                {
                    return Some(i);
                }
            }
            return Some(LB_ERR);
        }
        if exact && self.style & LBS_SORT != 0 {
            return None;
        }
        Some(
            order
                .into_iter()
                .find(|&i| self.items[i as usize].data == data)
                .unwrap_or(LB_ERR),
        )
    }

    /// Where a string goes in a sorted list with strings (-1 for the end
    /// of an unsorted one), or None when the owner compares the items.
    fn find_string_pos(&self, s: &[u8]) -> Option<i32> {
        if self.style & LBS_SORT == 0 {
            return Some(-1);
        }
        if !self.has_strings() {
            return None;
        }
        let (mut min, mut max) = (0, self.count());
        while min != max {
            let index = (min + max) / 2;
            match crate::win32::kernel32::compare_nocase(s, &self.items[index as usize].text) {
                Ordering::Equal => return Some(index),
                Ordering::Less => max = index,
                Ordering::Greater => min = index + 1,
            }
        }
        Some(max)
    }

    /// LB_ADDFILE's place: files, then directories, then drives.
    fn find_file_str_pos(&self, s: &[u8]) -> Option<i32> {
        if !self.has_strings() {
            return self.find_string_pos(s);
        }
        let cmp = |a: &[u8], b: &[u8]| match crate::win32::kernel32::compare_nocase(a, b) {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        };
        let at = |t: &[u8], i: usize| t.get(i).copied().unwrap_or(0) as i32;
        let (mut min, mut max) = (0, self.count());
        while min != max {
            let index = (min + max) / 2;
            let p = &self.items[index as usize].text;
            let res = if at(p, 0) == b'[' as i32 {
                if at(s, 0) != b'[' as i32 {
                    -1
                } else if at(p, 1) == b'-' as i32 {
                    if at(s, 1) == b'-' as i32 {
                        at(s, 2) - at(p, 2)
                    } else {
                        -1
                    }
                } else if at(s, 1) == b'-' as i32 {
                    1
                } else {
                    cmp(s, p)
                }
            } else if at(s, 0) == b'[' as i32 {
                1
            } else {
                cmp(s, p)
            };
            if res == 0 {
                return Some(index);
            }
            if res < 0 {
                max = index;
            } else {
                min = index + 1;
            }
        }
        Some(max)
    }

    fn sel_count(&self) -> i32 {
        if self.style & LBS_MULTIPLESEL == 0 || self.style & LBS_NOSEL != 0 {
            return LB_ERR;
        }
        self.items.iter().filter(|i| i.selected).count() as i32
    }

    /// LB_GETCURSEL's answer.
    fn cur_sel(&self) -> i32 {
        if self.items.is_empty() {
            LB_ERR
        } else if !self.multi_select() || self.selected != -1 {
            self.selected
        } else {
            self.focus
        }
    }
}

// What the combo box asks of its list.

fn list_of(cpu: &mut Cpu, hwnd: u32) -> Option<&ListBox> {
    user(cpu).window(hwnd).map(|w| &w.control.list)
}

/// LB_GETCURSEL of the list `hwnd`.
pub(super) fn cur_sel(cpu: &mut Cpu, hwnd: u32) -> i32 {
    list_of(cpu, hwnd).map_or(LB_ERR, |l| l.cur_sel())
}

/// LB_GETCOUNT of the list `hwnd`.
pub(super) fn count(cpu: &mut Cpu, hwnd: u32) -> i32 {
    list_of(cpu, hwnd).map_or(0, |l| l.count())
}

/// LB_GETITEMHEIGHT(0) of the list `hwnd`.
pub(super) fn item_height(cpu: &mut Cpu, hwnd: u32) -> i32 {
    list_of(cpu, hwnd).map_or(0, |l| {
        if l.style & LBS_OWNERDRAWVARIABLE != 0 && !l.items.is_empty() {
            l.items[0].height
        } else {
            l.item_height
        }
    })
}

/// The text of the item `index` of the list `hwnd` (None for no such
/// item), and its data.
pub(super) fn item(cpu: &mut Cpu, hwnd: u32, index: i32) -> Option<(Vec<u8>, u32)> {
    let l = list_of(cpu, hwnd)?;
    let i = l.items.get(usize::try_from(index).ok()?)?;
    Some((i.text.clone(), i.data))
}

/// LB_FINDSTRING(-1, `s`) of the list `hwnd`, which has strings.
pub(super) fn find_prefix(cpu: &mut Cpu, hwnd: u32, s: &[u8]) -> i32 {
    list_of(cpu, hwnd)
        .and_then(|l| l.find_string(-1, s, 0, false))
        .unwrap_or(LB_ERR)
}

/// What LB_SELECTSTRING(`start`, `lp`) of the list `hwnd` answers.
pub(super) fn select_string_result(cpu: &mut Cpu, hwnd: u32, start: i32, lp: u32) -> i32 {
    let strings = list_of(cpu, hwnd).is_some_and(|l| l.has_strings());
    let s = if strings {
        string_arg(cpu, lp).unwrap_or_default()
    } else {
        vec![]
    };
    list_of(cpu, hwnd)
        .and_then(|l| l.find_string(start, &s, lp, false))
        .unwrap_or(LB_ERR)
}

/// What LB_SETCURSEL(`index`) answers.
pub(super) fn set_cur_sel_result(cpu: &mut Cpu, hwnd: u32, index: i32) -> i32 {
    let Some(l) = list_of(cpu, hwnd) else {
        return LB_ERR;
    };
    if l.multi_select() || l.style & LBS_NOSEL != 0 || index < -1 || index >= l.count() {
        return LB_ERR;
    }
    index
}

// A message's work: the state taken out of the window, put back when it
// is done.

struct Ctx {
    hwnd: u32,
    id: u32,
    l: ListBox,
    out: Calls,
    redraw: bool,
}

impl Ctx {
    fn load(cpu: &mut Cpu, hwnd: u32) -> Option<Ctx> {
        let w = user(cpu).window(hwnd)?;
        Some(Ctx {
            hwnd,
            id: w.id & 0xFFFF,
            l: w.control.list.clone(),
            out: Calls::default(),
            redraw: false,
        })
    }

    /// Put the state back into the window.
    fn put(&mut self, cpu: &mut Cpu) {
        if let Some(w) = user(cpu).window_mut(self.hwnd) {
            w.control.list = self.l.clone();
        }
    }

    /// Put the state back, invalidate the window if it changed, and give
    /// the calls to make.
    fn finish(mut self, cpu: &mut Cpu) -> Calls {
        self.scroll_info();
        self.put(cpu);
        if self.redraw {
            paint::invalidate_rect(cpu, &[self.hwnd, 0, 1]);
        }
        self.out
    }

    fn store(self, cpu: &mut Cpu, result: i32) -> Option<u32> {
        let out = self.finish(cpu);
        run(cpu, LISTBOX, out, result as u32)
    }

    fn notify(&mut self, code: u32) {
        let wp = self.id | code << 16;
        self.out.send(self.l.owner, WM_COMMAND, wp, self.hwnd);
    }

    fn visible(&self, cpu: &mut Cpu) -> bool {
        user(cpu)
            .window(self.hwnd)
            .is_some_and(|w| w.style & WS_VISIBLE != 0)
    }

    /// SetScrollInfo for the scroll bars whose values changed, as
    /// LISTBOX_UpdateScroll has them.
    fn scroll_info(&mut self) {
        let l = &self.l;
        if l.style & LBS_NOREDRAW != 0 {
            return;
        }
        let mut bars = l.bars.clone();
        bars.resize(8, -1);
        let mut mask = SIF_RANGE | SIF_POS | SIF_PAGE;
        if l.style & LBS_DISABLENOSCROLL != 0 {
            mask |= SIF_DISABLENOSCROLL;
        }
        let (page, mask) = (l.page.max(1), mask as i32);
        // Each bar: the mask, the range's top, the page and the position.
        let (v, h) = if l.style & LBS_MULTICOLUMN != 0 {
            let cols = (l.width / l.column_width.max(1)).max(1);
            let h = [mask, (l.count() - 1) / page, cols, l.top / page];
            ([SIF_RANGE as i32, 0, 0, 0], Some(h))
        } else {
            let v = [mask, l.count() - 1, l.current_page_size(), l.top];
            let h = (l.horz_extent != 0).then_some([mask, l.horz_extent - 1, l.width, l.horz_pos]);
            (v, h)
        };
        let (v, h) = (v.map(|x| x as u32), h.map(|h| h.map(|x| x as u32)));
        if l.style & WS_VSCROLL != 0 {
            let words = v.map(|x| x as i32);
            if bars[0..4] != words {
                bars[0..4].copy_from_slice(&words);
                let si = [28, v[0], 0, v[1], v[2], v[3], 0];
                self.out.write(SCRATCH_DATA, &si);
                let args = [self.hwnd, SB_VERT, SCRATCH_DATA, 1];
                self.out.call_with("SetScrollInfo", &args, 4, false);
            }
        }
        if let Some(h) = h
            && l.style & WS_HSCROLL != 0
        {
            let words = h.map(|x| x as i32);
            if bars[4..8] != words {
                bars[4..8].copy_from_slice(&words);
                let si = [28, h[0], 0, h[1], h[2], h[3], 0];
                self.out.write(SCRATCH_DATA + 32, &si);
                let args = [self.hwnd, SB_HORZ, SCRATCH_DATA + 32, 1];
                self.out.call_with("SetScrollInfo", &args, 4, false);
            }
        }
        self.l.bars = bars;
    }

    // Repainting.

    fn invalidate_items(&mut self, index: i32) {
        if self.l.item_rect(index).0 == 1 {
            if self.l.style & LBS_NOREDRAW != 0 {
                self.l.flags |= LF_CHANGED;
                return;
            }
            self.redraw = true;
        }
    }

    fn invalidate_item(&mut self, index: i32) {
        if self.l.item_rect(index).0 == 1 {
            self.redraw = true;
        }
    }

    fn repaint_item(&mut self, index: i32) {
        if self.l.style & LBS_NOREDRAW != 0 {
            self.l.flags |= LF_CHANGED;
            return;
        }
        self.invalidate_item(index);
    }

    fn draw_focus_rect(&mut self) {
        if self.l.focus != -1 && self.l.caret_shows() {
            self.invalidate_item(self.l.focus);
        }
    }

    fn set_redraw(&mut self, on: bool) {
        let l = &mut self.l;
        if !on {
            l.style |= LBS_NOREDRAW;
            return;
        }
        if l.style & LBS_NOREDRAW == 0 {
            return;
        }
        l.style &= !LBS_NOREDRAW;
        if l.flags & LF_CHANGED != 0 {
            self.redraw = true;
            if l.top + l.page > l.count() {
                l.top = (l.count() - l.page).max(0);
            }
            l.flags &= !LF_CHANGED;
        }
    }

    // The layout.

    fn set_top_item(&mut self, index: i32) -> i32 {
        let l = &mut self.l;
        let mut index = index.min(l.max_top_index()).max(0);
        if l.style & LBS_MULTICOLUMN != 0 {
            index -= index % l.page.max(1);
        }
        if l.top != index {
            l.top = index;
            self.redraw = true;
        }
        LB_OKAY
    }

    fn update_page(&mut self) {
        let l = &mut self.l;
        let page = if l.item_height == 0 {
            1
        } else {
            (l.height / l.item_height).max(1)
        };
        if page == l.page {
            return;
        }
        l.page = page;
        if l.style & LBS_MULTICOLUMN != 0 {
            self.redraw = true;
        }
        let top = self.l.top;
        self.set_top_item(top);
    }

    /// WM_SIZE: the list's height a whole number of items, unless the
    /// style says no.
    fn update_size(&mut self, cpu: &mut Cpu) {
        let state = user(cpu);
        let client = client_of(state, self.hwnd);
        let rect = state.window(self.hwnd).map_or(Rect::default(), |w| w.rect);
        let l = &mut self.l;
        l.width = client.width();
        l.height = client.height();
        if l.style & (LBS_NOINTEGRALHEIGHT | LBS_OWNERDRAWVARIABLE) == 0 {
            let remaining = if l.item_height != 0 {
                l.height % l.item_height
            } else {
                0
            };
            if l.height > l.item_height && remaining != 0 {
                let (w, h) = (rect.width(), rect.height() - remaining);
                let flags = SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOMOVE;
                let args = [self.hwnd, 0, 0, 0, w as u32, h as u32, flags];
                self.out.call("SetWindowPos", &args);
                return;
            }
        }
        self.update_page();
        let focus = self.l.focus;
        self.invalidate_item(focus);
    }

    fn set_item_height(&mut self, index: i32, height: i32, repaint: bool) -> i32 {
        if height > 255 {
            return -1;
        }
        let height = height.max(1);
        if self.l.style & LBS_OWNERDRAWVARIABLE != 0 {
            if index < 0 || index >= self.l.count() {
                return LB_ERR;
            }
            self.l.items[index as usize].height = height;
            if repaint {
                self.invalidate_items(index);
            }
        } else if height != self.l.item_height {
            self.l.item_height = height;
            self.update_page();
            if repaint {
                self.redraw = true;
            }
        }
        LB_OKAY
    }

    fn set_horizontal_pos(&mut self, pos: i32) {
        let l = &mut self.l;
        let pos = pos.min(l.horz_extent - l.width).max(0);
        if l.horz_pos != pos {
            l.horz_pos = pos;
            self.redraw = true;
        }
    }

    fn set_horizontal_extent(&mut self, extent: i32) -> i32 {
        let l = &mut self.l;
        if l.horz_extent == 0 || l.style & LBS_MULTICOLUMN != 0 {
            return LB_OKAY;
        }
        let extent = extent.max(1);
        if extent == l.horz_extent {
            return LB_OKAY;
        }
        l.horz_extent = extent;
        if l.horz_pos > extent - l.width {
            self.set_horizontal_pos(extent - self.l.width);
        }
        LB_OKAY
    }

    fn set_column_width(&mut self, width: i32) -> i32 {
        if width != self.l.column_width {
            self.l.column_width = width;
            self.update_page();
        }
        LB_OKAY
    }

    /// The font's item height, and the average width for the tab stops.
    fn set_font(&mut self, cpu: &mut Cpu, font: u32) -> i32 {
        self.l.font = font;
        let (widths, height, _) = measure(cpu, font);
        let cx: i32 = (b'a'..=b'z')
            .chain(b'A'..=b'Z')
            .map(|b| widths[b as usize])
            .sum();
        self.l.avg_char_width = (cx / 26 + 1) / 2;
        if !self.l.owner_draw() {
            self.set_item_height(0, height, false);
        }
        height
    }

    fn make_item_visible(&mut self, index: i32, fully: bool) {
        let l = &self.l;
        let top = if index <= l.top {
            index
        } else if l.style & LBS_MULTICOLUMN != 0 {
            let mut cols = l.width;
            if !fully {
                cols += l.column_width - 1;
            }
            let cols = if cols >= l.column_width {
                cols / l.column_width.max(1)
            } else {
                1
            };
            if index < l.top + l.page * cols {
                return;
            }
            index - l.page * (cols - 1)
        } else if l.style & LBS_OWNERDRAWVARIABLE != 0 {
            let mut height = if fully {
                l.items[index as usize].height
            } else {
                1
            };
            let mut top = index;
            while top > l.top {
                height += l.items[top as usize - 1].height;
                if height > l.height {
                    break;
                }
                top -= 1;
            }
            top
        } else {
            if index < l.top + l.page {
                return;
            }
            if !fully && index == l.top + l.page && l.height > l.page * l.item_height {
                return;
            }
            index - l.page + 1
        };
        self.set_top_item(top);
    }

    // The selection.

    fn set_caret_index(&mut self, index: i32, fully: bool) -> i32 {
        if self.l.style & LBS_NOSEL != 0 || index < 0 || index >= self.l.count() {
            return LB_ERR;
        }
        if index == self.l.focus {
            return LB_OKAY;
        }
        self.draw_focus_rect();
        self.l.focus = index;
        self.make_item_visible(index, fully);
        self.draw_focus_rect();
        LB_OKAY
    }

    fn select_item_range(&mut self, first: i32, last: i32, on: bool) -> i32 {
        let l = &self.l;
        if l.style & LBS_NOSEL != 0 || l.style & LBS_MULTIPLESEL == 0 {
            return LB_ERR;
        }
        let n = l.count();
        if n == 0 {
            return LB_OKAY;
        }
        let last = if last == -1 || last >= n { n - 1 } else { last };
        let first = first.max(0);
        for i in first..=last {
            if self.l.items[i as usize].selected != on {
                self.l.items[i as usize].selected = on;
                self.invalidate_item(i);
            }
        }
        LB_OKAY
    }

    fn set_selection(&mut self, cpu: &mut Cpu, index: i32, on: bool, notify: bool) -> i32 {
        if self.l.style & LBS_NOSEL != 0 {
            self.l.selected = index;
            return LB_ERR;
        }
        if index < -1 || index >= self.l.count() {
            return LB_ERR;
        }
        if self.l.style & LBS_MULTIPLESEL != 0 {
            return if index == -1 {
                let n = self.l.count();
                self.select_item_range(0, n, on)
            } else {
                self.select_item_range(index, index, on)
            };
        }
        let old = self.l.selected;
        if index == old {
            return LB_OKAY;
        }
        if old != -1 {
            self.l.items[old as usize].selected = false;
            self.repaint_item(old);
        }
        if index != -1 {
            self.l.items[index as usize].selected = true;
        }
        self.l.selected = index;
        if index != -1 {
            self.repaint_item(index);
        }
        if notify && !self.l.items.is_empty() {
            self.notify(if index != -1 {
                LBN_SELCHANGE
            } else {
                LBN_SELCANCEL
            });
        } else if self.l.combo != 0 {
            combobox::set_state(cpu, self.l.combo, CBF_SELCHANGE, true);
        }
        LB_OKAY
    }

    /// Move the caret to `index`, the selection with it.
    fn move_caret(&mut self, cpu: &mut Cpu, index: i32, fully: bool) {
        if index < 0 || index >= self.l.count() {
            return;
        }
        self.draw_focus_rect();
        if self.l.style & LBS_EXTENDEDSEL != 0 {
            let anchor = self.l.anchor;
            if anchor != -1 {
                let (first, last) = (index.min(anchor), index.max(anchor));
                if first > 0 {
                    self.select_item_range(0, first - 1, false);
                }
                self.select_item_range(last + 1, -1, false);
                self.select_item_range(first, last, true);
            }
        } else if self.l.style & LBS_MULTIPLESEL == 0 {
            self.set_selection(cpu, index, true, false);
        }
        self.l.focus = index;
        self.make_item_visible(index, fully);
        self.draw_focus_rect();
    }

    // The items.

    /// WM_MEASUREITEM to the owner for the item `index` (-1 for all of an
    /// LBS_OWNERDRAWFIXED list), whose answer comes back as
    /// LB_SETITEMHEIGHT.
    fn measure_item(&mut self, index: i32, data: u32) {
        let mis = [
            ODT_LISTBOX,
            self.id,
            index as u32,
            0,
            self.l.item_height as u32,
            data,
        ];
        self.out.write(SCRATCH_DATA, &mis);
        self.out
            .send_struct(self.l.owner, WM_MEASUREITEM, self.id, SCRATCH_DATA, false);
        let at = index.max(0) as u32;
        self.out
            .send_read(self.hwnd, LB_SETITEMHEIGHT, at, SCRATCH_DATA + 16);
    }

    fn insert_item(&mut self, index: i32, text: Vec<u8>, data: u32) -> i32 {
        let n = self.l.count();
        let index = if index == -1 { n } else { index };
        if index < 0 || index > n {
            return LB_ERR;
        }
        let old_focus = self.l.focus;
        let height = self.l.item_height;
        let item = Item {
            text,
            data,
            selected: false,
            height,
        };
        self.l.items.insert(index as usize, item);
        if self.l.style & LBS_OWNERDRAWVARIABLE != 0 {
            self.measure_item(index, data);
        }
        self.invalidate_items(index);
        if self.l.count() == 1 {
            self.set_caret_index(0, false);
        } else if index <= self.l.selected {
            self.l.selected += 1;
            self.l.focus = old_focus;
        }
        LB_OKAY
    }

    fn insert_string(&mut self, index: i32, s: Option<Vec<u8>>, data: u32) -> i32 {
        let (text, data) = if self.l.has_strings() {
            (s.unwrap_or_default(), 0)
        } else {
            (Vec::new(), data)
        };
        let index = if index == -1 { self.l.count() } else { index };
        let r = self.insert_item(index, text, data);
        if r != 0 { r } else { index }
    }

    /// WM_DELETEITEM for an item that goes, if the owner needs it.
    fn delete_item(&mut self, index: i32, data: u32) {
        if self.l.owner_draw() || data != 0 {
            let dis = [ODT_LISTBOX, self.id, index as u32, self.hwnd, data];
            self.out.write(SCRATCH_DATA, &dis);
            self.out
                .send_struct(self.l.owner, WM_DELETEITEM, self.id, SCRATCH_DATA, false);
        }
    }

    fn remove_item(&mut self, index: i32) -> i32 {
        if index < 0 || index >= self.l.count() {
            return LB_ERR;
        }
        self.invalidate_items(index);
        let item = self.l.items.remove(index as usize);
        self.delete_item(index, item.data);
        let l = &mut self.l;
        if l.items.is_empty() {
            // As LB_RESETCONTENT leaves it.
            l.top = 0;
            l.selected = -1;
            l.focus = 0;
            l.anchor = -1;
            self.redraw = true;
            return LB_OKAY;
        }
        if l.anchor == l.count() {
            l.anchor -= 1;
        }
        if l.count() == l.page {
            self.set_top_item(0);
        }
        let l = &mut self.l;
        if !l.multi_select() {
            if index == l.selected {
                l.selected = -1;
            } else if index < l.selected {
                l.selected -= 1;
            }
        }
        if l.focus >= l.count() {
            l.focus = (l.count() - 1).max(0);
        }
        LB_OKAY
    }

    fn reset_content(&mut self) {
        for i in (0..self.l.count()).rev() {
            let data = self.l.items[i as usize].data;
            self.delete_item(i, data);
        }
        let l = &mut self.l;
        l.items.clear();
        l.top = 0;
        l.selected = -1;
        l.focus = 0;
        l.anchor = -1;
    }

    fn set_count(&mut self, count: i32) -> i32 {
        if self.l.has_strings() {
            return LB_ERR;
        }
        while count > self.l.count() {
            let r = self.insert_string(-1, None, 0);
            if r < 0 {
                return r;
            }
        }
        while count < self.l.count() {
            let n = self.l.count();
            let r = self.remove_item(n - 1);
            if r < 0 {
                return r;
            }
        }
        LB_OKAY
    }

    // Scrolling.

    fn v_scroll(&mut self, action: u32, pos: i32) {
        let l = &self.l;
        if l.style & LBS_MULTICOLUMN != 0 {
            return;
        }
        let top = match action {
            SB_LINEUP => l.top - 1,
            SB_LINEDOWN => l.top + 1,
            SB_PAGEUP => l.top - l.current_page_size(),
            SB_PAGEDOWN => l.top + l.current_page_size(),
            SB_THUMBPOSITION | SB_THUMBTRACK => pos,
            SB_TOP => 0,
            SB_BOTTOM => l.count(),
            _ => return,
        };
        self.set_top_item(top);
    }

    fn h_scroll(&mut self, action: u32, pos: i32) {
        let l = &self.l;
        if l.style & LBS_MULTICOLUMN != 0 {
            let cols = (l.width / l.column_width.max(1)).max(1);
            let top = match action {
                SB_LINEUP => l.top - l.page,
                SB_LINEDOWN => l.top + l.page,
                SB_PAGEUP => l.top - cols * l.page,
                SB_PAGEDOWN => l.top + cols * l.page,
                SB_THUMBPOSITION | SB_THUMBTRACK => pos * l.page,
                SB_TOP => 0,
                SB_BOTTOM => l.count(),
                _ => return,
            };
            self.set_top_item(top);
        } else if l.horz_extent != 0 {
            let to = match action {
                SB_LINEUP => l.horz_pos - 1,
                SB_LINEDOWN => l.horz_pos + 1,
                SB_PAGEUP => l.horz_pos - l.width,
                SB_PAGEDOWN => l.horz_pos + l.width,
                SB_THUMBPOSITION | SB_THUMBTRACK => pos,
                SB_TOP => 0,
                SB_BOTTOM => l.horz_extent - l.width,
                _ => return,
            };
            self.set_horizontal_pos(to);
        }
    }

    // The mouse.

    fn lbutton_down(&mut self, cpu: &mut Cpu, keys: u32, x: i32, y: i32) {
        let index = self.l.item_from_point(x, y);
        let (caret_on, in_focus) = (
            self.l.flags & LF_CARET_ON != 0,
            self.l.flags & LF_IN_FOCUS != 0,
        );
        if !caret_on && in_focus {
            return;
        }
        if !in_focus {
            let to = if self.l.combo == 0 {
                self.hwnd
            } else {
                let c = combobox::state(cpu, self.l.combo);
                if c.edit != 0 { c.edit } else { self.l.combo }
            };
            self.out.call("SetFocus", &[to]);
        }
        if index == -1 {
            return;
        }
        let notify = self.l.style & LBS_NOTIFY != 0;
        if self.l.combo == 0 && notify {
            let lp = make_long(x, y);
            self.out
                .send(self.l.owner, WM_LBTRACKPOINT, index as u32, lp);
        }
        self.l.flags |= LF_CAPTURED;
        self.out.call("SetCapture", &[self.hwnd]);
        let selected = self.l.items[index as usize].selected;
        if self.l.style & (LBS_EXTENDEDSEL | LBS_MULTIPLESEL) != 0 {
            if keys & MK_SHIFT == 0 {
                self.l.anchor = index;
            }
            if keys & MK_CONTROL != 0 {
                self.set_caret_index(index, false);
                self.set_selection(cpu, index, !selected, notify);
            } else {
                self.move_caret(cpu, index, false);
                let on = if self.l.style & LBS_EXTENDEDSEL != 0 {
                    self.l.items[index as usize].selected
                } else {
                    !self.l.items[index as usize].selected
                };
                self.set_selection(cpu, index, on, notify);
            }
        } else {
            self.l.anchor = index;
            self.move_caret(cpu, index, false);
            self.set_selection(cpu, index, true, notify);
        }
    }

    /// WM_LBUTTONDOWN and WM_LBUTTONDBLCLK in a combo box's list: in it,
    /// a click; outside it, the list closes, or its scroll bar scrolls.
    fn lbutton_down_combo(&mut self, cpu: &mut Cpu, msg: u32, keys: u32, x: i32, y: i32) {
        let client = Rect::new(0, 0, self.l.width, self.l.height);
        let combo = self.l.combo;
        if client.contains(x, y) {
            if msg == WM_LBUTTONDOWN {
                let index = if self.l.items.is_empty() {
                    -1
                } else {
                    self.l.selected
                };
                combobox::set_dropped_index(cpu, combo, index);
                self.lbutton_down(cpu, keys, x, y);
            } else if self.l.style & LBS_NOTIFY != 0 {
                self.notify(LBN_DBLCLK);
            }
            return;
        }
        let state = user(cpu);
        let old_capture = state.input.capture;
        let screen = state.screen_rect(self.hwnd);
        let origin = state.client_origin(self.hwnd);
        let style = state.window(self.hwnd).map_or(0, |w| w.style);
        self.out.call("ReleaseCapture", &[]);
        let (sx, sy) = (x + origin.0, y + origin.1);
        if !screen.contains(sx, sy) {
            let dropped = combobox::state(cpu, combo).dropped_index;
            self.set_caret_index(dropped, false);
            self.set_selection(cpu, dropped, false, false);
            self.put(cpu);
            combobox::flip(cpu, combo, false, false, &mut self.out);
            return;
        }
        let mut r = client;
        let mut hit = 0;
        if style & WS_VSCROLL != 0 {
            r.right += sysparams::metric(SM_CXVSCROLL);
            if r.contains(x, y) {
                hit = HTVSCROLL;
            }
        }
        if style & WS_HSCROLL != 0 {
            r.bottom += sysparams::metric(SM_CYHSCROLL);
            if r.contains(x, y) {
                hit = HTHSCROLL;
            }
        }
        if hit != 0 {
            self.out
                .send(self.hwnd, WM_NCLBUTTONDOWN, hit, make_long(sx, sy));
        }
        if old_capture != 0 {
            self.out.call("SetCapture", &[old_capture]);
        }
    }

    fn lbutton_up(&mut self, cpu: &mut Cpu) {
        if self.l.flags & LF_CAPTURED != 0 {
            self.l.flags &= !LF_CAPTURED;
            if user(cpu).input.capture == self.hwnd {
                self.out.call("ReleaseCapture", &[]);
            }
            if self.l.style & LBS_NOTIFY != 0 && !self.l.items.is_empty() {
                self.notify(LBN_SELCHANGE);
            }
        }
    }

    /// The mouse moves with the button down: the caret follows it, one
    /// item past the edge when it is outside.
    fn mouse_move(&mut self, cpu: &mut Cpu, x: i32, y: i32) {
        if self.l.flags & LF_CAPTURED == 0 {
            return;
        }
        let l = &self.l;
        let (mut x, mut y) = (x, y);
        // Which way it goes past the edge: up, left, down, right.
        let mut dir = 0;
        if l.style & LBS_MULTICOLUMN != 0 {
            y = y.max(0).min(l.item_height * l.page - 1);
            if x < 0 {
                dir = 2;
                x = 0;
            } else if x >= l.width {
                dir = 4;
                x = l.width - 1;
            }
        } else if y < 0 {
            dir = 1;
        } else if y >= l.height {
            dir = 3;
        }
        let mut index = l.item_from_point(x, y);
        if index == -1 {
            index = l.focus;
        }
        match dir {
            1 => index = (l.top - 1).max(0),
            2 => {
                if l.top != 0 {
                    index -= l.page;
                }
            }
            3 => {
                index = l.top + l.current_page_size();
                if index == l.focus {
                    index += 1;
                }
                index = index.min(l.count() - 1);
            }
            4 if index + l.page < l.count() => index += l.page,
            _ => {}
        }
        if index != self.l.focus {
            self.move_caret(cpu, index, false);
        }
    }

    // The keyboard.

    /// WM_KEYDOWN, with the item the owner chose for it (-1 for none).
    fn key_down(&mut self, cpu: &mut Cpu, key: u32, caret: i32) {
        let l = &self.l;
        let mut force = !(l.multi_select() || l.selected == l.focus);
        let multi_col = l.style & LBS_MULTICOLUMN != 0;
        let cols = (l.width / l.column_width.max(1)).max(1);
        let mut caret = caret;
        if caret == -1 {
            match key {
                VK_LEFT if multi_col => {
                    force = false;
                    if l.focus >= l.page {
                        caret = l.focus - l.page;
                    }
                }
                VK_LEFT | VK_UP => caret = (l.focus - 1).max(0),
                VK_RIGHT if multi_col => {
                    force = false;
                    if l.focus + l.page < l.count() {
                        caret = l.focus + l.page;
                    }
                }
                VK_RIGHT | VK_DOWN => caret = (l.focus + 1).min(l.count() - 1),
                VK_PRIOR => {
                    caret = if multi_col {
                        l.focus - cols * l.page + 1
                    } else {
                        l.focus - l.current_page_size() + 1
                    };
                    caret = caret.max(0);
                }
                VK_NEXT => {
                    caret = if multi_col {
                        l.focus + cols * l.page - 1
                    } else {
                        l.focus + l.current_page_size() - 1
                    };
                    caret = caret.min(l.count() - 1);
                }
                VK_HOME => caret = 0,
                VK_END => caret = l.count() - 1,
                VK_SPACE => {
                    if l.style & LBS_EXTENDEDSEL != 0 {
                        caret = l.focus;
                    } else if l.style & LBS_MULTIPLESEL != 0 && l.focus < l.count() {
                        let (focus, notify) = (l.focus, l.style & LBS_NOTIFY != 0);
                        let on = !l.items[focus as usize].selected;
                        self.set_selection(cpu, focus, on, notify);
                    }
                }
                _ => force = false,
            }
        }
        if force {
            caret = self.l.focus;
        }
        if caret < 0 {
            return;
        }
        let l = &self.l;
        if l.style & LBS_EXTENDEDSEL != 0 && !key_down(cpu, VK_SHIFT) || !l.multi_select() {
            self.l.anchor = caret;
        }
        self.move_caret(cpu, caret, true);
        if self.l.style & LBS_MULTIPLESEL != 0 {
            self.l.selected = caret;
        } else {
            self.set_selection(cpu, caret, true, false);
        }
        if self.l.style & LBS_NOTIFY != 0 {
            if self.l.combo != 0 && self.visible(cpu) {
                // So that the combo box doesn't close the list.
                combobox::set_state(cpu, self.l.combo, CBF_NOROLLUP, true);
            }
            if !self.l.items.is_empty() {
                self.notify(LBN_SELCHANGE);
            }
        }
    }

    /// WM_CHAR, with the item the owner chose for it (-1 for none).
    fn char_(&mut self, cpu: &mut Cpu, ch: u8, caret: i32) {
        let mut caret = caret;
        if caret == -1 {
            let focus = self.l.focus;
            caret = self.l.find_string(focus, &[ch], 0, false).unwrap_or(LB_ERR);
        }
        if caret == -1 {
            return;
        }
        if !self.l.multi_select() && self.l.selected == -1 {
            self.set_selection(cpu, caret, true, false);
        }
        self.move_caret(cpu, caret, true);
        if self.l.style & LBS_NOTIFY != 0 && !self.l.items.is_empty() {
            self.notify(LBN_SELCHANGE);
        }
    }
}

// The window procedure.

/// A string argument of a list with strings, or None for a null pointer.
fn string_arg(cpu: &mut Cpu, at: u32) -> Option<Vec<u8>> {
    if at == 0 {
        None
    } else {
        Some(bytes_at(cpu, at).unwrap_or_default())
    }
}

pub fn listbox_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, msg, wp, lp) = (args[0], args[1], args[2], args[3]);
    let created = user(cpu)
        .window(hwnd)
        .is_some_and(|w| w.control.list.flags & LF_CREATED != 0);
    if !created {
        if msg == WM_CREATE {
            return create(cpu, hwnd, lp);
        }
        return def_window_proc(cpu, args);
    }
    let mut c = Ctx::load(cpu, hwnd)?;
    let combo = c.l.combo;
    let result: i32 = match msg {
        LB_RESETCONTENT => {
            c.reset_content();
            c.redraw = true;
            0
        }
        LB_ADDSTRING | LB_ADDFILE => {
            let s = if c.l.has_strings() {
                string_arg(cpu, lp)
            } else {
                None
            };
            let key = s.clone().unwrap_or_default();
            let pos = if msg == LB_ADDSTRING {
                c.l.find_string_pos(&key)
            } else {
                c.l.find_file_str_pos(&key)
            };
            match pos {
                Some(pos) => c.insert_string(pos, s, lp),
                None => {
                    let out = c.finish(cpu);
                    return sort(cpu, out, hwnd, lp, false);
                }
            }
        }
        LB_INSERTSTRING => {
            let s = if c.l.has_strings() {
                string_arg(cpu, lp)
            } else {
                None
            };
            c.insert_string(wp as i32, s, lp)
        }
        LB_DELETESTRING => {
            if c.remove_item(wp as i32) != LB_ERR {
                c.l.count()
            } else {
                LB_ERR
            }
        }
        LB_GETITEMDATA => match c.l.items.get(wp as usize) {
            Some(i) => i.data as i32,
            None => LB_ERR,
        },
        LB_SETITEMDATA => match c.l.items.get_mut(wp as usize) {
            Some(i) => {
                i.data = lp;
                1
            }
            None => LB_ERR,
        },
        LB_GETCOUNT => c.l.count(),
        LB_GETTEXT => match c.l.items.get(wp as usize) {
            None => LB_ERR,
            Some(i) if c.l.has_strings() => {
                if lp != 0 {
                    let mut bytes = i.text.clone();
                    bytes.push(0);
                    cpu.bus.guest_write_bytes(lp, &bytes);
                }
                i.text.len() as i32
            }
            Some(i) => {
                if lp != 0 {
                    write_words(cpu, lp, &[i.data]);
                }
                4
            }
        },
        LB_GETTEXTLEN => match c.l.items.get(wp as usize) {
            None => LB_ERR,
            Some(_) if !c.l.has_strings() => 4,
            Some(i) => i.text.len() as i32,
        },
        LB_GETCURSEL => c.l.cur_sel(),
        LB_GETTOPINDEX => c.l.top,
        LB_GETITEMHEIGHT => {
            let index = wp as i32;
            if c.l.style & LBS_OWNERDRAWVARIABLE != 0 && !c.l.items.is_empty() {
                match c.l.items.get(index as usize) {
                    Some(i) => i.height,
                    None => LB_ERR,
                }
            } else {
                c.l.item_height
            }
        }
        LB_SETITEMHEIGHT => c.set_item_height(wp as i32, lp as i32, true),
        LB_ITEMFROMPOINT => {
            if c.l.items.is_empty() {
                0x1FFFF
            } else {
                let (mut x, mut y) = (lp as i16 as i32, (lp >> 16) as i16 as i32);
                let mut hit = true;
                if !Rect::new(0, 0, c.l.width, c.l.height).contains(x, y) {
                    x = x.min(c.l.width - 1).max(0);
                    y = y.min(c.l.height - 1).max(0);
                    hit = false;
                }
                let mut index = c.l.item_from_point(x, y);
                if index == -1 {
                    index = c.l.count() - 1;
                    hit = false;
                }
                make_long(index, !hit as i32) as i32
            }
        }
        LB_SETCARETINDEX => {
            if !c.l.multi_select() && c.l.selected != -1
                || c.set_caret_index(wp as i32, lp == 0) == LB_ERR
            {
                LB_ERR
            } else {
                LB_OKAY
            }
        }
        LB_GETCARETINDEX => c.l.focus,
        LB_SETTOPINDEX => c.set_top_item(wp as i32),
        LB_SETCOLUMNWIDTH => c.set_column_width(wp as i32),
        LB_GETITEMRECT => {
            let (r, rect) = c.l.item_rect(wp as i32);
            if lp != 0 {
                rect.write(cpu, lp);
            }
            r
        }
        LB_FINDSTRING | LB_FINDSTRINGEXACT | LB_SELECTSTRING => {
            let s = if c.l.has_strings() {
                string_arg(cpu, lp).unwrap_or_default()
            } else {
                vec![]
            };
            let exact = msg == LB_FINDSTRINGEXACT;
            match c.l.find_string(wp as i32, &s, lp, exact) {
                None => {
                    let out = c.finish(cpu);
                    return sort(cpu, out, hwnd, lp, true);
                }
                Some(index) => {
                    if msg == LB_SELECTSTRING && index != LB_ERR {
                        c.move_caret(cpu, index, true);
                        c.set_selection(cpu, index, true, false);
                    }
                    index
                }
            }
        }
        LB_GETSEL => match c.l.items.get(wp as usize) {
            Some(i) => i.selected as i32,
            None => LB_ERR,
        },
        LB_SETSEL => c.set_selection(cpu, lp as i32, wp != 0, false),
        LB_SETCURSEL => {
            if c.l.multi_select() {
                LB_ERR
            } else {
                c.set_caret_index(wp as i32, false);
                let r = c.set_selection(cpu, wp as i32, true, false);
                if r != LB_ERR { c.l.selected } else { r }
            }
        }
        LB_GETSELCOUNT => c.l.sel_count(),
        LB_GETSELITEMS => {
            if c.l.style & LBS_MULTIPLESEL == 0 {
                LB_ERR
            } else {
                let sel: Vec<u32> = (0..c.l.count())
                    .filter(|&i| c.l.items[i as usize].selected)
                    .take(wp as usize)
                    .map(|i| i as u32)
                    .collect();
                write_words(cpu, lp, &sel);
                sel.len() as i32
            }
        }
        LB_SELITEMRANGE => {
            let (lo, hi) = ((lp & 0xFFFF) as i32, (lp >> 16) as i32);
            c.select_item_range(lo.min(hi), lo.max(hi), wp != 0)
        }
        LB_SELITEMRANGEEX => {
            if lp as i32 >= wp as i32 {
                c.select_item_range(wp as i32, lp as i32, true)
            } else {
                c.select_item_range(lp as i32, wp as i32, false)
            }
        }
        LB_GETHORIZONTALEXTENT => c.l.horz_extent,
        LB_SETHORIZONTALEXTENT => c.set_horizontal_extent(wp as i32),
        LB_GETANCHORINDEX => c.l.anchor,
        LB_SETANCHORINDEX => {
            if (wp as i32) < -1 || wp as i32 >= c.l.count() {
                LB_ERR
            } else {
                c.l.anchor = wp as i32;
                LB_OKAY
            }
        }
        // Listing a directory isn't done.
        LB_DIR => LB_ERR,
        LB_GETLOCALE => c.l.locale as i32,
        LB_SETLOCALE => std::mem::replace(&mut c.l.locale, wp) as i32,
        LB_INITSTORAGE => LB_OKAY,
        LB_SETCOUNT => c.set_count(wp as i32),
        LB_SETTABSTOPS => {
            if c.l.style & LBS_USETABSTOPS == 0 {
                0
            } else {
                let n = (wp as i32).max(0) as usize;
                let stops = if n != 0 && lp != 0 {
                    read_words(cpu, lp, n.min(0x1000))
                } else {
                    vec![]
                };
                let avg = c.l.avg_char_width;
                c.l.tabs = stops.iter().map(|&t| t as i32 * avg / 4).collect();
                1
            }
        }
        LB_CARETON | LB_CARETOFF => {
            let on = msg == LB_CARETON;
            if (c.l.flags & LF_CARET_ON != 0) != on {
                c.l.flags ^= LF_CARET_ON;
                if c.l.focus != -1 && c.l.flags & LF_IN_FOCUS != 0 {
                    let focus = c.l.focus;
                    c.repaint_item(focus);
                }
            }
            LB_OKAY
        }
        WM_DESTROY => {
            c.reset_content();
            c.l.flags &= !LF_CREATED;
            1
        }
        WM_ENABLE => {
            c.redraw = true;
            0
        }
        WM_SETREDRAW => {
            c.set_redraw(wp != 0);
            0
        }
        WM_GETDLGCODE => (DLGC_WANTARROWS | DLGC_WANTCHARS) as i32,
        WM_PAINT | WM_PRINTCLIENT => {
            let owner = c.l.owner;
            return paint_start(cpu, LISTBOX, hwnd, wp, WM_CTLCOLORLISTBOX, owner, &[0, 0]);
        }
        WM_SIZE => {
            c.update_size(cpu);
            0
        }
        WM_GETFONT => c.l.font as i32,
        WM_SETFONT => {
            c.set_font(cpu, wp);
            if lp != 0 {
                c.redraw = true;
            }
            0
        }
        WM_SETFOCUS => {
            c.l.flags |= LF_IN_FOCUS | LF_CARET_ON;
            c.draw_focus_rect();
            c.notify(LBN_SETFOCUS);
            0
        }
        WM_KILLFOCUS => {
            if c.l.focus != -1 && c.l.caret_shows() {
                c.redraw = true;
            }
            c.l.flags &= !LF_IN_FOCUS;
            c.notify(LBN_KILLFOCUS);
            0
        }
        WM_HSCROLL => {
            c.h_scroll(wp & 0xFFFF, (wp >> 16) as i32);
            0
        }
        WM_VSCROLL => {
            c.v_scroll(wp & 0xFFFF, (wp >> 16) as i32);
            0
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            let (x, y) = (lp as i16 as i32, (lp >> 16) as i16 as i32);
            if combo != 0 {
                c.lbutton_down_combo(cpu, msg, wp, x, y);
            } else if msg == WM_LBUTTONDOWN {
                c.lbutton_down(cpu, wp, x, y);
            } else if c.l.style & LBS_NOTIFY != 0 {
                c.notify(LBN_DBLCLK);
            }
            0
        }
        WM_MOUSEMOVE => {
            let (x, y) = (lp as i16 as i32, (lp >> 16) as i16 as i32);
            if combo != 0 && combobox::state(cpu, combo).style & CBS_DROPDOWNLIST != CBS_SIMPLE {
                // A drop-down list tracks the mouse as if captured.
                let captured = c.l.flags & LF_CAPTURED;
                if Rect::new(0, 0, c.l.width, c.l.height).contains(x, y) {
                    c.l.flags |= LF_CAPTURED;
                }
                c.mouse_move(cpu, x, y);
                c.l.flags = c.l.flags & !LF_CAPTURED | captured;
            } else if user(cpu).input.capture == hwnd {
                c.mouse_move(cpu, x, y);
            }
            0
        }
        WM_LBUTTONUP => {
            if combo != 0 {
                let (x, y) = (lp as i16 as i32, (lp >> 16) as i16 as i32);
                let inside = Rect::new(0, 0, c.l.width, c.l.height).contains(x, y);
                if lp == u32::MAX || !inside {
                    let dropped = combobox::state(cpu, combo).dropped_index;
                    c.move_caret(cpu, dropped, false);
                }
            }
            c.lbutton_up(cpu);
            0
        }
        WM_KEYDOWN => {
            if combo != 0 {
                let cs = combobox::state(cpu, combo);
                let eui = cs.state & CBF_EUI != 0;
                if cs.style & CBS_DROPDOWNLIST != CBS_SIMPLE
                    && (!eui && wp == VK_F4
                        || eui && cs.state & CBF_DROPPED == 0 && (wp == VK_DOWN || wp == VK_UP))
                {
                    c.put(cpu);
                    combobox::flip(cpu, combo, false, false, &mut c.out);
                    return c.store(cpu, 0);
                }
            }
            if c.l.style & LBS_WANTKEYBOARDINPUT != 0 {
                return ask_owner(cpu, c, WM_VKEYTOITEM, wp);
            }
            c.key_down(cpu, wp, -1);
            0
        }
        WM_CHAR => {
            if c.l.style & LBS_WANTKEYBOARDINPUT != 0 {
                return ask_owner(cpu, c, WM_CHARTOITEM, wp & 0xFF);
            }
            c.char_(cpu, wp as u8, -1);
            0
        }
        WM_ERASEBKGND => {
            if c.l.owner_draw() && c.l.flags & LF_CHANGED == 0 {
                let owner = c.l.owner;
                return paint_start(cpu, LISTBOX, hwnd, wp, WM_CTLCOLORLISTBOX, owner, &[1, 0]);
            }
            1
        }
        WM_NCDESTROY => {
            if combo != 0 && combobox::state(cpu, combo).style & CBS_DROPDOWNLIST != CBS_SIMPLE {
                combobox::set_list(cpu, combo, 0);
            }
            c.finish(cpu);
            return def_window_proc(cpu, args);
        }
        WM_NCACTIVATE if combo != 0 => 0,
        _ => {
            c.finish(cpu);
            return def_window_proc(cpu, args);
        }
    };
    c.store(cpu, result)
}

/// WM_CREATE: the state from the style, the font's item height, and an
/// LBS_OWNERDRAWFIXED list's from its owner.
fn create(cpu: &mut Cpu, hwnd: u32, cs: u32) -> Option<u32> {
    let state = user(cpu);
    let w = state.window(hwnd)?;
    let (id, parent) = (w.id & 0xFFFF, w.parent);
    let mut style = w.style;
    let client = client_of(state, hwnd);
    let combo = if style & LBS_COMBOBOX != 0 && cs != 0 {
        cpu.bus.guest_read_32(cs)
    } else {
        0
    };
    let combo = if combobox::is_combo(cpu, combo) {
        combo
    } else {
        0
    };
    if style & LBS_EXTENDEDSEL != 0 {
        style |= LBS_MULTIPLESEL;
    }
    if style & LBS_MULTICOLUMN != 0 {
        style &= !LBS_OWNERDRAWVARIABLE;
    }
    if style & LBS_OWNERDRAWVARIABLE != 0 {
        style |= LBS_NOINTEGRALHEIGHT;
    }
    let mut flags = LF_CREATED;
    if combo == 0 && style & LBS_NOSEL == 0 {
        flags |= LF_CARET_ON;
    }
    let l = ListBox {
        style,
        owner: if combo != 0 { combo } else { parent },
        combo,
        flags,
        top: 0,
        selected: -1,
        focus: 0,
        anchor: -1,
        item_height: 1,
        page: 1,
        column_width: 150,
        horz_extent: (style & WS_HSCROLL != 0) as i32,
        locale: 0x0409,
        width: client.width(),
        height: client.height(),
        ..ListBox::default()
    };
    let mut c = Ctx {
        hwnd,
        id,
        l,
        out: Calls::default(),
        redraw: false,
    };
    c.l.item_height = c.set_font(cpu, 0);
    if combo != 0 {
        combobox::set_list(cpu, combo, hwnd);
    }
    if style & LBS_OWNERDRAWFIXED != 0 {
        let cs = combobox::state(cpu, combo);
        if combo != 0 && cs.style & combobox::CBS_DROPDOWN != 0 {
            c.l.item_height = cs.fixed_height.max(1);
        } else {
            c.measure_item(-1, 0);
        }
    }
    c.store(cpu, 0)
}

/// A sorted owner-drawn list without strings: the owner compares the
/// item with data `data` with the list's to find its place, and it goes
/// there (LB_ADDSTRING), or the index of an equal one is the answer
/// (LB_FINDSTRINGEXACT). After `out`.
fn sort(cpu: &mut Cpu, out: Calls, hwnd: u32, data: u32, find: bool) -> Option<u32> {
    let n = count(cpu, hwnd) as u32;
    let mut vars = vec![hwnd, data, find as u32, 0, n, 0];
    vars.extend(out.into_vars(SORT_CALLS));
    start(cpu, LISTBOX, K_SORT, vars, SCRATCH)
}

/// Where the calls before the search start in `sort`'s task. The
/// variables before: the list, the data, whether it finds, the search's
/// bottom and top, and the index compared.
const SORT_CALLS: usize = 7;

/// The steps of `sort`'s task: the calls before, then the search, then
/// the insertion.
fn sort_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    match t.step {
        0 => match ops_step(cpu, t, SORT_CALLS, value) {
            Some(next) => next,
            None => compare_next(cpu, t),
        },
        1 => {
            let index = t.var(6) as i32;
            let res = value as i32;
            if res == 0 {
                return finish_sort(t, index);
            }
            if res < 0 {
                t.set(5, index as u32);
            } else {
                t.set(4, (index + 1) as u32);
            }
            compare_next(cpu, t)
        }
        _ => Next::Done(value),
    }
}

/// WM_COMPAREITEM for the middle of what is left, or the end.
fn compare_next(cpu: &mut Cpu, t: &mut Task) -> Next {
    let (hwnd, data) = (t.var(1), t.var(2));
    let (min, max) = (t.var(4) as i32, t.var(5) as i32);
    if min == max {
        let find = t.var(3) != 0;
        return finish_sort(t, if find { -1 } else { max });
    }
    let index = (min + max) / 2;
    t.set(6, index as u32);
    let state = user(cpu);
    let Some(w) = state.window(hwnd) else {
        return Next::Done(LB_ERR as u32);
    };
    let (id, l) = (w.id & 0xFFFF, &w.control.list);
    let item_data = l.items.get(index as usize).map_or(0, |i| i.data);
    let (owner, locale) = (l.owner, l.locale);
    let cis = [
        ODT_LISTBOX,
        id,
        hwnd,
        u32::MAX,
        data,
        index as u32,
        item_data,
        locale,
    ];
    write_words(cpu, t.scratch + SCRATCH_DATA, &cis);
    let args = [owner, WM_COMPAREITEM, id, t.scratch + SCRATCH_DATA];
    Next::call(super::super::address_of("SendMessageA"), &args, 1)
}

/// The search found `index`: LB_FINDSTRINGEXACT's answer, or where
/// LB_ADDSTRING inserts the item.
fn finish_sort(t: &mut Task, index: i32) -> Next {
    if t.var(3) != 0 {
        return Next::Done(index as u32);
    }
    let args = [t.var(1), LB_INSERTSTRING, index as u32, t.var(2)];
    Next::call(super::super::address_of("SendMessageA"), &args, 2)
}

/// LBS_WANTKEYBOARDINPUT: the owner says which item a key goes to with
/// WM_VKEYTOITEM or WM_CHARTOITEM, then the list does the rest.
fn ask_owner(cpu: &mut Cpu, c: Ctx, msg: u32, key: u32) -> Option<u32> {
    let (hwnd, owner, focus) = (c.hwnd, c.l.owner, c.l.focus);
    c.finish(cpu);
    // The last is where the calls after are, which start after it.
    let vars = vec![
        hwnd,
        msg,
        key,
        owner,
        (key & 0xFFFF) | (focus as u32) << 16,
        0,
    ];
    start(cpu, LISTBOX, K_KEY, vars, SCRATCH)
}

fn key_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let (hwnd, msg, key) = (t.var(1), t.var(2), t.var(3));
    match t.step {
        0 => {
            let args = [t.var(4), msg, t.var(5), hwnd];
            Next::call(super::super::address_of("SendMessageA"), &args, 1)
        }
        1 => {
            let caret = value as i32;
            if caret == -2 {
                return Next::Done(0);
            }
            let Some(mut c) = Ctx::load(cpu, hwnd) else {
                return Next::Done(0);
            };
            if msg == WM_VKEYTOITEM {
                c.key_down(cpu, key, caret);
            } else {
                c.char_(cpu, key as u8, caret);
            }
            let out = c.finish(cpu);
            let base = ops_push(t, out);
            t.set(6, base as u32);
            Next::call(0, &[], 2)
        }
        _ => {
            let base = t.var(6) as usize;
            ops_step(cpu, t, base, value).unwrap_or(Next::Done(0))
        }
    }
}

pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    match super::kind(t) {
        K_CALLS => calls_step(cpu, t, value),
        K_PAINT => paint_step(cpu, t, value),
        K_SORT => sort_step(cpu, t, value),
        _ => key_step(cpu, t, value),
    }
}

// Painting.

/// The WM_PAINT task, or WM_ERASEBKGND's for an owner-drawn list (the
/// first of its own variables): the owner draws an owner-drawn list's
/// items, in calls whose list the second gives.
fn paint_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    if let Some(next) = paint_head(t, value) {
        return next;
    }
    let (hwnd, hdc) = (t.var(P_HWND), t.var(P_HDC));
    let erase = t.var(P_OWN) != 0;
    match t.step {
        3 => {
            if erase {
                let brush = t.var(P_BRUSH);
                let brush = if brush != 0 {
                    brush
                } else {
                    sysparams::brush(cpu, COLOR_WINDOW)
                };
                let client = client_of(user(cpu), hwnd);
                paint::fill(cpu, hdc, client, brush);
                return Next::Done(1);
            }
            let brush = t.var(P_BRUSH);
            let scratch = t.scratch + SCRATCH_DATA;
            let calls = draw(cpu, hwnd, hdc, brush, scratch);
            let base = ops_push(t, calls);
            t.set(P_OWN + 1, base as u32);
            Next::call(0, &[], 4)
        }
        4 => {
            let base = t.var(P_OWN + 1) as usize;
            if let Some(next) = ops_step(cpu, t, base, value) {
                return next;
            }
            paint_end(t, 5)
        }
        _ => Next::Done(0),
    }
}

/// Draw the list `hwnd` on `hdc`, as LISTBOX_Paint does, with `brush`:
/// what it draws itself now, the owner's items in the calls it gives.
/// `rect` is a place for a rectangle in the task's scratch space.
fn draw(cpu: &mut Cpu, hwnd: u32, hdc: u32, brush: u32, rect: u32) -> Calls {
    let mut out = Calls::default();
    let Some(w) = user(cpu).window(hwnd) else {
        return out;
    };
    let (id, enabled) = (w.id & 0xFFFF, w.style & WS_DISABLED == 0);
    let l = w.control.list.clone();
    if l.style & LBS_NOREDRAW != 0 {
        return out;
    }
    select_font(cpu, hdc, l.font);
    default_brush(cpu, hdc, brush, COLOR_WINDOW);
    if !enabled {
        let gray = user(cpu).color(COLOR_GRAYTEXT);
        dc::set_text_color(cpu, &[hdc, gray]);
    }
    let shift = |r: Rect| r.offset(-l.horz_pos, 0);
    let mut r = Rect::new(0, 0, l.width, l.height);
    if l.style & LBS_MULTICOLUMN != 0 {
        r.right = r.left + l.column_width;
    } else if l.horz_pos != 0 {
        r.right += l.horz_pos;
    }
    let owner_draw = l.owner_draw();
    let item = |cpu: &mut Cpu, out: &mut Calls, r: Rect, index: i32, action: u32| {
        let shown = shift(r);
        let focused = action == ODA_FOCUS;
        let Some(it) = l.items.get(index as usize) else {
            if focused {
                shown.write(cpu, rect);
                paint::draw_focus_rect(cpu, &[hdc, rect]);
            }
            return;
        };
        if owner_draw {
            let mut state = 0;
            if it.selected {
                state |= ODS_SELECTED;
            }
            if focused && l.focus == index && l.caret_shows() {
                state |= ODS_FOCUS;
            }
            if !enabled {
                state |= ODS_DISABLED;
            }
            let (a, b) = (shown.left as u32, shown.top as u32);
            let (c, d) = (shown.right as u32, shown.bottom as u32);
            let dis = [
                ODT_LISTBOX,
                id,
                index as u32,
                action,
                state,
                hwnd,
                hdc,
                a,
                b,
                c,
                d,
                it.data,
            ];
            out.write(SCRATCH_DATA, &dis);
            out.send_struct(l.owner, WM_DRAWITEM, id, SCRATCH_DATA, false);
            return;
        }
        if focused {
            shown.write(cpu, rect);
            paint::draw_focus_rect(cpu, &[hdc, rect]);
            return;
        }
        let colors = it.selected.then(|| {
            let (hb, hf) = (
                user(cpu).color(COLOR_HIGHLIGHT),
                user(cpu).color(COLOR_HIGHLIGHTTEXT),
            );
            let bk = dc::set_bk_color(cpu, &[hdc, hb]).unwrap_or(0);
            let fg = dc::set_text_color(cpu, &[hdc, hf]).unwrap_or(0);
            (bk, fg)
        });
        let at = (shown.left + 1, shown.top);
        if l.style & LBS_USETABSTOPS == 0 {
            font::draw_text(
                cpu,
                hdc,
                at,
                ETO_OPAQUE | ETO_CLIPPED,
                Some(shown),
                &it.text,
                None,
            );
        } else {
            font::draw_text(
                cpu,
                hdc,
                at,
                ETO_OPAQUE | ETO_CLIPPED,
                Some(shown),
                &[],
                None,
            );
            text::tabbed(cpu, hdc, at, &it.text, &l.tabs, 0, true);
        }
        if let Some((bk, fg)) = colors {
            dc::set_bk_color(cpu, &[hdc, bk]);
            dc::set_text_color(cpu, &[hdc, fg]);
        }
    };
    let blank = |cpu: &mut Cpu, r: Rect| {
        font::draw_text(
            cpu,
            hdc,
            (0, 0),
            ETO_OPAQUE | ETO_CLIPPED,
            Some(shift(r)),
            &[],
            None,
        );
    };
    if l.items.is_empty() && l.focus != -1 && l.caret_shows() {
        // An empty list shows its focus rectangle on the first line.
        r.bottom = r.top + l.item_height;
        blank(cpu, r);
        item(cpu, &mut out, r, l.focus, ODA_FOCUS);
        r.top = r.bottom;
    }
    let mut focus_rect = None;
    let mut col_pos = l.page - 1;
    for i in l.top..l.count() {
        r.bottom = if l.style & LBS_OWNERDRAWVARIABLE == 0 {
            r.top + l.item_height
        } else {
            r.top + l.items[i as usize].height
        };
        if i == l.focus {
            focus_rect = Some(r);
        }
        item(cpu, &mut out, r, i, ODA_DRAWENTIRE);
        r.top = r.bottom;
        if l.style & LBS_MULTICOLUMN != 0 && col_pos == 0 {
            if !owner_draw && r.top < l.height {
                r.bottom = l.height;
                blank(cpu, r);
            }
            r.left += l.column_width;
            r.right += l.column_width;
            r.top = 0;
            col_pos = l.page - 1;
        } else {
            col_pos -= 1;
            if r.top >= l.height {
                break;
            }
        }
    }
    if let Some(f) = focus_rect
        && f.top != f.bottom
        && l.caret_shows()
    {
        item(cpu, &mut out, f, l.focus, ODA_FOCUS);
    }
    if !owner_draw {
        if r.top < l.height {
            r.bottom = l.height;
            blank(cpu, r);
        }
        if r.right < l.width {
            blank(cpu, Rect::new(r.right, 0, l.width, l.height));
        }
    }
    out
}

const ETO_OPAQUE: u32 = 2;
const ETO_CLIPPED: u32 = 4;

crate::state_fields!(Item {
    text,
    data,
    selected,
    height
});

crate::state_fields!(ListBox {
    style,
    owner,
    combo,
    flags,
    items,
    top,
    selected,
    focus,
    anchor,
    item_height,
    page,
    column_width,
    horz_extent,
    horz_pos,
    tabs,
    avg_char_width,
    font,
    locale,
    width,
    height,
    bars
});
