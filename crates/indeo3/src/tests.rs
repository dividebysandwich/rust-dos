use super::*;
use crate::tables::{REQUANT_TAB, VQ_TAB};

/// A frame of `planes` (Y, U, V, each starting with its motion vector
/// count), laid out as an encoder writes it: the headers, the VQ table
/// pairs for modes 1 and 4, the planes in the order Y, V, U, then 16
/// bytes of padding.
fn frame(width: u16, height: u16, flags: u16, planes: [&[u8]; 3]) -> Vec<u8> {
    let [y, u, v] = planes;
    let y_off = 48u32;
    let v_off = y_off + y.len() as u32;
    let u_off = v_off + v.len() as u32;
    let size = u_off + u.len() as u32 + 16;

    let mut bs = Vec::new();
    bs.extend(32u16.to_le_bytes());
    bs.extend(flags.to_le_bytes());
    bs.extend((size * 8).to_le_bytes());
    bs.extend([0, 0, 0, 0]); // cb_offset, reserved, checksum
    bs.extend(height.to_le_bytes());
    bs.extend(width.to_le_bytes());
    for off in [y_off, v_off, u_off, 0] {
        bs.extend(off.to_le_bytes());
    }
    bs.extend([0; 16]);
    bs.extend(y);
    bs.extend(v);
    bs.extend(u);
    bs.extend([0; 16]);

    let (num, word2) = (7u32, 0u32);
    let mut out = Vec::new();
    for w in [num, word2, num ^ word2 ^ size ^ 0x4652_4D48, size] {
        out.extend(w.to_le_bytes());
    }
    out.extend(bs);
    out
}

/// A plane of one cell, INTRA, whose data is `cell`: tree codes INTRA
/// (10) and VQ DATA (11).
fn intra_plane(cell: &[u8]) -> Vec<u8> {
    let mut p = vec![0, 0, 0, 0, 0b1011_0000];
    p.extend(cell);
    p
}

/// A plane of one cell, INTER with motion vector (y, x), copied: tree
/// codes INTER (11), its vector index, then VQ NULL (10 00).
fn copy_plane(mv_y: i8, mv_x: i8) -> Vec<u8> {
    vec![1, 0, 0, 0, mv_y as u8, mv_x as u8, 0b1110_0000, 0]
}

/// A 16x16 frame whose planes are filled from the row of 64 on top: a
/// cell of mode 0 whose first block takes the RLE code FB with a count
/// for the remaining blocks (15 of luma, none of chroma).
fn gray_frame() -> Vec<u8> {
    let y = intra_plane(&[0x00, 0xFB, 0x10]);
    let c = intra_plane(&[0x00, 0xFB, 0x01]);
    frame(16, 16, 0, [&y, &c, &c])
}

#[test]
fn sizes() {
    assert!(Decoder::new(640, 480).is_ok());
    assert!(Decoder::new(16, 16).is_ok());
    assert_eq!(Decoder::new(644, 480).err(), Some(Error::Size));
    assert_eq!(Decoder::new(12, 16).err(), Some(Error::Size));
    assert_eq!(Decoder::new(18, 16).err(), Some(Error::Size));
    assert_eq!(Decoder::new(640, 484).err(), Some(Error::Size));
}

#[test]
fn tables_hold_every_index() {
    for (i, t) in VQ_TAB.iter().enumerate() {
        assert_eq!(t.deltas.len(), t.deltas_m10.len(), "table {i}");
        assert!(t.num_dyads as usize <= t.deltas.len(), "table {i}");
        for code in t.num_dyads..248 {
            let q = (code - t.num_dyads) as usize;
            let (a, b) = (q / t.quad_exp as usize, q % t.quad_exp as usize);
            assert!(a < t.deltas.len() && b < t.deltas.len(), "table {i}");
        }
    }
    // The first pairs of set 1's first table: (0, 0), (2, 2), (-2, -2),
    // (-1, 3) and its mirror images.
    let t = VQ_TAB[0].deltas;
    assert_eq!(
        &t[..7],
        &[0, 0x0202, 0xFDFE, 0x02FF, 0xFD01, 0xFF03, 0x00FD]
    );
    assert_eq!(VQ_TAB[0].deltas_m10[1], 0x0202_0202);
    assert!(REQUANT_TAB.iter().flatten().all(|&v| v < 128));
}

#[test]
fn empty_and_sync_frames_keep_the_picture() {
    let mut d = Decoder::new(16, 16).unwrap();
    d.decode(&gray_frame()).unwrap();
    let before = d.yuv410();
    d.decode(&[]).unwrap();
    assert_eq!(d.yuv410(), before);

    // A sync frame: a bitstream header that says 16 bytes.
    let mut sync = Vec::new();
    for w in [0u32, 0, 16 ^ 0x4652_4D48, 16] {
        sync.extend(w.to_le_bytes());
    }
    sync.extend(32u16.to_le_bytes());
    sync.extend(0u16.to_le_bytes());
    sync.extend(128u32.to_le_bytes());
    sync.extend([0; 8]);
    d.decode(&sync).unwrap();
    assert_eq!(d.yuv410(), before);
}

#[test]
fn intra_fill() {
    let mut d = Decoder::new(16, 16).unwrap();
    d.decode(&gray_frame()).unwrap();
    let [y, u, v] = d.yuv410();
    assert_eq!(y, vec![0x80; 256]);
    assert_eq!(u, vec![0x80; 16]);
    assert_eq!(v, vec![0x80; 16]);
}

#[test]
fn intra_deltas() {
    // U: line 0 takes dyads 1 (+2, +2) and 0 (none) on the row of 64
    // above; FD copies it down the remaining 3 lines.
    let y = intra_plane(&[0x00, 0xFB, 0x10]);
    let u = intra_plane(&[0x00, 0x00, 0x01, 0xFD]);
    let v = intra_plane(&[0x00, 0xFB, 0x01]);
    let mut d = Decoder::new(16, 16).unwrap();
    d.decode(&frame(16, 16, 0, [&y, &u, &v])).unwrap();
    let [_, u, v] = d.yuv410();
    assert_eq!(u, [0x84, 0x84, 0x80, 0x80].repeat(4));
    assert_eq!(v, vec![0x80; 16]);
}

#[test]
fn inter_copy() {
    let mut d = Decoder::new(16, 16).unwrap();
    let y = intra_plane(&[0x00, 0xFB, 0x10]);
    let u = intra_plane(&[0x00, 0x00, 0x01, 0xFD]);
    d.decode(&frame(16, 16, 0, [&y, &u, &u])).unwrap();
    let first = d.yuv410();

    // The second frame goes to buffer 1 and copies all of buffer 0.
    let c = copy_plane(0, 0);
    d.decode(&frame(16, 16, 1 << 9, [&c, &c, &c])).unwrap();
    assert_eq!(d.yuv410(), first);

    // A vector up by 2 points past the row on top.
    let bad = copy_plane(-2, 0);
    let err = d.decode(&frame(16, 16, 0, [&bad, &c, &c]));
    assert!(matches!(err, Err(Error::Invalid(_))));
}

#[test]
fn bad_headers() {
    let mut d = Decoder::new(16, 16).unwrap();
    let mut f = gray_frame();
    f[8] ^= 1;
    assert!(d.decode(&f).is_err(), "checksum");

    let f = gray_frame();
    let mut wrong_size = Decoder::new(32, 16).unwrap();
    assert!(wrong_size.decode(&f).is_err());

    let mut f = gray_frame();
    f[18] |= 1 << 1;
    assert_eq!(d.decode(&f), Err(Error::Unsupported("8-bit pixels")));
}

#[test]
fn truncated_frames_fail() {
    let f = gray_frame();
    for len in 1..f.len() - 16 {
        let mut d = Decoder::new(16, 16).unwrap();
        assert!(d.decode(&f[..len]).is_err(), "length {len}");
    }
}

/// Damaged frames give errors or pictures, never a panic.
#[test]
fn damaged_frames_dont_panic() {
    // Mode 3, mode 10 and mode 1 cells, and a copy.
    let y3 = intra_plane(&[0x30, 0x00, 0x01, 0xFD, 0xFB, 0x07]);
    let y10 = intra_plane(&[0xA0, 0xFB, 0x04]);
    let u = intra_plane(&[0x10, 0x00, 0x01, 0xFD]);
    let c = copy_plane(1, 1);
    let frames = [
        frame(16, 16, 0, [&y3, &u, &u]),
        frame(16, 16, 0, [&y10, &u, &u]),
        frame(16, 16, 1 << 9, [&c, &c, &c]),
    ];
    let mut seed = 0x1234_5678_9ABC_DEF0u64;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut d = Decoder::new(16, 16).unwrap();
    for _ in 0..20000 {
        let mut f = frames[(rand() % 3) as usize].clone();
        for _ in 0..1 + rand() % 4 {
            // Keep the header's checksum, damage what follows.
            let i = 16 + (rand() as usize) % (f.len() - 16);
            f[i] = rand() as u8;
        }
        let _ = d.decode(&f);
    }
    let mut noise = vec![0u8; 200];
    for _ in 0..2000 {
        noise.iter_mut().for_each(|b| *b = rand() as u8);
        let _ = d.decode(&noise);
    }
}

#[test]
fn rgb_output() {
    let mut d = Decoder::new(16, 16).unwrap();
    d.decode(&gray_frame()).unwrap();
    // Y 128 is (128 - 16) * 255 / 219 = 130.4; U and V 128 add nothing.
    assert_eq!(d.bgr24_dib(), vec![130; 16 * 16 * 3]);

    let mut out = vec![0; 16 * 16 * 2];
    d.write_picture(PixelFormat::Rgb565, false, 32, &mut out)
        .unwrap();
    assert_eq!(&out[..2], &0x8410u16.to_le_bytes());
    d.write_picture(PixelFormat::Rgb555, false, 32, &mut out)
        .unwrap();
    assert_eq!(&out[..2], &0x4210u16.to_le_bytes());

    let mut short = vec![0; 16 * 16 * 3 - 1];
    let r = d.write_picture(PixelFormat::Bgr24, false, 48, &mut short);
    assert_eq!(r, Err(Error::Size));
}

#[test]
fn bottom_up_reverses_rows() {
    // Luma: each line of the top row of blocks adds 2 to the line above,
    // so the rows differ.
    let mut cell = vec![0x00];
    for _ in 0..16 {
        cell.extend([0x00, 0x01]);
    }
    cell.extend([0xFB, 0x0C]);
    let y = intra_plane(&cell);
    let c = intra_plane(&[0x00, 0xFB, 0x01]);
    let mut d = Decoder::new(16, 16).unwrap();
    d.decode(&frame(16, 16, 0, [&y, &c, &c])).unwrap();
    let mut top = vec![0; 16 * 48];
    let mut bottom = vec![0; 16 * 48];
    d.write_picture(PixelFormat::Bgr24, false, 48, &mut top)
        .unwrap();
    d.write_picture(PixelFormat::Bgr24, true, 48, &mut bottom)
        .unwrap();
    let rows: Vec<_> = top.chunks(48).rev().flatten().copied().collect();
    assert_eq!(rows, bottom);
}
