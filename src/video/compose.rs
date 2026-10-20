//! Restore changed base rows and the rows under temporary overlays.

use super::{Frame, mono::Monochrome};
use std::ops::Range;

/// The smallest band containing both bands, ignoring empty ones.
pub fn union(a: Range<usize>, b: Range<usize>) -> Range<usize> {
    if a.is_empty() {
        b
    } else if b.is_empty() {
        a
    } else {
        a.start.min(b.start)..a.end.max(b.end)
    }
}

/// What was drawn over a persistent composition last time. Its pixels
/// must stay in the output frame between calls, including after presenting
/// it. Replacing that frame requires a new Composition.
#[derive(Default)]
pub struct Composition {
    overlay: Range<usize>,
    size: Option<(u32, u32)>,
    monochrome: Option<Monochrome>,
}

impl Composition {
    /// Remember an overlay that became visible after restoration, such
    /// as a capture-complete message. Its rows must also be compared for
    /// presentation now, and restored on the next composition.
    pub fn include_overlay(&mut self, rows: Range<usize>) {
        self.overlay = union(self.overlay.clone(), rows);
    }

    /// Restore the base where it changed, where an old overlay must be
    /// erased and where a new one will be drawn. Draw the overlays next,
    /// then apply monochrome only to this returned band. Include every
    /// overlay, even one drawn after monochrome (such as a settings panel).
    pub fn restore(
        &mut self,
        output: &mut Frame,
        base: &Frame,
        changed: Range<usize>,
        overlay: Range<usize>,
        monochrome: Monochrome,
    ) -> Range<usize> {
        let size = (base.width, base.height);
        let rows = if self.size != Some(size) || self.monochrome != Some(monochrome) {
            0..base.height as usize
        } else {
            union(changed, union(self.overlay.clone(), overlay.clone()))
        };
        self.size = Some(size);
        self.monochrome = Some(monochrome);
        self.overlay = overlay;
        output.copy_rows_from(base, rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlay(frame: &mut Frame, rows: Range<usize>, value: u8) {
        for y in rows {
            let at = (y * frame.width as usize + 2) * 4;
            for channel in &mut frame.rgba[at..at + 3] {
                *channel = ((*channel as u16 + value as u16) / 2) as u8;
            }
        }
    }

    #[test]
    fn partial_composition_matches_full_after_moves_blinks_and_monitor_changes() {
        let mut composition = Composition::default();
        let mut base = Frame::from_rgb(13, 17, (0..13 * 17 * 3).map(|i| i as u8).collect());
        let mut output = Frame::new(13, 17);
        let mut partial = 0;
        for step in 0..200 {
            let h = base.height as usize;
            let y = step * 7 % h;
            let changed = if step % 3 == 0 { y..y + 1 } else { 0..0 };
            for y in changed.clone() {
                let at = (y * base.width as usize + 5) * 4;
                base.rgba[at..at + 3].fill(step as u8);
            }
            let cursor_y = step % h;
            let cursor = if step % 4 == 0 {
                0..0
            } else {
                cursor_y..cursor_y + 1
            };
            let mono = Monochrome::ALL[step / 11 % 4];
            let previous = output.rgba.clone();
            let rows = composition.restore(&mut output, &base, changed, cursor.clone(), mono);
            if rows.len() < h {
                partial += 1;
            }
            overlay(&mut output, cursor.clone(), step as u8);
            super::super::mono::apply_rows(&mut output, mono, rows.clone());
            let mut full = base.clone();
            overlay(&mut full, cursor, step as u8);
            super::super::mono::apply(&mut full, mono);
            assert!(output == full, "step {step}, {mono:?}, rows {rows:?}");
            let bytes = base.width as usize * 4;
            assert_eq!(
                &output.rgba[..rows.start * bytes],
                &previous[..rows.start * bytes]
            );
            assert_eq!(
                &output.rgba[rows.end * bytes..],
                &previous[rows.end * bytes..]
            );
        }
        assert!(partial > 100);
    }

    #[test]
    fn an_overlay_erases_and_equal_byte_count_geometry_changes_restore_every_row() {
        let mut composition = Composition::default();
        let mut base = Frame::from_rgb(3, 4, (0..36).collect());
        let mut output = base.clone();
        composition.restore(&mut output, &base, 0..0, 2..3, Monochrome::Off);
        overlay(&mut output, 2..3, 210);
        let rows = composition.restore(&mut output, &base, 0..0, 0..0, Monochrome::Off);
        assert_eq!(rows, 2..3);
        assert_eq!(output, base);
        assert!(
            composition
                .restore(&mut output, &base, 0..0, 0..0, Monochrome::Off)
                .is_empty()
        );
        base = Frame::from_rgb(4, 3, (0..36).rev().collect());
        assert_eq!(
            composition.restore(&mut output, &base, 0..0, 0..0, Monochrome::Off),
            0..3
        );
        assert_eq!(output, base);
    }

    #[test]
    fn bounded_comparison_preserves_row_offsets_and_matches_full_comparison() {
        let mut old = Frame::new(11, 17);
        for step in 0..100 {
            let a = step % 17;
            let b = (step * 7) % 17;
            let mut new = old.clone();
            new.rgba[(a * 11 + 2) * 4] ^= 31;
            new.rgba[(b * 11 + 7) * 4] ^= 63;
            let band = a.min(b)..a.max(b) + 1;
            assert_eq!(
                super::super::changed_rows_in(&old.rgba, &new.rgba, 44, band),
                super::super::changed_rows(&old.rgba, &new.rgba, 44)
            );
            old = new;
        }
    }
}
