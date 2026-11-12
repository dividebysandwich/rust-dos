//! Indeo 3 (IV31 / IV32) codec-registry integration: the
//! [`oxideav_core`] [`Decoder`] trait wrapper around the in-crate
//! [`Indeo3PictureDecoder`], plus the FourCC routing surface a generic
//! container (`oxideav-avi`, `oxideav-mov`) needs to resolve an
//! `IV31` / `IV32` video track to this crate.
//!
//! Spec source for the FourCC set: `docs/video/indeo/indeo3/spec/00-scope.md`
//! (the two in-scope FourCCs `IV31` (R3.1) and `IV32` (R3.2), both of
//! which decode through the same code path — `spec/00 §`-scope and
//! `spec/01 §` confirm `IV31` is bitstream-identical to `IV32` at the
//! decoder level, the distinction being a purely external FourCC label).
//!
//! ## What this module adds
//!
//! Every other module in this crate is reachable only through the
//! crate's own typed API ([`Indeo3PictureDecoder::decode`] →
//! [`DecodedPicture`]). This module bridges that API to the framework's
//! published codec surface so a pipeline that resolves codecs through
//! an [`oxideav_core::CodecRegistry`] — the way the container crates do
//! — can construct and drive an Indeo 3 decoder without naming this
//! crate's concrete types:
//!
//! * [`codec_id_for_fourcc`] maps an on-wire FourCC (`IV31` / `IV32`,
//!   case-insensitive) to the [`CodecId`] this crate registers, so a
//!   demuxer's `CodecResolver` can route a video track here.
//! * [`Indeo3RegistryDecoder`] implements [`Decoder`]: it owns an
//!   [`Indeo3PictureDecoder`], feeds each [`Packet`]'s bytes through
//!   [`Indeo3PictureDecoder::decode`], and maps the resulting
//!   [`DecodedPicture`] (luma plus `spec/07 §5.5` box-upsampled chroma)
//!   into an [`oxideav_core::VideoFrame`] in [`PixelFormat::Yuv444P`]
//!   plane order (Y, U, V).
//! * [`make_decoder`] is the [`oxideav_core::registry::codec::DecoderFactory`]
//!   the registry calls; [`register_codecs`] / [`register`] install the
//!   codec (id + caps + factory + FourCC tags) into a
//!   [`CodecRegistry`] / [`RuntimeContext`]; and the crate-root
//!   `oxideav_core::register!` macro wires zero-config fleet
//!   registration.
//!
//! ## Output pixel format
//!
//! Indeo 3 is natively 4:1:0 (YVU9): luma at full resolution, chroma
//! subsampled 4×4. [`DecodedPicture::to_yuv444_planes`] box-upsamples
//! (`spec/07 §5.5`) both chroma planes to full luma resolution,
//! producing the three-plane surface the `spec/07 §5.4` YUV→RGB matrix
//! consumes. That surface is exactly
//! [`PixelFormat::Yuv444P`]-shaped (three equal-size 8-bit planes), so
//! the registry decoder emits `Yuv444P` rather than inventing a 4:1:0
//! format the framework does not carry. The chroma is genuine 4:1:0
//! data lifted to 4:4:4 geometry; no chroma detail is fabricated beyond
//! the spec's own box filter.
//!
//! ## Scope
//!
//! This is a thin bridge — it adds no decode behaviour of its own. Every
//! picture is what [`Indeo3PictureDecoder`] reconstructs (both staged
//! `IV32` corpora decode pixel-exact on every frame), re-shaped into
//! the framework's `VideoFrame`. A NULL / repeat frame re-emits the
//! previous frame (`spec/07 §6.3`), exactly as the underlying decoder
//! does.

use oxideav_core::{
    CodecCapabilities, CodecId, CodecInfo, CodecParameters, CodecRegistry, CodecTag, Decoder,
    Error, Frame, Packet, PixelFormat, ProbeContext, Result, RuntimeContext, VideoFrame,
    VideoPlane,
};

use super::header::{FRAME_HEADER_LEN, MAGIC_FRMH};
use super::picture_decoder::{DecodedPicture, Indeo3PictureDecoder, PictureDecodeError};

/// The public codec id this crate registers (`"indeo3"`).
pub const CODEC_ID_STR: &str = "indeo3";

/// The two in-scope Indeo 3 `VisualSampleEntry` / `fccHandler` FourCCs
/// (`docs/video/indeo/indeo3/spec/00-scope.md`): `IV31` (Indeo 3 R3.1)
/// and `IV32` (Indeo 3 R3.2, the common one). Both decode through the
/// same path — the distinction is a purely external FourCC label
/// (`spec/01 §`). Canonical upper-case spelling.
pub const INDEO3_FOURCCS: [&[u8; 4]; 2] = [b"IV31", b"IV32"];

/// Returns `Some(CodecId::new("indeo3"))` if `fourcc` (case-insensitive)
/// is one of the two in-scope Indeo 3 FourCCs (`IV31` / `IV32`).
///
/// A demuxer's `CodecResolver` calls this (or, equivalently, the
/// registry's tag path seeded by [`register_codecs`]) to route an
/// `IV31` / `IV32` video track to this crate.
pub fn codec_id_for_fourcc(fourcc: &[u8; 4]) -> Option<CodecId> {
    let mut upper = [0u8; 4];
    for i in 0..4 {
        upper[i] = fourcc[i].to_ascii_uppercase();
    }
    match &upper {
        b"IV31" | b"IV32" => Some(CodecId::new(CODEC_ID_STR)),
        _ => None,
    }
}

/// The [`oxideav_core::registry::codec::DecoderFactory`] for Indeo 3:
/// construct a fresh [`Indeo3RegistryDecoder`] from the stream
/// parameters.
///
/// The parameters' `width` / `height` / `pixel_format` are advisory —
/// the Indeo 3 bitstream carries its own dimensions in every frame
/// header (`spec/01 §`), so the decoder is self-describing and does not
/// require them. The factory therefore accepts any [`CodecParameters`]
/// whose `codec_id` routes here.
pub fn make_decoder(params: &CodecParameters) -> Result<Box<dyn Decoder>> {
    Ok(Box::new(Indeo3RegistryDecoder::new(
        params.codec_id.clone(),
    )))
}

/// An [`oxideav_core`] [`Decoder`] backed by the in-crate stateful
/// [`Indeo3PictureDecoder`].
///
/// Holds the multi-frame session (so NULL / repeat frames re-emit the
/// previous output per `spec/07 §6.3`) and a single-packet pending
/// slot. `send_packet` stashes the packet; `receive_frame` decodes it,
/// shapes the [`DecodedPicture`] into a [`PixelFormat::Yuv444P`]
/// [`VideoFrame`], and returns it.
pub struct Indeo3RegistryDecoder {
    codec_id: CodecId,
    inner: Indeo3PictureDecoder,
    pending: Option<Packet>,
    eof: bool,
}

impl Indeo3RegistryDecoder {
    /// Create a fresh registry decoder for the given codec id.
    pub fn new(codec_id: CodecId) -> Self {
        Indeo3RegistryDecoder {
            codec_id,
            inner: Indeo3PictureDecoder::new(),
            pending: None,
            eof: false,
        }
    }
}

/// Map an Indeo 3 [`DecoderError`] into the framework [`Error`] surface.
///
/// A session-level rejection (first-frame / seek must be INTRA, or a
/// malformed header) and a structural / reconstruction failure are all
/// surfaced as [`Error::invalid`] with the underlying message, since
/// they all mean "this packet's bytes are not a decodable Indeo 3
/// frame in this stream position".
fn map_decoder_error(e: PictureDecodeError) -> Error {
    Error::invalid(format!("indeo3: {e}"))
}

/// Shape a [`DecodedPicture`] into an [`oxideav_core`] [`VideoFrame`] in
/// [`PixelFormat::Yuv444P`] plane order (Y, U, V): luma as decoded,
/// each chroma plane box-replicated 4×4 to luma resolution
/// (`spec/07 §5.5`), every plane with stride == width.
fn picture_to_video_frame(pic: &DecodedPicture, pts: Option<i64>) -> VideoFrame {
    let planes = pic
        .to_yuv444_planes()
        .into_iter()
        .map(|data| VideoPlane {
            stride: pic.width as usize,
            data,
        })
        .collect();
    VideoFrame { pts, planes }
}

/// One-shot direct decode: decode a single Indeo 3 codec frame's bytes
/// into an [`oxideav_core`] [`VideoFrame`] in [`PixelFormat::Yuv444P`]
/// (Y, U, V) without managing a [`Decoder`] state machine.
///
/// This is the direct-API counterpart to the registry path, mirroring
/// the convention sibling codec crates follow (a free `decode_*`
/// function alongside the registry factory). It builds a fresh
/// [`Indeo3PictureDecoder`], decodes `data` as the **first** frame, and shapes
/// the output. Because the decoder starts empty, `data` must be an INTRA
/// frame (the `spec/01 §3.2` first-frame gate) — a non-INTRA first frame
/// returns [`Error::invalid`]. Callers decoding a *sequence* (where
/// inter-frame state, NULL-repeat, and the reference-bank ping-pong
/// matter) want the stateful [`Indeo3RegistryDecoder`] /
/// [`Indeo3PictureDecoder`] instead; this convenience is for
/// single-frame / first-frame decode.
///
/// `pts` is carried straight onto the returned frame.
pub fn decode_video_frame(data: &[u8], pts: Option<i64>) -> Result<VideoFrame> {
    let mut decoder = Indeo3PictureDecoder::new();
    let out = decoder.decode(data).map_err(map_decoder_error)?;
    Ok(picture_to_video_frame(&out, pts))
}

impl Decoder for Indeo3RegistryDecoder {
    fn codec_id(&self) -> &CodecId {
        &self.codec_id
    }

    fn send_packet(&mut self, packet: &Packet) -> Result<()> {
        if self.pending.is_some() {
            return Err(Error::other(
                "indeo3 decoder: receive_frame must be called before sending another packet",
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
        let out = self.inner.decode(&pkt.data).map_err(map_decoder_error)?;
        Ok(Frame::Video(picture_to_video_frame(&out, pkt.pts)))
    }

    fn flush(&mut self) -> Result<()> {
        self.eof = true;
        Ok(())
    }

    fn reset(&mut self) -> Result<()> {
        // A container seek restarts the Indeo 3 inter-frame state: drop
        // the pending packet and the held previous frame so the next
        // packet decodes as a fresh INTRA frame (the session's
        // first-frame / seek INTRA gate, spec/01 §3.2 / §4).
        self.pending = None;
        self.eof = false;
        self.inner = Indeo3PictureDecoder::new();
        Ok(())
    }
}

/// Confidence the [`probe`] returns when a first-packet's combined
/// header validates as a genuine Indeo 3 frame (the `spec/01 §2.1`
/// `check_sum` matches). Above the framework's `0.0` "not me" floor and
/// the implicit `1.0` an unprobed-but-tagged claim would get, this lets
/// a real Indeo 3 payload out-rank an unprobed claimant on the same
/// FourCC when one exists.
pub const PROBE_CONFIDENCE_HEADER_OK: f32 = 1.0;

/// Confidence the [`probe`] returns when no first-packet bytes are
/// available to validate (the common case: tags are resolved at
/// stream-discovery time before any packet is read). The FourCC match
/// alone is decent evidence, so this is a solid-but-not-certain score.
pub const PROBE_CONFIDENCE_TAG_ONLY: f32 = 0.6;

/// Spec/01 §2.1 — the [`oxideav_core::ProbeFn`] for Indeo 3 tag
/// disambiguation.
///
/// When the demuxer has peeked a first packet ([`ProbeContext::packet`]
/// is `Some`), validate the Indeo 3 combined-header `check_sum`
/// (`frame_number ^ unknown1 ^ frame_size ^ 'FRMH'`, the
/// [`super::FrameHeader::parse`] §2.1 check) and the §2.2
/// `frame_size > 16` constraint:
///
/// * a structurally-valid combined header → [`PROBE_CONFIDENCE_HEADER_OK`];
/// * a packet present but whose header fails the `check_sum` / size
///   checks → `0.0` ("not me" — this lets a colliding FourCC claimant
///   win on genuinely non-Indeo-3 bytes);
/// * no packet available → [`PROBE_CONFIDENCE_TAG_ONLY`] (the FourCC
///   match alone is decent evidence).
///
/// The probe is intentionally cheap — it reads only the fixed 16-byte
/// frame header words, not the full bitstream — so it never needs the
/// (docs-gapped) codebook-bank values.
pub fn probe(ctx: &ProbeContext) -> f32 {
    let Some(bytes) = ctx.packet else {
        return PROBE_CONFIDENCE_TAG_ONLY;
    };
    // Need the 16-byte frame header to validate the §2.1 check_sum.
    if bytes.len() < FRAME_HEADER_LEN {
        return 0.0;
    }
    let rd = |off: usize| -> u32 {
        u32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
    };
    let frame_number = rd(0x00);
    let unknown1 = rd(0x04);
    let check_sum = rd(0x08);
    let frame_size = rd(0x0c);
    let expected = frame_number ^ unknown1 ^ frame_size ^ MAGIC_FRMH;
    if check_sum != expected {
        return 0.0;
    }
    // §2.2 — frame_size must exceed the 16-byte frame header.
    if (frame_size as usize) <= FRAME_HEADER_LEN {
        return 0.0;
    }
    PROBE_CONFIDENCE_HEADER_OK
}

/// Register the Indeo 3 decoder (id + capabilities + factory + probe +
/// the two FourCC tags) into a [`CodecRegistry`].
///
/// The capabilities advertise the lossy video codec emitting
/// [`PixelFormat::Yuv444P`] (the full-luma-resolution surface the
/// registry decoder produces). The `IV31` / `IV32` tags let the
/// container's `CodecResolver` route either FourCC here, and the
/// [`probe`] validates a first packet's combined-header `check_sum`
/// (`spec/01 §2.1`) to out-rank a colliding claimant on genuine Indeo 3
/// bytes.
///
/// No encoder is registered — this crate is a decoder-only clean-room
/// rebuild.
pub fn register_codecs(reg: &mut CodecRegistry) {
    let caps = CodecCapabilities::video("indeo3_sw")
        .with_lossy(true)
        .with_pixel_format(PixelFormat::Yuv444P);
    reg.register(
        CodecInfo::new(CodecId::new(CODEC_ID_STR))
            .capabilities(caps)
            .decoder(make_decoder)
            .probe(probe)
            .tags([CodecTag::fourcc(b"IV31"), CodecTag::fourcc(b"IV32")]),
    );
}

/// Unified registration entry point: install the Indeo 3 codec factory
/// into the codec sub-registry of a [`RuntimeContext`].
///
/// This is the preferred entry point and matches the convention every
/// sibling crate follows. Direct callers that need only the codec
/// sub-registry can use [`register_codecs`].
pub fn register(ctx: &mut RuntimeContext) {
    register_codecs(&mut ctx.codecs);
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxideav_core::TimeBase;
    use oxideav_core::stream::CodecResolver;

    fn packet(data: Vec<u8>) -> Packet {
        Packet {
            stream_index: 0,
            time_base: TimeBase::new(1, 1000),
            pts: Some(0),
            dts: Some(0),
            duration: None,
            flags: Default::default(),
            data,
        }
    }

    // A minimal valid combined header whose three plane offsets are all
    // negative (every plane skipped) → a structurally-valid INTRA frame
    // that reconstructs to an empty (no-plane) frame. This keeps the
    // registry-bridge tests focused on the trait wiring + frame shaping,
    // not on the per-plane pixel synthesis (covered elsewhere).
    fn skipped_intra_frame(frame_number: u32) -> Vec<u8> {
        use super::super::header::{
            COMBINED_HEADER_LEN, FRAME_HEADER_LEN, MAGIC_FRMH, REQUIRED_DEC_VERSION,
        };
        const INTRA: u16 = 0x0004;
        let mut buf = vec![0u8; COMBINED_HEADER_LEN];
        let unknown1: u32 = 0;
        let frame_size: u32 = COMBINED_HEADER_LEN as u32;
        let check_sum = frame_number ^ unknown1 ^ frame_size ^ MAGIC_FRMH;
        buf[0x00..0x04].copy_from_slice(&frame_number.to_le_bytes());
        buf[0x04..0x08].copy_from_slice(&unknown1.to_le_bytes());
        buf[0x08..0x0c].copy_from_slice(&check_sum.to_le_bytes());
        buf[0x0c..0x10].copy_from_slice(&frame_size.to_le_bytes());
        let b = FRAME_HEADER_LEN;
        buf[b..b + 2].copy_from_slice(&REQUIRED_DEC_VERSION.to_le_bytes());
        buf[b + 2..b + 4].copy_from_slice(&INTRA.to_le_bytes());
        buf[b + 4..b + 8].copy_from_slice(&4096u32.to_le_bytes());
        buf[b + 0x0c..b + 0x0e].copy_from_slice(&64u16.to_le_bytes());
        buf[b + 0x0e..b + 0x10].copy_from_slice(&64u16.to_le_bytes());
        let neg = 0x8000_0000u32;
        buf[b + 0x10..b + 0x14].copy_from_slice(&neg.to_le_bytes());
        buf[b + 0x14..b + 0x18].copy_from_slice(&neg.to_le_bytes());
        buf[b + 0x18..b + 0x1c].copy_from_slice(&neg.to_le_bytes());
        buf
    }

    #[test]
    fn codec_id_for_fourcc_maps_both() {
        for fc in INDEO3_FOURCCS {
            assert_eq!(codec_id_for_fourcc(fc), Some(CodecId::new(CODEC_ID_STR)));
        }
    }

    #[test]
    fn codec_id_for_fourcc_is_case_insensitive() {
        assert_eq!(
            codec_id_for_fourcc(b"iv32"),
            Some(CodecId::new(CODEC_ID_STR))
        );
        assert_eq!(
            codec_id_for_fourcc(b"Iv31"),
            Some(CodecId::new(CODEC_ID_STR))
        );
    }

    #[test]
    fn codec_id_for_fourcc_rejects_other_indeo_and_unrelated() {
        // Indeo 2 (RT21 / IV20), Indeo 4 (IV41), Indeo 5 (IV50) are
        // separate codecs not handled by this crate, and must not
        // resolve to the indeo3 id.
        assert_eq!(codec_id_for_fourcc(b"RT21"), None);
        assert_eq!(codec_id_for_fourcc(b"IV20"), None);
        assert_eq!(codec_id_for_fourcc(b"IV41"), None);
        assert_eq!(codec_id_for_fourcc(b"IV50"), None);
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
    fn registry_resolves_both_fourcc_tags() {
        let mut reg = CodecRegistry::new();
        register_codecs(&mut reg);
        for fc in INDEO3_FOURCCS {
            let tag = CodecTag::fourcc(fc);
            let ctx = ProbeContext::new(&tag);
            let id = reg.resolve_tag(&ctx).expect("resolve_tag");
            assert_eq!(id, CodecId::new(CODEC_ID_STR), "fourcc {fc:?}");
        }
    }

    #[test]
    fn probe_no_packet_returns_tag_only_confidence() {
        let tag = CodecTag::fourcc(b"IV32");
        let ctx = ProbeContext::new(&tag);
        assert_eq!(probe(&ctx), PROBE_CONFIDENCE_TAG_ONLY);
    }

    #[test]
    fn probe_valid_header_returns_high_confidence() {
        let frame = skipped_intra_frame(0);
        let tag = CodecTag::fourcc(b"IV32");
        let ctx = ProbeContext::new(&tag).packet(&frame);
        assert_eq!(probe(&ctx), PROBE_CONFIDENCE_HEADER_OK);
    }

    #[test]
    fn probe_bad_checksum_returns_zero() {
        let mut frame = skipped_intra_frame(0);
        // Corrupt the check_sum word (offset 0x08) so §2.1 fails.
        frame[0x08] ^= 0xff;
        let tag = CodecTag::fourcc(b"IV32");
        let ctx = ProbeContext::new(&tag).packet(&frame);
        assert_eq!(probe(&ctx), 0.0);
    }

    #[test]
    fn probe_short_packet_returns_zero() {
        // Fewer than the 16-byte frame header → cannot validate.
        let short = vec![0u8; 8];
        let tag = CodecTag::fourcc(b"IV31");
        let ctx = ProbeContext::new(&tag).packet(&short);
        assert_eq!(probe(&ctx), 0.0);
    }

    #[test]
    fn probe_frame_size_too_small_returns_zero() {
        // A header whose check_sum is internally consistent but whose
        // frame_size is <= 16 fails the §2.2 size floor.
        let mut frame = vec![0u8; FRAME_HEADER_LEN];
        let frame_number = 0u32;
        let unknown1 = 0u32;
        let frame_size = 8u32; // <= 16
        let check_sum = frame_number ^ unknown1 ^ frame_size ^ MAGIC_FRMH;
        frame[0x00..0x04].copy_from_slice(&frame_number.to_le_bytes());
        frame[0x04..0x08].copy_from_slice(&unknown1.to_le_bytes());
        frame[0x08..0x0c].copy_from_slice(&check_sum.to_le_bytes());
        frame[0x0c..0x10].copy_from_slice(&frame_size.to_le_bytes());
        let tag = CodecTag::fourcc(b"IV32");
        let ctx = ProbeContext::new(&tag).packet(&frame);
        assert_eq!(probe(&ctx), 0.0);
    }

    #[test]
    fn registered_probe_validates_packet_through_resolver() {
        // The registered probe must drive resolve_tag: a valid first
        // packet resolves to indeo3; a corrupt one is rejected (no other
        // claimant on IV32 → resolve_tag returns None).
        let mut reg = CodecRegistry::new();
        register_codecs(&mut reg);
        let frame = skipped_intra_frame(0);
        let tag = CodecTag::fourcc(b"IV32");
        let ok_ctx = ProbeContext::new(&tag).packet(&frame);
        assert_eq!(
            reg.resolve_tag(&ok_ctx),
            Some(CodecId::new(CODEC_ID_STR)),
            "valid packet resolves"
        );

        let mut bad = frame.clone();
        bad[0x08] ^= 0xff;
        let bad_ctx = ProbeContext::new(&tag).packet(&bad);
        assert_eq!(
            reg.resolve_tag(&bad_ctx),
            None,
            "corrupt packet is rejected (probe returns 0.0, no other claimant)"
        );
    }

    #[test]
    fn make_decoder_reports_codec_id() {
        let params = CodecParameters::video(CodecId::new(CODEC_ID_STR));
        let dec = make_decoder(&params).expect("make_decoder");
        assert_eq!(dec.codec_id(), &CodecId::new(CODEC_ID_STR));
    }

    #[test]
    fn receive_before_send_needs_more() {
        let mut dec = Indeo3RegistryDecoder::new(CodecId::new(CODEC_ID_STR));
        assert!(matches!(dec.receive_frame(), Err(Error::NeedMore)));
    }

    #[test]
    fn double_send_without_receive_errors() {
        let mut dec = Indeo3RegistryDecoder::new(CodecId::new(CODEC_ID_STR));
        dec.send_packet(&packet(skipped_intra_frame(0)))
            .expect("first send");
        assert!(dec.send_packet(&packet(skipped_intra_frame(1))).is_err());
    }

    #[test]
    fn flush_then_receive_is_eof() {
        let mut dec = Indeo3RegistryDecoder::new(CodecId::new(CODEC_ID_STR));
        dec.flush().expect("flush");
        assert!(matches!(dec.receive_frame(), Err(Error::Eof)));
    }

    #[test]
    fn first_inter_packet_is_invalid() {
        // A non-INTRA first frame is the session's first-frame gate
        // rejection (spec/01 §3.2), surfaced as Error::invalid.
        let mut dec = Indeo3RegistryDecoder::new(CodecId::new(CODEC_ID_STR));
        let mut data = skipped_intra_frame(0);
        // Clear the INTRA flag (bit 2) in frame_flags to make it INTER.
        use super::super::header::FRAME_HEADER_LEN;
        let b = FRAME_HEADER_LEN;
        data[b + 2..b + 4].copy_from_slice(&0u16.to_le_bytes());
        dec.send_packet(&packet(data)).expect("send");
        assert!(matches!(dec.receive_frame(), Err(Error::InvalidData(_))));
    }

    #[test]
    fn skipped_intra_decodes_to_zero_video_frame() {
        // An all-planes-skipped INTRA frame reconstructs to no planes;
        // the registry decoder maps that to a Video frame with zero
        // planes and the packet's pts.
        let mut dec = Indeo3RegistryDecoder::new(CodecId::new(CODEC_ID_STR));
        dec.send_packet(&packet(skipped_intra_frame(0)))
            .expect("send");
        let frame = dec.receive_frame().expect("receive");
        match frame {
            Frame::Video(v) => {
                assert_eq!(v.planes.len(), 3);
                assert!(v.planes[0].data.iter().all(|&b| b == 0));
                assert_eq!(v.pts, Some(0));
            }
            other => panic!("expected video frame, got {other:?}"),
        }
        // No more frames until the next packet.
        assert!(matches!(dec.receive_frame(), Err(Error::NeedMore)));
    }

    #[test]
    fn reset_restarts_intra_gate() {
        let mut dec = Indeo3RegistryDecoder::new(CodecId::new(CODEC_ID_STR));
        dec.send_packet(&packet(skipped_intra_frame(0)))
            .expect("send");
        let _ = dec.receive_frame().expect("receive");
        dec.reset().expect("reset");
        // After reset the next frame is treated as the first again — an
        // INTER frame would be rejected; an INTRA one is accepted.
        dec.send_packet(&packet(skipped_intra_frame(0)))
            .expect("send post-reset");
        assert!(dec.receive_frame().is_ok());
    }

    #[test]
    fn decode_video_frame_one_shot_intra() {
        // An all-planes-skipped INTRA frame decodes through the one-shot
        // path to a Yuv444P VideoFrame whose planes keep the untouched
        // (zero) bank content, with the supplied pts.
        let vf = decode_video_frame(&skipped_intra_frame(0), Some(42)).expect("one-shot decode");
        assert_eq!(vf.planes.len(), 3);
        assert!(vf.planes[0].data.iter().all(|&b| b == 0));
        assert_eq!(vf.pts, Some(42));
    }

    #[test]
    fn decode_video_frame_rejects_inter_first() {
        // A non-INTRA first frame fails the spec/01 §3.2 first-frame gate.
        let mut data = skipped_intra_frame(0);
        use super::super::header::FRAME_HEADER_LEN;
        let b = FRAME_HEADER_LEN;
        data[b + 2..b + 4].copy_from_slice(&0u16.to_le_bytes());
        assert!(matches!(
            decode_video_frame(&data, None),
            Err(Error::InvalidData(_))
        ));
    }

    #[test]
    fn picture_to_video_frame_orders_planes_y_u_v() {
        // A 4x4 picture with 1x1 chroma: plane 0 = Y, 1 = U (box-replicated),
        // 2 = V, every plane full luma resolution.
        let pic = DecodedPicture {
            width: 4,
            height: 4,
            chroma_width: 1,
            chroma_height: 1,
            luma: vec![0x10; 16],
            chroma_v: vec![0x30],
            chroma_u: vec![0x20],
            repeated_previous: false,
            admission: super::super::frame_session::DecodeSession::new()
                .admit(&skipped_intra_frame(0))
                .unwrap(),
            stats: None,
        };
        let vf = picture_to_video_frame(&pic, Some(7));
        assert_eq!(vf.pts, Some(7));
        assert_eq!(vf.planes.len(), 3);
        assert!(vf.planes[0].data.iter().all(|&b| b == 0x10));
        assert!(vf.planes[1].data.iter().all(|&b| b == 0x20));
        assert!(vf.planes[2].data.iter().all(|&b| b == 0x30));
        assert_eq!(vf.planes[0].stride, 4);
        assert_eq!(vf.planes[1].data.len(), 16);
    }
}
