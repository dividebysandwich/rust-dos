use super::tests::two_unit_state;
use super::*;

/// A triangle's parameters with every iterator moving, so that the
/// texture's level of detail and the colours change along a scanline and
/// down the rows.
fn parameters() -> TriParams {
    let tp = TmuParams {
        starts: 10 << 32,
        startt: 20 << 32,
        startw: 1 << 32,
        dsdx: 3 << 28,
        dtdx: 1 << 27,
        dwdx: 1 << 20,
        dsdy: 1 << 27,
        dtdy: 3 << 28,
        dwdy: 1 << 20,
        lodbase: 0,
    };
    TriParams {
        startr: 100 << 12,
        startg: 50 << 12,
        startb: 20 << 12,
        starta: 200 << 12,
        startz: 0x4000 << 12,
        startw: 1 << 32,
        drdx: 1 << 10,
        dadx: -(1 << 10),
        dzdx: 1 << 14,
        dwdx: 1 << 20,
        dzdy: -(1 << 13),
        dwdy: 1 << 20,
        tmu: [tp, tp],
        ..Default::default()
    }
}

/// 48 rows of `st` drawn with variant `index` (None: the generic code):
/// the stipple and counters after them.
fn draw(st: &RasterState, index: Option<usize>) -> (u32, Stats) {
    let p = parameters();
    let (mut stipple, mut stats) = (0x1234_5678, Stats::default());
    for y in 0..48 {
        scanline_as(index, 2, st, &p, y, 0, 320, &mut stipple, &mut stats);
    }
    (stipple, stats)
}

fn same_memory(a: &RasterState, b: &RasterState) -> Option<usize> {
    (0..a.fb.len()).find(|&i| a.fb.get(i) != b.fb.get(i))
}

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
        let selected = two_unit_state(regs);
        assert_eq!(
            variant(&selected, 2),
            Some(VariantId::C3 as usize),
            "{regs:08X?}"
        );
        let generic = two_unit_state(regs);
        let expected = draw(&generic, None);
        assert!(expected.1.pixels_out > 0, "{regs:08X?} writes nothing");
        assert_eq!(
            draw(&selected, Some(VariantId::C3 as usize)),
            expected,
            "{regs:08X?}"
        );
        assert_eq!(same_memory(&selected, &generic), None, "{regs:08X?}");
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
        assert_eq!(draw(&a, None), draw(&b, None));
        assert_eq!(same_memory(&a, &b).is_some(), read, "textureMode {t0:08X}");
    }
}
