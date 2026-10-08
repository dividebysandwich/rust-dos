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
    /// Expanded memory, upper memory blocks and DOS's tables packed low.
    pub memory: (bool, bool, bool),
    /// The 3dfx card.
    pub voodoo: Option<crate::voodoo::Board>,
    /// The 3dfx card's gamma for Glide.
    pub gamma: Option<f32>,
    /// The guest environment variables the settings ask for.
    pub env: Vec<crate::env_inject::Rule>,
    /// The PowerVR card.
    pub powervr: Option<crate::powervr::Chip>,
    /// The IPX driver and the LAN.
    pub network: crate::net::NetSettings,
    /// The serial ports.
    pub serial: crate::serial::SerialSettings,
    /// The printer, and the capture folder its files go in by default.
    pub printer: (crate::printer::PrinterSettings, std::path::PathBuf),
}

/// Set up a new machine's hardware as `settings` ask for it: the
/// processor, display adapter, 3dfx card, disks, mixer, game port,
/// keyboard layout (with `host_layout` the host keyboard's for auto),
/// expanded and upper memory, DPMI, DOS version, sound cards, network and
/// serial ports. Returns the problems with the settings.
pub fn configure(cpu: &mut Cpu, settings: &Settings, host_layout: &'static crate::keylayout::Layout) -> Vec<String> {
    cpu.model = settings.cpu;
    cpu.core = settings.core;
    cpu.set_fpu_fast(settings.fpu_fast);
    cpu.bus.observe.enabled = settings.idle_skip;
    cpu.bus.idle_hint = settings.idle_hint;
    crate::video::bios::install(&mut cpu.bus, settings.video_setup());
    cpu.bus.configure_voodoo(settings.voodoo.board());
    cpu.set_env_rules(settings.environment());
    crate::voodoo::overlay::provide(&mut cpu.bus);
    cpu.bus.configure_powervr(settings.powervr);
    cpu.bus.set_disk_settings(settings.disk);
    cpu.bus.set_mixer(settings.mixer);
    cpu.bus.set_joystick(settings.joystick);
    cpu.bus.vga.set_composite(settings.composite);
    cpu.bus.kbd.layout = settings.keyboard_layout.layout(host_layout);
    let mut warnings: Vec<String> = cpu.set_dos_high(settings.dos_high).err().into_iter().collect();
    warnings.extend(cpu.set_upper_memory(settings.ems, settings.umb).err());
    cpu.bus.dpmi.enabled = settings.dpmi;
    crate::dos_data::set_version(&mut cpu.bus, settings.dos_version);
    cpu.bus.ide_hard_disks = settings.ide_hard_disks;
    cpu.bus.boot_cdrom = settings.boot_cdrom;
    warnings.extend(crate::sound::apply_config(cpu, &settings.sound, None));
    cpu.bus.configure_network(&settings.network);
    cpu.bus.configure_serial(&settings.serial);
    cpu.bus.configure_printer(&settings.printer, &settings.capture_dir);
    warnings
}

impl Hardware {
    /// The hardware `settings` ask for, as a machine started with them has
    /// it.
    pub fn of(settings: &Settings) -> Self {
        Self {
            cpu: settings.cpu,
            sound: settings.sound.clone(),
            video: settings.video_setup(),
            memory: (settings.ems, settings.umb, settings.dos_high),
            voodoo: settings.voodoo.board(),
            gamma: settings.voodoo.gamma,
            env: settings.environment(),
            powervr: settings.powervr,
            network: settings.network.clone(),
            serial: settings.serial.clone(),
            printer: (settings.printer.clone(), settings.capture_dir.clone()),
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
            dos_high: self.memory.2,
            voodoo: crate::voodoo::VoodooSettings {
                enabled: self.voodoo.is_some(),
                board: self.voodoo.unwrap_or(settings.voodoo.board),
                gamma: self.gamma,
                ..settings.voodoo
            },
            powervr: self.powervr,
            network: self.network.clone(),
            serial: self.serial.clone(),
            printer: self.printer.0.clone(),
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
        let voodoo = settings.voodoo.board();
        if voodoo != self.voodoo {
            match voodoo {
                Some(board) => cpu.bus.log_string(&format!("[CONFIG] A 3dfx Voodoo Graphics with {} MB", board.megabytes())),
                None => cpu.bus.log_string("[CONFIG] No 3dfx card"),
            }
            cpu.bus.configure_voodoo(voodoo);
        }
        let env = settings.environment();
        if env != self.env {
            cpu.set_env_rules(env);
        }
        if settings.powervr != self.powervr {
            match settings.powervr {
                Some(chip) => cpu.bus.log_string(&format!("[CONFIG] A PowerVR {}", chip.name().to_ascii_uppercase())),
                None => cpu.bus.log_string("[CONFIG] No PowerVR card"),
            }
            cpu.bus.configure_powervr(settings.powervr);
        }
        if (settings.ems, settings.umb, settings.dos_high) != self.memory {
            let on_off = |on| if on { "on" } else { "off" };
            cpu.bus.log_string(&format!(
                "[CONFIG] EMS {}, upper memory {}, DOS high {}",
                on_off(settings.ems),
                on_off(settings.umb),
                on_off(settings.dos_high)
            ));
            warnings.extend(cpu.set_dos_high(settings.dos_high).err());
            warnings.extend(cpu.set_upper_memory(settings.ems, settings.umb).err());
        }
        if settings.network != self.network {
            cpu.bus.log_string("[CONFIG] The network settings changed");
            cpu.bus.configure_network(&settings.network);
        }
        if settings.serial != self.serial {
            cpu.bus.log_string("[CONFIG] The serial ports changed");
            cpu.bus.configure_serial(&settings.serial);
        }
        if (&settings.printer, &settings.capture_dir) != (&self.printer.0, &self.printer.1) {
            cpu.bus.log_string("[CONFIG] The printer changed");
            cpu.bus.configure_printer(&settings.printer, &settings.capture_dir);
        }
        *self = Self::of(settings);
        warnings
    }
}
