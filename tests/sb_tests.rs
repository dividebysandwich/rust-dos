//! Sound hardware through its ports, the way drivers see it: the Sound
//! Blaster's DSP and mixer, DMA transfers timed in emulated time, the DMA
//! controllers, the OPL's detection timers and the MPU-401.

use rust_dos::bus::Bus;
use rust_dos::sb::{SbConfig, SbModel};
use std::path::PathBuf;

/// A bus at 1000 instructions per emulated ms with the given card.
fn bus_with(config: SbConfig) -> Bus {
    let mut bus = Bus::new(PathBuf::from("."));
    bus.set_cycles_per_ms(1000);
    bus.configure_sound(Some(config), true);
    // Unmask everything but the timer, so the card's IRQ reaches the
    // PIC's output.
    bus.io_write(0x21, 0x01);
    bus.io_write(0xA1, 0x00);
    bus
}

fn sb16() -> Bus {
    bus_with(SbConfig::default())
}

/// Let `ms` milliseconds of emulated time pass, running due timer events.
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

fn dsp_write(bus: &mut Bus, value: u8) {
    assert_eq!(bus.io_read(0x22C) & 0x80, 0, "DSP ready for a byte");
    bus.io_write(0x22C, value);
}

fn dsp_read(bus: &mut Bus) -> u8 {
    assert_eq!(bus.io_read(0x22E) & 0x80, 0x80, "DSP has a byte");
    bus.io_read(0x22A)
}

fn reset_dsp(bus: &mut Bus) {
    bus.io_write(0x226, 1);
    bus.io_write(0x226, 0);
    assert_eq!(dsp_read(bus), 0xAA);
}

/// Program 8-bit DMA channel 1 for a transfer of `len` bytes at `addr`.
fn program_dma1(bus: &mut Bus, addr: u32, len: u16, auto_init: bool) {
    bus.io_write(0x0A, 0x05); // mask channel 1
    bus.io_write(0x0C, 0x00); // clear the flip-flop
    bus.io_write(0x0B, 0x49 | if auto_init { 0x10 } else { 0 }); // single, read, ch 1
    bus.io_write(0x02, addr as u8);
    bus.io_write(0x02, (addr >> 8) as u8);
    bus.io_write(0x83, (addr >> 16) as u8);
    bus.io_write(0x03, (len - 1) as u8);
    bus.io_write(0x03, ((len - 1) >> 8) as u8);
    bus.io_write(0x0A, 0x01); // unmask
}

fn dma1_count(bus: &mut Bus) -> u16 {
    bus.io_write(0x0C, 0);
    let lo = bus.io_read(0x03) as u16;
    lo | (bus.io_read(0x03) as u16) << 8
}

#[test]
fn dsp_reset_version_and_identification() {
    let mut bus = sb16();
    reset_dsp(&mut bus);
    dsp_write(&mut bus, 0xE1);
    assert_eq!((dsp_read(&mut bus), dsp_read(&mut bus)), (4, 5));
    dsp_write(&mut bus, 0xE0);
    dsp_write(&mut bus, 0x55);
    assert_eq!(dsp_read(&mut bus), 0xAA);
    dsp_write(&mut bus, 0xE4);
    dsp_write(&mut bus, 0x3C);
    dsp_write(&mut bus, 0xE8);
    assert_eq!(dsp_read(&mut bus), 0x3C);

    let mut bus = bus_with(SbConfig { model: SbModel::Sb2, ..SbConfig::default() });
    reset_dsp(&mut bus);
    dsp_write(&mut bus, 0xE1);
    assert_eq!((dsp_read(&mut bus), dsp_read(&mut bus)), (2, 1));
}

#[test]
fn single_cycle_transfer_plays_in_emulated_time_and_interrupts() {
    let mut bus = sb16();
    reset_dsp(&mut bus);
    // 1000 bytes of a ramp at 10000 Hz: a tenth of a second.
    let data: Vec<u8> = (0..1000u32).map(|i| (i % 256) as u8).collect();
    bus.load_bytes(0x20000, &data);
    program_dma1(&mut bus, 0x20000, 1000, false);
    dsp_write(&mut bus, 0x40);
    dsp_write(&mut bus, 156); // 1e6 / (256 - 156) = 10000 Hz
    dsp_write(&mut bus, 0x14);
    dsp_write(&mut bus, (999 & 0xFF) as u8);
    dsp_write(&mut bus, (999 >> 8) as u8);

    wait_ms(&mut bus, 50.0);
    let left = dma1_count(&mut bus);
    assert!((495..=505).contains(&left), "half played: count {}", left);
    assert_eq!(bus.pic_pending_irq(), None);

    wait_ms(&mut bus, 51.0);
    assert_eq!(bus.pic_pending_irq(), Some(7), "block end raises IRQ 7");
    // The DMA controller saw terminal count; the status read clears it.
    assert_eq!(bus.io_read(0x08) & 0x02, 0x02);
    assert_eq!(bus.io_read(0x08) & 0x02, 0x00);
    // The mixer says it's the 8-bit interrupt; reading 22Eh acknowledges.
    bus.io_write(0x224, 0x82);
    assert_eq!(bus.io_read(0x225) & 0x03, 0x01);
    bus.io_read(0x22E);
    assert_eq!(bus.pic_pending_irq(), None);

    // The ramp was mixed into the output: 0.1 s of it at 44.1 kHz.
    bus.audio_catch_up();
    let loud = bus.audio_out.iter().step_by(2).filter(|s| s.unsigned_abs() > 8000).count();
    assert!(loud > 1000, "{} loud frames", loud);
}

#[test]
fn sb16_auto_init_16bit_stereo_interrupts_every_block() {
    let mut bus = sb16();
    reset_dsp(&mut bus);
    // DMA 5 (16-bit): words, page register 8Bh, address in words.
    let addr: u32 = 0x40000;
    bus.io_write(0xD4, 0x05); // mask channel 5
    bus.io_write(0xD8, 0x00);
    bus.io_write(0xD6, 0x59); // single, auto-init, read, ch 5
    bus.io_write(0xC4, (addr >> 1) as u8);
    bus.io_write(0xC4, (addr >> 9) as u8);
    bus.io_write(0x8B, (addr >> 16) as u8);
    bus.io_write(0xC6, 0xFF); // 1024 words
    bus.io_write(0xC6, 0x03);
    bus.io_write(0xD4, 0x01);
    // 22050 Hz stereo, signed 16-bit, blocks of 512 samples (256 frames).
    dsp_write(&mut bus, 0x41);
    dsp_write(&mut bus, (22050u16 >> 8) as u8);
    dsp_write(&mut bus, 22050u16 as u8);
    dsp_write(&mut bus, 0xB6);
    dsp_write(&mut bus, 0x30);
    dsp_write(&mut bus, 0xFF);
    dsp_write(&mut bus, 0x01);

    let mut irqs = 0;
    // One block is 256 frames: 11.6 ms. Count interrupts over 100 ms.
    for _ in 0..1000 {
        wait_ms(&mut bus, 0.1);
        if bus.pic_pending_irq() == Some(7) {
            irqs += 1;
            bus.io_write(0x224, 0x82);
            assert_eq!(bus.io_read(0x225) & 0x03, 0x02, "16-bit interrupt");
            bus.io_read(0x22F);
        }
    }
    assert!((8..=9).contains(&irqs), "{} interrupts", irqs);
}

#[test]
fn mixer_reports_the_configured_resources() {
    let mut bus = bus_with(SbConfig { irq: 5, dma8: 1, dma16: 7, ..SbConfig::default() });
    bus.io_write(0x224, 0x80);
    assert_eq!(bus.io_read(0x225), 0x02);
    bus.io_write(0x224, 0x81);
    assert_eq!(bus.io_read(0x225), 0x82);
    assert_eq!(SbConfig::default().blaster(), "A220 I7 D1 H5 P330 T6");
}

#[test]
fn force_irq_is_level_triggered_until_acknowledged() {
    let mut bus = sb16();
    reset_dsp(&mut bus);
    dsp_write(&mut bus, 0xF2);
    assert_eq!(bus.pic_pending_irq(), Some(7));
    bus.io_read(0x22E);
    assert_eq!(bus.pic_pending_irq(), None);
}

#[test]
fn opl_status_tells_opl2_from_opl3_and_times_its_timers() {
    for opl3 in [true, false] {
        let mut bus = Bus::new(PathBuf::from("."));
        bus.set_cycles_per_ms(1000);
        bus.configure_sound(Some(SbConfig::default()), opl3);
        let write = |bus: &mut Bus, reg: u8, value: u8| {
            bus.io_write(0x388, reg);
            bus.io_write(0x389, value);
        };
        write(&mut bus, 0x04, 0x60);
        write(&mut bus, 0x04, 0x80);
        assert_eq!(bus.io_read(0x388), if opl3 { 0x00 } else { 0x06 });
        write(&mut bus, 0x02, 0xFF);
        write(&mut bus, 0x04, 0x21);
        assert_eq!(bus.io_read(0x388) & 0xE0, 0x00);
        bus.clock.icount += 100; // 100 us
        assert_eq!(bus.io_read(0x388) & 0xE0, 0xC0);
    }
}

#[test]
fn fm_notes_make_sound() {
    let mut bus = sb16();
    let write = |bus: &mut Bus, reg: u8, value: u8| {
        bus.io_write(0x388, reg);
        bus.io_write(0x389, value);
    };
    // A plain sine on channel 0: modulator muted, carrier loud.
    for (reg, value) in [(0x20, 0x01), (0x40, 0x3F), (0x60, 0xF0), (0x80, 0x77), (0x23, 0x01), (0x43, 0x00), (0x63, 0xF0), (0x83, 0x77), (0xC0, 0x31), (0xA0, 0x98), (0xB0, 0x31)] {
        write(&mut bus, reg, value);
    }
    wait_ms(&mut bus, 20.0);
    bus.audio_catch_up();
    let peak = bus.audio_out.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
    assert!(peak > 1000, "peak {}", peak);
}

#[test]
fn mpu401_acknowledges_reset_and_uart_mode() {
    let mut bus = Bus::new(PathBuf::from("."));
    assert_eq!(bus.io_read(0x331) & 0x80, 0x80, "nothing to read");
    bus.io_write(0x331, 0xFF);
    assert_eq!(bus.io_read(0x331) & 0x80, 0x00);
    assert_eq!(bus.io_read(0x330), 0xFE);
    bus.io_write(0x331, 0x3F);
    assert_eq!(bus.io_read(0x330), 0xFE);
    // MIDI bytes are accepted.
    for b in [0x90, 60, 100, 0x80, 60, 0] {
        bus.io_write(0x330, b);
    }
    assert_eq!(bus.io_read(0x331) & 0xC0, 0x80);
}

#[test]
fn no_card_means_nothing_at_its_ports() {
    let mut bus = Bus::new(PathBuf::from("."));
    bus.configure_sound(None, false);
    bus.io_write(0x226, 1);
    bus.io_write(0x226, 0);
    assert_eq!(bus.io_read(0x22E), 0xFF);
    assert_eq!(bus.io_read(0x22A), 0xFF);
}
