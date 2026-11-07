//! The video decompressors Video for Windows has: Indeo Video 3 (IV31,
//! IV32) and Indeo Video 5 (IV50), decoded by the `oxideav-indeo` crate,
//! Indeo Video Interactive 4 (IV41, indeo4/), and Cinepak (CVID,
//! `iccvid.dll`, cinepak.rs). Windows 98 installs codecs for the four. A picture comes out as what
//! ICDecompress gives for a BI_RGB output of 24 bits: a bottom-up DIB.
//!
//! A save state doesn't keep a decoder's picture: a copy of a decoder
//! is empty, as one after a restore is, and its owner starts it again.

use crate::win32::gdi32::surface::dib_stride;
use oxideav_indeo::indeo3::{DecodedPicture, FrameHeader, Indeo3PictureDecoder};
use oxideav_indeo::indeo5::{HostBuffer, Indeo5Decoder, pack_yuy2};

const IV31: u32 = u32::from_le_bytes(*b"IV31");
const IV32: u32 = u32::from_le_bytes(*b"IV32");
const IV50: u32 = u32::from_le_bytes(*b"IV50");
const IV41: u32 = u32::from_le_bytes(*b"IV41");
const CVID: u32 = u32::from_le_bytes(*b"CVID");

/// The largest picture Indeo 3's decoder takes.
const INDEO3_MAX: (u32, u32) = (640, 480);

/// The widest and tallest Cinepak picture decoded. A frame's header gives
/// its size in 16 bits, and the decoder makes a picture of that size, so
/// a damaged header could ask for gigabytes.
const CINEPAK_MAX: u32 = 4096;

/// The FourCC `compression` in capitals, as ICOpen compares them.
fn upper(compression: u32) -> u32 {
    u32::from_le_bytes(compression.to_le_bytes().map(|b| b.to_ascii_uppercase()))
}

/// Whether a codec decompresses `compression` at this size.
pub fn supported(compression: u32, width: u32, height: u32) -> bool {
    match upper(compression) {
        IV31 | IV32 => (1..=INDEO3_MAX.0).contains(&width) && (1..=INDEO3_MAX.1).contains(&height),
        IV50 => width >= 1 && height >= 1,
        IV41 => (1..=4096).contains(&width) && (1..=4096).contains(&height),
        CVID => (1..=CINEPAK_MAX).contains(&width) && (1..=CINEPAK_MAX).contains(&height),
        _ => false,
    }
}

enum Decoder {
    Indeo3(Box<Indeo3PictureDecoder>),
    /// With the last frame's output and size, which a frame that fails
    /// to decode leaves as it is.
    Indeo5(Box<Indeo5Decoder>, Option<(HostBuffer, (u32, u32))>),
    Indeo4(Box<super::indeo4::Decoder>),
    Cinepak(Box<super::cinepak::Decoder>),
}

/// A decompressor, or none yet.
#[derive(Default)]
pub struct Codec(Option<Decoder>);

impl Clone for Codec {
    fn clone(&self) -> Codec {
        Codec(None)
    }
}

impl std::fmt::Debug for Codec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.0 {
            Some(Decoder::Indeo3(_)) => "Codec(Indeo 3)",
            Some(Decoder::Indeo5(..)) => "Codec(Indeo 5)",
            Some(Decoder::Indeo4(_)) => "Codec(Indeo 4)",
            Some(Decoder::Cinepak(_)) => "Codec(Cinepak)",
            None => "Codec(None)",
        })
    }
}

impl Codec {
    /// A new decoder of `compression`, or an empty Codec without one.
    pub fn new(compression: u32) -> Codec {
        Codec(match upper(compression) {
            IV31 | IV32 => Some(Decoder::Indeo3(Box::default())),
            IV50 => Some(Decoder::Indeo5(Box::default(), None)),
            IV41 => Some(Decoder::Indeo4(Box::new(super::indeo4::Decoder::new()))),
            CVID => Some(Decoder::Cinepak(Box::default())),
            _ => None,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    /// Decompress one frame. After an error the picture stays the one
    /// before.
    pub fn decode(&mut self, frame: &[u8]) -> Result<(), String> {
        match &mut self.0 {
            Some(Decoder::Indeo3(d)) => {
                // The crate takes a NULL frame as a repeat once a frame
                // has passed its header checks, and panics when that
                // frame then failed and there is no picture to repeat.
                if d.previous().is_none()
                    && FrameHeader::parse(frame).is_ok_and(|h| h.bitstream.is_null_frame())
                {
                    return Err("a NULL frame with no picture before it".to_string());
                }
                d.decode(frame).map(|_| ()).map_err(|e| format!("{e:?}"))
            }
            Some(Decoder::Indeo5(d, last)) => {
                let out = d.decode(frame).map_err(|e| e.to_string())?;
                *last = Some((out.output, out.dimensions));
                Ok(())
            }
            Some(Decoder::Indeo4(d)) => d.decode(frame),
            Some(Decoder::Cinepak(d)) => d.decode(frame, CINEPAK_MAX as usize),
            None => Err("no decoder".to_string()),
        }
    }

    /// The last picture as a bottom-up 24-bit DIB, if it is `width` by
    /// `height`.
    pub fn picture(&self, width: u32, height: u32) -> Option<Vec<u8>> {
        match self.0.as_ref()? {
            Decoder::Indeo3(d) => {
                let picture = d.previous()?;
                (picture.width == width && picture.height == height).then(|| indeo3_dib(picture))
            }
            Decoder::Indeo5(_, last) => {
                let (output, size) = last.as_ref()?;
                if *size != (width, height) {
                    return None;
                }
                let yuy2 = pack_yuy2(output, width as usize, height as usize)?;
                Some(yuy2_dib(&yuy2, width as usize, height as usize))
            }
            Decoder::Indeo4(d) => {
                let p = d.picture()?;
                (p.width == width as usize && p.height == height as usize).then(|| {
                    yvu9_dib(&p.y, &p.u, &p.v, (p.width, p.height), p.chroma_width())
                })
            }
            Decoder::Cinepak(d) => {
                // A frame is coded in blocks of 4 by 4 pixels, so a movie
                // whose size isn't a multiple of 4 is coded larger. The
                // picture is the top left of it, as FFmpeg's decoder
                // gives, which goes by the stream's size too.
                let (w, h) = d.size();
                (w >= width as usize && h >= height as usize).then(|| cinepak_dib(d, width, height))
            }
        }
    }
}

/// The top left `width` by `height` of a Cinepak picture, as a DIB.
fn cinepak_dib(d: &super::cinepak::Decoder, width: u32, height: u32) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let stride = dib_stride(width as i32, 24);
    let mut out = vec![0; stride * h];
    for y in 0..h {
        let row = out[(h - 1 - y) * stride..][..w * 3].as_chunks_mut::<3>().0;
        for (o, &[r, g, b]) in row.iter_mut().zip(d.row(y)) {
            *o = [b, g, r];
        }
    }
    out
}

/// The bytes of the rows of a DIB, which a file or a program can size so
/// that they don't fit 32 bits: None then.
pub fn dib_size(bits: u16, width: u32, height: u32) -> Option<u32> {
    let stride = dib_stride(width.try_into().ok()?, bits as u32) as u64;
    stride.checked_mul(height as u64)?.try_into().ok()
}

/// BT.601 limited range to RGB, in 16.16 fixed point.
const CY: i32 = 76309; // 255 / 219
const CRV: i32 = 104597; // 1.402 * 255 / 224
const CGU: i32 = 25675; // 0.344136 * 255 / 224
const CGV: i32 = 53279; // 0.714136 * 255 / 224
const CBU: i32 = 132201; // 1.772 * 255 / 224

/// One pixel in a DIB's order: blue, green, red.
#[inline]
pub fn bgr(y: u8, u: u8, v: u8) -> [u8; 3] {
    let (u, v) = (u as i32 - 128, v as i32 - 128);
    let l = CY * (y as i32 - 16) + (1 << 15);
    let r = (l + CRV * v) >> 16;
    let g = (l - CGU * u - CGV * v) >> 16;
    let b = (l + CBU * u) >> 16;
    [b, g, r].map(|c| c.clamp(0, 255) as u8)
}

/// An Indeo 3 picture as a DIB.
fn indeo3_dib(picture: &DecodedPicture) -> Vec<u8> {
    yvu9_dib(
        &picture.luma,
        &picture.chroma_u,
        &picture.chroma_v,
        (picture.width as usize, picture.height as usize),
        picture.chroma_width as usize,
    )
}

/// A YVU9 picture (Indeo 3's and 4's) as a DIB: each chroma sample, of
/// rows `cw` wide, colours its 4x4 block.
fn yvu9_dib(luma: &[u8], u: &[u8], v: &[u8], (w, h): (usize, usize), cw: usize) -> Vec<u8> {
    let stride = dib_stride(w as i32, 24);
    let mut out = vec![0; stride * h];
    for y in 0..h {
        let crow = y / 4 * cw;
        let ys = &luma[y * w..y * w + w];
        let row = &mut out[(h - 1 - y) * stride..][..w * 3];
        for (x, (&yv, d)) in ys.iter().zip(row.as_chunks_mut::<3>().0).enumerate() {
            *d = bgr(yv, u[crow + x / 4], v[crow + x / 4]);
        }
    }
    out
}

/// A picture in YUY2, whose chroma is what Indeo 5's codec gives that
/// format, as a DIB. `width` is even, as YUY2's are.
fn yuy2_dib(yuy2: &[u8], width: usize, height: usize) -> Vec<u8> {
    let stride = dib_stride(width as i32, 24);
    let mut out = vec![0; stride * height];
    for (y, line) in yuy2.chunks_exact(width * 2).enumerate() {
        let row = &mut out[(height - 1 - y) * stride..][..width * 3];
        for (&[y0, u, y1, v], d) in line
            .as_chunks::<4>()
            .0
            .iter()
            .zip(row.as_chunks_mut::<6>().0)
        {
            d[..3].copy_from_slice(&bgr(y0, u, v));
            d[3..].copy_from_slice(&bgr(y1, u, v));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yuy2_rows_go_bottom_up_with_padding() {
        // Two rows of 2 pixels: white, then black. Black chroma is 128.
        let yuy2 = [235, 128, 235, 128, 16, 128, 16, 128];
        let dib = yuy2_dib(&yuy2, 2, 2);
        assert_eq!(
            dib,
            [0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 255, 255, 0, 0]
        );
    }

    #[test]
    fn dib_sizes_pad_rows_and_stop_at_32_bits() {
        assert_eq!(dib_size(24, 2, 3), Some(24));
        assert_eq!(dib_size(16, 3, 2), Some(16));
        assert_eq!(dib_size(32, 640, 480), Some(640 * 480 * 4));
        assert_eq!(dib_size(24, 0x10000, 0x10000), None);
        assert_eq!(dib_size(24, u32::MAX, 1), None);
    }

    /// The headers of an Indeo 3 frame of 64 by 64 with no plane data.
    fn indeo3_frame(flags: u16, data_bits: u32) -> Vec<u8> {
        use oxideav_indeo::indeo3::{
            COMBINED_HEADER_LEN, FRAME_HEADER_LEN, MAGIC_FRMH, REQUIRED_DEC_VERSION,
        };
        let mut frame = vec![0; COMBINED_HEADER_LEN];
        let size = COMBINED_HEADER_LEN as u32;
        frame[8..12].copy_from_slice(&(size ^ MAGIC_FRMH).to_le_bytes());
        frame[12..16].copy_from_slice(&size.to_le_bytes());
        let b = &mut frame[FRAME_HEADER_LEN..];
        b[0..2].copy_from_slice(&REQUIRED_DEC_VERSION.to_le_bytes());
        b[2..4].copy_from_slice(&flags.to_le_bytes());
        b[4..8].copy_from_slice(&data_bits.to_le_bytes());
        b[0x0C..0x0E].copy_from_slice(&64u16.to_le_bytes());
        b[0x0E..0x10].copy_from_slice(&64u16.to_le_bytes());
        // Each plane within the bits the header says there are, and past
        // the end of the frame's bytes.
        for plane in 0..3 {
            b[0x10 + 4 * plane..0x14 + 4 * plane].copy_from_slice(&0x200u32.to_le_bytes());
        }
        frame
    }

    #[test]
    fn a_null_frame_after_a_failed_first_frame_is_refused() {
        const INTRA: u16 = 4;
        let mut codec = Codec::new(IV32);
        assert!(codec.decode(&indeo3_frame(INTRA, 4096)).is_err());
        let null = oxideav_indeo::indeo3::NULL_FRAME_DATA_SIZE_BITS;
        assert!(codec.decode(&indeo3_frame(0, null)).is_err());
        assert!(codec.picture(64, 64).is_none());
    }

    #[test]
    fn cinepak_pictures_come_out_as_bottom_up_dibs() {
        // A key frame of 8 by 8 pixels, four macroblocks, coded by the
        // crate's encoder: the top four rows red, the bottom four blue.
        let mut rgb = Vec::new();
        for y in 0..8 {
            for _ in 0..8 {
                rgb.extend_from_slice(if y < 4 { &[255, 0, 0] } else { &[0, 0, 255] });
            }
        }
        let frame = oxideav_cinepak::encode_rgb24(&rgb, 8, 8, Default::default()).unwrap();
        let mut codec = Codec::new(u32::from_le_bytes(*b"cvid"));
        codec.decode(&frame).unwrap();
        assert!(codec.picture(16, 8).is_none());
        assert!(codec.picture(8, 12).is_none());
        let dib = codec.picture(8, 8).unwrap();
        let stride = dib_stride(8, 24);
        // The DIB's first row is the picture's bottom one: blue, as blue,
        // green and red.
        let px = |row: usize| -> [u8; 3] { dib[row * stride..][..3].try_into().unwrap() };
        let (bottom, top) = (px(0), px(7));
        assert!(bottom[0] > 200 && bottom[2] < 60, "bottom {bottom:?}");
        assert!(top[2] > 200 && top[0] < 60, "top {top:?}");
    }

    /// An 8 by 8 picture, `top` above `bottom`, as rows of `n` bytes a
    /// pixel.
    fn halves(n: usize, top: &[u8], bottom: &[u8]) -> Vec<u8> {
        (0..64)
            .flat_map(|i| if i < 32 { &top[..n] } else { &bottom[..n] })
            .copied()
            .collect()
    }

    #[test]
    fn a_cinepak_movie_of_a_size_not_a_multiple_of_4_is_the_top_left() {
        let rgb = halves(3, &[255, 0, 0], &[0, 0, 255]);
        let frame = oxideav_cinepak::encode_rgb24(&rgb, 8, 8, Default::default()).unwrap();
        let mut codec = Codec::new(CVID);
        codec.decode(&frame).unwrap();
        // A 6 by 6 movie is coded as 8 by 8: its rows 4 and 5 are blue.
        let dib = codec.picture(6, 6).unwrap();
        let stride = dib_stride(6, 24);
        assert_eq!(dib.len(), stride * 6);
        let px = |row: usize| -> [u8; 3] { dib[row * stride + 15..][..3].try_into().unwrap() };
        let (bottom, top) = (px(0), px(5));
        assert!(bottom[0] > 200 && bottom[2] < 60, "bottom {bottom:?}");
        assert!(top[2] > 200 && top[0] < 60, "top {top:?}");
    }

    #[test]
    fn grey_cinepak_pictures_come_out_grey() {
        let grey = halves(1, &[230], &[20]);
        let frame = oxideav_cinepak::encode_gray8(&grey, 8, 8, Default::default()).unwrap();
        let mut codec = Codec::new(CVID);
        codec.decode(&frame).unwrap();
        let dib = codec.picture(8, 8).unwrap();
        let stride = dib_stride(8, 24);
        let (bottom, top) = (&dib[..3], &dib[7 * stride..][..3]);
        assert!(
            bottom.iter().all(|&c| c == bottom[0] && c < 60),
            "bottom {bottom:?}"
        );
        assert!(top.iter().all(|&c| c == top[0] && c > 200), "top {top:?}");
    }

    #[test]
    fn a_cinepak_frame_too_large_is_refused_before_it_is_decoded() {
        let rgb = halves(3, &[255, 0, 0], &[0, 0, 255]);
        let frame = oxideav_cinepak::encode_rgb24(&rgb, 8, 8, Default::default()).unwrap();
        let mut codec = Codec::new(CVID);
        codec.decode(&frame).unwrap();
        // 65532 by 65532, which would be 12 GB of pixels.
        let huge = [0, 0, 0, 10, 0xFF, 0xFC, 0xFF, 0xFC, 0, 1];
        assert!(codec.decode(&huge).is_err());
        assert!(codec.picture(8, 8).is_some());
        assert!(!supported(CVID, 8192, 8));
    }

    #[test]
    fn a_cinepak_frame_cut_short_leaves_the_decoder_as_it_was() {
        let rgb = halves(3, &[255, 0, 0], &[0, 0, 255]);
        let key = oxideav_cinepak::encode_rgb24(&rgb, 8, 8, Default::default()).unwrap();
        // A frame that keeps the codebooks: one inter strip with no
        // codebook chunk, whose four blocks are the V1 codebook's entry 0.
        #[rustfmt::skip]
        let next = [
            1, 0, 0, 30, 0, 8, 0, 8, 0, 1,
            0x11, 0x00, 0, 20, 0, 0, 0, 0, 0, 8, 0, 8,
            0x32, 0x00, 0, 8, 0, 0, 0, 0,
        ];
        let mut reference = Codec::new(CVID);
        reference.decode(&key).unwrap();
        reference.decode(&next).unwrap();
        let mut codec = Codec::new(CVID);
        codec.decode(&key).unwrap();
        // A header of one strip, and no strip.
        assert!(codec.decode(&[0, 0, 0, 10, 0, 8, 0, 8, 0, 1]).is_err());
        codec.decode(&next).unwrap();
        let picture = reference.picture(8, 8).unwrap();
        // The entry the key frame gave isn't an empty one's black.
        assert!(picture.iter().any(|&c| c > 100), "{picture:?}");
        assert_eq!(codec.picture(8, 8).unwrap(), picture);
    }

    #[test]
    fn codecs_go_by_fourcc_in_either_case() {
        let iv50 = u32::from_le_bytes(*b"iv50");
        assert!(supported(iv50, 640, 480));
        assert!(supported(u32::from_le_bytes(*b"cvid"), 320, 240));
        assert!(!supported(u32::from_le_bytes(*b"MSVC"), 320, 240));
        assert!(!supported(IV32, 800, 600));
        assert!(!Codec::new(iv50).is_empty());
        assert!(Codec::new(0).is_empty());
    }
}
