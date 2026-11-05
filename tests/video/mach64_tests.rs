//! The ATI mach64 GT (`machine=svga_ati_3drage`, `svga_ati_3drage2`):
//! finding it on the PCI bus and through its BIOS as ATI's drivers do,
//! its registers through the ports and the memory, and the accelerator
//! CRTC showing VESA modes.

use iced_x86::Register;
use rust_dos::cpu::Cpu;
use rust_dos::interrupts::int10;
use rust_dos::mach64::{IO_BASE, regs};
use rust_dos::video::adapter::{Adapter, VideoSetup};
use rust_dos::video::vbe::LFB_BASE;
use rust_dos::video::{self, Frame, VideoMode, bios};
use std::path::PathBuf;

/// A buffer for the BIOS's answers, at 3000:0000.
const BUF: u32 = 0x30000;

fn machine_with(adapter: Adapter) -> Cpu {
    let mut cpu = Cpu::new(PathBuf::from("."));
    bios::install(
        &mut cpu.bus,
        VideoSetup {
            adapter,
            ..Default::default()
        },
    );
    int10::set_mode(&mut cpu, 0x03);
    cpu
}

fn machine() -> Cpu {
    machine_with(Adapter::AtiRage)
}

/// INT 10h with AX, BX, CX, DX; returns AX.
fn int10(cpu: &mut Cpu, ax: u16, bx: u16, cx: u16, dx: u16) -> u16 {
    cpu.set_ax(ax);
    cpu.set_bx(bx);
    cpu.set_cx(cx);
    cpu.set_dx(dx);
    int10::handle(cpu);
    cpu.ax()
}

/// A register through the block I/O ports.
fn reg(cpu: &mut Cpu, reg: usize) -> u32 {
    cpu.bus.io_read_wide(IO_BASE + 4 * reg as u16, 4)
}

fn set_reg(cpu: &mut Cpu, reg: usize, value: u32) {
    cpu.bus.io_write_wide(IO_BASE + 4 * reg as u16, value, 4);
}

/// Where block 0's register `reg` is in the aperture.
fn mmio(reg: usize) -> usize {
    LFB_BASE + 0x7F_FC00 + 4 * reg
}

#[test]
fn the_card_is_on_the_pci_bus_with_its_aperture_and_ports() {
    let mut cpu = machine();
    assert!(cpu.bus.pci_present());
    cpu.bus.io_write_wide(0xCF8, 0x8000_0800, 4);
    assert_eq!(
        cpu.bus.io_read_wide(0xCFC, 4),
        0x4754_1002,
        "ATI mach64 GT at device 1"
    );
    cpu.bus.io_write_wide(0xCF8, 0x8000_0808, 4);
    assert_eq!(
        cpu.bus.io_read_wide(0xCFC, 4),
        0x0300_0040,
        "a VGA, revision 40h"
    );
    cpu.bus.io_write_wide(0xCF8, 0x8000_0810, 4);
    assert_eq!(cpu.bus.io_read_wide(0xCFC, 4), LFB_BASE as u32);
    cpu.bus.io_write_wide(0xCF8, 0x8000_0814, 4);
    assert_eq!(cpu.bus.io_read_wide(0xCFC, 4), IO_BASE as u32 | 1);
    assert_eq!(reg(&mut cpu, regs::CONFIG_CHIP_ID), 0x4000_4754);
}

#[test]
fn bar0_moves_the_aperture() {
    let mut cpu = machine();
    cpu.bus.io_write_wide(0xCF8, 0x8000_0810, 4);
    cpu.bus.io_write_wide(0xCFC, 0xD100_0000, 4);
    assert_eq!(cpu.bus.io_read_wide(0xCFC, 4), 0xD100_0000, "16 MB aligned");
    assert_eq!(cpu.bus.vbe.lfb_base, Some(0xD100_0000));
    assert_eq!(
        cpu.bus.read_32(0xD17F_FC00 + 4 * regs::CONFIG_CHIP_ID),
        0x4000_4754
    );
    // The aperture's location in CONFIG_CNTL, in 4 MB units.
    assert_eq!(reg(&mut cpu, regs::CONFIG_CNTL) >> 4 & 0x3FF, 0xD10 >> 2);
}

#[test]
fn the_rage_ii_has_its_own_ids() {
    let mut cpu = machine_with(Adapter::AtiRage2);
    assert_eq!(reg(&mut cpu, regs::CONFIG_CHIP_ID), 0x9A00_4754);
    int10(&mut cpu, 0xA006, 0, 0, 0);
    assert_eq!(cpu.get_reg8(Register::AH), 0);
    assert_eq!(cpu.dx(), 0x479A, "GT-B");
}

#[test]
fn the_rom_has_ati_s_signature_and_tables() {
    let cpu = machine();
    let rom: Vec<u8> = (0..0x100).map(|i| cpu.bus.read_8(0xC0000 + i)).collect();
    assert_eq!(&rom[0x30..0x3A], b" 761295520");
    assert_eq!(rom[0x43], 0x40);
    let table = u16::from_le_bytes([rom[0x48], rom[0x49]]) as usize;
    let clocks = u16::from_le_bytes([rom[table + 0x10], rom[table + 0x11]]) as usize;
    assert_eq!(rom[clocks], 4, "an internal PLL");
    assert_eq!(u16::from_le_bytes([rom[clocks + 8], rom[clocks + 9]]), 1432);
}

#[test]
fn the_query_structure_lists_the_modes() {
    let mut cpu = machine();
    int10(&mut cpu, 0xA008, 0, 1, 0);
    let size = cpu.cx() as u32;
    assert_eq!(size, 0x20 + 8 * 0x26);
    cpu.set_reg8(Register::CL, 1);
    int10(&mut cpu, 0xA009, 0, 1, 0x3000);
    assert_eq!(cpu.get_reg8(Register::AH), 0);
    let q: Vec<u8> = (0..size)
        .map(|i| cpu.bus.read_8((BUF + i) as usize))
        .collect();
    assert_eq!(u16::from_le_bytes([q[0], q[1]]) as u32, size);
    assert_eq!(q[0x03], 8);
    assert_eq!(q[0x06], 0x26);
    assert_eq!(u16::from_le_bytes([q[0x08], q[0x09]]), 0x4700);
    assert_eq!(
        u16::from_le_bytes([q[0x10], q[0x11]]),
        (LFB_BASE >> 20) as u16
    );
    assert_eq!(q[0x12], 0x42);
    assert_eq!(q[0x15], 1, "block I/O");
    assert_eq!(u16::from_le_bytes([q[0x18], q[0x19]]), IO_BASE);
    // The first mode: 640x480, resolution code 12h, 25.18 MHz.
    let t = &q[0x20..0x46];
    assert_eq!(
        (
            u16::from_le_bytes([t[0], t[1]]),
            u16::from_le_bytes([t[2], t[3]]),
            t[5]
        ),
        (640, 480, 0x12)
    );
    assert_eq!(u16::from_le_bytes([t[0x1A], t[0x1B]]), 2518);
}

#[test]
fn a_vesa_mode_is_shown_by_the_accelerator_crtc() {
    let mut cpu = machine();
    assert_eq!(int10(&mut cpu, 0x4F02, 0x4111, 0, 0), 0x004F);
    assert!(cpu.bus.mach64.native);
    assert_eq!(cpu.bus.video_mode, VideoMode::Vesa);
    let mode = cpu.bus.vbe.mode.unwrap();
    assert_eq!(
        (mode.number, mode.width, mode.height, mode.bpp),
        (0x111, 640, 480, 16)
    );
    assert_eq!(cpu.bus.vbe.pitch, 1280);
    // CRTC_GEN_CNTL: 16 bits a pixel, the accelerator's display.
    let gen_cntl = cpu.bus.read_32(mmio(regs::CRTC_GEN_CNTL));
    assert_eq!(gen_cntl >> 8 & 7, 4);
    assert_eq!(gen_cntl & 0x0300_0000, 0x0300_0000);
    assert!((cpu.bus.vga.timing().hz() - 60.0).abs() < 1.0);
    // AX=4F03h still knows the mode.
    assert_eq!(int10(&mut cpu, 0x4F03, 0, 0, 0), 0x004F);
    assert_eq!(cpu.bx() & 0x1FF, 0x111);
    // Back to text.
    int10::set_mode(&mut cpu, 0x03);
    assert!(!cpu.bus.mach64.native);
    assert_ne!(cpu.bus.video_mode, VideoMode::Vesa);
}

#[test]
fn the_display_start_and_pitch_go_to_the_crtc() {
    let mut cpu = machine();
    int10(&mut cpu, 0x4F02, 0x4101, 0, 0);
    assert_eq!(int10(&mut cpu, 0x4F06, 0x0000, 1024, 0), 0x004F);
    assert_eq!(reg(&mut cpu, regs::CRTC_OFF_PITCH) >> 22, 1024 / 8);
    assert_eq!(cpu.bus.vbe.pitch, 1024);
    assert_eq!(int10(&mut cpu, 0x4F07, 0x0000, 0, 10), 0x004F);
    assert_eq!(
        reg(&mut cpu, regs::CRTC_OFF_PITCH) & 0xF_FFFF,
        10 * 1024 / 8
    );
    assert_eq!(cpu.bus.vbe.start, 10 * 1024);
}

#[test]
fn the_window_banks_in_32_kb_pages() {
    let mut cpu = machine();
    int10(&mut cpu, 0x4F02, 0x0101, 0, 0);
    assert_eq!(int10(&mut cpu, 0x4F05, 0, 0, 3), 0x004F);
    cpu.bus.write_8(0xA0000, 0x11);
    cpu.bus.write_8(0xA8000, 0x22);
    assert_eq!(cpu.bus.vbe.vram[3 * 0x10000], 0x11);
    assert_eq!(cpu.bus.vbe.vram[3 * 0x10000 + 0x8000], 0x22);
    // A page of its own for the upper half.
    set_reg(&mut cpu, regs::MEM_VGA_WP_SEL, 0x0009_0000);
    cpu.bus.write_8(0xA8001, 0x33);
    assert_eq!(cpu.bus.vbe.vram[9 * 0x8000 + 1], 0x33);
}

#[test]
fn the_registers_are_in_the_vga_window_outside_text_modes() {
    let mut cpu = machine();
    // In text mode B800:F800 is text memory.
    cpu.bus.write_8(0xBFCE0, 0x5A);
    assert_eq!(cpu.bus.read_8(0xBFCE0), 0x5A);
    int10::set_mode(&mut cpu, 0x12);
    assert_eq!(
        cpu.bus.read_32(0xBFC00 + 4 * regs::CONFIG_CHIP_ID),
        0x4000_4754
    );
    assert_eq!(cpu.bus.read_32(0xBFC00 + 4 * regs::FIFO_STAT), 0);
}

#[test]
fn the_sparse_ports_when_the_card_decodes_them() {
    let mut cpu = machine();
    assert_eq!(
        cpu.bus.io_read_wide(0x6EEC, 4),
        0xFFFF_FFFF,
        "block I/O only"
    );
    cpu.bus.io_write_wide(0xCF8, 0x8000_0840, 4);
    cpu.bus.io_write_wide(0xCFC, 0, 4);
    assert_eq!(cpu.bus.io_read_wide(0x6EEC, 4), 0x4000_4754);
    cpu.bus.io_write_wide(0x02EC, 0x1234, 2);
    assert_eq!(cpu.bus.mach64.reg(regs::CRTC_H_TOTAL_DISP), 0x1234 & 0x1FF);
}

#[test]
fn the_dac_through_its_register() {
    let mut cpu = machine();
    // Index 5 in lane 0, then red, green and blue in lane 1.
    set_reg(&mut cpu, regs::DAC_REGS, 0x05);
    for c in [10, 20, 30] {
        cpu.bus
            .io_write_wide(IO_BASE + 4 * regs::DAC_REGS as u16 + 1, c, 1);
    }
    assert_eq!(&cpu.bus.vga.palette[15..18], &[10, 20, 30]);
    // DAC_CNTL bit 8: the 8-bit DAC.
    set_reg(&mut cpu, regs::DAC_CNTL, 0x100);
    assert!(cpu.bus.vga.dac_8bit);
}

#[test]
fn the_bios_loads_and_selects_the_accelerator_crtc() {
    let mut cpu = machine();
    // 800x600 (6Ah) in 8 bits a pixel, pitch the width, then shown.
    int10(&mut cpu, 0xA000, 0, 0x6A82, 0);
    assert_eq!(cpu.get_reg8(Register::AH), 0);
    assert!(!cpu.bus.mach64.native, "not shown yet");
    int10(&mut cpu, 0xA001, 0, 0x0001, 0);
    let screen = cpu.bus.mach64.screen().unwrap();
    assert_eq!(
        (screen.width, screen.height, screen.bpp, screen.pitch),
        (800, 600, 8, 800)
    );
    assert!(cpu.bus.mach64.native);
    assert!(
        (screen.timing.hz() - 60.3).abs() < 1.0,
        "{}",
        screen.timing.hz()
    );
}

#[test]
fn the_cursor_shows_over_the_picture() {
    let mut cpu = machine();
    int10(&mut cpu, 0x4F02, 0x4112, 0, 0);
    // A map of colour 1 everywhere but the first two pixels (transparent
    // and inverted), at 1 MB.
    let base = 0x10_0000;
    for i in 0..1024 {
        cpu.bus.vbe.vram[base + i] = 0b0101_0101;
    }
    cpu.bus.vbe.vram[base] = 0b0101_1110;
    set_reg(&mut cpu, regs::CUR_OFFSET, (base / 8) as u32);
    set_reg(&mut cpu, regs::CUR_CLR1, 0x00FF_0000);
    set_reg(&mut cpu, regs::CUR_HORZ_VERT_POSN, 100 << 16 | 50);
    set_reg(&mut cpu, regs::GEN_TEST_CNTL, 0x180);
    let (width, height) = video::frame_size(&cpu.bus);
    let mut frame = Frame::new(width, height);
    cpu.bus.vga.mark_dirty_full();
    video::render_screen(&mut frame, &cpu.bus);
    let at = |x: usize, y: usize| {
        let i = (y * width as usize + x) * 4;
        (frame.rgba[i], frame.rgba[i + 1], frame.rgba[i + 2])
    };
    assert_eq!(at(50, 100), (0, 0, 0), "transparent");
    assert_eq!(at(51, 100), (255, 255, 255), "inverted black");
    assert_eq!(at(52, 100), (0, 255, 0), "CUR_CLR1");
    assert_eq!(at(52, 99), (0, 0, 0));
}

#[test]
fn state_round_trips() {
    let mut cpu = machine();
    int10(&mut cpu, 0x4F02, 0x4111, 0, 0);
    set_reg(&mut cpu, regs::SCRATCH_REG0, 0x1234_5678);
    let mut w = rust_dos::savestate::Writer::new();
    rust_dos::savestate::State::save(&cpu.bus.mach64, &mut w);
    let mut other = machine();
    let mut r = rust_dos::savestate::Reader::new(&w.buf);
    rust_dos::savestate::State::load(&mut other.bus.mach64, &mut r).unwrap();
    assert_eq!(other.bus.mach64.reg(regs::SCRATCH_REG0), 0x1234_5678);
    assert_eq!(other.bus.mach64.screen(), cpu.bus.mach64.screen());
}
