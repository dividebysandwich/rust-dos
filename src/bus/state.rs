//! The bus's part of a save state: the RAM and the devices on the bus, in
//! sections. Every field of `Bus` is named here, as saved in one of the
//! sections or as left alone, so a field added to it fails to compile
//! until it is one or the other.

use super::Bus;
use crate::savestate::{Reader, Result, State, StateError, Writer};

/// The sections' versions, changed with what a section holds.
const RAM_VERSION: u16 = 1;
const CORE_VERSION: u16 = 1;
const VIDEO_VERSION: u16 = 1;

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
            // Not saved yet: the sound devices and DOS's memory managers,
            // drives and devices.
            opl: _,
            sb: _,
            mpu: _,
            gus: _,
            gus_line: _,
            tandy_sound: _,
            lpt_dac: _,
            cdaudio: _,
            xms: _,
            ems: _,
            umb: _,
            mouse: _,
            mscdex: _,
            disk: _,
            disk_io: _,
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
    }

    /// Read the bus's sections into it, in place: the RAM keeps its
    /// allocation, and must be as large as the saved one.
    pub(crate) fn load_state(&mut self, r: &mut Reader) -> Result<()> {
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
            opl: _,
            sb: _,
            mpu: _,
            gus: _,
            gus_line: _,
            tandy_sound: _,
            lpt_dac: _,
            cdaudio: _,
            xms: _,
            ems: _,
            umb: _,
            mouse: _,
            mscdex: _,
            disk: _,
            disk_io: _,
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
        Ok(())
    }

    /// Bring what is worked out from the saved state up to date after a
    /// load, once the CPU's caches are empty: the code generations start
    /// again from nothing (so machines loaded from one state are alike),
    /// the interrupt lines follow the devices, the picture is drawn anew
    /// and the sound rendered before the load is dropped.
    pub(crate) fn after_load(&mut self) {
        self.page_gen.fill(0);
        self.update_irq_levels();
        self.vga.after_load();
        self.audio_out.clear();
    }
}
