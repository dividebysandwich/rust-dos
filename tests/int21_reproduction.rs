use iced_x86::Register;
use rust_dos::cpu::{Cpu, CpuState};
use rust_dos::interrupts::int21;
use std::path::PathBuf;

#[test]
fn test_d_com_sequence() {
    let mut cpu = Cpu::new(PathBuf::from("."));

    // 1. Set DTA to 2000:0000
    // DS = 2000, DX = 0000
    cpu.ds = 0x2000;
    cpu.set_reg16(Register::DX, 0x0000);
    cpu.set_reg8(Register::AH, 0x1A);
    int21::handle(&mut cpu);

    // Verify DTA matched
    assert_eq!(cpu.bus.dta_segment, 0x2000);
    assert_eq!(cpu.bus.dta_offset, 0x0000);

    // 2. FindFirst ("*.*")
    // Writes "*.?*" (example) to DS:DX?
    // AH=4E, CX=Attr, DS:DX=ASCIZ Spec

    // Write "*.*" to 3000:0000
    let spec_seg = 0x3000;
    let spec_off = 0x0000;
    let spec_phys = (spec_seg * 16 + spec_off) as usize;
    let pattern = b"*.*\0";
    for (i, b) in pattern.iter().enumerate() {
        cpu.bus.write_8(spec_phys + i, *b);
    }

    cpu.ds = spec_seg as u16;
    cpu.set_reg16(Register::DX, spec_off as u16);
    cpu.set_reg16(Register::CX, 0x10); // Directories + Files
    cpu.set_reg8(Register::AH, 0x4E);

    println!("DEBUG: Invoking FindFirst (*.*)");
    int21::handle(&mut cpu);

    // Check Result (AX=0 is success)
    let ax = cpu.get_reg16(Register::AX);
    if ax != 0 {
        panic!("FindFirst failed with Error Code: 0x{:04X}", ax);
    }

    // Check DTA at 2000:0000 for Filename (Offset 30 / 0x1E)
    let dta_phys = 0x20000;
    println!("DEBUG: Reading DTA at {:05X}", dta_phys);

    let mut filename = String::new();
    for i in 0..13 {
        let b = cpu.bus.read_8(dta_phys + 0x1E + i);
        if b == 0 {
            break;
        }
        filename.push(b as char);
    }
    println!("Found File: '{}'", filename);
    assert!(!filename.is_empty(), "Filename in DTA should not be empty");

    // 3. FindNext
    // AH=4F

    // Loop max 10 times
    for _ in 0..10 {
        cpu.set_reg8(Register::AH, 0x4F); // Reset AH
        int21::handle(&mut cpu);
        let ax = cpu.get_reg16(Register::AX);
        if ax != 0 {
            println!("FindNext ended with 0x{:04X}", ax);
            break;
        }

        let mut f_name = String::new();
        for i in 0..13 {
            let b = cpu.bus.read_8(dta_phys + 0x1E + i);
            if b == 0 {
                break;
            }
            f_name.push(b as char);
        }
        println!("Found File: '{}'", f_name);
    }
}

#[test]
fn test_fcb_sequence() {
    let mut cpu = Cpu::new(PathBuf::from("."));

    // FCB FindFirst = AH=11h
    // DS:DX Points to FCB.
    // DTA is NOT used for input pattern, but IS used for result.

    // Set DTA to 4000:0000
    cpu.ds = 0x4000;
    cpu.set_reg16(Register::DX, 0x0000);
    cpu.set_reg8(Register::AH, 0x1A);
    int21::handle(&mut cpu);

    // Setup Input FCB at 5000:0000
    let fcb_seg = 0x5000;
    let fcb_off = 0x0000;
    let fcb_phys = (fcb_seg * 16 + fcb_off) as usize;

    // Pattern: "????????.???" (all wildcards)
    // Offset 1..9 = '?'
    // Offset 9..12 = '?'
    for i in 0..11 {
        cpu.bus.write_8(fcb_phys + 1 + i, b'?');
    }
    cpu.bus.write_8(fcb_phys, 0); // Drive Default

    cpu.ds = fcb_seg as u16;
    cpu.set_reg16(Register::DX, fcb_off as u16);
    cpu.set_reg8(Register::AH, 0x11);

    println!("DEBUG: Invoking FCB FindFirst");
    int21::handle(&mut cpu);

    // Check Success (AL=0 is success)
    let al = cpu.get_reg8(Register::AL);
    if al != 0 {
        panic!("FCB FindFirst failed. AL={:02X}", al);
    }

    // Result should be in DTA (40000)
    let dta_phys = 0x40000;
    // Check Filename at DTA+1 (11 bytes fixed width)
    let mut name_bytes = Vec::new();
    for i in 0..11 {
        name_bytes.push(cpu.bus.read_8(dta_phys + 1 + i));
    }
    let name = String::from_utf8_lossy(&name_bytes);
    println!("FCB Found: '{}'", name);

    // FindNext Loop
    for _ in 0..10 {
        cpu.set_reg8(Register::AH, 0x12);
        int21::handle(&mut cpu);
        let al = cpu.get_reg8(Register::AL);
        if al != 0 {
            println!("FCB FindNext ended.");
            break;
        }

        let mut nb = Vec::new();
        for i in 0..11 {
            nb.push(cpu.bus.read_8(dta_phys + 1 + i));
        }
        println!("FCB Found: '{}'", String::from_utf8_lossy(&nb));
    }
}

#[test]
fn test_find_next_with_path() {
    let mut cpu = Cpu::new(PathBuf::from("."));

    // Test that FindNext remembers the directory from FindFirst
    // Search for "tests\*.rs"

    // 1. Set DTA
    cpu.ds = 0x2000;
    cpu.set_reg16(Register::DX, 0x0000);
    cpu.set_reg8(Register::AH, 0x1A);
    int21::handle(&mut cpu);

    // 2. Write pattern "tests\*.rs"
    let spec_seg = 0x3000;
    let spec_off = 0x0000;
    let spec_phys = (spec_seg * 16 + spec_off) as usize;
    let pattern = b"tests\\*.rs\0";
    for (i, b) in pattern.iter().enumerate() {
        cpu.bus.write_8(spec_phys + i, *b);
    }

    // 3. FindFirst
    cpu.ds = spec_seg as u16;
    cpu.set_reg16(Register::DX, spec_off as u16);
    cpu.set_reg16(Register::CX, 0x10);
    cpu.set_reg8(Register::AH, 0x4E);
    int21::handle(&mut cpu);

    let ax = cpu.get_reg16(Register::AX);
    assert_eq!(ax, 0, "FindFirst failed for tests\\*.rs. Error: {:X}", ax);

    // Check first result
    let dta_phys = 0x20000;
    let mut filename = String::new();
    for i in 0..13 {
        let b = cpu.bus.read_8(dta_phys + 0x1E + i);
        if b == 0 {
            break;
        }
        filename.push(b as char);
    }
    println!("Found 1: {}", filename);

    // 4. FindNext Loop
    let mut count = 1;
    for _ in 0..10 {
        cpu.set_reg8(Register::AH, 0x4F);
        int21::handle(&mut cpu);
        let ax = cpu.get_reg16(Register::AX);
        if ax != 0 {
            break;
        }

        // Read name
        let mut f_name = String::new();
        for i in 0..13 {
            let b = cpu.bus.read_8(dta_phys + 0x1E + i);
            if b == 0 {
                break;
            }
            f_name.push(b as char);
        }
        println!("Found {}: {}", count + 1, f_name);
        count += 1;
    }

    // We should find at least 2 files in tests/ (bus_tests.rs, disk_reproduction.rs, etc)
    assert!(count >= 2, "Should find multiple .rs files in tests/");
}
