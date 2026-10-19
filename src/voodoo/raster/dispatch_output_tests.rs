use super::tests::two_unit_state;
use super::*;
use crate::voodoo::workers::{Job, Pool};

fn parameters() -> TriParams {
    TriParams {
        startr: 100 << 12,
        startg: 50 << 12,
        startb: 20 << 12,
        starta: 200 << 12,
        startz: 0x4000 << 12,
        startw: 1 << 32,
        tmu: [TmuParams {
            starts: 10 << 32,
            startt: 20 << 32,
            startw: 1 << 32,
            dsdx: 3 << 28,
            dtdx: 1 << 27,
            ..Default::default()
        }; 2],
        ..Default::default()
    }
}

#[test]
fn indexed_scanlines_preserve_generic_output_counters_and_stipple() {
    for &(count, fixed) in VARIANTS {
        let mut regs = fixed.map(|(_, value)| value);
        // Writes and the always-pass depth comparison remain dynamic.
        regs[0] |= 3 << 9 | 7 << 5;
        let selected = two_unit_state(regs);
        let generic = two_unit_state(regs);
        let index = variant(&selected, count).unwrap();
        let p = parameters();
        let (mut a, mut b) = (0x12345678, 0x12345678);
        let (mut sa, mut sb) = (Stats::default(), Stats::default());
        let draw = pipeline(Some(index), count);
        for y in 0..8 {
            draw(&selected, &p, y, 0, 32, &mut a, &mut sa);
            scanline_as(None, count, &generic, &p, y, 0, 32, &mut b, &mut sb);
        }
        assert_eq!((a, sa), (b, sb), "variant={index}");
        assert!(sa.pixels_out > 0, "variant={index} must exercise writes");
        assert!(
            (0..selected.fb.len()).all(|i| selected.fb.get(i) == generic.fb.get(i)),
            "variant={index}"
        );
    }
}

#[test]
fn invalid_variant_indices_keep_the_generic_fallback() {
    let regs = VARIANTS[0].1.map(|(_, value)| value);
    for count in 0..4 {
        let a = two_unit_state(regs);
        let b = two_unit_state(regs);
        let p = parameters();
        let (mut x, mut y) = (0, 0);
        let (mut sx, mut sy) = (Stats::default(), Stats::default());
        scanline_as(Some(usize::MAX), count, &a, &p, 0, 0, 32, &mut x, &mut sx);
        scanline_as(None, count, &b, &p, 0, 0, 32, &mut y, &mut sy);
        assert_eq!((x, sx), (y, sy));
        assert!((0..a.fb.len()).all(|i| a.fb.get(i) == b.fb.get(i)));
    }
}

#[test]
fn worker_dispatch_matches_whole_triangle_drawing() {
    let mut generic = VARIANTS[0].1.map(|(_, value)| value);
    generic[0] ^= 1 << 20;
    for mut regs in [
        VARIANTS[0].1.map(|(_, value)| value),
        VARIANTS[8].1.map(|(_, value)| value),
        generic,
    ] {
        regs[0] |= 3 << 9 | 7 << 5;
        let reference = two_unit_state(regs);
        let framebuffer = reference.fb.clone();
        let p = parameters();
        let verts = [(0.0, 0.0), (0.0, 16.0), (32.0, 16.0)];
        let mut expected = Stats::default();
        Job::Triangle {
            st: reference,
            p,
            verts,
            texcount: 2,
        }
        .run_all(&mut 0, &mut expected);
        for workers in [0, 1, 4] {
            let st = two_unit_state(regs);
            let output = st.fb.clone();
            let pool = Pool::new(workers);
            pool.submit(Job::Triangle {
                st,
                p,
                verts,
                texcount: 2,
            });
            pool.flush();
            assert_eq!(pool.stats(), expected, "workers={workers}");
            assert!(
                (0..output.len()).all(|i| output.get(i) == framebuffer.get(i)),
                "workers={workers}"
            );
        }
    }
}
