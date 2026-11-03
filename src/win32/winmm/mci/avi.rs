//! AVI files, as the AVIVideo and MPEGVideo devices (video.rs) play
//! them: the first video stream and the first audio stream of a file
//! that the Video for Windows code (`vfw::avi`) reads. Its pictures are
//! uncompressed DIBs of 8, 16, 24 or 32 bits or what a Video for Windows
//! codec decompresses (`vfw::codec`: Indeo 3 and 5), and its sound is
//! PCM. A file of other sound plays without it.
//!
//! The sound is read whole when the file opens, to be played as one
//! sound. The pictures are decoded as they are shown: a decoder goes on
//! from the frame it showed last, and a seek starts it again at the key
//! frame before the one asked for.

use super::media::{Kind, Media};
use crate::win32::dsound::device::{Format, WAVE_FORMAT_PCM};
use crate::win32::gdi32::surface::dib_stride;
use crate::win32::vfw::avi::{self, AVIIF_KEYFRAME, Chunk};
use crate::win32::vfw::codec::{self, Codec};

const VIDS: u32 = u32::from_le_bytes(*b"vids");
const AUDS: u32 = u32::from_le_bytes(*b"auds");
const BI_RGB: u32 = 0;

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
        let f = &s.format;
        if le16(f, 0)? == WAVE_FORMAT_PCM {
            let format = Format::pcm(le32(f, 4)?, le16(f, 14)?, le16(f, 2)?);
            if format.rate > 0 && matches!(format.bits, 8 | 16) && matches!(format.channels, 1 | 2)
            {
                let pcm = s.chunks.iter().flat_map(|c| bytes(d, c)).copied().collect();
                media.audio = Some((format, pcm));
            }
        }
    }
    (media.frames > 0 || media.audio.is_some()).then_some(media)
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
        // Back, or on: from the key frame before, unless the decoder is
        // past it.
        let key = (0..=n)
            .rev()
            .find(|&i| video.chunks[i as usize].flags & AVIIF_KEYFRAME != 0)
            .unwrap_or(0);
        let from = match self.at {
            Some(at) if at <= n && at >= key && !self.codec.is_empty() => at + 1,
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

    /// An AVI file of `frames` (24-bit BGR pictures, `w` by `h`, bottom
    /// up) at 10 a second and 8-bit mono PCM at 11025 Hz.
    pub fn file(w: u32, h: u32, frames: &[Vec<u8>], pcm: &[u8]) -> Vec<u8> {
        let mut bih = vec![0; 40];
        bih[..4].copy_from_slice(&40u32.to_le_bytes());
        bih[4..8].copy_from_slice(&w.to_le_bytes());
        bih[8..12].copy_from_slice(&h.to_le_bytes());
        bih[12..14].copy_from_slice(&1u16.to_le_bytes());
        bih[14..16].copy_from_slice(&24u16.to_le_bytes());
        let mut wf = vec![0; 16];
        wf[..2].copy_from_slice(&1u16.to_le_bytes());
        wf[2..4].copy_from_slice(&1u16.to_le_bytes());
        wf[4..8].copy_from_slice(&11025u32.to_le_bytes());
        wf[8..12].copy_from_slice(&11025u32.to_le_bytes());
        wf[12..14].copy_from_slice(&1u16.to_le_bytes());
        wf[14..16].copy_from_slice(&8u16.to_le_bytes());
        let mut main = vec![0; 56];
        main[24..28].copy_from_slice(&2u32.to_le_bytes());
        let mut strl_v = Vec::new();
        chunk(
            &mut strl_v,
            b"strh",
            &stream_header(b"vids", 1, 10, frames.len() as u32, 0),
        );
        chunk(&mut strl_v, b"strf", &bih);
        let mut strl_a = Vec::new();
        chunk(
            &mut strl_a,
            b"strh",
            &stream_header(b"auds", 1, 11025, pcm.len() as u32, 1),
        );
        chunk(&mut strl_a, b"strf", &wf);
        let mut hdrl = Vec::new();
        chunk(&mut hdrl, b"avih", &main);
        hdrl.extend(list(b"strl", &strl_v));
        hdrl.extend(list(b"strl", &strl_a));
        let mut movi = Vec::new();
        for f in frames {
            chunk(&mut movi, b"00db", f);
        }
        chunk(&mut movi, b"01wb", pcm);
        let mut body = b"AVI ".to_vec();
        body.extend(list(b"hdrl", &hdrl));
        body.extend(list(b"movi", &movi));
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

    #[test]
    fn other_files_are_refused() {
        assert!(parse(b"RIFF\x04\x00\x00\x00WAVE").is_none());
        assert!(parse(&[0, 0, 1, 0xBA]).is_none());
    }
}
