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

/// Where the single-step traps of `traced` are counted, and the return
/// address of the last one kept.
const TRAPS: usize = 0x500;
const TRAP_IP: usize = 0x502;

/// A CPU at 0000:0100 with `code` there and an INT 1 handler at 0000:0700
/// that counts the traps and keeps the address it returns to.
fn traced(code: &[u8]) -> Cpu {
    let mut cpu = cpu_with_code(code, false);
    cpu.set_ds(0x0000);
    cpu.bus.load_bytes(0x700, &[
        0x55, // PUSH BP
        0x8B, 0xEC, // MOV BP,SP
        0x50, // PUSH AX
        0x8B, 0x46, 0x02, // MOV AX,[BP+2]
        0xA3, 0x02, 0x05, // MOV [0502h],AX
        0xFF, 0x06, 0x00, 0x05, // INC WORD [0500h]
        0x58, // POP AX
        0x5D, // POP BP
        0xCF, // IRET
    ]);
    // INT 1's vector.
    cpu.bus.write_16(0x04, 0x0700);
    cpu.bus.write_16(0x06, 0x0000);
    cpu
}

#[test]
fn the_single_step_trap_comes_after_the_instruction_after_the_popf_that_sets_tf() {
    // 9D -> POPF ; 90 -> NOP ; 90 -> NOP
    let mut cpu = traced(&[0x9D, 0x90, 0x90]);
    cpu.set_sp(0x1FFE);
    cpu.bus.write_16(0x1FFE, 0x0102); // TF

    cpu.step(); // POPF: TF was clear as it began.
    assert_eq!((cpu.cs(), cpu.ip()), (0x0000, 0x101));
    assert!(cpu.get_cpu_flag(CpuFlags::TF));
    cpu.step(); // NOP, then the trap.
    assert_eq!((cpu.cs(), cpu.ip()), (0x0000, 0x0700));
    assert!(!cpu.get_cpu_flag(CpuFlags::TF));
    assert_ne!(cpu.dr[6] & 0x4000, 0, "DR6.BS");
    // It returns to the next instruction, with TF.
    assert_eq!(cpu.bus.read_16(0x2000 - 6), 0x102);
    assert_ne!(cpu.bus.read_16(0x2000 - 2) & 0x0100, 0);
}

#[test]
fn a_traced_program_traps_after_every_instruction() {
    // 90 -> NOP (3x) ; F4 -> HLT
    let mut cpu = traced(&[0x90, 0x90, 0x90, 0xF4]);
    cpu.set_cpu_flag(CpuFlags::TF, true);
    run_to_hlt(&mut cpu);
    assert_eq!(cpu.bus.read_16(TRAPS), 3);
    assert_eq!(cpu.bus.read_16(TRAP_IP), 0x103);
    assert!(cpu.get_cpu_flag(CpuFlags::TF));
}

#[test]
fn a_software_interrupt_runs_its_handler_untraced_and_drops_its_trap() {
    // CD 60 -> INT 60h ; 90 -> NOP ; F4 -> HLT
    let mut cpu = traced(&[0xCD, 0x60, 0x90, 0xF4]);
    cpu.bus.load_bytes(0x800, &[0x90, 0x90, 0xCF]); // NOP ; NOP ; IRET
    cpu.bus.write_16(0x60 * 4, 0x0800);
    cpu.bus.write_16(0x60 * 4 + 2, 0x0000);
    cpu.set_cpu_flag(CpuFlags::TF, true);
    run_to_hlt(&mut cpu);
    // Only the NOP after the INT, once the handler's IRET brought TF back.
    assert_eq!(cpu.bus.read_16(TRAPS), 1);
    assert_eq!(cpu.bus.read_16(TRAP_IP), 0x103);
}

#[test]
fn a_traced_rep_string_instruction_traps_after_every_iteration() {
    // F3 A4 -> REP MOVSB ; F4 -> HLT
    let mut cpu = traced(&[0xF3, 0xA4, 0xF4]);
    cpu.bus.load_bytes(0x900, b"abc");
    cpu.set_es(0x0000);
    cpu.set_si(0x900);
    cpu.set_di(0xA00);
    cpu.set_cx(3);
    cpu.set_cpu_flag(CpuFlags::TF, true);

    cpu.step(); // One iteration, and the trap back to the instruction.
    assert_eq!((cpu.ip(), cpu.bus.read_16(0x2000 - 6)), (0x0700, 0x100));
    run_to_hlt(&mut cpu);
    assert_eq!(cpu.bus.read_16(TRAPS), 3);
    assert_eq!(cpu.bus.read_16(TRAP_IP), 0x102);
    assert_eq!(cpu.cx(), 0);
    let copied: Vec<u8> = (0..3).map(|i| cpu.bus.read_8(0xA00 + i)).collect();
    assert_eq!(copied, b"abc");
}

#[test]
fn mov_ss_delays_the_single_step_trap_until_sp_is_loaded() {
    // 8E D0 -> MOV SS,AX ; 89 DC -> MOV SP,BX ; F4 -> HLT
    let mut cpu = traced(&[0x8E, 0xD0, 0x89, 0xDC, 0xF4]);
    cpu.set_ax(0x0000);
    cpu.set_bx(0x3000);
    cpu.set_cpu_flag(CpuFlags::TF, true);
    run_to_hlt(&mut cpu);
    assert_eq!(cpu.bus.read_16(TRAPS), 1);
    assert_eq!(cpu.bus.read_16(TRAP_IP), 0x104);
    // The trap's frame was on the new stack.
    assert_eq!(cpu.bus.read_16(0x3000 - 6), 0x104);
}

#[test]
fn a_dos_service_hands_back_its_callers_carry_through_a_chained_hook() {
    // Second Reality's loader hooks INT 21h and jumps on to DOS after a
    // compare of AH that sets CF; AH=25h must still return the caller's.
    for (set_carry, carry) in [(0xF8, false), (0xF9, true)] {
        let mut cpu = cpu_with_code(
            &[
                set_carry, // CLC or STC
                0xB8, 0x60, 0x25, // MOV AX,2560h
                0xBA, 0x78, 0x56, // MOV DX,5678h
                0xCD, 0x21, // INT 21h
                0xF4, // HLT
            ],
            false,
        );
        cpu.set_ds(0x0000);
        let dos = cpu.bus.read_32(0x21 * 4);
        cpu.bus.load_bytes(0x800, &[0x80, 0xFC, 0x30, 0xEA]); // CMP AH,30h ; JMP FAR
        cpu.bus.write_32(0x804, dos);
        cpu.bus.write_32(0x21 * 4, 0x0000_0800);
        run_to_hlt(&mut cpu);
        assert_eq!(cpu.bus.read_32(0x60 * 4), 0x0000_5678);
        assert_eq!(cpu.get_cpu_flag(CpuFlags::CF), carry);
    }
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
