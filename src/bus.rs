use sdl2::audio::AudioQueue;
use std::collections::VecDeque;
use std::time::Instant;

use crate::disk::DiskController;
use crate::video::{VideoMode, ADDR_VGA_GRAPHICS, ADDR_VGA_TEXT, SIZE_GRAPHICS, SIZE_TEXT};

pub struct Bus {
    pub ram: Vec<u8>,           // 1MB System RAM
    pub vram_graphics: Vec<u8>, // 0xA0000
    pub vram_text: Vec<u8>,     // 0xB8000
    pub video_mode: VideoMode,  // Current State
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
    pub audio_phase: f32,    // Track wave position to prevent clicking
}

impl Bus {
    pub fn new() -> Self {
        Self {
            ram: vec![0; 1024 * 1024],
            vram_graphics: vec![0; SIZE_GRAPHICS],
            vram_text: vec![0; SIZE_TEXT],
            video_mode: VideoMode::Text80x25, // Start in Text Mode (BIOS default)
            disk: DiskController::new(),
            keyboard_buffer: VecDeque::new(),
            cursor_x: 0,
            cursor_y: 0,
            start_time: Instant::now(),
            audio_device: None,
            speaker_on: false,
            pit_divisor: 0xFFFF,
            pit_mode: 0,
            pit_write_msb: false,
            audio_phase: 0.0,
        }
    }

    // Helper: Scroll the text screen up by 1 line
    pub fn scroll_up(&mut self) {
        // Row 1 becomes Row 0, etc.
        // Each row is 160 bytes (80 chars * 2 bytes)
        let row_size = 160;
        let screen_size = 25 * row_size;

        // Move memory back
        for i in 0..(screen_size - row_size) {
            self.vram_text[i] = self.vram_text[i + row_size];
        }

        // Clear bottom row
        for i in (screen_size - row_size)..screen_size {
            if i % 2 == 0 {
                self.vram_text[i] = 0x20;
            }
            // Space
            else {
                self.vram_text[i] = 0x07;
            } // Light Gray
        }
    }

    pub fn read_8(&self, addr: usize) -> u8 {
        if addr >= ADDR_VGA_GRAPHICS && addr < ADDR_VGA_GRAPHICS + SIZE_GRAPHICS {
            self.vram_graphics[addr - ADDR_VGA_GRAPHICS]
        } else if addr >= ADDR_VGA_TEXT && addr < ADDR_VGA_TEXT + SIZE_TEXT {
            self.vram_text[addr - ADDR_VGA_TEXT]
        } else {
            self.ram[addr]
        }
    }

    // Returns true if a write occurred to the *active* video memory
    pub fn write_8(&mut self, addr: usize, value: u8) -> bool {
        if addr >= ADDR_VGA_GRAPHICS && addr < ADDR_VGA_GRAPHICS + SIZE_GRAPHICS {
            self.vram_graphics[addr - ADDR_VGA_GRAPHICS] = value;
            self.video_mode == VideoMode::Graphics320x200 // Dirty only if active
        } else if addr >= ADDR_VGA_TEXT && addr < ADDR_VGA_TEXT + SIZE_TEXT {
            self.vram_text[addr - ADDR_VGA_TEXT] = value;
            self.video_mode == VideoMode::Text80x25 // Dirty only if active
        } else {
            self.ram[addr] = value;
            false
        }
    }

    // Write to an I/O Port
    pub fn io_write(&mut self, port: u16, value: u8) {
        match port {
            // PIT Channel 2 Data (Port 0x42) ---
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
                // If writing to Channel 2 (Bits 7-6 = 10), reset the LSB/MSB toggle
                if (value & 0xC0) == 0x80 {
                    self.pit_write_msb = false;
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

            _ => {
                // Unhandled port write
                println!(
                    "[Unhandled IO Write] Port: {:04X}, Value: {:02X}",
                    port, value
                );
            }
        }
    }

    // Read from an I/O Port
    pub fn io_read(&self, port: u16) -> u8 {
        match port {
            // Read PPI Port B (Speaker State)
            0x61 => {
                let mut val = 0;
                if self.speaker_on {
                    val |= 0x03;
                }
                val
            }
            _ => 0xFF, // Default open bus
        }
    }
}
