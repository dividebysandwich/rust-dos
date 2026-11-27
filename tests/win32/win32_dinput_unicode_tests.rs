//! DirectInput's Unicode interfaces: DirectInputCreateW gives
//! IDirectInputW, whose devices are IDirectInputDeviceW, and the
//! structures their methods fill have WCHAR names, as on Windows. The
//! ANSI interfaces of the same objects, through QueryInterface, keep
//! their CHAR names, and an object's interfaces share its references.
//! The program is a PE file made with rust-dos-pe's builder, its code
//! assembled with iced; it ends with exit code 100 when every check
//! passed, or with the number of the check that failed.

use iced_x86::code_asm::*;
use rust_dos::cpu::{Cpu, HostConsole};
use rust_dos::exec::{NoHook, StopReason, run_batch};
use rust_dos_pe::{self as pe, Builder, Imp};
use std::fs;
use std::path::PathBuf;

const BASE: u32 = 0x40_0000;
const PASSED: u8 = 100;

const DIERR_INVALIDPARAM: u32 = 0x8007_0057;
const DIENUM_STOP: u32 = 0;
const DIDFT_ALL: u32 = 0;

/// The sizes of DIDEVICEINSTANCEW and DIDEVICEOBJECTINSTANCEW, and of
/// DIDEVICEINSTANCEA.
const INSTANCE_W: u32 = 1100;
const OBJECT_W: u32 = 576;
const INSTANCE_A: u32 = 580;

/// Methods, by their places in the vtables.
const QUERY_INTERFACE: u32 = 0;
const RELEASE: u32 = 2;
const CREATE_DEVICE: u32 = 3;
const ENUM_DEVICES: u32 = 4;
const ENUM_OBJECTS: u32 = 4;
const GET_DEVICE_INFO: u32 = 15;

/// The data's places: the interfaces, what the callbacks saw, the GUIDs
/// and IIDs, and a DIDEVICEINSTANCE.
const DI: u32 = 0x000;
const DI_A: u32 = 0x004;
const DI_7W: u32 = 0x008;
const DEVICE: u32 = 0x00C;
const DEVICE_A: u32 = 0x010;
const CALLS: u32 = 0x020;
const SEEN_SIZE: u32 = 0x024;
const SEEN_NAME: u32 = 0x028;
const SEEN_PRODUCT: u32 = 0x02C;
const OBJECT_CALLS: u32 = 0x030;
const OBJECT_SIZE: u32 = 0x034;
const OBJECT_NAME: u32 = 0x038;
const GUID_SYS_MOUSE: u32 = 0x040;
const IID_DI_A: u32 = 0x050;
const IID_DI_7W: u32 = 0x060;
const IID_DEVICE_A: u32 = 0x070;
const INFO: u32 = 0x100;
const DATA_SIZE: usize = 0x600;

/// The first two characters of a name as a WCHAR string's first dword.
const fn wide2(a: u8, b: u8) -> u32 {
    a as u32 | (b as u32) << 16
}

fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Vec<u8> {
    let mut g = d1.to_le_bytes().to_vec();
    g.extend(d2.to_le_bytes());
    g.extend(d3.to_le_bytes());
    g.extend(d4);
    g
}

const DI_TAIL: [u8; 8] = [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0];

fn data() -> Vec<u8> {
    let mut d = vec![0u8; DATA_SIZE];
    let mut put = |at: u32, bytes: &[u8]| {
        d[at as usize..at as usize + bytes.len()].copy_from_slice(bytes);
    };
    put(GUID_SYS_MOUSE, &guid(0x6F1D_2B60, 0xD5A0, 0x11CF, DI_TAIL));
    put(IID_DI_A, &guid(0x8952_1360, 0xAA8A, 0x11CF, DI_TAIL));
    let tail_7 = [0x8E, 0x9D, 0x00, 0xC0, 0x4F, 0x68, 0x44, 0xAE];
    put(IID_DI_7W, &guid(0x9A4C_B685, 0x236D, 0x11D3, tail_7));
    put(IID_DEVICE_A, &guid(0x5944_E680, 0xC92E, 0x11CF, DI_TAIL));
    d
}

struct Layout {
    data: u32,
    iat: Vec<u32>,
}

/// The functions the program imports, in this order.
const IMPORTS: &[&str] = &["ExitProcess", "DirectInputCreateW"];

impl Layout {
    fn at(&self, offset: u32) -> u32 {
        self.data + offset
    }

    fn import(&self, name: &str) -> AsmMemoryOperand {
        let i = IMPORTS.iter().position(|n| *n == name).unwrap();
        dword_ptr(self.iat[i] as u64)
    }
}

/// End with exit code `exit` unless the flags say equal.
fn unless_equal(a: &mut CodeAssembler, l: &Layout, exit: u8) -> Result<(), IcedError> {
    let mut ok = a.create_label();
    a.je(ok)?;
    a.push(exit as i32)?;
    a.call(l.import("ExitProcess"))?;
    a.set_label(&mut ok)
}

fn expect_eax(a: &mut CodeAssembler, l: &Layout, value: u32, exit: u8) -> Result<(), IcedError> {
    a.cmp(eax, value as i32)?;
    unless_equal(a, l, exit)
}

fn expect_dword(
    a: &mut CodeAssembler,
    l: &Layout,
    offset: u32,
    value: u32,
    exit: u8,
) -> Result<(), IcedError> {
    a.cmp(dword_ptr(l.at(offset) as u64), value as i32)?;
    unless_equal(a, l, exit)
}

/// Call the method `index` of the interface at `object` with `args`,
/// pushed last first: data offsets for `At`, values for `I`.
fn method(
    a: &mut CodeAssembler,
    l: &Layout,
    object: u32,
    index: u32,
    args: &[Arg],
) -> Result<(), IcedError> {
    for arg in args.iter().rev() {
        match *arg {
            Arg::I(v) => a.push(v as i32)?,
            Arg::At(o) => a.push(l.at(o) as i32)?,
        }
    }
    a.mov(eax, dword_ptr(l.at(object) as u64))?;
    a.push(eax)?;
    a.mov(ecx, dword_ptr(eax))?;
    a.call(dword_ptr(ecx + 4 * index))
}

enum Arg {
    I(u32),
    At(u32),
}

/// The program: the checks, then the two callbacks.
fn program() -> Vec<u8> {
    let mut b = Builder::exe(BASE);
    b.subsystem(pe::SUBSYSTEM_CONSOLE);
    let iat = b
        .imports(&[
            ("KERNEL32.dll", &[Imp::Name("ExitProcess")][..]),
            ("DINPUT.dll", &[Imp::Name("DirectInputCreateW")][..]),
        ])
        .concat();
    let data_rva = b.section(
        ".data",
        &data(),
        pe::SCN_INITIALIZED_DATA | pe::SCN_READ | pe::SCN_WRITE,
    );
    let l = Layout {
        data: BASE + data_rva,
        iat,
    };
    let code_rva = b.next_rva();
    let ip = (BASE + code_rva) as u64;
    // The callbacks' addresses are pushed as immediates of four bytes:
    // assemble once to learn them, then again with them.
    let options = iced_x86::BlockEncoderOptions::RETURN_NEW_INSTRUCTION_OFFSETS;
    let mut a = CodeAssembler::new(32).unwrap();
    let (first, second) = code(&mut a, &l, (ip as u32, ip as u32)).unwrap();
    let r = a.assemble_options(ip, options).unwrap();
    let callbacks = (
        r.label_ip(&first).unwrap() as u32,
        r.label_ip(&second).unwrap() as u32,
    );
    let mut a = CodeAssembler::new(32).unwrap();
    code(&mut a, &l, callbacks).unwrap();
    let code = a.assemble(ip).unwrap();
    b.section(
        ".text",
        &code,
        pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ,
    );
    b.entry(code_rva);
    b.build()
}

/// The code, with the callbacks at `callbacks`: their labels.
fn code(
    a: &mut CodeAssembler,
    l: &Layout,
    callbacks: (u32, u32),
) -> Result<(CodeLabel, CodeLabel), IcedError> {
    use Arg::{At, I};
    let mut device_seen = a.create_label();
    let mut object_seen = a.create_label();

    // DirectInputCreateW(hinst, 0x700, &DI, NULL).
    for arg in [0, l.at(DI), 0x700, BASE] {
        a.push(arg as i32)?;
    }
    a.call(l.import("DirectInputCreateW"))?;
    expect_eax(a, l, 0, 1)?;

    // IDirectInputW::EnumDevices(0, callback, 0, 0): the mouse first,
    // its names in WCHARs.
    a.push(0)?;
    a.push(0)?;
    a.push(callbacks.0 as i32)?;
    a.push(0)?;
    a.mov(eax, dword_ptr(l.at(DI) as u64))?;
    a.push(eax)?;
    a.mov(ecx, dword_ptr(eax))?;
    a.call(dword_ptr(ecx + 4 * ENUM_DEVICES))?;
    expect_eax(a, l, 0, 2)?;
    expect_dword(a, l, CALLS, 1, 3)?;
    expect_dword(a, l, SEEN_SIZE, INSTANCE_W, 4)?;
    expect_dword(a, l, SEEN_NAME, wide2(b'M', b'o'), 5)?;
    expect_dword(a, l, SEEN_PRODUCT, wide2(b'M', b'o'), 6)?;

    // The mouse through IDirectInputW::CreateDevice is
    // IDirectInputDeviceW: its DIDEVICEINSTANCEW, and not the A one.
    method(
        a,
        l,
        DI,
        CREATE_DEVICE,
        &[At(GUID_SYS_MOUSE), At(DEVICE), I(0)],
    )?;
    expect_eax(a, l, 0, 10)?;
    a.mov(dword_ptr(l.at(INFO) as u64), INSTANCE_W as i32)?;
    method(a, l, DEVICE, GET_DEVICE_INFO, &[At(INFO)])?;
    expect_eax(a, l, 0, 11)?;
    expect_dword(a, l, INFO + 40, wide2(b'M', b'o'), 12)?;
    expect_dword(a, l, INFO + 40 + 520, wide2(b'M', b'o'), 13)?;
    a.mov(dword_ptr(l.at(INFO) as u64), INSTANCE_A as i32)?;
    method(a, l, DEVICE, GET_DEVICE_INFO, &[At(INFO)])?;
    expect_eax(a, l, DIERR_INVALIDPARAM, 14)?;

    // IDirectInputDeviceW::EnumObjects: the X axis first, its name in
    // WCHARs.
    a.push(DIDFT_ALL as i32)?;
    a.push(0)?;
    a.push(callbacks.1 as i32)?;
    a.mov(eax, dword_ptr(l.at(DEVICE) as u64))?;
    a.push(eax)?;
    a.mov(ecx, dword_ptr(eax))?;
    a.call(dword_ptr(ecx + 4 * ENUM_OBJECTS))?;
    expect_eax(a, l, 0, 20)?;
    expect_dword(a, l, OBJECT_CALLS, 1, 21)?;
    expect_dword(a, l, OBJECT_SIZE, OBJECT_W, 22)?;
    expect_dword(a, l, OBJECT_NAME, wide2(b'X', b' '), 23)?;

    // The device's ANSI interface is another pointer, with CHAR names.
    method(
        a,
        l,
        DEVICE,
        QUERY_INTERFACE,
        &[At(IID_DEVICE_A), At(DEVICE_A)],
    )?;
    expect_eax(a, l, 0, 30)?;
    a.mov(eax, dword_ptr(l.at(DEVICE_A) as u64))?;
    a.cmp(eax, dword_ptr(l.at(DEVICE) as u64))?;
    let mut differ = a.create_label();
    a.jne(differ)?;
    a.push(31)?;
    a.call(l.import("ExitProcess"))?;
    a.set_label(&mut differ)?;
    a.mov(dword_ptr(l.at(INFO) as u64), INSTANCE_A as i32)?;
    method(a, l, DEVICE_A, GET_DEVICE_INFO, &[At(INFO)])?;
    expect_eax(a, l, 0, 32)?;
    expect_dword(a, l, INFO + 40, u32::from_le_bytes(*b"Mous"), 33)?;

    // The DirectInput object's ANSI interface, and IDirectInput7W
    // through it, which is the interface DirectInputCreateW gave.
    method(a, l, DI, QUERY_INTERFACE, &[At(IID_DI_A), At(DI_A)])?;
    expect_eax(a, l, 0, 40)?;
    method(a, l, DI_A, QUERY_INTERFACE, &[At(IID_DI_7W), At(DI_7W)])?;
    expect_eax(a, l, 0, 41)?;
    a.mov(eax, dword_ptr(l.at(DI_7W) as u64))?;
    a.cmp(eax, dword_ptr(l.at(DI) as u64))?;
    unless_equal(a, l, 42)?;

    // The device's two interfaces share its references: the last
    // Release of either is the last.
    method(a, l, DEVICE_A, RELEASE, &[])?;
    expect_eax(a, l, 1, 50)?;
    method(a, l, DEVICE, RELEASE, &[])?;
    expect_eax(a, l, 0, 51)?;

    a.push(PASSED as i32)?;
    a.call(l.import("ExitProcess"))?;

    // The EnumDevices callback(instance, ref): the first device's size
    // and names, and DIENUM_STOP.
    a.set_label(&mut device_seen)?;
    a.mov(eax, dword_ptr(esp + 4))?;
    a.inc(dword_ptr(l.at(CALLS) as u64))?;
    a.mov(ecx, dword_ptr(eax))?;
    a.mov(dword_ptr(l.at(SEEN_SIZE) as u64), ecx)?;
    a.mov(ecx, dword_ptr(eax + 40))?;
    a.mov(dword_ptr(l.at(SEEN_NAME) as u64), ecx)?;
    a.mov(ecx, dword_ptr(eax + 40 + 520))?;
    a.mov(dword_ptr(l.at(SEEN_PRODUCT) as u64), ecx)?;
    a.mov(eax, DIENUM_STOP as i32)?;
    a.ret_1(8)?;

    // The EnumObjects callback(object, ref): the same of the first
    // object.
    a.set_label(&mut object_seen)?;
    a.mov(eax, dword_ptr(esp + 4))?;
    a.inc(dword_ptr(l.at(OBJECT_CALLS) as u64))?;
    a.mov(ecx, dword_ptr(eax))?;
    a.mov(dword_ptr(l.at(OBJECT_SIZE) as u64), ecx)?;
    a.mov(ecx, dword_ptr(eax + 32))?;
    a.mov(dword_ptr(l.at(OBJECT_NAME) as u64), ecx)?;
    a.mov(eax, DIENUM_STOP as i32)?;
    a.ret_1(8)?;
    Ok((device_seen, object_seen))
}

/// Run the program: its exit code, and what it printed.
fn run(test: &str, exe: Vec<u8>) -> (u8, String) {
    let base = PathBuf::from("target/test_win32").join(test);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join("T.EXE"), exe).unwrap();
    let mut cpu = Cpu::new(base);
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    let mut ended = false;
    for _ in 0..2000 {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        if run_batch(&mut cpu, &mut NoHook, false) == StopReason::ShellReloaded {
            ended = true;
            break;
        }
    }
    let console = cpu.host_console.as_ref().unwrap();
    let mut text = console.stdout.clone();
    text.extend(&console.stderr);
    let text = String::from_utf8_lossy(&text).into_owned();
    assert!(ended, "the program didn't end: {text:?}");
    (cpu.errorlevel, text)
}

#[test]
fn directinput_create_w_gives_unicode_interfaces_and_structures() {
    let (code, text) = run("dinput_unicode", program());
    assert_eq!(code, PASSED, "check {code} failed: {text:?}");
}
