//! The window and how the emulated picture fills it: the scale factor,
//! fullscreen, 4:3 aspect correction, the scaling filter and the CRT
//! shader. OpenGL 3 draws it (gl.rs), or where there is none SDL's own
//! renderer, without the shaders.

mod gl;

use crate::config::{Filter, Settings};
use crate::video::mono::Monochrome;
use crate::video::shader::{CrtSettings, Shader};
use crate::video::{self, Frame};
use gl::{GlScreen, NoGl};
use sdl2::VideoSubsystem;
use sdl2::pixels::PixelFormatEnum;
use sdl2::render::{ScaleMode, Texture, TextureCreator, WindowCanvas};
use sdl2::surface::Surface;
use sdl2::video::{FullscreenType, Window, WindowContext};
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
                (Output::Sdl { canvas, creator, texture }, renderer)
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
        };
        // For the window managers and taskbars that take the icon from the
        // window rather than from rust-dos.desktop. The test below keeps
        // the icon decoding, so there is nothing to report.
        let _ = set_icon(display.out.window_mut());
        display.set_fullscreen(settings.fullscreen)?;
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

    /// Follow the picture's size, which the video mode sets.
    pub fn set_frame_size(&mut self, width: u32, height: u32) -> Result<(), String> {
        if (width, height) == self.frame {
            return Ok(());
        }
        self.frame = (width, height);
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
        let (w, h) = display_size(self.frame.0, self.frame.1, self.aspect);
        if let Output::Sdl { canvas, .. } = &mut self.out {
            canvas.set_logical_size(w, h).map_err(|e| e.to_string())?;
        }
        if !self.fullscreen {
            fit_window(&self.video, self.out.window_mut(), w, h, self.scale);
        }
        Ok(())
    }

    /// Show `frame`, which must be as big as the last `set_frame_size`.
    pub fn present(&mut self, frame: &Frame) -> Result<(), String> {
        match &mut self.out {
            Output::Gl(gl) => {
                gl.present(frame, display_size(frame.width, frame.height, self.aspect));
                Ok(())
            }
            Output::Sdl { canvas, texture, .. } => {
                texture.update(None, &frame.rgb, frame.width as usize * 3).map_err(|e| e.to_string())?;
                canvas.clear();
                // Stretched over the whole logical size: that is the 4:3
                // correction.
                canvas.copy(texture, None, None)?;
                canvas.present();
                Ok(())
            }
        }
    }

    /// The frame pixel under a mouse position: in logical pixels from
    /// SDL's renderer, in window coordinates with OpenGL. Outside the
    /// picture (the black bars of fullscreen, the bezel of a curved
    /// shader) the result is outside the frame.
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

/// Make the window `scale` times the picture's size, or the largest whole
/// multiple of it that fits the desktop.
fn fit_window(video: &VideoSubsystem, window: &mut Window, width: u32, height: u32, scale: u32) {
    let bounds = window.display_index().and_then(|display| video.display_usable_bounds(display));
    let mut scale = scale.max(1);
    if let Ok(bounds) = bounds {
        while scale > 1 && (width * scale > bounds.width() || height * scale > bounds.height()) {
            scale -= 1;
        }
    }
    let _ = window.set_size(width * scale, height * scale);
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
        let flat = window_to_frame((0, 0), twice.0, twice.1, twice.2, twice.3, (Shader::Crt, CrtSettings { curvature: 0 }));
        assert!(flat.0 > x && flat.0 < 0 && flat.1 > y && flat.1 < 0, "{:?}", flat);
        assert_eq!(at((0, 0), twice, Shader::Aperture), (0, 0));
    }
}
