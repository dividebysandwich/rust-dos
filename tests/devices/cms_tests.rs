//! The Creative Music System's SAA1099s (`sbtype=gb`, `cms`): the Game
//! Blaster's ports and detection chip, the chips in an SB 2.0's sockets
//! next to its DSP, and their sound as the mixer hears it.

use rust_dos::bus::Bus;
use rust_dos::gameblaster::CmsConfig;
use rust_dos::mixer::{Channel, MixerSettings};
use std::path::PathBuf;

/// A bus at 1000 instructions an emulated ms, without a Sound Blaster,
/// with the CMS chips `config` gives.
fn bus(config: Option<CmsConfig>) -> Bus {
    let mut bus = Bus::new(PathBuf::from("."));
    bus.set_cycles_per_ms(1000);
    bus.configure_sound(None, rust_dos::opl::Chip::Opl3);
    bus.configure_cms(config);
    bus
}

fn game_blaster() -> Bus {
    bus(Some(CmsConfig {
        base: 0x220,
        detection: true,
    }))
}

/// The left and right samples of the next `ms` milliseconds.
fn samples(bus: &mut Bus, ms: u64) -> (Vec<i16>, Vec<i16>) {
    let mut out = Vec::new();
    for _ in 0..ms.div_ceil(100) {
        bus.clock.icount += 100 * 1000;
        bus.audio_catch_up();
        out.extend(bus.audio_out.drain(..));
    }
    (
        out.iter().step_by(2).copied().collect(),
        out.iter().skip(1).step_by(2).copied().collect(),
    )
}

fn peak(samples: &[i16]) -> u16 {
    samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0)
}

/// Register `reg` of chip `chip` (0 or 1) of the chips at `base`.
fn set(bus: &mut Bus, base: u16, chip: u16, reg: u8, value: u8) {
    bus.io_write(base + 1 + 2 * chip, reg);
    bus.io_write(base + 2 * chip, value);
}

/// Channel 0 of `chip` playing 440 Hz (octave 4, frequency 3) at
/// `amplitude` (right side in the high nibble), and the chip's sound on.
fn tone(bus: &mut Bus, base: u16, chip: u16, amplitude: u8) {
    set(bus, base, chip, 0x00, amplitude);
    set(bus, base, chip, 0x08, 3);
    set(bus, base, chip, 0x10, 0x04);
    set(bus, base, chip, 0x14, 0x01);
    set(bus, base, chip, 0x1C, 0x01);
}

#[test]
fn the_game_blaster_plays_a_tone() {
    let mut bus = game_blaster();
    let (left, right) = samples(&mut bus, 200);
    assert!(
        peak(&left) == 0 && peak(&right) == 0,
        "silent from the start"
    );
    // The first chip on the left, at full amplitude.
    tone(&mut bus, 0x220, 0, 0x0F);
    samples(&mut bus, 300);
    let (left, right) = samples(&mut bus, 1000);
    assert!(peak(&left) > 1500, "{}", peak(&left));
    assert_eq!(peak(&right), 0);
    // 7159090 / (2 * (511 - 3) * 16): 440.4 Hz.
    let crossings = left.windows(2).filter(|w| w[0] < 0 && w[1] >= 0).count();
    assert!(
        (435..=445).contains(&crossings),
        "{} rising zero crossings",
        crossings
    );
    // The second chip on the right.
    tone(&mut bus, 0x220, 1, 0xF0);
    samples(&mut bus, 300);
    let (_, right) = samples(&mut bus, 500);
    assert!(peak(&right) > 1500);
    // The mixer's volume for it.
    let mut settings = MixerSettings::default();
    settings.set_level(Channel::Cms, 0);
    bus.set_mixer(settings);
    samples(&mut bus, 100);
    assert_eq!(peak(&samples(&mut bus, 200).0), 0);
    bus.set_mixer(MixerSettings::default());
    // A program's end silences it.
    bus.reset_sound();
    samples(&mut bus, 500);
    let (left, right) = samples(&mut bus, 200);
    assert!(peak(&left) < 8 && peak(&right) < 8);
}

#[test]
fn half_the_amplitude_is_half_as_loud() {
    let mut bus = game_blaster();
    tone(&mut bus, 0x220, 0, 0x0E);
    samples(&mut bus, 300);
    let loud = peak(&samples(&mut bus, 500).0) as f32;
    tone(&mut bus, 0x220, 0, 0x07);
    samples(&mut bus, 300);
    let quiet = peak(&samples(&mut bus, 500).0) as f32;
    assert!((quiet / loud - 0.5).abs() < 0.05, "{} / {}", quiet, loud);
}

#[test]
fn noise_is_heard() {
    let mut bus = game_blaster();
    set(&mut bus, 0x220, 0, 0x00, 0x0F);
    set(&mut bus, 0x220, 0, 0x15, 0x01);
    set(&mut bus, 0x220, 0, 0x16, 0x01);
    set(&mut bus, 0x220, 0, 0x1C, 0x01);
    samples(&mut bus, 200);
    let (noise, _) = samples(&mut bus, 500);
    assert!(peak(&noise) > 1000);
    let crossings: Vec<usize> = noise
        .windows(2)
        .enumerate()
        .filter(|(_, w)| (w[0] < 0) != (w[1] < 0))
        .map(|(i, _)| i)
        .collect();
    let gaps: std::collections::HashSet<usize> =
        crossings.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(gaps.len() > 10, "{:?}", gaps);
}

/// Creative's check for the card: base + 4 reads 7Fh, and a byte written
/// to base + 6 reads back at base + 0Ah.
#[test]
fn the_detection_chip_answers() {
    let mut bus = bus(Some(CmsConfig {
        base: 0x240,
        detection: true,
    }));
    assert_eq!(bus.io_read(0x244), 0x7F);
    for value in [0x00, 0x55, 0xAA, 0xC6] {
        bus.io_write(0x246, value);
        assert_eq!(bus.io_read(0x24A), value);
        assert_eq!(bus.io_read(0x24B), value);
    }
    bus.io_write(0x247, 0x12);
    assert_eq!(bus.io_read(0x24A), 0x12);
    // Nothing answers without the card.
    let mut bus = bus_without();
    bus.io_write(0x226, 0x55);
    assert_eq!(bus.io_read(0x22A), 0xFF);
}

fn bus_without() -> Bus {
    bus(None)
}

/// On an SB 2.0 the chips take base + 0 to 3, and the DSP keeps its own
/// ports: a reset at base + 6 gives AAh at base + 0Ah.
#[test]
fn the_sb_2_0_s_chips_leave_its_dsp_alone() {
    let mut bus = Bus::new(PathBuf::from("."));
    bus.set_cycles_per_ms(1000);
    let sb = rust_dos::sb::SbConfig {
        model: rust_dos::sb::SbModel::Sb2,
        ..Default::default()
    };
    bus.configure_sound(Some(sb), rust_dos::opl::Chip::Opl2);
    bus.configure_cms(Some(CmsConfig {
        base: 0x220,
        detection: false,
    }));
    bus.io_write(0x226, 1);
    bus.clock.icount += 10_000;
    bus.io_write(0x226, 0);
    bus.clock.icount += 10_000;
    let mut ready = false;
    for _ in 0..100 {
        bus.clock.icount += 1000;
        if bus.io_read(0x22E) & 0x80 != 0 {
            ready = true;
            break;
        }
    }
    assert!(ready, "the DSP has data after its reset");
    assert_eq!(bus.io_read(0x22A), 0xAA);
    tone(&mut bus, 0x220, 0, 0xFF);
    samples(&mut bus, 300);
    let (left, right) = samples(&mut bus, 500);
    assert!(peak(&left) > 1500 && peak(&right) > 1500);
}
