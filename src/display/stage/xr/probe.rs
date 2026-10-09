//! `--vr-probe`: what the OpenXR runtime and OpenGL offer the headset,
//! written to the console and `vr-probe.log` in Rust-DOS's folder, for
//! finding out on a new headset (the Steam Frame's own runtime) what works
//! before it is used. Each binding the window's context allows makes a
//! session in turn, which is closed again at once.

use super::{Opened, entry, native, offered_of, open};
use glow::HasContext;
use openxr as xr;
use rust_dos::vr::{Binding, VrGraphics, VrSettings, candidates};
use std::fs::File;
use std::io::Write;

/// The report, as it is written.
struct Report {
    file: Option<File>,
}

impl Report {
    fn line(&mut self, line: impl AsRef<str>) {
        let line = line.as_ref();
        println!("{}", line);
        if let Some(file) = &mut self.file {
            let _ = writeln!(file, "{}", line);
            // (Each line kept, should a runtime crash the program.)
            let _ = file.flush();
        }
    }

    fn section(&mut self, title: &str) {
        self.line("");
        self.line(format!("== {}", title));
    }
}

/// Names of the formats swapchains are often offered in.
fn format_name(format: u32, vulkan: bool) -> String {
    let name = if vulkan {
        match format {
            37 => "R8G8B8A8_UNORM",
            43 => "R8G8B8A8_SRGB",
            44 => "B8G8R8A8_UNORM",
            50 => "B8G8R8A8_SRGB",
            64 => "A2B10G10R10_UNORM_PACK32",
            97 => "R16G16B16A16_SFLOAT",
            109 => "R32G32B32A32_SFLOAT",
            124 => "D16_UNORM",
            126 => "D32_SFLOAT",
            129 => "D24_UNORM_S8_UINT",
            130 => "D32_SFLOAT_S8_UINT",
            _ => "",
        }
    } else {
        match format {
            0x8C43 => "SRGB8_ALPHA8",
            0x8058 => "RGBA8",
            0x8059 => "RGB10_A2",
            0x881A => "RGBA16F",
            0x8814 => "RGBA32F",
            0x8C3A => "R11F_G11F_B10F",
            0x81A5 => "DEPTH_COMPONENT16",
            0x81A6 => "DEPTH_COMPONENT24",
            0x8CAC => "DEPTH_COMPONENT32F",
            0x88F0 => "DEPTH24_STENCIL8",
            0x8CAD => "DEPTH32F_STENCIL8",
            _ => "",
        }
    };
    if name.is_empty() { format!("{:#x}", format) } else { name.to_string() }
}

/// Write the report, with the OpenGL context `gl` current on this thread
/// (the window's kind: GLX's, EGL's or WGL's).
pub fn run(gl: &glow::Context, sdl_driver: &str, settings: &VrSettings) {
    let file = rust_dos::config::user_dir().and_then(|dir| File::create(dir.join("vr-probe.log")).ok());
    let mut report = Report { file };
    report.line(format!(
        "Rust-DOS {} VR probe on {} {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    report.line(format!("[vr] graphics={} resolution={}%", settings.graphics.name(), settings.resolution));

    report.section("OpenGL");
    let kind = native::current_kind();
    report.line(format!("SDL's video driver: {}; the context is {:?}'s", sdl_driver, kind));
    // SAFETY: the context is current.
    unsafe {
        report.line(format!("Vendor: {}", gl.get_parameter_string(glow::VENDOR)));
        report.line(format!("Renderer: {}", gl.get_parameter_string(glow::RENDERER)));
        report.line(format!("Version: {}", gl.get_parameter_string(glow::VERSION)));
        report.line(format!("GLSL: {}", gl.get_parameter_string(glow::SHADING_LANGUAGE_VERSION)));
    }
    let extensions = gl.supported_extensions();
    for wanted in [
        "GL_EXT_memory_object",
        "GL_EXT_memory_object_fd",
        "GL_EXT_semaphore",
        "GL_EXT_semaphore_fd",
        "GL_OVR_multiview2",
        "GL_EXT_multisampled_render_to_texture",
        "GL_OVR_multiview_multisampled_render_to_texture",
    ] {
        report.line(format!("{}: {}", wanted, if extensions.contains(wanted) { "yes" } else { "no" }));
    }
    #[cfg(target_os = "linux")]
    if let Ok(interop) = super::gl_interop::Interop::load(extensions) {
        let (device, driver) = interop.uuids();
        let hex = |b: &[u8]| b.iter().map(|b| format!("{:02x}", b)).collect::<String>();
        report.line(format!("UUIDs: device {}, driver {}", hex(&device), hex(&driver)));
    }

    report.section("OpenXR loader and runtime");
    for name in ["XR_RUNTIME_JSON", "XR_LOADER_DEBUG"] {
        if let Some(value) = std::env::var_os(name) {
            report.line(format!("{}={}", name, value.to_string_lossy()));
        }
    }
    #[cfg(target_os = "linux")]
    {
        let home = std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".config")));
        let places = home.into_iter().chain(["/etc/xdg".into(), "/usr/share".into(), "/usr/local/share".into()]);
        for place in places {
            let path = place.join("openxr/1/active_runtime.json");
            if path.exists() {
                let target = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                report.line(format!("Active runtime: {} -> {}", path.display(), target.display()));
            }
        }
    }
    let entry = match entry() {
        Ok(entry) => entry,
        Err(e) => {
            report.line(format!("No loader: {}", e));
            return;
        }
    };
    let available = match entry.enumerate_extensions() {
        Ok(available) => available,
        Err(e) => {
            report.line(format!("No runtime: {}", e));
            return;
        }
    };
    let offered = offered_of(&available);
    let mut names: Vec<String> = available
        .names()
        .iter()
        .map(|name| String::from_utf8_lossy(name).trim_end_matches('\0').to_string())
        .collect();
    names.sort();
    report.line(format!("Extensions ({}):", names.len()));
    for name in &names {
        report.line(format!("  {}", name));
    }

    // An instance with all of the bindings the runtime has.
    let mut wanted = xr::ExtensionSet::default();
    wanted.khr_opengl_enable = available.khr_opengl_enable;
    wanted.mndx_egl_enable = available.mndx_egl_enable && available.khr_opengl_enable;
    wanted.khr_vulkan_enable2 = available.khr_vulkan_enable2;
    wanted.ext_hand_interaction = available.ext_hand_interaction;
    let app = xr::ApplicationInfo {
        application_name: "Rust-DOS probe",
        application_version: 0,
        engine_name: "rust-dos",
        engine_version: 0,
        api_version: xr::Version::new(1, 0, 0),
    };
    let instance = match entry.create_instance(&app, &wanted, &[], &()) {
        Ok(instance) => instance,
        Err(e) => {
            report.line(format!("No instance: {}", e));
            return;
        }
    };
    if let Ok(properties) = instance.properties() {
        report.line(format!("Runtime: {} {}", properties.runtime_name, properties.runtime_version));
    }

    report.section("Headset");
    let system = match instance.system(xr::FormFactor::HEAD_MOUNTED_DISPLAY) {
        Ok(system) => system,
        Err(e) => {
            report.line(format!("No headset: {}", e));
            return;
        }
    };
    if let Ok(p) = instance.system_properties(system) {
        report.line(format!("System: {} (vendor {:#x})", p.system_name, p.vendor_id));
        report.line(format!(
            "Swapchains up to {}x{}, {} layers; tracks orientation {}, position {}",
            p.graphics_properties.max_swapchain_image_width,
            p.graphics_properties.max_swapchain_image_height,
            p.graphics_properties.max_layer_count,
            p.tracking_properties.orientation_tracking,
            p.tracking_properties.position_tracking
        ));
    }
    if let Ok(configurations) = instance.enumerate_view_configurations(system) {
        report.line(format!("View configurations: {:?}", configurations));
    }
    let view = xr::ViewConfigurationType::PRIMARY_STEREO;
    let mut sizes = Vec::new();
    match instance.enumerate_view_configuration_views(system, view) {
        Ok(views) => {
            for (i, v) in views.iter().enumerate() {
                report.line(format!(
                    "Eye {}: {}x{} recommended, {}x{} at most; {} samples recommended, {} at most",
                    i,
                    v.recommended_image_rect_width,
                    v.recommended_image_rect_height,
                    v.max_image_rect_width,
                    v.max_image_rect_height,
                    v.recommended_swapchain_sample_count,
                    v.max_swapchain_sample_count
                ));
                sizes.push(rust_dos::vr::eye_size(
                    (v.recommended_image_rect_width, v.recommended_image_rect_height),
                    (v.max_image_rect_width, v.max_image_rect_height),
                    settings.resolution,
                ));
            }
        }
        Err(e) => report.line(format!("No stereo views: {}", e)),
    }
    if let Ok(modes) = instance.enumerate_environment_blend_modes(system, view) {
        report.line(format!("Blend modes: {:?}", modes));
    }
    if wanted.khr_opengl_enable {
        match instance.graphics_requirements::<xr::OpenGL>(system) {
            Ok(r) => report.line(format!(
                "OpenGL wanted: {} to {}",
                r.min_api_version_supported, r.max_api_version_supported
            )),
            Err(e) => report.line(format!("OpenGL's requirements: {}", e)),
        }
    }
    if wanted.khr_vulkan_enable2 {
        match instance.graphics_requirements::<xr::Vulkan>(system) {
            Ok(r) => report.line(format!(
                "Vulkan wanted: {} to {}",
                r.min_api_version_supported, r.max_api_version_supported
            )),
            Err(e) => report.line(format!("Vulkan's requirements: {}", e)),
        }
    }

    report.section("Sessions");
    let bindings: &[Binding] = match kind {
        rust_dos::vr::ContextKind::Wgl => &[Binding::Wgl, Binding::Vulkan],
        rust_dos::vr::ContextKind::Glx => &[Binding::Glx, Binding::Vulkan],
        rust_dos::vr::ContextKind::Egl => &[Binding::Egl, Binding::Vulkan],
    };
    for &binding in bindings {
        let offered_here = match binding {
            Binding::Wgl | Binding::Glx => wanted.khr_opengl_enable,
            Binding::Egl => wanted.mndx_egl_enable,
            Binding::Vulkan => wanted.khr_vulkan_enable2 && cfg!(target_os = "linux"),
        };
        if !offered_here {
            report.line(format!("{}: not offered", binding.name()));
            continue;
        }
        match open(binding, gl, &instance, system, &sizes) {
            Ok(Opened { session, waiter, mut frames, keep }) => {
                let ((w, h), srgb) = frames.eye_format();
                let (formats, vulkan) = frames.formats();
                let named: Vec<String> = formats.iter().map(|&f| format_name(f, vulkan)).collect();
                report.line(format!(
                    "{}: works; {}x{} an eye, {}; formats {}",
                    binding.name(),
                    w,
                    h,
                    if srgb { "sRGB" } else { "linear" },
                    named.join(" ")
                ));
                frames.delete(gl);
                // The session goes before what it was made with.
                drop((frames, waiter, session));
                drop(keep);
            }
            Err(e) => report.line(format!("{}: fails: {}", binding.name(), e)),
        }
    }
    let other = if kind == rust_dos::vr::ContextKind::Glx { "egl" } else { "gl" };
    report.line(format!(
        "(The window's {:?} context can't try the other OpenGL binding: start again with --vr-graphics {} --vr-probe.)",
        kind, other
    ));

    report.section("Choice");
    for wanted in VrGraphics::ALL {
        let choice = match candidates(wanted, offered, kind) {
            Ok(list) => list.iter().map(|b| b.name()).collect::<Vec<_>>().join(", then "),
            Err(e) => format!("none ({})", e),
        };
        report.line(format!("graphics={}: {}", wanted.name(), choice));
    }
    report.line("The controllers' profiles are in vr.log once a session has the focus.");
}
