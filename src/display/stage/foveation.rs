//! Foveated rendering on the Steam Frame, through Valve's own fragment
//! density map layers: their OpenXR half (implicit, always loaded) learns
//! the size of the headset's images and the eyes' gaze, their Vulkan half
//! (explicit, which Steam asks for for its games) lays a density map over
//! every render pass of that size. The scene is drawn with OpenGL, which
//! on the Frame is Zink, Vulkan in this process: asked for before SDL makes
//! the window's context, when Zink makes its Vulkan instance, the layers
//! foveate the eyes as they do Steam's games.

use super::xr::log;
use rust_dos::vr::{FDM_LAYERS, VrFoveation, VrSettings, fdm_env};
use std::path::PathBuf;
use std::sync::OnceLock;

/// The level asked of the layers at the start, if any.
static ACTIVE: OnceLock<Option<VrFoveation>> = OnceLock::new();

/// The Vulkan layer's manifest, where it is installed.
pub fn manifest() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(home) = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
    }) {
        dirs.push(home);
    }
    let data = std::env::var("XDG_DATA_DIRS").unwrap_or_default();
    let data = if data.is_empty() { "/usr/local/share:/usr/share".to_string() } else { data };
    dirs.extend(data.split(':').filter(|d| !d.is_empty()).map(PathBuf::from));
    dirs.push(PathBuf::from("/etc"));
    dirs.into_iter()
        .flat_map(|dir| ["explicit_layer.d", "implicit_layer.d"].map(|kind| dir.join("vulkan").join(kind)))
        .map(|dir| dir.join("VkLayer_VALVE_fdm_injection.json"))
        .find(|path| path.is_file())
}

/// Ask Valve's layers for foveation as `settings` want it, where they are
/// installed: before SDL starts, and before the OpenXR runtime is loaded.
/// RUST_DOS_VR_FDM_ENV="K=V;K=V" sets others, to try them on the headset.
pub fn enable(settings: &VrSettings) {
    let manifest = manifest();
    let level = settings.foveation.level(manifest.is_some());
    let extra = std::env::var("RUST_DOS_VR_FDM_ENV").unwrap_or_default();
    let env = fdm_env(level, |key| std::env::var(key).ok(), &extra);
    for (key, value) in &env {
        // SAFETY: this runs before SDL, the OpenXR loader and the
        // emulator's threads start: no other thread reads the environment.
        unsafe { std::env::set_var(key, value) };
    }
    let asked = std::env::var("VK_INSTANCE_LAYERS").is_ok_and(|l| l.contains("VK_LAYER_VALVE_fdm_injection"));
    let active = level.filter(|_| asked);
    let _ = ACTIVE.set(active);
    let set: Vec<String> = env.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
    log::line(match (settings.foveation, &manifest, active) {
        (VrFoveation::Off, _, _) => "Foveation is off".to_string(),
        (_, None, _) => "Foveation: Valve's density map layer isn't installed (it is on a Steam Frame)".to_string(),
        (_, Some(_), None) => format!("Foveation: Valve's density map layer is turned off ({} is set)", rust_dos::vr::FDM_DISABLE),
        (_, Some(path), Some(level)) => format!(
            "Foveation {} through {} ({}); {}",
            level.name(),
            FDM_LAYERS,
            path.display(),
            if set.is_empty() { "all set already".to_string() } else { format!("set {}", set.join(" ")) }
        ),
    });
}

/// The level the layers were asked for at the start: None if they
/// weren't (switched on later, the headset isn't foveated until the next
/// start, since Zink's Vulkan instance is made without them).
pub fn active() -> Option<VrFoveation> {
    ACTIVE.get().copied().flatten()
}
