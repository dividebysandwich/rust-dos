//! Register writes and reads: the triangle parameters in their fixed and
//! float formats, the modes, the commands, the init registers and the
//! DAC behind them, the video timing, the NCC and fog tables. From
//! DOSBox-X's `register_w` and `register_r` (voodoo_emu.cpp); the gamma
//! table and vRetrace as MAME has them.

use super::regs::*;
use super::tables::{float_to_int32, float_to_int64};
use super::{Effect, Now, Voodoo};

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
        let regnum = if offset & 0x800C0 == 0x80000 && self.alt_regmap() {
            ALIAS[(offset & 0x3F) as usize] as usize
        } else {
            (offset & 0xFF) as usize
        };
        let access = ACCESS[regnum];
        if access & WRITE == 0 {
            return false;
        }
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
            // Bits a Voodoo Graphics hasn't.
            FBZ_COLOR_PATH => {
                if fbi {
                    self.reg[regnum] = data & 0x0FFF_FFFF;
                }
            }
            FBZ_MODE => {
                if fbi {
                    self.reg[regnum] = data & 0x001F_FFFF;
                }
            }
            FOG_MODE => {
                if fbi {
                    self.reg[regnum] = data & 0x3F;
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
                        let dims = self.reg[VIDEO_DIMENSIONS];
                        let (width, height) = (((dims & 0x3FF) + 1) & !1, (((dims >> 16) & 0x3FF) + 1) & !1);
                        if (width, height) != (self.fbi.width, self.fbi.height) {
                            (self.fbi.width, self.fbi.height) = (width, height);
                            self.display_dirty = true;
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
        access & FIFO != 0
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
        for r in [FBI_PIXELS_IN, FBI_CHROMA_FAIL, FBI_ZFUNC_FAIL, FBI_AFUNC_FAIL, FBI_PIXELS_OUT] {
            self.reg[r] = 0;
        }
        self.stats = Default::default();
    }

    /// Register read at dword `offset` of the window (`register_r`).
    pub(crate) fn register_read(&self, offset: u32, now: Now) -> u32 {
        let regnum = (offset & 0xFF) as usize;
        if ACCESS[regnum] & READ == 0 {
            return 0xFFFF_FFFF;
        }
        let s = &self.stats;
        let counter = |r: usize, n: i32| self.reg[r].wrapping_add(n as u32) & 0xFF_FFFF;
        match regnum {
            STATUS => self.status(now),
            V_RETRACE => self.scanline(now) & 0x1FFF,
            // initEnable bit 2 reads the DAC through fbiInit2.
            FBI_INIT2 if self.pci.init_enable & 4 != 0 => self.dac_read as u32,
            FBI_PIXELS_IN => counter(regnum, s.pixels_in),
            FBI_CHROMA_FAIL => counter(regnum, s.chroma_fail),
            FBI_ZFUNC_FAIL => counter(regnum, s.zfunc_fail),
            FBI_AFUNC_FAIL => counter(regnum, s.afunc_fail),
            FBI_PIXELS_OUT => counter(regnum, s.pixels_out),
            _ => self.reg[regnum],
        }
    }
}
