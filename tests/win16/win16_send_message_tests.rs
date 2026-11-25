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
const REPLY_MESSAGE: u16 = 115;
const CALL_WINDOW_PROC: u16 = 122;
const GET_CLASS_LONG: u16 = 131;
const SET_WINDOW_LONG: u16 = 136;
const GET_WINDOW_TASK: u16 = 224;

const WM_USER: u16 = 0x0400;
const PM_REMOVE_NOYIELD: u16 = 3;
const GWL_WNDPROC: u16 = 0xFFFC;
const GCL_WNDPROC: u16 = 0xFFE8;

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

    /// WinExec(`path`), then Yield until the window titled `title` is
    /// there, which is in AX then.
    fn start_and_wait(&mut self, path: &'static [u8], title: &'static [u8]) {
        self.push_string(path);
        self.code.push(1).call("KERNEL", WIN_EXEC);
        let wait = self.code.here();
        self.find_window(title);
        self.code.emit(&[
            0x0B, 0xC0, // or ax,ax
            0x75, 0x07, // jnz found
        ]);
        self.code.call("KERNEL", YIELD);
        let back = wait.wrapping_sub(self.code.here() + 2) as u8;
        self.code.emit(&[0xEB, back]); // jmp wait
    }

    /// DirectedYield to the task of the window titled `title`.
    fn yield_to_window(&mut self, title: &'static [u8]) {
        self.find_window(title);
        self.code
            .emit(&[0x50]) // push ax
            .call("USER", GET_WINDOW_TASK)
            .emit(&[0x50]) // push ax
            .call("KERNEL", DIRECTED_YIELD);
    }

    /// Give the window titled `title` a 16-bit window procedure: for
    /// `message` it runs `body`, whose result is DX:AX, and other
    /// messages go on to the class's procedure.
    fn subclass(&mut self, title: &'static [u8], message: u16, body: impl FnOnce(&mut Program)) {
        self.find_window(title);
        self.code.emit(&[0x50]).push(GWL_WNDPROC); // push ax
        let proc_at = self.code.here() + 2;
        self.code.push_far(0).call("USER", SET_WINDOW_LONG);
        let over = self.code.here();
        self.code.emit(&[0xE9, 0x00, 0x00]); // jmp near over the procedure
        let proc = self.code.here();
        let [lo, hi] = message.to_le_bytes();
        self.code.emit(&[
            0x55, // push bp
            0x8B, 0xEC, // mov bp,sp
            0x81, 0x7E, 0x0C, lo, hi, // cmp word [bp+0Ch],message
            0x74, 0x03, // je mine
            0xE9, 0x00, 0x00, // jmp near pass
        ]);
        let pass_at = self.code.here() - 2;
        body(self);
        self.code.emit(&[
            0x5D, // pop bp
            0xCA, 0x0A, 0x00, // retf 0Ah
        ]);
        let pass = self.code.here();
        self.code.patch(pass_at, pass - (pass_at + 2));
        // CallWindowProc(the class's procedure, hwnd, msg, wParam,
        // lParam).
        self.code
            .emit(&[0xFF, 0x76, 0x0E]) // push [bp+0Eh] (hwnd)
            .push(GCL_WNDPROC)
            .call("USER", GET_CLASS_LONG)
            .emit(&[
                0x52, // push dx
                0x50, // push ax
                0xFF, 0x76, 0x0E, // push [bp+0Eh] (hwnd)
                0xFF, 0x76, 0x0C, // push [bp+0Ch] (msg)
                0xFF, 0x76, 0x0A, // push [bp+0Ah] (wParam)
                0xFF, 0x76, 0x08, // push [bp+8] (lParam)
                0xFF, 0x76, 0x06, // push [bp+6]
            ])
            .call("USER", CALL_WINDOW_PROC)
            .emit(&[
                0x5D, // pop bp
                0xCA, 0x0A, 0x00, // retf 0Ah
            ]);
        let end = self.code.here();
        self.code.patch(over + 1, end - (over + 3));
        self.code.patch(proc_at, proc);
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
    p.start_and_wait(b"C:\\CHILD.EXE\0", b"QB\0");
    p.code
        .emit(&[0x8B, 0xF0]) // mov si,ax
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

/// Run `program` with the other programs `others` (file name, program,
/// exit code) in its folder: the program's exit code, 1 or 2, from the
/// log, as the run gives the code of the task that ended last, and the
/// log. Each of the others must have ended with its exit code.
fn run_tasks(test: &str, program: Vec<u8>, others: Vec<(&str, Vec<u8>, u8)>) -> (u8, String) {
    let codes: Vec<(String, u8)> = others.iter().map(|(n, _, c)| (n.to_string(), *c)).collect();
    let (_, log) = run_with(test, program, |_, base| {
        for (name, bytes, _) in others {
            fs::write(base.join(name), bytes).unwrap();
        }
    });
    let ended = |code: u8| {
        let line = format!(" ended with exit code {code:X}");
        log.lines()
            .any(|l| l.starts_with("[W16] Task ") && l.ends_with(&line))
    };
    for (name, code) in codes {
        assert!(ended(code), "{name} didn't end: {log}");
    }
    let exit = [1, 2].into_iter().find(|&c| ended(c)).unwrap_or(0);
    (exit, log)
}

/// Run `program_receiving(receive)` with CHILD (`run_tasks`).
fn run_receiving(test: &str, receive: impl FnOnce(&mut Program)) -> (u8, String) {
    let child = ("CHILD.EXE", child(), 7);
    run_tasks(test, program_receiving(receive), vec![child])
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

#[test]
fn reply_message_lets_the_sender_run_before_it_returns() {
    // Windows 95: a window procedure that calls ReplyMessage for a
    // message another task sent switches to the sender there, and the
    // sender runs (here, to its end) before ReplyMessage returns.
    //
    // QR gets a 16-bit window procedure that, for WM_USER, calls
    // ReplyMessage(55) and then keeps GetModuleHandle("CHILD") at SS:30h:
    // 0 when CHILD has ended. The program then handles CHILD's message in
    // PeekMessage, and ends with exit code 2 unless SS:30h is 0.
    let (exit, log) = run_receiving("reply_message", |p| {
        p.code.emit(&[0x36, 0xC7, 0x06, 0x30, 0x00, 0xFF, 0xFF]); // mov word [ss:30h],-1
        p.subclass(b"QR\0", WM_USER, |p| {
            p.code.push(0).push(55).call("USER", REPLY_MESSAGE);
            p.push_string(b"CHILD\0");
            p.code.call("KERNEL", GET_MODULE_HANDLE).emit(&[
                0x36, 0xA3, 0x30, 0x00, // mov [ss:30h],ax
                0x33, 0xC0, // xor ax,ax
                0x99, // cwd
            ]);
        });
        p.push_msg();
        p.code
            .push(0)
            .push(0)
            .push(0)
            .push(PM_REMOVE_NOYIELD)
            .call("USER", PEEK_MESSAGE)
            .emit(&[
                0x36, 0x83, 0x3E, 0x30, 0x00, 0x00, // cmp word [ss:30h],0
                0x74, 0x06, // je on
                0xB0, 0x02, // mov al,2
            ]);
        p.code.exit_al();
    });
    assert_eq!(exit, 1, "{log}");
}

#[test]
fn a_reply_in_send_messages_wait_lets_the_sender_run_before_it_returns() {
    // Windows 95, three tasks. The program sends WM_USER+2 to THIRD's
    // window QC and waits. QC's procedure posts CHILD a message and
    // yields to it, and CHILD sends WM_USER to the program's window QR,
    // which the program handles within its wait. QR's procedure yields to
    // THIRD, whose procedure returns: the program's own reply is there.
    // QR's procedure then returns, replying to CHILD, and CHILD runs to
    // its end before the program's SendMessage returns. Exit code 1 when
    // CHILD has ended when it returns, 2 when it is still there.
    let mut p = Program::default();
    p.create_window(b"QR\0");
    p.subclass(b"QR\0", WM_USER, |p| {
        p.yield_to_window(b"QC\0");
        p.code.emit(&[0x33, 0xC0, 0x99]); // xor ax,ax; cwd
    });
    p.start_and_wait(b"C:\\CHILD.EXE\0", b"QB\0");
    p.start_and_wait(b"C:\\THIRD.EXE\0", b"QC\0");
    p.code
        .emit(&[0x8B, 0xF0]) // mov si,ax
        .emit(&[0x56]) // push si
        .push(WM_USER + 2)
        .push(0)
        .push(0)
        .push(0)
        .call("USER", SEND_MESSAGE);
    p.push_string(b"CHILD\0");
    p.code
        .call("KERNEL", GET_MODULE_HANDLE)
        .emit(&[0x8B, 0xF8]) // mov di,ax
        // THIRD ends on the message it gets next.
        .emit(&[0x56]) // push si
        .push(WM_USER + 3)
        .push(0)
        .push(0)
        .push(0)
        .call("USER", POST_MESSAGE);
    // Until THIRD has ended, Yield.
    let wait = p.code.here();
    p.push_string(b"THIRD\0");
    p.code.call("KERNEL", GET_MODULE_HANDLE).emit(&[
        0x0B, 0xC0, // or ax,ax
        0x74, 0x07, // jz gone
    ]);
    p.code.call("KERNEL", YIELD);
    let back = wait.wrapping_sub(p.code.here() + 2) as u8;
    p.code.emit(&[0xEB, back]); // jmp wait
    p.code.emit(&[
        0x0B, 0xFF, // gone: or di,di
        0xB0, 0x01, // mov al,1
        0x74, 0x02, // jz done
        0xB0, 0x02, // mov al,2
    ]);
    p.code.exit_al();

    // THIRD: the window QC, whose procedure, for WM_USER+2, posts CHILD's
    // window QB a message and yields to CHILD. It ends with exit code 9 on
    // the first message posted to it.
    let mut third = Program::default();
    third.create_window(b"QC\0");
    third.subclass(b"QC\0", WM_USER + 2, |p| {
        p.find_window(b"QB\0");
        p.code
            .emit(&[0x50]) // push ax
            .push(WM_USER)
            .push(0)
            .push(0)
            .push(0)
            .call("USER", POST_MESSAGE);
        p.yield_to_window(b"QB\0");
        p.code.emit(&[0x33, 0xC0, 0x99]); // xor ax,ax; cwd
    });
    third.msg_space();
    third.push_msg();
    third.code.push(0).push(0).push(0).call("USER", GET_MESSAGE);
    third.code.emit(&[0xB0, 0x09]).exit_al(); // mov al,9

    let others = vec![
        ("CHILD.EXE", child(), 7),
        ("THIRD.EXE", third.build(Some("THIRD")), 9),
    ];
    let (exit, log) = run_tasks("reply_in_send_wait", p.build(None), others);
    assert_eq!(exit, 1, "{log}");
}
