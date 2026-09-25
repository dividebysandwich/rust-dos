use rust_dos::cpu::{Cpu, CpuFlags, CpuState};
use std::fs;
use std::path::PathBuf;

/// Fresh directory with the given files under target/.
fn scratch(name: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let base = PathBuf::from("target/test_exec").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    for (file, bytes) in files {
        fs::write(base.join(file), bytes).unwrap();
    }
    base
}

/// A CPU at 0000:0100 with `code` there, an IRQ 0 handler at 0000:0600,
/// and interrupts enabled or not.
fn cpu_with_code(code: &[u8], interrupts: bool) -> Cpu {
    let mut cpu = Cpu::new(PathBuf::from("."));
    cpu.bus.load_bytes(0x100, code);
    cpu.bus.write_16(0x08 * 4, 0x0600);
    cpu.bus.write_16(0x08 * 4 + 2, 0x0000);
    cpu.set_ss(0x0000);
    cpu.set_sp(0x2000);
    cpu.set_cpu_flag(CpuFlags::IF, interrupts);
    cpu
}

fn raise_irq0(cpu: &mut Cpu) {
    cpu.bus.pic.raise(0);
}

#[test]
fn sti_delays_interrupts_by_one_instruction() {
    // FB -> STI ; 90 -> NOP ; 90 -> NOP
    let mut cpu = cpu_with_code(&[0xFB, 0x90, 0x90], false);
    raise_irq0(&mut cpu);

    cpu.step(); // STI
    assert_eq!(cpu.ip(), 0x101);
    cpu.step(); // The pending IRQ waits for the instruction after STI.
    assert_eq!(cpu.ip(), 0x102);
    cpu.step(); // Now it is taken.
    assert_eq!((cpu.cs(), cpu.ip()), (0x0000, 0x0600));
}

#[test]
fn mov_ss_delays_interrupts_until_sp_is_loaded() {
    // 8E D0 -> MOV SS, AX ; 89 DC -> MOV SP, BX ; 90 -> NOP
    let mut cpu = cpu_with_code(&[0x8E, 0xD0, 0x89, 0xDC, 0x90], true);
    cpu.set_ax(0x0000);
    cpu.set_bx(0x3000);

    cpu.step(); // MOV SS, AX
    raise_irq0(&mut cpu);
    cpu.step(); // MOV SP, BX runs before the IRQ.
    assert_eq!((cpu.ip(), cpu.sp()), (0x104, 0x3000));
    cpu.step();
    assert_eq!((cpu.cs(), cpu.ip()), (0x0000, 0x0600));
    // The IRQ frame is on the new stack.
    assert_eq!(cpu.sp(), 0x3000 - 6);
}

#[test]
fn com_program_returning_to_psp_offset_0_terminates() {
    // C3 -> RET: pops the 0000h that DOS leaves at the top of a COM
    // program's stack and runs the INT 20h at PSP:0000.
    let dir = scratch("ret_to_psp", &[("RET.COM", &[0xC3])]);
    let mut cpu = Cpu::new(dir);
    assert!(cpu.load_executable("RET.COM", None));

    for _ in 0..10 {
        if cpu.state == CpuState::RebootShell {
            break;
        }
        cpu.step();
    }
    assert_eq!(cpu.state, CpuState::RebootShell);
}

/// Run from CS:IP until the next HLT.
fn run_to_hlt(cpu: &mut Cpu) {
    for _ in 0..1000 {
        let at = cpu.get_physical_addr(cpu.cs(), cpu.ip());
        if cpu.bus.read_8(at) == 0xF4 {
            return;
        }
        cpu.step();
    }
    panic!("no HLT reached");
}

#[test]
fn exec_loads_a_program_for_a_debugger_and_ends_it_at_its_terminate_address() {
    let parent = [
        0xB8, 0x01, 0x4B, // MOV AX,4B01h
        0xBB, 0x00, 0x02, // MOV BX,0200h
        0xBA, 0x00, 0x03, // MOV DX,0300h
        0xCD, 0x21, // INT 21h
        0xF4, // 010B: HLT, after loading
        0xF4, // 010C: HLT, the debugger's terminate handler
    ];
    let child = [0xB8, 0x07, 0x4C, 0xCD, 0x21]; // MOV AX,4C07h; INT 21h
    let dir = scratch("exec_load", &[("PARENT.COM", &parent), ("CHILD.COM", &child)]);
    let mut cpu = Cpu::new(dir);
    assert!(cpu.load_executable("PARENT.COM", None));
    let psp = cpu.current_psp;
    let base = psp as usize * 16;
    cpu.bus.write_16(base + 0x202, 0x0280); // command tail
    cpu.bus.write_16(base + 0x204, psp);
    // FCBs: the first on drive A:, the second on the default drive.
    for (field, fcb) in [(0x206, 0x2A0), (0x20A, 0x2C0)] {
        cpu.bus.write_16(base + field, fcb);
        cpu.bus.write_16(base + field + 2, psp);
    }
    cpu.bus.load_bytes(base + 0x2A0, b"\x01GAME    DAT");
    cpu.bus.load_bytes(base + 0x280, &[0x00, 0x0D]);
    cpu.bus.load_bytes(base + 0x300, b"CHILD.COM\0");
    let parent_sp = cpu.sp();

    run_to_hlt(&mut cpu);
    assert_eq!((cpu.cs(), cpu.ip(), cpu.sp()), (psp, 0x010B, parent_sp));
    assert!(!cpu.get_cpu_flag(CpuFlags::CF));
    let child_psp = cpu.current_psp;
    assert_ne!(child_psp, psp, "the child's PSP is current");
    let word = |cpu: &Cpu, off: usize| cpu.bus.read_16(base + 0x200 + off);
    let (sp, ss, ip, cs) = (word(&cpu, 0x0E), word(&cpu, 0x10), word(&cpu, 0x12), word(&cpu, 0x14));
    assert_eq!((cs, ip), (child_psp, 0x0100));
    // A: isn't there: AL is FFh.
    assert_eq!(cpu.bus.read_16(ss as usize * 16 + sp as usize), 0x00FF, "initial AX on the stack");
    let child_fcb: Vec<u8> = (0..12).map(|i| cpu.bus.read_8(child_psp as usize * 16 + 0x5C + i)).collect();
    assert_eq!(child_fcb, b"\x01GAME    DAT", "the FCBs are copied into the child's PSP");

    // The debugger takes over termination and runs the child.
    let child_base = child_psp as usize * 16;
    cpu.bus.write_16(child_base + 0x0A, 0x010C);
    cpu.bus.write_16(child_base + 0x0C, psp);
    cpu.set_cs(cs);
    cpu.set_ip(ip);
    cpu.set_ss(ss);
    cpu.set_sp(sp + 2);
    run_to_hlt(&mut cpu);
    assert_eq!((cpu.cs(), cpu.ip(), cpu.sp()), (psp, 0x010C, parent_sp));
    assert_eq!(cpu.current_psp, psp);
    assert_eq!(cpu.last_child_exit, 7);
}

#[test]
fn int_20h_frees_the_memory_of_a_child_program() {
    #[rustfmt::skip]
    let parent = [
        0xB4, 0x4A,             // MOV AH,4Ah: shrink to 1000h paragraphs
        0xBB, 0x00, 0x10,       // MOV BX,1000h
        0xCD, 0x21,             // INT 21h
        0xB8, 0x00, 0x4B,       // MOV AX,4B00h
        0xBB, 0x00, 0x02,       // MOV BX,0200h
        0xBA, 0x00, 0x03,       // MOV DX,0300h
        0xCD, 0x21,             // INT 21h
        0xF4,                   // HLT
    ];
    #[rustfmt::skip]
    let child = [
        0xB4, 0x4A,             // MOV AH,4Ah: shrink to 100h paragraphs
        0xBB, 0x00, 0x01,       // MOV BX,0100h
        0xCD, 0x21,             // INT 21h
        0xB4, 0x48,             // MOV AH,48h: allocate 10h paragraphs
        0xBB, 0x10, 0x00,       // MOV BX,0010h
        0xCD, 0x21,             // INT 21h
        0xCD, 0x20,             // INT 20h
    ];
    let dir = scratch("int20_frees", &[("PARENT.COM", &parent), ("CHILD.COM", &child)]);
    let mut cpu = Cpu::new(dir);
    assert!(cpu.load_executable("PARENT.COM", None));
    let psp = cpu.current_psp;
    let base = psp as usize * 16;
    cpu.bus.write_16(base + 0x202, 0x0280);
    cpu.bus.write_16(base + 0x204, psp);
    cpu.bus.load_bytes(base + 0x280, &[0x00, 0x0D]);
    cpu.bus.load_bytes(base + 0x300, b"CHILD.COM\0");

    let mut child_psp = None;
    for _ in 0..1000 {
        if cpu.current_psp != psp {
            child_psp = Some(cpu.current_psp);
        }
        let at = cpu.get_physical_addr(cpu.cs(), cpu.ip());
        if cpu.bus.read_8(at) == 0xF4 && cpu.current_psp == psp {
            break;
        }
        cpu.step();
    }
    let child_psp = child_psp.expect("the child ran");
    assert_eq!(cpu.current_psp, psp, "back in the parent");
    assert_eq!(cpu.last_child_exit, 0);
    let owned: Vec<u16> = rust_dos::mcb::walk(&cpu.bus).into_iter().filter(|(_, m)| m.owner == child_psp).map(|(seg, _)| seg).collect();
    assert!(owned.is_empty(), "the child's blocks are free: {:X?}", owned);
}
