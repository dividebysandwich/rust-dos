//! Register writes and reads: the triangle parameters in their fixed and
//! float formats, the modes, the commands, the init registers and the
//! DAC behind them, the video timing, the NCC and fog tables. From
//! DOSBox-X's `register_w` and `register_r` (voodoo_emu.cpp); the gamma
//! table and vRetrace as MAME has them, and the Voodoo2's registers as
//! MAME's voodoo_2.cpp has them: their decoding (`map_register_w`), the
//! fbiInit5-7, command FIFO, setup engine and blitter registers.

use super::regs::*;
use super::tables::{float_to_int32, float_to_int64};
use super::chip::fields;
use super::{Effect, LOG_IGNORED, Now, Voodoo};

/// A 24-bit signed value in the low bits.
fn s24(data: u32) -> i32 {
    ((data << 8) as i32) >> 8
}

impl Voodoo {
    /// Whether the triangle registers are aliased (fbiInit3 bit 0).
    fn alt_regmap(&self) -> bool {
        self.reg[FBI_INIT3] & 1 != 0
    }

    /// Register write at dword `offset` of the window. True if it went
    /// through the FIFO.
    pub(crate) fn register_write(&mut self, offset: u32, data: u32, now: Now, effect: &mut Effect) -> bool {
        let mut chips = (offset >> 8) & 0xF;
        if chips == 0 {
            chips = 0xF;
        }
        chips &= self.chipmask;
        let (regnum, data) = if self.chip.v2() {
            // Address bit 21 is the command FIFO's while it is on.
            if offset & 0x8_0000 != 0 && self.cmdfifo_on() {
                self.cmdfifo_write(offset, data, now, effect);
                return true;
            }
            // Address bit 20 swaps the bytes, if fbiInit0 bit 3 lets it.
            let swizzle = offset & 0x4_0000 != 0 && self.reg[FBI_INIT0] & (1 << 3) != 0;
            (self.regnum(offset), if swizzle { data.swap_bytes() } else { data })
        } else {
            (self.regnum(offset), data)
        };
        let access = self.chip.access[regnum];
        if access & WRITE == 0 {
            return false;
        }
        // The command FIFO takes what goes through the memory FIFO. Glide
        // also writes swapbufferCMD here, which on the card only counts
        // the swap as pending; the swap itself comes through the command
        // FIFO, which runs it and counts it then.
        if access & FIFO != 0 && self.cmdfifo_on() {
            if regnum != SWAPBUFFER_CMD {
                self.log_once(LOG_IGNORED, "[3DFX] Register writes outside the command FIFO are ignored while it is on");
            }
            return false;
        }
        self.register_to(regnum, chips, data, now, effect);
        access & FIFO != 0
    }

    /// The register at dword `offset`: with address bit 21 and fbiInit3
    /// bit 0, the triangle registers in their aliased order.
    fn regnum(&self, offset: u32) -> usize {
        if offset & 0x800C0 == 0x80000 && self.alt_regmap() {
            ALIAS[(offset & 0x3F) as usize] as usize
        } else {
            (offset & 0xFF) as usize
        }
    }

    /// Register `regnum` of the chips `chips` selects written: from the
    /// window or a command FIFO packet.
    pub(crate) fn register_to(&mut self, regnum: usize, chips: u32, data: u32, now: Now, effect: &mut Effect) {
        let (fbi, tmu0, tmu1) = (chips & 1 != 0, chips & 2 != 0, chips & 4 != 0);
        let fixed = |bits: i32| float_to_int32(data, bits) as u32;
        match regnum {
            VERTEX_AX | FVERTEX_AX | VERTEX_AY | FVERTEX_AY | VERTEX_BX | FVERTEX_BX | VERTEX_BY | FVERTEX_BY
            | VERTEX_CX | FVERTEX_CX | VERTEX_CY | FVERTEX_CY => {
                let value = if regnum >= FVERTEX_AX { fixed(4) } else { data } as i16;
                if fbi {
                    let f = &mut self.fbi;
                    match regnum & !0x20 {
                        VERTEX_AX => f.ax = value,
                        VERTEX_AY => f.ay = value,
                        VERTEX_BX => f.bx = value,
                        VERTEX_BY => f.by = value,
                        VERTEX_CX => f.cx = value,
                        _ => f.cy = value,
                    }
                }
            }
            // Colours 12.12, Z 20.12.
            START_R | START_G | START_B | START_A | D_R_DX | D_G_DX | D_B_DX | D_A_DX | D_R_DY | D_G_DY | D_B_DY
            | D_A_DY | FSTART_R | FSTART_G | FSTART_B | FSTART_A | FD_R_DX | FD_G_DX | FD_B_DX | FD_A_DX | FD_R_DY
            | FD_G_DY | FD_B_DY | FD_A_DY => {
                let value = s24(if regnum >= FSTART_R { fixed(12) } else { data });
                if fbi {
                    let f = &mut self.fbi;
                    match regnum & !0x20 {
                        START_R => f.startr = value,
                        START_G => f.startg = value,
                        START_B => f.startb = value,
                        START_A => f.starta = value,
                        D_R_DX => f.drdx = value,
                        D_G_DX => f.dgdx = value,
                        D_B_DX => f.dbdx = value,
                        D_A_DX => f.dadx = value,
                        D_R_DY => f.drdy = value,
                        D_G_DY => f.dgdy = value,
                        D_B_DY => f.dbdy = value,
                        _ => f.dady = value,
                    }
                }
            }
            START_Z | D_Z_DX | D_Z_DY | FSTART_Z | FD_Z_DX | FD_Z_DY => {
                let value = if regnum >= FSTART_Z { fixed(12) } else { data } as i32;
                if fbi {
                    match regnum & !0x20 {
                        START_Z => self.fbi.startz = value,
                        D_Z_DX => self.fbi.dzdx = value,
                        _ => self.fbi.dzdy = value,
                    }
                }
            }
            // S and T 14.18 (as 16.32), W 2.30 (as 16.32).
            START_S | START_T | D_S_DX | D_T_DX | D_S_DY | D_T_DY | START_W | D_W_DX | D_W_DY | FSTART_S
            | FSTART_T | FD_S_DX | FD_T_DX | FD_S_DY | FD_T_DY | FSTART_W | FD_W_DX | FD_W_DY => {
                let base = regnum & !0x20;
                let is_w = matches!(base, START_W | D_W_DX | D_W_DY);
                let value = if regnum >= FSTART_R {
                    float_to_int64(data, 32)
                } else if is_w {
                    (data as i32 as i64) << 2
                } else {
                    (data as i32 as i64) << 14
                };
                if is_w && fbi {
                    match base {
                        START_W => self.fbi.startw = value,
                        D_W_DX => self.fbi.dwdx = value,
                        _ => self.fbi.dwdy = value,
                    }
                }
                for (unit, on) in [(0, tmu0), (1, tmu1)] {
                    if !on || unit >= self.tmu.len() {
                        continue;
                    }
                    let t = &mut self.tmu[unit];
                    match base {
                        START_S => t.starts = value,
                        START_T => t.startt = value,
                        D_S_DX => t.dsdx = value,
                        D_T_DX => t.dtdx = value,
                        D_S_DY => t.dsdy = value,
                        D_T_DY => t.dtdy = value,
                        START_W => t.startw = value,
                        D_W_DX => t.dwdx = value,
                        _ => t.dwdy = value,
                    }
                }
            }
            // The bits the chip has.
            FBZ_COLOR_PATH => {
                if fbi {
                    self.reg[regnum] = data & self.chip.fbz_color_path_mask;
                }
            }
            FBZ_MODE => {
                if fbi {
                    self.reg[regnum] = data & self.chip.fbz_mode_mask;
                }
            }
            FOG_MODE => {
                if fbi {
                    self.reg[regnum] = data & self.chip.fog_mode_mask;
                }
            }
            TRIANGLE_CMD | FTRIANGLE_CMD => self.triangle(),
            NOP_CMD => {
                if data & 1 != 0 {
                    self.reset_counters();
                }
                if data & 2 != 0 {
                    self.reg[FBI_TRIANGLES_OUT] = 0;
                }
            }
            FASTFILL_CMD => self.fastfill(),
            SWAPBUFFER_CMD => self.swap(data, now, effect),
            CLUT_DATA => {
                // Ignored while the video timing is held in reset.
                if fbi && self.reg[FBI_INIT1] & (1 << 8) == 0 {
                    let index = (data >> 24) as usize;
                    if index <= 32 {
                        self.clut[index] = data;
                        self.palette_dirty = true;
                        self.display_dirty = true;
                    }
                }
            }
            DAC_DATA => {
                if fbi {
                    let reg = ((data >> 8) & 7) as usize;
                    if data & 0x800 == 0 {
                        self.dac[reg] = data as u8;
                    } else {
                        self.dac_read = self.dac_register(reg);
                    }
                }
            }
            H_SYNC | V_SYNC | BACK_PORCH | VIDEO_DIMENSIONS => {
                if fbi {
                    self.reg[regnum] = data;
                    if self.reg[H_SYNC] != 0 && self.reg[V_SYNC] != 0 && self.reg[VIDEO_DIMENSIONS] != 0 {
                        let (xwidth, yheight) = fields(self.reg[VIDEO_DIMENSIONS], self.chip.video.dimensions);
                        let (width, height) = ((xwidth + 1) & !1, (yheight + 1) & !1);
                        if (width, height) != (self.fbi.width, self.fbi.height) {
                            (self.fbi.width, self.fbi.height) = (width, height);
                            self.display_dirty = true;
                            self.update_tiles();
                        }
                        if regnum == VIDEO_DIMENSIONS {
                            self.recompute_video_memory();
                        }
                        self.update_timing();
                    }
                }
            }
            FBI_INIT0 => {
                if fbi && self.init_writes() {
                    if (self.reg[FBI_INIT0] ^ data) & 1 != 0 {
                        self.display_dirty = true;
                    }
                    self.reg[FBI_INIT0] = data;
                    if data & 2 != 0 {
                        self.reset_counters();
                        self.reg[FBI_TRIANGLES_OUT] = 0;
                        // The graphics reset turns chroma-ranging off.
                        // 3dfx's Voodoo2 driver tests the TMUs with a
                        // range that passes one colour only
                        // (sst1InitCheckTmuMemConst), resets the
                        // graphics engine after it and doesn't write
                        // chromaRange again: kept, the range would drop
                        // every texel a Direct3D game draws with its
                        // colour key on.
                        self.reg[CHROMA_RANGE] = 0;
                        // It also shows buffer 0 and draws into buffer 1
                        // (86Box does the same). Glide counts on that:
                        // grSstWinOpen takes buffer 0 for the front and
                        // aims its blits at the buffer after it, and its
                        // open and shutdown pulse this reset, but only a
                        // swap left pending makes it read the displayed
                        // buffer and swap to 0 (sst1InitClearSwapPending).
                        // Kept, the buffers of a program that quit on an
                        // odd frame stay the other way round, and the next
                        // one blits into the buffer on the screen.
                        self.fbi.frontbuf = 0;
                        self.fbi.backbuf = 1;
                        self.display_dirty = true;
                    }
                    self.recompute_video_memory();
                }
            }
            FBI_INIT1 | FBI_INIT2 | FBI_INIT4 => {
                if fbi && self.init_writes() {
                    self.reg[regnum] = data;
                    self.recompute_video_memory();
                    self.display_dirty = true;
                }
            }
            FBI_INIT3 => {
                if fbi && self.init_writes() {
                    self.reg[regnum] = data;
                    self.fbi.yorigin = (data >> 22) & 0x3FF;
                    self.recompute_video_memory();
                }
            }
            FBI_INIT5 | FBI_INIT6 | FBI_INIT7 => {
                if fbi && self.init_writes() {
                    // fbiInit5's straps stay (86Box).
                    let keep = if regnum == FBI_INIT5 { 0x41E6 } else { 0 };
                    self.reg[regnum] = (data & !keep) | (self.reg[regnum] & keep);
                    self.recompute_video_memory();
                    self.display_dirty = true;
                    self.cmdfifo.enabled = self.reg[FBI_INIT7] & (1 << 8) != 0;
                    self.cmdfifo.count_holes = self.reg[FBI_INIT7] & (1 << 10) == 0;
                }
            }
            CMDFIFO_BASE_ADDR | CMDFIFO_RD_PTR | CMDFIFO_AMIN | CMDFIFO_AMAX | CMDFIFO_DEPTH | CMDFIFO_HOLES => {
                if fbi {
                    self.reg[regnum] = data;
                    self.cmdfifo_set(regnum, data);
                }
            }
            S_ARGB => self.setup_argb(data),
            S_DRAW_TRI_CMD => self.draw_triangle(),
            S_BEGIN_TRI_CMD => self.begin_triangle(),
            BLT_DATA => self.blit_data(data),
            BLT_DST_XY | BLT_SIZE | BLT_COMMAND => {
                self.reg[regnum] = data;
                if data & (1 << 31) != 0 {
                    self.blit_start();
                }
            }
            n if (NCC_TABLE..NCC_TABLE + 24).contains(&n) => {
                for (unit, on) in [(0, tmu0), (1, tmu1)] {
                    if on && unit < self.tmu.len() {
                        let (reg, tmu) = (&mut self.reg[..], &mut self.tmu[unit]);
                        tmu.ncc_write(reg, n - NCC_TABLE, data);
                    }
                }
            }
            n if (FOG_TABLE..FOG_TABLE + 32).contains(&n) => {
                if fbi {
                    let base = 2 * (n - FOG_TABLE);
                    self.fbi.fogdelta[base] = data as u8;
                    self.fbi.fogblend[base] = (data >> 8) as u8;
                    self.fbi.fogdelta[base + 1] = (data >> 16) as u8;
                    self.fbi.fogblend[base + 1] = (data >> 24) as u8;
                }
            }
            TEXTURE_MODE | T_LOD | T_DETAIL | TEX_BASE_ADDR | TEX_BASE_ADDR_1 | TEX_BASE_ADDR_2 | TEX_BASE_ADDR_3_8 => {
                for (unit, on) in [(0, tmu0), (1, tmu1)] {
                    if on && unit < self.tmu.len() {
                        self.reg[0x100 * (unit + 1) + regnum] = data;
                        self.tmu[unit].mark_dirty();
                    }
                }
            }
            TREX_INIT1 => {
                // TMU 0 hands out its configuration instead of texels.
                self.send_config = data & (1 << 18) != 0;
                self.store(regnum, data, chips);
            }
            _ => self.store(regnum, data, chips),
        }
    }

    /// Feed a register to the chips `chips` selects.
    fn store(&mut self, regnum: usize, data: u32, chips: u32) {
        for chip in 0..4 {
            if chips & (1 << chip) != 0 {
                self.reg[0x100 * chip + regnum] = data;
            }
        }
    }

    /// A DAC register read through dacData bit 11: the value written,
    /// except the ID a Glide probing for the DAC type looks for
    /// (`dacdata_r`).
    fn dac_register(&self, reg: usize) -> u8 {
        if reg == 5 {
            match self.dac[7] {
                0x01 => 0x55,
                0x07 => 0x71,
                0x0B => 0x79,
                _ => 0xFF,
            }
        } else {
            self.dac[reg]
        }
    }

    fn reset_counters(&mut self) {
        self.flush();
        self.pool.reset_stats();
        for r in [FBI_PIXELS_IN, FBI_CHROMA_FAIL, FBI_ZFUNC_FAIL, FBI_AFUNC_FAIL, FBI_PIXELS_OUT] {
            self.reg[r] = 0;
        }
        self.stats = Default::default();
    }

    /// Register read at dword `offset` of the window (`register_r`).
    pub(crate) fn register_read(&self, offset: u32, now: Now) -> u32 {
        let regnum = (offset & 0xFF) as usize;
        if self.chip.access[regnum] & READ == 0 {
            return 0xFFFF_FFFF;
        }
        // A program that reads the counters wants every pixel counted:
        // no more dropping held-back jobs.
        let counted = || {
            self.counters_read.set(true);
            self.counted()
        };
        let counter = |r: usize, n: i32| self.reg[r].wrapping_add(n as u32) & 0xFF_FFFF;
        match regnum {
            STATUS => self.status(now),
            V_RETRACE => self.scanline(now) & 0x1FFF,
            HV_RETRACE => self.hv_retrace(now),
            CMDFIFO_RD_PTR => self.cmdfifo.read,
            CMDFIFO_AMIN => self.cmdfifo.amin,
            CMDFIFO_AMAX => self.cmdfifo.amax,
            CMDFIFO_DEPTH => self.cmdfifo.depth,
            CMDFIFO_HOLES => self.cmdfifo.holes,
            // The board's ID straps read 0 (86Box's reads).
            FBI_INIT5 => self.reg[regnum] & !0x1FF,
            FBI_INIT7 => self.reg[regnum] & !0xFF,
            // initEnable bit 2 reads the DAC through fbiInit2.
            FBI_INIT2 if self.pci.init_enable & 4 != 0 => self.dac_read as u32,
            FBI_PIXELS_IN => counter(regnum, counted().pixels_in),
            FBI_CHROMA_FAIL => counter(regnum, counted().chroma_fail),
            FBI_ZFUNC_FAIL => counter(regnum, counted().zfunc_fail),
            FBI_AFUNC_FAIL => counter(regnum, counted().afunc_fail),
            FBI_PIXELS_OUT => counter(regnum, counted().pixels_out),
            _ => self.reg[regnum],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::regs::*;
    use super::super::{Board, Now, Voodoo};

    fn reg(v: &mut Voodoo, r: usize, data: u32) {
        v.write((r as u32) << 2, data, !0, Now::default());
    }

    /// Draws a red triangle over pixel (1, 1) of the front buffer and
    /// returns that pixel.
    fn red_triangle(v: &mut Voodoo) -> u16 {
        for (r, value) in [
            (START_R, 255 << 12),
            (START_G, 0),
            (START_B, 0),
            (VERTEX_AX, 0),
            (VERTEX_AY, 0),
            (VERTEX_BX, 16 << 4),
            (VERTEX_BY, 0),
            (VERTEX_CX, 0),
            (VERTEX_CY, 16 << 4),
            (TRIANGLE_CMD, 0),
        ] {
            reg(v, r, value);
        }
        v.frame_buffer().get(640 + 1)
    }

    #[test]
    fn a_graphics_reset_turns_chroma_ranging_off() {
        let mut v = Voodoo::with_workers(Board::Voodoo2Max, 0);
        v.config_write(0x40, 1);
        reg(&mut v, FBI_INIT1, 10 << 4);
        reg(&mut v, FBI_INIT2, 150 << 11);
        reg(&mut v, CLIP_LEFT_RIGHT, 640);
        reg(&mut v, CLIP_LOW_Y_HIGH_Y, 480);
        reg(&mut v, FBZ_MODE, 1 << 9 | 1 << 1);
        // The last colour of 3dfx's TMU test: only A6A6A6h passes, as
        // every colour component outside its range blocks the pixel.
        reg(&mut v, CHROMA_KEY, 0xA6A6A6);
        reg(&mut v, CHROMA_RANGE, 0x1FA6_A6A6);
        assert_eq!(red_triangle(&mut v), 0, "red is outside the range");
        let chroma_fail = v.read((FBI_CHROMA_FAIL as u32) << 2, Now::default());
        assert!(chroma_fail > 0, "the chroma test blocked the triangle");
        // The driver's reset after the test (sst1InitResetFbi).
        for data in [0x5E, 0x5A, 0x58] {
            reg(&mut v, FBI_INIT0, data);
        }
        assert_eq!(v.reg[CHROMA_RANGE], 0);
        assert_eq!(red_triangle(&mut v), 0xF800);
    }

    #[test]
    fn a_graphics_reset_shows_buffer_0_and_draws_into_buffer_1() {
        // Moto Racer quit on an odd frame, so the buffers were the
        // other way round, and run again: Glide's shutdown and open
        // pulse the graphics reset, take buffer 0 for the front, and
        // blit the sky into the buffer after it. The sky has to land in
        // the buffer the frame's triangles go to.
        let mut v = Voodoo::with_workers(Board::Voodoo2Max, 0);
        v.config_write(0x40, 1);
        reg(&mut v, FBI_INIT1, 10 << 4);
        reg(&mut v, FBI_INIT2, 150 << 11);
        reg(&mut v, SWAPBUFFER_CMD, 0);
        assert_eq!((v.fbi.frontbuf, v.fbi.backbuf), (1, 0));
        // grGlideShutdown, then the next program's grSstWinOpen.
        for _ in 0..2 {
            for data in [0x5E, 0x5A, 0x58] {
                reg(&mut v, FBI_INIT0, data);
            }
        }
        assert_eq!((v.fbi.frontbuf, v.fbi.backbuf), (0, 1));
        // A frame: a fill of the back buffer, then a rectangle fill of
        // the blitter at the page Glide counts as the back buffer's.
        reg(&mut v, CLIP_LEFT_RIGHT, 640);
        reg(&mut v, CLIP_LOW_Y_HIGH_Y, 480);
        reg(&mut v, FBZ_MODE, 1 << 9 | 1 << 14);
        reg(&mut v, COLOR1, 0x0000_00FF);
        reg(&mut v, FASTFILL_CMD, 0);
        reg(&mut v, BLT_DST_BASE_ADDR, 150 << 12);
        reg(&mut v, BLT_XY_STRIDES, 1280 << 16);
        reg(&mut v, BLT_ROP, 0xCCCC);
        reg(&mut v, BLT_COLOR, 0xF800);
        reg(&mut v, BLT_DST_XY, 99 << 16);
        reg(&mut v, BLT_SIZE, 0);
        reg(&mut v, BLT_COMMAND, 1 << 31 | 2);
        let back = 150 * 0x1000 / 2;
        let row = 99 * 640;
        assert_eq!(v.frame_buffer().get(back), 0x001F, "the fill drew the back buffer");
        assert_eq!(v.frame_buffer().get(back + row), 0xF800, "the blit drew there too");
        assert_eq!(v.frame_buffer().get(row), 0, "nothing reached the front buffer");
    }

    #[test]
    fn eight_bit_downloads_follow_the_units_own_texture_mode() {
        // Moto Racer's Glide sets textureMode bit 31 (sequential 8-bit
        // downloads) in TMU 1 alone before it downloads a P8 texture
        // there, while TMU 0 keeps the mode it draws with.
        let mut v = Voodoo::with_workers(Board::Voodoo2Max, 0);
        v.config_write(0x40, 1);
        let (trex0, trex1) = (2 << 8, 4 << 8);
        reg(&mut v, trex0 | TEXTURE_MODE, 5 << 8);
        reg(&mut v, trex1 | TEXTURE_MODE, 1 << 31 | 5 << 8);
        // Dword 1 of row 0 of level 0: texels 4 to 7, one after another.
        let tmu1 = 0xA0_0000;
        v.write(tmu1 + 4, 0x4433_2211, !0, Now::default());
        let ram = &v.tmu[1].ram;
        assert_eq!([4, 5, 6, 7].map(|at| ram.byte(at)), [0x11, 0x22, 0x33, 0x44]);
        // Without the bit, TMU 0 takes 8-bit texels at even dwords only:
        // dword 1 is texels 0 to 3, as dword 0 is.
        v.write(0x80_0000 + 4, 0x4433_2211, !0, Now::default());
        let ram = &v.tmu[0].ram;
        assert_eq!([0, 1, 2, 3].map(|at| ram.byte(at)), [0x11, 0x22, 0x33, 0x44]);
        // The other way round, TMU 0's bit doesn't reach TMU 1: dword 1
        // of row 1 is that row's texels 0 to 3.
        reg(&mut v, trex0 | TEXTURE_MODE, 1 << 31 | 5 << 8);
        reg(&mut v, trex1 | TEXTURE_MODE, 5 << 8);
        v.write(tmu1 + (1 << 9 | 4), 0x8877_6655, !0, Now::default());
        let ram = &v.tmu[1].ram;
        let row = 256;
        let texels = [0, 1, 2, 3].map(|at| ram.byte(row + at));
        assert_eq!(texels, [0x55, 0x66, 0x77, 0x88]);
    }
}
