//! WinHelp: the help viewer WinHelpA opens on a program's .HLP file.
//!
//! Windows runs WINHELP.EXE for it; here the viewer is built in, after
//! Wine's winhlp32 (LGPL-2.1+), whose file reading `hlpfile` ports. Its
//! window (class MS_WINHELP) has a row of buttons, Contents, Back and
//! the browse buttons << and >>, over the topic (MS_WINTOPIC), which
//! scrolls. A click on a jump goes to its topic; a click on a popup link
//! shows its topic in a window of its own (MS_WINPOPUP) until the next
//! click or key.
//!
//! What the viewer keeps is in `Help`, out of save states: a viewer
//! open when a state is saved is an empty window after it is loaded.

pub mod hlpfile;
pub mod layout;

use std::collections::HashMap;

use hlpfile::{HelpFile, Target};
use layout::{Layout, Measure};

use crate::cpu::Cpu;
use crate::win32::gdi32::{self, Rect};
use crate::win32::kernel32;
use crate::win32::task::{self, Next, Task};
use crate::win32::user32::{DLL, address_of, defwnd, paint, string_at, user};

const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
const WS_POPUP: u32 = 0x8000_0000;
const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_CLIPCHILDREN: u32 = 0x0200_0000;
const WS_BORDER: u32 = 0x0080_0000;
const WS_VSCROLL: u32 = 0x0020_0000;
const WS_EX_TOPMOST: u32 = 0x0000_0008;
const CW_USEDEFAULT: u32 = 0x8000_0000;
const SW_SHOWNORMAL: u32 = 1;
const SW_SHOWNA: u32 = 8;
const SW_RESTORE: u32 = 9;
const SB_VERT: u32 = 1;
const SIF_RANGE: u32 = 0x01;
const SIF_PAGE: u32 = 0x02;
const SIF_POS: u32 = 0x04;
const SIF_DISABLENOSCROLL: u32 = 0x08;
const COLOR_WINDOW: u32 = 5;

const WM_CREATE: u32 = 0x0001;
const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_SETFOCUS: u32 = 0x0007;
const WM_PAINT: u32 = 0x000F;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_SETCURSOR: u32 = 0x0020;
const WM_MOUSEACTIVATE: u32 = 0x0021;
const WM_KEYDOWN: u32 = 0x0100;
const WM_COMMAND: u32 = 0x0111;
const WM_VSCROLL: u32 = 0x0115;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_MOUSEWHEEL: u32 = 0x020A;
const WM_CAPTURECHANGED: u32 = 0x0215;
const MA_NOACTIVATE: u32 = 3;
const HTCLIENT: u32 = 1;

const VK_PRIOR: u32 = 0x21;
const VK_NEXT: u32 = 0x22;
const VK_END: u32 = 0x23;
const VK_HOME: u32 = 0x24;
const VK_UP: u32 = 0x26;
const VK_DOWN: u32 = 0x28;
const VK_BACK: u32 = 0x08;

/// WinHelpA's commands.
const HELP_CONTEXT: u32 = 0x0001;
const HELP_QUIT: u32 = 0x0002;
const HELP_SETCONTENTS: u32 = 0x0005;
const HELP_CONTEXTPOPUP: u32 = 0x0008;
const HELP_COMMAND: u32 = 0x0102;
const HELP_HELPONHELP: u32 = 0x0004;
const HELP_SETPOPUP_POS: u32 = 0x000D;
const HELP_WM_HELP: u32 = 0x000C;
/// The flag a training card's call adds to its command.
const HELP_TCARD: u32 = 0x8000;

/// The viewer's classes, as the system's classes register them.
pub const FRAME_CLASS: &str = "MS_WINHELP";
pub const TOPIC_CLASS: &str = "MS_WINTOPIC";
pub const POPUP_CLASS: &str = "MS_WINPOPUP";

/// The buttons: label and command, in order, and their size.
const BUTTONS: [&str; 4] = ["&Contents", "&Back", "&<<", "&>>"];
const ID_CONTENTS: u32 = 1;
const ID_BACK: u32 = 2;
const ID_PREVIOUS: u32 = 3;
const ID_NEXT: u32 = 4;
const ID_TOPIC: u32 = 100;
const BUTTON_WIDTH: i32 = 64;
const BUTTON_HEIGHT: i32 = 22;
/// The height of the row of buttons.
const BAR: i32 = BUTTON_HEIGHT + 6;
/// The space around a topic's text.
const MARGIN: i32 = 6;
/// The width a popup's text is laid out in.
const POPUP_WIDTH: i32 = 360;
/// A jump's colour.
const GREEN: u32 = 0x00_8000;
/// The cursor over a link, and elsewhere.
const IDC_HAND: u32 = 32649;
const IDC_ARROW: u32 = 32512;

/// The viewer task's kinds, its first variable.
const K_OPEN: u32 = 0;
const K_SHOW: u32 = 1;
const K_PAINT: u32 = 2;
const K_SIZE: u32 = 3;
const K_SCROLL: u32 = 4;
const K_POPUP: u32 = 5;
const K_CLOSE_POPUP: u32 = 6;
const K_QUIT: u32 = 7;
const K_FOCUS: u32 = 8;

/// The scratch space of the task: strings, a PAINTSTRUCT, a SCROLLINFO.
const SCRATCH: u32 = 256;
const S_TEXT: u32 = 32;
const S_INFO: u32 = 160;

/// What the viewer keeps.
#[derive(Debug, Default)]
pub struct Help {
    pub viewer: Option<Viewer>,
    /// Where HELP_SETPOPUP_POS put the next popup, on the screen.
    popup_at: Option<(i32, i32)>,
}

/// The open viewer.
#[derive(Debug)]
pub struct Viewer {
    frame: u32,
    topic: u32,
    popup: u32,
    path: String,
    file: HelpFile,
    page: usize,
    /// The topic Contents goes to, if the program set one.
    contents: Option<usize>,
    /// The topics shown before, for Back.
    history: Vec<usize>,
    scroll: i32,
    layout: Layout,
    /// The width the topic was laid out in.
    width: i32,
    popup_layout: Layout,
    /// The fonts of the file's fonts, made when they are first used.
    fonts: Vec<u32>,
    /// The pictures, read when first shown.
    pictures: HashMap<u16, Option<Shown>>,
}

/// A picture read for showing: its size, and where its packed DIB and
/// the bits after the DIB's header are in memory.
#[derive(Clone, Copy, Debug)]
struct Shown {
    width: i32,
    height: i32,
    at: u32,
    bits: u32,
}

fn viewer(cpu: &mut Cpu) -> Option<&mut Viewer> {
    user(cpu).help.viewer.as_mut()
}

/// The file `name`, read whole.
fn read_file(cpu: &mut Cpu, name: &str) -> Option<Vec<u8>> {
    let at = kernel32::alloc_pages(cpu, name.len() as u32 + 1)?;
    let mut bytes = name.as_bytes().to_vec();
    bytes.push(0);
    cpu.bus.guest_write_bytes(at, &bytes);
    let read = kernel32::read_whole_file(cpu, at);
    kernel32::free_pages(cpu, at);
    let (data, size) = read??;
    let mut out = vec![0u8; size as usize];
    cpu.bus.guest_read_bytes(data, &mut out);
    kernel32::free_pages(cpu, data);
    Some(out)
}

/// WinHelpA(hwnd, file, command, data).
pub fn win_help(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (file, command, data) = (args[1], args[2] & !HELP_TCARD, args[3]);
    if command == HELP_QUIT {
        return match viewer(cpu) {
            Some(v) if v.frame != 0 => {
                let frame = v.frame;
                task::start(cpu, task::WINHELP, vec![K_QUIT, frame], 0)
            }
            _ => Some(1),
        };
    }
    // HELP_SETPOPUP_POS only places the next popup: it shows nothing.
    if command == HELP_SETPOPUP_POS {
        user(cpu).help.popup_at = Some((data as i16 as i32, (data >> 16) as i16 as i32));
        return Some(1);
    }
    if matches!(command, HELP_COMMAND | HELP_HELPONHELP) {
        return Some(1);
    }
    let name = if file == 0 {
        None
    } else {
        string_at(cpu, file)
    };
    let Some(name) = name else {
        return Some(0);
    };
    // "file>window" names a window of the file's.
    let name = name.split('>').next().unwrap_or("").trim().to_string();
    let loaded = viewer(cpu).is_some_and(|v| v.path.eq_ignore_ascii_case(&name));
    if !loaded {
        let Some(data) = read_file(cpu, &name) else {
            return Some(0);
        };
        let Ok(help) = HelpFile::parse(data) else {
            return Some(0);
        };
        let old = user(cpu).help.viewer.take();
        let (frame, topic, popup) = old
            .as_ref()
            .map_or((0, 0, 0), |v| (v.frame, v.topic, v.popup));
        if let Some(old) = old {
            free(cpu, old);
        }
        user(cpu).help.viewer = Some(Viewer {
            frame,
            topic,
            popup,
            path: name,
            fonts: vec![0; help.fonts.len()],
            file: help,
            page: 0,
            contents: None,
            history: Vec::new(),
            scroll: 0,
            layout: Layout::default(),
            width: -1,
            popup_layout: Layout::default(),
            pictures: HashMap::new(),
        });
    }
    let v = viewer(cpu)?;
    let contents = v.contents.unwrap_or_else(|| v.file.contents());
    let page = match command {
        HELP_CONTEXT => v.file.page_by_map(data).unwrap_or(contents),
        HELP_CONTEXTPOPUP => {
            let page = v.file.page_by_map(data).unwrap_or(contents);
            return show_popup(cpu, page);
        }
        HELP_WM_HELP => {
            // `data` is the program's list of control ids and their
            // context numbers, ended by a 0; the window is the control.
            let id = user(cpu).window(args[0]).map_or(0, |w| w.id);
            let number = (0..1024)
                .map(|i| {
                    let at = data.wrapping_add(i * 8);
                    (cpu.bus.guest_read_32(at), cpu.bus.guest_read_32(at + 4))
                })
                .take_while(|&(c, _)| c != 0)
                .find(|&(c, _)| c == id)
                .map(|(_, n)| n);
            let v = viewer(cpu)?;
            return match number.and_then(|n| v.file.page_by_map(n)) {
                Some(page) => show_popup(cpu, page),
                None => Some(1),
            };
        }
        HELP_SETCONTENTS => {
            v.contents = v.file.page_by_map(data);
            // A file new to the open window shows its contents there:
            // the topic it showed was the old file's.
            if loaded || v.frame == 0 {
                return Some(1);
            }
            v.contents.unwrap_or_else(|| v.file.contents())
        }
        _ => contents,
    };
    if loaded && v.page != page {
        let old = v.page;
        v.history.push(old);
    }
    v.page = page;
    v.scroll = 0;
    v.width = -1;
    let kind = if v.frame == 0 { K_OPEN } else { K_SHOW };
    // Another file in the open viewer: its window takes the file's title.
    let retitle = !loaded as u32;
    task::start(cpu, task::WINHELP, vec![kind, 0, retitle], SCRATCH)
}

/// Show the topic `page` in a popup, where HELP_SETPOPUP_POS put it or
/// at the mouse.
fn show_popup(cpu: &mut Cpu, page: usize) -> Option<u32> {
    let (x, y) = match user(cpu).help.popup_at.take() {
        Some(at) => at,
        None => user(cpu).input.cursor,
    };
    let vars = vec![K_POPUP, page as u32, x as u32, y as u32];
    task::start(cpu, task::WINHELP, vars, SCRATCH)
}

/// Give back what the viewer `v` made.
fn free(cpu: &mut Cpu, v: Viewer) {
    for font in v.fonts {
        if font != 0 {
            gdi32::delete_object(cpu, &[font]);
        }
    }
    for p in v.pictures.into_values().flatten() {
        kernel32::free_pages(cpu, p.at);
    }
}

/// The font of the file's font `n`, made if it isn't yet.
fn font(cpu: &mut Cpu, n: u16) -> u32 {
    let Some(v) = viewer(cpu) else {
        return 0;
    };
    let n = n as usize;
    if let Some(&f) = v.fonts.get(n)
        && f != 0
    {
        return f;
    }
    let Some(f) = v.file.fonts.get(n).cloned() else {
        return 0;
    };
    // The faces Windows 98 substitutes for Windows 3.0's.
    let face = match f.face.to_ascii_lowercase().as_str() {
        "helv" => "MS Sans Serif",
        "helvetica" => "Arial",
        "tms rmn" => "MS Serif",
        "times" => "Times New Roman",
        _ => f.face.as_str(),
    };
    let family = match f.family {
        1 => 0x30,
        2 => 0x10,
        3 => 0x20,
        4 => 0x40,
        5 => 0x50,
        _ => 0,
    };
    let height = -layout::pixels(f.half_points as i32).max(1);
    let weight = if f.bold { 700 } else { 400 };
    let mut log = gdi32::font::logfont(height, 0, weight, 0, family, face);
    log[20] = f.italic as u8;
    log[21] = f.underline as u8;
    log[22] = f.strike as u8;
    let handle = gdi32::add(
        cpu,
        gdi32::Kind::Font {
            font: gdi32::Font { log },
        },
    );
    if let Some(v) = viewer(cpu)
        && let Some(slot) = v.fonts.get_mut(n)
    {
        *slot = handle;
    }
    handle
}

/// Measuring text on a DC in the file's fonts.
struct Gdi<'a> {
    cpu: &'a mut Cpu,
    hdc: u32,
}

impl Measure for Gdi<'_> {
    fn width(&mut self, n: u16, text: &[u8]) -> i32 {
        let f = font(self.cpu, n);
        gdi32::dc::select_object(self.cpu, &[self.hdc, f]);
        gdi32::font::text_extent(self.cpu, self.hdc, text).map_or(0, |e| e.0)
    }

    fn height(&mut self, n: u16) -> (i32, i32) {
        let f = font(self.cpu, n);
        gdi32::dc::select_object(self.cpu, &[self.hdc, f]);
        gdi32::font::metrics(self.cpu, self.hdc).map_or((16, 13), |m| (m.height, m.ascent))
    }
}

/// The picture `n`, its DIB put in memory the first time.
fn picture(cpu: &mut Cpu, n: u16) -> Option<Shown> {
    let v = viewer(cpu)?;
    if let Some(p) = v.pictures.get(&n) {
        return *p;
    }
    let white = 0xFF_FFFF;
    let read = v.file.picture(n, white).and_then(|p| {
        let at = kernel32::alloc_pages(cpu, p.dib.len() as u32)?;
        cpu.bus.guest_write_bytes(at, &p.dib);
        let colors = p
            .dib
            .get(32..36)
            .map_or(0, |c| u32::from_le_bytes(c.try_into().unwrap_or([0; 4])));
        Some(Shown {
            width: p.width,
            height: p.height,
            at,
            bits: at + 40 + 4 * colors,
        })
    });
    viewer(cpu)?.pictures.insert(n, read);
    read
}

/// Lay the topic of page `page` out `width` wide, measuring with a DC
/// of `hwnd`.
fn lay_out(cpu: &mut Cpu, hwnd: u32, page: usize, width: i32) -> Layout {
    let Some(v) = viewer(cpu) else {
        return Layout::default();
    };
    let pars = v.file.paragraphs(page);
    let hdc = paint::get_dc(cpu, &[hwnd]).unwrap_or(0);
    if hdc == 0 {
        return Layout::default();
    }
    let first = font(cpu, 0);
    let old = gdi32::dc::select_object(cpu, &[hdc, first]).unwrap_or(0);
    let sizes = |cpu: &mut Cpu, n: u16| picture(cpu, n).map(|p| (p.width, p.height));
    let mut known: HashMap<u16, Option<(i32, i32)>> = HashMap::new();
    for par in &pars {
        for item in &par.items {
            if let hlpfile::Item::Picture(n) = item {
                known.entry(*n).or_insert_with(|| sizes(cpu, *n));
            }
        }
    }
    let mut measure = Gdi { cpu, hdc };
    let l = layout::layout(&pars, width, MARGIN, &mut measure, &mut |n| {
        known.get(&n).copied().flatten()
    });
    if old != 0 {
        gdi32::dc::select_object(cpu, &[hdc, old]);
    }
    paint::release_dc(cpu, &[hwnd, hdc]);
    l
}

/// The client size of `hwnd`.
fn client(cpu: &mut Cpu, hwnd: u32) -> (i32, i32) {
    user(cpu)
        .window(hwnd)
        .map_or((0, 0), |w| (w.client.width(), w.client.height()))
}

/// Lay the topic shown out again if the topic window's width changed,
/// and keep the scroll position in range.
fn refresh(cpu: &mut Cpu) {
    let Some(v) = viewer(cpu) else {
        return;
    };
    let (topic, page, laid) = (v.topic, v.page, v.width);
    let (w, h) = client(cpu, topic);
    // A minimized window has no width to lay the topic out in.
    if w != laid && w > 0 {
        let l = lay_out(cpu, topic, page, w);
        let Some(v) = viewer(cpu) else {
            return;
        };
        v.layout = l;
        v.width = w;
    }
    let Some(v) = viewer(cpu) else {
        return;
    };
    let most = (v.layout.height - h).max(0);
    v.scroll = v.scroll.clamp(0, most);
}

/// Write the topic's SCROLLINFO at `at` for SetScrollInfo.
fn scroll_info(cpu: &mut Cpu, at: u32) {
    let (topic, height, scroll) =
        viewer(cpu).map_or((0, 0, 0), |v| (v.topic, v.layout.height, v.scroll));
    let (_, h) = client(cpu, topic);
    let mask = SIF_RANGE | SIF_PAGE | SIF_POS | SIF_DISABLENOSCROLL;
    let words = [
        28,
        mask,
        0,
        height.max(1) as u32 - 1,
        h.max(0) as u32,
        scroll as u32,
        0,
    ];
    for (i, w) in words.iter().enumerate() {
        cpu.bus.guest_write_32(at + i as u32 * 4, *w);
    }
}

/// Go to the topic `page`: Back comes back to the one shown.
fn go(cpu: &mut Cpu, page: usize) -> Option<u32> {
    let v = viewer(cpu)?;
    if page != v.page {
        let old = v.page;
        v.history.push(old);
    }
    v.page = page;
    v.scroll = 0;
    v.width = -1;
    task::start(cpu, task::WINHELP, vec![K_SHOW, 1], SCRATCH)
}

/// The viewer's task.
pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    match t.var(0) {
        K_OPEN => open_step(cpu, t, value),
        K_SHOW => show_step(cpu, t),
        K_PAINT => paint_step(cpu, t, value),
        K_SIZE => match t.step {
            0 => {
                let topic = viewer(cpu).map_or(0, |v| v.topic);
                if topic == 0 {
                    return Next::Done(0);
                }
                let (w, h) = (t.var(1), t.var(2) as i32);
                let args = [topic, 0, BAR as u32, w, (h - BAR).max(0) as u32, 1];
                Next::call(address_of("MoveWindow"), &args, 1)
            }
            _ => Next::Done(0),
        },
        K_SCROLL => match t.step {
            0 => {
                let at = t.scratch + S_INFO;
                scroll_info(cpu, at);
                let topic = t.var(1);
                Next::call(address_of("SetScrollInfo"), &[topic, SB_VERT, at, 1], 1)
            }
            1 if t.var(2) != 0 => Next::call(address_of("InvalidateRect"), &[t.var(1), 0, 1], 2),
            _ => Next::Done(0),
        },
        K_POPUP => popup_step(cpu, t, value),
        K_CLOSE_POPUP => match t.step {
            0 if t.var(2) != 0 => Next::call(address_of("ReleaseCapture"), &[], 1),
            0 | 1 => Next::call(address_of("DestroyWindow"), &[t.var(1)], 2),
            _ => Next::Done(0),
        },
        K_QUIT => match t.step {
            0 => Next::call(address_of("DestroyWindow"), &[t.var(1)], 1),
            _ => Next::Done(1),
        },
        K_FOCUS => match t.step {
            0 => Next::call(address_of("SetFocus"), &[t.var(1)], 1),
            _ => Next::Done(0),
        },
        _ => Next::Done(0),
    }
}

/// The viewer window's title: the file's, or "Windows Help".
fn title(cpu: &mut Cpu) -> String {
    match viewer(cpu) {
        Some(v) if !v.file.title.is_empty() => v.file.title.clone(),
        Some(_) => "Windows Help".to_string(),
        None => String::new(),
    }
}

fn write_str(cpu: &mut Cpu, at: u32, s: &str, max: usize) {
    let mut b: Vec<u8> = s.bytes().take(max - 1).collect();
    b.push(0);
    cpu.bus.guest_write_bytes(at, &b);
}

/// Open the viewer: its window, the topic window, the buttons; then
/// show the topic.
fn open_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let instance = DLL.base;
    let class = t.scratch;
    let text = t.scratch + S_TEXT;
    let create = address_of("CreateWindowExA");
    match t.step {
        0 => {
            let title = title(cpu);
            write_str(cpu, class, FRAME_CLASS, 32);
            write_str(cpu, text, &title, 128);
            let style = WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN;
            let d = CW_USEDEFAULT;
            let args = [0, class, text, style, d, d, d, d, 0, 0, instance, 0];
            Next::call(create, &args, 1)
        }
        1 => {
            let Some(v) = viewer(cpu) else {
                return Next::Done(0);
            };
            if value == 0 {
                return Next::Done(0);
            }
            v.frame = value;
            write_str(cpu, class, TOPIC_CLASS, 32);
            let style = WS_CHILD | WS_VISIBLE | WS_VSCROLL;
            let args = [
                0, class, 0, style, 0, BAR as u32, 1, 1, value, ID_TOPIC, instance, 0,
            ];
            Next::call(create, &args, 2)
        }
        2..=5 => {
            let Some(v) = viewer(cpu) else {
                return Next::Done(0);
            };
            if t.step == 2 {
                v.topic = value;
            }
            let frame = v.frame;
            let i = (t.step - 2) as usize;
            write_str(cpu, class, "Button", 32);
            write_str(cpu, text, BUTTONS[i], 32);
            let x = 4 + i as i32 * (BUTTON_WIDTH + 2);
            let style = WS_CHILD | WS_VISIBLE;
            let (w, h) = (BUTTON_WIDTH as u32, BUTTON_HEIGHT as u32);
            let id = ID_CONTENTS + i as u32;
            let args = [
                0, class, text, style, x as u32, 3, w, h, frame, id, instance, 0,
            ];
            Next::call(create, &args, t.step + 1)
        }
        6 => {
            let Some(v) = viewer(cpu) else {
                return Next::Done(0);
            };
            let (frame, topic) = (v.frame, v.topic);
            let (w, h) = client(cpu, frame);
            let args = [topic, 0, BAR as u32, w as u32, (h - BAR).max(0) as u32, 0];
            Next::call(address_of("MoveWindow"), &args, 7)
        }
        7 => {
            refresh(cpu);
            let at = t.scratch + S_INFO;
            scroll_info(cpu, at);
            let topic = viewer(cpu).map_or(0, |v| v.topic);
            Next::call(address_of("SetScrollInfo"), &[topic, SB_VERT, at, 1], 8)
        }
        8 => {
            let frame = viewer(cpu).map_or(0, |v| v.frame);
            Next::call(address_of("ShowWindow"), &[frame, SW_SHOWNORMAL], 9)
        }
        _ => Next::Done(1),
    }
}

/// Show the topic chosen in the open viewer; `t.var(1)` is set when the
/// viewer itself goes there, so its window is left as it is, and
/// `t.var(2)` when the viewer has a file it didn't have, whose title its
/// window then takes.
fn show_step(cpu: &mut Cpu, t: &mut Task) -> Next {
    let Some(v) = viewer(cpu) else {
        return Next::Done(0);
    };
    let (frame, topic) = (v.frame, v.topic);
    match t.step {
        0 => {
            refresh(cpu);
            let at = t.scratch + S_INFO;
            scroll_info(cpu, at);
            Next::call(address_of("SetScrollInfo"), &[topic, SB_VERT, at, 1], 1)
        }
        1 => Next::call(address_of("InvalidateRect"), &[topic, 0, 1], 2),
        2 if t.var(1) == 0 => {
            let iconic = user(cpu)
                .window(frame)
                .is_some_and(|w| w.style & 0x2000_0000 != 0);
            let show = if iconic { SW_RESTORE } else { SW_SHOWNORMAL };
            Next::call(address_of("ShowWindow"), &[frame, show], 3)
        }
        2 => Next::call(address_of("SetFocus"), &[topic], 5),
        3 => Next::call(address_of("SetForegroundWindow"), &[frame], 4),
        4 if t.var(2) != 0 => {
            let text = t.scratch + S_TEXT;
            let title = title(cpu);
            write_str(cpu, text, &title, 128);
            Next::call(address_of("SetWindowTextA"), &[frame, text], 5)
        }
        _ => Next::Done(1),
    }
}

/// Paint the topic window or the popup `t.var(1)`.
fn paint_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let hwnd = t.var(1);
    let ps = t.scratch + S_INFO;
    match t.step {
        0 => Next::call(address_of("BeginPaint"), &[hwnd, ps], 1),
        1 => {
            if value != 0 {
                draw(cpu, hwnd, value);
            }
            Next::call(address_of("EndPaint"), &[hwnd, ps], 2)
        }
        _ => Next::Done(0),
    }
}

/// Draw the topic, or the popup's, on `hdc`.
fn draw(cpu: &mut Cpu, hwnd: u32, hdc: u32) {
    let (w, h) = client(cpu, hwnd);
    paint::fill_color(cpu, hdc, Rect::new(0, 0, w, h), COLOR_WINDOW);
    let Some(v) = viewer(cpu) else {
        return;
    };
    // The layout is lent out for the drawing and put back after it.
    let popup = hwnd == v.popup;
    let (l, scroll) = if popup {
        (std::mem::take(&mut v.popup_layout), 0)
    } else {
        (std::mem::take(&mut v.layout), v.scroll)
    };
    let colors: Vec<u32> = v.file.fonts.iter().map(|f| f.color).collect();
    gdi32::dc::set_bk_mode(cpu, &[hdc, gdi32::dc::TRANSPARENT]);
    let first = font(cpu, 0);
    let old = gdi32::dc::select_object(cpu, &[hdc, first]).unwrap_or(0);
    for (i, r) in l.runs.iter().enumerate() {
        let y = r.y - scroll;
        if y + r.height < 0 || y > h {
            continue;
        }
        let link = r.link.and_then(|i| l.links.get(i));
        let marked = link.is_some_and(|k| k.marked);
        let color = if marked {
            GREEN
        } else {
            colors.get(r.font as usize).copied().unwrap_or(0)
        };
        let f = font(cpu, r.font);
        gdi32::dc::select_object(cpu, &[hdc, f]);
        gdi32::dc::set_text_color(cpu, &[hdc, color]);
        gdi32::font::draw_text(cpu, hdc, (r.x, y), 0, None, &r.text, None);
        if marked {
            // The underline goes on under the spaces up to the link's
            // next word on the line.
            let next = l.runs.get(i + 1);
            let goes_on = next.is_some_and(|n| n.link == r.link && n.x == r.x + r.width);
            let n = r.text.len() - layout::trailing_spaces(&r.text);
            let ink = if goes_on {
                r.width
            } else {
                gdi32::font::text_extent(cpu, hdc, &r.text[..n]).map_or(r.width, |e| e.0)
            };
            let dotted = link.is_some_and(|k| matches!(k.target, Target::Popup { .. }));
            underline(cpu, hdc, r.x, y + r.ascent + 1, ink, dotted);
        }
    }
    if old != 0 {
        gdi32::dc::select_object(cpu, &[hdc, old]);
    }
    for p in &l.pictures {
        let y = p.y - scroll;
        if y + p.height < 0 || y > h {
            continue;
        }
        let Some(pic) = picture(cpu, p.number) else {
            continue;
        };
        let args = [
            hdc,
            p.x as u32,
            y as u32,
            pic.width as u32,
            pic.height as u32,
            0,
            0,
            0,
            pic.height as u32,
            pic.bits,
            pic.at,
            0,
        ];
        gdi32::bitmap::set_di_bits_to_device(cpu, &args);
    }
    if let Some(v) = viewer(cpu) {
        if popup {
            v.popup_layout = l;
        } else {
            v.layout = l;
        }
    }
}

/// A link's underline, `width` long at (`x`, `y`): dotted for a popup.
fn underline(cpu: &mut Cpu, hdc: u32, x: i32, y: i32, width: i32, dotted: bool) {
    let Some(brush) = gdi32::create_solid_brush(cpu, &[GREEN]) else {
        return;
    };
    if dotted {
        for dx in (0..width).step_by(2) {
            paint::fill(cpu, hdc, Rect::sized(x + dx, y, 1, 1), brush);
        }
    } else {
        paint::fill(cpu, hdc, Rect::sized(x, y, width, 1), brush);
    }
    gdi32::delete_object(cpu, &[brush]);
}

/// Show the topic `t.var(1)` in a popup at the screen point (`t.var(2)`,
/// `t.var(3)`), and take the mouse so that the next click closes it.
fn popup_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    match t.step {
        // A popup already open goes first.
        0 => match viewer(cpu).map(|v| std::mem::take(&mut v.popup)) {
            Some(old) if old != 0 => Next::call(address_of("DestroyWindow"), &[old], 1),
            _ => {
                t.step = 1;
                popup_step(cpu, t, 0)
            }
        },
        1 => {
            let Some(v) = viewer(cpu) else {
                return Next::Done(0);
            };
            let (frame, topic) = (v.frame, v.topic);
            let l = lay_out(cpu, topic, t.var(1) as usize, POPUP_WIDTH);
            let w = (l.width + MARGIN).clamp(MARGIN * 2 + 8, POPUP_WIDTH) + 2;
            let h = l.height + 2;
            if let Some(v) = viewer(cpu) {
                v.popup_layout = l;
            }
            let screen = user(cpu).screen_rect(crate::win32::user32::DESKTOP);
            let x = (t.var(2) as i32 - w / 2).clamp(0, (screen.width() - w).max(0));
            let mut y = t.var(3) as i32 + 12;
            if y + h > screen.height() {
                y = (t.var(3) as i32 - 12 - h).max(0);
            }
            let class = t.scratch;
            write_str(cpu, class, POPUP_CLASS, 32);
            let style = WS_POPUP | WS_BORDER;
            let args = [
                WS_EX_TOPMOST,
                class,
                0,
                style,
                x as u32,
                y as u32,
                w as u32,
                h as u32,
                frame,
                0,
                DLL.base,
                0,
            ];
            Next::call(address_of("CreateWindowExA"), &args, 2)
        }
        2 => {
            if value == 0 {
                return Next::Done(0);
            }
            if let Some(v) = viewer(cpu) {
                v.popup = value;
            }
            t.set(4, value);
            Next::call(address_of("ShowWindow"), &[value, SW_SHOWNA], 3)
        }
        3 => Next::call(address_of("SetCapture"), &[t.var(4)], 4),
        _ => Next::Done(1),
    }
}

/// Close the popup, if one is open.
fn close_popup(cpu: &mut Cpu) -> Option<u32> {
    let v = viewer(cpu)?;
    let popup = std::mem::take(&mut v.popup);
    if popup == 0 {
        return Some(0);
    }
    // When another window took the mouse, it keeps it.
    let held = (user(cpu).input.capture == popup) as u32;
    task::start(cpu, task::WINHELP, vec![K_CLOSE_POPUP, popup, held], 0)
}

/// Scroll the topic to `pos`.
fn scroll_to(cpu: &mut Cpu, pos: i32) -> Option<u32> {
    let v = viewer(cpu)?;
    let topic = v.topic;
    let (_, h) = client(cpu, topic);
    let v = viewer(cpu)?;
    let pos = pos.clamp(0, (v.layout.height - h).max(0));
    if pos == v.scroll {
        return Some(0);
    }
    v.scroll = pos;
    task::start(cpu, task::WINHELP, vec![K_SCROLL, topic, 1], SCRATCH)
}

/// The viewer window's procedure.
pub fn frame_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, msg, wp, lp] = args else {
        return Some(0);
    };
    let ours = viewer(cpu).is_some_and(|v| v.frame == hwnd);
    match msg {
        WM_SIZE if ours => {
            let (w, h) = (lp & 0xFFFF, lp >> 16);
            task::start(cpu, task::WINHELP, vec![K_SIZE, w, h], 0)
        }
        WM_SETFOCUS if ours => {
            let topic = viewer(cpu).map_or(0, |v| v.topic);
            task::start(cpu, task::WINHELP, vec![K_FOCUS, topic], 0)
        }
        WM_COMMAND if ours => {
            let v = viewer(cpu)?;
            let page = match wp & 0xFFFF {
                ID_CONTENTS => Some(v.contents.unwrap_or_else(|| v.file.contents())),
                ID_BACK => {
                    if let Some(p) = v.history.pop() {
                        v.page = p;
                        v.scroll = 0;
                        v.width = -1;
                        return task::start(cpu, task::WINHELP, vec![K_SHOW, 1], SCRATCH);
                    }
                    None
                }
                ID_PREVIOUS => v.file.pages[v.page]
                    .browse_back
                    .and_then(|o| v.file.page_by_offset(o)),
                ID_NEXT => v.file.pages[v.page]
                    .browse_next
                    .and_then(|o| v.file.page_by_offset(o)),
                _ => None,
            };
            // The topic window keeps the focus, for the keys that scroll
            // it: a click leaves it on the button.
            let topic = v.topic;
            match page {
                Some(p) => go(cpu, p),
                None => task::start(cpu, task::WINHELP, vec![K_FOCUS, topic], 0),
            }
        }
        WM_DESTROY if ours => {
            if let Some(v) = user(cpu).help.viewer.take() {
                free(cpu, v);
            }
            Some(0)
        }
        _ => defwnd::def_window_proc(cpu, args),
    }
}

/// The topic window's procedure, and the popup's.
pub fn topic_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, msg, wp, lp] = args else {
        return Some(0);
    };
    let Some(v) = viewer(cpu) else {
        return defwnd::def_window_proc(cpu, args);
    };
    let popup = v.popup == hwnd;
    let topic = v.topic == hwnd;
    if !popup && !topic {
        return defwnd::def_window_proc(cpu, args);
    }
    let x = (lp & 0xFFFF) as i16 as i32;
    let y = (lp >> 16) as i16 as i32;
    match msg {
        WM_CREATE => Some(0),
        WM_ERASEBKGND => Some(1),
        WM_PAINT => task::start(cpu, task::WINHELP, vec![K_PAINT, hwnd], SCRATCH),
        WM_SIZE if topic => {
            refresh(cpu);
            task::start(cpu, task::WINHELP, vec![K_SCROLL, hwnd, 1], SCRATCH)
        }
        WM_MOUSEACTIVATE if popup => Some(MA_NOACTIVATE),
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_KEYDOWN if popup => close_popup(cpu),
        WM_CAPTURECHANGED if popup => close_popup(cpu),
        WM_VSCROLL => {
            let (_, h) = client(cpu, hwnd);
            let v = viewer(cpu)?;
            let line = 16;
            let pos = match wp & 0xFFFF {
                0 => v.scroll - line,
                1 => v.scroll + line,
                2 => v.scroll - h.max(line),
                3 => v.scroll + h.max(line),
                4 | 5 => (wp >> 16) as i32,
                6 => 0,
                7 => v.layout.height,
                _ => v.scroll,
            };
            scroll_to(cpu, pos)
        }
        WM_MOUSEWHEEL => {
            let delta = (wp >> 16) as i16 as i32;
            let pos = v.scroll - delta / 120 * 48;
            scroll_to(cpu, pos)
        }
        // The popup doesn't get the focus: a key goes to the topic window,
        // and closes the popup.
        WM_KEYDOWN if v.popup != 0 => close_popup(cpu),
        WM_KEYDOWN => {
            let (_, h) = client(cpu, hwnd);
            let v = viewer(cpu)?;
            let pos = match wp {
                VK_UP => v.scroll - 16,
                VK_DOWN => v.scroll + 16,
                VK_PRIOR => v.scroll - h,
                VK_NEXT => v.scroll + h,
                VK_HOME => 0,
                VK_END => v.layout.height,
                VK_BACK => {
                    let frame = v.frame;
                    return frame_proc(cpu, &[frame, WM_COMMAND, ID_BACK, 0]);
                }
                _ => return defwnd::def_window_proc(cpu, args),
            };
            scroll_to(cpu, pos)
        }
        // No WM_SETCURSOR comes as the mouse moves, so the cursor is set
        // here, back to the arrow off a link. The popup's links aren't
        // followed, and while it is open the mouse is its own.
        WM_MOUSEMOVE if topic => {
            let id = if v.layout.link_at(x, y + v.scroll).is_some() {
                IDC_HAND
            } else {
                IDC_ARROW
            };
            let cursor = crate::win32::user32::resource::system_cursor(cpu, id);
            crate::win32::user32::input::set_cursor(cpu, &[cursor]);
            Some(0)
        }
        WM_SETCURSOR if topic => {
            if lp & 0xFFFF == HTCLIENT {
                let at = user(cpu).input.cursor;
                let origin = user(cpu).screen_client(hwnd);
                let v = viewer(cpu)?;
                if v.layout
                    .link_at(at.0 - origin.left, at.1 - origin.top + v.scroll)
                    .is_some()
                {
                    let hand = crate::win32::user32::resource::system_cursor(cpu, IDC_HAND);
                    crate::win32::user32::input::set_cursor(cpu, &[hand]);
                    return Some(1);
                }
            }
            defwnd::def_window_proc(cpu, args)
        }
        WM_LBUTTONDOWN => {
            let Some(i) = v.layout.link_at(x, y + v.scroll) else {
                return Some(0);
            };
            let link = v.layout.links[i].clone();
            match link.target {
                Target::Jump { file: None, hash } => match v.file.page_by_hash(hash) {
                    Some(p) => go(cpu, p),
                    None => Some(0),
                },
                Target::Popup { file: None, hash } => {
                    let Some(p) = v.file.page_by_hash(hash) else {
                        return Some(0);
                    };
                    let origin = user(cpu).screen_client(hwnd);
                    let (sx, sy) = (origin.left + x, origin.top + y);
                    let vars = vec![K_POPUP, p as u32, sx as u32, sy as u32];
                    task::start(cpu, task::WINHELP, vars, SCRATCH)
                }
                // Jumps to other files and macros aren't followed.
                _ => Some(0),
            }
        }
        _ => defwnd::def_window_proc(cpu, args),
    }
}
