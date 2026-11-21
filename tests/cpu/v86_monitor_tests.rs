//! The built-in virtual-8086 monitor (v86mon.rs) and its VCPI (vcpi.rs):
//! with expanded memory on, DOS runs in virtual-8086 mode as under EMM386,
//! programs see CR0.PE set, interrupts and exceptions reach the real-mode
//! vector table, privileged instructions are emulated or stop the
//! program, and a VCPI client switches to protected mode and back.

use crate::pmrig::{asm16, asm32, seg_desc, sys_desc};
use iced_x86::code_asm::*;
use rust_dos::cpu::{Cpu, CpuState, Seg};
use std::path::PathBuf;

/// Where the tests' code and data are: CODE_SEG:0000 and DATA_SEG:0000.
const CODE_SEG: u16 = 0x1000;
const DATA_SEG: u16 = 0x2000;
const DATA: u32 = 0x20000;
const STACK_SEG: u16 = 0x0700;

/// A machine at the prompt, DOS's mode as expanded memory (`ems`) has it,
/// with no hardware interrupts.
fn machine(ems: bool) -> Box<Cpu> {
    let mut cpu = Cpu::new(PathBuf::from("."));
    rust_dos::ems::set_enabled(&mut cpu.bus, ems);
    cpu.load_shell();
    cpu.bus.io_write(0x21, 0xFF);
    cpu.bus.io_write(0xA1, 0xFF);
    cpu
}

/// Run the 16-bit code `f` at CODE_SEG:0000, DS the data, up to a HLT
/// (or the end of the program).
fn run(cpu: &mut Cpu, f: impl FnOnce(&mut CodeAssembler) -> Result<(), IcedError>) {
    let code = asm16(0, f);
    cpu.bus.load_bytes((CODE_SEG as usize) << 4, &code);
    cpu.set_cs(CODE_SEG);
    cpu.set_ip(0);
    cpu.set_ds(DATA_SEG);
    cpu.set_es(DATA_SEG);
    cpu.set_ss(STACK_SEG);
    cpu.set_sp(0x1000);
    for _ in 0..100_000 {
        // (Linear is physical: the VCPI client maps the first megabyte
        // where it is.)
        let at = cpu.seg_cache(Seg::CS).base.wrapping_add(cpu.eip()) as usize;
        if cpu.state != CpuState::Running || cpu.bus.read_8(at) == 0xF4 {
            return;
        }
        cpu.step();
    }
    panic!("no HLT: CS:IP {:04X}:{:04X}", cpu.cs(), cpu.ip());
}

fn read16(cpu: &Cpu, offset: u32) -> u16 {
    cpu.bus.read_16((DATA + offset) as usize)
}

fn read32(cpu: &Cpu, offset: u32) -> u32 {
    cpu.bus.read_32((DATA + offset) as usize)
}

/// MOV EAX,CR0 and SMSW at the prompt's level: with EMS, DOS runs in
/// virtual-8086 mode, where the monitor gives its CR0 (PE and PG set) and
/// SMSW shows PE; without, real mode shows PE clear. Ultima Underworld's
/// installer looks for a memory manager so.
#[test]
fn cr0_shows_protected_mode_with_ems_only() {
    for ems in [false, true] {
        let mut cpu = machine(ems);
        assert_eq!(cpu.v86(), ems);
        run(&mut cpu, |a| {
            a.mov(eax, cr0)?;
            a.mov(dword_ptr(0), eax)?;
            a.smsw(ax)?;
            a.mov(word_ptr(4), ax)?;
            a.hlt()
        });
        // HLT, which the monitor emulates for a program at level 3.
        let ip = cpu.ip();
        cpu.step();
        assert_eq!(cpu.ip(), ip + 1, "past HLT (EMS {ems})");
        assert_ne!(cpu.state, CpuState::RebootShell);
        let (got, msw) = (read32(&cpu, 0), read16(&cpu, 4));
        assert_eq!(got & 1 != 0, ems, "CR0 {got:08X} with EMS {ems}");
        assert_eq!(msw & 1 != 0, ems, "MSW {msw:04X} with EMS {ems}");
        if ems {
            assert_eq!(got & 0x8000_0000, 0x8000_0000, "paging, as EMM386 has it");
            assert!(cpu.v86(), "still in virtual-8086 mode");
        }
    }
}

/// INT n, an exception and the flags under the monitor: the handlers in
/// the real-mode vector table run with the frames real mode gives them,
/// and IOPL stays 3.
#[test]
fn interrupts_and_exceptions_reach_the_vector_table() {
    let mut cpu = machine(true);
    // INT 60h's handler counts and keeps the flags it was called with;
    // the divide error's skips the 2-byte DIV.
    let handler60 = asm16(0x100, |a| {
        a.push(bp)?;
        a.mov(bp, sp)?;
        a.mov(ax, word_ptr(bp + 6))?;
        a.mov(word_ptr(0x12).ds(), ax)?;
        a.inc(byte_ptr(0x10).ds())?;
        a.pop(bp)?;
        a.iret()
    });
    let handler0 = asm16(0x200, |a| {
        a.push(bp)?;
        a.mov(bp, sp)?;
        a.add(word_ptr(bp + 2), 2)?;
        a.inc(byte_ptr(0x11).ds())?;
        a.pop(bp)?;
        a.iret()
    });
    cpu.bus.load_bytes(0x30100, &handler60);
    cpu.bus.load_bytes(0x30200, &handler0);
    cpu.bus.write_32(0x60 * 4, 0x3000_0100);
    cpu.bus.write_32(0, 0x3000_0200);
    run(&mut cpu, |a| {
        a.sti()?;
        a.int(0x60)?;
        a.xor(bl, bl)?;
        a.mov(ax, 1)?;
        a.div(bl)?;
        a.pushf()?;
        a.pop(ax)?;
        a.mov(word_ptr(0x14), ax)?;
        a.hlt()
    });
    assert!(cpu.v86());
    assert_eq!(cpu.bus.read_8(0x20010), 1, "INT 60h's handler ran once");
    assert_eq!(
        cpu.bus.read_8(0x20011),
        1,
        "the divide error's handler ran once"
    );
    // The INT's frame has the caller's flags: IF on, IOPL 3.
    assert_eq!(read16(&cpu, 0x12) & 0x3200, 0x3200);
    assert_eq!(read16(&cpu, 0x14) & 0x3000, 0x3000, "IOPL stays 3");
}

/// LGDT, as a program that switches to protected mode itself runs it,
/// stops the program under the monitor, as EMM386 does.
#[test]
fn a_privileged_instruction_ends_the_program() {
    let mut cpu = machine(true);
    run(&mut cpu, |a| {
        a.lgdt(ptr(0x40))?;
        a.mov(byte_ptr(0x10), 1)?;
        a.hlt()
    });
    assert_eq!(cpu.state, CpuState::RebootShell);
    assert_eq!(cpu.bus.read_8(0x20010), 0, "the program went no further");
}

/// The client's tables, in DATA_SEG.
const GDT: u32 = 0x100;
const GDTR: u32 = 0x300;
const IDTR: u32 = 0x308;
const SWITCH: u32 = 0x200;
const ENTRY: u32 = 0x24;
const TSS: u32 = 0x400;
/// The page directory and table 0, page-aligned.
const PAGE_DIR: u32 = 0x31000;
const PAGE_TABLE: u32 = 0x30000;
/// The protected-mode part, linear.
const PM_CODE: u32 = 0x12000;
const FLAT_CODE: u16 = 0x08;
const FLAT_DATA: u16 = 0x10;
const VCPI_CODE: u16 = 0x18;
const CLIENT_TSS: u16 = 0x30;

/// A VCPI client: finds the interface, gets its page table and
/// descriptors, switches to its protected mode with paging, writes there,
/// and comes back to virtual-8086 mode through the entry point, as DOS
/// extenders without a DPMI host do.
#[test]
fn a_vcpi_client_switches_to_protected_mode_and_back() {
    let mut cpu = machine(true);
    let a20 = cpu.bus.a20();
    let gdt = DATA + GDT;
    let write_desc =
        |cpu: &mut Cpu, sel: u16, d: u64| cpu.bus.write_32((gdt + sel as u32) as usize, d as u32);
    let write_hi = |cpu: &mut Cpu, sel: u16, d: u64| {
        cpu.bus
            .write_32((gdt + sel as u32 + 4) as usize, (d >> 32) as u32)
    };
    for (sel, d) in [
        (FLAT_CODE, seg_desc(0, 0xFFFFF, 0x9A, 0xC)),
        (FLAT_DATA, seg_desc(0, 0xFFFFF, 0x92, 0xC)),
        (CLIENT_TSS, sys_desc(DATA + TSS, 0x67, 0x9, 0)),
    ] {
        write_desc(&mut cpu, sel, d);
        write_hi(&mut cpu, sel, d);
    }
    cpu.bus.write_16((DATA + GDTR) as usize, 0x37);
    cpu.bus.write_32((DATA + GDTR + 2) as usize, gdt);
    cpu.bus.write_16((DATA + IDTR) as usize, 0);
    cpu.bus.write_32((DATA + IDTR + 2) as usize, 0);
    cpu.bus.write_32(PAGE_DIR as usize, PAGE_TABLE | 7);
    let s = (DATA + SWITCH) as usize;
    cpu.bus.write_32(s, PAGE_DIR);
    cpu.bus.write_32(s + 4, DATA + GDTR);
    cpu.bus.write_32(s + 8, DATA + IDTR);
    cpu.bus.write_16(s + 0x0C, 0);
    cpu.bus.write_16(s + 0x0E, CLIENT_TSS);
    cpu.bus.write_32(s + 0x10, PM_CODE);
    cpu.bus.write_16(s + 0x14, FLAT_CODE);
    cpu.bus.write_16((DATA + ENTRY + 4) as usize, VCPI_CODE);

    // Back in virtual-8086 mode at CODE_SEG:0100h.
    let back = asm16(0x100, |a| {
        a.mov(ax, DATA_SEG as u32)?;
        a.mov(ds, ax)?;
        a.mov(byte_ptr(0x30), 1)?;
        a.hlt()
    });
    cpu.bus
        .load_bytes(((CODE_SEG as usize) << 4) + 0x100, &back);
    let pm = asm32(PM_CODE, |a| {
        a.mov(ax, FLAT_DATA as u32)?;
        a.mov(ds, ax)?;
        a.mov(es, ax)?;
        a.mov(ss, ax)?;
        a.mov(esp, 0x9000)?;
        a.mov(dword_ptr(DATA + 0x40), 0x1234_5678)?;
        // The frame for the switch back: GS, FS, DS, ES, SS, ESP, EFLAGS,
        // CS, EIP, under the far call's return address.
        for seg in [DATA_SEG, DATA_SEG, DATA_SEG, DATA_SEG, STACK_SEG] {
            a.push(seg as u32)?;
        }
        a.push(0x0F00u32)?;
        a.push(0u32)?;
        a.push(CODE_SEG as u32)?;
        a.push(0x100u32)?;
        a.mov(ax, 0xDE0C)?;
        a.call(fword_ptr(DATA + ENTRY))
    });
    cpu.bus.load_bytes(PM_CODE as usize, &pm);

    run(&mut cpu, |a| {
        a.mov(ax, 0xDE00)?;
        a.int(0x67)?;
        a.mov(byte_ptr(0x20), ah)?;
        a.mov(word_ptr(0x22), bx)?;
        a.mov(ax, PAGE_TABLE >> 4)?;
        a.mov(es, ax)?;
        a.xor(di, di)?;
        a.mov(si, GDT + VCPI_CODE as u32)?;
        a.mov(ax, 0xDE01)?;
        a.int(0x67)?;
        a.mov(byte_ptr(0x21), ah)?;
        a.mov(dword_ptr(ENTRY), ebx)?;
        a.mov(word_ptr(0x2C), di)?;
        a.cli()?;
        a.mov(esi, DATA + SWITCH)?;
        a.mov(ax, 0xDE0C)?;
        a.int(0x67)?;
        // Not reached: the switch doesn't return.
        a.mov(byte_ptr(0x31), 1)?;
        a.hlt()
    });

    assert_eq!(cpu.bus.read_8(0x20020), 0, "VCPI is there");
    assert_eq!(read16(&cpu, 0x22), 0x0100, "VCPI 1.0");
    assert_eq!(cpu.bus.read_8(0x20021), 0);
    assert_eq!(
        read16(&cpu, 0x2C),
        0x110 * 4,
        "DI past the first megabyte and the HMA"
    );
    assert_eq!(
        cpu.bus.read_32(PAGE_TABLE as usize + 5 * 4) & !0xFFF,
        0x5000
    );
    assert_eq!(
        read32(&cpu, 0x40),
        0x1234_5678,
        "the protected-mode part ran"
    );
    assert_eq!(cpu.bus.read_8(0x20031), 0);
    assert_eq!(cpu.bus.read_8(0x20030), 1, "back in virtual-8086 mode");
    assert!(cpu.v86());
    assert_eq!(
        (cpu.cs(), cpu.ss(), cpu.sp()),
        (CODE_SEG, STACK_SEG, 0x0F00)
    );
    assert_eq!(cpu.bus.a20(), a20, "DOS's A20 gate back");
    assert_eq!(cpu.cr0 & 0x8000_0000, 0, "the monitor's paging");
}

/// VCPI's 4 KB pages come from expanded memory's pool and go back.
#[test]
fn vcpi_pages_are_allocated_and_freed() {
    let mut cpu = machine(true);
    run(&mut cpu, |a| {
        a.mov(ax, 0xDE03)?;
        a.int(0x67)?;
        a.mov(dword_ptr(0), edx)?;
        a.mov(ax, 0xDE04)?;
        a.int(0x67)?;
        a.mov(byte_ptr(0x10), ah)?;
        a.mov(dword_ptr(4), edx)?;
        a.mov(ax, 0xDE03)?;
        a.int(0x67)?;
        a.mov(dword_ptr(8), edx)?;
        a.mov(edx, dword_ptr(4))?;
        a.mov(ax, 0xDE05)?;
        a.int(0x67)?;
        a.mov(byte_ptr(0x11), ah)?;
        a.mov(ax, 0xDE05)?;
        a.int(0x67)?;
        a.mov(byte_ptr(0x12), ah)?;
        a.hlt()
    });
    let (free, page, after) = (read32(&cpu, 0), read32(&cpu, 4), read32(&cpu, 8));
    assert_eq!(cpu.bus.read_8(0x20010), 0);
    assert!(page >= 0x11_0000 && page & 0xFFF == 0, "{page:X}");
    assert_eq!(after, free - 1);
    assert_eq!(cpu.bus.read_8(0x20011), 0, "freed");
    assert_eq!(cpu.bus.read_8(0x20012), 0x8A, "not allocated any more");
}

/// In real mode (EMS off, or a state saved before the monitor), VCPI
/// isn't there.
#[test]
fn vcpi_is_absent_in_real_mode() {
    let mut cpu = machine(true);
    rust_dos::v86mon::leave(&mut cpu);
    run(&mut cpu, |a| {
        a.mov(ax, 0xDE00)?;
        a.int(0x67)?;
        a.mov(byte_ptr(0), ah)?;
        a.hlt()
    });
    assert_eq!(cpu.bus.read_8(0x20000), 0x84);
}

/// Turning EMS off at the prompt goes back to real mode, and on again
/// under the monitor.
#[test]
fn ems_at_the_prompt_switches_the_mode() {
    let mut cpu = machine(true);
    assert!(cpu.v86());
    cpu.set_upper_memory(false, true).unwrap();
    assert!(!cpu.pe());
    cpu.set_upper_memory(true, true).unwrap();
    assert!(cpu.v86());
}
