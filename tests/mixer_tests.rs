//! The host's mixer: each source at its volume, the master volume on the
//! mix, and the mute, which silences the output device but not the samples
//! handed on for recordings.

use rust_dos::audio::{AudioOutput, pump_audio};
use rust_dos::bus::Bus;
use rust_dos::mixer::{Channel, MixerSettings};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

/// A bus at 1000 instructions per emulated ms.
fn bus() -> Bus {
    let mut bus = Bus::new(PathBuf::from("."));
    bus.set_cycles_per_ms(1000);
    bus
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
    let mut settings = MixerSettings::default();
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
    fn queued_frames(&self) -> usize {
        // Enough that nothing is padded, little enough that all fits.
        rust_dos::opl::RATE as usize / 10
    }

    fn queue(&mut self, samples: &[i16]) -> Result<(), String> {
        self.0.borrow_mut().extend_from_slice(samples);
        Ok(())
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
