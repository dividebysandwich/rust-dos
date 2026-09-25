//! `machine=tandy` and `machine=pcjr`: what programs find, the video gate
//! array showing system memory through its page register, the 16-colour
//! and 640x200x4 modes, and DOS's memory around the video memory.

use iced_x86::Register;
use rust_dos::cpu::Cpu;
use rust_dos::interrupts::{int10, int12};
use rust_dos::mcb;
use rust_dos::video::adapter::{Adapter, VideoSetup};
use rust_dos::video::{self, Frame, VideoMode, bios};
use std::path::PathBuf;

const BLACK: (u8, u8, u8) = (0, 0, 0);
const BLUE: (u8, u8, u8) = (0, 0, 0xAA);
const GREEN: (u8, u8, u8) = (0, 0xAA, 0);
const CYAN: (u8, u8, u8) = (0, 0xAA, 0xAA);
const RED: (u8, u8, u8) = (0xAA, 0, 0);
const WHITE: (u8, u8, u8) = (0xFF, 0xFF, 0xFF);

fn int10(cpu: &mut Cpu, ax: u16, bx: u16, cx: u16, dx: u16) {
    cpu.set_ax(ax);
    cpu.set_reg16(Register::BX, bx);
    cpu.set_cx(cx);
    cpu.set_dx(dx);
    int10::handle(cpu);
}

/// A Tandy or PCjr at the DOS prompt, in `mode`.
fn machine(adapter: Adapter, mode: u8) -> Cpu {
    let mut cpu = Cpu::new(PathBuf::from("."));
    cpu.bus.set_cycles_per_ms(1000);
    bios::install(&mut cpu.bus, VideoSetup { adapter, ..Default::default() });
    cpu.load_shell();
    int10::set_mode(&mut cpu, mode);
    cpu
}

fn picture(cpu: &mut Cpu) -> Frame {
    cpu.bus.vga.latch_start_address();
    let (width, height) = video::frame_size(&cpu.bus);
    let mut frame = Frame::new(width, height);
    cpu.bus.vga.mark_dirty_full();
    video::render_screen(&mut frame, &cpu.bus);
    frame
}

fn pixel(frame: &Frame, x: usize, y: usize) -> (u8, u8, u8) {
    let i = (y * frame.width as usize + x) * 3;
    (frame.rgb[i], frame.rgb[i + 1], frame.rgb[i + 2])
}

#[test]
fn programs_find_a_tandy() {
    let mut cpu = machine(Adapter::Tandy, 0x03);
    // The model byte, and the BIOS name at F000:C000 games check for "!".
    assert_eq!(cpu.bus.read_8(0xFFFFE), 0xFF);
    assert_eq!(cpu.bus.read_8(0xFC000), 0x21);
    // 624 KB: the top 16 KB is the video memory.
    int12::handle(&mut cpu);
    assert_eq!(cpu.ax(), 624);
    // No VGA or EGA.
    int10(&mut cpu, 0x1A00, 0, 0, 0);
    assert_eq!(cpu.get_al(), 0x00);
    int10(&mut cpu, 0x1200, 0x0010, 0, 0);
    assert_eq!(cpu.get_reg8(Register::BL), 0x10);
}

#[test]
fn programs_find_a_pcjr() {
    let mut cpu = machine(Adapter::Pcjr, 0x03);
    assert_eq!(cpu.bus.read_8(0xFFFFE), 0xFD);
    assert_ne!(cpu.bus.read_8(0xFC000), 0x21);
    // Equipment bit 8: no DMA.
    assert_ne!(cpu.bus.read_16(0x0410) & 0x0100, 0);
    int12::handle(&mut cpu);
    assert_eq!(cpu.ax(), 640);
    // Another machine again: an AT with 640 KB.
    bios::install(&mut cpu.bus, VideoSetup::default());
    assert_eq!((cpu.bus.read_8(0xFFFFE), cpu.bus.read_16(0x0413)), (0xFC, 640));
}

#[test]
fn the_text_screen_is_system_memory() {
    // The prompt's page 7: the top 16 KB of 640 KB on a Tandy, of the first
    // 128 KB on a PCjr.
    for (adapter, page) in [(Adapter::Tandy, 0x9C000), (Adapter::Pcjr, 0x1C000)] {
        let mut cpu = machine(adapter, 0x03);
        cpu.bus.write_8(0xB8000, b'A');
        cpu.bus.write_8(0xB8001, 0x1F);
        assert_eq!(cpu.bus.read_8(page), b'A', "{:?}", adapter);
        assert_eq!(cpu.bus.read_8(0xB8001), 0x1F);
        // 16 KB, twice in the window.
        assert_eq!(cpu.bus.read_8(0xBC000), b'A');
        // Written through its RAM address, it shows too.
        cpu.bus.write_8(page + 2, b'B');
        assert_eq!(cpu.bus.read_8(0xB8002), b'B');
        let frame = picture(&mut cpu);
        // White on blue: the first cell's background is blue.
        assert_eq!(pixel(&frame, 7, 0), BLUE, "{:?}", adapter);
        assert_eq!(video::frame_size(&cpu.bus), (640, 400));
    }
}

#[test]
fn a_change_through_ram_is_repainted_at_the_next_retrace() {
    let mut cpu = machine(Adapter::Tandy, 0x09);
    picture(&mut cpu);
    cpu.bus.clock.icount += 20 * 1000;
    cpu.bus.sync_display();
    cpu.bus.vga.clear_dirty();
    // A program (or the recompiler) writing the memory as plain RAM.
    cpu.bus.write_8(0x98000, 0x11);
    assert!(!cpu.bus.vga.dirty);
    cpu.bus.clock.icount += 20 * 1000;
    cpu.bus.sync_display();
    assert!(cpu.bus.vga.dirty);
}

#[test]
fn mode_9_is_320x200_in_16_colours() {
    let mut cpu = machine(Adapter::Tandy, 0x09);
    assert_eq!(cpu.bus.video_mode, VideoMode::Tandy320x200x16);
    assert_eq!(cpu.bus.read_8(0x0449), 0x09);
    // Two pixels a byte, 160 bytes a line, the scanlines in 4 banks.
    cpu.bus.write_8(0xB8000, 0x12);
    cpu.bus.write_8(0xB8000 + 0x2000, 0x40);
    cpu.bus.write_8(0xB8000 + 160, 0x0F);
    let frame = picture(&mut cpu);
    assert_eq!((pixel(&frame, 0, 0), pixel(&frame, 2, 0)), (BLUE, GREEN));
    assert_eq!(pixel(&frame, 0, 2), RED);
    assert_eq!((pixel(&frame, 0, 8), pixel(&frame, 2, 8)), (BLACK, WHITE));
    // The Tandy's 32 KB modes are pages 6 and 7: 98000h on.
    assert_eq!(cpu.bus.read_8(0x98000), 0x12);
    // INT 10h reads and draws pixels there.
    int10(&mut cpu, 0x0D00, 0, 1, 0);
    assert_eq!(cpu.get_al(), 2);
    int10(&mut cpu, 0x0C04, 0, 3, 1);
    assert_eq!(cpu.bus.read_8(0x98000 + 0x2001), 0x04);
}

#[test]
fn mode_8_is_160x200_and_mode_a_640x200_in_4_colours() {
    let mut cpu = machine(Adapter::Tandy, 0x08);
    assert_eq!(cpu.bus.video_mode, VideoMode::Tandy160x200x16);
    cpu.bus.write_8(0xB8000, 0x3F);
    let frame = picture(&mut cpu);
    assert_eq!((pixel(&frame, 0, 0), pixel(&frame, 3, 0), pixel(&frame, 4, 0)), (CYAN, CYAN, WHITE));

    let mut cpu = machine(Adapter::Tandy, 0x0A);
    assert_eq!(cpu.bus.video_mode, VideoMode::Tandy640x200x4);
    // A byte of the low bits and one of the high bits for 8 pixels.
    cpu.bus.write_8(0xB8000, 0xC0);
    cpu.bus.write_8(0xB8001, 0xA0);
    let frame = picture(&mut cpu);
    assert_eq!((pixel(&frame, 0, 0), pixel(&frame, 1, 0), pixel(&frame, 2, 0)), (CYAN, BLUE, GREEN));
    int10(&mut cpu, 0x0D00, 0, 2, 0);
    assert_eq!(cpu.get_al(), 2);
}

#[test]
fn the_palette_registers_colour_the_pixels() {
    for adapter in [Adapter::Tandy, Adapter::Pcjr] {
        let mut cpu = machine(adapter, 0x09);
        cpu.bus.write_8(0xB8000, 0x10);
        // INT 10h AH=10h AL=00h: colour 1 is red.
        int10(&mut cpu, 0x1000, 0x0401, 0, 0);
        assert_eq!(pixel(&picture(&mut cpu), 0, 0), RED, "{:?}", adapter);
    }
    // The PCjr's gate array: index and data alternate at 3DAh, and reading
    // 3DAh makes the next write an index.
    let mut cpu = machine(Adapter::Pcjr, 0x09);
    cpu.bus.write_8(0x18000, 0x10);
    cpu.bus.io_read(0x3DA);
    cpu.bus.io_write(0x3DA, 0x11);
    cpu.bus.io_write(0x3DA, 0x02);
    cpu.bus.io_write(0x3DA, 0x00);
    assert_eq!(pixel(&picture(&mut cpu), 0, 0), GREEN);
    cpu.bus.io_write(0x3DA, 0x11);
    cpu.bus.io_read(0x3DA);
    cpu.bus.io_write(0x3DA, 0x00);
    assert_eq!(cpu.bus.vga.tandy.palette[1], 0x02, "the 00h after the read was an index");
}

#[test]
fn the_page_register_flips_pages() {
    let mut cpu = machine(Adapter::Tandy, 0x08);
    // Draw on page 7 (shown), then on page 5 through the processor's window.
    cpu.bus.write_8(0xB8000, 0x11);
    int10(&mut cpu, 0x0581, 0x0005, 0, 0);
    cpu.bus.write_8(0xB8000, 0x22);
    assert_eq!(cpu.bus.read_8(0x80000 + 5 * 0x4000), 0x22);
    assert_eq!(pixel(&picture(&mut cpu), 0, 0), BLUE);
    // Show page 5.
    int10(&mut cpu, 0x0582, 0x0500, 0, 0);
    assert_eq!(pixel(&picture(&mut cpu), 0, 0), GREEN);
    int10(&mut cpu, 0x0580, 0, 0, 0);
    assert_eq!((cpu.get_reg8(Register::BH), cpu.get_reg8(Register::BL)), (5, 5));
}

#[test]
fn the_cga_modes_look_as_on_a_cga() {
    let mut cpu = machine(Adapter::Tandy, 0x04);
    assert_eq!(cpu.bus.video_mode, VideoMode::Cga320x200Color);
    cpu.bus.write_8(0xB8000, 0x55);
    cpu.bus.write_8(0xBA000, 0xFF);
    let frame = picture(&mut cpu);
    assert_eq!(pixel(&frame, 0, 0), (0x55, 0xFF, 0xFF));
    assert_eq!(pixel(&frame, 0, 2), WHITE);
    let mut cpu = machine(Adapter::Pcjr, 0x06);
    cpu.bus.write_8(0xB8000, 0x80);
    let frame = picture(&mut cpu);
    assert_eq!((pixel(&frame, 0, 0), pixel(&frame, 1, 0)), (WHITE, BLACK));
}

#[test]
fn dos_memory_keeps_clear_of_the_video_memory() {
    // The Tandy's ends at 9C00h.
    let cpu = machine(Adapter::Tandy, 0x03);
    let chain = mcb::walk(&cpu.bus);
    let &(last, m) = chain.last().unwrap();
    assert!(m.is_free() && m.is_last());
    assert_eq!(last + 1 + m.size, 0x9C00);

    // The PCjr's first block for programs is above its video memory, and
    // mode 9 clearing that memory leaves the chain whole.
    let mut cpu = machine(Adapter::Pcjr, 0x03);
    let chain = mcb::walk(&cpu.bus);
    assert_eq!(chain[0].1.owner, mcb::DOS_OWNER);
    assert_eq!(chain[1].0, 0x2400);
    assert_eq!(cpu.resident_end, 0x2400);
    assert_eq!(cpu.transient_segment(), 0x2401);
    int10::set_mode(&mut cpu, 0x09);
    let chain = mcb::walk(&cpu.bus);
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[1].0 + 1 + chain[1].1.size, 0xA000);
}

#[test]
fn switching_machine_at_the_prompt_lays_memory_out_again() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    cpu.load_shell();
    video::print_string(&mut cpu, "C:\\>");
    bios::switch(&mut cpu, VideoSetup { adapter: Adapter::Tandy, ..Default::default() });
    let &(last, m) = mcb::walk(&cpu.bus).last().unwrap();
    assert_eq!(last + 1 + m.size, 0x9C00);
    // The prompt came along into the Tandy's memory.
    assert_eq!(cpu.bus.read_8(0x9C000 + 4), b'\\');
    bios::switch(&mut cpu, VideoSetup::default());
    let &(last, m) = mcb::walk(&cpu.bus).last().unwrap();
    assert_eq!(last + 1 + m.size, 0xA000);
    assert_eq!(cpu.bus.read_8(0xB8004), b'\\');
}
