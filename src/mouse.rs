//! Microsoft-compatible DOS mouse driver state.
//!
//! The driver is exposed to DOS code via INT 33h. Coordinates are always
//! expressed in "virtual" units (the driver's coordinate system), which by
//! convention is the pixel grid of the current video mode for graphics modes
//! and 8x the character grid for text modes. The SDL event loop converts host
//! mouse positions into these units before storing them in MouseState.

/// The PS/2 pointing device the BIOS offers through INT 15h AH=C2h, which
/// the same motion and buttons move: packets of the motion since the last
/// one and the buttons, which the BIOS's IRQ 12 handler (INT 74h) hands to
/// the handler a program installed (AX=C207h), as Windows' mouse driver
/// has it do.
pub struct Ps2Mouse {
    /// Reporting (AX=C200h BH=1).
    pub enabled: bool,
    /// The handler's far address (AX=C207h), segment and offset; 0:0 is
    /// none.
    pub handler: (u16, u16),
    /// Reports per second (AX=C202h), the resolution (AX=C203h, 0-3) and
    /// 2:1 scaling (AX=C206h).
    pub rate: u16,
    pub resolution: u8,
    pub scaling: bool,
    /// The motion not reported yet, in counts (pixels, down positive), and
    /// the buttons last reported.
    pub dx: i32,
    pub dy: i32,
    pub reported_buttons: u8,
    /// Where the pointer was put last (`MouseState::set_position`), whose
    /// moves count whole, where the INT 33h cursor stops at its window.
    last_position: Option<(i32, i32)>,
    /// When the next report may go, in emulated microseconds.
    pub next_report: u64,
    /// A command to the mouse (through the controller's D4h) waiting for
    /// its parameter: F3h (rate) or E8h (resolution).
    pending_command: Option<u8>,
    /// The bytes of the report the BIOS's IRQ 12 handler has read so far.
    packet: Vec<u8>,
}

impl Ps2Mouse {
    /// The state after a reset (AX=C201h): off, 100 reports a second, 4
    /// counts per millimetre, 1:1, with its handler kept.
    fn reset(&mut self) {
        self.enabled = false;
        self.rate = 100;
        self.resolution = 2;
        self.scaling = false;
    }
}

impl Default for Ps2Mouse {
    fn default() -> Self {
        let mut ps2 = Self {
            enabled: false,
            handler: (0, 0),
            rate: 0,
            resolution: 0,
            scaling: false,
            dx: 0,
            dy: 0,
            reported_buttons: 0,
            last_position: None,
            next_report: 0,
            pending_command: None,
            packet: Vec::new(),
        };
        ps2.reset();
        ps2
    }
}

crate::state_fields!(Ps2Mouse {
    enabled, handler, rate, resolution, scaling, dx, dy, reported_buttons, last_position, next_report,
    pending_command, packet,
});

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
    /// The parts of a pixel a captured mouse moved that the cursor hasn't
    /// yet (see `move_by`).
    rest_x: f64,
    rest_y: f64,
    /// The PS/2 pointing device of the BIOS.
    pub ps2: Ps2Mouse,
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
            rest_x: 0.0,
            rest_y: 0.0,
            ps2: Ps2Mouse::default(),
        }
    }

    /// Size of the virtual screen the host pointer spans: 640 wide in the
    /// 320-pixel modes like the Microsoft driver, the mode's height, or the
    /// cursor range when the program set a larger one (AX=0007h/0008h), as
    /// games that halve the coordinates do. Ranges of 2048 and up are left
    /// alone; they stand for relative movement rather than a screen.
    pub fn virtual_extent(&self, (w, h): (usize, usize)) -> (i32, i32) {
        let w = if w < 640 { 640 } else { w as i32 };
        let h = h as i32;
        let range = |max: i32, size: i32| if max < 2048 { size.max(max + 1) } else { size };
        (range(self.max_x, w), range(self.max_y, h))
    }

    /// The virtual screen of `bus`'s current video mode (see
    /// `virtual_extent` and `screen_size`).
    pub fn virtual_screen(&self, bus: &crate::bus::Bus) -> (i32, i32) {
        self.virtual_extent(screen_size(bus))
    }

    /// Forget the program's event handler (AX=000Ch), whose code is gone
    /// once the shell is back.
    pub fn remove_callback(&mut self) {
        self.callback_mask = 0;
        self.callback_cs = 0;
        self.callback_ip = 0;
        self.pending_callback_events = 0;
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
        if let Some((last_x, last_y)) = self.ps2.last_position.replace((x, y)) {
            self.ps2.dx += x - last_x;
            self.ps2.dy += y - last_y;
        }
        let new_x = x.clamp(self.min_x, self.max_x);
        let new_y = y.clamp(self.min_y, self.max_y);
        let dx = new_x - self.x;
        let dy = new_y - self.y;
        self.add_mickeys(dx, dy);

        if dx != 0 || dy != 0 {
            self.pending_callback_events |= 0x01; // motion
        }

        self.x = new_x;
        self.y = new_y;
    }

    /// Move the cursor by (`dx`, `dy`) virtual pixels, as a captured mouse
    /// does: the mickeys count all of the motion, at the edges of the
    /// clipping window too, where the cursor stops, so a game that turns
    /// with the mouse keeps turning.
    pub fn move_by(&mut self, dx: f64, dy: f64) {
        self.rest_x += dx;
        self.rest_y += dy;
        let (dx, dy) = (self.rest_x.trunc() as i32, self.rest_y.trunc() as i32);
        self.rest_x -= dx as f64;
        self.rest_y -= dy as f64;
        if dx == 0 && dy == 0 {
            return;
        }
        self.ps2.dx += dx;
        self.ps2.dy += dy;
        self.ps2.last_position = None;
        self.add_mickeys(dx, dy);
        self.pending_callback_events |= 0x01; // motion
        self.x = (self.x + dx).clamp(self.min_x, self.max_x);
        self.y = (self.y + dy).clamp(self.min_y, self.max_y);
    }

    /// Count the mickeys of a motion of (`dx`, `dy`) virtual pixels (see
    /// `set_position`).
    fn add_mickeys(&mut self, dx: i32, dy: i32) {
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

/// The screen the driver's coordinates cover in the current video mode:
/// the mode's pixels in a graphics mode, and 8 a character in a text mode
/// whatever its font (640x200 for 80x25, 640x400 for 80x50), which
/// programs divide by 8 for the column and row.
pub fn screen_size(bus: &crate::bus::Bus) -> (usize, usize) {
    match crate::video::text::geometry(bus) {
        Some(text) => (text.cols * 8, text.rows * 8),
        None => bus.display_size(),
    }
}

/// ROM trampoline (F000:F200) that runs the INT 33h AX=000Ch event handler
/// the way a mouse driver's IRQ handler does: save the registers, load the
/// event registers, CALL FAR the handler (which returns with RETF), restore
/// the registers and IRET to the interrupted code.
pub const CALLBACK_STUB: usize = 0xFF200;
/// The stub's data: AX, BX, CX, DX, SI, DI for the handler, the handler's
/// far address, then a busy byte the stub has cleared when the handler
/// returns (`bios::SERVICE_MOUSE_CALLBACK_DONE`).
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
    code.extend([0xFE, 0x39, crate::bios::SERVICE_MOUSE_CALLBACK_DONE]); // not busy
    code.extend([0xCF]); // iret
    bus.write_rom(CALLBACK_STUB, &code);
    clear_callback_busy(bus);
}

/// True while the event handler runs; events stay pending until it returns.
pub fn callback_busy(bus: &crate::bus::Bus) -> bool {
    bus.read_8(CALLBACK_BUSY) != 0
}

/// Forget a handler call that never returned, e.g. after a driver reset.
pub fn clear_callback_busy(bus: &mut crate::bus::Bus) {
    bus.write_rom(CALLBACK_BUSY, &[0]);
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
    let mouse = &cpu.bus.mouse;
    let fire = mouse.pending_callback_events & mouse.callback_mask;
    if fire == 0 || (mouse.callback_cs == 0 && mouse.callback_ip == 0) || callback_busy(&cpu.bus) {
        return false;
    }
    let mouse = &mut cpu.bus.mouse;
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

    let mut data: Vec<u8> = regs.into_iter().chain([handler_ip, handler_cs]).flat_map(u16::to_le_bytes).collect();
    data.push(1); // busy
    cpu.bus.write_rom(CALLBACK_DATA, &data);

    cpu.push(cpu.flags16());
    cpu.push(cpu.cs());
    cpu.push(cpu.ip());
    cpu.set_cs(0xF000);
    cpu.set_ip((CALLBACK_STUB - 0xF0000) as u16);
    cpu.set_cpu_flag(crate::cpu::CpuFlags::IF, false);
    cpu.set_cpu_flag(crate::cpu::CpuFlags::TF, false);
    true
}

impl MouseState {
    /// Whether the PS/2 device has a report to send at `now` (emulated
    /// microseconds): reporting is on, motion or buttons changed, and its
    /// rate lets it report again.
    pub fn ps2_report_due(&self, now: u64) -> bool {
        let ps2 = &self.ps2;
        ps2.enabled && now >= ps2.next_report && (ps2.dx != 0 || ps2.dy != 0 || self.buttons != ps2.reported_buttons)
    }

    /// The next PS/2 report, its three bytes: the status (the buttons, and
    /// the signs of the motion), X and Y (up positive) of at most 255
    /// counts each; motion beyond that goes in the next.
    pub fn take_ps2_packet(&mut self, now: u64) -> [u8; 3] {
        let ps2 = &mut self.ps2;
        let x = ps2.dx.clamp(-255, 255);
        let y = (-ps2.dy).clamp(-255, 255);
        ps2.dx -= x;
        ps2.dy += y;
        ps2.reported_buttons = self.buttons;
        ps2.next_report = now + 1_000_000 / ps2.rate.max(10) as u64;
        let status = 0x08 | (self.buttons & 0x07) | ((x < 0) as u8) << 4 | ((y < 0) as u8) << 5;
        [status, x as u8, y as u8]
    }

    /// A byte for the PS/2 mouse, sent through the keyboard controller's
    /// command D4h: its answer, an ACK (FAh) and what the command returns.
    pub fn ps2_command(&mut self, byte: u8, now: u64) -> Vec<u8> {
        const ACK: u8 = 0xFA;
        if let Some(command) = self.ps2.pending_command.take() {
            match command {
                0xF3 => self.ps2.rate = byte as u16,
                _ => self.ps2.resolution = byte & 0x03,
            }
            return vec![ACK];
        }
        match byte {
            // Reset: passed its test, a mouse.
            0xFF => {
                self.ps2.reset();
                vec![ACK, 0xAA, 0x00]
            }
            // Defaults.
            0xF6 => {
                self.ps2.reset();
                vec![ACK]
            }
            0xF5 | 0xF4 => {
                self.ps2.enabled = byte == 0xF4;
                vec![ACK]
            }
            0xF3 | 0xE8 => {
                self.ps2.pending_command = Some(byte);
                vec![ACK]
            }
            // Identify: a standard mouse.
            0xF2 => vec![ACK, 0x00],
            // Status: the mode, buttons and scaling, the resolution, the rate.
            0xE9 => {
                let ps2 = &self.ps2;
                let status = (self.buttons & BUTTON_RIGHT != 0) as u8
                    | ((self.buttons & BUTTON_MIDDLE != 0) as u8) << 1
                    | ((self.buttons & BUTTON_LEFT != 0) as u8) << 2
                    | (ps2.scaling as u8) << 4
                    | (ps2.enabled as u8) << 5;
                vec![ACK, status, ps2.resolution, ps2.rate as u8]
            }
            0xE6 | 0xE7 => {
                self.ps2.scaling = byte == 0xE7;
                vec![ACK]
            }
            // Read data: a report now.
            0xEB => {
                let mut answer = vec![ACK];
                answer.extend(self.take_ps2_packet(now));
                answer
            }
            _ => vec![ACK],
        }
    }
}

/// INT 15h AH=C2h, the BIOS's PS/2 pointing device services, AL the
/// function: AH 0 and CF clear, or the error in AH (01h invalid function,
/// 02h invalid input, 05h no handler installed) with CF set.
pub fn ps2_bios(cpu: &mut crate::cpu::Cpu) {
    use iced_x86::Register;
    let bh = cpu.get_reg8(Register::BH);
    let result = match cpu.get_al() {
        // Enable (BH=1) or disable (BH=0) reports. Enabled, the line is
        // unmasked, as a BIOS that found the device leaves it.
        0x00 => match bh {
            0 => {
                cpu.bus.mouse.ps2.enabled = false;
                cpu.bus.mouse.ps2.packet.clear();
                cpu.bus.kbc.flush_aux();
                Ok(())
            }
            1 if cpu.bus.mouse.ps2.handler == (0, 0) => Err(0x05),
            1 => {
                cpu.bus.mouse.ps2.enabled = true;
                if cpu.v86() {
                    // Through the ports, for a V86 monitor that keeps
                    // the machine's masks, as Windows' VPICD does.
                    use crate::bios::PortAccess::Update;
                    cpu.bus.port_accesses.push_back(Update { port: 0xA1, keep: !0x10, set: 0 });
                    cpu.bus.port_accesses.push_back(Update { port: 0x21, keep: !0x04, set: 0 });
                } else {
                    cpu.bus.pic.slave.imr &= !0x10;
                    cpu.bus.pic.master.imr &= !0x04;
                }
                Ok(())
            }
            _ => Err(0x02),
        },
        // Reset: BH the device ID (0, a mouse), BL AAh (passed its test).
        0x01 => {
            cpu.bus.mouse.ps2.reset();
            cpu.bus.mouse.ps2.packet.clear();
            cpu.bus.kbc.flush_aux();
            cpu.set_reg8(Register::BH, 0x00);
            cpu.set_reg8(Register::BL, 0xAA);
            Ok(())
        }
        // Reports per second: 10, 20, 40, 60, 80, 100 or 200.
        0x02 => match [10, 20, 40, 60, 80, 100, 200].get(bh as usize) {
            Some(&rate) => {
                cpu.bus.mouse.ps2.rate = rate;
                Ok(())
            }
            None => Err(0x02),
        },
        // Resolution: 1, 2, 4 or 8 counts per millimetre.
        0x03 if bh <= 3 => {
            cpu.bus.mouse.ps2.resolution = bh;
            Ok(())
        }
        0x03 => Err(0x02),
        // The device type: a mouse.
        0x04 => {
            cpu.set_reg8(Register::BH, 0x00);
            Ok(())
        }
        // Initialize for packets of BH bytes: reset.
        0x05 if (1..=8).contains(&bh) => {
            cpu.bus.mouse.ps2.reset();
            cpu.bus.mouse.ps2.packet.clear();
            cpu.bus.kbc.flush_aux();
            Ok(())
        }
        0x05 => Err(0x02),
        // Status (BH=0): BL the buttons (right bit 0, left bit 2), 2:1
        // scaling and reporting, CL the resolution, DL the rate. BH=1 and
        // 2 set 1:1 and 2:1 scaling.
        0x06 => match bh {
            0 => {
                let mouse = &cpu.bus.mouse;
                let buttons = mouse.buttons;
                let status = (buttons & BUTTON_RIGHT != 0) as u8
                    | ((buttons & BUTTON_MIDDLE != 0) as u8) << 1
                    | ((buttons & BUTTON_LEFT != 0) as u8) << 2
                    | (mouse.ps2.scaling as u8) << 4
                    | (mouse.ps2.enabled as u8) << 5;
                let (resolution, rate) = (mouse.ps2.resolution, mouse.ps2.rate as u8);
                cpu.set_reg8(Register::BL, status);
                cpu.set_reg8(Register::CL, resolution);
                cpu.set_reg8(Register::DL, rate);
                Ok(())
            }
            1 | 2 => {
                cpu.bus.mouse.ps2.scaling = bh == 2;
                Ok(())
            }
            _ => Err(0x02),
        },
        // The handler at ES:BX, which the IRQ 12 handler calls.
        0x07 => {
            cpu.bus.mouse.ps2.handler = (cpu.es(), cpu.bx());
            Ok(())
        }
        _ => Err(0x01),
    };
    cpu.set_reg8(Register::AH, result.err().unwrap_or(0));
    cpu.set_cpu_flag(crate::cpu::CpuFlags::CF, result.is_err());
}

/// The BIOS's IRQ 12 handler (`bios::PS2_HANDLER`) hands the byte it read
/// from port 60h in AL here (`bios::SERVICE_PS2_REPORT`). With the last of
/// a report's three: the status, X and Y in AX, BX and CX, and DX 1 with
/// the program's handler at `bios::PS2_HANDLER_ADDRESS` for it to call;
/// DX 0 otherwise. A report starts with a byte with bit 3 set.
pub fn ps2_report(cpu: &mut crate::cpu::Cpu) {
    let byte = cpu.get_al();
    cpu.set_dx(0);
    let mouse = &mut cpu.bus.mouse;
    if mouse.ps2.packet.is_empty() && byte & 0x08 == 0 {
        return;
    }
    mouse.ps2.packet.push(byte);
    if mouse.ps2.packet.len() < 3 {
        return;
    }
    let packet = std::mem::take(&mut mouse.ps2.packet);
    if !mouse.ps2.enabled || mouse.ps2.handler == (0, 0) {
        return;
    }
    let (status, x, y) = (packet[0] as u16, packet[1] as u16, packet[2] as u16);
    let (segment, offset) = mouse.ps2.handler;
    let at = 0xF0000 + crate::bios::PS2_HANDLER_ADDRESS as usize;
    cpu.bus.write_rom(at, &[offset.to_le_bytes(), segment.to_le_bytes()].concat());
    cpu.set_ax(status);
    cpu.set_bx(x);
    cpu.set_cx(y);
    cpu.set_dx(1);
}

crate::state_fields!(MouseState {
    installed, hide_counter, x, y, buttons, min_x, max_x, min_y, max_y,
    press_count, press_x, press_y, release_count, release_x, release_y,
    mickey_x, mickey_y, mickey_accum_x, mickey_accum_y,
    callback_mask, callback_cs, callback_ip, pending_callback_events,
    last_callback_mickey_x, last_callback_mickey_y, rest_x, rest_y, ps2,
});


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_captured_mouse_keeps_counting_at_the_edge() {
        let mut mouse = MouseState::new();
        mouse.reset(640, 200);
        mouse.set_position(639, 100);
        let before = mouse.mickey_x;
        for _ in 0..10 {
            mouse.move_by(3.0, 0.0);
        }
        assert_eq!(mouse.x, 639, "the cursor stops at the edge");
        assert_eq!(mouse.mickey_x - before, 20, "the mickeys count all 30 pixels");
        assert_ne!(mouse.pending_callback_events & 0x01, 0);

        // Parts of a pixel add up.
        let mut mouse = MouseState::new();
        mouse.reset(640, 200);
        for _ in 0..4 {
            mouse.move_by(0.25, -0.5);
        }
        assert_eq!((mouse.x, mouse.y), (321, 98));
    }
}
