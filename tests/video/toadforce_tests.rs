//! The ToadForce (`rust_dos::toadforce`) in the machine, as a guest
//! reaches it: its PCI function, its three windows in the address
//! space, the screen following its mode registers, its interrupt, the
//! video BIOS beside it, and save states. The card's own rules (what a
//! command does, what is rejected) are tested with the card, in
//! `src/toadforce`.

use rust_dos::cpu::Cpu;
use rust_dos::interrupts::int10;
use rust_dos::toadforce::command::{self, encode};
use rust_dos::toadforce::format::{Format, Rect, Surface};
use rust_dos::toadforce::pci::{INITIAL_MEMORY, INITIAL_REGISTERS, INITIAL_RING};
use rust_dos::toadforce::regs;
use rust_dos::video::adapter::{Adapter, VideoSetup};
use rust_dos::video::{self, Frame, VideoMode, bios};
use std::path::PathBuf;

const REGISTERS: usize = INITIAL_REGISTERS as usize;
const MEMORY: usize = INITIAL_MEMORY as usize;
const RING: usize = INITIAL_RING as usize;

fn machine_with(adapter: Adapter) -> Cpu {
    let mut cpu = Cpu::new(PathBuf::from("."));
    let setup = VideoSetup {
        adapter,
        ..Default::default()
    };
    bios::install(&mut cpu.bus, setup);
    int10::set_mode(&mut cpu, 0x03);
    cpu
}

fn machine() -> Cpu {
    machine_with(Adapter::ToadForce)
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

/// A register, through BAR 0.
fn reg(cpu: &Cpu, offset: u32) -> u32 {
    cpu.bus.read_32(REGISTERS + offset as usize)
}

fn set_reg(cpu: &mut Cpu, offset: u32, value: u32) {
    cpu.bus.write_32(REGISTERS + offset as usize, value);
}

/// A doubleword of the card's configuration space, at device 1.
fn config(cpu: &mut Cpu, register: u32) -> u32 {
    cpu.bus.io_write_wide(0xCF8, 0x8000_0800 | register, 4);
    cpu.bus.io_read_wide(0xCFC, 4)
}

fn set_config(cpu: &mut Cpu, register: u32, value: u32) {
    cpu.bus.io_write_wide(0xCF8, 0x8000_0800 | register, 4);
    cpu.bus.io_write_wide(0xCFC, value, 4);
}

/// Put `words` in the ring through BAR 2 from `RING_GET` on, and run
/// them with a write of `RING_PUT`.
fn run(cpu: &mut Cpu, words: &[u32]) {
    let mut at = reg(cpu, regs::RING_GET) as usize;
    for &word in words {
        cpu.bus.write_32(RING + at, word);
        at += 4;
    }
    set_reg(cpu, regs::RING_PUT, at as u32);
}

/// Show a mode of the card's own.
fn show(cpu: &mut Cpu, width: u32, height: u32, format: Format, refresh: u32) {
    for (register, value) in [
        (regs::MODE_WIDTH, width),
        (regs::MODE_HEIGHT, height),
        (regs::MODE_FORMAT, format.code()),
        (regs::MODE_PITCH, width * format.bytes()),
        (regs::MODE_REFRESH, refresh),
        (regs::MODE_ENABLE, 1),
    ] {
        set_reg(cpu, register, value);
    }
    assert_eq!(reg(cpu, regs::ERROR), 0);
    assert_eq!(reg(cpu, regs::MODE_ENABLE), 1);
}

/// Let `ns` nanoseconds of emulated time pass, and the devices' timers
/// with them.
fn pass(cpu: &mut Cpu, ns: u64) {
    let cycles = cpu.bus.clock.cycles_per_ms() as u64;
    cpu.bus.clock.icount += (ns * cycles).div_ceil(1_000_000);
    cpu.bus.service_timers();
}

/// The rendered picture's pixel at (x, y), after the display's next
/// retrace.
fn pixel(cpu: &mut Cpu, x: usize, y: usize) -> (u8, u8, u8) {
    cpu.bus.sync_display();
    let (width, height) = video::frame_size(&cpu.bus);
    let mut frame = Frame::new(width, height);
    cpu.bus.vga.mark_dirty_full();
    video::render_screen(&mut frame, &cpu.bus);
    let i = (y * width as usize + x) * 4;
    (frame.rgba[i], frame.rgba[i + 1], frame.rgba[i + 2])
}

#[test]
fn the_card_is_the_display_adapter_on_the_pci_bus() {
    let mut cpu = machine();
    assert!(cpu.bus.pci_present());
    assert_eq!(config(&mut cpu, 0x00), 0x7F01_1234);
    assert_eq!(config(&mut cpu, 0x08), 0x0300_0000);
    assert_eq!(config(&mut cpu, 0x10), INITIAL_REGISTERS);
    assert_eq!(config(&mut cpu, 0x14), INITIAL_MEMORY | 0x08);
    assert_eq!(config(&mut cpu, 0x18), INITIAL_RING);
    assert_eq!(config(&mut cpu, 0x2C), 0x5446_1234);
    assert_eq!(config(&mut cpu, 0x3C), 0x0000_010B);
    // 64 MB of card memory unless the settings say otherwise.
    set_config(&mut cpu, 0x14, 0xFFFF_FFFF);
    assert_eq!(config(&mut cpu, 0x14), 0xFC00_0008);
    set_config(&mut cpu, 0x14, INITIAL_MEMORY);
    assert_eq!(reg(&cpu, regs::MEMORY), 64 << 20);
    // No other function, and no other display adapter's device.
    cpu.bus.io_write_wide(0xCF8, 0x8000_0900, 4);
    assert_eq!(cpu.bus.io_read_wide(0xCFC, 4), 0xFFFF_FFFF);

    // The other adapters have no such card.
    let mut cpu = machine_with(Adapter::Svga);
    assert!(cpu.bus.toadforce.is_none());
    assert_eq!(cpu.bus.vbe.vram.len(), 4 << 20);
    assert_eq!(config(&mut cpu, 0x00), 0xFFFF_FFFF);
    assert_eq!(cpu.bus.read_32(REGISTERS), 0xFFFF_FFFF);
}

#[test]
fn the_memory_setting_sizes_the_card() {
    for megabytes in [32u32, 128] {
        let mut cpu = Cpu::new(PathBuf::from("."));
        cpu.bus.toadforce_memory = megabytes << 20;
        let setup = VideoSetup {
            adapter: Adapter::ToadForce,
            ..Default::default()
        };
        bios::install(&mut cpu.bus, setup);
        assert_eq!(reg(&cpu, regs::MEMORY), megabytes << 20);
        assert_eq!(cpu.bus.vbe.vram.len(), (megabytes as usize) << 20);
        set_config(&mut cpu, 0x14, 0xFFFF_FFFF);
        assert_eq!(config(&mut cpu, 0x14), !((megabytes << 20) - 1) | 0x08);
    }
    // Another adapter put in takes the card out, and its memory.
    let mut cpu = machine();
    let setup = VideoSetup {
        adapter: Adapter::Svga,
        ..Default::default()
    };
    bios::switch(&mut cpu, setup);
    assert!(cpu.bus.toadforce.is_none());
    assert_eq!(cpu.bus.vbe.vram.len(), 4 << 20);
    assert_eq!(cpu.bus.video_mode, VideoMode::Text80x25Color);
}

#[test]
fn the_registers_take_whole_words_and_read_by_bytes() {
    let mut cpu = machine();
    assert_eq!(reg(&cpu, regs::ID), 0x4441_4F54);
    let bytes: Vec<u8> = (0..4).map(|i| cpu.bus.read_8(REGISTERS + i)).collect();
    assert_eq!(bytes, b"TOAD");
    assert_eq!(cpu.bus.read_16(REGISTERS + 2), 0x4441);
    // A write of part of a register is ignored.
    cpu.bus.write_8(REGISTERS + regs::MODE_WIDTH as usize, 0x12);
    cpu.bus
        .write_16(REGISTERS + regs::MODE_WIDTH as usize, 0x1234);
    assert_eq!(reg(&cpu, regs::MODE_WIDTH), 0);
    set_reg(&mut cpu, regs::MODE_WIDTH, 0x1234);
    assert_eq!(reg(&cpu, regs::MODE_WIDTH), 0x1234);
    assert_eq!(
        cpu.bus.read_8(REGISTERS + regs::MODE_WIDTH as usize + 1),
        0x12
    );
    // The ring is memory: bytes written read back.
    cpu.bus.write_8(RING + 5, 0xAB);
    cpu.bus.write_16(RING + 6, 0xCDEF);
    assert_eq!(cpu.bus.read_32(RING + 4), 0xCDEF_AB00);
    cpu.bus.write_32(RING + 0xF_FFFC, 0x1122_3344);
    assert_eq!(cpu.bus.read_16(RING + 0xF_FFFE), 0x1122);
    assert_eq!(reg(&cpu, regs::ERROR), 0);
}

#[test]
fn card_memory_is_at_bar_1_and_moves_with_it() {
    let mut cpu = machine();
    let far = (64 << 20) - 4;
    cpu.bus.write_32(MEMORY + far, 0xDEAD_BEEF);
    cpu.bus.write_8(MEMORY + 5_000_000, 0x5A);
    assert_eq!(cpu.bus.vbe.vram[far..], [0xEF, 0xBE, 0xAD, 0xDE]);
    assert_eq!(cpu.bus.read_8(MEMORY + 5_000_000), 0x5A);
    // After the last byte there is nothing.
    assert_eq!(cpu.bus.read_8(MEMORY + (64 << 20)), 0xFF);
    // Moved, the windows are at their new addresses only.
    set_config(&mut cpu, 0x14, 0xC000_0000);
    set_config(&mut cpu, 0x10, 0xC800_0000);
    assert_eq!(cpu.bus.read_32(0xC000_0000 + far), 0xDEAD_BEEF);
    assert_eq!(cpu.bus.read_8(MEMORY + 5_000_000), 0xFF);
    assert_eq!(cpu.bus.read_32(0xC800_0000), regs::ID_VALUE);
    assert_eq!(cpu.bus.read_32(REGISTERS), 0xFFFF_FFFF);
    // The VESA BIOS gives the frame buffer where BAR 1 is.
    cpu.set_es(0x3000);
    cpu.set_di(0);
    assert_eq!(int10(&mut cpu, 0x4F01, 0, 0x101, 0), 0x004F);
    assert_eq!(cpu.bus.read_32(0x30000 + 40), 0xC000_0000);
    // The adapter's BIOS data written again, as when the built-in DOS
    // starts again, leaves the card and its windows where they are.
    let setup = cpu.bus.vga.setup();
    bios::install(&mut cpu.bus, setup);
    assert_eq!(cpu.bus.vbe.lfb_base, Some(0xC000_0000));
    assert_eq!(cpu.bus.read_32(0xC000_0000 + far), 0xDEAD_BEEF);
    assert_eq!(cpu.bus.read_8(MEMORY + 5_000_000), 0xFF);
    // With memory decoding off, none of the windows answers.
    set_config(&mut cpu, 0x04, 0x0001);
    assert_eq!(cpu.bus.read_32(0xC000_0000 + far), 0xFFFF_FFFF);
    assert_eq!(cpu.bus.read_32(0xC800_0000), 0xFFFF_FFFF);
    assert_eq!(cpu.bus.read_32(RING), 0xFFFF_FFFF);
    set_config(&mut cpu, 0x04, 0x0003);
    assert_eq!(cpu.bus.read_32(0xC800_0000), regs::ID_VALUE);
}

#[test]
fn commands_in_the_ring_draw_in_card_memory() {
    let mut cpu = machine();
    let surface = Surface {
        offset: 8 << 20,
        pitch: 256,
        width: 64,
        height: 32,
        format: Format::X8R8G8B8,
    };
    let whole = Rect {
        left: 0,
        top: 0,
        width: 64,
        height: 32,
    };
    let mut words = Vec::new();
    encode::fill(&mut words, &surface, &whole, 0x00C0_FFEE);
    run(&mut cpu, &words);
    assert_eq!(reg(&cpu, regs::ERROR), 0);
    assert_eq!(reg(&cpu, regs::RING_GET), 44);
    assert_eq!(
        cpu.bus.read_32(MEMORY + (8 << 20) + 31 * 256 + 63 * 4),
        0x00C0_FFEE
    );
    assert_eq!(cpu.bus.read_32(MEMORY + (8 << 20) + 32 * 256), 0);
    // A rejected command latches where it was in the ring.
    run(&mut cpu, &[command::header(0x7FFF, 1)]);
    assert_eq!(reg(&cpu, regs::ERROR), regs::Error::BadOpcode as u32);
    assert_eq!(reg(&cpu, regs::ERROR_AT), 44);
    assert_eq!(
        reg(&cpu, regs::STATUS) & regs::STATUS_ERROR,
        regs::STATUS_ERROR
    );
    set_reg(&mut cpu, regs::ERROR, 0);
    assert_eq!(reg(&cpu, regs::STATUS) & regs::STATUS_ERROR, 0);
}

#[test]
fn the_screen_shows_the_cards_mode_and_the_vga_comes_back() {
    let mut cpu = machine();
    cpu.bus.write_8(0xB8000, b'A');
    cpu.bus.write_16(MEMORY, 0xF800);
    cpu.bus.write_16(MEMORY + 2, 0x07E0);
    cpu.bus.write_16(MEMORY + 800 * 2, 0x001F);
    show(&mut cpu, 800, 600, Format::R5G6B5, 75);
    assert_eq!(cpu.bus.video_mode, VideoMode::Vesa);
    assert_eq!(video::frame_size(&cpu.bus), (800, 600));
    assert_eq!(pixel(&mut cpu, 0, 0), (0xFF, 0, 0));
    assert_eq!(pixel(&mut cpu, 1, 0), (0, 0xFF, 0));
    assert_eq!(pixel(&mut cpu, 0, 1), (0, 0, 0xFF));
    // The machine's timing is the card's: 600 lines and 38 of blank, 75
    // times a second, and port 3DAh says where in them it is.
    let timing = cpu.bus.vga.timing();
    assert_eq!((timing.display, timing.total), (600, 638));
    assert!((timing.hz() - 75.0).abs() < 0.1);
    let frame = timing.frame_ns();
    let into = cpu.bus.clock.now_ns() % frame;
    let line = timing.line_ns as u64;
    pass(&mut cpu, frame - into + 10 * line);
    assert_eq!(reg(&cpu, regs::SCANLINE), 10);
    assert_eq!(reg(&cpu, regs::STATUS) & regs::STATUS_VBLANK, 0);
    assert_eq!(cpu.bus.io_read(0x3DA) & 0x08, 0);
    let frames = reg(&cpu, regs::FRAME);
    pass(&mut cpu, 595 * line);
    assert_eq!(reg(&cpu, regs::SCANLINE), 605);
    assert_eq!(
        reg(&cpu, regs::STATUS) & regs::STATUS_VBLANK,
        regs::STATUS_VBLANK
    );
    assert_eq!(cpu.bus.io_read(0x3DA) & 0x08, 0x08);
    assert_eq!(reg(&cpu, regs::FRAME), frames + 1);

    // Another surface, by the register.
    cpu.bus.write_16(MEMORY + (1 << 20), 0xFFFF);
    set_reg(&mut cpu, regs::MODE_START, 1 << 20);
    // The display takes it at its next retrace.
    pass(&mut cpu, frame);
    assert_eq!(pixel(&mut cpu, 0, 0), (0xFF, 0xFF, 0xFF));
    // The VGA's registers take writes and none of it shows: its Start
    // Address, which moves a VESA mode's picture, leaves the card's.
    for (index, value) in [(0x0C, 0x12), (0x0D, 0x34)] {
        cpu.bus.io_write(0x3D4, index);
        cpu.bus.io_write(0x3D5, value);
    }
    assert_eq!(cpu.bus.vga.crtc_regs[0x0C], 0x12);
    assert_eq!(cpu.bus.vbe.start, 1 << 20);
    pass(&mut cpu, frame);
    assert_eq!(reg(&cpu, regs::MODE_START), 1 << 20);
    assert_eq!(pixel(&mut cpu, 0, 0), (0xFF, 0xFF, 0xFF));

    // The BIOS's mode set ends the card's mode: the text is back.
    assert_eq!(int10(&mut cpu, 0x0083, 0, 0, 0) & 0xFF00, 0);
    assert_eq!(reg(&cpu, regs::MODE_ENABLE), 0);
    assert_eq!(cpu.bus.video_mode, VideoMode::Text80x25Color);
    assert_eq!(cpu.bus.read_8(0xB8000), b'A');
    assert_eq!(cpu.bus.vga.timing().display, 400);
    // The registers keep the mode they described.
    assert_eq!(reg(&cpu, regs::MODE_WIDTH), 800);
}

#[test]
fn a_p8_mode_takes_its_colours_from_the_cards_palette() {
    let mut cpu = machine();
    cpu.bus.write_8(MEMORY, 7);
    cpu.bus.write_8(MEMORY + 1, 200);
    set_reg(&mut cpu, regs::PALETTE + 4 * 7, 0xFF10_2030);
    set_reg(&mut cpu, regs::PALETTE + 4 * 200, 0x00AA_BBCC);
    show(&mut cpu, 640, 480, Format::P8, 60);
    assert_eq!(reg(&cpu, regs::PALETTE + 4 * 7), 0x0010_2030);
    assert_eq!(pixel(&mut cpu, 0, 0), (0x10, 0x20, 0x30));
    assert_eq!(pixel(&mut cpu, 1, 0), (0xAA, 0xBB, 0xCC));
    // The VGA's DAC is another palette, which the card's mode ignores.
    cpu.bus.io_write(0x3C8, 7);
    for _ in 0..3 {
        cpu.bus.io_write(0x3C9, 0x3F);
    }
    assert_eq!(pixel(&mut cpu, 0, 0), (0x10, 0x20, 0x30));
    set_reg(&mut cpu, regs::PALETTE + 4 * 7, 0x00FF_0000);
    assert_eq!(pixel(&mut cpu, 0, 0), (0xFF, 0, 0));
}

#[test]
fn a_vesa_mode_comes_back_when_the_cards_mode_ends() {
    let mut cpu = machine();
    // 640x480 at 8 bits through the linear frame buffer, scrolled.
    assert_eq!(int10(&mut cpu, 0x4F02, 0x4101, 0, 0), 0x004F);
    assert_eq!(int10(&mut cpu, 0x4F07, 0, 0, 16), 0x004F);
    let (pitch, start) = (cpu.bus.vbe.pitch, cpu.bus.vbe.start);
    assert_eq!(start, 16 * 640);
    cpu.bus.write_8(MEMORY + start as usize, 15);
    let before = pixel(&mut cpu, 0, 0);
    // A byte past the BIOS's 4 MB, which its mode sets leave alone.
    cpu.bus.write_8(MEMORY + (6 << 20), 0x77);

    show(&mut cpu, 1024, 768, Format::X8R8G8B8, 60);
    assert_eq!(video::frame_size(&cpu.bus), (1024, 768));
    set_reg(&mut cpu, regs::MODE_ENABLE, 0);
    let mode = cpu.bus.vbe.mode.unwrap();
    assert_eq!((mode.number, mode.width, mode.bpp), (0x101, 640, 8));
    assert_eq!((cpu.bus.vbe.pitch, cpu.bus.vbe.start), (pitch, start));
    assert!(cpu.bus.vbe.lfb);
    assert_eq!(cpu.bus.vga.timing().display, 480);
    assert_eq!(pixel(&mut cpu, 0, 0), before);

    // A VESA mode set over the card's mode ends it, and clears the
    // BIOS's memory only.
    show(&mut cpu, 1024, 768, Format::X8R8G8B8, 60);
    assert_eq!(int10(&mut cpu, 0x4F02, 0x4111, 0, 0), 0x004F);
    assert_eq!(reg(&cpu, regs::MODE_ENABLE), 0);
    let mode = cpu.bus.vbe.mode.unwrap();
    assert_eq!((mode.number, mode.bpp), (0x111, 16));
    assert_eq!(cpu.bus.read_8(MEMORY + start as usize), 0);
    assert_eq!(cpu.bus.read_8(MEMORY + (6 << 20)), 0x77);

    // A program that ends in the card's mode leaves the prompt its text
    // mode, and the card none: nothing of the VESA mode under it is
    // left to come back.
    show(&mut cpu, 1024, 768, Format::X8R8G8B8, 60);
    bios::reset_for_shell(&mut cpu.bus);
    assert_eq!(reg(&cpu, regs::MODE_ENABLE), 0);
    assert_eq!(cpu.bus.video_mode, VideoMode::Text80x25Color);
    assert!(cpu.bus.vbe.mode.is_none());
    assert_eq!(cpu.bus.vga.timing().display, 400);
    let card = cpu.bus.toadforce.as_ref().unwrap();
    assert!(card.display.legacy.is_none());
}

#[test]
fn the_blank_interrupt_goes_up_on_the_cards_line() {
    let mut cpu = machine();
    show(&mut cpu, 640, 480, Format::X8R8G8B8, 60);
    let frame = cpu.bus.vga.timing().frame_ns();
    // Not enabled: no request, however long.
    pass(&mut cpu, 2 * frame);
    assert_eq!(
        reg(&cpu, regs::IRQ_STATUS) & regs::IRQ_VBLANK,
        regs::IRQ_VBLANK
    );
    assert!(!cpu.bus.pic.busy(11));
    set_reg(&mut cpu, regs::IRQ_STATUS, regs::IRQ_ALL);
    set_reg(&mut cpu, regs::IRQ_ENABLE, regs::IRQ_VBLANK);
    assert!(!cpu.bus.pic.busy(11));
    // The clock's deadline is the blank's, and serving it raises IRQ 11.
    pass(&mut cpu, frame);
    assert!(cpu.bus.pic.busy(11));
    // The system gives the card another line: the request moves.
    cpu.bus.io_write_wide(0xCF8, 0x8000_083C, 4);
    cpu.bus.io_write_wide(0xCFC, 10, 1);
    assert!(!cpu.bus.pic.busy(11));
    assert!(cpu.bus.pic.busy(10));
    // Cleared at the card, it is withdrawn, until the next blank.
    set_reg(&mut cpu, regs::IRQ_STATUS, regs::IRQ_VBLANK);
    assert!(!cpu.bus.pic.busy(10));
    pass(&mut cpu, frame);
    assert!(cpu.bus.pic.busy(10));
    // Disabled, too.
    set_reg(&mut cpu, regs::IRQ_ENABLE, 0);
    assert!(!cpu.bus.pic.busy(10));
}

#[test]
fn a_waiting_flip_shows_at_the_blank() {
    let mut cpu = machine();
    cpu.bus.write_32(MEMORY + (2 << 20), 0x00FF_FFFF);
    show(&mut cpu, 640, 480, Format::X8R8G8B8, 60);
    let timing = cpu.bus.vga.timing();
    // To the start of a frame, where the blank is 480 lines away.
    let into = cpu.bus.clock.now_ns() % timing.frame_ns();
    pass(&mut cpu, timing.frame_ns() - into + timing.line_ns as u64);
    let mut words = Vec::new();
    encode::flip(&mut words, 2 << 20, true);
    run(&mut cpu, &words);
    assert_eq!(reg(&cpu, regs::ERROR), 0);
    assert_eq!(
        reg(&cpu, regs::STATUS) & regs::STATUS_FLIP_PENDING,
        regs::STATUS_FLIP_PENDING
    );
    assert_eq!(reg(&cpu, regs::MODE_START), 0);
    assert_eq!(pixel(&mut cpu, 0, 0), (0, 0, 0));
    pass(&mut cpu, 400 * timing.line_ns as u64);
    assert_eq!(reg(&cpu, regs::MODE_START), 0);
    assert_eq!(cpu.bus.vbe.start, 0);
    pass(&mut cpu, 80 * timing.line_ns as u64);
    assert_eq!(reg(&cpu, regs::MODE_START), 2 << 20);
    assert_eq!(reg(&cpu, regs::STATUS) & regs::STATUS_FLIP_PENDING, 0);
    assert_eq!(reg(&cpu, regs::IRQ_STATUS) & regs::IRQ_FLIP, regs::IRQ_FLIP);
    assert_eq!(cpu.bus.vbe.start, 2 << 20);
    assert_eq!(pixel(&mut cpu, 0, 0), (0xFF, 0xFF, 0xFF));
}

#[test]
fn a_loaded_state_has_the_card_as_it_was() {
    let mut a = machine();
    // A VESA mode under the card's, a flip waiting, an interrupt
    // enabled, a rejection latched, a BAR moved and words in the ring.
    assert_eq!(int10(&mut a, 0x4F02, 0x4101, 0, 0), 0x004F);
    set_config(&mut a, 0x10, 0xC800_0000);
    let registers = 0xC800_0000usize;
    for i in 0..640 * 480 {
        a.bus.write_16(MEMORY + (3 << 20) + 2 * i, (i * 7) as u16);
    }
    let set = |cpu: &mut Cpu, offset: u32, value: u32| {
        cpu.bus.write_32(registers + offset as usize, value);
    };
    for (register, value) in [
        (regs::MODE_WIDTH, 640),
        (regs::MODE_HEIGHT, 480),
        (regs::MODE_FORMAT, Format::R5G6B5.code()),
        (regs::MODE_PITCH, 1280),
        (regs::MODE_REFRESH, 85),
        (regs::MODE_ENABLE, 1),
        (regs::IRQ_ENABLE, regs::IRQ_VBLANK | regs::IRQ_FLIP),
        (regs::PALETTE + 4 * 9, 0x00AB_CDEF),
    ] {
        set(&mut a, register, value);
    }
    let mut words = vec![command::header(0x7FFF, 1)];
    encode::flip(&mut words, 3 << 20, true);
    for (i, &word) in words.iter().enumerate() {
        a.bus.write_32(RING + 4 * i, word);
    }
    set(&mut a, regs::RING_PUT, 4 * words.len() as u32);

    let state = rust_dos::savestate::machine::save(&a);
    let mut b = machine();
    rust_dos::savestate::machine::load(&mut b, &state).unwrap();
    assert!(rust_dos::savestate::machine::save(&b) == state);
    for cpu in [&mut a, &mut b] {
        // Both go on the same way: the flip at the blank, the interrupt
        // with it, and the VESA mode back after.
        assert_eq!(cpu.bus.read_32(registers + regs::ERROR as usize), 1);
        assert_eq!(
            cpu.bus.read_32(registers + regs::PALETTE as usize + 36),
            0xAB_CDEF
        );
        assert_eq!(cpu.bus.read_32(RING + 4), words[1]);
        assert_eq!(video::frame_size(&cpu.bus), (640, 480));
        let timing = cpu.bus.vga.timing();
        assert_eq!((timing.display, timing.total), (480, 511));
        pass(cpu, timing.frame_ns());
        assert_eq!(
            cpu.bus.read_32(registers + regs::MODE_START as usize),
            3 << 20
        );
        assert!(cpu.bus.pic.busy(11));
        assert_eq!(pixel(cpu, 3, 0), (0, 0, 0xAD));
        cpu.bus.write_32(registers + regs::MODE_ENABLE as usize, 0);
        let mode = cpu.bus.vbe.mode.unwrap();
        assert_eq!((mode.number, mode.width, mode.bpp), (0x101, 640, 8));
    }
    assert!(rust_dos::savestate::machine::save(&a) == rust_dos::savestate::machine::save(&b));
}

#[test]
fn a_reset_of_the_machine_resets_the_card() {
    let mut cpu = machine();
    show(&mut cpu, 640, 480, Format::X8R8G8B8, 60);
    set_reg(&mut cpu, regs::IRQ_ENABLE, regs::IRQ_ALL);
    let frame = cpu.bus.vga.timing().frame_ns();
    pass(&mut cpu, frame);
    assert!(cpu.bus.pic.busy(11));
    cpu.bus.write_32(MEMORY + 100, 0x1234_5678);
    cpu.bus.reset_toadforce();
    assert_eq!(reg(&cpu, regs::MODE_ENABLE), 0);
    assert_eq!(reg(&cpu, regs::IRQ_ENABLE), 0);
    assert_eq!(reg(&cpu, regs::MODE_WIDTH), 0);
    assert!(!cpu.bus.pic.busy(11));
    assert_eq!(cpu.bus.video_mode, VideoMode::Text80x25Color);
    // Card memory keeps what it held.
    assert_eq!(cpu.bus.read_32(MEMORY + 100), 0x1234_5678);
}
