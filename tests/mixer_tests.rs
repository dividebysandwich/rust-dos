//! The host's mixer: each source at its volume, the master volume on the
//! mix, the mute, which silences the output device but not the samples
//! handed on for recordings, and the filters and effects.

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
    speaker_on(&mut bus);
    wait_ms(&mut bus, 20);
    let full = peak(&mut bus);
    assert!(full > 1000, "peak {}", full);

    bus.set_mixer(with_level(Channel::Speaker, 50));
    wait_ms(&mut bus, 20);
    assert_eq!(peak(&mut bus), full / 2);

    // Another source's volume leaves the speaker alone.
    bus.set_mixer(with_level(Channel::Fm, 0));
    wait_ms(&mut bus, 20);
    assert_eq!(peak(&mut bus), full);

    bus.set_mixer(with_level(Channel::Speaker, 200));
    wait_ms(&mut bus, 20);
    assert_eq!(peak(&mut bus), full * 2);
}

#[test]
fn master_is_the_volume_of_the_mix() {
    let mut bus = bus();
    speaker_on(&mut bus);
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
    assert_eq!(peak(&mut bus), full / 4);

    // The meters: the speaker at its volume, the mix at the master's.
    let peaks = bus.mixer.take_peaks();
    assert!((peaks[Channel::Speaker as usize] - full as f32 / 2.0 / 32768.0).abs() < 1e-3, "{:?}", peaks);
    assert!((peaks[Channel::Master as usize] - full as f32 / 4.0 / 32768.0).abs() < 1e-3, "{:?}", peaks);
    assert_eq!(peaks[Channel::Fm as usize], 0.0);
}

/// An output device that keeps what it is given.
struct Recorder(Rc<RefCell<Vec<i16>>>);

impl AudioOutput for Recorder {
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
    speaker_on(&mut bus);
    wait_ms(&mut bus, 20);
    let square = steepest(&samples(&mut bus));
    assert_eq!(square, 6000, "the edges of a square wave");

    bus.set_mixer(MixerSettings::default());
    wait_ms(&mut bus, 20);
    let filtered = samples(&mut bus);
    assert!(steepest(&filtered) < 3000, "{}", steepest(&filtered));
    assert!(filtered.iter().any(|&s| s.unsigned_abs() > 1000));
}

/// The Sound Blaster `model`'s direct DAC stepping from silence to loud:
/// its first samples, with its output filter or without.
fn dac_step(model: SbModel, filter: SbFilter) -> Vec<i16> {
    let mut bus = bus();
    bus.set_mixer(unfiltered_with(|m| m.sb_filter = filter));
    bus.configure_sound(Some(SbConfig { model, ..SbConfig::default() }), true);
    bus.io_write(0x226, 1);
    bus.io_write(0x226, 0);
    let _ = bus.io_read(0x22A);
    bus.io_write(0x22C, 0xD1); // speaker on
    wait_ms(&mut bus, 5);
    samples(&mut bus);
    bus.io_write(0x22C, 0x10);
    bus.io_write(0x22C, 0xF0);
    wait_ms(&mut bus, 5);
    samples(&mut bus).chunks(2).map(|frame| frame[0]).collect()
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
