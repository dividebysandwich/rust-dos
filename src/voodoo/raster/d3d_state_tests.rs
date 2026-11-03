use super::tests::two_unit_state;
use super::*;

/// A triangle's parameters with every iterator moving, so that depth,
/// fog, the texture's level of detail and the colours change along a
/// scanline and down the rows.
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
        dgdx: 1 << 9,
        dbdx: 1 << 8,
        dadx: -(1 << 10),
        dzdx: 1 << 14,
        dwdx: 1 << 20,
        drdy: 1 << 9,
        dzdy: -(1 << 13),
        dwdy: 1 << 20,
        tmu: [tp, tp],
        ..Default::default()
    }
}

/// Draw 48 rows of `regs` with variant `index` and with the generic code,
/// and require the same memory, counters and stipple.
fn same_as_generic(regs: [u32; 6], tmus: usize, index: usize) {
    let selected = two_unit_state(regs);
    let generic = two_unit_state(regs);
    let p = parameters();
    let run = |st: &RasterState, index| {
        let (mut stipple, mut stats) = (0x1234_5678, Stats::default());
        for y in 0..48 {
            scanline_as(index, tmus, st, &p, y, 0, 320, &mut stipple, &mut stats);
        }
        (stipple, stats)
    };
    let expected = run(&generic, None);
    assert!(expected.1.pixels_out > 0, "{regs:08X?} writes nothing");
    assert_eq!(run(&selected, Some(index)), expected, "{regs:08X?}");
    let differ = (0..selected.fb.len()).find(|&i| selected.fb.get(i) != generic.fb.get(i));
    assert_eq!(differ, None, "{regs:08X?} drawn with variant {index}");
}

/// The bits of fbzMode a variant leaves to the triangle: the depth
/// function (always, less, greater or equal, less or equal), the colour
/// and depth writes, clipping and the Y origin; and alphaMode's reference
/// value.
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
        r[0] |= fbz;
        r[2] |= reference << 24;
        r
    })
}

#[test]
fn direct3d_full_variants_draw_as_the_generic_code() {
    for (id, tmus) in [
        (VariantId::V10, 2),
        (VariantId::V11, 2),
        (VariantId::V12, 2),
        (VariantId::V13, 2),
        (VariantId::V14, 2),
        (VariantId::V15, 0),
    ] {
        let index = id as usize;
        let state = VARIANTS[index].1.map(|(_, value)| value);
        for regs in dynamic(state) {
            assert_eq!(variant(&two_unit_state(regs), tmus), Some(index));
            same_as_generic(regs, tmus, index);
            if tmus == 2 {
                // Its combine-only variant draws it the same way too.
                same_as_generic(regs, tmus, VariantId::C2 as usize);
            }
        }
    }
}

#[test]
fn the_direct3d_combine_variant_takes_the_drivers_other_states() {
    let (t0, t1) = (0x0000_0507, 0x0824_1507);
    // TMU 1's texture as 16-bit formats (RGB565 clamped in S and T,
    // ARGB4444 with LOD dither, AI88).
    let format = |f: u32, bits: u32| t1 & !0xF00 | f << 8 | bits;
    let (rgb565, argb4444, ai88) = (format(0xA, 0xC0), format(0xC, 0x10), format(0xD, 0));
    // Other blends, fog modes, filtering, clamping and texel formats with
    // the same colour path and combines, and the texture enable (27) and
    // subpixel adjustment (26) bits the raster doesn't read.
    let states = [
        [0x10930, 0x0000_2401, 0x0000_5110, 0x01, t0, t1],
        [0x10910, 0x0000_2401, 0x0000_1F0E, 0x41, t0, t1],
        [0x10918, 0x0C00_2401, 0x0000_511E, 0x41, t0, t1],
        [0x10910, 0x0800_2401, 0x0000_441E, 0x11, t0, t1],
        [0x10910, 0x0000_2401, 0x0000_040E, 0, t0 & !6, t1 & !6],
        [0x10910, 0x0000_2401, 0x0000_040E, 0, t0, rgb565],
        [0x10910, 0x0000_2401, 0x0000_511E, 0, t0, argb4444],
        [0x10910, 0x0000_2401, 0x0000_511E, 0, t0, ai88],
    ];
    for state in states {
        for regs in dynamic(state) {
            let index = variant(&two_unit_state(regs), 2);
            assert_eq!(index, Some(VariantId::C2 as usize), "{regs:08X?}");
            same_as_generic(regs, 2, VariantId::C2 as usize);
        }
    }
}
