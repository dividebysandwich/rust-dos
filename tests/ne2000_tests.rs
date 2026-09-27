//! The NE2000 on the bus (net/ne2000.rs, bus/net.rs): its ports as a
//! packet driver uses them, the data port a word at a time, its IRQ at
//! the PICs, its state, and frames between the cards of two machines
//! through a relay on this machine.

use rust_dos::bus::Bus;
use rust_dos::net::frame::{self, Mac};
use rust_dos::net::tunnel::relay::{RelayConfig, RelayServer};
use rust_dos::net::{NetSettings, ne2000::Ne2000};
use rust_dos::savestate::{Reader, State, Writer};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const BASE: u16 = 0x300;
const IRQ: u8 = 10;

fn bus_with_card(mac: Mac) -> Bus {
    let mut bus = Bus::new(PathBuf::from("."));
    bus.set_cycles_per_ms(1000);
    bus.configure_network(&NetSettings { ne2000: true, nic_base: BASE, nic_irq: IRQ, mac: Some(mac), ..Default::default() });
    bus
}

/// Set the card up as a packet driver does: word mode, the receive ring
/// from 4Ch, broadcasts taken, interrupts for frames in and out, its
/// address from the PROM, started.
fn start(bus: &mut Bus) {
    let w = |bus: &mut Bus, offset: u16, value: u8| bus.io_write(BASE + offset, value);
    w(bus, 0x00, 0x21);
    w(bus, 0x0E, 0x49);
    w(bus, 0x01, 0x4C);
    w(bus, 0x02, 0x80);
    w(bus, 0x03, 0x4C);
    w(bus, 0x04, 0x40);
    w(bus, 0x0C, 0x04);
    w(bus, 0x0D, 0x00);
    w(bus, 0x07, 0xFF);
    w(bus, 0x0F, 0x03);
    let prom = read_prom(bus);
    w(bus, 0x00, 0x61);
    for i in 0..6 {
        w(bus, 1 + i, prom[i as usize]);
    }
    w(bus, 0x07, 0x4D);
    w(bus, 0x00, 0x22);
}

/// The card's address from its PROM, read a word at a time.
fn read_prom(bus: &mut Bus) -> [u8; 6] {
    let w = |bus: &mut Bus, offset: u16, value: u8| bus.io_write(BASE + offset, value);
    w(bus, 0x0E, 0x49);
    w(bus, 0x08, 0);
    w(bus, 0x09, 0);
    w(bus, 0x0A, 32);
    w(bus, 0x0B, 0);
    w(bus, 0x00, 0x0A);
    let words: Vec<u32> = (0..16).map(|_| bus.io_read_wide(BASE + 0x10, 2)).collect();
    assert_eq!(words[7], 0x5757, "an NE2000's WW");
    let mut mac = [0; 6];
    for i in 0..6 {
        mac[i] = words[i] as u8;
    }
    mac
}

/// Send `frame` from the card: into its buffer through the data port,
/// then the transmit command.
fn send(bus: &mut Bus, frame: &[u8]) {
    let w = |bus: &mut Bus, offset: u16, value: u8| bus.io_write(BASE + offset, value);
    w(bus, 0x08, 0x00);
    w(bus, 0x09, 0x40);
    w(bus, 0x0A, frame.len() as u8);
    w(bus, 0x0B, (frame.len() >> 8) as u8);
    w(bus, 0x00, 0x12);
    for pair in frame.chunks(2) {
        let word = u16::from_le_bytes([pair[0], *pair.get(1).unwrap_or(&0)]);
        bus.io_write_wide(BASE + 0x10, word as u32, 2);
    }
    w(bus, 0x04, 0x40);
    w(bus, 0x05, frame.len() as u8);
    w(bus, 0x06, (frame.len() >> 8) as u8);
    w(bus, 0x00, 0x26);
}

fn irq_requested(bus: &Bus) -> bool {
    bus.pic.slave.irr & (1 << (IRQ - 8)) != 0
}

#[test]
fn a_packet_driver_finds_the_card_and_its_address() {
    let mac = Mac([0x02, 0x11, 0x22, 0x33, 0x44, 0x55]);
    let mut bus = bus_with_card(mac);
    assert_eq!(read_prom(&mut bus), mac.0);
    // Word reads across the reset port stay on the data port.
    bus.io_write(BASE, 0x21);
    assert_eq!(bus.io_read(BASE + 0x1F), 0);
    assert_eq!(bus.io_read(BASE + 0x07) & 0x80, 0x80, "reset");
    // Sending raises the IRQ once the frame is on the wire.
    start(&mut bus);
    send(&mut bus, &frame::build(Mac::BROADCAST, mac, 0x0800, &[0; 100]));
    assert!(!irq_requested(&bus));
    bus.clock.icount += 10_000;
    bus.start_batch(bus.clock.icount + 1000);
    bus.service_timers();
    assert_eq!(bus.io_read(BASE + 0x07) & 0x02, 0x02, "PTX");
    assert!(irq_requested(&bus));
}

#[test]
fn the_card_keeps_its_state() {
    let mac = Mac([0x02, 0x11, 0x22, 0x33, 0x44, 0x55]);
    let mut bus = bus_with_card(mac);
    start(&mut bus);
    let card = bus.net.nic.clone().unwrap();
    let mut w = Writer::new();
    card.save(&mut w);
    let mut loaded = Ne2000::default();
    loaded.load(&mut Reader::new(&w.buf)).unwrap();
    let mut again = Writer::new();
    loaded.save(&mut again);
    assert_eq!(w.buf, again.buf);
    assert_eq!(loaded.mac, mac);
}

#[test]
fn two_machines_exchange_frames_through_their_cards() {
    let relay = RelayServer::start("127.0.0.1:0".parse().unwrap(), RelayConfig::default(), Box::new(|_| {})).unwrap();
    let at = relay.local_addr().to_string();
    let (ma, mb) = (Mac([0x02, 0, 0, 0, 0, 0xA]), Mac([0x02, 0, 0, 0, 0, 0xB]));
    let mut a = bus_with_card(ma);
    let mut b = bus_with_card(mb);
    for bus in [&mut a, &mut b] {
        start(bus);
        bus.net.join(Some(&at), "test", "").unwrap();
    }
    use rust_dos::net::hub::LanState;
    let joined = |bus: &Bus| matches!(bus.net.status().hub.map(|h| h.lan), Some(LanState::Joined { .. }));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !(joined(&a) && joined(&b)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(joined(&a) && joined(&b));

    // A frame for B's card from A's arrives in B's ring.
    let payload: Vec<u8> = (0..200).map(|i| i as u8).collect();
    let sent = frame::build(mb, ma, 0x0800, &payload);
    send(&mut a, &sent);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !irq_requested(&b) && Instant::now() < deadline {
        b.start_batch(b.clock.icount + 1000);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(irq_requested(&b), "B's card got no frame");
    assert_eq!(b.io_read(BASE + 0x07) & 0x01, 0x01, "PRX");
    // The frame, after its header, in the ring at 4Dh.
    b.io_write(BASE + 0x08, 0x00);
    b.io_write(BASE + 0x09, 0x4D);
    b.io_write(BASE + 0x0A, (sent.len() + 4) as u8);
    b.io_write(BASE + 0x0B, ((sent.len() + 4) >> 8) as u8);
    b.io_write(BASE, 0x0A);
    let mut ring = Vec::new();
    for _ in 0..(sent.len() + 4) / 2 {
        ring.extend_from_slice(&(b.io_read_wide(BASE + 0x10, 2) as u16).to_le_bytes());
    }
    assert_eq!(u16::from_le_bytes([ring[2], ring[3]]) as usize, sent.len() + 4);
    assert_eq!(&ring[4..4 + sent.len()], &sent[..]);
}
