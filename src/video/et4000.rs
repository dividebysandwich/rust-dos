//! The Tseng Labs ET4000AX of `machine=svga_et4000`, with 1 MB and a
//! Sierra SC11487 HiColor DAC: the chip's extended registers behind its
//! KEY, the segment select register (3CDh) that banks the window at
//! A0000h, the display start and overflow bits past the VGA's, and the
//! DAC's hidden command register that turns on 15 and 16-bit colour.
//!
//! The ET4000's memory is the VGA's planes, 256 KB each: a chained mode's
//! byte L is in plane L & 3 at L >> 2, as the chip lays it out, so the
//! Super VGA modes keep the VGA's latches and write modes. The registers
//! follow DOSBox-X's vga_tseng.cpp and 86Box's vid_et4000.c.

use super::vga::VgaCard;

/// The chip's own registers.
#[derive(Clone, Debug)]
pub struct Et4000 {
    /// The Hercules Compatibility register (3BFh), the first half of the
    /// KEY.
    pub herc_compat: u8,
    /// Whether the KEY is set: 03h at 3BFh, then A0h at the Mode Control
    /// register (3D8h, or 3B8h with the CRTC at 3B4h). The extended
    /// registers only take writes and read back with it.
    pub keyed: bool,
    /// Segment Select (3CDh): the 64 KB bank writes to the window at
    /// A0000h go to in bits 0-3, reads come from in bits 4-7.
    pub segment: u8,
    /// CRTC registers 30h-3Fh, by index & 0Fh.
    crtc: [u8; 0x10],
    /// Sequencer registers 06h and 07h.
    seq: [u8; 2],
    /// Attribute controller registers 16h and 17h.
    atc: [u8; 2],
    /// Reads of the DAC's pixel mask (3C6h) in a row: the fourth one on
    /// reads, and a write after it goes to, the command register.
    dac_reads: u8,
    /// The Sierra DAC's command register: bit 7 HiColor, bit 6 its 16-bit
    /// 5:6:5 mode, bit 5 two clocks a pixel.
    pub dac_command: u8,
    /// The VESA mode set through INT 10h AX=4F02h, 0 after another mode.
    pub vbe_mode: u16,
}

impl Default for Et4000 {
    fn default() -> Self {
        Self::new()
    }
}

/// CR37, Video System Configuration 2: a 32-bit bus to 256K-deep chips,
/// 1 MB (DOSBox-X's value).
pub const MEMORY_CONFIG: u8 = 0x0F;

/// The clocks the clock select bits pick (MHz × 1000), as DOSBox-X has
/// them for the ET4000's usual ICS2494-type synthesizer.
const CLOCKS_KHZ: [u64; 16] = [
    25_175, 28_322, 32_515, 40_000, 36_000, 44_900, 31_500, 37_500, 50_000, 56_500, 64_900, 71_900, 79_900, 89_600, 62_800,
    74_800,
];

/// What a write to an extended CRTC register changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    None,
    /// The display's timing or shape.
    Timing,
    /// The display start (CR33).
    Start,
}

impl Et4000 {
    pub fn new() -> Self {
        let mut crtc = [0; 0x10];
        crtc[0x07] = MEMORY_CONFIG;
        Self {
            // Unlocked from power-on, as DOSBox-X has it: programs that
            // skip the KEY still find the chip.
            herc_compat: 0x03,
            keyed: true,
            segment: 0,
            crtc,
            seq: [0; 2],
            atc: [0; 2],
            dac_reads: 0,
            dac_command: 0,
            vbe_mode: 0,
        }
    }

    /// The registers a BIOS mode set leaves: no bank, no extended display
    /// bits, the DAC back to 256 colours (DOSBox-X's `FinishSetMode_ET4K`).
    pub fn reset_for_mode(&mut self) {
        self.segment = 0;
        self.crtc = [0; 0x10];
        self.crtc[0x07] = MEMORY_CONFIG;
        self.seq = [0; 2];
        self.atc = [0; 2];
        self.dac_command = 0;
        self.dac_reads = 0;
        self.vbe_mode = 0;
    }

    /// A write to the Mode Control register (3D8h/3B8h): A0h after 03h at
    /// 3BFh sets the KEY, anything else clears it.
    pub fn write_mode_control(&mut self, value: u8) {
        self.keyed = value == 0xA0 && self.herc_compat == 0x03;
    }

    /// CRTC register `index` (30h-3Fh), 0 for the others and, but for
    /// CR33, while the KEY isn't set.
    pub fn read_crtc(&self, index: u8) -> u8 {
        match index {
            0x33 => self.crtc[3],
            0x30..=0x3F if self.keyed => self.crtc[(index & 0x0F) as usize],
            _ => 0,
        }
    }

    /// CRTC register `index` (30h-3Fh) as it is, KEY or not, for status
    /// displays.
    pub fn crtc(&self, index: u8) -> u8 {
        self.crtc[(index & 0x0F) as usize]
    }

    /// Write CRTC register `index`. `protect` is CR11 bit 7, which also
    /// write-protects the vertical overflow bits in CR35.
    pub fn write_crtc(&mut self, index: u8, value: u8, protect: bool) -> Effect {
        match index {
            // Extended Start Address: bits 0-1 the display start's bits
            // 16-17, bits 2-3 the cursor's. The upper bits don't exist,
            // which is how programs tell an ET4000 (VGADOC).
            0x33 => {
                self.crtc[3] = value & 0x0F;
                Effect::Start
            }
            _ if !self.keyed => Effect::None,
            0x35 if protect => Effect::None,
            // CR37 is the board's memory straps.
            0x37 => Effect::None,
            0x30..=0x3F => {
                self.crtc[(index & 0x0F) as usize] = value;
                match index {
                    0x31 | 0x34 | 0x35 | 0x3F => Effect::Timing,
                    _ => Effect::None,
                }
            }
            _ => Effect::None,
        }
    }

    /// Sequencer register 06h or 07h.
    pub fn read_seq(&self, index: u8) -> u8 {
        match index {
            0x06 | 0x07 if self.keyed => self.seq[(index - 6) as usize],
            _ => 0,
        }
    }

    pub fn write_seq(&mut self, index: u8, value: u8) {
        if self.keyed && matches!(index, 0x06 | 0x07) {
            self.seq[(index - 6) as usize] = value;
        }
    }

    /// Attribute controller register 16h or 17h.
    pub fn read_atc(&self, index: u8) -> u8 {
        match index {
            0x16 | 0x17 if self.keyed => self.atc[(index - 0x16) as usize],
            _ => 0,
        }
    }

    pub fn write_atc(&mut self, index: u8, value: u8) {
        if self.keyed && matches!(index, 0x16 | 0x17) {
            self.atc[(index - 0x16) as usize] = value;
        }
    }

    /// The bank of the window at A0000h for a write or for a read.
    pub fn bank(&self, write: bool) -> usize {
        (if write { self.segment & 0x0F } else { self.segment >> 4 }) as usize
    }

    /// Whether 3CDh banks the window at A0000h: not in the linear system
    /// configuration (CR36 bit 4), nor with the window at B0000h or
    /// B8000h (Graphics Controller Miscellaneous bit 3).
    pub fn banks_on(&self, gr06: u8) -> bool {
        self.crtc[0x06] & 0x10 == 0 && gr06 & 0x08 == 0
    }

    /// Bits 16-17 of the display start (CR33 bits 0-1).
    pub fn start_high(&self) -> usize {
        (self.crtc[3] & 0x03) as usize
    }

    /// Set the display start's bits 16-17 (CR33 bits 0-1).
    pub fn set_start_high(&mut self, bits: usize) {
        self.crtc[3] = (self.crtc[3] & !0x03) | (bits & 0x03) as u8;
    }

    /// Set bit 8 of the CRTC Offset register (CR3F bit 7).
    pub fn set_offset_high(&mut self, on: bool) {
        self.crtc[0x0F] = (self.crtc[0x0F] & 0x7F) | if on { 0x80 } else { 0 };
    }

    /// Bit 8 of the CRTC Offset register (CR3F bit 7).
    pub fn offset_high(&self) -> usize {
        (self.crtc[0x0F] as usize & 0x80) << 1
    }

    /// The pixel clock in Hz: Miscellaneous Output bits 2-3, CR34 bit 1
    /// and CR31 bit 6 pick one of the 16.
    pub fn clock_hz(&self, misc: u8) -> u64 {
        let index = (misc >> 2) & 3 | (self.crtc[0x04] & 0x02) << 1 | (self.crtc[0x01] & 0x40) >> 3;
        CLOCKS_KHZ[index as usize] * 1000
    }

    /// Bit 8 of the Horizontal Total (CR3F bit 0).
    pub fn htotal_high(&self) -> u32 {
        (self.crtc[0x0F] as u32 & 0x01) << 8
    }

    /// Bit 10 of the vertical total, display end and retrace start (CR35
    /// bits 1, 2 and 3), and of the line compare (bit 4).
    pub fn vertical_high(&self) -> [u32; 4] {
        let cr35 = self.crtc[0x05] as u32;
        [cr35 >> 1 & 1, cr35 >> 2 & 1, cr35 >> 3 & 1, cr35 >> 4 & 1].map(|bit| bit << 10)
    }

    /// Whether each character clock is 16 dots instead of 8: the high
    /// colour modes' (ATC16 bit 5, 86Box's `hdisp <<= 1`).
    pub fn wide_chars(&self) -> bool {
        self.atc[0] & 0x20 != 0
    }

    /// Bits a pixel in a HiColor mode (15 or 16), or None: bit 7 or 5 of
    /// the command register, and bit 6 for 16 (86Box's SC11487).
    pub fn hicolor(&self) -> Option<u8> {
        match self.dac_command {
            c if c & 0xA0 == 0 => None,
            c if c & 0x40 != 0 => Some(16),
            _ => Some(15),
        }
    }

    /// A read of the DAC's pixel mask (3C6h), whose value is `mask`, as
    /// an SC11487 answers them (86Box's `sc1148x_ramdac_in`): the mask,
    /// then 0 three times, then the command register from the fifth read
    /// on, its bits 3-4 the mask's.
    pub fn dac_read_mask(&mut self, mask: u8) -> u8 {
        match self.dac_reads {
            0 => {
                self.dac_reads = 1;
                mask
            }
            1..=3 => {
                self.dac_reads += 1;
                0x00
            }
            _ => (self.dac_command & !0x18) | (mask & 0x18),
        }
    }

    /// A write to 3C6h: to the command register after four reads (true),
    /// but FFh, which goes nowhere; to the pixel mask otherwise. Bit 0 of
    /// the command register is bit 5 without bit 7.
    pub fn dac_write_mask(&mut self, value: u8) -> bool {
        if self.dac_reads != 4 {
            return false;
        }
        self.dac_reads = 0;
        if value != 0xFF {
            self.dac_command = (value & !1) | (((value >> 2) ^ value) & value & 0x20) >> 5;
        }
        true
    }

    /// Any other DAC port starts the count of reads again.
    pub fn dac_touched(&mut self) {
        self.dac_reads = 0;
    }

    /// The pixels each character clock of a graphics picture is: 8, 4
    /// in a 256-colour mode with Attribute Mode Control bit 6 (mode 13h,
    /// where a pixel is two dots), twice that with 16-dot characters, and
    /// half of it in a HiColor mode, two bytes a pixel.
    pub fn pixels_per_char(&self, vga: &VgaCard) -> usize {
        let narrow = vga.graphics_regs[0x05] & 0x40 != 0 && vga.attribute_regs[0x10] & 0x40 != 0;
        let mut pixels = if narrow { 4 } else { 8 };
        if self.wide_chars() {
            pixels *= 2;
        }
        if self.hicolor().is_some() {
            pixels /= 2;
        }
        pixels
    }
}

/// How a Tseng mode keeps its picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Characters and attributes, `width / 8` across.
    Text,
    /// 16 colours, a bit a pixel in each of the four planes.
    Planar,
    /// 256 colours (or HiColor), chained bytes.
    Packed,
}

/// A mode of Tseng's BIOS past the VGA's: text, 16 colours in planes or
/// 256 in chained memory, `width` dots across and `height` lines down,
/// with its character cell, and its timing as the
/// characters across (total, display, sync start, sync end), the lines
/// down (the same) and the clock (`Et4000::clock_hz`'s index), and the
/// doubled clock of its HiColor variant, if it has one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TsengMode {
    pub number: u8,
    pub kind: Kind,
    pub width: u16,
    pub height: u16,
    pub char_height: u8,
    h: [u16; 4],
    v: [u16; 4],
    clock: u8,
    hicolor_clock: Option<u8>,
}

/// The VGA's 640-dot line at 25.175 MHz, and VESA's 800 and 1024-dot ones
/// at 60 Hz.
const H_640: [u16; 4] = [100, 80, 82, 94];
const H_800: [u16; 4] = [132, 100, 105, 121];
const H_1024: [u16; 4] = [168, 128, 131, 148];
/// 132 characters of 8 dots at 40 MHz.
const H_132: [u16; 4] = [160, 132, 135, 147];

#[rustfmt::skip]
pub static TSENG_MODES: [TsengMode; 12] = [
    // 132 columns at 40 MHz, 31.25 kHz and 70 Hz: 44 rows of 8x8, 25 of
    // 8x14 and 28 of 8x13 (Ralf Brown's list).
    TsengMode { number: 0x22, kind: Kind::Text, width: 1056, height: 352, char_height: 8, h: H_132, v: [449, 352, 387, 389], clock: 3, hicolor_clock: None },
    TsengMode { number: 0x23, kind: Kind::Text, width: 1056, height: 350, char_height: 14, h: H_132, v: [449, 350, 387, 389], clock: 3, hicolor_clock: None },
    TsengMode { number: 0x24, kind: Kind::Text, width: 1056, height: 364, char_height: 13, h: H_132, v: [449, 364, 387, 389], clock: 3, hicolor_clock: None },
    // 80x60 of 8x8 at 640x480, 100x40 of 8x15 at 800x600.
    TsengMode { number: 0x26, kind: Kind::Text, width: 640, height: 480, char_height: 8, h: H_640, v: [525, 480, 490, 492], clock: 0, hicolor_clock: None },
    TsengMode { number: 0x2A, kind: Kind::Text, width: 800, height: 600, char_height: 15, h: H_800, v: [628, 600, 601, 605], clock: 3, hicolor_clock: None },
    TsengMode { number: 0x29, kind: Kind::Planar, width: 800, height: 600, char_height: 16, h: H_800, v: [628, 600, 601, 605], clock: 3, hicolor_clock: None },
    TsengMode { number: 0x2D, kind: Kind::Packed, width: 640, height: 350, char_height: 14, h: H_640, v: [449, 350, 387, 389], clock: 0, hicolor_clock: Some(8) },
    TsengMode { number: 0x2E, kind: Kind::Packed, width: 640, height: 480, char_height: 16, h: H_640, v: [525, 480, 490, 492], clock: 0, hicolor_clock: Some(8) },
    TsengMode { number: 0x2F, kind: Kind::Packed, width: 640, height: 400, char_height: 16, h: H_640, v: [449, 400, 412, 414], clock: 0, hicolor_clock: Some(8) },
    TsengMode { number: 0x30, kind: Kind::Packed, width: 800, height: 600, char_height: 16, h: H_800, v: [628, 600, 601, 605], clock: 3, hicolor_clock: Some(12) },
    TsengMode { number: 0x37, kind: Kind::Planar, width: 1024, height: 768, char_height: 16, h: H_1024, v: [806, 768, 771, 777], clock: 10, hicolor_clock: None },
    TsengMode { number: 0x38, kind: Kind::Packed, width: 1024, height: 768, char_height: 16, h: H_1024, v: [806, 768, 771, 777], clock: 10, hicolor_clock: None },
];

/// Tseng's mode `number`, if it is one of its graphics modes.
pub fn tseng_mode(number: u8) -> Option<&'static TsengMode> {
    TSENG_MODES.iter().find(|m| m.number == number)
}

/// The DAC command register's value for HiColor at `bits` (15 or 16).
pub fn hicolor_command(bits: u8) -> u8 {
    if bits == 16 { 0xE0 } else { 0xA0 }
}

/// The registers a Tseng mode sets: Miscellaneous Output, the CRTC's and
/// the chip's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TsengRegs {
    pub misc: u8,
    pub crtc: [u8; 25],
    pub cr31: u8,
    pub cr34: u8,
    pub cr35: u8,
    pub cr3f: u8,
    pub atc16: u8,
}

/// The clock select bits of clock `index`: Miscellaneous Output bits 2-3,
/// CR34 bit 1 and CR31 bit 6.
fn clock_bits(index: u8) -> (u8, u8, u8) {
    ((index & 3) << 2, (index >> 2 & 1) << 1, (index >> 3 & 1) << 6)
}

impl TsengMode {
    /// Whether the mode has a HiColor variant.
    pub fn has_hicolor(&self) -> bool {
        self.hicolor_clock.is_some()
    }

    /// The registers for the mode, in HiColor with `hicolor`: characters
    /// of 16 dots at twice the clock, two bytes a pixel.
    pub fn registers(&self, hicolor: bool) -> TsengRegs {
        let [ht, hd, hs, he] = self.h.map(u32::from);
        let [vt, vd, vs, ve] = self.v.map(u32::from);
        let clock = if hicolor { self.hicolor_clock.unwrap_or(self.clock) } else { self.clock };
        let (misc_clock, cr34, cr31) = clock_bits(clock);
        // The sync polarities tell a VGA monitor the lines.
        let polarity = match self.height {
            350 => 0x80,
            400 => 0x40,
            480 => 0xC0,
            _ => 0x00,
        };
        // The words from row to row: a plane's bytes a row over two, or a
        // chained mode's bytes over eight.
        let offset = match (self.kind, hicolor) {
            (Kind::Text | Kind::Planar, _) => self.width as u32 / 16,
            (Kind::Packed, false) => self.width as u32 / 8,
            (Kind::Packed, true) => self.width as u32 / 4,
        };
        // A text mode's character rows and its cursor at their bottom.
        let (max_scan, cursor) = match self.kind {
            Kind::Text => (self.char_height - 1, [self.char_height - 3, self.char_height - 2]),
            _ => (0, [0, 0]),
        };
        let bit = |v: u32, n: u32| ((v >> n) & 1) as u8;
        let (htotal, hblank_end) = (ht - 5, ht - 1);
        let (vtotal, vdisplay, vblank_start, vblank_end) = (vt - 2, vd - 1, vd, vt - 1);
        let crtc = [
            htotal as u8,
            (hd - 1) as u8,
            hd as u8,
            0x80 | (hblank_end & 0x1F) as u8,
            hs as u8,
            ((hblank_end & 0x20) << 2) as u8 | (he & 0x1F) as u8,
            vtotal as u8,
            bit(vtotal, 8)
                | bit(vdisplay, 8) << 1
                | bit(vs, 8) << 2
                | bit(vblank_start, 8) << 3
                | 1 << 4
                | bit(vtotal, 9) << 5
                | bit(vdisplay, 9) << 6
                | bit(vs, 9) << 7,
            0x00,
            0x40 | bit(vblank_start, 9) << 5 | max_scan,
            cursor[0],
            cursor[1],
            0x00,
            0x00,
            0x00,
            0x00,
            vs as u8,
            0x80 | (ve & 0x0F) as u8,
            vdisplay as u8,
            offset as u8,
            match self.kind {
                Kind::Text => 0x1F,
                Kind::Planar => 0x00,
                Kind::Packed => 0x40,
            },
            vblank_start as u8,
            vblank_end as u8,
            if self.kind == Kind::Planar { 0xE3 } else { 0xA3 },
            0xFF,
        ];
        TsengRegs {
            misc: 0x23 | polarity | misc_clock,
            crtc,
            cr31,
            cr34,
            cr35: bit(vblank_start, 10) | bit(vtotal, 10) << 1 | bit(vdisplay, 10) << 2 | bit(vs, 10) << 3 | 1 << 4,
            cr3f: bit(htotal, 8) | bit(offset, 8) << 7,
            atc16: if hicolor { 0x20 } else { 0x00 },
        }
    }
}

impl Et4000 {
    /// Set the chip's registers for a mode as Tseng's BIOS does, KEY or
    /// not, with the DAC command register `dac`.
    pub fn program(&mut self, regs: &TsengRegs, dac: u8) {
        self.crtc[0x01] = regs.cr31;
        self.crtc[0x04] = regs.cr34;
        self.crtc[0x05] = regs.cr35;
        self.crtc[0x0F] = regs.cr3f;
        self.atc[0] = regs.atc16;
        self.dac_command = dac;
    }
}

crate::state_fields!(Et4000 { herc_compat, keyed, segment, crtc, seq, atc, dac_reads, dac_command, vbe_mode });

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_guards_the_extended_registers_but_cr33() {
        let mut chip = Et4000::new();
        chip.herc_compat = 0x01;
        chip.write_mode_control(0x29);
        assert!(!chip.keyed);
        chip.write_crtc(0x34, 0x02, false);
        chip.write_atc(0x16, 0x20);
        assert_eq!(chip.read_crtc(0x34), 0);
        assert_eq!(chip.read_crtc(0x37), 0);
        assert!(!chip.wide_chars());
        // CR33 answers without it, four bits of it.
        chip.write_crtc(0x33, 0xFF, false);
        assert_eq!(chip.read_crtc(0x33), 0x0F);

        chip.herc_compat = 0x03;
        chip.write_mode_control(0xA0);
        assert!(chip.keyed);
        chip.write_crtc(0x34, 0x02, false);
        assert_eq!(chip.read_crtc(0x34), 0x02);
        assert_eq!(chip.read_crtc(0x37), MEMORY_CONFIG);
    }

    #[test]
    fn the_segment_register_has_a_bank_for_writes_and_one_for_reads() {
        let mut chip = Et4000::new();
        chip.segment = 0x3A;
        assert_eq!(chip.bank(true), 0x0A);
        assert_eq!(chip.bank(false), 0x03);
        assert!(chip.banks_on(0x05));
        assert!(!chip.banks_on(0x0E));
    }

    #[test]
    fn the_fifth_read_of_the_pixel_mask_is_the_command_register() {
        let mut chip = Et4000::new();
        assert_eq!(chip.dac_read_mask(0xFF), 0xFF);
        for _ in 0..3 {
            assert_eq!(chip.dac_read_mask(0xFF), 0x00);
        }
        assert_eq!(chip.dac_read_mask(0xFF), 0x18);
        assert_eq!(chip.dac_read_mask(0xE7), 0x00);
        assert!(chip.dac_write_mask(0xA0));
        assert_eq!(chip.hicolor(), Some(15));
        // The count starts again after a write, and after the other ports.
        assert!(!chip.dac_write_mask(0xFF));
        for _ in 0..3 {
            chip.dac_read_mask(0xFF);
        }
        chip.dac_touched();
        chip.dac_read_mask(0xFF);
        assert!(!chip.dac_write_mask(0x00));
        assert_eq!(chip.hicolor(), Some(15));
        for _ in 0..4 {
            chip.dac_read_mask(0xFF);
        }
        assert!(chip.dac_write_mask(0xE0));
        assert_eq!(chip.hicolor(), Some(16));
        // FFh leaves it; bit 0 follows bits 5 and 7.
        for _ in 0..4 {
            chip.dac_read_mask(0x00);
        }
        assert!(chip.dac_write_mask(0xFF));
        assert_eq!(chip.dac_command, 0xE0);
        for _ in 0..4 {
            chip.dac_read_mask(0x00);
        }
        chip.dac_write_mask(0x21);
        assert_eq!(chip.dac_command, 0x21);
        for _ in 0..4 {
            chip.dac_read_mask(0x00);
        }
        chip.dac_write_mask(0xA1);
        assert_eq!(chip.dac_command, 0xA0);
    }

    #[test]
    fn the_clock_select_bits_pick_a_clock() {
        let mut chip = Et4000::new();
        assert_eq!(chip.clock_hz(0x63), 25_175_000);
        assert_eq!(chip.clock_hz(0x67), 28_322_000);
        assert_eq!(chip.clock_hz(0xEF), 40_000_000);
        chip.write_crtc(0x34, 0x02, false);
        assert_eq!(chip.clock_hz(0xEF), 37_500_000);
        chip.write_crtc(0x34, 0x00, false);
        chip.write_crtc(0x31, 0x40, false);
        assert_eq!(chip.clock_hz(0xEB), 64_900_000);
        assert_eq!(chip.clock_hz(0xE3), 50_000_000);
    }

    #[test]
    fn tseng_modes_describe_a_sane_timing() {
        use super::super::crt::{CrtTiming, Extension};
        for mode in &TSENG_MODES {
            for hicolor in [false, true] {
                if hicolor && !mode.has_hicolor() {
                    continue;
                }
                let regs = mode.registers(hicolor);
                let mut chip = Et4000::new();
                chip.program(&regs, 0);
                let seq01 = 0x01;
                let ext = Extension {
                    clock: Some(chip.clock_hz(regs.misc)),
                    wide_chars: chip.wide_chars(),
                    htotal_high: chip.htotal_high(),
                    vertical_high: chip.vertical_high(),
                };
                let timing = CrtTiming::from_extended_registers(regs.misc, seq01, &regs.crtc, &ext);
                let timing = timing.unwrap_or_else(|| panic!("mode {:02X} hicolor {}", mode.number, hicolor));
                assert_eq!(timing.display, mode.height as u32, "mode {:02X}", mode.number);
                assert!((55.0..75.0).contains(&timing.hz()), "mode {:02X}: {} Hz", mode.number, timing.hz());
            }
        }
    }

    #[test]
    fn the_overflow_bits() {
        let mut chip = Et4000::new();
        chip.write_crtc(0x35, 0x06, false);
        chip.write_crtc(0x3F, 0x81, false);
        assert_eq!(chip.vertical_high(), [1024, 1024, 0, 0]);
        assert_eq!(chip.htotal_high(), 256);
        assert_eq!(chip.offset_high(), 256);
        // CR11 bit 7 protects CR35.
        chip.write_crtc(0x35, 0x00, true);
        assert_eq!(chip.vertical_high(), [1024, 1024, 0, 0]);
    }
}
