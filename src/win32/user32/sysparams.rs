//! The system's metrics, colors and parameters, as Windows 95 has them
//! with its standard color scheme, and the display's mode, which the
//! screen's size follows.
//!
//! The screen is as large as the display mode: the desktop's, 640x480
//! unless ChangeDisplaySettings set another, or the one DirectDraw set
//! while it has the display. GetSystemMetrics, the desktop window, the
//! monitor, the cursor's range and the rest take their size from
//! `screen_size`. A change of the mode (`set_display_mode`, DirectDraw's
//! SetDisplayMode and RestoreDisplayMode, the end of the process that
//! set it) resizes the desktop window, keeps the cursor on the screen
//! and tells the top-level windows with WM_DISPLAYCHANGE
//! (`display_changed`).

use super::{FALSE, TRUE, read_words, user, write_words};
use crate::cpu::Cpu;
use crate::win32::gdi32::{self, Brush, Kind};
use crate::win32::screen;

const WM_DISPLAYCHANGE: u32 = 0x007E;

/// ChangeDisplaySettings' results.
pub const DISP_CHANGE_SUCCESSFUL: u32 = 0;
pub const DISP_CHANGE_BADMODE: u32 = -2i32 as u32;

/// The display's mode: width, height and bits a pixel. While DirectDraw
/// has set a mode it is that one; else the desktop's, which
/// ChangeDisplaySettings may have changed (`set_display_mode`). A mode
/// Glide set on a Banshee or Voodoo3 isn't, as Windows doesn't know of
/// it.
pub fn display_mode(cpu: &Cpu) -> (u32, u32, u32) {
    crate::win32::ddraw::display::screen_mode(cpu).unwrap_or_else(|| {
        let d = screen::desktop(cpu);
        (d.width, d.height, d.bpp)
    })
}

/// The screen's size, the display mode's (`display_mode`): what
/// GetSystemMetrics(SM_CXSCREEN) and SM_CYSCREEN give, the desktop
/// window's and the monitor's size, and where the cursor can go.
pub fn screen_size(cpu: &Cpu) -> (i32, i32) {
    let (w, h, _) = display_mode(cpu);
    (w as i32, h as i32)
}

/// The modes EnumDisplaySettings lists and ChangeDisplaySettings sets,
/// as width, height and bits a pixel, by size and then depth.
pub fn display_modes() -> Vec<(u32, u32, u32)> {
    screen::DESKTOP_SIZES
        .iter()
        .flat_map(|&(w, h)| [8, 16, 32].map(|bpp| (w, h, bpp)))
        .collect()
}

/// Whether ChangeDisplaySettings sets the mode `w` x `h` x `bpp`: one of
/// `display_modes`, or one of their sizes at the desktop's own depth.
fn settable(cpu: &Cpu, (w, h, bpp): (u32, u32, u32)) -> bool {
    screen::DESKTOP_SIZES.contains(&(w, h))
        && ([8, 16, 32].contains(&bpp) || bpp == screen::desktop(cpu).bpp)
}

/// Set the desktop's display mode, as ChangeDisplaySettings does, for
/// the running process: `mode` is width, height and bits a pixel, one
/// of `display_modes` or the desktop's own depth, and None gives the
/// desktop's own mode back. DISP_CHANGE_SUCCESSFUL, or
/// DISP_CHANGE_BADMODE for a mode there isn't, and nothing changes.
///
/// The desktop takes the size: the adapter is set to it at once if the
/// desktop is on the screen and DirectDraw doesn't have the display,
/// and everything is painted again; else when the desktop next shows.
/// GDI goes on drawing at the desktop's own depth, whatever depth was
/// asked for. The screen's size and the metrics follow, and the
/// windows are told (`display_changed`). The process's end gives the
/// desktop's own mode back (`process_ended`).
pub fn set_display_mode(cpu: &mut Cpu, mode: Option<(u32, u32, u32)>) -> u32 {
    if mode.is_some_and(|m| !settable(cpu, m)) {
        return DISP_CHANGE_BADMODE;
    }
    let old = display_mode(cpu);
    let before = screen::desktop(cpu);
    let pid = super::current_pid(cpu);
    screen::set_desktop_size(cpu, mode.map(|(w, h, _)| (w, h)), pid);
    display_changed(cpu, old);
    let shown = user(cpu).desktop && !crate::win32::ddraw::owns_display(cpu);
    if shown && screen::desktop(cpu) != before {
        super::paint::restore_desktop(cpu);
    }
    DISP_CHANGE_SUCCESSFUL
}

/// The display's mode may have changed from `old` (`display_mode`). If
/// it has, the desktop window takes the new size, the cursor stays on
/// the screen (`input::screen_changed`), and each top-level window gets
/// WM_DISPLAYCHANGE with the bits a pixel and the size. Windows sends
/// it with SendNotifyMessage, which to the broadcast address posts it.
pub fn display_changed(cpu: &mut Cpu, old: (u32, u32, u32)) {
    let mode = display_mode(cpu);
    if mode == old {
        return;
    }
    let (w, h) = (mode.0 as i32, mode.1 as i32);
    if let Some(d) = user(cpu).window_mut(super::DESKTOP) {
        d.rect = super::Rect::new(0, 0, w, h);
        d.client = d.rect;
    }
    super::input::screen_changed(cpu);
    let lparam = super::make_long(w, h);
    let broadcast = super::message::HWND_BROADCAST;
    super::message::post_message(cpu, &[broadcast, WM_DISPLAYCHANGE, mode.2, lparam]);
}

/// The process `pid` ended: a desktop mode it set is given back.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    if screen::desktop_size_owner(cpu) == Some(pid) {
        set_display_mode(cpu, None);
    }
}

/// GetSystemMetrics' value of `index` on the screen as it is now.
pub fn screen_metric(cpu: &Cpu, index: u32) -> i32 {
    let (w, h) = screen_size(cpu);
    match index {
        0 | 16 => w,
        1 => h,
        17 => h - 19,
        59 => w + 12,
        60 => h + 12,
        61 => w + 8,
        62 => h + 8,
        _ => metric(index),
    }
}

/// GetSystemMetrics' value of `index`, of the metrics that don't depend
/// on the screen's size: SM_CXSCREEN, SM_CYSCREEN and the full-screen,
/// maximized and largest window sizes come from `screen_metric`.
pub fn metric(index: u32) -> i32 {
    match index {
        2 | 3 => 16,
        4 => 19,
        5 | 6 => 1,
        7 | 8 => 3,
        9 | 10 => 16,
        11..=14 => 32,
        15 => 19,
        19 => 1,
        20 | 21 => 16,
        28 => 112,
        29 => 27,
        30 | 31 => 18,
        32 | 33 => 4,
        34 => 112,
        35 => 27,
        36 | 37 => 4,
        38 | 39 => 75,
        43 => 2,
        45 | 46 => 2,
        47 => 160,
        48 => 24,
        49 | 50 => 16,
        51 => 15,
        52 | 53 => 13,
        54 | 55 => 18,
        56 => 8,
        57 => 160,
        58 => 24,
        68 | 69 => 2,
        71 | 72 => 13,
        _ => 0,
    }
}

/// GetSystemMetrics(index).
/// Windows 98 adds the virtual screen's and the monitors' (monitor.rs).
pub fn get_system_metrics(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if crate::win32::version::is_98(cpu)
        && let Some(value) = super::monitor::metric(cpu, args[0])
    {
        return Some(value as u32);
    }
    Some(screen_metric(cpu, args[0]) as u32)
}

/// The standard scheme's colors, by COLOR_ index.
const COLORS: [u32; 25] = [
    0xC0C0C0, 0x808000, 0x800000, 0x808080, 0xC0C0C0, 0xFFFFFF, 0x000000, 0x000000, 0x000000,
    0xFFFFFF, 0xC0C0C0, 0xC0C0C0, 0x808080, 0x800000, 0xFFFFFF, 0xC0C0C0, 0x808080, 0x808080,
    0x000000, 0xC0C0C0, 0xFFFFFF, 0x000000, 0xC0C0C0, 0x000000, 0xE1FFFF,
];

pub const COLOR_SCROLLBAR: u32 = 0;
pub const COLOR_BACKGROUND: u32 = 1;
pub const COLOR_ACTIVECAPTION: u32 = 2;
pub const COLOR_INACTIVECAPTION: u32 = 3;
pub const COLOR_MENU: u32 = 4;
pub const COLOR_WINDOW: u32 = 5;
pub const COLOR_WINDOWFRAME: u32 = 6;
pub const COLOR_MENUTEXT: u32 = 7;
pub const COLOR_WINDOWTEXT: u32 = 8;
pub const COLOR_CAPTIONTEXT: u32 = 9;
pub const COLOR_ACTIVEBORDER: u32 = 10;
pub const COLOR_APPWORKSPACE: u32 = 12;
pub const COLOR_HIGHLIGHT: u32 = 13;
pub const COLOR_HIGHLIGHTTEXT: u32 = 14;
pub const COLOR_BTNFACE: u32 = 15;
pub const COLOR_BTNSHADOW: u32 = 16;
pub const COLOR_GRAYTEXT: u32 = 17;
pub const COLOR_BTNTEXT: u32 = 18;
pub const COLOR_INACTIVECAPTIONTEXT: u32 = 19;
pub const COLOR_BTNHIGHLIGHT: u32 = 20;
pub const COLOR_3DDKSHADOW: u32 = 21;
pub const COLOR_3DLIGHT: u32 = 22;
pub const COLOR_INFOTEXT: u32 = 23;
pub const COLOR_INFOBK: u32 = 24;

/// The colors and their brushes, made when USER starts. The brushes
/// are GDI's own, which DeleteObject leaves.
pub fn init_colors(cpu: &mut Cpu) {
    gdi32::init(cpu);
    let mut brushes = Vec::with_capacity(COLORS.len());
    for &color in &COLORS {
        brushes.push(gdi32::add_stock(
            cpu,
            Kind::Brush {
                brush: Brush {
                    style: gdi32::BS_SOLID,
                    color,
                    ..Brush::default()
                },
            },
        ));
    }
    let state = user(cpu);
    state.colors = COLORS.to_vec();
    state.color_brushes = brushes;
}

/// GetSysColor(index).
pub fn get_sys_color(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    super::init(cpu);
    Some(user(cpu).color(args[0]))
}

/// GetSysColorBrush(index).
pub fn get_sys_color_brush(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    super::init(cpu);
    Some(brush(cpu, args[0]))
}

/// The brush of the color `index`, or 0.
pub fn brush(cpu: &mut Cpu, index: u32) -> u32 {
    user(cpu)
        .color_brushes
        .get(index as usize)
        .copied()
        .unwrap_or(0)
}

/// SetSysColors(count, indices, colors).
pub fn set_sys_colors(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    super::init(cpu);
    let count = args[0] as usize;
    let indices = read_words(cpu, args[1], count);
    let colors = read_words(cpu, args[2], count);
    if cpu.bus.guest_faulted() {
        return None;
    }
    for (&i, &c) in indices.iter().zip(&colors) {
        let i = i as usize;
        let state = user(cpu);
        if i >= state.colors.len() {
            continue;
        }
        state.colors[i] = c & 0xFF_FFFF;
        let handle = state.color_brushes[i];
        if let Some(gdi32::Object {
            kind: Kind::Brush { brush },
            ..
        }) = gdi32::gdi(cpu).get_mut(handle)
        {
            brush.color = c & 0xFF_FFFF;
        }
    }
    if user(cpu).desktop {
        super::paint::redraw_screen(cpu);
    }
    Some(TRUE)
}

/// The LOGFONTA of the screen's 8-point MS Sans Serif, bold or not.
pub fn sans_serif(bold: bool) -> Vec<u8> {
    let weight = if bold { 700 } else { 400 };
    gdi32::font::logfont(-11, 0, weight, 0, 0x22, "MS Sans Serif")
}

/// Write `value` to `at` as a 32-bit number.
fn put(cpu: &mut Cpu, at: u32, value: u32) -> u32 {
    if at == 0 {
        return FALSE;
    }
    cpu.bus.guest_write_32(at, value);
    TRUE
}

/// The value SystemParametersInfoA set for `action`, or `default`.
fn param(cpu: &mut Cpu, action: u32, default: u32) -> u32 {
    user(cpu)
        .params
        .iter()
        .find(|p| p.0 == action)
        .map_or(default, |p| p.1)
}

fn set_param(cpu: &mut Cpu, action: u32, value: u32) {
    let params = &mut user(cpu).params;
    match params.iter_mut().find(|p| p.0 == action) {
        Some(p) => p.1 = value,
        None => params.push((action, value)),
    }
}

/// SystemParametersInfoA(action, ui, pv, flags).
pub fn system_parameters_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (action, ui, pv) = (args[0], args[1], args[2]);
    let (w, h) = screen_size(cpu);
    let result = match action {
        // The GET actions whose value a SET action keeps: SPI_GETBEEP,
        // SPI_GETKEYBOARDSPEED, SPI_GETSCREENSAVETIMEOUT,
        // SPI_GETSCREENSAVEACTIVE, SPI_GETKEYBOARDDELAY,
        // SPI_GETDRAGFULLWINDOWS, SPI_GETSHOWSOUNDS, SPI_GETMENUSHOWDELAY.
        1 => {
            let v = param(cpu, 2, TRUE);
            put(cpu, pv, v)
        }
        10 => {
            let v = param(cpu, 11, 31);
            put(cpu, pv, v)
        }
        14 => {
            let v = param(cpu, 15, 0);
            put(cpu, pv, v)
        }
        16 => {
            let v = param(cpu, 17, FALSE);
            put(cpu, pv, v)
        }
        22 => {
            let v = param(cpu, 23, 1);
            put(cpu, pv, v)
        }
        38 => {
            let v = param(cpu, 37, FALSE);
            put(cpu, pv, v)
        }
        56 => {
            let v = param(cpu, 57, FALSE);
            put(cpu, pv, v)
        }
        106 => {
            let v = param(cpu, 107, 400);
            put(cpu, pv, v)
        }
        2 | 11 | 15 | 17 | 23 | 37 | 57 | 107 => {
            set_param(cpu, action, ui);
            TRUE
        }
        // SPI_GETMOUSE: the thresholds and speed.
        3 => {
            if pv == 0 {
                FALSE
            } else {
                write_words(cpu, pv, &[6, 10, 1]);
                TRUE
            }
        }
        // SPI_GETBORDER.
        5 => put(cpu, pv, 1),
        // SPI_ICONHORIZONTALSPACING and SPI_ICONVERTICALSPACING: the
        // spacing, written when `pv` is given.
        13 | 24 => {
            if pv != 0 {
                cpu.bus.guest_write_32(pv, 75);
            }
            TRUE
        }
        // SPI_GETICONTITLEWRAP, SPI_GETFASTTASKSWITCH.
        25 | 35 => put(cpu, pv, TRUE),
        // SPI_GETMENUDROPALIGNMENT, SPI_GETKEYBOARDPREF,
        // SPI_GETSCREENREADER, SPI_GETFONTSMOOTHING,
        // SPI_GETLOWPOWERACTIVE, SPI_GETPOWEROFFACTIVE,
        // SPI_GETMOUSETRAILS.
        27 | 68 | 70 | 74 | 83 | 84 | 94 => put(cpu, pv, 0),
        // SPI_GETICONTITLELOGFONT.
        31 => {
            if pv == 0 {
                FALSE
            } else {
                cpu.bus.guest_write_bytes(pv, &sans_serif(false));
                TRUE
            }
        }
        // SPI_GETNONCLIENTMETRICS.
        41 => {
            if pv == 0 || cpu.bus.guest_read_32(pv) != 340 {
                FALSE
            } else {
                let mut b = Vec::with_capacity(340);
                for v in [340u32, 1, 16, 16, 18, 18] {
                    b.extend_from_slice(&v.to_le_bytes());
                }
                b.extend_from_slice(&sans_serif(true));
                for v in [13u32, 15] {
                    b.extend_from_slice(&v.to_le_bytes());
                }
                b.extend_from_slice(&sans_serif(true));
                for v in [18u32, 18] {
                    b.extend_from_slice(&v.to_le_bytes());
                }
                for _ in 0..3 {
                    b.extend_from_slice(&sans_serif(false));
                }
                cpu.bus.guest_write_bytes(pv, &b);
                TRUE
            }
        }
        // SPI_GETMINIMIZEDMETRICS.
        43 => {
            if pv == 0 || cpu.bus.guest_read_32(pv) != 20 {
                FALSE
            } else {
                write_words(cpu, pv, &[20, 160, 0, 0, 8]);
                TRUE
            }
        }
        // SPI_GETICONMETRICS.
        45 => {
            if pv == 0 || cpu.bus.guest_read_32(pv) != 76 {
                FALSE
            } else {
                write_words(cpu, pv, &[76, 75, 75, 1]);
                cpu.bus.guest_write_bytes(pv + 16, &sans_serif(false));
                TRUE
            }
        }
        // SPI_SETWORKAREA, SPI_GETWORKAREA.
        47 => TRUE,
        48 => {
            if pv == 0 {
                FALSE
            } else {
                write_words(cpu, pv, &[0, 0, w as u32, h as u32]);
                TRUE
            }
        }
        // SPI_GETDEFAULTINPUTLANG.
        89 => put(cpu, pv, 0x0409_0409),
        // The SET actions that change nothing here.
        4
        | 6
        | 7
        | 12
        | 18..=21
        | 26
        | 28..=30
        | 32..=34
        | 36
        | 39
        | 40
        | 42
        | 44
        | 46
        | 49..=55
        | 58
        | 59
        | 61
        | 63
        | 65
        | 69
        | 71..=73
        | 75..=82
        | 85..=88
        | 90..=93
        | 95 => TRUE,
        _ => FALSE,
    };
    Some(result)
}

/// The DEVMODEA of the mode `w` x `h` x `bpp`.
fn devmode((w, h, bpp): (u32, u32, u32)) -> Vec<u8> {
    let mut b = vec![0u8; 124];
    b[..7].copy_from_slice(b"DISPLAY");
    b[32..34].copy_from_slice(&0x0400u16.to_le_bytes());
    b[36..38].copy_from_slice(&124u16.to_le_bytes());
    // DM_BITSPERPEL, DM_PELSWIDTH, DM_PELSHEIGHT, DM_DISPLAYFREQUENCY.
    b[40..44].copy_from_slice(&0x005C_0000u32.to_le_bytes());
    b[104..108].copy_from_slice(&bpp.to_le_bytes());
    b[108..112].copy_from_slice(&w.to_le_bytes());
    b[112..116].copy_from_slice(&h.to_le_bytes());
    b[120..124].copy_from_slice(&60u32.to_le_bytes());
    b
}

/// EnumDisplaySettingsA(device, mode, devmode): the display's mode now
/// (ENUM_CURRENT_SETTINGS), the desktop's own (ENUM_REGISTRY_SETTINGS),
/// or the `mode`th of `display_modes`.
pub fn enum_display_settings(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (mode, out) = (args[1], args[2]);
    let mode = match mode {
        0xFFFF_FFFF => display_mode(cpu),
        0xFFFF_FFFE => {
            let (w, h) = screen::DESKTOP;
            (w, h, screen::desktop(cpu).bpp)
        }
        i => match display_modes().get(i as usize) {
            Some(&m) => m,
            None => return Some(FALSE),
        },
    };
    if out == 0 {
        return Some(FALSE);
    }
    cpu.bus.guest_write_bytes(out, &devmode(mode));
    Some(TRUE)
}

/// ChangeDisplaySettingsA(devmode, flags): set the mode the DEVMODEA
/// gives (`set_display_mode`), the desktop's for what it leaves out;
/// with no DEVMODEA the desktop's own mode again. CDS_TEST only says
/// whether the mode is there.
pub fn change_display_settings(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    const CDS_TEST: u32 = 2;
    let (at, flags) = (args[0], args[1]);
    if at == 0 {
        return Some(set_display_mode(cpu, None));
    }
    let fields = cpu.bus.guest_read_32(at + 40);
    let d = screen::desktop(cpu);
    let field = |cpu: &mut Cpu, bit: u32, offset: u32, own: u32| {
        if fields & bit != 0 {
            cpu.bus.guest_read_32(at + offset)
        } else {
            own
        }
    };
    // DM_BITSPERPEL, DM_PELSWIDTH and DM_PELSHEIGHT.
    let bpp = field(cpu, 0x4_0000, 104, d.bpp);
    let width = field(cpu, 0x8_0000, 108, d.width);
    let height = field(cpu, 0x10_0000, 112, d.height);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let mode = (width, height, bpp);
    if flags & CDS_TEST != 0 {
        return Some(if settable(cpu, mode) {
            DISP_CHANGE_SUCCESSFUL
        } else {
            DISP_CHANGE_BADMODE
        });
    }
    Some(set_display_mode(cpu, Some(mode)))
}

/// GetProcessWindowStation: the one window station.
pub fn get_process_window_station(_cpu: &mut Cpu, _args: &[u32]) -> Option<u32> {
    Some(0x0030)
}

/// GetThreadDesktop(tid): the one desktop.
pub fn get_thread_desktop(_cpu: &mut Cpu, _args: &[u32]) -> Option<u32> {
    Some(0x0034)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapped_frame_is_windows_95s() {
        // AdjustWindowRect of WS_OVERLAPPEDWINDOW: 4 around, 19 more on
        // top for the caption.
        assert_eq!(metric(32), 4);
        assert_eq!(metric(4), 19);
        assert_eq!(COLORS.len(), 25);
        assert_eq!(COLORS[COLOR_BTNFACE as usize], 0xC0C0C0);
    }
}
