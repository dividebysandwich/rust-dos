//! The PC speaker's level over time: PIT channel 2's output with port 61h
//! bit 1, averaged over each eighth of a frame of the mix and band-limited
//! to the mix's rate. Programs play digitized sound through it by
//! pulse-width modulation and by switching port 61h, and the band limit
//! keeps tones too fast for the mix from folding down into tones a real
//! speaker doesn't make.

use rust_dos::bus::Bus;
use rust_dos::mixer::MixerSettings;
use std::f64::consts::PI;
use std::path::PathBuf;

/// The mix's rate.
const RATE: f64 = 44100.0;

/// A bus at 1000 instructions per emulated ms (one per microsecond), with
/// the speaker's filter on or off.
fn bus(filter: bool) -> Bus {
    let mut bus = Bus::new(PathBuf::from("."));
    bus.set_cycles_per_ms(1000);
    let mut settings = MixerSettings::default();
    settings.speaker_filter = filter;
    bus.set_mixer(settings);
    bus
}

fn wait_us(bus: &mut Bus, us: u64) {
    bus.clock.icount += us;
}

/// The left channel's samples rendered since the last call.
fn samples(bus: &mut Bus) -> Vec<f64> {
    bus.audio_catch_up();
    bus.audio_out
        .drain(..)
        .step_by(2)
        .map(|s| s as f64)
        .collect()
}

/// The amplitude of the `hz` component of `samples`.
fn amplitude(samples: &[f64], hz: f64) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for (n, s) in samples.iter().enumerate() {
        let w = 2.0 * PI * hz * n as f64 / RATE;
        re += s * w.cos();
        im += s * w.sin();
    }
    2.0 * (re * re + im * im).sqrt() / samples.len() as f64
}

/// A 500 Hz sine wave's samples at one every `period_us` microseconds,
/// between 0 and 1.
fn sine_sample(n: u64, period_us: u64) -> f64 {
    let t = (n * period_us) as f64 / 1e6;
    0.5 + 0.4 * (2.0 * PI * 500.0 * t).sin()
}

/// Channel 2's divisor in mode 3, as a tone is set.
fn tone(bus: &mut Bus, divisor: u16) {
    bus.io_write(0x43, 0xB6);
    bus.io_write(0x42, divisor as u8);
    bus.io_write(0x42, (divisor >> 8) as u8);
    bus.io_write(0x61, 0x03);
}

/// Play `ms` of a 500 Hz sine wave the way RealSound does: channel 2 in
/// mode 0 with the speaker on, a count written at every sample (each
/// 64 us, 76 ticks), which holds the output low for that many ticks and
/// then high until the next. The sample is the share of the period the
/// output is high.
fn play_pwm(bus: &mut Bus, ms: u64) {
    const PERIOD_US: u64 = 64;
    const PERIOD_TICKS: f64 = 76.0;
    bus.io_write(0x43, 0x90); // Channel 2, low byte only, mode 0.
    bus.io_write(0x61, 0x03);
    for n in 0..ms * 1000 / PERIOD_US {
        let low = (1.0 - sine_sample(n, PERIOD_US)) * PERIOD_TICKS;
        bus.io_write(0x42, low.round().max(1.0) as u8);
        wait_us(bus, PERIOD_US);
    }
}

#[test]
fn pwm_through_mode_0_reloads_plays_the_samples() {
    let mut bus = bus(true);
    play_pwm(&mut bus, 200);
    samples(&mut bus);
    play_pwm(&mut bus, 400);
    let played = samples(&mut bus);
    // The sine at 0.4 of the speaker's swing of 6000, 2400, through the
    // speaker's filter, which passes 500 Hz at about its level.
    let sine = amplitude(&played, 500.0);
    assert!(sine > 2000.0, "the sine at {sine}");
}

#[test]
fn bit_banging_port_61h_plays_the_samples() {
    // The gate low, so channel 2's output stays high and port 61h bit 1
    // alone moves the speaker: on for the sample's share of each 64 us.
    let mut bus = bus(true);
    bus.io_write(0x61, 0x00);
    let play = |bus: &mut Bus, ms: u64| {
        for n in 0..ms * 1000 / 64 {
            let on = (sine_sample(n, 64) * 64.0).round() as u64;
            bus.io_write(0x61, 0x02);
            wait_us(bus, on);
            bus.io_write(0x61, 0x00);
            wait_us(bus, 64 - on);
        }
    };
    play(&mut bus, 200);
    samples(&mut bus);
    play(&mut bus, 400);
    let played = samples(&mut bus);
    let sine = amplitude(&played, 500.0);
    assert!(sine > 2000.0, "the sine at {sine}");
}

#[test]
fn an_ultrasonic_square_wave_gives_its_mean() {
    // 596 kHz, as Xenon 2 leaves the speaker between its notes, and
    // 398 kHz, high for two ticks of three: steady levels the speaker's
    // filter takes out, with nothing folded down into what is heard.
    for divisor in [2, 3] {
        let mut bus = bus(true);
        tone(&mut bus, divisor);
        wait_us(&mut bus, 500_000);
        samples(&mut bus);
        wait_us(&mut bus, 100_000);
        let peak = samples(&mut bus).iter().fold(0.0f64, |m, s| m.max(s.abs()));
        assert!(peak < 32.0, "divisor {divisor}: peak {peak}");
    }
}

#[test]
fn an_audible_tone_is_unchanged() {
    // 1 kHz with the filter off: the square wave between the speaker's
    // two levels, 6000 apart, whose fundamental is 4/pi of its half
    // swing.
    let mut bus = bus(false);
    tone(&mut bus, 0x04A9);
    wait_us(&mut bus, 1_000_000);
    samples(&mut bus);
    wait_us(&mut bus, 200_000);
    let played = samples(&mut bus);
    let hz = 1_193_182.0 / 0x04A9 as f64;
    let fundamental = amplitude(&played, hz);
    let expected = 4.0 / PI * 3000.0;
    assert!(
        (fundamental / expected - 1.0).abs() < 0.02,
        "{fundamental} against {expected}"
    );
    // The square wave's odd harmonics, a third, a fifth and so on of the
    // fundamental, at their levels up to 15 kHz. A mean over each whole
    // frame took 0.2 dB off the 5 kHz harmonic and 1.6 dB off the 15 kHz
    // one; the band limit passes them all.
    for k in (3..=15).step_by(2) {
        let level = amplitude(&played, k as f64 * hz) * k as f64;
        assert!(
            (level / fundamental - 1.0).abs() < 0.01,
            "harmonic {k} at {level} of {fundamental} for its share"
        );
    }
    // Each frame is one of the two levels, or between them near an edge,
    // which the band limit spreads over about three frames: six frames of
    // the period's 44 at most.
    let high = played.iter().filter(|&&s| s > 2900.0).count();
    let low = played.iter().filter(|&&s| s < -2900.0).count();
    assert!(
        high + low > played.len() * 4 / 5,
        "{high} high and {low} low of {}",
        played.len()
    );
}

#[test]
fn a_tone_above_5_khz_folds_no_tone_down() {
    // A divisor of 80, 14.9 kHz: its third harmonic, at 44.7 kHz, comes
    // within 644 Hz of the mix's rate. Taking one point of the wave a
    // frame folds it down to 644 Hz at a third of the fundamental's
    // level; the frames' means leave under a hundredth of it.
    let mut bus = bus(false);
    tone(&mut bus, 80);
    wait_us(&mut bus, 1_000_000);
    samples(&mut bus);
    wait_us(&mut bus, 500_000);
    let played = samples(&mut bus);
    let hz = 1_193_182.0 / 80.0;
    let fundamental = amplitude(&played, hz);
    let folded = amplitude(&played, 3.0 * hz - RATE);
    assert!(fundamental > 2000.0, "the tone at {fundamental}");
    assert!(
        folded < fundamental / 100.0,
        "{folded} folded, the tone at {fundamental}"
    );
}

#[test]
fn a_pwm_carriers_harmonics_fold_no_tone_down() {
    // RealSound's way, playing 1 kHz: a mode 0 count written at every
    // sample, the samples 60 us apart (a 16.7 kHz carrier, about Mean
    // Streets' 16.57 kHz) or 36 us (27.8 kHz). Each harmonic of the
    // carrier above half the mix's rate that a frame's mean would fold
    // into the audible range comes out under a three-hundredth of the
    // tone: the 16.7 kHz carrier's second and third (10.8 and 5.9 kHz,
    // Mean Streets' 11.0 and 5.6 kHz of issue 1454), the 27.8 kHz
    // carrier's first and third (16.3 and 4.9 kHz). A carrier below half
    // the mix's rate stays: it is a tone a speaker plays.
    for period_us in [60u64, 36] {
        let mut bus = bus(false);
        bus.io_write(0x43, 0x90);
        bus.io_write(0x61, 0x03);
        let ticks = period_us as f64 * 1.193_182;
        let play = |bus: &mut Bus, ms: u64| {
            for n in 0..ms * 1000 / period_us {
                let t = (n * period_us) as f64 / 1e6;
                let level = 0.5 + 0.4 * (2.0 * PI * 1000.0 * t).sin();
                let low = (1.0 - level) * ticks;
                bus.io_write(0x42, low.round().max(1.0) as u8);
                wait_us(bus, period_us);
            }
        };
        play(&mut bus, 100);
        samples(&mut bus);
        play(&mut bus, 500);
        let played = samples(&mut bus);
        let carrier = 1e6 / period_us as f64;
        let tone = amplitude(&played, 1000.0);
        assert!(tone > 2000.0, "{period_us} us: the tone at {tone}");
        for harmonic in 1..=5 {
            let hz = harmonic as f64 * carrier;
            let folded = (hz - (hz / RATE).round() * RATE).abs();
            if hz < RATE / 2.0 || folded > 20_000.0 {
                continue;
            }
            let level = amplitude(&played, folded);
            assert!(
                level < tone / 300.0,
                "{period_us} us: harmonic {harmonic} at {folded} Hz at {level}, the tone at {tone}"
            );
        }
    }
}

#[test]
fn a_version_10_state_keeps_the_speakers_level() {
    // Version 10 of the CORE section kept the time the speaker was high
    // since the frame being made began; 11 keeps it since the eighth of
    // the frame began. A speaker held high (the data bit with the gate
    // low) for the whole frame loads high for the whole eighth.
    use rust_dos::cpu::Cpu;
    use rust_dos::savestate::machine;
    let machine = || {
        let mut cpu = Cpu::new(PathBuf::from("."));
        cpu.bus.set_cycles_per_ms(1000);
        cpu.load_shell();
        cpu
    };
    let mut a = machine();
    a.bus.io_write(0x61, 0x02);
    wait_us(&mut a.bus, 10_013);
    a.bus.audio_catch_up();
    let until = a.bus.speaker_until;
    let parts = RATE * 8.0 / 1_193_182.0;
    let part = (until * parts).floor();
    let part_start = part / parts;
    let frame_start = (part / 8.0).floor() * 8.0 / parts;
    assert!(until - frame_start > until - part_start + 1.0);
    a.bus.speaker_high = until - frame_start;
    let (mut bytes, labels) = machine::save_parts(&a);
    let core = labels.iter().find(|(n, _)| n == "CORE").unwrap().1.start;
    // The section's version follows its tag.
    bytes[core + 4..core + 6].copy_from_slice(&10u16.to_le_bytes());
    let mut b = machine();
    machine::load(&mut b, &bytes).unwrap();
    let high = b.bus.speaker_high;
    assert!(
        (high - (until - part_start)).abs() < 1e-6,
        "{high} against {}",
        until - part_start
    );
}
