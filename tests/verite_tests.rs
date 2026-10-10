//! The Rendition Vérité (`machine=svga_verite`): finding it as
//! Rendition's library does, starting its processor, and the microcode's
//! commands through the FIFO and by DMA.

use rust_dos::cpu::Cpu;
use rust_dos::verite::{IO_BASE, regs};
use rust_dos::video::adapter::{Adapter, VideoSetup};
use rust_dos::video::bios;
use std::path::PathBuf;

fn machine() -> Cpu {
    let mut cpu = Cpu::new(PathBuf::from("."));
    bios::install(&mut cpu.bus, VideoSetup { adapter: Adapter::Verite, ..Default::default() });
    cpu
}

fn int10(cpu: &mut Cpu, ax: u16) {
    cpu.set_ax(ax);
    rust_dos::bus::verite_bios(cpu);
}

/// The processor started, its version read, as the library does.
fn started() -> Cpu {
    let mut cpu = machine();
    int10(&mut cpu, 0x1582);
    cpu.set_cx(0x1000);
    cpu.set_dx(0);
    int10(&mut cpu, 0x1583);
    assert_eq!(cpu.bus.io_read_wide(IO_BASE + regs::FIFOOUTVALID as u16, 1), 1);
    cpu.bus.io_read_wide(IO_BASE, 4);
    cpu
}

fn fifo(cpu: &mut Cpu, words: &[u32]) {
    for &w in words {
        cpu.bus.io_write_wide(IO_BASE, w, 4);
    }
}

fn pixel(cpu: &Cpu, base: u32, x: u32, y: u32) -> u16 {
    let at = (base + y * 1280 + x * 2) as usize;
    u16::from_le_bytes([cpu.bus.vbe.vram[at], cpu.bus.vbe.vram[at + 1]])
}

#[test]
fn the_card_is_on_the_pci_bus_with_its_memory_and_ports() {
    let mut cpu = machine();
    assert!(cpu.bus.pci_present());
    cpu.bus.io_write_wide(0xCF8, 0x8000_0800, 4);
    assert_eq!(cpu.bus.io_read_wide(0xCFC, 4), 0x0001_1163, "Rendition V1000 at device 1");
    cpu.bus.io_write_wide(0xCF8, 0x8000_0814, 4);
    assert_eq!(cpu.bus.io_read_wide(0xCFC, 4), IO_BASE as u32 | 1, "BAR1: the ports");
}

#[test]
fn the_bios_describes_the_board() {
    let mut cpu = machine();
    int10(&mut cpu, 0x158D);
    assert_eq!(cpu.ax(), 0x0015);
    let at = ((cpu.dx() as usize) << 4) + cpu.cx() as usize;
    let data: Vec<u8> = (0..8).map(|i| cpu.bus.read_8(at + i)).collect();
    assert_eq!(data[0], 0x90, "vital product data");
    assert_eq!(&data[3..5], b"ZC");
    assert_eq!(data[6], (IO_BASE >> 8) as u8, "the ports' base");
    assert_eq!(data[7], 0xE0, "the memory's");
}

#[test]
fn the_microcode_reports_its_version_when_started() {
    let mut cpu = started();
    assert_eq!(cpu.bus.io_read_wide(IO_BASE + regs::FIFOOUTVALID as u16, 1), 0);
    fifo(&mut cpu, &[0x0000_0008]);
    assert_eq!(cpu.bus.io_read_wide(IO_BASE + regs::FIFOOUTVALID as u16, 1), 1, "a sync answers");
}

#[test]
fn a_gouraud_fan_is_drawn_into_the_destination() {
    let mut cpu = started();
    let base = 0x11008;
    let kxy = |k: u32, x: u32, y: u32| [k, x << 16, y << 16];
    let mut words = vec![0x1004, base, 0x143B, 0x51, 0x1231, 0, 0x001C_001B, 3];
    for v in [kxy(0xFF0000, 0, 0), kxy(0xFF0000, 32, 0), kxy(0xFF0000, 0, 32)] {
        words.extend_from_slice(&v);
    }
    fifo(&mut cpu, &words);
    assert_eq!(pixel(&cpu, base, 4, 4), 0xF800, "red inside");
    assert_eq!(pixel(&cpu, base, 30, 30), 0, "nothing past the long edge");
}

#[test]
fn display_shows_a_buffer() {
    let mut cpu = started();
    fifo(&mut cpu, &[0x0004, 0xA7008]);
    assert_eq!(cpu.bus.vbe.start, 0xA7008);
}

#[test]
fn dma_lists_feed_the_fifo_with_swapped_bytes() {
    let mut cpu = started();
    // A clear of 4 lines of 8 bytes at 1000h with 0x11223344, its words
    // byte-swapped in memory (mode 1).
    let words: [u32; 6] = [0x0000_000D, 0x1000, 1280, 8, 4, 0x1122_3344];
    for (i, w) in words.iter().enumerate() {
        cpu.bus.write_32(0x20000 + 4 * i, w.swap_bytes());
    }
    cpu.bus.write_32(0x30000, 0x20000);
    cpu.bus.write_32(0x30004, (words.len() as u32 * 4) | 1);
    cpu.bus.write_32(0x30008, 0);
    cpu.bus.io_write_wide(IO_BASE + regs::MODE as u16, 0x08, 1);
    cpu.bus.io_write_wide(IO_BASE + regs::DMACMDPTR as u16, 0x30000, 4);
    assert_eq!(&cpu.bus.vbe.vram[0x1000 + 3 * 1280..][..4], &0x1122_3344u32.to_le_bytes());
}

#[test]
fn a_shadow_darkens_by_its_alpha() {
    let mut cpu = started();
    let base = 0x11008;
    // White everywhere, then a half-alpha shape with the source factor 0
    // and the destination's the source alpha.
    fifo(&mut cpu, &[0x000D, base, 1280, 1280, 32, 0xFFFF_FFFF]);
    let mut words = vec![0x1004, base, 0x1231, 0, 0x2055, 0x0080_0000, 0x89C6, 1, 0x1241, 6, 0x1442, 2, 0x0001_001B, 3];
    for (x, y) in [(0, 0), (32, 0), (0, 32)] {
        words.extend_from_slice(&[x << 16, y << 16]);
    }
    fifo(&mut cpu, &words);
    let p = pixel(&cpu, base, 4, 4);
    assert_eq!(p >> 11, 16, "half the red of white: {:04X}", p);
}

#[test]
fn state_round_trips() {
    let mut cpu = started();
    fifo(&mut cpu, &[0x1004, 0x1234]);
    let mut w = rust_dos::savestate::Writer::new();
    rust_dos::savestate::State::save(&cpu.bus.verite, &mut w);
    let mut other = machine();
    let mut r = rust_dos::savestate::Reader::new(&w.buf);
    rust_dos::savestate::State::load(&mut other.bus.verite, &mut r).unwrap();
    assert!(other.bus.verite.running);
    assert_eq!(other.bus.verite.draw.dst_base, 0x1234);
}

#[test]
fn the_z_buffer_hides_what_is_behind() {
    let mut cpu = started();
    let (base, z) = (0x11008, 0x20_0000);
    // The Z buffer installed and cleared to the farthest; less passes.
    fifo(&mut cpu, &[0x000D, z, 1280, 1280, 32, 0xFFFF_FFFF]);
    fifo(&mut cpu, &[0x1004, base, 0x143B, 0x51, 0x1231, 0, 0x1010, z, 0x183C, 0x51, 0x1643, 1, 0x1844, 1]);
    // A near red triangle and a far blue one over it, as KXYZ.
    let triangle = |k: u32, depth: u32| {
        let mut words = vec![0x0012_0018];
        for (x, y) in [(0, 0), (32, 0), (0, 32)] {
            words.extend_from_slice(&[k, x << 16, y << 16, depth << 16]);
        }
        words
    };
    fifo(&mut cpu, &triangle(0xFF0000, 100));
    fifo(&mut cpu, &triangle(0x0000FF, 200));
    assert_eq!(pixel(&cpu, base, 4, 4), 0xF800, "the far one is hidden");
    assert_eq!(pixel(&cpu, z, 4, 4), 100, "the near one's depth");
    fifo(&mut cpu, &triangle(0x0000FF, 50));
    assert_eq!(pixel(&cpu, base, 4, 4), 0x001F, "a nearer one is drawn");
}

#[test]
fn four_bit_textures_look_up_their_palette() {
    let mut cpu = started();
    let (base, texture) = (0x11008, 0x20_0000);
    // Texel 5 everywhere; entry 5 is green.
    fifo(&mut cpu, &[0x000D, texture, 128, 128, 16, 0x5555_5555]);
    let mut palette = [0u32; 8];
    palette[2] = 0x07E0; // entries 4 and 5: the low half is the odd one
    let mut words = vec![0x1004, base, 0x143B, 0x51, 0x1231, 1, 0x4000, texture, 0x00, 0x000F_000F, 0x10000, 0x10000];
    words.extend_from_slice(&[0x1030, 8, 0x7020]);
    words.extend_from_slice(&palette);
    words.extend_from_slice(&[0x0002_0018]);
    for (x, y) in [(0, 0), (16, 0), (0, 16)] {
        words.extend_from_slice(&[x << 16, y << 16, x << 16, y << 16]);
    }
    fifo(&mut cpu, &words);
    assert_eq!(pixel(&cpu, base, 2, 2), 0x07E0);
}

/// A table or texture of 16-bit entries put into memory with MEM_WRITE,
/// two to a word with the first in the high half (vQuake's order).
fn mem_write(cpu: &mut Cpu, at: u32, entries: &[u16]) {
    let mut words = vec![0x0009, at, 2 * entries.len() as u32];
    words.extend(entries.chunks(2).map(|p| (p[0] as u32) << 16 | p.get(1).copied().unwrap_or(0) as u32));
    fifo(cpu, &words);
}

#[test]
fn mem_write_puts_the_high_half_first() {
    let mut cpu = started();
    mem_write(&mut cpu, 0x20_0000, &[0x1111, 0x2222, 0x3333]);
    assert_eq!(pixel(&cpu, 0x20_0000, 0, 0), 0x1111);
    assert_eq!(pixel(&cpu, 0x20_0000, 1, 0), 0x2222);
    assert_eq!(pixel(&cpu, 0x20_0000, 2, 0), 0x3333);
}

#[test]
fn lookup_turns_indices_into_colours() {
    let mut cpu = started();
    let (base, table) = (0x11008, 0x20_0000);
    mem_write(&mut cpu, table, &[0x0000, 0xF800, 0x07E0, 0x001F]);
    // The table as a 256x1 texture, then 3x2 indices at 4, 5, each line
    // padded to a word.
    fifo(&mut cpu, &[0x1004, base, 0x143B, 0x51, 0x4000, table, 0x01, 0x0000_00FF, 0x0100_0000, 0x10000]);
    fifo(&mut cpu, &[0x002A, 4 << 16 | 5, 3 << 16 | 2, 0xAA03_0201, 0xAA02_0103]);
    assert_eq!(pixel(&cpu, base, 4, 5), 0xF800);
    assert_eq!(pixel(&cpu, base, 5, 5), 0x07E0);
    assert_eq!(pixel(&cpu, base, 6, 5), 0x001F);
    assert_eq!(pixel(&cpu, base, 4, 6), 0x001F, "the second line, after the padding");
    assert_eq!(pixel(&cpu, base, 7, 5), 0, "nothing past the width");
}

#[test]
fn spans_are_textured_with_perspective_and_keep_their_depth() {
    let mut cpu = started();
    let (base, texture, z) = (0x11008, 0x20_0000, 0x30_0000);
    // Red then blue, 2x1, U times 2; depths written.
    mem_write(&mut cpu, texture, &[0xF800, 0x001F]);
    fifo(&mut cpu, &[0x1004, base, 0x143B, 0x51, 0x1231, 1, 0x1010, z, 0x183C, 0x51, 0x1643, 0, 0x1844, 1]);
    fifo(&mut cpu, &[0x4000, texture, 0x20, 0x0000_0001, 0x20000, 0x10000]);
    // S/Z a quarter more a pixel, 1/Z 1: U 0, 0.5, 1, 1.5 over 4 pixels.
    fifo(&mut cpu, &[0x0027, 0x4000, 0, 0, 0, 10 << 16 | 3, 4, 0, 0, 0x10000, 100 << 16, 0x8000_0000]);
    let row: Vec<u16> = (10..14).map(|x| pixel(&cpu, base, x, 3)).collect();
    assert_eq!(row, [0xF800, 0xF800, 0x001F, 0x001F]);
    assert_eq!(pixel(&cpu, base, 14, 3), 0, "4 pixels");
    assert_eq!(pixel(&cpu, z, 11, 3), 100);
}

#[test]
fn particles_are_z_buffered_squares() {
    let mut cpu = started();
    let (base, z) = (0x11008, 0x30_0000);
    // Quake's depths: nearer is greater, and mode 6 draws what is as near
    // or nearer.
    fifo(&mut cpu, &[0x000D, z, 1280, 1280, 32, 0x0032_0032]);
    fifo(&mut cpu, &[0x1004, base, 0x143B, 0x51, 0x1010, z, 0x183C, 0x51, 0x1643, 6, 0x1844, 0]);
    fifo(&mut cpu, &[0x0025, 2, 2 << 16 | 2, 2 << 16 | 2, 60 << 16, 0x00FF_0000, 8 << 16 | 2, 2 << 16 | 2, 40 << 16, 0x00FF_0000]);
    assert_eq!(pixel(&cpu, base, 3, 3), 0xF800, "nearer than 50");
    assert_eq!(pixel(&cpu, base, 4, 3), 0, "2 wide");
    assert_eq!(pixel(&cpu, base, 8, 2), 0, "farther than 50");
}

/// A port of the card's, as a 32-bit I/O address.
fn port(reg: u8) -> u16 {
    IO_BASE + reg as u16
}

/// An instruction forced through the held RISC, as Rendition's Windows
/// driver and xf86-video-rendition's `risc_forcestep` do.
fn force(cpu: &mut Cpu, instruction: u32) {
    cpu.bus.io_write_wide(port(regs::STATEINDEX), 0x80, 1);
    cpu.bus.io_write_wide(port(regs::STATEDATA), instruction, 4);
    cpu.bus.io_write_wide(port(regs::DEBUGREG), (regs::HOLDRISC | regs::STEPRISC) as u32, 1);
}

/// The 2D microcode started as the Windows driver starts it: the RISC held,
/// sent to the loader with a jump, let go, and the loader's four words.
fn windows_2d() -> Cpu {
    let mut cpu = machine();
    cpu.bus.io_write_wide(port(regs::DEBUGREG), regs::HOLDRISC as u32, 1);
    force(&mut cpu, 0x6C00_0000 | 0x800 >> 2);
    force(&mut cpu, 0);
    cpu.bus.io_write_wide(port(regs::DEBUGREG), 0, 1);
    fifo(&mut cpu, &[0, 0xC00, 0, 0x1000]);
    cpu
}

#[test]
fn the_risc_reaches_the_program_the_windows_driver_starts() {
    let mut cpu = machine();
    cpu.bus.io_write_wide(port(regs::DEBUGREG), regs::HOLDRISC as u32, 1);
    // The jump, then the nop in its delay slot.
    force(&mut cpu, 0x6C00_0000 | 0x800 >> 2);
    force(&mut cpu, 0);
    assert_eq!(cpu.bus.io_read_wide(port(regs::DEBUGREG), 1) as u8 & regs::STEPRISC, 0, "stepped");
    cpu.bus.io_write_wide(port(regs::STATEINDEX), 0x81, 1);
    assert_eq!(cpu.bus.io_read_wide(port(regs::STATEDATA), 4), 0x800, "the PC");
}

#[test]
fn the_crtc_shows_the_windows_drivers_mode() {
    let mut cpu = machine();
    // What the driver programs for 800x600 in 16 bits: 565, video on.
    for (reg, value) in [
        (regs::CRTCHORZ, 0x008F_1463),
        (regs::CRTCVERT, 0x0006_B257),
        (regs::CRTCOFFSET, 0x200),
        (regs::FRAMEBASEA, 0x20000),
        (regs::CRTCCTL, 0x1F04),
    ] {
        cpu.bus.io_write_wide(port(reg), value, 4);
    }
    let mode = cpu.bus.vbe.mode.expect("a mode");
    assert_eq!((mode.width, mode.height, mode.bpp), (800, 600, 16));
    assert_eq!(cpu.bus.vbe.pitch, 2048, "1536 bytes fetched, then the offset");
    assert_eq!(cpu.bus.vbe.start, 0x20000);
    cpu.bus.io_write_wide(port(regs::CRTCCTL), 0, 4);
    assert!(cpu.bus.vbe.mode.is_none(), "video off: the VGA's again");
}

#[test]
fn the_2d_microcode_fills_and_answers() {
    let mut cpu = windows_2d();
    // An 800x600 surface of 16 bits at 128 KB, 2048 bytes a line.
    fifo(&mut cpu, &[0x20, 800 << 16 | 600, 16 << 16 | 4, 0x20000, 2048, 0x5300]);
    fifo(&mut cpu, &[0x30, 0x7BEF_7BEF, 10 << 16 | 20, 3 << 16 | 2]);
    let at = |x: u32, y: u32| {
        let i = (0x20000 + y * 2048 + x * 2) as usize;
        u16::from_le_bytes([cpu.bus.vbe.vram[i], cpu.bus.vbe.vram[i + 1]])
    };
    assert_eq!((at(10, 20), at(12, 21)), (0x7BEF, 0x7BEF));
    assert_eq!((at(13, 20), at(10, 22)), (0, 0), "3x2");
    fifo(&mut cpu, &[8]);
    assert_eq!(cpu.bus.io_read_wide(port(regs::FIFOOUTVALID), 1), 1, "synced");
}
