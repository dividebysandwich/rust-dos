//! The Pentium MMX: CPUID, the MMX instructions, their registers being
//! the FPU's, and what a plain Pentium does with them.

mod pmrig;
mod testrunners;

use iced_x86::code_asm::*;
use pmrig::*;
use rust_dos::cpu::{Cpu, CpuModel};
use testrunners::run_cpu_code;

const UD: u8 = 6;
const NM: u8 = 7;

fn mmx_rig() -> Rig {
    let mut rig = Rig::new();
    rig.cpu.model = CpuModel::PentiumMmx;
    rig
}

fn write64(rig: &mut Rig, addr: u32, v: u64) {
    rig.write32(addr, v as u32);
    rig.write32(addr + 4, (v >> 32) as u32);
}

fn read64(rig: &Rig, addr: u32) -> u64 {
    rig.read32(addr) as u64 | (rig.read32(addr + 4) as u64) << 32
}

#[test]
fn cpuid_reports_a_pentium_mmx() {
    for (model, signature, features) in [(CpuModel::Pentium, 0x517, 0x139), (CpuModel::PentiumMmx, 0x543, 0x80_0139)] {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        cpu.model = model;
        // 66 B8 01 00 00 00 -> MOV EAX, 1 ; 0F A2 -> CPUID
        run_cpu_code(&mut cpu, &[0x66, 0xB8, 0x01, 0x00, 0x00, 0x00, 0x0F, 0xA2]);
        assert_eq!((cpu.eax(), cpu.edx()), (signature, features), "{model:?}");
    }
}

#[test]
fn movd_and_movq_in_real_mode() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    cpu.model = CpuModel::PentiumMmx;
    // 66 B8 78 56 34 12 -> MOV EAX, 12345678h ; 0F 6E C0 -> MOVD MM0, EAX ;
    // 0F 6F C8 -> MOVQ MM1, MM0 ; 0F 73 F1 20 -> PSLLQ MM1, 32 ;
    // 0F EB C8 -> POR MM1, MM0 ; 0F 7F 0E 00 02 -> MOVQ [200h], MM1 ;
    // 0F 73 D1 28 -> PSRLQ MM1, 40 ; 0F 7E CB -> MOVD EBX, MM1 ; 0F 77 -> EMMS
    run_cpu_code(
        &mut cpu,
        &[
            0x66, 0xB8, 0x78, 0x56, 0x34, 0x12, 0x0F, 0x6E, 0xC0, 0x0F, 0x6F, 0xC8, 0x0F, 0x73, 0xF1, 0x20, 0x0F, 0xEB,
            0xC8, 0x0F, 0x7F, 0x0E, 0x00, 0x02, 0x0F, 0x73, 0xD1, 0x28, 0x0F, 0x7E, 0xCB, 0x0F, 0x77,
        ],
    );
    let base = (cpu.ds() as usize) << 4;
    assert_eq!(cpu.bus.read_32(base + 0x200), 0x1234_5678);
    assert_eq!(cpu.bus.read_32(base + 0x204), 0x1234_5678);
    assert_eq!(cpu.ebx(), 0x0012_3456);
}

#[test]
fn a_pentium_has_no_mmx() {
    for code in [&[0x0F, 0x77][..], &[0x0F, 0x6F, 0xC8], &[0x0F, 0xFC, 0xC1]] {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        cpu.model = CpuModel::Pentium;
        cpu.bus.write_16(UD as usize * 4, 0x0700);
        cpu.bus.write_16(UD as usize * 4 + 2, 0x0000);
        run_cpu_code(&mut cpu, code);
        assert_eq!((cpu.cs(), cpu.ip()), (0x0000, 0x0700), "{:02X?} raises #UD", code);
    }
}

#[test]
fn sse_forms_of_mmx_mnemonics_are_undefined() {
    // 66 0F FC C1 -> PADDB XMM0, XMM1 ; 0F 70 C1 00 -> PSHUFW MM0, MM1, 0
    for code in [&[0x66, 0x0F, 0xFC, 0xC1][..], &[0x0F, 0x70, 0xC1, 0x00]] {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        cpu.model = CpuModel::PentiumMmx;
        cpu.bus.write_16(UD as usize * 4, 0x0700);
        cpu.bus.write_16(UD as usize * 4 + 2, 0x0000);
        run_cpu_code(&mut cpu, code);
        assert_eq!((cpu.cs(), cpu.ip()), (0x0000, 0x0700), "{:02X?} raises #UD", code);
    }
}

#[test]
fn cr0_em_makes_mmx_undefined_and_ts_not_available() {
    for (bit, vector) in [(4u32, UD), (8, NM)] {
        for emms in [false, true] {
            let mut rig = mmx_rig();
            rig.record(vector);
            rig.run(|a| {
                a.mov(eax, cr0)?;
                a.or(eax, bit)?;
                a.mov(cr0, eax)?;
                if emms { a.emms()? } else { a.paddb(mm0, mm1)? }
                a.hlt()
            });
            assert_eq!(rig.recorded().0, vector as u32, "CR0 bit {bit:#x}, EMMS {emms}");
        }
    }
}

#[test]
fn mmx_registers_are_the_fpu_registers() {
    let mut rig = mmx_rig();
    write64(&mut rig, DATA, 0x0123_4567_89AB_CDEF);
    rig.run(|a| {
        a.fninit()?;
        // FLD1 goes to physical R7, whose significand MM7 is.
        a.fld1()?;
        a.fnstenv(ptr(DATA + 0x100))?;
        // MMX instructions: TOP 0 and every register valid.
        a.movq(qword_ptr(RESULT), mm7)?;
        a.movq(mm3, qword_ptr(DATA))?;
        a.fnsave(ptr(DATA + 0x200))?;
        a.frstor(ptr(DATA + 0x200))?;
        // EMMS: all registers empty.
        a.emms()?;
        a.fnstenv(ptr(DATA + 0x300))?;
        a.hlt()
    });
    assert_eq!(read64(&rig, RESULT), 0x8000_0000_0000_0000, "1.0's significand");
    // FNSTENV before MOVQ: TOP 7, R7 valid, the rest empty.
    assert_eq!((rig.read32(DATA + 0x104) >> 11) & 7, 7);
    assert_eq!(rig.read32(DATA + 0x108) & 0xFFFF, 0x3FFF);
    // FNSAVE after it: TOP 0, no register empty (tags from the contents),
    // and MM3 in ST(3) with an exponent of all ones.
    assert_eq!((rig.read32(DATA + 0x204) >> 11) & 7, 0);
    let tags = rig.read32(DATA + 0x208) & 0xFFFF;
    assert!((0..8).all(|i| (tags >> (2 * i)) & 3 != 3), "tags {tags:04X}");
    let saved_st3 = DATA + 0x200 + 28 + 3 * 10;
    assert_eq!(read64(&rig, saved_st3), 0x0123_4567_89AB_CDEF);
    assert_eq!(rig.read32(saved_st3 + 8) & 0xFFFF, 0xFFFF);
    assert_eq!(rig.read32(DATA + 0x308) & 0xFFFF, 0xFFFF, "EMMS empties them");
}

#[test]
fn a_faulting_operand_leaves_the_fpu_as_it_was() {
    let mut rig = mmx_rig();
    rig.record(13);
    rig.set_gdt(FREE, seg_desc(0, 0xFF, 0x92, 0x4));
    rig.run(|a| {
        a.fninit()?;
        a.fld1()?;
        a.mov(ax, FREE as u32)?;
        a.mov(ds, ax)?;
        a.movq(mm0, qword_ptr(0xFC))?;
        a.hlt()
    });
    assert_eq!(rig.recorded().0, 13);
    assert_eq!(rig.cpu.fpu_top, 7, "TOP as FLD1 left it");
}

#[test]
fn packed_arithmetic_saturates_and_wraps() {
    let mut rig = mmx_rig();
    let cases: [(fn(&mut CodeAssembler) -> Result<(), IcedError>, u64, u64, u64); 10] = [
        (|a| a.paddb(mm0, mm1), 0x7F80_FF01_0000_0000, 0x0101_0101_0000_0000, 0x8081_0002_0000_0000),
        (|a| a.paddsb(mm0, mm1), 0x7F80_FF01_0000_0000, 0x01FF_0101_0000_0000, 0x7F80_0002_0000_0000),
        (|a| a.paddusw(mm0, mm1), 0xFFFF_0001_0000_8000, 0x0001_0001_0000_8000, 0xFFFF_0002_0000_FFFF),
        (|a| a.psubusb(mm0, mm1), 0x0010_2000_0000_0000, 0x0120_1000_0000_0000, 0x0000_1000_0000_0000),
        (|a| a.pmaddwd(mm0, mm1), 0x8000_8000_0002_0003, 0x8000_8000_0004_0005, 0x8000_0000_0000_0017),
        (|a| a.pmulhw(mm0, mm1), 0x0000_0000_4000_FFFF, 0x0000_0000_0004_FFFF, 0x0000_0000_0001_0000),
        (|a| a.packsswb(mm0, mm1), 0x0000_0000_7FFF_8000, 0x0001_FF00_0080_FF7F, 0x0180_7F80_0000_7F80),
        (|a| a.packuswb(mm0, mm1), 0x0100_FFFF_0080_007F, 0x0000_0000_0000_0000, 0x0000_0000_FF00_807F),
        (|a| a.punpcklbw(mm0, mm1), 0x0000_0000_4433_2211, 0x0000_0000_DDCC_BBAA, 0xDD44_CC33_BB22_AA11),
        (|a| a.punpckhdq(mm0, mm1), 0x1111_1111_0000_0000, 0x2222_2222_0000_0000, 0x2222_2222_1111_1111),
    ];
    for (i, (_, a, b, _)) in cases.iter().enumerate() {
        write64(&mut rig, DATA + 16 * i as u32, *a);
        write64(&mut rig, DATA + 16 * i as u32 + 8, *b);
    }
    rig.run(|asm| {
        for (i, (op, ..)) in cases.iter().enumerate() {
            asm.movq(mm0, qword_ptr(DATA + 16 * i as u32))?;
            asm.movq(mm1, qword_ptr(DATA + 16 * i as u32 + 8))?;
            op(asm)?;
            asm.movq(qword_ptr(DATA + 0x1000 + 8 * i as u32), mm0)?;
        }
        asm.hlt()
    });
    for (i, (.., want)) in cases.iter().enumerate() {
        assert_eq!(read64(&rig, DATA + 0x1000 + 8 * i as u32), *want, "case {i}");
    }
}

#[test]
fn shifts_past_the_lane_width_clear_or_fill_with_the_sign() {
    let mut rig = mmx_rig();
    write64(&mut rig, DATA, 0x8001_7FFF_0123_F000);
    rig.run(|a| {
        a.movq(mm0, qword_ptr(DATA))?;
        a.movq(mm1, mm0)?;
        a.movq(mm2, mm0)?;
        a.movq(mm3, mm0)?;
        a.psraw(mm0, 20)?;
        a.psrlw(mm1, 16)?;
        a.psllq(mm2, 4)?;
        a.mov(eax, 3u32)?;
        a.movd(mm4, eax)?;
        a.psrad(mm3, mm4)?;
        a.movq(qword_ptr(RESULT), mm0)?;
        a.movq(qword_ptr(RESULT + 8), mm1)?;
        a.movq(qword_ptr(RESULT + 16), mm2)?;
        a.movq(qword_ptr(RESULT + 24), mm3)?;
        a.hlt()
    });
    assert_eq!(read64(&rig, RESULT), 0xFFFF_0000_0000_FFFF);
    assert_eq!(read64(&rig, RESULT + 8), 0);
    assert_eq!(read64(&rig, RESULT + 16), 0x0017_FFF0_123F_0000);
    assert_eq!(read64(&rig, RESULT + 24), 0xF000_2FFF_0024_7E00);
}

/// Each MMX operation against the host's SSE2 one, which does the same
/// to the low quadword, on random operands.
#[cfg(target_arch = "x86_64")]
mod against_sse2 {
    use super::*;
    use std::arch::x86_64::*;

    type Asm = fn(&mut CodeAssembler, bool, u32) -> Result<(), IcedError>;
    type Reference = unsafe fn(__m128i, __m128i) -> __m128i;

    macro_rules! op {
        ($name:ident, $reference:expr) => {
            (
                stringify!($name),
                (|a: &mut CodeAssembler, mem: bool, at: u32| {
                    if mem { a.$name(mm0, qword_ptr(at)) } else { a.$name(mm0, mm1) }
                }) as Asm,
                $reference as Reference,
            )
        };
    }

    /// The low quadwords' halves as MMX PUNPCKH takes them.
    unsafe fn high(x: __m128i) -> __m128i {
        unsafe { _mm_srli_epi64(x, 32) }
    }

    /// `a`'s low quadword, then `b`'s, as MMX PACK packs them.
    unsafe fn both(a: __m128i, b: __m128i) -> __m128i {
        unsafe { _mm_unpacklo_epi64(a, b) }
    }

    fn ops() -> Vec<(&'static str, Asm, Reference)> {
        unsafe {
            vec![
                op!(paddb, |a, b| _mm_add_epi8(a, b)),
                op!(paddw, |a, b| _mm_add_epi16(a, b)),
                op!(paddd, |a, b| _mm_add_epi32(a, b)),
                op!(paddsb, |a, b| _mm_adds_epi8(a, b)),
                op!(paddsw, |a, b| _mm_adds_epi16(a, b)),
                op!(paddusb, |a, b| _mm_adds_epu8(a, b)),
                op!(paddusw, |a, b| _mm_adds_epu16(a, b)),
                op!(psubb, |a, b| _mm_sub_epi8(a, b)),
                op!(psubw, |a, b| _mm_sub_epi16(a, b)),
                op!(psubd, |a, b| _mm_sub_epi32(a, b)),
                op!(psubsb, |a, b| _mm_subs_epi8(a, b)),
                op!(psubsw, |a, b| _mm_subs_epi16(a, b)),
                op!(psubusb, |a, b| _mm_subs_epu8(a, b)),
                op!(psubusw, |a, b| _mm_subs_epu16(a, b)),
                op!(pmullw, |a, b| _mm_mullo_epi16(a, b)),
                op!(pmulhw, |a, b| _mm_mulhi_epi16(a, b)),
                op!(pmaddwd, |a, b| _mm_madd_epi16(a, b)),
                op!(pcmpeqb, |a, b| _mm_cmpeq_epi8(a, b)),
                op!(pcmpeqw, |a, b| _mm_cmpeq_epi16(a, b)),
                op!(pcmpeqd, |a, b| _mm_cmpeq_epi32(a, b)),
                op!(pcmpgtb, |a, b| _mm_cmpgt_epi8(a, b)),
                op!(pcmpgtw, |a, b| _mm_cmpgt_epi16(a, b)),
                op!(pcmpgtd, |a, b| _mm_cmpgt_epi32(a, b)),
                op!(pand, |a, b| _mm_and_si128(a, b)),
                op!(pandn, |a, b| _mm_andnot_si128(a, b)),
                op!(por, |a, b| _mm_or_si128(a, b)),
                op!(pxor, |a, b| _mm_xor_si128(a, b)),
                op!(psllw, |a, b| _mm_sll_epi16(a, b)),
                op!(pslld, |a, b| _mm_sll_epi32(a, b)),
                op!(psllq, |a, b| _mm_sll_epi64(a, b)),
                op!(psrlw, |a, b| _mm_srl_epi16(a, b)),
                op!(psrld, |a, b| _mm_srl_epi32(a, b)),
                op!(psrlq, |a, b| _mm_srl_epi64(a, b)),
                op!(psraw, |a, b| _mm_sra_epi16(a, b)),
                op!(psrad, |a, b| _mm_sra_epi32(a, b)),
                op!(packsswb, |a, b| _mm_packs_epi16(both(a, b), _mm_setzero_si128())),
                op!(packssdw, |a, b| _mm_packs_epi32(both(a, b), _mm_setzero_si128())),
                op!(packuswb, |a, b| _mm_packus_epi16(both(a, b), _mm_setzero_si128())),
                op!(punpcklbw, |a, b| _mm_unpacklo_epi8(a, b)),
                op!(punpcklwd, |a, b| _mm_unpacklo_epi16(a, b)),
                op!(punpckldq, |a, b| _mm_unpacklo_epi32(a, b)),
                op!(punpckhbw, |a, b| _mm_unpacklo_epi8(high(a), high(b))),
                op!(punpckhwd, |a, b| _mm_unpacklo_epi16(high(a), high(b))),
                op!(punpckhdq, |a, b| _mm_unpacklo_epi32(high(a), high(b))),
            ]
        }
    }

    const CASES: u32 = 64;

    /// Operands with lanes at the edges of their ranges now and then.
    fn operands(seed: u64, shift: bool) -> Vec<(u64, u64)> {
        let mut x = seed | 1;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        const EDGES: [u16; 6] = [0x0000, 0x8000, 0x7FFF, 0xFFFF, 0x0080, 0x007F];
        (0..CASES)
            .map(|_| {
                let mut value = || {
                    let r = next();
                    if r & 3 == 0 {
                        (0..4).fold(0, |v, i| v | (EDGES[(r >> (8 + 4 * i)) as usize % 6] as u64) << (16 * i))
                    } else {
                        next()
                    }
                };
                let a = value();
                let b = if shift { next() % 72 } else { value() };
                (a, b)
            })
            .collect()
    }

    fn check(batched: bool) {
        for (n, (name, asm, reference)) in ops().into_iter().enumerate() {
            let shift = name.starts_with("ps") && !name.starts_with("psub");
            let pairs = operands(0x9E37_79B9_7F4A_7C15 ^ n as u64, shift);
            let mut rig = mmx_rig();
            for (i, &(a, b)) in pairs.iter().enumerate() {
                write64(&mut rig, DATA + 16 * i as u32, a);
                write64(&mut rig, DATA + 16 * i as u32 + 8, b);
            }
            let code = |a: &mut CodeAssembler| {
                for i in 0..CASES {
                    let at = DATA + 16 * i;
                    a.movq(mm0, qword_ptr(at))?;
                    let mem = i % 2 == 1;
                    if !mem {
                        a.movq(mm1, qword_ptr(at + 8))?;
                    }
                    asm(a, mem, at + 8)?;
                    a.movq(qword_ptr(DATA + 0x1000 + 8 * i), mm0)?;
                }
                a.emms()?;
                a.hlt()
            };
            if batched { rig.run_batched(code) } else { rig.run(code) }
            for (i, &(a, b)) in pairs.iter().enumerate() {
                let want = unsafe {
                    _mm_cvtsi128_si64(reference(_mm_cvtsi64_si128(a as i64), _mm_cvtsi64_si128(b as i64))) as u64
                };
                let got = read64(&rig, DATA + 0x1000 + 8 * i as u32);
                assert_eq!(got, want, "{name} {a:016X}, {b:016X}: {got:016X}, not {want:016X}");
            }
        }
    }

    #[test]
    fn every_operation_matches_sse2() {
        check(false);
    }

    #[test]
    fn every_operation_matches_sse2_in_batches() {
        check(true);
    }
}
