//! `rust-dos run`: DOS programs and commands run as tools of the host, their
//! console output on its standard output and error, its standard input
//! typed at their keyboard, and their exit code its.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

fn scratch(name: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let dir = PathBuf::from("target/test_cli").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (file, bytes) in files {
        std::fs::write(dir.join(file), bytes).unwrap();
    }
    dir
}

/// `rust-dos run` with `args` in `dir`, given `input` on standard input.
fn run(dir: &PathBuf, args: &[&str], input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rust-dos"))
        .arg("run")
        .arg("-d")
        .arg(dir)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

#[rustfmt::skip]
/// Prints "Hello DOS" with AH=09h and exits with code 3.
const HELLO: &[u8] = &[
    0xB4, 0x09,             // MOV AH, 09h
    0xBA, 0x0C, 0x01,       // MOV DX, 010Ch
    0xCD, 0x21,             // INT 21h
    0xB8, 0x03, 0x4C,       // MOV AX, 4C03h
    0xCD, 0x21,             // INT 21h
    b'H', b'e', b'l', b'l', b'o', b' ', b'D', b'O', b'S', b'\r', b'\n', b'$',
];

#[rustfmt::skip]
/// Writes "oops" to handle 2 with AH=40h.
const OOPS: &[u8] = &[
    0xB4, 0x40,             // MOV AH, 40h
    0xBB, 0x02, 0x00,       // MOV BX, 2
    0xB9, 0x04, 0x00,       // MOV CX, 4
    0xBA, 0x12, 0x01,       // MOV DX, 0112h
    0xCD, 0x21,             // INT 21h
    0xB8, 0x00, 0x4C,       // MOV AX, 4C00h
    0xCD, 0x21,             // INT 21h
    b'o', b'o', b'p', b's',
];

#[rustfmt::skip]
/// Copies handle 0 to handle 1 until a read gets nothing.
const CAT: &[u8] = &[
    0xB4, 0x3F,             // read: MOV AH, 3Fh
    0x31, 0xDB,             // XOR BX, BX
    0xB9, 0x80, 0x00,       // MOV CX, 80h
    0xBA, 0x20, 0x01,       // MOV DX, 0120h
    0xCD, 0x21,             // INT 21h
    0x09, 0xC0,             // OR AX, AX
    0x74, 0x0B,             // JZ done
    0x89, 0xC1,             // MOV CX, AX
    0xB4, 0x40,             // MOV AH, 40h
    0xBB, 0x01, 0x00,       // MOV BX, 1
    0xCD, 0x21,             // INT 21h
    0xEB, 0xE5,             // JMP read
    0xB8, 0x00, 0x4C,       // done: MOV AX, 4C00h
    0xCD, 0x21,             // INT 21h
];

#[rustfmt::skip]
/// Waits for a key with INT 16h, again and again.
const WAIT_KEY: &[u8] = &[
    0xB4, 0x00,             // MOV AH, 00h
    0xCD, 0x16,             // INT 16h
    0xEB, 0xFA,             // JMP $-4
];

#[rustfmt::skip]
/// Prints "OK" with INT 29h.
const FAST: &[u8] = &[
    0xB0, b'O', 0xCD, 0x29, // MOV AL, 'O' ; INT 29h
    0xB0, b'K', 0xCD, 0x29, // MOV AL, 'K' ; INT 29h
    0xCD, 0x20,             // INT 20h
];

#[test]
fn a_program_s_output_and_exit_code_are_the_run_s() {
    let dir = scratch("hello", &[("HELLO.COM", HELLO)]);
    let output = run(&dir, &["HELLO"], b"");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "Hello DOS\r\n");
    assert_eq!(output.status.code(), Some(3));
}

#[test]
fn handle_2_goes_to_standard_error() {
    let dir = scratch("stderr", &[("OOPS.COM", OOPS)]);
    let output = run(&dir, &["OOPS"], b"");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "oops");
    assert!(output.stdout.is_empty());
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn standard_input_is_read_to_its_end() {
    let dir = scratch("cat", &[("CAT.COM", CAT)]);
    let output = run(&dir, &["CAT"], b"one\ntwo");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "one\r\ntwo\r\n");
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn a_program_waiting_for_keys_after_the_input_ends_is_stopped() {
    let dir = scratch("wait", &[("WAITKEY.COM", WAIT_KEY)]);
    let output = run(&dir, &["WAITKEY"], b"ab");
    assert_eq!(output.status.code(), Some(255));
    assert!(String::from_utf8_lossy(&output.stderr).contains("waits for input"));
}

#[test]
fn int_29h_prints() {
    let dir = scratch("int29", &[("FAST.COM", FAST)]);
    let output = run(&dir, &["FAST"], b"");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "OK");
}

#[test]
fn built_in_commands_run_and_a_missing_program_fails() {
    let dir = scratch("builtin", &[("HELLO.COM", HELLO)]);
    let output = run(&dir, &["DIR"], b"");
    assert!(String::from_utf8_lossy(&output.stdout).contains("HELLO    COM"));
    assert_eq!(output.status.code(), Some(0));

    let output = run(&dir, &["NOSUCH"], b"");
    assert!(String::from_utf8_lossy(&output.stderr).contains("Bad command or file name"));
    assert!(output.stdout.is_empty());
    assert_eq!(output.status.code(), Some(127));

    let output = run(&dir, &["CHOICE", "/C:YN"], b"n");
    assert_eq!(output.status.code(), Some(2));
}
