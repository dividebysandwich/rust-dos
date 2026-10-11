use rust_dos::cpu::Cpu;
use rust_dos::cpu::CpuFlags;
use iced_x86::{Register, code_asm::*};
use rust_dos::interrupts::int33;
use std::path::PathBuf;

fn call(cpu: &mut Cpu, function: u16) {
    cpu.set_ax(function);
    int33::handle(cpu);
}

#[test]
fn dos4gw_can_allocate_an_unused_mouse_callback_vector() {
    let cpu = Cpu::new(PathBuf::from("."));
    let ivt = rust_dos::bios::default_ivt();
    for vector in 0x60..=0x65 {
        assert_eq!(ivt[vector], 0);
        assert_eq!(cpu.bus.read_32(vector * 4), 0);
    }
    assert_ne!(ivt[0x67], 0, "EMS remains installed");
    assert_ne!(ivt[0x33], 0, "the mouse driver remains installed");
}

#[test]
fn enable_activates_mouse_without_reset() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    assert!(!cpu.bus.mouse.installed);
    cpu.set_bx(0x1234);
    cpu.set_cx(0x5678);
    cpu.set_dx(0x9ABC);
    cpu.set_es(0x2000);

    call(&mut cpu, 0x0020);

    assert_eq!(cpu.ax(), 0xFFFF, "Descent II D2_3DFX.EXE/D2VOODOO.EXE enable check succeeds");
    assert_eq!((cpu.bx(), cpu.cx(), cpu.dx(), cpu.es()), (0x1234, 0x5678, 0x9ABC, 0x2000));
    assert!(cpu.bus.mouse.installed);
    assert!(cpu.bus.mouse.in_use(cpu.bus.clock.now_ns()));

    cpu.bus.mouse.move_by(3.0, 3.0);
    cpu.bus.mouse.button_down(0);
    call(&mut cpu, 0x0003);
    assert_eq!((cpu.bx(), cpu.cx(), cpu.dx()), (1, 3, 3));
    call(&mut cpu, 0x000B);
    assert_eq!((cpu.cx(), cpu.dx()), (2, 4));
}

/// BloodNet resets the driver with the software reset alone, after its
/// setup program left the cursor shown.
#[test]
fn software_reset_resets_the_driver_as_a_reset_does() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    call(&mut cpu, 0x0000);
    call(&mut cpu, 0x0001);
    cpu.set_cx(0);
    cpu.set_dx(100);
    call(&mut cpu, 0x0007);
    cpu.set_cx(0x0001);
    cpu.set_es(0x1234);
    cpu.set_dx(0x5678);
    call(&mut cpu, 0x000C);
    cpu.bus.mouse.move_by(30.0, 20.0);
    cpu.bus.mouse.installed = false;

    cpu.set_bx(0);
    call(&mut cpu, 0x0021);

    assert_eq!((cpu.ax(), cpu.bx()), (0xFFFF, 3), "installed, three buttons");
    let m = &cpu.bus.mouse;
    assert!(m.installed);
    assert_eq!(m.hide_counter, 1, "the cursor is hidden");
    assert_eq!((m.callback_mask, m.callback_cs, m.callback_ip), (0, 0, 0), "no event handler");
    assert_eq!((m.mickey_x, m.mickey_y), (0, 0));
    let mut fresh = Cpu::new(PathBuf::from("."));
    call(&mut fresh, 0x0000);
    let f = &fresh.bus.mouse;
    assert_eq!((m.x, m.y), (f.x, f.y), "the cursor is where a reset puts it");
    assert_eq!((m.min_x, m.max_x, m.min_y, m.max_y), (f.min_x, f.max_x, f.min_y, f.max_y), "the ranges cover the screen");
}

#[test]
fn enable_preserves_driver_configuration_and_pending_input() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    call(&mut cpu, 0x0000);
    call(&mut cpu, 0x0001);
    cpu.set_cx(10);
    cpu.set_dx(500);
    call(&mut cpu, 0x0007);
    cpu.set_cx(20);
    cpu.set_dx(150);
    call(&mut cpu, 0x0008);
    cpu.set_cx(100);
    cpu.set_dx(50);
    call(&mut cpu, 0x0004);
    cpu.set_cx(0x0003);
    cpu.set_es(0x1234);
    cpu.set_dx(0x5678);
    call(&mut cpu, 0x000C);
    cpu.bus.mouse.move_by(3.0, 3.0);
    cpu.bus.mouse.button_down(0);

    // Enabling is idempotent, not a reset or a show-cursor request.
    for _ in 0..2 {
        call(&mut cpu, 0x0020);
        assert_eq!(cpu.ax(), 0xFFFF);
    }
    let m = &cpu.bus.mouse;
    assert_eq!((m.x, m.y, m.hide_counter), (103, 53, 0));
    assert_eq!((m.min_x, m.max_x, m.min_y, m.max_y), (10, 500, 20, 150));
    assert_eq!((m.callback_mask, m.callback_cs, m.callback_ip), (0x0003, 0x1234, 0x5678));
    assert_eq!((m.buttons, m.press_count[0], m.pending_callback_events), (1, 1, 3));
    assert_eq!((m.mickey_x, m.mickey_y), (2, 4));
    assert!(rust_dos::mouse::deliver_callback(&mut cpu));
}

const POLL_SEG: u16 = 0x2000;
const CALLBACK_IP: u16 = 0x0200;
const CALLBACK_COUNT: usize = 0x20400;
const CALLBACK_ARGS: usize = CALLBACK_COUNT + 2;

fn polling_cpu() -> Cpu {
    let mut cpu = Cpu::new(PathBuf::from("."));
    cpu.set_cs(POLL_SEG);
    cpu.set_ip(0x100);
    cpu.set_ss(0x3000);
    cpu.set_sp(0xFF00);
    cpu.set_ds(0x4000);
    cpu.set_es(0x5000);
    cpu.set_cpu_flag(CpuFlags::IF, false);
    cpu.bus.mouse.callback_cs = POLL_SEG;
    cpu.bus.mouse.callback_ip = CALLBACK_IP;
    cpu.bus.mouse.callback_mask = 0x007E;
    // Two polls: the second can deliver an event queued during the first
    // callback. No STI anywhere, as in an extender's reflected RM call.
    for (i, byte) in [0xCD, 0x33, 0xCD, 0x33, 0x90].into_iter().enumerate() {
        cpu.bus.write_8(0x20100 + i, byte);
    }
    let mut a = CodeAssembler::new(16).unwrap();
    a.inc(word_ptr(0x400).cs()).unwrap();
    for (i, reg) in [ax, bx, cx, dx, si, di].into_iter().enumerate() {
        a.mov(word_ptr(0x402 + i as i32 * 2).cs(), reg).unwrap();
    }
    // Reentrant polling must not redispatch the still-running callback.
    a.mov(ax, 0x000B).unwrap();
    a.int(0x33).unwrap();
    for reg in [eax, ebx, ecx, edx, esi, edi, ebp] {
        a.mov(reg, 0xDEAD_BEEFu32).unwrap();
    }
    a.mov(ds, ax).unwrap();
    a.mov(es, ax).unwrap();
    a.retf().unwrap();
    for (i, byte) in a.assemble(CALLBACK_IP as u64).unwrap().into_iter().enumerate() {
        cpu.bus.write_8(((POLL_SEG as usize) << 4) + CALLBACK_IP as usize + i, byte);
    }
    cpu
}

fn execute_poll(cpu: &mut Cpu, function: u16) {
    cpu.set_ax(function);
    cpu.step(); // INT 33h: enters the HLE trap with IF clear.
    assert_eq!(cpu.cs(), 0xF000);
    assert!(!cpu.get_cpu_flag(CpuFlags::IF));
    cpu.step(); // HLE service: simulated IRET, then synchronous dispatch.
}

fn finish_callback(cpu: &mut Cpu, return_ip: u16) {
    for _ in 0..120 {
        if (cpu.cs(), cpu.ip()) == (POLL_SEG, return_ip) {
            assert!(!rust_dos::mouse::callback_busy(&cpu.bus));
            return;
        }
        cpu.step();
    }
    panic!("callback failed to return: {:04X}:{:04X}", cpu.cs(), cpu.ip());
}

#[test]
fn reflected_motion_poll_delivers_buttons_with_if_clear_and_preserves_full_registers() {
    let mut cpu = polling_cpu();
    let regs = [Register::EAX, Register::EBX, Register::ECX, Register::EDX, Register::ESI, Register::EDI, Register::EBP];
    for (i, reg) in regs.into_iter().enumerate() {
        cpu.set_reg(reg, 0x1234_5000 + i as u32);
    }
    cpu.bus.mouse.move_by(3.0, 3.0);
    cpu.bus.mouse.button_down(0);
    execute_poll(&mut cpu, 0x000B);
    assert_eq!((cpu.cs(), cpu.ip()), (0xF000, 0xF200));
    assert_eq!(cpu.bus.read_16(0x3FEFA), 0x0102, "return is after INT, not its HLE trap");
    assert_eq!(cpu.bus.mouse.pending_callback_events, 1, "unmatched motion stays pending");
    finish_callback(&mut cpu, 0x0102);
    assert_eq!(cpu.bus.read_16(CALLBACK_COUNT), 1);
    assert_eq!(cpu.bus.read_16(CALLBACK_ARGS), 2, "left press callback");
    assert_eq!(cpu.bus.read_16(CALLBACK_ARGS + 2), 1, "left button held");
    assert_eq!(cpu.reg(Register::EAX), 0x1234_000B);
    assert_eq!(cpu.reg(Register::EBX), 0x1234_5001);
    assert_eq!(cpu.reg(Register::ECX), 0x1234_0002, "motion poll result survives callback");
    assert_eq!(cpu.reg(Register::EDX), 0x1234_0004);
    for (i, reg) in regs.into_iter().enumerate().skip(4) {
        assert_eq!(cpu.reg(reg), 0x1234_5000 + i as u32);
    }
    assert_eq!((cpu.ds(), cpu.es(), cpu.sp()), (0x4000, 0x5000, 0xFF00));
    assert!(!cpu.get_cpu_flag(CpuFlags::IF));
}

/// BloodNet's event handler moves its own pointer by SI and DI, then
/// clears the motion counters with AX=000Bh. Each call has to get the
/// motion since that read.
#[test]
fn handler_motion_counts_from_the_last_read_of_the_counters() {
    let mut cpu = polling_cpu();
    cpu.bus.mouse.callback_mask = 0x0001;
    for round in 1..=3 {
        cpu.bus.mouse.move_by(3.0, 3.0);
        assert!(rust_dos::mouse::deliver_callback(&mut cpu));
        finish_callback(&mut cpu, 0x0100);
        assert_eq!(cpu.bus.read_16(CALLBACK_COUNT), round);
        let mickeys = (cpu.bus.read_16(CALLBACK_ARGS + 8), cpu.bus.read_16(CALLBACK_ARGS + 10));
        assert_eq!(mickeys, (2, 4), "SI and DI at call {round}");
    }
}

/// A handler that leaves the counters alone gets them as they add up over
/// its calls, until the program reads them with AX=000Bh, as the
/// Microsoft driver, CuteMouse and DOSBox pass them.
#[test]
fn handler_motion_without_reads_is_the_running_counters() {
    const QUIET_IP: u16 = 0x0300;
    let mut cpu = polling_cpu();
    let mut a = CodeAssembler::new(16).unwrap();
    a.inc(word_ptr(0x400).cs()).unwrap();
    a.mov(word_ptr(0x40A).cs(), si).unwrap();
    a.mov(word_ptr(0x40C).cs(), di).unwrap();
    a.retf().unwrap();
    for (i, byte) in a.assemble(QUIET_IP as u64).unwrap().into_iter().enumerate() {
        cpu.bus.write_8(((POLL_SEG as usize) << 4) + QUIET_IP as usize + i, byte);
    }
    cpu.bus.mouse.callback_ip = QUIET_IP;
    cpu.bus.mouse.callback_mask = 0x0001;
    for (round, expected) in [(1, (2, 4)), (2, (4, 8)), (3, (2, 4))] {
        cpu.bus.mouse.move_by(3.0, 3.0);
        if round == 3 {
            cpu.bus.mouse.move_by(3.0, 3.0);
            call(&mut cpu, 0x000B);
            assert_eq!((cpu.cx(), cpu.dx()), (8, 16), "four moves, the handler's calls cleared nothing");
            cpu.bus.mouse.move_by(3.0, 3.0);
        }
        assert!(rust_dos::mouse::deliver_callback(&mut cpu));
        finish_callback(&mut cpu, 0x0100);
        assert_eq!(cpu.bus.read_16(CALLBACK_COUNT), round);
        let mickeys = (cpu.bus.read_16(CALLBACK_ARGS + 8), cpu.bus.read_16(CALLBACK_ARGS + 10));
        assert_eq!(mickeys, expected, "SI and DI at call {round}");
    }
}

#[test]
fn reflected_button_polls_preserve_service_results() {
    for function in [0x0003, 0x0005, 0x0006] {
        let mut cpu = polling_cpu();
        cpu.bus.mouse.warp(100, 50);
        cpu.bus.mouse.button_down(0);
        if function == 0x0006 {
            cpu.bus.mouse.button_up(0);
        }
        cpu.set_bx(0);
        execute_poll(&mut cpu, function);
        assert!(rust_dos::mouse::callback_busy(&cpu.bus));
        finish_callback(&mut cpu, 0x0102);
        let expected_ax = if function == 3 { 3 } else if function == 5 { 1 } else { 0 };
        assert_eq!((cpu.ax(), cpu.bx(), cpu.cx(), cpu.dx()), (expected_ax, 1, 100, 50));
        assert_eq!(cpu.bus.read_16(CALLBACK_COUNT), 1);
        assert!(!cpu.get_cpu_flag(CpuFlags::IF));
    }
}

#[test]
fn reflected_poll_callback_does_not_reenter_and_leaves_next_event_pending() {
    let mut cpu = polling_cpu();
    cpu.bus.mouse.button_down(0);
    execute_poll(&mut cpu, 0x000B);
    cpu.bus.mouse.button_up(0); // Arrives before callback's nested INT 33h.
    finish_callback(&mut cpu, 0x0102);
    assert_eq!(cpu.bus.read_16(CALLBACK_COUNT), 1);
    assert_eq!(cpu.bus.mouse.pending_callback_events, 4);
    execute_poll(&mut cpu, 0x000B);
    finish_callback(&mut cpu, 0x0104);
    assert_eq!(cpu.bus.read_16(CALLBACK_COUNT), 2);
    assert_eq!(cpu.bus.read_16(CALLBACK_ARGS), 4, "left release callback");
    assert_eq!(cpu.bus.mouse.pending_callback_events, 0);
}

#[test]
fn non_poll_mouse_service_does_not_dispatch_with_if_clear() {
    let mut cpu = polling_cpu();
    cpu.bus.mouse.button_down(0);
    execute_poll(&mut cpu, 0x0020);
    assert_eq!((cpu.cs(), cpu.ip(), cpu.ax()), (POLL_SEG, 0x0102, 0xFFFF));
    assert_eq!(cpu.bus.mouse.pending_callback_events, 2);
    assert!(!rust_dos::mouse::callback_busy(&cpu.bus));
}

#[test]
fn reflected_poll_without_matching_handler_leaves_event_pending() {
    for mask in [0, 1] {
        let mut cpu = polling_cpu();
        cpu.bus.mouse.callback_mask = mask;
        cpu.bus.mouse.button_down(0);
        execute_poll(&mut cpu, 0x000B);
        assert_eq!((cpu.cs(), cpu.ip(), cpu.sp()), (POLL_SEG, 0x0102, 0xFF00));
        assert_eq!(cpu.bus.mouse.pending_callback_events, 2);
        assert!(!rust_dos::mouse::callback_busy(&cpu.bus));
    }
}

/// INT 10h with AX and BX, as a program sets a video mode.
fn int10(cpu: &mut Cpu, function: u16, mode: u16) {
    cpu.set_ax(function);
    cpu.set_bx(mode);
    rust_dos::interrupts::int10::handle(cpu);
}

fn range(cpu: &mut Cpu, function: u16, min: u16, max: u16) {
    cpu.set_cx(min);
    cpu.set_dx(max);
    call(cpu, function);
}

#[test]
fn a_video_mode_set_gives_the_cursor_the_new_screens_range() {
    // Conqueror A.D. 1086: its menus in VESA mode 101h set 0-479, its
    // battles in mode 13h read 0-199 and set no range of their own.
    let mut cpu = Cpu::new(PathBuf::from("."));
    int10(&mut cpu, 0x4F02, 0x0101);
    assert_eq!(cpu.ax(), 0x004F);
    call(&mut cpu, 0x0000);
    range(&mut cpu, 0x0007, 0, 639);
    range(&mut cpu, 0x0008, 0, 479);
    cpu.set_cx(320);
    cpu.set_dx(240);
    call(&mut cpu, 0x0004);
    call(&mut cpu, 0x0001);
    assert_eq!(cpu.bus.mouse.hide_counter, 0);

    int10(&mut cpu, 0x0013, 0);
    let m = &cpu.bus.mouse;
    assert_eq!(m.hide_counter, 1, "a mode set hides the cursor, as CuteMouse and DOSBox do");
    assert_eq!((m.min_x, m.max_x, m.min_y, m.max_y), (0, 639, 0, 199));
    assert_eq!((m.x, m.y), (320, 199), "the cursor stays, inside the new range");
    assert_eq!(m.virtual_screen(&cpu.bus), (640, 200), "the picture's bottom row is y 199");
    call(&mut cpu, 0x0003);
    assert_eq!((cpu.cx(), cpu.dx()), (320, 199));

    // Back in the menus, through VBE function 02h.
    int10(&mut cpu, 0x4F02, 0x0101);
    let m = &cpu.bus.mouse;
    assert_eq!((m.min_x, m.max_x, m.min_y, m.max_y), (0, 639, 0, 479));
    assert_eq!(m.virtual_screen(&cpu.bus), (640, 480));
}

#[test]
fn setting_the_same_mode_again_keeps_the_programs_range() {
    // Dark Sun 2: x 0-319 and y 0-199 in mode 13h, then mode 13h again,
    // with its own cursor drawn at the driver's x as a 320-pixel position.
    let mut cpu = Cpu::new(PathBuf::from("."));
    int10(&mut cpu, 0x0013, 0);
    call(&mut cpu, 0x0000);
    range(&mut cpu, 0x0007, 0, 0x013F);
    range(&mut cpu, 0x0008, 0, 0x00C7);
    cpu.set_cx(0x9F);
    cpu.set_dx(0x63);
    call(&mut cpu, 0x0004);
    call(&mut cpu, 0x0001);

    int10(&mut cpu, 0x0013, 0);
    let m = &cpu.bus.mouse;
    assert_eq!((m.min_x, m.max_x, m.min_y, m.max_y), (0, 319, 0, 199));
    assert_eq!((m.x, m.y), (159, 99));
    assert_eq!(m.hide_counter, 1, "the mode set still hides the cursor");
    cpu.bus.mouse.set_position(10_000, 100);
    call(&mut cpu, 0x0003);
    assert_eq!(cpu.cx(), 319, "the far right is the 320-pixel screen's");

    // A text font load that leaves 25 rows is the same screen too.
    int10(&mut cpu, 0x0003, 0);
    assert_eq!(cpu.bus.mouse.max_x, 639, "another mode resets the range");
    range(&mut cpu, 0x0007, 0, 319);
    int10(&mut cpu, 0x1114, 0);
    assert_eq!(cpu.bus.mouse.max_x, 319);
}

#[test]
fn the_no_clear_bit_and_vbe_flags_leave_the_mode_the_same() {
    // AL bit 7 keeps video memory; it sets the same mode.
    let mut cpu = Cpu::new(PathBuf::from("."));
    int10(&mut cpu, 0x0013, 0);
    call(&mut cpu, 0x0000);
    range(&mut cpu, 0x0007, 0, 319);
    int10(&mut cpu, 0x0093, 0);
    assert_eq!(cpu.bus.mouse.max_x, 319);

    // VBE: bit 15 (keep memory) and bit 14 (linear frame buffer) don't
    // change the mode either. Another VBE mode keeps the range the
    // program set (y) and gives the other (x) the new screen.
    call(&mut cpu, 0x0000);
    int10(&mut cpu, 0x4F02, 0x0101);
    range(&mut cpu, 0x0008, 0, 399);
    int10(&mut cpu, 0x4F02, 0x8101);
    assert_eq!(cpu.ax(), 0x004F);
    assert_eq!(cpu.bus.mouse.max_y, 399);
    int10(&mut cpu, 0x4F02, 0x4101);
    assert_eq!(cpu.ax(), 0x004F);
    assert_eq!(cpu.bus.mouse.max_y, 399);
    int10(&mut cpu, 0x4F02, 0x0103);
    assert_eq!(cpu.ax(), 0x004F);
    let m = &cpu.bus.mouse;
    assert_eq!((m.min_x, m.max_x, m.min_y, m.max_y), (0, 799, 0, 399));
}

#[test]
fn a_vesa_mode_change_keeps_the_range_the_program_set_before_it() {
    // Down by the Laituri: in text mode 3 it resets the driver, sets
    // x 0-1280 and y 0-960, then VESA mode 101h and no range after it,
    // and draws its cursor at half the driver's position.
    let mut cpu = Cpu::new(PathBuf::from("."));
    int10(&mut cpu, 0x0003, 0);
    call(&mut cpu, 0x0000);
    range(&mut cpu, 0x0007, 0, 1280);
    range(&mut cpu, 0x0008, 0, 960);
    int10(&mut cpu, 0x4F02, 0x0101);
    assert_eq!(cpu.ax(), 0x004F);
    let m = &cpu.bus.mouse;
    assert_eq!((m.min_x, m.max_x, m.min_y, m.max_y), (0, 1280, 0, 960));
    assert_eq!(m.virtual_screen(&cpu.bus), (1281, 961), "the host pointer spans the whole range");
    cpu.bus.mouse.set_position(10_000, 10_000);
    call(&mut cpu, 0x0003);
    assert_eq!((cpu.cx(), cpu.dx()), (1280, 960), "the bottom-right corner of its 640x480 picture");

    // That mode set used the program's range up: the next VESA mode
    // change, with no range set in between, takes the new mode's size.
    int10(&mut cpu, 0x4F02, 0x0103);
    let m = &cpu.bus.mouse;
    assert_eq!((m.min_x, m.max_x, m.min_y, m.max_y), (0, 799, 0, 599));
}

#[test]
fn a_vesa_mode_change_with_no_range_set_gives_the_modes_size() {
    // The reset's range is the old screen's (text mode: 640x200), which
    // the program never asked for.
    let mut cpu = Cpu::new(PathBuf::from("."));
    int10(&mut cpu, 0x0003, 0);
    call(&mut cpu, 0x0000);
    int10(&mut cpu, 0x4F02, 0x0101);
    let m = &cpu.bus.mouse;
    assert_eq!((m.min_x, m.max_x, m.min_y, m.max_y), (0, 639, 0, 479));

    // A range set before a standard mode change goes with it, as
    // Conqueror A.D. 1086's battles need.
    range(&mut cpu, 0x0008, 0, 479);
    int10(&mut cpu, 0x0013, 0);
    assert_eq!(cpu.bus.mouse.max_y, 199);
}

#[test]
fn a_range_set_after_the_mode_stays() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    call(&mut cpu, 0x0000);
    int10(&mut cpu, 0x0013, 0);
    range(&mut cpu, 0x0008, 0, 399);
    let m = &cpu.bus.mouse;
    assert_eq!((m.min_y, m.max_y), (0, 399));
    assert_eq!(m.virtual_screen(&cpu.bus), (640, 400), "a program that halves the coordinates");
    // A VBE call other than a mode set leaves it.
    int10(&mut cpu, 0x4F03, 0);
    assert_eq!(cpu.bus.mouse.max_y, 399);
}

#[test]
fn a_font_load_that_changes_the_rows_gives_the_new_range() {
    // 80x25, then the 8x8 font for 80x50: CuteMouse takes AH=11h AL=10h-1Fh
    // for a mode set.
    let mut cpu = Cpu::new(PathBuf::from("."));
    int10(&mut cpu, 0x0003, 0);
    call(&mut cpu, 0x0000);
    assert_eq!(cpu.bus.mouse.max_y, 199);
    int10(&mut cpu, 0x1112, 0);
    let m = &cpu.bus.mouse;
    assert_eq!((m.min_x, m.max_x, m.min_y, m.max_y), (0, 639, 0, 399));
}
