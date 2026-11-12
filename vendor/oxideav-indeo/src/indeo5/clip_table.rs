//! Indeo 5 per-cell saturation clipping table.
//!
//! Spec source: `docs/video/indeo/indeo5/spec/06-slant-inverse-transform.md`
//! §5.3 (audit-corrected per `audit/00 §3.3`).
//!
//! After the per-handler inverse-Slant arithmetic, a reconstructed
//! coefficient may have a magnitude outside the displayable 8-bit pixel
//! range. The decoder saturates it through a small per-MB clipping
//! table built at the per-block prologue (`IR50_32.DLL!0x1001f421..
//! 0x1001f452`). The audit (`audit/00 §3.3`) corrected the table's
//! storage location to the per-block stack frame (`[esp+0x1c0+eax]` for
//! `eax ∈ [-0x30, 0)`, i.e. 48 bytes), not the `.bss 0x1009deb8` region
//! whose populator the audit could not locate.
//!
//! ## Construction (`spec/06 §5.3`)
//!
//! ```text
//! eax = -0x30                     ; loop counter, runs from -48 to -1
//! while (eax < 0):
//!   ebx = clip_input              ; per-MB shift count
//!   ebx += eax                    ; current cell value (signed)
//!   ebx += 0x18                   ; bias for unsigned range
//!   if (ebx < 0)    ebx = 0       ; lower clamp
//!   if (ebx > 0x17) ebx = 0x17    ; upper clamp
//!   ebx -= clip_input             ; un-bias
//!   ebx += 0x80                   ; centre at pixel 128
//!   [esp+eax+0x1c0] = bl          ; write clipped pixel
//!   ++eax
//! ```
//!
//! The result is a 48-entry lookup mapping signed cell magnitudes (the
//! `eax` counter, `[-48, -1]`, interpreted as offsets `[-24, +23]`
//! around mid-grey once biased) to clipped 8-bit pixel values centred
//! on `0x80`. The table is consumed by the per-row-pass byte-lookup at
//! `IR50_32.DLL!0x10031125`.
//!
//! This builder is fully specified by §5.3; no docs gap. The
//! `clip_input` (the per-MB combined shift count
//! `band_glob_quant + mb_qdelta`, `spec/06 §5.2`) is a parameter — its
//! derivation from a real bitstream depends on the gated coefficient
//! path, but the table-build given a known `clip_input` is exact.

/// The number of entries in the per-cell clipping table (`spec/06
/// §5.3`): the loop runs the counter over `[-0x30, 0)`.
pub const CLIP_TABLE_LEN: usize = 0x30;

/// Spec/06 §5.3 — the lower clamp bound applied to the biased value.
pub const CLIP_LOWER: i32 = 0;

/// Spec/06 §5.3 — the upper clamp bound applied to the biased value.
pub const CLIP_UPPER: i32 = 0x17;

/// Spec/06 §5.3 — the unsigned-range bias added before clamping.
pub const CLIP_BIAS: i32 = 0x18;

/// Spec/06 §5.3 — the pixel-domain centre added after un-biasing.
pub const CLIP_PIXEL_CENTRE: i32 = 0x80;

/// Build the 48-entry per-cell clipping table for a given per-MB shift
/// count `clip_input` (`spec/06 §5.3`).
///
/// Entry `k` (for `k` in `0..48`) corresponds to the binary's counter
/// value `eax = k - 0x30` (the binary writes `[esp + eax + 0x1c0]`, so
/// counter `-0x30` lands at relative index 0 and counter `-1` at index
/// 47). Each entry is the clipped 8-bit pixel value (`u8`).
pub fn build_clip_table(clip_input: i32) -> [u8; CLIP_TABLE_LEN] {
    let mut table = [0u8; CLIP_TABLE_LEN];
    // The binary's loop counter runs eax = -0x30 .. -1. Relative index
    // k = eax + 0x30 maps that to 0..48.
    for (k, slot) in table.iter_mut().enumerate() {
        // Counter eax runs -0x30 .. -1; relative index k = eax + 0x30.
        let eax = k as i32 - CLIP_TABLE_LEN as i32;
        // biased = clamp(clip_input + eax + 0x18, 0, 0x17), then un-bias
        // and centre on the mid-grey pixel (spec/06 §5.3).
        let biased = (clip_input + eax + CLIP_BIAS).clamp(CLIP_LOWER, CLIP_UPPER);
        *slot = (biased - clip_input + CLIP_PIXEL_CENTRE) as u8;
    }
    table
}

/// Clip a signed cell magnitude `value` (the binary's `eax` counter,
/// `[-48, -1]`) through a built table (`spec/06 §5.3`). Returns the
/// clipped 8-bit pixel, or `None` when `value` is outside `[-48, -1]`.
pub fn clip_lookup(table: &[u8; CLIP_TABLE_LEN], value: i32) -> Option<u8> {
    if !(-(CLIP_TABLE_LEN as i32)..0).contains(&value) {
        return None;
    }
    let k = (value + CLIP_TABLE_LEN as i32) as usize;
    Some(table[k])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_length() {
        let t = build_clip_table(0);
        assert_eq!(t.len(), CLIP_TABLE_LEN);
        assert_eq!(CLIP_TABLE_LEN, 48);
    }

    #[test]
    fn clip_input_zero_progression() {
        // clip_input = 0: ebx = eax + 0x18, clamped to [0, 0x17], + 0x80.
        // eax = -0x30 (k=0): ebx = -48 + 24 = -24 -> clamp 0 -> +0x80 = 128.
        let t = build_clip_table(0);
        assert_eq!(t[0], 0x80);
        // eax = -0x18 (k = 0x18): ebx = -24 + 24 = 0 -> 0 -> +0x80 = 128.
        assert_eq!(t[0x18], 0x80);
        // eax = -1 (k = 47): ebx = -1 + 24 = 23 = 0x17 -> clamp ok ->
        //   un-bias 23 - 0 = 23 -> +0x80 = 128 + 23 = 151.
        assert_eq!(t[47], 0x80 + 23);
    }

    #[test]
    fn clip_input_zero_unclamped_region() {
        // For clip_input=0, the biased value ebx = eax + 0x18 is in
        // [0, 0x17] when eax in [-0x18, -1]; in that region the result is
        // 0x80 + (eax + 0x18) = 0x80 + eax + 24.
        let t = build_clip_table(0);
        for (k, &entry) in t.iter().enumerate().skip(0x18) {
            let eax = k as i32 - CLIP_TABLE_LEN as i32;
            let expected = (0x80 + eax + 24) as u8;
            assert_eq!(entry, expected, "k={k}");
        }
        // The lower region (eax < -0x18) all clamps to 0x80.
        for (k, &entry) in t.iter().enumerate().take(0x18) {
            assert_eq!(entry, 0x80, "k={k}");
        }
    }

    #[test]
    fn clip_input_shifts_clamp_window() {
        // A non-zero clip_input shifts where the clamp activates: the
        // clamp is on (clip_input + eax + 0x18), then clip_input is
        // subtracted back, so the net unclamped result is independent of
        // clip_input but the clamp boundaries move.
        let t0 = build_clip_table(0);
        let t5 = build_clip_table(5);
        // In the fully-unclamped middle the two agree (clip_input cancels).
        // eax such that both 0+eax+0x18 and 5+eax+0x18 are within [0,0x17]:
        // need eax+0x18 in [0,0x17] and eax+0x1d in [0,0x17] -> eax in
        // [-0x18,-1] ∩ [-0x1d,-6] = [-0x18,-6].
        for (k, (&a, &b)) in t0
            .iter()
            .zip(t5.iter())
            .enumerate()
            .take(CLIP_TABLE_LEN - 5)
            .skip(0x18)
        {
            assert_eq!(a, b, "k={k}");
        }
    }

    #[test]
    fn clip_values_centred_on_pixel_band() {
        // result = 0x80 + clamp(clip_input + eax + 0x18, 0, 0x17) -
        // clip_input. The clamped term spans [0, 0x17], so the result
        // spans exactly [0x80 - ci, 0x97 - ci].
        for ci in [-10, 0, 5, 31] {
            let t = build_clip_table(ci);
            let lo = (0x80 - ci) as u8;
            let hi = (0x97 - ci) as u8;
            for &v in &t {
                assert!(v >= lo && v <= hi, "ci={ci} v={v} lo={lo} hi={hi}");
            }
        }
    }

    #[test]
    fn clip_lookup_maps_counter() {
        let t = build_clip_table(0);
        assert_eq!(clip_lookup(&t, -0x30), Some(t[0]));
        assert_eq!(clip_lookup(&t, -1), Some(t[47]));
        // Out of range.
        assert_eq!(clip_lookup(&t, 0), None);
        assert_eq!(clip_lookup(&t, -0x31), None);
    }
}
