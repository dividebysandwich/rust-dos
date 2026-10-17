//! The mach64's video overlay ([RRG] chapter 5): an image in the card's
//! memory, scaled into a window of the screen and shown over the
//! graphics where the graphics key lets it, as DirectDraw's overlays and
//! video players use it.

use super::Mach64;
use super::regs::*;

/// The overlay as its registers set it up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Overlay {
    /// The window on the screen, inclusive.
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
    /// The image: its size, where it is, its pitch in pixels and its
    /// format (VIDEO_FORMAT's: 3 RGB 555, 4 RGB 565, 6 RGB 8888, Bh
    /// YUYV, Ch UYVY).
    width: u32,
    height: u32,
    base: u32,
    pitch: u32,
    format: u32,
    /// The image's steps per screen pixel across and down, 4.12.
    h_step: u32,
    v_step: u32,
    /// OVERLAY_KEY_CNTL, and the graphics key's colour and mask.
    key_cntl: u32,
    key: u32,
    key_mask: u32,
}

impl Overlay {
    /// The overlay, if OVERLAY_SCALE_CNTL has it on and scaling.
    pub fn of(card: &Mach64) -> Option<Overlay> {
        let r = |reg| card.reg(reg);
        if r(OVERLAY_SCALE_CNTL) & 0xC000_0000 != 0xC000_0000 {
            return None;
        }
        let (start, end) = (r(OVERLAY_Y_X_START), r(OVERLAY_Y_X_END));
        let size = r(SCALER_HEIGHT_WIDTH);
        let (base, pitch) = if card.rage2 {
            (
                r(SCALER_BUF0_OFFSET) & 0x3F_FFFF,
                r(SCALER_BUF_PITCH) & 0xFFF,
            )
        } else {
            (r(BUF0_OFFSET) & 0x3F_FFF8, r(BUF0_PITCH) & 0xFFF)
        };
        let overlay = Overlay {
            x0: start >> 16 & 0x7FF,
            y0: start & 0x7FF,
            x1: end >> 16 & 0x7FF,
            y1: end & 0x7FF,
            width: size >> 16 & 0x3FF,
            height: size & 0x3FF,
            base,
            pitch,
            format: r(VIDEO_FORMAT) >> 16 & 0xF,
            h_step: r(OVERLAY_SCALE_INC) >> 16,
            v_step: r(OVERLAY_SCALE_INC) & 0xFFFF,
            key_cntl: r(OVERLAY_KEY_CNTL),
            key: r(OVERLAY_GRAPHICS_KEY_CLR),
            key_mask: r(OVERLAY_GRAPHICS_KEY_MSK),
        };
        (overlay.x0 <= overlay.x1
            && overlay.y0 <= overlay.y1
            && overlay.width > 0
            && overlay.height > 0)
            .then_some(overlay)
    }

    /// Whether screen row `y` has some of the overlay.
    pub fn covers(&self, y: u32) -> bool {
        (self.y0..=self.y1).contains(&y)
    }

    /// Whether the video shows over graphics pixel `pixel` (its raw
    /// value): never with CMP_MIX 1, always with 2, else where the
    /// graphics key's function is true (1 always, 4 the pixel isn't the
    /// key, 5 it is).
    fn shows_over(&self, pixel: u32) -> bool {
        let equal = pixel & self.key_mask == self.key & self.key_mask;
        match self.key_cntl >> 8 & 0xF {
            1 => false,
            2 => true,
            _ => match self.key_cntl >> 4 & 7 {
                1 => true,
                4 => !equal,
                5 => equal,
                _ => false,
            },
        }
    }

    /// Draw the overlay's part of screen row `y` over `row` (RGB, `width`
    /// pixels), the graphics' raw pixels from `graphics`. Point sampled.
    pub fn draw_row(
        &self,
        vram: &[u8],
        y: u32,
        row: &mut [u8],
        width: u32,
        graphics: impl Fn(u32) -> u32,
    ) {
        let byte = |at: u32| vram[at as usize & (vram.len() - 1)] as u32;
        let src_y = (((y - self.y0) * self.v_step) >> 12).min(self.height - 1);
        let bytes = match self.format {
            6 => 4,
            _ => 2,
        };
        let line = self.base + src_y * self.pitch * bytes;
        for x in self.x0..=self.x1.min(width.saturating_sub(1)) {
            if !self.shows_over(graphics(x)) {
                continue;
            }
            let src_x = (((x - self.x0) * self.h_step) >> 12).min(self.width - 1);
            let at = line + src_x * bytes;
            let word = || byte(at) | byte(at + 1) << 8;
            let five = |v: u32| (v << 3 | v >> 2) as u8;
            let (r, g, b) = match self.format {
                3 => {
                    let p = word();
                    (five(p >> 10 & 31), five(p >> 5 & 31), five(p & 31))
                }
                4 => {
                    let p = word();
                    let g = p >> 5 & 63;
                    (five(p >> 11), (g << 2 | g >> 4) as u8, five(p & 31))
                }
                6 => (byte(at + 2) as u8, byte(at + 1) as u8, byte(at) as u8),
                // A pair of pixels shares U and V.
                0xB | 0xC => {
                    let pair = line + (src_x & !1) * 2;
                    let (luma, u, v) = if self.format == 0xB {
                        (byte(pair + (src_x & 1) * 2), byte(pair + 1), byte(pair + 3))
                    } else {
                        (byte(pair + 1 + (src_x & 1) * 2), byte(pair), byte(pair + 2))
                    };
                    yuv(luma as i32, u as i32 - 128, v as i32 - 128)
                }
                _ => (0, 0, 0),
            };
            let i = x as usize * 3;
            if i + 3 <= row.len() {
                row[i..i + 3].copy_from_slice(&[r, g, b]);
            }
        }
    }
}

/// ITU-R BT.601 YUV as RGB.
fn yuv(y: i32, u: i32, v: i32) -> (u8, u8, u8) {
    let c = |v: i32| v.clamp(0, 255) as u8;
    (
        c(y + ((359 * v) >> 8)),
        c(y - ((88 * u + 183 * v) >> 8)),
        c(y + ((454 * u) >> 8)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_overlay_shows_where_the_key_is() {
        let mut card = Mach64::new(false);
        let mut vram = vec![0u8; super::super::MEMORY as usize];
        let mut target = super::super::Target {
            vram: &mut vram,
            dac: &[],
            dac_8bit: false,
        };
        for (reg, value) in [
            (OVERLAY_Y_X_START, 2 << 16),
            (OVERLAY_Y_X_END, 5 << 16 | 1),
            (SCALER_HEIGHT_WIDTH, 2 << 16 | 1),
            (BUF0_OFFSET, 0x1000),
            (BUF0_PITCH, 2),
            (VIDEO_FORMAT, 6 << 16),
            // Half a pixel a step: each image pixel twice.
            (OVERLAY_SCALE_INC, 0x800 << 16 | 0x1000),
            (OVERLAY_GRAPHICS_KEY_CLR, 0x55),
            (OVERLAY_GRAPHICS_KEY_MSK, 0xFF),
            (OVERLAY_KEY_CNTL, 5 << 4),
            (OVERLAY_SCALE_CNTL, 0xC000_0000),
        ] {
            card.load_reg(reg, value, &mut target);
        }
        vram[0x1000..0x1008].copy_from_slice(&[0, 0, 0xFF, 0, 0, 0xFF, 0, 0]);
        let overlay = Overlay::of(&card).unwrap();
        assert!(overlay.covers(1) && !overlay.covers(2));
        let mut row = vec![0u8; 8 * 3];
        // The key everywhere but at x=4.
        overlay.draw_row(&vram, 0, &mut row, 8, |x| if x == 4 { 0 } else { 0x55 });
        let red = |x: usize| row[x * 3];
        let green = |x: usize| row[x * 3 + 1];
        assert_eq!((red(1), red(2), red(3)), (0, 0xFF, 0xFF));
        assert_eq!((green(4), green(5), red(6)), (0, 0xFF, 0));
    }
}
