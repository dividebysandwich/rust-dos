//! Indeo 3 arena-parameterised per-cell mode-byte unpacker.
//!
//! Spec source: `docs/video/indeo/indeo3/spec/06-entropy.md` §3 / §4
//! (mode-byte stream, jump-table dispatch, continuation rule, RLE
//! escapes) + `spec/07-output-reconstruction.md` §1 / §2 / §3 (predictor
//! chain, the `0x10006e0f..0x10006e2e` dyad-pair add, the four
//! cell-shape variant stores, the static-table row-band seed).
//!
//! ## What this module adds
//!
//! [`super::reconstruct_cell_static`] executes the *static-table-only*
//! subset of the mode-byte stream and stops at every arena-addressed
//! literal. The arithmetic on the other side of that stop is already
//! landed ([`super::apply_dyad_pair`] / [`super::emit_variant`]) but
//! only over *caller-supplied* per-position deltas
//! ([`super::emit_cell_chain`]). Nothing yet connected the two: a
//! mode-byte stream driving arena lookups driving pixel stores.
//!
//! [`unpack_cell`] is that connection — the full `spec/06 §3` literal
//! path, generic over a caller-supplied [`VqArena`]:
//!
//! 1. **Dispatch** (`spec/06 §3.1` / `§3.2`): the mode byte's bit-3
//!    selected jump table routes the high nibble. Fault slots return
//!    the binary's error-code-1 as [`CellUnpackError::ModeByteFault`].
//!    The canonical dyad handlers (`0x10006c14`, table 1 high nibbles
//!    `0x0`/`0x3`/`0xA`; `0x10006c9c`, table 2 high nibble `0x0`) run
//!    step 2; every other (or unpinned) handler defers — their bodies
//!    are not staged past the dispatch level
//!    ([`UnpackOutcome::DeferredHandler`]).
//! 2. **The canonical dyad position** (`spec/07 §2.1` / `§3.2`):
//!    * the handler prologue seeds the predictor slot `[edi - 0xb0]`
//!      from the on-disk static dyad table at `.data + 0x1003d088`,
//!      bank = high nibble ([`apply_row_band_seed`]);
//!    * the inner loop loads the (just-seeded) predictor DWORD and
//!      adds the per-frame-arena primary entry at
//!      `arena + (low_nibble << 11) + 4*col + 0x400`;
//!    * on the `jns` sentinel (`spec/06 §3.3`) a **continuation byte**
//!      is consumed from the stream and re-indexes the band's
//!      secondary word at `+ 4*continuation + 0x402` (the §3.3
//!      "another dyad index" reading of `mov dl, [ebp+1]`); a
//!      still-negative low half faults with the binary's error code 2
//!      ([`CellUnpackError::DyadRangeFault`]);
//!    * the [`CellVariant`]'s store shape ([`emit_variant`]) writes
//!      the resulting DWORD to one or two `0xb0`-stride rows.
//! 3. **Escapes** (`spec/06 §4`): identical protocol to the static
//!    executor — `0xFD`/`0xFE`/`0xFF` row skips, `0xFB` counter
//!    terminate (decoded [`FbCounter`]), `0xF9`/`0xFC` next-cell-skip
//!    carry, `0xF8`/`0xFA` cell marks, all gated by the §4.3
//!    per-position acceptance matrix.
//!
//! ## What stays gated
//!
//! The **arena values themselves** are still the `spec/04 §7.1` /
//! `§5.2` docs-gap (the `.data + 0x1004d26a` block-format
//! spec-vs-audit contradiction blocks the codec-init materialisation).
//! This module is deliberately generic over the arena *content*: tests
//! drive it with synthetic arenas, and when the extraction round lands
//! the real values plug in with no algorithmic change. Also unstaged:
//! the non-canonical handler bodies (`0x10006c90` single-pixel fill,
//! `0x100072bb` doubled-row, `0x100072c7` / `0x1000771c` / `0x10007710`
//! / `0x10007a9b`) — those defer, precisely tagged with their RVA.
//!
//! ## Addressing note (docs tension)
//!
//! `spec/07 §2.1` states `esi = arena base + 2048 × low nibble`, so
//! the primary DWORD is read at `arena + low_nibble*0x800 + 4*col +
//! 0x400` — which for `low_nibble = 0` lands in the arena's
//! `+0x000..+0x7ff` codec-init region rather than the `spec/04 §6.3`
//! per-band region at `+0x800 + 0x800*band`, and `spec/07 §2.1`'s note
//! ("the `+0x400` literal is **not** the secondary table") sits uneasily
//! beside `spec/07 §2.3` step 2 ("`+0x402` … the secondary table's
//! low-half-DWORD position") and the `spec/04 §2.1` half-table layout.
//! This module implements the literal address arithmetic exactly as
//! the §2.1 / §2.3 instructions give it and leaves the half-table
//! naming to a Specifier reconciliation (reported as a docs gap).

use super::cell_emit::rows_per_source_row;
use super::cell_reconstruct::{CellReconstructGeometry, PositionEffect, fb_run};
use super::entropy::{
    FbCounter, LiteralMode, ModeByte, ModeByteKind, PRIMARY_TABLE_DISP, PositionClass, RleEscape,
    SECONDARY_TABLE_DISP,
};
use super::reconstruct::{
    DyadOutcome, PREDICTOR_ROW_STRIDE, TOP_OF_STRIP_PREDICTOR, VariantEmission, emit_variant,
    pack_predictor,
};
use super::vq::{ARENA_LEN, CellVariant, DyadDeltaTable, VqArena, apply_row_band_seed};

/// The two canonical literal-dyad handler RVAs (`spec/06 §3.2`): the
/// table-1 entry shared by high nibbles `0x0` / `0x3` / `0xA`, and the
/// table-2 high-nibble-`0x0` entry. These are the only mode-byte
/// handlers whose bodies are staged to the pixel level
/// (`spec/07 §2.1` / `§3.2`); every other handler defers.
pub const CANONICAL_DYAD_HANDLERS: [u32; 2] = [0x1000_6c14, 0x1000_6c9c];

/// One canonical-dyad position's emission, recorded for the trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DyadEmit {
    /// The literal mode byte that drove the position.
    pub mode_byte: u8,
    /// The source-row index (0-based).
    pub row: usize,
    /// The dyad-pair column index (0-based).
    pub dword: usize,
    /// The static-table seed delta written into the predictor slot
    /// (`spec/07 §3.2`).
    pub seed_delta: i8,
    /// The output pixel DWORD stored (post-variant shaping).
    pub pixels: u32,
    /// Number of `0xb0`-stride rows the variant store wrote (1 or 2).
    pub rows_written: usize,
    /// `true` when the position consumed a continuation byte
    /// (`spec/06 §3.3` two-byte form).
    pub continuation: bool,
}

/// One position's effect in the unpacker trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnpackEffect {
    /// A canonical-dyad literal emitted pixels ([`DyadEmit`]).
    Dyad(DyadEmit),
    /// An escape's row-skip / seed effect, shared with the static
    /// executor's vocabulary.
    Static(PositionEffect),
}

/// How a cell's arena-driven walk finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnpackOutcome {
    /// Every position resolved; the cell's pixels are in the strip.
    Complete(Vec<UnpackEffect>),
    /// The walk reached a literal whose jump-table slot routes to a
    /// handler body that is not staged past the dispatch level
    /// (`spec/06 §3.2`: `0x10006c90` / `0x100072bb` / `0x100072c7` /
    /// `0x1000771c` / `0x10007710` / `0x10007a9b`, or a table-2
    /// `0x5..=0x9` unpinned slot). The walk stops *before* the byte.
    DeferredHandler {
        /// The mode byte the walk stopped at.
        mode_byte: u8,
        /// The handler RVA the jump table routes to, when pinned
        /// (`None` for the table-2 `0x5..=0x9` unpinned slots).
        handler_rva: Option<u32>,
        /// The source-row index (0-based) of the deferred position.
        row: usize,
        /// The dyad-pair column index (0-based).
        dword: usize,
        /// The effects emitted before the deferral.
        emitted: Vec<UnpackEffect>,
    },
    /// Consumed by a carried next-cell-skip flag (`spec/06 §4.6`).
    SkippedByCarry,
}

/// One cell's unpacker run: outcome plus cross-cell state (mirrors
/// [`super::CellRun`] for the arena-driven executor).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnpackRun {
    /// How the walk finished.
    pub outcome: UnpackOutcome,
    /// Net bytes consumed from the mode-byte stream (mode bytes +
    /// continuation bytes + the `0xFB` counter byte).
    pub bytes_consumed: usize,
    /// The `spec/06 §4.6` next-cell-skip flag after this cell.
    pub next_cell_skip: bool,
}

/// Errors the arena-driven unpacker surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CellUnpackError {
    /// Degenerate geometry (zero width or zero rows).
    ZeroDimension {
        /// `true` if `width_dwords == 0`, else `source_rows == 0`.
        is_width: bool,
    },
    /// The mode-byte stream ran out mid-walk.
    ByteStreamExhausted {
        /// The byte index the next read would have used.
        next_index: usize,
        /// The supplied stream length.
        supplied: usize,
    },
    /// An escape at a position the `spec/06 §4.3` matrix rejects
    /// (the binary's error-code-1 return).
    EscapeFault {
        /// The faulting escape byte.
        escape: u8,
        /// The position class it faulted at.
        position: PositionClass,
        /// Source-row index.
        row: usize,
        /// Dyad-column index.
        dword: usize,
    },
    /// A literal mode byte indexed a fault slot of its jump table
    /// (`spec/06 §3.2`, target `0x10007a96` → error code 1).
    ModeByteFault {
        /// The faulting mode byte.
        mode_byte: u8,
        /// Source-row index.
        row: usize,
        /// Dyad-column index.
        dword: usize,
    },
    /// The `spec/07 §2.3` step-3 range fault: the low half's sign bit
    /// stayed set after the secondary-table add (the binary's
    /// error-code-2 return at `0x1000855f`).
    DyadRangeFault {
        /// Source-row index.
        row: usize,
        /// Dyad-column index.
        dword: usize,
    },
    /// A static-table row-band seed targeted a slot outside the strip
    /// (`spec/07 §3.2`).
    SeedWriteOutOfBounds {
        /// The write index whose `- 0xb0` predictor slot was bad.
        write_index: usize,
        /// The strip length.
        buffer_len: usize,
    },
    /// A variant store's row write would exceed the strip buffer.
    StoreOutOfBounds {
        /// The exclusive end byte offset the store needed.
        write_end: usize,
        /// The strip length.
        buffer_len: usize,
    },
    /// An `0xFB` counter byte the binary rejects (`spec/06 §4.4`
    /// corrected: `0x00`, `0x20`, or bits 6..7 set).
    FbCounterInvalid {
        /// The rejected counter byte.
        counter: u8,
        /// Source-row index.
        row: usize,
        /// Dyad-column index.
        dword: usize,
    },
    /// An arena read fell outside the `0x8020`-byte arena — cannot
    /// happen for a well-formed [`VqArena`] (the address arithmetic is
    /// bounded by construction) but surfaced rather than panicking.
    ArenaReadOutOfBounds {
        /// The offending arena byte offset.
        offset: usize,
    },
}

impl core::fmt::Display for CellUnpackError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CellUnpackError::ZeroDimension { is_width } => write!(
                f,
                "spec/07 §1.2: cell {} is zero",
                if *is_width {
                    "width"
                } else {
                    "source-row count"
                }
            ),
            CellUnpackError::ByteStreamExhausted {
                next_index,
                supplied,
            } => write!(
                f,
                "spec/06 §1.2: mode-byte stream exhausted at index {next_index} (len {supplied})"
            ),
            CellUnpackError::EscapeFault {
                escape,
                position,
                row,
                dword,
            } => write!(
                f,
                "spec/06 §4.3: escape {escape:#04x} faults (error code 1) at {position:?} \
                 (row {row}, dword {dword})"
            ),
            CellUnpackError::ModeByteFault {
                mode_byte,
                row,
                dword,
            } => write!(
                f,
                "spec/06 §3.2: mode byte {mode_byte:#04x} indexes a fault slot (error code 1) \
                 at row {row}, dword {dword}"
            ),
            CellUnpackError::DyadRangeFault { row, dword } => write!(
                f,
                "spec/07 §2.3: dyad delta out of representable range (error code 2) at \
                 row {row}, dword {dword}"
            ),
            CellUnpackError::SeedWriteOutOfBounds {
                write_index,
                buffer_len,
            } => write!(
                f,
                "spec/07 §3.2: row-band seed at write index {write_index} is outside the \
                 {buffer_len}-byte strip buffer"
            ),
            CellUnpackError::StoreOutOfBounds {
                write_end,
                buffer_len,
            } => write!(
                f,
                "spec/07 §2.2: variant store end {write_end} exceeds the {buffer_len}-byte \
                 strip buffer"
            ),
            CellUnpackError::FbCounterInvalid {
                counter,
                row,
                dword,
            } => write!(
                f,
                "spec/06 §4.4: 0xFB counter {counter:#04x} is invalid (error code 1) at \
                 row {row}, dword {dword}"
            ),
            CellUnpackError::ArenaReadOutOfBounds { offset } => write!(
                f,
                "spec/07 §2.1: arena read at offset {offset:#x} outside the 0x8020-byte arena"
            ),
        }
    }
}

impl std::error::Error for CellUnpackError {}

/// Spec/07 §2.1 — the arena byte offset of a literal mode byte's
/// primary-table DWORD: `low_nibble*0x800 + 4*col + 0x400`
/// (`add eax, [esi + 4*edx + 0x400]` with `esi = arena base +
/// (low_nibble << 11)`).
pub fn arena_primary_offset(low_nibble: u8, col: usize) -> usize {
    ((low_nibble as usize) << 11) + 4 * col + PRIMARY_TABLE_DISP
}

/// Spec/07 §2.3 / spec/06 §3.3 — the arena byte offset of the
/// continuation path's secondary 16-bit word:
/// `low_nibble*0x800 + 4*continuation_byte + 0x402` (the
/// `mov dl, [ebp+1]` re-index — the continuation byte is "another
/// dyad index" into the same band).
pub fn arena_secondary_offset(low_nibble: u8, continuation_byte: u8) -> usize {
    ((low_nibble as usize) << 11) + 4 * (continuation_byte as usize) + SECONDARY_TABLE_DISP
}

/// Read the primary DWORD at the `spec/07 §2.1` address (little-endian,
/// the x86 `mov`/`add` 32-bit load).
fn arena_primary_dword(
    arena: &VqArena,
    low_nibble: u8,
    col: usize,
) -> Result<u32, CellUnpackError> {
    let off = arena_primary_offset(low_nibble, col);
    read_dword(arena.as_bytes(), off)
}

/// Read the secondary word at the `spec/07 §2.3` address.
fn arena_secondary_word(
    arena: &VqArena,
    low_nibble: u8,
    continuation_byte: u8,
) -> Result<u16, CellUnpackError> {
    let off = arena_secondary_offset(low_nibble, continuation_byte);
    let bytes = arena.as_bytes();
    if off + 2 > ARENA_LEN {
        return Err(CellUnpackError::ArenaReadOutOfBounds { offset: off });
    }
    Ok(u16::from_le_bytes([bytes[off], bytes[off + 1]]))
}

fn read_dword(bytes: &[u8; ARENA_LEN], off: usize) -> Result<u32, CellUnpackError> {
    if off + 4 > ARENA_LEN {
        return Err(CellUnpackError::ArenaReadOutOfBounds { offset: off });
    }
    Ok(u32::from_le_bytes([
        bytes[off],
        bytes[off + 1],
        bytes[off + 2],
        bytes[off + 3],
    ]))
}

/// Spec/07 §1.1 / §1.3 — the row-above predictor DWORD for a write at
/// `write_offset` (`mov eax, [edi - 0xb0]`), with the top-of-strip
/// zero seed for rows without an in-buffer predecessor.
fn predictor_dword(strip: &[u8], write_offset: usize) -> u32 {
    match write_offset.checked_sub(PREDICTOR_ROW_STRIDE) {
        None => pack_predictor([TOP_OF_STRIP_PREDICTOR; 4]),
        Some(pred_off) => {
            let mut bytes = [TOP_OF_STRIP_PREDICTOR; 4];
            for (i, b) in bytes.iter_mut().enumerate() {
                if let Some(v) = strip.get(pred_off + i) {
                    *b = *v;
                }
            }
            pack_predictor(bytes)
        }
    }
}

/// Classify the position for the `spec/06 §4.3` acceptance matrix
/// (same collapse as the static executor's).
fn position_class(row: usize, dword: usize) -> PositionClass {
    match dword {
        0 if row == 0 => PositionClass::CellFirst,
        0 => PositionClass::RowFirst,
        1 => PositionClass::Continuation1,
        2 => PositionClass::Continuation2,
        _ => PositionClass::Continuation3,
    }
}

fn read_byte(bytes: &[u8], cursor: &mut usize) -> Result<u8, CellUnpackError> {
    match bytes.get(*cursor) {
        Some(&b) => {
            *cursor += 1;
            Ok(b)
        }
        None => Err(CellUnpackError::ByteStreamExhausted {
            next_index: *cursor,
            supplied: bytes.len(),
        }),
    }
}

/// Spec/06 §3 / §4 + spec/07 §1 / §2 / §3 — drive one cell through the
/// **arena-parameterised** mode-byte unpacker.
///
/// The full literal-dyad decode path over a caller-supplied
/// [`VqArena`]: jump-table dispatch, static-table predictor seeding,
/// the `spec/07 §2.1` softSIMD add with the `spec/06 §3.3`
/// continuation-byte fall-back, and the [`CellVariant`] store shape —
/// with the same escape protocol and cross-cell carry as
/// [`super::reconstruct_cell_stateful`]. See the module docs for the
/// staged-vs-gated boundary: the arena *values* remain the `spec/04
/// §7.1` docs-gap, so real-frame decode waits on the extraction, but
/// every algorithmic step is closed here.
pub fn unpack_cell(
    strip: &mut [u8],
    geometry: CellReconstructGeometry,
    variant: CellVariant,
    mode_bytes: &[u8],
    table: &DyadDeltaTable,
    arena: &VqArena,
    carry_in: bool,
) -> Result<UnpackRun, CellUnpackError> {
    if geometry.width_dwords == 0 {
        return Err(CellUnpackError::ZeroDimension { is_width: true });
    }
    if geometry.source_rows == 0 {
        return Err(CellUnpackError::ZeroDimension { is_width: false });
    }
    if carry_in {
        return Ok(UnpackRun {
            outcome: UnpackOutcome::SkippedByCarry,
            bytes_consumed: 0,
            next_cell_skip: false,
        });
    }

    let rows_per = rows_per_source_row(variant);
    let mut effects: Vec<UnpackEffect> = Vec::new();
    let mut cursor = 0usize;
    let mut row_dst_offset = geometry.top_left_offset;
    let mut row = 0usize;

    while row < geometry.source_rows {
        let mut dword = 0usize;
        let mut extra_skip_rows = 0usize;

        while dword < geometry.width_dwords {
            let raw = read_byte(mode_bytes, &mut cursor)?;
            let mode = ModeByte::classify(raw);
            let position = position_class(row, dword);

            match mode.kind {
                ModeByteKind::Literal(lit) => {
                    let entry = lit.dispatch_entry();
                    if entry.is_fault() {
                        return Err(CellUnpackError::ModeByteFault {
                            mode_byte: raw,
                            row,
                            dword,
                        });
                    }
                    let rva = entry.handler_rva();
                    if !matches!(rva, Some(r) if CANONICAL_DYAD_HANDLERS.contains(&r)) {
                        // A pinned-but-unstaged handler body (or a
                        // table-2 0x5..=0x9 unpinned slot): stop
                        // before the byte, cursor backed up so the
                        // caller sees the exact frontier.
                        cursor -= 1;
                        return Ok(UnpackRun {
                            outcome: UnpackOutcome::DeferredHandler {
                                mode_byte: raw,
                                handler_rva: rva,
                                row,
                                dword,
                                emitted: effects,
                            },
                            bytes_consumed: cursor,
                            next_cell_skip: false,
                        });
                    }

                    let emit = dyad_position(
                        strip,
                        variant,
                        lit,
                        row,
                        dword,
                        row_dst_offset,
                        mode_bytes,
                        &mut cursor,
                        table,
                        arena,
                    )?;
                    effects.push(UnpackEffect::Dyad(emit));
                    dword += 1;
                }
                ModeByteKind::Escape(escape) => {
                    if !escape.accepted_at(position) {
                        return Err(CellUnpackError::EscapeFault {
                            escape: raw,
                            position,
                            row,
                            dword,
                        });
                    }
                    match escape {
                        RleEscape::Ff => {
                            effects.push(UnpackEffect::Static(PositionEffect::RowSkip {
                                escape,
                                rows_skipped: 1,
                            }));
                            extra_skip_rows = 1;
                            break;
                        }
                        RleEscape::Fe => {
                            effects.push(UnpackEffect::Static(PositionEffect::RowSkip {
                                escape,
                                rows_skipped: 2,
                            }));
                            extra_skip_rows = 2;
                            break;
                        }
                        RleEscape::Fd => {
                            let remaining = geometry.source_rows - row;
                            effects.push(UnpackEffect::Static(PositionEffect::RowSkip {
                                escape,
                                rows_skipped: remaining,
                            }));
                            return Ok(UnpackRun {
                                outcome: UnpackOutcome::Complete(effects),
                                bytes_consumed: cursor,
                                next_cell_skip: false,
                            });
                        }
                        // `0xFB` — a bounded in-cell run of dyad
                        // positions (`spec/06 §4.4` corrected):
                        // repeat-row-above or edge-mark, stopping at
                        // the counter or the cell's end.
                        RleEscape::Fb => {
                            let counter = read_byte(mode_bytes, &mut cursor)?;
                            let decoded = FbCounter::decode(counter);
                            if !decoded.is_valid() {
                                return Err(CellUnpackError::FbCounterInvalid {
                                    counter,
                                    row,
                                    dword,
                                });
                            }
                            let run = fb_run(
                                strip,
                                geometry,
                                &mut row,
                                &mut dword,
                                &mut row_dst_offset,
                                rows_per,
                                decoded,
                            )
                            .map_err(|(write_index, buffer_len)| {
                                CellUnpackError::StoreOutOfBounds {
                                    write_end: write_index + 4,
                                    buffer_len,
                                }
                            })?;
                            effects.push(UnpackEffect::Static(PositionEffect::FbRun {
                                decoded,
                                positions: run.positions,
                            }));
                            if run.cell_exhausted {
                                return Ok(UnpackRun {
                                    outcome: UnpackOutcome::Complete(effects),
                                    bytes_consumed: cursor,
                                    next_cell_skip: false,
                                });
                            }
                            continue;
                        }
                        RleEscape::Fc | RleEscape::F9 => {
                            return Ok(UnpackRun {
                                outcome: UnpackOutcome::Complete(effects),
                                bytes_consumed: cursor,
                                next_cell_skip: true,
                            });
                        }
                        RleEscape::F8 | RleEscape::Fa => {
                            return Ok(UnpackRun {
                                outcome: UnpackOutcome::Complete(effects),
                                bytes_consumed: cursor,
                                next_cell_skip: false,
                            });
                        }
                    }
                }
            }
        }

        let advance_rows = if extra_skip_rows > 0 {
            extra_skip_rows
        } else {
            1
        };
        // The destination advances by the variant's per-source-row
        // output rows (`spec/07 §1.2` / §2.2 — one stride for the
        // with-edge variant, two for the doubling variants).
        row_dst_offset += advance_rows * rows_per * PREDICTOR_ROW_STRIDE;
        row += advance_rows;
    }

    Ok(UnpackRun {
        outcome: UnpackOutcome::Complete(effects),
        bytes_consumed: cursor,
        next_cell_skip: false,
    })
}

/// One canonical-dyad position (`spec/07 §2.1` / `§3.2`): static seed
/// prologue, arena primary add, optional continuation, variant store.
#[allow(clippy::too_many_arguments)]
fn dyad_position(
    strip: &mut [u8],
    variant: CellVariant,
    lit: LiteralMode,
    row: usize,
    dword: usize,
    row_dst_offset: usize,
    mode_bytes: &[u8],
    cursor: &mut usize,
    table: &DyadDeltaTable,
    arena: &VqArena,
) -> Result<DyadEmit, CellUnpackError> {
    let write_index = row_dst_offset + dword * 4;

    // §3.2 prologue — seed the predictor slot from the static table
    // (bank = high nibble; the 0x10006c34 `[edi - 0xb0] = dl` write).
    let seed = apply_row_band_seed(table, strip, write_index, lit.high_nibble, row, dword).ok_or(
        CellUnpackError::SeedWriteOutOfBounds {
            write_index,
            buffer_len: strip.len(),
        },
    )?;

    // §2.1 inner loop — predictor (including the seeded byte) plus the
    // arena primary entry.
    let predictor = predictor_dword(strip, write_index);
    let primary = arena_primary_dword(arena, lit.low_nibble, dword)?;

    // First try with a zero secondary word to detect the sentinel; on
    // continuation, re-run with the stream-fetched secondary.
    let sum = predictor.wrapping_add(primary);
    let (emission, continuation) = if sum & 0x8000_0000 == 0 {
        // `jns` taken — the primary path; the secondary word is never
        // fetched (pass 0, unused).
        (emit_variant(variant, predictor, primary, 0), false)
    } else {
        // `spec/06 §3.3` — read the continuation byte and re-index the
        // band's secondary word with it.
        let continuation_byte = read_byte(mode_bytes, cursor)?;
        let secondary = arena_secondary_word(arena, lit.low_nibble, continuation_byte)?;
        (emit_variant(variant, predictor, primary, secondary), true)
    };

    let VariantEmission { outcome, rows } = emission;
    if matches!(outcome, DyadOutcome::Fault) {
        return Err(CellUnpackError::DyadRangeFault { row, dword });
    }

    // Variant store — `rows[i]` at `[edi + i*0xb0]`.
    let mut pixels_stored = 0u32;
    for (i, &dword_out) in rows.as_slice().iter().enumerate() {
        let off = write_index + i * PREDICTOR_ROW_STRIDE;
        let end = off + 4;
        if end > strip.len() {
            return Err(CellUnpackError::StoreOutOfBounds {
                write_end: end,
                buffer_len: strip.len(),
            });
        }
        strip[off..end].copy_from_slice(&dword_out.to_le_bytes());
        pixels_stored = dword_out;
    }

    Ok(DyadEmit {
        mode_byte: lit.byte,
        row,
        dword,
        seed_delta: seed.delta,
        pixels: pixels_stored,
        rows_written: rows.len(),
        continuation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const STRIDE: usize = PREDICTOR_ROW_STRIDE;

    fn geom(width: usize, rows: usize, top: usize) -> CellReconstructGeometry {
        CellReconstructGeometry {
            width_dwords: width,
            source_rows: rows,
            top_left_offset: top,
        }
    }

    /// A synthetic arena with a recognisable primary DWORD / secondary
    /// word planted at the spec/07 §2.1 / §2.3 addresses.
    ///
    /// Note the address structure the §2.3 arithmetic implies: the
    /// secondary word at `4*i + 0x402` is the *high half* of the
    /// DWORD entry at `4*i + 0x400`, so the two writes can overlap
    /// when the indices coincide — the secondary is planted first so
    /// the primary's bytes win at any overlap.
    fn arena_with(low_nibble: u8, col: usize, primary: u32, cont: u8, secondary: u16) -> VqArena {
        let mut arena = VqArena::new();
        let s = arena_secondary_offset(low_nibble, cont);
        arena.bytes_mut()[s..s + 2].copy_from_slice(&secondary.to_le_bytes());
        let p = arena_primary_offset(low_nibble, col);
        arena.bytes_mut()[p..p + 4].copy_from_slice(&primary.to_le_bytes());
        arena
    }

    #[test]
    fn arena_offsets_match_spec_arithmetic() {
        // spec/07 §2.1: esi = ln << 11; primary at [esi + 4*col + 0x400].
        assert_eq!(arena_primary_offset(0, 0), 0x400);
        assert_eq!(arena_primary_offset(5, 3), (5 << 11) + 12 + 0x400);
        // §2.3: secondary at [esi + 4*cont + 0x402].
        assert_eq!(arena_secondary_offset(0, 0), 0x402);
        assert_eq!(arena_secondary_offset(15, 255), (15 << 11) + 1020 + 0x402);
        // Both bounded within the arena for every nibble/index.
        assert!(arena_secondary_offset(15, 255) + 2 <= ARENA_LEN);
        assert!(arena_primary_offset(15, 255) + 4 <= ARENA_LEN);
    }

    #[test]
    fn primary_path_emits_variant_a_two_rows() {
        let table = DyadDeltaTable::load();
        // Predictor row is zero (top at row 1 keeps a zero row 0 above),
        // primary delta 0x01020304 — no sentinel → primary path.
        let arena = arena_with(0, 0, 0x0102_0304, 0, 0);
        let top = STRIDE;
        let mut strip = vec![0u8; STRIDE * 6];
        let run = unpack_cell(
            &mut strip,
            geom(1, 1, top),
            CellVariant::Plain,
            &[0x00],
            &table,
            &arena,
            false,
        )
        .unwrap();
        assert_eq!(run.bytes_consumed, 1);
        match &run.outcome {
            UnpackOutcome::Complete(effects) => {
                assert_eq!(effects.len(), 1);
                match effects[0] {
                    UnpackEffect::Dyad(e) => {
                        assert!(!e.continuation);
                        assert_eq!(e.rows_written, 2, "variant A doubles vertically");
                        // Predictor bytes: row 0 zeros EXCEPT byte 0,
                        // which the §3.2 prologue seeded from the
                        // static table before the load.
                        let seeded = strip[0];
                        let pred = u32::from_le_bytes([seeded, 0, 0, 0]);
                        assert_eq!(e.pixels, pred.wrapping_add(0x0102_0304));
                    }
                    other => panic!("expected dyad, got {other:?}"),
                }
                // Both rows carry the stored DWORD.
                assert_eq!(strip[top..top + 4], strip[top + STRIDE..top + STRIDE + 4]);
            }
            other => panic!("expected Complete, got {other:?}"),
        }
    }

    #[test]
    fn sentinel_consumes_continuation_byte_and_secondary_word() {
        let table = DyadDeltaTable::load();
        // Primary delta with bit 31 set → sentinel → continuation.
        // The continuation byte 0x07 re-indexes the band's secondary
        // word. After the xor the low half is 0x8080 (the seeded
        // predictor byte + the flipped bit 15), so the secondary must
        // carry it past 0x10000 to clear the sign: 0x8100 →
        // 0x8080 + 0x8100 = 0x0180 (wrapped), bit 15 clear.
        let arena = arena_with(2, 0, 0x8000_0000, 0x07, 0x8100);
        let top = STRIDE;
        let mut strip = vec![0u8; STRIDE * 6];
        // Mode byte 0x02: high nibble 0, low nibble 2 (bit 3 clear →
        // table 2 → 0x10006c9c, canonical).
        let run = unpack_cell(
            &mut strip,
            geom(1, 1, top),
            CellVariant::WithEdge,
            &[0x02, 0x07],
            &table,
            &arena,
            false,
        )
        .unwrap();
        assert_eq!(run.bytes_consumed, 2, "mode byte + continuation byte");
        match &run.outcome {
            UnpackOutcome::Complete(effects) => match effects[0] {
                UnpackEffect::Dyad(e) => {
                    assert!(e.continuation);
                    assert_eq!(e.rows_written, 1, "variant B emits one row");
                }
                other => panic!("expected dyad, got {other:?}"),
            },
            other => panic!("expected Complete, got {other:?}"),
        }
    }

    #[test]
    fn still_negative_low_half_is_error_code_2() {
        let table = DyadDeltaTable::load();
        // Primary 0x8000_0000: the sum's bit 31 sets the sentinel; the
        // xor then flips the low half's bit 15 ON (0x0080 → 0x8080).
        // The continuation byte 0x40 indexes a zeroed arena slot, so
        // the secondary add leaves bit 15 set → the §2.3 step-3
        // error-code-2 fault.
        let arena = arena_with(0, 0, 0x8000_0000, 0x40, 0x0000);
        let top = STRIDE;
        let mut strip = vec![0u8; STRIDE * 6];
        let err = unpack_cell(
            &mut strip,
            geom(1, 1, top),
            CellVariant::Plain,
            &[0x00, 0x40],
            &table,
            &arena,
            false,
        )
        .unwrap_err();
        assert_eq!(err, CellUnpackError::DyadRangeFault { row: 0, dword: 0 });
    }

    #[test]
    fn fault_slot_mode_byte_is_error_code_1() {
        let table = DyadDeltaTable::load();
        let arena = VqArena::new();
        let top = STRIDE;
        let mut strip = vec![0u8; STRIDE * 6];
        // 0x28: high nibble 2 → fault in both tables.
        let err = unpack_cell(
            &mut strip,
            geom(1, 1, top),
            CellVariant::Plain,
            &[0x28],
            &table,
            &arena,
            false,
        )
        .unwrap_err();
        assert_eq!(
            err,
            CellUnpackError::ModeByteFault {
                mode_byte: 0x28,
                row: 0,
                dword: 0,
            }
        );
    }

    #[test]
    fn unstaged_handler_defers_with_rva() {
        let table = DyadDeltaTable::load();
        let arena = VqArena::new();
        let top = STRIDE;
        let mut strip = vec![0u8; STRIDE * 6];
        // 0x18: high nibble 1 → 0x10006c90 (first-row dyad/quad emit),
        // pinned but unstaged past the dispatch level.
        let run = unpack_cell(
            &mut strip,
            geom(1, 1, top),
            CellVariant::Plain,
            &[0x18],
            &table,
            &arena,
            false,
        )
        .unwrap();
        assert_eq!(run.bytes_consumed, 0, "cursor backed up to the frontier");
        match run.outcome {
            UnpackOutcome::DeferredHandler {
                mode_byte,
                handler_rva,
                ..
            } => {
                assert_eq!(mode_byte, 0x18);
                assert_eq!(handler_rva, Some(0x1000_6c90));
            }
            other => panic!("expected DeferredHandler, got {other:?}"),
        }
    }

    #[test]
    fn table1_high_nibble_3_rides_the_canonical_handler() {
        let table = DyadDeltaTable::load();
        // 0x38: high nibble 3, low nibble 8 (bit 3 set → table 1 →
        // 0x10006c14, canonical). Seed bank 3 of the static table.
        let arena = arena_with(8, 0, 0x0000_0000, 0, 0);
        let top = STRIDE;
        let mut strip = vec![0u8; STRIDE * 6];
        let run = unpack_cell(
            &mut strip,
            geom(1, 1, top),
            CellVariant::Plain,
            &[0x38],
            &table,
            &arena,
            false,
        )
        .unwrap();
        match &run.outcome {
            UnpackOutcome::Complete(effects) => match effects[0] {
                UnpackEffect::Dyad(e) => {
                    // Bank-3 static-table seed (spec/07 §3.1 index
                    // (3 << 9) + 0).
                    let expect = table.as_bytes()[3 << 9] as i8;
                    assert_eq!(e.seed_delta, expect);
                }
                other => panic!("expected dyad, got {other:?}"),
            },
            other => panic!("expected Complete, got {other:?}"),
        }
    }

    #[test]
    fn escapes_and_carry_share_the_stateful_protocol() {
        let table = DyadDeltaTable::load();
        let arena = VqArena::new();
        let top = STRIDE;
        let mut strip = vec![0u8; STRIDE * 8];
        // 0xFC carries the next-cell skip.
        let run = unpack_cell(
            &mut strip,
            geom(1, 2, top),
            CellVariant::Plain,
            &[0xFC],
            &table,
            &arena,
            false,
        )
        .unwrap();
        assert!(run.next_cell_skip);
        // A carried flag consumes the cell without bytes.
        let run = unpack_cell(
            &mut strip,
            geom(1, 2, top),
            CellVariant::Plain,
            &[0x00],
            &table,
            &arena,
            true,
        )
        .unwrap();
        assert_eq!(run.outcome, UnpackOutcome::SkippedByCarry);
        assert_eq!(run.bytes_consumed, 0);
        // §4.4 corrected: 0xFB runs in-cell. A 3-position repeat run
        // over a 1-dword x 2-row cell exhausts the cell (2 positions
        // emitted, counter bounded by the cell) and completes it.
        let run = unpack_cell(
            &mut strip,
            geom(1, 2, top),
            CellVariant::Plain,
            &[0xFB, 0x03],
            &table,
            &arena,
            false,
        )
        .unwrap();
        match run.outcome {
            UnpackOutcome::Complete(effects) => {
                assert!(matches!(
                    effects[0],
                    UnpackEffect::Static(PositionEffect::FbRun { positions: 2, .. })
                ));
            }
            other => panic!("expected Complete, got {other:?}"),
        }
        assert_eq!(run.bytes_consumed, 2);
        // Invalid counters are the binary's error return.
        let err = unpack_cell(
            &mut strip,
            geom(1, 2, top),
            CellVariant::Plain,
            &[0xFB, 0x20],
            &table,
            &arena,
            false,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            CellUnpackError::FbCounterInvalid { counter: 0x20, .. }
        ));
    }

    #[test]
    fn zero_dimension_and_exhaustion_are_typed() {
        let table = DyadDeltaTable::load();
        let arena = VqArena::new();
        let mut strip = vec![0u8; STRIDE * 4];
        assert_eq!(
            unpack_cell(
                &mut strip,
                geom(0, 1, STRIDE),
                CellVariant::Plain,
                &[],
                &table,
                &arena,
                false,
            )
            .unwrap_err(),
            CellUnpackError::ZeroDimension { is_width: true }
        );
        assert!(matches!(
            unpack_cell(
                &mut strip,
                geom(1, 2, STRIDE),
                CellVariant::WithEdge,
                &[0x00],
                &table,
                &arena,
                false,
            )
            .unwrap_err(),
            CellUnpackError::ByteStreamExhausted { .. }
        ));
    }
}
