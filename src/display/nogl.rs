//! What stands in for gl.rs in a build without OpenGL (the `gl` feature
//! off, as on Redox): a screen there never is, so the display always takes
//! SDL's renderer.

use crate::config::Filter;
use crate::video::Frame;
use crate::video::shader::{CrtSettings, Shader};
use rust_dos::config_ui::Layer;
use sdl2::VideoSubsystem;
use sdl2::video::Window;

/// Why there is no OpenGL 3 to draw with, and the window if one was made
/// for it, which SDL's renderer can take instead.
pub struct NoGl {
    pub window: Option<Window>,
    pub reason: String,
}

/// No value of it exists.
pub enum GlScreen {}

impl GlScreen {
    pub fn open(_video: &VideoSubsystem, _title: &str, _size: (u32, u32)) -> Result<Self, NoGl> {
        Err(NoGl { window: None, reason: "built without OpenGL".to_string() })
    }

    pub fn renderer(&self) -> &str {
        match *self {}
    }

    pub fn window(&self) -> &Window {
        match *self {}
    }

    pub fn window_mut(&mut self) -> &mut Window {
        match *self {}
    }

    pub fn active(&self) -> Shader {
        match *self {}
    }

    pub fn select(&mut self, _shader: Shader, _filter: Filter) -> Result<(), String> {
        match *self {}
    }

    pub fn set_color_mask(&mut self, _on: bool) {
        match *self {}
    }

    pub fn set_crt(&mut self, _crt: CrtSettings) {
        match *self {}
    }

    pub fn present(&mut self, _frame: &Frame, _rows: std::ops::Range<usize>, _display: (u32, u32), _layer: Option<&Layer>) {
        match *self {}
    }

    pub fn voodoo_problem(&self) -> Option<&str> {
        match *self {}
    }

    pub fn voodoo_scale(&self) -> Option<(u32, u32, u32)> {
        match *self {}
    }

    pub fn drop_voodoo(&mut self) {
        match *self {}
    }

    pub fn run_voodoo(
        &mut self,
        _recording: rust_dos::voodoo::mirror::Frame,
        _scale: u32,
        _samples: u32,
        _anisotropy: u32,
    ) -> Result<bool, String> {
        match *self {}
    }

    pub fn present_voodoo(&mut self, _screen: &Frame, _base: &Frame, _display: (u32, u32)) -> bool {
        match *self {}
    }

    pub fn capture_size(&self, _display: (u32, u32)) -> Option<(u32, u32)> {
        match *self {}
    }

    pub fn capture(&mut self, _frame: &Frame, _display: (u32, u32)) -> Option<Frame> {
        match *self {}
    }
}
