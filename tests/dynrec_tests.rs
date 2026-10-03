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
            // The flags each leaves count too (a 486's, which the code
            // generators work out only where they are read).
            let div = |a: &mut CodeAssembler, len: u32, f: &dyn Fn(&mut CodeAssembler) -> Result<(), IcedError>| {
                a.mov(esi, len)?;
                f(a)?;
                a.pushfd()?;
                a.pop(edi)?;
                a.add(ebp, edi)?;
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
fn code_traced_with_tf_takes_a_single_step_trap_after_every_instruction() {
    // A loop runs translated, then traced by a #DB handler that counts
    // the traps, as programs that decrypt themselves step by step do.
    let (mut a, mut b) = twins(|rig| {
        rig.handler(1, 0, |a| {
            a.inc(esi)?;
            a.iretd()
        });
        let code = asm32(CODE, |a| {
            a.xor(esi, esi)?;
            a.mov(edx, 2u32)?;
            let mut pass = a.create_label();
            a.set_label(&mut pass)?;
            a.mov(ecx, 50u32)?;
            let mut untraced = a.create_label();
            a.set_label(&mut untraced)?;
            a.inc(ebx)?;
            a.dec(ecx)?;
            a.jnz(untraced)?;
            // TF on: the trap comes after the MOV after the POPFD.
            a.pushfd()?;
            a.or(dword_ptr(esp), 0x100)?;
            a.popfd()?;
            a.mov(ecx, 50u32)?;
            let mut traced = a.create_label();
            a.set_label(&mut traced)?;
            a.inc(ebx)?;
            a.dec(ecx)?;
            a.jnz(traced)?;
            // TF off: the POPFD that clears it still traps.
            a.pushfd()?;
            a.and(dword_ptr(esp), !0x100)?;
            a.popfd()?;
            a.dec(edx)?;
            a.jnz(pass)?;
            a.hlt()
        });
        rig.load(CODE, &code);
    });
    run_both(&mut a, &mut b);
    // Per pass: the MOV, 50 times around the loop, and PUSHFD, AND, POPFD.
    assert_eq!(b.cpu.esi(), 2 * (1 + 150 + 3));
}

#[test]
fn port_io_that_lets_an_interrupt_through_stops_the_block_after_it() {
    // Blocks go on past IN, OUT and STI where they change nothing the
    // execution loop checks. Here the loop masks IRQ 0 and unmasks it in
    // the middle of a block, and reads the mask, in turn: once IRQ 0 is
    // waiting, the unmasking OUT lets it in before the next instruction,
    // as the interpreter does, and the IN in its shadow changes nothing.
    let (mut a, mut b) = twins(|rig| {
        with_timer(rig, |a| {
            a.mov(al, 0xFF)?;
            a.out(0x21, al)?;
            a.inc(esi)?;
            a.inc(esi)?;
            a.mov(al, 0xFE)?;
            a.out(0x21, al)?;
            a.mov(ebx, edi)?;
            a.in_(al, 0x21)?;
            a.add(ebp, ebx)
        });
    });
    let stats = run_both(&mut a, &mut b);
    assert!(b.cpu.edi() > 10, "IRQ 0 came {} times", b.cpu.edi());
    if AVAILABLE {
        // Going back to the execution loop at every IN and OUT would take
        // over 9000 runs.
        assert!(stats.runs < 2000, "{:?}", stats);
    }
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

/// Code that writes CL into every byte of the immediates of `add ebx,
/// imm32` and `add edx, imm32` at `at`: new code each time CL changes
/// (eight bytes of it), not a poke.
fn rewrite_immediates(a: &mut CodeAssembler, at: u32) -> Result<(), IcedError> {
    a.movzx(eax, cl)?;
    a.imul_3(eax, eax, 0x0101_0101)?;
    a.mov(dword_ptr(at as u64 + 2), eax)?;
    a.mov(dword_ptr(at as u64 + 8), eax)
}

/// What EBX sums to after `add ebx, imm32` with CL in every byte of the
/// immediate, for CL of `count` down to 1.
fn replicated_sum(count: u32) -> u32 {
    (1..=count).fold(0u32, |sum, i| sum.wrapping_add((i & 0xFF).wrapping_mul(0x0101_0101)))
}

#[test]
fn a_block_translated_again_takes_the_place_of_the_one_it_replaces() {
    // A loop that rewrites two immediates in the block it jumps to on each
    // of its 1000 passes: 1000 translations of that block, in room for far
    // fewer.
    let b_at = CODE + 0x80;
    let top = CODE + 0x40;
    let (mut a, mut b) = twins(|rig| {
        rig.load(CODE, &asm32(CODE, |a| {
            a.xor(ebx, ebx)?;
            a.mov(ecx, 1000u32)?;
            a.jmp(top as u64)
        }));
        rig.load(top, &asm32(top, |a| {
            rewrite_immediates(a, b_at)?;
            a.jmp(b_at as u64)
        }));
        rig.load(b_at, &asm32(b_at, |a| {
            a.db(&[0x81, 0xC3, 0, 0, 0, 0])?; // add ebx, imm32
            a.db(&[0x81, 0xC2, 0, 0, 0, 0])?; // add edx, imm32
            a.dec(ecx)?;
            a.jnz(top as u64)?;
            a.hlt()
        }));
    });
    b.cpu.dynrec.set_code_size(16 << 10);
    let stats = run_both(&mut a, &mut b);
    assert_eq!(b.cpu.ebx(), replicated_sum(1000));
    if AVAILABLE {
        assert!(stats.stale >= 900, "{:?}", stats);
        assert_eq!(stats.flushes, 0, "{:?}", stats);
    }
}

#[test]
fn a_return_linked_to_one_place_after_another_leaves_none_behind() {
    // A function in another page returns to two places in turn, 500 times
    // each, so its return is linked to each in turn; and the loop rewrites
    // the function's immediates on every pass, so its block is translated
    // again every time. Neither the links made before nor the blocks
    // thrown away may stay in the backlinks of the places.
    let f = CODE + 0x1000;
    let top = CODE + 0x40;
    let (mut a, mut b) = twins(|rig| {
        rig.load(f, &asm32(f, |a| {
            a.db(&[0x81, 0xC3, 0, 0, 0, 0])?; // add ebx, imm32
            a.db(&[0x81, 0xC2, 0, 0, 0, 0])?; // add edx, imm32
            a.ret()
        }));
        rig.load(CODE, &asm32(CODE, |a| {
            a.xor(ebx, ebx)?;
            a.mov(ecx, 500u32)?;
            a.jmp(top as u64)
        }));
        rig.load(top, &asm32(top, |a| {
            rewrite_immediates(a, f)?;
            a.call(f as u64)?;
            a.call(f as u64)?;
            a.dec(ecx)?;
            a.jnz(top as u64)?;
            a.hlt()
        }));
    });
    let stats = run_both(&mut a, &mut b);
    assert_eq!(b.cpu.ebx(), replicated_sum(500).wrapping_mul(2));
    if AVAILABLE {
        assert!(stats.stale >= 450, "{:?}", stats);
        // At most six links from each block: none left from before.
        assert!(stats.links <= 6 * stats.live_blocks, "{:?}", stats);
    }
}

#[test]
fn a_function_returning_to_two_places_in_turn_goes_back_to_each_through_its_links() {
    // A function in another page called from two places in a loop, 1000
    // times each: its return is linked to both, so the translated code
    // runs the loop without the execution loop.
    let f = CODE + 0x1000;
    let top = CODE + 0x40;
    let (mut a, mut b) = twins(|rig| {
        rig.load(f, &asm32(f, |a| {
            a.add(ebx, eax)?;
            a.ret()
        }));
        rig.load(CODE, &asm32(CODE, |a| {
            a.xor(ebx, ebx)?;
            a.mov(eax, 1u32)?;
            a.mov(ecx, 1000u32)?;
            a.jmp(top as u64)
        }));
        rig.load(top, &asm32(top, |a| {
            a.call(f as u64)?;
            a.inc(eax)?;
            a.call(f as u64)?;
            a.dec(ecx)?;
            a.jnz(top as u64)?;
            a.hlt()
        }));
    });
    let stats = run_both(&mut a, &mut b);
    assert_eq!(b.cpu.ebx(), (1..=1000u32).map(|i| 2 * i + 1).sum::<u32>());
    if AVAILABLE {
        assert!(stats.runs < 200, "the returns went through the execution loop: {:?}", stats);
    }
}

#[test]
fn a_function_returning_to_many_places_goes_back_through_the_engines_table() {
    // A function in another page called from twelve places in a loop, 500
    // times each: more places than its return has links, so it goes back
    // to them through the engine's table of places (`Return`), without the
    // execution loop. The instruction at the sixth place is rewritten on
    // every pass, an ADD and an IMUL in turn (too many bytes for a poke),
    // so the block there is translated again each time, to another place
    // in the code memory: its place in the table must go with it.
    let f = CODE + 0x1000;
    let top = CODE + 0x40;
    // The instruction after the sixth call: after the stores (40 bytes),
    // five calls and INCs (6 bytes each) and the call (5).
    let at = top + 40 + 5 * 6 + 5;
    let (mut a, mut b) = twins(|rig| {
        rig.load(f, &asm32(f, |a| {
            a.add(ebx, eax)?;
            a.ret()
        }));
        rig.load(CODE, &asm32(CODE, |a| {
            a.xor(ebx, ebx)?;
            a.mov(edx, 1u32)?;
            a.mov(eax, 1u32)?;
            a.mov(ecx, 500u32)?;
            a.jmp(top as u64)
        }));
        rig.load(top, &asm32(top, |a| {
            // movzx esi, cl; imul esi, esi, 01010101h; mov [at + 2], esi
            a.db(&[0x0F, 0xB6, 0xF1, 0x69, 0xF6, 1, 1, 1, 1, 0x89, 0x35])?;
            a.db(&(at + 2).to_le_bytes())?;
            // movzx edi, cl; and edi, 1; imul edi, edi, 0FE8h;
            // add edi, 0C281h: 81 C2 (add edx) or 69 D2 (imul edx, edx)
            a.db(&[0x0F, 0xB6, 0xF9, 0x83, 0xE7, 0x01, 0x69, 0xFF, 0xE8, 0x0F, 0, 0, 0x81, 0xC7, 0x81, 0xC2, 0, 0])?;
            // mov [at], di
            a.db(&[0x66, 0x89, 0x3D])?;
            a.db(&at.to_le_bytes())?;
            for k in 0..12 {
                a.call(f as u64)?;
                if k == 5 {
                    a.db(&[0x81, 0xC2, 0, 0, 0, 0])?; // add edx, imm32
                }
                a.db(&[0x40])?; // inc eax
            }
            a.dec(ecx)?;
            a.jnz(top as u64)?;
            a.hlt()
        }));
    });
    let stats = run_both(&mut a, &mut b);
    assert_eq!(b.cpu.ebx(), (1..=6000u32).sum::<u32>());
    let product = (1..=500u32).rev().fold(1u32, |sum, i| {
        let v = (i & 0xFF).wrapping_mul(0x0101_0101);
        if i & 1 != 0 { sum.wrapping_mul(v) } else { sum.wrapping_add(v) }
    });
    assert_eq!(b.cpu.edx(), product);
    if AVAILABLE {
        assert!(stats.stale >= 450, "{:?}", stats);
        assert!(stats.runs < 2500, "the returns went through the execution loop: {:?}", stats);
    }
}

#[test]
fn indirect_calls_are_translated_and_linked_to_where_they_go() {
    // A loop that calls two functions in another page in turn through a
    // table of pointers, and one of them through a register, 1000 times:
    // each call's link leads to the functions it went to.
    let f = CODE + 0x1000;
    let g = CODE + 0x1010;
    let table = CODE + 0x800;
    let top = CODE + 0x40;
    let (mut a, mut b) = twins(|rig| {
        rig.load(f, &asm32(f, |a| {
            a.add(ebx, 1)?;
            a.ret()
        }));
        rig.load(g, &asm32(g, |a| {
            a.add(ebx, 100)?;
            a.ret()
        }));
        rig.write32(table, f);
        rig.write32(table + 4, g);
        rig.load(CODE, &asm32(CODE, |a| {
            a.xor(ebx, ebx)?;
            a.xor(esi, esi)?;
            a.mov(edx, g)?;
            a.mov(ecx, 1000u32)?;
            a.jmp(top as u64)
        }));
        rig.load(top, &asm32(top, |a| {
            a.call(dword_ptr(esi * 4 + table))?;
            a.xor(esi, 1)?;
            a.call(edx)?;
            a.dec(ecx)?;
            a.jnz(top as u64)?;
            a.hlt()
        }));
    });
    let stats = run_both(&mut a, &mut b);
    assert_eq!(b.cpu.ebx(), 500 * 1 + 500 * 100 + 1000 * 100);
    if AVAILABLE {
        assert!(stats.runs < 200, "the calls went through the execution loop: {:?}", stats);
    }
}

#[test]
fn indirect_jumps_are_translated_and_linked_to_where_they_go() {
    // A loop that jumps through a table of three places in another page,
    // in turn, and from each back through a register, 1000 times; one
    // jump is past the CS limit at the end: #GP, with the instructions
    // before it done.
    let cases = CODE + 0x1000;
    let table = CODE + 0x800;
    let top = CODE + 0x40;
    let back = CODE + 0x80;
    let (mut a, mut b) = twins(|rig| {
        for k in 0..3u32 {
            let at = cases + 0x10 * k;
            rig.load(at, &asm32(at, |a| {
                a.add(ebx, 1 << (8 * k))?;
                a.jmp(edx)
            }));
            rig.write32(table + 4 * k, at);
        }
        rig.write32(table + 12, 0xFFFF_0000);
        rig.record(13);
        rig.load(CODE, &asm32(CODE, |a| {
            a.xor(ebx, ebx)?;
            a.xor(esi, esi)?;
            a.mov(edx, back)?;
            a.mov(ecx, 1000u32)?;
            a.jmp(top as u64)
        }));
        rig.load(top, &asm32(top, |a| {
            a.jmp(dword_ptr(esi * 4 + table))
        }));
        rig.load(back, &asm32(back, |a| {
            let mut next = a.create_label();
            a.inc(esi)?;
            a.cmp(esi, 3)?;
            a.jne(next)?;
            a.xor(esi, esi)?;
            a.set_label(&mut next)?;
            a.dec(ecx)?;
            a.jnz(top as u64)?;
            a.mov(esi, 3u32)?;
            a.mov(ebp, 7u32)?;
            a.jmp(dword_ptr(esi * 4 + table))
        }));
    });
    let stats = run_both(&mut a, &mut b);
    assert_eq!(b.cpu.ebx(), 334 + (333 << 8) + (333 << 16));
    let (vector, _) = b.recorded();
    assert_eq!(vector, 13);
    assert_eq!(b.cpu.ebp(), 7);
    if AVAILABLE {
        assert!(stats.runs < 200, "the jumps went through the execution loop: {:?}", stats);
    }
}

#[test]
fn rep_movs_and_stos_of_a_few_elements_run_as_translated_loops() {
    // REP MOVS and STOS of each size with counts of 0 to 17 (the last more
    // than a translated loop does, which the handler runs), overlapping
    // moves, MOVS and STOS without REP, going down (DF set, which the
    // handler runs), and with 16-bit addressing wrapping around at 64 KB,
    // 100 times over.
    let (mut a, mut b) = twins(|rig| {
        let bytes: Vec<u8> = (0..0x800u32).map(|i| (i * 7 + 3) as u8).collect();
        rig.load(DATA, &bytes);
        rig.load(CODE, &asm32(CODE, |a| {
            let mut top = a.create_label();
            let mut inner = a.create_label();
            a.xor(ebx, ebx)?;
            a.mov(ebp, 100u32)?;
            a.set_label(&mut top)?;
            a.xor(edx, edx)?;
            a.set_label(&mut inner)?;
            a.mov(esi, DATA + 0x100)?;
            a.mov(edi, DATA + 0x2000)?;
            a.mov(ecx, edx)?;
            a.rep().movsb()?;
            a.mov(esi, DATA + 0x100)?;
            a.mov(edi, DATA + 0x3001)?;
            a.mov(ecx, edx)?;
            a.rep().movsd()?;
            a.mov(ecx, edx)?;
            a.rep().movsw()?;
            // Each byte onto the next.
            a.mov(esi, DATA + 0x400)?;
            a.lea(edi, dword_ptr(esi + 1))?;
            a.mov(ecx, edx)?;
            a.rep().movsb()?;
            a.mov(edi, DATA + 0x5000)?;
            a.imul_3(eax, edx, 0x0101_0101)?;
            a.mov(ecx, edx)?;
            a.rep().stosw()?;
            a.mov(ecx, edx)?;
            a.rep().stosd()?;
            a.mov(ecx, edx)?;
            a.rep().stosb()?;
            a.movsd()?;
            a.stosb()?;
            a.movsb()?;
            a.add(ebx, esi)?;
            a.add(ebx, edi)?;
            a.add(ebx, ecx)?;
            a.inc(edx)?;
            a.cmp(edx, 18)?;
            a.jb(inner)?;
            // Going down.
            a.std()?;
            a.mov(esi, DATA + 0x10F)?;
            a.mov(edi, DATA + 0x600F)?;
            a.mov(ecx, 5u32)?;
            a.rep().movsb()?;
            a.movsw()?;
            a.stosb()?;
            a.cld()?;
            // 16-bit addressing: ADDR16 REP MOVSB, REP STOSW.
            a.mov(esi, 0x1234_FFFAu32)?;
            a.mov(edi, 0x5678_FFFCu32)?;
            a.mov(ecx, 0xABCD_0009u32)?;
            a.db(&[0x67, 0xF3, 0xA4])?;
            a.mov(edi, 0xFFFBu32)?;
            a.mov(ecx, 3u32)?;
            a.db(&[0x67, 0xF3, 0x66, 0xAB])?;
            a.add(ebx, esi)?;
            a.add(ebx, edi)?;
            a.dec(ebp)?;
            a.jnz(top)?;
            a.hlt()
        }));
    });
    run_both(&mut a, &mut b);
    // The first bytes moved, and the first byte over and over.
    assert_eq!(b.read32(DATA + 0x2000), a.read32(DATA + 0x100));
    assert_eq!(b.read32(DATA + 0x400), u32::from_le_bytes([b.read32(DATA + 0x400) as u8; 4]));
}

/// A handler for `vector` that keeps ECX, ESI and EDI at RESULT + 40h, 44h
/// and 48h, then records it (`Rig::record`).
fn record_indexes(rig: &mut Rig, vector: u8) {
    rig.handler(vector, 0, |a| {
        a.mov(dword_ptr(RESULT + 0x40), ecx)?;
        a.mov(dword_ptr(RESULT + 0x44), esi)?;
        a.mov(dword_ptr(RESULT + 0x48), edi)?;
        record_code(a, vector)
    });
}

/// ECX, ESI and EDI as `record_indexes` kept them.
fn recorded_indexes(rig: &Rig) -> (u32, u32, u32) {
    (rig.read32(RESULT + 0x40), rig.read32(RESULT + 0x44), rig.read32(RESULT + 0x48))
}

#[test]
fn a_translated_rep_stos_past_the_segment_limit_faults_where_it_is_reached() {
    // REP STOSD of 8 dwords from 1FF0h in an ES of 8 KB: #GP in the fifth
    // iteration, with the four before it done and counted.
    let (mut a, mut b) = twins(|rig| {
        rig.set_gdt(FREE, seg_desc(0x60000, 0x1FFF, DATA_R0, 0x4));
        record_indexes(rig, GP);
        rig.load(CODE, &asm32(CODE, |a| {
            a.mov(ax, FREE as u32)?;
            a.mov(es, ax)?;
            a.mov(esi, 0x1234u32)?;
            a.mov(edi, 0x1FF0u32)?;
            a.mov(ecx, 8u32)?;
            a.mov(eax, 0x5555_AAAAu32)?;
            a.rep().stosd()?;
            a.hlt()
        }));
    });
    run_both(&mut a, &mut b);
    assert_eq!(b.recorded().0, GP as u32);
    assert_eq!(recorded_indexes(&b), (4, 0x1234, 0x2000));
    assert_eq!((b.read32(0x61FFC), b.read32(0x62000)), (0x5555_AAAA, 0));
}

#[test]
fn a_translated_rep_movs_into_a_missing_page_faults_where_it_is_reached() {
    // REP MOVSD of 8 dwords from 54FF0h with page 55000h not present: #PF
    // in the fifth iteration, with CR2 on it.
    let (mut a, mut b) = twins(|rig| {
        let (dir, table) = (0x80000u32, 0x81000u32);
        rig.write32(dir, table | 3);
        for i in 0..1024u32 {
            rig.write32(table + 4 * i, (i << 12) | 3);
        }
        rig.write32(table + 0x55 * 4, 0);
        record_indexes(rig, PF);
        rig.load(CODE, &asm32(CODE, |a| {
            a.mov(eax, dir)?;
            a.mov(cr3, eax)?;
            a.mov(eax, cr0)?;
            a.or(eax, 0x8000_0000u32)?;
            a.mov(cr0, eax)?;
            a.mov(esi, 0x53000u32)?;
            a.mov(edi, 0x54FF0u32)?;
            a.mov(ecx, 8u32)?;
            a.rep().movsd()?;
            a.hlt()
        }));
    });
    run_both(&mut a, &mut b);
    assert_eq!(b.recorded().0, PF as u32);
    assert_eq!(b.cpu.cr2, 0x55000);
    assert_eq!(recorded_indexes(&b), (4, 0x53010, 0x55000));
}

#[test]
fn a_translated_rep_stos_over_the_rest_of_its_block_leaves_after_it() {
    // REP STOSB of 5 NOPs over the MOV EAX, 1 after it in the block: the
    // MOV doesn't run.
    let at = CODE + 0x200;
    let (mut a, mut b) = twins(|rig| {
        rig.load(CODE, &asm32(CODE, |a| {
            a.mov(eax, 7u32)?;
            a.jmp(at as u64)
        }));
        let mut code = vec![0xBF];
        code.extend((at + 14).to_le_bytes()); // mov edi, at + 14
        code.extend([0xB0, 0x90]); // mov al, 90h
        code.extend([0xB9, 5, 0, 0, 0]); // mov ecx, 5
        code.extend([0xF3, 0xAA]); // rep stosb
        code.extend([0xB8, 1, 0, 0, 0]); // mov eax, 1
        code.push(0xF4);
        rig.load(at, &code);
    });
    run_both(&mut a, &mut b);
    assert_eq!(b.cpu.eax() & 0xFFFF_FF00, 0);
    assert_eq!(b.cpu.eax(), 0x90);
}

#[test]
fn translated_rep_movs_and_stos_reach_the_video_memory() {
    // REP STOSB and MOVSD into the video memory (chained, at A0000h as
    // mode 13h has it), and REP MOVSB back out of it into RAM, a few
    // elements at a time.
    let (mut a, mut b) = twins(|rig| {
        let bytes: Vec<u8> = (0..0x100u32).map(|i| (i * 5 + 1) as u8).collect();
        rig.load(DATA, &bytes);
        rig.load(CODE, &asm32(CODE, |a| {
            a.mov(dx, 0x3C4u32)?;
            a.mov(ax, 0x0F02u32)?;
            a.out(dx, ax)?;
            a.mov(ax, 0x0E04u32)?;
            a.out(dx, ax)?;
            a.mov(dx, 0x3CEu32)?;
            a.mov(ax, 0x0506u32)?;
            a.out(dx, ax)?;
            a.mov(ax, 0x4005u32)?;
            a.out(dx, ax)?;
            a.mov(edi, 0xA0000u32)?;
            a.mov(ecx, 9u32)?;
            a.mov(al, 0x3Cu32)?;
            a.rep().stosb()?;
            a.mov(esi, DATA)?;
            a.mov(ecx, 5u32)?;
            a.rep().movsd()?;
            a.mov(esi, 0xA0000u32)?;
            a.mov(edi, DATA + 0x200)?;
            a.mov(ecx, 16u32)?;
            a.rep().movsb()?;
            a.hlt()
        }));
    });
    run_both(&mut a, &mut b);
    // Nine bytes stored, then the dwords moved after them.
    assert_eq!((b.read32(DATA + 0x200), b.read32(DATA + 0x204)), (0x3C3C_3C3C, 0x3C3C_3C3C));
    assert_eq!(b.read32(DATA + 0x208) & 0xFF, 0x3C);
    assert_eq!(b.read32(DATA + 0x209), b.read32(DATA));
}

#[test]
fn flags_set_in_one_block_reach_the_blocks_linked_after_it() {
    // Blocks that end right after setting the flags (by an instruction
    // its handler runs, RCL, too), and blocks linked after them that start
    // by reading them: ADC, a conditional jump in another page, and RCR,
    // 3000 times. The timer deadline stops the chain before each of them
    // in turn.
    let top = CODE + 0x40;
    let second = CODE + 0x100;
    let third = CODE + 0x1000;
    let (mut a, mut b) = twins(|rig| {
        rig.load(CODE, &asm32(CODE, |a| {
            a.xor(ebx, ebx)?;
            a.xor(esi, esi)?;
            a.xor(ebp, ebp)?;
            a.mov(edi, 0x9E37_79B9u32)?;
            a.mov(ecx, 3000u32)?;
            a.jmp(top as u64)
        }));
        rig.load(top, &asm32(top, |a| {
            a.add(edi, 0x6D2B_79F5u32)?;
            a.rcl(edx, 1)?;
            a.jmp(second as u64)
        }));
        rig.load(second, &asm32(second, |a| {
            a.adc(ebx, 0)?;
            a.add(esi, edi)?;
            a.jmp(third as u64)
        }));
        rig.load(third, &asm32(third, |a| {
            let mut skip = a.create_label();
            a.jae(skip)?;
            a.inc(ebp)?;
            a.set_label(&mut skip)?;
            a.rcr(eax, 1)?;
            a.sub(ecx, 1)?;
            a.jnz(top as u64)?;
            a.hlt()
        }));
    });
    run_both(&mut a, &mut b);
    // Carries both ways.
    assert!((1..3000).contains(&b.cpu.ebx()), "{}", b.cpu.ebx());
    assert!((1..3000).contains(&b.cpu.ebp()), "{}", b.cpu.ebp());
}

#[test]
fn the_instructions_before_a_deadline_a_block_doesnt_fit_get_no_blocks_of_their_own() {
    // A loop of 41 instructions, 3000 times, in batches of 25 rounds: the
    // end of each comes at the same place in it, where the block doesn't
    // fit, and the interpreter runs the instructions up to it without a
    // block starting at each.
    let (mut a, mut b) = twins(|rig| {
        rig.load(CODE, &asm32(CODE, |a| {
            let mut top = a.create_label();
            a.mov(ecx, 3000u32)?;
            a.set_label(&mut top)?;
            for k in 0..39u32 {
                a.add(eax, k)?;
            }
            a.dec(ecx)?;
            a.jnz(top)?;
            a.hlt()
        }));
    });
    lockstep_with(&mut a.cpu, &mut b.cpu, 2000, 41 * 25, true, |_, _| {}).unwrap();
    assert!(halted(&a.cpu));
    let stats = b.cpu.dynrec.stats();
    if AVAILABLE {
        assert!(stats.deadline > 100, "{:?}", stats);
        assert!(stats.live_blocks < 30, "{:?}", stats);
    }
}

#[test]
fn translated_stack_operations_that_fault_leave_the_stack_pointer_as_it_was() {
    // After a few instructions in the block: POP into memory past DS's
    // limit, PUSH of memory there, ENTER with a frame past SS's limit
    // (after pushing, where the fault's frame then goes), and LEAVE with a
    // frame pointer past it. The fault's handler keeps EBP and ESP.
    for case in 0..4 {
        let (mut a, mut b) = twins(|rig| {
            // DS: 4 KB at DATA; SS: 64 KB at 60000h.
            rig.set_gdt(FREE, seg_desc(DATA, 0xFFF, DATA_R0, 0x4));
            rig.set_gdt(FREE + 8, seg_desc(0x60000, 0xFFFF, DATA_R0, 0x4));
            for vector in [GP, 12] {
                rig.handler(vector, 0, |a| {
                    a.mov(ax, DATA32 as u32)?;
                    a.mov(es, ax)?;
                    a.mov(dword_ptr(RESULT + 0x40).es(), ebp)?;
                    a.mov(dword_ptr(RESULT + 0x44).es(), esp)?;
                    record_code(a, vector)
                });
            }
            rig.load(CODE, &asm32(CODE, |a| {
                a.mov(ax, FREE as u32)?;
                a.mov(ds, ax)?;
                a.mov(ax, (FREE + 8) as u32)?;
                a.mov(ss, ax)?;
                a.mov(esp, 0x100u32)?;
                a.mov(ebp, 0x80u32)?;
                a.push(0x1234_5678u32)?;
                a.inc(ebx)?;
                match case {
                    0 => a.pop(dword_ptr(0xFFE))?,
                    1 => a.push(dword_ptr(0xFFE))?,
                    2 => a.enter(0x200u32, 0u32)?,
                    _ => {
                        a.mov(ebp, 0x1_0000u32)?;
                        a.leave()?
                    }
                }
                a.hlt()
            }));
        });
        run_both(&mut a, &mut b);
        let (vector, _) = b.recorded();
        assert_eq!(vector, if case < 2 { GP } else { 12 } as u32, "case {}", case);
        // The handler's ESP: the faulting one less the frame (EFLAGS, CS,
        // EIP, error code).
        let frame = if case == 3 { 0x1_0000 } else { 0x80 };
        assert_eq!((b.read32(RESULT + 0x40), b.read32(RESULT + 0x44)), (frame, 0xFC - 16), "case {}", case);
    }
}

#[test]
fn an_indirect_call_through_a_pointer_it_cant_read_pushes_nothing() {
    // CALL [200h] in a data segment of 256 bytes: #GP before the return
    // address is pushed, after the instructions before it in the block.
    let before = |a: &mut CodeAssembler| {
        a.mov(ax, FREE as u32)?;
        a.mov(ds, ax)?;
        a.mov(ebx, 1u32)?;
        a.push(0x1234u32)
    };
    let faulting = CODE + 0x100 + asm32(CODE + 0x100, before).len() as u32;
    let (mut a, mut b) = twins(|rig| {
        rig.record(GP);
        rig.set_gdt(FREE, seg_desc(0x40000, 0xFF, DATA_R0, 0x4));
        rig.load(CODE, &asm32(CODE, |a| a.jmp(CODE as u64 + 0x100)));
        rig.load(CODE + 0x100, &asm32(CODE + 0x100, |a| {
            before(a)?;
            a.call(dword_ptr(0x200))?;
            a.hlt()
        }));
    });
    run_both(&mut a, &mut b);
    let (vector, stack) = b.recorded();
    assert_eq!(vector, GP as u32);
    assert_eq!(stack[1], faulting, "EIP of the faulting CALL");
    assert_eq!(b.cpu.ebx(), 1);
    assert_eq!(b.cpu.read_linear_u16(STACK0_TOP - 4), 0x1234, "the PUSH before it happened");
}

#[test]
fn pushad_and_popad_are_translated() {
    // A loop that saves the registers, changes them and loads them back,
    // 1000 times, with PUSHA and POPA of 16 bits once.
    let (mut a, mut b) = twins(|rig| {
        rig.load(CODE, &asm32(CODE, |a| {
            a.mov(eax, 1u32)?;
            a.mov(ebx, 2u32)?;
            a.mov(edx, 3u32)?;
            a.mov(ebp, 4u32)?;
            a.mov(esi, 5u32)?;
            a.mov(edi, 6u32)?;
            a.mov(ecx, 1000u32)?;
            a.jmp(CODE as u64 + 0x100)
        }));
        rig.load(CODE + 0x100, &asm32(CODE + 0x100, |a| {
            a.pushad()?;
            a.add(dword_ptr(esp + 28), 1)?; // EAX's slot
            a.xor(eax, eax)?;
            a.xor(ebx, ebx)?;
            a.popad()?;
            a.pusha()?;
            a.popa()?;
            a.dec(ecx)?;
            a.jnz(CODE as u64 + 0x100)?;
            a.hlt()
        }));
    });
    let stats = run_both(&mut a, &mut b);
    assert_eq!((b.cpu.eax(), b.cpu.ebx(), b.cpu.esp()), (1001, 2, STACK0_TOP));
    if AVAILABLE {
        assert_eq!(untranslated(&stats), 0, "{:?}", stats);
    }
}

/// Ring 3 code on a stack of `limit` + 1 bytes at 50000h from ESP
/// `stack_top`, which runs `body` with the registers set to 11h, 22h, ...,
/// and the #SS it raises recorded.
fn on_a_small_stack(limit: u32, stack_top: u32, body: impl Fn(&mut CodeAssembler) -> Result<(), IcedError>) -> (Rig, Rig) {
    twins(|rig| {
        rig.record(12);
        rig.set_gdt(FREE, seg_desc(0x50000, limit, DATA_R3, 0x4));
        for i in 0..16u32 {
            rig.write32(0x50FF0 + 4 * i, 0xA0 + i);
        }
        rig.load(CODE, &asm32(CODE, to_ring3));
        rig.ring3(|a| {
            a.mov(ax, (FREE | 3) as u32)?;
            a.mov(ss, ax)?;
            a.mov(esp, stack_top)?;
            a.mov(eax, 0x11u32)?;
            a.mov(ecx, 0x22u32)?;
            a.mov(edx, 0x33u32)?;
            a.mov(ebx, 0x44u32)?;
            a.mov(ebp, 0x66u32)?;
            a.mov(esi, 0x77u32)?;
            a.mov(edi, 0x88u32)?;
            body(a)?;
            a.hlt()
        });
    })
}

#[test]
fn a_pushad_past_the_stack_limit_writes_the_slots_below_it() {
    // Slots from EDI's at FF0h up; ECX's at 1008h is past the limit.
    let (mut a, mut b) = on_a_small_stack(0x1007, 0x1010, |a| a.pushad());
    run_both(&mut a, &mut b);
    let (vector, _) = b.recorded();
    assert_eq!(vector, 12);
    assert_eq!((b.read32(0x50FF0), b.read32(0x51004)), (0x88, 0x33), "EDI's and EDX's slots");
    assert_eq!(b.read32(0x51008), 0xA6, "ECX's slot isn't written");
}

#[test]
fn a_popad_past_the_stack_limit_loads_the_registers_below_it() {
    // EDI, ESI, EBP and ESP's image from FF0h up, EBX's at 1000h is past
    // the limit. (The recording handler changes EAX, ECX, ESI and EDI.)
    let (mut a, mut b) = on_a_small_stack(0xFFF, 0xFF0, |a| a.popad());
    run_both(&mut a, &mut b);
    let (vector, _) = b.recorded();
    assert_eq!(vector, 12);
    assert_eq!((b.cpu.ebp(), b.cpu.ebx(), b.cpu.edx()), (0xA2, 0x44, 0x33));
}

/// How many instructions of a run went through their handlers beyond
/// what getting to protected mode takes.
fn untranslated(stats: &DynStats) -> u64 {
    let (mut a, mut b) = twins(|rig| rig.load(CODE, &asm32(CODE, |a| a.nop().and_then(|_| a.hlt()))));
    let setup = run_both(&mut a, &mut b);
    (stats.instructions - stats.native) - (setup.instructions - setup.native)
}

#[test]
fn selectors_are_read_and_pushed_in_translated_code() {
    // MOV r/m, Sreg and PUSH Sreg of both sizes (a 32-bit push writes the
    // selector's word of the slot), and CLI, 100 times.
    let (mut a, mut b) = twins(|rig| {
        rig.load(CODE, &asm32(CODE, |a| {
            a.mov(ecx, 100u32)?;
            a.mov(ebx, 0xFFFF_FFFFu32)?;
            a.jmp(CODE as u64 + 0x100)
        }));
        rig.load(CODE + 0x100, &asm32(CODE + 0x100, |a| {
            a.mov(word_ptr(DATA), ds)?;
            a.mov(ebx, ss)?;
            a.mov(si, es)?;
            a.push(0xFFFF_FFFFu32)?;
            a.pop(eax)?;
            a.push(fs)?; // PUSHD FS over the slot of FFFFFFFFh
            a.pop(eax)?;
            a.db(&[0x66, 0x0F, 0xA8])?; // PUSHW GS
            a.pop(dx)?;
            a.cli()?;
            a.dec(ecx)?;
            a.jnz(CODE as u64 + 0x100)?;
            a.hlt()
        }));
    });
    let stats = run_both(&mut a, &mut b);
    assert_eq!(b.cpu.eax(), 0xFFFF_0000 | DATA32 as u32, "PUSHD FS writes the low word");
    assert_eq!((b.cpu.ebx(), b.cpu.esi() & 0xFFFF), (DATA32 as u32, DATA32 as u32));
    assert_eq!(b.read32(DATA) & 0xFFFF, DATA32 as u32);
    if AVAILABLE {
        assert_eq!(untranslated(&stats), 0, "{:?}", stats);
    }
}

/// Ring 3 code that runs CLI with IOPL `iopl`, and the #GP it may raise
/// recorded.
fn cli_at_ring_3(iopl: u32) -> Rig {
    let (mut a, mut b) = twins(|rig| {
        rig.record(GP);
        rig.load(CODE, &asm32(CODE, |a| {
            a.pushfd()?;
            a.or(dword_ptr(esp), (iopl << 12) as i32)?;
            a.popfd()?;
            to_ring3(a)
        }));
        rig.ring3(|a| {
            a.mov(ebx, 1u32)?;
            a.cli()?;
            a.mov(ebx, 2u32)?;
            a.hlt()
        });
    });
    run_both(&mut a, &mut b);
    b
}

#[test]
fn cli_above_iopl_is_a_general_protection_fault() {
    let b = cli_at_ring_3(3);
    assert_eq!((b.recorded().0, b.cpu.ebx()), (0, 2), "no fault");
    let b = cli_at_ring_3(0);
    let (vector, stack) = b.recorded();
    assert_eq!((vector, stack[1], b.cpu.ebx()), (GP as u32, RING3 + 5, 1), "#GP at the CLI");
}

#[test]
fn a_ret_poked_into_an_unrolled_loop_is_run_where_it_is_not_translated_again() {
    // The Doom engine's spans: a RET poked over the first byte of one of
    // an unrolled loop's groups, the loop called, the byte put back, for
    // each group in turn, 50 times over. After a few translations the
    // blocks watch those bytes instead: they stop where the RET is, and
    // the interpreter runs it.
    const GROUPS: u32 = 32;
    let unrolled = CODE + 0x1000;
    let top = CODE + 0x40;
    let (mut a, mut b) = twins(|rig| {
        rig.load(unrolled, &asm32(unrolled, |a| {
            for _ in 0..GROUPS {
                a.add(eax, ebx)?; // 01 D8
                a.inc(ebx)?;
            }
            a.ret()
        }));
        rig.load(CODE, &asm32(CODE, |a| {
            a.xor(eax, eax)?;
            a.xor(ebx, ebx)?;
            a.mov(ecx, 50u32)?;
            a.jmp(top as u64)
        }));
        rig.load(top, &asm32(top, |a| {
            let mut inner = a.create_label();
            a.xor(esi, esi)?;
            a.set_label(&mut inner)?;
            a.mov(byte_ptr(esi + unrolled), 0xC3)?;
            a.call(unrolled as u64)?;
            a.mov(byte_ptr(esi + unrolled), 0x01)?;
            a.add(esi, 3)?;
            a.cmp(esi, (GROUPS * 3) as i32)?;
            a.jb(inner)?;
            a.dec(ecx)?;
            a.jnz(top as u64)?;
            a.hlt()
        }));
    });
    let stats = run_both(&mut a, &mut b);
    // Each call runs 0 to 31 groups: EBX counts them.
    assert_eq!(b.cpu.ebx(), 50 * (0..GROUPS).sum::<u32>());
    if AVAILABLE {
        assert!(stats.watched > 1000, "{:?}", stats);
        assert!(stats.blocks < 300, "translated again and again: {:?}", stats);
    }
}

#[test]
fn an_immediate_poked_before_each_loop_is_read_where_it_is() {
    // The Doom engine's columns: the step poked into the immediate of the
    // loop's ADD before each run of it; and likewise a MOV's, an AND's on
    // memory and a sign-extended one's. Blocks translated once those bytes
    // are watched read them, and run without leaving at them.
    let fns = [CODE + 0x1000, CODE + 0x1100, CODE + 0x1200, CODE + 0x1300];
    let top = CODE + 0x40;
    let data = CODE + 0x3000;
    let (mut a, mut b) = twins(|rig| {
        rig.load(fns[0], &asm32(fns[0], |a| {
            let mut again = a.create_label();
            a.mov(ecx, 16u32)?;
            a.set_label(&mut again)?;
            a.add(ebp, 0x1234_5678)?; // 81 C5 imm32, at + 5
            a.dec(ecx)?;
            a.jnz(again)?;
            a.ret()
        }));
        rig.load(fns[1], &asm32(fns[1], |a| {
            a.mov(edx, 0x0BAD_F00Du32)?; // BA imm32
            a.add(edi, edx)?;
            a.ret()
        }));
        rig.load(fns[2], &asm32(fns[2], |a| {
            a.and(dword_ptr(data), 0x7FFF_FFFF)?; // 81 25 disp32 imm32
            a.ret()
        }));
        rig.load(fns[3], &asm32(fns[3], |a| {
            a.sub(esi, 0x12)?; // 83 EE imm8
            a.ret()
        }));
        rig.load(CODE, &asm32(CODE, |a| {
            a.xor(ebp, ebp)?;
            a.xor(edi, edi)?;
            a.xor(esi, esi)?;
            a.mov(dword_ptr(data), 0xFFFF_FFFFu32 as i32)?;
            a.mov(ebx, 200u32)?;
            a.jmp(top as u64)
        }));
        rig.load(top, &asm32(top, |a| {
            a.imul_3(eax, ebx, 0x0101_0101)?;
            a.mov(dword_ptr(fns[0] + 7), eax)?;
            a.mov(dword_ptr(fns[1] + 1), eax)?;
            a.mov(dword_ptr(fns[2] + 6), eax)?;
            a.mov(byte_ptr(fns[3] + 2), al)?;
            for f in fns {
                a.call(f as u64)?;
            }
            a.dec(ebx)?;
            a.jnz(top as u64)?;
            a.hlt()
        }));
    });
    let stats = run_both(&mut a, &mut b);
    let steps = (1..=200u32).map(|n| n.wrapping_mul(0x0101_0101));
    let sum = steps.clone().fold(0u32, |s, v| s.wrapping_add(v));
    assert_eq!((b.cpu.ebp(), b.cpu.edi()), (sum.wrapping_mul(16), sum));
    let bytes = steps.clone().fold(0u32, |s, v| s.wrapping_sub(v as u8 as i8 as u32));
    assert_eq!(b.cpu.esi(), bytes);
    assert_eq!(b.cpu.bus.read_32(data as usize), steps.fold(!0, |s, v| s & v));
    if AVAILABLE {
        assert!(stats.watched < 20, "left at the poked immediates: {:?}", stats);
        assert!(stats.blocks < 100, "translated again and again: {:?}", stats);
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

/// A loop whose block runs on into the last 15 bytes of its page, where
/// the interpreter's fetch looks the next page up, which walks the page
/// tables the first time (setting its accessed bit), or every time where it
/// isn't there. Translated code must leave that to the interpreter where
/// the TLB doesn't hold the page. (The loop ends before the first batch
/// does, where the interpreter would run some of its instructions and look
/// the page up anyway.) Returns the next page's table entry.
fn loop_in_a_page_tail(next_present: bool) -> u32 {
    let (dir, table) = (0x80000u32, 0x81000u32);
    let (mut a, mut b) = twins(|rig| {
        rig.write32(dir, table | 3);
        for i in 0..1024u32 {
            rig.write32(table + 4 * i, (i << 12) | 3);
        }
        if !next_present {
            rig.write32(table + 0x31 * 4, 0);
        }
        // From 30FE8h to the page's last byte: the tail's instructions from
        // 30FF1h on, and the jump back at its end.
        let body = asm32(0x30FE8, |a| {
            let mut top = a.create_label();
            a.set_label(&mut top)?;
            a.mov(eax, 0x1234_5678u32)?;
            a.add(ebx, 1)?;
            a.inc(edx)?;
            a.add(esi, 2)?;
            a.add(edi, 3)?;
            a.dec(ecx)?;
            a.jz(CODE as u64 + 0x100)?;
            a.jmp(top)
        });
        assert_eq!(body.len(), 0x31000 - 0x30FE8, "the loop ends at the page's end");
        rig.load(0x30FE8, &body);
        let code = asm32(CODE, |a| {
            a.mov(eax, dir)?;
            a.mov(cr3, eax)?;
            a.mov(eax, cr0)?;
            a.or(eax, 0x8000_0000u32)?;
            a.mov(cr0, eax)?;
            a.mov(ecx, 20u32)?;
            a.jmp(0x30FE8u64)
        });
        rig.load(CODE, &code);
        rig.load(CODE + 0x100, &asm32(CODE + 0x100, |a| a.hlt()));
    });
    run_both(&mut a, &mut b);
    assert_eq!((b.cpu.ebx(), b.cpu.edx(), b.cpu.edi()), (20, 20, 60));
    b.read32(table + 0x31 * 4)
}

#[test]
fn a_block_into_its_pages_tail_looks_the_next_page_up_as_the_interpreter_does() {
    assert_eq!(loop_in_a_page_tail(true), 0x31023, "the fetch set the next page's accessed bit");
    assert_eq!(loop_in_a_page_tail(false), 0);
}

#[test]
fn a_segment_load_that_changes_which_segments_are_flat_goes_on_in_its_block() {
    let (mut a, mut b) = twins(|rig| {
        // DS for 64 KB at 40000h: not flat.
        rig.set_gdt(FREE, seg_desc(0x40000, 0xFFFF, DATA_R0, 0x4));
        let code = asm32(CODE, |a| {
            // DS loaded and put back within the block: its accesses between
            // go through its base, and the loop's link is taken.
            a.mov(ecx, 100u32)?;
            a.xor(ebx, ebx)?;
            let mut top = a.create_label();
            a.set_label(&mut top)?;
            a.push(ds)?;
            a.mov(ax, FREE as u32)?;
            a.mov(ds, ax)?;
            a.mov(dword_ptr(0x10), ecx)?;
            a.add(ebx, dword_ptr(0x10))?;
            a.pop(ds)?;
            a.mov(dword_ptr(0x50000), ebx)?;
            a.dec(ecx)?;
            a.jnz(top)?;
            // DS flat or not, as the iteration before left it: the loop's
            // block runs in both, and leaves through its link only where
            // they are as it was entered with.
            a.mov(ecx, 50u32)?;
            let (mut top2, mut even, mut set) = (a.create_label(), a.create_label(), a.create_label());
            a.set_label(&mut top2)?;
            a.add(dword_ptr(0x3F00), ecx)?;
            a.test(ecx, 1)?;
            a.jz(even)?;
            a.mov(ax, FREE as u32)?;
            a.jmp(set)?;
            a.set_label(&mut even)?;
            a.mov(ax, DATA32 as u32)?;
            a.set_label(&mut set)?;
            a.mov(ds, ax)?;
            a.dec(ecx)?;
            a.jnz(top2)?;
            a.mov(ax, DATA32 as u32)?;
            a.mov(ds, ax)?;
            a.hlt()
        });
        rig.load(CODE, &code);
    });
    run_both(&mut a, &mut b);
    assert_eq!(b.read32(0x50000), (1..=100).sum::<u32>());
    assert_eq!(b.read32(0x40010), 1);
    // An iteration after an odd one runs with DS at 40000h.
    assert_eq!(b.read32(0x43F00), (1..50).filter(|n| n % 2 == 0).sum::<u32>());
    assert_eq!(b.read32(0x3F00), 50 + (1..50).filter(|n| n % 2 == 1).sum::<u32>());
}

#[test]
fn a_segment_load_that_faults_leaves_the_instructions_before_it_done() {
    // A selector past the GDT's limit (7FFh): #GP with the selector.
    const BAD: u32 = 0x0FF8;
    for pop in [false, true] {
        let (mut a, mut b) = twins(|rig| {
            rig.record(GP);
            let code = asm32(CODE, |a| {
                a.mov(ebx, 1u32)?;
                a.mov(eax, BAD)?;
                if pop {
                    a.push(eax)?;
                    a.mov(ebx, 2u32)?;
                    a.pop(ds)?;
                } else {
                    a.mov(ds, ax)?;
                }
                a.mov(ebx, 3u32)?;
                a.hlt()
            });
            rig.load(CODE, &code);
        });
        run_both(&mut a, &mut b);
        let (vector, stack) = b.recorded();
        assert_eq!((vector, stack[0]), (GP as u32, BAD));
        assert_eq!(b.cpu.ebx(), 1 + pop as u32);
        if pop {
            // Above the fault's frame (error code, EIP, CS, EFLAGS): ESP
            // didn't move past the selector.
            assert_eq!(stack[4], BAD, "the selector still on the stack");
        }
    }
}

#[test]
fn an_interrupt_popf_lets_through_comes_right_after_it() {
    // The handler adds EBP, which the loop counts up, so where the
    // interrupts come shows in EDI.
    let (mut a, mut b) = twins(|rig| {
        rig.handler(0x08, 0, |a| {
            a.push(eax)?;
            a.add(edi, ebp)?;
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
            a.xor(ebp, ebp)?;
            a.mov(ecx, 3000u32)?;
            let mut top = a.create_label();
            a.set_label(&mut top)?;
            a.cli()?;
            a.inc(ebp)?;
            a.inc(ebp)?;
            a.pushfd()?;
            a.or(dword_ptr(esp), 0x200)?;
            // IF from 0 to 1: a timer interrupt that waits comes now.
            a.popfd()?;
            a.inc(ebp)?;
            a.inc(ebp)?;
            a.dec(ecx)?;
            a.jnz(top)?;
            a.cli()?;
            a.hlt()
        });
        rig.load(CODE, &code);
    });
    run_both(&mut a, &mut b);
    assert!(b.cpu.edi() > 100, "EDI {}", b.cpu.edi());
}

#[test]
fn a_stack_switch_to_another_width_stops_the_block_after_it() {
    let (mut a, mut b) = twins(|rig| {
        let code = asm32(CODE, |a| {
            // ESP above 64 KB: a 16-bit stack pushes at SS:SP.
            a.mov(esp, 0x18000u32)?;
            a.mov(ecx, 100u32)?;
            let mut top = a.create_label();
            a.set_label(&mut top)?;
            a.mov(ax, DATA16 as u32)?;
            a.mov(ss, ax)?;
            a.push(ax)?;
            a.push(cx)?;
            a.pop(bx)?;
            a.pop(dx)?;
            a.mov(ax, DATA32 as u32)?;
            a.mov(ss, ax)?;
            a.push(ecx)?;
            a.pop(esi)?;
            a.dec(ecx)?;
            a.jnz(top)?;
            a.hlt()
        });
        rig.load(CODE, &code);
    });
    run_both(&mut a, &mut b);
    assert_eq!(b.cpu.esp(), 0x18000);
    // The 16-bit pushes of AX (the selector) and CX (1, the last time) at
    // SS:SP.
    assert_eq!(b.read32(0x7FFC), (DATA16 as u32) << 16 | 1);
}

/// Singles of every kind for the FPU tests: ordinary ones, zeros, a
/// denormal, infinities, a quiet and a signalling NaN, and ones that don't
/// fit a dword.
const SINGLES: [u32; 20] = [
    0x3FC0_0000, // 1.5
    0xC010_0000, // -2.25
    0x0000_0000,
    0x8000_0000,
    0x0000_1234, // a denormal
    0x7F80_0000,
    0xFF80_0000,
    0x7FC0_0000,
    0x7F80_0001,
    0x7F7F_FFFF,
    0x0DA2_4260, // 1e-30
    0x4B80_0001, // 16777218
    0x3F00_0000, // 0.5
    0x4020_0000, // 2.5
    0xBF00_0000, // -0.5
    0x501502F9, // 1e10
    0x4F00_0000, // 2^31
    0xCF00_0000, // -2^31
    0x46FF_FE00, // 32767
    0x4700_0000, // 32768
];
const INTS: [u32; 8] = [0, 1, 0xFFFF_FFFF, 0x7FFF, 0x8000, 0x1234_5678, 0x8000_0000, 0x7FFF_FFFF];

/// Run `body` once for every pair of `SINGLES` and control word (rounding
/// to nearest, down, up and chopping): ESI points at the two singles, a
/// dword and the control word; EDI at 64 bytes for results; EBX is the
/// body's to sum status words in. After FNINIT the stack holds what the
/// iteration before left.
fn fpu_loop(body: impl Fn(&mut CodeAssembler) -> Result<(), IcedError> + Copy) -> (Rig, Rig, DynStats) {
    let (input, output) = (DATA, DATA + 0x10000);
    let n = SINGLES.len() as u32;
    let count = n * n * 4;
    let top = CODE + 0x40;
    let (mut a, mut b) = twins(|rig| {
        for k in 0..count {
            let (i, j, rc) = (k % n, k / n % n, k / (n * n));
            let at = input + 16 * k;
            rig.write32(at, SINGLES[i as usize]);
            rig.write32(at + 4, SINGLES[j as usize]);
            rig.write32(at + 8, INTS[(k % 8) as usize]);
            rig.write32(at + 12, 0x037F | rc << 10);
        }
        rig.load(CODE, &asm32(CODE, |a| {
            a.fninit()?;
            a.mov(esi, input)?;
            a.mov(edi, output)?;
            a.xor(ebx, ebx)?;
            a.mov(ecx, count)?;
            a.jmp(top as u64)
        }));
        rig.load(top, &asm32(top, |a| {
            let mut again = a.create_label();
            a.set_label(&mut again)?;
            a.fldcw(word_ptr(esi + 12))?;
            body(a)?;
            a.add(esi, 16)?;
            a.add(edi, 64)?;
            a.and(edi, (output | 0xFFFF) as i32)?;
            a.dec(ecx)?;
            a.jnz(again)?;
            a.hlt()
        }));
    });
    let stats = run_both(&mut a, &mut b);
    (a, b, stats)
}

#[test]
fn fpu_loads_stores_and_products_run_as_their_handlers() {
    let (_, b, stats) = fpu_loop(|a| {
        a.fld(dword_ptr(esi))?;
        a.fld(dword_ptr(esi + 4))?;
        a.fld(st1)?;
        a.fmul_2(st0, st1)?;
        a.fst(dword_ptr(edi))?;
        a.fmulp(st2, st0)?;
        // (FXAM sets C1 for a negative ST(0), FXCH clears it.)
        a.fxam()?;
        a.fxch(st0, st1)?;
        a.fnstsw(ax)?;
        a.add(ebx, eax)?;
        a.fmul(dword_ptr(esi))?;
        a.fmul_2(st1, st0)?;
        a.fild(dword_ptr(esi + 8))?;
        a.fild(word_ptr(esi + 8))?;
        a.fmul_2(st0, st1)?;
        a.fst(st3)?;
        a.fstp(dword_ptr(edi + 4))?;
        a.fistp(dword_ptr(edi + 8))?;
        a.fist(dword_ptr(edi + 12))?;
        a.fist(word_ptr(edi + 16))?;
        a.fld1()?;
        a.fldz()?;
        a.fxch(st0, st2)?;
        a.fistp(word_ptr(edi + 20))?;
        a.fstp(st1)?;
        a.fstp(dword_ptr(edi + 24))?;
        a.fnstcw(word_ptr(edi + 28))?;
        a.fnstsw(ax)?;
        a.add(ebx, eax)?;
        a.fstp(dword_ptr(edi + 32))?;
        a.fstp(st0)
    });
    assert_ne!(b.read32(DATA + 0x10000), 0);
    if AVAILABLE && cfg!(target_arch = "x86_64") {
        assert!(stats.native + 20 > stats.instructions, "handlers ran them: {:?}", stats);
    }
}

#[test]
fn fpu_quotients_sums_and_comparisons_run_as_their_handlers() {
    let (_, b, _) = fpu_loop(|a| {
        a.fnclex()?;
        a.fld(dword_ptr(esi))?;
        a.fld(dword_ptr(esi + 4))?;
        a.fild(dword_ptr(esi + 8))?;
        // Quotients, by 0 too.
        a.fld(st1)?;
        a.fdiv(dword_ptr(esi))?;
        a.fdivr(dword_ptr(esi + 4))?;
        a.fdiv_2(st0, st2)?;
        a.fdiv_2(st2, st0)?;
        a.fdivr_2(st0, st3)?;
        a.fdivr_2(st3, st0)?;
        a.fst(dword_ptr(edi))?;
        a.fnstsw(ax)?;
        a.add(ebx, eax)?;
        a.fld(st1)?;
        a.fdivp(st2, st0)?;
        a.fld(st2)?;
        a.fdivrp(st1, st0)?;
        a.fstp(dword_ptr(edi + 4))?;
        // Sums and differences, on the registers' 80 bits.
        a.fld(dword_ptr(esi))?;
        a.fadd(dword_ptr(esi + 4))?;
        a.fsub(dword_ptr(esi))?;
        a.fsubr(dword_ptr(esi + 4))?;
        a.fadd_2(st0, st1)?;
        a.fadd_2(st1, st0)?;
        a.fsub_2(st0, st2)?;
        a.fsub_2(st2, st0)?;
        a.fsubr_2(st0, st1)?;
        a.fsubr_2(st1, st0)?;
        a.fst(dword_ptr(edi + 8))?;
        a.fld(st0)?;
        a.faddp(st2, st0)?;
        a.fld(st1)?;
        a.fsubp(st3, st0)?;
        a.fld(st2)?;
        a.fsubrp(st1, st0)?;
        a.fstp(dword_ptr(edi + 12))?;
        // Comparisons.
        a.fcom(dword_ptr(esi))?;
        a.fnstsw(ax)?;
        a.add(ebx, eax)?;
        a.fcom_2(st0, st1)?;
        a.fnstsw(ax)?;
        a.add(ebx, eax)?;
        a.fucom(st0, st2)?;
        a.fnstsw(ax)?;
        a.add(ebx, eax)?;
        a.fcomp(dword_ptr(esi + 4))?;
        a.fnstsw(ax)?;
        a.add(ebx, eax)?;
        a.fcomp_2(st0, st1)?;
        a.fnstsw(ax)?;
        a.add(ebx, eax)?;
        a.fcompp()?;
        a.fnstsw(ax)?;
        a.add(ebx, eax)?;
        a.mov(dword_ptr(edi + 16), ebx)
    });
    assert_ne!(b.cpu.ebx(), 0);
}

#[test]
fn fpu_instructions_on_empty_registers_run_as_their_handlers() {
    // Pops from an empty stack and pushes onto a full one: the registers
    // read as the real indefinite, which the handlers see to.
    let (_, _, _) = fpu_loop(|a| {
        a.fmul_2(st0, st1)?;
        a.fxch(st0, st3)?;
        a.fstp(dword_ptr(edi))?;
        a.fistp(dword_ptr(edi + 4))?;
        a.fcompp()?;
        a.fnstsw(ax)?;
        a.add(ebx, eax)?;
        for _ in 0..5 {
            a.fld(dword_ptr(esi))?;
            a.fld(st0)?;
        }
        a.fdivp(st1, st0)?;
        a.fld(st7)?;
        a.fst(dword_ptr(edi + 8))?;
        a.fninit()?;
        a.fld(dword_ptr(esi + 4))
    });
}

#[test]
fn an_fpu_instruction_without_the_coprocessor_faults_in_its_block() {
    const NM: u8 = 7;
    let (mut a, mut b) = twins(|rig| {
        rig.record(NM);
        rig.write32(DATA, 0x3FC0_0000);
        rig.load(CODE, &asm32(CODE, |a| {
            a.fninit()?;
            a.fld(dword_ptr(DATA))?;
            a.mov(eax, cr0)?;
            a.or(eax, 8)?; // TS
            a.mov(cr0, eax)?;
            a.jmp(CODE as u64 + 0x100)
        }));
        rig.load(CODE + 0x100, &asm32(CODE + 0x100, |a| {
            a.mov(ebx, 1u32)?;
            a.fmul(dword_ptr(DATA))?;
            a.mov(ebx, 2u32)?;
            a.hlt()
        }));
    });
    run_both(&mut a, &mut b);
    let (vector, stack) = b.recorded();
    assert_eq!((vector, stack[0], b.cpu.ebx()), (NM as u32, CODE + 0x105, 1));
}

#[test]
fn an_fpu_store_past_its_segments_limit_changes_nothing() {
    let (mut a, mut b) = twins(|rig| {
        rig.record(GP);
        // A data segment of 256 bytes at 40000h.
        rig.set_gdt(FREE, seg_desc(0x40000, 0xFF, DATA_R0, 0x4));
        rig.write32(DATA + 0x80, 0x3FC0_0000);
        rig.load(CODE, &asm32(CODE, |a| {
            a.fninit()?;
            a.mov(ax, FREE as u32)?;
            a.mov(fs, ax)?;
            a.jmp(CODE as u64 + 0x100)
        }));
        rig.load(CODE + 0x100, &asm32(CODE + 0x100, |a| {
            a.fld(dword_ptr(0x80).fs())?;
            a.fld1()?;
            a.mov(ebx, 1u32)?;
            a.fstp(dword_ptr(0xFD).fs())?;
            a.mov(ebx, 2u32)?;
            a.hlt()
        }));
    });
    run_both(&mut a, &mut b);
    assert_eq!((b.recorded().0, b.cpu.ebx()), (GP as u32, 1));
    assert_eq!(b.cpu.fpu_top, 6, "the FSTP didn't pop");
}

#[test]
fn fpu_products_and_quotients_too_small_for_a_double_are_zero() {
    // 1e-30 ten times over is 1e-300; times 2^-31, and divided by 2^31
    // again, it is a denormal double, which a register doesn't hold: it
    // is 0, and stays 0 when multiplied up again.
    let (mut a, mut b) = twins(|rig| {
        rig.write32(DATA, 0x0DA2_4260); // 1e-30
        rig.write32(DATA + 4, 0x3000_0000); // 2^-31
        rig.write32(DATA + 8, 0x4F00_0000); // 2^31
        rig.load(CODE, &asm32(CODE, |a| {
            a.fninit()?;
            a.fld1()?;
            for _ in 0..10 {
                a.fmul(dword_ptr(DATA))?;
            }
            a.fld(st0)?;
            a.fld(st0)?;
            a.jmp(CODE as u64 + 0x100)
        }));
        rig.load(CODE + 0x100, &asm32(CODE + 0x100, |a| {
            // ST(0) by FMUL with memory, ST(1) by FDIV, ST(2) by FMULP.
            a.fmul(dword_ptr(DATA + 4))?;
            a.fxch(st0, st1)?;
            a.fdiv(dword_ptr(DATA + 8))?;
            a.fld(dword_ptr(DATA + 4))?;
            a.fmulp(st3, st0)?;
            for _ in 0..3 {
                for _ in 0..4 {
                    a.fmul(dword_ptr(DATA + 8))?;
                }
                a.fstp(dword_ptr(edi))?;
                a.add(edi, 4)?;
            }
            a.hlt()
        }));
    });
    a.cpu.set_edi(DATA + 0x100);
    b.cpu.set_edi(DATA + 0x100);
    run_both(&mut a, &mut b);
    for i in 0..3 {
        assert_eq!(b.read32(DATA + 0x100 + 4 * i), 0, "result {}", i);
    }
}

/// Writes and reads of video memory, the ROMs and addresses past the end
/// of RAM, which translated code hands to the bus by their physical
/// addresses: the VGA's planes through every map mask as mode X programs
/// write them (a byte at a time, plainly), words and dwords, operands that
/// reach into the next page, a write mode with a bit mask, and chain 4.
/// With `paging`, through page tables that map the first 4 MB as they are
/// but put the window at A0000h at linear 300000h too, its pages swapped
/// in pairs.
fn video_memory_program(paging: bool) -> (Rig, Rig) {
    let window = if paging { 0x30_0000u32 } else { 0xA_0000 };
    let (mut a, mut b) = twins(|rig| {
        let (dir, table) = (0x80000u32, 0x81000u32);
        rig.write32(dir, table | 3);
        for i in 0..1024u32 {
            // (Its pages in pairs the other way round, so that an operand
            // in two of them is in two places.)
            let page = if (0x300..0x310).contains(&i) { 0xA0 + ((i - 0x300) ^ 1) } else { i };
            rig.write32(table + 4 * i, (page << 12) | 3);
        }
        rig.load(CODE, &asm32(CODE, |a| {
            if paging {
                a.mov(eax, dir)?;
                a.mov(cr3, eax)?;
                a.mov(eax, cr0)?;
                a.or(eax, 0x8000_0000u32)?;
                a.mov(cr0, eax)?;
            }
            a.jmp(CODE as u64 + 0x100)
        }));
        rig.load(CODE + 0x100, &asm32(CODE + 0x100, |a| {
            let port = |a: &mut CodeAssembler, port: u32, index: u32, value: u32| {
                a.mov(edx, port)?;
                a.mov(eax, index | value << 8)?;
                a.out(dx, ax)
            };
            // The planes one after the other, write mode 0, all bits.
            port(a, 0x3C4, 4, 0x06)?;
            for (index, value) in [(0, 0), (1, 0), (3, 0), (5, 0), (6, 0x05), (8, 0xFF)] {
                port(a, 0x3CE, index, value)?;
            }
            a.xor(ebx, ebx)?;
            for (round, mask) in [1u32, 2, 4, 8, 0x0F, 0x05, 0].into_iter().enumerate() {
                let mut pixels = a.create_label();
                port(a, 0x3C4, 2, mask)?;
                a.mov(esi, window + 0x10 * round as u32)?;
                a.mov(ecx, 0x1200u32)?;
                a.set_label(&mut pixels)?;
                a.mov(eax, ecx)?;
                a.imul_3(eax, eax, 0x0101_0301)?;
                a.mov(byte_ptr(esi), al)?;
                a.mov(byte_ptr(esi + 0x4000), ah)?;
                a.mov(word_ptr(esi + 0x8001), ax)?;
                a.mov(dword_ptr(esi + 0xC003), eax)?;
                // (0FFEh and on reach into the next page.)
                a.add(byte_ptr(esi + 0x100), cl)?;
                a.movzx(eax, byte_ptr(esi + 0x4000))?;
                a.add(ebx, eax)?;
                a.add(ebx, dword_ptr(esi + 0x7FFD))?;
                a.add(bx, word_ptr(esi + 0x2001))?;
                a.add(esi, 3)?;
                a.dec(ecx)?;
                a.jnz(pixels)?;
            }
            // Write mode 2 with a bit mask, then chain 4.
            port(a, 0x3C4, 2, 0x0F)?;
            port(a, 0x3CE, 5, 0x02)?;
            port(a, 0x3CE, 8, 0x3C)?;
            let mut masked = a.create_label();
            a.mov(ecx, 0x400u32)?;
            a.set_label(&mut masked)?;
            a.mov(al, byte_ptr(ecx + window))?;
            a.mov(byte_ptr(ecx + window), cl)?;
            a.mov(word_ptr(ecx + window + 0x2000), cx)?;
            a.dec(ecx)?;
            a.jnz(masked)?;
            port(a, 0x3CE, 5, 0x40)?;
            port(a, 0x3CE, 8, 0xFF)?;
            port(a, 0x3C4, 4, 0x0E)?;
            let mut chained = a.create_label();
            a.mov(ecx, 0x400u32)?;
            a.set_label(&mut chained)?;
            a.mov(byte_ptr(ecx + window + 0x3000), cl)?;
            a.mov(dword_ptr(ecx * 4 + window + 0x5000), ecx)?;
            a.add(bl, byte_ptr(ecx + window + 0x3000))?;
            a.dec(ecx)?;
            a.jnz(chained)?;
            // The ROMs, and nothing at all.
            a.mov(dword_ptr(0xC_8000), ebx)?;
            a.add(ebx, dword_ptr(0xC_8000))?;
            a.add(ebx, dword_ptr(0xF_FFF0))?;
            if !paging {
                a.mov(dword_ptr(0x7000_0000), ebx)?;
                a.add(ebx, dword_ptr(0x7000_0000))?;
                a.add(bl, byte_ptr(0xE000_0123u32))?;
            }
            a.hlt()
        }));
    });
    run_both(&mut a, &mut b);
    (a, b)
}

#[test]
fn video_memory_is_written_and_read_by_its_physical_address() {
    let (_, b) = video_memory_program(false);
    assert!(b.cpu.bus.vga.vram_graphics.iter().any(|&p| p != 0));
    assert_ne!(b.cpu.ebx(), 0);
}

#[test]
fn video_memory_is_written_and_read_through_the_tlb() {
    let (_, b) = video_memory_program(true);
    assert!(b.cpu.bus.vga.vram_graphics.iter().any(|&p| p != 0));
    assert_ne!(b.cpu.ebx(), 0);
}
