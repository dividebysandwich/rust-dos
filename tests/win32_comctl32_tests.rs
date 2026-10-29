//! The built-in COMCTL32: its functions and its controls, called by
//! programs made here with rust-dos-pe's builder, their code assembled
//! with iced, as in win32_screen_tests.rs. Each program makes its calls
//! one after another, keeps what each returned in its data, and spins;
//! the test reads the results while the program is still there.

use iced_x86::code_asm::*;
use rust_dos::cpu::{Cpu, HostConsole};
use rust_dos::exec::{NoHook, run_batch};
use rust_dos_pe::{self as pe, Builder, Imp};
use std::fs;
use std::path::PathBuf;

const BASE: u32 = 0x40_0000;

/// Where a test program's data and imports are, for its code.
struct Layout {
    data: u32,
    iat: Vec<u32>,
    imports: &'static [(&'static str, &'static [&'static str])],
}

impl Layout {
    /// The address of byte `offset` of the data.
    fn at(&self, offset: u32) -> u64 {
        (self.data + offset) as u64
    }
}

// The data's layout.

/// The results of the calls, in order.
const RESULTS: u32 = 0x40;
const MAX_CALLS: u32 = 128;
const DONE: u32 = RESULTS + 4 * MAX_CALLS;
/// Where the strings and structures the tests put start.
const FREE: u32 = 0x400;
const DATA_SIZE: usize = 0x2000;
/// Where the timer procedure keeps the file dialog's folder list
/// (`Arg::Press`): its first items, FOLDER_SIZE bytes each.
const FOLDERS: u32 = 0x1C00;
const FOLDER_ITEMS: u32 = 4;
const FOLDER_SIZE: u32 = 0x40;
/// The folder list's id, and the message that gives an item's text.
const LST2: u32 = 0x0461;
const LB_GETTEXT: u32 = 0x0189;

/// The imports every program has: the functions `Call::api` names.
const IMPORTS: &[(&str, &[&str])] = &[
    (
        "COMCTL32.DLL",
        &[
            "InitCommonControls",
            "InitCommonControlsEx",
            "DllGetVersion",
            "ImageList_Create",
            "ImageList_Destroy",
            "ImageList_GetImageCount",
            "ImageList_SetImageCount",
            "ImageList_GetIconSize",
            "ImageList_Remove",
            "CreateUpDownControl",
            "DPA_Create",
            "DPA_InsertPtr",
            "DPA_GetPtr",
            "DPA_Destroy",
            "CreateToolbarEx",
            "CreateStatusWindowA",
            "PropertySheetA",
            "CreatePropertySheetPageA",
            "DestroyPropertySheetPage",
        ],
    ),
    (
        "USER32.DLL",
        &[
            "CreateWindowExA",
            "DestroyWindow",
            "SendMessageA",
            "GetWindowTextA",
            "SetWindowTextA",
            "GetClientRect",
            "GetWindowLongA",
        ],
    ),
    ("GDI32.DLL", &["CreateBitmap", "DeleteObject"]),
    (
        "COMDLG32.DLL",
        &[
            "GetOpenFileNameA",
            "GetSaveFileNameA",
            "GetOpenFileNameW",
            "GetFileTitleA",
            "CommDlgExtendedError",
            "PrintDlgA",
        ],
    ),
    (
        "USER32.DLL",
        &[
            "SetTimer",
            "GetActiveWindow",
            "PostMessageA",
            "SendDlgItemMessageA",
        ],
    ),
    ("KERNEL32.DLL", &["GetModuleHandleA"]),
    (
        "USER32.DLL",
        &[
            "CreateDialogIndirectParamA",
            "GetWindowRect",
            "GetDialogBaseUnits",
        ],
    ),
    (
        "GDI32.DLL",
        &[
            "CreateCompatibleDC",
            "SelectObject",
            "GetStockObject",
            "CreateFontA",
            "GetTextMetricsA",
            "GetTextExtentPoint32A",
        ],
    ),
];

/// The import slot of `name`.
fn import(l: &Layout, name: &str) -> AsmMemoryOperand {
    let index = l
        .imports
        .iter()
        .flat_map(|(_, names)| names.iter())
        .position(|n| *n == name)
        .unwrap_or_else(|| panic!("{name} isn't imported"));
    dword_ptr(l.iat[index] as u64)
}

/// An argument a program passes.
#[derive(Clone, Copy, Debug)]
enum Arg {
    /// A value.
    I(u32),
    /// The address of the data's byte at an offset.
    At(u32),
    /// What the call of this index returned.
    Res(usize),
    /// The program's timer procedure, which presses OK in the active
    /// window: what ends a modal dialog. Before that it keeps the first
    /// items of the file dialog's folder list, if the window has one, at
    /// FOLDERS.
    Press,
}

/// A call a program makes: the function's name and its arguments.
struct Call(&'static str, Vec<Arg>);

fn call(name: &'static str, args: &[Arg]) -> Call {
    Call(name, args.to_vec())
}

/// A program making `calls`, with `data` (DATA_SIZE bytes) and where its
/// data is.
fn program(data: &[u8], calls: &[Call]) -> (Vec<u8>, u32) {
    program_with(IMPORTS, data, calls)
}

/// A program making `calls`, as `program`, importing `imports` only.
fn program_with(
    imports: &'static [(&'static str, &'static [&'static str])],
    data: &[u8],
    calls: &[Call],
) -> (Vec<u8>, u32) {
    assert!(calls.len() as u32 <= MAX_CALLS);
    let mut b = Builder::exe(BASE);
    let lists: Vec<Vec<Imp>> = imports
        .iter()
        .map(|(_, names)| names.iter().map(|n| Imp::Name(n)).collect())
        .collect();
    let dlls: Vec<(&str, &[Imp])> = imports
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
    let l = Layout {
        data: BASE + data_rva,
        iat,
        imports,
    };
    let code_rva = b.next_rva();
    // The timer procedure, first in the code.
    let ok = BASE + code_rva;
    let mut a = CodeAssembler::new(32).unwrap();
    a.call(import(&l, "GetActiveWindow")).unwrap();
    a.push(ebx).unwrap();
    a.mov(ebx, eax).unwrap();
    for i in 0..FOLDER_ITEMS {
        a.push(l.at(FOLDERS + FOLDER_SIZE * i) as i32).unwrap();
        a.push(i as i32).unwrap();
        a.push(LB_GETTEXT as i32).unwrap();
        a.push(LST2 as i32).unwrap();
        a.push(ebx).unwrap();
        a.call(import(&l, "SendDlgItemMessageA")).unwrap();
    }
    a.mov(eax, ebx).unwrap();
    a.pop(ebx).unwrap();
    a.push(0).unwrap();
    a.push(1).unwrap();
    a.push(0x111).unwrap();
    a.push(eax).unwrap();
    a.call(import(&l, "PostMessageA")).unwrap();
    a.ret_1(16).unwrap();
    let mut code = a.assemble(ok as u64).unwrap();
    let main_rva = code_rva + code.len() as u32;
    let mut a = CodeAssembler::new(32).unwrap();
    for (i, Call(name, args)) in calls.iter().enumerate() {
        for arg in args.iter().rev() {
            match *arg {
                Arg::I(v) => a.push(v as i32).unwrap(),
                Arg::At(o) => a.push(l.at(o) as i32).unwrap(),
                Arg::Res(r) => a.push(dword_ptr(l.at(RESULTS + 4 * r as u32))).unwrap(),
                Arg::Press => a.push(ok as i32).unwrap(),
            }
        }
        a.call(import(&l, name)).unwrap();
        a.mov(dword_ptr(l.at(RESULTS + 4 * i as u32)), eax).unwrap();
    }
    a.mov(dword_ptr(l.at(DONE)), 1u32).unwrap();
    let mut again = a.create_label();
    a.set_label(&mut again).unwrap();
    a.jmp(again).unwrap();
    code.extend(a.assemble((BASE + main_rva) as u64).unwrap());
    assert_eq!(
        b.section(
            ".text",
            &code,
            pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ
        ),
        code_rva
    );
    b.entry(main_rva);
    (b.build(), l.data)
}

/// A machine with the built-in Windows on and `program` loaded from the
/// shell as T.EXE.
fn machine(test: &str, program: Vec<u8>) -> Cpu {
    let base = PathBuf::from("target/test_win32_comctl32").join(test);
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

/// The physical address of the program's `lin`, through its page tables.
fn physical(cpu: &Cpu, lin: u32) -> usize {
    let pde = cpu
        .bus
        .read_32((cpu.cr3 & !0xFFF) as usize + 4 * (lin >> 22) as usize);
    assert!(pde & 1 != 0, "no page table for {lin:X}");
    let pte = cpu
        .bus
        .read_32((pde & !0xFFF) as usize + 4 * ((lin >> 12) & 0x3FF) as usize);
    assert!(pte & 1 != 0, "no page at {lin:X}");
    (pte & !0xFFF) as usize + (lin & 0xFFF) as usize
}

/// The program's dword at `lin`.
fn peek(cpu: &Cpu, lin: u32) -> u32 {
    cpu.bus.read_32(physical(cpu, lin))
}

/// The program's bytes at `lin` up to a 0.
fn peek_str(cpu: &Cpu, lin: u32) -> String {
    let mut out = Vec::new();
    for i in 0..256 {
        let b = cpu.bus.read_8(physical(cpu, lin + i));
        if b == 0 {
            break;
        }
        out.push(b);
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Run `calls` with `data`: the machine, still running the program,
/// and where its data is.
fn run(test: &str, data: &[u8], calls: &[Call]) -> (Cpu, u32) {
    run_program(test, program(data, calls))
}

/// Run the program `exe`, whose data is at `at`, as `run` does.
fn run_program(test: &str, (exe, at): (Vec<u8>, u32)) -> (Cpu, u32) {
    run_machine(machine(test, exe), at)
}

/// Run the program `machine` has loaded, whose data is at `at`, as `run`
/// does.
fn run_machine(mut cpu: Cpu, at: u32) -> (Cpu, u32) {
    for _ in 0..2000 {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        run_batch(&mut cpu, &mut NoHook, false);
        let paged = cpu.cr0 & 0x8000_0001 == 0x8000_0001;
        if paged && peek(&cpu, at + DONE) == 1 {
            return (cpu, at);
        }
    }
    panic!("the program didn't finish, at {:X}", cpu.eip());
}

/// What the calls returned.
fn results(cpu: &Cpu, data: u32, n: usize) -> Vec<u32> {
    (0..n as u32)
        .map(|i| peek(cpu, data + RESULTS + 4 * i))
        .collect()
}

fn data() -> Vec<u8> {
    vec![0u8; DATA_SIZE]
}

fn put(data: &mut [u8], offset: u32, bytes: &[u8]) {
    data[offset as usize..offset as usize + bytes.len()].copy_from_slice(bytes);
}

fn put32(data: &mut [u8], offset: u32, value: u32) {
    put(data, offset, &value.to_le_bytes());
}

const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;

/// The data's strings: class names, at these offsets.
const S_STATIC: u32 = 0x200;
const S_PROGRESS: u32 = 0x210;
const S_EDIT: u32 = 0x230;
const S_UPDOWN: u32 = 0x240;

fn strings(data: &mut [u8]) {
    put(data, S_STATIC, b"Static\0");
    put(data, S_PROGRESS, b"msctls_progress32\0");
    put(data, S_EDIT, b"Edit\0");
    put(data, S_UPDOWN, b"msctls_updown32\0");
}

/// CreateWindowExA of a top-level "Static" window, the parent.
fn parent() -> Call {
    use Arg::*;
    call(
        "CreateWindowExA",
        &[
            I(0),
            At(S_STATIC),
            I(0),
            I(WS_OVERLAPPEDWINDOW | WS_VISIBLE),
            I(0),
            I(0),
            I(300),
            I(200),
            I(0),
            I(0),
            I(0),
            I(0),
        ],
    )
}

/// CreateWindowExA of a child of the window call `parent` made.
fn child(class: u32, style: u32, w: u32, h: u32, parent: usize, id: u32) -> Call {
    use Arg::*;
    call(
        "CreateWindowExA",
        &[
            I(0),
            At(class),
            I(0),
            I(WS_CHILD | WS_VISIBLE | style),
            I(10),
            I(10),
            I(w),
            I(h),
            Res(parent),
            I(id),
            I(0),
            I(0),
        ],
    )
}

fn send(hwnd: usize, msg: u32, wp: Arg, lp: Arg) -> Call {
    call("SendMessageA", &[Arg::Res(hwnd), Arg::I(msg), wp, lp])
}

#[test]
fn dll_get_version_says_4_70() {
    use Arg::*;
    let mut d = data();
    put32(&mut d, FREE, 20);
    let (cpu, at) = run(
        "version",
        &d,
        &[
            call("InitCommonControls", &[]),
            call("DllGetVersion", &[At(FREE)]),
        ],
    );
    assert_eq!(results(&cpu, at, 2)[1], 0);
    let version: Vec<u32> = (1..4).map(|i| peek(&cpu, at + FREE + 4 * i)).collect();
    assert_eq!(version, [4, 70, 1158]);
}

#[test]
fn a_progress_bar_keeps_its_range_and_steps() {
    use Arg::*;
    const PBM_SETRANGE: u32 = 0x0401;
    const PBM_SETPOS: u32 = 0x0402;
    const PBM_SETSTEP: u32 = 0x0404;
    const PBM_STEPIT: u32 = 0x0405;
    const PBM_GETPOS: u32 = 0x0408;
    let mut d = data();
    strings(&mut d);
    let calls = [
        call("InitCommonControls", &[]),
        parent(),
        child(S_PROGRESS, 0, 200, 20, 1, 1),
        send(2, PBM_SETRANGE, I(0), I(50 << 16)),
        send(2, PBM_SETPOS, I(20), I(0)),
        send(2, PBM_SETSTEP, I(5), I(0)),
        send(2, PBM_STEPIT, I(0), I(0)),
        send(2, PBM_GETPOS, I(0), I(0)),
        send(2, PBM_SETPOS, I(80), I(0)),
        send(2, PBM_GETPOS, I(0), I(0)),
    ];
    let (cpu, at) = run("progress", &d, &calls);
    let r = results(&cpu, at, calls.len());
    assert_ne!(r[2], 0, "the progress bar is made");
    // SETRANGE gives the range before, 0 to 100.
    assert_eq!(r[3], 100 << 16);
    assert_eq!(r[4], 0);
    assert_eq!(r[6], 20, "STEPIT gives the position before");
    assert_eq!(r[7], 25);
    assert_eq!(r[9], 50, "a position past the range stops at its end");
}

#[test]
fn a_program_without_comctl32_makes_a_progress_bar() {
    use Arg::*;
    const PBM_SETPOS: u32 = 0x0402;
    const PBM_GETPOS: u32 = 0x0408;
    const S_COMCTL32: u32 = 0x260;
    // The timer procedure's functions, and those the calls make: no
    // COMCTL32, as in a setup program that uses SHELL32.
    const USER_ONLY: &[(&str, &[&str])] = &[
        (
            "USER32.DLL",
            &[
                "CreateWindowExA",
                "SendMessageA",
                "GetActiveWindow",
                "PostMessageA",
                "SendDlgItemMessageA",
            ],
        ),
        ("KERNEL32.DLL", &["GetModuleHandleA"]),
    ];
    let mut d = data();
    strings(&mut d);
    put(&mut d, S_COMCTL32, b"COMCTL32.DLL\0");
    let calls = [
        call("GetModuleHandleA", &[At(S_COMCTL32)]),
        parent(),
        child(S_PROGRESS, 0, 200, 20, 1, 1),
        send(2, PBM_SETPOS, I(30), I(0)),
        send(2, PBM_GETPOS, I(0), I(0)),
        call("GetModuleHandleA", &[At(S_COMCTL32)]),
    ];
    let (cpu, at) = run_program("no_comctl32", program_with(USER_ONLY, &d, &calls));
    let r = results(&cpu, at, calls.len());
    assert_eq!(r[0], 0, "the program starts without COMCTL32");
    assert_ne!(r[2], 0, "the progress bar is made");
    assert_eq!(r[4], 30, "its procedure runs");
    assert_ne!(r[5], 0, "COMCTL32 is mapped for it");
}

#[test]
fn an_up_down_control_shows_its_value_in_its_buddy() {
    use Arg::*;
    const UDS_SETBUDDYINT: u32 = 0x0002;
    const UDS_ALIGNRIGHT: u32 = 0x0004;
    const UDM_SETPOS: u32 = 0x0467;
    const UDM_GETPOS: u32 = 0x0468;
    const UDM_GETBUDDY: u32 = 0x046A;
    let mut d = data();
    strings(&mut d);
    put(&mut d, FREE + 0x100, b"1234\0");
    let calls = [
        call("InitCommonControls", &[]),
        parent(),
        child(S_EDIT, 0, 100, 20, 1, 1),
        call(
            "CreateUpDownControl",
            &[
                I(WS_CHILD | WS_VISIBLE | UDS_SETBUDDYINT | UDS_ALIGNRIGHT),
                I(0),
                I(0),
                I(16),
                I(20),
                Res(1),
                I(2),
                I(0),
                Res(2),
                I(5000),
                I(0),
                I(42),
            ],
        ),
        call("GetWindowTextA", &[Res(2), At(FREE), I(32)]),
        send(3, UDM_GETBUDDY, I(0), I(0)),
        call("SetWindowTextA", &[Res(2), At(FREE + 0x100)]),
        send(3, UDM_GETPOS, I(0), I(0)),
        send(3, UDM_SETPOS, I(0), I(4321)),
        call("GetWindowTextA", &[Res(2), At(FREE + 0x80), I(32)]),
    ];
    let (cpu, at) = run("updown", &d, &calls);
    let r = results(&cpu, at, calls.len());
    assert_ne!(r[3], 0, "the up-down control is made");
    assert_eq!(peek_str(&cpu, at + FREE), "42");
    assert_eq!(r[5], r[2], "the edit control is its buddy");
    assert_eq!(r[7], 1234, "the value is read back from the buddy");
    assert_eq!(r[8], 1234);
    assert_eq!(peek_str(&cpu, at + FREE + 0x80), "4,321");
}

#[test]
fn image_lists_grow_and_shrink() {
    use Arg::*;
    const ILC_COLOR16: u32 = 0x10;
    let d = data();
    let calls = [
        call(
            "ImageList_Create",
            &[I(16), I(16), I(ILC_COLOR16), I(1), I(4)],
        ),
        call("CreateBitmap", &[I(48), I(16), I(1), I(1), I(0)]),
        call("ImageList_SetImageCount", &[Res(0), I(3)]),
        call("ImageList_GetImageCount", &[Res(0)]),
        call("ImageList_Remove", &[Res(0), I(1)]),
        call("ImageList_GetImageCount", &[Res(0)]),
        call("ImageList_GetIconSize", &[Res(0), At(FREE), At(FREE + 4)]),
        call("ImageList_Remove", &[Res(0), I(5)]),
        call("ImageList_Destroy", &[Res(0)]),
        call("ImageList_Destroy", &[Res(0)]),
    ];
    let (cpu, at) = run("imagelist", &d, &calls);
    let r = results(&cpu, at, calls.len());
    assert_ne!(r[0], 0);
    assert_eq!(r[3], 3);
    assert_eq!(r[5], 2);
    assert_eq!((peek(&cpu, at + FREE), peek(&cpu, at + FREE + 4)), (16, 16));
    assert_eq!(r[7], 0, "there is no image 5 to remove");
    assert_eq!((r[8], r[9]), (1, 0), "a list is destroyed once");
}

#[test]
fn a_dpa_keeps_its_pointers_in_order() {
    use Arg::*;
    const DPA_APPEND: u32 = 0x7FFF_FFFF;
    let d = data();
    let calls = [
        call("DPA_Create", &[I(4)]),
        call("DPA_InsertPtr", &[Res(0), I(DPA_APPEND), I(0x111)]),
        call("DPA_InsertPtr", &[Res(0), I(0), I(0x222)]),
        call("DPA_InsertPtr", &[Res(0), I(DPA_APPEND), I(0x333)]),
        call("DPA_GetPtr", &[Res(0), I(0)]),
        call("DPA_GetPtr", &[Res(0), I(2)]),
        call("DPA_GetPtr", &[Res(0), I(3)]),
        call("DPA_Destroy", &[Res(0)]),
    ];
    let (cpu, at) = run("dpa", &d, &calls);
    let r = results(&cpu, at, calls.len());
    assert_eq!(&r[1..7], &[0, 0, 2, 0x222, 0x333, 0]);
    assert_eq!(r[7], 1);
}

const CDERR_STRUCTSIZE: u32 = 1;
const CDERR_INITIALIZATION: u32 = 2;

#[test]
fn comdlg32_says_what_went_wrong() {
    use Arg::*;
    let mut d = data();
    put(&mut d, FREE, b"C:\\GAMES\\SAVE.DAT\0");
    put32(&mut d, FREE + 0x100, 12);
    let calls = [
        call("GetOpenFileNameA", &[I(0)]),
        call("CommDlgExtendedError", &[]),
        call("GetSaveFileNameA", &[At(FREE + 0x100)]),
        call("CommDlgExtendedError", &[]),
        call("PrintDlgA", &[At(FREE + 0x100)]),
        call("CommDlgExtendedError", &[]),
        call("PrintDlgA", &[I(0)]),
        call("CommDlgExtendedError", &[]),
        call("GetOpenFileNameW", &[At(FREE + 0x100)]),
        call("GetFileTitleA", &[At(FREE), At(FREE + 0x200), I(32)]),
        call("GetFileTitleA", &[At(FREE), At(FREE + 0x280), I(4)]),
    ];
    let (cpu, at) = run("comdlg32_errors", &d, &calls);
    let r = results(&cpu, at, calls.len());
    assert_eq!((r[0], r[1]), (0, CDERR_INITIALIZATION));
    assert_eq!((r[2], r[3]), (0, CDERR_STRUCTSIZE), "a structure too small");
    assert_eq!((r[4], r[5]), (0, 0), "printing is cancelled");
    assert_eq!((r[6], r[7]), (0, CDERR_INITIALIZATION));
    assert_eq!(r[8], 0, "there are no wide dialogs");
    assert_eq!(r[9], 0);
    assert_eq!(peek_str(&cpu, at + FREE + 0x200), "SAVE.DAT");
    assert_eq!(r[10], 9, "the size the title needs");
}

const OFN_ALLOWMULTISELECT: u32 = 0x0200;
const OFN_EXPLORER: u32 = 0x0008_0000;

/// A program opening a file with `flags`: a timer presses OK in the
/// dialog, whose file name is T.EXE. What GetOpenFileNameA returned, and
/// the path it gave.
fn open_dialog(test: &str, flags: u32) -> (u32, String, u32) {
    use Arg::*;
    let ofn = FREE;
    let file = FREE + 0x100;
    let mut d = data();
    put32(&mut d, ofn, 76);
    put(&mut d, file, b"T.EXE\0");
    put32(&mut d, ofn + 32, 260);
    put32(&mut d, ofn + 52, flags);
    let calls = [
        call("SetTimer", &[I(0), I(0), I(100), Press]),
        call("GetOpenFileNameA", &[At(ofn)]),
        call("CommDlgExtendedError", &[]),
    ];
    // lpstrFile: the data is where it is in every program.
    let (_, at) = program(&d, &calls);
    put32(&mut d, ofn + 28, at + file);
    let (cpu, at) = run(test, &d, &calls);
    let r = results(&cpu, at, calls.len());
    assert_eq!(r[2], 0, "no error");
    let offset = peek(&cpu, at + ofn + 56) & 0xFFFF;
    (r[1], peek_str(&cpu, at + file), offset)
}

#[test]
fn the_explorer_style_open_dialog_gives_the_file_chosen() {
    let (ok, path, offset) = open_dialog("open_explorer", OFN_EXPLORER);
    assert_eq!(ok, 1);
    assert_eq!(path, r"C:\T.EXE");
    assert_eq!(offset, 3, "nFileOffset");
}

#[test]
fn the_old_style_open_dialog_gives_the_file_chosen() {
    let (ok, path, _) = open_dialog("open_old", OFN_ALLOWMULTISELECT);
    assert_eq!(ok, 1);
    assert_eq!(path.to_ascii_uppercase(), r"C:\T.EXE");
}

/// The old-style dialog's folder list shows the folders from the drive's
/// root down to the current one, then the current one's folders, in that
/// order, although the list is sorted: a folder named 1997 stays under
/// c:\, and DATA under the folder it is in.
#[test]
fn the_old_style_folder_list_keeps_the_path_in_order() {
    use Arg::*;
    let ofn = FREE;
    let file = FREE + 0x100;
    let dir = FREE + 0x200;
    let mut d = data();
    put32(&mut d, ofn, 76);
    put(&mut d, file, b"T.EXE\0");
    put(&mut d, dir, b"C:\\1997\\MOTO\0");
    put32(&mut d, ofn + 32, 260);
    put32(&mut d, ofn + 52, OFN_ALLOWMULTISELECT);
    let calls = [
        call("SetTimer", &[I(0), I(0), I(100), Press]),
        call("GetOpenFileNameA", &[At(ofn)]),
    ];
    // lpstrFile and lpstrInitialDir: the data is where it is in every
    // program.
    let (_, at) = program(&d, &calls);
    put32(&mut d, ofn + 28, at + file);
    put32(&mut d, ofn + 44, at + dir);
    let (exe, at) = program(&d, &calls);
    let cpu = machine("folder_list", exe);
    let base = PathBuf::from("target/test_win32_comctl32/folder_list");
    fs::create_dir_all(base.join("1997/MOTO/DATA")).unwrap();
    let (cpu, at) = run_machine(cpu, at);
    let folders: Vec<String> = (0..FOLDER_ITEMS)
        .map(|i| peek_str(&cpu, at + FOLDERS + FOLDER_SIZE * i))
        .collect();
    assert_eq!(folders, ["c:\\", "1997", "moto", "data"]);
}

#[test]
fn a_tree_view_sorts_selects_and_deletes_its_items() {
    use Arg::*;
    const TVM_INSERTITEMA: u32 = 0x1100;
    const TVM_DELETEITEM: u32 = 0x1101;
    const TVM_GETCOUNT: u32 = 0x1105;
    const TVM_GETNEXTITEM: u32 = 0x110A;
    const TVM_SELECTITEM: u32 = 0x110B;
    const TVI_ROOT: u32 = 0xFFFF_0000;
    const TVI_SORT: u32 = 0xFFFF_0003;
    const TVIF_TEXT: u32 = 1;
    const TVGN_ROOT: u32 = 0;
    const TVGN_NEXT: u32 = 1;
    const TVGN_CARET: u32 = 9;
    const S_TREE: u32 = 0x260;
    let mut d = data();
    strings(&mut d);
    put(&mut d, S_TREE, b"SysTreeView32\0");
    let calls = [
        call("InitCommonControls", &[]),
        parent(),
        child(S_TREE, 0, 200, 100, 1, 1),
        send(2, TVM_INSERTITEMA, I(0), At(FREE)),
        send(2, TVM_INSERTITEMA, I(0), At(FREE + 0x40)),
        send(2, TVM_GETNEXTITEM, I(TVGN_ROOT), I(0)),
        send(2, TVM_GETCOUNT, I(0), I(0)),
        send(2, TVM_SELECTITEM, I(TVGN_CARET), Res(3)),
        send(2, TVM_GETNEXTITEM, I(TVGN_CARET), I(0)),
        send(2, TVM_DELETEITEM, I(0), Res(4)),
        send(2, TVM_GETNEXTITEM, I(TVGN_ROOT), I(0)),
        send(2, TVM_GETNEXTITEM, I(TVGN_NEXT), Res(3)),
        send(2, TVM_DELETEITEM, I(0), I(TVI_ROOT)),
        send(2, TVM_GETCOUNT, I(0), I(0)),
    ];
    // The labels' addresses: the data is where the program has it.
    let (_, base) = program(&d, &calls);
    // Two TVINSERTSTRUCTs, "b" then "a", sorted in at the root.
    for (n, label) in [b"b\0", b"a\0"].iter().enumerate() {
        let at = FREE + 0x40 * n as u32;
        let text = FREE + 0x100 + 0x10 * n as u32;
        put(&mut d, text, *label);
        put32(&mut d, at, TVI_ROOT);
        put32(&mut d, at + 4, TVI_SORT);
        put32(&mut d, at + 8, TVIF_TEXT);
        put32(&mut d, at + 24, base + text);
    }
    let (cpu, at) = run("treeview", &d, &calls);
    let r = results(&cpu, at, calls.len());
    assert_ne!(r[2], 0, "the tree view is made");
    assert!(r[3] != 0 && r[4] != 0 && r[3] != r[4]);
    assert_eq!(r[5], r[4], "\"a\" is sorted in before \"b\"");
    assert_eq!(r[6], 2);
    assert_eq!((r[7], r[8]), (1, r[3]), "\"b\" is selected");
    assert_eq!(r[9], 1);
    assert_eq!((r[10], r[11]), (r[3], 0), "\"b\" is left alone");
    assert_eq!((r[12], r[13]), (1, 0), "TVI_ROOT deletes them all");
}

/// A toolbar as COMDLG32's file dialog makes one: the view bitmap of
/// COMCTL32, buttons without text, and a string after them, which
/// adds no text row.
#[test]
fn a_toolbar_without_text_rows_keeps_none() {
    use Arg::*;
    const TB_ADDBITMAP: u32 = 0x0413;
    const TB_ADDBUTTONSA: u32 = 0x0414;
    const TB_BUTTONCOUNT: u32 = 0x0418;
    const TB_ADDSTRINGA: u32 = 0x041C;
    const TB_BUTTONSTRUCTSIZE: u32 = 0x041E;
    const TB_AUTOSIZE: u32 = 0x0421;
    const TB_GETIMAGELIST: u32 = 0x0431;
    const TB_SETMAXTEXTROWS: u32 = 0x043C;
    const TB_GETTEXTROWS: u32 = 0x043D;
    const TBSTYLE_FLAT: u32 = 0x0800;
    const TBSTYLE_TOOLTIPS: u32 = 0x0100;
    const S_TOOLBAR: u32 = 0x260;
    let mut d = data();
    strings(&mut d);
    put(&mut d, S_TOOLBAR, b"ToolbarWindow32\0");
    // TBADDBITMAP: HINST_COMMCTRL, IDB_VIEW_SMALL_COLOR.
    put32(&mut d, FREE, 0xFFFF_FFFF);
    put32(&mut d, FREE + 4, 4);
    // Two TBBUTTONs: images 8 and 9, enabled, no text.
    for (i, (image, id)) in [(8u32, 0xA001u32), (9, 0xA002)].iter().enumerate() {
        let at = FREE + 0x20 + 20 * i as u32;
        put32(&mut d, at, *image);
        put32(&mut d, at + 4, *id);
        put32(&mut d, at + 8, 0x04);
    }
    put(&mut d, FREE + 0x80, b"winetestwinetest\0\0");
    let calls = [
        call("InitCommonControls", &[]),
        parent(),
        child(S_TOOLBAR, TBSTYLE_FLAT | TBSTYLE_TOOLTIPS, 100, 30, 1, 1),
        send(2, TB_BUTTONSTRUCTSIZE, I(20), I(0)),
        send(2, TB_SETMAXTEXTROWS, I(0), I(0)),
        send(2, TB_ADDBITMAP, I(12), At(FREE)),
        send(2, TB_GETIMAGELIST, I(0), I(0)),
        send(2, TB_ADDBUTTONSA, I(2), At(FREE + 0x20)),
        send(2, TB_AUTOSIZE, I(0), I(0)),
        send(2, TB_ADDSTRINGA, I(0), At(FREE + 0x80)),
        send(2, TB_GETTEXTROWS, I(0), I(0)),
        send(2, TB_BUTTONCOUNT, I(0), I(0)),
        call("ImageList_GetImageCount", &[Res(6)]),
    ];
    let (cpu, at) = run("toolbar", &d, &calls);
    let r = results(&cpu, at, calls.len());
    assert_ne!(r[2], 0, "the toolbar is made");
    assert_eq!(r[5], 0, "the view bitmap's images start at 0");
    assert_ne!(r[6], 0, "TB_ADDBITMAP made an image list");
    assert_eq!(r[9], 0, "the string is the first");
    assert_eq!(r[10], 0, "no text rows");
    assert_eq!(r[11], 2);
    assert_eq!(r[12], 12, "the view bitmap has 12 images");
}

/// CreateToolbarEx makes the toolbar, adds the bitmap and the buttons.
#[test]
fn create_toolbar_ex_adds_the_bitmap_and_buttons() {
    use Arg::*;
    const TB_BUTTONCOUNT: u32 = 0x0418;
    const TB_GETBUTTONSIZE: u32 = 0x043A;
    const TB_GETIMAGELIST: u32 = 0x0431;
    let mut d = data();
    strings(&mut d);
    for (i, id) in [0x100u32, 0x101, 0x102].iter().enumerate() {
        let at = FREE + 20 * i as u32;
        put32(&mut d, at, i as u32);
        put32(&mut d, at + 4, *id);
        put32(&mut d, at + 8, 0x04);
    }
    let calls = [
        call("InitCommonControls", &[]),
        parent(),
        call(
            "CreateToolbarEx",
            &[
                Res(1),
                I(WS_CHILD | WS_VISIBLE),
                I(7),
                I(15),
                I(0xFFFF_FFFF),
                I(0),
                At(FREE),
                I(3),
                I(0),
                I(0),
                I(16),
                I(16),
                I(20),
            ],
        ),
        send(2, TB_BUTTONCOUNT, I(0), I(0)),
        send(2, TB_GETBUTTONSIZE, I(0), I(0)),
        send(2, TB_GETIMAGELIST, I(0), I(0)),
        call("ImageList_GetImageCount", &[Res(5)]),
        call("GetWindowLongA", &[Res(2), I(-12i32 as u32)]),
    ];
    let (cpu, at) = run("toolbarex", &d, &calls);
    let r = results(&cpu, at, calls.len());
    assert_ne!(r[2], 0, "the toolbar is made");
    assert_eq!(r[3], 3);
    assert_eq!(
        r[4],
        23 | (22 << 16),
        "16 by 16 images make 23 by 22 buttons"
    );
    assert_eq!(r[6], 15, "the standard bitmap has 15 images");
    assert_eq!(r[7], 7, "the id");
}

#[test]
fn a_status_bar_keeps_the_text_of_its_parts() {
    use Arg::*;
    const SB_SETTEXTA: u32 = 0x0401;
    const SB_GETTEXTA: u32 = 0x0402;
    const SB_SETPARTS: u32 = 0x0404;
    const SB_GETPARTS: u32 = 0x0406;
    const SB_GETTEXTLENGTHA: u32 = 0x0403;
    let mut d = data();
    strings(&mut d);
    put(&mut d, FREE, b"Ready\0");
    put(&mut d, FREE + 0x10, b"Two\0");
    for (i, x) in [50u32, 100, u32::MAX].iter().enumerate() {
        put32(&mut d, FREE + 0x20 + 4 * i as u32, *x);
    }
    let calls = [
        call("InitCommonControls", &[]),
        parent(),
        call(
            "CreateStatusWindowA",
            &[I(WS_CHILD | WS_VISIBLE), At(FREE), Res(1), I(7)],
        ),
        send(2, SB_GETTEXTLENGTHA, I(0), I(0)),
        send(2, SB_SETPARTS, I(3), At(FREE + 0x20)),
        send(2, SB_GETPARTS, I(3), At(FREE + 0x40)),
        send(2, SB_SETTEXTA, I(1), At(FREE + 0x10)),
        send(2, SB_GETTEXTA, I(1), At(FREE + 0x60)),
        send(2, SB_GETTEXTLENGTHA, I(2), I(0)),
    ];
    let (cpu, at) = run("status", &d, &calls);
    let r = results(&cpu, at, calls.len());
    assert_ne!(r[2], 0, "the status bar is made");
    assert_eq!(r[3] & 0xFFFF, 5, "part 0 has the window's text");
    assert_eq!((r[4], r[5]), (1, 3));
    let parts: Vec<u32> = (0..3)
        .map(|i| peek(&cpu, at + FREE + 0x40 + 4 * i))
        .collect();
    assert_eq!(parts, [50, 100, u32::MAX]);
    assert_eq!(r[7] & 0xFFFF, 3);
    assert_eq!(peek_str(&cpu, at + FREE + 0x60), "Two");
    assert_eq!(r[8], 0, "part 2 has no text");
}

#[test]
fn a_trackbar_keeps_its_position_in_its_range() {
    use Arg::*;
    const TBS_AUTOTICKS: u32 = 0x0001;
    const TBM_GETPOS: u32 = 0x0400;
    const TBM_SETPOS: u32 = 0x0405;
    const TBM_SETRANGE: u32 = 0x0406;
    const TBM_GETNUMTICS: u32 = 0x0410;
    const TBM_SETTICFREQ: u32 = 0x0414;
    const TBM_GETPAGESIZE: u32 = 0x0416;
    const S_TRACKBAR: u32 = 0x300;
    let mut d = data();
    strings(&mut d);
    put(&mut d, S_TRACKBAR, b"msctls_trackbar32\0");
    let calls = [
        call("InitCommonControls", &[]),
        parent(),
        child(S_TRACKBAR, TBS_AUTOTICKS, 200, 30, 1, 1),
        send(2, TBM_SETRANGE, I(1), I(50 << 16)),
        send(2, TBM_SETPOS, I(1), I(80)),
        send(2, TBM_GETPOS, I(0), I(0)),
        send(2, TBM_GETPAGESIZE, I(0), I(0)),
        send(2, TBM_SETTICFREQ, I(10), I(0)),
        send(2, TBM_GETNUMTICS, I(0), I(0)),
    ];
    let (cpu, at) = run("trackbar", &d, &calls);
    let r = results(&cpu, at, calls.len());
    assert_ne!(r[2], 0, "the trackbar is made");
    assert_eq!(r[5], 50, "a position past the range stops at its end");
    assert_eq!(r[6], 10, "a page is a fifth of the range");
    // Tics at 10, 20, 30 and 40, and the two ends.
    assert_eq!(r[8], 6);
}

/// Where the data of a program making `calls` is: the data comes before
/// the code, so its address doesn't change with what it holds.
fn data_address(calls: &[Call]) -> u32 {
    program(&data(), calls).1
}

#[test]
fn a_tab_control_keeps_its_items_and_selection() {
    use Arg::*;
    const S_TAB: u32 = 0x260;
    const TCIF_TEXT: u32 = 0x0001;
    const TCM_GETITEMCOUNT: u32 = 0x1304;
    const TCM_GETITEMA: u32 = 0x1305;
    const TCM_INSERTITEMA: u32 = 0x1307;
    const TCM_GETCURSEL: u32 = 0x130B;
    const TCM_SETCURSEL: u32 = 0x130C;
    let item = |text: u32| [TCIF_TEXT, 0, 0, text, 32, 0, 0];
    let calls = [
        call("InitCommonControls", &[]),
        parent(),
        child(S_TAB, 0, 200, 100, 1, 1),
        send(2, TCM_INSERTITEMA, I(0), At(FREE)),
        send(2, TCM_INSERTITEMA, I(1), At(FREE + 0x20)),
        send(2, TCM_GETITEMCOUNT, I(0), I(0)),
        send(2, TCM_GETCURSEL, I(0), I(0)),
        send(2, TCM_SETCURSEL, I(1), I(0)),
        send(2, TCM_GETCURSEL, I(0), I(0)),
        send(2, TCM_SETCURSEL, I(5), I(0)),
        send(2, TCM_GETITEMA, I(1), At(FREE + 0x40)),
    ];
    let base = data_address(&calls);
    let mut d = data();
    strings(&mut d);
    put(&mut d, S_TAB, b"SysTabControl32\0");
    put(&mut d, FREE + 0x100, b"One\0");
    put(&mut d, FREE + 0x110, b"Two\0");
    for (at, words) in [
        (FREE, item(base + FREE + 0x100)),
        (FREE + 0x20, item(base + FREE + 0x110)),
        (FREE + 0x40, item(base + FREE + 0x180)),
    ] {
        for (i, w) in words.iter().enumerate() {
            put32(&mut d, at + 4 * i as u32, *w);
        }
    }
    let (cpu, at) = run("tab", &d, &calls);
    assert_eq!(at, base);
    let r = results(&cpu, at, calls.len());
    assert_ne!(r[2], 0, "the tab control is made");
    assert_eq!((r[3], r[4], r[5]), (0, 1, 2));
    assert_eq!(r[6], 0, "the first item is selected");
    assert_eq!((r[7], r[8]), (0, 1), "SETCURSEL gives the one before");
    assert_eq!(r[9], u32::MAX, "there is no item 5");
    assert_eq!(r[10], 1);
    assert_eq!(peek_str(&cpu, at + FREE + 0x180), "Two");
}

#[test]
fn a_modeless_property_sheet_shows_its_page() {
    use Arg::*;
    const PSP_DLGINDIRECT: u32 = 0x0001;
    const PSH_PROPSHEETPAGE: u32 = 0x0008;
    const PSH_MODELESS: u32 = 0x0400;
    const PSM_PRESSBUTTON: u32 = 0x0471;
    const PSM_GETTABCONTROL: u32 = 0x0474;
    const PSM_GETCURRENTPAGEHWND: u32 = 0x0476;
    const PSBTN_CANCEL: u32 = 5;
    const TCM_GETITEMCOUNT: u32 = 0x1304;
    const PAGE: u32 = FREE;
    const HEADER: u32 = FREE + 0x40;
    const TEMPLATE: u32 = FREE + 0x80;
    const CAPTION: u32 = FREE + 0xC0;
    const TEXT: u32 = FREE + 0x100;
    let calls = [
        call("InitCommonControls", &[]),
        call("PropertySheetA", &[At(HEADER)]),
        send(1, PSM_GETTABCONTROL, I(0), I(0)),
        send(2, TCM_GETITEMCOUNT, I(0), I(0)),
        send(1, PSM_GETCURRENTPAGEHWND, I(0), I(0)),
        call("GetWindowTextA", &[Res(1), At(TEXT), I(64)]),
        send(1, PSM_PRESSBUTTON, I(PSBTN_CANCEL), I(0)),
        send(1, PSM_GETCURRENTPAGEHWND, I(0), I(0)),
        call("DestroyWindow", &[Res(1)]),
        call("CreatePropertySheetPageA", &[At(PAGE)]),
        call("DestroyPropertySheetPage", &[Res(9)]),
    ];
    let base = data_address(&calls);
    let mut d = data();
    // A DLGTEMPLATE of a page with no controls, titled "Pg".
    put32(&mut d, TEMPLATE, WS_CHILD);
    put(&mut d, TEMPLATE + 8, &[0, 0, 0, 0, 0, 0, 100, 0, 50, 0]);
    put(&mut d, TEMPLATE + 22, &[b'P', 0, b'g', 0, 0, 0]);
    put(&mut d, CAPTION, b"Things\0");
    let page = [40, PSP_DLGINDIRECT, 0, base + TEMPLATE, 0, 0, 0, 0, 0, 0];
    let flags = PSH_PROPSHEETPAGE | PSH_MODELESS;
    let header = [40, flags, 0, 0, 0, base + CAPTION, 1, 0, base + PAGE, 0];
    for (at, words) in [(PAGE, page), (HEADER, header)] {
        for (i, w) in words.iter().enumerate() {
            put32(&mut d, at + 4 * i as u32, *w);
        }
    }
    let (cpu, at) = run("propsheet", &d, &calls);
    assert_eq!(at, base);
    let r = results(&cpu, at, calls.len());
    assert!(r[1] != 0 && r[1] != u32::MAX, "the sheet is made");
    assert_ne!(r[2], 0, "it has a tab control");
    assert_eq!(r[3], 1, "with a tab for the page");
    assert_ne!(r[4], 0, "the page is made");
    assert_eq!(peek_str(&cpu, at + TEXT), "Things");
    assert_eq!(r[7], 0, "Cancel leaves no current page");
    assert_eq!(r[8], 1);
    assert_ne!(r[9], 0);
    assert_eq!(r[10], 1);
}

#[test]
fn a_sorted_report_list_view_keeps_its_items_in_order() {
    use Arg::*;
    const LVS_REPORT: u32 = 0x0001;
    const LVS_SORTASCENDING: u32 = 0x0010;
    const LVM_GETITEMCOUNT: u32 = 0x1004;
    const LVM_INSERTITEMA: u32 = 0x1007;
    const LVM_INSERTCOLUMNA: u32 = 0x101B;
    const LVM_GETCOLUMNWIDTH: u32 = 0x101D;
    const LVM_GETHEADER: u32 = 0x101F;
    const LVM_GETITEMTEXTA: u32 = 0x102D;
    const HDM_GETITEMCOUNT: u32 = 0x1200;
    const S_LISTVIEW: u32 = 0x250;
    const COLUMN: u32 = FREE;
    const ITEMS: [u32; 2] = [FREE + 0x40, FREE + 0x80];
    const GET: u32 = FREE + 0xC0;
    const TEXTS: [u32; 3] = [FREE + 0x100, FREE + 0x110, FREE + 0x120];
    const BUFFER: u32 = FREE + 0x140;
    let calls = [
        call("InitCommonControls", &[]),
        parent(),
        child(S_LISTVIEW, LVS_REPORT | LVS_SORTASCENDING, 200, 100, 1, 1),
        send(2, LVM_INSERTCOLUMNA, I(0), At(COLUMN)),
        send(2, LVM_INSERTITEMA, I(0), At(ITEMS[0])),
        send(2, LVM_INSERTITEMA, I(0), At(ITEMS[1])),
        send(2, LVM_GETITEMCOUNT, I(0), I(0)),
        send(2, LVM_GETITEMTEXTA, I(0), At(GET)),
        send(2, LVM_GETHEADER, I(0), I(0)),
        call("SendMessageA", &[Res(8), I(HDM_GETITEMCOUNT), I(0), I(0)]),
        send(2, LVM_GETCOLUMNWIDTH, I(0), I(0)),
    ];
    // The structures point into the data, which is where the program
    // puts it whatever the data holds.
    let mut d = data();
    let (_, base) = program(&d, &calls);
    strings(&mut d);
    put(&mut d, S_LISTVIEW, b"SysListView32\0");
    put(&mut d, TEXTS[0], b"Name\0");
    put(&mut d, TEXTS[1], b"beta\0");
    put(&mut d, TEXTS[2], b"alpha\0");
    // LVCOLUMNA: LVCF_WIDTH | LVCF_TEXT, 80 wide.
    for (k, v) in [6, 0, 80, base + TEXTS[0], 0, 0].into_iter().enumerate() {
        put32(&mut d, COLUMN + 4 * k as u32, v);
    }
    // LVITEMA: LVIF_TEXT.
    for (item, text) in ITEMS.into_iter().zip([TEXTS[1], TEXTS[2]]) {
        for (k, v) in [1, 0, 0, 0, 0, base + text].into_iter().enumerate() {
            put32(&mut d, item + 4 * k as u32, v);
        }
    }
    for (k, v) in [0, 0, 0, 0, 0, base + BUFFER, 32].into_iter().enumerate() {
        put32(&mut d, GET + 4 * k as u32, v);
    }
    let (cpu, at) = run("listview", &d, &calls);
    assert_eq!(at, base);
    let r = results(&cpu, at, calls.len());
    assert_ne!(r[2], 0, "the list view is made");
    assert_eq!(r[3], 0);
    assert_eq!((r[4], r[5]), (0, 0), "alpha sorts before beta");
    assert_eq!(r[6], 2);
    assert_eq!(r[7], 5);
    assert_eq!(peek_str(&cpu, at + BUFFER), "alpha");
    assert_ne!(r[8], 0, "the report view has a header");
    assert_eq!(r[9], 1, "the column is the header's item");
    assert_eq!(r[10], 80);
}

// GDI's raster fonts and the dialog base units USER takes from them.

const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// The TEXTMETRICA at `at`: tmHeight, tmAscent, tmDescent,
/// tmInternalLeading, tmAveCharWidth and tmMaxCharWidth.
fn text_metric(cpu: &Cpu, at: u32) -> [u32; 6] {
    [0, 4, 8, 12, 20, 24].map(|o| peek(cpu, at + o))
}

#[test]
fn ms_sans_serif_8_and_system_have_windows_98s_metrics() {
    use Arg::*;
    const SYSTEM_FONT: u32 = 13;
    const FACE: u32 = FREE;
    const LETTERS: u32 = FREE + 0x20;
    const TM: [u32; 2] = [FREE + 0x60, FREE + 0xA0];
    const SIZE: [u32; 2] = [FREE + 0xE0, FREE + 0xE8];
    let mut font = vec![I(-11i32 as u32)];
    font.extend([I(0); 3]);
    font.push(I(400));
    font.extend([I(0); 8]);
    font.push(At(FACE));
    let calls = [
        call("CreateCompatibleDC", &[I(0)]),
        call("CreateFontA", &font),
        call("SelectObject", &[Res(0), Res(1)]),
        call("GetTextMetricsA", &[Res(0), At(TM[0])]),
        call(
            "GetTextExtentPoint32A",
            &[Res(0), At(LETTERS), I(52), At(SIZE[0])],
        ),
        call("GetStockObject", &[I(SYSTEM_FONT)]),
        call("SelectObject", &[Res(0), Res(5)]),
        call("GetTextMetricsA", &[Res(0), At(TM[1])]),
        call(
            "GetTextExtentPoint32A",
            &[Res(0), At(LETTERS), I(52), At(SIZE[1])],
        ),
        call("GetDialogBaseUnits", &[]),
    ];
    let mut d = data();
    put(&mut d, FACE, b"MS Sans Serif\0");
    put(&mut d, LETTERS, ALPHABET);
    let (cpu, at) = run("font_metrics", &d, &calls);
    let r = results(&cpu, at, calls.len());
    assert_ne!(r[1], 0, "the font is made");
    assert_eq!(r[3], 1);
    assert_eq!(r[4], 1);
    // SSERIFE.FON's 8 point size and VGASYS.FON's System, as Windows 98
    // SE gives them at 96 dots an inch.
    assert_eq!(text_metric(&cpu, at + TM[0]), [13, 11, 2, 2, 5, 11]);
    let extent = |i: usize| (peek(&cpu, at + SIZE[i]), peek(&cpu, at + SIZE[i] + 4));
    assert_eq!(extent(0), (323, 13), "the alphabet in MS Sans Serif 8");
    assert_eq!(text_metric(&cpu, at + TM[1]), [16, 13, 3, 3, 7, 14]);
    assert_eq!(extent(1), (429, 16), "the alphabet in System");
    // (429 / 26 + 1) / 2 = 8 by 16.
    assert_eq!(r[9], 0x0010_0008, "the dialog base units");
}

#[test]
fn a_dialog_in_ms_sans_serif_8_is_6_by_13_units() {
    use Arg::*;
    const WS_POPUP: u32 = 0x8000_0000;
    const TEMPLATE: u32 = FREE;
    const RECT: u32 = FREE + 0x80;
    let calls = [
        call("GetModuleHandleA", &[I(0)]),
        call(
            "CreateDialogIndirectParamA",
            &[Res(0), At(TEMPLATE), I(0), I(0), I(0)],
        ),
        call("GetWindowRect", &[Res(1), At(RECT)]),
    ];
    let mut d = data();
    // A DLGTEMPLATE at (10, 10) and 200 by 100 units.
    let t = dialog_template(false, WS_POPUP, 0, [10, 10], [200, 100]);
    put(&mut d, TEMPLATE, &t);
    let (cpu, at) = run("dialog_units", &d, &calls);
    let r = results(&cpu, at, calls.len());
    assert_ne!(r[1], 0, "the dialog is made");
    assert_eq!(r[2], 1);
    let rect = [0, 4, 8, 12].map(|o| peek(&cpu, at + RECT + o) as i32);
    // x 10 * 6 / 4, y 10 * 13 / 8, 200 * 6 / 4 wide and 100 * 13 / 8
    // high, rounded.
    assert_eq!(rect, [15, 16, 15 + 300, 16 + 163]);
}

/// A dialog template with no controls, no menu, class or title, and the
/// font 8 point MS Sans Serif: a DLGTEMPLATEEX if `ex_form`.
fn dialog_template(ex_form: bool, style: u32, ex: u32, at: [i16; 2], size: [i16; 2]) -> Vec<u8> {
    const DS_SETFONT: u32 = 0x40;
    let mut t = Vec::new();
    if ex_form {
        for v in [1u16, 0xFFFF, 0, 0] {
            t.extend(v.to_le_bytes());
        }
        t.extend(ex.to_le_bytes());
        t.extend((style | DS_SETFONT).to_le_bytes());
    } else {
        t.extend((style | DS_SETFONT).to_le_bytes());
        t.extend(ex.to_le_bytes());
    }
    let [x, y] = at.map(|v| v as u16);
    let [width, height] = size.map(|v| v as u16);
    for v in [0, x, y, width, height, 0, 0, 0, 8] {
        t.extend(v.to_le_bytes());
    }
    if ex_form {
        // The weight, italic and the character set.
        t.extend(400u16.to_le_bytes());
        t.extend(0u16.to_le_bytes());
    }
    for c in "MS Sans Serif\0".encode_utf16() {
        t.extend(c.to_le_bytes());
    }
    t
}

#[test]
fn a_dialogs_window_starts_where_its_template_says() {
    use Arg::*;
    const WS_POPUP: u32 = 0x8000_0000;
    const WS_CHILD: u32 = 0x4000_0000;
    const WS_CAPTION: u32 = 0x00C0_0000;
    const WS_BORDER: u32 = 0x0080_0000;
    const WS_SYSMENU: u32 = 0x0008_0000;
    const WS_EX_EDGES: u32 = 0x0002_0200;
    let template: [u32; 6] = [0, 1, 2, 3, 4, 5].map(|i| FREE + 0x80 * i);
    const RECT: u32 = FREE + 0x300;
    let mut calls = vec![
        call("GetModuleHandleA", &[I(0)]),
        call(
            "CreateDialogIndirectParamA",
            &[Res(0), At(template[0]), I(0), I(0), I(0)],
        ),
    ];
    for t in &template[1..] {
        calls.push(call(
            "CreateDialogIndirectParamA",
            &[Res(0), At(*t), Res(1), I(0), I(0)],
        ));
    }
    for i in 0..6 {
        calls.push(call(
            "GetWindowRect",
            &[Res(1 + i), At(RECT + 16 * i as u32)],
        ));
    }
    let mut d = data();
    let popup = WS_POPUP | WS_CAPTION | WS_SYSMENU;
    put(
        &mut d,
        template[0],
        &dialog_template(false, popup, 0, [10, 10], [250, 150]),
    );
    // Glover's CONFIG.EXE has its pages as DLGTEMPLATEEXs at (0, 0)
    // with a client and a static edge.
    let pages = [
        (true, WS_CHILD, WS_EX_EDGES, [0, 0]),
        (false, WS_CHILD | WS_BORDER, 0, [-2, 40]),
        (true, WS_CHILD, WS_EX_EDGES, [-2, 70]),
        (true, WS_CHILD | WS_BORDER, 0, [230, 140]),
        (true, WS_CHILD | WS_BORDER, 0, [100, -2]),
    ];
    for (i, (ex_form, style, ex, at)) in pages.into_iter().enumerate() {
        put(
            &mut d,
            template[1 + i],
            &dialog_template(ex_form, style, ex, at, [40, 20]),
        );
    }
    let (cpu, at) = run("dialog_position", &d, &calls);
    let r = results(&cpu, at, calls.len());
    for (i, v) in r[1..7].iter().enumerate() {
        assert_ne!(*v, 0, "dialog {i} is made");
    }
    let rect = |i: u32| [0, 4, 8, 12].map(|o| peek(&cpu, at + RECT + 16 * i + o) as i32);
    // What Windows 98 SE gives. The popup's window, not its client area,
    // is at (10 * 6 / 4, 10 * 13 / 8); its client area is at (18, 38).
    assert_eq!(rect(0), [15, 16, 396, 285]);
    // The pages' windows are at their template's place in the client
    // area, their edges inside: 40 * 6 / 4 + 6 by 20 * 13 / 8 + 6.
    assert_eq!(rect(1), [18, 38, 84, 77]);
    // One left of the client area starts at its edge, and so does one
    // above it.
    assert_eq!(rect(2), [18, 103, 80, 138]);
    assert_eq!(rect(3), [18, 152, 84, 191]);
    assert_eq!(rect(5), [168, 38, 230, 73]);
    // One past the right and the bottom stays there.
    assert_eq!(rect(4), [363, 266, 425, 301]);
}

#[test]
fn owned_popups_child_captions_and_ds_control_are_placed_as_on_windows_98() {
    use Arg::*;
    const WS_POPUP: u32 = 0x8000_0000;
    const WS_CHILD: u32 = 0x4000_0000;
    const WS_CAPTION: u32 = 0x00C0_0000;
    const WS_SYSMENU: u32 = 0x0008_0000;
    const DS_ABSALIGN: u32 = 0x0001;
    const DS_CONTROL: u32 = 0x0400;
    let pop = WS_POPUP | WS_CAPTION | WS_SYSMENU;
    let child = WS_CHILD | WS_CAPTION | WS_SYSMENU;
    // Each dialog's style, place, and whether the first one owns it.
    let dialogs: [(u32, [i16; 2], bool); 10] = [
        (pop, [10, 10], false),
        (pop, [20, 20], true),
        (pop | DS_ABSALIGN, [20, 20], true),
        (pop, [-1, -1], true),
        (pop, [-9, -9], true),
        (pop | DS_CONTROL, [20, 20], true),
        (child, [0, 0], true),
        (child | DS_CONTROL, [50, 0], true),
        (pop, [400, 10], false),
        (pop, [10, 300], false),
    ];
    let n = dialogs.len();
    let template = |i: usize| FREE + 0x80 * i as u32;
    let rect_at = |i: usize| FREE + 0x80 * n as u32 + 16 * i as u32;
    let mut d = data();
    let mut calls = vec![call("GetModuleHandleA", &[I(0)])];
    for (i, (style, at, owned)) in dialogs.into_iter().enumerate() {
        // The owner is 150 by 80 units, the others 40 by 20.
        let size = if i == 0 { [150, 80] } else { [40, 20] };
        put(
            &mut d,
            template(i),
            &dialog_template(false, style, 0, at, size),
        );
        let owner = if owned { Res(1) } else { I(0) };
        calls.push(call(
            "CreateDialogIndirectParamA",
            &[Res(0), At(template(i)), owner, I(0), I(0)],
        ));
    }
    for i in 0..n {
        calls.push(call("GetWindowRect", &[Res(1 + i), At(rect_at(i))]));
    }
    let (cpu, at) = run("dialog_owned", &d, &calls);
    let r = results(&cpu, at, calls.len());
    for (i, v) in r[1..=n].iter().enumerate() {
        assert_ne!(*v, 0, "dialog {i} is made");
    }
    let rect = |i: usize| [0, 4, 8, 12].map(|o| peek(&cpu, at + rect_at(i) + o) as i32);
    // What Windows 98 SE gives the same templates. The owner's client
    // area is at (18, 38), and an owned popup's window corner goes at its
    // template's place in it; DS_ABSALIGN makes the place the screen's.
    assert_eq!(rect(0), [15, 16, 246, 171]);
    assert_eq!(rect(1), [48, 71, 114, 129]);
    assert_eq!(rect(2), [30, 33, 96, 91]);
    // A place left of or above the client area rounds toward it: -1
    // unit is -1 pixel across and down, -9 units -13 and -14.
    assert_eq!(rect(3), [17, 37, 83, 95]);
    assert_eq!(rect(4), [5, 24, 71, 82]);
    // DS_CONTROL takes the caption and the frame away from a popup and
    // a child; a child dialog without it keeps its caption.
    assert_eq!(rect(5), [48, 71, 108, 104]);
    assert_eq!(rect(6), [18, 38, 84, 96]);
    assert_eq!(rect(7), [93, 38, 153, 71]);
    // A popup past the right of the screen ends at its edge; one past
    // the bottom ends 4 pixels above the bottom of the work area, which
    // Windows 98 SE had at 452 and the built-in Windows has at 480.
    assert_eq!(rect(8), [574, 16, 640, 74]);
    assert_eq!(rect(9), [15, 418, 81, 476]);
}
