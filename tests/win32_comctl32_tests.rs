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
    ("USER32.DLL", &["SetTimer", "GetActiveWindow", "PostMessageA"]),
    ("KERNEL32.DLL", &["GetModuleHandleA"]),
];

/// The import slot of `name`.
fn import(l: &Layout, name: &str) -> AsmMemoryOperand {
    let index = IMPORTS
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
    /// window: what ends a modal dialog.
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
    let l = Layout {
        data: BASE + data_rva,
        iat,
    };
    let code_rva = b.next_rva();
    // The timer procedure, first in the code.
    let ok = BASE + code_rva;
    let mut a = CodeAssembler::new(32).unwrap();
    a.call(import(&l, "GetActiveWindow")).unwrap();
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
    let (exe, at) = program(data, calls);
    let mut cpu = machine(test, exe);
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
        call("ImageList_Create", &[I(16), I(16), I(ILC_COLOR16), I(1), I(4)]),
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
