use super::tests::{draw_rows, first_difference, same_as_generic, two_unit_state};
use super::*;

/// The states with the most area in a saved Myth scene (Crow's Bridge),
/// and the same colour path and combines with the units' highlights and
/// another texture format.
const STATES: [[u32; 6]; 6] = [
    [
        0x0008_4321,
        0x0C00_0035,
        0x0004_0400,
        0,
        0x0C26_150F,
        0x0422_10C0,
    ],
    [
        0x0008_4321,
        0x0C00_0035,
        0x0004_2010,
        0,
        0x0C26_130F,
        0x0422_10C0,
    ],
    [
        0x0008_4321,
        0x0C00_0035,
        0x0004_5110,
        0,
        0x0C26_150F,
        0x0422_10C0,
    ],
    [
        0x0008_4321,
        0x0C00_0035,
        0x0004_0400,
        0,
        0x0C26_1A0F,
        0x0422_10C0,
    ],
    [
        0x0008_4321,
        0x0C00_0035,
        0x0004_5110,
        0,
        0x0C26_1E0F,
        0x0422_10C0,
    ],
    [
        0x0008_4321,
        0x0C00_0035,
        0x0004_5110,
        0,
        0x0C26_1C0F,
        0x0422_10C0,
    ],
];

#[test]
fn myths_combine_variant_draws_its_states_as_the_generic_code() {
    for regs in STATES {
        let index = variant(&two_unit_state(regs), 2);
        assert_eq!(index, Some(VariantId::C3 as usize), "{regs:08X?}");
        same_as_generic(regs, 2, VariantId::C3 as usize);
    }
}

#[test]
fn texture_combines_without_other_ignore_the_unit_before() {
    let t = two_unit_state(STATES[0]).tmu[0].clone().unwrap();
    let (lod, c_local) = (0x123, 0x9A3C_E571);
    for tc in 0..1u32 << 18 {
        let mode = tc << 12;
        if uses_other(mode) {
            continue;
        }
        let a = texture_combine(&t, mode, lod, 0, c_local);
        for cother in [0xFFFF_FFFF, 0x8040_C020] {
            let b = texture_combine(&t, mode, lod, cother, c_local);
            assert_eq!(a, b, "textureMode {mode:08X} reads c_other");
        }
    }
}

#[test]
fn tmu_1s_texture_shows_only_where_tmu_0_reads_it() {
    // TMU 1's memory changed, with TMU 1 passing its texel on (Myth's
    // TMU 1 passes nothing): the picture of a state whose TMU 0 doesn't
    // read it stays, and one whose TMU 0 adds it changes.
    let other = |st: &mut RasterState| {
        let t = st.tmu[1].as_mut().unwrap();
        let ram = Vram::new(t.ram.bytes());
        for i in 0..ram.len() {
            ram.set(i, (i as u16).wrapping_mul(7919) ^ 0x5A5A);
        }
        t.ram = ram;
    };
    for (t0, read) in [(STATES[0][4], false), (0x0C24_050F, true)] {
        let regs = [
            STATES[0][0],
            STATES[0][1],
            STATES[0][2],
            0,
            t0,
            STATES[0][4],
        ];
        assert_eq!(uses_other(t0), read);
        let a = two_unit_state(regs);
        let mut b = two_unit_state(regs);
        other(&mut b);
        assert_eq!(draw_rows(&a, 2, None), draw_rows(&b, 2, None));
        assert_eq!(
            first_difference(&a, &b).is_some(),
            read,
            "textureMode {t0:08X}"
        );
    }
}
