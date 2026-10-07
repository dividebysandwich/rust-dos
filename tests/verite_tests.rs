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
