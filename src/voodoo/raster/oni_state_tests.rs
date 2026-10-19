use super::tests::two_unit_state;
use super::*;

#[test]
fn menu_full_variants_preserve_dynamic_registers_and_combine_output() {
    for (full, combine) in [
        (VariantId::V8 as usize, VariantId::C1 as usize),
        (VariantId::V9 as usize, VariantId::C0 as usize),
    ] {
        for (fbz, reference) in [
            (0x843E1, 0),
            (0x843E1 ^ 0x600, 64),
            (0x843E1 ^ 0xE0, 128),
            (0x843E1 ^ 0x20000, 255),
        ] {
            let mut regs = VARIANTS[full].1.map(|(_, value)| value);
            regs[0] = fbz;
            regs[2] |= reference << 24;
            let selected = two_unit_state(regs);
            let previous = two_unit_state(regs);
            let generic = two_unit_state(regs);
            assert_eq!(variant(&selected, 2), Some(full));
            let p = TriParams {
                startr: 100 << 12,
                startg: 50 << 12,
                startb: 20 << 12,
                starta: 200 << 12,
                startw: 1 << 32,
                dadx: -(1 << 12),
                tmu: [TmuParams {
                    starts: 10 << 32,
                    startt: 20 << 32,
                    startw: 1 << 32,
                    dsdx: 3 << 28,
                    dtdx: 1 << 27,
                    ..Default::default()
                }; 2],
                ..Default::default()
            };
            let run = |st: &RasterState, index| {
                let (mut stipple, mut stats) = (0x12345678, Stats::default());
                for y in 0..32 {
                    scanline_as(index, 2, st, &p, y, 0, 320, &mut stipple, &mut stats);
                }
                (stipple, stats)
            };
            let expected = run(&generic, None);
            assert_eq!(run(&selected, Some(full)), expected);
            assert_eq!(run(&previous, Some(combine)), expected);
            for i in 0..selected.fb.len() {
                assert_eq!(
                    selected.fb.get(i),
                    generic.fb.get(i),
                    "full={full}, word={i}"
                );
                assert_eq!(
                    previous.fb.get(i),
                    generic.fb.get(i),
                    "combine={combine}, word={i}"
                );
            }
        }
    }
}
