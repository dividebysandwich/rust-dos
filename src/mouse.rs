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
    /// Fractional-mickey accumulators. We emit mickeys at a non-integer
    /// ratio to virtual-pixel delta (2/3 horizontal, 4/3 vertical) so
    /// Carrier Command's internal cursor integrator moves 1:1 with the
    /// host mouse. The remainder from each integer division is kept here
    /// so small sub-pixel motions accumulate instead of being lost.
    pub mickey_accum_x: i32,
    pub mickey_accum_y: i32,

    /// User-installed event callback (AH=0Ch). Invoked by the main loop
    /// when any event bit in `pending_callback_events` also appears in
    /// `callback_mask` and the callback vector is non-null.
    pub callback_mask: u16,
    pub callback_cs: u16,
    pub callback_ip: u16,
    /// Event bits that have fired since we last dispatched to the user
    /// callback. Bit layout matches the AX=000C mask convention:
    ///   bit 0 = motion, bit 1 = L-press, bit 2 = L-release,
    ///   bit 3 = R-press, bit 4 = R-release,
    ///   bit 5 = M-press, bit 6 = M-release.
    pub pending_callback_events: u16,
    /// Snapshot of the mickey delta passed to the last callback. Used so
    /// motion events can pass the accumulated mickey count without
    /// destroying the AH=0Bh counter's independent accumulator.
    pub last_callback_mickey_x: i16,
    pub last_callback_mickey_y: i16,
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
            mickey_accum_x: 0,
            mickey_accum_y: 0,
            callback_mask: 0,
            callback_cs: 0,
            callback_ip: 0,
            pending_callback_events: 0,
            last_callback_mickey_x: 0,
            last_callback_mickey_y: 0,
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
        self.mickey_accum_x = 0;
        self.mickey_accum_y = 0;
        self.callback_mask = 0;
        self.callback_cs = 0;
        self.callback_ip = 0;
        self.pending_callback_events = 0;
        self.last_callback_mickey_x = 0;
        self.last_callback_mickey_y = 0;
    }

    /// Move the cursor, clamped to the clipping window, and accumulate
    /// AH=0Bh mickey deltas. Empirical Carrier Command testing shows its
    /// integrator applies a ~1.5× gain horizontally and ~0.75× vertically
    /// to the mickey stream before advancing its cursor — a 2:1 vertical-
    /// to-horizontal ratio consistent with the standard Microsoft mouse
    /// sensitivity. To cancel that so the in-game cursor tracks the host
    /// mouse 1:1, we emit mickeys at 2/3 the horizontal delta and 4/3
    /// the vertical delta. Remainders accumulate in mickey_accum_* so
    /// slow motions aren't lost to integer truncation.
    pub fn set_position(&mut self, x: i32, y: i32) {
        let new_x = x.clamp(self.min_x, self.max_x);
        let new_y = y.clamp(self.min_y, self.max_y);
        let dx = new_x - self.x;
        let dy = new_y - self.y;

        self.mickey_accum_x += dx * 2;
        self.mickey_accum_y += dy * 4;
        let emit_x = self.mickey_accum_x / 3;
        let emit_y = self.mickey_accum_y / 3;
        self.mickey_accum_x -= emit_x * 3;
        self.mickey_accum_y -= emit_y * 3;

        self.mickey_x = self.mickey_x.wrapping_add(
            emit_x.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
        );
        self.mickey_y = self.mickey_y.wrapping_add(
            emit_y.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
        );

        if dx != 0 || dy != 0 {
            self.pending_callback_events |= 0x01; // motion
        }

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
        // Event-mask bits: L-press=1<<1, R-press=1<<3, M-press=1<<5.
        self.pending_callback_events |= 1u16 << (1 + 2 * button as u16);
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
        // Event-mask bits: L-release=1<<2, R-release=1<<4, M-release=1<<6.
        self.pending_callback_events |= 1u16 << (2 + 2 * button as u16);
    }
}
