//! Messages one Windows 3.1 task sends another's window: when the
//! receiver replies, the sender goes on before the receiver does, as
//! Windows 95 switches tasks there. The programs are made byte by byte
//! (tests/common/win16).

use crate::win16;

use std::fs;
use win16::{CODE, Code, program, program_named, run_with};

/// KERNEL's ordinals.
const YIELD: u16 = 29;
const GET_MODULE_HANDLE: u16 = 47;
const DIRECTED_YIELD: u16 = 150;
const WIN_EXEC: u16 = 166;

/// USER's ordinals.
const POST_QUIT_MESSAGE: u16 = 6;
const CREATE_WINDOW: u16 = 41;
const FIND_WINDOW: u16 = 50;
const GET_MESSAGE: u16 = 108;
const PEEK_MESSAGE: u16 = 109;
const POST_MESSAGE: u16 = 110;
const SEND_MESSAGE: u16 = 111;
const GET_WINDOW_TASK: u16 = 224;

const WM_USER: u16 = 0x0400;
const PM_REMOVE_NOYIELD: u16 = 3;

/// A program's code, and the strings that go after it.
#[derive(Default)]
struct Program {
    code: Code,
    /// Where the code pushes a string's offset, and the string.
    strings: Vec<(u16, &'static [u8])>,
}

impl Program {
    /// Push the far pointer to `text` (NUL-terminated).
    fn push_string(&mut self, text: &'static [u8]) {
        self.strings.push((self.code.here() + 2, text));
        self.code.push_far(0);
    }

    /// A hidden STATIC window titled `title`, in AX.
    fn create_window(&mut self, title: &'static [u8]) {
        self.push_string(b"STATIC\0");
        self.push_string(title);
        self.code
            .push(0x8000) // WS_POPUP
            .push(0);
        for v in [0, 0, 10, 10] {
            self.code.push(v);
        }
        self.code
            .push(0) // no parent
            .push(0) // no menu
            .push(0) // instance
            .push(0)
            .push(0) // no parameter
            .call("USER", CREATE_WINDOW);
    }

    /// FindWindow("STATIC", `title`), in AX.
    fn find_window(&mut self, title: &'static [u8]) {
        self.push_string(b"STATIC\0");
        self.push_string(title);
        self.code.call("USER", FIND_WINDOW);
    }

    /// Room for a MSG on the stack, its offset in DI.
    fn msg_space(&mut self) {
        self.code.emit(&[
            0x83, 0xEC, 0x12, // sub sp,18
            0x8B, 0xFC, // mov di,sp
        ]);
    }

    /// Push the far pointer to the MSG at SS:DI.
    fn push_msg(&mut self) {
        self.code.emit(&[0x16, 0x57]); // push ss; push di
    }

    /// The program's file, named `name` when it is the second program.
    fn build(mut self, name: Option<&str>) -> Vec<u8> {
        for (at, text) in self.strings {
            let offset = self.code.here();
            self.code.emit(text);
            self.code.patch(at, offset);
        }
        match name {
            Some(name) => program_named(name, &self.code, CODE),
            None => program(&self.code, CODE),
        }
    }
}

/// CHILD: makes the window QB, waits for a message, sends WM_USER to the
/// window QR and ends with exit code 7 when the send returns.
fn child() -> Vec<u8> {
    let mut p = Program::default();
    p.create_window(b"QB\0");
    p.msg_space();
    p.push_msg();
    p.code.push(0).push(0).push(0).call("USER", GET_MESSAGE);
    p.find_window(b"QR\0");
    p.code
        .emit(&[0x50]) // push ax
        .push(WM_USER)
        .push(0)
        .push(0)
        .push(0)
        .call("USER", SEND_MESSAGE)
        .emit(&[0xB0, 0x07]) // mov al,7
        .exit_al();
    p.build(Some("CHILD"))
}

/// The program: makes the window QR, starts CHILD and waits for its
/// window, posts it a message and yields to it with DirectedYield. CHILD
/// sends its message to QR and waits for the reply, and the program comes
/// back from DirectedYield with that message waiting. `receive` then
/// handles it: it replies to CHILD. The program ends with exit code 1
/// when CHILD has ended by the time `receive` returns, 2 when it is still
/// there.
fn program_receiving(receive: impl FnOnce(&mut Program)) -> Vec<u8> {
    let mut p = Program::default();
    p.create_window(b"QR\0");
    p.push_string(b"C:\\CHILD.EXE\0");
    p.code.push(1).call("KERNEL", WIN_EXEC);
    // Until CHILD's window is there, Yield.
    let wait = p.code.here();
    p.find_window(b"QB\0");
    p.code.emit(&[
        0x0B, 0xC0, // or ax,ax
        0x75, 0x07, // jnz found
    ]);
    p.code.call("KERNEL", YIELD);
    let back = wait.wrapping_sub(p.code.here() + 2) as u8;
    p.code.emit(&[0xEB, back]); // jmp wait
    p.code
        .emit(&[0x8B, 0xF0]) // found: mov si,ax
        .emit(&[0x56]) // push si
        .push(WM_USER)
        .push(0)
        .push(0)
        .push(0)
        .call("USER", POST_MESSAGE)
        .emit(&[0x56]) // push si
        .call("USER", GET_WINDOW_TASK)
        .emit(&[0x50]) // push ax
        .call("KERNEL", DIRECTED_YIELD);
    p.msg_space();
    receive(&mut p);
    p.push_string(b"CHILD\0");
    p.code.call("KERNEL", GET_MODULE_HANDLE).emit(&[
        0x0B, 0xC0, // or ax,ax
        0xB0, 0x01, // mov al,1
        0x74, 0x02, // jz done
        0xB0, 0x02, // mov al,2
    ]);
    p.code.exit_al();
    p.build(None)
}

/// Run `program_receiving(receive)`: the program's exit code, from the
/// log, as the run gives the code of the task that ended last, and the
/// log.
fn run_receiving(test: &str, receive: impl FnOnce(&mut Program)) -> (u8, String) {
    let (_, log) = run_with(test, program_receiving(receive), |_, base| {
        fs::write(base.join("CHILD.EXE"), child()).unwrap();
    });
    let ended = |code: u8| {
        let line = format!(" ended with exit code {code:X}");
        log.lines()
            .any(|l| l.starts_with("[W16] Task ") && l.ends_with(&line))
    };
    assert!(ended(7), "CHILD didn't end: {log}");
    let exit = [1, 2].into_iter().find(|&c| ended(c)).unwrap_or(0);
    (exit, log)
}

#[test]
fn a_reply_in_peek_message_lets_the_sender_run_before_it_returns() {
    // Windows 95: the task that replies in PeekMessage switches to the
    // sender, which runs until it yields or waits (here, to its end),
    // and only then does PeekMessage return.
    let (exit, log) = run_receiving("reply_in_peek", |p| {
        p.push_msg();
        p.code
            .push(0)
            .push(0)
            .push(0)
            .push(PM_REMOVE_NOYIELD)
            .call("USER", PEEK_MESSAGE);
    });
    assert_eq!(exit, 1, "{log}");
}

#[test]
fn a_reply_in_the_get_message_that_gets_wm_quit_lets_the_sender_run_first() {
    // Memphis Math's shell, told to quit, replies in the GetMessage that
    // gets its WM_QUIT to a message its log-in task sent while it closed
    // its files. On Windows 95 the log-in task finishes and ends before
    // that GetMessage returns; the shell then tells the tasks still there
    // to close their files, and the log-in task isn't one of them.
    let (exit, log) = run_receiving("reply_in_get_quit", |p| {
        p.code.push(0).call("USER", POST_QUIT_MESSAGE);
        p.push_msg();
        p.code.push(0).push(0).push(0).call("USER", GET_MESSAGE);
    });
    assert_eq!(exit, 1, "{log}");
}
