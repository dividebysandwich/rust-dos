//! A VR headset through OpenXR (SteamVR, Monado): a session drawing with
//! the OpenGL context current on the headset's thread, a swapchain for each
//! eye, the frames, which the headset paces, and the controllers.

use super::camera::fov_projection;
use super::controls::{Hand, Tracking};
use super::render::View;
use super::scene::Spawn;
use glam::{Mat4, Quat, Vec2, Vec3};
use glow::HasContext;
use openxr as xr;

const VIEW: xr::ViewConfigurationType = xr::ViewConfigurationType::PRIMARY_STEREO;

struct Eye {
    swapchain: xr::Swapchain<xr::OpenGL>,
    /// A framebuffer for each of the swapchain's images.
    framebuffers: Vec<glow::Framebuffer>,
    size: (u32, u32),
}

/// The controllers' actions, for both hands.
struct Controls {
    set: xr::ActionSet,
    /// (Kept with the spaces made of it.)
    _aim: xr::Action<xr::Posef>,
    select: xr::Action<bool>,
    squeeze: xr::Action<bool>,
    stick: xr::Action<xr::Vector2f>,
    primary: xr::Action<bool>,
    secondary: xr::Action<bool>,
    menu: xr::Action<bool>,
    /// /user/hand/left and /user/hand/right.
    hands: [xr::Path; 2],
    /// Where each hand points from, its -Z the way.
    spaces: Vec<xr::Space>,
}

pub struct Xr {
    // Dropped first: what belongs to the session, then the session, then
    // the instance.
    controls: Option<Controls>,
    eyes: Vec<Eye>,
    space: xr::Space,
    head: xr::Space,
    stream: xr::FrameStream<xr::OpenGL>,
    waiter: xr::FrameWaiter,
    session: xr::Session<xr::OpenGL>,
    instance: xr::Instance,
    /// The library OpenGL's native handles came from, kept loaded.
    _gl: libloading::Library,
    /// Where the space is in the runtime's own LOCAL space: moved by
    /// recentring.
    space_pose: xr::Posef,
    blend: xr::EnvironmentBlendMode,
    /// The eyes' swapchains encode linear light as sRGB themselves.
    srgb: bool,
    running: bool,
    /// The frame waited for and not drawn yet.
    pending: Option<xr::FrameState>,
    recenter: bool,
    events: xr::EventDataBuffer,
    description: String,
    /// Why the session can't go on, after a call failed in a way that
    /// ends it (`poll` says so).
    failed: Option<String>,
}

/// An OpenXR error as text.
fn err(what: &str) -> impl Fn(xr::sys::Result) -> String + '_ {
    move |e| format!("{}: {}", what, e)
}

/// Whether the session can't go on after a call failed with `e`: calling
/// on into a runtime that failed or lost the session is what crashes.
fn ends_session(e: xr::sys::Result) -> bool {
    matches!(e, xr::sys::Result::ERROR_RUNTIME_FAILURE | xr::sys::Result::ERROR_SESSION_LOST | xr::sys::Result::ERROR_INSTANCE_LOST)
}

impl Xr {
    /// Talk to the OpenXR runtime and start a session on its headset,
    /// drawn with the OpenGL context current on this thread.
    pub fn new(gl: &glow::Context) -> Result<Self, String> {
        // SAFETY: the loader is the Khronos one, or one that conforms.
        let entry = unsafe { xr::Entry::load(&()) }.map_err(|e| {
            format!(
                "the OpenXR loader can't be loaded ({}): install your system's openxr package, \
                 or put openxr_loader.dll beside rust-dos.exe",
                e
            )
        })?;
        let available = entry.enumerate_extensions().map_err(err("no OpenXR runtime"))?;
        if !available.khr_opengl_enable {
            return Err("the OpenXR runtime can't draw with OpenGL".into());
        }
        let mut extensions = xr::ExtensionSet::default();
        extensions.khr_opengl_enable = true;
        let app = xr::ApplicationInfo {
            application_name: "Rust-DOS",
            application_version: 0,
            engine_name: "rust-dos",
            engine_version: 0,
            api_version: xr::Version::new(1, 0, 0),
        };
        let instance = entry.create_instance(&app, &extensions, &[], &()).map_err(err("OpenXR"))?;
        let runtime = instance.properties().map_err(err("OpenXR"))?;
        let system = instance.system(xr::FormFactor::HEAD_MOUNTED_DISPLAY).map_err(err("no headset"))?;
        let system_name = instance.system_properties(system).map(|p| p.system_name).unwrap_or_default();
        // Asking is required before a session, whatever the answer.
        let _ = instance.graphics_requirements::<xr::OpenGL>(system).map_err(err("OpenXR"))?;
        let (info, library) = native::binding()?;
        // SAFETY: the handles are the context current on this thread, which
        // outlives the session (the window's).
        let (session, waiter, stream) =
            unsafe { instance.create_session::<xr::OpenGL>(system, &info) }.map_err(err("the headset's session"))?;
        let blend = instance
            .enumerate_environment_blend_modes(system, VIEW)
            .ok()
            .and_then(|modes| modes.first().copied())
            .unwrap_or(xr::EnvironmentBlendMode::OPAQUE);
        let formats = session.enumerate_swapchain_formats().map_err(err("the headset's formats"))?;
        let (format, srgb) = if formats.contains(&glow::SRGB8_ALPHA8) {
            (glow::SRGB8_ALPHA8, true)
        } else if formats.contains(&glow::RGBA8) {
            (glow::RGBA8, false)
        } else {
            return Err(format!("the headset takes none of OpenGL's RGBA8 formats ({:x?})", formats));
        };
        let views = instance.enumerate_view_configuration_views(system, VIEW).map_err(err("the headset's views"))?;
        let mut eyes = Vec::new();
        for view in &views {
            let (width, height) = (view.recommended_image_rect_width, view.recommended_image_rect_height);
            let swapchain = session
                .create_swapchain(&xr::SwapchainCreateInfo {
                    create_flags: xr::SwapchainCreateFlags::EMPTY,
                    usage_flags: xr::SwapchainUsageFlags::COLOR_ATTACHMENT | xr::SwapchainUsageFlags::TRANSFER_DST,
                    format,
                    sample_count: 1,
                    width,
                    height,
                    face_count: 1,
                    array_size: 1,
                    mip_count: 1,
                })
                .map_err(err("the headset's swapchain"))?;
            let images = swapchain.enumerate_images().map_err(err("the headset's swapchain"))?;
            let mut framebuffers = Vec::new();
            for image in images {
                let texture = std::num::NonZeroU32::new(image).map(glow::NativeTexture).ok_or("an empty swapchain image")?;
                // SAFETY: see `GlScreen`.
                unsafe {
                    let framebuffer = gl.create_framebuffer()?;
                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
                    gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(texture), 0);
                    let complete = gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
                    gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                    if !complete {
                        return Err("OpenGL can't draw into the headset's images".into());
                    }
                    framebuffers.push(framebuffer);
                }
            }
            eyes.push(Eye { swapchain, framebuffers, size: (width, height) });
        }
        let space = session
            .create_reference_space(xr::ReferenceSpaceType::LOCAL, xr::Posef::IDENTITY)
            .map_err(err("the headset's space"))?;
        let head = session
            .create_reference_space(xr::ReferenceSpaceType::VIEW, xr::Posef::IDENTITY)
            .map_err(err("the headset's space"))?;
        let controls = match make_controls(&instance, &session) {
            Ok(controls) => Some(controls),
            Err(e) => {
                eprintln!("[VR] No controllers: {}", e);
                None
            }
        };
        let size = eyes.first().map_or((0, 0), |e| e.size);
        let description = format!(
            "{} through {} {}: {}x{} an eye, {}",
            if system_name.is_empty() { "A headset" } else { &system_name },
            runtime.runtime_name,
            runtime.runtime_version,
            size.0,
            size.1,
            if srgb { "sRGB" } else { "linear RGBA8" }
        );
        Ok(Xr {
            controls,
            eyes,
            space,
            head,
            stream,
            waiter,
            session,
            instance,
            _gl: library,
            space_pose: xr::Posef::IDENTITY,
            blend,
            srgb,
            running: false,
            pending: None,
            recenter: false,
            events: xr::EventDataBuffer::new(),
            description,
            failed: None,
        })
    }

    pub fn describe(&self) -> &str {
        &self.description
    }

    /// Handle the runtime's events: what is worth saying, and whether the
    /// session is over for good.
    pub fn poll(&mut self) -> (Vec<String>, bool) {
        let mut notes = Vec::new();
        if let Some(why) = self.failed.take() {
            self.running = false;
            notes.push(format!("The headset failed ({}); the window shows the scene", why));
            return (notes, true);
        }
        let mut lost = false;
        loop {
            let event = match self.instance.poll_event(&mut self.events) {
                Ok(Some(event)) => event,
                Ok(None) => break,
                Err(e) => {
                    notes.push(format!("The headset's events: {}", e));
                    break;
                }
            };
            match event {
                xr::Event::SessionStateChanged(change) => match change.state() {
                    xr::SessionState::READY => match self.session.begin(VIEW) {
                        Ok(_) => {
                            self.running = true;
                            notes.push("The headset shows the scene".to_string());
                        }
                        Err(e) => notes.push(format!("The headset can't start: {}", e)),
                    },
                    xr::SessionState::STOPPING => {
                        let _ = self.session.end();
                        self.running = false;
                        self.pending = None;
                        notes.push("The headset stopped; the window shows the scene".to_string());
                    }
                    xr::SessionState::EXITING | xr::SessionState::LOSS_PENDING => {
                        self.running = false;
                        lost = true;
                    }
                    _ => {}
                },
                xr::Event::InstanceLossPending(_) => lost = true,
                _ => {}
            }
        }
        if lost {
            notes.push("The headset is gone; the window shows the scene".to_string());
        }
        (notes, lost)
    }

    pub fn running(&self) -> bool {
        self.running
    }

    /// The size of an eye's image, and whether it is sRGB.
    pub fn eye_format(&self) -> ((u32, u32), bool) {
        (self.eyes.first().map_or((0, 0), |e| e.size), self.srgb)
    }

    /// Wait until the headset wants the next frame, if it is showing the
    /// scene; then it is to be begun, tracked and drawn.
    pub fn wait_frame(&mut self) -> bool {
        if !self.running {
            return false;
        }
        match self.waiter.wait() {
            Ok(state) => {
                self.pending = Some(state);
                true
            }
            Err(e) => {
                self.fail("waiting for the headset", e);
                false
            }
        }
    }

    /// Centre the view where the head is and looks at the next frame.
    pub fn recenter(&mut self) {
        self.recenter = true;
    }

    /// Note a failure: what it is as text, and whether it ends the
    /// session.
    fn fail(&mut self, what: &str, e: xr::sys::Result) -> String {
        let text = format!("{}: {}", what, e);
        if ends_session(e) && self.failed.is_none() {
            self.failed = Some(text.clone());
        }
        text
    }

    /// Start the frame waited for.
    pub fn begin(&mut self) -> Result<(), String> {
        let Some(state) = self.pending else { return Err("no frame waited for".into()) };
        if let Err(e) = self.stream.begin() {
            // (Not begun, it isn't to be drawn or ended.)
            self.pending = None;
            return Err(self.fail("beginning the headset's frame", e));
        }
        if std::mem::take(&mut self.recenter) {
            self.center(state.predicted_display_time);
        }
        Ok(())
    }

    /// Where the space's origin is in the scene: at the scene's `spawn`,
    /// turned its way.
    fn world(spawn: Spawn) -> Mat4 {
        Mat4::from_translation(spawn.position) * Mat4::from_rotation_y(spawn.yaw)
    }

    /// The head and the controllers at the frame's time.
    pub fn track(&mut self, spawn: Spawn) -> Tracking {
        let mut tracking = Tracking::default();
        let Some(state) = self.pending else { return tracking };
        let time = state.predicted_display_time;
        let world = Self::world(spawn);
        let valid = xr::SpaceLocationFlags::POSITION_VALID | xr::SpaceLocationFlags::ORIENTATION_VALID;
        if let Ok(head) = self.head.locate(&self.space, time)
            && head.location_flags.contains(valid)
        {
            tracking.head = Some(world * pose_matrix(head.pose));
        }
        let Some(controls) = &self.controls else { return tracking };
        if self.session.sync_actions(&[xr::ActiveActionSet::new(&controls.set)]).is_err() {
            return tracking;
        }
        for (i, (&path, space)) in controls.hands.iter().zip(&controls.spaces).enumerate() {
            let Ok(location) = space.locate(&self.space, time) else { continue };
            if !location.location_flags.contains(valid) {
                continue;
            }
            let held = |action: &xr::Action<bool>| {
                action.state(&self.session, path).is_ok_and(|s| s.is_active && s.current_state)
            };
            let stick = controls
                .stick
                .state(&self.session, path)
                .ok()
                .filter(|s| s.is_active)
                .map_or(Vec2::ZERO, |s| Vec2::new(s.current_state.x, s.current_state.y));
            tracking.hands[i] = Some(Hand {
                aim: world * pose_matrix(location.pose),
                select: held(&controls.select),
                squeeze: held(&controls.squeeze),
                primary: held(&controls.primary),
                secondary: held(&controls.secondary),
                menu: held(&controls.menu),
                stick,
            });
        }
        tracking
    }

    /// Draw the frame begun: `draw` gets each eye's number, view, size,
    /// whether its images are sRGB, and the framebuffer to draw it into.
    /// The scene's `spawn` is where the space's origin is. The frame is
    /// ended whatever fails, with nothing in it if the eyes aren't drawn.
    pub fn draw(
        &mut self,
        spawn: Spawn,
        draw: impl FnMut(usize, View, (u32, u32), bool, glow::Framebuffer),
    ) -> Result<(), String> {
        let Some(state) = self.pending.take() else { return Ok(()) };
        let time = state.predicted_display_time;
        let drawn = if state.should_render { self.draw_eyes(time, spawn, draw).map(Some) } else { Ok(None) };
        let failed = drawn.as_ref().err().map(|&(what, e)| self.fail(what, e));
        let ended = match drawn.ok().flatten() {
            Some(views) => {
                let projection_views: Vec<_> = views
                    .iter()
                    .zip(&self.eyes)
                    .map(|(view, eye)| {
                        let rect = xr::Rect2Di {
                            offset: xr::Offset2Di { x: 0, y: 0 },
                            extent: xr::Extent2Di { width: eye.size.0 as i32, height: eye.size.1 as i32 },
                        };
                        xr::CompositionLayerProjectionView::new().pose(view.pose).fov(view.fov).sub_image(
                            xr::SwapchainSubImage::new().swapchain(&eye.swapchain).image_array_index(0).image_rect(rect),
                        )
                    })
                    .collect();
                let layer = xr::CompositionLayerProjection::new().space(&self.space).views(&projection_views);
                self.stream.end(time, self.blend, &[&layer])
            }
            None => self.stream.end(time, self.blend, &[]),
        };
        if let Err(e) = ended {
            let text = self.fail("ending the headset's frame", e);
            return Err(failed.unwrap_or(text));
        }
        failed.map_or(Ok(()), Err)
    }

    /// Draw each eye into an image of its swapchain: the views drawn, or
    /// what failed. An image acquired is released whatever fails after.
    fn draw_eyes(
        &mut self,
        time: xr::Time,
        spawn: Spawn,
        mut draw: impl FnMut(usize, View, (u32, u32), bool, glow::Framebuffer),
    ) -> Result<Vec<xr::View>, (&'static str, xr::sys::Result)> {
        let (_, views) = self.session.locate_views(VIEW, time, &self.space).map_err(|e| ("the headset's views", e))?;
        if views.len() < self.eyes.len() {
            return Err(("the headset's views", xr::sys::Result::ERROR_VALIDATION_FAILURE));
        }
        let world = Self::world(spawn);
        for (index, (view, eye)) in views.iter().zip(&mut self.eyes).enumerate() {
            let image = eye.swapchain.acquire_image().map_err(|e| ("acquiring the headset's image", e))?;
            if let Err(e) = eye.swapchain.wait_image(xr::Duration::INFINITE) {
                let _ = eye.swapchain.release_image();
                return Err(("waiting for the headset's image", e));
            }
            let fov = view.fov;
            let eye_view = View {
                view: (world * pose_matrix(view.pose)).inverse(),
                projection: fov_projection(fov.angle_left, fov.angle_right, fov.angle_up, fov.angle_down),
            };
            if let Some(&framebuffer) = eye.framebuffers.get(image as usize) {
                draw(index, eye_view, eye.size, self.srgb, framebuffer);
            }
            eye.swapchain.release_image().map_err(|e| ("releasing the headset's image", e))?;
        }
        Ok(views)
    }

    /// Move the space so that its origin is where the head is at `time`,
    /// facing where it faces (level).
    fn center(&mut self, time: xr::Time) {
        let Ok(location) = self.head.locate(&self.space, time) else { return };
        let valid = xr::SpaceLocationFlags::POSITION_VALID | xr::SpaceLocationFlags::ORIENTATION_VALID;
        if !location.location_flags.contains(valid) {
            return;
        }
        // The head in the runtime's LOCAL space: through the space's pose.
        let head = pose_matrix(self.space_pose) * pose_matrix(location.pose);
        let (_, rotation, position) = head.to_scale_rotation_translation();
        let forward = rotation * Vec3::NEG_Z;
        let yaw = (-forward.x).atan2(-forward.z);
        let level = Quat::from_rotation_y(yaw);
        let pose = xr::Posef {
            orientation: xr::Quaternionf { x: level.x, y: level.y, z: level.z, w: level.w },
            position: xr::Vector3f { x: position.x, y: position.y, z: position.z },
        };
        match self.session.create_reference_space(xr::ReferenceSpaceType::LOCAL, pose) {
            Ok(space) => {
                self.space = space;
                self.space_pose = pose;
            }
            Err(e) => eprintln!("[VR] Recentring: {}", e),
        }
    }
}

/// What a controller's input is bound to.
#[derive(Clone, Copy)]
enum Control {
    Aim,
    Select,
    Squeeze,
    Stick,
    Primary,
    Secondary,
    Menu,
}

/// The suggested bindings of the controllers OpenXR runtimes know, by
/// their interaction profile: the control, the hand ("left", "right" or
/// both) and the input's path under /user/hand/<hand>/input/. SteamVR lets
/// the player change them.
const PROFILES: &[(&str, &[(Control, &str, &str)])] = &[
    (
        "/interaction_profiles/khr/simple_controller",
        &[(Control::Aim, "", "aim/pose"), (Control::Select, "", "select/click"), (Control::Menu, "", "menu/click")],
    ),
    (
        "/interaction_profiles/valve/index_controller",
        &[
            (Control::Aim, "", "aim/pose"),
            (Control::Select, "", "trigger/click"),
            (Control::Squeeze, "", "squeeze/value"),
            (Control::Stick, "", "thumbstick"),
            (Control::Primary, "", "a/click"),
            (Control::Secondary, "", "b/click"),
            (Control::Menu, "left", "thumbstick/click"),
        ],
    ),
    (
        "/interaction_profiles/oculus/touch_controller",
        &[
            (Control::Aim, "", "aim/pose"),
            (Control::Select, "", "trigger/value"),
            (Control::Squeeze, "", "squeeze/value"),
            (Control::Stick, "", "thumbstick"),
            (Control::Primary, "left", "x/click"),
            (Control::Secondary, "left", "y/click"),
            (Control::Primary, "right", "a/click"),
            (Control::Secondary, "right", "b/click"),
            (Control::Menu, "left", "menu/click"),
        ],
    ),
    (
        "/interaction_profiles/htc/vive_controller",
        &[
            (Control::Aim, "", "aim/pose"),
            (Control::Select, "", "trigger/click"),
            (Control::Squeeze, "", "squeeze/click"),
            (Control::Stick, "", "trackpad"),
            (Control::Menu, "", "menu/click"),
        ],
    ),
    (
        "/interaction_profiles/microsoft/motion_controller",
        &[
            (Control::Aim, "", "aim/pose"),
            (Control::Select, "", "trigger/value"),
            (Control::Squeeze, "", "squeeze/click"),
            (Control::Stick, "", "thumbstick"),
            (Control::Menu, "", "menu/click"),
        ],
    ),
];

/// The controllers' actions, their bindings for the controllers OpenXR
/// knows, and where the hands point from.
fn make_controls(instance: &xr::Instance, session: &xr::Session<xr::OpenGL>) -> Result<Controls, String> {
    let path = |p: &str| instance.string_to_path(p).map_err(err("OpenXR"));
    let hands = [path("/user/hand/left")?, path("/user/hand/right")?];
    let set = instance.create_action_set("rustdos", "Rust-DOS", 0).map_err(err("the controllers"))?;
    let made = err("the controllers");
    let aim = set.create_action::<xr::Posef>("aim", "Point", &hands).map_err(&made)?;
    let select = set.create_action::<bool>("select", "Click (mouse left) / fire", &hands).map_err(&made)?;
    let squeeze = set.create_action::<bool>("squeeze", "Mouse right / second button", &hands).map_err(&made)?;
    let stick = set.create_action::<xr::Vector2f>("stick", "Joystick", &hands).map_err(&made)?;
    let primary = set.create_action::<bool>("primary", "Joystick button A / X", &hands).map_err(&made)?;
    let secondary = set.create_action::<bool>("secondary", "Joystick button B / Y", &hands).map_err(&made)?;
    let menu = set.create_action::<bool>("menu", "Settings window", &hands).map_err(&made)?;
    for (profile, inputs) in PROFILES {
        let mut bindings = Vec::new();
        for &(control, hand, input) in *inputs {
            for side in ["left", "right"].into_iter().filter(|side| hand.is_empty() || hand == *side) {
                let at = path(&format!("/user/hand/{}/input/{}", side, input))?;
                bindings.push(match control {
                    Control::Aim => xr::Binding::new(&aim, at),
                    Control::Select => xr::Binding::new(&select, at),
                    Control::Squeeze => xr::Binding::new(&squeeze, at),
                    Control::Stick => xr::Binding::new(&stick, at),
                    Control::Primary => xr::Binding::new(&primary, at),
                    Control::Secondary => xr::Binding::new(&secondary, at),
                    Control::Menu => xr::Binding::new(&menu, at),
                });
            }
        }
        // A runtime without the profile refuses it, and the rest still
        // count.
        if let Err(e) = instance.suggest_interaction_profile_bindings(path(profile)?, &bindings) {
            eprintln!("[VR] {}: {}", profile, e);
        }
    }
    session.attach_action_sets(&[&set]).map_err(&made)?;
    let mut spaces = Vec::new();
    for &hand in &hands {
        spaces.push(aim.create_space(session, hand, xr::Posef::IDENTITY).map_err(&made)?);
    }
    Ok(Controls { set, _aim: aim, select, squeeze, stick, primary, secondary, menu, hands, spaces })
}

fn pose_matrix(pose: xr::Posef) -> Mat4 {
    let o = pose.orientation;
    let p = pose.position;
    Mat4::from_rotation_translation(Quat::from_xyzw(o.x, o.y, o.z, o.w).normalize(), Vec3::new(p.x, p.y, p.z))
}

/// The native handles of the window's OpenGL context, which OpenXR draws
/// with too.
#[cfg(target_os = "linux")]
mod native {
    use openxr as xr;
    use std::ffi::{c_int, c_ulong, c_void};

    const GLX_SCREEN: c_int = 0x800C;
    const GLX_VISUAL_ID: c_int = 0x800B;
    const GLX_FBCONFIG_ID: c_int = 0x8013;

    pub fn binding() -> Result<(xr::opengl::SessionCreateInfo, libloading::Library), String> {
        // SAFETY: libGL is the library SDL draws with, already loaded; the
        // functions are GLX's, with GLX's signatures.
        unsafe {
            let library = libloading::Library::new("libGL.so.1")
                .or_else(|_| libloading::Library::new("libGLX.so.0"))
                .map_err(|e| format!("GLX can't be loaded: {}", e))?;
            let display = *library.get::<unsafe extern "C" fn() -> *mut c_void>(b"glXGetCurrentDisplay\0").map_err(|e| e.to_string())?;
            let drawable = *library.get::<unsafe extern "C" fn() -> c_ulong>(b"glXGetCurrentDrawable\0").map_err(|e| e.to_string())?;
            let context = *library.get::<unsafe extern "C" fn() -> *mut c_void>(b"glXGetCurrentContext\0").map_err(|e| e.to_string())?;
            let query = *library
                .get::<unsafe extern "C" fn(*mut c_void, *mut c_void, c_int, *mut c_int) -> c_int>(b"glXQueryContext\0")
                .map_err(|e| e.to_string())?;
            let configs = *library
                .get::<unsafe extern "C" fn(*mut c_void, c_int, *mut c_int) -> *mut *mut c_void>(b"glXGetFBConfigs\0")
                .map_err(|e| e.to_string())?;
            let attrib = *library
                .get::<unsafe extern "C" fn(*mut c_void, *mut c_void, c_int, *mut c_int) -> c_int>(b"glXGetFBConfigAttrib\0")
                .map_err(|e| e.to_string())?;
            let (x_display, glx_context) = (display(), context());
            if x_display.is_null() || glx_context.is_null() {
                return Err("OpenGL doesn't draw through GLX: start rust-dos under X11 (or XWayland)".into());
            }
            let (mut id, mut screen) = (0, 0);
            query(x_display, glx_context, GLX_FBCONFIG_ID, &mut id);
            query(x_display, glx_context, GLX_SCREEN, &mut screen);
            let mut count = 0;
            let list = configs(x_display, screen, &mut count);
            let mut glx_fb_config = std::ptr::null_mut();
            let mut visualid = 0;
            for i in 0..count.max(0) as usize {
                let config = *list.add(i);
                let mut value = 0;
                if attrib(x_display, config, GLX_FBCONFIG_ID, &mut value) == 0 && value == id {
                    glx_fb_config = config;
                    attrib(x_display, config, GLX_VISUAL_ID, &mut value);
                    visualid = value as u32;
                    break;
                }
            }
            // (The list is left to the end of the program: XFree is
            // Xlib's, which this doesn't load.)
            let info = xr::opengl::SessionCreateInfo::Xlib {
                x_display: x_display.cast(),
                visualid,
                glx_fb_config,
                glx_drawable: drawable(),
                glx_context,
            };
            Ok((info, library))
        }
    }
}

#[cfg(windows)]
mod native {
    use openxr as xr;

    pub fn binding() -> Result<(xr::opengl::SessionCreateInfo, libloading::Library), String> {
        // SAFETY: opengl32.dll is the library SDL draws with, already
        // loaded; the functions take nothing and return handles.
        unsafe {
            let library = libloading::Library::new("opengl32.dll").map_err(|e| format!("opengl32.dll: {}", e))?;
            let dc = *library.get::<unsafe extern "system" fn() -> isize>(b"wglGetCurrentDC\0").map_err(|e| e.to_string())?;
            let context =
                *library.get::<unsafe extern "system" fn() -> isize>(b"wglGetCurrentContext\0").map_err(|e| e.to_string())?;
            let (h_dc, h_glrc) = (dc(), context());
            if h_dc == 0 || h_glrc == 0 {
                return Err("there is no current WGL context".into());
            }
            Ok((xr::opengl::SessionCreateInfo::Windows { h_dc, h_glrc }, library))
        }
    }
}
