//! Microsoft-compatible DOS mouse driver state.
//!
//! The driver is exposed to DOS code via INT 33h. Coordinates are always
//! expressed in "virtual" units (the driver's coordinate system), which by
//! convention is the pixel grid of the current video mode for graphics modes
//! and 8x the character grid for text modes. The SDL event loop converts host
//! mouse positions into these units before storing them in MouseState.

/// Mouse button bits used by INT 33h.
pub const BUTTON_LEFT: u8 = 0x01;
pub const BUTTON_RIGHT: u8 = 0x02;
pub const BUTTON_MIDDLE: u8 = 0x04;

pub struct MouseState {
    /// True once the driver has been "detected" by software (INT 33h AX=0000h).
    pub installed: bool,
    /// Balanced show/hide counter. Cursor is drawn when this is 0.
    pub hide_counter: i32,
    /// Current cursor position in virtual units.
    pub x: i32,
    pub y: i32,
    /// Bit mask of currently pressed buttons (see BUTTON_* constants).
    pub buttons: u8,

    /// Clipping window the driver keeps the cursor inside.
    pub min_x: i32,
    pub max_x: i32,
    pub min_y: i32,
    pub max_y: i32,

    /// Press / release counters read by AH=05/06. Indexed by button number.
    pub press_count: [u16; 3],
    pub press_x: [i32; 3],
    pub press_y: [i32; 3],
    pub release_count: [u16; 3],
    pub release_x: [i32; 3],
    pub release_y: [i32; 3],

    /// Accumulated deltas in mickeys, read-and-clear via AH=0Bh.
    pub mickey_x: i16,
    pub mickey_y: i16,

    /// User-installed event callback (AH=0Ch). Not invoked yet; stored so that
    /// programs that check back for the address don't see a reset to zero.
    pub callback_mask: u16,
    pub callback_cs: u16,
    pub callback_ip: u16,
}

impl MouseState {
    pub fn new() -> Self {
        MouseState {
            installed: false,
            hide_counter: 1, // hidden until software asks to show
            x: 0,
            y: 0,
            buttons: 0,
            min_x: 0,
            max_x: 639,
            min_y: 0,
            max_y: 199,
            press_count: [0; 3],
            press_x: [0; 3],
            press_y: [0; 3],
            release_count: [0; 3],
            release_x: [0; 3],
            release_y: [0; 3],
            mickey_x: 0,
            mickey_y: 0,
            callback_mask: 0,
            callback_cs: 0,
            callback_ip: 0,
        }
    }

    /// Reset state to "just installed" defaults and return number of buttons.
    pub fn reset(&mut self, screen_w: i32, screen_h: i32) {
        self.installed = true;
        self.hide_counter = 1;
        self.x = screen_w / 2;
        self.y = screen_h / 2;
        self.buttons = 0;
        self.min_x = 0;
        self.max_x = screen_w - 1;
        self.min_y = 0;
        self.max_y = screen_h - 1;
        self.press_count = [0; 3];
        self.press_x = [0; 3];
        self.press_y = [0; 3];
        self.release_count = [0; 3];
        self.release_x = [0; 3];
        self.release_y = [0; 3];
        self.mickey_x = 0;
        self.mickey_y = 0;
        self.callback_mask = 0;
        self.callback_cs = 0;
        self.callback_ip = 0;
    }

    /// Move the cursor, clamped to the clipping window. Accumulates mickey
    /// deltas (1 mickey == 1/8 screen pixel on real hardware; we use 1:1
    /// which is good enough for most games that care about mickeys).
    pub fn set_position(&mut self, x: i32, y: i32) {
        let new_x = x.clamp(self.min_x, self.max_x);
        let new_y = y.clamp(self.min_y, self.max_y);
        self.mickey_x = self.mickey_x.wrapping_add((new_x - self.x) as i16);
        self.mickey_y = self.mickey_y.wrapping_add((new_y - self.y) as i16);
        self.x = new_x;
        self.y = new_y;
    }

    /// Record a button-down event at the current cursor position.
    pub fn button_down(&mut self, button: usize) {
        if button >= 3 {
            return;
        }
        let mask = 1u8 << button;
        self.buttons |= mask;
        self.press_count[button] = self.press_count[button].wrapping_add(1);
        self.press_x[button] = self.x;
        self.press_y[button] = self.y;
    }

    /// Record a button-up event at the current cursor position.
    pub fn button_up(&mut self, button: usize) {
        if button >= 3 {
            return;
        }
        let mask = 1u8 << button;
        self.buttons &= !mask;
        self.release_count[button] = self.release_count[button].wrapping_add(1);
        self.release_x[button] = self.x;
        self.release_y[button] = self.y;
    }
}
