//! The 3dfx Voodoo Graphics as Glide sees it: the PCI configuration space
//! through ports CF8h/CFCh and the PCI BIOS, BAR0 sizing and relocation,
//! the init registers behind initEnable, the DAC detection, the access
//! sizes the card takes, and what it draws: fastfills, triangles with
//! and without textures, depth, blending, fog, the frame buffer writes
//! and reads, swaps and the picture it shows.

mod pmrig;

use iced_x86::code_asm::*;
use pmrig::*;
use rust_dos::bus::Bus;
use rust_dos::voodoo::Board;
use std::path::PathBuf;

const BASE: usize = 0xD000_0000;

// Registers, as byte offsets.
const STATUS: u32 = 0x000;
const VERTEX_AX: u32 = 0x008;
const START_R: u32 = 0x020;
const START_G: u32 = 0x024;
const START_B: u32 = 0x028;
const START_Z: u32 = 0x02C;
const START_A: u32 = 0x030;
const START_S: u32 = 0x034;
const START_T: u32 = 0x038;
const D_R_DX: u32 = 0x040;
const D_S_DX: u32 = 0x054;
const D_T_DX: u32 = 0x058;
const D_R_DY: u32 = 0x060;
const D_S_DY: u32 = 0x074;
const D_T_DY: u32 = 0x078;
const TRIANGLE_CMD: u32 = 0x080;
const FBZ_COLOR_PATH: u32 = 0x104;
const FOG_MODE: u32 = 0x108;
const ALPHA_MODE: u32 = 0x10C;
const FBZ_MODE: u32 = 0x110;
const LFB_MODE: u32 = 0x114;
const CLIP_LEFT_RIGHT: u32 = 0x118;
const CLIP_LOW_Y_HIGH_Y: u32 = 0x11C;
const NOP_CMD: u32 = 0x120;
const FASTFILL_CMD: u32 = 0x124;
const SWAPBUFFER_CMD: u32 = 0x128;
const FOG_COLOR: u32 = 0x12C;
const ZA_COLOR: u32 = 0x130;
const CHROMA_KEY: u32 = 0x134;
const COLOR0: u32 = 0x144;
const COLOR1: u32 = 0x148;
const FBI_PIXELS_OUT: u32 = 0x15C;
const FOG_TABLE: u32 = 0x160;
const BACK_PORCH: u32 = 0x208;
const VIDEO_DIMENSIONS: u32 = 0x20C;
const FBI_INIT0: u32 = 0x210;
const FBI_INIT1: u32 = 0x214;
const FBI_INIT2: u32 = 0x218;
const FBI_INIT3: u32 = 0x21C;
const H_SYNC: u32 = 0x220;
const V_SYNC: u32 = 0x224;
const CLUT_DATA: u32 = 0x228;
const DAC_DATA: u32 = 0x22C;
const TEXTURE_MODE: u32 = 0x300;
const T_LOD: u32 = 0x304;
const TEX_BASE_ADDR: u32 = 0x30C;
const TREX_INIT1: u32 = 0x320;

// fbzMode bits.
const RGB_WRITE: u32 = 1 << 9;
const AUX_WRITE: u32 = 1 << 10;
const DEPTH_TEST: u32 = 1 << 4;
const CLIPPING: u32 = 1 << 0;
const CHROMA: u32 = 1 << 1;
const DRAW_BACK: u32 = 1 << 14;
const Y_ORIGIN: u32 = 1 << 17;

/// A machine with a 3dfx card.
fn bus(board: Board) -> Bus {
    let mut bus = Bus::new(PathBuf::from("."));
    bus.set_cycles_per_ms(1000);
    bus.configure_voodoo(Some(board));
    bus
}

fn cfg_read(bus: &mut Bus, device: u32, reg: u32) -> u32 {
    bus.io_write_wide(0xCF8, 0x8000_0000 | device << 11 | reg, 4);
    bus.io_read_wide(0xCFC, 4)
}

fn cfg_write(bus: &mut Bus, device: u32, reg: u32, value: u32) {
    bus.io_write_wide(0xCF8, 0x8000_0000 | device << 11 | reg, 4);
    bus.io_write_wide(0xCFC, value, 4);
}

fn w(bus: &mut Bus, reg: u32, value: u32) {
    bus.write_32(BASE + reg as usize, value);
}

fn r(bus: &Bus, reg: u32) -> u32 {
    bus.read_32(BASE + reg as usize)
}

/// Let `ms` milliseconds of emulated time pass, running due timer events.
fn wait_ms(bus: &mut Bus, ms: f64) {
    let target = bus.clock.icount + (ms * 1000.0) as u64;
    while bus.clock.icount < target {
        let step = (target - bus.clock.icount).min(10);
        bus.clock.icount += step;
        if bus.clock.icount >= bus.clock.deadline {
            bus.service_timers();
        }
    }
}

/// Set the card up as Glide does for 640x480: init writes on, 640 pixel
/// rows (10 tiles), the second buffer after 150 pages, the timing, the
/// clock running and the output on.
fn init(bus: &mut Bus) {
    cfg_write(bus, 0, 0x40, 1);
    w(bus, FBI_INIT1, 10 << 4);
    w(bus, FBI_INIT2, 150 << 11);
    w(bus, FBI_INIT3, 0);
    w(bus, BACK_PORCH, 2 << 16 | 40);
    w(bus, H_SYNC, 800 - 96 - 2 << 16 | 96 - 1);
    w(bus, V_SYNC, 525 - 2 << 16 | 2);
    w(bus, VIDEO_DIMENSIONS, 479 << 16 | 639);
    cfg_write(bus, 0, 0xC0, 0);
    w(bus, FBI_INIT0, 1);
    cfg_write(bus, 0, 0x40, 0);
    // Clip to the screen, write colours.
    w(bus, CLIP_LEFT_RIGHT, 640);
    w(bus, CLIP_LOW_Y_HIGH_Y, 480);
}

/// A pixel of the colour buffer `buffer` (0 or 1 of the two 640x480 ones).
fn pixel(bus: &Bus, buffer: usize, x: usize, y: usize) -> u16 {
    let v = bus.voodoo.as_ref().unwrap();
    v.frame_buffer().get(buffer * 150 * 0x1000 / 2 + y * 640 + x)
}

/// A pixel of the auxiliary (depth) buffer.
fn depth(bus: &Bus, x: usize, y: usize) -> u16 {
    let v = bus.voodoo.as_ref().unwrap();
    v.frame_buffer().get(2 * 150 * 0x1000 / 2 + y * 640 + x)
}

/// A triangle with vertices in pixels.
fn triangle(bus: &mut Bus, verts: [(f32, f32); 3]) {
    for (i, (x, y)) in verts.iter().enumerate() {
        w(bus, VERTEX_AX + 8 * i as u32, (x * 16.0) as i32 as u32);
        w(bus, VERTEX_AX + 4 + 8 * i as u32, (y * 16.0) as i32 as u32);
    }
    w(bus, TRIANGLE_CMD, 0);
}

/// A flat colour for the next triangles: iterated RGBA without slopes.
fn flat(bus: &mut Bus, r_: u32, g: u32, b: u32, a: u32) {
    w(bus, START_R, r_ << 12);
    w(bus, START_G, g << 12);
    w(bus, START_B, b << 12);
    w(bus, START_A, a << 12);
    for reg in [D_R_DX, D_R_DX + 4, D_R_DX + 8, D_R_DX + 16, D_R_DY, D_R_DY + 4, D_R_DY + 8, D_R_DY + 16] {
        w(bus, reg, 0);
    }
}

#[test]
fn the_card_is_device_0_of_the_pci_bus() {
    let mut bus = bus(Board::Max);
    assert!(bus.pci_present());
    assert_eq!(cfg_read(&mut bus, 0, 0x00), 0x0001_121A);
    assert_eq!(cfg_read(&mut bus, 0, 0x08), 0x0400_0002, "revision 2, multimedia video");
    assert_eq!(cfg_read(&mut bus, 0, 0x10), 0xD000_0008, "BAR0, prefetchable");
    assert_eq!(cfg_read(&mut bus, 0, 0x04) & 0xFFFF, 0x0002, "memory decoding on");
    assert_eq!(cfg_read(&mut bus, 0, 0x3C) & 0xFF, 0xFF, "no interrupt");
    // Nothing at device 1 without the S3, or at 2.
    assert_eq!(cfg_read(&mut bus, 1, 0x00), 0xFFFF_FFFF);
    assert_eq!(cfg_read(&mut bus, 2, 0x00), 0xFFFF_FFFF);
}

#[test]
fn the_pci_bios_finds_the_card() {
    let mut cpu = rust_dos::cpu::Cpu::new(PathBuf::from("."));
    cpu.bus.configure_voodoo(Some(Board::Max));
    // B102h: find device 0001h of vendor 121Ah.
    cpu.set_ax(0xB102);
    cpu.set_cx(0x0001);
    cpu.set_dx(0x121A);
    cpu.set_si(0);
    rust_dos::pci::bios(&mut cpu);
    assert_eq!(cpu.get_reg8(iced_x86::Register::AH), 0);
    assert_eq!(cpu.bx(), 0x0000, "bus 0, device 0, function 0");
    // B10Ah: read its BAR0.
    cpu.set_ax(0xB10A);
    cpu.set_di(0x10);
    rust_dos::pci::bios(&mut cpu);
    assert_eq!(cpu.ecx(), 0xD000_0008);
}

#[test]
fn no_pci_bus_without_the_card_or_the_s3() {
    let mut bus = Bus::new(PathBuf::from("."));
    assert!(!bus.pci_present());
    assert_eq!(cfg_read(&mut bus, 0, 0x00), 0xFFFF_FFFF);
}

#[test]
fn bar0_is_16_mb_and_moves_the_window() {
    let mut bus = bus(Board::Standard);
    cfg_write(&mut bus, 0, 0x10, 0xFFFF_FFFF);
    assert_eq!(cfg_read(&mut bus, 0, 0x10), 0xFF00_0008, "sizing: 16 MB");
    cfg_write(&mut bus, 0, 0x10, 0xE800_0000);
    assert_eq!(cfg_read(&mut bus, 0, 0x10), 0xE800_0008);
    // The registers answer there now, and not at the old place.
    cfg_write(&mut bus, 0, 0x40, 1);
    bus.write_32(0xE800_0000 + FBI_INIT3 as usize, 0x1234_0000);
    assert_eq!(bus.read_32(0xE800_0000 + FBI_INIT3 as usize), 0x1234_0000);
    assert_eq!(bus.read_32(BASE + FBI_INIT3 as usize), 0xFFFF_FFFF);
}

#[test]
fn a_bar_over_the_ram_maps_nothing() {
    let mut bus = bus(Board::Standard);
    cfg_write(&mut bus, 0, 0x10, 0);
    cfg_write(&mut bus, 0, 0x40, 1);
    bus.write_32(0x0010_0000 + FBI_INIT3 as usize, 0x55AA);
    assert_eq!(bus.read_32(0x0010_0000 + FBI_INIT3 as usize), 0x55AA, "RAM, not the card");
    assert_eq!(bus.voodoo.as_ref().unwrap().reg[0x21C / 4], 0x001E_4000);
}

#[test]
fn init_registers_need_init_enable() {
    let mut bus = bus(Board::Standard);
    let before = r(&bus, FBI_INIT1);
    w(&mut bus, FBI_INIT1, 0xAAAA);
    assert_eq!(r(&bus, FBI_INIT1), before, "ignored without initEnable");
    // Glide reads initEnable, sets bit 0 and writes it back.
    let enable = cfg_read(&mut bus, 0, 0x40);
    cfg_write(&mut bus, 0, 0x40, enable | 1);
    assert_eq!(cfg_read(&mut bus, 0, 0x40), 1);
    w(&mut bus, FBI_INIT1, 0xAAAA);
    assert_eq!(r(&bus, FBI_INIT1), 0xAAAA);
}

#[test]
fn the_dac_answers_glides_detection() {
    let mut bus = bus(Board::Max);
    cfg_write(&mut bus, 0, 0x40, 1);
    // Write DAC register 7 (the command register), then read register 5
    // through dacData bit 11 and fbiInit2 with initEnable bit 2.
    for (command, id) in [(0x01, 0x55), (0x07, 0x71), (0x0B, 0x79), (0x00, 0xFF)] {
        w(&mut bus, DAC_DATA, 7 << 8 | command);
        w(&mut bus, DAC_DATA, 1 << 11 | 5 << 8);
        cfg_write(&mut bus, 0, 0x40, 1 | 4);
        assert_eq!(r(&bus, FBI_INIT2) & 0xFF, id, "command {:02X}", command);
        cfg_write(&mut bus, 0, 0x40, 1);
    }
    // Other registers read back what was written.
    w(&mut bus, DAC_DATA, 2 << 8 | 0x5A);
    w(&mut bus, DAC_DATA, 1 << 11 | 2 << 8);
    cfg_write(&mut bus, 0, 0x40, 1 | 4);
    assert_eq!(r(&bus, FBI_INIT2), 0x5A);
}

#[test]
fn access_sizes_as_the_card_takes_them() {
    let mut bus = bus(Board::Standard);
    cfg_write(&mut bus, 0, 0x40, 1);
    w(&mut bus, FBI_INIT3, 0);
    // 16-bit halves of a register: the lower half written alone, the
    // upper half shifted in (as DOSBox-X, which ignores the mask for
    // registers).
    bus.write_16(BASE + COLOR0 as usize, 0x1234);
    assert_eq!(r(&bus, COLOR0), 0x1234);
    bus.write_16(BASE + COLOR0 as usize + 2, 0x5678);
    assert_eq!(r(&bus, COLOR0), 0x5678_0000);
    assert_eq!(bus.read_16(BASE + COLOR0 as usize + 2), 0x5678);
    // A dword at an odd word: the halves of two registers.
    w(&mut bus, COLOR1, 0);
    bus.write_32(BASE + COLOR0 as usize + 2, 0xAAAA_BBBB);
    assert_eq!(r(&bus, COLOR0), 0xBBBB_0000);
    assert_eq!(r(&bus, COLOR1), 0x0000_AAAA);
    // Bytes are ignored and read FFh.
    bus.write_8(BASE + COLOR1 as usize, 0x11);
    assert_eq!(r(&bus, COLOR1), 0x0000_AAAA);
    assert_eq!(bus.read_8(BASE + COLOR1 as usize), 0xFF);
    // Registers that can't be read.
    assert_eq!(r(&bus, TRIANGLE_CMD), 0xFFFF_FFFF);
}

#[test]
fn fstp_stores_a_float_register_whole() {
    let mut rig = Rig::new();
    rig.cpu.bus.configure_voodoo(Some(Board::Standard));
    rig.write32(DATA, 2.5f32.to_bits());
    // fstartR takes 2.5 as 12.12 fixed point: 2800h.
    rig.run(|a| {
        a.fld(dword_ptr(DATA))?;
        a.mov(ebx, (BASE + 0x0A0) as u32)?;
        a.fstp(dword_ptr(ebx))?;
        // And back: an FLD of the colour register reads it whole.
        a.mov(ebx, (BASE + COLOR0 as usize) as u32)?;
        a.mov(dword_ptr(ebx), 0x4020_0000u32)?;
        a.fld(dword_ptr(ebx))?;
        a.fstp(dword_ptr(DATA + 4))?;
        a.hlt()
    });
    assert_eq!(rig.cpu.bus.voodoo.as_ref().unwrap().fbi.startr, 0x2800);
    assert_eq!(f32::from_bits(rig.read32(DATA + 4)), 2.5);
}

#[test]
fn a_reset_gives_the_monitor_back() {
    let mut bus = bus(Board::Max);
    init(&mut bus);
    assert!(bus.voodoo_output());
    assert_eq!(rust_dos::video::frame_size(&bus), (640, 480));
    bus.reset_voodoo();
    assert!(!bus.voodoo_output());
    assert_eq!(cfg_read(&mut bus, 0, 0x10), 0xD000_0008);
    assert_eq!(cfg_read(&mut bus, 0, 0x40), 0);
}

#[test]
fn fastfill_clears_colour_and_depth() {
    let mut bus = bus(Board::Standard);
    init(&mut bus);
    w(&mut bus, COLOR1, 0x00FF_0000);
    w(&mut bus, ZA_COLOR, 0x1234);
    w(&mut bus, CLIP_LEFT_RIGHT, 10 << 16 | 20);
    w(&mut bus, CLIP_LOW_Y_HIGH_Y, 5 << 16 | 8);
    w(&mut bus, FBZ_MODE, RGB_WRITE | AUX_WRITE);
    w(&mut bus, FASTFILL_CMD, 0);
    assert_eq!(pixel(&bus, 0, 10, 5), 0xF800);
    assert_eq!(pixel(&bus, 0, 19, 7), 0xF800);
    assert_eq!(pixel(&bus, 0, 20, 7), 0, "the right edge is exclusive");
    assert_eq!(pixel(&bus, 0, 10, 8), 0, "and the bottom");
    assert_eq!(pixel(&bus, 0, 9, 5), 0);
    assert_eq!(depth(&bus, 15, 6), 0x1234);
    assert_eq!(depth(&bus, 15, 8), 0);
}

#[test]
fn a_flat_triangle_covers_pixel_centres() {
    let mut bus = bus(Board::Standard);
    init(&mut bus);
    w(&mut bus, FBZ_MODE, RGB_WRITE);
    flat(&mut bus, 0, 255, 0, 255);
    // The lower left half of an 8x8 square at (100, 100).
    triangle(&mut bus, [(100.0, 100.0), (100.0, 108.0), (108.0, 108.0)]);
    assert_eq!(pixel(&bus, 0, 100, 100), 0, "row 0 covers no pixel centre");
    assert_eq!(pixel(&bus, 0, 100, 101), 0x07E0);
    assert_eq!(pixel(&bus, 0, 101, 101), 0);
    assert_eq!(pixel(&bus, 0, 106, 107), 0x07E0);
    assert_eq!(pixel(&bus, 0, 107, 107), 0);
    assert_eq!(pixel(&bus, 0, 100, 108), 0, "the bottom edge is exclusive");
    assert_eq!(r(&bus, 0x25C) & 0xFFFF, 0xFFFF, "no triangle counter on a Voodoo Graphics");
    // 1 + 2 + ... + 7 pixels.
    assert_eq!(r(&bus, FBI_PIXELS_OUT), 28);
    w(&mut bus, NOP_CMD, 1);
    assert_eq!(r(&bus, FBI_PIXELS_OUT), 0);
}

#[test]
fn gouraud_shading_follows_the_gradients() {
    let mut bus = bus(Board::Standard);
    init(&mut bus);
    w(&mut bus, FBZ_MODE, RGB_WRITE);
    flat(&mut bus, 0, 0, 0, 0);
    // Red from 0 at x = 0 rising 8 a pixel.
    w(&mut bus, D_R_DX, 8 << 12);
    triangle(&mut bus, [(0.0, 0.0), (32.0, 0.0), (0.0, 32.0)]);
    // Pixel 10 of row 1 is 10 pixels right of vertex A: red 80 -> 10.
    assert_eq!(pixel(&bus, 0, 10, 1) >> 11, 80 >> 3);
    assert_eq!(pixel(&bus, 0, 20, 1) >> 11, 160 >> 3);
}

#[test]
fn the_depth_test_keeps_the_nearer_triangle() {
    let mut bus = bus(Board::Standard);
    init(&mut bus);
    // Clear depth to FFFFh, then draw with depth test "less than".
    w(&mut bus, ZA_COLOR, 0xFFFF);
    w(&mut bus, FBZ_MODE, AUX_WRITE);
    w(&mut bus, FASTFILL_CMD, 0);
    w(&mut bus, FBZ_MODE, RGB_WRITE | AUX_WRITE | DEPTH_TEST | 1 << 5);
    let square = |bus: &mut Bus| {
        triangle(bus, [(0.0, 0.0), (16.0, 0.0), (0.0, 16.0)]);
        triangle(bus, [(16.0, 0.0), (16.0, 16.0), (0.0, 16.0)]);
    };
    flat(&mut bus, 255, 0, 0, 0);
    w(&mut bus, START_Z, 0x1000 << 12);
    square(&mut bus);
    flat(&mut bus, 0, 0, 255, 0);
    w(&mut bus, START_Z, 0x2000 << 12);
    square(&mut bus);
    assert_eq!(pixel(&bus, 0, 5, 5), 0xF800, "the farther blue one is hidden");
    assert_eq!(depth(&bus, 5, 5), 0x1000);
    flat(&mut bus, 0, 0, 255, 0);
    w(&mut bus, START_Z, 0x0800 << 12);
    square(&mut bus);
    assert_eq!(pixel(&bus, 0, 5, 5), 0x001F);
    assert_eq!(depth(&bus, 5, 5), 0x0800);
}

#[test]
fn alpha_blending_mixes_with_the_frame_buffer() {
    let mut bus = bus(Board::Standard);
    init(&mut bus);
    // White background.
    w(&mut bus, COLOR1, 0x00FF_FFFF);
    w(&mut bus, FBZ_MODE, RGB_WRITE);
    w(&mut bus, FASTFILL_CMD, 0);
    // Black at alpha 128: src * alpha + dst * (1 - alpha).
    w(&mut bus, ALPHA_MODE, 1 << 4 | 1 << 8 | 5 << 12);
    flat(&mut bus, 0, 0, 0, 128);
    triangle(&mut bus, [(0.0, 0.0), (16.0, 0.0), (0.0, 16.0)]);
    // 248 * (256 - 128) >> 8 = 124 -> 15 of 31.
    assert_eq!(pixel(&bus, 0, 1, 1) >> 11, 124 >> 3);
}

#[test]
fn fog_from_the_table_blends_towards_the_fog_colour() {
    let mut bus = bus(Board::Standard);
    init(&mut bus);
    w(&mut bus, FBZ_MODE, RGB_WRITE);
    w(&mut bus, FOG_COLOR, 0x00FF_FFFF);
    // Every table entry: blend 255, no delta.
    for i in 0..32 {
        w(&mut bus, FOG_TABLE + 4 * i, 0xFF00_FF00);
    }
    w(&mut bus, FOG_MODE, 1);
    flat(&mut bus, 0, 0, 0, 0);
    triangle(&mut bus, [(0.0, 0.0), (16.0, 0.0), (0.0, 16.0)]);
    // Black fogged to (255 + 1) * 255 >> 8 = 255: white.
    assert_eq!(pixel(&bus, 0, 1, 1), 0xFFFF);
}

#[test]
fn the_chroma_key_and_alpha_test_drop_pixels() {
    let mut bus = bus(Board::Standard);
    init(&mut bus);
    w(&mut bus, FBZ_MODE, RGB_WRITE | CHROMA);
    w(&mut bus, CHROMA_KEY, 0x0000_FF00);
    flat(&mut bus, 0, 255, 0, 0);
    triangle(&mut bus, [(0.0, 0.0), (16.0, 0.0), (0.0, 16.0)]);
    assert_eq!(pixel(&bus, 0, 1, 1), 0, "the key colour is not drawn");
    flat(&mut bus, 255, 0, 0, 0);
    triangle(&mut bus, [(0.0, 0.0), (16.0, 0.0), (0.0, 16.0)]);
    assert_eq!(pixel(&bus, 0, 1, 1), 0xF800);
    // Alpha test "greater than 100".
    w(&mut bus, FBZ_MODE, RGB_WRITE);
    w(&mut bus, ALPHA_MODE, 1 | 4 << 1 | 100 << 24);
    flat(&mut bus, 0, 0, 255, 50);
    triangle(&mut bus, [(0.0, 0.0), (16.0, 0.0), (0.0, 16.0)]);
    assert_eq!(pixel(&bus, 0, 1, 1), 0xF800, "alpha 50 fails");
    flat(&mut bus, 0, 0, 255, 150);
    triangle(&mut bus, [(0.0, 0.0), (16.0, 0.0), (0.0, 16.0)]);
    assert_eq!(pixel(&bus, 0, 1, 1), 0x001F);
}

#[test]
fn pattern_stipple_draws_the_pattern() {
    // On the workers too: the pattern is the stipple register's.
    let mut bus = bus(Board::Standard);
    init(&mut bus);
    let pattern = 0xAAAA_5555u32;
    w(&mut bus, 0x140, pattern);
    w(&mut bus, FBZ_MODE, RGB_WRITE | 1 << 2 | 1 << 12);
    flat(&mut bus, 255, 255, 255, 0);
    triangle(&mut bus, [(0.0, 0.0), (16.0, 0.0), (0.0, 16.0)]);
    triangle(&mut bus, [(16.0, 0.0), (16.0, 16.0), (0.0, 16.0)]);
    for y in 0..16usize {
        for x in 0..16usize {
            let index = (y & 3) << 3 | (!x & 7);
            let want = if pattern >> index & 1 != 0 { 0xFFFF } else { 0 };
            assert_eq!(pixel(&bus, 0, x, y), want, "pixel ({}, {})", x, y);
        }
    }
}

#[test]
fn clipping_and_the_y_origin() {
    let mut bus = bus(Board::Standard);
    init(&mut bus);
    w(&mut bus, CLIP_LEFT_RIGHT, 2 << 16 | 6);
    w(&mut bus, CLIP_LOW_Y_HIGH_Y, 0 << 16 | 480);
    w(&mut bus, FBZ_MODE, RGB_WRITE | CLIPPING);
    flat(&mut bus, 255, 255, 255, 0);
    triangle(&mut bus, [(0.0, 0.0), (16.0, 0.0), (0.0, 16.0)]);
    assert_eq!(pixel(&bus, 0, 1, 1), 0);
    assert_eq!(pixel(&bus, 0, 2, 1), 0xFFFF);
    assert_eq!(pixel(&bus, 0, 4, 1), 0xFFFF);
    // The right clip edge stops a pixel short, as the original does.
    assert_eq!(pixel(&bus, 0, 5, 1), 0);
    // With the Y origin at the bottom (fbiInit3's 479), row 0 is the
    // bottom row.
    cfg_write(&mut bus, 0, 0x40, 1);
    w(&mut bus, FBI_INIT3, 479 << 22);
    w(&mut bus, FBZ_MODE, RGB_WRITE | Y_ORIGIN);
    flat(&mut bus, 0, 0, 255, 0);
    triangle(&mut bus, [(0.0, 0.0), (16.0, 0.0), (0.0, 16.0)]);
    assert_eq!(pixel(&bus, 0, 1, 479), 0x001F);
    assert_eq!(pixel(&bus, 0, 1, 478), 0x001F);
}

/// An 8x8 16-bit texture at level 5 of TMU 0, texel (x, y) = colour
/// `texel(x, y)`, drawn over the 8x8 square at (0, 0) one texel a pixel.
fn textured_square(bus: &mut Bus, format: u32, bilinear: bool, texel: impl Fn(u32, u32) -> u32) {
    // tLOD: level 5 only (lodmin = lodmax = 5 << 2).
    w(bus, T_LOD, 20 | 20 << 6);
    w(bus, TEX_BASE_ADDR, 0);
    let filter = if bilinear { 3 << 1 } else { 0 };
    // Decal: zero the other, add the local colour and alpha.
    w(bus, TEXTURE_MODE, format << 8 | filter | 1 << 12 | 1 << 18 | 1 << 21 | 1 << 27);
    for y in 0..8u32 {
        for x in (0..8u32).step_by(2) {
            let offset = 0x80_0000 | 5 << 17 | y << 9 | x << 1;
            let value = texel(x, y) | texel(x + 1, y) << 16;
            bus.write_32(BASE + offset as usize, value);
        }
    }
    // S and T in level-0 texels (14.18): 32 of them a level-5 texel,
    // starting at the first texel's centre.
    w(bus, START_S, 16 << 18);
    w(bus, START_T, 16 << 18);
    w(bus, D_S_DX, 32 << 18);
    w(bus, D_T_DX, 0);
    w(bus, D_S_DY, 0);
    w(bus, D_T_DY, 32 << 18);
    // The texture's colour.
    w(bus, FBZ_COLOR_PATH, 1 | 1 << 27);
    w(bus, FBZ_MODE, RGB_WRITE);
    triangle(bus, [(0.0, 0.0), (8.0, 0.0), (0.0, 8.0)]);
    triangle(bus, [(8.0, 0.0), (8.0, 8.0), (0.0, 8.0)]);
}

#[test]
fn point_sampled_textures() {
    let mut bus = bus(Board::Standard);
    init(&mut bus);
    // RGB565 texels: red rising with X, blue with Y.
    textured_square(&mut bus, 10, false, |x, y| (x * 4) << 11 | y * 4);
    for (x, y) in [(0, 0), (3, 2), (7, 7)] {
        assert_eq!(pixel(&bus, 0, x, y), ((x as u16 * 4) << 11 | y as u16 * 4), "texel {},{}", x, y);
    }
}

#[test]
fn bilinear_textures_blend_neighbours() {
    let mut bus = bus(Board::Standard);
    init(&mut bus);
    // A checkerboard of black and white RGB565 texels, filtered: at the
    // texel centres the texels themselves, one pixel = one texel.
    textured_square(&mut bus, 10, true, |x, y| if (x + y) % 2 == 0 { 0xFFFF } else { 0 });
    assert_eq!(pixel(&bus, 0, 2, 2), 0xFFFF);
    assert_eq!(pixel(&bus, 0, 3, 2), 0);
}

#[test]
fn the_tmus_hand_out_their_configuration() {
    // trexInit1 bit 18: TMU 0's "texels" are its configuration, which is
    // how Glide counts texture units. Two TMUs: D1h.
    for (board, config) in [(Board::Max, 0xD1u32), (Board::Standard, 0x11)] {
        let mut bus = bus(board);
        init(&mut bus);
        w(&mut bus, TREX_INIT1, 1 << 18);
        w(&mut bus, T_LOD, 0);
        w(&mut bus, FBZ_COLOR_PATH, 1 | 1 << 27);
        w(&mut bus, FBZ_MODE, RGB_WRITE);
        triangle(&mut bus, [(0.0, 0.0), (8.0, 0.0), (0.0, 8.0)]);
        // The low byte as blue.
        assert_eq!(pixel(&bus, 0, 1, 1) & 0x1F, (config & 0xFF) as u16 >> 3, "{:?}", board);
        w(&mut bus, TREX_INIT1, 0);
    }
}

#[test]
fn frame_buffer_writes_and_reads() {
    let mut bus = bus(Board::Standard);
    init(&mut bus);
    let lfb = |x: usize, y: usize| BASE + 0x40_0000 + y * 2048 + x * 2;
    // Format 0: two RGB565 pixels a dword, into the front buffer.
    w(&mut bus, LFB_MODE, 0);
    bus.write_32(lfb(4, 3), 0x07E0_F800);
    assert_eq!((pixel(&bus, 0, 4, 3), pixel(&bus, 0, 5, 3)), (0xF800, 0x07E0));
    // A 16-bit write: one pixel.
    bus.write_16(lfb(7, 3), 0x001F);
    assert_eq!(pixel(&bus, 0, 7, 3), 0x001F);
    assert_eq!(bus.read_32(lfb(4, 3)), 0x07E0_F800);
    // Format 1 (x555) and format 5 (ARGB 8888).
    w(&mut bus, LFB_MODE, 1);
    bus.write_32(lfb(10, 3), 0x7C00);
    assert_eq!(pixel(&bus, 0, 10, 3), 0xF800);
    w(&mut bus, LFB_MODE, 5);
    bus.write_32(BASE + 0x40_0000 + 3 * 4096 + 12 * 4, 0xFF00_00FF);
    assert_eq!(pixel(&bus, 0, 12, 3), 0x001F);
    // Format 12: depth in the upper half, RGB565 in the lower.
    w(&mut bus, LFB_MODE, 12);
    bus.write_32(BASE + 0x40_0000 + 3 * 4096 + 14 * 4, 0x4321_07E0);
    assert_eq!((pixel(&bus, 0, 14, 3), depth(&bus, 14, 3)), (0x07E0, 0x4321));
    // Format 15: two depths.
    w(&mut bus, LFB_MODE, 15);
    bus.write_32(lfb(16, 3), 0x2222_1111);
    assert_eq!((depth(&bus, 16, 3), depth(&bus, 17, 3)), (0x1111, 0x2222));
    // Reading the auxiliary buffer (read buffer 2).
    w(&mut bus, LFB_MODE, 2 << 6);
    assert_eq!(bus.read_32(lfb(16, 3)), 0x2222_1111);
    // Writes into the back buffer, and the Y origin flip for writes.
    w(&mut bus, LFB_MODE, 1 << 4);
    bus.write_32(lfb(0, 0), 0xF800);
    assert_eq!(pixel(&bus, 1, 0, 0), 0xF800);
}

#[test]
fn frame_buffer_writes_through_the_pixel_pipeline() {
    let mut bus = bus(Board::Standard);
    init(&mut bus);
    // Depth test "less than" against a depth buffer of 1000h, with the
    // write's own depth (format 12) deciding.
    w(&mut bus, ZA_COLOR, 0x1000);
    w(&mut bus, FBZ_MODE, AUX_WRITE);
    w(&mut bus, FASTFILL_CMD, 0);
    w(&mut bus, FBZ_MODE, RGB_WRITE | DEPTH_TEST | 1 << 5);
    w(&mut bus, LFB_MODE, 12 | 1 << 8);
    let lfb32 = |x: usize, y: usize| BASE + 0x40_0000 + y * 4096 + x * 4;
    bus.write_32(lfb32(1, 1), 0x2000_F800);
    assert_eq!(pixel(&bus, 0, 1, 1), 0, "farther: dropped");
    bus.write_32(lfb32(1, 1), 0x0800_F800);
    assert_eq!(pixel(&bus, 0, 1, 1), 0xF800);
}

#[test]
fn the_front_buffer_is_what_shows() {
    let mut bus = bus(Board::Standard);
    let before = rust_dos::video::frame_size(&bus);
    init(&mut bus);
    assert_eq!(rust_dos::video::frame_size(&bus), (640, 480));
    // Red into the back buffer, then swap.
    w(&mut bus, COLOR1, 0x00FF_0000);
    w(&mut bus, FBZ_MODE, RGB_WRITE | DRAW_BACK);
    w(&mut bus, FASTFILL_CMD, 0);
    w(&mut bus, SWAPBUFFER_CMD, 0);
    bus.sync_display();
    let mut frame = rust_dos::video::Frame::new(640, 480);
    rust_dos::video::render_screen(&mut frame, &bus);
    assert_eq!(&frame.rgb[0..3], &[0xFF, 0, 0]);
    // The output off: the VGA's picture again.
    cfg_write(&mut bus, 0, 0x40, 1);
    w(&mut bus, FBI_INIT0, 0);
    assert_eq!(rust_dos::video::frame_size(&bus), before);
}

#[test]
fn the_gamma_table_brightens_the_picture() {
    let mut bus = bus(Board::Standard);
    init(&mut bus);
    w(&mut bus, COLOR1, 0x0080_8080);
    w(&mut bus, FBZ_MODE, RGB_WRITE);
    w(&mut bus, FASTFILL_CMD, 0);
    // A table that doubles everything (saturating): entry i = 16 i.
    cfg_write(&mut bus, 0, 0x40, 1);
    w(&mut bus, FBI_INIT1, 10 << 4);
    for i in 0..33u32 {
        let v = (i * 16).min(255);
        w(&mut bus, CLUT_DATA, i << 24 | v << 16 | v << 8 | v);
    }
    bus.sync_display();
    let mut frame = rust_dos::video::Frame::new(640, 480);
    rust_dos::video::render_screen(&mut frame, &bus);
    assert!(frame.rgb[0] >= 0xF0, "red {}", frame.rgb[0]);
}

#[test]
fn swaps_that_wait_for_the_retrace() {
    let mut bus = bus(Board::Standard);
    init(&mut bus);
    let front = |bus: &Bus| r(bus, STATUS) >> 10 & 3;
    let pending = |bus: &Bus| r(bus, STATUS) >> 28 & 7;
    // Swap interval 1, waiting for the retrace.
    w(&mut bus, SWAPBUFFER_CMD, 1 << 1 | 1);
    assert_eq!(front(&bus), 1, "the buffers swap");
    assert_eq!(pending(&bus), 1, "the swap waits for the retrace");
    assert_ne!(r(&bus, STATUS) & 0x380, 0, "busy meanwhile");
    w(&mut bus, SWAPBUFFER_CMD, 1 << 1 | 1);
    assert_eq!(pending(&bus), 2);
    // A frame at 60 Hz later, the first is done; two frames, both.
    wait_ms(&mut bus, 17.0);
    assert_eq!(pending(&bus), 1);
    wait_ms(&mut bus, 17.0);
    assert_eq!(pending(&bus), 0);
    assert_eq!(r(&bus, STATUS) & 0x380, 0, "idle");
    // Glide's idle wait: several non-busy reads in a row.
    assert!((0..4).all(|_| r(&bus, STATUS) & 0x200 == 0));
    // The retrace bit comes and goes.
    let mut seen = [false; 2];
    for _ in 0..200 {
        seen[(r(&bus, STATUS) >> 6 & 1) as usize] = true;
        wait_ms(&mut bus, 0.1);
    }
    assert_eq!(seen, [true, true]);
}

#[test]
fn its_swaps_are_the_frames_drawn_while_it_shows() {
    let mut bus = bus(Board::Standard);
    init(&mut bus);
    let start = bus.frames_drawn;
    for _ in 0..10 {
        w(&mut bus, SWAPBUFFER_CMD, 1 << 1 | 1);
        wait_ms(&mut bus, 17.0);
        bus.sync_display();
    }
    wait_ms(&mut bus, 17.0);
    bus.sync_display();
    assert_eq!(bus.frames_drawn - start, 10);
    // With the output off, the VGA's picture shows again, which is a frame,
    // and the swaps don't count.
    cfg_write(&mut bus, 0, 0x40, 1);
    w(&mut bus, FBI_INIT0, 0);
    for _ in 0..3 {
        w(&mut bus, SWAPBUFFER_CMD, 0);
        wait_ms(&mut bus, 17.0);
        bus.sync_display();
    }
    assert_eq!(bus.frames_drawn - start, 11);
}

#[test]
fn a_full_fifo_waits_for_the_swap() {
    let mut bus = bus(Board::Standard);
    init(&mut bus);
    // No memory FIFO: 64 entries, the swap command's own and 63 more.
    w(&mut bus, SWAPBUFFER_CMD, 1);
    let start = bus.clock.now_ns();
    for _ in 0..63 {
        w(&mut bus, COLOR0, 0);
    }
    assert!(bus.clock.now_ns() - start < 1_000_000, "the FIFO holds 64");
    w(&mut bus, COLOR0, 0);
    let waited = bus.clock.now_ns() - start;
    assert!(waited > 1_000_000, "the next write waits for the retrace ({} ns)", waited);
    assert_eq!(r(&bus, STATUS) >> 28 & 7, 0);
}

#[test]
fn a_saved_card_comes_back() {
    let mut bus = bus(Board::Standard);
    init(&mut bus);
    w(&mut bus, COLOR1, 0x0000_FF00);
    w(&mut bus, FBZ_MODE, RGB_WRITE);
    w(&mut bus, FASTFILL_CMD, 0);
    w(&mut bus, SWAPBUFFER_CMD, 1 << 1 | 1);
    let mut cpu = rust_dos::cpu::Cpu::new(PathBuf::from("."));
    cpu.bus = bus;
    let state = rust_dos::savestate::machine::save(&cpu);
    // Change everything, then load.
    w(&mut cpu.bus, COLOR1, 0);
    w(&mut cpu.bus, FASTFILL_CMD, 0);
    cpu.bus.reset_voodoo();
    rust_dos::savestate::machine::load(&mut cpu, &state).unwrap();
    let bus = &cpu.bus;
    assert!(bus.voodoo_output());
    assert_eq!(pixel(bus, 0, 3, 3), 0x07E0);
    assert_eq!(r(bus, STATUS) >> 28 & 7, 1, "the pending swap too");
    assert_eq!(r(bus, STATUS) >> 10 & 3, 1);
}

#[test]
fn states_need_the_same_card() {
    let mut cpu = rust_dos::cpu::Cpu::new(PathBuf::from("."));
    let without = rust_dos::savestate::machine::save(&cpu);
    cpu.bus.configure_voodoo(Some(Board::Standard));
    assert!(rust_dos::savestate::machine::load(&mut cpu, &without).is_err());
    let with = rust_dos::savestate::machine::save(&cpu);
    cpu.bus.configure_voodoo(Some(Board::Max));
    assert!(rust_dos::savestate::machine::load(&mut cpu, &with).is_err());
    cpu.bus.configure_voodoo(None);
    assert!(rust_dos::savestate::machine::load(&mut cpu, &with).is_err());
    assert!(rust_dos::savestate::machine::load(&mut cpu, &without).is_ok());
}

#[test]
fn the_debugger_reads_the_window() {
    let mut bus = bus(Board::Standard);
    init(&mut bus);
    w(&mut bus, COLOR0, 0x1122_3344);
    assert_eq!(bus.peek_8(BASE + COLOR0 as usize), 0x44);
    assert_eq!(bus.peek_8(BASE + COLOR0 as usize + 3), 0x11);
    let status = bus.voodoo_status().unwrap();
    assert_eq!(status["output"], true);
    assert_eq!(status["width"], 640);
}

/// Draw a bit of everything: a fastfill, textured, blended, fogged and
/// depth-tested triangles in many strips, frame buffer writes between
/// them, and a stippled triangle.
fn scene(bus: &mut Bus) {
    init(bus);
    w(bus, ZA_COLOR, 0xFFFF);
    w(bus, COLOR1, 0x0020_4060);
    w(bus, FBZ_MODE, RGB_WRITE | AUX_WRITE | 1 << 8);
    w(bus, FASTFILL_CMD, 0);
    textured_square(bus, 10, true, |x, y| (x * 7 + y * 3) << 11 | (y * 5) << 5 | x * 3);
    for i in 0..40 {
        let y = (i * 11 % 400) as f32;
        let x = (i * 37 % 500) as f32;
        w(bus, FBZ_COLOR_PATH, if i % 3 == 0 { 1 | 1 << 27 } else { 0 });
        w(bus, FBZ_MODE, RGB_WRITE | AUX_WRITE | DEPTH_TEST | 3 << 5 | 1 << 8);
        w(bus, ALPHA_MODE, if i % 2 == 0 { 1 << 4 | 1 << 8 | 5 << 12 } else { 0 });
        w(bus, FOG_MODE, (i % 4 == 0) as u32);
        flat(bus, i * 6 % 256, i * 13 % 256, 255 - i * 5 % 256, 90 + i);
        w(bus, D_R_DX, 3 << 11);
        w(bus, D_R_DY, 2 << 11);
        w(bus, START_Z, (0x4000 + i * 100) << 12);
        w(bus, D_S_DX, (i % 5 + 1) << 17);
        triangle(bus, [(x, y), (x + 120.0, y + 17.0), (x + 30.0, y + 90.0)]);
        if i % 7 == 0 {
            w(bus, LFB_MODE, 0);
            bus.write_32(BASE + 0x40_0000 + (y as usize + 5) * 2048 + (x as usize + 6) * 2, 0x1234_5678);
        }
    }
    for i in 0..32 {
        w(bus, FOG_TABLE + 4 * i, (i * 8) << 24 | (i * 8 + 4) << 8);
    }
    w(bus, FOG_MODE, 1);
    w(bus, FBZ_COLOR_PATH, 0);
    w(bus, 0x140, 0xAAAA_5555);
    w(bus, FBZ_MODE, RGB_WRITE | 1 << 2);
    triangle(bus, [(300.0, 300.0), (400.0, 310.0), (320.0, 420.0)]);
    w(bus, FBZ_MODE, RGB_WRITE | 1 << 2 | 1 << 12);
    triangle(bus, [(100.0, 300.0), (200.0, 310.0), (120.0, 420.0)]);
}

#[test]
fn any_number_of_workers_draws_the_same_picture() {
    let pictures: Vec<(Vec<u8>, u32)> = [0, 1, 4]
        .into_iter()
        .map(|workers| {
            let mut bus = bus(Board::Max);
            bus.voodoo = Some(rust_dos::voodoo::Voodoo::with_workers(Board::Max, workers));
            scene(&mut bus);
            let v = bus.voodoo.as_ref().unwrap();
            (v.frame_buffer().to_bytes(), r(&bus, FBI_PIXELS_OUT))
        })
        .collect();
    assert!(pictures[0].1 > 10_000, "the scene draws ({} pixels)", pictures[0].1);
    for (i, p) in pictures.iter().enumerate().skip(1) {
        assert_eq!(p.1, pictures[0].1, "pixel counts, {} workers", [0, 1, 4][i]);
        assert!(p.0 == pictures[0].0, "frame buffer differs with {} workers", [0, 1, 4][i]);
    }
}

#[test]
fn frame_buffer_reads_see_what_was_just_drawn() {
    let mut bus = bus(Board::Standard);
    bus.voodoo = Some(rust_dos::voodoo::Voodoo::with_workers(Board::Standard, 3));
    init(&mut bus);
    w(&mut bus, FBZ_MODE, RGB_WRITE);
    flat(&mut bus, 255, 0, 0, 0);
    triangle(&mut bus, [(0.0, 0.0), (64.0, 0.0), (0.0, 64.0)]);
    w(&mut bus, LFB_MODE, 0);
    assert_eq!(bus.read_32(BASE + 0x40_0000 + 10 * 2048 + 4 * 2), 0xF800_F800);
}

/// Rasterizer speed: 2000 textured, bilinear, fogged triangles a frame.
/// `cargo test --release --test voodoo_tests benchmark -- --ignored
/// --nocapture`.
#[test]
#[ignore]
fn benchmark_rasterizer() {
    for workers in [0, 1, 2, 4] {
        let mut bus = bus(Board::Standard);
        bus.voodoo = Some(rust_dos::voodoo::Voodoo::with_workers(Board::Standard, workers));
        init(&mut bus);
        textured_square(&mut bus, 10, true, |x, y| (x * 7 + y * 3) << 11 | x * 3);
        w(&mut bus, FOG_MODE, 1);
        w(&mut bus, FBZ_MODE, RGB_WRITE | AUX_WRITE | DEPTH_TEST | 7 << 5 | 1 << 8);
        w(&mut bus, D_S_DX, 1 << 17);
        w(&mut bus, D_T_DY, 1 << 17);
        let start = std::time::Instant::now();
        let frames = 10;
        for frame in 0..frames {
            for i in 0..2000u32 {
                let x = ((i * 53 + frame * 7) % 560) as f32;
                let y = ((i * 29) % 420) as f32;
                triangle(&mut bus, [(x, y), (x + 60.0, y + 8.0), (x + 12.0, y + 50.0)]);
            }
            w(&mut bus, SWAPBUFFER_CMD, 0);
        }
        let _ = bus.voodoo.as_ref().unwrap().frame_buffer();
        let per_frame = start.elapsed() / frames;
        println!("{} workers: {:?} a frame ({} pixels)", workers, per_frame, r(&bus, FBI_PIXELS_OUT) / frames);
    }
}

/// DOSBox-X's card and this one, given the same writes, hold the same
/// frame buffer at every swap: a trace recorded by a DOSBox-X built with
/// a `VOODOO_TRACE` hook (13-byte records: kind, then three
/// little-endian dwords; `P` a PCI configuration byte written, `W` a
/// dword written with its mask, `R` a dword read, `S` a swap with the
/// FNV-1a hash of the frame buffer after it) replayed here. Local only:
/// `RUST_DOS_VOODOO_TRACE=path cargo test --release --test voodoo_tests
/// dosbox_x -- --ignored --nocapture`.
#[test]
#[ignore]
fn replays_a_dosbox_x_trace() {
    let Ok(path) = std::env::var("RUST_DOS_VOODOO_TRACE") else { return };
    let data = std::fs::read(path).unwrap();
    let mut v = rust_dos::voodoo::Voodoo::with_workers(Board::Max, 4);
    let now = rust_dos::voodoo::Now::default();
    let fnv = |bytes: &[u8]| bytes.iter().fold(2166136261u32, |h, &b| (h ^ b as u32).wrapping_mul(16777619));
    let (mut swaps, mut bad_swaps, mut reads, mut bad_reads) = (0, 0, 0, 0);
    for (i, record) in data.chunks_exact(13).enumerate() {
        let word = |n: usize| u32::from_le_bytes(record[1 + 4 * n..5 + 4 * n].try_into().unwrap());
        match record[0] {
            b'P' => {
                v.config_write(word(0) as u8, word(1) as u8);
            }
            b'W' => {
                v.write(word(0) << 2, word(1), word(2), now);
            }
            // Frame buffer reads; the registers' depend on time.
            b'R' if word(0) & (0xC0_0000 / 4) != 0 => {
                reads += 1;
                let got = v.read(word(0) << 2, now);
                if got != word(1) {
                    if bad_reads < 5 {
                        println!("record {}: LFB read {:06X} = {:08X}, DOSBox-X {:08X}", i, word(0) << 2, got, word(1));
                    }
                    bad_reads += 1;
                }
            }
            b'S' => {
                swaps += 1;
                let hash = fnv(&v.frame_buffer().to_bytes());
                if hash != word(1) {
                    if bad_swaps < 5 {
                        println!("record {}: swap {} differs", i, swaps);
                    }
                    bad_swaps += 1;
                }
            }
            _ => {}
        }
    }
    println!("{} swaps, {} differ; {} LFB reads, {} differ", swaps, bad_swaps, reads, bad_reads);
    assert_eq!((bad_swaps, bad_reads), (0, 0));
}

/// The OpenGL renderer's recording since it last took it.
fn take_mirror(bus: &mut Bus) -> rust_dos::voodoo::mirror::Frame {
    bus.voodoo.as_mut().unwrap().take_mirror().expect("recording")
}

#[test]
fn the_opengl_renderer_gets_what_is_drawn() {
    use rust_dos::voodoo::mirror::{Command, Fill, Pixels};
    let mut bus = bus(Board::Standard);
    init(&mut bus);
    bus.voodoo.as_mut().unwrap().set_mirror(true);
    // First the buffers as they are.
    let frame = take_mirror(&mut bus);
    assert!(frame.output);
    assert_eq!((frame.width, frame.height, frame.front), (640, 480, Some(0)));
    let [Command::Resync(snapshot)] = &frame.commands[..] else { panic!("{:?}", frame.commands) };
    assert_eq!((snapshot.layout.width, snapshot.layout.height), (640, 480));
    assert_eq!(snapshot.layout.color, [0, 150 * 0x1000 / 2]);
    assert_eq!(snapshot.layout.aux, Some(2 * 150 * 0x1000 / 2));
    assert_eq!(snapshot.color[0].len(), 640 * 480);

    // A fastfill, two Gouraud triangles and a frame buffer write.
    w(&mut bus, COLOR1, 0x0012_3456);
    w(&mut bus, ZA_COLOR, 0x1234);
    w(&mut bus, FBZ_MODE, RGB_WRITE | AUX_WRITE | CLIPPING);
    w(&mut bus, FASTFILL_CMD, 0);
    flat(&mut bus, 100, 0, 0, 255);
    w(&mut bus, D_R_DX, 2 << 12);
    w(&mut bus, FBZ_COLOR_PATH, 0);
    triangle(&mut bus, [(10.0, 10.0), (50.0, 10.0), (10.0, 50.0)]);
    triangle(&mut bus, [(50.0, 10.0), (50.0, 50.0), (10.0, 50.0)]);
    w(&mut bus, LFB_MODE, 0);
    bus.write_32(BASE + 0x40_0000 + 5 * 2048 + 4 * 2, 0xF800_07E0);
    let frame = take_mirror(&mut bus);
    let [Command::Fill(fill), Command::Draw(draw), Command::Pixels(pixels)] = &frame.commands[..] else {
        panic!("{:?}", frame.commands)
    };
    assert_eq!(
        *fill,
        Fill { dest: 0, rect: [0, 640, 0, 480], color: Some(0x12_3456), aux: Some(0x1234), alpha_planes: false }
    );
    assert_eq!(draw.vertices.len(), 6, "the same state: one draw");
    assert_eq!(draw.state.clip, Some([0, 640, 0, 480]));
    // The card samples pixel 10 at its left edge, where red is 100: at
    // the vertex, half a pixel left of the pixel's centre, it is 99.
    let a = draw.vertices[0];
    assert_eq!(a.pos, [10.0, 10.0]);
    assert_eq!(a.color, [99.0, 0.0, 0.0, 255.0]);
    assert_eq!(draw.vertices[1].color[0], 179.0);
    assert_eq!(*pixels, Pixels { dest: Some(0), x: 4, y: 5, values: vec![0x07E0, 0xF800] });
    // The software drew the same.
    assert_eq!(pixel(&bus, 0, 4, 5), 0x07E0);
}

#[test]
fn frame_buffer_writes_are_recorded_a_row_a_buffer() {
    use rust_dos::voodoo::mirror::{Command, Pixels};
    let mut bus = bus(Board::Standard);
    init(&mut bus);
    bus.voodoo.as_mut().unwrap().set_mirror(true);
    take_mirror(&mut bus);
    w(&mut bus, FBZ_MODE, RGB_WRITE);
    // Format 12: depth and a 5-6-5 colour, a pixel a dword.
    w(&mut bus, LFB_MODE, 12);
    for y in 7..9u32 {
        for x in 3..7u32 {
            bus.write_32(BASE + 0x40_0000 + (y * 4096 + x * 4) as usize, (0x1000 + x) << 16 | (0x20 + y));
        }
    }
    // Format 15: two depths a dword, no colour.
    w(&mut bus, LFB_MODE, 15);
    bus.write_32(BASE + 0x40_0000 + 20 * 2048 + 8 * 2, 0x0002_0001);
    let frame = take_mirror(&mut bus);
    let pixels: Vec<&Pixels> = frame
        .commands
        .iter()
        .map(|c| match c {
            Command::Pixels(p) => p,
            other => panic!("{:?}", other),
        })
        .collect();
    let colour = |y| Pixels { dest: Some(0), x: 3, y, values: vec![0x20 + y as u16; 4] };
    let depth = |y| Pixels { dest: None, x: 3, y, values: (3..7).map(|x| 0x1000 + x).collect() };
    assert_eq!(
        pixels,
        [&colour(7), &depth(7), &colour(8), &depth(8), &Pixels { dest: None, x: 8, y: 20, values: vec![1, 2] }]
    );
    // A drawing after them breaks the rows: what comes then goes on top.
    w(&mut bus, LFB_MODE, 0);
    bus.write_32(BASE + 0x40_0000 + 30 * 2048, 0x0001_0001);
    w(&mut bus, FASTFILL_CMD, 0);
    bus.write_32(BASE + 0x40_0000 + 30 * 2048 + 4, 0x0001_0001);
    let frame = take_mirror(&mut bus);
    assert!(
        matches!(&frame.commands[..], [Command::Pixels(_), Command::Fill(_), Command::Pixels(p)] if p.x == 2),
        "{:?}",
        frame.commands
    );
}

#[test]
fn lfb_writes_keep_their_order_among_queued_triangles() {
    let pictures: Vec<Vec<u8>> = [0, 1, 3]
        .into_iter()
        .map(|workers| {
            let mut bus = bus(Board::Max);
            bus.voodoo = Some(rust_dos::voodoo::Voodoo::with_workers(Board::Max, workers));
            init(&mut bus);
            w(&mut bus, FBZ_MODE, RGB_WRITE | AUX_WRITE);
            for i in 0..40u32 {
                flat(&mut bus, i * 6, 255 - i * 6, i * 3, 0);
                w(&mut bus, START_Z, (i * 100) << 12);
                triangle(&mut bus, [(0.0, 0.0), (300.0, (i * 10) as f32), (10.0, 200.0 + i as f32)]);
                // Frame buffer writes over the triangles, colour and depth.
                w(&mut bus, LFB_MODE, if i % 2 == 0 { 0 } else { 12 });
                for y in i * 4..i * 4 + 30 {
                    for x in (i..i + 120).step_by(2) {
                        let at = if i % 2 == 0 { y * 2048 + x * 2 } else { y * 4096 + x * 4 };
                        bus.write_32(BASE + 0x40_0000 + at as usize, i << 24 | y << 8 | x);
                    }
                }
            }
            bus.voodoo.as_ref().unwrap().frame_buffer().to_bytes()
        })
        .collect();
    assert!(pictures[0] == pictures[1], "1 worker");
    assert!(pictures[0] == pictures[2], "3 workers");
}

#[test]
fn the_opengl_renderer_gets_the_textures_as_the_card_reads_them() {
    use rust_dos::voodoo::mirror::Command;
    let textures = |frame: &rust_dos::voodoo::mirror::Frame| -> Vec<rust_dos::voodoo::mirror::Texture> {
        frame.commands.iter().filter_map(|c| if let Command::Texture(t) = c { Some((**t).clone()) } else { None }).collect()
    };
    let mut bus = bus(Board::Standard);
    init(&mut bus);
    bus.voodoo.as_mut().unwrap().set_mirror(true);
    take_mirror(&mut bus);
    textured_square(&mut bus, 10, false, |x, y| (x * 4) << 11 | y * 4);
    let frame = take_mirror(&mut bus);
    let decoded = textures(&frame);
    assert_eq!(decoded.len(), 1, "one texture for both triangles");
    let level = &decoded[0].levels[0];
    assert_eq!((level.width, level.height), (8, 8), "level 5 of a 256x256 texture");
    // Texel (3, 2): red 12 and blue 8 of 5-6-5, widened to 8 bits.
    assert_eq!(level.argb[2 * 8 + 3], 0xFF63_0042);
    let draw = frame.commands.iter().find_map(|c| if let Command::Draw(d) = c { Some(d) } else { None }).unwrap();
    let unit = draw.state.tmu[0].unwrap();
    assert_eq!((unit.texture, unit.first_level, unit.width, unit.height), (decoded[0].id, 5, 256, 256));
    assert!(draw.state.tmu[1].is_none());

    // Drawn with again: nothing new to decode.
    triangle(&mut bus, [(0.0, 0.0), (8.0, 0.0), (0.0, 8.0)]);
    assert!(textures(&take_mirror(&mut bus)).is_empty());
    // Its texels written: the same texture again, as it is now.
    bus.write_32(BASE + 0x80_0000 + (5 << 17), 0xFFFF_FFFF);
    triangle(&mut bus, [(0.0, 0.0), (8.0, 0.0), (0.0, 8.0)]);
    let again = textures(&take_mirror(&mut bus));
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].id, decoded[0].id);
    assert_eq!(again[0].levels[0].argb[0], 0xFFFF_FFFF);
}

#[test]
fn a_loaded_state_gives_the_opengl_renderer_the_buffers_again() {
    use rust_dos::voodoo::mirror::Command;
    let mut bus = bus(Board::Standard);
    init(&mut bus);
    w(&mut bus, COLOR1, 0x0000_FF00);
    w(&mut bus, FBZ_MODE, RGB_WRITE);
    w(&mut bus, FASTFILL_CMD, 0);
    let mut cpu = rust_dos::cpu::Cpu::new(PathBuf::from("."));
    cpu.bus = bus;
    let state = rust_dos::savestate::machine::save(&cpu);
    cpu.bus.voodoo.as_mut().unwrap().set_mirror(true);
    take_mirror(&mut cpu.bus);
    w(&mut cpu.bus, COLOR1, 0);
    w(&mut cpu.bus, FASTFILL_CMD, 0);
    rust_dos::savestate::machine::load(&mut cpu, &state).unwrap();
    let frame = take_mirror(&mut cpu.bus);
    let [Command::Resync(snapshot)] = &frame.commands[..] else { panic!("{:?}", frame.commands) };
    assert_eq!(snapshot.color[0][3 * 640 + 3], 0x07E0);
}
