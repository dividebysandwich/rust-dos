//! Indeo 5 motion vectors — packed layout, half-pel mode, predictor.
//!
//! Spec source: `docs/video/indeo/indeo5/spec/07-motion-compensation.md`
//! §2.1 (per-MB MV count), §2.2 (per-band MV resolution), §2.4 (packed
//! MV layout), §3.2 (left-neighbour spatial predictor), §3.3 (tile-entry
//! reset).
//!
//! Indeo 5 carries **one motion vector per macroblock** (`spec/07
//! §2.1`); all blocks of a 4-block MB share it. Each component is a
//! signed value in `[-128, +127]` (`spec/07 §2.4` — the level zig-zag
//! table's `±0x80` range), in **half-pel units** when the band's
//! `mv_res` flag is set (`spec/07 §2.2`), so `±64` pixels.
//!
//! ## Packed layout (`spec/07 §2.4`)
//!
//! | Bits   | Field           |
//! | ------ | --------------- |
//! | 0..=7  | Δy (signed byte; bit 7 = sign) |
//! | 8..=15 | Δx (signed byte; bit 15 = sign) |
//! | 23     | `delta_present` |
//! | 24..   | state flags consumed at the fetcher entry |
//!
//! ## Half-pel mode (`spec/07 §2.2`)
//!
//! With `mv_res = 1` each component's LSB is its half-pel flag; the MC
//! fetcher extracts the flag pair into a 2-bit kernel selector
//! (`ecx & 3` after the `sar; rcl ecx` sequence): `0` full-pel, `1`
//! half-pel-X, `2` half-pel-Y, `3` the 2D half-pel position ("average
//! of four pels" — there is no true quarter-pel mode). The remaining
//! bits (arithmetic `>> 1`) are the full-pel displacement.
//!
//! ## Predictor (`spec/07 §3.2`/`§3.3`)
//!
//! The spatial predictor is **left-neighbour only** (the
//! previously-decoded MB's MV in the same tile, via the per-tile MV
//! ring), *not* a three-neighbour median. The ring is re-initialised
//! to a zero MV at each tile entry, so the first coded MB of every
//! tile has a zero predictor; tiles and bands do not share predictor
//! history.

/// `spec/07 §2.4` — the `delta_present` bit in the packed MV.
pub const MV_DELTA_PRESENT: u32 = 1 << 23;

/// A per-MB motion vector (`spec/07 §2.1`/`§2.4`): signed components
/// in `[-128, +127]`, in half-pel units when the band's `mv_res` is
/// set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mv {
    /// Horizontal component (Δx).
    pub x: i8,
    /// Vertical component (Δy).
    pub y: i8,
}

impl Mv {
    /// The zero MV (`spec/07 §3.3` tile-entry reset value).
    pub const ZERO: Mv = Mv { x: 0, y: 0 };

    /// `spec/07 §2.4` — pack into the low 16 bits of the per-MB MV
    /// slot (`Δy` in bits 0..=7, `Δx` in bits 8..=15) with the
    /// `delta_present` bit set.
    pub fn pack(self) -> u32 {
        (self.y as u8 as u32) | ((self.x as u8 as u32) << 8) | MV_DELTA_PRESENT
    }

    /// `spec/07 §2.4` — unpack from a packed per-MB MV slot value.
    /// Returns `None` when the `delta_present` bit is clear (no MV
    /// delta was decoded for this MB).
    pub fn unpack(packed: u32) -> Option<Mv> {
        if packed & MV_DELTA_PRESENT == 0 {
            return None;
        }
        Some(Mv {
            y: (packed & 0xff) as u8 as i8,
            x: ((packed >> 8) & 0xff) as u8 as i8,
        })
    }

    /// `spec/07 §3.2` — apply a decoded delta to this (predictor) MV,
    /// producing the MB's final MV. Component arithmetic wraps in the
    /// signed-byte range (the binary's 8-bit adds).
    pub fn apply_delta(self, delta: Mv) -> Mv {
        Mv {
            x: self.x.wrapping_add(delta.x),
            y: self.y.wrapping_add(delta.y),
        }
    }
}

/// `spec/07 §2.2` — the per-band MV resolution (`mv_res`,
/// `[band+0x1c]`, `spec/02 §1.7` bit 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MvResolution {
    /// `mv_res = 0` — one pixel per MV unit.
    FullPel,
    /// `mv_res = 1` — half a pixel per MV unit; each component's LSB
    /// is its half-pel flag.
    HalfPel,
}

/// `spec/07 §2.2`/`§5.2` — the MC-kernel selector (`ecx & 3` at the
/// fetcher entry).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McMode {
    /// `0` — full-pel fetch (no interpolation).
    FullPel,
    /// `1` — half-pel X (average with the next column).
    HalfPelX,
    /// `2` — half-pel Y (average with the next row).
    HalfPelY,
    /// `3` — 2D half-pel (average of four pels). Not a true
    /// quarter-pel mode (`spec/07 §2.2`).
    HalfPelXY,
}

impl McMode {
    /// The raw `ecx & 3` selector value (`spec/07 §2.2`).
    #[inline]
    pub fn selector(self) -> u32 {
        match self {
            McMode::FullPel => 0,
            McMode::HalfPelX => 1,
            McMode::HalfPelY => 2,
            McMode::HalfPelXY => 3,
        }
    }

    /// Build from the raw 2-bit selector.
    #[inline]
    pub fn from_selector(sel: u32) -> McMode {
        match sel & 3 {
            0 => McMode::FullPel,
            1 => McMode::HalfPelX,
            2 => McMode::HalfPelY,
            _ => McMode::HalfPelXY,
        }
    }
}

/// A motion vector resolved to a full-pel displacement plus the MC
/// interpolation mode (`spec/07 §2.2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedMv {
    /// Full-pel horizontal displacement.
    pub dx: i32,
    /// Full-pel vertical displacement.
    pub dy: i32,
    /// The MC kernel to apply at the displaced position.
    pub mode: McMode,
}

/// `spec/07 §2.2` — resolve an MV against the band's `mv_res`: at
/// full-pel resolution the components are used directly with the
/// full-pel kernel; at half-pel resolution each component's LSB
/// becomes its half-pel flag (folded into the `ecx & 3` kernel
/// selector — bit 0 = X, bit 1 = Y) and the remaining bits (arithmetic
/// shift right, the binary's `sar`) are the full-pel displacement.
pub fn resolve_mv(mv: Mv, res: MvResolution) -> ResolvedMv {
    match res {
        MvResolution::FullPel => ResolvedMv {
            dx: mv.x as i32,
            dy: mv.y as i32,
            mode: McMode::FullPel,
        },
        MvResolution::HalfPel => {
            let half_x = (mv.x & 1) as u32;
            let half_y = (mv.y & 1) as u32;
            ResolvedMv {
                dx: (mv.x as i32) >> 1,
                dy: (mv.y as i32) >> 1,
                mode: McMode::from_selector(half_x | (half_y << 1)),
            }
        }
    }
}

/// `spec/07 §3.2`/`§3.3` — the per-tile left-neighbour MV predictor.
///
/// Models the per-tile MV ring's sliding one-MV window: construction
/// (= tile entry) seeds a zero MV, [`predict`](MvPredictor::predict)
/// returns the previously-decoded MB's MV, and
/// [`update`](MvPredictor::update) records a just-decoded MB's final
/// MV. Tiles do not share history — build a fresh predictor per tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MvPredictor {
    last: Mv,
}

impl MvPredictor {
    /// Tile-entry construction (`spec/07 §3.3`) — the ring's first
    /// slot holds the zero MV.
    pub fn new() -> Self {
        MvPredictor { last: Mv::ZERO }
    }

    /// The predictor for the next MB (`spec/07 §3.2` — the
    /// left-neighbour, i.e. previously-decoded, MB's MV).
    #[inline]
    pub fn predict(&self) -> Mv {
        self.last
    }

    /// Record a decoded MB's final MV as the next MB's predictor.
    #[inline]
    pub fn update(&mut self, mv: Mv) {
        self.last = mv;
    }

    /// Decode step for one MB: apply `delta` to the current predictor,
    /// record and return the final MV (`spec/07 §3.2`). A skipped MB
    /// passes a zero delta (`spec/07 §6.1` — skip inherits the
    /// left-neighbour's MV).
    pub fn decode_mb(&mut self, delta: Mv) -> Mv {
        let mv = self.predict().apply_delta(delta);
        self.update(mv);
        mv
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_unpack_round_trip() {
        // spec/07 §2.4 bit layout.
        for &(x, y) in &[(0i8, 0i8), (1, -1), (-128, 127), (64, -64)] {
            let mv = Mv { x, y };
            let packed = mv.pack();
            assert!(packed & MV_DELTA_PRESENT != 0);
            assert_eq!(Mv::unpack(packed), Some(mv), "({x},{y})");
        }
    }

    #[test]
    fn pack_layout_fields() {
        // Δy in bits 0..=7, Δx in bits 8..=15 (spec/07 §2.4).
        let packed = Mv { x: 0x12, y: 0x34 }.pack();
        assert_eq!(packed & 0xff, 0x34);
        assert_eq!((packed >> 8) & 0xff, 0x12);
        assert_eq!(packed & MV_DELTA_PRESENT, MV_DELTA_PRESENT);
    }

    #[test]
    fn unpack_without_delta_present_is_none() {
        assert_eq!(Mv::unpack(0x0000_1234), None);
    }

    #[test]
    fn resolve_full_pel() {
        // mv_res = 0: components used directly, full-pel kernel.
        let r = resolve_mv(Mv { x: 5, y: -3 }, MvResolution::FullPel);
        assert_eq!((r.dx, r.dy), (5, -3));
        assert_eq!(r.mode, McMode::FullPel);
    }

    #[test]
    fn resolve_half_pel_modes() {
        // spec/07 §2.2: LSB pair -> ecx & 3 selector.
        let r = resolve_mv(Mv { x: 4, y: 6 }, MvResolution::HalfPel);
        assert_eq!((r.dx, r.dy, r.mode), (2, 3, McMode::FullPel));
        let r = resolve_mv(Mv { x: 5, y: 6 }, MvResolution::HalfPel);
        assert_eq!((r.dx, r.dy, r.mode), (2, 3, McMode::HalfPelX));
        let r = resolve_mv(Mv { x: 4, y: 7 }, MvResolution::HalfPel);
        assert_eq!((r.dx, r.dy, r.mode), (2, 3, McMode::HalfPelY));
        let r = resolve_mv(Mv { x: 5, y: 7 }, MvResolution::HalfPel);
        assert_eq!((r.dx, r.dy, r.mode), (2, 3, McMode::HalfPelXY));
    }

    #[test]
    fn resolve_half_pel_negative_uses_arithmetic_shift() {
        // -1 (0xff): half flag set, full-pel part -1 >> 1 = -1 (sar).
        let r = resolve_mv(Mv { x: -1, y: -2 }, MvResolution::HalfPel);
        assert_eq!((r.dx, r.dy), (-1, -1));
        assert_eq!(r.mode, McMode::HalfPelX); // x LSB=1, y LSB=0
        // -3 -> sar 1 = -2, LSB=1.
        let r = resolve_mv(Mv { x: -3, y: -3 }, MvResolution::HalfPel);
        assert_eq!((r.dx, r.dy, r.mode), (-2, -2, McMode::HalfPelXY));
    }

    #[test]
    fn mc_mode_selector_round_trip() {
        for sel in 0..4 {
            assert_eq!(McMode::from_selector(sel).selector(), sel);
        }
    }

    #[test]
    fn predictor_starts_zero_and_slides() {
        // spec/07 §3.2/§3.3: zero at tile entry, left-neighbour after.
        let mut p = MvPredictor::new();
        assert_eq!(p.predict(), Mv::ZERO);
        // First MB: delta (3, -2) over zero predictor.
        let mv1 = p.decode_mb(Mv { x: 3, y: -2 });
        assert_eq!(mv1, Mv { x: 3, y: -2 });
        // Second MB: delta (1, 1) over the first MB's MV.
        let mv2 = p.decode_mb(Mv { x: 1, y: 1 });
        assert_eq!(mv2, Mv { x: 4, y: -1 });
        // Skip MB (zero delta) inherits the left-neighbour (spec/07 §6.1).
        let mv3 = p.decode_mb(Mv::ZERO);
        assert_eq!(mv3, mv2);
    }

    #[test]
    fn fresh_predictor_per_tile() {
        // spec/07 §3.3: tiles do not share history.
        let mut p = MvPredictor::new();
        p.decode_mb(Mv { x: 9, y: 9 });
        let p2 = MvPredictor::new();
        assert_eq!(p2.predict(), Mv::ZERO);
    }

    #[test]
    fn apply_delta_wraps_in_signed_byte_range() {
        let mv = Mv { x: 127, y: -128 }.apply_delta(Mv { x: 1, y: -1 });
        assert_eq!(mv, Mv { x: -128, y: 127 });
    }
}
