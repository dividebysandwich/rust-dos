//! GDI32.DLL: device contexts and what they draw with (pens, brushes,
//! fonts, bitmaps, palettes, regions), and the drawing itself, on the
//! screen (the mode USER sets for the desktop), on bitmaps in GDI's
//! memory and on DIB sections in the program's.
//!
//! GDI's objects are the system's, as on Windows 95, in `State`, in
//! `Win32::gdi`; their handles are small numbers, as GDI's 16-bit heap
//! made them. Compatible bitmaps have the desktop's depth: 32 bits a
//! pixel, or 16 (5-6-5) on a Banshee or Voodoo3. Like Windows 95's GDI
//! it has no world transforms (GM_ADVANCED), and of the Unicode (W)
//! functions only those Windows 95 has.

pub mod bitmap;
pub mod dc;
pub mod draw;
pub mod font;
pub mod metafile;
pub mod palette;
pub mod region;
pub mod rgn;
pub mod surface;

pub use region::{Rect, Region};

use super::stubs::{Api, Conv, Dll};
use crate::cpu::Cpu;
use crate::win32::task::{Next, Task};

pub static DLL: Dll = Dll {
    name: "GDI32.DLL",
    base: 0xBFF3_0000,
    apis: APIS,
    vars: &[],
    init: None,
    vtables: &[],
};

const fn stdcall(
    name: &'static str,
    ordinal: u16,
    args: u8,
    handler: super::stubs::Handler,
) -> Api {
    Api {
        name,
        ordinal,
        args,
        conv: Conv::Stdcall,
        handler,
    }
}

pub const TRUE: u32 = 1;
pub const FALSE: u32 = 0;
/// What functions that return a value of their own return when they
/// fail (GetPixel's CLR_INVALID, GetTextAlign's GDI_ERROR).
pub const CLR_INVALID: u32 = 0xFFFF_FFFF;
pub const GDI_ERROR: u32 = 0xFFFF_FFFF;

pub const ERROR_INVALID_HANDLE: u32 = 6;
pub const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
pub const ERROR_INVALID_PARAMETER: u32 = 87;
pub const ERROR_CALL_NOT_IMPLEMENTED: u32 = 120;

/// GetObjectType's answers.
pub const OBJ_PEN: u32 = 1;
pub const OBJ_BRUSH: u32 = 2;
pub const OBJ_DC: u32 = 3;
pub const OBJ_METADC: u32 = 4;
pub const OBJ_PAL: u32 = 5;
pub const OBJ_FONT: u32 = 6;
pub const OBJ_BITMAP: u32 = 7;
pub const OBJ_REGION: u32 = 8;
pub const OBJ_METAFILE: u32 = 9;
pub const OBJ_MEMDC: u32 = 10;
pub const OBJ_EXTPEN: u32 = 11;
pub const OBJ_COLORSPACE: u32 = 14;

/// Pen styles.
pub const PS_SOLID: u32 = 0;
pub const PS_DASH: u32 = 1;
pub const PS_DOT: u32 = 2;
pub const PS_DASHDOT: u32 = 3;
pub const PS_DASHDOTDOT: u32 = 4;
pub const PS_NULL: u32 = 5;
pub const PS_INSIDEFRAME: u32 = 6;
pub const PS_USERSTYLE: u32 = 7;
pub const PS_ALTERNATE: u32 = 8;
pub const PS_STYLE_MASK: u32 = 0xF;
pub const PS_GEOMETRIC: u32 = 0x0001_0000;
pub const PS_TYPE_MASK: u32 = 0x000F_0000;

/// Brush styles.
pub const BS_SOLID: u32 = 0;
pub const BS_NULL: u32 = 1;
pub const BS_HATCHED: u32 = 2;
pub const BS_PATTERN: u32 = 3;
pub const BS_DIBPATTERN: u32 = 5;
pub const BS_DIBPATTERNPT: u32 = 6;

/// A pen.
#[derive(Clone, Debug, Default)]
pub struct Pen {
    pub style: u32,
    pub width: i32,
    pub color: u32,
    /// Made by ExtCreatePen, with the brush style and hatch it was given.
    pub ext: bool,
    pub brush_style: u32,
    pub hatch: u32,
    /// PS_USERSTYLE's dashes.
    pub dashes: Vec<u32>,
}

/// A brush's pattern: its pixels as colors, or for a monochrome one as
/// 0 and 1, which the DC's text and background colors draw.
#[derive(Clone, Debug, Default)]
pub struct Pattern {
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u32>,
    pub mono: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Brush {
    pub style: u32,
    pub color: u32,
    /// The hatch style, or for a pattern brush what GetObject gives as
    /// lbHatch: the bitmap or the DIB's memory handle.
    pub hatch: u32,
    pub pattern: Option<Pattern>,
    /// A DIB pattern's color usage (DIB_RGB_COLORS, DIB_PAL_COLORS).
    pub usage: u32,
}

/// A font: its LOGFONTA, 60 bytes.
#[derive(Clone, Debug, Default)]
pub struct Font {
    pub log: Vec<u8>,
}

/// A DIB section's own part: where its bits are in the program's memory
/// and how they are laid out.
#[derive(Clone, Debug, Default)]
pub struct Dib {
    pub bits: u32,
    /// Its BITMAPINFOHEADER as it was made.
    pub header: Vec<u8>,
    pub colors: Vec<u32>,
    /// BI_BITFIELDS' masks, red, green, blue.
    pub masks: [u32; 3],
    pub top_down: bool,
    pub compression: u32,
    /// The file mapping its bits are in and their offset, if one was
    /// given.
    pub section: u32,
    pub offset: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Bitmap {
    pub width: i32,
    pub height: i32,
    pub bpp: u16,
    pub planes: u16,
    /// Bytes a row: a word-aligned row for a bitmap of GDI's, a
    /// doubleword-aligned one for a DIB section.
    pub stride: u32,
    /// A bitmap of GDI's: its rows, the top first.
    pub bits: Vec<u8>,
    pub dib: Option<Dib>,
    /// The DC it is selected into, or 0.
    pub dc: u32,
    /// SetBitmapDimensionEx's size.
    pub dimension: (i32, i32),
}

#[derive(Clone, Debug, Default)]
pub struct Palette {
    /// The entries: red, green, blue and flags.
    pub entries: Vec<[u8; 4]>,
    /// On a desktop of 256 colours, the system palette's index of each
    /// entry, from when it was last realized; empty before.
    pub mapping: Vec<u8>,
}

/// What an object is.
#[derive(Clone, Debug)]
pub enum Kind {
    Pen { pen: Pen },
    Brush { brush: Brush },
    Font { font: Font },
    Bitmap { bitmap: Bitmap },
    Palette { palette: Palette },
    Region { region: Region },
    Dc { dc: dc::Dc },
    ColorSpace { data: Vec<u8> },
    /// A Windows metafile's records, header first (metafile.rs).
    MetaFile { data: Vec<u8> },
}

impl Default for Kind {
    fn default() -> Kind {
        Kind::Region {
            region: Region::default(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Object {
    pub kind: Kind,
    /// A stock object, which DeleteObject leaves.
    pub stock: bool,
    /// The process that made it.
    pub pid: u32,
    /// DeleteObject was called while it was selected into a DC: it goes
    /// when it is deselected.
    pub deleted: bool,
}

impl Object {
    pub fn type_of(&self) -> u32 {
        match &self.kind {
            Kind::Pen { pen } if pen.ext => OBJ_EXTPEN,
            Kind::Pen { .. } => OBJ_PEN,
            Kind::Brush { .. } => OBJ_BRUSH,
            Kind::Font { .. } => OBJ_FONT,
            Kind::Bitmap { .. } => OBJ_BITMAP,
            Kind::Palette { .. } => OBJ_PAL,
            Kind::Region { .. } => OBJ_REGION,
            Kind::Dc { dc } if dc.kind == dc::MEMORY => OBJ_MEMDC,
            Kind::Dc { .. } => OBJ_DC,
            Kind::ColorSpace { .. } => OBJ_COLORSPACE,
            Kind::MetaFile { .. } => OBJ_METAFILE,
        }
    }
}

/// Where object handles start, and how far apart they are.
const HANDLE_BASE: u32 = 0x0080;
const HANDLE_STEP: u32 = 4;
const MAX_OBJECTS: usize = ((0x1_0000 - HANDLE_BASE) / HANDLE_STEP) as usize;

/// GDI's state: the system's.
#[derive(Debug, Default)]
pub struct State {
    pub objects: Vec<Option<Object>>,
    /// Where the search for a free place starts.
    pub next: usize,
    /// The system palette: 256 entries, the 20 static colors at its ends.
    pub system_palette: Vec<[u8; 4]>,
    /// What each entry of the system palette is (palette.rs): free,
    /// static, a logical palette's or a PC_RESERVED one's.
    pub system_use: Vec<u8>,
    /// The logical palette whose realization took each entry of the
    /// system palette, 0 for a static or free entry and for one whose
    /// palette was deleted.
    pub system_owner: Vec<u32>,
    /// SetSystemPaletteUse's setting: SYSPAL_STATIC or SYSPAL_NOSTATIC.
    pub static_use: u32,
    /// The palette realized in the foreground (USER's primary palette),
    /// and the one realized last.
    pub primary_palette: u32,
    pub last_realized: u32,
    /// The fonts AddFontResourceA added, by file name.
    pub font_files: Vec<String>,
}

/// The stock objects, by GetStockObject's index; the default bitmap
/// after them.
pub const WHITE_BRUSH: u32 = 0;
pub const LTGRAY_BRUSH: u32 = 1;
pub const GRAY_BRUSH: u32 = 2;
pub const DKGRAY_BRUSH: u32 = 3;
pub const BLACK_BRUSH: u32 = 4;
pub const NULL_BRUSH: u32 = 5;
pub const WHITE_PEN: u32 = 6;
pub const BLACK_PEN: u32 = 7;
pub const NULL_PEN: u32 = 8;
pub const OEM_FIXED_FONT: u32 = 10;
pub const ANSI_FIXED_FONT: u32 = 11;
pub const ANSI_VAR_FONT: u32 = 12;
pub const SYSTEM_FONT: u32 = 13;
pub const DEVICE_DEFAULT_FONT: u32 = 14;
pub const DEFAULT_PALETTE: u32 = 15;
pub const SYSTEM_FIXED_FONT: u32 = 16;
pub const DEFAULT_GUI_FONT: u32 = 17;
pub const DEFAULT_BITMAP: u32 = 20;
const STOCK_COUNT: usize = 21;

impl State {
    pub fn get(&self, handle: u32) -> Option<&Object> {
        self.objects.get(slot(handle)?)?.as_ref()
    }

    pub fn get_mut(&mut self, handle: u32) -> Option<&mut Object> {
        self.objects.get_mut(slot(handle)?)?.as_mut()
    }

    /// A new object's handle, or None when there are too many.
    pub fn add(&mut self, object: Object) -> Option<u32> {
        let len = self.objects.len();
        let free = (0..len)
            .map(|i| (self.next + i) % len.max(1))
            .find(|&i| i >= STOCK_COUNT && self.objects[i].is_none());
        let i = match free {
            Some(i) if len >= 256 => i,
            _ if len < MAX_OBJECTS => {
                self.objects.push(None);
                len
            }
            Some(i) => i,
            None => return None,
        };
        self.next = i + 1;
        self.objects[i] = Some(object);
        Some(handle(i))
    }

    pub fn stock(&self, index: u32) -> u32 {
        handle(index as usize)
    }

    pub fn pen(&self, handle: u32) -> Option<&Pen> {
        match &self.get(handle)?.kind {
            Kind::Pen { pen } => Some(pen),
            _ => None,
        }
    }

    pub fn brush(&self, handle: u32) -> Option<&Brush> {
        match &self.get(handle)?.kind {
            Kind::Brush { brush } => Some(brush),
            _ => None,
        }
    }

    pub fn font(&self, handle: u32) -> Option<&Font> {
        match &self.get(handle)?.kind {
            Kind::Font { font } => Some(font),
            _ => None,
        }
    }

    pub fn bitmap(&self, handle: u32) -> Option<&Bitmap> {
        match &self.get(handle)?.kind {
            Kind::Bitmap { bitmap } => Some(bitmap),
            _ => None,
        }
    }

    pub fn bitmap_mut(&mut self, handle: u32) -> Option<&mut Bitmap> {
        match &mut self.get_mut(handle)?.kind {
            Kind::Bitmap { bitmap } => Some(bitmap),
            _ => None,
        }
    }

    pub fn palette(&self, handle: u32) -> Option<&Palette> {
        match &self.get(handle)?.kind {
            Kind::Palette { palette } => Some(palette),
            _ => None,
        }
    }

    pub fn region(&self, handle: u32) -> Option<&Region> {
        match &self.get(handle)?.kind {
            Kind::Region { region } => Some(region),
            _ => None,
        }
    }

    pub fn region_mut(&mut self, handle: u32) -> Option<&mut Region> {
        match &mut self.get_mut(handle)?.kind {
            Kind::Region { region } => Some(region),
            _ => None,
        }
    }

    pub fn dc(&self, handle: u32) -> Option<&dc::Dc> {
        match &self.get(handle)?.kind {
            Kind::Dc { dc } => Some(dc),
            _ => None,
        }
    }

    pub fn dc_mut(&mut self, handle: u32) -> Option<&mut dc::Dc> {
        match &mut self.get_mut(handle)?.kind {
            Kind::Dc { dc } => Some(dc),
            _ => None,
        }
    }
}

fn slot(handle: u32) -> Option<usize> {
    let offset = handle.checked_sub(HANDLE_BASE)?;
    (offset % HANDLE_STEP == 0 && handle < 0x1_0000).then_some((offset / HANDLE_STEP) as usize)
}

fn handle(slot: usize) -> u32 {
    HANDLE_BASE + HANDLE_STEP * slot as u32
}

/// GDI's state.
pub fn gdi(cpu: &mut Cpu) -> &mut State {
    &mut cpu.bus.win32.gdi
}

const ERROR_NOACCESS: u32 = 998;

pub fn set_error(cpu: &mut Cpu, code: u32) {
    super::process(cpu).last_error = code;
}

fn current_pid(cpu: &Cpu) -> u32 {
    super::running(cpu).map_or(0, |p| p.id)
}

/// Add `kind` as an object of the running process: its handle, or 0
/// with ERROR_NOT_ENOUGH_MEMORY when GDI's table is full.
pub fn add(cpu: &mut Cpu, kind: Kind) -> u32 {
    init(cpu);
    let pid = current_pid(cpu);
    let object = Object {
        kind,
        stock: false,
        pid,
        deleted: false,
    };
    match gdi(cpu).add(object) {
        Some(h) => h,
        None => {
            set_error(cpu, ERROR_NOT_ENOUGH_MEMORY);
            0
        }
    }
}

/// Add `kind` as an object of the system's, which DeleteObject leaves
/// (USER's brushes of the system colors).
pub fn add_stock(cpu: &mut Cpu, kind: Kind) -> u32 {
    init(cpu);
    let object = Object {
        kind,
        stock: true,
        pid: 0,
        deleted: false,
    };
    gdi(cpu).add(object).unwrap_or(0)
}

/// The colors of the stock brushes and the static colors of the system
/// palette, as COLORREFs.
const STATIC_COLORS: [u32; 20] = [
    0x000000, 0x000080, 0x008000, 0x008080, 0x800000, 0x800080, 0x808000, 0xC0C0C0, 0xC0DCC0,
    0xF0CAA6, 0xF0FBFF, 0xA4A0A0, 0x808080, 0x0000FF, 0x00FF00, 0x00FFFF, 0xFF0000, 0xFF00FF,
    0xFFFF00, 0xFFFFFF,
];

/// The stock objects and the system palette, made on GDI's first use.
pub fn init(cpu: &mut Cpu) {
    let state = gdi(cpu);
    if !state.objects.is_empty() {
        return;
    }
    let solid = |color: u32| Kind::Brush {
        brush: Brush {
            style: BS_SOLID,
            color,
            ..Brush::default()
        },
    };
    let pen = |style: u32, color: u32| Kind::Pen {
        pen: Pen {
            style,
            width: 0,
            color,
            ..Pen::default()
        },
    };
    let font =
        |height: i32, width: i32, weight: i32, charset: u8, pitch: u8, face: &str| Kind::Font {
            font: Font {
                log: font::logfont(height, width, weight, charset, pitch, face),
            },
        };
    let mut stock: Vec<Option<Kind>> = vec![
        Some(solid(0xFFFFFF)),
        Some(solid(0xC0C0C0)),
        Some(solid(0x808080)),
        Some(solid(0x404040)),
        Some(solid(0x000000)),
        Some(Kind::Brush {
            brush: Brush {
                style: BS_NULL,
                ..Brush::default()
            },
        }),
        Some(pen(PS_SOLID, 0xFFFFFF)),
        Some(pen(PS_SOLID, 0x000000)),
        // Windows's null pen is 1 wide.
        Some(Kind::Pen {
            pen: Pen {
                style: PS_NULL,
                width: 1,
                ..Pen::default()
            },
        }),
        None,
        Some(font(12, 8, 400, 255, 0x31, "Terminal")),
        Some(font(12, 9, 400, 0, 0x31, "Courier")),
        Some(font(12, 9, 400, 0, 0x22, "MS Sans Serif")),
        Some(font(16, 7, 700, 0, 0x22, "System")),
        Some(font(16, 0, 400, 0, 0x00, "")),
        None,
        Some(font(16, 8, 400, 0, 0x31, "Fixedsys")),
        Some(font(-11, 0, 400, 0, 0x00, "MS Sans Serif")),
        None,
        None,
        Some(Kind::Bitmap {
            bitmap: Bitmap {
                width: 1,
                height: 1,
                bpp: 1,
                planes: 1,
                stride: 2,
                bits: vec![0; 2],
                ..Bitmap::default()
            },
        }),
    ];
    stock[DEFAULT_PALETTE as usize] = Some(Kind::Palette {
        palette: Palette {
            entries: STATIC_COLORS
                .iter()
                .map(|&c| [c as u8, (c >> 8) as u8, (c >> 16) as u8, 0])
                .collect(),
            mapping: (0..10).chain(246..=255).collect(),
        },
    });
    let state = gdi(cpu);
    state.objects = stock
        .into_iter()
        .map(|k| {
            k.map(|kind| Object {
                kind,
                stock: true,
                pid: 0,
                deleted: false,
            })
        })
        .collect();
    state.next = STOCK_COUNT;
    // The system palette: the first ten static colors, black for the
    // rest, the last ten static colors.
    let mut system = vec![[0u8; 4]; 256];
    for (i, &c) in STATIC_COLORS.iter().enumerate() {
        let at = if i < 10 { i } else { 236 + i };
        system[at] = [c as u8, (c >> 8) as u8, (c >> 16) as u8, 0];
    }
    state.system_palette = system;
    state.static_use = palette::SYSPAL_STATIC;
    state.system_use = palette::static_uses(palette::SYSPAL_STATIC);
    state.system_owner = vec![0; 256];
}

/// The static colors of the system palette as COLORREFs, for the
/// default palette and the halftone one.
pub fn static_colors() -> &'static [u32; 20] {
    &STATIC_COLORS
}

/// The region of the region object `handle`, if it is one.
pub fn region_of(cpu: &mut Cpu, handle: u32) -> Option<Region> {
    gdi(cpu).region(handle).cloned()
}

/// Make the region object `handle` hold `region`: false if it isn't a
/// region.
pub fn set_region(cpu: &mut Cpu, handle: u32, region: Region) -> bool {
    match gdi(cpu).region_mut(handle) {
        Some(r) => {
            *r = region;
            true
        }
        None => false,
    }
}

/// Delete the object `handle` whatever holds it (a region SetWindowRgn
/// took).
pub fn delete_object_now(cpu: &mut Cpu, handle: u32) {
    if let Some(i) = slot(handle)
        && let Some(o) = gdi(cpu).objects.get_mut(i)
        && o.as_ref().is_some_and(|o| !o.stock)
    {
        *o = None;
    }
}

/// GetStockObject(index).
pub fn get_stock_object(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    init(cpu);
    let index = args[0];
    if index as usize >= STOCK_COUNT || index == DEFAULT_BITMAP {
        return Some(0);
    }
    let state = gdi(cpu);
    Some(if state.objects[index as usize].is_some() {
        state.stock(index)
    } else {
        0
    })
}

/// GetObjectType(handle).
pub fn get_object_type(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    init(cpu);
    match gdi(cpu).get(args[0]) {
        Some(o) => Some(o.type_of()),
        None => {
            set_error(cpu, ERROR_INVALID_HANDLE);
            Some(0)
        }
    }
}

/// DeleteObject(handle): an object selected into a DC goes when it is
/// deselected; stock objects stay.
pub fn delete_object(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let handle = args[0];
    let Some(object) = gdi(cpu).get(handle) else {
        if handle != 0 {
            set_error(cpu, ERROR_INVALID_HANDLE);
        }
        return Some(FALSE);
    };
    if object.stock {
        return Some(TRUE);
    }
    if let Kind::Dc { .. } = object.kind {
        // DeleteObject doesn't delete DCs.
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    }
    if dc::selected_anywhere(gdi(cpu), handle) {
        if let Some(o) = gdi(cpu).get_mut(handle) {
            o.deleted = true;
        }
        return Some(TRUE);
    }
    free(cpu, handle);
    Some(TRUE)
}

/// Free the object `handle` and what it holds: a DIB section's memory.
pub fn free(cpu: &mut Cpu, handle: u32) {
    let Some(i) = slot(handle) else {
        return;
    };
    let Some(Some(object)) = gdi(cpu).objects.get_mut(i).map(|o| o.take()) else {
        return;
    };
    if let Kind::Palette { .. } = object.kind {
        palette::palette_deleted(cpu, handle);
    }
    if let Kind::Bitmap { bitmap } = object.kind
        && let Some(dib) = bitmap.dib
        && dib.bits != 0
        && object.pid == current_pid(cpu)
    {
        bitmap::free_dib_bits(cpu, dib.bits, dib.section, dib.offset);
    }
}

/// The objects of the process `pid`, which ended, go.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    for o in gdi(cpu).objects.iter_mut() {
        if o.as_ref().is_some_and(|o| o.pid == pid && !o.stock) {
            *o = None;
        }
    }
}

/// Write `bytes` to the buffer at `at` of `size` bytes, as GetObjectA
/// does: as much as fits. How many it wrote.
fn write_object(cpu: &mut Cpu, at: u32, size: u32, bytes: &[u8]) -> u32 {
    let n = bytes.len().min(size as usize);
    if !super::kernel32::virtual_memory::allows(cpu, at, n as u32, true) {
        set_error(cpu, ERROR_NOACCESS);
        return 0;
    }
    cpu.bus.guest_write_bytes(at, &bytes[..n]);
    n as u32
}

fn le(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// GetObjectA(handle, size, buffer): the object's LOG structure; with no
/// buffer, its size.
pub fn get_object(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_object_as(cpu, args, false)
}

/// GetObjectW: the same, but a font's LOGFONTW.
pub fn get_object_w(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_object_as(cpu, args, true)
}

fn get_object_as(cpu: &mut Cpu, args: &[u32], wide: bool) -> Option<u32> {
    let (handle, size, at) = (args[0], args[1], args[2]);
    init(cpu);
    let Some(object) = gdi(cpu).get(handle).cloned() else {
        if handle != 0 {
            set_error(cpu, ERROR_INVALID_HANDLE);
        }
        return Some(0);
    };
    let bytes: Vec<u8> = match &object.kind {
        Kind::Pen { pen } if pen.ext => {
            let mut b = le(&[
                pen.style,
                pen.width as u32,
                pen.brush_style,
                pen.color,
                pen.hatch,
            ]);
            b.extend(le(&[pen.dashes.len() as u32]));
            b.extend(le(&pen.dashes));
            // An extended pen comes whole or not at all.
            if at != 0 && (size as usize) < b.len() {
                return Some(0);
            }
            b
        }
        Kind::Pen { pen } => {
            if at != 0 && size < 16 {
                return Some(0);
            }
            if pen.style & PS_STYLE_MASK == PS_NULL && size == 28 {
                // An EXTLOGPEN, asked for with its size, comes as one.
                le(&[pen.style, 0, BS_SOLID, pen.color, 0, 0, 0])
            } else {
                le(&[pen.style, pen.width as u32, 0, pen.color])
            }
        }
        Kind::Brush { brush } => le(&[brush.style, brush.color, brush.hatch]),
        Kind::Font { font } => {
            if wide {
                let mut b = font.log[..28].to_vec();
                let face: Vec<u8> = font.log[28..60].iter().flat_map(|&c| [c, 0]).collect();
                b.extend(face);
                b
            } else {
                font.log.clone()
            }
        }
        Kind::Bitmap { bitmap } => {
            let mut b = bitmap::bitmap_struct(bitmap);
            if let Some(dib) = &bitmap.dib
                && (size as usize) >= 84
            {
                // DIBSECTION: the BITMAP, the header, the masks, the
                // section and its offset.
                let mut header = dib.header.clone();
                header.resize(40, 0);
                b.extend(header);
                b.extend(le(&dib.masks));
                b.extend(le(&[dib.section, dib.offset]));
            }
            b
        }
        Kind::Palette { palette } => (palette.entries.len() as u16).to_le_bytes().to_vec(),
        Kind::ColorSpace { data } => data.clone(),
        Kind::Region { .. } | Kind::Dc { .. } | Kind::MetaFile { .. } => {
            set_error(cpu, ERROR_INVALID_HANDLE);
            return Some(0);
        }
    };
    if at == 0 {
        return Some(bytes.len() as u32);
    }
    Some(write_object(cpu, at, size, &bytes))
}

/// GetCurrentObject(hdc, type).
pub fn get_current_object(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(dc) = gdi(cpu).dc(args[0]) else {
        if args[0] != 0 {
            set_error(cpu, ERROR_INVALID_HANDLE);
        }
        return Some(0);
    };
    let a = &dc.attrs;
    Some(match args[1] {
        OBJ_PEN | OBJ_EXTPEN => a.pen,
        OBJ_BRUSH => a.brush,
        OBJ_FONT => a.font,
        OBJ_BITMAP => a.bitmap,
        OBJ_PAL => a.palette,
        _ => 0,
    })
}

/// UnrealizeObject(handle): TRUE for a brush or palette.
pub fn unrealize_object(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match gdi(cpu).get(args[0]).map(Object::type_of) {
        Some(OBJ_PAL) => {
            palette::unrealize(cpu, args[0]);
            Some(TRUE)
        }
        Some(_) => Some(TRUE),
        None => {
            if args[0] != 0 {
                set_error(cpu, ERROR_INVALID_HANDLE);
            }
            Some(FALSE)
        }
    }
}

/// CreatePen(style, width, color).
pub fn create_pen(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let style = args[0];
    let style = if style & PS_STYLE_MASK >= PS_USERSTYLE {
        PS_SOLID
    } else {
        style & PS_STYLE_MASK
    };
    if style == PS_NULL {
        init(cpu);
        return Some(gdi(cpu).stock(NULL_PEN));
    }
    let width = args[1] as i32;
    Some(add(
        cpu,
        Kind::Pen {
            pen: Pen {
                style,
                width: width.abs(),
                color: args[2] & 0x03FF_FFFF,
                ..Pen::default()
            },
        },
    ))
}

/// CreatePenIndirect(logpen).
pub fn create_pen_indirect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let w = super::user32::read_words(cpu, args[0], 4);
    if cpu.bus.guest_faulted() {
        return None;
    }
    create_pen(cpu, &[w[0], w[1], w[3]])
}

/// ExtCreatePen(style, width, logbrush, count, styles).
pub fn ext_create_pen(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (style, width, brush, count, styles) = (args[0], args[1], args[2], args[3], args[4]);
    let b = super::user32::read_words(cpu, brush, 3);
    let kind = style & PS_TYPE_MASK;
    let line = style & PS_STYLE_MASK;
    if line == PS_USERSTYLE && count as i32 <= 0 {
        // No dashes fails without an error.
        return Some(0);
    }
    let dashes = if line == PS_USERSTYLE && styles != 0 {
        super::user32::read_words(cpu, styles, count.min(16) as usize)
    } else {
        Vec::new()
    };
    if cpu.bus.guest_faulted() {
        return None;
    }
    let dashes_wrong = if line == PS_USERSTYLE {
        // A geometric pen's dashes can't be all 0 or have one below 0.
        let odd = kind == PS_GEOMETRIC
            && (dashes.iter().all(|&d| d == 0) || dashes.iter().any(|&d| (d as i32) < 0));
        count > 16 || styles == 0 || odd
    } else {
        count != 0 || styles != 0
    };
    if dashes_wrong || kind > PS_GEOMETRIC || line > PS_ALTERNATE {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    if line == PS_NULL {
        return create_pen(cpu, &[PS_NULL, 0, b[1]]);
    }
    let bad = if kind == PS_GEOMETRIC {
        line == PS_ALTERNATE
    } else {
        width != 1 || b[0] != BS_SOLID || line == PS_INSIDEFRAME
    };
    if bad {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    Some(add(
        cpu,
        Kind::Pen {
            pen: Pen {
                style,
                width: (width as i32).abs(),
                color: b[1],
                ext: true,
                brush_style: b[0],
                hatch: b[2],
                dashes,
            },
        },
    ))
}

/// CreateSolidBrush(color).
pub fn create_solid_brush(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(add(
        cpu,
        Kind::Brush {
            brush: Brush {
                style: BS_SOLID,
                color: args[0],
                ..Brush::default()
            },
        },
    ))
}

/// CreateHatchBrush(hatch, color).
pub fn create_hatch_brush(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] > 5 {
        return create_solid_brush(cpu, &[args[1]]);
    }
    Some(add(
        cpu,
        Kind::Brush {
            brush: Brush {
                style: BS_HATCHED,
                color: args[1],
                hatch: args[0],
                pattern: Some(draw::hatch_pattern(args[0])),
                ..Brush::default()
            },
        },
    ))
}

/// CreatePatternBrush(bitmap): a copy of the bitmap's pixels.
pub fn create_pattern_brush(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(pattern) = bitmap::pattern_of(cpu, args[0]) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    };
    Some(add(
        cpu,
        Kind::Brush {
            brush: Brush {
                style: BS_PATTERN,
                color: 0,
                hatch: args[0],
                pattern: Some(pattern),
                ..Brush::default()
            },
        },
    ))
}

/// CreateDIBPatternBrushPt(packed DIB, usage).
pub fn create_dib_pattern_brush_pt(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    dib_pattern(cpu, args[0], args[0], args[1], BS_DIBPATTERNPT)
}

/// CreateDIBPatternBrush(global handle, usage).
pub fn create_dib_pattern_brush(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    // A global handle of a fixed block is its address; a moveable one's
    // memory is what GlobalLock gives, which the handle is close to.
    let at = crate::win32::kernel32::global_address(cpu, args[0]);
    dib_pattern(cpu, at, args[0], args[1], BS_DIBPATTERN)
}

fn dib_pattern(cpu: &mut Cpu, at: u32, handle: u32, usage: u32, style: u32) -> Option<u32> {
    if at == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let Some(pattern) = bitmap::pattern_of_packed_dib(cpu, at, usage) else {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    };
    Some(add(
        cpu,
        Kind::Brush {
            brush: Brush {
                style,
                color: usage,
                hatch: handle,
                pattern: Some(pattern),
                usage,
            },
        },
    ))
}

/// CreateBrushIndirect(logbrush).
pub fn create_brush_indirect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let w = super::user32::read_words(cpu, args[0], 3);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let (style, color, hatch) = (w[0], w[1], w[2]);
    match style {
        BS_SOLID => create_solid_brush(cpu, &[color]),
        BS_HATCHED => create_hatch_brush(cpu, &[hatch, color]),
        BS_PATTERN | 4 => create_pattern_brush(cpu, &[hatch]),
        BS_DIBPATTERN => create_dib_pattern_brush(cpu, &[hatch, color]),
        BS_DIBPATTERNPT => create_dib_pattern_brush_pt(cpu, &[hatch, color]),
        _ => Some(add(
            cpu,
            Kind::Brush {
                brush: Brush {
                    style: BS_NULL,
                    ..Brush::default()
                },
            },
        )),
    }
}

/// CreateColorSpaceA(logcolorspace): kept as given.
pub fn create_color_space(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut data = vec![0u8; 328];
    cpu.bus.guest_read_bytes(args[0], &mut data);
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(add(cpu, Kind::ColorSpace { data }))
}

/// GetLogColorSpaceA(colorspace, buffer, size).
pub fn get_log_color_space(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(Kind::ColorSpace { data }) = gdi(cpu).get(args[0]).map(|o| o.kind.clone()) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    };
    if args[2] < data.len() as u32 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    cpu.bus.guest_write_bytes(args[1], &data);
    Some(TRUE)
}

/// GetColorSpace(hdc): none is selected.
pub fn get_color_space(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if gdi(cpu).dc(args[0]).is_none() {
        set_error(cpu, ERROR_INVALID_HANDLE);
    }
    Some(0)
}

/// The variables of a callback's enumeration: its kind, the callback,
/// its argument, the next item's place, then the items.
const CB_KIND: usize = 0;
const CB_PROC: usize = 1;
const CB_PARAM: usize = 2;
const CB_AT: usize = 3;
const CB_LIST: usize = 4;

pub const CB_OBJECTS: u32 = 0;
pub const CB_FONTS: u32 = 1;
pub const CB_LINE_DDA: u32 = 2;
/// WM_PALETTECHANGED broadcast to the top-level windows after
/// RealizePalette: the variables are its result and the window.
pub const CB_PALETTE_CHANGED: u32 = 3;

pub const TASK_CALLBACK: u16 = 14;

/// The callbacks of GDI's enumerations (EnumObjects, EnumFontFamiliesA,
/// LineDDA): each item in turn until the callback returns 0. And
/// RealizePalette's WM_PALETTECHANGED to each top-level window.
pub fn callback_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    if t.step == 1 && value == 0 {
        return Next::Done(0);
    }
    let at = t.var(CB_AT) as usize;
    match t.var(CB_KIND) {
        CB_OBJECTS => {
            // Items: the LOGPEN or LOGBRUSH, 16 bytes each, which go to
            // the scratch space.
            let Some(item) = t.vars.get(CB_LIST + 4 * at..CB_LIST + 4 * at + 4) else {
                return Next::Done(value.max(1));
            };
            let item = item.to_vec();
            t.set(CB_AT, at as u32 + 1);
            super::user32::write_words(cpu, t.scratch, &item);
            Next::call(t.var(CB_PROC), &[t.scratch, t.var(CB_PARAM)], 1)
        }
        CB_FONTS => font::enum_callback(cpu, t, at),
        CB_PALETTE_CHANGED => {
            if at != 0 {
                return Next::Done(t.var(CB_PROC));
            }
            t.set(CB_AT, 1);
            t.step = 2;
            let send = super::user32::address_of("SendMessageA");
            let msg = super::user32::message::WM_PALETTECHANGED;
            let to = super::user32::message::HWND_BROADCAST;
            Next::call(send, &[to, msg, t.var(CB_PARAM), 0], 2)
        }
        CB_LINE_DDA => {
            let Some(&p) = t.vars.get(CB_LIST + at) else {
                return Next::Done(TRUE);
            };
            t.set(CB_AT, at as u32 + 1);
            let (x, y) = (p as i16 as i32, (p >> 16) as i16 as i32);
            // LineDDA's callback's result doesn't stop it.
            t.step = 2;
            Next::call(t.var(CB_PROC), &[x as u32, y as u32, t.var(CB_PARAM)], 2)
        }
        _ => Next::Done(0),
    }
}

/// EnumObjects(hdc, type, proc, param): the stock pens or the solid
/// brushes of the standard colors.
pub fn enum_objects(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, kind, proc, param) = (args[0], args[1], args[2], args[3]);
    if gdi(cpu).dc(hdc).is_none() {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    }
    let colors = &STATIC_COLORS[..];
    let mut vars = vec![CB_OBJECTS, proc, param, 0];
    match kind {
        OBJ_PEN => {
            for style in [PS_SOLID, PS_DASH, PS_DOT, PS_DASHDOT, PS_DASHDOTDOT] {
                for &c in colors {
                    vars.extend([style, 1, 0, c]);
                }
            }
        }
        OBJ_BRUSH => {
            for &c in colors {
                vars.extend([BS_SOLID, c, 0, 0]);
            }
            for hatch in 0..6 {
                for &c in colors {
                    vars.extend([BS_HATCHED, c, hatch, 0]);
                }
            }
        }
        _ => {
            set_error(cpu, ERROR_INVALID_PARAMETER);
            return Some(0);
        }
    }
    crate::win32::task::start(cpu, TASK_CALLBACK, vars, 16)
}

/// LineDDA(x1, y1, x2, y2, proc, param): the callback for each point of
/// the line but its last.
pub fn line_dda(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (x1, y1, x2, y2) = (
        args[0] as i32,
        args[1] as i32,
        args[2] as i32,
        args[3] as i32,
    );
    let mut vars = vec![CB_LINE_DDA, args[4], args[5], 0];
    let points = draw::line_points(x1, y1, x2, y2);
    for (x, y) in points.into_iter().take(0x4000) {
        vars.push(super::user32::make_long(x, y));
    }
    crate::win32::task::start(cpu, TASK_CALLBACK, vars, 0)
}

/// A function GDI32 of Windows 95 has only as a stub that fails.
pub fn not_implemented(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    set_error(cpu, ERROR_CALL_NOT_IMPLEMENTED);
    Some(FALSE)
}

fn nothing(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

fn succeed(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(TRUE)
}

/// GdiSetBatchLimit and GdiGetBatchLimit: GDI doesn't batch; 1.
fn batch_limit(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(1)
}

/// The functions. Append only (see `stubs`).
static APIS: &[Api] = &[
    // Objects.
    stdcall("GetStockObject", 1, 1, get_stock_object),
    stdcall("SelectObject", 2, 2, dc::select_object),
    stdcall("DeleteObject", 3, 1, delete_object),
    stdcall("GetObjectA", 4, 3, get_object),
    stdcall("GetObjectW", 5, 3, get_object_w),
    stdcall("GetObjectType", 6, 1, get_object_type),
    stdcall("GetCurrentObject", 7, 2, get_current_object),
    stdcall("UnrealizeObject", 8, 1, unrealize_object),
    stdcall("EnumObjects", 9, 4, enum_objects),
    stdcall("CreatePen", 10, 3, create_pen),
    stdcall("CreatePenIndirect", 11, 1, create_pen_indirect),
    stdcall("ExtCreatePen", 12, 5, ext_create_pen),
    stdcall("CreateSolidBrush", 13, 1, create_solid_brush),
    stdcall("CreateHatchBrush", 14, 2, create_hatch_brush),
    stdcall("CreatePatternBrush", 15, 1, create_pattern_brush),
    stdcall("CreateBrushIndirect", 16, 1, create_brush_indirect),
    stdcall("CreateDIBPatternBrush", 17, 2, create_dib_pattern_brush),
    stdcall(
        "CreateDIBPatternBrushPt",
        18,
        2,
        create_dib_pattern_brush_pt,
    ),
    stdcall("SetBrushOrgEx", 19, 4, dc::set_brush_org_ex),
    stdcall("GetBrushOrgEx", 20, 2, dc::get_brush_org_ex),
    // Device contexts.
    stdcall("CreateDCA", 21, 4, dc::create_dc),
    stdcall("CreateICA", 22, 4, dc::create_ic),
    stdcall("CreateCompatibleDC", 23, 1, dc::create_compatible_dc),
    stdcall("DeleteDC", 24, 1, dc::delete_dc),
    stdcall("SaveDC", 25, 1, dc::save_dc),
    stdcall("RestoreDC", 26, 2, dc::restore_dc),
    stdcall("GetDeviceCaps", 27, 2, dc::get_device_caps),
    stdcall("ResetDCA", 28, 2, dc::reset_dc),
    stdcall("GetDCOrgEx", 29, 2, dc::get_dc_org_ex),
    stdcall("SetBkColor", 30, 2, dc::set_bk_color),
    stdcall("GetBkColor", 31, 1, dc::get_bk_color),
    stdcall("SetBkMode", 32, 2, dc::set_bk_mode),
    stdcall("GetBkMode", 33, 1, dc::get_bk_mode),
    stdcall("SetTextColor", 34, 2, dc::set_text_color),
    stdcall("GetTextColor", 35, 1, dc::get_text_color),
    stdcall("SetROP2", 36, 2, dc::set_rop2),
    stdcall("GetROP2", 37, 1, dc::get_rop2),
    stdcall("SetPolyFillMode", 38, 2, dc::set_poly_fill_mode),
    stdcall("GetPolyFillMode", 39, 1, dc::get_poly_fill_mode),
    stdcall("SetStretchBltMode", 40, 2, dc::set_stretch_blt_mode),
    stdcall("GetStretchBltMode", 41, 1, dc::get_stretch_blt_mode),
    stdcall("SetTextAlign", 42, 2, dc::set_text_align),
    stdcall("GetTextAlign", 43, 1, dc::get_text_align),
    stdcall("SetMapMode", 44, 2, dc::set_map_mode),
    stdcall("GetMapMode", 45, 1, dc::get_map_mode),
    stdcall("SetWindowOrgEx", 46, 4, dc::set_window_org_ex),
    stdcall("GetWindowOrgEx", 47, 2, dc::get_window_org_ex),
    stdcall("SetWindowExtEx", 48, 4, dc::set_window_ext_ex),
    stdcall("GetWindowExtEx", 49, 2, dc::get_window_ext_ex),
    stdcall("SetViewportOrgEx", 50, 4, dc::set_viewport_org_ex),
    stdcall("GetViewportOrgEx", 51, 2, dc::get_viewport_org_ex),
    stdcall("SetViewportExtEx", 52, 4, dc::set_viewport_ext_ex),
    stdcall("GetViewportExtEx", 53, 2, dc::get_viewport_ext_ex),
    stdcall("OffsetWindowOrgEx", 54, 4, dc::offset_window_org_ex),
    stdcall("OffsetViewportOrgEx", 55, 4, dc::offset_viewport_org_ex),
    stdcall("ScaleWindowExtEx", 56, 6, dc::scale_window_ext_ex),
    stdcall("ScaleViewportExtEx", 57, 6, dc::scale_viewport_ext_ex),
    stdcall("LPtoDP", 58, 3, dc::lp_to_dp),
    stdcall("DPtoLP", 59, 3, dc::dp_to_lp),
    stdcall("SetGraphicsMode", 60, 2, dc::set_graphics_mode),
    stdcall("GetGraphicsMode", 61, 1, dc::get_graphics_mode),
    stdcall("SetWorldTransform", 62, 2, not_implemented),
    stdcall("GetWorldTransform", 63, 2, not_implemented),
    stdcall("ModifyWorldTransform", 64, 3, not_implemented),
    stdcall("GetCurrentPositionEx", 65, 2, dc::get_current_position_ex),
    stdcall("SetBoundsRect", 66, 3, dc::set_bounds_rect),
    stdcall("GetBoundsRect", 67, 3, dc::get_bounds_rect),
    stdcall("SetArcDirection", 68, 2, dc::set_arc_direction),
    stdcall("GetArcDirection", 69, 1, dc::get_arc_direction),
    stdcall("SetMapperFlags", 70, 2, dc::set_mapper_flags),
    stdcall("GetNearestColor", 71, 2, dc::get_nearest_color),
    // Clipping.
    stdcall("SelectClipRgn", 72, 2, dc::select_clip_rgn),
    stdcall("ExtSelectClipRgn", 73, 3, dc::ext_select_clip_rgn),
    stdcall("IntersectClipRect", 74, 5, dc::intersect_clip_rect),
    stdcall("ExcludeClipRect", 75, 5, dc::exclude_clip_rect),
    stdcall("OffsetClipRgn", 76, 3, dc::offset_clip_rgn),
    stdcall("GetClipBox", 77, 2, dc::get_clip_box),
    stdcall("GetClipRgn", 78, 2, dc::get_clip_rgn),
    stdcall("GetRandomRgn", 79, 3, dc::get_random_rgn),
    stdcall("PtVisible", 80, 3, dc::pt_visible),
    stdcall("RectVisible", 81, 2, dc::rect_visible),
    stdcall("SetMetaRgn", 82, 1, dc::set_meta_rgn),
    stdcall("GetMetaRgn", 83, 2, dc::get_meta_rgn),
    stdcall("SelectClipPath", 84, 2, draw::select_clip_path),
    // Regions.
    stdcall("CreateRectRgn", 85, 4, rgn::create_rect_rgn),
    stdcall(
        "CreateRectRgnIndirect",
        86,
        1,
        rgn::create_rect_rgn_indirect,
    ),
    stdcall("CreateEllipticRgn", 87, 4, rgn::create_elliptic_rgn),
    stdcall(
        "CreateEllipticRgnIndirect",
        88,
        1,
        rgn::create_elliptic_rgn_indirect,
    ),
    stdcall("CreateRoundRectRgn", 89, 6, rgn::create_round_rect_rgn),
    stdcall("CreatePolygonRgn", 90, 3, rgn::create_polygon_rgn),
    stdcall("CreatePolyPolygonRgn", 91, 4, rgn::create_poly_polygon_rgn),
    stdcall("ExtCreateRegion", 92, 3, rgn::ext_create_region),
    stdcall("CombineRgn", 93, 4, rgn::combine_rgn),
    stdcall("EqualRgn", 94, 2, rgn::equal_rgn),
    stdcall("OffsetRgn", 95, 3, rgn::offset_rgn),
    stdcall("GetRgnBox", 96, 2, rgn::get_rgn_box),
    stdcall("GetRegionData", 97, 3, rgn::get_region_data),
    stdcall("PtInRegion", 98, 3, rgn::pt_in_region),
    stdcall("RectInRegion", 99, 2, rgn::rect_in_region),
    stdcall("SetRectRgn", 100, 5, rgn::set_rect_rgn),
    stdcall("FillRgn", 101, 3, draw::fill_rgn),
    stdcall("FrameRgn", 102, 5, draw::frame_rgn),
    stdcall("InvertRgn", 103, 2, draw::invert_rgn),
    stdcall("PaintRgn", 104, 2, draw::paint_rgn),
    // Drawing.
    stdcall("MoveToEx", 105, 4, draw::move_to_ex),
    stdcall("LineTo", 106, 3, draw::line_to),
    stdcall("Polyline", 107, 3, draw::polyline),
    stdcall("PolylineTo", 108, 3, draw::polyline_to),
    stdcall("Polygon", 109, 3, draw::polygon),
    stdcall("PolyPolygon", 110, 4, draw::poly_polygon),
    stdcall("PolyPolyline", 111, 4, draw::poly_polyline),
    stdcall("Rectangle", 112, 5, draw::rectangle),
    stdcall("RoundRect", 113, 7, draw::round_rect),
    stdcall("Ellipse", 114, 5, draw::ellipse),
    stdcall("Arc", 115, 9, draw::arc),
    stdcall("Pie", 116, 9, draw::pie),
    stdcall("Chord", 117, 9, draw::chord),
    stdcall("PatBlt", 118, 6, draw::pat_blt),
    stdcall("BitBlt", 119, 9, draw::bit_blt),
    stdcall("StretchBlt", 120, 11, draw::stretch_blt),
    stdcall("MaskBlt", 121, 12, not_implemented),
    stdcall("PlgBlt", 122, 10, not_implemented),
    stdcall("SetPixel", 123, 4, draw::set_pixel),
    stdcall("SetPixelV", 124, 4, draw::set_pixel_v),
    stdcall("GetPixel", 125, 3, draw::get_pixel),
    stdcall("FloodFill", 126, 4, draw::flood_fill),
    stdcall("ExtFloodFill", 127, 5, draw::ext_flood_fill),
    stdcall("LineDDA", 128, 6, line_dda),
    stdcall("PolyBezier", 129, 3, draw::poly_bezier),
    stdcall("PolyBezierTo", 130, 3, draw::poly_bezier_to),
    stdcall("GdiFlush", 131, 0, succeed),
    stdcall("GdiSetBatchLimit", 132, 1, batch_limit),
    stdcall("GdiGetBatchLimit", 133, 0, batch_limit),
    // Bitmaps.
    stdcall("CreateBitmap", 134, 5, bitmap::create_bitmap),
    stdcall(
        "CreateBitmapIndirect",
        135,
        1,
        bitmap::create_bitmap_indirect,
    ),
    stdcall(
        "CreateCompatibleBitmap",
        136,
        3,
        bitmap::create_compatible_bitmap,
    ),
    stdcall(
        "CreateDiscardableBitmap",
        137,
        3,
        bitmap::create_compatible_bitmap,
    ),
    stdcall("CreateDIBitmap", 138, 6, bitmap::create_di_bitmap),
    stdcall("CreateDIBSection", 139, 6, bitmap::create_dib_section),
    stdcall("GetBitmapBits", 140, 3, bitmap::get_bitmap_bits),
    stdcall("SetBitmapBits", 141, 3, bitmap::set_bitmap_bits),
    stdcall("GetDIBits", 142, 7, bitmap::get_di_bits),
    stdcall("SetDIBits", 143, 7, bitmap::set_di_bits),
    stdcall("SetDIBitsToDevice", 144, 12, bitmap::set_di_bits_to_device),
    stdcall("StretchDIBits", 145, 13, bitmap::stretch_di_bits),
    stdcall("GetDIBColorTable", 146, 4, bitmap::get_dib_color_table),
    stdcall("SetDIBColorTable", 147, 4, bitmap::set_dib_color_table),
    stdcall(
        "GetBitmapDimensionEx",
        148,
        2,
        bitmap::get_bitmap_dimension_ex,
    ),
    stdcall(
        "SetBitmapDimensionEx",
        149,
        4,
        bitmap::set_bitmap_dimension_ex,
    ),
    // Text.
    stdcall("CreateFontA", 150, 14, font::create_font),
    stdcall("CreateFontIndirectA", 151, 1, font::create_font_indirect),
    stdcall("GetTextMetricsA", 152, 2, font::get_text_metrics),
    stdcall(
        "GetTextExtentPoint32A",
        153,
        4,
        font::get_text_extent_point32,
    ),
    stdcall("GetTextExtentPointA", 154, 4, font::get_text_extent_point32),
    stdcall(
        "GetTextExtentExPointA",
        155,
        7,
        font::get_text_extent_ex_point,
    ),
    stdcall("TextOutA", 156, 5, font::text_out),
    stdcall("ExtTextOutA", 157, 8, font::ext_text_out),
    stdcall("GetCharWidthA", 158, 4, font::get_char_width),
    stdcall("GetCharWidth32A", 159, 4, font::get_char_width),
    stdcall("GetCharABCWidthsA", 160, 4, font::get_char_abc_widths),
    stdcall("GetTextFaceA", 161, 3, font::get_text_face),
    stdcall("EnumFontFamiliesA", 162, 4, font::enum_font_families),
    stdcall("EnumFontFamiliesExA", 163, 5, font::enum_font_families_ex),
    stdcall("EnumFontsA", 164, 4, font::enum_font_families),
    stdcall(
        "SetTextCharacterExtra",
        165,
        2,
        dc::set_text_character_extra,
    ),
    stdcall(
        "GetTextCharacterExtra",
        166,
        1,
        dc::get_text_character_extra,
    ),
    stdcall("SetTextJustification", 167, 3, dc::set_text_justification),
    stdcall("GetTextCharset", 168, 1, font::get_text_charset),
    stdcall("GetTextCharsetInfo", 169, 3, font::get_text_charset_info),
    stdcall("AddFontResourceA", 170, 1, font::add_font_resource),
    stdcall("RemoveFontResourceA", 171, 1, font::remove_font_resource),
    stdcall("GetOutlineTextMetricsA", 172, 3, nothing),
    stdcall("GetKerningPairsA", 173, 3, nothing),
    stdcall("GetFontData", 174, 5, font::get_font_data),
    stdcall("GetGlyphOutlineA", 175, 7, font::get_font_data),
    stdcall("TranslateCharsetInfo", 176, 3, font::translate_charset_info),
    stdcall(
        "GetCharacterPlacementA",
        177,
        6,
        font::get_character_placement,
    ),
    stdcall("TextOutW", 178, 5, font::text_out_w),
    stdcall("ExtTextOutW", 179, 8, font::ext_text_out_w),
    stdcall(
        "GetTextExtentPoint32W",
        180,
        4,
        font::get_text_extent_point32_w,
    ),
    stdcall(
        "GetTextExtentPointW",
        181,
        4,
        font::get_text_extent_point32_w,
    ),
    stdcall("GetCharWidthW", 182, 4, font::get_char_width),
    stdcall("GetCharWidth32W", 183, 4, font::get_char_width),
    stdcall("CreateFontW", 184, 14, not_implemented),
    stdcall("CreateFontIndirectW", 185, 1, not_implemented),
    stdcall("GetTextMetricsW", 186, 2, not_implemented),
    stdcall("GetTextFaceW", 187, 3, not_implemented),
    stdcall("EnumFontFamiliesW", 188, 4, not_implemented),
    stdcall("EnumFontFamiliesExW", 189, 5, not_implemented),
    stdcall("GetRasterizerCaps", 190, 2, font::get_rasterizer_caps),
    // Palettes.
    stdcall("CreatePalette", 191, 1, palette::create_palette),
    stdcall(
        "CreateHalftonePalette",
        192,
        1,
        palette::create_halftone_palette,
    ),
    stdcall("GetPaletteEntries", 193, 4, palette::get_palette_entries),
    stdcall("SetPaletteEntries", 194, 4, palette::set_palette_entries),
    stdcall("ResizePalette", 195, 2, palette::resize_palette),
    stdcall("AnimatePalette", 196, 4, palette::animate_palette),
    stdcall("SelectPalette", 197, 3, palette::select_palette),
    stdcall("RealizePalette", 198, 1, palette::realize_palette),
    stdcall(
        "GetSystemPaletteEntries",
        199,
        4,
        palette::get_system_palette_entries,
    ),
    stdcall(
        "GetSystemPaletteUse",
        200,
        1,
        palette::get_system_palette_use,
    ),
    stdcall(
        "SetSystemPaletteUse",
        201,
        2,
        palette::set_system_palette_use,
    ),
    stdcall(
        "GetNearestPaletteIndex",
        202,
        2,
        palette::get_nearest_palette_index,
    ),
    stdcall("UpdateColors", 203, 1, succeed),
    // Color spaces, printing, OpenGL and the rest.
    stdcall("CreateColorSpaceA", 204, 1, create_color_space),
    stdcall("GetColorSpace", 205, 1, get_color_space),
    stdcall("GetLogColorSpaceA", 206, 3, get_log_color_space),
    stdcall("SetColorSpace", 207, 2, nothing),
    stdcall("DeleteColorSpace", 208, 1, delete_object),
    stdcall("SetICMMode", 209, 2, nothing),
    stdcall("GetDeviceGammaRamp", 210, 2, not_implemented),
    stdcall("SetDeviceGammaRamp", 211, 2, not_implemented),
    stdcall("StartDocA", 212, 2, nothing),
    stdcall("EndDoc", 213, 1, nothing),
    stdcall("StartPage", 214, 1, nothing),
    stdcall("EndPage", 215, 1, nothing),
    stdcall("AbortDoc", 216, 1, nothing),
    stdcall("SetAbortProc", 217, 2, nothing),
    stdcall("Escape", 218, 5, nothing),
    stdcall("ExtEscape", 219, 6, nothing),
    stdcall("ChoosePixelFormat", 220, 2, nothing),
    stdcall("SetPixelFormat", 221, 3, nothing),
    stdcall("GetPixelFormat", 222, 1, nothing),
    stdcall("DescribePixelFormat", 223, 4, nothing),
    stdcall("SwapBuffers", 224, 1, nothing),
    stdcall("GdiComment", 225, 3, succeed),
    stdcall("CombineTransform", 226, 3, not_implemented),
    // Paths.
    stdcall("BeginPath", 227, 1, draw::begin_path),
    stdcall("EndPath", 228, 1, draw::end_path),
    stdcall("AbortPath", 229, 1, draw::abort_path),
    stdcall("CloseFigure", 230, 1, draw::close_figure),
    stdcall("FillPath", 231, 1, draw::fill_path),
    stdcall("StrokePath", 232, 1, draw::stroke_path),
    stdcall("StrokeAndFillPath", 233, 1, draw::stroke_and_fill_path),
    stdcall("FlattenPath", 234, 1, draw::flatten_path),
    stdcall("WidenPath", 235, 1, draw::flatten_path),
    stdcall("PathToRegion", 236, 1, draw::path_to_region),
    stdcall("GetPath", 237, 4, draw::get_path),
    stdcall("SetMetaFileBitsEx", 238, 2, metafile::set_meta_file_bits_ex),
    stdcall("GetMetaFileBitsEx", 239, 3, metafile::get_meta_file_bits_ex),
    stdcall("DeleteMetaFile", 240, 1, metafile::delete_meta_file),
    stdcall("PlayMetaFile", 241, 2, metafile::play_meta_file),
];

crate::state_fields!(Pen {
    style,
    width,
    color,
    ext,
    brush_style,
    hatch,
    dashes
});
crate::state_fields!(Pattern {
    width,
    height,
    pixels,
    mono
});
crate::state_fields!(Brush {
    style,
    color,
    hatch,
    pattern,
    usage
});
crate::state_fields!(Font { log });
crate::state_fields!(Dib {
    bits,
    header,
    colors,
    masks,
    top_down,
    compression,
    section,
    offset
});
crate::state_fields!(Bitmap {
    width,
    height,
    bpp,
    planes,
    stride,
    bits,
    dib,
    dc,
    dimension
});
crate::state_fields!(Palette { entries, mapping });
crate::state_fields!(Object {
    kind,
    stock,
    pid,
    deleted
});
crate::state_fields!(State {
    objects,
    next,
    system_palette,
    font_files,
    system_use,
    system_owner,
    static_use,
    primary_palette,
    last_realized
});
super::state_variants!(Kind {
    Pen { pen },
    Brush { brush },
    Font { font },
    Bitmap { bitmap },
    Palette { palette },
    Region { region },
    Dc { dc },
    ColorSpace { data },
    MetaFile { data },
});
