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

use super::super::mirror::{LayoutKey, span};
use super::super::raster::Stride;
use super::super::regs::*;
use super::super::{Effect, NONE, Now, PendingSwap, Voodoo};
use super::{VID_DESKTOP_START_ADDR, VID_PROC_CFG, VIDPROC_HALF_MODE, VIDPROC_OVERLAY_ENABLE};

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
            SWAP_PENDING => self.banshee_3d_register(offset & 0x3FFC, data, now, effect),
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
            // From BAR0 or from a command FIFO packet alike.
            SWAP_PENDING => {
                if let Some(b) = &mut self.banshee {
                    b.swap_count += 1;
                }
            }
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

    /// The buffers the OpenGL renderer's recording keeps (`mirror_sync`),
    /// `old` the layout recorded last. A program swaps by moving
    /// colBufferAddr between two or three buffers, so those drawn into
    /// lately stay in the layout beside colBufferAddr's, and a swap
    /// doesn't make the recording start over from memory: up to three,
    /// the oldest dropped first, while they keep their size and stride
    /// and stay apart from colBufferAddr's buffer and the auxiliary one.
    /// The buffers are as wide as their stride and as high as the
    /// desktop, which is as much as the video processor shows of them
    /// unscaled; in half mode with the overlay off, half as high, as it
    /// shows each row of the desktop's buffer twice. The card has no
    /// register for their height.
    pub(crate) fn banshee_mirror_key(&self, old: Option<&LayoutKey>) -> LayoutKey {
        let fbi = &self.fbi;
        let width = (fbi.stride.pixels() as u32).clamp(1, 2048);
        let height = self.banshee.as_ref().map_or(1, |b| {
            let cfg = b.reg(VID_PROC_CFG);
            let rows = b.desktop_size().1;
            let half = cfg & (VIDPROC_HALF_MODE | VIDPROC_OVERLAY_ENABLE) == VIDPROC_HALF_MODE;
            if half { rows.div_ceil(2) } else { rows }
        });
        let height = height.clamp(1, 2048);
        let col = fbi.rgboffs[0];
        let key = LayoutKey {
            width,
            height,
            stride: fbi.stride,
            aux_stride: fbi.aux_stride,
            color: [col, NONE, NONE],
            aux: fbi.auxoffs,
        };
        let same = |o: &&LayoutKey| {
            (o.width, o.height, o.stride, o.aux_stride, o.aux)
                == (width, height, fbi.stride, fbi.aux_stride, fbi.auxoffs)
        };
        let Some(old) = old.filter(same) else {
            return key;
        };
        if old.color.contains(&col) {
            return *old;
        }
        let bytes = 2 * span(fbi.stride, height) as u64;
        let aux_bytes = 2 * span(fbi.aux_stride, height) as u64;
        let apart = |a: u32, a_len: u64, b: u32, b_len: u64| {
            a as u64 + a_len <= b as u64 || b as u64 + b_len <= a as u64
        };
        let mut color = [NONE; 3];
        let mut n = 0;
        for c in old.color {
            if c != NONE && apart(c, bytes, col, bytes) && apart(c, bytes, fbi.auxoffs, aux_bytes) {
                color[n] = c;
                n += 1;
            }
        }
        if n == 3 {
            color.rotate_left(1);
            n = 2;
        }
        color[n] = col;
        LayoutKey { color, ..key }
    }

    /// The colour buffer of the recording (its word offset) the video
    /// processor shows: the overlay's, which a swap points at
    /// leftOverlayBuf, while the overlay is on, or else the desktop's.
    pub(crate) fn banshee_mirror_front(&self, key: &LayoutKey) -> Option<u32> {
        let b = self.banshee.as_ref()?;
        let addr = if b.reg(VID_PROC_CFG) & VIDPROC_OVERLAY_ENABLE != 0 {
            b.overlay_addr
        } else {
            b.reg(VID_DESKTOP_START_ADDR)
        };
        key.color.contains(&addr).then_some(addr / 2)
    }

    /// swapbufferCMD: the overlay shows leftOverlayBuf, now or with bit 0
    /// at the retrace `bits 8:1` retraces after the last swap, and the
    /// swap counts off swapPending's count then (86Box's
    /// `voodoo_reg_writel`). The address is taken now, so a program can
    /// set the next one while the swap waits.
    fn banshee_swap(&mut self, data: u32, now: Now, effect: &mut Effect) {
        self.backlog.borrow_mut().swapped();
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
    pub(crate) fn banshee_swaps_done(&mut self, retired: usize, now: Now, effect: &mut Effect) {
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
            for n in 0..2 {
                self.run_banshee_fifo(n, now, effect);
            }
        }
    }

    /// Whether the command FIFOs wait for a swap's retrace.
    pub(crate) fn banshee_fifo_waits(&self) -> bool {
        !self.swaps.is_empty()
    }

    /// TMU 0's texture space from 600000h: texture memory from its
    /// texBaseAddr on, linear (86Box's `voodoo_tex_writel`). The
    /// triangles queued before are drawn first if they may read or draw
    /// into the dword and can't be given a copy of what they read
    /// (`spare`).
    pub(crate) fn banshee_texture_write(&mut self, offset: u32, data: u32) {
        let base = self.reg[0x100 + TEX_BASE_ADDR] & 0xFF_FFF0;
        let addr = base.wrapping_add(offset & 0x1F_FFFC) & self.fbi.mask;
        if !self.spare(addr, addr + 4) {
            self.flush();
        }
        self.gpu_sync_bytes(addr, 4);
        super::blit2d::write(&self.fbi.ram, addr, 4, data, self.fbi.mask);
        self.mirror_memory(addr, addr + 4);
    }

    /// `data` downloaded into the card's memory from byte `addr` on, as
    /// writes of it through TMU 0's texture space would put it, all at
    /// once: Glide's texture downloads. The triangles queued before are
    /// drawn first if they may read or draw into that memory and can't
    /// be given a copy of what they read (`spare`), and the recording
    /// hears of the bytes once. Past the end of the memory they wrap
    /// round.
    pub(crate) fn banshee_texture_download(&mut self, addr: u32, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        let size = self.fbi.mask as usize + 1;
        let start = addr as usize & (size - 1);
        let (here, wrapped) = data.split_at(data.len().min(size - start));
        if !self.spare(start as u32, (start + here.len()) as u32) {
            self.flush();
        }
        self.gpu_sync_bytes(start as u32, here.len());
        self.fbi.ram.write_bytes(start, here);
        self.mirror_memory(start as u32, (start + here.len()) as u32);
        if !wrapped.is_empty() {
            self.banshee_texture_download(0, wrapped);
        }
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
    fn gl_display_leaves_jobs_until_a_software_picture_is_requested() {
        use super::super::{VGA_INIT0, VGAINIT0_EXTENDED_SHIFT_OUT, VID_PROC_CFG, VIDPROC_ENABLE};

        for board in [Board::Banshee, Board::Voodoo3] {
            let mut v = Voodoo::with_workers(board, 4);
            v.write(VGA_INIT0, VGAINIT0_EXTENDED_SHIFT_OUT, u32::MAX, Now::default());
            v.write(VID_PROC_CFG, VIDPROC_ENABLE, u32::MAX, Now::default());
            reg3d(&mut v, COL_BUFFER_ADDR, 0);
            reg3d(&mut v, COL_BUFFER_STRIDE, 1280);
            reg3d(&mut v, AUX_BUFFER_ADDR, 0x20_0000);
            reg3d(&mut v, AUX_BUFFER_STRIDE, 1280);
            reg3d(&mut v, FBZ_MODE, 1 << 9);
            v.set_mirror(true);
            v.set_software_picture(false);
            // Consume the initial snapshot before queuing any drawing.
            v.take_mirror();
            assert!(v.output());
            assert!(v.prepare_display(), "switching output still signals a change");
            rectangle(&mut v, (0.0, 0.0, 16.0, 16.0), (255.0, 0.0, 0.0));
            assert!(v.writes_queue());
            assert_eq!(v.fbi.ram.get(0), 0, "the batch has not been sent yet");
            assert!(!v.prepare_display());
            assert_eq!(v.fbi.ram.get(0), 0, "GL scanout must not flush the batch");
            assert!(v.display_dirty);
            // The video processor's change went to the renderer with the
            // scanout `take_mirror` gave it.
            assert!(!v.banshee.as_ref().unwrap().dirty);

            // Screenshots, the debugger and renderer failure request the
            // software picture through the same switch.
            v.set_software_picture(true);
            assert!(v.prepare_display());
            assert!(!v.writes_queue());
            assert_eq!(v.fbi.ram.get(0), 0xF800);
            assert!(!v.prepare_display());

            // A change confined to the video processor is retained too.
            v.set_software_picture(false);
            v.banshee.as_mut().unwrap().dirty = true;
            assert!(!v.prepare_display());
            assert!(v.banshee.as_ref().unwrap().dirty);
            v.set_mirror(false);
            assert!(v.prepare_display());
            assert!(!v.banshee.as_ref().unwrap().dirty);
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
    fn the_card_is_busy_while_a_swap_waits_for_its_retrace() {
        // 3dfx's driver on Windows 95 flips Moto Racer's menus with a
        // swap at the retrace, then waits for the card to go idle before
        // it draws the next frame into the buffer still shown (#202).
        let mut v = card();
        let now = Now::default();
        let busy = |v: &Voodoo, now: Now| v.banshee_status(now) & 0x780 != 0;
        assert!(!busy(&v, now));
        reg3d(&mut v, LEFT_OVERLAY_BUF, 0x10_0000);
        reg3d(&mut v, SWAPBUFFER_CMD, 0);
        assert!(!busy(&v, now), "a swap at once is done at once");
        reg3d(&mut v, LEFT_OVERLAY_BUF, 0x20_0000);
        reg3d(&mut v, SWAPBUFFER_CMD, 1);
        assert!(busy(&v, now), "busy until the retrace");
        assert_eq!(v.read(VID_CURR_OVERLAY_START_ADDR, now), 0x10_0000);
        let due = v.next_event().expect("the swap waits for a retrace");
        let then = Now {
            ns: 1_000_000_000,
            ticks: due,
        };
        // Busy until the overlay shows the new buffer, which the timer
        // event at the retrace does.
        assert!(busy(&v, then), "busy until the swap is done");
        v.service(then);
        assert_eq!(v.read(VID_CURR_OVERLAY_START_ADDR, then), 0x20_0000);
        assert!(!busy(&v, then));
    }

    #[test]
    fn a_lower_refresh_rate_doesnt_put_swaps_off() {
        // A swap at once after ten minutes at 70 Hz, then a mode at 60
        // Hz: the next swap at a retrace waits about a frame, not for the
        // 6000 retraces 70 Hz counted past what 60 Hz has.
        use crate::timer::PIT_HZ;
        use crate::video::crt::CrtTiming;
        let mut v = card();
        let now = Now {
            ns: 600_000_000_000,
            ticks: 600 * PIT_HZ,
        };
        let swap = |v: &mut Voodoo, data: u32| {
            let at = 0x20_0000 + 4 * SWAPBUFFER_CMD as u32;
            v.write(at, data, 0xFFFF_FFFF, now);
        };
        v.set_timing(CrtTiming::VGA_400, now);
        swap(&mut v, 0);
        v.set_timing(CrtTiming::VESA_480, now);
        swap(&mut v, 1 << 1 | 1);
        let due = v.next_event().expect("the swap waits for a retrace");
        assert!(due - now.ticks < PIT_HZ / 30);
    }

    /// A card recording for the OpenGL renderer, a desktop of 64 rows on
    /// and the linear frame buffer untiled.
    fn mirrored() -> Voodoo {
        let mut v = card();
        let now = Now::default();
        v.write(VGA_INIT0, VGAINIT0_EXTENDED_SHIFT_OUT, u32::MAX, now);
        v.write(VID_PROC_CFG, VIDPROC_ENABLE | 1 << 18, u32::MAX, now);
        v.write(LFB_MEMORY_CONFIG, 0x1FFF, u32::MAX, now);
        v.banshee.as_mut().unwrap().crtc[0x12] = 63;
        reg3d(&mut v, FBZ_MODE, 1 << 9);
        v.set_mirror(true);
        v
    }

    fn snapshots(frame: &Frame) -> Vec<&Snapshot> {
        let resyncs = frame.commands.iter().filter_map(|c| match c {
            Command::Resync(s) => Some(&**s),
            _ => None,
        });
        resyncs.collect()
    }

    fn pixels(frame: &Frame) -> Vec<&Pixels> {
        let rows = frame.commands.iter().filter_map(|c| match c {
            Command::Pixels(p) => Some(p),
            _ => None,
        });
        rows.collect()
    }

    use super::super::super::mirror::{Command, Frame, Pixels, Snapshot};
    use super::super::{LFB_MEMORY_CONFIG, VGA_INIT0, VGAINIT0_EXTENDED_SHIFT_OUT, VIDPROC_ENABLE};

    #[test]
    fn the_recording_reads_tiled_buffers() {
        let mut v = mirrored();
        // Colour at 1 MB in rows of 10 tiles (640 pixels), depth at 2 MB
        // linear, 1280 bytes a row.
        reg3d(&mut v, COL_BUFFER_ADDR, 0x10_0000);
        reg3d(&mut v, COL_BUFFER_STRIDE, 1 << 15 | 10);
        reg3d(&mut v, AUX_BUFFER_ADDR, 0x20_0000);
        reg3d(&mut v, AUX_BUFFER_STRIDE, 1280);
        // Pixel (65, 33): tile 1 of the second row of tiles, row 1 in it.
        v.fbi
            .ram
            .set(0x10_0000 / 2 + 10 * 2048 + 2048 + 64 + 1, 0xF800);
        v.fbi.ram.set(0x20_0000 / 2 + 33 * 640 + 65, 0x1234);
        let frame = v.take_mirror().unwrap();
        let [snapshot] = snapshots(&frame)[..] else {
            panic!("one snapshot")
        };
        let layout = &snapshot.layout;
        assert_eq!((layout.width, layout.height), (640, 64));
        assert_eq!(layout.color, [0x10_0000 / 2]);
        assert_eq!(snapshot.color[0][33 * 640 + 65], 0xF800);
        assert_eq!(snapshot.color[0][33 * 640 + 64], 0);
        assert_eq!(snapshot.aux.as_ref().unwrap()[33 * 640 + 65], 0x1234);
    }

    #[test]
    fn textures_written_through_bar1_are_decoded_again() {
        use super::super::LINEAR;
        let decoded = |frame: &Frame| -> Vec<(u32, u32)> {
            let textures = frame.commands.iter().filter_map(|c| match c {
                Command::Texture(t) => Some((t.id, t.levels[0].argb[0])),
                _ => None,
            });
            textures.collect()
        };
        let mut v = mirrored();
        reg3d(&mut v, COL_BUFFER_ADDR, 0x10_0000);
        reg3d(&mut v, COL_BUFFER_STRIDE, 1280);
        // Level 5 (8x8) of a 5-6-5 texture at 5 MB, as its colour.
        reg3d(&mut v, TEX_BASE_ADDR, 0x50_0000);
        reg3d(&mut v, T_LOD, 20 | 20 << 6);
        let decal = 10 << 8 | 1 << 12 | 1 << 18 | 1 << 21 | 1 << 27;
        reg3d(&mut v, TEXTURE_MODE, decal);
        reg3d(&mut v, FBZ_COLOR_PATH, 1 | 1 << 27);
        rectangle(&mut v, (0.0, 0.0, 8.0, 8.0), (0.0, 0.0, 0.0));
        // A Voodoo3's two units draw with it.
        let [(first, _), (second, _)] = decoded(&v.take_mirror().unwrap())[..] else {
            panic!("a texture for each unit")
        };
        // Drawn with again, nothing to decode; its first texel written by
        // the CPU, which a Banshee's texture units share memory with, it
        // is decoded again as it is now.
        rectangle(&mut v, (0.0, 0.0, 8.0, 8.0), (0.0, 0.0, 0.0));
        assert!(decoded(&v.take_mirror().unwrap()).is_empty());
        let level5 = 2 * (65536 + 16384 + 4096 + 1024 + 256);
        v.write(LINEAR + 0x50_0000 + level5, 0xF800, 0xFFFF, Now::default());
        rectangle(&mut v, (0.0, 0.0, 8.0, 8.0), (0.0, 0.0, 0.0));
        let red = 0xFFFF_0000;
        assert_eq!(
            decoded(&v.take_mirror().unwrap()),
            [(first, red), (second, red)]
        );
    }

    #[test]
    fn double_buffered_frames_drop_what_is_drawn_over() {
        use super::super::LINEAR;
        // Eight frames as a game draws them, into two tiled buffers in
        // turn: clear the buffer and the depth with a fastfill, draw,
        // swap through the overlay, polling the busy bits on the way. The
        // status register reads leave held jobs alone. The recording
        // takes the second buffer's memory once, when it joins the
        // layout; from then on nothing waits for the drawing.
        let (a, b) = (0x10_0000, 0x20_0000);
        let run = |mirror: bool| {
            let mut v = mirrored();
            v.pool = super::super::super::workers::Pool::new(2);
            v.set_mirror(mirror);
            reg3d(&mut v, COL_BUFFER_STRIDE, 1 << 15 | 10);
            reg3d(&mut v, AUX_BUFFER_ADDR, 0x40_0000);
            reg3d(&mut v, AUX_BUFFER_STRIDE, 1 << 15 | 10);
            reg3d(&mut v, CLIP_LEFT_RIGHT, 640);
            reg3d(&mut v, CLIP_LOW_Y_HIGH_Y, 64);
            reg3d(&mut v, FBZ_MODE, 1 | 1 << 9 | 1 << 10);
            let mut joined = 0;
            for frame in 0..8u32 {
                if frame == 2 {
                    joined = v.catch_ups.get();
                }
                let draw = if frame % 2 == 0 { a } else { b };
                reg3d(&mut v, COL_BUFFER_ADDR, draw);
                reg3d(&mut v, COLOR1, frame * 0x10_1010);
                reg3d(&mut v, FASTFILL_CMD, 0);
                let x = frame as f32 * 8.0;
                rectangle(&mut v, (x, 4.0, x + 30.0, 40.0), (255.0, 128.0, 0.0));
                v.read(0x20_0000 + 4 * STATUS as u32, Now::default());
                reg3d(&mut v, LEFT_OVERLAY_BUF, draw);
                reg3d(&mut v, SWAPBUFFER_CMD, 0);
                if mirror {
                    v.take_mirror();
                }
            }
            let held = (v.backlog.borrow().pruned, v.catch_ups.get() - joined);
            // The CPU reads pixel (65, 5) of the last frame.
            let pixel = v.read(LINEAR + b + 2 * (2048 + 5 * 64 + 1), Now::default());
            (held, pixel, v.frame_buffer().to_bytes())
        };
        let ((pruned, catch_ups), pixel, recorded) = run(true);
        assert!(pruned > 0, "the frames drawn over are dropped");
        assert_eq!(catch_ups, 0, "nothing read the memory while they were held");
        let (_, software_pixel, software) = run(false);
        assert_eq!(pixel, software_pixel);
        assert!(recorded == software, "the memory ends up the same");
    }

    #[test]
    fn frames_drawn_over_after_a_depth_clear_drop_what_was_under_them() {
        // A game that clears only the depth and draws over the whole
        // picture each frame, double buffered. The background is drawn
        // with "less than or equal"; from frame 4 on, a depth-only
        // triangle nearer than it goes first in a corner, where the
        // background then fails its test and the colour of two frames
        // before stays. A blended triangle goes on top.
        let (a, b) = (0x10_0000, 0x20_0000);
        let depth_only = 1 | 1 << 10;
        let background = 1 | 1 << 4 | 3 << 5 | 1 << 9 | 1 << 10;
        let run = |mirror: bool| {
            let mut v = mirrored();
            v.pool = super::super::super::workers::Pool::new(2);
            v.set_mirror(mirror);
            reg3d(&mut v, COL_BUFFER_STRIDE, 1 << 15 | 10);
            reg3d(&mut v, AUX_BUFFER_ADDR, 0x40_0000);
            reg3d(&mut v, AUX_BUFFER_STRIDE, 1 << 15 | 10);
            reg3d(&mut v, CLIP_LEFT_RIGHT, 640);
            reg3d(&mut v, CLIP_LOW_Y_HIGH_Y, 64);
            reg3d(&mut v, ZA_COLOR, 0xFFFF);
            let mut joined = 0;
            for frame in 0..10u32 {
                if frame == 2 {
                    joined = v.catch_ups.get();
                }
                let draw = if frame % 2 == 0 { a } else { b };
                reg3d(&mut v, COL_BUFFER_ADDR, draw);
                reg3d(&mut v, ALPHA_MODE, 0);
                reg3d(&mut v, FBZ_MODE, 1 << 10);
                reg3d(&mut v, FASTFILL_CMD, 0);
                if frame >= 4 {
                    reg3d(&mut v, FBZ_MODE, depth_only);
                    float(&mut v, FSTART_Z, 0.0);
                    rectangle(&mut v, (0.0, 0.0, 20.0, 12.0), (0.0, 0.0, 0.0));
                }
                reg3d(&mut v, FBZ_MODE, background);
                float(&mut v, FSTART_Z, 1000.0);
                let shade = (frame * 25) as f32;
                let colour = (shade, 255.0 - shade, 60.0);
                rectangle(&mut v, (0.0, 0.0, 640.0, 64.0), colour);
                // Blended: source alpha and one minus it.
                reg3d(&mut v, ALPHA_MODE, 1 << 4 | 1 << 8 | 5 << 12);
                float(&mut v, FSTART_A, 128.0);
                rectangle(&mut v, (100.0, 10.0, 200.0, 50.0), (255.0, 255.0, 255.0));
                reg3d(&mut v, LEFT_OVERLAY_BUF, draw);
                reg3d(&mut v, SWAPBUFFER_CMD, 0);
                if mirror {
                    v.take_mirror();
                }
            }
            let held = (v.backlog.borrow().pruned, v.catch_ups.get() - joined);
            (held, v.frame_buffer().to_bytes())
        };
        let ((pruned, catch_ups), recorded) = run(true);
        assert!(pruned > 0, "the frames drawn over are dropped");
        assert_eq!(catch_ups, 0);
        let (_, software) = run(false);
        assert!(recorded == software, "the memory ends up the same");
    }

    #[test]
    fn triangles_textured_from_a_colour_buffer_keep_what_they_read() {
        // A Banshee's texture units read the frame buffer's memory, so a
        // triangle can draw with a colour buffer's pixels as its texture.
        // Each frame clears the depth, covers the picture, then draws a
        // small square over pixel (0, 0), in a tile of its own. Frame 8
        // first draws with buffer A's own pixel (0, 0) as texture, before
        // the background covers it; frame 9 draws into B with A's. A
        // fastfill of A's first tile comes last. What these read has to
        // be drawn, and stay what the software path draws.
        let (a, b) = (0x10_0000u32, 0x20_0000u32);
        let near = 1 | 1 << 4 | 3 << 5 | 1 << 9 | 1 << 10;
        let decal = 10 << 8 | 1 << 12 | 1 << 18 | 1 << 21 | 1 << 27;
        let textured = |v: &mut Voodoo, from: u32, x: f32| {
            reg3d(v, TEX_BASE_ADDR, from);
            reg3d(v, T_LOD, 0);
            reg3d(v, TEXTURE_MODE, decal);
            reg3d(v, FBZ_COLOR_PATH, 1 | 1 << 27);
            reg3d(v, FBZ_MODE, near);
            float(v, FSTART_Z, 0.0);
            rectangle(v, (x, 20.0, x + 40.0, 40.0), (0.0, 0.0, 0.0));
            reg3d(v, FBZ_COLOR_PATH, 0);
        };
        let run = |mirror: bool| {
            // No workers: each draws its own rows, and a texture
            // read from another's rows doesn't wait for it.
            let mut v = mirrored();
            v.set_mirror(mirror);
            reg3d(&mut v, COL_BUFFER_STRIDE, 1 << 15 | 10);
            reg3d(&mut v, AUX_BUFFER_ADDR, 0x40_0000);
            reg3d(&mut v, AUX_BUFFER_STRIDE, 1 << 15 | 10);
            reg3d(&mut v, CLIP_LEFT_RIGHT, 640);
            reg3d(&mut v, CLIP_LOW_Y_HIGH_Y, 64);
            reg3d(&mut v, ZA_COLOR, 0xFFFF);
            for frame in 0..10u32 {
                let draw = if frame % 2 == 0 { a } else { b };
                reg3d(&mut v, COL_BUFFER_ADDR, draw);
                reg3d(&mut v, FBZ_MODE, 1 << 10);
                reg3d(&mut v, FASTFILL_CMD, 0);
                if frame == 8 {
                    textured(&mut v, a, 400.0);
                }
                reg3d(&mut v, FBZ_MODE, near);
                float(&mut v, FSTART_Z, 1000.0);
                let shade = (frame * 25) as f32;
                let colour = (shade, 255.0 - shade, 60.0);
                rectangle(&mut v, (0.0, 0.0, 640.0, 64.0), colour);
                float(&mut v, FSTART_Z, 500.0);
                rectangle(&mut v, (0.0, 0.0, 8.0, 4.0), (60.0, shade, 255.0));
                if frame == 9 {
                    textured(&mut v, a, 500.0);
                }
                reg3d(&mut v, LEFT_OVERLAY_BUF, draw);
                reg3d(&mut v, SWAPBUFFER_CMD, 0);
                if mirror {
                    v.take_mirror();
                }
            }
            reg3d(&mut v, COL_BUFFER_ADDR, a);
            reg3d(&mut v, CLIP_LEFT_RIGHT, 80);
            reg3d(&mut v, CLIP_LOW_Y_HIGH_Y, 8);
            reg3d(&mut v, FBZ_MODE, 1 | 1 << 9);
            reg3d(&mut v, COLOR1, 0xFF_FFFF);
            reg3d(&mut v, FASTFILL_CMD, 0);
            let memory = v.frame_buffer().to_bytes();
            (v.backlog.borrow().pruned, memory)
        };
        let (pruned, recorded) = run(true);
        assert!(pruned > 0, "the frames drawn over are dropped");
        let (_, software) = run(false);
        // Pixel (420, 30) of A and (520, 30) of B, which the textured
        // squares drew: row 30 of the first row of tiles.
        let pixel = |memory: &[u8], base: u32, x: u32| {
            let at = (base / 2 + (x / 64) * 2048 + 30 * 64 + x % 64) as usize * 2;
            u16::from_le_bytes([memory[at], memory[at + 1]])
        };
        for (base, x) in [(a, 420), (b, 520)] {
            assert_eq!(pixel(&recorded, base, x), pixel(&software, base, x));
        }
        assert!(recorded == software, "the memory ends up the same");
    }

    #[test]
    fn half_mode_buffers_are_half_the_desktop() {
        let mut v = mirrored();
        let cfg = VIDPROC_ENABLE | 1 << 18 | VIDPROC_HALF_MODE;
        v.write(VID_PROC_CFG, cfg, u32::MAX, Now::default());
        reg3d(&mut v, COL_BUFFER_STRIDE, 1280);
        // Two buffers of 32 rows back to back stay apart.
        let (a, b) = (0x10_0000, 0x10_0000 + 32 * 1280);
        for draw in [a, b] {
            reg3d(&mut v, COL_BUFFER_ADDR, draw);
            rectangle(&mut v, (0.0, 0.0, 8.0, 8.0), (255.0, 0.0, 0.0));
        }
        let frame = v.take_mirror().unwrap();
        let [snapshot] = snapshots(&frame)[..] else {
            panic!("one snapshot")
        };
        assert_eq!(snapshot.layout.height, 32);
        assert_eq!(snapshot.layout.color, [a / 2, b / 2]);
    }

    #[test]
    fn swapping_between_buffers_keeps_the_recording() {
        let mut v = mirrored();
        reg3d(&mut v, COL_BUFFER_STRIDE, 1 << 15 | 10);
        reg3d(&mut v, AUX_BUFFER_ADDR, 0x40_0000);
        reg3d(&mut v, AUX_BUFFER_STRIDE, 1 << 15 | 10);
        // Draw into a buffer, show it through the overlay, draw into the
        // other, as a program double buffering does.
        let (a, b) = (0x10_0000, 0x20_0000);
        let mut frames = Vec::new();
        for draw in [a, b, a, b] {
            reg3d(&mut v, COL_BUFFER_ADDR, draw);
            rectangle(&mut v, (0.0, 0.0, 8.0, 8.0), (255.0, 0.0, 0.0));
            reg3d(&mut v, LEFT_OVERLAY_BUF, draw);
            reg3d(&mut v, SWAPBUFFER_CMD, 0);
            frames.push(v.take_mirror().unwrap());
        }
        // The second buffer joins the layout once; then nothing starts
        // over from memory.
        let counts: Vec<usize> = frames.iter().map(|f| snapshots(f).len()).collect();
        assert_eq!(counts, [1, 1, 0, 0]);
        assert_eq!(snapshots(&frames[1])[0].layout.color, [a / 2, b / 2]);
        // Without the overlay on, the desktop's buffer is the one shown.
        assert_eq!(frames[3].front, None);
        let cfg = VIDPROC_ENABLE | 1 << 18 | VIDPROC_OVERLAY_ENABLE;
        v.write(VID_PROC_CFG, cfg, u32::MAX, Now::default());
        assert_eq!(v.take_mirror().unwrap().front, Some(b / 2));
        v.write(VID_DESKTOP_START_ADDR, a, u32::MAX, Now::default());
        v.write(
            VID_PROC_CFG,
            VIDPROC_ENABLE | 1 << 18,
            u32::MAX,
            Now::default(),
        );
        assert_eq!(v.take_mirror().unwrap().front, Some(a / 2));
        // A third buffer joins, a fourth drops the oldest.
        for (draw, expect) in [
            (0x30_0000, [a, b, 0x30_0000]),
            (0x50_0000, [b, 0x30_0000, 0x50_0000]),
        ] {
            reg3d(&mut v, COL_BUFFER_ADDR, draw);
            rectangle(&mut v, (0.0, 0.0, 8.0, 8.0), (255.0, 0.0, 0.0));
            let frame = v.take_mirror().unwrap();
            assert_eq!(snapshots(&frame)[0].layout.color, expect.map(|c| c / 2));
        }
        // One that overlaps a buffer replaces it.
        reg3d(&mut v, COL_BUFFER_ADDR, 0x30_0000 + 0x8000);
        rectangle(&mut v, (0.0, 0.0, 8.0, 8.0), (255.0, 0.0, 0.0));
        let frame = v.take_mirror().unwrap();
        assert_eq!(
            snapshots(&frame)[0].layout.color,
            [b / 2, 0x50_0000 / 2, 0x30_8000 / 2]
        );
    }

    #[test]
    fn writes_past_the_3d_engine_reach_the_recording() {
        use super::super::LINEAR;
        let mut v = mirrored();
        reg3d(&mut v, COL_BUFFER_ADDR, 0x10_0000);
        reg3d(&mut v, COL_BUFFER_STRIDE, 1280);
        reg3d(&mut v, AUX_BUFFER_ADDR, 0x20_0000);
        reg3d(&mut v, AUX_BUFFER_STRIDE, 1 << 15 | 10);
        v.take_mirror();
        // The CPU through BAR1: pixels (2, 3) and (3, 3) of the colour
        // buffer, then (65, 33) of the tiled depth.
        let now = Now::default();
        v.write(
            LINEAR + 0x10_0000 + 3 * 1280 + 4,
            0x07E0_F800,
            u32::MAX,
            now,
        );
        let depth = 0x20_0000 + 2 * (10 * 2048 + 2048 + 64);
        v.write(LINEAR + depth, 0x4321_FFFF, 0xFFFF_0000, now);
        let frame = v.take_mirror().unwrap();
        let rows = pixels(&frame);
        assert_eq!(rows.len(), 2);
        assert_eq!(
            (rows[0].dest, rows[0].x, rows[0].y),
            (Some(0x10_0000 / 2), 2, 3)
        );
        assert_eq!(rows[0].values, [0xF800, 0x07E0]);
        assert_eq!((rows[1].dest, rows[1].x, rows[1].y), (None, 65, 33));
        assert_eq!(rows[1].values, [0x4321]);
        // Memory elsewhere isn't any of the buffers' pixels.
        v.write(LINEAR + 0x30_0000, 1, u32::MAX, now);
        assert!(pixels(&v.take_mirror().unwrap()).is_empty());

        // The 2D engine: a rectangle of 12x3 at (1, 2) in the colour
        // buffer. It reaches the recording as the pixels drawn, a run of
        // them per row, not as the rows whole, which would overwrite
        // what the OpenGL renderer drew beside it.
        let reg2d =
            |v: &mut Voodoo, reg: u32, data: u32| v.write(0x10_0000 + reg, data, u32::MAX, now);
        reg2d(&mut v, 0x14, 3 << 16 | 1280);
        reg2d(&mut v, 0x10, 0x10_0000);
        reg2d(&mut v, 0x08, 0);
        reg2d(&mut v, 0x0C, 0x0FFF_0FFF);
        reg2d(&mut v, 0x64, 0x001F);
        reg2d(&mut v, 0x68, 3 << 16 | 12);
        reg2d(&mut v, 0x6C, 2 << 16 | 1);
        reg2d(&mut v, 0x70, 0xCC << 24 | 1 << 8 | 5);
        let frame = v.take_mirror().unwrap();
        let rows = pixels(&frame);
        let covered: Vec<u32> = rows.iter().map(|p| p.y).collect();
        assert_eq!(covered, [2, 3, 4]);
        assert!(rows.iter().all(|p| p.x == 1 && p.values == [0x001F; 12]));
    }

    #[test]
    fn a_run_through_the_pipeline_writes_what_its_dwords_write() {
        // 1-5-5-5 through the pixel pipeline with chroma keying, as Sub
        // Culture writes its overlay: the keyed pixels stay as they were.
        let setup = |v: &mut Voodoo| {
            reg3d(v, COL_BUFFER_ADDR, 0x40_0000);
            reg3d(v, COL_BUFFER_STRIDE, 1 << 15 | 8);
            reg3d(v, LFB_MODE, 2 | 1 << 8);
            reg3d(v, FBZ_MODE, 1 << 1 | 1 << 9);
            reg3d(v, CHROMA_KEY, 0);
            // The first two tiles, 64 pixels by 32 rows each.
            for i in 0..2 * 2048 {
                v.fbi.ram.set(0x40_0000 / 2 + i, 0x1234);
            }
        };
        let data: Vec<u32> = (0..40u32)
            .map(|i| {
                if i % 5 == 0 {
                    0
                } else {
                    i.wrapping_mul(0x9E37_79B9) | 0x8000_8000
                }
            })
            .collect();
        let (mut run, mut each) = (card(), card());
        setup(&mut run);
        setup(&mut each);
        // Pixels 6 to 85 of row 3: into the second tile from pixel 64 on.
        let offset = 0x100_0000 + 3 * 4096 + 6 * 2;
        let index = run
            .lfb_run(offset, 4 * data.len() as u32)
            .expect("in the LFB");
        run.lfb_write_run(index, data.iter().copied());
        for (i, &value) in (0..).zip(&data) {
            each.write(offset + 4 * i, value, u32::MAX, Now::default());
        }
        let base = 0x40_0000 / 2;
        for i in 0..2 * 2048 {
            assert_eq!(
                run.fbi.ram.get(base + i),
                each.fbi.ram.get(base + i),
                "word {i}"
            );
        }
        let (row, next_tile) = (base + 3 * 64, base + 2048 + 3 * 64);
        assert_eq!(run.fbi.ram.get(row + 6), 0x1234, "keyed");
        assert_ne!(run.fbi.ram.get(row + 8), 0x1234);
        assert_ne!(run.fbi.ram.get(next_tile), 0x1234, "pixel 64");
    }

    #[test]
    fn pipeline_writes_are_recorded_from_a_tiled_buffer() {
        let mut v = mirrored();
        reg3d(&mut v, COL_BUFFER_ADDR, 0x10_0000);
        reg3d(&mut v, COL_BUFFER_STRIDE, 1 << 15 | 10);
        v.take_mirror();
        // 5-6-5 through the pixel pipeline: pixel (64, 33).
        reg3d(&mut v, LFB_MODE, 1 << 8);
        let offset = 0x100_0000 + 33 * 4096 + 64 * 2;
        v.write(offset, 0xF800, 0x0000_FFFF, Now::default());
        assert_eq!(v.fbi.ram.get(0x10_0000 / 2 + 10 * 2048 + 2048 + 64), 0xF800);
        let frame = v.take_mirror().unwrap();
        let rows = pixels(&frame);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].dest, rows[0].x, rows[0].y),
            (Some(0x10_0000 / 2), 64, 33)
        );
        assert_eq!(rows[0].values, [0xF800]);
    }

    /// The pages of memory a frame's recording carries for the
    /// renderer's copy of it: where each run starts and its bytes.
    fn memory(frame: &Frame) -> Vec<(u32, &[u8])> {
        let runs = frame.commands.iter().filter_map(|c| match c {
            Command::Memory { at, bytes, .. } => Some((*at, &bytes[..])),
            _ => None,
        });
        runs.collect()
    }

    #[test]
    fn the_scanout_sends_the_pages_written_since_the_renderer_had_them() {
        use super::super::super::pages::PAGE;
        use super::super::{LINEAR, VID_DESKTOP_OVERLAY_STRIDE};
        let mut v = mirrored();
        v.set_software_picture(false);
        let now = Now::default();
        v.write(VID_DESKTOP_OVERLAY_STRIDE, 1280, u32::MAX, now);
        reg3d(&mut v, COL_BUFFER_ADDR, 0x10_0000);
        reg3d(&mut v, COL_BUFFER_STRIDE, 1 << 15 | 10);
        // The desktop at 0 isn't a buffer the renderer draws: its pages
        // go the first time, and not again until something writes them.
        let first = v.take_mirror().unwrap();
        assert_eq!(first.scanout.as_ref().unwrap().desktop.buffer, None);
        assert_eq!(memory(&first).first().map(|m| m.0), Some(0));
        assert!(memory(&v.take_mirror().unwrap()).is_empty());
        v.write(LINEAR + 4, 0x1234_5678, u32::MAX, now);
        let frame = v.take_mirror().unwrap();
        let sent = memory(&frame);
        assert_eq!(sent.len(), 1);
        assert_eq!((sent[0].0, sent[0].1.len()), (0, PAGE as usize));
        assert_eq!(sent[0].1[4..8], [0x78, 0x56, 0x34, 0x12]);
        // Moved onto the colour buffer, it's drawn from there; moved back,
        // the renderer still has its pages, and none go again.
        v.write(VID_DESKTOP_START_ADDR, 0x10_0000, u32::MAX, now);
        v.write(
            VID_PROC_CFG,
            VIDPROC_ENABLE | 1 << 18 | 1 << 24,
            u32::MAX,
            now,
        );
        v.write(VID_DESKTOP_OVERLAY_STRIDE, 10, u32::MAX, now);
        let on_buffer = v.take_mirror().unwrap();
        let scanout = on_buffer.scanout.as_ref().unwrap();
        assert_eq!(scanout.desktop.buffer, Some(0x10_0000 / 2));
        assert!(memory(&on_buffer).is_empty());
        v.write(VID_DESKTOP_START_ADDR, 0, u32::MAX, now);
        v.write(VID_PROC_CFG, VIDPROC_ENABLE | 1 << 18, u32::MAX, now);
        v.write(VID_DESKTOP_OVERLAY_STRIDE, 1280, u32::MAX, now);
        assert!(memory(&v.take_mirror().unwrap()).is_empty());
        // A recording wants the software picture too, which looks at the
        // change first: the renderer still gets the page.
        v.set_software_picture(true);
        v.prepare_display();
        v.write(LINEAR + 8, 0x0BAD_F00D, u32::MAX, now);
        assert!(v.prepare_display());
        let frame = v.take_mirror().unwrap();
        assert_eq!(memory(&frame)[0].1[8..12], [0x0D, 0xF0, 0xAD, 0x0B]);
        // A renderer starting over gets them all again.
        v.set_mirror(false);
        v.set_mirror(true);
        assert_eq!(memory(&v.take_mirror().unwrap()).first().map(|m| m.0), Some(0));
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

    #[test]
    fn a_texture_download_is_what_writes_through_the_texture_space_make() {
        // Rows of an odd length at an odd address: the bytes around them
        // stay as they were.
        let data: Vec<u8> = (0..37u8).map(|i| i.wrapping_mul(7) ^ 0x5A).collect();
        let (at, end) = (0x40_0003u32, 0x40_0003 + data.len() as u32);
        let (mut bulk, mut dwords) = (card(), card());
        for v in [&bulk, &dwords] {
            v.fbi.ram.fill(0x1F_FFF0..0x20_0040, 0xA5C3);
        }
        bulk.banshee_texture_download(at, &data);
        // The same bytes a dword at a time, merged into what is there.
        let port = at & !0xF;
        reg3d(&mut dwords, TEX_BASE_ADDR, port);
        for dword in (at & !3..end).step_by(4) {
            let bytes: [u8; 4] = std::array::from_fn(|i| {
                let a = dword + i as u32;
                if (at..end).contains(&a) {
                    data[(a - at) as usize]
                } else {
                    dwords.fbi.ram.byte(a as usize)
                }
            });
            let value = u32::from_le_bytes(bytes);
            dwords.write(0x60_0000 + dword - port, value, u32::MAX, Now::default());
        }
        let around = |v: &Voodoo| {
            (0x3F_FFE0..0x40_0080)
                .map(|a| v.fbi.ram.byte(a))
                .collect::<Vec<_>>()
        };
        assert_eq!(around(&bulk), around(&dwords));
        assert_eq!(bulk.fbi.ram.byte(at as usize - 1), 0xC3);
        assert_eq!(bulk.fbi.ram.byte(at as usize), data[0]);
    }
}
