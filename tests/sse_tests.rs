//! The Pentium III: CPUID and its MSRs, SSE on XMM registers with MXCSR's
//! rounding and exceptions, the SSE integer instructions on MMX registers,
//! FXSAVE and FXRSTOR, what earlier processors and CR4 do with them, and
//! the recompiler running them as the interpreter does.

mod dyndiff;
mod pmrig;
mod testrunners;

use iced_x86::code_asm::*;
use pmrig::*;
use rust_dos::cpu::{CoreMode, Cpu, CpuModel};
use testrunners::run_cpu_code;

const UD: u8 = 6;
const NM: u8 = 7;
const GP: u8 = 13;
const XM: u8 = 19;

/// CR4's OSFXSR and OSXMMEXCPT.
const OSFXSR: u32 = 0x200;
const OSXMMEXCPT: u32 = 0x400;

fn p3_rig() -> Rig {
    let mut rig = Rig::new();
    rig.cpu.model = CpuModel::PentiumIII;
    rig
}

/// Set CR4 bits `bits`.
fn set_cr4(a: &mut CodeAssembler, bits: u32) -> Result<(), IcedError> {
    a.mov(eax, cr4)?;
    a.or(eax, bits as i32)?;
    a.mov(cr4, eax)
}

fn write_lanes(rig: &mut Rig, addr: u32, lanes: [u32; 4]) {
    for (i, v) in lanes.into_iter().enumerate() {
        rig.write32(addr + 4 * i as u32, v);
    }
}

fn write_ps(rig: &mut Rig, addr: u32, ps: [f32; 4]) {
    write_lanes(rig, addr, ps.map(f32::to_bits));
}

fn read_lanes(rig: &Rig, addr: u32) -> [u32; 4] {
    [0, 1, 2, 3].map(|i| rig.read32(addr + 4 * i))
}

fn read_ps(rig: &Rig, addr: u32) -> [f32; 4] {
    read_lanes(rig, addr).map(f32::from_bits)
}

/// CPUID leaf `leaf` on `model`, in real mode: EAX, EBX, ECX, EDX.
fn cpuid(cpu: &mut Cpu, leaf: u32) -> [u32; 4] {
    // 66 B8 imm32 -> MOV EAX, leaf ; 0F A2 -> CPUID
    let mut code = vec![0x66, 0xB8];
    code.extend_from_slice(&leaf.to_le_bytes());
    code.extend_from_slice(&[0x0F, 0xA2]);
    cpu.set_ip(0x100);
    run_cpu_code(cpu, &code);
    [cpu.eax(), cpu.ebx(), cpu.ecx(), cpu.edx()]
}

#[test]
fn cpuid_reports_a_pentium_iii() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    cpu.model = CpuModel::PentiumIII;
    assert_eq!(cpuid(&mut cpu, 0)[0], 3, "up to the serial number's leaf");
    // The Pentium II's features, and FXSR, SSE and the serial number.
    assert_eq!(cpuid(&mut cpu, 1), [0x673, 0, 0, 0x0384_8939]);
    assert_eq!(cpuid(&mut cpu, 2), [0x0302_0101, 0, 0, 0x0C04_0843]);
    assert_eq!(cpuid(&mut cpu, 3)[2..], [0x534F_442D, 0x5453_5552]);
    assert_eq!(cpuid(&mut cpu, 4), [0; 4]);
    assert_eq!(CpuModel::PentiumIII.family(), 6);
    assert!(CpuModel::PentiumIII.has_mmx() && CpuModel::PentiumIII.is_p6());
}

#[test]
fn switching_the_serial_number_off_takes_its_leaf_away() {
    let mut rig = p3_rig();
    rig.run(|a| {
        a.mov(ecx, 0x119u32)?;
        a.rdmsr()?;
        a.mov(dword_ptr(DATA), eax)?;
        a.or(eax, 0x20_0000)?;
        a.wrmsr()?;
        a.rdmsr()?;
        a.mov(dword_ptr(DATA + 4), eax)?;
        a.xor(eax, eax)?;
        a.cpuid()?;
        a.mov(dword_ptr(DATA + 8), eax)?;
        a.mov(eax, 1u32)?;
        a.cpuid()?;
        a.mov(dword_ptr(DATA + 12), edx)?;
        a.mov(eax, 3u32)?;
        a.cpuid()?;
        a.mov(dword_ptr(DATA + 16), ecx)?;
        a.hlt()
    });
    assert_eq!((rig.read32(DATA), rig.read32(DATA + 4)), (0, 0x20_0000));
    assert_eq!(rig.read32(DATA + 8), 2);
    assert_eq!(rig.read32(DATA + 12) & 0x4_0000, 0);
    assert_eq!(rig.read32(DATA + 16), 0);
    // Only a reset brings it back.
    rig.cpu.reset();
    assert!(!rig.cpu.sse.psn_off);
}

#[test]
fn pentium_iii_msrs() {
    let mut rig = p3_rig();
    rig.run(|a| {
        a.mov(ecx, 0x8Bu32)?;
        a.rdmsr()?;
        a.mov(dword_ptr(DATA), edx)?;
        a.mov(ecx, 0x17u32)?;
        a.rdmsr()?;
        a.mov(dword_ptr(DATA + 4), edx)?;
        for msr in [0x140u32, 0x186, 0x187] {
            a.mov(ecx, msr)?;
            a.wrmsr()?;
            a.rdmsr()?;
        }
        a.mov(dword_ptr(DATA + 8), eax)?;
        a.hlt()
    });
    assert_eq!((rig.read32(DATA), rig.read32(DATA + 4), rig.read32(DATA + 8)), (0xE, 0x1100_0000, 0));
    // The platform ID can't be written, and a Pentium II has none.
    for (model, write) in [(CpuModel::PentiumIII, true), (CpuModel::PentiumII, false)] {
        let mut rig = Rig::new();
        rig.cpu.model = model;
        rig.record(GP);
        rig.run(|a| {
            a.mov(ecx, 0x17u32)?;
            if write { a.wrmsr()? } else { a.rdmsr()? }
            a.hlt()
        });
        assert_eq!(rig.recorded().0, GP as u32, "{model:?}");
    }
}

#[test]
fn cr4_takes_the_sse_bits_only_on_a_pentium_iii() {
    let mut rig = p3_rig();
    rig.run(|a| {
        set_cr4(a, OSFXSR | OSXMMEXCPT)?;
        a.hlt()
    });
    assert_eq!(rig.cpu.cr4, OSFXSR | OSXMMEXCPT);
    let mut rig = Rig::new();
    rig.cpu.model = CpuModel::PentiumII;
    rig.record(GP);
    rig.run(|a| {
        set_cr4(a, OSFXSR)?;
        a.hlt()
    });
    assert_eq!(rig.recorded().0, GP as u32);
}

/// Run `code` in protected mode on `model` with CR4 bits `cr4_bits` and
/// CR0 bits `cr0_bits` set, recording `vector`: whether it was raised.
fn raises(
    model: CpuModel,
    cr4_bits: u32,
    cr0_bits: u32,
    vector: u8,
    code: impl Fn(&mut CodeAssembler) -> Result<(), IcedError>,
) -> bool {
    let mut rig = Rig::new();
    rig.cpu.model = model;
    rig.record(vector);
    rig.write32(RESULT, 0xFF);
    rig.run(|a| {
        if cr4_bits != 0 {
            set_cr4(a, cr4_bits)?;
        }
        a.mov(eax, cr0)?;
        a.or(eax, cr0_bits as i32)?;
        a.mov(cr0, eax)?;
        code(a)?;
        a.hlt()
    });
    rig.recorded().0 == vector as u32
}

#[test]
fn sse_needs_a_pentium_iii_osfxsr_and_no_em_or_ts() {
    let addps = |a: &mut CodeAssembler| a.addps(xmm0, xmm1);
    assert!(raises(CpuModel::PentiumII, 0, 0, UD, addps));
    assert!(raises(CpuModel::PentiumIII, 0, 0, UD, addps), "without OSFXSR");
    assert!(!raises(CpuModel::PentiumIII, OSFXSR, 0, UD, addps));
    assert!(raises(CpuModel::PentiumIII, OSFXSR, 4, UD, addps), "with EM");
    assert!(raises(CpuModel::PentiumIII, OSFXSR, 8, NM, addps), "with TS");
    // The integer instructions on MMX registers are MMX instructions:
    // CR4 doesn't matter.
    let pavgb = |a: &mut CodeAssembler| a.pavgb(mm0, mm1);
    assert!(!raises(CpuModel::PentiumIII, 0, 0, UD, pavgb));
    assert!(raises(CpuModel::PentiumII, 0, 0, UD, pavgb));
    assert!(raises(CpuModel::PentiumIII, 0, 8, NM, pavgb));
    // FXSAVE needs neither, but raises #NM for EM too.
    let fxsave = |a: &mut CodeAssembler| a.fxsave(ptr(DATA));
    assert!(!raises(CpuModel::PentiumIII, 0, 0, UD, fxsave));
    assert!(raises(CpuModel::PentiumII, 0, 0, UD, fxsave));
    assert!(raises(CpuModel::PentiumIII, 0, 4, NM, fxsave));
    // SFENCE, and the prefetches, which a Pentium II runs as hinting NOPs.
    assert!(!raises(CpuModel::PentiumIII, 0, 0, UD, |a| a.sfence()));
    assert!(raises(CpuModel::PentiumII, 0, 0, UD, |a| a.sfence()));
    assert!(!raises(CpuModel::PentiumIII, 0, 0, UD, |a| a.prefetchnta(byte_ptr(0xFFFF_FFF0u32))));
}

#[test]
fn sse2_is_undefined_on_a_pentium_iii() {
    // 66 0F 58 C1 -> ADDPD ; F2 0F 10 C1 -> MOVSD XMM0, XMM1 ;
    // F2 0F C2 C1 00 -> CMPSD XMM0, XMM1, 0 ; 66 0F FC C1 -> PADDB XMM0, XMM1 ;
    // 66 0F E0 C1 -> PAVGB XMM0, XMM1
    for code in [
        &[0x66, 0x0F, 0x58, 0xC1][..],
        &[0xF2, 0x0F, 0x10, 0xC1],
        &[0xF2, 0x0F, 0xC2, 0xC1, 0x00],
        &[0x66, 0x0F, 0xFC, 0xC1],
        &[0x66, 0x0F, 0xE0, 0xC1],
    ] {
        assert!(raises(CpuModel::PentiumIII, OSFXSR, 0, UD, |a| a.db(code)), "{code:02X?}");
    }
}

#[test]
fn packed_and_scalar_arithmetic() {
    let mut rig = p3_rig();
    write_ps(&mut rig, DATA, [1.0, 2.0, 3.0, 4.0]);
    write_ps(&mut rig, DATA + 0x10, [0.5, -4.0, 8.0, 16.0]);
    rig.run(|a| {
        set_cr4(a, OSFXSR)?;
        a.movaps(xmm0, xmmword_ptr(DATA))?;
        a.movaps(xmm1, xmm0)?;
        a.addps(xmm1, xmmword_ptr(DATA + 0x10))?;
        a.movaps(xmmword_ptr(DATA + 0x20), xmm1)?;
        a.movaps(xmm2, xmm0)?;
        a.mulps(xmm2, xmmword_ptr(DATA + 0x10))?;
        a.movups(xmmword_ptr(DATA + 0x31), xmm2)?;
        a.movaps(xmm3, xmmword_ptr(DATA + 0x10))?;
        a.divps(xmm3, xmm0)?;
        a.movaps(xmmword_ptr(DATA + 0x50), xmm3)?;
        a.sqrtps(xmm4, xmmword_ptr(DATA))?;
        a.movaps(xmmword_ptr(DATA + 0x60), xmm4)?;
        // Scalar: only the low single changes.
        a.movaps(xmm5, xmm0)?;
        a.subss(xmm5, dword_ptr(DATA + 0x10))?;
        a.maxss(xmm5, xmm3)?;
        a.movaps(xmmword_ptr(DATA + 0x70), xmm5)?;
        a.movaps(xmm6, xmm0)?;
        a.minps(xmm6, xmmword_ptr(DATA + 0x10))?;
        a.movaps(xmmword_ptr(DATA + 0x80), xmm6)?;
        a.hlt()
    });
    assert_eq!(read_ps(&rig, DATA + 0x20), [1.5, -2.0, 11.0, 20.0]);
    let mut product = [0u8; 16];
    for (i, b) in product.iter_mut().enumerate() {
        *b = rig.cpu.bus.read_8((DATA + 0x31) as usize + i);
    }
    let product: Vec<f32> = product.chunks(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
    assert_eq!(product, [0.5, -8.0, 24.0, 64.0]);
    assert_eq!(read_ps(&rig, DATA + 0x50), [0.5, -2.0, 8.0 / 3.0, 4.0]);
    assert_eq!(read_ps(&rig, DATA + 0x60), [1.0, 2f32.sqrt(), 3f32.sqrt(), 2.0]);
    assert_eq!(read_ps(&rig, DATA + 0x70), [0.5, 2.0, 3.0, 4.0]);
    assert_eq!(read_ps(&rig, DATA + 0x80), [0.5, -4.0, 3.0, 4.0]);
    assert_eq!(rig.cpu.sse.mxcsr, 0x1F80 | 0x20, "8/3 and the square roots are inexact");
}

#[test]
fn a_misaligned_packed_operand_faults() {
    type Code<'a> = &'a dyn Fn(&mut CodeAssembler) -> Result<(), IcedError>;
    let cases: [(&str, Code, bool); 7] = [
        ("movaps", &|a| a.movaps(xmm0, xmmword_ptr(DATA + 8)), true),
        ("addps", &|a| a.addps(xmm0, xmmword_ptr(DATA + 4)), true),
        ("movntps", &|a| a.movntps(xmmword_ptr(DATA + 1), xmm0), true),
        ("movups", &|a| a.movups(xmm0, xmmword_ptr(DATA + 8)), false),
        ("addss", &|a| a.addss(xmm0, dword_ptr(DATA + 1)), false),
        ("movlps", &|a| a.movlps(xmm0, qword_ptr(DATA + 4)), false),
        ("fxsave", &|a| a.fxsave(ptr(DATA + 8)), true),
    ];
    for (name, code, faults) in cases {
        assert_eq!(raises(CpuModel::PentiumIII, OSFXSR, 0, GP, code), faults, "{name}");
    }
}

#[test]
fn moves_shuffles_and_logic() {
    let mut rig = p3_rig();
    write_lanes(&mut rig, DATA, [0x11, 0x22, 0x33, 0x44]);
    write_lanes(&mut rig, DATA + 0x10, [0x8000_0055, 0x66, 0x8000_0077, 0x88]);
    rig.run(|a| {
        set_cr4(a, OSFXSR)?;
        a.movaps(xmm0, xmmword_ptr(DATA))?;
        a.movaps(xmm1, xmmword_ptr(DATA + 0x10))?;
        a.movaps(xmm2, xmm0)?;
        a.unpcklps(xmm2, xmm1)?;
        a.movaps(xmmword_ptr(DATA + 0x20), xmm2)?;
        a.movaps(xmm2, xmm0)?;
        a.unpckhps(xmm2, xmm1)?;
        a.movaps(xmmword_ptr(DATA + 0x30), xmm2)?;
        a.movaps(xmm2, xmm0)?;
        a.shufps(xmm2, xmm1, 0x1B)?;
        a.movaps(xmmword_ptr(DATA + 0x40), xmm2)?;
        a.movaps(xmm2, xmm0)?;
        a.movhlps(xmm2, xmm1)?;
        a.movaps(xmmword_ptr(DATA + 0x50), xmm2)?;
        a.movaps(xmm2, xmm0)?;
        a.movlhps(xmm2, xmm1)?;
        a.movhps(qword_ptr(DATA + 0x60), xmm2)?;
        a.movlps(qword_ptr(DATA + 0x68), xmm2)?;
        a.movaps(xmm2, xmm0)?;
        a.movhps(xmm2, qword_ptr(DATA + 0x10))?;
        a.movaps(xmmword_ptr(DATA + 0x70), xmm2)?;
        // MOVSS from memory clears the rest; between registers keeps it.
        a.movss(xmm3, dword_ptr(DATA + 4))?;
        a.movaps(xmmword_ptr(DATA + 0x80), xmm3)?;
        a.movaps(xmm3, xmm1)?;
        a.movss(xmm3, xmm0)?;
        a.movaps(xmmword_ptr(DATA + 0x90), xmm3)?;
        a.movss(dword_ptr(DATA + 0xA0), xmm1)?;
        a.movmskps(eax, xmm1)?;
        a.mov(dword_ptr(DATA + 0xA4), eax)?;
        a.movaps(xmm4, xmm1)?;
        a.andps(xmm4, xmm0)?;
        a.movaps(xmmword_ptr(DATA + 0xB0), xmm4)?;
        a.movaps(xmm4, xmm0)?;
        a.andnps(xmm4, xmm1)?;
        a.orps(xmm4, xmm0)?;
        a.xorps(xmm4, xmmword_ptr(DATA))?;
        a.movaps(xmmword_ptr(DATA + 0xC0), xmm4)?;
        a.hlt()
    });
    assert_eq!(read_lanes(&rig, DATA + 0x20), [0x11, 0x8000_0055, 0x22, 0x66]);
    assert_eq!(read_lanes(&rig, DATA + 0x30), [0x33, 0x8000_0077, 0x44, 0x88]);
    assert_eq!(read_lanes(&rig, DATA + 0x40), [0x44, 0x33, 0x66, 0x8000_0055]);
    assert_eq!(read_lanes(&rig, DATA + 0x50), [0x8000_0077, 0x88, 0x33, 0x44]);
    assert_eq!(read_lanes(&rig, DATA + 0x60), [0x8000_0055, 0x66, 0x11, 0x22]);
    assert_eq!(read_lanes(&rig, DATA + 0x70), [0x11, 0x22, 0x8000_0055, 0x66]);
    assert_eq!(read_lanes(&rig, DATA + 0x80), [0x22, 0, 0, 0]);
    assert_eq!(read_lanes(&rig, DATA + 0x90), [0x11, 0x66, 0x8000_0077, 0x88]);
    assert_eq!((rig.read32(DATA + 0xA0), rig.read32(DATA + 0xA4)), (0x8000_0055, 0b0101));
    assert_eq!(read_lanes(&rig, DATA + 0xB0), [0x11, 0x22, 0x33, 0x00]);
    // (!a & b | a) ^ a = b & !a
    assert_eq!(read_lanes(&rig, DATA + 0xC0), [0x8000_0044, 0x44, 0x8000_0044, 0x88]);
}

#[test]
fn compares_and_conversions() {
    let mut rig = p3_rig();
    write_ps(&mut rig, DATA, [1.0, 2.0, f32::NAN, -3.5]);
    write_ps(&mut rig, DATA + 0x10, [1.0, 1.0, 0.0, 2.5]);
    rig.write32(DATA + 0x20, 7.5f32.to_bits());
    rig.write32(DATA + 0x24, (-7i32) as u32);
    rig.run(|a| {
        set_cr4(a, OSFXSR)?;
        a.movaps(xmm0, xmmword_ptr(DATA))?;
        a.movaps(xmm1, xmm0)?;
        a.cmpps(xmm1, xmmword_ptr(DATA + 0x10), 2)?; // LE
        a.movaps(xmmword_ptr(DATA + 0x30), xmm1)?;
        a.movaps(xmm1, xmm0)?;
        a.cmpps(xmm1, xmmword_ptr(DATA + 0x10), 4)?; // NEQ
        a.movaps(xmmword_ptr(DATA + 0x40), xmm1)?;
        // COMISS: 1 = 1, then 2 > 1.
        a.movaps(xmm2, xmmword_ptr(DATA + 0x10))?;
        a.comiss(xmm0, xmm2)?;
        a.sete(byte_ptr(DATA + 0x50))?;
        a.shufps(xmm0, xmm0, 0xE5)?; // low single 2.0
        a.ucomiss(xmm0, xmm2)?;
        a.seta(byte_ptr(DATA + 0x51))?;
        a.setp(byte_ptr(DATA + 0x52))?;
        // CVTSS2SI rounds to even; CVTTSS2SI truncates.
        a.cvtss2si(eax, dword_ptr(DATA + 0x20))?;
        a.mov(dword_ptr(DATA + 0x54), eax)?;
        a.movss(xmm3, dword_ptr(DATA + 0x20))?;
        a.cvttss2si(eax, xmm3)?;
        a.mov(dword_ptr(DATA + 0x58), eax)?;
        a.cvtsi2ss(xmm4, dword_ptr(DATA + 0x24))?;
        a.movss(dword_ptr(DATA + 0x5C), xmm4)?;
        // CVTPI2PS and CVTPS2PI through MM0.
        a.movq(mm0, qword_ptr(DATA + 0x20))?;
        a.cvtpi2ps(xmm5, mm0)?;
        a.movaps(xmmword_ptr(DATA + 0x60), xmm5)?;
        a.movaps(xmm5, xmmword_ptr(DATA + 0x10))?;
        a.cvtps2pi(mm1, xmm5)?;
        a.movq(qword_ptr(DATA + 0x70), mm1)?;
        a.cvttps2pi(mm2, qword_ptr(DATA + 0x18))?;
        a.movq(qword_ptr(DATA + 0x78), mm2)?;
        a.emms()?;
        a.hlt()
    });
    assert_eq!(read_lanes(&rig, DATA + 0x30), [!0, 0, 0, !0]);
    assert_eq!(read_lanes(&rig, DATA + 0x40), [0, !0, !0, !0]);
    assert_eq!([0x50, 0x51, 0x52].map(|o| rig.read32(DATA + o) & 0xFF), [1, 1, 0]);
    assert_eq!((rig.read32(DATA + 0x54), rig.read32(DATA + 0x58)), (8, 7));
    assert_eq!(f32::from_bits(rig.read32(DATA + 0x5C)), -7.0);
    // The integers 7.5's bits make, and -7.
    assert_eq!(read_ps(&rig, DATA + 0x60)[..2], [7.5f32.to_bits() as i32 as f32, -7.0]);
    assert_eq!((rig.read32(DATA + 0x70), rig.read32(DATA + 0x74)), (1, 1));
    assert_eq!((rig.read32(DATA + 0x78), rig.read32(DATA + 0x7C)), (0, 2));
    // The NaN: LE raised the invalid flag, NEQ alone wouldn't have.
    assert_eq!(rig.cpu.sse.mxcsr & 0x3F, 0x01 | 0x20);
}

#[test]
fn mxcsr_rounds_and_masks() {
    let mut rig = p3_rig();
    rig.write32(DATA, 0x1F80 | 0x2000); // round down
    rig.write32(DATA + 4, 1.0f32.to_bits());
    rig.write32(DATA + 8, 3.0f32.to_bits());
    rig.run(|a| {
        set_cr4(a, OSFXSR)?;
        a.ldmxcsr(dword_ptr(DATA))?;
        a.movss(xmm0, dword_ptr(DATA + 4))?;
        a.divss(xmm0, dword_ptr(DATA + 8))?;
        a.movss(dword_ptr(DATA + 0x10), xmm0)?;
        a.movss(xmm1, dword_ptr(DATA + 4))?;
        a.xorps(xmm2, xmm2)?;
        a.divss(xmm1, xmm2)?;
        a.movss(dword_ptr(DATA + 0x14), xmm1)?;
        a.stmxcsr(dword_ptr(DATA + 0x18))?;
        a.hlt()
    });
    // 1/3 rounded down is the single below the nearest one.
    assert_eq!(rig.read32(DATA + 0x10), (1.0f32 / 3.0).to_bits() - 1);
    assert_eq!(f32::from_bits(rig.read32(DATA + 0x14)), f32::INFINITY);
    assert_eq!(rig.read32(DATA + 0x18), 0x1F80 | 0x2000 | 0x20 | 0x04);
}

#[test]
fn ldmxcsr_refuses_reserved_bits() {
    // Bit 6, the denormals-are-zero of later processors, and bit 16.
    for value in [0x1FC0u32, 0x1_1F80] {
        let mut rig = p3_rig();
        rig.record(GP);
        rig.write32(DATA, value);
        rig.run(|a| {
            set_cr4(a, OSFXSR)?;
            a.ldmxcsr(dword_ptr(DATA))?;
            a.hlt()
        });
        assert_eq!(rig.recorded().0, GP as u32, "{value:#x}");
        assert_eq!(rig.cpu.sse.mxcsr, 0x1F80);
    }
}

/// Divide 1 by 0 with ZE unmasked and CR4 bits `bits`, recording
/// `vector`: XMM0 after it, and MXCSR.
fn unmasked_divide(bits: u32, vector: u8) -> (Rig, [u32; 4], u32) {
    let mut rig = p3_rig();
    rig.record(vector);
    write_ps(&mut rig, DATA, [1.0, 1.0, 1.0, 1.0]);
    write_ps(&mut rig, DATA + 0x10, [3.0, 0.0, 4.0, 8.0]);
    rig.write32(DATA + 0x20, 0x1F80 & !0x200);
    rig.run(|a| {
        set_cr4(a, bits)?;
        a.ldmxcsr(dword_ptr(DATA + 0x20))?;
        a.movaps(xmm0, xmmword_ptr(DATA))?;
        a.divps(xmm0, xmmword_ptr(DATA + 0x10))?;
        a.hlt()
    });
    let dst = rig.cpu.sse.xmm[0];
    let lanes = [0, 1, 2, 3].map(|i| (dst >> (32 * i)) as u32);
    let mxcsr = rig.cpu.sse.mxcsr;
    (rig, lanes, mxcsr)
}

#[test]
fn an_unmasked_exception_raises_xm_and_leaves_the_destination() {
    let (rig, dst, mxcsr) = unmasked_divide(OSFXSR | OSXMMEXCPT, XM);
    assert_eq!(rig.recorded().0, XM as u32);
    assert_eq!(dst, [1.0f32.to_bits(); 4]);
    // The divide by zero, found before computing: 1/3's inexact result
    // doesn't count.
    assert_eq!(mxcsr & 0x3F, 0x04);
    // Without OSXMMEXCPT, #UD.
    let (rig, _, _) = unmasked_divide(OSFXSR, UD);
    assert_eq!(rig.recorded().0, UD as u32);
}

#[test]
fn sse_integer_instructions_on_mmx_registers() {
    let mut rig = p3_rig();
    rig.write32(DATA, 0x0403_02FF);
    rig.write32(DATA + 4, 0x8070_0605);
    rig.write32(DATA + 8, 0x0102_0301);
    rig.write32(DATA + 12, 0x0170_FF05);
    rig.write32(DATA + 0x20, 0xFFFF_FFFF);
    rig.write32(DATA + 0x24, 0xFFFF_FFFF);
    rig.run(|a| {
        a.movq(mm0, qword_ptr(DATA))?;
        a.movq(mm1, qword_ptr(DATA + 8))?;
        a.movq(mm2, mm0)?;
        a.pavgb(mm2, mm1)?;
        a.movq(qword_ptr(DATA + 0x30), mm2)?;
        a.movq(mm2, mm0)?;
        a.pminub(mm2, mm1)?;
        a.movq(qword_ptr(DATA + 0x38), mm2)?;
        a.movq(mm2, mm0)?;
        a.pmaxsw(mm2, mm1)?;
        a.movq(qword_ptr(DATA + 0x40), mm2)?;
        a.movq(mm2, mm0)?;
        a.psadbw(mm2, mm1)?;
        a.movq(qword_ptr(DATA + 0x48), mm2)?;
        a.pshufw(mm2, mm0, 0x1B)?;
        a.movq(qword_ptr(DATA + 0x50), mm2)?;
        a.pextrw(eax, mm0, 3)?;
        a.mov(dword_ptr(DATA + 0x58), eax)?;
        a.pmovmskb(eax, mm0)?;
        a.mov(dword_ptr(DATA + 0x5C), eax)?;
        a.mov(eax, 0xABCD_1234u32)?;
        a.movq(mm3, mm0)?;
        a.pinsrw(mm3, eax, 2)?;
        a.movq(qword_ptr(DATA + 0x60), mm3)?;
        a.movq(mm4, mm0)?;
        a.pmulhuw(mm4, mm1)?;
        a.movntq(qword_ptr(DATA + 0x68), mm4)?;
        // Store MM1's bytes where MM0's have their top bit set: bytes 0 and 7.
        a.mov(edi, DATA + 0x20)?;
        a.maskmovq(mm1, mm0)?;
        a.emms()?;
        a.hlt()
    });
    let q = |o: u32| rig.read32(DATA + o) as u64 | (rig.read32(DATA + o + 4) as u64) << 32;
    assert_eq!(q(0x30), 0x4170_8305_0303_0380);
    assert_eq!(q(0x38), 0x0170_0605_0102_0201);
    // Words: 02FF/0301 -> 0301, 0403/0102 -> 0403, 0605/FF05 -> 0605, 8070/0170 -> 0170.
    assert_eq!(q(0x40), 0x0170_0605_0403_0301);
    // |FF-01| + |02-03| + |03-02| + |04-01| + |05-05| + |06-FF| + |70-70| + |80-01|
    assert_eq!(q(0x48), (0xFE + 1 + 1 + 3 + 0 + 0xF9 + 0 + 0x7F) as u64);
    assert_eq!(q(0x50), 0x02FF_0403_0605_8070);
    assert_eq!((rig.read32(DATA + 0x58), rig.read32(DATA + 0x5C)), (0x8070, 0b1000_0001));
    assert_eq!(q(0x60), 0x8070_1234_0403_02FF);
    let mulhuw = [(0x02FFu64, 0x0301u64), (0x0403, 0x0102), (0x0605, 0xFF05), (0x8070, 0x0170)]
        .iter()
        .enumerate()
        .fold(0, |r, (i, &(a, b))| r | ((a * b) >> 16) << (16 * i));
    assert_eq!(q(0x68), mulhuw);
    assert_eq!(q(0x20), 0x01FF_FFFF_FFFF_FF01);
}

#[test]
fn fxsave_and_fxrstor() {
    let mut rig = p3_rig();
    rig.write32(DATA + 0x400, 3.0f32.to_bits());
    write_lanes(&mut rig, DATA + 0x410, [1, 2, 3, 4]);
    rig.write32(DATA + 0x420, 0x1F80 | 0x6000);
    rig.run(|a| {
        set_cr4(a, OSFXSR)?;
        a.fninit()?;
        a.fld(dword_ptr(DATA + 0x400))?; // ST0 = 3, physical register 7
        a.fld1()?; // ST0 = 1, physical register 6
        a.movaps(xmm7, xmmword_ptr(DATA + 0x410))?;
        a.ldmxcsr(dword_ptr(DATA + 0x420))?;
        a.fxsave(ptr(DATA))?;
        // Change it all, then load it back.
        a.fninit()?;
        a.xorps(xmm7, xmm7)?;
        a.fxrstor(ptr(DATA))?;
        a.fstp(dword_ptr(DATA + 0x430))?;
        a.fstp(dword_ptr(DATA + 0x434))?;
        a.movaps(xmmword_ptr(DATA + 0x440), xmm7)?;
        a.stmxcsr(dword_ptr(DATA + 0x450))?;
        a.hlt()
    });
    assert_eq!(rig.read32(DATA) & 0xFFFF, 0x037F, "the control word");
    assert_eq!(rig.read32(DATA) >> 16 & 0x3800, 6 << 11, "the top of stack in the status word");
    assert_eq!(rig.read32(DATA + 4) & 0xFF, 0xC0, "registers 6 and 7 in use");
    assert_eq!((rig.read32(DATA + 24), rig.read32(DATA + 28)), (0x7F80, 0));
    // ST0 (1.0) first: 80 bits, then 6 bytes of 0.
    assert_eq!((rig.read32(DATA + 32), rig.read32(DATA + 36), rig.read32(DATA + 40)), (0, 0x8000_0000, 0x3FFF));
    assert_eq!(rig.read32(DATA + 44), 0);
    assert_eq!(read_lanes(&rig, DATA + 160 + 7 * 16), [1, 2, 3, 4]);
    assert_eq!((f32::from_bits(rig.read32(DATA + 0x430)), f32::from_bits(rig.read32(DATA + 0x434))), (1.0, 3.0));
    assert_eq!(read_lanes(&rig, DATA + 0x440), [1, 2, 3, 4]);
    assert_eq!(rig.read32(DATA + 0x450), 0x7F80);
}

#[test]
fn fxrstor_refuses_reserved_mxcsr_bits() {
    let mut rig = p3_rig();
    rig.record(GP);
    rig.write32(DATA + 24, 0x1F80 | 0x40);
    rig.run(|a| {
        a.fninit()?;
        a.fxrstor(ptr(DATA))?;
        a.hlt()
    });
    assert_eq!(rig.recorded().0, GP as u32);
    assert_eq!(rig.cpu.fpu_control, 0x037F, "nothing loaded");
}

#[test]
fn the_sse_state_goes_into_a_save_state() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    cpu.model = CpuModel::PentiumIII;
    cpu.sse.xmm[3] = 0x0123_4567_89AB_CDEF_FEDC_BA98_7654_3210;
    cpu.sse.mxcsr = 0x7F80;
    cpu.sse.psn_off = true;
    let saved = cpu.sse;
    let state = rust_dos::savestate::machine::save(&cpu);
    cpu.sse = Default::default();
    rust_dos::savestate::machine::load(&mut cpu, &state).unwrap();
    assert_eq!(cpu.sse, saved);
    // A state from before it (without its section, the last) loads, with
    // the registers as after a reset.
    let older = &state[..state.len() - (4 + 2 + 4 + 8 * 16 + 4 + 1)];
    rust_dos::savestate::machine::load(&mut cpu, older).unwrap();
    assert_eq!(cpu.sse, Default::default());
}

/// The recompiler runs SSE as the interpreter does: a loop of packed and
/// scalar arithmetic, conversions, compares into EFLAGS, the integer
/// instructions on MMX registers, and FXSAVE/FXRSTOR, in lockstep.
#[test]
fn the_recompiler_runs_sse_as_the_interpreter() {
    rust_dos::hosttime::fix(chrono::NaiveDate::from_ymd_opt(1999, 2, 26).unwrap().and_hms_opt(12, 34, 56));
    let mut a = p3_rig();
    let mut b = p3_rig();
    a.cpu.core = CoreMode::Normal;
    b.cpu.core = CoreMode::Dynamic;
    for rig in [&mut a, &mut b] {
        write_ps(rig, DATA + 0x100, [1.5, -2.25, 1e-3, 7.0]);
        let code = asm32(CODE, |a| {
            set_cr4(a, OSFXSR)?;
            a.mov(edi, 300u32)?;
            a.movaps(xmm1, xmmword_ptr(DATA + 0x100))?;
            a.jmp((CODE + 0x100) as u64)
        });
        rig.load(CODE, &code);
        let body = asm32(CODE + 0x100, |a| {
            a.cvtsi2ss(xmm0, edi)?;
            a.shufps(xmm0, xmm0, 0)?;
            a.mulps(xmm0, xmm1)?;
            a.rsqrtps(xmm2, xmm0)?;
            a.addps(xmm1, xmm2)?;
            a.sqrtss(xmm3, xmm1)?;
            a.comiss(xmm3, xmm0)?;
            a.adc(esi, 0)?;
            a.cvttps2pi(mm0, xmm1)?;
            a.pavgw(mm0, mm1)?;
            a.psadbw(mm1, mm0)?;
            a.pextrw(eax, mm1, 0)?;
            a.add(esi, eax)?;
            a.emms()?;
            a.fxsave(ptr(DATA + 0x200))?;
            a.xorps(xmm1, xmm3)?;
            a.fxrstor(ptr(DATA + 0x200))?;
            a.movaps(xmmword_ptr(DATA), xmm1)?;
            a.stmxcsr(dword_ptr(DATA + 0x10))?;
            a.dec(edi)?;
            a.jnz((CODE + 0x100) as u64)?;
            a.hlt()
        });
        rig.load(CODE + 0x100, &body);
        rig.enter_pm();
    }
    dyndiff::lockstep_with(&mut a.cpu, &mut b.cpu, 2000, 997, true, |_, _| {}).unwrap();
    assert_eq!(a.cpu.edi(), 0, "the loop ran to its end");
    assert_eq!(a.cpu.sse, b.cpu.sse);
    if rust_dos::dynrec::AVAILABLE {
        assert!(b.cpu.dynrec.stats().runs > 0);
    }
}
