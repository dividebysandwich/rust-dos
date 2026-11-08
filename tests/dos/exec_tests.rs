use rust_dos::cpu::{Cpu, CpuFlags, CpuState};
use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;

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

/// Run from CS:IP until the next HLT reached in a state `stop` accepts.
fn run_to_hlt_where(cpu: &mut Cpu, stop: impl Fn(&Cpu) -> bool) {
    for _ in 0..1000 {
        let at = cpu.get_physical_addr(cpu.cs(), cpu.ip());
        if cpu.bus.read_8(at) == 0xF4 && stop(cpu) {
            return;
        }
        cpu.step();
    }
    panic!("no HLT reached");
}

/// Point the EXEC parameter block at PSP:0200 of the program `psp` at
/// an empty command tail and at `CHILD.COM`, for a parent that calls
/// AX=4B00h with BX=0200h and DX=0300h.
fn exec_child_com(cpu: &mut Cpu, psp: u16) {
    let base = psp as usize * 16;
    cpu.bus.write_16(base + 0x202, 0x0280);
    cpu.bus.write_16(base + 0x204, psp);
    cpu.bus.load_bytes(base + 0x280, &[0x00, 0x0D]);
    cpu.bus.load_bytes(base + 0x300, b"CHILD.COM\0");
}

#[test]
fn a_com_program_the_shell_starts_owns_all_of_conventional_memory() {
    // Titus's CD loader (CDRUN.COM) unpacks the game above its first
    // 64 KB, then sets its block's size with AH=4Ah.
    #[rustfmt::skip]
    let code = [
        0x8C, 0xC8,             // MOV AX,CS
        0x05, 0x00, 0x10,       // ADD AX,1000h
        0x8E, 0xC0,             // MOV ES,AX
        0x31, 0xFF,             // XOR DI,DI
        0xB8, 0x55, 0x55,       // MOV AX,5555h
        0xB9, 0x08, 0x00,       // MOV CX,8
        0xF3, 0xAB,             // REP STOSW: over CS+1000h:0000
        0x0E, 0x07,             // PUSH CS; POP ES
        0xB4, 0x4A,             // MOV AH,4Ah: set the block to 2000h
        0xBB, 0x00, 0x20,       // MOV BX,2000h
        0xCD, 0x21,             // INT 21h
        0xF4,                   // HLT
    ];
    let dir = scratch("com_all_memory", &[("UNPACK.COM", &code)]);
    let mut cpu = Cpu::new(dir);
    assert!(cpu.load_executable("UNPACK.COM", None));
    let psp = cpu.current_psp;
    let end = rust_dos::mcb::low_end(&cpu.bus);
    let block = rust_dos::mcb::read_mcb(&mut cpu.bus, psp - 1);
    assert_eq!((block.signature, block.owner, block.size), (rust_dos::mcb::MCB_Z, psp, end - psp));
    assert_eq!(cpu.bus.read_16(psp as usize * 16 + 2), end, "the PSP's top of memory");

    run_to_hlt(&mut cpu);
    assert!(!cpu.get_cpu_flag(CpuFlags::CF), "AH=4Ah failed, AX={:04X}", cpu.ax());
    let block = rust_dos::mcb::read_mcb(&mut cpu.bus, psp - 1);
    assert_eq!(block.size, 0x2000);
    let free = rust_dos::mcb::read_mcb(&mut cpu.bus, psp + 0x2000);
    assert!(free.is_valid() && free.is_free() && free.is_last(), "{free:?}");
    assert_eq!(free.size, end - psp - 0x2001);
}

#[test]
fn exec_leaves_a_com_program_the_whole_block_it_allocated() {
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
    let dir = scratch("exec_com_block", &[("PARENT.COM", &parent), ("CHILD.COM", &[0xF4])]);
    let mut cpu = Cpu::new(dir);
    assert!(cpu.load_executable("PARENT.COM", None));
    let psp = cpu.current_psp;
    exec_child_com(&mut cpu, psp);

    run_to_hlt_where(&mut cpu, |cpu| cpu.current_psp != psp);
    let child = cpu.current_psp;
    // The free memory after the parent's block, all of it.
    let end = rust_dos::mcb::low_end(&cpu.bus);
    let block = rust_dos::mcb::read_mcb(&mut cpu.bus, child - 1);
    assert_eq!((block.owner, block.size), (child, end - child));
    assert!(block.is_last());
    assert_eq!(cpu.bus.read_16(child as usize * 16 + 2), end, "the PSP's top of memory");
}

#[test]
fn a_com_child_in_a_block_under_64_kb_stays_in_it() {
    // MOV AX,4B00h; MOV BX,0200h; MOV DX,0300h; INT 21h; HLT
    let parent = [0xB8, 0x00, 0x4B, 0xBB, 0x00, 0x02, 0xBA, 0x00, 0x03, 0xCD, 0x21, 0xF4];
    let dir = scratch("exec_com_small_block", &[("PARENT.COM", &parent), ("CHILD.COM", &[0xF4])]);
    let mut cpu = Cpu::new(dir);
    assert!(cpu.load_executable("PARENT.COM", None));
    let psp = cpu.current_psp;
    exec_child_com(&mut cpu, psp);
    // The largest free block is a hole of 800h paragraphs between the
    // parent's block and one it owns above.
    use rust_dos::mcb;
    mcb::resize(&mut cpu.bus, psp, 0x1000).unwrap();
    let hole = mcb::alloc(&mut cpu.bus, psp, 0x800).unwrap();
    let rest = mcb::alloc(&mut cpu.bus, psp, 0xFFFF).unwrap_err();
    let above = mcb::alloc(&mut cpu.bus, psp, rest).unwrap();
    mcb::free(&mut cpu.bus, hole).unwrap();

    run_to_hlt_where(&mut cpu, |cpu| cpu.current_psp != psp);
    let child = cpu.current_psp;
    let block = mcb::read_mcb(&mut cpu.bus, child - 1);
    assert_eq!(child + block.size, above - 1, "the child has the rest of the hole");
    assert_eq!(cpu.bus.read_16(child as usize * 16 + 2), above - 1, "the PSP's top of memory");
    assert_eq!((cpu.ss(), cpu.sp()), (child, block.size * 16 - 2), "the stack, in the block");
    let next = mcb::read_mcb(&mut cpu.bus, above - 1);
    assert!(next.is_valid() && next.owner == psp && next.size == rest, "{next:?}");
}

/// The `Restore Failed` lines `cpu` logs from now on.
fn restore_failures(cpu: &mut Cpu) -> Rc<RefCell<Vec<String>>> {
    let failures = Rc::new(RefCell::new(Vec::new()));
    let log = failures.clone();
    cpu.bus.log_hook = Some(Box::new(move |line: &str| {
        if line.contains("Restore Failed") {
            log.borrow_mut().push(line.to_string());
        }
    }));
    failures
}

/// Run until the shell is to be loaded again, in at most `steps` steps.
fn run_to_shell(cpu: &mut Cpu, steps: usize) {
    for _ in 0..steps {
        if cpu.state == CpuState::RebootShell {
            return;
        }
        cpu.step();
    }
    assert_eq!(cpu.state, CpuState::RebootShell);
}

#[test]
fn a_program_the_shell_started_ends_without_a_restore_failure() {
    // INT 20h; MOV AX,4C00h; INT 21h; and MOV AX,3100h; MOV DX,0010h; INT 21h.
    let dir = scratch(
        "shell_exit",
        &[
            ("INT20.COM", &[0xCD, 0x20]),
            ("EXIT.COM", &[0xB8, 0x00, 0x4C, 0xCD, 0x21]),
            ("TSR.COM", &[0xB8, 0x00, 0x31, 0xBA, 0x10, 0x00, 0xCD, 0x21]),
        ],
    );
    for program in ["INT20.COM", "EXIT.COM", "TSR.COM"] {
        let mut cpu = Cpu::new(dir.clone());
        let failures = restore_failures(&mut cpu);
        assert!(cpu.load_executable(program, None));
        run_to_shell(&mut cpu, 10);
        assert!(failures.borrow().is_empty(), "{program}: {:?}", failures.borrow());
    }
}

#[test]
fn a_debugger_the_shell_started_quits_without_running_its_child_and_without_a_restore_failure() {
    #[rustfmt::skip]
    let parent = [
        0xB4, 0x4A,             // MOV AH,4Ah: shrink to 1000h paragraphs
        0xBB, 0x00, 0x10,       // MOV BX,1000h
        0xCD, 0x21,             // INT 21h
        0xB8, 0x01, 0x4B,       // MOV AX,4B01h: load the child, don't run it
        0xBB, 0x00, 0x02,       // MOV BX,0200h
        0xBA, 0x00, 0x03,       // MOV DX,0300h
        0xCD, 0x21,             // INT 21h
        0x0E, 0x5B,             // PUSH CS; POP BX
        0xB4, 0x50,             // MOV AH,50h: its own PSP current again
        0xCD, 0x21,             // INT 21h
        0xB8, 0x00, 0x4C,       // MOV AX,4C00h
        0xCD, 0x21,             // INT 21h
    ];
    let child = [0xB8, 0x07, 0x4C, 0xCD, 0x21]; // MOV AX,4C07h; INT 21h
    let dir = scratch("exec_load_quit", &[("PARENT.COM", &parent), ("CHILD.COM", &child)]);
    let mut cpu = Cpu::new(dir);
    let failures = restore_failures(&mut cpu);
    assert!(cpu.load_executable("PARENT.COM", None));
    let psp = cpu.current_psp;
    let base = psp as usize * 16;
    cpu.bus.write_16(base + 0x202, 0x0280); // command tail
    cpu.bus.write_16(base + 0x204, psp);
    for (field, fcb) in [(0x206, 0x2A0), (0x20A, 0x2C0)] {
        cpu.bus.write_16(base + field, fcb);
        cpu.bus.write_16(base + field + 2, psp);
    }
    cpu.bus.load_bytes(base + 0x280, &[0x00, 0x0D]);
    cpu.bus.load_bytes(base + 0x300, b"CHILD.COM\0");
    run_to_shell(&mut cpu, 100);
    assert!(failures.borrow().is_empty(), "{:?}", failures.borrow());
}

/// Run from CS:IP until the next HLT.
fn run_to_hlt(cpu: &mut Cpu) {
    run_to_hlt_where(cpu, |_| true);
}

#[test]
fn exec_loads_a_program_for_a_debugger_and_ends_it_at_its_terminate_address() {
    let parent = [
        0xB4, 0x4A, // MOV AH,4Ah: shrink to 1000h paragraphs
        0xBB, 0x00, 0x10, // MOV BX,1000h
        0xCD, 0x21, // INT 21h
        0xB8, 0x01, 0x4B, // MOV AX,4B01h
        0xBB, 0x00, 0x02, // MOV BX,0200h
        0xBA, 0x00, 0x03, // MOV DX,0300h
        0xCD, 0x21, // INT 21h
        0xF4, // 0112: HLT, after loading
        0xF4, // 0113: HLT, the debugger's terminate handler
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
    assert_eq!((cpu.cs(), cpu.ip(), cpu.sp()), (psp, 0x0112, parent_sp));
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
    cpu.bus.write_16(child_base + 0x0A, 0x0113);
    cpu.bus.write_16(child_base + 0x0C, psp);
    cpu.set_cs(cs);
    cpu.set_ip(ip);
    cpu.set_ss(ss);
    cpu.set_sp(sp + 2);
    run_to_hlt(&mut cpu);
    assert_eq!((cpu.cs(), cpu.ip(), cpu.sp()), (psp, 0x0113, parent_sp));
    assert_eq!(cpu.current_psp, psp);
    assert_eq!(cpu.last_child_exit, 7);
}

#[test]
fn exec_points_int_22h_at_the_return_address_and_the_end_restores_the_vectors() {
    // Bureau 13's SETUP, run through EXEC by B13.COM, reckons the memory
    // the game has from the segment of the INT 22h vector. DOS sets it to
    // the parent's return address and keeps INT 22h, 23h and 24h in the
    // child's PSP (0Ah, 0Eh, 12h), and sets them from there when the child
    // ends, so the child's Ctrl-Break handler goes with it.
    #[rustfmt::skip]
    let parent = [
        0xB4, 0x4A,             // MOV AH,4Ah: shrink to 1000h paragraphs
        0xBB, 0x00, 0x10,       // MOV BX,1000h
        0xCD, 0x21,             // INT 21h
        0xB8, 0x00, 0x4B,       // MOV AX,4B00h
        0xBB, 0x00, 0x02,       // MOV BX,0200h
        0xBA, 0x00, 0x03,       // MOV DX,0300h
        0xCD, 0x21,             // INT 21h
        0xF4,                   // 0112: HLT
    ];
    #[rustfmt::skip]
    let child = [
        0xF4,                                       // HLT
        0x31, 0xC0,                                 // XOR AX,AX
        0x8E, 0xC0,                                 // MOV ES,AX
        0x26, 0xC7, 0x06, 0x8C, 0x00, 0x34, 0x12,   // MOV WORD ES:[008Ch],1234h
        0xB8, 0x00, 0x4C,                           // MOV AX,4C00h
        0xCD, 0x21,                                 // INT 21h
    ];
    let dir = scratch("exec_int22", &[("PARENT.COM", &parent), ("CHILD.COM", &child)]);
    let mut cpu = Cpu::new(dir);
    assert!(cpu.load_executable("PARENT.COM", None));
    let psp = cpu.current_psp;
    exec_child_com(&mut cpu, psp);
    let vectors = |cpu: &Cpu| [0x22, 0x23, 0x24].map(|v| cpu.bus.read_32(v * 4));
    let ctrl_break = vectors(&cpu)[1];

    run_to_hlt_where(&mut cpu, |cpu| cpu.current_psp != psp);
    let child_psp = cpu.current_psp;
    let return_address = (psp as u32) << 16 | 0x0112;
    assert_eq!(vectors(&cpu)[0], return_address, "INT 22h is the parent's return address");
    let kept = [0x0A, 0x0E, 0x12].map(|off| cpu.bus.read_32(child_psp as usize * 16 + off));
    assert_eq!(kept, vectors(&cpu), "the child's PSP keeps the three vectors");

    cpu.set_ip(cpu.ip() + 1);
    run_to_hlt_where(&mut cpu, |cpu| cpu.current_psp == psp);
    assert_eq!((cpu.cs(), cpu.ip()), (psp, 0x0112));
    assert_eq!(vectors(&cpu)[1], ctrl_break, "the child's INT 23h handler went with it");
    assert_eq!(vectors(&cpu)[0], return_address);
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
    let owned: Vec<u16> = rust_dos::mcb::walk(&mut cpu.bus).into_iter().filter(|(_, m)| m.owner == child_psp).map(|(seg, _)| seg).collect();
    assert!(owned.is_empty(), "the child's blocks are free: {:X?}", owned);
}

/// An EXE file whose load module is `code`, with the header's MINALLOC
/// and MAXALLOC.
fn exe(code: &[u8], min_alloc: u16, max_alloc: u16) -> Vec<u8> {
    let mut exe = vec![0u8; 0x20];
    exe.extend_from_slice(code);
    let len = exe.len() as u16;
    exe[0..2].copy_from_slice(b"MZ");
    exe[2..4].copy_from_slice(&(len % 512).to_le_bytes());
    exe[4..6].copy_from_slice(&len.div_ceil(512).to_le_bytes());
    exe[8..10].copy_from_slice(&2u16.to_le_bytes()); // header paragraphs
    exe[10..12].copy_from_slice(&min_alloc.to_le_bytes());
    exe[12..14].copy_from_slice(&max_alloc.to_le_bytes());
    exe[14..16].copy_from_slice(&0u16.to_le_bytes()); // SS
    exe[16..18].copy_from_slice(&0x0100u16.to_le_bytes()); // SP
    exe
}

#[test]
fn exec_gives_an_exe_file_no_more_than_its_maxalloc_paragraphs() {
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
    // Millennia's ECGAME.EXE: MINALLOC 0, MAXALLOC 1, and an EXEC of its
    // own without shrinking its block first. Here it allocates instead.
    #[rustfmt::skip]
    let child = [
        0xB4, 0x48,             // MOV AH,48h: allocate 100h paragraphs
        0xBB, 0x00, 0x01,       // MOV BX,0100h
        0xCD, 0x21,             // INT 21h
        0xF4,                   // HLT
    ];
    let dir = scratch("exec_maxalloc", &[("PARENT.COM", &parent), ("CHILD.EXE", &exe(&child, 0, 1))]);
    let mut cpu = Cpu::new(dir);
    assert!(cpu.load_executable("PARENT.COM", None));
    let psp = cpu.current_psp;
    let base = psp as usize * 16;
    cpu.bus.write_16(base + 0x202, 0x0280);
    cpu.bus.write_16(base + 0x204, psp);
    cpu.bus.load_bytes(base + 0x280, &[0x00, 0x0D]);
    cpu.bus.load_bytes(base + 0x300, b"CHILD.EXE\0");

    for _ in 0..1000 {
        let at = cpu.get_physical_addr(cpu.cs(), cpu.ip());
        if cpu.bus.read_8(at) == 0xF4 && cpu.current_psp != psp {
            break;
        }
        cpu.step();
    }
    let child_psp = cpu.current_psp;
    assert_ne!(child_psp, psp, "the child ran to its HLT");
    assert!(!cpu.get_cpu_flag(CpuFlags::CF), "the child's allocation failed, AX={:04X}", cpu.ax());
    // Its PSP, its one paragraph of code, and MAXALLOC's one.
    let block = rust_dos::mcb::read_mcb(&mut cpu.bus, child_psp - 1);
    assert_eq!(block.size, 0x12);
    // Its PSP's top of memory is the end of that block.
    let top = cpu.bus.read_16(child_psp as usize * 16 + 2);
    assert_eq!(top, child_psp + 0x12);
}

#[test]
fn exec_of_an_exe_file_too_big_for_memory_fails_with_insufficient_memory() {
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
    // MINALLOC F000h paragraphs: more than all of conventional memory.
    let child = exe(&[0xF4], 0xF000, 0xF000);
    let dir = scratch("exec_too_big", &[("PARENT.COM", &parent), ("CHILD.EXE", &child)]);
    let mut cpu = Cpu::new(dir);
    assert!(cpu.load_executable("PARENT.COM", None));
    let psp = cpu.current_psp;
    let base = psp as usize * 16;
    cpu.bus.write_16(base + 0x202, 0x0280);
    cpu.bus.write_16(base + 0x204, psp);
    cpu.bus.load_bytes(base + 0x280, &[0x00, 0x0D]);
    cpu.bus.load_bytes(base + 0x300, b"CHILD.EXE\0");

    for _ in 0..1000 {
        let at = cpu.get_physical_addr(cpu.cs(), cpu.ip());
        if cpu.bus.read_8(at) == 0xF4 {
            break;
        }
        cpu.step();
    }
    assert_eq!(cpu.current_psp, psp, "the parent is back at its HLT");
    assert!(cpu.get_cpu_flag(CpuFlags::CF));
    assert_eq!(cpu.ax(), 0x0008);
}

/// Run PARENT.COM, which EXECs PLAYER.EXE and halts, in a directory with
/// `files` too, and give the exit code it got. `off` is `game_fixes_off`.
fn exec_player(name: &str, files: &[(&str, &[u8])], off: &[&str]) -> u16 {
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
    let mut all: Vec<(&str, &[u8])> = vec![("PARENT.COM", &parent)];
    all.extend_from_slice(files);
    let mut cpu = Cpu::new(scratch(name, &all));
    cpu.bus.game_fixes_off = off.iter().map(|name| name.to_string()).collect();
    assert!(cpu.load_executable("PARENT.COM", None));
    let psp = cpu.current_psp;
    let base = psp as usize * 16;
    cpu.bus.write_16(base + 0x202, 0x0280);
    cpu.bus.write_16(base + 0x204, psp);
    cpu.bus.load_bytes(base + 0x280, &[0x00, 0x0D]);
    cpu.bus.load_bytes(base + 0x300, b"PLAYER.EXE\0");
    for _ in 0..1000 {
        let at = cpu.get_physical_addr(cpu.cs(), cpu.ip());
        if cpu.bus.read_8(at) == 0xF4 && cpu.current_psp == psp {
            break;
        }
        cpu.step();
    }
    assert_eq!(cpu.current_psp, psp, "back in the parent");
    cpu.last_child_exit
}

#[test]
fn exec_runs_a_player_exe_that_isnt_microproses() {
    // MOV AX,4C05h; INT 21h
    let player = [0xB8, 0x05, 0x4C, 0xCD, 0x21];
    assert_eq!(exec_player("player_runs", &[("PLAYER.EXE", &player)], &[]), 5);
}

#[test]
fn exec_skips_microproses_player_exe() {
    let player = [0xB8, 0x05, 0x4C, 0xCD, 0x21];
    let dswap = [0xB8, 0x00, 0x4C, 0xCD, 0x21];
    let files: [(&str, &[u8]); 2] = [("PLAYER.EXE", &player), ("DSWAP.EXE", &dswap)];
    assert_eq!(exec_player("player_skipped", &files, &[]), 0);
}

#[test]
fn exec_runs_microproses_player_exe_with_its_game_fix_off() {
    let player = [0xB8, 0x05, 0x4C, 0xCD, 0x21];
    let dswap = [0xB8, 0x00, 0x4C, 0xCD, 0x21];
    let files: [(&str, &[u8]); 2] = [("PLAYER.EXE", &player), ("DSWAP.EXE", &dswap)];
    let off = ["microprose-player"];
    assert_eq!(exec_player("player_fix_off", &files, &off), 5);
}
