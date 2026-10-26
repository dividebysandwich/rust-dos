//! Native xBRZ input and the composed pixels drawn over its output.

use crate::video::{Frame, mono};

pub struct XbrzFrame {
    pub source: Frame,
    pub overlay: Frame,
}

impl Default for XbrzFrame {
    fn default() -> Self {
        Self {
            source: Frame::new(0, 0),
            overlay: Frame::new(0, 0),
        }
    }
}

impl XbrzFrame {
    /// Recover native pixels, tint them like the composed screen, and
    /// isolate overlays. True if any overlay pixels are visible.
    pub fn prepare(&mut self, screen: &Frame, base: &Frame, mono: mono::Monochrome) -> bool {
        assert_eq!((screen.width, screen.height), (base.width, base.height));
        let scale = if base.doubled { 2 } else { 1 };
        let width = base.width as usize / scale;
        let height = base.height as usize / scale;
        self.source.resize(width as u32, height as u32);
        for (i, pixel) in self
            .source
            .rgba
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .enumerate()
        {
            // A doubled frame repeats each native pixel in a 2x2 block.
            // Its top-left pixel recovers the native colour.
            let from = ((i / width * scale) * base.width as usize + i % width * scale) * 4;
            pixel.copy_from_slice(&base.rgba[from..from + 4]);
        }
        mono::apply(&mut self.source, mono);
        self.overlay.resize(screen.width, screen.height);
        let native = self.source.rgba.as_chunks::<4>().0;
        let mut visible = false;
        for (i, (pixel, out)) in screen
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .zip(self.overlay.rgba.as_chunks_mut::<4>().0)
            .enumerate()
        {
            let x = i % screen.width as usize / scale;
            let y = i / screen.width as usize / scale;
            let changed = *pixel != native[y * width + x];
            *out = if changed { *pixel } else { [0; 4] };
            visible |= changed;
        }
        visible
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
        let mut prepared = XbrzFrame::default();
        for base in [&native, &doubled] {
            for mono in mono::Monochrome::ALL {
                let mut composed = base.clone();
                mono::apply(&mut composed, mono);
                let mut expected = native.clone();
                mono::apply(&mut expected, mono);
                assert!(!prepared.prepare(&composed, base, mono));
                assert_eq!(prepared.source, expected);
                assert!(prepared.overlay.rgba.iter().all(|&byte| byte == 0));
                composed.rgba[..4].copy_from_slice(&[11, 222, 33, 255]);
                assert!(prepared.prepare(&composed, base, mono));
                assert_eq!(prepared.source, expected);
                assert_eq!(&prepared.overlay.rgba[..4], &[11, 222, 33, 255]);
                assert!(prepared.overlay.rgba[4..].iter().all(|&byte| byte == 0));
            }
        }
    }

    #[test]
    fn empty_frames_clear_previously_prepared_pixels() {
        let base = Frame::from_rgb(1, 1, vec![200, 40, 40]);
        let mut screen = base.clone();
        screen.clear([11, 222, 33]);
        let mut prepared = XbrzFrame::default();
        assert!(prepared.prepare(&screen, &base, mono::Monochrome::Off));
        let empty = Frame::new(0, 0);
        assert!(!prepared.prepare(&empty, &empty, mono::Monochrome::Amber));
        assert_eq!(prepared.source, empty);
        assert_eq!(prepared.overlay, empty);
        assert!(!prepared.prepare(&base, &base, mono::Monochrome::Off));
        assert_eq!(prepared.source, base);
    }
}
