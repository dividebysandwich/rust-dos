//! Dialog boxes as Norton Commander draws them: a double frame inside a
//! margin, grey (or red, for warnings and errors), a shadow, and rows of
//! text, input fields, check boxes, radio buttons, lists and buttons.

use crate::cp437;
use crate::keys::Key;
use crate::screen::{Screen, pad};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Ok,
    Cancel,
    Copy,
    Rename,
    Tree,
    Delete,
    All,
    Skip,
    Overwrite,
    Retry,
    Yes,
    No,
    Set,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// A line of text; the parts between '~' highlighted.
    Text {
        text: String,
        centre: bool,
    },
    /// ╟───╢
    Sep,
    Field {
        text: Vec<u8>,
        cursor: usize,
        scroll: usize,
    },
    Check {
        label: String,
        on: bool,
        centre: bool,
    },
    Radio {
        label: String,
        on: bool,
    },
    Buttons {
        list: Vec<(String, Button)>,
        brackets: bool,
    },
    /// A row of choices, such as drive letters.
    Choices {
        list: Vec<String>,
        sel: usize,
    },
    /// A list `height` rows high.
    List {
        list: Vec<String>,
        sel: usize,
        top: usize,
        height: usize,
    },
}

impl Item {
    pub fn text(text: &str) -> Item {
        Item::Text {
            text: text.to_string(),
            centre: false,
        }
    }

    pub fn centred(text: &str) -> Item {
        Item::Text {
            text: text.to_string(),
            centre: true,
        }
    }

    pub fn field(text: &str) -> Item {
        let text = cp437::bytes(text);
        Item::Field {
            cursor: text.len(),
            text,
            scroll: 0,
        }
    }

    pub fn check(label: &str, on: bool) -> Item {
        Item::Check {
            label: label.to_string(),
            on,
            centre: false,
        }
    }

    /// A check box in the middle of its line, as narrow dialogs have it.
    pub fn centred_check(label: &str, on: bool) -> Item {
        Item::Check {
            label: label.to_string(),
            on,
            centre: true,
        }
    }

    pub fn radio(label: &str, on: bool) -> Item {
        Item::Radio {
            label: label.to_string(),
            on,
        }
    }

    pub fn buttons(list: &[(&str, Button)], brackets: bool) -> Item {
        Item::Buttons {
            list: list.iter().map(|(t, b)| (t.to_string(), *b)).collect(),
            brackets,
        }
    }

    fn height(&self) -> usize {
        match self {
            Item::List { height, .. } => *height,
            _ => 1,
        }
    }
}

/// What a key or a click did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    None,
    /// A button pressed, or Enter for the first one.
    Button(Button),
    Cancel,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dialog {
    pub title: String,
    /// The frame's width.
    pub width: usize,
    pub red: bool,
    pub items: Vec<Item>,
    /// Which of `targets` has the focus.
    pub focus: usize,
    /// The top row, else row 5 as NC puts them.
    pub top: Option<usize>,
    /// The columns to centre it in, else the screen's.
    pub region: Option<(usize, usize)>,
}

/// Where a dialog is: its margin's top left corner and size.
#[derive(Clone, Copy, Debug)]
pub struct Place {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl Place {
    /// The first column inside the frame, after a space.
    fn inner_x(&self) -> usize {
        self.x + 5
    }

    fn inner_w(&self) -> usize {
        self.w - 10
    }
}

impl Dialog {
    pub fn new(title: &str, width: usize, items: Vec<Item>) -> Dialog {
        let mut d = Dialog {
            title: title.to_string(),
            width,
            red: false,
            items,
            focus: 0,
            top: None,
            region: None,
        };
        // The focus starts on the first field or list, else the first
        // button.
        let targets = d.targets();
        d.focus = targets
            .iter()
            .position(|&(i, _)| {
                matches!(
                    d.items[i],
                    Item::Field { .. } | Item::List { .. } | Item::Choices { .. }
                )
            })
            .or_else(|| {
                targets
                    .iter()
                    .position(|&(i, _)| matches!(d.items[i], Item::Buttons { .. }))
            })
            .unwrap_or(0);
        d
    }

    pub fn red(mut self) -> Dialog {
        self.red = true;
        self
    }

    /// A message with buttons in a row, NC's plain ones.
    pub fn message(title: &str, lines: &[&str], buttons: &[(&str, Button)], red: bool) -> Dialog {
        let width = lines
            .iter()
            .map(|l| l.chars().filter(|&c| c != '~').count())
            .chain(std::iter::once(
                buttons.iter().map(|(b, _)| b.len() + 3).sum::<usize>(),
            ))
            .max()
            .unwrap_or(10)
            .max(title.len() + 6)
            + 4;
        let mut items: Vec<Item> = lines.iter().map(|l| Item::centred(l)).collect();
        items.push(Item::buttons(buttons, false));
        let d = Dialog::new(title, width, items);
        if red { d.red() } else { d }
    }

    fn colours(&self) -> (u8, u8, u8) {
        // Background, highlight, focused button.
        if self.red {
            (0x4F, 0x4E, 0x70)
        } else {
            (0x70, 0x7E, 0x30)
        }
    }

    /// The items that take the focus: (item, button or 0).
    fn targets(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for (i, item) in self.items.iter().enumerate() {
            match item {
                Item::Field { .. }
                | Item::Check { .. }
                | Item::Radio { .. }
                | Item::Choices { .. }
                | Item::List { .. } => out.push((i, 0)),
                Item::Buttons { list, .. } => out.extend((0..list.len()).map(|b| (i, b))),
                _ => {}
            }
        }
        out
    }

    fn focused(&self) -> Option<(usize, usize)> {
        self.targets().get(self.focus).copied()
    }

    pub fn place(&self, cols: usize, rows: usize) -> Place {
        let w = self.width + 6;
        let h = self.items.iter().map(Item::height).sum::<usize>() + 4;
        let (rx, rw) = self.region.unwrap_or((0, cols));
        let x = rx + rw.saturating_sub(w + 1) / 2;
        let y = self
            .top
            .unwrap_or_else(|| 5.min(rows.saturating_sub(h + 2) / 2));
        Place { x, y, w, h }
    }

    // ----- Reading what was entered -----

    pub fn field(&self, n: usize) -> String {
        self.items
            .iter()
            .filter_map(|i| match i {
                Item::Field { text, .. } => Some(cp437::string(text)),
                _ => None,
            })
            .nth(n)
            .unwrap_or_default()
    }

    pub fn set_field(&mut self, n: usize, value: &str) {
        if let Some(Item::Field {
            text,
            cursor,
            scroll,
        }) = self
            .items
            .iter_mut()
            .filter(|i| matches!(i, Item::Field { .. }))
            .nth(n)
        {
            *text = cp437::bytes(value);
            *cursor = text.len();
            *scroll = 0;
        }
    }

    pub fn checked(&self, n: usize) -> bool {
        self.items
            .iter()
            .filter_map(|i| match i {
                Item::Check { on, .. } => Some(*on),
                _ => None,
            })
            .nth(n)
            .unwrap_or(false)
    }

    /// Which radio button is on, from 0.
    pub fn radio(&self) -> usize {
        self.items
            .iter()
            .filter_map(|i| match i {
                Item::Radio { on, .. } => Some(*on),
                _ => None,
            })
            .position(|on| on)
            .unwrap_or(0)
    }

    /// The choice or list row chosen.
    pub fn selected(&self) -> usize {
        self.items
            .iter()
            .find_map(|i| match i {
                Item::Choices { sel, .. } | Item::List { sel, .. } => Some(*sel),
                _ => None,
            })
            .unwrap_or(0)
    }

    /// The text of the first line, to change as an operation goes on.
    pub fn set_text(&mut self, n: usize, value: &str) {
        if let Some(Item::Text { text, .. }) = self
            .items
            .iter_mut()
            .filter(|i| matches!(i, Item::Text { .. }))
            .nth(n)
        {
            *text = value.to_string();
        }
    }

    fn first_button(&self) -> Option<Button> {
        self.items.iter().find_map(|i| match i {
            Item::Buttons { list, .. } => list.first().map(|b| b.1),
            _ => None,
        })
    }

    // ----- Keys -----

    pub fn key(&mut self, key: Key) -> Event {
        let targets = self.targets();
        let n = targets.len().max(1);
        let focused = self.focused();
        if key == Key::Esc {
            return Event::Cancel;
        }
        // The focused item's own keys first.
        if let Some((i, b)) = focused {
            match &mut self.items[i] {
                Item::Field { text, cursor, .. } => match key {
                    Key::Char(c) => {
                        text.insert(*cursor, c);
                        *cursor += 1;
                        return Event::None;
                    }
                    Key::Backspace => {
                        if *cursor > 0 {
                            *cursor -= 1;
                            text.remove(*cursor);
                        }
                        return Event::None;
                    }
                    Key::Del => {
                        if *cursor < text.len() {
                            text.remove(*cursor);
                        }
                        return Event::None;
                    }
                    Key::Left => {
                        *cursor = cursor.saturating_sub(1);
                        return Event::None;
                    }
                    Key::Right => {
                        *cursor = (*cursor + 1).min(text.len());
                        return Event::None;
                    }
                    Key::Home => {
                        *cursor = 0;
                        return Event::None;
                    }
                    Key::End => {
                        *cursor = text.len();
                        return Event::None;
                    }
                    Key::Ctrl(b'Y') => {
                        text.clear();
                        *cursor = 0;
                        return Event::None;
                    }
                    _ => {}
                },
                Item::Check { on, .. } => {
                    if key == Key::Char(b' ') {
                        *on = !*on;
                        return Event::None;
                    }
                }
                Item::Radio { .. } => {
                    if key == Key::Char(b' ') {
                        self.choose_radio(i);
                        return Event::None;
                    }
                }
                Item::Choices { list, sel } => match key {
                    Key::Left => {
                        *sel = sel.checked_sub(1).unwrap_or(list.len().saturating_sub(1));
                        return Event::None;
                    }
                    Key::Right => {
                        *sel = if *sel + 1 >= list.len() { 0 } else { *sel + 1 };
                        return Event::None;
                    }
                    Key::Char(c) => {
                        if let Some(at) = list.iter().position(|l| {
                            l.bytes().next().is_some_and(|f| f.eq_ignore_ascii_case(&c))
                        }) {
                            *sel = at;
                            return Event::Button(Button::Ok);
                        }
                    }
                    _ => {}
                },
                Item::List {
                    list,
                    sel,
                    top,
                    height,
                } => {
                    let last = list.len().saturating_sub(1);
                    let moved = match key {
                        Key::Up => Some(sel.saturating_sub(1)),
                        Key::Down => Some((*sel + 1).min(last)),
                        Key::PgUp => Some(sel.saturating_sub(*height)),
                        Key::PgDn => Some((*sel + *height).min(last)),
                        Key::Home => Some(0),
                        Key::End => Some(last),
                        _ => None,
                    };
                    if let Some(to) = moved {
                        *sel = to;
                        if *sel < *top {
                            *top = *sel;
                        } else if *sel >= *top + *height {
                            *top = *sel + 1 - *height;
                        }
                        return Event::None;
                    }
                }
                Item::Buttons { list, .. } => {
                    if matches!(key, Key::Enter | Key::Char(b' ')) {
                        return Event::Button(list[b].1);
                    }
                }
                _ => {}
            }
        }
        match key {
            Key::Tab | Key::Down => {
                self.focus = (self.focus + 1) % n;
            }
            Key::ShiftTab | Key::Up => {
                self.focus = (self.focus + n - 1) % n;
            }
            Key::Left | Key::Right
                if focused.is_some_and(|(i, _)| matches!(self.items[i], Item::Buttons { .. })) =>
            {
                self.focus = if key == Key::Left {
                    (self.focus + n - 1) % n
                } else {
                    (self.focus + 1) % n
                };
            }
            Key::Enter => {
                return Event::Button(self.first_button().unwrap_or(Button::Ok));
            }
            Key::F(10) => {
                if self.has(Button::Tree) {
                    return Event::Button(Button::Tree);
                }
            }
            Key::Char(c) | Key::Alt(c) => {
                // A button's first letter presses it, but not while typing.
                let typing =
                    focused.is_some_and(|(i, _)| matches!(self.items[i], Item::Field { .. }));
                if typing && matches!(key, Key::Char(_)) {
                    return Event::None;
                }
                for item in &self.items {
                    if let Item::Buttons { list, .. } = item {
                        for (label, b) in list {
                            if label
                                .bytes()
                                .find(u8::is_ascii_alphabetic)
                                .is_some_and(|f| f.eq_ignore_ascii_case(&c))
                            {
                                return Event::Button(*b);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        Event::None
    }

    pub fn has(&self, button: Button) -> bool {
        self.items
            .iter()
            .any(|i| matches!(i, Item::Buttons { list, .. } if list.iter().any(|b| b.1 == button)))
    }

    fn choose_radio(&mut self, at: usize) {
        for (i, item) in self.items.iter_mut().enumerate() {
            if let Item::Radio { on, .. } = item {
                *on = i == at;
            }
        }
    }

    // ----- Mouse -----

    /// A click at a cell.
    pub fn click(&mut self, col: usize, row: usize, cols: usize, rows: usize) -> Event {
        let p = self.place(cols, rows);
        if row < p.y || row >= p.y + p.h || col < p.x || col >= p.x + p.w {
            return Event::None;
        }
        let targets = self.targets();
        let mut r = p.y + 2;
        for i in 0..self.items.len() {
            let height = self.items[i].height();
            if row >= r && row < r + height {
                let line = row - r;
                match &mut self.items[i] {
                    Item::Buttons { list, brackets } => {
                        for (b, (x, w)) in button_cells(list, *brackets, p.inner_x(), p.inner_w())
                            .into_iter()
                            .enumerate()
                        {
                            if col >= x && col < x + w {
                                let pressed = list[b].1;
                                self.focus = targets.iter().position(|&t| t == (i, b)).unwrap_or(0);
                                return Event::Button(pressed);
                            }
                        }
                    }
                    Item::Check { on, .. } => *on = !*on,
                    Item::Radio { .. } => self.choose_radio(i),
                    Item::Choices { list, sel } => {
                        for (c, (x, w)) in choice_cells(list, p.inner_x(), p.inner_w())
                            .into_iter()
                            .enumerate()
                        {
                            if col >= x && col < x + w {
                                *sel = c;
                                return Event::Button(Button::Ok);
                            }
                        }
                    }
                    Item::List { list, sel, top, .. } => {
                        let at = *top + line;
                        if at < list.len() {
                            if *sel == at {
                                return Event::Button(Button::Ok);
                            }
                            *sel = at;
                        }
                    }
                    Item::Field {
                        text,
                        cursor,
                        scroll,
                    } => {
                        *cursor = (*scroll + col.saturating_sub(p.inner_x())).min(text.len());
                    }
                    _ => {}
                }
                if let Some(f) = targets.iter().position(|&(t, _)| t == i) {
                    self.focus = f;
                }
                return Event::None;
            }
            r += height;
        }
        Event::None
    }

    // ----- Drawing -----

    pub fn draw(&mut self, s: &mut Screen) {
        let p = self.place(s.cols, s.rows);
        let (bg, hot, focus_attr) = self.colours();
        s.fill(p.y, p.x, p.w, p.h, b' ', bg);
        s.frame(p.y + 1, p.x + 3, p.w - 6, p.h - 2, bg, true);
        if !self.title.is_empty() {
            s.title(p.y + 1, p.x + 3, p.w - 6, &self.title, bg);
        }
        s.shadow(p.y, p.x, p.w, p.h);
        let focused = self.focused();
        let (x, w) = (p.inner_x(), p.inner_w());
        let mut r = p.y + 2;
        s.cursor = None;
        for i in 0..self.items.len() {
            let has_focus = focused.is_some_and(|(f, _)| f == i);
            match &mut self.items[i] {
                Item::Text { text, centre: c } => {
                    let len = text.chars().filter(|&ch| ch != '~').count();
                    let at = if *c {
                        x - 1 + (w + 2).saturating_sub(len).div_ceil(2)
                    } else {
                        x
                    };
                    s.marked(r, at, text, bg, hot);
                }
                Item::Sep => s.divider(r, p.x + 3, p.w - 6, bg),
                Item::Field {
                    text,
                    cursor,
                    scroll,
                } => {
                    if *cursor < *scroll {
                        *scroll = *cursor;
                    } else if *cursor >= *scroll + w {
                        *scroll = *cursor + 1 - w;
                    }
                    let shown: Vec<u8> = text.iter().skip(*scroll).copied().collect();
                    s.text(r, x, &pad(&shown, w), 0x30);
                    if has_focus {
                        s.cursor = Some((x + *cursor - *scroll, r));
                    }
                }
                Item::Check { label, on, centre } => {
                    let text = format!("[{}] {}", if *on { 'x' } else { ' ' }, label);
                    let len = text.chars().count();
                    let at = if *centre {
                        x - 1 + (w + 2).saturating_sub(len).div_ceil(2)
                    } else {
                        x
                    };
                    s.str(r, at, &text, bg);
                    if has_focus {
                        s.cursor = Some((at + 1, r));
                    }
                }
                Item::Radio { label, on } => {
                    s.str(
                        r,
                        x,
                        &format!("({}) {}", if *on { '•' } else { ' ' }, label),
                        bg,
                    );
                    if has_focus {
                        s.cursor = Some((x + 1, r));
                    }
                }
                Item::Buttons { list, brackets } => {
                    let cells = button_cells(list, *brackets, x, w);
                    for (b, ((label, _), (bx, _))) in list.iter().zip(cells).enumerate() {
                        let text = if *brackets {
                            format!("[ {} ]", label)
                        } else {
                            format!(" {} ", label)
                        };
                        let attr = if focused == Some((i, b)) {
                            focus_attr
                        } else {
                            bg
                        };
                        s.str(r, bx, &text, attr);
                    }
                }
                Item::Choices { list, sel } => {
                    for (c, (cx, _)) in choice_cells(list, x, w).into_iter().enumerate() {
                        let attr = if c == *sel { focus_attr } else { bg };
                        s.str(r, cx, &format!(" {} ", list[c]), attr);
                    }
                }
                Item::List {
                    list,
                    sel,
                    top,
                    height,
                } => {
                    for line in 0..*height {
                        let at = *top + line;
                        let text = list.get(at).map(|t| cp437::bytes(t)).unwrap_or_default();
                        let attr = if at == *sel && list.get(at).is_some() {
                            focus_attr
                        } else {
                            bg
                        };
                        s.text(r + line, x, &pad(&text, w), attr);
                    }
                }
            }
            r += self.items[i].height();
        }
    }
}

/// Where each button of a row is: (column, width), the row centred with
/// one space between NC's plain buttons and three between bracketed ones.
fn button_cells(
    list: &[(String, Button)],
    brackets: bool,
    x: usize,
    w: usize,
) -> Vec<(usize, usize)> {
    let widths: Vec<usize> = list
        .iter()
        .map(|(l, _)| l.chars().count() + if brackets { 4 } else { 2 })
        .collect();
    let gap = if brackets { 3 } else { 1 };
    let total: usize = widths.iter().sum::<usize>() + gap * widths.len().saturating_sub(1);
    let mut at = x - 1 + (w + 2).saturating_sub(total).div_ceil(2);
    let mut out = Vec::new();
    for bw in widths {
        out.push((at, bw));
        at += bw + gap;
    }
    out
}

/// Where each choice of a row is, " C " each with a space between.
fn choice_cells(list: &[String], x: usize, w: usize) -> Vec<(usize, usize)> {
    let widths: Vec<usize> = list.iter().map(|l| l.chars().count() + 2).collect();
    let total: usize = widths.iter().sum::<usize>() + widths.len().saturating_sub(1);
    let mut at = x - 1 + (w + 2).saturating_sub(total).div_ceil(2);
    let mut out = Vec::new();
    for cw in widths {
        out.push((at, cw));
        at += cw + 1;
    }
    out
}
