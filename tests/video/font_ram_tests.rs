//! An EGA's or VGA's text modes draw their characters with the font in
//! plane 2, which the BIOS loads at a mode set and programs change with
//! INT 10h AH=11h or by writing the plane: HWiNFO for DOS draws its
//! windows' frames and shadows with glyphs of its own.

use iced_x86::Register;
use rust_dos::cpu::Cpu;
use rust_dos::interrupts::int10;
use rust_dos::video::{self, Frame};
use std::path::PathBuf;

/// Where the tests keep a program's glyphs: 2000:0000.
const GLYPHS_SEG: u16 = 0x2000;

fn machine() -> Box<Cpu> {
    let mut cpu = Cpu::new(PathBuf::from("."));
    cpu.load_shell();
    cpu
}

/// INT 10h with the given registers.
fn int10(cpu: &mut Cpu, ax: u16, bx: u16, cx: u16, dx: u16) {
    cpu.set_ax(ax);
    cpu.set_reg16(Register::BX, bx);
    cpu.set_cx(cx);
    cpu.set_dx(dx);
    int10::handle(cpu);
}

/// The text cell at the top left: character `ch`, attribute `attr`.
fn put(cpu: &mut Cpu, ch: u8, attr: u8) {
    cpu.bus.text_mem_mut()[..2].copy_from_slice(&[ch, attr]);
    cpu.bus.vga.mark_dirty_full();
}

/// The top left 8x16 cell as drawn: a row of 8 bits for each scanline,
/// set where the pixel is lit.
fn cell(cpu: &Cpu) -> Vec<u8> {
    let (width, height) = video::frame_size(&cpu.bus);
    let mut frame = Frame::new(width, height);
    video::render_screen(&mut frame, &cpu.bus);
    (0..16)
        .map(|y| {
            (0..8).fold(0u8, |bits, x| {
                let i = (y * width as usize + x) * 4;
                let lit = frame.rgba[i..i + 3].iter().any(|&c| c != 0);
                bits << 1 | lit as u8
            })
        })
        .collect()
}

/// A glyph of 16 rows, a frame around the cell.
const BOX: [u8; 16] = [
    0xFF, 0x81, 0x81, 0x81, 0x81, 0x81, 0x81, 0x81, 0x81, 0x81, 0x81, 0x81, 0x81, 0x81, 0x81, 0xFF,
];

/// Load `BOX` as character `ch` of map `map` with INT 10h AX=1100h.
fn load_box(cpu: &mut Cpu, map: u8, ch: u16) {
    cpu.bus.load_bytes((GLYPHS_SEG as usize) << 4, &BOX);
    cpu.set_es(GLYPHS_SEG);
    cpu.set_bp(0);
    int10(cpu, 0x1100, 0x1000 | map as u16, 1, ch);
}

/// AX=1100h loads a program's glyph (BH=16 bytes, CX=1 of them, from
/// character DX) into map 0, and the text shows it in place of the ROM's.
/// HWiNFO loads its frames and shadows so, one character at a time.
#[test]
fn a_glyph_a_program_loads_replaces_the_roms() {
    let mut cpu = machine();
    put(&mut cpu, b'A', 0x07);
    let rom = cell(&cpu);
    assert_ne!(rom, BOX);
    load_box(&mut cpu, 0, b'A' as u16);
    assert_eq!(cell(&cpu), BOX);
    // The others keep the ROM's glyphs.
    put(&mut cpu, b'B', 0x07);
    assert_ne!(cell(&cpu), BOX);
    // A mode set loads the ROM's font again.
    int10(&mut cpu, 0x0003, 0, 0, 0);
    put(&mut cpu, b'A', 0x07);
    assert_eq!(cell(&cpu), rom);
}

/// A font written into plane 2 through the sequencer and the graphics
/// controller, as programs load one without the BIOS: map mask plane 2,
/// sequential addressing, A0000h for 64 KB.
#[test]
fn a_font_written_into_plane_2_shows() {
    let mut cpu = machine();
    let out = |cpu: &mut Cpu, port: u16, index: u8, value: u8| {
        cpu.bus.io_write(port, index);
        cpu.bus.io_write(port + 1, value);
    };
    out(&mut cpu, 0x3C4, 0x02, 0x04);
    out(&mut cpu, 0x3C4, 0x04, 0x07);
    out(&mut cpu, 0x3CE, 0x04, 0x02);
    out(&mut cpu, 0x3CE, 0x05, 0x00);
    out(&mut cpu, 0x3CE, 0x06, 0x04);
    for (row, &bits) in BOX.iter().enumerate() {
        cpu.bus.write_8(0xA0000 + b'Z' as usize * 32 + row, bits);
    }
    out(&mut cpu, 0x3C4, 0x02, 0x03);
    out(&mut cpu, 0x3C4, 0x04, 0x03);
    out(&mut cpu, 0x3CE, 0x04, 0x00);
    out(&mut cpu, 0x3CE, 0x05, 0x10);
    out(&mut cpu, 0x3CE, 0x06, 0x0E);
    put(&mut cpu, b'Z', 0x07);
    assert_eq!(cell(&cpu), BOX);
}

/// 512 characters: AX=1103h sets Character Map Select, here map 0 for
/// attribute bit 3 clear and map 1 for it set, and the characters with
/// bit 3 set take their glyphs from map 1, loaded with AX=1104h and a
/// glyph of its own.
#[test]
fn attribute_bit_3_selects_the_second_font() {
    let mut cpu = machine();
    int10(&mut cpu, 0x1104, 0x0001, 0, 0);
    load_box(&mut cpu, 1, b'A' as u16);
    put(&mut cpu, b'A', 0x0F);
    let one_font = cell(&cpu);
    assert_ne!(one_font, BOX, "map 0 for both until AX=1103h");
    int10(&mut cpu, 0x1103, 0x0004, 0, 0);
    assert_eq!(cell(&cpu), BOX);
    put(&mut cpu, b'A', 0x07);
    assert_eq!(cell(&cpu), one_font, "bit 3 clear: map 0's");
}
