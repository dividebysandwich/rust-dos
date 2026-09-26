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
    assert!((rust_dos::bios::PORT_ACCESSES as u32..rust_dos::bios::PORT_ACCESSES as u32 + 0x20).contains(&stack[1]));
    assert_eq!((rig.cpu.dx(), rig.cpu.get_al()), (0x3C2, 0xE3));
    assert!(!rig.cpu.bus.port_accesses.is_empty(), "more to write");
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
fn exec_refuses_a_virtual_machine_whose_memory_is_elsewhere() {
    let mut rig = Rig::new();
    // Paging with conventional memory where its addresses say but for the
    // page at 20000h, as a DOS machine of Windows' 386 enhanced mode has
    // memory of its own.
    page_tables(&mut rig);
    rig.write32(0x80000, 0x81000 | 0x7);
    for i in 0..1024u32 {
        rig.write32(0x81000 + 4 * i, (i << 12) | 0x7);
    }
    rig.write32(0x81000 + 4 * 0x20, (0x300 << 12) | 0x7);
    let v86 = asm16(0x30000, |a| {
        a.mov(ax, 0x3000u32)?;
        a.mov(ds, ax)?;
        a.mov(es, ax)?;
        a.mov(dx, 0x200u32)?;
        a.mov(bx, 0x210u32)?;
        a.mov(ax, 0x4B00u32)?;
        a.pushf()?;
        a.db(&[0x9A, 0x30, 0x10, 0x00, 0xF0])?; // CALL FAR F000:1030, INT 21h
        a.mov(word_ptr(0x100), ax)?;
        a.pushf()?;
        a.pop(ax)?;
        a.mov(word_ptr(0x102), ax)?;
        a.int(0x40)
    });
    rig.load(0x30000, &v86);
    rig.load(0x30200, b"X.COM\0");
    rig.handler(0x40, 3, |a| record_code(a, 0x40));
    rig.run(|a| {
        enable_paging(a)?;
        for v in [0u32, 0, 0, 0, 0x2000, 0xFFFE, 0x0002_3002, 0x3000, 0] {
            a.push(v)?;
        }
        a.iretd()
    });
    assert_eq!(rig.recorded().0, 0x40);
    assert_eq!(rig.cpu.bus.read_16(0x30100), 0x0008, "insufficient memory");
    assert_ne!(rig.cpu.bus.read_16(0x30102) & 0x0001, 0, "CF");
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
