//! PCI cards that share an IRQ: a Voodoo3 and a PowerVR PCX2, both with
//! INTA# on IRQ 11, as the BIOS routes them.

use rust_dos::bus::Bus;
use rust_dos::powervr::{Chip, regs};
use rust_dos::voodoo::{Board, banshee};
use std::path::PathBuf;

const PVR_REGS: usize = 0xD400_0000;

fn bus() -> Bus {
    let mut bus = Bus::new(PathBuf::from("."));
    bus.set_cycles_per_ms(1000);
    bus.configure_voodoo(Some(Board::Voodoo3));
    bus.configure_powervr(Some(Chip::Pcx2));
    bus
}

/// Let `ms` milliseconds of emulated time pass (at 1000 instructions a
/// millisecond).
fn wait_ms(bus: &mut Bus, ms: f64) {
    let target = bus.clock.icount + (ms * 1000.0) as u64;
    while bus.clock.icount < target {
        let step = (target - bus.clock.icount).min(10);
        bus.clock.icount += step;
        if bus.clock.icount >= bus.clock.deadline {
            bus.service_timers();
        }
    }
}

fn crtc_write(bus: &mut Bus, index: u8, value: u8) {
    bus.io_write_wide(0x3D4, index as u32, 1);
    bus.io_write_wide(0x3D5, value as u32, 1);
}

/// Let the Voodoo3 interrupt at the next vertical retrace, and wait for
/// it.
fn banshee_interrupts(bus: &mut Bus) {
    let pci_init0 = banshee::INITIAL_IO as u16 + banshee::PCI_INIT0 as u16;
    let value = bus.io_read_wide(pci_init0, 4) | 1 << 18;
    bus.io_write_wide(pci_init0, value, 4);
    crtc_write(bus, 0x11, 0x10);
    wait_ms(bus, 40.0);
}

/// End a render on the PCX2 with its interrupt unmasked.
fn powervr_interrupts(bus: &mut Bus) {
    bus.write_32(PVR_REGS + 4 * regs::INTMASK, regs::END_OF_RENDER);
    bus.write_32(PVR_REGS + 4 * regs::STARTRENDER, 0);
    wait_ms(bus, 1.0);
}

/// The PCX2's handler reads its status, which drops its line by the
/// card's next register write.
fn powervr_acknowledged(bus: &mut Bus) {
    assert_ne!(
        bus.read_32(PVR_REGS + 4 * regs::INTSTATUS) & regs::END_OF_RENDER,
        0
    );
    bus.write_32(PVR_REGS + 4 * regs::INTMASK, regs::END_OF_RENDER);
}

#[test]
fn a_card_dropping_its_line_leaves_the_other_s_request() {
    let mut bus = bus();
    banshee_interrupts(&mut bus);
    assert!(bus.pic.busy(11), "the Voodoo3 interrupts");
    powervr_interrupts(&mut bus);
    powervr_acknowledged(&mut bus);
    assert!(bus.pic.busy(11), "the Voodoo3 still holds IRQ 11");
    // The Voodoo3's handler clears its interrupt: nobody holds the line.
    crtc_write(&mut bus, 0x11, 0x00);
    assert!(!bus.pic.busy(11));
}

#[test]
fn a_card_interrupting_while_the_other_is_served_interrupts_again() {
    let mut bus = bus();
    banshee_interrupts(&mut bus);
    bus.pic.acknowledge(11);
    // The PCX2 comes up while the Voodoo3's interrupt is in service.
    powervr_interrupts(&mut bus);
    crtc_write(&mut bus, 0x11, 0x00);
    bus.io_write_wide(0xA0, 0x20, 1);
    bus.io_write_wide(0x20, 0x20, 1);
    assert!(bus.pic.busy(11), "the PCX2's request is still there");
    powervr_acknowledged(&mut bus);
}

#[test]
fn the_cards_on_different_lines_keep_to_their_own() {
    let mut bus = bus();
    // The PCX2's interrupt line register, routed to IRQ 10.
    bus.io_write_wide(0xCF8, 0x8000_0000 | 2 << 11 | 0x3C, 4);
    bus.io_write_wide(0xCFC, 10, 4);
    banshee_interrupts(&mut bus);
    powervr_interrupts(&mut bus);
    assert!(bus.pic.busy(10) && bus.pic.busy(11));
    powervr_acknowledged(&mut bus);
    assert!(!bus.pic.busy(10));
    assert!(bus.pic.busy(11));
}
