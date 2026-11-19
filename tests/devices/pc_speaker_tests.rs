//! The PC speaker's level over time: PIT channel 2's output with port 61h
//! bit 1, averaged over each frame of the mix. Programs play digitized
//! sound through it by pulse-width modulation and by switching port 61h,
//! and the mean keeps tones too fast for the mix from folding down into
//! tones a real speaker doesn't make.

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
    // Each frame is one of the two levels, or between them where an edge
    // falls inside it.
    let high = played.iter().filter(|&&s| s > 2900.0).count();
    let low = played.iter().filter(|&&s| s < -2900.0).count();
    assert!(
        high + low > played.len() * 9 / 10,
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
