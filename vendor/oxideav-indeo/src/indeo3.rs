//! Indeo 3 (IV31 / IV32) — structural decoders.
//!
//! Round 1 lands `FrameHeader::parse` (`spec/01`). Round 2 adds
//! `PictureLayer::parse` (`spec/02`). Round 3 adds
//! `decode_plane_tree` (`spec/03`), the binary-tree walk over a
//! plane's bitstream payload that produces a typed [`CellTree`] of
//! INTRA / INTER leaf cells (the INTRA cells carry their VQ
//! sub-tree leaves inline). Round 4 adds the VQ codebook
//! materialisation (`spec/04`): the static dyad-mode delta table
//! ([`DyadDeltaTable`]), the packed-codebook-DWORD decode
//! ([`CodebookEntry`]), the static codebook seed-dispatch table
//! ([`seed_dispatch_entries`]), the per-frame arena + `alt_quant[]`
//! overlay ([`VqArena`]), and the VQ_NULL runtime sub-codes
//! ([`VqNullRuntime`]). Round 5 adds the byte-level entropy module
//! (`spec/06`): the per-cell mode-byte stream classifier
//! ([`ModeByte`]), the variable-byte continuation rule
//! ([`continuation_needed`]), the eight RLE escapes ([`RleEscape`])
//! with their per-position acceptance matrix
//! ([`RleEscape::accepted_at`]), and the `0xFB` counter-byte category
//! table ([`fb_category_table`]). Round 6 adds the output-
//! reconstruction kernel (`spec/07` §1 + §2 + §4): the predictor
//! address ([`predictor_offset`]) and the softSIMD dyad-pair
//! `predictor + delta` add ([`apply_dyad_pair`]) with its
//! continuation / secondary-table fall-back and 7-bit-per-byte
//! overflow detection. Round 7 adds the four cell-shape variant
//! inner-loop emission kernels ([`emit_variant`], `spec/07` §2.2):
//! variant A's direct two-row store, variant B's `0x7f7f7f7f`-clamped
//! per-byte average ([`average_7bit`]), variant C's doubled-row
//! average, and variant D's `and 0xfefefefe; shr 1` halve
//! ([`halve_fefefefe`]). Round 8 adds the spec/02 §4–§7
//! picture-decomposition state ([`strip_slot_index`],
//! [`StripSlotDescriptor`], [`StripGeometry`], [`PerPlaneDecodeCall`],
//! [`PlaneDecodeStatus`], [`luma_strip_slot_count`],
//! [`chroma_strip_slot_count`], [`chroma_plane_height`]) — the
//! strip-context array layout (§5), the per-plane decode-call
//! signature (§6), the codec-init strip-count arithmetic (§7), and
//! the §4.1 / §4.2 strip-geometry formulae. Round 9 adds the
//! outer per-cell row/column loop preamble (`spec/04` §3.3):
//! [`dispatch_cell_preamble`] runs the four-step
//! `IR32_32.DLL!0x1000665e..0x10006670` sequence — pick the
//! [`CodebookBankView`] (primary vs `+0xb00` mirror) from the
//! cell-stack top, load the cell-position DWORD with the `0xf423f`
//! sanity check ([`CELL_POSITION_MAX`]), load the new `cl` row
//! counter, and clear the intra-context flag — returning a
//! [`CellLoopState`] that bridges round 4's [`CodebookEntry`] to
//! round 7's [`emit_variant`]; [`advance_row`] /
//! [`iterate_column_rows`] step the `(cl, edi)` walk across a
//! cell's rows. Round 10 adds the per-cell sub-array wiring
//! (`spec/03` §5.1 / §5.3 / §5.5) — the cell-stack at
//! `[strip_slot + 0x40+]`: [`cell_stack_slot_offset`] /
//! [`cell_stack_array_offset`] enforce the §5 240-entry bound,
//! [`CellStackReadSite`] enumerates the three §5.3 read sites
//! within `IR32_32.DLL!0x10006538`, and [`CellStackTopDispatch`]
//! classifies the destination-slot stack-top load into the §5.4
//! strip-edge vs §5.5 inter-cell branch (with §5.5's
//! [`PER_CELL_EDGE_PREV_BR_OFFSET`] / [`PER_CELL_EDGE_PREV_BR_NEXT_OFFSET`]
//! / [`PER_CELL_EDGE_ROW_STRIDE`] / [`PER_CELL_EDGE_HEIGHT_STEP`]
//! constants surfaced). Round 11 adds the spec/03 §5.4 end-of-strip
//! edge fix-up parameter surface — [`StripEdgeFixupDims::for_slot`]
//! resolves the per-plane-role `sar 2` chroma divide, and
//! [`StripEdgeRowIter`] yields the per-row read/write byte-offsets
//! ([`STRIP_EDGE_BYTE_READ_OFFSET`] / [`STRIP_EDGE_BYTE_WRITE_OFFSET`])
//! the rightmost-column duplication walks. Round 12 adds the spec/05
//! §1 per-plane packed-MV table layout: [`MV_TABLE_BASE_OFFSET`] /
//! [`MV_TABLE_ENTRY_SIZE`] / [`MV_TABLE_BYTES`] /
//! [`MV_TABLE_MAX_BYTE_INDEXABLE_ENTRIES`] fix the §1.2 arena geometry,
//! [`MvTableParserArm::from_frame_flags`] resolves the §1.2 four-way
//! parser-arm dispatch on `frame_flags` bits 4 + 5 with the four
//! write-site RVAs surfaced, [`mv_table_entry_byte_offset`] /
//! [`MvIndexFetch::for_index`] model the §1.3
//! `xor eax,eax; mov al,[ebp]; shl eax,0x2; add eax,inner_instance`
//! INTER-leaf sequence up to (but not including) the table dereference,
//! and [`MvIndexValidity`] classifies an MV-index byte against the
//! plane's `num_vectors` per §1.4
//! (written-this-frame / stale-tail-entry / out-of-range). Round 13
//! adds the spec/05 §2.2 / §2.3 / §3.3 / §3.4 packed-MV bit-layout
//! decode and four-way MC dispatch: [`PackedMv::from_raw`] wraps the
//! 32-bit packed-MV DWORD fetched from `inner_instance[4*i]`,
//! [`PackedMv::pixel_offset`] recovers the §2.3 / §3.4 signed
//! strip-pixel byte offset via the dispatcher's `sar edx, 0x2`
//! ([`MV_PIXEL_OFFSET_SHIFT`] = `2`), [`PackedMv::mode`] returns
//! [`McDispatchMode`] — the §2.2 four-way fork (`FullPel` /
//! `VerticalHalfPel` / `HorizontalHalfPel` / `BothHalfPel`) selected
//! by [`MV_MODE_BITS_MASK`] (`0x3`) with each variant carrying its
//! inner-loop RVA (`0x1000670d` / `0x10006780` / `0x1000684b` /
//! `0x100068f8`); [`apply_mv_source_offset`] /
//! [`PackedMv::source_address`] model the §2.3
//! `src_addr = dst_cell_base + sign_extend(packed_MV >> 2)`, and
//! [`pack_mv_components`] surfaces the §3.3 constructive packer
//! `((176*vert + horiz) << 2) | (horiz_lsb << 1) | vert_lsb`. The
//! §3.3 row-stride constant [`MV_PIXEL_OFFSET_ROW_STRIDE`] (`176`)
//! aliases [`reconstruct::PREDICTOR_ROW_STRIDE`] with a `const _`
//! cross-check. Round 14 adds the spec/05 §5.1 / §5.2 / §5.3 MC
//! cell-copy inner-loop kernel: [`MC_ROW_STRIDE`] (`0xb0`) /
//! [`MC_INNER_LOOP_DWORDS_PER_ITER`] (`4`) /
//! [`MC_INNER_LOOP_BYTES_PER_ITER`] (`16`) / [`MC_BAND_ROWS`] (`4`) /
//! [`MC_BAND_BYTE_STRIDE`] (`0x2c0`) / [`MC_COLUMN_GROUP_PIXELS`]
//! (`4`) pin the §5.1 inner-loop shape; the
//! [`MC_FULL_PEL_ROW_OFFSETS`] table mirrors the four `mov [esi +
//! 0]`, `[esi + 0xb0]`, `[esi + 0x160]`, `[esi + 0x210]` immediates
//! the full-pel kernel at `IR32_32.DLL!0x1000670d..0x1000673d`
//! emits; [`mc_full_pel_row_dword`] / [`McKernelStep::for_row`]
//! expose the same offsets through a typed surface.
//! [`McKernelGeometry::new`] enforces the §5.1 multiple-of-4
//! width / height invariants and the §5.3 row-stride bound
//! ([`MC_MAX_CELL_WIDTH_BYTES`] = `0xb0`). The §5.2 per-DWORD
//! averaging kernels — [`mc_vert_half_pel_pair`]
//! ([`MC_VERT_HALF_PEL_NEIGHBOUR_OFFSET`] = `0xb0`),
//! [`mc_horiz_half_pel_pair`] ([`MC_HORIZ_HALF_PEL_NEIGHBOUR_OFFSET`]
//! = `1`, with the in-DWORD byte splice for the `[esi]` /
//! `[esi + 1]` neighbour pair), and [`mc_both_half_pel_quad`]
//! (the §2.2 2×2 box filter, composed as horizontal-pair-first /
//! vertical-pair-second) — share the §2.2 / §5.2 byte-parallel
//! `(a + b) >> 1` SWAR identity with the output-reconstruction
//! kernel's [`reconstruct::average_7bit`], confirming the
//! "no separate filter coefficient tables" §2.2 disposition.
//! Round 15 adds the spec/05 §5.4 / §7.2 cell-position decoding
//! entry — the cell-state dispatcher's index-arithmetic chain that
//! resolves the per-cell destination and source pixel-buffer
//! addresses the round-14 MC fetcher's inner loop consumes:
//! [`CELL_SLOT_STRIDE`] (`16`, the §7.2 / §4.3 `shl eax, 0x4` at
//! `IR32_32.DLL!0x10006615`); [`CELL_SLOT_INDEX_MAX`] (`15`, the
//! §7.2 "cell-slot index 0..15" upper bound); [`CellSlotBase`] /
//! [`CellSlotBase::from_bank_byte`] surface the post-`shl 0x4`
//! base index; [`CellSubarrayIndex::dst`] / [`CellSubarrayIndex::src`]
//! compose `idx_dst = 16 * cell_slot + dst_slot` /
//! `idx_src = 16 * cell_slot + src_slot` (the §7.2 / §4.3
//! per-cell sub-array element indices loaded at
//! `IR32_32.DLL!0x10006638..0x10006641`); [`CellAddrEntry::dst`] /
//! [`CellAddrEntry::src`] hold the destination / source cell-data
//! DWORDs tagged with their [`CellAddrRole`] (`Dest` /
//! `Src`) and carry the §7.2 `[esp+0x38]` extra-offset companion on
//! the source-role branch; [`mc_dest_address`] composes
//! `dst_addr = dst_cell_data + cell_pos_aux`, and
//! [`mc_source_address`] composes
//! `src_addr = src_cell_data + cell_pos_aux + sign_extend(packed_MV >> 2)`
//! by chaining the §5.4 cell-base add with the §2.3
//! [`apply_mv_source_offset`] sign-extending MV displacement.
//! [`McCellAddressPair::resolve`] runs the complete §7.2 chain in
//! one entry point and returns the (dst, src) byte-address pair
//! the MC fetcher's inner loop consumes — with [`McAddressError`]
//! capturing the four safe-Rust check failures (destination
//! overflow, source overflow, MV underflow / overflow, and a
//! role-mismatch type-level guard). Per the §5.4 / §7 chapter
//! boundary, the module deliberately does not own the `bank[+0x200]`
//! slot-index LUT or the `bank[+0x700]` cell-position aux LUT (those
//! per-entry values are §7.5 Extractor territory), does not own the
//! strip-context per-cell sub-array DWORDs (those are populated by
//! the spec/03 §6 open-question-4 pre-frame cell-stack setup), does
//! not perform the §7.2 `[esp+0x34]` boundary-fix-up reduction,
//! does not perform the §7.3 `(x, y, w, h)` reverse decomposition,
//! and does not perform the §4.2 `frame_flags` bit 9 source /
//! destination slot inversion (a per-plane-decoder decision).
//! Round 16 adds the spec/05 §4.2 ping-pong bank-selection surface
//! the round-15 [`McCellAddressPair::resolve`] entry deferred:
//! [`Bank`] (the typed primary / secondary bank enum with a
//! [`Bank::from_buffer_selector`] decoder of `frame_flags` bit 9
//! per the parser-text `test ch, 0x2` at
//! `IR32_32.DLL!0x100045b1`), [`BANK_INVERSION_DELTA`] (`= 3`,
//! the §4.2 "plane_idx + 3" identity surfaced as a named
//! constant aliased to `PRIMARY_BANK_SLOTS[i] -
//! SECONDARY_BANK_SLOTS[i]`), and [`McBankAssignment::resolve`]
//! (the typed `(FrameFlags, plane_idx) → (dst_slot, src_slot,
//! dst_bank)` mapping the per-plane decoder's
//! `IR32_32.DLL!0x100045b1..0x100045fd` sequence emits before
//! pushing `[esp+0x54]` / `[esp+0x58]`, with the source-bank
//! inversion baked in and a defensive [`McBankAssignment::is_self_copy`]
//! predicate for the §4.2 "never observed in the binary"
//! same-bank degenerate case). Round 17 adds the spec/05 §7.3
//! reverse-decomposition surface that round 15's [`McCellAddressPair::resolve`]
//! deferred ("does not perform the §7.3 `(x, y, w, h)` recovery from
//! the `dst_addr` byte address back into pixel coordinates"):
//! [`CELL_PIXELS_PER_COLUMN_GROUP`] (`4`, the §7.3 `cl_inner * 4`
//! factor aliased to [`MC_COLUMN_GROUP_PIXELS`] with a `const _`
//! cross-check), [`CELL_PIXELS_PER_ROW_BAND`] (`4`, the §7.3
//! `row_band_count * 4` factor aliased to [`MC_BAND_ROWS`]),
//! [`cell_width_from_column_group_count`] / [`cell_height_from_row_band_count`]
//! (the §7.3 `cell_w = cl_inner * 4` / `cell_h = row_band_count * 4`
//! mappings with `u32` overflow guards and §2.4 minimum-cell-size
//! zero-input rejection), [`row_band_count_from_ch_register`] (the
//! §7.3 / §7.1 `ecx >> 24` upper-byte extraction from the initial
//! `ch` register snapshot), [`CellCoords`] / [`cell_coords_from_dst_addr`]
//! (the §7.3 modular decomposition
//! `dst_addr → (cell_x = dst_addr mod 0xb0, cell_y = (dst_addr -
//! strip_base) / 0xb0)` against [`MC_ROW_STRIDE`]), and the
//! [`CellRect::from_parts`] / [`reverse_decompose`] entry points
//! that compose the three sub-facets into the full §7.3
//! `(cell_x, cell_y, cell_w, cell_h)` shape descriptor, with a
//! typed [`CellRectDecodeError`] surface for the four failure
//! modes (dst-address-below-strip-base, zero column-group count,
//! zero row-band count, dimension overflow). Per the §7.3 chapter
//! boundary, the module does not own the codebook-bank
//! `bank[+0x000]` LUT values (§7.5 Extractor territory; passed as
//! pre-resolved bytes), does not bridge strip-pixel-buffer
//! coordinates into whole-frame coordinates (`spec/07 §5.7`
//! strip-to-frame assembly), and does not validate the rectangle
//! against the §5.5 plane-role visible width (plane-role
//! classification stays with [`McPlaneRole::strip_visible_width`]).
//! A later round adds the spec/02 §9 typed plane-data byte map
//! ([`PlaneByteMap`]): [`PictureLayer::plane_byte_map`] returns a
//! per-plane structural view exposing the §9 landmark offsets —
//! the [`PlaneByteMap::num_vectors_range`] (§3.1 / §9 row 1) and
//! [`PlaneByteMap::mc_vectors_range`] (§3.2 / §9 row 2) absolute
//! byte ranges, the [`PlaneByteMap::payload_start`] (§3.4 / §9
//! row 3) bitstream entry, and the §6.1 / §10 item 4
//! [`PlaneByteMap::payload_upper_bound`] (the strict byte
//! budget the decoder may scan, resolved against the next
//! present plane's `plane_base` or the codec-frame buffer length).
//! The map's [`PlaneByteMap::payload_budget`] convenience exposes
//! `upper_bound - payload_start` — the §10 item 4
//! "end-of-plane padding" surface bridging the structural plane
//! layout to the (orthogonal) binary-tree walker's actual
//! consumption count. A later round adds the spec/02 §6.2 per-frame
//! plane-iteration terminator ([`frame_exit`]): [`PLANE_ITERATION_ORDER`]
//! pins the §8 `[2, 1, 0]` (U, V, Y) count-down loop order;
//! [`PER_PLANE_DECODE_CALL_SITE_RVA`] / [`PER_PLANE_DECODE_ENTRY_RVA`]
//! / [`PER_PLANE_DECODE_RET_RVA`] / [`PER_PLANE_DECODE_RET_CLEANUP_BYTES`]
//! pin the §6 call site, entry, and `ret 0x1c` seven-argument cdecl
//! cleanup; [`FRAME_OUTPUT_RECONSTRUCTION_RVA`] /
//! [`FRAME_FAULT_RETURN_RVA`] pin the §6.2 success handoff
//! (`IR32_32.DLL!0x10004644`) and the §6 end-of-frame fault path
//! (`IR32_32.DLL!0x10006ba2`, status `3`); [`FrameExitDisposition`]
//! and [`FramePlaneStatusFold`] fold the three round-8
//! [`PlaneDecodeStatus`] values, in §8 iteration order, into one
//! per-frame outcome (proceed-to-reconstruction vs end-of-frame
//! fault), short-circuiting on the first faulting plane. A later
//! round adds the spec/07 §4.3 / §5.6 / §5.7 output-buffer write
//! (`frame_output`): [`upshift_7bit_to_8bit`] runs the §4.3
//! 1-bit upshift (`shl byte, 1`, clearing the §4.4
//! [`EDGE_MARKER_BIT`] sentinel); [`OUTPUT_PLANE_ORDER`] pins the
//! §5.6 step 2 Y → V → U output plane order (the reverse of the
//! §5.2 decode order); [`IF09_FOURCC`] / [`IF09_FOURCC_CASE_RVA`] /
//! [`IF09_PASSTHROUGH_RVA`] pin the §5.3 / §5.6 IF09 dispatch
//! surface; [`assemble_plane_if09`] executes the §5.7
//! strip-to-frame assembly — walking a plane's strips left to
//! right, upshifting each visible row out of its 0xb0-stride strip
//! pixel buffer into the caller's full-width output raster; and
//! [`upsample_chroma_4x4`] runs the §5.5 4:1:0 → output box-filter
//! chroma upsampler — replicating each chroma sample into a 4×4
//! output block ([`CHROMA_UPSAMPLE_FACTOR`]). A later
//! round adds the spec/05 §5.1 / §5.2 / §7.2 + spec/03 §5.5
//! buffer-mutating MC executor ([`mc_exec`]):
//! [`boundary_fixup_dst_cell_offset`] runs the §7.2 `[esp+0x34]`
//! boundary-fix-up reduction (`bank[+0x700][cl] sar 2 +
//! extra_offset + ch`, [`BOUNDARY_FIXUP_SCRATCH_OFFSET`] /
//! [`BOUNDARY_FIXUP_AUX_SHIFT`]) that round 15 deferred, with
//! [`advance_boundary_fixup_row`] applying the spec/07 §1.2
//! per-row `add [esp+0x34], 0xb0` ([`BOUNDARY_FIXUP_ROW_ADVANCE`]);
//! [`mc_copy_cell`] executes the §5.1 / §5.2 cell copy over a strip
//! pixel buffer in the inner-loop order (read rows 0+1, write rows
//! 0+1, read rows 2+3, write rows 2+3; columns then bands) through
//! the round-14 per-DWORD kernels, with [`mc_copy_cell_mv`] driving
//! it from a [`PackedMv`] (§2.2 mode + §2.3 displacement) and
//! [`McCopyError`] carrying the safe-Rust arena-edge bounds the
//! binary omits per §4.4; [`apply_per_cell_edge_fixup`] executes the
//! spec/03 §5.5 inter-cell edge fix-up loop (the spec/07 §1.3
//! predictor-continuity exchange: `[esi+0x24]` → `[edi-4]`,
//! `[edi]` → `[esi+0x28]`, one row stride per iteration, do-while
//! `edx -= 4`), with [`PerCellEdgeFixupError`] for the buffer-edge
//! failure modes.
//!
//! All offsets, field widths, validation rules, and sentinel
//! values are taken from the per-chapter spec under
//! `docs/video/indeo/indeo3/spec/`. Section references in
//! doc-comments below cite the chapter named in each module.

mod bank_select;
mod cell_decoder;
mod cell_emit;
mod cell_geometry;
mod cell_loop;
mod cell_null;
mod cell_reconstruct;
mod cell_subarray;
mod cell_unpack;
mod codebook_seed;
mod decoder;
mod entropy;
mod frame;
mod frame_assemble;
mod frame_exit;
mod frame_finalise;
mod frame_output;
mod frame_reconstruct;
mod frame_session;
mod frame_yuv;
mod geometry_bank;
mod header;
mod macroblock;
mod mc_address;
mod mc_arena;
mod mc_bounds;
mod mc_chroma;
mod mc_exec;
mod mc_kernel;
mod mc_packed;
mod mc_residual_boundary;
mod mc_source_plumbing;
mod mc_table;
mod picture_decoder;
mod picture_layer;
mod plane_execute;
mod plane_reconstruct;
mod reconstruct;
mod registry;
mod row_stream;
mod staging;
mod strip_context;
mod strip_edge;
mod vq;

pub use bank_select::Bank;
pub use cell_decoder::{CellDecodeError, PlaneBuffers, PlaneContext, PlaneStats, decode_plane};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use bank_select::{BANK_INVERSION_DELTA, McBankAssignment};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use cell_emit::{
    CellEmitError, CellEmitGeometry, CellEmitStats, DyadDelta, PIXELS_PER_DYAD_DWORD,
    emit_cell_chain, rows_per_source_row,
};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use cell_geometry::{
    CELL_PIXELS_PER_COLUMN_GROUP, CELL_PIXELS_PER_ROW_BAND, CellCoords, CellRect,
    CellRectDecodeError, cell_coords_from_dst_addr, cell_height_from_row_band_count,
    cell_width_from_column_group_count, reverse_decompose, row_band_count_from_ch_register,
};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use cell_loop::{
    CELL_BANK_LEN, CELL_DATA_TABLE, CELL_POSITION_MAX, CELL_POSITION_TABLE, CH_CONTROL_LUT,
    CL_ROW_COUNTER_LUT, CellLoopPreamble, CellLoopState, CellRowAdvance, CodebookBankView,
    INTRA_CONTEXT_CLEAR_MASK, INTRA_CONTEXT_FLAG, MIRROR_TABLE_OFFSET, SLOT_INDEX_LUT, advance_row,
    dispatch_cell_preamble, iterate_column_rows, read_cell_position_dword, read_cl_row_counter,
};
pub use cell_null::{CopyUpperError, MarkEdgeError};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use cell_null::{
    COPY_UPPER_COLUMN_GROUP_BYTES, COPY_UPPER_RAW_ROW_OFFSETS, COPY_UPPER_ROW_COUNT,
    CopyUpperGeometry, CopyUpperStats, MarkEdgeGeometry, MarkEdgeStats, VqNullSubCode,
    copy_upper_cell, mark_edge_cell,
};
pub use cell_reconstruct::{
    CellOutcome, CellReconstructError, CellReconstructGeometry, CellRun, PositionEffect,
    SequenceReport, SequenceStep, reconstruct_cell_stateful, reconstruct_cell_static,
    run_cell_sequence,
};
pub use cell_unpack::{
    CANONICAL_DYAD_HANDLERS, CellUnpackError, DyadEmit, UnpackEffect, UnpackOutcome, UnpackRun,
    arena_primary_offset, arena_secondary_offset, unpack_cell,
};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use cell_subarray::{
    CELL_STACK_BEGIN_OFFSET, CELL_STACK_ENTRY_SIZE, CELL_STACK_MAX_ENTRIES, CellStackReadSite,
    CellStackTopDispatch, PER_CELL_EDGE_HEIGHT_STEP, PER_CELL_EDGE_PREV_BR_NEXT_OFFSET,
    PER_CELL_EDGE_PREV_BR_OFFSET, PER_CELL_EDGE_ROW_STRIDE, cell_stack_array_offset,
    cell_stack_slot_offset,
};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use codebook_seed::{
    BLOCK_DEST_ADVANCE, BLOCK_TERMINATOR, CodebookSeedArea, SEED_AREA_LEN, SEED_AREA_VMA,
    SEED_BLOCK_COUNT, SEED_PAIR_TOTAL, SEED_WORD_BIAS, SeedAreaError, SeedBlock, SeedPair,
};
pub use decoder::{DecodedOutput, DecoderError, Indeo3Decoder};
pub use picture_decoder::{DecodedPicture, Indeo3PictureDecoder, PictureDecodeError};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use row_stream::{CellRowsRun, Row, RowStreamError, decode_cell_rows, expand_doubled_rows};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use staging::{
    RowDeltaOutcome, STAGING_ALT_SET_OFFSET, STAGING_BLOCK_COUNT, STAGING_BLOCK_STRIDE,
    STAGING_IMAGE_LEN, STAGING_SUB_TABLE_LEN, StagingImage,
};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use entropy::{
    ARENA_BAND_STRIDE, CONTINUATION_XOR, CodebookBase, DyadAddress, FbCategory, FbCounter,
    HighNibbleAction, JumpTable, JumpTableEntry, LITERAL_MODE_MAX, LiteralMode,
    MAX_ROW_LOOKAHEAD_OFFSET, ModeByte, ModeByteKind, PRIMARY_TABLE_DISP, PositionClass,
    RLE_ESCAPE_MIN, RleEscape, RowLookahead, SECONDARY_TABLE_DISP, VARIANT_A_ENTRY,
    VARIANT_B_ENTRY, VARIANT_C_ENTRY, VARIANT_D_ENTRY, apply_continuation_xor, continuation_needed,
    fb_category, fb_category_table, handler_prologue, variant_entry_rva,
};
pub use frame::{
    DecodedFrame, DecodedPlane, FRAME_PLANE_DECODE_ORDER, FrameDecodeError, PlaneCellStats,
    ReconstructionStatus, decode_frame, decode_frame_with_selector,
};
pub use frame_assemble::{
    AssembleError, OUTPUT_ASSEMBLE_ORDER, OutputFrame, OutputPlane, allocate_strip_buffers,
    assemble_output, plane_strip_buffer_lengths,
};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use frame_exit::{
    FRAME_FAULT_RETURN_RVA, FRAME_OUTPUT_RECONSTRUCTION_RVA, FrameExitDisposition,
    FramePlaneStatusFold, PER_PLANE_DECODE_ARG_COUNT, PER_PLANE_DECODE_CALL_SITE_RVA,
    PER_PLANE_DECODE_ENTRY_RVA, PER_PLANE_DECODE_RET_CLEANUP_BYTES, PER_PLANE_DECODE_RET_RVA,
    PLANE_ITERATION_ORDER,
};
pub use frame_finalise::{
    DecodeReturn, FrameContinuity, FrameFinalisation, SavedFrameFlags, SavedFrameNumber,
};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use frame_finalise::{
    BUFFER_SELECTOR_MASK, FRAME_NUMBER_CONTINUITY_CHECK_RVA, FRAME_NUMBER_SEEK_PATH_RVA,
    PERFORMS_BUFFER_ROTATION, RETURN_INPUT_ERROR, RETURN_REPEAT_PREVIOUS, RETURN_SUCCESS,
    SAVED_FRAME_FLAGS_SLOT_OFFSET, SAVED_FRAME_FLAGS_STORE_RVA, SAVED_FRAME_NUMBER_SLOT_OFFSET,
    SAVED_FRAME_NUMBER_STORE_RVA,
};
pub use frame_output::{
    CHROMA_UPSAMPLE_FACTOR, ChromaUpsampleError, PlaneAssembleError, assemble_plane_if09,
    upsample_chroma_4x4, upshift_7bit_to_8bit,
};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use frame_output::{
    BI_BITFIELDS, BI_RGB, FRAME_OUTPUT_SRC_ROW_STRIDE, IF09_FOURCC, IF09_FOURCC_CASE_RVA,
    IF09_PASSTHROUGH_RVA, OUTPUT_PLANE_ORDER, OUTPUT_UPSHIFT_BITS, OutputConversion,
    OutputDispatchError, RGB24_STRIDE_FIXUP_BIT_COUNT, select_output_conversion,
    strip_min_buffer_bytes,
};
pub use frame_reconstruct::{
    FrameReconstructError, FrameReconstructStats, ReconstructedFrame, reconstruct_frame,
    reconstruct_frame_with_stream,
};
pub use frame_session::{AdmittedFrame, DecodeSession, FrameAdmission, SessionError};
pub use frame_yuv::{YuvError, YuvFrame, YuvPlane, assemble_yuv, upsample_frame};
pub use geometry_bank::{
    GeometryBank, PlaneBanks, UNDER_FOUR_CODE, UNDER_FOUR_YPOS, chroma_plane_dims, split_extent,
};
pub use header::{
    BITSTREAM_HEADER_LEN, BitstreamHeader, COMBINED_HEADER_LEN, FLAG_YVU9_8BIT, FRAME_HEADER_LEN,
    FrameFlags, FrameHeader, FrameHeaderPreamble, HeaderError, MAGIC_FRMH, MAX_HEIGHT, MAX_WIDTH,
    MIN_DIMENSION, NULL_FRAME_DATA_SIZE_BITS, REQUIRED_DEC_VERSION, alt_quant_indices,
};
pub use macroblock::{
    CHROMA_STRIP_WIDTH, Cell, CellTree, LUMA_STRIP_WIDTH, MacroblockError, NodeCode, VqCell,
    VqLeaf, VqNull, decode_plane_tree,
};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use mc_address::{
    CELL_SLOT_INDEX_MAX, CELL_SLOT_STRIDE, CellAddrEntry, CellAddrRole, CellSlotBase,
    CellSubarrayIndex, McAddressError, McCellAddressPair, mc_dest_address, mc_source_address,
};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use mc_arena::{
    MC_ARENA_LEN, MC_ARENA_ROW_STRIDE, STRIP_PIXEL_BUFFER_ALIAS_COUNT, StripArenaCapacity,
    StripPixelBufferAlias, base_pointer_aliases_equal, strip_region_bytes,
};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use mc_bounds::{
    MC_NO_BOUNDARY_CHECK, MvSourceOffsetClass, PaddingPixelPreservation,
    STRIP_REGION_LUMA_240_BYTES, STRIP_REGION_LUMA_240_FITS_IN_ARENA, SourcePointerBoundsCheck,
    mv_source_offset_in_strip_region,
};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use mc_chroma::{
    CHROMA_PACKED_MV_FACTOR_IS_BUFFER_STRIDE, LUMA_PIXEL_PER_CHROMA_PIXEL,
    MC_KERNEL_GEOMETRY_IS_PLANE_ROLE_INVARIANT, McKernelGeometryIsPlaneRoleInvariant, McPlaneRole,
    MvPixelOffsetInterpretation,
};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use mc_exec::{
    BOUNDARY_FIXUP_AUX_SHIFT, BOUNDARY_FIXUP_ROW_ADVANCE, BOUNDARY_FIXUP_SCRATCH_OFFSET,
    McCopyError, PerCellEdgeFixupError, advance_boundary_fixup_row, apply_per_cell_edge_fixup,
    boundary_fixup_dst_cell_offset, mc_copy_cell, mc_copy_cell_mv,
};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use mc_kernel::{
    MC_BAND_BYTE_STRIDE, MC_BAND_ROWS, MC_BYTES_PER_DWORD, MC_COLUMN_GROUP_PIXELS,
    MC_FULL_PEL_ROW_OFFSETS, MC_HORIZ_HALF_PEL_NEIGHBOUR_OFFSET, MC_INNER_LOOP_BYTES_PER_ITER,
    MC_INNER_LOOP_DWORDS_PER_ITER, MC_MAX_CELL_WIDTH_BYTES, MC_ROW_STRIDE,
    MC_VERT_HALF_PEL_NEIGHBOUR_OFFSET, McKernelGeometry, McKernelGeometryError, McKernelStep,
    mc_both_half_pel_quad, mc_full_pel_row_dword, mc_horiz_half_pel_pair, mc_vert_half_pel_pair,
};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use mc_packed::{
    MV_HORIZ_HALFPEL_BIT, MV_MODE_BITS_MASK, MV_PIXEL_OFFSET_ROW_STRIDE, MV_PIXEL_OFFSET_SHIFT,
    MV_VERT_HALFPEL_BIT, McDispatchMode, PackedMv, apply_mv_source_offset, pack_mv_components,
};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use mc_residual_boundary::{
    MC_CHAPTER_LAST_DST_ROW_INDEX, MC_FETCHER_LAST_WRITE_DST_OFFSET, MC_FETCHER_LAST_WRITE_RVA,
    MC_INNER_LOOP_BAND_ROWS_ALIAS, McCellDisposition, McToVqHandoff, ResidualApplication,
    VQ_RESIDUAL_DISPATCH_RVA, shares_destination_buffer,
};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use mc_source_plumbing::{
    DECODER_ARG_DST_SLOT_OFFSET, DECODER_ARG_SRC_SLOT_OFFSET, DISPATCHER_SCRATCH_DST_DATA_OFFSET,
    DISPATCHER_SCRATCH_EXTRA_OFFSET_OFFSET, DISPATCHER_SCRATCH_SRC_DATA_OFFSET, DecoderStackArg,
    DispatcherScratch, STRIP_CTX_ARRAY_ELEMENT_SHIFT, SourcePlumbingPair, is_self_copy_degenerate,
};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use mc_table::{
    MV_HALFPEL_HORIZ, MV_HALFPEL_MASK, MV_HALFPEL_VERT, MV_INDEX_SCALE_SHIFT, MV_TABLE_BASE_OFFSET,
    MV_TABLE_BYTES, MV_TABLE_ENTRY_SIZE, MV_TABLE_MAX_BYTE_INDEXABLE_ENTRIES, MvIndexFetch,
    MvIndexValidity, MvTableParserArm, mv_table_entry_byte_offset,
};
pub use picture_layer::{
    MC_VECTOR_ENTRY_LEN, MIN_PRELUDE_LEN, MotionVector, NUM_VECTORS_FIELD_LEN, PLANE_COUNT,
    PLANE_IDX_U, PLANE_IDX_V, PLANE_IDX_Y, PictureLayer, PictureLayerError, PlaneByteMap,
    PlaneDecodePlan, PlanePrelude, PlanePresence,
};
pub use plane_execute::{
    DeferredFrontier, PlaneExecError, PlaneExecStats, ReconstructedPlane, STRIP_ROW_STRIDE,
    UnpackerCellFault, exec_plane_plan, exec_plane_plan_with_stream, plane_strip_len,
};
pub use plane_reconstruct::{
    CellDisposition, CellPlanEntry, DispositionCounts, PlaneReconstructError, PlaneReconstructPlan,
    VqNullDriveStats, classify_cell_tree, classify_plane, drive_vq_null_copies,
};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use reconstruct::{
    CLAMP_7BIT_MASK, DyadOutcome, EDGE_MARKER_BIT, HALF_SENTINEL_MASK, HALVE_CARRY_MASK,
    PIXEL_VALUE_MAX, PREDICTOR_ROW_STRIDE, RowEmission, SoftSimdSum, TOP_OF_STRIP_PREDICTOR,
    VariantEmission, apply_dyad_pair, average_7bit, emit_variant, halve_fefefefe, jns_taken,
    pack_predictor, predictor_offset, unpack_pixels,
};
pub use registry::{
    CODEC_ID_STR, INDEO3_FOURCCS, Indeo3RegistryDecoder, PROBE_CONFIDENCE_HEADER_OK,
    PROBE_CONFIDENCE_TAG_ONLY, codec_id_for_fourcc, decode_video_frame, make_decoder, probe,
    register, register_codecs,
};
pub use strip_context::{
    PerPlaneDecodeCall, PlaneDecodeStatus, PlaneRole, StripGeometry, StripSlotDescriptor,
    chroma_plane_height, chroma_plane_width, chroma_strip_slot_count, luma_strip_slot_count,
    strip_slot_index,
};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use strip_context::{
    DISPATCHABLE_SLOT_COUNT, INSTANCE_CHROMA_CODEBOOK_BANK, INSTANCE_LUMA_CODEBOOK_BANK,
    INSTANCE_SECONDARY_CODEBOOK_PTR, INSTANCE_STATE_LEN, INSTANCE_STRIP_ARRAY_VIEW_PTR,
    PIXEL_BUFFER_ARENA_LEN, PLANE_DECODE_STATUS_MALFORMED, PLANE_DECODE_STATUS_OK,
    PRIMARY_BANK_SLOTS, SECONDARY_BANK_SLOTS, STRIP_ARRAY_OFFSET_IN_INSTANCE,
    STRIP_SLOT_BASE_PTR_COUNT, STRIP_SLOT_COUNT, STRIP_SLOT_SENTINEL, STRIP_SLOT_STRIDE,
    slot_field,
};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use strip_edge::{
    STRIP_EDGE_BYTE_READ_OFFSET, STRIP_EDGE_BYTE_WRITE_OFFSET, STRIP_EDGE_CHROMA_SHIFT,
    STRIP_EDGE_ROW_STRIDE, StripEdgeApplyError, StripEdgeFixupDims, StripEdgeRow, StripEdgeRowIter,
    strip_edge_byte_copy_offsets, strip_edge_chroma_shift, strip_edge_row_step,
};
pub use vq::{DyadDeltaTable, SeedDispatchTables, VqError};
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub use vq::{
    ARENA_BAND_COUNT, ARENA_BAND_LEN, ARENA_BANDS_OFFSET, ARENA_HALF_LEN, ARENA_LEN, CellVariant,
    CodebookEntry, DYAD_BANK_COUNT, DYAD_BANK_STRIDE, DYAD_BANK15_VALID_ROWS, DYAD_TABLE_LEN,
    RowBandSeed, SEED_DISPATCH_RECORDS, SEED_PAIR_COUNT, SEED_TABLE_LEN, STAGING_SELECT_STRIDE,
    SeedEntry, VqArena, VqNullRuntime, apply_row_band_seed, seed_dispatch_entries,
};
