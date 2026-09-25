use iced_x86::{Decoder, DecoderOptions, FlowControl, Instruction, Mnemonic, OpKind};
use rust_dos::cpu::{Cpu, SHELL_SEGMENT, SHELL_STACK};
use rust_dos::disk::MountOptions;
use rust_dos::interrupts::handle_hle;
use rust_dos::shell::{SHELL_COMMAND_BOP, get_shell_code};
use std::fs;
use std::path::PathBuf;

const SHELL_BASE: u64 = 0x100;
const PROMPT_START: u64 = 0x10B;

fn scratch(name: &str, dirs: &[&str]) -> PathBuf {
    let base = PathBuf::from("target/test_shell").join(name);
    let _ = fs::remove_dir_all(&base);
    for d in dirs {
        fs::create_dir_all(base.join(d)).unwrap();
    }
    base
}

/// Decode the shell code at 0000:0100, treating each `FE 38 xx` trap as a
/// 3-byte instruction. Returns (offset, instruction) pairs; traps are None.
fn decode_shell() -> Vec<(u64, Option<Instruction>)> {
    let code = get_shell_code();
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < code.len() {
        let ip = SHELL_BASE + pos as u64;
        if code[pos] == 0xFE && code.get(pos + 1) == Some(&0x38) {
            out.push((ip, None));
            pos += 3;
            continue;
        }
        let mut decoder = Decoder::with_ip(16, &code[pos..], ip, DecoderOptions::NONE);
        let instr = decoder.decode();
        assert!(!instr.is_invalid(), "invalid instruction at {:04X}", ip);
        pos += instr.len();
        out.push((ip, Some(instr)));
    }
    out
}

#[test]
fn shell_code_branches_land_on_instructions() {
    let decoded = decode_shell();
    let starts: Vec<u64> = decoded.iter().map(|(ip, _)| *ip).collect();

    for (ip, instr) in &decoded {
        let Some(instr) = instr else { continue };
        if matches!(
            instr.flow_control(),
            FlowControl::UnconditionalBranch | FlowControl::ConditionalBranch
        ) {
            let target = instr.near_branch_target();
            assert!(
                starts.contains(&target),
                "branch at {:04X} targets {:04X}, not an instruction start",
                ip,
                target
            );
        }
    }

    // The loop ends with JMP PROMPT_START, right after the command trap.
    let (last_ip, last) = decoded.last().unwrap();
    let last = last.as_ref().unwrap();
    assert_eq!(last.mnemonic(), Mnemonic::Jmp);
    assert_eq!(last.near_branch_target(), PROMPT_START);
    let (_, trap) = &decoded[decoded.len() - 2];
    assert!(trap.is_none());
    assert_eq!(
        get_shell_code()[(*last_ip - SHELL_BASE) as usize - 1],
        SHELL_COMMAND_BOP
    );

    // The return address pushed for the trap is that JMP.
    let mov_ax = decoded
        .iter()
        .filter_map(|(_, i)| i.as_ref())
        .find(|i| {
            i.mnemonic() == Mnemonic::Mov
                && i.op0_register() == iced_x86::Register::AX
                && i.op1_kind() == OpKind::Immediate16
        })
        .expect("MOV AX, imm16");
    assert_eq!(mov_ax.immediate16() as u64, *last_ip);

    // Code must stay clear of the input buffer at 0x200.
    assert!(SHELL_BASE + (get_shell_code().len() as u64) <= 0x200);
}

fn screen_text(cpu: &Cpu) -> String {
    cpu.bus
        .vga
        .vram_text
        .iter()
        .step_by(2)
        .take(80 * 25)
        .map(|&b| if b == 0 { ' ' } else { b as char })
        .collect()
}

fn type_keys(cpu: &mut Cpu, text: &str) {
    for b in text.bytes() {
        cpu.bus.keyboard_buffer.push_back(b as u16);
    }
}

/// Step the shell until it hands over a command (or give up).
fn run_until_command(cpu: &mut Cpu) -> Option<String> {
    for _ in 0..200_000 {
        cpu.step();
        if cpu.pending_command.is_some() {
            return cpu.pending_command.take();
        }
    }
    None
}

#[test]
fn shell_trap_returns_to_the_prompt_loop() {
    let base = scratch("trap", &["c"]);
    let mut cpu = Cpu::new(base.join("c"));
    cpu.load_shell();
    type_keys(&mut cpu, "ver\r");

    assert_eq!(run_until_command(&mut cpu).as_deref(), Some("ver"));
    // The trap popped the frame the shell pushed: back at the JMP with the
    // stack balanced, not sliding through the IVT from 0000:0000.
    assert_eq!((cpu.cs(), cpu.ip(), cpu.sp()), (SHELL_SEGMENT, 0x182, SHELL_STACK));

    // Next command comes through the same loop, prompt reprinted.
    type_keys(&mut cpu, "dir\r");
    assert_eq!(run_until_command(&mut cpu).as_deref(), Some("dir"));
    assert_eq!(screen_text(&cpu).matches("C:\\>").count(), 2);
}

#[test]
fn prompt_shows_the_current_drive() {
    let base = scratch("prompt", &["c", "d/GAMES"]);
    let mut cpu = Cpu::new(base.join("c"));
    cpu.bus
        .mount_drive(3, &base.join("d"), MountOptions::default(), false)
        .unwrap();
    cpu.bus.disk.set_current_drive(3);
    assert!(cpu.bus.disk.set_current_directory("GAMES"));
    cpu.load_shell();
    type_keys(&mut cpu, "x\r");
    run_until_command(&mut cpu);
    assert!(
        screen_text(&cpu).starts_with("D:\\GAMES>x"),
        "screen: {:?}",
        &screen_text(&cpu)[..20]
    );
    assert_eq!(rust_dos::shell::prompt_string(&cpu.bus.disk), "D:\\GAMES>");
}

#[test]
fn only_the_private_trap_queues_commands() {
    let base = scratch("vectors", &["c"]);
    let mut cpu = Cpu::new(base.join("c"));
    let text = b"hello\0";
    for (i, &b) in text.iter().enumerate() {
        cpu.bus.write_8(0x20000 + i, b);
    }
    cpu.set_ds(0x2000);
    cpu.set_dx(0);

    // A program's INT 2Fh (here the DPMI check) must not run as a command
    // or eat typed keys.
    cpu.bus.keyboard_buffer.push_back(b'k' as u16);
    cpu.set_ax(0x1687);
    handle_hle(&mut cpu, 0x2F);
    assert!(cpu.pending_command.is_none());
    assert_eq!(cpu.ax(), 0x1687);
    assert_eq!(cpu.bus.keyboard_buffer.len(), 1);

    handle_hle(&mut cpu, SHELL_COMMAND_BOP);
    assert_eq!(cpu.pending_command.as_deref(), Some("hello"));
}

#[test]
fn a_mode_set_at_the_prompt_leaves_the_shell_alone() {
    // The BIOS points INT 43h at the graphics font on every mode set, as a
    // change of video card at the prompt makes one. The shell's code is
    // clear of the vector table, so the prompt carries on below.
    let base = scratch("mode_set", &["c"]);
    let mut cpu = Cpu::new(base.join("c"));
    cpu.load_shell();
    rust_dos::interrupts::int10::set_mode(&mut cpu, 0x83);
    let code = get_shell_code();
    let at = SHELL_SEGMENT as usize * 16 + 0x100;
    assert_eq!((0..code.len()).map(|i| cpu.bus.read_8(at + i)).collect::<Vec<_>>(), code);

    type_keys(&mut cpu, "ver\r");
    assert_eq!(run_until_command(&mut cpu).as_deref(), Some("ver"));
    type_keys(&mut cpu, "dir\r");
    assert_eq!(run_until_command(&mut cpu).as_deref(), Some("dir"));
    let text = screen_text(&cpu);
    assert_eq!(text.matches("C:\\>").count(), 2, "{}", text);
    // The second prompt is on the next row, not over the first.
    assert_eq!(&text[..7], "C:\\>ver");
    assert_eq!(&text[80..87], "C:\\>dir");
}

#[test]
fn another_video_card_keeps_the_prompt_where_it_was() {
    use rust_dos::video::adapter::{Adapter, VideoSetup};
    let base = scratch("switch", &["c"]);
    let mut cpu = Cpu::new(base.join("c"));
    cpu.load_shell();
    type_keys(&mut cpu, "ver\r");
    assert_eq!(run_until_command(&mut cpu).as_deref(), Some("ver"));
    let before = (cpu.bus.read_8(0x0450), cpu.bus.read_8(0x0451));
    assert_eq!(before.1, 1);
    for adapter in [Adapter::Ega, Adapter::Cga, Adapter::Hercules, Adapter::Svga] {
        rust_dos::video::bios::switch(&mut cpu, VideoSetup { adapter, ..Default::default() });
        assert_eq!((cpu.bus.read_8(0x0450), cpu.bus.read_8(0x0451)), before, "{:?}", adapter);
        assert_eq!(&screen_text(&cpu)[..7], "C:\\>ver", "{:?}: the screen stays", adapter);
    }
}
