//! The bus's part of a save state: the RAM and the devices on the bus, in
//! sections. Every field of `Bus` is named here, as saved in one of the
//! sections or as left alone, so a field added to it fails to compile
//! until it is one or the other.

use super::Bus;
use crate::savestate::{Reader, Result, State, StateError, Writer, load_device, save_device};

/// The sections' versions, changed with what a section holds.
const RAM_VERSION: u16 = 2;
const CORE_VERSION: u16 = 8;
const VIDEO_VERSION: u16 = 2;
const SOUND_VERSION: u16 = 3;
const DOS_VERSION: u16 = 6;
const VOODOO_VERSION: u16 = 1;
const POWERVR_VERSION: u16 = 1;
const IDE_VERSION: u16 = 2;
const FDC_VERSION: u16 = 1;
const DPMI_VERSION: u16 = 2;
const NET_VERSION: u16 = 2;
const SERIAL_VERSION: u16 = 1;
const AWE_VERSION: u16 = 1;
const VIRGE_VERSION: u16 = 1;
const VERITE_VERSION: u16 = 2;
const ET4000_VERSION: u16 = 1;
const SHARED_VERSION: u16 = 1;
const SB_HIGH_SPEED_VERSION: u16 = 1;
const ISAPNP_VERSION: u16 = 1;
const PIT2_VERSION: u16 = 1;

/// Save or load each of a list of fields.
/// Each field is labelled with its name, for comparing states part by
/// part (`Writer::labelled`).
macro_rules! save_all {
    ($w:expr; $($f:ident),* $(,)?) => { $( $w.label(stringify!($f)); State::save($f, $w); )* };
}
macro_rules! load_all {
    ($r:expr; $($f:expr),* $(,)?) => { $( State::load($f, $r)?; )* };
}

impl Bus {
    /// Write the bus's sections.
    pub(crate) fn save_state(&self, w: &mut Writer) {
        let Bus {
            ram,
            boot,
            pci,
            voodoo,
            powervr,
            powervr_line,
            hold: _,
            notices: _,
            glide_hint: _,
            ide,
            fdc,
            net,
            serial,
            keyboard_buffer,
            kbd,
            kbc,
            kbc_release_at: _,
            rtc_next: _,
            a20_mask,
            cmos,
            pit_divisor,
            pit_read_msb,
            pit_mode,
            pit_write_msb,
            pit2,
            speaker_data,
            ppi_checks,
            pit2_gate_at_start: _,
            pit0_divisor,
            pit0_write_msb,
            pit0_read_msb,
            pit0_access,
            pit0_latched,
            pit0_latched_active,
            pit0,
            clock,
            pic,
            dma,
            joystick,
            dta_segment,
            dta_offset,
            search_handles,
            search_serial,
            cursor_x,
            cursor_y,
            post_code,
            refresh_toggle,
            speaker_on,
            audio_phase,
            audio_frames,
            sb_phase,
            sb_frame,
            beep_frames,
            video_mode,
            vga,
            vbe,
            s3_engine,
            virge,
            verite,
            retraces,
            last_flip,
            gate_array_shadow,
            gate_array_shadow_at,
            opl,
            sb,
            isapnp,
            mpu,
            gus,
            gus_line,
            awe,
            awe_rom: _,
            awe_ram_kb: _,
            tandy_sound,
            lpt_dac,
            cdaudio,
            xms,
            ems,
            dpmi,
            umb,
            dos_high,
            mouse,
            mscdex,
            disk,
            disk_io,
            // Set from the configuration, which a state carries in its
            // header.
            tandy_mode: _,
            ultrasnd_drive: _,
            dos_version: _,
            ide_hard_disks: _,
            floppy_controller: _,
            boot_cdrom: _,
            idle_hint: _,
            disk_notices: _,
            // Worked out again after a load.
            sb_irq: _,
            irq_ready: _,
            page_gen: _,
            observe: _,
            audio_settled: _,
            code_blocks: _,
            // A service's port accesses for a V86 monitor, made within the
            // instructions after it.
            port_accesses: _,
            faked_io: _,
            port_log: _,
            profile: _,
            guest_paging: _,
            guest_fault: _,
            // How many ignored ROM writes were logged.
            rom_writes: _,
            // Requests to the front end, which it carries out before a
            // state can be saved.
            reset_requested: _,
            config_ui_requested: _,
            exit_requested: _,
            mixer_changed: _,
            // The host's: its output, its settings and what it shows.
            config_dir: _,
            freezes: _,
            frames_drawn: _,
            activity: _,
            debug_console: _,
            start_time: _,
            audio_device: _,
            log_file: _,
            disknoise: _,
            monitor_noise: _,
            power_ons: _,
            setup_requested: _,
            drives_active: _,
            audio_out: _,
            // The printer is the host's: loading a state takes back no
            // paper.
            printer: _,
            printer_settings: _,
            printer_dir: _,
            audio_feed: _,
            mixer: _,
            audio_peak: _,
            audio_underruns: _,
            key_waits: _,
            unhandled_writes: _,
            attribute_reset: _,
            log_hook: _,
            audio_hook: _,
        } = self;
        w.section(b"RAM ", RAM_VERSION, |w| ram.save(w));
        w.section(b"CORE", CORE_VERSION, |w| {
            save_all!(w;
                keyboard_buffer, kbd, kbc, a20_mask, cmos,
                pit_divisor, pit_read_msb, pit_mode, pit_write_msb,
                pit0_divisor, pit0_write_msb, pit0_read_msb, pit0_access, pit0_latched, pit0_latched_active,
                pit0, clock, pic, dma, joystick,
                dta_segment, dta_offset, search_handles, search_serial, cursor_x, cursor_y,
                post_code, refresh_toggle, speaker_on, audio_phase, audio_frames, sb_phase, sb_frame, beep_frames,
                boot, pci,
            );
        });
        // Channel 2's counter and gate, in a section of their own so states
        // saved before it load. The fields after `speaker_data` came later:
        // a section that ends before them loads with their defaults.
        let pit2_count_due = &pit2.count_due;
        w.section(b"PIT2", PIT2_VERSION, |w| {
            save_all!(w; pit2, speaker_data, pit2_count_due, ppi_checks);
        });
        w.section(b"VIDE", VIDEO_VERSION, |w| {
            save_all!(w; video_mode, vga, vbe, s3_engine, retraces, last_flip, gate_array_shadow, gate_array_shadow_at);
        });
        // An S3 ViRGE's engines, only on one, so other states load as they
        // did.
        if vga.adapter.is_virge() {
            w.section(b"VIRG", VIRGE_VERSION, |w| virge.save(w));
        }
        // The Vérité's registers and FIFO, only on one, likewise.
        if vga.adapter.is_verite() {
            w.section(b"VRTE", VERITE_VERSION, |w| verite.save(w));
        }
        // The ET4000's registers, only on one, likewise.
        if vga.adapter.is_et4000() {
            w.section(b"ET4K", ET4000_VERSION, |w| vga.et4000.save(w));
        }
        // A booted system's disks made of shared host directories, before
        // the drives, which need them for their checkpoints; only with
        // them, so other states load as they did.
        let shared = disk.shared_manifests();
        if !shared.is_empty() {
            w.section(b"SHRD", SHARED_VERSION, |w| {
                shared.len().save(w);
                for (drive, manifest) in &shared {
                    drive.save(w);
                    manifest.save(w);
                }
            });
        }
        w.section(b"DOS ", DOS_VERSION, |w| {
            save_all!(w; xms, mouse, mscdex, disk_io);
            w.label("ems");
            save_device(ems, w);
            w.label("umb");
            save_device(umb, w);
            save_all!(w; dos_high);
            w.label("disk");
            disk.save_state(w);
        });
        // The Ultrasound's memory first, where it stays in place for
        // rewind's deltas (rewind.rs) whatever the queues after it hold.
        w.section(b"SOUN", SOUND_VERSION, |w| {
            w.label("gus");
            save_device(gus, w);
            save_all!(w; opl, mpu, gus_line, tandy_sound);
            w.label("sb");
            save_device(sb, w);
            w.label("lpt_dac");
            save_device(lpt_dac, w);
            w.label("cdaudio");
            cdaudio.save_state(w);
        });
        // The AWE32's EMU8000 (its RAM first, for rewind), only with one,
        // so states of machines without load as they did.
        if let Some(awe) = awe {
            w.section(b"AWE ", AWE_VERSION, |w| awe.save(w));
        }
        // The ISA Plug and Play cards (a ViBRA16C), likewise.
        if let Some(isapnp) = isapnp {
            w.section(b"PNP ", ISAPNP_VERSION, |w| isapnp.save(w));
        }
        // A 3dfx card, whose memory comes first, and only on a machine
        // with one, so states of machines without load as they did.
        if let Some(voodoo) = voodoo {
            w.section(b"3DFX", VOODOO_VERSION, |w| voodoo.save(w));
        }
        // A PowerVR card, likewise.
        if let Some(powervr) = powervr {
            w.section(b"PVR ", POWERVR_VERSION, |w| {
                powervr.save(w);
                powervr_line.save(w);
            });
        }
        // A booted system's IDE channels, likewise.
        if ide.iter().any(Option::is_some) {
            w.section(b"IDE ", IDE_VERSION, |w| ide.save(w));
        }
        // A booted system's floppy disk controller, likewise.
        if let Some(fdc) = fdc {
            w.section(b"FDC ", FDC_VERSION, |w| fdc.save(w));
        }
        // The DPMI host, while it has clients.
        if dpmi.active() {
            w.section(b"DPMI", DPMI_VERSION, |w| dpmi.save(w));
        }
        // The IPX driver, once installed, and the network card. The LAN
        // isn't part of the machine: frames in flight are lost with a load.
        if net.ipx.is_some() || net.nic.is_some() {
            w.section(b"NET ", NET_VERSION, |w| {
                w.label("ipx");
                net.ipx.save(w);
                w.label("nic");
                save_device(&net.nic, w);
            });
        }
        // The serial ports. A modem's call, like the LAN, isn't part of the
        // machine.
        if serial.any() {
            w.section(b"SER ", SERIAL_VERSION, |w| serial.save(w));
        }
        // The Sound Blaster's high-speed mode, an empty section that is
        // there only while the mode runs, so states saved before rust-dos
        // kept the mode load as they did, into a DSP out of it. It comes
        // last: rewind pairs the sections of two states by position, and a
        // section that comes and goes with each 91h block would otherwise
        // shift the ones after it (a 3dfx card's memory) out of their pairs.
        if sb.as_ref().is_some_and(|sb| sb.high_speed()) {
            w.section(b"SBHS", SB_HIGH_SPEED_VERSION, |_| {});
        }
    }

    /// Read the bus's sections into it, in place: the RAM keeps its
    /// allocation, and must be as large as the saved one. Returns the
    /// files open in the state that couldn't be opened again.
    pub(crate) fn load_state(&mut self, r: &mut Reader) -> Result<Vec<String>> {
        let Bus {
            ram,
            boot,
            pci,
            voodoo,
            powervr,
            powervr_line,
            hold: _,
            notices: _,
            glide_hint: _,
            ide,
            fdc,
            net,
            serial,
            keyboard_buffer,
            kbd,
            kbc,
            kbc_release_at: _,
            rtc_next: _,
            a20_mask,
            cmos,
            pit_divisor,
            pit_read_msb,
            pit_mode,
            pit_write_msb,
            pit2,
            speaker_data,
            ppi_checks,
            pit2_gate_at_start: _,
            pit0_divisor,
            pit0_write_msb,
            pit0_read_msb,
            pit0_access,
            pit0_latched,
            pit0_latched_active,
            pit0,
            clock,
            pic,
            dma,
            joystick,
            dta_segment,
            dta_offset,
            search_handles,
            search_serial,
            cursor_x,
            cursor_y,
            post_code,
            refresh_toggle,
            speaker_on,
            audio_phase,
            audio_frames,
            sb_phase,
            sb_frame,
            beep_frames,
            video_mode,
            vga,
            vbe,
            s3_engine,
            virge,
            verite,
            retraces,
            last_flip,
            gate_array_shadow,
            gate_array_shadow_at,
            opl,
            sb,
            isapnp,
            mpu,
            gus,
            gus_line,
            awe,
            awe_rom: _,
            awe_ram_kb: _,
            tandy_sound,
            lpt_dac,
            cdaudio,
            xms,
            ems,
            dpmi,
            umb,
            dos_high,
            mouse,
            mscdex,
            disk,
            disk_io,
            tandy_mode: _,
            ultrasnd_drive: _,
            dos_version: _,
            ide_hard_disks: _,
            floppy_controller: _,
            boot_cdrom: _,
            idle_hint: _,
            disk_notices: _,
            sb_irq: _,
            irq_ready: _,
            page_gen,
            observe: _,
            audio_settled: _,
            code_blocks,
            reset_requested: _,
            port_accesses: _,
            faked_io: _,
            port_log: _,
            profile: _,
            guest_paging: _,
            guest_fault: _,
            rom_writes: _,
            config_ui_requested: _,
            exit_requested: _,
            mixer_changed: _,
            config_dir: _,
            freezes: _,
            frames_drawn: _,
            activity: _,
            debug_console: _,
            start_time: _,
            audio_device: _,
            log_file: _,
            disknoise: _,
            monitor_noise: _,
            power_ons: _,
            setup_requested: _,
            drives_active: _,
            audio_out: _,
            printer: _,
            printer_settings: _,
            printer_dir: _,
            audio_feed: _,
            mixer: _,
            audio_peak: _,
            audio_underruns: _,
            key_waits: _,
            unhandled_writes: _,
            attribute_reset: _,
            log_hook: _,
            audio_hook: _,
        } = self;
        let mut section = r.section(b"RAM ", RAM_VERSION)?;
        let len = section.count()?;
        if len != ram.len() {
            // A state of another memsize: the memory takes the state's size.
            let mb = len >> 20;
            if len & 0xF_FFFF != 0 || !(crate::config::MIN_MEMSIZE..=crate::config::MAX_MEMSIZE).contains(&mb) {
                return Err(StateError::Invalid(format!("{} KB of memory", len / 1024)));
            }
            *ram = vec![0; len];
            *page_gen = vec![0; len >> super::GEN_SHIFT];
            *code_blocks = vec![0; len >> super::GEN_SHIFT];
        }
        ram.copy_from_slice(section.take(len)?);
        let mut section = r.section(b"CORE", CORE_VERSION)?;
        load_all!(&mut section;
            keyboard_buffer, kbd, kbc, a20_mask, cmos,
            pit_divisor, pit_read_msb, pit_mode, pit_write_msb,
            pit0_divisor, pit0_write_msb, pit0_read_msb, pit0_access, pit0_latched, pit0_latched_active,
            pit0, clock, pic, dma, joystick,
            dta_segment, dta_offset, search_handles, search_serial, cursor_x, cursor_y,
            post_code, refresh_toggle, speaker_on, audio_phase, audio_frames, sb_phase, sb_frame, beep_frames,
                boot, pci,
        );
        if r.next_is(b"PIT2") {
            let mut section = r.section(b"PIT2", PIT2_VERSION)?;
            load_all!(&mut section; pit2, speaker_data);
            pit2.count_due = false;
            *ppi_checks = 0;
            if !section.is_empty() {
                load_all!(&mut section; &mut pit2.count_due, ppi_checks);
            }
        } else {
            // An older state: a stopped channel 2 with the gate and the
            // speaker as they were.
            *pit2 = crate::timer::Pit2::new();
            let now = clock.now_ticks();
            pit2.write_count(*pit_divisor, now);
            pit2.set_gate(*speaker_on, now);
            *speaker_data = *speaker_on;
            *ppi_checks = 0;
        }
        let mut section = r.section(b"VIDE", VIDEO_VERSION)?;
        load_all!(&mut section; video_mode, vga, vbe, s3_engine, retraces, last_flip, gate_array_shadow, gate_array_shadow_at);
        *virge = Default::default();
        if r.next_is(b"VIRG") {
            virge.load(&mut r.section(b"VIRG", VIRGE_VERSION)?)?;
        }
        verite.reset();
        if r.next_is(b"VRTE") {
            verite.load(&mut r.section(b"VRTE", VERITE_VERSION)?)?;
        }
        vga.et4000 = Default::default();
        if r.next_is(b"ET4K") {
            vga.et4000.load(&mut r.section(b"ET4K", ET4000_VERSION)?)?;
        }
        disk.shared_from_state = None;
        if r.next_is(b"SHRD") {
            let mut section = r.section(b"SHRD", SHARED_VERSION)?;
            let mut shared = Vec::new();
            for _ in 0..section.count()? {
                let (mut drive, mut manifest) = (0u8, String::new());
                drive.load(&mut section)?;
                manifest.load(&mut section)?;
                shared.push((drive, manifest));
            }
            disk.shared_from_state = Some(shared);
        }
        // The drives before the sound: the CD playing is in one.
        let mut section = r.section(b"DOS ", DOS_VERSION)?;
        load_all!(&mut section; xms, mouse, mscdex, disk_io);
        load_device(ems, "expanded memory manager", &mut section)?;
        load_device(umb, "upper memory", &mut section)?;
        dos_high.load(&mut section)?;
        let lost = disk.load_state(&mut section)?;
        let mut section = r.section(b"SOUN", SOUND_VERSION)?;
        load_device(gus, "Gravis Ultrasound", &mut section)?;
        load_all!(&mut section; opl, mpu, gus_line, tandy_sound);
        load_device(sb, "Sound Blaster", &mut section)?;
        load_device(lpt_dac, "DAC on LPT1", &mut section)?;
        cdaudio.load_state(&mut section, |drive| disk.cd_image(drive))?;
        match (r.next_is(b"AWE "), awe) {
            (true, Some(awe)) => awe.load(&mut r.section(b"AWE ", AWE_VERSION)?)?,
            (false, None) => {}
            (true, None) => return Err(StateError::Mismatch("it has an AWE32 and this machine hasn't".into())),
            (false, Some(_)) => return Err(StateError::Mismatch("this machine has an AWE32 and it hasn't".into())),
        }
        match (r.next_is(b"PNP "), isapnp) {
            (true, Some(isapnp)) => isapnp.load(&mut r.section(b"PNP ", ISAPNP_VERSION)?)?,
            (false, None) => {}
            (true, None) => return Err(StateError::Mismatch("it has a ViBRA16C and this machine hasn't".into())),
            (false, Some(_)) => return Err(StateError::Mismatch("this machine has a ViBRA16C and it hasn't".into())),
        }
        match (r.next_is(b"3DFX"), voodoo) {
            (true, Some(voodoo)) => voodoo.load(&mut r.section(b"3DFX", VOODOO_VERSION)?)?,
            (false, None) => {}
            (true, None) => return Err(StateError::Mismatch("it has a 3dfx card and this machine hasn't".into())),
            (false, Some(_)) => return Err(StateError::Mismatch("this machine has a 3dfx card and it hasn't".into())),
        }
        match (r.next_is(b"PVR "), powervr) {
            (true, Some(powervr)) => {
                let mut section = r.section(b"PVR ", POWERVR_VERSION)?;
                powervr.load(&mut section)?;
                powervr_line.load(&mut section)?;
            }
            (false, None) => {}
            (true, None) => return Err(StateError::Mismatch("it has a PowerVR card and this machine hasn't".into())),
            (false, Some(_)) => return Err(StateError::Mismatch("this machine has a PowerVR card and it hasn't".into())),
        }
        // The IDE channels come and go with the booted system.
        *ide = [None, None];
        if r.next_is(b"IDE ") {
            ide.load(&mut r.section(b"IDE ", IDE_VERSION)?)?;
        }
        // So does the floppy disk controller; a state saved before
        // rust-dos had one has none.
        *fdc = None;
        if r.next_is(b"FDC ") {
            let mut controller = crate::fdc::Fdc::new();
            controller.load(&mut r.section(b"FDC ", FDC_VERSION)?)?;
            *fdc = Some(controller);
        }
        dpmi.reset();
        if r.next_is(b"DPMI") {
            dpmi.load(&mut r.section(b"DPMI", DPMI_VERSION)?)?;
        }
        // The extended memory it holds.
        xms.dpmi = dpmi.reservations();
        // The IPX driver and the network card come and go with the state.
        if r.next_is(b"NET ") {
            let mut section = r.section(b"NET ", NET_VERSION)?;
            net.ipx.load(&mut section)?;
            load_device(&mut net.nic, "NE2000", &mut section)?;
        } else if net.nic.is_some() {
            return Err(StateError::Mismatch("this machine has an NE2000 and it hasn't".into()));
        } else {
            net.ipx = None;
        }
        // The serial ports come and go with the state.
        if r.next_is(b"SER ") {
            serial.load(&mut r.section(b"SER ", SERIAL_VERSION)?)?;
        } else {
            serial.ports = Default::default();
            serial.pic_lines = 0;
        }
        let high_speed = r.next_is(b"SBHS");
        if high_speed {
            r.section(b"SBHS", SB_HIGH_SPEED_VERSION)?;
        }
        match sb {
            Some(sb) => {
                if !sb.set_high_speed(high_speed) {
                    return Err(StateError::Invalid("a Sound Blaster's high-speed mode without a transfer".into()));
                }
                sb.restart_dma_identification();
            }
            None if high_speed => return Err(StateError::Invalid("a Sound Blaster's high-speed mode without a Sound Blaster".into())),
            None => {}
        }
        Ok(lost)
    }

    /// Bring what is worked out from the saved state up to date after a
    /// load, once the CPU's caches are empty: the code generations start
    /// again from nothing (so machines loaded from one state are alike),
    /// the interrupt lines follow the devices, the picture is drawn anew,
    /// the FM chip and the MIDI synthesizer are told their registers and
    /// setup again, and the sound rendered before the load is dropped with
    /// what rang on of it in the mixer.
    pub(crate) fn after_load(&mut self) {
        self.after_reload();
        self.opl.after_load();
        self.mpu.after_load();
        if let Some(gus) = &mut self.gus {
            gus.after_load();
        }
        self.mixer.clear_tails();
        self.audio_out.clear();
        // The frames that came for the machine before the load are gone,
        // and the IPX driver's node is the state's.
        self.net.ipx_queue.clear();
        self.net.ipx_installed();
        // The clock's periodic interrupt as the state's registers have it,
        // and the keyboard's next byte at once.
        self.rtc_next = None;
        self.schedule_rtc();
        self.kbc_release_at = None;
        self.kbc.release();
        self.net.nic_changed();
    }

    /// `after_load` for a state loaded into the machine that saved it,
    /// whose FM chip, synthesizer and sound output are still those of the
    /// state.
    pub(crate) fn after_reload(&mut self) {
        for failed in self.disk.revert_disks() {
            self.log_string(&format!("[STATE] The disk can't go back with the state: {}", failed));
        }
        // A booted system's disks keep journals; the built-in DOS's don't.
        self.disk.keep_journals(self.boot.is_some());
        // A state of the built-in DOS over a booted system: what it wrote
        // on the host directories it had as disks goes into them, and the
        // discs made of host folders go.
        if self.boot.is_none() {
            for line in self.disk.finish_shared_disks(false) {
                self.log_string(&format!("[STATE] {}", line));
                self.disk_notices.push(line);
            }
            self.disk.drop_boot_cds();
        }
        if self.boot.is_some() {
            for line in self.disk.prepare_boot_cds() {
                self.log_string(&format!("[IDE] {}", line));
            }
        }
        self.page_gen.fill(0);
        // The PICs' requests are the state's: the Sound Blaster's interrupts
        // as it has them raise none.
        self.sb_irq = self.sb_irq_now();
        self.refresh_irq();
        self.vga.after_load();
        if let Some(awe) = &mut self.awe {
            awe.after_load();
        }
    }
}
