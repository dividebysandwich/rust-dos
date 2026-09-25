//! The colours a CGA shows on a composite (NTSC) monitor or TV. The card's
//! composite output is one signal for brightness and colour: patterns of
//! pixels finer than the 3.58 MHz colour carrier come out as colours. At
//! 640x200, where four pixels span one cycle of the carrier, programs drew
//! with those "artifact" colours to get 16 of them where an RGB monitor
//! shows two: King's Quest, the Ultima games, Sierra's AGI games and many
//! more have a composite option. At 320x200 the four colours of a palette
//! blend into others likewise.
//!
//! The decoder is reenigne's model of the CGA's composite circuitry, which
//! works for every mode and colour setting and both revisions of the card,
//! as DOSBox Staging has it (vga_other.cpp `update_cga16_color` and
//! vga_draw.cpp `Composite_Process`, GPL-2.0-or-later): the signal the card
//! makes for each pair of neighbouring pixels at each phase of the carrier,
//! then an NTSC decoder that turns the signal back into RGB.

/// Whether the CGA's picture is decoded as a composite monitor would
/// (`composite`): `Auto` does it when a program turns on 640x200 graphics
/// with the colour burst, which only programs for composite monitors do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CompositeMode {
    #[default]
    Auto,
    On,
    Off,
}

impl CompositeMode {
    pub const ALL: [CompositeMode; 3] = [CompositeMode::Auto, CompositeMode::On, CompositeMode::Off];

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(CompositeMode::Auto),
            "on" | "true" | "yes" | "1" => Some(CompositeMode::On),
            "off" | "false" | "no" | "0" => Some(CompositeMode::Off),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            CompositeMode::Auto => "auto",
            CompositeMode::On => "on",
            CompositeMode::Off => "off",
        }
    }

    /// As the settings window shows it.
    pub fn describe(self) -> &'static str {
        match self {
            CompositeMode::Auto => "auto (640x200 colour burst)",
            CompositeMode::On => "on",
            CompositeMode::Off => "off (RGB monitor)",
        }
    }
}

/// Which CGA makes the composite signal (`composite_era`): IBM's first
/// cards (1981-1983), which the classic composite games were drawn for, or
/// the later revision, whose signal mixes the colours differently.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CompositeEra {
    #[default]
    Old,
    New,
}

impl CompositeEra {
    pub const ALL: [CompositeEra; 2] = [CompositeEra::Old, CompositeEra::New];

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "old" | "early" => Some(CompositeEra::Old),
            "new" | "late" => Some(CompositeEra::New),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            CompositeEra::Old => "old",
            CompositeEra::New => "new",
        }
    }

    /// As the settings window shows it.
    pub fn describe(self) -> &'static str {
        match self {
            CompositeEra::Old => "old CGA (1981-83)",
            CompositeEra::New => "new CGA (1984 on)",
        }
    }
}

/// The `composite` and `composite_era` settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompositeSettings {
    pub mode: CompositeMode,
    pub era: CompositeEra,
}

/// The CGA's chroma multiplexer: the colour signal for a pair of
/// neighbouring pixels' colours (3 bits each) at each quarter of the
/// carrier's cycle, as reenigne measured it.
#[rustfmt::skip]
const CHROMA_MULTIPLEXER: [u8; 256] = [
    2,   2,   2,   2,   114, 174, 4,   3,   2,   1,   133, 135, 2,
    113, 150, 4,   133, 2,   1,   99,  151, 152, 2,   1,   3,   2,
    96,  136, 151, 152, 151, 152, 2,   56,  62,  4,   111, 250, 118,
    4,   0,   51,  207, 137, 1,   171, 209, 5,   140, 50,  54,  100,
    133, 202, 57,  4,   2,   50,  153, 149, 128, 198, 198, 135, 32,
    1,   36,  81,  147, 158, 1,   42,  33,  1,   210, 254, 34,  109,
    169, 77,  177, 2,   0,   165, 189, 154, 3,   44,  33,  0,   91,
    197, 178, 142, 144, 192, 4,   2,   61,  67,  117, 151, 112, 83,
    4,   0,   249, 255, 3,   107, 249, 117, 147, 1,   50,  162, 143,
    141, 52,  54,  3,   0,   145, 206, 124, 123, 192, 193, 72,  78,
    2,   0,   159, 208, 4,   0,   53,  58,  164, 159, 37,  159, 171,
    1,   248, 117, 4,   98,  212, 218, 5,   2,   54,  59,  93,  121,
    176, 181, 134, 130, 1,   61,  31,  0,   160, 255, 34,  1,   1,
    58,  197, 166, 0,   177, 194, 2,   162, 111, 34,  96,  205, 253,
    32,  1,   1,   57,  123, 125, 119, 188, 150, 112, 78,  4,   0,
    75,  166, 180, 20,  38,  78,  1,   143, 246, 42,  113, 156, 37,
    252, 4,   1,   188, 175, 129, 1,   37,  118, 4,   88,  249, 202,
    150, 145, 200, 61,  59,  60,  60,  228, 252, 117, 77,  60,  58,
    248, 251, 81,  212, 254, 107, 198, 59,  58,  169, 250, 251, 81,
    80,  100, 58,  154, 250, 251, 252, 252, 252,
];

/// The signal's level for the intensity bits of a pair of pixels, as
/// reenigne measured them.
#[allow(clippy::excessive_precision)]
const INTENSITY: [f32; 4] = [77.175381, 88.654656, 166.564623, 174.228438];

/// The later CGA's mix of the chroma, intensity and colour signals.
fn new_cga_v(c: f32, i: f32, r: f32, g: f32, b: f32) -> f32 {
    0.29 * c / 0.72 + 0.32 * i / 0.28 + 0.10 * r / 0.28 + 0.22 * g / 0.28 + 0.07 * b / 0.28
}

/// The composite signal of a CGA and the decoder for it, for one revision
/// of the card and one setting of the colour burst.
pub struct Decoder {
    era: CompositeEra,
    /// Mode Control bit 2: no colour burst, so the monitor shows grey.
    bw: bool,
    /// The signal for the left and right pixel's RGBI colours (4 bits each)
    /// at each phase of the carrier (2 bits).
    table: Box<[i32; 1024]>,
    /// The decoder's colour matrix.
    ri: i32,
    rq: i32,
    gi: i32,
    gq: i32,
    bi: i32,
    bq: i32,
}

/// DOSBox's knobs at their defaults.
const BRIGHTNESS: f32 = 0.0;
const CONTRAST: f32 = 100.0;
const SATURATION: f32 = 100.0;
const HUE: f32 = 0.0;
/// The convergence knob (sharpness), at 0.
const SHARPNESS: i32 = 0;

impl Decoder {
    pub fn new(era: CompositeEra, bw: bool) -> Self {
        let new = era == CompositeEra::New;
        let (i0, i3) = (INTENSITY[0], INTENSITY[3]);
        let (c0, c255) = (CHROMA_MULTIPLEXER[0] as f32, CHROMA_MULTIPLEXER[255] as f32);
        let min_v = if new { new_cga_v(c0, i0, i0, i0, i0) } else { c0 + i0 };
        let max_v = if new { new_cga_v(c255, i3, i3, i3, i3) } else { c255 + i3 };
        let mode_contrast = 2.56 * CONTRAST / (max_v - min_v);
        let mode_brightness = BRIGHTNESS * 5.0 - 256.0 * min_v / (max_v - min_v);
        // Graphics: the text modes' 14 is for 80 columns.
        let mode_hue = 4.0;
        let mode_saturation = SATURATION * if new { 5.8 } else { 2.9 } / 100.0;

        let mut table = Box::new([0i32; 1024]);
        for (x, entry) in table.iter_mut().enumerate() {
            let right = (x >> 2) & 15;
            let left = (x >> 6) & 15;
            // Without the burst, every colour but black is white.
            let grey = |c: usize| if bw { (c & 8) | if c & 7 != 0 { 7 } else { 0 } } else { c };
            let (rc, lc) = (grey(right), grey(left));
            let phase = x & 3;
            let c = CHROMA_MULTIPLEXER[((lc & 7) << 5) | ((rc & 7) << 2) | phase] as f32;
            let i = INTENSITY[(left >> 3) | ((right >> 2) & 2)];
            let v = if new {
                let r = INTENSITY[((left >> 2) & 1) | ((right >> 1) & 2)];
                let g = INTENSITY[((left >> 1) & 1) | (right & 2)];
                let b = INTENSITY[(left & 1) | ((right << 1) & 2)];
                new_cga_v(c, i, r, g, b)
            } else {
                c + i
            };
            *entry = (v * mode_contrast + mode_brightness) as i32;
        }

        let i = (table[6 * 68] - table[6 * 68 + 2]) as f32;
        let q = (table[6 * 68 + 1] - table[6 * 68 + 3]) as f32;
        let a = std::f32::consts::TAU * (33.0 + 90.0 + HUE + mode_hue) / 360.0;
        let (s, c) = a.sin_cos();
        let r = if bw { 0.0 } else { 256.0 * mode_saturation / (i * i + q * q).sqrt() };
        let iq_adjust_i = -(i * c + q * s) * r;
        let iq_adjust_q = (q * c - i * s) * r;
        let (ri, rq, gi, gq, bi, bq) = (0.9563f32, 0.6210f32, -0.2721f32, -0.6474f32, -1.1069f32, 1.7046f32);
        Decoder {
            era,
            bw,
            table,
            ri: (ri * iq_adjust_i + rq * iq_adjust_q) as i32,
            rq: (-ri * iq_adjust_q + rq * iq_adjust_i) as i32,
            gi: (gi * iq_adjust_i + gq * iq_adjust_q) as i32,
            gq: (-gi * iq_adjust_q + gq * iq_adjust_i) as i32,
            bi: (bi * iq_adjust_i + bq * iq_adjust_q) as i32,
            bq: (-bi * iq_adjust_q + bq * iq_adjust_i) as i32,
        }
    }

    /// Whether this decoder is the one for `era` and the burst setting.
    pub fn is_for(&self, era: CompositeEra, bw: bool) -> bool {
        self.era == era && self.bw == bw
    }

    /// Decode a scanline: `samples` are the RGBI colours (0-15) of the
    /// card's pixel clock (14.318 MHz, 640 of them across the picture, a
    /// multiple of 4), `border` the colour of the overscan around them.
    /// Writes one RGB pixel a sample to `out`.
    pub fn decode_line(&self, samples: &[u8], border: u8, out: &mut [u8]) {
        let w = samples.len();
        debug_assert!(w.is_multiple_of(4) && out.len() >= w * 3);
        let t = &self.table;
        let border = border as usize & 15;
        let b = border * 68;

        // The signal, with the border on both sides.
        let mut temp = vec![0i32; w + 10];
        let mut o = 0;
        let mut push = |v: i32| {
            temp[o] = v;
            o += 1;
        };
        for x in 0..4 {
            push(t[b + ((x + 3) & 3)]);
        }
        push(t[(border << 6) | ((samples[0] as usize) << 2) | 3]);
        for x in 0..w - 1 {
            push(t[((samples[x] as usize) << 6) | ((samples[x + 1] as usize) << 2) | (x & 3)]);
        }
        push(t[((samples[w - 1] as usize) << 6) | (border << 2) | 3]);
        for x in 0..5 {
            push(t[b + (x & 3)]);
        }

        let clamp = |v: i32| (v >> 13).clamp(0, 255) as u8;
        if self.bw {
            for x in 0..w {
                let i = x + 5;
                let c = (temp[i] + temp[i]) << 3;
                let d = (temp[i - 1] + temp[i + 1]) << 3;
                let y = clamp(((c + d) << 8) + SHARPNESS * (c - d));
                out[x * 3..x * 3 + 3].copy_from_slice(&[y, y, y]);
            }
            return;
        }

        // The chroma, in phase and in quadrature: `ap[x]` and `bp[x]` for
        // x from -1 on, kept one along.
        let mut ap = vec![0i32; w + 2];
        let mut bp = vec![0i32; w + 2];
        for x in 0..w + 2 {
            let i = x + 4;
            ap[x] = temp[i - 4] - ((temp[i - 2] - temp[i] + temp[i + 2]) << 1) + temp[i + 4];
            bp[x] = (temp[i - 3] - temp[i - 1] + temp[i + 1] - temp[i + 3]) << 1;
        }

        // The luma, less the chroma, and the colour for each sample: sample
        // x is at temp[x + 5], and its chroma at ap[x + 1] and bp[x + 1].
        temp[4] = (temp[4] << 3) - ap[0];
        temp[5] = (temp[5] << 3) - ap[1];
        for x in 0..w {
            let (i, a) = (x + 5, x + 1);
            let (ai, bi) = (ap[a], bp[a]);
            // The carrier's phase turns the chroma a quarter each sample.
            let (ii, q) = match x & 3 {
                0 => (ai, bi),
                1 => (-bi, ai),
                2 => (-ai, -bi),
                _ => (bi, -ai),
            };
            temp[i + 1] = (temp[i + 1] << 3) - ap[a + 1];
            let c = temp[i] + temp[i];
            let d = temp[i - 1] + temp[i + 1];
            let y = ((c + d) << 8) + SHARPNESS * (c - d);
            out[x * 3] = clamp(y + self.ri * ii + self.rq * q);
            out[x * 3 + 1] = clamp(y + self.gi * ii + self.gq * q);
            out[x * 3 + 2] = clamp(y + self.bi * ii + self.bq * q);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The colour of a solid run of `pattern` (four samples repeated).
    fn solid(decoder: &Decoder, pattern: [u8; 4]) -> (u8, u8, u8) {
        let samples: Vec<u8> = (0..640).map(|x| pattern[x & 3]).collect();
        let mut out = vec![0u8; 640 * 3];
        decoder.decode_line(&samples, 0, &mut out);
        let x = 320;
        (out[x * 3], out[x * 3 + 1], out[x * 3 + 2])
    }

    #[test]
    fn black_white_and_artifact_colours() {
        let d = Decoder::new(CompositeEra::Old, false);
        let (r, g, b) = solid(&d, [0; 4]);
        assert!(r < 16 && g < 16 && b < 16, "black: {:?}", (r, g, b));
        let (r, g, b) = solid(&d, [15; 4]);
        assert!(r > 230 && g > 230 && b > 230, "white: {:?}", (r, g, b));
        // Two pixels on, two off: a cycle of the colour carrier, at
        // opposite phases, gives two different colours.
        let first = solid(&d, [15, 15, 0, 0]);
        let second = solid(&d, [0, 0, 15, 15]);
        let saturated = |(r, g, b): (u8, u8, u8)| r.max(g).max(b) as i32 - r.min(g).min(b) as i32 > 60;
        assert!(saturated(first) && saturated(second), "{:?} {:?}", first, second);
        assert_ne!(first, second);
        // Every other pixel is twice the carrier's frequency: grey, as the
        // composite palette's two greys (0101 and 1010) are.
        let (r, g, b) = solid(&d, [15, 0, 15, 0]);
        assert!(r.abs_diff(g) < 8 && g.abs_diff(b) < 8, "{:?}", (r, g, b));
    }

    #[test]
    fn without_the_burst_the_picture_is_grey() {
        let d = Decoder::new(CompositeEra::Old, true);
        for pattern in [[15, 0, 15, 0], [15, 15, 0, 0], [15, 0, 0, 0]] {
            let (r, g, b) = solid(&d, pattern);
            assert!(r == g && g == b, "{:?}: {:?}", pattern, (r, g, b));
        }
    }

    #[test]
    fn the_revisions_differ() {
        let old = solid(&Decoder::new(CompositeEra::Old, false), [15, 15, 0, 0]);
        let new = solid(&Decoder::new(CompositeEra::New, false), [15, 15, 0, 0]);
        assert_ne!(old, new);
    }
}
