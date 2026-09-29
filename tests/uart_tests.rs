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

/// Run `bus` for `ms` of emulated time, letting the network thread work
/// in real time alongside.
fn run_networked(bus: &mut Bus, ms: u64) {
    for _ in 0..ms {
        pass(bus, 1);
        std::thread::sleep(std::time::Duration::from_micros(200));
    }
}

#[test]
fn two_machines_play_over_a_null_modem_and_call_by_modem() {
    use rust_dos::net::tunnel::relay::{RelayConfig, RelayServer};
    use std::time::{Duration, Instant};
    let relay = RelayServer::start("127.0.0.1:0".parse().unwrap(), RelayConfig::default(), Box::new(|_| {})).unwrap();
    let at = relay.local_addr().to_string();
    let cable = SerialSettings { ports: [PortType::Off, PortType::NullModem, PortType::Off, PortType::Off], ..Default::default() };
    let modem = SerialSettings { ports: [PortType::Off, PortType::Modem, PortType::Off, PortType::Off], ..Default::default() };
    let mut a = bus_with(cable.clone());
    let mut b = bus_with(cable);
    let mut c = bus_with(modem.clone());
    for bus in [&mut a, &mut b] {
        setup(bus, COM2, 1, 0x03);
        bus.io_write(COM2 + 4, 0x03);
        bus.net.join(Some(&at), "serial", "").unwrap();
    }
    // Both hear the other is there: CTS, and DSR and DCD from its DTR.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && (a.io_read(COM2 + 6) & 0xB0 != 0xB0 || b.io_read(COM2 + 6) & 0xB0 != 0xB0) {
        run_networked(&mut a, 5);
        run_networked(&mut b, 5);
    }
    assert_eq!(a.io_read(COM2 + 6) & 0xB0, 0xB0, "A's lines");
    assert_eq!(b.io_read(COM2 + 6) & 0xB0, 0xB0, "B's lines");

    // What A sends, B receives, in order.
    let sent: Vec<u8> = (0..2000u32).map(|i| (i * 13 % 256) as u8).collect();
    let mut got = Vec::new();
    let mut next = 0;
    let deadline = Instant::now() + Duration::from_secs(10);
    while got.len() < sent.len() && Instant::now() < deadline {
        while next < sent.len() && a.io_read(COM2 + 5) & 0x20 != 0 {
            a.io_write(COM2, sent[next]);
            next += 1;
        }
        run_networked(&mut a, 1);
        run_networked(&mut b, 1);
        while b.io_read(COM2 + 5) & 1 != 0 {
            got.push(b.io_read(COM2));
        }
    }
    assert_eq!(got, sent);

    // A third machine in the room is left out.
    c.net.join(Some(&at), "serial", "").unwrap();
    let mut notices = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && !notices.iter().any(|n: &String| n.contains("only the first two")) {
        run_networked(&mut c, 5);
        notices.extend(c.net.take_notices());
    }
    assert!(notices.iter().any(|n| n.contains("only the first two")), "C is told: {:?}", notices);
    assert_eq!(c.io_read(COM2 + 6) & 0x80, 0);

    // B leaves; C moves up and its modem is linked to A.
    b.net.leave();
    let mut a2 = bus_with(modem.clone());
    drop(a);
    setup(&mut a2, COM2, 12, 0x03);
    a2.io_write(COM2 + 4, 0x03);
    a2.net.join(Some(&at), "serial", "").unwrap();
    setup(&mut c, COM2, 12, 0x03);
    c.io_write(COM2 + 4, 0x03);
    let type_line = |bus: &mut Bus, line: &str| {
        for &byte in line.as_bytes() {
            while bus.io_read(COM2 + 5) & 0x20 == 0 {
                run_networked(bus, 1);
            }
            bus.io_write(COM2, byte);
        }
    };
    let read_line = |bus: &mut Bus, other: &mut Bus, want: &str| {
        let mut text = String::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !text.contains(want) && Instant::now() < deadline {
            run_networked(bus, 2);
            run_networked(other, 2);
            while bus.io_read(COM2 + 5) & 1 != 0 {
                text.push(bus.io_read(COM2) as char);
            }
        }
        text
    };
    // Wait for the pair to link, then C dials any number.
    let up = |bus: &Bus| bus.net.serial_status()["up"] == true;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && !(up(&c) && up(&a2)) {
        run_networked(&mut c, 5);
        run_networked(&mut a2, 5);
    }
    assert!(up(&c) && up(&a2), "{} {}", c.net.serial_status(), a2.net.serial_status());
    type_line(&mut c, "ATDT5551234\r");
    let rang = read_line(&mut a2, &mut c, "RING\r\n");
    assert!(rang.contains("RING"), "{:?}", rang);
    type_line(&mut a2, "ATA\r");
    let answered = read_line(&mut a2, &mut c, "CONNECT 9600\r\n");
    assert!(answered.contains("CONNECT 9600"), "{:?}", answered);
    let connected = read_line(&mut c, &mut a2, "CONNECT 9600\r\n");
    assert!(connected.contains("CONNECT 9600"), "{:?}", connected);
    type_line(&mut c, "hello");
    let heard = read_line(&mut a2, &mut c, "hello");
    assert!(heard.ends_with("hello"), "{:?}", heard);
}

#[test]
fn the_modem_dials_a_host_and_takes_calls() {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::{Duration, Instant};
    let free_port = || TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let listen = free_port();
    let settings = SerialSettings {
        ports: [PortType::Off, PortType::Modem, PortType::Off, PortType::Off],
        modem_listen: Some(listen),
        ..Default::default()
    };
    let mut bus = bus_with(settings);
    setup(&mut bus, COM2, 12, 0x03);
    bus.io_write(COM2 + 4, 0x03);
    let type_line = |bus: &mut Bus, line: &str| {
        for &byte in line.as_bytes() {
            while bus.io_read(COM2 + 5) & 0x20 == 0 {
                run_networked(bus, 1);
            }
            bus.io_write(COM2, byte);
        }
    };
    let read_until = |bus: &mut Bus, want: &str| {
        let mut text = String::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !text.contains(want) && Instant::now() < deadline {
            run_networked(bus, 2);
            while bus.io_read(COM2 + 5) & 1 != 0 {
                text.push(bus.io_read(COM2) as char);
            }
        }
        text
    };

    // Dialing out: a host that greets and echoes.
    let host = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = host.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = host.accept().unwrap();
        stream.write_all(b"welcome").unwrap();
        let mut buf = [0u8; 5];
        stream.read_exact(&mut buf).unwrap();
        buf
    });
    type_line(&mut bus, "ATE0\r");
    read_until(&mut bus, "OK\r\n");
    type_line(&mut bus, &format!("ATDT127.0.0.1:{}\r", port));
    let text = read_until(&mut bus, "welcome");
    assert!(text.contains("CONNECT 9600\r\nwelcome"), "{:?}", text);
    assert_ne!(bus.io_read(COM2 + 6) & 0x80, 0, "DCD");
    type_line(&mut bus, "howdy");
    run_networked(&mut bus, 20);
    assert_eq!(&server.join().unwrap(), b"howdy");
    // The host hangs up.
    let text = read_until(&mut bus, "NO CARRIER");
    assert!(text.contains("NO CARRIER"), "{:?}", text);
    assert_eq!(bus.io_read(COM2 + 6) & 0x80, 0, "no DCD");

    // A call comes in.
    let mut caller = None;
    let deadline = Instant::now() + Duration::from_secs(5);
    while caller.is_none() && Instant::now() < deadline {
        caller = TcpStream::connect(("127.0.0.1", listen)).ok();
        run_networked(&mut bus, 10);
    }
    let mut caller = caller.expect("the modem takes calls");
    let text = read_until(&mut bus, "RING");
    assert!(text.contains("RING"), "{:?}", text);
    type_line(&mut bus, "ATA\r");
    let text = read_until(&mut bus, "CONNECT");
    assert!(text.contains("CONNECT"), "{:?}", text);
    caller.write_all(b"hi there").unwrap();
    let text = read_until(&mut bus, "hi there");
    assert!(text.ends_with("hi there"), "{:?}", text);
}
