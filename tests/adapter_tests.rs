//! The display adapters (`machine`) as programs find them: the BIOS
//! functions each has, what the BIOS data area says, and the modes it sets.

use iced_x86::Register;
use rust_dos::cpu::Cpu;
use rust_dos::interrupts::int10;
use rust_dos::video::adapter::{Adapter, VideoSetup};
use rust_dos::video::bios;
use rust_dos::video::{self, overlay, Frame};
use std::path::PathBuf;

/// INT 10h with the given registers.
fn int10(cpu: &mut Cpu, ax: u16, bx: u16, cx: u16, dx: u16) {
    cpu.set_ax(ax);
    cpu.set_reg16(Register::BX, bx);
    cpu.set_cx(cx);
    cpu.set_dx(dx);
    int10::handle(cpu);
}

/// A machine with `adapter`, in its text mode as at the prompt.
fn machine(adapter: Adapter) -> Cpu {
    let mut cpu = Cpu::new(PathBuf::from("."));
    bios::install(&mut cpu.bus, VideoSetup { adapter, ..Default::default() });
    int10::set_mode(&mut cpu, 0x03);
    cpu
}

#[test]
fn vesa_is_only_on_the_super_vga() {
    let mut cpu = machine(Adapter::Svga);
    int10(&mut cpu, 0x4F00, 0, 0, 0);
    assert_eq!(cpu.ax(), 0x004F);

    let mut cpu = machine(Adapter::Vga);
    int10(&mut cpu, 0x4F00, 0, 0, 0);
    assert_eq!(cpu.ax(), 0x4F00, "no VBE: the registers stay as they were");
    int10(&mut cpu, 0x4F02, 0x0101, 0, 0);
    assert_eq!(cpu.ax(), 0x4F02);
}

#[test]
fn both_vgas_have_the_vga_bios() {
    for adapter in [Adapter::Svga, Adapter::Vga] {
        let mut cpu = machine(adapter);
        int10(&mut cpu, 0x1A00, 0, 0, 0);
        assert_eq!(cpu.get_al(), 0x1A);
        assert_eq!(cpu.get_reg8(Register::BL), 0x08, "VGA with a colour monitor");
        assert_eq!(cpu.bus.read_16(0x0410) & 0x30, 0x20, "80x25 colour in the equipment word");
        assert_eq!(cpu.bus.read_16(0x0463), 0x3D4);
        assert_eq!(cpu.bus.read_8(0xC0000), 0x55);
        assert_eq!(cpu.bus.read_8(0xC0001), 0xAA);
    }
}

#[test]
fn the_vga_state_service_reports_support() {
    for adapter in [
        Adapter::Vga,
        Adapter::Svga,
        Adapter::S3,
        Adapter::S3Virge,
        Adapter::S3VirgeVx,
        Adapter::Et4000,
        Adapter::Verite,
    ] {
        let mut cpu = machine(adapter);
        int10(&mut cpu, 0x1C00, 0, 1, 0);
        assert_eq!(cpu.get_al(), 0x1C, "{:?}", adapter);
        assert_eq!(
            cpu.bx(),
            2,
            "two 64-byte blocks for VGA hardware state: {:?}",
            adapter
        );
    }

    let mut cpu = machine(Adapter::Ega);
    int10(&mut cpu, 0x1C00, 0, 1, 0);
    assert_eq!(cpu.ax(), 0x1C00, "EGA has no VGA state service");
}

#[test]
fn the_vga_state_service_restores_hardware_bios_and_dac_state() {
    let mut cpu = machine(Adapter::Vga);
    cpu.set_es(0x3000);
    cpu.set_bx(0);
    cpu.set_cx(7);
    cpu.bus.vga.sequencer_regs[1] = 0x12;
    cpu.bus.vga.crtc_regs[0x0C] = 0x34;
    cpu.bus.vga.palette[3..6].copy_from_slice(&[0x11, 0x22, 0x33]);
    cpu.bus.vga.dac_mask = 0x7F;
    cpu.bus.write_8(0x0449, 0x13);
    cpu.bus.write_8(0x0463, 0xD4);
    int10(&mut cpu, 0x1C00, 0, 7, 0);
    assert_eq!(cpu.get_al(), 0x1C);
    assert_eq!(cpu.bx(), 16, "the full state fits 16 64-byte blocks");
    int10(&mut cpu, 0x1C01, 0, 7, 0);
    assert_eq!(cpu.get_al(), 0x1C);

    cpu.bus.vga.sequencer_regs[1] = 0xFF;
    cpu.bus.vga.crtc_regs[0x0C] = 0xFF;
    cpu.bus.vga.palette[3..6].copy_from_slice(&[0x3F, 0x3F, 0x3F]);
    cpu.bus.vga.dac_mask = 0xFF;
    cpu.bus.write_8(0x0449, 0x03);
    cpu.bus.write_8(0x0463, 0xB4);

    int10(&mut cpu, 0x1C02, 0, 7, 0);
    assert_eq!(cpu.get_al(), 0x1C);
    assert_eq!(cpu.bus.vga.sequencer_regs[1], 0x12);
    assert_eq!(cpu.bus.vga.crtc_regs[0x0C], 0x34);
    assert_eq!(&cpu.bus.vga.palette[3..6], &[0x11, 0x22, 0x33]);
    assert_eq!(cpu.bus.vga.dac_mask, 0x7F);
    assert_eq!(cpu.bus.read_8(0x0449), 0x13);
    assert_eq!(cpu.bus.read_8(0x0463), 0xD4);
    assert_eq!(cpu.bus.video_mode, video::VideoMode::Graphics320x200);
}

#[test]
fn the_adapter_keeps_the_floppies_in_the_equipment_word() {
    let mut cpu = machine(Adapter::Vga);
    let floppies = cpu.bus.read_16(0x0410) & !0x30;
    bios::install(&mut cpu.bus, VideoSetup { adapter: Adapter::Svga, ..Default::default() });
    assert_eq!(cpu.bus.read_16(0x0410) & !0x30, floppies);
}

#[test]
fn the_mouse_counts_8_a_character_in_text_on_every_adapter() {
    // INT 33h AX=0000h: reset the driver, and where a click on the
    // picture lands.
    let reset = |cpu: &mut Cpu| {
        cpu.set_ax(0);
        rust_dos::interrupts::int33::handle(cpu);
        (cpu.bus.mouse.max_x, cpu.bus.mouse.max_y)
    };
    let click = |cpu: &Cpu, (x, y): (u32, u32)| {
        let (width, height) = video::frame_size(&cpu.bus);
        let frame = Frame::new(width, height);
        overlay::frame_to_mouse(&cpu.bus, &frame, ((x * width / 80) as i32, (y * height / 25) as i32))
    };
    for adapter in Adapter::ALL {
        let setup = VideoSetup { adapter, ..Default::default() };
        let mut cpu = Cpu::new(PathBuf::from("."));
        bios::install(&mut cpu.bus, setup);
        int10::set_mode(&mut cpu, setup.prompt_mode());
        assert_eq!(reset(&mut cpu), (639, 199), "{:?}", adapter);
        // Column 30, row 12: its top left corner, 8 a character.
        assert_eq!(click(&cpu, (30, 12)), (240, 96), "{:?}", adapter);
    }

    // 80x50 with the 8x8 font (INT 10h AX=1112h) has twice the rows.
    let mut cpu = machine(Adapter::Vga);
    int10(&mut cpu, 0x1112, 0, 0, 0);
    assert_eq!(reset(&mut cpu), (639, 399));
}
