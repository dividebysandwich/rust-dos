//! Intel Indeo Video Interactive 5 (`IV50`) decoder.
//!
//! Indeo 5 is a subband codec, structurally distinct from the VQ-based
//! Indeo 3 ([`crate::indeo3`]). A coded frame is a bit-packed header
//! stack (picture start, GOP header on intra frames, frame header, band
//! headers) followed by per-band, per-tile macroblock headers and
//! coefficient data. Inverse Slant transforms and motion compensation
//! rebuild each band, and a four-band plane is recombined into pixels by
//! an integer synthesis filter.
//!
//! [`Iv50Decoder`] decodes one frame at a time and keeps the state that
//! spans frames. Its 8-bit planes follow the vendor driver
//! (IR50_32.DLL R.5.06.15.2.44, MMX path): intra, inter and NULL frames,
//! one-band and four-band luma, and the transparency window that limits
//! which part of the picture a frame rebuilds. Features that no known
//! stream uses and whose decoding is not established (several tiles,
//! half-size chroma, locked streams, seven-band luma, four-band chroma,
//! bitstream versions other than 2) return [`Iv50Error::Unsupported`].
//!
//! [`Indeo5RegistryDecoder`] and [`decode_video_frame`] bridge the
//! decoder to the `oxideav_core` codec registry.

mod decoder;
mod registry;

pub use decoder::{FrameType, Iv50Decoder, Iv50Error, Iv50Plane, MAX_PICTURE_PIXELS};
pub use registry::{
    CODEC_ID_STR, INDEO5_FOURCCS, Indeo5RegistryDecoder, PROBE_CONFIDENCE_HEADER_OK,
    PROBE_CONFIDENCE_TAG_ONLY, codec_id_for_fourcc, decode_video_frame, make_decoder, probe,
    register, register_codecs,
};
