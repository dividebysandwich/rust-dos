//! EDIT, the built-in text editor: opening and saving files through its
//! menus and dialogs, selecting, copying and pasting, Find and Replace,
//! and a save state taken while it runs.

use rust_dos::cpu::Cpu;
use rust_dos::exec::{self, NoHook};
use rust_dos::savestate::machine;
use rust_dos::shell::ShellWait;
use std::fs;
use std::path::{Path, PathBuf};

fn scratch(name: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let base = PathBuf::from("target/test_edit").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    for (file, bytes) in files {
        let path = base.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    base
}

fn machine(dir: &Path) -> Cpu {
    let mut cpu = Cpu::new(dir.to_path_buf());
    cpu.bus.set_cycles_per_ms(1000);
    cpu.load_shell();
    cpu
}

fn run(cpu: &mut Cpu, ms: u64) {
    for _ in 0..ms {
        let end = cpu.bus.clock.icount + 1000;
        cpu.bus.start_batch(end);
        exec::run_batch(cpu, &mut NoHook, false);
    }
}

fn editing(cpu: &Cpu) -> bool {
    matches!(cpu.shell_wait, Some(ShellWait::Edit(_)))
}

/// Start EDIT with `args`, and wait for it.
fn edit(cpu: &mut Cpu, args: &str) {
    cpu.queue_batch_lines([format!("EDIT {}", args)]);
    for _ in 0..200 {
        run(cpu, 5);
        if editing(cpu) {
            return;
        }
    }
    panic!("EDIT starts:\n{}", screen(cpu));
}

/// Type keystrokes (scan code << 8 | character), and let EDIT take them.
fn keys(cpu: &mut Cpu, keys: &[u16]) {
    for &key in keys {
        cpu.bus.keyboard_buffer.push_back(key);
        run(cpu, 30);
    }
}

fn text(cpu: &mut Cpu, text: &str) {
    let keys_: Vec<u16> = text.bytes().map(|b| if b == b'\r' { 0x1C0D } else { b as u16 }).collect();
    keys(cpu, &keys_);
}

/// The same keys with Shift held.
fn shifted(cpu: &mut Cpu, k: &[u16]) {
    let flags = cpu.bus.read_8(0x0417);
    cpu.bus.write_8(0x0417, flags | 0x02);
    keys(cpu, k);
    cpu.bus.write_8(0x0417, flags);
}

const ALT_F: u16 = 0x2100;
const ALT_S: u16 = 0x1F00;
const ENTER: u16 = 0x1C0D;
const TAB: u16 = 0x0F09;
const DOWN: u16 = 0x50E0;
const END: u16 = 0x4FE0;
const HOME: u16 = 0x47E0;
const CTRL_C: u16 = 0x2E03;
const CTRL_V: u16 = 0x2F16;
const F3: u16 = 0x3D00;

fn rows(cpu: &Cpu) -> Vec<String> {
    cpu.bus
        .vga
        .vram_text
        .chunks(160)
        .take(25)
        .map(|row| row.iter().step_by(2).map(|&b| if b == 0 { ' ' } else if b < 0x80 { b as char } else { '#' }).collect::<String>())
        .collect()
}

fn screen(cpu: &Cpu) -> String {
    rows(cpu).join("\n")
}

fn status(cpu: &Cpu) -> String {
    rows(cpu)[24].clone()
}

/// Save with File, Save, then leave with File, Exit.
fn save_and_exit(cpu: &mut Cpu) {
    keys(cpu, &[ALT_F, b's' as u16]);
    keys(cpu, &[ALT_F, b'x' as u16]);
    assert!(!editing(cpu), "EDIT ends:\n{}", screen(cpu));
}

#[test]
fn opens_a_file_and_shows_it() {
    let dir = scratch("open", &[("NOTES.TXT", b"hello\r\n\tworld\r\n")]);
    let mut cpu = machine(&dir);
    edit(&mut cpu, "notes.txt");
    let rows = rows(&cpu);
    assert!(rows[0].contains("File  Edit  Search"), "{}", rows[0]);
    assert!(rows[0].trim_end().ends_with("Help"), "{}", rows[0]);
    assert!(rows[1].contains(" NOTES.TXT "), "{}", rows[1]);
    assert!(rows[2][1..].starts_with("hello"), "{}", rows[2]);
    assert!(rows[3][1..].starts_with("        world"), "tabs expanded: {}", rows[3]);
    assert!(status(&cpu).contains("00001:001"), "{}", status(&cpu));
}

#[test]
fn types_saves_and_exits() {
    let dir = scratch("save", &[]);
    let mut cpu = machine(&dir);
    cpu.queue_batch_lines(["ECHO before"]);
    run(&mut cpu, 50);
    edit(&mut cpu, "NEW.TXT");
    text(&mut cpu, "abc\rdef");
    assert!(status(&cpu).contains("00002:004"), "{}", status(&cpu));
    save_and_exit(&mut cpu);
    assert_eq!(fs::read(dir.join("NEW.TXT")).unwrap(), b"abc\r\ndef");
    // The screen as it was, and the prompt after it.
    run(&mut cpu, 50);
    let text = screen(&cpu);
    assert!(text.contains("before"), "{}", text);
    assert!(text.contains("C:\\>EDIT NEW.TXT"), "{}", text);
    assert!(!text.contains("Search"), "{}", text);
}

#[test]
fn asks_to_save_changes_on_exit() {
    let dir = scratch("unsaved", &[]);
    let mut cpu = machine(&dir);
    edit(&mut cpu, "NEW.TXT");
    text(&mut cpu, "x");
    keys(&mut cpu, &[ALT_F, b'x' as u16]);
    assert!(editing(&cpu));
    assert!(screen(&cpu).contains("Loaded file is not saved. Save it now?"), "{}", screen(&cpu));
    keys(&mut cpu, &[b'n' as u16]);
    assert!(!editing(&cpu));
    assert!(!dir.join("NEW.TXT").exists());

    // Yes on an untitled text asks for a name, then saves and ends.
    edit(&mut cpu, "");
    text(&mut cpu, "y");
    keys(&mut cpu, &[ALT_F, b'x' as u16, b'y' as u16]);
    assert!(screen(&cpu).contains("Save As"), "{}", screen(&cpu));
    text(&mut cpu, "NAMED.TXT\r");
    assert!(!editing(&cpu), "{}", screen(&cpu));
    assert_eq!(fs::read(dir.join("NAMED.TXT")).unwrap(), b"y");
}

#[test]
fn selects_copies_and_pastes() {
    let dir = scratch("clip", &[("A.TXT", b"one two\r\nthree")]);
    let mut cpu = machine(&dir);
    edit(&mut cpu, "A.TXT");
    shifted(&mut cpu, &[END]);
    keys(&mut cpu, &[CTRL_C, DOWN, END, CTRL_V]);
    save_and_exit(&mut cpu);
    assert_eq!(fs::read(dir.join("A.TXT")).unwrap(), b"one two\r\nthreeone two");

    // A selection over lines, cut with Shift+Del and pasted at the end.
    edit(&mut cpu, "A.TXT");
    keys(&mut cpu, &[0x4DE0, 0x4DE0, 0x4DE0, 0x4DE0]);
    shifted(&mut cpu, &[DOWN, 0x53E0]);
    keys(&mut cpu, &[0x75E0]);
    shifted(&mut cpu, &[0x52E0]);
    save_and_exit(&mut cpu);
    assert_eq!(fs::read(dir.join("A.TXT")).unwrap(), b"one eone twotwo\r\nthre");
}

#[test]
fn finds_and_repeats() {
    let dir = scratch("find", &[("A.TXT", b"cat\r\ndog\r\nconcat cat")]);
    let mut cpu = machine(&dir);
    edit(&mut cpu, "A.TXT");
    keys(&mut cpu, &[DOWN, ALT_S, b'f' as u16]);
    let text_ = screen(&cpu);
    assert!(text_.contains("Find What:"), "{}", text_);
    assert!(text_.contains("dog"), "the word at the cursor: {}", text_);
    // Whole words of "cat", from the cursor.
    for _ in 0..3 {
        keys(&mut cpu, &[0x0E08]);
    }
    text(&mut cpu, "cat");
    keys(&mut cpu, &[TAB, TAB, b' ' as u16, ENTER]);
    assert!(status(&cpu).contains("00003:011"), "{}", status(&cpu));
    keys(&mut cpu, &[F3]);
    assert!(status(&cpu).contains("00001:004"), "goes round: {}", status(&cpu));
    // Nothing to find.
    keys(&mut cpu, &[ALT_S, b'f' as u16]);
    text(&mut cpu, "zebra\r");
    assert!(screen(&cpu).contains("Match not found."), "{}", screen(&cpu));
    keys(&mut cpu, &[ENTER]);
    assert!(!screen(&cpu).contains("Match not found."));
}

#[test]
fn replaces_all_and_verifies() {
    let dir = scratch("replace", &[("A.TXT", b"a-b-a\r\nb a")]);
    let mut cpu = machine(&dir);
    edit(&mut cpu, "A.TXT");
    keys(&mut cpu, &[ALT_S, b'p' as u16]);
    assert!(screen(&cpu).contains("Replace With:"), "{}", screen(&cpu));
    keys(&mut cpu, &[TAB]);
    text(&mut cpu, "XY");
    // Tab to Replace All.
    keys(&mut cpu, &[TAB, TAB, TAB, TAB, ENTER]);
    assert!(screen(&cpu).contains("Replace complete."), "{}", screen(&cpu));
    keys(&mut cpu, &[ENTER]);

    // Find and Verify: replace the first, skip the second.
    keys(&mut cpu, &[0x7700, ALT_S, b'p' as u16]);
    for _ in 0..2 {
        keys(&mut cpu, &[0x0E08]);
    }
    text(&mut cpu, "b");
    keys(&mut cpu, &[TAB]);
    for _ in 0..2 {
        keys(&mut cpu, &[0x0E08]);
    }
    text(&mut cpu, "BB\r");
    assert!(screen(&cpu).contains("Replace this occurrence?"), "{}", screen(&cpu));
    keys(&mut cpu, &[b'r' as u16]);
    assert!(screen(&cpu).contains("Replace this occurrence?"), "{}", screen(&cpu));
    keys(&mut cpu, &[b's' as u16]);
    assert!(screen(&cpu).contains("Replace complete."), "{}", screen(&cpu));
    keys(&mut cpu, &[ENTER]);
    save_and_exit(&mut cpu);
    assert_eq!(fs::read(dir.join("A.TXT")).unwrap(), b"XY-BB-XY\r\nb XY");
}

#[test]
fn open_dialog_lists_files_and_directories() {
    let dir = scratch(
        "dialog",
        &[("A.TXT", b"a"), ("B.TXT", b"b"), ("D.DAT", b"d"), ("SUB/C.TXT", b"in sub")],
    );
    let mut cpu = machine(&dir);
    edit(&mut cpu, "");
    keys(&mut cpu, &[ALT_F, b'o' as u16]);
    let text_ = screen(&cpu);
    assert!(text_.contains(" Open "), "{}", text_);
    assert!(text_.contains("A.TXT") && text_.contains("B.TXT"), "{}", text_);
    assert!(!text_.contains("D.DAT"), "*.TXT only: {}", text_);
    assert!(text_.contains("SUB") && text_.contains("[-C-]"), "{}", text_);
    // Into SUB from the directories, then the file there.
    keys(&mut cpu, &[TAB, TAB, ENTER]);
    let text_ = screen(&cpu);
    assert!(text_.contains("C:\\SUB") && text_.contains("C.TXT") && text_.contains(".."), "{}", text_);
    keys(&mut cpu, &[0x0F00, ENTER]);
    let rows = rows(&cpu);
    assert!(rows[1].contains(" C.TXT "), "{}", screen(&cpu));
    assert!(rows[2][1..].starts_with("in sub"), "{}", screen(&cpu));
}

#[test]
fn a_save_state_keeps_the_editor() {
    let dir = scratch("state", &[]);
    let mut cpu = machine(&dir);
    edit(&mut cpu, "S.TXT");
    text(&mut cpu, "kept");
    let state = machine::save(&cpu);
    text(&mut cpu, " lost");
    machine::load(&mut cpu, &state).unwrap();
    run(&mut cpu, 100);
    assert!(editing(&cpu));
    assert!(rows(&cpu)[2][1..].starts_with("kept "), "{}", screen(&cpu));
    text(&mut cpu, "!");
    save_and_exit(&mut cpu);
    assert_eq!(fs::read(dir.join("S.TXT")).unwrap(), b"kept!");
}

#[test]
fn ctrl_c_copies_and_does_not_end_the_batch() {
    let dir = scratch("ctrlc", &[("A.TXT", b"x")]);
    let mut cpu = machine(&dir);
    cpu.queue_batch_lines(["EDIT A.TXT", "ECHO after"]);
    run(&mut cpu, 50);
    assert!(editing(&cpu));
    keys(&mut cpu, &[CTRL_C, HOME]);
    keys(&mut cpu, &[ALT_F, b'x' as u16]);
    run(&mut cpu, 100);
    assert!(screen(&cpu).contains("after"), "{}", screen(&cpu));
}

/// Click the left button on cell (`row`, `col`).
fn click(cpu: &mut Cpu, row: i32, col: i32) {
    cpu.bus.mouse.set_position(col * 8 + 4, row * 8 + 4);
    cpu.bus.mouse.button_down(0);
    run(cpu, 30);
    cpu.bus.mouse.button_up(0);
    run(cpu, 30);
}

#[test]
fn the_mouse_selects_and_uses_the_menus() {
    let dir = scratch("mouse", &[("A.TXT", b"hello world")]);
    let mut cpu = machine(&dir);
    edit(&mut cpu, "A.TXT");
    // Drag over "world".
    cpu.bus.mouse.set_position(7 * 8 + 4, 2 * 8 + 4);
    cpu.bus.mouse.button_down(0);
    run(&mut cpu, 30);
    cpu.bus.mouse.set_position(12 * 8 + 4, 2 * 8 + 4);
    run(&mut cpu, 30);
    cpu.bus.mouse.button_up(0);
    run(&mut cpu, 30);
    assert!(status(&cpu).contains("00001:012"), "{}", status(&cpu));
    // Edit, Cut; then File, Exit, and No to saving.
    click(&mut cpu, 0, 8);
    assert!(screen(&cpu).contains("Shift+Del"), "{}", screen(&cpu));
    click(&mut cpu, 2, 10);
    assert!(rows(&cpu)[2][1..].starts_with("hello  "), "{}", screen(&cpu));
    click(&mut cpu, 0, 3);
    click(&mut cpu, 7, 4);
    assert!(screen(&cpu).contains("Save it now?"), "{}", screen(&cpu));
    let row = rows(&cpu).iter().position(|r| r.contains("< No >")).unwrap();
    let col = rows(&cpu)[row].find("< No >").unwrap();
    click(&mut cpu, row as i32, col as i32 + 2);
    assert!(!editing(&cpu), "{}", screen(&cpu));
    assert_eq!(fs::read(dir.join("A.TXT")).unwrap(), b"hello world");
}
