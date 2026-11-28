use super::tests::{draw_rows, dynamic, same_as_generic, two_unit_state};
use super::*;

/// The registers of Rage Rally's triangles with the most area in a race
/// at Goldstone (the built-in Direct3D HAL on a Voodoo3), as the card has
/// them: fbzMode, fbzColorPath, alphaMode, fogMode and the textureMode of
/// TMU 0 and TMU 1, with the full variant that draws them, if any, and the
/// combine variant for their colour path. TMU 0 ignores TMU 1 in all of
/// them, so they draw with one unit; TMU 1's mode isn't in the capture
/// and stands here as the HAL's.
const RACE: [([u32; 6], usize, Option<usize>, Option<usize>); 12] = [
    (
        [
            0x94771,
            0x0C00_2405,
            0x0004_0400,
            0x19,
            0x0C26_1B0F,
            0x0422_10C0,
        ],
        1,
        Some(VariantId::V20 as usize),
        Some(VariantId::C5 as usize),
    ),
    (
        [
            0x94771,
            0x0C00_2405,
            0x0004_0409,
            0x19,
            0x0C26_1B0F,
            0x0422_10C0,
        ],
        1,
        Some(VariantId::V21 as usize),
        Some(VariantId::C5 as usize),
    ),
    (
        [0x94371, 0x0400_0000, 0x0104_5119, 0, 0, 0],
        0,
        Some(VariantId::V22 as usize),
        None,
    ),
    (
        [
            0x94771,
            0x0C58_2405,
            0x0004_0400,
            0,
            0x0C26_1B0F,
            0x0422_10C0,
        ],
        1,
        None,
        Some(VariantId::C6 as usize),
    ),
    (
        [
            0x94371,
            0x0C58_2405,
            0x0104_5119,
            0x19,
            0x0C26_1CCF,
            0x0422_10C0,
        ],
        1,
        None,
        Some(VariantId::C6 as usize),
    ),
    (
        [
            0x94371,
            0x0C00_2405,
            0x0104_5119,
            0,
            0x0C26_1CC9,
            0x0422_10C0,
        ],
        1,
        None,
        Some(VariantId::C5 as usize),
    ),
    (
        [
            0x94371,
            0x0C00_2405,
            0x0104_5119,
            0,
            0x0C26_1C0F,
            0x0422_10C0,
        ],
        1,
        None,
        Some(VariantId::C5 as usize),
    ),
    (
        [
            0x94771,
            0x0C58_2405,
            0x0004_0400,
            0x19,
            0x0C26_1BCF,
            0x0422_10C0,
        ],
        1,
        None,
        Some(VariantId::C6 as usize),
    ),
    (
        [
            0x94771,
            0x0C58_2405,
            0x0104_0400,
            0,
            0x0C26_1B0F,
            0x0422_10C0,
        ],
        1,
        None,
        Some(VariantId::C6 as usize),
    ),
    (
        [
            0x94351,
            0x0C58_0004,
            0x0004_4410,
            0x19,
            0x0C26_1BCF,
            0x0422_10C0,
        ],
        1,
        None,
        Some(VariantId::C7 as usize),
    ),
    (
        [
            0x94371,
            0x0C58_2405,
            0x0104_4119,
            0,
            0x0C26_1A0F,
            0x0422_10C0,
        ],
        1,
        None,
        Some(VariantId::C6 as usize),
    ),
    (
        [
            0x94351,
            0x0C58_0004,
            0x0104_4410,
            0,
            0x0C26_1BCF,
            0x0422_10C0,
        ],
        1,
        None,
        Some(VariantId::C7 as usize),
    ),
];

/// `dynamic`'s replacements, with the alpha test's reference kept below
/// 0xC0: the race's alpha test passes alpha greater than the reference,
/// which a reference of 0xFF would let no pixel do.
fn dynamic_tested(regs: [u32; 6]) -> impl Iterator<Item = [u32; 6]> {
    dynamic(regs).map(|mut r| {
        if r[2] & 1 != 0 {
            r[2] = r[2] & AM_FIXED | (r[2] >> 24).min(0xBF) << 24;
        }
        r
    })
}

#[test]
fn rage_rallys_race_states_take_their_variants() {
    for (regs, tmus, full, combine) in RACE {
        let st = two_unit_state(regs);
        let card = if tmus == 0 { 0 } else { 2 };
        assert_eq!(units(&st, card), tmus, "{regs:08X?}");
        let expected = full.or(combine);
        assert_eq!(variant(&st, tmus), expected, "{regs:08X?}");
    }
}

#[test]
fn rage_rallys_variants_draw_as_the_generic_code() {
    let mut alpha_failed = 0;
    for (state, tmus, full, combine) in RACE {
        for regs in dynamic_tested(state) {
            let st = two_unit_state(regs);
            assert_eq!(variant(&st, tmus), full.or(combine), "{regs:08X?}");
            for id in [full, combine].into_iter().flatten() {
                same_as_generic(regs, tmus, id);
            }
            alpha_failed += draw_rows(&st, tmus, None).1.afunc_fail;
        }
    }
    // The alpha test leaves some pixels out, so the comparison covers it.
    assert!(alpha_failed > 0);
}
