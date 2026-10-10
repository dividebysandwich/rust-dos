//! The V1000's I/O registers, offsets from BAR1 (the XFree86 driver's
//! `commonregs.h`).

pub const FIFOINFREE: u8 = 0x40;
pub const FIFOOUTVALID: u8 = 0x41;
pub const COMM: u8 = 0x42;
pub const MEMENDIAN: u8 = 0x43;
pub const INTR: u8 = 0x44;
pub const INTREN: u8 = 0x46;
pub const DEBUGREG: u8 = 0x48;
pub const LOWWATERMARK: u8 = 0x49;
pub const STATUS: u8 = 0x4A;
pub const PCITEST: u8 = 0x4C;
pub const DMACMDPTR: u8 = 0x50;
pub const DMA_ADDRESS: u8 = 0x54;
pub const DMA_COUNT: u8 = 0x58;
pub const STATEINDEX: u8 = 0x60;
pub const STATEDATA: u8 = 0x64;
pub const SCRATCH: u8 = 0x70;
pub const MODE: u8 = 0x72;
pub const BANKSELECT: u8 = 0x74;
pub const CRTCTEST: u8 = 0x80;
pub const CRTCCTL: u8 = 0x84;
pub const CRTCHORZ: u8 = 0x88;
pub const CRTCVERT: u8 = 0x8C;
pub const FRAMEBASEB: u8 = 0x90;
pub const FRAMEBASEA: u8 = 0x94;
pub const CRTCOFFSET: u8 = 0x98;
pub const CRTCSTATUS: u8 = 0x9C;
pub const DRAMCTL: u8 = 0xA0;
pub const PALETTE: u8 = 0xB0;

/// DEBUGREG: reset the chip, hold the RISC, step it.
pub const SOFTRESET: u8 = 0x01;
pub const HOLDRISC: u8 = 0x02;
pub const STEPRISC: u8 = 0x04;

/// A register's name, for traces.
pub fn name(reg: u8) -> String {
    let fixed = match reg & 0xFC {
        0x00..=0x0C => return format!("FIFO{}+{}", reg >> 2, reg & 3),
        0x40 => match reg {
            FIFOINFREE => "FIFOINFREE",
            FIFOOUTVALID => "FIFOOUTVALID",
            COMM => "COMM",
            _ => "MEMENDIAN",
        },
        INTR => if reg < INTREN { "INTR" } else { "INTREN" },
        DEBUGREG => match reg {
            DEBUGREG => "DEBUGREG",
            LOWWATERMARK => "LOWWATERMARK",
            _ => "STATUS",
        },
        PCITEST => "PCITEST",
        DMACMDPTR => "DMACMDPTR",
        DMA_ADDRESS => "DMA_ADDRESS",
        DMA_COUNT => "DMA_COUNT",
        STATEINDEX => "STATEINDEX",
        STATEDATA => "STATEDATA",
        SCRATCH => match reg {
            SCRATCH | 0x71 => "SCRATCH",
            MODE => "MODE",
            _ => "SCRATCH8",
        },
        BANKSELECT => "BANKSELECT",
        CRTCTEST => "CRTCTEST",
        CRTCCTL => "CRTCCTL",
        CRTCHORZ => "CRTCHORZ",
        CRTCVERT => "CRTCVERT",
        FRAMEBASEB => "FRAMEBASEB",
        FRAMEBASEA => "FRAMEBASEA",
        CRTCOFFSET => "CRTCOFFSET",
        CRTCSTATUS => "CRTCSTATUS",
        DRAMCTL => "DRAMCTL",
        0xB0..=0xBC => "PALETTE",
        _ => return format!("reg{:02X}", reg),
    };
    let byte_register = matches!(reg, 0x40..=0x4B | 0x70..=0x73);
    if reg & 3 != 0 && !byte_register { format!("{}+{}", fixed, reg & 3) } else { fixed.to_string() }
}
