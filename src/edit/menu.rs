//! EDIT's menu bar and its pull-down menus.

use super::screen::Screen;

/// What a menu item does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    New,
    Open,
    Save,
    SaveAs,
    Exit,
    Cut,
    Copy,
    Paste,
    Clear,
    Find,
    RepeatFind,
    Replace,
    Keyboard,
    About,
}

pub struct Item {
    /// The label, '&' before its hotkey; None for a divider.
    pub label: Option<&'static str>,
    pub shortcut: &'static str,
    pub action: Action,
    /// What the status line says about it.
    pub help: &'static str,
}

const fn item(label: &'static str, shortcut: &'static str, action: Action, help: &'static str) -> Item {
    Item { label: Some(label), shortcut, action, help }
}

const DIVIDER: Item = Item { label: None, shortcut: "", action: Action::About, help: "" };

pub struct Menu {
    pub title: &'static str,
    pub items: &'static [Item],
}

pub const MENUS: [Menu; 4] = [
    Menu {
        title: "&File",
        items: &[
            item("&New", "", Action::New, "Removes currently loaded file from memory"),
            item("&Open...", "", Action::Open, "Loads new file into memory"),
            item("&Save", "", Action::Save, "Saves current file"),
            item("Save &As...", "", Action::SaveAs, "Saves current file with specified name"),
            DIVIDER,
            item("E&xit", "", Action::Exit, "Exits editor and returns to DOS"),
        ],
    },
    Menu {
        title: "&Edit",
        items: &[
            item("Cu&t", "Shift+Del", Action::Cut, "Deletes selected text and copies it to buffer"),
            item("&Copy", "Ctrl+Ins", Action::Copy, "Copies selected text to buffer"),
            item("&Paste", "Shift+Ins", Action::Paste, "Inserts buffer contents at current location"),
            item("Cl&ear", "Del", Action::Clear, "Deletes selected text without copying it to buffer"),
        ],
    },
    Menu {
        title: "&Search",
        items: &[
            item("&Find...", "", Action::Find, "Finds specified text"),
            item("&Repeat Last Find", "F3", Action::RepeatFind, "Finds next occurrence of text specified in previous search"),
            item("Re&place...", "", Action::Replace, "Finds and replaces specified text"),
        ],
    },
    Menu {
        title: "&Help",
        items: &[
            item("&Keyboard", "F1", Action::Keyboard, "Displays the keys EDIT takes"),
            DIVIDER,
            item("&About...", "", Action::About, "Displays product version"),
        ],
    },
];

/// The hotkey of a label: the letter after '&'.
pub fn hotkey(label: &str) -> u8 {
    label.split_once('&').and_then(|(_, rest)| rest.bytes().next()).unwrap_or(0).to_ascii_uppercase()
}

fn label_len(label: &str) -> usize {
    label.len() - label.matches('&').count()
}

/// The column of menu `n`'s title on the bar of a screen `cols` wide:
/// Help at the right end, as in EDIT.COM.
pub fn title_col(n: usize, cols: usize) -> usize {
    if n == MENUS.len() - 1 {
        return cols.saturating_sub(label_len(MENUS[n].title) + 3);
    }
    2 + MENUS[..n].iter().map(|m| label_len(m.title) + 2).sum::<usize>()
}

/// The menu whose title is at column `col` of the bar.
pub fn title_at(col: usize, cols: usize) -> Option<usize> {
    (0..MENUS.len()).find(|&n| {
        let start = title_col(n, cols) - 1;
        col >= start && col < start + label_len(MENUS[n].title) + 2
    })
}

/// Where menu `n`'s box goes, and how big it is: (col, width, height).
pub fn menu_box(n: usize, cols: usize) -> (usize, usize, usize) {
    let items = MENUS[n].items;
    let width = items
        .iter()
        .map(|i| i.label.map_or(0, |l| label_len(l) + if i.shortcut.is_empty() { 0 } else { i.shortcut.len() + 3 }))
        .max()
        .unwrap_or(0)
        + 4;
    let col = (title_col(n, cols) - 1).min(cols.saturating_sub(width + 2));
    (col, width, items.len() + 2)
}

/// Menu colours: black on grey, hotkeys bright white; the highlighted
/// item grey on black; items that don't apply dark grey.
const ATTR: u8 = 0x70;
const HOT: u8 = 0x7F;
const SELECTED: u8 = 0x07;
const SELECTED_HOT: u8 = 0x0F;
const DISABLED: u8 = 0x78;

/// The menu bar on row 0: `highlight` the title shown selected, with the
/// hotkeys showing when `hotkeys`.
pub fn draw_bar(s: &mut Screen, highlight: Option<usize>, hotkeys: bool) {
    let cols = s.cols;
    s.fill(0, 0, cols, 1, b' ', ATTR);
    for (n, menu) in MENUS.iter().enumerate() {
        let col = title_col(n, cols);
        let selected = highlight == Some(n);
        let (attr, hot) = match (selected, hotkeys) {
            (true, true) => (SELECTED, SELECTED_HOT),
            (true, false) => (SELECTED, SELECTED),
            (false, true) => (ATTR, HOT),
            (false, false) => (ATTR, ATTR),
        };
        if selected {
            s.fill(0, col - 1, label_len(menu.title) + 2, 1, b' ', attr);
        }
        s.label(0, col, menu.title, attr, hot);
    }
}

/// Menu `n` pulled down, item `sel` highlighted; `enabled` says which
/// actions apply.
pub fn draw_menu(s: &mut Screen, n: usize, sel: usize, enabled: impl Fn(Action) -> bool) {
    let (col, width, height) = menu_box(n, s.cols);
    s.frame(1, col, width, height, ATTR);
    s.shadow(1, col, width, height);
    for (i, item) in MENUS[n].items.iter().enumerate() {
        let row = 2 + i;
        let Some(label) = item.label else {
            s.divider(row, col, width, ATTR);
            continue;
        };
        let on = enabled(item.action);
        let (attr, hot) = match (i == sel, on) {
            (true, true) => (SELECTED, SELECTED_HOT),
            (true, false) => (0x08, 0x08),
            (false, true) => (ATTR, HOT),
            (false, false) => (DISABLED, DISABLED),
        };
        s.fill(row, col + 1, width - 2, 1, b' ', attr);
        s.label(row, col + 2, label, attr, hot);
        if !item.shortcut.is_empty() {
            s.str(row, col + width - 2 - item.shortcut.len(), item.shortcut, attr);
        }
    }
}

/// The next item of menu `n` from `sel`, past dividers.
pub fn step(n: usize, sel: usize, down: bool) -> usize {
    let items = MENUS[n].items;
    let len = items.len();
    let mut at = sel;
    loop {
        at = if down { (at + 1) % len } else { (at + len - 1) % len };
        if items[at].label.is_some() {
            return at;
        }
    }
}
