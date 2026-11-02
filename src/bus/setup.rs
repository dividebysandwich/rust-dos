//! Building the machine, the built-in DOS's setup, and the settings the
//! frontends give it.

use std::path::PathBuf;

use super::*;

impl Bus {
    /// A machine with the default 16 MB of RAM.
    pub fn new(root_path: PathBuf) -> Self {
        Self::with_memory(root_path, DEFAULT_MEMORY_MB)
    }

    /// A machine with `memory_mb` MB of RAM (at least 2).
    pub fn with_memory(root_path: PathBuf, memory_mb: usize) -> Self {
        let ram_len = memory_mb.max(2) << 20;
        let mut bus = Self {
            ram: vec![0; ram_len],
            video_mode: VideoMode::Text80x25, // Start in Text Mode (BIOS default)
            disk: DiskController::new(root_path),
            boot: None,
            guest_paging: None,
            guest_fault: None,
            keyboard_buffer: VecDeque::new(),
            key_waits: 0,
            kbd: crate::keyboard::KeyboardState::default(),
            kbc: crate::kbc::Kbc::new(),
            kbc_release_at: None,
            rtc_next: None,
            a20_mask: !0x0010_0000,
            reset_requested: false,
            port_accesses: VecDeque::new(),
            port_log: None,
            profile: None,
            rom_writes: 0,
            config_ui_requested: false,
            exit_requested: false,
            config_dir: None,
            cmos: crate::cmos::Cmos::new(((ram_len >> 10) - 1024) as u32),
            post_code: 0,
            debug_console: Vec::new(),
            xms: crate::xms::Xms::new(),
            ems: None,
            dpmi: crate::dpmi::Dpmi::default(),
            win32: crate::win32::Win32::default(),
            dos_version: crate::config::DosVersion::default(),
            ide_hard_disks: true,
            floppy_controller: true,
            boot_cdrom: true,
            idle_hint: false,
            disk_notices: Vec::new(),
            umb: None,
            dos_high: true,
            lpt_dac: None,
            printer: None,
            printer_dir: None,
            printer_settings: crate::printer::PrinterSettings { output: crate::printer::PrinterOutput::None, ..Default::default() },
            tandy_sound: crate::sn76489::Sn76489::new(crate::sn76489::Variant::Ncr8496),
            tandy_mode: crate::sn76489::TandySound::Auto,
            freezes: Vec::new(),
            mixer_changed: false,
            frames_drawn: 0,
            activity: crate::autospeed::Activity::new(),
            retraces: 0,
            last_flip: None,
            gate_array_shadow: Vec::new(),
            gate_array_shadow_at: usize::MAX,
            refresh_toggle: false,
            cursor_x: 0,
            cursor_y: 0,
            start_time: Instant::now(),
            joystick: crate::joystick::GamePort::default(),
            vfx1: None,
            audio_device: None,
            speaker_on: false,
            pit_divisor: 0xFFFF,
            pit_mode: 0,
            pit_write_msb: false,
            pit_read_msb: false,
            pit2: crate::timer::Pit2::new(),
            speaker_data: false,
            ppi_checks: 0,
            pit2_gate_at_start: false,
            pit0_divisor: 0xFFFF,
            pit0_write_msb: false,
            pit0_read_msb: false,
            pit0_access: 3,
            pit0_latched: 0,
            pit0_latched_active: false,
            pit0: crate::timer::Pit0::new(),
            clock: crate::timer::Clock::new(crate::timer::CpuSpeed::Max.initial_cycles()),
            pic: crate::pic::Pic::new(),
            audio_phase: 0.0,
            log_file: None,
            dta_segment: 0x1000,
            dta_offset: 0x0000,
            vga: crate::video::vga::VgaCard::new(),
            vbe: crate::video::vbe::Vbe::new(),
            s3_engine: crate::video::s3::engine::Engine::new(),
            virge: Default::default(),
            verite: Default::default(),
            mach64: Default::default(),
            attribute_reset: Default::default(),
            pci: crate::pci::Pci::default(),
            voodoo: None,
            powervr: None,
            powervr_line: None,
            hold: None,
            notices: Vec::new(),
            glide_hint: None,
            ide: [None, None],
            fdc: None,
            faked_io: false,
            search_handles: std::collections::HashMap::new(),
            search_serial: 0,
            mouse: crate::mouse::MouseState::new(),
            opl: crate::opl::Opl::new(true),
            sb: Some(crate::sb::SoundBlaster::new(crate::sb::SbConfig::default(), 0)),
            game_fixes: true,
            game_fixes_off: Vec::new(),
            isapnp: None,
            dma: crate::dma::Dma::new(),
            mpu: crate::mpu401::Mpu401::new(),
            gus: Some(crate::gus::Gus::new(crate::gus::GusConfig::default(), 0)),
            gus_line: None,
            awe: None,
            awe_rom: std::sync::Arc::from([]),
            awe_ram_kb: crate::awe32::DEFAULT_RAM_KB,
            net: crate::net::Net::new(),
            serial: crate::serial::Serial::default(),
            ultrasnd_drive: None,
            cdaudio: crate::cdrom::audio::CdPlayer::new(),
            disk_io: crate::diskio::DiskIo::default(),
            disknoise: crate::disknoise::DiskNoise::new(),
            monitor_noise: Default::default(),
            power_ons: 0,
            setup_requested: false,
            drives_active: 0,
            mscdex: Default::default(),
            audio_out: VecDeque::new(),
            audio_feed: crate::audio::Feed::default(),
            audio_frames: 0,
            audio_settled: false,
            sb_frame: (0, 0),
            beep_frames: 0,
            mixer: crate::mixer::Mixer::default(),
            sb_irq: (None, false, false),
            irq_ready: false,
            audio_peak: 0,
            audio_underruns: 0,
            host_frames: crate::hostframes::HostFrames::new(),
            unhandled_writes: vec![0; 0x10000],
            page_gen: vec![0; ram_len >> GEN_SHIFT],
            observe: crate::idle::Observe::default(),
            code_blocks: vec![0; ram_len >> GEN_SHIFT],
            log_hook: None,
            log_backlog: None,
            audio_hook: None,
        };
        bus.init_dos_machine(video::adapter::VideoSetup::default());

        // The default Ultrasound's software, and the equipment word, hard
        // disk count and DPBs for the drives C:, X: and Z:.
        let _ = bus.mount_ultrasnd(crate::gus::GusConfig::default().drive);
        // The file table, with the standard devices open, and the XMS
        // driver's entry.
        crate::dos_files::write_table(&mut bus);
        crate::xms::install_entry(&mut bus);

        bus
    }

    /// The BIOS data area, the ROMs' code and the vector table, and an
    /// empty chain of DOS memory, as the built-in DOS starts with them on
    /// the display adapter `setup`.
    fn init_dos_machine(&mut self, setup: video::adapter::VideoSetup) {
        // BIOS Data Area (BDA) Initialization
        // 0x0449: Current Video Mode (03 = 80x25 Color)
        self.write_8(0x0449, 0x03);
        // 0x044A: Number of Columns (80 = 0x50)
        self.write_16(0x044A, 80);
        // 0x044C: Video Page Size (80x25 text: 4000 bytes, rounded to 4 KB),
        // 0x044E: the offset of the active page.
        self.write_16(0x044C, 0x1000);
        self.write_16(0x044E, 0);
        // 0x0460: Cursor Shape (Start Line 13, End Line 14 for VGA)
        self.write_16(0x0460, 0x0D0E);
        // 0x0462: Active Page (0)
        self.write_8(0x0462, 0);
        // 0x0463: CRT Controller Base Address (0x3D4 for Color)
        self.write_16(0x0463, 0x03D4);

        // 0x0410: Equipment List. Bit 0 = Floppy (see `sync_drive_bda`);
        // the video adapter's bits come with it below. Bits 1 and 2: the
        // coprocessor and the PS/2 mouse every machine has.
        self.write_16(0x0410, crate::bios::EQUIPMENT_FIXED | 0x0001);

        // 0x0417: Num Lock on, as the BIOS leaves it; 0x0496: an enhanced
        // (101-key) keyboard.
        self.write_8(0x0417, 0x20);
        self.write_8(0x0496, crate::keyboard::ENHANCED_KEYBOARD);
        // The keyboard buffer, from 40:1E to 40:3E, empty: DOS's keys are
        // the host's (`keyboard_buffer`), but the machines of Windows' 386
        // enhanced mode keep theirs there (`keyboard::bios_keystrokes`).
        self.write_16(0x041A, 0x001E);
        self.write_16(0x041C, 0x001E);
        self.write_16(0x0480, 0x001E);
        self.write_16(0x0482, 0x003E);

        // 0x046C: the timer ticks since midnight, by the real-time clock.
        crate::bios::set_tick_count(self);

        // 0x0484: Rows on Screen (minus 1). 24 = 25-row default.
        self.write_8(0x0484, 24);
        // 0x0485: Character height in scan lines. 16 = VGA 8x16 default.
        self.write_16(0x0485, 16);

        // The display adapter: its BIOS data and ROM.
        video::bios::install(self, setup);

        // The video BIOS's static functionality table at F000:E000: all
        // modes (00-02), all scan line counts (07), 8 character blocks
        // (0B), 2 of them active (0C); no further capabilities.
        self.write_rom(0xFE000, &[0xFF, 0xFF, 0xFF, 0, 0, 0, 0, 0xFF, 0, 0, 0, 0x08, 0x02, 0, 0, 0, 0]);

        // BIOS ROM code and the interrupt vector table.
        crate::bios::install(self);
        crate::mouse::install_callback_stub(self);

        // Build a baseline MCB chain — one large free block covering
        // conventional memory. load_shell / load_exe rebuild as needed, but we
        // still want mcb::alloc to work for tests and any early allocation.
        crate::mcb::init_empty(self);
    }

    /// Start the built-in DOS again after the system booted from a disk
    /// turned the machine off (`boot::power_off`): the BIOS's data and
    /// vectors, DOS's tables and an empty memory chain as at rust-dos's
    /// start, for the drives and devices there are now.
    pub fn restore_dos_machine(&mut self) {
        self.fill_ram(0..0x500, 0);
        self.pic = crate::pic::Pic::new();
        self.reset_voodoo();
        self.reset_powervr();
        self.detach_ide();
        self.detach_fdc();
        self.disk.drop_boot_cds();
        self.cmos.set_hard_disks(&[]);
        self.init_dos_machine(self.vga.setup());
        if self.lpt1_present() {
            self.write_lpt_bda();
        }
        if self.joystick.present() {
            let equipment = self.read_16(0x0410);
            self.write_16(0x0410, equipment | 0x1000);
        }
        self.write_serial_bda();
        self.reset_serial();
        crate::mcb::build_upper(self);
        self.sync_drive_bda();
        crate::dos_files::write_table(self);
        crate::xms::install_entry(self);
    }

    /// The layout text typed in from the host goes in as: the keyboard's,
    /// or on a booted system, whose BIOS has none but the US one, that.
    pub fn typing_layout(&self) -> &'static crate::keylayout::Layout {
        if self.boot.is_some() { crate::keylayout::Layout::us() } else { self.kbd.layout }
    }

    /// Take the disk speed and noise settings.
    pub fn set_disk_settings(&mut self, settings: crate::diskio::DiskSettings) {
        self.audio_catch_up();
        self.disk_io.settings = settings;
        self.disknoise.set_modes(settings.floppy_disk_noise, settings.hard_disk_noise);
    }

    /// Take the volumes of the host's mixer, from the sound rendered next.
    pub fn set_mixer(&mut self, settings: crate::mixer::MixerSettings) {
        self.audio_catch_up();
        self.mixer.set(settings);
    }

    /// Put the frozen values back (the frontends do before every frame).
    pub fn apply_freezes(&mut self) {
        for i in 0..self.freezes.len() {
            let freeze = self.freezes[i];
            for (j, byte) in freeze.width.bytes_of(freeze.value).into_iter().enumerate() {
                self.write_8(freeze.addr + j, byte);
            }
        }
    }

    /// Set the `tandy` setting.
    pub fn configure_tandy_sound(&mut self, mode: crate::sn76489::TandySound) {
        self.audio_catch_up();
        self.tandy_mode = mode;
    }

    /// Set PIT channel 2's gate (port 61h bit 0) as `pit2_gate_at_start`
    /// has it at startup, with the speaker data bit clear so nothing
    /// sounds, as DOSBox-X's BIOS does for its "pcspeaker clock gate enable
    /// at startup". With the gate up, the channel counts in mode 3 from
    /// FFFFh, whatever an earlier program left in it: a game's profile
    /// turns this on at the DOS prompt, after other programs ran.
    pub fn set_pit2_gate_at_start(&mut self, on: bool) {
        self.pit2_gate_at_start = on;
        // The speaker off and the gate low first, so a tone ends where it
        // was.
        self.io_write(0x61, self.ppi_checks);
        if on {
            self.pit2 = crate::timer::Pit2::new();
            self.pit_divisor = 0xFFFF;
            self.pit_write_msb = false;
            self.pit_read_msb = false;
            self.io_write(0x61, self.ppi_checks | 0x01);
        }
    }

    /// Put the VFX1's card in (`[vfx1]`) at its port, or take it out.
    pub fn configure_vfx1(&mut self, settings: &crate::vfx1::Vfx1Settings) {
        let base = settings.enabled.then_some(settings.port);
        if self.vfx1.as_ref().map(|v| v.base) == base {
            return;
        }
        self.vfx1 = base.map(|base| {
            self.log_string(&format!("[VFX1] The VIP card at {:X}h", base));
            crate::vfx1::Vfx1::new(base)
        });
    }

    /// Put a DAC on LPT1 (`lpt_dac`), or take it away: the printer gives
    /// way to it, and the BIOS data area has LPT1 while either is there.
    pub fn configure_lpt_dac(&mut self, kind: crate::lpt_dac::LptDacType) {
        self.audio_catch_up();
        let present = kind != crate::lpt_dac::LptDacType::None;
        self.lpt_dac = present.then(|| crate::lpt_dac::LptDac::new(kind));
        self.place_printer();
    }

    /// Take the `[joystick]` settings: what the game port has plugged in,
    /// and whether there is one, which bit 12 of the equipment word says.
    pub fn set_joystick(&mut self, settings: crate::joystick::JoystickSettings) {
        self.joystick.set_settings(settings);
        if self.boot.is_some() {
            return;
        }
        let equipment = self.read_16(0x0410) & !0x1000;
        self.write_16(0x0410, equipment | if self.joystick.present() { 0x1000 } else { 0 });
    }

    /// Power-on state of the PIT channel 0 and the PIC, so a program that
    /// exits (or is killed) with a fast timer or masked IRQs doesn't leave
    /// them behind for the shell and the next program.
    pub fn reset_timers(&mut self) {
        // Mode 3, count 65536, counting from now.
        let now = self.clock.now_ticks();
        self.pit0.set_mode(3, now);
        self.pit0.write_count(0, now);
        self.pit0_divisor = 0;
        self.pit0_write_msb = false;
        self.pit0_read_msb = false;
        self.pit0_access = 3;
        self.pit0_latched_active = false;
        self.pic = crate::pic::Pic::new();
        self.clock.schedule(self.next_event());
    }
}
