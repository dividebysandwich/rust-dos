//! Indeo 5 (`IV50`) codec-registry integration: the [`oxideav_core`]
//! [`Decoder`] trait wrapper around the in-crate [`Indeo5Decoder`], plus
//! the FourCC routing surface a generic container (`oxideav-avi`,
//! `oxideav-mov`) needs to resolve an `IV50` video track to this crate.
//!
//! Spec source for the FourCC / output format: the `IV50` handler and
//! the `spec/08` planar output layout under
//! `docs/video/indeo/indeo5/spec/`.
//!
//! ## What this module adds
//!
//! Every other `indeo5` module is reachable only through the crate's own
//! typed API ([`Indeo5Decoder::decode`] → [`SessionOutput`]). This module
//! bridges that API to the framework's published codec surface so a
//! pipeline that resolves codecs through an
//! [`oxideav_core::CodecRegistry`] — the way the container crates do —
//! can construct and drive an Indeo 5 decoder without naming this crate's
//! concrete types:
//!
//! * [`codec_id_for_fourcc`] maps an on-wire FourCC (`IV50`,
//!   case-insensitive) to the [`CodecId`] this crate registers.
//! * [`Indeo5RegistryDecoder`] implements [`Decoder`]: it owns an
//!   [`Indeo5Decoder`] (so NULL / repeat frames re-emit the previous
//!   output, `spec/08 §6.4`), feeds each [`Packet`]'s bytes through
//!   [`Indeo5Decoder::decode`], and shapes the [`SessionOutput`]'s
//!   planar [`HostBuffer`](super::HostBuffer) into an
//!   [`oxideav_core::VideoFrame`].
//! * [`make_decoder`] is the [`oxideav_core::registry::codec::DecoderFactory`]
//!   the registry calls; [`register_codecs`] / [`register`] install the
//!   codec (id + caps + factory + probe + FourCC tag) into a
//!   [`CodecRegistry`] / [`RuntimeContext`].
//!
//! ## Output pixel format
//!
//! Indeo 5's dominant output is 4:1:0 (`YVU9`): luma at full resolution,
//! chroma subsampled 4×4 (the `Yv12` / `I420` streams are 4:2:0). The
//! framework carries no 4:1:0 pixel format, so — exactly as the sibling
//! Indeo 3 bridge does — this module box-upsamples both chroma planes to
//! full luma resolution via the spec's own top-left-cosited box filter
//! ([`upsample_chroma`], `spec/08 §3.5`/`§5.2`) and emits
//! [`PixelFormat::Yuv444P`] (three equal-size 8-bit planes, Y/U/V order).
//! No chroma detail is fabricated beyond that box filter.
//!
//! ## Scope
//!
//! This is a thin bridge — it adds no new decode behaviour. It emits
//! exactly what [`Indeo5Decoder`] reconstructs: the structurally-decoded
//! frame, whose coefficient *pixel* synthesis stays at the `spec/06`
//! fused-transform / dequant docs-gap (uniform mid-grey where the
//! transform is not yet staged), reshaped into the framework's
//! `VideoFrame`.

use oxideav_core::{
    CodecCapabilities, CodecId, CodecInfo, CodecParameters, CodecRegistry, CodecTag, Decoder,
    Error, Frame, Packet, PixelFormat, ProbeContext, Result, RuntimeContext, VideoFrame,
    VideoPlane,
};

use super::chroma::upsample_chroma;
use super::output::OutputPlane;
use super::picture::PictureHeader;
use super::planes::PlaneRole;
use super::session::{Indeo5Decoder, SessionError, SessionOutput};

/// The public codec id this crate registers for Indeo 5 (`"indeo5"`).
pub const CODEC_ID_STR: &str = "indeo5";

/// The in-scope Indeo 5 `VisualSampleEntry` / `fccHandler` FourCC:
/// `IV50` (Indeo Video Interactive 5). Canonical upper-case spelling.
pub const INDEO5_FOURCCS: [&[u8; 4]; 1] = [b"IV50"];

/// Returns `Some(CodecId::new("indeo5"))` if `fourcc` (case-insensitive)
/// is the in-scope Indeo 5 FourCC (`IV50`).
///
/// A demuxer's `CodecResolver` calls this (or, equivalently, the
/// registry's tag path seeded by [`register_codecs`]) to route an `IV50`
/// video track to this crate.
pub fn codec_id_for_fourcc(fourcc: &[u8; 4]) -> Option<CodecId> {
    let mut upper = [0u8; 4];
    for i in 0..4 {
        upper[i] = fourcc[i].to_ascii_uppercase();
    }
    match &upper {
        b"IV50" => Some(CodecId::new(CODEC_ID_STR)),
        _ => None,
    }
}

/// The [`oxideav_core::registry::codec::DecoderFactory`] for Indeo 5:
/// construct a fresh [`Indeo5RegistryDecoder`] from the stream
/// parameters.
///
/// The parameters' `width` / `height` / `pixel_format` are advisory —
/// the Indeo 5 bitstream carries its own dimensions in every INTRA GOP
/// header (`spec/02 §1.6`), so the decoder is self-describing.
pub fn make_decoder(params: &CodecParameters) -> Result<Box<dyn Decoder>> {
    Ok(Box::new(Indeo5RegistryDecoder::new(
        params.codec_id.clone(),
    )))
}

/// An [`oxideav_core`] [`Decoder`] backed by the in-crate stateful
/// [`Indeo5Decoder`].
///
/// Holds the multi-frame session (NULL / repeat frames re-emit the
/// previous output per `spec/08 §6.4`; INTER frames predict against the
/// held reference) and a single-packet pending slot. `send_packet`
/// stashes the packet; `receive_frame` decodes it and shapes the
/// [`SessionOutput`] into a [`PixelFormat::Yuv444P`] [`VideoFrame`].
pub struct Indeo5RegistryDecoder {
    codec_id: CodecId,
    inner: Indeo5Decoder,
    pending: Option<Packet>,
    eof: bool,
}

impl Indeo5RegistryDecoder {
    /// Create a fresh registry decoder for the given codec id.
    pub fn new(codec_id: CodecId) -> Self {
        Indeo5RegistryDecoder {
            codec_id,
            inner: Indeo5Decoder::new(),
            pending: None,
            eof: false,
        }
    }
}

/// Map an Indeo 5 [`SessionError`] into the framework [`Error`] surface.
///
/// A session-level rejection (first-frame must be INTRA) and any
/// structural / reconstruction fault both mean "this packet's bytes are
/// not a decodable Indeo 5 frame in this stream position", so both
/// surface as [`Error::invalid`].
fn map_session_error(e: SessionError) -> Error {
    Error::invalid(format!("indeo5: {e}"))
}

/// Shape an Indeo 5 [`SessionOutput`] (a planar host buffer at the
/// stream's native subsampling) into an [`oxideav_core`] [`VideoFrame`]
/// in [`PixelFormat::Yuv444P`] plane order (Y, U, V).
///
/// The host buffer carries the three planes tightly packed at their
/// native resolutions (`spec/08 §5.3`); this unpacks them by role, box-
/// upsamples the two chroma planes to full luma resolution (`spec/08
/// §3.5`), and lays them out as three equal-size 8-bit planes.
///
/// Returns [`Error::invalid`] for the packed (`Yuy2`) / RGB output
/// formats — those are not planar concatenations and their conversion is
/// a `spec/08 §9` docs-gap. The staged INTRA/INTER decode path only ever
/// produces the planar `Yvu9` / `Yv12` / `I420` formats.
fn session_output_to_video_frame(out: &SessionOutput, pts: Option<i64>) -> Result<VideoFrame> {
    let (lw, lh) = out.dimensions;
    let Some(subsampling) = out.format.subsampling() else {
        return Err(Error::invalid(
            "indeo5: non-planar output format (Yuy2/RGB) is a spec/08 §9 docs-gap",
        ));
    };
    let (cw, ch) = subsampling.chroma_dims(lw, lh);

    let luma = OutputPlane {
        width: lw,
        height: lh,
        pixels: out.output.plane_bytes(PlaneRole::Luma).to_vec(),
    };
    let u_sub = OutputPlane {
        width: cw,
        height: ch,
        pixels: out.output.plane_bytes(PlaneRole::ChromaU).to_vec(),
    };
    let v_sub = OutputPlane {
        width: cw,
        height: ch,
        pixels: out.output.plane_bytes(PlaneRole::ChromaV).to_vec(),
    };

    let u = upsample_chroma(&u_sub, subsampling, lw, lh)
        .ok_or_else(|| Error::invalid("indeo5: chroma-U upsample dimension mismatch"))?;
    let v = upsample_chroma(&v_sub, subsampling, lw, lh)
        .ok_or_else(|| Error::invalid("indeo5: chroma-V upsample dimension mismatch"))?;

    let stride = lw as usize;
    let planes = vec![
        VideoPlane {
            stride,
            data: luma.pixels,
        },
        VideoPlane {
            stride,
            data: u.pixels,
        },
        VideoPlane {
            stride,
            data: v.pixels,
        },
    ];
    Ok(VideoFrame { pts, planes })
}

/// One-shot direct decode: decode a single Indeo 5 codec frame's bytes
/// into an [`oxideav_core`] [`VideoFrame`] in [`PixelFormat::Yuv444P`]
/// (Y, U, V) without managing a [`Decoder`] state machine.
///
/// This is the direct-API counterpart to the registry path, mirroring
/// the convention sibling codec crates follow. It builds a fresh
/// [`Indeo5Decoder`] and decodes `data` as the **first** frame; because
/// the session starts empty, `data` must be an INTRA frame (the `spec/01
/// §3.2` first-frame gate) — a non-INTRA first frame returns
/// [`Error::invalid`]. Callers decoding a *sequence* (where NULL-repeat
/// and the reference-bank rotation matter) want the stateful
/// [`Indeo5RegistryDecoder`] / [`Indeo5Decoder`] instead.
///
/// `pts` is carried straight onto the returned frame.
pub fn decode_video_frame(data: &[u8], pts: Option<i64>) -> Result<VideoFrame> {
    let mut decoder = Indeo5Decoder::new();
    let out = decoder.decode(data).map_err(map_session_error)?;
    session_output_to_video_frame(&out, pts)
}

impl Decoder for Indeo5RegistryDecoder {
    fn codec_id(&self) -> &CodecId {
        &self.codec_id
    }

    fn send_packet(&mut self, packet: &Packet) -> Result<()> {
        if self.pending.is_some() {
            return Err(Error::other(
                "indeo5 decoder: receive_frame must be called before sending another packet",
            ));
        }
        self.pending = Some(packet.clone());
        Ok(())
    }

    fn receive_frame(&mut self) -> Result<Frame> {
        let Some(pkt) = self.pending.take() else {
            return if self.eof {
                Err(Error::Eof)
            } else {
                Err(Error::NeedMore)
            };
        };
        let out = self.inner.decode(&pkt.data).map_err(map_session_error)?;
        Ok(Frame::Video(session_output_to_video_frame(&out, pkt.pts)?))
    }

    fn flush(&mut self) -> Result<()> {
        self.eof = true;
        Ok(())
    }

    fn reset(&mut self) -> Result<()> {
        // A container seek restarts the Indeo 5 inter-frame state: drop
        // the pending packet and the held session so the next packet
        // decodes as a fresh INTRA frame (the `spec/01 §3.2` first-frame
        // gate).
        self.pending = None;
        self.eof = false;
        self.inner = Indeo5Decoder::new();
        Ok(())
    }
}

/// Confidence [`probe`] returns when a first packet parses as a genuine
/// Indeo 5 picture header (PSC + frame-type + dimensions all validate).
pub const PROBE_CONFIDENCE_HEADER_OK: f32 = 1.0;

/// Confidence [`probe`] returns when no first-packet bytes are available
/// to validate (tags resolved at stream-discovery time, before any
/// packet is read). The FourCC match alone is decent evidence.
pub const PROBE_CONFIDENCE_TAG_ONLY: f32 = 0.6;

/// The [`oxideav_core::ProbeFn`] for Indeo 5 tag disambiguation.
///
/// When the demuxer has peeked a first packet ([`ProbeContext::packet`]
/// is `Some`), parse the `spec/01 §3` picture-start header (PSC `0x1f`,
/// frame type, and — for INTRA — the GOP dimensions):
///
/// * a structurally-valid picture header → [`PROBE_CONFIDENCE_HEADER_OK`];
/// * a packet present but whose header fails to parse → `0.0`
///   ("not me" — lets a colliding FourCC claimant win on genuinely
///   non-Indeo-5 bytes);
/// * no packet available → [`PROBE_CONFIDENCE_TAG_ONLY`].
///
/// The probe reads only the fixed header bits, never the coefficient
/// stream, so it stays cheap and touches no docs-gapped table.
pub fn probe(ctx: &ProbeContext) -> f32 {
    let Some(bytes) = ctx.packet else {
        return PROBE_CONFIDENCE_TAG_ONLY;
    };
    match PictureHeader::parse(bytes, None) {
        Ok(_) => PROBE_CONFIDENCE_HEADER_OK,
        Err(_) => 0.0,
    }
}

/// Register the Indeo 5 decoder (id + capabilities + factory + probe +
/// the `IV50` FourCC tag) into a [`CodecRegistry`].
///
/// The capabilities advertise the lossy video codec emitting
/// [`PixelFormat::Yuv444P`] (the full-luma-resolution surface the
/// registry decoder produces after the `spec/08 §3.5` chroma box
/// upsample). No encoder is registered — this crate is a decoder-only
/// clean-room rebuild.
pub fn register_codecs(reg: &mut CodecRegistry) {
    let caps = CodecCapabilities::video("indeo5_sw")
        .with_lossy(true)
        .with_pixel_format(PixelFormat::Yuv444P);
    reg.register(
        CodecInfo::new(CodecId::new(CODEC_ID_STR))
            .capabilities(caps)
            .decoder(make_decoder)
            .probe(probe)
            .tags([CodecTag::fourcc(b"IV50")]),
    );
}

/// Unified registration entry point: install the Indeo 5 codec factory
/// into the codec sub-registry of a [`RuntimeContext`].
pub fn register(ctx: &mut RuntimeContext) {
    register_codecs(&mut ctx.codecs);
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxideav_core::stream::CodecResolver;

    #[test]
    fn codec_id_for_fourcc_maps_iv50() {
        assert_eq!(
            codec_id_for_fourcc(b"IV50"),
            Some(CodecId::new(CODEC_ID_STR))
        );
    }

    #[test]
    fn codec_id_for_fourcc_is_case_insensitive() {
        assert_eq!(
            codec_id_for_fourcc(b"iv50"),
            Some(CodecId::new(CODEC_ID_STR))
        );
        assert_eq!(
            codec_id_for_fourcc(b"Iv50"),
            Some(CodecId::new(CODEC_ID_STR))
        );
    }

    #[test]
    fn codec_id_for_fourcc_rejects_other_indeo_and_unrelated() {
        // Indeo 2 (RT21 / IV20), Indeo 3 (IV31 / IV32), Indeo 4 (IV41)
        // are separate codecs and must not resolve to the indeo5 id.
        assert_eq!(codec_id_for_fourcc(b"RT21"), None);
        assert_eq!(codec_id_for_fourcc(b"IV20"), None);
        assert_eq!(codec_id_for_fourcc(b"IV31"), None);
        assert_eq!(codec_id_for_fourcc(b"IV32"), None);
        assert_eq!(codec_id_for_fourcc(b"IV41"), None);
        assert_eq!(codec_id_for_fourcc(b"avc1"), None);
    }

    #[test]
    fn decoder_registered_no_encoder() {
        let mut reg = CodecRegistry::new();
        register_codecs(&mut reg);
        assert!(reg.has_decoder(&CodecId::new(CODEC_ID_STR)));
        assert!(!reg.has_encoder(&CodecId::new(CODEC_ID_STR)));
    }

    #[test]
    fn register_via_runtime_context_installs_factory() {
        let mut ctx = RuntimeContext::new();
        register(&mut ctx);
        assert!(ctx.codecs.has_decoder(&CodecId::new(CODEC_ID_STR)));
    }

    #[test]
    fn registry_resolves_iv50_tag() {
        let mut reg = CodecRegistry::new();
        register_codecs(&mut reg);
        let tag = CodecTag::fourcc(b"IV50");
        let ctx = ProbeContext::new(&tag);
        let id = reg.resolve_tag(&ctx).expect("resolve_tag");
        assert_eq!(id, CodecId::new(CODEC_ID_STR));
    }

    #[test]
    fn probe_no_packet_returns_tag_only_confidence() {
        let tag = CodecTag::fourcc(b"IV50");
        let ctx = ProbeContext::new(&tag);
        assert_eq!(probe(&ctx), PROBE_CONFIDENCE_TAG_ONLY);
    }

    #[test]
    fn probe_garbage_packet_returns_zero() {
        // A short all-zero packet cannot carry a valid picture-start
        // triplet (the PSC is 0x1f, not 0).
        let tag = CodecTag::fourcc(b"IV50");
        let junk = [0u8; 4];
        let ctx = ProbeContext::new(&tag).packet(&junk);
        assert_eq!(probe(&ctx), 0.0);
    }

    #[test]
    fn make_decoder_reports_codec_id() {
        let params = CodecParameters::video(CodecId::new(CODEC_ID_STR));
        let dec = make_decoder(&params).expect("make_decoder");
        assert_eq!(dec.codec_id(), &CodecId::new(CODEC_ID_STR));
    }

    #[test]
    fn receive_before_send_needs_more() {
        let mut dec = Indeo5RegistryDecoder::new(CodecId::new(CODEC_ID_STR));
        assert!(matches!(dec.receive_frame(), Err(Error::NeedMore)));
    }

    #[test]
    fn flush_then_receive_is_eof() {
        let mut dec = Indeo5RegistryDecoder::new(CodecId::new(CODEC_ID_STR));
        dec.flush().expect("flush");
        assert!(matches!(dec.receive_frame(), Err(Error::Eof)));
    }
}
