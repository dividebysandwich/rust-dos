//! INT 10h AH=F0h-F5h and FAh, the EGA Register Interface Library that the
//! mouse driver adds to the EGA's and VGA's BIOS.

use iced_x86::Register;
use rust_dos::cpu::Cpu;
use rust_dos::interrupts::int10;
use rust_dos::video::adapter::{Adapter, VideoSetup};
use rust_dos::video::{self, Frame, bios};
use std::path::PathBuf;

/// INT 10h with the given registers.
fn int10(cpu: &mut Cpu, ax: u16, bx: u16, cx: u16, dx: u16) {
    cpu.set_ax(ax);
    cpu.set_reg16(Register::BX, bx);
    cpu.set_cx(cx);
    cpu.set_dx(dx);
    int10::handle(cpu);
}

fn machine(mode: u8) -> Box<Cpu> {
    let mut cpu = Cpu::new(PathBuf::from("."));
    int10(&mut cpu, mode as u16, 0, 0, 0);
    cpu
}

/// The colour of the top left pixel, as 8-bit RGB.
fn first_pixel(cpu: &mut Cpu) -> (u8, u8, u8) {
    cpu.bus.vga.latch_start_address();
    let (width, height) = video::frame_size(&cpu.bus);
    let mut frame = Frame::new(width, height);
    cpu.bus.vga.mark_dirty_full();
    video::render_screen(&mut frame, &cpu.bus);
    (frame.rgba[0], frame.rgba[1], frame.rgba[2])
}

const BUFFER: u16 = 0x2000;

fn buffer(offset: u16) -> usize {
    (BUFFER as usize) * 16 + offset as usize
}

#[test]
fn a_map_mask_written_through_the_ril_picks_the_planes() {
    // A-Train loads its map graphics a plane at a time, selecting each
    // with the Map Mask through AH=F1h.
    let mut cpu = machine(0x12);
    int10(&mut cpu, 0xF100, 0x0402, 0, 0x08);
    cpu.bus.write_8(0xA0000, 0x80);
    assert_eq!(cpu.bus.vga.sequencer_regs[2], 0x04);
    assert_eq!(first_pixel(&mut cpu), cpu.bus.vga.get_rgb(4));
}

#[test]
fn one_register_reads_back_and_the_programs_index_stays() {
    let mut cpu = machine(0x12);
    cpu.bus.io_write(0x3CE, 0x05);
    int10(&mut cpu, 0xF100, 0x0703, 0, 0x10);
    assert_eq!(cpu.bus.vga.graphics_regs[3], 0x07);
    assert_eq!(cpu.bus.io_read(0x3CE), 0x05);
    int10(&mut cpu, 0xF000, 0x0003, 0, 0x10);
    assert_eq!(cpu.get_reg8(Register::BL), 0x07);

    // The CRT controller at the BIOS's port, and the Miscellaneous Output,
    // a single register written from BL.
    int10(&mut cpu, 0xF100, 0x420F, 0, 0x00);
    assert_eq!(cpu.bus.vga.crtc_regs[0x0F], 0x42);
    let misc = cpu.bus.vga.misc_output_reg;
    int10(&mut cpu, 0xF100, misc as u16 ^ 0x20, 0, 0x20);
    assert_eq!(cpu.bus.vga.misc_output_reg, misc ^ 0x20);
    int10(&mut cpu, 0xF000, 0, 0, 0x20);
    assert_eq!(cpu.get_reg8(Register::BL), misc ^ 0x20);
}

#[test]
fn an_attribute_register_is_written_with_the_flip_flop_as_it_was() {
    let mut cpu = machine(0x12);
    // The program left the flip-flop at the data, after index 01h.
    cpu.bus.io_read(0x3DA);
    cpu.bus.io_write(0x3C0, 0x21);
    int10(&mut cpu, 0xF100, 0x3F04, 0, 0x18);
    assert_eq!(cpu.bus.vga.attribute_regs[4], 0x3F);
    // Its next write still goes to register 01h.
    cpu.bus.io_write(0x3C0, 0x15);
    assert_eq!(cpu.bus.vga.attribute_regs[1], 0x15);
}

#[test]
fn ranges_and_sets_go_through_es_bx() {
    let mut cpu = machine(0x12);
    cpu.set_es(BUFFER);
    // F3h: graphics registers 07h and 08h from the table.
    cpu.bus.write_8(buffer(0), 0x0C);
    cpu.bus.write_8(buffer(1), 0xF0);
    int10(&mut cpu, 0xF300, 0, 0x0702, 0x10);
    assert_eq!(cpu.bus.vga.graphics_regs[7..9], [0x0C, 0xF0]);
    // F2h reads them back.
    int10(&mut cpu, 0xF200, 0x0010, 0x0702, 0x10);
    assert_eq!(
        [cpu.bus.read_8(buffer(0x10)), cpu.bus.read_8(buffer(0x11))],
        [0x0C, 0xF0]
    );

    // F5h: a set of records, the group's port, the index and the value.
    let records = [0x08, 0x00, 0x02, 0x03, 0x10, 0x00, 0x05, 0x02];
    for (i, byte) in records.into_iter().enumerate() {
        cpu.bus.write_8(buffer(0x20 + i as u16), byte);
    }
    int10(&mut cpu, 0xF500, 0x0020, 2, 0);
    assert_eq!(cpu.bus.vga.sequencer_regs[2], 0x03);
    assert_eq!(cpu.bus.vga.graphics_regs[5], 0x02);
    // F4h fills the records' values in.
    cpu.bus.write_8(buffer(0x23), 0);
    cpu.bus.write_8(buffer(0x27), 0);
    int10(&mut cpu, 0xF400, 0x0020, 2, 0);
    assert_eq!(
        [cpu.bus.read_8(buffer(0x23)), cpu.bus.read_8(buffer(0x27))],
        [0x03, 0x02]
    );
}

/// A machine with `adapter`, in its text mode.
fn machine_with(adapter: Adapter) -> Box<Cpu> {
    let mut cpu = Cpu::new(PathBuf::from("."));
    let setup = VideoSetup {
        adapter,
        ..Default::default()
    };
    bios::install(&mut cpu.bus, setup);
    int10::set_mode(&mut cpu, setup.prompt_mode());
    cpu
}

#[test]
fn interrogating_finds_the_ril_on_an_ega_and_a_vga_only() {
    for adapter in [Adapter::Ega, Adapter::Vga, Adapter::Svga, Adapter::S3] {
        let mut cpu = machine_with(adapter);
        int10(&mut cpu, 0xFA00, 0, 0, 0);
        assert_ne!(cpu.bx(), 0, "{adapter:?}");
        let version = (cpu.es() as usize) * 16 + cpu.bx() as usize;
        assert_eq!(
            [cpu.bus.read_8(version), cpu.bus.read_8(version + 1)],
            [1, 0],
            "{adapter:?}"
        );
    }
    let others = [
        Adapter::Cga,
        Adapter::Hercules,
        Adapter::Tandy,
        Adapter::Pcjr,
    ];
    for adapter in others {
        let mut cpu = machine_with(adapter);
        int10(&mut cpu, 0xFA00, 0, 0, 0);
        assert_eq!(cpu.bx(), 0, "{adapter:?}");
    }
}

#[test]
fn a_booted_system_has_no_ril_of_the_bios() {
    // Its mouse driver, if it loads one, brings the RIL; the BIOS leaves
    // the functions alone.
    let mut cpu = machine(0x12);
    cpu.bus.boot = Some(Default::default());
    int10(&mut cpu, 0xFA00, 0, 0, 0);
    assert_eq!(cpu.bx(), 0);
    let map_mask = cpu.bus.vga.sequencer_regs[2];
    int10(&mut cpu, 0xF100, 0x0102, 0, 0x08);
    assert_eq!(cpu.bus.vga.sequencer_regs[2], map_mask);
}

#[test]
fn feature_control_reads_as_the_bios_left_it() {
    let mut cpu = machine(0x12);
    int10(&mut cpu, 0xF000, 0x00FF, 0, 0x28);
    assert_eq!(cpu.get_reg8(Register::BL), 0);
}
