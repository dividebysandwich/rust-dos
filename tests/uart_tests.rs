//! The serial ports on the bus (serial/, bus/serial.rs): the BIOS data
//! area, the UART's ports and IRQ at the PICs, a serial mouse as a driver
//! finds it, the modem's commands, and the ports in a save state.

use rust_dos::bus::Bus;
use rust_dos::serial::{PortType, SerialSettings};
use std::path::PathBuf;

const COM1: u16 = 0x3F8;
const COM2: u16 = 0x2F8;

fn bus_with(settings: SerialSettings) -> Bus {
    let mut bus = Bus::new(PathBuf::from("."));
    bus.set_cycles_per_ms(1000);
    bus.configure_serial(&settings);
    bus
}

/// Let `ms` of emulated time pass.
fn pass(bus: &mut Bus, ms: u64) {
    for _ in 0..ms {
        bus.clock.icount += 1000;
        bus.start_batch(bus.clock.icount + 1000);
        bus.service_timers();
    }
}

/// Set a port up at `divisor` with the line format `lcr`.
fn setup(bus: &mut Bus, base: u16, divisor: u16, lcr: u8) {
    bus.io_write(base + 3, 0x80);
    bus.io_write(base, divisor as u8);
    bus.io_write(base + 1, (divisor >> 8) as u8);
    bus.io_write(base + 3, lcr);
}

/// What the port received, read as it comes until nothing more does for
/// 50 ms.
fn read_all(bus: &mut Bus, base: u16) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut quiet = 0;
    while quiet < 50 {
        if bus.io_read(base + 5) & 1 != 0 {
            bytes.push(bus.io_read(base));
            quiet = 0;
        } else {
            pass(bus, 1);
            quiet += 1;
        }
    }
    bytes
}

#[test]
fn the_bios_lists_the_ports() {
    let bus = bus_with(SerialSettings::default());
    assert_eq!(bus.read_16(0x400), COM1);
    assert_eq!(bus.read_16(0x402), COM2);
    assert_eq!(bus.read_16(0x404), 0);
    assert_eq!(bus.read_16(0x410) >> 9 & 7, 2);

    let bus = bus_with(SerialSettings { ports: [PortType::Off; 4], ..Default::default() });
    assert_eq!(bus.read_16(0x400), 0);
    assert_eq!(bus.read_16(0x410) >> 9 & 7, 0);
    // Nothing answers at the ports.
    let mut bus = bus;
    assert_eq!(bus.io_read(COM1 + 5), 0xFF);
}

#[test]
fn loopback_interrupts_on_irq_4() {
    let mut bus = bus_with(SerialSettings::default());
    setup(&mut bus, COM1, 1, 0x03);
    // Loopback, OUT2 for the IRQ, received data interrupts.
    bus.io_write(COM1 + 4, 0x18);
    bus.io_write(COM1 + 1, 0x01);
    bus.pic.master.imr = 0;
    bus.io_write(COM1, 0xA5);
    assert_eq!(bus.pic.master.irr & 0x10, 0);
    pass(&mut bus, 1);
    assert_ne!(bus.pic.master.irr & 0x10, 0, "IRQ 4");
    assert_eq!(bus.io_read(COM1 + 2) & 0x0F, 0x04);
    assert_eq!(bus.io_read(COM1), 0xA5);
    assert_eq!(bus.io_read(COM1 + 2) & 0x0F, 0x01);
}

#[test]
fn a_mouse_driver_finds_the_mouse() {
    let mut bus = bus_with(SerialSettings::default());
    // 1200 baud, 7N1, as a Microsoft mouse driver sets it up.
    setup(&mut bus, COM1, 96, 0x02);
    bus.io_write(COM1 + 4, 0x00);
    pass(&mut bus, 5);
    bus.io_write(COM1 + 4, 0x0B);
    pass(&mut bus, 20);
    assert_eq!(read_all(&mut bus, COM1), b"M");
    // The host mouse moves.
    bus.mouse.set_position(100, 100);
    bus.mouse.set_position(103, 98);
    bus.mouse.button_down(0);
    pass(&mut bus, 40);
    assert_eq!(read_all(&mut bus, COM1), [0x40 | 0x20 | 0x0C, 0x03, 0x3E]);
    assert!(bus.serial.mouse_in_use());
}

#[test]
fn the_modem_answers_at() {
    let mut bus = bus_with(SerialSettings::default());
    setup(&mut bus, COM2, 12, 0x03);
    bus.io_write(COM2 + 4, 0x03);
    for &b in b"ATI3\r" {
        while bus.io_read(COM2 + 5) & 0x20 == 0 {
            pass(&mut bus, 1);
        }
        bus.io_write(COM2, b);
    }
    pass(&mut bus, 60);
    let reply = String::from_utf8(read_all(&mut bus, COM2)).unwrap();
    assert!(reply.starts_with("ATI3\r"), "{:?}", reply);
    assert!(reply.contains("rust-dos modem"), "{:?}", reply);
    assert!(reply.ends_with("OK\r\n"), "{:?}", reply);
    // CTS and DSR: the modem is on; no carrier.
    assert_eq!(bus.io_read(COM2 + 6) & 0xF0, 0x30);
}

#[test]
fn ports_in_a_save_state() {
    use rust_dos::savestate::{Reader, State, Writer};
    let mut bus = bus_with(SerialSettings::default());
    setup(&mut bus, COM2, 3, 0x1B);
    bus.io_write(COM2 + 7, 0x5A);
    let mut w = Writer::new();
    bus.serial.save(&mut w);
    let mut other = bus_with(SerialSettings { ports: [PortType::Off; 4], ..Default::default() });
    other.serial.load(&mut Reader::new(&w.buf)).unwrap();
    assert_eq!(other.io_read(COM2 + 7), 0x5A);
    assert_eq!(other.io_read(COM2 + 3), 0x1B);
    assert_eq!(other.serial.ports[0].as_ref().map(|p| p.backend.kind()), Some(PortType::Mouse));
}
