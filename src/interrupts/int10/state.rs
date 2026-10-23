//! INT 10h AH=1Ch, the VGA BIOS's video-state query, save, and restore.

use crate::cpu::Cpu;
use iced_x86::Register;

const HEADER_SIZE: u32 = 0x20;
const HARDWARE_SIZE: usize = 0x46;
const BIOS_DATA_START: u32 = 0x449;
const BIOS_DATA_SIZE: usize = 0x60;
const DAC_SIZE: usize = 0x304;
const STATE_MASK: u16 = 0x0007;

/// The number of bytes required by the selected sections, rounded to blocks.
pub(super) fn buffer_size(states: u16) -> u32 {
    let states = states & STATE_MASK;
    if states == 0 {
        return 0;
    }

    let mut size = HEADER_SIZE;
    if states & 1 != 0 {
        size += HARDWARE_SIZE as u32;
    }
    if states & 2 != 0 {
        size += BIOS_DATA_SIZE as u32;
    }
    if states & 4 != 0 {
        size += DAC_SIZE as u32;
    }
    size.div_ceil(64)
}

/// Implements INT 10h AH=1Ch. The buffer starts at ES:BX; its first 32
/// bytes contain offsets to each requested state section.
pub(super) fn handle(cpu: &mut Cpu) {
    let function = cpu.get_al();
    let states = cpu.cx() & STATE_MASK;
    match function {
        0x00 => {
            cpu.set_bx(buffer_size(states) as u16);
            cpu.set_reg8(Register::AL, 0x1C);
        }
        0x01 | 0x02 if states != 0 => {
            let base = (cpu.es() as u32)
                .wrapping_mul(16)
                .wrapping_add(cpu.bx() as u32);
            let mut offset = HEADER_SIZE as usize;
            if function == 0x01 {
                cpu.bus
                    .guest_fill(base, (buffer_size(states) * 64) as usize, 0);
            }

            if states & 1 != 0 {
                let section = offset;
                offset += HARDWARE_SIZE;
                if function == 0x01 {
                    state_pointer(cpu, base, 0, section);
                    save_hardware(cpu, base + section as u32);
                } else {
                    let address = state_address(cpu, base, 0);
                    restore_hardware(cpu, address);
                }
            }
            if states & 2 != 0 {
                let section = offset;
                offset += BIOS_DATA_SIZE;
                if function == 0x01 {
                    state_pointer(cpu, base, 2, section);
                    let mut data = [0; BIOS_DATA_SIZE];
                    cpu.bus.guest_read_bytes(BIOS_DATA_START, &mut data);
                    cpu.bus
                        .guest_write_bytes(base.wrapping_add(section as u32), &data);
                } else {
                    let mut data = [0; BIOS_DATA_SIZE];
                    let address = state_address(cpu, base, 2);
                    cpu.bus.guest_read_bytes(address, &mut data);
                    cpu.bus.guest_write_bytes(BIOS_DATA_START, &data);
                    restore_mode_from_bda(cpu);
                }
            }
            if states & 4 != 0 {
                let section = offset;
                if function == 0x01 {
                    state_pointer(cpu, base, 4, section);
                    save_dac(cpu, base + section as u32);
                } else {
                    let address = state_address(cpu, base, 4);
                    restore_dac(cpu, address);
                }
            }
            cpu.set_reg8(Register::AL, 0x1C);
        }
        _ => cpu.bus.log_string(&format!(
            "[BIOS] Unhandled INT 10h AH=1Ch AL={function:02X}"
        )),
    }
}

fn state_pointer(cpu: &mut Cpu, base: u32, slot: u32, section: usize) {
    let offset = cpu.bx().wrapping_add(section as u16);
    cpu.bus.guest_write_16(base.wrapping_add(slot), offset);
}

fn state_address(cpu: &mut Cpu, base: u32, slot: u32) -> u32 {
    let offset = cpu.bus.guest_read_16(base.wrapping_add(slot));
    (cpu.es() as u32)
        .wrapping_mul(16)
        .wrapping_add(offset as u32)
}

fn save_hardware(cpu: &mut Cpu, address: u32) {
    let crtc_address = [cpu.bus.guest_read_8(0x0463), cpu.bus.guest_read_8(0x0464)];
    let vga = &cpu.bus.vga;
    let mut state = [0; HARDWARE_SIZE];
    state[0] = vga.sequencer_index;
    state[1] = vga.crtc_index;
    state[2] = vga.graphics_index;
    state[3] = vga.attribute_index | 0x20;
    state[5..9].copy_from_slice(&vga.sequencer_regs[1..5]);
    state[9] = vga.misc_output_reg;
    state[0x0A..0x23].copy_from_slice(&vga.crtc_regs);
    state[0x23..0x33].copy_from_slice(&vga.attribute_regs[..16]);
    state[0x33..0x37].copy_from_slice(&vga.attribute_regs[0x10..0x14]);
    state[0x37..0x40].copy_from_slice(&vga.graphics_regs);
    state[0x40..0x42].copy_from_slice(&crtc_address);
    state[0x42..0x46].copy_from_slice(&vga.latches.get());
    cpu.bus.guest_write_bytes(address, &state);
}

fn restore_hardware(cpu: &mut Cpu, address: u32) {
    let mut state = [0; HARDWARE_SIZE];
    cpu.bus.guest_read_bytes(address, &mut state);
    let crtc_port = match u16::from_le_bytes([state[0x40], state[0x41]]) {
        0x03B4 | 0x03D4 => u16::from_le_bytes([state[0x40], state[0x41]]),
        _ => 0x03D4,
    };
    cpu.bus.io_write(0x3C4, 0);
    cpu.bus.io_write(0x3C5, 0x01);
    for (index, value) in (1..5).zip(&state[5..9]) {
        cpu.bus.io_write(0x3C4, index);
        cpu.bus.io_write(0x3C5, *value);
    }
    cpu.bus.io_write(0x3C2, state[9]);
    cpu.bus.io_write(0x3C4, 0);
    cpu.bus.io_write(0x3C5, 0x03);

    cpu.bus.io_write(crtc_port, 0x11);
    cpu.bus.io_write(crtc_port + 1, state[0x0A + 0x11] & 0x7F);
    for (index, value) in (0..25).zip(&state[0x0A..0x23]) {
        cpu.bus.io_write(crtc_port, index);
        cpu.bus.io_write(crtc_port + 1, *value);
    }

    for (index, value) in (0..9).zip(&state[0x37..0x40]) {
        cpu.bus.io_write(0x3CE, index);
        cpu.bus.io_write(0x3CF, *value);
    }
    cpu.bus.io_read(crtc_port + 6);
    for (index, value) in (0..20).zip(&state[0x23..0x37]) {
        cpu.bus.io_write(0x3C0, index);
        cpu.bus.io_write(0x3C0, *value);
    }
    cpu.bus.io_read(crtc_port + 6);
    cpu.bus.io_write(0x3C4, state[0]);
    cpu.bus.io_write(crtc_port, state[1]);
    cpu.bus.io_write(0x3CE, state[2]);
    cpu.bus.io_write(0x3C0, state[3]);
    cpu.bus.vga.attribute_flip_flop = false;
    cpu.bus
        .vga
        .latches
        .set(state[0x42..0x46].try_into().unwrap_or([0; 4]));
    cpu.bus.vga.invalidate_timing();
    cpu.bus.vga.mark_dirty_full();
    cpu.bus.guest_write_8(0x0463, state[0x40]);
    cpu.bus.guest_write_8(0x0464, state[0x41]);
}

fn save_dac(cpu: &mut Cpu, address: u32) {
    let vga = &cpu.bus.vga;
    let mut state = [0; DAC_SIZE];
    state[0] = u8::from(vga.dac_state == 3);
    state[1] = if state[0] == 0 {
        vga.dac_write_index
    } else {
        vga.dac_read_index
    };
    state[2] = vga.dac_mask;
    let palette_size = state[3..0x303].len().min(vga.palette.len());
    state[3..3 + palette_size].copy_from_slice(&vga.palette[..palette_size]);
    state[0x303] = vga.attribute_regs[0x14];
    cpu.bus.guest_write_bytes(address, &state);
}

fn restore_dac(cpu: &mut Cpu, address: u32) {
    let mut state = [0; DAC_SIZE];
    cpu.bus.guest_read_bytes(address, &mut state);
    let vga = &mut cpu.bus.vga;
    vga.dac_state = if state[0] == 0 { 0 } else { 3 };
    if state[0] == 0 {
        vga.dac_write_index = state[1];
    } else {
        vga.dac_read_index = state[1];
    }
    vga.dac_step = 0;
    vga.dac_mask = state[2];
    let palette_size = vga.palette.len().min(state[3..0x303].len());
    vga.palette[..palette_size].copy_from_slice(&state[3..3 + palette_size]);
    vga.attribute_regs[0x14] = state[0x303];
    vga.mark_dirty_full();
}

fn restore_mode_from_bda(cpu: &mut Cpu) {
    let mode = cpu.bus.guest_read_8(0x0449);
    cpu.bus.video_mode = match mode {
        0x00 => crate::video::VideoMode::Text40x25,
        0x01 => crate::video::VideoMode::Text40x25Color,
        0x02 => crate::video::VideoMode::Text80x25,
        0x03 => crate::video::VideoMode::Text80x25Color,
        0x04 => crate::video::VideoMode::Cga320x200Color,
        0x05 => crate::video::VideoMode::Cga320x200,
        0x06 => crate::video::VideoMode::Cga640x200,
        0x07 => crate::video::VideoMode::Mono80x25,
        0x08 => crate::video::VideoMode::Tandy160x200x16,
        0x09 => crate::video::VideoMode::Tandy320x200x16,
        0x0A => crate::video::VideoMode::Tandy640x200x4,
        0x0D => crate::video::VideoMode::Ega320x200,
        0x0E => crate::video::VideoMode::Ega640x200,
        0x0F => crate::video::VideoMode::Ega640x350Mono,
        0x10 => crate::video::VideoMode::Ega640x350,
        0x11 => crate::video::VideoMode::Vga640x480Mono,
        0x12 => crate::video::VideoMode::Vga640x480,
        0x13 => crate::video::VideoMode::Graphics320x200,
        _ => return,
    };
}
