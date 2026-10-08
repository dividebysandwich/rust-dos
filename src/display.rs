//! The window and how the emulated picture fills it: the scale factor,
//! fullscreen, 4:3 aspect correction, the scaling filter and the CRT
//! shader. OpenGL 3 draws it (gl.rs), or where there is none SDL's own
//! renderer, without the shaders. With OpenGL, the 3dfx card's picture
//! can be drawn at a higher resolution (voodoo_gl.rs). Built without the
//! `gl` feature, there is only SDL's renderer (nogl.rs).

#[cfg(feature = "gl")]
mod gl;
#[cfg(not(feature = "gl"))]
#[path = "display/nogl.rs"]
mod gl;
pub mod audio_mix;
#[cfg(feature = "vr")]
mod stage;
#[cfg(feature = "gl")]
mod voodoo_gl;

use crate::config::{Filter, Settings};
use crate::video::mono::Monochrome;
use crate::video::shader::{CrtSettings, Shader};
use crate::video::{self, Frame};
use gl::{GlScreen, NoGl};
use rust_dos::bus::Bus;
use rust_dos::config_ui::Layer;
use rust_dos::voodoo::{Renderer, VoodooSettings};
use sdl2::VideoSubsystem;
use sdl2::pixels::PixelFormatEnum;
use sdl2::render::{ScaleMode, Texture, TextureCreator, WindowCanvas};
use sdl2::surface::Surface;
use sdl2::video::{FullscreenType, Window, WindowContext, WindowPos};
use std::cell::OnceCell;

/// The window's icon: packaging/linux/rust-dos.svg drawn at 128x128 with
/// `rsvg-convert -w 128 -h 128`.
static ICON_PNG: &[u8] = include_bytes!("../assets/rust-dos.png");

/// The size the picture is shown at, in the renderer's logical pixels: the
/// frame itself, or with `aspect` stretched to 4:3, the shape a monitor
/// gave 320x200 and 640x400. It only ever stretches.
pub fn display_size(width: u32, height: u32, aspect: bool) -> (u32, u32) {
    if !aspect || width * 3 == height * 4 {
        (width, height)
    } else if width * 3 > height * 4 {
        (width, (width * 3).div_ceil(4))
    } else {
        ((height * 4).div_ceil(3), height)
    }
}

/// The rows (of `row_bytes` each) from the first to the last that differ
/// between two pictures of the same size, if any do.
fn changed_rows(old: &[u8], new: &[u8], row_bytes: usize) -> Option<std::ops::Range<usize>> {
    let pairs = || old.chunks_exact(row_bytes).zip(new.chunks_exact(row_bytes));
    let first = pairs().position(|(a, b)| a != b)?;
    let last = pairs().rposition(|(a, b)| a != b)?;
    Some(first..last + 1)
}

/// A coordinate in logical pixels (`logical` of them across the picture)
/// as the frame pixel under it (`frame` of them). Positions in the black
/// bars around the picture fall outside `0..frame`.
fn logical_to_frame(pos: i32, logical: u32, frame: u32) -> i32 {
    (pos as i64 * frame as i64).div_euclid(logical.max(1) as i64) as i32
}

pub struct Display<'a> {
    video: VideoSubsystem,
    out: Output<'a>,
    frame: (u32, u32),
    scale: u32,
    fullscreen: bool,
    aspect: bool,
    filter: Filter,
    shader: Shader,
    crt: CrtSettings,
    /// What draws the picture, for the log.
    renderer: String,
    /// Why the shader the settings ask for isn't shown, if it isn't.
    warning: Option<String>,
    /// The picture as last shown, and whether the window must be drawn
    /// again whatever the picture: frames that show nothing new aren't
    /// uploaded or drawn, and those that do upload the rows that changed.
    shown: Vec<u8>,
    redraw: bool,
    /// Why OpenGL doesn't draw the 3dfx card although the settings ask for
    /// it, once said; and whether the last picture shown was the card's
    /// OpenGL drew, which the texture of `shown` isn't.
    voodoo_said: bool,
    voodoo_shown: bool,
    /// The layer over the picture last shown (a manual's page), by its
    /// generation.
    layer_shown: Option<u64>,
    /// The refresh rate of the display the window is on, where SDL knows
    /// it.
    refresh_hz: Option<f64>,
    /// The frame pixel the mouse was last over on the 3D scene's screen,
    /// where it stays while the mouse is off the screen.
    #[cfg(feature = "vr")]
    picked: std::cell::Cell<(i32, i32)>,
}

/// What draws the picture.
enum Output<'a> {
    Gl(Box<GlScreen>),
    /// SDL's renderer, where there is no OpenGL 3: no CRT shaders. The
    /// texture is the picture, `frame` pixels big.
    Sdl {
        canvas: WindowCanvas,
        creator: &'a TextureCreator<WindowContext>,
        texture: Texture<'a>,
        /// The layer over the picture, and its generation, once there is
        /// one.
        layer: Option<(Texture<'a>, u64)>,
    },
}

impl Output<'_> {
    fn window_mut(&mut self) -> &mut Window {
        match self {
            Output::Gl(gl) => gl.window_mut(),
            Output::Sdl { canvas, .. } => canvas.window_mut(),
        }
    }
}

impl<'a> Display<'a> {
    /// The window's size before the first frame: the text mode picture at
    /// the settings' scale and aspect.
    pub fn window_size(settings: &Settings) -> (u32, u32) {
        let (w, h) = display_size(video::SCREEN_WIDTH, video::SCREEN_HEIGHT, settings.aspect);
        let scale = settings.scale.max(1);
        (w * scale, h * scale)
    }

    /// Open the window, drawn with OpenGL 3 or else SDL's renderer, whose
    /// textures come from `textures`.
    pub fn open(
        video: &VideoSubsystem,
        title: &str,
        settings: &Settings,
        textures: &'a OnceCell<TextureCreator<WindowContext>>,
    ) -> Result<Self, String> {
        let frame = (video::SCREEN_WIDTH, video::SCREEN_HEIGHT);
        let size = Self::window_size(settings);
        let mut warning = None;
        let (out, renderer) = match GlScreen::open(video, title, size) {
            Ok(mut gl) => {
                if let Err(problem) = gl.select(settings.shader, settings.filter) {
                    warning = Some(problem);
                }
                gl.set_color_mask(settings.monochrome == Monochrome::Off);
                gl.set_crt(settings.crt);
                let renderer = gl.renderer().to_string();
                (Output::Gl(Box::new(gl)), renderer)
            }
            Err(NoGl { window, reason }) => {
                // What was asked of OpenGL isn't for SDL's renderer.
                // SAFETY: it only resets SDL's settings for new contexts.
                unsafe { sdl2::sys::SDL_GL_ResetAttributes() };
                let window = match window {
                    Some(window) => window,
                    None => {
                        video.window(title, size.0, size.1).position_centered().build().map_err(|e| e.to_string())?
                    }
                };
                let canvas = window.into_canvas().build().map_err(|e| e.to_string())?;
                let creator = textures.get_or_init(|| canvas.texture_creator());
                let texture = create_texture(creator, frame, settings.filter)?;
                if settings.shader != Shader::None {
                    warning = Some(format!(
                        "shader={} needs OpenGL 3, which isn't available ({})",
                        settings.shader.name(),
                        reason
                    ));
                }
                let renderer = format!("SDL renderer {} (no OpenGL 3: {})", canvas.info().name, reason);
                (Output::Sdl { canvas, creator, texture, layer: None }, renderer)
            }
        };
        let mut display = Self {
            video: video.clone(),
            out,
            frame,
            scale: settings.scale,
            fullscreen: false,
            aspect: settings.aspect,
            filter: settings.filter,
            shader: settings.shader,
            crt: settings.crt,
            renderer,
            warning,
            shown: Vec::new(),
            redraw: true,
            voodoo_said: false,
            voodoo_shown: false,
            layer_shown: None,
            refresh_hz: None,
            #[cfg(feature = "vr")]
            picked: std::cell::Cell::new((0, 0)),
        };
        // For the window managers and taskbars that take the icon from the
        // window rather than from rust-dos.desktop. The test below keeps
        // the icon decoding, so there is nothing to report.
        let _ = set_icon(display.out.window_mut());
        display.set_fullscreen(settings.fullscreen)?;
        display.update_refresh_rate();
        Ok(display)
    }

    /// What draws the picture: OpenGL and its version, or SDL's renderer.
    pub fn renderer(&self) -> &str {
        &self.renderer
    }

    /// Why the shader the settings asked for at the start isn't shown.
    pub fn shader_warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    /// Draw the window again at the next frame, whatever the picture: it
    /// was resized or uncovered.
    /// Whether the window must be drawn anew at the next `present`.
    pub fn wants_redraw(&self) -> bool {
        self.redraw
    }

    pub fn redraw(&mut self) {
        self.redraw = true;
        // The window may have moved to another display.
        self.update_refresh_rate();
    }

    fn update_refresh_rate(&mut self) {
        let index = self.out.window_mut().display_index();
        self.refresh_hz = index
            .and_then(|i| self.video.current_display_mode(i))
            .ok()
            .map(|mode| mode.refresh_rate as f64)
            .filter(|&hz| hz > 0.0);
    }

    /// Whether the display shows frames at `hz`, as far as SDL knows: a
    /// display with a variable refresh rate goes up to its own, and no
    /// faster. SDL rounds the rate down to whole hertz.
    pub fn shows_hz(&self, hz: f64) -> bool {
        self.refresh_hz.is_none_or(|max| hz < max + 1.0)
    }

    /// Follow the picture's size, which the video mode sets.
    pub fn set_frame_size(&mut self, width: u32, height: u32) -> Result<(), String> {
        if (width, height) == self.frame {
            return Ok(());
        }
        self.frame = (width, height);
        self.redraw = true;
        if let Output::Sdl { creator, texture, .. } = &mut self.out {
            *texture = create_texture(creator, self.frame, self.filter)?;
        }
        self.layout()
    }

    /// Take on the display settings: scale, fullscreen, aspect, filter,
    /// shader, the CRT look's own and the monochrome tube's missing mask. A
    /// shader that can't be shown is the error, once the rest is done; the
    /// setting stays, to be saved.
    pub fn apply(&mut self, settings: &Settings) -> Result<(), String> {
        let mut problem = None;
        self.redraw = true;
        self.crt = settings.crt;
        if let Output::Gl(gl) = &mut self.out {
            gl.set_color_mask(settings.monochrome == Monochrome::Off);
            gl.set_crt(settings.crt);
        }
        if (settings.shader, settings.filter) != (self.shader, self.filter) {
            let new_shader = settings.shader != self.shader;
            self.shader = settings.shader;
            self.filter = settings.filter;
            match &mut self.out {
                Output::Gl(gl) => {
                    if let Err(e) = gl.select(self.shader, self.filter)
                        && new_shader
                    {
                        problem = Some(e);
                    }
                }
                Output::Sdl { texture, .. } => {
                    texture.set_scale_mode(scale_mode(self.filter));
                    if new_shader && self.shader != Shader::None {
                        problem = Some("CRT shaders need OpenGL 3, which isn't available here".to_string());
                    }
                }
            }
        }
        let resize = (settings.scale, settings.aspect) != (self.scale, self.aspect);
        self.scale = settings.scale;
        self.aspect = settings.aspect;
        if settings.fullscreen != self.fullscreen {
            self.set_fullscreen(settings.fullscreen)?;
        } else if resize {
            self.layout()?;
        }
        problem.map_or(Ok(()), Err)
    }

    fn set_fullscreen(&mut self, on: bool) -> Result<(), String> {
        let mode = if on { FullscreenType::Desktop } else { FullscreenType::Off };
        self.out.window_mut().set_fullscreen(mode)?;
        self.fullscreen = on;
        self.layout()
    }

    /// Outside fullscreen, a window to fit the picture at its display
    /// size. SDL's renderer letterboxes it through its logical size;
    /// OpenGL does at each frame.
    fn layout(&mut self) -> Result<(), String> {
        self.redraw = true;
        let (w, h) = display_size(self.frame.0, self.frame.1, self.aspect);
        if let Output::Sdl { canvas, .. } = &mut self.out {
            canvas.set_logical_size(w, h).map_err(|e| e.to_string())?;
        }
        if !self.fullscreen {
            fit_window(&self.video, self.out.window_mut(), w, h, self.scale);
        }
        Ok(())
    }

    /// With `voodoo_renderer=opengl`, draw what the 3dfx card recorded
    /// with OpenGL; true if its picture is the one to show
    /// (`present`'s `voodoo`). Otherwise the card stops recording.
    pub fn run_voodoo(&mut self, bus: &mut Bus, settings: &VoodooSettings) -> bool {
        let want = settings.renderer == Renderer::OpenGl;
        let problem = match &mut self.out {
            Output::Gl(gl) => {
                let Some(card) = bus.voodoo.as_mut() else {
                    gl.drop_voodoo();
                    return false;
                };
                if !want || gl.voodoo_problem().is_some() {
                    card.set_mirror(false);
                    gl.drop_voodoo();
                    gl.voodoo_problem().filter(|_| want).map(str::to_string)
                } else {
                    // A new card, scale or sampling setting: start again
                    // from its memory.
                    if !card.mirror_attached()
                        || gl.voodoo_scale() != Some((settings.scale, settings.msaa, settings.anisotropy))
                    {
                        card.set_mirror(false);
                        card.set_mirror(true);
                        gl.drop_voodoo();
                        self.redraw = true;
                    }
                    let Some(recording) = card.take_mirror() else { return false };
                    let shown = recording.output && recording.front.is_some();
                    match gl.run_voodoo(recording, settings.scale, settings.msaa, settings.anisotropy) {
                        Ok(changed) => {
                            // The frame `present` gets does not change with
                            // the card's picture, which the software
                            // renderer no longer draws into it.
                            self.redraw |= changed;
                            return shown;
                        }
                        Err(e) => {
                            card.set_mirror(false);
                            Some(e)
                        }
                    }
                }
            }
            Output::Sdl { .. } => {
                (want && bus.voodoo.is_some()).then(|| "there is no OpenGL 3 here".to_string())
            }
        };
        if let Some(problem) = problem
            && !std::mem::replace(&mut self.voodoo_said, true)
        {
            bus.log_string(&format!("[3DFX] voodoo_renderer=opengl: {}; the software renderer draws", problem));
        }
        false
    }

    /// Show `frame`, which must be as big as the last `set_frame_size`,
    /// if it differs from the one shown or the window needs drawing. The
    /// display keeps the picture to compare the next one with, and leaves
    /// the one it showed before in `frame` instead, which saves copying it.
    /// With `voodoo`, the machine's picture without what is drawn over it,
    /// OpenGL shows the 3dfx card's picture it drew instead
    /// (`run_voodoo`), with what `frame` has over it.
    ///
    /// `layer` goes over the picture, at as many of the window's pixels as
    /// it has (`output_scale`).
    pub fn present(&mut self, frame: &mut Frame, voodoo: Option<&Frame>, layer: Option<&Layer>) -> Result<(), String> {
        let row_bytes = frame.width as usize * 3;
        // The layer covers the 3dfx card's picture.
        let voodoo = voodoo.filter(|_| layer.is_none());
        let layer_changed = layer.map(|l| l.generation) != self.layer_shown;
        self.layer_shown = layer.map(|l| l.generation);
        let rows = if self.redraw
            || layer_changed
            || self.shown.len() != frame.rgb.len()
            || self.voodoo_shown && voodoo.is_none()
        {
            0..frame.height as usize
        } else {
            match changed_rows(&self.shown, &frame.rgb, row_bytes) {
                Some(rows) => rows,
                None => {
                    // The 3D scene is drawn every frame all the same.
                    #[cfg(feature = "vr")]
                    if let Output::Gl(gl) = &mut self.out {
                        gl.render_stage();
                    }
                    return Ok(());
                }
            }
        };
        self.redraw = false;
        let display = display_size(frame.width, frame.height, self.aspect);
        let shown = match &mut self.out {
            Output::Gl(gl) => {
                self.voodoo_shown = voodoo.is_some_and(|base| gl.present_voodoo(frame, base, display));
                if !self.voodoo_shown {
                    gl.present(frame, rows, display, layer);
                }
                Ok(())
            }
            Output::Sdl { canvas, creator, texture, layer: layer_texture } => {
                let rect = sdl2::rect::Rect::new(0, rows.start as i32, frame.width, (rows.end - rows.start) as u32);
                let pixels = &frame.rgb[rows.start * row_bytes..rows.end * row_bytes];
                texture.update(Some(rect), pixels, row_bytes).map_err(|e| e.to_string())?;
                canvas.clear();
                // Stretched over the whole logical size: that is the 4:3
                // correction.
                canvas.copy(texture, None, None)?;
                if let Some(layer) = layer {
                    let picture = &layer.picture;
                    let fits = layer_texture.as_ref().is_some_and(|(t, _)| {
                        let q = t.query();
                        (q.width, q.height) == (picture.width, picture.height)
                    });
                    if !fits {
                        let mut new = create_texture(creator, (picture.width, picture.height), Filter::Linear)?;
                        new.set_scale_mode(ScaleMode::Linear);
                        *layer_texture = Some((new, u64::MAX));
                    }
                    let (t, generation) = layer_texture.as_mut().expect("the layer's texture");
                    if *generation != layer.generation {
                        t.update(None, &picture.rgb, picture.width as usize * 3).map_err(|e| e.to_string())?;
                        *generation = layer.generation;
                    }
                    // In logical pixels, which the picture is stretched over.
                    let (sx, sy) = (display.0 as f32 / frame.width as f32, display.1 as f32 / frame.height as f32);
                    let (x, y, w, h) = layer.rect;
                    let to = sdl2::rect::Rect::new((x * sx) as i32, (y * sy) as i32, (w * sx) as u32, (h * sy) as u32);
                    canvas.copy(t, None, Some(to))?;
                }
                canvas.present();
                Ok(())
            }
        };
        if self.shown.len() == frame.rgb.len() {
            std::mem::swap(&mut self.shown, &mut frame.rgb);
        } else {
            self.shown.clone_from(&frame.rgb);
        }
        shown
    }

    /// The size screenshots and recordings of a `width` x `height` picture
    /// through the CRT shader are: the size the window shows it at. None
    /// where the window shows it without a shader.
    pub fn shaded_size(&self, width: u32, height: u32) -> Option<(u32, u32)> {
        match &self.out {
            Output::Gl(gl) => gl.capture_size(display_size(width, height, self.aspect)),
            Output::Sdl { .. } => None,
        }
    }

    /// `frame` through the CRT shader, as the window shows it, for a
    /// screenshot or a recording; None where the window shows it without
    /// one. Drawing it again and reading it back takes time, so only
    /// captures that show the shader ask for it.
    pub fn shaded(&mut self, frame: &Frame) -> Option<Frame> {
        match &mut self.out {
            Output::Gl(gl) => gl.capture(frame, display_size(frame.width, frame.height, self.aspect)),
            Output::Sdl { .. } => None,
        }
    }

    /// The frame pixel under a mouse position: in logical pixels from
    /// SDL's renderer, in window coordinates with OpenGL. Outside the
    /// picture (the black bars of fullscreen, the bezel of a curved
    /// shader) the result is outside the frame.
    /// How many of the window's pixels a frame pixel is shown at, across
    /// and down.
    pub fn output_scale(&self) -> (f64, f64) {
        let display = display_size(self.frame.0, self.frame.1, self.aspect);
        let size = match &self.out {
            Output::Gl(gl) => gl.window().drawable_size(),
            Output::Sdl { canvas, .. } => canvas.output_size().unwrap_or(display),
        };
        let (_, _, w, h) = letterbox(size, display);
        (w as f64 / self.frame.0.max(1) as f64, h as f64 / self.frame.1.max(1) as f64)
    }

    /// How many frame pixels a pixel of mouse motion on the window (in
    /// its events' units) is, across and down.
    pub fn frame_scale(&self) -> (f64, f64) {
        let display = display_size(self.frame.0, self.frame.1, self.aspect);
        let (w, h) = match &self.out {
            Output::Gl(gl) => {
                let (_, _, w, h) = letterbox(gl.window().size(), display);
                (w, h)
            }
            Output::Sdl { .. } => display,
        };
        (self.frame.0 as f64 / w.max(1) as f64, self.frame.1 as f64 / h.max(1) as f64)
    }

    pub fn to_frame(&self, x: i32, y: i32) -> (i32, i32) {
        let display = display_size(self.frame.0, self.frame.1, self.aspect);
        match &self.out {
            #[cfg(feature = "vr")]
            Output::Gl(gl) if let Some(stage) = gl.stage() => {
                let window = gl.window();
                let (ws, ds) = (window.size(), window.drawable_size());
                let px = (x as f32 + 0.5) * ds.0 as f32 / ws.0.max(1) as f32;
                let py = (y as f32 + 0.5) * ds.1 as f32 / ws.1.max(1) as f32;
                if let Some(uv) = stage.pick((px, py)) {
                    let texture = stage.screen_size(display);
                    let flat = CrtSettings { curvature: 0, ..self.crt };
                    self.picked.set(screen_to_frame((uv.x, uv.y), texture, display, self.frame, (gl.active(), flat)));
                }
                self.picked.get()
            }
            Output::Gl(gl) => {
                let window = gl.window();
                let look = (gl.active(), self.crt);
                window_to_frame((x, y), window.size(), window.drawable_size(), display, self.frame, look)
            }
            Output::Sdl { .. } => {
                (logical_to_frame(x, display.0, self.frame.0), logical_to_frame(y, display.1, self.frame.1))
            }
        }
    }
}

/// What has to be done before SDL starts for a VR headset to work: Xlib
/// made safe for the headset's thread, and SDL told what the headset's
/// session needs of the window's OpenGL context.
#[cfg_attr(not(feature = "vr"), allow(unused_variables))]
pub fn before_headset(vr: &rust_dos::vr::VrSettings) {
    #[cfg(feature = "vr")]
    stage::before_sdl(vr);
}

/// Write what the OpenXR runtime and OpenGL offer a VR headset to the
/// console and vr-probe.log (`--vr-probe`).
#[cfg_attr(not(feature = "vr"), allow(unused_variables))]
pub fn vr_probe(video: &sdl2::VideoSubsystem, vr: &rust_dos::vr::VrSettings) {
    #[cfg(feature = "vr")]
    stage::probe(video, vr);
    #[cfg(not(feature = "vr"))]
    println!("This build has no VR");
}

/// What a VR headset's controllers do to the machine at a frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct VrControl {
    /// The frame pixel the laser points at, if it is on the screen.
    pub pointer: Option<(i32, i32)>,
    /// The mouse's left and right buttons.
    pub buttons: [bool; 2],
    /// The controllers as a gamepad, while they are one.
    pub pad: Option<rust_dos::padmap::PadSnapshot>,
    /// The menu button's presses so far: each opens or closes the
    /// settings window.
    pub menu_presses: u32,
}

/// The 3D scene (`[vr]`).
#[cfg(feature = "vr")]
impl Display<'_> {
    /// Show the picture in the 3D scene of `settings`; what is worth
    /// saying about it.
    pub fn open_stage(&mut self, settings: &rust_dos::vr::VrSettings) -> Vec<String> {
        self.redraw = true;
        match &mut self.out {
            Output::Gl(gl) => gl.open_stage(settings).unwrap_or_else(|e| vec![format!("[VR] No 3D scene: {}", e)]),
            Output::Sdl { .. } => vec!["[VR] The 3D scene needs OpenGL 3, which isn't available here".to_string()],
        }
    }

    fn stage_mut(&mut self) -> Option<&mut stage::Stage> {
        match &mut self.out {
            Output::Gl(gl) => gl.stage_mut(),
            Output::Sdl { .. } => None,
        }
    }

    /// Whether the picture is shown in the 3D scene, which is drawn every
    /// frame, changed or not.
    pub fn every_frame(&self) -> bool {
        matches!(&self.out, Output::Gl(gl) if gl.stage().is_some())
    }

    /// Whether a VR headset shows the scene, or could.
    pub fn has_headset(&self) -> bool {
        matches!(&self.out, Output::Gl(gl) if gl.stage().is_some_and(|s| s.has_headset()))
    }

    /// Turn the window's camera by mouse motion.
    pub fn camera_look(&mut self, dx: f32, dy: f32) {
        if let Some(stage) = self.stage_mut() {
            stage.camera_mut().look(dx, dy);
        }
    }

    /// Slide the window's camera sideways and up by mouse motion.
    pub fn camera_pan(&mut self, dx: f32, dy: f32) {
        if let Some(stage) = self.stage_mut() {
            stage.camera_mut().pan(dx, dy);
        }
    }

    /// Move the window's camera ahead (back, below 0) by mouse motion.
    pub fn camera_dolly(&mut self, d: f32) {
        if let Some(stage) = self.stage_mut() {
            stage.camera_mut().dolly(d);
        }
    }

    /// Move the window's camera up (down, below 0) by `metres`.
    pub fn camera_rise(&mut self, metres: f32) {
        if let Some(stage) = self.stage_mut() {
            stage.camera_mut().rise(metres);
        }
    }

    /// The camera back where the scene starts, and the headset centred.
    pub fn recenter(&mut self) {
        if let Some(stage) = self.stage_mut() {
            stage.recenter();
        }
    }

    /// The headset's events; what is worth saying.
    pub fn poll_headset(&mut self) -> Vec<String> {
        match &mut self.out {
            Output::Gl(gl) => gl.poll_stage(),
            Output::Sdl { .. } => Vec::new(),
        }
    }

    /// The PC's lights in the scene, as the machine's are.
    pub fn set_leds(&mut self, leds: rust_dos::vr::Leds) {
        if let Some(stage) = self.stage_mut() {
            stage.set_leds(leds);
        }
    }

    /// Take on the `[vr]` settings: the scene shown or not, in the window
    /// or a headset, as `mode` says. What is worth saying about it.
    pub fn apply_vr(&mut self, settings: &rust_dos::vr::VrSettings) -> Vec<String> {
        self.redraw = true;
        match &mut self.out {
            Output::Gl(gl) => gl.apply_stage(settings),
            Output::Sdl { .. } if settings.mode != rust_dos::vr::VrMode::Off => {
                vec!["[VR] The 3D scene needs OpenGL 3, which isn't available here".to_string()]
            }
            Output::Sdl { .. } => Vec::new(),
        }
    }

    /// What the headset's controllers do, while it shows the scene.
    pub fn vr_input(&self) -> Option<VrControl> {
        let Output::Gl(gl) = &self.out else { return None };
        let stage = gl.stage()?;
        let input = stage.input()?;
        let display = display_size(self.frame.0, self.frame.1, self.aspect);
        let pointer = input.pointer.map(|uv| {
            let texture = stage.screen_size(display);
            let flat = CrtSettings { curvature: 0, ..self.crt };
            screen_to_frame((uv.x, uv.y), texture, display, self.frame, (gl.active(), flat))
        });
        Some(VrControl { pointer, buttons: input.buttons, pad: input.pad, menu_presses: input.menu_presses })
    }

    /// How the sound's channels mix for where the viewer is in the scene,
    /// if it is shown.
    pub fn audio_mix(&self) -> Option<audio_mix::Mix> {
        match &self.out {
            Output::Gl(gl) => gl.stage().map(|stage| stage.audio_mix()),
            Output::Sdl { .. } => None,
        }
    }

    /// Draw the 3D scene again with the picture as it was: the viewer may
    /// have moved.
    pub fn refresh_stage(&mut self) {
        if let Output::Gl(gl) = &mut self.out {
            gl.render_stage();
        }
    }
}

#[cfg(not(feature = "vr"))]
impl Display<'_> {
    pub fn set_leds(&mut self, _leds: rust_dos::vr::Leds) {}

    pub fn apply_vr(&mut self, settings: &rust_dos::vr::VrSettings) -> Vec<String> {
        if settings.mode == rust_dos::vr::VrMode::Off {
            Vec::new()
        } else {
            self.open_stage(settings)
        }
    }

    pub fn vr_input(&self) -> Option<VrControl> {
        None
    }

    pub fn audio_mix(&self) -> Option<audio_mix::Mix> {
        None
    }

    pub fn open_stage(&mut self, _settings: &rust_dos::vr::VrSettings) -> Vec<String> {
        vec!["[VR] This build has no 3D scene (the vr feature)".to_string()]
    }

    pub fn every_frame(&self) -> bool {
        false
    }

    pub fn has_headset(&self) -> bool {
        false
    }

    pub fn camera_look(&mut self, _dx: f32, _dy: f32) {}

    pub fn camera_pan(&mut self, _dx: f32, _dy: f32) {}

    pub fn camera_dolly(&mut self, _d: f32) {}

    pub fn camera_rise(&mut self, _metres: f32) {}

    pub fn recenter(&mut self) {}

    pub fn poll_headset(&mut self) -> Vec<String> {
        Vec::new()
    }


    pub fn refresh_stage(&mut self) {}
}

impl Display<'_> {
    /// Put the host's pointer over the frame's point (`fx`, `fy`), on the
    /// picture as it is drawn (but for the CRT shader's curve).
    pub fn warp_mouse(&self, mouse: &sdl2::mouse::MouseUtil, (fx, fy): (f64, f64)) {
        let display = display_size(self.frame.0, self.frame.1, self.aspect);
        let window = match &self.out {
            Output::Gl(gl) => gl.window(),
            Output::Sdl { canvas, .. } => canvas.window(),
        };
        // SDL's renderer scales its logical size to the window itself.
        let drawable = match &self.out {
            Output::Gl(_) => window.drawable_size(),
            Output::Sdl { .. } => window.size(),
        };
        let (x, y) = frame_to_window((fx, fy), window.size(), drawable, display, self.frame);
        mouse.warp_mouse_in_window(window, x, y);
    }
}

/// The window's pixel showing the frame's point (`fx`, `fy`): the reverse
/// of `window_to_frame`, without a shader's curve.
fn frame_to_window((fx, fy): (f64, f64), window: Size, drawable: Size, display: Size, frame: Size) -> (i32, i32) {
    let (vx, vy, vw, vh) = letterbox(drawable, display);
    let px = vx as f64 + fx / frame.0.max(1) as f64 * vw as f64;
    let py = vy as f64 + fy / frame.1.max(1) as f64 * vh as f64;
    let x = px * window.0 as f64 / drawable.0.max(1) as f64;
    let y = py * window.1 as f64 / drawable.1.max(1) as f64;
    (x as i32, y as i32)
}

/// Where a picture of `inner` proportions goes in `outer` pixels, as big as
/// fits: x, y (from the top), width and height. The same as SDL's renderer
/// makes of a logical size.
fn letterbox(outer: Size, inner: Size) -> (u32, u32, u32, u32) {
    let (ow, oh) = (outer.0 as u64, outer.1 as u64);
    let (iw, ih) = (inner.0.max(1) as u64, inner.1.max(1) as u64);
    if iw * oh == ih * ow {
        (0, 0, outer.0, outer.1)
    } else if iw * oh > ih * ow {
        // Wider: bars above and below.
        let h = (ih * ow / iw) as u32;
        (0, (outer.1 - h) / 2, outer.0, h)
    } else {
        let w = (iw * oh / ih) as u32;
        ((outer.0 - w) / 2, 0, w, outer.1)
    }
}

/// Width and height.
type Size = (u32, u32);

/// A mouse position in window coordinates as the frame pixel under it,
/// through the letterbox and the curvature of the shader with its CRT
/// settings: in a `window` whose drawable has `drawable` pixels (more on a
/// high-DPI screen), the `frame` shown at `display` proportions.
fn window_to_frame(
    (x, y): (i32, i32),
    window: Size,
    drawable: Size,
    display: Size,
    frame: Size,
    (shader, crt): (Shader, CrtSettings),
) -> (i32, i32) {
    let (vx, vy, vw, vh) = letterbox(drawable, display);
    // The middle of the window's pixel, in the drawable's.
    let px = (x as f32 + 0.5) * drawable.0 as f32 / window.0.max(1) as f32;
    let py = (y as f32 + 0.5) * drawable.1 as f32 / window.1.max(1) as f32;
    let u = (px - vx as f32) / vw.max(1) as f32;
    let v = (py - vy as f32) / vh.max(1) as f32;
    let (u, v) = shader.warp(crt, u, v);
    ((u * frame.0 as f32).floor() as i32, (v * frame.1 as f32).floor() as i32)
}

/// A point of the 3D scene's screen (`uv`, 0 to 1 across and down its
/// texture of `texture` pixels) as the frame pixel there: through the
/// letterbox of the `frame` shown at `display` proportions, and the look's
/// overscan.
#[cfg_attr(not(feature = "vr"), allow(dead_code))]
fn screen_to_frame(
    (u, v): (f32, f32),
    texture: Size,
    display: Size,
    frame: Size,
    (shader, crt): (Shader, CrtSettings),
) -> (i32, i32) {
    let (x, y, w, h) = letterbox(texture, display);
    let u = (u * texture.0 as f32 - x as f32) / w.max(1) as f32;
    let v = (v * texture.1 as f32 - y as f32) / h.max(1) as f32;
    let (u, v) = shader.warp(crt, u, v);
    ((u * frame.0 as f32).floor() as i32, (v * frame.1 as f32).floor() as i32)
}

/// The icon's width, height and pixels, four bytes each: red, green, blue
/// and alpha.
fn icon() -> Result<(u32, u32, Vec<u8>), String> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(ICON_PNG)).read_info().map_err(|e| e.to_string())?;
    let mut pixels = vec![0; reader.output_buffer_size().ok_or("the icon is too big")?];
    let info = reader.next_frame(&mut pixels).map_err(|e| e.to_string())?;
    if (info.color_type, info.bit_depth) != (png::ColorType::Rgba, png::BitDepth::Eight) {
        return Err(format!("the icon is {:?} {:?}, not 8-bit RGBA", info.color_type, info.bit_depth));
    }
    pixels.truncate(info.buffer_size());
    Ok((info.width, info.height, pixels))
}

fn set_icon(window: &mut Window) -> Result<(), String> {
    let (width, height, mut pixels) = icon()?;
    let surface = Surface::from_data(&mut pixels, width, height, width * 4, PixelFormatEnum::RGBA32)?;
    // SDL keeps a copy.
    window.set_icon(surface);
    Ok(())
}

fn scale_mode(filter: Filter) -> ScaleMode {
    match filter {
        Filter::Nearest => ScaleMode::Nearest,
        Filter::Linear => ScaleMode::Linear,
    }
}

fn create_texture<'a>(
    creator: &'a TextureCreator<WindowContext>,
    (width, height): (u32, u32),
    filter: Filter,
) -> Result<Texture<'a>, String> {
    let mut texture = creator
        .create_texture_streaming(PixelFormatEnum::RGB24, width, height)
        .map_err(|e| e.to_string())?;
    texture.set_scale_mode(scale_mode(filter));
    Ok(texture)
}

/// Make the window `scale` times the picture's size, or as big as fits the
/// desktop with its frame, and move it where all of it is on the desktop.
fn fit_window(video: &VideoSubsystem, window: &mut Window, width: u32, height: u32, scale: u32) {
    let Ok(bounds) = window.display_index().and_then(|display| video.display_usable_bounds(display)) else {
        let _ = window.set_size(width * scale.max(1), height * scale.max(1));
        return;
    };
    let (top, left, bottom, right) = window.border_size().unwrap_or((0, 0, 0, 0));
    let (top, left, bottom, right) = (top as i32, left as i32, bottom as i32, right as i32);
    let room = (
        (bounds.width() as i32 - left - right).max(0) as u32,
        (bounds.height() as i32 - top - bottom).max(0) as u32,
    );
    let (w, h) = window_fit((width, height), scale, room);
    let _ = window.set_size(w, h);
    let (x, y) = window.position();
    let inside = |at: i32, low: i32, high: i32| at.min(high).max(low);
    let moved = (
        inside(x, bounds.x() + left, bounds.right() - right - w as i32),
        inside(y, bounds.y() + top, bounds.bottom() - bottom - h as i32),
    );
    if moved != (x, y) {
        window.set_position(WindowPos::Positioned(moved.0), WindowPos::Positioned(moved.1));
    }
}

/// The window's size for a picture `size` big at `scale` in `room`: the
/// picture `scale` times over if that fits, else the largest of its shape
/// that does, but never smaller than the picture. Dropping to the next
/// whole scale instead would leave a 1366x768 desktop, or 1920x1080 at
/// 150%, with 1x whatever the scale.
fn window_fit((width, height): Size, scale: u32, room: Size) -> Size {
    let scale = scale.max(1);
    let wanted = (width * scale, height * scale);
    if wanted.0 <= room.0 && wanted.1 <= room.1 {
        return wanted;
    }
    let (w, h) = (width as u64, height as u64);
    let fit = if room.0 as u64 * h <= room.1 as u64 * w {
        (room.0, (room.0 as u64 * h / w) as u32)
    } else {
        ((room.1 as u64 * w / h) as u32, room.1)
    };
    if fit.0 < width || fit.1 < height { (width, height) } else { fit }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_decodes() {
        let (width, height, pixels) = icon().unwrap();
        assert_eq!((width, height), (128, 128));
        assert_eq!(pixels.len(), 128 * 128 * 4);
        // Transparent in the corner, the opaque orange monitor in the middle
        // of its top edge.
        assert_eq!(pixels[3], 0);
        let top = (12 * 128 + 64) * 4;
        assert!(pixels[top + 3] == 255 && pixels[top] > pixels[top + 2]);
    }

    #[test]
    fn aspect_correction_stretches_to_4_3() {
        assert_eq!(display_size(640, 400, false), (640, 400));
        assert_eq!(display_size(640, 400, true), (640, 480));
        assert_eq!(display_size(640, 350, true), (640, 480));
        assert_eq!(display_size(640, 480, true), (640, 480));
        assert_eq!(display_size(1024, 768, true), (1024, 768));
        // Taller than 4:3 widens instead.
        assert_eq!(display_size(640, 512, true), (683, 512));
    }

    #[test]
    fn window_takes_the_scale_or_as_much_as_fits() {
        // 2x fits a 1920x1080 desktop.
        assert_eq!(window_fit((640, 400), 2, (1920, 1040)), (1280, 800));
        // Not 1366x768's (less the taskbar and the title bar): as big as
        // fits, not back to 1x.
        assert_eq!(window_fit((640, 400), 2, (1366, 697)), (1115, 697));
        assert_eq!(window_fit((640, 400), 4, (1366, 697)), (1115, 697));
        // Wider than tall room fits the width.
        assert_eq!(window_fit((640, 480), 3, (1000, 2000)), (1000, 750));
        // Never less than 1x, even where that doesn't fit.
        assert_eq!(window_fit((640, 400), 1, (600, 300)), (640, 400));
        assert_eq!(window_fit((640, 400), 2, (600, 300)), (640, 400));
        assert_eq!(window_fit((640, 400), 0, (1920, 1040)), (640, 400));
    }

    #[test]
    fn mouse_positions_map_to_frame_pixels() {
        // 640x400 shown as 640x480.
        assert_eq!(logical_to_frame(479, 480, 400), 399);
        assert_eq!(logical_to_frame(240, 480, 400), 200);
        assert_eq!(logical_to_frame(0, 480, 400), 0);
        // The black bars of fullscreen.
        assert_eq!(logical_to_frame(-3, 480, 400), -3);
        assert_eq!(logical_to_frame(490, 480, 400), 408);
    }

    #[test]
    fn letterbox_as_sdl_does() {
        // 4:3 in 16:9 has bars left and right.
        assert_eq!(letterbox((1920, 1080), (640, 480)), (240, 0, 1440, 1080));
        // 16:10 in 4:3 has them above and below.
        assert_eq!(letterbox((1024, 768), (640, 400)), (0, 64, 1024, 640));
        assert_eq!(letterbox((1280, 800), (640, 400)), (0, 0, 1280, 800));
    }

    #[test]
    fn window_positions_map_to_frame_pixels() {
        let at = |pos, (window, drawable, display, frame), shader| {
            window_to_frame(pos, window, drawable, display, frame, (shader, CrtSettings::default()))
        };
        // A 640x400 frame at 2x, as SDL's renderer maps it.
        let twice = ((1280, 800), (1280, 800), (640, 400), (640, 400));
        for x in [0, 1, 639, 1278, 1279] {
            assert_eq!(at((x, x / 2), twice, Shader::None), (logical_to_frame(x, 1280, 640), x / 4));
        }
        // The bars of fullscreen are outside the frame.
        let full = ((1920, 1080), (1920, 1080), (640, 480), (640, 400));
        assert_eq!(at((100, 540), full, Shader::None).0, -62);
        assert_eq!(at((1900, 540), full, Shader::None).0, 738);
        assert_eq!(at((960, 1079), full, Shader::None), (320, 399));
        // A high-DPI window has twice the pixels in its drawable.
        let retina = ((640, 400), (1280, 800), (640, 400), (640, 400));
        assert_eq!(at((320, 200), retina, Shader::None), (320, 200));
        // The curved tube keeps the middle and bends the corners away.
        assert_eq!(at((639, 399), twice, Shader::Crt), (319, 199));
        let (x, y) = at((0, 0), twice, Shader::Crt);
        assert!(x < 0 && y < 0);
        // A flat CRT only has the overscan.
        let flat = window_to_frame((0, 0), twice.0, twice.1, twice.2, twice.3, (Shader::Crt, CrtSettings { curvature: 0, ..CrtSettings::default() }));
        assert!(flat.0 > x && flat.0 < 0 && flat.1 > y && flat.1 < 0, "{:?}", flat);
        assert_eq!(at((0, 0), twice, Shader::Aperture), (0, 0));
    }

    #[test]
    fn frame_points_map_back_to_the_window() {
        for (window, drawable, display, frame) in [
            ((1280, 800), (1280, 800), (640, 400), (640, 400)),
            ((1920, 1080), (1920, 1080), (640, 480), (320, 200)),
            ((640, 400), (1280, 800), (640, 400), (640, 400)),
        ] {
            for point in [(0.5, 0.5), (160.5, 100.5), (319.5, 199.5)] {
                let pos = frame_to_window(point, window, drawable, display, frame);
                let back = window_to_frame(pos, window, drawable, display, frame, (Shader::None, CrtSettings::default()));
                assert_eq!(back, (point.0 as i32, point.1 as i32), "{:?} in {:?}", point, (window, frame));
            }
        }
    }
}
