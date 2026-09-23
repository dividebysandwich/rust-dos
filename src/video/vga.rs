use super::crt::CrtTiming;
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
    /// The DAC takes 8 bits per color instead of 6 (VBE function 08h).
    pub dac_8bit: bool,
    pub misc_output_reg: u8,
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
    /// copies `crtc_regs[0x0C]/[0x0D]` here when a vertical retrace begins
    /// in emulated time (`Bus::sync_display`).
    pub latched_start_addr: usize,

    /// The display timing the registers describe, computed when first
    /// needed after a timing register changed.
    timing_cache: Option<CrtTiming>,
    /// The last timing the registers described sensibly. Programs that
    /// reprogram the CRTC pass through nonsense on the way; the monitor
    /// keeps the old picture meanwhile.
    good_timing: CrtTiming,
    /// A timing that overrides the registers, for modes whose registers
    /// the VGA doesn't model (VESA).
    fixed_timing: Option<CrtTiming>,
    /// Vertical retraces counted so far, to notice when another began.
    retraces: u64,
    /// The timing changed: restart the retrace count.
    rebase: bool,

    /// Set whenever VRAM, palette, or any VGA state that would change the
    /// rendered image is touched. The main loop uses this to skip the
    /// 640x400x3 render pass on frames where nothing moved.
    pub dirty: bool,

    /// Inclusive lower / exclusive upper screen-row bounds of the region
    /// that needs re-rendering this frame (u32::MAX: to the bottom). When
    /// `dirty_y_min >= dirty_y_max` the dirty region is empty. Sites that
    /// can't easily compute an affected row range (palette writes, mode
    /// changes, planar VRAM writes) widen this to the full screen via
    /// `mark_dirty_full`. The text-VRAM path in `Bus::write_8` narrows it
    /// to the single character row that was touched, which is what makes
    /// shell scrolling and incremental terminal output cheap.
    pub dirty_y_min: u32,
    pub dirty_y_max: u32,
}

impl VgaCard {
    pub fn new() -> Self {
        let mut vga = Self {
            sequencer_index: 0,
            sequencer_regs: [0; 5],
            graphics_index: 0,
            graphics_regs: [0; 9],
            crtc_index: 0,
            crtc_regs: [0; 25],
            dac_write_index: 0,
            dac_read_index: 0,
            dac_step: 0,
            dac_state: 0,
            dac_mask: 0xFF,
            dac_8bit: false,
            misc_output_reg: 0,
            palette: VGA_DEFAULT_PALETTE.to_vec(),
            vram_graphics: vec![0; 256 * 1024], // 256KB (4 Planes x 64KB)
            vram_text: vec![0; 32 * 1024],      // 32KB (B8000-BFFFF)
            latches: Cell::new([0; 4]),
            attribute_index: 0,
            attribute_regs: [0; 21],
            attribute_flip_flop: false,
            latched_start_addr: 0,
            timing_cache: None,
            good_timing: CrtTiming::VGA_400,
            fixed_timing: None,
            retraces: 0,
            rebase: true,
            dirty: true,
            dirty_y_min: 0,
            dirty_y_max: u32::MAX,
        };
        // The BIOS starts in 80x25 color text mode.
        vga.set_video_mode(super::VideoMode::Text80x25Color);
        vga
    }

    /// Mark the entire screen as needing re-rendering. Use for state changes
    /// (palette, mode, attribute regs, latched start address) where computing
    /// an affected row range would be more work than just repainting.
    #[inline]
    pub fn mark_dirty_full(&mut self) {
        self.dirty = true;
        self.dirty_y_min = 0;
        self.dirty_y_max = u32::MAX;
    }

    /// Widen the dirty range to include `[y_start, y_end)` (screen rows).
    /// Caller is responsible for clipping to SCREEN_HEIGHT.
    #[inline]
    pub fn mark_dirty_rows(&mut self, y_start: u32, y_end: u32) {
        if y_end <= y_start {
            return;
        }
        self.dirty = true;
        if self.dirty_y_min > y_start {
            self.dirty_y_min = y_start;
        }
        if self.dirty_y_max < y_end {
            self.dirty_y_max = y_end;
        }
    }

    /// Reset after rendering. The empty range encodes "nothing dirty" as
    /// `dirty_y_min == u32::MAX, dirty_y_max == 0` so the next
    /// `mark_dirty_rows` widens correctly from a clean slate.
    #[inline]
    pub fn clear_dirty(&mut self) {
        self.dirty = false;
        self.dirty_y_min = u32::MAX;
        self.dirty_y_max = 0;
    }

    pub fn get_rgb(&self, index: u8) -> (u8, u8, u8) {
        let base = (index as usize) * 3;
        if base + 2 < self.palette.len() {
            // A 6-bit DAC's values (0-63) shifted to 8 bits: 0-252.
            let shift = if self.dac_8bit { 0 } else { 2 };
            let r = self.palette[base] << shift;
            let g = self.palette[base + 1] << shift;
            let b = self.palette[base + 2] << shift;
            (r, g, b)
        } else {
            (0, 0, 0)
        }
    }

    /// The bits of a DAC color value the DAC keeps.
    pub fn dac_value_mask(&self) -> u8 {
        if self.dac_8bit { 0xFF } else { 0x3F }
    }

    /// The mode the registers were programmed for when that differs from
    /// what the BIOS set: 256 colors (Graphics Mode register bit 6) on a
    /// color CRTC is mode 13h or one of its unchained "mode X" variants,
    /// whichever mode the program started from. Some games start from mode
    /// 12h for its 480-line timing and switch to 256 colors themselves.
    pub fn check_video_mode(&self) -> Option<super::VideoMode> {
        let is_256_color = self.graphics_regs[0x05] & 0x40 != 0;
        let is_color = self.misc_output_reg & 0x01 != 0;
        (is_color && is_256_color).then_some(super::VideoMode::Graphics320x200)
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
        self.mark_dirty_full();
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
            self.mark_dirty_full();
        }
    }

    /// The first of the `rows` a mode displays that the split screen shows:
    /// past the scanline in the CRTC Line Compare register, the display
    /// starts again from address 0, as games do for a status bar below a
    /// scrolling playfield. Each row is `(Max Scan Line & 1Fh) + 1`
    /// scanlines, twice that when bit 7 doubles them.
    pub fn split_row(&self) -> usize {
        let crtc = &self.crtc_regs;
        let line_compare = crtc[0x18] as usize
            | (crtc[0x07] as usize & 0x10) << 4
            | (crtc[0x09] as usize & 0x40) << 3;
        let mut scanlines = (crtc[0x09] as usize & 0x1F) + 1;
        if crtc[0x09] & 0x80 != 0 {
            scanlines *= 2;
        }
        line_compare / scanlines + 1
    }

    /// Width and height in pixels of a graphics picture: 8 pixels per
    /// character clock up to Horizontal Display End (4 in 256-color modes),
    /// and the displayed scanlines over the scanlines per row (Maximum Scan
    /// Line, doubled by its bit 7). 320x200 in mode 13h, 640x480 in mode
    /// 12h, and whatever variants games program themselves: 320x240 or
    /// 360x480 in 256 colors, 640x240 in 16.
    pub fn graphics_size(&self) -> (usize, usize) {
        let pixels_per_char = if self.graphics_regs[0x05] & 0x40 != 0 { 4 } else { 8 };
        let width = (self.crtc_regs[0x01] as usize + 1) * pixels_per_char;
        let mut scanlines = (self.crtc_regs[0x09] as usize & 0x1F) + 1;
        if self.crtc_regs[0x09] & 0x80 != 0 {
            scanlines *= 2;
        }
        let rows = self.peek_timing().display as usize / scanlines;
        (width.clamp(16, 1024), rows.clamp(1, 1024))
    }

    /// Pixels the display is shifted left by (Attribute register 13h, Horizontal
    /// PEL Panning): in 256-color modes in steps of half a pixel, of which
    /// only whole pixels show.
    pub fn pixel_panning(&self) -> usize {
        let value = self.attribute_regs[0x13] as usize & 0x0F;
        if self.attribute_regs[0x10] & 0x40 != 0 {
            (value & 0x07) >> 1
        } else if value < 8 {
            value
        } else {
            0
        }
    }

    /// Program the registers the way the BIOS does for a mode (see
    /// `modes.rs`), with its palette.
    pub fn set_video_mode(&mut self, mode: super::VideoMode) {
        self.mark_dirty_full();
        self.latched_start_addr = 0;
        let regs = super::modes::mode_regs(mode);
        self.misc_output_reg = regs.misc;
        self.sequencer_regs[0] = 0x03;
        self.sequencer_regs[1..].copy_from_slice(&regs.seq);
        self.crtc_regs = regs.crtc;
        self.graphics_regs = regs.gc;
        self.attribute_regs = regs.attr;
        self.fixed_timing = None;
        self.timing_cache = None;
        self.dac_mask = 0xFF;
        self.dac_8bit = false;
        if mode.is_planar() {
            self.load_ega_palette();
        } else {
            // The standard 256-color palette; its first 16 entries are the
            // text and CGA colors. Programs that customize only part of the
            // palette rely on sensible defaults for the rest.
            self.palette.copy_from_slice(&VGA_DEFAULT_PALETTE);
        }
    }

    /// Give the display a timing the registers don't describe (a VESA
    /// mode's), or with None go back to the registers'.
    pub fn set_fixed_timing(&mut self, timing: Option<CrtTiming>) {
        self.fixed_timing = timing;
        self.timing_cache = None;
    }

    /// Invalidate the display timing after a register it depends on changed.
    fn timing_changed(&mut self) {
        self.timing_cache = None;
    }

    /// The display timing: the one the registers describe, or the last
    /// sensible one while they describe none.
    pub fn timing(&mut self) -> CrtTiming {
        if let Some(timing) = self.timing_cache {
            return timing;
        }
        let timing = self.fixed_timing.unwrap_or_else(|| {
            CrtTiming::from_registers(
                self.misc_output_reg,
                self.sequencer_regs[1],
                &self.crtc_regs,
            )
            .unwrap_or(self.good_timing)
        });
        if timing != self.good_timing {
            self.good_timing = timing;
            self.rebase = true;
        }
        self.timing_cache = Some(timing);
        timing
    }

    /// The display timing, without caching it (for status displays).
    pub fn peek_timing(&self) -> CrtTiming {
        self.timing_cache.or(self.fixed_timing).unwrap_or_else(|| {
            CrtTiming::from_registers(
                self.misc_output_reg,
                self.sequencer_regs[1],
                &self.crtc_regs,
            )
            .unwrap_or(self.good_timing)
        })
    }

    /// Whether a vertical retrace began since the last call, at emulated
    /// time `t_ns`: that is when the CRTC latches the Start Address.
    pub fn retrace_began(&mut self, t_ns: u64) -> bool {
        let retraces = self.timing().retraces(t_ns);
        if self.rebase {
            self.rebase = false;
            self.retraces = retraces;
            return true;
        }
        let began = retraces != self.retraces;
        self.retraces = retraces;
        began
    }
}

impl Device for VgaCard {
    fn ports(&self) -> &'static [u16] {
        // Static slice so the bus can check port ownership without allocating
        // a Vec on every I/O (palette updates do >1000 port writes each).
        const PORTS: &[u16] = &[
            0x3C0, 0x3C1, // Attribute Controller
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
        // Input Status 1 (3DAh/3BAh) depends on the time; the bus answers it.
        let port = match port {
            0x3B4 => 0x3D4,
            0x3B5 => 0x3D5,
            other => other,
        };
        match port {
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
                        self.mark_dirty_full();
                    }
                    self.attribute_flip_flop = false; // Switch back to Address
                }
            }
            0x3C2 => {
                self.misc_output_reg = value;
                self.timing_changed();
                self.mark_dirty_full();
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
                    if self.sequencer_index == 0x01 {
                        self.timing_changed();
                    }
                    self.mark_dirty_full();
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
                    self.mark_dirty_full();
                }
            }
            0x3D4 => self.crtc_index = value,
            0x3D5 => {
                let index = self.crtc_index as usize;
                if index < self.crtc_regs.len() {
                    // Vertical Retrace End bit 7 write-protects the timing
                    // registers 00h-07h, all but the Line Compare bit of
                    // the Overflow register.
                    let value = match index {
                        0x00..=0x06 if self.crtc_regs[0x11] & 0x80 != 0 => return,
                        0x07 if self.crtc_regs[0x11] & 0x80 != 0 => {
                            (self.crtc_regs[0x07] & !0x10) | (value & 0x10)
                        }
                        _ => value,
                    };
                    self.crtc_regs[index] = value;
                    if matches!(index, 0x00..=0x07 | 0x10..=0x12 | 0x17) {
                        self.timing_changed();
                    }
                    // Start Address registers (0x0C/0x0D) update a pending
                    // value that the CRTC only picks up at vretrace.
                    // Writing them does NOT trigger a re-render — that
                    // would cause flicker when games rapid-flip buffers
                    // mid-frame.
                    if index != 0x0C && index != 0x0D {
                        self.mark_dirty_full();
                    }
                }
            }
            0x3C6 => {
                self.dac_mask = value;
                self.mark_dirty_full();
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
                    self.palette[index] = value & self.dac_value_mask();
                    self.mark_dirty_full();
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
