//! Indeo 3 stateful picture decoder over the cell decoder (r459): the
//! per-frame pipeline that turns a sequence of `IV31` / `IV32` codec
//! frames into pixel planes, with the two-bank reference ping-pong of
//! `spec/05 §4.2` and the NULL-frame repeat of `spec/07 §6.3`.
//!
//! Per picture frame:
//!
//! 1. parse the `spec/01` header and the `spec/02` picture layer;
//! 2. rebuild the per-frame codebook arena from the `alt_quant[]`
//!    overlay (`spec/04 §6`);
//! 3. select the destination bank by `frame_flags` bit 9 (the other
//!    bank is the reference the INTER leaves fetch from);
//! 4. decode the three planes (Y, V, U) into the destination bank's
//!    strip buffers ([`super::decode_plane`]);
//! 5. emit the planes as 8-bit samples (`spec/07 §4.3` upshift),
//!    chroma at the 4:1:0 size `ceil(w / 4) × ceil(h / 4)`.
//!
//! Both staged `IV32` corpora decode pixel-exact through this path on
//! every frame (`tests/indeo3_pixels.rs`).

use super::cell_decoder::{CellDecodeError, PlaneBuffers, PlaneContext, PlaneStats, decode_plane};
use super::codebook_seed::CodebookSeedArea;
use super::frame_session::{AdmittedFrame, DecodeSession, FrameAdmission, SessionError};
use super::header::{FrameHeader, HeaderError, MAX_HEIGHT, MAX_WIDTH, MIN_DIMENSION};
use super::picture_layer::{PictureLayer, PictureLayerError, PlanePresence};
use super::staging::StagingImage;
use super::vq::{DyadDeltaTable, VqArena, VqError};

/// Errors raised while decoding one frame through an
/// [`Indeo3PictureDecoder`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PictureDecodeError {
    /// The frame's sequencing was rejected by the session.
    Session(SessionError),
    /// The frame header failed to parse.
    Header(HeaderError),
    /// The picture layer (plane preludes) failed to parse.
    PictureLayer(PictureLayerError),
    /// The `alt_quant` overlay selected a staging block outside the
    /// image.
    Overlay(VqError),
    /// A plane's cell decode failed.
    Plane {
        /// Plane index (0 = Y, 1 = V, 2 = U).
        plane_idx: usize,
        /// The underlying error.
        error: CellDecodeError,
    },
    /// The header's picture size is outside the codec's envelope
    /// (`spec/01 §3.6`: `MIN_DIMENSION..=MAX_WIDTH` × `..=MAX_HEIGHT`).
    BadDimensions {
        /// The header's width.
        width: u16,
        /// The header's height.
        height: u16,
    },
    /// The picture size changed between frames without a session reset.
    SizeChanged {
        /// The session's picture size.
        expected: (u16, u16),
        /// The frame's picture size.
        found: (u16, u16),
    },
}

impl core::fmt::Display for PictureDecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PictureDecodeError::Session(e) => write!(f, "indeo3 picture decoder: {e}"),
            PictureDecodeError::Header(e) => write!(f, "indeo3 picture decoder: {e}"),
            PictureDecodeError::PictureLayer(e) => write!(f, "indeo3 picture decoder: {e}"),
            PictureDecodeError::Overlay(e) => write!(f, "indeo3 picture decoder: {e}"),
            PictureDecodeError::Plane { plane_idx, error } => {
                write!(f, "indeo3 picture decoder: plane {plane_idx}: {error}")
            }
            PictureDecodeError::BadDimensions { width, height } => write!(
                f,
                "indeo3 picture decoder: picture size {width}x{height} outside the codec envelope"
            ),
            PictureDecodeError::SizeChanged { expected, found } => write!(
                f,
                "indeo3 picture decoder: picture size changed from {}x{} to {}x{}",
                expected.0, expected.1, found.0, found.1
            ),
        }
    }
}

impl std::error::Error for PictureDecodeError {}

impl From<SessionError> for PictureDecodeError {
    fn from(e: SessionError) -> Self {
        PictureDecodeError::Session(e)
    }
}

impl From<HeaderError> for PictureDecodeError {
    fn from(e: HeaderError) -> Self {
        PictureDecodeError::Header(e)
    }
}

impl From<PictureLayerError> for PictureDecodeError {
    fn from(e: PictureLayerError) -> Self {
        PictureDecodeError::PictureLayer(e)
    }
}

impl From<VqError> for PictureDecodeError {
    fn from(e: VqError) -> Self {
        PictureDecodeError::Overlay(e)
    }
}

/// One decoded picture: three 8-bit planes at their native sizes.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DecodedPicture {
    /// Picture width in luma samples.
    pub width: u32,
    /// Picture height in luma samples.
    pub height: u32,
    /// Chroma plane width (`ceil(width / 4)`).
    pub chroma_width: u32,
    /// Chroma plane height (`ceil(height / 4)`).
    pub chroma_height: u32,
    /// Luma samples, `width × height`.
    pub luma: Vec<u8>,
    /// V samples, `chroma_width × chroma_height` (the first coded
    /// chroma plane).
    pub chroma_v: Vec<u8>,
    /// U samples, `chroma_width × chroma_height`.
    pub chroma_u: Vec<u8>,
    /// `true` when the frame was a NULL frame and the previous picture
    /// was re-emitted (`spec/07 §6.3`).
    pub repeated_previous: bool,
    /// The session's admission of the frame.
    pub admission: AdmittedFrame,
    /// Per-plane cell statistics (`None` for a repeated frame).
    pub stats: Option<[PlaneStats; 3]>,
}

impl Clone for DecodedPicture {
    fn clone(&self) -> Self {
        DecodedPicture {
            luma: self.luma.clone(),
            chroma_v: self.chroma_v.clone(),
            chroma_u: self.chroma_u.clone(),
            ..*self
        }
    }

    /// Reuses `self`'s plane buffers, so the decoder can keep its copy
    /// of the last picture without allocating one per frame.
    fn clone_from(&mut self, source: &Self) {
        // Destructured in full so a field added later fails to compile
        // here instead of being silently left stale.
        let DecodedPicture {
            width,
            height,
            chroma_width,
            chroma_height,
            luma,
            chroma_v,
            chroma_u,
            repeated_previous,
            admission,
            stats,
        } = source;
        self.width = *width;
        self.height = *height;
        self.chroma_width = *chroma_width;
        self.chroma_height = *chroma_height;
        self.luma.clone_from(luma);
        self.chroma_v.clone_from(chroma_v);
        self.chroma_u.clone_from(chroma_u);
        self.repeated_previous = *repeated_previous;
        self.admission = *admission;
        self.stats = *stats;
    }
}

impl DecodedPicture {
    /// The picture as three full-luma-resolution planes `(Y, U, V)` —
    /// chroma box-replicated 4×4 (`spec/07 §5.5`), the `Yuv444P` shape
    /// the registry bridge emits.
    pub fn to_yuv444_planes(&self) -> [Vec<u8>; 3] {
        let up = |c: &[u8]| -> Vec<u8> {
            let mut out = Vec::with_capacity((self.width * self.height) as usize);
            for y in 0..self.height {
                let cy = (y / 4).min(self.chroma_height.saturating_sub(1));
                for x in 0..self.width {
                    let cx = (x / 4).min(self.chroma_width.saturating_sub(1));
                    out.push(c[(cy * self.chroma_width + cx) as usize]);
                }
            }
            out
        };
        [self.luma.clone(), up(&self.chroma_u), up(&self.chroma_v)]
    }
}

/// The stateful Indeo 3 decoder: codec-init tables, the per-frame
/// arena, the two reference banks and the last emitted picture.
pub struct Indeo3PictureDecoder {
    session: DecodeSession,
    staging: StagingImage,
    lut: DyadDeltaTable,
    arena: VqArena,
    size: Option<(u16, u16)>,
    /// `banks[b][plane]`, allocated on the first picture frame.
    banks: Vec<Vec<PlaneBuffers>>,
    previous: Option<DecodedPicture>,
}

impl Default for Indeo3PictureDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Indeo3PictureDecoder {
    /// A fresh decoder: codec-init tables built, no frame seen.
    pub fn new() -> Self {
        Indeo3PictureDecoder {
            session: DecodeSession::new(),
            staging: StagingImage::build(&CodebookSeedArea::load()),
            lut: DyadDeltaTable::load(),
            arena: VqArena::new(),
            size: None,
            banks: Vec::new(),
            previous: None,
        }
    }

    /// The last emitted picture, if any.
    pub fn previous(&self) -> Option<&DecodedPicture> {
        self.previous.as_ref()
    }

    /// Decode one codec frame (the bytes of one `IV31` / `IV32` access
    /// unit, `FRMH` header included).
    pub fn decode(&mut self, input: &[u8]) -> Result<DecodedPicture, PictureDecodeError> {
        let admitted = self.session.admit(input)?;
        if matches!(admitted.admission, FrameAdmission::NullRepeat) {
            let prev = self
                .previous
                .as_ref()
                .expect("the session never admits a NULL frame first");
            let mut out = prev.clone();
            out.repeated_previous = true;
            out.admission = admitted;
            out.stats = None;
            return Ok(out);
        }

        let header = FrameHeader::parse(input)?;
        let (w, h) = (header.bitstream.width, header.bitstream.height);
        if !(MIN_DIMENSION..=MAX_WIDTH).contains(&w) || !(MIN_DIMENSION..=MAX_HEIGHT).contains(&h) {
            return Err(PictureDecodeError::BadDimensions {
                width: w,
                height: h,
            });
        }
        match self.size {
            None => {
                self.size = Some((w, h));
                let mk = |p: usize| {
                    if p == 0 {
                        PlaneBuffers::luma(u32::from(w), u32::from(h))
                    } else {
                        PlaneBuffers::chroma(u32::from(w), u32::from(h))
                    }
                };
                self.banks = vec![vec![mk(0), mk(1), mk(2)], vec![mk(0), mk(1), mk(2)]];
            }
            Some(s) if s != (w, h) => {
                return Err(PictureDecodeError::SizeChanged {
                    expected: s,
                    found: (w, h),
                });
            }
            Some(_) => {}
        }
        let pl = PictureLayer::parse(&header, input)?;
        self.arena.apply_alt_quant(
            &self.staging,
            &header.bitstream.alt_quant,
            header.bitstream.cb_offset,
        )?;

        // spec/05 §4.2 — destination bank by the frame's own bit 9, the
        // other bank is the reference.
        let sel = usize::from(header.bitstream.frame_flags.buffer_selector());
        let (dst, src) = {
            let (a, b) = self.banks.split_at_mut(1);
            if sel == 0 {
                (&mut a[0], &b[0])
            } else {
                (&mut b[0], &a[0])
            }
        };
        let mut stats = [PlaneStats::default(); 3];
        for (plane_idx, plane) in pl.planes.iter().enumerate() {
            let PlanePresence::Present(prelude) = plane else {
                // An absent plane keeps the bank's previous content
                // (spec/02 §2: the decoder skips it).
                continue;
            };
            let payload = &input[prelude.bitstream_offset..];
            let ctx = PlaneContext {
                staging: &self.staging,
                arena: &self.arena,
                cb_offset: header.bitstream.cb_offset,
                lut: &self.lut,
                mvs: &prelude.motion_vectors,
                reference: Some(&src[plane_idx]),
            };
            stats[plane_idx] = decode_plane(payload, &mut dst[plane_idx], &ctx)
                .map_err(|error| PictureDecodeError::Plane { plane_idx, error })?;
        }

        let (cw, ch) = (u32::from(w).div_ceil(4), u32::from(h).div_ceil(4));
        let out = DecodedPicture {
            width: u32::from(w),
            height: u32::from(h),
            chroma_width: cw,
            chroma_height: ch,
            luma: dst[0].to_pixels(u32::from(w), u32::from(h)),
            chroma_v: dst[1].to_pixels(cw, ch),
            chroma_u: dst[2].to_pixels(cw, ch),
            repeated_previous: false,
            admission: admitted,
            stats: Some(stats),
        };
        match &mut self.previous {
            Some(prev) => prev.clone_from(&out),
            None => self.previous = Some(out.clone()),
        }
        Ok(out)
    }
}
