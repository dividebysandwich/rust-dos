//! The video decompressors Video for Windows has: Indeo Video 3 (IV31,
//! IV32) and Indeo Video 5 (IV50), decoded by the `oxideav-indeo` crate.
//! Windows 98 installs codecs for the two. A picture comes out as what
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

/// The largest picture Indeo 3's decoder takes.
const INDEO3_MAX: (u32, u32) = (640, 480);

/// The FourCC `compression` in capitals, as ICOpen compares them.
fn upper(compression: u32) -> u32 {
    u32::from_le_bytes(compression.to_le_bytes().map(|b| b.to_ascii_uppercase()))
}

/// Whether a codec decompresses `compression` at this size.
pub fn supported(compression: u32, width: u32, height: u32) -> bool {
    match upper(compression) {
        IV31 | IV32 => (1..=INDEO3_MAX.0).contains(&width) && (1..=INDEO3_MAX.1).contains(&height),
        IV50 => width >= 1 && height >= 1,
        _ => false,
    }
}

enum Decoder {
    Indeo3(Box<Indeo3PictureDecoder>),
    /// With the last frame's output and size, which a frame that fails
    /// to decode leaves as it is.
    Indeo5(Box<Indeo5Decoder>, Option<(HostBuffer, (u32, u32))>),
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
        }
    }
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
fn bgr(y: u8, u: u8, v: u8) -> [u8; 3] {
    let (u, v) = (u as i32 - 128, v as i32 - 128);
    let l = CY * (y as i32 - 16) + (1 << 15);
    let r = (l + CRV * v) >> 16;
    let g = (l - CGU * u - CGV * v) >> 16;
    let b = (l + CBU * u) >> 16;
    [b, g, r].map(|c| c.clamp(0, 255) as u8)
}

/// An Indeo 3 picture as a DIB: each chroma sample colours its 4x4
/// block.
fn indeo3_dib(picture: &DecodedPicture) -> Vec<u8> {
    let (w, h) = (picture.width as usize, picture.height as usize);
    let cw = picture.chroma_width as usize;
    let stride = dib_stride(picture.width as i32, 24);
    let mut out = vec![0; stride * h];
    for y in 0..h {
        let crow = y / 4 * cw;
        let ys = &picture.luma[y * w..y * w + w];
        let row = &mut out[(h - 1 - y) * stride..][..w * 3];
        for (x, (&yv, d)) in ys.iter().zip(row.as_chunks_mut::<3>().0).enumerate() {
            *d = bgr(
                yv,
                picture.chroma_u[crow + x / 4],
                picture.chroma_v[crow + x / 4],
            );
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
    fn codecs_go_by_fourcc_in_either_case() {
        let iv50 = u32::from_le_bytes(*b"iv50");
        assert!(supported(iv50, 640, 480));
        assert!(!supported(u32::from_le_bytes(*b"cvid"), 320, 240));
        assert!(!supported(IV32, 800, 600));
        assert!(!Codec::new(iv50).is_empty());
        assert!(Codec::new(0).is_empty());
    }
}
