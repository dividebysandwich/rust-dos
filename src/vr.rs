//! The `[vr]` settings: the picture on a screen in a 3D scene, in a VR
//! headset or in the window with a camera to fly around.

use std::path::{Path, PathBuf};

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

/// The `[vr]` settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VrSettings {
    pub mode: VrMode,
    /// The scene the screen is in: a glTF file (.glb or .gltf) exported
    /// from Blender, with a mesh named `screen`. None for the built-in
    /// test room.
    pub scene: Option<PathBuf>,
}

impl Default for VrSettings {
    fn default() -> Self {
        VrSettings { mode: VrMode::Off, scene: None }
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
            _ => return Err(format!("unknown setting '{}'", key)),
        }
        Ok(())
    }

    pub fn entries(&self) -> Vec<(&'static str, Option<String>)> {
        vec![
            ("mode", Some(self.mode.name().to_string())),
            ("scene", self.scene.as_ref().map(|path| path.display().to_string())),
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
