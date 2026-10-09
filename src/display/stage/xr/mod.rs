//! A VR headset through OpenXR (SteamVR, Monado): a session drawing with
//! the OpenGL context current on the headset's thread, a swapchain for each
//! eye, the frames, which the headset paces, and the controllers.

#[cfg(target_os = "linux")]
mod egl;
mod frames;
mod gl_frames;
#[cfg(target_os = "linux")]
mod gl_interop;
pub mod log;
mod native;
pub mod probe;
mod profiles;
#[cfg(target_os = "linux")]
mod vulkan;

use super::camera::fov_projection;
use super::controls::{Hand, Tracking};
use super::render::View;
use frames::Frames;
use gl_frames::GlFrames;
use glam::{Mat4, Quat, Vec2, Vec3};
use glow::HasContext;
use openxr as xr;
use log::FrameStats;
use profiles::Control;
use rust_dos::vr::{Binding, Offered, VrGraphics};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const VIEW: xr::ViewConfigurationType = xr::ViewConfigurationType::PRIMARY_STEREO;

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
    /// The eyes' swapchains and the frames' stream.
    frames: Frames,
    space: xr::Space,
    head: xr::Space,
    waiter: xr::FrameWaiter,
    session: xr::Session<xr::AnyGraphics>,
    /// What the session draws with that outlives it.
    _keep: Keep,
    instance: xr::Instance,
    /// Where the space is in the runtime's own LOCAL space: moved by
    /// recentring.
    space_pose: xr::Posef,
    blend: xr::EnvironmentBlendMode,
    running: bool,
    /// The frame waited for and not drawn yet.
    pending: Option<xr::FrameState>,
    /// The view is to be centred on the head at the next frame it is
    /// tracked: asked for, or the session's first focus (where the runtime's
    /// own centre is, the player seldom is).
    recenter: bool,
    /// The session has had the focus.
    focused: bool,
    /// The eyes' views have been logged.
    views_logged: bool,
    events: xr::EventDataBuffer,
    description: String,
    /// Why the session can't go on, after a call failed in a way that
    /// ends it (`poll` says so).
    failed: Option<String>,
    stats: FrameStats,
    /// When the frame being drawn was begun.
    begun: Option<Instant>,
}

/// What the session is made with: how the headset's pictures get to the
/// runtime, and their size.
pub struct XrOptions {
    pub graphics: VrGraphics,
    /// The eyes' images, in percent of the size the runtime recommends.
    pub resolution: u32,
}

/// What a session draws with that is to outlive it.
enum Keep {
    /// The library the window's OpenGL handles came from, kept loaded.
    Library(#[allow(dead_code)] libloading::Library),
    /// Vulkan's device and instance, for the Vulkan bridge.
    #[cfg(target_os = "linux")]
    Vulkan(#[allow(dead_code)] Box<vulkan::Device>),
}

/// A session made with one of the bindings.
struct Opened {
    session: xr::Session<xr::AnyGraphics>,
    waiter: xr::FrameWaiter,
    frames: Frames,
    keep: Keep,
}

/// The OpenXR loader, loaded once and kept: whether the runtime's library
/// is there doesn't change while the program runs.
fn entry() -> Result<&'static xr::Entry, String> {
    static ENTRY: OnceLock<Result<xr::Entry, String>> = OnceLock::new();
    ENTRY
        .get_or_init(|| {
            // SAFETY: the loader is the Khronos one, or one that conforms.
            let loaded = unsafe { xr::Entry::load(&()) };
            // Else the one the Linux tarballs ship beside rust-dos, which
            // the dynamic loader doesn't look for there.
            #[cfg(target_os = "linux")]
            let loaded = loaded.or_else(|e| {
                let beside = std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.join("libopenxr_loader.so.1")));
                match beside {
                    // SAFETY: as above.
                    Some(path) if path.is_file() => unsafe { xr::Entry::load_from(&path, &()) },
                    _ => Err(e),
                }
            });
            loaded.map_err(|e| {
                format!(
                    "the OpenXR loader can't be loaded ({}): install your system's openxr package, \
                     or put {} beside rust-dos",
                    e,
                    if cfg!(windows) { "openxr_loader.dll" } else { "libopenxr_loader.so.1" }
                )
            })
        })
        .as_ref()
        .map_err(Clone::clone)
}

fn offered_of(set: &xr::ExtensionSet) -> Offered {
    Offered {
        opengl: set.khr_opengl_enable,
        egl: set.mndx_egl_enable,
        vulkan: set.khr_vulkan_enable2,
        hand_interaction: set.ext_hand_interaction,
    }
}

/// What the OpenXR runtime offers to draw with, asked before SDL starts
/// (which it may need to be told of).
pub fn offered() -> Result<Offered, String> {
    entry()?.enumerate_extensions().map(|set| offered_of(&set)).map_err(err("no OpenXR runtime"))
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
    /// drawn with the OpenGL context current on this thread: through the
    /// first of the bindings `options` and the runtime allow that works.
    pub fn new(gl: &glow::Context, options: &XrOptions) -> Result<Self, String> {
        let entry = entry()?;
        let available = entry.enumerate_extensions().map_err(err("no OpenXR runtime"))?;
        let offered = offered_of(&available);
        let kind = native::current_kind();
        let bindings = rust_dos::vr::candidates(options.graphics, offered, kind)?;
        let names: Vec<&str> = bindings.iter().map(|b| b.name()).collect();
        log::line(format!(
            "The runtime offers {}; the window's context is {:?}; graphics={} tries {}",
            offered.describe(),
            kind,
            options.graphics.name(),
            names.join(", then ")
        ));
        let mut extensions = xr::ExtensionSet::default();
        for binding in &bindings {
            match binding {
                Binding::Wgl | Binding::Glx => extensions.khr_opengl_enable = true,
                Binding::Egl => {
                    extensions.khr_opengl_enable = true;
                    extensions.mndx_egl_enable = true;
                }
                Binding::Vulkan => extensions.khr_vulkan_enable2 = true,
            }
        }
        // (Windows asks for what it always did.)
        #[cfg(not(windows))]
        {
            extensions.ext_hand_interaction = available.ext_hand_interaction;
        }
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
        let blend = instance
            .enumerate_environment_blend_modes(system, VIEW)
            .ok()
            .and_then(|modes| modes.first().copied())
            .unwrap_or(xr::EnvironmentBlendMode::OPAQUE);
        let views = instance.enumerate_view_configuration_views(system, VIEW).map_err(err("the headset's views"))?;
        let sizes: Vec<_> = views
            .iter()
            .map(|view| {
                rust_dos::vr::eye_size(
                    (view.recommended_image_rect_width, view.recommended_image_rect_height),
                    (view.max_image_rect_width, view.max_image_rect_height),
                    options.resolution,
                )
            })
            .collect();
        if let Some(view) = views.first() {
            log::line(format!(
                "{} through {} {}: {}x{} an eye recommended, {}x{} at most; {}%",
                if system_name.is_empty() { "A headset" } else { &system_name },
                runtime.runtime_name,
                runtime.runtime_version,
                view.recommended_image_rect_width,
                view.recommended_image_rect_height,
                view.max_image_rect_width,
                view.max_image_rect_height,
                options.resolution
            ));
        }
        let mut failures = Vec::new();
        let mut opened = None;
        for &binding in &bindings {
            match open(binding, gl, &instance, system, &sizes) {
                Ok(made) => {
                    opened = Some((binding, made));
                    break;
                }
                Err(e) => {
                    log::line(format!("{} can't be used: {}", binding.name(), e));
                    failures.push(format!("{}: {}", binding.name(), e));
                }
            }
        }
        let Some((binding, Opened { session, waiter, frames, keep })) = opened else {
            return Err(failures.join("; "));
        };
        let space = session
            .create_reference_space(xr::ReferenceSpaceType::LOCAL, xr::Posef::IDENTITY)
            .map_err(err("the headset's space"))?;
        let head = session
            .create_reference_space(xr::ReferenceSpaceType::VIEW, xr::Posef::IDENTITY)
            .map_err(err("the headset's space"))?;
        let controls = match make_controls(&instance, &session, extensions.ext_hand_interaction) {
            Ok(controls) => Some(controls),
            Err(e) => {
                log::line(format!("No controllers: {}", e));
                None
            }
        };
        let ((width, height), srgb) = frames.eye_format();
        let description = format!(
            "{} through {} {} ({}): {}x{} an eye, {}",
            if system_name.is_empty() { "A headset" } else { &system_name },
            runtime.runtime_name,
            runtime.runtime_version,
            binding.name(),
            width,
            height,
            if srgb { "sRGB" } else { "linear RGBA8" }
        );
        Ok(Xr {
            controls,
            frames,
            space,
            head,
            waiter,
            session,
            _keep: keep,
            instance,
            space_pose: xr::Posef::IDENTITY,
            blend,
            running: false,
            pending: None,
            recenter: false,
            focused: false,
            views_logged: false,
            events: xr::EventDataBuffer::new(),
            description,
            failed: None,
            stats: FrameStats::default(),
            begun: None,
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
        // (Taken out while its events are read.)
        let mut events = std::mem::replace(&mut self.events, xr::EventDataBuffer::new());
        loop {
            let event = match self.instance.poll_event(&mut events) {
                Ok(Some(event)) => event,
                Ok(None) => break,
                Err(e) => {
                    notes.push(format!("The headset's events: {}", e));
                    break;
                }
            };
            match event {
                xr::Event::SessionStateChanged(change) => {
                    log::line(format!("The session is {:?}", change.state()));
                    self.session_state(change.state(), &mut notes, &mut lost);
                }
                xr::Event::InteractionProfileChanged(_) => self.log_profiles(),
                xr::Event::InstanceLossPending(_) => lost = true,
                _ => {}
            }
        }
        self.events = events;
        if lost {
            notes.push("The headset is gone; the window shows the scene".to_string());
        }
        (notes, lost)
    }

    /// The session went into `state`.
    fn session_state(&mut self, state: xr::SessionState, notes: &mut Vec<String>, lost: &mut bool) {
        match state {
            xr::SessionState::READY => match self.session.begin(VIEW) {
                Ok(_) => {
                    self.running = true;
                    notes.push("The headset shows the scene".to_string());
                }
                Err(e) => notes.push(format!("The headset can't start: {}", e)),
            },
            xr::SessionState::FOCUSED if !self.focused => {
                self.focused = true;
                self.recenter = true;
            }
            xr::SessionState::STOPPING => {
                let _ = self.session.end();
                self.running = false;
                self.pending = None;
                notes.push("The headset stopped; the window shows the scene".to_string());
            }
            xr::SessionState::EXITING | xr::SessionState::LOSS_PENDING => {
                self.running = false;
                *lost = true;
            }
            _ => {}
        }
    }

    /// Say which controllers (interaction profiles) the hands are, as the
    /// runtime has them now: how a headset's own controllers are found out.
    fn log_profiles(&self) {
        let Some(controls) = &self.controls else { return };
        let mut said = Vec::new();
        for (side, &hand) in ["left", "right"].iter().zip(&controls.hands) {
            let profile = match self.session.current_interaction_profile(hand) {
                Ok(path) if path == xr::Path::NULL => "none".to_string(),
                Ok(path) => self.instance.path_to_string(path).unwrap_or_else(|e| e.to_string()),
                Err(e) => e.to_string(),
            };
            said.push(format!("{}={}", side, profile));
        }
        log::line(format!("Controllers: {}", said.join(" ")));
    }

    pub fn running(&self) -> bool {
        self.running
    }

    /// The size of an eye's image, and whether it is sRGB.
    pub fn eye_format(&self) -> ((u32, u32), bool) {
        self.frames.eye_format()
    }

    /// Delete what the session's swapchains were drawn through, with `gl`,
    /// the context it was made with, current.
    pub fn close(mut self, gl: &glow::Context) {
        self.frames.delete(gl);
    }

    /// Wait until the headset wants the next frame, if it is showing the
    /// scene; then it is to be begun, tracked and drawn.
    pub fn wait_frame(&mut self) -> bool {
        if !self.running {
            return false;
        }
        match self.waiter.wait() {
            Ok(state) => {
                let period = Duration::from_nanos(state.predicted_display_period.as_nanos().max(0) as u64);
                self.stats.waited(Instant::now(), period, state.should_render);
                if let Some(line) = self.stats.report(Instant::now()) {
                    log::line(line);
                }
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
        if let Err(e) = self.frames.begin() {
            // (Not begun, it isn't to be drawn or ended.)
            self.pending = None;
            return Err(self.fail("beginning the headset's frame", e));
        }
        self.begun = Some(Instant::now());
        if self.recenter && self.center(state.predicted_display_time) {
            self.recenter = false;
        }
        Ok(())
    }

    /// The head and the controllers at the frame's time.
    pub fn track(&mut self, world: Mat4) -> Tracking {
        let mut tracking = Tracking::default();
        let Some(state) = self.pending else { return tracking };
        let time = state.predicted_display_time;
        let valid = xr::SpaceLocationFlags::POSITION_VALID | xr::SpaceLocationFlags::ORIENTATION_VALID;
        if let Ok(head) = self.head.locate(&self.space, time)
            && head.location_flags.contains(valid)
        {
            tracking.head = Some(rigid(world * pose_matrix(head.pose)));
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
                aim: rigid(world * pose_matrix(location.pose)),
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
    /// `world` is where the space is in the scene. The frame is
    /// ended whatever fails, with nothing in it if the eyes aren't drawn.
    pub fn draw(
        &mut self,
        gl: &glow::Context,
        world: Mat4,
        draw: impl FnMut(usize, View, (u32, u32), bool, glow::Framebuffer),
    ) -> Result<(), String> {
        let Some(state) = self.pending.take() else { return Ok(()) };
        let time = state.predicted_display_time;
        let drawn = if state.should_render { self.draw_eyes(gl, time, world, draw).map(Some) } else { Ok(None) };
        let flushed = self.frames.flush(gl);
        let drawn = drawn.and_then(|views| flushed.map(|()| views));
        let failed = drawn.as_ref().err().map(|&(what, e)| self.fail(what, e));
        let views = drawn.ok().flatten();
        let ended = self.frames.end(time, self.blend, &self.space, views.as_deref());
        if let Some(begun) = self.begun.take() {
            self.stats.worked(begun.elapsed());
        }
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
        gl: &glow::Context,
        time: xr::Time,
        world: Mat4,
        mut draw: impl FnMut(usize, View, (u32, u32), bool, glow::Framebuffer),
    ) -> Result<Vec<xr::View>, (&'static str, xr::sys::Result)> {
        let (_, views) = self.session.locate_views(VIEW, time, &self.space).map_err(|e| ("the headset's views", e))?;
        if views.len() < self.frames.eyes() {
            return Err(("the headset's views", xr::sys::Result::ERROR_VALIDATION_FAILURE));
        }
        if !self.views_logged {
            self.views_logged = true;
            for (index, view) in views.iter().enumerate() {
                let (f, o, p) = (view.fov, view.pose.orientation, view.pose.position);
                log::line(format!(
                    "Eye {}: field of view left {:.1} right {:.1} up {:.1} down {:.1} degrees; \
                     orientation {:.3} {:.3} {:.3} {:.3}, position {:.3} {:.3} {:.3}",
                    index,
                    f.angle_left.to_degrees(),
                    f.angle_right.to_degrees(),
                    f.angle_up.to_degrees(),
                    f.angle_down.to_degrees(),
                    o.x,
                    o.y,
                    o.z,
                    o.w,
                    p.x,
                    p.y,
                    p.z
                ));
            }
        }
        let (_, srgb) = self.frames.eye_format();
        for (index, view) in views.iter().take(self.frames.eyes()).enumerate() {
            let (framebuffer, size) = self.frames.acquire(index)?;
            let fov = view.fov;
            let eye_view = View {
                view: (world * pose_matrix(view.pose)).inverse(),
                projection: fov_projection(fov.angle_left, fov.angle_right, fov.angle_up, fov.angle_down),
            };
            draw(index, eye_view, size, srgb, framebuffer);
            self.frames.release(gl, index)?;
        }
        Ok(views)
    }

    /// Move the space so that its origin is where the head is at `time`,
    /// facing where it faces (level). False while the head isn't tracked.
    fn center(&mut self, time: xr::Time) -> bool {
        let Ok(location) = self.head.locate(&self.space, time) else { return false };
        let valid = xr::SpaceLocationFlags::POSITION_VALID | xr::SpaceLocationFlags::ORIENTATION_VALID;
        if !location.location_flags.contains(valid) {
            return false;
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
            Err(e) => log::line(format!("Recentring: {}", e)),
        }
        true
    }
}

/// A pose in the scene through a `world` that may scale: where it is and
/// which way it turns, unscaled, for the hands' beams and the ears.
fn rigid(pose: Mat4) -> Mat4 {
    let (_, rotation, position) = pose.to_scale_rotation_translation();
    Mat4::from_rotation_translation(rotation, position)
}

/// The controllers' actions, their bindings for the controllers OpenXR
/// knows, and where the hands point from.
fn make_controls(
    instance: &xr::Instance,
    session: &xr::Session<xr::AnyGraphics>,
    hand_interaction: bool,
) -> Result<Controls, String> {
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
    for (profile, inputs) in profiles::profiles(hand_interaction) {
        let mut bindings = Vec::new();
        for &(control, hand, input) in inputs {
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
            log::line(format!("{}: {}", profile, e));
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

/// A session made with `binding`, its eyes' images `sizes` big.
fn open(
    binding: Binding,
    gl: &glow::Context,
    instance: &xr::Instance,
    system: xr::SystemId,
    sizes: &[(u32, u32)],
) -> Result<Opened, String> {
    match binding {
        Binding::Wgl | Binding::Glx => {
            check_gl::<xr::OpenGL>(gl, instance, system)?;
            let (info, library) = native::binding()?;
            // SAFETY: the handles are the context current on this thread,
            // which outlives the session (the window's).
            let (session, waiter, stream) =
                unsafe { instance.create_session::<xr::OpenGL>(system, &info) }.map_err(err("the headset's session"))?;
            let frames = Frames::Gl(GlFrames::new(gl, &session, stream, sizes)?);
            Ok(Opened { session: session.into_any_graphics(), waiter, frames, keep: Keep::Library(library) })
        }
        #[cfg(target_os = "linux")]
        Binding::Egl => {
            check_gl::<egl::Egl>(gl, instance, system)?;
            let (info, library) = egl::current()?;
            // SAFETY: as above.
            let (session, waiter, stream) =
                unsafe { instance.create_session::<egl::Egl>(system, &info) }.map_err(err("the headset's session"))?;
            let frames = Frames::Egl(GlFrames::new(gl, &session, stream, sizes)?);
            Ok(Opened { session: session.into_any_graphics(), waiter, frames, keep: Keep::Library(library) })
        }
        #[cfg(target_os = "linux")]
        Binding::Vulkan => {
            let (session, waiter, bridge, device) = vulkan::open(gl, instance, system, sizes)?;
            Ok(Opened { session, waiter, frames: Frames::Vulkan(Box::new(bridge)), keep: Keep::Vulkan(Box::new(device)) })
        }
        #[cfg(not(target_os = "linux"))]
        _ => Err("not on this system".into()),
    }
}

/// Whether the context has the OpenGL the runtime needs. Asking is
/// required before a session; under the runtime's least OpenGL, it calls
/// what the context doesn't have, and crashes.
fn check_gl<G: xr::Graphics<Requirements = xr::opengl::Requirements>>(
    gl: &glow::Context,
    instance: &xr::Instance,
    system: xr::SystemId,
) -> Result<(), String> {
    let least = instance.graphics_requirements::<G>(system).map_err(err("OpenXR"))?.min_api_version_supported;
    let version = gl.version();
    if (version.major, version.minor) < (least.major() as u32, least.minor() as u32) {
        return Err(format!(
            "the OpenXR runtime needs OpenGL {}.{}, and there is {}.{}",
            least.major(),
            least.minor(),
            version.major,
            version.minor
        ));
    }
    Ok(())
}
