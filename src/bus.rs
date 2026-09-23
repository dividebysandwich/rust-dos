use sdl2::audio::AudioQueue;
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::time::Instant;

use crate::disk::{DiskController, DriveKind, LASTDRIVE, MountOptions};
use crate::video::{self, ADDR_VGA_GRAPHICS, ADDR_VGA_TEXT, SIZE_GRAPHICS, SIZE_TEXT, VideoMode};

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

pub struct Bus {
    ram: Vec<u8>, // System RAM, allocated once
    pub video_mode: VideoMode, // Current State
    pub disk: DiskController,
    pub keyboard_buffer: VecDeque<u16>, // Stores (Scancode << 8) | ASCII
    /// The 8042 keyboard controller (ports 60h/64h): scan codes for
    /// programs that read the keyboard themselves, the A20 gate and the
    /// CPU reset line.
    pub kbc: crate::kbc::Kbc,
    /// The A20 gate: when false, address line 20 is forced to 0 and
    /// addresses wrap at 1 MB as on an 8086.
    pub a20: bool,
    /// Something asked for a CPU reset (8042 or port 92h); the execution
    /// loop carries it out.
    pub reset_requested: bool,
    pub cmos: crate::cmos::Cmos,
    /// The XMS driver's allocations and A20 state.
    pub xms: crate::xms::Xms,
    /// Last POST code written to port 80h (or 190h, test ROMs).
    pub post_code: u8,
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
    pub audio_device: Option<AudioQueue<i16>>,
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
    pub log_file: Option<BufWriter<File>>,

    // VGA State
    pub vga: crate::video::vga::VgaCard,
    pub search_handles: std::collections::HashMap<u32, String>,

    // Mouse State (INT 33h)
    pub mouse: crate::mouse::MouseState,

    // AdLib / OPL2 FM synthesizer (ports 0x388/0x389)
    pub adlib: crate::adlib::AdLib,

    // Sound Blaster 2.0 (ports 0x220..0x22F) + 8237 DMA channel 1.
    // Kept side-by-side with the bus so the audio pump can pull PCM
    // bytes straight out of ram using the DMA channel's address/page.
    pub sb: crate::sb::SoundBlaster,
    pub dma_ch1: crate::sb::Dma8237Ch1,

    /// Per-4KB-page generation counter for every page of RAM. Bumped on every
    /// write inside the Bus write helpers. The decoded-instruction cache
    /// stores the gen at decode time and invalidates a cached entry when the
    /// gen for its page has changed — this is how we stay correct in the
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
            a20: false,
            reset_requested: false,
            cmos: crate::cmos::Cmos::new(((ram_len >> 10) - 1024) as u32),
            post_code: 0,
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
            search_handles: std::collections::HashMap::new(),
            mouse: crate::mouse::MouseState::new(),
            adlib: crate::adlib::AdLib::new(),
            sb: crate::sb::SoundBlaster::new(),
            dma_ch1: crate::sb::Dma8237Ch1::default(),
            page_gen: vec![0; ram_len >> 12],
            log_hook: None,
            audio_hook: None,
        };
        // BIOS Data Area (BDA) Initialization
        // 0x0449: Current Video Mode (03 = 80x25 Color)
        bus.write_8(0x0449, 0x03);
        // 0x044A: Number of Columns (80 = 0x50)
        bus.write_16(0x044A, 80);
        // 0x044E: Video Page Size (4096 bytes approx, usually 0x1000)
        bus.write_16(0x044E, 0x1000);
        // 0x0460: Cursor Shape (Start Line 13, End Line 14 for VGA)
        bus.write_16(0x0460, 0x0D0E);
        // 0x0462: Active Page (0)
        bus.write_8(0x0462, 0);
        // 0x0463: CRT Controller Base Address (0x3D4 for Color)
        bus.write_16(0x0463, 0x03D4);

        // 0x0410: Equipment List. Bits 4-5 = 10 (80x25 Color)
        // Bit 0 = Floppy. 0x21 (Floppy + Color)
        bus.write_16(0x0410, 0x0021);

        // 0x0484: Rows on Screen (minus 1). 24 = 25-row default.
        bus.write_8(0x0484, 24);
        // 0x0485: Character height in scan lines. 16 = VGA 8x16 default.
        bus.write_16(0x0485, 16);

        // 0x0487: EGA/VGA Info. Bits 5-6 = 11 (256KB Video RAM).
        // 0x60 = 01100000
        bus.write_8(0x0487, 0x60);

        // 0x0488: VGA Feature Switches & Misc (bits 3-0 = EGA config switches)
        // 0x09 is a common VGA config (1001b).
        bus.write_8(0x0488, 0x09);

        // 0x0489: VGA Misc Flags
        //   Bit 0 = cursor emulation enabled (standard on VGA)
        //   Bits 6-5 = 01 (400 scan-line mode)
        // 0x21 = 00100001
        bus.write_8(0x0489, 0x21);

        // 0x048A: DCC (Display Combination Code)
        // 0x08 = VGA w/ Color
        bus.write_8(0x048A, 0x08);

        // 0x0496: Keyboard State (0 = Standard)ture at C000:0000
        bus.ram[0xC0000] = 0x55;
        bus.ram[0xC0001] = 0xAA;
        bus.ram[0xC0002] = 0x40; // 32KB (64 * 512 bytes)
        // bus.write_string(0xC001E, "IBM VGA");
        // write "IBM VGA" to C000:001E
        let signature = b"IBM VGA";
        for (i, &byte) in signature.iter().enumerate() {
            bus.ram[0xC001E + i] = byte;
        }

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

        // Initialize 8x16 Font at C000:2000 (Address 0xC2000)
        // Just fill with a visible pattern so checks pass (non-zero)
        for i in 0..(256 * 16) {
            bus.ram[0xC2000 + i] = (i % 256) as u8;
        }

        // BIOS ROM code and the interrupt vector table.
        crate::bios::install(&mut bus);
        crate::mouse::install_callback_stub(&mut bus);

        // Build a baseline MCB chain — one large free block covering
        // conventional memory. load_shell / load_exe rebuild as needed, but we
        // still want mcb::alloc to work for tests and any early allocation.
        crate::mcb::init_empty(&mut bus);

        // Equipment word, hard disk count and DPBs reflect the drives C:/Z:.
        bus.sync_drive_bda();

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
        let result = self.disk.mount(drive, path, opts, replace);
        self.sync_drive_bda();
        result
    }

    /// Unmount a DOS drive and refresh the BIOS view of the drive set.
    pub fn unmount_drive(&mut self, drive: u8) -> Result<(), String> {
        let result = self.disk.unmount(drive);
        self.sync_drive_bda();
        result
    }

    /// Mirror the mounted drives into the BIOS data area and the ROM tables
    /// DOS hands out pointers to. Must run whenever the drive set changes.
    pub fn sync_drive_bda(&mut self) {
        // Equipment word: bit 0 = floppy present, bits 6-7 = floppy count - 1.
        // Only A: and B: are BIOS floppy units. Other bits are left alone.
        let floppies = (0..2)
            .filter(|&d| self.disk.drive_kind(d) == Some(DriveKind::Floppy))
            .count() as u16;
        let mut equipment = self.read_16(0x0410) & !0x00C1;
        if floppies > 0 {
            equipment |= 0x0001 | ((floppies - 1) << 6);
        }
        self.write_16(0x0410, equipment);

        // 0x0475: number of fixed disks (INT 13h units 80h+).
        let hard_disks = self.disk.drives_of_kind(DriveKind::HardDisk).len();
        self.write_8(0x0475, hard_disks.min(0xFF) as u8);

        // CD-ROMs are redirector drives and have no DPB.
        let with_dpb: Vec<(u8, DriveKind)> = (0..LASTDRIVE)
            .filter_map(|d| self.disk.drive_kind(d).map(|k| (d, k)))
            .filter(|&(_, k)| k != DriveKind::CdRom)
            .collect();
        for drive in 0..LASTDRIVE {
            let media = self.disk.drive_kind(drive).map_or(0, |k| k.media_descriptor());
            self.write_8(MEDIA_ID_TABLE + drive as usize, media);
            let base = DPB_TABLE + drive as usize * DPB_SIZE;
            for i in 0..DPB_SIZE {
                self.write_8(base + i, 0);
            }
        }
        for (i, &(drive, kind)) in with_dpb.iter().enumerate() {
            let next = with_dpb.get(i + 1).map(|&(d, _)| d);
            self.write_dpb(drive, kind, next);
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
        let max_sector = with_dpb.iter().map(|&(_, k)| k.geometry().1).max();
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
        for (i, &b) in b"NUL     ".iter().enumerate() {
            self.write_8(nul + 0x0A + i, b);
        }
        self.write_8(base + 0x43, 3); // 43: boot drive C:
        self.write_8(base + 0x50, 0xCB); // RETF for the NUL driver entries, past the table
    }

    /// Fill in a DOS 4+ style Drive Parameter Block with a plausible FAT
    /// layout for the drive's reported geometry.
    fn write_dpb(&mut self, drive: u8, kind: DriveKind, next: Option<u8>) {
        let (spc, bps, total) = kind.geometry();
        let base = DPB_TABLE + drive as usize * DPB_SIZE;
        let (root_entries, sectors_per_fat): (u16, u16) = match kind {
            DriveKind::Floppy => (224, 9),
            _ => (512, ((total as u32 + 2) * 2).div_ceil(bps as u32) as u16),
        };
        let reserved: u16 = 1;
        let fat_count: u16 = 2;
        let first_dir_sector = reserved + fat_count * sectors_per_fat;
        let root_sectors = (root_entries as u32 * 32).div_ceil(bps as u32) as u16;

        self.write_8(base, drive); // 00: drive number (0=A)
        self.write_8(base + 0x01, drive); // 01: unit within driver
        self.write_16(base + 0x02, bps); // 02: bytes per sector
        self.write_8(base + 0x04, (spc - 1) as u8); // 04: sectors per cluster - 1
        self.write_8(base + 0x05, spc.trailing_zeros() as u8); // 05: cluster shift
        self.write_16(base + 0x06, reserved); // 06: reserved sectors
        self.write_8(base + 0x08, fat_count as u8); // 08: number of FATs
        self.write_16(base + 0x09, root_entries); // 09: root directory entries
        self.write_16(base + 0x0B, first_dir_sector + root_sectors); // 0B: first data sector
        self.write_16(base + 0x0D, total.saturating_add(1)); // 0D: highest cluster
        self.write_16(base + 0x0F, sectors_per_fat); // 0F: sectors per FAT
        self.write_16(base + 0x11, first_dir_sector); // 11: first directory sector
        self.write_8(base + 0x17, kind.media_descriptor()); // 17: media ID
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
        if end <= start {
            return;
        }
        let (row_bytes, cell_h) = match self.video_mode {
            VideoMode::Text80x25 | VideoMode::Text80x25Color => {
                let cell_h = self.read_8(0x0485) as usize;
                (160, if cell_h == 0 { 16 } else { cell_h })
            }
            // 8x8 font scaled 2x, irrespective of the BDA value
            VideoMode::Text40x25 | VideoMode::Text40x25Color => (80, 16),
            _ => {
                self.vga.mark_dirty_full();
                return;
            }
        };
        let h = video::SCREEN_HEIGHT;
        let y0 = ((start / row_bytes) * cell_h) as u32;
        let y1 = ((end - 1) / row_bytes + 1) as u32 * cell_h as u32;
        self.vga.mark_dirty_rows(y0.min(h), y1.min(h));
    }

    // Helper: Scroll the text screen up by 1 line
    pub fn scroll_up(&mut self) {
        // Read the current row count from BDA so 80x43 / 80x50 modes scroll
        // their whole visible area, not just the first 25 rows.
        let rows = self.read_8(0x0484) as usize + 1;
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
        for page in (start >> 12)..=((end - 1) >> 12) {
            let g = &mut self.page_gen[page];
            *g = g.wrapping_add(1);
        }
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
            // Route through VGA so chain-4, odd/even, and Read Map Select
            // work correctly. read_graphics also latches planes, needed
            // for planar read-modify-write sequences.
            return self.vga.read_graphics(addr - ADDR_VGA_GRAPHICS);
        }
        if addr >= ADDR_VGA_TEXT && addr < ADDR_VGA_TEXT + SIZE_TEXT {
            return self.vga.vram_text[addr - ADDR_VGA_TEXT];
        }
        if addr < self.ram.len() {
            return self.ram[addr];
        }
        if addr >= 0xFFFF_0000 {
            // The top 64 KB of the address space mirror the BIOS ROM, where
            // a 386 fetches its first instruction after reset.
            return self.ram[0xF0000 + (addr & 0xFFFF)];
        }
        0xFF // nothing there: open bus
    }

    /// Side-effect-free byte read for debuggers. Same mapping as `read_8`,
    /// but VGA plane latches are restored afterwards so inspecting video
    /// memory can't disturb a program's read-modify-write sequences.
    pub fn peek_8(&self, addr: usize) -> u8 {
        if addr >= self.ram.len() {
            return 0xFF;
        }
        if (ADDR_VGA_GRAPHICS..ADDR_VGA_GRAPHICS + SIZE_GRAPHICS).contains(&addr) {
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
                let g = self.page_gen.get_unchecked_mut(addr >> 12);
                *g = g.wrapping_add(1);
            }
            return false;
        }
        if addr >= ADDR_VGA_GRAPHICS && addr < ADDR_VGA_GRAPHICS + SIZE_GRAPHICS {
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
        if addr >= ADDR_VGA_TEXT && addr < ADDR_VGA_TEXT + SIZE_TEXT {
            let text_off = addr - ADDR_VGA_TEXT;
            self.vga.vram_text[text_off] = value;

            // Narrow the dirty range to just the affected character row when
            // we're in a text mode. Cell height comes from BDA 0x485 (set by
            // INT 10h font swaps); 80x50 mode loads the 8-pixel font and
            // reduces this to 8. CGA graphics modes (4/5/6) also live in
            // this VRAM but their byte-to-scanline mapping is interleaved,
            // so we conservatively repaint everything for those.
            match self.video_mode {
                VideoMode::Text80x25 | VideoMode::Text80x25Color => {
                    let cell_h = self.read_8(0x0485) as usize;
                    let cell_h = if cell_h == 0 { 16 } else { cell_h };
                    let row = text_off / 160;
                    let y0 = (row * cell_h) as u32;
                    let y1 = ((row + 1) * cell_h) as u32;
                    let h = video::SCREEN_HEIGHT;
                    self.vga.mark_dirty_rows(y0.min(h), y1.min(h));
                }
                VideoMode::Text40x25 | VideoMode::Text40x25Color => {
                    // 40-col modes use the 8x8 font scaled 2x = 16 screen
                    // rows per text row, irrespective of the BDA value.
                    let row = text_off / 80;
                    let y0 = (row * 16) as u32;
                    let y1 = ((row + 1) * 16) as u32;
                    let h = video::SCREEN_HEIGHT;
                    self.vga.mark_dirty_rows(y0.min(h), y1.min(h));
                }
                _ => self.vga.mark_dirty_full(),
            }

            // Check if current mode uses this memory
            return matches!(
                self.video_mode,
                VideoMode::Text80x25
                    | VideoMode::Text80x25Color
                    | VideoMode::Text40x25
                    | VideoMode::Text40x25Color
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
            let page = addr >> 12;
            self.page_gen[page] = self.page_gen[page].wrapping_add(1);
        }
        false
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
                let g = self.page_gen.get_unchecked_mut(addr >> 12);
                *g = g.wrapping_add(1);
                let g = self.page_gen.get_unchecked_mut((addr + 1) >> 12);
                *g = g.wrapping_add(1);
            }
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
        let low = self.read_16(addr) as u32;
        let high = self.read_16(addr + 2) as u32;
        (high << 16) | low
    }

    #[inline(always)]
    pub fn write_32(&mut self, addr: usize, value: u32) {
        if self.is_plain_ram(addr, 4) {
            self.ram[addr..addr + 4].copy_from_slice(&value.to_le_bytes());
            self.page_gen[addr >> 12] = self.page_gen[addr >> 12].wrapping_add(1);
            let last = (addr + 3) >> 12;
            self.page_gen[last] = self.page_gen[last].wrapping_add(1);
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
        self.clock.schedule(self.pit0.next_event());
    }

    /// Change the emulated CPU speed (instructions per emulated ms).
    pub fn set_cycles_per_ms(&mut self, cycles_per_ms: u32) {
        self.clock.set_cycles_per_ms(cycles_per_ms);
        self.clock.schedule(self.pit0.next_event());
    }

    /// Bring the PIT up to the current instruction and request IRQ 0 if it
    /// fired. The main loop calls this when `clock.icount` reaches
    /// `clock.deadline`.
    pub fn service_timers(&mut self) {
        if self.pit0.advance(self.clock.now_ticks()) {
            self.pic.raise(0);
        }
        self.clock.schedule(self.pit0.next_event());
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
        self.clock.schedule(self.pit0.next_event());
    }

    /// Level-triggered request lines (bit n = IRQ n): the Sound Blaster
    /// holds its line until the driver acknowledges at port 22Eh.
    #[inline(always)]
    fn irq_levels(&self) -> u16 {
        (self.sb.irq_pending as u16) << 5
    }

    /// The IRQ (0-15) the PICs would deliver now: requested, not masked,
    /// and not blocked by an interrupt of equal or higher priority still in
    /// service.
    #[inline(always)]
    pub fn pic_pending_irq(&self) -> Option<u8> {
        self.pic.pending(self.irq_levels())
    }

    /// Whether any device requests an interrupt or a mouse event handler
    /// call waits: the cheap test the CPU makes before each instruction,
    /// ahead of the PIC's priority logic.
    #[inline(always)]
    pub fn interrupt_requested(&self) -> bool {
        (self.pic.master.irr | self.pic.slave.irr) != 0
            || self.irq_levels() != 0
            || self.mouse.pending_callback_events & self.mouse.callback_mask != 0
    }

    /// The CPU takes interrupt `irq`: it is in service until an EOI.
    /// Returns its vector.
    pub fn pic_acknowledge(&mut self, irq: u8) -> u8 {
        self.pic.acknowledge(irq)
    }

    /// Discard a request for `irq`, for lines with no handler installed.
    pub fn pic_drop(&mut self, irq: u8) {
        self.pic.lower(irq);
        if irq == 5 {
            self.sb.irq_pending = false;
        }
    }

    /// Raise IRQ 1 if a byte just entered the keyboard controller's output
    /// buffer.
    pub fn sync_keyboard_irq(&mut self) {
        if self.kbc.take_irq() {
            self.pic.raise(1);
        }
    }

    /// Carry out what a keyboard controller or port 92h write asked for.
    fn apply_kbc_effects(&mut self, effects: crate::kbc::Effects) {
        if let Some(a20) = effects.a20 {
            self.a20 = a20;
        }
        if effects.reset {
            self.reset_requested = true;
        }
        self.sync_keyboard_irq();
    }

    // Write to an I/O Port
    pub fn io_write(&mut self, port: u16, value: u8) {
        self.clock.stall(crate::timer::IO_WRITE_NS);
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
                self.a20 = value & 0x02 != 0;
                if value & 0x01 != 0 {
                    self.reset_requested = true;
                }
            }

            // POST code ports (80h on a PC, 190h for test ROMs).
            0x80 | 0x190 => self.post_code = value,
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
                    self.clock.schedule(self.pit0.next_event());
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
                            self.clock.schedule(self.pit0.next_event());
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
                self.speaker_on = enabled;
            }

            // AdLib / OPL2 (YM3812). Port 0x388 selects the register,
            // 0x389 writes data into the previously selected register.
            // Sound Blaster also exposes OPL2 at 0x228/0x229 (mono FM)
            // and mirrors it at 0x220/0x221 (SB 1.x legacy).
            0x388 | 0x228 | 0x220 => {
                self.adlib.write_register_select(value);
            }
            0x389 | 0x229 | 0x221 => {
                self.adlib
                    .write_register_data(value, self.clock.now_micros());
            }

            // --- Sound Blaster DSP (base 0x220) ---
            // 0x226 Reset: write 1 then 0 triggers DSP ready (0xAA).
            0x226 => { self.sb.write_reset(value); }
            // 0x22C Write Command/Data. Buffer-status reads from same port.
            0x22C => { self.sb.write_command(value); }
            // 0x224/0x225 Mixer (SB Pro). We accept writes but stay SB 2.0
            // identified at DSP level — some drivers probe the mixer
            // before checking the DSP version.
            0x224 => { self.sb.mixer_index_write(value); }
            0x225 => { self.sb.mixer_data_write(value); }

            // --- 8237 DMA controller — only channel 1 matters for SB 8-bit. ---
            0x02 => { self.dma_ch1.write_addr(value); }
            0x03 => { self.dma_ch1.write_count(value); }
            0x0A => { self.dma_ch1.write_single_mask(value); }
            0x0B => { self.dma_ch1.write_mode(value); }
            0x0C => { self.dma_ch1.clear_flipflop(); }
            0x0D => { self.dma_ch1.master_reset(); }
            // Other channels (0, 2, 3) — writes are harmless and we
            // don't model them. Swallow so the unhandled-port log stays
            // quiet when games initialize the full controller.
            0x00 | 0x01 | 0x04..=0x09 | 0x0E | 0x0F => {}
            // DMA page registers. Channel 1 lives at port 0x83.
            0x83 => { self.dma_ch1.write_page(value); }
            0x81 | 0x82 | 0x84..=0x8F => {}

            // Dispatch to Devices
            // TODO: Use a proper map lookup
            // Ports we intentionally ignore — writes are harmless but other-
            // wise spam the log. Programs blindly touch these as leftovers
            // from CGA/EGA-era code even when they're really talking to VGA.
            0x3D8 | 0x3D9 => {
                // CGA Mode Control / Color Select. Real VGA ignores writes
                // here; VGA mode lives at 0x3D4/0x3D5 (handled by the VGA).
            }
            0x0201 => {
                // Game port write: arm the one-shot timers. Reset the
                // read counter so the next read-loop starts fresh.
                self.joystick_read_count = 0;
            }

            _ => {
                if self.vga.ports().contains(&port) {
                    self.vga.io_write(port, value);
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

                    // Check if video mode changed
                    if let Some(new_mode) = self.vga.check_video_mode() {
                        if self.video_mode != new_mode && new_mode == VideoMode::Graphics320x200 {
                            self.log_string("[VGA] Switch to Graphics320x200 detected via IO");
                            self.video_mode = new_mode;
                            self.vga.mark_dirty_full();
                        }
                    }
                } else {
                    // Unhandled port write
                    self.log_string(&format!(
                        "[Unhandled IO Write] Port: {:04X}, Value: {:02X}",
                        port, value
                    ));
                }
            }
        }
    }

    // Read from an I/O Port
    pub fn io_read(&mut self, port: u16) -> u8 {
        self.clock.stall(crate::timer::IO_READ_NS);
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
            0x92 => (self.a20 as u8) << 1,

            // AdLib status register (port 0x388). Bit 7 = IRQ, bit 6 = timer1
            // expired, bit 5 = timer2 expired. Games poll this to detect the
            // card by arming timer1 and checking that the bits flip in time.
            // Mirrored onto the SB's FM ports for AdLib-on-SB detection.
            0x388 | 0x228 | 0x220 => self.adlib.read_status(self.clock.now_micros()),
            0x389 | 0x229 | 0x221 => 0xFF,

            // --- Sound Blaster DSP reads ---
            // 0x22A: Read Data — drains the DSP response FIFO.
            0x22A => self.sb.read_data(),
            // 0x22C: Write-buffer status — bit 7 set = DSP busy. Always ready.
            0x22C => self.sb.read_write_status(),
            // 0x22E: Read-buffer status + IRQ acknowledge.
            0x22E => self.sb.read_buffer_status(),
            // Mixer data read-back at 0x225.
            0x225 => self.sb.mixer_data_read(),

            // --- 8237 DMA controller reads (channel 1) ---
            0x02 => self.dma_ch1.read_addr(),
            0x03 => self.dma_ch1.read_count(),
            0x83 => self.dma_ch1.read_page(),
            // Other DMA regs return open bus; keep quiet.
            0x00 | 0x01 | 0x04..=0x0F | 0x80..=0x82 | 0x84..=0x8F => 0xFF,

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

            _ => {
                if self.vga.ports().contains(&port) {
                    self.vga.io_read(port)
                } else {
                    0xFF // Default open bus
                }
            }
        }
    }

    pub fn log_string(&mut self, s: &str) {
        if self.log_file.is_none() {
            let file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open("trace.log")
                .expect("Failed to open trace.log");
            self.log_file = Some(BufWriter::new(file));
        }

        println!("{}", s);
        if let Some(writer) = &mut self.log_file {
            let _ = writeln!(writer, "{}", s);
        }
        if let Some(hook) = &mut self.log_hook {
            hook(s);
        }
    }

    /// Write buffered log lines to trace.log now, so they survive an abort.
    pub fn flush_log(&mut self) {
        if let Some(writer) = &mut self.log_file {
            let _ = writer.flush();
        }
    }
}
