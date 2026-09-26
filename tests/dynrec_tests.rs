//! The dynamic recompiler's hard cases, each run on the interpreter and on
//! the recompiler side by side (tests/dyndiff), which must agree after
//! every batch: code that rewrites the block it is in, faults in the
//! middle of a block, interrupt shadows, the timer deadline, a full code
//! memory, and `core=auto`'s switching.

mod dyndiff;
mod pmrig;

use chrono::NaiveDate;
use dyndiff::lockstep_with;
use iced_x86::code_asm::*;
use pmrig::*;
use rust_dos::cpu::{CoreMode, Cpu};
use rust_dos::dynrec::{AVAILABLE, DynStats};

const GP: u8 = 13;
const PF: u8 = 14;

/// Two rigs set up by `setup`, the second on the recompiler, both in
/// protected mode at CODE. They see the same fixed time: DOS stamps its
/// file table with it, so rigs made a moment apart could differ.
fn twins(setup: impl Fn(&mut Rig)) -> (Rig, Rig) {
    rust_dos::hosttime::fix(NaiveDate::from_ymd_opt(1995, 4, 11).unwrap().and_hms_opt(12, 34, 56));
    let mut a = Rig::new();
    let mut b = Rig::new();
    a.cpu.core = CoreMode::Normal;
    b.cpu.core = CoreMode::Dynamic;
    for rig in [&mut a, &mut b] {
        setup(rig);
        rig.enter_pm();
    }
    (a, b)
}

/// Run both in lockstep until they halt (small batches, so the
/// comparisons come often), and return the recompiler's counts.
fn run_both(a: &mut Rig, b: &mut Rig) -> DynStats {
    lockstep_with(&mut a.cpu, &mut b.cpu, 2000, 997, true, |_, _| {}).unwrap();
    assert!(halted(&a.cpu), "no HLT: CS:EIP {:04X}:{:08X}", a.cpu.cs(), a.cpu.eip());
    let stats = b.cpu.dynrec.stats();
    if AVAILABLE {
        assert!(stats.runs > 0, "no translated code ran: {:?}", stats);
    }
    stats
}

/// The next instruction is a HLT (a halted CPU in a batch goes on at the
/// next timer event, so this is the test's end).
fn halted(cpu: &Cpu) -> bool {
    cpu.bus.read_8(cpu.seg_cache(rust_dos::cpu::Seg::CS).base.wrapping_add(cpu.eip()) as usize) == 0xF4
}

#[test]
fn a_store_into_the_rest_of_its_block_is_seen() {
    // The loop patches the immediate of the ADD after it, in the same
    // block, with the count: EAX sums 100..1.
    let (mut a, mut b) = twins(|rig| {
        let code = asm32(CODE, |a| {
            a.xor(eax, eax)?;
            a.mov(ecx, 100u32)?;
            a.jmp(CODE as u64 + 0x100)
        });
        rig.load(CODE, &code);
        let lp = asm32(CODE + 0x100, |a| {
            // mov [CODE+0x108], cl: 6 bytes, then ADD EAX, imm8 at +6.
            a.mov(byte_ptr(CODE as u64 + 0x108), cl)?;
            a.db(&[0x83, 0xC0, 0x00])?;
            a.dec(ecx)?;
            a.jnz(CODE as u64 + 0x100)?;
            a.hlt()
        });
        rig.load(CODE + 0x100, &lp);
    });
    let stats = run_both(&mut a, &mut b);
    assert_eq!(b.cpu.eax(), 5050);
    if AVAILABLE {
        assert!(stats.smc > 0, "{:?}", stats);
    }
}

#[test]
fn the_translated_code_counts_the_instructions_it_runs() {
    // A loop of 3 instructions, 10000 times: the Stats page's share of
    // them the recompiler ran.
    let (mut a, mut b) = twins(|rig| {
        let code = asm32(CODE, |a| {
            a.xor(eax, eax)?;
            a.mov(ecx, 10000u32)?;
            a.jmp(CODE as u64 + 0x100)
        });
        rig.load(CODE, &code);
        let lp = asm32(CODE + 0x100, |a| {
            a.add(eax, ecx)?;
            a.dec(ecx)?;
            a.jnz(CODE as u64 + 0x100)?;
            a.hlt()
        });
        rig.load(CODE + 0x100, &lp);
    });
    run_both(&mut a, &mut b);
    assert_eq!(a.cpu.dynrec.counts().executed, 0, "the interpreter ran them all");
    if AVAILABLE {
        let translated = b.cpu.dynrec.counts().executed;
        assert!(translated > 29_000 && translated <= b.cpu.executed, "{} of {}", translated, b.cpu.executed);
    }
}

#[test]
fn rep_stos_over_the_next_instruction_is_seen() {
    // REP STOSB turns the MOV BL after it into NOPs.
    let (mut a, mut b) = twins(|rig| {
        let code = asm32(CODE, |a| {
            a.mov(ax, DATA32 as u32)?;
            a.mov(es, ax)?;
            a.xor(ebx, ebx)?;
            a.mov(edi, CODE + 0x100 + 12)?;
            a.jmp(CODE as u64 + 0x100)
        });
        rig.load(CODE, &code);
        let body = asm32(CODE + 0x100, |a| {
            a.mov(al, 0x90)?; // 2 bytes
            a.mov(ecx, 2u32)?; // 5 bytes
            a.cld()?; // 1
            a.nop()?; // 1
            a.rep().stosb()?; // 2
            a.nop()?; // 1: +12 is next
            a.mov(bl, 0x55)?; // 2 bytes at +12
            a.inc(ebx)?;
            a.hlt()
        });
        rig.load(CODE + 0x100, &body);
    });
    run_both(&mut a, &mut b);
    assert_eq!(b.cpu.ebx(), 1, "the MOV became NOPs");
}

#[test]
fn a_fault_in_the_middle_of_a_block_leaves_the_instructions_before_it_done() {
    // (The recording handler uses EAX, ECX, ESI and EDI.)
    let before = |a: &mut CodeAssembler| {
        a.mov(ax, FREE as u32)?;
        a.mov(ds, ax)?;
        a.mov(ebx, 1u32)?;
        a.add(ebp, 2)?;
        a.push(0x1234u32)
    };
    let faulting = CODE + asm32(CODE, before).len() as u32;
    let (mut a, mut b) = twins(|rig| {
        rig.record(GP);
        // A data segment of 256 bytes at 40000h.
        rig.set_gdt(FREE, seg_desc(0x40000, 0xFF, DATA_R0, 0x4));
        let code = asm32(CODE, |a| {
            a.xor(ebp, ebp)?;
            a.jmp(CODE as u64 + 0x100)
        });
        rig.load(CODE, &code);
        let body = asm32(CODE + 0x100, |a| {
            before(a)?;
            a.mov(eax, dword_ptr(0x200))?;
            a.mov(edx, 3u32)?;
            a.hlt()
        });
        rig.load(CODE + 0x100, &body);
    });
    run_both(&mut a, &mut b);
    let (vector, stack) = b.recorded();
    assert_eq!(vector, GP as u32);
    assert_eq!(stack[1], faulting + 0x100, "EIP of the faulting instruction");
    assert_eq!((b.cpu.ebx(), b.cpu.ebp()), (1, 2));
    assert_ne!(b.cpu.edx(), 3);
    assert_eq!(b.cpu.read_linear_u16(STACK0_TOP - 4), 0x1234, "the PUSH before it happened");
}

#[test]
fn a_page_fault_in_the_middle_of_a_block_reports_its_address() {
    let (mut a, mut b) = twins(|rig| {
        // Identity-mapped first 4 MB, but for page 55000h.
        let (dir, table) = (0x80000u32, 0x81000u32);
        rig.write32(dir, table | 3);
        for i in 0..1024u32 {
            rig.write32(table + 4 * i, (i << 12) | 3);
        }
        rig.write32(table + 0x55 * 4, 0);
        rig.record(PF);
        let code = asm32(CODE, |a| {
            a.mov(eax, dir)?;
            a.mov(cr3, eax)?;
            a.mov(eax, cr0)?;
            a.or(eax, 0x8000_0000u32)?;
            a.mov(cr0, eax)?;
            a.jmp(CODE as u64 + 0x100)
        });
        rig.load(CODE, &code);
        let body = asm32(CODE + 0x100, |a| {
            a.mov(ebx, 7u32)?;
            a.inc(ebx)?;
            a.mov(dword_ptr(0x55123), ebx)?;
            a.mov(edx, 9u32)?;
            a.hlt()
        });
        rig.load(CODE + 0x100, &body);
    });
    run_both(&mut a, &mut b);
    let (vector, stack) = b.recorded();
    assert_eq!((vector, stack[0]), (PF as u32, 2));
    assert_eq!(stack[1], CODE + 0x100 + 6);
    assert_eq!(b.cpu.cr2, 0x55123);
    assert_eq!(b.cpu.ebx(), 8);
    assert_ne!(b.cpu.edx(), 9);
}

/// The arithmetic flags of EFLAGS.
const ARITH: u32 = 0x8D5;

#[test]
fn flags_the_code_keeps_in_a_register_reach_a_fault_handler() {
    // SUB sets CF (and AF and SF); INC then sets the rest but leaves CF,
    // and the load faults before the block puts the flags back.
    let (mut a, mut b) = twins(|rig| {
        rig.record(GP);
        rig.set_gdt(FREE, seg_desc(0x40000, 0xFF, DATA_R0, 0x4));
        let code = asm32(CODE, |a| {
            a.mov(ax, FREE as u32)?;
            a.mov(ds, ax)?;
            a.mov(ecx, 0x7Fu32)?;
            a.mov(ebx, 5u32)?;
            a.sub(ebx, 7)?;
            a.inc(ecx)?;
            a.mov(eax, dword_ptr(0x200))?;
            a.hlt()
        });
        rig.load(CODE, &code);
    });
    run_both(&mut a, &mut b);
    let (vector, stack) = b.recorded();
    assert_eq!(vector, GP as u32);
    assert_eq!(stack[3] & ARITH, 0x11, "CF from the SUB, AF from the INC: EFLAGS {:08X}", stack[3]);
}

#[test]
fn flags_the_code_keeps_in_a_register_survive_a_store_into_the_block() {
    let prefix = |a: &mut CodeAssembler| {
        a.mov(ecx, 0x7Fu32)?;
        a.mov(ebx, 5u32)?;
        a.sub(ebx, 7)?;
        a.inc(ecx)?;
        a.mov(byte_ptr(0), 0x42)
    };
    // The MOV's displacement is where the next instruction's immediate is.
    let patch = CODE + asm32(CODE, prefix).len() as u32 + 1;
    let (mut a, mut b) = twins(|rig| {
        let code = asm32(CODE, |a| {
            a.mov(ecx, 0x7Fu32)?;
            a.mov(ebx, 5u32)?;
            a.sub(ebx, 7)?;
            a.inc(ecx)?;
            a.mov(byte_ptr(patch as u64), 0x42)?;
            a.mov(edx, 0x1111_1111u32)?;
            a.pushfd()?;
            a.pop(esi)?;
            a.hlt()
        });
        rig.load(CODE, &code);
    });
    let stats = run_both(&mut a, &mut b);
    assert_eq!(b.cpu.edx(), 0x1111_1142, "the patched immediate");
    assert_eq!(b.cpu.esi() & ARITH, 0x11, "EFLAGS {:08X}", b.cpu.esi());
    if AVAILABLE {
        assert!(stats.smc > 0, "the store stopped the block: {:?}", stats);
    }
}

#[test]
fn flags_are_exact_where_handlers_read_them_and_after_flags_nothing_reads() {
    let (mut a, mut b) = twins(|rig| {
        with_timer(rig, |a| {
            // CF from the SUB through the INC into the ADC.
            a.mov(eax, ecx)?;
            a.sub(eax, 1000)?;
            a.inc(ebx)?;
            a.adc(esi, 0)?;
            // Flags set again before anything reads them.
            a.shr(eax, 3)?;
            a.sar(ebx, 2)?;
            a.rol(edx, 5)?;
            a.add(ebp, eax)?;
            // Handlers that read them in the middle of a block.
            a.cmp(eax, ebx)?;
            a.pushfd()?;
            a.pop(edx)?;
            a.test(ecx, 3)?;
            a.lahf()?;
            a.xor(edx, eax)?;
            // CF set and complemented in the register, and read.
            a.stc()?;
            a.cmc()?;
            a.adc(ebp, 0)?;
            a.neg(eax)?;
            a.sbb(esi, eax)?;
            a.shl(ebp, 1)?;
            a.rcr(esi, 1)
        });
    });
    run_both(&mut a, &mut b);
    assert!(b.cpu.edi() > 10, "IRQ 0 came {} times", b.cpu.edi());
}

#[test]
fn divisions_and_their_faults_are_the_interpreters() {
    let (mut a, mut b) = twins(|rig| {
        // #DE: skip the division (ESI holds its length) and count it.
        rig.handler(0, 0, |a| {
            a.add(dword_ptr(esp), esi)?;
            a.inc(dword_ptr(RESULT))?;
            a.iretd()
        });
        with_timer(rig, |a| {
            // Dividends and divisors from the loop count, some of them 0,
            // some quotients too big, and the most negative ones by -1.
            a.mov(eax, ecx)?;
            a.imul_3(eax, eax, 0x9E37_79B1u32 as i32)?;
            a.mov(ebx, ecx)?;
            a.and(ebx, 0x1F)?;
            a.sub(ebx, 3)?;
            a.mov(dword_ptr(DATA), ebx)?;
            a.mov(edx, eax)?;
            a.sar(edx, 7)?;
            a.add(ebp, eax)?;
            let div = |a: &mut CodeAssembler, len: u32, f: &dyn Fn(&mut CodeAssembler) -> Result<(), IcedError>| {
                a.mov(esi, len)?;
                f(a)?;
                a.xor(ebp, eax)?;
                a.add(ebp, edx)
            };
            div(a, 2, &|a| a.idiv(ebx))?;
            a.mov(edx, ecx)?;
            a.shr(edx, 9)?;
            div(a, 2, &|a| a.div(ebx))?;
            div(a, 6, &|a| a.idiv(dword_ptr(DATA)))?;
            a.mov(edx, eax)?;
            div(a, 3, &|a| a.idiv(bx))?;
            div(a, 3, &|a| a.div(bx))?;
            a.mov(eax, ecx)?;
            div(a, 2, &|a| a.idiv(bl))?;
            div(a, 2, &|a| a.div(bl))?;
            // -2^31 / -1, -2^15 / -1, -2^7 / -1.
            a.mov(edx, 0x8000_0000u32)?;
            a.xor(eax, eax)?;
            a.mov(ebx, 0xFFFF_FFFFu32)?;
            div(a, 2, &|a| a.idiv(ebx))?;
            a.mov(edx, 0x8000u32)?;
            div(a, 3, &|a| a.idiv(bx))?;
            a.mov(eax, 0x8000u32)?;
            div(a, 2, &|a| a.idiv(bl))
        });
    });
    run_both(&mut a, &mut b);
    let faults = b.read32(RESULT);
    assert!(faults > 100, "#DE came {} times", faults);
}

#[test]
fn setcc_sees_the_flags_wherever_they_are() {
    let (mut a, mut b) = twins(|rig| {
        with_timer(rig, |a| {
            // Every condition, on flags in the register and in the CPU
            // (after a handler), into byte registers high and low and
            // memory, summed into EBP.
            a.mov(eax, ecx)?;
            a.imul_3(eax, eax, 0x9E37_79B1u32 as i32)?;
            a.mov(ebx, ecx)?;
            a.shl(ebx, 20)?;
            macro_rules! set {
                ($m:ident, $k:expr) => {
                    match $k % 3 {
                        0 => a.$m(dl)?,
                        1 => a.$m(dh)?,
                        _ => a.$m(byte_ptr(DATA + $k))?,
                    }
                };
            }
            for k in 0..16u32 {
                if k % 4 == 0 {
                    a.cmp(eax, ebx)?;
                } else if k % 4 == 2 {
                    a.sub(ebx, eax)?;
                    a.pushfd()?;
                    a.popfd()?;
                }
                match k {
                    0 => set!(seto, k),
                    1 => set!(setno, k),
                    2 => set!(setb, k),
                    3 => set!(setae, k),
                    4 => set!(sete, k),
                    5 => set!(setne, k),
                    6 => set!(setbe, k),
                    7 => set!(seta, k),
                    8 => set!(sets, k),
                    9 => set!(setns, k),
                    10 => set!(setp, k),
                    11 => set!(setnp, k),
                    12 => set!(setl, k),
                    13 => set!(setge, k),
                    14 => set!(setle, k),
                    _ => set!(setg, k),
                }
                a.add(ebp, edx)?;
                a.rol(ebp, 3)?;
            }
            a.add(ebp, dword_ptr(DATA))
        });
    });
    run_both(&mut a, &mut b);
}

#[test]
fn shifts_by_cl_and_of_memory_are_the_interpreters() {
    let (mut a, mut b) = twins(|rig| {
        with_timer(rig, |a| {
            // CL, the loop count's low byte, takes every count: 0, and
            // those past a byte's and a word's width, which a 386 shifts
            // its own way.
            a.mov(eax, ecx)?;
            a.imul_3(eax, eax, 0x9E37_79B1u32 as i32)?;
            a.mov(ebx, eax)?;
            a.ror(ebx, 7)?;
            a.mov(edx, ebx)?;
            a.not(edx)?;
            a.mov(dword_ptr(DATA), eax)?;
            a.mov(dword_ptr(DATA + 4), ebx)?;
            a.mov(dword_ptr(DATA + 8), edx)?;
            macro_rules! all {
                ($m:ident) => {
                    a.$m(eax, cl)?;
                    a.adc(ebp, eax)?;
                    a.$m(bx, cl)?;
                    a.$m(dl, cl)?;
                    a.$m(dh, cl)?;
                    a.pushfd()?;
                    a.pop(esi)?;
                    a.xor(ebp, esi)?;
                    a.$m(dword_ptr(DATA), cl)?;
                    a.$m(word_ptr(DATA + 4), cl)?;
                    a.$m(byte_ptr(DATA + 6), cl)?;
                    a.$m(dword_ptr(DATA + 8), 5)?;
                    a.$m(byte_ptr(DATA + 9), 1)?;
                    a.adc(ebp, ebx)?;
                    a.sbb(ebp, edx)?;
                };
            }
            all!(shl);
            all!(shr);
            all!(sar);
            all!(rol);
            all!(ror);
            a.shld(eax, ebx, cl)?;
            a.adc(ebp, eax)?;
            a.shrd(ebx, edx, cl)?;
            a.shld(si, bx, cl)?;
            a.shrd(word_ptr(DATA + 4), ax, cl)?;
            a.shld(dword_ptr(DATA), edx, cl)?;
            a.adc(ebp, esi)?;
            a.add(ebp, dword_ptr(DATA))?;
            a.add(ebp, dword_ptr(DATA + 4))?;
            a.add(ebp, dword_ptr(DATA + 8))?;
            a.add(ebp, ebx)?;
            a.add(ebp, edx)
        });
    });
    run_both(&mut a, &mut b);
}

#[test]
fn a_shift_by_a_cl_of_0_still_checks_its_operand_for_writing() {
    let (mut a, mut b) = twins(|rig| {
        rig.record(GP);
        // A read-only data segment.
        rig.set_gdt(FREE, seg_desc(0x40000, 0xFF, 0x90, 0x4));
        let code = asm32(CODE, |a| {
            a.mov(ax, FREE as u32)?;
            a.mov(ds, ax)?;
            a.xor(ecx, ecx)?;
            a.shl(dword_ptr(0x10), cl)?;
            a.mov(edx, 3u32)?;
            a.hlt()
        });
        rig.load(CODE, &code);
    });
    run_both(&mut a, &mut b);
    assert_eq!(b.recorded().0, GP as u32);
    assert_ne!(b.cpu.edx(), 3);
}

/// A program with IRQ 0 firing often, running `body` in a loop.
fn with_timer(rig: &mut Rig, body: impl Fn(&mut CodeAssembler) -> Result<(), IcedError>) {
    rig.handler(0x08, 0, |a| {
        a.push(eax)?;
        a.inc(edi)?;
        a.mov(al, 0x20)?;
        a.out(0x20, al)?;
        a.pop(eax)?;
        a.iretd()
    });
    let code = asm32(CODE, |a| {
        a.mov(al, 0xFE)?;
        a.out(0x21, al)?;
        a.mov(al, 0x34)?;
        a.out(0x43, al)?;
        a.mov(al, 0x61)?;
        a.out(0x40, al)?;
        a.mov(al, 0x00)?;
        a.out(0x40, al)?;
        a.xor(edi, edi)?;
        a.mov(ecx, 3000u32)?;
        a.sti()?;
        let mut top = a.create_label();
        a.set_label(&mut top)?;
        body(a)?;
        a.dec(ecx)?;
        a.jnz(top)?;
        a.cli()?;
        a.hlt()
    });
    rig.load(CODE, &code);
}

#[test]
fn interrupts_wait_out_the_shadows_of_sti_and_mov_ss() {
    let (mut a, mut b) = twins(|rig| {
        with_timer(rig, |a| {
            a.cli()?;
            a.inc(esi)?;
            a.inc(esi)?;
            a.sti()?;
            a.inc(ebp)?;
            a.mov(ax, ss)?;
            a.mov(ss, ax)?;
            a.inc(ebp)?;
            a.pushfd()?;
            a.popfd()?;
            a.inc(ebp)
        });
    });
    run_both(&mut a, &mut b);
    assert!(b.cpu.edi() > 10, "IRQ 0 came {} times", b.cpu.edi());
}

#[test]
fn timer_reads_and_reprogramming_between_blocks_keep_the_time() {
    let (mut a, mut b) = twins(|rig| {
        with_timer(rig, |a| {
            // Latch and read the count, and add it up.
            a.mov(al, 0x00)?;
            a.out(0x43, al)?;
            a.in_(al, 0x40)?;
            a.movzx(edx, al)?;
            a.in_(al, 0x40)?;
            a.add(esi, edx)?;
            a.imul_3(ebx, esi, 3)?;
            a.xor(ebx, ecx)
        });
    });
    run_both(&mut a, &mut b);
    assert_eq!(a.cpu.esi(), b.cpu.esi());
}

#[test]
fn a_full_code_memory_starts_over() {
    let (mut a, mut b) = twins(|rig| {
        // Many little blocks: a chain of 400 jumps, twice around.
        let mut code = Vec::new();
        for i in 0..400u32 {
            let at = CODE + 0x100 + i * 16;
            let next = if i == 399 { CODE + 0x100 + 400 * 16 } else { at + 16 };
            let mut block = asm32(at, |a| {
                a.inc(esi)?;
                a.jmp(next as u64)
            });
            block.resize(16, 0x90);
            code.extend(block);
        }
        code.extend(asm32(CODE + 0x100 + 400 * 16, |a| {
            a.dec(ecx)?;
            a.jnz(CODE as u64 + 0x100)?;
            a.hlt()
        }));
        rig.load(CODE + 0x100, &code);
        rig.load(CODE, &asm32(CODE, |a| {
            a.mov(ecx, 2u32)?;
            a.jmp(CODE as u64 + 0x100)
        }));
    });
    b.cpu.dynrec.set_code_size(16 << 10);
    let stats = run_both(&mut a, &mut b);
    assert_eq!(b.cpu.esi(), 800);
    if AVAILABLE {
        assert!(stats.flushes > 0, "{:?}", stats);
    }
}

#[test]
fn auto_uses_the_recompiler_from_protected_mode_until_the_program_ends() {
    let mut rig = Rig::new();
    rig.cpu.core = CoreMode::Auto;
    assert!(!rig.cpu.dynamic_active(), "real mode starts on the interpreter");
    rig.load(CODE, &asm32(CODE, |a| {
        a.xor(eax, eax)?;
        a.mov(ecx, 1000u32)?;
        let mut top = a.create_label();
        a.set_label(&mut top)?;
        a.inc(eax)?;
        a.dec(ecx)?;
        a.jnz(top)?;
        a.hlt()
    }));
    rig.enter_pm();
    assert_eq!(rig.cpu.dynamic_active(), AVAILABLE, "on the recompiler once in protected mode");
    rig.run_batched_to_halt();
    assert_eq!(rig.cpu.eax(), 1000);
    if AVAILABLE {
        assert!(rig.cpu.dynrec.stats().runs > 0);
    }
    // The program ends: back to the shell, on the interpreter.
    rig.cpu.load_shell();
    assert!(!rig.cpu.dynamic_active());
}

#[test]
fn changing_a_linked_block_unlinks_it() {
    // Two passes of a loop whose blocks link to each other; between them
    // the code rewrites the ADD's immediate in the block the jump leads
    // to, 1 then 16: EBX = 5 + 80.
    let b_at = CODE + 0x80;
    let (mut a, mut b) = twins(|rig| {
        let head = asm32(CODE, |a| {
            a.xor(ebx, ebx)?;
            a.mov(esi, 2u32)?;
            a.mov(ecx, 5u32)?;
            a.inc(edx)?;
            a.jmp(b_at as u64)
        });
        rig.load(CODE, &head);
        // The loop's top: the INC EDX (at 12 = 2 + 5 + 5).
        let top = CODE + 12;
        let body = asm32(b_at, |a| {
            a.db(&[0x83, 0xC3, 0x01])?; // add ebx, 1
            a.dec(ecx)?;
            a.jnz(top as u64)?;
            a.mov(byte_ptr(b_at as u64 + 2), 0x10)?;
            a.mov(ecx, 5u32)?;
            a.dec(esi)?;
            a.jnz(top as u64)?;
            a.hlt()
        });
        rig.load(b_at, &body);
    });
    let stats = run_both(&mut a, &mut b);
    assert_eq!(b.cpu.ebx(), 85);
    if AVAILABLE {
        assert!(stats.stale > 0, "{:?}", stats);
    }
}

#[test]
fn a_block_translated_again_takes_the_place_of_the_one_it_replaces() {
    // A loop that rewrites the ADD's immediate in the block it jumps to on
    // each of its 1000 passes, CL, as the Doom engine pokes its drawing
    // loops: 1000 translations of that block, in room for far fewer.
    let b_at = CODE + 0x80;
    let top = CODE + 0x40;
    let (mut a, mut b) = twins(|rig| {
        rig.load(CODE, &asm32(CODE, |a| {
            a.xor(ebx, ebx)?;
            a.mov(ecx, 1000u32)?;
            a.jmp(top as u64)
        }));
        rig.load(top, &asm32(top, |a| {
            a.mov(byte_ptr(b_at as u64 + 2), cl)?;
            a.jmp(b_at as u64)
        }));
        rig.load(b_at, &asm32(b_at, |a| {
            a.db(&[0x81, 0xC3, 0, 0, 0, 0])?; // add ebx, imm32
            a.dec(ecx)?;
            a.jnz(top as u64)?;
            a.hlt()
        }));
    });
    b.cpu.dynrec.set_code_size(16 << 10);
    let stats = run_both(&mut a, &mut b);
    assert_eq!(b.cpu.ebx(), (1..=1000u32).map(|i| i & 0xFF).sum::<u32>());
    if AVAILABLE {
        assert!(stats.stale >= 900, "{:?}", stats);
        assert_eq!(stats.flushes, 0, "{:?}", stats);
    }
}

#[test]
fn a_return_linked_to_one_place_after_another_leaves_none_behind() {
    // A function in another page returns to two places in turn, 500 times
    // each, so its return is linked to each in turn; and the loop rewrites
    // the function's ADD immediate, CL, on every pass, so its block is
    // translated again every time. Neither the links made before nor the
    // blocks thrown away may stay in the backlinks of the places.
    let f = CODE + 0x1000;
    let top = CODE + 0x40;
    let (mut a, mut b) = twins(|rig| {
        rig.load(f, &asm32(f, |a| {
            a.db(&[0x81, 0xC7, 0, 0, 0, 0])?; // add edi, imm32
            a.ret()
        }));
        rig.load(CODE, &asm32(CODE, |a| {
            a.xor(edi, edi)?;
            a.mov(ecx, 500u32)?;
            a.jmp(top as u64)
        }));
        rig.load(top, &asm32(top, |a| {
            a.mov(byte_ptr(f as u64 + 2), cl)?;
            a.call(f as u64)?;
            a.call(f as u64)?;
            a.dec(ecx)?;
            a.jnz(top as u64)?;
            a.hlt()
        }));
    });
    let stats = run_both(&mut a, &mut b);
    assert_eq!(b.cpu.edi(), 2 * (1..=500u32).map(|i| i & 0xFF).sum::<u32>());
    if AVAILABLE {
        assert!(stats.stale >= 450, "{:?}", stats);
        // At most three links from each block: none left from before.
        assert!(stats.links <= 3 * stats.live_blocks, "{:?}", stats);
    }
}

#[test]
fn a_smaller_cs_limit_stops_a_linked_block() {
    // A loop of two linked blocks runs under a flat code segment, then
    // the same code under one whose limit ends inside the JNZ: #GP(0)
    // there, as the interpreter has it.
    let b_at = CODE + 0x80;
    let (mut a, mut b) = twins(|rig| {
        rig.record(GP);
        rig.set_gdt(FREE, seg_desc(0, b_at + 1, CODE_R0, 0x4));
        let head = asm32(CODE, |a| {
            a.mov(ecx, 100u32)?;
            a.xor(esi, esi)?;
            a.inc(edx)?;
            a.jmp(b_at as u64)
        });
        rig.load(CODE, &head);
        let top = CODE + 7;
        let body = asm32(b_at, |a| {
            a.dec(ecx)?; // 1 byte, at the limit - 1
            a.jnz(top as u64)?; // 2 bytes: its last is past the limit
            a.inc(esi)?;
            a.mov(ecx, 100u32)?;
            a.cmp(esi, 1)?;
            a.jne(0x10400u64)?;
            // The second time around: under the small segment.
            a.db(&[0xEA])?;
            a.dd(&[top])?;
            a.dw(&[FREE])?;
            a.hlt()
        });
        rig.load(b_at, &body);
    });
    run_both(&mut a, &mut b);
    let (vector, stack) = b.recorded();
    assert_eq!((vector, stack[0], stack[1]), (GP as u32, 0, b_at + 1));
}

/// A loop that calls a function in another page 50 times, then maps that
/// page to another function (with INVLPG if `flush`) and calls it 50 times
/// more. The links from the loop to the function and back are made under
/// the first translation; after the remap they must go wherever the
/// interpreter's fetch goes: to the new function after INVLPG, and without
/// it to the old one the TLB still has.
fn remapped_callee(flush: bool) -> u32 {
    let (dir, table) = (0x80000u32, 0x81000u32);
    let (mut a, mut b) = twins(|rig| {
        rig.write32(dir, table | 3);
        for i in 0..1024u32 {
            rig.write32(table + 4 * i, (i << 12) | 3);
        }
        rig.load(0x31000, &asm32(0x31000, |a| {
            a.add(ebx, 1)?;
            a.ret()
        }));
        rig.load(0x32000, &asm32(0x31000, |a| {
            a.add(ebx, 100)?;
            a.ret()
        }));
        let code = asm32(CODE, |a| {
            a.mov(eax, dir)?;
            a.mov(cr3, eax)?;
            a.mov(eax, cr0)?;
            a.or(eax, 0x8000_0000u32)?;
            a.mov(cr0, eax)?;
            a.xor(ebx, ebx)?;
            a.mov(ecx, 50u32)?;
            let mut first = a.create_label();
            a.set_label(&mut first)?;
            a.call(0x31000u64)?;
            a.dec(ecx)?;
            a.jnz(first)?;
            a.mov(dword_ptr(table + 0x31 * 4), 0x32003u32)?;
            if flush {
                a.invlpg(ptr(0x31000))?;
            }
            a.mov(ecx, 50u32)?;
            let mut second = a.create_label();
            a.set_label(&mut second)?;
            a.call(0x31000u64)?;
            a.dec(ecx)?;
            a.jnz(second)?;
            a.hlt()
        });
        rig.load(CODE, &code);
    });
    run_both(&mut a, &mut b);
    b.cpu.ebx()
}

#[test]
fn links_to_another_page_follow_its_remapping() {
    assert_eq!(remapped_callee(true), 50 + 5000);
    assert_eq!(remapped_callee(false), 100, "the TLB keeps the old translation");
}
