use sdl2::audio::AudioQueue;
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::time::Instant;

use crate::disk::DiskController;
use crate::video::{ADDR_VGA_GRAPHICS, ADDR_VGA_TEXT, SIZE_GRAPHICS, SIZE_TEXT, VideoMode};

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

pub struct Bus {
    pub ram: Vec<u8>,          // 1MB System RAM
    pub video_mode: VideoMode, // Current State
    pub disk: DiskController,
    pub keyboard_buffer: VecDeque<u16>, // Stores (Scancode << 8) | ASCII
    /// Last scan code delivered to port 0x60. Real hardware latches the byte
    /// there until the CPU reads it. High bit set = key release.
    pub last_scan_code: u8,
    /// True while a key-scan IRQ1 (INT 09h) is pending delivery. Set by the
    /// SDL event handler on key-down/key-up, cleared by the emulator loop
    /// once the INT 09h ISR has been invoked.
    pub irq1_pending: bool,
    pub cursor_x: usize,
    pub cursor_y: usize,
    pub start_time: Instant, // System timer
    pub audio_device: Option<AudioQueue<i16>>,
    pub speaker_on: bool,    // Is the speaker playing?
    pub pit_divisor: u16,    // Current Frequency Divisor
    pub pit_mode: u8,        // PIT Command Mode
    pub pit_write_msb: bool, // Toggle to handle 2-byte writes (LSB/MSB)
    pub pit0_divisor: u16,
    pub pit0_write_msb: bool,
    /// Toggle for alternating LSB/MSB when reading port 0x40 in 2-byte mode.
    pub pit0_read_msb: bool,
    /// Value latched into the read buffer by a `latch counter` command on
    /// port 0x43. When `pit0_latched_active` is true, reads of port 0x40
    /// return this value instead of the live count until both bytes are read.
    pub pit0_latched: u16,
    pub pit0_latched_active: bool,
    pub pic_mask: u8,
    pub audio_phase: f32, // Track wave position to prevent clicking
    pub dta_segment: u16,
    pub dta_offset: u16,
    pub log_file: Option<BufWriter<File>>,
    /// Segment returned by the most recent INT 21h AH=48h allocation. The
    /// MicroProse TSR stub (INT 21h AX=FFFFh / AX=BFBFh) treats "the last
    /// alloc" as the destination for its file loads.
    pub last_alloc_segment: u16,
    /// Most recent file handle returned by INT 21h AH=3Dh.
    pub last_opened_handle: u16,
    /// Name of the most recently opened file, for TSR-emulated reads.
    pub last_opened_filename: String,

    // VGA State
    pub vga: crate::video::vga::VgaCard,
    pub search_handles: std::collections::HashMap<u32, String>,

    // Mouse State (INT 33h)
    pub mouse: crate::mouse::MouseState,

    // AdLib / OPL2 FM synthesizer (ports 0x388/0x389)
    pub adlib: crate::adlib::AdLib,

    /// Per-4KB-page generation counter covering the full 1 MiB address space
    /// (256 pages). Bumped on every write inside the Bus write helpers. The
    /// decoded-instruction cache stores the gen at decode time and invalidates
    /// a cached entry when the gen for its page has changed — this is how we
    /// stay correct in the face of self-modifying code (LZEXE, packers, etc.)
    /// without paying the cost of verifying cached bytes on every fetch.
    pub page_gen: [u32; 256],
}

use std::path::PathBuf;

impl Bus {
    pub fn new(root_path: PathBuf) -> Self {
        let mut bus = Self {
            ram: vec![0; 1024 * 1024],
            video_mode: VideoMode::Text80x25, // Start in Text Mode (BIOS default)
            disk: DiskController::new(root_path),
            keyboard_buffer: VecDeque::new(),
            last_scan_code: 0,
            irq1_pending: false,
            cursor_x: 0,
            cursor_y: 0,
            start_time: Instant::now(),
            audio_device: None,
            speaker_on: false,
            pit_divisor: 0xFFFF,
            pit_mode: 0,
            pit_write_msb: false,
            pit0_divisor: 0xFFFF,
            pit0_write_msb: false,
            pit0_read_msb: false,
            pit0_latched: 0,
            pit0_latched_active: false,
            pic_mask: 0x00,
            audio_phase: 0.0,
            log_file: None,
            last_alloc_segment: 0,
            last_opened_handle: 0xFFFF,
            last_opened_filename: String::new(),
            dta_segment: 0x1000,
            dta_offset: 0x0000,
            vga: crate::video::vga::VgaCard::new(),
            search_handles: std::collections::HashMap::new(),
            mouse: crate::mouse::MouseState::new(),
            adlib: crate::adlib::AdLib::new(),
            page_gen: [0; 256],
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

        // Install HLE traps

        bus.install_hle_trap(0x10, 0xF1000); // Video
        bus.install_hle_trap(0x11, 0xF1004); // Equipment
        bus.install_hle_trap(0x12, 0xF1008); // Memory
        bus.install_hle_trap(0x15, 0xF100C); // System
        bus.install_hle_trap(0x16, 0xF1010); // Keyboard
        bus.install_hle_trap(0x1A, 0xF1014); // Time
        bus.install_hle_trap(0x20, 0xF1018); // Terminate
        bus.install_hle_trap(0x21, 0xF101C); // DOS
        bus.install_hle_trap(0x2F, 0xF1020); // Shell Command
        bus.install_hle_trap(0x33, 0xF1024); // Mouse

        // Build a baseline MCB chain — one large free block covering
        // conventional memory. load_shell / load_exe rebuild as needed, but we
        // still want mcb::alloc to work for tests and any early allocation.
        crate::mcb::init_empty(&mut bus);

        bus
    }

    /// Installs a Magic Trap (FE 38 <Vector> CF) at the given Physical Address
    /// and updates the IVT to point to it.
    fn install_hle_trap(&mut self, vector: u8, phys_addr: usize) {
        // Update IVT (0000:Vector*4)
        let ivt_offset = (vector as usize) * 4;
        let handler_offset = (phys_addr & 0xFFFF) as u16; // Offset part of F000:Offset

        self.write_16(ivt_offset, handler_offset); // IP
        self.write_16(ivt_offset + 2, 0xF000); // CS

        // Write Trap Code
        self.write_8(phys_addr, 0xFE); // BOP
        self.write_8(phys_addr + 1, 0x38); // Magic
        self.write_8(phys_addr + 2, vector); // The Vector ID
        self.write_8(phys_addr + 3, 0xCF); // IRET
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
        self.vga.dirty = true;
    }

    #[inline(always)]
    pub fn read_8(&self, addr: usize) -> u8 {
        // Fast path — the vast majority of memory accesses (code fetch,
        // stack, program data) land below 0xA0000 and don't need the VGA
        // range checks. One comparison covers them.
        if addr < ADDR_VGA_GRAPHICS {
            // SAFETY: ram is a fixed 1 MiB buffer; addr < 0xA0000 is in range.
            return unsafe { *self.ram.get_unchecked(addr) };
        }
        if addr < ADDR_VGA_GRAPHICS + SIZE_GRAPHICS {
            // Route through VGA so chain-4, odd/even, and Read Map Select
            // work correctly. read_graphics also latches planes, needed
            // for planar read-modify-write sequences.
            return self.vga.read_graphics(addr - ADDR_VGA_GRAPHICS);
        }
        if addr >= ADDR_VGA_TEXT && addr < ADDR_VGA_TEXT + SIZE_TEXT {
            return self.vga.vram_text[addr - ADDR_VGA_TEXT];
        }
        self.ram[addr]
    }

    // Returns true if a write occurred to the *active* video memory
    #[inline(always)]
    pub fn write_8(&mut self, addr: usize, value: u8) -> bool {
        // Fast path — conventional memory writes are the overwhelming
        // majority. One comparison routes them to the ram Vec, skipping
        // both VGA range checks.
        if addr < ADDR_VGA_GRAPHICS {
            // SAFETY: ram is a fixed 1 MiB buffer; addr < 0xA0000 is in range.
            unsafe {
                *self.ram.get_unchecked_mut(addr) = value;
                // Bump generation for this page so the decoded-instruction
                // cache invalidates any cached decodes that fell in it.
                let page = (addr >> 12) & 0xFF;
                let g = self.page_gen.get_unchecked_mut(page);
                *g = g.wrapping_add(1);
            }
            return false;
        }
        if addr < ADDR_VGA_GRAPHICS + SIZE_GRAPHICS {
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
            self.vga.vram_text[addr - ADDR_VGA_TEXT] = value;
            self.vga.dirty = true;

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
        self.ram[addr] = value;
        let page = (addr >> 12) & 0xFF;
        self.page_gen[page] = self.page_gen[page].wrapping_add(1);
        false
    }

    // Write a 16-bit value to memory (Little Endian)
    pub fn write_16(&mut self, addr: usize, value: u16) -> bool {
        // Low byte
        let d1 = self.write_8(addr, (value & 0xFF) as u8);
        // High byte
        let d2 = self.write_8(addr + 1, (value >> 8) as u8);
        d1 || d2
    }

    // read_16 helper
    pub fn read_16(&self, addr: usize) -> u16 {
        let low = self.read_8(addr) as u16;
        let high = self.read_8(addr + 1) as u16;
        (high << 8) | low
    }

    pub fn read_32(&self, addr: usize) -> u32 {
        let low = self.read_16(addr) as u32;
        let high = self.read_16(addr + 2) as u32;
        (high << 16) | low
    }

    pub fn write_32(&mut self, addr: usize, value: u32) {
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

    // Write to an I/O Port
    pub fn io_write(&mut self, port: u16, value: u8) {
        match port {
            // PIC (Programmable Interrupt Controller) 0x20 / 0x21
            // We ignore initialization words (ICWs) but acknowledge EOI (0x20).
            0x20 => {
                self.log_string("[PIC] EOI Received");
                // Command Register. 0x20 = End of Interrupt (EOI).
                // log_string("[PIC] Command received");
            }
            0x21 => {
                self.log_string(&format!("[PIC] IMR Set to {:02X}", value));
                self.pic_mask = value;
            }

            // Port 0x40: Channel 0 Data (System Timer)
            // Controls the system tick rate (IRQ 0).
            // Default is 18.2 Hz (Divisor 65535).
            0x40 => {
                if !self.pit0_write_msb {
                    // Write LSB
                    self.pit0_divisor = (self.pit0_divisor & 0xFF00) | (value as u16);
                    self.pit0_write_msb = true; // Next write is MSB
                } else {
                    // Write MSB
                    self.pit0_divisor = (self.pit0_divisor & 0x00FF) | ((value as u16) << 8);
                    self.pit0_write_msb = false; // Reset to LSB

                    if self.pit0_divisor > 0 {
                        let hz = 1_193_182 / self.pit0_divisor as u32;
                        self.log_string(&format!("[PIT] Channel 0 Frequency set to {} Hz", hz));
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
                        self.pit0_latched = self.pit0_current_count();
                        self.pit0_latched_active = true;
                        self.pit0_read_msb = false;
                    }
                } else {
                    match channel {
                        0 => self.pit0_write_msb = false, // Reset Channel 0 LSB/MSB
                        2 => self.pit_write_msb = false,  // Reset Channel 2 LSB/MSB
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
            0x388 => {
                self.adlib.write_register_select(value);
            }
            0x389 => {
                self.adlib.write_register_data(value);
            }

            // Dispatch to Devices
            // TODO: Use a proper map lookup
            // Ports we intentionally ignore — writes are harmless but other-
            // wise spam the log. Programs blindly touch these as leftovers
            // from CGA/EGA-era code even when they're really talking to VGA.
            0x3D8 | 0x3D9 => {
                // CGA Mode Control / Color Select. Real VGA ignores writes
                // here; VGA mode lives at 0x3D4/0x3D5 (handled by the VGA).
            }
            0xA0 | 0xA1 => {
                // Slave PIC — we don't model cascaded IRQs.
            }

            _ => {
                if self.vga.ports().contains(&port) {
                    self.vga.io_write(port, value);
                    // Suppress the per-write log for DAC ports (0x3C6..0x3C9):
                    // a full 256-color palette update is 1024 writes, which
                    // buries everything else in the trace. Still log the less
                    // frequent mode / register writes.
                    if !matches!(port, 0x3C6..=0x3C9) {
                        self.log_string(&format!(
                            "[VGA-IO] Write Port {:04X} Value {:02X}",
                            port, value
                        ));
                    }

                    // Check if video mode changed
                    if let Some(new_mode) = self.vga.check_video_mode() {
                        if self.video_mode != new_mode && new_mode == VideoMode::Graphics320x200 {
                            self.log_string("[VGA] Switch to Graphics320x200 detected via IO");
                            self.video_mode = new_mode;
                            self.vga.dirty = true;
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

    /// Compute the current PIT channel 0 count (0..=divisor-1). Real hardware
    /// counts DOWN from `divisor` toward 0 at 1.193182 MHz, then reloads.
    /// Programs time short intervals by latching two reads and subtracting —
    /// if we always returned 0xFF, the two reads would be identical, elapsed
    /// would evaluate to zero, and the next `DIV elapsed` would crash.
    fn pit0_current_count(&self) -> u16 {
        let divisor = if self.pit0_divisor == 0 {
            0x10000u32
        } else {
            self.pit0_divisor as u32
        };
        let micros = self.start_time.elapsed().as_micros() as u64;
        // ticks = micros * 1_193_182 / 1_000_000, done without overflow.
        let ticks = micros.wrapping_mul(1_193_182) / 1_000_000;
        let rem = (ticks % divisor as u64) as u32;
        ((divisor - 1 - rem) & 0xFFFF) as u16
    }

    // Read from an I/O Port
    pub fn io_read(&mut self, port: u16) -> u8 {
        match port {
            // Port 0x40 — PIT channel 0 (system timer) data. The counter
            // decrements at 1.193 MHz. Programs that need sub-tick timing
            // (MicroProse's VGAME computes 1/elapsed_time, which faults if
            // elapsed == 0) issue a latch command and read LSB then MSB.
            0x40 => {
                let val = if self.pit0_latched_active {
                    self.pit0_latched
                } else {
                    self.pit0_current_count()
                };
                let byte = if !self.pit0_read_msb {
                    self.pit0_read_msb = true;
                    (val & 0xFF) as u8
                } else {
                    self.pit0_read_msb = false;
                    if self.pit0_latched_active {
                        self.pit0_latched_active = false;
                    }
                    (val >> 8) as u8
                };
                byte
            }

            // Port 0x60 — Keyboard data port. Real hardware latches the
            // last-received scan code here; programs either read this from
            // their INT 09h ISR after IRQ1 fires, or poll it directly.
            0x60 => self.last_scan_code,

            // Port 0x64 — Keyboard controller status (8042).
            //   Bit 0 = output buffer full (1 = scan code ready to read)
            //   Bit 1 = input buffer full (we never have commands pending)
            // We report "output ready" whenever a key event is pending.
            0x64 => {
                if self.irq1_pending {
                    0x01
                } else {
                    0x00
                }
            }

            // AdLib status register (port 0x388). Bit 7 = IRQ, bit 6 = timer1
            // expired, bit 5 = timer2 expired. Games poll this to detect the
            // card by arming timer1 and checking that the bits flip in time.
            0x388 => self.adlib.read_status(),
            0x389 => 0xFF,

            // Read PPI Port B (Speaker State)
            0x61 => {
                let mut val = 0;
                if self.speaker_on {
                    val |= 0x03;
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
    }

    pub fn log_trace(&mut self, s: &str) {
        if self.log_file.is_none() {
            let file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open("trace.log")
                .expect("Failed to open trace.log");
            self.log_file = Some(BufWriter::new(file));
        }

        // NO PRINTLN
        if let Some(writer) = &mut self.log_file {
            let _ = writeln!(writer, "{}", s);
        }
    }
}
