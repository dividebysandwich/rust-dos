//! Device contexts: what they draw on and with, their coordinates
//! (mapping modes) and their clipping.
//!
//! A DC's device coordinates have their origin at its surface's
//! `origin`: a window's or its client area's corner on the screen for a
//! DC of USER's, the corner of the bitmap or the screen for the others.
//! What it may draw on is its visible region (USER's, for a window DC),
//! the clip region the program selected and the meta region.

use super::region::{
    COMPLEXREGION, ERROR, NULLREGION, Op, RGN_AND, RGN_COPY, RGN_DIFF, SIMPLEREGION,
};
use super::surface::Target;
use super::{
    ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, FALSE, GDI_ERROR, Kind, Rect, Region, State,
    TRUE, add, gdi, init, set_error,
};
use crate::cpu::Cpu;
use crate::win32::user32::{read_words, write_words};

/// What a DC is.
pub const DISPLAY: u32 = 0;
pub const WINDOW: u32 = 1;
pub const MEMORY: u32 = 2;
pub const INFO: u32 = 3;

/// GetDCEx's flags, which a window DC keeps.
pub const DCX_WINDOW: u32 = 0x0001;
pub const DCX_CACHE: u32 = 0x0002;
pub const DCX_NORESETATTRS: u32 = 0x0004;
pub const DCX_CLIPCHILDREN: u32 = 0x0008;
pub const DCX_CLIPSIBLINGS: u32 = 0x0010;
pub const DCX_PARENTCLIP: u32 = 0x0020;
pub const DCX_EXCLUDERGN: u32 = 0x0040;
pub const DCX_INTERSECTRGN: u32 = 0x0080;
pub const DCX_EXCLUDEUPDATE: u32 = 0x0100;
pub const DCX_INTERSECTUPDATE: u32 = 0x0200;
pub const DCX_USESTYLE: u32 = 0x0001_0000;

/// The modes.
pub const TRANSPARENT: u32 = 1;
pub const OPAQUE: u32 = 2;
pub const ALTERNATE: u32 = 1;
pub const WINDING: u32 = 2;
pub const R2_COPYPEN: u32 = 13;
pub const MM_TEXT: u32 = 1;
pub const MM_LOMETRIC: u32 = 2;
pub const MM_HIMETRIC: u32 = 3;
pub const MM_LOENGLISH: u32 = 4;
pub const MM_HIENGLISH: u32 = 5;
pub const MM_TWIPS: u32 = 6;
pub const MM_ISOTROPIC: u32 = 7;
pub const MM_ANISOTROPIC: u32 = 8;
pub const GM_COMPATIBLE: u32 = 1;
pub const AD_COUNTERCLOCKWISE: u32 = 1;
pub const BLACKONWHITE: u32 = 1;

/// The attributes of a DC that SaveDC keeps.
#[derive(Clone, Debug, Default)]
pub struct Attrs {
    pub pen: u32,
    pub brush: u32,
    pub font: u32,
    pub bitmap: u32,
    pub palette: u32,
    pub text_color: u32,
    pub bk_color: u32,
    pub bk_mode: u32,
    pub rop2: u32,
    pub poly_fill: u32,
    pub stretch: u32,
    pub text_align: u32,
    pub map_mode: u32,
    pub window_org: (i32, i32),
    pub window_ext: (i32, i32),
    pub viewport_org: (i32, i32),
    pub viewport_ext: (i32, i32),
    pub brush_org: (i32, i32),
    pub pos: (i32, i32),
    /// The clip and meta regions, in device coordinates.
    pub clip: Option<Region>,
    pub meta: Option<Region>,
    pub char_extra: i32,
    pub break_extra: i32,
    pub break_count: i32,
    pub arc_dir: u32,
    pub mapper: u32,
    pub graphics_mode: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Dc {
    pub kind: u32,
    /// A window DC's window and GetDCEx's flags.
    pub hwnd: u32,
    pub flags: u32,
    /// The region GetDCEx or BeginPaint gave it, on the screen.
    pub extra: Option<Region>,
    /// The region handle GetDCEx was given, which the DC now owns: it is
    /// deleted when the DC lets the region go.
    pub extra_rgn: u32,
    /// A window's own or class DC: it stays when released.
    pub own: bool,
    pub attrs: Attrs,
    pub saved: Vec<Attrs>,
    /// SetBoundsRect's rectangle, in device coordinates, and whether it
    /// gathers.
    pub bounds: Rect,
    pub bounds_on: bool,
    /// The path being made, in device coordinates: its figures' points
    /// and kinds (PT_MOVETO and the rest), and whether it is closed.
    pub path: Vec<(i32, i32, u8)>,
    pub path_state: u32,
}

impl Dc {
    pub fn new(kind: u32) -> Dc {
        Dc {
            kind,
            attrs: default_attrs(),
            ..Dc::default()
        }
    }

    /// The device point of the logical point (`x`, `y`).
    pub fn to_device(&self, x: i32, y: i32) -> (i32, i32) {
        let a = &self.attrs;
        let map = |v: i32, wo: i32, we: i32, vo: i32, ve: i32| -> i32 {
            if we == ve {
                return v - wo + vo;
            }
            let f = (v as f64 - wo as f64) * ve as f64 / we as f64 + vo as f64;
            (f + 0.5).floor() as i32
        };
        (
            map(
                x,
                a.window_org.0,
                a.window_ext.0,
                a.viewport_org.0,
                a.viewport_ext.0,
            ),
            map(
                y,
                a.window_org.1,
                a.window_ext.1,
                a.viewport_org.1,
                a.viewport_ext.1,
            ),
        )
    }

    /// The logical point of the device point (`x`, `y`).
    pub fn to_logical(&self, x: i32, y: i32) -> (i32, i32) {
        let a = &self.attrs;
        let map = |v: i32, wo: i32, we: i32, vo: i32, ve: i32| -> i32 {
            if we == ve {
                return v - vo + wo;
            }
            if ve == 0 {
                return wo;
            }
            let f = (v as f64 - vo as f64) * we as f64 / ve as f64 + wo as f64;
            (f + 0.5).floor() as i32
        };
        (
            map(
                x,
                a.window_org.0,
                a.window_ext.0,
                a.viewport_org.0,
                a.viewport_ext.0,
            ),
            map(
                y,
                a.window_org.1,
                a.window_ext.1,
                a.viewport_org.1,
                a.viewport_ext.1,
            ),
        )
    }

    /// A logical rectangle in device coordinates, its corners ordered.
    pub fn rect_to_device(&self, r: &Rect) -> Rect {
        let (l, t) = self.to_device(r.left, r.top);
        let (rr, b) = self.to_device(r.right, r.bottom);
        Rect::new(l, t, rr, b).ordered()
    }

    /// A length along x in device units (pen widths).
    pub fn width_to_device(&self, w: i32) -> i32 {
        let a = &self.attrs;
        if a.window_ext.0 == a.viewport_ext.0 || a.window_ext.0 == 0 {
            return w;
        }
        ((w as f64 * a.viewport_ext.0 as f64 / a.window_ext.0 as f64).abs() + 0.5).floor() as i32
    }

    /// Whether `handle` is selected in it, now or in a saved state.
    pub fn holds(&self, handle: u32) -> bool {
        std::iter::once(&self.attrs)
            .chain(&self.saved)
            .any(|a| [a.pen, a.brush, a.font, a.bitmap, a.palette].contains(&handle))
    }
}

/// A new DC's attributes, as Windows sets them.
pub fn default_attrs() -> Attrs {
    Attrs {
        pen: super::handle(super::BLACK_PEN as usize),
        brush: super::handle(super::WHITE_BRUSH as usize),
        font: super::handle(super::SYSTEM_FONT as usize),
        bitmap: 0,
        palette: super::handle(super::DEFAULT_PALETTE as usize),
        text_color: 0,
        bk_color: 0xFFFFFF,
        bk_mode: OPAQUE,
        rop2: R2_COPYPEN,
        poly_fill: ALTERNATE,
        stretch: BLACKONWHITE,
        text_align: 0,
        map_mode: MM_TEXT,
        window_org: (0, 0),
        window_ext: (1, 1),
        viewport_org: (0, 0),
        viewport_ext: (1, 1),
        brush_org: (0, 0),
        pos: (0, 0),
        clip: None,
        meta: None,
        char_extra: 0,
        break_extra: 0,
        break_count: 0,
        arc_dir: AD_COUNTERCLOCKWISE,
        mapper: 0,
        graphics_mode: GM_COMPATIBLE,
    }
}

/// Whether any DC holds `handle`.
pub fn selected_anywhere(state: &State, handle: u32) -> bool {
    state.objects.iter().flatten().any(|o| match &o.kind {
        Kind::Dc { dc } => dc.holds(handle),
        _ => false,
    })
}

/// The DC `hdc`, or None with ERROR_INVALID_HANDLE.
fn get(cpu: &mut Cpu, hdc: u32) -> Option<Dc> {
    match gdi(cpu).dc(hdc) {
        Some(dc) => Some(dc.clone()),
        None => {
            set_error(cpu, ERROR_INVALID_HANDLE);
            None
        }
    }
}

/// Change the DC `hdc` with `f`: what it returns, or None if it isn't a
/// DC.
pub fn with<T>(cpu: &mut Cpu, hdc: u32, f: impl FnOnce(&mut Dc) -> T) -> Option<T> {
    match gdi(cpu).dc_mut(hdc) {
        Some(dc) => Some(f(dc)),
        None => {
            set_error(cpu, ERROR_INVALID_HANDLE);
            None
        }
    }
}

/// What a DC draws on: its surface, where its device origin is there,
/// and the region it may draw on, in the surface's coordinates.
pub struct Place {
    pub target: Target,
    pub origin: (i32, i32),
    pub clip: Region,
}

/// The device's size: the screen's for all but a memory DC, whose
/// bitmap's it is.
pub fn device_size(cpu: &Cpu, dc: &Dc) -> (i32, i32) {
    if dc.kind == MEMORY
        && let Some(b) = cpu.bus.win32.gdi.bitmap(dc.attrs.bitmap)
    {
        return (b.width, b.height);
    }
    crate::win32::user32::sysparams::screen_size()
}

/// The visible region of the DC, in device coordinates, and where its
/// device origin is on the surface.
pub fn visible(cpu: &mut Cpu, dc: &Dc) -> (Region, (i32, i32)) {
    match dc.kind {
        WINDOW => {
            let (origin, vis) =
                crate::win32::user32::paint::dc_region(cpu, dc.hwnd, dc.flags, dc.extra.as_ref());
            (vis.offset_by(-origin.0, -origin.1), origin)
        }
        INFO => (Region::new(), (0, 0)),
        _ => {
            let (w, h) = device_size(cpu, dc);
            (Region::from_rect(Rect::new(0, 0, w, h)), (0, 0))
        }
    }
}

/// The region a DC may draw on, in device coordinates: its visible
/// region, clip region and meta region.
pub fn effective_clip(cpu: &mut Cpu, dc: &Dc) -> (Region, (i32, i32)) {
    let (mut region, origin) = visible(cpu, dc);
    if let Some(clip) = &dc.attrs.clip {
        region = region.and(clip);
    }
    if let Some(meta) = &dc.attrs.meta {
        region = region.and(meta);
    }
    (region, origin)
}

/// Where the DC `hdc` draws: None if it can't (an information DC, or a
/// memory DC without its bitmap).
pub fn place(cpu: &mut Cpu, hdc: u32) -> Option<Place> {
    let dc = gdi(cpu).dc(hdc)?.clone();
    let target = match dc.kind {
        DISPLAY | WINDOW => {
            crate::win32::user32::paint::ensure_desktop(cpu);
            Target::Screen
        }
        MEMORY => {
            gdi(cpu).bitmap(dc.attrs.bitmap)?;
            Target::Bitmap(dc.attrs.bitmap)
        }
        _ => return None,
    };
    let (clip, origin) = effective_clip(cpu, &dc);
    Some(Place {
        target,
        origin,
        clip: clip.offset_by(origin.0, origin.1),
    })
}

/// Add the device rectangle `r` to the DC's bounds, if it gathers them.
pub fn accumulate(cpu: &mut Cpu, hdc: u32, r: Rect) {
    if let Some(dc) = gdi(cpu).dc_mut(hdc)
        && dc.bounds_on
        && !r.is_empty()
    {
        dc.bounds = dc.bounds.union(&r);
    }
}

/// A new DC of `kind`, its handle (0 when GDI's table is full).
pub fn new_dc(cpu: &mut Cpu, kind: u32) -> u32 {
    init(cpu);
    let mut dc = Dc::new(kind);
    if kind == MEMORY {
        dc.attrs.bitmap = super::handle(super::DEFAULT_BITMAP as usize);
    }
    add(cpu, Kind::Dc { dc })
}

/// A DC of USER's for `hwnd`, with GetDCEx's flags and region (on the
/// screen).
pub fn new_window_dc(
    cpu: &mut Cpu,
    hwnd: u32,
    flags: u32,
    extra: Option<Region>,
    own: bool,
) -> u32 {
    let hdc = new_dc(cpu, WINDOW);
    with(cpu, hdc, |dc| {
        dc.hwnd = hwnd;
        dc.flags = flags;
        dc.extra = extra;
        dc.own = own;
    });
    hdc
}

/// A window's own DC (CS_OWNDC) goes with it.
pub fn free_window_dc(cpu: &mut Cpu, hdc: u32) {
    release_objects(cpu, hdc);
    super::free(cpu, hdc);
}

/// The window `hwnd` went: the DCs of USER's for it go too, but class
/// DCs, which other windows of its class use.
pub fn window_gone(cpu: &mut Cpu, hwnd: u32) {
    let list: Vec<u32> = gdi(cpu)
        .objects
        .iter()
        .enumerate()
        .filter_map(|(i, o)| match &o.as_ref()?.kind {
            Kind::Dc { dc } if dc.kind == WINDOW && dc.hwnd == hwnd && !dc.own => {
                Some(super::handle(i))
            }
            _ => None,
        })
        .collect();
    for hdc in list {
        free_window_dc(cpu, hdc);
    }
}

/// Free what a DC that goes held and DeleteObject deleted meanwhile.
fn release_objects(cpu: &mut Cpu, hdc: u32) {
    let Some(dc) = gdi(cpu).dc(hdc).cloned() else {
        return;
    };
    let mut held: Vec<u32> = Vec::new();
    for a in std::iter::once(&dc.attrs).chain(&dc.saved) {
        held.extend([a.pen, a.brush, a.font, a.bitmap, a.palette]);
    }
    if let Some(dc) = gdi(cpu).dc_mut(hdc) {
        dc.attrs = Attrs::default();
        dc.saved.clear();
    }
    for h in held {
        if let Some(b) = gdi(cpu).bitmap_mut(h)
            && b.dc == hdc
        {
            b.dc = 0;
        }
        free_if_deleted(cpu, h);
    }
}

/// Free the object `handle` if DeleteObject deleted it and no DC holds
/// it any more.
pub fn free_if_deleted(cpu: &mut Cpu, handle: u32) {
    let deleted = gdi(cpu).get(handle).is_some_and(|o| o.deleted);
    if deleted && !selected_anywhere(gdi(cpu), handle) {
        super::free(cpu, handle);
    }
}

/// CreateDCA(driver, device, output, devmode): the display only.
pub fn create_dc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    create(cpu, args, DISPLAY)
}

/// CreateICA: a DC that only gives information.
pub fn create_ic(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    create(cpu, args, INFO)
}

fn create(cpu: &mut Cpu, args: &[u32], kind: u32) -> Option<u32> {
    let driver = if args[0] != 0 {
        crate::win32::user32::string_at(cpu, args[0])?
    } else if args[1] != 0 {
        crate::win32::user32::string_at(cpu, args[1])?
    } else {
        String::new()
    };
    let name = driver.trim_start_matches("\\\\.\\").to_ascii_uppercase();
    if !(name == "DISPLAY" || name == "DISPLAY1") {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    Some(new_dc(cpu, kind))
}

/// CreateCompatibleDC(hdc): a memory DC, its bitmap the 1x1 monochrome
/// one.
pub fn create_compatible_dc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    init(cpu);
    if args[0] != 0 && gdi(cpu).dc(args[0]).is_none() {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    }
    Some(new_dc(cpu, MEMORY))
}

/// DeleteDC(hdc).
pub fn delete_dc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hdc = args[0];
    let Some(dc) = get(cpu, hdc) else {
        return Some(FALSE);
    };
    if dc.kind == WINDOW && dc.own {
        // A window's own or class DC stays while the window does, and
        // DeleteDC claims it went.
        return Some(TRUE);
    }
    release_objects(cpu, hdc);
    super::free(cpu, hdc);
    Some(TRUE)
}

/// ResetDCA: a display DC stays as it is.
pub fn reset_dc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(if gdi(cpu).dc(args[0]).is_some() {
        args[0]
    } else {
        0
    })
}

/// SaveDC(hdc): how many states are saved now.
pub fn save_dc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(
        with(cpu, args[0], |dc| {
            dc.saved.push(dc.attrs.clone());
            dc.saved.len() as u32
        })
        .unwrap_or(0),
    )
}

/// RestoreDC(hdc, level): the state saved at `level`, or the one
/// `-level` back.
pub fn restore_dc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, level) = (args[0], args[1] as i32);
    let Some(dc) = get(cpu, hdc) else {
        return Some(FALSE);
    };
    let count = dc.saved.len() as i32;
    let level = if level < 0 { count + level + 1 } else { level };
    if level <= 0 || level > count {
        return Some(FALSE);
    }
    let old_bitmap = dc.attrs.bitmap;
    let mut released = Vec::new();
    with(cpu, hdc, |dc| {
        let attrs = dc.saved[level as usize - 1].clone();
        let gone = dc.saved.split_off(level as usize - 1);
        for a in std::iter::once(&dc.attrs).chain(&gone) {
            released.extend([a.pen, a.brush, a.font, a.bitmap, a.palette]);
        }
        dc.attrs = attrs;
    });
    let new_bitmap = gdi(cpu).dc(hdc).map_or(0, |d| d.attrs.bitmap);
    if old_bitmap != new_bitmap {
        if let Some(b) = gdi(cpu).bitmap_mut(old_bitmap) {
            b.dc = 0;
        }
        if let Some(b) = gdi(cpu).bitmap_mut(new_bitmap) {
            b.dc = hdc;
        }
    }
    for h in released {
        free_if_deleted(cpu, h);
    }
    Some(TRUE)
}

/// GetDeviceCaps(hdc, index): the display's, a 640x480 one at 96 pixels
/// an inch, with the desktop's depth: 32 bits a pixel, or 16 on a
/// Banshee or Voodoo3.
pub fn get_device_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if get(cpu, args[0]).is_none() {
        return Some(0);
    }
    let bpp = crate::win32::screen::desktop(cpu).bpp;
    match args[1] {
        BITSPIXEL => return Some(bpp),
        // COLORRES: the bits of a color the display shows.
        108 if bpp == 16 => return Some(16),
        _ => {}
    }
    Some(device_cap(args[1]))
}

/// GetDeviceCaps' index of the bits a pixel.
const BITSPIXEL: u32 = 12;

/// The display's capability `index`, the depth that of the pixels GDI
/// works on (32 bits): its compatible bitmaps'.
pub fn device_cap(index: u32) -> u32 {
    let (w, h) = crate::win32::user32::sysparams::screen_size();
    // Millimeters at 96 pixels an inch.
    let mm = |pixels: i32| ((pixels as f64 * 25.4 / 96.0) + 0.5) as u32;
    match index {
        0 => 0x400,
        2 => 1,
        4 => mm(w),
        6 => mm(h),
        8 => w as u32,
        10 => h as u32,
        12 => 32,
        14 => 1,
        16 | 18 => 0xFFFF_FFFF,
        20 | 22 => 0,
        24 => 0xFFFF_FFFF,
        26 => 0,
        28 => 0xFF,
        30 => 0xFE,
        32 => 0xFF,
        34 => 0x7807,
        36 => 1,
        38 => 0x7E99,
        40 | 42 => 36,
        44 => 51,
        88 | 90 => 96,
        104 | 106 => 0,
        108 => 24,
        116 => 60,
        117 => h as u32,
        118 => w as u32,
        _ => 0,
    }
}

/// GetDCOrgEx(hdc, point): where the DC's device origin is on the
/// screen.
pub fn get_dc_org_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(dc) = get(cpu, args[0]) else {
        return Some(FALSE);
    };
    let (_, origin) = visible(cpu, &dc);
    write_words(cpu, args[1], &[origin.0 as u32, origin.1 as u32]);
    Some(TRUE)
}

/// Set an attribute of the DC with `f`, which returns the old value;
/// `fail` if it isn't a DC.
fn set_attr(cpu: &mut Cpu, hdc: u32, fail: u32, f: impl FnOnce(&mut Attrs) -> u32) -> Option<u32> {
    Some(with(cpu, hdc, |dc| f(&mut dc.attrs)).unwrap_or(fail))
}

fn get_attr(cpu: &mut Cpu, hdc: u32, fail: u32, f: impl FnOnce(&Attrs) -> u32) -> Option<u32> {
    Some(get(cpu, hdc).map_or(fail, |dc| f(&dc.attrs)))
}

pub fn set_bk_color(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let c = args[1];
    set_attr(cpu, args[0], super::CLR_INVALID, |a| {
        std::mem::replace(&mut a.bk_color, c)
    })
}

pub fn get_bk_color(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_attr(cpu, args[0], super::CLR_INVALID, |a| a.bk_color)
}

pub fn set_bk_mode(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mode = args[1];
    if !(TRANSPARENT..=OPAQUE).contains(&mode) {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    set_attr(cpu, args[0], 0, |a| std::mem::replace(&mut a.bk_mode, mode))
}

pub fn get_bk_mode(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_attr(cpu, args[0], 0, |a| a.bk_mode)
}

pub fn set_text_color(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let c = args[1];
    set_attr(cpu, args[0], super::CLR_INVALID, |a| {
        std::mem::replace(&mut a.text_color, c)
    })
}

pub fn get_text_color(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_attr(cpu, args[0], super::CLR_INVALID, |a| a.text_color)
}

pub fn set_rop2(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mode = args[1];
    if !(1..=16).contains(&mode) {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    set_attr(cpu, args[0], 0, |a| std::mem::replace(&mut a.rop2, mode))
}

pub fn get_rop2(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_attr(cpu, args[0], 0, |a| a.rop2)
}

pub fn set_poly_fill_mode(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mode = args[1];
    if !(ALTERNATE..=WINDING).contains(&mode) {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    set_attr(cpu, args[0], 0, |a| {
        std::mem::replace(&mut a.poly_fill, mode)
    })
}

pub fn get_poly_fill_mode(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_attr(cpu, args[0], 0, |a| a.poly_fill)
}

pub fn set_stretch_blt_mode(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mode = args[1];
    if !(1..=4).contains(&mode) {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    set_attr(cpu, args[0], 0, |a| std::mem::replace(&mut a.stretch, mode))
}

pub fn get_stretch_blt_mode(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_attr(cpu, args[0], 0, |a| a.stretch)
}

pub fn set_text_align(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let align = args[1];
    set_attr(cpu, args[0], GDI_ERROR, |a| {
        std::mem::replace(&mut a.text_align, align)
    })
}

pub fn get_text_align(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_attr(cpu, args[0], GDI_ERROR, |a| a.text_align)
}

pub fn set_text_character_extra(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let extra = args[1] as i32;
    set_attr(cpu, args[0], 0x8000_0000, |a| {
        std::mem::replace(&mut a.char_extra, extra) as u32
    })
}

pub fn get_text_character_extra(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_attr(cpu, args[0], 0x8000_0000, |a| a.char_extra as u32)
}

/// SetTextJustification(hdc, extra, count).
pub fn set_text_justification(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (extra, count) = (args[1] as i32, args[2] as i32);
    Some(
        with(cpu, args[0], |dc| {
            dc.attrs.break_extra = if count > 0 { extra } else { 0 };
            dc.attrs.break_count = count.max(0);
            TRUE
        })
        .unwrap_or(FALSE),
    )
}

pub fn set_arc_direction(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let dir = args[1];
    if dir != 1 && dir != 2 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    set_attr(cpu, args[0], 0, |a| std::mem::replace(&mut a.arc_dir, dir))
}

pub fn get_arc_direction(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_attr(cpu, args[0], 0, |a| a.arc_dir)
}

pub fn set_mapper_flags(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let flags = args[1];
    set_attr(cpu, args[0], GDI_ERROR, |a| {
        std::mem::replace(&mut a.mapper, flags)
    })
}

/// SetGraphicsMode(hdc, mode): Windows 95 has only GM_COMPATIBLE.
pub fn set_graphics_mode(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mode = args[1];
    if get(cpu, args[0]).is_none() {
        return Some(0);
    }
    if mode != GM_COMPATIBLE {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    Some(GM_COMPATIBLE)
}

pub fn get_graphics_mode(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_attr(cpu, args[0], 0, |a| a.graphics_mode)
}

/// GetNearestColor(hdc, color): on a 32-bit display, the color itself.
pub fn get_nearest_color(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(dc) = get(cpu, args[0]) else {
        return Some(super::CLR_INVALID);
    };
    let color = resolve_color(cpu, &dc, args[1]);
    if dc.kind == MEMORY
        && let Some(b) = gdi(cpu).bitmap(dc.attrs.bitmap)
        && b.bpp == 1
        && b.dib.is_none()
    {
        let sum = (color & 0xFF) + ((color >> 8) & 0xFF) + ((color >> 16) & 0xFF);
        return Some(if sum >= 384 { 0xFFFFFF } else { 0 });
    }
    Some(color & 0xFFFFFF)
}

/// The RGB color a COLORREF means in the DC: PALETTEINDEX takes the
/// entry of its palette, PALETTERGB the color.
pub fn resolve_color(cpu: &mut Cpu, dc: &Dc, color: u32) -> u32 {
    match color >> 24 {
        1 => {
            // An index past the palette's end wraps around it.
            let index = (color & 0xFFFF) as usize;
            let entry = gdi(cpu)
                .palette(dc.attrs.palette)
                .filter(|p| !p.entries.is_empty())
                .map(|p| p.entries[index % p.entries.len()])
                .unwrap_or([0; 4]);
            entry[0] as u32 | (entry[1] as u32) << 8 | (entry[2] as u32) << 16
        }
        _ => color & 0xFFFFFF,
    }
}

/// GetCurrentPositionEx(hdc, point).
pub fn get_current_position_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(dc) = get(cpu, args[0]) else {
        return Some(FALSE);
    };
    let (x, y) = dc.attrs.pos;
    if args[1] != 0 {
        write_words(cpu, args[1], &[x as u32, y as u32]);
    }
    Some(TRUE)
}

/// SetBrushOrgEx(hdc, x, y, old).
pub fn set_brush_org_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (x, y) = (args[1] as i32, args[2] as i32);
    let Some(old) = with(cpu, args[0], |dc| {
        std::mem::replace(&mut dc.attrs.brush_org, (x, y))
    }) else {
        return Some(FALSE);
    };
    if args[3] != 0 {
        write_words(cpu, args[3], &[old.0 as u32, old.1 as u32]);
    }
    Some(TRUE)
}

pub fn get_brush_org_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(dc) = get(cpu, args[0]) else {
        return Some(FALSE);
    };
    let (x, y) = dc.attrs.brush_org;
    write_words(cpu, args[1], &[x as u32, y as u32]);
    Some(TRUE)
}

/// SelectObject(hdc, object): the object it replaced, or for a region
/// what the clip region is now.
pub fn select_object(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, handle) = (args[0], args[1]);
    init(cpu);
    let Some(dc) = gdi(cpu).dc(hdc).cloned() else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    };
    let Some(object) = gdi(cpu).get(handle).cloned() else {
        // No object, as against a wrong one, sets no error.
        if handle != 0 {
            set_error(cpu, ERROR_INVALID_HANDLE);
        }
        return Some(0);
    };
    if object.deleted {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    }
    let old = match &object.kind {
        Kind::Pen { .. } => with(cpu, hdc, |d| std::mem::replace(&mut d.attrs.pen, handle)),
        Kind::Brush { .. } => with(cpu, hdc, |d| std::mem::replace(&mut d.attrs.brush, handle)),
        Kind::Font { .. } => with(cpu, hdc, |d| std::mem::replace(&mut d.attrs.font, handle)),
        Kind::Region { region } => {
            return Some(select_clip(cpu, hdc, Some(region.clone()), RGN_COPY));
        }
        Kind::Bitmap { bitmap } => {
            if dc.kind != MEMORY {
                return Some(0);
            }
            if bitmap.dc != 0 && bitmap.dc != hdc {
                return Some(0);
            }
            // A bitmap of GDI's must be monochrome or the display's
            // depth; a DIB section may be any.
            let depth = bitmap.bpp as u32 * bitmap.planes as u32;
            if bitmap.dib.is_none() && depth != 1 && depth != device_cap(12) {
                return Some(0);
            }
            let old = dc.attrs.bitmap;
            if let Some(b) = gdi(cpu).bitmap_mut(old) {
                b.dc = 0;
            }
            if let Some(b) = gdi(cpu).bitmap_mut(handle) {
                b.dc = hdc;
            }
            with(cpu, hdc, |d| {
                d.attrs.bitmap = handle;
                // The clip region goes with the old bitmap.
                d.attrs.clip = None;
            })
            .map(|_| old)
        }
        _ => {
            set_error(cpu, ERROR_INVALID_HANDLE);
            return Some(0);
        }
    };
    let old = old.unwrap_or(0);
    if old != handle {
        free_if_deleted(cpu, old);
    }
    Some(old)
}

/// The DC's clip region becomes `region` (device coordinates) combined
/// by `mode` with what it is: the kind of region it may draw on then.
pub fn select_clip(cpu: &mut Cpu, hdc: u32, region: Option<Region>, mode: u32) -> u32 {
    let Some(dc) = gdi(cpu).dc(hdc).cloned() else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return ERROR;
    };
    let (w, h) = device_size(cpu, &dc);
    let whole = Region::from_rect(Rect::new(0, 0, w, h));
    let current = dc.attrs.clip.clone();
    let new = match (region, mode) {
        (None, RGN_COPY) => None,
        (None, _) => {
            set_error(cpu, ERROR_INVALID_PARAMETER);
            return ERROR;
        }
        (Some(r), RGN_COPY) => Some(r),
        (Some(r), m) => {
            let base = current.unwrap_or_else(|| whole.clone());
            let op = match m {
                RGN_AND => Op::And,
                2 => Op::Or,
                3 => Op::Xor,
                RGN_DIFF => Op::Diff,
                _ => {
                    set_error(cpu, ERROR_INVALID_PARAMETER);
                    return ERROR;
                }
            };
            Some(base.combine(&r, op))
        }
    };
    with(cpu, hdc, |d| d.attrs.clip = new);
    clip_kind(cpu, hdc)
}

/// What the region the DC may draw on is.
pub fn clip_kind(cpu: &mut Cpu, hdc: u32) -> u32 {
    let Some(dc) = gdi(cpu).dc(hdc).cloned() else {
        return ERROR;
    };
    let (region, _) = effective_clip(cpu, &dc);
    region.kind()
}

/// SelectClipRgn(hdc, region).
pub fn select_clip_rgn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    ext_select_clip_rgn(cpu, &[args[0], args[1], RGN_COPY])
}

/// ExtSelectClipRgn(hdc, region, mode).
pub fn ext_select_clip_rgn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, hrgn, mode) = (args[0], args[1], args[2]);
    let region = if hrgn == 0 {
        None
    } else {
        match gdi(cpu).region(hrgn) {
            Some(r) => Some(r.clone()),
            None => {
                set_error(cpu, ERROR_INVALID_HANDLE);
                return Some(ERROR);
            }
        }
    };
    Some(select_clip(cpu, hdc, region, mode))
}

/// IntersectClipRect(hdc, left, top, right, bottom), in logical units.
pub fn intersect_clip_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    clip_rect(cpu, args, RGN_AND)
}

/// ExcludeClipRect(hdc, left, top, right, bottom).
pub fn exclude_clip_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    clip_rect(cpu, args, RGN_DIFF)
}

fn clip_rect(cpu: &mut Cpu, args: &[u32], mode: u32) -> Option<u32> {
    let Some(dc) = get(cpu, args[0]) else {
        return Some(ERROR);
    };
    let r = Rect::new(
        args[1] as i32,
        args[2] as i32,
        args[3] as i32,
        args[4] as i32,
    );
    let r = dc.rect_to_device(&r);
    Some(select_clip(cpu, args[0], Some(Region::from_rect(r)), mode))
}

/// OffsetClipRgn(hdc, x, y), in logical units.
pub fn offset_clip_rgn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(dc) = get(cpu, args[0]) else {
        return Some(ERROR);
    };
    let (x0, y0) = dc.to_device(0, 0);
    let (x1, y1) = dc.to_device(args[1] as i32, args[2] as i32);
    with(cpu, args[0], |d| {
        if let Some(clip) = &mut d.attrs.clip {
            clip.offset(x1 - x0, y1 - y0);
        }
    });
    Some(clip_kind(cpu, args[0]))
}

/// GetClipBox(hdc, rect): the bounds of what it may draw on, in logical
/// units.
pub fn get_clip_box(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(dc) = get(cpu, args[0]) else {
        return Some(ERROR);
    };
    let (region, _) = effective_clip(cpu, &dc);
    let b = region.bounds();
    let (l, t) = dc.to_logical(b.left, b.top);
    let (r, bt) = dc.to_logical(b.right, b.bottom);
    Rect::new(l, t, r, bt).ordered().write(cpu, args[1]);
    Some(region.kind())
}

/// GetClipRgn(hdc, region): 1 with the clip region copied, 0 if there is
/// none, -1 on error.
pub fn get_clip_rgn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(dc) = get(cpu, args[0]) else {
        return Some(-1i32 as u32);
    };
    let Some(clip) = dc.attrs.clip else {
        return Some(0);
    };
    if !super::set_region(cpu, args[1], clip) {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(-1i32 as u32);
    }
    Some(1)
}

/// GetMetaRgn(hdc, region).
pub fn get_meta_rgn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(dc) = get(cpu, args[0]) else {
        return Some(0);
    };
    let Some(meta) = dc.attrs.meta else {
        return Some(0);
    };
    Some(super::set_region(cpu, args[1], meta) as u32)
}

/// SetMetaRgn(hdc): the meta region becomes what it and the clip region
/// let through, and the clip region goes.
pub fn set_meta_rgn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(dc) = get(cpu, args[0]) else {
        return Some(ERROR);
    };
    let meta = match (dc.attrs.meta, dc.attrs.clip) {
        (Some(m), Some(c)) => Some(m.and(&c)),
        (m, c) => m.or(c),
    };
    with(cpu, args[0], |d| {
        d.attrs.meta = meta;
        d.attrs.clip = None;
    });
    Some(clip_kind(cpu, args[0]))
}

/// GetRandomRgn(hdc, region, which): SYSRGN (4) is the visible region,
/// on Windows 9x in the DC's device coordinates; 1 the clip region.
pub fn get_random_rgn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(dc) = get(cpu, args[0]) else {
        return Some(-1i32 as u32);
    };
    let region = match args[2] {
        1 => dc.attrs.clip.clone(),
        2 => dc.attrs.meta.clone(),
        3 => match (dc.attrs.meta.clone(), dc.attrs.clip.clone()) {
            (Some(m), Some(c)) => Some(m.and(&c)),
            (m, c) => m.or(c),
        },
        4 => {
            let (vis, _) = visible(cpu, &dc);
            Some(vis)
        }
        _ => return Some(-1i32 as u32),
    };
    let Some(region) = region else {
        return Some(0);
    };
    if !super::set_region(cpu, args[1], region) {
        return Some(-1i32 as u32);
    }
    Some(1)
}

/// PtVisible(hdc, x, y).
pub fn pt_visible(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(dc) = get(cpu, args[0]) else {
        return Some(-1i32 as u32);
    };
    let (x, y) = dc.to_device(args[1] as i32, args[2] as i32);
    let (region, _) = effective_clip(cpu, &dc);
    Some(region.contains(x, y) as u32)
}

/// RectVisible(hdc, rect).
pub fn rect_visible(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(dc) = get(cpu, args[0]) else {
        return Some(FALSE);
    };
    let r = Rect::read(cpu, args[1]);
    let r = dc.rect_to_device(&r);
    let (region, _) = effective_clip(cpu, &dc);
    Some(region.overlaps(&r) as u32)
}

/// SetBoundsRect(hdc, rect, flags): the state it had.
pub fn set_bounds_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, at, flags) = (args[0], args[1], args[2]);
    let Some(dc) = get(cpu, hdc) else {
        return Some(0);
    };
    let mut ret = if dc.bounds_on { 4 } else { 8 };
    if dc.bounds.is_empty() {
        ret |= 1;
    } else {
        ret |= 2;
    }
    let r = if at != 0 && flags & 2 != 0 {
        Some(dc.rect_to_device(&Rect::read(cpu, at)))
    } else {
        None
    };
    with(cpu, hdc, |d| {
        if flags & 1 != 0 {
            d.bounds = Rect::default();
        }
        if let Some(r) = r {
            d.bounds = d.bounds.union(&r);
        }
        if flags & 4 != 0 {
            d.bounds_on = true;
        }
        if flags & 8 != 0 {
            d.bounds_on = false;
        }
    });
    Some(ret)
}

/// GetBoundsRect(hdc, rect, flags): DCB_RESET if the bounds are empty,
/// DCB_SET if not.
pub fn get_bounds_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, at, flags) = (args[0], args[1], args[2]);
    let Some(dc) = get(cpu, hdc) else {
        return Some(0);
    };
    let b = dc.bounds;
    let ret = if b.is_empty() { 1 } else { 3 };
    if at != 0 {
        if b.is_empty() {
            Rect::default().write(cpu, at);
        } else {
            let (l, t) = dc.to_logical(b.left, b.top);
            let (r, bt) = dc.to_logical(b.right, b.bottom);
            Rect::new(l, t, r, bt).ordered().write(cpu, at);
        }
    }
    if flags & 1 != 0 {
        with(cpu, hdc, |d| d.bounds = Rect::default());
    }
    Some(ret)
}

/// The window and viewport extents a mapping mode sets, as Windows 95
/// has them: the viewport is an inch of LOGPIXELSX by LOGPIXELSY, the
/// window that inch in the mode's units, with y going up.
fn extents_of(mode: u32) -> Option<((i32, i32), (i32, i32))> {
    let dpi = (device_cap(88) as i32, device_cap(90) as i32);
    let units = match mode {
        MM_TEXT => return Some(((1, 1), (1, 1))),
        MM_LOMETRIC | MM_ISOTROPIC => 254,
        MM_HIMETRIC => 2540,
        MM_LOENGLISH => 100,
        MM_HIENGLISH => 1000,
        MM_TWIPS => 1440,
        _ => return None,
    };
    Some(((units, -units), dpi))
}

/// SetMapMode(hdc, mode): the old mode.
pub fn set_map_mode(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mode = args[1];
    if !(MM_TEXT..=MM_ANISOTROPIC).contains(&mode) {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    Some(
        with(cpu, args[0], |d| {
            let old = d.attrs.map_mode;
            if let Some((w, v)) = extents_of(mode) {
                d.attrs.window_ext = w;
                d.attrs.viewport_ext = v;
            }
            d.attrs.map_mode = mode;
            old
        })
        .unwrap_or(0),
    )
}

pub fn get_map_mode(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_attr(cpu, args[0], 0, |a| a.map_mode)
}

/// Write a point to `at`, if it isn't 0.
fn write_point(cpu: &mut Cpu, at: u32, p: (i32, i32)) {
    if at != 0 {
        write_words(cpu, at, &[p.0 as u32, p.1 as u32]);
    }
}

/// Change a point of the DC's mapping with `f`, writing the old one to
/// `out`.
fn change_point(
    cpu: &mut Cpu,
    hdc: u32,
    out: u32,
    f: impl FnOnce(&mut Attrs) -> (i32, i32),
) -> Option<u32> {
    let Some(old) = with(cpu, hdc, |d| f(&mut d.attrs)) else {
        return Some(FALSE);
    };
    write_point(cpu, out, old);
    Some(TRUE)
}

pub fn set_window_org_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let p = (args[1] as i32, args[2] as i32);
    change_point(cpu, args[0], args[3], |a| {
        std::mem::replace(&mut a.window_org, p)
    })
}

pub fn set_viewport_org_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let p = (args[1] as i32, args[2] as i32);
    change_point(cpu, args[0], args[3], |a| {
        std::mem::replace(&mut a.viewport_org, p)
    })
}

pub fn offset_window_org_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (dx, dy) = (args[1] as i32, args[2] as i32);
    change_point(cpu, args[0], args[3], |a| {
        let old = a.window_org;
        a.window_org = (old.0 + dx, old.1 + dy);
        old
    })
}

pub fn offset_viewport_org_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (dx, dy) = (args[1] as i32, args[2] as i32);
    change_point(cpu, args[0], args[3], |a| {
        let old = a.viewport_org;
        a.viewport_org = (old.0 + dx, old.1 + dy);
        old
    })
}

fn get_point(
    cpu: &mut Cpu,
    hdc: u32,
    out: u32,
    f: impl FnOnce(&Attrs) -> (i32, i32),
) -> Option<u32> {
    let Some(dc) = get(cpu, hdc) else {
        return Some(FALSE);
    };
    write_point(cpu, out, f(&dc.attrs));
    Some(TRUE)
}

pub fn get_window_org_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_point(cpu, args[0], args[1], |a| a.window_org)
}

pub fn get_viewport_org_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_point(cpu, args[0], args[1], |a| a.viewport_org)
}

pub fn get_window_ext_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_point(cpu, args[0], args[1], |a| a.window_ext)
}

pub fn get_viewport_ext_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_point(cpu, args[0], args[1], |a| a.viewport_ext)
}

/// MM_ISOTROPIC keeps a unit as long along x as along y: the viewport's
/// extent shrinks along the axis where it is longer.
fn fix_isotropic(a: &mut Attrs) {
    if a.map_mode != MM_ISOTROPIC || a.window_ext.0 == 0 || a.window_ext.1 == 0 {
        return;
    }
    let hs = device_cap(4) as f64;
    let vs = device_cap(6) as f64;
    let hr = device_cap(8) as f64;
    let vr = device_cap(10) as f64;
    let xdim = (a.viewport_ext.0 as f64 * hs / (hr * a.window_ext.0 as f64)).abs();
    let ydim = (a.viewport_ext.1 as f64 * vs / (vr * a.window_ext.1 as f64)).abs();
    if xdim > ydim {
        let min = if a.viewport_ext.0 >= 0 { 1 } else { -1 };
        a.viewport_ext.0 = (a.viewport_ext.0 as f64 * ydim / xdim + 0.5).floor() as i32;
        if a.viewport_ext.0 == 0 {
            a.viewport_ext.0 = min;
        }
    } else {
        let min = if a.viewport_ext.1 >= 0 { 1 } else { -1 };
        a.viewport_ext.1 = (a.viewport_ext.1 as f64 * xdim / ydim + 0.5).floor() as i32;
        if a.viewport_ext.1 == 0 {
            a.viewport_ext.1 = min;
        }
    }
}

/// Set an extent: only MM_ISOTROPIC and MM_ANISOTROPIC change it, and
/// never to 0.
fn set_extent(cpu: &mut Cpu, args: &[u32], window: bool) -> Option<u32> {
    let (x, y) = (args[1] as i32, args[2] as i32);
    let Some(dc) = get(cpu, args[0]) else {
        return Some(FALSE);
    };
    let old = if window {
        dc.attrs.window_ext
    } else {
        dc.attrs.viewport_ext
    };
    write_point(cpu, args[3], old);
    let mode = dc.attrs.map_mode;
    if mode != MM_ISOTROPIC && mode != MM_ANISOTROPIC {
        return Some(TRUE);
    }
    if x == 0 || y == 0 {
        return Some(FALSE);
    }
    with(cpu, args[0], |d| {
        if window {
            d.attrs.window_ext = (x, y);
        } else {
            d.attrs.viewport_ext = (x, y);
        }
        fix_isotropic(&mut d.attrs);
    });
    Some(TRUE)
}

pub fn set_window_ext_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    set_extent(cpu, args, true)
}

pub fn set_viewport_ext_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    set_extent(cpu, args, false)
}

/// ScaleWindowExtEx(hdc, xnum, xdenom, ynum, ydenom, old) and the
/// viewport's.
fn scale_extent(cpu: &mut Cpu, args: &[u32], window: bool) -> Option<u32> {
    let [xn, xd, yn, yd] = [args[1], args[2], args[3], args[4]].map(|v| v as i32);
    let Some(dc) = get(cpu, args[0]) else {
        return Some(FALSE);
    };
    let old = if window {
        dc.attrs.window_ext
    } else {
        dc.attrs.viewport_ext
    };
    write_point(cpu, args[5], old);
    let mode = dc.attrs.map_mode;
    if mode != MM_ISOTROPIC && mode != MM_ANISOTROPIC {
        return Some(TRUE);
    }
    if xd == 0 || yd == 0 {
        return Some(FALSE);
    }
    let new = (
        (old.0 as i64 * xn as i64 / xd as i64) as i32,
        (old.1 as i64 * yn as i64 / yd as i64) as i32,
    );
    if new.0 == 0 || new.1 == 0 {
        return Some(FALSE);
    }
    with(cpu, args[0], |d| {
        if window {
            d.attrs.window_ext = new;
        } else {
            d.attrs.viewport_ext = new;
        }
        fix_isotropic(&mut d.attrs);
    });
    Some(TRUE)
}

pub fn scale_window_ext_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    scale_extent(cpu, args, true)
}

pub fn scale_viewport_ext_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    scale_extent(cpu, args, false)
}

/// LPtoDP(hdc, points, count) and DPtoLP.
fn convert_points(cpu: &mut Cpu, args: &[u32], to_device: bool) -> Option<u32> {
    let (hdc, at, count) = (args[0], args[1], args[2] as i32);
    let Some(dc) = get(cpu, hdc) else {
        return Some(FALSE);
    };
    if count < 0 {
        return Some(FALSE);
    }
    for i in 0..count as u32 {
        let w = read_words(cpu, at + 8 * i, 2);
        let (x, y) = (w[0] as i32, w[1] as i32);
        let (x, y) = if to_device {
            dc.to_device(x, y)
        } else {
            if dc.attrs.viewport_ext.0 == 0 || dc.attrs.viewport_ext.1 == 0 {
                return Some(FALSE);
            }
            dc.to_logical(x, y)
        };
        write_words(cpu, at + 8 * i, &[x as u32, y as u32]);
    }
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(TRUE)
}

pub fn lp_to_dp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    convert_points(cpu, args, true)
}

pub fn dp_to_lp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    convert_points(cpu, args, false)
}

/// The kinds of regions, for what clip functions return.
pub const _KINDS: [u32; 3] = [NULLREGION, SIMPLEREGION, COMPLEXREGION];

crate::state_fields!(Attrs {
    pen,
    brush,
    font,
    bitmap,
    palette,
    text_color,
    bk_color,
    bk_mode,
    rop2,
    poly_fill,
    stretch,
    text_align,
    map_mode,
    window_org,
    window_ext,
    viewport_org,
    viewport_ext,
    brush_org,
    pos,
    clip,
    meta,
    char_extra,
    break_extra,
    break_count,
    arc_dir,
    mapper,
    graphics_mode
});
crate::state_fields!(Dc {
    kind,
    hwnd,
    flags,
    extra,
    extra_rgn,
    own,
    attrs,
    saved,
    bounds,
    bounds_on,
    path,
    path_state
});
