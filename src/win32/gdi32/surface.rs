//! What GDI draws on: the screen (the picture in video memory, the
//! desktop's or that of the mode DirectDraw set), a bitmap of GDI's
//! own, or a DIB section in the program's memory. A drawing function
//! loads the rows it draws on into a `Surface`, works on their pixels as
//! the bitmap stores them, and stores them back. The screen's rows are
//! 32 bits a pixel here whatever the mode's depth is (16 and 24 bits are
//! converted as they are loaded and stored), but in a mode of 256
//! colours they are its 8-bit pixels, indices into the system palette,
//! as are those of GDI's 8-bit bitmaps (`device`).
//!
//! Colors are COLORREFs (0x00BBGGRR); a pixel's value is what its bits
//! say, an index into the color table for 8 bits a pixel and fewer.

use super::{Rect, gdi};
use crate::cpu::Cpu;

/// What a surface is the pixels of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Screen,
    Bitmap(u32),
    /// Pixels of a DIB a function was given, which aren't stored
    /// anywhere.
    Memory,
}

/// Some rows of a surface's pixels.
#[derive(Clone, Debug)]
pub struct Surface {
    pub target: Target,
    pub width: i32,
    pub height: i32,
    pub bpp: u32,
    pub stride: usize,
    /// The first row loaded, and the rows' bytes.
    pub top: i32,
    pub rows: i32,
    pub data: Vec<u8>,
    /// The color table, for 8 bits a pixel and fewer.
    pub colors: Vec<u32>,
    /// The masks of 16 and 32 bits a pixel: red, green, blue.
    pub masks: [u32; 3],
    /// A monochrome bitmap of GDI's: its pixels are 0 and 1, which blits
    /// from and to color take as the DCs' colors.
    pub mono: bool,
    /// Its pixels are indices into the system palette, which `colors`
    /// is: the screen at 8 bits a pixel, or an 8-bit bitmap of GDI's.
    /// Colors are matched to them through the DC's logical palette.
    pub device: bool,
    /// A DIB given with DIB_PAL_COLORS: the logical palette's index each
    /// entry of its color table was, which `colors` has the colors of.
    pub pal_indices: Option<Vec<u16>>,
    /// Where a DIB section's bits are, and whether its rows run top
    /// down.
    dib: Option<(u32, bool)>,
    changed: bool,
}

/// The size of the screen GDI draws on (`screen::gdi_mode`), and the
/// depth and pitch of its rows as GDI works on them: 32 bits a pixel,
/// or 8 in a mode of 256 colours.
pub fn screen(cpu: &Cpu) -> (i32, i32, u32, usize) {
    let m = screen_mode(cpu);
    let (w, h) = (m.width as i32, m.height as i32);
    if m.bpp == 8 {
        (w, h, 8, w as usize)
    } else {
        (w, h, 32, w as usize * 4)
    }
}

/// The mode of the screen GDI draws on: the desktop's while Glide has
/// the display, where nothing GDI draws shows.
pub fn screen_mode(cpu: &Cpu) -> crate::win32::screen::Mode {
    crate::win32::screen::gdi_mode(cpu).unwrap_or_else(|| crate::win32::screen::desktop(cpu))
}

/// The size and depth of what `target` is, if it is there.
pub fn dimensions(cpu: &Cpu, target: Target) -> Option<(i32, i32, u32)> {
    match target {
        Target::Screen => {
            let (w, h, bpp, _) = screen(cpu);
            Some((w, h, bpp))
        }
        Target::Bitmap(handle) => {
            let b = cpu.bus.win32.gdi.bitmap(handle)?;
            Some((b.width, b.height, b.bpp as u32 * b.planes as u32))
        }
        Target::Memory => None,
    }
}

/// Bytes a row of a DIB: doubleword-aligned.
pub fn dib_stride(width: i32, bpp: u32) -> usize {
    (width.max(0) as usize * bpp as usize).div_ceil(32) * 4
}

/// Bytes a row of a bitmap of GDI's: word-aligned.
pub fn ddb_stride(width: i32, bpp: u32) -> usize {
    (width.max(0) as usize * bpp as usize).div_ceil(16) * 2
}

const MASKS_16: [u32; 3] = [0x7C00, 0x03E0, 0x001F];
const MASKS_32: [u32; 3] = [0xFF_0000, 0x00_FF00, 0x00_00FF];

/// The default masks of a depth.
pub fn default_masks(bpp: u32) -> [u32; 3] {
    if bpp == 16 { MASKS_16 } else { MASKS_32 }
}

/// The masks of a bitmap of GDI's of a depth: the display's format for
/// its own depth (5-6-5 on a 16-bit desktop), else the depth's default.
pub fn ddb_masks(cpu: &Cpu, bpp: u32) -> [u32; 3] {
    if bpp == 16 && crate::win32::screen::desktop(cpu).bpp == 16 {
        [0xF800, 0x07E0, 0x001F]
    } else {
        default_masks(bpp)
    }
}

impl Surface {
    /// A surface of pixels in memory, its rows the top first.
    pub fn memory(width: i32, height: i32, bpp: u32, colors: Vec<u32>, masks: [u32; 3]) -> Surface {
        let stride = dib_stride(width, bpp);
        Surface {
            target: Target::Memory,
            width,
            height,
            bpp,
            stride,
            top: 0,
            rows: height,
            data: vec![0; stride * height.max(0) as usize],
            colors,
            masks,
            mono: false,
            device: false,
            pal_indices: None,
            dib: None,
            changed: false,
        }
    }

    /// Load the rows `y0..y1` (cut to the surface) of `target`. None if
    /// it isn't there or a DIB section's memory isn't.
    pub fn load(cpu: &mut Cpu, target: Target, y0: i32, y1: i32) -> Option<Surface> {
        let (width, height, bpp) = dimensions(cpu, target)?;
        let y0 = y0.clamp(0, height);
        let y1 = y1.clamp(y0, height);
        let rows = y1 - y0;
        match target {
            Target::Memory => None,
            Target::Screen => {
                // 8 or 32 bits a pixel (`screen`).
                let pitch = width as usize * bpp as usize / 8;
                let data = match crate::win32::screen::gdi_mode(cpu) {
                    Some(m) => crate::win32::screen::load_rows(cpu, &m, y0 as u32, rows as u32),
                    None => vec![0; pitch * rows as usize],
                };
                let device = bpp == 8;
                let colors = if device {
                    super::palette::system_colors(cpu)
                } else {
                    Vec::new()
                };
                Some(Surface {
                    target,
                    width,
                    height,
                    bpp,
                    stride: pitch,
                    top: y0,
                    rows,
                    data,
                    colors,
                    masks: MASKS_32,
                    mono: false,
                    device,
                    pal_indices: None,
                    dib: None,
                    changed: false,
                })
            }
            Target::Bitmap(handle) => {
                let b = gdi(cpu).bitmap(handle)?.clone_header();
                let stride = b.stride as usize;
                let mut data = vec![0u8; stride * rows as usize];
                let mut dib = None;
                let (colors, masks, mono) = match &b.dib {
                    Some(d) => {
                        let masks = if d.compression == 3 {
                            d.masks
                        } else {
                            default_masks(bpp)
                        };
                        dib = Some((d.bits, d.top_down));
                        (d.colors.clone(), masks, false)
                    }
                    None if bpp == 1 => (vec![0x000000, 0xFFFFFF], MASKS_32, true),
                    None if bpp == 8 => (super::palette::system_colors(cpu), MASKS_32, false),
                    None => (Vec::new(), ddb_masks(cpu, bpp), false),
                };
                let device = b.dib.is_none() && bpp == 8;
                match dib {
                    Some((bits, top_down)) => {
                        for r in 0..rows {
                            let y = y0 + r;
                            let row = if top_down { y } else { height - 1 - y };
                            let at = bits.wrapping_add(row as u32 * stride as u32);
                            let start = r as usize * stride;
                            cpu.bus
                                .guest_read_bytes(at, &mut data[start..start + stride]);
                        }
                        if cpu.bus.guest_faulted() {
                            return None;
                        }
                    }
                    None => {
                        let bits = &gdi(cpu).bitmap(handle)?.bits;
                        let start = y0 as usize * stride;
                        let len = data.len().min(bits.len().saturating_sub(start));
                        data[..len].copy_from_slice(&bits[start..start + len]);
                    }
                }
                Some(Surface {
                    target,
                    width,
                    height,
                    bpp,
                    stride,
                    top: y0,
                    rows,
                    data,
                    colors,
                    masks,
                    mono,
                    device,
                    pal_indices: None,
                    dib,
                    changed: false,
                })
            }
        }
    }

    /// Load the whole surface.
    pub fn load_all(cpu: &mut Cpu, target: Target) -> Option<Surface> {
        Surface::load(cpu, target, 0, i32::MAX)
    }

    /// Store the rows back, if they were changed.
    pub fn store(&self, cpu: &mut Cpu) {
        if !self.changed || self.rows == 0 {
            return;
        }
        match self.target {
            Target::Memory => {}
            Target::Screen => {
                // In the mode DirectDraw set too; while Glide has the
                // display, nowhere.
                if let Some(m) = crate::win32::screen::gdi_mode(cpu) {
                    crate::win32::screen::store_rows(cpu, &m, self.top as u32, &self.data);
                }
            }
            Target::Bitmap(handle) => match self.dib {
                Some((bits, top_down)) => {
                    for r in 0..self.rows {
                        let y = self.top + r;
                        let row = if top_down { y } else { self.height - 1 - y };
                        let at = bits.wrapping_add(row as u32 * self.stride as u32);
                        let start = r as usize * self.stride;
                        cpu.bus
                            .guest_write_bytes(at, &self.data[start..start + self.stride]);
                    }
                }
                None => {
                    if let Some(b) = gdi(cpu).bitmap_mut(handle) {
                        let start = self.top as usize * self.stride;
                        let len = self.data.len().min(b.bits.len().saturating_sub(start));
                        b.bits[start..start + len].copy_from_slice(&self.data[..len]);
                    }
                }
            },
        }
    }

    /// The rectangle of the surface.
    pub fn rect(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }

    /// Whether the pixel (`x`, `y`) is in the rows loaded.
    #[inline]
    pub fn has(&self, x: i32, y: i32) -> bool {
        x >= 0 && x < self.width && y >= self.top && y < self.top + self.rows
    }

    /// Where row `y` starts in the rows loaded.
    #[inline]
    fn row_offset(&self, y: i32) -> usize {
        (y - self.top) as usize * self.stride
    }

    /// The value of the pixel (`x`, `y`), which must be loaded.
    #[inline]
    pub fn get(&self, x: i32, y: i32) -> u32 {
        let row = self.row_offset(y);
        let x = x as usize;
        let d = &self.data;
        match self.bpp {
            1 => (d[row + x / 8] >> (7 - x % 8)) as u32 & 1,
            4 => {
                let b = d[row + x / 2];
                (if x.is_multiple_of(2) { b >> 4 } else { b & 15 }) as u32
            }
            8 => d[row + x] as u32,
            16 => u16::from_le_bytes([d[row + 2 * x], d[row + 2 * x + 1]]) as u32,
            24 => {
                let i = row + 3 * x;
                d[i] as u32 | (d[i + 1] as u32) << 8 | (d[i + 2] as u32) << 16
            }
            _ => {
                let i = row + 4 * x;
                u32::from_le_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]])
            }
        }
    }

    /// Set the pixel (`x`, `y`), which must be loaded, to `value`.
    #[inline]
    pub fn put(&mut self, x: i32, y: i32, value: u32) {
        let row = self.row_offset(y);
        let x = x as usize;
        self.changed = true;
        let d = &mut self.data;
        match self.bpp {
            1 => {
                let bit = 0x80u8 >> (x % 8);
                if value & 1 != 0 {
                    d[row + x / 8] |= bit;
                } else {
                    d[row + x / 8] &= !bit;
                }
            }
            4 => {
                let b = &mut d[row + x / 2];
                if x.is_multiple_of(2) {
                    *b = (*b & 0x0F) | ((value as u8 & 15) << 4);
                } else {
                    *b = (*b & 0xF0) | (value as u8 & 15);
                }
            }
            8 => d[row + x] = value as u8,
            16 => d[row + 2 * x..row + 2 * x + 2].copy_from_slice(&(value as u16).to_le_bytes()),
            24 => {
                let i = row + 3 * x;
                d[i] = value as u8;
                d[i + 1] = (value >> 8) as u8;
                d[i + 2] = (value >> 16) as u8;
            }
            _ => {
                let i = row + 4 * x;
                d[i..i + 4].copy_from_slice(&value.to_le_bytes());
            }
        }
    }

    /// The mask of a pixel's bits.
    pub fn value_mask(&self) -> u32 {
        if self.bpp >= 32 {
            0xFFFF_FFFF
        } else {
            (1u32 << self.bpp) - 1
        }
    }

    /// The color a pixel's value stands for.
    pub fn color_of(&self, value: u32) -> u32 {
        match self.bpp {
            1 | 4 | 8 => self.colors.get(value as usize).copied().unwrap_or(0),
            _ => {
                let channel = |mask: u32| -> u32 {
                    if mask == 0 {
                        return 0;
                    }
                    let shift = mask.trailing_zeros();
                    let bits = (mask >> shift).count_ones();
                    let v = (value & mask) >> shift;
                    if bits >= 8 {
                        v >> (bits - 8)
                    } else {
                        // Widen to 8 bits by repeating the top bits.
                        let v = v << (8 - bits);
                        v | v >> bits
                    }
                };
                let [r, g, b] = self.masks.map(channel);
                r | g << 8 | b << 16
            }
        }
    }

    /// The value of the color `color` (a COLORREF): for a color table, the
    /// index of the nearest color.
    pub fn value_of(&self, color: u32) -> u32 {
        let (r, g, b) = (color & 0xFF, (color >> 8) & 0xFF, (color >> 16) & 0xFF);
        match self.bpp {
            1 | 4 | 8 => nearest(&self.colors, color),
            _ => {
                let channel = |mask: u32, v: u32| -> u32 {
                    if mask == 0 {
                        return 0;
                    }
                    let shift = mask.trailing_zeros();
                    let bits = (mask >> shift).count_ones();
                    let v = if bits >= 8 {
                        v << (bits - 8)
                    } else {
                        v >> (8 - bits)
                    };
                    (v << shift) & mask
                };
                channel(self.masks[0], r) | channel(self.masks[1], g) | channel(self.masks[2], b)
            }
        }
    }
}

/// The index of the color in `table` nearest to `color`.
pub fn nearest(table: &[u32], color: u32) -> u32 {
    let (r, g, b) = (
        (color & 0xFF) as i32,
        ((color >> 8) & 0xFF) as i32,
        ((color >> 16) & 0xFF) as i32,
    );
    let mut best = (i32::MAX, 0);
    for (i, &c) in table.iter().enumerate() {
        let dr = (c & 0xFF) as i32 - r;
        let dg = ((c >> 8) & 0xFF) as i32 - g;
        let db = ((c >> 16) & 0xFF) as i32 - b;
        let d = dr * dr + dg * dg + db * db;
        if d < best.0 {
            best = (d, i as u32);
            if d == 0 {
                break;
            }
        }
    }
    best.1
}

impl super::Bitmap {
    /// The bitmap without its pixels, for what reads its layout.
    pub fn clone_header(&self) -> super::Bitmap {
        super::Bitmap {
            bits: Vec::new(),
            ..self.clone_shallow()
        }
    }

    fn clone_shallow(&self) -> super::Bitmap {
        super::Bitmap {
            width: self.width,
            height: self.height,
            bpp: self.bpp,
            planes: self.planes,
            stride: self.stride,
            bits: Vec::new(),
            dib: self.dib.clone(),
            dc: self.dc,
            dimension: self.dimension,
        }
    }
}
