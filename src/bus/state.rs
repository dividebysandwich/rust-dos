//! The bus's part of a save state: the RAM and the devices on the bus, in
//! sections. Every field of `Bus` is named here, as saved in one of the
//! sections or as left alone, so a field added to it fails to compile
//! until it is one or the other.

use super::Bus;
use crate::savestate::{Reader, Result, State, StateError, Writer, load_device, save_device};

/// The sections' versions, changed with what a section holds.
const RAM_VERSION: u16 = 2;
const CORE_VERSION: u16 = 11;
/// The CORE versions this rust-dos loads: its own, 9, which kept the
/// phase of the speaker's square wave where 10 and 11 keep its level, and
/// 10, which keeps the time high in the frame being made where 11 keeps
/// it in the eighth of the frame being made (`Bus::speaker_high`).
const CORE_LOADS: [u16; 3] = [9, 10, CORE_VERSION];
const VIDEO_VERSION: u16 = 2;
const SOUND_VERSION: u16 = 5;
/// The SOUN versions this rust-dos loads: its own, and 4, which has no
/// right OPL2 of a dual OPL2 (`Opl::save_right`): on a machine with one,
/// it gets what the one chip's second bank held (`Opl::right_from_bank_1`).
const SOUND_LOADS: [u16; 2] = [4, SOUND_VERSION];
const DOS_VERSION: u16 = 6;
const CD_HEAD_VERSION: u16 = 1;
const VCPI_VERSION: u16 = 1;
const VOODOO_VERSION: u16 = 1;
const POWERVR_VERSION: u16 = 1;
const IDE_VERSION: u16 = 3;
const FDC_VERSION: u16 = 1;
const DPMI_VERSION: u16 = 2;
const WIN32_VERSION: u16 = 86;
/// The W32 versions this rust-dos loads: its own, and those an older one
/// saved. 71's keeps only the names of the font files a program added
/// (`gdi32::fnt::AddedFile`), 71's and 72's not the desktop's own size
/// (`version::OwnSize`), 71's to 73's MMSYSTEM's wave headers in an
/// older form and no callback calls (`win16::Mmsystem`), 71's to 74's
/// not DDEML's 16-bit copies, user handles and asynchronous transactions
/// (`ddeml::DDE_SAVED`), 71's to 75's no DirectPlay 8 objects
/// (`dpnet::State`), 71's to 76's not OLE32's local servers and proxies
/// (`ole32::LOCAL_SERVERS`), 71's to 77's not owner-drawn menu items'
/// sizes and drawing (`user32::menu::OWNER_DRAWN`), 71's to 78's no
/// system heaps (`heap::SYSTEM_HEAP_SAVED`), 71's to 79's no Toolhelp32
/// snapshots (`object::Kind::Snapshot`, which they load without), 71's
/// to 80's not the TrueType fonts of the files a program added
/// (`gdi32::fnt::TRUETYPE_SAVED`), 71's to 81's no drop targets
/// (`ole32::DROP_TARGETS`), 71's to 82's no ExitWindowsExec command
/// line (`boot::EXIT_EXEC`), 71's to 83's not the 16-bit OLE modules'
/// state (`win16::ole2::OLE16`), 71's to 84's no DOS program a
/// Windows program started (`dosvm::DOSVM_SAVED`), and 85's not the
/// folder it starts in (`dosvm::DOSVM_DIRECTORY`).
const WIN32_LOADS: [u16; 16] = [
    71,
    72,
    73,
    74,
    75,
    76,
    77,
    78,
    79,
    80,
    81,
    82,
    83,
    84,
    85,
    WIN32_VERSION,
];
const NET_VERSION: u16 = 2;
const SERIAL_VERSION: u16 = 2;
const VFX1_VERSION: u16 = 1;
const AWE_VERSION: u16 = 1;
const ADLIB_GOLD_VERSION: u16 = 1;
const VIRGE_VERSION: u16 = 1;
const VERITE_VERSION: u16 = 3;
const ET4000_VERSION: u16 = 1;
const MACH64_VERSION: u16 = 1;
const TOADFORCE_VERSION: u16 = crate::toadforce::STATE_VERSION;
const SHARED_VERSION: u16 = 1;
const SB_HIGH_SPEED_VERSION: u16 = 1;
const ISAPNP_VERSION: u16 = 1;
const PIT2_VERSION: u16 = 1;
/// The 8254's parts that states saved before it lack: where a mode 3
/// count loaded at a half-cycle starts, the control words' bits the
/// read-back status gives, the status latches and channel 1.
const PIT_8254_VERSION: u16 = 1;
const TSC_VERSION: u16 = 1;
const KEY_QUEUE_VERSION: u16 = 1;
const TYPEMATIC_VERSION: u16 = 1;
const FRAME_CAP_VERSION: u16 = 1;

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
            frame_cap,
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
            pit0_status,
            pit0,
            pit1,
            pit1_port,
            clock,
            pic,
            dma,
            joystick,
            vfx1,
            dta_segment,
            dta_offset,
            sda_written: _,
            search_handles,
            search_serial,
            cursor_x,
            cursor_y,
            post_code,
            refresh_toggle,
            speaker_on,
            speaker_high,
            speaker_until,
            audio_frames,
            sb_frame,
            beep_frames,
            video_mode,
            vga,
            vbe,
            s3_engine,
            virge,
            verite,
            mach64,
            toadforce,
            toadforce_memory,
            retraces,
            last_flip,
            last_present: _,
            gate_array_shadow,
            gate_array_shadow_at,
            opl,
            sb,
            game_fixes: _,
            game_fixes_off: _,
            isapnp,
            mpu,
            gus,
            gus_line,
            adlib_gold,
            adlib_gold_line,
            awe,
            awe_rom: _,
            awe_ram_kb: _,
            tandy_sound,
            lpt_dac,
            cdaudio,
            xms,
            ems,
            vcpi,
            dpmi,
            win32,
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
            host_frames: _,
            key_waits: _,
            unhandled_writes: _,
            limited_logs: _,
            attribute_reset: _,
            memory_derived: _,
            cpu_derived: _,
            log_hook: _,
            log_backlog: _,
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
                post_code, refresh_toggle, speaker_on, speaker_high, speaker_until,
                audio_frames, sb_frame, beep_frames, boot, pci,
            );
        });
        // Channel 2's counter and gate, in a section of their own so states
        // saved before it load. The fields after `speaker_data` came later:
        // a section that ends before them loads with their defaults.
        let pit2_count_due = &pit2.count_due;
        w.section(b"PIT2", PIT2_VERSION, |w| {
            save_all!(w; pit2, speaker_data, pit2_count_due, ppi_checks);
        });
        // The 8254's read-back state, channel 1 and the half-cycle loads,
        // in a section of their own so states saved before it load.
        // Channel 1's phase, load tick and control word go with it, which
        // `Pit0`'s own fields leave out as channel 0's CORE layout does.
        let (pit0_phase, pit0_pending_at, pit0_control) =
            (&pit0.phase, &pit0.pending_at, &pit0.control);
        let (pit1_phase, pit1_pending_at, pit1_control) =
            (&pit1.phase, &pit1.pending_at, &pit1.control);
        let (pit2_phase, pit2_control, pit2_status) = (&pit2.phase, &pit2.control, &pit2.status);
        w.section(b"8254", PIT_8254_VERSION, |w| {
            save_all!(w;
                pit0_phase, pit0_pending_at, pit0_control, pit0_status, pit1, pit1_port,
                pit1_phase, pit1_pending_at, pit1_control,
                pit2_phase, pit2_control, pit2_status,
            );
        });
        // The time stamp counter's clock, in a section of its own so states
        // saved before it load (counting instructions, as it did then).
        w.section(b"TSC ", TSC_VERSION, |w| clock.tsc.save(w));
        // Which queued keystrokes the BIOS never saw, in a section of its
        // own so states saved before it load.
        w.section(b"KEYQ", KEY_QUEUE_VERSION, |w| kbd.save_queue(w));
        // The keyboard's typematic rate and the key it repeats, likewise.
        w.section(b"TYPM", TYPEMATIC_VERSION, |w| {
            kbd.save_typematic(kbc.typematic, w)
        });
        w.section(b"VIDE", VIDEO_VERSION, |w| {
            save_all!(w; video_mode, vga);
            // A ToadForce's memory is saved with what its host renderer
            // drew read back, into a copy: saving changes nothing.
            w.label("vbe");
            match toadforce.as_deref().and_then(|card| card.settled(&vbe.vram)) {
                Some(memory) => vbe.save_with(&memory, w),
                None => State::save(vbe, w),
            }
            save_all!(w; s3_engine, retraces, last_flip, gate_array_shadow, gate_array_shadow_at);
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
        // An ATI mach64's registers and engine, only on one, likewise.
        if vga.adapter.is_mach64() {
            w.section(b"MC64", MACH64_VERSION, |w| mach64.save(w));
        }
        // A ToadForce's registers and ring, only on one, likewise. Its
        // memory is the Super VGA's, in the section above. How much it
        // has is the machine's setting, which the card saved carries.
        let _ = toadforce_memory;
        if let Some(toadforce) = toadforce {
            w.section(b"TOAD", TOADFORCE_VERSION, |w| toadforce.save(w));
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
        // Where MSCDEX's drives' heads are, in a section of their own so
        // states saved before it load (with every head at sector 0).
        w.section(b"CDHD", CD_HEAD_VERSION, |w| mscdex.save_heads(w));
        // VCPI's, in a section of its own so states saved before it load
        // (with none of its pages handed out).
        w.section(b"VCPI", VCPI_VERSION, |w| vcpi.save(w));
        // The Ultrasound's memory first, where it stays in place for
        // rewind's deltas (rewind.rs) whatever the queues after it hold.
        w.section(b"SOUN", SOUND_VERSION, |w| {
            w.label("gus");
            save_device(gus, w);
            save_all!(w; opl, mpu, gus_line, tandy_sound);
            w.label("opl_right");
            opl.save_right(w);
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
        // The AdLib Gold, likewise.
        if let Some(gold) = adlib_gold {
            w.section(b"AGLD", ADLIB_GOLD_VERSION, |w| {
                gold.save(w);
                adlib_gold_line.save(w);
            });
        }
        // A 3dfx card, whose memory comes first, and only on a machine
        // with one, so states of machines without load as they did.
        if let Some(voodoo) = voodoo {
            w.section(b"3DFX", VOODOO_VERSION, |w| voodoo.save(w));
        }
        // The frame rate cap's last frame, which a 3dfx card's section
        // held before every card's present went through the cap: in a
        // section of its own, on every machine, so states saved before it
        // load (with the 3dfx card's, if they have one).
        w.section(b"PACE", FRAME_CAP_VERSION, |w| frame_cap.save(w));
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
        // The built-in Windows, while it runs a program or is about to.
        if win32.active() || win32.pending() {
            w.section(b"W32 ", WIN32_VERSION, |w| win32.save(w));
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
        // The VFX1's card and driver, only with one.
        if let Some(vfx1) = vfx1 {
            w.section(b"VFX1", VFX1_VERSION, |w| vfx1.save(w));
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
            frame_cap,
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
            pit0_status,
            pit0,
            pit1,
            pit1_port,
            clock,
            pic,
            dma,
            joystick,
            vfx1,
            dta_segment,
            dta_offset,
            sda_written: _,
            search_handles,
            search_serial,
            cursor_x,
            cursor_y,
            post_code,
            refresh_toggle,
            speaker_on,
            speaker_high,
            speaker_until,
            audio_frames,
            sb_frame,
            beep_frames,
            video_mode,
            vga,
            vbe,
            s3_engine,
            virge,
            verite,
            mach64,
            toadforce,
            toadforce_memory,
            retraces,
            last_flip,
            last_present,
            gate_array_shadow,
            gate_array_shadow_at,
            opl,
            sb,
            game_fixes: _,
            game_fixes_off: _,
            isapnp,
            mpu,
            gus,
            gus_line,
            adlib_gold,
            adlib_gold_line,
            awe,
            awe_rom: _,
            awe_ram_kb: _,
            tandy_sound,
            lpt_dac,
            cdaudio,
            xms,
            ems,
            vcpi,
            dpmi,
            win32,
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
            host_frames: _,
            key_waits: _,
            unhandled_writes: _,
            limited_logs: _,
            attribute_reset: _,
            // The front end's, which sets them from the settings a load
            // leaves (`slots::loaded_settings`).
            memory_derived: _,
            cpu_derived: _,
            log_hook: _,
            log_backlog: _,
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
        let mut section = r.section_of(b"CORE", &CORE_LOADS)?;
        load_all!(&mut section;
            keyboard_buffer, kbd, kbc, a20_mask, cmos,
            pit_divisor, pit_read_msb, pit_mode, pit_write_msb,
            pit0_divisor, pit0_write_msb, pit0_read_msb, pit0_access, pit0_latched, pit0_latched_active,
            pit0, clock, pic, dma, joystick,
            dta_segment, dta_offset, search_handles, search_serial, cursor_x, cursor_y,
            post_code, refresh_toggle, speaker_on,
        );
        if section.version() == Some(9) {
            // The square wave's phase, which the speaker's level replaced:
            // the frame being made starts at the frames mixed.
            let mut phase = 0.0f32;
            load_all!(&mut section; &mut phase, audio_frames);
            *speaker_high = 0.0;
            *speaker_until = super::frame_tick(*audio_frames);
        } else {
            load_all!(&mut section; speaker_high, speaker_until, audio_frames);
            if section.version() == Some(10) {
                // The time high since the frame began: its mean over that
                // time, over the eighth being made.
                let (part, frame) = super::speaker_starts(*speaker_until);
                let span = *speaker_until - frame;
                let mean = if span > 0.0 {
                    *speaker_high / span
                } else {
                    0.0
                };
                *speaker_high = mean * (*speaker_until - part);
            }
        }
        load_all!(&mut section; sb_frame, beep_frames, boot, pci);
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
        if r.next_is(b"8254") {
            let mut section = r.section(b"8254", PIT_8254_VERSION)?;
            load_all!(&mut section;
                &mut pit0.phase, &mut pit0.pending_at, &mut pit0.control, pit0_status, pit1, pit1_port,
                &mut pit1.phase, &mut pit1.pending_at, &mut pit1.control,
                &mut pit2.phase, &mut pit2.control, &mut pit2.status,
            );
        } else {
            // An older state: counts loaded at the end of a period, which
            // is where it kept a pending channel 0 count, no status
            // latched, the control words' bits as the access and mode give
            // them (not BCD), and channel 1 as the BIOS leaves it.
            pit0.without_8254_state(*pit0_access);
            *pit0_status = None;
            *pit1 = crate::timer::Pit0::refresh();
            *pit1_port = crate::timer::PitPort::new(1);
            pit2.without_8254_state();
        }
        clock.tsc = Default::default();
        if r.next_is(b"TSC ") {
            clock.tsc.load(&mut r.section(b"TSC ", TSC_VERSION)?)?;
        }
        if r.next_is(b"KEYQ") {
            kbd.load_queue(&mut r.section(b"KEYQ", KEY_QUEUE_VERSION)?)?;
        } else {
            kbd.forget_queue();
        }
        kbc.typematic = if r.next_is(b"TYPM") {
            kbd.load_typematic(&mut r.section(b"TYPM", TYPEMATIC_VERSION)?)?
        } else {
            kbd.forget_typematic()
        };
        let mut section = r.section(b"VIDE", VIDEO_VERSION)?;
        load_all!(&mut section; video_mode, vga, vbe, s3_engine, retraces, last_flip, gate_array_shadow, gate_array_shadow_at);
        *last_present = None;
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
        mach64.reset();
        mach64.rage2 = vga.adapter == crate::video::adapter::Adapter::AtiRage2;
        if r.next_is(b"MC64") {
            mach64.load(&mut r.section(b"MC64", MACH64_VERSION)?)?;
        }
        match (r.next_is(b"TOAD"), toadforce) {
            (true, Some(toadforce)) => {
                toadforce.load(&mut r.section(b"TOAD", TOADFORCE_VERSION)?)?;
                // The card is the state's, with the memory it had there.
                if toadforce.memory() as usize != vbe.vram.len() {
                    let what = "a ToadForce whose memory isn't its size";
                    return Err(StateError::Invalid(what.into()));
                }
                *toadforce_memory = toadforce.memory();
            }
            (false, None) => {}
            (true, None) => {
                let what = "it has a ToadForce and this machine hasn't";
                return Err(StateError::Mismatch(what.into()));
            }
            (false, Some(_)) => {
                let what = "this machine has a ToadForce and it hasn't";
                return Err(StateError::Mismatch(what.into()));
            }
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
        if r.next_is(b"CDHD") {
            mscdex.load_heads(Some(&mut r.section(b"CDHD", CD_HEAD_VERSION)?))?;
        } else {
            mscdex.load_heads(None)?;
        }
        if r.next_is(b"VCPI") {
            vcpi.load(&mut r.section(b"VCPI", VCPI_VERSION)?)?;
        } else {
            *vcpi = crate::vcpi::Vcpi::default();
        }
        let mut section = r.section_of(b"SOUN", &SOUND_LOADS)?;
        load_device(gus, "Gravis Ultrasound", &mut section)?;
        load_all!(&mut section; opl, mpu, gus_line, tandy_sound);
        if section.version() >= Some(5) {
            opl.load_right(&mut section)?;
        } else {
            opl.right_from_bank_1();
        }
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
        match (r.next_is(b"AGLD"), adlib_gold) {
            (true, Some(gold)) => {
                let mut section = r.section(b"AGLD", ADLIB_GOLD_VERSION)?;
                gold.load(&mut section)?;
                adlib_gold_line.load(&mut section)?;
            }
            (false, None) => {}
            (true, None) => return Err(StateError::Mismatch("it has an AdLib Gold and this machine hasn't".into())),
            (false, Some(_)) => return Err(StateError::Mismatch("this machine has an AdLib Gold and it hasn't".into())),
        }
        // A state saved before the cap had its section has the 3dfx
        // card's last capped frame, which the card's load puts in.
        frame_cap.forget();
        match (r.next_is(b"3DFX"), voodoo) {
            (true, Some(voodoo)) => voodoo.load(&mut r.section(b"3DFX", VOODOO_VERSION)?)?,
            (false, None) => {}
            (true, None) => return Err(StateError::Mismatch("it has a 3dfx card and this machine hasn't".into())),
            (false, Some(_)) => return Err(StateError::Mismatch("this machine has a 3dfx card and it hasn't".into())),
        }
        if r.next_is(b"PACE") {
            frame_cap.load(&mut r.section(b"PACE", FRAME_CAP_VERSION)?)?;
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
        win32.reset();
        if r.next_is(b"W32 ") {
            win32.load(&mut r.section_of(b"W32 ", &WIN32_LOADS)?)?;
        }
        // The extended memory they hold.
        xms.dpmi = dpmi.reservations();
        xms.dpmi.extend(win32.reservations());
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
        // The VFX1's card is the settings': a state's goes to it if it is
        // there, and a state without one leaves it as it was set up.
        if r.next_is(b"VFX1") {
            let mut section = r.section(b"VFX1", VFX1_VERSION)?;
            if let Some(vfx1) = vfx1 {
                vfx1.load(&mut section)?;
            }
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
        // The processes' stubs from older tables call what their programs
        // were bound to.
        crate::win32::stub_tables::mend(self)?;
        // The Win16 KERNEL's stubs for imports nothing has keep their ids
        // when KERNEL's table grows.
        crate::win16::builtin::mend(self)?;
        Ok(lost)
    }

    /// Bring what is worked out from the saved state up to date after a
    /// load, once the CPU's caches are empty: the code generations start
    /// again from nothing (so machines loaded from one state are alike),
    /// the interrupt lines follow the devices, the picture is drawn anew,
    /// the FM chip and the MIDI synthesizer are told their registers and
    /// setup again, and the sound rendered before the load is dropped with
    /// what rang on of it in the mixer, and the idle skipper forgets the
    /// loops of the run before the load.
    pub(crate) fn after_load(&mut self) {
        self.after_reload();
        self.observe.after_load();
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
        self.kbc.after_load();
        self.net.nic_changed();
        // What the machine last wrote into its SDA isn't what the state's
        // SDA holds: until the next DOS call writes it, nothing in it
        // counts as the program's.
        self.sda_written = None;
    }

    /// `after_load` for a state loaded into the machine that saved it,
    /// whose FM chip, synthesizer and sound output are still those of the
    /// state.
    pub(crate) fn after_reload(&mut self) {
        // The drives the state mounted as it had them.
        self.log_disk_notes();
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
        // A Banshee's video timing, which its swaps wait on, isn't in the
        // state: it comes from its VGA core's registers.
        if self.banshee().is_some() {
            self.banshee_settle();
        }
        self.toadforce_after_load();
        if let Some(awe) = &mut self.awe {
            awe.after_load();
        }
        self.verite_after_load();
        // The mouse's mickeys count for the state's picture and range.
        crate::mouse::update_mickey_scale(self);
        // The built-in DLLs' files are of the version of Windows the
        // state's programs run under, which may not be the settings'.
        crate::win32::dll_files::sync(self);
        // The system's time starts from the machine's clock again, which
        // isn't in the state (`kernel32::time::now`).
        self.win32.system_clock = None;
        // When the first of the state's threads' timeouts ends, which
        // isn't in the state either. The clock's deadline is in the state,
        // so it is worked out again only when a thread has such a wait:
        // a machine without one saves the same bytes it was loaded from.
        let now = self.clock.now_micros();
        self.win32.wait_due = crate::win32::thread::wait_due(&self.win32, now);
        if self.win32.wait_due.is_some() {
            self.reschedule_timers();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::WIN32_VERSION;
    use std::path::Path;

    /// The layouts the W32 section has had, oldest first: its version and
    /// `win32_layout()` of that version's code. Changing a saved list
    /// means raising WIN32_VERSION and adding a line here. A version
    /// raised for something the hash doesn't see adds a line with the
    /// same hash as the one before it. Keep the old lines: two branches
    /// that each take the same next version then add different lines at
    /// the same place, and git stops the second merge, where the two
    /// equal changes to WIN32_VERSION alone merge without a word. The
    /// comment on a line says what the version changed, which keeps two
    /// lines with an unchanged hash different as well.
    const WIN32_LAYOUTS: &[(u16, u64)] = &[
        (21, 0x4413_3346_5643_A054), // Video for Windows, last of three
        (22, 0x1329_1DAD_33BC_DC4A), // dinput's app_data
        (23, 0x1329_1DAD_33BC_DC4A), // the mixer's seven lines
        (24, 0xF61B_D786_C674_4FD3), // WNASPI32 and its pending requests
        (25, 0xABF4_8D5F_2FB4_2680), // OLEAUT32's type libraries and error objects
        (26, 0xEFE8_9E7A_456D_99B4), // the boot's work and the program pending
        (27, 0x48DE_57AA_C42E_36F2), // the MPEGVideo and AVIVideo devices
        (28, 0x8D14_5D81_BEAF_AC09), // DirectShow's objects and graph devices
        (29, 0x6A10_30CA_74D4_A5B0), // DirectPlay's connections and sessions
        (30, 0xC6E5_70EA_CFE3_C03E), // Glide 3's vertex layout, viewport and strips
        (31, 0xA966_371F_B41A_E64E), // MSACM32's streams
        (32, 0xC2B1_4444_B41A_6AA5), // when an OpenGL window's next swap may show
        (33, 0xC498_7C01_FF00_3137), // the threads higher ones preempted
        (34, 0x2A81_5B37_A1B7_C90B), // DirectShow's program filters, allocators and clocks
        (35, 0xF4AA_E5DA_D534_7E8F), // which Win16 modules' own loaders are yet to run
        (36, 0xFB13_32FF_2A2F_4AC7), // each thread's debug registers
        (37, 0x9341_EE06_0BDB_0188), // an AVIVideo device's palette and MCI_REALIZE's background
        (38, 0xEED4_AAD5_91F9_E9C2), // Win16's open serial ports
        (39, 0xEC3D_4AFA_6107_B768), // the screens kept under CS_SAVEBITS windows
        (40, 0x7030_77A9_A34C_1A60), // the Win16 task a message handed the turn to
        (41, 0x6147_C450_7D40_D8CF), // the codecs a process opened with ICOpen
        (42, 0x7540_74DC_C874_40DC), // change notification handles
        (43, 0x1974_9F8E_624E_4E47), // DirectShow's joints and media type enumerators
        (44, 0xBE96_CD5A_0B06_51EC), // the program's allocator a DirectShow link uses
        (45, 0xBE96_CD5A_0B06_51EC), // a file mapping's views keep their protection
        (46, 0x5C6F_53A0_DDD4_1506), // ASPI requests' due times, the drive's head
        (47, 0x5C6F_53A0_DDD4_1506), // MSVCRT's variables a page further on
        (48, 0x39FB_7FAD_F632_9763), // OpenGL's lighting state and attribute stack
        (49, 0x39FB_7FAD_F632_9763), // what a qsort or bsearch in progress keeps
        (50, 0x39FB_7FAD_F632_9763), // MSVCRT's descriptors in ioinfo blocks
        (51, 0x85E6_4D93_8609_5176), // the ToadForce HAL's copies of textures
        (52, 0xB8ED_A219_CC6C_C4BC), // a device made as the T&L HAL device
        (53, 0x59CC_53A1_8176_269C), // a filter graph's full-screen mode
        (54, 0xB1D0_B934_F547_E963), // Direct3D 8's cube textures
        (55, 0x33AE_9F33_F125_1478), // a video device's full-screen play
        (56, 0x0BF5_7A03_9493_E7DE), // TrackMouseEvent's request, one for the desktop
        (57, 0xB847_1A2D_27D3_3646), // OpenGL's draws that wait
        (58, 0xDC7D_5C01_8FC2_8FD2), // the system locale
        (59, 0xCDE1_505E_0044_1486), // the page file behind programs' own memory
        (61, 0x9267_6BA0_CE58_B06F), // OpenGL's display lists
        (62, 0x9F78_ACB5_CCD2_8975), // OpenGL's textures in their contexts' group
        (63, 0xD1E3_14A4_31DE_2AAA), // OpenGL's vertices as given, and the card's lights
        (65, 0x0208_3289_86A2_082E), // Win32's serial ports
        (67, 0xD1E3_14A4_31DE_2AAA), // a Direct3D device made as a software device
        (69, 0x0328_CB26_D841_A2ED), // Win32's serial ports, merged with 63's and 67's
        (71, 0x0DD2_950E_C2FB_1628), // copy-on-write views paged, pages held for a fault
        (72, 0x0DD2_950E_C2FB_1628), // the fonts of files AddFontResource added
        (73, 0x2412_6CA7_250D_4193), // the desktop's own size, `win32_desktop`
        (74, 0x6C70_EAD3_11EB_EDB6), // MMSYSTEM's headers keep lpData, callbacks
        (75, 0x9B62_A189_49CB_DB03), // DDEML's 16-bit copies, user handles and transactions
        (76, 0xF698_DBEB_B887_D533), // DirectPlay 8's objects
        (77, 0xA5E9_A942_6FF9_9965), // OLE32's local servers, proxies and stubs
        (78, 0x9A03_DEE6_921B_EE10), // owner-drawn menu items' sizes and drawing
        (79, 0x3960_B363_99FF_D384), // each process's system heap
        (80, 0x2471_174C_D479_E2F6), // Toolhelp32 snapshots
        (81, 0x2471_174C_D479_E2F6), // the TrueType fonts of files AddFontResource added
        (82, 0x7F8F_BB1B_6F82_7B99), // OLE32's drop targets and OleInitialize's processes
        (83, 0xE951_CEB4_D9AD_4CAC), // ExitWindowsExec's command line
        (84, 0x3A9C_D1FD_6601_1D0E), // the 16-bit OLE modules' state
        (85, 0xD065_06B4_D1EE_C645), // the DOS program a Windows program started
        (86, 0x64E2_699F_8225_8EB3), // the DOS program's folder and lent memory
    ];

    /// The text between the parentheses of `macro_name!(...)` at `from`.
    fn macro_body(source: &str, from: usize) -> Option<&str> {
        let start = from + source[from..].find('(')? + 1;
        let mut depth = 1;
        for (i, c) in source[start..].char_indices() {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                return Some(&source[start..start + i]);
            }
        }
        None
    }

    /// The lists of saved fields in the Rust source `source`: what each
    /// `state_fields!` saves and each `state_enum!` or `state_variants!`
    /// numbers, without comments, spaces or the fields it skips.
    fn saved_lists(source: &str) -> Vec<String> {
        let code: String = source
            .lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        let mut lists = Vec::new();
        for name in ["state_fields!", "state_enum!", "state_variants!"] {
            for (at, _) in code.match_indices(name) {
                let Some(body) = macro_body(&code, at) else {
                    continue;
                };
                let body: String = body.chars().filter(|c| !c.is_whitespace()).collect();
                let saved = body.find("}skip{").map_or(&body[..], |end| &body[..=end]);
                lists.push(saved.to_string());
            }
        }
        lists
    }

    fn collect(dir: &Path, lists: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect(&path, lists);
            } else if path.extension().is_some_and(|e| e == "rs") {
                lists.extend(saved_lists(&std::fs::read_to_string(&path).unwrap()));
            }
        }
    }

    /// FNV-1a of the saved lists of `src/win32` and of `src/win16`, which
    /// the W32 section holds as `Win32::win16`, in sorted order, so moving
    /// a type to another file doesn't change it. It doesn't see a field
    /// that changes its type, `dinput::poll::MouseSeen`'s hand-written
    /// `State`, or what the section saves from elsewhere, such as
    /// `dpmi::Context`.
    fn win32_layout() -> u64 {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut lists = Vec::new();
        collect(&src.join("win32"), &mut lists);
        collect(&src.join("win16"), &mut lists);
        lists.sort();
        lists
            .join("\n")
            .bytes()
            .fold(0xCBF2_9CE4_8422_2325, |hash: u64, byte| {
                (hash ^ byte as u64).wrapping_mul(0x100_0000_01B3)
            })
    }

    #[test]
    fn a_changed_win32_layout_has_its_own_version() {
        for pair in WIN32_LAYOUTS.windows(2) {
            assert!(
                pair[0].0 < pair[1].0,
                "two layouts of the W32 section can't share version {}: take the next one",
                pair[1].0
            );
        }
        let &(version, layout) = WIN32_LAYOUTS.last().unwrap();
        let now = win32_layout();
        assert_eq!(
            (version, layout),
            (WIN32_VERSION, now),
            "WIN32_LAYOUTS doesn't end with the W32 section as it is: raise WIN32_VERSION \
             past {version} if it is still there, and add its version with {now:#X} to \
             WIN32_LAYOUTS, leaving the lines above it",
        );
    }

    #[test]
    fn saved_lists_leave_out_comments_and_skipped_fields() {
        let source = "// state_fields!(Old { a });\n\
                      crate::state_fields!(Thing {\n    a, // first\n    b,\n} \
                      skip {\n    c,\n});\n\
                      crate::state_fields!(Other { d }\nskip { e });\n\
                      crate::win32::state_variants!(Kind {\n    A { x },\n    B {},\n});\n";
        assert_eq!(
            saved_lists(source),
            ["Thing{a,b,}", "Other{d}", "Kind{A{x},B{},}"]
        );
    }
}
