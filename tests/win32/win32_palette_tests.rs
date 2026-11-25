//! What becomes of the foreground palette on the built-in Windows when
//! the window that selected it goes: destroyed, minimized, or with its
//! program. On a palette device Windows 98 SE's USER makes
//! DEFAULT_PALETTE the foreground palette then (RealizeDefaultPalette),
//! which frees the system palette's entries the old one took, and sends
//! WM_PALETTECHANGED to the top-level windows. The programs are PE files
//! made with rust-dos-pe's builder, their code assembled with iced; a
//! child program may be a Win16 one made byte by byte
//! (tests/common/win16).

use iced_x86::code_asm::*;
use rust_dos::config::DesktopDepth;
use rust_dos::cpu::{Cpu, HostConsole};
use rust_dos::exec::{NoHook, StopReason, run_batch};
use rust_dos_pe::{self as pe, Builder, Imp};
use std::fs;
use std::path::PathBuf;

use crate::win16;

const BASE: u32 = 0x40_0000;

/// Where a program's data and imports are, for its code.
struct Layout {
    data: u32,
    names: Vec<&'static str>,
    iat: Vec<u32>,
}

impl Layout {
    /// The address of byte `offset` of the data.
    fn at(&self, offset: u32) -> u64 {
        (self.data + offset) as u64
    }

    /// The import address table slot of the function `name`.
    fn f(&self, name: &str) -> AsmMemoryOperand {
        let i = self.names.iter().position(|n| *n == name).unwrap();
        dword_ptr(self.iat[i] as u64)
    }
}

/// An argument of `call`.
#[derive(Clone, Copy)]
enum Arg {
    /// A value.
    I(i32),
    /// The address of a place in the data.
    At(u32),
    /// The dword at a place in the data.
    Val(u32),
}

/// Call the function `name` with `args`, the first one first.
fn call(a: &mut CodeAssembler, l: &Layout, name: &str, args: &[Arg]) -> Result<(), IcedError> {
    for arg in args.iter().rev() {
        match *arg {
            Arg::I(v) => a.push(v)?,
            Arg::At(at) => a.push(l.at(at) as i32)?,
            Arg::Val(at) => a.push(dword_ptr(l.at(at)))?,
        }
    }
    a.call(l.f(name))
}

const KERNEL32: &[&str] = &[
    "ExitProcess",
    "CreateProcessA",
    "GetExitCodeProcess",
    "Sleep",
];
const USER32: &[&str] = &[
    "RegisterClassA",
    "CreateWindowExA",
    "DefWindowProcA",
    "GetDC",
    "DestroyWindow",
    "ShowWindow",
    "PeekMessageA",
    "DispatchMessageA",
    "GetDesktopWindow",
];
const GDI32: &[&str] = &[
    "CreatePalette",
    "SelectPalette",
    "RealizePalette",
    "CreateCompatibleDC",
    "GetSystemPaletteEntries",
];

/// A program importing KERNEL32, USER32 and GDI32's functions above,
/// with `data` and the code `f` makes.
fn program(
    data: &[u8],
    f: impl FnOnce(&mut CodeAssembler, &Layout) -> Result<(), IcedError>,
) -> Vec<u8> {
    let mut b = Builder::exe(BASE);
    let lists: Vec<Vec<Imp>> = [KERNEL32, USER32, GDI32]
        .iter()
        .map(|names| names.iter().map(|n| Imp::Name(n)).collect())
        .collect();
    let dlls: Vec<(&str, &[Imp])> = ["KERNEL32.DLL", "USER32.DLL", "GDI32.DLL"]
        .into_iter()
        .zip(&lists)
        .map(|(dll, list)| (dll, list.as_slice()))
        .collect();
    let iat = b.imports(&dlls).concat();
    let data_rva = b.section(
        ".data",
        data,
        pe::SCN_INITIALIZED_DATA | pe::SCN_READ | pe::SCN_WRITE,
    );
    let layout = Layout {
        data: BASE + data_rva,
        names: [KERNEL32, USER32, GDI32].concat(),
        iat,
    };
    let code_rva = b.next_rva();
    let mut a = CodeAssembler::new(32).unwrap();
    f(&mut a, &layout).unwrap();
    let code = a.assemble((BASE + code_rva) as u64).unwrap();
    b.section(
        ".text",
        &code,
        pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ,
    );
    b.entry(code_rva);
    b.build()
}

/// Run T.EXE of `files` to its end, on a desktop of 256 colours with
/// `palette`, else of 32 bits: its exit code.
fn run(test: &str, files: &[(&str, Vec<u8>)], palette: bool) -> u8 {
    let base = PathBuf::from("target/test_win32_palette").join(test);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    for (file, bytes) in files {
        fs::write(base.join(file), bytes).unwrap();
    }
    let mut cpu = Cpu::new(base);
    cpu.bus.win32.enabled = true;
    if palette {
        cpu.bus.win32.depth = DesktopDepth::Palette;
    }
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    for _ in 0..1000 {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        if run_batch(&mut cpu, &mut NoHook, false) == StopReason::ShellReloaded {
            return cpu.errorlevel;
        }
    }
    let console = cpu.host_console.as_ref().unwrap();
    panic!(
        "T.EXE didn't end: {}",
        String::from_utf8_lossy(&console.stdout)
    );
}

// The data's layout.

const CLASS: u32 = 0x00;
const CHILD: u32 = 0x10;
const STARTUP: u32 = 0x40;
const PROCESS: u32 = 0x90;
/// How many WM_PALETTECHANGED the listening window got, and the last
/// one's wParam.
const COUNT: u32 = 0xA0;
const WPARAM: u32 = 0xA4;
const EXIT_CODE: u32 = 0xA8;
const MSG: u32 = 0xB0;
const WNDCLASS: u32 = 0xE0;
const LISTENER: u32 = 0x110;
const WINDOW: u32 = 0x114;
const HDC: u32 = 0x118;
const PALETTE: u32 = 0x11C;
const MEMORY_DC: u32 = 0x120;
const SYSTEM: u32 = 0x130;
const LOGPALETTE_A: u32 = 0x200;
const LOGPALETTE_B: u32 = 0x220;

/// The colors of palettes A and B, none of them a static color.
const COLORS_A: [u32; 4] = [0x33_2211, 0x66_5544, 0x99_8877, 0xCC_BBAA];
const COLORS_B: [u32; 4] = [0x03_0201, 0x06_0504, 0x09_0807, 0x0C_0B0A];

const WM_PALETTECHANGED: i32 = 0x0311;
const WS_POPUP_VISIBLE: i32 = 0x9000_0000u32 as i32;
const SW_MINIMIZE: i32 = 6;
const STILL_ACTIVE: i32 = 259;

/// A LOGPALETTE of `colors` at `at` of `data`.
fn put_palette(data: &mut [u8], at: u32, colors: &[u32]) {
    let at = at as usize;
    data[at..at + 4].copy_from_slice(&(0x300 | (colors.len() as u32) << 16).to_le_bytes());
    for (i, c) in colors.iter().enumerate() {
        data[at + 4 + 4 * i..at + 8 + 4 * i].copy_from_slice(&c.to_le_bytes());
    }
}

/// T's data: the class's name, the child's command line, a STARTUPINFO
/// and the two LOGPALETTEs.
fn data() -> Vec<u8> {
    let mut data = vec![0u8; 0x300];
    data[..4].copy_from_slice(b"Pal\0");
    data[CHILD as usize..CHILD as usize + 6].copy_from_slice(b"C.EXE\0");
    data[STARTUP as usize] = 68;
    data[(WNDCLASS + 16) as usize..(WNDCLASS + 20) as usize].copy_from_slice(&BASE.to_le_bytes());
    put_palette(&mut data, LOGPALETTE_A, &COLORS_A);
    put_palette(&mut data, LOGPALETTE_B, &COLORS_B);
    data
}

/// Register the class whose window procedure counts the
/// WM_PALETTECHANGED messages of the window at LISTENER, and keeps the
/// last one's wParam.
fn register_class(a: &mut CodeAssembler, l: &Layout) -> Result<(), IcedError> {
    let mut start = a.create_label();
    a.call(start)?;
    let mut other = a.create_label();
    let mut done = a.create_label();
    a.cmp(dword_ptr(esp + 8), WM_PALETTECHANGED)?;
    a.jne(other)?;
    a.mov(eax, dword_ptr(esp + 4))?;
    a.cmp(eax, dword_ptr(l.at(LISTENER)))?;
    a.jne(done)?;
    a.inc(dword_ptr(l.at(COUNT)))?;
    a.mov(eax, dword_ptr(esp + 12))?;
    a.mov(dword_ptr(l.at(WPARAM)), eax)?;
    a.set_label(&mut done)?;
    a.xor(eax, eax)?;
    a.ret_1(16)?;
    a.set_label(&mut other)?;
    a.jmp(l.f("DefWindowProcA"))?;
    a.set_label(&mut start)?;
    a.pop(eax)?;
    a.mov(dword_ptr(l.at(WNDCLASS + 4)), eax)?;
    a.mov(dword_ptr(l.at(WNDCLASS + 36)), l.at(CLASS) as i32)?;
    call(a, l, "RegisterClassA", &[Arg::At(WNDCLASS)])
}

/// A visible popup window of the class, kept at `at`.
fn create_window(a: &mut CodeAssembler, l: &Layout, at: u32) -> Result<(), IcedError> {
    let class = Arg::At(CLASS);
    let args = [Arg::I(0), class, class, Arg::I(WS_POPUP_VISIBLE)];
    let place = [Arg::I(0), Arg::I(0), Arg::I(50), Arg::I(50)];
    let rest = [Arg::I(0), Arg::I(0), Arg::I(BASE as i32), Arg::I(0)];
    call(
        a,
        l,
        "CreateWindowExA",
        &[&args[..], &place, &rest].concat(),
    )?;
    a.mov(dword_ptr(l.at(at)), eax)
}

/// Dispatch the messages in the queue.
fn pump(a: &mut CodeAssembler, l: &Layout) -> Result<(), IcedError> {
    let mut next = a.create_label();
    let mut done = a.create_label();
    a.set_label(&mut next)?;
    let args = [Arg::At(MSG), Arg::I(0), Arg::I(0), Arg::I(0), Arg::I(1)];
    call(a, l, "PeekMessageA", &args)?;
    a.test(eax, eax)?;
    a.jz(done)?;
    call(a, l, "DispatchMessageA", &[Arg::At(MSG)])?;
    a.jmp(next)?;
    a.set_label(&mut done)?;
    a.nop()
}

/// Palette A from LOGPALETTE_A selected without bForceBackground into
/// the DC of the window at WINDOW, and realized.
fn realize_a(a: &mut CodeAssembler, l: &Layout) -> Result<(), IcedError> {
    call(a, l, "GetDC", &[Arg::Val(WINDOW)])?;
    a.mov(dword_ptr(l.at(HDC)), eax)?;
    call(a, l, "CreatePalette", &[Arg::At(LOGPALETTE_A)])?;
    a.mov(dword_ptr(l.at(PALETTE)), eax)?;
    call(
        a,
        l,
        "SelectPalette",
        &[Arg::Val(HDC), Arg::Val(PALETTE), Arg::I(0)],
    )?;
    call(a, l, "RealizePalette", &[Arg::Val(HDC)])
}

/// The end of T: palette B realized in a memory DC, which is in the
/// background, then the exit code: bit 0 set if the last
/// WM_PALETTECHANGED the listening window got had `gone` (a place in the
/// data) as wParam, bit 1 if it got any, bit 2 if B took the system
/// palette's entry 10, the first after the low static colors.
fn finish(a: &mut CodeAssembler, l: &Layout, gone: u32) -> Result<(), IcedError> {
    call(a, l, "CreateCompatibleDC", &[Arg::I(0)])?;
    a.mov(dword_ptr(l.at(MEMORY_DC)), eax)?;
    call(a, l, "CreatePalette", &[Arg::At(LOGPALETTE_B)])?;
    a.mov(dword_ptr(l.at(PALETTE)), eax)?;
    let select = [Arg::Val(MEMORY_DC), Arg::Val(PALETTE), Arg::I(1)];
    call(a, l, "SelectPalette", &select)?;
    call(a, l, "RealizePalette", &[Arg::Val(MEMORY_DC)])?;
    let read = [Arg::Val(MEMORY_DC), Arg::I(10), Arg::I(1), Arg::At(SYSTEM)];
    call(a, l, "GetSystemPaletteEntries", &read)?;
    a.xor(ebx, ebx)?;
    a.mov(eax, dword_ptr(l.at(WPARAM)))?;
    a.cmp(eax, dword_ptr(l.at(gone)))?;
    a.sete(bl)?;
    a.cmp(dword_ptr(l.at(COUNT)), 0)?;
    a.setne(al)?;
    a.shl(al, 1)?;
    a.or(bl, al)?;
    a.mov(eax, dword_ptr(l.at(SYSTEM)))?;
    a.and(eax, 0xFF_FFFF)?;
    a.cmp(eax, COLORS_B[0] as i32)?;
    a.sete(al)?;
    a.shl(al, 2)?;
    a.or(bl, al)?;
    a.push(ebx)?;
    a.call(l.f("ExitProcess"))?;
    a.int3()
}

/// How the window that realized palette A in the foreground goes.
#[derive(Clone, Copy)]
enum Gone {
    Destroyed,
    Minimized,
}

/// T: a listening window, then a window that realizes A in the
/// foreground and goes as `gone` says.
fn window_gone_program(gone: Gone) -> Vec<u8> {
    program(&data(), |a, l| {
        register_class(a, l)?;
        create_window(a, l, LISTENER)?;
        create_window(a, l, WINDOW)?;
        realize_a(a, l)?;
        pump(a, l)?;
        a.mov(dword_ptr(l.at(COUNT)), 0)?;
        a.mov(dword_ptr(l.at(WPARAM)), 0)?;
        match gone {
            Gone::Destroyed => call(a, l, "DestroyWindow", &[Arg::Val(WINDOW)])?,
            Gone::Minimized => call(a, l, "ShowWindow", &[Arg::Val(WINDOW), Arg::I(SW_MINIMIZE)])?,
        }
        pump(a, l)?;
        finish(a, l, WINDOW)
    })
}

/// T: a listening window, then C.EXE run to its end while T takes the
/// messages sent and posted to it.
fn child_exits_program() -> Vec<u8> {
    program(&data(), |a, l| {
        register_class(a, l)?;
        create_window(a, l, LISTENER)?;
        call(a, l, "GetDesktopWindow", &[])?;
        a.mov(dword_ptr(l.at(WINDOW)), eax)?;
        let create = [
            Arg::I(0),
            Arg::At(CHILD),
            Arg::I(0),
            Arg::I(0),
            Arg::I(0),
            Arg::I(0),
            Arg::I(0),
            Arg::I(0),
            Arg::At(STARTUP),
            Arg::At(PROCESS),
        ];
        call(a, l, "CreateProcessA", &create)?;
        let mut wait = a.create_label();
        let mut ended = a.create_label();
        a.set_label(&mut wait)?;
        a.nop()?;
        pump(a, l)?;
        let ask = [Arg::Val(PROCESS), Arg::At(EXIT_CODE)];
        call(a, l, "GetExitCodeProcess", &ask)?;
        a.cmp(dword_ptr(l.at(EXIT_CODE)), STILL_ACTIVE)?;
        a.jne(ended)?;
        call(a, l, "Sleep", &[Arg::I(10)])?;
        a.jmp(wait)?;
        a.set_label(&mut ended)?;
        a.nop()?;
        pump(a, l)?;
        finish(a, l, WINDOW)
    })
}

/// C.EXE: a window of the system's STATIC class that realizes A in the
/// foreground, and the end.
fn win32_child() -> Vec<u8> {
    let mut d = data();
    d[CLASS as usize..CLASS as usize + 7].copy_from_slice(b"STATIC\0");
    program(&d, |a, l| {
        create_window(a, l, WINDOW)?;
        realize_a(a, l)?;
        call(a, l, "ExitProcess", &[Arg::I(0)])?;
        a.int3()
    })
}

/// C.EXE as a Win16 program: a window of the system's BUTTON class that
/// realizes A with USER's SelectPalette and RealizePalette, and the end.
fn win16_child() -> Vec<u8> {
    let mut code = win16::Code::new();
    let class_at = code.here() + 2;
    code.push_far(0);
    let title_at = code.here() + 2;
    code.push_far(0)
        .push(0x9000) // WS_POPUP | WS_VISIBLE
        .push(0)
        .push(0)
        .push(0)
        .push(50)
        .push(50)
        .push(0) // no parent
        .push(0) // no menu
        .push(0) // instance
        .push(0)
        .push(0) // no parameter
        .call("USER", 41) // CreateWindow
        .emit(&[0x50]) // push ax
        .call("USER", 66) // GetDC
        .emit(&[0x8B, 0xF8]); // mov di,ax
    let palette_at = code.here() + 2;
    code.push_far(0)
        .call("GDI", 360) // CreatePalette
        .emit(&[0x57, 0x50]) // push di, push ax
        .push(0)
        .call("USER", 282) // SelectPalette
        .emit(&[0x57]) // push di
        .call("USER", 283) // RealizePalette
        .emit(&[0xB0, 0x00]) // mov al,0
        .exit_al();
    let class = code.here();
    code.emit(b"BUTTON\0");
    let title = code.here();
    code.emit(b"\0");
    let palette = code.here();
    let mut logpalette = vec![0u8; 4 + 4 * COLORS_A.len()];
    put_palette(&mut logpalette, 0, &COLORS_A);
    code.emit(&logpalette);
    code.patch(class_at, class);
    code.patch(title_at, title);
    code.patch(palette_at, palette);
    win16::program_named("C", &code, win16::CODE)
}

/// All three bits of `finish` on a desktop of 256 colours.
const RESTORED: u8 = 0b111;

#[test]
fn destroying_the_window_with_the_foreground_palette_frees_its_entries() {
    // Windows 98 SE's DestroyWindow of the active window, which selected
    // a palette on a palette device, calls USER's function at segment
    // 11, 195Eh, once the next window is active. That window has no
    // palette, so USER calls RealizeDefaultPalette on the screen and
    // sends WM_PALETTECHANGED to the top-level windows with the
    // destroyed window as wParam. A's entries are no palette's then, so
    // B, realized in the background, takes the first of them.
    let program = window_gone_program(Gone::Destroyed);
    let files = [("T.EXE", program)];
    assert_eq!(run("destroyed", &files, true), RESTORED);
    // On a desktop of 32 bits there is no system palette to free, and
    // no WM_PALETTECHANGED.
    assert_eq!(run("destroyed_deep", &files, false), 0);
}

#[test]
fn minimizing_the_window_with_the_foreground_palette_frees_its_entries() {
    // ShowWindow's minimizing of a top-level window that selected a
    // palette calls the same function: a minimized active window keeps
    // no palette in the foreground.
    let program = window_gone_program(Gone::Minimized);
    let files = [("T.EXE", program)];
    assert_eq!(run("minimized", &files, true), RESTORED);
    assert_eq!(run("minimized_deep", &files, false), 0);
}

#[test]
fn a_program_that_selected_a_palette_frees_its_entries_as_it_ends() {
    // USER's task exit (segment 16 at 2134h, from SignalProc32) calls it
    // for a task that selected a palette, with the desktop as
    // WM_PALETTECHANGED's wParam: the system palette's entries the
    // program's palette took are free for the programs left.
    let files = [("T.EXE", child_exits_program()), ("C.EXE", win32_child())];
    assert_eq!(run("child_exits", &files, true), RESTORED);
    assert_eq!(run("child_exits_deep", &files, false), 0);
}

#[test]
fn a_win16_program_that_selected_a_palette_frees_its_entries_as_it_ends() {
    // The same for a Windows 3.1 program, whose task ends.
    let files = [("T.EXE", child_exits_program()), ("C.EXE", win16_child())];
    assert_eq!(run("win16_child_exits", &files, true), RESTORED);
    assert_eq!(run("win16_child_exits_deep", &files, false), 0);
}
