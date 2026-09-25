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
    /// IBM's Enhanced Graphics Adapter with an Enhanced Color Display: 16
    /// of 64 colours at 640x350, 16 at 320x200 and 640x200, 60 Hz.
    Ega,
    /// IBM's Color Graphics Adapter: 4 colours at 320x200, 2 at 640x200,
    /// 16 in text, through a 6845 CRTC.
    Cga,
    /// The Hercules Graphics Card on a monochrome monitor: the MDA's text
    /// and 720x348 graphics, 50 Hz.
    Hercules,
    /// The Tandy 1000's video: the CGA's modes and 16 colours at 160x200
    /// and 320x200 and 4 at 640x200, from the top of system memory.
    Tandy,
    /// The IBM PCjr's, which the Tandy's copies: its memory is the first
    /// 128 KB of system memory.
    Pcjr,
}

impl Adapter {
    pub const ALL: [Adapter; 7] =
        [Adapter::Svga, Adapter::Vga, Adapter::Ega, Adapter::Cga, Adapter::Tandy, Adapter::Pcjr, Adapter::Hercules];

    /// The adapter a `machine` value names, DOSBox's names included.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "svga" | "svga_s3" | "svga_et4000" | "svga_et3000" | "svga_paradise" | "vesa_nolfb" | "vesa_oldvbe" => {
                Some(Adapter::Svga)
            }
            "vga" | "vgaonly" => Some(Adapter::Vga),
            "ega" => Some(Adapter::Ega),
            "cga" => Some(Adapter::Cga),
            "hercules" | "herc" | "hgc" => Some(Adapter::Hercules),
            "tandy" => Some(Adapter::Tandy),
            "pcjr" => Some(Adapter::Pcjr),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Adapter::Svga => "svga",
            Adapter::Vga => "vga",
            Adapter::Ega => "ega",
            Adapter::Cga => "cga",
            Adapter::Hercules => "hercules",
            Adapter::Tandy => "tandy",
            Adapter::Pcjr => "pcjr",
        }
    }

    /// The adapter as the settings window shows it.
    pub fn describe(self) -> &'static str {
        match self {
            Adapter::Svga => "Super VGA (VESA)",
            Adapter::Vga => "VGA",
            Adapter::Ega => "EGA",
            Adapter::Cga => "CGA",
            Adapter::Hercules => "Hercules (mono)",
            Adapter::Tandy => "Tandy 1000",
            Adapter::Pcjr => "IBM PCjr",
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
        !matches!(self, Adapter::Cga | Adapter::Hercules | Adapter::Tandy | Adapter::Pcjr)
    }

    /// Whether the adapter works as a CGA does: a 6845 at 3D4h, the CGA's
    /// modes and colours, and no EGA or VGA registers.
    pub fn cga_like(self) -> bool {
        matches!(self, Adapter::Cga | Adapter::Tandy | Adapter::Pcjr)
    }

    /// Whether the video is the PCjr's video gate array (or the Tandy
    /// 1000's copy of it), which shows system memory.
    pub fn gate_array(self) -> bool {
        matches!(self, Adapter::Tandy | Adapter::Pcjr)
    }

    /// Whether the adapter only has a monochrome monitor's modes: the text
    /// mode 7 (and graphics the BIOS doesn't know).
    pub fn mono_only(self) -> bool {
        self == Adapter::Hercules
    }

    /// Whether the BIOS sets standard mode `mode` (INT 10h AH=00h).
    pub fn supports_mode(self, mode: u8) -> bool {
        match self {
            Adapter::Hercules => mode == 0x07,
            Adapter::Cga => mode <= 0x06,
            Adapter::Tandy | Adapter::Pcjr => matches!(mode, 0x00..=0x06 | 0x08..=0x0A),
            Adapter::Ega => matches!(mode, 0x00..=0x06 | 0x0D | 0x0E | 0x10),
            _ => matches!(mode, 0x00..=0x07 | 0x0D..=0x13),
        }
    }
}

/// The display adapter and the monitor on it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VideoSetup {
    pub adapter: Adapter,
    /// A monochrome monitor (`monochrome`): a VGA's analog one, which only
    /// shows shades of grey, or the IBM Monochrome Display on an EGA, which
    /// only takes the monochrome modes. A Hercules card always has one; a
    /// CGA's colour monitor shows a monochrome look only.
    pub mono_monitor: bool,
}

impl VideoSetup {
    /// Whether programs see a monochrome display.
    pub fn mono(self) -> bool {
        self.adapter.mono_only() || self.mono_monitor
    }

    /// Whether the BIOS sets standard mode `mode` (INT 10h AH=00h): an EGA
    /// on a monochrome monitor has modes 07h and 0Fh only.
    pub fn supports_mode(self, mode: u8) -> bool {
        match self.adapter {
            Adapter::Ega if self.mono_monitor => matches!(mode, 0x07 | 0x0F),
            adapter => adapter.supports_mode(mode),
        }
    }

    /// The mode the machine starts in and the DOS prompt runs in: 80x25
    /// text, in colour (3) or monochrome (7).
    pub fn prompt_mode(self) -> u8 {
        if self.mono() { 0x07 } else { 0x03 }
    }
}
