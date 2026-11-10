//! IMAGEHLP.DLL's ReBaseImage: a program moves DLL files on C: to new
//! image bases, as MDK2 does to OPENGL32.DLL before it loads it. The
//! program writes what each call returned (its result, the last error
//! and the four values it gives back) to RESULT.BIN, and the tests read
//! that and the DLL files afterwards. The program and the DLLs are made
//! with rust-dos-pe's builder, the program's code assembled with iced.
//! The same program and DLLs (`write_probe` writes them) were run from a
//! DOS box of a booted Windows 98 SE, after `attrib +r K.DLL`: its
//! IMAGEHLP gave the results the tests expect, and left the DLL files
//! the same, byte for byte, as the built-in one does.

use iced_x86::code_asm::*;
use rust_dos::config::WindowsVersion;
use rust_dos::cpu::{Cpu, HostConsole};
use rust_dos::exec::{NoHook, StopReason, run_batch};
use rust_dos_pe::{self as pe, Builder, Headers, Imp};
use std::fs;
use std::path::PathBuf;

const BASE: u32 = 0x40_0000;

const GENERIC_WRITE: u32 = 0x4000_0000;
const CREATE_ALWAYS: u32 = 2;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;

/// What the program sets the last error to before each call, so that a
/// call that leaves it shows.
const UNTOUCHED_ERROR: u32 = 0xDEAD;
/// What the program puts in the values a call gives back before it, so
/// that one the call leaves shows.
const UNTOUCHED: u32 = 0xCCCC_CCCC;

/// One call of ReBaseImage(name, "", rebase, system, down, check, ...,
/// &new_base, stamp).
#[derive(Clone, Copy)]
struct Call {
    name: &'static str,
    rebase: bool,
    system: bool,
    down: bool,
    check: u32,
    new_base: u32,
    stamp: u32,
}

/// What a call gave: its result, the last error, and the old size, old
/// base, new size and new base.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Outcome {
    result: u32,
    error: u32,
    old_size: u32,
    old_base: u32,
    new_size: u32,
    new_base: u32,
}

// The program's data: the names, the empty symbol path, the result
// file's name, the count WriteFile gives, and a result block per call.
const NAMES: u32 = 0;
const NAME_SIZE: u32 = 0x20;
const SYMBOLS: u32 = 0x200;
const RESULT_NAME: u32 = 0x210;
const WRITTEN: u32 = 0x240;
const RESULTS: u32 = 0x400;
const RESULT_SIZE: u32 = 32;

/// A program that makes `calls` in turn and writes RESULT.BIN.
fn program(calls: &[Call]) -> Vec<u8> {
    let mut data = vec![0u8; (RESULTS + RESULT_SIZE * calls.len() as u32) as usize];
    for (i, call) in calls.iter().enumerate() {
        let at = (NAMES + NAME_SIZE * i as u32) as usize;
        data[at..at + call.name.len()].copy_from_slice(call.name.as_bytes());
    }
    let result = b"RESULT.BIN";
    data[RESULT_NAME as usize..][..result.len()].copy_from_slice(result);

    let kernel32 = [
        "SetLastError",
        "GetLastError",
        "CreateFileA",
        "WriteFile",
        "CloseHandle",
    ];
    let mut b = Builder::exe(BASE);
    let k: Vec<Imp> = kernel32.iter().map(|n| Imp::Name(n)).collect();
    let iat = b.imports(&[
        ("KERNEL32.DLL", &k),
        ("IMAGEHLP.DLL", &[Imp::Name("ReBaseImage")]),
    ]);
    let import = |name: &str| {
        let i = kernel32.iter().position(|n| *n == name).unwrap();
        dword_ptr(iat[0][i] as u64)
    };
    let rebase_image = dword_ptr(iat[1][0] as u64);
    let data_va = BASE
        + b.section(
            ".data",
            &data,
            pe::SCN_INITIALIZED_DATA | pe::SCN_READ | pe::SCN_WRITE,
        );
    let code_rva = b.next_rva();
    let mut a = CodeAssembler::new(32).unwrap();
    let at = |offset: u32| data_va + offset;
    let var = |offset: u32| dword_ptr(at(offset) as u64);
    (|| -> Result<(), IcedError> {
        for (i, call) in calls.iter().enumerate() {
            let block = RESULTS + RESULT_SIZE * i as u32;
            for field in 2..5 {
                a.mov(var(block + 4 * field), UNTOUCHED as i32)?;
            }
            a.mov(var(block + 20), call.new_base as i32)?;
            a.push(UNTOUCHED_ERROR as i32)?;
            a.call(import("SetLastError"))?;
            let args = [
                at(NAMES + NAME_SIZE * i as u32),
                at(SYMBOLS),
                call.rebase as u32,
                call.system as u32,
                call.down as u32,
                call.check,
                at(block + 8),
                at(block + 12),
                at(block + 16),
                at(block + 20),
                call.stamp,
            ];
            for &arg in args.iter().rev() {
                a.push(arg as i32)?;
            }
            a.call(rebase_image)?;
            a.mov(var(block), eax)?;
            a.call(import("GetLastError"))?;
            a.mov(var(block + 4), eax)?;
        }
        let create = [
            at(RESULT_NAME),
            GENERIC_WRITE,
            0,
            0,
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            0,
        ];
        for &arg in create.iter().rev() {
            a.push(arg as i32)?;
        }
        a.call(import("CreateFileA"))?;
        a.mov(esi, eax)?;
        a.push(0)?;
        a.push(at(WRITTEN) as i32)?;
        a.push((RESULT_SIZE * calls.len() as u32) as i32)?;
        a.push(at(RESULTS) as i32)?;
        a.push(esi)?;
        a.call(import("WriteFile"))?;
        a.push(esi)?;
        a.call(import("CloseHandle"))?;
        a.xor(eax, eax)?;
        a.ret()
    })()
    .unwrap();
    let code = a.assemble((BASE + code_rva) as u64).unwrap();
    b.section(
        ".text",
        &code,
        pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ,
    );
    b.entry(code_rva);
    b.build()
}

/// The image base of the DLLs the tests move.
const DLL_BASE: u32 = 0x1000_0000;
/// The addresses in a DLL's data section: three with base relocations,
/// then a number without one.
const DLL_WORDS: [u32; 4] = [
    DLL_BASE + 0x1000,
    DLL_BASE + 0x1008,
    DLL_BASE + 0x2000,
    0x1234_5678,
];

/// A DLL at DLL_BASE whose data section holds DLL_WORDS, and `bss`
/// bytes of zeros in memory after it. Its sections are 200h-aligned in
/// the file, as linkers write them, so a section's file offset isn't
/// its RVA. It has the file header's `characteristics` too, and the
/// checksum CheckSumMappedFile gives when `checksum`.
fn dll(name: &str, relocated: bool, bss: u32, characteristics: u16, checksum: bool) -> Vec<u8> {
    let mut b = Builder::dll(DLL_BASE, name);
    let words: Vec<u8> = DLL_WORDS.iter().flat_map(|w| w.to_le_bytes()).collect();
    let rva = b.section(
        ".data",
        &words,
        pe::SCN_INITIALIZED_DATA | pe::SCN_READ | pe::SCN_WRITE,
    );
    if bss != 0 {
        b.section_sized(
            ".bss",
            &[],
            bss,
            pe::SCN_UNINITIALIZED_DATA | pe::SCN_READ | pe::SCN_WRITE,
        );
    }
    if relocated {
        for i in 0..3 {
            b.reloc(rva + 4 * i);
        }
    }
    let mut file = pack(&b.build());
    let coff = pe_offset(&file) + 4;
    let old = u16::from_le_bytes([file[coff + 18], file[coff + 19]]);
    file[coff + 18..coff + 20].copy_from_slice(&(old | characteristics).to_le_bytes());
    if checksum {
        let sum = pe::checksum(&file);
        file[coff + 20 + 64..][..4].copy_from_slice(&sum.to_le_bytes());
    }
    file
}

fn pe_offset(file: &[u8]) -> usize {
    u32::from_le_bytes(file[0x3C..0x40].try_into().unwrap()) as usize
}

/// The builder's file, whose sections are at their RVAs, with its
/// headers in 400h bytes and each section's data 200h-aligned after
/// them.
fn pack(built: &[u8]) -> Vec<u8> {
    let headers = Headers::parse(built).unwrap();
    let opt = pe_offset(built) + 24;
    let table = opt + 0xE0;
    let mut out = built[..0x400].to_vec();
    out[opt + 36..opt + 40].copy_from_slice(&0x200u32.to_le_bytes());
    out[opt + 60..opt + 64].copy_from_slice(&0x400u32.to_le_bytes());
    for (i, s) in headers.sections.iter().enumerate() {
        let at = table + 40 * i;
        let size = s.raw_size.min(s.virtual_size.next_multiple_of(0x200));
        let offset = if s.raw_offset == 0 {
            0
        } else {
            out.len() as u32
        };
        let size = if offset == 0 { 0 } else { size };
        out[at + 16..at + 20].copy_from_slice(&size.to_le_bytes());
        out[at + 20..at + 24].copy_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(&built[s.raw_offset as usize..][..size as usize]);
    }
    out
}

/// A C: drive for the test `name` with `files` on it.
fn scratch(name: &str, files: &[(&str, Vec<u8>)]) -> PathBuf {
    let base = PathBuf::from("target/test_win32_imagehlp").join(name);
    // A read-only file of an earlier run would stay.
    for entry in fs::read_dir(&base).into_iter().flatten().flatten() {
        let mut permissions = entry.metadata().unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        let _ = fs::set_permissions(entry.path(), permissions);
    }
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    for (file, bytes) in files {
        fs::write(base.join(file), bytes).unwrap();
    }
    base
}

/// Run the program making `calls` as C:\T.EXE, with `files` on C: and
/// the one named `read_only` read-only, as Windows 98 SE: what each call
/// gave, and C:'s folder.
fn run(
    test: &str,
    calls: &[Call],
    files: &[(&str, Vec<u8>)],
    read_only: Option<&str>,
) -> (Vec<Outcome>, PathBuf) {
    let mut all = files.to_vec();
    all.push(("T.EXE", program(calls)));
    let drive = scratch(test, &all);
    if let Some(name) = read_only {
        let mut permissions = fs::metadata(drive.join(name)).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(drive.join(name), permissions).unwrap();
    }
    let mut cpu = Cpu::new(drive.clone());
    cpu.bus.win32.enabled = true;
    cpu.bus.win32.version = WindowsVersion::Win98Se;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    let mut ended = false;
    for _ in 0..400 {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        if run_batch(&mut cpu, &mut NoHook, false) == StopReason::ShellReloaded {
            ended = true;
            break;
        }
    }
    let console = cpu.host_console.as_ref().unwrap();
    let printed = [&console.stdout[..], &console.stderr[..]].concat();
    assert!(ended, "the program didn't end");
    assert!(printed.is_empty(), "{}", String::from_utf8_lossy(&printed));
    assert_eq!(cpu.errorlevel, 0);
    let result = fs::read(drive.join("RESULT.BIN")).unwrap();
    let word = |at: usize| u32::from_le_bytes(result[at..at + 4].try_into().unwrap());
    let outcomes = (0..calls.len())
        .map(|i| {
            let at = RESULT_SIZE as usize * i;
            Outcome {
                result: word(at),
                error: word(at + 4),
                old_size: word(at + 8),
                old_base: word(at + 12),
                new_size: word(at + 16),
                new_base: word(at + 20),
            }
        })
        .collect();
    (outcomes, drive)
}

/// The fields of a DLL file that ReBaseImage changes: the image base,
/// the time stamp, the checksum and DLL_WORDS.
#[derive(Debug, PartialEq, Eq)]
struct Fields {
    image_base: u32,
    stamp: u32,
    checksum: u32,
    words: [u32; 4],
}

fn fields(file: &[u8]) -> Fields {
    let coff = pe_offset(file) + 4;
    let word = |at: usize| u32::from_le_bytes(file[at..at + 4].try_into().unwrap());
    let headers = Headers::parse(file).unwrap();
    let data = headers.sections[0].raw_offset as usize;
    Fields {
        image_base: word(coff + 20 + 28),
        stamp: word(coff + 4),
        checksum: word(coff + 20 + 64),
        words: [0, 1, 2, 3].map(|i| word(data + 4 * i)),
    }
}

fn call(name: &'static str) -> Call {
    Call {
        name,
        rebase: true,
        system: true,
        down: true,
        check: 0,
        new_base: 0x6450_0000,
        stamp: 1,
    }
}

const IMAGE_FILE_SYSTEM: u16 = 0x1000;

/// The DLLs the calls of `calls()` move, by name.
fn dlls() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("A.DLL", dll("A.DLL", true, 0x12000, 0, true)),
        ("B.DLL", dll("B.DLL", true, 0, 0, false)),
        ("C.DLL", dll("C.DLL", true, 0, 0, false)),
        ("D.DLL", dll("D.DLL", false, 0, 0, false)),
        ("E.DLL", dll("E.DLL", true, 0, IMAGE_FILE_SYSTEM, false)),
        ("F.DLL", dll("F.DLL", true, 0, 0, false)),
        ("G.DLL", dll("G.DLL", true, 0, 0, false)),
        ("H.EXE", dll("H.DLL", true, 0, 0, false)),
        ("I.DLL", stamped(dll("I.DLL", true, 0, 0, false))),
        ("J.DLL", dll("J.DLL", true, 0, 0, false)),
        ("K.DLL", dll("K.DLL", true, 0, 0, false)),
        ("L.DLL", dll("L.DLL", false, 0, 0, false)),
        ("M.DLL", b"This is no program.\r\n".to_vec()),
    ]
}

/// A DLL file with the time stamp 12345678h.
fn stamped(mut file: Vec<u8>) -> Vec<u8> {
    let at = pe_offset(&file) + 8;
    file[at..at + 4].copy_from_slice(&0x1234_5678u32.to_le_bytes());
    file
}

fn calls() -> Vec<Call> {
    vec![
        // MDK2's call, on a DLL of 15000h bytes.
        call("A.DLL"),
        // Up, without a time stamp.
        Call {
            down: false,
            new_base: 0x2000_0000,
            stamp: 0,
            ..call("B.DLL")
        },
        // Worked out, not done.
        Call {
            rebase: false,
            stamp: 5,
            ..call("C.DLL")
        },
        // No relocations.
        Call {
            stamp: 7,
            ..call("D.DLL")
        },
        // A system file, which isn't to be moved.
        Call {
            system: false,
            stamp: 9,
            ..call("E.DLL")
        },
        // Larger than the size it may have.
        Call {
            check: 0x1000,
            stamp: 11,
            ..call("F.DLL")
        },
        // Where it is already.
        Call {
            new_base: DLL_BASE + 0x1_0000,
            stamp: 13,
            ..call("G.DLL")
        },
        // A name without an extension.
        call("H"),
        call("NOFILE.DLL"),
        // Up, without a time stamp, from one.
        Call {
            down: false,
            new_base: 0x3000_0000,
            stamp: 0,
            ..call("I.DLL")
        },
        // Up, worked out and not done.
        Call {
            rebase: false,
            down: false,
            new_base: 0x3000_0000,
            ..call("J.DLL")
        },
        // A read-only file (the tests make it one).
        call("K.DLL"),
        // No relocations, worked out and not done.
        Call {
            rebase: false,
            ..call("L.DLL")
        },
        // No program.
        call("M.DLL"),
    ]
}

/// Writes the program and the DLLs of `calls()` to the folder that
/// RUST_DOS_IMAGEHLP_PROBE names, to be run on a booted Windows 98:
/// T.EXE writes RESULT.BIN next to the DLLs.
#[test]
#[ignore]
fn write_probe() {
    let dir = PathBuf::from(std::env::var("RUST_DOS_IMAGEHLP_PROBE").unwrap());
    fs::create_dir_all(&dir).unwrap();
    for (name, bytes) in dlls() {
        fs::write(dir.join(name), bytes).unwrap();
    }
    fs::write(dir.join("T.EXE"), program(&calls())).unwrap();
    fs::write(dir.join("O.EXE"), program(&[call("OPENGL32.DLL")])).unwrap();
}

/// What Windows 98 SE's ReBaseImage gave for each of `calls()`.
fn windows_98_outcomes() -> Vec<Outcome> {
    let failed = |error, sizes: Option<(u32, u32, u32)>| {
        let (old_size, old_base, new_size) = sizes.unwrap_or((UNTOUCHED, UNTOUCHED, UNTOUCHED));
        Outcome {
            result: 0,
            error,
            old_size,
            old_base,
            new_size,
            new_base: 0x6450_0000,
        }
    };
    let ok = |old_size, new_size, new_base| Outcome {
        result: 1,
        error: 0,
        old_size,
        old_base: DLL_BASE,
        new_size,
        new_base,
    };
    vec![
        ok(0x1_5000, 0x2_0000, 0x644E_0000),
        ok(0x3000, 0x1_0000, 0x2001_0000),
        ok(0x3000, 0x1_0000, 0x6450_0000),
        failed(ERROR_INVALID_ADDRESS, Some((0x2000, DLL_BASE, 0x1_0000))),
        ok(0x3000, 0x1_0000, 0x644F_0000),
        ok(0x3000, 0x1_0000, 0x644F_0000),
        ok(0x3000, 0x1_0000, DLL_BASE),
        ok(0x3000, 0x1_0000, 0x644F_0000),
        failed(ERROR_FILE_NOT_FOUND, None),
        ok(0x3000, 0x1_0000, 0x3001_0000),
        ok(0x3000, 0x1_0000, 0x3000_0000),
        failed(ERROR_ACCESS_DENIED, None),
        ok(0x2000, 0x1_0000, 0x6450_0000),
        failed(0, None),
    ]
}

const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_INVALID_ADDRESS: u32 = 487;

/// What a DLL file moved to `base` with the time stamp `stamp` holds, and
/// the checksum Windows 98 SE wrote.
fn moved(base: u32, stamp: u32, checksum: u32) -> Fields {
    let delta = base.wrapping_sub(DLL_BASE);
    Fields {
        image_base: base,
        stamp,
        checksum,
        words: [
            DLL_WORDS[0].wrapping_add(delta),
            DLL_WORDS[1].wrapping_add(delta),
            DLL_WORDS[2].wrapping_add(delta),
            DLL_WORDS[3],
        ],
    }
}

/// The calls of `calls()` give what they gave on Windows 98 SE, and the
/// files are the same as Windows 98 SE left them: moved down below
/// 64500000h (A, E, F, H, K) or up to 20000000h or 30000000h (B, I),
/// with their addresses, image base, time stamp and checksum changed;
/// the others as they were.
#[test]
fn rebase_image_moves_files_as_windows_98_se_does() {
    let files = dlls();
    let (outcomes, drive) = run("moves", &calls(), &files, Some("K.DLL"));
    // Root on Unix writes read-only files anyway: K.DLL isn't checked.
    let root = fs::OpenOptions::new()
        .write(true)
        .open(drive.join("K.DLL"))
        .is_ok();
    for (i, (outcome, expected)) in outcomes.iter().zip(windows_98_outcomes()).enumerate() {
        if !(root && calls()[i].name == "K.DLL") {
            assert_eq!(*outcome, expected, "{}", calls()[i].name);
        }
    }
    let changed = [
        ("A.DLL", moved(0x644E_0000, 1, 0xEA8F)),
        ("B.DLL", moved(0x2000_0000, 0, 0xA32D)),
        ("E.DLL", moved(0x644F_0000, 9, 0xC473)),
        ("F.DLL", moved(0x644F_0000, 11, 0xB475)),
        ("H.EXE", moved(0x644F_0000, 1, 0xB46B)),
        ("I.DLL", moved(0x3000_0000, 0x1234_5678, 0x4BDA)),
    ];
    for (name, original) in &files {
        let now = fs::read(drive.join(name)).unwrap();
        match changed.iter().find(|(n, _)| n == name) {
            Some((_, fields_now)) => {
                assert_eq!(fields(&now), *fields_now, "{}", name);
                // Nothing else changed.
                let mut back = now.clone();
                let was = fields(original);
                let at = pe_offset(&now) + 4;
                back[at + 4..at + 8].copy_from_slice(&was.stamp.to_le_bytes());
                back[at + 48..at + 52].copy_from_slice(&was.image_base.to_le_bytes());
                back[at + 84..at + 88].copy_from_slice(&was.checksum.to_le_bytes());
                let data = Headers::parse(&now).unwrap().sections[0].raw_offset as usize;
                for (i, word) in was.words.iter().enumerate() {
                    back[data + 4 * i..][..4].copy_from_slice(&word.to_le_bytes());
                }
                assert!(back == *original, "{} has other changes", name);
            }
            None if root && *name == "K.DLL" => {}
            None => assert!(now == *original, "{} changed", name),
        }
    }
}

/// MDK2 moves OPENGL32.DLL below 64500000h before it loads it. On
/// Windows 98 that is the system's file, which ReBaseImage changes; the
/// built-in OPENGL32.DLL's file is read-only, so the call fails as it
/// does for any read-only file, and the game, which ignores the result,
/// goes on to load it.
#[test]
fn rebasing_the_built_in_opengl32_is_denied() {
    let (outcomes, _) = run("opengl32", &[call("OPENGL32.DLL")], &[], None);
    assert_eq!(
        outcomes,
        [Outcome {
            result: 0,
            error: ERROR_ACCESS_DENIED,
            old_size: UNTOUCHED,
            old_base: UNTOUCHED,
            new_size: UNTOUCHED,
            new_base: 0x6450_0000,
        }]
    );
}
