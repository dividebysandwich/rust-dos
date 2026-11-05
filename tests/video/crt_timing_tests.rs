//! Input Status 1 (port 3DAh) follows the CRT timing the VGA registers
//! describe, in emulated time: vertical retrace in bit 3, display enable
//! off in bit 0, and the Start Address latched when a retrace begins.

use iced_x86::Register;
use rust_dos::cpu::Cpu;
use rust_dos::interrupts::int10;
use std::path::PathBuf;

/// 100 instructions per microsecond, so one port read (1 µs) is 100.
const CYCLES_PER_MS: u32 = 100_000;

fn machine(mode: u8) -> Cpu {
    let mut cpu = Cpu::new(PathBuf::from("."));
    cpu.bus.set_cycles_per_ms(CYCLES_PER_MS);
    cpu.set_ax(mode as u16);
    cpu.set_reg8(Register::AH, 0x00);
    int10::handle(&mut cpu);
    cpu
}

fn out(cpu: &mut Cpu, port: u16, index: u8, value: u8) {
    cpu.bus.io_write(port, index);
    cpu.bus.io_write(port + 1, value);
}

/// Read port 3DAh the way a polling loop does (IN, TEST, Jcc), servicing
/// timer interrupts as the execution loop would.
fn status(cpu: &mut Cpu) -> u8 {
    cpu.bus.clock.icount += 3;
    if cpu.bus.clock.icount >= cpu.bus.clock.deadline {
        cpu.bus.service_timers();
    }
    cpu.bus.io_read(0x3DA)
}

/// Poll until `bit` of 3DAh reads `level`.
fn wait(cpu: &mut Cpu, bit: u8, level: bool) {
    for _ in 0..1_000_000 {
        if (status(cpu) & bit != 0) == level {
            return;
        }
    }
    panic!("3DAh bit {:02X} never became {}", bit, level);
}

/// Time from one vertical retrace to the next, in ns.
fn frame_ns(cpu: &mut Cpu) -> u64 {
    wait(cpu, 0x08, false);
    wait(cpu, 0x08, true);
    let start = cpu.bus.clock.now_ns();
    wait(cpu, 0x08, false);
    wait(cpu, 0x08, true);
    cpu.bus.clock.now_ns() - start
}

/// Display enable pulses (bit 0 going from 1 to 0) between the end of one
/// vertical retrace and the start of the next: the displayed lines.
fn displayed_lines(cpu: &mut Cpu) -> u32 {
    wait(cpu, 0x08, true);
    wait(cpu, 0x08, false);
    let mut lines = 0;
    let mut last = status(cpu);
    loop {
        let now = status(cpu);
        if now & 0x08 != 0 {
            return lines;
        }
        if last & 0x01 != 0 && now & 0x01 == 0 {
            lines += 1;
        }
        last = now;
    }
}

fn assert_near(actual: u64, expected: u64) {
    let tolerance = expected / 1000 + 2_000; // 0.1% plus two polls
    assert!(
        actual.abs_diff(expected) <= tolerance,
        "{} ns, expected {} ns",
        actual,
        expected
    );
}

#[test]
fn text_and_mode_13h_run_at_70_hz() {
    let mut cpu = machine(0x03);
    assert_near(frame_ns(&mut cpu), 14_268_000);
    let mut cpu = machine(0x13);
    assert_near(frame_ns(&mut cpu), 14_268_000);
    assert_eq!(displayed_lines(&mut cpu), 400);
}

#[test]
fn mode_12h_is_60_hz_with_480_lines() {
    let mut cpu = machine(0x12);
    assert_near(frame_ns(&mut cpu), 16_683_000);
    assert_eq!(displayed_lines(&mut cpu), 480);
}

/// The CRTC values of the classic unchained 320x240 mode, programmed over
/// mode 13h.
fn mode_x_240(cpu: &mut Cpu) {
    let protect = cpu.bus.vga.crtc_regs[0x11] & 0x7F;
    out(cpu, 0x3D4, 0x11, protect);
    cpu.bus.io_write(0x3C2, 0xE3);
    let registers = [(0x06, 0x0D), (0x07, 0x3E), (0x10, 0xEA), (0x11, 0xAC), (0x12, 0xDF), (0x15, 0xE7), (0x16, 0x06)];
    for (index, value) in registers {
        out(cpu, 0x3D4, index, value);
    }
}

#[test]
fn mode_x_240_lines_come_from_the_registers() {
    let mut cpu = machine(0x13);
    mode_x_240(&mut cpu);
    assert_near(frame_ns(&mut cpu), 527 * 31_778);
    assert_eq!(displayed_lines(&mut cpu), 480);
    assert_eq!(cpu.bus.vga.graphics_size(), (320, 240));
}

#[test]
fn write_protect_ignores_crtc_timing_writes() {
    // Mode 13h leaves Vertical Retrace End bit 7 set.
    let mut cpu = machine(0x13);
    out(&mut cpu, 0x3D4, 0x06, 0x0D);
    out(&mut cpu, 0x3D4, 0x07, 0x3E);
    assert_eq!(cpu.bus.vga.crtc_regs[0x06], 0xBF);
    // Only the Line Compare bit of the Overflow register gets through.
    assert_eq!(cpu.bus.vga.crtc_regs[0x07], 0x1F);
    assert_near(frame_ns(&mut cpu), 14_268_000);
}

#[test]
fn inconsistent_registers_keep_the_last_timing() {
    let mut cpu = machine(0x13);
    out(&mut cpu, 0x3D4, 0x11, 0x0E);
    out(&mut cpu, 0x3D4, 0x06, 0x00);
    out(&mut cpu, 0x3D4, 0x07, 0x00);
    assert_near(frame_ns(&mut cpu), 14_268_000);
}

#[test]
fn start_address_latches_when_a_retrace_begins() {
    let mut cpu = machine(0x13);
    wait(&mut cpu, 0x08, false);
    out(&mut cpu, 0x3D4, 0x0C, 0x10);
    status(&mut cpu);
    assert_eq!(cpu.bus.vga.latched_start_addr, 0, "not before the retrace");
    wait(&mut cpu, 0x08, true);
    assert_eq!(cpu.bus.vga.latched_start_addr, 0x1000);

    // A retrace passes while nobody looks; the next Start Address written
    // after it waits for the retrace after that.
    wait(&mut cpu, 0x08, false);
    out(&mut cpu, 0x3D4, 0x0C, 0x20);
    let frame_ns = cpu.bus.vga.timing().frame_ns();
    cpu.bus.clock.icount += frame_ns * CYCLES_PER_MS as u64 / 1_000_000;
    out(&mut cpu, 0x3D4, 0x0C, 0x30);
    assert_eq!(cpu.bus.vga.latched_start_addr, 0x2000);
    status(&mut cpu);
    assert_eq!(cpu.bus.vga.latched_start_addr, 0x2000);
    wait(&mut cpu, 0x08, false);
    wait(&mut cpu, 0x08, true);
    assert_eq!(cpu.bus.vga.latched_start_addr, 0x3000);
}

/// Pinball Fantasies' calibration (4C2A:04EC-05C6): find the PIT ch0
/// count that lasts as long as the displayed lines of a frame, by counting
/// display enable pulses until a mode 0 one-shot fires.
#[test]
fn pinball_fantasies_calibration_converges() {
    let mut cpu = machine(0x13);
    let target = displayed_lines(&mut cpu) as i32;
    assert_eq!(target, 400);

    let mut di: i32 = 0x1CE8;
    let mut hits = 0;
    for _ in 0..200 {
        wait(&mut cpu, 0x08, false);
        wait(&mut cpu, 0x08, true);
        cpu.bus.io_write(0x43, 0x30);
        cpu.bus.io_write(0x40, di as u8);
        cpu.bus.io_write(0x40, (di >> 8) as u8);
        cpu.bus.pic.master.irr &= !0x01;
        // The interrupt handler takes the count of lines completed so far.
        let fired = |cpu: &Cpu| cpu.bus.pic.master.irr & 0x01 != 0;
        let mut lines = 0;
        'count: loop {
            for level in [0, 1] {
                while status(&mut cpu) & 0x01 != level {
                    if fired(&cpu) {
                        break 'count;
                    }
                }
            }
            lines += 1;
        }
        let diff = lines - target;
        if diff == 0 {
            hits += 1;
            if hits == 10 {
                // From the start of the retrace, the count lasts the 37
                // lines to the top of the display and 400 displayed lines,
                // and ends before the first line of the next frame.
                assert!((16_500..18_500).contains(&di), "count {}", di);
                return;
            }
        } else {
            di -= if diff.abs() == 1 { diff } else { diff * 10 };
        }
    }
    panic!("no convergence, count {} after {} hits", di, hits);
}
