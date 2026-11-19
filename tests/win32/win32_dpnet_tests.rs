//! The built-in Windows' DirectPlay 8 (DPNET.DLL): Win32 programs made
//! here with rust-dos-pe's builder make its peer, lobbied application
//! and address, as DirectX 8 on Windows 98 SE has them, on a machine
//! whose network protocols can't open a connection. They start the way
//! Magic & Mayhem: The Art of Magic does, register a program for
//! lobbies, fail to host, and build addresses and their URLs.
//!
//! Each program checks the results itself and ends with exit code 0, or
//! with the number of the step that failed.

use iced_x86::code_asm::*;
use rust_dos::cpu::{Cpu, HostConsole};
use rust_dos::exec::{NoHook, StopReason, run_batch};
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
    fn at(&self, offset: u32) -> u64 {
        (self.data + offset) as u64
    }

    fn import(&self, index: usize) -> AsmMemoryOperand {
        dword_ptr(self.iat[index] as u64)
    }
}

/// A program importing IMPORTS, with `data` and the code `f` makes.
fn program(
    data: &[u8],
    f: impl FnOnce(&mut CodeAssembler, &Layout) -> Result<(), IcedError>,
) -> Vec<u8> {
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
    let layout = Layout {
        data: BASE + data_rva,
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

/// The folder of the test `test`'s machine.
fn folder(test: &str) -> PathBuf {
    PathBuf::from("target/test_win32_dpnet").join(test)
}

/// A machine with the program as T.EXE.
fn machine(test: &str, program: Vec<u8>) -> Box<Cpu> {
    let base = folder(test);
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

/// Run the program to its end: its exit code.
fn run(cpu: &mut Cpu) -> u8 {
    for _ in 0..50_000 {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        if run_batch(cpu, &mut NoHook, false) == StopReason::ShellReloaded {
            return cpu.errorlevel;
        }
    }
    panic!("the program didn't end");
}

/// An argument of a call.
#[derive(Clone, Copy)]
enum Arg {
    Imm(u32),
    /// The address of a place in the data.
    Ptr(u32),
    /// The number at a place in the data.
    Val(u32),
}
use Arg::{Imm, Ptr, Val};

fn push(a: &mut CodeAssembler, l: &Layout, args: &[Arg]) -> Result<(), IcedError> {
    for arg in args.iter().rev() {
        match *arg {
            Imm(v) => a.push(v as i32)?,
            Ptr(at) => a.push(l.at(at) as i32)?,
            Val(at) => a.push(dword_ptr(l.at(at)))?,
        }
    }
    Ok(())
}

/// End with exit code `step` unless EAX is `want`.
fn expect(a: &mut CodeAssembler, fail: CodeLabel, step: u32, want: u32) -> Result<(), IcedError> {
    a.mov(ebx, step)?;
    a.cmp(eax, want as i32)?;
    a.jne(fail)
}

/// End with exit code `step` unless the number at `at` is `want`.
fn expect_at(
    a: &mut CodeAssembler,
    l: &Layout,
    fail: CodeLabel,
    step: u32,
    at: u32,
    want: u32,
) -> Result<(), IcedError> {
    a.mov(eax, dword_ptr(l.at(at)))?;
    expect(a, fail, step, want)
}

/// Call the import `import` with `args`; its result must be `want`.
fn call(
    a: &mut CodeAssembler,
    l: &Layout,
    fail: CodeLabel,
    step: u32,
    import: usize,
    args: &[Arg],
    want: u32,
) -> Result<(), IcedError> {
    push(a, l, args)?;
    a.call(l.import(import))?;
    expect(a, fail, step, want)
}

/// Call method `index` of the object whose pointer is at `object` in
/// the data, with `args`; its result must be `want`.
fn method(
    a: &mut CodeAssembler,
    l: &Layout,
    fail: CodeLabel,
    step: u32,
    (object, index): (u32, i32),
    args: &[Arg],
    want: u32,
) -> Result<(), IcedError> {
    push(a, l, args)?;
    a.mov(eax, dword_ptr(l.at(object)))?;
    a.push(eax)?;
    a.mov(eax, dword_ptr(eax))?;
    a.call(dword_ptr(eax + 4 * index))?;
    expect(a, fail, step, want)
}

/// The end: exit code 0, or the step that failed.
fn exit(a: &mut CodeAssembler, l: &Layout, fail: &mut CodeLabel) -> Result<(), IcedError> {
    a.push(0)?;
    a.call(l.import(EXIT))?;
    a.set_label(fail)?;
    a.push(ebx)?;
    a.call(l.import(EXIT))?;
    a.int3()
}

const IMPORTS: &[(&str, &[&str])] = &[
    ("KERNEL32.dll", &["ExitProcess", "lstrcmpA"]),
    ("OLE32.dll", &["CoInitialize", "CoCreateInstance"]),
];
const EXIT: usize = 0;
const LSTRCMP: usize = 1;
const CO_INITIALIZE: usize = 2;
const CO_CREATE: usize = 3;

const S_OK: u32 = 0;
const E_POINTER: u32 = 0x8000_4003;
const CLSCTX_INPROC_SERVER: u32 = 1;

const fn dpnerr(code: u32) -> u32 {
    0x8015_8000 + code
}
const DPNSUCCESS_EQUAL: u32 = 0x0015_8005;
const DPNSUCCESS_NOTEQUAL: u32 = 0x0015_800A;
const DPNERR_ALREADYINITIALIZED: u32 = dpnerr(0x80);
const DPNERR_BUFFERTOOSMALL: u32 = dpnerr(0x100);
const DPNERR_DOESNOTEXIST: u32 = dpnerr(0x180);
const DPNERR_INVALIDDEVICEADDRESS: u32 = dpnerr(0x320);
const DPNERR_INVALIDHANDLE: u32 = dpnerr(0x360);
const DPNERR_INVALIDURL: u32 = dpnerr(0x450);
const DPNERR_NOCONNECTION: u32 = dpnerr(0x480);
const DPNERR_UNINITIALIZED: u32 = dpnerr(0x640);
const DPNERR_UNSUPPORTED: u32 = 0x8000_4001;

/// A GUID's bytes.
const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> [u8; 16] {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

const CLSID_PEER: [u8; 16] = guid(
    0x286F_484D,
    0x375E,
    0x4458,
    [0xA2, 0x72, 0xB1, 0x38, 0xE2, 0xF8, 0x0A, 0x6A],
);
const IID_PEER: [u8; 16] = guid(
    0x5102_DACF,
    0x241B,
    0x11D3,
    [0xAE, 0xA7, 0x00, 0x60, 0x97, 0xB0, 0x14, 0x11],
);
const CLSID_LOBBIED: [u8; 16] = guid(
    0x6679_55AD,
    0x6B3B,
    0x43CA,
    [0xB9, 0x49, 0xBC, 0x69, 0xB5, 0xBA, 0xFF, 0x7F],
);
const IID_LOBBIED: [u8; 16] = guid(
    0x8190_74A3,
    0x016C,
    0x11D3,
    [0xAE, 0x14, 0x00, 0x60, 0x97, 0xB0, 0x14, 0x11],
);
const CLSID_ADDRESS: [u8; 16] = guid(
    0x934A_9523,
    0xA3CA,
    0x4BC5,
    [0xAD, 0xA0, 0xD6, 0xD9, 0x5D, 0x97, 0x94, 0x21],
);
const IID_ADDRESS: [u8; 16] = guid(
    0x8378_3300,
    0x4063,
    0x4C8A,
    [0x9D, 0xB3, 0x82, 0x83, 0x0A, 0x7F, 0xEB, 0x31],
);
const CLSID_DP8SP_TCPIP: [u8; 16] = guid(
    0xEBFE_7BA0,
    0x628D,
    0x11D2,
    [0xAE, 0x0F, 0x00, 0x60, 0x97, 0xB0, 0x14, 0x11],
);
/// The program a test registers for lobbies.
const APPLICATION: [u8; 16] = guid(0x1234_5678, 0x9ABC, 0xDEF0, [1, 2, 3, 4, 5, 6, 7, 8]);

// Methods, by their places in the vtables.
const RELEASE: i32 = 2;
const PEER_INITIALIZE: i32 = 3;
const PEER_ENUM_SERVICE_PROVIDERS: i32 = 4;
const PEER_HOST: i32 = 9;
const PEER_GET_APPLICATION_DESC: i32 = 10;
const PEER_CLOSE: i32 = 24;
const PEER_ENUM_HOSTS: i32 = 25;
const PEER_GET_CAPS: i32 = 30;
const LOBBIED_INITIALIZE: i32 = 3;
const LOBBIED_REGISTER_PROGRAM: i32 = 4;
const LOBBIED_UNREGISTER_PROGRAM: i32 = 5;
const LOBBIED_GET_CONNECTION_SETTINGS: i32 = 10;
const ADDRESS_BUILD_FROM_URL_A: i32 = 4;
const ADDRESS_IS_EQUAL: i32 = 7;
const ADDRESS_GET_URL_A: i32 = 10;
const ADDRESS_GET_SP: i32 = 11;
const ADDRESS_SET_SP: i32 = 13;
const ADDRESS_ADD_COMPONENT: i32 = 18;

const DPNA_DATATYPE_STRING: u32 = 1;
const DPNA_DATATYPE_DWORD: u32 = 2;
const DPNENUMSERVICEPROVIDERS_ALL: u32 = 1;

// The places in the programs' data.
const G_CLSID_PEER: u32 = 0x00;
const G_IID_PEER: u32 = 0x10;
const G_CLSID_LOBBIED: u32 = 0x20;
const G_IID_LOBBIED: u32 = 0x30;
const G_CLSID_ADDRESS: u32 = 0x40;
const G_IID_ADDRESS: u32 = 0x50;
const G_TCPIP: u32 = 0x60;
const G_APPLICATION: u32 = 0x70;
const PEER: u32 = 0x100;
const LOBBIED: u32 = 0x104;
const ADDRESS: u32 = 0x108;
const ADDRESS2: u32 = 0x10C;
const SIZE: u32 = 0x110;
const COUNT: u32 = 0x114;
const CONNECTION: u32 = 0x118;
const VALUE: u32 = 0x11C;
const CAPS: u32 = 0x120;
const HANDLER: u32 = 0x13C;
const SP_GUID: u32 = 0x140;
const PROGRAM_DESC: u32 = 0x180;
const APP_DESC: u32 = 0x200;
const DEVICES: u32 = 0x260;
const STRINGS: u32 = 0x300;
const BUFFER: u32 = 0x800;
const DATA_SIZE: usize = 0x1000;

/// Strings in the data, after STRINGS: their places and contents.
struct Strings {
    data: Vec<u8>,
    next: u32,
}

impl Strings {
    fn wide(&mut self, text: &str) -> u32 {
        let bytes: Vec<u8> = text
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect();
        self.bytes(&bytes)
    }

    fn ansi(&mut self, text: &str) -> u32 {
        let mut bytes = text.as_bytes().to_vec();
        bytes.push(0);
        self.bytes(&bytes)
    }

    fn bytes(&mut self, bytes: &[u8]) -> u32 {
        let at = self.next;
        self.data[at as usize..at as usize + bytes.len()].copy_from_slice(bytes);
        self.next = (at + bytes.len() as u32).next_multiple_of(4);
        at
    }
}

/// The data every program starts with: the GUIDs, then room.
fn data() -> Strings {
    let mut d = vec![0u8; DATA_SIZE];
    let mut put = |at: u32, bytes: &[u8]| {
        d[at as usize..at as usize + bytes.len()].copy_from_slice(bytes);
    };
    put(G_CLSID_PEER, &CLSID_PEER);
    put(G_IID_PEER, &IID_PEER);
    put(G_CLSID_LOBBIED, &CLSID_LOBBIED);
    put(G_IID_LOBBIED, &IID_LOBBIED);
    put(G_CLSID_ADDRESS, &CLSID_ADDRESS);
    put(G_IID_ADDRESS, &IID_ADDRESS);
    put(G_TCPIP, &CLSID_DP8SP_TCPIP);
    put(G_APPLICATION, &APPLICATION);
    Strings {
        data: d,
        next: STRINGS,
    }
}

/// CoInitialize, then an object of the class `clsid` as the interface
/// `iid` at `out`.
fn create(
    a: &mut CodeAssembler,
    l: &Layout,
    fail: CodeLabel,
    step: u32,
    (clsid, iid): (u32, u32),
    out: u32,
) -> Result<(), IcedError> {
    call(a, l, fail, step, CO_INITIALIZE, &[Imm(0)], S_OK)?;
    let args = [
        Ptr(clsid),
        Imm(0),
        Imm(CLSCTX_INPROC_SERVER),
        Ptr(iid),
        Ptr(out),
    ];
    call(a, l, fail, step + 1, CO_CREATE, &args, S_OK)
}

/// A message handler, which returns S_OK and is never called: its
/// address goes to HANDLER. The call over it pushes the address.
fn handler(a: &mut CodeAssembler, l: &Layout) -> Result<(), IcedError> {
    let mut over = a.create_label();
    a.call(over)?;
    a.xor(eax, eax)?;
    a.ret_1(12)?;
    a.set_label(&mut over)?;
    a.pop(dword_ptr(l.at(HANDLER)))?;
    Ok(())
}

/// The peer starts as DirectX 8 has it: it initializes once, keeps
/// DirectPlay 8's default caps, and lists no service provider that can
/// start, but four registered ones (TCP/IP, IPX, modem, serial) when
/// asked for all.
#[test]
fn a_peer_initializes_and_lists_its_service_providers() {
    let exe = program(&data().data, |a, l| {
        let mut fail = a.create_label();
        handler(a, l)?;
        create(a, l, fail, 1, (G_CLSID_PEER, G_IID_PEER), PEER)?;
        // Before Initialize, nothing else.
        let args = [Imm(0), Imm(0), Imm(0), Ptr(SIZE), Ptr(COUNT), Imm(0)];
        let enum_sps = (PEER, PEER_ENUM_SERVICE_PROVIDERS);
        method(a, l, fail, 3, enum_sps, &args, DPNERR_UNINITIALIZED)?;
        a.mov(eax, dword_ptr(l.at(HANDLER)))?;
        a.mov(dword_ptr(l.at(VALUE)), eax)?;
        let init = [Imm(0), Val(VALUE), Imm(0)];
        method(a, l, fail, 4, (PEER, PEER_INITIALIZE), &init, S_OK)?;
        method(
            a,
            l,
            fail,
            5,
            (PEER, PEER_INITIALIZE),
            &init,
            DPNERR_ALREADYINITIALIZED,
        )?;
        // DPN_CAPS: a connect timeout of 200 ms, 14 retries, keep-alive
        // after 60 s.
        a.mov(dword_ptr(l.at(CAPS)), 20)?;
        let args = [Ptr(CAPS), Imm(0)];
        method(a, l, fail, 6, (PEER, PEER_GET_CAPS), &args, S_OK)?;
        expect_at(a, l, fail, 7, CAPS + 8, 200)?;
        expect_at(a, l, fail, 8, CAPS + 12, 14)?;
        expect_at(a, l, fail, 9, CAPS + 16, 60_000)?;
        // No provider can start: none is listed, and nothing is needed.
        a.mov(dword_ptr(l.at(SIZE)), 0)?;
        a.mov(dword_ptr(l.at(COUNT)), 99)?;
        method(a, l, fail, 10, enum_sps, &args_enum(0, 0), S_OK)?;
        expect_at(a, l, fail, 11, SIZE, 0)?;
        expect_at(a, l, fail, 12, COUNT, 0)?;
        // Every registered one: four, 32 bytes each, and their names.
        let names = [
            "DirectPlay8 TCP/IP Service Provider",
            "DirectPlay8 IPX Service Provider",
            "DirectPlay8 Modem Service Provider",
            "DirectPlay8 Serial Service Provider",
        ];
        let needed = 4 * 32 + names.iter().map(|n| 2 * (n.len() as u32 + 1)).sum::<u32>();
        let all = args_enum(0, DPNENUMSERVICEPROVIDERS_ALL);
        method(a, l, fail, 13, enum_sps, &all, DPNERR_BUFFERTOOSMALL)?;
        expect_at(a, l, fail, 14, SIZE, needed)?;
        expect_at(a, l, fail, 15, COUNT, 4)?;
        let all = args_enum(BUFFER, DPNENUMSERVICEPROVIDERS_ALL);
        method(a, l, fail, 16, enum_sps, &all, S_OK)?;
        expect_at(a, l, fail, 17, COUNT, 4)?;
        // The first is TCP/IP's, its name after the four entries.
        expect_at(
            a,
            l,
            fail,
            18,
            BUFFER + 4,
            u32::from_le_bytes(CLSID_DP8SP_TCPIP[..4].try_into().unwrap()),
        )?;
        a.mov(eax, dword_ptr(l.at(BUFFER + 20)))?;
        a.sub(eax, l.at(BUFFER) as i32)?;
        expect(a, fail, 19, 4 * 32)?;
        a.mov(eax, dword_ptr(l.at(BUFFER + 4 * 32)))?;
        expect(a, fail, 20, u32::from(b'D') | u32::from(b'i') << 16)?;
        // A provider's adapters: TCP/IP's can't be found, and a provider
        // that isn't registered isn't there.
        let args = [Ptr(G_TCPIP), Imm(0), Imm(0), Ptr(SIZE), Ptr(COUNT), Imm(0)];
        method(a, l, fail, 21, enum_sps, &args, DPNERR_UNSUPPORTED)?;
        let args = [
            Ptr(G_APPLICATION),
            Imm(0),
            Imm(0),
            Ptr(SIZE),
            Ptr(COUNT),
            Imm(0),
        ];
        method(a, l, fail, 22, enum_sps, &args, DPNERR_DOESNOTEXIST)?;
        method(a, l, fail, 23, (PEER, PEER_CLOSE), &[Imm(0)], S_OK)?;
        method(
            a,
            l,
            fail,
            24,
            (PEER, PEER_CLOSE),
            &[Imm(0)],
            DPNERR_UNINITIALIZED,
        )?;
        method(a, l, fail, 25, (PEER, RELEASE), &[], 0)?;
        exit(a, l, &mut fail)
    });
    let mut cpu = machine("providers", exe);
    assert_eq!(run(&mut cpu), 0);
}

/// EnumServiceProviders' arguments: no provider or application, the
/// buffer at `buffer` (0 for none) of the size at SIZE.
fn args_enum(buffer: u32, flags: u32) -> [Arg; 6] {
    let buffer = if buffer == 0 { Imm(0) } else { Ptr(buffer) };
    [Imm(0), Imm(0), buffer, Ptr(SIZE), Ptr(COUNT), Imm(flags)]
}

/// A peer can't host, join or look for a session on TCP/IP, whose
/// provider can't start: the device address is the one at fault. What
/// needs a session finds none.
#[test]
fn a_peer_cant_host_without_a_network() {
    let exe = program(&data().data, |a, l| {
        let mut fail = a.create_label();
        handler(a, l)?;
        create(a, l, fail, 1, (G_CLSID_PEER, G_IID_PEER), PEER)?;
        let args = [
            Ptr(G_CLSID_ADDRESS),
            Imm(0),
            Imm(CLSCTX_INPROC_SERVER),
            Ptr(G_IID_ADDRESS),
            Ptr(ADDRESS),
        ];
        call(a, l, fail, 3, CO_CREATE, &args, S_OK)?;
        a.mov(eax, dword_ptr(l.at(HANDLER)))?;
        a.mov(dword_ptr(l.at(VALUE)), eax)?;
        let init = [Imm(0), Val(VALUE), Imm(0)];
        method(a, l, fail, 4, (PEER, PEER_INITIALIZE), &init, S_OK)?;
        let args = [Ptr(G_TCPIP)];
        method(a, l, fail, 5, (ADDRESS, ADDRESS_SET_SP), &args, S_OK)?;
        // DPN_APPLICATION_DESC, 72 bytes.
        a.mov(dword_ptr(l.at(APP_DESC)), 72)?;
        a.mov(eax, dword_ptr(l.at(ADDRESS)))?;
        a.mov(dword_ptr(l.at(DEVICES)), eax)?;
        let args = [
            Ptr(APP_DESC),
            Ptr(DEVICES),
            Imm(1),
            Imm(0),
            Imm(0),
            Imm(0),
            Imm(0),
        ];
        method(
            a,
            l,
            fail,
            6,
            (PEER, PEER_HOST),
            &args,
            DPNERR_INVALIDDEVICEADDRESS,
        )?;
        let args = [
            Ptr(APP_DESC),
            Val(ADDRESS),
            Imm(0),
            Imm(0),
            Imm(0),
            Imm(0),
            Imm(0),
            Imm(0),
            Imm(0),
            Imm(0),
            Imm(0),
        ];
        method(
            a,
            l,
            fail,
            7,
            (PEER, PEER_ENUM_HOSTS),
            &args,
            DPNERR_INVALIDDEVICEADDRESS,
        )?;
        let args = [Ptr(BUFFER), Ptr(SIZE), Imm(0)];
        method(
            a,
            l,
            fail,
            8,
            (PEER, PEER_GET_APPLICATION_DESC),
            &args,
            DPNERR_NOCONNECTION,
        )?;
        method(a, l, fail, 9, (ADDRESS, RELEASE), &[], 0)?;
        method(a, l, fail, 10, (PEER, PEER_CLOSE), &[Imm(0)], S_OK)?;
        method(a, l, fail, 11, (PEER, RELEASE), &[], 0)?;
        exit(a, l, &mut fail)
    });
    let mut cpu = machine("host", exe);
    assert_eq!(run(&mut cpu), 0);
}

/// The lobbied application of a program no lobby launched: Initialize
/// leaves the connection handle as it was, RegisterProgram writes the
/// program's key, and a connection handle is none there is.
#[test]
fn a_lobbied_application_registers_a_program() {
    let mut d = data();
    let name = d.wide("Test Game");
    let file = d.wide("GAME.EXE");
    let path = d.wide("C:\\GAME");
    let mut desc = vec![0u8; 0x38];
    desc[..4].copy_from_slice(&0x38u32.to_le_bytes());
    desc[8..24].copy_from_slice(&APPLICATION);
    d.data[PROGRAM_DESC as usize..PROGRAM_DESC as usize + 0x38].copy_from_slice(&desc);
    let exe = program(&d.data, |a, l| {
        let mut fail = a.create_label();
        handler(a, l)?;
        for (at, s) in [(0x18, name), (0x28, file), (0x2C, path)] {
            a.mov(dword_ptr(l.at(PROGRAM_DESC + at)), l.at(s) as i32)?;
        }
        create(a, l, fail, 1, (G_CLSID_LOBBIED, G_IID_LOBBIED), LOBBIED)?;
        let args = [Ptr(PROGRAM_DESC), Imm(0)];
        let register = (LOBBIED, LOBBIED_REGISTER_PROGRAM);
        method(a, l, fail, 3, register, &args, DPNERR_UNINITIALIZED)?;
        a.mov(eax, dword_ptr(l.at(HANDLER)))?;
        a.mov(dword_ptr(l.at(VALUE)), eax)?;
        a.mov(dword_ptr(l.at(CONNECTION)), 0x5A5A_5A5A)?;
        let init = [Imm(0), Val(VALUE), Ptr(CONNECTION), Imm(0)];
        method(a, l, fail, 4, (LOBBIED, LOBBIED_INITIALIZE), &init, S_OK)?;
        expect_at(a, l, fail, 5, CONNECTION, 0x5A5A_5A5A)?;
        // Registered twice, as the game does on every start: one key.
        method(a, l, fail, 6, register, &args, S_OK)?;
        method(a, l, fail, 7, register, &args, S_OK)?;
        let args = [Imm(1), Ptr(BUFFER), Ptr(SIZE), Imm(0)];
        method(
            a,
            l,
            fail,
            8,
            (LOBBIED, LOBBIED_GET_CONNECTION_SETTINGS),
            &args,
            DPNERR_INVALIDHANDLE,
        )?;
        exit(a, l, &mut fail)
    });
    let mut cpu = machine("lobbied", exe);
    assert_eq!(run(&mut cpu), 0);
    let registry = fs::read_to_string(folder("lobbied").join("REGISTRY.REG")).unwrap();
    let key = "[HKEY_LOCAL_MACHINE\\Software\\Microsoft\\DirectPlay8\\Applications\\Test Game]";
    assert_eq!(registry.matches(key).count(), 1, "{registry}");
    let values = registry.split(key).nth(1).unwrap();
    let values = values.replace("\r\n", "\n");
    let values = values.split("\n\n").next().unwrap();
    for line in [
        "\"ApplicationName\"=\"Test Game\"",
        "\"ExecutableFilename\"=\"GAME.EXE\"",
        // A .reg file doubles the backslash.
        "\"ExecutablePath\"=\"C:\\\\GAME\"",
        "\"GUID\"=\"{12345678-9ABC-DEF0-0102-030405060708}\"",
    ] {
        assert!(values.contains(line), "{line} isn't in {values}");
    }
}

/// UnRegisterProgram takes a registered program's key away, and fails
/// for one that isn't registered.
#[test]
fn a_lobbied_application_unregisters_a_program() {
    let mut d = data();
    let name = d.wide("Test Game");
    let file = d.wide("GAME.EXE");
    let mut desc = vec![0u8; 0x38];
    desc[..4].copy_from_slice(&0x38u32.to_le_bytes());
    desc[8..24].copy_from_slice(&APPLICATION);
    d.data[PROGRAM_DESC as usize..PROGRAM_DESC as usize + 0x38].copy_from_slice(&desc);
    let exe = program(&d.data, |a, l| {
        let mut fail = a.create_label();
        handler(a, l)?;
        a.mov(dword_ptr(l.at(PROGRAM_DESC + 0x18)), l.at(name) as i32)?;
        a.mov(dword_ptr(l.at(PROGRAM_DESC + 0x28)), l.at(file) as i32)?;
        create(a, l, fail, 1, (G_CLSID_LOBBIED, G_IID_LOBBIED), LOBBIED)?;
        a.mov(eax, dword_ptr(l.at(HANDLER)))?;
        a.mov(dword_ptr(l.at(VALUE)), eax)?;
        let init = [Imm(0), Val(VALUE), Imm(0), Imm(0)];
        method(a, l, fail, 3, (LOBBIED, LOBBIED_INITIALIZE), &init, S_OK)?;
        let register = (LOBBIED, LOBBIED_REGISTER_PROGRAM);
        method(a, l, fail, 4, register, &[Ptr(PROGRAM_DESC), Imm(0)], S_OK)?;
        let unregister = (LOBBIED, LOBBIED_UNREGISTER_PROGRAM);
        let args = [Ptr(G_APPLICATION), Imm(0)];
        method(a, l, fail, 5, unregister, &args, S_OK)?;
        method(a, l, fail, 6, unregister, &args, DPNERR_DOESNOTEXIST)?;
        method(a, l, fail, 7, (LOBBIED, RELEASE), &[], 0)?;
        exit(a, l, &mut fail)
    });
    let mut cpu = machine("unregister", exe);
    assert_eq!(run(&mut cpu), 0);
    let registry = fs::read_to_string(folder("unregister").join("REGISTRY.REG")).unwrap();
    assert!(!registry.contains("Test Game"), "{registry}");
}

/// The URL of the address the test makes.
const URL: &str = "x-directplay:/provider=%7BEBFE7BA0-628D-11D2-AE0F-006097B01411%7D;\
                   hostname=my%20host;port=2302";

/// An address made of components gives the URL DirectPlay 8 writes, and
/// one built from that URL is equal to it; a URL of another scheme isn't
/// taken.
#[test]
fn an_address_is_written_and_read_as_a_url() {
    let mut d = data();
    let hostname = d.wide("hostname");
    let host = d.wide("my host");
    let port = d.wide("port");
    let port_value = d.bytes(&2302u32.to_le_bytes());
    let want = d.ansi(URL);
    let other = d.ansi("http://example.com/");
    let exe = program(&d.data, |a, l| {
        let mut fail = a.create_label();
        create(a, l, fail, 1, (G_CLSID_ADDRESS, G_IID_ADDRESS), ADDRESS)?;
        let args = [
            Ptr(G_CLSID_ADDRESS),
            Imm(0),
            Imm(CLSCTX_INPROC_SERVER),
            Ptr(G_IID_ADDRESS),
            Ptr(ADDRESS2),
        ];
        call(a, l, fail, 3, CO_CREATE, &args, S_OK)?;
        // No provider yet.
        let args = [Ptr(SP_GUID)];
        method(
            a,
            l,
            fail,
            4,
            (ADDRESS, ADDRESS_GET_SP),
            &args,
            DPNERR_DOESNOTEXIST,
        )?;
        let add = (ADDRESS, ADDRESS_ADD_COMPONENT);
        let args = [Ptr(hostname), Ptr(host), Imm(16), Imm(DPNA_DATATYPE_STRING)];
        method(a, l, fail, 5, add, &args, S_OK)?;
        let args = [Ptr(port), Ptr(port_value), Imm(4), Imm(DPNA_DATATYPE_DWORD)];
        method(a, l, fail, 6, add, &args, S_OK)?;
        method(
            a,
            l,
            fail,
            7,
            (ADDRESS, ADDRESS_SET_SP),
            &[Ptr(G_TCPIP)],
            S_OK,
        )?;
        // The length first, then the URL.
        a.mov(dword_ptr(l.at(SIZE)), 0)?;
        let get_url = (ADDRESS, ADDRESS_GET_URL_A);
        let args = [Ptr(BUFFER), Ptr(SIZE)];
        method(a, l, fail, 8, get_url, &args, DPNERR_BUFFERTOOSMALL)?;
        expect_at(a, l, fail, 9, SIZE, URL.len() as u32 + 1)?;
        method(a, l, fail, 10, get_url, &args, S_OK)?;
        call(a, l, fail, 11, LSTRCMP, &[Ptr(BUFFER), Ptr(want)], 0)?;
        let build = (ADDRESS2, ADDRESS_BUILD_FROM_URL_A);
        method(a, l, fail, 12, build, &[Ptr(other)], DPNERR_INVALIDURL)?;
        method(a, l, fail, 13, build, &[Ptr(want)], S_OK)?;
        let args = [Val(ADDRESS2)];
        method(
            a,
            l,
            fail,
            14,
            (ADDRESS, ADDRESS_IS_EQUAL),
            &args,
            DPNSUCCESS_EQUAL,
        )?;
        method(a, l, fail, 15, build, &[Ptr(other)], DPNERR_INVALIDURL)?;
        let args = [Ptr(SP_GUID)];
        method(a, l, fail, 16, (ADDRESS2, ADDRESS_GET_SP), &args, S_OK)?;
        expect_at(
            a,
            l,
            fail,
            17,
            SP_GUID,
            u32::from_le_bytes(CLSID_DP8SP_TCPIP[..4].try_into().unwrap()),
        )?;
        let args = [Ptr(hostname), Ptr(host), Imm(4), Imm(DPNA_DATATYPE_STRING)];
        method(
            a,
            l,
            fail,
            18,
            (ADDRESS2, ADDRESS_ADD_COMPONENT),
            &args,
            0x8007_0057,
        )?;
        let args = [Ptr(hostname), Imm(0), Imm(16), Imm(DPNA_DATATYPE_STRING)];
        method(
            a,
            l,
            fail,
            19,
            (ADDRESS2, ADDRESS_ADD_COMPONENT),
            &args,
            E_POINTER,
        )?;
        let args = [Ptr(port), Ptr(port_value), Imm(2), Imm(DPNA_DATATYPE_DWORD)];
        method(
            a,
            l,
            fail,
            20,
            (ADDRESS2, ADDRESS_ADD_COMPONENT),
            &args,
            0x8007_0057,
        )?;
        let args = [Ptr(port), Ptr(G_TCPIP), Imm(4), Imm(DPNA_DATATYPE_DWORD)];
        method(
            a,
            l,
            fail,
            21,
            (ADDRESS2, ADDRESS_ADD_COMPONENT),
            &args,
            S_OK,
        )?;
        let args = [Val(ADDRESS2)];
        method(
            a,
            l,
            fail,
            22,
            (ADDRESS, ADDRESS_IS_EQUAL),
            &args,
            DPNSUCCESS_NOTEQUAL,
        )?;
        method(a, l, fail, 23, (ADDRESS2, RELEASE), &[], 0)?;
        method(a, l, fail, 24, (ADDRESS, RELEASE), &[], 0)?;
        exit(a, l, &mut fail)
    });
    let mut cpu = machine("address", exe);
    assert_eq!(run(&mut cpu), 0);
}
