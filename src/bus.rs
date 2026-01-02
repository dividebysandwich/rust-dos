use sdl2::audio::AudioQueue;
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::time::Instant;

use crate::disk::DiskController;
use crate::video::{ADDR_VGA_GRAPHICS, ADDR_VGA_TEXT, SIZE_GRAPHICS, SIZE_TEXT, VideoMode};

pub trait Device {
    fn ports(&self) -> Vec<u16>;
    fn io_read(&mut self, port: u16) -> u8;
    fn io_write(&mut self, port: u16, value: u8);
    fn step(&mut self) {}
}

pub struct Bus {
    pub ram: Vec<u8>,          // 1MB System RAM
    pub video_mode: VideoMode, // Current State
    pub disk: DiskController,
    pub keyboard_buffer: VecDeque<u16>, // Stores (Scancode << 8) | ASCII
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
    pub pic_mask: u8,
    pub audio_phase: f32, // Track wave position to prevent clicking
    pub dta_segment: u16,
    pub dta_offset: u16,
    pub log_file: Option<BufWriter<File>>,

    // VGA State
    pub vga: crate::video::vga::VgaCard,
    pub search_handles: std::collections::HashMap<u32, String>,
}

use std::path::PathBuf;

impl Bus {
    pub fn new(root_path: PathBuf) -> Self {
        let mut bus = Self {
            ram: vec![0; 1024 * 1024],
            video_mode: VideoMode::Text80x25, // Start in Text Mode (BIOS default)
            disk: DiskController::new(root_path),
            keyboard_buffer: VecDeque::new(),
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
            pic_mask: 0x00,
            audio_phase: 0.0,
            log_file: None,
            dta_segment: 0x1000,
            dta_offset: 0x0000,
            vga: crate::video::vga::VgaCard::new(),
            search_handles: std::collections::HashMap::new(),
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

        // 0x0484: Rows on Screen (minus 1). 24
        bus.write_8(0x0484, 24);

        // 0x0487: EGA/VGA Info. Bits 5-6 = 11 (256KB Video RAM).
        // 0x60 = 01100000
        bus.write_8(0x0487, 0x60);

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
        // Row 1 becomes Row 0, etc.
        // Each row is 160 bytes (80 chars * 2 bytes)
        let row_size = 160;
        let screen_size = 25 * row_size;

        // Move memory back
        for i in 0..(screen_size - row_size) {
            self.vga.vram_text[i] = self.vga.vram_text[i + row_size];
        }

        // Clear bottom row
        for i in (screen_size - row_size)..screen_size {
            if i % 2 == 0 {
                self.vga.vram_text[i] = 0x20;
            }
            // Space
            else {
                self.vga.vram_text[i] = 0x07;
            } // Light Gray
        }
    }

    pub fn read_8(&self, addr: usize) -> u8 {
        // if addr >= 0x116F2 && addr < 0x116F2 + 12 {
        //      println!("[MEM WATCH] CPU reading DTA Filename @ {:05X}. Value: {:02X} ({})",
        //               addr, self.ram[addr], self.ram[addr] as char);
        // }
        if addr >= ADDR_VGA_GRAPHICS && addr < ADDR_VGA_GRAPHICS + SIZE_GRAPHICS {
            self.vga.vram_graphics[addr - ADDR_VGA_GRAPHICS]
        } else if addr >= ADDR_VGA_TEXT && addr < ADDR_VGA_TEXT + SIZE_TEXT {
            self.vga.vram_text[addr - ADDR_VGA_TEXT]
        } else {
            self.ram[addr]
        }
    }

    // Returns true if a write occurred to the *active* video memory
    pub fn write_8(&mut self, addr: usize, value: u8) -> bool {
        //if addr >= 0xB8000 && addr < 0xB8FA0 && (addr % 2 == 0) {
        // if value >= 0x20 && value <= 0x7E { // Printable chars only
        //     let offset = (addr - 0xB8000) / 2;
        //     let row = offset / 80;
        //     let col = offset % 80;
        //     self.log_string(&format!("[VIDEO] '{}' @ {},{}", value as char, col, row));
        // }
        //}

        if addr >= ADDR_VGA_GRAPHICS && addr < ADDR_VGA_GRAPHICS + SIZE_GRAPHICS {
            self.vga.write_graphics(addr - ADDR_VGA_GRAPHICS, value);
            self.video_mode == VideoMode::Graphics320x200
        } else if addr >= ADDR_VGA_TEXT && addr < ADDR_VGA_TEXT + SIZE_TEXT {
            self.vga.vram_text[addr - ADDR_VGA_TEXT] = value;

            // Check if current mode uses this memory
            match self.video_mode {
                VideoMode::Text80x25
                | VideoMode::Text80x25Color
                | VideoMode::Text40x25
                | VideoMode::Text40x25Color
                | VideoMode::Cga320x200
                | VideoMode::Cga320x200Color
                | VideoMode::Cga640x200 => true, // Dirty!
                _ => false,
            }
        } else {
            self.ram[addr] = value;
            false
        }
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

                // If the command is for the Counter (not Read-Back), reset the flip-flop.
                // We check Access bits (5-4) to ensure it's not a Latch command (00).
                let access = (value >> 4) & 0x03;

                if access != 0 {
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

            // Dispatch to Devices
            // TODO: Use a proper map lookup
            _ => {
                if self.vga.ports().contains(&port) {
                    self.vga.io_write(port, value);
                    // Log manual VGA writes
                    self.log_string(&format!(
                        "[VGA-IO] Write Port {:04X} Value {:02X}",
                        port, value
                    ));

                    // Check if video mode changed
                    if let Some(new_mode) = self.vga.check_video_mode() {
                        if self.video_mode != new_mode && new_mode == VideoMode::Graphics320x200 {
                            self.log_string("[VGA] Switch to Graphics320x200 detected via IO");
                            self.video_mode = new_mode;
                            // Clear VRAM?
                            // memset(&mut self.vram_graphics, 0);
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
        match port {
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
