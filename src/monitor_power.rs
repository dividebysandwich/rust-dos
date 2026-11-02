//! The monitor's power effects: coming on, syncing to another frequency,
//! and going off as rust-dos quits.

use crate::display::Display;
use crate::video;

/// The monitor coming on (`monitor_effects`): the picture warming up and
/// degaussed, to the high voltage's crackle and the coil's thump.
pub(crate) fn power_on(power: &mut video::power::PowerFx, bus: &mut rust_dos::bus::Bus, effects: video::power::MonitorEffects) {
    use rust_dos::monitor_noise::Sound;
    if effects.picture() {
        power.trigger(video::power::Effect::PowerOn, std::time::Instant::now());
    }
    if effects.sound() {
        bus.audio_catch_up();
        bus.monitor_noise.play(Sound::Relay, 0);
        bus.monitor_noise.play(Sound::Crackle, 60);
        bus.monitor_noise.play(Sound::Degauss, 200);
    }
}

/// The monitor syncing to another frequency: the picture blanks, the relay
/// clicks.
pub(crate) fn resync(power: &mut video::power::PowerFx, bus: &mut rust_dos::bus::Bus, effects: video::power::MonitorEffects) {
    if effects.picture() {
        power.trigger(video::power::Effect::Resync, std::time::Instant::now());
    }
    if effects.sound() {
        bus.audio_catch_up();
        bus.monitor_noise.play(rust_dos::monitor_noise::Sound::Relay, 0);
    }
}

/// The monitor going off as rust-dos quits: `last`, the picture shown
/// last, collapses to a dot, and the supply thunks, straight to the sound
/// device, as the machine no longer runs.
pub(crate) fn power_off(
    power: &mut video::power::PowerFx,
    display: &mut Display,
    last: &video::Frame,
    bus: &mut rust_dos::bus::Bus,
    effects: video::power::MonitorEffects,
) -> Result<(), String> {
    use rust_dos::mixer::Channel;
    let start = std::time::Instant::now();
    if effects.sound() {
        bus.monitor_noise.enabled = true;
        bus.monitor_noise.play(rust_dos::monitor_noise::Sound::PowerOff, 0);
        let gains = bus.mixer.gains();
        let gain = gains[Channel::Monitor.index()] * gains[Channel::Master.index()];
        let frames = bus.monitor_noise.take(rust_dos::opl::RATE as usize / 3);
        let samples: Vec<i16> =
            frames.iter().flat_map(|&s| [(s * gain).clamp(-32768.0, 32767.0) as i16; 2]).collect();
        // Past `pump_audio`, which is where the mute silences the device.
        if let Some(device) = &mut bus.audio_device
            && !bus.mixer.muted
        {
            let _ = device.queue(&samples);
        }
    }
    if !effects.picture() {
        // The sound plays out before the device closes.
        std::thread::sleep(std::time::Duration::from_millis(300));
        return Ok(());
    }
    power.trigger(video::power::Effect::PowerOff, start);
    loop {
        let now = std::time::Instant::now();
        if !power.active(now) {
            return Ok(());
        }
        if let Some(look) = power.look(now) {
            let mut frame = last.clone();
            video::power::apply(&mut frame, &look);
            display.redraw();
            display.present(&mut frame, None, None)?;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
}
