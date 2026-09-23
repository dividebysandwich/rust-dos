//! 386/486 instructions and prefixes in real-mode (16-bit) code.

use rust_dos::cpu::{Cpu, CpuFlags, CpuModel};
mod testrunners;
use testrunners::run_cpu_code;

fn cpu() -> Cpu {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    cpu.set_ss(0);
    cpu.set_sp(0x8000);
    cpu
}

#[test]
fn operand_size_prefix_gives_32_bit_registers_and_immediates() {
    let mut cpu = cpu();
    // 66 B8 78 56 34 12 -> MOV EAX, 12345678h
    // 66 05 88 A9 CB ED -> ADD EAX, EDCBA988h
    run_cpu_code(&mut cpu, &[0x66, 0xB8, 0x78, 0x56, 0x34, 0x12, 0x66, 0x05, 0x88, 0xA9, 0xCB, 0xED]);
    assert_eq!(cpu.eax(), 0x0000_0000);
    assert!(cpu.get_cpu_flag(CpuFlags::CF) && cpu.get_cpu_flag(CpuFlags::ZF));

    // 66 83 C0 FF -> ADD EAX, -1 (sign-extended imm8)
    run_cpu_code(&mut cpu, &[0x66, 0x83, 0xC0, 0xFF]);
    assert_eq!(cpu.eax(), 0xFFFF_FFFF);
}

#[test]
fn sixteen_bit_writes_keep_the_upper_half() {
    let mut cpu = cpu();
    cpu.set_eax(0xAAAA_0000);
    // B8 34 12 -> MOV AX, 1234h
    run_cpu_code(&mut cpu, &[0xB8, 0x34, 0x12]);
    assert_eq!(cpu.eax(), 0xAAAA_1234);
}

#[test]
fn address_size_prefix_gives_sib_addressing() {
    let mut cpu = cpu();
    cpu.set_ds(0x1000);
    cpu.set_eax(0x0000_0100);
    cpu.set_ecx(0x0000_0004);
    cpu.bus.write_16(0x10000 + 0x100 + 4 * 4 + 0x10, 0xBEEF);
    // 67 8B 5C 88 10 -> MOV BX, [EAX+ECX*4+10h]
    run_cpu_code(&mut cpu, &[0x67, 0x8B, 0x5C, 0x88, 0x10]);
    assert_eq!(cpu.bx(), 0xBEEF);
}

#[test]
fn word_access_across_the_segment_limit_faults() {
    let mut cpu = cpu();
    cpu.set_ds(0x2000);
    cpu.set_bx(0xFFFF);
    // #GP handler at 0000:0600 (vector 0Dh).
    cpu.bus.write_16(0x0D * 4, 0x0600);
    cpu.bus.write_16(0x0D * 4 + 2, 0x0000);
    // 8B 07 -> MOV AX, [BX] with BX=FFFFh: the word crosses the 64K limit.
    run_cpu_code(&mut cpu, &[0x8B, 0x07]);
    assert_eq!((cpu.cs(), cpu.ip()), (0x0000, 0x0600));
    // The return address is the faulting instruction.
    assert_eq!(cpu.bus.read_16(cpu.sp() as usize), 0x0100);
}

#[test]
fn movzx_movsx() {
    let mut cpu = cpu();
    cpu.set_reg8(iced_x86::Register::BL, 0x80);
    // 66 0F B6 C3 -> MOVZX EAX, BL ; 66 0F BE CB -> MOVSX ECX, BL
    run_cpu_code(&mut cpu, &[0x66, 0x0F, 0xB6, 0xC3, 0x66, 0x0F, 0xBE, 0xCB]);
    assert_eq!(cpu.eax(), 0x80);
    assert_eq!(cpu.ecx(), 0xFFFF_FF80);
}

#[test]
fn bit_test_with_register_offset_reaches_past_the_operand() {
    let mut cpu = cpu();
    cpu.set_ds(0x1000);
    cpu.set_bx(0x0010);
    cpu.set_ax(35); // bit 3 of the word at [BX+4]
    cpu.bus.write_16(0x10014, 0x0008);
    // 0F A3 07 -> BT [BX], AX
    run_cpu_code(&mut cpu, &[0x0F, 0xA3, 0x07]);
    assert!(cpu.get_cpu_flag(CpuFlags::CF));
    // 0F AB 07 -> BTS [BX], AX with AX=-1: bit 15 of the word at [BX-2]
    cpu.set_ax(0xFFFF);
    run_cpu_code(&mut cpu, &[0x0F, 0xAB, 0x07]);
    assert_eq!(cpu.bus.read_16(0x1000E), 0x8000);
}

#[test]
fn shld_and_shrd() {
    let mut cpu = cpu();
    cpu.set_eax(0x1234_5678);
    cpu.set_edx(0x9ABC_DEF0);
    // 66 0F A4 D0 08 -> SHLD EAX, EDX, 8
    run_cpu_code(&mut cpu, &[0x66, 0x0F, 0xA4, 0xD0, 0x08]);
    assert_eq!(cpu.eax(), 0x3456_789A);
    // 66 0F AC D0 04 -> SHRD EAX, EDX, 4
    run_cpu_code(&mut cpu, &[0x66, 0x0F, 0xAC, 0xD0, 0x04]);
    assert_eq!(cpu.eax(), 0x0345_6789);
}

#[test]
fn pushad_popad_round_trip() {
    let mut cpu = cpu();
    let values = [0x1111_1111, 0x2222_2222, 0x3333_3333, 0x4444_4444];
    cpu.set_eax(values[0]);
    cpu.set_ecx(values[1]);
    cpu.set_edx(values[2]);
    cpu.set_ebx(values[3]);
    // 66 60 -> PUSHAD ; 66 31 C0 -> XOR EAX, EAX ; 66 61 -> POPAD
    run_cpu_code(&mut cpu, &[0x66, 0x60, 0x66, 0x31, 0xC0, 0x66, 0x61]);
    assert_eq!([cpu.eax(), cpu.ecx(), cpu.edx(), cpu.ebx()], values);
    assert_eq!(cpu.sp(), 0x8000);
}

#[test]
fn rep_movsd_with_32_bit_addressing() {
    let mut cpu = cpu();
    cpu.set_ds(0x1000);
    cpu.set_es(0x2000);
    for i in 0..8 {
        cpu.bus.write_32(0x10000 + i * 4, 0x0101_0101 * i as u32);
    }
    cpu.set_esi(0);
    cpu.set_edi(0x10);
    cpu.set_ecx(8);
    // F3 66 67 A5 -> REP MOVSD with ESI/EDI/ECX
    run_cpu_code(&mut cpu, &[0xF3, 0x66, 0x67, 0xA5]);
    assert_eq!(cpu.ecx(), 0);
    assert_eq!(cpu.edi(), 0x30);
    assert_eq!(cpu.bus.read_32(0x20010 + 7 * 4), 0x0707_0707);
}

#[test]
fn imul_three_operand_and_wide_divide() {
    let mut cpu = cpu();
    cpu.set_ebx(100_000);
    // 66 69 C3 A0 86 01 00 -> IMUL EAX, EBX, 100000
    run_cpu_code(&mut cpu, &[0x66, 0x69, 0xC3, 0xA0, 0x86, 0x01, 0x00]);
    assert_eq!(cpu.eax(), 1_410_065_408); // 10^10 mod 2^32
    assert!(cpu.get_cpu_flag(CpuFlags::OF));

    cpu.set_edx(2);
    cpu.set_eax(0);
    cpu.set_ecx(3);
    // 66 F7 F1 -> DIV ECX: 2^33 / 3
    run_cpu_code(&mut cpu, &[0x66, 0xF7, 0xF1]);
    assert_eq!((cpu.eax(), cpu.edx()), (2_863_311_530, 2));
}

#[test]
fn flags_look_like_a_386_to_detection_code() {
    let mut cpu = cpu();
    // Classic check: set bits 12-15 with POPF and read them back.
    // B8 00 F0 -> MOV AX, F000h ; 50 -> PUSH AX ; 9D -> POPF ;
    // 9C -> PUSHF ; 58 -> POP AX
    run_cpu_code(&mut cpu, &[0xB8, 0x00, 0xF0, 0x50, 0x9D, 0x9C, 0x58]);
    // An 8086 sets bits 12-15, a 286 clears them; a 386 keeps IOPL and NT
    // (12-14) and clears bit 15.
    assert_eq!(cpu.ax() & 0xF000, 0x7000);
    assert_eq!(cpu.ax() & 0x0002, 0x0002);
}

#[test]
fn eflags_ac_bit_tells_a_486_from_a_386() {
    // 66 9C -> PUSHFD ; 66 58 -> POP EAX ; 66 35 00 00 04 00 -> XOR EAX, 40000h ;
    // 66 50 -> PUSH EAX ; 66 9D -> POPFD ; 66 9C -> PUSHFD ; 66 5B -> POP EBX
    let code = [
        0x66, 0x9C, 0x66, 0x58, 0x66, 0x35, 0x00, 0x00, 0x04, 0x00, 0x66, 0x50, 0x66, 0x9D, 0x66, 0x9C,
        0x66, 0x5B,
    ];
    let mut cpu = cpu();
    run_cpu_code(&mut cpu, &code);
    assert_ne!(cpu.ebx() & 0x40000, 0, "486: AC can be set");

    let mut cpu = self::cpu();
    cpu.model = CpuModel::I386;
    run_cpu_code(&mut cpu, &code);
    assert_eq!(cpu.ebx() & 0x40000, 0, "386: AC stays clear");
}

#[test]
fn bswap_xadd_cmpxchg() {
    let mut cpu = cpu();
    cpu.set_eax(0x1122_3344);
    // 0F C8 -> BSWAP EAX (the register form needs no 66h)
    run_cpu_code(&mut cpu, &[0x66, 0x0F, 0xC8]);
    assert_eq!(cpu.eax(), 0x4433_2211);

    cpu.set_ax(5);
    cpu.set_bx(7);
    // 0F C1 C3 -> XADD BX, AX
    run_cpu_code(&mut cpu, &[0x0F, 0xC1, 0xC3]);
    assert_eq!((cpu.bx(), cpu.ax()), (12, 7));

    cpu.set_ax(12);
    cpu.set_cx(99);
    // 0F B1 CB -> CMPXCHG BX, CX: AX == BX, so BX = CX
    run_cpu_code(&mut cpu, &[0x0F, 0xB1, 0xCB]);
    assert_eq!(cpu.bx(), 99);
    assert!(cpu.get_cpu_flag(CpuFlags::ZF));
}

#[test]
fn setcc_and_jecxz() {
    let mut cpu = cpu();
    cpu.set_ax(1);
    // 3D 02 00 -> CMP AX, 2 ; 0F 9C C1 -> SETL CL
    run_cpu_code(&mut cpu, &[0x3D, 0x02, 0x00, 0x0F, 0x9C, 0xC1]);
    assert_eq!(cpu.get_reg8(iced_x86::Register::CL), 1);

    // 66 31 C9 -> XOR ECX, ECX ; 67 E3 02 -> JECXZ +2 ; 90 90 ; 90
    cpu.set_ip(0x200);
    run_cpu_code(&mut cpu, &[0x66, 0x31, 0xC9, 0x67, 0xE3, 0x02, 0x90, 0x90, 0xF4]);
    assert_eq!(cpu.ip(), 0x209, "JECXZ skipped the NOPs and reached the HLT");
}

#[test]
fn lfs_and_segment_overrides() {
    let mut cpu = cpu();
    cpu.set_ds(0x1000);
    cpu.bus.write_16(0x10000, 0x0020);
    cpu.bus.write_16(0x10002, 0x3000);
    cpu.bus.write_16(0x30020, 0x5A5A);
    // 0F B4 1E 00 00 -> LFS BX, [0000] ; 64 8B 07 -> MOV AX, FS:[BX]
    run_cpu_code(&mut cpu, &[0x0F, 0xB4, 0x1E, 0x00, 0x00, 0x64, 0x8B, 0x07]);
    assert_eq!(cpu.fs(), 0x3000);
    assert_eq!(cpu.ax(), 0x5A5A);
}

#[test]
fn invalid_opcode_goes_to_int_06() {
    let mut cpu = cpu();
    cpu.bus.write_16(0x06 * 4, 0x0700);
    cpu.bus.write_16(0x06 * 4 + 2, 0x0000);
    // 0F 0B -> UD2
    run_cpu_code(&mut cpu, &[0x0F, 0x0B]);
    assert_eq!((cpu.cs(), cpu.ip()), (0x0000, 0x0700));
    assert_eq!(cpu.bus.read_16(cpu.sp() as usize), 0x0100);
}

#[test]
fn enter_with_nesting_level() {
    let mut cpu = cpu();
    cpu.set_bp(0x9000);
    cpu.bus.write_16(0x9000 - 2, 0xAAAA);
    // C8 04 00 02 -> ENTER 4, 2
    run_cpu_code(&mut cpu, &[0xC8, 0x04, 0x00, 0x02]);
    // Old BP, the copied frame pointer, and the new frame pointer.
    assert_eq!(cpu.bp(), 0x7FFE);
    assert_eq!(cpu.bus.read_16(0x7FFE), 0x9000);
    assert_eq!(cpu.bus.read_16(0x7FFC), 0xAAAA);
    assert_eq!(cpu.bus.read_16(0x7FFA), 0x7FFE);
    assert_eq!(cpu.sp(), 0x7FFA - 4);
}

#[test]
fn sib_byte_with_base_only() {
    // 67 F7 2C A2 -> IMUL WORD [EDX] (a SIB byte with no index), from the
    // SingleStepTests suite.
    let mut cpu = cpu();
    cpu.set_eax(0x9338_0B59);
    cpu.set_edx(0x2A2);
    cpu.set_ds(0x3EF7);
    cpu.bus.write_16(0x3EF70 + 0x2A2, 0x05A1);
    run_cpu_code(&mut cpu, &[0x67, 0xF7, 0x2C, 0xA2]);
    assert_eq!(cpu.eax(), 0x9338_DFF9);
    assert_eq!(cpu.dx(), 0x003F);
}
