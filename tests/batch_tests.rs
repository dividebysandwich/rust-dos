//! Batch files on a running machine: their lines run at the prompt one
//! after the other, with their parameters and the environment's
//! variables put in, programs they start run before the next line, and
//! chaining, CALL, labels and Ctrl+C behave as in DOS.

use rust_dos::cpu::Cpu;
use rust_dos::exec::{self, NoHook, StopReason};
use std::fs;
use std::path::{Path, PathBuf};

fn scratch(name: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let base = PathBuf::from("target/test_batch").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    for (file, bytes) in files {
        fs::write(base.join(file), bytes).unwrap();
    }
    base
}

/// A .COM program that runs for a little while and exits with `code`.
fn exits_with(code: u8) -> Vec<u8> {
    #[rustfmt::skip]
    let program = vec![
        0xB9, 0x00, 0x40,       // MOV CX, 4000h
        0xE2, 0xFE,             // LOOP $
        0xB8, code, 0x4C,       // MOV AX, 4Cxxh
        0xCD, 0x21,             // INT 21h
    ];
    program
}

fn machine(dir: &Path) -> Cpu {
    let mut cpu = Cpu::new(dir.to_path_buf());
    cpu.bus.set_cycles_per_ms(1000);
    cpu.load_shell();
    cpu
}

/// Run the machine for up to `ms` ms of emulated time, until `stop`.
fn run_until(cpu: &mut Cpu, ms: u64, stop: impl Fn(&Cpu) -> bool) -> bool {
    for _ in 0..ms {
        if stop(cpu) {
            return true;
        }
        let end = cpu.bus.clock.icount + 1000;
        cpu.bus.start_batch(end);
        exec::run_batch(cpu, &mut NoHook, false);
    }
    stop(cpu)
}

/// Run until the batch files have ended and the prompt is back.
fn run_batch_files(cpu: &mut Cpu) {
    assert!(
        run_until(cpu, 10_000, |cpu| !cpu.batch.is_active() && cpu.pending_command.is_none() && cpu.shell_idle()),
        "the batch files end"
    );
}

/// The text screen's rows, without their trailing blanks.
fn rows(cpu: &Cpu) -> Vec<String> {
    cpu.bus
        .vga
        .vram_text
        .chunks(160)
        .take(25)
        .map(|row| row.iter().step_by(2).map(|&b| if b == 0 { ' ' } else { b as char }).collect::<String>().trim_end().to_string())
        .collect()
}

fn screen(cpu: &Cpu) -> String {
    rows(cpu).join("\n").trim_end().to_string()
}

#[test]
fn parameters_and_variables_are_put_in() {
    let dir = scratch("params", &[("GO.BAT", b"@echo off\r\necho %0 [%1] [%2] [%3]\r\necho %GAME%!\r\n")]);
    let mut cpu = machine(&dir);
    cpu.queue_batch_lines(["@SET GAME=keen", "@GO one two"]);
    run_batch_files(&mut cpu);
    let screen = screen(&cpu);
    assert!(screen.contains("GO [one] [two] []\nkeen!"), "{}", screen);
    assert!(cpu.batch.echo, "ECHO is on again after the batch file");
}

#[test]
fn lines_are_echoed_at_the_prompt_unless_echo_is_off() {
    let dir = scratch("echo", &[("X.BAT", b"echo one\r\n@echo two\r\necho off\r\necho three\r\n")]);
    let mut cpu = machine(&dir);
    cpu.pending_command = Some("X".into());
    run_batch_files(&mut cpu);
    let screen = screen(&cpu);
    assert!(screen.contains("C:\\>echo one\none\ntwo\nC:\\>echo off\nthree"), "{}", screen);
}

#[test]
fn a_program_runs_before_the_next_line() {
    let dir = scratch("program", &[("RUN.BAT", b"@echo off\r\nEXIT3\r\necho after\r\n"), ("EXIT3.COM", &exits_with(3))]);
    let mut cpu = machine(&dir);
    cpu.queue_batch_lines(["RUN"]);
    assert!(run_until(&mut cpu, 1000, |cpu| !cpu.shell_idle()), "the program starts");
    assert!(cpu.batch.is_active(), "the rest of the batch file waits for it");
    run_batch_files(&mut cpu);
    assert!(screen(&cpu).contains("after"), "{}", screen(&cpu));
}

#[test]
fn autoexec_bat_runs_after_autoexec_lines_that_chain_to_a_batch_file() {
    let dir = scratch(
        "chain",
        &[("X.BAT", b"@echo in x\r\n"), ("AUTOEXEC.BAT", b"@echo in autoexec\r\n")],
    );
    let mut cpu = machine(&dir);
    cpu.queue_batch_lines(["@X", "@echo never"]);
    assert!(cpu.queue_batch_file("C:\\AUTOEXEC.BAT"));
    run_batch_files(&mut cpu);
    let screen = screen(&cpu);
    assert!(screen.contains("in x\nin autoexec"), "{}", screen);
    assert!(!screen.contains("never"), "chaining never comes back: {}", screen);
}

#[test]
fn code_page_437_characters_are_echoed_as_they_are() {
    let dir = scratch("cp437", &[("MENU.BAT", b"@echo \xC9\xCD\xBB \x84\r\n")]);
    let mut cpu = machine(&dir);
    cpu.pending_command = Some("MENU".into());
    run_batch_files(&mut cpu);
    let text: Vec<u8> = cpu.bus.vga.vram_text.iter().step_by(2).copied().collect();
    assert!(text.windows(5).any(|w| w == b"\xC9\xCD\xBB \x84"));
}

#[test]
fn a_batch_file_that_never_ends_gives_the_machine_back_and_ctrl_c_ends_it() {
    // It starts itself again and again, and never runs a program.
    let dir = scratch("forever", &[("LOOP.BAT", b"@LOOP\r\n")]);
    let mut cpu = machine(&dir);
    cpu.pending_command = Some("LOOP".into());
    let end = cpu.bus.clock.icount + 100_000;
    cpu.bus.start_batch(end);
    assert_eq!(exec::run_batch(&mut cpu, &mut NoHook, false), StopReason::BatchEnd);
    assert!(cpu.batch.is_active());

    cpu.bus.keyboard_buffer.push_back(0x2E03); // Ctrl+C
    run_batch_files(&mut cpu);
    assert!(screen(&cpu).contains("^C"));
}

#[test]
fn a_typed_batch_file_gets_its_arguments() {
    let dir = scratch("typed", &[("GO.BAT", b"@echo [%1]\r\n")]);
    let mut cpu = machine(&dir);
    cpu.pending_command = Some("go.bat hello".into());
    run_batch_files(&mut cpu);
    assert!(screen(&cpu).contains("[hello]"), "{}", screen(&cpu));
}

#[test]
fn if_errorlevel_tests_the_last_program_s_exit_code() {
    let dir = scratch(
        "errorlevel",
        &[
            ("TEST.BAT", b"@echo off\r\nEXIT3\r\nif errorlevel 4 echo four\r\nif errorlevel 3 echo three\r\nif not errorlevel 3 echo below\r\nEXIT0\r\nif not errorlevel 1 echo zero\r\n"),
            ("EXIT3.COM", &exits_with(3)),
            ("EXIT0.COM", &exits_with(0)),
        ],
    );
    let mut cpu = machine(&dir);
    cpu.pending_command = Some("TEST".into());
    run_batch_files(&mut cpu);
    // Each program clears the screen as it ends, so only what came after the last shows.
    assert!(screen(&cpu).contains("zero"), "{}", screen(&cpu));
    assert_eq!(cpu.errorlevel, 0);

    let dir = scratch("errorlevel3", &[("T.BAT", b"@echo off\r\nEXIT3\r\nif errorlevel 4 echo four\r\nif errorlevel 3 echo three\r\nif not errorlevel 3 echo below\r\n"), ("EXIT3.COM", &exits_with(3))]);
    let mut cpu = machine(&dir);
    cpu.pending_command = Some("T".into());
    run_batch_files(&mut cpu);
    let screen = screen(&cpu);
    assert!(screen.contains("three") && !screen.contains("four") && !screen.contains("below"), "{}", screen);
    assert_eq!(cpu.errorlevel, 3);
}

#[test]
fn if_compares_strings_and_finds_files() {
    let dir = scratch(
        "if",
        &[
            ("T.BAT", b"@echo off\r\nif \"%1\"==\"\" echo no parameter\r\nif %1==x echo x given\r\nif not %1==y echo not y\r\nif exist T.BAT echo found\r\nif exist *.XYZ echo wild\r\nif not exist NONE.TXT echo missing\r\nif exist SUB\\NUL echo dir\r\n"),
            ("A.XYZ", b""),
        ],
    );
    fs::create_dir_all(dir.join("SUB")).unwrap();
    let mut cpu = machine(&dir);
    cpu.pending_command = Some("T x".into());
    run_batch_files(&mut cpu);
    let screen = screen(&cpu);
    assert!(screen.contains("x given\nnot y\nfound\nwild\nmissing\ndir"), "{}", screen);
    assert!(!screen.contains("no parameter"));
}

#[test]
fn goto_jumps_to_labels_and_a_missing_label_ends_the_file() {
    let dir = scratch(
        "goto",
        &[("T.BAT", b"@echo off\r\nset N=\r\n:again\r\nif \"%N%\"==\"xxx\" goto done\r\nset N=%N%x\r\necho %N%\r\ngoto again\r\n:done\r\necho done\r\ngoto nowhere\r\necho never\r\n")],
    );
    let mut cpu = machine(&dir);
    cpu.pending_command = Some("T".into());
    run_batch_files(&mut cpu);
    let screen = screen(&cpu);
    assert!(screen.contains("x\nxx\nxxx\ndone\nLabel not found"), "{}", screen);
    assert!(!screen.contains("never"));
}

#[test]
fn call_comes_back_and_shift_moves_the_parameters() {
    let dir = scratch(
        "call",
        &[
            ("MAIN.BAT", b"@echo off\r\ncall sub one two\r\necho back in main\r\n"),
            ("SUB.BAT", b"echo sub %1\r\nshift\r\necho sub %1\r\n"),
        ],
    );
    let mut cpu = machine(&dir);
    cpu.pending_command = Some("MAIN".into());
    run_batch_files(&mut cpu);
    let screen = screen(&cpu);
    assert!(screen.contains("sub one\nsub two\nback in main"), "{}", screen);
}

#[test]
fn for_runs_a_command_for_every_member_and_every_matching_file() {
    let dir = scratch(
        "for",
        &[("T.BAT", b"@echo off\r\nfor %%v in (a b,c) do echo [%%v]\r\nfor %%f in (*.TXT) do type %%f\r\n"), ("ONE.TXT", b"first"), ("TWO.TXT", b"second")],
    );
    let mut cpu = machine(&dir);
    cpu.pending_command = Some("T".into());
    run_batch_files(&mut cpu);
    let screen = screen(&cpu);
    assert!(screen.contains("[a]\n[b]\n[c]\nfirst\nsecond"), "{}", screen);
}

#[test]
fn pause_waits_for_a_key_with_the_clock_running() {
    let dir = scratch("pause", &[("T.BAT", b"@echo off\r\npause\r\necho after\r\n")]);
    let mut cpu = machine(&dir);
    cpu.pending_command = Some("T".into());
    run_until(&mut cpu, 300, |_| false);
    let ticks = cpu.bus.read_16(0x046C);
    run_until(&mut cpu, 300, |_| false);
    assert!(cpu.bus.read_16(0x046C) > ticks, "the timer ticks while PAUSE waits");
    let text = screen(&cpu);
    assert!(text.contains("Press any key to continue . . .") && !text.contains("after"), "{}", text);

    cpu.bus.keyboard_buffer.push_back(0x1E61);
    run_batch_files(&mut cpu);
    assert!(screen(&cpu).contains("continue . . .\nafter"), "{}", screen(&cpu));
}

#[test]
fn choice_sets_the_errorlevel_to_the_key_s_position() {
    let dir = scratch(
        "choice",
        &[("T.BAT", b"@echo off\r\nchoice /c:ynq Go on\r\nif errorlevel 3 goto q\r\nif errorlevel 2 goto n\r\necho yes\r\ngoto end\r\n:n\r\necho no\r\ngoto end\r\n:q\r\necho quit\r\n:end\r\n")],
    );
    let mut cpu = machine(&dir);
    cpu.pending_command = Some("T".into());
    run_until(&mut cpu, 100, |_| false);
    assert!(screen(&cpu).contains("Go on[Y,N,Q]?"), "{}", screen(&cpu));
    // A key it doesn't take is ignored.
    cpu.bus.keyboard_buffer.push_back(0x2D78); // x
    run_until(&mut cpu, 100, |_| false);
    assert!(cpu.shell_wait.is_some());
    cpu.bus.keyboard_buffer.push_back(0x316E); // n
    run_batch_files(&mut cpu);
    assert!(screen(&cpu).contains("Go on[Y,N,Q]?N\nno"), "{}", screen(&cpu));
    assert_eq!(cpu.errorlevel, 2);
}

#[test]
fn choice_takes_its_default_when_the_time_is_up() {
    let dir = scratch("choice_timeout", &[("T.BAT", b"@echo off\r\nchoice /N /T:n,2\r\necho done %1\r\n")]);
    let mut cpu = machine(&dir);
    cpu.pending_command = Some("T".into());
    run_until(&mut cpu, 1500, |_| false);
    assert!(cpu.shell_wait.is_some(), "still waiting after 1.5 s");
    run_batch_files(&mut cpu);
    assert!(screen(&cpu).contains("N\ndone"), "{}", screen(&cpu));
    assert_eq!(cpu.errorlevel, 2);
}

#[test]
fn prompt_codes_make_the_prompt() {
    let dir = scratch("prompt", &[]);
    let mut cpu = machine(&dir);
    cpu.queue_batch_lines(["@prompt $n$g", "@echo x", "@prompt [$p]$_$$"]);
    run_batch_files(&mut cpu);
    let text = screen(&cpu);
    assert!(text.ends_with("x\n[C:\\]\n$"), "{}", text);
}

#[test]
fn echo_off_at_the_prompt_hides_the_prompt() {
    let dir = scratch("echo_off", &[]);
    let mut cpu = machine(&dir);
    for line in [b"echo off\r".as_slice(), b"echo hi\r"] {
        for b in line {
            cpu.bus.keyboard_buffer.push_back(*b as u16);
        }
        run_until(&mut cpu, 100, |_| false);
    }
    assert_eq!(screen(&cpu), "C:\\>echo off\necho hi\nhi");
}

#[test]
fn batch_lines_run_while_the_prompt_waits_for_a_key() {
    let dir = scratch("abandon", &[]);
    let mut cpu = machine(&dir);
    // The prompt is up and waiting for a key, something typed already.
    for key in [0x1E61u16, 0x3062] {
        cpu.bus.keyboard_buffer.push_back(key);
    }
    run_until(&mut cpu, 100, |_| false);
    assert_eq!(screen(&cpu), "C:\\>ab");
    cpu.queue_batch_lines(["echo one"]);
    run_batch_files(&mut cpu);
    assert_eq!(screen(&cpu), "C:\\>echo one\none\nC:\\>");
}

fn type_line(cpu: &mut Cpu, line: &[u8]) {
    for &b in line {
        cpu.bus.keyboard_buffer.push_back(b as u16);
    }
    run_until(cpu, 100, |_| false);
}

#[test]
fn date_and_time_set_the_machine_s_clock() {
    use chrono::NaiveDate;
    rust_dos::hosttime::fix(NaiveDate::from_ymd_opt(2026, 9, 25).unwrap().and_hms_opt(14, 3, 5));
    let dir = scratch("date", &[]);
    let mut cpu = machine(&dir);
    type_line(&mut cpu, b"date\r");
    assert!(screen(&cpu).contains("Current date is Fri 09-25-2026\nEnter new date (mm-dd-yy):"), "{}", screen(&cpu));
    type_line(&mut cpu, b"13-01-93\r");
    assert!(screen(&cpu).contains("Invalid date\nEnter new date (mm-dd-yy):"), "{}", screen(&cpu));
    type_line(&mut cpu, b"12-24-93\r");
    type_line(&mut cpu, b"time 23:59\r");

    // DOS and the BIOS both see the new date and time.
    cpu.set_ax(0x2A00);
    rust_dos::interrupts::int21::handle(&mut cpu);
    assert_eq!((cpu.cx(), cpu.dx()), (1993, 0x0C18));
    cpu.set_ax(0x0400);
    rust_dos::interrupts::int1a::handle(&mut cpu);
    assert_eq!((cpu.cx(), cpu.dx()), (0x1993, 0x1224));
    cpu.set_ax(0x0200);
    rust_dos::interrupts::int1a::handle(&mut cpu);
    assert_eq!(cpu.cx(), 0x2359, "{}", screen(&cpu));
    // INT 21h AH=2Bh refuses a date that isn't one.
    cpu.set_ax(0x2B00);
    cpu.set_cx(1999);
    cpu.set_dx(0x021E); // February 30th
    rust_dos::interrupts::int21::handle(&mut cpu);
    assert_eq!(cpu.get_al(), 0xFF);
    rust_dos::hosttime::fix(None);
}

#[test]
fn ctrl_c_gives_up_the_line_at_the_prompt() {
    let dir = scratch("ctrl_c", &[]);
    let mut cpu = machine(&dir);
    type_line(&mut cpu, b"dir\x03");
    type_line(&mut cpu, b"ver\r");
    assert!(screen(&cpu).starts_with("C:\\>dir^C\nC:\\>ver\nRust-DOS"), "{}", screen(&cpu));
}

#[test]
fn programs_are_found_in_the_current_directory_then_on_the_path() {
    let dir = scratch("path", &[("HELLO.COM", &exits_with(1))]);
    fs::create_dir_all(dir.join("BIN")).unwrap();
    fs::write(dir.join("BIN/HELLO.COM"), exits_with(7)).unwrap();
    fs::write(dir.join("BIN/TOOL.EXE.TXT"), b"").unwrap();
    fs::write(dir.join("BIN/GO.BAT"), b"@echo went %1\r\n").unwrap();
    let mut cpu = machine(&dir);
    cpu.queue_batch_lines(["@SET PATH=C:\\BIN", "@HELLO"]);
    run_batch_files(&mut cpu);
    assert_eq!(cpu.errorlevel, 1, "the current directory comes first");

    fs::remove_file(dir.join("HELLO.COM")).unwrap();
    cpu.queue_batch_lines(["@HELLO"]);
    run_batch_files(&mut cpu);
    assert_eq!(cpu.errorlevel, 7, "then PATH");

    // (GO takes the place of the lines it is run from.)
    cpu.queue_batch_lines(["@GO there"]);
    cpu.queue_batch_lines(["@TOOL.EXE.TXT"]);
    run_batch_files(&mut cpu);
    let screen = screen(&cpu);
    assert!(screen.contains("went there\nBad command or file name."), "{}", screen);
}

#[test]
fn a_program_reads_a_line_with_int_21h_0ah() {
    #[rustfmt::skip]
    let read = [
        0xC6, 0x06, 0x00, 0x02, 0x0A,   // MOV BYTE [0200h], 10
        0xB4, 0x0A,                     // MOV AH, 0Ah
        0xBA, 0x00, 0x02,               // MOV DX, 0200h
        0xCD, 0x21,                     // INT 21h
        0xA0, 0x01, 0x02,               // MOV AL, [0201h]: the count
        0xB4, 0x4C,                     // MOV AH, 4Ch
        0xCD, 0x21,                     // INT 21h
    ];
    let dir = scratch("read_line", &[("READ.COM", &read)]);
    let mut cpu = machine(&dir);
    cpu.pending_command = Some("READ".into());
    run_until(&mut cpu, 200, |_| false);
    assert!(!cpu.shell_idle(), "it waits for the line");
    for key in b"abc\r" {
        cpu.bus.keyboard_buffer.push_back(*key as u16);
    }
    assert!(run_until(&mut cpu, 1000, |cpu| cpu.shell_idle()));
    assert_eq!(cpu.errorlevel, 3);
}

#[test]
fn a_program_finds_its_parameters_in_its_fcbs_and_its_dta_at_psp_80h() {
    #[rustfmt::skip]
    let fcb = [
        0xB4, 0x2F, 0xCD, 0x21,         // INT 21h AH=2Fh: ES:BX = the DTA
        0x80, 0xFB, 0x80,               // CMP BL, 80h
        0x75, 0x07,                     // JNE fail
        0xA0, 0x5E, 0x00,               // MOV AL, [5Eh]: the first FCB's second letter
        0xB4, 0x4C, 0xCD, 0x21,         // exit with it
        0x00, 0x00,
        0xB8, 0x01, 0x4C, 0xCD, 0x21,   // fail: exit with 1
    ];
    let dir = scratch("fcbs", &[("FCB.COM", &fcb)]);
    let mut cpu = machine(&dir);
    cpu.queue_batch_lines(["@FCB c:game.dat other"]);
    run_batch_files(&mut cpu);
    assert_eq!(cpu.errorlevel, b'A', "GAME from C:GAME.DAT, upper case");
}

/// A program that runs C:\COMMAND.COM with the command tail `tail` and
/// exits with the exit code it got back (INT 21h AH=4Dh).
fn shell_out(tail: &str) -> Vec<u8> {
    use rust_dos::asm16::Asm;
    let mut a = Asm::new(0x100);
    a.op(&[0xB4, 0x4A, 0xBB, 0x00, 0x10, 0xCD, 0x21]); // shrink to 64 KB
    for field in ["TAIL_SEG", "FCB1_SEG", "FCB2_SEG"] {
        a.address(&[0x8C, 0x0E], field); // MOV [field], CS
    }
    a.address(&[0xBB], "PARAMS"); // MOV BX, PARAMS
    a.address(&[0xBA], "NAME"); // MOV DX, NAME
    a.op(&[0xB8, 0x00, 0x4B, 0xCD, 0x21]); // EXEC
    a.op(&[0xB4, 0x4D, 0xCD, 0x21]); // AH=4Dh: AL = its exit code
    a.op(&[0xB4, 0x4C, 0xCD, 0x21]); // exit with it
    a.label("NAME");
    a.op(b"C:\\COMMAND.COM\0");
    a.label("TAIL");
    a.op(&[tail.len() as u8]);
    a.op(tail.as_bytes());
    a.op(&[0x0D]);
    a.label("FCB");
    a.op(&[0; 16]);
    a.label("PARAMS");
    a.op(&[0, 0]);
    a.address(&[], "TAIL");
    a.label("TAIL_SEG");
    a.op(&[0, 0]);
    a.address(&[], "FCB");
    a.label("FCB1_SEG");
    a.op(&[0, 0]);
    a.address(&[], "FCB");
    a.label("FCB2_SEG");
    a.op(&[0, 0]);
    a.finish()
}

#[test]
fn a_program_runs_commands_through_command_com() {
    let dir = scratch(
        "command_com",
        &[
            ("SHCOPY.COM", &shell_out(" /C COPY A.TXT B.TXT")),
            ("BAT.COM", &shell_out(" /C X.BAT")),
            ("FINDIT.COM", &shell_out(" /C HELLO one")),
            ("X.BAT", b"@echo off\r\nEXIT5\r\n"),
            ("EXIT5.COM", &exits_with(5)),
            ("A.TXT", b"text"),
        ],
    );
    fs::create_dir_all(dir.join("BIN")).unwrap();
    fs::write(dir.join("BIN/HELLO.COM"), exits_with(7)).unwrap();
    let mut cpu = machine(&dir);

    cpu.pending_command = Some("SHCOPY".into());
    run_batch_files(&mut cpu);
    assert_eq!(fs::read(dir.join("B.TXT")).unwrap(), b"text", "a built-in command");
    assert_eq!(cpu.errorlevel, 0);

    cpu.pending_command = Some("BAT".into());
    run_batch_files(&mut cpu);
    assert_eq!(cpu.errorlevel, 5, "a batch file's program's exit code comes back");

    cpu.queue_batch_lines(["@SET PATH=C:\\BIN", "@FINDIT"]);
    run_batch_files(&mut cpu);
    assert_eq!(cpu.errorlevel, 7, "a program found on the PATH");
    assert!(cpu.secondary_shells.is_empty());
}

#[test]
fn command_com_without_c_is_a_prompt_until_exit() {
    let dir = scratch("command_prompt", &[]);
    let mut cpu = machine(&dir);
    type_line(&mut cpu, b"z:\\command\r");
    assert!(screen(&cpu).contains("Type EXIT to go back."), "{}", screen(&cpu));
    type_line(&mut cpu, b"echo inside\r");
    assert!(!cpu.shell_idle(), "still in COMMAND.COM");
    type_line(&mut cpu, b"exit\r");
    run_until(&mut cpu, 200, |cpu| cpu.shell_idle());
    assert!(cpu.shell_idle());
    assert!(cpu.secondary_shells.is_empty());
}
