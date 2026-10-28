//! Blits from 8-bit DIB sections to the screen, as Wages of War draws
//! its tactical map on a desktop of 256 colours: the pixels they leave,
//! and how long one takes.

use super::*;
use crate::config::DesktopDepth;
use crate::win32::user32::paint::get_dc;

const SRCCOPY: u32 = 0x00CC_0020;

/// Guest memory for the structures the calls read.
const SCRATCH: u32 = 0x0008_0000;

/// Where the DIB sections' bits are.
const BITS: u32 = 0x0010_0000;

/// A machine with a desktop of `depth` on a Voodoo3, as the game runs,
/// and the screen's DC with a palette of `colors` realized.
fn machine(depth: DesktopDepth, colors: &[u32]) -> (Cpu, u32) {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    cpu.bus.win32.desktop_depth = depth;
    cpu.bus
        .configure_voodoo(Some(crate::voodoo::Board::Voodoo3));
    crate::win32::user32::paint::ensure_desktop(&mut cpu);
    let screen = get_dc(&mut cpu, &[0]).unwrap();
    // LOGPALETTE: version 0x300, the count, PC_NOCOLLAPSE entries.
    cpu.bus
        .guest_write_32(SCRATCH, 0x300 | (colors.len() as u32) << 16);
    for (i, &c) in colors.iter().enumerate() {
        cpu.bus
            .guest_write_32(SCRATCH + 4 + 4 * i as u32, c | 0x0400_0000);
    }
    let hpal = palette::create_palette(&mut cpu, &[SCRATCH]).unwrap();
    palette::select_palette(&mut cpu, &[screen, hpal, 0]);
    palette::realize_palette(&mut cpu, &[screen]);
    (cpu, screen)
}

/// A memory DC holding a 640x480 DIB section of 8 bits a pixel, its
/// bits at `bits` in guest memory as CreateDIBSection would place them,
/// its color table `colors` and its pixels `pixel(x, y)`.
fn dib_dc(
    cpu: &mut Cpu,
    bits: u32,
    colors: &[u32],
    top_down: bool,
    pixel: impl Fn(i32, i32) -> u8,
) -> u32 {
    let (w, h) = (640, 480);
    let bitmap = Bitmap {
        width: w,
        height: h,
        bpp: 8,
        planes: 1,
        stride: w as u32,
        bits: Vec::new(),
        dib: Some(Dib {
            bits,
            header: vec![0; 40],
            colors: colors.to_vec(),
            masks: [0; 3],
            top_down,
            compression: 0,
            section: 0,
            offset: 0,
        }),
        dc: 0,
        dimension: (0, 0),
    };
    let dib = add(cpu, Kind::Bitmap { bitmap });
    for y in 0..h {
        let row: Vec<u8> = (0..w).map(|x| pixel(x, y)).collect();
        let stored = if top_down { y } else { h - 1 - y };
        cpu.bus.guest_write_bytes(bits + (stored * w) as u32, &row);
    }
    let mem = dc::create_compatible_dc(cpu, &[0]).unwrap();
    dc::select_object(cpu, &[mem, dib]);
    mem
}

/// The game's palette: shades that the static colors don't have.
fn game_colors() -> Vec<u32> {
    (0..256u32)
        .map(|i| (i * 7 % 256) | (i * 13 % 256) << 8 | (i * 29 % 256) << 16)
        .collect()
}

/// A picture of tiles, every pixel's index from its place.
fn tile_pixel(x: i32, y: i32) -> u8 {
    ((x / 3) ^ (y * 5) ^ (x * y / 64)) as u8
}

fn bit_blt(
    cpu: &mut Cpu,
    dst: u32,
    (x, y, w, h): (i32, i32, i32, i32),
    src: u32,
    sx: i32,
    sy: i32,
) {
    let args = [
        dst, x as u32, y as u32, w as u32, h as u32, src, sx as u32, sy as u32, SRCCOPY,
    ];
    assert_eq!(draw::bit_blt(cpu, &args), Some(TRUE));
}

/// The screen's rows as GDI works on them.
fn screen_pixels(cpu: &mut Cpu) -> Vec<u8> {
    let (_, h, _, _) = surface::screen(cpu);
    crate::win32::screen::load_desktop(cpu, 0, h as u32)
}

/// A blit of a DIB whose color table is the realized palette puts the
/// system palette entries the palette's entries took, and leaves the
/// rest of the screen.
#[test]
fn a_blit_of_the_palettes_colors_puts_their_system_entries() {
    let colors = game_colors();
    let (mut cpu, screen) = machine(DesktopDepth::Palette, &colors);
    let mem = dib_dc(&mut cpu, BITS, &colors, false, tile_pixel);
    let hpal = gdi(&mut cpu).dc(screen).unwrap().attrs.palette;
    let mapping = gdi(&mut cpu).palette(hpal).unwrap().mapping.clone();
    let before = screen_pixels(&mut cpu);
    bit_blt(&mut cpu, screen, (100, 50, 32, 32), mem, 300, 200);
    let pixels = screen_pixels(&mut cpu);
    for y in 0..480 {
        for x in 0..640 {
            let at = (y * 640 + x) as usize;
            let want = if (100..132).contains(&x) && (50..82).contains(&y) {
                mapping[tile_pixel(200 + x, 150 + y) as usize]
            } else {
                before[at]
            };
            assert_eq!(pixels[at], want, "({x}, {y})");
        }
    }
}

/// Blits of every kind a fast path could take or has to leave to the
/// one that works a pixel at a time: tiles, the whole screen, ones cut
/// by the screen's and the DIB's edges, through a clip region of several
/// rectangles and a moved viewport, mirrored and stretched ones, other
/// raster operations, a top-down DIB and one whose colors aren't the
/// palette's. A hash of the screen after them.
fn blit_everything(depth: DesktopDepth) -> u64 {
    let colors = game_colors();
    let (mut cpu, screen) = machine(depth, &colors);
    let mem = dib_dc(&mut cpu, BITS, &colors, false, tile_pixel);
    let grays: Vec<u32> = (0..256u32).map(|i| i * 0x01_0101).collect();
    let other = BITS + 640 * 480;
    let gray = dib_dc(&mut cpu, other, &grays, true, |x, y| (x + 2 * y) as u8);
    let blits = [
        ((0, 0, 640, 480), 0, 0),
        ((100, 50, 32, 32), 300, 200),
        ((620, 470, 32, 32), 10, 10),
        ((-10, -5, 32, 32), 600, 460),
        ((5, 7, 1, 1), 639, 479),
        ((40, 40, 64, 3), 0, 478),
        ((200, 300, 0, 10), 0, 0),
    ];
    for (r, sx, sy) in blits {
        bit_blt(&mut cpu, screen, r, mem, sx, sy);
    }
    bit_blt(&mut cpu, screen, (300, 20, 40, 40), gray, 100, 100);
    // Mirrored, and stretched.
    bit_blt(&mut cpu, screen, (400, 100, -20, 16), mem, 33, 44);
    let neg = |v: i32| v as u32;
    let args = [screen, 450, 150, neg(-20), 16, mem, 33, 44, 20, 16, SRCCOPY];
    assert_eq!(draw::stretch_blt(&mut cpu, &args), Some(TRUE));
    let args = [screen, 500, 200, 50, 30, mem, 1, 2, 25, 40, SRCCOPY];
    assert_eq!(draw::stretch_blt(&mut cpu, &args), Some(TRUE));
    // SRCINVERT and NOTSRCCOPY.
    let mut args = [screen, 10, 300, 48, 48, mem, 77, 99, 0x0066_0046];
    assert_eq!(draw::bit_blt(&mut cpu, &args), Some(TRUE));
    args[8] = 0x0033_0008;
    args[1] = 70;
    assert_eq!(draw::bit_blt(&mut cpu, &args), Some(TRUE));
    // A clip region with a hole, and the viewport moved.
    dc::exclude_clip_rect(&mut cpu, &[screen, 220, 220, 240, 240]);
    dc::set_viewport_org_ex(&mut cpu, &[screen, 7, 3, 0]);
    bit_blt(&mut cpu, screen, (200, 200, 64, 64), mem, 50, 60);
    bit_blt(&mut cpu, screen, (210, 260, 64, 64), gray, 5, 6);
    // Between bitmaps, cut by their edges: into the top-down DIB, whose
    // rows go back to the program's memory, and into a bitmap of GDI's,
    // from itself too; then from each onto the screen.
    bit_blt(&mut cpu, gray, (600, 10, 64, 20), mem, 7, 9);
    bit_blt(&mut cpu, gray, (-8, 470, 30, 30), mem, 100, 100);
    let ddb = bitmap::create_compatible_bitmap(&mut cpu, &[screen, 300, 200]).unwrap();
    let dev = dc::create_compatible_dc(&mut cpu, &[screen]).unwrap();
    dc::select_object(&mut cpu, &[dev, ddb]);
    bit_blt(&mut cpu, dev, (0, 0, 300, 200), mem, 0, 0);
    bit_blt(&mut cpu, dev, (250, 150, 64, 64), gray, 590, 0);
    bit_blt(&mut cpu, dev, (10, 20, 30, 40), dev, 15, 25);
    bit_blt(&mut cpu, screen, (330, 0, 300, 200), dev, 0, 0);
    bit_blt(&mut cpu, screen, (0, 400, 640, 80), gray, 0, 400);
    bit_blt(&mut cpu, screen, (560, 0, 80, 40), gray, 560, 0);
    fnv(&screen_pixels(&mut cpu))
}

fn fnv(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
    })
}

#[test]
fn blits_to_the_palette_screen_leave_the_same_pixels() {
    let hash = blit_everything(DesktopDepth::Palette);
    assert_eq!(hash, 0xb63f_3df0_a605_f819, "{hash:#018x}");
}

/// The Voodoo3's own desktop is 16 bits a pixel.
#[test]
fn blits_to_the_16_bit_screen_leave_the_same_pixels() {
    let hash = blit_everything(DesktopDepth::Adapter);
    assert_eq!(hash, 0x1280_930d_72d1_d005, "{hash:#018x}");
}

/// How long the game's blits take: a 32x32 tile and the whole screen.
/// `cargo test --release --lib gdi32::draw_tests::bench -- --ignored
/// --nocapture`.
#[test]
#[ignore]
fn bench_bit_blt_dib_to_palette_screen() {
    let colors = game_colors();
    let (mut cpu, screen) = machine(DesktopDepth::Palette, &colors);
    let mem = dib_dc(&mut cpu, BITS, &colors, false, tile_pixel);
    let time = |cpu: &mut Cpu, r, n: u32| {
        let start = std::time::Instant::now();
        for i in 0..n {
            let (sx, sy) = ((i * 32 % 600) as i32, (i * 32 % 440) as i32);
            bit_blt(cpu, screen, r, mem, sx, sy);
        }
        start.elapsed().as_secs_f64() * 1e6 / n as f64
    };
    let tile = time(&mut cpu, (164, 164, 32, 32), 20_000);
    let full = time(&mut cpu, (0, 0, 640, 480), 200);
    println!("32x32: {tile:.2} us a blit, 640x480: {full:.1} us a blit");
}
