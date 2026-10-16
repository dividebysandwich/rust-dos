use std::collections::VecDeque;
use web_time::Instant;

use crate::disk::{DRIVE_SLOTS, DiskController, DriveKind, MountOptions};
use crate::video::{self, ADDR_VGA_GRAPHICS, SIZE_GRAPHICS, VideoMode};

mod guest;
pub mod port_log;
mod profile;
pub mod s3;
mod et4000;
mod fdc;
mod ide;
mod net;
mod printer;
mod serial;
mod state;
mod setup;
mod drives;
mod sound;
mod powervr;
mod verite;
mod voodoo;

pub use verite::bios as verite_bios;

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

/// `Bus::code_blocks`' marks: a block with code, and every block while
/// the writes are watched.
pub const CODE_BLOCK: u8 = 1;
pub const WATCHED_BLOCK: u8 = 2;

/// Where `Bus` keeps the A20 mask and `irq_ready`, for the dynamic
/// recompiler's code.
pub(crate) const A20_MASK_OFFSET: usize = std::mem::offset_of!(Bus, a20_mask);
pub(crate) const IRQ_READY_OFFSET: usize = std::mem::offset_of!(Bus, irq_ready);

/// How long releasing the time slice lets pass at most, in PIT ticks:
/// about 50 us, what the mode switches of a DOS extender's call to real
/// mode and back take.
const IDLE_SLICE_TICKS: u64 = 60;

/// Fill `range` with elements of `size` bytes (1, 2 or 4) of `value`, as a
/// string store does. By the element's size: a copy of a size known only
/// at run time is a call per element.
#[inline]
fn fill_pattern(range: &mut [u8], size: usize, value: u32) {
    let bytes = value.to_le_bytes();
    match size {
        1 => range.fill(bytes[0]),
        2 => range.as_chunks_mut::<2>().0.fill([bytes[0], bytes[1]]),
        _ => range.as_chunks_mut::<4>().0.fill(bytes),
    }
}

/// A program waiting, as at a HLT, until PIT tick `until`: the code it
/// runs stays where it was (`at`, CS and EIP, from when the loop first
/// sees it), while interrupts come and their handlers run.
#[derive(Clone, Copy, Debug)]
pub struct Hold {
    pub until: u64,
    pub at: Option<(u16, u32)>,
}

/// An observer of the log's lines (`Bus::log_hook`).
pub type LogHook = Box<dyn FnMut(&str)>;
/// An observer of the mixed audio (`Bus::audio_hook`).
pub type AudioHook = Box<dyn FnMut(&[i16])>;

pub struct Bus {
    ram: Vec<u8>, // System RAM, reallocated only by a state of another size
    pub video_mode: VideoMode, // Current State
    pub disk: DiskController,
    /// The operating system booted from a disk image (BOOT), which has the
    /// machine to itself: None while the built-in DOS runs.
    pub boot: Option<crate::boot::BootState>,
    /// The page tables the BIOS's services reach memory through while a
    /// service runs with paging on, and the first page they found missing
    /// (its linear address and the page fault's error code): see
    /// `guest.rs`.
    pub guest_paging: Option<crate::cpu::paging::GuestPaging>,
    pub guest_fault: Option<(u32, u32)>,
    pub keyboard_buffer: VecDeque<u16>, // Stores (Scancode << 8) | ASCII
    /// How many times a service waited for a key (`Cpu::key_wait`):
    /// `rust-dos run` ends a program that waits for good.
    pub key_waits: u32,
    /// The keyboard's layout and the keys held (see keyboard.rs).
    pub kbd: crate::keyboard::KeyboardState,
    /// The 8042 keyboard controller (ports 60h/64h): scan codes for
    /// programs that read the keyboard themselves, the A20 gate and the
    /// CPU reset line.
    pub kbc: crate::kbc::Kbc,
    /// When the keyboard's next byte enters the controller's output
    /// buffer (`Kbc::release`), in PIT ticks.
    pub kbc_release_at: Option<u64>,
    /// When the clock's next periodic interrupt comes, in PIT ticks.
    pub rtc_next: Option<u64>,
    /// The A20 gate as a mask for physical addresses: with the gate
    /// closed, address line 20 is forced to 0 and addresses wrap at 1 MB
    /// as on an 8086. See `a20()` and `set_a20()`.
    a20_mask: u32,
    /// Something asked for a CPU reset (8042 or port 92h); the execution
    /// loop carries it out.
    pub reset_requested: bool,
    /// Writes to the ROMs logged so far (`note_rom_write`).
    rom_writes: u16,
    /// The port accesses a BIOS service left for a V86 monitor to see,
    /// which the BIOS makes on its way out (`bios::PORT_ACCESSES`).
    pub port_accesses: VecDeque<crate::bios::PortAccess>,
    /// The port accesses made, while a debugger keeps a log of them.
    pub port_log: Option<port_log::PortLog>,
    /// What the profiler counts, while a profile runs (`profile::Session`).
    pub profile: Option<Box<crate::profile::Counters>>,
    /// The DOSCONFIG command asked for the settings window; the frontend
    /// opens it.
    pub config_ui_requested: bool,
    /// The EXIT command asked to turn the machine off; the frontend quits.
    pub exit_requested: bool,
    /// The folder of the configuration file (or game profile) whose startup
    /// commands run, which MOUNT -pr takes relative paths from.
    pub config_dir: Option<std::path::PathBuf>,
    pub cmos: crate::cmos::Cmos,
    /// The XMS driver's allocations and A20 state.
    pub xms: crate::xms::Xms,
    /// The expanded memory manager (INT 67h), if EMS is on.
    pub ems: Option<crate::ems::Ems>,
    /// The DPMI host (INT 2Fh AX=1687h, INT 31h) and its clients.
    pub dpmi: crate::dpmi::Dpmi,
    /// The DOS version programs are told (`dos_version`).
    pub dos_version: crate::config::DosVersion,
    /// A booted system's hard disks are ATA disks on the IDE channels too
    /// (`ide_hard_disks`).
    pub ide_hard_disks: bool,
    /// A booted system has a floppy disk controller (`floppy_controller`).
    pub floppy_controller: bool,
    /// A booted system has a CD-ROM drive even with no CD mounted, for one
    /// to go in later (`boot_cdrom`).
    pub boot_cdrom: bool,
    /// Windows' idle call-outs wait for the next event (`idle_hint`).
    pub idle_hint: bool,
    /// What the user should hear of the host folders shared with a booted
    /// system (`shared_disk`): copies back and failures, for the screen.
    pub disk_notices: Vec<String>,
    /// Upper memory blocks, if DOS has them (see mcb.rs).
    pub umb: Option<crate::mcb::Umb>,
    /// Whether DOS packs its tables below the first MCB (`dos_high`), as
    /// DOS=HIGH leaves conventional memory (`dos_data::layout`).
    pub dos_high: bool,
    /// The Covox or Disney Sound Source on LPT1, if there is one.
    pub lpt_dac: Option<crate::lpt_dac::LptDac>,
    /// The printer on LPT1 (`[printer]`), while the DAC isn't there.
    pub printer: Option<crate::printer::Printer>,
    /// The `[printer]` settings, which the printer has when LPT1 is free.
    pub printer_settings: crate::printer::PrinterSettings,
    /// Where the printer's files go when `docpath` doesn't say, in place
    /// of the capture folder (the libretro core's).
    pub printer_dir: Option<std::path::PathBuf>,
    /// The Tandy's and PCjr's sound chip (`tandy`), heard while
    /// `tandy_sound_enabled`.
    pub tandy_sound: crate::sn76489::Sn76489,
    pub tandy_mode: crate::sn76489::TandySound,
    /// Values the settings window's Cheats page froze, put back before
    /// every frame while the program runs (`apply_freezes`).
    pub freezes: Vec<crate::cheats::Freeze>,
    /// The MIXER command changed the mixer's settings, which the frontend
    /// takes over into its own.
    pub mixer_changed: bool,
    /// The frames the program drew: the retraces after which the picture
    /// had changed, or while it flips pages, the flips (see `sync_display`).
    pub frames_drawn: u64,
    /// What the program does that tells `cycles=auto` the speed it needs:
    /// the frames it draws into video memory and flips, its polling.
    pub activity: crate::autospeed::Activity,
    /// Retraces counted, and the one of the last page flip.
    retraces: u64,
    last_flip: Option<u64>,
    /// The Tandy's or PCjr's picture memory as it was at the last retrace,
    /// and where it was: programs (and the recompiler) write it as plain
    /// RAM, which marks nothing for repainting.
    gate_array_shadow: Vec<u8>,
    gate_array_shadow_at: usize,
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
    /// The game port at 201h and the joysticks on it (see joystick.rs).
    pub joystick: crate::joystick::GamePort,
    /// The Forte VFX1's card and driver (`[vfx1]`, vfx1/).
    pub vfx1: Option<crate::vfx1::Vfx1>,
    pub audio_device: Option<Box<dyn crate::audio::AudioOutput>>,
    pub speaker_on: bool,    // Is the speaker playing?
    pub pit_divisor: u16,    // Current Frequency Divisor
    pub pit_read_msb: bool,  // Channel 2 read LSB/MSB toggle
    pub pit_mode: u8,        // PIT Command Mode
    pub pit_write_msb: bool, // Toggle to handle 2-byte writes (LSB/MSB)
    /// Channel 2's count and output as programs read them, with its gate.
    pub pit2: crate::timer::Pit2,
    /// Port 61h bit 1, the speaker data enable.
    pub speaker_data: bool,
    /// Port 61h bits 2 and 3 as written, the RAM parity and I/O channel
    /// check enables, which reads give back.
    pub ppi_checks: u8,
    /// The `pit2_gate_at_start` setting, which a reset puts back.
    pub pit2_gate_at_start: bool,
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
    /// The S3 Trio64's graphics engine, on `Adapter::S3`.
    pub s3_engine: crate::video::s3::engine::Engine,
    /// The S3 ViRGE's own 2D and 3D engines and streams processor, on
    /// `Adapter::S3Virge` and `S3VirgeVx`.
    pub virge: crate::video::s3::virge::Virge,
    /// The Rendition Vérité's registers and FIFO, on `Adapter::Verite`.
    pub verite: crate::verite::Verite,
    /// A memory-mapped read of Input Status 1 (a ViRGE's 83DAh) happened,
    /// whose reset of the attribute flip-flop is done at the next port
    /// access, as memory reads can't change the machine.
    pub attribute_reset: std::cell::Cell<bool>,
    /// The PCI bus's configuration address, on `Adapter::S3` or with the
    /// 3dfx card.
    pub pci: crate::pci::Pci,
    /// The 3dfx Voodoo Graphics card, if there is one.
    pub voodoo: Option<crate::voodoo::Voodoo>,
    /// The PowerVR card, if there is one.
    pub powervr: Option<crate::powervr::PowerVr>,
    /// The interrupt request the PowerVR card has up.
    powervr_line: Option<u8>,
    /// The program waits for a swap the frame rate cap holds back
    /// (`exec::held`).
    pub hold: Option<Hold>,
    /// What to tell the user on the screen, a title over details, for the
    /// frontend to show.
    pub notices: Vec<(String, String)>,
    /// The program last told it lacks Glide's DOS overlay (`overlay`).
    pub glide_hint: Option<u16>,
    /// A booted system's IDE channels, primary and secondary, with its
    /// hard disks and CD-ROM drive.
    pub ide: [Option<crate::ide::Channel>; 2],
    /// The port accesses going on are the BIOS's show of driving an IDE
    /// disk (`ide::int13`) or the floppy disk controller (`interrupts::int13`)
    /// for a V86 monitor: the disks and the controller answer at once.
    pub faked_io: bool,
    /// A booted system's floppy disk controller, with drives 0 and 1.
    pub fdc: Option<crate::fdc::Fdc>,
    pub search_handles: std::collections::HashMap<u32, String>,
    /// The search ID FindFirst handed out last (the key of
    /// `search_handles`, kept in the program's DTA).
    pub search_serial: u32,

    // Mouse State (INT 33h)
    pub mouse: crate::mouse::MouseState,

    /// The FM synthesizer (OPL3 or OPL2) at 388h-38Bh and on the Sound
    /// Blaster's ports.
    pub opl: crate::opl::Opl,
    /// The Sound Blaster, if one is installed.
    pub sb: Option<crate::sb::SoundBlaster>,
    /// The ISA Plug and Play cards: the Sound Blaster when it is a
    /// ViBRA16C (`sbtype=sb16vibra`), None for the others.
    pub isapnp: Option<crate::isapnp::IsaPnp>,
    /// The two 8237 DMA controllers.
    pub dma: crate::dma::Dma,
    /// The MPU-401 MIDI interface at 330h.
    pub mpu: crate::mpu401::Mpu401,
    /// The Gravis Ultrasound, if one is installed.
    pub gus: Option<crate::gus::Gus>,
    /// The IRQ the Ultrasound's interrupt line holds up, if any.
    gus_line: Option<u8>,
    /// The AWE32's EMU8000, with a Sound Blaster AWE32.
    pub awe: Option<Box<crate::awe32::Emu8000>>,
    /// The ROM and RAM (KB) an AWE32 gets.
    awe_rom: std::sync::Arc<[i16]>,
    awe_ram_kb: u32,
    /// The network: the IPX driver, and the thread with the sockets.
    pub net: crate::net::Net,
    /// The serial ports.
    pub serial: crate::serial::Serial,
    /// The drive with the built-in Ultrasound software, if any.
    ultrasnd_drive: Option<u8>,
    /// The CD drive playing audio tracks, for MSCDEX.
    pub cdaudio: crate::cdrom::audio::CdPlayer,
    /// The time disk access takes, and the drives' noises.
    pub disk_io: crate::diskio::DiskIo,
    pub disknoise: crate::disknoise::DiskNoise,
    /// The monitor's sounds (`monitor_effects`).
    pub monitor_noise: crate::monitor_noise::MonitorNoise,
    /// Bumped each time the machine is reset or a system it boots starts:
    /// the monitor's power effects show a power on.
    pub power_ons: u32,
    /// Del during the POST: the front end opens the BIOS setup.
    pub setup_requested: bool,
    /// The drives read or written since the front end last looked (bit n
    /// for drive n), for its activity lights.
    pub drives_active: u32,
    /// What MSCDEX keeps between calls.
    pub mscdex: crate::interrupts::mscdex::MscdexState,
    /// Mixed output (44.1 kHz stereo, interleaved) rendered up to
    /// `audio_frames` frames of emulated time, waiting for `pump_audio`.
    pub audio_out: VecDeque<i16>,
    audio_frames: u64,
    /// The last frames mixed came to exact zeros (see `audio_catch_up`).
    audio_settled: bool,
    /// How `pump_audio` holds the output device's queue at its target.
    pub audio_feed: crate::audio::Feed,
    /// Resampling position and last frame of the Sound Blaster's output.
    sb_phase: f64,
    sb_frame: (i16, i16),
    /// Frames left of a BEL beep.
    pub beep_frames: u32,
    /// The host's volumes for each source (`[mixer]`) and the mute.
    pub mixer: crate::mixer::Mixer,
    /// The Sound Blaster's IRQ and whether its 8-bit and 16-bit interrupts
    /// wait for the driver, as `sync_sb_irq` last saw them.
    sb_irq: (Option<u8>, bool, bool),
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
    /// What the CPU touches while `idle` proves a loop changes nothing:
    /// the blocks of RAM it writes, and whether it did anything else.
    pub observe: crate::idle::Observe,
    /// Per block of RAM (`GEN_SHIFT`), `CODE_BLOCK` if it or the next block
    /// holds code that was decoded (by the decoded-instruction cache) or
    /// translated (by the dynamic recompiler). A block stays marked, and
    /// every write to it bumps its generation. The recompiler's code writes
    /// other blocks of RAM without bumping theirs: nothing decoded depends
    /// on them. (A write of up to 4 bytes that reaches into a block with
    /// code starts in it or the block before, which is marked too.)
    /// `WATCHED_BLOCK` makes the recompiler's writes bump every block's
    /// generation, for the debugger's heatmap (`set_write_tracking`).
    pub code_blocks: Vec<u8>,

    /// Optional observer for every `log_string` line. Installed by the debug
    /// server so log output can be streamed to remote clients.
    pub log_hook: Option<LogHook>,
    /// Optional observer for every block of mixed audio samples produced by
    /// `pump_audio` (44.1 kHz mono i16). Used by the debug audio stream.
    pub audio_hook: Option<AudioHook>,
}

impl Bus {
    /// Whether the Tandy's and PCjr's sound chip is there: on those
    /// machines, or on any with `tandy=on`.
    pub fn tandy_sound_enabled(&self) -> bool {
        match self.tandy_mode {
            crate::sn76489::TandySound::On => true,
            crate::sn76489::TandySound::Off => false,
            crate::sn76489::TandySound::Auto => self.vga.adapter.gate_array(),
        }
    }

    /// The sound chip, the PCjr's SN76496 on a PCjr and the NCR 8496
    /// elsewhere, as the machine is now.
    fn tandy_chip(&mut self) -> &mut crate::sn76489::Sn76489 {
        use crate::sn76489::{Sn76489, Variant};
        let variant = if self.vga.adapter == video::adapter::Adapter::Pcjr { Variant::Sn76496 } else { Variant::Ncr8496 };
        if self.tandy_sound.variant() != variant {
            self.tandy_sound = Sn76489::new(variant);
        }
        &mut self.tandy_sound
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

    /// The characters in a row of an 80-column text mode: 80, or what an
    /// ET4000's CRTC shows (132 in Tseng's wide modes).
    pub fn text_cols(&self) -> usize {
        if self.et4000() { (self.vga.crtc_regs[0x01] as usize + 1).clamp(1, 160) } else { 80 }
    }

    // Helper: Scroll the text screen up by 1 line
    pub fn scroll_up(&mut self) {
        // Read the current row count from BDA so 80x43 / 80x50 modes scroll
        // their whole visible area, not just the first 25 rows.
        let rows = self.text_rows();
        let row_size = self.text_cols() * 2;
        let screen_size = rows * row_size;
        let text = self.text_mem_mut();
        if screen_size > text.len() {
            return;
        }

        // Move memory back
        text.copy_within(row_size..screen_size, 0);

        // Clear bottom row with space + light-gray attribute pairs.
        for (i, byte) in text[screen_size - row_size..screen_size].iter_mut().enumerate() {
            *byte = if i % 2 == 0 { 0x20 } else { 0x07 };
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

    /// RAM for a frontend that reads and pokes it directly (libretro's
    /// memory maps). Writes this way bypass the decoded-instruction cache.
    pub fn ram_mut(&mut self) -> &mut [u8] {
        &mut self.ram
    }

    /// Copy `data` into RAM at `addr`, bypassing the VGA mapping, as program
    /// loaders do. Bytes past the end of RAM are dropped. Bytes for the
    /// video memory window (A0000h-BFFFFh) go through it.
    pub fn load_bytes(&mut self, addr: usize, data: &[u8]) {
        if Self::touches_video(addr, data.len()) {
            for (i, &b) in data.iter().enumerate() {
                self.write_8(addr + i, b);
            }
            return;
        }
        let end = addr.saturating_add(data.len()).min(self.ram.len());
        if addr >= end {
            return;
        }
        self.ram[addr..end].copy_from_slice(&data[..end - addr]);
        self.bump_page_gens(addr, end);
    }

    /// Whether `addr` is in a ROM, which ignores the CPU's writes: the video
    /// BIOS's 32 KB at C0000h, and the BIOS's 64 KB at F0000h.
    pub fn is_rom(addr: usize) -> bool {
        (0xC0000..0xC8000).contains(&addr) || (0xF0000..0x10_0000).contains(&addr)
    }

    /// Put `data` into a ROM at `addr`, as the emulator sets its ROMs up
    /// and keeps its services' data there.
    pub fn write_rom(&mut self, addr: usize, data: &[u8]) {
        debug_assert!(Self::is_rom(addr) && Self::is_rom(addr + data.len() - 1));
        self.load_bytes(addr, data);
    }

    /// Whether the `len` bytes at `addr` are plain RAM (`is_plain_ram`),
    /// for a `len` a program chose: one longer than RAM is not, so that
    /// `addr + len` can't overflow a 32-bit `usize`.
    pub(crate) fn is_plain_run(&self, addr: usize, len: usize) -> bool {
        len <= self.ram.len() && self.is_plain_ram(addr, len)
    }

    /// Append the `len` bytes at `addr` to `out` as programs read them:
    /// plain RAM as one slice, anything else (video memory, the ROMs,
    /// upper memory) through `read_8`.
    pub(crate) fn read_run(&self, addr: usize, len: usize, out: &mut Vec<u8>) {
        if self.is_plain_run(addr, len) {
            out.extend_from_slice(&self.ram[addr..addr + len]);
        } else {
            out.extend((0..len).map(|i| self.read_8(addr.wrapping_add(i))));
        }
    }

    /// Write `data` at `addr` as programs write it, as `read_run` reads
    /// it: plain RAM with one copy, anything else through `write_8`.
    pub(crate) fn write_run(&mut self, addr: usize, data: &[u8]) {
        if self.is_plain_run(addr, data.len()) {
            self.load_bytes(addr, data);
        } else {
            for (i, &b) in data.iter().enumerate() {
                self.write_8(addr.wrapping_add(i), b);
            }
        }
    }

    /// Copy `len` bytes of RAM from `from` to `to`, bypassing the VGA
    /// mapping, as the expanded memory manager maps its pages and the XMS
    /// driver moves blocks. Overlapping ranges copy the source as it was.
    pub fn copy_ram(&mut self, from: usize, to: usize, len: usize) {
        let len = len.min(self.ram.len().saturating_sub(from.max(to)));
        if len == 0 {
            return;
        }
        self.ram.copy_within(from..from + len, to);
        self.bump_page_gens(to, to + len);
    }

    /// Set every RAM byte in `range` to `value`, bypassing the VGA mapping
    /// but for the video memory window, which it goes through.
    pub fn fill_ram(&mut self, range: std::ops::Range<usize>, value: u8) {
        if Self::touches_video(range.start, range.len()) {
            for addr in range {
                self.write_8(addr, value);
            }
            return;
        }
        let end = range.end.min(self.ram.len());
        if range.start >= end {
            return;
        }
        self.ram[range.start..end].fill(value);
        self.bump_page_gens(range.start, end);
    }

    /// Move `count` elements of `size` bytes in RAM from `src` to `dst` one
    /// after the other, as a string move does: from the lowest up, or with
    /// `backward` from the highest down (`src` and `dst` are the first
    /// elements moved either way), so an element may copy one moved before
    /// it. Bumps the code generations of what it writes. Both ranges are in
    /// plain RAM.
    pub(crate) fn move_elements(&mut self, src: usize, dst: usize, count: usize, size: usize, backward: bool) {
        let len = count * size;
        let (src_lo, dst_lo) = if backward { (src + size - len, dst + size - len) } else { (src, dst) };
        // Where an element would read one written before it, one at a time.
        let hazard = if backward { dst < src && dst + len > src } else { dst > src && dst < src + len };
        if !hazard {
            self.ram.copy_within(src_lo..src_lo + len, dst_lo);
        } else {
            // Each element read whole before it is written.
            let mut element = [0u8; 4];
            for k in 0..count {
                let (from, to) = if backward { (src - k * size, dst - k * size) } else { (src + k * size, dst + k * size) };
                element[..size].copy_from_slice(&self.ram[from..from + size]);
                self.ram[to..to + size].copy_from_slice(&element[..size]);
            }
        }
        self.bump_page_gens(dst_lo, dst_lo + len);
    }

    /// Write the `len` bytes of plain RAM at `src` into the VGA's graphics
    /// window at `dst`, as a string move's writes through `write_8` do,
    /// where they are plain (`VgaCard::plain_writes`): each goes to a place
    /// of its own, so the order they go in doesn't matter. Returns false,
    /// having written nothing, where they aren't, or the window isn't the
    /// VGA's (a VESA mode's, the S3's registers).
    pub(crate) fn move_to_vga(&mut self, src: usize, dst: usize, len: usize) -> bool {
        if !self.plain_vga_run(dst, len) {
            return false;
        }
        let offset = self.vga_window(dst, true);
        self.note_video_write(len as u64);
        let ram = &self.ram[src..src + len];
        self.vga.write_plain_run(offset, len, |i| ram[i]);
        true
    }

    /// Fill `len` bytes of the VGA's graphics window at `dst` with `count`
    /// elements of `size` bytes of `value`, as a string store's writes do,
    /// where they are plain (see `move_to_vga`).
    pub(crate) fn fill_vga(&mut self, dst: usize, count: usize, size: usize, value: u32) -> bool {
        if !self.plain_vga_run(dst, count * size) {
            return false;
        }
        let bytes = value.to_le_bytes();
        let offset = self.vga_window(dst, true);
        self.note_video_write((count * size) as u64);
        self.vga.write_plain_run(offset, count * size, |i| bytes[i % size]);
        true
    }

    /// Write the `len` bytes of plain RAM at `src` into the VESA linear
    /// frame buffer at `dst`, as a string move's writes through `write_32`
    /// do (a DirectDraw blit's row). Returns false, having written nothing,
    /// where the range isn't all in the frame buffer. `size` is the
    /// element's, for the profile's count of writes.
    pub(crate) fn move_to_lfb(&mut self, src: usize, dst: usize, len: usize, size: usize) -> bool {
        let Some(offset) = self.lfb_run(dst, len) else {
            return false;
        };
        let _profile = self.profile.as_deref().map(|p| p.mem_run(dst, true, (len / size) as u64));
        self.vbe.vram[offset..offset + len].copy_from_slice(&self.ram[src..src + len]);
        self.lfb_written(dst, offset, len);
        true
    }

    /// Fill the VESA linear frame buffer at `dst` with `count` elements of
    /// `size` bytes of `value`, as a string store's writes do (see
    /// `move_to_lfb`).
    pub(crate) fn fill_lfb(&mut self, dst: usize, count: usize, size: usize, value: u32) -> bool {
        let len = count * size;
        let Some(offset) = self.lfb_run(dst, len) else {
            return false;
        };
        let _profile = self.profile.as_deref().map(|p| p.mem_run(dst, true, count as u64));
        fill_pattern(&mut self.vbe.vram[offset..offset + len], size, value);
        self.lfb_written(dst, offset, len);
        true
    }

    /// Where `len` bytes at `dst` are in the video memory, if all of them
    /// are in the linear frame buffer.
    fn lfb_run(&self, dst: usize, len: usize) -> Option<usize> {
        if dst < self.ram.len() {
            return None;
        }
        self.vbe.lfb_offset(dst, len)
    }

    /// The rest of `write_vram` for `len` bytes written at `offset` (at
    /// `dst`) at once, and the hooks of the writes one by one.
    fn lfb_written(&mut self, dst: usize, offset: usize, len: usize) {
        if self.observe.on {
            self.observe.mapped_write(dst, self.ram.len());
        }
        self.vram_written(offset, len);
    }

    /// Where the byte at `addr` in the graphics window at A0000h is for
    /// the VGA's memory logic: its offset in the window, for a write or a
    /// read.
    #[inline]
    fn vga_window(&self, addr: usize, write: bool) -> usize {
        let offset = addr - ADDR_VGA_GRAPHICS;
        if self.et4000() { self.et4000_window(offset, write) } else { offset }
    }

    /// Whether writes of `len` bytes at `dst` all go to the VGA's planes,
    /// plainly.
    fn plain_vga_run(&self, dst: usize, len: usize) -> bool {
        (ADDR_VGA_GRAPHICS..=ADDR_VGA_GRAPHICS + SIZE_GRAPHICS - len).contains(&dst)
            && self.video_mode != VideoMode::Vesa
            && self.s3_mmio(dst).is_none()
            && self.vga.plain_writes()
    }

    /// Fill `count` elements of `size` bytes in RAM from `dst` up with
    /// `value`, as a string store does, and bump their code generations.
    /// The range is in plain RAM.
    pub(crate) fn fill_elements(&mut self, dst: usize, count: usize, size: usize, value: u32) {
        fill_pattern(&mut self.ram[dst..dst + count * size], size, value);
        self.bump_page_gens(dst, dst + count * size);
    }

    /// Whether `len` bytes at `addr` reach into the video memory window,
    /// A0000h-BFFFFh.
    fn touches_video(addr: usize, len: usize) -> bool {
        len > 0 && addr < 0xC0000 && addr.saturating_add(len) > ADDR_VGA_GRAPHICS
    }

    /// Note that code was decoded or translated from the RAM at
    /// `start..end`: writes to its blocks bump their generations from now
    /// on (see `code_blocks`).
    #[inline]
    pub fn mark_code(&mut self, start: usize, end: usize) {
        let blocks = &mut self.code_blocks;
        let last = ((end - 1) >> GEN_SHIFT).min(blocks.len() - 1);
        for block in &mut blocks[(start >> GEN_SHIFT).saturating_sub(1)..=last] {
            *block |= CODE_BLOCK;
        }
    }

    /// Make the recompiler's writes to every block of RAM bump its
    /// generation (`page_gen`), as the interpreter's do, so that all the
    /// machine's writes show; or only those to blocks with code again.
    pub fn set_write_tracking(&mut self, on: bool) {
        for block in &mut self.code_blocks {
            if on {
                *block |= WATCHED_BLOCK;
            } else {
                *block &= !WATCHED_BLOCK;
            }
        }
    }

    /// Invalidate cached decodes of the pages covering `start..end`.
    fn bump_page_gens(&mut self, start: usize, end: usize) {
        if self.observe.on {
            self.observe.wrote(start >> GEN_SHIFT, (end - 1) >> GEN_SHIFT);
        }
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
    pub(crate) fn is_plain_ram(&self, addr: usize, len: usize) -> bool {
        addr + len <= ADDR_VGA_GRAPHICS || (addr >= 0x10_0000 && addr + len <= self.ram.len())
    }

    /// Read `S` bytes (1, 2 or 4) of plain RAM at `addr`, which
    /// `is_plain_ram` accepted.
    #[inline(always)]
    pub fn ram_read<const S: u8>(&self, addr: usize) -> u32 {
        debug_assert!(self.is_plain_ram(addr, S as usize));
        // SAFETY: plain RAM is within `ram`.
        unsafe {
            let p = self.ram.as_ptr().add(addr);
            match S {
                1 => *p as u32,
                2 => u16::from_le_bytes(*(p as *const [u8; 2])) as u32,
                _ => u32::from_le_bytes(*(p as *const [u8; 4])),
            }
        }
    }

    /// Write `S` bytes of plain RAM, as `ram_read` reads them, bumping the
    /// generations of the blocks they are in as `write_8` does.
    #[inline(always)]
    pub fn ram_write<const S: u8>(&mut self, addr: usize, value: u32) {
        debug_assert!(self.is_plain_ram(addr, S as usize));
        // SAFETY: plain RAM is within `ram`, and there is a generation for
        // every block of it.
        unsafe {
            let p = self.ram.as_mut_ptr().add(addr);
            match S {
                1 => *p = value as u8,
                2 => *(p as *mut [u8; 2]) = (value as u16).to_le_bytes(),
                _ => *(p as *mut [u8; 4]) = value.to_le_bytes(),
            }
            let g = self.page_gen.get_unchecked_mut(addr >> GEN_SHIFT);
            *g = g.wrapping_add(1);
            if S > 1 {
                let g = self.page_gen.get_unchecked_mut((addr + S as usize - 1) >> GEN_SHIFT);
                *g = g.wrapping_add(1);
            }
        }
        if self.observe.on {
            self.observe.wrote(addr >> GEN_SHIFT, (addr + S as usize - 1) >> GEN_SHIFT);
        }
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
        let _profile = self.profile.as_deref().map(|p| p.mem(addr, false));
        self.mapped_8(addr)
    }

    /// `read_8_mapped` without the profiler's count, for debuggers.
    fn mapped_8(&self, addr: usize) -> u8 {
        if self.observe.on {
            self.observe.mapped(addr, self.ram.len());
        }
        if let Some(port) = self.s3_mmio(addr) {
            return self.mmio_peek(port, 1) as u8;
        }
        if (ADDR_VGA_GRAPHICS..ADDR_VGA_GRAPHICS + SIZE_GRAPHICS).contains(&addr) {
            // VESA modes: the window onto the bank of video memory.
            if self.video_mode == VideoMode::Vesa {
                return self.vbe.vram[self.vbe.window_offset(addr - ADDR_VGA_GRAPHICS)];
            }
            // Route through VGA so chain-4, odd/even, and Read Map Select
            // work correctly. read_graphics also latches planes, needed
            // for planar read-modify-write sequences.
            return self.vga.read_graphics(self.vga_window(addr, false));
        }
        if let Some(ram) = self.vga.cpu_window(addr) {
            return self.ram[ram];
        }
        let (text, size, wrap) = self.vga.text_window();
        if (text..text + size).contains(&addr) {
            return self.vga.vram_text[(addr - text) & wrap];
        }
        if addr < self.ram.len() {
            return self.ram[addr];
        }
        if let Some(offset) = self.vbe.lfb_offset(addr, 1) {
            return self.vbe.vram[offset];
        }
        if let Some(at) = self.powervr_at(addr) {
            return self.powervr_read(at, 1) as u8;
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
        if let Some(dword) = self.voodoo_peek_32(addr) {
            return dword as u8;
        }
        if let Some((crate::powervr::Window::Registers, offset)) = self.powervr_at(addr)
            && let Some(p) = &self.powervr
        {
            return (p.peek_register(offset & !3) >> (8 * (offset & 3))) as u8;
        }
        if addr >= self.ram.len() {
            return self.mapped_8(addr);
        }
        if (ADDR_VGA_GRAPHICS..ADDR_VGA_GRAPHICS + SIZE_GRAPHICS).contains(&addr) && self.video_mode != VideoMode::Vesa {
            let saved = self.vga.latches.get();
            let v = self.vga.read_graphics(self.vga_window(addr, false));
            self.vga.latches.set(saved);
            return v;
        }
        // (Not `read_8`: a profile counts the program's reads, not these.)
        if self.is_plain_ram(addr, 1) { self.ram[addr] } else { self.mapped_8(addr) }
    }

    /// `write_8`, `write_16` or `write_32` (`size` bytes of `value`) at
    /// `addr` where it is a plain write into the VGA's planes through the
    /// window at A0000h (`VgaCard::write_planes_plainly`), for the dynamic
    /// recompiler's code. False, with nothing done, where it isn't: those
    /// then write it.
    #[inline]
    #[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
    /// `VgaCard::plain_planes`, where writes to the graphics window go to
    /// the VGA's planes as `write_planes_plainly` has them.
    #[cfg_attr(not(dynrec), allow(dead_code))]
    pub(crate) fn plain_planes(&mut self) -> Option<(u8, *mut u8, usize)> {
        if self.video_mode == VideoMode::Vesa || self.et4000() || (self.s3() && self.vga.s3.mmio()) {
            return None;
        }
        self.vga.plain_planes()
    }

    pub(crate) fn write_planes_plainly(&mut self, addr: usize, value: u32, size: usize) -> bool {
        if !(ADDR_VGA_GRAPHICS..=ADDR_VGA_GRAPHICS + SIZE_GRAPHICS - size).contains(&addr)
            || self.video_mode == VideoMode::Vesa
            || self.et4000()
            || (self.s3() && self.vga.s3.mmio())
            || !self.vga.write_planes_plainly(addr - ADDR_VGA_GRAPHICS, value, size)
        {
            return false;
        }
        self.note_video_write(size as u64);
        true
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
            if self.observe.on {
                self.observe.wrote(addr >> GEN_SHIFT, addr >> GEN_SHIFT);
            }
            return false;
        }
        let _profile = self.profile.as_deref().map(|p| p.mem(addr, true));
        if self.observe.on {
            self.observe.mapped_write(addr, self.ram.len());
        }
        if let Some(port) = self.s3_mmio(addr) {
            self.mmio_write(port, value as u32, 1);
            return true;
        }
        if (ADDR_VGA_GRAPHICS..ADDR_VGA_GRAPHICS + SIZE_GRAPHICS).contains(&addr) {
            if self.video_mode == VideoMode::Vesa {
                let offset = self.vbe.window_offset(addr - ADDR_VGA_GRAPHICS);
                self.write_vram(offset, &[value]);
                return true;
            }
            self.note_video_write(1);
            // write_graphics already sets vga.dirty unconditionally. The
            // Return value only matters to callers that care whether the
            // write hit the active display plane, but rendering is gated
            // off vga.dirty directly, so we simplify here.
            self.vga.write_graphics(self.vga_window(addr, true), value);
            return matches!(
                self.video_mode,
                VideoMode::Graphics320x200
                    | VideoMode::Ega320x200
                    | VideoMode::Ega640x200
                    | VideoMode::Ega640x350
                    | VideoMode::Vga640x480
            );
        }
        // The Tandy's and PCjr's window onto system memory.
        if let Some(ram) = self.vga.cpu_window(addr) {
            self.ram[ram] = value;
            let page = ram >> GEN_SHIFT;
            self.page_gen[page] = self.page_gen[page].wrapping_add(1);
            let (base, size) = self.vga.crt_range();
            if !(base..base + size).contains(&ram) {
                return false;
            }
            self.mark_text_dirty(ram - base, ram - base + 1);
            return true;
        }
        let (text, size, wrap) = self.vga.text_window();
        if (text..text + size).contains(&addr) {
            let text_off = (addr - text) & wrap;
            self.vga.vram_text[text_off] = value;
            self.note_video_write(1);

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

        // The ROMs ignore writes; the upper memory between them is RAM.
        // Writes past the end of RAM go nowhere.
        if Self::is_rom(addr) {
            self.note_rom_write(addr);
            return false;
        }
        if addr < self.ram.len() {
            self.ram[addr] = value;
            let page = addr >> GEN_SHIFT;
            self.page_gen[page] = self.page_gen[page].wrapping_add(1);
            return false;
        }
        if let Some(offset) = self.vbe.lfb_offset(addr, 1) {
            self.write_vram(offset, &[value]);
            return true;
        }
        if self.voodoo_at(addr).is_some() {
            self.voodoo_write_8();
        }
        if let Some(at) = self.powervr_at(addr) {
            self.powervr_write(at, value as u32, 1);
        }
        false
    }

    /// The program set the display start (CRTC registers 0Ch and 0Dh, or
    /// the VESA BIOS's): a page flip, for `cycles=auto`.
    pub fn note_display_start(&mut self) {
        let start = if self.video_mode == VideoMode::Vesa {
            self.vbe.start
        } else {
            (self.vga.crtc_regs[0x0C] as u32) << 8 | self.vga.crtc_regs[0x0D] as u32
        };
        self.activity.display_start(self.clock.icount, start);
    }

    /// The CPU wrote `bytes` bytes to video memory.
    #[inline]
    fn note_video_write(&mut self, bytes: u64) {
        self.activity.video_write(self.clock.icount, bytes);
    }

    /// Log the first writes to the ROMs, which go nowhere.
    #[cold]
    fn note_rom_write(&mut self, addr: usize) {
        const LOGGED: u16 = 8;
        if self.rom_writes < LOGGED {
            self.rom_writes += 1;
            let more = if self.rom_writes == LOGGED { " (not logging more)" } else { "" };
            self.log_string(&format!("[BIOS] Write to the ROM at {:05X} ignored{}", addr, more));
        }
    }

    /// A write of `size` bytes of `value` at `addr` into the VESA linear
    /// frame buffer, as `write_8`, `write_16` and `write_32` make one there.
    /// False, having written nothing, where the bytes aren't all in it.
    /// Keep it as their frame buffer branches, which stay inline for speed.
    #[inline]
    pub(crate) fn write_lfb(&mut self, addr: usize, value: u32, size: usize) -> bool {
        if addr < self.ram.len() {
            return false;
        }
        let Some(offset) = self.vbe.lfb_offset(addr, size) else {
            return false;
        };
        let _profile = self.profile.as_deref().map(|p| p.mem(addr, true));
        if self.observe.on {
            self.observe.mapped_write(addr, self.ram.len());
        }
        self.write_vram(offset, &value.to_le_bytes()[..size]);
        true
    }

    /// Write VESA video memory, marking the rows of the picture it shows
    /// in for repainting.
    #[inline]
    fn write_vram(&mut self, offset: usize, bytes: &[u8]) {
        self.vbe.vram[offset..offset + bytes.len()].copy_from_slice(bytes);
        self.vram_written(offset, bytes.len());
    }

    /// Count `len` bytes written to VESA video memory at `offset` and mark
    /// the rows of the picture they show in for repainting.
    #[inline]
    fn vram_written(&mut self, offset: usize, len: usize) {
        self.note_video_write(len as u64);
        if self.video_mode == VideoMode::Vesa
            && let Some((first, last)) = self.vbe.frame_rows(offset, len)
        {
            self.vga.mark_dirty_rows(first, last);
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
            if self.observe.on {
                self.observe.wrote(addr >> GEN_SHIFT, (addr + 1) >> GEN_SHIFT);
            }
            return false;
        }
        let _profile = self.profile.as_deref().map(|p| p.mem(addr, true));
        if self.observe.on {
            self.observe.mapped_write(addr, self.ram.len());
        }
        if let Some(offset) = self.vbe.lfb_offset(addr, 2) {
            self.write_vram(offset, &value.to_le_bytes());
            return true;
        }
        if let Some(port) = self.s3_mmio(addr) {
            self.mmio_write(port, value as u32, 2);
            return true;
        }
        if let Some(offset) = self.voodoo_at(addr) {
            self.voodoo_write_16(offset, value);
            return true;
        }
        if addr & 3 != 3
            && let Some(at) = self.powervr_at(addr)
        {
            self.powervr_write(at, value as u32, 2);
            return false;
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
        let _profile = self.profile.as_deref().map(|p| p.mem(addr, false));
        if self.observe.on {
            self.observe.mapped(addr, self.ram.len());
        }
        if let Some(offset) = self.vbe.lfb_offset(addr, 2) {
            return u16::from_le_bytes([self.vbe.vram[offset], self.vbe.vram[offset + 1]]);
        }
        if let Some(port) = self.s3_mmio(addr) {
            return self.mmio_peek(port, 2) as u16;
        }
        if let Some(offset) = self.voodoo_at(addr) {
            return self.voodoo_read_16(offset);
        }
        if addr & 3 != 3
            && let Some(at) = self.powervr_at(addr)
        {
            return self.powervr_read(at, 2) as u16;
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
        let _profile = self.profile.as_deref().map(|p| p.mem(addr, false));
        if self.observe.on {
            self.observe.mapped(addr, self.ram.len());
        }
        if let Some(offset) = self.vbe.lfb_offset(addr, 4) {
            let v = &self.vbe.vram[offset..offset + 4];
            return u32::from_le_bytes([v[0], v[1], v[2], v[3]]);
        }
        if let Some(port) = self.s3_mmio(addr) {
            return self.mmio_peek(port, 4);
        }
        if let Some(offset) = self.voodoo_at(addr) {
            return self.voodoo_read_32(offset);
        }
        if addr & 3 == 0
            && let Some(at) = self.powervr_at(addr)
        {
            return self.powervr_read(at, 4);
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
            if self.observe.on {
                self.observe.wrote(addr >> GEN_SHIFT, last);
            }
            return;
        }
        let _profile = self.profile.as_deref().map(|p| p.mem(addr, true));
        if self.observe.on {
            self.observe.mapped_write(addr, self.ram.len());
        }
        if let Some(offset) = self.vbe.lfb_offset(addr, 4) {
            self.write_vram(offset, &value.to_le_bytes());
            return;
        }
        if let Some(port) = self.s3_mmio(addr) {
            self.mmio_write(port, value, 4);
            return;
        }
        if let Some(offset) = self.voodoo_at(addr) {
            self.voodoo_write_32(offset, value);
            return;
        }
        if addr & 3 == 0
            && let Some(at) = self.powervr_at(addr)
        {
            self.powervr_write(at, value, 4);
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

    /// Start an execution batch that runs until instruction `end`.
    pub fn start_batch(&mut self, end: u64) {
        self.clock.set_batch_end(end);
        self.clock.schedule(self.next_event());
        // The PS/2 mouse reports when it moved or a button changed, once
        // the keyboard controller has passed on its last report.
        let now = self.clock.now_micros();
        if self.kbc.aux_idle() && self.mouse.ps2_report_due(now) {
            let packet = self.mouse.take_ps2_packet(now);
            self.kbc.push_aux(&packet);
            self.sync_keyboard_irq();
        }
        // Frames the network passed on since the last batch.
        self.net_poll();
        // The serial mouse's motion.
        self.serial_mouse_poll();
        // A print job that has had nothing for a while ends.
        if self.printer.is_some() {
            self.printer_poll();
        }
        self.refresh_irq();
    }

    /// Change the emulated CPU speed (instructions per emulated ms).
    pub fn set_cycles_per_ms(&mut self, cycles_per_ms: u32) {
        self.clock.set_cycles_per_ms(cycles_per_ms);
        self.clock.schedule(self.next_event());
    }

    /// The next time (PIT ticks) a device needs attention: the timer's
    /// next IRQ 0, the end of the Sound Blaster's current block, the
    /// Ultrasound's next timer, DMA or voice interrupt, or the retrace a
    /// 3dfx card's swap waits for.
    fn next_event(&self) -> Option<u64> {
        let sb = self.sb.as_ref().and_then(|sb| sb.next_event());
        let gus = self.gus_next_event();
        [
            self.pit0.next_event(),
            sb,
            gus,
            self.voodoo_next_event(),
            self.powervr_next_event(),
            self.ide_next_event(),
            self.fdc_next_event(),
            self.net_next_event(),
            self.serial_next_event(),
            self.kbc_release_at,
            self.rtc_next,
        ]
        .into_iter()
            .flatten()
            .min()
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
        if self.voodoo_next_event().is_some_and(|t| t <= now) {
            self.voodoo_service();
        }
        if self.powervr_next_event().is_some_and(|t| t <= now) {
            self.powervr_service();
        }
        if self.ide_next_event().is_some_and(|t| t <= now) {
            self.ide_service();
        }
        if self.fdc_next_event().is_some_and(|t| t <= now) {
            self.fdc_service();
        }
        if self.net_next_event().is_some_and(|t| t <= now) {
            self.net_service();
        }
        if self.serial_next_event().is_some_and(|t| t <= now) {
            self.serial_service();
        }
        // The clock's periodic interrupt.
        if let Some(at) = self.rtc_next
            && at <= now
        {
            self.rtc_next = self.cmos.periodic_ticks().map(|period| if at + period > now { at + period } else { now + period });
            if self.rtc_next.is_some() && self.cmos.periodic_fired() {
                self.pic.raise(8);
            }
        }
        // The keyboard's next byte, once it had time to send it.
        if let Some(at) = self.kbc_release_at
            && at <= now
        {
            self.kbc_release_at = None;
            self.kbc.release();
            self.sync_keyboard_irq();
        }
        self.clock.schedule(self.next_event());
        self.refresh_irq();
    }

    /// Run the Sound Blaster's DSP up to the present.
    fn sb_advance(&mut self) {
        let _profile = crate::profile::zone(crate::profile::Zone::SbDma);
        let now = self.clock.now_ticks();
        if let Some(sb) = &mut self.sb {
            sb.advance(now, &mut self.dma, &self.ram);
            self.sync_sb_irq();
        }
    }

    /// Let the Sound Blaster write what it has for memory (the answer of
    /// DSP command E2h) if its DMA channel takes it.
    fn sb_dma_write(&mut self) {
        if let Some(sb) = &mut self.sb
            && let Some(written) = sb.dma_write(&mut self.dma, &mut self.ram)
        {
            self.bump_page_gens(written.start, written.end);
        }
    }

    /// Run the Ultrasound up to the present and update its interrupt line.
    fn gus_advance(&mut self) {
        let _profile = crate::profile::zone(crate::profile::Zone::Gus);
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

    /// Render the mixed audio of every device up to the present, so a
    /// change a program makes now (an FM register, a DAC sample, the
    /// speaker gate) is heard from now on.
    pub fn audio_catch_up(&mut self) {
        let _profile = crate::profile::zone(crate::profile::Zone::AudioSynth);
        self.sb_advance();
        self.gus_advance();
        let rate = crate::opl::RATE as u64;
        let target = crate::timer::mul_div(self.clock.now_ticks(), rate, crate::timer::PIT_HZ);
        // After a long pause (a debugger stop, a slow host) start afresh
        // rather than render seconds of catch-up.
        if target.saturating_sub(self.audio_frames) > rate / 2 {
            // A CD plays on meanwhile, and the disks spin.
            self.cdaudio.skip(target - rate / 2 - self.audio_frames);
            self.disknoise.skip(target - rate / 2 - self.audio_frames);
            self.serial.skip_sound(target - rate / 2 - self.audio_frames);
            self.audio_frames = target - rate / 2;
        }
        let frames = target.saturating_sub(self.audio_frames) as usize;
        if frames == 0 {
            return;
        }
        self.audio_frames = target;
        // Silence from every source, with the filters and effects run out
        // to exact zeros (the last frames mixed were): the frames are those
        // zeros, without mixing them one by one.
        let quiet = self.audio_quiet();
        if quiet && self.audio_settled {
            if let Some(gus) = &mut self.gus {
                gus.skip_idle(frames, crate::opl::RATE);
            }
            self.audio_out.extend(std::iter::repeat_n(0, 2 * frames));
            self.trim_audio();
            return;
        }
        let mut settled = quiet;

        let divisor = if self.pit_divisor == 0 { 65536.0 } else { self.pit_divisor as f32 };
        let speaker_step = crate::timer::PIT_HZ as f32 / divisor / rate as f32;
        let speaker = self.speaker_on && speaker_step * rate as f32 > 20.0;
        let ((vl, vr), (fl, fr)) = self.sb.as_ref().map_or(((1.0, 1.0), (1.0, 1.0)), |sb| sb.volumes());
        let (cl, cr) = self.sb.as_ref().map_or((1.0, 1.0), |sb| sb.cd_volume());
        let (sb_on, sb_step, dac) = match &self.sb {
            Some(sb) => (
                sb.speaker_on || sb.config.model.is_sb16(),
                sb.out_rate as f64 / rate as f64,
                sb.dac,
            ),
            None => (false, 0.0, 0),
        };
        const SPEAKER: f32 = 3000.0;
        const GUS_GAIN: f32 = 1.0;
        // The Sound Blaster's output filter, as its model has it: its
        // cutoff and order, or none.
        let sb_filter = match &self.sb {
            Some(sb) if self.mixer.settings().sb_filter == crate::mixer::SbFilter::Auto => match sb.config.model {
                crate::sb::SbModel::Sb2 => Some((4800.0, 2)),
                crate::sb::SbModel::SbPro2 => Some((3200.0, 2)),
                // The SB16 filters at half its sample rate.
                m if m.is_sb16() && sb.out_rate >= 4000 => Some((sb.out_rate as f32 / 2.0, 4)),
                _ => None,
            },
            _ => None,
        }
        .filter(|&(cutoff, _)| cutoff < 0.45 * rate as f32);
        // Each source at its volume in the host's mixer, then all of them
        // at the master volume.
        use crate::mixer::Channel;
        let mut peaks = [0.0f32; crate::mixer::CHANNELS];
        let tandy_on = self.tandy_sound_enabled();
        let modem_on = self.serial.sound_playing();
        for _ in 0..frames {
            let mut mix = crate::mixer::MixFrame::default();
            // The PC speaker, and the prompt's beep, through the speaker's
            // filter, which rings on a little after them.
            let mut speaker_sample = 0.0;
            let mut sounding = false;
            if speaker {
                self.audio_phase += speaker_step;
                if self.audio_phase >= 1.0 {
                    self.audio_phase -= 1.0;
                }
                speaker_sample += if self.audio_phase < 0.5 { SPEAKER } else { -SPEAKER };
                sounding = true;
            }
            if self.beep_frames > 0 {
                self.beep_frames -= 1;
                speaker_sample += if self.beep_frames % 50 < 25 { SPEAKER } else { -SPEAKER };
                sounding = true;
            }
            let s = self.mixer.speaker_filter(speaker_sample);
            if sounding || s != 0.0 {
                self.mixer.add(&mut mix, &mut peaks, Channel::Speaker, (s, s));
            }
            let (ol, or) = self.opl.render();
            self.mixer.add(&mut mix, &mut peaks, Channel::Fm, (ol as f32 * fl, or as f32 * fr));
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
                    let mut frame = (self.sb_frame.0 as f32 * vl, self.sb_frame.1 as f32 * vr);
                    if let Some((cutoff, order)) = sb_filter {
                        frame = self.mixer.sb_filter(frame, cutoff, order);
                    }
                    self.mixer.add(&mut mix, &mut peaks, Channel::Sb, frame);
                }
            }
            self.mixer.add(&mut mix, &mut peaks, Channel::Midi, self.mpu.render());
            let (cdl, cdr) = self.cdaudio.render();
            self.mixer.add(&mut mix, &mut peaks, Channel::CdAudio, (cdl * cl, cdr * cr));
            let noise = self.disknoise.render();
            self.mixer.add(&mut mix, &mut peaks, Channel::DiskNoise, (noise, noise));
            if !self.monitor_noise.is_silent() {
                let noise = self.monitor_noise.render();
                self.mixer.add(&mut mix, &mut peaks, Channel::Monitor, (noise, noise));
            }
            if modem_on {
                let s = self.serial.render_sound();
                self.mixer.add(&mut mix, &mut peaks, Channel::Modem, (s, s));
            }
            if let Some(gus) = &mut self.gus {
                let (gl, gr) = gus.pop_frame(crate::opl::RATE);
                self.mixer.add(&mut mix, &mut peaks, Channel::Gus, (gl * GUS_GAIN, gr * GUS_GAIN));
            }
            if let Some(awe) = &mut self.awe {
                // The SB16 mixer's MIDI volume is the synthesizers'.
                let (l, r) = awe.render();
                self.mixer.add(&mut mix, &mut peaks, Channel::Awe, (l * fl, r * fr));
            }
            if let Some(dac) = &mut self.lpt_dac {
                let s = dac.render();
                self.mixer.add(&mut mix, &mut peaks, Channel::LptDac, (s, s));
            }
            if tandy_on {
                let s = self.tandy_sound.render();
                self.mixer.add(&mut mix, &mut peaks, Channel::Tandy, (s, s));
            }
            let (l, r) = self.mixer.finish(mix, &mut peaks);
            settled &= l == 0.0 && r == 0.0;
            self.audio_out.push_back(l.clamp(-32768.0, 32767.0) as i16);
            self.audio_out.push_back(r.clamp(-32768.0, 32767.0) as i16);
        }
        self.audio_settled = settled;
        self.mixer.add_peaks(peaks);
        self.trim_audio();
    }

    /// Whether no source makes a sound, or would move on in a way the
    /// sound coming later depends on, as the frames to mix start (see
    /// `audio_settled`).
    fn audio_quiet(&mut self) -> bool {
        let speaker = self.speaker_on && {
            let divisor = if self.pit_divisor == 0 { 65536.0 } else { self.pit_divisor as f32 };
            crate::timer::PIT_HZ as f32 / divisor > 20.0
        };
        !speaker
            && self.beep_frames == 0
            && self.opl.is_idle()
            && self.sb.as_ref().is_none_or(|sb| sb.out.is_empty() && sb.dac == 0)
            && self.mpu.is_idle()
            && self.cdaudio.is_idle()
            && self.disknoise.is_silent()
            && self.monitor_noise.is_silent()
            && !self.serial.sound_playing()
            && self.gus.as_ref().is_none_or(|g| g.is_idle())
            && self.awe.is_none()
            && self.lpt_dac.is_none()
            && !self.tandy_sound_enabled()
    }

    /// Keep the queues of output in bounds.
    fn trim_audio(&mut self) {
        let rate = crate::opl::RATE as u64;
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

    /// Emulated time in the EMU8000's 44.1 kHz frames.
    fn awe_frames(&self) -> u64 {
        crate::timer::mul_div(self.clock.now_ticks(), 44_100, crate::timer::PIT_HZ)
    }

    /// Whether `port` belongs to the AWE32's EMU8000.
    #[inline]
    pub(crate) fn awe_claims(&self, port: u16) -> bool {
        self.awe.as_ref().is_some_and(|awe| awe.claims(port))
    }

    /// Write the EMU8000: a byte, a word, or a doubleword as two words.
    /// The audio catches up first, so the change is heard from its
    /// sample on.
    pub(crate) fn awe_write(&mut self, port: u16, value: u32, len: u8) {
        let Some(awe) = &self.awe else { return };
        if !awe.latch_only(port) {
            self.audio_catch_up();
        }
        let Some(awe) = &mut self.awe else { return };
        match len {
            1 => awe.write_byte(port, value as u8),
            2 => awe.write_word(port, value as u16),
            _ => {
                awe.write_word(port, value as u16);
                awe.write_word(port.wrapping_add(2), (value >> 16) as u16);
            }
        }
    }

    /// Read the EMU8000, the voices' positions and volumes brought up to
    /// the present.
    pub(crate) fn awe_read(&mut self, port: u16, len: u8) -> u32 {
        let Some(awe) = &self.awe else { return 0xFFFF_FFFF };
        if !awe.latch_only(port) && !awe.reads_clock(port) {
            self.audio_catch_up();
        }
        let now = self.awe_frames();
        let Some(awe) = &mut self.awe else { return 0xFFFF_FFFF };
        match len {
            1 => awe.read_byte(port, now) as u32,
            2 => awe.read_word(port, now) as u32,
            _ => awe.read_word(port, now) as u32 | (awe.read_word(port.wrapping_add(2), now) as u32) << 16,
        }
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
                // The DSP's time up to the command, so what it starts counts
                // from now.
                self.sb_advance();
                let mut log = Vec::new();
                if let Some(sb) = &mut self.sb {
                    sb.write(offset, value, &mut log);
                }
                self.sb_dma_write();
                for line in log {
                    self.log_string(&line);
                }
                self.sync_sb_irq();
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
                self.sync_sb_irq();
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

    /// The Sound Blaster's base port, if the card is there: a ViBRA16C's
    /// is where its Plug and Play registers put it, while it is active. The
    /// Sound Blaster's ports decode on address bits 15-4, so the low 4
    /// bits of a base the registers have are ignored: an unaligned base
    /// still leaves the card somewhere.
    pub fn sb_base(&self) -> Option<u16> {
        match &self.isapnp {
            Some(pnp) => pnp.audio().map(|audio| audio.base & 0xFFF0).filter(|&base| base != 0),
            None => self.sb.as_ref().map(|sb| sb.config.base),
        }
    }

    /// A write to the ISA Plug and Play WRITE_DATA port. A logical device
    /// configured anew takes the Sound Blaster's IRQ and DMA channels
    /// along: none while it is inactive.
    fn isapnp_write(&mut self, value: u8) {
        let Some(pnp) = &mut self.isapnp else { return };
        let before = (pnp.read_port(), pnp.cards.iter().map(|c| c.csn).collect::<Vec<_>>(), pnp.audio());
        pnp.write_data(value);
        let after = (pnp.read_port(), pnp.cards.iter().map(|c| c.csn).collect::<Vec<_>>(), pnp.audio());
        if after.0 != before.0 {
            let port = after.0.map_or("none".to_string(), |p| format!("{:03X}h", p));
            self.log_string(&format!("[ISAPNP] Read port {}", port));
        }
        if after.1 != before.1 {
            self.log_string(&format!("[ISAPNP] CSNs {:?}", after.1));
        }
        if after.2 == before.2 {
            return;
        }
        let audio = after.2;
        let line = match audio {
            Some(a) => format!("[ISAPNP] ViBRA16C audio at {:03X}h, IRQ {:?}, DMA {:?} and {:?}", a.base, a.irq, a.dma8, a.dma16),
            None => "[ISAPNP] ViBRA16C audio deactivated".to_string(),
        };
        self.log_string(&line);
        self.sb_advance();
        self.route_isapnp_audio();
        self.clock.schedule(self.next_event());
    }

    /// The Sound Blaster's IRQ and whether its 8-bit and 16-bit interrupts
    /// wait for the driver.
    fn sb_irq_now(&self) -> (Option<u8>, bool, bool) {
        self.sb.as_ref().map_or((None, false, false), |sb| (sb.irq, sb.irq8, sb.irq16))
    }

    /// Drive the Sound Blaster's interrupt request, edge-triggered as the
    /// PC's PICs are: raise it when the DSP has a new 8-bit or 16-bit
    /// interrupt, withdraw it when the driver has acknowledged them all
    /// before the CPU took it. An interrupt the driver ends at the PIC
    /// without acknowledging at the card doesn't come again, as DMX's
    /// search for the card's IRQ has it.
    fn sync_sb_irq(&mut self) {
        let (irq, irq8, irq16) = self.sb_irq_now();
        let (old_irq, old8, old16) = self.sb_irq;
        let up = irq8 || irq16;
        if let Some(old) = old_irq
            && (old8 || old16)
            && (!up || irq != old_irq)
        {
            self.pic.lower(old);
        }
        if let Some(line) = irq
            && ((irq8 && !old8) || (irq16 && !old16) || (up && irq != old_irq))
        {
            self.pic.raise(line);
        }
        self.sb_irq = (irq, irq8, irq16);
        self.refresh_irq();
    }

    /// The IRQ (0-15) the PICs would deliver now: requested, not masked,
    /// and not blocked by an interrupt of equal or higher priority still in
    /// service.
    #[inline(always)]
    pub fn pic_pending_irq(&self) -> Option<u8> {
        self.pic.pending()
    }

    /// Whether the PICs would let an interrupt through (a request on a line
    /// they don't mask, of a higher priority than those in service), or a
    /// mouse event handler call waits. Masked requests stay out of it: a
    /// driver that polls its card with the card's IRQ masked (HMI's
    /// Ultrasound driver) leaves the request latched for as long as it
    /// runs; so do requests an interrupt handler in service holds back,
    /// which would otherwise send the CPU to the PICs before every
    /// instruction of the handler.
    #[inline(always)]
    pub fn interrupt_requested(&self) -> bool {
        self.pic.pending().is_some() || self.mouse.pending_callback_events & self.mouse.callback_mask != 0
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
            && sb.irq == Some(irq)
        {
            sb.irq8 = false;
            sb.irq16 = false;
        }
        self.sync_sb_irq();
    }

    /// A program released its time slice (INT 2Fh AX=1680h): let up to
    /// `IDLE_SLICE_TICKS` of emulated time pass, to the next device event
    /// at most, so the interrupts that come while it idles come here and
    /// not in the code it runs between its calls. HX's Win32 console takes
    /// a keystroke twice when the keyboard interrupt lands between its
    /// check of its own queue and of the BIOS's buffer.
    /// A guest that said it is idle waits until the next device event, as
    /// a halted CPU does (or `IDLE_SLICE_TICKS` with none due).
    pub fn idle_until_event(&mut self) {
        let now = self.clock.now_ticks();
        let until = self.next_event().unwrap_or(now + IDLE_SLICE_TICKS).max(now);
        self.clock.stall_to(until);
    }

    pub fn release_time_slice(&mut self) {
        let now = self.clock.now_ticks();
        let until = self.next_event().map_or(now + IDLE_SLICE_TICKS, |t| t.min(now + IDLE_SLICE_TICKS));
        self.clock.stall_to(until);
    }

    /// Start or stop the clock's periodic interrupt as its status
    /// registers have it.
    pub fn schedule_rtc(&mut self) {
        match self.cmos.periodic_ticks() {
            Some(period) if self.rtc_next.is_none() => {
                self.rtc_next = Some(self.clock.now_ticks() + period);
                self.clock.schedule(self.next_event());
            }
            Some(_) => {}
            None => self.rtc_next = None,
        }
    }

    /// Raise IRQ 1 if a keyboard byte just entered the keyboard
    /// controller's output buffer, IRQ 12 if a mouse byte did.
    pub fn sync_keyboard_irq(&mut self) {
        if self.kbc.holding() && self.kbc_release_at.is_none() {
            self.kbc_release_at = Some(self.clock.now_ticks() + crate::kbc::KBD_BYTE_TICKS);
            self.clock.schedule(self.next_event());
        }
        if self.kbc.take_irq() {
            self.pic.raise(1);
            self.refresh_irq();
        }
        if self.kbc.take_aux_irq() {
            self.pic.raise(12);
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
        if let Some(byte) = effects.aux {
            let answer = self.mouse.ps2_command(byte, self.clock.now_micros());
            self.kbc.push_aux(&answer);
        }
        self.sync_keyboard_irq();
    }

    /// Write to an I/O port.
    pub fn io_write(&mut self, port: u16, value: u8) {
        let _profile = self.profile.as_mut().map(|p| p.port(port, true));
        self.observe.port_write();
        self.clock.stall(crate::timer::IO_WRITE_NS);
        self.log_port(port, value as u32, 1, true);
        self.write_port(port, value);
        // The write may have programmed the PICs or made a device raise or
        // withdraw its interrupt.
        self.refresh_irq();
    }

    /// Keep a port access in the debugger's log, if there is one.
    #[inline]
    pub(crate) fn log_port(&mut self, port: u16, value: u32, len: u8, write: bool) {
        if let Some(log) = &mut self.port_log {
            log.push(port_log::PortAccess { icount: self.clock.icount, port, value, len, write });
        }
    }

    fn write_port(&mut self, port: u16, value: u8) {
        if self.attribute_reset.take() {
            self.vga.attribute_flip_flop = false;
        }
        if let Some(reg) = self.verite_port(port) {
            self.verite_write(reg, value as u32, 1);
            return;
        }
        match port {
            // ISA Plug and Play's ADDRESS and WRITE_DATA ports.
            crate::isapnp::ADDRESS if self.isapnp.is_some() => {
                if let Some(pnp) = &mut self.isapnp {
                    pnp.write_address(value);
                }
            }
            crate::isapnp::WRITE_DATA if self.isapnp.is_some() => self.isapnp_write(value),
            // A booted system's floppy disk controller, ahead of the IDE
            // channel that would have 3F7h.
            p if self.fdc_claims(p) => self.fdc_write(p, value),
            // A booted system's IDE channels.
            p if self.ide_claims(p) => self.ide_write(p, value),
            p if self.ne2000_claims(p) => self.ne2000_write(p, value),
            p if self.serial_claims(p) => self.serial_write(p, value),
            // The ET4000's KEY, segment select, extended sequencer
            // registers and DAC command register.
            p @ (0x3BF | 0x3B8 | 0x3D8 | 0x3CD | 0x3C5..=0x3C9) if self.et4000() && self.et4000_write_port(p, value) => {}
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
            0x71 => {
                self.cmos.write_data(value);
                self.schedule_rtc();
            }

            // System control port A: bit 1 is the "fast A20" gate, bit 0
            // resets the CPU.
            0x92 => {
                self.set_a20(value & 0x02 != 0);
                if value & 0x01 != 0 {
                    self.reset_requested = true;
                }
            }

            // The chipset's reset control register: bit 2 resets the CPU
            // (bit 1 makes it a hard reset).
            0xCF9 => {
                if value & 0x04 != 0 {
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
                let now = self.clock.now_ticks();
                match self.pit2.access {
                    1 => {
                        self.pit_divisor = value as u16;
                        self.pit2.write_count(self.pit_divisor, now);
                    }
                    2 => {
                        self.pit_divisor = (value as u16) << 8;
                        self.pit2.write_count(self.pit_divisor, now);
                    }
                    _ if !self.pit_write_msb => {
                        // Write LSB
                        self.pit_divisor = (self.pit_divisor & 0xFF00) | (value as u16);
                        self.pit_write_msb = true; // Next write will be MSB
                    }
                    _ => {
                        // Write MSB
                        self.pit_divisor = (self.pit_divisor & 0x00FF) | ((value as u16) << 8);
                        self.pit_write_msb = false; // Reset to LSB
                        self.pit2.write_count(self.pit_divisor, now);
                    }
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
                    } else if channel == 2 && self.pit2.latched.is_none() {
                        self.pit2.latched = Some(self.pit2.read(self.clock.now_ticks()).0);
                        self.pit_read_msb = false;
                    }
                } else {
                    match channel {
                        0 => {
                            // New mode: the counter stops until a count is
                            // written, and LSB/MSB sequencing starts over.
                            self.pit0_write_msb = false;
                            self.pit0_read_msb = false;
                            self.pit0_access = access;
                            self.pit0.set_mode((value >> 1) & 0x07, self.clock.now_ticks());
                            self.clock.schedule(self.next_event());
                        }
                        2 => {
                            // Reset Channel 2 LSB/MSB
                            self.pit_write_msb = false;
                            self.pit_read_msb = false;
                            self.pit2.set_mode((value >> 1) & 0x07, access, self.clock.now_ticks());
                        }
                        _ => {}
                    }
                }
            }

            // PPI Port B (Speaker Control 0x61)
            // Bit 0: Timer 2 Gate (Must be 1 for timer to run)
            // Bit 1: Speaker Data (Must be 1 for sound to pass to speaker)
            0x61 => {
                self.pit2.set_gate(value & 0x01 != 0, self.clock.now_ticks());
                self.speaker_data = value & 0x02 != 0;
                self.ppi_checks = value & 0x0C;
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
            p if self.awe_claims(p) => self.awe_write(p, value as u32, 1),

            // MPU-401 MIDI interface.
            0x330 => {
                self.audio_catch_up();
                self.mpu.write_data(value);
            }
            0x331 => self.mpu.write_command(value),

            // The Tandy's and PCjr's sound chip, where the second DMA
            // controller's channel 4 (the cascade, which nothing
            // programs) has its address and count.
            0xC0 | 0xC1 if self.tandy_sound_enabled() => {
                self.audio_catch_up();
                self.tandy_chip().write(value);
            }

            // The DMA controllers and page registers. The sound cards run
            // up to now first, so the transfers they are in see the change
            // when it happens.
            p if crate::dma::Dma::owns(p) => {
                self.sb_advance();
                self.gus_advance();
                self.dma.write(p, value);
                self.sb_dma_write();
                self.clock.schedule(self.next_event());
            }

            // Dispatch to Devices
            // TODO: Use a proper map lookup
            // Ports we intentionally ignore — writes are harmless but other-
            // wise spam the log. Programs blindly touch these as leftovers
            // from CGA/EGA-era code even when they're really talking to VGA.
            0x3D8 | 0x3D9 if !self.vga.decodes(port) => {
                // CGA Mode Control / Color Select. Real VGA ignores writes
                // here; VGA mode lives at 0x3D4/0x3D5 (handled by the VGA).
            }
            // Game port write: the one-shots fire.
            0x0201 => {
                let now_ns = self.clock.now_ns();
                self.joystick.arm(now_ns);
            }

            // The printer on the parallel port.
            p if self.printer_claims(p) => self.printer_write(p, value),

            // The VFX1's card.
            p if self.vfx1.as_ref().is_some_and(|v| v.claims(p)) => {
                if let Some(vfx1) = &mut self.vfx1 {
                    vfx1.write(p, value);
                }
            }

            // The DAC on the parallel port, heard from now on.
            crate::lpt_dac::DATA if self.lpt_dac.is_some() => {
                self.audio_catch_up();
                if let Some(dac) = &mut self.lpt_dac {
                    dac.write_data(value);
                }
            }
            crate::lpt_dac::CONTROL if self.lpt_dac.is_some() => {
                self.audio_catch_up();
                if let Some(dac) = &mut self.lpt_dac {
                    dac.write_control(value);
                }
            }

            // Super VGA CRTC registers, past the VGA's 00h-18h.
            0x3D5 | 0x3B5 if self.vga.crtc_index >= 0x19 && self.vga.decodes(port) => {
                self.ext_crtc_write(self.vga.crtc_index, value)
            }
            // The S3's sequencer registers, from 08h, and its graphics
            // engine's ports.
            0x3C5 if self.s3() && self.vga.sequencer_index >= 0x08 => {
                self.vga.s3.write_seq(self.vga.sequencer_index, value)
            }
            p if self.s3() && s3::is_engine_port(p) => self.engine_write(p, value as u32, 1),
            0xCFC..=0xCFF if self.pci_present() => self.pci_write(port, value),

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
                        self.note_display_start();
                    }
                    // What the S3 shows depends on the VGA's registers too.
                    if self.s3() && !matches!(port, 0x3C6..=0x3C9 | 0x3DA | 0x3BA) {
                        self.s3_settle();
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
                        (0x3D8 | 0x3DA | 0x3DE, video::adapter::Adapter::Tandy)
                        | (0x3DA, video::adapter::Adapter::Pcjr) => Some(self.vga.tandy_video_mode()),
                        _ => None,
                    };
                    if let Some(mode) = mode_control
                        && mode != self.video_mode
                    {
                        self.log_string(&format!("[VIDEO] Mode Control {:02X}: {:?}", value, mode));
                        self.video_mode = mode;
                        self.vga.mark_dirty_full();
                    }

                    // Check if video mode changed
                    // (A VESA mode is a 256-color mode to these registers.)
                    if let Some(new_mode) = self.vga.check_video_mode().filter(|_| self.video_mode != VideoMode::Vesa)
                        && self.video_mode != new_mode
                        && new_mode == VideoMode::Graphics320x200
                    {
                        self.log_string("[VGA] Switch to Graphics320x200 detected via IO");
                        self.video_mode = new_mode;
                        self.vga.mark_dirty_full();
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
        let _profile = self.profile.as_mut().map(|p| p.port(port, false));
        self.observe.port_read(port);
        // Sample the game port at the beginning of the I/O cycle. Its read
        // stall advances time for the next sample, as one polling interval.
        let now_ns = self.clock.now_ns();
        self.clock.stall(crate::timer::IO_READ_NS);
        let value = self.read_port(port, now_ns);
        self.log_port(port, value as u32, 1, false);
        // Reads acknowledge interrupts of some devices (the Sound Blaster's
        // at 22Eh, the Ultrasound's status).
        self.refresh_irq();
        value
    }

    fn read_port(&mut self, port: u16, now_ns: u64) -> u8 {
        if self.attribute_reset.take() {
            self.vga.attribute_flip_flop = false;
        }
        if let Some(reg) = self.verite_port(port) {
            return self.verite_read(reg, 1) as u8;
        }
        match port {
            // ISA Plug and Play's READ_DATA port, wherever software put it,
            // while a card answers there.
            p if self.isapnp.as_ref().is_some_and(|pnp| pnp.reads(p)) => self.isapnp.as_mut().map_or(0xFF, |pnp| pnp.read_data()),
            p if self.fdc_claims(p) => self.fdc_read(p),
            p if self.ide_claims(p) => self.ide_read(p),
            p if self.ne2000_claims(p) => self.ne2000_read(p),
            p if self.serial_claims(p) => self.serial_read(p),
            p @ (0x3BF | 0x3CD | 0x3C5..=0x3C9) if self.et4000() => match self.et4000_read_port(p) {
                Some(value) => value,
                None if self.vga.ports().contains(&p) => self.vga.io_read(p),
                None => 0xFF,
            },
            // PIC: port 0x20 returns IRR or ISR (selected by OCW3), port
            // 0x21 the interrupt mask. Programs read-modify-write the mask
            // to unmask their IRQ without disturbing the others.
            0x20 | 0x21 | 0xA0 | 0xA1 => self.pic.read(port),

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

            // The game port: the joysticks' axes and buttons.
            0x0201 => self.joystick.read(&self.mouse, now_ns),

            // The printer on the parallel port.
            p if self.printer_claims(p) => self.printer_read(p),

            // The VFX1's card.
            p if self.vfx1.as_ref().is_some_and(|v| v.claims(p)) => {
                self.vfx1.as_ref().map_or(0xFF, |v| v.read(p))
            }

            // The DAC on the parallel port: the Disney's FIFO empties as
            // it plays.
            crate::lpt_dac::DATA | crate::lpt_dac::STATUS | crate::lpt_dac::CONTROL if self.lpt_dac.is_some() => {
                self.audio_catch_up();
                let dac = self.lpt_dac.as_ref().unwrap();
                match port {
                    crate::lpt_dac::DATA => dac.read_data(),
                    crate::lpt_dac::STATUS => dac.read_status(),
                    _ => dac.read_control(),
                }
            }

            // Port 0x42 — PIT channel 2 (PC speaker tone) data. Programs
            // read it as a free-running counter for timing loops, or time a
            // delay loop by gating it through port 61h bit 0.
            0x42 => {
                let count = match self.pit2.latched {
                    Some(latched) => latched,
                    None => self.pit2.read(self.clock.now_ticks()).0,
                };
                match self.pit2.access {
                    1 => {
                        self.pit2.latched = None;
                        (count & 0xFF) as u8
                    }
                    2 => {
                        self.pit2.latched = None;
                        (count >> 8) as u8
                    }
                    _ if !self.pit_read_msb => {
                        self.pit_read_msb = true;
                        (count & 0xFF) as u8
                    }
                    _ => {
                        self.pit_read_msb = false;
                        self.pit2.latched = None;
                        (count >> 8) as u8
                    }
                }
            }

            // Port 0x60 — Keyboard data port. Real hardware latches the
            // last-received scan code here; programs either read this from
            // their INT 09h ISR after IRQ1 fires, or poll it directly.
            0x60 => {
                let value = self.kbc.read_data();
                // The next queued byte, if any, moved into the buffer, or
                // the keyboard's comes a little later.
                self.sync_keyboard_irq();
                value
            }

            // Port 0x64 — Keyboard controller status (8042).
            0x64 => self.kbc.read_status(),

            0x71 => self.cmos.read_register(),
            0x92 => (self.a20() as u8) << 1,

            // The FM chip's status register: timer flags, which programs
            // poll to detect an AdLib.
            0x388 | 0x38A => self.opl.read_status(self.clock.now_micros()),
            0x389 | 0x38B => 0xFF,

            p if self.sb_base().is_some_and(|b| p & 0xFFF0 == b) => self.sb_read(p & 0xF),

            p if self.gus_claims(p) => self.gus_read(p),
            p if self.awe_claims(p) => self.awe_read(p, 1) as u8,

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

            // Read PPI Port B: speaker gate and data and the check enables
            // as written, the DRAM refresh request (bit 4, toggles), and the
            // PIT channel 2 output (bit 5).
            0x61 => {
                let mut val = self.ppi_checks;
                if self.pit2.gate() {
                    val |= 0x01;
                }
                if self.speaker_data {
                    val |= 0x02;
                }
                self.refresh_toggle = !self.refresh_toggle;
                if self.refresh_toggle {
                    val |= 0x10;
                }
                if self.pit2.read(self.clock.now_ticks()).1 {
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
            0x3C5 if self.s3() && self.vga.sequencer_index >= 0x08 => self.vga.s3.read_seq(self.vga.sequencer_index),
            p if self.s3() && s3::is_engine_port(p) => self.engine_read(p, 1) as u8,
            0xCFC..=0xCFF if self.pci_present() => self.pci_read(port),

            _ => {
                if self.vga.ports().contains(&port) {
                    self.vga.io_read(port)
                } else {
                    0xFF // Default open bus
                }
            }
        }
    }

    /// The video timing of the picture the monitor shows: the 3dfx card's
    /// while it drives the monitor, the display's otherwise.
    pub fn refresh_timing(&mut self) -> crate::video::crt::CrtTiming {
        match &self.voodoo {
            Some(v) if v.output() => v.timing(),
            _ => self.vga.timing(),
        }
    }

    /// Latch the display Start Address if emulated time has passed the
    /// start of a vertical retrace since it was last latched, as the CRTC
    /// does at every retrace.
    pub fn sync_display(&mut self) {
        // The 3dfx card's picture changed, or it took or gave back the
        // monitor: draw the screen anew.
        if let Some(v) = &mut self.voodoo
            && v.prepare_display()
        {
            self.vga.mark_dirty_full();
        }
        let now = self.clock.now_ns();
        if self.vga.retrace_began(now) {
            if self.virge() {
                self.virge.set_status(crate::video::s3::virge::STAT_VSY);
                // The overlay's picture and a full streams mode's primary
                // stream are away from the rows writes mark: draw anew.
                let streams = &self.virge.streams;
                if crate::video::s3::streams::Streams::full(self.vga.s3.crtc(0x67)) || streams.overlay(true).is_some() {
                    self.vga.mark_dirty_full();
                }
            }
            let screen = crate::autospeed::screen_bytes(self);
            self.activity.set_screen(screen);
            self.settle_register_mode();
            self.vga.latch_start_address();
            if self.vbe.latched_start != self.vbe.start {
                self.vbe.latched_start = self.vbe.start;
                self.vga.mark_dirty_full();
                self.vga.flipped = true;
            }
            if self.vga.adapter.gate_array() {
                self.compare_gate_array();
            }
            self.count_frame();
        }
    }

    /// After a switch between text and graphics made through the attribute
    /// controller, or between 256 colours and 16 through the Graphics Mode
    /// register, as Windows' VDD makes one restoring a machine's display or
    /// passing on a video BIOS service's writes: show the mode the
    /// registers describe now that the writes are done.
    pub fn settle_register_mode(&mut self) {
        if !std::mem::take(&mut self.vga.mode_switched) || self.video_mode == VideoMode::Vesa {
            return;
        }
        let colours256 = |mode: VideoMode| mode == VideoMode::Graphics320x200;
        if let Some(mode) = self.vga.register_mode()
            && (mode.is_text() != self.video_mode.is_text() || colours256(mode) != colours256(self.video_mode))
        {
            self.log_string(&format!("[VGA] Switch to {:?} through the registers", mode));
            self.video_mode = mode;
            self.vga.mark_dirty_full();
        }
    }

    /// Repaint what changed in the Tandy's or PCjr's picture memory since
    /// the last retrace, however it was written.
    fn compare_gate_array(&mut self) {
        let (base, size) = self.vga.crt_range();
        let Some(now) = self.ram.get(base..base + size) else { return };
        if self.gate_array_shadow_at != base || self.gate_array_shadow.len() != size {
            self.gate_array_shadow.clear();
            self.gate_array_shadow.extend_from_slice(now);
            self.gate_array_shadow_at = base;
            self.vga.mark_dirty_full();
            return;
        }
        let Some(first) = now.iter().zip(&self.gate_array_shadow).position(|(a, b)| a != b) else { return };
        let last = now.iter().zip(&self.gate_array_shadow).rposition(|(a, b)| a != b).unwrap_or(first);
        self.gate_array_shadow.copy_from_slice(now);
        self.mark_text_dirty(first, last + 1);
    }

    /// The memory the text and CGA modes show from: the card's, or on a
    /// Tandy or PCjr the pages of system memory the picture comes from.
    pub fn display_mem(&self) -> &[u8] {
        if self.vga.adapter.gate_array() {
            let (base, size) = self.vga.crt_range();
            return &self.ram[base..base + size];
        }
        &self.vga.vram_text
    }

    /// The text and CGA modes' memory as programs see it at B8000h, from
    /// its start: the card's, or on a Tandy or PCjr the pages of system
    /// memory the window shows.
    pub fn text_mem(&self) -> &[u8] {
        if self.vga.adapter.gate_array() {
            let (base, size) = self.vga.cpu_range();
            return &self.ram[base..base + size];
        }
        &self.vga.vram_text
    }

    /// `text_mem` to write. The caller marks what it changes for
    /// repainting.
    pub fn text_mem_mut(&mut self) -> &mut [u8] {
        if self.vga.adapter.gate_array() {
            let (base, size) = self.vga.cpu_range();
            self.bump_page_gens(base, base + size);
            return &mut self.ram[base..base + size];
        }
        &mut self.vga.vram_text
    }

    /// A retrace began: count a frame if the program drew one. A program
    /// that flips pages draws the next frame out of sight, so while it
    /// flips (a flip within the last second or so) only the flips count.
    /// The 3dfx card only ever flips.
    fn count_frame(&mut self) {
        self.retraces += 1;
        let flipped = std::mem::take(&mut self.vga.flipped);
        let drawn = std::mem::take(&mut self.vga.drawn);
        // While the 3dfx card shows, its buffer swaps are the frames.
        if let Some(v) = &mut self.voodoo {
            let swapped = std::mem::take(&mut v.swapped);
            if v.output() {
                self.frames_drawn += swapped;
                self.activity.swapped(swapped);
                return;
            }
        }
        if flipped {
            self.last_flip = Some(self.retraces);
        }
        let flipping = self.last_flip.is_some_and(|at| self.retraces - at < 70);
        if flipped || (drawn && !flipping) {
            self.frames_drawn += 1;
        }
    }

    /// Write a Super VGA CRTC register (index 19h and up). Like an S3
    /// card, 6Ah is the bank of the window at A0000h and 69h the high
    /// bits of the display start; the VBE protected-mode interface uses
    /// them.
    fn ext_crtc_write(&mut self, index: u8, value: u8) {
        if self.s3() {
            return self.s3_crtc_write(index, value);
        }
        if self.et4000() {
            return self.et4000_crtc_write(index, value);
        }
        match index {
            0x69 => {
                self.vbe.start_high = value;
                self.update_vbe_start();
            }
            0x6A => self.vbe.bank = (value & 0x3F) as u32,
            _ => {}
        }
    }

    fn ext_crtc_read(&mut self, index: u8) -> u8 {
        if self.s3() {
            return self.s3_crtc_read(index);
        }
        if self.et4000() {
            return self.et4000_crtc_read(index);
        }
        match index {
            0x69 => self.vbe.start_high,
            0x6A => self.vbe.bank as u8,
            _ => 0,
        }
    }

    /// In a VESA mode the Start Address registers and CRTC 69h give the
    /// display start in doublewords.
    fn update_vbe_start(&mut self) {
        // A Vérité's own CRTC has its start in its own register.
        if self.video_mode == VideoMode::Vesa && !self.verite.native {
            let crtc = &self.vga.crtc_regs;
            let dwords = (self.vbe.start_high as u32) << 16 | (crtc[0x0C] as u32) << 8 | crtc[0x0D] as u32;
            self.vbe.start = dwords * 4;
        }
    }

    /// The size of the screen in the mode's own pixels: the VESA mode's
    /// size, or what the standard mode has.
    pub fn display_size(&self) -> (usize, usize) {
        if let Some(v) = &self.voodoo
            && v.output()
        {
            let (w, h) = v.size();
            return (w as usize, h as usize);
        }
        match (self.video_mode, self.vbe.mode) {
            (VideoMode::Vesa, Some(mode)) => (mode.width as usize, mode.height as usize),
            // An ET4000's modes are the VGA's at the size its registers
            // give them.
            (
                VideoMode::Graphics320x200
                | VideoMode::Ega320x200
                | VideoMode::Ega640x200
                | VideoMode::Ega640x350
                | VideoMode::Vga640x480,
                _,
            ) if self.et4000() => self.vga.graphics_size(),
            (mode, _) => mode.dimensions(),
        }
    }

    /// Input Status 1 (port 3DAh): bit 3 in the vertical retrace, bit 0
    /// while display enable is off, from the CRT timing and emulated time.
    /// Reading it also resets the attribute controller's flip-flop.
    /// For `idle`: until which instruction count the bits `mask` of port
    /// `port` read as at instruction count `from`, if reading it changes
    /// nothing a program could see the next time but those bits. Only the
    /// VGA's Input Status 1, whose bits follow the time.
    pub fn idle_port(&mut self, port: u16, mask: u8, from: u64) -> Option<u64> {
        let vga = matches!(
            self.vga.adapter,
            video::adapter::Adapter::Svga
                | video::adapter::Adapter::Vga
                | video::adapter::Adapter::Ega
                | video::adapter::Adapter::S3
                | video::adapter::Adapter::S3Virge
                | video::adapter::Adapter::S3VirgeVx
                | video::adapter::Adapter::Et4000
        );
        if !(vga && matches!(port, 0x3DA | 0x3BA) && self.vga.decodes(port)) {
            return None;
        }
        let timing = self.vga.timing();
        let until = timing.next_status_change(self.clock.ns_at(from), mask);
        Some(self.clock.icount_at_ns(until))
    }

    /// For `idle`: what an IN of port `port` (one `idle_port` knows) that
    /// starts at instruction count `at` reads, without reading it.
    pub fn idle_port_value(&mut self, port: u16, at: u64) -> u8 {
        debug_assert!(matches!(port, 0x3DA | 0x3BA));
        let sample = at + self.clock.stall_count(crate::timer::IO_READ_NS);
        self.vga.timing().status(self.clock.ns_at(sample))
    }

    fn input_status_1(&mut self) -> u8 {
        self.activity.status_reads += 1;
        self.sync_display();
        self.vga.attribute_flip_flop = false;
        self.vga.gate_array_status_read();
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
