//! The Gravis Ultrasound through its ports, the way drivers see it: DRAM
//! peek and poke, voice registers, playback on emulated time, voice, timer
//! and DMA interrupts, the IRQ/DMA latches, and DMA uploads.

use rust_dos::bus::Bus;
use rust_dos::cpu::Cpu;
use std::path::PathBuf;

const BASE: u16 = 0x240;
const STATUS: u16 = BASE + 6;
const TIMER_CTRL: u16 = BASE + 8;
const TIMER_DATA: u16 = BASE + 9;
const VOICE: u16 = BASE + 0x102;
const SELECT: u16 = BASE + 0x103;
const DATA_LO: u16 = BASE + 0x104;
const DATA_HI: u16 = BASE + 0x105;
const DRAM: u16 = BASE + 0x107;

/// A bus at 1000 instructions per emulated ms with every IRQ unmasked but
/// the timer's.
fn bus() -> Bus {
    let mut bus = Bus::new(PathBuf::from("."));
    bus.set_cycles_per_ms(1000);
    bus.io_write(0x21, 0x01);
    bus.io_write(0xA1, 0x00);
    bus
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

fn reg16(bus: &mut Bus, reg: u8, value: u16) {
    bus.io_write(SELECT, reg);
    bus.io_write(DATA_LO, value as u8);
    bus.io_write(DATA_HI, (value >> 8) as u8);
}

fn reg8(bus: &mut Bus, reg: u8, value: u8) {
    bus.io_write(SELECT, reg);
    bus.io_write(DATA_HI, value);
}

fn read16(bus: &mut Bus, reg: u8) -> u16 {
    bus.io_write(SELECT, reg);
    let lo = bus.io_read(DATA_LO) as u16;
    lo | (bus.io_read(DATA_HI) as u16) << 8
}

fn read8(bus: &mut Bus, reg: u8) -> u8 {
    bus.io_write(SELECT, reg);
    bus.io_read(DATA_HI)
}

fn dram_at(bus: &mut Bus, addr: u32) {
    reg16(bus, 0x43, addr as u16);
    reg8(bus, 0x44, (addr >> 16) as u8);
}

fn poke(bus: &mut Bus, addr: u32, value: u8) {
    dram_at(bus, addr);
    bus.io_write(DRAM, value);
}

fn peek(bus: &mut Bus, addr: u32) -> u8 {
    dram_at(bus, addr);
    bus.io_read(DRAM)
}

/// An address register pair (high at `reg`, low at `reg + 1`), in whole
/// samples.
fn set_addr(bus: &mut Bus, reg: u8, addr: u32) {
    let pos = addr << 9;
    reg16(bus, reg, (pos >> 16) as u16 & 0x1FFF);
    reg16(bus, reg + 1, pos as u16);
}

fn addr(bus: &mut Bus, reg: u8) -> u32 {
    let hi = read16(bus, reg) as u32;
    let lo = read16(bus, reg + 1) as u32;
    ((hi << 16) | lo) >> 9
}

/// Start `voice` playing samples `start..end` at one sample a frame, at
/// full volume in the centre.
fn play(bus: &mut Bus, voice: u8, start: u32, end: u32, ctrl: u8) {
    bus.io_write(VOICE, voice);
    set_addr(bus, 0x02, start);
    set_addr(bus, 0x04, end);
    set_addr(bus, 0x0A, start);
    reg16(bus, 0x01, 1 << 10);
    reg16(bus, 0x09, 0xFFF0);
    reg8(bus, 0x0C, 7);
    reg8(bus, 0x0D, 0x03);
    reg8(bus, 0x00, ctrl);
}

fn irr(bus: &Bus, irq: u8) -> bool {
    if irq < 8 { bus.pic.master.irr & (1 << irq) != 0 } else { bus.pic.slave.irr & (1 << (irq - 8)) != 0 }
}

/// Program 8-bit DMA channel 3 for `len` bytes at `addr`, unmasked unless
/// `masked`. `write` is a device-to-memory transfer.
fn program_dma3(bus: &mut Bus, addr: u32, len: u16, write: bool, masked: bool) {
    bus.io_write(0x0A, 0x07);
    bus.io_write(0x0C, 0x00);
    bus.io_write(0x0B, if write { 0x47 } else { 0x4B });
    bus.io_write(0x06, addr as u8);
    bus.io_write(0x06, (addr >> 8) as u8);
    bus.io_write(0x82, (addr >> 16) as u8);
    bus.io_write(0x07, (len - 1) as u8);
    bus.io_write(0x07, ((len - 1) >> 8) as u8);
    if !masked {
        bus.io_write(0x0A, 0x03);
    }
}

#[test]
fn detection_through_dram_peek_and_poke() {
    let mut bus = bus();
    reg8(&mut bus, 0x4C, 0x00);
    reg8(&mut bus, 0x4C, 0x01);
    poke(&mut bus, 0, 0xAA);
    poke(&mut bus, 0x100, 0x55);
    assert_eq!(peek(&mut bus, 0), 0xAA);
    assert_eq!(peek(&mut bus, 0x100), 0x55);
    // 1 MB: each 256K bank is separate memory.
    for (i, a) in [0x00000u32, 0x40000, 0x80000, 0xC0000].into_iter().enumerate() {
        poke(&mut bus, a + 1, i as u8 + 1);
    }
    for (i, a) in [0x00000u32, 0x40000, 0x80000, 0xC0000].into_iter().enumerate() {
        assert_eq!(peek(&mut bus, a + 1), i as u8 + 1);
    }
}

#[test]
fn registers_read_back() {
    let mut bus = bus();
    bus.io_write(VOICE, 5);
    assert_eq!(bus.io_read(VOICE), 5);
    reg16(&mut bus, 0x01, 0x1234);
    assert_eq!(read16(&mut bus, 0x81), 0x1234);
    // The high address registers hold 13 bits.
    reg16(&mut bus, 0x02, 0xFFFF);
    assert_eq!(read16(&mut bus, 0x82), 0x1FFF);
    reg16(&mut bus, 0x09, 0xABC0);
    assert_eq!(read16(&mut bus, 0x89), 0xABC0);
    reg8(&mut bus, 0x0C, 0x0F);
    assert_eq!(read8(&mut bus, 0x8C), 0x0F);
    reg8(&mut bus, 0x0E, 31);
    assert_eq!(read8(&mut bus, 0x8E), 0xDF);
    assert_eq!(bus.io_read(SELECT), 0x8E);
    // 3X3h reads back the last byte written to 3X3h-3X5h.
    reg8(&mut bus, 0x0E, 0xCE);
    assert_eq!(bus.io_read(SELECT), 0xCE);
    // The MIDI UART is ready to send and has nothing.
    assert_eq!(bus.io_read(BASE + 0x100), 0x02);
    // Not a GUS MAX.
    assert_eq!(bus.io_read(BASE + 0x106), 0xFF);
}

#[test]
fn selecting_a_register_clears_the_data_latch() {
    let mut bus = bus();
    bus.io_write(SELECT, 0x01);
    bus.io_write(DATA_LO, 0x34);
    // A new selection: only the high byte arrives.
    bus.io_write(SELECT, 0x01);
    bus.io_write(DATA_HI, 0x12);
    assert_eq!(read16(&mut bus, 0x81), 0x1200);
}

#[test]
fn voices_play_at_the_rate_of_the_active_voice_count() {
    let mut bus = bus();
    play(&mut bus, 0, 0, 0xF_0000, 0x00);
    wait_ms(&mut bus, 10.0);
    let pos = addr(&mut bus, 0x8A);
    assert!((440..=442).contains(&pos), "14 voices: {}", pos);

    reg8(&mut bus, 0x0E, 31);
    set_addr(&mut bus, 0x0A, 0);
    wait_ms(&mut bus, 10.0);
    let pos = addr(&mut bus, 0x8A);
    assert!((192..=194).contains(&pos), "32 voices: {}", pos);
}

#[test]
fn a_one_shot_voice_stops_at_its_end() {
    let mut bus = bus();
    play(&mut bus, 0, 100, 200, 0x00);
    wait_ms(&mut bus, 5.0);
    assert_eq!(read8(&mut bus, 0x80) & 0x01, 0x01, "stopped");
    assert_eq!(addr(&mut bus, 0x8A), 200);
}

#[test]
fn volume_and_pan_reach_the_mixer() {
    let mut bus = bus();
    for a in 0..256 {
        poke(&mut bus, a, 0x40);
    }
    play(&mut bus, 0, 0, 255, 0x08);
    reg8(&mut bus, 0x0C, 0);
    bus.audio_catch_up();
    bus.audio_out.clear();
    wait_ms(&mut bus, 5.0);
    bus.audio_catch_up();
    let frames: Vec<(i16, i16)> = bus.audio_out.iter().copied().collect::<Vec<_>>().chunks(2).map(|c| (c[0], c[1])).collect();
    let (l, r) = frames[frames.len() / 2];
    assert!((16000..=16500).contains(&l), "left {}", l);
    assert!(r.abs() < 10, "right {}", r);

    // Without the DAC, nothing.
    reg8(&mut bus, 0x4C, 0x05);
    bus.audio_catch_up();
    wait_ms(&mut bus, 5.0);
    bus.audio_catch_up();
    let last = bus.audio_out.len() - 2;
    assert_eq!(bus.audio_out[last], 0);
}

#[test]
fn wave_irqs_reach_the_pic_and_8f_names_each_voice() {
    let mut bus = bus();
    play(&mut bus, 0, 0, 100, 0x20);
    play(&mut bus, 1, 0, 200, 0x20);
    // 100 frames at 44.1 kHz: 2.27 ms.
    wait_ms(&mut bus, 2.0);
    assert_eq!(bus.io_read(STATUS) & 0x20, 0);
    assert!(!irr(&bus, 5));
    wait_ms(&mut bus, 0.5);
    assert_eq!(bus.io_read(STATUS) & 0x20, 0x20);
    assert!(irr(&bus, 5));
    // Voice 0 has a wave IRQ (bit 7 clear), no volume IRQ (bit 6 set).
    assert_eq!(read8(&mut bus, 0x8F), 0x60);
    assert_eq!(bus.io_read(STATUS) & 0x20, 0);
    assert!(!irr(&bus, 5), "the line dropped");
    assert_eq!(read8(&mut bus, 0x8F) & 0xE0, 0xE0);

    wait_ms(&mut bus, 2.5);
    assert!(irr(&bus, 5));
    assert_eq!(read8(&mut bus, 0x8F), 0x61);
}

#[test]
fn reading_the_low_byte_acknowledges_nothing() {
    let mut bus = bus();
    play(&mut bus, 0, 0, 10, 0x20);
    wait_ms(&mut bus, 1.0);
    bus.io_write(SELECT, 0x8F);
    bus.io_read(DATA_LO);
    assert_eq!(bus.io_read(STATUS) & 0x20, 0x20);
    assert_eq!(bus.io_read(DATA_HI), 0x60);
    assert_eq!(bus.io_read(STATUS) & 0x20, 0);
}

#[test]
fn voice_irqs_need_the_reset_register_enable() {
    let mut bus = bus();
    reg8(&mut bus, 0x4C, 0x03);
    play(&mut bus, 0, 0, 10, 0x20);
    wait_ms(&mut bus, 1.0);
    assert_eq!(bus.io_read(STATUS) & 0x20, 0x20);
    assert!(!irr(&bus, 5));
    reg8(&mut bus, 0x4C, 0x07);
    assert!(irr(&bus, 5));
}

#[test]
fn writing_a0_to_voice_control_sets_a_pending_irq() {
    let mut bus = bus();
    bus.io_write(VOICE, 3);
    reg8(&mut bus, 0x00, 0xA3);
    assert_eq!(read8(&mut bus, 0x80) & 0x80, 0x80);
    assert!(irr(&bus, 5));
    reg8(&mut bus, 0x00, 0x03);
    assert_eq!(read8(&mut bus, 0x80) & 0x80, 0);
    assert_eq!(bus.io_read(STATUS), 0);
}

#[test]
fn timers_count_and_interrupt() {
    let mut bus = bus();
    reg8(&mut bus, 0x46, 0xFF); // timer 1: 80 us
    reg8(&mut bus, 0x47, 0xFE); // timer 2: 640 us
    reg8(&mut bus, 0x45, 0x04); // IRQ from timer 1
    bus.io_write(TIMER_CTRL, 0x04);
    bus.io_write(TIMER_DATA, 0x03);
    wait_ms(&mut bus, 0.07);
    assert_eq!(bus.io_read(STATUS) & 0x04, 0);
    wait_ms(&mut bus, 0.02);
    assert_eq!(bus.io_read(STATUS) & 0x04, 0x04);
    assert!(irr(&bus, 5));
    assert_eq!(bus.io_read(TIMER_CTRL), 0xC4);

    // The driver acknowledges timer 1 by clearing its enable.
    reg8(&mut bus, 0x45, 0x00);
    assert_eq!(bus.io_read(STATUS) & 0x04, 0);
    bus.io_write(TIMER_DATA, 0x80);
    assert_eq!(bus.io_read(TIMER_CTRL) & 0x40, 0);

    // Timer 2 reaches 640 us after it started, without interrupting.
    wait_ms(&mut bus, 0.6);
    assert_eq!(bus.io_read(TIMER_CTRL) & 0x20, 0x20);
    assert_eq!(bus.io_read(STATUS) & 0x08, 0);
}

#[test]
fn a_running_timer_sets_the_deadline() {
    let mut bus = bus();
    bus.start_batch(u64::MAX);
    let idle = bus.clock.deadline;
    reg8(&mut bus, 0x46, 0x00); // 256 * 80 us
    reg8(&mut bus, 0x45, 0x04);
    bus.io_write(TIMER_DATA, 0x01);
    let pit = rust_dos::timer::PIT_HZ as u128;
    let start_ns = bus.clock.now_ticks() as u128 * 1_000_000_000 / pit;
    let due_ticks = ((start_ns + 20_480_000) * pit).div_ceil(1_000_000_000) as u64;
    let due = bus.clock.icount_at(due_ticks);
    assert!(bus.clock.deadline < idle);
    assert!(bus.clock.deadline.abs_diff(due) <= 2, "{} vs {}", bus.clock.deadline, due);
}

#[test]
fn dma_uploads_to_dram_and_interrupts_at_terminal_count() {
    let mut bus = bus();
    let data: Vec<u8> = (0..=255).collect();
    bus.load_bytes(0x2_0000, &data);
    program_dma3(&mut bus, 0x2_0000, 256, false, false);
    reg16(&mut bus, 0x42, 0x1000 >> 4);
    reg8(&mut bus, 0x41, 0x21);
    // 256 bytes at 650 KB/s take about 0.4 ms.
    wait_ms(&mut bus, 0.2);
    assert_eq!(bus.io_read(STATUS) & 0x80, 0);
    wait_ms(&mut bus, 0.4);
    assert_eq!(bus.io_read(STATUS) & 0x80, 0x80);
    assert!(irr(&bus, 5));
    for a in [0u32, 1, 128, 255] {
        assert_eq!(peek(&mut bus, 0x1000 + a), a as u8);
    }
    // The 8237 reached terminal count and masked the channel.
    assert_eq!(bus.io_read(0x08) & 0x08, 0x08);
    assert_eq!(bus.io_read(0x0F) & 0x08, 0x08);
    // Reading 41h reports and acknowledges the IRQ; the transfer is over.
    let ctrl = read8(&mut bus, 0x41);
    assert_eq!(ctrl & 0x41, 0x40);
    assert_eq!(bus.io_read(STATUS) & 0x80, 0);
    assert!(!irr(&bus, 5));
}

#[test]
fn dma_converts_unsigned_samples() {
    let mut bus = bus();
    bus.load_bytes(0x2_0000, &[0x00, 0x80, 0xFF, 0x7F]);
    program_dma3(&mut bus, 0x2_0000, 4, false, false);
    reg16(&mut bus, 0x42, 0);
    reg8(&mut bus, 0x41, 0x81); // invert the MSB of 8-bit data
    wait_ms(&mut bus, 0.1);
    assert_eq!([peek(&mut bus, 0), peek(&mut bus, 1), peek(&mut bus, 2), peek(&mut bus, 3)], [0x80, 0x00, 0x7F, 0xFF]);

    program_dma3(&mut bus, 0x2_0000, 4, false, false);
    reg8(&mut bus, 0x41, 0xC1); // 16-bit data: only the high bytes
    wait_ms(&mut bus, 0.1);
    assert_eq!([peek(&mut bus, 0), peek(&mut bus, 1), peek(&mut bus, 2), peek(&mut bus, 3)], [0x00, 0x00, 0xFF, 0xFF]);
}

#[test]
fn dma_waits_for_the_channel_to_be_unmasked() {
    let mut bus = bus();
    bus.load_bytes(0x2_0000, &[0x11; 16]);
    program_dma3(&mut bus, 0x2_0000, 16, false, true);
    reg16(&mut bus, 0x42, 0);
    reg8(&mut bus, 0x41, 0x21);
    wait_ms(&mut bus, 1.0);
    assert_eq!(peek(&mut bus, 0), 0x00);
    bus.io_write(0x0A, 0x03);
    wait_ms(&mut bus, 1.0);
    assert_eq!(peek(&mut bus, 15), 0x11);
    assert_eq!(bus.io_read(STATUS) & 0x80, 0x80);
}

#[test]
fn sixteen_bit_dma_channels_translate_the_address() {
    let mut bus = bus();
    bus.load_bytes(0x2_0000, &[1, 2, 3, 4]);
    // Channel 5: word address, word count.
    bus.io_write(0xD4, 0x05);
    bus.io_write(0xD8, 0x00);
    bus.io_write(0xD6, 0x49);
    bus.io_write(0xC4, 0x00);
    bus.io_write(0xC4, 0x00);
    bus.io_write(0x8B, 0x02);
    bus.io_write(0xC6, 0x01);
    bus.io_write(0xC6, 0x00);
    bus.io_write(0xD4, 0x01);
    // Latch DMA 5 (latch value 3) and upload to DRAM 40200h.
    bus.io_write(BASE, 0x08);
    bus.io_write(BASE + 0x0B, 0x03);
    reg16(&mut bus, 0x42, 0x4010);
    reg8(&mut bus, 0x41, 0x05);
    wait_ms(&mut bus, 0.1);
    assert_eq!([peek(&mut bus, 0x40200), peek(&mut bus, 0x40203)], [1, 4]);
}

#[test]
fn dma_downloads_to_memory() {
    let mut bus = bus();
    for (i, v) in [9u8, 8, 7, 6].into_iter().enumerate() {
        poke(&mut bus, 0x500 + i as u32, v);
    }
    let before = bus.page_gen.clone();
    program_dma3(&mut bus, 0x3_0000, 4, true, false);
    reg16(&mut bus, 0x42, 0x50);
    reg8(&mut bus, 0x41, 0x03);
    wait_ms(&mut bus, 0.1);
    assert_eq!(&bus.ram()[0x3_0000..0x3_0004], &[9, 8, 7, 6]);
    assert_ne!(bus.page_gen, before, "the decode cache sees the write");
}

#[test]
fn the_irq_latch_moves_the_interrupt() {
    let mut bus = bus();
    // IRQ 7 (latch value 4), then a stray 2XBh write that isn't a latch.
    bus.io_write(BASE, 0x48);
    bus.io_write(BASE + 0x0B, 0x04);
    bus.io_write(BASE + 0x0B, 0x02);
    bus.io_write(VOICE, 0);
    reg8(&mut bus, 0x00, 0xA3);
    assert!(irr(&bus, 7));
    assert!(!irr(&bus, 5));
    reg8(&mut bus, 0x00, 0x03);
    assert!(!irr(&bus, 7));

    // A zero field keeps the IRQ; IRQ 2 is IRQ 9 on an AT.
    bus.io_write(BASE, 0x48);
    bus.io_write(BASE + 0x0B, 0x00);
    reg8(&mut bus, 0x00, 0xA3);
    assert!(irr(&bus, 7));
    reg8(&mut bus, 0x00, 0x03);
    bus.io_write(BASE, 0x48);
    bus.io_write(BASE + 0x0B, 0x01);
    reg8(&mut bus, 0x00, 0xA3);
    assert!(irr(&bus, 9));
}

#[test]
fn reset_stops_voices_and_keeps_dram() {
    let mut bus = bus();
    poke(&mut bus, 0x1234, 0x5A);
    play(&mut bus, 0, 0, 0xF_0000, 0x00);
    reg8(&mut bus, 0x4C, 0x00);
    reg8(&mut bus, 0x4C, 0x07);
    bus.io_write(VOICE, 0);
    assert_eq!(read8(&mut bus, 0x80) & 0x01, 0x01);
    assert_eq!(peek(&mut bus, 0x1234), 0x5A);

    // So does a program's exit.
    play(&mut bus, 0, 0, 0xF_0000, 0x00);
    bus.reset_sound();
    bus.io_write(VOICE, 0);
    assert_eq!(read8(&mut bus, 0x80) & 0x01, 0x01);
    assert_eq!(peek(&mut bus, 0x1234), 0x5A);
}

#[test]
fn an_idle_card_needs_no_events() {
    let mut bus = bus();
    let gus = bus.gus.as_ref().unwrap();
    assert_eq!(gus.next_event(&bus.dma), None);
    // After an interrupt the next event is in the future.
    play(&mut bus, 0, 0, 20, 0x28);
    wait_ms(&mut bus, 1.0);
    let now = bus.clock.now_ticks();
    let next = bus.gus.as_ref().unwrap().next_event(&bus.dma).unwrap();
    assert!(next > now);
}

#[test]
fn without_a_card_its_ports_are_open_bus() {
    let mut bus = bus();
    bus.configure_gus(None);
    assert_eq!(bus.io_read(STATUS), 0xFF);
    bus.io_write(VOICE, 3);
    assert_eq!(bus.io_read(VOICE), 0xFF);
}

#[test]
fn programs_find_the_card_and_its_patches_in_the_environment() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    assert_eq!(cpu.get_env("ULTRASND"), Some("240,3,3,5,5"));
    assert_eq!(cpu.get_env("ULTRADIR"), Some("X:\\ULTRASND"));
    assert!(cpu.bus.disk.is_file("X:\\ULTRASND\\MIDI\\ACPIANO.PAT"));

    // Elsewhere, or nowhere.
    cpu.bus.mount_ultrasnd(Some(b'U' - b'A')).unwrap();
    assert!(!cpu.bus.disk.is_mounted(b'X' - b'A'));
    assert!(cpu.bus.disk.is_file("U:\\ULTRASND\\MIDI\\ACPIANO.PAT"));
    cpu.bus.mount_ultrasnd(None).unwrap();
    assert!(!cpu.bus.disk.is_mounted(b'U' - b'A'));
}
