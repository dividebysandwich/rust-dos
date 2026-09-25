//! The CRT looks the picture can be shown with (`shader`): the GLSL the
//! window's OpenGL and the browser's WebGL 2 draw them with, and the tube's
//! curvature, which mouse positions have to go through too.

/// The CRT look's own settings, in percent: `crt_curvature` and
/// `crt_glow` in `[emulator]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CrtSettings {
    /// How far the tube bends, from 0 (flat) to `MAX_AMOUNT`.
    pub curvature: u16,
    /// How much light spreads around bright parts, from 0 (none) to
    /// `MAX_AMOUNT`.
    pub glow: u16,
}

impl Default for CrtSettings {
    fn default() -> Self {
        Self { curvature: 30, glow: 20 }
    }
}

/// The most of a CRT setting, in percent.
pub const MAX_AMOUNT: u16 = 100;

/// A CRT setting as written: a number of percent, with or without the %.
pub fn parse_amount(value: &str) -> Option<u16> {
    let number = value.trim().trim_end_matches('%').trim_end();
    number.parse::<u16>().ok().filter(|&percent| percent <= MAX_AMOUNT)
}

/// How far the CRT's tube bends at the most, across and down: a quarter
/// more down, as the picture is a quarter wider than high.
const MAX_CURVATURE: [f32; 2] = [0.1, 0.4 / 3.0];

/// The most glow of the CRT: the light of the frame around a point, blurred,
/// that adds to it.
const MAX_GLOW: f32 = 0.4;

/// How the picture is shown: as it is, or through a CRT look.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Shader {
    #[default]
    None,
    /// A flat screen with VGA scanlines.
    Scanlines,
    /// A flat aperture grille monitor: scanlines and phosphor stripes.
    Aperture,
    /// A curved tube with a shadow mask, rounded corners and darker edges.
    Crt,
}

impl Shader {
    pub const ALL: [Shader; 4] = [Shader::None, Shader::Scanlines, Shader::Aperture, Shader::Crt];

    /// A look by its name, or the CRT by its old one, `curved`.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let found = Self::ALL.into_iter().find(|shader| shader.name().eq_ignore_ascii_case(s));
        found.or_else(|| s.eq_ignore_ascii_case("curved").then_some(Shader::Crt))
    }

    pub fn name(self) -> &'static str {
        match self {
            Shader::None => "none",
            Shader::Scanlines => "scanlines",
            Shader::Aperture => "aperture",
            Shader::Crt => "crt",
        }
    }

    /// The look as the settings window shows it.
    pub fn describe(self) -> &'static str {
        match self {
            Shader::None => "none",
            Shader::Scanlines => "scanlines",
            Shader::Aperture => "aperture grille",
            Shader::Crt => "CRT",
        }
    }

    /// Where the picture shows the point `(u, v)` of the frame: 0 to 1
    /// across and down it, the same bend as the shader's with `crt`.
    /// Points on the black around a curved picture come out below 0 or
    /// above 1.
    pub fn warp(self, crt: CrtSettings, u: f32, v: f32) -> (f32, f32) {
        let Some(look) = self.look().filter(|look| look.curved) else {
            return (u, v);
        };
        let [cx, cy] = self.curvature(crt);
        let (x, y) = ((u * 2.0 - 1.0) * look.overscan, (v * 2.0 - 1.0) * look.overscan);
        let (x, y) = (x * (1.0 + cx * y * y), y * (1.0 + cy * x * x));
        (x * 0.5 + 0.5, y * 0.5 + 0.5)
    }

    /// How far the tube bends across and down with `crt` (`u_curvature`):
    /// only the CRT's does.
    pub fn curvature(self, crt: CrtSettings) -> [f32; 2] {
        match self {
            Shader::Crt => {
                let amount = crt.curvature.min(MAX_AMOUNT) as f32 / MAX_AMOUNT as f32;
                MAX_CURVATURE.map(|most| most * amount)
            }
            _ => [0.0, 0.0],
        }
    }

    /// How much light spreads around bright parts with `crt` (`u_glow`):
    /// the CRT's as its setting says, the flat looks' a little.
    pub fn glow(self, crt: CrtSettings) -> f32 {
        match self {
            Shader::None => 0.0,
            Shader::Scanlines => 0.04,
            Shader::Aperture => 0.06,
            Shader::Crt => MAX_GLOW * crt.glow.min(MAX_AMOUNT) as f32 / MAX_AMOUNT as f32,
        }
    }

    fn look(self) -> Option<Look> {
        let flat = Look {
            mask: Mask::None,
            beam: [0.16, 0.28],
            edge: 0.5,
            mask_strength: 0.0,
            slot_gap: 1.0,
            curved: false,
            overscan: 1.0,
            corner: 0.0,
            vignette: 0.0,
        };
        match self {
            Shader::None => None,
            Shader::Scanlines => Some(flat),
            Shader::Aperture => Some(Look {
                mask: Mask::Grille,
                beam: [0.18, 0.30],
                edge: 0.4,
                mask_strength: 0.35,
                ..flat
            }),
            Shader::Crt => Some(Look {
                mask: Mask::Slots,
                beam: [0.18, 0.30],
                edge: 0.6,
                mask_strength: 0.35,
                slot_gap: 0.5,
                curved: true,
                overscan: 1.02,
                corner: 0.03,
                vignette: 0.15,
            }),
        }
    }
}

/// The phosphors in front of the picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mask {
    None = 0,
    Grille = 1,
    Slots = 2,
}

/// A CRT look, as the #defines crt.glsl is built with. The overscan is
/// here once for the shader and `Shader::warp`; how far the tube bends and
/// how much it glows are uniforms (`Shader::curvature`, `Shader::glow`).
#[derive(Clone, Copy, Debug)]
struct Look {
    mask: Mask,
    /// The beam's width at black and at white, in scanlines.
    beam: [f32; 2],
    edge: f32,
    mask_strength: f32,
    slot_gap: f32,
    /// A tube that bends as `u_curvature` says, with rounded corners.
    curved: bool,
    overscan: f32,
    corner: f32,
    vignette: f32,
}

impl Look {
    /// The look as GLSL. `{:?}` writes a float so that it reads back as
    /// the same float, with a decimal point.
    fn defines(&self) -> String {
        let defines = [
            ("MASK", format!("{}", self.mask as u8)),
            ("CURVED", format!("{}", self.curved as u8)),
            ("BEAM_MIN", format!("{:?}", self.beam[0])),
            ("BEAM_MAX", format!("{:?}", self.beam[1])),
            ("EDGE", format!("{:?}", self.edge)),
            ("MASK_STRENGTH", format!("{:?}", self.mask_strength)),
            ("SLOT_GAP", format!("{:?}", self.slot_gap)),
            ("OVERSCAN", format!("{:?}", self.overscan)),
            ("CORNER", format!("{:?}", self.corner)),
            ("VIGNETTE", format!("{:?}", self.vignette)),
        ];
        defines.iter().map(|(name, value)| format!("#define {} {}\n", name, value)).collect()
    }
}

/// The GLSL version a context takes. The sources are written for all of
/// them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glsl {
    /// OpenGL 3.0 and 3.1.
    Gl130,
    /// OpenGL 3.2 and later, which macOS only has as a core profile.
    Gl150,
    /// OpenGL ES 3 and WebGL 2.
    Es300,
}

impl Glsl {
    /// The GLSL of an OpenGL (ES if `embedded`) version, if it is new
    /// enough.
    pub fn for_gl(major: u32, minor: u32, embedded: bool) -> Option<Self> {
        match (embedded, (major, minor)) {
            (true, v) if v >= (3, 0) => Some(Glsl::Es300),
            (false, v) if v >= (3, 2) => Some(Glsl::Gl150),
            (false, v) if v >= (3, 0) => Some(Glsl::Gl130),
            _ => None,
        }
    }

    fn preamble(self) -> &'static str {
        match self {
            Glsl::Gl130 => "#version 130\n",
            Glsl::Gl150 => "#version 150\n",
            Glsl::Es300 => "#version 300 es\nprecision highp float;\nprecision highp int;\n",
        }
    }
}

const VERTEX: &str = include_str!("shader/vertex.glsl");
const PLAIN: &str = include_str!("shader/plain.glsl");
const CRT: &str = include_str!("shader/crt.glsl");

/// The vertex and fragment shader of a look. They take the frame as the
/// texture `u_frame`; the CRT looks also take the frame's size in pixels
/// as `u_source`, the picture's on the screen as `u_output`, whether the
/// tube has a colour mask as `u_mask` (1 or 0, for a monochrome tube), how
/// far it bends as `u_curvature` (`Shader::curvature`) and how much it
/// glows as `u_glow` (`Shader::glow`), and read a mipmap of the frame. The
/// fragment shader writes `o_color`.
pub fn sources(shader: Shader, glsl: Glsl) -> (String, String) {
    let preamble = glsl.preamble();
    let fragment = match shader.look() {
        None => format!("{}{}", preamble, PLAIN),
        Some(look) => format!("{}{}{}", preamble, look.defines(), CRT),
    };
    (format!("{}{}", preamble, VERTEX), fragment)
}

/// Whether a look reads the frame's mipmap, which has to be made after
/// each new frame.
pub fn needs_mipmaps(shader: Shader) -> bool {
    shader != Shader::None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_parse_back() {
        for shader in Shader::ALL {
            assert_eq!(Shader::parse(shader.name()), Some(shader));
        }
        assert_eq!(Shader::parse(" CRT "), Some(Shader::Crt));
        assert_eq!(Shader::parse("Curved"), Some(Shader::Crt));
        assert_eq!(Shader::parse("crt-royale"), None);
    }

    #[test]
    fn flat_looks_bend_nothing() {
        for shader in [Shader::None, Shader::Scanlines, Shader::Aperture] {
            for (u, v) in [(0.0, 0.0), (0.3, 0.7), (1.0, 1.0), (-0.2, 1.5)] {
                assert_eq!(shader.warp(CrtSettings::default(), u, v), (u, v));
            }
        }
    }

    #[test]
    fn the_curved_tube_bends_the_edges() {
        let warp = |u, v| Shader::Crt.warp(CrtSettings::default(), u, v);
        assert_eq!(warp(0.5, 0.5), (0.5, 0.5));
        // The corners are behind the bezel.
        let (u, v) = warp(0.0, 0.0);
        assert!(u < 0.0 && v < 0.0);
        let (u, v) = warp(1.0, 1.0);
        assert!(u > 1.0 && v > 1.0);
        // The overscan hides the very edge, not more.
        assert!(warp(0.5, 0.0).1 < 0.0);
        let (u, v) = warp(0.5, 0.02);
        assert!(u == 0.5 && (0.0..0.05).contains(&v));
        // Points further down stay further down.
        let rows: Vec<f32> = (0..=20).map(|i| warp(0.1, i as f32 / 20.0).1).collect();
        assert!(rows.windows(2).all(|w| w[0] < w[1]));

        // The default is the bend the tube always had; more bends the
        // corners further, none leaves only the overscan.
        let [x, y] = Shader::Crt.curvature(CrtSettings::default());
        assert!((x - 0.03).abs() < 1e-6 && (y - 0.04).abs() < 1e-6);
        assert_eq!(Shader::Aperture.curvature(CrtSettings::default()), [0.0, 0.0]);
        let corner = |curvature| Shader::Crt.warp(CrtSettings { curvature, ..CrtSettings::default() }, 0.0, 0.0).0;
        assert!(corner(100) < corner(30) && corner(30) < corner(0));
        assert_eq!(corner(0), 0.5 - 0.5 * 1.02);
        assert_eq!(parse_amount(" 45% "), Some(45));

        // The CRT glows as it always did by default, and as its setting
        // says; the flat looks a little, whatever it says.
        assert!((Shader::Crt.glow(CrtSettings::default()) - 0.08).abs() < 1e-6);
        assert_eq!(Shader::Crt.glow(CrtSettings { glow: 0, ..CrtSettings::default() }), 0.0);
        assert_eq!(Shader::Crt.glow(CrtSettings { glow: 100, ..CrtSettings::default() }), MAX_GLOW);
        assert_eq!(Shader::Scanlines.glow(CrtSettings { glow: 100, ..CrtSettings::default() }), 0.04);
        assert_eq!(parse_amount("101"), None);
    }

    #[test]
    fn sources_start_with_their_version() {
        for (glsl, version) in
            [(Glsl::Gl130, "#version 130\n"), (Glsl::Gl150, "#version 150\n"), (Glsl::Es300, "#version 300 es\n")]
        {
            for shader in Shader::ALL {
                let (vertex, fragment) = sources(shader, glsl);
                assert!(vertex.starts_with(version) && fragment.starts_with(version));
                assert!(vertex.is_ascii() && fragment.is_ascii());
                assert!(fragment.contains("out vec4 o_color;"));
            }
        }
        let (_, curved) = sources(Shader::Crt, Glsl::Gl150);
        assert!(curved.contains("uniform vec2 u_curvature;") && curved.contains("u_curvature * c.yx * c.yx"));
        assert!(curved.contains("uniform float u_glow;") && curved.contains("u_glow * glow(t)"));
        assert!(curved.contains("#define CURVED 1\n") && curved.contains("#define MASK 2\n"));
        assert!(curved.contains("uniform float u_mask;") && curved.contains("MASK_STRENGTH * u_mask"));
        let (_, flat) = sources(Shader::Scanlines, Glsl::Gl150);
        assert!(flat.contains("#define CURVED 0\n") && flat.contains("#define MASK 0\n"));
    }

    #[test]
    fn gl_versions() {
        assert_eq!(Glsl::for_gl(4, 6, false), Some(Glsl::Gl150));
        assert_eq!(Glsl::for_gl(3, 1, false), Some(Glsl::Gl130));
        assert_eq!(Glsl::for_gl(2, 1, false), None);
        assert_eq!(Glsl::for_gl(3, 0, true), Some(Glsl::Es300));
        assert_eq!(Glsl::for_gl(2, 0, true), None);
    }

    /// Every shader in every dialect compiles, where glslang's validator is
    /// installed.
    #[test]
    fn sources_compile() {
        use std::io::Write;
        use std::process::{Command, Stdio};

        if Command::new("glslangValidator").arg("--version").output().is_err() {
            eprintln!("glslangValidator isn't installed, not compiling the shaders");
            return;
        }
        for glsl in [Glsl::Gl130, Glsl::Gl150, Glsl::Es300] {
            for shader in Shader::ALL {
                let (vertex, fragment) = sources(shader, glsl);
                for (stage, source) in [("vert", vertex), ("frag", fragment)] {
                    let mut child = Command::new("glslangValidator")
                        .args(["--stdin", "-S", stage])
                        .stdin(Stdio::piped())
                        .stdout(Stdio::piped())
                        .spawn()
                        .unwrap();
                    child.stdin.take().unwrap().write_all(source.as_bytes()).unwrap();
                    let output = child.wait_with_output().unwrap();
                    assert!(
                        output.status.success(),
                        "{:?} {:?} {}: {}",
                        shader,
                        glsl,
                        stage,
                        String::from_utf8_lossy(&output.stdout)
                    );
                }
            }
        }
    }
}
