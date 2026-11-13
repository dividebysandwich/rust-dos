# oxideav-indeo

[![CI](https://github.com/OxideAV/oxideav-indeo/actions/workflows/ci.yml/badge.svg)](https://github.com/OxideAV/oxideav-indeo/actions/workflows/ci.yml) [![crates.io](https://img.shields.io/crates/v/oxideav-indeo.svg)](https://crates.io/crates/oxideav-indeo) [![docs.rs](https://docs.rs/oxideav-indeo/badge.svg)](https://docs.rs/oxideav-indeo) [![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Pure-Rust Intel Indeo (IV2 / IV3 / IV4 / IV5) video codec family for the
[oxideav](https://github.com/OxideAV/oxideav-workspace) framework. Built
from clean-room specification and behavioural-trace documents under
`docs/video/indeo/` only — no external codec source is consulted.

## Status

Two of the four Indeo generations decode real streams, both validated
pixel-for-pixel against black-box reference decodes of the fixtures
staged under `docs/video/indeo/`:

| Codec | FourCC | State |
| ----- | ------ | ----- |
| Indeo 3 | `IV31` / `IV32` | **Decodes.** Both staged corpora (160×120 all-intra, 176×144 with a 4-frame intra period) reproduce the reference decode **pixel-exact on every frame and every plane** — 16 / 16 frames: intra (families A / E), inter (motion compensation with full- and half-pel vectors, families B / F), VQ_NULL cells, NULL-frame repeat, the two-bank reference ping-pong. |
| Indeo 5 | `IV50` | **Decodes.** Intra, inter and NULL frames, one-band and four-band luma, transparency windows. On the 40 IV50 movies of Rent-A-Hero (5 245 frames) the 8-bit planes equal the vendor driver's on every frame of 22 movies; in the other 18, all with transparency, the only difference is one run of at most 31 luma pixels that the driver never initialises. |
| Indeo 2 / 4 | `RT21` / `IV41` | Wiki-snapshot documentation only under `docs/video/indeo/indeoN/wiki/` (no formal `spec/`); scaffold pending docs work. |

All truth is drawn from the staged spec / trace docs — no external
codec source is consulted; where the spec text leaves a value-level
choice open, the fixtures arbitrate it and the choice is recorded in
the module docs and the CHANGELOG.

### Indeo 3 (`IV31` / `IV32`)

Entry points: `indeo3::Indeo3PictureDecoder` (stateful; `decode(frame)
-> DecodedPicture` with native 4:1:0 planes), the `oxideav_core`
bridge `indeo3::Indeo3RegistryDecoder` / `decode_video_frame`
(`PixelFormat::Yuv444P`, chroma box-replicated 4×4 per `spec/07
§5.5`), and the building blocks `indeo3::decode_plane` /
`PlaneBuffers` / `PlaneBanks`.

How a frame decodes (r459):

- `spec/01` / `spec/02` — the 64-byte combined header, the per-plane
  preludes (motion-vector tables) and the per-frame codebook arena
  rebuilt from `alt_quant[]` over the codec-init staging image
  (`spec/04 §5.2` / `§6`, reproduced word-for-word from the seed area).
- `spec/04 §5.3` — the **cell-geometry banks**: five sub-tables per
  plane, regenerated from the picture size by the vendor's populator
  rule (4 080 / 4 080 staged entries, `tests/indeo3_geometry_banks.rs`);
  every leaf of the binary tree is positioned through them (full-strip
  or last-strip bank by strip slot).
- `spec/03` — the MSB-first binary-tree walk with the vertical /
  horizontal heap indices; the tree bits and the byte-level reads share
  one cursor whose accumulator keeps a tree byte's leftover bits across
  a cell's byte stream.
- `spec/06` / `spec/07` — a coded cell is one mode byte (nibble tables
  → family, codebook base, LUT rewrite of the row above for band
  nibbles ≥ 8, which also selects the even seed sets, staging blocks
  8..15) followed by codes per 4×4 block (A / B) or 8×8 block (E / F),
  four per block, row-group major, with the eight `0xF8..0xFF` escapes
  at their block positions (`0xFB` runs count blocks); a code's word is
  applied dyad by dyad, the raw `pred + word` bit 31 selecting the
  two-byte form. E doubles rows horizontally and stores `(avg(above,
  row), row)` pairs (repeating at the strip top); F adds the doubled
  deltas per pixel to both rows over the motion-compensated content.
- `spec/05` — INTER leaves fetch the reference cell by the packed
  byte-offset vector (`176 · vert + horiz`, bits 0 / 1 the half-pel
  filters, truncating averages), then take their residual through B /
  F; a VQ_NULL on an INTER cell keeps the fetched content.
- Output: 7-bit samples upshifted (`spec/07 §4.3`), chroma at
  `ceil(w / 4) × ceil(h / 4)`.

Not exercised by the fixtures (implemented per spec, unvalidated or
reported): families C / D (8-row × 4-pixel cells — reported as
`CellDecodeError::UnsupportedFamily`, not guessed), pictures wider
than 320 (three-strip wide flag), the `0xF8` literal fill, RGB host
output (the `spec/07 §5.4` LUT is unstaged; not needed for YUV
output).

### Indeo 5 (`IV50`)

Entry points: `indeo5::Iv50Decoder` (stateful; `decode(frame) ->
FrameType`, then the `y()`, `u()` and `v()` planes) and the
`oxideav_core` bridge `indeo5::Indeo5RegistryDecoder` /
`decode_video_frame` (`PixelFormat::Yuv444P`, each chroma sample
repeated over its 4×4 luma pixels).

The decoder was written from a clean-room specification of the vendor
driver IR50_32.DLL (R.5.06.15.2.44, MMX path), with no other decoder's
source consulted, and reproduces that driver: picture start and the
repeated-number NULL rule, GOP, frame and band headers, the row-width
prefix codes, macroblock headers with quantiser deltas and inherited
vectors, run/value tables with corrections and escapes,
dequantisation, DC prediction, the inverse Slant transforms,
half-sample motion compensation from the rotating reference buffers,
the driver's four-band synthesis filter, and the transparency bounding
box that limits which part of the picture a frame rebuilds. The planes
keep what earlier frames wrote outside that box, as the driver's do.

Refused with `Iv50Error::Unsupported` because no known stream uses them
and their decoding isn't established: several tiles per band, half-size
chroma, locked streams (access key), seven-band luma and four-band
chroma, bitstream versions other than 2, and a few inheritance cases
the driver handles in ways not yet traced.

## Selected public API

- `indeo3::Indeo3PictureDecoder::decode` → `DecodedPicture` (`luma`,
  `chroma_v`, `chroma_u`, `repeated_previous`, per-plane `PlaneStats`;
  `to_yuv444_planes()`).
- `indeo3::decode_plane` / `PlaneBuffers` / `PlaneContext` — one plane
  through the cell decoder over its strip buffers; `PlaneBanks` /
  `GeometryBank` / `split_extent` / `chroma_plane_dims` — the geometry
  banks.
- `indeo3::FrameHeader::parse`, `PictureLayer::parse`, `StagingImage`,
  `VqArena::apply_alt_quant`, `DyadDeltaTable` — the header, picture
  layer and codec-init tables.
- `indeo3::DecodeSession` — the inter-frame sequencer (first-frame /
  seek INTRA gate, NULL repeat, reference bank).
- `indeo5::Iv50Decoder::decode` → `FrameType`; `y()` / `u()` / `v()`
  → `Iv50Plane` (width, height, stride, samples); `Iv50Error`.
- Registry: `register` (crate root), `indeo3::register_codecs` /
  `indeo5::register`, `codec_id_for_fourcc`, `probe`.

The crate forbids `unsafe` (`#![forbid(unsafe_code)]`).

## License

MIT.
