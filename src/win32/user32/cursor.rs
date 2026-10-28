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
    let image = image_of(cpu, handle).unwrap_or_default();
    let input = &mut user(cpu).input;
    input.cursor_image = image;
    std::mem::replace(&mut input.cursor_handle, handle)
}

/// The picture of the icon or cursor `handle`: None for none, or when
/// one of its bitmaps can't be read. The screen's pixel is ANDed with
/// the mask and XORed with the image, as DrawIcon does, so the picture
/// keeps which of the two each pixel needs.
fn image_of(cpu: &mut Cpu, handle: u32) -> Option<CursorImage> {
    let ic = super::resource::icon(cpu, handle)?;
    let mask = Surface::load_all(cpu, Target::Bitmap(ic.mask))?;
    // Without a color bitmap, the image is the mask's lower half.
    let (color, image_top) = match ic.color {
        0 => (None, ic.height),
        c => (Some(Surface::load_all(cpu, Target::Bitmap(c))?), 0),
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
    Some(CursorImage {
        width: w,
        height: h,
        hotspot: ic.hotspot,
        pixels,
    })
}

/// The cursor the screen shows, if it shows one: its picture and where
/// its hot spot is, in the screen's pixels (`screen_size`). It shows
/// while a Win32 program runs with the desktop on the screen, a cursor
/// is set, ShowCursor's count is 0 or more and DirectInput doesn't have
/// the mouse exclusively, which hides it on Windows.
pub fn shown(win32: &crate::win32::Win32) -> Option<(&CursorImage, (i32, i32))> {
    let user = &win32.user;
    let input = &user.input;
    let shows = win32.active() && user.desktop && input.cursor_count >= 0;
    (shows
        && input.cursor_handle != 0
        && !input.cursor_image.pixels.is_empty()
        && !win32.dinput.mouse_taken())
    .then_some((&input.cursor_image, input.cursor))
}

/// The screen the host's pointer spans while the built-in Windows has
/// the mouse, a Win32 program running with the desktop or a DirectDraw
/// mode on the screen: Windows' screen (`screen_size`), which the
/// mouse's range covers and in whose pixels programs get the cursor.
/// None otherwise, for the DOS driver's virtual screen.
pub fn pointer_screen(win32: &crate::win32::Win32) -> Option<(i32, i32)> {
    let shows = win32.user.desktop || win32.ddraw.display.mode.0 != 0;
    (win32.active() && shows).then(|| screen_size(win32))
}

/// The size of the screen the cursor moves over, in its pixels:
/// DirectDraw's mode while it has the display, else the desktop's, as
/// ChangeDisplaySettings may have set it (`screen::desktop`).
pub fn screen_size(win32: &crate::win32::Win32) -> (i32, i32) {
    let (w, h) = match win32.ddraw.display.mode {
        (0, ..) => win32
            .desktop_size
            .map_or(crate::win32::screen::DESKTOP, |(w, h, _)| (w, h)),
        (w, h, _) => (w, h),
    };
    (w as i32, h as i32)
}
