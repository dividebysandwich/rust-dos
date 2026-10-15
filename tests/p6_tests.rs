//! The Pentium Pro and Pentium II: CPUID, CMOVcc, FCMOVcc, FCOMI, the
//! hinting NOPs, MMX on the Pentium II only, the P6 MSRs, SYSENTER and
//! SYSEXIT, what a Pentium does with them, and the recompiler running
//! them as the interpreter does.

mod dyndiff;
mod pmrig;
mod testrunners;

use iced_x86::code_asm::*;
use pmrig::*;
use rust_dos::cpu::{CoreMode, Cpu, CpuModel};
use testrunners::run_cpu_code;

const UD: u8 = 6;
const GP: u8 = 13;

fn p6_rig(model: CpuModel) -> Rig {
    let mut rig = Rig::new();
    rig.cpu.model = model;
    rig
}

/// Run `code` in real mode on `model`, with #UD's handler at 0000:0700:
/// whether it raised #UD.
fn undefined_on(model: CpuModel, code: &[u8]) -> bool {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    cpu.model = model;
    cpu.bus.write_16(UD as usize * 4, 0x0700);
    cpu.bus.write_16(UD as usize * 4 + 2, 0x0000);
    run_cpu_code(&mut cpu, code);
    (cpu.cs(), cpu.ip()) == (0x0000, 0x0700)
}

#[test]
fn cpuid_reports_a_pentium_pro_and_a_pentium_ii() {
    // As DOSBox-X's ppro_slow and pentium_ii, without the PSE-36 bit:
    // FPU, PSE, TSC, MSR, CMPXCHG8B and CMOV, and MMX and SEP on the II.
    for (model, signature, features) in [(CpuModel::PentiumPro, 0x612, 0x8139), (CpuModel::PentiumII, 0x633, 0x80_8939)] {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        cpu.model = model;
        // 66 B8 01 00 00 00 -> MOV EAX, 1 ; 0F A2 -> CPUID
        run_cpu_code(&mut cpu, &[0x66, 0xB8, 0x01, 0x00, 0x00, 0x00, 0x0F, 0xA2]);
        assert_eq!((cpu.eax(), cpu.edx()), (signature, features), "{model:?}");
        assert_eq!(model.family(), 6);
    }
}

#[test]
fn p6_instructions_are_undefined_on_a_pentium_mmx() {
    // 0F 44 C3 -> CMOVE AX, BX ; DA C1 -> FCMOVB ST0, ST1 ;
    // DB F1 -> FCOMI ST0, ST1 ; DF E9 -> FUCOMIP ST0, ST1 ;
    // 0F 19 C0 -> hinting NOP ; 0F 1F 00 -> NOP [BX+SI] ;
    // 0F 34 -> SYSENTER ; 0F 35 -> SYSEXIT
    for code in [
        &[0x0F, 0x44, 0xC3][..],
        &[0xDA, 0xC1],
        &[0xDB, 0xF1],
        &[0xDF, 0xE9],
        &[0x0F, 0x19, 0xC0],
        &[0x0F, 0x1F, 0x00],
        &[0x0F, 0x34],
        &[0x0F, 0x35],
    ] {
        assert!(undefined_on(CpuModel::PentiumMmx, code), "{:02X?} raises #UD on a Pentium MMX", code);
    }
    // 0F 1C 00 -> CLDEMOTE [BX+SI] ; F3 0F 1E FB -> ENDBR32: hinting NOPs too
    for code in [&[0x0F, 0x44, 0xC3][..], &[0x0F, 0x19, 0xC0], &[0x0F, 0x1C, 0x00], &[0xF3, 0x0F, 0x1E, 0xFB], &[0x0F, 0x1F, 0x00]] {
        assert!(!undefined_on(CpuModel::PentiumPro, code), "{:02X?} runs on a Pentium Pro", code);
    }
}

#[test]
fn a_pentium_pro_has_no_mmx_or_sysenter() {
    // 0F 77 -> EMMS ; 0F FC C1 -> PADDB MM0, MM1 ; 0F 34 -> SYSENTER
    for code in [&[0x0F, 0x77][..], &[0x0F, 0xFC, 0xC1], &[0x0F, 0x34]] {
        assert!(undefined_on(CpuModel::PentiumPro, code), "{:02X?} raises #UD on a Pentium Pro", code);
    }
    assert!(!undefined_on(CpuModel::PentiumII, &[0x0F, 0x77]), "EMMS runs on a Pentium II");
}

#[test]
fn cmov_moves_only_when_the_condition_holds() {
    let mut rig = p6_rig(CpuModel::PentiumPro);
    rig.write32(DATA, 0x1234_5678);
    rig.run(|a| {
        a.mov(eax, 1u32)?;
        a.mov(ebx, 2u32)?;
        a.mov(ecx, 3u32)?;
        a.mov(edx, 4u32)?;
        a.cmp(eax, ebx)?; // 1 < 2: CF and SF set, ZF clear
        a.cmovb(ecx, ebx)?; // taken: ECX = 2
        a.cmove(edx, ebx)?; // not taken: EDX = 4
        a.cmovl(esi, dword_ptr(DATA))?; // taken, from memory
        a.mov(edi, 0xFFFF_FFFFu32)?;
        a.cmovg(di, bx)?; // not taken: DI stays
        a.cmovge(bp, ax)?;
        a.cmovnz(bp, bx)?; // taken: BP = 2
        a.hlt()
    });
    let cpu = &rig.cpu;
    assert_eq!((cpu.ecx(), cpu.edx(), cpu.esi(), cpu.edi(), cpu.ebp() & 0xFFFF), (2, 4, 0x1234_5678, 0xFFFF_FFFF, 2));
}

#[test]
fn cmov_reads_its_memory_source_when_the_condition_fails() {
    let mut rig = p6_rig(CpuModel::PentiumII);
    rig.record(GP);
    rig.run(|a| {
        a.xor(eax, eax)?; // ZF set
        a.mov(ebx, 0x1234_5678u32)?;
        // A 16-bit data segment: the offset is past its limit.
        a.mov(cx, DATA16 as u32)?;
        a.mov(fs, cx)?;
        a.cmovnz(ebx, dword_ptr(0x2_0000).fs())?;
        a.hlt()
    });
    assert_eq!(rig.recorded().0, GP as u32);
}

#[test]
fn fcmov_and_fcomi_compare_and_move() {
    let mut rig = p6_rig(CpuModel::PentiumPro);
    rig.write32(DATA, 2.0f32.to_bits());
    rig.write32(DATA + 4, 5.0f32.to_bits());
    rig.run(|a| {
        a.fninit()?;
        a.fld(dword_ptr(DATA + 4))?; // ST1 = 5
        a.fld(dword_ptr(DATA))?; // ST0 = 2, ST1 = 5
        a.fcomi(st0, st1)?; // 2 < 5: CF set, ZF and PF clear
        a.setb(byte_ptr(DATA + 8))?;
        a.sete(byte_ptr(DATA + 9))?;
        a.setp(byte_ptr(DATA + 10))?;
        a.fcmovnb(st0, st1)?; // not taken: ST0 stays 2
        a.fst(dword_ptr(DATA + 12))?;
        a.fcmovb(st0, st1)?; // taken: ST0 = 5
        a.fst(dword_ptr(DATA + 16))?;
        a.fucomip(st0, st1)?; // 5 = 5: ZF set, pops
        a.sete(byte_ptr(DATA + 20))?;
        a.fstp(dword_ptr(DATA + 24))?; // what was ST1: 5
        a.hlt()
    });
    assert_eq!(rig.read32(DATA + 8) & 0xFF_FFFF, 0x00_0001);
    assert_eq!(f32::from_bits(rig.read32(DATA + 12)), 2.0);
    assert_eq!(f32::from_bits(rig.read32(DATA + 16)), 5.0);
    assert_eq!(rig.read32(DATA + 20) & 0xFF, 1);
    assert_eq!(f32::from_bits(rig.read32(DATA + 24)), 5.0);
}

#[test]
fn fcomi_orders_signed_zeros_and_negative_numbers() {
    let mut rig = p6_rig(CpuModel::PentiumPro);
    rig.write32(DATA, (-2.0f32).to_bits());
    rig.write32(DATA + 4, (-5.0f32).to_bits());
    rig.run(|a| {
        a.fninit()?;
        a.fldz()?;
        a.fchs()?; // ST1 = -0
        a.fldz()?; // ST0 = +0
        a.mov(eax, 0x7FFF_FFFFu32)?;
        a.add(eax, 1)?; // OF, SF and AF set
        a.fcomi(st0, st1)?; // +0 = -0: ZF set, OF, SF and AF cleared
        a.sete(byte_ptr(DATA + 8))?;
        a.seto(byte_ptr(DATA + 9))?;
        a.sets(byte_ptr(DATA + 10))?;
        a.fninit()?;
        a.fld(dword_ptr(DATA + 4))?; // ST1 = -5
        a.fld(dword_ptr(DATA))?; // ST0 = -2
        a.fcomi(st0, st1)?; // -2 > -5: CF and ZF clear
        a.setb(byte_ptr(DATA + 12))?;
        a.sete(byte_ptr(DATA + 13))?;
        a.fxch(st0, st1)?;
        a.fcomi(st0, st1)?; // -5 < -2: CF set
        a.setb(byte_ptr(DATA + 14))?;
        a.hlt()
    });
    assert_eq!(rig.read32(DATA + 8) & 0xFF_FFFF, 0x00_0001);
    assert_eq!(rig.read32(DATA + 12) & 0xFF_FFFF, 0x01_0000);
}

#[test]
fn p6_msrs() {
    let mut rig = p6_rig(CpuModel::PentiumII);
    rig.run(|a| {
        // The microcode revision and the local APIC base.
        a.mov(ecx, 0x8Bu32)?;
        a.rdmsr()?;
        a.mov(dword_ptr(DATA), eax)?;
        a.mov(dword_ptr(DATA + 4), edx)?;
        a.mov(ecx, 0x1Bu32)?;
        a.mov(eax, 0x800u32)?;
        a.wrmsr()?;
        a.rdmsr()?;
        a.mov(dword_ptr(DATA + 8), eax)?;
        // The SYSENTER registers keep what is written; CS 16 bits.
        a.mov(ecx, 0x174u32)?;
        a.mov(eax, 0xABCD_0008u32)?;
        a.wrmsr()?;
        a.rdmsr()?;
        a.mov(dword_ptr(DATA + 12), eax)?;
        a.mov(ecx, 0x176u32)?;
        a.mov(eax, 0x1234_5678u32)?;
        a.wrmsr()?;
        a.xor(eax, eax)?;
        a.rdmsr()?;
        a.mov(dword_ptr(DATA + 16), eax)?;
        a.hlt()
    });
    assert_eq!((rig.read32(DATA), rig.read32(DATA + 4)), (0x001E_03FF, 0x003F_003F));
    assert_eq!(rig.read32(DATA + 8), 0, "no local APIC");
    assert_eq!((rig.read32(DATA + 12), rig.read32(DATA + 16)), (0x0008, 0x1234_5678));
    assert_eq!(rig.cpu.sysenter, [0x0008, 0, 0x1234_5678]);
}

#[test]
fn pentium_ii_msrs_fault_on_a_pentium_pro_and_the_pentiums_on_a_p6() {
    for (model, msr, faults) in [
        (CpuModel::PentiumPro, 0x174u32, true),
        (CpuModel::PentiumPro, 0x8B, false),
        (CpuModel::PentiumPro, 0x11, true),
        (CpuModel::Pentium, 0x8B, true),
        (CpuModel::Pentium, 0x11, false),
        (CpuModel::PentiumII, 0x119, false),
    ] {
        let mut rig = p6_rig(model);
        rig.record(GP);
        rig.run(|a| {
            a.mov(dword_ptr(RESULT), 0xFFu32)?;
            a.mov(ecx, msr)?;
            a.rdmsr()?;
            a.hlt()
        });
        assert_eq!(rig.recorded().0 == GP as u32, faults, "{model:?} MSR {msr:#x}");
    }
}

/// Ring 0 code that sets up the SYSENTER MSRs (code selector CODE32, so
/// the stack is DATA32 and SYSEXIT goes to selectors 1Bh and 23h) and
/// leaves to RING3 with SYSEXIT. The ring 3 code there stores CS and SS
/// at DATA and comes back with SYSENTER to ENTRY, which stores CS, SS and
/// ESP after them and halts.
fn sysenter_program(rig: &mut Rig) {
    const ENTRY: u32 = CODE + 0x800;
    rig.ring3(|a| {
        a.mov(eax, cs)?;
        a.mov(dword_ptr(DATA), eax)?;
        a.mov(eax, ss)?;
        a.mov(dword_ptr(DATA + 4), eax)?;
        a.mov(dword_ptr(DATA + 8), esp)?;
        a.sysenter()
    });
    let entry = asm32(ENTRY, |a| {
        a.mov(eax, cs)?;
        a.mov(dword_ptr(DATA + 12), eax)?;
        a.mov(eax, ss)?;
        a.mov(dword_ptr(DATA + 16), eax)?;
        a.mov(dword_ptr(DATA + 20), esp)?;
        a.hlt()
    });
    rig.load(ENTRY, &entry);
    let code = asm32(CODE, |a| {
        a.xor(edx, edx)?;
        a.mov(ecx, 0x174u32)?;
        a.mov(eax, CODE32 as u32)?;
        a.wrmsr()?;
        a.mov(ecx, 0x175u32)?;
        a.mov(eax, STACK0_TOP - 0x40)?;
        a.wrmsr()?;
        a.mov(ecx, 0x176u32)?;
        a.mov(eax, ENTRY)?;
        a.wrmsr()?;
        a.mov(edx, RING3)?;
        a.mov(ecx, STACK3_TOP)?;
        a.sysexit()
    });
    rig.load(CODE, &code);
}

#[test]
fn sysexit_and_sysenter_switch_levels() {
    let mut rig = p6_rig(CpuModel::PentiumII);
    sysenter_program(&mut rig);
    rig.enter_pm();
    rig.run_to_halt();
    let got: Vec<u32> = (0..6).map(|i| rig.read32(DATA + 4 * i)).collect();
    assert_eq!(got, [0x1B, 0x23, STACK3_TOP, 0x08, 0x10, STACK0_TOP - 0x40]);
    assert_eq!(rig.cpu.cpl, 0);
}

#[test]
fn sysenter_with_no_code_selector_faults() {
    let mut rig = p6_rig(CpuModel::PentiumII);
    rig.record(GP);
    rig.run(|a| a.sysenter());
    assert_eq!(rig.recorded().0, GP as u32);
}

#[test]
fn sysexit_at_level_3_faults() {
    let mut rig = p6_rig(CpuModel::PentiumII);
    rig.handler(GP, 0, |a| record_code(a, GP));
    rig.ring3(|a| a.sysexit());
    let code = asm32(CODE, |a| {
        a.xor(edx, edx)?;
        a.mov(ecx, 0x174u32)?;
        a.mov(eax, CODE32 as u32)?;
        a.wrmsr()?;
        a.mov(edx, RING3)?;
        a.mov(ecx, STACK3_TOP)?;
        a.sysexit()
    });
    rig.load(CODE, &code);
    rig.enter_pm();
    rig.run_to_halt();
    assert_eq!(rig.recorded().0, GP as u32);
}

#[test]
fn the_sysenter_msrs_go_into_a_save_state() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    cpu.model = CpuModel::PentiumII;
    cpu.sysenter = [0x08, 0x7000, 0x1234];
    let state = rust_dos::savestate::machine::save(&cpu);
    cpu.sysenter = [0; 3];
    rust_dos::savestate::machine::load(&mut cpu, &state).unwrap();
    assert_eq!(cpu.sysenter, [0x08, 0x7000, 0x1234]);
    // A state from before them (without their section and the SSE and K6
    // ones after it, the last) loads, with them 0.
    let older = &state[..state.len() - (4 + 2 + 4 + 12) - (4 + 2 + 4 + 8 * 16 + 4 + 1) - (4 + 2 + 4 + 6 * 8)];
    rust_dos::savestate::machine::load(&mut cpu, older).unwrap();
    assert_eq!(cpu.sysenter, [0; 3]);
}

/// The recompiler runs the P6 instructions as the interpreter does: a
/// loop of CMOVcc, FCMOVcc, FCOMI and a ring 3 round trip through SYSEXIT
/// and SYSENTER, in lockstep.
#[test]
fn the_recompiler_runs_p6_instructions_as_the_interpreter() {
    rust_dos::hosttime::fix(chrono::NaiveDate::from_ymd_opt(1995, 4, 11).unwrap().and_hms_opt(12, 34, 56));
    let mut a = p6_rig(CpuModel::PentiumII);
    let mut b = p6_rig(CpuModel::PentiumII);
    a.cpu.core = CoreMode::Normal;
    b.cpu.core = CoreMode::Dynamic;
    for rig in [&mut a, &mut b] {
        const ENTRY: u32 = CODE + 0x800;
        rig.write32(DATA + 0x100, 3.0f32.to_bits());
        rig.ring3(|a| {
            a.add(ebp, 7u32)?;
            a.sysenter()
        });
        let entry = asm32(ENTRY, |a| {
            a.dec(edi)?;
            a.jnz((CODE + 0x100) as u64)?;
            a.hlt()
        });
        rig.load(ENTRY, &entry);
        let code = asm32(CODE, |a| {
            a.xor(edx, edx)?;
            a.mov(ecx, 0x174u32)?;
            a.mov(eax, CODE32 as u32)?;
            a.wrmsr()?;
            a.mov(ecx, 0x175u32)?;
            a.mov(eax, STACK0_TOP)?;
            a.wrmsr()?;
            a.mov(ecx, 0x176u32)?;
            a.mov(eax, ENTRY)?;
            a.wrmsr()?;
            a.mov(edi, 200u32)?;
            a.fninit()?;
            a.fldz()?;
            a.jmp((CODE + 0x100) as u64)
        });
        rig.load(CODE, &code);
        // The loop: EDI counts down; the conditions vary with it.
        let body = asm32(CODE + 0x100, |a| {
            a.mov(eax, edi)?;
            a.imul_3(eax, eax, 0x9E37_79B1u32 as i32)?;
            a.mov(ebx, edi)?;
            a.cmp(eax, 0x8000_0000u32)?;
            a.cmovb(esi, eax)?;
            a.cmovae(ebx, dword_ptr(DATA + 0x100))?;
            a.cmovg(bp, bx)?;
            a.add(esi, ebx)?;
            a.mov(dword_ptr(DATA), esi)?;
            a.fild(dword_ptr(DATA))?;
            a.fld(dword_ptr(DATA + 0x100))?;
            a.fcomi(st0, st1)?;
            a.fcmovbe(st0, st1)?;
            a.fucomip(st0, st2)?;
            a.adc(esi, 0)?;
            a.fcmovu(st0, st1)?;
            a.fstp(st1)?;
            a.mov(edx, RING3)?;
            a.mov(ecx, STACK3_TOP)?;
            a.sysexit()
        });
        rig.load(CODE + 0x100, &body);
        rig.enter_pm();
    }
    dyndiff::lockstep_with(&mut a.cpu, &mut b.cpu, 2000, 997, true, |_, _| {}).unwrap();
    assert_eq!(a.cpu.edi(), 0, "the loop ran to its end");
    if rust_dos::dynrec::AVAILABLE {
        assert!(b.cpu.dynrec.stats().runs > 0);
    }
}
