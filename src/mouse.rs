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

    /// Size of the virtual screen the host pointer spans: 640 wide in the
    /// 320-pixel modes like the Microsoft driver, the mode's height, or the
    /// cursor range when the program set a larger one (AX=0007h/0008h), as
    /// games that halve the coordinates do. Ranges of 2048 and up are left
    /// alone; they stand for relative movement rather than a screen.
    pub fn virtual_extent(&self, mode: crate::video::VideoMode) -> (i32, i32) {
        let (w, h) = mode.dimensions();
        let w = if w < 640 { 640 } else { w as i32 };
        let h = h as i32;
        let range = |max: i32, size: i32| if max < 2048 { size.max(max + 1) } else { size };
        (range(self.max_x, w), range(self.max_y, h))
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

/// ROM trampoline (F000:F200) that runs the INT 33h AX=000Ch event handler
/// the way a mouse driver's IRQ handler does: save the registers, load the
/// event registers, CALL FAR the handler (which returns with RETF), restore
/// the registers and IRET to the interrupted code.
pub const CALLBACK_STUB: usize = 0xFF200;
/// The stub's data: AX, BX, CX, DX, SI, DI for the handler, the handler's
/// far address, then a busy byte the stub clears when the handler returns.
const CALLBACK_DATA: usize = 0xFF240;
const CALLBACK_BUSY: usize = CALLBACK_DATA + 0x10;

/// Write the event handler trampoline into the ROM area.
pub fn install_callback_stub(bus: &mut crate::bus::Bus) {
    let d = (CALLBACK_DATA - 0xF0000) as u16;
    let [b0, b1] = d.to_le_bytes();
    let at = |off: u16| (d + off).to_le_bytes();
    let mut code = vec![0x1E, 0x06, 0x55, 0x57, 0x56, 0x52, 0x51, 0x53, 0x50]; // push ds..ax
    code.extend([0x2E, 0xA1, b0, b1]); // mov ax,cs:[d]
    for (modrm, off) in [(0x1E, 2), (0x0E, 4), (0x16, 6), (0x36, 8), (0x3E, 10)] {
        code.extend([0x2E, 0x8B, modrm]); // mov bx/cx/dx/si/di,cs:[d+off]
        code.extend(at(off));
    }
    code.extend([0x2E, 0xFF, 0x1E]); // call far cs:[d+12]
    code.extend(at(12));
    code.extend([0x58, 0x5B, 0x59, 0x5A, 0x5E, 0x5F, 0x5D, 0x07, 0x1F]); // pop ax..ds
    code.extend([0x2E, 0xC6, 0x06]); // mov byte cs:[busy],0
    code.extend(at(0x10));
    code.extend([0x00, 0xCF]); // iret
    for (i, b) in code.into_iter().enumerate() {
        bus.write_8(CALLBACK_STUB + i, b);
    }
    clear_callback_busy(bus);
}

/// True while the event handler runs; events stay pending until it returns.
pub fn callback_busy(bus: &crate::bus::Bus) -> bool {
    bus.read_8(CALLBACK_BUSY) != 0
}

/// Forget a handler call that never returned, e.g. after a driver reset.
pub fn clear_callback_busy(bus: &mut crate::bus::Bus) {
    bus.write_8(CALLBACK_BUSY, 0);
}

/// Mouse event callback: INT 33h AX=000C registers a far pointer that the
/// "driver" invokes on the events in its mask. Many Microsoft-mouse
/// compatible games expect button presses to arrive this way rather than
/// via polling AH=03 or AH=05. When an event in the mask is pending, a
/// handler is installed and not still running, enter the ROM stub like the
/// driver's IRQ handler: it saves the registers and CALL FARs the handler,
/// which returns with RETF. The caller checks IF first. Returns true when
/// the handler was entered.
pub fn deliver_callback(cpu: &mut crate::cpu::Cpu) -> bool {
    let busy = callback_busy(&cpu.bus);
    let mouse = &mut cpu.bus.mouse;
    let fire = mouse.pending_callback_events & mouse.callback_mask;
    if fire == 0 || (mouse.callback_cs == 0 && mouse.callback_ip == 0) || busy {
        return false;
    }
    // Snapshot and consume the bits we're about to handle.
    mouse.pending_callback_events &= !fire;
    let dx = mouse.mickey_x - mouse.last_callback_mickey_x;
    let dy = mouse.mickey_y - mouse.last_callback_mickey_y;
    mouse.last_callback_mickey_x = mouse.mickey_x;
    mouse.last_callback_mickey_y = mouse.mickey_y;
    let regs = [
        fire,
        mouse.buttons as u16,
        mouse.x as u16,
        mouse.y as u16,
        dx as u16,
        dy as u16,
    ];
    let (handler_cs, handler_ip) = (mouse.callback_cs, mouse.callback_ip);

    for (i, r) in regs.into_iter().enumerate() {
        cpu.bus.write_16(CALLBACK_DATA + i * 2, r);
    }
    cpu.bus.write_16(CALLBACK_DATA + 12, handler_ip);
    cpu.bus.write_16(CALLBACK_DATA + 14, handler_cs);
    cpu.bus.write_8(CALLBACK_BUSY, 1);

    cpu.push(cpu.flags16());
    cpu.push(cpu.cs());
    cpu.push(cpu.ip());
    cpu.set_cs(0xF000);
    cpu.set_ip((CALLBACK_STUB - 0xF0000) as u16);
    cpu.set_cpu_flag(crate::cpu::CpuFlags::IF, false);
    cpu.set_cpu_flag(crate::cpu::CpuFlags::TF, false);
    true
}
