//! The built-in USER32's EDIT control, driven by programs made here with
//! rust-dos-pe's builder, their code assembled with iced, as in
//! win32_comctl32_tests.rs. Each program makes its calls one after
//! another, keeps what each returned in its data, and spins; the test
//! reads the results while the program is still there.
//!
//! The expected values are what Windows 98 SE gives, as a probe program
//! run on it in 86Box showed (see `page_keys_scroll_a_page`).

use iced_x86::code_asm::*;
use rust_dos::cpu::{Cpu, HostConsole};
use rust_dos::exec::{NoHook, run_batch};
use rust_dos_pe::{self as pe, Builder, Imp};
use std::fs;
use std::path::PathBuf;

const BASE: u32 = 0x40_0000;

// The data's layout.

/// The results of the calls, in order.
const RESULTS: u32 = 0x40;
const MAX_CALLS: u32 = 128;
const DONE: u32 = RESULTS + 4 * MAX_CALLS;
/// Where the strings and structures the tests put start.
const FREE: u32 = 0x400;
const DATA_SIZE: usize = 0x1000;

const IMPORTS: &[(&str, &[&str])] = &[
    (
        "USER32.DLL",
        &[
            "CreateWindowExA",
            "SendMessageA",
            "SetWindowTextA",
            "SetFocus",
        ],
    ),
    ("GDI32.DLL", &["GetStockObject"]),
];

/// An argument a program passes.
#[derive(Clone, Copy, Debug)]
enum Arg {
    /// A value.
    I(u32),
    /// The address of the data's byte at an offset.
    At(u32),
    /// What the call of this index returned.
    Res(usize),
}

/// A call a program makes: the function's name and its arguments.
struct Call(&'static str, Vec<Arg>);

fn call(name: &'static str, args: &[Arg]) -> Call {
    Call(name, args.to_vec())
}

/// A program making `calls`, with `data` (DATA_SIZE bytes), and where
/// its data is.
fn program(data: &[u8], calls: &[Call]) -> (Vec<u8>, u32) {
    assert!(calls.len() as u32 <= MAX_CALLS);
    let mut b = Builder::exe(BASE);
    let lists: Vec<Vec<Imp>> = IMPORTS
        .iter()
        .map(|(_, names)| names.iter().map(|n| Imp::Name(n)).collect())
        .collect();
    let dlls: Vec<(&str, &[Imp])> = IMPORTS
        .iter()
        .zip(&lists)
        .map(|((dll, _), list)| (*dll, list.as_slice()))
        .collect();
    let iat = b.imports(&dlls).concat();
    let data_rva = b.section(
        ".data",
        data,
        pe::SCN_INITIALIZED_DATA | pe::SCN_READ | pe::SCN_WRITE,
    );
    let at = BASE + data_rva;
    let import = |name: &str| {
        let index = IMPORTS
            .iter()
            .flat_map(|(_, names)| names.iter())
            .position(|n| *n == name)
            .unwrap_or_else(|| panic!("{name} isn't imported"));
        dword_ptr(iat[index] as u64)
    };
    let code_rva = b.next_rva();
    let mut a = CodeAssembler::new(32).unwrap();
    for (i, Call(name, args)) in calls.iter().enumerate() {
        for arg in args.iter().rev() {
            match *arg {
                Arg::I(v) => a.push(v as i32).unwrap(),
                Arg::At(o) => a.push((at + o) as i32).unwrap(),
                Arg::Res(r) => a
                    .push(dword_ptr((at + RESULTS + 4 * r as u32) as u64))
                    .unwrap(),
            }
        }
        a.call(import(name)).unwrap();
        a.mov(dword_ptr((at + RESULTS + 4 * i as u32) as u64), eax)
            .unwrap();
    }
    a.mov(dword_ptr((at + DONE) as u64), 1u32).unwrap();
    let mut again = a.create_label();
    a.set_label(&mut again).unwrap();
    a.jmp(again).unwrap();
    let code = a.assemble((BASE + code_rva) as u64).unwrap();
    assert_eq!(
        b.section(
            ".text",
            &code,
            pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ
        ),
        code_rva
    );
    b.entry(code_rva);
    (b.build(), at)
}

/// A machine with the built-in Windows on and `program` loaded from the
/// shell as T.EXE.
fn machine(test: &str, program: Vec<u8>) -> Box<Cpu> {
    let base = PathBuf::from("target/test_win32_edit").join(test);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join("T.EXE"), program).unwrap();
    let mut cpu = Cpu::new(base);
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    cpu
}

/// The program's dword at `lin`, through its page tables.
fn peek(cpu: &Cpu, lin: u32) -> u32 {
    let pde = cpu
        .bus
        .read_32((cpu.cr3 & !0xFFF) as usize + 4 * (lin >> 22) as usize);
    assert!(pde & 1 != 0, "no page table for {lin:X}");
    let pte = cpu
        .bus
        .read_32((pde & !0xFFF) as usize + 4 * ((lin >> 12) & 0x3FF) as usize);
    assert!(pte & 1 != 0, "no page at {lin:X}");
    cpu.bus
        .read_32((pte & !0xFFF) as usize + (lin & 0xFFF) as usize)
}

/// Run `calls` with `data`: what each call returned, and the data's
/// dwords from FREE on.
fn run(test: &str, data: &[u8], calls: &[Call]) -> (Vec<u32>, Box<Cpu>, u32) {
    let (exe, at) = program(data, calls);
    let mut cpu = machine(test, exe);
    for _ in 0..2000 {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        run_batch(&mut cpu, &mut NoHook, false);
        let paged = cpu.cr0 & 0x8000_0001 == 0x8000_0001;
        if paged && peek(&cpu, at + DONE) == 1 {
            let r = (0..calls.len() as u32)
                .map(|i| peek(&cpu, at + RESULTS + 4 * i))
                .collect();
            return (r, cpu, at);
        }
    }
    panic!("the program didn't finish, at {:X}", cpu.eip());
}

const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_BORDER: u32 = 0x0080_0000;
const WS_VSCROLL: u32 = 0x0020_0000;
const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
const ES_MULTILINE: u32 = 0x0004;
const ES_AUTOVSCROLL: u32 = 0x0040;
const ES_NOHIDESEL: u32 = 0x0100;
const SYSTEM_FIXED_FONT: u32 = 16;
const WM_SETFONT: u32 = 0x0030;
const WM_KEYDOWN: u32 = 0x0100;
const WM_VSCROLL: u32 = 0x0115;
const EM_GETSEL: u32 = 0x00B0;
const EM_SETSEL: u32 = 0x00B1;
const EM_GETRECT: u32 = 0x00B2;
const EM_SCROLL: u32 = 0x00B5;
const EM_LINESCROLL: u32 = 0x00B6;
const EM_SCROLLCARET: u32 = 0x00B7;
const EM_GETFIRSTVISIBLELINE: u32 = 0x00CE;
const SB_PAGEUP: u32 = 2;
const SB_PAGEDOWN: u32 = 3;
const VK_PRIOR: u32 = 0x21;
const VK_NEXT: u32 = 0x22;

/// Page Down and Page Up in a multi-line edit, and EM_SCROLL's and
/// WM_VSCROLL's pages. Windows 98 SE scrolls a page of one line less
/// than the edit shows, and puts the caret where a click at its old
/// place in the window would: it keeps its row and column, and one out
/// of view goes to the line next to the view on its side. A probe on
/// Windows 98 SE in 86Box sent these keys and messages to edits of 9,
/// 10 and 1 lines from these starting points; the values below are what
/// it gave for 9 lines. Only the setting up differs: there EM_SETSEL
/// leaves the view as it is (#2237), so the calls here put the view in
/// place after it.
#[test]
fn page_keys_scroll_a_page() {
    use Arg::*;
    // The edit's 100 lines, "line 00" to "line 99", 9 bytes each with
    // the line break.
    const S_STATIC: u32 = FREE;
    const S_EDIT: u32 = FREE + 0x10;
    const RECT: u32 = FREE + 0x20;
    const TEXT: u32 = FREE + 0x40;
    let mut d = vec![0u8; DATA_SIZE];
    let put = |d: &mut [u8], at: u32, bytes: &[u8]| {
        d[at as usize..at as usize + bytes.len()].copy_from_slice(bytes)
    };
    put(&mut d, S_STATIC, b"Static\0");
    put(&mut d, S_EDIT, b"Edit\0");
    let text: Vec<String> = (0..100).map(|i| format!("line {i:02}")).collect();
    put(&mut d, TEXT, text.join("\r\n").as_bytes());
    let send = |msg: u32, wp: u32, lp: u32| call("SendMessageA", &[Res(1), I(msg), I(wp), I(lp)]);
    let key = |vk: u32| send(WM_KEYDOWN, vk, 1);
    let first = || send(EM_GETFIRSTVISIBLELINE, 0, 0);
    let sel = || send(EM_GETSEL, 0, 0);
    let mut calls = vec![
        call(
            "CreateWindowExA",
            &[
                I(0),
                At(S_STATIC),
                I(0),
                I(WS_OVERLAPPEDWINDOW | WS_VISIBLE),
                I(0),
                I(0),
                I(400),
                I(400),
                I(0),
                I(0),
                I(0),
                I(0),
            ],
        ),
        call(
            "CreateWindowExA",
            &[
                I(0),
                At(S_EDIT),
                I(0),
                I(WS_CHILD
                    | WS_VISIBLE
                    | WS_BORDER
                    | WS_VSCROLL
                    | ES_MULTILINE
                    | ES_AUTOVSCROLL
                    | ES_NOHIDESEL),
                I(0),
                I(0),
                I(300),
                // Nine lines of the fixed font, 16 pixels high.
                I(162),
                Res(0),
                I(1),
                I(0),
                I(0),
            ],
        ),
        call("GetStockObject", &[I(SYSTEM_FIXED_FONT)]),
        call("SendMessageA", &[Res(1), I(WM_SETFONT), Res(2), I(0)]),
        call("SetWindowTextA", &[Res(1), At(TEXT)]),
        call("SetFocus", &[Res(1)]),
        call("SendMessageA", &[Res(1), I(EM_GETRECT), I(0), At(RECT)]),
    ];
    // (what was done, the call that gives the first line in view, the
    // one that gives the selection, the expected first line and caret).
    let mut checks: Vec<(&str, usize, usize, u32, u32)> = Vec::new();
    let mut check = |calls: &mut Vec<Call>, what, line, caret| {
        calls.push(first());
        calls.push(sel());
        checks.push((what, calls.len() - 2, calls.len() - 1, line, caret));
    };
    let at = |line: u32, col: u32| line * 9 + col;
    // From the top: a page is 8 lines, and the caret keeps its row.
    calls.push(key(VK_NEXT));
    check(&mut calls, "Page Down from the top", 8, at(8, 0));
    calls.push(key(VK_NEXT));
    check(&mut calls, "Page Down again", 16, at(16, 0));
    calls.push(key(VK_PRIOR));
    check(&mut calls, "Page Up", 8, at(8, 0));
    // The caret keeps its column too.
    calls.push(send(EM_SETSEL, 0, 0));
    calls.push(send(EM_SCROLLCARET, 0, 0));
    calls.push(send(EM_SETSEL, at(3, 4), at(3, 4)));
    calls.push(key(VK_NEXT));
    check(&mut calls, "Page Down from row 3", 8, at(11, 4));
    calls.push(key(VK_PRIOR));
    check(&mut calls, "Page Up to row 3", 0, at(3, 4));
    // At the end the view stops with the last line at its bottom, and
    // the caret moves as far as the view did.
    calls.push(send(EM_LINESCROLL, 0, 85));
    calls.push(send(EM_SETSEL, at(86, 0), at(86, 0)));
    calls.push(key(VK_NEXT));
    check(&mut calls, "Page Down near the end", 91, at(92, 0));
    calls.push(key(VK_NEXT));
    check(&mut calls, "Page Down at the end", 91, at(92, 0));
    // A caret below the view: the line just below the page's view,
    // which the view then follows by one line.
    calls.push(send(EM_SETSEL, 0, 0));
    calls.push(send(EM_SCROLLCARET, 0, 0));
    calls.push(send(EM_SETSEL, at(30, 3), at(30, 3)));
    calls.push(send(EM_LINESCROLL, 0, (-30i32) as u32));
    calls.push(key(VK_NEXT));
    check(&mut calls, "Page Down, caret below", 9, at(17, 3));
    // A caret above the view: the line just above it. Line 10 is in
    // view, so the view stays at 9 and then moves to 40.
    calls.push(send(EM_SETSEL, at(10, 3), at(10, 3)));
    calls.push(send(EM_LINESCROLL, 0, 31));
    calls.push(key(VK_PRIOR));
    check(&mut calls, "Page Up, caret above", 31, at(31, 3));
    // EM_SCROLL's page: the lines it moved in the low word, 1 above.
    calls.push(send(EM_SETSEL, 0, 0));
    calls.push(send(EM_SCROLLCARET, 0, 0));
    let scrolls = calls.len();
    calls.push(send(EM_SCROLL, SB_PAGEDOWN, 0));
    calls.push(send(EM_SCROLL, SB_PAGEDOWN, 0));
    calls.push(send(EM_SCROLL, SB_PAGEUP, 0));
    check(&mut calls, "EM_SCROLL pages", 8, 0);
    // Page up from line 3 moves 3 lines.
    calls.push(send(EM_LINESCROLL, 0, (-5i32) as u32));
    let up = calls.len();
    calls.push(send(EM_SCROLL, SB_PAGEUP, 0));
    calls.push(send(EM_SCROLL, SB_PAGEUP, 0));
    check(&mut calls, "EM_SCROLL page up to the top", 0, 0);
    calls.push(send(WM_VSCROLL, SB_PAGEDOWN, 0));
    check(&mut calls, "WM_VSCROLL page down", 8, 0);
    calls.push(send(EM_LINESCROLL, 0, 82));
    let end = calls.len();
    calls.push(send(EM_SCROLL, SB_PAGEDOWN, 0));
    calls.push(send(EM_SCROLL, SB_PAGEDOWN, 0));
    check(&mut calls, "EM_SCROLL page down to the end", 91, 0);

    let (r, cpu, data) = run("page_keys", &d, &calls);
    assert_ne!(r[1], 0, "the edit is made");
    let top = peek(&cpu, data + RECT + 4);
    let bottom = peek(&cpu, data + RECT + 12);
    assert_eq!((bottom - top) / 16, 9, "the edit shows 9 lines");
    for (what, first, sel, line, caret) in checks {
        assert_eq!(
            (r[first], r[sel] >> 16, r[sel] & 0xFFFF),
            (line, caret, caret),
            "{what}: the first line in view, and the selection"
        );
    }
    assert_eq!(&r[scrolls..scrolls + 3], &[0x1_0008, 0x1_0008, 0x1_FFF8]);
    // From line 3, after EM_LINESCROLL took 8 - 5.
    assert_eq!(&r[up..up + 2], &[0x1_FFFD, 0]);
    // From line 8 + 82 = 90, one line to go.
    assert_eq!(&r[end..end + 2], &[0x1_0001, 0]);
}
