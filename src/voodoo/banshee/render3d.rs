//! The 3D engine's registers in the Banshee's and Voodoo3's map: the
//! Voodoo2's triangle and mode registers, with what the card does its
//! own way. Its buffers are placed by address and stride (colBufferAddr,
//! auxBufferAddr and their strides) instead of the fbiInit registers,
//! linear or in tiles of 128 bytes by 32 rows; the Y origin at the
//! bottom counts from miscInit0's; a swap shows leftOverlayBuf through
//! the video processor's overlay, at once or at a vertical retrace,
//! while the command FIFOs wait for it; swapPending counts the swaps the
//! status register reports; texBaseAddr is a byte address; and the 3D
//! linear frame buffer has rows of 2048 pixels.
//!
//! From 86Box's vid_voodoo_reg.c, vid_voodoo_render.c, vid_voodoo_fb.c
//! and vid_voodoo_banshee.c (Sarah Walker, GPLv2) and sections 5 and 8
//! of 3dfx's Voodoo3 specification.

use super::super::raster::Stride;
use super::super::regs::*;
use super::super::{Effect, NONE, Now, PendingSwap, Voodoo};

// The card's registers where a Voodoo2 has its command FIFO's and some
// of its init registers (dword numbers in the 3D space).
pub const COL_BUFFER_ADDR: usize = 0x1EC / 4;
pub const COL_BUFFER_STRIDE: usize = 0x1F0 / 4;
pub const AUX_BUFFER_ADDR: usize = 0x1F4 / 4;
pub const AUX_BUFFER_STRIDE: usize = 0x1F8 / 4;
pub const CLIP_LEFT_RIGHT1: usize = 0x200 / 4;
pub const CLIP_TOP_BOTTOM1: usize = 0x204 / 4;
pub const SWAP_PENDING: usize = 0x24C / 4;
pub const LEFT_OVERLAY_BUF: usize = 0x250 / 4;
pub const RIGHT_OVERLAY_BUF: usize = 0x254 / 4;

/// The registers the card keeps as written and reads back.
const KEPT: [usize; 8] = [
    COL_BUFFER_ADDR,
    COL_BUFFER_STRIDE,
    AUX_BUFFER_ADDR,
    AUX_BUFFER_STRIDE,
    CLIP_LEFT_RIGHT1,
    CLIP_TOP_BOTTOM1,
    LEFT_OVERLAY_BUF,
    RIGHT_OVERLAY_BUF,
];

/// A buffer's stride register: bytes a row when linear (bit 15 clear),
/// tiles a row of tiles when tiled.
fn stride(value: u32) -> Stride {
    if value & (1 << 15) != 0 {
        Stride {
            words: (value & 0x7F) as usize * 2048,
            tiled: true,
        }
    } else {
        Stride::linear((value & 0x3FFF) as usize / 2)
    }
}

impl Voodoo {
    /// A 3D register read through BAR0 (200000h-5FFFFFh).
    pub(crate) fn banshee_3d_read(&self, offset: u32, now: Now) -> u32 {
        match ((offset >> 2) & 0xFF) as usize {
            STATUS => self.banshee_status(now),
            INTR_CTRL => self.banshee.as_ref().map_or(0, |b| b.intr_ctrl),
            regnum if KEPT.contains(&regnum) => self.reg[regnum],
            // The Voodoo2's video and init registers aren't there.
            0x80..=0x96 => 0,
            regnum => self.register_read(regnum as u32, now),
        }
    }

    /// A 3D register written through BAR0. swapPending counts at once;
    /// the rest waits behind a swap that waits for the retrace, as in the
    /// card's FIFO.
    pub(crate) fn banshee_3d_write(
        &mut self,
        offset: u32,
        data: u32,
        now: Now,
        effect: &mut Effect,
    ) {
        match ((offset >> 2) & 0xFF) as usize {
            SWAP_PENDING => {
                if let Some(b) = &mut self.banshee {
                    b.swap_count += 1;
                }
            }
            INTR_CTRL => {
                if let Some(b) = &mut self.banshee {
                    b.intr_ctrl = data & 0x0030_003F;
                }
            }
            _ => {
                self.banshee_3d_register(offset & 0x3FFC, data, now, effect);
                self.fifo_write(now, effect);
            }
        }
    }

    /// A 3D register written, from BAR0 or a command FIFO: byte address
    /// `addr`, with the chips it goes to in bits 13:10.
    pub(crate) fn banshee_3d_register(
        &mut self,
        addr: u32,
        data: u32,
        now: Now,
        effect: &mut Effect,
    ) {
        let mut chips = (addr >> 10) & 0xF;
        if chips == 0 {
            chips = 0xF;
        }
        chips &= self.chipmask;
        let regnum = ((addr >> 2) & 0xFF) as usize;
        match regnum {
            COL_BUFFER_ADDR | COL_BUFFER_STRIDE | AUX_BUFFER_ADDR | AUX_BUFFER_STRIDE => {
                self.reg[regnum] = data;
                self.banshee_buffers();
            }
            CLIP_LEFT_RIGHT1 | CLIP_TOP_BOTTOM1 | LEFT_OVERLAY_BUF | RIGHT_OVERLAY_BUF => {
                self.reg[regnum] = data;
            }
            SWAPBUFFER_CMD => self.banshee_swap(data, now, effect),
            // The Voodoo2's video and init registers aren't there.
            0x80..=0x96 => {}
            _ => {
                if self.chip.access[regnum] & WRITE != 0 {
                    self.register_to(regnum, chips, data, now, effect);
                }
            }
        }
    }

    /// Where the 3D engine draws, from colBufferAddr, auxBufferAddr and
    /// their strides: the colour buffer is the same whichever fbzMode
    /// and lfbMode pick.
    pub(crate) fn banshee_buffers(&mut self) {
        let col = self.reg[COL_BUFFER_ADDR] & 0xFF_FFF0;
        let aux = self.reg[AUX_BUFFER_ADDR] & 0xFF_FFF0;
        let (col_stride, aux_stride) = (
            stride(self.reg[COL_BUFFER_STRIDE]),
            stride(self.reg[AUX_BUFFER_STRIDE]),
        );
        let fbi = &mut self.fbi;
        fbi.rgboffs = [col, col, NONE];
        fbi.auxoffs = aux;
        fbi.frontbuf = 0;
        fbi.backbuf = 1;
        fbi.stride = col_stride;
        fbi.aux_stride = aux_stride;
        fbi.rowpixels = col_stride.pixels() as u32;
        fbi.ymask = -1;
        fbi.fifo_size = 0;
        // Rows below the buffer are other memory, not other rows.
        fbi.width = fbi.rowpixels;
        fbi.height = 4096;
        self.update_tiles();
    }

    /// swapbufferCMD: the overlay shows leftOverlayBuf, now or with bit 0
    /// at the retrace `bits 8:1` retraces after the last swap, and the
    /// swap counts off swapPending's count then (86Box's
    /// `voodoo_reg_writel`). The address is taken now, so a program can
    /// set the next one while the swap waits.
    fn banshee_swap(&mut self, data: u32, now: Now, effect: &mut Effect) {
        let addr = self.reg[LEFT_OVERLAY_BUF] & 0xFF_FFFF;
        let current = self.retraces(now.ns);
        if data & 1 == 0 {
            self.last_swap = current;
            self.banshee_show(addr);
            return;
        }
        let interval = ((data >> 1) & 0xFF) as u64;
        let after = self.swaps.back().map_or(self.last_swap, |s| s.retrace);
        let retrace = (current + 1).max(after + interval.max(1));
        let due = self.retrace_ticks(retrace);
        self.swaps.push_back(PendingSwap { retrace, due });
        if let Some(b) = &mut self.banshee {
            b.overlay_queue.push_back(addr);
        }
        effect.reschedule = true;
    }

    /// A swap happened: the overlay shows `addr`.
    fn banshee_show(&mut self, addr: u32) {
        if let Some(b) = &mut self.banshee {
            b.overlay_addr = addr;
            b.swap_count = b.swap_count.saturating_sub(1);
            b.dirty = true;
        }
        self.swapped += 1;
        self.display_dirty = true;
    }

    /// `retired` swaps happened: their buffers shown, and the command
    /// FIFOs that waited for them go on.
    pub(crate) fn banshee_swaps_done(&mut self, retired: usize, now: Now) {
        for _ in 0..retired {
            let addr = self
                .banshee
                .as_mut()
                .and_then(|b| b.overlay_queue.pop_front());
            if let Some(addr) = addr {
                self.banshee_show(addr);
            }
        }
        if retired > 0 && self.swaps.is_empty() {
            let mut effect = Effect::default();
            for n in 0..2 {
                self.run_banshee_fifo(n, now, &mut effect);
            }
        }
    }

    /// Whether the command FIFOs wait for a swap's retrace.
    pub(crate) fn banshee_fifo_waits(&self) -> bool {
        !self.swaps.is_empty()
    }

    /// TMU 0's texture space from 600000h: texture memory from its
    /// texBaseAddr on, linear (86Box's `voodoo_tex_writel`).
    pub(crate) fn banshee_texture_write(&mut self, offset: u32, data: u32) {
        if self.tmu_in_use.get() != 0 || self.writes_queue() {
            self.flush();
        }
        let base = self.reg[0x100 + TEX_BASE_ADDR] & 0xFF_FFF0;
        let addr = base.wrapping_add(offset & 0x1F_FFFC);
        super::blit2d::write(&self.fbi.ram, addr, 4, data, self.fbi.mask);
    }

    /// For the debugger: where the 3D engine draws and what it shows.
    pub(crate) fn banshee_3d_describe(&self) -> serde_json::Value {
        let fbi = &self.fbi;
        let describe = |s: Stride| {
            if s.tiled {
                format!("{} tiles", s.words / 2048)
            } else {
                format!("{} bytes", s.words * 2)
            }
        };
        serde_json::json!({
            "color": format!("{:06X}", fbi.rgboffs[0]),
            "color_stride": describe(fbi.stride),
            "aux": format!("{:06X}", fbi.auxoffs),
            "aux_stride": describe(fbi.aux_stride),
            "y_origin": fbi.yorigin,
            "left_overlay": format!("{:06X}", self.reg[LEFT_OVERLAY_BUF] & 0xFF_FFFF),
            "swap_count": self.banshee.as_ref().map_or(0, |b| b.swap_count),
            "swaps": self.swapped,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::Board;
    use super::super::{MISC_INIT0, VID_CURR_OVERLAY_START_ADDR};
    use super::*;

    fn card() -> Voodoo {
        Voodoo::with_workers(Board::Voodoo3, 0)
    }

    fn reg3d(v: &mut Voodoo, regnum: usize, data: u32) {
        v.write(
            0x20_0000 + 4 * regnum as u32,
            data,
            0xFFFF_FFFF,
            Now::default(),
        );
    }

    fn float(v: &mut Voodoo, regnum: usize, value: f32) {
        reg3d(v, regnum, value.to_bits());
    }

    /// A flat-shaded rectangle of two triangles, `x0..x1` by `y0..y1`, in
    /// colour `rgb`, the iterated colour.
    fn rectangle(v: &mut Voodoo, (x0, y0, x1, y1): (f32, f32, f32, f32), rgb: (f32, f32, f32)) {
        for (a, b, c) in [
            ((x0, y0), (x1, y0), (x1, y1)),
            ((x0, y0), (x1, y1), (x0, y1)),
        ] {
            float(v, FVERTEX_AX, a.0);
            float(v, FVERTEX_AY, a.1);
            float(v, FVERTEX_BX, b.0);
            float(v, FVERTEX_BY, b.1);
            float(v, FVERTEX_CX, c.0);
            float(v, FVERTEX_CY, c.1);
            float(v, FSTART_R, rgb.0);
            float(v, FSTART_G, rgb.1);
            float(v, FSTART_B, rgb.2);
            reg3d(v, FTRIANGLE_CMD, 0);
        }
    }

    #[test]
    fn triangles_draw_into_a_tiled_colour_buffer() {
        let mut v = card();
        // The colour buffer at 1 MB, 16 tiles (1024 pixels) a row of tiles.
        reg3d(&mut v, COL_BUFFER_ADDR, 0x10_0000);
        reg3d(&mut v, COL_BUFFER_STRIDE, 1 << 15 | 16);
        // RGB writes, no clipping or depth.
        reg3d(&mut v, FBZ_MODE, 1 << 9);
        rectangle(&mut v, (60.0, 30.0, 70.0, 40.0), (255.0, 0.0, 0.0));
        v.flush();
        // Pixel (65, 33): tile 1 of the second row of tiles, row 1 in it.
        let band = 0x10_0000 / 2 + 16 * 2048;
        assert_eq!(v.fbi.ram.get(band + 2048 + 64 + 1), 0xF800);
        // Pixel (59, 33) is left of the rectangle.
        assert_eq!(v.fbi.ram.get(band + 64 + 59), 0);
        assert_eq!(v.reg[FBI_TRIANGLES_OUT], 2);
    }

    #[test]
    fn a_fastfill_of_a_linear_buffer_and_tiled_depth() {
        let mut v = card();
        reg3d(&mut v, COL_BUFFER_ADDR, 0x20_0000);
        reg3d(&mut v, COL_BUFFER_STRIDE, 1280);
        reg3d(&mut v, AUX_BUFFER_ADDR, 0x30_0000);
        reg3d(&mut v, AUX_BUFFER_STRIDE, 1 << 15 | 10);
        reg3d(&mut v, CLIP_LEFT_RIGHT, 2 << 16 | 4);
        reg3d(&mut v, CLIP_LOW_Y_HIGH_Y, 40 << 16 | 41);
        reg3d(&mut v, COLOR1, 0x00_00FF);
        reg3d(&mut v, ZA_COLOR, 0x1234);
        reg3d(&mut v, FBZ_MODE, 1 << 9 | 1 << 10);
        reg3d(&mut v, FASTFILL_CMD, 0);
        v.flush();
        let ram = &v.fbi.ram;
        assert_eq!(ram.get(0x20_0000 / 2 + 40 * 640 + 3), 0x001F);
        assert_eq!(ram.get(0x20_0000 / 2 + 40 * 640 + 4), 0);
        // Row 40 of the depth is row 8 of the second row of tiles.
        assert_eq!(ram.get(0x30_0000 / 2 + 10 * 2048 + 8 * 64 + 3), 0x1234);
    }

    #[test]
    fn the_y_origin_comes_from_misc_init0() {
        let mut v = card();
        v.write(MISC_INIT0, 99 << 18, 0xFFFF_FFFF, Now::default());
        assert_eq!(v.fbi.yorigin, 99);
        reg3d(&mut v, COL_BUFFER_ADDR, 0);
        reg3d(&mut v, COL_BUFFER_STRIDE, 2048);
        // The Y origin at the bottom: row 0 is line 99.
        reg3d(&mut v, FBZ_MODE, 1 << 9 | 1 << 17);
        rectangle(&mut v, (0.0, 0.0, 4.0, 1.0), (0.0, 255.0, 0.0));
        v.flush();
        assert_eq!(v.fbi.ram.get(99 * 1024 + 2), 0x07E0);
        assert_eq!(v.fbi.ram.get(2), 0);
    }

    #[test]
    fn lfb_rows_are_2048_pixels() {
        let mut v = card();
        reg3d(&mut v, COL_BUFFER_ADDR, 0x40_0000);
        reg3d(&mut v, COL_BUFFER_STRIDE, 1 << 15 | 8);
        // 16-bit 5-6-5, two pixels a dword: pixels (6, 3) and (7, 3).
        reg3d(&mut v, LFB_MODE, 0);
        let offset = 0x100_0000 + 3 * 4096 + 6 * 2;
        v.write(offset, 0x07E0_F800, 0xFFFF_FFFF, Now::default());
        let row = 0x40_0000 / 2 + 3 * 64;
        assert_eq!(v.fbi.ram.get(row + 6), 0xF800);
        assert_eq!(v.fbi.ram.get(row + 7), 0x07E0);
        assert_eq!(v.read(offset, Now::default()), 0x07E0_F800);
        // 32-bit ARGB, a pixel a dword: pixel (70, 1), in the next tile.
        reg3d(&mut v, LFB_MODE, 5);
        v.write(
            0x100_0000 + 8192 + 70 * 4,
            0xFF00_00FF,
            0xFFFF_FFFF,
            Now::default(),
        );
        assert_eq!(v.fbi.ram.get(0x40_0000 / 2 + 2048 + 64 + 6), 0x001F);
    }

    #[test]
    fn swaps_show_the_left_overlay_buffer() {
        let mut v = card();
        let now = Now::default();
        // A pending swap counted, then a swap at once.
        reg3d(&mut v, LEFT_OVERLAY_BUF, 0x12_3400);
        reg3d(&mut v, SWAP_PENDING, 0);
        assert_eq!(v.banshee_status(now) >> 28, 1);
        reg3d(&mut v, SWAPBUFFER_CMD, 0);
        assert_eq!(v.read(VID_CURR_OVERLAY_START_ADDR, now), 0x12_3400);
        assert_eq!(v.banshee_status(now) >> 28, 0);
        // At a retrace: pending until it comes, with the address of when
        // it was asked for.
        reg3d(&mut v, LEFT_OVERLAY_BUF, 0x20_0000);
        reg3d(&mut v, SWAP_PENDING, 0);
        reg3d(&mut v, SWAPBUFFER_CMD, 1);
        reg3d(&mut v, LEFT_OVERLAY_BUF, 0x30_0000);
        assert_eq!(v.read(VID_CURR_OVERLAY_START_ADDR, now), 0x12_3400);
        assert_eq!(v.banshee_status(now) >> 28, 1);
        let due = v.next_event().expect("the swap waits for a retrace");
        v.service(Now {
            ns: 1_000_000_000,
            ticks: due,
        });
        assert_eq!(v.read(VID_CURR_OVERLAY_START_ADDR, now), 0x20_0000);
        assert_eq!(v.banshee_status(now) >> 28, 0);
    }

    #[test]
    fn texture_base_addresses_are_bytes() {
        let mut v = card();
        reg3d(&mut v, TEX_BASE_ADDR, 0x0050_0010);
        let reg = v.reg.clone();
        v.tmu[0].prepare(&reg[..]);
        assert_eq!(v.tmu[0].lodoffset[0], 0x50_0010);
        // A download from 600000h lands from there on.
        v.write(0x60_0008, 0x1122_3344, 0xFFFF_FFFF, Now::default());
        assert_eq!(v.fbi.ram.get(0x50_0018 / 2), 0x3344);
    }
}
