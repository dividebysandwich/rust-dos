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
fn test_regression_acquire_panic() {
    let mut cpu = Cpu::new(PathBuf::from("target/test_regression"));

    // 1. Verify EBP access in set_reg16/get_reg16 does not panic
    cpu.set_reg16(Register::EBP, 0x1234);
    assert_eq!(cpu.get_reg16(Register::EBP), 0x1234);
    assert_eq!(cpu.bp(), 0x1234); // Should affect BP

    // 2. Verify OUTSB (String Output Byte)
    // OUTS DX, DS:SI
    // Port: DX=0x0300
    // Data: DS:SI points to [0xAA, 0xBB]
    cpu.set_dx(0x0300);
    cpu.set_ds(0x2000);
    cpu.set_si(0x0000);

    let addr = cpu.get_physical_addr(cpu.ds(), cpu.si());
    cpu.bus.write_8(addr, 0xAA);
    cpu.bus.write_8(addr + 1, 0xBB);

    // Clear Direction Flag (Increment)
    cpu.set_cpu_flag(CpuFlags::DF, false);

    // Mock Instruction for OUTSB
    // We can't easily construct a raw Instruction object without decoding bytes.
    // So we'll run a mini-program.

    // Code: 6E (OUTSB)
    let code_addr = cpu.get_physical_addr(cpu.cs(), cpu.ip());
    cpu.bus.write_8(code_addr, 0x6E);

    // Step
    cpu.step();

    // Verify IO Write
    // Note: Bus doesn't store IO state by default unless mapped to a device.
    // However, our string.rs uses cpu.bus.io_write.
    // If no device is attached to 0x300, it just logs or ignores.
    // But we want to ensure it didn't panic and SI advanced.
    assert_eq!(cpu.si(), 1);

    // OUTSW
    // Code: 6F
    let code_addr = cpu.get_physical_addr(cpu.cs(), cpu.ip());
    cpu.bus.write_8(code_addr, 0x6F);

    cpu.step(); // Should write 0xBB...? Wait, SI is 1. Address is 2000:0001 -> 0xBB.
    // OUTSW reads Word at 2000:0001 -> Low=0xBB, High=Unknown(0).
    // And writes to DX.
    // SI should advance by 2.
    assert_eq!(cpu.si(), 3);
}

/// `dos_version` is what AH=30h, AX=3306h and a new PSP tell programs.
#[test]
fn the_reported_dos_version_is_the_setting() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    let version = |cpu: &mut Cpu| {
        cpu.set_reg16(Register::AX, 0x3000);
        rust_dos::interrupts::int21::handle(cpu);
        let ah30 = cpu.get_reg16(Register::AX);
        cpu.set_reg16(Register::AX, 0x3306);
        rust_dos::interrupts::int21::handle(cpu);
        (ah30, cpu.get_reg16(Register::BX))
    };
    assert_eq!(version(&mut cpu), (0x0005, 0x0005));
    rust_dos::dos_data::set_version(&mut cpu.bus, rust_dos::config::DosVersion::new(7, 10));
    assert_eq!(version(&mut cpu), (0x0A07, 0x0A07));
    // A PSP made now carries it too (AH=26h).
    cpu.set_reg8(Register::AH, 0x26);
    cpu.set_reg16(Register::DX, 0x5000);
    rust_dos::interrupts::int21::handle(&mut cpu);
    assert_eq!(cpu.bus.read_16(0x50000 + 0x40), 0x0A07);
}
