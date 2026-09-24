//! Monochrome monitors (`monochrome`): the picture as a tube with a single
//! phosphor shows it. A monochrome monitor on a colour card sees only how
//! bright each colour is, so every pixel becomes its luminance in the
//! phosphor's colour: white, the amber of the P3 phosphor, or the green of
//! the P1.

use super::Frame;

/// The monitor's phosphor, or `Off` for a colour monitor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Monochrome {
    #[default]
    Off,
    White,
    Amber,
    Green,
}

impl Monochrome {
    pub const ALL: [Monochrome; 4] = [Monochrome::Off, Monochrome::White, Monochrome::Amber, Monochrome::Green];

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mono| mono.name().eq_ignore_ascii_case(s.trim()))
    }

    pub fn name(self) -> &'static str {
        match self {
            Monochrome::Off => "off",
            Monochrome::White => "white",
            Monochrome::Amber => "amber",
            Monochrome::Green => "green",
        }
    }

    /// The monitor as the settings window shows it.
    pub fn describe(self) -> &'static str {
        match self {
            Monochrome::Off => "off (colour)",
            Monochrome::White => "white",
            Monochrome::Amber => "amber",
            Monochrome::Green => "green",
        }
    }

    /// The phosphor's colour at full brightness, if the monitor has one.
    pub fn phosphor(self) -> Option<[u8; 3]> {
        match self {
            Monochrome::Off => None,
            Monochrome::White => Some([0xFF, 0xFF, 0xFF]),
            Monochrome::Amber => Some([0xFF, 0xB0, 0x00]),
            Monochrome::Green => Some([0x33, 0xFF, 0x33]),
        }
    }
}

/// How bright a colour looks: its luma, weighted as television and the
/// VGA BIOS's gray-scale summing weigh red, green and blue (30%, 59% and
/// 11%).
fn luma(r: u8, g: u8, b: u8) -> u8 {
    ((77 * r as u32 + 150 * g as u32 + 29 * b as u32 + 128) >> 8) as u8
}

/// The phosphor's colour at each luma, black to full brightness.
fn ramp(phosphor: [u8; 3]) -> [[u8; 3]; 256] {
    let mut ramp = [[0; 3]; 256];
    for (y, entry) in ramp.iter_mut().enumerate() {
        *entry = phosphor.map(|c| ((y as u32 * c as u32 + 127) / 255) as u8);
    }
    ramp
}

/// Show `frame` as the monitor `mono` would. A colour monitor leaves it as
/// it is.
pub fn apply(frame: &mut Frame, mono: Monochrome) {
    let Some(phosphor) = mono.phosphor() else {
        return;
    };
    let ramp = ramp(phosphor);
    for pixel in frame.rgb.as_chunks_mut::<3>().0 {
        let [r, g, b] = *pixel;
        *pixel = ramp[luma(r, g, b) as usize];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tinted(mono: Monochrome, pixels: &[[u8; 3]]) -> Vec<[u8; 3]> {
        let mut frame = Frame::new(pixels.len() as u32, 1);
        frame.rgb = pixels.concat();
        apply(&mut frame, mono);
        frame.rgb.as_chunks::<3>().0.to_vec()
    }

    #[test]
    fn names_parse_back() {
        for mono in Monochrome::ALL {
            assert_eq!(Monochrome::parse(mono.name()), Some(mono));
        }
        assert_eq!(Monochrome::parse(" Amber "), Some(Monochrome::Amber));
        assert_eq!(Monochrome::parse("paper"), None);
    }

    #[test]
    fn a_colour_monitor_changes_nothing() {
        let pixels = [[0xAA, 0x55, 0x00], [0x12, 0x34, 0x56]];
        assert_eq!(tinted(Monochrome::Off, &pixels), pixels);
    }

    #[test]
    fn black_stays_black_and_white_is_the_phosphor() {
        for mono in [Monochrome::White, Monochrome::Amber, Monochrome::Green] {
            let out = tinted(mono, &[[0, 0, 0], [0xFF, 0xFF, 0xFF]]);
            assert_eq!(out, [[0, 0, 0], mono.phosphor().unwrap()]);
        }
    }

    #[test]
    fn colours_keep_their_brightness_in_the_phosphors_hue() {
        // Green is the brightest of the three, blue the darkest.
        let out = tinted(Monochrome::White, &[[0xFF, 0, 0], [0, 0xFF, 0], [0, 0, 0xFF]]);
        assert_eq!(out, [[77; 3], [149; 3], [29; 3]]);

        // Amber and green: dimmer, but the same hue as the phosphor.
        for mono in [Monochrome::Amber, Monochrome::Green] {
            let [pr, pg, pb] = mono.phosphor().unwrap().map(u32::from);
            let [r, g, b] = tinted(mono, &[[0x55, 0x55, 0x55]])[0].map(u32::from);
            assert!(g < pg);
            assert!((r * pg / g).abs_diff(pr) <= 2 && (b * pg / g).abs_diff(pb) <= 2, "{:?}", (r, g, b));
        }
    }
}
