//! The display adapters (`machine`) as programs find them: the BIOS
//! functions each has, what the BIOS data area says, and the modes it sets.

use iced_x86::Register;
use rust_dos::cpu::Cpu;
use rust_dos::interrupts::int10;
use rust_dos::video::adapter::{Adapter, VideoSetup};
use rust_dos::video::bios;
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
fn the_adapter_keeps_the_floppies_in_the_equipment_word() {
    let mut cpu = machine(Adapter::Vga);
    let floppies = cpu.bus.read_16(0x0410) & !0x30;
    bios::install(&mut cpu.bus, VideoSetup { adapter: Adapter::Svga, ..Default::default() });
    assert_eq!(cpu.bus.read_16(0x0410) & !0x30, floppies);
}
