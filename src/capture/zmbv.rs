//! The Zip Motion Block Video codec (ZMBV), DOSBox's for its AVI captures,
//! which ffmpeg, VLC and mpv decode: lossless, and small for a DOS screen,
//! where little changes from one frame to the next.
//!
//! A frame is a flags byte (bit 0: a keyframe), on a keyframe a header
//! (version 0.1, zlib, the pixel format, 16x16 blocks), then zlib data: a
//! keyframe's pixels, or for each 16x16 block a motion vector and whether
//! XOR data follows, then the XOR of each changed block with the previous
//! frame. One zlib stream runs from a keyframe to the next, each frame
//! flushed to a byte boundary.

use crate::video::Frame;
use flate2::{Compress, Compression, FlushCompress};

const BLOCK: usize = 16;
/// 32-bit pixels: blue, green, red and a zero byte.
const FORMAT_32BPP: u8 = 8;
/// A keyframe every this many frames, so players can seek.
const KEYFRAME_EVERY: u64 = 300;

pub struct Encoder {
    width: usize,
    height: usize,
    /// The previous frame's pixels, 4 bytes each.
    previous: Vec<u8>,
    zlib: Compress,
    frames: u64,
}

impl Encoder {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            previous: vec![0; width * height * 4],
            zlib: Compress::new(Compression::fast(), true),
            frames: 0,
        }
    }

    /// Encode the next frame (RGB24, `width` x `height`). Returns its bytes
    /// and whether it is a keyframe.
    pub fn encode(&mut self, rgb: &[u8]) -> (Vec<u8>, bool) {
        let pixels: Vec<u8> = rgb.as_chunks::<3>().0.iter().flat_map(|&[r, g, b]| [b, g, r, 0]).collect();
        let keyframe = self.frames.is_multiple_of(KEYFRAME_EVERY);
        self.frames += 1;
        let mut out = Vec::new();
        let data = if keyframe {
            out.extend_from_slice(&[0x01, 0, 1, 1, FORMAT_32BPP, BLOCK as u8, BLOCK as u8]);
            self.zlib.reset();
            pixels.clone()
        } else {
            out.push(0x00);
            self.delta(&pixels)
        };
        self.deflate(&data, &mut out);
        self.previous = pixels;
        (out, keyframe)
    }

    /// A delta frame's data: a vector for each block (always 0, 0; bit 0 of
    /// the first byte if the block changed), padded to 4 bytes, then the
    /// changed blocks XORed with the previous frame, row by row.
    fn delta(&self, pixels: &[u8]) -> Vec<u8> {
        let (blocks_x, blocks_y) = (self.width.div_ceil(BLOCK), self.height.div_ceil(BLOCK));
        let table = (blocks_x * blocks_y * 2 + 3) & !3;
        let mut data = vec![0u8; table];
        let pitch = self.width * 4;
        for by in 0..blocks_y {
            for bx in 0..blocks_x {
                let (x0, y0) = (bx * BLOCK * 4, by * BLOCK);
                let (w, h) = ((BLOCK * 4).min(pitch - x0), BLOCK.min(self.height - y0));
                let rows = (y0..y0 + h).map(|y| y * pitch + x0..y * pitch + x0 + w);
                if rows.clone().all(|row| pixels[row.clone()] == self.previous[row]) {
                    continue;
                }
                data[(by * blocks_x + bx) * 2] = 1;
                for row in rows {
                    data.extend(pixels[row.clone()].iter().zip(&self.previous[row]).map(|(new, old)| new ^ old));
                }
            }
        }
        data
    }

    /// Compress `data` into `out`, flushing to a byte boundary.
    fn deflate(&mut self, data: &[u8], out: &mut Vec<u8>) {
        let start = self.zlib.total_in();
        loop {
            let done = (self.zlib.total_in() - start) as usize;
            out.reserve(data.len() - done + 1024);
            self.zlib.compress_vec(&data[done..], out, FlushCompress::Sync).expect("zlib");
            // Flushed once all is in and the output had room to spare.
            if (self.zlib.total_in() - start) as usize == data.len() && out.len() < out.capacity() {
                break;
            }
        }
    }
}

/// Scale `frame` to `width` x `height`, nearest neighbour, if it isn't that
/// size: a recording keeps the size it started with.
pub fn fitted(frame: &Frame, width: usize, height: usize) -> std::borrow::Cow<'_, [u8]> {
    if (frame.width as usize, frame.height as usize) == (width, height) {
        return std::borrow::Cow::Borrowed(&frame.rgb);
    }
    let mut rgb = Vec::with_capacity(width * height * 3);
    for y in 0..height {
        let sy = y * frame.height as usize / height;
        for x in 0..width {
            let at = (sy * frame.width as usize + x * frame.width as usize / width) * 3;
            rgb.extend_from_slice(&frame.rgb[at..at + 3]);
        }
    }
    std::borrow::Cow::Owned(rgb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Decompress, FlushDecompress};

    /// A ZMBV decoder for 32-bit frames with zero vectors, as the encoder
    /// makes them.
    struct Decoder {
        width: usize,
        height: usize,
        zlib: Decompress,
        pixels: Vec<u8>,
    }

    impl Decoder {
        fn decode(&mut self, frame: &[u8]) -> Vec<u8> {
            let data = if frame[0] & 1 != 0 {
                assert_eq!(&frame[1..7], &[0, 1, 1, FORMAT_32BPP, 16, 16]);
                self.zlib = Decompress::new(true);
                &frame[7..]
            } else {
                &frame[1..]
            };
            let mut raw = Vec::with_capacity(self.width * self.height * 4 + 4096);
            self.zlib.decompress_vec(data, &mut raw, FlushDecompress::Sync).unwrap();
            if frame[0] & 1 != 0 {
                self.pixels = raw;
            } else {
                let (bx_n, by_n) = (self.width.div_ceil(16), self.height.div_ceil(16));
                let mut at = (bx_n * by_n * 2 + 3) & !3;
                for by in 0..by_n {
                    for bx in 0..bx_n {
                        let vector = &raw[(by * bx_n + bx) * 2..][..2];
                        assert_eq!((vector[0] & !1, vector[1]), (0, 0));
                        if vector[0] & 1 == 0 {
                            continue;
                        }
                        for y in by * 16..(by * 16 + 16).min(self.height) {
                            for x in bx * 16 * 4..((bx * 16 + 16).min(self.width)) * 4 {
                                self.pixels[y * self.width * 4 + x] ^= raw[at];
                                at += 1;
                            }
                        }
                    }
                }
                assert_eq!(at, raw.len());
            }
            self.pixels.as_chunks::<4>().0.iter().flat_map(|&[b, g, r, _]| [r, g, b]).collect()
        }
    }

    #[test]
    fn frames_decode_back() {
        // Not a multiple of 16, to have edge blocks.
        let (width, height) = (40, 20);
        let mut encoder = Encoder::new(width, height);
        let mut decoder = Decoder { width, height, zlib: Decompress::new(true), pixels: Vec::new() };
        let mut frame: Vec<u8> = (0..width * height * 3).map(|i| (i * 7 % 251) as u8).collect();
        let (bytes, keyframe) = encoder.encode(&frame);
        assert!(keyframe);
        assert_eq!(decoder.decode(&bytes), frame);

        // Change a pixel in the last, partial block.
        frame[(19 * width + 39) * 3] ^= 0xFF;
        let (bytes, keyframe) = encoder.encode(&frame);
        assert!(!keyframe);
        assert_eq!(decoder.decode(&bytes), frame);

        // Nothing changes: a small frame.
        let (bytes, _) = encoder.encode(&frame);
        assert!(bytes.len() < 32, "{} bytes", bytes.len());
        assert_eq!(decoder.decode(&bytes), frame);
    }

    #[test]
    fn frames_of_another_size_are_scaled() {
        let mut frame = Frame::new(4, 2);
        frame.rgb = (0..24).collect();
        assert!(matches!(fitted(&frame, 4, 2), std::borrow::Cow::Borrowed(_)));
        let half = fitted(&frame, 2, 1);
        assert_eq!(&half[..], &[0, 1, 2, 6, 7, 8]);
    }
}
