//! What DOS extenders need from DOS and the BIOS before they switch to
//! protected mode: the program loader, the environment with the program's
//! path, command tails, the XMS driver, and the DOS calls their runtimes
//! make.

use iced_x86::Register;
use rust_dos::command::CommandDispatcher;
use rust_dos::cpu::{Cpu, CpuFlags};
use rust_dos::exec::{self, ExecHook, StopReason};
use rust_dos::interrupts::int21;
use rust_dos::xms;
use std::fs;
use std::path::PathBuf;

/// Fresh directory with the given files under target/.
fn scratch(name: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let base = PathBuf::from("target/test_extender").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    for (file, bytes) in files {
        let path = base.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    base
}

/// An MZ program whose load module is `code`, followed by `overlay` bytes
/// the header doesn't count (as a bound DOS extender's protected-mode
/// image is).
fn mz(code: &[u8], overlay: &[u8]) -> Vec<u8> {
    let mut exe = vec![0u8; 0x20];
    let module = 0x20 + code.len();
    exe[0..2].copy_from_slice(b"MZ");
    exe[2..4].copy_from_slice(&((module % 512) as u16).to_le_bytes());
    exe[4..6].copy_from_slice(&(module.div_ceil(512) as u16).to_le_bytes());
    exe[8..10].copy_from_slice(&2u16.to_le_bytes()); // header paragraphs
    exe[12..14].copy_from_slice(&0xFFFFu16.to_le_bytes()); // max alloc
    exe[14..16].copy_from_slice(&0x0010u16.to_le_bytes()); // SS
    exe[16..18].copy_from_slice(&0x0100u16.to_le_bytes()); // SP
    exe[24..26].copy_from_slice(&0x1Cu16.to_le_bytes()); // relocation table
    exe.extend_from_slice(code);
    exe.extend_from_slice(overlay);
    exe
}

fn psp_word(cpu: &Cpu, offset: usize) -> u16 {
    cpu.bus.read_16(cpu.current_psp as usize * 16 + offset)
}

/// The environment strings and the program path after them.
fn environment(cpu: &Cpu, env_seg: u16) -> (Vec<String>, String) {
    let mut at = env_seg as usize * 16;
    let read_z = |at: &mut usize| {
        let mut s = String::new();
        loop {
            let b = cpu.bus.read_8(*at);
            *at += 1;
            if b == 0 {
                return s;
            }
            s.push(b as char);
        }
    };
    let mut vars = Vec::new();
    loop {
        let var = read_z(&mut at);
        if var.is_empty() {
            break;
        }
        vars.push(var);
    }
    assert_eq!(cpu.bus.read_16(at), 1, "one string after the environment");
    at += 2;
    (vars, read_z(&mut at))
}

#[test]
fn exe_loader_copies_only_the_load_module() {
    // B4 4C CD 21 -> MOV AH, 4Ch ; INT 21h, padded to a paragraph
    let mut code = vec![0xB4, 0x4C, 0xCD, 0x21];
    code.resize(16, 0x90);
    let dir = scratch("load_module", &[("PROG.EXE", &mz(&code, &[0xAA; 256]))]);
    let mut cpu = Cpu::new(dir);
    assert!(cpu.load_executable("PROG.EXE", None));

    let image = (cpu.current_psp as usize + 0x10) * 16;
    assert_eq!(cpu.bus.read_8(image), 0xB4);
    assert!(
        (16..272).all(|i| cpu.bus.read_8(image + i) != 0xAA),
        "the data after the load module stays in the file"
    );
}

#[test]
fn environment_ends_with_the_fully_qualified_program_path() {
    let code = [0xEB, 0xFE];
    let dir = scratch("env_path", &[("GAMES/PROG.EXE", &mz(&code, &[]))]);
    let mut cpu = Cpu::new(dir);
    assert!(cpu.bus.disk.set_current_directory("GAMES"));
    cpu.set_env("DOS4GVM", "1");
    assert!(cpu.load_executable("prog.exe", None));

    let env_seg = psp_word(&cpu, 0x2C);
    assert_ne!(env_seg, 0);
    let (vars, path) = environment(&cpu, env_seg);
    assert!(vars.iter().any(|v| v.starts_with("COMSPEC=")), "{:?}", vars);
    assert!(vars.contains(&"DOS4GVM=1".to_string()), "{:?}", vars);
    assert_eq!(path, "C:\\GAMES\\PROG.EXE");
}

/// Stops the execution loop once the shell has started a program.
struct StopInProgram;

impl ExecHook for StopInProgram {
    fn before_exec(&mut self, cpu: &Cpu, _phys_ip: usize, _ram: &[u8]) -> bool {
        cpu.cs() != 0
    }
}

#[test]
fn shell_passes_arguments_in_the_command_tail() {
    let dir = scratch("tail", &[("PROG.COM", &[0xEB, 0xFE])]);
    let mut cpu = Cpu::new(dir);
    cpu.load_shell();
    cpu.pending_command = Some("PROG -nosound  -verbose".to_string());
    cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
    let reason = exec::run_batch(&mut cpu, &mut StopInProgram, true);
    assert_eq!(reason, StopReason::Paused);

    let psp = cpu.current_psp as usize * 16;
    let len = cpu.bus.read_8(psp + 0x80) as usize;
    let tail: String = (0..len).map(|i| cpu.bus.read_8(psp + 0x81 + i) as char).collect();
    assert_eq!(tail, " -nosound  -verbose");
    assert_eq!(cpu.bus.read_8(psp + 0x81 + len), 0x0D);
}

#[test]
fn set_and_path_change_the_master_environment() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    let dispatcher = CommandDispatcher::new();
    assert!(dispatcher.dispatch(&mut cpu, "SET", "dos16m=@2"));
    assert_eq!(cpu.get_env("DOS16M"), Some("@2"));
    assert!(dispatcher.dispatch(&mut cpu, "SET", "DOS16M="));
    assert_eq!(cpu.get_env("DOS16M"), None);
    assert!(dispatcher.dispatch(&mut cpu, "PATH", "c:\\dos;c:\\bin"));
    assert_eq!(cpu.get_env("PATH"), Some("C:\\DOS;C:\\BIN"));
}

#[test]
fn exec_gives_the_child_its_own_environment() {
    let dir = scratch(
        "exec_env",
        &[("PARENT.COM", &[0xEB, 0xFE]), ("CHILD.COM", &[0xEB, 0xFE])],
    );
    let mut cpu = Cpu::new(dir);
    cpu.set_env("BLASTER", "A220 I7 D1");
    assert!(cpu.load_executable("PARENT.COM", None));
    let parent = cpu.current_psp;
    let parent_base = parent as usize * 16;

    // Name at PSP:0200, empty command tail at PSP:0210, parameter block
    // at PSP:0220 (environment 0: a copy of the parent's).
    cpu.bus.load_bytes(parent_base + 0x200, b"CHILD.COM\0");
    cpu.bus.load_bytes(parent_base + 0x210, &[0x00, 0x0D]);
    cpu.bus.load_bytes(parent_base + 0x220, &[0; 14]);
    cpu.bus.write_16(parent_base + 0x222, 0x210);
    cpu.bus.write_16(parent_base + 0x224, parent);
    cpu.set_ds(parent);
    cpu.set_dx(0x200);
    cpu.set_es(parent);
    cpu.set_bx(0x220);
    cpu.set_ax(0x4B00);
    int21::handle(&mut cpu);

    let child = cpu.current_psp;
    assert_ne!(child, parent);
    let env_seg = psp_word(&cpu, 0x2C);
    let (vars, path) = environment(&cpu, env_seg);
    assert!(vars.contains(&"BLASTER=A220 I7 D1".to_string()), "{:?}", vars);
    assert_eq!(path, "C:\\CHILD.COM");
    let env_mcb = rust_dos::mcb::read_mcb(&cpu.bus, env_seg - 1);
    assert_eq!(env_mcb.owner, child, "the child owns its environment");
}

/// Run from CS:IP until a HLT, through service traps and far calls.
fn run_to_hlt(cpu: &mut Cpu) {
    for _ in 0..1000 {
        let at = cpu.get_physical_addr(cpu.cs(), cpu.ip());
        if cpu.bus.read_8(at) == 0xF4 {
            return;
        }
        cpu.step();
    }
    panic!("no HLT reached");
}

#[test]
fn xms_driver_is_found_and_called_through_int_2fh() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    cpu.set_ss(0);
    cpu.set_sp(0x8000);
    cpu.set_ds(0);
    #[rustfmt::skip]
    let code = [
        0xB8, 0x00, 0x43,       // MOV AX, 4300h
        0xCD, 0x2F,             // INT 2Fh
        0xA2, 0x00, 0x06,       // MOV [0600h], AL
        0xB8, 0x10, 0x43,       // MOV AX, 4310h
        0xCD, 0x2F,             // INT 2Fh
        0x89, 0x1E, 0x04, 0x06, // MOV [0604h], BX
        0x8C, 0x06, 0x06, 0x06, // MOV [0606h], ES
        0xB4, 0x09,             // MOV AH, 09h (allocate)
        0xBA, 0x40, 0x00,       // MOV DX, 64 (KB)
        0xFF, 0x1E, 0x04, 0x06, // CALL FAR [0604h]
        0xA3, 0x08, 0x06,       // MOV [0608h], AX
        0xB4, 0x0C,             // MOV AH, 0Ch (lock)
        0xFF, 0x1E, 0x04, 0x06, // CALL FAR [0604h]
        0x89, 0x1E, 0x0A, 0x06, // MOV [060Ah], BX
        0x89, 0x16, 0x0C, 0x06, // MOV [060Ch], DX
        0xF4,                   // HLT
    ];
    cpu.bus.load_bytes(0x100, &code);
    run_to_hlt(&mut cpu);

    assert_eq!(cpu.bus.read_8(0x600), 0x80, "driver installed");
    assert_eq!(cpu.bus.read_16(0x606), 0xF000, "entry in the BIOS ROM");
    assert_eq!(cpu.bus.read_16(0x608), 1, "allocation succeeded");
    let base = cpu.bus.read_16(0x60A) as u32 | (cpu.bus.read_16(0x60C) as u32) << 16;
    assert!(base >= 0x11_0000, "block above the HMA: {:08X}", base);
    assert_eq!(cpu.ip(), 0x100 + code.len() as u16 - 1, "back from the far calls");
}

fn xms_call(cpu: &mut Cpu, ah: u8) -> bool {
    cpu.set_reg8(Register::AH, ah);
    xms::call(cpu);
    cpu.ax() == 1
}

#[test]
fn xms_moves_lock_free_and_a20() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    cpu.set_dx(16);
    assert!(xms_call(&mut cpu, 0x09));
    let handle = cpu.dx();

    // Move 16 bytes from 0000:7000 to offset 4 of the block.
    cpu.bus.load_bytes(0x7000, b"extended memory!");
    let params = 0x6000;
    cpu.bus.write_32(params, 16);
    cpu.bus.write_16(params + 4, 0);
    cpu.bus.write_32(params + 6, 0x0000_7000);
    cpu.bus.write_16(params + 10, handle);
    cpu.bus.write_32(params + 12, 4);
    cpu.set_ds(0);
    cpu.set_si(params as u16);
    assert!(xms_call(&mut cpu, 0x0B));

    cpu.set_dx(handle);
    assert!(xms_call(&mut cpu, 0x0C));
    let base = (cpu.dx() as usize) << 16 | cpu.bx() as usize;
    let copied: Vec<u8> = (0..16).map(|i| cpu.bus.read_8(base + 4 + i)).collect();
    assert_eq!(&copied, b"extended memory!");

    // A locked block can't be freed.
    cpu.set_dx(handle);
    assert!(!xms_call(&mut cpu, 0x0A));
    assert_eq!(cpu.get_reg8(Register::BL), 0xAB);
    cpu.set_dx(handle);
    assert!(xms_call(&mut cpu, 0x0D));
    cpu.set_dx(handle);
    assert!(xms_call(&mut cpu, 0x0A));

    // Odd lengths are rejected.
    cpu.bus.write_32(params, 3);
    assert!(!xms_call(&mut cpu, 0x0B));
    assert_eq!(cpu.get_reg8(Register::BL), 0xA7);

    // Local enables nest; the gate stays on until the last is undone.
    assert!(xms_call(&mut cpu, 0x05));
    assert!(xms_call(&mut cpu, 0x05));
    assert!(xms_call(&mut cpu, 0x06));
    assert!(cpu.bus.a20());
    xms_call(&mut cpu, 0x07);
    assert_eq!(cpu.ax(), 1);
    assert!(xms_call(&mut cpu, 0x06));
    assert!(!cpu.bus.a20());
}

fn int21(cpu: &mut Cpu, ax: u16) {
    cpu.set_ax(ax);
    int21::handle(cpu);
}

/// Put an ASCIIZ string at 2000:0000 and point DS:DX at it.
fn set_dsdx_string(cpu: &mut Cpu, s: &str) {
    cpu.bus.load_bytes(0x20000, s.as_bytes());
    cpu.bus.write_8(0x20000 + s.len(), 0);
    cpu.set_ds(0x2000);
    cpu.set_dx(0);
}

fn cf(cpu: &Cpu) -> bool {
    cpu.get_cpu_flag(CpuFlags::CF)
}

#[test]
fn file_calls_of_extender_runtimes() {
    let dir = scratch("files", &[("DATA.BIN", b"0123456789")]);
    let mut cpu = Cpu::new(dir.clone());

    // AH=5Bh: create new fails on an existing file.
    set_dsdx_string(&mut cpu, "DATA.BIN");
    int21(&mut cpu, 0x5B00);
    assert!(cf(&cpu));
    assert_eq!(cpu.ax(), 0x50);
    // AH=59h reports it.
    int21(&mut cpu, 0x5900);
    assert_eq!(cpu.ax(), 0x50);

    // AX=6C00h: open the existing file (action 1 = opened).
    cpu.bus.load_bytes(0x20000, b"DATA.BIN\0");
    cpu.set_ds(0x2000);
    cpu.set_si(0);
    cpu.set_bx(0x0000);
    cpu.set_dx(0x0001);
    int21(&mut cpu, 0x6C00);
    assert!(!cf(&cpu));
    assert_eq!(cpu.cx(), 1);
    let handle = cpu.ax();

    // AH=45h: the duplicate shares the file position.
    cpu.set_bx(handle);
    int21(&mut cpu, 0x4500);
    assert!(!cf(&cpu));
    let dup = cpu.ax();
    assert_ne!(dup, handle);
    cpu.set_bx(handle);
    cpu.set_cx(0);
    cpu.set_dx(4);
    int21(&mut cpu, 0x4200); // seek to 4
    cpu.set_bx(dup);
    cpu.set_cx(2);
    cpu.set_ds(0x3000);
    cpu.set_dx(0);
    int21(&mut cpu, 0x3F00);
    assert_eq!(cpu.ax(), 2);
    assert_eq!(cpu.bus.read_8(0x30000), b'4');

    // AX=5700h: a DOS date after 1980.
    cpu.set_bx(handle);
    int21(&mut cpu, 0x5700);
    assert!(!cf(&cpu));
    assert!(cpu.dx() >> 9 > 0);

    // AH=5Ah: a temporary file, its name appended to the directory.
    set_dsdx_string(&mut cpu, "C:\\");
    int21(&mut cpu, 0x5A00);
    assert!(!cf(&cpu));
    let name: String = (0..)
        .map(|i| cpu.bus.read_8(0x20000 + i))
        .take_while(|&b| b != 0)
        .map(|b| b as char)
        .collect();
    assert!(name.starts_with("C:\\") && name.len() == 11, "{}", name);
    let temp = name.trim_start_matches("C:\\").to_string();
    assert!(dir.join(&temp).exists());
    cpu.set_bx(cpu.ax());
    int21(&mut cpu, 0x3E00);

    // AH=56h renames, AH=41h deletes.
    set_dsdx_string(&mut cpu, &temp);
    cpu.bus.load_bytes(0x40000, b"VMM.SWP\0");
    cpu.set_es(0x4000);
    cpu.set_di(0);
    int21(&mut cpu, 0x5600);
    assert!(!cf(&cpu));
    assert!(dir.join("VMM.SWP").exists() && !dir.join(&temp).exists());
    set_dsdx_string(&mut cpu, "VMM.SWP");
    int21(&mut cpu, 0x4100);
    assert!(!cf(&cpu));
    assert!(!dir.join("VMM.SWP").exists());
    int21(&mut cpu, 0x4100);
    assert!(cf(&cpu));
    assert_eq!(cpu.ax(), 0x02);

    // AH=60h: the canonical name.
    cpu.bus.load_bytes(0x20000, b"data.bin\0");
    cpu.set_ds(0x2000);
    cpu.set_si(0);
    cpu.set_es(0x4000);
    cpu.set_di(0);
    int21(&mut cpu, 0x6000);
    assert!(!cf(&cpu));
    let full: String = (0..)
        .map(|i| cpu.bus.read_8(0x40000 + i))
        .take_while(|&b| b != 0)
        .map(|b| b as char)
        .collect();
    assert_eq!(full, "C:\\DATA.BIN");
}

#[test]
fn country_date_and_indos() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    // AH=38h: country information at DS:DX.
    cpu.set_ds(0x3000);
    cpu.set_dx(0);
    int21(&mut cpu, 0x3800);
    assert!(!cf(&cpu));
    assert_eq!(cpu.bx(), 1);
    assert_eq!(cpu.bus.read_8(0x30002), b'$');
    assert_eq!(cpu.bus.read_8(0x30009), b'.');

    // AH=65h AL=20h: upper case of a character.
    cpu.set_dx(b'q' as u16);
    int21(&mut cpu, 0x6520);
    assert_eq!(cpu.get_dl(), b'Q');

    // AH=2Ah: today.
    int21(&mut cpu, 0x2A00);
    assert!(cpu.cx() >= 2024);
    assert!((1..=12).contains(&(cpu.dx() >> 8)));

    // AH=34h: the InDOS flag is clear when a program looks.
    int21(&mut cpu, 0x3400);
    let flag = cpu.get_physical_addr(cpu.es(), cpu.bx());
    assert_eq!(cpu.bus.read_8(flag), 0);
}

#[test]
fn the_shell_starts_in_real_mode_with_extended_memory_free() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    cpu.set_dx(8 * 1024);
    assert!(xms_call(&mut cpu, 0x09));
    cpu.bus.io_write(0x92, 0x02);
    cpu.bus.io_write(0x70, 0x0F);
    cpu.bus.io_write(0x71, 0x0A);
    // What a DOS extender killed in protected mode leaves behind.
    cpu.cr0 |= rust_dos::cpu::CR0_PE;
    cpu.cr3 = 0x0010_0000;
    cpu.idtr.base = 0x0020_0000;

    cpu.load_shell();
    assert_eq!(cpu.cr0 & rust_dos::cpu::CR0_PE, 0);
    assert_eq!(cpu.cr3, 0);
    assert_eq!((cpu.idtr.base, cpu.idtr.limit), (0, 0x3FF));
    assert!(!cpu.bus.a20());
    cpu.bus.io_write(0x70, 0x0F);
    assert_eq!(cpu.bus.io_read(0x71), 0, "a reset is a cold boot again");
    // All of extended memory is free again.
    xms_call(&mut cpu, 0x08);
    assert_eq!(cpu.ax(), 15 * 1024 - 64);
}

#[test]
fn device_names_open_devices_not_files() {
    let dir = scratch("devices", &[]);
    let mut cpu = Cpu::new(dir.clone());
    for name in ["NUL", "C:\\SUB\\nul.txt", "PRN"] {
        set_dsdx_string(&mut cpu, name);
        int21(&mut cpu, 0x3C00);
        assert!(!cf(&cpu), "{}", name);
        let handle = cpu.ax();
        // Writes vanish, reads find nothing, and the handle is a device.
        cpu.set_bx(handle);
        cpu.set_cx(4);
        int21(&mut cpu, 0x4000);
        assert_eq!(cpu.ax(), 4);
        cpu.set_bx(handle);
        int21(&mut cpu, 0x3F00);
        assert_eq!(cpu.ax(), 0);
        cpu.set_bx(handle);
        int21(&mut cpu, 0x4400);
        assert!(cpu.dx() & 0x80 != 0, "character device");
        cpu.set_bx(handle);
        int21(&mut cpu, 0x3E00);
    }
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 0, "no files created");
}
