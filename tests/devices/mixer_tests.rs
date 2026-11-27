//! The host's mixer: each source at its volume, the master volume on the
//! mix, the mute, which silences the output device but not the samples
//! handed on for recordings, and the filters and effects.

use rust_dos::adlibgold::AdlibGoldConfig;
use rust_dos::audio::{AudioOutput, pump_audio};
use rust_dos::bus::Bus;
use rust_dos::mixer::{Channel, ChorusPreset, MixerSettings, ReverbPreset, SbFilter};
use rust_dos::sb::{SbConfig, SbModel};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

/// A bus at 1000 instructions per emulated ms, the PC speaker's square
/// wave unfiltered.
fn bus() -> Bus {
    let mut bus = Bus::new(PathBuf::from("."));
    bus.set_cycles_per_ms(1000);
    bus.set_mixer(unfiltered());
    bus
}

fn unfiltered() -> MixerSettings {
    with(|m| m.speaker_filter = false)
}

/// The default settings, changed by `change`.
fn with(change: impl FnOnce(&mut MixerSettings)) -> MixerSettings {
    let mut settings = MixerSettings::default();
    change(&mut settings);
    settings
}

/// No speaker filter, and changed by `change`.
fn unfiltered_with(change: impl FnOnce(&mut MixerSettings)) -> MixerSettings {
    let mut settings = unfiltered();
    change(&mut settings);
    settings
}

fn wait_ms(bus: &mut Bus, ms: u64) {
    bus.clock.icount += ms * 1000;
}

/// Sound the PC speaker at 1 kHz: PIT channel 2 as a square wave, gated to
/// the speaker through port 61h.
fn speaker_on(bus: &mut Bus) {
    bus.io_write(0x43, 0xB6);
    bus.io_write(0x42, 0xA9);
    bus.io_write(0x42, 0x04);
    bus.io_write(0x61, 0x03);
}

/// The speaker on at 1 kHz for a second, its sound thrown away: the
/// square wave's mean level, which the speaker's filter takes out (or,
/// with the filter off, the high-pass that keeps the mix centred), has
/// gone, and the wave swings evenly about 0.
fn speaker_settled(bus: &mut Bus) {
    speaker_on(bus);
    wait_ms(bus, 1000);
    peak(bus);
}

/// Whether two peaks are the same, but for the rounding of a sample and
/// for where the speaker's band-limited edges fall among the frames: the
/// overshoot of an edge peaks between frames, so the loudest sample of a
/// window differs a little from one window to the next (up to 0.5%).
fn near(a: u16, b: u16) -> bool {
    a.abs_diff(b) <= 2.max(b / 200)
}

/// The loudest sample rendered since the last call.
fn peak(bus: &mut Bus) -> u16 {
    bus.audio_catch_up();
    bus.audio_out.drain(..).map(|s| s.unsigned_abs()).max().unwrap_or(0)
}

fn with_level(channel: Channel, percent: u16) -> MixerSettings {
    let mut settings = unfiltered();
    settings.set_level(channel, percent);
    settings
}

#[test]
fn a_source_plays_at_its_volume() {
    let mut bus = bus();
    speaker_settled(&mut bus);
    wait_ms(&mut bus, 20);
    let full = peak(&mut bus);
    assert!(full > 1000, "peak {}", full);

    bus.set_mixer(with_level(Channel::Speaker, 50));
    wait_ms(&mut bus, 20);
    let half = peak(&mut bus);
    assert!(near(half, full / 2), "{half} against {full}");

    // Another source's volume leaves the speaker alone.
    bus.set_mixer(with_level(Channel::Fm, 0));
    wait_ms(&mut bus, 20);
    let same = peak(&mut bus);
    assert!(near(same, full), "{same} against {full}");

    bus.set_mixer(with_level(Channel::Speaker, 200));
    wait_ms(&mut bus, 20);
    let double = peak(&mut bus);
    assert!(near(double, full * 2), "{double} against {full}");
}

#[test]
fn master_is_the_volume_of_the_mix() {
    let mut bus = bus();
    speaker_settled(&mut bus);
    wait_ms(&mut bus, 20);
    let full = peak(&mut bus);

    bus.set_mixer(with_level(Channel::Master, 0));
    wait_ms(&mut bus, 20);
    assert_eq!(peak(&mut bus), 0);

    let mut settings = with_level(Channel::Master, 50);
    settings.set_level(Channel::Speaker, 50);
    bus.set_mixer(settings);
    bus.mixer.take_peaks();
    wait_ms(&mut bus, 20);
    let quarter = peak(&mut bus);
    assert!(near(quarter, full / 4), "{quarter} against {full}");

    // The meters: the speaker at its volume, the mix at the master's.
    let peaks = bus.mixer.take_peaks();
    assert!((peaks[Channel::Speaker as usize] - full as f32 / 2.0 / 32768.0).abs() < 1e-3, "{:?}", peaks);
    assert!((peaks[Channel::Master as usize] - full as f32 / 4.0 / 32768.0).abs() < 1e-3, "{:?}", peaks);
    assert_eq!(peaks[Channel::Fm as usize], 0.0);
}

/// PIT channel 2's divisor, written as Xenon 2 writes it: the mode, then
/// the low and high bytes.
fn speaker_divisor(bus: &mut Bus, divisor: u16) {
    bus.io_write(0x43, 0xB6);
    bus.io_write(0x42, divisor as u8);
    bus.io_write(0x42, (divisor >> 8) as u8);
}

#[test]
fn an_ultrasonic_divisor_silences_the_speaker() {
    // Xenon 2 leaves the speaker gated on and sets a divisor of 2
    // (596 kHz) between its notes: too fast for the cone, which rests at
    // the wave's mean level, and the speaker's filter takes that out.
    // What is left is the frames' means differing by a part of a period,
    // far below a sound.
    let mut bus = bus();
    bus.set_mixer(MixerSettings::default());
    speaker_on(&mut bus);
    speaker_divisor(&mut bus, 2);
    wait_ms(&mut bus, 200);
    peak(&mut bus);
    wait_ms(&mut bus, 20);
    let peak = peak(&mut bus);
    assert!(peak < 32, "peak {peak}");
}

#[test]
fn a_tone_after_an_ultrasonic_divisor_sounds() {
    let mut bus = bus();
    speaker_on(&mut bus);
    // Long enough at 596 kHz to have pushed the wave's phase far out of
    // its period, were it not wrapped.
    speaker_divisor(&mut bus, 2);
    wait_ms(&mut bus, 200);
    peak(&mut bus);
    speaker_divisor(&mut bus, 0x04A9);
    wait_ms(&mut bus, 20);
    // The 1 kHz square wave swings both ways, not a level held on one
    // side of it.
    let tone = samples(&mut bus);
    assert!(tone.iter().any(|&s| s > 1000), "no high half");
    assert!(tone.iter().any(|&s| s < -1000), "no low half");
}

/// An output device that keeps what it is given.
struct Recorder(Rc<RefCell<Vec<i16>>>);

impl AudioOutput for Recorder {
    fn name(&self) -> &str {
        "recorder"
    }

    // Always at its target, so the sound is neither padded, dropped nor
    // resampled.
    fn queued_frames(&self) -> usize {
        rust_dos::opl::RATE as usize / 10
    }

    fn queue(&mut self, samples: &[i16]) -> Result<(), String> {
        self.0.borrow_mut().extend_from_slice(samples);
        Ok(())
    }

    fn target_frames(&self) -> usize {
        rust_dos::opl::RATE as usize / 10
    }
}

#[test]
fn the_mute_silences_the_device_but_not_the_recording() {
    let mut bus = bus();
    let played = Rc::new(RefCell::new(Vec::new()));
    bus.audio_device = Some(Box::new(Recorder(played.clone())));
    speaker_on(&mut bus);

    wait_ms(&mut bus, 20);
    let samples = pump_audio(&mut bus, false);
    assert!(samples.iter().any(|&s| s != 0));
    assert_eq!(*played.borrow(), samples);

    played.borrow_mut().clear();
    bus.mixer.muted = true;
    wait_ms(&mut bus, 20);
    let samples = pump_audio(&mut bus, false);
    assert!(samples.iter().any(|&s| s != 0));
    assert_eq!(played.borrow().len(), samples.len());
    assert!(played.borrow().iter().all(|&s| s == 0));
}

/// The samples rendered since the last call, left and right interleaved.
fn samples(bus: &mut Bus) -> Vec<i16> {
    bus.audio_catch_up();
    bus.audio_out.drain(..).collect()
}

/// The largest change between one sample of a channel and the next.
fn steepest(samples: &[i16]) -> i32 {
    samples.chunks(2).collect::<Vec<_>>().windows(2).map(|w| (w[1][0] as i32 - w[0][0] as i32).abs()).max().unwrap_or(0)
}

#[test]
fn the_speaker_filter_softens_the_square_wave() {
    let mut bus = bus();
    speaker_settled(&mut bus);
    wait_ms(&mut bus, 20);
    // The edges of a square wave, band-limited to the mix's rate: each
    // rises over two or three frames.
    let square = steepest(&samples(&mut bus));
    assert!(square > 4000, "the edges of a square wave: {square}");

    bus.set_mixer(MixerSettings::default());
    wait_ms(&mut bus, 20);
    let filtered = samples(&mut bus);
    assert!(steepest(&filtered) < 3000, "{}", steepest(&filtered));
    assert!(filtered.iter().any(|&s| s.unsigned_abs() > 1000));
}

/// The Sound Blaster `model`'s direct DAC stepping from silence to loud:
/// its first samples, with its output filter or without, and without the
/// capacitor on its output.
fn dac_step(model: SbModel, filter: SbFilter) -> Vec<i16> {
    dac_hold(model, filter, false, 5)
}

/// The left samples of `ms` after the Sound Blaster `model`'s direct DAC
/// steps from silence to loud and holds that, with its output filter or
/// without, and with the capacitor on its output (`sb_coupling`) or
/// without.
fn dac_hold(model: SbModel, filter: SbFilter, coupling: bool, ms: u64) -> Vec<i16> {
    let mut bus = bus();
    bus.set_mixer(unfiltered_with(|m| {
        m.sb_filter = filter;
        m.sb_coupling = coupling;
    }));
    bus.configure_sound(Some(SbConfig { model, ..SbConfig::default() }), true);
    bus.io_write(0x226, 1);
    bus.io_write(0x226, 0);
    let _ = bus.io_read(0x22A);
    bus.io_write(0x22C, 0xD1); // speaker on
    wait_ms(&mut bus, 5);
    samples(&mut bus);
    bus.io_write(0x22C, 0x10);
    bus.io_write(0x22C, 0xF0);
    // In steps, as the window renders: the mixer starts afresh after a
    // gap of over half a second.
    let mut left = Vec::new();
    let mut waited = 0;
    while waited < ms {
        let step = 10.min(ms - waited);
        wait_ms(&mut bus, step);
        waited += step;
        left.extend(samples(&mut bus).chunks(2).map(|frame| frame[0]));
    }
    left
}

#[test]
fn the_sb_filter_follows_the_model() {
    let raw = dac_step(SbModel::SbPro2, SbFilter::Off);
    let loud = *raw.last().unwrap();
    assert!(loud > 10000, "{}", loud);
    assert_eq!(raw[0], loud, "without the filter the step is at once");
    // The SB Pro's 3.2 kHz filter rounds the step off; an SB 2.0's at
    // 4.8 kHz less so.
    let pro = dac_step(SbModel::SbPro2, SbFilter::Auto);
    let sb2 = dac_step(SbModel::Sb2, SbFilter::Auto);
    assert!(pro[0] < loud / 4, "{:?}", &pro[..4]);
    assert!(sb2[0] > pro[0] && sb2[0] < loud / 2, "{:?} {:?}", &sb2[..4], &pro[..4]);
    assert!((pro[200] - loud).abs() < loud / 50, "it settles on the level");
}

/// A level the DAC holds, as ShadowCaster's driver leaves it after its
/// probe of the card (issue 719), fades out of the mix through the
/// capacitor on the card's output: the step is heard, and a second later
/// the output is back at silence. Without the capacitor it stays as a
/// constant offset.
#[test]
fn the_dac_level_held_fades_out_of_the_mix() {
    for model in [SbModel::Sb2, SbModel::SbPro2, SbModel::Sb16] {
        let raw = dac_hold(model, SbFilter::Auto, false, 1000);
        let loud = *raw.last().unwrap();
        assert!(loud > 10000, "{model:?}: {loud}");
        let coupled = dac_hold(model, SbFilter::Auto, true, 1000);
        let peak = *coupled.iter().max().unwrap();
        assert!(peak > loud - loud / 10, "{model:?}: peak {peak}");
        let tenth = coupled[6615];
        assert!(tenth > 0 && tenth < loud / 10, "{model:?}: {tenth}");
        let tail = &coupled[coupled.len() - 4410..];
        assert!(tail.iter().all(|s| s.abs() <= 1), "{model:?}: {tail:?}");
    }
}

/// The left samples of the second after an FM note is stopped the way
/// Covert Action's AdLib driver stops its sounds (issue 1689): the
/// frequency set to 0 and the key released, with a release rate of 0.
/// The phase stops and the envelope stays, so the chip holds its output at
/// a level. With the capacitor on the card's output (`sb_coupling`) or
/// without, on an AdLib or an AdLib Gold.
fn fm_hold(coupling: bool, gold: bool) -> Vec<i16> {
    let mut bus = bus();
    bus.set_mixer(unfiltered_with(|m| m.sb_coupling = coupling));
    bus.configure_adlib_gold(gold.then(AdlibGoldConfig::default));
    let write = |bus: &mut Bus, reg: u8, value: u8| {
        bus.io_write(0x388, reg);
        bus.io_write(0x389, value);
    };
    // A plain sine at full level, attack at once, no decay or release.
    for (reg, value) in [
        (0x20, 0x01),
        (0x40, 0x3F),
        (0x60, 0xF0),
        (0x80, 0x00),
        (0x23, 0x01),
        (0x43, 0x00),
        (0x63, 0xF0),
        (0x83, 0x00),
        (0xC0, 0x30),
        (0xA0, 0x98),
        (0xB0, 0x31),
    ] {
        write(&mut bus, reg, value);
    }
    wait_ms(&mut bus, 7);
    samples(&mut bus);
    write(&mut bus, 0xA0, 0x00);
    write(&mut bus, 0xB0, 0x00);
    let mut left = Vec::new();
    for _ in 0..100 {
        wait_ms(&mut bus, 10);
        left.extend(samples(&mut bus).chunks(2).map(|frame| frame[0]));
    }
    left
}

/// The level the FM chip holds after such a note fades out of the mix
/// through the capacitor on the card's output, as on a real AdLib or
/// Sound Blaster: a second later the output is back at silence. Without
/// the capacitor it stays as a constant offset. An AdLib Gold's output
/// stage passes the level on to the same capacitor.
#[test]
fn the_fm_level_held_fades_out_of_the_mix() {
    for gold in [false, true] {
        let raw = fm_hold(false, gold);
        let held = *raw.last().unwrap();
        assert!(held.unsigned_abs() > 1000, "{held}, gold {gold}");
        let tail = &raw[raw.len() - 4410..];
        assert!(tail.iter().all(|&s| s == held), "the chip holds its level, gold {gold}");
        let coupled = fm_hold(true, gold);
        let tail = &coupled[coupled.len() - 4410..];
        assert!(tail.iter().all(|s| s.abs() <= 1), "gold {gold}: {tail:?}");
    }
}

/// The left samples of the second after the AdLib Gold's MMA plays a
/// FIFO of one level, 6000h, on channel 0 at 11 kHz and runs out of
/// samples: its DAC holds the last one. With the capacitor on the card's
/// output (`sb_coupling`) or without.
fn gold_dac_hold(coupling: bool) -> Vec<i16> {
    let mut bus = bus();
    bus.set_mixer(unfiltered_with(|m| m.sb_coupling = coupling));
    bus.configure_adlib_gold(Some(AdlibGoldConfig::default()));
    let mma = |bus: &mut Bus, reg: u8, value: u8| {
        bus.io_write(0x38C, reg);
        bus.io_write(0x38D, value);
    };
    mma(&mut bus, 0x09, 0x80);
    mma(&mut bus, 0x0C, 0x00);
    mma(&mut bus, 0x0A, 0xFF);
    for _ in 0..128 {
        mma(&mut bus, 0x0B, 0x60);
    }
    // Both outputs, 11 kHz, PCM playback, go.
    mma(&mut bus, 0x09, 0x77);
    let mut left = Vec::new();
    for _ in 0..100 {
        wait_ms(&mut bus, 10);
        left.extend(samples(&mut bus).chunks(2).map(|frame| frame[0]));
    }
    left
}

/// The level the AdLib Gold's DAC holds after the sampled sound stops
/// fades out of the mix through the capacitor on the card's output, as
/// the FM's does: the step is heard, and a second later the output is
/// back at silence. Without the capacitor it stays as a constant offset.
#[test]
fn the_adlib_gold_dac_level_held_fades_out_of_the_mix() {
    let raw = gold_dac_hold(false);
    let held = *raw.last().unwrap();
    assert!(held > 1000, "{held}");
    let tail = &raw[raw.len() - 4410..];
    assert!(tail.iter().all(|&s| s == held), "the DAC holds its level");
    let coupled = gold_dac_hold(true);
    let peak = *coupled.iter().max().unwrap();
    assert!(peak > held - held / 10, "the step is heard: {peak}");
    let tail = &coupled[coupled.len() - 4410..];
    assert!(tail.iter().all(|s| s.abs() <= 1), "{tail:?}");
}

/// An FM note that plays for 50 ms, then the loudness of the 250 ms after
/// it ends, with `settings`.
fn fm_tail(settings: MixerSettings) -> (Vec<i16>, u64) {
    let mut bus = bus();
    bus.set_mixer(settings);
    let write = |bus: &mut Bus, reg: u8, value: u8| {
        bus.io_write(0x388, reg);
        bus.io_write(0x389, value);
    };
    // A plain sine, released fast.
    for (reg, value) in [(0x20, 0x01), (0x40, 0x3F), (0x60, 0xF0), (0x80, 0x7F), (0x23, 0x01), (0x43, 0x00), (0x63, 0xF0), (0x83, 0x7F), (0xC0, 0x31), (0xA0, 0x98), (0xB0, 0x31)] {
        write(&mut bus, reg, value);
    }
    wait_ms(&mut bus, 50);
    let note = samples(&mut bus);
    write(&mut bus, 0xB0, 0x11);
    wait_ms(&mut bus, 50);
    samples(&mut bus);
    wait_ms(&mut bus, 250);
    let tail = samples(&mut bus).iter().map(|s| s.unsigned_abs() as u64).sum();
    (note, tail)
}

#[test]
fn the_reverb_rings_on_after_fm_music() {
    let (_, dry) = fm_tail(unfiltered());
    let (_, wet) = fm_tail(unfiltered_with(|m| m.reverb = ReverbPreset::Large));
    assert!(wet > dry * 10 + 10000, "dry {} wet {}", dry, wet);
}

#[test]
fn the_chorus_changes_fm_but_not_the_speaker() {
    let (dry, _) = fm_tail(unfiltered());
    let (chorus, _) = fm_tail(unfiltered_with(|m| m.chorus = ChorusPreset::Strong));
    assert_ne!(dry, chorus);

    // The speaker sends nothing to the effects.
    let speaker = |settings: MixerSettings| {
        let mut bus = bus();
        bus.set_mixer(settings);
        speaker_on(&mut bus);
        wait_ms(&mut bus, 20);
        samples(&mut bus)
    };
    let effects = unfiltered_with(|m| {
        m.chorus = ChorusPreset::Strong;
        m.reverb = ReverbPreset::Huge;
    });
    assert_eq!(speaker(unfiltered()), speaker(effects));
}

#[test]
fn silence_stays_silent_with_the_filters_and_effects() {
    let mut bus = Bus::new(PathBuf::from("."));
    bus.set_cycles_per_ms(1000);
    bus.set_mixer(with(|m| {
        m.reverb = ReverbPreset::Huge;
        m.chorus = ChorusPreset::Strong;
    }));
    wait_ms(&mut bus, 100);
    assert!(samples(&mut bus).iter().all(|&s| s == 0));
}
