//! The cursor's picture on the screen. Windows draws the cursor that
//! SetCursor last set over everything at the mouse's place while
//! ShowCursor's count is 0 or more. USER keeps the cursor's pixels here
//! as it is set, so that the front ends can draw it over each frame
//! (`video::overlay`) from the machine's state alone.

use super::user;
use crate::cpu::Cpu;
use crate::win32::gdi32::surface::{Surface, Target};

/// What a pixel of the cursor does to the screen under it: its top byte
/// says how, the rest is the color as a COLORREF (0x00BBGGRR).
pub const PIXEL_KEEP: u32 = 0;
/// The screen's pixel becomes the color (the AND mask's bit is 0).
pub const PIXEL_SET: u32 = 1 << 24;
/// The screen's pixel is XORed with the color (the AND mask's bit is 1
/// and the image's isn't black): white inverts it.
pub const PIXEL_XOR: u32 = 2 << 24;

/// The picture of the cursor that is set, as the screen shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct CursorImage {
    pub width: i32,
    pub height: i32,
    /// The pixel that is at the mouse's place.
    pub hotspot: (i32, i32),
    /// The rows, the top first: PIXEL_KEEP, or PIXEL_SET or PIXEL_XOR
    /// with a color.
    pub pixels: Vec<u32>,
}

crate::state_fields!(CursorImage {
    width,
    height,
    hotspot,
    pixels
});

impl CursorImage {
    /// What the pixel (`x`, `y`) of the cursor does.
    pub fn pixel(&self, x: i32, y: i32) -> u32 {
        if !(0..self.width).contains(&x) || !(0..self.height).contains(&y) {
            return PIXEL_KEEP;
        }
        self.pixels[(y * self.width + x) as usize]
    }
}

/// SetCursor's work: `handle` becomes the cursor (0 for none), and its
/// picture the one the screen shows. The cursor it replaced.
pub fn set(cpu: &mut Cpu, handle: u32) -> u32 {
    let image = image_of(cpu, handle);
    let input = &mut user(cpu).input;
    input.cursor_image = image;
    std::mem::replace(&mut input.cursor_handle, handle)
}

/// The picture of the icon or cursor `handle`: empty for none. The
/// screen's pixel is ANDed with the mask and XORed with the image, as
/// DrawIcon does, so the picture keeps which of the two each pixel needs.
fn image_of(cpu: &mut Cpu, handle: u32) -> CursorImage {
    let Some(ic) = super::resource::icon(cpu, handle) else {
        return CursorImage::default();
    };
    let Some(mask) = Surface::load_all(cpu, Target::Bitmap(ic.mask)) else {
        return CursorImage::default();
    };
    // Without a color bitmap, the image is the mask's lower half.
    let (color, image_top) = match ic.color {
        0 => (None, ic.height),
        c => (Surface::load_all(cpu, Target::Bitmap(c)), 0),
    };
    let image = color.as_ref().unwrap_or(&mask);
    let (w, h) = (ic.width.max(0), ic.height.max(0));
    let mut pixels = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let and = mask.has(x, y) && mask.color_of(mask.get(x, y)) & 0xFF_FFFF != 0;
            let (ix, iy) = (x, y + image_top);
            let xor = if image.has(ix, iy) {
                image.color_of(image.get(ix, iy)) & 0xFF_FFFF
            } else {
                0
            };
            pixels.push(match (and, xor) {
                (false, c) => PIXEL_SET | c,
                (true, 0) => PIXEL_KEEP,
                (true, c) => PIXEL_XOR | c,
            });
        }
    }
    CursorImage {
        width: w,
        height: h,
        hotspot: ic.hotspot,
        pixels,
    }
}

/// The cursor the screen shows, if it shows one: its picture and where
/// its hot spot is, in the mouse's coordinates. It shows while a Win32
/// program runs with the desktop on the screen, a cursor is set and
/// ShowCursor's count is 0 or more.
pub fn shown(win32: &crate::win32::Win32) -> Option<(&CursorImage, (i32, i32))> {
    let user = &win32.user;
    let input = &user.input;
    let shows = win32.active() && user.desktop && input.cursor_count >= 0;
    (shows && input.cursor_handle != 0 && !input.cursor_image.pixels.is_empty())
        .then_some((&input.cursor_image, input.cursor))
}
