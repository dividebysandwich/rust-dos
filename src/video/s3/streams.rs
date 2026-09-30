//! The S3 ViRGE's streams processor (MMIO 8180h-81FFh): the primary
//! stream, whose frame buffer addresses S3D Toolkit programs flip between
//! in full streams mode (CR67 bits 3-2), and the secondary stream, the
//! scaled overlay DirectDraw and video players show YUV or RGB pictures
//! in. The registers read back as written, and are decoded where they are
//! used.
//!
//! The register layout follows DOSBox-X's vga_xga.cpp (8180h-81FCh) and
//! its compositing in vga_draw.cpp.

#[derive(Clone, Debug, Default)]
pub struct Streams {
    /// 8180h-81FCh, a doubleword each.
    pub raw: [u32; 0x20],
}

crate::state_fields!(Streams { raw });

impl Streams {
    fn reg(&self, port: u16) -> u32 {
        self.raw[((port - 0x8180) >> 2) as usize]
    }

    /// Write `len` bytes of `val` at `port`.
    pub fn write(&mut self, port: u16, val: u32, len: u8) {
        let raw = &mut self.raw[((port - 0x8180) >> 2) as usize];
        let sh = (port as u32 & 3) * 8;
        let msk = (if len >= 4 { 0xFFFF_FFFFu32 } else { (1u32 << (len as u32 * 8)) - 1 }) << sh;
        *raw = (*raw & !msk) | ((val << sh) & msk);
    }

    pub fn read(&self, port: u16, len: u8) -> u32 {
        let v = self.reg(port & !3) >> ((port & 3) * 8);
        if len < 4 { v & ((1u32 << (len as u32 * 8)) - 1) } else { v }
    }

    /// The primary stream's frame buffer address, of the two (81C0h,
    /// 81C4h) the buffer select (81CCh bit 0) picks.
    pub fn primary_address(&self) -> u32 {
        let select = self.reg(0x81CC) & 1;
        self.reg(if select == 0 { 0x81C0 } else { 0x81C4 }) & 0x3F_FFFF
    }

    /// The primary stream's stride in bytes (81C8h).
    pub fn primary_stride(&self) -> u32 {
        self.reg(0x81C8) & 0x1FFF
    }

    /// Whether the primary stream is shown from its own frame buffer
    /// address rather than the CRTC's start: full streams mode, CR67 bits
    /// 3-2 = 11b.
    pub fn full(cr67: u8) -> bool {
        cr67 >> 2 & 3 == 3
    }

    /// The secondary stream (the overlay), if it is shown: its window, its
    /// picture and how it goes over the primary stream. `vx` for the
    /// ViRGE/VX's 12-bit horizontal scale.
    pub fn overlay(&self, vx: bool) -> Option<Overlay> {
        let start = self.reg(0x81F8);
        let size = self.reg(0x81FC);
        let (sx, sy) = (start >> 16 & 0x3FF, start & 0x3FF);
        let height = size & 0x3FF;
        // Compose mode (81A0h bits 26-24): 0 the overlay opaque over the
        // primary stream, 5 where the primary stream has the colour key.
        // Windows 3.1's Trio64V+ driver puts a closed overlay at the top
        // left corner; DOSBox-X shows none at x or y 0.
        let compose = self.reg(0x81A0) >> 24 & 7;
        if height == 0 || sx == 0 || sy == 0 || !matches!(compose, 0 | 5) {
            return None;
        }
        let signed = |v: u32, bits: u32| ((v << (32 - bits)) as i32) >> (32 - bits);
        let hmask = if vx { 0xFFF } else { 0x7FF };
        let hbits = if vx { 12 } else { 11 };
        let hscale = self.reg(0x8198);
        let control = self.reg(0x8190);
        let key = self.reg(0x8184);
        let select = if self.reg(0x81CC) >> 1 & 3 == 1 { 0x81D4 } else { 0x81D0 };
        // The coordinates written are one more, the width one less.
        let (x, y) = (sx - 1, sy - 1);
        Some(Overlay {
            x,
            y,
            end_x: x + (size >> 16 & 0x3FF) + 1,
            end_y: y + height,
            format: control >> 24 & 7,
            address: self.reg(select) & 0x3F_FFF8,
            stride: self.reg(0x81D8) & 0x1FFF,
            haccum: signed(control & 0xFFF, 12),
            k1h: (hscale & hmask) as i32,
            k2h: signed(hscale >> 16 & hmask, hbits),
            vaccum: signed(self.reg(0x81E8) & 0xFFF, 12),
            k1v: (self.reg(0x81E0) & 0x7FF) as i32,
            k2v: signed(self.reg(0x81E4) & 0x7FF, 11),
            keyed: compose == 5,
            key: key & 0xFF_FFFF,
            key_mask: (0xFFu32 << (7 - (key >> 24 & 7))) & 0xFF,
        })
    }
}

/// The secondary stream as it is to be drawn: its window on the screen
/// (`x` to `end_x`, `y` to `end_y`), its pixels, and the DDAs that scale
/// them, as DOSBox-X draws them (vga_draw.cpp).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Overlay {
    pub x: u32,
    pub y: u32,
    pub end_x: u32,
    pub end_y: u32,
    /// The pixel format (8190h bits 26-24): 3 RGB 1555, 5 RGB 565, 6 RGB
    /// 24, 7 XRGB 32, the others YUY2.
    pub format: u32,
    pub address: u32,
    pub stride: u32,
    /// The horizontal DDA: its start and the K1 and K2 terms.
    pub haccum: i32,
    pub k1h: i32,
    pub k2h: i32,
    /// The vertical one.
    pub vaccum: i32,
    pub k1v: i32,
    pub k2v: i32,
    /// Over the primary stream's pixels of the colour key only.
    pub keyed: bool,
    pub key: u32,
    /// The bits of each colour component compared.
    pub key_mask: u32,
}

impl Overlay {
    fn rgb(&self) -> bool {
        self.format == 3 || self.format >= 5
    }

    /// The video memory address of each of the window's rows' pictures,
    /// from the vertical DDA.
    pub fn rows(&self) -> Vec<u32> {
        let mut rows = Vec::with_capacity((self.end_y - self.y) as usize);
        let (mut addr, mut acc) = (self.address, self.vaccum);
        for _ in self.y..self.end_y {
            rows.push(addr);
            if self.k1v == 0 && self.k2v == 0 {
                addr = addr.wrapping_add(self.stride);
            } else {
                acc += self.k1v;
                if acc >= 0 {
                    acc += self.k2v - self.k1v;
                    addr = addr.wrapping_add(self.stride);
                }
            }
        }
        rows
    }

    /// Draw the overlay's row starting at `addr` over `row`, a row of
    /// `width` pixels of RGB (the primary stream's, to be keyed against).
    /// `six_bit` for a primary stream through a 6-bit DAC, whose colours
    /// can't match a key's low two bits.
    pub fn draw_row(&self, addr: u32, vram: &[u8], row: &mut [u8], width: u32, six_bit: bool) {
        let at = |a: u32| vram[a as usize & (vram.len() - 1)] as u32;
        let mut mask = self.key_mask * 0x01_0101;
        if six_bit {
            mask &= 0xFC_FCFC;
        }
        let key = self.key & mask;
        let bytes = [2, 2, 2, 2, 2, 2, 3, 4][self.format as usize];
        let (mut src, mut acc) = (addr, self.haccum);
        for x in self.x..self.end_x.min(width) {
            let i = x as usize * 3;
            let (r, g, b) = if self.rgb() {
                match self.format {
                    3 => {
                        let p = at(src) | at(src + 1) << 8;
                        let five = |v: u32| (v << 3 | v >> 2) as u8;
                        (five(p >> 10 & 31), five(p >> 5 & 31), five(p & 31))
                    }
                    5 => {
                        let p = at(src) | at(src + 1) << 8;
                        let (r, g, b) = (p >> 11 & 31, p >> 5 & 63, p & 31);
                        ((r << 3 | r >> 2) as u8, (g << 2 | g >> 4) as u8, (b << 3 | b >> 2) as u8)
                    }
                    _ => (at(src + 2) as u8, at(src + 1) as u8, at(src) as u8),
                }
            } else {
                // YUY2: Y U Y V; each pair of pixels shares U and V.
                yuv(at(src) as u8, at((src & !3) + 1) as u8, at((src & !3) + 3) as u8)
            };
            let primary = (row[i] as u32) << 16 | (row[i + 1] as u32) << 8 | row[i + 2] as u32;
            if !self.keyed || primary & mask == key {
                row[i] = r;
                row[i + 1] = g;
                row[i + 2] = b;
            }
            let step = if self.rgb() { bytes } else { 2 };
            if self.rgb() && self.k1h == 0 && self.k2h == 0 {
                src = src.wrapping_add(step);
            } else {
                acc += self.k1h;
                if acc >= 0 {
                    acc += self.k2h - self.k1h;
                    src = src.wrapping_add(step);
                }
            }
        }
    }
}

/// MPEG-range YUV (BT.601) as RGB.
fn yuv(y: u8, u: u8, v: u8) -> (u8, u8, u8) {
    let (y, u, v) = (298 * (y as i32 - 16), u as i32 - 128, v as i32 - 128);
    let c = |x: i32| (x >> 8).clamp(0, 255) as u8;
    (c(y + 409 * v), c(y - 208 * v - 100 * u), c(y + 516 * u))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn streams(regs: &[(u16, u32)]) -> Streams {
        let mut s = Streams::default();
        for &(port, value) in regs {
            s.write(port, value, 4);
        }
        s
    }

    /// A 4x2 RGB 565 overlay at (10, 5), at 0x1000 with a stride of 8.
    fn rgb_overlay(compose: u32) -> Streams {
        streams(&[
            (0x8190, 5 << 24),
            (0x81A0, compose << 24),
            (0x81D0, 0x1000),
            (0x81D8, 8),
            (0x81F8, 11 << 16 | 6),
            (0x81FC, 3 << 16 | 2),
        ])
    }

    #[test]
    fn the_registers_read_back_and_the_primary_address_follows_the_select() {
        let mut s = streams(&[(0x81C0, 0x1000), (0x81C4, 0x9_6000)]);
        assert_eq!(s.read(0x81C4, 4), 0x9_6000);
        assert_eq!(s.read(0x81C5, 1), 0x60);
        assert_eq!(s.primary_address(), 0x1000);
        s.write(0x81CC, 1, 1);
        assert_eq!(s.primary_address(), 0x9_6000);
        assert!(Streams::full(0x0C) && !Streams::full(0x08));
    }

    #[test]
    fn an_overlay_needs_a_window_and_a_compose_mode() {
        assert_eq!(Streams::default().overlay(false), None);
        let o = rgb_overlay(0).overlay(false).unwrap();
        assert_eq!((o.x, o.y, o.end_x, o.end_y), (10, 5, 14, 7));
        assert_eq!(o.rows(), [0x1000, 0x1008]);
        // Blended compose modes aren't drawn.
        assert_eq!(rgb_overlay(2).overlay(false), None);
    }

    #[test]
    fn the_vertical_dda_repeats_rows_to_scale_up() {
        // Twice the height: K1 = 1 (2 source rows less one), K2 = 2 - 4,
        // from -2.
        let mut s = rgb_overlay(0);
        s.write(0x81FC, 3 << 16 | 4, 4);
        s.write(0x81E0, 1, 4);
        s.write(0x81E4, (-2i32 as u32) & 0x7FF, 4);
        s.write(0x81E8, (-2i32 as u32) & 0xFFF, 4);
        let rows = s.overlay(false).unwrap().rows();
        assert_eq!(rows, [0x1000, 0x1000, 0x1008, 0x1008]);
    }

    #[test]
    fn rgb_pixels_go_over_the_primary_stream_where_it_has_the_key() {
        let mut vram = vec![0u8; 0x2000];
        // Red, green, blue, white in 565.
        for (i, p) in [0xF800u16, 0x07E0, 0x001F, 0xFFFF].iter().enumerate() {
            vram[0x1000 + i * 2..][..2].copy_from_slice(&p.to_le_bytes());
        }
        let o = rgb_overlay(0).overlay(false).unwrap();
        let mut row = vec![0u8; 20 * 3];
        o.draw_row(0x1000, &vram, &mut row, 20, false);
        assert_eq!(&row[30..42], [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255]);
        assert_eq!(&row[27..30], [0, 0, 0]);
        // Keyed on magenta: only where the primary stream has it.
        let mut s = rgb_overlay(5);
        s.write(0x8184, 7 << 24 | 0xFF00FF, 4);
        let o = s.overlay(false).unwrap();
        let mut row = vec![0u8; 20 * 3];
        row[33..36].copy_from_slice(&[255, 0, 255]);
        o.draw_row(0x1000, &vram, &mut row, 20, false);
        assert_eq!(&row[30..39], [0, 0, 0, 0, 255, 0, 0, 0, 0]);
    }

    #[test]
    fn yuy2_pixels_share_their_colour() {
        let mut vram = vec![0u8; 0x2000];
        // White then black luma, no colour.
        vram[0x1000..0x1004].copy_from_slice(&[235, 128, 16, 128]);
        let o = streams(&[(0x8190, 1 << 24), (0x81D0, 0x1000), (0x81F8, 1 << 16 | 1), (0x81FC, 1 << 16 | 1)])
            .overlay(false)
            .unwrap();
        let mut row = vec![0u8; 4 * 3];
        o.draw_row(0x1000, &vram, &mut row, 4, false);
        // 235 is white, 16 black (as DOSBox-X converts them).
        assert_eq!(&row[0..6], [254, 254, 254, 0, 0, 0]);
    }
}
