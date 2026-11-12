//! Indeo 5 whole-frame decode driver.
//!
//! Spec source: `docs/video/indeo/indeo5/spec/01`..`spec/08` — the
//! per-frame thread of `spec/02 §4.4`:
//!
//! ```text
//! picture header (spec/01 + spec/02 §1/§2)
//!   for each plane band chain:
//!     for each band:
//!       band header (spec/02 §3)          — empty band -> zeros
//!       for each tile in raster order:
//!         per-tile size header (spec/03 §2) — empty tile -> zeros
//!         per-MB header phase (spec/03 §3/§4)
//!         per-block coefficient phase (spec/05)
//!     wavelet recompose (spec/06 §3)
//!   bias-and-clamp + planar pack (spec/08) -> HostBuffer
//! ```
//!
//! ## Tile phase split (fixture-arbitrated, r388)
//!
//! Within one coded tile **all per-MB headers come first, then all
//! coded blocks' coefficient streams** — the Indeo 4 wiki "Bitstream
//! organization" split ("Macroblocks info data" then "Blocks data"),
//! shared by Indeo 5. This two-phase layout (with the CBP-before-
//! qdelta field order, [`super::MbHeader`]) is the only arrangement
//! under which both staged `IV50` INTRA fixtures decode every band to
//! byte-exact exhaustion; an interleaved per-MB reading does not
//! decode the fixtures.
//!
//! The per-block `(run, val)` streams decode through the band's
//! block-Huffman codebook ([`super::Codebook`], prefix form) and the
//! band's rv-table ([`super::RvTable`], the r338 static slots +
//! swap corrections). §2.8 explicit tile sizes are confirmed to span
//! the whole tile from its first byte (three independent byte-exact
//! chains in the 320x240 fixture), and the driver repositions on them.
//!
//! **Reconstruction (r451).** Intra bands now reconstruct to pixels:
//! per coded block the scan-ordered symbols place through the
//! `spec/06 §2.5` scan table, the intra DC chain (Indeo 4 annex-B
//! differential DC, shared by Indeo 5) resolves scan position 0, the
//! `spec/06 §1.2`/§2.3 inverse Slant kernel (validated byte-exact
//! upstream on 460 live invocations) transforms the block, and the
//! `spec/08 §3.0` saturate-then-bias writes plane bytes. The all-flat
//! fixture verifies **fully** against all four of its stored
//! `spec/08 §7.3` checksums (Y band, both chroma bands, frame).
//!
//! **Gated stage.** The `band_glob_quant` dequantisation of decoded
//! levels is a documented docs-gap (`spec/06 §5.4`: where the scale
//! multiplies "is not established"), so levels stage as-is — exact at
//! `band_glob_quant == 0`; quantised bands are reconstructed but
//! their `spec/08 §7.3` checksum oracle reports the remaining gap
//! quantitatively.

use super::assemble::{AssembleError, assemble_frame};
use super::band::{BandError, BandHeader};
use super::bitreader::{BitReader, BitReaderError};
use super::codebook::{Codebook, CodebookError, HuffContext};
use super::format::OutputFormat;
use super::frame::FrameHeader;
use super::gop::{BandInfo, GopHeader, Subsampling, TransformId};
use super::header::FrameType;
use super::level_table::{LEVEL_TABLE_LEN, build_level_table};
use super::mb::MbGrid;
use super::mb_header::{MbContext, MbHeader, MbHeaderError, QdeltaMode, effective_mb_quant};
use super::mc::{BandView, McError, mc_add_block};
use super::mv::{Mv, MvPredictor, MvResolution, resolve_mv};
use super::output::{OutputError, ReconstructionPlane, plane_stride};
use super::pack::HostBuffer;
use super::picture::{PictureError, PictureHeader};
use super::quant::{BandQuant, dequant_level};
use super::rv_table::{RvEntry, RvTable, RvTableError, escape_lindex, escape_value, run_advance};
use super::tile::TileGrid;
use super::tile_header::{TileDataSize, TileHeader};
use super::transform::{
    SCAN_COLUMN_8X8, SCAN_RASTER_4X4, SCAN_RASTER_8X8, SCAN_ZIGZAG_4X4, SCAN_ZIGZAG_8X8,
    inverse_slant_2d_4x4, inverse_slant_2d_8x8, inverse_slant_col_4x4, inverse_slant_col_8x8,
    inverse_slant_row_4x4, inverse_slant_row_8x8, place_scan_4x4, place_scan_8x8,
};
use super::wavelet::{Band, LevelBands, recompose_plane};

/// The largest picture area the driver allocates band buffers for
/// (4096 × 4096 samples): a resource guard against hostile headers,
/// far above any Indeo 5 stream's real size.
pub const MAX_PICTURE_PIXELS: u64 = 4096 * 4096;

/// Errors raised by the whole-frame driver.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecodeError {
    /// Fault in the spec/01/spec/02 picture-header stack.
    Picture(PictureError),
    /// The driver currently decodes INTRA (and NULL) frames only;
    /// INTER frames need the session-carried GOP state + reference
    /// planes (`spec/07`).
    NotIntra {
        /// The frame type found.
        found: FrameType,
    },
    /// Fault in a band header.
    Band {
        /// Plane index (0 = Y; 1 = the V chain, 2 = the U chain —
        /// `YVU9` coded order, `spec/02 §4.4`).
        plane_idx: usize,
        /// Band index within the plane.
        band_idx: usize,
        /// The underlying error.
        error: BandError,
    },
    /// A codebook descriptor failed to build (`spec/04 §1`).
    Codebook(CodebookError),
    /// A band's `rv_tab_sel` was invalid (`spec/02 §3.5`).
    RvTable(RvTableError),
    /// Fault in a per-MB header.
    MbHeader {
        /// Plane index.
        plane_idx: usize,
        /// Band index.
        band_idx: usize,
        /// Tile index in raster order.
        tile_idx: usize,
        /// The underlying error.
        error: MbHeaderError,
    },
    /// A per-block coefficient stream faulted (`spec/05`).
    BlockStream {
        /// Plane index.
        plane_idx: usize,
        /// Band index.
        band_idx: usize,
        /// Tile index in raster order.
        tile_idx: usize,
        /// Description of the fault.
        reason: BlockStreamFault,
    },
    /// An explicit tile size (`spec/03 §2.8`) or `band_data_size`
    /// (`spec/02 §3.2`) placed the next-payload target *behind* the
    /// parser's position — the stream and its size fields disagree.
    SizeFieldBehindCursor {
        /// The parser's byte position.
        at_byte: u64,
        /// The size field's target byte.
        target_byte: u64,
    },
    /// An INTER frame's reference band buffers did not match the
    /// GOP-derived band geometry (`spec/07 §1.2` workspace contract).
    ReferenceMismatch {
        /// Plane index.
        plane_idx: usize,
        /// Band index.
        band_idx: usize,
    },
    /// A motion-compensated fetch fell outside the reference band
    /// (`spec/07 §5.4` padding contract violated by the MV).
    Mc {
        /// Plane index.
        plane_idx: usize,
        /// Band index.
        band_idx: usize,
        /// Tile index in raster order.
        tile_idx: usize,
        /// The underlying error.
        error: McError,
    },
    /// The picture area exceeds [`MAX_PICTURE_PIXELS`] — a decoder
    /// resource guard, not a format limit (the custom-dimension fields
    /// can name 8191×8191, which would cost gigabytes of band buffers).
    PictureTooLarge {
        /// The picture width.
        width: u32,
        /// The picture height.
        height: u32,
    },
    /// Underlying bit-reader fault.
    BitReader(BitReaderError),
    /// Reconstruction-plane geometry fault (`spec/08`).
    Output(OutputError),
    /// Output-assembly fault (`spec/08`).
    Assemble(AssembleError),
}

/// The ways a per-block coefficient stream can fault (`spec/05`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockStreamFault {
    /// The scan position advanced past the block's coefficient count.
    PositionOverflow {
        /// The overflowing position.
        position: i32,
        /// The block's coefficient budget (16 or 64).
        budget: u32,
    },
    /// A decoded symbol had no rv-table mapping (the over-256
    /// custom-codebook tail — a reported docs-gap).
    UnmappedSymbol {
        /// The decoded symbol.
        symbol: u32,
    },
}

impl From<PictureError> for DecodeError {
    fn from(e: PictureError) -> Self {
        DecodeError::Picture(e)
    }
}
impl From<BitReaderError> for DecodeError {
    fn from(e: BitReaderError) -> Self {
        DecodeError::BitReader(e)
    }
}
impl From<CodebookError> for DecodeError {
    fn from(e: CodebookError) -> Self {
        DecodeError::Codebook(e)
    }
}
impl From<RvTableError> for DecodeError {
    fn from(e: RvTableError) -> Self {
        DecodeError::RvTable(e)
    }
}
impl From<OutputError> for DecodeError {
    fn from(e: OutputError) -> Self {
        DecodeError::Output(e)
    }
}
impl From<AssembleError> for DecodeError {
    fn from(e: AssembleError) -> Self {
        DecodeError::Assemble(e)
    }
}

impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DecodeError::Picture(e) => write!(f, "indeo5 decode: {e}"),
            DecodeError::NotIntra { found } => write!(
                f,
                "indeo5 decode: frame type {found:?} needs session state (INTRA-only driver)"
            ),
            DecodeError::Band {
                plane_idx,
                band_idx,
                error,
            } => write!(
                f,
                "indeo5 decode: plane {plane_idx} band {band_idx}: {error}"
            ),
            DecodeError::Codebook(e) => write!(f, "indeo5 decode: {e}"),
            DecodeError::RvTable(e) => write!(f, "indeo5 decode: {e}"),
            DecodeError::MbHeader {
                plane_idx,
                band_idx,
                tile_idx,
                error,
            } => write!(
                f,
                "indeo5 decode: plane {plane_idx} band {band_idx} tile {tile_idx}: {error}"
            ),
            DecodeError::BlockStream {
                plane_idx,
                band_idx,
                tile_idx,
                reason,
            } => write!(
                f,
                "indeo5 decode: plane {plane_idx} band {band_idx} tile {tile_idx}: block stream {reason:?} (spec/05)"
            ),
            DecodeError::SizeFieldBehindCursor {
                at_byte,
                target_byte,
            } => write!(
                f,
                "indeo5 decode: size field targets byte {target_byte} behind cursor byte {at_byte} (spec/03 §2.8)"
            ),
            DecodeError::ReferenceMismatch {
                plane_idx,
                band_idx,
            } => write!(
                f,
                "indeo5 decode: plane {plane_idx} band {band_idx}: reference band geometry mismatch (spec/07 §1.2)"
            ),
            DecodeError::Mc {
                plane_idx,
                band_idx,
                tile_idx,
                error,
            } => write!(
                f,
                "indeo5 decode: plane {plane_idx} band {band_idx} tile {tile_idx}: {error}"
            ),
            DecodeError::PictureTooLarge { width, height } => write!(
                f,
                "indeo5 decode: picture {width}x{height} exceeds the {MAX_PICTURE_PIXELS}-pixel decoder guard"
            ),
            DecodeError::BitReader(e) => write!(f, "indeo5 decode: {e}"),
            DecodeError::Output(e) => write!(f, "indeo5 decode: {e}"),
            DecodeError::Assemble(e) => write!(f, "indeo5 decode: {e}"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Why a walk stopped short of full reconstruction at some position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontierReason {
    /// A coded tile in an inter band with the MV-inheritance flag
    /// set — gated on the `spec/07 §3.4`/`§3.5` per-band
    /// `0x3604`/`0x3664` inheritance-MV tables docs-gap.
    MvInheritance,
}

/// One gated element the driver encountered (and, where possible,
/// skipped past via an explicit size field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeFrontier {
    /// Plane index (0 = Y; 1 = V chain, 2 = U chain).
    pub plane_idx: usize,
    /// Band index within the plane.
    pub band_idx: usize,
    /// Tile index in raster order.
    pub tile_idx: usize,
    /// The gate.
    pub reason: FrontierReason,
    /// `true` when an explicit tile size / `band_data_size` allowed
    /// the driver to reposition and keep parsing; `false` when the
    /// parse stopped here (`parse_complete == false`).
    pub skipped_past: bool,
}

/// Aggregate structural counts over one decoded frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct DecodeStats {
    /// Bands walked (including empty ones).
    pub bands: u32,
    /// Bands taking the `spec/02 §3.3` empty fast path.
    pub empty_bands: u32,
    /// Tiles walked.
    pub tiles: u32,
    /// Tiles with the `spec/03 §2.2` empty flag.
    pub empty_tiles: u32,
    /// Macroblocks walked.
    pub mbs: u32,
    /// Macroblocks with the `spec/03 §4.1` skip flag.
    pub mbs_skipped: u32,
    /// Coded macroblocks whose CBP carried no AC data.
    pub mbs_coded_no_ac: u32,
    /// Coded blocks whose `(run, val)` stream was decoded (`spec/05`).
    pub coded_blocks: u32,
    /// Non-zero coefficients decoded across all coded blocks.
    pub coefficients: u64,
    /// Escape-path emissions (`spec/05 §4.2`).
    pub escapes: u32,
}

/// Per-band byte-consumption trace: how many bytes of the band's
/// payload the structural walk consumed (before size-field
/// reconciliation), against the declared `band_data_size`. The staged
/// fixtures decode with `consumed <= declared` and a small trailing
/// tail (encoder padding inside the last tile's explicit byte count).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BandTrace {
    /// Plane index.
    pub plane_idx: usize,
    /// Band index within the plane.
    pub band_idx: usize,
    /// Bytes consumed by the walk from the band header's first byte
    /// (after end-of-band byte alignment).
    pub consumed: u64,
    /// The declared `band_data_size` (`None` when the frame does not
    /// carry per-band sizes).
    pub declared: Option<u32>,
}

/// One band's decoded coefficient work list plus its checksum status:
/// the input the (docs-gapped) coefficient→pixel transform stage
/// consumes, and the `spec/08 §7` reconstruction oracle for the band.
#[derive(Debug, Clone)]
pub struct BandReconstruction {
    /// Plane index (0 = Y; 1 = V chain, 2 = U chain).
    pub plane_idx: usize,
    /// Band index within the plane.
    pub band_idx: usize,
    /// The band's global quantiser (`spec/06 §5.1`, `band_glob_quant`).
    pub glob_quant: u8,
    /// Every walked block of the band, in decode order, carrying its
    /// scan-ordered decoded coefficients (`spec/05` stream) plus the
    /// block's effective per-MB quantiser (`spec/06 §5.2`). Empty when
    /// the band took a gated / empty path.
    pub blocks: Vec<BlockRecord>,
    /// The band's stored `band_checksum` (`spec/08 §7.2`), if present.
    pub stored_checksum: Option<u16>,
    /// The band checksum recomputed from the reconstructed pixels vs
    /// the stored value (`spec/08 §7.2`, formula recovered by black-box
    /// validation) — a byte-sum-exact reconstruction oracle. Luma bands
    /// stay [`ChecksumStatus::Mismatch`] while the transform is gated;
    /// genuinely-flat chroma bands match exactly.
    pub checksum: super::ChecksumStatus,
}

/// One decoded INTRA frame: the parsed header stack, the assembled
/// host buffer, and the structural coverage report.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DecodedPicture {
    /// The parsed picture header.
    pub header: PictureHeader,
    /// The host output format the GOP subsampling selects
    /// (`None` for a NULL frame).
    pub format: Option<OutputFormat>,
    /// The assembled planar host buffer (`None` for a NULL frame —
    /// `spec/08 §6.4` no-output path).
    pub output: Option<HostBuffer>,
    /// Every gated element encountered, in decode order.
    pub frontiers: Vec<DecodeFrontier>,
    /// Structural coverage counts.
    pub stats: DecodeStats,
    /// Per-band consumption traces.
    pub band_traces: Vec<BandTrace>,
    /// Per-band decoded coefficients + reconstruction-checksum status
    /// (the coefficient→pixel transform's input work list and its
    /// `spec/08 §7` oracle).
    pub bands: Vec<BandReconstruction>,
    /// The recomputed-vs-stored **frame** checksum (`spec/08 §7.1`,
    /// formula recovered by black-box validation). [`ChecksumStatus::Match`]
    /// only once every plane reconstructs byte-sum-exactly.
    pub frame_checksum: super::ChecksumStatus,
    /// `false` when a frontier could not be skipped past and parsing
    /// stopped early (later bands / planes reconstruct as zeros).
    pub parse_complete: bool,
}

impl DecodedPicture {
    /// `true` when every walked structural element was decoded without
    /// hitting a gate.
    pub fn fully_reconstructed(&self) -> bool {
        self.parse_complete && self.frontiers.is_empty()
    }

    /// The count of bands whose recomputed checksum matched the stored
    /// value (`spec/08 §7.2`) — bands reconstructed byte-sum-exactly.
    pub fn bands_verified(&self) -> usize {
        self.bands.iter().filter(|b| b.checksum.verified()).count()
    }
}

/// Per-plane geometry derived from the GOP header (`spec/02 §1.5`/
/// `§1.6`).
struct PlaneGeom<'g> {
    width: u32,
    height: u32,
    levels: u32,
    band_info: &'g [BandInfo],
}

/// The band dimensions for band `band_idx` of a plane with `levels`
/// decomposition levels (`spec/06 §3.1`/`§4.1`).
fn band_dims(plane_w: u32, plane_h: u32, levels: u32, band_idx: usize) -> (u32, u32) {
    let shift = if band_idx == 0 {
        levels
    } else {
        levels - (band_idx as u32 - 1) / 3
    };
    (
        plane_w.div_ceil(1 << shift).max(1),
        plane_h.div_ceil(1 << shift).max(1),
    )
}

/// The per-band tile grid counts: `ceil(band_dim / slice_size)` per
/// axis (`spec/03 §1.1`), one tile when the GOP carries no slice size.
fn band_tile_counts(band_w: u32, band_h: u32, slice_size: Option<u32>) -> (u32, u32) {
    match slice_size {
        None => (1, 1),
        Some(s) => (band_w.div_ceil(s).max(1), band_h.div_ceil(s).max(1)),
    }
}

/// Reposition the reader at an absolute byte target (used by the
/// explicit-size tile skip and the `band_data_size` band skip).
fn skip_to_byte(r: &mut BitReader<'_>, target_byte: u64) -> Result<(), DecodeError> {
    let target_bits = target_byte * 8;
    let at = r.bits_read();
    if target_bits < at {
        return Err(DecodeError::SizeFieldBehindCursor {
            at_byte: at / 8,
            target_byte,
        });
    }
    let mut remaining = target_bits - at;
    while remaining > 0 {
        let take = remaining.min(u64::from(super::bitreader::MAX_READ_BITS));
        r.skip(take as u32)?;
        remaining -= take;
    }
    Ok(())
}

/// One parsed MB record from the tile's header phase (inter walk).
struct MbRecord {
    header: MbHeader,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

/// How one block of a coded macroblock is signalled (`spec/03 §4.3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockCoding {
    /// CBP bit set — a `(run, val)` coefficient stream follows.
    Coded,
    /// CBP bit clear — DC-only / no coefficient stream.
    DcOnly,
    /// The whole MB carried the skip flag — no header fields either.
    Skipped,
}

/// One block of a walked tile, in band coordinates, carrying its
/// decoded scan-ordered coefficients (`spec/05` stream through the
/// band codebook + rv-table; `coeffs[pos]` is the value emitted at
/// scan position `pos`, `0..budget`).
#[derive(Debug, Clone)]
pub struct BlockRecord {
    /// Band-relative top-left x of the block.
    pub x: u32,
    /// Band-relative top-left y of the block.
    pub y: u32,
    /// Block side (4 or 8).
    pub blk_size: u32,
    /// Effective per-MB quantiser (`spec/06 §5.2`).
    pub quant: u8,
    /// How the block was signalled.
    pub coding: BlockCoding,
    /// Scan-position-ordered coefficient values (first
    /// `blk_size * blk_size` entries meaningful).
    pub coeffs: [i16; 64],
}

/// Decode one coded block's `(run, val)` coefficient stream
/// (`spec/05` + the wiki "Block data" annex): symbols through the
/// block codebook, `(run, val)` through the rv-table, `pos += run + 1`
/// scan advance from `pos = -1`, EOB-terminated. Every symbol carries
/// a non-zero level (the composite space has no level-0 entry — the
/// r451 "stuffing" reading is withdrawn, see [`super::RvTable`]);
/// placement and dequantisation happen in `reconstruct_intra_band`.
fn decode_block_stream(
    r: &mut BitReader<'_>,
    blk_cb: &Codebook,
    rv: &RvTable,
    budget: u32,
    stats: &mut DecodeStats,
    at: (usize, usize, usize),
    coeffs: &mut [i16; 64],
) -> Result<(), DecodeError> {
    let (plane_idx, band_idx, tile_idx) = at;
    let mut pos: i32 = -1;
    loop {
        let symbol = blk_cb.decode(r).map_err(DecodeError::Codebook)?;
        let entry = rv.lookup(symbol).ok_or(DecodeError::BlockStream {
            plane_idx,
            band_idx,
            tile_idx,
            reason: BlockStreamFault::UnmappedSymbol { symbol },
        })?;
        let (run, val) = match entry {
            RvEntry::Eob => return Ok(()),
            RvEntry::Esc => {
                stats.escapes += 1;
                // spec/05 §4.2 — three further VLCs: run, lindex_lo,
                // lindex_hi.
                let run = blk_cb.decode(r).map_err(DecodeError::Codebook)?;
                let lo = blk_cb.decode(r).map_err(DecodeError::Codebook)?;
                let hi = blk_cb.decode(r).map_err(DecodeError::Codebook)?;
                let val = escape_value(escape_lindex(lo, hi));
                (run.min(255) as u8, val)
            }
            RvEntry::Val { run, val } => (run, val),
        };
        pos = run_advance(pos, run);
        if pos >= budget as i32 {
            return Err(DecodeError::BlockStream {
                plane_idx,
                band_idx,
                tile_idx,
                reason: BlockStreamFault::PositionOverflow {
                    position: pos,
                    budget,
                },
            });
        }
        // The level stays quantised here; `reconstruct_intra_band`
        // reverses the quantiser by table once the block's raster
        // slot (and so its step) is known (spec/05 §2.3, spec/06 §5.4).
        coeffs[pos as usize] = val;
        stats.coefficients += 1;
    }
}

/// Walk one coded tile (intra): phase 1 parses every MB header, phase
/// 2 decodes every coded block's coefficient stream (the wiki
/// "Macroblocks info data" / "Blocks data" split). `inherited` holds
/// the qdeltas of the first luma band's MBs in the same tile, which a
/// band in `Inherit` mode takes. Returns the qdelta each MB used.
#[allow(clippy::too_many_arguments)]
fn walk_tile_intra(
    r: &mut BitReader<'_>,
    grid: &MbGrid,
    ctx: &MbContext,
    blk_cb: &Codebook,
    mb_cb: &Codebook,
    rv: &RvTable,
    level_table: &[i8; LEVEL_TABLE_LEN],
    blk_budget: u32,
    stats: &mut DecodeStats,
    at: (usize, usize, usize),
    tile: &super::tile::Tile,
    band_glob_quant: u8,
    inherited: Option<&[i8]>,
    sink: &mut dyn FnMut(BlockRecord),
) -> Result<Vec<i8>, DecodeError> {
    let (plane_idx, band_idx, tile_idx) = at;
    // Phase 1 — MB headers, from the byte boundary after the tile
    // header (spec/03 §2.1 / §4.1 byte-alignment; r459).
    r.align()?;
    let mut records: Vec<(MbHeader, super::mb::Macroblock)> = Vec::new();
    for mb in grid.iter() {
        let header =
            MbHeader::parse(r, ctx, mb_cb, level_table).map_err(|error| DecodeError::MbHeader {
                plane_idx,
                band_idx,
                tile_idx,
                error,
            })?;
        stats.mbs += 1;
        if header.skipped {
            stats.mbs_skipped += 1;
        }
        records.push((header, mb));
    }
    let qdeltas: Vec<i8> = records
        .iter()
        .enumerate()
        .map(|(i, (header, _))| match ctx.qdelta_mode {
            QdeltaMode::Inherit => inherited.and_then(|q| q.get(i)).copied().unwrap_or(0),
            _ => header.qdelta.unwrap_or(0),
        })
        .collect();
    // Phase 2 — coded-block coefficient streams, from the next byte
    // boundary (spec/03 §4.1: the block kernel is entered "at the next
    // byte alignment"; r459 fixture arbitration — see the module docs).
    r.align()?;
    for ((header, mb), &qdelta) in records.iter().zip(&qdeltas) {
        let quant = effective_mb_quant(band_glob_quant, qdelta);
        let blocks = grid.blocks(mb);
        if header.skipped {
            for blk in &blocks {
                sink(BlockRecord {
                    x: tile.x + blk.x,
                    y: tile.y + blk.y,
                    blk_size: grid.blk_size,
                    quant,
                    coding: BlockCoding::Skipped,
                    coeffs: [0i16; 64],
                });
            }
            continue;
        }
        let Some(cbp) = header.cbp else { continue };
        if cbp.coded_blocks(ctx.blocks_per_mb) == 0 {
            stats.mbs_coded_no_ac += 1;
        }
        for blk in &blocks {
            let mut coeffs = [0i16; 64];
            let coding = if cbp.block_coded(blk.block_idx) {
                stats.coded_blocks += 1;
                decode_block_stream(r, blk_cb, rv, blk_budget, stats, at, &mut coeffs)?;
                BlockCoding::Coded
            } else {
                BlockCoding::DcOnly
            };
            sink(BlockRecord {
                x: tile.x + blk.x,
                y: tile.y + blk.y,
                blk_size: grid.blk_size,
                quant,
                coding,
                coeffs,
            });
        }
    }
    Ok(qdeltas)
}

/// Walk one coded **inter** tile: the same two-phase layout with the
/// per-MB MV deltas in the header phase; motion-compensated predictor
/// copies apply per MB, then the coded blocks' coefficient streams
/// decode (their residual reconstruction rides the same transform
/// docs-gap as the intra path).
#[allow(clippy::too_many_arguments)]
fn walk_tile_inter(
    r: &mut BitReader<'_>,
    grid: &MbGrid,
    ctx: &MbContext,
    blk_cb: &Codebook,
    mb_cb: &Codebook,
    rv: &RvTable,
    level_table: &[i8; LEVEL_TABLE_LEN],
    blk_budget: u32,
    stats: &mut DecodeStats,
    at: (usize, usize, usize),
    tile: &super::tile::Tile,
    mv_res: MvResolution,
    work: &mut [i16],
    reference: &[i16],
    band_w: usize,
) -> Result<(), DecodeError> {
    let (plane_idx, band_idx, tile_idx) = at;

    // Phase 1 — MB headers (with MVs), from the byte boundary after
    // the tile header; spec/07 §3.3 zero-MV predictor reset at tile
    // entry.
    r.align()?;
    let mut predictor = MvPredictor::new();
    let mut records: Vec<MbRecord> = Vec::new();
    for mb in grid.iter() {
        let header =
            MbHeader::parse(r, ctx, mb_cb, level_table).map_err(|error| DecodeError::MbHeader {
                plane_idx,
                band_idx,
                tile_idx,
                error,
            })?;
        stats.mbs += 1;
        if header.skipped {
            stats.mbs_skipped += 1;
        }
        records.push(MbRecord {
            header,
            x: mb.x,
            y: mb.y,
            width: mb.width,
            height: mb.height,
        });
    }

    // Phase 1b — per-MB MC predictor application, in MB order.
    for rec in &records {
        let mv = if rec.header.skipped {
            // spec/07 §6.1 — skip inherits the left-neighbour MV.
            predictor.decode_mb(Mv::ZERO)
        } else {
            let delta = rec
                .header
                .mv_delta
                .map(|(dx, dy)| Mv { x: dx, y: dy })
                .unwrap_or(Mv::ZERO);
            predictor.decode_mb(delta)
        };
        if mv != Mv::ZERO {
            let resolved = resolve_mv(mv, mv_res);
            let dst_x = (tile.x + rec.x) as usize;
            let dst_y = (tile.y + rec.y) as usize;
            for row in 0..rec.height as usize {
                let base = (dst_y + row) * band_w + dst_x;
                work[base..base + rec.width as usize].fill(0);
            }
            mc_add_block(
                work,
                band_w,
                dst_x,
                dst_y,
                BandView {
                    data: reference,
                    stride: band_w,
                },
                dst_x as i32 + resolved.dx,
                dst_y as i32 + resolved.dy,
                rec.width as usize,
                rec.height as usize,
                resolved.mode,
            )
            .map_err(|error| DecodeError::Mc {
                plane_idx,
                band_idx,
                tile_idx,
                error,
            })?;
        }
    }

    // Phase 2 — coded-block coefficient streams, byte-aligned.
    r.align()?;
    for rec in &records {
        if rec.header.skipped {
            continue;
        }
        let Some(cbp) = rec.header.cbp else { continue };
        if cbp.coded_blocks(ctx.blocks_per_mb) == 0 {
            stats.mbs_coded_no_ac += 1;
            continue;
        }
        for b in 0..ctx.blocks_per_mb {
            if !cbp.block_coded(b) {
                continue;
            }
            stats.coded_blocks += 1;
            let mut coeffs = [0i16; 64];
            decode_block_stream(r, blk_cb, rv, blk_budget, stats, at, &mut coeffs)?;
        }
    }
    Ok(())
}

/// Crop a recomposed wavelet plane into a `spec/08 §1.1`
/// stride-padded reconstruction plane.
fn to_reconstruction_plane(
    band: &Band,
    w: u32,
    h: u32,
) -> Result<ReconstructionPlane, OutputError> {
    let stride = plane_stride(w);
    let mut data = vec![0i32; (stride * h) as usize];
    for y in 0..h {
        for x in 0..w {
            data[(y * stride + x) as usize] = band.at(x as usize, y as usize);
        }
    }
    ReconstructionPlane::new(w, h, stride, data)
}

/// Resolve a band's effective transform (`spec/02 §1.7`): explicit
/// `ext_trans` wins; the `Standard` fallback maps the band's
/// frequency position — LL → 2D Slant, HL → row Slant, LH → column
/// Slant, HH → no transform (`spec/06 §1.1` wiki-annex confirmation).
fn resolve_transform(binfo: &BandInfo, band_idx: usize) -> TransformId {
    match binfo.transform_id {
        TransformId::Standard => {
            if band_idx == 0 {
                TransformId::Slant2d
            } else {
                match (band_idx - 1) % 3 {
                    0 => TransformId::SlantRow,
                    1 => TransformId::SlantColumn,
                    _ => TransformId::None,
                }
            }
        }
        explicit => explicit,
    }
}

/// The scan table a transform variant pairs with (`spec/06 §2.5`):
/// the 2D Slant scans the diagonal zig-zag; a 1D variant scans its
/// no-transform axis first (column-major for the row Slant,
/// row-major for the column Slant); the no-transform variant is the
/// identity raster scan.
fn scan_for_transform(transform: TransformId) -> &'static [u8; 64] {
    match transform {
        TransformId::Slant2d => &SCAN_ZIGZAG_8X8,
        TransformId::SlantRow => &SCAN_COLUMN_8X8,
        TransformId::SlantColumn | TransformId::None | TransformId::Standard => &SCAN_RASTER_8X8,
    }
}

/// Reconstruct one intra band's sample buffer from its decoded block
/// work list (`spec/06 §2`): per block, place the scan-ordered
/// coefficients into the raster grid (`spec/06 §2.5`), carry the
/// running intra DC predictor (the fixture-arbitrated inheritance
/// that lets a single coded block set a whole flat plane), apply the
/// band's inverse-Slant variant, and store the spatial samples into
/// the band buffer (clipped at the band edge).
///
/// Returns a gate when the band's block size needs the unstaged
/// 4-point kernel (`spec/06 §2.3`).
fn reconstruct_intra_band(
    band: &mut Band,
    records: &[BlockRecord],
    binfo: &BandInfo,
    band_idx: usize,
    plane_bands: usize,
) {
    let transform = resolve_transform(binfo, band_idx);
    let mut bq = BandQuant::new(binfo.blk_size, band_idx, plane_bands);
    // Only a band under the 2D Slant carries the DC chain below. In the
    // other bands of a wavelet-split plane (the 1D Slants and the band
    // without a transform) scan position 0 is an ordinary coefficient,
    // and a block without a coefficient stream is all zeros.
    let dc_predicted = matches!(transform, TransformId::Slant2d);
    // Intra DC chain (the Indeo 4 annex-B "DC coded using differential
    // coding", shared by Indeo 5): scan position 0 of a coded block
    // carries a *delta* against the running DC of the previous block
    // in decode order (dequantised at the DC step), and blocks without
    // a coefficient stream repeat the running DC. The chain starts at
    // zero: the first block of a band carries its DC as a large
    // (typically escape-coded) delta — `lidx` 216 / 42 / 257 on the
    // 320×240 fixture's three bands, 448 on the flat fixture's single
    // coded block, which is its block (0,0). The 320×240 fixture pins
    // the predictor on 658 / 658 coded luma blocks (r459: previous
    // block in decode order; the left- and top-neighbour readings fit
    // 469 and 367).
    let mut dc_chain: i16 = 0;

    for rec in records {
        let side = rec.blk_size as usize;
        let mut block = [0i16; 64];
        let n = side * side;
        if matches!(rec.coding, BlockCoding::Coded) {
            block = match (side, transform) {
                (8, _) => place_scan_8x8(&rec.coeffs, scan_for_transform(transform)),
                (_, TransformId::None) => {
                    let b = place_scan_4x4(&rec.coeffs, &SCAN_RASTER_4X4);
                    let mut wide = [0i16; 64];
                    wide[..16].copy_from_slice(&b);
                    wide
                }
                _ => {
                    let b = place_scan_4x4(&rec.coeffs, &SCAN_ZIGZAG_4X4);
                    let mut wide = [0i16; 64];
                    wide[..16].copy_from_slice(&b);
                    wide
                }
            };
            // spec/05 §2.3 / spec/06 §5.4 — quantisation reversal by
            // table: every placed level becomes ±r(|level|, b) with
            // the step b = bank[g][c][q_mb][raster slot].
            let steps = bq.steps(rec.quant);
            if side == 8 {
                for (slot, level) in block.iter_mut().enumerate() {
                    *level = dequant_level(*level, steps[slot]);
                }
            } else {
                // The 4×4 group's matrices sit on 8-byte rows
                // (`tables/quant_matrices_1007b000` final form), so a
                // 4-stride raster slot addresses row `slot >> 2` at
                // stride 8. (Both fixtures' chroma bands verify their
                // stored checksums under this and under a 4-stride
                // read; the pixel oracle does not reach chroma yet.)
                for (slot, level) in block.iter_mut().enumerate().take(16) {
                    let idx = (slot >> 2) * 8 + (slot & 3);
                    *level = dequant_level(*level, steps[idx]);
                }
            }
        }
        if dc_predicted {
            match rec.coding {
                BlockCoding::Coded => {
                    dc_chain = dc_chain.wrapping_add(block[0]);
                    block[0] = dc_chain;
                }
                BlockCoding::DcOnly | BlockCoding::Skipped => {
                    block[0] = dc_chain;
                }
            }
        }

        if side == 8 {
            let full: &mut [i16; 64] = &mut block;
            match transform {
                TransformId::Slant2d => inverse_slant_2d_8x8(full),
                TransformId::SlantRow => inverse_slant_row_8x8(full),
                TransformId::SlantColumn => inverse_slant_col_8x8(full),
                TransformId::None | TransformId::Standard => {}
            }
        } else {
            let mut quarter = [0i16; 16];
            quarter.copy_from_slice(&block[..16]);
            match transform {
                TransformId::Slant2d => inverse_slant_2d_4x4(&mut quarter),
                TransformId::SlantRow => inverse_slant_row_4x4(&mut quarter),
                TransformId::SlantColumn => inverse_slant_col_4x4(&mut quarter),
                TransformId::None | TransformId::Standard => {}
            }
            block[..16].copy_from_slice(&quarter);
        }
        let _ = n;

        let bw = band.width;
        let bh = band.height;
        for row in 0..side {
            let y = rec.y as usize + row;
            if y >= bh {
                break;
            }
            for col in 0..side {
                let x = rec.x as usize + col;
                if x >= bw {
                    break;
                }
                band.data[y * bw + x] = i32::from(block[side * row + col]);
            }
        }
    }
}

/// The decoded payload of one picture-carrying frame: the per-plane
/// band coefficient buffers (the `spec/07 §1.2` reference workspace
/// for the next frame) plus the recomposed reconstruction planes.
#[derive(Debug, Clone)]
pub(crate) struct PayloadOutcome {
    pub bands: Vec<Vec<Band>>,
    pub recon: Vec<ReconstructionPlane>,
    pub frontiers: Vec<DecodeFrontier>,
    pub stats: DecodeStats,
    pub band_traces: Vec<BandTrace>,
    pub parse_complete: bool,
    pub blocks: Vec<BandBlockSet>,
}

/// All walked blocks of one band, in decode order (the reconstruction
/// work list the coefficient stage hands the transform stage).
#[derive(Debug, Clone)]
pub(crate) struct BandBlockSet {
    pub plane_idx: usize,
    pub band_idx: usize,
    pub glob_quant: u8,
    pub records: Vec<BlockRecord>,
    pub stored_checksum: Option<u16>,
    /// The band checksum of a band of a wavelet-split plane, taken over
    /// its own samples before recomposition; `None` for the single band
    /// of a plane without a split, whose checksum is over its pixels.
    pub band_sum: Option<u16>,
}

/// Decode one INTRA frame to pixels.
///
/// Threads the `spec/02 §4.4` per-plane / per-band / per-tile walk
/// over the structural layers, decodes the per-block coefficient
/// streams, recomposes each plane's bands (`spec/06 §3`), and
/// assembles the `spec/08` host buffer. For multi-frame sequences
/// (INTER / NULL-repeat) use [`super::Indeo5Decoder`].
pub fn decode_intra_picture(bitstream: &[u8]) -> Result<DecodedPicture, DecodeError> {
    let (header, mut r) = PictureHeader::parse_with_reader(bitstream, None)?;

    // NULL frame: repeat-previous, no coded payload (spec/08 §6.4).
    if header.is_null() {
        return Ok(DecodedPicture {
            header,
            format: None,
            output: None,
            frontiers: Vec::new(),
            stats: DecodeStats::default(),
            band_traces: Vec::new(),
            bands: Vec::new(),
            frame_checksum: super::ChecksumStatus::Absent,
            parse_complete: true,
        });
    }
    if header.frame_type() != FrameType::Intra {
        return Err(DecodeError::NotIntra {
            found: header.frame_type(),
        });
    }

    let gop = header.gop.as_ref().expect("INTRA frame carries a GOP");
    let frame = header
        .frame
        .as_ref()
        .expect("INTRA frame carries a frame header");

    let payload = decode_payload(&mut r, gop, frame, None)?;
    let format = output_format(gop);
    let output = assemble_frame(
        &payload.recon[0],
        &payload.recon[1],
        &payload.recon[2],
        format,
    )?;

    let (bands, frame_ck) = reconstruction_checksums(&output, frame.frm_checksum, &payload.blocks);

    Ok(DecodedPicture {
        header,
        format: Some(format),
        output: Some(output),
        frontiers: payload.frontiers,
        stats: payload.stats,
        band_traces: payload.band_traces,
        bands,
        frame_checksum: frame_ck,
        parse_complete: payload.parse_complete,
    })
}

/// `spec/08 §7` reconstruction oracle: recompute the per-band and
/// per-frame checksums from the assembled pixels and compare them
/// against the stream's stored values (formulas recovered by black-box
/// validation; see [`super::verify`]). A luma band stays
/// [`super::ChecksumStatus::Mismatch`] while its coefficient→pixel
/// transform is gated; a genuinely-flat chroma band matches exactly.
pub(crate) fn reconstruction_checksums(
    output: &HostBuffer,
    stored_frame_checksum: Option<u16>,
    blocks: &[BandBlockSet],
) -> (Vec<BandReconstruction>, super::ChecksumStatus) {
    let luma = output.plane_bytes(super::PlaneRole::Luma);
    let cu = output.plane_bytes(super::PlaneRole::ChromaU);
    let cv = output.plane_bytes(super::PlaneRole::ChromaV);
    let frame_ck =
        super::ChecksumStatus::compare(stored_frame_checksum, super::frame_checksum(luma, cu, cv));
    let bands = blocks
        .iter()
        .map(|set| {
            // For a 0-level plane the band is the plane, so its
            // reconstructed pixels are the whole plane's bytes; the
            // band checksum is over `pixel - 128`. The chroma chains
            // are coded V first, then U (`YVU9` order; r459 — the
            // vendor's packed `YUY2` output carries the first chain's
            // samples in its V byte).
            let pixels = match set.plane_idx {
                0 => luma,
                1 => cv,
                _ => cu,
            };
            let computed = set.band_sum.unwrap_or_else(|| super::band_checksum(pixels));
            let checksum = super::ChecksumStatus::compare(set.stored_checksum, computed);
            BandReconstruction {
                plane_idx: set.plane_idx,
                band_idx: set.band_idx,
                glob_quant: set.glob_quant,
                blocks: set.records.clone(),
                stored_checksum: set.stored_checksum,
                checksum,
            }
        })
        .collect();
    (bands, frame_ck)
}

/// The host output format the GOP subsampling selects
/// (`spec/08 §2.2`).
pub(crate) fn output_format(gop: &GopHeader) -> OutputFormat {
    match gop.flags.subsampling() {
        Subsampling::Yvu9 => OutputFormat::Yvu9,
        Subsampling::Yv12 => OutputFormat::Yv12,
    }
}

/// Decode a picture-carrying frame's per-band payload and recompose
/// its planes (`spec/02 §4.4` walk). `reference` is `None` for an
/// INTRA frame (zero seeds) and the previous frame's band buffers for
/// an INTER frame (`spec/07 §4.4` band-coefficient-layer prediction).
pub(crate) fn decode_payload(
    r: &mut BitReader<'_>,
    gop: &GopHeader,
    frame: &FrameHeader,
    reference: Option<&[Vec<Band>]>,
) -> Result<PayloadOutcome, DecodeError> {
    if u64::from(gop.width) * u64::from(gop.height) > MAX_PICTURE_PIXELS {
        return Err(DecodeError::PictureTooLarge {
            width: gop.width,
            height: gop.height,
        });
    }
    let band_size_present = frame.flags.band_data_size_present();
    let slice_size = gop.slice_size();
    let level_table = build_level_table();
    // The frame-level MB-Huffman codebook (qdelta / MV VLCs).
    let mb_cb = Codebook::from_huff_desc(HuffContext::Mb, frame.mb_huff_desc.as_ref())?;

    let planes_geom = [
        PlaneGeom {
            width: gop.width,
            height: gop.height,
            levels: gop.decomp.luma_levels,
            band_info: &gop.luma_band_info,
        },
        PlaneGeom {
            width: gop.chroma_width,
            height: gop.chroma_height,
            levels: gop.decomp.chroma_levels,
            band_info: &gop.chroma_band_info,
        },
        PlaneGeom {
            width: gop.chroma_width,
            height: gop.chroma_height,
            levels: gop.decomp.chroma_levels,
            band_info: &gop.chroma_band_info,
        },
    ];

    let mut stats = DecodeStats::default();
    let mut frontiers: Vec<DecodeFrontier> = Vec::new();
    let mut band_traces: Vec<BandTrace> = Vec::new();
    let mut parse_complete = true;
    let mut recon_planes: Vec<ReconstructionPlane> = Vec::with_capacity(3);
    let mut all_bands: Vec<Vec<Band>> = Vec::with_capacity(3);
    let mut all_blocks: Vec<BandBlockSet> = Vec::new();
    // The per-MB qdeltas of the first luma band, tile by tile: a later
    // band in `Inherit` mode takes the qdelta of the MB at the same
    // place in the same tile (every band shares that band's MB grid).
    let mut ref_qdeltas: Vec<Vec<i8>> = Vec::new();

    for (plane_idx, geom) in planes_geom.iter().enumerate() {
        // Seed every band: zeros for INTRA; the previous frame's band
        // content for INTER (spec/07 §4.4).
        let mut bands: Vec<Band> = (0..geom.band_info.len())
            .map(|band_idx| {
                let (bw, bh) = band_dims(geom.width, geom.height, geom.levels, band_idx);
                match reference {
                    Some(prev) => prev
                        .get(plane_idx)
                        .and_then(|p| p.get(band_idx))
                        .filter(|b| b.width == bw as usize && b.height == bh as usize)
                        .cloned()
                        .ok_or(DecodeError::ReferenceMismatch {
                            plane_idx,
                            band_idx,
                        }),
                    None => Ok(Band::new(
                        bw as usize,
                        bh as usize,
                        vec![0i32; (bw * bh) as usize],
                    )),
                }
            })
            .collect::<Result<_, _>>()?;

        for (band_idx, binfo) in geom.band_info.iter().enumerate() {
            if !parse_complete {
                break;
            }
            r.align()?;
            let band_start = r.byte_pos();
            let band_header = BandHeader::parse(&mut *r, band_size_present).map_err(|error| {
                DecodeError::Band {
                    plane_idx,
                    band_idx,
                    error,
                }
            })?;
            stats.bands += 1;
            if band_header.empty {
                stats.empty_bands += 1;
                continue;
            }
            let band_end = band_header
                .band_data_size
                .map(|s| band_start + u64::from(s));
            // Per-band entropy state: block codebook + rv-table.
            let blk_cb =
                Codebook::from_huff_desc(HuffContext::Block, band_header.blk_huff_desc.as_ref())?;
            let rv = RvTable::for_band(band_header.rv_tab_sel, &band_header.rv_tab_corr)?;
            let (bw, bh) = band_dims(geom.width, geom.height, geom.levels, band_idx);
            let (count_x, count_y) = band_tile_counts(bw, bh, slice_size);
            let tile_grid = TileGrid::build(bw, bh, count_x, count_y);

            let mut band_blocks: Vec<BlockRecord> = Vec::new();
            let qdelta_mode = QdeltaMode::from_band_flags(
                band_header.flags.qdelta_present(),
                band_header.flags.qdelta_inherit(),
            );
            let inter = reference.is_some();
            let explicit_mv = inter && !band_header.flags.mv_inherit();
            let mv_inherit_gated = inter && band_header.flags.mv_inherit();
            let mv_res = if binfo.mv_halfpel {
                MvResolution::HalfPel
            } else {
                MvResolution::FullPel
            };
            let blk_budget = binfo.blk_size * binfo.blk_size;

            let mut work: Vec<i16> = if inter {
                bands[band_idx].data.iter().map(|&v| v as i16).collect()
            } else {
                Vec::new()
            };
            let ref_i16: Vec<i16> = if inter { work.clone() } else { Vec::new() };

            r.align()?;
            let mut band_aborted = false;
            let mut tile_idx = 0usize;
            'tiles: for row in 0..count_y {
                for col in 0..count_x {
                    let tile = tile_grid.tile(col, row).expect("in-range tile");
                    r.align()?;
                    let tile_start = r.byte_pos();
                    let th = TileHeader::parse(&mut *r, false)?;
                    stats.tiles += 1;

                    match th.size {
                        TileDataSize::Empty => {
                            stats.empty_tiles += 1;
                        }
                        TileDataSize::Implicit | TileDataSize::Explicit(_) => {
                            let grid = MbGrid::build(
                                tile.width,
                                tile.height,
                                binfo.mb_size,
                                binfo.blk_size,
                            );
                            let ctx = MbContext {
                                qdelta_mode,
                                explicit_mv,
                                blocks_per_mb: grid.blocks_per_mb(),
                                qdelta_always: plane_idx == 0
                                    && band_idx == 0
                                    && frame.flags.flag_b3(),
                            };
                            let frontier = if mv_inherit_gated {
                                Some(FrontierReason::MvInheritance)
                            } else {
                                if inter {
                                    walk_tile_inter(
                                        &mut *r,
                                        &grid,
                                        &ctx,
                                        &blk_cb,
                                        &mb_cb,
                                        &rv,
                                        &level_table,
                                        blk_budget,
                                        &mut stats,
                                        (plane_idx, band_idx, tile_idx),
                                        tile,
                                        mv_res,
                                        &mut work,
                                        &ref_i16,
                                        bw as usize,
                                    )?;
                                } else {
                                    let inherited = if plane_idx == 0 && band_idx == 0 {
                                        None
                                    } else {
                                        ref_qdeltas.get(tile_idx).map(Vec::as_slice)
                                    };
                                    let qdeltas = walk_tile_intra(
                                        &mut *r,
                                        &grid,
                                        &ctx,
                                        &blk_cb,
                                        &mb_cb,
                                        &rv,
                                        &level_table,
                                        blk_budget,
                                        &mut stats,
                                        (plane_idx, band_idx, tile_idx),
                                        tile,
                                        band_header.band_glob_quant.unwrap_or(0),
                                        inherited,
                                        &mut |rec| band_blocks.push(rec),
                                    )?;
                                    if plane_idx == 0 && band_idx == 0 {
                                        if ref_qdeltas.len() <= tile_idx {
                                            ref_qdeltas.resize(tile_idx + 1, Vec::new());
                                        }
                                        ref_qdeltas[tile_idx] = qdeltas;
                                    }
                                }
                                None
                            };
                            r.align()?;

                            if let Some(reason) = frontier {
                                let skipped_past = match th.size {
                                    TileDataSize::Explicit(n) => {
                                        // §2.8 (behaviourally
                                        // confirmed): the count spans
                                        // the whole tile from its
                                        // first byte.
                                        skip_to_byte(&mut *r, tile_start + u64::from(n))?;
                                        true
                                    }
                                    TileDataSize::Implicit => {
                                        if let Some(end) = band_end {
                                            skip_to_byte(&mut *r, end)?;
                                            band_aborted = true;
                                            true
                                        } else {
                                            parse_complete = false;
                                            false
                                        }
                                    }
                                    TileDataSize::Empty => unreachable!(),
                                };
                                frontiers.push(DecodeFrontier {
                                    plane_idx,
                                    band_idx,
                                    tile_idx,
                                    reason,
                                    skipped_past,
                                });
                                if band_aborted || !parse_complete {
                                    break 'tiles;
                                }
                            } else if let TileDataSize::Explicit(n) = th.size {
                                // §2.8 reconciliation: pad-skip when
                                // the walk consumed less than the
                                // whole-tile count (trailing encoder
                                // padding); behind-cursor is the §2.8
                                // error return.
                                skip_to_byte(&mut *r, tile_start + u64::from(n))?;
                            }
                        }
                    }
                    tile_idx += 1;
                }
            }

            // Band exit: record the consumption trace, then reconcile
            // against band_data_size when known.
            if parse_complete {
                band_traces.push(BandTrace {
                    plane_idx,
                    band_idx,
                    consumed: r.byte_pos() - band_start,
                    declared: band_header.band_data_size,
                });
            }
            if parse_complete
                && !band_aborted
                && let Some(end) = band_end
            {
                r.align()?;
                if r.byte_pos() < end {
                    skip_to_byte(&mut *r, end)?;
                }
            }
            if inter {
                bands[band_idx] = Band::new(
                    bw as usize,
                    bh as usize,
                    work.iter().map(|&v| v as i32).collect(),
                );
            } else {
                reconstruct_intra_band(
                    &mut bands[band_idx],
                    &band_blocks,
                    binfo,
                    band_idx,
                    geom.band_info.len(),
                );
            }
            // A band of a wavelet-split plane checksums its own samples:
            // their sum, wrapped to 16 bits, as the plane's single band
            // sums its pixels less the bias.
            let band_sum = (geom.band_info.len() > 1).then(|| {
                let sum = bands[band_idx]
                    .data
                    .iter()
                    .fold(0i32, |a, &v| a.wrapping_add(v));
                sum as u16
            });
            all_blocks.push(BandBlockSet {
                plane_idx,
                band_idx,
                glob_quant: band_header.band_glob_quant.unwrap_or(0),
                records: band_blocks,
                stored_checksum: band_header.band_checksum,
                band_sum,
            });
        }

        // spec/06 §3.4 — bottom-up recompose: bands[0] is the
        // innermost LL; each level's (HL, LH, HH) triple follows.
        let levels: Vec<LevelBands> = (0..geom.levels as usize)
            .map(|l| LevelBands {
                hl: bands[1 + 3 * l].clone(),
                lh: bands[2 + 3 * l].clone(),
                hh: bands[3 + 3 * l].clone(),
            })
            .collect();
        let plane = recompose_plane(&bands[0], &levels);
        recon_planes.push(to_reconstruction_plane(&plane, geom.width, geom.height)?);
        all_bands.push(bands);
    }

    Ok(PayloadOutcome {
        bands: all_bands,
        recon: recon_planes,
        frontiers,
        stats,
        band_traces,
        parse_complete,
        blocks: all_blocks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_dims_per_level() {
        // 0-level plane: one band at plane resolution.
        assert_eq!(band_dims(352, 288, 0, 0), (352, 288));
        // 1-level: LL + HF triple all at half resolution.
        assert_eq!(band_dims(352, 288, 1, 0), (176, 144));
        assert_eq!(band_dims(352, 288, 1, 1), (176, 144));
        assert_eq!(band_dims(352, 288, 1, 3), (176, 144));
        // 2-level: LL + inner triple at quarter, outer triple at half.
        assert_eq!(band_dims(352, 288, 2, 0), (88, 72));
        assert_eq!(band_dims(352, 288, 2, 2), (88, 72));
        assert_eq!(band_dims(352, 288, 2, 4), (176, 144));
        assert_eq!(band_dims(352, 288, 2, 6), (176, 144));
    }

    #[test]
    fn tile_counts_from_slice() {
        // No slice -> whole band is one tile.
        assert_eq!(band_tile_counts(176, 144, None), (1, 1));
        // spec/03 §1.1: 176x144 band at 64-px slices -> 3x3.
        assert_eq!(band_tile_counts(176, 144, Some(64)), (3, 3));
        // Band smaller than the slice -> one tile.
        assert_eq!(band_tile_counts(44, 36, Some(64)), (1, 1));
    }
}
