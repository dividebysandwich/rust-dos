//! The arithmetic of a stage and of what follows the stages, on
//! channels of 0 to 255 ("The stage cascade" and "After the cascade" in
//! `docs/toadforce.md`). Each function here is one formula of the
//! specification; the span code applies them to the pixels of a row.

use super::texel::{A, B, G, R, Rgba};

/// A stage operation (`D3DTOP`), without `DISABLE` and the two bump
/// operations, which the lowering turns into other steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Op {
    SelectArg1,
    SelectArg2,
    Modulate,
    Modulate2x,
    Modulate4x,
    Add,
    AddSigned,
    AddSigned2x,
    Subtract,
    AddSmooth,
    /// The four `BLEND...ALPHA` operations: the lowering names where
    /// the alpha comes from.
    BlendAlpha,
    /// `BLENDTEXTUREALPHAPM`.
    BlendAlphaPm,
    ModulateAlphaAddColor,
    ModulateColorAddAlpha,
    ModulateInvAlphaAddColor,
    ModulateInvColorAddAlpha,
    DotProduct3,
    MultiplyAdd,
    Lerp,
}

impl Op {
    /// From a `COLOROP` or `ALPHAOP` value. None for `DISABLE`, the
    /// bump operations and values that are no operation.
    pub fn from_state(value: u32) -> Option<Self> {
        Some(match value {
            2 => Op::SelectArg1,
            3 => Op::SelectArg2,
            4 => Op::Modulate,
            5 => Op::Modulate2x,
            6 => Op::Modulate4x,
            7 => Op::Add,
            8 => Op::AddSigned,
            9 => Op::AddSigned2x,
            10 => Op::Subtract,
            11 => Op::AddSmooth,
            12..=14 | 16 => Op::BlendAlpha,
            15 => Op::BlendAlphaPm,
            18 => Op::ModulateAlphaAddColor,
            19 => Op::ModulateColorAddAlpha,
            20 => Op::ModulateInvAlphaAddColor,
            21 => Op::ModulateInvColorAddAlpha,
            24 => Op::DotProduct3,
            25 => Op::MultiplyAdd,
            26 => Op::Lerp,
            _ => return None,
        })
    }

    /// Whether the operation is one for the alpha channel. The others
    /// mix alpha into colour, and as an alpha operation give their
    /// first argument.
    pub fn for_alpha(self) -> bool {
        !matches!(
            self,
            Op::ModulateAlphaAddColor
                | Op::ModulateColorAddAlpha
                | Op::ModulateInvAlphaAddColor
                | Op::ModulateInvColorAddAlpha
                | Op::DotProduct3
        )
    }

    /// Which of arguments 1, 2 and 0 the operation reads.
    pub fn reads(self) -> [bool; 3] {
        match self {
            Op::SelectArg1 => [true, false, false],
            Op::SelectArg2 => [false, true, false],
            Op::MultiplyAdd | Op::Lerp => [true, true, true],
            _ => [true, true, false],
        }
    }
}

/// `round(a * b / 255)`.
#[inline]
pub fn mul(a: i32, b: i32) -> i32 {
    (a * b + 127) / 255
}

/// One channel of `op` on arguments `a` (1), `b` (2) and `c` (0), with
/// `t` the alpha the operation blends by or adds. The result isn't
/// clamped. `DotProduct3` works on three channels at once, in `dot3`.
#[inline]
pub fn channel(op: Op, a: i32, b: i32, c: i32, t: i32) -> i32 {
    match op {
        Op::SelectArg1 => a,
        Op::SelectArg2 => b,
        Op::Modulate => mul(a, b),
        Op::Modulate2x => mul(a, b) * 2,
        Op::Modulate4x => mul(a, b) * 4,
        Op::Add => a + b,
        Op::AddSigned => a + b - 128,
        Op::AddSigned2x => (a + b - 128) * 2,
        Op::Subtract => a - b,
        Op::AddSmooth => a + b - mul(a, b),
        Op::BlendAlpha => (a * t + b * (255 - t) + 127) / 255,
        Op::BlendAlphaPm => a + mul(b, 255 - t),
        Op::ModulateAlphaAddColor => a + mul(t, b),
        Op::ModulateColorAddAlpha => mul(a, b) + t,
        Op::ModulateInvAlphaAddColor => mul(255 - t, b) + a,
        Op::ModulateInvColorAddAlpha => mul(255 - a, b) + t,
        Op::MultiplyAdd => c + mul(a, b),
        Op::Lerp => (c * a + (255 - c) * b + 127) / 255,
        Op::DotProduct3 => a,
    }
}

/// `DOTPRODUCT3` of two colours: the value every channel gets.
#[inline]
pub fn dot3(a: Rgba, b: Rgba) -> i32 {
    let term = |ch: usize| (a[ch] as i32 - 128) * (b[ch] as i32 - 128);
    let s = term(R) + term(G) + term(B);
    if s <= 0 {
        0
    } else {
        ((4 * s + 127) / 255).min(255)
    }
}

#[inline]
pub fn clamp(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// A compare function (`D3DCMP`): whether `value` passes against
/// `reference`.
#[inline]
pub fn compare(func: u32, value: u32, reference: u32) -> bool {
    match func {
        1 => false,
        2 => value < reference,
        3 => value == reference,
        4 => value <= reference,
        5 => value > reference,
        6 => value != reference,
        7 => value >= reference,
        _ => true,
    }
}

/// A stencil operation (`D3DSTENCILOP`) on `value` with the reference
/// `reference`.
#[inline]
pub fn stencil_op(op: u32, value: u32, reference: u32) -> u32 {
    match op {
        2 => 0,
        3 => reference & 0xFF,
        4 => (value + 1).min(255),
        5 => value.saturating_sub(1),
        6 => !value & 0xFF,
        7 => (value + 1) & 0xFF,
        8 => value.wrapping_sub(1) & 0xFF,
        _ => value,
    }
}

/// A blend factor (`D3DBLEND`) for each channel, of 255, with the
/// fragment `s` and the pixel there `d`. `BOTHSRCALPHA` and
/// `BOTHINVSRCALPHA` are named by the lowering as the pair of factors
/// they stand for, and aren't seen here.
#[inline]
pub fn factor(blend: u32, s: Rgba, d: Rgba) -> [i32; 4] {
    let all = |v: i32| [v; 4];
    let of = |c: Rgba| [c[0] as i32, c[1] as i32, c[2] as i32, c[3] as i32];
    let inv = |c: [i32; 4]| [255 - c[0], 255 - c[1], 255 - c[2], 255 - c[3]];
    match blend {
        1 => all(0),
        3 => of(s),
        4 => inv(of(s)),
        5 => all(s[A] as i32),
        6 => all(255 - s[A] as i32),
        7 => all(d[A] as i32),
        8 => all(255 - d[A] as i32),
        9 => of(d),
        10 => inv(of(d)),
        11 => {
            let f = (s[A] as i32).min(255 - d[A] as i32);
            [f, f, f, 255]
        }
        _ => all(255),
    }
}

/// The blend (`D3DBLENDOP`) of the fragment `s` and the pixel `d` with
/// the factors `fs` and `fd`.
#[inline]
pub fn blend(op: u32, s: Rgba, d: Rgba, fs: [i32; 4], fd: [i32; 4]) -> Rgba {
    let mut out = [0; 4];
    for ch in 0..4 {
        let (sc, dc) = (s[ch] as i32, d[ch] as i32);
        let (sf, df) = (sc * fs[ch], dc * fd[ch]);
        out[ch] = match op {
            2 => clamp((sf - df + 127).div_euclid(255)),
            3 => clamp((df - sf + 127).div_euclid(255)),
            4 => sc.min(dc) as u8,
            5 => sc.max(dc) as u8,
            _ => clamp((sf + df + 127) / 255),
        };
    }
    out
}

/// The fog blend: `f` of 255 of the colour and the rest of the fog's.
#[inline]
pub fn fog(c: u8, fog: u8, f: i32) -> u8 {
    ((c as i32 * f + fog as i32 * (255 - f) + 127) / 255) as u8
}

/// The 4x4 ordered dither matrix, a row after another.
pub const DITHER: [i32; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];

/// A colour dithered for a 5-6-5 target at pixel (`x`, `y`): reduced
/// to the target's bits with the matrix added, and widened again.
#[inline]
pub fn dither(c: Rgba, x: i32, y: i32) -> Rgba {
    let d = DITHER[((y & 3) * 4 + (x & 3)) as usize];
    let five = |v: u8| super::texel::widen(((v as u32 + d as u32 / 2) >> 3).min(31), 5);
    let six = |v: u8| super::texel::widen(((v as u32 + d as u32 / 4) >> 2).min(63), 6);
    [five(c[B]), six(c[G]), five(c[R]), c[A]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_operation_follows_its_formula() {
        let ch = |op, a, b| channel(op, a, b, 0, 0);
        assert_eq!(ch(Op::SelectArg1, 10, 20), 10);
        assert_eq!(ch(Op::SelectArg2, 10, 20), 20);
        assert_eq!(ch(Op::Modulate, 255, 255), 255);
        assert_eq!(ch(Op::Modulate, 255, 77), 77);
        assert_eq!(ch(Op::Modulate, 128, 128), 64);
        assert_eq!(ch(Op::Modulate, 1, 127), 0);
        assert_eq!(ch(Op::Modulate, 1, 128), 1);
        assert_eq!(ch(Op::Modulate2x, 128, 128), 128);
        assert_eq!(ch(Op::Modulate4x, 128, 128), 256);
        assert_eq!(ch(Op::Add, 200, 100), 300);
        assert_eq!(ch(Op::AddSigned, 200, 100), 172);
        assert_eq!(ch(Op::AddSigned, 10, 20), -98);
        assert_eq!(ch(Op::AddSigned2x, 100, 100), 144);
        assert_eq!(ch(Op::Subtract, 10, 20), -10);
        assert_eq!(ch(Op::AddSmooth, 255, 255), 255);
        assert_eq!(ch(Op::AddSmooth, 128, 128), 192);
        assert_eq!(channel(Op::BlendAlpha, 200, 100, 0, 255), 200);
        assert_eq!(channel(Op::BlendAlpha, 200, 100, 0, 0), 100);
        assert_eq!(channel(Op::BlendAlpha, 200, 100, 0, 128), 150);
        assert_eq!(channel(Op::BlendAlphaPm, 200, 100, 0, 128), 250);
        assert_eq!(channel(Op::ModulateAlphaAddColor, 10, 100, 0, 128), 60);
        assert_eq!(channel(Op::ModulateColorAddAlpha, 128, 128, 0, 30), 94);
        assert_eq!(channel(Op::ModulateInvAlphaAddColor, 10, 100, 0, 128), 60);
        assert_eq!(channel(Op::ModulateInvColorAddAlpha, 55, 100, 0, 30), 108);
        assert_eq!(channel(Op::MultiplyAdd, 128, 128, 100, 0), 164);
        assert_eq!(channel(Op::Lerp, 200, 100, 255, 0), 200);
        assert_eq!(channel(Op::Lerp, 200, 100, 0, 0), 100);
        assert_eq!(channel(Op::Lerp, 200, 100, 64, 0), 125);
        assert_eq!(clamp(300), 255);
        assert_eq!(clamp(-98), 0);
    }

    #[test]
    fn dot_product_3_is_four_times_the_signed_dot() {
        // Blue, green, red, alpha. Two normals along +z: 127 * 127.
        let up = [255, 128, 128, 0];
        assert_eq!(dot3(up, up), 253);
        // Opposite and perpendicular normals give 0.
        assert_eq!(dot3(up, [0, 128, 128, 0]), 0);
        assert_eq!(dot3(up, [128, 128, 255, 0]), 0);
        // Half the length on one side: (4 * 127 * 64 + 127) / 255.
        assert_eq!(dot3(up, [192, 128, 128, 0]), 127);
        assert_eq!(dot3([255; 4], [255; 4]), 255);
    }

    #[test]
    fn every_operation_number_is_an_operation_or_a_special_case() {
        for value in 0..=27 {
            let special = matches!(value, 0 | 1 | 17 | 22 | 23 | 27);
            assert_eq!(Op::from_state(value).is_none(), special, "{value}");
        }
    }

    #[test]
    fn compares_and_stencil_operations_follow_direct3d() {
        let passes: Vec<u32> = (1..=8).filter(|&f| compare(f, 5, 7)).collect();
        assert_eq!(passes, [2, 4, 6, 8]);
        let passes: Vec<u32> = (1..=8).filter(|&f| compare(f, 7, 7)).collect();
        assert_eq!(passes, [3, 4, 7, 8]);
        let passes: Vec<u32> = (1..=8).filter(|&f| compare(f, 9, 7)).collect();
        assert_eq!(passes, [5, 6, 7, 8]);
        let ops = |v| {
            (1..=8)
                .map(|op| stencil_op(op, v, 0x1AB))
                .collect::<Vec<_>>()
        };
        assert_eq!(ops(0), [0, 0, 0xAB, 1, 0, 255, 1, 255]);
        assert_eq!(ops(255), [255, 0, 0xAB, 255, 254, 0, 0, 254]);
    }

    #[test]
    fn blends_weigh_the_fragment_and_the_pixel() {
        let s = [200, 100, 50, 128];
        let d = [10, 20, 30, 255];
        assert_eq!(factor(1, s, d), [0; 4]);
        assert_eq!(factor(2, s, d), [255; 4]);
        assert_eq!(factor(3, s, d), [200, 100, 50, 128]);
        assert_eq!(factor(4, s, d), [55, 155, 205, 127]);
        assert_eq!(factor(5, s, d), [128; 4]);
        assert_eq!(factor(6, s, d), [127; 4]);
        assert_eq!(factor(7, s, d), [255; 4]);
        assert_eq!(factor(8, s, d), [0; 4]);
        assert_eq!(factor(9, s, d), [10, 20, 30, 255]);
        assert_eq!(factor(10, s, d), [245, 235, 225, 0]);
        assert_eq!(factor(11, s, [0, 0, 0, 200]), [55, 55, 55, 255]);
        // Alpha blending: 128 of the fragment, 127 of the pixel.
        let (fs, fd) = (factor(5, s, d), factor(6, s, d));
        assert_eq!(blend(1, s, d, fs, fd), [105, 60, 40, 191]);
        // Additive, saturating.
        let one = [255; 4];
        assert_eq!(blend(1, s, [100; 4], one, one), [255, 200, 150, 228]);
        assert_eq!(blend(2, s, [100; 4], one, one), [100, 0, 0, 28]);
        assert_eq!(blend(3, s, [100; 4], one, one), [0, 0, 50, 0]);
        assert_eq!(blend(4, s, [100; 4], one, one), [100, 100, 50, 100]);
        assert_eq!(blend(5, s, [100; 4], one, one), [200, 100, 100, 128]);
    }

    #[test]
    fn fog_and_dither_follow_their_formulas() {
        assert_eq!(fog(200, 40, 255), 200);
        assert_eq!(fog(200, 40, 0), 40);
        assert_eq!(fog(200, 40, 128), 120);
        // Pixel (0, 0) adds nothing: the top bits, widened.
        assert_eq!(dither([0x87, 0x87, 0x87, 9], 0, 0), [0x84, 0x86, 0x84, 9]);
        // Pixel (0, 3) adds 15: 7 to a 5-bit channel, 3 to a 6-bit one.
        assert_eq!(dither([0x81, 0x81, 0x81, 9], 0, 3), [0x8C, 0x86, 0x8C, 9]);
        // White stays white.
        assert_eq!(dither([255; 4], 0, 3), [255; 4]);
    }
}
