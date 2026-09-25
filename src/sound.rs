//! Putting the configured sound hardware in place, for the rust-dos
//! program and the browser build alike.

use crate::config::{MidiSynth, SoundConfig};
use crate::cpu::Cpu;
use crate::gus::patch::PatchBank;

/// Install the configured sound hardware and the drive with the built-in
/// Ultrasound software, advertise them in the BLASTER, ULTRASND and
/// ULTRADIR environment variables, and give the MPU-401 its synthesizer.
/// With the configuration in place (`old`), only the parts that changed
/// are replaced, so a resident Ultrasound driver keeps its card when only
/// the Sound Blaster changes. Returns the problems.
pub fn apply_config(cpu: &mut Cpu, sound: &SoundConfig, old: Option<&SoundConfig>) -> Vec<String> {
    let mut warnings = Vec::new();
    // A changed configuration gets the checks the file's got.
    let mut sound = sound.clone();
    let mut old = old.cloned();
    if let Some(old) = &mut old {
        old.check();
        warnings.extend(sound.check().into_iter().map(|w| w.trim_start_matches("[sound]: ").to_string()));
    }
    let changed = |part: &dyn Fn(&SoundConfig) -> String| old.as_ref().is_none_or(|old| part(old) != part(&sound));

    if changed(&|s| format!("{:?} {}", s.card(), s.opl3)) {
        cpu.bus.configure_sound(sound.card(), sound.opl3);
        match &sound.card() {
            Some(sb) => cpu.set_env("BLASTER", &sb.blaster()),
            None => cpu.set_env("BLASTER", ""),
        }
    }
    let gus = sound.ultrasound();
    if changed(&|s| format!("{:?}", s.ultrasound().and_then(|g| g.drive)))
        && let Err(e) = cpu.bus.mount_ultrasnd(gus.as_ref().and_then(|g| g.drive))
    {
        warnings.push(format!("gusdrive: {}", e));
    }
    match &gus {
        Some(g) => {
            cpu.set_env("ULTRASND", &g.ultrasnd());
            cpu.set_env("ULTRADIR", &g.ultradir());
        }
        None => {
            cpu.set_env("ULTRASND", "");
            cpu.set_env("ULTRADIR", "");
        }
    }
    if changed(&|s| format!("{:?}", s.ultrasound().map(|g| (g.base, g.irq, g.dma)))) {
        cpu.bus.configure_gus(gus);
    }
    if changed(&|s| s.lpt_dac.name().to_string()) {
        cpu.bus.configure_lpt_dac(sound.lpt_dac);
    }

    let midi = |s: &SoundConfig| format!("{:?} {:?} {} {}", s.midisynth, s.soundfont, s.gus.builtin(), s.gus.ultradir());
    if !changed(&midi) {
        return warnings;
    }
    cpu.bus.mpu.remove_synth();
    let soundfont = match sound.midisynth {
        MidiSynth::SoundFont => true,
        MidiSynth::Auto => sound.soundfont.is_some(),
        MidiSynth::Gus | MidiSynth::None => false,
    };
    if soundfont {
        match &sound.soundfont {
            Some(path) => match cpu.bus.mpu.load_soundfont(path) {
                Ok(()) => cpu
                    .bus
                    .log_string(&format!("[CONFIG] General MIDI with SoundFont {}", path.display())),
                Err(e) => warnings.push(format!("soundfont: {}", e)),
            },
            None => warnings.push("midisynth=soundfont needs a soundfont setting".to_string()),
        }
    } else if sound.midisynth != MidiSynth::None {
        // The built-in patches whether or not their drive is there.
        let bank = if sound.gus.builtin() {
            Ok((PatchBank::builtin(), "built into rust-dos".to_string()))
        } else {
            PatchBank::from_dos_dir(&cpu.bus.disk, &sound.gus.ultradir()).map(|(bank, dir)| (bank, format!("in {}", dir)))
        };
        match bank {
            Ok((bank, place)) => {
                cpu.bus
                    .log_string(&format!("[CONFIG] General MIDI with the Ultrasound patches {}", place));
                cpu.bus.mpu.load_gus_patches(bank);
            }
            Err(e) if sound.midisynth == MidiSynth::Gus => warnings.push(format!("midisynth=gus: {}", e)),
            Err(e) => cpu.bus.log_string(&format!(
                "[CONFIG] No General MIDI synthesizer (no soundfont, and {})",
                e
            )),
        }
    }
    warnings
}
