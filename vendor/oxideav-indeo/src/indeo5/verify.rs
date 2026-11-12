//! Indeo 5 reconstruction checksums (`spec/08 §7`, formula recovered by
//! black-box validation).
//!
//! `spec/08 §7` documents a per-frame `frm_checksum` and a per-band
//! `band_checksum`, both 16-bit, both parsed-and-stored, and — as
//! [`super::checksum`] models — **never verified** by the shipping
//! decoder (a byte-exact black-box corruption test confirms a frame
//! with a mangled stored checksum decodes to identical pixels). The
//! spec therefore did not need — and did not stage — the arithmetic
//! that *produces* those checksums.
//!
//! This module supplies that arithmetic as a **decoder-side acceptance
//! oracle**: given a fully reconstructed frame, recomputing the
//! checksums and matching them against the stream's stored values is a
//! byte-sum-exact end-to-end reconstruction check. The formulas were
//! recovered as numeric observations from the two staged `IV50` INTRA
//! fixtures (the reference `expected.yuv` pixels + the stored checksum
//! values parsed from the bitstream + a black-box `band+0x20` memory
//! read), never from any decoder source:
//!
//! * **Band checksum** — `(Σ (pixel − 128)) & 0xffff` over the band's
//!   reconstructed pixel region. For a plane at 0 decomposition levels
//!   the band *is* the plane, so the luma band checksum is
//!   `Σ (Y − 128)` over the whole luma plane. Verified exact on both
//!   fixtures (`educ` Y `0x2c00`; `indeo5` Y `0xee60`).
//! * **Frame checksum** — `(Σ Y + Σ U + Σ V) & 0xffff` over every
//!   reconstructed sample, chroma taken at its **native** (subsampled)
//!   resolution. Verified exact on both fixtures (`educ` `0x1800`;
//!   `indeo5` `0xc975`).
//!
//! Because the shipping decoder does not enforce these, a mismatch is
//! **not** a decode error — it is a reconstruction-completeness signal:
//! while the coefficient→pixel transform stays at its docs-gap (see
//! [`super::decode`]), a coded band's recomputed checksum will not match
//! its stored value, whereas a genuinely flat band (all-`128` chroma of
//! a black frame) matches exactly. The decoder surfaces this as a
//! per-band / per-frame [`ChecksumStatus`] so the reconstruction
//! frontier is quantitatively pinned rather than merely asserted.

/// Fold a signed pixel-sum accumulator into the stored 16-bit checksum
/// space (`& 0xffff`, two's-complement wrap).
#[inline]
fn fold(sum: i64) -> u16 {
    (sum & 0xffff) as u16
}

/// `spec/08 §7.2` (formula recovered by black-box validation) — the
/// per-band checksum over a band's reconstructed 8-bit pixels:
/// `(Σ (pixel − 128)) & 0xffff`.
pub fn band_checksum(pixels: &[u8]) -> u16 {
    let sum: i64 = pixels.iter().map(|&p| i64::from(p) - 128).sum();
    fold(sum)
}

/// `spec/08 §7.1` (formula recovered by black-box validation) — the
/// per-frame checksum over every reconstructed sample of the three
/// planes at their native resolutions: `(Σ Y + Σ U + Σ V) & 0xffff`.
pub fn frame_checksum(luma: &[u8], chroma_u: &[u8], chroma_v: &[u8]) -> u16 {
    let sum: i64 = luma.iter().map(|&p| i64::from(p)).sum::<i64>()
        + chroma_u.iter().map(|&p| i64::from(p)).sum::<i64>()
        + chroma_v.iter().map(|&p| i64::from(p)).sum::<i64>();
    fold(sum)
}

/// The outcome of matching a recomputed checksum against the stream's
/// stored value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumStatus {
    /// No checksum was stored for this element (nothing to check).
    Absent,
    /// Stored and recomputed values agree — the reconstruction of this
    /// element is byte-sum-exact.
    Match {
        /// The agreed value.
        value: u16,
    },
    /// Stored and recomputed values differ — this element is not yet
    /// fully reconstructed (its coefficient→pixel transform is gated).
    Mismatch {
        /// The value stored in the bitstream.
        stored: u16,
        /// The value recomputed from the reconstructed pixels.
        computed: u16,
    },
}

impl ChecksumStatus {
    /// Compare a stored (optional) checksum against a freshly computed
    /// one.
    pub fn compare(stored: Option<u16>, computed: u16) -> Self {
        match stored {
            None => ChecksumStatus::Absent,
            Some(s) if s == computed => ChecksumStatus::Match { value: s },
            Some(s) => ChecksumStatus::Mismatch {
                stored: s,
                computed,
            },
        }
    }

    /// `true` when the stored checksum was present and matched (the
    /// element reconstructed byte-sum-exactly).
    pub fn verified(self) -> bool {
        matches!(self, ChecksumStatus::Match { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_checksum_of_uniform_plane() {
        // A flat plane of value v over n pixels: Σ(v-128) mod 2^16.
        // Neutral chroma (128) sums to zero.
        assert_eq!(band_checksum(&[128u8; 100]), 0);
        // Black luma (16) over 240*180: (16-128)*43200 = -4838400,
        // &0xffff = 0x2c00 — the educ fixture's stored Y band checksum.
        let plane = vec![16u8; 240 * 180];
        assert_eq!(band_checksum(&plane), 0x2c00);
    }

    #[test]
    fn frame_checksum_of_uniform_black_frame() {
        // educ black frame: Y=16 (240*180), native chroma 128 (60*45
        // each): Σ = 16*43200 + 128*2700 + 128*2700 = 1382400,
        // &0xffff = 0x1800 — the educ fixture's stored frm_checksum.
        let luma = vec![16u8; 240 * 180];
        let cu = vec![128u8; 60 * 45];
        let cv = vec![128u8; 60 * 45];
        assert_eq!(frame_checksum(&luma, &cu, &cv), 0x1800);
    }

    #[test]
    fn status_compare() {
        assert_eq!(ChecksumStatus::compare(None, 5), ChecksumStatus::Absent);
        assert_eq!(
            ChecksumStatus::compare(Some(5), 5),
            ChecksumStatus::Match { value: 5 }
        );
        assert!(ChecksumStatus::compare(Some(5), 5).verified());
        assert_eq!(
            ChecksumStatus::compare(Some(5), 6),
            ChecksumStatus::Mismatch {
                stored: 5,
                computed: 6
            }
        );
        assert!(!ChecksumStatus::compare(Some(5), 6).verified());
    }

    #[test]
    fn fold_wraps_negative() {
        assert_eq!(super::fold(-4838400), 0x2c00);
        assert_eq!(super::fold(0), 0);
        assert_eq!(super::fold(0x1_0000), 0);
    }
}
