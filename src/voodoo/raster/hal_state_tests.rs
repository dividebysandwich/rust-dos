use super::tests::{same_as_generic, two_unit_state};
use super::*;

/// The registers of the built-in Direct3D HAL's triangles with the most
/// area in Omikron's first dialogue on a Voodoo3, as the card has them:
/// fbzMode, fbzColorPath, alphaMode, fogMode and the textureMode of TMU 0
/// and TMU 1, and the units drawn with. Every one of them was drawn by
/// the generic pipeline before V16-V19.
const DIALOGUE: [([u32; 6], usize, usize); 4] = [
    (
        [
            0x94391,
            0x0C00_2401,
            0x0004_4410,
            0,
            0x0C26_1A0F,
            0x0422_10C0,
        ],
        2,
        VariantId::V16 as usize,
    ),
    (
        [
            0x94391,
            0x0C00_2401,
            0x0004_6010,
            0,
            0x0C26_1A0F,
            0x0422_10C0,
        ],
        2,
        VariantId::V17 as usize,
    ),
    (
        [0x84381, 0x0400_0000, 0x0004_6010, 0, 0, 0],
        0,
        VariantId::V18 as usize,
    ),
    (
        [0x84381, 0x0400_0000, 0x0004_4410, 0, 0, 0],
        0,
        VariantId::V19 as usize,
    ),
];

/// The bits of fbzMode a variant leaves to the triangle (the depth
/// function, the writes, clipping and the Y origin) and alphaMode's
/// reference value, as in `d3d_state_tests`.
fn dynamic(regs: [u32; 6]) -> impl Iterator<Item = [u32; 6]> {
    [
        (7 << 5 | 3 << 9, 0),
        (1 << 5 | 3 << 9, 64),
        (6 << 5 | 1 << 9 | 1, 128),
        (3 << 5 | 3 << 9 | 1 << 17, 255),
    ]
    .into_iter()
    .map(move |(fbz, reference)| {
        let mut r = regs;
        r[0] = r[0] & !(7 << 5 | 3 << 9 | 1 | 1 << 17) | fbz;
        r[2] |= reference << 24;
        r
    })
}

#[test]
fn the_hals_dialogue_states_take_their_variants() {
    for (regs, tmus, id) in DIALOGUE {
        assert_eq!(
            variant(&two_unit_state(regs), tmus),
            Some(id),
            "{regs:08X?}"
        );
    }
}

#[test]
fn the_hals_variants_draw_as_the_generic_code() {
    for (state, tmus, id) in DIALOGUE {
        for regs in dynamic(state) {
            assert_eq!(variant(&two_unit_state(regs), tmus), Some(id));
            same_as_generic(regs, tmus, id);
            if tmus == 2 {
                same_as_generic(regs, tmus, VariantId::C4 as usize);
            }
        }
    }
}

#[test]
fn the_hals_combine_variant_takes_its_other_states() {
    let (t0, t1) = (0x0C26_1A0F, 0x0422_10C0);
    // TMU 0's texture as other 16-bit formats (ARGB1555, ARGB4444, AI88)
    // and point sampled, with other blends, fog and the alpha test.
    let format = |f: u32| t0 & !0xF00 | f << 8;
    let states = [
        [0x94391, 0x0C00_2401, 0x0004_0400, 0, t0, t1],
        [0x94391, 0x0C00_2401, 0x0004_4410, 0x01, format(0xB), t1],
        [0x94791, 0x0C00_2401, 0x0004_4412, 0, format(0xC), t1],
        [0x94391, 0x0C00_2401, 0x0004_5110, 0, format(0xD), t1],
        [0x94391, 0x0C00_2401, 0x0004_4410, 0, t0 & !6, t1],
    ];
    for state in states {
        for regs in dynamic(state) {
            let index = variant(&two_unit_state(regs), 2);
            if index.is_some_and(|i| i < VariantId::C0 as usize) {
                // A full variant fixes these bits too.
                continue;
            }
            assert_eq!(index, Some(VariantId::C4 as usize), "{regs:08X?}");
            same_as_generic(regs, 2, VariantId::C4 as usize);
        }
    }
}
