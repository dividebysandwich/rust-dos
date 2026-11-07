//! Win16's serial ports: USER's comm functions (OpenComm to FlushComm)
//! on the machine's UARTs, and GDI's GetAspectRatioFilter. The programs
//! are made byte by byte (tests/common/win16); each ends with exit code
//! 100 when every check passed, or with the number of the check that
//! failed.

use crate::win16;

use rust_dos::serial::SerialSettings;
use win16::{CODE, Code, program, run_with};

/// USER's ordinals.
const GET_CURRENT_TIME: u16 = 15;
const GET_DC: u16 = 66;
const OPEN_COMM: u16 = 200;
const SET_COMM_STATE: u16 = 201;
const GET_COMM_STATE: u16 = 202;
const GET_COMM_ERROR: u16 = 203;
const READ_COMM: u16 = 204;
const WRITE_COMM: u16 = 205;
const CLOSE_COMM: u16 = 207;
const SET_COMM_EVENT_MASK: u16 = 208;
const GET_COMM_EVENT_MASK: u16 = 209;
const BUILD_COMM_DCB: u16 = 213;
const ESCAPE_COMM_FUNCTION: u16 = 214;
const FLUSH_COMM: u16 = 215;

/// GDI's ordinals.
const SET_MAPPER_FLAGS: u16 = 349;
const GET_ASPECT_RATIO_FILTER: u16 = 353;

/// OpenComm's errors, as words.
const IE_BADID: u16 = 0xFFFF;
const IE_OPEN: u16 = 0xFFFE;
const IE_HARDWARE: u16 = 0xFFF6;

const EV_RXCHAR: u16 = 0x0001;
const EV_TXEMPTY: u16 = 0x0004;

/// EscapeCommFunction's functions.
const SETXOFF: u16 = 1;
const SETXON: u16 = 2;
const CLRDTR: u16 = 6;
const GETMAXCOM: u16 = 9;
const GETBASEIRQ: u16 = 10;

/// The exit code of a program whose checks all passed.
const PASSED: u8 = 100;

/// A program's code, and the strings that go after it.
#[derive(Default)]
struct Program {
    code: Code,
    /// Where the code pushes a string's offset, and the string.
    strings: Vec<(u16, &'static [u8])>,
}

impl Program {
    /// Push the far pointer to `text` (NUL-terminated) in the code
    /// segment.
    fn push_string(&mut self, text: &'static [u8]) -> &mut Code {
        self.strings.push((self.code.here() + 2, text));
        self.code.push_far(0)
    }

    /// Push the far pointer to `offset` in the data segment.
    fn push_data(&mut self, offset: u16) -> &mut Code {
        self.code.emit(&[0x1E]).push(offset) // push ds
    }

    /// The program's file: the code, then the strings.
    fn build(mut self) -> Vec<u8> {
        for (at, text) in self.strings {
            let offset = self.code.here();
            self.code.emit(text);
            self.code.patch(at, offset);
        }
        program(&self.code, CODE)
    }
}

/// After a compare: end with exit code `exit` unless it found the two
/// equal.
fn unless_equal(code: &mut Code, exit: u8) {
    code.emit(&[
        0x74, 0x06, // je over the rest
        0xB0, exit, // mov al,exit
    ])
    .exit_al();
}

/// End with exit code `exit` unless AX is `value`.
fn expect_ax(code: &mut Code, value: u16, exit: u8) {
    let [lo, hi] = value.to_le_bytes();
    code.emit(&[0x3D, lo, hi]); // cmp ax,value
    unless_equal(code, exit);
}

/// End with exit code `exit` unless DX is `value`.
fn expect_dx(code: &mut Code, value: u16, exit: u8) {
    let [lo, hi] = value.to_le_bytes();
    code.emit(&[0x81, 0xFA, lo, hi]); // cmp dx,value
    unless_equal(code, exit);
}

/// End with exit code `exit` unless the word at DS:`at` is `value`.
fn expect_word(code: &mut Code, at: u16, value: u16, exit: u8) {
    let [a, b] = at.to_le_bytes();
    let [lo, hi] = value.to_le_bytes();
    code.emit(&[0x81, 0x3E, a, b, lo, hi]); // cmp word [at],value
    unless_equal(code, exit);
}

/// End with exit code `exit` unless the byte at DS:`at` is `value`.
fn expect_byte(code: &mut Code, at: u16, value: u8, exit: u8) {
    let [a, b] = at.to_le_bytes();
    code.emit(&[0x80, 0x3E, a, b, value]); // cmp byte [at],value
    unless_equal(code, exit);
}

/// The relative offset of a short jump at `from` (its first byte) to
/// `to`.
fn rel8(from: u16, to: u16) -> u8 {
    let rel = to as i32 - (from as i32 + 2);
    assert!((-128..128).contains(&rel), "a short jump of {rel}");
    rel as i8 as u8
}

/// Run the program with the default serial ports: a mouse on COM1, a
/// modem on COM2, no COM3 or COM4.
fn run(test: &str, p: Program) -> (u8, String) {
    run_with(test, p.build(), |cpu, _| {
        cpu.bus.configure_serial(&SerialSettings::default())
    })
}

#[test]
fn open_comm_gives_windows_errors_and_the_port_id() {
    let mut p = Program::default();
    // A port the machine doesn't have, and a name that isn't a port's.
    p.push_string(b"COM3\0");
    p.code.push(64).push(64).call("USER", OPEN_COMM);
    expect_ax(&mut p.code, IE_HARDWARE, 1);
    p.push_string(b"XYZ1\0");
    p.code.push(64).push(64).call("USER", OPEN_COMM);
    expect_ax(&mut p.code, IE_BADID, 2);
    // COM2 opens once, as id 1.
    p.push_string(b"COM2\0");
    p.code.push(64).push(64).call("USER", OPEN_COMM);
    expect_ax(&mut p.code, 1, 3);
    p.push_string(b"com2:\0");
    p.code.push(64).push(64).call("USER", OPEN_COMM);
    expect_ax(&mut p.code, IE_OPEN, 4);
    p.code.push(1).call("USER", CLOSE_COMM);
    expect_ax(&mut p.code, 0, 5);
    p.code.push(1).call("USER", CLOSE_COMM);
    expect_ax(&mut p.code, 0xFFFF, 6);
    p.code.emit(&[0xB0, PASSED]).exit_al();
    let (exit, log) = run("comm_open", p);
    assert_eq!(exit, PASSED, "{log}");
}

#[test]
fn a_modem_answers_through_write_comm_and_read_comm() {
    // "AT" and a return to the modem on COM2, which echoes them and
    // answers "OK": nine characters back. The event word SetCommEventMask
    // points to gets EV_RXCHAR while the program calls nothing but
    // GetCurrentTime, as COMM.DRV's interrupts would set it.
    let mut p = Program::default();
    p.push_string(b"COM2\0");
    p.code.push(256).push(256).call("USER", OPEN_COMM);
    expect_ax(&mut p.code, 1, 1);
    p.push_string(b"COM2:9600,n,8,1\0");
    p.push_data(0x40).call("USER", BUILD_COMM_DCB);
    expect_ax(&mut p.code, 0, 2);
    p.push_data(0x40).call("USER", SET_COMM_STATE);
    expect_ax(&mut p.code, 0, 3);
    p.code
        .push(1)
        .push(EV_RXCHAR | EV_TXEMPTY)
        .call("USER", SET_COMM_EVENT_MASK)
        .emit(&[
            0xA3, 0x10, 0x00, // mov [10h],ax
            0x89, 0x16, 0x12, 0x00, // mov [12h],dx
            0x0B, 0xD2, // or dx,dx
            0x75, 0x06, // jnz over the rest
            0xB0, 0x04, // mov al,4
        ])
        .exit_al();
    p.code.push(1);
    p.push_string(b"AT\r");
    p.code.push(3).call("USER", WRITE_COMM);
    expect_ax(&mut p.code, 3, 5);
    // Wait for EV_RXCHAR, at most 65536 times round.
    p.code.emit(&[0xC7, 0x06, 0x14, 0x00, 0x00, 0x00]); // mov word [14h],0
    let wait = p.code.here();
    p.code.call("USER", GET_CURRENT_TIME).emit(&[
        0xC4, 0x1E, 0x10, 0x00, // les bx,[10h]
        0x26, 0xF6, 0x07, 0x01, // test byte es:[bx],1
        0x75, 0x0C, // jnz over the rest
        0xFF, 0x0E, 0x14, 0x00, // dec word [14h]
    ]);
    let back = rel8(p.code.here(), wait);
    p.code.emit(&[0x75, back, 0xB0, 0x06]).exit_al(); // jnz wait; mov al,6
    // Wait for the nine characters in the receive queue (COMSTAT's
    // cbInQue at 31h).
    p.code.emit(&[0xC7, 0x06, 0x14, 0x00, 0x00, 0x00]); // mov word [14h],0
    let wait = p.code.here();
    p.code.push(1);
    p.push_data(0x30).call("USER", GET_COMM_ERROR).emit(&[
        0x83, 0x3E, 0x31, 0x00, 0x09, // cmp word [31h],9
        0x73, 0x0C, // jae over the rest
        0xFF, 0x0E, 0x14, 0x00, // dec word [14h]
    ]);
    let back = rel8(p.code.here(), wait);
    p.code.emit(&[0x75, back, 0xB0, 0x07]).exit_al(); // jnz wait; mov al,7
    p.code.push(1);
    p.push_data(0x50).push(32).call("USER", READ_COMM);
    expect_ax(&mut p.code, 9, 8);
    // "AT\r\r\nOK\r\n": the answer at 55h.
    expect_word(&mut p.code, 0x55, u16::from_le_bytes(*b"OK"), 9);
    // GetCommEventMask gives the events and clears them in the word.
    p.code
        .push(1)
        .push(0xFFFF)
        .call("USER", GET_COMM_EVENT_MASK)
        .emit(&[0x25])
        .emit(&(EV_RXCHAR | EV_TXEMPTY).to_le_bytes()); // and ax,...
    expect_ax(&mut p.code, EV_RXCHAR | EV_TXEMPTY, 10);
    p.code.emit(&[
        0xC4, 0x1E, 0x10, 0x00, // les bx,[10h]
        0x26, 0x83, 0x3F, 0x00, // cmp word es:[bx],0
    ]);
    unless_equal(&mut p.code, 11);
    p.code.push(1).call("USER", CLOSE_COMM);
    expect_ax(&mut p.code, 0, 12);
    p.code.emit(&[0xB0, PASSED]).exit_al();
    let (exit, log) = run("comm_modem", p);
    assert_eq!(exit, PASSED, "{log}");
}

#[test]
fn comm_state_escapes_and_held_transmission() {
    let mut p = Program::default();
    p.push_string(b"COM2\0");
    p.code.push(256).push(256).call("USER", OPEN_COMM);
    expect_ax(&mut p.code, 1, 1);
    // GetCommState gives the DCB SetCommState set.
    p.push_string(b"COM2:1200,e,7,2\0");
    p.push_data(0x40).call("USER", BUILD_COMM_DCB);
    expect_ax(&mut p.code, 0, 2);
    p.push_data(0x40).call("USER", SET_COMM_STATE);
    expect_ax(&mut p.code, 0, 3);
    p.code.push(1);
    p.push_data(0x60).call("USER", GET_COMM_STATE);
    expect_ax(&mut p.code, 0, 4);
    expect_word(&mut p.code, 0x61, 1200, 5);
    expect_word(&mut p.code, 0x63, 0x0207, 6); // seven bits, even parity
    expect_byte(&mut p.code, 0x65, 2, 7); // two stop bits
    // A definition BuildCommDCB can't read.
    p.push_string(b"COM2:9600,n,8\0");
    p.push_data(0x40).call("USER", BUILD_COMM_DCB);
    expect_ax(&mut p.code, 0xFFFF, 8);
    // COM2's base address and IRQ, and the highest COM port id.
    p.code
        .push(1)
        .push(GETBASEIRQ)
        .call("USER", ESCAPE_COMM_FUNCTION);
    expect_ax(&mut p.code, 0x2F8, 9);
    expect_dx(&mut p.code, 3, 10);
    p.code
        .push(1)
        .push(GETMAXCOM)
        .call("USER", ESCAPE_COMM_FUNCTION);
    expect_ax(&mut p.code, 3, 11);
    p.code
        .push(1)
        .push(CLRDTR)
        .call("USER", ESCAPE_COMM_FUNCTION);
    expect_ax(&mut p.code, 0, 12);
    // SETXOFF holds what WriteComm queues: COMSTAT says so, with the
    // three characters in cbOutQue.
    p.code
        .push(1)
        .push(SETXOFF)
        .call("USER", ESCAPE_COMM_FUNCTION);
    expect_ax(&mut p.code, 0, 13);
    p.code.push(1);
    p.push_string(b"AT\r");
    p.code.push(3).call("USER", WRITE_COMM);
    expect_ax(&mut p.code, 3, 14);
    p.code.push(1);
    p.push_data(0x30).call("USER", GET_COMM_ERROR);
    expect_ax(&mut p.code, 0, 15);
    expect_byte(&mut p.code, 0x30, 0x08, 16); // CSTF_XOFFHOLD
    expect_word(&mut p.code, 0x33, 3, 17);
    p.code
        .push(1)
        .push(SETXON)
        .call("USER", ESCAPE_COMM_FUNCTION);
    expect_ax(&mut p.code, 0, 18);
    // The receive queue flushes; there is no queue 7.
    p.code.push(1).push(1).call("USER", FLUSH_COMM);
    expect_ax(&mut p.code, 0, 19);
    p.code.push(1).push(7).call("USER", FLUSH_COMM);
    expect_ax(&mut p.code, 0xFFFF, 20);
    p.code.push(1).call("USER", CLOSE_COMM);
    expect_ax(&mut p.code, 0, 21);
    p.code.emit(&[0xB0, PASSED]).exit_al();
    let (exit, log) = run("comm_state", p);
    assert_eq!(exit, PASSED, "{log}");
}

#[test]
fn get_aspect_ratio_filter_follows_the_mapper_flags() {
    // 0 while ASPECT_FILTERING is off; the display's 96 by 96 pixels an
    // inch once SetMapperFlags turns it on.
    let mut p = Program::default();
    p.code
        .push(0)
        .call("USER", GET_DC)
        .emit(&[0x8B, 0xF0, 0x56]) // mov si,ax; push si
        .call("GDI", GET_ASPECT_RATIO_FILTER);
    expect_ax(&mut p.code, 0, 1);
    expect_dx(&mut p.code, 0, 2);
    p.code
        .emit(&[0x56]) // push si
        .push(0)
        .push(1) // ASPECT_FILTERING
        .call("GDI", SET_MAPPER_FLAGS)
        .emit(&[0x56]) // push si
        .call("GDI", GET_ASPECT_RATIO_FILTER);
    expect_ax(&mut p.code, 96, 3);
    expect_dx(&mut p.code, 96, 4);
    p.code.emit(&[0xB0, PASSED]).exit_al();
    let (exit, log) = run("aspect_filter", p);
    assert_eq!(exit, PASSED, "{log}");
}
