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
