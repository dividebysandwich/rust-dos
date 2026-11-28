use super::tests::{dynamic, same_as_generic, two_unit_state};
use super::*;

/// A captured state: its registers, the units it draws with, and its
/// full and combine variants.
type State = ([u32; 6], usize, Option<usize>, Option<usize>);

/// The registers of Redguard's triangles with the most area in the fight
/// on the ship (Glide 2, `RGFX.EXE`, on a Voodoo Graphics with two
/// units), as the card has them: fbzMode, fbzColorPath, alphaMode,
/// fogMode and the textureMode of TMU 0 and TMU 1, with the full variant
/// that draws them, if any, and the combine variant for their colour
/// path. TMU 0 ignores TMU 1 in all of them, so they draw with one unit;
/// TMU 1's mode isn't in the capture and stands here as 0.
const SHIP: [State; 8] = [
    (
        [0x94739, 0x0D42_2429, 0x0004_0400, 0x01, 0x8C26_150F, 0],
        1,
        Some(VariantId::V23 as usize),
        Some(VariantId::C8 as usize),
    ),
    (
        [0x943F9, 0x0D42_2429, 0x0004_0400, 0, 0x8C26_150F, 0],
        1,
        Some(VariantId::V24 as usize),
        Some(VariantId::C8 as usize),
    ),
    (
        [0x943F9, 0x0542_610A, 0x0004_5110, 0, 0, 0],
        0,
        Some(VariantId::V25 as usize),
        None,
    ),
    (
        [0x943F9, 0x0D42_2429, 0x0004_0400, 0x01, 0x8C26_150F, 0],
        1,
        Some(VariantId::V23 as usize),
        Some(VariantId::C8 as usize),
    ),
    (
        [0x943FB, 0x0D42_2429, 0x0004_0400, 0x01, 0x8C26_1509, 0],
        1,
        None,
        Some(VariantId::C8 as usize),
    ),
    (
        [0x943FB, 0x0D42_2429, 0x0004_0400, 0, 0x8C26_1509, 0],
        1,
        None,
        Some(VariantId::C8 as usize),
    ),
    (
        [0x943FB, 0x0D42_2429, 0x0004_5110, 0, 0x8C26_1509, 0],
        1,
        None,
        Some(VariantId::C8 as usize),
    ),
    (
        [0x943FB, 0x0D42_2429, 0x0004_5110, 0x01, 0x8C26_1509, 0],
        1,
        None,
        Some(VariantId::C8 as usize),
    ),
];

#[test]
fn redguards_ship_states_take_their_variants() {
    for (regs, tmus, full, combine) in SHIP {
        let st = two_unit_state(regs);
        let card = if tmus == 0 { 0 } else { 2 };
        assert_eq!(units(&st, card), tmus, "{regs:08X?}");
        let expected = full.or(combine);
        assert_eq!(variant(&st, tmus), expected, "{regs:08X?}");
    }
}

#[test]
fn redguards_variants_draw_as_the_generic_code() {
    for (state, tmus, full, combine) in SHIP {
        for regs in dynamic(state) {
            let expected = full.or(combine);
            assert_eq!(
                variant(&two_unit_state(regs), tmus),
                expected,
                "{regs:08X?}"
            );
            for id in [full, combine].into_iter().flatten() {
                same_as_generic(regs, tmus, id);
            }
        }
    }
}
