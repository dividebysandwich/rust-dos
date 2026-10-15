//! What tells the two chips apart where they share the code: the
//! register map, the widths of the register fields, the PCI IDs and the
//! power-on values. The code common to both reads them from the `Chip`
//! of the board's model; what only the Voodoo2 has (the command FIFO,
//! the triangle setup engine, the 2D blitter) is in modules of its own,
//! reached through `Chip::model`.

use super::Model;
use super::regs;

/// The fields of hSync, vSync and videoDimensions: the bits of the low
/// field and of the high one (at bit 16).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoFields {
    pub hsync: (u32, u32),
    pub vsync: (u32, u32),
    pub dimensions: (u32, u32),
}

/// A chip's constants.
#[derive(Clone, Copy, Debug)]
pub struct Chip {
    pub model: Model,
    /// The PCI device ID (vendor 121Ah).
    pub device_id: u16,
    /// Who may read and write each register.
    pub access: &'static [u8; 0x100],
    /// The bits fbzColorPath, fbzMode and fogMode keep.
    pub fbz_color_path_mask: u32,
    pub fbz_mode_mask: u32,
    pub fog_mode_mask: u32,
    /// The bits of the clip registers' fields.
    pub clip_mask: u32,
    /// The fraction bits bilinear filtering blends with.
    pub bilinear_mask: i32,
    /// The bits of a fog table delta used.
    pub fogdelta_mask: i32,
    pub video: VideoFields,
    /// What TMU 0 hands out with trexInit1 bit 18 (how Glide counts the
    /// units), for one and for two units.
    pub tmu_config: [u32; 2],
    /// fbiInit0 to 7 at power-on.
    pub fbi_init: [u32; 8],
    /// Whether PCI configuration C0h and E0h start and stop the video
    /// clock, which the picture needs; a Voodoo2's runs once its DAC is
    /// set up, and fbiInit0 alone gives it the monitor (86Box).
    pub pci_video_clock: bool,
}

/// The Voodoo Graphics, as DOSBox-X has it.
pub const VOODOO1: Chip = Chip {
    model: Model::Voodoo1,
    device_id: 0x0001,
    access: &regs::ACCESS,
    fbz_color_path_mask: 0x0FFF_FFFF,
    fbz_mode_mask: 0x001F_FFFF,
    fog_mode_mask: 0x3F,
    clip_mask: 0x3FF,
    bilinear_mask: 0xF0,
    fogdelta_mask: 0xFF,
    video: VideoFields { hsync: (8, 10), vsync: (12, 12), dimensions: (10, 10) },
    tmu_config: [0x11, 0xD1],
    fbi_init: [(1 << 4) | (0x10 << 6), (1 << 1) | (1 << 8) | (1 << 12) | (2 << 20), (1 << 6) | (0x100 << 23), (2 << 13) | (0xF << 17), 1, 0, 0, 0],
    pci_video_clock: true,
};

/// The Voodoo2: wider fields (its resolutions go to 1024x768), RGBZW
/// clamping in fbzColorPath bit 28, fog dithering and float depth bits,
/// full bilinear fractions, and the power-on values of Glide's cvg
/// sst1init.h (as 86Box has them).
pub const VOODOO2: Chip = Chip {
    model: Model::Voodoo2,
    device_id: 0x0002,
    access: &regs::ACCESS_V2,
    fbz_color_path_mask: 0x3FFF_FFFF,
    fbz_mode_mask: 0x003F_FFFF,
    fog_mode_mask: 0xFF,
    clip_mask: 0xFFF,
    bilinear_mask: 0xFF,
    fogdelta_mask: 0xFC,
    video: VideoFields { hsync: (9, 11), vsync: (13, 13), dimensions: (11, 11) },
    // MAME's Voodoo Graphics value with bit 11, which the Voodoo2 adds.
    tmu_config: [0x811, 0x8D1],
    fbi_init: [0x0000_1C10, 0x0020_1102, 0x8000_0040, 0x001E_4040, 1, 0x0840_8000, 0, 0],
    pci_video_clock: false,
};

impl Chip {
    pub fn of(model: Model) -> &'static Chip {
        match model {
            Model::Voodoo1 => &VOODOO1,
            Model::Voodoo2 => &VOODOO2,
        }
    }

    pub fn v2(&self) -> bool {
        self.model == Model::Voodoo2
    }
}

/// The low and the high field of a video register.
pub fn fields(value: u32, (low, high): (u32, u32)) -> (u32, u32) {
    (value & ((1 << low) - 1), (value >> 16) & ((1 << high) - 1))
}

#[cfg(test)]
mod tests {
    use super::super::regs::*;
    use super::super::{Board, Voodoo};

    #[test]
    fn power_on_registers() {
        let v = Voodoo::with_workers(Board::Max, 0);
        let init = |v: &Voodoo| [FBI_INIT0, FBI_INIT1, FBI_INIT2, FBI_INIT3, FBI_INIT4, FBI_INIT5].map(|r| v.reg[r]);
        assert_eq!(init(&v), [0x0410, 0x0020_1102, 0x8000_0040, 0x001E_4000, 1, 0]);
        assert_eq!((v.reg[H_SYNC], v.config_read(2)), (0, 0x01));
        let v = Voodoo::with_workers(Board::Voodoo2, 0);
        assert_eq!(init(&v), [0x1C10, 0x0020_1102, 0x8000_0040, 0x001E_4040, 1, 0x0840_8000]);
        assert_eq!((v.reg[H_SYNC], v.config_read(2)), (0, 0x02));
    }
}
