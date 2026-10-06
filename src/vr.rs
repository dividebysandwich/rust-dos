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
            ScreenFit::Auto => "as the scene says",
            ScreenFit::Fit => "keep its shape",
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
}

impl Default for VrSettings {
    fn default() -> Self {
        VrSettings {
            mode: VrMode::Off,
            scene: None,
            controllers: VrControllers::Both,
            spatial_audio: true,
            screen_fit: ScreenFit::Auto,
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
        ]
    }
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
}
