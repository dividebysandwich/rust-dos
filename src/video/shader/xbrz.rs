//! Native xBRZ input and the composed pixels drawn over its output.

use crate::video::{Frame, mono};

pub struct XbrzFrame {
    pub source: Frame,
    pub overlay: Frame,
    mono: Option<mono::Monochrome>,
    native: Vec<u8>,
    overlay_pixels: usize,
    // Coverage is independent of alpha: marked pixels can be transparent.
    coverage: Vec<u8>,
    source_revision: u32,
    overlay_revision: u32,
    source_rows: std::ops::Range<usize>,
    overlay_rows: std::ops::Range<usize>,
    overlay_valid: bool,
}

impl Default for XbrzFrame {
    fn default() -> Self {
        Self {
            source: Frame::new(0, 0),
            overlay: Frame::new(0, 0),
            mono: None,
            native: Vec::new(),
            overlay_pixels: 0,
            coverage: Vec::new(),
            source_revision: 0,
            overlay_revision: 0,
            source_rows: 0..0,
            overlay_rows: 0..0,
            overlay_valid: false,
        }
    }
}

impl XbrzFrame {
    /// Changes only when native pixels, tint or source geometry change.
    pub fn source_revision(&self) -> u32 {
        self.source_revision
    }

    /// Changes only when the composed overlay texture changes.
    pub fn overlay_revision(&self) -> u32 {
        self.overlay_revision
    }

    /// Native rows changed by the last preparation, for partial uploads.
    pub fn source_rows(&self) -> std::ops::Range<usize> {
        self.source_rows.clone()
    }

    /// Rows changed at the current overlay revision, retained until its
    /// next change so a delayed upload can still use the correct band.
    pub fn overlay_rows(&self) -> std::ops::Range<usize> {
        self.overlay_rows.clone()
    }

    /// A bypassed composition may change pixels without changing coverage.
    pub fn invalidate_overlay(&mut self) {
        self.overlay_valid = false;
    }

    /// Recover native pixels, tint them like the composed screen, and
    /// isolate overlays. Without a base, treat the already tinted screen
    /// as flattened input and ignore its overlay coverage. Returns overlay
    /// visibility, any input change, and whether the overlay texture itself
    /// changed.
    pub fn prepare(
        &mut self,
        screen: &Frame,
        base: Option<&Frame>,
        mono: mono::Monochrome,
        screen_rows: std::ops::Range<usize>,
    ) -> (bool, bool, bool) {
        let separate_overlays = base.is_some();
        let (base, mono) = match base {
            Some(base) => (base, mono),
            None => (screen, mono::Monochrome::Off),
        };
        assert_eq!((screen.width, screen.height), (base.width, base.height));
        let scale = if base.doubled { 2 } else { 1 };
        let width = base.width as usize / scale;
        let height = base.height as usize / scale;
        let overlay_resized = (self.overlay.width, self.overlay.height) != (screen.width, screen.height);
        let resized = (self.source.width, self.source.height) != (width as u32, height as u32)
            || overlay_resized
            || self.mono != Some(mono);
        let mut native_rows = if resized { 0..height } else { 0..0 };
        if !resized {
            for y in 0..height {
                if scale == 1 {
                    let row = y * width * 4..(y + 1) * width * 4;
                    if self.native[row.clone()] != base.rgba[row] {
                        native_rows = crate::video::compose::union(native_rows, y..y + 1);
                    }
                    continue;
                }
                for x in 0..width {
                    let at = (y * width + x) * 4;
                    let from = ((y * scale) * base.width as usize + x * scale) * 4;
                    if self.native[at..at + 4] != base.rgba[from..from + 4] {
                        native_rows = crate::video::compose::union(native_rows, y..y + 1);
                        break;
                    }
                }
            }
        }
        let changed = resized || !native_rows.is_empty();
        self.source_rows = native_rows.clone();
        if changed {
            self.source_revision = self.source_revision.wrapping_add(1);
        }
        self.source.resize(width as u32, height as u32);
        self.native.resize(width * height * 4, 0);
        if scale == 1 {
            let band = native_rows.start * width * 4..native_rows.end * width * 4;
            self.native[band.clone()].copy_from_slice(&base.rgba[band.clone()]);
            self.source.rgba[band.clone()].copy_from_slice(&base.rgba[band]);
        } else {
            for y in native_rows.clone() {
                for x in 0..width {
                    let i = y * width + x;
                    let pixel = &mut self.source.rgba[i * 4..i * 4 + 4];
                    // A doubled frame repeats each native pixel in a 2x2
                    // block; its top-left pixel recovers the native colour.
                    let from = ((y * scale) * base.width as usize + x * scale) * 4;
                    let native = &base.rgba[from..from + 4];
                    self.native[i * 4..i * 4 + 4].copy_from_slice(native);
                    pixel.copy_from_slice(native);
                }
            }
        }
        mono::apply_rows(&mut self.source, mono, native_rows);
        self.mono = Some(mono);
        let previous_rows = std::mem::replace(&mut self.overlay_rows, 0..0);
        let (visible, overlay_changed) =
            self.prepare_overlay(screen, screen_rows, separate_overlays);
        if overlay_changed {
            self.overlay_revision = self.overlay_revision.wrapping_add(1);
        } else {
            self.overlay_rows = previous_rows;
        }
        (visible, changed || overlay_changed, overlay_changed)
    }

    fn prepare_overlay(
        &mut self,
        screen: &Frame,
        mut rows: std::ops::Range<usize>,
        separate: bool,
    ) -> (bool, bool) {
        if !self.overlay_valid {
            rows = 0..screen.height as usize;
            self.overlay_valid = true;
        }
        let mut changed = self.overlay.resize(screen.width, screen.height);
        if changed {
            self.overlay_rows = 0..screen.height as usize;
            self.overlay.rgba.fill(0);
            self.overlay_pixels = 0;
            self.coverage = Vec::new();
        }
        let mask = screen
            .overlay_mask()
            .filter(|mask| separate && mask.contains(&1));
        let Some(mask) = mask else {
            if !self.coverage.is_empty() {
                if self.overlay.rgba.iter().any(|&byte| byte != 0) {
                    changed = true;
                    self.overlay_rows = 0..screen.height as usize;
                }
                self.overlay.rgba.fill(0);
                self.overlay_pixels = 0;
                self.coverage.clear();
            }
            return (false, changed);
        };
        self.coverage.resize(mask.len(), 0);
        let width = screen.width as usize;
        if let Some(damage) = crate::video::changed_rows(&self.coverage, mask, width) {
            rows = crate::video::compose::union(rows, damage);
        }
        for y in rows.start.min(screen.height as usize)..rows.end.min(screen.height as usize) {
            let range = y * width..(y + 1) * width;
            let current = &mask[range.clone()];
            let previous = &mut self.coverage[range.clone()];
            let output = &mut self.overlay.rgba[range.start * 4..range.end * 4];
            let input = &screen.rgba[range.start * 4..range.end * 4];
            let mut row_changed = false;
            if current.iter().all(|&byte| byte == 1) {
                if output != input {
                    self.overlay_pixels -= output
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .filter(|pixel| pixel[3] != 0)
                        .count();
                    self.overlay_pixels += input
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .filter(|pixel| pixel[3] != 0)
                        .count();
                    output.copy_from_slice(input);
                    row_changed = true;
                }
            } else {
                let covered = || current.iter().zip(previous.iter());
                let Some(start) = covered().position(|(&a, &b)| a != 0 || b != 0) else {
                    continue;
                };
                let end = covered().rposition(|(&a, &b)| a != 0 || b != 0).unwrap() + 1;
                let output = &mut output.as_chunks_mut::<4>().0[start..end];
                let input = &input.as_chunks::<4>().0[start..end];
                for ((out, pixel), &marked) in
                    output.iter_mut().zip(input).zip(&current[start..end])
                {
                    let next = if marked != 0 { *pixel } else { [0; 4] };
                    if *out != next {
                        self.overlay_pixels -= usize::from(out[3] != 0);
                        self.overlay_pixels += usize::from(next[3] != 0);
                        *out = next;
                        row_changed = true;
                    }
                }
            }
            previous.copy_from_slice(current);
            if row_changed {
                changed = true;
                self.overlay_rows =
                    crate::video::compose::union(self.overlay_rows.clone(), y..y + 1);
            }
        }
        (self.overlay_pixels != 0, changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_damage_tracks_pixels_movement_and_delayed_uploads() {
        let base = Frame::from_rgb(8, 6, vec![80; 8 * 6 * 3]);
        let mut screen = base.clone();
        screen.track_overlays(true);
        screen.mark_overlay_rect(1, 1, 2, 1);
        let mut prepared = XbrzFrame::default();
        prepared.prepare(&screen, Some(&base), mono::Monochrome::Off, 0..6);
        let source = prepared.source_revision();
        screen.rgba[(8 + 1) * 4] ^= 127;
        prepared.prepare(&screen, Some(&base), mono::Monochrome::Off, 1..2);
        let revision = prepared.overlay_revision();
        assert_eq!(prepared.overlay_rows(), 1..2);
        prepared.prepare(&screen, Some(&base), mono::Monochrome::Off, 0..6);
        assert_eq!(prepared.overlay_rows(), 1..2);
        assert_eq!(prepared.overlay_revision(), revision);
        let mut moved = base.clone();
        moved.track_overlays(true);
        moved.mark_overlay_rect(3, 4, 2, 1);
        prepared.prepare(&moved, Some(&base), mono::Monochrome::Off, 0..0);
        assert_eq!(prepared.overlay_rows(), 1..5);
        assert!(
            prepared.overlay.rgba[8 * 4..2 * 8 * 4]
                .iter()
                .all(|&byte| byte == 0)
        );
        prepared.invalidate_overlay();
        moved.rgba[(4 * 8 + 3) * 4] ^= 127;
        prepared.overlay_revision = u32::MAX;
        prepared.prepare(&moved, Some(&base), mono::Monochrome::Off, 0..0);
        assert_eq!(prepared.overlay_rows(), 4..5);
        assert_eq!(prepared.overlay_revision(), 0);
        assert_eq!(prepared.source_revision(), source);
        assert_eq!(
            prepared.overlay.rgba[(4 * 8 + 3) * 4],
            moved.rgba[(4 * 8 + 3) * 4]
        );
        prepared.prepare(&base, Some(&base), mono::Monochrome::Off, 0..0);
        assert_eq!(prepared.overlay_rows(), 0..6);
        assert!(prepared.overlay.rgba.iter().all(|&byte| byte == 0));
    }

    #[test]
    fn flattened_input_ignores_tint_and_coverage_after_clean_presentation() {
        for doubled in [false, true] {
            let mut base = Frame::from_rgb(4, 2, (0..24).map(|i| i * 9).collect());
            base.doubled = doubled;
            let mut composed = base.clone();
            mono::apply(&mut composed, mono::Monochrome::Amber);
            composed.track_overlays(true);
            composed.mark_overlay(0, 0);
            composed.rgba[..4].copy_from_slice(&[11, 222, 33, 255]);
            let mut prepared = XbrzFrame::default();
            assert!(
                prepared
                    .prepare(&composed, Some(&base), mono::Monochrome::Amber, 0..2)
                    .0
            );
            let clean_source = prepared.source.clone();
            let scale = if doubled { 2 } else { 1 };
            let mut expected = Frame::new(base.width / scale, base.height / scale);
            for y in 0..expected.height as usize {
                for x in 0..expected.width as usize {
                    let from = (y * scale as usize * base.width as usize + x * scale as usize) * 4;
                    let to = (y * expected.width as usize + x) * 4;
                    expected.rgba[to..to + 4].copy_from_slice(&composed.rgba[from..from + 4]);
                }
            }
            assert_eq!(
                prepared.prepare(&composed, None, mono::Monochrome::Green, 0..0),
                (false, true, true)
            );
            assert_eq!(prepared.source, expected);
            assert!(prepared.overlay.rgba.iter().all(|&byte| byte == 0));
            assert!(
                prepared
                    .prepare(&composed, Some(&base), mono::Monochrome::Amber, 0..0)
                    .0
            );
            assert_eq!(prepared.source, clean_source);
            assert_eq!(&prepared.overlay.rgba[..4], &[11, 222, 33, 255]);
        }
    }

    #[test]
    fn transparent_coverage_does_not_leak_visibility() {
        let base = Frame::from_rgb(3, 2, vec![80; 18]);
        let mut screen = base.clone();
        screen.track_overlays(true);
        screen.mark_overlay(1, 0);
        screen.rgba[7] = 0;
        let mut prepared = XbrzFrame::default();
        for _ in 0..3 {
            assert!(
                !prepared
                    .prepare(&screen, Some(&base), mono::Monochrome::Off, 0..2)
                    .0
            );
            assert_eq!(&prepared.overlay.rgba[4..8], &screen.rgba[4..8]);
        }
        assert_eq!(
            prepared.prepare(&base, Some(&base), mono::Monochrome::Off, 0..0),
            (false, true, true)
        );
        assert!(prepared.overlay.rgba.iter().all(|&byte| byte == 0));
    }

    #[test]
    fn overlay_matches_scalar_reference_through_coverage_and_pixel_changes() {
        let mut prepared = XbrzFrame::default();
        for (width, height, doubled) in [(9, 6, false), (18, 12, true), (0, 0, false)] {
            let mut base = Frame::from_rgb(width, height, vec![80; (width * height * 3) as usize]);
            base.doubled = doubled;
            for step in 0..24 {
                let mut screen = base.clone();
                screen.track_overlays(true);
                for y in 0..height as usize {
                    for x in 0..width as usize {
                        if step % 6 == 0 || (x + y + step) % 7 < 2 {
                            screen.mark_overlay(x, y);
                            let pixel = &mut screen.rgba[(y * width as usize + x) * 4..][..4];
                            pixel.copy_from_slice(&[
                                step as u8,
                                x as u8,
                                y as u8,
                                [0, 127, 255][(x + step) % 3],
                            ]);
                        }
                    }
                }
                let expected: Vec<u8> = screen
                    .rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(screen.overlay_mask().unwrap())
                    .flat_map(|(pixel, &marked)| if marked != 0 { *pixel } else { [0; 4] })
                    .collect();
                let previous = prepared.overlay.rgba.clone();
                let geometry = (prepared.overlay.width, prepared.overlay.height) != (width, height);
                let revision = prepared.overlay_revision();
                let changed = geometry || previous != expected;
                let result = prepared.prepare(
                    &screen,
                    Some(&base),
                    mono::Monochrome::Off,
                    0..height as usize,
                );
                assert_eq!(prepared.overlay.rgba, expected);
                assert_eq!(
                    result.0,
                    expected.as_chunks::<4>().0.iter().any(|p| p[3] != 0)
                );
                assert_eq!(result.2, changed);
                assert_eq!(
                    prepared.overlay_revision(),
                    revision.wrapping_add(u32::from(changed))
                );
                let capacity = prepared.coverage.capacity();
                assert_eq!(
                    prepared.prepare(&screen, Some(&base), mono::Monochrome::Off, 0..0),
                    (result.0, false, false)
                );
                assert_eq!(prepared.coverage.capacity(), capacity);
                base.rgba
                    .iter_mut()
                    .step_by(4)
                    .for_each(|byte| *byte ^= 127);
                assert!(
                    !prepared
                        .prepare(&screen, Some(&base), mono::Monochrome::Amber, 0..0)
                        .2
                );
            }
            prepared.prepare(&base, Some(&base), mono::Monochrome::Off, 0..0);
            assert!(prepared.coverage.is_empty());
            assert!(prepared.overlay.rgba.iter().all(|&byte| byte == 0));
        }
        assert_eq!(prepared.coverage.capacity(), 0);
    }

    #[test]
    fn overlay_pixel_damage_is_limited_to_reported_rows() {
        let base = Frame::from_rgb(8, 4, vec![80; 96]);
        let mut screen = base.clone();
        screen.track_overlays(true);
        let mut prepared = XbrzFrame::default();
        prepared.prepare(&screen, Some(&base), mono::Monochrome::Off, 0..4);
        assert_eq!(prepared.coverage.capacity(), 0);
        screen.mark_overlay_rect(0, 0, 8, 4);
        prepared.prepare(&screen, Some(&base), mono::Monochrome::Off, 0..0);
        screen.rgba[32..64].fill(127);
        assert_eq!(
            prepared.prepare(&screen, Some(&base), mono::Monochrome::Off, 1..2),
            (true, true, true)
        );
        assert_eq!(prepared.overlay.rgba, screen.rgba);
        screen.track_overlays(false);
        assert_eq!(
            prepared.prepare(&screen, Some(&base), mono::Monochrome::Off, 0..0),
            (false, true, true)
        );
        let capacity = prepared.coverage.capacity();
        screen.track_overlays(true);
        screen.mark_overlay(7, 3);
        prepared.prepare(&screen, Some(&base), mono::Monochrome::Off, 0..0);
        assert_eq!(prepared.coverage.capacity(), capacity);
        assert_eq!(&prepared.overlay.rgba[124..], &screen.rgba[124..]);
        assert!(prepared.overlay.rgba[..124].iter().all(|&byte| byte == 0));
    }

    #[test]
    fn partial_native_rows_preserve_other_rows_and_tint() {
        let mut base = Frame::from_rgb(3, 3, (0..27).map(|i| i * 7).collect());
        let mut prepared = XbrzFrame::default();
        for mono in mono::Monochrome::ALL {
            prepared.prepare(&base, Some(&base), mono, 0..3);
            base.rgba[16] ^= 127;
            prepared.prepare(&base, Some(&base), mono, 1..2);
            let mut expected = base.clone();
            mono::apply(&mut expected, mono);
            assert_eq!(prepared.source, expected);
            assert_eq!(prepared.source_rows(), 1..2);
            prepared.prepare(&base, Some(&base), mono, 0..0);
            assert_eq!(prepared.source_rows(), 0..0);
            assert_eq!(prepared.source, expected);
        }
    }

    #[test]
    fn revisions_separate_base_and_overlay_changes() {
        let mut base = Frame::from_rgb(2, 1, vec![40, 80, 120, 1, 2, 3]);
        let mut screen = base.clone();
        screen.track_overlays(true);
        let mut prepared = XbrzFrame::default();
        prepared.prepare(&screen, Some(&base), mono::Monochrome::Off, 0..1);
        let revision = prepared.source_revision();
        assert_eq!(prepared.source_rows(), 0..1);
        let overlay = prepared.overlay_revision();
        screen.mark_overlay(0, 0);
        prepared.prepare(&screen, Some(&base), mono::Monochrome::Off, 0..0);
        assert_eq!(prepared.source_revision(), revision);
        assert_eq!(prepared.source_rows(), 0..0);
        assert_ne!(prepared.overlay_revision(), overlay);
        let overlay = prepared.overlay_revision();
        base.rgba[0] ^= 127;
        prepared.prepare(&screen, Some(&base), mono::Monochrome::Off, 0..0);
        assert_ne!(prepared.source_revision(), revision);
        assert_eq!(prepared.overlay_revision(), overlay);
        let revision = prepared.source_revision();
        prepared.prepare(&screen, Some(&base), mono::Monochrome::Amber, 0..0);
        assert_ne!(prepared.source_revision(), revision);
        let revision = prepared.source_revision();
        prepared.prepare(&screen, Some(&base), mono::Monochrome::Amber, 0..0);
        assert_eq!(prepared.source_revision(), revision);
    }

    #[test]
    fn native_monochrome_pixels_and_overlays_stay_separate() {
        let native = Frame::from_rgb(2, 1, vec![200, 40, 40, 30, 60, 200]);
        let mut doubled = Frame::from_rgb(
            4,
            2,
            [200, 40, 40, 200, 40, 40, 30, 60, 200, 30, 60, 200].repeat(2),
        );
        doubled.doubled = true;
        doubled.track_overlays(true);
        let mut prepared = XbrzFrame::default();
        for base in [&native, &doubled] {
            for mono in mono::Monochrome::ALL {
                let mut composed = base.clone();
                mono::apply(&mut composed, mono);
                let mut expected = native.clone();
                mono::apply(&mut expected, mono);
                composed.track_overlays(true);
                assert!(
                    !prepared
                        .prepare(&composed, Some(base), mono, 0..composed.height as usize)
                        .0
                );
                assert_eq!(prepared.source, expected);
                assert!(prepared.overlay.rgba.iter().all(|&byte| byte == 0));
                composed.rgba[..4].copy_from_slice(&[11, 222, 33, 255]);
                composed.mark_overlay(0, 0);
                assert_eq!(
                    prepared.prepare(&composed, Some(base), mono, 0..composed.height as usize),
                    (true, true, true)
                );
                assert_eq!(prepared.source, expected);
                assert_eq!(&prepared.overlay.rgba[..4], &[11, 222, 33, 255]);
                assert!(prepared.overlay.rgba[4..].iter().all(|&byte| byte == 0));
                assert_eq!(
                    prepared.prepare(&composed, Some(base), mono, 0..composed.height as usize),
                    (true, false, false)
                );
            }
        }
    }

    #[test]
    fn empty_frames_clear_previously_prepared_pixels() {
        let base = Frame::from_rgb(1, 1, vec![200, 40, 40]);
        let mut screen = base.clone();
        screen.clear([11, 222, 33]);
        let mut prepared = XbrzFrame::default();
        screen.track_overlays(true);
        screen.mark_overlay(0, 0);
        assert_eq!(
            prepared.prepare(&screen, Some(&base), mono::Monochrome::Off, 0..1),
            (true, true, true)
        );
        let empty = Frame::new(0, 0);
        let mut empty = empty;
        empty.track_overlays(true);
        assert_eq!(
            prepared.prepare(&empty, Some(&empty), mono::Monochrome::Amber, 0..0),
            (false, true, true)
        );
        assert_eq!(prepared.source, empty);
        assert_eq!(prepared.overlay, empty);
        let mut base = base;
        base.track_overlays(true);
        assert_eq!(
            prepared.prepare(&base, Some(&base), mono::Monochrome::Off, 0..1),
            (false, true, true)
        );
        assert_eq!(prepared.source, base);
    }

    #[test]
    fn equal_colour_overlay_is_preserved_and_source_changes_are_reported() {
        let base = Frame::from_rgb(2, 1, vec![40, 80, 120, 1, 2, 3]);
        let mut screen = base.clone();
        screen.track_overlays(true);
        screen.mark_overlay(0, 0);
        let mut prepared = XbrzFrame::default();
        assert_eq!(
            prepared.prepare(&screen, Some(&base), mono::Monochrome::Off, 0..1),
            (true, true, true)
        );
        assert_eq!(&prepared.overlay.rgba[..4], &base.rgba[..4]);
        assert_eq!(
            prepared.prepare(&screen, Some(&base), mono::Monochrome::Off, 0..0),
            (true, false, false)
        );
        let mut changed = base.clone();
        changed.rgba[4] = 9;
        assert_eq!(
            prepared.prepare(&screen, Some(&changed), mono::Monochrome::Off, 0..0),
            (true, true, false)
        );
    }
}
