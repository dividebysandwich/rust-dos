use std::collections::VecDeque;
use web_time::Instant;

use crate::disk::{DiskController, DriveKind, LASTDRIVE, MountOptions};
use crate::video::vbe::Vbe;
use crate::video::{self, ADDR_VGA_GRAPHICS, SIZE_GRAPHICS, VideoMode};

/// ROM table of media descriptor bytes, one per drive letter. INT 21h
/// AH=1Bh/1Ch return a far pointer (F000:E900+drive) into it.
pub const MEDIA_ID_TABLE: usize = 0xFE900;
/// ROM table of DOS Drive Parameter Blocks for INT 21h AH=1Fh/32h, one
/// `DPB_SIZE` slot per drive letter.
pub const DPB_TABLE: usize = 0xFEA00;
pub const DPB_SIZE: usize = 0x40;
/// ROM copy of the DOS List of Lists (SYSVARS) that INT 21h AH=52h points
/// ES:BX at. The word just below it holds the first MCB segment.
pub const DOS_LIST_OF_LISTS: usize = 0xFF110;

pub trait Device {
    /// Return the set of I/O ports this device owns.
    ///
    /// Must return a `'static` slice rather than a freshly-allocated `Vec`,
    /// because the bus dispatcher calls this on *every* IO read/write and a
    /// heap allocation per port access would dominate the runtime of any
    /// program doing heavy VGA register work (palette writes, status polls).
    fn ports(&self) -> &'static [u16];
    fn io_read(&mut self, port: u16) -> u8;
    fn io_write(&mut self, port: u16, value: u8);
    fn step(&mut self) {}
}

/// RAM when the configuration doesn't say.
pub const DEFAULT_MEMORY_MB: usize = 16;

/// log2 of the bytes of RAM each code generation counter covers: small
/// enough that a program's variables rarely share a block with its code.
pub const GEN_SHIFT: usize = 6;

pub struct Bus {
    ram: Vec<u8>, // System RAM, allocated once
    pub video_mode: VideoMode, // Current State
    pub disk: DiskController,
    pub keyboard_buffer: VecDeque<u16>, // Stores (Scancode << 8) | ASCII
    /// The 8042 keyboard controller (ports 60h/64h): scan codes for
    /// programs that read the keyboard themselves, the A20 gate and the
    /// CPU reset line.
    pub kbc: crate::kbc::Kbc,
    /// The A20 gate as a mask for physical addresses: with the gate
    /// closed, address line 20 is forced to 0 and addresses wrap at 1 MB
    /// as on an 8086. See `a20()` and `set_a20()`.
    a20_mask: u32,
    /// Something asked for a CPU reset (8042 or port 92h); the execution
    /// loop carries it out.
    pub reset_requested: bool,
    /// The DOSCONFIG command asked for the settings window; the frontend
    /// opens it.
    pub config_ui_requested: bool,
    /// The EXIT command asked to turn the machine off; the frontend quits.
    pub exit_requested: bool,
    pub cmos: crate::cmos::Cmos,
    /// The XMS driver's allocations and A20 state.
    pub xms: crate::xms::Xms,
    /// Last POST code written to port 80h (or 190h, test ROMs).
    pub post_code: u8,
    /// Text written to port E9h, the Bochs debug console, which test ROMs
    /// and debugging builds of some programs print to (the first 16 MB).
    pub debug_console: Vec<u8>,
    /// Port 61h bit 4, the DRAM refresh request, toggles on every read;
    /// delay loops count the toggles.
    refresh_toggle: bool,
    pub cursor_x: usize,
    pub cursor_y: usize,
    pub start_time: Instant, // System timer
    /// Joystick port (0x0201) polling counter. Real hardware: a write to
    /// 0x0201 charges 4 RC one-shots; reads return each bit high until
    /// the timer expires, with the expiry time proportional to axis
    /// resistance. Games sample the joystick by writing 0x0201 then
    /// reading it in a tight loop until each bit goes low — the loop
    /// count tells them the position. We count reads since the last
    /// arm-write and trip each bit when the count passes a mouse-derived
    /// threshold. A read-count is used instead of elapsed time because
    /// the emulator runs much faster than 4.77 MHz, which would make a
    /// real-time threshold (e.g. 24–1124 µs) show the stick permanently
    /// pegged. Carrier Command uses this as its primary cursor input.
    pub joystick_read_count: u32,
    pub audio_device: Option<Box<dyn crate::audio::AudioOutput>>,
    pub speaker_on: bool,    // Is the speaker playing?
    pub pit_divisor: u16,    // Current Frequency Divisor
    pub pit_read_msb: bool,  // Channel 2 read LSB/MSB toggle
    pub pit_mode: u8,        // PIT Command Mode
    pub pit_write_msb: bool, // Toggle to handle 2-byte writes (LSB/MSB)
    /// Channel 0 count register as assembled from port 0x40 writes. A
    /// complete count is handed to `pit0`.
    pub pit0_divisor: u16,
    pub pit0_write_msb: bool,
    /// Toggle for alternating LSB/MSB when reading port 0x40 in 2-byte mode.
    pub pit0_read_msb: bool,
    /// Channel 0 access mode from the last control word: 1 = LSB only,
    /// 2 = MSB only, 3 = LSB then MSB.
    pub pit0_access: u8,
    /// Value latched into the read buffer by a `latch counter` command on
    /// port 0x43. When `pit0_latched_active` is true, reads of port 0x40
    /// return this value instead of the live count until both bytes are read.
    pub pit0_latched: u16,
    pub pit0_latched_active: bool,
    /// Channel 0 timing: when IRQ 0 fires.
    pub pit0: crate::timer::Pit0,
    /// Emulated time, advanced by the main loop.
    pub clock: crate::timer::Clock,
    /// The two 8259 interrupt controllers.
    pub pic: crate::pic::Pic,
    pub audio_phase: f32, // Track wave position to prevent clicking
    pub dta_segment: u16,
    pub dta_offset: u16,
    /// The log file `log_string` writes to, if the emulator opened one.
    pub log_file: Option<crate::log::LogFile>,

    // VGA State
    pub vga: crate::video::vga::VgaCard,
    /// The Super VGA side of the card: VESA modes and their memory.
    pub vbe: crate::video::vbe::Vbe,
    pub search_handles: std::collections::HashMap<u32, String>,

    // Mouse State (INT 33h)
    pub mouse: crate::mouse::MouseState,

    /// The FM synthesizer (OPL3 or OPL2) at 388h-38Bh and on the Sound
    /// Blaster's ports.
    pub opl: crate::opl::Opl,
    /// The Sound Blaster, if one is installed.
    pub sb: Option<crate::sb::SoundBlaster>,
    /// The two 8237 DMA controllers.
    pub dma: crate::dma::Dma,
    /// The MPU-401 MIDI interface at 330h.
    pub mpu: crate::mpu401::Mpu401,
    /// The Gravis Ultrasound, if one is installed.
    pub gus: Option<crate::gus::Gus>,
    /// The IRQ the Ultrasound's interrupt line holds up, if any.
    gus_line: Option<u8>,
    /// The drive with the built-in Ultrasound software, if any.
    ultrasnd_drive: Option<u8>,
    /// The CD drive playing audio tracks, for MSCDEX.
    pub cdaudio: crate::cdrom::audio::CdPlayer,
    /// The time disk access takes, and the drives' noises.
    pub disk_io: crate::diskio::DiskIo,
    pub disknoise: crate::disknoise::DiskNoise,
    /// The drives read or written since the front end last looked (bit n
    /// for drive n), for its activity lights.
    pub drives_active: u32,
    /// What MSCDEX keeps between calls.
    pub mscdex: crate::interrupts::mscdex::MscdexState,
    /// Mixed output (44.1 kHz stereo, interleaved) rendered up to
    /// `audio_frames` frames of emulated time, waiting for `pump_audio`.
    pub audio_out: VecDeque<i16>,
    audio_frames: u64,
    /// Resampling position and last frame of the Sound Blaster's output.
    sb_phase: f64,
    sb_frame: (i16, i16),
    /// Frames left of a BEL beep.
    pub beep_frames: u32,
    /// The host's volumes for each source (`[mixer]`) and the mute.
    pub mixer: crate::mixer::Mixer,
    /// Level-triggered interrupt request lines (bit n = IRQ n): the Sound
    /// Blaster holds its line until the driver acknowledges. Kept up to
    /// date by `update_irq_levels`, as the CPU tests it every instruction.
    irq_levels: u16,
    /// `interrupt_requested` as of the last change to the interrupt lines,
    /// the PICs or the mouse event handler, so the CPU tests one flag
    /// before each instruction. `refresh_irq` works it out again; it runs
    /// after everything that can change it while instructions execute
    /// (timer events, port I/O, interrupt delivery, emulator services) and
    /// at the start of each batch, after the front end's input.
    pub irq_ready: bool,
    /// Output level and underruns, for the debugger: the peak sample since
    /// it was last read, and how often the output device ran dry.
    pub audio_peak: u16,
    pub audio_underruns: u64,
    /// Writes logged so far to each port nothing emulates: a program
    /// polling for hardware that isn't there would flood the log.
    unhandled_writes: Vec<u8>,

    /// Generation counter for every block of RAM (`GEN_SHIFT`). Bumped on
    /// every write inside the Bus write helpers. The decoded-instruction
    /// cache stores the gen at decode time and invalidates a cached entry
    /// when the gen for its blocks has changed — this is how we stay correct in the
    /// face of self-modifying code (LZEXE, packers, etc.) without paying the
    /// cost of verifying cached bytes on every fetch.
    pub page_gen: Vec<u32>,

    /// Optional observer for every `log_string` line. Installed by the debug
    /// server so log output can be streamed to remote clients.
    pub log_hook: Option<Box<dyn FnMut(&str)>>,
    /// Optional observer for every block of mixed audio samples produced by
    /// `pump_audio` (44.1 kHz mono i16). Used by the debug audio stream.
    pub audio_hook: Option<Box<dyn FnMut(&[i16])>>,
}

use std::path::PathBuf;

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
            keyboard_buffer: VecDeque::new(),
            kbc: crate::kbc::Kbc::new(),
            a20_mask: !0x0010_0000,
            reset_requested: false,
            config_ui_requested: false,
            exit_requested: false,
            cmos: crate::cmos::Cmos::new(((ram_len >> 10) - 1024) as u32),
            post_code: 0,
            debug_console: Vec::new(),
            xms: crate::xms::Xms::new(),
            refresh_toggle: false,
            cursor_x: 0,
            cursor_y: 0,
            start_time: Instant::now(),
            joystick_read_count: 0,
            audio_device: None,
            speaker_on: false,
            pit_divisor: 0xFFFF,
            pit_mode: 0,
            pit_write_msb: false,
            pit_read_msb: false,
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
            search_handles: std::collections::HashMap::new(),
            mouse: crate::mouse::MouseState::new(),
            opl: crate::opl::Opl::new(true),
            sb: Some(crate::sb::SoundBlaster::new(crate::sb::SbConfig::default())),
            dma: crate::dma::Dma::new(),
            mpu: crate::mpu401::Mpu401::new(),
            gus: Some(crate::gus::Gus::new(crate::gus::GusConfig::default(), 0)),
            gus_line: None,
            ultrasnd_drive: None,
            cdaudio: crate::cdrom::audio::CdPlayer::new(),
            disk_io: crate::diskio::DiskIo::default(),
            disknoise: crate::disknoise::DiskNoise::new(),
            drives_active: 0,
            mscdex: Default::default(),
            audio_out: VecDeque::new(),
            audio_frames: 0,
            sb_phase: 0.0,
            sb_frame: (0, 0),
            beep_frames: 0,
            mixer: crate::mixer::Mixer::default(),
            irq_levels: 0,
            irq_ready: false,
            audio_peak: 0,
            audio_underruns: 0,
            unhandled_writes: vec![0; 0x10000],
            page_gen: vec![0; ram_len >> GEN_SHIFT],
            log_hook: None,
            audio_hook: None,
        };
        // BIOS Data Area (BDA) Initialization
        // 0x0449: Current Video Mode (03 = 80x25 Color)
        bus.write_8(0x0449, 0x03);
        // 0x044A: Number of Columns (80 = 0x50)
        bus.write_16(0x044A, 80);
        // 0x044C: Video Page Size (80x25 text: 4000 bytes, rounded to 4 KB),
        // 0x044E: the offset of the active page.
        bus.write_16(0x044C, 0x1000);
        bus.write_16(0x044E, 0);
        // 0x0460: Cursor Shape (Start Line 13, End Line 14 for VGA)
        bus.write_16(0x0460, 0x0D0E);
        // 0x0462: Active Page (0)
        bus.write_8(0x0462, 0);
        // 0x0463: CRT Controller Base Address (0x3D4 for Color)
        bus.write_16(0x0463, 0x03D4);

        // 0x0410: Equipment List. Bit 0 = Floppy (see `sync_drive_bda`);
        // the video adapter's bits come with it below.
        bus.write_16(0x0410, 0x0001);

        // 0x0484: Rows on Screen (minus 1). 24 = 25-row default.
        bus.write_8(0x0484, 24);
        // 0x0485: Character height in scan lines. 16 = VGA 8x16 default.
        bus.write_16(0x0485, 16);

        // The display adapter: its BIOS data and ROM.
        video::bios::install(&mut bus, video::adapter::VideoSetup::default());

        // Initialize SFT at F000:E000 (Address 0xFE000)
        // 00-02: Modes supported (All)
        bus.write_8(0xFE000, 0xFF);
        bus.write_8(0xFE001, 0xFF);
        bus.write_8(0xFE002, 0xFF);
        // 03-06: Reserved (0)
        // 07: Scanlines supported (All?) -> Let's say FF
        bus.write_8(0xFE007, 0xFF);
        // 0B: Total Char Blocks (8)
        bus.write_8(0xFE00B, 0x08);
        // 0C: Max Active Blocks (2)
        bus.write_8(0xFE00C, 0x02);
        // 0D: Misc Flags (0)
        // 10: Save Pointer Caps (0)


        // BIOS ROM code and the interrupt vector table.
        crate::bios::install(&mut bus);
        crate::mouse::install_callback_stub(&mut bus);

        // Build a baseline MCB chain — one large free block covering
        // conventional memory. load_shell / load_exe rebuild as needed, but we
        // still want mcb::alloc to work for tests and any early allocation.
        crate::mcb::init_empty(&mut bus);

        // The default Ultrasound's software, and the equipment word, hard
        // disk count and DPBs for the drives C:, X: and Z:.
        let _ = bus.mount_ultrasnd(crate::gus::GusConfig::default().drive);

        bus
    }

    /// Mount a host directory as a DOS drive and refresh the BIOS view of
    /// the drive set. See `DiskController::mount`.
    pub fn mount_drive(
        &mut self,
        drive: u8,
        path: &std::path::Path,
        opts: MountOptions,
        replace: bool,
    ) -> Result<std::path::PathBuf, String> {
        self.mount_with(drive, |disk| disk.mount(drive, path, opts, replace))
    }

    /// Mount a disk or CD image held in memory as a DOS drive, replacing
    /// what is there. See `DiskController::mount_memory_image`.
    pub fn mount_memory_image(
        &mut self,
        drive: u8,
        name: &str,
        data: crate::diskimage::MemoryImage,
        opts: MountOptions,
    ) -> Result<(), String> {
        self.mount_with(drive, |disk| disk.mount_memory_image(drive, name, data, opts))
    }

    /// Mount a disk image made in memory as a DOS drive, replacing what is
    /// there. See `DiskController::mount_disk_image`.
    pub fn mount_disk_image(
        &mut self,
        drive: u8,
        image: crate::diskimage::DiskImage,
        opts: MountOptions,
    ) -> Result<(), String> {
        self.mount_with(drive, |disk| disk.mount_disk_image(drive, image, opts))
    }

    /// Mount `drive` with `mount` and refresh the BIOS view of the drive set.
    fn mount_with<T>(
        &mut self,
        drive: u8,
        mount: impl FnOnce(&mut DiskController) -> Result<T, String>,
    ) -> Result<T, String> {
        let result = mount(&mut self.disk);
        if result.is_ok() {
            // Another disc: whatever played stops, and MSCDEX says so.
            self.cdaudio.stop_drive(drive);
            self.mscdex.disc_changed(drive);
        }
        self.sync_drive_bda();
        result
    }

    /// Put the Ultrasound software built into rust-dos (`gus::builtin`) on
    /// `drive`, or with None nowhere, in place of where it was.
    pub fn mount_ultrasnd(&mut self, drive: Option<u8>) -> Result<(), String> {
        // Unless a MOUNT has taken its place.
        if let Some(old) = self.ultrasnd_drive.take()
            && self.disk.drive_kind(old) == Some(DriveKind::Virtual)
        {
            let _ = self.disk.unmount(old);
        }
        let result = match drive {
            Some(drive) => {
                let files = crate::gus::builtin::drive(crate::disk::drive_letter(drive));
                self.disk.mount_memory(drive, files, crate::gus::builtin::LABEL).map(|()| {
                    self.ultrasnd_drive = Some(drive);
                })
            }
            None => Ok(()),
        };
        self.sync_drive_bda();
        result
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

    /// A drive of `class` moved `bytes`: charge the time that takes at the
    /// drive's speed, and make its noise.
    pub fn disk_activity(&mut self, class: crate::diskio::DiskClass, bytes: u32, access: crate::disknoise::Access) {
        let pending = self.disk_io.charge(class, bytes);
        if self.disknoise.enabled(class) {
            // The noise starts now and goes on while the access does.
            self.audio_catch_up();
            let frames = (pending as u128 * crate::opl::RATE as u128 / 1_000_000_000) as u64;
            self.disknoise.io(class, access, frames);
        }
    }

    /// `bytes` were moved on `drive`, if it is a floppy drive or a hard
    /// disk.
    pub fn drive_activity(&mut self, drive: u8, bytes: u32, access: crate::disknoise::Access) {
        self.drives_active |= 1 << drive;
        if let Some(class) = self.disk.drive_kind(drive).and_then(crate::diskio::DiskClass::of) {
            self.disk_activity(class, bytes, access);
        }
    }

    /// Sectors of a drive's disk image were read or written through the
    /// BIOS or INT 25h/26h, from sector `lba` of the disk on.
    pub fn sector_activity(&mut self, drive: u8, lba: u64, sectors: u32, _write: bool) {
        let per_track = self.disk.bios_image(drive).map_or(1, |disk| disk.geometry().sectors.max(1) as u64);
        let bytes = sectors.saturating_mul(crate::diskimage::SECTOR_SIZE as u32);
        self.drive_activity(drive, bytes, crate::disknoise::Access::Track(lba / per_track));
    }

    /// Put the next image in every drive mounted from a list of images, as
    /// Ctrl+F4 does. Returns what changed, and what went wrong.
    pub fn swap_images(&mut self) -> Vec<String> {
        let mut messages = Vec::new();
        for drive in 0..LASTDRIVE {
            match self.disk.swap_image(drive) {
                Ok(Some(message)) => {
                    self.cdaudio.stop_drive(drive);
                    self.mscdex.disc_changed(drive);
                    messages.push(message);
                }
                Ok(None) => {}
                Err(e) => messages.push(e),
            }
        }
        if !messages.is_empty() {
            self.sync_drive_bda();
        }
        messages
    }

    /// Unmount a DOS drive and refresh the BIOS view of the drive set.
    pub fn unmount_drive(&mut self, drive: u8) -> Result<(), String> {
        let result = self.disk.unmount(drive);
        self.cdaudio.stop_drive(drive);
        self.sync_drive_bda();
        result
    }

    /// Mirror the mounted drives into the BIOS data area and the ROM tables
    /// DOS hands out pointers to. Must run whenever the drive set changes.
    pub fn sync_drive_bda(&mut self) {
        // Equipment word: bit 0 = floppy present, bits 6-7 = floppy count - 1.
        // Only A: and B: are BIOS floppy units. Other bits are left alone.
        let floppies = self.disk.floppy_units();
        let mut equipment = self.read_16(0x0410) & !0x00C1;
        if floppies > 0 {
            equipment |= 0x0001 | ((floppies as u16 - 1) << 6);
        }
        self.write_16(0x0410, equipment);
        self.cmos.set_floppies(floppies);

        // 0x0475: number of fixed disks (INT 13h units 80h+).
        let hard_disks = self.disk.drives_of_kind(DriveKind::HardDisk).len();
        self.write_8(0x0475, hard_disks.min(0xFF) as u8);

        // CD-ROMs are redirector drives and have no DPB.
        let with_dpb: Vec<(u8, DriveKind)> = (0..LASTDRIVE)
            .filter_map(|d| self.disk.drive_kind(d).map(|k| (d, k)))
            .filter(|&(_, k)| k != DriveKind::CdRom)
            .collect();
        for drive in 0..LASTDRIVE {
            let media = self.disk.media_descriptor(drive);
            self.write_8(MEDIA_ID_TABLE + drive as usize, media);
            let base = DPB_TABLE + drive as usize * DPB_SIZE;
            for i in 0..DPB_SIZE {
                self.write_8(base + i, 0);
            }
        }
        for (i, &(drive, _)) in with_dpb.iter().enumerate() {
            let next = with_dpb.get(i + 1).map(|&(d, _)| d);
            if let Some(layout) = self.disk.layout(drive) {
                self.write_dpb(drive, layout, next);
            }
        }
        self.write_list_of_lists(&with_dpb);
    }

    /// Fill in the DOS 5 List of Lists for INT 21h AH=52h. Programs mostly
    /// read the first MCB segment at offset -2 to walk the memory chain.
    /// Structures the emulator doesn't keep in DOS memory (SFTs, CDS, disk
    /// buffers, CLOCK$/CON drivers) are left as null pointers.
    fn write_list_of_lists(&mut self, with_dpb: &[(u8, DriveKind)]) {
        let base = DOS_LIST_OF_LISTS;
        for i in 0..0x50 {
            self.write_8(base + i, 0);
        }
        self.write_16(base - 2, crate::mcb::FIRST_MCB_SEG); // -2: first MCB
        // 00: far pointer to the first DPB
        match with_dpb.first() {
            Some(&(d, _)) => {
                self.write_16(base, (DPB_TABLE + d as usize * DPB_SIZE - 0xF0000) as u16);
                self.write_16(base + 0x02, 0xF000);
            }
            None => self.write_32(base, 0xFFFF_FFFF),
        }
        let max_sector = with_dpb.iter().filter_map(|&(d, _)| self.disk.layout(d)).map(|l| l.bytes_per_sector).max();
        self.write_16(base + 0x10, max_sector.unwrap_or(512)); // 10: max bytes per sector
        self.write_8(base + 0x20, with_dpb.len() as u8); // 20: block devices
        self.write_8(base + 0x21, LASTDRIVE); // 21: LASTDRIVE
        // 22: NUL device header, the last driver in the chain
        let nul = base + 0x22;
        self.write_32(nul, 0xFFFF_FFFF); // next driver
        self.write_16(nul + 0x04, 0x8004); // character device, NUL
        let retf = (base + 0x50 - 0xF0000) as u16;
        self.write_16(nul + 0x06, retf); // strategy entry
        self.write_16(nul + 0x08, retf); // interrupt entry
        // The CD-ROM driver follows NUL when there are CD drives.
        crate::interrupts::mscdex::install_device(self, nul);
        for (i, &b) in b"NUL     ".iter().enumerate() {
            self.write_8(nul + 0x0A + i, b);
        }
        self.write_8(base + 0x43, 3); // 43: boot drive C:
        self.write_8(base + 0x50, 0xCB); // RETF for the NUL driver entries, past the table
    }

    /// Fill in a DOS 4+ style Drive Parameter Block with the drive's FAT
    /// layout: a disk image's own, or a plausible one for its geometry.
    fn write_dpb(&mut self, drive: u8, layout: crate::disk::FatLayout, next: Option<u8>) {
        let spc = layout.sectors_per_cluster;
        let base = DPB_TABLE + drive as usize * DPB_SIZE;

        self.write_8(base, drive); // 00: drive number (0=A)
        self.write_8(base + 0x01, drive); // 01: unit within driver
        self.write_16(base + 0x02, layout.bytes_per_sector); // 02: bytes per sector
        self.write_8(base + 0x04, (spc - 1) as u8); // 04: sectors per cluster - 1
        self.write_8(base + 0x05, spc.trailing_zeros() as u8); // 05: cluster shift
        self.write_16(base + 0x06, layout.reserved_sectors); // 06: reserved sectors
        self.write_8(base + 0x08, layout.fats as u8); // 08: number of FATs
        self.write_16(base + 0x09, layout.root_entries); // 09: root directory entries
        self.write_16(base + 0x0B, layout.first_data_sector()); // 0B: first data sector
        self.write_16(base + 0x0D, layout.clusters.saturating_add(1)); // 0D: highest cluster
        self.write_16(base + 0x0F, layout.sectors_per_fat); // 0F: sectors per FAT
        self.write_16(base + 0x11, layout.first_dir_sector()); // 11: first directory sector
        self.write_8(base + 0x17, layout.media); // 17: media ID
        self.write_8(base + 0x18, 0x00); // 18: disk accessed
        // 19: far pointer to the next DPB, FFFF:FFFF ends the chain
        match next {
            Some(n) => {
                self.write_16(base + 0x19, (DPB_TABLE + n as usize * DPB_SIZE - 0xF0000) as u16);
                self.write_16(base + 0x1B, 0xF000);
            }
            None => {
                self.write_16(base + 0x19, 0xFFFF);
                self.write_16(base + 0x1B, 0xFFFF);
            }
        }
        self.write_16(base + 0x1D, 2); // 1D: cluster to start free search
        self.write_16(base + 0x1F, 0xFFFF); // 1F: free clusters unknown
    }

    /// Installs a Magic Trap (FE 38 <Vector> CF) at the given Physical Address
    /// and updates the IVT to point to it.
    /// Mark the text VRAM byte range `[start, end)` for re-rendering. Code
    /// that writes `vga.vram_text` directly instead of through `write_8`
    /// must call this (or `vga.mark_dirty_full`), or the dirty-rect renderer
    /// never repaints the change. Same row math as the `write_8` text path.
    pub fn mark_text_dirty(&mut self, start: usize, end: usize) {
        match video::text::geometry(self) {
            Some(geometry) => {
                if let Some((y0, y1)) = geometry.screen_rows(start, end) {
                    self.vga.mark_dirty_rows(y0, y1);
                }
            }
            None => self.vga.mark_dirty_full(),
        }
    }

    /// The rows of the text screen: BDA 0484h holds them less one, and is 0
    /// where a BIOS doesn't keep it (the CGA's), which means 25.
    pub fn text_rows(&self) -> usize {
        match self.read_8(0x0484) {
            0 => 25,
            rows => rows as usize + 1,
        }
    }

    // Helper: Scroll the text screen up by 1 line
    pub fn scroll_up(&mut self) {
        // Read the current row count from BDA so 80x43 / 80x50 modes scroll
        // their whole visible area, not just the first 25 rows.
        let rows = self.text_rows();
        let row_size = 160; // 80 chars * 2 bytes
        let screen_size = rows * row_size;
        if screen_size > self.vga.vram_text.len() {
            return;
        }

        // Move memory back
        for i in 0..(screen_size - row_size) {
            self.vga.vram_text[i] = self.vga.vram_text[i + row_size];
        }

        // Clear bottom row with space + light-gray attribute pairs.
        for i in (screen_size - row_size)..screen_size {
            self.vga.vram_text[i] = if i % 2 == 0 { 0x20 } else { 0x07 };
        }
        // Scroll moves every visible row, so widen to the full screen.
        self.vga.mark_dirty_full();
    }

    /// Read-only view of system RAM for the instruction decoder, DMA and
    /// debug dumps. It bypasses the VGA memory mapping. Writes go through
    /// `write_8`, `load_bytes` or `fill_ram`, which let the decoded-
    /// instruction cache see them.
    #[inline(always)]
    pub fn ram(&self) -> &[u8] {
        &self.ram
    }

    /// Copy `data` into RAM at `addr`, bypassing the VGA mapping, as program
    /// loaders do. Bytes past the end of RAM are dropped.
    pub fn load_bytes(&mut self, addr: usize, data: &[u8]) {
        let end = addr.saturating_add(data.len()).min(self.ram.len());
        if addr >= end {
            return;
        }
        self.ram[addr..end].copy_from_slice(&data[..end - addr]);
        self.bump_page_gens(addr, end);
    }

    /// Set every RAM byte in `range` to `value`, bypassing the VGA mapping.
    pub fn fill_ram(&mut self, range: std::ops::Range<usize>, value: u8) {
        let end = range.end.min(self.ram.len());
        if range.start >= end {
            return;
        }
        self.ram[range.start..end].fill(value);
        self.bump_page_gens(range.start, end);
    }

    /// Invalidate cached decodes of the pages covering `start..end`.
    fn bump_page_gens(&mut self, start: usize, end: usize) {
        for page in (start >> GEN_SHIFT)..=((end - 1) >> GEN_SHIFT) {
            let g = &mut self.page_gen[page];
            *g = g.wrapping_add(1);
        }
    }

    /// Whether the A20 gate is open.
    #[inline(always)]
    pub fn a20(&self) -> bool {
        self.a20_mask & 0x0010_0000 != 0
    }

    /// Open or close the A20 gate.
    pub fn set_a20(&mut self, open: bool) {
        self.a20_mask = if open { !0 } else { !0x0010_0000 };
    }

    /// Apply the A20 gate to a physical address.
    #[inline(always)]
    pub fn a20_mask(&self) -> u32 {
        self.a20_mask
    }

    /// True for `len` bytes at `addr` that are plain RAM: conventional
    /// memory below the video window, or extended memory above 1 MB.
    #[inline(always)]
    fn is_plain_ram(&self, addr: usize, len: usize) -> bool {
        addr + len <= ADDR_VGA_GRAPHICS || (addr >= 0x10_0000 && addr + len <= self.ram.len())
    }

    #[inline(always)]
    pub fn read_8(&self, addr: usize) -> u8 {
        // Fast path — the vast majority of memory accesses (code fetch,
        // stack, program data) are plain RAM and don't need the VGA range
        // checks.
        if self.is_plain_ram(addr, 1) {
            // SAFETY: is_plain_ram checked that addr is within ram.
            return unsafe { *self.ram.get_unchecked(addr) };
        }
        self.read_8_mapped(addr)
    }

    /// Reads of the video memory, the ROM area and past the end of RAM.
    fn read_8_mapped(&self, addr: usize) -> u8 {
        if addr < ADDR_VGA_GRAPHICS + SIZE_GRAPHICS && addr >= ADDR_VGA_GRAPHICS {
            // VESA modes: the window onto the bank of video memory.
            if self.video_mode == VideoMode::Vesa {
                return self.vbe.vram[self.vbe.window_offset(addr - ADDR_VGA_GRAPHICS)];
            }
            // Route through VGA so chain-4, odd/even, and Read Map Select
            // work correctly. read_graphics also latches planes, needed
            // for planar read-modify-write sequences.
            return self.vga.read_graphics(addr - ADDR_VGA_GRAPHICS);
        }
        let (text, size, wrap) = self.vga.text_window();
        if (text..text + size).contains(&addr) {
            return self.vga.vram_text[(addr - text) & wrap];
        }
        if addr < self.ram.len() {
            return self.ram[addr];
        }
        if let Some(offset) = Vbe::lfb_offset(addr, 1) {
            return self.vbe.vram[offset];
        }
        if addr >= 0xFFFE_0000 {
            // The top 128 KB of the address space mirror the BIOS ROM area
            // (E0000h-FFFFFh), where a 386 fetches its first instruction
            // after reset.
            return self.ram[0xE0000 + (addr & 0x1FFFF)];
        }
        0xFF // nothing there: open bus
    }

    /// Side-effect-free byte read for debuggers. Same mapping as `read_8`,
    /// but VGA plane latches are restored afterwards so inspecting video
    /// memory can't disturb a program's read-modify-write sequences.
    pub fn peek_8(&self, addr: usize) -> u8 {
        if addr >= self.ram.len() {
            return self.read_8_mapped(addr);
        }
        if (ADDR_VGA_GRAPHICS..ADDR_VGA_GRAPHICS + SIZE_GRAPHICS).contains(&addr) && self.video_mode != VideoMode::Vesa {
            let saved = self.vga.latches.get();
            let v = self.vga.read_graphics(addr - ADDR_VGA_GRAPHICS);
            self.vga.latches.set(saved);
            return v;
        }
        self.read_8(addr)
    }

    // Returns true if a write occurred to the *active* video memory
    #[inline(always)]
    pub fn write_8(&mut self, addr: usize, value: u8) -> bool {
        // Fast path — plain RAM writes are the overwhelming majority.
        if self.is_plain_ram(addr, 1) {
            // SAFETY: is_plain_ram checked that addr is within ram, and
            // page_gen has an entry for every page of ram.
            unsafe {
                *self.ram.get_unchecked_mut(addr) = value;
                // Bump generation for this page so the decoded-instruction
                // cache invalidates any cached decodes that fell in it.
                let g = self.page_gen.get_unchecked_mut(addr >> GEN_SHIFT);
                *g = g.wrapping_add(1);
            }
            return false;
        }
        if addr >= ADDR_VGA_GRAPHICS && addr < ADDR_VGA_GRAPHICS + SIZE_GRAPHICS {
            if self.video_mode == VideoMode::Vesa {
                let offset = self.vbe.window_offset(addr - ADDR_VGA_GRAPHICS);
                self.write_vram(offset, &[value]);
                return true;
            }
            // write_graphics already sets vga.dirty unconditionally. The
            // Return value only matters to callers that care whether the
            // write hit the active display plane, but rendering is gated
            // off vga.dirty directly, so we simplify here.
            self.vga.write_graphics(addr - ADDR_VGA_GRAPHICS, value);
            return matches!(
                self.video_mode,
                VideoMode::Graphics320x200
                    | VideoMode::Ega320x200
                    | VideoMode::Ega640x200
                    | VideoMode::Ega640x350
                    | VideoMode::Vga640x480
            );
        }
        let (text, size, wrap) = self.vga.text_window();
        if (text..text + size).contains(&addr) {
            let text_off = (addr - text) & wrap;
            self.vga.vram_text[text_off] = value;

            // Narrow the dirty range to just the affected character row when
            // we're in a text mode (see `text::geometry`). CGA graphics modes
            // (4/5/6) also live in this VRAM but their byte-to-scanline
            // mapping is interleaved, so we conservatively repaint everything
            // for those.
            self.mark_text_dirty(text_off, text_off + 1);

            // Check if current mode uses this memory
            return matches!(
                self.video_mode,
                VideoMode::Text80x25
                    | VideoMode::Text80x25Color
                    | VideoMode::Text40x25
                    | VideoMode::Text40x25Color
                    | VideoMode::Mono80x25
                    | VideoMode::HercGraphics
                    | VideoMode::Cga320x200
                    | VideoMode::Cga320x200Color
                    | VideoMode::Cga640x200
            );
        }

        // ROM / reserved area (0xC0000..0x100000 on a real PC). Still backed
        // by our Vec<u8> so BIOS-ROM writes from initialization work.
        // Writes past the end of RAM go nowhere.
        if addr < self.ram.len() {
            self.ram[addr] = value;
            let page = addr >> GEN_SHIFT;
            self.page_gen[page] = self.page_gen[page].wrapping_add(1);
            return false;
        }
        if let Some(offset) = Vbe::lfb_offset(addr, 1) {
            self.write_vram(offset, &[value]);
            return true;
        }
        false
    }

    /// Write VESA video memory, marking the rows of the picture it shows
    /// in for repainting.
    #[inline]
    fn write_vram(&mut self, offset: usize, bytes: &[u8]) {
        self.vbe.vram[offset..offset + bytes.len()].copy_from_slice(bytes);
        if self.video_mode == VideoMode::Vesa {
            if let Some((first, last)) = self.vbe.frame_rows(offset, bytes.len()) {
                self.vga.mark_dirty_rows(first, last);
            }
        }
    }

    // Write a 16-bit value to memory (Little Endian)
    #[inline(always)]
    pub fn write_16(&mut self, addr: usize, value: u16) -> bool {
        // Fast path: both bytes in plain RAM, as in write_8.
        if self.is_plain_ram(addr, 2) {
            // SAFETY: is_plain_ram checked both bytes are within ram.
            unsafe {
                *self.ram.get_unchecked_mut(addr) = value as u8;
                *self.ram.get_unchecked_mut(addr + 1) = (value >> 8) as u8;
                // Both bytes' pages: the word may straddle a page boundary.
                let g = self.page_gen.get_unchecked_mut(addr >> GEN_SHIFT);
                *g = g.wrapping_add(1);
                let g = self.page_gen.get_unchecked_mut((addr + 1) >> GEN_SHIFT);
                *g = g.wrapping_add(1);
            }
            return false;
        }
        if let Some(offset) = Vbe::lfb_offset(addr, 2) {
            self.write_vram(offset, &value.to_le_bytes());
            return true;
        }
        // Low byte
        let d1 = self.write_8(addr, (value & 0xFF) as u8);
        // High byte
        let d2 = self.write_8(addr + 1, (value >> 8) as u8);
        d1 || d2
    }

    // read_16 helper
    #[inline(always)]
    pub fn read_16(&self, addr: usize) -> u16 {
        if self.is_plain_ram(addr, 2) {
            // SAFETY: is_plain_ram checked both bytes are within ram.
            return unsafe {
                u16::from_le_bytes([
                    *self.ram.get_unchecked(addr),
                    *self.ram.get_unchecked(addr + 1),
                ])
            };
        }
        if let Some(offset) = Vbe::lfb_offset(addr, 2) {
            return u16::from_le_bytes([self.vbe.vram[offset], self.vbe.vram[offset + 1]]);
        }
        let low = self.read_8(addr) as u16;
        let high = self.read_8(addr + 1) as u16;
        (high << 8) | low
    }

    #[inline(always)]
    pub fn read_32(&self, addr: usize) -> u32 {
        if self.is_plain_ram(addr, 4) {
            // SAFETY: is_plain_ram checked all four bytes are within ram.
            return unsafe {
                u32::from_le_bytes([
                    *self.ram.get_unchecked(addr),
                    *self.ram.get_unchecked(addr + 1),
                    *self.ram.get_unchecked(addr + 2),
                    *self.ram.get_unchecked(addr + 3),
                ])
            };
        }
        if let Some(offset) = Vbe::lfb_offset(addr, 4) {
            let v = &self.vbe.vram[offset..offset + 4];
            return u32::from_le_bytes([v[0], v[1], v[2], v[3]]);
        }
        let low = self.read_16(addr) as u32;
        let high = self.read_16(addr + 2) as u32;
        (high << 16) | low
    }

    #[inline(always)]
    pub fn write_32(&mut self, addr: usize, value: u32) {
        if self.is_plain_ram(addr, 4) {
            self.ram[addr..addr + 4].copy_from_slice(&value.to_le_bytes());
            self.page_gen[addr >> GEN_SHIFT] = self.page_gen[addr >> GEN_SHIFT].wrapping_add(1);
            let last = (addr + 3) >> GEN_SHIFT;
            self.page_gen[last] = self.page_gen[last].wrapping_add(1);
            return;
        }
        if let Some(offset) = Vbe::lfb_offset(addr, 4) {
            self.write_vram(offset, &value.to_le_bytes());
            return;
        }
        self.write_16(addr, (value & 0xFFFF) as u16);
        self.write_16(addr + 2, (value >> 16) as u16);
    }

    pub fn read_64(&self, addr: usize) -> u64 {
        let low = self.read_32(addr) as u64;
        let high = self.read_32(addr + 4) as u64;
        (high << 32) | low
    }

    pub fn write_64(&mut self, addr: usize, value: u64) {
        self.write_32(addr, (value & 0xFFFFFFFF) as u32);
        self.write_32(addr + 4, (value >> 32) as u32);
    }

    /// PIT channel 2's output: a square wave at the programmed frequency.
    fn pit2_output(&self) -> bool {
        let divisor = if self.pit_divisor == 0 { 0x10000u64 } else { self.pit_divisor as u64 };
        self.clock.now_ticks() % divisor < divisor / 2
    }

    /// Start an execution batch that runs until instruction `end`.
    pub fn start_batch(&mut self, end: u64) {
        self.clock.set_batch_end(end);
        self.clock.schedule(self.next_event());
        self.refresh_irq();
    }

    /// Change the emulated CPU speed (instructions per emulated ms).
    pub fn set_cycles_per_ms(&mut self, cycles_per_ms: u32) {
        self.clock.set_cycles_per_ms(cycles_per_ms);
        self.clock.schedule(self.next_event());
    }

    /// The next time (PIT ticks) a device needs attention: the timer's
    /// next IRQ 0, the end of the Sound Blaster's current block, or the
    /// Ultrasound's next timer, DMA or voice interrupt.
    fn next_event(&self) -> Option<u64> {
        let sb = self.sb.as_ref().and_then(|sb| sb.next_event());
        let gus = self.gus_next_event();
        [self.pit0.next_event(), sb, gus].into_iter().flatten().min()
    }

    fn gus_next_event(&self) -> Option<u64> {
        self.gus.as_ref().and_then(|gus| gus.next_event(&self.dma))
    }

    /// Bring the PIT and the Sound Blaster up to the current instruction
    /// and request the interrupts that came due. The main loop calls this
    /// when `clock.icount` reaches `clock.deadline`.
    pub fn service_timers(&mut self) {
        let now = self.clock.now_ticks();
        if self.pit0.advance(now) {
            self.pic.raise(0);
        }
        if self.sb.as_ref().and_then(|sb| sb.next_event()).is_some_and(|t| t <= now) {
            self.sb_advance();
        }
        if self.gus_next_event().is_some_and(|t| t <= now) {
            self.gus_advance();
        }
        self.clock.schedule(self.next_event());
        self.refresh_irq();
    }

    /// Run the Sound Blaster's DSP up to the present.
    fn sb_advance(&mut self) {
        let now = self.clock.now_ticks();
        if let Some(sb) = &mut self.sb {
            sb.advance(now, &mut self.dma, &self.ram);
            self.update_irq_levels();
        }
    }

    /// Run the Ultrasound up to the present and update its interrupt line.
    fn gus_advance(&mut self) {
        let now = self.clock.now_ticks();
        if let Some(gus) = &mut self.gus {
            if let Some(written) = gus.advance(now, &mut self.dma, &mut self.ram) {
                self.bump_page_gens(written.start, written.end);
            }
            self.sync_gus_irq();
        }
    }

    /// Drive the Ultrasound's interrupt request: raise it when the line
    /// goes up and whenever the card has a new reason to interrupt while
    /// it is up, withdraw it when the line drops.
    fn sync_gus_irq(&mut self) {
        let Some(gus) = &mut self.gus else {
            if let Some(irq) = self.gus_line.take() {
                self.pic.lower(irq);
                self.refresh_irq();
            }
            return;
        };
        let fresh = gus.take_fresh();
        let line = gus.irq_line().then(|| gus.irq()).flatten();
        if let Some(old) = self.gus_line
            && line != Some(old)
        {
            self.pic.lower(old);
        }
        if let Some(irq) = line
            && (fresh || self.gus_line != line)
        {
            self.pic.raise(irq);
        }
        self.gus_line = line;
        self.refresh_irq();
    }

    /// Whether `port` belongs to the Ultrasound.
    #[inline]
    fn gus_claims(&self, port: u16) -> bool {
        self.gus.as_ref().is_some_and(|gus| gus.claims(port))
    }

    fn gus_write(&mut self, port: u16, value: u8) {
        let now = self.clock.now_ticks();
        let Some(gus) = &mut self.gus else { return };
        if gus.latch_only(port, true) {
            // Drivers write the selects constantly.
            gus.write(port, value, now);
            return;
        }
        self.gus_advance();
        if let Some(gus) = &mut self.gus {
            gus.write(port, value, now);
        }
        self.sync_gus_irq();
        self.clock.schedule(self.next_event());
    }

    fn gus_read(&mut self, port: u16) -> u8 {
        if let Some(gus) = &mut self.gus
            && gus.latch_only(port, false)
        {
            return gus.read(port);
        }
        self.gus_advance();
        let value = self.gus.as_mut().map_or(0xFF, |gus| gus.read(port));
        self.sync_gus_irq();
        self.clock.schedule(self.next_event());
        value
    }

    /// Install (or with None remove) the Gravis Ultrasound.
    pub fn configure_gus(&mut self, config: Option<crate::gus::GusConfig>) {
        let now = self.clock.now_ticks();
        self.gus = config.filter(|c| c.enabled).map(|c| crate::gus::Gus::new(c, now));
        self.sync_gus_irq();
        self.clock.schedule(self.next_event());
    }

    /// Render the mixed audio of every device up to the present, so a
    /// change a program makes now (an FM register, a DAC sample, the
    /// speaker gate) is heard from now on.
    pub fn audio_catch_up(&mut self) {
        self.sb_advance();
        self.gus_advance();
        let rate = crate::opl::RATE as u64;
        let target = (self.clock.now_ticks() as u128 * rate as u128 / crate::timer::PIT_HZ as u128) as u64;
        // After a long pause (a debugger stop, a slow host) start afresh
        // rather than render seconds of catch-up.
        if target.saturating_sub(self.audio_frames) > rate / 2 {
            // A CD plays on meanwhile, and the disks spin.
            self.cdaudio.skip(target - rate / 2 - self.audio_frames);
            self.disknoise.skip(target - rate / 2 - self.audio_frames);
            self.audio_frames = target - rate / 2;
        }
        let frames = target.saturating_sub(self.audio_frames) as usize;
        if frames == 0 {
            return;
        }
        self.audio_frames = target;

        let divisor = if self.pit_divisor == 0 { 65536.0 } else { self.pit_divisor as f32 };
        let speaker_step = crate::timer::PIT_HZ as f32 / divisor / rate as f32;
        let speaker = self.speaker_on && speaker_step * rate as f32 > 20.0;
        let ((vl, vr), (fl, fr)) = self.sb.as_ref().map_or(((1.0, 1.0), (1.0, 1.0)), |sb| sb.volumes());
        let (cl, cr) = self.sb.as_ref().map_or((1.0, 1.0), |sb| sb.cd_volume());
        let (sb_on, sb_step, dac) = match &self.sb {
            Some(sb) => (
                sb.speaker_on || sb.config.model == crate::sb::SbModel::Sb16,
                sb.out_rate as f64 / rate as f64,
                sb.dac,
            ),
            None => (false, 0.0, 0),
        };
        const SPEAKER: f32 = 3000.0;
        const GUS_GAIN: f32 = 1.0;
        // Each source at its volume in the host's mixer, then all of them
        // at the master volume.
        use crate::mixer::{Channel, add};
        let gains = self.mixer.gains();
        let mut peaks = [0.0f32; crate::mixer::CHANNELS];
        for _ in 0..frames {
            let mut mix = (0.0f32, 0.0f32);
            if speaker {
                self.audio_phase += speaker_step;
                if self.audio_phase >= 1.0 {
                    self.audio_phase -= 1.0;
                }
                let s = if self.audio_phase < 0.5 { SPEAKER } else { -SPEAKER };
                add(&mut mix, &mut peaks, &gains, Channel::Speaker, (s, s));
            }
            let (ol, or) = self.opl.render();
            add(&mut mix, &mut peaks, &gains, Channel::Fm, (ol as f32 * fl, or as f32 * fr));
            if let Some(sb) = &mut self.sb {
                if sb.out.is_empty() && self.sb_phase < 1.0 {
                    self.sb_frame = (dac, dac);
                }
                self.sb_phase += sb_step;
                while self.sb_phase >= 1.0 {
                    self.sb_phase -= 1.0;
                    match sb.out.pop_front() {
                        Some(f) => self.sb_frame = f,
                        None => {
                            self.sb_phase = 0.0;
                            break;
                        }
                    }
                }
                if sb_on {
                    let frame = (self.sb_frame.0 as f32 * vl, self.sb_frame.1 as f32 * vr);
                    add(&mut mix, &mut peaks, &gains, Channel::Sb, frame);
                }
            }
            add(&mut mix, &mut peaks, &gains, Channel::Midi, self.mpu.render());
            let (cdl, cdr) = self.cdaudio.render();
            add(&mut mix, &mut peaks, &gains, Channel::CdAudio, (cdl * cl, cdr * cr));
            let noise = self.disknoise.render();
            add(&mut mix, &mut peaks, &gains, Channel::DiskNoise, (noise, noise));
            if let Some(gus) = &mut self.gus {
                let (gl, gr) = gus.pop_frame(crate::opl::RATE);
                add(&mut mix, &mut peaks, &gains, Channel::Gus, (gl * GUS_GAIN, gr * GUS_GAIN));
            }
            if self.beep_frames > 0 {
                self.beep_frames -= 1;
                let s = if self.beep_frames % 50 < 25 { SPEAKER } else { -SPEAKER };
                add(&mut mix, &mut peaks, &gains, Channel::Speaker, (s, s));
            }
            let (l, r) = crate::mixer::master(mix, &mut peaks, &gains);
            self.audio_out.push_back(l.clamp(-32768.0, 32767.0) as i16);
            self.audio_out.push_back(r.clamp(-32768.0, 32767.0) as i16);
        }
        self.mixer.add_peaks(peaks);
        // Nobody drains it without an audio device: keep a second.
        let max = 2 * rate as usize;
        if self.audio_out.len() > max {
            let extra = self.audio_out.len() - max;
            self.audio_out.drain(..extra);
        }
        // The DSP's output gets ahead when its rate and ours round
        // differently; keep at most a tenth of a second queued.
        if let Some(sb) = &mut self.sb {
            let keep = (sb.out_rate / 10).max(64) as usize;
            if sb.out.len() > keep {
                let extra = sb.out.len() - keep;
                sb.out.drain(..extra);
            }
        }
        if let Some(gus) = &mut self.gus {
            gus.trim_output();
        }
    }

    /// Frames of audio rendered since start.
    pub fn audio_frames(&self) -> u64 {
        self.audio_frames
    }

    /// Replace the sound hardware: the Sound Blaster (None removes it) and
    /// whether the FM chip is an OPL3.
    pub fn configure_sound(&mut self, sb: Option<crate::sb::SbConfig>, opl3: bool) {
        self.sb = sb.map(crate::sb::SoundBlaster::new);
        self.opl = crate::opl::Opl::new(opl3);
        self.update_irq_levels();
        self.clock.schedule(self.next_event());
    }

    /// Power-on state of the sound hardware, when a program ends. The
    /// Ultrasound keeps its DRAM, and when a resident program has hooked
    /// its interrupt (a MIDI driver like ULTRAMID), its settings too: only
    /// its voices stop.
    pub fn reset_sound(&mut self) {
        let config = self.sb.as_ref().map(|sb| sb.config);
        let opl3 = self.opl.is_opl3();
        self.configure_sound(config, opl3);
        self.dma = crate::dma::Dma::new();
        self.mpu.reset();
        self.cdaudio.reset();
        self.sb_frame = (0, 0);
        let hooked = self.gus.as_ref().and_then(|gus| gus.irq()).is_some_and(|irq| {
            let vector = if irq < 8 { 0x08 + irq as usize } else { 0x70 + irq as usize - 8 };
            let entry = (self.read_16(vector * 4 + 2) as u32) << 16 | self.read_16(vector * 4) as u32;
            entry != crate::bios::default_ivt()[vector]
        });
        if let Some(gus) = &mut self.gus {
            if hooked { gus.silence() } else { gus.power_on() }
        }
        self.sync_gus_irq();
        self.clock.schedule(self.next_event());
    }

    /// Port access of the Sound Blaster at `offset` from its base: FM
    /// ports go to the OPL.
    fn sb_write(&mut self, offset: u16, value: u8) {
        let model = self.sb.as_ref().map(|sb| sb.config.model);
        let fm_ports = model != Some(crate::sb::SbModel::Sb2);
        match offset {
            0x0 | 0x8 => self.opl.write_address(0, value),
            0x2 if fm_ports => self.opl.write_address(1, value),
            0x1 | 0x9 => self.opl_data(0, value),
            0x3 if fm_ports => self.opl_data(1, value),
            _ => {
                self.audio_catch_up();
                let mut log = Vec::new();
                if let Some(sb) = &mut self.sb {
                    sb.write(offset, value, &mut log);
                }
                for line in log {
                    self.log_string(&line);
                }
                self.update_irq_levels();
                self.clock.schedule(self.next_event());
            }
        }
    }

    fn sb_read(&mut self, offset: u16) -> u8 {
        match offset {
            0x0 | 0x2 | 0x8 => self.opl.read_status(self.clock.now_micros()),
            _ => {
                self.sb_advance();
                let value = self.sb.as_mut().map_or(0xFF, |sb| sb.read(offset));
                self.update_irq_levels();
                self.clock.schedule(self.next_event());
                value
            }
        }
    }

    /// Write an FM register at its emulated time.
    fn opl_data(&mut self, bank: usize, value: u8) {
        self.audio_catch_up();
        let now = self.clock.now_micros();
        self.opl.write_data(bank, value, now);
    }

    /// The Sound Blaster's base port, if the card is there.
    fn sb_base(&self) -> Option<u16> {
        self.sb.as_ref().map(|sb| sb.config.base)
    }

    /// Power-on state of the PIT channel 0 and the PIC, so a program that
    /// exits (or is killed) with a fast timer or masked IRQs doesn't leave
    /// them behind for the shell and the next program.
    pub fn reset_timers(&mut self) {
        // Mode 3, count 65536, counting from now.
        self.pit0.set_mode(3);
        self.pit0.write_count(0, self.clock.now_ticks());
        self.pit0_divisor = 0;
        self.pit0_write_msb = false;
        self.pit0_read_msb = false;
        self.pit0_access = 3;
        self.pit0_latched_active = false;
        self.pic = crate::pic::Pic::new();
        self.clock.schedule(self.next_event());
    }

    /// Level-triggered request lines (bit n = IRQ n): the Sound Blaster
    /// holds its line until the driver acknowledges at port 22Eh.
    #[inline(always)]
    fn irq_levels(&self) -> u16 {
        self.irq_levels
    }

    /// Recompute the level-triggered request lines after a device changed
    /// its interrupt output.
    fn update_irq_levels(&mut self) {
        self.irq_levels = match &self.sb {
            Some(sb) if sb.irq_pending() => 1 << sb.config.irq,
            _ => 0,
        };
        self.refresh_irq();
    }

    /// The IRQ (0-15) the PICs would deliver now: requested, not masked,
    /// and not blocked by an interrupt of equal or higher priority still in
    /// service.
    #[inline(always)]
    pub fn pic_pending_irq(&self) -> Option<u8> {
        self.pic.pending(self.irq_levels())
    }

    /// Whether any device requests an interrupt on a line its PIC doesn't
    /// mask, or a mouse event handler call waits: the cheap test the CPU
    /// makes before each instruction, ahead of the PIC's priority logic.
    /// Masked requests stay out of it: a driver that polls its card with the
    /// card's IRQ masked (HMI's Ultrasound driver) leaves the request
    /// latched for as long as it runs.
    #[inline(always)]
    pub fn interrupt_requested(&self) -> bool {
        let levels = self.irq_levels();
        let master = (self.pic.master.irr | levels as u8) & !self.pic.master.imr;
        let slave = (self.pic.slave.irr | (levels >> 8) as u8) & !self.pic.slave.imr;
        master | slave != 0 || self.mouse.pending_callback_events & self.mouse.callback_mask != 0
    }

    /// Work out `irq_ready` again.
    #[inline]
    pub fn refresh_irq(&mut self) {
        self.irq_ready = self.interrupt_requested();
    }

    /// The CPU takes interrupt `irq`: it is in service until an EOI.
    /// Returns its vector.
    pub fn pic_acknowledge(&mut self, irq: u8) -> u8 {
        let vector = self.pic.acknowledge(irq);
        self.refresh_irq();
        vector
    }

    /// Discard a request for `irq`, for lines with no handler installed.
    pub fn pic_drop(&mut self, irq: u8) {
        self.pic.lower(irq);
        if let Some(sb) = &mut self.sb
            && sb.config.irq == irq
        {
            sb.irq8 = false;
            sb.irq16 = false;
        }
        self.update_irq_levels();
    }

    /// Raise IRQ 1 if a byte just entered the keyboard controller's output
    /// buffer.
    pub fn sync_keyboard_irq(&mut self) {
        if self.kbc.take_irq() {
            self.pic.raise(1);
            self.refresh_irq();
        }
    }

    /// Carry out what a keyboard controller or port 92h write asked for.
    fn apply_kbc_effects(&mut self, effects: crate::kbc::Effects) {
        if let Some(a20) = effects.a20 {
            self.set_a20(a20);
        }
        if effects.reset {
            self.reset_requested = true;
        }
        self.sync_keyboard_irq();
    }

    /// Write to an I/O port.
    pub fn io_write(&mut self, port: u16, value: u8) {
        self.clock.stall(crate::timer::IO_WRITE_NS);
        self.write_port(port, value);
        // The write may have programmed the PICs or made a device raise or
        // withdraw its interrupt.
        self.refresh_irq();
    }

    fn write_port(&mut self, port: u16, value: u8) {
        match port {
            // The two 8259 interrupt controllers.
            0x20 | 0x21 | 0xA0 | 0xA1 => self.pic.write(port, value),

            // 8042 keyboard controller: data and command ports.
            0x60 => {
                let effects = self.kbc.write_data(value);
                self.apply_kbc_effects(effects);
            }
            0x64 => {
                let effects = self.kbc.write_command(value);
                self.apply_kbc_effects(effects);
            }

            // CMOS RAM / real-time clock.
            0x70 => self.cmos.write_index(value),
            0x71 => self.cmos.write_data(value),

            // System control port A: bit 1 is the "fast A20" gate, bit 0
            // resets the CPU.
            0x92 => {
                self.set_a20(value & 0x02 != 0);
                if value & 0x01 != 0 {
                    self.reset_requested = true;
                }
            }

            // POST code ports (80h on a PC, 190h for test ROMs).
            0x80 | 0x190 => self.post_code = value,
            0xE9 => {
                if self.debug_console.len() < 16 << 20 {
                    self.debug_console.push(value);
                }
            }
            // Delay port, and the coprocessor's busy latch.
            0xED | 0xF0 | 0xF1 => {}

            // Port 0x40: Channel 0 Data (System Timer)
            // Controls the system tick rate (IRQ 0).
            // Default is 18.2 Hz (Divisor 65535).
            0x40 => {
                let complete = match self.pit0_access {
                    1 => {
                        self.pit0_divisor = value as u16;
                        true
                    }
                    2 => {
                        self.pit0_divisor = (value as u16) << 8;
                        true
                    }
                    _ if !self.pit0_write_msb => {
                        // Write LSB
                        self.pit0_divisor = (self.pit0_divisor & 0xFF00) | (value as u16);
                        self.pit0_write_msb = true; // Next write is MSB
                        false
                    }
                    _ => {
                        // Write MSB
                        self.pit0_divisor = (self.pit0_divisor & 0x00FF) | ((value as u16) << 8);
                        self.pit0_write_msb = false; // Reset to LSB
                        true
                    }
                };
                if complete {
                    // Programs that play sound through the timer rewrite the
                    // count on every tick. Only log the first count after a
                    // control word, which is how programs set a new rate.
                    let was_counting = self.pit0.is_counting();
                    self.pit0
                        .write_count(self.pit0_divisor, self.clock.now_ticks());
                    self.clock.schedule(self.next_event());
                    if !was_counting {
                        self.log_string(&format!(
                            "[PIT] Channel 0 Frequency set to {} Hz",
                            1_193_182 / self.pit0.reload()
                        ));
                    }
                }
            }

            // PIT Channel 2 Data (Port 0x42)
            // This sets the frequency.
            // Frequency = 1,193,182 Hz / Divisor
            0x42 => {
                if self.speaker_on {
                    self.audio_catch_up();
                }
                if !self.pit_write_msb {
                    // Write LSB
                    self.pit_divisor = (self.pit_divisor & 0xFF00) | (value as u16);
                    self.pit_write_msb = true; // Next write will be MSB
                } else {
                    // Write MSB
                    self.pit_divisor = (self.pit_divisor & 0x00FF) | ((value as u16) << 8);
                    self.pit_write_msb = false; // Reset to LSB
                    // println!("[PIT] Frequency Divisor Set to: {}", self.pit_divisor);
                }
            }

            // PIT Command Register (Port 0x43)
            0x43 => {
                self.pit_mode = value;

                // Extract the Channel bits (7-6)
                // 00 = Channel 0, 01 = Channel 1, 10 = Channel 2
                let channel = (value >> 6) & 0x03;

                // Access bits (5-4): 00 = latch count value command.
                let access = (value >> 4) & 0x03;

                if access == 0 {
                    // Latch counter command: freeze the current count into
                    // the read buffer so LSB/MSB reads stay consistent.
                    if channel == 0 {
                        self.pit0_latched = self.pit0.count(self.clock.now_ticks());
                        self.pit0_latched_active = true;
                        self.pit0_read_msb = false;
                    }
                } else {
                    match channel {
                        0 => {
                            // New mode: the counter stops until a count is
                            // written, and LSB/MSB sequencing starts over.
                            self.pit0_write_msb = false;
                            self.pit0_read_msb = false;
                            self.pit0_access = access;
                            self.pit0.set_mode((value >> 1) & 0x07);
                            self.clock.schedule(self.next_event());
                        }
                        2 => self.pit_write_msb = false, // Reset Channel 2 LSB/MSB
                        _ => {}
                    }
                }
            }

            // PPI Port B (Speaker Control 0x61)
            // Bit 0: Timer 2 Gate (Must be 1 for timer to run)
            // Bit 1: Speaker Data (Must be 1 for sound to pass to speaker)
            0x61 => {
                // If both Bit 0 and Bit 1 are set, the speaker is ON
                let enabled = (value & 0x03) == 0x03;
                if enabled != self.speaker_on {
                    self.audio_catch_up();
                }
                self.speaker_on = enabled;
            }

            // The FM chip: AdLib ports 388h/389h, and the OPL3's second
            // register bank at 38Ah/38Bh.
            0x388 => {
                self.opl.write_address(0, value);
                // The Ultrasound latches AdLib register numbers too; its
                // detection reads them back at 2XAh.
                if let Some(gus) = &mut self.gus {
                    gus.write_adlib_address(value);
                }
            }
            0x389 => self.opl_data(0, value),
            0x38A => self.opl.write_address(1, value),
            0x38B => self.opl_data(1, value),

            // The Sound Blaster, 16 ports from its base.
            p if self.sb_base().is_some_and(|b| p & 0xFFF0 == b) => self.sb_write(p & 0xF, value),

            // The Gravis Ultrasound, at 2X0h-2XFh and 3X0h-3X7h.
            p if self.gus_claims(p) => self.gus_write(p, value),

            // MPU-401 MIDI interface.
            0x330 => {
                self.audio_catch_up();
                self.mpu.write_data(value);
            }
            0x331 => self.mpu.write_command(value),

            // The DMA controllers and page registers. The sound cards run
            // up to now first, so the transfers they are in see the change
            // when it happens.
            p if crate::dma::Dma::owns(p) => {
                self.sb_advance();
                self.gus_advance();
                self.dma.write(p, value);
                self.clock.schedule(self.next_event());
            }

            // Dispatch to Devices
            // TODO: Use a proper map lookup
            // Ports we intentionally ignore — writes are harmless but other-
            // wise spam the log. Programs blindly touch these as leftovers
            // from CGA/EGA-era code even when they're really talking to VGA.
            0x3D8 | 0x3D9 if self.vga.adapter != video::adapter::Adapter::Cga => {
                // CGA Mode Control / Color Select. Real VGA ignores writes
                // here; VGA mode lives at 0x3D4/0x3D5 (handled by the VGA).
            }
            0x0201 => {
                // Game port write: arm the one-shot timers. Reset the
                // read counter so the next read-loop starts fresh.
                self.joystick_read_count = 0;
            }

            // Super VGA CRTC registers, past the VGA's 00h-18h.
            0x3D5 | 0x3B5 if self.vga.crtc_index >= 0x19 && self.vga.decodes(port) => {
                self.ext_crtc_write(self.vga.crtc_index, value)
            }

            _ => {
                if self.vga.ports().contains(&port) {
                    // A retrace nobody watched may have passed since the
                    // Start Address was last latched: latch it before the
                    // program writes the next one.
                    if matches!(port, 0x3D5 | 0x3B5) && matches!(self.vga.crtc_index, 0x0C | 0x0D) {
                        self.sync_display();
                    }
                    self.vga.io_write(port, value);
                    if matches!(port, 0x3D5 | 0x3B5) && matches!(self.vga.crtc_index, 0x0C | 0x0D) {
                        self.update_vbe_start();
                    }
                    // Suppress the per-write log for DAC ports (0x3C6..0x3C9):
                    // a full 256-color palette update is 1024 writes, which
                    // buries everything else in the trace. Still log the less
                    // frequent mode / register writes.
                    // Log throttling: palette (3C6..3C9) and graphics
                    // index/data (3CE/3CF) churn so much during EGA drawing
                    // that they drown everything else. Log the rarer
                    // mode/register ports.
                    // if !matches!(port, 0x3C6..=0x3C9 | 0x3CE | 0x3CF) {
                    //     self.log_string(&format!(
                    //         "[VGA-IO] Write Port {:04X} Value {:02X}",
                    //         port, value
                    //     ));
                    // }

                    // A CGA's or Hercules card's mode is what its Mode
                    // Control register says, whether the BIOS set it or the
                    // program did.
                    let mode_control = match (port, self.vga.adapter) {
                        (0x3D8, video::adapter::Adapter::Cga) => Some(self.vga.cga_video_mode()),
                        (0x3B8, video::adapter::Adapter::Hercules) => Some(self.vga.herc_video_mode()),
                        _ => None,
                    };
                    if let Some(mode) = mode_control {
                        if mode != self.video_mode {
                            self.log_string(&format!("[VIDEO] Mode Control {:02X}: {:?}", value, mode));
                            self.video_mode = mode;
                            self.vga.mark_dirty_full();
                        }
                    }

                    // Check if video mode changed
                    // (A VESA mode is a 256-color mode to these registers.)
                    if let Some(new_mode) = self.vga.check_video_mode().filter(|_| self.video_mode != VideoMode::Vesa) {
                        if self.video_mode != new_mode && new_mode == VideoMode::Graphics320x200 {
                            self.log_string("[VGA] Switch to Graphics320x200 detected via IO");
                            self.video_mode = new_mode;
                            self.vga.mark_dirty_full();
                        }
                    }
                } else {
                    // Unhandled port write: log the first few to each port.
                    const LOGGED: u8 = 8;
                    let count = &mut self.unhandled_writes[port as usize];
                    if *count < LOGGED {
                        *count += 1;
                        let more = if *count == LOGGED { " (not logging more)" } else { "" };
                        self.log_string(&format!(
                            "[Unhandled IO Write] Port: {:04X}, Value: {:02X}{}",
                            port, value, more
                        ));
                    }
                }
            }
        }
    }

    // Read from an I/O Port
    /// Read from an I/O port.
    pub fn io_read(&mut self, port: u16) -> u8 {
        self.clock.stall(crate::timer::IO_READ_NS);
        let value = self.read_port(port);
        // Reads acknowledge interrupts of some devices (the Sound Blaster's
        // at 22Eh, the Ultrasound's status).
        self.refresh_irq();
        value
    }

    fn read_port(&mut self, port: u16) -> u8 {
        match port {
            // PIC: port 0x20 returns IRR or ISR (selected by OCW3), port
            // 0x21 the interrupt mask. Programs read-modify-write the mask
            // to unmask their IRQ without disturbing the others.
            0x20 | 0x21 | 0xA0 | 0xA1 => self.pic.read(port, self.irq_levels()),

            // Port 0x40 — PIT channel 0 (system timer) data. The counter
            // decrements at 1.193 MHz of emulated time. Programs that need
            // sub-tick timing (MicroProse's VGAME computes 1/elapsed_time,
            // which faults if elapsed == 0) issue a latch command and read
            // LSB then MSB.
            0x40 => {
                let val = if self.pit0_latched_active {
                    self.pit0_latched
                } else {
                    self.pit0.count(self.clock.now_ticks())
                };
                match self.pit0_access {
                    1 => {
                        self.pit0_latched_active = false;
                        (val & 0xFF) as u8
                    }
                    2 => {
                        self.pit0_latched_active = false;
                        (val >> 8) as u8
                    }
                    _ if !self.pit0_read_msb => {
                        self.pit0_read_msb = true;
                        (val & 0xFF) as u8
                    }
                    _ => {
                        self.pit0_read_msb = false;
                        self.pit0_latched_active = false;
                        (val >> 8) as u8
                    }
                }
            }

            // Port 0x0201 — game port (joystick). Reads return four axis
            // bits (one-shot timers, high until they trip) and four button
            // bits (low when pressed). Trip time on real hardware is
            //   t = 24.2 µs + 0.011 µs × R   (R in ohms, max 100 kΩ)
            // → ~24 µs at minimum, ~1124 µs at maximum.
            // We map mouse X/Y across the visible window to the full axis
            // range so games like Carrier Command (which use the joystick
            // as their primary cursor input) follow the host pointer.
            // Mouse buttons map to joystick A buttons 1 and 2.
            0x0201 => {
                // Map mouse position to per-axis read-loop iteration count.
                // On a 4.77 MHz PC, `IN AL, DX` takes ~2.5 µs, so the axis
                // trips after ~10 reads at minimum resistance and ~450 reads
                // at maximum. Matching that range makes games compute the
                // stick position correctly regardless of emulator speed.
                let virt_w = 640i64;
                let virt_h = 200i64;
                let mx = (self.mouse.x.clamp(0, virt_w as i32 - 1)) as i64;
                let my = (self.mouse.y.clamp(0, virt_h as i32 - 1)) as i64;
                let trip_x = (10 + (440 * mx) / (virt_w - 1)) as u32;
                let trip_y = (10 + (440 * my) / (virt_h - 1)) as u32;
                let count = self.joystick_read_count;
                self.joystick_read_count = count.saturating_add(1);

                // Buttons: bit clear = pressed (active-low on real HW).
                let mut value: u8 = 0xF0;
                if (self.mouse.buttons & crate::mouse::BUTTON_LEFT) != 0 {
                    value &= !0x10;
                }
                if (self.mouse.buttons & crate::mouse::BUTTON_RIGHT) != 0 {
                    value &= !0x20;
                }
                // Joystick A axes still timing out → bits 0 and 1 high.
                if count < trip_x {
                    value |= 0x01;
                }
                if count < trip_y {
                    value |= 0x02;
                }
                // Joystick B axes left as "tripped" (low) so games that
                // probe both sticks don't read phantom motion.
                value
            }

            // Port 0x42 — PIT channel 2 (PC speaker tone) data. Some games
            // read this port as a cheap free-running counter for tight
            // timing loops ("wait until count < threshold"), independent of
            // whether they ever use the speaker. Returning a constant would
            // hang those loops, so we synthesize the current count from the
            // active divisor and emulated time, matching real hardware's
            // 1.193 MHz tick rate.
            0x42 => {
                let divisor = if self.pit_divisor == 0 {
                    0x10000u32
                } else {
                    self.pit_divisor as u32
                };
                let ticks = self.clock.now_ticks();
                let rem = (ticks % divisor as u64) as u32;
                let count = ((divisor - 1 - rem) & 0xFFFF) as u16;
                if !self.pit_read_msb {
                    self.pit_read_msb = true;
                    (count & 0xFF) as u8
                } else {
                    self.pit_read_msb = false;
                    (count >> 8) as u8
                }
            }

            // Port 0x60 — Keyboard data port. Real hardware latches the
            // last-received scan code here; programs either read this from
            // their INT 09h ISR after IRQ1 fires, or poll it directly.
            0x60 => {
                let value = self.kbc.read_data();
                // The next queued byte, if any, moved into the buffer.
                self.sync_keyboard_irq();
                value
            }

            // Port 0x64 — Keyboard controller status (8042).
            0x64 => self.kbc.read_status(),

            0x71 => self.cmos.read_data(),
            0x92 => (self.a20() as u8) << 1,

            // The FM chip's status register: timer flags, which programs
            // poll to detect an AdLib.
            0x388 | 0x38A => self.opl.read_status(self.clock.now_micros()),
            0x389 | 0x38B => 0xFF,

            p if self.sb_base().is_some_and(|b| p & 0xFFF0 == b) => self.sb_read(p & 0xF),

            p if self.gus_claims(p) => self.gus_read(p),

            0x330 => self.mpu.read_data(),
            0x331 => self.mpu.read_status(),

            // The DMA controllers, with the live address and count of a
            // transfer a sound card is running.
            p if crate::dma::Dma::owns(p) => {
                self.sb_advance();
                self.gus_advance();
                let value = self.dma.read(p);
                self.clock.schedule(self.next_event());
                value
            }

            // Read PPI Port B: speaker gate and data, the DRAM refresh
            // request (bit 4, toggles), and the PIT channel 2 output (bit 5).
            0x61 => {
                let mut val = 0;
                if self.speaker_on {
                    val |= 0x03;
                }
                self.refresh_toggle = !self.refresh_toggle;
                if self.refresh_toggle {
                    val |= 0x10;
                }
                if self.pit2_output() {
                    val |= 0x20;
                }
                val
            }

            // VGA Input Status 1: retrace and display enable, at the CRTC's
            // address (3DAh in colour modes, 3BAh in monochrome ones).
            0x3DA | 0x3BA if self.vga.decodes(port) => self.input_status_1(),
            0x3D5 | 0x3B5 if self.vga.crtc_index >= 0x19 && self.vga.decodes(port) => {
                self.ext_crtc_read(self.vga.crtc_index)
            }

            _ => {
                if self.vga.ports().contains(&port) {
                    self.vga.io_read(port)
                } else {
                    0xFF // Default open bus
                }
            }
        }
    }

    /// Latch the display Start Address if emulated time has passed the
    /// start of a vertical retrace since it was last latched, as the CRTC
    /// does at every retrace.
    pub fn sync_display(&mut self) {
        let now = self.clock.now_ns();
        if self.vga.retrace_began(now) {
            self.vga.latch_start_address();
            if self.vbe.latched_start != self.vbe.start {
                self.vbe.latched_start = self.vbe.start;
                self.vga.mark_dirty_full();
            }
        }
    }

    /// Write a Super VGA CRTC register (index 19h and up). Like an S3
    /// card, 6Ah is the bank of the window at A0000h and 69h the high
    /// bits of the display start; the VBE protected-mode interface uses
    /// them.
    fn ext_crtc_write(&mut self, index: u8, value: u8) {
        match index {
            0x69 => {
                self.vbe.start_high = value;
                self.update_vbe_start();
            }
            0x6A => self.vbe.bank = (value & 0x3F) as u32,
            _ => {}
        }
    }

    fn ext_crtc_read(&self, index: u8) -> u8 {
        match index {
            0x69 => self.vbe.start_high,
            0x6A => self.vbe.bank as u8,
            _ => 0,
        }
    }

    /// In a VESA mode the Start Address registers and CRTC 69h give the
    /// display start in doublewords.
    fn update_vbe_start(&mut self) {
        if self.video_mode == VideoMode::Vesa {
            let crtc = &self.vga.crtc_regs;
            let dwords = (self.vbe.start_high as u32) << 16 | (crtc[0x0C] as u32) << 8 | crtc[0x0D] as u32;
            self.vbe.start = dwords * 4;
        }
    }

    /// The size of the screen in the mode's own pixels: the VESA mode's
    /// size, or what the standard mode has.
    pub fn display_size(&self) -> (usize, usize) {
        match (self.video_mode, self.vbe.mode) {
            (VideoMode::Vesa, Some(mode)) => (mode.width as usize, mode.height as usize),
            (mode, _) => mode.dimensions(),
        }
    }

    /// Input Status 1 (port 3DAh): bit 3 in the vertical retrace, bit 0
    /// while display enable is off, from the CRT timing and emulated time.
    /// Reading it also resets the attribute controller's flip-flop.
    fn input_status_1(&mut self) -> u8 {
        self.sync_display();
        self.vga.attribute_flip_flop = false;
        let now = self.clock.now_ns();
        let timing = self.vga.timing();
        match self.vga.adapter {
            video::adapter::Adapter::Hercules => video::hercules::status(&timing, now),
            _ => timing.status(now),
        }
    }

    /// Write a line to the log file and the debug server's log.
    pub fn log_string(&mut self, s: &str) {
        if let Some(log) = &mut self.log_file {
            log.write_line(s);
        }
        if let Some(hook) = &mut self.log_hook {
            hook(s);
        }
    }

    /// Write buffered log lines to the log file now, so they survive an
    /// abort.
    pub fn flush_log(&mut self) {
        if let Some(log) = &mut self.log_file {
            log.flush();
        }
    }
}
