use iced_x86::Register;
use rust_dos::cpu::{Cpu, CpuFlags};
use std::fs;
use std::path::PathBuf;

#[test]
fn test_int21_ah0e_drive_selection() {
    let root_path = PathBuf::from("target/test_int21_ah0e");
    if root_path.exists() {
        fs::remove_dir_all(&root_path).unwrap();
    }
    fs::create_dir_all(&root_path).unwrap();

    let mut cpu = Cpu::new(root_path.clone());

    // Initially should be C: (Drive 2)
    assert_eq!(cpu.bus.disk.get_current_drive(), 2);

    // Call AH=19h (Get Default Drive)
    cpu.set_reg8(Register::AH, 0x19);
    rust_dos::interrupts::int21::handle(&mut cpu);
    assert_eq!(cpu.get_reg8(Register::AL), 2);

    // Call AH=0Eh (Select Default Drive) -> Select Z: (Drive 25)
    cpu.set_reg8(Register::AH, 0x0E);
    cpu.set_reg8(Register::DL, 25);
    rust_dos::interrupts::int21::handle(&mut cpu);

    // Verify AL (Logical Drives)
    // Implementation returns 26.
    assert_eq!(cpu.get_reg8(Register::AL), 26);

    // Verify Current Drive is Z:
    assert_eq!(cpu.bus.disk.get_current_drive(), 25);

    // Call AH=19h again
    cpu.set_reg8(Register::AH, 0x19);
    rust_dos::interrupts::int21::handle(&mut cpu);
    assert_eq!(cpu.get_reg8(Register::AL), 25);

    fs::remove_dir_all(&root_path).unwrap();
}

#[test]
fn test_int21_ah4b_command_com_interception() {
    let root_path = PathBuf::from("target/test_int21_ah4b_intercept");
    if root_path.exists() {
        fs::remove_dir_all(&root_path).unwrap();
    }
    fs::create_dir_all(&root_path).unwrap();

    let mut cpu = Cpu::new(root_path.clone());

    // Create target program "TARGET.COM"
    // Content: 90 (NOP), CD 20 (INT 20 Exit)
    let target_path = root_path.join("TARGET.COM");
    fs::write(&target_path, vec![0x90, 0xCD, 0x20]).unwrap();

    // Setup DS:DX -> "COMMAND.COM"
    cpu.ds = 0x2000;
    cpu.dx = 0x0000;
    let filename = "COMMAND.COM";
    let mut phys_dx = cpu.get_physical_addr(cpu.ds, cpu.dx);
    for b in filename.bytes() {
        cpu.bus.write_8(phys_dx, b);
        phys_dx += 1;
    }
    cpu.bus.write_8(phys_dx, 0); // Null terminator

    // Setup Command Tail at 0x4000:0000 -> " /C TARGET.COM"
    // Note: Leading space is critical for robust parsing logic in real DOS,
    // and our splitn logic likely trims it but " /C" needs space separator?
    // Our logic: `trimmed.to_ascii_uppercase().starts_with("/C")`.
    // " /C ..." trimmed is "/C ...".
    let cmd_tail = " /C TARGET.COM";
    let tail_seg = 0x4000;
    let tail_off = 0x0000;
    let tail_phys = cpu.get_physical_addr(tail_seg, tail_off);
    cpu.bus.write_8(tail_phys, cmd_tail.len() as u8); // Length
    for (i, b) in cmd_tail.bytes().enumerate() {
        cpu.bus.write_8(tail_phys + 1 + i, b);
    }
    cpu.bus.write_8(tail_phys + 1 + cmd_tail.len(), 0x0D); // CR

    // Setup Parameter Block at ES:BX
    cpu.es = 0x3000;
    cpu.bx = 0x0000;
    let param_phys = cpu.get_physical_addr(cpu.es, cpu.bx);

    // Offset 2: Cmd Line Pointer -> 0x4000:0000
    cpu.bus.write_16(param_phys + 2, tail_off);
    cpu.bus.write_16(param_phys + 4, tail_seg);

    // Call AH=4B
    cpu.set_reg8(Register::AH, 0x4B);
    cpu.set_reg8(Register::AL, 0x00);

    rust_dos::interrupts::int21::handle(&mut cpu);

    // Verify that TARGET.COM was loaded
    // TARGET.COM is loaded at CS:0100 (Standard COM)
    // We expect CS to be updated (new segment) and IP=0x100.
    // And content at CS:0100 should be 0x90.

    // Also verify no "File Not Found" error (CF clear)
    assert!(
        !cpu.get_cpu_flag(CpuFlags::CF),
        "CF should be clear (EXEC success)"
    );

    assert_eq!(cpu.ip, 0x100, "IP should be 0x100");
    let code_phys = cpu.get_physical_addr(cpu.cs, cpu.ip);
    assert_eq!(
        cpu.bus.read_8(code_phys),
        0x90,
        "Code at entry point should be NOP (from TARGET.COM)"
    );

    // Verify PSP tail contains args for TARGET.COM?
    // The command tail for TARGET.COM should be extracted from "/C TARGET.COM".
    // Logic: `after_c` = "TARGET.COM".
    // `prog` = "TARGET.COM". `args` = "".
    // Tail should be empty? Or " "?
    // My implementation: `new_tail` logic.
    // If args is empty, `new_tail` might be empty.

    // Let's verify PSP tail.
    let psp_phys = cpu.get_physical_addr(cpu.ds, 0x80); // DS points to PSP after load
    let len = cpu.bus.read_8(psp_phys);
    // If args empty, logic:
    // `if !args.is_empty() { ... }`
    // So new_tail is empty vec.
    // write_8 len = 0.
    assert_eq!(len, 0, "TARGET.COM should receive empty args");

    fs::remove_dir_all(&root_path).unwrap();
}

#[test]
fn test_regression_acquire_panic() {
    let mut cpu = Cpu::new(PathBuf::from("target/test_regression"));

    // 1. Verify EBP access in set_reg16/get_reg16 does not panic
    cpu.set_reg16(Register::EBP, 0x1234);
    assert_eq!(cpu.get_reg16(Register::EBP), 0x1234);
    assert_eq!(cpu.bp, 0x1234); // Should affect BP

    // 2. Verify OUTSB (String Output Byte)
    // OUTS DX, DS:SI
    // Port: DX=0x0300
    // Data: DS:SI points to [0xAA, 0xBB]
    cpu.dx = 0x0300;
    cpu.ds = 0x2000;
    cpu.si = 0x0000;

    let addr = cpu.get_physical_addr(cpu.ds, cpu.si);
    cpu.bus.write_8(addr, 0xAA);
    cpu.bus.write_8(addr + 1, 0xBB);

    // Clear Direction Flag (Increment)
    cpu.set_cpu_flag(CpuFlags::DF, false);

    // Mock Instruction for OUTSB
    // We can't easily construct a raw Instruction object without decoding bytes.
    // So we'll run a mini-program.

    // Code: 6E (OUTSB)
    let code_addr = cpu.get_physical_addr(cpu.cs, cpu.ip);
    cpu.bus.write_8(code_addr, 0x6E);

    // Step
    cpu.step();

    // Verify IO Write
    // Note: Bus doesn't store IO state by default unless mapped to a device.
    // However, our string.rs uses cpu.bus.io_write.
    // If no device is attached to 0x300, it just logs or ignores.
    // But we want to ensure it didn't panic and SI advanced.
    assert_eq!(cpu.si, 1);

    // OUTSW
    // Code: 6F
    let code_addr = cpu.get_physical_addr(cpu.cs, cpu.ip);
    cpu.bus.write_8(code_addr, 0x6F);

    cpu.step(); // Should write 0xBB...? Wait, SI is 1. Address is 2000:0001 -> 0xBB.
    // OUTSW reads Word at 2000:0001 -> Low=0xBB, High=Unknown(0).
    // And writes to DX.
    // SI should advance by 2.
    assert_eq!(cpu.si, 3);
}
