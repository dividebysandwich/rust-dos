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
