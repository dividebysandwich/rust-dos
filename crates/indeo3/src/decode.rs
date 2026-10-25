//! The frame header, the binary trees of cells and the cells' VQ data.
//!
//! Ported from FFmpeg's libavcodec/indeo3.c:
//!
//! Indeo Video v3 compatible decoder
//! Copyright (c) 2009 - 2011 Maxim Poliakovski
//!
//! This file is part of FFmpeg.
//!
//! FFmpeg is free software; you can redistribute it and/or
//! modify it under the terms of the GNU Lesser General Public
//! License as published by the Free Software Foundation; either
//! version 2.1 of the License, or (at your option) any later version.
//!
//! FFmpeg is distributed in the hope that it will be useful,
//! but WITHOUT ANY WARRANTY; without even the implied warranty of
//! MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
//! Lesser General Public License for more details.
//!
//! You should have received a copy of the GNU Lesser General Public
//! License along with FFmpeg; if not, write to the Free Software
//! Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA
//! 02110-1301 USA
//!
//! Pixels are 7 bits, kept in two buffers per plane, the frame being
//! decoded and the one it predicts from. A buffer has one more row on top
//! than the plane, filled with 64, which the cells on the top row predict
//! from. Every read and write of a buffer is checked, so a block that the
//! checks taken from FFmpeg miss gives an error instead of a panic.

use crate::Error;
use crate::tables::{REQUANT_TAB, VQ_TAB, VqEntry};

// RLE opcodes.
/// Same as FA, and do the same with the next block.
const RLE_ESC_F9: u8 = 249;
/// INTRA: skip the block; INTER: copy it from the reference.
const RLE_ESC_FA: u8 = 250;
/// Apply a null delta to N blocks, or skip N blocks.
const RLE_ESC_FB: u8 = 251;
/// Same as FD, and do the same with the next block.
const RLE_ESC_FC: u8 = 252;
/// Apply a null delta to the remaining lines of the block.
const RLE_ESC_FD: u8 = 253;
/// Apply a null delta to the lines up to the 3rd.
const RLE_ESC_FE: u8 = 254;
/// Apply a null delta to the lines up to the 2nd.
const RLE_ESC_FF: u8 = 255;

// Frame flags.
const BS_8BIT_PEL: u16 = 1 << 1;
const BS_MV_Y_HALF: u16 = 1 << 4;
const BS_MV_X_HALF: u16 = 1 << 5;
/// The bit that says which of the two buffers the frame goes to.
const BS_BUFFER: u16 = 9;

// Binary tree codes.
const H_SPLIT: u32 = 0;
const V_SPLIT: u32 = 1;
const INTRA_NULL: u32 = 2;

const CELL_STACK_MAX: i32 = 20;

const OS_HDR_ID: u32 = u32::from_be_bytes(*b"FRMH");

const OUT_OF_DATA: Error = Error::Invalid("cell data runs past the end of the plane");
const BAD_DATA: Error = Error::Invalid("invalid VQ data");
const OUTSIDE: Error = Error::Invalid("a block reaches outside the plane");

/// One plane's two buffers.
pub(crate) struct Plane {
    bufs: [Vec<u8>; 2],
    pub width: usize,
    pub height: usize,
    pub pitch: usize,
}

impl Plane {
    pub fn new(width: usize, height: usize) -> Plane {
        let pitch = width.next_multiple_of(16);
        let mut buf = vec![0; pitch * (height + 1)];
        buf[..pitch].fill(0x40);
        Plane {
            bufs: [buf.clone(), buf],
            width,
            height,
            pitch,
        }
    }

    /// Buffer `sel`'s pixels, rows `pitch` apart, the prediction row left
    /// out.
    pub fn pixels(&self, sel: usize) -> &[u8] {
        &self.bufs[sel][self.pitch..]
    }
}

/// Where a frame's planes are, and what selects their VQ tables.
pub(crate) struct FrameInfo {
    pub buf_sel: usize,
    cb_offset: u8,
    alt_quant: [u8; 16],
    /// Start and size of the Y, U and V data in the frame.
    pub planes: [(usize, usize); 3],
}

/// Reads little-endian values as FFmpeg's bytestream2 does: what is past
/// the end reads as 0, and the position stops at the end.
struct ByteReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl ByteReader<'_> {
    fn bytes<const N: usize>(&mut self) -> [u8; N] {
        match self.buf[self.pos..].first_chunk::<N>() {
            Some(b) => {
                self.pos += N;
                *b
            }
            None => {
                self.pos = self.buf.len();
                [0; N]
            }
        }
    }

    fn u8(&mut self) -> u8 {
        self.bytes::<1>()[0]
    }

    fn le16(&mut self) -> u16 {
        u16::from_le_bytes(self.bytes())
    }

    fn le32(&mut self) -> u32 {
        u32::from_le_bytes(self.bytes())
    }

    fn skip(&mut self, n: usize) {
        self.pos = (self.pos + n).min(self.buf.len());
    }
}

/// Reads a frame's headers. `None` is a sync frame, which has no picture.
pub(crate) fn parse_header(
    buf: &[u8],
    width: usize,
    height: usize,
) -> Result<Option<FrameInfo>, Error> {
    let mut gb = ByteReader { buf, pos: 0 };

    // The OS header.
    let frame_num = gb.le32();
    let word2 = gb.le32();
    let check_sum = gb.le32();
    let data_size = gb.le32();
    if frame_num ^ word2 ^ data_size ^ OS_HDR_ID != check_sum {
        return Err(Error::Invalid("OS header checksum mismatch"));
    }

    // The bitstream header. Offsets in it count from here.
    let bs_hdr = gb.pos;
    if gb.le16() != 32 {
        return Err(Error::Unsupported("codec version other than 32"));
    }
    let flags = gb.le16();
    let mut data_size = (gb.le32().wrapping_add(7) >> 3) as i64;
    let cb_offset = gb.u8();
    if data_size == 16 {
        return Ok(None);
    }
    data_size = data_size.min(buf.len() as i64 - 16);
    gb.skip(3); // a reserved byte and a checksum

    let frame_height = gb.le16() as usize;
    let frame_width = gb.le16() as usize;
    if frame_width != width || frame_height != height {
        return Err(Error::Invalid("frame size differs from the decoder's"));
    }

    let offsets = [gb.le32() as i32, gb.le32() as i32, gb.le32() as i32];
    gb.skip(4);

    // The planes come in no fixed order, so each one ends where the next
    // one after it starts, or at the end of the data.
    let starts = offsets.map(|o| o as u32);
    let mut sizes = [0i64; 3];
    for j in 0..3 {
        let mut end = data_size as u32;
        for &s in starts.iter().rev() {
            if s < end && s > starts[j] {
                end = s;
            }
        }
        sizes[j] = end.wrapping_sub(starts[j]) as i32 as i64;
    }
    let min_off = *offsets.iter().min().unwrap() as i64;
    let max_off = *offsets.iter().max().unwrap() as i64;
    if min_off < 0
        || max_off >= data_size - 16
        || min_off < (gb.pos - bs_hdr) as i64 + 16
        || *sizes.iter().min().unwrap() <= 0
    {
        return Err(Error::Invalid("one of the Y, U and V offsets is invalid"));
    }

    let mut alt_quant = [0; 16];
    for (i, q) in alt_quant.iter_mut().enumerate() {
        *q = buf.get(gb.pos + i).copied().unwrap_or(0);
    }

    if flags & BS_8BIT_PEL != 0 {
        return Err(Error::Unsupported("8-bit pixels"));
    }
    if flags & (BS_MV_X_HALF | BS_MV_Y_HALF) != 0 {
        return Err(Error::Unsupported("half-pixel motion vectors"));
    }

    // In the header the order is Y, V, U.
    let plane = |i: usize| (bs_hdr + offsets[i] as usize, sizes[i] as usize);
    Ok(Some(FrameInfo {
        buf_sel: ((flags >> BS_BUFFER) & 1) as usize,
        cb_offset,
        alt_quant,
        planes: [plane(0), plane(2), plane(1)],
    }))
}

/// Decodes one plane of a frame into buffer `sel`, predicting from the
/// other one.
pub(crate) fn decode_plane(
    pkt: &[u8],
    plane: &mut Plane,
    sel: usize,
    info: &FrameInfo,
    index: usize,
    strip_width: i32,
) -> Result<(), Error> {
    let (start, size) = info.planes[index];
    if pkt.get(start..start + size).is_none() || size < 4 {
        return Err(Error::Invalid("plane data runs past the end of the frame"));
    }

    // Each plane starts with the number of motion vectors, the vectors,
    // then the binary tree and the VQ data.
    let num_vectors =
        u32::from_le_bytes([pkt[start], pkt[start + 1], pkt[start + 2], pkt[start + 3]]) as usize;
    let data = start + 4;
    let data_size = size - 4;
    if num_vectors > 256 {
        return Err(Error::Invalid("too many motion vectors"));
    }
    if num_vectors * 2 > data_size {
        return Err(Error::Invalid("motion vectors run past the plane"));
    }
    let gb_start = data + num_vectors * 2;

    let [b0, b1] = &mut plane.bufs;
    let (cur, prev) = if sel == 0 { (b0, b1) } else { (b1, b0) };
    let mut ctx = PlaneDecoder {
        pkt,
        gb: BitReader {
            buf: &pkt[gb_start..],
            size: ((data_size - num_vectors * 2) * 8) as i64,
            index: 0,
        },
        gb_start,
        need_resync: false,
        skip_bits: 0,
        next_cell_data: gb_start,
        last_byte: data + data_size,
        mv_start: data,
        num_vectors,
        cb_offset: info.cb_offset,
        alt_quant: info.alt_quant,
        cur,
        prev,
        width: plane.width as i32,
        height: plane.height as i32,
        pitch: plane.pitch as isize,
    };

    // The first cell is the whole plane.
    let mut cell = Cell {
        xpos: 0,
        ypos: 0,
        width: plane.width as i32 >> 2,
        height: plane.height as i32 >> 2,
        tree: 0,
        mv: None,
    };
    ctx.parse_bintree(INTRA_NULL, &mut cell, CELL_STACK_MAX, strip_width)
}

/// A rectangle of 4x4 blocks, and how it is predicted.
#[derive(Clone, Copy)]
struct Cell {
    xpos: i32,
    ypos: i32,
    width: i32,
    height: i32,
    /// 0 in the motion compensation tree, 1 in the VQ tree.
    tree: u8,
    /// The motion vector (y, x) of an INTER cell; `None` is INTRA.
    mv: Option<(i32, i32)>,
}

/// FFmpeg's checked bit reader: reading past the end gives the bytes
/// after the plane, or 0 past the frame, and the position stops 8 bits
/// past the end.
struct BitReader<'a> {
    buf: &'a [u8],
    size: i64,
    index: i64,
}

impl BitReader<'_> {
    fn get_bits(&mut self, n: u32) -> u32 {
        let byte = (self.index >> 3) as usize;
        let mut w = 0u32;
        for i in 0..4 {
            w = w << 8 | self.buf.get(byte + i).copied().unwrap_or(0) as u32;
        }
        let v = (w << (self.index & 7)) >> (32 - n);
        self.index = (self.index + n as i64).min(self.size + 8);
        v
    }

    fn skip_bits_long(&mut self, n: i64) {
        self.index += n.clamp(-self.index, self.size + 8 - self.index);
    }

    fn left(&self) -> i64 {
        self.size - self.index
    }
}

fn split_cell(size: i32) -> i32 {
    if size > 2 { ((size + 2) >> 2) << 1 } else { 1 }
}

fn replicate64(a: u64) -> u64 {
    let a = a & 0x00FF_00FF_00FF_00FF;
    a | a << 8
}

fn replicate32(a: u32) -> u32 {
    let a = a & 0x00FF_00FF;
    a | a << 8
}

fn rd<const N: usize>(s: &[u8], o: isize) -> Result<[u8; N], Error> {
    // A negative offset turns into one past any buffer.
    s.get(o as usize..)
        .and_then(|s| s.first_chunk::<N>())
        .copied()
        .ok_or(OUTSIDE)
}

fn wr<const N: usize>(s: &mut [u8], o: isize, v: [u8; N]) -> Result<(), Error> {
    let d = s
        .get_mut(o as usize..)
        .and_then(|s| s.first_chunk_mut::<N>())
        .ok_or(OUTSIDE)?;
    *d = v;
    Ok(())
}

struct PlaneDecoder<'a> {
    pkt: &'a [u8],
    gb: BitReader<'a>,
    gb_start: usize,
    need_resync: bool,
    skip_bits: i64,
    next_cell_data: usize,
    last_byte: usize,
    mv_start: usize,
    num_vectors: usize,
    cb_offset: u8,
    alt_quant: [u8; 16],
    cur: &'a mut [u8],
    prev: &'a mut [u8],
    width: i32,
    height: i32,
    pitch: isize,
}

impl PlaneDecoder<'_> {
    /// The buffer a cell predicts from: the one being decoded for INTRA
    /// cells, the other one for INTER cells.
    fn src(&self, in_cur: bool) -> &[u8] {
        if in_cur { self.cur } else { self.prev }
    }

    fn r32(&self, in_cur: bool, o: isize) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(rd(self.src(in_cur), o)?))
    }

    fn r16(&self, in_cur: bool, o: isize) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(rd(self.src(in_cur), o)?))
    }

    fn r64(&self, in_cur: bool, o: isize) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(rd(self.src(in_cur), o)?))
    }

    fn w16(&mut self, o: isize, v: u16) -> Result<(), Error> {
        wr(self.cur, o, v.to_le_bytes())
    }

    fn w32(&mut self, o: isize, v: u32) -> Result<(), Error> {
        wr(self.cur, o, v.to_le_bytes())
    }

    fn w64(&mut self, o: isize, v: u64) -> Result<(), Error> {
        wr(self.cur, o, v.to_le_bytes())
    }

    fn byte(&self, o: usize) -> Result<u8, Error> {
        self.pkt.get(o).copied().ok_or(OUT_OF_DATA)
    }

    /// Where the cell's data starts: the byte after the tree code just
    /// read.
    fn bit_pos_byte(&self) -> usize {
        self.gb_start + ((self.gb.index + 7) >> 3) as usize
    }

    fn update_bitpos(&mut self, n: i64) {
        self.skip_bits += n;
        self.need_resync = true;
    }

    /// Once the bit reader reaches a byte boundary, skips the bytes cell
    /// data took.
    fn resync(&mut self) {
        if self.need_resync && self.gb.index & 7 == 0 {
            self.gb.skip_bits_long(self.skip_bits);
            self.skip_bits = 0;
            self.need_resync = false;
        }
    }

    fn check_cell(&self, c: &Cell) -> Result<(), Error> {
        if c.xpos + c.width > self.width >> 2 || c.ypos + c.height > self.height >> 2 {
            return Err(Error::Invalid("cell outside the plane"));
        }
        Ok(())
    }

    /// Where a cell's block (0, 0) is in the buffers.
    fn cell_offset(&self, c: &Cell) -> isize {
        self.pitch + (c.ypos as isize * 4) * self.pitch + c.xpos as isize * 4
    }

    /// Checks that the cell moved by `mv` lies in the plane, the row on
    /// top included.
    fn check_mv(&self, c: &Cell, (mv_y, mv_x): (i32, i32)) -> Result<(), Error> {
        if (c.ypos << 2) + mv_y < -1
            || (c.xpos << 2) + mv_x < 0
            || ((c.ypos + c.height) << 2) + mv_y > self.height
            || ((c.xpos + c.width) << 2) + mv_x > self.width
        {
            return Err(Error::Invalid("motion vector points out of the frame"));
        }
        Ok(())
    }

    /// Copies the cell moved by its motion vector from the previous frame
    /// into the cell in the current one.
    fn copy_cell(&mut self, c: &Cell) -> Result<(), Error> {
        let mv = c.mv.unwrap_or((0, 0));
        self.check_mv(c, mv)?;
        let dst = self.cell_offset(c);
        let src = dst + mv.0 as isize * self.pitch + mv.1 as isize;
        let w = c.width as usize * 4;
        for row in 0..c.height as isize * 4 {
            let s = (src + row * self.pitch) as usize;
            let d = (dst + row * self.pitch) as usize;
            let from = self.prev.get(s..).and_then(|b| b.get(..w));
            let to = self.cur.get_mut(d..).and_then(|b| b.get_mut(..w));
            to.ok_or(OUTSIDE)?.copy_from_slice(from.ok_or(OUTSIDE)?);
        }
        Ok(())
    }

    fn parse_bintree(
        &mut self,
        code: u32,
        ref_cell: &mut Cell,
        depth: i32,
        strip_width: i32,
    ) -> Result<(), Error> {
        if depth <= 0 {
            return Err(Error::Invalid("binary tree too deep"));
        }

        let mut curr = *ref_cell;
        if code == H_SPLIT {
            curr.height = split_cell(ref_cell.height);
            ref_cell.ypos += curr.height;
            ref_cell.height -= curr.height;
            if ref_cell.height <= 0 || curr.height <= 0 {
                return Err(Error::Invalid("bad horizontal split"));
            }
        } else if code == V_SPLIT {
            if curr.width > strip_width {
                // Split a strip.
                let n = if curr.width <= strip_width << 1 { 1 } else { 2 };
                curr.width = n * strip_width;
            } else {
                curr.width = split_cell(ref_cell.width);
            }
            ref_cell.xpos += curr.width;
            ref_cell.width -= curr.width;
            if ref_cell.width <= 0 || curr.width <= 0 {
                return Err(Error::Invalid("bad vertical split"));
            }
        }

        while self.gb.left() >= 2 {
            self.resync();
            let code = self.gb.get_bits(2);
            match code {
                H_SPLIT | V_SPLIT => {
                    self.parse_bintree(code, &mut curr, depth - 1, strip_width)?;
                }
                INTRA_NULL => {
                    if curr.tree == 0 {
                        // An INTRA strip: enter the VQ tree.
                        curr.mv = None;
                        curr.tree = 1;
                    } else {
                        // VQ NULL: copy the cell. Code 1 is SkipCell,
                        // which FFmpeg doesn't implement and copies too.
                        self.resync();
                        if self.gb.get_bits(2) >= 2 {
                            return Err(Error::Invalid("invalid VQ_NULL code"));
                        }
                        self.check_cell(&curr)?;
                        if curr.mv.is_none() {
                            return Err(Error::Invalid("VQ_NULL in an INTRA cell"));
                        }
                        return self.copy_cell(&curr);
                    }
                }
                _ => {
                    if curr.tree == 0 {
                        // An INTER strip: read its motion vector's index.
                        if !self.need_resync {
                            self.next_cell_data = self.bit_pos_byte();
                        }
                        if self.next_cell_data >= self.last_byte {
                            return Err(Error::Invalid("motion vector out of array"));
                        }
                        let mv_idx = self.byte(self.next_cell_data)? as usize;
                        self.next_cell_data += 1;
                        if mv_idx >= self.num_vectors {
                            return Err(Error::Invalid("motion vector index out of range"));
                        }
                        let mv = self.mv_start + mv_idx * 2;
                        curr.mv =
                            Some((self.byte(mv)? as i8 as i32, self.byte(mv + 1)? as i8 as i32));
                        curr.tree = 1;
                        self.update_bitpos(8);
                    } else {
                        // VQ DATA.
                        if !self.need_resync {
                            self.next_cell_data = self.bit_pos_byte();
                        }
                        self.check_cell(&curr)?;
                        let bytes_used = self.decode_cell(&curr, self.next_cell_data)?;
                        self.update_bitpos(bytes_used as i64 * 8);
                        self.next_cell_data += bytes_used;
                        return Ok(());
                    }
                }
            }
        }
        Err(Error::Invalid("binary tree runs past the end of the plane"))
    }

    /// Decodes a VQ cell, returning how many bytes of data it took.
    fn decode_cell(&mut self, c: &Cell, data_start: usize) -> Result<usize, Error> {
        if data_start >= self.last_byte {
            return Err(OUT_OF_DATA);
        }
        let mut data_ptr = data_start;

        // The coding mode and VQ table index.
        let code = self.byte(data_ptr)?;
        data_ptr += 1;
        let mode = code >> 4;
        let mut vq_index = (code & 0xF) as usize;

        let block = self.cell_offset(c);
        // The reference: the row above for INTRA cells; the cell moved
        // by the motion vector for INTER cells of modes 0 to 4. Modes 10
        // and 11 INTER copy the moved cell first and add to it.
        let mut ref_block = None;
        let mut ref_cur = true;
        match c.mv {
            None => ref_block = Some(block - self.pitch),
            Some(_) if mode >= 10 => self.copy_cell(c)?,
            Some(mv) => {
                self.check_mv(c, mv)?;
                ref_block = Some(block + mv.0 as isize * self.pitch + mv.1 as isize);
                ref_cur = false;
            }
        }

        // Modes 0 and 3 use the primary table for all lines of a block;
        // 1 and 4 use the primary and secondary on alternate lines.
        let (prim_indx, second_indx);
        if mode == 1 || mode == 4 {
            let q = self.alt_quant[vq_index];
            prim_indx = (q >> 4) as usize + self.cb_offset as usize;
            second_indx = (q & 0xF) as usize + self.cb_offset as usize;
        } else {
            vq_index += self.cb_offset as usize;
            prim_indx = vq_index;
            second_indx = vq_index;
        }
        if prim_indx >= 24 || second_indx >= 24 {
            return Err(Error::Invalid("invalid VQ table index"));
        }
        let delta = [&VQ_TAB[second_indx], &VQ_TAB[prim_indx]];
        let swap_quads = [second_indx >= 16, prim_indx >= 16];

        // Requantize the prediction when this cell's VQ index differs
        // from the predicted cell's, so the deltas can't overflow.
        if vq_index >= 8
            && let Some(r) = ref_block
        {
            let tab = &REQUANT_TAB[vq_index & 7];
            let buf = if ref_cur {
                &mut *self.cur
            } else {
                &mut *self.prev
            };
            let w = c.width as usize * 4;
            let row = buf.get_mut(r as usize..).and_then(|b| b.get_mut(..w));
            for p in row.ok_or(OUTSIDE)? {
                *p = tab[(*p & 127) as usize];
            }
        }

        let args = CellArgs {
            mode,
            delta,
            swap_quads,
            ref_cur,
        };
        match mode {
            0 | 1 | 3 | 4 => {
                if mode >= 3 && c.mv.is_some() {
                    return Err(Error::Invalid("mode 3 or 4 in an INTER cell"));
                }
                let v_zoom = (mode >= 3) as u32;
                let r = ref_block.unwrap_or(block);
                self.decode_cell_data(c, block, r, 0, v_zoom, &args, &mut data_ptr)?;
            }
            10 | 11 => {
                if mode == 10 && c.mv.is_none() {
                    let r = ref_block.unwrap_or(block);
                    self.decode_cell_data(c, block, r, 1, 1, &args, &mut data_ptr)?;
                } else {
                    if mode == 11 && c.mv.is_none() {
                        return Err(Error::Invalid("mode 11 in an INTRA cell"));
                    }
                    // These add to the copied cell and don't read the
                    // reference.
                    let h_zoom = (mode == 10) as u32;
                    self.decode_cell_data(c, block, block, h_zoom, 1, &args, &mut data_ptr)?;
                }
            }
            _ => return Err(Error::Unsupported("coding mode")),
        }
        Ok(data_ptr - data_start)
    }

    fn next_byte(&self, data_ptr: &mut usize) -> Result<u8, Error> {
        if *data_ptr >= self.last_byte {
            return Err(OUT_OF_DATA);
        }
        let b = self.byte(*data_ptr)?;
        *data_ptr += 1;
        Ok(b)
    }

    /// Copies `n` rows of 4 pixels from the reference.
    fn copy_block4(&mut self, dst: isize, rf: isize, in_cur: bool, n: u32) -> Result<(), Error> {
        for i in 0..n as isize {
            let v = self.r32(in_cur, rf + i * self.pitch)?;
            self.w32(dst + i * self.pitch, v)?;
        }
        Ok(())
    }

    fn fill_64(&mut self, mut dst: isize, pix: u64, n: u32) -> Result<(), Error> {
        for _ in 0..n {
            self.w64(dst, pix)?;
            dst += self.pitch;
        }
        Ok(())
    }

    /// Averages 4 pixels of the reference with those of `b` into `dst`,
    /// without rounding.
    fn avg_32(&mut self, dst: isize, rf: isize, in_cur: bool, b: isize) -> Result<(), Error> {
        let v = self.r32(in_cur, rf)?.wrapping_add(self.r32(true, b)?);
        self.w32(dst, (v >> 1) & 0x7F7F_7F7F)
    }

    fn avg_64(&mut self, dst: isize, rf: isize, in_cur: bool, b: isize) -> Result<(), Error> {
        let v = self.r64(in_cur, rf)?.wrapping_add(self.r64(true, b)?);
        self.w64(dst, (v >> 1) & 0x7F7F_7F7F_7F7F_7F7F)
    }

    /// Mode 10 INTRA: fills `2 * n` rows of 8 pixels from the reference.
    /// The top row of a cell is interpolated from the pixel above and
    /// the row below, where only even pixels are taken.
    fn lines_copy_m10(
        &mut self,
        dst: isize,
        rf: isize,
        in_cur: bool,
        top: bool,
        n: u32,
    ) -> Result<(), Error> {
        let pix = self.r64(in_cur, rf)?;
        if top {
            self.fill_64(dst + self.pitch, replicate64(pix), n * 2 - 1)?;
            self.avg_64(dst, rf, in_cur, dst + self.pitch)
        } else {
            self.fill_64(dst, pix, n * 2)
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn decode_cell_data(
        &mut self,
        c: &Cell,
        mut block: isize,
        mut ref_block: isize,
        h_zoom: u32,
        v_zoom: u32,
        args: &CellArgs,
        data_ptr: &mut usize,
    ) -> Result<(), Error> {
        let CellArgs {
            mode,
            delta,
            swap_quads,
            ref_cur: rc,
        } = *args;
        let row = self.pitch;
        let blk_row_offset = (row << (2 + v_zoom)) - (c.width as isize * 4);
        let line_offset = if v_zoom != 0 { row } else { 0 };
        let inter = c.mv.is_some();
        let m10_intra = mode == 10 && !inter;

        if c.height & v_zoom as i32 != 0 || c.width & h_zoom as i32 != 0 {
            return Err(BAD_DATA);
        }

        let mut rle_blocks = 0i32;
        let mut skip_flag = false;
        let mut is_first_row = true;
        let mut y = 0;
        while y < c.height {
            let mut x = 0;
            while x < c.width {
                let mut rf = ref_block;
                let mut dst = block;

                if rle_blocks > 0 {
                    if mode <= 4 {
                        if inter || !skip_flag {
                            self.copy_block4(dst, rf, rc, 4 << v_zoom)?;
                        }
                    } else if m10_intra {
                        self.lines_copy_m10(dst, rf, rc, is_first_row, 4)?;
                    }
                    rle_blocks -= 1;
                } else {
                    let mut line = 0i32;
                    while line < 4 {
                        let mut num_lines = 1i32;
                        let is_top = is_first_row && line == 0;
                        // The primary table for odd lines, the secondary
                        // for even ones.
                        let dt: &VqEntry = if mode <= 4 {
                            delta[(line & 1) as usize]
                        } else {
                            delta[1]
                        };
                        let mut code = self.next_byte(data_ptr)?;
                        if code < 248 {
                            let (mut dyad1, mut dyad2);
                            if code < dt.num_dyads {
                                dyad1 = self.next_byte(data_ptr)? as usize;
                                dyad2 = code as usize;
                                if dyad1 >= dt.num_dyads as usize || dyad1 >= 248 {
                                    return Err(BAD_DATA);
                                }
                            } else {
                                // Quads.
                                let q = (code - dt.num_dyads) as usize;
                                dyad1 = q / dt.quad_exp as usize;
                                dyad2 = q % dt.quad_exp as usize;
                                if swap_quads[(line & 1) as usize] {
                                    std::mem::swap(&mut dyad1, &mut dyad2);
                                }
                            }
                            if mode <= 4 {
                                let d = dt.deltas;
                                let v = self.r16(rc, rf)?.wrapping_add(d[dyad1]) & 0x7F7F;
                                self.w16(dst + line_offset, v)?;
                                let v = self.r16(rc, rf + 2)?.wrapping_add(d[dyad2]) & 0x7F7F;
                                self.w16(dst + line_offset + 2, v)?;
                                if mode >= 3 {
                                    if is_top && c.ypos == 0 {
                                        let v = self.r32(true, dst + row)?;
                                        self.w32(dst, v)?;
                                    } else {
                                        self.avg_32(dst, rf, rc, dst + row)?;
                                    }
                                }
                            } else if m10_intra {
                                // Two 32-bit deltas to the next even
                                // line; odd lines are interpolated, or
                                // replicated on the top of the picture.
                                let d = dt.deltas_m10;
                                let (mut a, mut b) = (self.r32(rc, rf)?, self.r32(rc, rf + 4)?);
                                if is_top {
                                    a = replicate32(a);
                                    b = replicate32(b);
                                }
                                self.w32(dst + row, a.wrapping_add(d[dyad1]) & 0x7F7F_7F7F)?;
                                self.w32(dst + row + 4, b.wrapping_add(d[dyad2]) & 0x7F7F_7F7F)?;
                                if is_top && c.ypos == 0 {
                                    let v = self.r64(true, dst + row)?;
                                    self.w64(dst, v)?;
                                } else {
                                    self.avg_64(dst, rf, rc, dst + row)?;
                                }
                            } else if mode == 10 {
                                // Modes 10 and 11 INTER add to the cell.
                                let d = dt.deltas_m10;
                                for o in [dst, dst + row] {
                                    let v = self.r32(true, o)?.wrapping_add(d[dyad1]);
                                    self.w32(o, v & 0x7F7F_7F7F)?;
                                    let v = self.r32(true, o + 4)?.wrapping_add(d[dyad2]);
                                    self.w32(o + 4, v & 0x7F7F_7F7F)?;
                                }
                            } else {
                                let d = dt.deltas;
                                for o in [dst, dst + row] {
                                    let v = self.r16(true, o)?.wrapping_add(d[dyad1]);
                                    self.w16(o, v & 0x7F7F)?;
                                    let v = self.r16(true, o + 2)?.wrapping_add(d[dyad2]);
                                    self.w16(o + 2, v & 0x7F7F)?;
                                }
                            }
                        } else {
                            // RLE codes.
                            match code {
                                RLE_ESC_FC | RLE_ESC_FF | RLE_ESC_FE | RLE_ESC_FD => {
                                    if code == RLE_ESC_FC {
                                        skip_flag = false;
                                        rle_blocks = 1;
                                        code = RLE_ESC_FD;
                                    }
                                    num_lines = 257 - code as i32 - line;
                                    if num_lines <= 0 {
                                        return Err(Error::Invalid("RLE code not allowed here"));
                                    }
                                    if mode <= 4 {
                                        let n = (num_lines as u32) << v_zoom;
                                        self.copy_block4(dst, rf, rc, n)?;
                                    } else if m10_intra {
                                        let n = num_lines as u32;
                                        self.lines_copy_m10(dst, rf, rc, is_top, n)?;
                                    }
                                }
                                RLE_ESC_FB => {
                                    let code = self.next_byte(data_ptr)?;
                                    rle_blocks = (code & 0x1F) as i32 - 1;
                                    if code >= 64 || rle_blocks < 0 {
                                        return Err(Error::Invalid("bad RLE FB counter"));
                                    }
                                    skip_flag = code & 0x20 != 0;
                                    num_lines = 4 - line;
                                    if mode >= 10 || inter || !skip_flag {
                                        if mode <= 4 {
                                            let n = (num_lines as u32) << v_zoom;
                                            self.copy_block4(dst, rf, rc, n)?;
                                        } else if m10_intra {
                                            let n = num_lines as u32;
                                            self.lines_copy_m10(dst, rf, rc, is_top, n)?;
                                        }
                                    }
                                }
                                RLE_ESC_F9 | RLE_ESC_FA => {
                                    if code == RLE_ESC_F9 {
                                        skip_flag = true;
                                        rle_blocks = 1;
                                    }
                                    if line != 0 {
                                        return Err(Error::Invalid("RLE code not allowed here"));
                                    }
                                    num_lines = 4;
                                    if inter && mode <= 4 {
                                        self.copy_block4(dst, rf, rc, 4 << v_zoom)?;
                                    }
                                }
                                _ => return Err(Error::Unsupported("RLE code F8")),
                            }
                        }

                        line += num_lines;
                        rf += row * ((num_lines as isize) << v_zoom);
                        dst += row * ((num_lines as isize) << v_zoom);
                    }
                }

                // The next block across.
                block += 4 << h_zoom;
                ref_block += 4 << h_zoom;
                x += 1 + h_zoom as i32;
            }

            // The next row of blocks.
            ref_block += blk_row_offset;
            block += blk_row_offset;
            is_first_row = false;
            y += 1 + v_zoom as i32;
        }
        Ok(())
    }
}

/// What a cell's mode selects for its blocks.
struct CellArgs {
    mode: u8,
    delta: [&'static VqEntry; 2],
    swap_quads: [bool; 2],
    /// Whether the reference is in the buffer being decoded.
    ref_cur: bool,
}
