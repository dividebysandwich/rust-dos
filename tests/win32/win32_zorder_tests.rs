//! The built-in USER32's z-order of top-level windows: SetWindowPos with
//! HWND_TOPMOST, HWND_NOTOPMOST, HWND_TOP, HWND_BOTTOM and a window to
//! go after, on windows that own others. The program is a PE file made
//! with rust-dos-pe's builder, its code assembled with iced, as in
//! win32_ddraw_tests.rs. It takes a picture of the z-order after each
//! call, and the test compares the pictures with what the same calls
//! gave on Windows 98 SE.

use iced_x86::code_asm::*;
use rust_dos::cpu::{Cpu, HostConsole};
use rust_dos::exec::{NoHook, run_batch};
use rust_dos_pe::{self as pe, Builder, Imp};
use std::fs;
use std::path::PathBuf;

const BASE: u32 = 0x40_0000;

const WS_POPUP: u32 = 0x8000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_CAPTION: u32 = 0x00C0_0000;
const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
const WS_EX_TOPMOST: u32 = 0x8;
const HWND_TOP: u32 = 0;
const HWND_BOTTOM: u32 = 1;
const HWND_TOPMOST: u32 = -1i32 as u32;
const HWND_NOTOPMOST: u32 = -2i32 as u32;
const SWP_NOSIZE: u32 = 0x1;
const SWP_NOMOVE: u32 = 0x2;
const SWP_NOZORDER: u32 = 0x4;
const SWP_NOACTIVATE: u32 = 0x10;
const GW_HWNDNEXT: u32 = 2;
const GWL_EXSTYLE: u32 = -20i32 as u32;

const USER32: &[&str] = &[
    "CreateWindowExA",
    "SetWindowPos",
    "GetTopWindow",
    "GetWindow",
    "GetWindowLongA",
];
const CREATE_WINDOW: usize = 0;
const SET_WINDOW_POS: usize = 1;
const GET_TOP_WINDOW: usize = 2;
const GET_WINDOW: usize = 3;
const GET_WINDOW_LONG: usize = 4;

// The data's layout.

/// The windows' handles, in the order of `NAMES`.
const HANDLES: u32 = 0x00;
const DONE: u32 = 0x40;
const CLASS: u32 = 0x50;
/// Each window's title, its letter, 4 bytes each.
const TITLES: u32 = 0x60;
/// The pictures: for each, the top-level windows from the top, a
/// handle and its extended style each, ended by a zero handle.
const SHOTS: u32 = 0x100;
const SHOT_SIZE: u32 = 0x100;

/// The windows, made in this order: A overlapped, B owned by A, C owned
/// by B, E owned by A, H owned by A and hidden, D unowned and made
/// first.
const NAMES: [char; 6] = ['D', 'A', 'B', 'C', 'E', 'H'];

/// Where `HANDLES` has the window named `name`.
fn handle(name: char) -> u32 {
    HANDLES + 4 * NAMES.iter().position(|&n| n == name).unwrap() as u32
}

/// What a step does: SetWindowPos on a window, after HWND_TOP and the
/// others or after a window of the test, with these flags besides
/// SWP_NOMOVE, SWP_NOSIZE and SWP_NOACTIVATE.
#[derive(Clone, Copy)]
enum After {
    Special(u32),
    Window(char),
}

/// A step: the window, where it goes, other flags, and the z-order
/// Windows 98 SE had after it, top first: each window's letter, T for
/// WS_EX_TOPMOST, and "|" where the windows without WS_EX_TOPMOST start.
type Step = (char, After, u32, &'static str);

use After::{Special, Window};

/// The steps, and what Windows 98 SE gave for them (TOPMOST.EXE, built
/// with OpenWatcom, in 86Box). Windows 98 SE's own topmost taskbar sits
/// among them there; it isn't counted.
const STEPS: &[Step] = &[
    ('D', Special(HWND_TOP), 0, "| D H E C B A"),
    ('A', Special(HWND_TOPMOST), 0, "HT ET CT BT AT | D"),
    ('D', Special(HWND_TOP), 0, "HT ET CT BT AT | D"),
    ('B', Special(HWND_TOP), 0, "CT BT HT ET AT | D"),
    ('A', Special(HWND_TOP), 0, "CT BT HT ET AT | D"),
    ('A', Special(HWND_NOTOPMOST), 0, "| C B H E A D"),
    ('B', Special(HWND_TOPMOST), 0, "CT BT | H E A D"),
    ('A', Special(HWND_TOP), 0, "CT BT | H E A D"),
    ('C', Special(HWND_NOTOPMOST), 0, "BT | C H E A D"),
    ('A', Special(HWND_TOPMOST), 0, "CT BT HT ET AT | D"),
    ('B', Special(HWND_NOTOPMOST), 0, "| H E C B A D"),
    ('A', Special(HWND_TOPMOST), 0, "HT ET CT BT AT | D"),
    ('D', Special(HWND_TOPMOST), 0, "DT HT ET CT BT AT |"),
    ('C', Special(HWND_NOTOPMOST), 0, "DT | H E C B A"),
    ('D', Special(HWND_NOTOPMOST), 0, "| D H E C B A"),
    ('A', Special(HWND_BOTTOM), 0, "| D H E C B A"),
    ('A', Special(HWND_TOPMOST), 0, "HT ET CT BT AT | D"),
    (
        'A',
        Special(HWND_NOTOPMOST),
        SWP_NOZORDER,
        "HT ET CT BT AT | D",
    ),
    ('H', Special(HWND_NOTOPMOST), 0, "| E C B H A D"),
    ('B', Special(HWND_TOPMOST), 0, "CT BT | E H A D"),
    ('A', Special(HWND_NOTOPMOST), 0, "| C B E H A D"),
    ('H', Window('C'), 0, "| C H B E A D"),
    ('A', Special(HWND_TOP), 0, "| H C B E A D"),
    ('E', Window('D'), 0, "| D H C B E A"),
    ('A', Special(HWND_TOP), 0, "| H C B E A D"),
    ('B', Special(HWND_TOPMOST), 0, "CT BT | H E A D"),
    ('A', Special(HWND_TOPMOST), 0, "CT BT HT ET AT | D"),
    ('H', Window('D'), 0, "| D C B E H A"),
    ('A', Special(HWND_TOPMOST), 0, "CT BT ET HT AT | D"),
    ('A', Special(HWND_BOTTOM), 0, "| D C B E H A"),
    ('A', Special(HWND_TOPMOST), 0, "CT BT ET HT AT | D"),
    ('B', Special(HWND_BOTTOM), 0, "| D E H C B A"),
    ('A', Special(HWND_TOPMOST), 0, "ET HT CT BT AT | D"),
    ('D', Special(HWND_TOP), 0, "ET HT CT BT AT | D"),
    ('D', Window('B'), 0, "ET HT CT BT DT AT |"),
    ('D', Special(HWND_NOTOPMOST), 0, "ET HT CT BT AT | D"),
    ('A', Special(HWND_TOPMOST), 0, "ET HT CT BT AT | D"),
    ('D', Window('A'), 0, "ET HT CT BT AT DT |"),
    ('D', Special(HWND_NOTOPMOST), 0, "ET HT CT BT AT | D"),
    ('A', Special(HWND_TOPMOST), 0, "ET HT CT BT AT | D"),
    ('E', Window('D'), 0, "| D H C B E A"),
    ('A', Special(HWND_TOPMOST), 0, "HT CT BT ET AT | D"),
    ('C', Special(HWND_TOP), 0, "CT BT HT ET AT | D"),
    ('A', Special(HWND_TOP), 0, "CT BT HT ET AT | D"),
    ('E', Window('C'), 0, "CT ET BT HT AT | D"),
    ('A', Special(HWND_NOTOPMOST), 0, "| E C B H A D"),
    ('C', Special(HWND_BOTTOM), 0, "| D E H C B A"),
    ('D', Special(HWND_TOP), 0, "| D E H C B A"),
    ('C', Special(HWND_TOP), 0, "| C B E H A D"),
];

/// The program: the windows, a picture, then each step and a picture.
fn zorder_program() -> (Vec<u8>, u32) {
    let mut data = vec![0u8; (SHOTS + SHOT_SIZE * (STEPS.len() as u32 + 1)) as usize];
    data[CLASS as usize..CLASS as usize + 7].copy_from_slice(b"Static\0");
    for (i, n) in NAMES.iter().enumerate() {
        data[TITLES as usize + 4 * i] = *n as u8;
    }
    let mut b = Builder::exe(BASE);
    let list: Vec<Imp> = USER32.iter().map(|n| Imp::Name(n)).collect();
    let iat = b.imports(&[("USER32.DLL", &list)]).concat();
    let data_rva = b.section(
        ".data",
        &data,
        pe::SCN_INITIALIZED_DATA | pe::SCN_READ | pe::SCN_WRITE,
    );
    let at = |offset: u32| (BASE + data_rva + offset) as i32;
    let f = |index: usize| dword_ptr(iat[index] as u64);
    let code_rva = b.next_rva();
    let mut a = CodeAssembler::new(32).unwrap();
    let code = |a: &mut CodeAssembler| -> Result<(), IcedError> {
        let windows = [
            ('D', WS_OVERLAPPEDWINDOW | WS_VISIBLE, None, 300),
            ('A', WS_OVERLAPPEDWINDOW | WS_VISIBLE, None, 0),
            ('B', WS_POPUP | WS_CAPTION | WS_VISIBLE, Some('A'), 40),
            ('C', WS_POPUP | WS_CAPTION | WS_VISIBLE, Some('B'), 80),
            ('E', WS_POPUP | WS_CAPTION | WS_VISIBLE, Some('A'), 120),
            ('H', WS_POPUP | WS_CAPTION, Some('A'), 160),
        ];
        for (name, style, owner, x) in windows {
            let i = NAMES.iter().position(|&n| n == name).unwrap() as u32;
            for _ in 0..3 {
                a.push(0)?;
            }
            match owner {
                Some(o) => a.push(dword_ptr(at(handle(o)) as u64))?,
                None => a.push(0)?,
            }
            for v in [120, 200, 40 + x / 2, x, style as i32] {
                a.push(v)?;
            }
            a.push(at(TITLES + 4 * i))?;
            a.push(at(CLASS))?;
            a.push(0)?;
            a.call(f(CREATE_WINDOW))?;
            a.mov(dword_ptr(at(handle(name)) as u64), eax)?;
        }
        for i in 0..=STEPS.len() {
            if i > 0 {
                let (name, after, flags, _) = STEPS[i - 1];
                let flags = flags | SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
                for v in [flags, 0, 0, 0, 0] {
                    a.push(v as i32)?;
                }
                match after {
                    Special(v) => a.push(v as i32)?,
                    Window(w) => a.push(dword_ptr(at(handle(w)) as u64))?,
                }
                a.push(dword_ptr(at(handle(name)) as u64))?;
                a.call(f(SET_WINDOW_POS))?;
            }
            // The picture: every top-level window from the top.
            a.mov(edi, at(SHOTS + SHOT_SIZE * i as u32))?;
            a.push(0)?;
            a.call(f(GET_TOP_WINDOW))?;
            let mut next = a.create_label();
            let mut end = a.create_label();
            a.set_label(&mut next)?;
            a.mov(esi, eax)?;
            a.mov(dword_ptr(edi), esi)?;
            a.test(esi, esi)?;
            a.je(end)?;
            a.push(GWL_EXSTYLE as i32)?;
            a.push(esi)?;
            a.call(f(GET_WINDOW_LONG))?;
            a.mov(dword_ptr(edi + 4), eax)?;
            a.add(edi, 8)?;
            a.push(GW_HWNDNEXT as i32)?;
            a.push(esi)?;
            a.call(f(GET_WINDOW))?;
            a.jmp(next)?;
            a.set_label(&mut end)?;
        }
        a.mov(dword_ptr(at(DONE) as u64), 1)?;
        let mut again = a.create_label();
        a.set_label(&mut again)?;
        a.jmp(again)
    };
    code(&mut a).unwrap();
    let bytes = a.assemble((BASE + code_rva) as u64).unwrap();
    let flags = pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ;
    assert_eq!(b.section(".text", &bytes, flags), code_rva);
    b.entry(code_rva);
    (b.build(), BASE + data_rva)
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

/// Run `program` as T.EXE until it says it is done.
fn run(test: &str, program: Vec<u8>, data: u32) -> Cpu {
    let base = PathBuf::from("target/test_win32_zorder").join(test);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join("T.EXE"), program).unwrap();
    let mut cpu = Cpu::new(base);
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    for _ in 0..400 {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        run_batch(&mut cpu, &mut NoHook, false);
        let paged = cpu.cr0 & 0x8000_0001 == 0x8000_0001;
        if paged && peek(&cpu, data + DONE) == 1 {
            return cpu;
        }
    }
    panic!("the program didn't finish, at {:X}", cpu.eip());
}

/// Picture `index` as the steps write it: the test's windows from the
/// top, and "|" before the first window without WS_EX_TOPMOST, or at
/// the end if every window is topmost.
fn shot(cpu: &Cpu, data: u32, index: usize) -> String {
    let handles: Vec<u32> = (0..NAMES.len())
        .map(|i| peek(cpu, data + HANDLES + 4 * i as u32))
        .collect();
    let mut out = Vec::new();
    let mut band = false;
    let mut at = data + SHOTS + SHOT_SIZE * index as u32;
    loop {
        let hwnd = peek(cpu, at);
        if hwnd == 0 {
            break;
        }
        let topmost = peek(cpu, at + 4) & WS_EX_TOPMOST != 0;
        if !topmost && !band {
            out.push("|".to_string());
            band = true;
        }
        if let Some(i) = handles.iter().position(|&h| h == hwnd) {
            out.push(format!("{}{}", NAMES[i], if topmost { "T" } else { "" }));
        }
        at += 8;
    }
    if !band {
        out.push("|".to_string());
    }
    out.join(" ")
}

#[test]
fn set_window_pos_moves_owners_and_owned_windows_as_windows_98_does() {
    let (program, data) = zorder_program();
    let cpu = run("owners", program, data);
    // Made in this order, each new one on top, the hidden one too.
    assert_eq!(shot(&cpu, data, 0), "| H E C B A D");
    let mut wrong = Vec::new();
    for (i, &(name, after, flags, want)) in STEPS.iter().enumerate() {
        let got = shot(&cpu, data, i + 1);
        if got != want {
            let after = match after {
                Special(v) => format!("{}", v as i32),
                Window(w) => format!("after {w}"),
            };
            wrong.push(format!(
                "step {}: {name} {after} flags {flags:X}: got {got:?}, Windows 98 SE {want:?}",
                i + 1
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
