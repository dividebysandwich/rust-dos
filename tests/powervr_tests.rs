//! The PowerVR PCX2: its place on the PCI bus, its windows, and the
//! render handshake Tomb Raider's PowerVR version uses.

use rust_dos::bus::Bus;
use rust_dos::powervr::{Chip, regs};
use std::path::PathBuf;

const REGS: usize = 0xD100_0000;
const TEXTURES: usize = 0xD140_0000;

/// A machine with a PCX2.
fn bus() -> Bus {
    let mut bus = Bus::new(PathBuf::from("."));
    bus.set_cycles_per_ms(1000);
    bus.configure_powervr(Some(Chip::Pcx2));
    bus
}

fn cfg_read(bus: &mut Bus, device: u32, reg: u32) -> u32 {
    bus.io_write_wide(0xCF8, 0x8000_0000 | device << 11 | reg, 4);
    bus.io_read_wide(0xCFC, 4)
}

fn cfg_write(bus: &mut Bus, device: u32, reg: u32, value: u32) {
    bus.io_write_wide(0xCF8, 0x8000_0000 | device << 11 | reg, 4);
    bus.io_write_wide(0xCFC, value, 4);
}

fn w(bus: &mut Bus, reg: usize, value: u32) {
    bus.write_32(REGS + 4 * reg, value);
}

fn r(bus: &Bus, reg: usize) -> u32 {
    bus.read_32(REGS + 4 * reg)
}

#[test]
fn the_card_is_device_2_of_the_pci_bus() {
    let mut bus = bus();
    assert!(bus.pci_present());
    assert_eq!(cfg_read(&mut bus, 2, 0x00), 0x0046_1033, "NEC PCX2");
    assert_eq!(cfg_read(&mut bus, 2, 0x10), 0xD100_0000, "BAR0, the registers");
    assert_eq!(cfg_read(&mut bus, 2, 0x14), 0xD140_0008, "BAR1, the texture memory, prefetchable");
    assert_eq!(cfg_read(&mut bus, 2, 0x3C) & 0xFFFF, 0x010B, "INTA# on IRQ 11");
    assert_eq!(cfg_read(&mut bus, 0, 0x00), 0xFFFF_FFFF, "no 3dfx card");
}

#[test]
fn the_pci_bios_finds_the_card() {
    let mut cpu = rust_dos::cpu::Cpu::new(PathBuf::from("."));
    cpu.bus.configure_powervr(Some(Chip::Pcx2));
    cpu.set_ax(0xB102);
    cpu.set_cx(0x0046);
    cpu.set_dx(0x1033);
    cpu.set_si(0);
    rust_dos::pci::bios(&mut cpu);
    assert_eq!(cpu.get_reg8(iced_x86::Register::AH), 0);
    assert_eq!(cpu.bx(), 2 << 3, "bus 0, device 2, function 0");
}

#[test]
fn bars_size_and_move() {
    let mut bus = bus();
    cfg_write(&mut bus, 2, 0x10, 0xFFFF_FFFF);
    cfg_write(&mut bus, 2, 0x14, 0xFFFF_FFFF);
    assert_eq!(cfg_read(&mut bus, 2, 0x10), 0xFFFF_0000, "64 KB of registers");
    assert_eq!(cfg_read(&mut bus, 2, 0x14), 0xFFC0_0008, "4 MB of texture memory");
    cfg_write(&mut bus, 2, 0x10, 0xE900_0000);
    cfg_write(&mut bus, 2, 0x14, 0xE940_0000);
    w(&mut bus, regs::FOGCOL, 0); // nowhere now
    bus.write_32(0xE900_0000 + 4 * regs::FOGCOL, 0x0112_3456);
    assert_eq!(bus.read_32(0xE900_0000 + 4 * regs::FOGCOL), 0x0112_3456);
    bus.write_16(0xE940_0002, 0xBEEF);
    assert_eq!(bus.read_32(0xE940_0000), 0xBEEF_0000);
    assert_eq!(bus.read_32(TEXTURES), 0xFFFF_FFFF, "nothing at the old place");
}

#[test]
fn texture_memory_takes_every_access_size() {
    let mut bus = bus();
    bus.write_32(TEXTURES + 0x100, 0x4433_2211);
    bus.write_8(TEXTURES + 0x104, 0x55);
    assert_eq!(bus.read_16(TEXTURES + 0x102), 0x4433);
    assert_eq!(bus.read_8(TEXTURES + 0x104), 0x55);
}

#[test]
fn a_render_ends_and_the_reset_pulse_clears_its_status() {
    let mut bus = bus();
    assert_eq!(r(&bus, regs::INTSTATUS) & regs::END_OF_RENDER, 0);
    w(&mut bus, regs::SOFTRESET, 1);
    w(&mut bus, regs::SOFTRESET, 0);
    w(&mut bus, regs::STARTRENDER, 0);
    assert_ne!(r(&bus, regs::INTSTATUS) & regs::END_OF_RENDER, 0);
    w(&mut bus, regs::SOFTRESET, 1);
    assert_eq!(r(&bus, regs::INTSTATUS) & regs::END_OF_RENDER, 0);
}

#[test]
fn the_end_of_a_render_interrupts_when_unmasked() {
    let mut bus = bus();
    w(&mut bus, regs::INTMASK, regs::END_OF_RENDER);
    w(&mut bus, regs::STARTRENDER, 0);
    assert!(bus.pic.busy(11), "IRQ 11 requested");
}

#[test]
fn state_round_trips() {
    let mut bus = bus();
    w(&mut bus, regs::PACKMODE, 0x12);
    bus.write_32(TEXTURES + 0x3F_FFFC, 0xCAFE_F00D);
    let mut w2 = rust_dos::savestate::Writer::new();
    rust_dos::savestate::State::save(bus.powervr.as_ref().unwrap(), &mut w2);
    let bytes = w2.buf;
    let mut other = self::bus();
    let mut reader = rust_dos::savestate::Reader::new(&bytes);
    rust_dos::savestate::State::load(other.powervr.as_mut().unwrap(), &mut reader).unwrap();
    assert_eq!(r(&other, regs::PACKMODE), 0x12);
    assert_eq!(other.read_32(TEXTURES + 0x3F_FFFC), 0xCAFE_F00D);
}
