//! The K6-2: CPUID with AMD's extended leaves, its MSRs, SYSCALL and
//! SYSRET, 3DNow! with its own rules for zeros and out-of-range results,
//! what it lacks of the P6, and the recompiler running 3DNow! as the
//! interpreter does.

mod dyndiff;
mod pmrig;
mod testrunners;

use iced_x86::code_asm::*;
use pmrig::*;
use rust_dos::cpu::{CoreMode, Cpu, CpuModel, FPU_TAG_EMPTY};
use testrunners::run_cpu_code;

const UD: u8 = 6;
const NM: u8 = 7;
const GP: u8 = 13;

fn k6_rig() -> Rig {
    let mut rig = Rig::new();
    rig.cpu.model = CpuModel::K6_2;
    rig
}

/// CPUID leaf `leaf`, in real mode: EAX, EBX, ECX, EDX.
fn cpuid(cpu: &mut Cpu, leaf: u32) -> [u32; 4] {
    // 66 B8 imm32 -> MOV EAX, leaf ; 0F A2 -> CPUID
    let mut code = vec![0x66, 0xB8];
    code.extend_from_slice(&leaf.to_le_bytes());
    code.extend_from_slice(&[0x0F, 0xA2]);
    cpu.set_ip(0x100);
    run_cpu_code(cpu, &code);
    [cpu.eax(), cpu.ebx(), cpu.ecx(), cpu.edx()]
}

/// The values AMD's processor recognition note (20734, table 23) gives
/// for a K6-2 model 8 stepping 0, less the features this one lacks:
/// virtual-8086 mode extensions, debugging extensions and machine checks.
#[test]
fn cpuid_reports_a_k6_2() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    cpu.model = CpuModel::K6_2;
    assert_eq!(cpuid(&mut cpu, 0), [1, 0x6874_7541, 0x444D_4163, 0x6974_6E65], "AuthenticAMD");
    assert_eq!(cpuid(&mut cpu, 1), [0x580, 0, 0, 0x0080_0139]);
    assert_eq!(cpuid(&mut cpu, 2), [0; 4]);
    assert_eq!(cpuid(&mut cpu, 0x8000_0000), [0x8000_0005, 0, 0, 0]);
    // The standard features, SYSCALL and 3DNow!.
    assert_eq!(cpuid(&mut cpu, 0x8000_0001), [0x680, 0, 0, 0x8080_0939]);
    assert_eq!(cpuid(&mut cpu, 0x8000_0002), [0x2D44_4D41, 0x7428_364B, 0x3320_296D, 0x7270_2044]);
    assert_eq!(cpuid(&mut cpu, 0x8000_0003), [0x7365_636F, 0x0072_6F73, 0, 0]);
    assert_eq!(cpuid(&mut cpu, 0x8000_0004), [0; 4]);
    assert_eq!(cpuid(&mut cpu, 0x8000_0005), [0, 0x0280_0140, 0x2002_0220, 0x2002_0220]);
    assert_eq!(cpuid(&mut cpu, 0x8000_0006), [0; 4]);
    assert_eq!(cpu.model.family(), 5);
    assert!(cpu.model.has_mmx() && !cpu.model.is_p6() && !cpu.model.has_sse());
    // An Intel CPU has no extended leaves, so no 3DNow! to find.
    cpu.model = CpuModel::PentiumIII;
    assert_eq!(cpuid(&mut cpu, 0x8000_0000), [0; 4]);
    assert_eq!(cpuid(&mut cpu, 0x8000_0001), [0; 4]);
}

#[test]
fn k6_2_msrs() {
    let mut rig = k6_rig();
    let wrmsr = |a: &mut CodeAssembler, msr: u32, hi: u32, lo: u32| -> Result<(), IcedError> {
        a.mov(ecx, msr)?;
        a.mov(edx, hi)?;
        a.mov(eax, lo)?;
        a.wrmsr()
    };
    let rdmsr = |a: &mut CodeAssembler, msr: u32, at: u32| -> Result<(), IcedError> {
        a.mov(ecx, msr)?;
        a.rdmsr()?;
        a.mov(dword_ptr(at), eax)?;
        a.mov(dword_ptr(at + 4), edx)
    };
    rig.run(|a| {
        wrmsr(a, 0xC000_0080, 0, 1)?;
        wrmsr(a, 0xC000_0081, 0x0023_0008, 0x1234_5678)?;
        wrmsr(a, 0xC000_0082, 0xFFFF_FFFF, 0xFFFF_FFFF)?;
        wrmsr(a, 0x0E, 0, 0xFF)?;
        wrmsr(a, 0x00, 0xAAAA_AAAA, 0x5555_5555)?;
        wrmsr(a, 0x01, 0, 0xFF)?;
        rdmsr(a, 0xC000_0080, DATA)?;
        rdmsr(a, 0xC000_0081, DATA + 8)?;
        rdmsr(a, 0xC000_0082, DATA + 0x10)?;
        rdmsr(a, 0x0E, DATA + 0x18)?;
        rdmsr(a, 0x00, DATA + 0x20)?;
        rdmsr(a, 0x01, DATA + 0x28)?;
        a.hlt()
    });
    let q = |o: u32| rig.read32(DATA + o) as u64 | (rig.read32(DATA + o + 4) as u64) << 32;
    assert_eq!(q(0), 1, "EFER.SCE");
    assert_eq!(q(8), 0x0023_0008_1234_5678, "STAR");
    assert_eq!(q(0x10), 0x1FF, "WHCR's bits");
    assert_eq!(q(0x18), 0x08, "TR12's cache inhibit bit");
    assert_eq!(q(0x20), 0xAAAA_AAAA_5555_5555, "MCAR");
    assert_eq!(q(0x28), 0x1F, "MCTR");
    // A reset clears them.
    rig.cpu.reset();
    assert_eq!(rig.cpu.k6, Default::default());

    // A 1 in one of EFER's reserved bits, the Pentium's performance
    // counters and the Pentium II's SYSENTER registers raise #GP(0).
    for (msr, value) in [(0xC000_0080u32, 2u32), (0x11, 0), (0x12, 0), (0x174, 0)] {
        let mut rig = k6_rig();
        rig.record(GP);
        rig.run(|a| wrmsr(a, msr, 0, value).and_then(|_| a.hlt()));
        assert_eq!(rig.recorded().0, GP as u32, "MSR {msr:X}");
    }
    // An Intel CPU has none of the K6-2's.
    let mut rig = Rig::new();
    rig.cpu.model = CpuModel::PentiumIII;
    rig.record(GP);
    rig.run(|a| rdmsr(a, 0xC000_0080, DATA).and_then(|_| a.hlt()));
    assert_eq!(rig.recorded().0, GP as u32);
}

/// Run `code` in protected mode on `model` with CR0 bits `cr0_bits` set,
/// recording `vector`: whether it was raised.
fn raises(model: CpuModel, cr0_bits: u32, vector: u8, code: impl Fn(&mut CodeAssembler) -> Result<(), IcedError>) -> bool {
    let mut rig = Rig::new();
    rig.cpu.model = model;
    rig.record(vector);
    rig.write32(RESULT, 0xFF);
    rig.run(|a| {
        a.mov(eax, cr0)?;
        a.or(eax, cr0_bits as i32)?;
        a.mov(cr0, eax)?;
        code(a)?;
        a.hlt()
    });
    rig.recorded().0 == vector as u32
}

#[test]
fn syscall_and_sysret_need_efer_sce() {
    assert!(raises(CpuModel::K6_2, 0, UD, |a| a.syscall()));
    assert!(raises(CpuModel::K6_2, 0, UD, |a| a.sysret()));
    let enable = |a: &mut CodeAssembler| -> Result<(), IcedError> {
        a.mov(ecx, 0xC000_0080u32)?;
        a.mov(eax, 1u32)?;
        a.xor(edx, edx)?;
        a.wrmsr()
    };
    assert!(!raises(CpuModel::K6_2, 0, UD, |a| enable(a).and_then(|_| a.mov(eax, eax))));
    // An Intel CPU has neither.
    assert!(raises(CpuModel::PentiumIII, 0, UD, |a| a.syscall()));
}

/// SYSRET from level 0 to code at level 3 that runs SYSCALL back, with
/// STAR naming the standard GDT's flat segments, as AMD's note has them.
#[test]
fn syscall_and_sysret_switch_levels() {
    let mut rig = k6_rig();
    let entry = CODE + 0x200;
    rig.ring3(|a| {
        a.mov(ebx, 0x1234u32)?;
        a.syscall()?;
        a.hlt()
    });
    let handler = asm32(entry, |a| {
        a.mov(dword_ptr(DATA), ecx)?;
        a.mov(dword_ptr(DATA + 4), ebx)?;
        a.mov(ax, cs)?;
        a.mov(dword_ptr(DATA + 8), eax)?;
        a.mov(ax, ss)?;
        a.mov(dword_ptr(DATA + 0xC), eax)?;
        a.pushfd()?;
        a.pop(eax)?;
        a.mov(dword_ptr(DATA + 0x10), eax)?;
        a.hlt()
    });
    rig.load(entry, &handler);
    let star_hi = (CODE32_R3 as u32 & !3) << 16 | CODE32 as u32;
    rig.run(|a| {
        a.mov(ecx, 0xC000_0080u32)?;
        a.mov(eax, 1u32)?;
        a.xor(edx, edx)?;
        a.wrmsr()?;
        a.mov(ecx, 0xC000_0081u32)?;
        a.mov(eax, entry)?;
        a.mov(edx, star_hi)?;
        a.wrmsr()?;
        a.mov(esp, STACK3_TOP)?;
        a.mov(ecx, RING3)?;
        a.cli()?;
        a.sysret()
    });
    // SYSCALL's return address follows it: MOV EBX (5 bytes), SYSCALL (2).
    assert_eq!(rig.read32(DATA), RING3 + 7);
    assert_eq!(rig.read32(DATA + 4), 0x1234);
    assert_eq!(rig.read32(DATA + 8) & 0xFFFF, CODE32 as u32);
    assert_eq!(rig.read32(DATA + 0xC) & 0xFFFF, DATA32 as u32);
    assert_eq!(rig.read32(DATA + 0x10) & 0x200, 0, "IF cleared");
    assert_eq!(rig.cpu.cpl, 0);

    // SYSRET set IF, CPL 3 and the RPL of CS and SS, though STAR's
    // selectors have RPL 0; at level 3 it raises #GP(0).
    let mut rig = k6_rig();
    rig.record(GP);
    rig.ring3(|a| {
        a.pushfd()?;
        a.pop(eax)?;
        a.mov(dword_ptr(DATA), eax)?;
        a.mov(ax, ss)?;
        a.mov(dword_ptr(DATA + 4), eax)?;
        a.mov(ax, cs)?;
        a.mov(dword_ptr(DATA + 8), eax)?;
        a.sysret()
    });
    rig.run(|a| {
        a.mov(ecx, 0xC000_0080u32)?;
        a.mov(eax, 1u32)?;
        a.xor(edx, edx)?;
        a.wrmsr()?;
        a.mov(ecx, 0xC000_0081u32)?;
        a.xor(eax, eax)?;
        a.mov(edx, star_hi)?;
        a.wrmsr()?;
        a.mov(esp, STACK3_TOP)?;
        a.mov(ecx, RING3)?;
        a.sysret()
    });
    assert_eq!(rig.read32(DATA) & 0x200, 0x200, "IF set");
    assert_eq!(rig.read32(DATA + 4) & 0xFFFF, DATA32_R3 as u32);
    assert_eq!(rig.read32(DATA + 8) & 0xFFFF, CODE32_R3 as u32);
    assert_eq!(rig.recorded().0, GP as u32);
}

#[test]
fn the_k6_2_has_no_p6_instructions() {
    assert!(raises(CpuModel::K6_2, 0, UD, |a| a.cmove(eax, ebx)));
    assert!(raises(CpuModel::K6_2, 0, UD, |a| a.fcomi(st0, st1)));
    assert!(raises(CpuModel::K6_2, 0, UD, |a| a.sysenter()));
    assert!(raises(CpuModel::K6_2, 0, UD, |a| a.prefetchnta(byte_ptr(DATA))));
    assert!(raises(CpuModel::K6_2, 0, UD, |a| a.addps(xmm0, xmm1)));
    // 0F 1F /0 -> NOP r/m32
    assert!(raises(CpuModel::K6_2, 0, UD, |a| a.db(&[0x0F, 0x1F, 0xC0])));
}

#[test]
fn three_dnow_needs_a_k6_2_and_no_em_or_ts() {
    let pfadd = |a: &mut CodeAssembler| a.pfadd(mm0, mm1);
    assert!(!raises(CpuModel::K6_2, 0, UD, pfadd));
    assert!(raises(CpuModel::PentiumIII, 0, UD, pfadd));
    assert!(raises(CpuModel::PentiumMmx, 0, UD, |a| a.femms()));
    assert!(raises(CpuModel::K6_2, 4, UD, pfadd), "with EM");
    assert!(raises(CpuModel::K6_2, 8, NM, pfadd), "with TS");
    assert!(raises(CpuModel::K6_2, 8, NM, |a| a.femms()), "with TS");
    // The Athlon's additions: 0F 0F C1 BB -> PSWAPD MM0, MM1 ;
    // 0F 0F C1 0C -> PI2FW MM0, MM1
    assert!(raises(CpuModel::K6_2, 0, UD, |a| a.db(&[0x0F, 0x0F, 0xC1, 0xBB])));
    assert!(raises(CpuModel::K6_2, 0, UD, |a| a.db(&[0x0F, 0x0F, 0xC1, 0x0C])));
    // LOCK -> #UD
    assert!(raises(CpuModel::K6_2, 0, UD, |a| a.db(&[0xF0, 0x0F, 0x0F, 0xC1, 0x9E])));
}

#[test]
fn prefetches_never_fault() {
    // Not with EM and TS set, not at an address past the segment's limit,
    // and not with a reserved type (0F 0D /2 and /7).
    for code in [&[0x0F, 0x0D, 0x05][..], &[0x0F, 0x0D, 0x0D], &[0x0F, 0x0D, 0x15], &[0x0F, 0x0D, 0x3D]] {
        let mut prefetch = code.to_vec();
        prefetch.extend_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
        for vector in [UD, NM, GP] {
            assert!(!raises(CpuModel::K6_2, 0xC, vector, |a| a.db(&prefetch)), "{code:02X?} {vector}");
        }
        assert!(raises(CpuModel::PentiumIII, 0, UD, |a| a.db(&prefetch)), "{code:02X?}");
    }
    // The register form is undefined: 0F 0D C0
    assert!(raises(CpuModel::K6_2, 0, UD, |a| a.db(&[0x0F, 0x0D, 0xC0])));
}

fn write_q(rig: &mut Rig, addr: u32, lo: u32, hi: u32) {
    rig.write32(addr, lo);
    rig.write32(addr + 4, hi);
}

fn f(x: f32) -> u32 {
    x.to_bits()
}

#[test]
fn three_dnow_arithmetic() {
    let mut rig = k6_rig();
    let mut at = DATA;
    let mut operand = |rig: &mut Rig, lo: u32, hi: u32| {
        write_q(rig, at, lo, hi);
        at += 8;
        at - 8
    };
    type Emit = Box<dyn Fn(&mut CodeAssembler) -> Result<(), IcedError>>;
    let cases: Vec<(Emit, u32, u32)> = vec![
        // dst, src -> result (MM0 gets dst, MM1 src)
        (Box::new(|a| a.pfadd(mm0, mm1)), operand(&mut rig, f(1.5), f(-2.25)), operand(&mut rig, f(0.25), f(2.25))),
        (Box::new(|a| a.pfsub(mm0, mm1)), operand(&mut rig, f(1.0), f(-0.0)), operand(&mut rig, f(3.0), f(0.0))),
        (Box::new(|a| a.pfsubr(mm0, mm1)), operand(&mut rig, f(1.0), f(5.0)), operand(&mut rig, f(3.0), f(5.0))),
        (Box::new(|a| a.pfmul(mm0, mm1)), operand(&mut rig, f(3e38), f(1e-20)), operand(&mut rig, f(10.0), f(1e-20))),
        (Box::new(|a| a.pfacc(mm0, mm1)), operand(&mut rig, f(1.0), f(2.0)), operand(&mut rig, f(3.0), f(4.0))),
        (Box::new(|a| a.pfmax(mm0, mm1)), operand(&mut rig, f(-1.0), f(5.0)), operand(&mut rig, f(-0.0), f(2.0))),
        (Box::new(|a| a.pfmin(mm0, mm1)), operand(&mut rig, f(1.0), f(-5.0)), operand(&mut rig, f(0.0), f(-2.0))),
        (Box::new(|a| a.pfcmpge(mm0, mm1)), operand(&mut rig, f(1.0), f(-0.0)), operand(&mut rig, f(1.0), f(0.0))),
        (Box::new(|a| a.pfcmpgt(mm0, mm1)), operand(&mut rig, f(1.0), f(2.0)), operand(&mut rig, f(1.0), f(1.0))),
        (Box::new(|a| a.pfcmpeq(mm0, mm1)), operand(&mut rig, 0x0000_0001, f(2.0)), operand(&mut rig, 0x8000_0000, f(2.0))),
        (Box::new(|a| a.pi2fd(mm0, mm1)), operand(&mut rig, 0, 0), operand(&mut rig, 16_777_217, -5i32 as u32)),
        (Box::new(|a| a.pf2id(mm0, mm1)), operand(&mut rig, 0, 0), operand(&mut rig, f(-2.75), f(3e9))),
        (Box::new(|a| a.pfrcp(mm0, mm1)), operand(&mut rig, 0, 0), operand(&mut rig, f(4.0), f(3.0))),
        (Box::new(|a| a.pfrsqrt(mm0, mm1)), operand(&mut rig, 0, 0), operand(&mut rig, f(-16.0), f(3.0))),
        // The manual's examples.
        (Box::new(|a| a.pavgusb(mm0, mm1)), operand(&mut rig, 0x0144_F7A8, 0xFF00_FF10), operand(&mut rig, 0x0070_079A, 0xFFFF_010F)),
        (Box::new(|a| a.pmulhrw(mm0, mm1)), operand(&mut rig, 0x7FFE_FFFF, 0x8807_EC22), operand(&mut rig, 0x7007_FFFF, 0xD250_5321)),
    ];
    let want: [(u32, u32); 16] = [
        // -2.25 + 2.25 cancels to a zero with the first source's sign.
        (f(1.75), 0x8000_0000),
        (f(-2.0), 0x8000_0000),
        // 5 - 5 cancels to a zero with the minuend's sign.
        (f(2.0), 0),
        // Saturated, and a product below 2^-126 a zero.
        (0x7F7F_FFFF, 0),
        (f(3.0), f(7.0)),
        // A zero and a negative number give +0.
        (0, f(5.0)),
        (0, f(-5.0)),
        // +0 is -0.
        (u32::MAX, u32::MAX),
        (0, u32::MAX),
        // An exponent of 0 is a zero.
        (u32::MAX, u32::MAX),
        // Truncated.
        (f(16_777_216.0), f(-5.0)),
        (-2i32 as u32, 0x7FFF_FFFF),
        // Of the low number, in both halves.
        (f(0.25), f(0.25)),
        (f(-0.25), f(-0.25)),
        (0x015A_7FA1, 0xFF80_8010),
        (0x3803_0000, 0x1569_F98C),
    ];
    let ops: Vec<_> = cases.iter().map(|(_, d, s)| (*d, *s)).collect();
    rig.run(|a| {
        for (i, (op, d, s)) in cases.iter().enumerate() {
            a.movq(mm0, qword_ptr(*d))?;
            a.movq(mm1, qword_ptr(*s))?;
            op(a)?;
            a.movq(qword_ptr(DATA + 0x800 + 8 * i as u32), mm0)?;
        }
        a.femms()?;
        a.hlt()
    });
    for (i, &(lo, hi)) in want.iter().enumerate() {
        let at = DATA + 0x800 + 8 * i as u32;
        assert_eq!((rig.read32(at), rig.read32(at + 4)), (lo, hi), "case {i}: {:08X?}", ops[i]);
    }
    assert_eq!(rig.cpu.fpu_tags, [FPU_TAG_EMPTY; 8], "FEMMS empties the tags");
}

/// The manual's division: a/b to 24 bits with PFRCP, PFRCPIT1, PFRCPIT2
/// and PFMUL; and its reciprocal square root with PFRSQRT, PFMUL, PFRSQIT1
/// and PFRCPIT2.
#[test]
fn the_manuals_division_and_square_root() {
    let mut rig = k6_rig();
    write_q(&mut rig, DATA, f(10.0), f(1.0));
    write_q(&mut rig, DATA + 8, f(3.0), f(7.0));
    rig.run(|a| {
        a.movq(mm0, qword_ptr(DATA + 8))?;
        // b's low number, then its high one.
        a.pfrcp(mm1, mm0)?;
        a.movq(mm4, mm0)?;
        a.punpckhdq(mm4, mm4)?;
        a.pfrcp(mm2, mm4)?;
        a.punpckldq(mm1, mm2)?;
        a.movq(mm2, mm0)?;
        a.pfrcpit1(mm2, mm1)?;
        a.pfrcpit2(mm2, mm1)?;
        a.movq(mm3, qword_ptr(DATA))?;
        a.pfmul(mm3, mm2)?;
        a.movq(qword_ptr(DATA + 0x10), mm3)?;
        // 1/sqrt(3)
        a.pfrsqrt(mm1, mm0)?;
        a.movq(mm2, mm1)?;
        a.pfmul(mm1, mm1)?;
        a.pfrsqit1(mm1, mm0)?;
        a.pfrcpit2(mm1, mm2)?;
        a.movq(qword_ptr(DATA + 0x18), mm1)?;
        a.emms()?;
        a.hlt()
    });
    let got = |at| f32::from_bits(rig.read32(at));
    let close = |x: f32, want: f32| (x - want).abs() <= want.abs() * 2e-7;
    assert!(close(got(DATA + 0x10), 10.0 / 3.0), "{}", got(DATA + 0x10));
    assert!(close(got(DATA + 0x14), 1.0 / 7.0), "{}", got(DATA + 0x14));
    assert!(close(got(DATA + 0x18), 1.0 / 3f32.sqrt()), "{}", got(DATA + 0x18));
}

#[test]
fn the_k6_2_msrs_go_into_a_save_state() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    cpu.model = CpuModel::K6_2;
    cpu.k6.efer = 1;
    cpu.k6.star = 0x0023_0008_1234_5678;
    cpu.k6.whcr = 0x1FE;
    let saved = cpu.k6;
    let state = rust_dos::savestate::machine::save(&cpu);
    cpu.k6 = Default::default();
    rust_dos::savestate::machine::load(&mut cpu, &state).unwrap();
    assert_eq!(cpu.k6, saved);
    // A state from before it (without its section and the IDLE one after
    // it, the last) loads, with the MSRs as after a reset.
    let older = &state[..state.len() - (4 + 2 + 4 + 6 * 8) - (4 + 2 + 4 + 1)];
    rust_dos::savestate::machine::load(&mut cpu, older).unwrap();
    assert_eq!(cpu.k6, Default::default());
}

/// The recompiler runs 3DNow! as the interpreter does: a loop of packed
/// arithmetic, conversions and the byte and word instructions, in lockstep.
#[test]
fn the_recompiler_runs_3dnow_as_the_interpreter() {
    rust_dos::hosttime::fix(chrono::NaiveDate::from_ymd_opt(1999, 2, 26).unwrap().and_hms_opt(12, 34, 56));
    let mut a = k6_rig();
    let mut b = k6_rig();
    a.cpu.core = CoreMode::Normal;
    b.cpu.core = CoreMode::Dynamic;
    for rig in [&mut a, &mut b] {
        write_q(rig, DATA + 0x100, f(1.5), f(-2.25));
        let code = asm32(CODE, |a| {
            a.mov(edi, 300u32)?;
            a.movq(mm1, qword_ptr(DATA + 0x100))?;
            a.jmp((CODE + 0x100) as u64)
        });
        rig.load(CODE, &code);
        let body = asm32(CODE + 0x100, |a| {
            a.movd(mm0, edi)?;
            a.punpckldq(mm0, mm0)?;
            a.pi2fd(mm0, mm0)?;
            a.pfmul(mm0, mm1)?;
            a.pfrsqrt(mm2, mm0)?;
            a.pfadd(mm1, mm2)?;
            a.pfacc(mm2, mm1)?;
            a.pfmax(mm1, mm2)?;
            a.pfcmpgt(mm2, mm0)?;
            a.pf2id(mm3, mm1)?;
            a.pavgusb(mm3, mm2)?;
            a.pmulhrw(mm3, mm1)?;
            a.movd(eax, mm3)?;
            a.add(esi, eax)?;
            a.movq(qword_ptr(DATA), mm1)?;
            a.femms()?;
            a.dec(edi)?;
            a.jnz((CODE + 0x100) as u64)?;
            a.hlt()
        });
        rig.load(CODE + 0x100, &body);
        rig.enter_pm();
    }
    dyndiff::lockstep_with(&mut a.cpu, &mut b.cpu, 2000, 997, true, |_, _| {}).unwrap();
    assert_eq!(a.cpu.edi(), 0, "the loop ran to its end");
    assert_eq!(a.cpu.esi(), b.cpu.esi());
    if rust_dos::dynrec::AVAILABLE {
        assert!(b.cpu.dynrec.stats().runs > 0);
    }
}
