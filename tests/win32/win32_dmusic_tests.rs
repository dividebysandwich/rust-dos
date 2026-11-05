//! The built-in Windows' DirectMusic: Win32 programs made here with
//! rust-dos-pe's builder load a MIDI file with the loader, play it with
//! a performance, and read its notifications; what they play reaches the
//! MPU-401's synthesizer. The results are those Windows 98 SE gives.

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
    let base = PathBuf::from("target/test_win32_dmusic").join(test);
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

/// Run the program to its end, `watch` called after each batch; its exit
/// code.
fn run(cpu: &mut Cpu, mut watch: impl FnMut(&mut Cpu)) -> u8 {
    for _ in 0..50_000 {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        let stop = run_batch(cpu, &mut NoHook, false);
        watch(cpu);
        if stop == StopReason::ShellReloaded {
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
/// data, with `args`; its result must be `want`.
fn method(
    a: &mut CodeAssembler,
    l: &Layout,
    fail: CodeLabel,
    step: u32,
    (object, offset): (u32, i32),
    args: &[Arg],
    want: u32,
) -> Result<(), IcedError> {
    push(a, l, args)?;
    a.mov(eax, dword_ptr(l.at(object)))?;
    a.push(eax)?;
    a.mov(eax, dword_ptr(eax))?;
    a.call(dword_ptr(eax + offset))?;
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
    (
        "KERNEL32.dll",
        &[
            "ExitProcess",
            "Sleep",
            "CreateEventA",
            "WaitForSingleObject",
        ],
    ),
    (
        "OLE32.dll",
        &["CoInitialize", "CoInitializeEx", "CoCreateInstance"],
    ),
];
const EXIT: usize = 0;
const CREATE_EVENT: usize = 2;
const WAIT: usize = 3;
const CO_INITIALIZE: usize = 4;
const CO_INITIALIZE_EX: usize = 5;
const CO_CREATE: usize = 6;

const S_OK: u32 = 0;
const S_FALSE: u32 = 1;
const E_INVALIDARG: u32 = 0x8007_0057;

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

const fn dm(d1: u32) -> [u8; 16] {
    guid(
        d1,
        0xB39B,
        0x11D1,
        [0x87, 0x04, 0x00, 0x60, 0x08, 0x93, 0xB1, 0xBD],
    )
}

const CLSID_PERFORMANCE: [u8; 16] = dm(0xD2AC_2881);
const CLSID_SEGMENT: [u8; 16] = dm(0xD2AC_2882);
const CLSID_LOADER: [u8; 16] = dm(0xD2AC_2892);
const GUID_NOTIFICATION_SEGMENT: [u8; 16] = dm(0xD2AC_2899);
const CLSID_DIRECTMUSIC: [u8; 16] = guid(
    0x636B_9F10,
    0x0C7D,
    0x11D1,
    [0x95, 0xB2, 0x00, 0x20, 0xAF, 0xDC, 0x74, 0x21],
);
const IID_IUNKNOWN: [u8; 16] = guid(0, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_IDIRECTMUSIC: [u8; 16] = guid(
    0x6536_115A,
    0x7B2D,
    0x11D2,
    [0xBA, 0x18, 0x00, 0x00, 0xF8, 0x75, 0xAC, 0x12],
);
const IID_LOADER: [u8; 16] = guid(
    0x2FFA_ACA2,
    0x5DCA,
    0x11D2,
    [0xAF, 0xA6, 0x00, 0xAA, 0x00, 0x24, 0xD8, 0xB6],
);
const IID_PERFORMANCE: [u8; 16] = guid(
    0x07D4_3D03,
    0x6523,
    0x11D2,
    [0x87, 0x1D, 0x00, 0x60, 0x08, 0x93, 0xB1, 0xBD],
);
const IID_SEGMENT: [u8; 16] = guid(
    0xF960_29A2,
    0x4282,
    0x11D2,
    [0x87, 0x17, 0x00, 0x60, 0x08, 0x93, 0xB1, 0xBD],
);

// The places in the programs' data.
const G_CLSID_LOADER: u32 = 0x00;
const G_IID_LOADER: u32 = 0x10;
const G_CLSID_PERF: u32 = 0x20;
const G_IID_PERF: u32 = 0x30;
const G_IID_SEGMENT: u32 = 0x40;
const G_NOTIFICATION: u32 = 0x50;
const G_CLSID_DM: u32 = 0x60;
const G_IID_DM: u32 = 0x70;
const G_IID_UNKNOWN: u32 = 0x80;
const LOADER: u32 = 0x100;
const SEG: u32 = 0x104;
const SEG2: u32 = 0x108;
const PERF: u32 = 0x10C;
const MUSIC: u32 = 0x110;
const STATE: u32 = 0x114;
const EVENT: u32 = 0x118;
const MSG: u32 = 0x11C;
const VALUE: u32 = 0x120;
const PORT: u32 = 0x124;
const PARAMS: u32 = 0x140;
const CAPS: u32 = 0x200;
const DESC: u32 = 0x400;
const SIZE: usize = 0x800;

/// DMUS_OBJECTDESC: dwSize, dwValidData, guidClass at 24, wszFileName at
/// 312.
fn data() -> Vec<u8> {
    let mut d = vec![0u8; SIZE];
    let mut put = |at: u32, bytes: &[u8]| {
        d[at as usize..at as usize + bytes.len()].copy_from_slice(bytes);
    };
    put(G_CLSID_LOADER, &CLSID_LOADER);
    put(G_IID_LOADER, &IID_LOADER);
    put(G_CLSID_PERF, &CLSID_PERFORMANCE);
    put(G_IID_PERF, &IID_PERFORMANCE);
    put(G_IID_SEGMENT, &IID_SEGMENT);
    put(G_NOTIFICATION, &GUID_NOTIFICATION_SEGMENT);
    put(G_CLSID_DM, &CLSID_DIRECTMUSIC);
    put(G_IID_DM, &IID_IDIRECTMUSIC);
    put(G_IID_UNKNOWN, &IID_IUNKNOWN);
    // DMUS_OBJ_CLASS | DMUS_OBJ_FILENAME.
    put(DESC, &848u32.to_le_bytes());
    put(DESC + 4, &0x12u32.to_le_bytes());
    put(DESC + 24, &CLSID_SEGMENT);
    let name: Vec<u8> = "TEST.MID"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    put(DESC + 312, &name);
    put(PARAMS, &32u32.to_le_bytes());
    put(CAPS, &308u32.to_le_bytes());
    d
}

/// A MIDI file of one track at 240 beats a minute, 4/4, a measure of
/// quarter notes from middle C on channel 1: a second long.
fn test_mid() -> Vec<u8> {
    let mut track = vec![0, 0xFF, 0x51, 3, 0x03, 0xD0, 0x90];
    track.extend([0, 0xFF, 0x58, 4, 4, 2, 24, 8]);
    // 256 ticks a beat: each note half a beat (128 ticks, 81h 00h as
    // a delta) and as long a rest.
    for beat in 0..4u8 {
        if beat == 0 {
            track.push(0);
        } else {
            track.extend([0x81, 0x00]);
        }
        track.extend([0x90, 60 + beat, 100]);
        track.extend([0x81, 0x00, 0x80, 60 + beat, 0]);
    }
    track.extend([0, 0xFF, 0x2F, 0]);
    let mut out = b"MThd".to_vec();
    out.extend(6u32.to_be_bytes());
    out.extend([0, 0, 0, 1, 0x01, 0x00]);
    out.extend(b"MTrk");
    out.extend((track.len() as u32).to_be_bytes());
    out.extend(track);
    out
}

// Methods, as vtable offsets.
const RELEASE: i32 = 8;
const LOADER_GET_OBJECT: i32 = 12;
const SEGMENT_GET_LENGTH: i32 = 12;
const PERF_INIT: i32 = 12;
const PERF_PLAY_SEGMENT: i32 = 16;
const PERF_IS_PLAYING: i32 = 56;
const PERF_FREE_PMSG: i32 = 68;
const PERF_SET_NOTIFICATION_HANDLE: i32 = 80;
const PERF_GET_NOTIFICATION_PMSG: i32 = 84;
const PERF_ADD_NOTIFICATION_TYPE: i32 = 88;
const PERF_ADD_PORT: i32 = 96;
const PERF_PCHANNEL_INFO: i32 = 112;
const PERF_CLOSE_DOWN: i32 = 152;
const DM_ENUM_PORT: i32 = 12;
const DM_CREATE_PORT: i32 = 20;

/// The loader and the segment: CoInitialize, a loader, and the segment
/// of TEST.MID in `a`.
fn load(a: &mut CodeAssembler, l: &Layout, fail: CodeLabel) -> Result<(), IcedError> {
    call(a, l, fail, 1, CO_INITIALIZE, &[Imm(0)], S_OK)?;
    let args = [
        Ptr(G_CLSID_LOADER),
        Imm(0),
        Imm(1),
        Ptr(G_IID_LOADER),
        Ptr(LOADER),
    ];
    call(a, l, fail, 2, CO_CREATE, &args, S_OK)?;
    let args = [Ptr(DESC), Ptr(G_IID_SEGMENT), Ptr(SEG)];
    method(a, l, fail, 3, (LOADER, LOADER_GET_OBJECT), &args, S_OK)
}

#[test]
fn the_loader_makes_a_segment_of_a_midi_file() {
    let exe = program(IMPORTS, &data(), |a, l| {
        let mut fail = a.create_label();
        load(a, l, fail)?;
        // Four beats, rounded up to the measure: 3072.
        method(
            a,
            l,
            fail,
            4,
            (SEG, SEGMENT_GET_LENGTH),
            &[Ptr(VALUE)],
            S_OK,
        )?;
        a.mov(eax, dword_ptr(l.at(VALUE)))?;
        expect(a, fail, 5, 3072)?;
        // The cache gives the same object again.
        let args = [Ptr(DESC), Ptr(G_IID_SEGMENT), Ptr(SEG2)];
        method(a, l, fail, 6, (LOADER, LOADER_GET_OBJECT), &args, S_OK)?;
        a.mov(eax, dword_ptr(l.at(SEG2)))?;
        a.mov(ecx, dword_ptr(l.at(SEG)))?;
        a.sub(eax, ecx)?;
        expect(a, fail, 7, 0)?;
        // DMUS_OBJECTDESC of another size isn't taken.
        a.mov(dword_ptr(l.at(DESC)), 832)?;
        let args = [Ptr(DESC), Ptr(G_IID_SEGMENT), Ptr(SEG2)];
        method(
            a,
            l,
            fail,
            8,
            (LOADER, LOADER_GET_OBJECT),
            &args,
            E_INVALIDARG,
        )?;
        // The loader's cache holds the segment: one release leaves it.
        method(a, l, fail, 9, (SEG, RELEASE), &[], 2)?;
        exit(a, l, &mut fail)
    });
    let mut cpu = machine("loader", exe, &[("TEST.MID", test_mid())]);
    assert_eq!(run(&mut cpu, |_| {}), 0);
}

#[test]
fn a_performance_plays_a_segment_and_tells_of_it() {
    let exe = program(IMPORTS, &data(), |a, l| {
        let mut fail = a.create_label();
        load(a, l, fail)?;
        let args = [
            Ptr(G_CLSID_PERF),
            Imm(0),
            Imm(1),
            Ptr(G_IID_PERF),
            Ptr(PERF),
        ];
        call(a, l, fail, 10, CO_CREATE, &args, S_OK)?;
        let args = [Ptr(MUSIC), Imm(0), Imm(0)];
        method(a, l, fail, 11, (PERF, PERF_INIT), &args, S_OK)?;
        method(a, l, fail, 12, (PERF, PERF_ADD_PORT), &[Imm(0)], S_OK)?;
        // Performance channel 0 is the synthesizer's first group's
        // channel 0.
        let args = [Imm(0), Imm(0), Ptr(VALUE), Ptr(PORT)];
        method(a, l, fail, 13, (PERF, PERF_PCHANNEL_INFO), &args, S_OK)?;
        a.mov(eax, dword_ptr(l.at(VALUE)))?;
        expect(a, fail, 14, 1)?;
        let args = [Imm(0), Imm(0), Imm(0), Imm(0)];
        push(a, l, &args)?;
        a.call(l.import(CREATE_EVENT))?;
        a.mov(dword_ptr(l.at(EVENT)), eax)?;
        let args = [Val(EVENT), Imm(0), Imm(0)];
        method(
            a,
            l,
            fail,
            15,
            (PERF, PERF_SET_NOTIFICATION_HANDLE),
            &args,
            S_OK,
        )?;
        let args = [Ptr(G_NOTIFICATION)];
        method(
            a,
            l,
            fail,
            16,
            (PERF, PERF_ADD_NOTIFICATION_TYPE),
            &args,
            S_OK,
        )?;
        let args = [Val(SEG), Imm(0), Imm(0), Imm(0), Ptr(STATE)];
        method(a, l, fail, 17, (PERF, PERF_PLAY_SEGMENT), &args, S_OK)?;
        // Not before it starts, the latency on.
        let args = [Val(SEG), Imm(0)];
        method(a, l, fail, 18, (PERF, PERF_IS_PLAYING), &args, S_FALSE)?;
        // The notifications: the start, then (after the start, as the
        // segment is shorter than the prepare time) almost the end, and
        // the end.
        for (i, option) in [0u32, 2, 1].into_iter().enumerate() {
            let step = 20 + 10 * i as u32;
            call(a, l, fail, step, WAIT, &[Val(EVENT), Imm(5000)], 0)?;
            let args = [Ptr(MSG)];
            method(
                a,
                l,
                fail,
                step + 1,
                (PERF, PERF_GET_NOTIFICATION_PMSG),
                &args,
                S_OK,
            )?;
            a.mov(eax, dword_ptr(l.at(MSG)))?;
            a.mov(eax, dword_ptr(eax + 72))?;
            expect(a, fail, step + 2, option)?;
            if option == 0 {
                let args = [Val(SEG), Imm(0)];
                method(a, l, fail, step + 3, (PERF, PERF_IS_PLAYING), &args, S_OK)?;
            }
            let args = [Val(MSG)];
            method(a, l, fail, step + 4, (PERF, PERF_FREE_PMSG), &args, S_OK)?;
        }
        let args = [Val(SEG), Imm(0)];
        method(a, l, fail, 60, (PERF, PERF_IS_PLAYING), &args, S_FALSE)?;
        // The state was the program's alone at the end.
        method(a, l, fail, 61, (STATE, RELEASE), &[], 0)?;
        method(a, l, fail, 62, (PERF, PERF_CLOSE_DOWN), &[], S_OK)?;
        method(a, l, fail, 63, (PERF, RELEASE), &[], 0)?;
        method(a, l, fail, 64, (MUSIC, RELEASE), &[], 0)?;
        exit(a, l, &mut fail)
    });
    let mut cpu = machine("perf", exe, &[("TEST.MID", test_mid())]);
    // The notes the synthesizer held on channel 1, in turn.
    let mut notes: Vec<usize> = Vec::new();
    let code = run(&mut cpu, |cpu| {
        let held = cpu.bus.mpu.held_notes();
        if let Some(n) = (0..128).find(|&n| held[0][n] != 0)
            && notes.last() != Some(&n)
        {
            notes.push(n);
        }
    });
    assert_eq!(code, 0, "{:?}", notes);
    assert_eq!(notes, [60, 61, 62, 63]);
}

#[test]
fn directmusic_lists_its_ports() {
    let exe = program(IMPORTS, &data(), |a, l| {
        let mut fail = a.create_label();
        call(a, l, fail, 1, CO_INITIALIZE, &[Imm(0)], S_OK)?;
        let args = [Ptr(G_CLSID_DM), Imm(0), Imm(1), Ptr(G_IID_DM), Ptr(MUSIC)];
        call(a, l, fail, 2, CO_CREATE, &args, S_OK)?;
        // The second port is the synthesizer: DLS, software and
        // DirectSound.
        let args = [Imm(1), Ptr(CAPS)];
        method(a, l, fail, 3, (MUSIC, DM_ENUM_PORT), &args, S_OK)?;
        a.mov(eax, dword_ptr(l.at(CAPS + 4)))?;
        expect(a, fail, 4, 0x85)?;
        let args = [Imm(2), Ptr(CAPS)];
        method(a, l, fail, 5, (MUSIC, DM_ENUM_PORT), &args, S_FALSE)?;
        // Its port needs DirectSound first (DMUS_E_DSOUND_NOT_SET).
        let args = [Ptr(CAPS + 8), Ptr(PARAMS), Ptr(PORT), Imm(0)];
        method(a, l, fail, 6, (MUSIC, DM_CREATE_PORT), &args, 0x8878_1139)?;
        exit(a, l, &mut fail)
    });
    let mut cpu = machine("ports", exe, &[]);
    assert_eq!(run(&mut cpu, |_| {}), 0);
}

#[test]
fn the_multithreaded_apartment_gets_only_iunknown() {
    let exe = program(IMPORTS, &data(), |a, l| {
        let mut fail = a.create_label();
        call(a, l, fail, 1, CO_INITIALIZE_EX, &[Imm(0), Imm(0)], S_OK)?;
        // REGDB_E_IIDNOTREG, as Windows 98 SE has no proxy for it.
        let args = [
            Ptr(G_CLSID_LOADER),
            Imm(0),
            Imm(1),
            Ptr(G_IID_LOADER),
            Ptr(LOADER),
        ];
        call(a, l, fail, 2, CO_CREATE, &args, 0x8004_0155)?;
        let args = [
            Ptr(G_CLSID_LOADER),
            Imm(0),
            Imm(1),
            Ptr(G_IID_UNKNOWN),
            Ptr(LOADER),
        ];
        call(a, l, fail, 3, CO_CREATE, &args, S_OK)?;
        exit(a, l, &mut fail)
    });
    let mut cpu = machine("mta", exe, &[]);
    assert_eq!(run(&mut cpu, |_| {}), 0);
}

#[test]
fn the_dlls_are_there_for_programs_that_import_them() {
    let imports: &[(&str, &[&str])] = &[
        ("KERNEL32.dll", &["ExitProcess"]),
        ("DMUSIC.dll", &["DllCanUnloadNow"]),
        ("DMIME.dll", &["DllCanUnloadNow"]),
        ("DMLOADER.dll", &["DllGetClassObject"]),
    ];
    let exe = program(imports, &[0; 4], |a, l| {
        a.call(l.import(1))?;
        a.push(eax)?;
        a.call(l.import(EXIT))?;
        a.int3()
    });
    let mut cpu = machine("import", exe, &[]);
    // S_OK: the process has no DirectMusic objects.
    assert_eq!(run(&mut cpu, |_| {}), 0);
}

#[test]
fn objects_load_from_streams_on_global_memory() {
    let imports: &[(&str, &[&str])] = &[
        ("KERNEL32.dll", &["ExitProcess"]),
        (
            "OLE32.dll",
            &["CoInitialize", "CoCreateInstance", "CreateStreamOnHGlobal"],
        ),
    ];
    const CO_INIT: usize = 1;
    const CREATE: usize = 2;
    const CREATE_STREAM: usize = 3;
    // IStream's and the others' methods, as vtable offsets.
    const QUERY_INTERFACE: i32 = 0;
    const STREAM_WRITE: i32 = 16;
    const STREAM_SEEK: i32 = 20;
    const PERSIST_LOAD: i32 = 20;
    const OBJECT_PARSE_DESCRIPTOR: i32 = 20;
    // Places of this program's data, after data()'s.
    const STREAM: u32 = 0x128;
    const PERSIST: u32 = 0x12C;
    const OBJECT: u32 = 0x130;
    const G_CLSID_SEGMENT: u32 = 0x90;
    const G_IID_PERSIST: u32 = 0xA0;
    const G_IID_OBJECT: u32 = 0xB0;
    const DESC2: u32 = 0x800;
    const MIDI: u32 = 0xC00;
    let midi = test_mid();
    let mut d = data();
    d.resize(0x1000, 0);
    let mut put = |at: u32, bytes: &[u8]| {
        d[at as usize..at as usize + bytes.len()].copy_from_slice(bytes);
    };
    put(G_CLSID_SEGMENT, &CLSID_SEGMENT);
    put(
        G_IID_PERSIST,
        &guid(0x109, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]),
    );
    put(G_IID_OBJECT, &dm(0xD2AC_28B5));
    // DMUS_OBJ_CLASS | DMUS_OBJ_STREAM, pStream filled in below.
    put(DESC + 4, &0x802u32.to_le_bytes());
    put(DESC2, &848u32.to_le_bytes());
    put(MIDI, &midi);
    let seek = |a: &mut CodeAssembler, l: &Layout, fail, step| {
        let args = [Imm(0), Imm(0), Imm(0), Imm(0)];
        method(a, l, fail, step, (STREAM, STREAM_SEEK), &args, S_OK)
    };
    let exe = program(imports, &d, |a, l| {
        let mut fail = a.create_label();
        call(a, l, fail, 1, CO_INIT, &[Imm(0)], S_OK)?;
        let args = [Imm(0), Imm(1), Ptr(STREAM)];
        call(a, l, fail, 2, CREATE_STREAM, &args, S_OK)?;
        let args = [Ptr(MIDI), Imm(midi.len() as u32), Imm(0)];
        method(a, l, fail, 3, (STREAM, STREAM_WRITE), &args, S_OK)?;
        seek(a, l, fail, 4)?;
        // The loader reads the segment from the stream (DirectX 7's
        // DMUS_OBJ_STREAM).
        a.mov(eax, dword_ptr(l.at(STREAM)))?;
        a.mov(dword_ptr(l.at(DESC + 844)), eax)?;
        let args = [
            Ptr(G_CLSID_LOADER),
            Imm(0),
            Imm(1),
            Ptr(G_IID_LOADER),
            Ptr(LOADER),
        ];
        call(a, l, fail, 5, CREATE, &args, S_OK)?;
        let args = [Ptr(DESC), Ptr(G_IID_SEGMENT), Ptr(SEG)];
        method(a, l, fail, 6, (LOADER, LOADER_GET_OBJECT), &args, S_OK)?;
        method(
            a,
            l,
            fail,
            7,
            (SEG, SEGMENT_GET_LENGTH),
            &[Ptr(VALUE)],
            S_OK,
        )?;
        a.mov(eax, dword_ptr(l.at(VALUE)))?;
        expect(a, fail, 8, 3072)?;
        // A segment of its own loads it with IPersistStream::Load.
        let args = [
            Ptr(G_CLSID_SEGMENT),
            Imm(0),
            Imm(1),
            Ptr(G_IID_SEGMENT),
            Ptr(SEG2),
        ];
        call(a, l, fail, 9, CREATE, &args, S_OK)?;
        let args = [Ptr(G_IID_PERSIST), Ptr(PERSIST)];
        method(a, l, fail, 10, (SEG2, QUERY_INTERFACE), &args, S_OK)?;
        seek(a, l, fail, 11)?;
        let args = [Val(STREAM)];
        method(a, l, fail, 12, (PERSIST, PERSIST_LOAD), &args, S_OK)?;
        method(
            a,
            l,
            fail,
            13,
            (SEG2, SEGMENT_GET_LENGTH),
            &[Ptr(VALUE)],
            S_OK,
        )?;
        a.mov(eax, dword_ptr(l.at(VALUE)))?;
        expect(a, fail, 14, 3072)?;
        // ParseDescriptor gives the class of what the stream holds.
        let args = [Ptr(G_IID_OBJECT), Ptr(OBJECT)];
        method(a, l, fail, 15, (SEG2, QUERY_INTERFACE), &args, S_OK)?;
        seek(a, l, fail, 16)?;
        let args = [Val(STREAM), Ptr(DESC2)];
        method(
            a,
            l,
            fail,
            17,
            (OBJECT, OBJECT_PARSE_DESCRIPTOR),
            &args,
            S_OK,
        )?;
        a.mov(eax, dword_ptr(l.at(DESC2 + 4)))?;
        a.and(eax, 2)?;
        expect(a, fail, 18, 2)?;
        exit(a, l, &mut fail)
    });
    let mut cpu = machine("stream", exe, &[]);
    assert_eq!(run(&mut cpu, |_| {}), 0);
}
