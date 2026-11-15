//! `FILL` and `BLT`: the card's 2D operations on card memory (chapter 9
//! of `docs/toadforce-spec.md`).
//!
//! Both take surfaces and rectangles `command::decode` checked, so every
//! pixel they name is inside `memory`.

use super::format::{Rect, Surface};

/// Write the low bytes of `value` to each pixel of `rect` of `dst`.
pub fn fill(memory: &mut [u8], dst: &Surface, rect: &Rect, value: u32) {
    let bytes = dst.format.bytes() as usize;
    let len = rect.width as usize * bytes;
    let pixel = value.to_le_bytes();
    for y in rect.top..rect.top + rect.height {
        let at = dst.at(rect.left, y);
        let row = &mut memory[at..at + len];
        match bytes {
            1 => row.fill(pixel[0]),
            _ => row
                .chunks_exact_mut(bytes)
                .for_each(|p| p.copy_from_slice(&pixel[..bytes])),
        }
    }
}

/// Copy `src_rect` of `src` to `dst_rect` of `dst`, whose pixels have the
/// same size: pixel for pixel where the rectangles' sizes are equal, and
/// otherwise each destination pixel from the source pixel under its
/// centre. With `key`, a source pixel that equals it in the bits the
/// source's format defines leaves its destination pixel alone. Where
/// the two rectangles overlap in memory, the result is as if the whole
/// source were read first.
pub fn blt(
    memory: &mut [u8],
    dst: &Surface,
    dst_rect: &Rect,
    src: &Surface,
    src_rect: &Rect,
    key: Option<u32>,
) {
    let bytes = src.format.bytes() as usize;
    let row_len = src_rect.width as usize * bytes;
    // The source's rows, packed, when the destination may overwrite
    // source pixels still to be read.
    let (src_span, dst_span) = (src.span(src_rect), dst.span(dst_rect));
    let overlap = src_span.0 < dst_span.1 && dst_span.0 < src_span.1;
    let copy: Option<Vec<u8>> = overlap.then(|| {
        let mut copy = Vec::with_capacity(row_len * src_rect.height as usize);
        for y in src_rect.top..src_rect.top + src_rect.height {
            let at = src.at(src_rect.left, y);
            copy.extend_from_slice(&memory[at..at + row_len]);
        }
        copy
    });
    // Where line `y` of the source rectangle begins, in `copy` or in
    // card memory.
    let src_row = |y: u32| match &copy {
        Some(_) => y as usize * row_len,
        None => src.at(src_rect.left, src_rect.top + y),
    };
    let same_size = (src_rect.width, src_rect.height) == (dst_rect.width, dst_rect.height);
    let key = key.map(|k| (k & src.format.defined_bits(), src.format.defined_bits()));
    let mut row = Vec::new();
    for y in 0..dst_rect.height {
        let sy = if same_size {
            y
        } else {
            centre(y, dst_rect.height, src_rect.height)
        };
        let from = src_row(sy);
        let at = dst.at(dst_rect.left, dst_rect.top + y);
        if same_size && key.is_none() {
            // A whole line, copied once. Without `copy` the rectangles
            // share no byte of card memory, so a line read after another
            // was written still holds the source's pixels.
            match &copy {
                Some(copy) => memory[at..at + row_len].copy_from_slice(&copy[from..from + row_len]),
                None => memory.copy_within(from..from + row_len, at),
            }
            continue;
        }
        // A line of the source, apart from card memory, so that the
        // destination can be written while it is read.
        row.clear();
        row.extend_from_slice(match &copy {
            Some(copy) => &copy[from..from + row_len],
            None => &memory[from..from + row_len],
        });
        let out = &mut memory[at..at + dst_rect.width as usize * bytes];
        for (x, pixel) in out.chunks_exact_mut(bytes).enumerate() {
            let sx = if same_size {
                x
            } else {
                centre(x as u32, dst_rect.width, src_rect.width) as usize
            };
            let source = &row[sx * bytes..(sx + 1) * bytes];
            if let Some((key, bits)) = key {
                let mut value = [0u8; 4];
                value[..bytes].copy_from_slice(source);
                if u32::from_le_bytes(value) & bits == key {
                    continue;
                }
            }
            pixel.copy_from_slice(source);
        }
    }
}

/// The source pixel under the centre of pixel `i` of `dst` stretched
/// from `src` pixels: `((2i + 1) * src) / (2 * dst)`, rounded down.
fn centre(i: u32, dst: u32, src: u32) -> u32 {
    ((2 * i as u64 + 1) * src as u64 / (2 * dst as u64)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toadforce::format::Format;

    fn surface(offset: u32, width: u32, height: u32, format: Format) -> Surface {
        Surface {
            offset,
            pitch: width * format.bytes() + 3,
            width,
            height,
            format,
        }
    }

    fn rect(left: u32, top: u32, width: u32, height: u32) -> Rect {
        Rect {
            left,
            top,
            width,
            height,
        }
    }

    fn pixel(memory: &[u8], s: &Surface, x: u32, y: u32) -> u32 {
        let at = s.at(x, y);
        let mut value = [0u8; 4];
        let bytes = s.format.bytes() as usize;
        value[..bytes].copy_from_slice(&memory[at..at + bytes]);
        u32::from_le_bytes(value)
    }

    fn set(memory: &mut [u8], s: &Surface, x: u32, y: u32, value: u32) {
        fill(memory, s, &rect(x, y, 1, 1), value);
    }

    #[test]
    fn a_fill_writes_its_rectangle_and_nothing_else() {
        for format in [Format::P8, Format::R5G6B5, Format::A8R8G8B8] {
            let s = surface(16, 8, 4, format);
            let mut memory = vec![0xEEu8; 512];
            fill(&mut memory, &s, &rect(2, 1, 3, 2), 0xA1B2_C3D4);
            let low = 0xA1B2_C3D4 & format.defined_bits();
            let untouched = 0xEEEE_EEEE & format.defined_bits();
            for y in 0..4 {
                for x in 0..8 {
                    let inside = (2..5).contains(&x) && (1..3).contains(&y);
                    let expected = if inside { low } else { untouched };
                    assert_eq!(pixel(&memory, &s, x, y), expected, "{format:?} {x},{y}");
                }
            }
            // The bytes between the lines' ends and the next lines.
            let written = memory.iter().filter(|&&b| b != 0xEE).count();
            assert_eq!(written, 6 * format.bytes() as usize);
        }
    }

    #[test]
    fn a_fill_writes_the_bits_the_format_doesnt_define() {
        let s = surface(0, 2, 1, Format::X8R8G8B8);
        let mut memory = vec![0u8; 16];
        fill(&mut memory, &s, &rect(1, 0, 1, 1), 0xA1B2_C3D4);
        assert_eq!(memory[4..8], [0xD4, 0xC3, 0xB2, 0xA1]);
        assert_eq!(memory[..4], [0; 4]);
        // And a copy keeps them.
        blt(
            &mut memory,
            &s,
            &rect(0, 0, 1, 1),
            &s,
            &rect(1, 0, 1, 1),
            None,
        );
        assert_eq!(memory[..4], [0xD4, 0xC3, 0xB2, 0xA1]);
    }

    #[test]
    fn a_copy_moves_pixels_between_surfaces_of_different_pitch() {
        let src = surface(0, 6, 4, Format::R5G6B5);
        let dst = Surface {
            pitch: 40,
            ..surface(256, 10, 6, Format::R5G6B5)
        };
        let mut memory = vec![0u8; 1024];
        for y in 0..4 {
            for x in 0..6 {
                set(&mut memory, &src, x, y, 0x100 + y * 16 + x);
            }
        }
        blt(
            &mut memory,
            &dst,
            &rect(3, 2, 4, 3),
            &src,
            &rect(1, 1, 4, 3),
            None,
        );
        for y in 0..3 {
            for x in 0..4 {
                assert_eq!(
                    pixel(&memory, &dst, 3 + x, 2 + y),
                    0x100 + (y + 1) * 16 + x + 1
                );
            }
        }
        assert_eq!(pixel(&memory, &dst, 2, 2), 0);
        assert_eq!(pixel(&memory, &dst, 7, 4), 0);
    }

    #[test]
    fn an_overlapping_copy_reads_its_source_first() {
        // One surface, the rectangle moved by one pixel each way: every
        // direction of copying overwrites source pixels it hasn't read.
        let s = surface(0, 8, 8, Format::P8);
        for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1), (1, 1), (-1, -1)] {
            let mut memory = vec![0u8; 256];
            for y in 0..8 {
                for x in 0..8 {
                    set(&mut memory, &s, x, y, 1 + y * 8 + x);
                }
            }
            let before = memory.clone();
            let from = rect(2, 2, 4, 4);
            let to = rect((2 + dx) as u32, (2 + dy) as u32, 4, 4);
            blt(&mut memory, &s, &to, &s, &from, None);
            for y in 0..4 {
                for x in 0..4 {
                    assert_eq!(
                        pixel(&memory, &s, to.left + x, to.top + y),
                        pixel(&before, &s, 2 + x, 2 + y),
                        "moved by {dx},{dy}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_stretch_takes_the_pixel_under_each_centre() {
        // 4 pixels to 8: each twice. 4 to 2: the second and the fourth.
        // 3 to 5: 0, 0, 1, 2, 2.
        let picks = |dst, src| (0..dst).map(|i| centre(i, dst, src)).collect::<Vec<_>>();
        assert_eq!(picks(8, 4), [0, 0, 1, 1, 2, 2, 3, 3]);
        assert_eq!(picks(2, 4), [1, 3]);
        assert_eq!(picks(5, 3), [0, 0, 1, 2, 2]);
        assert_eq!(picks(1, 4096), [2048]);

        let src = surface(0, 2, 2, Format::X8R8G8B8);
        let dst = surface(64, 4, 4, Format::X8R8G8B8);
        let mut memory = vec![0u8; 256];
        for (i, value) in [0x11, 0x22, 0x33, 0x44].into_iter().enumerate() {
            set(&mut memory, &src, i as u32 % 2, i as u32 / 2, value);
        }
        let (all_dst, all_src) = (rect(0, 0, 4, 4), rect(0, 0, 2, 2));
        blt(&mut memory, &dst, &all_dst, &src, &all_src, None);
        let rows: Vec<u32> = (0..16)
            .map(|i| pixel(&memory, &dst, i % 4, i / 4))
            .collect();
        assert_eq!(
            rows,
            [
                0x11, 0x11, 0x22, 0x22, 0x11, 0x11, 0x22, 0x22, 0x33, 0x33, 0x44, 0x44, 0x33, 0x33,
                0x44, 0x44
            ]
        );
    }

    #[test]
    fn an_overlapping_stretch_reads_its_source_first() {
        let s = surface(0, 8, 1, Format::P8);
        let mut memory = vec![0u8; 16];
        for x in 0..8 {
            set(&mut memory, &s, x, 0, 10 + x);
        }
        // The left half stretched over the whole line.
        blt(
            &mut memory,
            &s,
            &rect(0, 0, 8, 1),
            &s,
            &rect(0, 0, 4, 1),
            None,
        );
        assert_eq!(&memory[..8], [10, 10, 11, 11, 12, 12, 13, 13]);
    }

    #[test]
    fn a_colour_key_compares_the_bits_the_format_defines() {
        let src = surface(0, 4, 1, Format::X8R8G8B8);
        let dst = surface(64, 4, 1, Format::X8R8G8B8);
        let mut memory = vec![0u8; 128];
        // The key's colour with other unused bits, and another colour.
        for (x, value) in [0xFF00_FF00u32, 0x0000_FF00, 0x0000_FF01, 0x7F00_FF00]
            .into_iter()
            .enumerate()
        {
            set(&mut memory, &src, x as u32, 0, value);
        }
        fill(&mut memory, &dst, &rect(0, 0, 4, 1), 0x5555_5555);
        let all = rect(0, 0, 4, 1);
        blt(&mut memory, &dst, &all, &src, &all, Some(0xAA00_FF00));
        let row: Vec<u32> = (0..4).map(|x| pixel(&memory, &dst, x, 0)).collect();
        assert_eq!(row, [0x5555_5555, 0x5555_5555, 0x0000_FF01, 0x5555_5555]);

        // With alpha defined, it takes part.
        let src = Surface {
            format: Format::A8R8G8B8,
            ..src
        };
        blt(&mut memory, &dst, &all, &src, &all, Some(0xFF00_FF00));
        let row: Vec<u32> = (0..4).map(|x| pixel(&memory, &dst, x, 0)).collect();
        assert_eq!(row, [0x5555_5555, 0x0000_FF00, 0x0000_FF01, 0x7F00_FF00]);
    }

    #[test]
    fn a_colour_key_of_a_small_pixel_is_the_keys_low_bits() {
        let all = rect(0, 0, 4, 1);
        // `P8`: the key's low byte, whatever is above it.
        let src = surface(0, 4, 1, Format::P8);
        let dst = surface(64, 4, 1, Format::P8);
        let mut memory = vec![0u8; 128];
        for (x, value) in [7u32, 8, 7, 0x107].into_iter().enumerate() {
            set(&mut memory, &src, x as u32, 0, value);
        }
        fill(&mut memory, &dst, &all, 0x55);
        blt(&mut memory, &dst, &all, &src, &all, Some(0xFFFF_FF07));
        let row: Vec<u32> = (0..4).map(|x| pixel(&memory, &dst, x, 0)).collect();
        assert_eq!(row, [0x55, 8, 0x55, 0x55]);
        // No key: every pixel is copied, the key's among them.
        blt(&mut memory, &dst, &all, &src, &all, None);
        let row: Vec<u32> = (0..4).map(|x| pixel(&memory, &dst, x, 0)).collect();
        assert_eq!(row, [7, 8, 7, 7]);

        // `R5G6B5`: the low 16 bits, all of which the format defines.
        let src = surface(0, 4, 1, Format::R5G6B5);
        let dst = surface(64, 4, 1, Format::R5G6B5);
        let mut memory = vec![0u8; 128];
        for (x, value) in [0xF81Fu32, 0xF81E, 0x781F, 0xF81F].into_iter().enumerate() {
            set(&mut memory, &src, x as u32, 0, value);
        }
        fill(&mut memory, &dst, &all, 0x5555);
        blt(&mut memory, &dst, &all, &src, &all, Some(0xABCD_F81F));
        let row: Vec<u32> = (0..4).map(|x| pixel(&memory, &dst, x, 0)).collect();
        assert_eq!(row, [0x5555, 0xF81E, 0x781F, 0x5555]);
        // A stretch keys the pixels it takes.
        let wide = rect(0, 0, 8, 1);
        let dst = surface(64, 8, 1, Format::R5G6B5);
        fill(&mut memory, &dst, &wide, 0x5555);
        blt(&mut memory, &dst, &wide, &src, &all, Some(0xF81F));
        let row: Vec<u32> = (0..8).map(|x| pixel(&memory, &dst, x, 0)).collect();
        assert_eq!(
            row,
            [
                0x5555, 0x5555, 0xF81E, 0xF81E, 0x781F, 0x781F, 0x5555, 0x5555
            ]
        );
    }
}
