//! The Pentium: the ID flag and CPUID, the time stamp counter, MSRs,
//! CMPXCHG8B, CR4 and 4 MB pages, and what a 486 does with them.

mod pmrig;
mod testrunners;

use iced_x86::code_asm::*;
use pmrig::*;
use rust_dos::cpu::{Cpu, CpuFlags, CpuModel};
use rust_dos::savestate::machine;
use testrunners::run_cpu_code;

const UD: u8 = 6;
const GP: u8 = 13;
const PF: u8 = 14;

fn cpu(model: CpuModel) -> Cpu {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    cpu.model = model;
    cpu.set_ss(0);
    cpu.set_sp(0x8000);
    cpu
}

fn pentium_rig() -> Rig {
    let mut rig = Rig::new();
    rig.cpu.model = CpuModel::Pentium;
    rig
}

#[test]
fn eflags_id_bit_tells_a_pentium_from_a_486() {
    // 66 9C -> PUSHFD ; 66 58 -> POP EAX ; 66 35 00 00 20 00 -> XOR EAX, 200000h ;
    // 66 50 -> PUSH EAX ; 66 9D -> POPFD ; 66 9C -> PUSHFD ; 66 5B -> POP EBX
    let code = [
        0x66, 0x9C, 0x66, 0x58, 0x66, 0x35, 0x00, 0x00, 0x20, 0x00, 0x66, 0x50, 0x66, 0x9D, 0x66, 0x9C,
        0x66, 0x5B,
    ];
    let mut cpu = cpu(CpuModel::Pentium);
    run_cpu_code(&mut cpu, &code);
    assert_ne!(cpu.ebx() & 0x20_0000, 0, "Pentium: ID can be set");
    assert!(cpu.get_cpu_flag(CpuFlags::ID));

    let mut cpu = self::cpu(CpuModel::I486);
    run_cpu_code(&mut cpu, &code);
    assert_eq!(cpu.ebx() & 0x20_0000, 0, "486: ID stays clear");
}

#[test]
fn cpuid_reports_a_genuine_intel_pentium() {
    let mut cpu = cpu(CpuModel::Pentium);
    // 66 31 C0 -> XOR EAX, EAX ; 0F A2 -> CPUID
    run_cpu_code(&mut cpu, &[0x66, 0x31, 0xC0, 0x0F, 0xA2]);
    let vendor: Vec<u8> = [cpu.ebx(), cpu.edx(), cpu.ecx()].iter().flat_map(|r| r.to_le_bytes()).collect();
    assert_eq!((cpu.eax(), &vendor[..]), (1, &b"GenuineIntel"[..]));

    // 66 B8 01 00 00 00 -> MOV EAX, 1 ; 0F A2 -> CPUID
    cpu.set_ip(0x100);
    run_cpu_code(&mut cpu, &[0x66, 0xB8, 0x01, 0x00, 0x00, 0x00, 0x0F, 0xA2]);
    assert_eq!(cpu.eax(), 0x517, "family 5, model 1, stepping 7");
    assert_eq!(cpu.edx(), 0x139, "FPU, PSE, TSC, MSR and CX8");
    assert_eq!((cpu.ebx(), cpu.ecx()), (0, 0));
}

#[test]
fn a_486_has_no_pentium_instructions() {
    // CPUID, RDTSC, RDMSR, WRMSR, CMPXCHG8B [0], MOV EAX, CR4.
    for code in [
        &[0x0F, 0xA2][..],
        &[0x0F, 0x31],
        &[0x0F, 0x32],
        &[0x0F, 0x30],
        &[0x0F, 0xC7, 0x0E, 0x00, 0x00],
        &[0x0F, 0x20, 0xE0],
    ] {
        let mut cpu = cpu(CpuModel::I486);
        cpu.bus.write_16(UD as usize * 4, 0x0700);
        cpu.bus.write_16(UD as usize * 4 + 2, 0x0000);
        run_cpu_code(&mut cpu, code);
        assert_eq!((cpu.cs(), cpu.ip()), (0x0000, 0x0700), "{:02X?} raises #UD", code);
    }
}

#[test]
fn the_time_stamp_counter_counts_instructions_and_can_be_set() {
    let mut cpu = cpu(CpuModel::Pentium);
    // FA -> CLI ; 0F 31 -> RDTSC ; 66 89 C6 -> MOV ESI, EAX ; 90 90 90 ;
    // 0F 31 -> RDTSC
    run_cpu_code(&mut cpu, &[0xFA, 0x0F, 0x31, 0x66, 0x89, 0xC6, 0x90, 0x90, 0x90, 0x0F, 0x31]);
    assert_eq!(cpu.eax().wrapping_sub(cpu.esi()), 5);
    assert_eq!(cpu.tsc(), ((cpu.edx() as u64) << 32 | cpu.eax() as u64) + 1, "RDTSC was the last instruction");

    // 66 B9 10 00 00 00 -> MOV ECX, 10h ; 66 B8 78 56 34 12 -> MOV EAX, 12345678h ;
    // 66 BA 9A 00 00 00 -> MOV EDX, 9Ah ; 0F 30 -> WRMSR ; 0F 31 -> RDTSC ;
    // 66 89 C6 -> MOV ESI, EAX ; 0F 32 -> RDMSR
    cpu.set_ip(0x100);
    run_cpu_code(
        &mut cpu,
        &[
            0x66, 0xB9, 0x10, 0x00, 0x00, 0x00, 0x66, 0xB8, 0x78, 0x56, 0x34, 0x12, 0x66, 0xBA, 0x9A, 0x00, 0x00,
            0x00, 0x0F, 0x30, 0x0F, 0x31, 0x66, 0x89, 0xC6, 0x0F, 0x32,
        ],
    );
    assert_eq!(cpu.esi(), 0x1234_5679, "RDTSC the instruction after WRMSR");
    assert_eq!((cpu.edx(), cpu.eax()), (0x9A, 0x1234_567B), "RDMSR 10h two after it");
}

#[test]
fn the_time_stamp_counter_counts_the_instructions_of_translated_blocks() {
    // Run in batches, as the emulator runs programs: on the dynamic
    // recompiler the loop is a block whose RDTSC reads the count so far.
    let mut rig = pentium_rig();
    rig.run_batched(|a| {
        let mut again = a.create_label();
        a.rdtsc()?;
        a.mov(esi, eax)?;
        a.mov(ecx, 100u32)?;
        a.set_label(&mut again)?;
        a.dec(ecx)?;
        a.jnz(again)?;
        a.rdtsc()?;
        a.sub(eax, esi)?;
        a.hlt()
    });
    // RDTSC, MOV, MOV and 100 times DEC and JNZ.
    assert_eq!(rig.cpu.eax(), 203);
}

#[test]
fn msrs_are_for_level_0_and_unknown_ones_fault() {
    // An MSR a Pentium doesn't have: #GP(0).
    let mut rig = pentium_rig();
    rig.record(GP);
    rig.run(|a| {
        a.mov(ecx, 0x1Bu32)?;
        a.rdmsr()?;
        a.hlt()
    });
    assert_eq!(rig.recorded().0, GP as u32);
    assert_eq!(rig.recorded().1[0], 0, "error code");

    // The performance counters keep what is written to them.
    let mut rig = pentium_rig();
    rig.run(|a| {
        a.mov(ecx, 0x12u32)?;
        a.mov(eax, 0xCAFEu32)?;
        a.mov(edx, 0x12u32)?;
        a.wrmsr()?;
        a.xor(eax, eax)?;
        a.xor(edx, edx)?;
        a.rdmsr()?;
        a.hlt()
    });
    assert_eq!((rig.cpu.edx(), rig.cpu.eax()), (0x12, 0xCAFE));

    // At ring 3, RDMSR and WRMSR raise #GP(0).
    for write in [false, true] {
        let mut rig = pentium_rig();
        rig.record(GP);
        rig.ring3(|a| {
            a.mov(ecx, 0x10u32)?;
            if write { a.wrmsr()? } else { a.rdmsr()? }
            a.hlt()
        });
        rig.run(to_ring3);
        assert_eq!(rig.recorded().0, GP as u32, "write: {write}");
        assert_eq!(rig.recorded().1[2], CODE32_R3 as u32);
    }
}

#[test]
fn cr4_tsd_keeps_rdtsc_to_level_0() {
    for tsd in [false, true] {
        let mut rig = pentium_rig();
        rig.record(GP);
        rig.ring3(|a| {
            a.xor(eax, eax)?;
            a.rdtsc()?;
            a.mov(ebx, 1u32)?;
            a.hlt()
        });
        rig.run(|a| {
            a.mov(eax, if tsd { 4u32 } else { 0 })?;
            a.mov(cr4, eax)?;
            to_ring3(a)
        });
        if tsd {
            assert_eq!(rig.recorded().0, GP as u32);
        } else {
            assert_eq!(rig.cpu.ebx(), 1);
            assert_ne!(rig.cpu.eax(), 0);
        }
    }
}

#[test]
fn cr4_has_the_bits_this_pentium_has() {
    let mut rig = pentium_rig();
    rig.run(|a| {
        a.mov(eax, 0x14u32)?; // PSE and TSD
        a.mov(cr4, eax)?;
        a.mov(ebx, cr4)?;
        a.hlt()
    });
    assert_eq!((rig.cpu.ebx(), rig.cpu.cr4), (0x14, 0x14));

    // VME, which CPUID doesn't report: #GP(0).
    let mut rig = pentium_rig();
    rig.record(GP);
    rig.run(|a| {
        a.mov(eax, 1u32)?;
        a.mov(cr4, eax)?;
        a.hlt()
    });
    assert_eq!(rig.recorded().0, GP as u32);
    assert_eq!(rig.cpu.cr4, 0);

    // A 486 has no CR4.
    let mut rig = Rig::new();
    rig.record(UD);
    rig.run(|a| {
        a.mov(eax, cr4)?;
        a.hlt()
    });
    assert_eq!(rig.recorded().0, UD as u32);
}

#[test]
fn cmpxchg8b_exchanges_quadwords() {
    let mut rig = pentium_rig();
    rig.write32(DATA, 0x1111_1111);
    rig.write32(DATA + 4, 0x2222_2222);
    rig.run(|a| {
        // Equal: ECX:EBX goes to memory.
        a.mov(eax, 0x1111_1111u32)?;
        a.mov(edx, 0x2222_2222u32)?;
        a.mov(ebx, 0x3333_3333u32)?;
        a.mov(ecx, 0x4444_4444u32)?;
        a.cmpxchg8b(qword_ptr(DATA))?;
        a.setz(byte_ptr(RESULT))?;
        // Not equal: memory goes to EDX:EAX.
        a.cmpxchg8b(qword_ptr(DATA))?;
        a.setz(byte_ptr(RESULT + 1))?;
        a.hlt()
    });
    assert_eq!((rig.read32(DATA), rig.read32(DATA + 4)), (0x3333_3333, 0x4444_4444));
    assert_eq!(rig.read32(RESULT) & 0xFFFF, 0x0001, "ZF set, then clear");
    assert_eq!((rig.cpu.eax(), rig.cpu.edx()), (0x3333_3333, 0x4444_4444));

    // A read-only quadword faults even where the values differ, as the
    // processor writes it either way.
    let mut rig = pentium_rig();
    rig.record(GP);
    rig.set_gdt(FREE, seg_desc(0, 0xFFFFF, 0x90, G32));
    rig.run(|a| {
        a.mov(ax, FREE as u32)?;
        a.mov(ds, ax)?;
        a.mov(eax, 1u32)?;
        a.cmpxchg8b(qword_ptr(DATA))?;
        a.hlt()
    });
    assert_eq!(rig.recorded().0, GP as u32);
    assert_eq!(rig.cpu.eax(), 1, "EDX:EAX as it was");
}

fn page_tables(rig: &mut Rig) {
    // Directory at 80000h, one table at 81000h identity-mapping the first
    // 4 MB.
    rig.write32(0x80000, 0x81000 | 0x3);
    for i in 0..1024u32 {
        rig.write32(0x81000 + 4 * i, (i << 12) | 0x3);
    }
}

fn enable_paging(a: &mut CodeAssembler, cr4_bits: u32) -> Result<(), IcedError> {
    a.mov(eax, cr4_bits)?;
    a.mov(cr4, eax)?;
    a.mov(eax, 0x80000u32)?;
    a.mov(cr3, eax)?;
    a.mov(eax, cr0)?;
    a.or(eax, 0x8000_0000u32)?;
    a.mov(cr0, eax)
}

#[test]
fn four_mb_pages_map_through_the_directory() {
    let mut rig = pentium_rig();
    page_tables(&mut rig);
    // Linear 400000h-7FFFFFh: the 4 MB page at 800000h.
    rig.write32(0x80004, 0x80_0000 | 0x83);
    rig.run(|a| {
        enable_paging(a, 0x10)?;
        a.mov(dword_ptr(0x40_1234), 0xABCDu32)?;
        a.hlt()
    });
    assert_eq!(rig.read32(0x80_1234), 0xABCD);
    assert_eq!(rig.read32(0x80004) & 0x60, 0x60, "accessed and dirty");
    assert_eq!(rig.cpu.peek_translate(0x40_1234), Some(0x80_1234));

    // Without CR4.PSE the entry points to a page table, whose entries
    // (zeros there) aren't present.
    let mut rig = pentium_rig();
    page_tables(&mut rig);
    rig.write32(0x80004, 0x80_0000 | 0x83);
    rig.record(PF);
    rig.run(|a| {
        enable_paging(a, 0)?;
        a.mov(dword_ptr(0x40_1234), 0xABCDu32)?;
        a.hlt()
    });
    assert_eq!(rig.recorded().0, PF as u32);
    assert_eq!(rig.cpu.cr2, 0x40_1234);
}

#[test]
fn invlpg_anywhere_in_a_4_mb_page_drops_all_of_it() {
    let mut rig = pentium_rig();
    page_tables(&mut rig);
    rig.write32(0x80004, 0x80_0000 | 0x83);
    rig.write32(0x80_1000, 0x8888);
    rig.write32(0xC0_1000, 0xCCCC);
    rig.run(|a| {
        enable_paging(a, 0x10)?;
        a.mov(eax, dword_ptr(0x40_1000))?;
        // Move the 4 MB page without telling the TLB.
        a.mov(dword_ptr(0x80004), 0xC0_0083u32)?;
        a.mov(ebx, dword_ptr(0x40_1000))?;
        // Another 4 KB piece of the same page.
        a.invlpg(ptr(0x40_0000))?;
        a.mov(ecx, dword_ptr(0x40_1000))?;
        a.hlt()
    });
    assert_eq!(rig.cpu.eax(), 0x8888);
    assert_eq!(rig.cpu.ebx(), 0x8888, "stale TLB entry");
    assert_eq!(rig.cpu.ecx(), 0xCCCC);
}

#[test]
fn a_reset_puts_the_signature_in_dx() {
    let mut cpu = cpu(CpuModel::Pentium);
    cpu.reset();
    assert_eq!(cpu.edx(), 0x517);
    assert_eq!(cpu.tsc(), 0, "the time stamp counter starts over");
}

#[test]
fn save_states_keep_cr4_and_the_time_stamp_counter() {
    let mut rig = pentium_rig();
    rig.run(|a| {
        a.mov(eax, 0x10u32)?;
        a.mov(cr4, eax)?;
        a.mov(ecx, 0x10u32)?;
        a.mov(eax, 0x1234_5678u32)?;
        a.mov(edx, 0x9Au32)?;
        a.wrmsr()?;
        a.hlt()
    });
    let state = machine::save(&rig.cpu);
    let mut b = pentium_rig().cpu;
    machine::load(&mut b, &state).unwrap();
    assert_eq!((b.cr4, b.tsc()), (0x10, rig.cpu.tsc()));
}
