//! The display adapter the machine has (`machine`): the one programs find
//! when they look for it, and so the one whose graphics they choose.

/// A display adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Adapter {
    /// A VGA with the VESA BIOS extensions: Super VGA modes up to
    /// 1024x768.
    #[default]
    Svga,
    /// An S3 Trio64 with 4 MB: the Super VGA's modes, and the chip's own
    /// registers and graphics engine that S3's drivers (Windows') use.
    S3,
    /// An S3 ViRGE (86C325) with 4 MB: the Trio64's registers, and the
    /// ViRGE's own 2D engine and 3D engine (S3d) behind its memory-mapped
    /// registers, which Windows 95's Direct3D drives through S3's driver.
    S3Virge,
    /// The ViRGE/VX (86C988): the ViRGE's engines, other IDs, and its
    /// engine reset in CR63 instead of CR66.
    S3VirgeVx,
    /// A Rendition Vérité V1000 with 4 MB: the Super VGA's modes, and the
    /// RISC processor whose microcode draws 3D from commands in its FIFO.
    Verite,
    /// A Tseng Labs ET4000AX with 1 MB and a Sierra HiColor DAC: Tseng's
    /// BIOS modes up to 1024x768 in 256 colours, 32K and 64K colours, and
    /// VBE 1.2 through the 64 KB window its segment register banks.
    Et4000,
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
    pub const ALL: [Adapter; 12] = [
        Adapter::Svga,
        Adapter::S3,
        Adapter::S3Virge,
        Adapter::S3VirgeVx,
        Adapter::Verite,
        Adapter::Et4000,
        Adapter::Vga,
        Adapter::Ega,
        Adapter::Cga,
        Adapter::Tandy,
        Adapter::Pcjr,
        Adapter::Hercules,
    ];

    /// The adapter a `machine` value names, DOSBox's names included.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "svga" | "svga_et3000" | "svga_paradise" | "vesa_nolfb" | "vesa_oldvbe" => {
                Some(Adapter::Svga)
            }
            "svga_s3" | "s3" | "s3trio" => Some(Adapter::S3),
            "svga_s3virge" | "virge" | "s3virge" => Some(Adapter::S3Virge),
            "svga_s3virgevx" | "virgevx" | "s3virgevx" => Some(Adapter::S3VirgeVx),
            "svga_verite" | "verite" | "rendition" => Some(Adapter::Verite),
            "svga_et4000" | "et4000" | "tseng" => Some(Adapter::Et4000),
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
            Adapter::S3 => "svga_s3",
            Adapter::S3Virge => "svga_s3virge",
            Adapter::S3VirgeVx => "svga_s3virgevx",
            Adapter::Verite => "svga_verite",
            Adapter::Et4000 => "svga_et4000",
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
            Adapter::S3 => "S3 Trio64",
            Adapter::S3Virge => "S3 ViRGE (3D)",
            Adapter::S3VirgeVx => "S3 ViRGE/VX (3D)",
            Adapter::Verite => "Rendition Vérité (3D)",
            Adapter::Et4000 => "Tseng ET4000 (HiColor)",
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
        matches!(self, Adapter::Svga | Adapter::Et4000 | Adapter::Verite) || self.is_s3()
    }

    /// Whether the adapter is one of S3's chips: the Trio64's registers
    /// and hardware cursor, and PCI.
    pub fn is_s3(self) -> bool {
        matches!(self, Adapter::S3 | Adapter::S3Virge | Adapter::S3VirgeVx)
    }

    /// Whether the adapter is the Rendition Vérité.
    pub fn is_verite(self) -> bool {
        self == Adapter::Verite
    }

    /// Whether the adapter is the Tseng ET4000.
    pub fn is_et4000(self) -> bool {
        self == Adapter::Et4000
    }

    /// Whether the adapter is a ViRGE, with the ViRGE's engines.
    pub fn is_virge(self) -> bool {
        matches!(self, Adapter::S3Virge | Adapter::S3VirgeVx)
    }

    /// Whether the BIOS has the VGA's functions: the display combination
    /// code (INT 10h AH=1Ah), the state information (AH=1Bh) and the DAC.
    pub fn vga_bios(self) -> bool {
        matches!(self, Adapter::Svga | Adapter::Vga | Adapter::Et4000 | Adapter::Verite) || self.is_s3()
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
            // Tseng's text with 132 columns, 80x60 and 100x40, 800x600 and
            // 1024x768 in 16 colours, and 640x350 to 1024x768 in 256.
            Adapter::Et4000 => matches!(mode, 0x00..=0x07 | 0x0D..=0x13 | 0x22..=0x24 | 0x26 | 0x29 | 0x2A | 0x2D..=0x30 | 0x37 | 0x38),
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
