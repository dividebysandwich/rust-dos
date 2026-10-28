//! Resources USER loads: strings, icons and cursors, bitmaps, and
//! accelerator tables; and icons and cursors made from bits.
//!
//! An icon or cursor is a mask bitmap and a color bitmap of GDI's. A
//! monochrome one has no color bitmap: its mask is twice as high, the
//! AND mask over the XOR one, as on Windows. The system's icons and
//! cursors (LoadIconA and LoadCursorA with no module) are drawn here, as
//! there is no USER.EXE to take them from.

use super::message::{WM_CHAR, WM_COMMAND, WM_KEYDOWN, WM_SYSCHAR, WM_SYSCOMMAND, WM_SYSKEYDOWN};
use super::{
    ERROR_INVALID_ACCEL_HANDLE, ERROR_INVALID_ICON_HANDLE, ERROR_INVALID_PARAMETER,
    ERROR_RESOURCE_NAME_NOT_FOUND, FALSE, TRUE, current_pid, read_words, set_error, user,
    write_words,
};
use crate::cpu::Cpu;
use crate::win32::gdi32::surface::{Surface, Target};
use crate::win32::gdi32::{self, bitmap};
use crate::win32::kernel32;
use crate::win32::task::{self, Next, Task};

const RT_CURSOR: u32 = 1;
const RT_BITMAP: u32 = 2;
const RT_ICON: u32 = 3;
const RT_STRING: u32 = 6;
const RT_ACCELERATOR: u32 = 9;
const RT_GROUP_CURSOR: u32 = 12;
const RT_GROUP_ICON: u32 = 14;

const IMAGE_BITMAP: u32 = 0;
const IMAGE_ICON: u32 = 1;
const IMAGE_CURSOR: u32 = 2;

const LR_MONOCHROME: u32 = 0x0001;
const LR_COPYDELETEORG: u32 = 0x0008;
const LR_LOADFROMFILE: u32 = 0x0010;
const LR_DEFAULTSIZE: u32 = 0x0040;
const LR_CREATEDIBSECTION: u32 = 0x2000;
const LR_SHARED: u32 = 0x8000;

/// Where icon and accelerator table handles start, and how far apart
/// they are.
const ICON_BASE: u32 = 0x0001_0000;
const ACCEL_BASE: u32 = 0x0003_0000;
const STEP: u32 = 4;

/// An icon or cursor.
#[derive(Clone, Debug, Default)]
pub struct Icon {
    pub width: i32,
    pub height: i32,
    pub cursor: bool,
    pub hotspot: (i32, i32),
    /// The AND mask (and under it the XOR one, without a color bitmap),
    /// and the color bitmap or 0.
    pub mask: u32,
    pub color: u32,
    /// Loaded as a shared resource (LoadIconA, LR_SHARED): it stays, and
    /// DestroyIcon leaves it. The module it came from, and the name as
    /// name_key gives it.
    pub shared: bool,
    pub module: u32,
    pub name: String,
    /// The process that made it, or 0 for the system's.
    pub pid: u32,
}

/// An entry of an accelerator table, as ACCEL has it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Accel {
    pub virt: u8,
    pub key: u16,
    pub cmd: u16,
}

const FVIRTKEY: u8 = 0x01;
const FSHIFT: u8 = 0x04;
const FCONTROL: u8 = 0x08;
const FALT: u8 = 0x10;

/// The icon or cursor `handle`.
pub fn icon(cpu: &mut Cpu, handle: u32) -> Option<Icon> {
    let i = handle.checked_sub(ICON_BASE)?;
    if i % STEP != 0 {
        return None;
    }
    user(cpu).icons.get((i / STEP) as usize)?.clone()
}

fn add_icon(cpu: &mut Cpu, icon: Icon) -> u32 {
    let icons = &mut user(cpu).icons;
    let i = match icons.iter().position(|i| i.is_none()) {
        Some(i) => i,
        None => {
            icons.push(None);
            icons.len() - 1
        }
    };
    icons[i] = Some(icon);
    ICON_BASE + STEP * i as u32
}

/// Make a bitmap of GDI's a stock one, which stays when its process
/// ends.
fn keep(cpu: &mut Cpu, handle: u32) {
    if let Some(o) = gdi32::gdi(cpu).get_mut(handle) {
        o.stock = true;
    }
}

/// An icon or cursor of `w` by `h` from its pixels: None is transparent,
/// a color is drawn. `mono` makes a monochrome one.
fn icon_of(
    cpu: &mut Cpu,
    w: i32,
    h: i32,
    mono: bool,
    pixel: &dyn Fn(i32, i32) -> Option<u32>,
) -> Icon {
    let mask = bitmap::new_ddb(cpu, w, if mono { 2 * h } else { h }, 1);
    let color = if mono {
        0
    } else {
        let depth = gdi32::dc::device_depth(cpu) as u16;
        bitmap::new_ddb(cpu, w, h, depth)
    };
    if let Some(mut m) = Surface::load_all(cpu, Target::Bitmap(mask)) {
        for y in 0..h {
            for x in 0..w {
                let p = pixel(x, y);
                m.put(x, y, p.is_none() as u32);
                if mono {
                    let white = p.is_some_and(|c| c & 0xFF_FFFF == 0xFF_FFFF);
                    m.put(x, y + h, white as u32);
                }
            }
        }
        m.store(cpu);
    }
    if color != 0
        && let Some(mut c) = Surface::load_all(cpu, Target::Bitmap(color))
    {
        for y in 0..h {
            for x in 0..w {
                if let Some(rgb) = pixel(x, y) {
                    let v = c.value_of(rgb);
                    c.put(x, y, v);
                }
            }
        }
        c.store(cpu);
    }
    Icon {
        width: w,
        height: h,
        mask,
        color,
        pid: current_pid(cpu),
        ..Icon::default()
    }
}

/// The arrow cursor: X black, . white.
const ARROW: [&str; 19] = [
    "X",
    "XX",
    "X.X",
    "X..X",
    "X...X",
    "X....X",
    "X.....X",
    "X......X",
    "X.......X",
    "X........X",
    "X.....XXXXX",
    "X..X..X",
    "X.X X..X",
    "XX  X..X",
    "X    X..X",
    "     X..X",
    "      X..X",
    "      X..X",
    "       XX",
];

/// The hand cursor, over links: X black, . white.
const HAND: [&str; 21] = [
    "    XX",
    "   X..X",
    "   X..X",
    "   X..X",
    "   X..X",
    "   X..XXX",
    "   X..X..XXX",
    "   X..X..X..XX",
    "XX X..X..X..X.X",
    "X..X........X..X",
    "X...X..........X",
    " X.............X",
    "  X............X",
    "  X...........X",
    "   X..........X",
    "   X.........X",
    "    X........X",
    "    X........X",
    "     X......X",
    "     X......X",
    "     XXXXXXXX",
];

/// The pixel of a picture in characters at (`x`, `y`).
fn art(lines: &[&str], x: i32, y: i32) -> Option<u32> {
    let row = lines.get(y as usize)?;
    match row.as_bytes().get(x as usize)? {
        b'X' => Some(0),
        b'.' => Some(0xFF_FFFF),
        _ => None,
    }
}

/// The pixel of the system's cursor `id` at (`x`, `y`), and its hot spot.
fn cursor_pixel(id: u32, x: i32, y: i32) -> Option<u32> {
    let black = Some(0);
    let white = Some(0xFF_FFFF);
    match id {
        // IDC_IBEAM.
        32513 => {
            let bar = x == 15 && (2..30).contains(&y);
            let serif = (y == 1 || y == 30) && (12..19).contains(&x) && x != 15;
            (bar || serif).then_some(0)
        }
        // IDC_CROSS.
        32515 => ((x == 15 && y < 31) || (y == 15 && x < 31)).then_some(0),
        // IDC_SIZEWE, IDC_SIZENS, IDC_SIZEALL: lines with ends.
        32644 => (y == 15 && (4..28).contains(&x)
            || (x - 4).abs() + (y - 15).abs() < 4 && x >= 4
            || (x - 27).abs() + (y - 15).abs() < 4 && x <= 27)
            .then_some(0),
        32645 => (x == 15 && (4..28).contains(&y)
            || (y - 4).abs() + (x - 15).abs() < 4 && y >= 4
            || (y - 27).abs() + (x - 15).abs() < 4 && y <= 27)
            .then_some(0),
        32646 => {
            ((x == 15 && (2..30).contains(&y)) || (y == 15 && (2..30).contains(&x))).then_some(0)
        }
        // IDC_WAIT: an hourglass.
        32514 => {
            let (cx, top, bottom) = (15, 4, 27);
            if y == top || y == bottom {
                return ((8..23).contains(&x)).then_some(0);
            }
            if !(top..bottom).contains(&y) {
                return None;
            }
            let half = (y - 15).abs().max(1) * 7 / 11;
            if (x - cx).abs() == half {
                black
            } else if (x - cx).abs() < half {
                white
            } else {
                None
            }
        }
        // IDC_NO: a ring.
        32648 => {
            let d = ((x - 15) * (x - 15) + (y - 15) * (y - 15)) as f64;
            (d.sqrt() > 9.0 && d.sqrt() < 12.0 || (x - y).abs() < 2 && (6..25).contains(&x))
                .then_some(0)
        }
        // IDC_HAND: a pointing hand.
        32649 => art(&HAND, x, y),
        _ => art(&ARROW, x, y),
    }
}

/// The hot spot of the system's cursor `id`.
fn cursor_hotspot(id: u32) -> (i32, i32) {
    match id {
        32513 | 32514 | 32515 | 32644 | 32645 | 32646 | 32648 => (15, 15),
        32649 => (5, 0),
        _ => (0, 0),
    }
}

/// The pixel of the system's icon `id` at (`x`, `y`).
fn icon_pixel(id: u32, x: i32, y: i32) -> Option<u32> {
    let disc = |color: u32| {
        let d = (x - 15) * (x - 15) + (y - 15) * (y - 15);
        if d < 13 * 13 {
            Some(color)
        } else if d < 14 * 14 {
            Some(0)
        } else {
            None
        }
    };
    match id {
        // IDI_HAND: a red disc with a white cross.
        32513 => {
            let cross = ((x - y).abs() < 2 || (x + y - 30).abs() < 2) && (8..23).contains(&x);
            if cross && disc(0).is_some() {
                Some(0xFF_FFFF)
            } else {
                disc(0x00_00FF)
            }
        }
        // IDI_QUESTION, IDI_ASTERISK: a disc with a mark.
        32514 | 32516 => {
            let mark = (14..=16).contains(&x) && (y == 8 || y == 9 || (12..23).contains(&y));
            if mark {
                Some(0xFF_FFFF)
            } else {
                disc(if id == 32514 { 0x80_0000 } else { 0xFF_0000 })
            }
        }
        // IDI_EXCLAMATION: a yellow triangle with a mark.
        32515 => {
            let half = (y - 2) / 2;
            if !(2..30).contains(&y) || (x - 15).abs() > half {
                return None;
            }
            let mark = (x - 15).abs() < 2 && ((9..21).contains(&y) || (24..27).contains(&y));
            Some(if mark || (x - 15).abs() == half || y == 29 {
                0
            } else {
                0x00_FFFF
            })
        }
        // IDI_APPLICATION, IDI_WINLOGO: a window.
        _ => {
            if !(2..30).contains(&x) || !(4..28).contains(&y) {
                return None;
            }
            Some(if x == 2 || x == 29 || y == 4 || y == 27 {
                0
            } else if y < 9 {
                0x80_0000
            } else {
                0xFF_FFFF
            })
        }
    }
}

/// The standard arrow cursor's id.
pub const IDC_ARROW: u32 = 32512;

/// The system's cursor `id` (IDC_ARROW and the rest): made the first
/// time it is asked for.
pub fn system_cursor(cpu: &mut Cpu, id: u32) -> u32 {
    system_image(cpu, id, true)
}

fn system_image(cpu: &mut Cpu, id: u32, cursor: bool) -> u32 {
    let key = format!("#{id}");
    let found = user(cpu).icons.iter().position(|i| {
        i.as_ref()
            .is_some_and(|i| i.module == 0 && i.name == key && i.cursor == cursor && i.shared)
    });
    if let Some(i) = found {
        return ICON_BASE + STEP * i as u32;
    }
    let mut ic = if cursor {
        let mut ic = icon_of(cpu, 32, 32, true, &|x, y| cursor_pixel(id, x, y));
        ic.hotspot = cursor_hotspot(id);
        ic
    } else {
        icon_of(cpu, 32, 32, false, &|x, y| icon_pixel(id, x, y))
    };
    if !cursor {
        ic.hotspot = (16, 16);
    }
    keep(cpu, ic.mask);
    if ic.color != 0 {
        keep(cpu, ic.color);
    }
    ic.cursor = cursor;
    ic.shared = true;
    ic.name = key;
    ic.pid = 0;
    add_icon(cpu, ic)
}

/// The system's icons and cursors a program may load.
fn is_system_icon(id: u32) -> bool {
    (32512..=32517).contains(&id)
}

fn is_system_cursor(id: u32) -> bool {
    matches!(id, 32512..=32516 | 32640..=32651)
}

// Icons and cursors from bits.

/// An icon or cursor from its resource's bits at `at` (a cursor's have
/// its hot spot first): None if they aren't a DIB it knows.
fn from_bits(cpu: &mut Cpu, at: u32, size: u32, is_icon: bool) -> Option<Icon> {
    let (hotspot, at) = if is_icon {
        ((0, 0), at)
    } else {
        let w = cpu.bus.guest_read_32(at);
        ((w as u16 as i32, (w >> 16) as u16 as i32), at + 4)
    };
    if size < 40 {
        return None;
    }
    let mut info = bitmap::read_info(cpu, at, 0, 0)?;
    info.height /= 2;
    let (w, h) = (info.width, info.abs_height());
    if w <= 0 || h <= 0 {
        return None;
    }
    let table = if info.bpp <= 8 {
        info.table_len * if info.core { 3 } else { 4 }
    } else {
        0
    };
    let xor_at = at + info.header_size + table;
    let xor = bitmap::read_dib(cpu, &info, xor_at, 0, h as u32)?;
    let mask_info = bitmap::DibInfo {
        width: w,
        height: info.height,
        bpp: 1,
        colors: vec![0, 0xFF_FFFF],
        masks: gdi32::surface::default_masks(1),
        table_len: 2,
        header_size: 40,
        ..bitmap::DibInfo::default()
    };
    let and_at = xor_at + info.stride() as u32 * h as u32;
    let and = bitmap::read_dib(cpu, &mask_info, and_at, 0, h as u32);
    let mono = info.bpp == 1
        && info.colors.first() == Some(&0)
        && info
            .colors
            .get(1)
            .is_some_and(|&c| c & 0xFF_FFFF == 0xFF_FFFF);
    let pixel = |x: i32, y: i32| {
        let transparent = and.as_ref().is_some_and(|a| a.get(x, y) != 0);
        let c = xor.color_of(xor.get(x, y));
        if transparent && c & 0xFF_FFFF == 0 {
            None
        } else {
            Some(c)
        }
    };
    let mut ic = icon_of(cpu, w, h, mono, &pixel);
    // A pixel of the AND mask with a color under it inverts the screen:
    // its mask bit stays.
    if let Some(a) = and.as_ref()
        && let Some(mut m) = Surface::load_all(cpu, Target::Bitmap(ic.mask))
    {
        for y in 0..h {
            for x in 0..w {
                m.put(x, y, a.get(x, y));
            }
        }
        m.store(cpu);
    }
    ic.cursor = !is_icon;
    ic.hotspot = if is_icon { (w / 2, h / 2) } else { hotspot };
    Some(ic)
}

/// The id of the entry of the icon or cursor directory at `dir` that
/// best fits `cx` by `cy` (0: the default size) on the screen.
fn best_entry(cpu: &mut Cpu, dir: u32, is_icon: bool, cx: i32, cy: i32, flags: u32) -> Option<u32> {
    let head = read_words(cpu, dir, 2);
    // GRPICONDIR: reserved and type, then the count in the low half of
    // the second word.
    let count = head[1] & 0xFFFF;
    if cpu.bus.guest_faulted() || count == 0 {
        return None;
    }
    let (cx, cy) = if cx == 0 && cy == 0 {
        if is_icon {
            (super::sysparams::metric(11), super::sysparams::metric(12))
        } else {
            (super::sysparams::metric(13), super::sysparams::metric(14))
        }
    } else {
        (cx, cy)
    };
    let want_bpp = if flags & LR_MONOCHROME != 0 { 1 } else { 32 };
    let mut best: Option<(i32, i32, u32)> = None;
    for i in 0..count {
        let e = dir + 6 + 14 * i;
        let mut b = [0u8; 14];
        cpu.bus.guest_read_bytes(e, &mut b);
        let (w, h, bpp) = if is_icon {
            let w = if b[0] == 0 { 256 } else { b[0] as i32 };
            let h = if b[1] == 0 { 256 } else { b[1] as i32 };
            let mut bpp = u16::from_le_bytes([b[6], b[7]]) as i32;
            if bpp == 0 {
                bpp = match b[2] {
                    2 => 1,
                    16 => 4,
                    0 => 8,
                    _ => 8,
                };
            }
            (w, h, bpp)
        } else {
            let w = u16::from_le_bytes([b[0], b[1]]) as i32;
            let h = u16::from_le_bytes([b[2], b[3]]) as i32 / 2;
            (w, h, u16::from_le_bytes([b[6], b[7]]) as i32)
        };
        let id = u16::from_le_bytes([b[12], b[13]]) as u32;
        let size_off = (w - cx).abs() + (h - cy).abs();
        let depth_off = if bpp > want_bpp {
            1000 + bpp
        } else {
            want_bpp - bpp
        };
        let better = match best {
            None => true,
            Some((s, d, _)) => size_off < s || (size_off == s && depth_off < d),
        };
        if better {
            best = Some((size_off, depth_off, id));
        }
    }
    best.map(|b| b.2)
}

/// LookupIconIdFromDirectory(dir, icon).
pub fn lookup_icon_id_from_directory(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    lookup_icon_id_from_directory_ex(cpu, &[args[0], args[1], 0, 0, 0])
}

/// LookupIconIdFromDirectoryEx(dir, icon, cx, cy, flags).
pub fn lookup_icon_id_from_directory_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (dir, is_icon, cx, cy, flags) = (
        args[0],
        args[1] != 0,
        args[2] as i32,
        args[3] as i32,
        args[4],
    );
    if dir == 0 {
        return Some(0);
    }
    Some(best_entry(cpu, dir, is_icon, cx, cy, flags).unwrap_or(0))
}

/// CreateIconFromResource(bits, size, icon, version).
pub fn create_icon_from_resource(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    create_icon_from_resource_ex(cpu, &[args[0], args[1], args[2], args[3], 0, 0, 0])
}

/// CreateIconFromResourceEx(bits, size, icon, version, cx, cy, flags).
pub fn create_icon_from_resource_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (bits, size, is_icon) = (args[0], args[1], args[2] != 0);
    if bits == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    match from_bits(cpu, bits, size, is_icon) {
        Some(ic) => Some(add_icon(cpu, ic)),
        None => {
            if cpu.bus.guest_faulted() {
                return None;
            }
            set_error(cpu, ERROR_INVALID_PARAMETER);
            Some(0)
        }
    }
}

/// Load an icon or cursor from the resources of `module`, the best of
/// its group for `cx` by `cy`: 0 with the error set if it isn't there.
fn load_from_module(
    cpu: &mut Cpu,
    module: u32,
    name: u32,
    is_icon: bool,
    cx: i32,
    cy: i32,
    flags: u32,
) -> Option<u32> {
    let shared = flags & LR_SHARED != 0;
    let key = name_key(cpu, name);
    if shared {
        let found = user(cpu).icons.iter().position(|i| {
            i.as_ref().is_some_and(|i| {
                i.shared && i.module == module && i.name == key && i.cursor != is_icon
            })
        });
        if let Some(i) = found {
            return Some(ICON_BASE + STEP * i as u32);
        }
    }
    let group = if is_icon {
        RT_GROUP_ICON
    } else {
        RT_GROUP_CURSOR
    };
    let (dir, _) = match kernel32::resource_data(cpu, module, group, name)? {
        Ok(found) => found,
        Err(_) => {
            set_error(cpu, ERROR_RESOURCE_NAME_NOT_FOUND);
            return Some(0);
        }
    };
    let Some(id) = best_entry(cpu, dir, is_icon, cx, cy, flags) else {
        set_error(cpu, ERROR_RESOURCE_NAME_NOT_FOUND);
        return Some(0);
    };
    let kind = if is_icon { RT_ICON } else { RT_CURSOR };
    let (bits, size) = match kernel32::resource_data(cpu, module, kind, id)? {
        Ok(found) => found,
        Err(_) => {
            set_error(cpu, ERROR_RESOURCE_NAME_NOT_FOUND);
            return Some(0);
        }
    };
    let Some(mut ic) = from_bits(cpu, bits, size, is_icon) else {
        set_error(cpu, ERROR_RESOURCE_NAME_NOT_FOUND);
        return Some(0);
    };
    ic.shared = shared;
    ic.module = module;
    ic.name = key;
    Some(add_icon(cpu, ic))
}

/// A resource name as a shared icon keeps it to be found by: "#123" for
/// a number, given as one or as that string, and a string in capitals
/// otherwise, as resource names are compared without case. Not the
/// string's address, which a caller may reuse for the next name, as the
/// 16-bit CREATESTRUCT conversion does for each SS_ICON static.
fn name_key(cpu: &mut Cpu, name: u32) -> String {
    match super::Name::read(cpu, name) {
        Some(super::Name::Id(id)) => format!("#{id}"),
        Some(super::Name::Text(text)) => text.to_ascii_uppercase(),
        None => String::new(),
    }
}

/// LoadIconA(module, name).
pub fn load_icon(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (module, name) = (args[0], args[1]);
    super::init(cpu);
    if module == 0 {
        if name < 0x1_0000 && is_system_icon(name) {
            return Some(system_image(cpu, name, false));
        }
        set_error(cpu, ERROR_RESOURCE_NAME_NOT_FOUND);
        return Some(0);
    }
    load_from_module(cpu, module, name, true, 0, 0, LR_SHARED)
}

/// LoadCursorA(module, name).
pub fn load_cursor(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (module, name) = (args[0], args[1]);
    super::init(cpu);
    if module == 0 {
        if name < 0x1_0000 && is_system_cursor(name) {
            return Some(system_image(cpu, name, true));
        }
        set_error(cpu, ERROR_RESOURCE_NAME_NOT_FOUND);
        return Some(0);
    }
    load_from_module(cpu, module, name, false, 0, 0, LR_SHARED)
}

/// The bits of the packed DIB at `at` (its header and color table
/// first).
fn packed_bits(cpu: &mut Cpu, at: u32) -> Option<(bitmap::DibInfo, u32)> {
    let info = bitmap::read_info(cpu, at, 0, 0)?;
    let mut offset = info.header_size;
    if info.bpp <= 8 {
        offset += info.table_len * if info.core { 3 } else { 4 };
    } else if info.compression == 3 && info.header_size == 40 {
        offset += 12;
    }
    Some((info, at + offset))
}

/// A bitmap from the packed DIB at `at`: one of GDI's, or a DIB section.
fn bitmap_from_dib(cpu: &mut Cpu, at: u32, dib_section: bool) -> Option<u32> {
    let Some((info, bits)) = packed_bits(cpu, at) else {
        return if cpu.bus.guest_faulted() {
            None
        } else {
            Some(0)
        };
    };
    if dib_section {
        let h = bitmap::create_dib_section(cpu, &[0, at, 0, 0, 0, 0])?;
        let to = gdi32::gdi(cpu)
            .bitmap(h)
            .and_then(|b| b.dib.as_ref())
            .map_or(0, |d| d.bits);
        if to != 0 {
            // No more than the section's own bits hold.
            let size = if info.image_size != 0 && info.compression != 0 {
                info.image_size.min(info.bits_size())
            } else {
                info.bits_size()
            };
            let mut data = vec![0u8; size as usize];
            cpu.bus.guest_read_bytes(bits, &mut data);
            cpu.bus.guest_write_bytes(to, &data);
        }
        return Some(h);
    }
    let screen = super::paint::window_dc(cpu, super::DESKTOP, 0, None);
    let h = bitmap::create_di_bitmap(cpu, &[screen, at, 4, bits, at, 0]);
    super::paint::release_dc(cpu, &[0, screen]);
    h
}

/// LoadBitmapA(module, name).
pub fn load_bitmap(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (module, name) = (args[0], args[1]);
    if module == 0 {
        // The system's bitmaps (OBM_): an empty one of their size.
        let depth = gdi32::dc::device_depth(cpu) as u16;
        let h = bitmap::new_ddb(cpu, 16, 16, depth);
        return Some(h);
    }
    match kernel32::resource_data(cpu, module, RT_BITMAP, name)? {
        Ok((at, _)) => bitmap_from_dib(cpu, at, false),
        Err(_) => {
            set_error(cpu, ERROR_RESOURCE_NAME_NOT_FOUND);
            Some(0)
        }
    }
}

/// A file read into pages of the program's memory, for LR_LOADFROMFILE:
/// its address and size, which the caller frees. Some(None) if it can't
/// be read.
fn load_file(cpu: &mut Cpu, name: u32) -> Option<Option<(u32, u32)>> {
    kernel32::read_whole_file(cpu, name)
}

/// LoadImageA(module, name, type, cx, cy, flags).
pub fn load_image(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[module, name, kind, cx, cy, flags] = args else {
        unreachable!()
    };
    super::init(cpu);
    let (mut cx, mut cy) = (cx as i32, cy as i32);
    if flags & LR_DEFAULTSIZE != 0 && cx == 0 && cy == 0 && kind != IMAGE_BITMAP {
        let base = if kind == IMAGE_ICON { 11 } else { 13 };
        cx = super::sysparams::metric(base);
        cy = super::sysparams::metric(base + 1);
    }
    if flags & LR_LOADFROMFILE != 0 {
        let Some((at, size)) = load_file(cpu, name)? else {
            return Some(0);
        };
        let result = match kind {
            IMAGE_BITMAP => {
                // A BITMAPFILEHEADER, then the packed DIB.
                let magic = cpu.bus.guest_read_16(at);
                if magic == 0x4D42 && size > 14 {
                    let bits = at + cpu.bus.guest_read_32(at + 10);
                    let info_at = at + 14;
                    if flags & LR_CREATEDIBSECTION != 0 {
                        bitmap_from_dib(cpu, info_at, true)
                    } else {
                        let screen = super::paint::window_dc(cpu, super::DESKTOP, 0, None);
                        let h =
                            bitmap::create_di_bitmap(cpu, &[screen, info_at, 4, bits, info_at, 0]);
                        super::paint::release_dc(cpu, &[0, screen]);
                        h
                    }
                } else {
                    Some(0)
                }
            }
            _ => {
                // An .ICO or .CUR file: its directory has each image's
                // offset where a group has its id.
                let is_icon = kind == IMAGE_ICON;
                let count = cpu.bus.guest_read_16(at + 4) as u32;
                let mut best: Option<(i32, u32)> = None;
                for i in 0..count {
                    let e = at + 6 + 16 * i;
                    let w = cpu.bus.guest_read_8(e) as i32;
                    let w = if w == 0 { 256 } else { w };
                    let want = if cx == 0 { 32 } else { cx };
                    let off = (w - want).abs();
                    if best.is_none_or(|(o, _)| off < o) {
                        best = Some((off, i));
                    }
                }
                match best {
                    Some((_, i)) => {
                        let e = at + 6 + 16 * i;
                        let len = cpu.bus.guest_read_32(e + 8);
                        let offset = cpu.bus.guest_read_32(e + 12);
                        let hot = cpu.bus.guest_read_32(e + 4);
                        let image = at + offset;
                        let made = if is_icon {
                            from_bits(cpu, image, len, true)
                        } else {
                            from_bits(cpu, image, len, true).map(|mut ic| {
                                ic.cursor = true;
                                ic.hotspot = (hot as u16 as i32, (hot >> 16) as u16 as i32);
                                ic
                            })
                        };
                        Some(made.map_or(0, |ic| add_icon(cpu, ic)))
                    }
                    None => Some(0),
                }
            }
        };
        kernel32::free_pages(cpu, at);
        return result;
    }
    match kind {
        IMAGE_BITMAP => {
            if module == 0 {
                return load_bitmap(cpu, &[0, name]);
            }
            match kernel32::resource_data(cpu, module, RT_BITMAP, name)? {
                Ok((at, _)) => bitmap_from_dib(cpu, at, flags & LR_CREATEDIBSECTION != 0),
                Err(_) => {
                    set_error(cpu, ERROR_RESOURCE_NAME_NOT_FOUND);
                    Some(0)
                }
            }
        }
        IMAGE_ICON | IMAGE_CURSOR => {
            let is_icon = kind == IMAGE_ICON;
            if module == 0 {
                return if is_icon {
                    load_icon(cpu, &[0, name])
                } else {
                    load_cursor(cpu, &[0, name])
                };
            }
            load_from_module(cpu, module, name, is_icon, cx, cy, flags)
        }
        _ => {
            set_error(cpu, ERROR_INVALID_PARAMETER);
            Some(0)
        }
    }
}

// Icons and cursors made by programs.

/// A bitmap of GDI's that is a copy of `handle`, as a bitmap of GDI's.
pub fn copy_bitmap(cpu: &mut Cpu, handle: u32) -> u32 {
    let Some(b) = gdi32::gdi(cpu).bitmap(handle).cloned() else {
        return 0;
    };
    if b.dib.is_none() {
        let mut copy = b;
        copy.dc = 0;
        return gdi32::add(cpu, gdi32::Kind::Bitmap { bitmap: copy });
    }
    let bpp = (b.bpp * b.planes).max(1);
    let new = bitmap::new_ddb(cpu, b.width, b.height, bpp);
    let (Some(src), Some(mut dst)) = (
        Surface::load_all(cpu, Target::Bitmap(handle)),
        Surface::load_all(cpu, Target::Bitmap(new)),
    ) else {
        return new;
    };
    for y in 0..b.height {
        for x in 0..b.width {
            let v = dst.value_of(src.color_of(src.get(x, y)));
            dst.put(x, y, v);
        }
    }
    dst.store(cpu);
    new
}

/// CreateIcon(module, width, height, planes, bpp, and bits, xor bits).
pub fn create_icon(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[_, w, h, planes, bpp, and, xor] = args else {
        unreachable!()
    };
    let (w, h) = (w as i32, h as i32);
    if w <= 0 || h <= 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let mono = planes as u8 as u32 * bpp as u8 as u32 == 1;
    let ic = if mono {
        let stride = gdi32::surface::ddb_stride(w, 1) * h as usize;
        let mut bits = vec![0u8; 2 * stride];
        cpu.bus.guest_read_bytes(and, &mut bits[..stride]);
        cpu.bus.guest_read_bytes(xor, &mut bits[stride..]);
        let mask = bitmap::new_ddb(cpu, w, 2 * h, 1);
        if let Some(b) = gdi32::gdi(cpu).bitmap_mut(mask) {
            let n = b.bits.len().min(bits.len());
            b.bits[..n].copy_from_slice(&bits[..n]);
        }
        Icon {
            width: w,
            height: h,
            mask,
            ..Icon::default()
        }
    } else {
        let mask = bitmap::create_bitmap(cpu, &[w as u32, h as u32, 1, 1, and])?;
        let color = bitmap::create_bitmap(cpu, &[w as u32, h as u32, planes, bpp, xor])?;
        Icon {
            width: w,
            height: h,
            mask,
            color,
            ..Icon::default()
        }
    };
    if cpu.bus.guest_faulted() {
        return None;
    }
    let ic = Icon {
        hotspot: (w / 2, h / 2),
        pid: current_pid(cpu),
        ..ic
    };
    Some(add_icon(cpu, ic))
}

/// CreateCursor(module, x hot, y hot, width, height, and bits, xor bits).
pub fn create_cursor(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[module, x, y, w, h, and, xor] = args else {
        unreachable!()
    };
    let handle = create_icon(cpu, &[module, w, h, 1, 1, and, xor])?;
    let i = (handle.wrapping_sub(ICON_BASE) / STEP) as usize;
    if let Some(Some(ic)) = user(cpu).icons.get_mut(i) {
        ic.cursor = true;
        ic.hotspot = (x as i32, y as i32);
    }
    Some(handle)
}

/// CreateIconIndirect(info): the bitmaps are copied.
pub fn create_icon_indirect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let w = read_words(cpu, args[0], 5);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let (is_icon, hx, hy, mask, color) = (w[0] != 0, w[1] as i32, w[2] as i32, w[3], w[4]);
    let Some(m) = gdi32::gdi(cpu).bitmap(mask).cloned() else {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    };
    let height = if color == 0 { m.height / 2 } else { m.height };
    let ic = Icon {
        width: m.width,
        height,
        cursor: !is_icon,
        hotspot: if is_icon {
            (m.width / 2, height / 2)
        } else {
            (hx, hy)
        },
        mask: copy_bitmap(cpu, mask),
        color: if color != 0 {
            copy_bitmap(cpu, color)
        } else {
            0
        },
        pid: current_pid(cpu),
        ..Icon::default()
    };
    Some(add_icon(cpu, ic))
}

/// DestroyIcon(icon) and DestroyCursor: a shared one stays.
pub fn destroy_icon(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(ic) = icon(cpu, args[0]) else {
        set_error(cpu, ERROR_INVALID_ICON_HANDLE);
        return Some(FALSE);
    };
    if ic.shared {
        return Some(TRUE);
    }
    free_icon(cpu, args[0], &ic);
    Some(TRUE)
}

fn free_icon(cpu: &mut Cpu, handle: u32, ic: &Icon) {
    for b in [ic.mask, ic.color] {
        if b != 0 {
            gdi32::delete_object_now(cpu, b);
        }
    }
    let i = ((handle - ICON_BASE) / STEP) as usize;
    if let Some(slot) = user(cpu).icons.get_mut(i) {
        *slot = None;
    }
}

/// CopyIcon(icon).
pub fn copy_icon(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(ic) = icon(cpu, args[0]) else {
        set_error(cpu, ERROR_INVALID_ICON_HANDLE);
        return Some(0);
    };
    let copy = Icon {
        mask: copy_bitmap(cpu, ic.mask),
        color: if ic.color != 0 {
            copy_bitmap(cpu, ic.color)
        } else {
            0
        },
        shared: false,
        pid: current_pid(cpu),
        ..ic
    };
    Some(add_icon(cpu, copy))
}

/// GetIconInfo(icon, info): copies of its bitmaps.
pub fn get_icon_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(ic) = icon(cpu, args[0]) else {
        set_error(cpu, ERROR_INVALID_ICON_HANDLE);
        return Some(FALSE);
    };
    let mask = copy_bitmap(cpu, ic.mask);
    let color = if ic.color != 0 {
        copy_bitmap(cpu, ic.color)
    } else {
        0
    };
    let words = [
        !ic.cursor as u32,
        ic.hotspot.0 as u32,
        ic.hotspot.1 as u32,
        mask,
        color,
    ];
    write_words(cpu, args[1], &words);
    Some(TRUE)
}

/// CopyImage(image, type, cx, cy, flags).
pub fn copy_image(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[image, kind, cx, cy, flags] = args else {
        unreachable!()
    };
    let copy = match kind {
        IMAGE_BITMAP => {
            let Some(b) = gdi32::gdi(cpu).bitmap(image).cloned() else {
                return Some(0);
            };
            let (cx, cy) = (cx as i32, cy as i32);
            if (cx == 0 || cx == b.width) && (cy == 0 || cy == b.height) {
                copy_bitmap(cpu, image)
            } else {
                // A copy of another size: stretched.
                let (w, h) = (
                    if cx == 0 { b.width } else { cx },
                    if cy == 0 { b.height } else { cy },
                );
                let new = bitmap::new_ddb(cpu, w, h, (b.bpp * b.planes).max(1));
                let src = gdi32::dc::create_compatible_dc(cpu, &[0])?;
                let dst = gdi32::dc::create_compatible_dc(cpu, &[0])?;
                gdi32::dc::select_object(cpu, &[src, image]);
                gdi32::dc::select_object(cpu, &[dst, new]);
                let a = [
                    dst,
                    0,
                    0,
                    w as u32,
                    h as u32,
                    src,
                    0,
                    0,
                    b.width as u32,
                    b.height as u32,
                    0x00CC_0020,
                ];
                gdi32::draw::stretch_blt(cpu, &a);
                gdi32::dc::delete_dc(cpu, &[src]);
                gdi32::dc::delete_dc(cpu, &[dst]);
                new
            }
        }
        IMAGE_ICON | IMAGE_CURSOR => copy_icon(cpu, &[image])?,
        _ => {
            set_error(cpu, ERROR_INVALID_PARAMETER);
            return Some(0);
        }
    };
    if flags & LR_COPYDELETEORG != 0 && copy != 0 {
        if kind == IMAGE_BITMAP {
            gdi32::delete_object(cpu, &[image]);
        } else {
            destroy_icon(cpu, &[image]);
        }
    }
    Some(copy)
}

// Strings.

/// The string `id` of the module's string table, as UTF-16 units.
pub fn string_units(cpu: &mut Cpu, module: u32, id: u32) -> Option<Option<Vec<u16>>> {
    let block = (id >> 4) + 1;
    let (at, size) = match kernel32::resource_data(cpu, module, RT_STRING, block)? {
        Ok(found) => found,
        Err(_) => return Some(None),
    };
    let mut p = at;
    let end = at + size;
    for _ in 0..(id & 15) {
        let len = cpu.bus.guest_read_16(p) as u32;
        p += 2 + 2 * len;
        if p >= end {
            return Some(None);
        }
    }
    let len = cpu.bus.guest_read_16(p) as usize;
    let mut bytes = vec![0u8; 2 * len];
    cpu.bus.guest_read_bytes(p + 2, &mut bytes);
    Some(Some(
        bytes
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect(),
    ))
}

/// LoadStringA(module, id, buffer, size): how many bytes it copied.
pub fn load_string(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (module, id, buffer, size) = (args[0], args[1] & 0xFFFF, args[2], args[3]);
    let units = string_units(cpu, module, id)?.unwrap_or_default();
    if buffer == 0 {
        return Some(0);
    }
    let bytes: Vec<u8> = units
        .iter()
        .map(|&c| {
            crate::win32::convert::CodePage::Ansi
                .byte(c)
                .unwrap_or(b'?')
        })
        .collect();
    if size == 0 {
        return Some(0);
    }
    Some(super::copy_out(cpu, buffer, size, &bytes))
}

/// LoadStringW(module, id, buffer, size): Windows 95 has it.
pub fn load_string_w(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (module, id, buffer, size) = (args[0], args[1] & 0xFFFF, args[2], args[3]);
    let units = string_units(cpu, module, id)?.unwrap_or_default();
    if buffer == 0 || size == 0 {
        return Some(0);
    }
    let n = units.len().min(size as usize - 1);
    let mut bytes: Vec<u8> = units[..n].iter().flat_map(|c| c.to_le_bytes()).collect();
    bytes.extend([0, 0]);
    cpu.bus.guest_write_bytes(buffer, &bytes);
    Some(n as u32)
}

// Accelerator tables.

fn accel_table(cpu: &mut Cpu, handle: u32) -> Option<Vec<Accel>> {
    let i = handle.checked_sub(ACCEL_BASE)?;
    if i % STEP != 0 {
        return None;
    }
    user(cpu).accels.get((i / STEP) as usize)?.clone()
}

fn add_accels(cpu: &mut Cpu, table: Vec<Accel>) -> u32 {
    let accels = &mut user(cpu).accels;
    let i = match accels.iter().position(|a| a.is_none()) {
        Some(i) => i,
        None => {
            accels.push(None);
            accels.len() - 1
        }
    };
    accels[i] = Some(table);
    ACCEL_BASE + STEP * i as u32
}

/// LoadAcceleratorsA(module, name): the table, 8 bytes an entry, the
/// last one marked.
pub fn load_accelerators(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (at, size) = match kernel32::resource_data(cpu, args[0], RT_ACCELERATOR, args[1])? {
        Ok(found) => found,
        Err(_) => {
            set_error(cpu, ERROR_RESOURCE_NAME_NOT_FOUND);
            return Some(0);
        }
    };
    let mut table = Vec::new();
    for i in 0..size / 8 {
        let e = at + 8 * i;
        let virt = cpu.bus.guest_read_16(e);
        table.push(Accel {
            virt: virt as u8 & 0x7F,
            key: cpu.bus.guest_read_16(e + 2),
            cmd: cpu.bus.guest_read_16(e + 4),
        });
        if virt & 0x80 != 0 {
            break;
        }
    }
    Some(add_accels(cpu, table))
}

/// CreateAcceleratorTableA(entries, count): ACCEL is 6 bytes.
pub fn create_accelerator_table(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (at, count) = (args[0], args[1] as i32);
    if count <= 0 || at == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    if !cpu.bus.guest_probe(at, 6 * count as usize, false) {
        return None;
    }
    let mut bytes = vec![0u8; 6 * count as usize];
    cpu.bus.guest_read_bytes(at, &mut bytes);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let table = bytes
        .chunks(6)
        .map(|c| Accel {
            virt: c[0] & 0x7F,
            key: u16::from_le_bytes([c[2], c[3]]),
            cmd: u16::from_le_bytes([c[4], c[5]]),
        })
        .collect();
    Some(add_accels(cpu, table))
}

/// DestroyAcceleratorTable(table).
pub fn destroy_accelerator_table(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if accel_table(cpu, args[0]).is_none() {
        set_error(cpu, ERROR_INVALID_ACCEL_HANDLE);
        return Some(FALSE);
    }
    let i = ((args[0] - ACCEL_BASE) / STEP) as usize;
    user(cpu).accels[i] = None;
    Some(TRUE)
}

/// CopyAcceleratorTableA(table, entries, count): how many it copied, or
/// with no buffer how many there are.
pub fn copy_accelerator_table(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(table) = accel_table(cpu, args[0]) else {
        set_error(cpu, ERROR_INVALID_ACCEL_HANDLE);
        return Some(0);
    };
    if args[1] == 0 {
        return Some(table.len() as u32);
    }
    let n = table.len().min(args[2] as usize);
    let mut bytes = Vec::new();
    for a in table.iter().take(n) {
        bytes.push(a.virt);
        bytes.push(0);
        bytes.extend(a.key.to_le_bytes());
        bytes.extend(a.cmd.to_le_bytes());
    }
    cpu.bus.guest_write_bytes(args[1], &bytes);
    Some(n as u32)
}

const TASK_RESOURCE: u16 = 18;

/// TranslateAcceleratorA(hwnd, table, msg): whether a key of the message
/// is in the table, which sends the window its WM_COMMAND.
pub fn translate_accelerator(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, handle, at) = (args[0], args[1], args[2]);
    let Some(table) = accel_table(cpu, handle) else {
        set_error(cpu, ERROR_INVALID_ACCEL_HANDLE);
        return Some(0);
    };
    if at == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let m = read_words(cpu, at, 4);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let (message, key) = (m[1], m[2]);
    let virt = match message {
        WM_KEYDOWN | WM_SYSKEYDOWN => true,
        WM_CHAR | WM_SYSCHAR => false,
        _ => return Some(0),
    };
    let input = &user(cpu).input;
    let down = |vk: usize| input.keys.get(vk).is_some_and(|&k| k & 0x80 != 0);
    let (shift, ctrl, alt) = (down(0x10), down(0x11), down(0x12));
    let found = table.iter().find(|a| {
        if virt != (a.virt & FVIRTKEY != 0) {
            return false;
        }
        if virt {
            a.key as u32 == key & 0xFFFF
                && (a.virt & FSHIFT != 0) == shift
                && (a.virt & FCONTROL != 0) == ctrl
                && (a.virt & FALT != 0) == alt
        } else {
            a.key as u32 == key & 0xFFFF && ((a.virt & FALT != 0) == (message == WM_SYSCHAR))
        }
    });
    let Some(a) = found.copied() else {
        return Some(0);
    };
    if !user(cpu).is_window(hwnd) {
        set_error(cpu, super::ERROR_INVALID_WINDOW_HANDLE);
        return Some(0);
    }
    // A command of the window's system menu goes as WM_SYSCOMMAND.
    let system = a.cmd >= 0xF000;
    let msg = if system { WM_SYSCOMMAND } else { WM_COMMAND };
    let wp = a.cmd as u32 | if system { 0 } else { 1 << 16 };
    task::start(cpu, TASK_RESOURCE, vec![hwnd, msg, wp], 0)
}

/// TranslateAcceleratorA's step: the message, then TRUE.
pub fn step(_cpu: &mut Cpu, t: &mut Task, _value: u32) -> Next {
    match t.step {
        0 => super::window::send(t.var(0), t.var(1), t.var(2), 0, 1),
        _ => Next::Done(TRUE),
    }
}

/// The process `pid` ended: its icons and cursors go.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    let list: Vec<(u32, Icon)> = user(cpu)
        .icons
        .iter()
        .enumerate()
        .filter_map(|(i, ic)| {
            ic.as_ref()
                .filter(|ic| ic.pid == pid && pid != 0)
                .map(|ic| (ICON_BASE + STEP * i as u32, ic.clone()))
        })
        .collect();
    for (handle, ic) in list {
        let i = ((handle - ICON_BASE) / STEP) as usize;
        if let Some(slot) = user(cpu).icons.get_mut(i) {
            *slot = None;
        }
        let _ = ic;
    }
}

crate::state_fields!(Icon {
    width,
    height,
    cursor,
    hotspot,
    mask,
    color,
    shared,
    module,
    name,
    pid
});
crate::state_fields!(Accel { virt, key, cmd });

#[cfg(test)]
mod tests {
    use super::*;

    /// The count of a directory's entries is its third word: entries
    /// after it aren't looked at, though they'd fit better.
    #[test]
    fn only_the_directorys_entries_count() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        let dir = 0x1_0000;
        let mut bytes = vec![0, 0, 1, 0, 1, 0];
        // 32x32 at 4 bits, id 5, then bytes that read as a 32x32 entry
        // at 32 bits, id 9.
        bytes.extend([32, 32, 16, 0, 1, 0, 4, 0, 0xE8, 2, 0, 0, 5, 0]);
        bytes.extend([32, 32, 0, 0, 1, 0, 32, 0, 0xA8, 0x10, 0, 0, 9, 0]);
        for (i, b) in bytes.iter().enumerate() {
            cpu.bus.write_8(dir as usize + i, *b);
        }
        assert_eq!(best_entry(&mut cpu, dir, true, 32, 32, 0), Some(5));
    }

    /// A shared icon is found by the name the string holds, so a string
    /// rewritten in place names another icon, and "#5" names icon 5.
    #[test]
    fn shared_icons_are_found_by_the_names_text() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        let at = 0x1_0000;
        let key = |cpu: &mut Cpu, text: &str| {
            for (i, b) in text.bytes().chain([0]).enumerate() {
                cpu.bus.write_8(at as usize + i, b);
            }
            name_key(cpu, at)
        };
        assert_eq!(key(&mut cpu, "#253"), "#253");
        assert_eq!(key(&mut cpu, "#323"), "#323");
        assert_eq!(key(&mut cpu, "Hero"), key(&mut cpu, "HERO"));
        assert_eq!(name_key(&mut cpu, 5), key(&mut cpu, "#5"));
    }
}
