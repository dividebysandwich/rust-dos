//! A monochrome monitor (`monochrome` with a VGA or an EGA) as programs
//! see it: the display combination code, the equipment word, mode 7 at the
//! prompt, the VGA BIOS summing colours to grey, and the EGA's monochrome
//! modes.

use iced_x86::Register;
use rust_dos::config::Settings;
use rust_dos::cpu::Cpu;
use rust_dos::interrupts::int10;
use rust_dos::video::adapter::{Adapter, VideoSetup};
use rust_dos::video::mono::Monochrome;
use rust_dos::video::{self, Frame, VideoMode, bios};
use std::path::PathBuf;

fn int10(cpu: &mut Cpu, ax: u16, bx: u16, cx: u16, dx: u16) {
    cpu.set_ax(ax);
    cpu.set_reg16(Register::BX, bx);
    cpu.set_cx(cx);
    cpu.set_dx(dx);
    int10::handle(cpu);
}

/// A machine with `adapter` and a monochrome monitor, at the prompt.
fn mono(adapter: Adapter) -> Cpu {
    let mut cpu = Cpu::new(PathBuf::from("."));
    bios::install(&mut cpu.bus, VideoSetup { adapter, mono_monitor: true });
    cpu.load_shell();
    cpu
}

fn dac(cpu: &Cpu, entry: usize) -> [u8; 3] {
    cpu.bus.vga.palette[entry * 3..entry * 3 + 3].try_into().unwrap()
}

#[test]
fn the_setting_puts_a_monochrome_monitor_on_the_vgas_and_the_ega() {
    for (machine, mono) in [(Adapter::Svga, true), (Adapter::Vga, true), (Adapter::Ega, true), (Adapter::Cga, false)] {
        let settings = Settings { machine, monochrome: Monochrome::Green, ..Settings::default() };
        assert_eq!(settings.video_setup().mono(), mono, "{:?}", machine);
        let colour = Settings { machine, ..Settings::default() };
        assert!(!colour.video_setup().mono());
    }
    let hercules = Settings { machine: Adapter::Hercules, ..Settings::default() };
    assert!(hercules.video_setup().mono());
}

#[test]
fn a_vga_with_a_monochrome_monitor() {
    let mut cpu = mono(Adapter::Vga);
    int10(&mut cpu, 0x1A00, 0, 0, 0);
    assert_eq!((cpu.get_al(), cpu.get_reg8(Register::BL)), (0x1A, 0x07));
    assert_eq!(cpu.bus.read_16(0x0410) & 0x30, 0x30);
    assert_eq!(cpu.bus.read_16(0x0463), 0x3B4);
    assert_eq!(cpu.bus.read_8(0x0489) & 0x06, 0x06, "gray-scale summing, a monochrome monitor");
    // The prompt is in mode 7, at B0000h, in 9x16 cells.
    assert_eq!(cpu.bus.video_mode, VideoMode::Mono80x25);
    assert_eq!(cpu.bus.read_8(0x0449), 0x07);
    video::print_string(&mut cpu, "HI");
    assert_eq!(cpu.bus.read_8(0xB0000), b'H');
    assert_eq!(video::frame_size(&cpu.bus), (720, 400));
    let (width, height) = video::frame_size(&cpu.bus);
    let mut frame = Frame::new(width, height);
    video::render_screen(&mut frame, &cpu.bus);
    assert!(frame.rgb[..720 * 16 * 3].iter().any(|&b| b != 0));
    // Colour modes still work, in grey.
    int10(&mut cpu, 0x0003, 0, 0, 0);
    assert_eq!(cpu.bus.video_mode, VideoMode::Text80x25Color);
    let red = dac(&cpu, 4);
    assert!(red[0] == red[1] && red[1] == red[2] && red[0] > 0, "{:?}", red);
}

#[test]
fn the_bios_sums_the_colours_it_loads() {
    let mut cpu = mono(Adapter::Svga);
    int10(&mut cpu, 0x0013, 0, 0, 0);
    // Set DAC entry 1 to bright red: it becomes a grey of its brightness.
    int10(&mut cpu, 0x1010, 0x0001, 0x0000, 0x3F00);
    assert_eq!(dac(&cpu, 1), [19; 3]);
    int10(&mut cpu, 0x1015, 0x0001, 0, 0);
    assert_eq!(cpu.get_reg8(Register::DH), 19);
    // AH=12h BL=33h turns summing off.
    int10(&mut cpu, 0x1201, 0x0033, 0, 0);
    assert_eq!(cpu.get_al(), 0x12);
    int10(&mut cpu, 0x1010, 0x0001, 0x0000, 0x3F00);
    assert_eq!(dac(&cpu, 1), [0x3F, 0, 0]);
    // A program writing the DAC itself isn't summed: the BIOS does it.
    cpu.bus.io_write(0x3C8, 2);
    for value in [0, 0x3F, 0] {
        cpu.bus.io_write(0x3C9, value);
    }
    assert_eq!(dac(&cpu, 2), [0, 0x3F, 0]);
}

#[test]
fn an_ega_with_ibms_monochrome_display() {
    let mut cpu = mono(Adapter::Ega);
    int10(&mut cpu, 0x1200, 0x0010, 0, 0);
    assert_eq!(cpu.get_reg8(Register::BH), 1, "a monochrome CRTC");
    assert_eq!(cpu.get_reg8(Register::CL), 0x0B);
    assert_eq!(cpu.bus.read_8(0x0487) & 0x02, 0x02);
    assert_eq!(cpu.bus.video_mode, VideoMode::Mono80x25);
    assert_eq!(video::frame_size(&cpu.bus), (720, 350));
    // Only the monochrome modes.
    int10(&mut cpu, 0x0003, 0, 0, 0);
    assert_eq!(cpu.bus.video_mode, VideoMode::Mono80x25);
    int10(&mut cpu, 0x000F, 0, 0, 0);
    assert_eq!(cpu.bus.video_mode, VideoMode::Ega640x350Mono);
    assert_eq!(video::frame_size(&cpu.bus), (640, 350));
}

#[test]
fn the_vgas_two_colour_modes() {
    // Mode 11h: 640x480, plane 0 in white.
    let mut cpu = Cpu::new(PathBuf::from("."));
    int10(&mut cpu, 0x0011, 0, 0, 0);
    assert_eq!(cpu.bus.video_mode, VideoMode::Vga640x480Mono);
    assert_eq!(video::frame_size(&cpu.bus), (640, 480));
    cpu.bus.write_8(0xA0000, 0x80);
    let mut frame = Frame::new(640, 480);
    video::render_screen(&mut frame, &cpu.bus);
    assert_eq!(&frame.rgb[..6], &[0xFC, 0xFC, 0xFC, 0, 0, 0]);
    // Mode 0Fh: 640x350 monochrome graphics.
    int10(&mut cpu, 0x000F, 0, 0, 0);
    assert_eq!(cpu.bus.video_mode, VideoMode::Ega640x350Mono);
    assert_eq!(video::frame_size(&cpu.bus), (640, 350));
}
