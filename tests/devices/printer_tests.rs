//! The printer on LPT1: bytes strobed in at the port, the BIOS data area
//! listing the port, the DAC taking its place, and print jobs ending
//! after the timeout.

use rust_dos::bus::Bus;
use rust_dos::lpt_dac::LptDacType;
use rust_dos::printer::{PrinterOutput, PrinterSettings};
use std::path::{Path, PathBuf};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rust-dos-printer-tests-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn bus_with(output: PrinterOutput, dir: &Path, timeout: u32) -> Bus {
    let mut bus = Bus::new(PathBuf::from("."));
    bus.set_cycles_per_ms(1000);
    let settings = PrinterSettings { output, timeout, ..Default::default() };
    bus.configure_printer(&settings, dir);
    bus
}

/// Send `byte` as a program does: wait until the printer isn't busy, put
/// it on the data lines and pulse strobe.
fn print(bus: &mut Bus, byte: u8) {
    while bus.io_read(0x379) & 0x80 == 0 {}
    bus.io_write(0x378, byte);
    bus.io_write(0x37A, 0x0D);
    bus.io_write(0x37A, 0x0C);
}

#[test]
fn bytes_strobed_in_go_to_the_file() {
    let dir = temp_dir("file");
    let mut bus = bus_with(PrinterOutput::File, &dir, 0);
    assert_eq!(bus.read_16(0x0408), 0x378);
    assert_eq!(bus.read_16(0x0410) & 0xC000, 0x4000);
    for &b in b"\x1b@Hello\r\n\x0c" {
        print(&mut bus, b);
    }
    // Acknowledged: the status read after the byte has ACK low.
    print(&mut bus, b'!');
    assert_eq!(bus.io_read(0x379) & 0x40, 0);
    assert_eq!(bus.io_read(0x379) & 0x40, 0x40);
    let notices = bus.finish_printing();
    assert_eq!(notices.len(), 1, "{:?}", notices);
    let path = notices[0].strip_prefix("Printed to ").unwrap();
    assert_eq!(std::fs::read(path).unwrap(), b"\x1b@Hello\r\n\x0c!");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_dac_takes_lpt1() {
    let dir = temp_dir("dac");
    let mut bus = bus_with(PrinterOutput::Pdf, &dir, 0);
    assert!(bus.printer.is_some());
    bus.configure_lpt_dac(LptDacType::Disney);
    assert!(bus.printer.is_none());
    assert_eq!(bus.read_16(0x0408), 0x378);
    bus.configure_lpt_dac(LptDacType::None);
    assert!(bus.printer.is_some());
    let none = PrinterSettings { output: PrinterOutput::None, ..Default::default() };
    bus.configure_printer(&none, &dir);
    assert!(bus.printer.is_none());
    assert_eq!(bus.read_16(0x0408), 0);
    assert_eq!(bus.read_16(0x0410) & 0xC000, 0);
    // Nothing answers at the port.
    assert_eq!(bus.io_read(0x379), 0xFF);
}

#[test]
fn a_job_ends_after_the_timeout() {
    let dir = temp_dir("timeout");
    let mut bus = bus_with(PrinterOutput::Pdf, &dir, 500);
    for &b in b"Page one\r\n" {
        print(&mut bus, b);
    }
    let batch = |bus: &mut Bus, ms: u64| {
        bus.clock.icount += ms * 1000;
        let end = bus.clock.icount + 1000;
        bus.start_batch(end);
    };
    batch(&mut bus, 100);
    assert!(bus.printer.as_ref().unwrap().busy());
    batch(&mut bus, 600);
    let printer = bus.printer.as_mut().unwrap();
    assert!(!printer.busy());
    printer.sync();
    assert_eq!(printer.pages, 1);
    let last = printer.last.clone().unwrap();
    assert!(last.ends_with(".pdf") && Path::new(&last).exists(), "{}", last);
    let _ = std::fs::remove_dir_all(&dir);
}
