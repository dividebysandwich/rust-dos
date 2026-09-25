use iced_x86::{Decoder, DecoderOptions, FlowControl, Instruction, Mnemonic, OpKind};
use rust_dos::cpu::{Cpu, SHELL_SEGMENT, SHELL_STACK};
use rust_dos::disk::MountOptions;
use rust_dos::interrupts::handle_hle;
use rust_dos::shell::{SHELL_COMMAND_BOP, get_shell_code};
use std::fs;
use std::path::PathBuf;

const SHELL_BASE: u64 = 0x100;

fn scratch(name: &str, dirs: &[&str]) -> PathBuf {
    let base = PathBuf::from("target/test_shell").join(name);
    let _ = fs::remove_dir_all(&base);
    for d in dirs {
        fs::create_dir_all(base.join(d)).unwrap();
    }
    base
}

/// Decode the shell code at 0000:0100, treating each `FE 38 xx` trap and
/// `FE 39 xx` service as a 3-byte instruction. Returns (offset,
/// instruction) pairs; traps are None.
fn decode_shell() -> Vec<(u64, Option<Instruction>)> {
    let code = get_shell_code();
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < code.len() {
        let ip = SHELL_BASE + pos as u64;
        if code[pos] == 0xFE && matches!(code.get(pos + 1), Some(&0x38) | Some(&0x39)) {
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

    // The command trap returns to a JMP PROMPT_START.
    let labels = rust_dos::shell::labels();
    let at = |ip: u16| decoded.iter().position(|(i, _)| *i == ip as u64).expect("an instruction starts there");
    let (_, jmp) = &decoded[at(labels.after_trap)];
    let jmp = jmp.as_ref().unwrap();
    assert_eq!(jmp.mnemonic(), Mnemonic::Jmp);
    assert_eq!(jmp.near_branch_target(), labels.prompt_start as u64);
    let (trap_ip, trap) = &decoded[at(labels.after_trap) - 1];
    assert!(trap.is_none());
    assert_eq!(get_shell_code()[(*trap_ip - SHELL_BASE) as usize + 2], SHELL_COMMAND_BOP);
    assert!(decoded[at(labels.prompt_start)].1.is_none(), "the prompt is the emulator's");
    assert!(decoded[at(labels.key_ready)].1.is_none(), "the key goes to the emulator");
    at(labels.shell_wait);
    at(labels.key_read);

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
    assert_eq!(mov_ax.immediate16(), labels.after_trap);

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
    // Right after the trap.
    let code = get_shell_code();
    let trap = code.windows(3).position(|w| w == [0xFE, 0x38, SHELL_COMMAND_BOP]).unwrap();
    let jmp_ip = (SHELL_BASE as usize + trap + 3) as u16;
    assert_eq!((cpu.cs(), cpu.ip(), cpu.sp()), (SHELL_SEGMENT, jmp_ip, SHELL_STACK));

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

/// An extended key: AL 0, AH its scan code (48h Up, 50h Down, 4Bh Left).
fn extended_key(cpu: &mut Cpu, scan: u8) {
    cpu.bus.keyboard_buffer.push_back((scan as u16) << 8);
}

const UP: u8 = 0x48;
const DOWN: u8 = 0x50;

#[test]
fn up_and_down_step_through_the_command_history() {
    let base = scratch("history", &["c"]);
    let mut cpu = Cpu::new(base.join("c"));
    cpu.load_shell();
    // Nothing yet: Up does nothing.
    extended_key(&mut cpu, UP);
    type_keys(&mut cpu, "echo one\r");
    assert_eq!(run_until_command(&mut cpu).as_deref(), Some("echo one"));
    type_keys(&mut cpu, "echo two\r");
    assert_eq!(run_until_command(&mut cpu).as_deref(), Some("echo two"));

    // Up: the last line, on the screen and entered.
    extended_key(&mut cpu, UP);
    type_keys(&mut cpu, "\r");
    assert_eq!(run_until_command(&mut cpu).as_deref(), Some("echo two"));
    assert!(screen_text(&cpu).contains("C:\\>echo two"), "{}", screen_text(&cpu));
    // The same line again isn't kept twice: Up, Up is the first line, and
    // Down from there the second.
    extended_key(&mut cpu, UP);
    extended_key(&mut cpu, UP);
    extended_key(&mut cpu, UP);
    extended_key(&mut cpu, DOWN);
    type_keys(&mut cpu, "\r");
    assert_eq!(run_until_command(&mut cpu).as_deref(), Some("echo two"));
    assert_eq!(cpu.shell_history.entries(), ["echo one", "echo two"]);

    // Down past the newest line: an empty one to type in.
    extended_key(&mut cpu, UP);
    extended_key(&mut cpu, DOWN);
    type_keys(&mut cpu, "ver\r");
    assert_eq!(run_until_command(&mut cpu).as_deref(), Some("ver"));

    // A recalled line can be changed before it is entered.
    extended_key(&mut cpu, UP);
    extended_key(&mut cpu, UP);
    type_keys(&mut cpu, "\x08\x08\x08six\r");
    assert_eq!(run_until_command(&mut cpu).as_deref(), Some("echo six"));
    // What was there is gone from the screen: "echo six", then the shorter
    // "ver" over it.
    extended_key(&mut cpu, UP);
    extended_key(&mut cpu, UP);
    type_keys(&mut cpu, "\r");
    assert_eq!(run_until_command(&mut cpu).as_deref(), Some("ver"));
    let text = screen_text(&cpu);
    let last = text.as_bytes().chunks(80).map(|r| String::from_utf8_lossy(r).trim_end().to_string()).filter(|r| r.starts_with("C:\\>ver")).last();
    assert_eq!(last.as_deref(), Some("C:\\>ver"), "{}", text);
}

#[test]
fn other_extended_keys_leave_the_line_alone() {
    let base = scratch("extended", &["c"]);
    let mut cpu = Cpu::new(base.join("c"));
    cpu.load_shell();
    // Left and F1 used to end the line at a NUL.
    type_keys(&mut cpu, "ab");
    extended_key(&mut cpu, 0x4B);
    extended_key(&mut cpu, 0x3B);
    // A grey arrow's E0h.
    cpu.bus.keyboard_buffer.push_back(0x4DE0);
    type_keys(&mut cpu, "c\r");
    assert_eq!(run_until_command(&mut cpu).as_deref(), Some("abc"));
}

#[test]
fn esc_blanks_the_line() {
    let base = scratch("esc", &["c"]);
    let mut cpu = Cpu::new(base.join("c"));
    cpu.load_shell();
    // Esc used to be typed as a left arrow.
    type_keys(&mut cpu, "dir\x1bver\r");
    assert_eq!(run_until_command(&mut cpu).as_deref(), Some("ver"));
    let text = screen_text(&cpu);
    assert_eq!(text.trim_end(), "C:\\>ver", "{}", text);
}

#[test]
fn control_keys_are_not_typed() {
    let base = scratch("control", &["c"]);
    let mut cpu = Cpu::new(base.join("c"));
    cpu.load_shell();
    // Tab and Ctrl+A used to show as a circle and a face.
    type_keys(&mut cpu, "a\tb\x01c\r");
    assert_eq!(run_until_command(&mut cpu).as_deref(), Some("abc"));
    let text = screen_text(&cpu);
    assert_eq!(text.trim_end(), "C:\\>abc", "{}", text);
}

const SHIFT_TAB: u8 = 0x0F;

/// The last row of the screen that begins with `prefix`, without the
/// blanks after it.
fn last_row_with(cpu: &Cpu, prefix: &str) -> Option<String> {
    let text = screen_text(cpu);
    text.as_bytes()
        .chunks(80)
        .map(|r| String::from_utf8_lossy(r).trim_end().to_string())
        .filter(|r| r.starts_with(prefix))
        .last()
}

#[test]
fn tab_completes_file_and_directory_names() {
    let base = scratch("tab", &["c/GAMES", "c/GRAPHICS"]);
    fs::write(base.join("c/GO.EXE"), b"").unwrap();
    fs::write(base.join("c/GAMES/DOOM.EXE"), b"").unwrap();
    fs::write(base.join("c/GAMES/DOOM.WAD"), b"").unwrap();
    let mut cpu = Cpu::new(base.join("c"));
    cpu.load_shell();

    // The programs first, then the others by name, and round again.
    for (tabs, line) in [(1, "GO.EXE"), (2, "GAMES"), (3, "GRAPHICS"), (4, "GO.EXE")] {
        type_keys(&mut cpu, "g");
        type_keys(&mut cpu, &"\t".repeat(tabs));
        type_keys(&mut cpu, "\r");
        assert_eq!(run_until_command(&mut cpu).as_deref(), Some(line), "{} tabs", tabs);
    }
    // A shorter name leaves nothing of the longer one on the screen.
    assert_eq!(last_row_with(&cpu, "C:\\>GA").as_deref(), Some("C:\\>GAMES"));

    // Shift+Tab goes backwards, from the last.
    type_keys(&mut cpu, "g");
    extended_key(&mut cpu, SHIFT_TAB);
    extended_key(&mut cpu, SHIFT_TAB);
    type_keys(&mut cpu, "\r");
    assert_eq!(run_until_command(&mut cpu).as_deref(), Some("GAMES"));

    // The last word, in the directory it names; typing more starts over.
    type_keys(&mut cpu, "type ga\t\\d\t\t\r");
    assert_eq!(run_until_command(&mut cpu).as_deref(), Some("type GAMES\\DOOM.WAD"));
    assert_eq!(last_row_with(&cpu, "C:\\>type").as_deref(), Some("C:\\>type GAMES\\DOOM.WAD"));

    // CD goes only through the directories.
    type_keys(&mut cpu, "cd \t\t\t\r");
    assert_eq!(run_until_command(&mut cpu).as_deref(), Some("cd GAMES"));

    // Nothing fits: Tab does nothing.
    type_keys(&mut cpu, "x\ty\r");
    assert_eq!(run_until_command(&mut cpu).as_deref(), Some("xy"));
    assert_eq!(last_row_with(&cpu, "C:\\>x").as_deref(), Some("C:\\>xy"));
}

#[test]
fn a_line_takes_at_most_127_characters() {
    let base = scratch("long_line", &["c"]);
    let mut cpu = Cpu::new(base.join("c"));
    cpu.load_shell();
    type_keys(&mut cpu, &"x".repeat(200));
    type_keys(&mut cpu, "\r");
    let line = run_until_command(&mut cpu).unwrap();
    assert_eq!(line.len(), rust_dos::shell::MAX_LINE);
    // The directory buffer after it is untouched: the prompt still shows.
    type_keys(&mut cpu, "ver\r");
    assert_eq!(run_until_command(&mut cpu).as_deref(), Some("ver"));
}

#[test]
fn a_recalled_line_that_wrapped_is_erased_whole() {
    let base = scratch("history_wrap", &["c"]);
    let mut cpu = Cpu::new(base.join("c"));
    cpu.load_shell();
    type_keys(&mut cpu, &"y".repeat(100));
    type_keys(&mut cpu, "\r");
    assert_eq!(run_until_command(&mut cpu).map(|l| l.len()), Some(100));
    type_keys(&mut cpu, "ab\r");
    assert_eq!(run_until_command(&mut cpu).as_deref(), Some("ab"));
    // The long line onto two rows, then the short one over it.
    extended_key(&mut cpu, UP);
    extended_key(&mut cpu, UP);
    extended_key(&mut cpu, DOWN);
    type_keys(&mut cpu, "\r");
    assert_eq!(run_until_command(&mut cpu).as_deref(), Some("ab"));
    let text = screen_text(&cpu);
    let rows: Vec<String> = text.as_bytes().chunks(80).map(|r| String::from_utf8_lossy(r).trim_end().to_string()).collect();
    // The recalled line, with the long one's second row blank under it (the
    // next prompt comes after the command runs).
    let at = rows.iter().rposition(|r| r == "C:\\>ab").expect("the recalled line");
    assert_eq!(rows[at + 1], "", "{:?}", &rows[..at + 2]);
}

#[test]
fn code_page_437_characters_reach_the_command_line() {
    let base = scratch("cp437", &["c"]);
    let mut cpu = Cpu::new(base.join("c"));
    cpu.load_shell();
    // "echo ä" with the umlaut as its code page 437 byte, 84h.
    for key in [b'e', b'c', b'h', b'o', b' ', 0x84, b'\r'] {
        cpu.bus.keyboard_buffer.push_back(key as u16);
    }
    assert_eq!(run_until_command(&mut cpu).as_deref(), Some("echo \u{84}"));
}
