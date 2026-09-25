//! The processor, sound hardware, display adapter and expanded and upper
//! memory in place, as the settings ask for them. Changed settings reach
//! them only while no program runs, as one would lose track of the
//! hardware it set up, or when a save state is loaded, which brings its
//! own.

use crate::config::{Settings, SoundConfig};
use crate::cpu::{Cpu, CpuModel};
use crate::video::adapter::VideoSetup;

#[derive(Clone, Debug, PartialEq)]
pub struct Hardware {
    pub cpu: CpuModel,
    pub sound: SoundConfig,
    pub video: VideoSetup,
    /// Expanded memory and upper memory blocks.
    pub memory: (bool, bool),
}

impl Hardware {
    /// The hardware `settings` ask for, as a machine started with them has
    /// it.
    pub fn of(settings: &Settings) -> Self {
        Self {
            cpu: settings.cpu,
            sound: settings.sound.clone(),
            video: settings.video_setup(),
            memory: (settings.ems, settings.umb),
        }
    }

    /// `settings` with this hardware, which differs from theirs while a
    /// change waits for the program running to end.
    pub fn settings(&self, settings: &Settings) -> Settings {
        let mut monochrome = settings.monochrome;
        if settings.video_setup().mono_monitor != self.video.mono_monitor {
            monochrome = if self.video.mono_monitor { crate::video::mono::Monochrome::White } else { Default::default() };
        }
        Settings {
            cpu: self.cpu,
            sound: self.sound.clone(),
            machine: self.video.adapter,
            monochrome,
            ems: self.memory.0,
            umb: self.memory.1,
            ..settings.clone()
        }
    }

    pub fn differs(&self, settings: &Settings) -> bool {
        *self != Self::of(settings)
    }

    /// Put the settings' hardware in place. Returns the problems with it.
    pub fn apply(&mut self, cpu: &mut Cpu, settings: &Settings) -> Vec<String> {
        cpu.model = settings.cpu;
        let mut warnings = if settings.sound != self.sound {
            cpu.bus.log_string("[CONFIG] The sound settings changed");
            crate::sound::apply_config(cpu, &settings.sound, Some(&self.sound))
        } else {
            Vec::new()
        };
        // Another display adapter: its BIOS data, and the text mode it
        // starts in, keeping what the screen shows.
        let setup = settings.video_setup();
        if setup != self.video {
            let monitor = if setup.mono() { "monochrome" } else { "colour" };
            cpu.bus.log_string(&format!(
                "[CONFIG] The display is now {} with a {} monitor",
                setup.adapter.describe(),
                monitor
            ));
            crate::video::bios::switch(cpu, setup);
        }
        if (settings.ems, settings.umb) != self.memory {
            let on_off = |on| if on { "on" } else { "off" };
            cpu.bus.log_string(&format!("[CONFIG] EMS {}, upper memory {}", on_off(settings.ems), on_off(settings.umb)));
            warnings.extend(cpu.set_upper_memory(settings.ems, settings.umb).err());
        }
        *self = Self::of(settings);
        warnings
    }
}
