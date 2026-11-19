//! The AdLib Gold through the bus's ports, as the SDTK's drivers and The
//! Jungle Book's HMI driver program it: the control chip behind the
//! OPL3's second bank, the MMA's DMA playback into the mixer, and its
//! interrupt on the PIC.

use rust_dos::adlibgold::AdlibGoldConfig;
use rust_dos::bus::Bus;
use rust_dos::mixer::{Channel, MixerSettings};
use std::path::PathBuf;

const BASE: u16 = 0x388;

fn bus(gold: bool) -> Bus {
    let mut bus = Bus::new(PathBuf::from("."));
    bus.set_cycles_per_ms(1000);
    bus.io_write(0x21, 0x01);
    bus.io_write(0xA1, 0x00);
    bus.configure_adlib_gold(gold.then(AdlibGoldConfig::default));
    bus
}

/// Let `ms` milliseconds of emulated time pass, running due events.
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

fn peak(bus: &mut Bus) -> u16 {
    bus.audio_catch_up();
    bus.audio_out
        .drain(..)
        .map(|s| s.unsigned_abs())
        .max()
        .unwrap_or(0)
}

fn mma(bus: &mut Bus, ch: u16, reg: u8, value: u8) {
    bus.io_write(BASE + 4, reg);
    bus.io_write(BASE + 5 + ch * 2, value);
}

/// `CtGetGoldCardPresence` and `GetControlRegister` of the SDTK.
fn detect(bus: &mut Bus) -> Option<u8> {
    bus.io_write(BASE + 4, 0x0B);
    bus.io_write(BASE + 5, 0x5A);
    if bus.io_read(BASE + 5) != 0x5A {
        return None;
    }
    bus.io_write(BASE + 2, 0xFF);
    bus.io_write(BASE + 2, 0x09);
    bus.io_write(BASE + 3, 0x54);
    let found = bus.io_read(BASE + 3) & 0x7C == 0x54;
    bus.io_write(BASE + 2, 0x00);
    let id = bus.io_read(BASE + 3);
    bus.io_write(BASE + 2, 0xFE);
    found.then_some(id)
}

#[test]
fn the_sdtk_finds_the_card_and_the_opl3_stays_behind_it() {
    assert_eq!(detect(&mut bus(false)), None, "no card on main's ports");
    let mut bus = bus(true);
    assert_eq!(detect(&mut bus), Some(0x51), "a Gold 1000 with surround");
    // With the control chip deselected, 38Ah/38Bh are the OPL3's second
    // bank again: its timer detection works through 388h as ever.
    bus.io_write(BASE, 0x04);
    bus.io_write(BASE + 1, 0x60);
    bus.io_write(BASE, 0x04);
    bus.io_write(BASE + 1, 0x80);
    assert_eq!(bus.io_read(BASE) & 0xE0, 0);
    bus.io_write(BASE, 0x02);
    bus.io_write(BASE + 1, 0xFF);
    bus.io_write(BASE, 0x04);
    bus.io_write(BASE + 1, 0x21);
    wait_ms(&mut bus, 1.0);
    assert_eq!(bus.io_read(BASE) & 0xE0, 0xC0);
}

/// The Ultrasound latches what is written to 388h whichever card's FM
/// chip answers there: its detection reads the number back at 2XAh.
#[test]
fn the_ultrasound_still_latches_388h_behind_the_card() {
    let mut bus = bus(true);
    assert!(bus.gus.is_some(), "the machine has its Ultrasound at 240h");
    bus.io_write(BASE, 0x5A);
    assert_eq!(bus.io_read(0x24A), 0x5A);
    bus.io_write(BASE, 0xA5);
    assert_eq!(bus.io_read(0x24A), 0xA5);
}

/// The Jungle Book's effects: MMA channel 0 at 11 kHz, 8-bit PCM from an
/// auto-init buffer on DMA 1, which the game follows by reading the
/// channel's count.
#[test]
fn dma_playback_reaches_the_mixer_and_moves_the_count() {
    let mut bus = bus(true);
    for n in 0..0x800u32 {
        bus.ram_mut()[0x20000 + n as usize] = if n & 8 == 0 { 0x60 } else { 0xA0 };
    }
    bus.io_write(0x0A, 0x05);
    bus.io_write(0x0B, 0x59);
    bus.io_write(0x0C, 0x00);
    bus.io_write(0x02, 0x00);
    bus.io_write(0x02, 0x00);
    bus.io_write(0x83, 0x02);
    bus.io_write(0x03, 0xFF);
    bus.io_write(0x03, 0x07);
    bus.io_write(0x0A, 0x01);
    mma(&mut bus, 0, 0x09, 0x80);
    mma(&mut bus, 0, 0x09, 0x00);
    mma(&mut bus, 0, 0x0C, 0x01);
    mma(&mut bus, 0, 0x0A, 0x80);
    mma(&mut bus, 0, 0x09, 0x36);
    mma(&mut bus, 0, 0x09, 0x37);
    let count = |bus: &mut Bus| {
        bus.io_write(0x0C, 0);
        bus.io_read(0x03) as u16 | (bus.io_read(0x03) as u16) << 8
    };
    let start = count(&mut bus);
    assert_eq!(start, 0x7FF - 128, "the FIFO filled at once");
    peak(&mut bus);
    wait_ms(&mut bus, 10.0);
    // 10 ms at 11025 Hz: 110 bytes, give or take the one in play.
    let moved = start - count(&mut bus);
    assert!((109..=111).contains(&moved), "{moved}");
    assert!(peak(&mut bus) > 3000);
    // The card's channel in the mixer.
    let mut settings = MixerSettings::default();
    settings.set_level(Channel::AdlibGold, 0);
    bus.set_mixer(settings);
    wait_ms(&mut bus, 2.0);
    peak(&mut bus);
    wait_ms(&mut bus, 5.0);
    assert_eq!(peak(&mut bus), 0);
}

/// A single-cycle transfer ends with the FIFO running low: the MMA's
/// FIFO interrupt comes up on IRQ 7, and reading its status withdraws it.
#[test]
fn the_fifo_interrupt_reaches_the_pic() {
    let mut bus = bus(true);
    bus.io_write(0x0A, 0x05);
    bus.io_write(0x0B, 0x49);
    bus.io_write(0x0C, 0x00);
    bus.io_write(0x02, 0x00);
    bus.io_write(0x02, 0x00);
    bus.io_write(0x83, 0x02);
    bus.io_write(0x03, 0xFF);
    bus.io_write(0x03, 0x00);
    bus.io_write(0x0A, 0x01);
    mma(&mut bus, 0, 0x0C, 0x01 | 3 << 2);
    mma(&mut bus, 0, 0x0A, 0x80);
    mma(&mut bus, 0, 0x09, 0x37);
    assert_eq!(bus.pic.master.irr & 0x80, 0);
    wait_ms(&mut bus, 30.0);
    assert_ne!(bus.pic.master.irr & 0x80, 0, "IRQ 7 raised");
    assert_ne!(bus.io_read(BASE + 4) & 0x01, 0, "FIF0 in the status");
    bus.audio_catch_up();
    assert_eq!(bus.pic.master.irr & 0x80, 0, "withdrawn by the read");
}

/// The OPL3's timers interrupt on the card's line, as the SDTK's timer
/// driver uses them: timer 1 running unmasked raises IRQ 7 when it runs
/// out, the control chip's status shows the FM source, and the IRQ reset
/// of register 04h withdraws it.
#[test]
fn the_opl3_timers_interrupt_on_the_card_s_line() {
    let mut bus = bus(true);
    bus.io_write(BASE, 0x02);
    bus.io_write(BASE + 1, 0x00);
    bus.io_write(BASE, 0x04);
    bus.io_write(BASE + 1, 0x01);
    wait_ms(&mut bus, 10.0);
    assert_eq!(bus.pic.master.irr & 0x80, 0, "20.48 ms to run out");
    wait_ms(&mut bus, 11.0);
    assert_ne!(bus.pic.master.irr & 0x80, 0, "IRQ 7 raised");
    bus.io_write(BASE + 2, 0xFF);
    assert_eq!(bus.io_read(BASE + 2) & 0x01, 0, "FM in the status");
    bus.io_write(BASE + 2, 0xFE);
    bus.io_write(BASE, 0x04);
    bus.io_write(BASE + 1, 0x80);
    assert_eq!(bus.pic.master.irr & 0x80, 0, "withdrawn by the IRQ reset");
    // Without the card, the FM chip has no interrupt line.
    let mut bus = self::bus(false);
    bus.io_write(BASE, 0x02);
    bus.io_write(BASE + 1, 0x00);
    bus.io_write(BASE, 0x04);
    bus.io_write(BASE + 1, 0x01);
    wait_ms(&mut bus, 25.0);
    assert_eq!(bus.pic.master.irr & 0x80, 0);
}
