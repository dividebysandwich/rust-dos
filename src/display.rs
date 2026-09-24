//! The window and how the emulated picture fills it: the scale factor,
//! fullscreen, 4:3 aspect correction and the scaling filter.

use crate::config::{Filter, Settings};
use crate::video::{self, Frame};
use sdl2::VideoSubsystem;
use sdl2::pixels::PixelFormatEnum;
use sdl2::render::{ScaleMode, Texture, TextureCreator, WindowCanvas};
use sdl2::video::{FullscreenType, Window, WindowContext};

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
    canvas: WindowCanvas,
    creator: &'a TextureCreator<WindowContext>,
    video: VideoSubsystem,
    /// The picture, `frame` pixels big.
    texture: Texture<'a>,
    frame: (u32, u32),
    scale: u32,
    fullscreen: bool,
    aspect: bool,
    filter: Filter,
}

impl<'a> Display<'a> {
    /// The window's size before the first frame: the text mode picture at
    /// the settings' scale and aspect.
    pub fn window_size(settings: &Settings) -> (u32, u32) {
        let (w, h) = display_size(video::SCREEN_WIDTH, video::SCREEN_HEIGHT, settings.aspect);
        let scale = settings.scale.max(1);
        (w * scale, h * scale)
    }

    pub fn new(
        canvas: WindowCanvas,
        creator: &'a TextureCreator<WindowContext>,
        video: VideoSubsystem,
        settings: &Settings,
    ) -> Result<Self, String> {
        let frame = (video::SCREEN_WIDTH, video::SCREEN_HEIGHT);
        let mut display = Self {
            canvas,
            creator,
            video,
            texture: create_texture(creator, frame, settings.filter)?,
            frame,
            scale: settings.scale,
            fullscreen: false,
            aspect: settings.aspect,
            filter: settings.filter,
        };
        display.set_fullscreen(settings.fullscreen)?;
        Ok(display)
    }

    /// Follow the picture's size, which the video mode sets.
    pub fn set_frame_size(&mut self, width: u32, height: u32) -> Result<(), String> {
        if (width, height) == self.frame {
            return Ok(());
        }
        self.frame = (width, height);
        self.texture = create_texture(self.creator, self.frame, self.filter)?;
        self.layout()
    }

    /// Take on the display settings: scale, fullscreen, aspect and filter.
    pub fn apply(&mut self, settings: &Settings) -> Result<(), String> {
        if settings.filter != self.filter {
            self.filter = settings.filter;
            self.texture.set_scale_mode(scale_mode(self.filter));
        }
        let resize = (settings.scale, settings.aspect) != (self.scale, self.aspect);
        self.scale = settings.scale;
        self.aspect = settings.aspect;
        if settings.fullscreen != self.fullscreen {
            self.set_fullscreen(settings.fullscreen)
        } else if resize {
            self.layout()
        } else {
            Ok(())
        }
    }

    fn set_fullscreen(&mut self, on: bool) -> Result<(), String> {
        let mode = if on { FullscreenType::Desktop } else { FullscreenType::Off };
        self.canvas.window_mut().set_fullscreen(mode)?;
        self.fullscreen = on;
        self.layout()
    }

    /// The renderer's logical size, which letterboxes the picture at its
    /// display size, and outside fullscreen a window to fit it.
    fn layout(&mut self) -> Result<(), String> {
        let (w, h) = display_size(self.frame.0, self.frame.1, self.aspect);
        self.canvas.set_logical_size(w, h).map_err(|e| e.to_string())?;
        if !self.fullscreen {
            fit_window(&self.video, self.canvas.window_mut(), w, h, self.scale);
        }
        Ok(())
    }

    /// Show `frame`, which must be as big as the last `set_frame_size`.
    pub fn present(&mut self, frame: &Frame) -> Result<(), String> {
        self.texture
            .update(None, &frame.rgb, frame.width as usize * 3)
            .map_err(|e| e.to_string())?;
        self.canvas.clear();
        // Stretched over the whole logical size: that is the 4:3 correction.
        self.canvas.copy(&self.texture, None, None)?;
        self.canvas.present();
        Ok(())
    }

    /// The frame pixel under a mouse position, which SDL gives in logical
    /// pixels. Outside the picture (the black bars of fullscreen) the
    /// result is outside the frame.
    pub fn to_frame(&self, x: i32, y: i32) -> (i32, i32) {
        let (lw, lh) = display_size(self.frame.0, self.frame.1, self.aspect);
        (logical_to_frame(x, lw, self.frame.0), logical_to_frame(y, lh, self.frame.1))
    }
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
}
