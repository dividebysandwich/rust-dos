//! Band decoding: tiles, macroblock headers, block coefficients,
//! dequantisation, DC prediction, inverse transforms and motion
//! compensation (spec 03 to 07).

use super::Iv50Error;
use super::bits::Bits;
use super::headers::{BandHeader, BandInfo};
use super::huff::{Huff, signed};
use super::tables::{RV_SLOTS, SCAN_0, SCAN_1, SCAN_2, SCAN_3};
use super::xform;

/// Where a band lives in its plane area and how it is coded.
#[derive(Debug, Clone)]
pub(crate) struct BandGeom {
    /// Band height in samples.
    pub height: usize,
    /// Row pitch in samples: the DLL's `(2 * width + 31) & ~31` bytes
    /// (spec 02, section 4).
    pub pitch: usize,
    /// Offset of sample (0, 0) in the plane area.
    pub offset: usize,
    pub info: BandInfo,
    /// Quantisation matrix id (spec 05, 2.4).
    pub matrix: usize,
    /// Tiles of the band, in raster order.
    pub tiles: Vec<TileGeom>,
}

/// One tile of a band, in band samples.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TileGeom {
    pub x: usize,
    pub y: usize,
    /// Macroblock columns and rows.
    pub cols: usize,
    pub rows: usize,
    /// Width in samples of the area an empty tile copies.
    pub copy_width: usize,
    /// Index of the tile's first macroblock in the luma band 0 records.
    pub rec_base: usize,
}

/// What a luma band 0 macroblock leaves for the bands that inherit from
/// it (spec 03, sections 6 and 7). The records persist across frames.
#[derive(Debug, Clone, Copy)]
pub(crate) struct B0Rec {
    pub intra: bool,
    pub mvx: i32,
    pub mvy: i32,
    /// The stored quantiser delta.
    pub delta: i32,
}

impl Default for B0Rec {
    fn default() -> Self {
        B0Rec {
            intra: true,
            mvx: 0,
            mvy: 0,
            delta: 0,
        }
    }
}

/// A decoded macroblock header.
#[derive(Debug, Clone, Copy, Default)]
struct Mb {
    intra: bool,
    cbp: u32,
    q: u32,
    mvx: i32,
    mvy: i32,
}

/// Meaning of a block symbol after the run/value table and the
/// corrections (spec 04, 3.4 and 3.5).
#[derive(Clone, Copy, Default)]
struct RvSym {
    run: u8,
    /// 0 normal, 1 end of block, 2 escape, 3 invalid.
    kind: u8,
    code: u16,
}

/// The run/value map of a band.
pub(crate) struct RvMap([RvSym; 256]);

impl RvMap {
    /// Build the map for slot `slot` with the band's correction pairs.
    pub fn new(slot: usize, corrections: &[(u8, u8)]) -> Result<RvMap, Iv50Error> {
        let t = &RV_SLOTS[slot];
        // Rank to (run, value code).
        let mut by_rank: Vec<(u8, u16)> = vec![(0, 0), (0, 0)];
        for (i, &c) in t.runs.iter().enumerate() {
            let run = (i + 1) as u8;
            let c = c as u16;
            for k in 0..c {
                by_rank.push((run, 2 * c - 2 * k));
            }
            for k in 0..c {
                by_rank.push((run, 2 * k + 1));
            }
        }
        if by_rank.len() > 256 {
            return Err(Iv50Error::BadHeader("run/value table"));
        }
        let mut map = [RvSym {
            run: 0,
            kind: 3,
            code: 0,
        }; 256];
        for (s, m) in map.iter_mut().enumerate() {
            let rank = t.rank[s] as usize;
            if rank == 0 {
                m.kind = 1;
            } else if rank == 1 {
                m.kind = 2;
            } else if rank < by_rank.len() {
                let (run, code) = by_rank[rank];
                *m = RvSym { run, kind: 0, code };
            }
        }
        // Corrections exchange the meaning of two symbols. End of block
        // and escape stay with their symbols; a pair that involves one of
        // them leaves the other symbol without a valid meaning (spec 05,
        // section 7: invalid stream).
        for &(a, b) in corrections {
            let (a, b) = (a as usize, b as usize);
            let (ka, kb) = (map[a].kind, map[b].kind);
            if ka == 1 || ka == 2 || kb == 1 || kb == 2 {
                if ka == 0 || ka == 3 {
                    map[a].kind = 3;
                }
                if kb == 0 || kb == 3 {
                    map[b].kind = 3;
                }
                continue;
            }
            map.swap(a, b);
        }
        Ok(RvMap(map))
    }
}

#[cfg(test)]
impl RvMap {
    /// The symbol that stands for (run, value code), for building test
    /// streams.
    pub fn symbol_for(&self, run: u8, code: u16) -> Option<u32> {
        (0..256u32).find(|&s| {
            let e = self.0[s as usize];
            e.kind == 0 && e.run == run && e.code == code
        })
    }

    /// The end-of-block symbol.
    pub fn eob(&self) -> u32 {
        (0..256u32).find(|&s| self.0[s as usize].kind == 1).unwrap()
    }
}

/// Quantiser steps, `bank[matrix][class][q][position]` (spec 06, 5.1).
pub(crate) type Bank = [[[[u8; 64]; 24]; 2]; 6];

/// Build the step bank from the base matrices and scale factors.
pub(crate) fn build_bank() -> Box<Bank> {
    use super::tables::{QUANT_BASE, QUANT_SCALE};
    let mut bank = Box::new([[[[0u8; 64]; 24]; 2]; 6]);
    for m in 0..6 {
        for cls in 0..2 {
            for q in 0..24 {
                let scale = QUANT_SCALE[m][cls][q] as u32;
                for p in 0..64 {
                    let base = if m == 5 {
                        let (r, k) = (p / 8, p % 8);
                        if r < 4 && k < 4 {
                            QUANT_BASE[m][cls][r * 4 + k] as u32
                        } else {
                            0
                        }
                    } else {
                        QUANT_BASE[m][cls][p] as u32
                    };
                    bank[m][cls][q][p] = ((base * scale) >> 8).clamp(1, 255) as u8;
                }
            }
        }
    }
    bank
}

/// The coefficient value of value code `code` at step `s` (spec 05,
/// 2.3). A code past the end of the DLL's run for `s` continues into the
/// run for `s + 1`, as the DLL's table is laid out.
#[inline]
fn dequant(code: u32, s: u32) -> Result<i32, Iv50Error> {
    if code == 0 {
        return Err(Iv50Error::BadBlockData("value code 0"));
    }
    let mut idx = code - 1;
    let mut s = s;
    loop {
        let n = 2 * (8192 / s);
        if idx < n {
            break;
        }
        idx -= n;
        s += 1;
        if s > 256 {
            return Err(Iv50Error::BadBlockData("value code past the table"));
        }
    }
    let k = idx / 2 + 1;
    let adj = if s & 1 == 1 && s > 1 { 1 } else { 0 };
    let mag = (k * s + s / 2 - adj) as i32;
    Ok(if idx & 1 == 0 { mag } else { -mag })
}

/// The scan of a transform type (spec 05, 2.2).
fn scan_for(xform: u32) -> Result<&'static [u8], Iv50Error> {
    match xform {
        0 => Ok(&SCAN_0),
        1 => Ok(&SCAN_1),
        2 | 3 => Ok(&SCAN_2),
        4 => Ok(&SCAN_3),
        _ => Err(Iv50Error::Unsupported("transform types 5 to 7")),
    }
}

/// Everything one band decode needs besides the bitstream.
pub(crate) struct BandJob<'a> {
    pub geom: &'a BandGeom,
    pub hdr: &'a BandHeader,
    pub is_b0: bool,
    pub frame_type: u32,
    pub frame_flags: u32,
    /// Macroblock size of luma band 0, for inherited vectors.
    pub b0_mb_size: u32,
    /// Luma band 0 has 1-block macroblocks in rows of odd length, whose
    /// records the DLL steps through with a drift (00-gaps.md, item 16).
    pub b0_drifts: bool,
    pub mb_huff: &'a Huff,
    pub blk_huff: &'a Huff,
    pub bank: &'a Bank,
    /// The plane area being decoded into, and the reference's.
    pub cur: &'a mut [i16],
    pub refp: &'a [i16],
    pub b0: &'a mut [B0Rec],
}

/// Scale an inherited vector component to a band with smaller
/// macroblocks (spec 03, section 6): divide by 2^n, halves away from
/// zero.
#[inline]
fn scale_mv(v: i32, n: u32) -> i32 {
    if n == 0 {
        return v;
    }
    let mag = (v.abs() + (1 << (n - 1))) >> n;
    if v < 0 { -mag } else { mag }
}

impl BandJob<'_> {
    /// Decode the band's tiles from `r`, which stands at the first tile.
    pub fn decode(mut self, r: &mut Bits) -> Result<(), Iv50Error> {
        let info = self.geom.info;
        let four = info.mb_size != info.blk_size;
        let ks = info.mb_size as i32 / 8 - self.b0_mb_size as i32 / 8;
        if self.hdr.inherit() && ks > 0 {
            return Err(Iv50Error::Unsupported(
                "inheriting into larger macroblocks than luma band 0's",
            ));
        }
        let ks = (-ks) as u32;
        let reads_b0 = self.hdr.inherit() || (self.hdr.has_delta() && !self.hdr.reads_delta());
        if reads_b0 && !self.is_b0 && self.b0_drifts {
            return Err(Iv50Error::Unsupported(
                "inheriting from 1-block luma band 0 macroblocks in odd rows",
            ));
        }
        let scan = scan_for(info.xform)?;
        let xf_blk = if info.xform >= 4 { 4 } else { 8 };
        if xf_blk != info.blk_size {
            return Err(Iv50Error::Unsupported(
                "transform size differs from block size",
            ));
        }
        if self.geom.matrix > 5 {
            return Err(Iv50Error::Unsupported("quantisation matrix ids 6 to 8"));
        }
        let rv = RvMap::new(self.hdr.rv_table, &self.hdr.corrections)?;
        let ntiles = self.geom.tiles.len();
        let mut mbs: Vec<Mb> = Vec::new();
        for t in 0..ntiles {
            let tile = self.geom.tiles[t];
            let tile_start = r.byte_pos();
            if r.bit() {
                // Empty tile (spec 03, 1.1; spec 07, 7.7).
                if self.frame_type != 0 {
                    self.copy_tile(&tile);
                    if self.is_b0 && !self.hdr.has_delta() {
                        let gq = self.hdr.gq as i32;
                        for rec in
                            &mut self.b0[tile.rec_base..tile.rec_base + tile.cols * tile.rows]
                        {
                            *rec = B0Rec {
                                intra: false,
                                mvx: 0,
                                mvy: 0,
                                delta: gq,
                            };
                        }
                    }
                }
                continue;
            }
            let size = if r.bit() {
                let s = r.read(8);
                if s == 0xff { r.read(24) } else { s }
            } else {
                0
            };
            r.align();
            self.parse_mbs(r, &tile, four, ks, &mut mbs)?;
            r.align();
            self.decode_blocks(r, &tile, four, scan, &rv, &mbs)?;
            r.align();
            if r.overrun() {
                return Err(Iv50Error::Truncated);
            }
            if size != 0 {
                if r.byte_pos() - tile_start != size as usize {
                    return Err(Iv50Error::BadBlockData("tile size mismatch"));
                }
            } else if t + 1 < ntiles {
                return Err(Iv50Error::Unsupported("coded tile without a size"));
            }
        }
        Ok(())
    }

    /// Copy an empty tile's area from the reference (spec 07, 7.7).
    fn copy_tile(&mut self, tile: &TileGeom) {
        let g = self.geom;
        let mb = g.info.mb_size as usize;
        let rows = (tile.rows * mb).min(g.height.next_multiple_of(16) - tile.y);
        for row in 0..rows {
            let o = g.offset + (tile.y + row) * g.pitch + tile.x;
            self.cur[o..o + tile.copy_width].copy_from_slice(&self.refp[o..o + tile.copy_width]);
        }
    }

    /// Parse the macroblock headers of a tile (spec 03, section 4).
    fn parse_mbs(
        &mut self,
        r: &mut Bits,
        tile: &TileGeom,
        four: bool,
        ks: u32,
        mbs: &mut Vec<Mb>,
    ) -> Result<(), Iv50Error> {
        let hdr = self.hdr;
        let inherit = hdr.inherit();
        let reads_delta = hdr.reads_delta();
        let has_delta = hdr.has_delta();
        let is_b0 = self.is_b0;
        let intra_frame = self.frame_type == 0;
        let delta_always = is_b0 && self.frame_flags & 8 != 0;
        let gq = hdr.gq as i32;
        let huff = self.mb_huff;
        let mut p: u32 = 0x8080;
        mbs.clear();
        let n = tile.cols * tile.rows;
        for k in 0..n {
            let src = if inherit || (has_delta && !reads_delta) {
                self.b0.get(tile.rec_base + k).copied().unwrap_or_default()
            } else {
                B0Rec::default()
            };
            let mut mb = Mb::default();
            let mut delta = 0;
            if r.bit() {
                mb.intra = intra_frame;
                if reads_delta && delta_always {
                    delta = signed(huff.decode(r).ok_or(Iv50Error::BadMbData)?);
                }
                if inherit {
                    mb.mvx = scale_mv(src.mvx, ks);
                    mb.mvy = scale_mv(src.mvy, ks);
                }
            } else {
                mb.intra = if intra_frame {
                    true
                } else if inherit {
                    src.intra
                } else {
                    !r.bit()
                };
                mb.cbp = if four { r.read(4) } else { r.read(1) };
                if reads_delta && (mb.cbp != 0 || delta_always) {
                    delta = signed(huff.decode(r).ok_or(Iv50Error::BadMbData)?);
                }
                if !mb.intra {
                    if inherit {
                        mb.mvx = scale_mv(src.mvx, ks);
                        mb.mvy = scale_mv(src.mvy, ks);
                    } else {
                        let dy = signed(huff.decode(r).ok_or(Iv50Error::BadMbData)?);
                        let dx = signed(huff.decode(r).ok_or(Iv50Error::BadMbData)?);
                        // The vector is one word with biased byte
                        // components; carries cross from x into y
                        // (spec 03, section 5).
                        p = p
                            .wrapping_add((((dy + 128) & 0xff) as u32) << 8)
                            .wrapping_add(((dx + 128) & 0xff) as u32)
                            .wrapping_sub(0x8080);
                        mb.mvx = (p & 0xff) as i32 - 128;
                        mb.mvy = ((p >> 8) & 0xff) as i32 - 128;
                    }
                }
            }
            // The quantiser (spec 03, section 7).
            let stored;
            mb.q = if reads_delta {
                if !(-24..=23).contains(&delta) {
                    return Err(Iv50Error::BadMbData);
                }
                let q = (gq + delta).clamp(0, 23);
                stored = q - gq;
                q as u32
            } else if has_delta {
                stored = 0;
                (gq + src.delta).clamp(0, 23) as u32
            } else {
                stored = 0;
                gq as u32
            };
            if is_b0 {
                let rec = &mut self.b0[tile.rec_base + k];
                rec.intra = mb.intra;
                if !mb.intra {
                    rec.mvx = mb.mvx;
                    rec.mvy = mb.mvy;
                }
                rec.delta = stored;
            }
            mbs.push(mb);
        }
        if r.overrun() {
            return Err(Iv50Error::Truncated);
        }
        Ok(())
    }

    /// Decode the block data of a tile and reconstruct its blocks (spec
    /// 05, spec 06 and spec 07).
    fn decode_blocks(
        &mut self,
        r: &mut Bits,
        tile: &TileGeom,
        four: bool,
        scan: &[u8],
        rv: &RvMap,
        mbs: &[Mb],
    ) -> Result<(), Iv50Error> {
        let g = self.geom;
        let info = g.info;
        let mb_size = info.mb_size as usize;
        let blk = info.blk_size as usize;
        let pitch = g.pitch;
        let xform = info.xform;
        let dc_pred = xform == 0 || xform == 4;
        let nblk = if four { 4 } else { 1 };
        let max_pos = scan.len() as i32 - 1;
        let half_pel = info.half_pel;
        let bank = &self.bank[g.matrix];
        let mut pred: i32 = 0;
        let mut coefs;
        for row in 0..tile.rows {
            for col in 0..tile.cols {
                let mb = &mbs[row * tile.cols + col];
                let steps = &bank[mb.intra as usize][mb.q.min(23) as usize];
                for j in 0..nblk {
                    let bx = tile.x + col * mb_size + (j & 1) * blk;
                    let by = tile.y + row * mb_size + (j >> 1) * blk;
                    let off = g.offset + by * pitch + bx;
                    if mb.cbp & (1 << j) != 0 {
                        if mb.q > 23 {
                            return Err(Iv50Error::Unsupported("global quantiser above 23"));
                        }
                        coefs = [0; 64];
                        let mut pos: i32 = -1;
                        loop {
                            let s = self
                                .blk_huff
                                .decode(r)
                                .ok_or(Iv50Error::BadBlockData("code without a symbol"))?;
                            let e = rv.0[s as usize];
                            let (run, code) = match e.kind {
                                0 => (e.run as i32, e.code as u32),
                                1 => break,
                                2 => {
                                    let run = self
                                        .blk_huff
                                        .decode(r)
                                        .ok_or(Iv50Error::BadBlockData("escape"))?
                                        as i32
                                        + 1;
                                    let lo = self
                                        .blk_huff
                                        .decode(r)
                                        .ok_or(Iv50Error::BadBlockData("escape"))?;
                                    let hi = self
                                        .blk_huff
                                        .decode(r)
                                        .ok_or(Iv50Error::BadBlockData("escape"))?;
                                    (run, hi * 64 + lo)
                                }
                                _ => return Err(Iv50Error::BadBlockData("corrected symbol")),
                            };
                            pos += run;
                            if pos > max_pos {
                                return Err(Iv50Error::BadBlockData("scan position past the end"));
                            }
                            let p = scan[pos as usize] as usize;
                            coefs[p] = dequant(code, steps[p] as u32)? as i16;
                            if r.overrun() {
                                return Err(Iv50Error::Truncated);
                            }
                        }
                        if dc_pred && mb.intra {
                            let d = coefs[0] as i32;
                            coefs[0] = (pred + d) as i16;
                            pred += d;
                        }
                        xform::inverse(xform, &coefs, self.cur, off, pitch);
                        if !mb.intra {
                            self.motion(off, blk, mb.mvx, mb.mvy, half_pel, true);
                        }
                    } else if mb.intra {
                        self.fill_intra(off, blk, dc_pred, pred);
                    } else {
                        self.motion(off, blk, mb.mvx, mb.mvy, half_pel, false);
                    }
                }
            }
            // A row of 1-block macroblocks with an odd count gets one more
            // record, an uncoded intra block (spec 05, 2.1), which the
            // intra fill writes.
            if !four && tile.cols % 2 == 1 {
                let bx = tile.x + tile.cols * mb_size;
                let by = tile.y + row * mb_size;
                if bx + blk <= pitch {
                    let off = g.offset + by * pitch + bx;
                    self.fill_intra(off, blk, dc_pred, pred);
                }
            }
        }
        Ok(())
    }

    /// Fill an uncoded intra block (spec 05, 4.1).
    #[inline]
    fn fill_intra(&mut self, off: usize, blk: usize, dc_pred: bool, pred: i32) {
        let v = if dc_pred { ((pred + 1) >> 1) as i16 } else { 0 };
        xform::fill(self.cur, off, self.geom.pitch, blk, v);
    }

    /// Motion compensation of one inter block (spec 07, 7.3 and 7.4):
    /// add the prediction to the residual of a coded block, or write the
    /// prediction into an uncoded one.
    #[inline]
    fn motion(&mut self, off: usize, n: usize, mvx: i32, mvy: i32, half_pel: bool, coded: bool) {
        let pitch = self.geom.pitch as isize;
        let (ix, iy, fx, fy) = if half_pel {
            (mvx >> 1, mvy >> 1, (mvx & 1) as usize, (mvy & 1) as usize)
        } else {
            (mvx, mvy, 0, 0)
        };
        let src = off as isize + iy as isize * pitch + ix as isize;
        let last = src + (n + fy - 1) as isize * pitch + (n + fx) as isize;
        let refp = self.refp;
        let cur = &mut *self.cur;
        let pitch = pitch as usize;
        if src >= 0 && last <= refp.len() as isize {
            let src = src as usize;
            macro_rules! run {
                ($f:expr) => {
                    for rr in 0..n {
                        let s = src + rr * pitch;
                        let d = off + rr * pitch;
                        for c in 0..n {
                            let p: i16 = $f(s + c);
                            let o = &mut cur[d + c];
                            *o = if coded { o.wrapping_add(p) } else { p };
                        }
                    }
                };
            }
            match (fx, fy) {
                (0, 0) => run!(|i: usize| refp[i]),
                (1, 0) => run!(|i: usize| refp[i].wrapping_add(refp[i + 1]) >> 1),
                (0, 1) => run!(|i: usize| refp[i].wrapping_add(refp[i + pitch]) >> 1),
                _ => run!(|i: usize| refp[i]
                    .wrapping_add(refp[i + 1])
                    .wrapping_add(refp[i + pitch])
                    .wrapping_add(refp[i + pitch + 1])
                    >> 2),
            }
        } else {
            // Reads outside the plane area: the DLL reads whatever memory
            // lies there (spec 07, 7.5); this decoder reads zeros.
            let get = |i: isize| -> i16 {
                if i >= 0 && (i as usize) < refp.len() {
                    refp[i as usize]
                } else {
                    0
                }
            };
            let pi = pitch as isize;
            for rr in 0..n {
                for c in 0..n {
                    let i = src + rr as isize * pi + c as isize;
                    let p = match (fx, fy) {
                        (0, 0) => get(i),
                        (1, 0) => get(i).wrapping_add(get(i + 1)) >> 1,
                        (0, 1) => get(i).wrapping_add(get(i + pi)) >> 1,
                        _ => {
                            get(i)
                                .wrapping_add(get(i + 1))
                                .wrapping_add(get(i + pi))
                                .wrapping_add(get(i + pi + 1))
                                >> 2
                        }
                    };
                    let o = &mut cur[off + rr * pitch + c];
                    *o = if coded { o.wrapping_add(p) } else { p };
                }
            }
        }
    }
}

/// Fill an empty band (spec 08, 8.8, MMX path): bands 0 to 3 of a plane
/// get 0, 1, 3 and 2; later bands get the dword 0x80000000, that is the
/// samples 0 and -32768 in turn. Nothing is written when the band's
/// first dword already has its sign bit set.
pub(crate) fn fill_empty_band(cur: &mut [i16], g: &BandGeom, index: usize) {
    let o = g.offset;
    if cur[o + 1] < 0 {
        return;
    }
    let area = &mut cur[o..o + g.height * g.pitch];
    match index {
        0..=3 => area.fill([0, 1, 3, 2][index]),
        _ => {
            for (i, v) in area.iter_mut().enumerate() {
                *v = if i & 1 == 0 { 0 } else { i16::MIN };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dequant_examples() {
        // s = 1: 1, 2, 3; s = 2: 3, 5, 7; s = 3: 3, 6, 9; s = 4: 6, 10.
        let pos = |s: u32, k: u32| dequant(2 * k - 1, s).unwrap();
        assert_eq!([pos(1, 1), pos(1, 2), pos(1, 3)], [1, 2, 3]);
        assert_eq!([pos(2, 1), pos(2, 2), pos(2, 3)], [3, 5, 7]);
        assert_eq!([pos(3, 1), pos(3, 2), pos(3, 3)], [3, 6, 9]);
        assert_eq!([pos(4, 1), pos(4, 2)], [6, 10]);
        assert_eq!(dequant(2, 4).unwrap(), -6);
        // Past the run for s = 200 (2 * 40 entries): continue in s = 201.
        assert_eq!(dequant(81, 200).unwrap(), 201 + 100 - 1);
    }

    /// The bank equals the DLL's runtime bank, dumped in
    /// `tests/data/iv50-quant-matrices-1007b000.csv` (id, class, q, then
    /// 64 steps; the 4x4 id holds its rows at stride 8).
    #[test]
    fn bank_matches_runtime_dump() {
        let csv = include_str!("../../../tests/data/iv50-quant-matrices-1007b000.csv");
        let bank = build_bank();
        let mut rows = 0;
        for line in csv.lines().skip(1) {
            let f: Vec<&str> = line.split(',').collect();
            let m: usize = f[0].parse().unwrap();
            let cls: usize = f[1].parse().unwrap();
            let q: usize = f[2].parse().unwrap();
            let v: Vec<u8> = f[6..70].iter().map(|x| x.parse().unwrap()).collect();
            for p in 0..64 {
                if m == 5 && (p / 8 >= 4 || p % 8 >= 4) {
                    continue;
                }
                assert_eq!(bank[m][cls][q][p], v[p], "id {m} class {cls} q {q} pos {p}");
            }
            rows += 1;
        }
        assert_eq!(rows, 6 * 2 * 24);
    }

    #[test]
    fn rv_map_ranks() {
        let m = RvMap::new(0, &[]).unwrap();
        // Slot 0: rank[5] = 0 (EOB), rank[2] = 1 (ESC).
        assert_eq!(m.0[5].kind, 1);
        assert_eq!(m.0[2].kind, 2);
        // Run 1 has 40 pairs: ranks 2..41 are codes 80, 78, ..., 2 and
        // ranks 42..81 codes 1, 3, ..., 79. rank[0] = 42.
        assert_eq!((m.0[0].run, m.0[0].code), (1, 1));
        assert_eq!((m.0[1].run, m.0[1].code), (1, 2));
        assert_eq!((m.0[4].run, m.0[4].code), (1, 4));
        let c = RvMap::new(0, &[(0, 1)]).unwrap();
        assert_eq!(c.0[0].code, 2);
        assert_eq!(c.0[1].code, 1);
    }

    #[test]
    fn inherited_vectors_round_halves_away_from_zero() {
        assert_eq!(scale_mv(3, 1), 2);
        assert_eq!(scale_mv(-3, 1), -2);
        assert_eq!(scale_mv(2, 1), 1);
        assert_eq!(scale_mv(-5, 2), -1);
        assert_eq!(scale_mv(-6, 2), -2);
        assert_eq!(scale_mv(7, 0), 7);
    }
}
