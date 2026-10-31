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
    cpu.set_ds(0x2000);
    cpu.set_reg16(Register::SI, 0x0000);
    let input_str = "TEST.TXT "; // Space terminated
    let mut phys_si = cpu.get_physical_addr(cpu.ds(), 0x0000);
    for b in input_str.bytes() {
        cpu.bus.write_8(phys_si, b);
        phys_si += 1;
    }
    cpu.bus.write_8(phys_si, 0x00); // Null term just in case

    // Setup Output FCB at ES:DI (0x3000:0000)
    cpu.set_es(0x3000);
    cpu.set_reg16(Register::DI, 0x0000);

    // Call INT 21, AH=29
    cpu.set_reg8(Register::AH, 0x29);
    cpu.set_reg8(Register::AL, 0x00); // No special flags

    // We can't rely on full execution loop for unit test easily without running code.
    // So we manually invoke the handler.
    rust_dos::interrupts::int21::handle(&mut cpu);

    // Verify Output
    let phys_di = cpu.get_physical_addr(cpu.es(), 0x0000);

    // Byte 0: Drive (0=Default)
    assert_eq!(cpu.bus.read_8(phys_di), 0x00, "Drive should be 0");

    // Bytes 1-8: Name "TEST    "
    let expected_name = b"TEST    ";
    for (i, &byte) in expected_name.iter().enumerate() {
        assert_eq!(cpu.bus.read_8(phys_di + 1 + i), byte, "Name mismatch at index {}", i);
    }

    // Bytes 9-11: Ext "TXT"
    let expected_ext = b"TXT";
    for (i, &byte) in expected_ext.iter().enumerate() {
        assert_eq!(cpu.bus.read_8(phys_di + 9 + i), byte, "Ext mismatch at index {}", i);
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
    cpu.set_ds(0x2000);
    cpu.set_dx(0x0000);
    let filename = "RUNME.COM";
    let mut phys_dx = cpu.get_physical_addr(cpu.ds(), cpu.dx());
    for b in filename.bytes() {
        cpu.bus.write_8(phys_dx, b);
        phys_dx += 1;
    }
    cpu.bus.write_8(phys_dx, 0);

    // Setup Parameter Block at ES:BX
    // We strictly need this to exist, even if null.
    cpu.set_es(0x3000);
    cpu.set_bx(0x0000);
    let param_phys = cpu.get_physical_addr(cpu.es(), cpu.bx());

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

    // Verify CS:IP reset (COM file). The exact segment is whatever the MCB
    // allocator handed out for the child; just confirm the offset and that
    // DS/CS agree (the PSP lives at CS:0000 for a COM file).
    assert_eq!(cpu.ip(), 0x100);
    let psp_seg = cpu.ds();
    assert_eq!(psp_seg, cpu.cs(), "DS should equal CS for a COM child");
    assert!(psp_seg > rust_dos::mcb::first_mcb(&cpu.bus), "PSP segment should be in conventional memory");

    let psp_phys = cpu.get_physical_addr(psp_seg, 0x80);
    let tail_len = cpu.bus.read_8(psp_phys);
    assert_eq!(tail_len, cmd_tail.len() as u8);

    let mut tail_read = String::new();
    for i in 0..tail_len {
        tail_read.push(cpu.bus.read_8(psp_phys + 1 + i as usize) as char);
    }
    assert_eq!(tail_read, cmd_tail);

    // The child gets an environment block of its own, owned by it.
    let env_seg_ptr_phys = cpu.get_physical_addr(psp_seg, 0x2C);
    let new_env_seg = cpu.bus.read_16(env_seg_ptr_phys);
    assert_ne!(new_env_seg, 0);
    let env_mcb = rust_dos::mcb::read_mcb(&mut cpu.bus, new_env_seg - 1);
    assert_eq!(env_mcb.owner, psp_seg);

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
    cpu.set_ds(0x2000);
    cpu.set_dx(0x0000);
    let filename = "ENVtest.COM";
    let mut phys_dx = cpu.get_physical_addr(cpu.ds(), cpu.dx());
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
    cpu.set_es(0x3000);
    cpu.set_bx(0x0000);
    let param_phys = cpu.get_physical_addr(cpu.es(), cpu.bx());

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

    // The child's environment is a copy of the block passed, followed by
    // the program's path.
    let psp_phys = cpu.get_physical_addr(cpu.ds(), 0x2C); // DS is new PSP
    let new_env_seg = cpu.bus.read_16(psp_phys);
    assert_ne!(new_env_seg, env_src_seg);
    let new_env_phys = cpu.get_physical_addr(new_env_seg, 0);

    let expected = [&env_data[..], b"\x01\x00C:\\ENVTEST.COM\0"].concat();
    for (i, &b) in expected.iter().enumerate() {
        assert_eq!(
            cpu.bus.read_8(new_env_phys + i),
            b,
            "Env byte mismatch at index {}",
            i
        );
    }

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

    // Setup Parent PSP at 0x1000, and the stack its INT 21h runs on,
    // where DOS saves its registers.
    cpu.current_psp = 0x1000;
    cpu.set_ss(0x4000);
    cpu.set_sp(0x0100);
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
    cpu.set_ds(0x2000);
    cpu.set_dx(0x0000);
    let filename = "INHERIT.COM";
    let mut phys_dx = cpu.get_physical_addr(cpu.ds(), cpu.dx());
    for b in filename.bytes() {
        cpu.bus.write_8(phys_dx, b);
        phys_dx += 1;
    }
    cpu.bus.write_8(phys_dx, 0);

    // Setup Parameter Block at ES:BX
    cpu.set_es(0x3000);
    cpu.set_bx(0x0000);
    let param_phys = cpu.get_physical_addr(cpu.es(), cpu.bx());

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

    // The child's environment is a copy of the parent's.
    let psp_phys = cpu.get_physical_addr(cpu.ds(), 0x2C); // DS is new PSP
    let new_env_seg = cpu.bus.read_16(psp_phys);
    assert_ne!(new_env_seg, parent_env_seg);
    let new_env_phys = cpu.get_physical_addr(new_env_seg, 0);

    for (i, &b) in env_data.iter().enumerate() {
        assert_eq!(
            cpu.bus.read_8(new_env_phys + i),
            b,
            "Inherited Env byte mismatch at index {}",
            i
        );
    }

    fs::remove_dir_all(&root_path).unwrap();
}

/// DOS's console output expands a tab to the spaces up to the next column
/// that is a multiple of eight (AH=02h, AH=09h, and AH=40h to the screen),
/// where the BIOS would show the tab's character; AH=06h's output is raw.
#[test]
fn console_output_expands_tabs_to_the_next_multiple_of_eight() {
    let root_path = PathBuf::from("target/test_int21_tabs");
    let _ = fs::remove_dir_all(&root_path);
    fs::create_dir_all(&root_path).unwrap();
    let mut cpu = Cpu::new(root_path);
    cpu.load_shell();
    cpu.bus.cursor_x = 0;
    cpu.bus.cursor_y = 0;
    cpu.bus.write_8(0x0450, 0);
    cpu.bus.write_8(0x0451, 0);
    let cell = |cpu: &Cpu, col: usize| cpu.bus.read_8(0xB8000 + 2 * col);

    // AH=09h: "A", a tab to column 8, "B".
    cpu.set_ds(0x2000);
    for (i, &b) in b"A\tB$".iter().enumerate() {
        cpu.bus.write_8(0x20000 + i, b);
    }
    cpu.set_reg16(Register::DX, 0);
    cpu.set_reg8(Register::AH, 0x09);
    rust_dos::interrupts::int21::handle(&mut cpu);
    assert_eq!(cell(&cpu, 0), b'A');
    for col in 1..8 {
        assert_eq!(cell(&cpu, col), b' ', "column {}", col);
    }
    assert_eq!(cell(&cpu, 8), b'B');
    assert_eq!(cpu.bus.cursor_x, 9);

    // AH=02h: a tab from column 9 goes to 16.
    cpu.set_reg8(Register::AH, 0x02);
    cpu.set_reg8(Register::DL, 0x09);
    rust_dos::interrupts::int21::handle(&mut cpu);
    assert_eq!(cpu.bus.cursor_x, 16);

    // AH=06h: the tab's own character, one column.
    cpu.set_reg8(Register::AH, 0x06);
    cpu.set_reg8(Register::DL, 0x09);
    rust_dos::interrupts::int21::handle(&mut cpu);
    assert_eq!(cell(&cpu, 16), 0x09);
    assert_eq!(cpu.bus.cursor_x, 17);

    // AH=40h to handle 1: a tab from column 17 goes to 24, then "Z".
    for (i, &b) in b"\tZ".iter().enumerate() {
        cpu.bus.write_8(0x20000 + i, b);
    }
    cpu.set_reg8(Register::AH, 0x40);
    cpu.set_reg16(Register::BX, 1);
    cpu.set_reg16(Register::CX, 2);
    cpu.set_reg16(Register::DX, 0);
    rust_dos::interrupts::int21::handle(&mut cpu);
    assert_eq!(cpu.get_reg16(Register::AX), 2);
    assert_eq!(cell(&cpu, 24), b'Z');
    assert_eq!(cpu.bus.cursor_x, 25);
}

/// A program that puts CON in raw (binary) mode with IOCTL 4401h gets its
/// tabs written as they are by AH=40h, and 4400h reports the mode.
#[test]
fn raw_console_output_keeps_tabs() {
    let root_path = PathBuf::from("target/test_int21_raw_tabs");
    let _ = fs::remove_dir_all(&root_path);
    fs::create_dir_all(&root_path).unwrap();
    let mut cpu = Cpu::new(root_path);
    cpu.load_shell();
    cpu.bus.cursor_x = 0;
    cpu.bus.cursor_y = 0;
    let cell = |cpu: &Cpu, col: usize| cpu.bus.read_8(0xB8000 + 2 * col);
    let device_info = |cpu: &mut Cpu| {
        cpu.set_reg16(Register::AX, 0x4400);
        cpu.set_reg16(Register::BX, 1);
        rust_dos::interrupts::int21::handle(cpu);
        cpu.get_reg16(Register::DX)
    };
    let write_a_tab_b = |cpu: &mut Cpu| {
        cpu.set_ds(0x2000);
        for (i, &b) in b"A\tB".iter().enumerate() {
            cpu.bus.write_8(0x20000 + i, b);
        }
        cpu.set_reg8(Register::AH, 0x40);
        cpu.set_reg16(Register::BX, 1);
        cpu.set_reg16(Register::CX, 3);
        cpu.set_reg16(Register::DX, 0);
        rust_dos::interrupts::int21::handle(cpu);
    };
    assert_eq!(device_info(&mut cpu), 0x80D3);

    // Raw mode, bit 5 of DL: the tab's own character.
    cpu.set_reg16(Register::AX, 0x4401);
    cpu.set_reg16(Register::BX, 1);
    cpu.set_reg16(Register::DX, 0x00F3);
    rust_dos::interrupts::int21::handle(&mut cpu);
    assert_eq!(device_info(&mut cpu), 0x80F3);
    write_a_tab_b(&mut cpu);
    assert_eq!(cell(&cpu, 1), 0x09);
    assert_eq!(cell(&cpu, 2), b'B');
    assert_eq!(cpu.bus.cursor_x, 3);

    // Back to cooked: from column 3, the tab goes to 8.
    cpu.set_reg16(Register::AX, 0x4401);
    cpu.set_reg16(Register::BX, 1);
    cpu.set_reg16(Register::DX, 0x00D3);
    rust_dos::interrupts::int21::handle(&mut cpu);
    assert_eq!(device_info(&mut cpu), 0x80D3);
    write_a_tab_b(&mut cpu);
    assert_eq!(cell(&cpu, 4), b' ');
    assert_eq!(cell(&cpu, 8), b'B');
    assert_eq!(cpu.bus.cursor_x, 9);
}

/// MS-DOS 7 fails IOCTL subfunctions past 11h with error 1. Windows 98
/// Setup's network detection takes a successful 44E0h for Sun PC-NFS and
/// then asks for a PC-NFS network client (#318).
#[test]
fn ioctl_subfunctions_past_11h_are_invalid() {
    let root_path = PathBuf::from("target/test_int21_ioctl_invalid");
    let _ = fs::remove_dir_all(&root_path);
    fs::create_dir_all(&root_path).unwrap();
    let mut cpu = Cpu::new(root_path);
    cpu.load_shell();

    for ax in [0x44E0, 0x4452, 0x4412] {
        cpu.set_ds(0x2000);
        cpu.set_reg16(Register::DX, 0);
        cpu.set_reg16(Register::BX, 0);
        cpu.set_reg16(Register::AX, ax);
        cpu.set_cpu_flag(CpuFlags::CF, false);
        rust_dos::interrupts::int21::handle(&mut cpu);
        assert!(cpu.get_cpu_flag(CpuFlags::CF), "AX={ax:04X} should fail");
        assert_eq!(cpu.get_reg16(Register::AX), 0x0001, "AX={ax:04X}");
    }

    // 4400h on standard output still answers.
    cpu.set_reg16(Register::AX, 0x4400);
    cpu.set_reg16(Register::BX, 1);
    rust_dos::interrupts::int21::handle(&mut cpu);
    assert!(!cpu.get_cpu_flag(CpuFlags::CF));
    assert_eq!(cpu.get_reg16(Register::DX), 0x80D3);
}
