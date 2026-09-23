//! The VESA BIOS Extensions: INT 10h AX=4Fxxh, banked and linear video
//! memory, and the picture VESA modes make.

mod pmrig;

use iced_x86::Register;
use rust_dos::cpu::Cpu;
use rust_dos::interrupts::int10;
use rust_dos::video::vbe::LFB_BASE;
use rust_dos::video::{self, Frame, VideoMode};
use std::path::PathBuf;

const BUF: usize = 0x30000; // 3000:0000

fn machine() -> Cpu {
    let mut cpu = Cpu::new(PathBuf::from("."));
    cpu.set_es(0x3000);
    cpu.set_di(0);
    cpu
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

fn set_mode(cpu: &mut Cpu, mode: u16) {
    assert_eq!(int10(cpu, 0x4F02, mode, 0, 0), 0x004F, "mode {:X}", mode);
}

/// The rendered picture's pixel at (x, y).
fn pixel(cpu: &mut Cpu, x: usize, y: usize) -> (u8, u8, u8) {
    let (width, height) = video::frame_size(&cpu.bus);
    let mut frame = Frame::new(width, height);
    cpu.bus.vga.mark_dirty_full();
    video::render_screen(&mut frame, &cpu.bus);
    let i = (y * width as usize + x) * 3;
    (frame.rgb[i], frame.rgb[i + 1], frame.rgb[i + 2])
}

#[test]
fn controller_info_in_both_sizes() {
    let mut cpu = machine();
    cpu.bus.write_8(BUF + 256, 0xAA);
    assert_eq!(int10(&mut cpu, 0x4F00, 0, 0, 0), 0x004F);
    assert_eq!(cpu.bus.read_32(BUF), u32::from_le_bytes(*b"VESA"));
    assert_eq!(cpu.bus.read_16(BUF + 4), 0x0200);
    assert_eq!(cpu.bus.read_16(BUF + 18), 64, "4 MB");
    assert_eq!(cpu.bus.read_8(BUF + 256), 0xAA, "a VBE 1.x block is 256 bytes");
    // The mode list, through its far pointer.
    let list = |cpu: &Cpu| {
        let ptr = cpu.bus.read_32(BUF + 14);
        let at = ((ptr >> 16) as usize) * 16 + (ptr & 0xFFFF) as usize;
        (0..).map(|i| cpu.bus.read_16(at + i * 2)).take_while(|&m| m != 0xFFFF).collect::<Vec<u16>>()
    };
    let modes = list(&cpu);
    assert_eq!(modes.len(), 16);
    assert!(modes.contains(&0x101) && modes.contains(&0x118));

    // VBE 2.0: 512 bytes, with the list and strings in the buffer.
    for (i, &b) in b"VBE2".iter().enumerate() {
        cpu.bus.write_8(BUF + i, b);
    }
    assert_eq!(int10(&mut cpu, 0x4F00, 0, 0, 0), 0x004F);
    assert_eq!(cpu.bus.read_32(BUF), u32::from_le_bytes(*b"VESA"));
    assert_eq!(cpu.bus.read_32(BUF + 14), 0x3000_0022, "mode list at 3000:0022");
    assert_eq!(list(&cpu), modes);
    let oem = cpu.bus.read_32(BUF + 6);
    assert_eq!(oem >> 16, 0x3000);
    assert_eq!(cpu.bus.read_8(BUF + (oem & 0xFFFF) as usize), b'r');
}

#[test]
fn mode_info() {
    let mut cpu = machine();
    assert_eq!(int10(&mut cpu, 0x4F01, 0, 0x101, 0), 0x004F);
    assert_eq!(cpu.bus.read_16(BUF), 0x009B);
    assert_eq!(cpu.bus.read_16(BUF + 8), 0xA000);
    assert_eq!((cpu.bus.read_16(BUF + 16), cpu.bus.read_16(BUF + 18), cpu.bus.read_16(BUF + 20)), (640, 640, 480));
    assert_eq!((cpu.bus.read_8(BUF + 25), cpu.bus.read_8(BUF + 27)), (8, 4));
    assert_eq!(cpu.bus.read_32(BUF + 40), LFB_BASE as u32);

    assert_eq!(int10(&mut cpu, 0x4F01, 0, 0x111, 0), 0x004F);
    assert_eq!((cpu.bus.read_8(BUF + 25), cpu.bus.read_8(BUF + 27)), (16, 6));
    let masks: Vec<u8> = (31..39).map(|i| cpu.bus.read_8(BUF + i)).collect();
    assert_eq!(masks, [5, 11, 6, 5, 5, 0, 0, 0]);

    assert_eq!(int10(&mut cpu, 0x4F01, 0, 0x118, 0), 0x004F);
    assert_eq!((cpu.bus.read_16(BUF + 16), cpu.bus.read_8(BUF + 25)), (4096, 32));

    assert_eq!(int10(&mut cpu, 0x4F01, 0, 0x102, 0), 0x014F);
}

#[test]
fn modes_switch_and_come_back() {
    let mut cpu = machine();
    set_mode(&mut cpu, 0x101);
    assert_eq!(cpu.bus.video_mode, VideoMode::Vesa);
    assert_eq!(cpu.bus.display_size(), (640, 480));
    assert_eq!(video::frame_size(&cpu.bus), (640, 480));
    assert_eq!(int10(&mut cpu, 0x4F03, 0, 0, 0), 0x004F);
    assert_eq!(cpu.bx(), 0x101);
    assert!((cpu.bus.vga.timing().hz() - 59.94).abs() < 0.05);

    // Palette writes don't make the VGA side mode 13h.
    cpu.bus.io_write(0x3C8, 1);
    for v in [10, 20, 30] {
        cpu.bus.io_write(0x3C9, v);
    }
    assert_eq!(cpu.bus.video_mode, VideoMode::Vesa);

    // Small modes are doubled.
    set_mode(&mut cpu, 0x10E | 0x4000);
    assert_eq!(video::frame_size(&cpu.bus), (640, 400));
    assert_eq!(int10(&mut cpu, 0x4F03, 0, 0, 0), 0x004F);
    assert_eq!(cpu.bx(), 0x410E);

    // A standard mode ends it, through 4F02 or AH=00h.
    set_mode(&mut cpu, 0x03);
    assert_eq!(cpu.bus.video_mode, VideoMode::Text80x25Color);
    set_mode(&mut cpu, 0x105);
    int10(&mut cpu, 0x0013, 0, 0, 0);
    assert_eq!(cpu.bus.video_mode, VideoMode::Graphics320x200);
    assert!(cpu.bus.vbe.mode.is_none());
}

#[test]
fn banked_and_linear_video_memory() {
    let mut cpu = machine();
    set_mode(&mut cpu, 0x101);
    // Pixel (0, 200) is at 128000: bank 1, offset 62464.
    assert_eq!(int10(&mut cpu, 0x4F05, 0x0000, 0, 1), 0x004F);
    cpu.bus.write_8(0xA0000 + 62464, 0x0F);
    assert_eq!(cpu.bus.vbe.vram[128_000], 0x0F);
    assert_eq!(pixel(&mut cpu, 0, 200), cpu.bus.vga.get_rgb(0x0F));
    assert_eq!(int10(&mut cpu, 0x4F05, 0x0100, 0, 0), 0x004F);
    assert_eq!(cpu.dx(), 1);
    assert_eq!(int10(&mut cpu, 0x4F05, 0x0000, 0, 64), 0x014F, "past the 4 MB");

    // The linear frame buffer, in all access sizes.
    cpu.bus.write_8(LFB_BASE + 5, 0x22);
    cpu.bus.write_16(LFB_BASE + 6, 0x4433);
    cpu.bus.write_32(LFB_BASE + 8, 0x8877_6655);
    assert_eq!(&cpu.bus.vbe.vram[5..12], &[0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]);
    assert_eq!(cpu.bus.read_32(LFB_BASE + 4), 0x4433_2200);
    assert_eq!(cpu.bus.read_16(LFB_BASE + 7), 0x5544);
    assert_eq!(cpu.bus.read_8(LFB_BASE + 11), 0x88);
}

#[test]
fn the_window_function_switches_banks_with_a_far_call() {
    let mut cpu = machine();
    set_mode(&mut cpu, 0x103);
    assert_eq!(int10(&mut cpu, 0x4F01, 0, 0x103, 0), 0x004F);
    let window_function = cpu.bus.read_32(BUF + 12);
    // CALL FAR [window function]; HLT, with BX=0 and DX=3.
    let code = [0x9A, window_function as u8, (window_function >> 8) as u8, (window_function >> 16) as u8, (window_function >> 24) as u8, 0xF4];
    for (i, &b) in code.iter().enumerate() {
        cpu.bus.write_8(0x20000 + i, b);
    }
    cpu.set_cs(0x2000);
    cpu.set_ip(0);
    cpu.set_ss(0x4000);
    cpu.set_sp(0x1000);
    cpu.set_bx(0);
    cpu.set_dx(3);
    for _ in 0..10 {
        if cpu.cs() == 0x2000 && cpu.ip() == 5 {
            break;
        }
        cpu.step();
    }
    assert_eq!((cpu.cs(), cpu.ip()), (0x2000, 5));
    assert_eq!(cpu.bus.vbe.bank, 3);
}

#[test]
fn scan_lines_and_display_start() {
    let mut cpu = machine();
    set_mode(&mut cpu, 0x101);
    assert_eq!(int10(&mut cpu, 0x4F06, 0x0000, 1024, 0), 0x004F);
    assert_eq!((cpu.bx(), cpu.cx(), cpu.dx()), (1024, 1024, 4096));
    assert_eq!(int10(&mut cpu, 0x4F06, 0x0003, 0, 0), 0x004F);
    assert_eq!(cpu.bx(), 8736, "4 MB over 480 lines, in steps of 8 bytes");
    assert_eq!(int10(&mut cpu, 0x4F06, 0x0000, 20000, 0), 0x024F);

    // The display start takes effect at the next retrace.
    cpu.bus.vbe.vram[1024 * 10 + 4] = 9;
    assert_eq!(int10(&mut cpu, 0x4F07, 0x0000, 4, 10), 0x004F);
    assert_eq!(cpu.bus.vbe.start, 1024 * 10 + 4);
    assert_ne!(pixel(&mut cpu, 0, 0), cpu.bus.vga.get_rgb(9));
    let frame_instructions = cpu.bus.vga.timing().frame_ns() * cpu.bus.clock.cycles_per_ms() as u64 / 1_000_000;
    cpu.bus.clock.icount += frame_instructions;
    cpu.bus.sync_display();
    assert_eq!(pixel(&mut cpu, 0, 0), cpu.bus.vga.get_rgb(9));
    assert_eq!(int10(&mut cpu, 0x4F07, 0x0001, 0, 0), 0x004F);
    assert_eq!((cpu.cx(), cpu.dx()), (4, 10));
}

#[test]
fn direct_color_pixels() {
    let mut cpu = machine();
    // 15 bits: 0RRRRRGG GGGBBBBB.
    set_mode(&mut cpu, 0x110 | 0x4000);
    cpu.bus.write_16(LFB_BASE + 2, 0b0_11111_00000_10000);
    assert_eq!(pixel(&mut cpu, 1, 0), (0xFF, 0, 0x84));
    // 16 bits: RRRRRGGG GGGBBBBB.
    set_mode(&mut cpu, 0x111 | 0x4000);
    cpu.bus.write_16(LFB_BASE, 0b00000_111111_00000);
    assert_eq!(pixel(&mut cpu, 0, 0), (0, 0xFF, 0));
    // 32 bits: blue, green, red, unused.
    set_mode(&mut cpu, 0x112 | 0x4000);
    cpu.bus.write_32(LFB_BASE + 4 * 641, 0x0012_3456);
    assert_eq!(pixel(&mut cpu, 1, 1), (0x12, 0x34, 0x56));
    // 320x200 is shown doubled.
    set_mode(&mut cpu, 0x10E | 0x4000);
    cpu.bus.write_16(LFB_BASE + 2 * (320 + 1), 0xFFFF);
    assert_eq!(pixel(&mut cpu, 3, 3), (0xFF, 0xFF, 0xFF));
    assert_eq!(pixel(&mut cpu, 1, 1), (0, 0, 0));
}

#[test]
fn eight_bit_dac_and_palette_data() {
    let mut cpu = machine();
    set_mode(&mut cpu, 0x101);
    // Entries 1 and 2 as blue, green, red, padding.
    for (i, &b) in [0x3F, 0x00, 0x00, 0, 0x00, 0x20, 0x3F, 0].iter().enumerate() {
        cpu.bus.write_8(BUF + i, b);
    }
    assert_eq!(int10(&mut cpu, 0x4F09, 0x0000, 2, 1), 0x004F);
    assert_eq!(cpu.bus.vga.get_rgb(1), (0, 0, 0xFC));
    assert_eq!(cpu.bus.vga.get_rgb(2), (0xFC, 0x80, 0));

    assert_eq!(int10(&mut cpu, 0x4F08, 0x0800, 0, 0), 0x004F);
    assert_eq!(cpu.get_reg8(Register::BH), 8);
    for (i, &b) in [0xFF, 0x80, 0x01, 0].iter().enumerate() {
        cpu.bus.write_8(BUF + i, b);
    }
    assert_eq!(int10(&mut cpu, 0x4F09, 0x0000, 1, 3), 0x004F);
    assert_eq!(cpu.bus.vga.get_rgb(3), (0x01, 0x80, 0xFF));
    assert_eq!(int10(&mut cpu, 0x4F09, 0x0001, 1, 3), 0x004F);
    assert_eq!((0..4).map(|i| cpu.bus.read_8(BUF + i)).collect::<Vec<_>>(), [0xFF, 0x80, 0x01, 0]);
    // A mode set goes back to 6 bits.
    set_mode(&mut cpu, 0x101);
    assert_eq!(int10(&mut cpu, 0x4F08, 0x0001, 0, 0), 0x004F);
    assert_eq!(cpu.get_reg8(Register::BH), 6);
}

#[test]
fn the_mouse_spans_the_vesa_screen() {
    let mut cpu = machine();
    set_mode(&mut cpu, 0x103);
    cpu.set_ax(0);
    rust_dos::interrupts::int33::handle(&mut cpu);
    assert_eq!((cpu.bus.mouse.max_x, cpu.bus.mouse.max_y), (799, 599));
}

#[test]
fn the_shell_takes_back_vesa_and_the_mouse_handler() {
    let mut cpu = machine();
    set_mode(&mut cpu, 0x101);
    // INT 33h AX=000Ch: event handler 2000:0000 for all events.
    cpu.set_ax(0x000C);
    cpu.set_cx(0x7F);
    cpu.set_es(0x2000);
    cpu.set_dx(0);
    rust_dos::interrupts::int33::handle(&mut cpu);
    assert_eq!(cpu.bus.mouse.callback_cs, 0x2000);

    cpu.load_shell();
    assert_eq!(cpu.bus.video_mode, VideoMode::Text80x25Color);
    assert!(cpu.bus.vbe.mode.is_none());
    assert_eq!((cpu.bus.mouse.callback_cs, cpu.bus.mouse.callback_mask), (0, 0));
}

#[test]
fn the_protected_mode_interface_runs_in_a_32_bit_segment() {
    use iced_x86::code_asm::{bl, cx, dx, eax, edi};
    let mut rig = pmrig::Rig::new();
    let cpu = &mut rig.cpu;
    set_mode(cpu, 0x101);
    assert_eq!(int10(cpu, 0x4F0A, 0, 0, 0), 0x004F);
    let table = (cpu.es() as u32) << 4 | cpu.di() as u32;
    assert!(cpu.cx() > 8);
    let routine = |cpu: &Cpu, i: u32| table + cpu.bus.read_16((table + i * 2) as usize) as u32;
    let (set_window, set_start, set_palette) = (routine(cpu, 0), routine(cpu, 1), routine(cpu, 2));
    let ports = routine(cpu, 3) as usize;
    assert_eq!(cpu.bus.read_16(ports), 0x3D4);

    // Palette entry 7 as blue, green, red, padding.
    rig.write32(pmrig::DATA, 0x0030_2010);
    rig.run(|a| {
        a.mov(bl, 0)?;
        a.mov(dx, 5u32)?;
        a.mov(eax, set_window)?;
        a.call(eax)?;
        // Display start 21234h doublewords.
        a.mov(bl, 0)?;
        a.mov(cx, 0x1234u32)?;
        a.mov(dx, 0x0002u32)?;
        a.mov(eax, set_start)?;
        a.call(eax)?;
        a.mov(bl, 0)?;
        a.mov(cx, 1u32)?;
        a.mov(dx, 7u32)?;
        a.mov(edi, pmrig::DATA)?;
        a.mov(eax, set_palette)?;
        a.call(eax)?;
        // And again, waiting for the retrace.
        a.mov(bl, 0x80)?;
        a.mov(cx, 0x0010u32)?;
        a.mov(dx, 0u32)?;
        a.mov(eax, set_start)?;
        a.call(eax)?;
        a.hlt()?;
        Ok(())
    });
    let cpu = &mut rig.cpu;
    assert_eq!(cpu.bus.vbe.bank, 5);
    assert_eq!(&cpu.bus.vga.palette[21..24], &[0x30, 0x20, 0x10]);
    // The retrace began as the call returned: the new start shows.
    assert_eq!(cpu.bus.vbe.start, 0x40);
    assert_eq!(cpu.bus.vbe.latched_start, 0x40);
    assert_ne!(cpu.bus.io_read(0x3DA) & 0x08, 0, "in the retrace");
}
