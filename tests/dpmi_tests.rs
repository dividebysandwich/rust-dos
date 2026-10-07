//! The DPMI host (dpmi.rs): programs that find it with INT 2Fh AX=1687h,
//! become its clients and use its services, as DOS extenders do. The test
//! programs are COM files assembled here; they keep what they find in
//! their own segment, which outlives them, and end with an exit code.

use iced_x86::code_asm::*;
use rust_dos::cpu::{Cpu, CpuState};
use rust_dos::exec::{NoHook, StopReason, run_batch};
use std::fs;
use std::path::PathBuf;

/// Where the test programs keep things, in their segment.
const ENTRY: u16 = 0x2000;
const OLD8: u16 = 0x2004;
const DSSEL: u16 = 0x2010;
const CSSEL: u16 = 0x2012;
const SSSEL: u16 = 0x2014;
const RMSEG: u16 = 0x2016;
const RM2PM: u16 = 0x2018;
const PM2RM: u16 = 0x201C;
const TICKS: u16 = 0x2020;
const CBCOUNT: u16 = 0x2022;
const EXCCOUNT: u16 = 0x2024;
const EXCERR: u16 = 0x2026;
const STEP: u16 = 0x2028;
const OLDEXC: u16 = 0x202C;
const CBADDR: u16 = 0x2030;
const STRUCT: u16 = 0x2040;
const CBSTRUCT: u16 = 0x2080;
/// Results, a word each.
const R: u16 = 0x2100;
const MAGIC: u16 = 0x2300;
const NAME: u16 = 0x2400;
const PARAMS: u16 = 0x2420;
const TAIL: u16 = 0x2440;
const FCB: u16 = 0x2450;
const END: u16 = 0x2500;

/// Code placed at fixed offsets: handlers and the parts of the raw switch.
const EXC_HANDLER: u16 = 0x0C00;
const TICK_HANDLER: u16 = 0x0D00;
const CB_PROC: u16 = 0x0E00;
const RM_PART: u16 = 0x0F00;
const PM_BACK: u16 = 0x1000;

const MAGIC_VALUE: u16 = 0x4D47;

fn scratch(name: &str, files: &[(&str, Vec<u8>)]) -> PathBuf {
    let base = PathBuf::from("target/test_dpmi").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    for (file, bytes) in files {
        fs::write(base.join(file), bytes).unwrap();
    }
    base
}

fn asm16(origin: u16, f: impl FnOnce(&mut CodeAssembler) -> Result<(), IcedError>) -> Vec<u8> {
    let mut a = CodeAssembler::new(16).unwrap();
    f(&mut a).unwrap();
    a.assemble(origin as u64).unwrap()
}

/// A COM file with `parts` at their offsets in the segment, and the magic
/// word at `MAGIC`.
fn com(parts: &[(u16, Vec<u8>)]) -> Vec<u8> {
    let mut image = vec![0u8; (END - 0x100) as usize];
    for (at, code) in parts {
        let at = (*at - 0x100) as usize;
        image[at..at + code.len()].copy_from_slice(code);
    }
    let magic = (MAGIC - 0x100) as usize;
    image[magic..magic + 2].copy_from_slice(&MAGIC_VALUE.to_le_bytes());
    image
}

/// Note the stage the program reached, for a failure.
fn step(a: &mut CodeAssembler, n: u16) -> Result<(), IcedError> {
    a.mov(word_ptr(STEP), n as u32)
}

/// Real mode: give DOS back all but 64 KB, find the host, give it its
/// private data and enter protected mode as a 32-bit (`bits32`) or 16-bit
/// client, keeping the selectors and the real-mode segment.
fn enter(a: &mut CodeAssembler, bits32: bool, fail: CodeLabel) -> Result<(), IcedError> {
    a.mov(word_ptr(RMSEG), cs)?;
    a.mov(bx, 0x1000)?;
    a.mov(ah, 0x4A)?;
    a.int(0x21)?;
    step(a, 1)?;
    a.mov(ax, 0x1687)?;
    a.int(0x2F)?;
    a.test(ax, ax)?;
    a.jnz(fail)?;
    a.mov(word_ptr(ENTRY), di)?;
    a.mov(word_ptr(ENTRY + 2), es)?;
    step(a, 2)?;
    a.mov(bx, si)?;
    a.mov(ah, 0x48)?;
    a.int(0x21)?;
    a.jc(fail)?;
    a.mov(es, ax)?;
    a.mov(ax, bits32 as u32)?;
    // CALL FAR [ENTRY]
    a.db(&[0xFF, 0x1E])?;
    a.dw(&[ENTRY])?;
    step(a, 3)?;
    a.jc(fail)?;
    a.mov(word_ptr(DSSEL), ds)?;
    a.mov(word_ptr(CSSEL), cs)?;
    a.mov(word_ptr(SSSEL), ss)
}

/// End the program with exit code `code`.
fn exit(a: &mut CodeAssembler, code: u8) -> Result<(), IcedError> {
    a.mov(ax, 0x4C00 | code as u32)?;
    a.int(0x21)
}

/// Zero the call structure at `at` (DS = ES = the data).
fn clear_struct(a: &mut CodeAssembler, at: u16) -> Result<(), IcedError> {
    a.push(ds)?;
    a.pop(es)?;
    a.mov(di, at as u32)?;
    a.mov(cx, 0x19)?;
    a.xor(ax, ax)?;
    a.cld()?;
    a.rep().stosw()
}

/// The program exercising the host's services, for a 16-bit or 32-bit
/// client. It ends with code 2Ah after the raw switches.
fn services_program(bits32: bool) -> Vec<u8> {
    let size: u16 = if bits32 { 4 } else { 2 };
    let main = asm16(0x100, |a| {
        let mut fail = a.create_label();
        enter(a, bits32, fail)?;
        a.mov(word_ptr(R), 1)?;
        a.mov(word_ptr(R + 2), cs)?;
        a.mov(word_ptr(R + 4), ds)?;
        a.mov(word_ptr(R + 6), ss)?;
        a.mov(word_ptr(R + 8), es)?;
        // The PSP through ES, and its environment's selector.
        a.mov(ax, word_ptr(0).es())?;
        a.mov(word_ptr(R + 10), ax)?;
        a.mov(ax, word_ptr(0x2C).es())?;
        a.mov(word_ptr(R + 12), ax)?;
        // The version.
        step(a, 10)?;
        a.mov(ax, 0x0400)?;
        a.int(0x31)?;
        a.jc(fail)?;
        a.mov(word_ptr(R + 14), ax)?;
        a.mov(word_ptr(R + 16), bx)?;
        // The coprocessor, as DJGPP's startup asks, and emulation off.
        a.mov(ax, 0x0E00)?;
        a.int(0x31)?;
        a.jc(fail)?;
        a.mov(word_ptr(R + 42), ax)?;
        a.mov(ax, 0x0E01)?;
        a.mov(bx, 1)?;
        a.int(0x31)?;
        a.jc(fail)?;
        // A descriptor with DS's base and a 64 KB limit, read through.
        step(a, 11)?;
        a.xor(ax, ax)?;
        a.mov(cx, 1)?;
        a.int(0x31)?;
        a.jc(fail)?;
        a.mov(word_ptr(R + 18), ax)?;
        a.mov(ax, 0x0006)?;
        a.mov(bx, ds)?;
        a.int(0x31)?;
        a.jc(fail)?;
        a.mov(ax, 0x0007)?;
        a.mov(bx, word_ptr(R + 18))?;
        a.int(0x31)?;
        a.jc(fail)?;
        a.mov(ax, 0x0008)?;
        a.xor(cx, cx)?;
        a.mov(dx, 0xFFFF)?;
        a.int(0x31)?;
        a.jc(fail)?;
        // Access rights for level 0, as for an extender's own host, which
        // the client gets at its level.
        a.mov(ax, 0x0009)?;
        a.mov(cx, 0x0093)?;
        a.int(0x31)?;
        a.jc(fail)?;
        a.mov(es, word_ptr(R + 18))?;
        a.mov(ax, word_ptr(MAGIC as u32).es())?;
        a.mov(word_ptr(R + 20), ax)?;
        // 64 KB of extended memory, written through that descriptor.
        step(a, 12)?;
        a.mov(ax, 0x0501)?;
        a.mov(bx, 1)?;
        a.xor(cx, cx)?;
        a.int(0x31)?;
        a.jc(fail)?;
        a.mov(word_ptr(R + 22), cx)?;
        a.mov(word_ptr(R + 24), bx)?;
        a.mov(dx, cx)?;
        a.mov(cx, bx)?;
        a.mov(bx, word_ptr(R + 18))?;
        a.mov(ax, 0x0007)?;
        a.int(0x31)?;
        a.jc(fail)?;
        a.mov(es, word_ptr(R + 18))?;
        a.mov(word_ptr(0x10).es(), 0xBEEF)?;
        // Real-mode INT 21h AH=62h through a call structure: the PSP.
        step(a, 13)?;
        clear_struct(a, STRUCT)?;
        a.mov(word_ptr(STRUCT + 0x1C), 0x6200)?;
        a.mov(ax, 0x0300)?;
        a.mov(bx, 0x21)?;
        a.xor(cx, cx)?;
        a.xor(edi, edi)?;
        a.mov(di, STRUCT as u32)?;
        a.int(0x31)?;
        a.jc(fail)?;
        a.mov(ax, word_ptr(STRUCT + 0x10))?;
        a.mov(word_ptr(R + 26), ax)?;
        // The BIOS data area at selector 0040h: the equipment word.
        a.mov(ax, 0x40)?;
        a.mov(es, ax)?;
        a.mov(ax, word_ptr(0x10).es())?;
        a.mov(word_ptr(R + 36), ax)?;
        // INT 21h AH=30h reflected: DOS's version in AL.
        step(a, 14)?;
        a.mov(ax, 0x3000)?;
        a.int(0x21)?;
        a.mov(word_ptr(R + 28), ax)?;
        // A #GP (loading a selector that isn't there) to the exception
        // handler, which skips the instruction.
        step(a, 15)?;
        a.mov(ax, 0x0202)?;
        a.mov(bl, 0x0D)?;
        a.int(0x31)?;
        a.mov(dword_ptr(OLDEXC), edx)?;
        a.mov(word_ptr(OLDEXC + 4), cx)?;
        a.mov(ax, 0x0203)?;
        a.mov(bl, 0x0D)?;
        a.mov(cx, cs)?;
        a.xor(edx, edx)?;
        a.mov(dx, EXC_HANDLER as u32)?;
        a.int(0x31)?;
        a.jc(fail)?;
        a.mov(ax, 0x1234)?;
        a.mov(es, ax)?;
        a.mov(ax, 0x0203)?;
        a.mov(bl, 0x0D)?;
        a.mov(edx, dword_ptr(OLDEXC))?;
        a.mov(cx, word_ptr(OLDEXC + 4))?;
        a.int(0x31)?;
        a.jc(fail)?;
        // The timer interrupt to a handler that chains to the default
        // one, waited for with HLT, which level 3 may not run.
        step(a, 16)?;
        a.mov(ax, 0x0204)?;
        a.mov(bl, 8)?;
        a.int(0x31)?;
        a.mov(word_ptr(OLD8), dx)?;
        a.mov(word_ptr(OLD8 + 2), cx)?;
        a.mov(ax, 0x0205)?;
        a.mov(bl, 8)?;
        a.mov(cx, cs)?;
        a.xor(edx, edx)?;
        a.mov(dx, TICK_HANDLER as u32)?;
        a.int(0x31)?;
        a.jc(fail)?;
        a.sti()?;
        let mut wait = a.create_label();
        a.set_label(&mut wait)?;
        a.hlt()?;
        a.cmp(word_ptr(TICKS), 3)?;
        a.jb(wait)?;
        a.mov(ax, 0x0205)?;
        a.mov(bl, 8)?;
        a.mov(cx, word_ptr(OLD8 + 2))?;
        a.xor(edx, edx)?;
        a.mov(dx, word_ptr(OLD8))?;
        a.int(0x31)?;
        a.jc(fail)?;
        // A real-mode callback, far-called from real mode.
        step(a, 17)?;
        a.push(ds)?;
        a.pop(es)?;
        a.xor(edi, edi)?;
        a.mov(di, CBSTRUCT as u32)?;
        a.push(cs)?;
        a.pop(ds)?;
        a.xor(esi, esi)?;
        a.mov(si, CB_PROC as u32)?;
        a.mov(ax, 0x0303)?;
        a.int(0x31)?;
        a.push(es)?;
        a.pop(ds)?;
        a.jc(fail)?;
        a.mov(word_ptr(CBADDR), dx)?;
        a.mov(word_ptr(CBADDR + 2), cx)?;
        clear_struct(a, STRUCT)?;
        a.mov(ax, word_ptr(CBADDR))?;
        a.mov(word_ptr(STRUCT + 0x2A), ax)?;
        a.mov(ax, word_ptr(CBADDR + 2))?;
        a.mov(word_ptr(STRUCT + 0x2C), ax)?;
        a.mov(ax, 0x0301)?;
        a.xor(bx, bx)?;
        a.xor(cx, cx)?;
        a.mov(di, STRUCT as u32)?;
        a.int(0x31)?;
        a.jc(fail)?;
        a.mov(ax, 0x0304)?;
        a.mov(cx, word_ptr(CBADDR + 2))?;
        a.mov(dx, word_ptr(CBADDR))?;
        a.int(0x31)?;
        a.jc(fail)?;
        // Raw switches to real mode and back (which ends the program), FS and
        // GS kept.
        step(a, 18)?;
        a.mov(ax, word_ptr(R + 18))?;
        a.mov(fs, ax)?;
        a.mov(gs, ax)?;
        a.mov(ax, 0x0306)?;
        a.int(0x31)?;
        a.mov(word_ptr(RM2PM), cx)?;
        a.mov(word_ptr(RM2PM + 2), bx)?;
        a.mov(word_ptr(PM2RM), di)?;
        a.mov(word_ptr(PM2RM + 2), si)?;
        a.mov(ax, word_ptr(RMSEG))?;
        a.mov(cx, ax)?;
        a.mov(dx, ax)?;
        a.mov(si, ax)?;
        a.xor(ebx, ebx)?;
        a.mov(bx, sp)?;
        a.xor(edi, edi)?;
        a.mov(di, RM_PART as u32)?;
        // JMP FAR [PM2RM]
        a.db(&[0xFF, 0x2E])?;
        a.dw(&[PM2RM])?;
        a.set_label(&mut fail)?;
        exit(a, 0xEE)
    });
    // The exception handler: the error code, and on past the 2-byte
    // instruction.
    let exc = asm16(EXC_HANDLER, |a| {
        a.push(bp)?;
        a.mov(bp, sp)?;
        a.mov(ax, word_ptr(bp + (2 + 2 * size) as i32))?;
        a.mov(word_ptr(EXCERR), ax)?;
        if bits32 {
            a.add(dword_ptr(bp + (2 + 3 * size) as i32), 2)?;
        } else {
            a.add(word_ptr(bp + (2 + 3 * size) as i32), 2)?;
        }
        a.inc(word_ptr(EXCCOUNT))?;
        a.pop(bp)?;
        if bits32 { a.db(&[0x66, 0xCB]) } else { a.retf() }
    });
    // The timer handler: count, then JMP FAR to the default handler.
    let tick = asm16(TICK_HANDLER, |a| {
        a.push(ds)?;
        a.push(ax)?;
        a.mov(ds, word_ptr(DSSEL).cs())?;
        a.inc(word_ptr(TICKS))?;
        a.pop(ax)?;
        a.pop(ds)?;
        a.db(&[0x2E, 0xFF, 0x2E])?;
        a.dw(&[OLD8])
    });
    // The callback's procedure: return to the caller of the real-mode far
    // call, and count.
    let cb = asm16(CB_PROC, |a| {
        a.mov(ax, word_ptr(si))?;
        a.mov(word_ptr(di + 0x2A).es(), ax)?;
        a.mov(ax, word_ptr(si + 2))?;
        a.mov(word_ptr(di + 0x2C).es(), ax)?;
        a.add(word_ptr(di + 0x2E).es(), 4)?;
        a.inc(word_ptr(CBCOUNT).es())?;
        if bits32 { a.iretd() } else { a.iret() }
    });
    // In real mode after the raw switch: note it, and switch back.
    let rm = asm16(RM_PART, |a| {
        a.mov(word_ptr(R + 30), 0x5151)?;
        a.mov(ax, word_ptr(DSSEL))?;
        a.mov(cx, ax)?;
        a.mov(dx, word_ptr(SSSEL))?;
        a.mov(si, word_ptr(CSSEL))?;
        a.xor(ebx, ebx)?;
        a.mov(bx, sp)?;
        a.xor(edi, edi)?;
        a.mov(di, PM_BACK as u32)?;
        a.db(&[0xFF, 0x2E])?;
        a.dw(&[RM2PM])
    });
    let back = asm16(PM_BACK, |a| {
        a.mov(word_ptr(R + 32), ds)?;
        a.mov(word_ptr(R + 34), cs)?;
        a.mov(word_ptr(R + 38), fs)?;
        a.mov(word_ptr(R + 40), gs)?;
        exit(a, 0x2A)
    });
    com(&[(0x100, main), (EXC_HANDLER, exc), (TICK_HANDLER, tick), (CB_PROC, cb), (RM_PART, rm), (PM_BACK, back)])
}

/// Run the machine until the program ends and the shell is back, for at
/// most `batches` batches.
fn run_to_exit(cpu: &mut Cpu, batches: usize) -> bool {
    for _ in 0..batches {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        if run_batch(cpu, &mut NoHook, false) == StopReason::ShellReloaded {
            return true;
        }
    }
    false
}

/// A machine with the program `name` in `files` loaded from the shell.
fn machine(test: &str, files: &[(&str, Vec<u8>)], name: &str) -> (Cpu, u16) {
    let mut cpu = Cpu::new(scratch(test, files));
    cpu.load_shell();
    assert!(cpu.load_executable(name, None));
    let psp = cpu.current_psp;
    (cpu, psp)
}

fn word(cpu: &Cpu, psp: u16, offset: u16) -> u16 {
    cpu.bus.read_16(psp as usize * 16 + offset as usize)
}

fn check_services(bits32: bool) {
    let (mut cpu, psp) = machine(
        if bits32 { "services32" } else { "services16" },
        &[("T.COM", services_program(bits32))],
        "T.COM",
    );
    let a20 = cpu.bus.a20();
    let timer = cpu.bus.read_32(8 * 4);
    assert!(run_to_exit(&mut cpu, 2000), "the program didn't end (step {})", word(&cpu, psp, STEP));
    let w = |offset: u16| word(&cpu, psp, offset);
    assert_eq!(cpu.errorlevel, 0x2A, "failed at step {}", w(STEP));
    assert_eq!(w(R), 1);
    // CS, DS, SS and ES are LDT selectors at level 3.
    for i in 1..=4 {
        assert_eq!(w(R + 2 * i) & 7, 7, "selector {}", i);
    }
    // ES is the PSP, whose environment pointer is a selector now.
    assert_eq!(w(R + 10), 0x20CD);
    assert_eq!(w(R + 12) & 7, 7);
    // DPMI 0.90 of a 32-bit host that goes to real mode.
    assert_eq!((w(R + 14), w(R + 16) & 3), (0x005A, 3));
    // A coprocessor, enabled for the client and not emulated.
    assert_eq!(w(R + 42) & 0x0F, 0x05);
    assert_eq!(w(R + 18) & 7, 7);
    assert_eq!(w(R + 20), MAGIC_VALUE);
    let linear = (w(R + 24) as usize) << 16 | w(R + 22) as usize;
    assert!(linear >= 0x11_0000, "{:X}", linear);
    assert_eq!(cpu.bus.read_16(linear + 0x10), 0xBEEF);
    assert_eq!(w(R + 26), psp);
    assert_eq!(w(R + 28) & 0xFF, 5);
    assert_eq!((w(EXCCOUNT), w(EXCERR)), (1, 0x1234));
    assert!(w(TICKS) >= 3);
    assert_eq!(w(CBCOUNT), 1);
    assert_eq!(w(R + 30), 0x5151);
    assert_eq!((w(R + 32), w(R + 34)), (w(DSSEL), w(CSSEL)));
    assert_eq!((w(R + 38), w(R + 40)), (w(R + 18), w(R + 18)));
    assert_eq!(w(R + 36), cpu.bus.read_16(0x410));
    // The host has gone with its client, and left things as they were.
    assert!(!cpu.bus.dpmi.active());
    assert_eq!(cpu.bus.read_32(8 * 4), timer);
    assert_eq!(cpu.bus.a20(), a20);
    assert_eq!(cpu.state, CpuState::Running);
}

#[test]
fn a_16_bit_client_uses_the_services() {
    check_services(false);
}

#[test]
fn a_32_bit_client_uses_the_services() {
    check_services(true);
}

#[test]
fn the_host_answers_int_2fh_1687h() {
    let (mut cpu, _) = machine("check", &[("T.COM", vec![0xEB, 0xFE])], "T.COM");
    cpu.set_ax(0x1687);
    rust_dos::interrupts::handle_hle(&mut cpu, 0x2F);
    assert_eq!((cpu.ax(), cpu.bx() & 1, cpu.dx()), (0, 1, 0x005A));
    assert!(cpu.si() > 0);
    assert_eq!((cpu.es(), cpu.di()), (0xF000, rust_dos::dpmi::ENTRY));

    // Turned off, it isn't there.
    cpu.bus.dpmi.enabled = false;
    cpu.set_ax(0x1687);
    rust_dos::interrupts::handle_hle(&mut cpu, 0x2F);
    assert_eq!(cpu.ax(), 0x1687);
}

/// A child that becomes a client too, then ends with code 33h.
fn child_program() -> Vec<u8> {
    let main = asm16(0x100, |a| {
        let mut fail = a.create_label();
        enter(a, false, fail)?;
        exit(a, 0x33)?;
        a.set_label(&mut fail)?;
        exit(a, 0xEE)
    });
    com(&[(0x100, main)])
}

/// A 32-bit client that runs CHILD.COM with EXEC through INT 31h AX=0300h
/// and ends with the child's exit code.
fn parent_program() -> Vec<u8> {
    let main = asm16(0x100, |a| {
        let mut fail = a.create_label();
        // The EXEC parameter block: no environment of its own, an empty
        // command tail, empty FCBs.
        a.mov(word_ptr(PARAMS + 4), cs)?;
        a.mov(word_ptr(PARAMS + 8), cs)?;
        a.mov(word_ptr(PARAMS + 12), cs)?;
        enter(a, true, fail)?;
        step(a, 20)?;
        clear_struct(a, STRUCT)?;
        a.mov(word_ptr(STRUCT + 0x1C), 0x4B00)?;
        a.mov(word_ptr(STRUCT + 0x14), NAME as u32)?;
        a.mov(word_ptr(STRUCT + 0x10), PARAMS as u32)?;
        a.mov(ax, word_ptr(RMSEG))?;
        a.mov(word_ptr(STRUCT + 0x24), ax)?;
        a.mov(word_ptr(STRUCT + 0x22), ax)?;
        a.mov(ax, 0x0300)?;
        a.mov(bx, 0x21)?;
        a.xor(cx, cx)?;
        a.xor(edi, edi)?;
        a.mov(di, STRUCT as u32)?;
        a.int(0x31)?;
        a.jc(fail)?;
        step(a, 21)?;
        a.test(byte_ptr(STRUCT + 0x20), 1)?;
        a.jnz(fail)?;
        // Its exit code, and the parent's own data still there.
        a.mov(ah, 0x4D)?;
        a.int(0x21)?;
        a.mov(word_ptr(R), ax)?;
        a.mov(bx, word_ptr(MAGIC))?;
        a.mov(word_ptr(R + 2), bx)?;
        a.mov(ah, 0x4C)?;
        a.int(0x21)?;
        a.set_label(&mut fail)?;
        exit(a, 0xEE)
    });
    let mut image = com(&[(0x100, main)]);
    let at = |offset: u16| (offset - 0x100) as usize;
    image[at(NAME)..at(NAME) + 10].copy_from_slice(b"CHILD.COM\0");
    image[at(PARAMS) + 2..at(PARAMS) + 4].copy_from_slice(&TAIL.to_le_bytes());
    image[at(PARAMS) + 6..at(PARAMS) + 8].copy_from_slice(&FCB.to_le_bytes());
    image[at(PARAMS) + 10..at(PARAMS) + 12].copy_from_slice(&FCB.to_le_bytes());
    image[at(TAIL)..at(TAIL) + 2].copy_from_slice(&[0x00, 0x0D]);
    image
}

#[test]
fn a_client_runs_a_child_that_is_a_client_too() {
    let (mut cpu, psp) =
        machine("nested", &[("PARENT.COM", parent_program()), ("CHILD.COM", child_program())], "PARENT.COM");
    assert!(run_to_exit(&mut cpu, 200), "the program didn't end (step {})", word(&cpu, psp, STEP));
    assert_eq!(cpu.errorlevel, 0x33, "failed at step {}", word(&cpu, psp, STEP));
    assert_eq!(word(&cpu, psp, R + 2), MAGIC_VALUE);
    assert!(!cpu.bus.dpmi.active());
}

/// Where the DOS program keeps things: results (a word each), the "MS-DOS"
/// entry point, handles and blocks, names, EXEC's parameter block, the DTA
/// and a buffer DOS reads into directly.
const D: u16 = 0x2200;
const MSDOS_ENTRY: u16 = 0x2280;
const HANDLE: u16 = 0x2288;
const BLOCK_A: u16 = 0x228A;
const BLOCK_B: u16 = 0x228C;
const BLOCK_C: u16 = 0x228E;
const VENDOR: u16 = 0x2410;
const CHILD_NAME: u16 = 0x2460;
const MESSAGE: u16 = 0x2470;
const DTA: u16 = 0x2480;
const DIRECT: u16 = 0x24F0;
const FILE_SIZE: u32 = 20000;

/// A client that calls DOS with selectors (INT 21h in protected mode) and
/// uses the "MS-DOS" extensions, as Borland's RTM does. It ends with code
/// 2Ah.
fn dos_program(bits32: bool) -> Vec<u8> {
    let main = asm16(0x100, |a| {
        let mut fail = a.create_label();
        enter(a, bits32, fail)?;
        // The "MS-DOS" extensions, and the LDT through their selector: the
        // descriptor of CS has the base INT 31h AX=0006h gives.
        step(a, 30)?;
        a.mov(ax, 0x168A)?;
        a.mov(esi, VENDOR as u32)?;
        a.int(0x2F)?;
        a.mov(word_ptr(D), ax)?;
        a.mov(dword_ptr(MSDOS_ENTRY), edi)?;
        a.mov(word_ptr(MSDOS_ENTRY + if bits32 { 4 } else { 2 }), es)?;
        a.mov(ax, 0x0100)?;
        // CALL FAR [MSDOS_ENTRY], with a 48-bit pointer for a 32-bit client.
        if bits32 {
            a.db(&[0x66])?;
        }
        a.db(&[0xFF, 0x1E])?;
        a.dw(&[MSDOS_ENTRY])?;
        a.jc(fail)?;
        a.mov(word_ptr(D + 2), ax)?;
        a.mov(es, ax)?;
        a.mov(bx, cs)?;
        a.and(bx, 0xFFF8)?;
        a.mov(ax, word_ptr(bx + 2).es())?;
        a.mov(word_ptr(D + 4), ax)?;
        a.mov(ax, 0x0006)?;
        a.mov(bx, cs)?;
        a.int(0x31)?;
        a.mov(word_ptr(D + 6), dx)?;
        // A data alias of a data segment.
        a.mov(ax, 0x000A)?;
        a.mov(bx, ds)?;
        a.int(0x31)?;
        a.setb(byte_ptr(D + 8))?;
        // Two blocks of memory for the file, and a file written from one
        // (more than the transfer buffer holds) and read back into the
        // other.
        step(a, 31)?;
        for block in [BLOCK_A, BLOCK_B] {
            a.mov(ah, 0x48)?;
            a.mov(ebx, FILE_SIZE.div_ceil(16))?;
            a.int(0x21)?;
            a.jc(fail)?;
            a.mov(word_ptr(block as u32), ax)?;
        }
        a.mov(es, word_ptr(BLOCK_A as u32))?;
        a.xor(di, di)?;
        a.mov(cx, FILE_SIZE)?;
        a.xor(al, al)?;
        a.cld()?;
        let mut fill = a.create_label();
        a.set_label(&mut fill)?;
        a.stosb()?;
        a.add(al, 7)?;
        a.loop_(fill)?;
        step(a, 32)?;
        a.mov(ah, 0x3C)?;
        a.xor(cx, cx)?;
        a.mov(edx, NAME as u32)?;
        a.int(0x21)?;
        a.jc(fail)?;
        a.mov(word_ptr(HANDLE), ax)?;
        a.mov(word_ptr(D + 10), ax)?;
        a.mov(ah, 0x40)?;
        a.mov(bx, word_ptr(HANDLE))?;
        a.mov(ecx, FILE_SIZE)?;
        a.xor(edx, edx)?;
        a.push(ds)?;
        a.mov(ds, word_ptr(BLOCK_A as u32))?;
        a.int(0x21)?;
        a.pop(ds)?;
        a.jc(fail)?;
        a.mov(word_ptr(D + 12), ax)?;
        a.mov(ax, 0x4200)?;
        a.xor(cx, cx)?;
        a.xor(dx, dx)?;
        a.int(0x21)?;
        a.mov(ah, 0x3F)?;
        a.mov(ecx, FILE_SIZE)?;
        a.xor(edx, edx)?;
        a.push(ds)?;
        a.mov(ds, word_ptr(BLOCK_B as u32))?;
        a.int(0x21)?;
        a.pop(ds)?;
        a.jc(fail)?;
        a.mov(word_ptr(D + 14), ax)?;
        a.push(ds)?;
        a.mov(es, word_ptr(BLOCK_B as u32))?;
        a.mov(ds, word_ptr(BLOCK_A as u32))?;
        a.xor(si, si)?;
        a.xor(di, di)?;
        a.mov(cx, FILE_SIZE)?;
        a.repe().cmpsb()?;
        a.pop(ds)?;
        a.setne(byte_ptr(D + 16))?;
        // Its start again, read into conventional memory.
        a.mov(ax, 0x4200)?;
        a.xor(cx, cx)?;
        a.xor(dx, dx)?;
        a.int(0x21)?;
        a.mov(ah, 0x3F)?;
        a.mov(ecx, 16)?;
        a.mov(edx, DIRECT as u32)?;
        a.int(0x21)?;
        a.jc(fail)?;
        a.mov(ax, word_ptr(DIRECT))?;
        a.mov(word_ptr(D + 18), ax)?;
        a.mov(ah, 0x3E)?;
        a.int(0x21)?;
        // A search with a DTA of its own, which INT 21h AH=2Fh gives back.
        step(a, 33)?;
        a.mov(ah, 0x1A)?;
        a.mov(edx, DTA as u32)?;
        a.int(0x21)?;
        a.mov(ah, 0x4E)?;
        a.xor(cx, cx)?;
        a.mov(edx, NAME as u32)?;
        a.int(0x21)?;
        a.setb(byte_ptr(D + 20))?;
        a.mov(ax, word_ptr(DTA + 0x1A))?;
        a.mov(word_ptr(D + 22), ax)?;
        a.mov(ah, 0x2F)?;
        a.int(0x21)?;
        a.mov(word_ptr(D + 24), es)?;
        a.mov(word_ptr(D + 26), bx)?;
        // Protected-mode vectors.
        step(a, 34)?;
        a.mov(ax, 0x2560)?;
        a.push(ds)?;
        a.push(cs)?;
        a.pop(ds)?;
        a.mov(edx, 0x1234)?;
        a.int(0x21)?;
        a.pop(ds)?;
        a.mov(ax, 0x3560)?;
        a.int(0x21)?;
        a.mov(word_ptr(D + 28), bx)?;
        a.mov(word_ptr(D + 30), es)?;
        // Segments DOS returns, as selectors: the InDOS flag's and the
        // PSP.
        step(a, 35)?;
        a.mov(ah, 0x34)?;
        a.int(0x21)?;
        a.mov(ax, 0x0006)?;
        a.mov(bx, es)?;
        a.int(0x31)?;
        a.mov(word_ptr(D + 32), dx)?;
        a.mov(word_ptr(D + 34), cx)?;
        a.mov(ah, 0x62)?;
        a.int(0x21)?;
        a.mov(ax, 0x0006)?;
        a.int(0x31)?;
        a.mov(word_ptr(D + 36), dx)?;
        a.mov(word_ptr(D + 38), cx)?;
        // A block of more than 64 KB: its limit, shrunk, then freed.
        step(a, 36)?;
        a.mov(ah, 0x48)?;
        a.mov(ebx, 0x1100)?;
        a.int(0x21)?;
        a.jc(fail)?;
        a.mov(word_ptr(BLOCK_C), ax)?;
        a.db(&[0x66])?;
        a.lsl(ax, word_ptr(BLOCK_C))?;
        a.mov(dword_ptr(D + 40), eax)?;
        a.mov(ah, 0x4A)?;
        a.mov(es, word_ptr(BLOCK_C))?;
        a.mov(ebx, 0x100)?;
        a.int(0x21)?;
        a.setb(byte_ptr(D + 44))?;
        a.db(&[0x66])?;
        a.lsl(ax, word_ptr(BLOCK_C))?;
        a.mov(dword_ptr(D + 46), eax)?;
        a.mov(ah, 0x49)?;
        a.int(0x21)?;
        a.setb(byte_ptr(D + 50))?;
        a.mov(ax, 0x0006)?;
        a.mov(bx, word_ptr(BLOCK_C))?;
        a.int(0x31)?;
        a.setb(byte_ptr(D + 52))?;
        // EXEC, with the parameter block in the client's format.
        step(a, 37)?;
        a.push(ds)?;
        a.pop(es)?;
        a.mov(di, PARAMS as u32)?;
        a.mov(cx, 16)?;
        a.xor(ax, ax)?;
        a.rep().stosw()?;
        let pointers: [(u16, u16); 3] = if bits32 { [(0, TAIL), (8, FCB), (16, FCB)] } else { [(2, TAIL), (6, FCB), (10, FCB)] };
        for (at, target) in pointers {
            a.mov(word_ptr(PARAMS + at), target as u32)?;
            a.mov(word_ptr(PARAMS + at + if bits32 { 4 } else { 2 }), ds)?;
        }
        a.mov(ax, 0x4B00)?;
        a.mov(edx, CHILD_NAME as u32)?;
        a.mov(ebx, PARAMS as u32)?;
        a.int(0x21)?;
        a.setb(byte_ptr(D + 54))?;
        a.mov(ah, 0x4D)?;
        a.int(0x21)?;
        a.mov(word_ptr(D + 56), ax)?;
        // A string to the screen.
        step(a, 38)?;
        a.mov(ah, 0x09)?;
        a.mov(edx, MESSAGE as u32)?;
        a.int(0x21)?;
        exit(a, 0x2A)?;
        a.set_label(&mut fail)?;
        exit(a, 0xEE)
    });
    let mut image = com(&[(0x100, main)]);
    let at = |offset: u16| (offset - 0x100) as usize;
    for (offset, text) in [
        (NAME, &b"OUT.DAT\0"[..]),
        (VENDOR, b"MS-DOS\0"),
        (CHILD_NAME, b"CHILD.COM\0"),
        (MESSAGE, b"TRANSLATED$"),
        (TAIL, &[0x00, 0x0D]),
    ] {
        image[at(offset)..at(offset) + text.len()].copy_from_slice(text);
    }
    image
}

fn check_dos(bits32: bool) {
    let test = if bits32 { "dos32" } else { "dos16" };
    let (mut cpu, psp) = machine(test, &[("T.COM", dos_program(bits32)), ("CHILD.COM", child_program())], "T.COM");
    assert!(run_to_exit(&mut cpu, 2000), "the program didn't end (step {})", word(&cpu, psp, STEP));
    let w = |offset: u16| word(&cpu, psp, offset);
    assert_eq!(cpu.errorlevel, 0x2A, "failed at step {}", w(STEP));
    // The extensions are there, with a selector for the LDT.
    assert_eq!(w(D) & 0xFF, 0);
    assert_eq!(w(D + 2) & 7, 7);
    assert_eq!(w(D + 4), w(D + 6));
    assert_eq!(w(D + 8) & 0xFF, 0);
    // The file went through the transfer buffer in pieces, both ways.
    let file = fs::read(PathBuf::from("target/test_dpmi").join(test).join("OUT.DAT")).unwrap();
    assert_eq!(file.len(), FILE_SIZE as usize);
    assert!(file.iter().enumerate().all(|(i, &b)| b == (i * 7) as u8));
    assert_eq!((w(D + 12), w(D + 14), w(D + 16) & 0xFF), (FILE_SIZE as u16, FILE_SIZE as u16, 0));
    assert_eq!(w(D + 18), 0x0700);
    // The search filled the client's DTA.
    assert_eq!((w(D + 20) & 0xFF, w(D + 22)), (0, FILE_SIZE as u16));
    let name = psp as usize * 16 + DTA as usize + 0x1E;
    assert_eq!(&cpu.bus.ram()[name..name + 8], b"OUT.DAT\0");
    assert_eq!((w(D + 24), w(D + 26)), (w(DSSEL), DTA));
    assert_eq!((w(D + 28), w(D + 30)), (0x1234, w(CSSEL)));
    let base = |low: u16| (w(low + 2) as u32) << 16 | w(low) as u32;
    assert_eq!(base(D + 32), rust_dos::dos_data::SEGMENT as u32 * 16);
    assert_eq!(base(D + 36), psp as u32 * 16);
    // One selector for all of it for a 32-bit client; a 16-bit one's are
    // 64 KB each.
    let limits = if bits32 { (0x10FFF, 0xFFF) } else { (0xFFFF, 0xFFF) };
    assert_eq!((base(D + 40), base(D + 46)), limits);
    assert_eq!((w(D + 44) & 0xFF, w(D + 50) & 0xFF, w(D + 52) & 0xFF), (0, 0, 1));
    assert_eq!((w(D + 54) & 0xFF, w(D + 56)), (0, 0x33));
    let screen: Vec<u8> = (0..4000).step_by(2).map(|i| cpu.bus.read_8(0xB8000 + i)).collect();
    assert!(screen.windows(10).any(|w| w == b"TRANSLATED"));
    assert!(!cpu.bus.dpmi.active());
}

#[test]
fn a_16_bit_client_calls_dos_with_selectors() {
    check_dos(false);
}

#[test]
fn a_32_bit_client_calls_dos_with_selectors() {
    check_dos(true);
}

#[test]
fn a_client_goes_on_after_a_state_is_loaded() {
    let (mut cpu, psp) = machine("state", &[("T.COM", services_program(true))], "T.COM");
    // Until it waits for timer ticks in protected mode.
    for _ in 0..2000 {
        cpu.bus.start_batch(cpu.bus.clock.icount + 1000);
        run_batch(&mut cpu, &mut NoHook, false);
        if word(&cpu, psp, STEP) == 16 && word(&cpu, psp, TICKS) >= 1 {
            break;
        }
    }
    assert!(cpu.bus.dpmi.active() && cpu.pe());
    let state = rust_dos::savestate::machine::save(&cpu);
    assert!(run_to_exit(&mut cpu, 2000));
    assert_eq!(cpu.errorlevel, 0x2A);

    rust_dos::savestate::machine::load(&mut cpu, &state).unwrap();
    assert!(cpu.bus.dpmi.active());
    cpu.errorlevel = 0;
    assert!(run_to_exit(&mut cpu, 2000), "the program didn't end (step {})", word(&cpu, psp, STEP));
    assert_eq!(cpu.errorlevel, 0x2A, "failed at step {}", word(&cpu, psp, STEP));
}

#[test]
fn an_exception_the_client_does_not_handle_ends_it() {
    // In protected mode, UD2.
    let main = asm16(0x100, |a| {
        let mut fail = a.create_label();
        enter(a, false, fail)?;
        a.ud2()?;
        exit(a, 0x2A)?;
        a.set_label(&mut fail)?;
        exit(a, 0xEE)
    });
    let (mut cpu, psp) = machine("abort", &[("T.COM", com(&[(0x100, main)]))], "T.COM");
    assert!(run_to_exit(&mut cpu, 200), "the program didn't end (step {})", word(&cpu, psp, STEP));
    assert_eq!(cpu.errorlevel, 0xFF);
    assert!(!cpu.bus.dpmi.active());
}

#[test]
fn a_client_moves_to_and_from_debug_and_control_registers() {
    // In protected mode, with a #GP handler that ends the program: DR3 set
    // and read back, and CR0 read (DOS/4GW Professional's NULLP option
    // sets the debug registers itself).
    let main = asm16(0x100, |a| {
        let mut fail = a.create_label();
        enter(a, false, fail)?;
        a.mov(ax, 0x0203)?;
        a.mov(bl, 0x0D)?;
        a.mov(cx, cs)?;
        a.mov(dx, EXC_HANDLER as u32)?;
        a.int(0x31)?;
        step(a, 4)?;
        a.mov(eax, 0x1234_5678)?;
        a.mov(dr3, eax)?;
        a.xor(ebx, ebx)?;
        a.mov(ebx, dr3)?;
        a.mov(dword_ptr(R), ebx)?;
        a.mov(ecx, cr0)?;
        a.mov(word_ptr(R + 4), cx)?;
        exit(a, 0x2A)?;
        a.set_label(&mut fail)?;
        exit(a, 0xEE)
    });
    let exc = asm16(EXC_HANDLER, |a| exit(a, 0xDD));
    let (mut cpu, psp) = machine("movsystem", &[("T.COM", com(&[(0x100, main), (EXC_HANDLER, exc)]))], "T.COM");
    assert!(run_to_exit(&mut cpu, 200), "the program didn't end (step {})", word(&cpu, psp, STEP));
    assert_eq!(cpu.errorlevel, 0x2A, "failed at step {}", word(&cpu, psp, STEP));
    assert_eq!(cpu.bus.read_32(psp as usize * 16 + R as usize), 0x1234_5678);
    assert_eq!(word(&cpu, psp, R + 4) & 1, 1);
}

#[test]
fn a_client_allocates_linear_memory_at_an_address() {
    // DPMI 1.0's 0504h, as HX's loader places a program whose relocations
    // were stripped at its image base: the block at 400000h, the address
    // taken a second time and one not page-aligned refused, the block
    // resized with 0505h, freed, and taken again.
    let main = asm16(0x100, |a| {
        let mut fail = a.create_label();
        enter(a, true, fail)?;
        step(a, 4)?;
        a.mov(ax, 0x0504)?;
        a.mov(ebx, 0x40_0000)?;
        a.mov(ecx, 0x1_0000)?;
        a.mov(edx, 1)?;
        a.int(0x31)?;
        a.jc(fail)?;
        a.mov(dword_ptr(R), ebx)?;
        a.mov(dword_ptr(R + 4), esi)?;
        step(a, 5)?;
        a.mov(ax, 0x0504)?;
        a.mov(ebx, 0x40_0000)?;
        a.mov(ecx, 0x1000)?;
        a.int(0x31)?;
        a.mov(word_ptr(R + 8), ax)?;
        a.mov(ax, 0x0504)?;
        a.mov(ebx, 0x50_0800)?;
        a.mov(ecx, 0x1000)?;
        a.int(0x31)?;
        a.mov(word_ptr(R + 10), ax)?;
        step(a, 6)?;
        a.mov(ax, 0x0505)?;
        a.mov(esi, dword_ptr(R + 4))?;
        a.mov(ecx, 0x2_0000)?;
        a.xor(edx, edx)?;
        a.int(0x31)?;
        a.jc(fail)?;
        a.mov(dword_ptr(R + 12), ebx)?;
        step(a, 7)?;
        a.mov(ax, 0x0502)?;
        a.mov(di, word_ptr(R + 4))?;
        a.mov(si, word_ptr(R + 6))?;
        a.int(0x31)?;
        a.jc(fail)?;
        a.mov(ax, 0x0504)?;
        a.mov(ebx, 0x40_0000)?;
        a.mov(ecx, 0x1000)?;
        a.int(0x31)?;
        a.jc(fail)?;
        a.mov(dword_ptr(R + 16), ebx)?;
        exit(a, 0x2A)?;
        a.set_label(&mut fail)?;
        exit(a, 0xEE)
    });
    let (mut cpu, psp) = machine("linear", &[("T.COM", com(&[(0x100, main)]))], "T.COM");
    assert!(run_to_exit(&mut cpu, 200), "the program didn't end (step {})", word(&cpu, psp, STEP));
    assert_eq!(cpu.errorlevel, 0x2A, "failed at step {}", word(&cpu, psp, STEP));
    let dword = |offset: u16| cpu.bus.read_32(psp as usize * 16 + offset as usize);
    assert_eq!(dword(R), 0x40_0000);
    assert_eq!(word(&cpu, psp, R + 8), 0x8012, "the address is taken");
    assert_eq!(word(&cpu, psp, R + 10), 0x8025, "not page-aligned");
    assert_eq!(dword(R + 12), 0x40_0000, "grown where it is");
    assert_eq!(dword(R + 16), 0x40_0000);
}
