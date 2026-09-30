//! The Sound Blaster AWE32's EMU8000 through its ports, the way drivers
//! see it: detection, the sample counter, sample RAM, a voice playing,
//! its release, and a save state.

use rust_dos::bus::Bus;
use rust_dos::cpu::Cpu;
use rust_dos::sb::{SbConfig, SbModel};
use rust_dos::savestate::machine;
use std::path::PathBuf;
use std::sync::Arc;

const DATA0: u16 = 0x620;
const DATA1: u16 = 0xA20;
const DATA2: u16 = 0xA22;
const DATA3: u16 = 0xE20;
const POINTER: u16 = 0xE22;

fn awe32() -> SbConfig {
    SbConfig { model: SbModel::Awe32, ..SbConfig::default() }
}

/// A bus with an AWE32 at 220h with 512 KB of RAM and no ROM.
fn bus() -> Bus {
    let mut bus = Bus::new(PathBuf::from("."));
    bus.set_cycles_per_ms(1000);
    bus.set_awe_setup(Arc::from([]), 512);
    bus.configure_sound(Some(awe32()), true);
    bus
}

fn wait_ms(bus: &mut Bus, ms: u64) {
    bus.clock.icount += ms * 1000;
    if bus.clock.icount >= bus.clock.deadline {
        bus.service_timers();
    }
}

fn select(bus: &mut Bus, reg: u16, voice: u16) {
    bus.io_write_wide(POINTER, (reg << 5 | voice) as u32, 2);
}

fn write16(bus: &mut Bus, port: u16, reg: u16, voice: u16, value: u16) {
    select(bus, reg, voice);
    bus.io_write_wide(port, value as u32, 2);
}

fn read16(bus: &mut Bus, port: u16, reg: u16, voice: u16) -> u16 {
    select(bus, reg, voice);
    bus.io_read_wide(port, 2) as u16
}

/// A doubleword register: DATA0's two words, or DATA1 and DATA2.
fn write32(bus: &mut Bus, port: u16, reg: u16, voice: u16, value: u32) {
    select(bus, reg, voice);
    bus.io_write_wide(port, value & 0xFFFF, 2);
    bus.io_write_wide(port + 2, value >> 16, 2);
}

/// Write words to sample memory from `addr` on, through SMALW and SMLD.
fn upload(bus: &mut Bus, addr: u32, words: &[i16]) {
    write32(bus, DATA1, 1, 22, addr);
    select(bus, 1, 26);
    for &w in words {
        bus.io_write_wide(DATA1, w as u16 as u32, 2);
    }
}

/// Read words from `addr` on through SMALR and SMLD, throwing away the
/// word the chip read ahead before, as drivers do.
fn download(bus: &mut Bus, addr: u32, count: usize) -> Vec<u16> {
    write32(bus, DATA1, 1, 20, addr);
    select(bus, 1, 26);
    bus.io_read_wide(DATA1, 2);
    (0..count).map(|_| bus.io_read_wide(DATA1, 2) as u16).collect()
}

/// Play voice 0 over `len` words from `start`, looping, at its pitch.
fn play(bus: &mut Bus, start: u32, len: u32) {
    write16(bus, DATA1, 5, 0, 0x0080);
    write16(bus, DATA1, 4, 0, 0x8000);
    write16(bus, DATA1, 6, 0, 0x8000);
    write16(bus, DATA2, 4, 0, 0x7F7F);
    write16(bus, DATA2, 6, 0, 0x7F7F);
    write16(bus, DATA3, 0, 0, 0xE000);
    write16(bus, DATA3, 1, 0, 0xFF00);
    write32(bus, DATA0, 6, 0, 0x8000_0000 | (start - 1));
    write32(bus, DATA0, 7, 0, start + len - 1);
    write32(bus, DATA1, 0, 0, start - 1);
    // The engine on: the note starts, full volume sustained.
    write16(bus, DATA1, 5, 0, 0x7F7F);
}

/// The output of the next `ms` milliseconds, left channel.
fn sound(bus: &mut Bus, ms: u64) -> Vec<i16> {
    bus.audio_catch_up();
    bus.audio_out.clear();
    wait_ms(bus, ms);
    bus.audio_catch_up();
    bus.audio_out.iter().step_by(2).copied().collect()
}

#[test]
fn the_blaster_variable_names_the_emu8000() {
    assert_eq!(awe32().blaster(), "A220 I7 D1 H5 P330 E620 T6");
    assert!(!SbConfig::default().blaster().contains('E'));
    assert_eq!(SbModel::parse("sbawe"), Some(SbModel::Awe32));
}

#[test]
fn the_dsp_is_a_ct3990s() {
    let mut bus = bus();
    bus.io_write(0x226, 1);
    bus.io_write(0x226, 0);
    wait_ms(&mut bus, 1);
    assert_eq!(bus.io_read(0x22A), 0xAA);
    bus.io_write(0x22C, 0xE1);
    assert_eq!((bus.io_read(0x22A), bus.io_read(0x22A)), (4, 13));
}

#[test]
fn drivers_find_the_chip() {
    let mut bus = bus();
    // Creative's AWEUTIL compares all 16 bits of the ID.
    assert_eq!(read16(&mut bus, DATA3, 7, 0), 0x000C);
    // The Linux and Windows drivers' test of the configuration words.
    assert_eq!(read16(&mut bus, DATA1, 1, 29) & 0x7E, 0x58);
    assert_eq!(read16(&mut bus, DATA1, 1, 30) & 0x03, 0x03);
    // The pointer echoes the selection, its high byte counts reads, and
    // bit 12 goes on and off (Impulse Tracker, Cubic Player).
    select(&mut bus, 3, 5);
    let reads: Vec<u16> = (0..40).map(|_| bus.io_read_wide(POINTER, 2) as u16).collect();
    assert!(reads.iter().all(|r| r & 0xFF == 3 << 5 | 5));
    assert!(reads.windows(2).all(|w| w[0] >> 8 != w[1] >> 8));
    assert!(reads.iter().any(|r| r & 0x1000 != 0) && reads.iter().any(|r| r & 0x1000 == 0));
    // Nothing else answers there with an SB16.
    let mut sb16 = Bus::new(PathBuf::from("."));
    sb16.configure_sound(Some(SbConfig::default()), true);
    assert_eq!(sb16.io_read_wide(DATA3, 2), 0xFFFF);
}

#[test]
fn the_sample_counter_follows_emulated_time() {
    let mut bus = bus();
    let first = read16(&mut bus, DATA2, 1, 27);
    wait_ms(&mut bus, 10);
    let second = bus.io_read_wide(DATA2, 2) as u16;
    let passed = second.wrapping_sub(first);
    assert!((440..=442).contains(&passed), "{} frames in 10 ms", passed);
}

#[test]
fn sample_ram_reads_back_and_wraps_at_its_end() {
    let mut bus = bus();
    let words: Vec<i16> = (0..64).map(|i| (i * 1000 - 20000) as i16).collect();
    upload(&mut bus, 0x20_0100, &words);
    let back = download(&mut bus, 0x20_0100, 64);
    assert_eq!(back, words.iter().map(|&w| w as u16).collect::<Vec<_>>());
    // 512 KB end at 240000h: a write there lands at the start, which is
    // how Cubic Player sizes the RAM.
    upload(&mut bus, 0x20_0000, &[0x1111]);
    upload(&mut bus, 0x24_0000, &[0x2222]);
    assert_eq!(download(&mut bus, 0x20_0000, 1), [0x2222]);
    // Past the RAM and in the missing ROM, memory reads 0.
    assert_eq!(download(&mut bus, 0x24_0000, 1), [0]);
    assert_eq!(download(&mut bus, 0x00_1000, 1), [0]);
}

#[test]
fn dword_accesses_are_two_words() {
    let mut bus = bus();
    select(&mut bus, 6, 3);
    bus.io_write_wide(DATA0, 0x8012_3456, 4);
    assert_eq!(bus.io_read_wide(DATA0, 4), 0x8012_3456);
    assert_eq!(read16(&mut bus, DATA0, 6, 3), 0x3456);
    assert_eq!(bus.io_read_wide(DATA0 + 2, 2), 0x8012);
}

#[test]
fn a_voice_plays_a_looped_sample_at_its_pitch() {
    let mut bus = bus();
    // A square wave of 100 words.
    let wave: Vec<i16> = (0..100).map(|i| if i < 50 { 12000 } else { -12000 }).collect();
    upload(&mut bus, 0x20_0000, &wave);
    play(&mut bus, 0x20_0000, 100);
    let out = sound(&mut bus, 100);
    let tail = &out[out.len() / 2..];
    let loudest = tail.iter().map(|s| s.unsigned_abs()).max().unwrap();
    assert!(loudest > 3000, "loudest {}", loudest);
    // 441 Hz: a rising zero crossing every 100 samples.
    let rising: Vec<usize> = tail.windows(2).enumerate().filter(|(_, w)| w[0] < 0 && w[1] >= 0).map(|(i, _)| i).collect();
    assert!(rising.len() >= 10, "{:?}", rising);
    for w in rising.windows(2) {
        assert!((99..=101).contains(&(w[1] - w[0])), "{:?}", rising);
    }
    // Where it plays, as drivers read it back.
    let addr = read16(&mut bus, DATA1, 0, 0) as u32 | (read16(&mut bus, DATA2, 0, 0) as u32 & 0xFF) << 16;
    assert!((0x1F_FFFF..0x20_0064).contains(&addr), "{:06X}", addr);
}

#[test]
fn full_attenuation_is_silent_and_a_release_fades_out() {
    let mut bus = bus();
    let wave: Vec<i16> = (0..100).map(|i| if i < 50 { 12000 } else { -12000 }).collect();
    upload(&mut bus, 0x20_0000, &wave);
    play(&mut bus, 0x20_0000, 100);
    write16(&mut bus, DATA3, 1, 0, 0xFFFF);
    sound(&mut bus, 20);
    assert!(sound(&mut bus, 20).iter().all(|&s| s.unsigned_abs() < 50));

    write16(&mut bus, DATA3, 1, 0, 0xFF00);
    assert!(sound(&mut bus, 50).iter().any(|&s| s.unsigned_abs() > 3000));
    // Release to silence at the fastest rate.
    write16(&mut bus, DATA1, 5, 0, 0x807F);
    sound(&mut bus, 100);
    assert!(sound(&mut bus, 20).iter().all(|&s| s.unsigned_abs() < 50));
}

#[test]
fn the_card_starts_unmuted_and_aweutil_unmutes_a_muted_chip() {
    let mut bus = bus();
    // The emulated card starts as AWEUTIL /S leaves it: unmuted.
    assert_eq!(read16(&mut bus, DATA1, 1, 29) & 1, 0);
    assert_ne!(read16(&mut bus, DATA1, 1, 30) & 0x40, 0);
    write16(&mut bus, DATA1, 1, 31, 0);
    assert_eq!(read16(&mut bus, DATA1, 1, 30) & 0x40, 0);
    bus.awe_init();
    assert_ne!(read16(&mut bus, DATA1, 1, 30) & 0x40, 0);
}

#[test]
fn programs_ending_stop_the_voices_but_keep_the_ram() {
    let mut bus = bus();
    let wave: Vec<i16> = (0..100).map(|i| if i < 50 { 12000 } else { -12000 }).collect();
    upload(&mut bus, 0x20_0000, &wave);
    play(&mut bus, 0x20_0000, 100);
    assert!(sound(&mut bus, 50).iter().any(|&s| s.unsigned_abs() > 3000));
    bus.reset_sound();
    sound(&mut bus, 20);
    assert!(sound(&mut bus, 20).iter().all(|&s| s.unsigned_abs() < 50));
    assert_eq!(download(&mut bus, 0x20_0000, 2), [12000, 12000]);
}

#[test]
fn a_loaded_machine_plays_on_the_same() {
    let machine_with_awe = || {
        let mut cpu = Cpu::new(PathBuf::from("."));
        cpu.bus.set_cycles_per_ms(1000);
        cpu.bus.set_awe_setup(Arc::from([]), 512);
        cpu.bus.configure_sound(Some(awe32()), true);
        cpu.load_shell();
        cpu
    };
    let mut a = machine_with_awe();
    let wave: Vec<i16> = (0..100).map(|i| (i * 300 - 15000) as i16).collect();
    upload(&mut a.bus, 0x20_0000, &wave);
    play(&mut a.bus, 0x20_0000, 100);
    sound(&mut a.bus, 30);

    let state = machine::save(&a);
    let mut b = machine_with_awe();
    machine::load(&mut b, &state).unwrap();
    machine::load(&mut a, &state).unwrap();
    assert!(machine::save(&b) == state, "a loaded state saves as the same bytes");
    for n in 0..20 {
        let (sa, sb) = (sound(&mut a.bus, 5), sound(&mut b.bus, 5));
        assert!(sa == sb, "the sound differs after {} ms", n * 5);
    }
    assert!(sound(&mut b.bus, 10).iter().any(|&s| s.unsigned_abs() > 3000));

    // A machine with an SB16 doesn't take the state.
    let mut c = Cpu::new(PathBuf::from("."));
    c.bus.configure_sound(Some(SbConfig::default()), true);
    c.load_shell();
    assert!(machine::load(&mut c, &state).is_err());
}

/// The ROM's download: over the network, so only on request
/// (`cargo test -- --ignored`).
#[test]
#[ignore]
fn the_download_gives_the_rom() {
    let dir = std::env::temp_dir().join(format!("rust-dos-awe32-{}", std::process::id()));
    let dest = dir.join("awe32.raw");
    rust_dos::awe32::rom::download(&dest).unwrap();
    let bytes = std::fs::read(&dest).unwrap();
    assert!(rust_dos::awe32::rom::verify(&bytes));
    assert_eq!(rust_dos::awe32::rom::load(&dest).unwrap().len(), rust_dos::awe32::ROM_WORDS);
    std::fs::remove_dir_all(&dir).unwrap();
}
