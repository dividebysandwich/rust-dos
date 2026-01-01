use crate::bus::Device;
use std::cell::Cell;

pub struct VgaCard {
    pub sequencer_index: u8,
    pub sequencer_regs: [u8; 5],
    pub graphics_index: u8,
    pub graphics_regs: [u8; 9],
    pub crtc_index: u8,
    pub crtc_regs: [u8; 25],
    pub dac_write_index: u8,
    pub dac_read_index: u8,
    pub dac_step: u8,
    pub misc_output_reg: u8,
    pub retrace_counter: u8,
    pub palette: Vec<u8>, // 256 * 3
    pub vram_graphics: Vec<u8>,
    pub vram_text: Vec<u8>,
    pub latches: Cell<[u8; 4]>,

    // Attribute Controller
    pub attribute_index: u8,
    pub attribute_regs: [u8; 21],  // 0-0xF: Palette, 0x10-0x14: Control
    pub attribute_flip_flop: bool, // false = Address, true = Data
}

impl VgaCard {
    pub fn new() -> Self {
        let mut palette = vec![0; 768];

        // Initialize with default VGA colors (Procedural generation)
        for i in 0..256 {
            let (r, g, b) = match i {
                0x00 => (0, 0, 0),       // Black
                0x01 => (0, 0, 170),     // Blue
                0x02 => (0, 170, 0),     // Green
                0x03 => (0, 170, 170),   // Cyan
                0x04 => (170, 0, 0),     // Red
                0x05 => (170, 0, 170),   // Magenta
                0x06 => (170, 85, 0),    // Brown
                0x07 => (170, 170, 170), // Light Gray
                0x08 => (85, 85, 85),    // Dark Gray
                0x09 => (85, 85, 255),   // Light Blue
                0x0A => (85, 255, 85),   // Light Green
                0x0B => (85, 255, 255),  // Light Cyan
                0x0C => (255, 85, 85),   // Light Red
                0x0D => (255, 85, 255),  // Light Magenta
                0x0E => (255, 255, 85),  // Yellow
                0x0F => (255, 255, 255), // White
                _ => {
                    // 6-bit procedural generation for the rest
                    // We must generate 8-bit first then downscale?
                    // Or just logic it out.
                    // The old logic was:
                    // r = (index % 32) * 8;
                    // g = (index % 64) * 4;
                    // b = (index % 128) * 2;
                    // Those produce 0-255 range.
                    let r = (i % 32) * 8;
                    let g = (i % 64) * 4;
                    let b = (i % 128) * 2;
                    (r as u8, g as u8, b as u8)
                }
            };

            // Store as 6-bit values (Host 8-bit >> 2)
            palette[i * 3] = r >> 2;
            palette[i * 3 + 1] = g >> 2;
            palette[i * 3 + 2] = b >> 2;
        }

        let mut sequencer_regs = [0u8; 5];
        sequencer_regs[4] = 0x02; // Extended Memory (Odd/Even)

        let mut graphics_regs = [0u8; 9];
        graphics_regs[5] = 0x10; // Mode: Odd/Even (10)
        graphics_regs[6] = 0x0E; // Misc: Memory Map B8000 (10), Text Mode (0)

        Self {
            sequencer_index: 0,
            sequencer_regs,
            graphics_index: 0,
            graphics_regs,
            crtc_index: 0,
            crtc_regs: [0; 25],
            dac_write_index: 0,
            dac_read_index: 0,
            dac_step: 0,
            misc_output_reg: 0x67, // Text Mode (Color + RAM Enable)
            retrace_counter: 0,
            palette,
            vram_graphics: vec![0; 256 * 1024], // 256KB (4 Planes x 64KB)
            vram_text: vec![0; 32 * 1024],      // 32KB (B8000-BFFFF)
            latches: Cell::new([0; 4]),
            attribute_index: 0,
            attribute_regs: [0; 21],
            attribute_flip_flop: false,
        }
    }

    pub fn get_rgb(&self, index: u8) -> (u8, u8, u8) {
        let base = (index as usize) * 3;
        if base + 2 < self.palette.len() {
            let r = self.palette[base] << 2; // Convert 6-bit (0-63) to 8-bit (0-255) roughly
            let g = self.palette[base + 1] << 2;
            let b = self.palette[base + 2] << 2;
            // Accurate scaling: (val * 255) / 63
            // But simple shift << 2 is (val * 4) -> range 0-252. Good enough.
            (r, g, b)
        } else {
            (0, 0, 0)
        }
    }

    pub fn check_video_mode(&self) -> Option<super::VideoMode> {
        // Check for Mode 13h (320x200 256 Color)

        let gfx_mode = self.graphics_regs[0x05];
        let is_256_color = (gfx_mode & 0x40) != 0;

        // Sequencer Memory Mode (Index 0x04)
        // Bit 3: Chain 4 (1=Enable/Doubleword aka Mode 13h, 0=Sequential/Byte/Word)
        let seq_mem_mode = self.sequencer_regs[0x04];
        let chain4 = (seq_mem_mode & 0x08) != 0;

        // Misc Output (0x3C2)
        // Bit 0: 0 = Mono (3B4), 1 = Color (3D4)
        // Bit 6: Hsync Polarity
        // Bit 7: Vsync Polarity
        // Mode 13h: Color (1)
        let misc = self.misc_output_reg;
        let is_color = (misc & 0x01) != 0;

        // REMOVEME
        println!(
            "[VGA CHECK] Misc={:02X} Seq04={:02X} Gfx05={:02X}",
            misc, seq_mem_mode, gfx_mode
        );

        if is_color && is_256_color && chain4 {
            return Some(super::VideoMode::Graphics320x200);
        }

        None
    }

    pub fn read_graphics(&self, offset: usize) -> u8 {
        // Mode 13h Check (Chain 4)
        let seq_mem_mode = self.sequencer_regs[0x04];
        let chain4 = (seq_mem_mode & 0x08) != 0;
        let odd_even = (seq_mem_mode & 0x02) != 0;

        // Latch Loading & Offset Calculation
        let plane_offset = if chain4 {
            offset >> 2
        } else if odd_even {
            offset >> 1
        } else {
            offset
        };

        let mut new_latches = [0u8; 4];
        for p in 0..4 {
            let idx = (p * 65536) + plane_offset;
            if idx < self.vram_graphics.len() {
                new_latches[p] = self.vram_graphics[idx];
            }
        }
        self.latches.set(new_latches);

        let final_index: usize;

        if chain4 {
            let plane = offset & 3;
            final_index = (plane * 65536) + plane_offset;
        } else {
            // Read Map Select
            let read_map = self.graphics_regs[0x04] & 0x03;
            // In Odd/Even mode, typically Read Map selects the plane,
            // but the offset is shifted. Address LSB doesn't force plane selection for READs
            // the same way it does for WRITEs (usually).
            // Exception: "Two Way" or "Chain 2" modes.
            // For now, respect Read Map.
            final_index = (read_map as usize * 65536) + plane_offset;
        }

        if final_index < self.vram_graphics.len() {
            self.vram_graphics[final_index]
        } else {
            0xFF
        }
    }

    pub fn write_graphics(&mut self, offset: usize, value: u8) {
        let seq_mem_mode = self.sequencer_regs[0x04];
        let chain4 = (seq_mem_mode & 0x08) != 0;
        let odd_even = (seq_mem_mode & 0x02) != 0;

        // Planar Offset
        let plane_offset = if chain4 {
            offset >> 2
        } else if odd_even {
            offset >> 1
        } else {
            offset
        };

        // Determine planes to write
        let mut planes_to_write = if chain4 {
            1 << (offset & 3)
        } else {
            self.sequencer_regs[0x02] & 0x0F
        };

        // Apply Odd/Even Plane Masking
        if odd_even && !chain4 {
            if (offset & 1) == 0 {
                // Even Address: Planes 0 & 2
                planes_to_write &= 0x05; // 0101
            } else {
                // Odd Address: Planes 1 & 3
                planes_to_write &= 0x0A; // 1010
            }
        }

        // Bit Mask (Graphics Reg 8)
        let bit_mask = self.graphics_regs[0x08];
        let latches = self.latches.get();

        // Basic Write Mode 0 Implementation
        for p in 0..4 {
            if (planes_to_write & (1 << p)) != 0 {
                // Combine CPU data with Latch data using Bit Mask
                // Result = (CPU & Mask) | (Latch & ~Mask)
                let latch_val = latches[p];
                let val_to_write = (value & bit_mask) | (latch_val & !bit_mask);

                let idx = (p * 65536) + plane_offset;
                if idx < self.vram_graphics.len() {
                    self.vram_graphics[idx] = val_to_write;
                }
            }
        }
    }

    pub fn set_video_mode(&mut self, mode: super::VideoMode) {
        match mode {
            super::VideoMode::Graphics320x200 => {
                // Initialize Registers for Mode 13h

                // Misc Output
                self.misc_output_reg = 0x63;

                // Sequencer
                self.sequencer_regs[0] = 0x03; // Reset
                self.sequencer_regs[1] = 0x01; // Clocking Mode
                self.sequencer_regs[2] = 0x0F; // Map Mask (All planes)
                self.sequencer_regs[3] = 0x00; // Char Map Select
                self.sequencer_regs[4] = 0x0E; // Memory Mode (Chain 4)

                // Graphics Controller
                self.graphics_regs[0] = 0x00; // Set/Reset
                self.graphics_regs[1] = 0x00; // Enable Set/Reset
                self.graphics_regs[2] = 0x00; // Color Compare
                self.graphics_regs[3] = 0x00; // Data Rotate
                self.graphics_regs[4] = 0x00; // Read Map Select
                self.graphics_regs[5] = 0x40; // Mode Register (256 Color)
                self.graphics_regs[6] = 0x05; // Misc (Graphics + A0000)
                self.graphics_regs[7] = 0x0F; // Color Don't Care
                self.graphics_regs[8] = 0xFF; // Bit Mask

                // Attribute Controller
                self.attribute_regs[0x10] = 0x41; // Mode Control (Graphics)
                self.attribute_regs[0x11] = 0x00; // Overscan
                self.attribute_regs[0x12] = 0x0F; // Color Plane Enable
                self.attribute_regs[0x13] = 0x00; // Horizontal Panning
            }
            _ => {
                // Text Mode defaults?
            }
        }
    }
}

impl Device for VgaCard {
    fn ports(&self) -> Vec<u16> {
        vec![
            0x3C2, // Misc Output (Write) / Input Status 0 (Read)
            0x3C3, // Video Enable
            0x3C4, 0x3C5, // Sequencer
            0x3CE, 0x3CF, // Graphics
            0x3CC, // Misc Output Read
            0x3D4, 0x3D5, // CRTC
            0x3C8, 0x3C9, // DAC
            0x3DA, // Status
        ]
    }

    fn io_read(&mut self, port: u16) -> u8 {
        // println!("[VGA] Read Port {:04X}", port);
        match port {
            0x3DA => {
                // Input Status #1
                // Reading 3DA resets the Attribute Controller Flip-Flop to Address Mode
                self.attribute_flip_flop = false;

                // Toggle VRetrace (Bit 3) and Display Enable (Bit 0)
                self.retrace_counter = self.retrace_counter.wrapping_add(1);

                // Toggle active/retrace every 8 reads to simulate timing
                if (self.retrace_counter & 8) != 0 {
                    0x09 // Retrace Active (Bit 3) + Display Disabled (Bit 0)
                } else {
                    0x00 // Display Active, No Retrace
                }
            }
            0x3C1 => {
                let val = if (self.attribute_index as usize) < self.attribute_regs.len() {
                    self.attribute_regs[self.attribute_index as usize]
                } else {
                    0
                };
                // println!("[VGA] Read Attr {:02X} -> {:02X}", self.attribute_index, val);
                val
            }
            0x3CC => {
                println!("[VGA] Read Misc Output: {:02X}", self.misc_output_reg);
                self.misc_output_reg
            }
            0x3C5 => {
                let val = if (self.sequencer_index as usize) < self.sequencer_regs.len() {
                    self.sequencer_regs[self.sequencer_index as usize]
                } else {
                    0
                };
                println!("[VGA] Read Seq {:02X} -> {:02X}", self.sequencer_index, val);
                val
            }
            0x3CF => {
                let val = if (self.graphics_index as usize) < self.graphics_regs.len() {
                    self.graphics_regs[self.graphics_index as usize]
                } else {
                    0
                };
                println!("[VGA] Read Gfx {:02X} -> {:02X}", self.graphics_index, val);
                val
            }
            0x3D5 => {
                let val = if (self.crtc_index as usize) < self.crtc_regs.len() {
                    self.crtc_regs[self.crtc_index as usize]
                } else {
                    0
                };
                println!("[VGA] Read CRTC {:02X} -> {:02X}", self.crtc_index, val);
                val
            }
            _ => {
                println!("[VGA] Read Unhandled {:04X}", port);
                0xFF
            }
        }
    }

    fn io_write(&mut self, port: u16, value: u8) {
        match port {
            0x3C0 => {
                if !self.attribute_flip_flop {
                    // Address Mode
                    self.attribute_index = value & 0x1F;
                    self.attribute_flip_flop = true; // Switch to Data
                // Note: Bit 5 (0x20) controls Video Enable, important for blinking/screen off
                } else {
                    // Data Mode
                    if (self.attribute_index as usize) < self.attribute_regs.len() {
                        self.attribute_regs[self.attribute_index as usize] = value;
                        // println!("[VGA] Attr Reg {:02X} = {:02X}", self.attribute_index, value);
                    }
                    self.attribute_flip_flop = false; // Switch back to Address
                }
            }
            0x3C2 => {
                self.misc_output_reg = value;
                println!("[VGA] Write Misc Output: {:02X}", value);
            }
            0x3C4 => self.sequencer_index = value,
            0x3C5 => {
                if (self.sequencer_index as usize) < self.sequencer_regs.len() {
                    let mut val = value;
                    // Mask Map Mask to 4 bits
                    if self.sequencer_index == 0x02 {
                        val &= 0x0F;
                    }
                    // Mask Memory Mode (Index 4) to 0x0E (Chain4, O/E, Ext)
                    if self.sequencer_index == 0x04 {
                        val &= 0x0E;
                    }

                    self.sequencer_regs[self.sequencer_index as usize] = val;
                    // println!("[VGA] Seq Reg {:02X} = {:02X}", self.sequencer_index, val);
                }
            }
            0x3CE => self.graphics_index = value,
            0x3CF => {
                if (self.graphics_index as usize) < self.graphics_regs.len() {
                    let mut val = value;
                    // Mask Read Map Select to 2 bits
                    if self.graphics_index == 0x04 {
                        val &= 0x03;
                    }
                    // Mask Mode Register (Index 5)
                    if self.graphics_index == 0x05 {
                        val &= 0x73;
                    }

                    self.graphics_regs[self.graphics_index as usize] = val;
                    // println!("[VGA] Gfx Reg {:02X} = {:02X}", self.graphics_index, val);
                }
            }
            0x3D4 => self.crtc_index = value,
            0x3D5 => {
                if (self.crtc_index as usize) < self.crtc_regs.len() {
                    self.crtc_regs[self.crtc_index as usize] = value;
                    println!("[VGA] CRTC Reg {:02X} = {:02X}", self.crtc_index, value);
                }
            }
            0x3C8 => {
                self.dac_write_index = value;
                self.dac_step = 0;
            }
            0x3C9 => {
                let index = (self.dac_write_index as usize) * 3 + (self.dac_step as usize);
                if index < self.palette.len() {
                    self.palette[index] = value & 0x3F;
                }
                self.dac_step += 1;
                if self.dac_step == 3 {
                    self.dac_step = 0;
                    self.dac_write_index = self.dac_write_index.wrapping_add(1);
                }
            }
            _ => {}
        }
    }
}
