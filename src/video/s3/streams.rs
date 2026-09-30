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

    /// The secondary stream (the overlay), if it is shown: its window, its
    /// picture and how it goes over the primary stream.
    pub fn overlay(&self) -> Option<Overlay> {
        let start = self.reg(0x81F8);
        let size = self.reg(0x81FC);
        let (x, y) = (start >> 16 & 0x3FF, start & 0x3FF);
        let (width, height) = (size >> 16 & 0x3FF, size & 0x3FF);
        // Compose mode (81A0h bits 26-24): 0 the overlay opaque over the
        // primary stream, 5 where the primary stream has the colour key.
        let compose = self.reg(0x81A0) >> 24 & 7;
        if height == 0 || (x == 0 && y == 0) || !matches!(compose, 0 | 5) {
            return None;
        }
        let control = self.reg(0x8190);
        // K1: the input width less one; K2 (signed, 11 bits here) with it
        // the horizontal scale's DDA.
        let hscale = self.reg(0x8198);
        let k1h = hscale & 0x7FF;
        let k1v = self.reg(0x81E0) & 0x7FF;
        let select = self.reg(0x81CC) >> 1 & 3;
        let key = self.reg(0x8184);
        let key_high = self.reg(0x8194);
        Some(Overlay {
            x,
            y,
            // The window size registers hold the size plus one.
            width: width.saturating_sub(1).max(1),
            height,
            format: control >> 24 & 7,
            address: self.reg(if select & 1 == 0 { 0x81D0 } else { 0x81D4 }) & 0x3F_FFFF,
            stride: self.reg(0x81D8) & 0x1FFF,
            src_width: k1h + 1,
            src_height: k1v + 1,
            keyed: compose == 5,
            key_low: [key >> 16 & 0xFF, key >> 8 & 0xFF, key & 0xFF],
            key_high: [key_high >> 16 & 0xFF, key_high >> 8 & 0xFF, key_high & 0xFF],
            key_bits: (key >> 24 & 7) + 1,
            key_on: key >> 28 & 1 != 0,
        })
    }
}

/// The secondary stream as it is to be drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Overlay {
    /// The window on the screen.
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    /// The pixel format (8190h bits 26-24): 1, 2 YUV 4:2:2, 3 RGB 1555,
    /// 4 YUV, 5 RGB 565, 6 RGB 24, 7 XRGB 32.
    pub format: u32,
    pub address: u32,
    pub stride: u32,
    /// The picture's size before scaling.
    pub src_width: u32,
    pub src_height: u32,
    /// Over the primary stream's pixels that match the colour key only.
    pub keyed: bool,
    pub key_low: [u32; 3],
    pub key_high: [u32; 3],
    /// Bits of each colour compared.
    pub key_bits: u32,
    /// Compare a range (low to high) rather than the low value.
    pub key_on: bool,
}
