//! The pull-down menus F9 opens: Left, Files, Commands, Options and Right,
//! as Norton Commander 4 has them.

use crate::cp437;
use crate::panel::{Sort, View};
use crate::screen::Screen;

const BAR: u8 = 0x30;
const BAR_SELECTED: u8 = 0x0F;
const ITEM: u8 = 0x3F;
const ITEM_HOT: u8 = 0x3E;
const SELECTED: u8 = 0x0F;
const SELECTED_HOT: u8 = 0x0E;

pub const TITLES: [&str; 5] = ["Left", "Files", "Commands", "Options", "Right"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// What a panel shows, left (0) or right (1).
    View(usize, View),
    OnOff(usize),
    Sort(usize, Sort),
    Reread(usize),
    Filter(usize),
    Drive(usize),
    Help,
    ViewFile,
    Edit,
    Copy,
    Move,
    Mkdir,
    Delete,
    Attributes,
    SelectGroup,
    DeselectGroup,
    Invert,
    Restore,
    Quit,
    Ncd,
    FindFile,
    History,
    Swap,
    PanelsOnOff,
    AutoMenus,
    PathPrompt,
    KeyBar,
    FullScreen,
    MiniStatus,
    Clock,
    /// Shown, as in NC, but not there.
    Missing,
    Separator,
}

/// What the menus show checked.
#[derive(Clone, Copy, Debug)]
pub struct Checks {
    pub views: [View; 2],
    pub sorts: [Sort; 2],
    pub auto_menus: bool,
    pub path_prompt: bool,
    pub key_bar: bool,
    pub full_screen: bool,
    pub mini_status: bool,
    pub clock: bool,
}

#[derive(Clone, Debug)]
pub struct MenuItem {
    /// The label, the hot key after '~'.
    pub label: &'static str,
    pub shortcut: String,
    pub action: Action,
    pub checked: bool,
}

fn item(label: &'static str, shortcut: &str, action: Action, checked: bool) -> MenuItem {
    MenuItem {
        label,
        shortcut: shortcut.to_string(),
        action,
        checked,
    }
}

fn separator() -> MenuItem {
    item("", "", Action::Separator, false)
}

/// A menu's items.
pub fn items(menu: usize, c: &Checks) -> Vec<MenuItem> {
    match menu {
        0 | 4 => {
            let p = if menu == 0 { 0 } else { 1 };
            let fkey = if p == 0 { "F1" } else { "F2" };
            let v = c.views[p];
            let s = c.sorts[p];
            vec![
                item("~Brief", "", Action::View(p, View::Brief), v == View::Brief),
                item("~Full", "", Action::View(p, View::Full), v == View::Full),
                item("~Info", "", Action::View(p, View::Info), v == View::Info),
                item("~Tree", "", Action::View(p, View::Tree), v == View::Tree),
                item(
                    "Quick ~view",
                    "",
                    Action::View(p, View::Quick),
                    v == View::Quick,
                ),
                item("~Compressed File", "", Action::Missing, false),
                item("Lin~k", "", Action::Missing, false),
                item(
                    "~On/Off",
                    &format!("Ctrl-{}", fkey),
                    Action::OnOff(p),
                    false,
                ),
                separator(),
                item(
                    "~Name",
                    "Ctrl-F3",
                    Action::Sort(p, Sort::Name),
                    s == Sort::Name,
                ),
                item(
                    "E~xtension",
                    "Ctrl-F4",
                    Action::Sort(p, Sort::Extension),
                    s == Sort::Extension,
                ),
                item(
                    "Ti~me",
                    "Ctrl-F5",
                    Action::Sort(p, Sort::Time),
                    s == Sort::Time,
                ),
                item(
                    "~Size",
                    "Ctrl-F6",
                    Action::Sort(p, Sort::Size),
                    s == Sort::Size,
                ),
                item(
                    "~Unsorted",
                    "Ctrl-F7",
                    Action::Sort(p, Sort::Unsorted),
                    s == Sort::Unsorted,
                ),
                separator(),
                item("~Re-read", "", Action::Reread(p), false),
                item("Fi~lter...", "", Action::Filter(p), false),
                item(
                    "~Drive...",
                    &format!("Alt-{}", fkey),
                    Action::Drive(p),
                    false,
                ),
            ]
        }
        1 => vec![
            item("~Help", "F1", Action::Help, false),
            item("~User menu", "F2", Action::Missing, false),
            item("~View", "F3", Action::ViewFile, false),
            item("~Edit", "F4", Action::Edit, false),
            item("~Copy", "F5", Action::Copy, false),
            item("~Rename or move", "F6", Action::Move, false),
            item("~Make directory", "F7", Action::Mkdir, false),
            item("~Delete", "F8", Action::Delete, false),
            separator(),
            item("File ~attributes", "", Action::Attributes, false),
            separator(),
            item("Select ~group", "Gray +", Action::SelectGroup, false),
            item("Deselec~t group", "Gray -", Action::DeselectGroup, false),
            item("~Invert  selection", "Gray *", Action::Invert, false),
            item("Re~store selection", "", Action::Restore, false),
            separator(),
            item("~Quit", "F10", Action::Quit, false),
        ],
        2 => vec![
            item("~NCD tree", "Alt-F10", Action::Ncd, false),
            item("~Find file", "Alt-F7", Action::FindFile, false),
            item("~History", "Alt-F8", Action::History, false),
            item("~EGA lines", "Alt-F9", Action::Missing, false),
            item("System ~information", "", Action::Missing, false),
            separator(),
            item("~Swap panels", "Ctrl-U", Action::Swap, false),
            item("~Panels on/off", "Ctrl-O", Action::PanelsOnOff, false),
            item("~Compare directories", "", Action::Missing, false),
            separator(),
            item("~Terminal Emulation", "", Action::Missing, false),
            separator(),
            item("~Menu file edit", "", Action::Missing, false),
            item("E~xtension file edit", "", Action::Missing, false),
        ],
        _ => vec![
            item("~Configuration...", "", Action::Missing, false),
            item("~Editor...", "", Action::Missing, false),
            item("C~onfirmation...", "", Action::Missing, false),
            item("Comp~ression...", "", Action::Missing, false),
            separator(),
            item("~Auto menus", "", Action::AutoMenus, c.auto_menus),
            item("~Path prompt", "", Action::PathPrompt, c.path_prompt),
            item("~Key bar", "Ctrl-B", Action::KeyBar, c.key_bar),
            item("~Full screen", "", Action::FullScreen, c.full_screen),
            item("~Mini status", "", Action::MiniStatus, c.mini_status),
            item("C~lock", "", Action::Clock, c.clock),
            separator(),
            item("~Save setup", "Shift-F9", Action::Missing, false),
        ],
    }
}

/// Where a title is on the bar: its first letter's column.
pub fn title_col(menu: usize) -> usize {
    let mut at = 4;
    for t in TITLES.iter().take(menu) {
        at += t.len() + 4;
    }
    at
}

/// A menu's box: its column and width, whether it has a column for check
/// marks, and where the shortcuts go (from the box's edge).
fn shape(menu: usize, cols: usize) -> (usize, usize, bool, usize) {
    let (w, marks, keys) = match menu {
        1 => (31, false, 22),
        2 => (34, false, 24),
        3 => (33, true, 22),
        _ => (31, true, 21),
    };
    let x = (title_col(menu) - 2).min(cols.saturating_sub(w + 2));
    (x, w, marks, keys)
}

/// The menu bar and its state: the title chosen, and the item, once a
/// menu is pulled down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bar {
    pub menu: usize,
    pub item: Option<usize>,
}

/// Where a click on the bar or a menu lands.
pub enum Hit {
    Title(usize),
    Item(usize),
    Outside,
}

impl Bar {
    /// Pull the menu down, on its first item.
    pub fn open(&mut self, c: &Checks) {
        let list = items(self.menu, c);
        self.item = list.iter().position(|i| i.action != Action::Separator);
    }

    /// The next or previous item, past separators.
    pub fn step(&mut self, delta: isize, c: &Checks) {
        let list = items(self.menu, c);
        let Some(mut at) = self.item else { return };
        let n = list.len() as isize;
        loop {
            at = (at as isize + delta).rem_euclid(n) as usize;
            if list[at].action != Action::Separator {
                break;
            }
        }
        self.item = Some(at);
    }

    /// The item with a hot key, if one has it.
    pub fn hot_item(&self, key: u8, c: &Checks) -> Option<usize> {
        items(self.menu, c)
            .iter()
            .position(|i| hot(i.label).is_some_and(|h| h.eq_ignore_ascii_case(&key)))
    }

    pub fn hit(&self, col: usize, row: usize, cols: usize, c: &Checks) -> Hit {
        if row == 0 {
            for (m, title) in TITLES.iter().enumerate() {
                let x = title_col(m) - 2;
                if col >= x && col < x + title.len() + 4 {
                    return Hit::Title(m);
                }
            }
            return Hit::Outside;
        }
        if self.item.is_some() {
            let (x, w, _, _) = shape(self.menu, cols);
            let n = items(self.menu, c).len();
            if row >= 2 && row < 2 + n && col > x && col < x + w - 1 {
                return Hit::Item(row - 2);
            }
        }
        Hit::Outside
    }

    pub fn draw(&self, s: &mut Screen, c: &Checks) {
        s.fill(0, 0, s.cols, 1, b' ', BAR);
        for (m, t) in TITLES.iter().enumerate() {
            let x = title_col(m);
            let attr = if m == self.menu { BAR_SELECTED } else { BAR };
            s.str(0, x - 2, &format!("  {}  ", t), attr);
        }
        let Some(selected) = self.item else { return };
        let list = items(self.menu, c);
        let (x, w, marks, keys) = shape(self.menu, s.cols);
        let h = list.len() + 2;
        s.fill(1, x, w, h, b' ', BAR);
        s.frame(1, x, w, h, BAR, false);
        s.shadow(1, x, w, h);
        let label_x = x + if marks { 3 } else { 2 };
        for (i, it) in list.iter().enumerate() {
            let r = 2 + i;
            if it.action == Action::Separator {
                for col in label_x - 1..x + w - 2 {
                    s.set(r, col, crate::screen::line::H, BAR);
                }
                continue;
            }
            let (attr, hot_attr) = if i == selected {
                (SELECTED, SELECTED_HOT)
            } else {
                (ITEM, ITEM_HOT)
            };
            s.fill(r, x + 1, w - 3, 1, b' ', attr);
            if it.checked {
                s.set(r, x + 1, 0xFB, attr);
            }
            draw_label(s, r, label_x, it.label, attr, hot_attr);
            s.text(r, x + keys, &cp437::bytes(&it.shortcut), attr);
        }
    }
}

/// A label with its hot key (the letter after '~') in `hot_attr`.
fn draw_label(s: &mut Screen, row: usize, col: usize, label: &str, attr: u8, hot_attr: u8) {
    let mut at = col;
    let mut next_hot = false;
    for c in label.chars() {
        if c == '~' {
            next_hot = true;
            continue;
        }
        s.set(
            row,
            at,
            cp437::byte(c),
            if next_hot { hot_attr } else { attr },
        );
        next_hot = false;
        at += 1;
    }
}

/// A label's hot key.
fn hot(label: &str) -> Option<u8> {
    let i = label.find('~')?;
    label.as_bytes().get(i + 1).copied()
}

/// The menu a letter on the bar opens.
pub fn title_for(key: u8) -> Option<usize> {
    TITLES
        .iter()
        .position(|t| t.as_bytes()[0].eq_ignore_ascii_case(&key))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checks() -> Checks {
        Checks {
            views: [View::Brief; 2],
            sorts: [Sort::Name; 2],
            auto_menus: false,
            path_prompt: true,
            key_bar: true,
            full_screen: true,
            mini_status: true,
            clock: true,
        }
    }

    #[test]
    fn titles_sit_where_nc_has_them() {
        assert_eq!([0, 1, 2, 3, 4].map(title_col), [4, 12, 21, 33, 44]);
        assert_eq!(shape(0, 80).0, 2);
        assert_eq!(shape(1, 80).0, 10);
        assert_eq!(shape(2, 80).0, 19);
        assert_eq!(shape(3, 80).0, 31);
        assert_eq!(shape(4, 80).0, 42);
    }

    #[test]
    fn draws_the_left_menu() {
        let mut s = Screen::new(80, 25);
        let c = checks();
        let mut bar = Bar {
            menu: 0,
            item: None,
        };
        bar.open(&c);
        bar.draw(&mut s, &c);
        let row: Vec<u8> = (0..33).map(|i| s.get(2, i).0).collect();
        assert_eq!(&row[2..10], &cp437::bytes("│√ Brief"));
        assert_eq!(s.get(2, 5).1, SELECTED_HOT);
        assert_eq!(s.get(3, 5), (b'F', ITEM_HOT));
        let on_off: Vec<u8> = (23..30).map(|i| s.get(9, i).0).collect();
        assert_eq!(on_off, b"Ctrl-F1");
        bar.step(1, &c);
        assert_eq!(bar.item, Some(1));
        assert_eq!(bar.hot_item(b'x', &c), Some(10));
    }
}
