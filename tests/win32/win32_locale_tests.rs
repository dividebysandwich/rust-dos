//! The system locale of the built-in Windows (`win32_locale`): the code
//! pages, locale information and fonts a program sees under a localized
//! Windows 98, made by programs built here with rust-dos-pe's builder, as
//! in win32_comctl32_tests.rs. Each program makes its calls one after
//! another, keeps what each returned in its data, and spins; the test
//! reads the results while the program is still there.

use iced_x86::code_asm::*;
use rust_dos::config::Win32Locale;
use rust_dos::cpu::{Cpu, HostConsole};
use rust_dos::exec::{NoHook, run_batch};
use rust_dos_pe::{self as pe, Builder, Imp};
use std::fs;
use std::path::PathBuf;

const BASE: u32 = 0x40_0000;

/// The results of the calls, in order.
const RESULTS: u32 = 0x40;
const MAX_CALLS: u32 = 64;
const DONE: u32 = RESULTS + 4 * MAX_CALLS;
const DATA_SIZE: usize = 0x2000;

const IMPORTS: &[(&str, &[&str])] = &[
    (
        "KERNEL32.DLL",
        &[
            "GetACP",
            "GetOEMCP",
            "GetSystemDefaultLCID",
            "MultiByteToWideChar",
            "WideCharToMultiByte",
            "GetLocaleInfoA",
            "CompareStringA",
        ],
    ),
    ("USER32.DLL", &["CharUpperBuffA"]),
    (
        "GDI32.DLL",
        &[
            "CreateCompatibleDC",
            "CreateDIBSection",
            "SelectObject",
            "SetTextColor",
            "SetBkMode",
            "TextOutA",
            "TextOutW",
            "GetTextCharset",
            "CreateFontA",
        ],
    ),
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

/// A program making `calls`, with `data` (DATA_SIZE bytes), and where its
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
    let data_at = BASE + data_rva;
    let at = |offset: u32| (data_at + offset) as u64;
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
                Arg::At(o) => a.push(at(o) as i32).unwrap(),
                Arg::Res(r) => a.push(dword_ptr(at(RESULTS + 4 * r as u32))).unwrap(),
            }
        }
        a.call(import(name)).unwrap();
        a.mov(dword_ptr(at(RESULTS + 4 * i as u32)), eax).unwrap();
    }
    a.mov(dword_ptr(at(DONE)), 1u32).unwrap();
    let mut again = a.create_label();
    a.set_label(&mut again).unwrap();
    a.jmp(again).unwrap();
    let code = a.assemble((BASE + code_rva) as u64).unwrap();
    let flags = pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ;
    assert_eq!(b.section(".text", &code, flags), code_rva);
    b.entry(code_rva);
    (b.build(), data_at)
}

/// Run `calls` with `data` under the system locale `locale`: the
/// machine, still running the program, and where its data is.
fn run(test: &str, locale: Win32Locale, data: &[u8], calls: &[Call]) -> (Cpu, u32) {
    let (exe, at) = program(data, calls);
    let base = PathBuf::from("target/test_win32_locale").join(test);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join("T.EXE"), exe).unwrap();
    let mut cpu = Cpu::new(base);
    cpu.bus.win32.enabled = true;
    cpu.bus.win32.locale = locale;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
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

fn peek(cpu: &Cpu, lin: u32) -> u32 {
    cpu.bus.read_32(physical(cpu, lin))
}

fn peek_bytes(cpu: &Cpu, lin: u32, n: u32) -> Vec<u8> {
    (0..n)
        .map(|i| cpu.bus.read_8(physical(cpu, lin + i)))
        .collect()
}

fn results(cpu: &Cpu, data: u32, n: usize) -> Vec<u32> {
    (0..n as u32)
        .map(|i| peek(cpu, data + RESULTS + 4 * i))
        .collect()
}

fn put(data: &mut [u8], offset: u32, bytes: &[u8]) {
    data[offset as usize..offset as usize + bytes.len()].copy_from_slice(bytes);
}

/// Where the data's strings and structures are: "При" in code page
/// 1251, the conversions' buffers, the month's name, "приz" for
/// CharUpperBuffA, Д in 1251 and in UTF-16, the DIB's BITMAPINFO and its
/// pixels' pointer.
const S_RUSSIAN: u32 = 0x400;
const S_WIDE: u32 = 0x440;
const S_BACK: u32 = 0x480;
const S_MONTH: u32 = 0x4C0;
const S_UPPER: u32 = 0x500;
const S_DE: u32 = 0x540;
const S_DE_WIDE: u32 = 0x548;
const BMI: u32 = 0x600;
const BITS: u32 = 0x680;

const CP_ACP: u32 = 0;
const LOCALE_USER_DEFAULT: u32 = 0x0400;
const LOCALE_SMONTHNAME1: u32 = 0x38;

/// The calls that read a locale's code pages and convert Russian text.
fn code_page_calls() -> (Vec<u8>, Vec<Call>) {
    use Arg::*;
    let mut data = vec![0u8; DATA_SIZE];
    put(&mut data, S_RUSSIAN, b"\xCF\xF0\xE8\0");
    put(&mut data, S_UPPER, b"\xEF\xF0\xE8z\0");
    let calls = vec![
        call("GetACP", &[]),
        call("GetOEMCP", &[]),
        call("GetSystemDefaultLCID", &[]),
        call(
            "MultiByteToWideChar",
            &[
                I(CP_ACP),
                I(0),
                At(S_RUSSIAN),
                I(-1i32 as u32),
                At(S_WIDE),
                I(16),
            ],
        ),
        call(
            "WideCharToMultiByte",
            &[
                I(CP_ACP),
                I(0),
                At(S_WIDE),
                I(-1i32 as u32),
                At(S_BACK),
                I(16),
                I(0),
                I(0),
            ],
        ),
        call(
            "GetLocaleInfoA",
            &[
                I(LOCALE_USER_DEFAULT),
                I(LOCALE_SMONTHNAME1),
                At(S_MONTH),
                I(32),
            ],
        ),
        call("CharUpperBuffA", &[At(S_UPPER), I(4)]),
    ];
    (data, calls)
}

#[test]
fn a_russian_windows_has_its_code_pages_and_names() {
    let (data, calls) = code_page_calls();
    let (cpu, at) = run("russian_code_pages", Win32Locale::RuRu, &data, &calls);
    let r = results(&cpu, at, calls.len());
    assert_eq!(&r[..3], &[1251, 866, 0x0419]);
    // MultiByteToWideChar reads 1251: П, р, и.
    assert_eq!(r[3], 4);
    let wide = peek_bytes(&cpu, at + S_WIDE, 8);
    assert_eq!(wide, [0x1F, 0x04, 0x40, 0x04, 0x38, 0x04, 0, 0]);
    // And back.
    assert_eq!(r[4], 4);
    assert_eq!(peek_bytes(&cpu, at + S_BACK, 4), b"\xCF\xF0\xE8\0");
    // January, "Январь", in 1251.
    assert_eq!(
        peek_bytes(&cpu, at + S_MONTH, 7),
        b"\xDF\xED\xE2\xE0\xF0\xFC\0"
    );
    // CharUpperBuffA knows the Cyrillic letters' cases: "приz" to "ПРИZ".
    assert_eq!(peek_bytes(&cpu, at + S_UPPER, 4), b"\xCF\xD0\xC8Z");
}

#[test]
fn a_us_windows_keeps_code_page_1252() {
    let (data, calls) = code_page_calls();
    let (cpu, at) = run("us_code_pages", Win32Locale::EnUs, &data, &calls);
    let r = results(&cpu, at, calls.len());
    assert_eq!(&r[..3], &[1252, 437, 0x0409]);
    // The same bytes are Ï, ð and è.
    let wide = peek_bytes(&cpu, at + S_WIDE, 6);
    assert_eq!(wide, [0xCF, 0, 0xF0, 0, 0xE8, 0]);
    assert_eq!(peek_bytes(&cpu, at + S_MONTH, 8), b"January\0");
}

/// A 64x16 DIB of 32 bits a pixel, top-down, its pixels at BITS's
/// pointer: Д drawn by TextOutA from its byte in the system's code page
/// at x 0, and by TextOutW at x 32, in white on black. The pixels and
/// the DC's character set.
fn draw_de(test: &str, locale: Win32Locale) -> (Vec<u8>, u32) {
    use Arg::*;
    let mut data = vec![0u8; DATA_SIZE];
    put(&mut data, S_DE, b"\xC4\0");
    put(&mut data, S_DE_WIDE, &[0x14, 0x04, 0, 0]);
    let mut bmi = Vec::new();
    for v in [40u32, 64, -16i32 as u32] {
        bmi.extend_from_slice(&v.to_le_bytes());
    }
    bmi.extend_from_slice(&1u16.to_le_bytes());
    bmi.extend_from_slice(&32u16.to_le_bytes());
    put(&mut data, BMI, &bmi);
    let calls = vec![
        call("CreateCompatibleDC", &[I(0)]),
        call(
            "CreateDIBSection",
            &[Res(0), At(BMI), I(0), At(BITS), I(0), I(0)],
        ),
        call("SelectObject", &[Res(0), Res(1)]),
        call("SetTextColor", &[Res(0), I(0xFF_FFFF)]),
        call("SetBkMode", &[Res(0), I(1)]),
        call("TextOutA", &[Res(0), I(0), I(0), At(S_DE), I(1)]),
        call("TextOutW", &[Res(0), I(32), I(0), At(S_DE_WIDE), I(1)]),
        call("GetTextCharset", &[Res(0)]),
    ];
    let (cpu, at) = run(test, locale, &data, &calls);
    let r = results(&cpu, at, calls.len());
    let bits = peek(&cpu, at + BITS);
    (peek_bytes(&cpu, bits, 64 * 16 * 4), r[7])
}

/// The lit pixels of the 16 columns from `x`, a row a u16.
fn glyph(pixels: &[u8], x: usize) -> Vec<u16> {
    (0..16)
        .map(|y| {
            (0..16).fold(0u16, |row, col| {
                let lit = pixels[(y * 64 + x + col) * 4] != 0;
                row | (lit as u16) << col
            })
        })
        .collect()
}

#[test]
fn a_russian_windows_draws_cyrillic_from_ansi_and_unicode() {
    let (ru, charset) = draw_de("russian_text", Win32Locale::RuRu);
    // The system font is Cyrillic (RUSSIAN_CHARSET).
    assert_eq!(charset, 204);
    let from_ansi = glyph(&ru, 0);
    assert!(from_ansi.iter().any(|&row| row != 0), "Д is drawn");
    assert_eq!(
        from_ansi,
        glyph(&ru, 32),
        "TextOutA and TextOutW draw the same Д"
    );
    // A US Windows draws the byte as Ä, and the Unicode Д as '?', since
    // its system font is Western.
    let (us, charset) = draw_de("us_text", Win32Locale::EnUs);
    assert_eq!(charset, 0);
    assert_ne!(glyph(&us, 0), from_ansi);
}
