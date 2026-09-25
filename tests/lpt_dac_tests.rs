//! The DACs on the parallel port: the Covox Speech Thing sounds what is
//! written to 378h; the Disney Sound Source takes bytes into its FIFO on
//! the Select line's rising edges, plays them at 7 kHz and says when the
//! FIFO is full.

use rust_dos::bus::Bus;
use rust_dos::lpt_dac::LptDacType;
use rust_dos::mixer::{Channel, MixerSettings};
use std::path::PathBuf;

fn bus_with(kind: LptDacType) -> Bus {
    let mut bus = Bus::new(PathBuf::from("."));
    bus.set_cycles_per_ms(1000);
    bus.configure_lpt_dac(kind);
    bus
}

fn wait_ms(bus: &mut Bus, ms: f64) {
    bus.clock.icount += (ms * 1000.0) as u64;
}

fn peak(bus: &mut Bus) -> u16 {
    bus.audio_catch_up();
    bus.audio_out.drain(..).map(|s| s.unsigned_abs()).max().unwrap_or(0)
}

/// Put `byte` on the data lines and raise Select, which clocks it into
/// the Disney's FIFO.
fn disney_write(bus: &mut Bus, byte: u8) {
    bus.io_write(0x378, byte);
    bus.io_write(0x37A, 0x0C);
    bus.io_write(0x37A, 0x04);
}

fn fifo_full(bus: &mut Bus) -> bool {
    bus.io_read(0x379) & 0x40 != 0
}

#[test]
fn the_covox_plays_the_data_port() {
    let mut bus = bus_with(LptDacType::Covox);
    wait_ms(&mut bus, 5.0);
    assert_eq!(peak(&mut bus), 0, "silence in the middle of the range");
    bus.io_write(0x378, 0xFF);
    wait_ms(&mut bus, 2.0);
    assert!(peak(&mut bus) > 20000);
    assert_eq!(bus.io_read(0x378), 0xFF);
    // At its volume in the mixer.
    let mut settings = MixerSettings::default();
    settings.set_level(Channel::LptDac, 0);
    bus.set_mixer(settings);
    bus.io_write(0x378, 0x00);
    wait_ms(&mut bus, 2.0);
    bus.audio_catch_up();
    bus.audio_out.clear();
    wait_ms(&mut bus, 2.0);
    assert_eq!(peak(&mut bus), 0);
}

#[test]
fn the_disney_fifo_fills_on_select_edges_and_drains_at_7khz() {
    let mut bus = bus_with(LptDacType::Disney);
    assert!(!fifo_full(&mut bus));
    // It holds 16 bytes; the one it plays is among them.
    for _ in 0..15 {
        disney_write(&mut bus, 0xF0);
    }
    assert!(fifo_full(&mut bus));
    // Without an edge nothing goes in.
    bus.io_write(0x378, 0x00);
    bus.io_write(0x37A, 0x0C);
    // After a millisecond, seven have played and there is room.
    wait_ms(&mut bus, 1.0);
    assert!(!fifo_full(&mut bus));
    assert!(peak(&mut bus) > 10000, "they are heard");
    for _ in 0..7 {
        disney_write(&mut bus, 0xF0);
    }
    assert!(fifo_full(&mut bus));
}

#[test]
fn lpt1_is_in_the_bios_data_area() {
    let mut bus = bus_with(LptDacType::Disney);
    assert_eq!(bus.read_16(0x0408), 0x378);
    assert_eq!(bus.read_16(0x0410) & 0xC000, 0x4000, "one parallel port");
    bus.configure_lpt_dac(LptDacType::None);
    assert_eq!(bus.read_16(0x0408), 0);
    assert_eq!(bus.read_16(0x0410) & 0xC000, 0);
}

#[test]
fn without_a_dac_the_ports_are_open_bus() {
    let mut bus = Bus::new(PathBuf::from("."));
    bus.io_write(0x378, 0x12);
    assert_eq!(bus.io_read(0x378), 0xFF);
    assert_eq!(bus.io_read(0x379), 0xFF);
}
