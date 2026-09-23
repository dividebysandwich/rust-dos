use crate::cpu::Cpu;
use crate::mouse::{BUTTON_LEFT, BUTTON_MIDDLE, BUTTON_RIGHT};
use crate::video::VideoMode;
use iced_x86::Register;

/// Return (width, height) in driver virtual units for the current video mode.
/// For graphics modes the driver historically reports X in pixels but rounded
/// up to multiples that match the internal mouse resolution. We keep it simple
/// and use the mode's native pixel grid, except for 320-wide modes where the
/// x range is doubled (convention for the DOS mouse driver).
fn virtual_screen_dims(mode: VideoMode) -> (i32, i32) {
    let (w, h) = mode.dimensions();
    let virt_w = if w < 640 { 640 } else { w as i32 };
    (virt_w, h as i32)
}

/// Bytes of driver state AX=16h saves: the event handler's mask and
/// address, the show/hide counter and the position.
const STATE_SIZE: usize = 12;

pub fn handle(cpu: &mut Cpu) {
    let ax = cpu.ax();
    let func = ax & 0xFFFF;

    match func {
        0x0000 => {
            // Reset driver + query installation status.
            // Returns: AX = FFFFh if installed, 0000h if not. BX = button count.
            let (w, h) = virtual_screen_dims(cpu.bus.video_mode);
            cpu.bus.mouse.reset(w, h);
            crate::mouse::clear_callback_busy(&mut cpu.bus);
            cpu.set_ax(0xFFFF);
            cpu.set_bx(3); // 3-button mouse
        }

        0x0001 => {
            // Show mouse cursor. Decrements hide_counter; visible when 0.
            if cpu.bus.mouse.hide_counter > 0 {
                cpu.bus.mouse.hide_counter -= 1;
            }
        }

        0x0002 => {
            // Hide mouse cursor. Increments hide_counter.
            cpu.bus.mouse.hide_counter += 1;
        }

        0x0003 => {
            // Get position + button state.
            //   BX = button state (bit 0 = left, 1 = right, 2 = middle)
            //   CX = X in virtual coords
            //   DX = Y in virtual coords
            cpu.set_bx(cpu.bus.mouse.buttons as u16);
            cpu.set_cx((cpu.bus.mouse.x as u16) & 0xFFFF);
            cpu.set_dx((cpu.bus.mouse.y as u16) & 0xFFFF);
        }

        0x0004 => {
            // Set cursor position. CX = X, DX = Y.
            let x = cpu.cx() as i16 as i32;
            let y = cpu.dx() as i16 as i32;
            cpu.bus.mouse.set_position(x, y);
        }

        0x0005 => {
            // Get button press info. BX selects button (0=L, 1=R, 2=M).
            // Returns AX = current button state, BX = press count since last
            // query (reset to 0), CX = X at last press, DX = Y at last press.
            let btn = (cpu.bx() as usize).min(2);
            let count = cpu.bus.mouse.press_count[btn];
            cpu.set_ax(cpu.bus.mouse.buttons as u16);
            cpu.set_bx(count);
            cpu.set_cx((cpu.bus.mouse.press_x[btn] as u16) & 0xFFFF);
            cpu.set_dx((cpu.bus.mouse.press_y[btn] as u16) & 0xFFFF);
            cpu.bus.mouse.press_count[btn] = 0;
        }

        0x0006 => {
            // Get button release info. Mirror of 0x0005.
            let btn = (cpu.bx() as usize).min(2);
            let count = cpu.bus.mouse.release_count[btn];
            cpu.set_ax(cpu.bus.mouse.buttons as u16);
            cpu.set_bx(count);
            cpu.set_cx((cpu.bus.mouse.release_x[btn] as u16) & 0xFFFF);
            cpu.set_dx((cpu.bus.mouse.release_y[btn] as u16) & 0xFFFF);
            cpu.bus.mouse.release_count[btn] = 0;
        }

        0x0007 => {
            // Set horizontal range: CX = min, DX = max.
            let lo = cpu.cx() as i16 as i32;
            let hi = cpu.dx() as i16 as i32;
            let (min, max) = if lo <= hi { (lo, hi) } else { (hi, lo) };
            cpu.bus.mouse.min_x = min;
            cpu.bus.mouse.max_x = max;
            // Also clamp current position.
            let cx = cpu.bus.mouse.x;
            let cy = cpu.bus.mouse.y;
            cpu.bus.mouse.set_position(cx, cy);
        }

        0x0008 => {
            // Set vertical range: CX = min, DX = max.
            let lo = cpu.cx() as i16 as i32;
            let hi = cpu.dx() as i16 as i32;
            let (min, max) = if lo <= hi { (lo, hi) } else { (hi, lo) };
            cpu.bus.mouse.min_y = min;
            cpu.bus.mouse.max_y = max;
            let cx = cpu.bus.mouse.x;
            let cy = cpu.bus.mouse.y;
            cpu.bus.mouse.set_position(cx, cy);
        }

        0x0009 => {
            // Set graphics cursor shape. ES:DX -> bitmap data (2 * 16 words).
            // We don't draw a custom cursor, so accept and ignore.
        }

        0x000A => {
            // Set text cursor shape. Accept and ignore.
        }

        0x000B => {
            // Read motion counters. Returns CX = mickey X, DX = mickey Y. Clears.
            cpu.set_cx(cpu.bus.mouse.mickey_x as u16);
            cpu.set_dx(cpu.bus.mouse.mickey_y as u16);
            cpu.bus.mouse.mickey_x = 0;
            cpu.bus.mouse.mickey_y = 0;
        }

        0x000C => {
            // Set event handler. CX = event mask, ES:DX = far pointer to a
            // handler the main loop CALL FARs on those events.
            cpu.bus.mouse.callback_mask = cpu.cx();
            cpu.bus.mouse.callback_cs = cpu.es();
            cpu.bus.mouse.callback_ip = cpu.dx();
            crate::mouse::clear_callback_busy(&mut cpu.bus);
        }

        0x0014 => {
            // Swap event handlers: install CX / ES:DX, return the old ones.
            let (mask, seg, off) = (cpu.cx(), cpu.es(), cpu.dx());
            let m = &mut cpu.bus.mouse;
            let old = (m.callback_mask, m.callback_cs, m.callback_ip);
            (m.callback_mask, m.callback_cs, m.callback_ip) = (mask, seg, off);
            crate::mouse::clear_callback_busy(&mut cpu.bus);
            cpu.set_cx(old.0);
            cpu.set_es(old.1);
            cpu.set_dx(old.2);
        }

        0x0015 => {
            // Size of the driver state that AX=16h/17h save and restore.
            cpu.set_bx(STATE_SIZE as u16);
        }

        0x0016 | 0x0017 => {
            // Save the driver state to ES:DX, or restore it from there: the
            // event handler, the cursor's visibility and position.
            let addr = cpu.get_physical_addr(cpu.es(), cpu.dx());
            if func == 0x0016 {
                let m = &cpu.bus.mouse;
                let words = [m.callback_mask, m.callback_cs, m.callback_ip, m.hide_counter as u16, m.x as u16, m.y as u16];
                for (i, w) in words.into_iter().enumerate() {
                    cpu.bus.write_16(addr + i * 2, w);
                }
            } else {
                let w: Vec<u16> = (0..6).map(|i| cpu.bus.read_16(addr + i * 2)).collect();
                let m = &mut cpu.bus.mouse;
                (m.callback_mask, m.callback_cs, m.callback_ip) = (w[0], w[1], w[2]);
                m.hide_counter = w[3] as i16 as i32;
                (m.x, m.y) = (w[4] as i16 as i32, w[5] as i16 as i32);
                crate::mouse::clear_callback_busy(&mut cpu.bus);
            }
        }

        0x000F => {
            // Set mickeys-per-8-pixels. CX = X mickeys, DX = Y mickeys. Ignored.
        }

        0x0010 => {
            // Conditional-off region. We don't implement.
        }

        0x0013 => {
            // Set double-speed threshold. Ignored.
        }

        0x001A => {
            // Set mouse sensitivity. Ignored.
        }

        0x001B => {
            // Get mouse sensitivity. Return plausible defaults.
            cpu.set_bx(50); // horiz mickeys per 8px
            cpu.set_cx(50); // vert
            cpu.set_dx(50); // double-speed threshold
        }

        0x0024 => {
            // Get driver version, type and IRQ.
            // BX = version (high byte major, low byte minor) -> 8.20
            // CH = mouse type (4 = PS/2)
            // CL = IRQ (0 = PS/2)
            cpu.set_bx(0x0814);
            cpu.set_reg8(Register::CH, 0x04);
            cpu.set_reg8(Register::CL, 0x00);
        }

        _ => {
            cpu.bus.log_string(&format!(
                "[MOUSE] Unhandled INT 33h AX={:04X} BX={:04X} CX={:04X} DX={:04X}",
                cpu.ax(), cpu.bx(), cpu.cx(), cpu.dx()
            ));
        }
    }

    // Keep BUTTON_* used even when only some call sites reference them.
    let _ = (BUTTON_LEFT, BUTTON_RIGHT, BUTTON_MIDDLE);
}
