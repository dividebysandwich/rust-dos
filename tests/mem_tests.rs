//! MEM, as MS-DOS 6.22 shows memory: at the prompt, with upper and
//! expanded memory, and with a TSR resident low and high.

use rust_dos::cpu::{Cpu, CpuState};
use rust_dos::interrupts::int21;
use rust_dos::mcb::{self, Mcb};
use rust_dos::mem_command::text;
use std::fs;
use std::path::PathBuf;

fn scratch(name: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let base = PathBuf::from("target/test_mem").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    for (file, bytes) in files {
        fs::write(base.join(file), bytes).unwrap();
    }
    base
}

/// A machine at its prompt, with upper memory and EMS if asked, and DOS's
/// tables spread up to 64 KB (`dos_high=false`), where these numbers are.
fn machine(name: &str, ems: bool, umb: bool) -> Cpu {
    // TSR.COM just loops; the tests make the DOS calls.
    let mut cpu = Cpu::new(scratch(name, &[("TSR.COM", &[0xEB, 0xFE])]));
    cpu.set_dos_high(false).unwrap();
    cpu.set_upper_memory(ems, umb).unwrap();
    cpu.load_shell();
    cpu
}

/// MEM's lines, without the CRs.
fn mem(cpu: &mut Cpu, args: &str) -> Vec<String> {
    text(&mut cpu.bus, args).split("\r\n").map(str::to_string).collect()
}

fn has(lines: &[String], line: &str) -> bool {
    lines.iter().any(|l| l == line)
}

/// TSR.COM, loaded low or high, stays resident with 20h paragraphs.
fn load_tsr(cpu: &mut Cpu, high: bool) -> u16 {
    let loaded = if high { cpu.load_executable_high("TSR.COM") } else { cpu.load_executable("TSR.COM", None) };
    assert!(loaded);
    let psp = cpu.current_psp;
    cpu.set_dx(0x20);
    cpu.set_ax(0x3100);
    int21::handle(cpu);
    assert_eq!(cpu.state, CpuState::RebootShell);
    cpu.load_shell();
    psp
}

#[test]
fn summary_at_the_prompt() {
    let mut cpu = machine("plain", false, false);
    let lines = mem(&mut cpu, "");
    for line in [
        "Memory Type        Total  =   Used  +   Free",
        "----------------  -------   -------   -------",
        "Conventional         640K       64K      576K",
        "Upper                  0K        0K        0K",
        // The adapters' area, so that it adds up to the RAM.
        "Reserved             384K      384K        0K",
        "Extended (XMS)    15,360K       64K   15,296K",
        "Total memory      16,384K      512K   15,872K",
        "Total under 1 MB     640K       64K      576K",
        "Largest executable program size       576K (589,824 bytes)  ",
        "Largest free upper memory block         0K       (0 bytes)  ",
        "The high memory area is available.",
    ] {
        assert!(has(&lines, line), "no {:?} in\n{}", line, lines.join("\n"));
    }
    // No expanded memory.
    assert!(!lines.iter().any(|l| l.contains("EMS")));
}

#[test]
fn dos_high_packs_dos_below_the_first_block() {
    let mut cpu = machine("high", false, false);
    cpu.set_dos_high(true).unwrap();
    let first = mcb::first_mcb(&cpu.bus);
    assert_eq!(first, 0x05A9);
    let lines = mem(&mut cpu, "");
    assert!(has(&lines, "Conventional         640K       23K      617K"), "{}", lines.join("\n"));
    // The List of Lists has the first MCB and the file table where they
    // are now, and a program loads above them.
    let lol = rust_dos::dos_data::address(rust_dos::dos_data::SYSVARS);
    assert_eq!(cpu.bus.read_16(lol - 2), first);
    assert_eq!(cpu.bus.read_16(lol + 6), rust_dos::dos_data::HIGH.sft);
    load_tsr(&mut cpu, false);
    assert!(cpu.set_dos_high(false).is_err(), "a resident program keeps the layout");
}

#[test]
fn upper_and_expanded_memory() {
    let mut cpu = machine("ems", true, true);
    let lines = mem(&mut cpu, "");
    // With EMS its page frame takes half of upper memory; the rest of the
    // adapters' area is reserved, and all of it adds up to the RAM.
    for line in [
        "Upper                 64K        0K       64K",
        "Reserved             320K      320K        0K",
        "Extended (XMS)*   15,360K       64K   15,296K",
        "Total memory      16,384K      448K   15,936K",
        "Total under 1 MB     704K       64K      640K",
        "* EMM386 is using XMS memory to simulate EMS memory as needed.",
    ] {
        assert!(has(&lines, line), "no {:?} in\n{}", line, lines.join("\n"));
    }
    // Wider than MS-DOS 6.22's columns, which cut it off.
    assert!(has(&lines, "Total Expanded (EMS)                15,296K (15,663,104 bytes)  "), "{}", lines.join("\n"));
    assert!(has(&lines, "Free Expanded (EMS)*                15,296K (15,663,104 bytes)  "));
    // The cover MCB at 9FFFh takes conventional memory's last paragraph.
    assert!(has(&lines, "Largest executable program size       576K (589,808 bytes)  "));
    assert!(has(&lines, "Largest free upper memory block        64K  (65,520 bytes)  "));
}

#[test]
fn resident_programs_by_name() {
    let mut cpu = machine("tsr", false, true);
    let low = load_tsr(&mut cpu, false);
    let high = load_tsr(&mut cpu, true);
    assert!(high > 0xD000, "{:04X}", high);

    let lines = mem(&mut cpu, "/c");
    // 20h paragraphs and the MCB, low and high.
    assert!(has(&lines, "  MSDOS       65,536   (64K)     65,536   (64K)          0    (0K)"), "{}", lines.join("\n"));
    assert!(has(&lines, "  TSR          1,056    (1K)        528    (1K)        528    (1K)"), "{}", lines.join("\n"));
    assert!(has(&lines, "  Conventional         655,360       66,064      589,296"), "{}", lines.join("\n"));

    let lines = mem(&mut cpu, "/m tsr");
    assert!(has(&lines, "TSR is using the following memory:"));
    assert!(lines.iter().any(|l| l.contains(&format!("{:05X}", low - 1)) && l.ends_with("Program")));
    assert!(lines.iter().any(|l| l.contains(&format!("{:05X}", high - 1)) && l.ends_with("Program")));

    // Every block, the free ones among them.
    let lines = mem(&mut cpu, "/d");
    assert!(lines.iter().any(|l| l.contains("TSR") && l.contains("Program")));
    assert!(lines.iter().any(|l| l.contains("-- Free --")));
}

#[test]
fn switches_it_refuses() {
    let mut cpu = machine("switches", false, false);
    assert_eq!(mem(&mut cpu, "/x"), ["Invalid switch - /x", ""]);
    assert_eq!(mem(&mut cpu, "/c /f"), ["Too many switches - /f", ""]);
    assert!(mem(&mut cpu, "/m").join("").contains("Required parameter missing"));
    assert!(has(&mem(&mut cpu, "/m nothere"), "NOTHERE is not currently in memory."));
    let help = mem(&mut cpu, "/?");
    assert!(help[0].starts_with("Displays the amount of used and free memory"));
    assert!(has(&help, "  /PAGE or /P      Pauses after each screenful of information."));
}

#[test]
fn the_shell_has_mem() {
    let mut cpu = machine("shell", false, false);
    let (command, args) = rust_dos::command::split_command("MEM /C");
    assert!(rust_dos::command::CommandDispatcher::new().dispatch(&mut cpu, command, args));
}

#[test]
fn a_programs_environment_and_data() {
    let mut cpu = machine("blocks", false, false);
    let bus = &mut cpu.bus;
    // As EXEC lays a child out: its environment, then its block with its
    // PSP, and a block it allocates.
    let env = mcb::alloc(bus, 0xFFFF, 4).unwrap();
    let psp = mcb::alloc(bus, 0xFFFF, 0x100).unwrap();
    let data = mcb::alloc(bus, psp, 0x10).unwrap();
    for block in [env, psp] {
        let m = mcb::read_mcb(bus, block - 1);
        mcb::write_mcb(bus, block - 1, &Mcb { owner: psp, ..m });
    }
    bus.write_16(psp as usize * 16 + 0x2C, env);
    let environment = b"PATH=Z:\\\0\0\x01\0C:\\TOOLS\\EDIT.COM\0";
    for (i, &b) in environment.iter().enumerate() {
        bus.write_8(env as usize * 16 + i, b);
    }

    // No name in its MCB: the one after its environment.
    let lines = mem(&mut cpu, "/m edit");
    let row = |segment: u16, kind: &str| lines.iter().any(|l| l.starts_with(&format!("   {:05X}", segment)) && l.ends_with(kind));
    assert!(row(env - 1, "Environment"), "{}", lines.join("\n"));
    assert!(row(psp - 1, "Program"), "{}", lines.join("\n"));
    assert!(row(data - 1, "Data"), "{}", lines.join("\n"));

    // EXEC names its block, and the name stays as the program resizes it.
    mcb::name_program(&mut cpu.bus, psp, "C:\\GAMES\\GAME.EXE");
    mcb::resize(&mut cpu.bus, psp, 0x80).unwrap();
    assert_eq!(mcb::read_name(&mut cpu.bus, psp - 1), "GAME");
    let lines = mem(&mut cpu, "/c");
    assert!(lines.iter().any(|l| l.starts_with("  GAME     ")), "{}", lines.join("\n"));
    // Freed, it has none.
    mcb::free_owned_by(&mut cpu.bus, psp);
    assert_eq!(mcb::read_name(&mut cpu.bus, psp - 1), "");
}
