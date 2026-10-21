//! The built-in Windows' Direct3D (in DDRAW.DLL): Win32 programs that
//! make a device on the Voodoo3's HAL in a 16-bit mode, put vertices
//! through its transform and lighting, and draw with vertex buffers,
//! DrawPrimitive and execute buffers. The programs are PE files made
//! with rust-dos-pe's builder, their code assembled with iced, as in
//! win32_ddraw_tests.rs. Each leaves its results in its data and spins,
//! and the test looks at them while the program is still there.

use iced_x86::code_asm::*;
use rust_dos::cpu::{Cpu, HostConsole};
use rust_dos::exec::{NoHook, run_batch};
use rust_dos::video::adapter::{Adapter, VideoSetup};
use rust_dos::voodoo::Board;
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

    /// The import address table slot of the function `index`.
    fn import(&self, index: usize) -> AsmMemoryOperand {
        dword_ptr(self.iat[index] as u64)
    }
}

/// A program importing `imports`, with `data` and the code `f` makes,
/// and where its data is.
fn program(
    imports: &[(&str, &[&str])],
    data: &[u8],
    f: impl FnOnce(&mut CodeAssembler, &Layout) -> Result<(), IcedError>,
) -> (Vec<u8>, u32) {
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
    assert_eq!(
        b.section(
            ".text",
            &code,
            pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ
        ),
        code_rva
    );
    b.entry(code_rva);
    (b.build(), layout.data)
}

/// A machine with a Voodoo3 as its display adapter, the built-in
/// Windows on and `program` loaded from the shell as T.EXE.
fn machine(test: &str, program: Vec<u8>) -> Cpu {
    let base = PathBuf::from("target/test_win32_d3d").join(test);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join("T.EXE"), program).unwrap();
    let mut cpu = Cpu::new(base);
    let setup = VideoSetup {
        adapter: Adapter::Voodoo3,
        mono_monitor: false,
    };
    rust_dos::video::bios::install(&mut cpu.bus, setup);
    cpu.bus.configure_voodoo(Some(Board::Voodoo3));
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    cpu
}

/// What the program printed.
fn printed(cpu: &Cpu) -> String {
    let console = cpu.host_console.as_ref().unwrap();
    String::from_utf8_lossy(&console.stdout).into_owned()
}

/// The program's dword at `lin`, through its page tables.
fn peek(cpu: &Cpu, lin: u32) -> u32 {
    let pde = cpu
        .bus
        .read_32((cpu.cr3 & !0xFFF) as usize + 4 * (lin >> 22) as usize);
    assert!(pde & 1 != 0, "no page table for {lin:X}");
    let pte = cpu
        .bus
        .read_32((pde & !0xFFF) as usize + 4 * ((lin >> 12) & 0x3FF) as usize);
    assert!(pte & 1 != 0, "no page at {lin:X}");
    cpu.bus
        .read_32((pte & !0xFFF) as usize + (lin & 0xFFF) as usize)
}

fn peek_f32(cpu: &Cpu, lin: u32) -> f32 {
    f32::from_bits(peek(cpu, lin))
}

/// Run the program until it says it is done (DONE in its data), with
/// its results there.
fn run_until_done(cpu: &mut Cpu, data: u32) {
    for _ in 0..400 {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        run_batch(cpu, &mut NoHook, false);
        // The program's memory is there while its code runs, with paging.
        let paged = cpu.cr0 & 0x8000_0001 == 0x8000_0001;
        if paged && peek(cpu, data + DONE) == 1 {
            return;
        }
    }
    panic!(
        "the program didn't finish: {:?}, at {:X}",
        printed(cpu),
        cpu.eip()
    );
}

// The data's layout.

const HWND: u32 = 0x00;
const DD: u32 = 0x04;
const D3D: u32 = 0x08;
const TARGET: u32 = 0x0C;
const DEVICE: u32 = 0x10;
const SOURCE: u32 = 0x14;
const DEST: u32 = 0x18;
const LOCKED: u32 = 0x1C;
const VIEWPORT: u32 = 0x20;
const DEVICE1: u32 = 0x24;
const BUFFER: u32 = 0x28;
/// The HRESULTs of the calls, in order.
const RESULTS: u32 = 0x40;
const DONE: u32 = 0xF0;
const CLASS: u32 = 0x100;
const IID_DD7: u32 = 0x110;
const IID_D3D7: u32 = 0x120;
const IID_HAL: u32 = 0x130;
const IID_D3D2: u32 = 0x140;
const IID_DEVICE1: u32 = 0x150;
const DESC: u32 = 0x200;
const VB_DESC: u32 = 0x280;
const VIEW: u32 = 0x2A0;
const LIGHT: u32 = 0x300;
const MATERIAL: u32 = 0x380;
const STATS: u32 = 0x3E0;
const VERTICES: u32 = 0x400;
const EB_DESC: u32 = 0x500;
const EXECUTE_DATA: u32 = 0x520;
const DATA_SIZE: usize = 0x600;

/// The data the programs start with: the window class's name and the
/// interface IDs.
fn data() -> Vec<u8> {
    let mut data = vec![0u8; DATA_SIZE];
    put(&mut data, CLASS, b"Static\0");
    guid(
        &mut data,
        IID_DD7,
        0x15E6_5EC0,
        0x3B9C,
        0x11D2,
        [0xB9, 0x2F, 0x00, 0x60, 0x97, 0x97, 0xEA, 0x5B],
    );
    guid(
        &mut data,
        IID_D3D7,
        0xF504_9E77,
        0x4861,
        0x11D2,
        [0xA4, 0x07, 0x00, 0xA0, 0xC9, 0x06, 0x29, 0xA8],
    );
    guid(
        &mut data,
        IID_HAL,
        0x84E6_3DE0,
        0x46AA,
        0x11CF,
        [0x81, 0x6F, 0x00, 0x00, 0xC0, 0x20, 0x15, 0x6E],
    );
    guid(
        &mut data,
        IID_D3D2,
        0x6AAE_1EC1,
        0x662A,
        0x11D0,
        [0x88, 0x9D, 0x00, 0xAA, 0x00, 0xBB, 0xB7, 0x6A],
    );
    guid(
        &mut data,
        IID_DEVICE1,
        0x6410_8800,
        0x957D,
        0x11D0,
        [0x89, 0xAB, 0x00, 0xA0, 0xC9, 0x05, 0x41, 0x29],
    );
    data
}

fn put(data: &mut [u8], offset: u32, bytes: &[u8]) {
    data[offset as usize..offset as usize + bytes.len()].copy_from_slice(bytes);
}

fn put32(data: &mut [u8], offset: u32, value: u32) {
    put(data, offset, &value.to_le_bytes());
}

fn put_f32s(data: &mut [u8], offset: u32, values: &[f32]) {
    for (i, v) in values.iter().enumerate() {
        put32(data, offset + 4 * i as u32, v.to_bits());
    }
}

fn guid(data: &mut [u8], offset: u32, d1: u32, d2: u16, d3: u16, d4: [u8; 8]) {
    put32(data, offset, d1);
    put(data, offset + 4, &d2.to_le_bytes());
    put(data, offset + 6, &d3.to_le_bytes());
    put(data, offset + 8, &d4);
}

/// An argument a program passes.
#[derive(Clone, Copy)]
enum Arg {
    /// A value.
    I(u32),
    /// The address of the data's byte at an offset.
    At(u32),
    /// The value in the data at an offset.
    Val(u32),
}

fn push(a: &mut CodeAssembler, l: &Layout, args: &[Arg]) -> Result<(), IcedError> {
    for arg in args.iter().rev() {
        match *arg {
            Arg::I(v) => a.push(v as i32)?,
            Arg::At(o) => a.push(l.at(o) as i32)?,
            Arg::Val(o) => a.push(dword_ptr(l.at(o)))?,
        }
    }
    Ok(())
}

/// The program's call results, one after another in RESULTS.
struct Calls {
    next: u32,
}

impl Calls {
    fn keep(&mut self, a: &mut CodeAssembler, l: &Layout) -> Result<(), IcedError> {
        a.mov(dword_ptr(l.at(RESULTS + 4 * self.next)), eax)?;
        self.next += 1;
        Ok(())
    }

    /// Call the import `index` with `args`.
    fn api(
        &mut self,
        a: &mut CodeAssembler,
        l: &Layout,
        index: usize,
        args: &[Arg],
    ) -> Result<(), IcedError> {
        push(a, l, args)?;
        a.call(l.import(index))?;
        self.keep(a, l)
    }

    /// Call method `method` of the interface whose pointer is at data
    /// offset `object`, with `args` after `this`.
    fn com(
        &mut self,
        a: &mut CodeAssembler,
        l: &Layout,
        object: u32,
        method: u32,
        args: &[Arg],
    ) -> Result<(), IcedError> {
        push(a, l, args)?;
        a.mov(eax, dword_ptr(l.at(object)))?;
        a.push(eax)?;
        a.mov(eax, dword_ptr(eax))?;
        a.call(dword_ptr(eax + 4 * method))?;
        self.keep(a, l)
    }
}

/// Copy `len` bytes of the data at `from` to where the pointer at data
/// offset `to` points.
fn copy(a: &mut CodeAssembler, l: &Layout, from: u32, to: u32, len: u32) -> Result<(), IcedError> {
    a.mov(edi, dword_ptr(l.at(to)))?;
    a.mov(esi, l.at(from) as u32)?;
    a.mov(ecx, len)?;
    a.cld()?;
    a.rep().movsb()
}

/// The end of a program: DONE set, and it spins, its memory there to
/// look at.
fn finish(a: &mut CodeAssembler, l: &Layout) -> Result<(), IcedError> {
    a.mov(dword_ptr(l.at(DONE)), 1)?;
    let mut again = a.create_label();
    a.set_label(&mut again)?;
    a.jmp(again)
}

/// The results of the first `n` calls, read from the program's data.
fn results(cpu: &Cpu, data: u32, n: u32) -> Vec<u32> {
    (0..n).map(|i| peek(cpu, data + RESULTS + 4 * i)).collect()
}

// Methods' places in their vtables.
const QUERY_INTERFACE: u32 = 0;
const DD_CREATE_SURFACE: u32 = 6;
const DD_SET_COOPERATIVE_LEVEL: u32 = 20;
const DD_SET_DISPLAY_MODE: u32 = 21;
const D3D2_CREATE_VIEWPORT: u32 = 6;
const D3D2_CREATE_DEVICE: u32 = 8;
const D3D7_CREATE_DEVICE: u32 = 4;
const D3D7_CREATE_VERTEX_BUFFER: u32 = 5;
const DEV1_CREATE_EXECUTE_BUFFER: u32 = 6;
const DEV1_EXECUTE: u32 = 8;
const DEV2_GET_STATS: u32 = 5;
const DEV2_ADD_VIEWPORT: u32 = 6;
const DEV2_BEGIN_SCENE: u32 = 10;
const DEV2_END_SCENE: u32 = 11;
const DEV2_SET_CURRENT_VIEWPORT: u32 = 13;
const DEV2_DRAW_PRIMITIVE: u32 = 29;
const DEV7_BEGIN_SCENE: u32 = 5;
const DEV7_END_SCENE: u32 = 6;
const DEV7_SET_VIEWPORT: u32 = 13;
const DEV7_SET_MATERIAL: u32 = 16;
const DEV7_SET_LIGHT: u32 = 18;
const DEV7_DRAW_PRIMITIVE_VB: u32 = 31;
const DEV7_LIGHT_ENABLE: u32 = 44;
const VP2_SET_VIEWPORT2: u32 = 17;
const EB_LOCK: u32 = 4;
const EB_UNLOCK: u32 = 5;
const EB_SET_EXECUTE_DATA: u32 = 6;
const VB_LOCK: u32 = 3;
const VB_UNLOCK: u32 = 4;
const VB_PROCESS_VERTICES: u32 = 5;

const DDSD_CAPS: u32 = 0x1;
const DDSD_HEIGHT: u32 = 0x2;
const DDSD_WIDTH: u32 = 0x4;
const DDSCAPS_OFFSCREENPLAIN: u32 = 0x40;
const DDSCAPS_3DDEVICE: u32 = 0x2000;
const DDSCL_FULLSCREEN: u32 = 0x1;
const DDSCL_EXCLUSIVE: u32 = 0x10;
const WS_POPUP: u32 = 0x8000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const D3DFVF_XYZ: u32 = 0x2;
const D3DFVF_XYZRHW: u32 = 0x4;
const D3DFVF_NORMAL: u32 = 0x10;
const D3DFVF_DIFFUSE: u32 = 0x40;
const D3DVOP_TRANSFORM: u32 = 0x1;
const D3DVOP_LIGHT: u32 = 0x400;
const D3DPT_TRIANGLELIST: u32 = 4;
const D3DVT_TLVERTEX: u32 = 3;

const USER32: (&str, &[&str]) = ("USER32.DLL", &["CreateWindowExA"]);
const DDRAW: (&str, &[&str]) = ("DDRAW.DLL", &["DirectDrawCreate", "DirectDrawCreateEx"]);
const CREATE_WINDOW: usize = 0;
const DIRECT_DRAW_CREATE: usize = 1;
const DIRECT_DRAW_CREATE_EX: usize = 2;

/// A DDSURFACEDESC (or, `size` 124, DDSURFACEDESC2) at `offset` of an
/// off-screen surface to draw into, `w` x `h`.
fn target_desc(data: &mut [u8], offset: u32, size: u32, w: u32, h: u32) {
    put32(data, offset, size);
    put32(data, offset + 0x04, DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT);
    put32(data, offset + 0x08, h);
    put32(data, offset + 0x0C, w);
    put32(
        data,
        offset + 0x68,
        DDSCAPS_OFFSCREENPLAIN | DDSCAPS_3DDEVICE,
    );
}

/// Make a window, the DirectDraw object (with DirectDrawCreateEx for
/// IDirectDraw7 when `ex`), and a 640x480 16-bit mode in full screen.
fn start(a: &mut CodeAssembler, l: &Layout, c: &mut Calls, ex: bool) -> Result<(), IcedError> {
    c.api(
        a,
        l,
        CREATE_WINDOW,
        &[
            Arg::I(0),
            Arg::At(CLASS),
            Arg::At(CLASS),
            Arg::I(WS_POPUP | WS_VISIBLE),
            Arg::I(0),
            Arg::I(0),
            Arg::I(640),
            Arg::I(480),
            Arg::I(0),
            Arg::I(0),
            Arg::I(BASE),
            Arg::I(0),
        ],
    )?;
    a.mov(dword_ptr(l.at(HWND)), eax)?;
    if ex {
        c.api(
            a,
            l,
            DIRECT_DRAW_CREATE_EX,
            &[Arg::I(0), Arg::At(DD), Arg::At(IID_DD7), Arg::I(0)],
        )?;
    } else {
        c.api(
            a,
            l,
            DIRECT_DRAW_CREATE,
            &[Arg::I(0), Arg::At(DD), Arg::I(0)],
        )?;
    }
    c.com(
        a,
        l,
        DD,
        DD_SET_COOPERATIVE_LEVEL,
        &[Arg::Val(HWND), Arg::I(DDSCL_EXCLUSIVE | DDSCL_FULLSCREEN)],
    )?;
    let mode = [Arg::I(640), Arg::I(480), Arg::I(16), Arg::I(0), Arg::I(0)];
    let args = if ex { &mode[..] } else { &mode[..3] };
    c.com(a, l, DD, DD_SET_DISPLAY_MODE, args)
}

/// Direct3D 7: a device on a 64x64 surface, three vertices with normals
/// put through the transform and a directional light by
/// ProcessVertices into a buffer of screen vertices, which DrawPrimitiveVB
/// draws.
fn process_program() -> (Vec<u8>, u32) {
    let mut d = data();
    target_desc(&mut d, DESC, 124, 64, 64);
    // D3DVERTEXBUFFERDESC: the source's and the destination's.
    put32(&mut d, VB_DESC, 16);
    put32(&mut d, VB_DESC + 8, D3DFVF_XYZ | D3DFVF_NORMAL);
    put32(&mut d, VB_DESC + 12, 3);
    put32(&mut d, VB_DESC + 16, 16);
    put32(&mut d, VB_DESC + 24, D3DFVF_XYZRHW | D3DFVF_DIFFUSE);
    put32(&mut d, VB_DESC + 28, 3);
    // D3DVIEWPORT7: all of the surface, depths 0 to 1.
    put32(&mut d, VIEW + 8, 64);
    put32(&mut d, VIEW + 12, 64);
    put_f32s(&mut d, VIEW + 16, &[0.0, 1.0]);
    // D3DLIGHT7: directional, white, shining along +z.
    put32(&mut d, LIGHT, 3);
    put_f32s(&mut d, LIGHT + 4, &[1.0, 1.0, 1.0, 1.0]);
    put_f32s(&mut d, LIGHT + 64, &[0.0, 0.0, 1.0]);
    // D3DMATERIAL7: its diffuse color.
    put_f32s(&mut d, MATERIAL, &[1.0, 0.5, 0.25, 1.0]);
    // The vertices: positions, and normals facing the light.
    put_f32s(
        &mut d,
        VERTICES,
        &[
            0.0, 0.0, 0.5, 0.0, 0.0, -1.0, //
            1.0, 1.0, 0.0, 0.0, 0.0, -1.0, //
            -0.5, -0.5, 1.0, 0.0, 0.0, -1.0,
        ],
    );
    program(&[USER32, DDRAW], &d, |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c, true)?;
        c.com(
            a,
            l,
            DD,
            QUERY_INTERFACE,
            &[Arg::At(IID_D3D7), Arg::At(D3D)],
        )?;
        c.com(
            a,
            l,
            DD,
            DD_CREATE_SURFACE,
            &[Arg::At(DESC), Arg::At(TARGET), Arg::I(0)],
        )?;
        c.com(
            a,
            l,
            D3D,
            D3D7_CREATE_DEVICE,
            &[Arg::At(IID_HAL), Arg::Val(TARGET), Arg::At(DEVICE)],
        )?;
        c.com(
            a,
            l,
            D3D,
            D3D7_CREATE_VERTEX_BUFFER,
            &[Arg::At(VB_DESC), Arg::At(SOURCE), Arg::I(0)],
        )?;
        c.com(
            a,
            l,
            D3D,
            D3D7_CREATE_VERTEX_BUFFER,
            &[Arg::At(VB_DESC + 16), Arg::At(DEST), Arg::I(0)],
        )?;
        c.com(
            a,
            l,
            SOURCE,
            VB_LOCK,
            &[Arg::I(0), Arg::At(LOCKED), Arg::I(0)],
        )?;
        copy(a, l, VERTICES, LOCKED, 72)?;
        c.com(a, l, SOURCE, VB_UNLOCK, &[])?;
        c.com(a, l, DEVICE, DEV7_SET_VIEWPORT, &[Arg::At(VIEW)])?;
        c.com(a, l, DEVICE, DEV7_SET_LIGHT, &[Arg::I(0), Arg::At(LIGHT)])?;
        c.com(a, l, DEVICE, DEV7_LIGHT_ENABLE, &[Arg::I(0), Arg::I(1)])?;
        c.com(a, l, DEVICE, DEV7_SET_MATERIAL, &[Arg::At(MATERIAL)])?;
        c.com(
            a,
            l,
            DEST,
            VB_PROCESS_VERTICES,
            &[
                Arg::I(D3DVOP_TRANSFORM | D3DVOP_LIGHT),
                Arg::I(0),
                Arg::I(3),
                Arg::Val(SOURCE),
                Arg::I(0),
                Arg::Val(DEVICE),
                Arg::I(0),
            ],
        )?;
        c.com(a, l, DEVICE, DEV7_BEGIN_SCENE, &[])?;
        c.com(
            a,
            l,
            DEVICE,
            DEV7_DRAW_PRIMITIVE_VB,
            &[
                Arg::I(D3DPT_TRIANGLELIST),
                Arg::Val(DEST),
                Arg::I(0),
                Arg::I(3),
                Arg::I(0),
            ],
        )?;
        c.com(a, l, DEVICE, DEV7_END_SCENE, &[])?;
        c.com(
            a,
            l,
            DEST,
            VB_LOCK,
            &[Arg::I(0), Arg::At(LOCKED), Arg::I(0)],
        )?;
        finish(a, l)
    })
}

#[test]
fn process_vertices_transforms_and_lights_into_screen_vertices() {
    let (exe, data) = process_program();
    let mut cpu = machine("process", exe);
    run_until_done(&mut cpu, data);
    let r = results(&cpu, data, 20);
    assert!(r[1..].iter().all(|&hr| hr == 0), "results: {r:X?}");
    // The screen vertices: x, y, z, rhw and the diffuse color, 20 bytes
    // each.
    let at = peek(&cpu, data + LOCKED);
    let vertex = |i: u32| {
        let v = at + 20 * i;
        (
            [0, 4, 8, 12].map(|o| peek_f32(&cpu, v + o)),
            peek(&cpu, v + 16),
        )
    };
    let expected = [
        [32.0, 32.0, 0.5, 1.0],
        [64.0, 0.0, 0.0, 1.0],
        [16.0, 48.0, 1.0, 1.0],
    ];
    for (i, e) in expected.iter().enumerate() {
        let (position, color) = vertex(i as u32);
        assert_eq!(position, *e, "vertex {i}");
        // The material's diffuse color, all lit: 1, 0.5, 0.25.
        let [b, g, r, alpha] = color.to_le_bytes();
        assert_eq!((alpha, r), (0xFF, 0xFF), "vertex {i}: {color:08X}");
        assert!((0x7F..=0x80).contains(&g), "vertex {i}: {color:08X}");
        assert!((0x3F..=0x40).contains(&b), "vertex {i}: {color:08X}");
    }
}

// D3DOP_ instructions and D3DPROCESSVERTICES_COPY.
const D3DOP_TRIANGLE: u8 = 3;
const D3DOP_PROCESSVERTICES: u8 = 9;
const D3DOP_EXIT: u8 = 11;
const D3DPROCESSVERTICES_COPY: u32 = 2;

/// Direct3D 5: a device on a 64x64 surface with a viewport, an execute
/// buffer that copies three screen vertices and draws a triangle of
/// them, and DrawPrimitive with the same vertices; then GetStats.
fn execute_program() -> (Vec<u8>, u32) {
    let mut d = data();
    target_desc(&mut d, DESC, 108, 64, 64);
    // D3DVIEWPORT2: all of the surface.
    put32(&mut d, VIEW, 44);
    put32(&mut d, VIEW + 12, 64);
    put32(&mut d, VIEW + 16, 64);
    put_f32s(&mut d, VIEW + 20, &[-1.0, 1.0, 2.0, 2.0, 0.0, 1.0]);
    // Three D3DTLVERTEX, then the instructions.
    let tl = [(8.0, 8.0), (56.0, 8.0), (32.0, 56.0)];
    for (i, (x, y)) in tl.iter().enumerate() {
        let v = VERTICES + 32 * i as u32;
        put_f32s(&mut d, v, &[*x, *y, 0.5, 1.0]);
        put32(&mut d, v + 16, 0xFFFF_FFFF);
    }
    let code = VERTICES + 96;
    put(&mut d, code, &[D3DOP_PROCESSVERTICES, 16, 1, 0]);
    put32(&mut d, code + 4, D3DPROCESSVERTICES_COPY);
    put32(&mut d, code + 12, 3);
    put(&mut d, code + 20, &[D3DOP_TRIANGLE, 8, 1, 0]);
    put(&mut d, code + 24, &[0, 0, 1, 0, 2, 0, 0, 7]);
    put(&mut d, code + 32, &[D3DOP_EXIT, 0, 0, 0]);
    // D3DEXECUTEBUFFERDESC: 256 bytes; D3DEXECUTEDATA: the vertices and
    // the instructions.
    put32(&mut d, EB_DESC, 20);
    put32(&mut d, EB_DESC + 4, 1);
    put32(&mut d, EB_DESC + 12, 256);
    put32(&mut d, EXECUTE_DATA, 48);
    put32(&mut d, EXECUTE_DATA + 8, 3);
    put32(&mut d, EXECUTE_DATA + 12, 96);
    put32(&mut d, EXECUTE_DATA + 16, 36);
    put32(&mut d, STATS, 24);
    program(&[USER32, DDRAW], &d, |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c, false)?;
        c.com(
            a,
            l,
            DD,
            QUERY_INTERFACE,
            &[Arg::At(IID_D3D2), Arg::At(D3D)],
        )?;
        c.com(
            a,
            l,
            DD,
            DD_CREATE_SURFACE,
            &[Arg::At(DESC), Arg::At(TARGET), Arg::I(0)],
        )?;
        c.com(
            a,
            l,
            D3D,
            D3D2_CREATE_DEVICE,
            &[Arg::At(IID_HAL), Arg::Val(TARGET), Arg::At(DEVICE)],
        )?;
        c.com(
            a,
            l,
            D3D,
            D3D2_CREATE_VIEWPORT,
            &[Arg::At(VIEWPORT), Arg::I(0)],
        )?;
        c.com(a, l, DEVICE, DEV2_ADD_VIEWPORT, &[Arg::Val(VIEWPORT)])?;
        c.com(a, l, VIEWPORT, VP2_SET_VIEWPORT2, &[Arg::At(VIEW)])?;
        c.com(
            a,
            l,
            DEVICE,
            DEV2_SET_CURRENT_VIEWPORT,
            &[Arg::Val(VIEWPORT)],
        )?;
        c.com(
            a,
            l,
            DEVICE,
            QUERY_INTERFACE,
            &[Arg::At(IID_DEVICE1), Arg::At(DEVICE1)],
        )?;
        c.com(
            a,
            l,
            DEVICE1,
            DEV1_CREATE_EXECUTE_BUFFER,
            &[Arg::At(EB_DESC), Arg::At(BUFFER), Arg::I(0)],
        )?;
        c.com(a, l, BUFFER, EB_LOCK, &[Arg::At(EB_DESC)])?;
        copy(a, l, VERTICES, EB_DESC + 16, 132)?;
        c.com(a, l, BUFFER, EB_UNLOCK, &[])?;
        c.com(a, l, BUFFER, EB_SET_EXECUTE_DATA, &[Arg::At(EXECUTE_DATA)])?;
        c.com(a, l, DEVICE, DEV2_BEGIN_SCENE, &[])?;
        c.com(
            a,
            l,
            DEVICE1,
            DEV1_EXECUTE,
            &[Arg::Val(BUFFER), Arg::Val(VIEWPORT), Arg::I(0)],
        )?;
        c.com(
            a,
            l,
            DEVICE,
            DEV2_DRAW_PRIMITIVE,
            &[
                Arg::I(D3DPT_TRIANGLELIST),
                Arg::I(D3DVT_TLVERTEX),
                Arg::At(VERTICES),
                Arg::I(3),
                Arg::I(0),
            ],
        )?;
        c.com(a, l, DEVICE, DEV2_END_SCENE, &[])?;
        c.com(a, l, DEVICE, DEV2_GET_STATS, &[Arg::At(STATS)])?;
        finish(a, l)
    })
}

#[test]
fn an_execute_buffer_and_draw_primitive_count_in_get_stats() {
    let (exe, data) = execute_program();
    let mut cpu = machine("execute", exe);
    run_until_done(&mut cpu, data);
    let r = results(&cpu, data, 21);
    assert!(r[1..].iter().all(|&hr| hr == 0), "results: {r:X?}");
    // D3DSTATS: triangles, lines, points, spans and vertices processed.
    let stats = [4, 8, 12, 20].map(|o| peek(&cpu, data + STATS + o));
    assert_eq!(stats, [2, 0, 0, 6]);
}
