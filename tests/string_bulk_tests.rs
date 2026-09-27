//! REP MOVS and REP STOS do the iterations that stay in plain RAM at once
//! (`instructions::string`, `Cpu::string_bulk`). Each case here runs on a
//! machine with that and one without, in lockstep (tests/dyndiff): they
//! must agree after every batch, overlapping moves, the direction flag,
//! page and segment ends, faults part of the way and video memory
//! included.

mod dyndiff;
mod pmrig;

use chrono::NaiveDate;
use dyndiff::lockstep_with;
use iced_x86::code_asm::*;
use pmrig::*;
use rust_dos::cpu::CoreMode;

const GP: u8 = 13;
const PF: u8 = 14;

/// Two rigs set up by `setup`, both on the interpreter, the first doing the
/// iterations one at a time, in protected mode at CODE.
fn pair(setup: impl Fn(&mut Rig)) -> (Rig, Rig) {
    rust_dos::hosttime::fix(NaiveDate::from_ymd_opt(1995, 4, 11).unwrap().and_hms_opt(12, 34, 56));
    let mut a = Rig::new();
    let mut b = Rig::new();
    a.cpu.string_bulk = false;
    for rig in [&mut a, &mut b] {
        rig.cpu.core = CoreMode::Normal;
        setup(rig);
        rig.enter_pm();
    }
    (a, b)
}

/// Run both in lockstep until they halt.
fn run(a: &mut Rig, b: &mut Rig) {
    lockstep_with(&mut a.cpu, &mut b.cpu, 400, 97, true, |_, _| {}).unwrap();
    let at = |rig: &Rig| rig.cpu.bus.read_8(rig.cpu.seg_cache(rust_dos::cpu::Seg::CS).base.wrapping_add(rig.cpu.eip()) as usize);
    assert_eq!(at(a), 0xF4, "no HLT");
}

/// Bytes the moves copy: none the same as its neighbours.
fn pattern(rig: &mut Rig, at: u32, len: u32) {
    let bytes: Vec<u8> = (0..len).map(|i| (i * 7 + i / 251 + 3) as u8).collect();
    rig.load(at, &bytes);
}

#[test]
fn moves_and_stores_in_ram_are_the_iterations_one_at_a_time() {
    let (mut a, mut b) = pair(|rig| {
        for at in [0x100000, 0x300000, 0x310000, 0x320000, 0x9F000] {
            pattern(rig, at, 0x3000);
        }
        let code = asm32(CODE, |a| {
            // Apart, across pages.
            a.cld()?;
            a.mov(esi, 0x100010u32)?;
            a.mov(edi, 0x200FF0u32)?;
            a.mov(ecx, 3000u32)?;
            a.rep().movsd()?;
            // Each element onto the next: the first byte over and over,
            // and dwords each read whole before they are written.
            a.mov(esi, 0x300000u32)?;
            a.mov(edi, 0x300001u32)?;
            a.mov(ecx, 5000u32)?;
            a.rep().movsb()?;
            a.mov(esi, 0x310000u32)?;
            a.mov(edi, 0x310001u32)?;
            a.mov(ecx, 2000u32)?;
            a.rep().movsd()?;
            // Down, each onto the one below, and apart.
            a.std()?;
            a.mov(esi, 0x321000u32)?;
            a.mov(edi, 0x320FFFu32)?;
            a.mov(ecx, 3000u32)?;
            a.rep().movsw()?;
            a.mov(esi, 0x322000u32)?;
            a.mov(edi, 0x400800u32)?;
            a.mov(ecx, 1500u32)?;
            a.rep().movsd()?;
            a.mov(edi, 0x331000u32)?;
            a.mov(ecx, 2500u32)?;
            a.mov(eax, 0xDEAD_BEEFu32)?;
            a.rep().stosd()?;
            // Up again, stores of each size across pages.
            a.cld()?;
            a.mov(edi, 0x340FFDu32)?;
            a.mov(ecx, 3000u32)?;
            a.mov(eax, 0x1234_5678u32)?;
            a.rep().stosw()?;
            a.mov(ecx, 9000u32)?;
            a.rep().stosb()?;
            // From RAM into the video memory, and out of it.
            a.mov(esi, 0x100000u32)?;
            a.mov(edi, 0x9FF80u32)?;
            a.mov(ecx, 0x40u32)?;
            a.rep().movsd()?;
            a.mov(esi, 0x9FFF0u32)?;
            a.mov(edi, 0x500000u32)?;
            a.mov(ecx, 0x20u32)?;
            a.rep().movsd()?;
            // Within the video memory with write mode 1, where each write
            // stores the latches the read before it loaded.
            a.mov(dx, 0x3CEu32)?;
            a.mov(ax, 0x4105u32)?;
            a.out(dx, ax)?;
            a.mov(esi, 0xA0000u32)?;
            a.mov(edi, 0xA0100u32)?;
            a.mov(ecx, 0x180u32)?;
            a.rep().movsb()?;
            a.mov(ax, 0x4005u32)?;
            a.out(dx, ax)?;
            // 16-bit addressing, SI and DI wrapping around at 64 KB
            // (ADDR16 REP MOVSB, REP STOSW).
            a.mov(esi, 0x1234_FFF0u32)?;
            a.mov(edi, 0x5678_FFE0u32)?;
            a.mov(ecx, 0xABCD_0080u32)?;
            a.db(&[0x67, 0xF3, 0xA4])?;
            a.mov(edi, 0xFFF9u32)?;
            a.mov(ecx, 0x100u32)?;
            a.db(&[0x67, 0xF3, 0x66, 0xAB])?;
            a.hlt()
        });
        rig.load(CODE, &code);
    });
    run(&mut a, &mut b);
    // The first byte over and over.
    assert_eq!(b.read32(0x300000 + 4000), u32::from_le_bytes([3; 4]));
}

#[test]
fn a_page_fault_part_of_the_way_leaves_the_iterations_before_it_done() {
    let (mut a, mut b) = pair(|rig| {
        let (dir, table) = (0x80000u32, 0x81000u32);
        rig.write32(dir, table | 3);
        for i in 0..1024u32 {
            rig.write32(table + 4 * i, (i << 12) | 3);
        }
        rig.write32(table + 0x55 * 4, 0);
        rig.record(PF);
        pattern(rig, 0x53000, 0x1000);
        let code = asm32(CODE, |a| {
            a.mov(eax, dir)?;
            a.mov(cr3, eax)?;
            a.mov(eax, cr0)?;
            a.or(eax, 0x8000_0000u32)?;
            a.mov(cr0, eax)?;
            a.cld()?;
            // Into the missing page from below.
            a.mov(esi, 0x53000u32)?;
            a.mov(edi, 0x54F00u32)?;
            a.mov(ecx, 0x200u32)?;
            a.rep().movsd()?;
            a.hlt()
        });
        rig.load(CODE, &code);
    });
    run(&mut a, &mut b);
    let (vector, _) = b.recorded();
    assert_eq!(vector, PF as u32);
    assert_eq!(b.cpu.cr2, 0x55000);
    // The dwords up to the page, moved.
    assert_eq!(b.read32(0x54FFC), b.read32(0x530FC));
}

#[test]
fn a_segment_limit_part_of_the_way_faults_where_it_is_reached() {
    let (mut a, mut b) = pair(|rig| {
        // ES: 8 KB at 60000h.
        rig.set_gdt(FREE, seg_desc(0x60000, 0x1FFF, DATA_R0, 0x4));
        rig.record(GP);
        let code = asm32(CODE, |a| {
            a.mov(ax, FREE as u32)?;
            a.mov(es, ax)?;
            a.cld()?;
            a.mov(edi, 0x1F00u32)?;
            a.mov(ecx, 0x100u32)?;
            a.mov(eax, 0x5555_AAAAu32)?;
            a.rep().stosd()?;
            a.hlt()
        });
        rig.load(CODE, &code);
    });
    run(&mut a, &mut b);
    let (vector, _) = b.recorded();
    assert_eq!(vector, GP as u32);
    // The dwords up to the limit, stored.
    assert_eq!((b.read32(0x61F00), b.read32(0x61FFC), b.read32(0x62000)), (0x5555_AAAA, 0x5555_AAAA, 0));
}
