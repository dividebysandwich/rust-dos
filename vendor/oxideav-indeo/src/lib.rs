//! Pure-Rust Indeo (IV2/IV3/IV4/IV5) video codec.
//!
//! **Indeo 3 (IV31 / IV32) — structural decode + unblocked
//! reconstruction + multi-frame sequencing.**
//!
//! The crate threads the spec/01 → spec/03 structural layers into one
//! [`indeo3::decode_frame`] pass (the 64-byte combined header, the
//! per-plane picture-layer prelude + decode plan, and the binary-tree
//! cell walk that produces a typed [`indeo3::CellTree`] of INTRA / INTER
//! leaf cells). On top of that, [`indeo3::reconstruct_frame`]
//! materialises the genuinely-unblocked (VQ_NULL) pixel subset of every
//! present plane into strip pixel buffers, and
//! [`indeo3::Indeo3Decoder`] drives a whole IV31 / IV32 frame sequence:
//! it owns the spec/07 §6 inter-frame state ([`indeo3::DecodeSession`])
//! and the previous [`indeo3::ReconstructedFrame`], enforcing the
//! first-frame / seek INTRA gate (spec/01 §3.2 / §4), the NULL-frame
//! repeat-previous output (spec/07 §6.3), and the reference-bank
//! ping-pong (spec/07 §6.1 / spec/05 §4.2).
//!
//! ## Where the codebook gate stands (r451)
//!
//! The formerly-gating codebook values are now material: the settled
//! seed grammar builds the codec-init **staging image**
//! ([`indeo3::StagingImage`], verified word-for-word against the
//! staged ground truth) and the per-frame `alt_quant[]` overlay fills
//! a real [`indeo3::VqArena`]. Driven over the vendored real `IV32`
//! fixture corpus, the fixture-arbitrated row-stream executor
//! ([`indeo3::decode_cell_rows`]) reproduces the black-box reference
//! decode byte-exactly outside the picture's re-coded detail region
//! (10 404 pixels pinned in `tests/indeo3_fixtures.rs`). What still
//! gates a full-frame decode is the **cell sequencing**: the
//! cell-geometry bank tables (`IR32_32.DLL!0x100038f0`) that place
//! cells without interleaved tree codes are explicitly undocumented
//! in the staged spec (`spec/04 §5.3`/`§7.9`) — the primary open
//! docs ask. See `crates/oxideav-indeo/README.md` for the precise
//! gap list.
//!
//! Spec coverage in `docs/video/indeo/indeo3/spec/`:
//!
//! * `00-scope.md` — bit/byte conventions + binary identity.
//! * `01-file-header.md` — frame + bitstream header, NULL-frame
//!   sentinel, continuity check.
//! * `02-picture-layer.md` — plane iteration order, plane prelude,
//!   half-pel scaling, packed-MV formula, per-plane decode plan.
//! * `03-macroblock-layer.md` — MSB-first bit reader, 2-bit node
//!   codes, MC_TREE / VQ_TREE walk, leaf index-byte fetch.
//! * `04-vq-codebooks.md` — the VQ codebook structure + codec-init
//!   table materialisation (the codebook-bank values are §7.1 gated).
//! * `05-motion-compensation.md` — packed-MV decode + the MC fetcher.
//! * `06-entropy.md` — the per-cell mode-byte stream alphabet.
//! * `07-output-reconstruction.md` — the predictor chain, strip-to-frame
//!   assembly, chroma upsample, and §6 frame finalisation.
//!
//! Indeo 5 (`IV50`) has a staged clean-room spec under
//! `docs/video/indeo/indeo5/spec/`; [`indeo5`] is bootstrapping its
//! decode stack bottom-up (LSB-first bit reader, format descriptor,
//! picture-start triplet, standard picture-size tables). Indeo 2 / 4
//! have only a multimedia.cx wiki snapshot under
//! `docs/video/indeo/indeoN/wiki/`, no `spec/`, so they remain at the
//! round-0 scaffold pending docs work.

#![forbid(unsafe_code)]

pub mod indeo3;
pub mod indeo5;

/// Install this crate's codecs into a [`oxideav_core::RuntimeContext`]:
/// the Indeo 3 (`IV31` / `IV32`) decoder and the Indeo 5 (`IV50`)
/// decoder.
///
/// This is the crate-level registration entry point, delegating to
/// [`indeo3::register`] and [`indeo5::register`]. The
/// `oxideav_core::register!` macro below wires it into `oxideav-meta`'s
/// zero-config fleet registration; direct callers can invoke this (or
/// [`indeo3::register_codecs`] / [`indeo5::register_codecs`] for a bare
/// [`oxideav_core::CodecRegistry`]) themselves.
pub fn register(ctx: &mut oxideav_core::RuntimeContext) {
    indeo3::register(ctx);
    indeo5::register(ctx);
}

oxideav_core::register!("indeo", register);

/// Crate-local error type.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// Reserved placeholder for codec paths that have not landed
    /// yet (anything beyond the Indeo 3 header + picture layer).
    NotImplemented,
    /// Indeo 3: header errors. Carries the spec section that
    /// pinned down the failing constraint.
    Indeo3Header(indeo3::HeaderError),
    /// Indeo 3: picture-layer (plane prelude) errors per
    /// `docs/video/indeo/indeo3/spec/02-picture-layer.md`.
    Indeo3PictureLayer(indeo3::PictureLayerError),
    /// Indeo 3: macroblock-layer (binary-tree walk) errors per
    /// `docs/video/indeo/indeo3/spec/03-macroblock-layer.md`.
    Indeo3Macroblock(indeo3::MacroblockError),
    /// Indeo 3: VQ-codebook materialisation errors per
    /// `docs/video/indeo/indeo3/spec/04-vq-codebooks.md`.
    Indeo3Vq(indeo3::VqError),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::NotImplemented => f.write_str(
                "oxideav-indeo: clean-room rebuild in progress — see crates/oxideav-indeo/README.md",
            ),
            Error::Indeo3Header(e) => write!(f, "oxideav-indeo[iv3 header]: {e}"),
            Error::Indeo3PictureLayer(e) => write!(f, "oxideav-indeo[iv3 picture]: {e}"),
            Error::Indeo3Macroblock(e) => write!(f, "oxideav-indeo[iv3 macroblock]: {e}"),
            Error::Indeo3Vq(e) => write!(f, "oxideav-indeo[iv3 vq]: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<indeo3::HeaderError> for Error {
    fn from(e: indeo3::HeaderError) -> Self {
        Error::Indeo3Header(e)
    }
}

impl From<indeo3::PictureLayerError> for Error {
    fn from(e: indeo3::PictureLayerError) -> Self {
        Error::Indeo3PictureLayer(e)
    }
}

impl From<indeo3::MacroblockError> for Error {
    fn from(e: indeo3::MacroblockError) -> Self {
        Error::Indeo3Macroblock(e)
    }
}

impl From<indeo3::VqError> for Error {
    fn from(e: indeo3::VqError) -> Self {
        Error::Indeo3Vq(e)
    }
}

/// Crate-local Result alias.
pub type Result<T> = core::result::Result<T, Error>;
