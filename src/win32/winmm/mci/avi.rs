//! AVI files, as the AVIVideo and MPEGVideo devices (video.rs) play
//! them: the first video stream and the first audio stream of a file
//! that the Video for Windows code (`vfw::avi`) reads. Its pictures are
//! uncompressed DIBs of 8, 16, 24 or 32 bits, 8-bit Microsoft RLE
//! (BI_RLE8), which MCIAVI decodes itself, or what a Video for Windows
//! codec decompresses (`vfw::codec`: Indeo 3, 4 and 5). Its sound is
//! PCM, or MS or IMA ADPCM, which ACM's codecs give MCIAVI as PCM. A
//! file of other sound plays without it.
//!
//! The sound is read whole when the file opens, to be played as one
//! sound. The pictures are decoded as they are shown: a decoder goes on
//! from the frame it showed last, and a seek starts it again at the key
//! frame before the one asked for. An RLE frame after the first draws
//! only what changed onto the picture before it.

use super::media::{Kind, Media};
use crate::win32::dsound::device::{Format, WAVE_FORMAT_PCM};
use crate::win32::gdi32::bitmap::rle_onto;
use crate::win32::gdi32::surface::dib_stride;
use crate::win32::msacm32::adpcm;
use crate::win32::vfw::avi::{self, AVIIF_KEYFRAME, Chunk};
use crate::win32::vfw::codec::{self, Codec};

const VIDS: u32 = u32::from_le_bytes(*b"vids");
const AUDS: u32 = u32::from_le_bytes(*b"auds");
const BI_RGB: u32 = 0;
const BI_RLE8: u32 = 1;

/// The largest picture decoded.
const MAX_SIZE: u32 = 1024;

/// A file's video stream: the file, where its frames are, and their
/// format.
#[derive(Default)]
pub struct Video {
    data: Vec<u8>,
    chunks: Vec<Chunk>,
    compression: u32,
    bits: u16,
    /// Whether the rows run from the top (a negative biHeight).
    top_down: bool,
    /// An 8-bit picture's colours, as blue, green and red.
    palette: Vec<[u8; 3]>,
}

impl Video {
    /// The bytes of its frames.
    pub fn bytes(&self) -> usize {
        self.chunks.iter().map(|c| c.size as usize).sum()
    }
}

fn le16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// The bytes of `c` in the file `d`.
fn bytes<'a>(d: &'a [u8], c: &Chunk) -> &'a [u8] {
    let at = (c.offset as usize).min(d.len());
    &d[at..(at + c.size as usize).min(d.len())]
}

/// Reads the file `d`: None when it isn't an AVI file, or has no stream
/// that plays.
pub fn parse(d: &[u8]) -> Option<Media> {
    let len = u32::try_from(d.len()).ok()?;
    let file = avi::parse(len, &mut |at, n| {
        let at = (at as usize).min(d.len());
        Some(d[at..(at + n as usize).min(d.len())].to_vec())
    })
    .ok()?;
    let mut media = Media::default();
    if let Some(s) = file.streams.iter().find(|s| s.kind() == VIDS) {
        let f = &s.format;
        let width = le32(f, 4)? as i32;
        let height = le32(f, 8)? as i32;
        let bits = le16(f, 14)?;
        let compression = le32(f, 16)?;
        let (w, h) = (width.unsigned_abs(), height.unsigned_abs());
        let plays = match compression {
            BI_RGB => matches!(bits, 8 | 16 | 24 | 32),
            BI_RLE8 => bits == 8,
            c => codec::supported(c, w, h),
        };
        if plays && (1..=MAX_SIZE).contains(&w) && (1..=MAX_SIZE).contains(&h) {
            let mut palette = Vec::new();
            if bits == 8 {
                let size = le32(f, 0)? as usize;
                let used = match le32(f, 32)? {
                    0 => 256,
                    n => n.min(256) as usize,
                };
                for i in 0..used {
                    let q = f.get(size + i * 4..size + i * 4 + 3).unwrap_or(&[0; 3]);
                    palette.push([q[0], q[1], q[2]]);
                }
            }
            media.width = w;
            media.height = h;
            media.frames = s.chunks.len() as u32;
            media.rate = (s.rate().max(1), s.scale().max(1));
            media.video = Kind::Avi(Video {
                data: d.to_vec(),
                chunks: s.chunks.clone(),
                compression,
                bits,
                top_down: height < 0 && compression == BI_RGB,
                palette,
            });
        }
    }
    if let Some(s) = file.streams.iter().find(|s| s.kind() == AUDS) {
        media.audio = audio(d, s);
    }
    (media.frames > 0 || media.audio.is_some()).then_some(media)
}

/// The sound of the audio stream `s` of the file `d` as PCM, if it is
/// PCM or ADPCM: None for another format, or a format too short to say,
/// which leaves the file's video to play without it.
fn audio(d: &[u8], s: &avi::Stream) -> Option<(Format, Vec<u8>)> {
    let f = &s.format;
    let data = || -> Vec<u8> { s.chunks.iter().flat_map(|c| bytes(d, c)).copied().collect() };
    if le16(f, 0)? == WAVE_FORMAT_PCM {
        let format = Format::pcm(le32(f, 4)?, le16(f, 14)?, le16(f, 2)?);
        let plays =
            format.rate > 0 && matches!(format.bits, 8 | 16) && matches!(format.channels, 1 | 2);
        return plays.then(|| (format, data()));
    }
    let (adpcm, rate) = adpcm::Format::from_wave_format(f)?;
    // ACM decodes it to 16-bit PCM of its rate and channels, block by
    // block here. msadp32.acm writes nothing for a conversion that meets
    // a block it can't decode, and MCIAVI converts the sound a buffer at
    // a time as it plays: such a block is silence, which keeps the sound
    // after it in step with the pictures, where decoding the file in one
    // conversion would silence the whole of it.
    let block = adpcm.block_align as usize;
    // A block's 16-bit samples, all channels'.
    let silence = adpcm.samples_per_block as usize * adpcm.channels as usize * 2;
    let mut pcm = Vec::new();
    for b in data().chunks(block) {
        let samples = adpcm.decode_buffer(b);
        if samples.is_empty() && b.len() == block {
            pcm.resize(pcm.len() + silence, 0);
        }
        pcm.extend(samples.iter().flat_map(|v| v.to_le_bytes()));
    }
    Some((Format::pcm(rate, 16, adpcm.channels), pcm))
}

/// A picture decoder, which goes on from where it got to. A copy is
/// empty, as one after a restore is: it starts again at the next frame
/// it is asked for.
#[derive(Default)]
pub struct Pictures {
    codec: Codec,
    /// The last frame decoded, and the picture shown, as rows from the
    /// top.
    at: Option<u32>,
    shown: Option<Vec<u8>>,
    /// An RLE8 video's picture as it was decoded last: rows of palette
    /// indexes from the bottom, which the next frame draws onto.
    indexes: Vec<u8>,
}

impl Clone for Pictures {
    fn clone(&self) -> Pictures {
        Pictures::default()
    }
}

impl std::fmt::Debug for Pictures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pictures({:?})", self.at)
    }
}

impl Pictures {
    /// The frame `n` of `video`, `width` by `height`, as rows of 24-bit
    /// pixels from the top, `stride` bytes each: the nearest one before
    /// it that decodes, for one that doesn't.
    pub fn frame(&mut self, video: &Video, size: (u32, u32), n: u32) -> Option<&[u8]> {
        let n = n.min(video.chunks.len().checked_sub(1)? as u32);
        if video.compression == BI_RGB {
            // A frame of no bytes repeats the one before.
            let c = (0..=n).rev().find(|&i| video.chunks[i as usize].size > 0)?;
            if self.at != Some(c) {
                let data = bytes(&video.data, &video.chunks[c as usize]);
                self.shown = Some(uncompressed(video, size, data)?);
                self.at = Some(c);
            }
            return self.shown.as_deref();
        }
        if video.compression == BI_RLE8 {
            return self.rle_frame(video, size, n);
        }
        // Back, or on: from the key frame before, unless the decoder goes
        // on from where it is.
        let key = key_frame(video, n);
        let from = match self.at {
            Some(at) if goes_on(at, key, n) && !self.codec.is_empty() => at + 1,
            _ => {
                self.codec = Codec::new(video.compression);
                key
            }
        };
        for i in from..=n {
            let chunk = &video.chunks[i as usize];
            if chunk.size > 0 {
                let _ = self.codec.decode(bytes(&video.data, chunk));
            }
            self.at = Some(i);
        }
        if from <= n {
            let stride = dib_stride(size.0 as i32, 24);
            self.shown = self
                .codec
                .picture(size.0, size.1)
                .map(|dib| flip(&dib, stride));
        }
        self.shown.as_deref()
    }

    /// The frame `n` of an RLE8 video, as `frame` gives it: each frame's
    /// codes drawn in turn onto the picture, from the key frame before
    /// `n`, or on from the frame decoded last.
    fn rle_frame(&mut self, video: &Video, (w, h): (u32, u32), n: u32) -> Option<&[u8]> {
        let stride = dib_stride(w as i32, 8);
        let len = stride * h as usize;
        let key = key_frame(video, n);
        let from = match self.at {
            Some(at) if goes_on(at, key, n) && self.indexes.len() == len => at + 1,
            _ => {
                self.indexes = vec![0; len];
                key
            }
        };
        for i in from..=n {
            let data = bytes(&video.data, &video.chunks[i as usize]);
            rle_onto(&mut self.indexes, data, w as usize, stride, false);
            self.at = Some(i);
        }
        if from <= n {
            self.shown = uncompressed(video, (w, h), &self.indexes);
        }
        self.shown.as_deref()
    }
}

/// The key frame at or before the frame `n`: the first frame if none is
/// marked.
fn key_frame(video: &Video, n: u32) -> u32 {
    (0..=n)
        .rev()
        .find(|&i| video.chunks[i as usize].flags & AVIIF_KEYFRAME != 0)
        .unwrap_or(0)
}

/// Whether a decoder that has decoded the frames up to `at` goes on to
/// the frame `n`, whose key frame is `key`, rather than start again at
/// `key`: when it hasn't passed `n` and has reached the frame before
/// `key`. A key frame decoded on from the frame before it gives the
/// picture it gives on its own, so playing on never starts again. A
/// file without an index has every frame marked a key frame, and
/// starting again at each would decode each frame's changes alone.
fn goes_on(at: u32, key: u32, n: u32) -> bool {
    at <= n && at + 1 >= key
}

/// The rows of a bottom-up DIB, from the top.
fn flip(dib: &[u8], stride: usize) -> Vec<u8> {
    dib.chunks_exact(stride).rev().flatten().copied().collect()
}

/// An uncompressed frame as 24-bit rows from the top.
fn uncompressed(video: &Video, (w, h): (u32, u32), data: &[u8]) -> Option<Vec<u8>> {
    let (w, h) = (w as usize, h as usize);
    let from = dib_stride(w as i32, video.bits as u32);
    let to = dib_stride(w as i32, 24);
    if data.len() < from * h {
        return None;
    }
    let mut out = vec![0; to * h];
    for y in 0..h {
        let row = if video.top_down { y } else { h - 1 - y };
        let src = &data[row * from..][..from];
        let dst = &mut out[y * to..][..w * 3];
        for (x, d) in dst.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let px: [u8; 3] = match video.bits {
                8 => video
                    .palette
                    .get(src[x] as usize)
                    .copied()
                    .unwrap_or_default(),
                16 => {
                    // 5 bits each of red, green and blue.
                    let v = u16::from_le_bytes([src[x * 2], src[x * 2 + 1]]);
                    let c = |s: u16| {
                        let c = ((v >> s) & 0x1F) as u8;
                        (c << 3) | (c >> 2)
                    };
                    [c(0), c(5), c(10)]
                }
                24 => [src[x * 3], src[x * 3 + 1], src[x * 3 + 2]],
                _ => [src[x * 4], src[x * 4 + 1], src[x * 4 + 2]],
            };
            d.copy_from_slice(&px);
        }
    }
    Some(out)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    fn chunk(out: &mut Vec<u8>, id: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(id);
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        if data.len() % 2 == 1 {
            out.push(0);
        }
    }

    fn list(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut data = kind.to_vec();
        data.extend_from_slice(body);
        let mut out = Vec::new();
        chunk(&mut out, b"LIST", &data);
        out
    }

    fn stream_header(kind: &[u8; 4], scale: u32, rate: u32, length: u32, size: u32) -> Vec<u8> {
        let mut h = vec![0; 56];
        h[..4].copy_from_slice(kind);
        h[20..24].copy_from_slice(&scale.to_le_bytes());
        h[24..28].copy_from_slice(&rate.to_le_bytes());
        h[32..36].copy_from_slice(&length.to_le_bytes());
        h[44..48].copy_from_slice(&size.to_le_bytes());
        h
    }

    /// A BITMAPINFOHEADER of `w` by `h`, `bits` a pixel, compressed with
    /// `compression`, and the colour table `colors` (blue, green, red).
    fn bitmap_info(w: u32, h: u32, bits: u16, compression: u32, colors: &[[u8; 3]]) -> Vec<u8> {
        let mut bih = vec![0; 40];
        bih[..4].copy_from_slice(&40u32.to_le_bytes());
        bih[4..8].copy_from_slice(&w.to_le_bytes());
        bih[8..12].copy_from_slice(&h.to_le_bytes());
        bih[12..14].copy_from_slice(&1u16.to_le_bytes());
        bih[14..16].copy_from_slice(&bits.to_le_bytes());
        bih[16..20].copy_from_slice(&compression.to_le_bytes());
        bih[32..36].copy_from_slice(&(colors.len() as u32).to_le_bytes());
        for c in colors {
            bih.extend_from_slice(&[c[0], c[1], c[2], 0]);
        }
        bih
    }

    /// The format of 8-bit mono PCM at 11025 Hz.
    fn pcm_format() -> Vec<u8> {
        let mut wf = vec![0; 16];
        wf[..2].copy_from_slice(&1u16.to_le_bytes());
        wf[2..4].copy_from_slice(&1u16.to_le_bytes());
        wf[4..8].copy_from_slice(&11025u32.to_le_bytes());
        wf[8..12].copy_from_slice(&11025u32.to_le_bytes());
        wf[12..14].copy_from_slice(&1u16.to_le_bytes());
        wf[14..16].copy_from_slice(&8u16.to_le_bytes());
        wf
    }

    /// An AVI file of `frames` (24-bit BGR pictures, `w` by `h`, bottom
    /// up) at 10 a second and 8-bit mono PCM at 11025 Hz.
    pub fn file(w: u32, h: u32, frames: &[Vec<u8>], pcm: &[u8]) -> Vec<u8> {
        let frames: Vec<_> = frames.iter().map(|f| (f.clone(), true)).collect();
        let bih = bitmap_info(w, h, 24, BI_RGB, &[]);
        avi_file(&bih, &frames, &pcm_format(), pcm, false)
    }

    /// An AVI file of the video format `bih` and its `frames` (each with
    /// whether it is a key frame) at 10 a second, and the sound format
    /// `wf` and its bytes `sound`. With `index`, an `idx1` index that
    /// has the frames' key frame flags.
    fn avi_file(
        bih: &[u8],
        frames: &[(Vec<u8>, bool)],
        wf: &[u8],
        sound: &[u8],
        index: bool,
    ) -> Vec<u8> {
        let mut main = vec![0; 56];
        main[24..28].copy_from_slice(&2u32.to_le_bytes());
        let mut strl_v = Vec::new();
        chunk(
            &mut strl_v,
            b"strh",
            &stream_header(b"vids", 1, 10, frames.len() as u32, 0),
        );
        chunk(&mut strl_v, b"strf", bih);
        let mut strl_a = Vec::new();
        chunk(
            &mut strl_a,
            b"strh",
            &stream_header(b"auds", 1, 11025, sound.len() as u32, 1),
        );
        chunk(&mut strl_a, b"strf", wf);
        let mut hdrl = Vec::new();
        chunk(&mut hdrl, b"avih", &main);
        hdrl.extend(list(b"strl", &strl_v));
        hdrl.extend(list(b"strl", &strl_a));
        let mut movi = Vec::new();
        let mut idx = Vec::new();
        for (f, key) in frames {
            // Where the chunk is, from the list's type.
            let at = 4 + movi.len() as u32;
            let flags = if *key { AVIIF_KEYFRAME } else { 0 };
            for v in [u32::from_le_bytes(*b"00db"), flags, at, f.len() as u32] {
                idx.extend_from_slice(&v.to_le_bytes());
            }
            chunk(&mut movi, b"00db", f);
        }
        chunk(&mut movi, b"01wb", sound);
        let mut body = b"AVI ".to_vec();
        body.extend(list(b"hdrl", &hdrl));
        body.extend(list(b"movi", &movi));
        if index {
            chunk(&mut body, b"idx1", &idx);
        }
        let mut out = Vec::new();
        chunk(&mut out, b"RIFF", &body);
        out
    }

    /// A picture of one colour, as blue, green and red.
    pub fn flat(w: u32, h: u32, px: [u8; 3]) -> Vec<u8> {
        let stride = dib_stride(w as i32, 24);
        let mut out = vec![0; stride * h as usize];
        for row in out.chunks_exact_mut(stride) {
            for d in row[..w as usize * 3].chunks_exact_mut(3) {
                d.copy_from_slice(&px);
            }
        }
        out
    }

    #[test]
    fn an_avi_file_has_its_pictures_and_its_sound() {
        let (red, blue) = ([0, 0, 255], [255, 0, 0]);
        let frames = [flat(6, 4, red), flat(6, 4, red), flat(6, 4, blue)];
        let m = parse(&file(6, 4, &frames, &[0x80; 1000])).unwrap();
        assert_eq!((m.width, m.height, m.frames, m.rate), (6, 4, 3, (10, 1)));
        assert_eq!(m.length_ms(), 300);
        let (format, pcm) = m.audio.clone().unwrap();
        assert_eq!((format.rate, format.bits, format.channels), (11025, 8, 1));
        assert_eq!(pcm.len(), 1000);
        let Kind::Avi(v) = &m.video else { panic!() };
        let mut p = Pictures::default();
        for (n, want) in [(2, blue), (0, red), (1, red)] {
            let rows = p.frame(v, (6, 4), n).unwrap();
            assert_eq!(rows[..3], want, "frame {n}");
        }
    }

    /// Show the frames `shown` of an RLE8 file, with an index or without
    /// one, and check each picture.
    fn rle8_pictures(index: bool, shown: &[u32]) {
        // Black, red, green, blue.
        let colors = [[0, 0, 0], [0, 0, 255], [0, 255, 0], [255, 0, 0]];
        let bih = bitmap_info(4, 2, 8, BI_RLE8, &colors);
        let frames = [
            // The bottom row red, a line's end, the top row green, the
            // bitmap's end.
            (vec![4, 1, 0, 0, 4, 2, 0, 1], true),
            // A delta to (1, 1), the top row, then two blue pixels: the
            // others keep their colour.
            (vec![0, 2, 1, 1, 2, 3, 0, 1], false),
            // Only the bitmap's end: the picture stays as it was.
            (vec![0, 1], false),
        ];
        let m = parse(&avi_file(&bih, &frames, &pcm_format(), &[0x80; 10], index)).unwrap();
        assert_eq!((m.width, m.height, m.frames), (4, 2, 3));
        let Kind::Avi(v) = &m.video else { panic!() };
        let px = |rows: &[u8], x: usize, y: usize| -> [u8; 3] {
            let at = y * dib_stride(4, 24) + x * 3;
            rows[at..at + 3].try_into().unwrap()
        };
        let (red, green, blue) = (colors[1], colors[2], colors[3]);
        let mut p = Pictures::default();
        for &n in shown {
            let rows = p.frame(v, (4, 2), n).unwrap();
            let top: Vec<_> = (0..4).map(|x| px(rows, x, 0)).collect();
            let bottom: Vec<_> = (0..4).map(|x| px(rows, x, 1)).collect();
            assert_eq!(bottom, [red; 4], "frame {n}");
            let want = if n == 0 {
                vec![green; 4]
            } else {
                vec![green, blue, blue, green]
            };
            assert_eq!(top, want, "frame {n}");
        }
    }

    #[test]
    fn rle8_frames_draw_onto_the_picture_before() {
        // On from frame 0, back to it, then a seek past the delta.
        rle8_pictures(true, &[0, 1, 2, 0, 2]);
    }

    #[test]
    fn rle8_frames_of_a_file_without_an_index_play_on() {
        // Every frame of such a file is marked a key frame, and playing
        // on still draws each onto the one before.
        rle8_pictures(false, &[0, 1, 2]);
    }

    #[test]
    fn ms_adpcm_sound_plays_as_16_bit_pcm() {
        // Mono MS ADPCM in blocks of 16 bytes: a 7-byte header and 9
        // bytes of codes, 2 + 18 samples.
        let mut wf = Vec::new();
        wf.extend_from_slice(&2u16.to_le_bytes());
        wf.extend_from_slice(&1u16.to_le_bytes());
        wf.extend_from_slice(&11025u32.to_le_bytes());
        wf.extend_from_slice(&11025u32.to_le_bytes());
        for v in [16u16, 4, 32, 20, 7] {
            wf.extend_from_slice(&v.to_le_bytes());
        }
        for (a, b) in adpcm::tests::MS_COEFFICIENTS {
            wf.extend_from_slice(&a.to_le_bytes());
            wf.extend_from_slice(&b.to_le_bytes());
        }
        // Predictor 0, delta 16, then the samples 1000 and 500, which
        // play in the other order, and codes of 0.
        let mut block = vec![0, 16, 0];
        block.extend_from_slice(&1000i16.to_le_bytes());
        block.extend_from_slice(&500i16.to_le_bytes());
        block.extend_from_slice(&[0; 9]);
        // A block whose predictor is past the coefficients, which the
        // codec doesn't decode, between two that it does.
        let mut bad = block.clone();
        bad[0] = 7;
        let sound = [block.clone(), bad, block].concat();
        let bih = bitmap_info(2, 2, 24, BI_RGB, &[]);
        let frame = [(flat(2, 2, [0, 0, 0]), true)];
        let m = parse(&avi_file(&bih, &frame, &wf, &sound, false)).unwrap();
        let (format, pcm) = m.audio.clone().unwrap();
        let f = (format.tag, format.rate, format.bits, format.channels);
        assert_eq!(f, (WAVE_FORMAT_PCM, 11025, 16, 1));
        // Three blocks of 20 samples, 2 bytes each, the second silent.
        assert_eq!(pcm.len(), 3 * 20 * 2);
        let sample = |i: usize| i16::from_le_bytes([pcm[i * 2], pcm[i * 2 + 1]]);
        assert_eq!((sample(0), sample(1)), (500, 1000));
        assert!((20..40).all(|i| sample(i) == 0));
        assert_eq!((sample(40), sample(41)), (500, 1000));
    }

    #[test]
    fn a_sound_format_too_short_leaves_the_pictures() {
        let bih = bitmap_info(2, 2, 24, BI_RGB, &[]);
        let frame = [(flat(2, 2, [0, 0, 0]), true)];
        let m = parse(&avi_file(&bih, &frame, &[1], &[0x80; 10], false)).unwrap();
        assert_eq!(m.frames, 1);
        assert!(m.audio.is_none());
    }

    #[test]
    fn other_files_are_refused() {
        assert!(parse(b"RIFF\x04\x00\x00\x00WAVE").is_none());
        assert!(parse(&[0, 0, 1, 0xBA]).is_none());
    }
}
