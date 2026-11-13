//! Tests of the whole decoder on synthetic bitstreams, and of the
//! four-band recombination.

use super::band::RvMap;
use super::headers::{BandInfo, GopHeader};
use super::huff::{BLOCK_PRESETS, MB_PRESETS};
use super::synth::{OutPlane, Window, four_band};
use super::*;

/// Writes fields least significant bit first, the way the decoder reads
/// them.
struct BitWriter {
    bytes: Vec<u8>,
    bit: usize,
}

impl BitWriter {
    fn new() -> Self {
        BitWriter {
            bytes: Vec::new(),
            bit: 0,
        }
    }

    fn put(&mut self, value: u32, n: u32) {
        for i in 0..n {
            if self.bit.is_multiple_of(8) {
                self.bytes.push(0);
            }
            let b = (value >> i) & 1;
            *self.bytes.last_mut().unwrap() |= (b as u8) << (self.bit % 8);
            self.bit += 1;
        }
    }

    fn align(&mut self) {
        self.bit = self.bit.next_multiple_of(8);
    }

    /// Write symbol `s` of the code described by `rows`.
    fn sym(&mut self, rows: &[u8], s: u32) {
        let mut base = 0;
        for (i, &w) in rows.iter().enumerate() {
            let size = 1u32 << w;
            if s < base + size {
                let last = i == rows.len() - 1;
                self.put((1 << i) - 1, i as u32);
                if !last {
                    self.put(0, 1);
                }
                let x = s - base;
                for j in (0..w as u32).rev() {
                    self.put((x >> j) & 1, 1);
                }
                return;
            }
            base += size;
        }
        panic!("symbol {s} not in the code");
    }
}

/// The value code that dequantises to `d` at step 1 (spec 05, 2.3).
fn code_for(d: i32) -> u16 {
    if d > 0 {
        (2 * d - 1) as u16
    } else {
        (-2 * d) as u16
    }
}

/// Write one coded block holding only a DC value `d` (step 1), with the
/// default run/value table and block code.
fn dc_block(w: &mut BitWriter, rv: &RvMap, d: i32) {
    let rows = BLOCK_PRESETS[7];
    w.sym(rows, rv.symbol_for(1, code_for(d)).expect("run 1 symbol"));
    w.sym(rows, rv.eob());
}

/// Picture start, then for an intra frame a GOP header of a custom
/// `width` by `height` picture with one luma band (16x16 macroblocks,
/// 8x8 blocks) and one chroma band (4x4 macroblocks and blocks).
fn picture_start(w: &mut BitWriter, ftype: u32, number: u32, size: Option<(u32, u32)>) {
    w.put(0x1f, 5);
    w.put(ftype, 3);
    w.put(number, 8);
    if let Some((width, height)) = size {
        w.put(0, 8); // gop_flags
        w.put(0, 3); // subsampling: 1 luma band, 1 chroma band
        w.put(15, 4); // custom size
        w.put((width << 13) | height, 26);
        w.put(0, 6); // luma band info: mb 16, blk 8
        w.put(6, 6); // chroma band info: mb 4, blk 4
        w.align();
        w.put(2 << 19, 24); // version 2
        w.align();
    }
}

/// Frame header with `flags` and default tables.
fn frame_header(w: &mut BitWriter, flags: u32) {
    w.put(flags, 8);
    w.put(0, 3); // plane filter bits
    w.align();
}

/// Two empty chroma bands (V, then U).
fn empty_chroma(w: &mut BitWriter) {
    w.put(1, 8);
    w.put(1, 8);
}

/// DC values of the four intra blocks of the test picture. With DC
/// prediction the blocks hold (P + 1) >> 1 for the running sums P = 20,
/// 40, 14, 30: 10, 20, 7 and 15.
const DCS: [i32; 4] = [20, 20, -26, 16];

/// An intra frame of a 16 by 8 picture whose one luma macroblock codes
/// the four blocks of `DCS`.
fn intra_frame() -> Vec<u8> {
    let rv = RvMap::new(8, &[]).unwrap();
    let mut w = BitWriter::new();
    picture_start(&mut w, 0, 0, Some((16, 8)));
    frame_header(&mut w, 0);
    // Luma band header: no flags, no checksum, gq 0.
    w.put(0, 8);
    w.put(0, 1);
    w.put(0, 5);
    w.align();
    // Tile: coded, no size.
    w.put(0, 1);
    w.put(0, 1);
    w.align();
    // Macroblock: coded, all four blocks.
    w.put(0, 1);
    w.put(0xf, 4);
    w.align();
    for d in DCS {
        dc_block(&mut w, &rv, d);
    }
    w.align();
    empty_chroma(&mut w);
    w.bytes
}

/// An inter frame (type 1) whose luma macroblock is inter with vector
/// (0, 8), so its top blocks predict from rows 8 to 15, the band padding
/// below the 8-row picture. Block 0 adds a residual of DC 10, which the
/// transform makes a constant 5. The frame carries band sizes.
fn inter_frame(number: u32) -> Vec<u8> {
    let rv = RvMap::new(8, &[]).unwrap();
    let mut w = BitWriter::new();
    picture_start(&mut w, 1, number, None);
    frame_header(&mut w, 0x80);
    let band_start = w.bytes.len();
    w.put(0, 8);
    w.put(0, 24); // band size, patched below
    w.put(0, 1);
    w.put(0, 5);
    w.align();
    w.put(0, 1);
    w.put(0, 1);
    w.align();
    w.put(0, 1); // coded
    w.put(1, 1); // inter
    w.put(1, 4); // cbp: block 0
    let mb = MB_PRESETS[7];
    w.sym(mb, 15); // dy = +8
    w.sym(mb, 0); // dx = 0
    w.align();
    dc_block(&mut w, &rv, 10);
    w.align();
    let size = (w.bytes.len() - band_start) as u32;
    w.bytes[band_start + 1..band_start + 4].copy_from_slice(&size.to_le_bytes()[..3]);
    empty_chroma(&mut w);
    w.bytes
}

fn rows(p: Iv50Plane) -> Vec<Vec<u8>> {
    (0..p.height).map(|r| p.row(r).to_vec()).collect()
}

#[test]
fn custom_size_is_height_then_width() {
    let mut dec = Iv50Decoder::new();
    assert_eq!(dec.decode(&intra_frame()), Ok(FrameType::Intra));
    // The 26-bit field holds the height in bits 0..12 and the width in
    // bits 13..25 (spec 02, 1.3).
    assert_eq!(dec.dimensions(), Some((16, 8)));
    let y = dec.y().unwrap();
    assert_eq!((y.width, y.height), (16, 8));
    let u = dec.u().unwrap();
    assert_eq!((u.width, u.height), (4, 2));
}

#[test]
fn intra_blocks_use_dc_prediction() {
    let mut dec = Iv50Decoder::new();
    dec.decode(&intra_frame()).unwrap();
    for row in rows(dec.y().unwrap()) {
        assert_eq!(&row[..8], &[128 + 10; 8]);
        assert_eq!(&row[8..], &[128 + 20; 8]);
    }
    for p in [dec.u().unwrap(), dec.v().unwrap()] {
        assert!(rows(p).iter().flatten().all(|&v| v == 128));
    }
}

#[test]
fn inter_residual_adds_to_prediction_from_band_padding() {
    let mut dec = Iv50Decoder::new();
    dec.decode(&intra_frame()).unwrap();
    assert_eq!(dec.decode(&inter_frame(1)), Ok(FrameType::Inter));
    // Block 0 = block 2 of the reference (7) + residual 5; block 1 is
    // uncoded and takes block 3 of the reference (15).
    for row in rows(dec.y().unwrap()) {
        assert_eq!(&row[..8], &[128 + 12; 8]);
        assert_eq!(&row[8..], &[128 + 15; 8]);
    }
}

#[test]
fn repeated_frame_number_is_a_null_frame() {
    let mut dec = Iv50Decoder::new();
    dec.decode(&intra_frame()).unwrap();
    dec.decode(&inter_frame(1)).unwrap();
    let before = rows(dec.y().unwrap());
    // Same number as the previous frame: NULL, the picture stays.
    assert_eq!(dec.decode(&inter_frame(1)), Ok(FrameType::Null));
    assert_eq!(rows(dec.y().unwrap()), before);
    // A new number decodes again, from the inter frame's picture.
    dec.decode(&inter_frame(2)).unwrap();
    assert_ne!(rows(dec.y().unwrap()), before);
}

#[test]
fn bad_headers_are_rejected() {
    let mut dec = Iv50Decoder::new();
    assert_eq!(dec.decode(&[0x1e, 0]), Err(Iv50Error::BadPictureStart));
    assert_eq!(
        dec.decode(&[0x1f | (5 << 5), 0]),
        Err(Iv50Error::BadPictureStart)
    );
    assert_eq!(dec.decode(&inter_frame(1)), Err(Iv50Error::NoReference));
    // Subsampling code 3.
    let mut w = BitWriter::new();
    w.put(0x1f, 5);
    w.put(0, 3);
    w.put(0, 8);
    w.put(0, 8);
    w.put(3, 3);
    w.put(0, 32);
    assert_eq!(
        dec.decode(&w.bytes),
        Err(Iv50Error::BadHeader("subsampling code"))
    );
    // Several tiles and half-size chroma are refused.
    for flags in [0x40, 0x02] {
        let mut w = BitWriter::new();
        w.put(0x1f, 5);
        w.put(0, 3);
        w.put(0, 8);
        w.put(flags, 8);
        w.put(0, 32);
        w.put(0, 32);
        assert!(matches!(
            dec.decode(&w.bytes),
            Err(Iv50Error::Unsupported(_))
        ));
    }
    // Seven luma bands (codes 2 and 6) are refused, also when every band
    // of the frame is empty.
    for sub in [2, 6] {
        let (luma, chroma) = if sub == 2 { (7, 1) } else { (7, 4) };
        let mut w = BitWriter::new();
        w.put(0x1f, 5);
        w.put(0, 3);
        w.put(0, 8);
        w.put(0, 8);
        w.put(sub, 3);
        w.put(15, 4);
        w.put((16 << 13) | 8, 26);
        for _ in 0..luma {
            w.put(0, 6);
        }
        for _ in 0..chroma {
            w.put(6, 6);
        }
        w.align();
        w.put(2 << 19, 24);
        w.align();
        frame_header(&mut w, 0);
        for _ in 0..luma + 2 * chroma {
            w.put(1, 8);
        }
        assert!(matches!(
            Iv50Decoder::new().decode(&w.bytes),
            Err(Iv50Error::Unsupported(_))
        ));
    }
    // Truncated band data.
    let f = intra_frame();
    assert!(dec.decode(&f[..f.len() - 4]).is_err());
}

/// A plane of 64 by 32 with four luma bands of 32 by 16.
fn four_band_layout() -> PlaneLayout {
    let info = BandInfo {
        half_pel: false,
        mb_size: 8,
        blk_size: 8,
        xform: 0,
    };
    let gop = GopHeader {
        flags: 0,
        luma_bands: 4,
        chroma_bands: 1,
        width: 64,
        height: 32,
        luma_info: vec![info; 4],
        chroma_info: vec![info],
        transparency: false,
        intra_only: false,
    };
    build_layout(&gop).swap_remove(0)
}

/// Recombine the whole 64 by 32 plane with band `b` set by `f(row, col)`
/// over its whole buffer and the other bands zero.
fn synth_one(b: usize, f: impl Fn(usize, usize) -> i16) -> OutPlane {
    let lay = four_band_layout();
    let mut area = vec![0i16; lay.area_len];
    let g = &lay.bands[b];
    for r in 0..16 {
        for c in 0..g.pitch {
            area[g.offset + r * g.pitch + c] = f(r, c);
        }
    }
    let mut out = OutPlane::new(64, 32);
    let win = Window {
        y: 0,
        x: 0,
        h: 32,
        w: 64,
    };
    four_band(&area, &lay.bands, win, &mut out);
    out
}

fn px(out: &OutPlane, r: usize, c: usize) -> u8 {
    out.data[r * out.stride + c]
}

#[test]
fn four_band_low_band_is_upsampled() {
    // L = 16 * c: even columns 16 * L >> 6 = 4c, odd columns average
    // with the next sample.
    let out = synth_one(0, |_, c| 16 * c as i16);
    for r in 0..32 {
        for c in 0..31 {
            assert_eq!(px(&out, r, 2 * c), 128 + 4 * c as u8);
            assert_eq!(px(&out, r, 2 * c + 1), 128 + 4 * c as u8 + 2);
        }
        // The last written column takes its left neighbour's value.
        assert_eq!(px(&out, r, 63), px(&out, r, 62));
    }
}

#[test]
fn four_band_constant_high_bands() {
    // A constant V band of 64 adds +16 to even rows and -16 to odd rows
    // (spec 08, 8.8 gives the same pattern for a V band of 1, scaled).
    let out = synth_one(1, |_, _| 64);
    for r in 0..32 {
        let want = if r % 2 == 0 { 144 } else { 112 };
        assert!((0..64).all(|c| px(&out, r, c) == want), "row {r}");
    }
    // A constant D band of 64: ee and oo +16, eo and oe -16.
    let out = synth_one(3, |_, _| 64);
    for r in 0..32 {
        for c in 0..63 {
            let want = if (r + c) % 2 == 0 { 144 } else { 112 };
            assert_eq!(px(&out, r, c), want, "row {r} col {c}");
        }
    }
}

#[test]
fn four_band_first_column_repeats_itself() {
    // H nonzero only in band column 0: the left neighbour of column 0 is
    // column 0 itself, so ee = 8 * (H + H) = 8 * 128.
    let out = synth_one(2, |_, c| if c == 0 { 64 } else { 0 });
    for r in (0..32).step_by(2) {
        assert_eq!(px(&out, r, 0), 128 + 16);
        // Band column 1: s2 = H[1] + H[0] = 64.
        assert_eq!(px(&out, r, 2), 128 + 8);
    }
}

/// An intra frame like [`intra_frame`] in a stream with transparency
/// (gop_flags bit 3), whose trailer carries `rects` as (x, y, w, h).
fn transparent_intra_frame(rects: &[(u32, u32, u32, u32)]) -> Vec<u8> {
    let rv = RvMap::new(8, &[]).unwrap();
    let mut w = BitWriter::new();
    w.put(0x1f, 5);
    w.put(0, 3);
    w.put(0, 8);
    w.put(8, 8); // gop_flags: transparency
    w.put(0, 3);
    w.put(15, 4);
    w.put((16 << 13) | 8, 26);
    w.put(0, 6);
    w.put(6, 6);
    w.put(0, 4); // transparency flags: no colour
    w.align();
    w.put(2 << 19, 24);
    w.align();
    frame_header(&mut w, 0);
    w.put(0, 8);
    w.put(0, 1);
    w.put(0, 5);
    w.align();
    w.put(0, 1);
    w.put(0, 1);
    w.align();
    w.put(0, 1);
    w.put(0xf, 4);
    w.align();
    for d in DCS {
        dc_block(&mut w, &rv, d);
    }
    w.align();
    empty_chroma(&mut w);
    // Trailer: rectangle count, the rectangles, mask flag, no
    // descriptor.
    w.put(rects.len() as u32, 3);
    for &(x, y, rw, rh) in rects {
        w.put(x, 16);
        w.put(y, 16);
        w.put(rw, 16);
        w.put(rh, 16);
    }
    w.put(0, 2);
    w.align();
    w.bytes
}

#[test]
fn transparency_rectangles_outside_the_picture_write_nothing() {
    for rect in [
        (60000, 0, 10, 10),
        (0, 60000, 10, 10),
        (65535, 65535, 65535, 65535),
    ] {
        let mut dec = Iv50Decoder::new();
        assert_eq!(
            dec.decode(&transparent_intra_frame(&[rect])),
            Ok(FrameType::Intra)
        );
        // The window lies outside the picture, so nothing is
        // recombined and the planes keep their initial zeros.
        assert!(rows(dec.y().unwrap()).iter().flatten().all(|&v| v == 0));
    }
}

#[test]
fn failed_intra_frame_with_a_new_size_leaves_no_picture() {
    let mut dec = Iv50Decoder::new();
    dec.decode(&intra_frame()).unwrap();
    // A different size whose band data is missing: the GOP header is
    // taken, which clears the buffers, and then the frame fails.
    let mut w = BitWriter::new();
    picture_start(&mut w, 0, 1, Some((32, 8)));
    frame_header(&mut w, 0);
    assert_eq!(dec.decode(&w.bytes), Err(Iv50Error::Truncated));
    assert!(dec.y().is_none());
    assert_eq!(dec.decode(&inter_frame(2)), Err(Iv50Error::NoReference));
    // A failed frame of the same size keeps the picture.
    dec.decode(&intra_frame()).unwrap();
    let f = intra_frame();
    assert!(dec.decode(&f[..f.len() - 4]).is_err());
    assert!(dec.y().is_some());
}
