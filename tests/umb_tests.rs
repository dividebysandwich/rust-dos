//! Upper memory blocks as DOS 5 has them: the chain through the cover MCB
//! at 9FFFh, linking it to conventional memory, the upper memory allocation
//! strategies, and LOADHIGH with a TSR staying there.

use iced_x86::Register;
use rust_dos::dos_data::{SYSVARS, address};
use rust_dos::command::CommandDispatcher;
use rust_dos::cpu::{Cpu, CpuFlags, CpuState};
use rust_dos::interrupts::int21;
use rust_dos::mcb::{self, FIRST_MCB_SEG, MCB_M, MCB_Z, UMB_COVER_SEG, UMB_START, walk, walk_upper};
use rust_dos::xms;
use std::fs;
use std::path::PathBuf;

fn scratch(name: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let base = PathBuf::from("target/test_umb").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    for (file, bytes) in files {
        fs::write(base.join(file), bytes).unwrap();
    }
    base
}

/// A machine at its prompt with upper memory, and EMS if `ems`.
fn machine(name: &str, files: &[(&str, &[u8])], ems: bool) -> Cpu {
    let mut cpu = Cpu::new(scratch(name, files));
    cpu.set_upper_memory(ems, true).unwrap();
    cpu.load_shell();
    cpu
}

fn int21(cpu: &mut Cpu, ax: u16) -> bool {
    cpu.set_ax(ax);
    int21::handle(cpu);
    !cpu.get_cpu_flag(CpuFlags::CF)
}

fn ivt(cpu: &Cpu, vector: usize) -> (u16, u16) {
    (cpu.bus.read_16(vector * 4 + 2), cpu.bus.read_16(vector * 4))
}

#[test]
fn upper_memory_is_chained_at_9fff() {
    let cpu = machine("chain", &[], false);
    // Conventional memory ends a paragraph short, at the cover MCB.
    let low = walk(&cpu.bus);
    let &(last, m) = low.last().unwrap();
    assert!(m.is_free() && m.signature == MCB_Z);
    assert_eq!(last + 1 + m.size, UMB_COVER_SEG);
    let cover = mcb::read_mcb(&cpu.bus, UMB_COVER_SEG);
    assert_eq!((cover.signature, cover.owner, cover.size), (MCB_M, 8, UMB_START - UMB_COVER_SEG - 1));
    assert_eq!(cpu.bus.read_8(UMB_COVER_SEG as usize * 16 + 8), b'S');
    // D000h to EFFFh, one free block.
    let upper = walk_upper(&cpu.bus);
    assert_eq!(upper.len(), 1);
    assert_eq!((upper[0].0, upper[0].1.size, upper[0].1.is_free()), (UMB_START, 0x1FFF, true));
    // The List of Lists has them, unlinked.
    assert_eq!(cpu.bus.read_16(address(SYSVARS) + 0x66), UMB_COVER_SEG);
    assert_eq!(cpu.bus.read_8(address(SYSVARS) + 0x63), 0);
    // Programs see a byte less of conventional memory, and XMS has no UMBs.
    let mut cpu = cpu;
    cpu.set_reg8(Register::AH, 0x10);
    cpu.set_dx(0x100);
    xms::call(&mut cpu);
    assert_eq!((cpu.ax(), cpu.get_reg8(Register::BL)), (0, 0xB1));
}

#[test]
fn ems_shrinks_upper_memory_to_d000_dfff() {
    let cpu = machine("with_ems", &[], true);
    let upper = walk_upper(&cpu.bus);
    assert_eq!((upper[0].0, upper[0].1.size), (UMB_START, 0x0FFF));
    // Turned off again at the prompt, conventional memory is whole.
    let mut cpu = cpu;
    cpu.set_upper_memory(true, false).unwrap();
    let &(last, m) = walk(&cpu.bus).last().unwrap();
    assert_eq!(last + 1 + m.size, 0xA000);
    assert_eq!(cpu.bus.read_16(address(SYSVARS) + 0x66), 0xFFFF);
}

#[test]
fn link_state_and_strategies() {
    let mut cpu = machine("link", &[], false);
    cpu.current_psp = 0x1234;
    assert!(int21(&mut cpu, 0x5802));
    assert_eq!(cpu.get_al(), 0);

    // Upper memory first (80h): the block is in upper memory, linked or not.
    cpu.set_bx(0x80);
    assert!(int21(&mut cpu, 0x5801));
    cpu.set_bx(0x100);
    assert!(int21(&mut cpu, 0x4800));
    let high = cpu.ax();
    assert_eq!(high, UMB_START + 1);
    assert_eq!(mcb::read_mcb(&cpu.bus, high - 1).owner, 0x1234);
    // More than upper memory has: from conventional memory instead, but
    // not with upper memory only (40h), which says how much it has.
    cpu.set_bx(0x3000);
    assert!(int21(&mut cpu, 0x4800));
    assert!(cpu.ax() < UMB_COVER_SEG);
    cpu.set_bx(0x40);
    assert!(int21(&mut cpu, 0x5801));
    cpu.set_bx(0x3000);
    assert!(!int21(&mut cpu, 0x4800));
    assert_eq!(cpu.bx(), 0x1FFF - 0x101);
    // Bad strategies are refused.
    cpu.set_bx(0x03);
    assert!(!int21(&mut cpu, 0x5801));
    cpu.set_bx(0xC0);
    assert!(!int21(&mut cpu, 0x5801));

    // Linked, one chain runs through the cover MCB into upper memory.
    cpu.set_bx(1);
    assert!(int21(&mut cpu, 0x5803));
    assert!(int21(&mut cpu, 0x5802));
    assert_eq!(cpu.get_al(), 1);
    assert_eq!(cpu.bus.read_8(address(SYSVARS) + 0x63), 1);
    let chain = walk(&cpu.bus);
    assert!(chain.iter().any(|&(s, _)| s == UMB_COVER_SEG));
    assert!(chain.iter().any(|&(s, _)| s == UMB_START));
    // Freeing an upper block merges it there, linked or not.
    cpu.set_es(high);
    assert!(int21(&mut cpu, 0x4900));
    cpu.set_bx(0);
    assert!(int21(&mut cpu, 0x5803));
    let upper = walk_upper(&cpu.bus);
    assert_eq!((upper.len(), upper[0].1.size, upper[0].1.is_free()), (1, 0x1FFF, true));
    assert_eq!(walk(&cpu.bus).last().unwrap().1.signature, MCB_Z);

    // The shell unlinks it, and frees what programs left there.
    cpu.set_bx(0x80);
    assert!(int21(&mut cpu, 0x5801));
    cpu.set_bx(0x10);
    assert!(int21(&mut cpu, 0x4800));
    cpu.set_bx(1);
    assert!(int21(&mut cpu, 0x5803));
    cpu.load_shell();
    assert!(int21(&mut cpu, 0x5802));
    assert_eq!(cpu.get_al(), 0);
    assert!(walk_upper(&cpu.bus)[0].1.is_free());
    assert_eq!(cpu.alloc_strategy, 0);

    // Without upper memory there is nothing to link.
    let mut cpu = Cpu::new(scratch("no_umb", &[]));
    cpu.set_bx(1);
    assert!(!int21(&mut cpu, 0x5803));
}

#[test]
fn programs_started_from_the_shell_stay_below_upper_memory() {
    let mut cpu = machine("low", &[("APP.EXE", &mz_exe())], false);
    assert!(cpu.load_executable("APP.EXE", None));
    let psp = cpu.current_psp;
    assert!(psp < UMB_COVER_SEG);
    // Its block ends at the cover MCB, which it can't overwrite.
    assert_eq!(cpu.bus.read_16(psp as usize * 16 + 2), UMB_COVER_SEG);
    let cover = mcb::read_mcb(&cpu.bus, UMB_COVER_SEG);
    assert_eq!((cover.signature, cover.owner), (MCB_M, 8));
    assert_eq!(walk_upper(&cpu.bus).len(), 1);
}

/// A small EXE that loops.
fn mz_exe() -> Vec<u8> {
    let mut exe = vec![0u8; 0x20];
    exe[0..2].copy_from_slice(b"MZ");
    exe[2..4].copy_from_slice(&0x22u16.to_le_bytes()); // bytes in the last page
    exe[4..6].copy_from_slice(&1u16.to_le_bytes()); // pages
    exe[8..10].copy_from_slice(&2u16.to_le_bytes()); // header paragraphs
    exe[10..12].copy_from_slice(&0x10u16.to_le_bytes()); // minimum allocation
    exe[12..14].copy_from_slice(&0xFFFFu16.to_le_bytes()); // maximum
    exe[16..18].copy_from_slice(&0x100u16.to_le_bytes()); // SP
    exe.extend_from_slice(&[0xEB, 0xFE]);
    exe
}

#[test]
fn loadhigh_keeps_a_tsr_and_its_hooks_in_upper_memory() {
    // The TSR and the program after it just loop; the test makes their
    // DOS calls.
    let mut cpu = machine("loadhigh", &[("TSR.COM", &[0xEB, 0xFE]), ("APP.COM", &[0xEB, 0xFE])], false);
    assert!(CommandDispatcher::new().dispatch(&mut cpu, "LH", "/L:1 TSR"));
    let tsr = cpu.current_psp;
    assert_eq!(tsr, UMB_START + 1, "loaded high");
    assert_eq!(cpu.cs(), tsr);
    assert_eq!(mcb::read_mcb(&cpu.bus, tsr - 1).owner, tsr);
    assert_eq!(cpu.bus.read_16(tsr as usize * 16 + 2), 0xF000, "its memory ends with upper memory");
    assert!(cpu.sp() > 0xFF00, "a whole segment's stack");
    // It hooks INT 60h and stays resident with 20h paragraphs.
    cpu.bus.write_16(0x60 * 4, 0x0180);
    cpu.bus.write_16(0x60 * 4 + 2, tsr);
    cpu.set_dx(0x20);
    assert!(int21(&mut cpu, 0x3100));
    assert_eq!(cpu.state, CpuState::RebootShell);
    assert_eq!(cpu.resident_end, FIRST_MCB_SEG, "conventional memory stays free");
    cpu.load_shell();
    let upper = walk_upper(&cpu.bus);
    assert_eq!((upper[0].1.owner, upper[0].1.size), (tsr, 0x20));
    assert!(upper[1].1.is_free() && upper[1].1.is_last());

    // The next program loads low and leaves the TSR's hook in place.
    assert!(cpu.load_executable("APP.COM", None));
    assert_eq!(cpu.current_psp, FIRST_MCB_SEG + 1);
    assert_eq!(ivt(&cpu, 0x60), (tsr, 0x0180));
    int21(&mut cpu, 0x4C00);
    cpu.load_shell();
    assert_eq!(ivt(&cpu, 0x60), (tsr, 0x0180));
    assert_eq!(walk_upper(&cpu.bus)[0].1.owner, tsr);

    // With upper memory full, LOADHIGH loads low.
    let mut cpu = machine("loadhigh_full", &[("TSR.COM", &[0xEB, 0xFE])], false);
    cpu.set_bx(0x80);
    assert!(int21(&mut cpu, 0x5801));
    cpu.set_bx(0x1FFF);
    assert!(int21(&mut cpu, 0x4800));
    assert!(CommandDispatcher::new().dispatch(&mut cpu, "LOADHIGH", "TSR.COM"));
    assert!(cpu.current_psp < UMB_COVER_SEG);
}
