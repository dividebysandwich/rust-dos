//! The bus's part of a save state: the RAM and the devices on the bus, in
//! sections. Every field of `Bus` is named here, as saved in one of the
//! sections or as left alone, so a field added to it fails to compile
//! until it is one or the other.

use super::Bus;
use crate::savestate::{Reader, Result, State, StateError, Writer, load_device, save_device};

/// The sections' versions, changed with what a section holds.
const RAM_VERSION: u16 = 1;
const CORE_VERSION: u16 = 1;
const VIDEO_VERSION: u16 = 1;
const SOUND_VERSION: u16 = 1;
const DOS_VERSION: u16 = 1;

/// Save or load each of a list of fields.
macro_rules! save_all {
    ($w:expr; $($f:expr),* $(,)?) => { $( State::save($f, $w); )* };
}
macro_rules! load_all {
    ($r:expr; $($f:expr),* $(,)?) => { $( State::load($f, $r)?; )* };
}

impl Bus {
    /// Write the bus's sections.
    pub(crate) fn save_state(&self, w: &mut Writer) {
        let Bus {
            ram,
            keyboard_buffer,
            kbd,
            kbc,
            a20_mask,
            cmos,
            pit_divisor,
            pit_read_msb,
            pit_mode,
            pit_write_msb,
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
            retraces,
            last_flip,
            gate_array_shadow,
            gate_array_shadow_at,
            opl,
            sb,
            mpu,
            gus,
            gus_line,
            tandy_sound,
            lpt_dac,
            cdaudio,
            xms,
            ems,
            umb,
            mouse,
            mscdex,
            disk,
            disk_io,
            // Set from the configuration, which a state carries in its
            // header.
            tandy_mode: _,
            ultrasnd_drive: _,
            // Worked out again after a load.
            irq_levels: _,
            irq_ready: _,
            page_gen: _,
            // Requests to the front end, which it carries out before a
            // state can be saved.
            reset_requested: _,
            config_ui_requested: _,
            exit_requested: _,
            mixer_changed: _,
            // The host's: its output, its settings and what it shows.
            freezes: _,
            frames_drawn: _,
            debug_console: _,
            start_time: _,
            audio_device: _,
            log_file: _,
            disknoise: _,
            drives_active: _,
            audio_out: _,
            audio_feed: _,
            mixer: _,
            audio_peak: _,
            audio_underruns: _,
            unhandled_writes: _,
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
            );
        });
        w.section(b"VIDE", VIDEO_VERSION, |w| {
            save_all!(w; video_mode, vga, vbe, retraces, last_flip, gate_array_shadow, gate_array_shadow_at);
        });
        w.section(b"DOS ", DOS_VERSION, |w| {
            save_all!(w; xms, mouse, mscdex, disk_io);
            save_device(ems, w);
            save_device(umb, w);
            disk.save_state(w);
        });
        // The Ultrasound's memory first, where it stays in place for
        // rewind's deltas (rewind.rs) whatever the queues after it hold.
        w.section(b"SOUN", SOUND_VERSION, |w| {
            save_device(gus, w);
            save_all!(w; opl, mpu, gus_line, tandy_sound);
            save_device(sb, w);
            save_device(lpt_dac, w);
            cdaudio.save_state(w);
        });
    }

    /// Read the bus's sections into it, in place: the RAM keeps its
    /// allocation, and must be as large as the saved one. Returns the
    /// files open in the state that couldn't be opened again.
    pub(crate) fn load_state(&mut self, r: &mut Reader) -> Result<Vec<String>> {
        let Bus {
            ram,
            keyboard_buffer,
            kbd,
            kbc,
            a20_mask,
            cmos,
            pit_divisor,
            pit_read_msb,
            pit_mode,
            pit_write_msb,
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
            retraces,
            last_flip,
            gate_array_shadow,
            gate_array_shadow_at,
            opl,
            sb,
            mpu,
            gus,
            gus_line,
            tandy_sound,
            lpt_dac,
            cdaudio,
            xms,
            ems,
            umb,
            mouse,
            mscdex,
            disk,
            disk_io,
            tandy_mode: _,
            ultrasnd_drive: _,
            irq_levels: _,
            irq_ready: _,
            page_gen: _,
            reset_requested: _,
            config_ui_requested: _,
            exit_requested: _,
            mixer_changed: _,
            freezes: _,
            frames_drawn: _,
            debug_console: _,
            start_time: _,
            audio_device: _,
            log_file: _,
            disknoise: _,
            drives_active: _,
            audio_out: _,
            audio_feed: _,
            mixer: _,
            audio_peak: _,
            audio_underruns: _,
            unhandled_writes: _,
            log_hook: _,
            audio_hook: _,
        } = self;
        let mut section = r.section(b"RAM ", RAM_VERSION)?;
        let len = section.count()?;
        if len != ram.len() {
            return Err(StateError::Mismatch(format!(
                "it has {} KB of memory, this machine {} KB",
                len / 1024,
                ram.len() / 1024
            )));
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
        );
        let mut section = r.section(b"VIDE", VIDEO_VERSION)?;
        load_all!(&mut section; video_mode, vga, vbe, retraces, last_flip, gate_array_shadow, gate_array_shadow_at);
        // The drives before the sound: the CD playing is in one.
        let mut section = r.section(b"DOS ", DOS_VERSION)?;
        load_all!(&mut section; xms, mouse, mscdex, disk_io);
        load_device(ems, "expanded memory manager", &mut section)?;
        load_device(umb, "upper memory", &mut section)?;
        let lost = disk.load_state(&mut section)?;
        let mut section = r.section(b"SOUN", SOUND_VERSION)?;
        load_device(gus, "Gravis Ultrasound", &mut section)?;
        load_all!(&mut section; opl, mpu, gus_line, tandy_sound);
        load_device(sb, "Sound Blaster", &mut section)?;
        load_device(lpt_dac, "DAC on LPT1", &mut section)?;
        cdaudio.load_state(&mut section, |drive| disk.cd_image(drive))?;
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
        self.mixer.clear_tails();
        self.audio_out.clear();
    }

    /// `after_load` for a state loaded into the machine that saved it,
    /// whose FM chip, synthesizer and sound output are still those of the
    /// state.
    pub(crate) fn after_reload(&mut self) {
        self.page_gen.fill(0);
        self.update_irq_levels();
        self.vga.after_load();
    }
}
