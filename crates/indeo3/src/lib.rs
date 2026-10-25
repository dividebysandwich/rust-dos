//! A decoder for Intel Indeo Video 3, the FOURCCs IV31 and IV32 of Video
//! for Windows files, for the built-in Windows' codec.
//!
//! The decoder is ported from FFmpeg's libavcodec/indeo3.c and
//! indeo3data.h (LGPL 2.1 or later), as of FFmpeg commit db8beb39c034 of
//! 2026-09-23, the last to change indeo3.c. Its YUV planes match what
//! FFmpeg decodes. The files taken from FFmpeg, decode.rs and tables.rs,
//! carry its copyright notice and license.
//!
//! `Decoder::decode` takes one compressed frame, as an AVI's `00dc` chunk
//! holds it. Every value in the frame is checked: a frame that doesn't
//! decode gives an error, and nothing in it makes the decoder panic or
//! read outside the frame. A frame that fails can leave its picture
//! half-decoded in the buffer the next frames predict from, as FFmpeg's
//! does; the picture shown stays the last one decoded until a frame
//! succeeds.
//!
//! The picture comes out as the codec decodes it, YUV 4:1:0 (`yuv410`),
//! or converted to RGB (`write_picture`) with BT.601 limited range
//! coefficients, each chroma sample taken for its 4x4 block of pixels.

#![forbid(unsafe_code)]

mod decode;
mod tables;
#[cfg(test)]
mod tests;

use decode::Plane;

/// Why a frame couldn't be decoded, or a picture written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The frame's data is not valid Indeo 3.
    Invalid(&'static str),
    /// The frame uses a feature the decoder doesn't have, as FFmpeg's
    /// doesn't: 8-bit pixels and half-pixel motion vectors.
    Unsupported(&'static str),
    /// The picture size is outside 16x16 to 640x480 or not a multiple of
    /// 4, or the buffer given for a picture is too small.
    Size,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Invalid(what) => write!(f, "invalid Indeo 3 frame: {what}"),
            Error::Unsupported(what) => write!(f, "unsupported Indeo 3 feature: {what}"),
            Error::Size => f.write_str("picture size out of range"),
        }
    }
}

impl std::error::Error for Error {}

/// The pixel formats `write_picture` writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    /// 3 bytes per pixel: blue, green, red.
    Bgr24,
    /// 16 bits per pixel, little-endian, 5 bits each of red, green and
    /// blue, red highest.
    Rgb555,
    /// 16 bits per pixel, little-endian, 5 bits of red, 6 of green and 5
    /// of blue, red highest.
    Rgb565,
}

impl PixelFormat {
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            PixelFormat::Bgr24 => 3,
            PixelFormat::Rgb555 | PixelFormat::Rgb565 => 2,
        }
    }

    /// The distance between rows of a DIB `width` pixels wide: rows are
    /// padded to 4 bytes.
    pub fn dib_stride(self, width: u32) -> usize {
        (width as usize * self.bytes_per_pixel()).next_multiple_of(4)
    }
}

/// Decodes the frames of one stream, in order.
pub struct Decoder {
    width: usize,
    height: usize,
    /// Y, U, V.
    planes: [Plane; 3],
    /// The buffer that holds the last picture decoded.
    shown: usize,
}

impl Decoder {
    /// A decoder for pictures `width` by `height`, which must be
    /// multiples of 4, 16 to 640 by 16 to 480, as the codec allows. The
    /// picture starts out with every value 0.
    pub fn new(width: u32, height: u32) -> Result<Decoder, Error> {
        if !(16..=640).contains(&width)
            || !(16..=480).contains(&height)
            || !width.is_multiple_of(4)
            || !height.is_multiple_of(4)
        {
            return Err(Error::Size);
        }
        let (w, h) = (width as usize, height as usize);
        let (cw, ch) = ((w >> 2).next_multiple_of(4), (h >> 2).next_multiple_of(4));
        Ok(Decoder {
            width: w,
            height: h,
            planes: [Plane::new(w, h), Plane::new(cw, ch), Plane::new(cw, ch)],
            shown: 0,
        })
    }

    pub fn width(&self) -> u32 {
        self.width as u32
    }

    pub fn height(&self) -> u32 {
        self.height as u32
    }

    /// Decodes one frame. An empty frame, as AVI files have for a frame
    /// that repeats the one before, and a sync frame, which has only
    /// headers, leave the picture as it is. A frame of another size than
    /// the decoder's is an error.
    pub fn decode(&mut self, frame: &[u8]) -> Result<(), Error> {
        if frame.is_empty() {
            return Ok(());
        }
        let Some(info) = decode::parse_header(frame, self.width, self.height)? else {
            return Ok(());
        };
        let sel = info.buf_sel;
        for (i, strip_width) in [(0, 40), (1, 10), (2, 10)] {
            decode::decode_plane(frame, &mut self.planes[i], sel, &info, i, strip_width)?;
        }
        self.shown = sel;
        Ok(())
    }

    /// The picture's Y, U and V planes as FFmpeg's YUV410P, rows top
    /// down: Y `width` by `height`, U and V each a quarter of that across
    /// and down.
    pub fn yuv410(&self) -> [Vec<u8>; 3] {
        let (w, h) = (self.width, self.height);
        let sizes = [(w, h), (w / 4, h / 4), (w / 4, h / 4)];
        std::array::from_fn(|i| {
            let (pw, ph) = sizes[i];
            let plane = &self.planes[i];
            let src = plane.pixels(self.shown);
            let mut out = Vec::with_capacity(pw * ph);
            for row in src.chunks(plane.pitch).take(ph) {
                out.extend(row[..pw].iter().map(|&p| (p & 0x7F) << 1));
            }
            out
        })
    }

    /// Writes the picture to `out` in `format`, rows `stride` bytes
    /// apart, top down or, as a DIB has them, bottom up.
    pub fn write_picture(
        &self,
        format: PixelFormat,
        bottom_up: bool,
        stride: usize,
        out: &mut [u8],
    ) -> Result<(), Error> {
        match format {
            PixelFormat::Bgr24 => self.convert::<3>(bottom_up, stride, out, |d, r, g, b| {
                *d = [b, g, r];
            }),
            PixelFormat::Rgb555 => self.convert::<2>(bottom_up, stride, out, |d, r, g, b| {
                let v = (r as u16 >> 3) << 10 | (g as u16 >> 3) << 5 | b as u16 >> 3;
                *d = v.to_le_bytes();
            }),
            PixelFormat::Rgb565 => self.convert::<2>(bottom_up, stride, out, |d, r, g, b| {
                let v = (r as u16 >> 3) << 11 | (g as u16 >> 2) << 5 | b as u16 >> 3;
                *d = v.to_le_bytes();
            }),
        }
    }

    /// The picture as a 24-bit bottom-up DIB, what ICDecompress gives for
    /// a BI_RGB output of 24 bits.
    pub fn bgr24_dib(&self) -> Vec<u8> {
        let stride = PixelFormat::Bgr24.dib_stride(self.width as u32);
        let mut out = vec![0; stride * self.height];
        // The buffer is the size the picture needs.
        let _ = self.write_picture(PixelFormat::Bgr24, true, stride, &mut out);
        out
    }

    fn convert<const BPP: usize>(
        &self,
        bottom_up: bool,
        stride: usize,
        out: &mut [u8],
        store: impl Fn(&mut [u8; BPP], u8, u8, u8),
    ) -> Result<(), Error> {
        let (w, h) = (self.width, self.height);
        if stride < w * BPP || out.len() < stride * (h - 1) + w * BPP {
            return Err(Error::Size);
        }
        let [yp, up, vp] = &self.planes;
        let (ys, us, vs) = (
            yp.pixels(self.shown),
            up.pixels(self.shown),
            vp.pixels(self.shown),
        );
        let cw = w / 4;
        // Each chroma sample's terms for red, green and blue.
        let mut chroma = vec![(0i32, 0i32, 0i32); cw];
        for y in 0..h {
            if y % 4 == 0 {
                let crow = y / 4 * up.pitch;
                for (cx, c) in chroma.iter_mut().enumerate() {
                    let u = ((us[crow + cx] & 0x7F) as i32) * 2 - 128;
                    let v = ((vs[crow + cx] & 0x7F) as i32) * 2 - 128;
                    *c = (CRV * v, -CGU * u - CGV * v, CBU * u);
                }
            }
            let src = &ys[y * yp.pitch..y * yp.pitch + w];
            let oy = if bottom_up { h - 1 - y } else { y };
            let dst = &mut out[oy * stride..oy * stride + w * BPP];
            let pixels = dst.as_chunks_mut::<BPP>().0.as_chunks_mut::<4>().0;
            for ((ys4, ds4), &(cr, cg, cb)) in
                src.as_chunks::<4>().0.iter().zip(pixels).zip(&chroma)
            {
                for (&yv, d) in ys4.iter().zip(ds4) {
                    let l = CY * (((yv & 0x7F) as i32) * 2 - 16) + (1 << 15);
                    let r = ((l + cr) >> 16).clamp(0, 255) as u8;
                    let g = ((l + cg) >> 16).clamp(0, 255) as u8;
                    let b = ((l + cb) >> 16).clamp(0, 255) as u8;
                    store(d, r, g, b);
                }
            }
        }
        Ok(())
    }
}

// BT.601 limited range YUV to RGB, in 16.16 fixed point.
const CY: i32 = 76309; // 255 / 219
const CRV: i32 = 104597; // 1.402 * 255 / 224
const CGU: i32 = 25675; // 0.344136 * 255 / 224
const CGV: i32 = 53279; // 0.714136 * 255 / 224
const CBU: i32 = 132201; // 1.772 * 255 / 224
