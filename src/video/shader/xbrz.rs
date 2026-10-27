//! Native xBRZ input and the composed pixels drawn over its output.

use crate::video::{Frame, mono};

pub struct XbrzFrame {
    pub source: Frame,
    pub overlay: Frame,
    mono: Option<mono::Monochrome>,
    native: Vec<u8>,
    overlay_pixels: usize,
}

impl Default for XbrzFrame {
    fn default() -> Self {
        Self {
            source: Frame::new(0, 0),
            overlay: Frame::new(0, 0),
            mono: None,
            native: Vec::new(),
            overlay_pixels: 0,
        }
    }
}

impl XbrzFrame {
    /// Recover native pixels, tint them like the composed screen, and
    /// isolate overlays. Returns overlay visibility, any input change, and
    /// whether the overlay texture itself changed.
    pub fn prepare(
        &mut self,
        screen: &Frame,
        base: &Frame,
        mono: mono::Monochrome,
        screen_rows: std::ops::Range<usize>,
    ) -> (bool, bool, bool) {
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
        let source_screen_rows = native_rows.start * scale..(native_rows.end * scale).min(screen.height as usize);
        let mut rows = crate::video::compose::union(screen_rows, source_screen_rows);
        let mask = screen.overlay_mask();
        if mask.is_none() && self.overlay_pixels != 0 {
            rows = 0..screen.height as usize;
        }
        if !resized && let Some(mask) = mask {
            for y in 0..screen.height as usize {
                if (0..screen.width as usize).any(|x| {
                    let i = y * screen.width as usize + x;
                    (self.overlay.rgba[i * 4 + 3] != 0) != (mask[i] != 0)
                }) {
                    rows = crate::video::compose::union(rows, y..y + 1);
                }
            }
        }
        if resized {
            rows = 0..screen.height as usize;
        } else {
            rows.start = rows.start.min(screen.height as usize);
            rows.end = rows.end.min(screen.height as usize);
        }
        let changed = resized || !native_rows.is_empty();
        self.source.resize(width as u32, height as u32);
        self.native.resize(width * height * 4, 0);
        for y in native_rows.clone() {
            for x in 0..width {
                let i = y * width + x;
                let pixel = &mut self.source.rgba[i * 4..i * 4 + 4];
                // A doubled frame repeats each native pixel in a 2x2 block.
                // Its top-left pixel recovers the native colour.
                let from = ((y * scale) * base.width as usize + x * scale) * 4;
                let native = &base.rgba[from..from + 4];
                self.native[i * 4..i * 4 + 4].copy_from_slice(native);
                pixel.copy_from_slice(native);
            }
        }
        mono::apply_rows(&mut self.source, mono, native_rows);
        self.mono = Some(mono);
        if self.overlay.resize(screen.width, screen.height) {
            self.overlay.rgba.fill(0);
            self.overlay_pixels = 0;
        }
        let mut overlay_changed = overlay_resized;
        for y in rows.clone() {
            for x in 0..screen.width as usize {
                let i = y * screen.width as usize + x;
                let marked = mask.is_some_and(|mask| mask[i] != 0);
                let pixel = &screen.rgba[i * 4..i * 4 + 4];
                let next = if marked { pixel } else { &[0, 0, 0, 0] };
                let was_marked = self.overlay.rgba[i * 4 + 3] != 0;
                if marked != was_marked {
                    if marked {
                        self.overlay_pixels += 1;
                    } else {
                        self.overlay_pixels -= 1;
                    }
                }
                overlay_changed |= self.overlay.rgba[i * 4..i * 4 + 4] != *next;
                self.overlay.rgba[i * 4..i * 4 + 4].copy_from_slice(next);
            }
        }
        (self.overlay_pixels != 0, changed || overlay_changed, overlay_changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                assert!(!prepared.prepare(&composed, base, mono, 0..composed.height as usize).0);
                assert_eq!(prepared.source, expected);
                assert!(prepared.overlay.rgba.iter().all(|&byte| byte == 0));
                composed.rgba[..4].copy_from_slice(&[11, 222, 33, 255]);
                composed.mark_overlay(0, 0);
                assert_eq!(prepared.prepare(&composed, base, mono, 0..composed.height as usize), (true, true, true));
                assert_eq!(prepared.source, expected);
                assert_eq!(&prepared.overlay.rgba[..4], &[11, 222, 33, 255]);
                assert!(prepared.overlay.rgba[4..].iter().all(|&byte| byte == 0));
                assert_eq!(prepared.prepare(&composed, base, mono, 0..composed.height as usize), (true, false, false));
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
        assert_eq!(prepared.prepare(&screen, &base, mono::Monochrome::Off, 0..1), (true, true, true));
        let empty = Frame::new(0, 0);
        let mut empty = empty;
        empty.track_overlays(true);
        assert_eq!(prepared.prepare(&empty, &empty, mono::Monochrome::Amber, 0..0), (false, true, true));
        assert_eq!(prepared.source, empty);
        assert_eq!(prepared.overlay, empty);
        let mut base = base;
        base.track_overlays(true);
        assert_eq!(prepared.prepare(&base, &base, mono::Monochrome::Off, 0..1), (false, true, true));
        assert_eq!(prepared.source, base);
    }

    #[test]
    fn equal_colour_overlay_is_preserved_and_source_changes_are_reported() {
        let base = Frame::from_rgb(2, 1, vec![40, 80, 120, 1, 2, 3]);
        let mut screen = base.clone();
        screen.track_overlays(true);
        screen.mark_overlay(0, 0);
        let mut prepared = XbrzFrame::default();
        assert_eq!(prepared.prepare(&screen, &base, mono::Monochrome::Off, 0..1), (true, true, true));
        assert_eq!(&prepared.overlay.rgba[..4], &base.rgba[..4]);
        assert_eq!(prepared.prepare(&screen, &base, mono::Monochrome::Off, 0..0), (true, false, false));
        let mut changed = base.clone();
        changed.rgba[4] = 9;
        assert_eq!(prepared.prepare(&screen, &changed, mono::Monochrome::Off, 0..0), (true, true, false));
    }
}
