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
        let st = two_unit_state(regs);
        assert_eq!(units(&st, 2), 1, "{regs:08X?}");
        let index = variant(&st, 1);
        assert_eq!(index, Some(VariantId::C3 as usize), "{regs:08X?}");
        same_as_generic(regs, 1, VariantId::C3 as usize);
    }
}

#[test]
fn triangles_whose_tmu_0_ignores_tmu_1_draw_the_same_with_one_unit() {
    // Myth's states, Quake's with TMU 1 iterating the same W (which two
    // units divide by once), and the HAL's.
    let states = STATES.into_iter().chain([
        [0x90110, 0x0D42_0009, 0x0004_0400, 0, 0x8C26_1A0F, 0x8422_1C0F],
        [0x90110, 0x0D42_0009, 0x0000_6010, 0, 0x8C26_130F, 0x8422_130F],
        [0x94391, 0x0C00_2401, 0x0004_4410, 0, 0x0C26_1A0F, 0x0422_10C0],
    ]);
    for mut regs in states {
        // Colour and depth written, the depth test always passing.
        regs[0] |= 3 << 9 | 7 << 5;
        let two = two_unit_state(regs);
        let mut one = two_unit_state(regs);
        assert_eq!(units(&two, 2), 1, "{regs:08X?}");
        one.tmu[1] = None;
        let drawn = draw_rows(&two, 2, None);
        assert!(drawn.1.pixels_out > 0, "{regs:08X?} writes nothing");
        assert_eq!(draw_rows(&one, 1, None), drawn, "{regs:08X?}");
        assert_eq!(first_difference(&one, &two), None, "{regs:08X?}");
    }
}

#[test]
fn triangles_keep_tmu_1_where_tmu_0_reads_it_or_draws_nothing() {
    let regs = STATES[0];
    assert_eq!(units(&two_unit_state(regs), 1), 1);
    assert_eq!(units(&two_unit_state(regs), 0), 0);
    // TMU 0 adds TMU 1's texel.
    let mut reads = regs;
    reads[4] = 0x0C24_050F;
    assert_eq!(units(&two_unit_state(reads), 2), 2);
    // TMU 0 draws nothing and passes TMU 1's texel on.
    let mut off = two_unit_state(regs);
    off.tmu[0].as_mut().unwrap().lodmin = 8 << 8;
    assert_eq!(units(&off, 2), 2);
    // TMU 0 sends its configuration in place of any texel.
    let mut config = two_unit_state(reads);
    config.send_config = Some(0x1234);
    assert_eq!(units(&config, 2), 1);
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
