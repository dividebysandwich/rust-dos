//! The dynamic recompiler's hard cases, each run on the interpreter and on
//! the recompiler side by side (tests/dyndiff), which must agree after
//! every batch: code that rewrites the block it is in, faults in the
//! middle of a block, interrupt shadows, the timer deadline, a full code
//! memory, and `core=auto`'s switching.

mod dyndiff;
mod pmrig;

use dyndiff::lockstep_with;
use iced_x86::code_asm::*;
use pmrig::*;
use rust_dos::cpu::{CoreMode, Cpu};
use rust_dos::dynrec::{AVAILABLE, DynStats};

const GP: u8 = 13;
const PF: u8 = 14;

/// Two rigs set up by `setup`, the second on the recompiler, both in
/// protected mode at CODE.
fn twins(setup: impl Fn(&mut Rig)) -> (Rig, Rig) {
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
