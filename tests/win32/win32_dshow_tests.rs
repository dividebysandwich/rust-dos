//! The built-in Windows' DirectShow: Win32 programs made here with
//! rust-dos-pe's builder make a filter graph and play a movie with it,
//! and read a movie's pictures through amstream's multimedia streams into
//! a DirectDraw surface, as Magic & Mayhem does. The results are those
//! Windows 98 SE's QUARTZ.DLL and AMSTREAM.DLL give.

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

/// A program importing `imports`, with `data` and the code `f` makes.
fn program(
    imports: &[(&str, &[&str])],
    data: &[u8],
    f: impl FnOnce(&mut CodeAssembler, &Layout) -> Result<(), IcedError>,
) -> Vec<u8> {
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

/// A machine with the program as T.EXE and `files` next to it.
fn machine(test: &str, program: Vec<u8>, files: &[(&str, Vec<u8>)]) -> Cpu {
    let base = PathBuf::from("target/test_win32_dshow").join(test);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join("T.EXE"), program).unwrap();
    for (name, bytes) in files {
        fs::write(base.join(name), bytes).unwrap();
    }
    let mut cpu = Cpu::new(base);
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    cpu
}

/// What the program and the system printed.
fn printed(cpu: &Cpu) -> String {
    let console = cpu.host_console.as_ref().unwrap();
    let mut text = console.stdout.clone();
    text.extend(&console.stderr);
    String::from_utf8_lossy(&text).into_owned()
}

/// Run the program to its end; its exit code.
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

/// Call method `offset` of the object whose pointer is at `object` in the
/// data, with `args`, leaving its result in EAX.
fn invoke(
    a: &mut CodeAssembler,
    l: &Layout,
    (object, offset): (u32, i32),
    args: &[Arg],
) -> Result<(), IcedError> {
    push(a, l, args)?;
    a.mov(eax, dword_ptr(l.at(object)))?;
    a.push(eax)?;
    a.mov(eax, dword_ptr(eax))?;
    a.call(dword_ptr(eax + offset))
}

/// Call a method as `invoke` does; its result must be `want`.
fn method(
    a: &mut CodeAssembler,
    l: &Layout,
    fail: CodeLabel,
    step: u32,
    target: (u32, i32),
    args: &[Arg],
    want: u32,
) -> Result<(), IcedError> {
    invoke(a, l, target, args)?;
    expect(a, fail, step, want)
}

/// The end: exit code `code`, or the step that failed.
fn exit(
    a: &mut CodeAssembler,
    l: &Layout,
    fail: &mut CodeLabel,
    code: Arg,
) -> Result<(), IcedError> {
    push(a, l, &[code])?;
    a.call(l.import(EXIT))?;
    a.set_label(fail)?;
    a.push(ebx)?;
    a.call(l.import(EXIT))?;
    a.int3()
}

const IMPORTS: &[(&str, &[&str])] = &[
    (
        "KERNEL32.dll",
        &["ExitProcess", "WaitForSingleObject", "Sleep"],
    ),
    ("OLE32.dll", &["CoInitialize", "CoCreateInstance"]),
];
const EXIT: usize = 0;
const WAIT: usize = 1;
const SLEEP: usize = 2;
const CO_INITIALIZE: usize = 3;
const CO_CREATE: usize = 4;

const S_OK: u32 = 0;
const E_NOINTERFACE: u32 = 0x8000_4002;
const MS_S_ENDOFSTREAM: u32 = 0x0004_0003;
const MS_E_NOTRUNNING: u32 = 0x8004_040A;

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

const fn quartz(d1: u32) -> [u8; 16] {
    guid(
        d1,
        0x0AD4,
        0x11CE,
        [0xB0, 0x3A, 0x00, 0x20, 0xAF, 0x0B, 0xA7, 0x70],
    )
}

const fn amstream(d1: u32, d2: u16) -> [u8; 16] {
    guid(
        d1,
        d2,
        0x11D0,
        [0x8F, 0xDE, 0x00, 0xC0, 0x4F, 0xD9, 0x18, 0x9D],
    )
}

const CLSID_FILTER_GRAPH: [u8; 16] = guid(
    0xE436_EBB3,
    0x524F,
    0x11CE,
    [0x9F, 0x53, 0x00, 0x20, 0xAF, 0x0B, 0xA7, 0x70],
);
const IID_IGRAPH_BUILDER: [u8; 16] = quartz(0x56A8_68A9);
const IID_IMEDIA_CONTROL: [u8; 16] = quartz(0x56A8_68B1);
const IID_IMEDIA_EVENT: [u8; 16] = quartz(0x56A8_68B6);
const CLSID_AM_MULTI_MEDIA_STREAM: [u8; 16] = guid(
    0x49C4_7CE5,
    0x9BA4,
    0x11D0,
    [0x82, 0x12, 0x00, 0xC0, 0x4F, 0xC3, 0x2C, 0x45],
);
const IID_IAM_MULTI_MEDIA_STREAM: [u8; 16] = amstream(0xBEBE_595C, 0x9A6F);
const IID_IDIRECT_DRAW_MEDIA_STREAM: [u8; 16] = amstream(0xF410_4FCE, 0x9A70);
const MSPID_PRIMARY_VIDEO: [u8; 16] = guid(
    0xA35F_F56A,
    0x9FDA,
    0x11D0,
    [0x8F, 0xDF, 0x00, 0xC0, 0x4F, 0xD9, 0x18, 0x9D],
);

// The places in the programs' data.
const G_CLSID_GRAPH: u32 = 0x00;
const G_IID_GRAPH: u32 = 0x10;
const G_IID_CONTROL: u32 = 0x20;
const G_IID_EVENT: u32 = 0x30;
const G_CLSID_MMS: u32 = 0x40;
const G_IID_MMS: u32 = 0x50;
const G_IID_DDSTREAM: u32 = 0x60;
const G_VIDEO: u32 = 0x70;
const GRAPH: u32 = 0x100;
const CONTROL: u32 = 0x104;
const EVENT: u32 = 0x108;
const MMS: u32 = 0x10C;
const STREAM: u32 = 0x110;
const DDSTREAM: u32 = 0x114;
const SAMPLE: u32 = 0x118;
const SURFACE: u32 = 0x11C;
const VALUE: u32 = 0x120;
const COUNT: u32 = 0x128;
const PARAM1: u32 = 0x12C;
const PARAM2: u32 = 0x130;
const HANDLE: u32 = 0x134;
const FILE: u32 = 0x140;
const RECT: u32 = 0x180;
const FORMAT: u32 = 0x200;
const LOCKED: u32 = 0x280;
const SIZE: usize = 0x400;

fn data() -> Vec<u8> {
    let mut d = vec![0u8; SIZE];
    let mut put = |at: u32, bytes: &[u8]| {
        d[at as usize..at as usize + bytes.len()].copy_from_slice(bytes);
    };
    put(G_CLSID_GRAPH, &CLSID_FILTER_GRAPH);
    put(G_IID_GRAPH, &IID_IGRAPH_BUILDER);
    put(G_IID_CONTROL, &IID_IMEDIA_CONTROL);
    put(G_IID_EVENT, &IID_IMEDIA_EVENT);
    put(G_CLSID_MMS, &CLSID_AM_MULTI_MEDIA_STREAM);
    put(G_IID_MMS, &IID_IAM_MULTI_MEDIA_STREAM);
    put(G_IID_DDSTREAM, &IID_IDIRECT_DRAW_MEDIA_STREAM);
    put(G_VIDEO, &MSPID_PRIMARY_VIDEO);
    let name: Vec<u8> = "CLIP.AVI\0"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    put(FILE, &name);
    // A DDSURFACEDESC with DDSD_PIXELFORMAT: 32-bit RGB (DDPF_RGB).
    put(FORMAT, &108u32.to_le_bytes());
    put(FORMAT + 4, &0x1000u32.to_le_bytes());
    put(FORMAT + 72, &32u32.to_le_bytes());
    put(FORMAT + 76, &0x40u32.to_le_bytes());
    put(FORMAT + 84, &32u32.to_le_bytes());
    put(FORMAT + 88, &0xFF_0000u32.to_le_bytes());
    put(FORMAT + 92, &0xFF00u32.to_le_bytes());
    put(FORMAT + 96, &0xFFu32.to_le_bytes());
    put(LOCKED, &108u32.to_le_bytes());
    d
}

/// A movie of twelve 32 by 32 pictures at 25 a second, uncompressed
/// 24-bit, with an index: six red, then six blue.
fn clip() -> Vec<u8> {
    fn chunk(out: &mut Vec<u8>, id: &[u8], data: &[u8]) {
        out.extend_from_slice(id);
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
    }
    let le = |v: u32| v.to_le_bytes();
    let mut strh = vec![0; 56];
    strh[..4].copy_from_slice(b"vids");
    strh[20..24].copy_from_slice(&le(1));
    strh[24..28].copy_from_slice(&le(25));
    strh[32..36].copy_from_slice(&le(12));
    let mut bih = vec![0; 40];
    bih[..4].copy_from_slice(&le(40));
    bih[4..8].copy_from_slice(&le(32));
    bih[8..12].copy_from_slice(&le(32));
    bih[12..16].copy_from_slice(&[1, 0, 24, 0]);
    let mut strl = b"strl".to_vec();
    chunk(&mut strl, b"strh", &strh);
    chunk(&mut strl, b"strf", &bih);
    let mut hdrl = b"hdrl".to_vec();
    chunk(&mut hdrl, b"avih", &[0; 56]);
    chunk(&mut hdrl, b"LIST", &strl);
    let mut movi = b"movi".to_vec();
    let mut idx1 = Vec::new();
    for n in 0..12 {
        let px: [u8; 3] = if n < 6 { [0, 0, 255] } else { [255, 0, 0] };
        idx1.extend_from_slice(b"00db");
        idx1.extend_from_slice(&le(0x10));
        idx1.extend_from_slice(&le(movi.len() as u32));
        idx1.extend_from_slice(&le(32 * 32 * 3));
        chunk(&mut movi, b"00db", &px.repeat(32 * 32));
    }
    let mut body = b"AVI ".to_vec();
    chunk(&mut body, b"LIST", &hdrl);
    chunk(&mut body, b"LIST", &movi);
    chunk(&mut body, b"idx1", &idx1);
    let mut out = Vec::new();
    chunk(&mut out, b"RIFF", &body);
    out
}

// Methods, as vtable offsets.
const QUERY_INTERFACE: i32 = 0;
const RELEASE: i32 = 8;
const GRAPH_RENDER_FILE: i32 = 52;
const CONTROL_RUN: i32 = 28;
const EVENT_GET_EVENT_HANDLE: i32 = 28;
const EVENT_GET_EVENT: i32 = 32;
const EVENT_WAIT_FOR_COMPLETION: i32 = 36;
const MMS_GET_MEDIA_STREAM: i32 = 16;
const MMS_SET_STATE: i32 = 28;
const MMS_INITIALIZE: i32 = 48;
const MMS_ADD_MEDIA_STREAM: i32 = 60;
const MMS_OPEN_FILE: i32 = 64;
const DDSTREAM_SET_FORMAT: i32 = 40;
const DDSTREAM_CREATE_SAMPLE: i32 = 52;
const DDSTREAM_GET_TIME_PER_FRAME: i32 = 56;
const SAMPLE_UPDATE: i32 = 24;
const SAMPLE_GET_SURFACE: i32 = 32;
const SURFACE_LOCK: i32 = 100;
const SURFACE_UNLOCK: i32 = 128;

/// Magic & Mayhem's check for DirectShow: a filter graph, released at
/// once; and then a graph that plays a movie to its end.
#[test]
fn a_filter_graph_plays_a_movie_to_its_end() {
    let exe = program(IMPORTS, &data(), |a, l| {
        let mut fail = a.create_label();
        call(a, l, fail, 1, CO_INITIALIZE, &[Imm(0)], S_OK)?;
        let args = [
            Ptr(G_CLSID_GRAPH),
            Imm(0),
            Imm(1),
            Ptr(G_IID_GRAPH),
            Ptr(GRAPH),
        ];
        call(a, l, fail, 2, CO_CREATE, &args, S_OK)?;
        method(a, l, fail, 3, (GRAPH, RELEASE), &[], 0)?;
        call(a, l, fail, 4, CO_CREATE, &args, S_OK)?;
        // A graph isn't a multimedia stream.
        let args = [Ptr(G_IID_MMS), Ptr(MMS)];
        method(
            a,
            l,
            fail,
            5,
            (GRAPH, QUERY_INTERFACE),
            &args,
            E_NOINTERFACE,
        )?;
        let args = [Ptr(FILE), Imm(0)];
        method(a, l, fail, 6, (GRAPH, GRAPH_RENDER_FILE), &args, S_OK)?;
        let args = [Ptr(G_IID_CONTROL), Ptr(CONTROL)];
        method(a, l, fail, 7, (GRAPH, QUERY_INTERFACE), &args, S_OK)?;
        let args = [Ptr(G_IID_EVENT), Ptr(EVENT)];
        method(a, l, fail, 8, (GRAPH, QUERY_INTERFACE), &args, S_OK)?;
        method(a, l, fail, 9, (CONTROL, CONTROL_RUN), &[], S_OK)?;
        // EC_COMPLETE, within the five seconds.
        let args = [Imm(5000), Ptr(VALUE)];
        method(
            a,
            l,
            fail,
            10,
            (EVENT, EVENT_WAIT_FOR_COMPLETION),
            &args,
            S_OK,
        )?;
        a.mov(eax, dword_ptr(l.at(VALUE)))?;
        expect(a, fail, 11, 1)?;
        method(a, l, fail, 12, (EVENT, RELEASE), &[], 2)?;
        method(a, l, fail, 13, (CONTROL, RELEASE), &[], 1)?;
        method(a, l, fail, 14, (GRAPH, RELEASE), &[], 0)?;
        exit(a, l, &mut fail, Imm(0))
    });
    let mut cpu = machine("graph", exe, &[("CLIP.AVI", clip())]);
    let code = run(&mut cpu);
    assert_eq!(code, 0, "{}", printed(&cpu));
}

/// A program that waits on the graph's event handle rather than in
/// WaitForCompletion: the multimedia thread sees the play's end, sets
/// the event, and GetEvent gives EC_COMPLETE. The program sleeps before
/// Run, as one that pumps its messages does, so that the multimedia
/// thread has gone to sleep with nothing to do: Run must wake it.
#[test]
fn a_running_graph_sets_its_event_at_the_end() {
    let exe = program(IMPORTS, &data(), |a, l| {
        let mut fail = a.create_label();
        call(a, l, fail, 1, CO_INITIALIZE, &[Imm(0)], S_OK)?;
        let args = [
            Ptr(G_CLSID_GRAPH),
            Imm(0),
            Imm(1),
            Ptr(G_IID_GRAPH),
            Ptr(GRAPH),
        ];
        call(a, l, fail, 2, CO_CREATE, &args, S_OK)?;
        let args = [Ptr(FILE), Imm(0)];
        method(a, l, fail, 3, (GRAPH, GRAPH_RENDER_FILE), &args, S_OK)?;
        let args = [Ptr(G_IID_CONTROL), Ptr(CONTROL)];
        method(a, l, fail, 4, (GRAPH, QUERY_INTERFACE), &args, S_OK)?;
        let args = [Ptr(G_IID_EVENT), Ptr(EVENT)];
        method(a, l, fail, 5, (GRAPH, QUERY_INTERFACE), &args, S_OK)?;
        let args = [Ptr(HANDLE)];
        method(a, l, fail, 6, (EVENT, EVENT_GET_EVENT_HANDLE), &args, S_OK)?;
        push(a, l, &[Imm(100)])?;
        a.call(l.import(SLEEP))?;
        method(a, l, fail, 7, (CONTROL, CONTROL_RUN), &[], S_OK)?;
        // WAIT_OBJECT_0 within the five seconds: the clip is 480 ms.
        call(a, l, fail, 8, WAIT, &[Val(HANDLE), Imm(5000)], 0)?;
        let args = [Ptr(COUNT), Ptr(PARAM1), Ptr(PARAM2), Imm(0)];
        method(a, l, fail, 9, (EVENT, EVENT_GET_EVENT), &args, S_OK)?;
        a.mov(eax, dword_ptr(l.at(COUNT)))?;
        expect(a, fail, 10, 1)?;
        method(a, l, fail, 11, (EVENT, RELEASE), &[], 2)?;
        method(a, l, fail, 12, (CONTROL, RELEASE), &[], 1)?;
        method(a, l, fail, 13, (GRAPH, RELEASE), &[], 0)?;
        exit(a, l, &mut fail, Imm(0))
    });
    let mut cpu = machine("graph-event", exe, &[("CLIP.AVI", clip())]);
    let code = run(&mut cpu);
    assert_eq!(code, 0, "{}", printed(&cpu));
}

/// A program that ends while its graph plays, without releasing it: the
/// process's objects, device and window go with it.
#[test]
fn a_program_can_end_while_its_graph_plays() {
    let exe = program(IMPORTS, &data(), |a, l| {
        let mut fail = a.create_label();
        call(a, l, fail, 1, CO_INITIALIZE, &[Imm(0)], S_OK)?;
        let args = [
            Ptr(G_CLSID_GRAPH),
            Imm(0),
            Imm(1),
            Ptr(G_IID_GRAPH),
            Ptr(GRAPH),
        ];
        call(a, l, fail, 2, CO_CREATE, &args, S_OK)?;
        let args = [Ptr(FILE), Imm(0)];
        method(a, l, fail, 3, (GRAPH, GRAPH_RENDER_FILE), &args, S_OK)?;
        let args = [Ptr(G_IID_CONTROL), Ptr(CONTROL)];
        method(a, l, fail, 4, (GRAPH, QUERY_INTERFACE), &args, S_OK)?;
        push(a, l, &[Imm(100)])?;
        a.call(l.import(SLEEP))?;
        method(a, l, fail, 5, (CONTROL, CONTROL_RUN), &[], S_OK)?;
        push(a, l, &[Imm(100)])?;
        a.call(l.import(SLEEP))?;
        exit(a, l, &mut fail, Imm(0))
    });
    let mut cpu = machine("graph-exit", exe, &[("CLIP.AVI", clip())]);
    let code = run(&mut cpu);
    assert_eq!(code, 0, "{}", printed(&cpu));
}

/// Lock the sample's surface and leave its first pixel in EAX.
fn first_pixel(
    a: &mut CodeAssembler,
    l: &Layout,
    fail: CodeLabel,
    step: u32,
) -> Result<(), IcedError> {
    // DDLOCK_WAIT.
    let args = [Imm(0), Ptr(LOCKED), Imm(1), Imm(0)];
    method(a, l, fail, step, (SURFACE, SURFACE_LOCK), &args, S_OK)?;
    let args = [Imm(0)];
    method(a, l, fail, step + 1, (SURFACE, SURFACE_UNLOCK), &args, S_OK)?;
    a.mov(eax, dword_ptr(l.at(LOCKED + 36)))?;
    a.mov(eax, dword_ptr(eax))?;
    a.and(eax, 0xFF_FFFF)
}

/// Magic & Mayhem's movies: a multimedia stream of the file's pictures,
/// each update one of them in the sample's DirectDraw surface, at its
/// time, until the end of the stream.
#[test]
fn a_multimedia_stream_gives_each_picture_at_its_time() {
    let exe = program(IMPORTS, &data(), |a, l| {
        let mut fail = a.create_label();
        call(a, l, fail, 1, CO_INITIALIZE, &[Imm(0)], S_OK)?;
        let args = [Ptr(G_CLSID_MMS), Imm(0), Imm(1), Ptr(G_IID_MMS), Ptr(MMS)];
        call(a, l, fail, 2, CO_CREATE, &args, S_OK)?;
        let args = [Imm(0), Imm(0), Imm(0)];
        method(a, l, fail, 3, (MMS, MMS_INITIALIZE), &args, S_OK)?;
        let args = [Imm(0), Ptr(G_VIDEO), Imm(0), Imm(0)];
        method(a, l, fail, 4, (MMS, MMS_ADD_MEDIA_STREAM), &args, S_OK)?;
        let args = [Ptr(FILE), Imm(0)];
        method(a, l, fail, 5, (MMS, MMS_OPEN_FILE), &args, S_OK)?;
        let args = [Ptr(G_VIDEO), Ptr(STREAM)];
        method(a, l, fail, 6, (MMS, MMS_GET_MEDIA_STREAM), &args, S_OK)?;
        let args = [Ptr(G_IID_DDSTREAM), Ptr(DDSTREAM)];
        method(a, l, fail, 7, (STREAM, QUERY_INTERFACE), &args, S_OK)?;
        let args = [Ptr(FORMAT), Imm(0)];
        method(a, l, fail, 8, (DDSTREAM, DDSTREAM_SET_FORMAT), &args, S_OK)?;
        let args = [Imm(0), Imm(0), Imm(0), Ptr(SAMPLE)];
        method(
            a,
            l,
            fail,
            9,
            (DDSTREAM, DDSTREAM_CREATE_SAMPLE),
            &args,
            S_OK,
        )?;
        let args = [Ptr(SURFACE), Ptr(RECT)];
        method(a, l, fail, 10, (SAMPLE, SAMPLE_GET_SURFACE), &args, S_OK)?;
        a.mov(eax, dword_ptr(l.at(RECT + 8)))?;
        expect(a, fail, 11, 32)?;
        // 40 ms a picture, in 100-nanosecond units.
        let args = [Ptr(VALUE)];
        method(
            a,
            l,
            fail,
            12,
            (DDSTREAM, DDSTREAM_GET_TIME_PER_FRAME),
            &args,
            S_OK,
        )?;
        a.mov(eax, dword_ptr(l.at(VALUE)))?;
        expect(a, fail, 13, 400_000)?;
        // Not before the stream runs.
        let update = [Imm(0), Imm(0), Imm(0), Imm(0)];
        method(
            a,
            l,
            fail,
            14,
            (SAMPLE, SAMPLE_UPDATE),
            &update,
            MS_E_NOTRUNNING,
        )?;
        method(a, l, fail, 15, (MMS, MMS_SET_STATE), &[Imm(1)], S_OK)?;
        // The first picture is red.
        method(a, l, fail, 16, (SAMPLE, SAMPLE_UPDATE), &update, S_OK)?;
        first_pixel(a, l, fail, 17)?;
        expect(a, fail, 19, 0xFF_0000)?;
        a.mov(dword_ptr(l.at(COUNT)), 1)?;
        // The rest, until the end.
        let mut next = a.create_label();
        let mut end = a.create_label();
        a.set_label(&mut next)?;
        invoke(a, l, (SAMPLE, SAMPLE_UPDATE), &update)?;
        a.cmp(eax, MS_S_ENDOFSTREAM as i32)?;
        a.je(end)?;
        expect(a, fail, 20, S_OK)?;
        a.inc(dword_ptr(l.at(COUNT)))?;
        a.jmp(next)?;
        a.set_label(&mut end)?;
        // The last is blue.
        first_pixel(a, l, fail, 21)?;
        expect(a, fail, 23, 0xFF)?;
        method(a, l, fail, 24, (SURFACE, RELEASE), &[], 1)?;
        method(a, l, fail, 25, (SAMPLE, RELEASE), &[], 0)?;
        method(a, l, fail, 26, (DDSTREAM, RELEASE), &[], 2)?;
        method(a, l, fail, 27, (STREAM, RELEASE), &[], 1)?;
        method(a, l, fail, 28, (MMS, RELEASE), &[], 0)?;
        // 100 and how many pictures it gave.
        a.add(dword_ptr(l.at(COUNT)), 100)?;
        exit(a, l, &mut fail, Val(COUNT))
    });
    let mut cpu = machine("stream", exe, &[("CLIP.AVI", clip())]);
    let code = run(&mut cpu);
    assert_eq!(code, 112, "{}", printed(&cpu));
}

// A renderer filter of the program's own, as Omikron's "Psygnosis
// Renderer" is: the places in its program's data.
const R_IID_MEM_INPUT: u32 = 0x40;
const R_IID_SINK: u32 = 0x50;
const R_FILTER: u32 = 0x110;
const R_ENUM: u32 = 0x114;
const R_PIN: u32 = 0x118;
const R_INPUT: u32 = 0x11C;
const R_JOINED: u32 = 0x120;
const R_CLOCK: u32 = 0x124;
const R_ENUM_COUNT: u32 = 0x128;
const R_ATTEMPTS: u32 = 0x12C;
const R_CONNECTOR: u32 = 0x130;
const R_ALLOCATOR: u32 = 0x134;
const R_RECEIVED: u32 = 0x138;
const R_ACTUAL: u32 = 0x13C;
const R_START: u32 = 0x140;
const R_STOP: u32 = 0x148;
const R_EOS: u32 = 0x150;
const R_SEGMENTS: u32 = 0x154;
const R_STATE: u32 = 0x158;
const R_SINK: u32 = 0x15C;
const R_PIXEL: u32 = 0x160;
const R_SAMPLE: u32 = 0x164;
const R_POINTER: u32 = 0x168;
const R_RUNS: u32 = 0x16C;
const R_CODE: u32 = 0x170;
const R_NAME: u32 = 0x180;
const R_FILE: u32 = 0x1C0;
const R_TYPE: u32 = 0x200;
const R_FORMAT: u32 = 0x250;
const R_VT_FILTER: u32 = 0x300;
const R_VT_ENUM: u32 = 0x340;
const R_VT_PIN: u32 = 0x360;
const R_VT_INPUT: u32 = 0x3B0;

const E_NOTIMPL: u32 = 0x8000_4001;
const VFW_E_NOT_CONNECTED: u32 = 0x8004_0209;
const VFW_E_NO_ALLOCATOR: u32 = 0x8004_020A;
const VFW_E_TYPE_NOT_ACCEPTED: u32 = 0x8004_022A;
const EVENT_EX_IID: [u8; 16] = quartz(0x56A8_68C0);
const GRAPH_ADD_FILTER: i32 = 12;
const CONTROL_STOP: i32 = 36;

fn renderer_data() -> Vec<u8> {
    let mut d = vec![0u8; SIZE];
    let mut put = |at: u32, bytes: &[u8]| {
        d[at as usize..at as usize + bytes.len()].copy_from_slice(bytes);
    };
    put(G_CLSID_GRAPH, &CLSID_FILTER_GRAPH);
    put(G_IID_GRAPH, &IID_IGRAPH_BUILDER);
    put(G_IID_CONTROL, &IID_IMEDIA_CONTROL);
    put(G_IID_EVENT, &EVENT_EX_IID);
    put(R_IID_MEM_INPUT, &quartz(0x56A8_689D));
    put(R_IID_SINK, &quartz(0x56A8_68A2));
    let wide = |s: &str| -> Vec<u8> {
        s.encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect()
    };
    put(R_NAME, &wide("Test Renderer"));
    put(R_FILE, &wide("CLIP.MPG"));
    d
}

/// A method that returns `value` and takes `args` arguments with `this`.
fn returns(
    a: &mut CodeAssembler,
    label: &mut CodeLabel,
    value: u32,
    args: u32,
) -> Result<(), IcedError> {
    a.set_label(label)?;
    a.mov(eax, value)?;
    a.ret_1(4 * (args + 1) as i32)
}

/// Fill the vtable at `at` with `methods`, and leave its address in EAX.
/// 32-bit code can't take a label's address, so each entry is a jump to
/// its method that a call steps over, which pushes the jump's address.
fn vtable(
    a: &mut CodeAssembler,
    l: &Layout,
    at: u32,
    methods: &[CodeLabel],
) -> Result<(), IcedError> {
    for (k, &m) in methods.iter().enumerate() {
        let mut after = a.create_label();
        a.call(after)?;
        a.jmp(m)?;
        a.set_label(&mut after)?;
        a.pop(eax)?;
        a.mov(dword_ptr(l.at(at + 4 * k as u32)), eax)?;
    }
    a.mov(eax, l.at(at) as u32)?;
    Ok(())
}

/// Call method `offset` of the interface at the address `this` (an
/// object of the program's data), with `args` pushed before it.
fn call_on(a: &mut CodeAssembler, this: AsmMemoryOperand, offset: i32) -> Result<(), IcedError> {
    a.mov(eax, this)?;
    a.push(eax)?;
    a.mov(eax, dword_ptr(eax))?;
    a.call(dword_ptr(eax + offset))
}

/// End with exit code `step` unless the dword at `at` is `want`.
fn check(
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

/// A renderer filter written as a program writes one with DirectShow's
/// base classes, but for its only pin's media types: an input pin that
/// takes RGB565 alone, as Omikron's renderer on a 16-bit display does,
/// and that keeps what it is given. The graph connects the MPEG video
/// decoder to it when it renders a movie, after the decoder's YV12,
/// YUY2, UYVY, RGB24 and RGB32, which it refuses; the pin gets twelve
/// 32 by 32 pictures, each in a sample from the decoder's own allocator
/// timed from the start, then EndOfStream, and tells the graph
/// EC_COMPLETE, which WaitForCompletion gives. Releasing the graph takes
/// the filter out of it.
#[test]
fn a_programs_renderer_filter_gets_the_movies_pictures() {
    let exe = program(IMPORTS, &renderer_data(), |a, l| {
        let mut fail = a.create_label();
        let mut start = a.create_label();
        a.jmp(start)?;

        // The filter's methods.
        let mut not_impl = [(); 4].map(|_| a.create_label());
        for (k, label) in not_impl.iter_mut().enumerate() {
            returns(a, label, E_NOTIMPL, k as u32)?;
        }
        let mut one = a.create_label();
        returns(a, &mut one, 1, 0)?;
        let mut no_interface = a.create_label();
        a.set_label(&mut no_interface)?;
        a.mov(eax, dword_ptr(esp + 12))?;
        a.mov(dword_ptr(eax), 0)?;
        a.mov(eax, E_NOINTERFACE)?;
        a.ret_1(12)?;
        let mut state = [(); 3].map(|_| a.create_label());
        for (k, label) in state.iter_mut().enumerate() {
            a.set_label(label)?;
            a.mov(dword_ptr(l.at(R_STATE)), k as u32)?;
            if k == 2 {
                a.inc(dword_ptr(l.at(R_RUNS)))?;
                a.xor(eax, eax)?;
                a.ret_1(12)?;
            } else {
                a.xor(eax, eax)?;
                a.ret_1(4)?;
            }
        }
        let mut set_sync_source = a.create_label();
        a.set_label(&mut set_sync_source)?;
        a.mov(eax, dword_ptr(esp + 8))?;
        a.mov(dword_ptr(l.at(R_CLOCK)), eax)?;
        a.xor(eax, eax)?;
        a.ret_1(8)?;
        let mut enum_pins = a.create_label();
        a.set_label(&mut enum_pins)?;
        a.mov(eax, dword_ptr(esp + 8))?;
        a.mov(dword_ptr(eax), l.at(R_ENUM) as u32)?;
        a.mov(dword_ptr(l.at(R_ENUM_COUNT)), 0)?;
        a.xor(eax, eax)?;
        a.ret_1(8)?;
        let mut join = a.create_label();
        a.set_label(&mut join)?;
        a.mov(eax, dword_ptr(esp + 8))?;
        a.mov(dword_ptr(l.at(R_JOINED)), eax)?;
        a.xor(eax, eax)?;
        a.ret_1(12)?;

        // IEnumPins::Next: the pin, once.
        let mut next = a.create_label();
        let mut none = a.create_label();
        let mut counted = a.create_label();
        a.set_label(&mut next)?;
        a.cmp(dword_ptr(l.at(R_ENUM_COUNT)), 0)?;
        a.jne(none)?;
        a.inc(dword_ptr(l.at(R_ENUM_COUNT)))?;
        a.mov(eax, dword_ptr(esp + 12))?;
        a.mov(dword_ptr(eax), l.at(R_PIN) as u32)?;
        a.mov(eax, dword_ptr(esp + 16))?;
        a.test(eax, eax)?;
        a.jz(counted)?;
        a.mov(dword_ptr(eax), 1)?;
        a.set_label(&mut counted)?;
        a.xor(eax, eax)?;
        a.ret_1(16)?;
        a.set_label(&mut none)?;
        a.mov(eax, dword_ptr(esp + 12))?;
        a.mov(dword_ptr(eax), 0)?;
        a.mov(eax, 1)?;
        a.ret_1(16)?;

        // The pin's QueryInterface: IMemInputPin.
        let mut pin_qi = a.create_label();
        a.set_label(&mut pin_qi)?;
        a.mov(eax, dword_ptr(esp + 8))?;
        for k in 0..4u32 {
            a.mov(ecx, dword_ptr(eax + 4 * k as i32))?;
            a.cmp(ecx, dword_ptr(l.at(R_IID_MEM_INPUT + 4 * k)))?;
            a.jne(no_interface)?;
        }
        a.mov(eax, dword_ptr(esp + 12))?;
        a.mov(dword_ptr(eax), l.at(R_INPUT) as u32)?;
        a.xor(eax, eax)?;
        a.ret_1(12)?;

        // ReceiveConnection: RGB565 alone, kept with its format.
        let mut receive_connection = a.create_label();
        let mut reject = a.create_label();
        a.set_label(&mut receive_connection)?;
        a.mov(eax, dword_ptr(esp + 12))?;
        a.inc(dword_ptr(l.at(R_ATTEMPTS)))?;
        a.cmp(dword_ptr(eax + 16), 0xE436_EB7Bu32 as i32)?;
        a.jne(reject)?;
        for k in 0..18 {
            a.mov(ecx, dword_ptr(eax + 4 * k))?;
            a.mov(dword_ptr(l.at(R_TYPE + 4 * k as u32)), ecx)?;
        }
        a.mov(edx, dword_ptr(eax + 68))?;
        for k in 0..25 {
            a.mov(ecx, dword_ptr(edx + 4 * k))?;
            a.mov(dword_ptr(l.at(R_FORMAT + 4 * k as u32)), ecx)?;
        }
        a.mov(eax, dword_ptr(esp + 8))?;
        a.mov(dword_ptr(l.at(R_CONNECTOR)), eax)?;
        a.xor(eax, eax)?;
        a.ret_1(12)?;
        a.set_label(&mut reject)?;
        a.mov(eax, VFW_E_TYPE_NOT_ACCEPTED)?;
        a.ret_1(12)?;

        let mut connected_to = a.create_label();
        a.set_label(&mut connected_to)?;
        a.mov(eax, dword_ptr(esp + 8))?;
        a.mov(dword_ptr(eax), 0)?;
        a.mov(eax, VFW_E_NOT_CONNECTED)?;
        a.ret_1(8)?;
        let mut direction = a.create_label();
        a.set_label(&mut direction)?;
        a.mov(eax, dword_ptr(esp + 8))?;
        a.mov(dword_ptr(eax), 0)?;
        a.xor(eax, eax)?;
        a.ret_1(8)?;
        let mut new_segment = a.create_label();
        a.set_label(&mut new_segment)?;
        a.inc(dword_ptr(l.at(R_SEGMENTS)))?;
        a.xor(eax, eax)?;
        a.ret_1(28)?;
        let mut ok0 = a.create_label();
        returns(a, &mut ok0, S_OK, 0)?;

        // EndOfStream: EC_COMPLETE, through the graph's IMediaEventSink.
        let mut end_of_stream = a.create_label();
        a.set_label(&mut end_of_stream)?;
        a.inc(dword_ptr(l.at(R_EOS)))?;
        push(a, l, &[Ptr(R_IID_SINK), Ptr(R_SINK)])?;
        call_on(a, dword_ptr(l.at(R_JOINED)), 0)?;
        push(a, l, &[Imm(1), Imm(0), Ptr(R_FILTER)])?;
        call_on(a, dword_ptr(l.at(R_SINK)), 12)?;
        call_on(a, dword_ptr(l.at(R_SINK)), RELEASE)?;
        a.xor(eax, eax)?;
        a.ret_1(4)?;

        // IMemInputPin.
        let mut get_allocator = a.create_label();
        a.set_label(&mut get_allocator)?;
        a.mov(eax, dword_ptr(esp + 8))?;
        a.mov(dword_ptr(eax), 0)?;
        a.mov(eax, VFW_E_NO_ALLOCATOR)?;
        a.ret_1(8)?;
        let mut notify_allocator = a.create_label();
        a.set_label(&mut notify_allocator)?;
        a.mov(eax, dword_ptr(esp + 8))?;
        a.mov(dword_ptr(l.at(R_ALLOCATOR)), eax)?;
        a.xor(eax, eax)?;
        a.ret_1(12)?;
        let mut receive = a.create_label();
        let mut received = a.create_label();
        a.set_label(&mut receive)?;
        a.inc(dword_ptr(l.at(R_RECEIVED)))?;
        a.mov(eax, dword_ptr(esp + 8))?;
        a.mov(dword_ptr(l.at(R_SAMPLE)), eax)?;
        // GetActualDataLength, GetTime, and the first sample's first
        // pixel through GetPointer.
        call_on(a, dword_ptr(l.at(R_SAMPLE)), 44)?;
        a.mov(dword_ptr(l.at(R_ACTUAL)), eax)?;
        push(a, l, &[Ptr(R_START), Ptr(R_STOP)])?;
        call_on(a, dword_ptr(l.at(R_SAMPLE)), 20)?;
        a.cmp(dword_ptr(l.at(R_RECEIVED)), 1)?;
        a.jne(received)?;
        push(a, l, &[Ptr(R_POINTER)])?;
        call_on(a, dword_ptr(l.at(R_SAMPLE)), 12)?;
        a.mov(eax, dword_ptr(l.at(R_POINTER)))?;
        a.movzx(eax, word_ptr(eax))?;
        a.mov(dword_ptr(l.at(R_PIXEL)), eax)?;
        a.set_label(&mut received)?;
        a.xor(eax, eax)?;
        a.ret_1(8)?;

        a.set_label(&mut start)?;
        let [_, n1, n2, n3] = not_impl;
        let [stop, pause, run_] = state;
        vtable(
            a,
            l,
            R_VT_FILTER,
            &[
                no_interface,
                one,
                one,
                n1,
                stop,
                pause,
                run_,
                n2,
                set_sync_source,
                n1,
                enum_pins,
                n2,
                n1,
                join,
                n1,
            ],
        )?;
        a.mov(dword_ptr(l.at(R_FILTER)), eax)?;
        vtable(
            a,
            l,
            R_VT_ENUM,
            &[no_interface, one, one, next, n1, ok0, n1],
        )?;
        a.mov(dword_ptr(l.at(R_ENUM)), eax)?;
        vtable(
            a,
            l,
            R_VT_PIN,
            &[
                pin_qi,
                one,
                one,
                n2,
                receive_connection,
                ok0,
                connected_to,
                n1,
                n1,
                direction,
                n1,
                n1,
                n1,
                n2,
                end_of_stream,
                ok0,
                ok0,
                new_segment,
            ],
        )?;
        a.mov(dword_ptr(l.at(R_PIN)), eax)?;
        vtable(
            a,
            l,
            R_VT_INPUT,
            &[
                no_interface,
                one,
                one,
                get_allocator,
                notify_allocator,
                n1,
                receive,
                n3,
                ok0,
            ],
        )?;
        a.mov(dword_ptr(l.at(R_INPUT)), eax)?;

        // The program.
        call(a, l, fail, 1, CO_INITIALIZE, &[Imm(0)], S_OK)?;
        let args = [
            Ptr(G_CLSID_GRAPH),
            Imm(0),
            Imm(1),
            Ptr(G_IID_GRAPH),
            Ptr(GRAPH),
        ];
        call(a, l, fail, 2, CO_CREATE, &args, S_OK)?;
        let args = [Ptr(R_FILTER), Ptr(R_NAME)];
        method(a, l, fail, 3, (GRAPH, GRAPH_ADD_FILTER), &args, S_OK)?;
        // It joined the graph as the graph's IFilterGraph.
        a.mov(ebx, 4)?;
        a.mov(eax, dword_ptr(l.at(R_JOINED)))?;
        a.cmp(eax, dword_ptr(l.at(GRAPH)))?;
        a.jne(fail)?;
        let args = [Ptr(G_IID_CONTROL), Ptr(CONTROL)];
        method(a, l, fail, 5, (GRAPH, QUERY_INTERFACE), &args, S_OK)?;
        let args = [Ptr(G_IID_EVENT), Ptr(EVENT)];
        method(a, l, fail, 6, (GRAPH, QUERY_INTERFACE), &args, S_OK)?;
        let args = [Ptr(R_FILE), Imm(0)];
        method(a, l, fail, 7, (GRAPH, GRAPH_RENDER_FILE), &args, S_OK)?;
        // The decoder's types in order, RGB565 the sixth.
        check(a, l, fail, 8, R_ATTEMPTS, 6)?;
        check(a, l, fail, 9, R_TYPE, 0x7364_6976)?;
        check(a, l, fail, 10, R_TYPE + 16, 0xE436_EB7B)?;
        check(a, l, fail, 11, R_TYPE + 40, 32 * 32 * 2)?;
        check(a, l, fail, 12, R_TYPE + 44, 0x0558_9F80)?;
        check(a, l, fail, 13, R_TYPE + 64, 100)?;
        check(a, l, fail, 14, R_FORMAT + 52, 32)?;
        check(a, l, fail, 15, R_FORMAT + 56, 32)?;
        check(a, l, fail, 16, R_FORMAT + 64, 3)?;
        check(a, l, fail, 17, R_FORMAT + 88, 0xF800)?;
        a.movzx(eax, word_ptr(l.at(R_FORMAT + 62)))?;
        expect(a, fail, 18, 16)?;
        method(a, l, fail, 19, (CONTROL, CONTROL_RUN), &[], S_OK)?;
        let args = [Imm(5000), Ptr(R_CODE)];
        method(
            a,
            l,
            fail,
            20,
            (EVENT, EVENT_WAIT_FOR_COMPLETION),
            &args,
            S_OK,
        )?;
        check(a, l, fail, 21, R_CODE, 1)?;
        method(a, l, fail, 22, (CONTROL, CONTROL_STOP), &[], S_OK)?;
        check(a, l, fail, 23, R_STATE, 0)?;
        check(a, l, fail, 24, R_RUNS, 1)?;
        check(a, l, fail, 25, R_RECEIVED, 12)?;
        check(a, l, fail, 26, R_ACTUAL, 32 * 32 * 2)?;
        // The last picture's time: 11 of 40 ms.
        check(a, l, fail, 27, R_START, 4_400_000)?;
        check(a, l, fail, 28, R_STOP, 4_800_000)?;
        check(a, l, fail, 29, R_EOS, 1)?;
        check(a, l, fail, 30, R_SEGMENTS, 1)?;
        // The pictures' first six are red.
        a.mov(eax, dword_ptr(l.at(R_PIXEL)))?;
        a.shr(eax, 11)?;
        a.mov(ebx, 31)?;
        a.cmp(eax, 28)?;
        a.jb(fail)?;
        for (step, at) in [(32, R_CLOCK), (33, R_ALLOCATOR), (34, R_CONNECTOR)] {
            a.mov(ebx, step)?;
            a.cmp(dword_ptr(l.at(at)), 0)?;
            a.je(fail)?;
        }
        method(a, l, fail, 35, (EVENT, RELEASE), &[], 2)?;
        method(a, l, fail, 36, (CONTROL, RELEASE), &[], 1)?;
        method(a, l, fail, 37, (GRAPH, RELEASE), &[], 0)?;
        // The filter left the graph.
        check(a, l, fail, 38, R_JOINED, 0)?;
        exit(a, l, &mut fail, Imm(0))
    });
    let mut cpu = machine(
        "renderer",
        exe,
        &[("CLIP.MPG", super::win32_winmm_tests::clip())],
    );
    let code = run(&mut cpu);
    assert_eq!(code, 0, "{}", printed(&cpu));
}

// "Transform and renderer": the places in its program's data. A filter
// is its vtable, its pins' enumerator, the count Pause found, and the
// graph it joined; an enumerator its vtable, how far it got and its pins,
// ending with 0; a pin its vtable, its direction, its peer, how often
// Connect and Disconnect were called, and its IMemInputPin (0 for none).
const T_COUNTER: u32 = 0x110;
const T_TRANSFORM: u32 = 0x200;
const T_RENDERER: u32 = 0x220;
const T_TRANSFORM_ENUM: u32 = 0x240;
const T_RENDERER_ENUM: u32 = 0x260;
const T_IN: u32 = 0x280;
const T_OUT: u32 = 0x2A0;
const T_RENDERER_IN: u32 = 0x2C0;
const T_INPUT: u32 = 0x2E0;
const T_VT_FILTER: u32 = 0x400;
const T_VT_ENUM: u32 = 0x440;
const T_VT_PIN: u32 = 0x460;
const T_VT_INPUT: u32 = 0x4B0;
const T_SIZE: usize = 0x500;
const FILTER_PAUSED: u32 = 8;
const FILTER_JOINED: u32 = 16;
const PIN_PEER: u32 = 8;
const PIN_CONNECTS: u32 = 12;
const PIN_DISCONNECTS: u32 = 16;
const CONTROL_PAUSE: i32 = 32;

/// A program's transform filter and renderer, added in that order, as
/// Submarine Titans adds its pair. The transform's input pin takes the
/// decoder's first type and its output pin connects with its own
/// IPin::Connect; the renderer has one input pin. RenderFile connects
/// the decoder to the transform, then the transform's output pin to the
/// renderer's input pin, as Windows' graph builder renders a filter's
/// outputs after its input. The graph pauses the renderer before the
/// transform, and as it goes it disconnects both ends of that
/// connection and takes both filters out of it.
#[test]
fn a_programs_transform_filter_gets_its_renderer_connected() {
    let mut data = vec![0u8; T_SIZE];
    let mut put = |at: u32, bytes: &[u8]| {
        data[at as usize..at as usize + bytes.len()].copy_from_slice(bytes);
    };
    put(G_CLSID_GRAPH, &CLSID_FILTER_GRAPH);
    put(G_IID_GRAPH, &IID_IGRAPH_BUILDER);
    put(G_IID_CONTROL, &IID_IMEDIA_CONTROL);
    let file: Vec<u8> = "CLIP.MPG\0"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    put(R_FILE, &file);
    // The output pin's direction.
    put(T_OUT + 4, &1u32.to_le_bytes());
    let exe = program(IMPORTS, &data, |a, l| {
        let mut fail = a.create_label();
        let mut start = a.create_label();
        a.jmp(start)?;
        let at = |o: u32| l.at(o) as u32;

        let mut not_impl = [(); 3].map(|_| a.create_label());
        for (k, label) in not_impl.iter_mut().enumerate() {
            returns(a, label, E_NOTIMPL, k as u32)?;
        }
        let [_, n1, n2] = not_impl;
        let mut ok = [(); 3].map(|_| a.create_label());
        for (k, label) in ok.iter_mut().enumerate() {
            returns(a, label, S_OK, k as u32)?;
        }
        let [ok0, ok1, ok2] = ok;
        let mut ok_segment = a.create_label();
        returns(a, &mut ok_segment, S_OK, 6)?;
        let mut one = a.create_label();
        returns(a, &mut one, 1, 0)?;
        let mut no_interface = a.create_label();
        a.set_label(&mut no_interface)?;
        a.mov(eax, dword_ptr(esp + 12))?;
        a.mov(dword_ptr(eax), 0)?;
        a.mov(eax, E_NOINTERFACE)?;
        a.ret_1(12)?;

        // IBaseFilter.
        let mut pause = a.create_label();
        a.set_label(&mut pause)?;
        a.mov(eax, dword_ptr(esp + 4))?;
        a.inc(dword_ptr(l.at(T_COUNTER)))?;
        a.mov(ecx, dword_ptr(l.at(T_COUNTER)))?;
        a.mov(dword_ptr(eax + FILTER_PAUSED as i32), ecx)?;
        a.xor(eax, eax)?;
        a.ret_1(4)?;
        let mut enum_pins = a.create_label();
        a.set_label(&mut enum_pins)?;
        a.mov(eax, dword_ptr(esp + 4))?;
        a.mov(ecx, dword_ptr(eax + 4))?;
        a.mov(dword_ptr(ecx + 4), 0)?;
        a.mov(edx, dword_ptr(esp + 8))?;
        a.mov(dword_ptr(edx), ecx)?;
        a.xor(eax, eax)?;
        a.ret_1(8)?;
        let mut join = a.create_label();
        a.set_label(&mut join)?;
        a.mov(eax, dword_ptr(esp + 4))?;
        a.mov(ecx, dword_ptr(esp + 8))?;
        a.mov(dword_ptr(eax + FILTER_JOINED as i32), ecx)?;
        a.xor(eax, eax)?;
        a.ret_1(12)?;

        // IEnumPins::Next: the enumerator's pins, one at a time.
        let mut next = a.create_label();
        let mut none = a.create_label();
        let mut counted = a.create_label();
        a.set_label(&mut next)?;
        a.mov(eax, dword_ptr(esp + 4))?;
        a.mov(ecx, dword_ptr(eax + 4))?;
        a.mov(edx, dword_ptr(eax + ecx * 4 + 8))?;
        a.test(edx, edx)?;
        a.jz(none)?;
        a.inc(dword_ptr(eax + 4))?;
        a.mov(eax, dword_ptr(esp + 12))?;
        a.mov(dword_ptr(eax), edx)?;
        a.mov(eax, dword_ptr(esp + 16))?;
        a.test(eax, eax)?;
        a.jz(counted)?;
        a.mov(dword_ptr(eax), 1)?;
        a.set_label(&mut counted)?;
        a.xor(eax, eax)?;
        a.ret_1(16)?;
        a.set_label(&mut none)?;
        a.mov(eax, dword_ptr(esp + 12))?;
        a.mov(dword_ptr(eax), 0)?;
        a.mov(eax, 1)?;
        a.ret_1(16)?;

        // IPin.
        let mut pin_qi = a.create_label();
        let mut pin_qi_none = a.create_label();
        a.set_label(&mut pin_qi)?;
        a.mov(eax, dword_ptr(esp + 4))?;
        a.mov(ecx, dword_ptr(eax + 20))?;
        a.mov(edx, dword_ptr(esp + 12))?;
        a.mov(dword_ptr(edx), ecx)?;
        a.test(ecx, ecx)?;
        a.jz(pin_qi_none)?;
        a.xor(eax, eax)?;
        a.ret_1(12)?;
        a.set_label(&mut pin_qi_none)?;
        a.mov(eax, E_NOINTERFACE)?;
        a.ret_1(12)?;
        let mut connect = a.create_label();
        a.set_label(&mut connect)?;
        a.mov(eax, dword_ptr(esp + 4))?;
        a.mov(ecx, dword_ptr(esp + 8))?;
        a.mov(dword_ptr(eax + PIN_PEER as i32), ecx)?;
        a.inc(dword_ptr(eax + PIN_CONNECTS as i32))?;
        a.xor(eax, eax)?;
        a.ret_1(12)?;
        let mut receive_connection = a.create_label();
        a.set_label(&mut receive_connection)?;
        a.mov(eax, dword_ptr(esp + 4))?;
        a.mov(ecx, dword_ptr(esp + 8))?;
        a.mov(dword_ptr(eax + PIN_PEER as i32), ecx)?;
        a.xor(eax, eax)?;
        a.ret_1(12)?;
        let mut disconnect = a.create_label();
        a.set_label(&mut disconnect)?;
        a.mov(eax, dword_ptr(esp + 4))?;
        a.mov(dword_ptr(eax + PIN_PEER as i32), 0)?;
        a.inc(dword_ptr(eax + PIN_DISCONNECTS as i32))?;
        a.xor(eax, eax)?;
        a.ret_1(4)?;
        let mut connected_to = a.create_label();
        let mut not_connected = a.create_label();
        a.set_label(&mut connected_to)?;
        a.mov(eax, dword_ptr(esp + 4))?;
        a.mov(ecx, dword_ptr(eax + PIN_PEER as i32))?;
        a.mov(edx, dword_ptr(esp + 8))?;
        a.mov(dword_ptr(edx), ecx)?;
        a.test(ecx, ecx)?;
        a.jz(not_connected)?;
        a.xor(eax, eax)?;
        a.ret_1(8)?;
        a.set_label(&mut not_connected)?;
        a.mov(eax, VFW_E_NOT_CONNECTED)?;
        a.ret_1(8)?;
        let mut direction = a.create_label();
        a.set_label(&mut direction)?;
        a.mov(eax, dword_ptr(esp + 4))?;
        a.mov(ecx, dword_ptr(eax + 4))?;
        a.mov(edx, dword_ptr(esp + 8))?;
        a.mov(dword_ptr(edx), ecx)?;
        a.xor(eax, eax)?;
        a.ret_1(8)?;

        // IMemInputPin: the decoder's own allocator, and every sample
        // taken.
        let mut get_allocator = a.create_label();
        a.set_label(&mut get_allocator)?;
        a.mov(eax, dword_ptr(esp + 8))?;
        a.mov(dword_ptr(eax), 0)?;
        a.mov(eax, VFW_E_NO_ALLOCATOR)?;
        a.ret_1(8)?;

        a.set_label(&mut start)?;
        vtable(
            a,
            l,
            T_VT_FILTER,
            &[
                no_interface,
                one,
                one,
                n1,
                ok0,
                pause,
                ok2,
                n2,
                ok1,
                n1,
                enum_pins,
                n2,
                n1,
                join,
                n1,
            ],
        )?;
        for (filter, e) in [(T_TRANSFORM, T_TRANSFORM_ENUM), (T_RENDERER, T_RENDERER_ENUM)] {
            a.mov(dword_ptr(l.at(filter)), eax)?;
            a.mov(dword_ptr(l.at(filter + 4)), at(e))?;
        }
        vtable(a, l, T_VT_ENUM, &[no_interface, one, one, next, n1, ok0, n1])?;
        a.mov(dword_ptr(l.at(T_TRANSFORM_ENUM)), eax)?;
        a.mov(dword_ptr(l.at(T_RENDERER_ENUM)), eax)?;
        a.mov(dword_ptr(l.at(T_TRANSFORM_ENUM + 8)), at(T_IN))?;
        a.mov(dword_ptr(l.at(T_TRANSFORM_ENUM + 12)), at(T_OUT))?;
        a.mov(dword_ptr(l.at(T_RENDERER_ENUM + 8)), at(T_RENDERER_IN))?;
        vtable(
            a,
            l,
            T_VT_PIN,
            &[
                pin_qi,
                one,
                one,
                connect,
                receive_connection,
                disconnect,
                connected_to,
                n1,
                n1,
                direction,
                n1,
                ok1,
                n1,
                n2,
                ok0,
                ok0,
                ok0,
                ok_segment,
            ],
        )?;
        for pin in [T_IN, T_OUT, T_RENDERER_IN] {
            a.mov(dword_ptr(l.at(pin)), eax)?;
        }
        a.mov(dword_ptr(l.at(T_IN + 20)), at(T_INPUT))?;
        vtable(
            a,
            l,
            T_VT_INPUT,
            &[
                no_interface,
                one,
                one,
                get_allocator,
                ok2,
                n1,
                ok1,
                n2,
                ok0,
            ],
        )?;
        a.mov(dword_ptr(l.at(T_INPUT)), eax)?;

        // The program.
        call(a, l, fail, 1, CO_INITIALIZE, &[Imm(0)], S_OK)?;
        let args = [
            Ptr(G_CLSID_GRAPH),
            Imm(0),
            Imm(1),
            Ptr(G_IID_GRAPH),
            Ptr(GRAPH),
        ];
        call(a, l, fail, 2, CO_CREATE, &args, S_OK)?;
        for (step, filter) in [(3, T_TRANSFORM), (4, T_RENDERER)] {
            let args = [Ptr(filter), Imm(0)];
            method(a, l, fail, step, (GRAPH, GRAPH_ADD_FILTER), &args, S_OK)?;
        }
        let args = [Ptr(G_IID_CONTROL), Ptr(CONTROL)];
        method(a, l, fail, 5, (GRAPH, QUERY_INTERFACE), &args, S_OK)?;
        let args = [Ptr(R_FILE), Imm(0)];
        method(a, l, fail, 6, (GRAPH, GRAPH_RENDER_FILE), &args, S_OK)?;
        // The decoder took the transform's input, whose output pin
        // connected to the renderer's input pin once.
        a.mov(ebx, 7)?;
        a.cmp(dword_ptr(l.at(T_IN + PIN_PEER)), 0)?;
        a.je(fail)?;
        check(a, l, fail, 8, T_OUT + PIN_PEER, at(T_RENDERER_IN))?;
        check(a, l, fail, 9, T_OUT + PIN_CONNECTS, 1)?;
        check(a, l, fail, 10, T_RENDERER_IN + PIN_CONNECTS, 0)?;
        // The renderer pauses first.
        method(a, l, fail, 11, (CONTROL, CONTROL_PAUSE), &[], S_OK)?;
        check(a, l, fail, 12, T_RENDERER + FILTER_PAUSED, 1)?;
        check(a, l, fail, 13, T_TRANSFORM + FILTER_PAUSED, 2)?;
        method(a, l, fail, 14, (CONTROL, CONTROL_STOP), &[], S_OK)?;
        method(a, l, fail, 15, (CONTROL, RELEASE), &[], 1)?;
        method(a, l, fail, 16, (GRAPH, RELEASE), &[], 0)?;
        // Both ends of each connection disconnected, and both filters
        // left the graph.
        check(a, l, fail, 17, T_IN + PIN_DISCONNECTS, 1)?;
        check(a, l, fail, 18, T_OUT + PIN_DISCONNECTS, 1)?;
        check(a, l, fail, 19, T_RENDERER_IN + PIN_DISCONNECTS, 1)?;
        check(a, l, fail, 20, T_TRANSFORM + FILTER_JOINED, 0)?;
        check(a, l, fail, 21, T_RENDERER + FILTER_JOINED, 0)?;
        exit(a, l, &mut fail, Imm(0))
    });
    let mut cpu = machine(
        "transform",
        exe,
        &[("CLIP.MPG", super::win32_winmm_tests::clip())],
    );
    let code = run(&mut cpu);
    assert_eq!(code, 0, "{}", printed(&cpu));
}
