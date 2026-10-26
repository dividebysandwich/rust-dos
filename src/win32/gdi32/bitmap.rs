//! Bitmaps: GDI's own (device-dependent, monochrome or the display's
//! depth), DIB sections, whose bits are in the program's memory, and the
//! conversions between them and the DIBs programs pass in their memory.

use super::draw::{self, Source};
use super::surface::{self, Surface, Target};
use super::{
    Bitmap, Dib, ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, ERROR_NOT_ENOUGH_MEMORY, FALSE,
    Kind, Pattern, TRUE, add, gdi, init, set_error,
};
use crate::cpu::Cpu;
use crate::win32::user32::{read_words, write_words};

const BI_RGB: u32 = 0;
const BI_RLE8: u32 = 1;
const BI_RLE4: u32 = 2;
const BI_BITFIELDS: u32 = 3;
const DIB_RGB_COLORS: u32 = 0;
const DIB_PAL_COLORS: u32 = 1;
const CBM_INIT: u32 = 4;
/// The most bytes a bitmap's pixels take.
const MAX_BITS: usize = 64 << 20;

/// A DIB's header, as a BITMAPINFO at an address says it.
#[derive(Clone, Debug, Default)]
pub struct DibInfo {
    pub width: i32,
    /// Negative for rows that run top down.
    pub height: i32,
    pub bpp: u32,
    pub compression: u32,
    pub colors: Vec<u32>,
    pub masks: [u32; 3],
    /// The header's size, and where the color table starts after it.
    pub header_size: u32,
    pub image_size: u32,
    /// A BITMAPCOREHEADER, whose colors are RGBTRIPLEs.
    pub core: bool,
    /// How many entries its color table has.
    pub table_len: u32,
    /// With DIB_PAL_COLORS, the logical palette's indices its color
    /// table had, which `colors` has the colors of.
    pub pal_indices: Option<Vec<u16>>,
}

impl DibInfo {
    pub fn abs_height(&self) -> i32 {
        self.height.saturating_abs()
    }

    pub fn top_down(&self) -> bool {
        self.height < 0
    }

    pub fn stride(&self) -> usize {
        surface::dib_stride(self.width, self.bpp)
    }

    /// The bytes of its rows, at most MAX_BITS + 1 (too many).
    pub fn bits_size(&self) -> u32 {
        let size = self.stride().saturating_mul(self.abs_height() as usize);
        size.min(MAX_BITS + 1) as u32
    }
}

/// An RGBQUAD as a COLORREF.
fn quad_to_color(q: u32) -> u32 {
    let (b, g, r) = (q & 0xFF, (q >> 8) & 0xFF, (q >> 16) & 0xFF);
    r | g << 8 | b << 16
}

fn color_to_quad(c: u32) -> u32 {
    let (r, g, b) = (c & 0xFF, (c >> 8) & 0xFF, (c >> 16) & 0xFF);
    b | g << 8 | r << 16
}

/// Read the BITMAPINFO at `at`: with DIB_PAL_COLORS its color table's
/// indices are entries of the palette of `hdc`. None if it isn't a DIB
/// GDI draws.
pub fn read_info(cpu: &mut Cpu, at: u32, usage: u32, hdc: u32) -> Option<DibInfo> {
    let size = cpu.bus.guest_read_32(at);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let mut info = DibInfo::default();
    if size == 12 {
        let w = read_words(cpu, at + 4, 2);
        info.width = w[0] as u16 as i16 as i32;
        info.height = (w[0] >> 16) as u16 as i16 as i32;
        info.bpp = w[1] >> 16;
        info.core = true;
    } else if size >= 40 {
        let w = read_words(cpu, at, 10);
        info.width = w[1] as i32;
        info.height = w[2] as i32;
        info.bpp = w[3] >> 16;
        info.compression = w[4];
        info.image_size = w[5];
        info.table_len = w[8];
    } else {
        return None;
    }
    info.header_size = size;
    if !matches!(info.bpp, 1 | 4 | 8 | 16 | 24 | 32) || info.width < 0 {
        return None;
    }
    // Rows that take more than a bitmap may: GDI makes no room for them.
    if info.bits_size() as usize > MAX_BITS {
        return None;
    }
    let colors_at = at + size;
    match info.compression {
        BI_BITFIELDS if info.bpp == 16 || info.bpp == 32 => {
            let m = if size >= 52 {
                read_words(cpu, at + 40, 3)
            } else {
                read_words(cpu, colors_at, 3)
            };
            info.masks = [m[0], m[1], m[2]];
            if size < 52 {
                info.header_size = size;
            }
        }
        BI_RGB | BI_RLE8 | BI_RLE4 => info.masks = surface::default_masks(info.bpp),
        _ => return None,
    }
    if info.bpp <= 8 {
        let max = 1u32 << info.bpp;
        let n = if info.table_len == 0 || info.table_len > max {
            max
        } else {
            info.table_len
        };
        info.table_len = n;
        info.colors = match usage {
            DIB_PAL_COLORS => {
                let selected = gdi(cpu).dc(hdc).map(|d| d.attrs.palette);
                let palette = selected
                    .and_then(|p| gdi(cpu).palette(p))
                    .map(|p| p.entries.clone())
                    .unwrap_or_default();
                let indices: Vec<u16> = (0..n)
                    .map(|i| cpu.bus.guest_read_16(colors_at + 2 * i))
                    .collect();
                let colors = indices
                    .iter()
                    .map(|&index| {
                        // An index past the palette's end wraps around it.
                        let e = match palette.len() {
                            0 => [0; 4],
                            n => palette[index as usize % n],
                        };
                        e[0] as u32 | (e[1] as u32) << 8 | (e[2] as u32) << 16
                    })
                    .collect();
                info.pal_indices = Some(indices);
                colors
            }
            _ if info.core => (0..n)
                .map(|i| {
                    let mut b = [0u8; 3];
                    cpu.bus.guest_read_bytes(colors_at + 3 * i, &mut b);
                    b[2] as u32 | (b[1] as u32) << 8 | (b[0] as u32) << 16
                })
                .collect(),
            _ => read_words(cpu, colors_at, n as usize)
                .into_iter()
                .map(quad_to_color)
                .collect(),
        };
    } else {
        info.table_len = 0;
    }
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(info)
}

/// The bytes of the color table `info` has after its header, as
/// `usage` has it (DIB_PAL_COLORS: 16-bit indices).
fn table_bytes(info: &DibInfo, usage: u32) -> u32 {
    let entry = if usage == DIB_PAL_COLORS {
        2
    } else if info.core {
        3
    } else {
        4
    };
    let masks = if info.compression == BI_BITFIELDS && info.header_size < 52 {
        12
    } else {
        0
    };
    info.table_len * entry + masks
}

/// A BITMAP structure for GetObjectA.
pub fn bitmap_struct(b: &Bitmap) -> Vec<u8> {
    let bits = b.dib.as_ref().map_or(0, |d| d.bits);
    let mut out = Vec::with_capacity(24);
    for v in [0, b.width as u32, b.height as u32, b.stride] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&b.planes.to_le_bytes());
    out.extend_from_slice(&b.bpp.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out
}

/// The depth a requested one is made: 1, 4, 8, 16, 24 or 32.
fn round_depth(bpp: u32) -> Option<u16> {
    Some(match bpp {
        0 => return None,
        1 => 1,
        2..=4 => 4,
        5..=8 => 8,
        9..=16 => 16,
        17..=24 => 24,
        25..=32 => 32,
        _ => return None,
    })
}

/// A new bitmap of GDI's, its pixels 0.
pub fn new_ddb(cpu: &mut Cpu, width: i32, height: i32, bpp: u16) -> u32 {
    let stride = surface::ddb_stride(width, bpp as u32) as u32;
    let size = stride as usize * height as usize;
    if size > MAX_BITS {
        set_error(cpu, ERROR_NOT_ENOUGH_MEMORY);
        return 0;
    }
    add(
        cpu,
        Kind::Bitmap {
            bitmap: Bitmap {
                width,
                height,
                bpp,
                planes: 1,
                stride,
                bits: vec![0; size],
                ..Bitmap::default()
            },
        },
    )
}

/// The default bitmap, which a bitmap of no size is.
fn default_bitmap(cpu: &mut Cpu) -> u32 {
    init(cpu);
    super::handle(super::DEFAULT_BITMAP as usize)
}

/// CreateBitmap(width, height, planes, bpp, bits).
pub fn create_bitmap(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (w, h, planes, bpp, bits) = (args[0] as i32, args[1] as i32, args[2], args[3], args[4]);
    if w < 0 || h < 0 || planes > 32 || bpp > 32 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    if w == 0 || h == 0 {
        return Some(default_bitmap(cpu));
    }
    let Some(depth) = round_depth(bpp * planes.max(1)) else {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    };
    let handle = new_ddb(cpu, w, h, depth);
    if handle != 0 && bits != 0 {
        let size = gdi(cpu).bitmap(handle).map_or(0, |b| b.bits.len());
        let mut data = vec![0u8; size];
        cpu.bus.guest_read_bytes(bits, &mut data);
        if let Some(b) = gdi(cpu).bitmap_mut(handle) {
            b.bits = data;
        }
    }
    Some(handle)
}

/// CreateBitmapIndirect(bitmap).
pub fn create_bitmap_indirect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let w = read_words(cpu, args[0], 6);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let (planes, bpp) = (w[4] & 0xFFFF, w[4] >> 16);
    create_bitmap(cpu, &[w[1], w[2], planes, bpp, w[5]])
}

/// CreateCompatibleBitmap(hdc, width, height): the depth of the DC's
/// bitmap for a memory DC (a DIB section for one that has a DIB
/// section), else the display's.
pub fn create_compatible_bitmap(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, w, h) = (args[0], args[1] as i32, args[2] as i32);
    init(cpu);
    let Some(dc) = gdi(cpu).dc(hdc).cloned() else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    };
    if w < 0 || h < 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    if w == 0 || h == 0 {
        return Some(default_bitmap(cpu));
    }
    if dc.kind == super::dc::MEMORY
        && let Some(b) = gdi(cpu).bitmap(dc.attrs.bitmap).cloned()
    {
        if let Some(dib) = &b.dib {
            let info = DibInfo {
                width: w,
                height: if dib.top_down { -h } else { h },
                bpp: b.bpp as u32,
                compression: dib.compression,
                colors: dib.colors.clone(),
                masks: dib.masks,
                header_size: 40,
                table_len: dib.colors.len() as u32,
                ..DibInfo::default()
            };
            return Some(make_dib_section(cpu, &info, 0, 0).0);
        }
        if b.bpp == 1 {
            return Some(new_ddb(cpu, w, h, 1));
        }
    }
    let depth = super::dc::device_depth(cpu) as u16;
    Some(new_ddb(cpu, w, h, depth))
}

/// Make a DIB section of `info`'s format: its handle and its bits'
/// address, or (0, 0).
fn make_dib_section(cpu: &mut Cpu, info: &DibInfo, section: u32, offset: u32) -> (u32, u32) {
    if info.bits_size() as usize > MAX_BITS {
        set_error(cpu, ERROR_NOT_ENOUGH_MEMORY);
        return (0, 0);
    }
    let size = info.bits_size().max(1);
    let Some(bits) = dib_bits(cpu, size, section, offset) else {
        return (0, 0);
    };
    let mut header = vec![0u8; 40];
    let words = [
        40,
        info.width as u32,
        info.height as u32,
        1 | info.bpp << 16,
        info.compression,
        info.bits_size(),
        0,
        0,
        if info.bpp <= 8 {
            info.colors.len() as u32
        } else {
            0
        },
        0,
    ];
    for (i, w) in words.iter().enumerate() {
        header[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
    }
    let handle = add(
        cpu,
        Kind::Bitmap {
            bitmap: Bitmap {
                width: info.width,
                height: info.abs_height(),
                bpp: info.bpp as u16,
                planes: 1,
                stride: info.stride() as u32,
                bits: Vec::new(),
                dib: Some(Dib {
                    bits,
                    header,
                    colors: info.colors.clone(),
                    masks: if info.compression == BI_BITFIELDS {
                        info.masks
                    } else {
                        [0; 3]
                    },
                    top_down: info.top_down(),
                    compression: info.compression,
                    section,
                    offset,
                }),
                ..Bitmap::default()
            },
        },
    );
    if handle == 0 {
        free_dib_bits(cpu, bits, section, offset);
        return (0, 0);
    }
    (handle, bits)
}

/// Where a DIB section's `size` bytes of bits go: pages of GDI's own, or
/// with a file mapping `section`, a view of it from `offset` (a multiple
/// of 4). None with the error set.
fn dib_bits(cpu: &mut Cpu, size: u32, section: u32, offset: u32) -> Option<u32> {
    if section == 0 {
        let bits = crate::win32::kernel32::alloc_pages(cpu, size);
        if bits.is_none() {
            set_error(cpu, ERROR_NOT_ENOUGH_MEMORY);
        }
        return bits;
    }
    let (view, len) = crate::win32::kernel32::map_dib_view(cpu, section)?;
    if !offset.is_multiple_of(4) || offset as u64 + size as u64 > len as u64 {
        crate::win32::kernel32::unmap_dib_view(cpu, view);
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return None;
    }
    Some(view + offset)
}

/// Let go of the bits `dib_bits` gave.
pub fn free_dib_bits(cpu: &mut Cpu, bits: u32, section: u32, offset: u32) {
    if section == 0 {
        crate::win32::kernel32::free_pages(cpu, bits);
    } else {
        crate::win32::kernel32::unmap_dib_view(cpu, bits - offset);
    }
}

/// CreateDIBSection(hdc, info, usage, bits, section, offset).
pub fn create_dib_section(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, at, usage, out, section, offset) =
        (args[0], args[1], args[2], args[3], args[4], args[5]);
    init(cpu);
    if out != 0 {
        cpu.bus.guest_write_32(out, 0);
    }
    if usage > DIB_PAL_COLORS || at == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let Some(info) = read_info(cpu, at, usage, hdc) else {
        if cpu.bus.guest_faulted() {
            return None;
        }
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    };
    if info.compression == BI_RLE8 || info.compression == BI_RLE4 {
        // A compressed DIB section fails without an error.
        return Some(0);
    }
    if info.width == 0 || info.height == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let (handle, bits) = make_dib_section(cpu, &info, section, offset);
    if out != 0 && handle != 0 {
        cpu.bus.guest_write_32(out, bits);
    }
    Some(handle)
}

/// Whether a DIB's color table is black and then white, which makes
/// CreateDIBitmap's bitmap monochrome.
fn black_and_white(info: &DibInfo) -> bool {
    info.bpp == 1 && info.colors.len() == 2 && info.colors[0] == 0 && info.colors[1] == 0xFFFFFF
}

/// CreateDIBitmap(hdc, header, init, bits, info, usage).
pub fn create_di_bitmap(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, header, flags, bits, at, usage) =
        (args[0], args[1], args[2], args[3], args[4], args[5]);
    init(cpu);
    if header == 0 {
        return Some(0);
    }
    let size = cpu.bus.guest_read_32(header);
    let (width, height) = if size == 12 {
        let w = cpu.bus.guest_read_32(header + 4);
        (w as u16 as i16 as i32, (w >> 16) as u16 as i16 as i32)
    } else {
        (
            cpu.bus.guest_read_32(header + 4) as i32,
            cpu.bus.guest_read_32(header + 8) as i32,
        )
    };
    let height = height.saturating_abs();
    if width < 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let info = if at != 0 {
        read_info(cpu, at, usage, hdc)
    } else {
        None
    };
    let mono = hdc == 0 || info.as_ref().is_some_and(black_and_white);
    let handle = if width == 0 || height == 0 {
        default_bitmap(cpu)
    } else if mono {
        new_ddb(cpu, width, height, 1)
    } else {
        let Some(h) = create_compatible_bitmap(cpu, &[hdc, width as u32, height as u32]) else {
            return Some(0);
        };
        h
    };
    if handle != 0 && flags & CBM_INIT != 0 && bits != 0 && at != 0 {
        set_bits(cpu, handle, 0, height as u32, bits, at, usage, hdc);
    }
    Some(handle)
}

/// The pixels of the bitmap `handle` as a surface: all of it.
fn bitmap_surface(cpu: &mut Cpu, handle: u32) -> Option<Surface> {
    Surface::load_all(cpu, Target::Bitmap(handle))
}

/// GetBitmapBits(bitmap, count, bits): its rows, top first, each a whole
/// number of words.
pub fn get_bitmap_bits(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (handle, count, out) = (args[0], args[1] as usize, args[2]);
    let Some(b) = gdi(cpu).bitmap(handle).map(|b| b.clone_header()) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    };
    let stride = surface::ddb_stride(b.width, b.bpp as u32);
    let total = stride * b.height as usize;
    if out == 0 {
        return Some(total as u32);
    }
    let Some(s) = bitmap_surface(cpu, handle) else {
        return Some(0);
    };
    let rows = (count / stride.max(1)).min(b.height as usize);
    let mut data = Vec::with_capacity(rows * stride);
    for y in 0..rows {
        let start = y * s.stride;
        data.extend_from_slice(&s.data[start..start + stride]);
    }
    cpu.bus.guest_write_bytes(out, &data);
    Some(data.len() as u32)
}

/// SetBitmapBits(bitmap, count, bits).
pub fn set_bitmap_bits(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (handle, count, at) = (args[0], args[1] as usize, args[2]);
    let Some(b) = gdi(cpu).bitmap(handle).map(|b| b.clone_header()) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    };
    let stride = surface::ddb_stride(b.width, b.bpp as u32);
    let Some(mut s) = bitmap_surface(cpu, handle) else {
        return Some(0);
    };
    let count = count.min(stride * b.height as usize);
    let mut data = vec![0u8; count];
    cpu.bus.guest_read_bytes(at, &mut data);
    if cpu.bus.guest_faulted() {
        return None;
    }
    for (y, row) in data.chunks(stride).enumerate() {
        // Through `put`, so the surface knows it changed.
        let start = y * s.stride;
        s.data[start..start + row.len()].copy_from_slice(row);
    }
    if count > 0 {
        let v = s.get(0, 0);
        s.put(0, 0, v);
    }
    s.store(cpu);
    Some(count as u32)
}

/// The colors a DIB of `bpp` bits a pixel gets when it is made from a
/// bitmap that has none: black and white, the 16 VGA colors, or the
/// halftone palette.
fn default_table(bpp: u32) -> Vec<u32> {
    match bpp {
        1 => vec![0x000000, 0xFFFFFF],
        4 => vec![
            0x000000, 0x000080, 0x008000, 0x008080, 0x800000, 0x800080, 0x808000, 0x808080,
            0xC0C0C0, 0x0000FF, 0x00FF00, 0x00FFFF, 0xFF0000, 0xFF00FF, 0xFFFF00, 0xFFFFFF,
        ],
        _ => super::palette::halftone_colors(),
    }
}

/// The scan line `scan` of a DIB of `info`'s height as a row of the
/// bitmap, the top first.
fn row_of_scan(info: &DibInfo, height: i32, scan: i32) -> i32 {
    if info.top_down() {
        scan
    } else {
        height - 1 - scan
    }
}

/// GetDIBits(hdc, bitmap, start, lines, bits, info, usage).
pub fn get_di_bits(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, handle, start, lines, out, at, usage) = (
        args[0], args[1], args[2], args[3], args[4], args[5], args[6],
    );
    if gdi(cpu).dc(hdc).is_none() {
        set_error(
            cpu,
            if hdc == 0 {
                ERROR_INVALID_PARAMETER
            } else {
                ERROR_INVALID_HANDLE
            },
        );
        return Some(0);
    }
    let Some(b) = gdi(cpu).bitmap(handle).map(|b| b.clone_header()) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    };
    if lines == 0 && out != 0 {
        // Windows 95 doesn't take 0 scan lines.
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let size = cpu.bus.guest_read_32(at);
    if size != 12 && size < 40 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let core = size == 12;
    let bpp_at = if core { at + 10 } else { at + 14 };
    let bpp = cpu.bus.guest_read_16(bpp_at) as u32;
    let bitmap_bpp = b.bpp as u32 * b.planes as u32;
    if bpp == 0 {
        // Fill in the header with the bitmap's format; its masks, if it
        // has any, are for a call that names the depth.
        if out != 0 {
            set_error(cpu, ERROR_INVALID_PARAMETER);
            return Some(0);
        }
        let stride = surface::dib_stride(b.width, bitmap_bpp);
        if core {
            cpu.bus.guest_write_16(at + 4, b.width as u16);
            cpu.bus.guest_write_16(at + 6, b.height as u16);
            cpu.bus.guest_write_16(at + 8, 1);
            cpu.bus.guest_write_16(at + 10, bitmap_bpp as u16);
        } else {
            let height = match &b.dib {
                Some(d) if d.top_down => -b.height,
                _ => b.height,
            };
            let compression = if bitmap_bpp == 16 || bitmap_bpp == 32 {
                BI_BITFIELDS
            } else {
                BI_RGB
            };
            write_words(
                cpu,
                at + 4,
                &[
                    b.width as u32,
                    height as u32,
                    1 | bitmap_bpp << 16,
                    compression,
                    (stride * b.height as usize) as u32,
                    0,
                    0,
                    0,
                    0,
                ],
            );
        }
        return Some(b.height.max(1) as u32);
    }
    let Some(mut info) = read_info(cpu, at, DIB_RGB_COLORS, hdc) else {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    };
    if info.compression == BI_RLE8 || info.compression == BI_RLE4 {
        return Some(0);
    }
    // The color table: the DIB section's own when it has the same depth,
    // else what the depth gets.
    let table: Vec<u32> = if bpp <= 8 {
        match &b.dib {
            Some(d) if bitmap_bpp == bpp => d.colors.clone(),
            _ if bitmap_bpp == 1 && b.dib.is_none() && bpp == 1 => vec![0, 0xFFFFFF],
            // A palette device's bitmap: its indices, with the system
            // palette's colors.
            None if bitmap_bpp == 8 && bpp == 8 => super::palette::system_colors(cpu),
            _ => default_table(bpp),
        }
    } else {
        Vec::new()
    };
    info.colors = table.clone();
    // The masks are the bitmap's at its own depth, else the depth's.
    info.masks = match &b.dib {
        Some(d) if bitmap_bpp == bpp && d.compression == BI_BITFIELDS => d.masks,
        None if bitmap_bpp == bpp && info.compression == BI_BITFIELDS => {
            surface::ddb_masks(cpu, bpp)
        }
        _ => surface::default_masks(bpp),
    };
    if !core {
        // The header gets the size of the image and of the table.
        let image = (info.stride() * info.abs_height() as usize) as u32;
        cpu.bus.guest_write_32(at + 20, image);
        if bpp <= 8 {
            cpu.bus.guest_write_32(at + 32, 0);
        }
    }
    if bpp <= 8 {
        let colors_at = at + size;
        let n = 1u32 << bpp;
        for i in 0..n {
            let c = table.get(i as usize).copied().unwrap_or(0);
            if usage == DIB_PAL_COLORS {
                cpu.bus.guest_write_16(colors_at + 2 * i, i as u16);
            } else if core {
                cpu.bus
                    .guest_write_bytes(colors_at + 3 * i, &color_to_quad(c).to_le_bytes()[..3]);
            } else {
                cpu.bus.guest_write_32(colors_at + 4 * i, color_to_quad(c));
            }
        }
        info.table_len = n;
    } else if info.compression == BI_BITFIELDS && size < 52 {
        write_words(cpu, at + size, &info.masks);
    }
    if out == 0 {
        return Some(b.height.max(1) as u32);
    }
    let Some(src) = bitmap_surface(cpu, handle) else {
        return Some(0);
    };
    let height = info.abs_height();
    let lines = (lines as i32).min(height - start as i32).max(0);
    let stride = info.stride();
    let mut dib = Surface::memory(info.width, 1, bpp, table, info.masks);
    let mut copied = 0;
    for i in 0..lines {
        let scan = start as i32 + i;
        let row = row_of_scan(&info, height, scan);
        let at_row = out + (i as usize * stride) as u32;
        if row < 0 || row >= b.height {
            continue;
        }
        dib.data.fill(0);
        for x in 0..info.width.min(b.width) {
            let v = src.get(x, row);
            let color = src.color_of(v);
            let same = bpp == bitmap_bpp
                && if bpp > 8 {
                    src.masks == dib.masks
                } else {
                    src.colors == dib.colors
                };
            let nv = if same || (bpp == 1 && src.mono) {
                v
            } else {
                dib.value_of(color)
            };
            dib.put(x, 0, nv);
        }
        cpu.bus.guest_write_bytes(at_row, &dib.data[..stride]);
        copied += 1;
    }
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(copied)
}

/// Set rows of the bitmap `handle` from the DIB at `bits` with the header
/// at `at`: its scan lines `start` to `start + lines`. How many it set.
#[allow(clippy::too_many_arguments)]
fn set_bits(
    cpu: &mut Cpu,
    handle: u32,
    start: u32,
    lines: u32,
    bits: u32,
    at: u32,
    usage: u32,
    hdc: u32,
) -> u32 {
    let Some(info) = read_info(cpu, at, usage, hdc) else {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return 0;
    };
    let Some(mut dst) = bitmap_surface(cpu, handle) else {
        return 0;
    };
    let Some(src) = read_dib(cpu, &info, bits, start, lines) else {
        return 0;
    };
    let height = info.abs_height();
    let map = dst.device.then(|| {
        let hpal = gdi(cpu).dc(hdc).map_or(0, |d| d.attrs.palette);
        super::palette::DeviceMap::new(cpu, hpal)
    });
    let mut xlat = draw::Translate::new(&src, &dst, map.as_ref(), 0, 0, 1);
    let mut set = 0;
    for i in 0..src.height {
        let scan = start as i32 + i;
        // The DIB's scan lines count from its bottom unless its rows run
        // top down; the bitmap's rows from its top.
        let row = row_of_scan(&info, height, scan);
        if row < 0 || row >= dst.height {
            continue;
        }
        // The rows of `src` are in the order the bits have them.
        let src_row = if info.top_down() {
            i
        } else {
            src.height - 1 - i
        };
        for x in 0..info.width.min(dst.width) {
            let v = src.get(x, src_row);
            let nv = if dst.mono && src.bpp == 1 && src.colors == [0, 0xFFFFFF] {
                v
            } else {
                xlat.get(&src, v, &dst)
            };
            let nv = if dst.mono && !(src.bpp == 1 && src.colors == [0, 0xFFFFFF]) {
                let c = src.color_of(v);
                let sum = (c & 0xFF) + ((c >> 8) & 0xFF) + ((c >> 16) & 0xFF);
                (sum >= 384) as u32
            } else {
                nv
            };
            dst.put(x, row, nv);
        }
        set += 1;
    }
    dst.store(cpu);
    set
}

/// The scan lines `start` to `start + lines` of the DIB at `bits`
/// (decoded from RLE) as a surface whose rows run top down: for a
/// top-down DIB its row 0 is scan line `start`, for a bottom-up one the
/// last scan line read. None if its memory isn't there.
pub fn read_dib(
    cpu: &mut Cpu,
    info: &DibInfo,
    bits: u32,
    start: u32,
    lines: u32,
) -> Option<Surface> {
    let height = info.abs_height();
    let lines = (lines as i32).min(height - start as i32).max(0);
    let mut s = Surface::memory(info.width, lines, info.bpp, info.colors.clone(), info.masks);
    s.pal_indices = info.pal_indices.clone();
    if info.compression == BI_RLE8 || info.compression == BI_RLE4 {
        let size = if info.image_size != 0 {
            info.image_size.min(MAX_BITS as u32)
        } else {
            info.bits_size()
        };
        let mut data = vec![0u8; size as usize];
        cpu.bus.guest_read_bytes(bits, &mut data);
        let full = decode_rle(&data, info);
        // `full` has the scan lines from the bottom; `s` has them from
        // the last one wanted, as for bits that aren't compressed.
        let stride = s.stride;
        for i in 0..lines as usize {
            let scan = start as usize + i;
            let at = scan * stride;
            if at + stride <= full.len() {
                let row = lines as usize - 1 - i;
                s.data[row * stride..row * stride + stride].copy_from_slice(&full[at..at + stride]);
            }
        }
        return Some(s);
    }
    let stride = s.stride;
    let mut data = vec![0u8; stride * lines as usize];
    cpu.bus.guest_read_bytes(bits, &mut data);
    if cpu.bus.guest_faulted() {
        return None;
    }
    // The bits have the first scan line first, which for a bottom-up DIB
    // is its bottom row: the surface has them the other way round, so
    // its row 0 is the top of what was read.
    if info.top_down() {
        s.data = data;
    } else {
        for r in 0..lines as usize {
            let from = (lines as usize - 1 - r) * stride;
            s.data[r * stride..r * stride + stride].copy_from_slice(&data[from..from + stride]);
        }
    }
    Some(s)
}

/// Decode an RLE8 or RLE4 DIB into rows of its bits, bottom first.
fn decode_rle(data: &[u8], info: &DibInfo) -> Vec<u8> {
    let stride = info.stride();
    let height = info.abs_height() as usize;
    let mut out = vec![0u8; stride * height];
    let four = info.compression == BI_RLE4;
    let (mut x, mut y) = (0usize, 0usize);
    let mut i = 0;
    let put = |out: &mut Vec<u8>, x: usize, y: usize, v: u8| {
        if y >= height || x >= info.width as usize {
            return;
        }
        let row = y * stride;
        if four {
            let b = &mut out[row + x / 2];
            if x.is_multiple_of(2) {
                *b = (*b & 0x0F) | (v << 4);
            } else {
                *b = (*b & 0xF0) | (v & 15);
            }
        } else {
            out[row + x] = v;
        }
    };
    while i + 1 < data.len() {
        let (n, v) = (data[i] as usize, data[i + 1]);
        i += 2;
        if n > 0 {
            for k in 0..n {
                let value = if four {
                    if k % 2 == 0 { v >> 4 } else { v & 15 }
                } else {
                    v
                };
                put(&mut out, x, y, value);
                x += 1;
            }
            continue;
        }
        match v {
            0 => {
                x = 0;
                y += 1;
            }
            1 => break,
            2 => {
                if i + 1 >= data.len() {
                    break;
                }
                x += data[i] as usize;
                y += data[i + 1] as usize;
                i += 2;
            }
            count => {
                let count = count as usize;
                let bytes = if four { count.div_ceil(2) } else { count };
                for k in 0..count {
                    let byte = data
                        .get(i + if four { k / 2 } else { k })
                        .copied()
                        .unwrap_or(0);
                    let value = if four {
                        if k % 2 == 0 { byte >> 4 } else { byte & 15 }
                    } else {
                        byte
                    };
                    put(&mut out, x, y, value);
                    x += 1;
                }
                i += bytes.next_multiple_of(2);
            }
        }
    }
    out
}

/// SetDIBits(hdc, bitmap, start, lines, bits, info, usage).
pub fn set_di_bits(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, handle, start, lines, bits, at, usage) = (
        args[0], args[1], args[2], args[3], args[4], args[5], args[6],
    );
    if gdi(cpu).bitmap(handle).is_none() {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    }
    if usage > DIB_PAL_COLORS {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    Some(set_bits(cpu, handle, start, lines, bits, at, usage, hdc))
}

/// SetDIBitsToDevice(hdc, xdst, ydst, width, height, xsrc, ysrc, start,
/// lines, bits, info, usage): the scan lines it drew.
pub fn set_di_bits_to_device(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [hdc, xd, yd, w, h, xs, ys, start, lines, bits, at, usage] = args.try_into().unwrap();
    let Some(dc) = gdi(cpu).dc(hdc).cloned() else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    };
    let Some(info) = read_info(cpu, at, usage, hdc) else {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    };
    let height = info.abs_height();
    let Some(src) = read_dib(cpu, &info, bits, start, lines) else {
        return Some(0);
    };
    let (w, h) = (w as i32, h as i32);
    let (xs, ys) = (xs as i32, ys as i32);
    // The source rectangle in the rows of `src`: a bottom-up DIB's
    // coordinates start at its bottom, and `src` has the scan lines from
    // `start` on, the last first.
    let (top, rows) = if info.top_down() {
        (ys - start as i32, src.height)
    } else {
        // Scan line s is row (start + n - 1 - s) of `src`.
        let n = src.height;
        let top_scan = ys + h - 1;
        (start as i32 + n - 1 - top_scan, n)
    };
    let _ = (height, rows);
    let (x0, y0) = dc.to_device(xd as i32, yd as i32);
    let source = Source {
        surface: src,
        bk_color: 0xFFFFFF,
        origin: (0, 0),
    };
    draw::blit_from(
        cpu,
        hdc,
        (x0, y0, x0 + w, y0 + h),
        Some((source, (xs, top, xs + w, top + h))),
        0x00CC_0020,
    );
    Some(lines.min(height as u32))
}

/// StretchDIBits(hdc, xdst, ydst, wdst, hdst, xsrc, ysrc, wsrc, hsrc,
/// bits, info, usage, rop): the scan lines of the DIB.
pub fn stretch_di_bits(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [hdc, xd, yd, wd, hd, xs, ys, ws, hs, bits, at, usage, rop] = args.try_into().unwrap();
    let Some(dc) = gdi(cpu).dc(hdc).cloned() else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    };
    let Some(info) = read_info(cpu, at, usage, hdc) else {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    };
    let height = info.abs_height();
    let source = if draw::uses_source((rop >> 16) as u8) {
        let Some(src) = read_dib(cpu, &info, bits, 0, height as u32) else {
            return Some(0);
        };
        let (xs, ys, ws, hs) = (xs as i32, ys as i32, ws as i32, hs as i32);
        // The source's coordinates start at the DIB's bottom, even for a
        // DIB whose rows run top down.
        let top = height - ys - hs;
        Some((
            Source {
                surface: src,
                bk_color: 0xFFFFFF,
                origin: (0, 0),
            },
            (xs, top, xs + ws, top + hs),
        ))
    } else {
        None
    };
    let (x0, y0) = dc.to_device(xd as i32, yd as i32);
    let (x1, y1) = dc.to_device(xd as i32 + wd as i32, yd as i32 + hd as i32);
    draw::blit_from(cpu, hdc, (x0, y0, x1, y1), source, rop);
    Some(height as u32)
}

/// GetDIBColorTable(hdc, start, count, colors): the color table of the
/// DIB section selected into a memory DC.
pub fn get_dib_color_table(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, start, count, out) = (args[0], args[1] as usize, args[2] as usize, args[3]);
    let Some(colors) = selected_table(cpu, hdc) else {
        return Some(0);
    };
    let end = (start + count).min(colors.len());
    if start >= end {
        return Some(0);
    }
    let quads: Vec<u32> = colors[start..end]
        .iter()
        .map(|&c| color_to_quad(c))
        .collect();
    write_words(cpu, out, &quads);
    Some((end - start) as u32)
}

/// SetDIBColorTable(hdc, start, count, colors).
pub fn set_dib_color_table(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, start, count, at) = (args[0], args[1] as usize, args[2] as usize, args[3]);
    let Some(colors) = selected_table(cpu, hdc) else {
        return Some(0);
    };
    let end = (start + count).min(colors.len());
    if start >= end {
        return Some(0);
    }
    let new = read_words(cpu, at, end - start);
    let bitmap = gdi(cpu).dc(hdc).map_or(0, |d| d.attrs.bitmap);
    if let Some(b) = gdi(cpu).bitmap_mut(bitmap)
        && let Some(d) = &mut b.dib
    {
        for (i, q) in new.into_iter().enumerate() {
            d.colors[start + i] = quad_to_color(q);
        }
    }
    Some((end - start) as u32)
}

/// The color table of the DIB section selected into the DC `hdc`.
fn selected_table(cpu: &mut Cpu, hdc: u32) -> Option<Vec<u32>> {
    let Some(dc) = gdi(cpu).dc(hdc).cloned() else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return None;
    };
    let b = gdi(cpu).bitmap(dc.attrs.bitmap)?;
    let d = b.dib.as_ref()?;
    (b.bpp <= 8).then(|| d.colors.clone())
}

/// GetBitmapDimensionEx(bitmap, size).
pub fn get_bitmap_dimension_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(d) = gdi(cpu).bitmap(args[0]).map(|b| b.dimension) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    };
    write_words(cpu, args[1], &[d.0 as u32, d.1 as u32]);
    Some(TRUE)
}

/// SetBitmapDimensionEx(bitmap, width, height, old).
pub fn set_bitmap_dimension_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let new = (args[1] as i32, args[2] as i32);
    let Some(b) = gdi(cpu).bitmap_mut(args[0]) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    };
    let old = std::mem::replace(&mut b.dimension, new);
    if args[3] != 0 {
        write_words(cpu, args[3], &[old.0 as u32, old.1 as u32]);
    }
    Some(TRUE)
}

/// A pattern brush's pattern from the bitmap `handle`: at most 8x8 on
/// Windows 95, which uses the bitmap's top-left corner.
pub fn pattern_of(cpu: &mut Cpu, handle: u32) -> Option<Pattern> {
    let b = gdi(cpu).bitmap(handle)?.clone_header();
    let s = bitmap_surface(cpu, handle)?;
    let (w, h) = (b.width.min(8), b.height.min(8));
    let mono = s.mono;
    let mut pixels = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let v = s.get(x, y);
            pixels.push(if mono { v } else { s.color_of(v) });
        }
    }
    Some(Pattern {
        width: w.max(1),
        height: h.max(1),
        pixels,
        mono,
    })
}

/// A DIB pattern brush's pattern from the packed DIB at `at`.
pub fn pattern_of_packed_dib(cpu: &mut Cpu, at: u32, usage: u32) -> Option<Pattern> {
    let info = read_info(cpu, at, usage, 0)?;
    let bits = at + info.header_size + table_bytes(&info, usage);
    let s = read_dib(cpu, &info, bits, 0, info.abs_height() as u32)?;
    let (w, h) = (info.width.min(8), info.abs_height().min(8));
    let mut pixels = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        // The pattern's rows from the image's top.
        let row = s.height - h + y;
        for x in 0..w {
            pixels.push(s.color_of(s.get(x, row.max(0))));
        }
    }
    Some(Pattern {
        width: w.max(1),
        height: h.max(1),
        pixels,
        mono: false,
    })
}
