//! The texture units (TMUs): their RAM, the layout of a texture's
//! mipmap levels in it from the texture registers, the NCC (YIQ) tables
//! and the palette, and textures written by the host. From DOSBox-X's
//! voodoo_emu.cpp: `init_tmu`, `recompute_texture_params`, `prepare_tmu`,
//! `ncc_table_write`, `ncc_table_update` and `texture_w`; a Voodoo2's
//! 8-bit palette of 6-bit ARGB (format 6) and its separate base addresses
//! for the first mipmap levels as MAME's `recompute_texture_params` has
//! them.

use super::mem::Vram;
use super::regs::*;
use super::tables::{argb, tables};
use std::sync::Arc;

/// Where texture addresses start and how far they reach (texBaseAddr is
/// in units of 8 bytes).
const TEXADDR_MASK: u32 = 0x0F_FFFF;
const TEXADDR_SHIFT: u32 = 3;

/// One of a TMU's two NCC tables: 16 intensities (Y) and 4 each of the
/// I and Q colour offsets, and the 256 colours they make.
#[derive(Clone, Debug)]
pub struct Ncc {
    y: [i32; 16],
    ir: [i32; 4],
    ig: [i32; 4],
    ib: [i32; 4],
    qr: [i32; 4],
    qg: [i32; 4],
    qb: [i32; 4],
    pub texel: Arc<[u32; 256]>,
    dirty: bool,
    /// Counts the changes to the table (`Tmu::lookup_key`).
    pub changes: u32,
}

impl Default for Ncc {
    fn default() -> Self {
        Self {
            y: [0; 16],
            ir: [0; 4],
            ig: [0; 4],
            ib: [0; 4],
            qr: [0; 4],
            qg: [0; 4],
            qb: [0; 4],
            texel: Arc::new([0; 256]),
            dirty: true,
            changes: 0,
        }
    }
}

crate::state_fields!(Ncc { y, ir, ig, ib, qr, qg, qb } skip { texel, dirty, changes });

impl Ncc {
    /// Register `n` (0-11) of the table was written with `data`, which the
    /// card's registers already hold.
    fn set(&mut self, n: usize, data: u32) {
        if n < 4 {
            for i in 0..4 {
                self.y[n * 4 + i] = (data >> (8 * i) & 0xFF) as i32;
            }
        } else {
            let (r, g, b) = (((data << 5) as i32) >> 23, ((data << 14) as i32) >> 23, ((data << 23) as i32) >> 23);
            let i = n & 3;
            if n < 8 {
                (self.ir[i], self.ig[i], self.ib[i]) = (r, g, b);
            } else {
                (self.qr[i], self.qg[i], self.qb[i]) = (r, g, b);
            }
        }
        self.dirty = true;
        self.changes = self.changes.wrapping_add(1);
    }

    /// Work the 256 colours out again if the table changed.
    fn update(&mut self) {
        if !self.dirty {
            return;
        }
        let texel = Arc::make_mut(&mut self.texel);
        for (i, t) in texel.iter_mut().enumerate() {
            let (vi, vq) = ((i >> 2) & 3, i & 3);
            let y = self.y[(i >> 4) & 0x0F];
            let r = (y + self.ir[vi] + self.qr[vq]).clamp(0, 255) as u32;
            let g = (y + self.ig[vi] + self.qg[vq]).clamp(0, 255) as u32;
            let b = (y + self.ib[vi] + self.qb[vq]).clamp(0, 255) as u32;
            *t = argb(0xFF, r, g, b);
        }
        self.dirty = false;
    }

    /// Everything worked out from the registers again, after a load.
    fn rebuild(&mut self) {
        self.dirty = true;
        self.changes = self.changes.wrapping_add(1);
        self.update();
    }
}

/// A texture unit.
#[derive(Clone, Debug)]
pub struct Tmu {
    pub ram: Vram,
    /// Byte mask of the RAM.
    pub mask: u32,
    /// Where its registers are in the card's (100h for TMU 0, 200h for 1).
    pub base: usize,
    /// The texture registers changed since the layout was worked out.
    regdirty: bool,

    /// The triangle's S, T (16.32) and W (2.30 as 16.32) and their deltas.
    pub starts: i64,
    pub startt: i64,
    pub startw: i64,
    pub dsdx: i64,
    pub dtdx: i64,
    pub dwdx: i64,
    pub dsdy: i64,
    pub dtdy: i64,
    pub dwdy: i64,

    /// The texture's layout, from textureMode, tLOD, tDetail and
    /// texBaseAddr.
    pub lodmin: i32,
    pub lodmax: i32,
    pub lodbias: i32,
    pub lodmask: u32,
    pub lodoffset: [u32; 9],
    pub detailmax: i32,
    pub detailbias: i32,
    pub detailscale: u32,
    pub wmask: u32,
    pub hmask: u32,

    pub ncc: [Ncc; 2],
    /// The 256 colours of the P8 and AP88 formats, written through the
    /// NCC table registers with bit 31 set, and the same entries as 6-bit
    /// ARGB for a Voodoo2's format 6.
    pub palette: Arc<[u32; 256]>,
    pub palette6666: Arc<[u32; 256]>,
    /// Whether it is a Voodoo2's, which has format 6 and tLOD bit 24,
    /// and whether texBaseAddr is in bytes, as on a Banshee, rather than
    /// in units of 8 bytes.
    pub v2: bool,
    pub byte_base: bool,

    /// For the OpenGL renderer's decoded textures: the palette's changes,
    /// the writes to the RAM, and for each 4 KB page of it the count of
    /// writes at its last.
    pub palette_changes: u32,
    pub writes: u32,
    pub page_writes: Vec<u32>,

    /// Copies of the RAM it had (`detach`), with its `writes` when it had
    /// them: jobs read them, or did.
    spares: Vec<(Vram, u32)>,
}

/// The pages `Tmu::page_writes` counts in.
pub const PAGE_SHIFT: u32 = 12;

crate::state_fields!(Tmu {
    starts, startt, startw, dsdx, dtdx, dwdx, dsdy, dtdy, dwdy, ncc, palette,
} skip {
    ram, mask, base, regdirty, palette6666, v2, byte_base,
    lodmin, lodmax, lodbias, lodmask, lodoffset, detailmax, detailbias, detailscale, wmask, hmask,
    palette_changes, writes, page_writes, spares,
});

impl Tmu {
    pub fn new(bytes: usize, base: usize) -> Self {
        Self::with_ram(Vram::new(bytes), base)
    }

    /// A texture unit on `ram`, which may be another's memory (a
    /// Banshee's units share the frame buffer's).
    pub fn with_ram(ram: Vram, base: usize) -> Self {
        let bytes = ram.bytes();
        Self {
            ram,
            mask: bytes as u32 - 1,
            base,
            regdirty: true,
            starts: 0,
            startt: 0,
            startw: 0,
            dsdx: 0,
            dtdx: 0,
            dwdx: 0,
            dsdy: 0,
            dtdy: 0,
            dwdy: 0,
            lodmin: 0,
            lodmax: 0,
            lodbias: 0,
            lodmask: 0,
            lodoffset: [0; 9],
            detailmax: 0,
            detailbias: 0,
            detailscale: 0,
            wmask: 0,
            hmask: 0,
            ncc: [Ncc::default(), Ncc::default()],
            palette: Arc::new([0; 256]),
            palette6666: Arc::new([0; 256]),
            v2: false,
            byte_base: false,
            palette_changes: 0,
            writes: 0,
            page_writes: vec![0; bytes.div_ceil(1 << PAGE_SHIFT)],
            spares: Vec::new(),
        }
    }

    /// As at power-on, keeping what the RAM holds. The palette and the
    /// NCC tables count on from their changes before.
    pub fn reset(&mut self) {
        let changes = [self.ncc[0].changes, self.ncc[1].changes, self.palette_changes].map(|c| c.wrapping_add(1));
        let fresh = Self {
            ram: self.ram.clone(),
            mask: self.mask,
            base: self.base,
            v2: self.v2,
            byte_base: self.byte_base,
            palette_changes: changes[2],
            writes: self.writes,
            page_writes: std::mem::take(&mut self.page_writes),
            ..Self::new(2, self.base)
        };
        *self = fresh;
        self.ncc[0].changes = changes[0];
        self.ncc[1].changes = changes[1];
    }

    pub fn mark_dirty(&mut self) {
        self.regdirty = true;
    }

    /// After a load: the layout and the NCC colours from the registers.
    pub fn rebuild(&mut self, reg: &[u32]) {
        self.recompute(reg);
        for ncc in &mut self.ncc {
            ncc.rebuild();
        }
        self.palette6666 = Arc::new(self.palette.map(argb6666));
        // The RAM and the palette came from the state.
        self.palette_changes = self.palette_changes.wrapping_add(1);
        self.writes = self.writes.wrapping_add(1);
        self.page_writes.fill(self.writes);
        self.spares.clear();
    }

    /// Leave the RAM as it is to the jobs that read it, and write from now
    /// on in other memory with the same words: a copy it had before that
    /// no job reads any more, brought up to date by the pages written
    /// since (`page_writes`), or a new one while it has few. False, with
    /// nothing changed, when jobs read all of the copies it keeps: the
    /// caller waits for the jobs instead, which gives all of them back.
    ///
    /// # Safety
    ///
    /// Nothing but `write` on this thread writes the RAM.
    pub unsafe fn detach(&mut self) -> bool {
        /// The most copies it keeps, the RAM jobs read included.
        const COPIES: usize = 8;
        let writes = self.writes;
        // Beyond that the counts can't tell what was written since.
        self.spares.retain(|&(_, at)| writes.wrapping_sub(at) < 1 << 30);
        let mut free = None;
        for (i, (ram, at)) in self.spares.iter_mut().enumerate() {
            let newer = free.is_none_or(|(_, newest): (usize, u32)| {
                writes.wrapping_sub(*at) < writes.wrapping_sub(newest)
            });
            if newer && ram.unique() {
                free = Some((i, *at));
            }
        }
        let fresh = match free {
            Some((i, at)) => {
                let (ram, _) = self.spares.swap_remove(i);
                let words = 1usize << (PAGE_SHIFT - 1);
                for (page, &w) in self.page_writes.iter().enumerate() {
                    if w.wrapping_sub(at) as i32 > 0 {
                        let start = (page * words).min(ram.len());
                        let end = (start + words).min(ram.len());
                        // SAFETY: `ram` is ours alone (`unique`), and the
                        // caller keeps other writers out of the RAM.
                        unsafe { ram.copy_words(&self.ram, start..end) };
                    }
                }
                ram
            }
            None if self.spares.len() + 1 >= COPIES => return false,
            // SAFETY: the caller keeps other writers out of the RAM.
            None => unsafe { self.ram.copy() },
        };
        let old = std::mem::replace(&mut self.ram, fresh);
        self.spares.push((old, writes));
        true
    }

    /// A write to NCC table register `n` (0-23: table 0's twelve, then
    /// table 1's). With bit 31 set, an I or Q register of table 0 writes a
    /// palette entry instead.
    pub fn ncc_write(&mut self, reg: &mut [u32], n: usize, data: u32) {
        let (table, index) = (n / 12, n % 12);
        if index >= 4 && data & 0x8000_0000 != 0 && table == 0 {
            let entry = ((data >> 23) & 0xFE) as usize | (index & 1);
            let color = 0xFF00_0000 | data;
            if self.palette[entry] != color {
                Arc::make_mut(&mut self.palette)[entry] = color;
                Arc::make_mut(&mut self.palette6666)[entry] = argb6666(color);
                self.palette_changes = self.palette_changes.wrapping_add(1);
            }
            return;
        }
        let at = self.base + NCC_TABLE + n;
        if reg[at] == data {
            return;
        }
        reg[at] = data;
        self.ncc[table].set(index, data);
    }

    /// Work out the texture's layout from the registers
    /// (`recompute_texture_params`).
    fn recompute(&mut self, reg: &[u32]) {
        let (mode, lod, detail) = (reg[self.base + TEXTURE_MODE], reg[self.base + T_LOD], reg[self.base + T_DETAIL]);
        self.lodmin = ((lod & 0x3F) << 6) as i32;
        self.lodmax = (((lod >> 6) & 0x3F) << 6) as i32;
        self.lodbias = ((((lod >> 12) & 0x3F) << 2) as u8 as i8 as i32) << 4;

        // Which levels the texture has: all, or with the LOD split the odd
        // or the even ones.
        self.lodmask = 0x1FF;
        if lod & (1 << 19) != 0 {
            self.lodmask = if lod & (1 << 18) == 0 { 0x155 } else { 0x0AA };
        }

        // The base level's size: 256 texels wide and high, less on the
        // narrow side of the aspect ratio.
        self.wmask = 0xFF;
        self.hmask = 0xFF;
        let aspect = (lod >> 21) & 3;
        if lod & (1 << 20) != 0 {
            self.hmask >>= aspect;
        } else {
            self.wmask >>= aspect;
        }

        let bppscale = ((mode >> 8) & 0xF) >> 3;
        let address = |r: usize| {
            if self.byte_base {
                reg[self.base + r] & 0xFF_FFF0
            } else {
                (reg[self.base + r] & TEXADDR_MASK) << TEXADDR_SHIFT
            }
        };
        let mut base = address(TEX_BASE_ADDR);
        self.lodoffset[0] = base & self.mask;
        // A Voodoo2 with tLOD bit 24 has levels 1 and 2 and from 3 on at
        // addresses of their own; MAME leaves it off where bits 31:28 are
        // set, which games leave so without meaning it. Otherwise the
        // levels follow each other.
        let multibase = self.v2 && lod & (1 << 24) != 0 && lod >> 28 == 0;
        for level in 1..=8usize {
            if multibase && level <= 3 {
                base = address([TEX_BASE_ADDR_1, TEX_BASE_ADDR_2, TEX_BASE_ADDR_3_8][level - 1]);
            } else if self.lodmask & (1 << (level - 1)) != 0 {
                let mut size = ((self.wmask >> (level - 1)) + 1) * ((self.hmask >> (level - 1)) + 1);
                if level >= 4 {
                    size = size.max(4);
                }
                base = base.wrapping_add(size << bppscale);
            }
            self.lodoffset[level] = base & self.mask;
        }

        self.detailmax = (detail & 0xFF) as i32;
        self.detailbias = ((((detail >> 8) & 0x3F) << 2) as u8 as i8 as i32) << 6;
        self.detailscale = (detail >> 14) & 7;
        self.regdirty = false;
    }

    /// Which colours `lookup` gives for `mode`, as they are now: the same
    /// key, the same colours.
    pub fn lookup_key(&self, mode: u32) -> u64 {
        let table = (mode >> 5) & 1;
        match (mode >> 8) & 0xF {
            1 | 9 => 1 << 40 | (table as u64) << 32 | self.ncc[table as usize].changes as u64,
            5 | 14 => 2 << 40 | self.palette_changes as u64,
            6 if self.v2 => 3 << 40 | self.palette_changes as u64,
            _ => 0,
        }
    }

    /// The colours the texel values of the current format stand for.
    pub fn lookup(&self, mode: u32) -> Arc<[u32]> {
        let t = tables();
        let ncc = &self.ncc[((mode >> 5) & 1) as usize];
        match (mode >> 8) & 0xF {
            0 | 8 => t.rgb332.clone(),
            1 | 9 => ncc.texel.clone(),
            2 => t.alpha8.clone(),
            3 | 13 => t.int8.clone(),
            4 => t.ai44.clone(),
            5 | 14 => self.palette.clone(),
            6 if self.v2 => self.palette6666.clone(),
            10 => t.rgb565.clone(),
            11 => t.argb1555.clone(),
            12 => t.argb4444.clone(),
            _ => t.none.clone(),
        }
    }

    /// Get ready to draw a triangle with this texture: its layout and NCC
    /// colours up to date, and the base level of detail from how fast S
    /// and T change across the screen (`prepare_tmu`).
    pub fn prepare(&mut self, reg: &[u32]) -> i32 {
        if self.regdirty {
            self.recompute(reg);
        }
        let mode = reg[self.base + TEXTURE_MODE];
        if (mode >> 8) & 7 == 1 {
            self.ncc[((mode >> 5) & 1) as usize].update();
        }
        let sq = |d: i64| (d >> 14).wrapping_mul(d >> 14);
        let texdx = sq(self.dsdx).wrapping_add(sq(self.dtdx));
        let texdy = sq(self.dsdy).wrapping_add(sq(self.dtdy));
        let texd = texdx.max(texdy) >> 16;
        let (_, lodbase) = tables().reciplog(texd);
        (-lodbase + (12 << 8)) / 2
    }

    /// The host wrote `data` to the texture RAM at dword `offset` of the
    /// card's texture space (`texture_w`): four 8-bit texels or two 16-bit
    /// ones of level `offset >> 15`, row `(offset >> 7) & FFh`.
    /// `seq_8_download` is the unit's textureMode bit 31. DOSBox-X and
    /// MAME take TMU 0's for both units, but each TREX lays out what it is
    /// sent by its own register, as 86Box has it, and Glide sets the bit
    /// in the unit it downloads to.
    pub fn write(&mut self, reg: &[u32], offset: u32, mut data: u32, seq_8_download: bool) {
        if self.regdirty {
            self.recompute(reg);
        }
        let lod_reg = reg[self.base + T_LOD];
        if lod_reg & (1 << 25) != 0 {
            data = data.swap_bytes();
        }
        if lod_reg & (1 << 26) != 0 {
            data = data.rotate_left(16);
        }
        let lod = ((offset >> 15) & 0x0F) as usize;
        if lod > 8 {
            return;
        }
        let tt = (offset >> 7) & 0xFF;
        let width = (self.wmask >> lod) + 1;
        if (reg[self.base + TEXTURE_MODE] >> 8) & 0xF < 8 {
            let ts = if seq_8_download { (offset << 2) & 0xFC } else { (offset << 1) & 0xFC };
            let at = self.lodoffset[lod].wrapping_add(tt * width + ts) & self.mask;
            for i in 0..4 {
                self.ram.set_byte((at.wrapping_add(i) & self.mask) as usize, (data >> (8 * i)) as u8);
            }
            self.count_write(at, at.wrapping_add(3) & self.mask);
        } else {
            let ts = (offset << 1) & 0xFE;
            let at = (self.lodoffset[lod].wrapping_add(2 * (tt * width + ts)) & self.mask) >> 1;
            let words = self.mask >> 1;
            self.ram.set((at & words) as usize, data as u16);
            self.ram.set((at.wrapping_add(1) & words) as usize, (data >> 16) as u16);
            self.count_write((at & words) * 2, (at.wrapping_add(1) & words) * 2 + 1);
        }
    }

    /// Bytes `start..end` of the RAM were written other than through
    /// `write`: a Banshee's units read the frame buffer's memory, which
    /// the CPU, the 2D engine and the command FIFOs write as well.
    pub fn note_written(&mut self, start: u32, end: u32) {
        if start >= end {
            return;
        }
        self.writes = self.writes.wrapping_add(1);
        let last = ((end - 1) >> PAGE_SHIFT) as usize;
        let pages = (start >> PAGE_SHIFT) as usize..=last.min(self.page_writes.len() - 1);
        for page in self.page_writes.get_mut(pages).into_iter().flatten() {
            *page = self.writes;
        }
    }

    /// The bytes `first` and `last` were written.
    fn count_write(&mut self, first: u32, last: u32) {
        self.writes = self.writes.wrapping_add(1);
        for at in [first, last] {
            if let Some(page) = self.page_writes.get_mut((at >> PAGE_SHIFT) as usize) {
                *page = self.writes;
            }
        }
    }
}

/// A palette entry's 6-bit alpha, red, green and blue (bits 23:18, 17:12,
/// 11:6 and 5:0) as 8-bit ARGB.
fn argb6666(entry: u32) -> u32 {
    let c = |shift: u32| {
        let v = (entry >> shift) & 0x3F;
        (v << 2) | (v >> 4)
    };
    argb(c(18), c(12), c(6), c(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regs() -> Vec<u32> {
        vec![0; 0x400]
    }

    #[test]
    fn layout_of_a_256_square_16_bit_texture() {
        let mut reg = regs();
        let mut tmu = Tmu::new(2 << 20, 0x100);
        reg[0x100 + TEXTURE_MODE] = 10 << 8; // RGB565
        reg[0x100 + T_LOD] = 8 << 8; // levels 0 to 8
        reg[0x100 + TEX_BASE_ADDR] = 0x100; // 800h bytes
        tmu.recompute(&reg);
        assert_eq!((tmu.wmask, tmu.hmask), (0xFF, 0xFF));
        assert_eq!(tmu.lodoffset[0], 0x800);
        assert_eq!(tmu.lodoffset[1], 0x800 + 256 * 256 * 2);
        assert_eq!(tmu.lodoffset[2], 0x800 + 256 * 256 * 2 + 128 * 128 * 2);
        assert_eq!((tmu.lodmin, tmu.lodmax), (0, 8 << 8));
    }

    #[test]
    fn texture_writes_follow_the_layout() {
        let mut reg = regs();
        let mut tmu = Tmu::new(2 << 20, 0x100);
        reg[0x100 + TEXTURE_MODE] = 10 << 8;
        // Level 0, row 1, texels 2 and 3: dword offset row << 7 | s / 2.
        tmu.write(&reg, 1 << 7 | 1, 0xBBBB_AAAA, false);
        let row = 256 * 2;
        assert_eq!(tmu.ram.get((row + 4) / 2), 0xAAAA);
        assert_eq!(tmu.ram.get((row + 6) / 2), 0xBBBB);
        // 8-bit: four texels a dword.
        reg[0x100 + TEXTURE_MODE] = 3 << 8;
        tmu.mark_dirty();
        tmu.write(&reg, 2 << 7 | 2, 0x4433_2211, false);
        let at = 2 * 256 + 4;
        assert_eq!([tmu.ram.byte(at), tmu.ram.byte(at + 3)], [0x11, 0x44]);
    }

    #[test]
    fn detached_ram_leaves_jobs_theirs_and_comes_back_up_to_date() {
        let mut reg = regs();
        let mut tmu = Tmu::new(2 << 20, 0x100);
        reg[0x100 + TEXTURE_MODE] = 10 << 8;
        tmu.write(&reg, 0, 0x1111_1111, false);
        let job = tmu.ram.clone();
        // SAFETY: only this thread writes the RAM.
        assert!(unsafe { tmu.detach() });
        tmu.write(&reg, 0, 0x2222_2222, false);
        assert_eq!((job.get(0), tmu.ram.get(0)), (0x1111, 0x2222));
        // The job is done: the next detach takes its memory back, with
        // the pages written since, one of them far from the first.
        drop(job);
        tmu.write(&reg, 1 << 7, 0x3333_3333, false);
        tmu.write(&reg, 100 << 7, 0x4444_4444, false);
        let job = tmu.ram.clone();
        // SAFETY: as above.
        assert!(unsafe { tmu.detach() });
        assert_eq!(tmu.spares.len(), 1, "the first memory taken back");
        assert!(!tmu.ram.same(&job));
        assert_eq!(tmu.ram.to_bytes(), job.to_bytes());
        assert_eq!(
            [tmu.ram.get(0), tmu.ram.get(256), tmu.ram.get(100 * 256)],
            [0x2222, 0x3333, 0x4444]
        );
    }

    #[test]
    fn ncc_tables_and_palette() {
        let mut reg = regs();
        let mut tmu = Tmu::new(2 << 20, 0x100);
        // Y0 = 100, all others 0: texel 0 is grey 100.
        tmu.ncc_write(&mut reg, 0, 100);
        tmu.ncc[0].update();
        assert_eq!(tmu.ncc[0].texel[0], 0xFF64_6464);
        // I0 red +50 (9 bits at 26:18).
        tmu.ncc_write(&mut reg, 4, 50 << 18);
        tmu.ncc[0].update();
        assert_eq!(tmu.ncc[0].texel[0], 0xFF96_6464);
        // A palette write: entry (bits 30:24) << 1 | the register's parity.
        tmu.ncc_write(&mut reg, 5, 0x8000_0000 | 3 << 24 | 0x12_3456);
        assert_eq!(tmu.palette[7], 0xFF12_3456);
        // As 6-bit ARGB: 04h, 23h, 11h and 16h.
        assert_eq!(tmu.palette6666[7], 0x10_8E_45_59);
    }

    #[test]
    fn multiple_base_addresses_on_a_voodoo2_only() {
        let mut reg = regs();
        let mut tmu = Tmu::new(2 << 20, 0x100);
        reg[0x100 + TEXTURE_MODE] = 10 << 8;
        reg[0x100 + T_LOD] = 1 << 24 | 8 << 8;
        reg[0x100 + TEX_BASE_ADDR] = 0x100;
        reg[0x100 + TEX_BASE_ADDR_1] = 0x2000;
        reg[0x100 + TEX_BASE_ADDR_2] = 0x3000;
        reg[0x100 + TEX_BASE_ADDR_3_8] = 0x4000;
        tmu.recompute(&reg);
        assert_eq!(tmu.lodoffset[1], 0x800 + 256 * 256 * 2);
        tmu.v2 = true;
        tmu.recompute(&reg);
        assert_eq!(tmu.lodoffset[..4], [0x800, 0x10000, 0x18000, 0x20000]);
        // Level 4 follows level 3, of 32x32 texels.
        assert_eq!(tmu.lodoffset[4], 0x20000 + 32 * 32 * 2);
        // Not with bits 31:28 set.
        reg[0x100 + T_LOD] |= 0xF000_0000;
        tmu.recompute(&reg);
        assert_eq!(tmu.lodoffset[1], 0x800 + 256 * 256 * 2);
    }
}
