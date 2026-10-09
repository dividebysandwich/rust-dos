//! The `[vr]` settings: the picture on a screen in a 3D scene, in a VR
//! headset or in the window with a camera to fly around.

use crate::padmap::PadSnapshot;
use std::path::{Path, PathBuf};

/// Which of the PC's lights in the scene are lit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Leds {
    pub power: bool,
    /// Lit while the CPU runs faster than `TURBO_CYCLES`.
    pub turbo: bool,
    /// Lit while a hard disk is read or written.
    pub hdd: bool,
    /// Lit while a floppy disk is.
    pub floppy: bool,
}

/// The speed (instructions a millisecond) above which the turbo light is
/// on: faster than an XT or a slow AT.
pub const TURBO_CYCLES: u32 = 1000;

/// The bits of `PadSnapshot::buttons` the headset's controllers press, in
/// the order of `padmap::INPUTS`.
pub const PAD_A: u32 = 1 << 4;
pub const PAD_B: u32 = 1 << 5;
pub const PAD_X: u32 = 1 << 6;
pub const PAD_Y: u32 = 1 << 7;
pub const PAD_LEFT_SHOULDER: u32 = 1 << 8;
pub const PAD_RIGHT_SHOULDER: u32 = 1 << 9;

/// The plain joystick of the game port from the controllers' gamepad: A
/// or the right trigger is its first button, B or the right grip the
/// second, X or the left trigger the third, Y or the left grip the fourth.
pub fn joystick(pad: &PadSnapshot) -> crate::joystick::PadState {
    use crate::joystick::{PAD_A as A, PAD_B as B, PAD_X as X, PAD_Y as Y};
    let held = |bit: u32| pad.buttons & bit != 0;
    let mut buttons = 0;
    for (on, bit) in [
        (held(PAD_A) || pad.triggers[1] > 0.5, A),
        (held(PAD_B) || held(PAD_RIGHT_SHOULDER), B),
        (held(PAD_X) || pad.triggers[0] > 0.5, X),
        (held(PAD_Y) || held(PAD_LEFT_SHOULDER), Y),
    ] {
        if on {
            buttons |= bit;
        }
    }
    crate::joystick::PadState { axes: pad.axes, buttons }
}


/// Where the 3D scene is shown, if it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VrMode {
    /// The picture fills the window, as without VR.
    #[default]
    Off,
    /// In a VR headset through OpenXR, and the left eye's view in the
    /// window (`--vr`).
    Headset,
    /// In the window, seen through a camera moved with the mouse while
    /// Ctrl+Shift is held (`--vr-desktop`).
    Desktop,
}

impl VrMode {
    pub const ALL: [VrMode; 3] = [VrMode::Off, VrMode::Headset, VrMode::Desktop];

    pub fn name(self) -> &'static str {
        match self {
            VrMode::Off => "off",
            VrMode::Headset => "headset",
            VrMode::Desktop => "desktop",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        Self::ALL.into_iter().find(|mode| mode.name().eq_ignore_ascii_case(value))
    }
}

/// What a VR headset's controllers do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VrControllers {
    /// The hand pointing at the screen is the mouse, its trigger and grip
    /// the buttons; the sticks and the other buttons are a gamepad.
    #[default]
    Both,
    /// Point and click only.
    Pointer,
    /// A gamepad only.
    Gamepad,
}

impl VrControllers {
    pub const ALL: [VrControllers; 3] = [VrControllers::Both, VrControllers::Pointer, VrControllers::Gamepad];

    pub fn name(self) -> &'static str {
        match self {
            VrControllers::Both => "both",
            VrControllers::Pointer => "pointer",
            VrControllers::Gamepad => "gamepad",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            VrControllers::Both => "laser mouse and gamepad",
            VrControllers::Pointer => "laser mouse",
            VrControllers::Gamepad => "gamepad",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        Self::ALL.into_iter().find(|c| c.name().eq_ignore_ascii_case(value))
    }

    /// Whether a hand points and clicks.
    pub fn pointer(self) -> bool {
        self != VrControllers::Gamepad
    }

    /// Whether the controllers are a gamepad.
    pub fn gamepad(self) -> bool {
        self != VrControllers::Pointer
    }
}

/// How the picture fills the scene's screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScreenFit {
    /// As the scene says (its `rustdos_screen_fit` property), keeping the
    /// picture's shape if it doesn't.
    #[default]
    Auto,
    /// The picture keeps its shape, with bars where the screen is wider or
    /// taller.
    Fit,
    /// The picture is stretched over the whole screen.
    Stretch,
}

impl ScreenFit {
    pub const ALL: [ScreenFit; 3] = [ScreenFit::Auto, ScreenFit::Fit, ScreenFit::Stretch];

    pub fn name(self) -> &'static str {
        match self {
            ScreenFit::Auto => "auto",
            ScreenFit::Fit => "fit",
            ScreenFit::Stretch => "stretch",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            ScreenFit::Auto => "defined by scene",
            ScreenFit::Fit => "fit",
            ScreenFit::Stretch => "stretch to fill",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        Self::ALL.into_iter().find(|f| f.name().eq_ignore_ascii_case(value))
    }

    /// Whether the picture is stretched, for a scene that asks for it
    /// (`scene_stretches`) or not.
    pub fn stretches(self, scene_stretches: bool) -> bool {
        match self {
            ScreenFit::Auto => scene_stretches,
            ScreenFit::Fit => false,
            ScreenFit::Stretch => true,
        }
    }
}

/// How much of the 3D scene's lighting is worked out: shadows, the
/// screen's light in patches of the picture's colours, and the light
/// bouncing around the scene.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VrQuality {
    /// Hard shadows, the screen's light in one colour, no bounced light or
    /// ambient occlusion.
    Low,
    /// Soft shadows, the screen's light in four patches, bounced light,
    /// ambient occlusion.
    Medium,
    /// Softer shadows, the screen's light in twelve patches, bounced
    /// light, finer ambient occlusion.
    #[default]
    High,
}

impl VrQuality {
    pub const ALL: [VrQuality; 3] = [VrQuality::Low, VrQuality::Medium, VrQuality::High];

    pub fn name(self) -> &'static str {
        match self {
            VrQuality::Low => "low",
            VrQuality::Medium => "medium",
            VrQuality::High => "high",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            VrQuality::Low => "low: hard shadows, no bounced light",
            VrQuality::Medium => "medium",
            VrQuality::High => "high: soft shadows, bounced light",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        Self::ALL.into_iter().find(|q| q.name().eq_ignore_ascii_case(value))
    }
}

/// How the headset's pictures get to the OpenXR runtime.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VrGraphics {
    /// Whichever the runtime takes (`candidates`).
    #[default]
    Auto,
    /// OpenGL through GLX (Linux, under X11) or WGL (Windows).
    Gl,
    /// OpenGL through EGL (XR_MNDX_egl_enable): under Wayland, as Monado
    /// takes it.
    Egl,
    /// Vulkan, the pictures drawn with OpenGL into images Vulkan copies
    /// into the runtime's: for a runtime that takes Vulkan only.
    Vulkan,
}

impl VrGraphics {
    pub const ALL: [VrGraphics; 4] = [VrGraphics::Auto, VrGraphics::Gl, VrGraphics::Egl, VrGraphics::Vulkan];

    pub fn name(self) -> &'static str {
        match self {
            VrGraphics::Auto => "auto",
            VrGraphics::Gl => "gl",
            VrGraphics::Egl => "egl",
            VrGraphics::Vulkan => "vulkan",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            VrGraphics::Auto => "automatic",
            VrGraphics::Gl => "OpenGL (GLX/WGL)",
            VrGraphics::Egl => "OpenGL through EGL",
            VrGraphics::Vulkan => "Vulkan bridge",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        Self::ALL.into_iter().find(|g| g.name().eq_ignore_ascii_case(value))
    }
}

/// What the OpenXR runtime offers of what the headset can draw with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Offered {
    /// XR_KHR_opengl_enable.
    pub opengl: bool,
    /// XR_MNDX_egl_enable.
    pub egl: bool,
    /// XR_KHR_vulkan_enable2.
    pub vulkan: bool,
    /// XR_EXT_hand_interaction.
    pub hand_interaction: bool,
}

impl Offered {
    /// As text, for when none of it will do.
    pub fn describe(self) -> String {
        let names: Vec<&str> = [
            (self.opengl, "XR_KHR_opengl_enable"),
            (self.egl, "XR_MNDX_egl_enable"),
            (self.vulkan, "XR_KHR_vulkan_enable2"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect();
        if names.is_empty() { "none of OpenGL, EGL or Vulkan".to_string() } else { names.join(", ") }
    }
}

/// Where the program runs, for the choice of how the headset draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Windows,
    /// Linux: whether there is an X server (DISPLAY) to draw through.
    Linux { x11: bool },
}

/// The kind of OpenGL context the window has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextKind {
    Wgl,
    Glx,
    Egl,
}

/// How the headset's session is made: what it is given to draw with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Binding {
    Wgl,
    Glx,
    Egl,
    Vulkan,
}

impl Binding {
    pub fn name(self) -> &'static str {
        match self {
            Binding::Wgl => "OpenGL (WGL)",
            Binding::Glx => "OpenGL (GLX)",
            Binding::Egl => "OpenGL (EGL)",
            Binding::Vulkan => "Vulkan bridge",
        }
    }
}

/// What SDL is told before it starts, for the headset to be drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SdlHints {
    /// SDL_VIDEODRIVER=x11: GLX contexts are X11's.
    pub force_x11: bool,
    /// SDL_VIDEO_X11_FORCE_EGL=1: EGL contexts under X11 too.
    pub force_egl: bool,
}

/// What SDL is to be told, before it starts, for `wanted` with what the
/// runtime `offered` (None: no runtime to ask, which draws with GLX as it
/// always did).
pub fn startup_hints(wanted: VrGraphics, offered: Option<Offered>, platform: Platform) -> SdlHints {
    let Platform::Linux { x11 } = platform else { return SdlHints::default() };
    let glx = SdlHints { force_x11: true, force_egl: false };
    let egl = SdlHints { force_x11: false, force_egl: true };
    match (wanted, offered) {
        (VrGraphics::Gl, _) | (VrGraphics::Auto, None) => glx,
        (VrGraphics::Egl, _) => egl,
        (VrGraphics::Vulkan, _) => SdlHints::default(),
        (VrGraphics::Auto, Some(offered)) => {
            if offered.opengl && x11 {
                glx
            } else if offered.egl {
                egl
            } else {
                SdlHints::default()
            }
        }
    }
}

/// The ways to make the headset's session, in the order to try them, for
/// `wanted` with what the runtime `offered` and the window's context of
/// `kind`; or why there is none.
pub fn candidates(wanted: VrGraphics, offered: Offered, kind: ContextKind) -> Result<Vec<Binding>, String> {
    // Windows draws with WGL, as it always did.
    if kind == ContextKind::Wgl {
        return if offered.opengl {
            Ok(vec![Binding::Wgl])
        } else {
            Err(format!("the OpenXR runtime can't draw with OpenGL (it offers {})", offered.describe()))
        };
    }
    let glx = (kind == ContextKind::Glx && offered.opengl).then_some(Binding::Glx);
    let egl = (kind == ContextKind::Egl && offered.egl && offered.opengl).then_some(Binding::Egl);
    let vulkan = offered.vulkan.then_some(Binding::Vulkan);
    let list: Vec<Binding> = match wanted {
        VrGraphics::Auto => [glx, egl, vulkan].into_iter().flatten().collect(),
        VrGraphics::Gl => glx.into_iter().collect(),
        VrGraphics::Egl => egl.into_iter().collect(),
        VrGraphics::Vulkan => vulkan.into_iter().collect(),
    };
    if !list.is_empty() {
        return Ok(list);
    }
    let context = match kind {
        ContextKind::Glx => "GLX",
        ContextKind::Egl => "EGL",
        ContextKind::Wgl => "WGL",
    };
    Err(match wanted {
        VrGraphics::Gl if kind == ContextKind::Egl => {
            "graphics=gl needs X11: press F2 to keep the setting and start Rust-DOS again".to_string()
        }
        VrGraphics::Egl if kind != ContextKind::Egl => {
            "graphics=egl needs an EGL context: press F2 to keep the setting and start Rust-DOS again".to_string()
        }
        _ => format!(
            "graphics={} can't be drawn with the window's {} context (the OpenXR runtime offers {})",
            wanted.name(),
            context,
            offered.describe()
        ),
    })
}

/// An eye's image size: `percent` of the runtime's `recommended` (even,
/// if it is scaled), from 16 pixels to the runtime's `max`.
pub fn eye_size(recommended: (u32, u32), max: (u32, u32), percent: u32) -> (u32, u32) {
    let scale = |n: u32, max: u32| {
        let scaled = if percent == 100 { n } else { (n as u64 * percent as u64 / 100) as u32 & !1 };
        let max = if max == 0 { u32::MAX } else { max };
        scaled.clamp(16.min(max), max)
    };
    (scale(recommended.0, max.0), scale(recommended.1, max.1))
}

/// The part of an eye's image `size` big drawn at `percent` of it across
/// and down: even, from 16 pixels to the whole.
pub fn area(size: (u32, u32), percent: u32) -> (u32, u32) {
    let scale = |n: u32| {
        if percent >= 100 {
            return n;
        }
        ((n as u64 * percent as u64 / 100) as u32 & !1).clamp(16.min(n), n)
    };
    (scale(size.0), scale(size.1))
}

/// `resolution`'s range, in percent.
pub const RESOLUTION_MIN: u32 = 30;
pub const RESOLUTION_MAX: u32 = 150;

/// `screen_glow`'s range, in percent.
pub const SCREEN_GLOW_MAX: u32 = 400;

/// `scene_scale`'s range, in percent.
pub const SCENE_SCALE_MIN: u32 = 50;
pub const SCENE_SCALE_MAX: u32 = 200;
/// How far the seat moves each way from the scene's `spawn`, in cm.
pub const SEAT_SHIFT_MAX: i32 = 100;
/// How far the seat turns each way, in degrees.
pub const SEAT_TURN_MAX: i32 = 180;
/// The settings of the seat's shift, in the order of `VrSettings::seat`.
pub const SEAT_AXES: [&str; 3] = ["seat_right", "seat_up", "seat_forward"];

/// The `[vr]` settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VrSettings {
    pub mode: VrMode,
    /// The scene the screen is in: a glTF file (.glb or .gltf) exported
    /// from Blender, with a mesh named `screen`. None for the built-in
    /// test room.
    pub scene: Option<PathBuf>,
    pub controllers: VrControllers,
    /// The sound comes from the screen's sides (or the scene's speakers)
    /// as the viewer turns and moves.
    pub spatial_audio: bool,
    pub screen_fit: ScreenFit,
    pub quality: VrQuality,
    /// How brightly the screen lights the scene, in percent of what the
    /// scene says.
    pub screen_glow: u32,
    /// How big the scene looks in the headset, in percent: above 100 it
    /// is bigger and the viewer smaller.
    pub scene_scale: u32,
    /// Where the headset's seat is from the scene's `spawn`, in cm to the
    /// right, up and ahead (as the spawn faces).
    pub seat: [i32; 3],
    /// How far the seat is turned from the spawn's way, in degrees to the
    /// left.
    pub seat_turn: i32,
    /// How the headset's pictures get to the OpenXR runtime.
    pub graphics: VrGraphics,
    /// The eyes' images, in percent of the size the runtime recommends.
    pub resolution: u32,
}

impl Default for VrSettings {
    fn default() -> Self {
        VrSettings {
            mode: VrMode::Off,
            scene: None,
            controllers: VrControllers::Both,
            spatial_audio: true,
            screen_fit: ScreenFit::Auto,
            quality: VrQuality::High,
            screen_glow: 100,
            scene_scale: 100,
            seat: [0; 3],
            seat_turn: 0,
            graphics: VrGraphics::Auto,
            resolution: 100,
        }
    }
}

impl VrSettings {
    /// A setting of the file; relative scene paths are from `base_dir`, the
    /// file's folder.
    pub fn set(&mut self, key: &str, value: &str, base_dir: &Path) -> Result<(), String> {
        match key.to_ascii_lowercase().as_str() {
            "mode" => {
                self.mode = VrMode::parse(value)
                    .ok_or_else(|| format!("invalid mode '{}' (off, headset or desktop)", value))?;
            }
            "scene" => {
                let value = value.trim();
                self.scene = (!value.is_empty()).then(|| base_dir.join(value));
            }
            "controllers" => {
                self.controllers = VrControllers::parse(value)
                    .ok_or_else(|| format!("invalid controllers '{}' (both, pointer or gamepad)", value))?;
            }
            "spatial_audio" => {
                self.spatial_audio = match value.trim().to_ascii_lowercase().as_str() {
                    "true" | "on" | "yes" | "1" => true,
                    "false" | "off" | "no" | "0" => false,
                    _ => return Err(format!("invalid spatial_audio '{}' (true or false)", value)),
                }
            }
            "screen_fit" => {
                self.screen_fit = ScreenFit::parse(value)
                    .ok_or_else(|| format!("invalid screen_fit '{}' (auto, fit or stretch)", value))?;
            }
            "quality" => {
                self.quality = VrQuality::parse(value)
                    .ok_or_else(|| format!("invalid quality '{}' (low, medium or high)", value))?;
            }
            "screen_glow" => {
                self.screen_glow = value
                    .trim()
                    .trim_end_matches('%')
                    .parse::<u32>()
                    .ok()
                    .filter(|g| *g <= SCREEN_GLOW_MAX)
                    .ok_or_else(|| format!("invalid screen_glow '{}' (0 to {} percent)", value, SCREEN_GLOW_MAX))?;
            }
            "scene_scale" => {
                self.scene_scale = value
                    .trim()
                    .trim_end_matches('%')
                    .parse::<u32>()
                    .ok()
                    .filter(|s| (SCENE_SCALE_MIN..=SCENE_SCALE_MAX).contains(s))
                    .ok_or_else(|| {
                        format!("invalid scene_scale '{}' ({} to {} percent)", value, SCENE_SCALE_MIN, SCENE_SCALE_MAX)
                    })?;
            }
            key @ ("seat_right" | "seat_up" | "seat_forward") => {
                let axis = SEAT_AXES.iter().position(|&a| a == key).unwrap_or(0);
                self.seat[axis] = parse_signed(value, "cm", SEAT_SHIFT_MAX)
                    .ok_or_else(|| format!("invalid {} '{}' (-{} to {} cm)", key, value, SEAT_SHIFT_MAX, SEAT_SHIFT_MAX))?;
            }
            "seat_turn" => {
                self.seat_turn = parse_signed(value, "°", SEAT_TURN_MAX).ok_or_else(|| {
                    format!("invalid seat_turn '{}' (-{} to {} degrees)", value, SEAT_TURN_MAX, SEAT_TURN_MAX)
                })?;
            }
            "graphics" => {
                self.graphics = VrGraphics::parse(value)
                    .ok_or_else(|| format!("invalid graphics '{}' (auto, gl, egl or vulkan)", value))?;
            }
            "resolution" => {
                self.resolution = value
                    .trim()
                    .trim_end_matches('%')
                    .parse::<u32>()
                    .ok()
                    .filter(|r| (RESOLUTION_MIN..=RESOLUTION_MAX).contains(r))
                    .ok_or_else(|| {
                        format!("invalid resolution '{}' ({} to {} percent)", value, RESOLUTION_MIN, RESOLUTION_MAX)
                    })?;
            }
            _ => return Err(format!("unknown setting '{}'", key)),
        }
        Ok(())
    }

    pub fn entries(&self) -> Vec<(&'static str, Option<String>)> {
        vec![
            ("mode", Some(self.mode.name().to_string())),
            ("scene", self.scene.as_ref().map(|path| path.display().to_string())),
            ("controllers", Some(self.controllers.name().to_string())),
            ("spatial_audio", Some(self.spatial_audio.to_string())),
            ("screen_fit", Some(self.screen_fit.name().to_string())),
            ("quality", Some(self.quality.name().to_string())),
            ("screen_glow", Some(self.screen_glow.to_string())),
            ("scene_scale", Some(self.scene_scale.to_string())),
            (SEAT_AXES[0], Some(self.seat[0].to_string())),
            (SEAT_AXES[1], Some(self.seat[1].to_string())),
            (SEAT_AXES[2], Some(self.seat[2].to_string())),
            ("seat_turn", Some(self.seat_turn.to_string())),
            ("graphics", Some(self.graphics.name().to_string())),
            ("resolution", Some(self.resolution.to_string())),
        ]
    }
}

/// A whole number from -`max` to `max`, with `unit` after it or not.
fn parse_signed(value: &str, unit: &str, max: i32) -> Option<i32> {
    value.trim().trim_end_matches(unit).trim().parse::<i32>().ok().filter(|n| n.abs() <= max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_it_writes() {
        let mut s = VrSettings::default();
        s.set("Mode", "Desktop", Path::new("/cfg")).unwrap();
        s.set("scene", "rooms/den.glb", Path::new("/cfg")).unwrap();
        s.set("controllers", "Pointer", Path::new("/cfg")).unwrap();
        s.set("spatial_audio", "off", Path::new("/cfg")).unwrap();
        s.set("screen_fit", "Stretch", Path::new("/cfg")).unwrap();
        assert_eq!(s.screen_fit, ScreenFit::Stretch);
        assert!(s.set("screen_fit", "zoom", Path::new("/cfg")).is_err());
        s.set("quality", "Medium", Path::new("/cfg")).unwrap();
        s.set("screen_glow", "250%", Path::new("/cfg")).unwrap();
        assert_eq!((s.quality, s.screen_glow), (VrQuality::Medium, 250));
        assert!(s.set("screen_glow", "401", Path::new("/cfg")).is_err());
        assert!(s.set("quality", "ultra", Path::new("/cfg")).is_err());
        s.set("scene_scale", "110%", Path::new("/cfg")).unwrap();
        s.set("seat_right", "-5", Path::new("/cfg")).unwrap();
        s.set("seat_up", "3 cm", Path::new("/cfg")).unwrap();
        s.set("seat_forward", "12", Path::new("/cfg")).unwrap();
        s.set("seat_turn", "-15", Path::new("/cfg")).unwrap();
        assert_eq!((s.scene_scale, s.seat, s.seat_turn), (110, [-5, 3, 12], -15));
        assert!(s.set("scene_scale", "20", Path::new("/cfg")).is_err());
        assert!(s.set("seat_up", "101", Path::new("/cfg")).is_err());
        assert!(s.set("seat_turn", "181", Path::new("/cfg")).is_err());
        s.set("graphics", "EGL", Path::new("/cfg")).unwrap();
        s.set("resolution", "70%", Path::new("/cfg")).unwrap();
        assert_eq!((s.graphics, s.resolution), (VrGraphics::Egl, 70));
        assert!(s.set("graphics", "d3d", Path::new("/cfg")).is_err());
        assert!(s.set("resolution", "29", Path::new("/cfg")).is_err());
        assert!(s.set("resolution", "151", Path::new("/cfg")).is_err());
        assert_eq!(s.controllers, VrControllers::Pointer);
        assert!(!s.spatial_audio);
        assert_eq!(s.mode, VrMode::Desktop);
        assert_eq!(s.scene.as_deref(), Some(Path::new("/cfg/rooms/den.glb")));
        assert!(s.set("mode", "holodeck", Path::new("/cfg")).is_err());
        assert!(s.set("eye_height", "120", Path::new("/cfg")).is_err());
        let mut again = VrSettings::default();
        for (key, value) in s.entries() {
            again.set(key, &value.unwrap(), Path::new("/elsewhere")).unwrap();
        }
        assert_eq!(again, s);
    }

    const ALL: Offered = Offered { opengl: true, egl: true, vulkan: true, hand_interaction: false };
    const GL_ONLY: Offered = Offered { opengl: true, egl: false, vulkan: false, hand_interaction: false };
    const VK_ONLY: Offered = Offered { opengl: false, egl: false, vulkan: true, hand_interaction: false };

    #[test]
    fn windows_always_draws_with_wgl() {
        for wanted in VrGraphics::ALL {
            assert_eq!(startup_hints(wanted, Some(ALL), Platform::Windows), SdlHints::default());
            assert_eq!(candidates(wanted, ALL, ContextKind::Wgl), Ok(vec![Binding::Wgl]));
        }
        assert!(candidates(VrGraphics::Auto, VK_ONLY, ContextKind::Wgl).is_err());
    }

    #[test]
    fn sdl_is_told_what_the_binding_needs() {
        let linux = Platform::Linux { x11: true };
        let glx = SdlHints { force_x11: true, force_egl: false };
        let egl = SdlHints { force_x11: false, force_egl: true };
        // Without a runtime to ask, and for SteamVR, as before: X11.
        assert_eq!(startup_hints(VrGraphics::Auto, None, linux), glx);
        assert_eq!(startup_hints(VrGraphics::Auto, Some(GL_ONLY), linux), glx);
        assert_eq!(startup_hints(VrGraphics::Auto, Some(ALL), linux), glx);
        assert_eq!(startup_hints(VrGraphics::Gl, Some(VK_ONLY), linux), glx);
        assert_eq!(startup_hints(VrGraphics::Egl, Some(ALL), linux), egl);
        assert_eq!(startup_hints(VrGraphics::Vulkan, Some(ALL), linux), SdlHints::default());
        assert_eq!(startup_hints(VrGraphics::Auto, Some(VK_ONLY), linux), SdlHints::default());
        // No X server: EGL if the runtime takes it.
        let wayland = Platform::Linux { x11: false };
        assert_eq!(startup_hints(VrGraphics::Auto, Some(ALL), wayland), egl);
        assert_eq!(startup_hints(VrGraphics::Auto, Some(GL_ONLY), wayland), SdlHints::default());
    }

    #[test]
    fn bindings_are_tried_in_order() {
        use Binding::*;
        assert_eq!(candidates(VrGraphics::Auto, ALL, ContextKind::Glx), Ok(vec![Glx, Vulkan]));
        assert_eq!(candidates(VrGraphics::Auto, ALL, ContextKind::Egl), Ok(vec![Egl, Vulkan]));
        assert_eq!(candidates(VrGraphics::Auto, GL_ONLY, ContextKind::Glx), Ok(vec![Glx]));
        assert_eq!(candidates(VrGraphics::Auto, VK_ONLY, ContextKind::Egl), Ok(vec![Vulkan]));
        assert_eq!(candidates(VrGraphics::Gl, ALL, ContextKind::Glx), Ok(vec![Glx]));
        assert_eq!(candidates(VrGraphics::Egl, ALL, ContextKind::Egl), Ok(vec![Egl]));
        assert_eq!(candidates(VrGraphics::Vulkan, ALL, ContextKind::Glx), Ok(vec![Vulkan]));
        // What the window's context or the runtime can't do.
        assert!(candidates(VrGraphics::Gl, ALL, ContextKind::Egl).unwrap_err().contains("X11"));
        assert!(candidates(VrGraphics::Egl, ALL, ContextKind::Glx).unwrap_err().contains("EGL context"));
        assert!(candidates(VrGraphics::Auto, GL_ONLY, ContextKind::Egl).is_err());
        assert!(candidates(VrGraphics::Vulkan, GL_ONLY, ContextKind::Glx).unwrap_err().contains("XR_KHR_opengl_enable"));
        let none = Offered::default();
        assert!(candidates(VrGraphics::Auto, none, ContextKind::Glx).unwrap_err().contains("none of"));
    }

    #[test]
    fn eye_sizes_scale_evenly_within_the_runtime_limits() {
        assert_eq!(eye_size((2016, 2240), (4096, 4096), 100), (2016, 2240));
        assert_eq!(eye_size((2016, 2240), (4096, 4096), 70), (1410, 1568));
        assert_eq!(eye_size((1001, 999), (0, 0), 100), (1001, 999));
        assert_eq!(eye_size((1001, 999), (0, 0), 50), (500, 498));
        assert_eq!(eye_size((2016, 2240), (2500, 2500), 150), (2500, 2500));
        assert_eq!(eye_size((20, 20), (4096, 4096), 30), (16, 16));
    }
}
