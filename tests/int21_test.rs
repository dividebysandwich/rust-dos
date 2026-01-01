use iced_x86::Register;
use rust_dos::cpu::{Cpu, CpuFlags};
use std::fs;
use std::path::PathBuf;

#[test]
fn test_int21_ah29_parse_filename() {
    let root_path = PathBuf::from("target/test_int21_ah29");
    if root_path.exists() {
        fs::remove_dir_all(&root_path).unwrap();
    }
    fs::create_dir_all(&root_path).unwrap();

    let mut cpu = Cpu::new(root_path.clone());

    // Setup input string at DS:SI (0x2000:0000)
    cpu.ds = 0x2000;
    cpu.set_reg16(Register::SI, 0x0000);
    let input_str = "TEST.TXT "; // Space terminated
    let mut phys_si = cpu.get_physical_addr(cpu.ds, 0x0000);
    for b in input_str.bytes() {
        cpu.bus.write_8(phys_si, b);
        phys_si += 1;
    }
    cpu.bus.write_8(phys_si, 0x00); // Null term just in case

    // Setup Output FCB at ES:DI (0x3000:0000)
    cpu.es = 0x3000;
    cpu.set_reg16(Register::DI, 0x0000);

    // Call INT 21, AH=29
    cpu.set_reg8(Register::AH, 0x29);
    cpu.set_reg8(Register::AL, 0x00); // No special flags

    // We can't rely on full execution loop for unit test easily without running code.
    // So we manually invoke the handler.
    rust_dos::interrupts::int21::handle(&mut cpu);

    // Verify Output
    let phys_di = cpu.get_physical_addr(cpu.es, 0x0000);

    // Byte 0: Drive (0=Default)
    assert_eq!(cpu.bus.read_8(phys_di), 0x00, "Drive should be 0");

    // Bytes 1-8: Name "TEST    "
    let expected_name = b"TEST    ";
    for i in 0..8 {
        assert_eq!(
            cpu.bus.read_8(phys_di + 1 + i),
            expected_name[i],
            "Name mismatch at index {}",
            i
        );
    }

    // Bytes 9-11: Ext "TXT"
    let expected_ext = b"TXT";
    for i in 0..3 {
        assert_eq!(
            cpu.bus.read_8(phys_di + 9 + i),
            expected_ext[i],
            "Ext mismatch at index {}",
            i
        );
    }

    // Verify AL = 0 (No wildcards)
    assert_eq!(cpu.get_reg8(Register::AL), 0x00);

    // Verify SI advanced?
    // Our implementation simply adds length of token.
    // "TEST.TXT" is 8 bytes long string wise.
    // SI should be advanced by 8?
    // In our impl: split_whitespace returns "TEST.TXT". len=8.
    // SI was 0. Should be 8.
    assert_eq!(cpu.get_reg16(Register::SI), 8);

    fs::remove_dir_all(&root_path).unwrap();
}

#[test]
fn test_int21_ah4b_exec() {
    let root_path = PathBuf::from("target/test_int21_ah4b");
    if root_path.exists() {
        fs::remove_dir_all(&root_path).unwrap();
    }
    fs::create_dir_all(&root_path).unwrap();

    let mut cpu = Cpu::new(root_path.clone());

    // Create a dummy COM file
    let com_path = root_path.join("RUNME.COM");
    fs::write(&com_path, vec![0x90, 0xCD, 0x20]).unwrap(); // NOP, INT 20

    // Setup Filename at DS:DX
    cpu.ds = 0x2000;
    cpu.dx = 0x0000;
    let filename = "RUNME.COM";
    let mut phys_dx = cpu.get_physical_addr(cpu.ds, cpu.dx);
    for b in filename.bytes() {
        cpu.bus.write_8(phys_dx, b);
        phys_dx += 1;
    }
    cpu.bus.write_8(phys_dx, 0);

    // Setup Parameter Block at ES:BX
    // We strictly need this to exist, even if null.
    cpu.es = 0x3000;
    cpu.bx = 0x0000;
    let param_phys = cpu.get_physical_addr(cpu.es, cpu.bx);

    // Set Command Line Pointer (Offset 2, Seg 4) to 0x4000:0000
    cpu.bus.write_16(param_phys + 2, 0x0000); // Offset
    cpu.bus.write_16(param_phys + 4, 0x4000); // Segment

    // Setup Command Line at 0x4000:0000
    // [LEN][Chars][CR]
    let cmd_tail = " FOO BAR"; // Space at start is standard DOS compat
    let cmd_phys = cpu.get_physical_addr(0x4000, 0x0000);
    cpu.bus.write_8(cmd_phys, cmd_tail.len() as u8);
    for (i, b) in cmd_tail.bytes().enumerate() {
        cpu.bus.write_8(cmd_phys + 1 + i, b);
    }
    cpu.bus.write_8(cmd_phys + 1 + cmd_tail.len(), 0x0D);

    // Invoke EXEC
    cpu.set_reg8(Register::AH, 0x4B);
    cpu.set_reg8(Register::AL, 0x00); // Load and Execute

    // We need to ensure load_executable can find the file.
    // It uses standard filesystem ops relative to root_path.

    rust_dos::interrupts::int21::handle(&mut cpu);

    // Verify Success
    assert!(
        !cpu.get_cpu_flag(CpuFlags::CF),
        "CF should be clear on success"
    );

    // Verify CS:IP reset (COM file)
    assert_eq!(cpu.cs, 0x2000);
    assert_eq!(cpu.ip, 0x100);

    // Verify PSP Command Tail
    // PSP is at DS:0000 (after load)
    let psp_seg = cpu.ds;
    assert_eq!(psp_seg, 0x2000);

    let psp_phys = cpu.get_physical_addr(psp_seg, 0x80);
    let tail_len = cpu.bus.read_8(psp_phys);
    assert_eq!(tail_len, cmd_tail.len() as u8);

    let mut tail_read = String::new();
    for i in 0..tail_len {
        tail_read.push(cpu.bus.read_8(psp_phys + 1 + i as usize) as char);
    }
    assert_eq!(tail_read, cmd_tail);

    // Verify Environment Block
    // Since we didn't specify one, it should use Default.
    // Check offsets, etc.
    let env_seg_ptr_phys = cpu.get_physical_addr(psp_seg, 0x2C);
    let new_env_seg = cpu.bus.read_16(env_seg_ptr_phys);
    assert_eq!(new_env_seg, 0x0C00, "Should use scratch segment 0x0C00");

    fs::remove_dir_all(&root_path).unwrap();
}

#[test]
fn test_int21_ah4b_exec_with_env() {
    let root_path = PathBuf::from("target/test_int21_ah4b_env");
    if root_path.exists() {
        fs::remove_dir_all(&root_path).unwrap();
    }
    fs::create_dir_all(&root_path).unwrap();

    let mut cpu = Cpu::new(root_path.clone());

    // Create dummy COM
    let com_path = root_path.join("ENVtest.COM");
    fs::write(&com_path, vec![0x90, 0xCD, 0x20]).unwrap();

    // Setup Filename
    cpu.ds = 0x2000;
    cpu.dx = 0x0000;
    let filename = "ENVtest.COM";
    let mut phys_dx = cpu.get_physical_addr(cpu.ds, cpu.dx);
    for b in filename.bytes() {
        cpu.bus.write_8(phys_dx, b);
        phys_dx += 1;
    }
    cpu.bus.write_8(phys_dx, 0);

    // Setup Custom Env Block at 0x5000:0000
    // "VAR=VAL\0\0"
    let env_src_seg = 0x5000;
    let env_src_phys = cpu.get_physical_addr(env_src_seg, 0);
    let env_data = b"VAR=VAL\0\0";
    for (i, &b) in env_data.iter().enumerate() {
        cpu.bus.write_8(env_src_phys + i, b);
    }

    // Setup Parameter Block at ES:BX
    cpu.es = 0x3000;
    cpu.bx = 0x0000;
    let param_phys = cpu.get_physical_addr(cpu.es, cpu.bx);

    // Offset 0: Env Seg
    cpu.bus.write_16(param_phys, env_src_seg);
    // Offset 2: Cmd Line (Null)
    cpu.bus.write_16(param_phys + 2, 0);
    cpu.bus.write_16(param_phys + 4, 0);

    // CALL EXEC
    cpu.set_reg8(Register::AH, 0x4B);
    cpu.set_reg8(Register::AL, 0x00);

    rust_dos::interrupts::int21::handle(&mut cpu);

    assert!(!cpu.get_cpu_flag(CpuFlags::CF));

    // Verify New Env Block at 0x0C00
    let new_env_seg = 0x0C00;
    let new_env_phys = cpu.get_physical_addr(new_env_seg, 0);

    for (i, &b) in env_data.iter().enumerate() {
        assert_eq!(
            cpu.bus.read_8(new_env_phys + i),
            b,
            "Env byte mismatch at index {}",
            i
        );
    }

    // Verify PSP -> Env Pointer
    let psp_phys = cpu.get_physical_addr(cpu.ds, 0x2C); // DS is new PSP
    assert_eq!(cpu.bus.read_16(psp_phys), new_env_seg);

    fs::remove_dir_all(&root_path).unwrap();
}

#[test]
fn test_int21_ah4b_exec_inheritance() {
    let root_path = PathBuf::from("target/test_int21_ah4b_inherit");
    if root_path.exists() {
        fs::remove_dir_all(&root_path).unwrap();
    }
    fs::create_dir_all(&root_path).unwrap();

    let mut cpu = Cpu::new(root_path.clone());

    // Create dummy COM
    let com_path = root_path.join("INHERIT.COM");
    fs::write(&com_path, vec![0x90, 0xCD, 0x20]).unwrap();

    // Setup Parent PSP at 0x1000
    cpu.current_psp = 0x1000;
    let parent_psp_phys = cpu.get_physical_addr(0x1000, 0);
    // Parent Env at 0x0500
    let parent_env_seg = 0x0500;
    cpu.bus.write_16(parent_psp_phys + 0x2C, parent_env_seg);

    // Setup Parent Env Content
    let parent_env_phys = cpu.get_physical_addr(parent_env_seg, 0);
    let env_data = b"PARENT=TRUE\0\0";
    for (i, &b) in env_data.iter().enumerate() {
        cpu.bus.write_8(parent_env_phys + i, b);
    }

    // Setup Filename
    cpu.ds = 0x2000;
    cpu.dx = 0x0000;
    let filename = "INHERIT.COM";
    let mut phys_dx = cpu.get_physical_addr(cpu.ds, cpu.dx);
    for b in filename.bytes() {
        cpu.bus.write_8(phys_dx, b);
        phys_dx += 1;
    }
    cpu.bus.write_8(phys_dx, 0);

    // Setup Parameter Block at ES:BX
    cpu.es = 0x3000;
    cpu.bx = 0x0000;
    let param_phys = cpu.get_physical_addr(cpu.es, cpu.bx);

    // Offset 0: Env Seg = 0 (INHERIT)
    cpu.bus.write_16(param_phys, 0x0000);
    // Offset 2: Cmd Line (Null)
    cpu.bus.write_16(param_phys + 2, 0);
    cpu.bus.write_16(param_phys + 4, 0);

    // CALL EXEC
    cpu.set_reg8(Register::AH, 0x4B);
    cpu.set_reg8(Register::AL, 0x00);

    rust_dos::interrupts::int21::handle(&mut cpu);

    assert!(!cpu.get_cpu_flag(CpuFlags::CF));

    // Verify New Env Block at 0x0C00
    let new_env_seg = 0x0C00;
    let new_env_phys = cpu.get_physical_addr(new_env_seg, 0);

    for (i, &b) in env_data.iter().enumerate() {
        assert_eq!(
            cpu.bus.read_8(new_env_phys + i),
            b,
            "Inherited Env byte mismatch at index {}",
            i
        );
    }

    // Verify PSP -> Env Pointer
    let psp_phys = cpu.get_physical_addr(cpu.ds, 0x2C); // DS is new PSP
    assert_eq!(cpu.bus.read_16(psp_phys), new_env_seg);

    fs::remove_dir_all(&root_path).unwrap();
}
