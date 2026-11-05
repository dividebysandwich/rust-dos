//! The Tandy's and PCjr's SN76489 at port C0h (`tandy`): its tones and
//! volumes as the mixer hears them, and where it is.

use rust_dos::bus::Bus;
use rust_dos::mixer::{Channel, MixerSettings};
use rust_dos::sn76489::TandySound;
use rust_dos::video::adapter::{Adapter, VideoSetup};
use rust_dos::video::bios;
use std::path::PathBuf;

/// A bus of `adapter` at 1000 instructions an emulated ms.
fn bus(adapter: Adapter) -> Bus {
    let mut bus = Bus::new(PathBuf::from("."));
    bus.set_cycles_per_ms(1000);
    bios::install(&mut bus, VideoSetup { adapter, ..Default::default() });
    bus
}

/// The left channel's samples of the next `ms` milliseconds.
fn samples(bus: &mut Bus, ms: u64) -> Vec<i16> {
    let mut out = Vec::new();
    for _ in 0..ms.div_ceil(100) {
        bus.clock.icount += 100 * 1000;
        bus.audio_catch_up();
        out.extend(bus.audio_out.drain(..).step_by(2));
    }
    out
}

/// Tone channel 0 at period `period`, volume `attenuation` (0 loudest).
fn tone(bus: &mut Bus, period: u16, attenuation: u8) {
    bus.io_write(0xC0, 0x80 | (period & 0x0F) as u8);
    bus.io_write(0xC0, (period >> 4) as u8 & 0x3F);
    bus.io_write(0xC0, 0x90 | attenuation);
}

fn peak(samples: &[i16]) -> u16 {
    samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0)
}

#[test]
fn a_tone_at_the_period_s_frequency() {
    for adapter in [Adapter::Tandy, Adapter::Pcjr] {
        let mut bus = bus(adapter);
        assert!(samples(&mut bus, 200).iter().all(|&s| s.abs() < 8), "silent from the start");
        // 3.579545 MHz / 32 / 254: 440 Hz.
        tone(&mut bus, 254, 0);
        samples(&mut bus, 100);
        let one_second = samples(&mut bus, 1000);
        let crossings = one_second.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count();
        assert!((860..=900).contains(&crossings), "{:?}: {} zero crossings", adapter, crossings);
        let loud = peak(&one_second);
        assert!(loud > 3000, "{}", loud);
        // 2 dB a step: 10 steps are 20 dB, a tenth as loud.
        tone(&mut bus, 254, 10);
        samples(&mut bus, 100);
        let quiet = peak(&samples(&mut bus, 500));
        assert!((quiet as f32 / loud as f32 - 0.1).abs() < 0.02, "{} / {}", quiet, loud);
        // 15 is off, and so is everything after a program ends.
        tone(&mut bus, 254, 15);
        samples(&mut bus, 100);
        assert!(peak(&samples(&mut bus, 200)) < 8);
        tone(&mut bus, 254, 0);
        bus.reset_sound();
        samples(&mut bus, 100);
        assert!(peak(&samples(&mut bus, 200)) < 8);
    }
}

#[test]
fn white_noise_is_heard() {
    let mut bus = bus(Adapter::Tandy);
    // Noise at the clock / 512, white, loudest.
    bus.io_write(0xC0, 0xE4);
    bus.io_write(0xC0, 0xF0);
    samples(&mut bus, 100);
    let noise = samples(&mut bus, 500);
    assert!(peak(&noise) > 1000);
    // Not a steady tone: the gaps between zero crossings vary.
    let crossings: Vec<usize> =
        noise.windows(2).enumerate().filter(|(_, w)| (w[0] < 0) != (w[1] < 0)).map(|(i, _)| i).collect();
    let gaps: std::collections::HashSet<usize> = crossings.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(gaps.len() > 10, "{:?}", gaps);
}

#[test]
fn other_machines_have_it_only_when_asked() {
    let mut bus = bus(Adapter::Vga);
    assert!(!bus.tandy_sound_enabled());
    tone(&mut bus, 254, 0);
    samples(&mut bus, 100);
    assert!(peak(&samples(&mut bus, 200)) < 8, "C0h is the DMA controller's");

    bus.configure_tandy_sound(TandySound::On);
    tone(&mut bus, 254, 0);
    samples(&mut bus, 100);
    assert!(peak(&samples(&mut bus, 200)) > 3000);

    // The mixer's volume for it.
    let mut settings = MixerSettings::default();
    settings.set_level(Channel::Tandy, 0);
    bus.set_mixer(settings);
    samples(&mut bus, 100);
    assert!(peak(&samples(&mut bus, 200)) < 8);

    let mut bus = bus_tandy_off();
    tone(&mut bus, 254, 0);
    samples(&mut bus, 100);
    assert!(peak(&samples(&mut bus, 200)) < 8);
}

fn bus_tandy_off() -> Bus {
    let mut bus = bus(Adapter::Tandy);
    bus.configure_tandy_sound(TandySound::Off);
    bus
}
