//! The display adapter the machine has (`machine`): the one programs find
//! when they look for it, and so the one whose graphics they choose.

/// A display adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Adapter {
    /// A VGA with the VESA BIOS extensions: Super VGA modes up to
    /// 1024x768.
    #[default]
    Svga,
    /// IBM's VGA, without VESA modes.
    Vga,
    /// IBM's Color Graphics Adapter: 4 colours at 320x200, 2 at 640x200,
    /// 16 in text, through a 6845 CRTC.
    Cga,
}

impl Adapter {
    pub const ALL: [Adapter; 3] = [Adapter::Svga, Adapter::Vga, Adapter::Cga];

    /// The adapter a `machine` value names, DOSBox's names included.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "svga" | "svga_s3" | "svga_et4000" | "svga_et3000" | "svga_paradise" | "vesa_nolfb" | "vesa_oldvbe" => {
                Some(Adapter::Svga)
            }
            "vga" | "vgaonly" => Some(Adapter::Vga),
            "cga" => Some(Adapter::Cga),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Adapter::Svga => "svga",
            Adapter::Vga => "vga",
            Adapter::Cga => "cga",
        }
    }

    /// The adapter as the settings window shows it.
    pub fn describe(self) -> &'static str {
        match self {
            Adapter::Svga => "Super VGA (VESA)",
            Adapter::Vga => "VGA",
            Adapter::Cga => "CGA",
        }
    }

    /// Whether the BIOS has the VESA extensions (INT 10h AH=4Fh).
    pub fn has_vbe(self) -> bool {
        self == Adapter::Svga
    }

    /// Whether the BIOS has the VGA's functions: the display combination
    /// code (INT 10h AH=1Ah), the state information (AH=1Bh) and the DAC.
    pub fn vga_bios(self) -> bool {
        matches!(self, Adapter::Svga | Adapter::Vga)
    }

    /// Whether the BIOS has the EGA's functions: the palette registers
    /// (INT 10h AH=10h), the character generator (AH=11h) and the
    /// configuration (AH=12h).
    pub fn ega_bios(self) -> bool {
        self != Adapter::Cga
    }

    /// Whether the BIOS sets standard mode `mode` (INT 10h AH=00h).
    pub fn supports_mode(self, mode: u8) -> bool {
        match self {
            Adapter::Cga => mode <= 0x06,
            _ => matches!(mode, 0x00..=0x06 | 0x0D | 0x0E | 0x10 | 0x12 | 0x13),
        }
    }
}

/// The display adapter and the monitor on it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VideoSetup {
    pub adapter: Adapter,
}
