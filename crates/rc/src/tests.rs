//! RC against a file system in memory: what keys and clicks do to the
//! files and the screen.

use std::collections::BTreeMap;

use crate::host::{Drive, DriveKind, Entry, Host, Input, Mouse, Outcome, UserScreen, attr};
use crate::{Commander, cp437, path};

#[derive(Clone, Debug)]
struct Node {
    /// None for a directory.
    data: Option<Vec<u8>>,
    attr: u8,
    stamp: (u16, u16),
}

/// Drives C: (writable) and D: (read-only), and E: (writable) for moves
/// across drives.
#[derive(Default)]
struct Memory {
    nodes: BTreeMap<String, Node>,
    current: String,
    history: Vec<String>,
    now: u64,
}

impl Memory {
    fn new() -> Memory {
        let mut m = Memory {
            current: "C:\\".to_string(),
            ..Memory::default()
        };
        for root in ["C:\\", "D:\\", "E:\\"] {
            m.dir(root);
        }
        m
    }

    fn dir(&mut self, p: &str) {
        self.nodes.insert(
            p.to_ascii_uppercase(),
            Node {
                data: None,
                attr: attr::DIRECTORY,
                stamp: (0, 0),
            },
        );
    }

    fn file(&mut self, p: &str, data: &[u8]) {
        self.nodes.insert(
            p.to_ascii_uppercase(),
            Node {
                data: Some(data.to_vec()),
                attr: attr::ARCHIVE,
                stamp: (0x5A00, 0x5D52),
            },
        );
    }

    fn has(&self, p: &str) -> bool {
        self.nodes.contains_key(&p.to_ascii_uppercase())
    }

    fn data(&self, p: &str) -> Option<&[u8]> {
        self.nodes
            .get(&p.to_ascii_uppercase())
            .and_then(|n| n.data.as_deref())
    }

    fn writable(p: &str) -> Result<(), u8> {
        if path::drive(p) == 3 {
            Err(0x13)
        } else {
            Ok(())
        }
    }

    fn children(&self, dir: &str) -> Vec<(String, Node)> {
        let dir = dir.to_ascii_uppercase();
        self.nodes
            .iter()
            .filter(|(k, _)| path::parent(k).is_some_and(|p| p == dir))
            .map(|(k, n)| (k.clone(), n.clone()))
            .collect()
    }
}

impl Host for Memory {
    fn drives(&self) -> Vec<Drive> {
        [2, 3, 4]
            .map(|d| Drive {
                drive: d,
                kind: DriveKind::Hard,
                writable: d != 3,
            })
            .to_vec()
    }

    fn current_drive(&self) -> u8 {
        path::drive(&self.current)
    }

    fn current_dir(&self, drive: u8) -> Option<String> {
        if path::drive(&self.current) == drive {
            Some(self.current.clone())
        } else {
            Some(path::root(drive))
        }
    }

    fn set_current(&mut self, dir: &str) {
        self.current = dir.to_string();
    }

    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, u8> {
        match self.nodes.get(&dir.to_ascii_uppercase()) {
            Some(n) if n.data.is_none() => {}
            _ => return Err(0x03),
        }
        Ok(self
            .children(dir)
            .into_iter()
            .map(|(k, n)| Entry {
                name: path::name(&k).to_string(),
                size: n.data.as_ref().map_or(0, |d| d.len() as u64),
                attr: n.attr,
                time: n.stamp.0,
                date: n.stamp.1,
            })
            .collect())
    }

    fn read(&mut self, p: &str) -> Result<Vec<u8>, u8> {
        self.data(p).map(<[u8]>::to_vec).ok_or(0x02)
    }

    fn write(&mut self, p: &str, data: &[u8], stamp: (u16, u16)) -> Result<(), u8> {
        Self::writable(p)?;
        let parent = path::parent(p).ok_or(0x03)?;
        if !self.has(&parent) {
            return Err(0x03);
        }
        if let Some(n) = self.nodes.get(&p.to_ascii_uppercase())
            && n.attr & attr::READ_ONLY != 0
        {
            return Err(0x05);
        }
        self.nodes.insert(
            p.to_ascii_uppercase(),
            Node {
                data: Some(data.to_vec()),
                attr: attr::ARCHIVE,
                stamp,
            },
        );
        Ok(())
    }

    fn delete(&mut self, p: &str) -> Result<(), u8> {
        Self::writable(p)?;
        match self.nodes.get(&p.to_ascii_uppercase()) {
            Some(n) if n.data.is_some() && n.attr & attr::READ_ONLY == 0 => {}
            Some(_) => return Err(0x05),
            None => return Err(0x02),
        }
        self.nodes.remove(&p.to_ascii_uppercase());
        Ok(())
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), u8> {
        Self::writable(to)?;
        if path::drive(from) != path::drive(to) {
            return Err(0x11);
        }
        if self.has(to) {
            return Err(0x05);
        }
        let (from, to) = (from.to_ascii_uppercase(), to.to_ascii_uppercase());
        let moved: Vec<String> = self
            .nodes
            .keys()
            .filter(|k| path::within(k, &from))
            .cloned()
            .collect();
        for k in moved {
            let n = self.nodes.remove(&k).unwrap();
            self.nodes.insert(format!("{}{}", to, &k[from.len()..]), n);
        }
        Ok(())
    }

    fn mkdir(&mut self, p: &str) -> Result<(), u8> {
        Self::writable(p)?;
        if self.has(p) {
            return Err(0x05);
        }
        self.dir(p);
        Ok(())
    }

    fn rmdir(&mut self, p: &str) -> Result<(), u8> {
        Self::writable(p)?;
        if !self.children(p).is_empty() {
            return Err(0x05);
        }
        self.nodes
            .remove(&p.to_ascii_uppercase())
            .map(|_| ())
            .ok_or(0x03)
    }

    fn attributes(&self, p: &str) -> Result<u8, u8> {
        self.nodes
            .get(&p.to_ascii_uppercase())
            .map(|n| n.attr)
            .ok_or(0x02)
    }

    fn set_attributes(&mut self, p: &str, a: u8) -> Result<(), u8> {
        let n = self.nodes.get_mut(&p.to_ascii_uppercase()).ok_or(0x02)?;
        n.attr = (n.attr & attr::DIRECTORY) | a;
        Ok(())
    }

    fn space(&self, _drive: u8) -> Option<(u64, u64)> {
        Some((1_000_000, 500_000))
    }

    fn label(&self, _drive: u8) -> Option<String> {
        Some("TEST".to_string())
    }

    fn memory(&mut self) -> (u64, u64) {
        (655_360, 600_000)
    }

    fn prompt(&self) -> Vec<u8> {
        format!("{}>", self.current).into_bytes()
    }

    fn history(&self) -> Vec<String> {
        self.history.clone()
    }

    fn now_ms(&self) -> u64 {
        self.now
    }

    fn clock(&self) -> (u8, u8, u8) {
        (12, 0, 0)
    }
}

/// A machine with some files, and RC started in C:\.
fn setup() -> (Memory, Commander) {
    let mut m = Memory::new();
    m.dir("C:\\GAMES");
    m.dir("C:\\GAMES\\SUB");
    m.file("C:\\GAMES\\SUB\\DEEP.DAT", b"deep");
    m.file("C:\\GAMES\\PLAY.EXE", b"MZ");
    m.file("C:\\README.TXT", b"hello\r\nworld\r\n");
    m.file("C:\\NOTES.DOC", b"notes");
    m.dir("C:\\TARGET");
    let rc = Commander::new(
        &mut m,
        80,
        25,
        UserScreen::new(80, 25, vec![(b' ', 7); 2000], 24, 23),
    );
    (m, rc)
}

fn input(key: Option<u16>) -> Input {
    Input {
        key,
        cols: 80,
        rows: 25,
        ..Input::default()
    }
}

/// Keys as INT 16h has them.
mod k {
    pub const ENTER: u16 = 0x1C0D;
    pub const ESC: u16 = 0x011B;
    pub const TAB: u16 = 0x0F09;
    pub const DOWN: u16 = 0x5000;
    pub const HOME: u16 = 0x4700;
    pub const END: u16 = 0x4F00;
    pub const INS: u16 = 0x5200;
    pub const F3: u16 = 0x3D00;
    pub const F4: u16 = 0x3E00;
    pub const F5: u16 = 0x3F00;
    pub const F6: u16 = 0x4000;
    pub const F7: u16 = 0x4100;
    pub const F8: u16 = 0x4200;
    pub const F9: u16 = 0x4300;
    pub const F10: u16 = 0x4400;
    pub const CTRL_Y: u16 = 0x1519;
    pub const CTRL_F6: u16 = 0x6300;
    pub const GRAY_PLUS: u16 = 0x4E2B;
}

/// Keys pressed, the operation they start run to its end; the last
/// outcome.
fn press(m: &mut Memory, rc: &mut Commander, keys: &[u16]) -> Outcome {
    let mut out = Outcome::Stay { redraw: false };
    for &key in keys {
        out = rc.step(m, &input(Some(key)));
        for _ in 0..50 {
            rc.step(m, &input(None));
        }
    }
    out
}

fn typed(text: &str) -> Vec<u16> {
    text.bytes().map(|b| b as u16).collect()
}

fn row(rc: &Commander, r: usize) -> String {
    let s = rc.screen();
    cp437::string(&(0..s.cols).map(|c| s.get(r, c).0).collect::<Vec<u8>>())
}

fn screen_text(rc: &Commander) -> String {
    (0..25).map(|r| row(rc, r)).collect::<Vec<_>>().join("\n")
}

/// The cursor onto a name in the active (left) panel.
fn select(m: &mut Memory, rc: &mut Commander, name: &str) {
    press(m, rc, &[k::HOME]);
    for _ in 0..20 {
        if rc.panels[rc.active]
            .current()
            .is_some_and(|i| i.entry.name.eq_ignore_ascii_case(name))
        {
            return;
        }
        press(m, rc, &[k::DOWN]);
    }
    panic!("no {} in the panel", name);
}

/// Replace a dialog's field with `text`, and Enter.
fn answer(m: &mut Memory, rc: &mut Commander, text: &str) {
    press(m, rc, &[k::CTRL_Y]);
    press(m, rc, &typed(text));
    press(m, rc, &[k::ENTER]);
}

#[test]
fn starts_with_two_panels_like_nc() {
    let (_, rc) = setup();
    assert!(row(&rc, 0).starts_with("╔════════════╤═══ C:\\ ════╤════════════╗╔"));
    assert!(row(&rc, 2).starts_with("║GAMES       │"));
    assert!(row(&rc, 23).starts_with("C:\\>"));
    assert!(row(&rc, 24).starts_with("1Help   2Menu   3View   4Edit   5Copy   6RenMov"));
    // The cursor bar on the active panel only.
    assert_eq!(rc.screen().get(2, 1).1, 0x30);
    assert_eq!(rc.screen().get(2, 41).1, 0x1B);
    assert_eq!(rc.screen().get(1, 5), (b'N', 0x1E));
}

#[test]
fn enters_directories_and_comes_back_onto_them() {
    let (mut m, mut rc) = setup();
    select(&mut m, &mut rc, "GAMES");
    press(&mut m, &mut rc, &[k::ENTER]);
    assert_eq!(rc.panels[0].dir, "C:\\GAMES");
    assert_eq!(m.current, "C:\\GAMES", "the DOS directory follows");
    assert!(row(&rc, 2).starts_with("║..          │"));
    press(&mut m, &mut rc, &[k::HOME, k::ENTER]);
    assert_eq!(rc.panels[0].dir, "C:\\");
    assert_eq!(rc.panels[0].current().unwrap().entry.name, "GAMES");
}

#[test]
fn marks_and_counts_the_bytes() {
    let (mut m, mut rc) = setup();
    select(&mut m, &mut rc, "NOTES.DOC");
    press(&mut m, &mut rc, &[k::INS, k::INS]);
    assert_eq!(rc.panels[0].marked_count(), 2);
    assert!(
        row(&rc, 21).contains("19 bytes in 2 selected files"),
        "{}",
        row(&rc, 21)
    );
    // Marked yellow, the cursor's black on cyan.
    assert_eq!(rc.screen().get(4, 1).1, 0x1E);
    assert_eq!(rc.screen().get(5, 1).1, 0x3E);
    // Gray + with *.TXT marks only the text file.
    press(&mut m, &mut rc, &[k::INS]);
    let mut keys = vec![k::GRAY_PLUS, k::CTRL_Y];
    keys.extend(typed("*.txt"));
    keys.push(k::ENTER);
    let mut fresh = setup();
    press(&mut fresh.0, &mut fresh.1, &keys);
    assert_eq!(fresh.1.panels[0].marked_names(), ["README.TXT"]);
}

#[test]
fn sorts_by_size_with_ctrl_f6() {
    let (mut m, mut rc) = setup();
    press(&mut m, &mut rc, &[k::CTRL_F6]);
    let names: Vec<_> = rc.panels[0]
        .items
        .iter()
        .map(|i| i.entry.name.as_str())
        .collect();
    assert_eq!(names, ["GAMES", "TARGET", "README.TXT", "NOTES.DOC"]);
}

#[test]
fn copies_a_directory_tree_keeping_the_stamps() {
    let (mut m, mut rc) = setup();
    select(&mut m, &mut rc, "GAMES");
    press(&mut m, &mut rc, &[k::F5]);
    assert!(screen_text(&rc).contains("Copy \"GAMES\" to"));
    answer(&mut m, &mut rc, "c:\\target");
    assert_eq!(
        m.data("C:\\TARGET\\GAMES\\SUB\\DEEP.DAT"),
        Some(&b"deep"[..])
    );
    assert_eq!(m.data("C:\\TARGET\\GAMES\\PLAY.EXE"), Some(&b"MZ"[..]));
    let stamp = m.nodes["C:\\TARGET\\GAMES\\PLAY.EXE"].stamp;
    assert_eq!(stamp, (0x5A00, 0x5D52));
    assert!(matches!(rc.mode, crate::Mode::Panels));
}

#[test]
fn refuses_to_copy_a_directory_into_itself() {
    let (mut m, mut rc) = setup();
    select(&mut m, &mut rc, "GAMES");
    press(&mut m, &mut rc, &[k::F5]);
    answer(&mut m, &mut rc, "c:\\games\\sub");
    assert!(screen_text(&rc).contains("You cannot copy a directory into itself"));
    assert!(!m.has("C:\\GAMES\\SUB\\GAMES"));
}

#[test]
fn asks_before_writing_over_a_file() {
    let (mut m, mut rc) = setup();
    m.file("C:\\TARGET\\README.TXT", b"old");
    m.file("C:\\TARGET\\NOTES.DOC", b"old");
    select(&mut m, &mut rc, "NOTES.DOC");
    press(&mut m, &mut rc, &[k::INS, k::INS]);
    press(&mut m, &mut rc, &[k::F5]);
    assert!(screen_text(&rc).contains("Copy 2 files to"));
    answer(&mut m, &mut rc, "c:\\target");
    let text = screen_text(&rc);
    assert!(text.contains("The file C:\\TARGET\\notes.doc"), "{}", text);
    assert!(text.contains("Overwrite   All   Skip   Cancel"));
    // Skip the first, overwrite the second.
    press(&mut m, &mut rc, &[b's' as u16]);
    assert!(screen_text(&rc).contains("readme.txt"));
    press(&mut m, &mut rc, &[b'o' as u16]);
    assert_eq!(m.data("C:\\TARGET\\NOTES.DOC"), Some(&b"old"[..]));
    assert_eq!(
        m.data("C:\\TARGET\\README.TXT"),
        Some(&b"hello\r\nworld\r\n"[..])
    );
    assert_eq!(rc.panels[0].marked_count(), 0);
}

#[test]
fn moves_on_a_drive_and_across_drives() {
    let (mut m, mut rc) = setup();
    select(&mut m, &mut rc, "GAMES");
    press(&mut m, &mut rc, &[k::F6]);
    answer(&mut m, &mut rc, "c:\\target");
    assert!(m.has("C:\\TARGET\\GAMES\\SUB\\DEEP.DAT"));
    assert!(!m.has("C:\\GAMES"));
    // Across drives: copied, then the source deleted.
    select(&mut m, &mut rc, "README.TXT");
    press(&mut m, &mut rc, &[k::F6]);
    answer(&mut m, &mut rc, "e:\\");
    assert!(m.has("E:\\README.TXT"));
    assert!(!m.has("C:\\README.TXT"));
    // A rename in place leaves the cursor on the new name.
    select(&mut m, &mut rc, "NOTES.DOC");
    press(&mut m, &mut rc, &[k::F6]);
    answer(&mut m, &mut rc, "letter.doc");
    assert!(m.has("C:\\LETTER.DOC"));
    assert_eq!(rc.panels[0].current().unwrap().entry.name, "LETTER.DOC");
}

#[test]
fn deletes_after_asking_about_a_full_directory() {
    let (mut m, mut rc) = setup();
    select(&mut m, &mut rc, "GAMES");
    press(&mut m, &mut rc, &[k::F8]);
    assert!(screen_text(&rc).contains("Do you wish to delete"));
    press(&mut m, &mut rc, &[k::ENTER]);
    assert!(screen_text(&rc).contains("The following directory will be DELETED:"));
    assert!(m.has("C:\\GAMES"));
    press(&mut m, &mut rc, &[k::ENTER]);
    assert!(!m.has("C:\\GAMES"));
    assert!(!m.has("C:\\GAMES\\SUB\\DEEP.DAT"));
}

#[test]
fn reports_a_write_protected_drive() {
    let (mut m, mut rc) = setup();
    select(&mut m, &mut rc, "README.TXT");
    press(&mut m, &mut rc, &[k::F5]);
    answer(&mut m, &mut rc, "d:\\");
    assert!(screen_text(&rc).contains("write-protected"));
    assert!(!m.has("D:\\README.TXT"));
}

#[test]
fn makes_a_directory_and_puts_the_cursor_on_it() {
    let (mut m, mut rc) = setup();
    press(&mut m, &mut rc, &[k::F7]);
    answer(&mut m, &mut rc, "newdir");
    assert!(m.has("C:\\NEWDIR"));
    assert_eq!(rc.panels[0].current().unwrap().entry.name, "NEWDIR");
}

#[test]
fn views_text_and_hex() {
    let (mut m, mut rc) = setup();
    select(&mut m, &mut rc, "README.TXT");
    press(&mut m, &mut rc, &[k::F3]);
    assert!(row(&rc, 0).starts_with("Text View: C:\\readme.txt"));
    assert!(row(&rc, 1).starts_with("hello"));
    assert!(row(&rc, 24).contains("2Unwrap"));
    press(&mut m, &mut rc, &[k::F4]);
    assert!(row(&rc, 1).starts_with("00000  68 65 6C 6C"));
    press(&mut m, &mut rc, &[k::ESC]);
    assert!(row(&rc, 0).starts_with("╔"));
}

#[test]
fn runs_command_lines_programs_and_the_editor() {
    let (mut m, mut rc) = setup();
    let out = press(&mut m, &mut rc, &typed("dir /w"));
    assert_eq!(out, Outcome::Stay { redraw: true });
    assert!(row(&rc, 23).starts_with("C:\\>dir /w"));
    let out = press(&mut m, &mut rc, &[k::ENTER]);
    assert_eq!(
        out,
        Outcome::Run {
            line: "dir /w".to_string(),
            echo: true
        }
    );
    assert!(rc.cmdline.is_empty());
    select(&mut m, &mut rc, "GAMES");
    press(&mut m, &mut rc, &[k::ENTER]);
    select(&mut m, &mut rc, "PLAY.EXE");
    let out = press(&mut m, &mut rc, &[k::ENTER]);
    assert_eq!(
        out,
        Outcome::Run {
            line: "play.exe".to_string(),
            echo: true
        }
    );
    let out = press(&mut m, &mut rc, &[k::F4]);
    assert_eq!(
        out,
        Outcome::Run {
            line: "EDIT C:\\GAMES\\PLAY.EXE".to_string(),
            echo: false
        }
    );
}

#[test]
fn quits_after_asking() {
    let (mut m, mut rc) = setup();
    press(&mut m, &mut rc, &[k::F10]);
    assert!(screen_text(&rc).contains("Do you want to quit the Rat Commander?"));
    assert_eq!(press(&mut m, &mut rc, &[b'y' as u16]), Outcome::Quit);
}

#[test]
fn menus_change_the_panels() {
    let (mut m, mut rc) = setup();
    // F9, Enter: the Left menu; F for Full.
    press(&mut m, &mut rc, &[k::F9, k::ENTER]);
    assert!(row(&rc, 2).contains("√ Brief"));
    press(&mut m, &mut rc, &[b'f' as u16]);
    assert_eq!(rc.panels[0].view, crate::panel::View::Full);
    assert!(row(&rc, 1).contains("Size"));
    // The right panel's Info, through the bar's letter.
    press(&mut m, &mut rc, &[k::F9, b'r' as u16, b'i' as u16]);
    assert!(row(&rc, 0).contains(" Info "));
    assert!(screen_text(&rc).contains("655,360 Bytes Memory"));
    // Tab doesn't go to an Info panel.
    press(&mut m, &mut rc, &[k::TAB]);
    assert_eq!(rc.active, 0);
}

#[test]
fn the_tree_panel_moves_the_other_panel() {
    let (mut m, mut rc) = setup();
    press(&mut m, &mut rc, &[k::F9, k::ENTER, b't' as u16]);
    assert!(screen_text(&rc).contains("├──GAMES"));
    assert!(screen_text(&rc).contains("│  └──SUB"));
    // The left panel is the tree, and active.
    press(&mut m, &mut rc, &[k::DOWN]);
    assert_eq!(rc.panels[1].dir, "C:\\GAMES");
    press(&mut m, &mut rc, &[k::END]);
    assert_eq!(rc.panels[1].dir, "C:\\TARGET");
}

#[test]
fn the_mouse_clicks_files_menus_and_keys() {
    let (mut m, mut rc) = setup();
    let mut mouse = Mouse::default();
    let mut click = |m: &mut Memory, rc: &mut Commander, button: usize, col: usize, row: usize| {
        mouse.presses[button] += 1;
        mouse.pressed_at[button] = (col, row);
        m.now += 1000;
        rc.step(
            m,
            &Input {
                mouse,
                ..input(None)
            },
        )
    };
    rc.step(&mut m, &input(None));
    // A click on TARGET, the second row.
    click(&mut m, &mut rc, 0, 5, 3);
    assert_eq!(rc.panels[0].cursor, 1);
    // A right click marks.
    click(&mut m, &mut rc, 1, 5, 4);
    assert_eq!(rc.panels[0].marked_count(), 1);
    // Two clicks in a row on a directory go into it.
    m.now = 0;
    let mut mouse2 = Mouse {
        presses: [10, 0],
        ..Mouse::default()
    };
    rc.step(
        &mut m,
        &Input {
            mouse: mouse2,
            ..input(None)
        },
    );
    for _ in 0..2 {
        mouse2.presses[0] += 1;
        mouse2.pressed_at[0] = (5, 2);
        m.now += 100;
        rc.step(
            &mut m,
            &Input {
                mouse: mouse2,
                ..input(None)
            },
        );
    }
    assert_eq!(rc.panels[0].dir, "C:\\GAMES");
    // The key bar: 10Quit asks.
    mouse2.presses[0] += 1;
    mouse2.pressed_at[0] = (76, 24);
    m.now += 1000;
    rc.step(
        &mut m,
        &Input {
            mouse: mouse2,
            ..input(None)
        },
    );
    assert!(screen_text(&rc).contains("Do you want to quit"));
}
