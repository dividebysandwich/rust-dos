use crate::bus::Device;
use std::cell::Cell;

/// Standard IBM VGA default 256-color palette for mode 13h.
/// 256 entries × 3 bytes (R, G, B) in 6-bit DAC form (0..=0x3F).
/// Layout:
///   0..=15   : 16 CGA/EGA basic colors
///   16..=31  : 16-step grayscale ramp
///   32..=247 : three 72-color HSV paragraphs (full/medium/low saturation
///              at three brightness levels)
///   248..=255: unused — reserved as zero by the standard BIOS.
#[rustfmt::skip]
static VGA_DEFAULT_PALETTE: [u8; 768] = [
    // 0-15: Standard EGA colors
    0x00,0x00,0x00, 0x00,0x00,0x2A, 0x00,0x2A,0x00, 0x00,0x2A,0x2A,
    0x2A,0x00,0x00, 0x2A,0x00,0x2A, 0x2A,0x15,0x00, 0x2A,0x2A,0x2A,
    0x15,0x15,0x15, 0x15,0x15,0x3F, 0x15,0x3F,0x15, 0x15,0x3F,0x3F,
    0x3F,0x15,0x15, 0x3F,0x15,0x3F, 0x3F,0x3F,0x15, 0x3F,0x3F,0x3F,
    // 16-31: Grayscale ramp
    0x00,0x00,0x00, 0x05,0x05,0x05, 0x08,0x08,0x08, 0x0B,0x0B,0x0B,
    0x0E,0x0E,0x0E, 0x11,0x11,0x11, 0x14,0x14,0x14, 0x18,0x18,0x18,
    0x1C,0x1C,0x1C, 0x20,0x20,0x20, 0x24,0x24,0x24, 0x28,0x28,0x28,
    0x2D,0x2D,0x2D, 0x32,0x32,0x32, 0x38,0x38,0x38, 0x3F,0x3F,0x3F,
    // 32-55: Full saturation, full value (24 hues)
    0x00,0x00,0x3F, 0x10,0x00,0x3F, 0x1F,0x00,0x3F, 0x2F,0x00,0x3F,
    0x3F,0x00,0x3F, 0x3F,0x00,0x2F, 0x3F,0x00,0x1F, 0x3F,0x00,0x10,
    0x3F,0x00,0x00, 0x3F,0x10,0x00, 0x3F,0x1F,0x00, 0x3F,0x2F,0x00,
    0x3F,0x3F,0x00, 0x2F,0x3F,0x00, 0x1F,0x3F,0x00, 0x10,0x3F,0x00,
    0x00,0x3F,0x00, 0x00,0x3F,0x10, 0x00,0x3F,0x1F, 0x00,0x3F,0x2F,
    0x00,0x3F,0x3F, 0x00,0x2F,0x3F, 0x00,0x1F,0x3F, 0x00,0x10,0x3F,
    // 56-79: Medium saturation, full value
    0x1F,0x1F,0x3F, 0x27,0x1F,0x3F, 0x2F,0x1F,0x3F, 0x37,0x1F,0x3F,
    0x3F,0x1F,0x3F, 0x3F,0x1F,0x37, 0x3F,0x1F,0x2F, 0x3F,0x1F,0x27,
    0x3F,0x1F,0x1F, 0x3F,0x27,0x1F, 0x3F,0x2F,0x1F, 0x3F,0x37,0x1F,
    0x3F,0x3F,0x1F, 0x37,0x3F,0x1F, 0x2F,0x3F,0x1F, 0x27,0x3F,0x1F,
    0x1F,0x3F,0x1F, 0x1F,0x3F,0x27, 0x1F,0x3F,0x2F, 0x1F,0x3F,0x37,
    0x1F,0x3F,0x3F, 0x1F,0x37,0x3F, 0x1F,0x2F,0x3F, 0x1F,0x27,0x3F,
    // 80-103: Low saturation, full value
    0x2D,0x2D,0x3F, 0x31,0x2D,0x3F, 0x36,0x2D,0x3F, 0x3A,0x2D,0x3F,
    0x3F,0x2D,0x3F, 0x3F,0x2D,0x3A, 0x3F,0x2D,0x36, 0x3F,0x2D,0x31,
    0x3F,0x2D,0x2D, 0x3F,0x31,0x2D, 0x3F,0x36,0x2D, 0x3F,0x3A,0x2D,
    0x3F,0x3F,0x2D, 0x3A,0x3F,0x2D, 0x36,0x3F,0x2D, 0x31,0x3F,0x2D,
    0x2D,0x3F,0x2D, 0x2D,0x3F,0x31, 0x2D,0x3F,0x36, 0x2D,0x3F,0x3A,
    0x2D,0x3F,0x3F, 0x2D,0x3A,0x3F, 0x2D,0x36,0x3F, 0x2D,0x31,0x3F,
    // 104-127: Full saturation, medium value
    0x00,0x00,0x1C, 0x07,0x00,0x1C, 0x0E,0x00,0x1C, 0x15,0x00,0x1C,
    0x1C,0x00,0x1C, 0x1C,0x00,0x15, 0x1C,0x00,0x0E, 0x1C,0x00,0x07,
    0x1C,0x00,0x00, 0x1C,0x07,0x00, 0x1C,0x0E,0x00, 0x1C,0x15,0x00,
    0x1C,0x1C,0x00, 0x15,0x1C,0x00, 0x0E,0x1C,0x00, 0x07,0x1C,0x00,
    0x00,0x1C,0x00, 0x00,0x1C,0x07, 0x00,0x1C,0x0E, 0x00,0x1C,0x15,
    0x00,0x1C,0x1C, 0x00,0x15,0x1C, 0x00,0x0E,0x1C, 0x00,0x07,0x1C,
    // 128-151: Medium saturation, medium value
    0x0E,0x0E,0x1C, 0x11,0x0E,0x1C, 0x15,0x0E,0x1C, 0x18,0x0E,0x1C,
    0x1C,0x0E,0x1C, 0x1C,0x0E,0x18, 0x1C,0x0E,0x15, 0x1C,0x0E,0x11,
    0x1C,0x0E,0x0E, 0x1C,0x11,0x0E, 0x1C,0x15,0x0E, 0x1C,0x18,0x0E,
    0x1C,0x1C,0x0E, 0x18,0x1C,0x0E, 0x15,0x1C,0x0E, 0x11,0x1C,0x0E,
    0x0E,0x1C,0x0E, 0x0E,0x1C,0x11, 0x0E,0x1C,0x15, 0x0E,0x1C,0x18,
    0x0E,0x1C,0x1C, 0x0E,0x18,0x1C, 0x0E,0x15,0x1C, 0x0E,0x11,0x1C,
    // 152-175: Low saturation, medium value
    0x14,0x14,0x1C, 0x16,0x14,0x1C, 0x18,0x14,0x1C, 0x1A,0x14,0x1C,
    0x1C,0x14,0x1C, 0x1C,0x14,0x1A, 0x1C,0x14,0x18, 0x1C,0x14,0x16,
    0x1C,0x14,0x14, 0x1C,0x16,0x14, 0x1C,0x18,0x14, 0x1C,0x1A,0x14,
    0x1C,0x1C,0x14, 0x1A,0x1C,0x14, 0x18,0x1C,0x14, 0x16,0x1C,0x14,
    0x14,0x1C,0x14, 0x14,0x1C,0x16, 0x14,0x1C,0x18, 0x14,0x1C,0x1A,
    0x14,0x1C,0x1C, 0x14,0x1A,0x1C, 0x14,0x18,0x1C, 0x14,0x16,0x1C,
    // 176-199: Full saturation, low value
    0x00,0x00,0x10, 0x04,0x00,0x10, 0x08,0x00,0x10, 0x0C,0x00,0x10,
    0x10,0x00,0x10, 0x10,0x00,0x0C, 0x10,0x00,0x08, 0x10,0x00,0x04,
    0x10,0x00,0x00, 0x10,0x04,0x00, 0x10,0x08,0x00, 0x10,0x0C,0x00,
    0x10,0x10,0x00, 0x0C,0x10,0x00, 0x08,0x10,0x00, 0x04,0x10,0x00,
    0x00,0x10,0x00, 0x00,0x10,0x04, 0x00,0x10,0x08, 0x00,0x10,0x0C,
    0x00,0x10,0x10, 0x00,0x0C,0x10, 0x00,0x08,0x10, 0x00,0x04,0x10,
    // 200-223: Medium saturation, low value
    0x08,0x08,0x10, 0x0A,0x08,0x10, 0x0C,0x08,0x10, 0x0E,0x08,0x10,
    0x10,0x08,0x10, 0x10,0x08,0x0E, 0x10,0x08,0x0C, 0x10,0x08,0x0A,
    0x10,0x08,0x08, 0x10,0x0A,0x08, 0x10,0x0C,0x08, 0x10,0x0E,0x08,
    0x10,0x10,0x08, 0x0E,0x10,0x08, 0x0C,0x10,0x08, 0x0A,0x10,0x08,
    0x08,0x10,0x08, 0x08,0x10,0x0A, 0x08,0x10,0x0C, 0x08,0x10,0x0E,
    0x08,0x10,0x10, 0x08,0x0E,0x10, 0x08,0x0C,0x10, 0x08,0x0A,0x10,
    // 224-247: Low saturation, low value
    0x0B,0x0B,0x10, 0x0C,0x0B,0x10, 0x0D,0x0B,0x10, 0x0F,0x0B,0x10,
    0x10,0x0B,0x10, 0x10,0x0B,0x0F, 0x10,0x0B,0x0D, 0x10,0x0B,0x0C,
    0x10,0x0B,0x0B, 0x10,0x0C,0x0B, 0x10,0x0D,0x0B, 0x10,0x0F,0x0B,
    0x10,0x10,0x0B, 0x0F,0x10,0x0B, 0x0D,0x10,0x0B, 0x0C,0x10,0x0B,
    0x0B,0x10,0x0B, 0x0B,0x10,0x0C, 0x0B,0x10,0x0D, 0x0B,0x10,0x0F,
    0x0B,0x10,0x10, 0x0B,0x0F,0x10, 0x0B,0x0D,0x10, 0x0B,0x0C,0x10,
    // 248-255: Reserved (zero on real VGA)
    0x00,0x00,0x00, 0x00,0x00,0x00, 0x00,0x00,0x00, 0x00,0x00,0x00,
    0x00,0x00,0x00, 0x00,0x00,0x00, 0x00,0x00,0x00, 0x00,0x00,0x00,
];

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
    pub dac_state: u8,     // 0 = write mode, 3 = read mode (readable via 0x3C7)
    pub dac_mask: u8,      // PEL (pixel) mask register, port 0x3C6 (default 0xFF)
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

    /// Display-latched Start Address (byte offset after byte/word scaling).
    /// Real CRTCs sample the Start Address register at vertical retrace,
    /// not on every write — so games that page-flip rapidly mid-frame don't
    /// produce tearing. The renderer reads this value; `latch_start_address`
    /// copies `crtc_regs[0x0C]/[0x0D]` here when the game polls the retrace
    /// status bit (port 0x3DA bit 3 active), matching hardware semantics.
    pub latched_start_addr: usize,

    /// Set whenever VRAM, palette, or any VGA state that would change the
    /// rendered image is touched. The main loop uses this to skip the
    /// 640x400x3 render pass on frames where nothing moved.
    pub dirty: bool,
}

impl VgaCard {
    pub fn new() -> Self {
        // Power-on palette matches the standard IBM VGA 256-color default,
        // which is what real BIOS loads when the card is initialized. Mode
        // switches that target mode 13h reload the same table via set_video_mode.
        let palette: Vec<u8> = VGA_DEFAULT_PALETTE.to_vec();

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
            dac_state: 0,
            dac_mask: 0xFF,
            misc_output_reg: 0x67, // Text Mode (Color + RAM Enable)
            retrace_counter: 0,
            palette,
            vram_graphics: vec![0; 256 * 1024], // 256KB (4 Planes x 64KB)
            vram_text: vec![0; 32 * 1024],      // 32KB (B8000-BFFFF)
            latches: Cell::new([0; 4]),
            attribute_index: 0,
            attribute_regs: [0; 21],
            attribute_flip_flop: false,
            latched_start_addr: 0,
            dirty: true,
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

        if is_color && is_256_color && chain4 {
            return Some(super::VideoMode::Graphics320x200);
        }

        None
    }

    pub fn read_graphics(&self, offset: usize) -> u8 {
        // Mode 13h Check (Chain 4)
        let seq_mem_mode = self.sequencer_regs[0x04];
        let chain4 = (seq_mem_mode & 0x08) != 0;
        // Sequencer Memory Mode bit 2 is "Odd/Even Disable": 1 = sequential
        // (the standard setup for graphics modes), 0 = odd/even mapping
        // (text modes). Bit 1 is Extended Memory — a totally different thing.
        // Our earlier code checked bit 1, which silently put mode 0Dh into
        // odd/even mode (because 256 KB VRAM is enabled) and halved every
        // plane offset, tiling each drawn scanline horizontally.
        let odd_even = (seq_mem_mode & 0x04) == 0;

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
        // Sequencer Memory Mode bit 2 is "Odd/Even Disable": 1 = sequential
        // (the standard setup for graphics modes), 0 = odd/even mapping
        // (text modes). Bit 1 is Extended Memory — a totally different thing.
        // Our earlier code checked bit 1, which silently put mode 0Dh into
        // odd/even mode (because 256 KB VRAM is enabled) and halved every
        // plane offset, tiling each drawn scanline horizontally.
        let odd_even = (seq_mem_mode & 0x04) == 0;

        let plane_offset = if chain4 {
            offset >> 2
        } else if odd_even {
            offset >> 1
        } else {
            offset
        };

        let mut planes_to_write = if chain4 {
            1u8 << (offset & 3)
        } else {
            self.sequencer_regs[0x02] & 0x0F
        };

        if odd_even && !chain4 {
            if (offset & 1) == 0 {
                planes_to_write &= 0x05;
            } else {
                planes_to_write &= 0x0A;
            }
        }

        let mode_reg = self.graphics_regs[0x05];
        let write_mode = mode_reg & 0x03;
        let set_reset = self.graphics_regs[0x00] & 0x0F;
        let enable_sr = self.graphics_regs[0x01] & 0x0F;
        let data_rotate = self.graphics_regs[0x03];
        let rotate_count = data_rotate & 0x07;
        let logical_op = (data_rotate >> 3) & 0x03;
        let bit_mask = self.graphics_regs[0x08];
        let latches = self.latches.get();

        let apply_op = |data: u8, latch: u8| -> u8 {
            match logical_op {
                1 => data & latch,
                2 => data | latch,
                3 => data ^ latch,
                _ => data,
            }
        };

        let mut per_plane = [0u8; 4];
        match write_mode {
            0 => {
                let rotated = value.rotate_right(rotate_count as u32);
                for p in 0..4 {
                    let data = if (enable_sr >> p) & 1 == 1 {
                        if (set_reset >> p) & 1 == 1 { 0xFF } else { 0x00 }
                    } else {
                        rotated
                    };
                    let after = apply_op(data, latches[p]);
                    per_plane[p] = (after & bit_mask) | (latches[p] & !bit_mask);
                }
            }
            1 => {
                for p in 0..4 {
                    per_plane[p] = latches[p];
                }
            }
            2 => {
                for p in 0..4 {
                    let data = if (value >> p) & 1 == 1 { 0xFF } else { 0x00 };
                    let after = apply_op(data, latches[p]);
                    per_plane[p] = (after & bit_mask) | (latches[p] & !bit_mask);
                }
            }
            _ => {
                // Write Mode 3: rotated CPU data is AND'd with bit mask to
                // produce the effective mask; set/reset supplies the value.
                let rotated = value.rotate_right(rotate_count as u32);
                let effective_mask = rotated & bit_mask;
                for p in 0..4 {
                    let data = if (set_reset >> p) & 1 == 1 { 0xFF } else { 0x00 };
                    let after = apply_op(data, latches[p]);
                    per_plane[p] = (after & effective_mask) | (latches[p] & !effective_mask);
                }
            }
        }

        for p in 0..4 {
            if (planes_to_write & (1 << p)) != 0 {
                let idx = (p * 65536) + plane_offset;
                if idx < self.vram_graphics.len() {
                    self.vram_graphics[idx] = per_plane[p];
                }
            }
        }
        self.dirty = true;
    }

    /// Load the 64-color EGA palette into DAC entries 0..63 for the 16-color
    /// planar modes. Each EGA color byte has the form rgbRGB (lower-case = 2/3
    /// intensity, upper-case = full intensity), and this is what attribute
    /// palette registers reference in standard EGA/VGA modes.
    fn load_ega_palette(&mut self) {
        for c in 0usize..64 {
            let secondary_r = ((c >> 5) & 1) as u8 * 0x15;
            let secondary_g = ((c >> 4) & 1) as u8 * 0x15;
            let secondary_b = ((c >> 3) & 1) as u8 * 0x15;
            let primary_r = ((c >> 2) & 1) as u8 * 0x2A;
            let primary_g = ((c >> 1) & 1) as u8 * 0x2A;
            let primary_b = (c & 1) as u8 * 0x2A;
            self.palette[c * 3] = primary_r + secondary_r;
            self.palette[c * 3 + 1] = primary_g + secondary_g;
            self.palette[c * 3 + 2] = primary_b + secondary_b;
        }
    }

    /// Apply the attribute-controller defaults for 16-color planar modes:
    /// the 16 palette registers point into the 64-color EGA palette, with
    /// the historic quirk that entry 6 remaps to brown (0x14) and the
    /// "bright" colors 8..15 use secondary + primary bits (0x38..0x3F).
    fn load_ega_attribute_defaults(&mut self) {
        const EGA_DEFAULTS: [u8; 16] = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x14, 0x07,
            0x38, 0x39, 0x3A, 0x3B, 0x3C, 0x3D, 0x3E, 0x3F,
        ];
        for (i, &v) in EGA_DEFAULTS.iter().enumerate() {
            self.attribute_regs[i] = v;
        }
        self.attribute_regs[0x10] = 0x01; // Mode Control: graphics, 6-bit indices
        self.attribute_regs[0x11] = 0x00; // Overscan
        self.attribute_regs[0x12] = 0x0F; // Color Plane Enable (all 4 planes)
        self.attribute_regs[0x13] = 0x00; // Horizontal Pixel Panning
        self.attribute_regs[0x14] = 0x00; // Color Select
    }

    /// Snapshot CRTC Start Address High/Low into the display-latched byte
    /// offset. Games overwhelmingly write Start Address as a direct byte
    /// offset (matching what they use for ES:DI when drawing the page),
    /// regardless of the CRTC 0x17 word/byte mode bit — real hardware's
    /// word-mode scaling is a subtle address-bit permutation that most
    /// DOS games didn't know or care about. Treat it as a plain byte
    /// offset so game page-flipping works with byte-mode semantics.
    pub fn latch_start_address(&mut self) {
        let hi = self.crtc_regs[0x0C] as usize;
        let lo = self.crtc_regs[0x0D] as usize;
        let new_addr = (hi << 8) | lo;
        if new_addr != self.latched_start_addr {
            self.latched_start_addr = new_addr;
            self.dirty = true;
        }
    }

    pub fn set_video_mode(&mut self, mode: super::VideoMode) {
        self.dirty = true;
        match mode {
            super::VideoMode::Ega320x200
            | super::VideoMode::Ega640x200
            | super::VideoMode::Ega640x350
            | super::VideoMode::Vga640x480 => {
                // Misc Output: pick CRT timing roughly matching the mode.
                self.misc_output_reg = match mode {
                    super::VideoMode::Vga640x480 => 0xE3,
                    super::VideoMode::Ega640x350 => 0xA7,
                    _ => 0x63,
                };

                // Sequencer: planar layout (no chain-4, no odd/even).
                self.sequencer_regs[0] = 0x03;
                self.sequencer_regs[1] = 0x01;
                self.sequencer_regs[2] = 0x0F; // Map Mask = all planes writable
                self.sequencer_regs[3] = 0x00;
                self.sequencer_regs[4] = 0x06; // Extended memory + sequential

                // Graphics Controller: write mode 0, 4-plane.
                self.graphics_regs[0] = 0x00; // Set/Reset
                self.graphics_regs[1] = 0x00; // Enable Set/Reset
                self.graphics_regs[2] = 0x00; // Color Compare
                self.graphics_regs[3] = 0x00; // Data Rotate
                self.graphics_regs[4] = 0x00; // Read Map Select
                self.graphics_regs[5] = 0x00; // Mode Register (write mode 0)
                self.graphics_regs[6] = 0x05; // Graphics mode, map A0000
                self.graphics_regs[7] = 0x0F; // Color Don't Care
                self.graphics_regs[8] = 0xFF; // Bit Mask

                // CRTC defaults. The real IBM EGA/VGA BIOS loads a full
                // 25-register table when setting mode 0Dh, but only a few
                // actually affect our emulation. The critical ones:
                //   0x13 = Offset (words-per-row, 0x14 = 40-byte stride)
                //   0x17 = Mode Control (0xA3 = word addressing, which is
                //          what games assume when they page-flip via
                //          Start Address High)
                //   0x09 = Max Scan Line (0x41 = double-scan on 200-line modes)
                self.crtc_regs[0x09] = match mode {
                    super::VideoMode::Ega320x200 | super::VideoMode::Ega640x200 => 0x41,
                    _ => 0x00,
                };
                self.crtc_regs[0x0C] = 0x00; // Start Address High
                self.crtc_regs[0x0D] = 0x00; // Start Address Low
                self.crtc_regs[0x13] = match mode {
                    super::VideoMode::Ega320x200 => 0x14,
                    super::VideoMode::Ega640x200
                    | super::VideoMode::Ega640x350
                    | super::VideoMode::Vga640x480 => 0x28,
                    _ => 0x14,
                };
                self.crtc_regs[0x17] = 0xA3;

                self.load_ega_palette();
                self.load_ega_attribute_defaults();
                self.dac_mask = 0xFF;

                // Clear graphics VRAM so we don't see stale pixels.
                for b in self.vram_graphics.iter_mut() {
                    *b = 0;
                }
            }
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

                // Reload the standard 256-color DAC palette, matching what the
                // real IBM VGA BIOS does when setting mode 13h. Programs that
                // customize only part of the palette rely on sensible defaults
                // being present for the rest.
                self.palette.copy_from_slice(&VGA_DEFAULT_PALETTE);
                self.dac_mask = 0xFF;
            }
            _ => {
                // Text Mode defaults?
            }
        }
    }
}

impl Device for VgaCard {
    fn ports(&self) -> &'static [u16] {
        // Static slice so the bus can check port ownership without allocating
        // a Vec on every I/O (palette updates do >1000 port writes each).
        const PORTS: &[u16] = &[
            0x3C2, // Misc Output (Write) / Input Status 0 (Read)
            0x3C3, // Video Enable
            0x3C4, 0x3C5, // Sequencer
            0x3CE, 0x3CF, // Graphics
            0x3CC, // Misc Output Read
            0x3D4, 0x3D5, // CRTC (color addressing, MISC bit 0 = 1)
            0x3B4, 0x3B5, // CRTC (mono addressing, MISC bit 0 = 0)
            0x3C6, 0x3C7, 0x3C8, 0x3C9, // DAC
            0x3DA, // Status (color)
            0x3BA, // Status (mono) — alias used for retrace polling and detection
        ];
        PORTS
    }

    fn io_read(&mut self, port: u16) -> u8 {
        // Mono-CRTC aliases: 3B4/3B5 == 3D4/3D5 and 3BA == 3DA. Games probe
        // these for monitor type detection; transparently redirect.
        let port = match port {
            0x3B4 => 0x3D4,
            0x3B5 => 0x3D5,
            0x3BA => 0x3DA,
            other => other,
        };
        match port {
            0x3DA => {
                // Input Status #1
                // Reading 3DA resets the Attribute Controller Flip-Flop to Address Mode
                self.attribute_flip_flop = false;

                // Toggle VRetrace (Bit 3) and Display Enable (Bit 0)
                self.retrace_counter = self.retrace_counter.wrapping_add(1);

                // Toggle active/retrace every 8 reads to simulate timing
                if (self.retrace_counter & 8) != 0 {
                    // Entering retrace — real CRTCs latch Start Address here,
                    // then hold it stable for the whole frame. Mirror that:
                    // games can write CRTC 0x0C/0x0D arbitrarily many times
                    // before the next vretrace and only the last value sticks.
                    self.latch_start_address();
                    0x09 // Retrace Active (Bit 3) + Display Disabled (Bit 0)
                } else {
                    0x00 // Display Active, No Retrace
                }
            }
            0x3C2 => {
                // Input Status #0
                // Bit 7: IRQ Pending (0=Clear)
                // Bit 4: Switch Sense. Determined by Misc Output (Write) bits 2-3.
                // Switches for "EGA Color 80x25" are typically 0110 (binary) = 6.
                // SW1=Off(1), SW2=Off(1), SW3=On(0), SW4=On(0)? Wait.
                // Common setting: 0110 aka 6.
                // Let's emulate bits 2-3 of Write directing which bit of 0110 to read.
                let select = (self.misc_output_reg >> 2) & 0x03;
                let switches = 0b0110; // EGA Color 80x25? Or 0b1001?
                // RBIL:
                // 0110 = Color 80x25
                let switch_val = (switches >> select) & 0x01;

                switch_val << 4 // Return switch sense in Bit 4
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
            0x3CC => self.misc_output_reg,
            0x3C5 => {
                let val = if (self.sequencer_index as usize) < self.sequencer_regs.len() {
                    self.sequencer_regs[self.sequencer_index as usize]
                } else {
                    0
                };
                val
            }
            0x3CF => {
                let val = if (self.graphics_index as usize) < self.graphics_regs.len() {
                    self.graphics_regs[self.graphics_index as usize]
                } else {
                    0
                };
                val
            }
            0x3D5 => {
                let val = if (self.crtc_index as usize) < self.crtc_regs.len() {
                    self.crtc_regs[self.crtc_index as usize]
                } else {
                    0
                };
                val
            }
            0x3C6 => self.dac_mask,
            0x3C7 => self.dac_state,
            0x3C8 => self.dac_write_index,
            0x3C9 => {
                let index = (self.dac_read_index as usize) * 3 + (self.dac_step as usize);
                let val = if index < self.palette.len() {
                    self.palette[index]
                } else {
                    0
                };
                self.dac_step += 1;
                if self.dac_step == 3 {
                    self.dac_step = 0;
                    self.dac_read_index = self.dac_read_index.wrapping_add(1);
                }
                val
            }
            _ => 0xFF,
        }
    }

    fn io_write(&mut self, port: u16, value: u8) {
        let port = match port {
            0x3B4 => 0x3D4,
            0x3B5 => 0x3D5,
            0x3BA => 0x3DA,
            other => other,
        };
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
                        self.dirty = true;
                    }
                    self.attribute_flip_flop = false; // Switch back to Address
                }
            }
            0x3C2 => {
                self.misc_output_reg = value;
                self.dirty = true;
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
                    self.dirty = true;
                }
            }
            0x3CE => self.graphics_index = value,
            0x3CF => {
                if (self.graphics_index as usize) < self.graphics_regs.len() {
                    let mut val = value;
                    // Mask Read Map Select to 2 bits
                    // if self.graphics_index == 0x04 {
                    //    val &= 0x03;
                    // }
                    // Mask Mode Register (Index 5)
                    if self.graphics_index == 0x05 {
                        val &= 0x73;
                    }

                    self.graphics_regs[self.graphics_index as usize] = val;
                    // println!("[VGA] Gfx Reg {:02X} = {:02X}", self.graphics_index, val);
                    self.dirty = true;
                }
            }
            0x3D4 => self.crtc_index = value,
            0x3D5 => {
                if (self.crtc_index as usize) < self.crtc_regs.len() {
                    self.crtc_regs[self.crtc_index as usize] = value;
                    // Start Address registers (0x0C/0x0D) update a pending
                    // value that the CRTC only picks up at vretrace.
                    // Writing them does NOT trigger a re-render — that
                    // would cause flicker when games rapid-flip buffers
                    // mid-frame. The retrace read in `io_read` latches.
                    if self.crtc_index != 0x0C && self.crtc_index != 0x0D {
                        self.dirty = true;
                    }
                }
            }
            0x3C6 => {
                self.dac_mask = value;
                self.dirty = true;
            }
            0x3C7 => {
                // Set DAC Read Index. Subsequent reads from 0x3C9 return R,G,B triplets.
                self.dac_read_index = value;
                self.dac_step = 0;
                self.dac_state = 3; // Read mode
            }
            0x3C8 => {
                self.dac_write_index = value;
                self.dac_step = 0;
                self.dac_state = 0; // Write mode
            }
            0x3C9 => {
                let index = (self.dac_write_index as usize) * 3 + (self.dac_step as usize);
                if index < self.palette.len() {
                    self.palette[index] = value & 0x3F;
                    self.dirty = true;
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
