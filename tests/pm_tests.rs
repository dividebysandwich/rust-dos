//! Protected mode: entering it, segment protection, gates and privilege
//! levels, paging, task switches and virtual-8086 mode.

mod pmrig;

use iced_x86::code_asm::*;
use pmrig::*;
use rust_dos::cpu::CpuFlags;

const GP: u8 = 13;
const NP: u8 = 11;
const SS: u8 = 12;
const PF: u8 = 14;
const DF: u8 = 8;

#[test]
fn enters_protected_mode_and_runs_32bit_code() {
    let mut rig = Rig::new();
    rig.run(|a| {
        a.mov(eax, 0x1234_5678u32)?;
        a.mov(dword_ptr(DATA), eax)?;
        a.mov(ebx, dword_ptr(DATA))?;
        a.hlt()
    });
    assert!(rig.cpu.pe());
    assert_eq!(rig.cpu.cs(), CODE32);
    assert_eq!(rig.read32(DATA), 0x1234_5678);
    assert_eq!(rig.cpu.ebx(), 0x1234_5678);
    assert_eq!(rig.cpu.tr.selector, TSS_SEL);
    assert_eq!(rig.gdt(TSS_SEL) >> 40 & 0xF, 0xB, "LTR marks the TSS busy");
}

#[test]
fn access_past_a_segment_limit_raises_gp() {
    let mut rig = Rig::new();
    rig.record(GP);
    rig.set_gdt(FREE, seg_desc(DATA, 0xFFF, DATA_R0, 0x4));
    rig.run(|a| {
        a.mov(ax, FREE as u32)?; // 4 bytes
        a.mov(ds, ax)?; // 2
        a.mov(eax, dword_ptr(0xFFC))?; // 5: the last dword is fine
        a.mov(eax, dword_ptr(0xFFD))?; // one byte past the limit
        a.hlt()
    });
    let (vector, stack) = rig.recorded();
    assert_eq!(vector, GP as u32);
    assert_eq!(stack[0], 0, "error code");
    // The pushed EIP is the faulting MOV EAX, [0FFDh].
    assert_eq!(rig.cpu.bus.read_8(stack[1] as usize), 0xA1);
    assert_eq!(rig.read32(stack[1] + 1), 0xFFD);
    assert_eq!(stack[2], CODE32 as u32);
}

#[test]
fn null_selector_loads_but_faults_on_use() {
    let mut rig = Rig::new();
    rig.record(GP);
    rig.run(|a| {
        a.xor(eax, eax)?;
        a.mov(ds, ax)?;
        a.mov(dword_ptr(DATA), eax)?;
        a.hlt()
    });
    let (vector, stack) = rig.recorded();
    assert_eq!((vector, stack[0]), (GP as u32, 0));
}

#[test]
fn segment_loads_check_type_and_presence() {
    // SS with a code segment: #GP(selector).
    let mut rig = Rig::new();
    rig.record(GP);
    rig.run(|a| {
        a.mov(ax, CODE32 as u32)?;
        a.mov(ss, ax)?;
        a.hlt()
    });
    assert_eq!(rig.recorded().0, GP as u32);
    assert_eq!(rig.recorded().1[0], CODE32 as u32);

    // A data segment that isn't present: #NP(selector).
    let mut rig = Rig::new();
    rig.record(NP);
    rig.set_gdt(FREE, seg_desc(DATA, 0xFFFF, DATA_R0 & 0x7F, 0));
    rig.run(|a| {
        a.mov(ax, FREE as u32)?;
        a.mov(es, ax)?;
        a.hlt()
    });
    assert_eq!(rig.recorded().0, NP as u32);
    assert_eq!(rig.recorded().1[0], FREE as u32);

    // A stack segment that isn't present: #SS(selector).
    let mut rig = Rig::new();
    rig.record(SS);
    rig.set_gdt(FREE, seg_desc(DATA, 0xFFFF, DATA_R0 & 0x7F, 0));
    rig.run(|a| {
        a.mov(ax, FREE as u32)?;
        a.mov(ss, ax)?;
        a.hlt()
    });
    assert_eq!(rig.recorded().0, SS as u32);
    assert_eq!(rig.recorded().1[0], FREE as u32);

    // A ring 3 data segment is fine at ring 0 and gets its accessed bit.
    let mut rig = Rig::new();
    rig.set_gdt(FREE, seg_desc(DATA, 0xFFFF, 0xF2, 0));
    rig.run(|a| {
        a.mov(ax, FREE as u32)?;
        a.mov(fs, ax)?;
        a.hlt()
    });
    assert_eq!(rig.gdt(FREE) >> 40 & 1, 1, "accessed");
}

#[test]
fn read_only_segments_and_code_segments_refuse_writes() {
    let mut rig = Rig::new();
    rig.record(GP);
    rig.set_gdt(FREE, seg_desc(DATA, 0xFFFF, 0x90, 0));
    rig.run(|a| {
        a.mov(ax, FREE as u32)?;
        a.mov(ds, ax)?;
        a.mov(eax, dword_ptr(0))?; // reading is fine
        a.mov(dword_ptr(0), eax)?;
        a.hlt()
    });
    let (vector, stack) = rig.recorded();
    assert_eq!((vector, stack[0]), (GP as u32, 0));

    let mut rig = Rig::new();
    rig.record(GP);
    rig.run(|a| {
        a.mov(eax, 1u32)?;
        a.mov(dword_ptr(DATA).cs(), eax)?;
        a.hlt()
    });
    assert_eq!(rig.recorded().0, GP as u32);
}

#[test]
fn expand_down_segments_allow_offsets_above_the_limit() {
    let mut rig = Rig::new();
    rig.record(GP);
    // Expand-down, writable, B=1: valid offsets 1000h..FFFFFFFFh.
    rig.set_gdt(FREE, seg_desc(0, 0x0FFF, 0x96, 0x4));
    rig.run(|a| {
        a.mov(ax, FREE as u32)?;
        a.mov(ds, ax)?;
        a.mov(dword_ptr(DATA + 0xFC), 0x55u32)?;
        a.mov(dword_ptr(0x1000), 0x66u32)?;
        a.mov(dword_ptr(0xFFE), 0x77u32)?; // below the valid range
        a.hlt()
    });
    assert_eq!(rig.read32(DATA + 0xFC), 0x55);
    assert_eq!(rig.read32(0x1000), 0x66);
    let (vector, stack) = rig.recorded();
    assert_eq!((vector, stack[0]), (GP as u32, 0));
}

#[test]
fn far_call_and_return_between_16_and_32_bit_code() {
    let mut rig = Rig::new();
    // A 16-bit routine at 0000:3000 in CODE16 returning to the 32-bit
    // caller with a 32-bit RETF.
    let routine = asm16(0x3000, |a| {
        a.mov(ax, 0x1616u32)?;
        a.db(&[0x66, 0xCB])
    });
    rig.load(0x3000, &routine);
    rig.run(|a| {
        a.call_far(CODE16, 0x3000)?;
        a.mov(ebx, 0x3232u32)?;
        a.hlt()
    });
    assert_eq!(rig.cpu.ax(), 0x1616);
    assert_eq!(rig.cpu.ebx(), 0x3232);
    assert_eq!(rig.cpu.cs(), CODE32);
    assert_eq!(rig.cpu.esp(), STACK0_TOP);
}

#[test]
fn iret_to_ring_3_and_interrupt_back_to_ring_0() {
    let mut rig = Rig::new();
    rig.handler(0x40, 3, |a| record_code(a, 0x40));
    rig.ring3(|a| {
        a.mov(ax, DATA32_R3 as u32)?;
        a.mov(ds, ax)?;
        a.mov(dword_ptr(DATA), 3u32)?;
        a.int(0x40)?;
        a.hlt()
    });
    rig.run(to_ring3);
    let (vector, stack) = rig.recorded();
    assert_eq!(vector, 0x40);
    // No error code: EIP, CS, EFLAGS, ESP, SS of ring 3.
    assert!(stack[0] > CODE);
    assert_eq!(stack[1], CODE32_R3 as u32);
    assert_eq!(stack[3], STACK3_TOP);
    assert_eq!(stack[4], DATA32_R3 as u32);
    assert_eq!(rig.read32(DATA), 3);
    assert_eq!(rig.cpu.cs(), CODE32, "the handler runs at ring 0");
    assert_eq!(rig.cpu.cpl, 0);
    assert_eq!(rig.cpu.ss(), DATA32, "on the ring 0 stack from the TSS");
    assert!(!rig.cpu.get_cpu_flag(CpuFlags::IF), "an interrupt gate clears IF");
}

#[test]
fn ring_3_is_kept_out_of_privileged_things() {
    // A gate of DPL 0 can't be used by INT n from ring 3: #GP(vector*8+2).
    let mut rig = Rig::new();
    rig.record(GP);
    rig.handler(0x41, 0, |a| a.hlt());
    rig.ring3(|a| {
        a.int(0x41)?;
        a.hlt()
    });
    rig.run(to_ring3);
    let (vector, stack) = rig.recorded();
    assert_eq!((vector, stack[0]), (GP as u32, 0x41 * 8 + 2));

    // HLT, CLI (with IOPL 0) and port I/O without a bitmap: #GP(0).
    for body in [0u8, 1, 2] {
        let mut rig = Rig::new();
        rig.record(GP);
        rig.ring3(|a| {
            match body {
                0 => a.hlt()?,
                1 => a.cli()?,
                _ => a.out(0x80, al)?,
            }
            a.int3()
        });
        rig.run(to_ring3);
        let (vector, stack) = rig.recorded();
        assert_eq!((vector, stack[0]), (GP as u32, 0), "case {}", body);
        assert_eq!(stack[2], CODE32_R3 as u32);
    }
}

#[test]
fn io_permission_bitmap_decides_ring_3_port_access() {
    let mut rig = Rig::new();
    rig.record(GP);
    // A TSS with a bitmap at 68h allowing only ports 60h-6Fh.
    rig.set_gdt(TSS_SEL, sys_desc(TSS, 0x68 + 0x2000, TSS32, 0));
    for i in 0..0x2000 {
        rig.cpu.bus.write_8((TSS + 0x68 + i) as usize, 0xFF);
    }
    rig.write16(TSS + 0x68 + 0x60 / 8, 0);
    rig.ring3(|a| {
        a.in_(al, 0x64)?; // allowed
        a.mov(ebx, 1u32)?;
        a.in_(al, 0x70)?; // not allowed
        a.mov(ebx, 2u32)?;
        a.int3()
    });
    rig.run(to_ring3);
    assert_eq!(rig.recorded().0, GP as u32);
    assert_eq!(rig.cpu.ebx(), 1);
}

#[test]
fn call_gate_to_ring_0_copies_parameters_and_returns_to_ring_3() {
    let mut rig = Rig::new();
    let routine_at = HANDLERS;
    // The ring 0 routine records its stack: EIP, CS, param 1, param 0,
    // ESP, SS; then returns releasing the parameters.
    let routine = asm32(routine_at, |a| {
        a.mov(esi, esp)?;
        a.mov(edi, RESULT + 4)?;
        a.mov(ecx, 6u32)?;
        a.cld()?;
        a.rep().movsd()?;
        a.mov(dword_ptr(RESULT), ss)?;
        a.retf_1(8)
    });
    rig.load(routine_at, &routine);
    rig.set_gdt(FREE, gate_desc(CODE32, routine_at, CALL_GATE32, 3, 2));
    rig.handler(0x40, 3, |a| a.hlt());
    rig.ring3(|a| {
        a.mov(ax, DATA32_R3 as u32)?;
        a.mov(ds, ax)?;
        a.mov(es, ax)?;
        a.push(0x1111u32)?;
        a.push(0x2222u32)?;
        a.call_far((FREE | 3) as u16, 0)?;
        a.mov(ebx, esp)?;
        a.mov(ecx, ss)?;
        a.int(0x40)
    });
    rig.run(to_ring3);
    assert_eq!(rig.read32(RESULT), DATA32 as u32, "ring 0 stack");
    assert_eq!(rig.read32(RESULT + 8), CODE32_R3 as u32);
    assert_eq!(rig.read32(RESULT + 12), 0x2222);
    assert_eq!(rig.read32(RESULT + 16), 0x1111);
    assert_eq!(rig.read32(RESULT + 20), STACK3_TOP - 8);
    assert_eq!(rig.read32(RESULT + 24), DATA32_R3 as u32);
    // Back at ring 3 with the parameters released.
    assert_eq!(rig.cpu.ebx(), STACK3_TOP);
    assert_eq!(rig.cpu.ecx() & 0xFFFF, DATA32_R3 as u32);
}

#[test]
fn return_to_ring_3_nulls_ring_0_data_segments() {
    let mut rig = Rig::new();
    rig.handler(0x40, 3, |a| a.hlt());
    // DS is still the ring 0 data segment when the IRET happens.
    rig.ring3(|a| {
        a.mov(eax, ds)?;
        a.int(0x40)
    });
    rig.run(to_ring3);
    assert_eq!(rig.cpu.eax() & 0xFFFF, 0);
}

fn page_tables(rig: &mut Rig) {
    // Directory at 80000h, one table at 81000h identity-mapping the first
    // 4 MB, supervisor-only except where tests say.
    let dir = 0x80000;
    let table = 0x81000;
    rig.write32(dir, table | 0x3);
    for i in 0..1024u32 {
        rig.write32(table + 4 * i, (i << 12) | 0x3);
    }
}

fn enable_paging(a: &mut CodeAssembler) -> Result<(), IcedError> {
    a.mov(eax, 0x80000u32)?;
    a.mov(cr3, eax)?;
    a.mov(eax, cr0)?;
    a.or(eax, 0x8000_0000u32)?;
    a.mov(cr0, eax)
}

#[test]
fn paging_translates_and_sets_accessed_and_dirty_bits() {
    let mut rig = Rig::new();
    page_tables(&mut rig);
    // Linear page 50h (50000h) maps to physical 90000h.
    rig.write32(0x81000 + 0x50 * 4, 0x90000 | 0x3);
    rig.run(|a| {
        enable_paging(a)?;
        a.mov(dword_ptr(0x50010), 0xABCDu32)?;
        a.hlt()
    });
    assert_eq!(rig.read32(0x90010), 0xABCD);
    assert_eq!(rig.read32(0x50010), 0);
    let pte = rig.read32(0x81000 + 0x50 * 4);
    assert_eq!(pte & 0x60, 0x60, "accessed and dirty");
    assert_eq!(rig.read32(0x80000) & 0x20, 0x20, "directory entry accessed");
}

#[test]
fn page_faults_report_the_address_and_cause() {
    // Not present: error 2 for a write at ring 0, CR2 = the address.
    let mut rig = Rig::new();
    page_tables(&mut rig);
    rig.record(PF);
    rig.write32(0x81000 + 0x55 * 4, 0);
    rig.run(|a| {
        enable_paging(a)?;
        a.mov(dword_ptr(0x55123), 1u32)?;
        a.hlt()
    });
    let (vector, stack) = rig.recorded();
    assert_eq!((vector, stack[0]), (PF as u32, 0x2));
    assert_eq!(rig.cpu.cr2, 0x55123);

    // A supervisor page read from ring 3: error 5 (present, user).
    let mut rig = Rig::new();
    page_tables(&mut rig);
    // User pages: ring 3's code and stack.
    rig.write32(0x80000, 0x81000 | 0x7);
    for page in [0x18u32, 0x5F] {
        rig.write32(0x81000 + page * 4, (page << 12) | 0x7);
    }
    rig.record(PF);
    rig.ring3(|a| {
        a.mov(ax, DATA32_R3 as u32)?;
        a.mov(ds, ax)?;
        a.mov(eax, dword_ptr(0x42000))?;
        a.hlt()
    });
    rig.run(|a| {
        enable_paging(a)?;
        to_ring3(a)
    });
    let (vector, stack) = rig.recorded();
    assert_eq!((vector, stack[0]), (PF as u32, 0x5));
    assert_eq!(rig.cpu.cr2, 0x42000);
}

#[test]
fn code_at_the_end_of_a_page_before_a_missing_one_leaves_cr2_alone() {
    // The last instructions of linear page 30000h, with page 31000h not
    // present: they fit in their page, so nothing faults and CR2 keeps
    // the value the program gave it.
    let mut tail = vec![0x90; 0xFF4];
    tail.extend(asm32(0x30FF4, |a| {
        a.mov(ebx, 0x1234u32)?;
        a.hlt()
    }));
    assert!(tail.len() <= 0x1000);
    for batched in [false, true] {
        let mut rig = Rig::new();
        page_tables(&mut rig);
        rig.write32(0x81000 + 0x31 * 4, 0);
        rig.load(0x30000, &tail);
        let program = |a: &mut CodeAssembler| {
            enable_paging(a)?;
            a.mov(eax, 0xC0FFEEu32)?;
            a.mov(cr2, eax)?;
            a.mov(eax, 0x30FF4u32)?;
            a.jmp(eax)
        };
        if batched {
            rig.run_batched(program);
            // run_batched stops before the HLT.
        } else {
            rig.run(program);
        }
        assert_eq!(rig.cpu.ebx(), 0x1234, "batched: {batched}");
        assert_eq!(rig.cpu.cr2, 0xC0FFEE, "batched: {batched}");
    }
}

#[test]
fn tlb_keeps_old_translations_until_invlpg() {
    let mut rig = Rig::new();
    page_tables(&mut rig);
    rig.write32(0x90000, 0x9999);
    rig.write32(0x91000, 0x1111);
    rig.write32(0x81000 + 0x50 * 4, 0x90000 | 0x3);
    rig.run(|a| {
        enable_paging(a)?;
        a.mov(eax, dword_ptr(0x50000))?;
        // Remap the page without telling the TLB.
        a.mov(dword_ptr(0x81000 + 0x50 * 4), 0x91003u32)?;
        a.mov(ebx, dword_ptr(0x50000))?;
        a.invlpg(ptr(0x50000))?;
        a.mov(ecx, dword_ptr(0x50000))?;
        a.hlt()
    });
    assert_eq!(rig.cpu.eax(), 0x9999);
    assert_eq!(rig.cpu.ebx(), 0x9999, "stale TLB entry");
    assert_eq!(rig.cpu.ecx(), 0x1111);
}

#[test]
fn cr0_rejects_paging_without_protection() {
    let mut rig = Rig::new();
    rig.record(GP);
    rig.run(|a| {
        a.mov(eax, 0x8000_0010u32)?;
        a.mov(cr0, eax)?;
        a.hlt()
    });
    assert_eq!(rig.recorded().0, GP as u32);
}

#[test]
fn a_fault_while_delivering_a_fault_is_a_double_fault() {
    let mut rig = Rig::new();
    rig.record(DF);
    // #GP's gate points at a segment that isn't present: #NP during the
    // delivery of #GP, both contributory.
    rig.set_gdt(FREE, seg_desc(0, 0xFFFFF, CODE_R0 & 0x7F, G32));
    rig.set_idt(GP, gate_desc(FREE, 0, INT_GATE32, 0, 0));
    rig.run(|a| {
        a.xor(eax, eax)?;
        a.mov(ds, ax)?;
        a.mov(dword_ptr(0), eax)?;
        a.hlt()
    });
    let (vector, stack) = rig.recorded();
    assert_eq!((vector, stack[0]), (DF as u32, 0));
}

#[test]
fn a_triple_fault_resets_the_processor() {
    let mut rig = Rig::new();
    rig.run(|a| {
        a.mov(eax, 0x10u32)?;
        a.mov(ebx, 0u32)?;
        a.mov(esp, 0x10u32)?;
        a.xor(eax, eax)?;
        a.mov(ds, ax)?;
        // #GP with no IDT entries -> #GP -> #DF -> shutdown.
        a.mov(dword_ptr(0), eax)?;
        a.hlt()
    });
    assert!(!rig.cpu.pe(), "back in real mode");
    assert_eq!(rig.cpu.cs(), 0xF000, "through the BIOS reset vector");
    assert_eq!(rig.cpu.state, rust_dos::cpu::CpuState::RebootShell);
}

#[test]
fn irq_0_is_delivered_to_vector_8_and_in_service() {
    let mut rig = Rig::new();
    rig.record(0x08);
    let code = asm32(CODE, |a| {
        a.mov(al, 0xFEu32)?;
        a.out(0x21, al)?;
        a.sti()?;
        let mut spin = a.create_label();
        a.set_label(&mut spin)?;
        a.jmp(spin)
    });
    rig.load(CODE, &code);
    rig.enter_pm();
    for _ in 0..10 {
        rig.cpu.step();
    }
    rig.cpu.bus.pic.raise(0);
    rig.run_to_halt();
    let (vector, stack) = rig.recorded();
    assert_eq!(vector, 8);
    assert_eq!(stack[1], CODE32 as u32, "no error code: EIP, CS, EFLAGS");
    // DOS/4GW tells IRQ 0 from a double fault by reading the ISR.
    rig.cpu.bus.io_write(0x20, 0x0B);
    assert_eq!(rig.cpu.bus.io_read(0x20) & 1, 1);
}

#[test]
fn jmp_to_a_tss_switches_tasks_and_iret_returns() {
    let mut rig = Rig::new();
    let task2_code = HANDLERS;
    // Task 2: record EAX, bump it, return with IRET (NT is set by CALL).
    let code = asm32(task2_code, |a| {
        a.mov(dword_ptr(RESULT), eax)?;
        a.mov(dword_ptr(RESULT + 4), 0x2222u32)?;
        a.iretd()?;
        a.hlt()
    });
    rig.load(task2_code, &code);
    rig.set_gdt(FREE, sys_desc(TSS2, 0x67, TSS32, 0));
    rig.write32(TSS2 + 0x20, task2_code); // EIP
    rig.write32(TSS2 + 0x24, 0x0002); // EFLAGS
    rig.write32(TSS2 + 0x28, 0x7777); // EAX
    rig.write32(TSS2 + 0x38, STACK0_TOP - 0x1000); // ESP
    for (i, sel) in [DATA32, CODE32, DATA32, DATA32, DATA32, DATA32].iter().enumerate() {
        rig.write32(TSS2 + 0x48 + 4 * i as u32, *sel as u32);
    }
    rig.run(|a| {
        a.mov(eax, 0x1111u32)?;
        a.call_far(FREE, 0)?;
        a.mov(ebx, eax)?;
        a.hlt()
    });
    assert_eq!(rig.read32(RESULT), 0x7777, "task 2's EAX");
    assert_eq!(rig.read32(RESULT + 4), 0x2222);
    assert_eq!(rig.cpu.ebx(), 0x1111, "task 1's EAX came back");
    assert_eq!(rig.cpu.tr.selector, TSS_SEL);
    assert_eq!(rig.read32(TSS2), TSS_SEL as u32, "back link");
    assert_eq!(rig.gdt(FREE) >> 40 & 0xF, 0x9, "task 2 no longer busy");
    assert!(rig.cpu.cr0 & 8 != 0, "TS set");
}

#[test]
fn virtual_8086_mode_runs_real_mode_code_and_traps_to_ring_0() {
    let mut rig = Rig::new();
    // V86 code at 1000:0000 (10000h would clash with CODE: use 3000:0000).
    let v86 = asm16(0x30000, |a| {
        a.mov(ax, 0x3000u32)?;
        a.mov(ds, ax)?;
        a.mov(word_ptr(0x100), 0xBEEFu32)?;
        a.int(0x40)
    });
    rig.load(0x30000, &v86);
    rig.handler(0x40, 3, |a| record_code(a, 0x40));
    rig.run(|a| {
        // IRETD frame for V86: GS FS DS ES SS ESP EFLAGS(VM, IOPL 3) CS EIP.
        for v in [0u32, 0, 0, 0, 0x2000, 0xFFFE, 0x0002_3002, 0x3000, 0] {
            a.push(v)?;
        }
        a.iretd()
    });
    assert_eq!(rig.cpu.bus.read_16(0x30100), 0xBEEF);
    let (vector, stack) = rig.recorded();
    assert_eq!(vector, 0x40);
    assert_eq!(stack[1], 0x3000, "CS");
    assert!(stack[2] & 0x2_0000 != 0, "VM in the saved EFLAGS");
    assert_eq!(stack[3], 0xFFFE, "ESP");
    assert_eq!(stack[4], 0x2000, "SS");
    assert_eq!(stack[6], 0x3000, "DS");
    assert!(!rig.cpu.v86());
}

#[test]
fn virtual_8086_mode_with_iopl_0_traps_cli() {
    let mut rig = Rig::new();
    let v86 = asm16(0x30000, |a| {
        a.cli()?;
        a.hlt()
    });
    rig.load(0x30000, &v86);
    rig.record(GP);
    rig.run(|a| {
        for v in [0u32, 0, 0, 0, 0x2000, 0xFFFE, 0x0002_0002, 0x3000, 0] {
            a.push(v)?;
        }
        a.iretd()
    });
    let (vector, stack) = rig.recorded();
    assert_eq!((vector, stack[0], stack[1], stack[2]), (GP as u32, 0, 0, 0x3000));
}

#[test]
fn a_bios_service_called_in_virtual_8086_mode_returns_through_the_bios_iret() {
    let mut rig = Rig::new();
    // As a monitor reflects INT 21h: flags, CS and IP on the stack and on
    // to the vector's handler, the DOS service at F000:1030 (AH=30h, the
    // version).
    let v86 = asm16(0x30000, |a| {
        a.mov(ax, 0x3000u32)?;
        a.mov(ds, ax)?;
        a.mov(ah, 0x30u32)?;
        a.pushf()?;
        a.db(&[0x9A, 0x30, 0x10, 0x00, 0xF0])?; // CALL FAR F000:1030
        a.mov(word_ptr(0x100), ax)?;
        a.int(0x40)
    });
    rig.load(0x30000, &v86);
    rig.handler(0x40, 3, |a| record_code(a, 0x40));
    rig.run(|a| {
        for v in [0u32, 0, 0, 0, 0x2000, 0xFFFE, 0x0002_3002, 0x3000, 0] {
            a.push(v)?;
        }
        a.iretd()
    });
    assert_eq!(rig.cpu.bus.read_16(0x30100), 0x0005, "DOS 5.0");
    let (vector, stack) = rig.recorded();
    assert_eq!((vector, stack[1]), (0x40, 0x3000), "back in the V86 code");
}

#[test]
fn a_bios_service_in_virtual_8086_mode_with_iopl_0_leaves_its_iret_to_the_monitor() {
    let mut rig = Rig::new();
    // The frame the monitor built at 2000:FFF0 returns to 3000:0000 with
    // CF clear; the service runs at F000:1030 (AH=3Eh, a handle that isn't
    // open).
    for (i, v) in [0x0000u16, 0x3000, 0x0002].into_iter().enumerate() {
        rig.write16(0x2FFF0 + 2 * i as u32, v);
    }
    rig.record(GP);
    rig.run(|a| {
        a.mov(eax, 0x3E00u32)?;
        a.mov(ebx, 0xFFFFu32)?;
        for v in [0u32, 0, 0, 0, 0x2000, 0xFFF0, 0x0002_0002, 0xF000, 0x1030] {
            a.push(v)?;
        }
        a.iretd()
    });
    // The BIOS's IRET faults, for the monitor to carry out, with the
    // service's result in AX and its CF in the flags it pops.
    let (vector, stack) = rig.recorded();
    assert_eq!((vector, stack[1], stack[2]), (GP as u32, 0xFF53, 0xF000));
    assert_eq!(rig.cpu.ax(), 0x0006, "invalid handle");
    assert_eq!(rig.cpu.bus.read_16(0x2FFF4), 0x0003, "CF set");
}

#[test]
fn a_video_mode_set_in_virtual_8086_mode_makes_its_register_writes_through_the_ports() {
    let mut rig = Rig::new();
    // INT 10h AX=0012h, as a monitor reflects it. With no I/O permission
    // bitmap every port access in V86 mode traps, as the VGA's do to
    // Windows' VDD.
    let v86 = asm16(0x30000, |a| {
        a.mov(ax, 0x0012u32)?;
        a.pushf()?;
        a.db(&[0x9A, 0x08, 0x10, 0x00, 0xF0])?; // CALL FAR F000:1008
        a.int(0x40)
    });
    rig.load(0x30000, &v86);
    rig.record(GP);
    let before = rig.cpu.bus.video_mode;
    rig.run(|a| {
        for v in [0u32, 0, 0, 0, 0x2000, 0xFFFE, 0x0002_3002, 0x3000, 0] {
            a.push(v)?;
        }
        a.iretd()
    });
    // The first, the Miscellaneous Output register's 640x480 clock, faults
    // in the BIOS's replay.
    let (vector, stack) = rig.recorded();
    assert_eq!((vector, stack[2]), (GP as u32, 0xF000));
    assert!((rust_dos::bios::PORT_ACCESSES as u32..rust_dos::bios::PORT_ACCESSES as u32 + 0x40).contains(&stack[1]));
    assert_eq!((rig.cpu.dx(), rig.cpu.get_al()), (0x3C2, 0xE3));
    assert!(!rig.cpu.bus.port_accesses.is_empty(), "more to write");
    // The ports trap, so the card is as it was: the machine's display is
    // the monitor's to keep. Its memory is cleared last, by the processor.
    assert_eq!(rig.cpu.bus.video_mode, before);
    assert_eq!(
        rig.cpu.bus.port_accesses.back(),
        Some(&rust_dos::bios::PortAccess::Fill { segment: 0xA000, words: 0x8000, value: 0 })
    );
}

#[test]
fn enabling_the_ps2_mouse_in_virtual_8086_mode_unmasks_irq_12_through_the_ports() {
    let mut rig = Rig::new();
    // INT 15h AX=C200h BH=1 with a handler, as a monitor reflects it:
    // the PIC's mask changes by port, where a monitor that keeps the
    // masks (Windows' VPICD) traps it.
    rig.cpu.bus.mouse.ps2.handler = (0x1234, 0x5678);
    let v86 = asm16(0x30000, |a| {
        a.mov(ax, 0xC200u32)?;
        a.mov(bx, 0x0100u32)?;
        a.pushf()?;
        a.db(&[0x9A, 0x1C, 0x10, 0x00, 0xF0])?; // CALL FAR F000:101C
        a.int(0x40)
    });
    rig.load(0x30000, &v86);
    rig.record(GP);
    rig.run(|a| {
        for v in [0u32, 0, 0, 0, 0x2000, 0xFFFE, 0x0002_3002, 0x3000, 0] {
            a.push(v)?;
        }
        a.iretd()
    });
    let (vector, stack) = rig.recorded();
    assert_eq!((vector, stack[2]), (GP as u32, 0xF000));
    assert_eq!(rig.cpu.dx(), 0xA1, "reading the slave's mask");
    assert_eq!(rig.cpu.bus.pic.slave.imr & 0x10, 0x10, "not unmasked behind the monitor's back");
    assert!(rig.cpu.bus.mouse.ps2.enabled);
}

#[test]
fn exec_loads_a_program_into_a_virtual_machines_own_memory() {
    use rust_dos::mcb::{self, DOS_OWNER, FIRST_MCB_SEG, FREE_OWNER, MCB_M, MCB_Z, Mcb};
    let mut rig = Rig::new();
    let dir = std::path::PathBuf::from("target/test_pm/vm_exec");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let program = [0xB4, 0x4C, 0xCD, 0x21];
    std::fs::write(dir.join("X.COM"), program).unwrap();
    rig.cpu.bus.mount_drive(3, &dir, Default::default(), false).unwrap();
    // DOS's memory up to 90000h, and the free memory above it in pages at
    // 300000h, as a DOS machine of Windows' 386 enhanced mode has memory of
    // its own.
    let bus = &mut rig.cpu.bus;
    mcb::write_mcb(bus, FIRST_MCB_SEG, &Mcb { signature: MCB_M, owner: DOS_OWNER, size: 0x9000 - 0x1000 });
    rig.load(0x30_0000, &[MCB_Z, FREE_OWNER as u8, 0, 0xFF, 0x0F]);
    page_tables(&mut rig);
    rig.write32(0x80000, 0x81000 | 0x7);
    for page in 0..1024u32 {
        let at = if (0x90..0xA0).contains(&page) { 0x300 + page - 0x90 } else { page };
        rig.write32(0x81000 + 4 * page, (at << 12) | 0x7);
    }
    // EXEC D:\X.COM to load it (AL=01h), and the running process after.
    let v86 = asm16(0x30000, |a| {
        a.mov(ax, 0x3000u32)?;
        a.mov(ds, ax)?;
        a.mov(es, ax)?;
        a.mov(dx, 0x200u32)?;
        a.mov(bx, 0x210u32)?;
        a.mov(ax, 0x4B01u32)?;
        a.pushf()?;
        a.db(&[0x9A, 0x30, 0x10, 0x00, 0xF0])?; // CALL FAR F000:1030, INT 21h
        a.mov(word_ptr(0x100), ax)?;
        a.pushf()?;
        a.pop(ax)?;
        a.mov(word_ptr(0x102), ax)?;
        a.mov(ah, 0x62u32)?;
        a.pushf()?;
        a.db(&[0x9A, 0x30, 0x10, 0x00, 0xF0])?;
        a.mov(word_ptr(0x104), bx)?;
        a.int(0x40)
    });
    rig.load(0x30000, &v86);
    rig.load(0x30200, b"D:\\X.COM\0");
    // The parameter block: the parent's environment, an empty command tail
    // and two blank FCBs.
    rig.load(0x30210, &[0, 0, 0x20, 0x02, 0x00, 0x30, 0x30, 0x02, 0x00, 0x30, 0x40, 0x02, 0x00, 0x30]);
    rig.load(0x30220, &[0, 0x0D]);
    rig.handler(0x40, 3, |a| record_code(a, 0x40));
    rig.run(|a| {
        enable_paging(a)?;
        for v in [0u32, 0, 0, 0, 0x2000, 0xFFFE, 0x0002_3002, 0x3000, 0] {
            a.push(v)?;
        }
        a.iretd()
    });
    assert_eq!(rig.recorded().0, 0x40);
    assert_eq!(rig.cpu.bus.read_16(0x30102) & 0x0001, 0, "CF, error {:04X}", rig.cpu.bus.read_16(0x30100));
    let psp = rig.cpu.bus.read_16(0x30104) as usize;
    assert!((0x9000..0xA000).contains(&psp), "PSP {:04X}", psp);
    // The program, its PSP and its memory block are in the machine's pages.
    let at = 0x30_0000 + psp * 16 - 0x9_0000;
    let bytes = |rig: &Rig, at: usize, len: usize| (0..len).map(|i| rig.cpu.bus.read_8(at + i)).collect::<Vec<u8>>();
    assert_eq!(bytes(&rig, at + 0x100, 4), program);
    assert_eq!(bytes(&rig, at, 2), [0xCD, 0x20]);
    assert_eq!(rig.cpu.bus.read_16(at - 16 + 1), psp as u16, "the block's owner");
    assert_eq!(bytes(&rig, psp * 16 + 0x100, 4), [0; 4], "memory where its addresses say");
}

/// CALL FAR F000:1030, the INT 21h trap, after a PUSHF: INT 21h as a
/// monitor reflects it.
const INT21: [u8; 5] = [0x9A, 0x30, 0x10, 0x00, 0xF0];

/// Page tables as `user_page_tables` makes them, but with the linear
/// pages `moved` (page, physical page) elsewhere, as a Windows virtual
/// machine has memory of its own.
fn vm_page_tables(rig: &mut Rig, moved: &[(u32, u32)]) {
    user_page_tables(rig);
    for &(page, at) in moved {
        rig.write32(0x81000 + 4 * page, (at << 12) | 0x7);
    }
}

/// Run `v86` at 3000:0000 in virtual-8086 mode with IOPL 3 and paging on,
/// its stack at 2000:FFFE.
fn run_v86_paged(rig: &mut Rig, v86: &[u8]) {
    rig.load(0x30000, v86);
    rig.run(|a| {
        enable_paging(a)?;
        for v in [0u32, 0, 0, 0, 0x2000, 0xFFFE, 0x0002_3002, 0x3000, 0] {
            a.push(v)?;
        }
        a.iretd()
    });
}

#[test]
fn dos_takes_the_running_process_from_the_machines_sda() {
    use rust_dos::dos_data::{SDA, address};
    let mut rig = Rig::new();
    // Under Windows each virtual machine has an SDA of its own, which
    // DOSMGR swaps in with the machine: its running process is there,
    // whatever the machine that called DOS before had.
    vm_page_tables(&mut rig, &[]);
    let psp_field = address(SDA) + 0x10;
    rig.cpu.bus.write_16(psp_field, 0x1234);
    rig.cpu.current_psp = 0x0777;
    let v86 = asm16(0x30000, |a| {
        a.mov(ax, 0x3000u32)?;
        a.mov(ds, ax)?;
        a.mov(ah, 0x62u32)?;
        a.pushf()?;
        a.db(&INT21)?;
        a.mov(word_ptr(0x100), bx)?;
        a.mov(ah, 0x50u32)?;
        a.mov(bx, 0x4567u32)?;
        a.pushf()?;
        a.db(&INT21)?;
        a.int(0x40)
    });
    rig.handler(0x40, 3, |a| record_code(a, 0x40));
    run_v86_paged(&mut rig, &v86);
    assert_eq!(rig.recorded().0, 0x40);
    assert_eq!(rig.cpu.bus.read_16(0x30100), 0x1234, "AH=62h");
    assert_eq!(rig.cpu.bus.read_16(psp_field), 0x4567, "AH=50h, kept in the SDA");
}

#[test]
fn dos_takes_the_current_directory_from_the_machines_cds() {
    let mut rig = Rig::new();
    // The machine's current directory structure has C:\SRC, which DOSMGR
    // keeps for it, whatever another machine changed to since.
    vm_page_tables(&mut rig, &[]);
    assert!(rig.cpu.bus.disk.set_current_directory("C:\\SRC"));
    rust_dos::dos_data::write_cds(&mut rig.cpu.bus, 2);
    assert!(rig.cpu.bus.disk.set_current_directory("C:\\"));
    let v86 = asm16(0x30000, |a| {
        a.mov(ax, 0x3000u32)?;
        a.mov(ds, ax)?;
        a.mov(si, 0x200u32)?;
        a.mov(dl, 3u32)?;
        a.mov(ah, 0x47u32)?;
        a.pushf()?;
        a.db(&INT21)?;
        a.int(0x40)
    });
    rig.handler(0x40, 3, |a| record_code(a, 0x40));
    run_v86_paged(&mut rig, &v86);
    assert_eq!(rig.recorded().0, 0x40);
    let dir: Vec<u8> = (0..4).map(|i| rig.cpu.bus.read_8(0x30200 + i)).collect();
    assert_eq!(dir, b"SRC\0");
}

#[test]
fn the_keyboard_interrupt_reads_its_scan_code_through_the_ports_in_virtual_8086_mode() {
    let mut rig = Rig::new();
    // IRQ 1 as a monitor reflects it (Windows' VKD): the scan code is read,
    // and the interrupt acknowledged, where the monitor traps them, as it
    // hands the machine its keys one at a time.
    let v86 = asm16(0x30000, |a| {
        a.pushf()?;
        a.db(&[0x9A, 0x04, 0x10, 0x00, 0xF0])?; // CALL FAR F000:1004, INT 09h
        a.int(0x40)
    });
    rig.load(0x30000, &v86);
    rig.record(GP);
    rig.run(|a| {
        for v in [0u32, 0, 0, 0, 0x2000, 0xFFFE, 0x0002_3002, 0x3000, 0] {
            a.push(v)?;
        }
        a.iretd()
    });
    let (vector, stack) = rig.recorded();
    assert_eq!((vector, stack[2]), (GP as u32, 0xF000));
    assert_eq!(rig.cpu.dx(), 0x60, "reading the scan code");
}

#[test]
fn dos_writes_on_a_virtual_machines_own_screen() {
    let mut rig = Rig::new();
    // The machine's text screen is in memory of its own at 310000h, as
    // Windows keeps a DOS box's in a window.
    vm_page_tables(&mut rig, &[(0xB8, 0x310)]);
    let (col, row) = (rig.cpu.bus.read_8(0x0450) as usize, rig.cpu.bus.read_8(0x0451) as usize);
    let v86 = asm16(0x30000, |a| {
        a.mov(ax, 0x3000u32)?;
        a.mov(ds, ax)?;
        a.mov(dx, 0x200u32)?;
        a.mov(ah, 0x09u32)?;
        a.pushf()?;
        a.db(&INT21)?;
        a.int(0x40)
    });
    rig.load(0x30200, b"Hi$");
    rig.handler(0x40, 3, |a| record_code(a, 0x40));
    // The cursor's CRTC registers, made through the ports on the way out.
    rig.record(GP);
    run_v86_paged(&mut rig, &v86);
    let cell = (row * 80 + col) * 2;
    let at = |rig: &Rig, base: usize| [rig.cpu.bus.read_8(base + cell), rig.cpu.bus.read_8(base + cell + 2)];
    assert_eq!(at(&rig, 0x31_0000), *b"Hi");
    assert_ne!(at(&rig, 0xB_8000), *b"Hi", "the card's memory");
    assert_eq!(rig.cpu.bus.read_8(0x0450) as usize, col + 2, "the machine's cursor");
}

#[test]
fn lar_lsl_verr_verw_and_arpl() {
    let mut rig = Rig::new();
    rig.set_gdt(FREE, seg_desc(DATA, 0x1234, 0x90, 0x4));
    rig.run(|a| {
        a.mov(ecx, FREE as u32)?;
        a.lar(eax, ecx)?;
        a.lsl(ebx, ecx)?;
        a.verr(cx)?;
        a.setz(dl)?;
        a.verw(cx)?;
        a.setz(dh)?;
        a.mov(esi, 0x0008u32)?;
        a.mov(edi, 0x0003u32)?;
        a.arpl(si, di)?;
        a.hlt()
    });
    assert_eq!(rig.cpu.eax(), 0x0040_9000);
    assert_eq!(rig.cpu.ebx(), 0x1234);
    assert_eq!(rig.cpu.dx(), 0x0001, "readable, not writable");
    assert_eq!(rig.cpu.si(), 0x000B);
}

/// A 32-bit LAR reads the limit's bits 16-19 as well as the flags, which
/// Windows 95 writes back to set a descriptor's AVL bit.
#[test]
fn lar_reads_the_high_limit_bits() {
    let mut rig = Rig::new();
    rig.set_gdt(FREE, seg_desc(0x8040_0000, 0x3_64DF, 0xF3, 0x1));
    rig.run(|a| {
        a.mov(ecx, FREE as u32 | 3)?;
        a.lar(eax, ecx)?;
        a.lar(bx, cx)?;
        a.hlt()
    });
    assert_eq!(rig.cpu.eax(), 0x0013_F300);
    assert_eq!(rig.cpu.bx(), 0xF300);
}

#[test]
fn back_to_real_mode_with_a_flat_ds() {
    let mut rig = Rig::new();
    let real = asm16(0x3000, |a| {
        a.mov(eax, cr0)?;
        a.and(eax, 0xFFFF_FFFEu32)?;
        a.mov(cr0, eax)?;
        // JMP 0000:3100 reloads CS as a real-mode segment.
        a.jmp_far(0, 0x3100)?;
        Ok(())
    });
    rig.load(0x3000, &real);
    let after = asm16(0x3100, |a| {
        a.xor(ax, ax)?;
        a.mov(ds, ax)?;
        a.mov(ebx, 0x0020_0000u32)?;
        a.mov(dword_ptr(ebx), 0x5A5Au32)?;
        a.hlt()
    });
    rig.load(0x3100, &after);
    rig.run(|a| {
        a.jmp_far(CODE16, 0x3000)?;
        Ok(())
    });
    assert!(!rig.cpu.pe());
    assert_eq!(rig.cpu.cs(), 0);
    assert_eq!(rig.read32(0x20_0000), 0x5A5A, "DS kept its 4 GB limit");
}

// --- Running in batches ---
//
// The emulator runs programs through `run_batch`, which keeps the page
// instructions come from as a code window instead of translating every
// fetch. These tests change what that translation depends on while code
// runs on in the same page.

/// Code for linear page 30000h that points the page's table entry at
/// physical 31000h, flushes the TLB with `flush`, and jumps to offset 40h of
/// the page, where BX gets `marker`. It goes at both 30000h and 31000h, with
/// different markers, so BX tells which page the jump landed in.
fn remap_and_jump(marker: u32, flush: fn(&mut CodeAssembler) -> Result<(), IcedError>) -> Vec<u8> {
    let mut code = asm32(0x30000, |a| {
        a.mov(dword_ptr(0x81000 + 0x30 * 4), 0x31003u32)?;
        flush(a)?;
        a.jmp(0x30040u64)
    });
    assert!(code.len() <= 0x40);
    code.resize(0x40, 0x90);
    code.extend(asm32(0x30040, |a| {
        a.mov(ebx, marker)?;
        a.hlt()
    }));
    code
}

fn run_remap(flush: fn(&mut CodeAssembler) -> Result<(), IcedError>) -> Rig {
    let mut rig = Rig::new();
    page_tables(&mut rig);
    rig.load(0x30000, &remap_and_jump(0xAAAA, flush));
    rig.load(0x31000, &remap_and_jump(0xBBBB, flush));
    rig.run_batched(|a| {
        enable_paging(a)?;
        a.mov(eax, 0x30000u32)?;
        a.jmp(eax)
    });
    rig
}

#[test]
fn batched_code_follows_its_page_remapped_by_a_cr3_load() {
    let rig = run_remap(|a| {
        a.mov(eax, cr3)?;
        a.mov(cr3, eax)
    });
    assert_eq!(rig.cpu.ebx(), 0xBBBB);
}

#[test]
fn batched_code_follows_its_page_remapped_by_invlpg() {
    let rig = run_remap(|a| a.invlpg(ptr(0x30000)));
    assert_eq!(rig.cpu.ebx(), 0xBBBB);
}

#[test]
fn batched_ring_3_code_on_a_supervisor_page_faults() {
    let mut rig = Rig::new();
    page_tables(&mut rig);
    // Ring 3 may use pages whose entries say so: only its stack. The code
    // page 30000h is the supervisor's.
    rig.write32(0x80000, 0x81000 | 0x7);
    rig.write32(0x81000 + 0x5F * 4, 0x5F000 | 0x7);
    rig.record(PF);
    // Ring 0 code on the page IRETs to ring 3 code further down it.
    let mut code = asm32(0x30000, |a| {
        a.push(DATA32_R3 as u32)?;
        a.push(STACK3_TOP)?;
        a.pushfd()?;
        a.push(CODE32_R3 as u32)?;
        a.push(0x30080u32)?;
        a.iretd()
    });
    code.resize(0x80, 0x90);
    code.extend(asm32(0x30080, |a| {
        a.mov(ebx, 0x3333u32)?;
        a.hlt()
    }));
    rig.load(0x30000, &code);
    rig.run_batched(|a| {
        enable_paging(a)?;
        a.mov(eax, 0x30000u32)?;
        a.jmp(eax)
    });
    let (vector, stack) = rig.recorded();
    assert_eq!((vector, stack[0]), (PF as u32, 0x5), "user fetch from a supervisor page");
    assert_eq!(rig.cpu.cr2, 0x30080);
    assert_ne!(rig.cpu.ebx(), 0x3333);
}

#[test]
fn batched_code_follows_the_a20_gate() {
    // Real mode at FFFF:0110, linear 100100h: with A20 on, physical
    // 100100h, with it off, 000100h. The code turns A20 off through port
    // 92h and jumps ahead; BX tells which copy it landed in.
    let block = |marker: u16| {
        let mut code = asm16(0x110, |a| {
            a.in_(al, 0x92)?;
            a.and(al, 0xFD)?;
            a.out(0x92, al)?;
            a.jmp(0x130u64)
        });
        assert!(code.len() <= 0x20);
        code.resize(0x20, 0x90);
        code.extend(asm16(0x130, |a| {
            a.mov(bx, marker as u32)?;
            a.hlt()
        }));
        code
    };
    let mut rig = Rig::new();
    rig.load(0x10_0100, &block(0xAAAA));
    rig.load(0x100, &block(0xBBBB));
    rig.cpu.set_cs(0xFFFF);
    rig.cpu.set_ip(0x110);
    rig.run_batched_to_halt();
    assert!(!rig.cpu.bus.a20());
    assert_eq!(rig.cpu.bx(), 0xBBBB);
}

#[test]
fn batched_instruction_running_past_the_cs_limit_raises_gp() {
    let mut rig = Rig::new();
    rig.record(GP);
    // A 32-bit code segment at 30000h, byte granular, whose limit FFFh is
    // the fourth byte of a 5-byte MOV at FFCh.
    rig.set_gdt(FREE, seg_desc(0x30000, 0x0FFF, CODE_R0, 0x4));
    let mut code = vec![0x90; 0xFC];
    code.extend(asm32(0xFFC, |a| a.mov(eax, 0x1234_5678u32)));
    rig.load(0x30F00, &code);
    rig.run_batched(|a| a.jmp_far(FREE, 0xF00));
    let (vector, stack) = rig.recorded();
    assert_eq!((vector, stack[0], stack[1]), (GP as u32, 0, 0xFFC));
    assert_ne!(rig.cpu.eax(), 0x1234_5678);
}

#[test]
fn batched_code_sees_its_own_changes() {
    let mut rig = Rig::new();
    // A subroutine in the same page: MOV AL, 0; RET.
    rig.load(0x10100, &asm32(0x10100, |a| {
        a.mov(al, 0)?;
        a.ret()
    }));
    rig.run_batched(|a| {
        a.call(0x10100u64)?;
        a.mov(ebx, eax)?;
        // Rewrite the immediate and call it again.
        a.mov(byte_ptr(0x10101), 0x42)?;
        a.call(0x10100u64)?;
        a.hlt()
    });
    assert_eq!(rig.cpu.ebx() & 0xFF, 0);
    assert_eq!(rig.cpu.eax() & 0xFF, 0x42);
}

/// Page tables as `page_tables` makes them, but open to ring 3 (and so to
/// virtual-8086 mode).
fn user_page_tables(rig: &mut Rig) {
    page_tables(rig);
    rig.write32(0x80000, 0x81000 | 0x7);
    for i in 0..1024u32 {
        rig.write32(0x81000 + 4 * i, (i << 12) | 0x7);
    }
}

/// V86 code at 3000:0000 that asks INT 1Ah for the ticks and INT 15h
/// AX=E820h for the first entry of the memory map into 3100:0000, calling
/// the BIOS's services as a monitor reflects them, then stores the ticks
/// (CX:DX) and EAX at 3000:0100 and raises INT 40h.
fn v86_calls_the_bios(rig: &mut Rig) {
    let v86 = asm16(0x30000, |a| {
        a.mov(ax, 0x3000u32)?;
        a.mov(ds, ax)?;
        a.mov(ah, 0u32)?;
        a.pushf()?;
        a.db(&[0x9A, 0x28, 0x10, 0x00, 0xF0])?; // CALL FAR F000:1028 (INT 1Ah)
        a.mov(word_ptr(0x100), dx)?;
        a.mov(word_ptr(0x102), cx)?;
        a.mov(ax, 0x3100u32)?;
        a.mov(es, ax)?;
        a.xor(di, di)?;
        a.mov(eax, 0xE820u32)?;
        a.mov(edx, 0x534D_4150u32)?;
        a.xor(ebx, ebx)?;
        a.mov(ecx, 20u32)?;
        a.pushf()?;
        a.db(&[0x9A, 0x1C, 0x10, 0x00, 0xF0])?; // CALL FAR F000:101C (INT 15h)
        a.mov(dword_ptr(0x104), eax)?;
        a.int(0x40)
    });
    rig.load(0x30000, &v86);
}

/// Enter V86 mode at 3000:0000 with paging on.
fn enter_v86_with_paging(a: &mut CodeAssembler) -> Result<(), IcedError> {
    enable_paging(a)?;
    for v in [0u32, 0, 0, 0, 0x2000, 0xFFFE, 0x0002_3002, 0x3000, 0] {
        a.push(v)?;
    }
    a.iretd()
}

#[test]
fn bios_services_in_virtual_8086_mode_reach_memory_through_the_page_tables() {
    let mut rig = Rig::new();
    user_page_tables(&mut rig);
    // The machine's page 0 (vectors and BIOS data) and its buffer at 31000h
    // are elsewhere in physical memory, as a Windows VM's are.
    for i in 0..0x1000u32 {
        let byte = rig.cpu.bus.read_8(i as usize);
        rig.cpu.bus.write_8(0x90000 + i as usize, byte);
    }
    rig.write32(0x9046C, 0x0012_3456);
    rig.write32(0x046C, 0x0077_7777);
    rig.write32(0x81000, 0x90000 | 0x7);
    rig.write32(0x81000 + 0x31 * 4, 0x92000 | 0x7);
    v86_calls_the_bios(&mut rig);
    rig.handler(0x40, 3, |a| record_code(a, 0x40));
    rig.run(enter_v86_with_paging);
    assert_eq!(rig.recorded().0, 0x40, "back in the V86 code");
    assert_eq!(rig.read32(0x30100), 0x0012_3456, "the machine's own ticks");
    assert_eq!(rig.read32(0x30104), 0x534D_4150, "SMAP");
    assert_eq!(rig.read32(0x92008), 0xA0000, "the map's first entry, in the buffer's page");
    assert_eq!(rig.read32(0x31008), 0, "not at its linear address");
    assert_eq!(rig.read32(0x81000 + 0x31 * 4) & 0x60, 0x60, "accessed and dirty");
}

#[test]
fn a_bios_service_that_finds_a_page_missing_faults_and_runs_again() {
    let mut rig = Rig::new();
    user_page_tables(&mut rig);
    // The buffer's page isn't there; the page fault handler puts it at
    // 92000h and returns to the service, which runs again.
    rig.write32(0x81000 + 0x31 * 4, 0);
    v86_calls_the_bios(&mut rig);
    rig.handler(0x40, 3, |a| record_code(a, 0x40));
    rig.handler(PF, 0, |a| {
        a.push(eax)?;
        // Coming from V86 mode, DS is null.
        a.mov(ax, DATA32 as u32)?;
        a.mov(ds, ax)?;
        a.mov(eax, dword_ptr(esp + 4))?;
        a.mov(dword_ptr(RESULT + 0x40), eax)?;
        a.mov(eax, cr2)?;
        a.mov(dword_ptr(RESULT + 0x44), eax)?;
        a.mov(eax, dword_ptr(esp + 8))?;
        a.mov(dword_ptr(RESULT + 0x48), eax)?;
        a.mov(eax, dword_ptr(esp + 12))?;
        a.mov(dword_ptr(RESULT + 0x4C), eax)?;
        a.mov(dword_ptr(0x81000 + 0x31 * 4), 0x92000u32 | 0x7)?;
        a.pop(eax)?;
        a.add(esp, 4)?;
        a.iretd()
    });
    rig.run(enter_v86_with_paging);
    assert_eq!(rig.read32(RESULT + 0x40), 0x6, "a user write to a page not there");
    assert_eq!(rig.read32(RESULT + 0x44), 0x31000, "CR2");
    assert_eq!((rig.read32(RESULT + 0x4C), rig.read32(RESULT + 0x48)), (0xF000, 0x101C), "on the INT 15h trap");
    assert_eq!(rig.recorded().0, 0x40, "back in the V86 code");
    assert_eq!(rig.read32(0x30104), 0x534D_4150, "the service ran again");
    assert_eq!(rig.read32(0x92008), 0xA0000);
}

#[test]
fn fpu_operands_across_a_page_boundary_follow_the_page_tables() {
    // Linear pages 50h and 51h map to physical 90000h and 93000h, so an
    // operand at the end of the first continues on a page that isn't next
    // to it physically.
    let mut rig = Rig::new();
    page_tables(&mut rig);
    rig.write32(0x81000 + 0x50 * 4, 0x90000 | 0x3);
    rig.write32(0x81000 + 0x51 * 4, 0x93000 | 0x3);
    rig.write32(DATA, 1.5f32.to_bits());
    rig.run(|a| {
        enable_paging(a)?;
        a.fld(dword_ptr(DATA))?;
        // A dword split 2 + 2, and a qword split 3 + 5.
        a.fst(dword_ptr(0x50FFE))?;
        a.fstp(qword_ptr(0x50FFD))?;
        a.fld(dword_ptr(0x50FFE))?;
        a.fstp(dword_ptr(DATA + 4))?;
        a.hlt()
    });
    // The qword store overwrote the dword; read what's there byte by byte.
    let bytes: Vec<u8> = (0..3).map(|i| rig.cpu.bus.read_8(0x90FFD + i)).chain((0..5).map(|i| rig.cpu.bus.read_8(0x93000 + i))).collect();
    assert_eq!(f64::from_le_bytes(bytes.try_into().unwrap()), 1.5);
    assert_eq!(rig.read32(0x51000), 0, "nothing at the physical page after the first");
    let dword = rig.read32(DATA + 4);
    let expected = u32::from_le_bytes([
        rig.cpu.bus.read_8(0x90FFE),
        rig.cpu.bus.read_8(0x90FFF),
        rig.cpu.bus.read_8(0x93000),
        rig.cpu.bus.read_8(0x93001),
    ]);
    assert_eq!(dword, expected, "the load reads the same split bytes back");
}

#[test]
fn a_segment_loaded_again_sees_its_descriptor_as_it_is_now() {
    // DS loaded with the same selector again and again, while its
    // descriptor changes: a new base, the accessed bit cleared, the
    // table moved (LGDT of a copy with another base), and not present.
    let mut rig = Rig::new();
    rig.record(NP);
    let desc = |base: u32| seg_desc(base, 0xFFF, DATA_R0, 0x4);
    rig.set_gdt(FREE, desc(DATA));
    for (at, v) in [(DATA, 0x1111_1111u32), (DATA + 0x100, 0x2222_2222), (DATA + 0x200, 0x3333_3333)] {
        rig.write32(at, v);
    }
    // A copy of the GDT at 3000h, where FREE's base is DATA + 200h.
    let copy = 0x3000;
    for i in (0..0x800).step_by(4) {
        let v = rig.read32(GDT + i);
        rig.write32(copy + i, v);
    }
    let free = desc(DATA + 0x200);
    rig.write32(copy + FREE as u32, free as u32);
    rig.write32(copy + FREE as u32 + 4, (free >> 32) as u32);
    rig.write16(0x7B10, 0x07FF);
    rig.write32(0x7B12, copy);
    let entry = GDT + FREE as u32;
    let moved = desc(DATA + 0x100);
    rig.run(|a| {
        a.mov(ax, FREE as u32)?;
        a.mov(ds, ax)?;
        a.mov(ebx, dword_ptr(0))?;
        // A new base.
        a.mov(ax, DATA32 as u32)?;
        a.mov(es, ax)?;
        a.mov(dword_ptr(entry as u64).es(), moved as u32)?;
        a.mov(dword_ptr(entry as u64 + 4).es(), (moved >> 32) as u32)?;
        a.mov(ax, FREE as u32)?;
        a.mov(ds, ax)?;
        a.mov(edx, dword_ptr(0))?;
        // Not accessed any more: the load marks it again.
        a.and(byte_ptr(entry as u64 + 5).es(), 0xFEu32)?;
        a.mov(ds, ax)?;
        // Another table.
        a.lgdt(ptr(0x7B10).es())?;
        a.mov(ds, ax)?;
        a.mov(ebp, dword_ptr(0))?;
        // Not present.
        a.and(byte_ptr(copy as u64 + FREE as u64 + 5).es(), 0x7Fu32)?;
        a.mov(ds, ax)?;
        a.hlt()
    });
    assert_eq!((rig.cpu.ebx(), rig.cpu.edx(), rig.cpu.ebp()), (0x1111_1111, 0x2222_2222, 0x3333_3333));
    assert_eq!(rig.gdt(FREE) >> 40 & 1, 1, "accessed again");
    let (vector, stack) = rig.recorded();
    assert_eq!(vector, NP as u32);
    assert_eq!(stack[0], FREE as u32, "error code");
}

#[test]
fn a_segment_loaded_again_after_its_table_was_mapped_elsewhere_uses_the_new_page() {
    // With paging, the GDT's page mapped to a copy of it where DS's
    // descriptor has another base (INVLPG after): loading DS again reads
    // the copy.
    let mut rig = Rig::new();
    page_tables(&mut rig);
    rig.set_gdt(FREE, seg_desc(DATA, 0xFFF, DATA_R0, 0x4));
    rig.write32(DATA, 0x1111_1111);
    rig.write32(DATA + 0x100, 0x2222_2222);
    let copy = 0x9_0000;
    for i in (0..0x1000).step_by(4) {
        let v = rig.read32(i);
        rig.write32(copy + i, v);
    }
    let moved = seg_desc(DATA + 0x100, 0xFFF, DATA_R0, 0x4) | 1 << 40;
    rig.write32(copy + GDT + FREE as u32, moved as u32);
    rig.write32(copy + GDT + FREE as u32 + 4, (moved >> 32) as u32);
    rig.run(|a| {
        enable_paging(a)?;
        a.mov(ax, FREE as u32)?;
        a.mov(ds, ax)?;
        a.mov(ebx, dword_ptr(0))?;
        a.mov(ax, DATA32 as u32)?;
        a.mov(es, ax)?;
        a.mov(dword_ptr(0x81000u64).es(), (copy | 3) as u32)?;
        a.invlpg(ptr(GDT).es())?;
        a.mov(ax, FREE as u32)?;
        a.mov(ds, ax)?;
        a.mov(edx, dword_ptr(0))?;
        a.hlt()
    });
    assert_eq!((rig.cpu.ebx(), rig.cpu.edx()), (0x1111_1111, 0x2222_2222));
}

#[test]
fn a_segment_loaded_again_at_another_privilege_level_is_checked_again() {
    // DS loaded with a DPL 0 selector at ring 0, then with the same
    // selector (RPL 0) at ring 3: #GP(selector) there.
    let mut rig = Rig::new();
    rig.record(GP);
    rig.set_gdt(FREE, seg_desc(DATA, 0xFFF, DATA_R0, 0x4));
    rig.ring3(|a| {
        a.mov(ax, FREE as u32)?;
        a.mov(ds, ax)?;
        a.int3()
    });
    rig.run(|a| {
        a.mov(ax, FREE as u32)?;
        a.mov(ds, ax)?;
        a.mov(ax, DATA32 as u32)?;
        a.mov(ds, ax)?;
        to_ring3(a)
    });
    let (vector, stack) = rig.recorded();
    assert_eq!((vector, stack[0]), (GP as u32, FREE as u32));
    assert_eq!(stack[2], CODE32_R3 as u32);
}

#[test]
fn a_segment_loaded_into_ds_is_checked_again_for_ss() {
    // A read-only data selector loads into DS, and then not into SS, which
    // must be writable: #GP(selector).
    let mut rig = Rig::new();
    rig.record(GP);
    rig.set_gdt(FREE, seg_desc(DATA, 0xFFF, DATA_R0 & !2, 0x4));
    rig.run(|a| {
        a.mov(ax, FREE as u32)?;
        a.mov(ds, ax)?;
        a.mov(ss, ax)?;
        a.hlt()
    });
    let (vector, stack) = rig.recorded();
    assert_eq!((vector, stack[0]), (GP as u32, FREE as u32));
}

/// A booted machine whose hard disk is on the primary IDE channel, with
/// INT 13h called in virtual-8086 mode as a monitor reflects it, with the
/// IDE ports trapped (no I/O permission bitmap).
fn int13_in_v86(function: u8) -> Rig {
    use rust_dos::diskimage::DiskImage;
    let mut rig = Rig::new();
    let disk = DiskImage::blank_hard_disk("ide.img", 8 << 20, None).unwrap();
    rig.cpu.bus.mount_disk_image(2, disk, rust_dos::disk::MountOptions::default()).unwrap();
    rig.cpu.bus.boot = Some(Default::default());
    rig.cpu.bus.attach_ide();
    assert!(rig.cpu.bus.ide[0].is_some());
    // INT 13h: one sector from 0/0/1 to 3000:0200.
    let v86 = asm16(0x30000, |a| {
        a.mov(ax, 0x3000u32)?;
        a.mov(es, ax)?;
        a.mov(bx, 0x0200u32)?;
        a.mov(ax, (function as u32) << 8 | 1)?;
        a.mov(cx, 0x0001u32)?;
        a.mov(dx, 0x0080u32)?;
        a.pushf()?;
        a.db(&[0x9A, 0x14, 0x10, 0x00, 0xF0])?; // CALL FAR F000:1014
        a.int(0x40)
    });
    rig.load(0x30000, &v86);
    rig.record(GP);
    rig.handler(0x40, 3, |a| record_code(a, 0x40));
    rig.run(|a| {
        for v in [0u32, 0, 0, 0, 0x2000, 0xFFFE, 0x0002_3002, 0x3000, 0] {
            a.push(v)?;
        }
        a.iretd()
    });
    rig
}

/// Windows 9x's IDE driver watches the ports the BIOS drives for INT 13h:
/// a read's accesses go through the BIOS's replay, where the trapped
/// ports fault for the monitor.
#[test]
fn int13_in_virtual_8086_mode_drives_the_ide_ports_for_a_monitor_to_see() {
    use rust_dos::bios::PortAccess;
    let rig = int13_in_v86(0x02);
    let (vector, stack) = rig.recorded();
    assert_eq!((vector, stack[2]), (GP as u32, 0xF000), "the first access faults in the ROM");
    assert!((rust_dos::bios::PORT_ACCESSES as u32..rust_dos::bios::PORT_ACCESSES as u32 + 0x40).contains(&stack[1]));
    assert_eq!(rig.cpu.dx(), 0x1F7, "reading the status");
    let queue: Vec<PortAccess> = rig.cpu.bus.port_accesses.iter().copied().collect();
    assert_eq!(queue[0], PortAccess::Out(0x1F6, 0x00), "the master selected");
    assert!(queue.contains(&PortAccess::Cli));
    assert!(queue.contains(&PortAccess::Out(0x1F3, 1)), "sector 1");
    assert!(queue.contains(&PortAccess::Out(0x1F7, 0x20)), "READ SECTORS");
    assert!(queue.contains(&PortAccess::WaitWhile { port: 0x3F6, mask: 0x80 }));
    assert!(queue.contains(&PortAccess::InWords { port: 0x1F0, count: 256 }));
    assert_eq!(&queue[queue.len() - 2..], [PortAccess::Out(0xA0, 0x66), PortAccess::Faked(false)]);
    assert_eq!(rig.cpu.bus.read_16(0x301FE + 0x200), 0xAA55, "the sector was read all the same");
}

/// A reset (AH=00h) too: DEVICE RESET.
#[test]
fn int13_reset_in_virtual_8086_mode_resets_the_ide_disk_for_a_monitor_to_see() {
    use rust_dos::bios::PortAccess;
    let rig = int13_in_v86(0x00);
    let queue: Vec<PortAccess> = rig.cpu.bus.port_accesses.iter().copied().collect();
    assert!(queue.contains(&PortAccess::Out(0x1F7, 0x08)));
    assert!(!queue.contains(&PortAccess::Cli));
}

/// The replay runs to its end where the monitor lets the ports through
/// after the first: the ROM's loop reads the sector from the IDE disk
/// with CLI, the wait for BSY and the words of data, then the IRET.
#[test]
fn the_bios_replay_of_an_ide_read_runs_through() {
    use rust_dos::diskimage::DiskImage;
    let mut rig = Rig::new();
    let disk = DiskImage::blank_hard_disk("ide.img", 8 << 20, None).unwrap();
    rig.cpu.bus.mount_disk_image(2, disk, rust_dos::disk::MountOptions::default()).unwrap();
    rig.cpu.bus.boot = Some(Default::default());
    rig.cpu.bus.attach_ide();
    // An I/O permission bitmap that traps 1F7h alone; the monitor lets it
    // through at its first fault.
    rig.set_gdt(TSS_SEL, sys_desc(TSS, 0x68 + 0x80, TSS32, 0));
    for i in 0..0x80 {
        rig.cpu.bus.write_8((TSS + 0x68 + i) as usize, 0);
    }
    rig.cpu.bus.write_8((TSS + 0x68 + 0x80) as usize, 0xFF);
    rig.cpu.bus.write_8((TSS + 0x68 + 0x1F7 / 8) as usize, 0x80);
    rig.handler(GP, 0, |a| {
        a.push(eax)?;
        a.mov(ax, DATA32 as u32)?;
        a.mov(ds, ax)?;
        a.and(byte_ptr(TSS + 0x68 + 0x1F7 / 8), 0x7F)?;
        a.pop(eax)?;
        a.add(esp, 4)?;
        a.iretd()
    });
    let v86 = asm16(0x30000, |a| {
        a.mov(ax, 0x3000u32)?;
        a.mov(es, ax)?;
        a.mov(bx, 0x0200u32)?;
        a.mov(ax, 0x0201u32)?;
        a.mov(cx, 0x0001u32)?;
        a.mov(dx, 0x0080u32)?;
        a.pushf()?;
        a.db(&[0x9A, 0x14, 0x10, 0x00, 0xF0])?; // CALL FAR F000:1014
        a.int(0x40)
    });
    rig.load(0x30000, &v86);
    rig.handler(0x40, 3, |a| record_code(a, 0x40));
    // In batches, as the emulator runs programs: the disk's timed commands
    // come due.
    rig.run_batched(|a| {
        for v in [0u32, 0, 0, 0, 0x2000, 0xFFFE, 0x0002_3202, 0x3000, 0] {
            a.push(v)?;
        }
        a.iretd()
    });
    let (vector, stack) = rig.recorded();
    assert_eq!((vector, stack[1]), (0x40, 0x3000), "back in the V86 code after the replay");
    assert!(rig.cpu.bus.port_accesses.is_empty());
    assert!(!rig.cpu.bus.ide_faked);
    assert!(stack[2] & 0x200 != 0, "the caller's IF, back from the IRET");
    // The disk read the sector for the replay and is done.
    let status = rig.cpu.bus.io_read(0x3F6);
    assert_eq!(status & 0x89, 0, "not busy, no data left, no error: {:02X}", status);
    assert_eq!((rig.cpu.bus.io_read(0x1F3), rig.cpu.bus.io_read(0x1F2)), (1, 0));
}
