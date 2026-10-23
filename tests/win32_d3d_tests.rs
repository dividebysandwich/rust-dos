//! The built-in Windows' Direct3D (in DDRAW.DLL): Win32 programs that
//! make a device on the Voodoo3's HAL in a 16-bit mode, or on the 3dfx
//! device of a Voodoo Graphics or Voodoo2 add-on card, put vertices
//! through its transform and lighting, and draw with vertex buffers,
//! DrawPrimitive and execute buffers: flat, gouraud shaded, textured,
//! depth tested, blended and fogged, the pixels read back with Lock.
//! The programs are PE files made with rust-dos-pe's builder, their
//! code assembled with iced, as in win32_ddraw_tests.rs. Each leaves its
//! results in its data and spins, and the test looks at them while the
//! program is still there.

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
    machine_with(test, None, program)
}

/// A machine with a Voodoo3 as its display adapter or, given a `card`
/// other than the Voodoo3, its usual adapter and that add-on card.
fn machine_with(test: &str, card: Option<Board>, program: Vec<u8>) -> Cpu {
    let base = PathBuf::from("target/test_win32_d3d").join(test);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join("T.EXE"), program).unwrap();
    let mut cpu = Cpu::new(base);
    if card.is_none_or(|c| c == Board::Voodoo3) {
        let setup = VideoSetup {
            adapter: Adapter::Voodoo3,
            mono_monitor: false,
        };
        rust_dos::video::bios::install(&mut cpu.bus, setup);
    }
    cpu.bus
        .configure_voodoo(Some(card.unwrap_or(Board::Voodoo3)));
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
/// The HRESULTs of the calls, in order, and how many calls a scene
/// made.
const RESULTS: u32 = 0x1000;
const DONE: u32 = 0xF0;
const CALLS: u32 = 0xF4;
const CLASS: u32 = 0x100;
const IID_DD7: u32 = 0x110;
const IID_D3D7: u32 = 0x120;
const IID_HAL: u32 = 0x130;
const IID_D3D2: u32 = 0x140;
const IID_DEVICE1: u32 = 0x150;
const IID_D3D3: u32 = 0x160;
/// The 3dfx device's GUID, and the DDSCAPS2 of a back buffer.
const CARD_GUID: u32 = 0x170;
const BACK_CAPS: u32 = 0x180;
const DESC: u32 = 0x200;
const VB_DESC: u32 = 0x280;
const VIEW: u32 = 0x2A0;
const LIGHT: u32 = 0x300;
const MATERIAL: u32 = 0x380;
const STATS: u32 = 0x3E0;
const VERTICES: u32 = 0x400;
const EB_DESC: u32 = 0x500;
const EXECUTE_DATA: u32 = 0x520;
/// The 64x64 pixels a scene's target has at its top left, copied there
/// by the program.
const PIXELS: u32 = 0x2000;
const DATA_SIZE: usize = 0x6000;

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
    guid(
        &mut data,
        IID_D3D3,
        0xBB22_3240,
        0xE72B,
        0x11D0,
        [0xA9, 0xB4, 0x00, 0xAA, 0x00, 0xC0, 0x99, 0x3E],
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
/// IDirectDraw7 when `ex`; the 3dfx device's, with the GUID at
/// CARD_GUID, when `card`), and a 640x480 16-bit mode in full screen.
fn start(
    a: &mut CodeAssembler,
    l: &Layout,
    c: &mut Calls,
    ex: bool,
    card: bool,
) -> Result<(), IcedError> {
    start_in(a, l, c, ex, card, 16)
}

/// The same, with a mode of `bpp` bits a pixel.
fn start_in(
    a: &mut CodeAssembler,
    l: &Layout,
    c: &mut Calls,
    ex: bool,
    card: bool,
    bpp: u32,
) -> Result<(), IcedError> {
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
            &[
                Arg::I(if card { l.at(CARD_GUID) as u32 } else { 0 }),
                Arg::At(DD),
                Arg::At(IID_DD7),
                Arg::I(0),
            ],
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
    let mode = [Arg::I(640), Arg::I(480), Arg::I(bpp), Arg::I(0), Arg::I(0)];
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
        start(a, l, &mut c, true, false)?;
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
    put32(&mut d, LOCK_DESC, 108);
    program(&[USER32, DDRAW], &d, |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c, false, false)?;
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
        c.com(
            a,
            l,
            TARGET,
            SURF_LOCK,
            &[
                Arg::I(0),
                Arg::At(LOCK_DESC),
                Arg::I(DDLOCK_WAIT),
                Arg::I(0),
            ],
        )?;
        finish(a, l)
    })
}

#[test]
fn an_execute_buffer_and_draw_primitive_draw_and_count_in_get_stats() {
    let (exe, data) = execute_program();
    let mut cpu = machine("execute", exe);
    run_until_done(&mut cpu, data);
    let r = results(&cpu, data, 22);
    assert!(r[1..].iter().all(|&hr| hr == 0), "results: {r:X?}");
    // D3DSTATS: triangles, lines, points, spans and vertices processed.
    let stats = [4, 8, 12, 20].map(|o| peek(&cpu, data + STATS + o));
    assert_eq!(stats, [2, 0, 0, 6]);
    // The white triangle, on the target's black.
    let bits = peek(&cpu, data + LOCK_DESC + 0x24);
    let pitch = peek(&cpu, data + LOCK_DESC + 0x10);
    let pixel = |x: u32, y: u32| {
        let at = bits + y * pitch + 2 * x;
        (peek(&cpu, at & !3) >> (8 * (at & 2))) as u16
    };
    assert_eq!(pixel(32, 20), 0xFFFF);
    assert_eq!(pixel(32, 60), 0);
}

// Drawing: scenes of screen vertices drawn on a 64x64 target, which the
// program then locks for the test to read its pixels.

/// The DDSURFACEDESC2 of the render target's Lock, of a z-buffer, of a
/// texture and of the texture's Lock; the z-buffer's and the texture's
/// interfaces; the texture's texels; a D3DRECT; the DDSURFACEDESC2 of
/// the textures that fill video memory and their interface; the scene's
/// vertices.
const LOCK_DESC: u32 = 0x600;
const Z_DESC: u32 = 0x680;
const TEX_DESC: u32 = 0x700;
const ZBUF: u32 = 0x780;
const TEXTURE: u32 = 0x784;
const TEX_LOCK_DESC: u32 = 0x800;
const TEXELS: u32 = 0x880;
const RECT: u32 = 0x900;
const FILL_DESC: u32 = 0x910;
const FILLED: u32 = 0x990;
const FRONT: u32 = 0x994;
const SCENE: u32 = 0xA00;

const DDSD_PIXELFORMAT: u32 = 0x1000;
const DDSCAPS_TEXTURE: u32 = 0x1000;
const DDSCAPS_ZBUFFER: u32 = 0x2_0000;
const DDSCAPS_VIDEOMEMORY: u32 = 0x4000;
const DDSCAPS_BACKBUFFER: u32 = 0x4;
const DDSCAPS_COMPLEX: u32 = 0x8;
const DDSCAPS_FLIP: u32 = 0x10;
const DDSCAPS_PRIMARYSURFACE: u32 = 0x200;
const DDSD_BACKBUFFERCOUNT: u32 = 0x20;
const DDPF_RGB: u32 = 0x40;
const DDPF_ZBUFFER: u32 = 0x400;
const DDLOCK_WAIT: u32 = 1;
const DDFLIP_WAIT: u32 = 1;
const DDFLIP_NOVSYNC: u32 = 8;
const D3DCLEAR_TARGET: u32 = 1;
const D3DCLEAR_ZBUFFER: u32 = 2;
/// D3DFVF_XYZRHW, DIFFUSE, SPECULAR and TEX1: a D3DTLVERTEX.
const TL_FVF: u32 = 0x1C4;

const SURF_ADD_ATTACHED_SURFACE: u32 = 3;
const SURF_GET_ATTACHED_SURFACE: u32 = 12;
const SURF_FLIP: u32 = 11;
const SURF_LOCK: u32 = 25;
const SURF_UNLOCK: u32 = 32;
const D3D3_CREATE_VIEWPORT: u32 = 6;
const D3D3_CREATE_DEVICE: u32 = 8;
const DEV3_ADD_VIEWPORT: u32 = 5;
const DEV3_BEGIN_SCENE: u32 = 9;
const DEV3_END_SCENE: u32 = 10;
const DEV3_SET_CURRENT_VIEWPORT: u32 = 12;
const DEV3_SET_RENDER_STATE: u32 = 22;
const DEV3_DRAW_PRIMITIVE: u32 = 28;
const VP3_CLEAR2: u32 = 20;
const DEV7_CLEAR: u32 = 10;
const DEV7_SET_RENDER_STATE: u32 = 20;
const DEV7_DRAW_PRIMITIVE: u32 = 25;
const DEV7_SET_TEXTURE: u32 = 35;

// Render states.
const RS_ZENABLE: u32 = 7;
const RS_ZFUNC: u32 = 23;
const RS_SHADEMODE: u32 = 9;
const RS_SRCBLEND: u32 = 19;
const RS_DESTBLEND: u32 = 20;
const RS_ALPHABLENDENABLE: u32 = 27;
const RS_FOGENABLE: u32 = 28;
const RS_FOGCOLOR: u32 = 34;
const RS_FOGTABLEMODE: u32 = 35;
const RS_FOGSTART: u32 = 36;
const RS_FOGEND: u32 = 37;
const RS_SPECULARENABLE: u32 = 29;

/// A D3DTLVERTEX: its position, rhw, diffuse and specular colors and
/// texture coordinates.
#[derive(Clone, Copy)]
struct Tl {
    x: f32,
    y: f32,
    z: f32,
    rhw: f32,
    diffuse: u32,
    specular: u32,
    u: f32,
    v: f32,
}

fn tl(x: f32, y: f32, z: f32, diffuse: u32) -> Tl {
    Tl {
        x,
        y,
        z,
        rhw: 1.0,
        diffuse,
        specular: 0xFF00_0000,
        u: 0.0,
        v: 0.0,
    }
}

/// Two triangles covering the rectangle from (x0, y0) to (x1, y1), its
/// texture coordinates 0 to 1, all with depth `z` and `color`.
fn quad(x0: f32, y0: f32, x1: f32, y1: f32, z: f32, color: u32) -> Vec<Tl> {
    let at = |x: f32, y: f32, u: f32, v: f32| Tl {
        u,
        v,
        ..tl(x, y, z, color)
    };
    let (a, b) = (at(x0, y0, 0.0, 0.0), at(x1, y0, 1.0, 0.0));
    let (c, d) = (at(x0, y1, 0.0, 1.0), at(x1, y1, 1.0, 1.0));
    vec![a, b, c, b, d, c]
}

/// What a scene does after its Clear.
enum Step {
    /// SetRenderState.
    State(u32, u32),
    /// DrawPrimitive of a triangle list.
    Draw(Vec<Tl>),
    /// 256x256 textures made in video memory until there is no more
    /// room (Direct3D 7), as Unreal does.
    Fill,
    /// Flip of the flip chain, without waiting for the frame.
    Flip,
}

/// Which Direct3D draws a scene.
#[derive(Clone, Copy, PartialEq)]
enum Api {
    /// Direct3D 6: IDirect3DDevice3, cleared through its viewport.
    D3D6,
    /// Direct3D 7: IDirect3DDevice7.
    D3D7,
}

struct Scene {
    api: Api,
    /// The add-on card whose 3dfx device draws (Direct3D 7), into the
    /// back buffer of a 640x480 flip chain; the Voodoo3's HAL if None,
    /// into a 64x64 surface.
    card: Option<Board>,
    /// The color the target is cleared with, ARGB.
    clear: u32,
    /// The depth the target's 16-bit z-buffer is cleared to, if it has
    /// one.
    zbuffer: Option<f32>,
    /// An 8x8 565 texture on stage 0 (Direct3D 7).
    texture: Option<[u16; 64]>,
    steps: Vec<Step>,
}

/// A program drawing `scene`, which ends with the target locked.
fn scene_program(scene: &Scene) -> (Vec<u8>, u32) {
    scene_program_in(scene, 16)
}

/// The same in a mode of `bpp` bits a pixel (16 or 32), whose z-buffer
/// has as many bits (24 of them depth at 32).
fn scene_program_in(scene: &Scene, bpp: u32) -> (Vec<u8>, u32) {
    let mut d = data();
    let size = if scene.api == Api::D3D7 { 124 } else { 108 };
    let (zw, zh) = if let Some(board) = scene.card {
        let d1 = if board == Board::Voodoo2 {
            (0x472B_EA00, 0x40DF, 0x11D1)
        } else {
            (0x3A0C_FD01, 0x9320, 0x11CF)
        };
        let d4 = if board == Board::Voodoo2 {
            [0xA9, 0xDF, 0x00, 0x60, 0x97, 0xC2, 0xED, 0xB2]
        } else {
            [0xAC, 0xA1, 0x00, 0xA0, 0x24, 0x13, 0xC2, 0xE2]
        };
        guid(&mut d, CARD_GUID, d1.0, d1.1, d1.2, d4);
        put32(&mut d, DESC, size);
        put32(&mut d, DESC + 0x04, DDSD_CAPS | DDSD_BACKBUFFERCOUNT);
        put32(&mut d, DESC + 0x14, 1);
        let caps = DDSCAPS_PRIMARYSURFACE | DDSCAPS_FLIP | DDSCAPS_COMPLEX | DDSCAPS_3DDEVICE;
        put32(&mut d, DESC + 0x68, caps);
        put32(&mut d, BACK_CAPS, DDSCAPS_BACKBUFFER);
        (640, 480)
    } else {
        target_desc(&mut d, DESC, size, 64, 64);
        (64, 64)
    };
    put32(&mut d, LOCK_DESC, size);
    put32(&mut d, TEX_LOCK_DESC, 124);
    // D3DVIEWPORT7: all of the surface, depths 0 to 1; or D3DVIEWPORT2.
    if scene.api == Api::D3D7 {
        put32(&mut d, VIEW + 8, 64);
        put32(&mut d, VIEW + 12, 64);
        put_f32s(&mut d, VIEW + 16, &[0.0, 1.0]);
    } else {
        put32(&mut d, VIEW, 44);
        put32(&mut d, VIEW + 12, 64);
        put32(&mut d, VIEW + 16, 64);
        put_f32s(&mut d, VIEW + 20, &[-1.0, 1.0, 2.0, 2.0, 0.0, 1.0]);
    }
    put32(&mut d, RECT + 8, 64);
    put32(&mut d, RECT + 12, 64);
    // The z-buffer's and the texture's DDSURFACEDESC2.
    let flags = DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT | DDSD_PIXELFORMAT;
    put32(&mut d, Z_DESC, size);
    put32(&mut d, Z_DESC + 0x04, flags);
    put32(&mut d, Z_DESC + 0x08, zh);
    put32(&mut d, Z_DESC + 0x0C, zw);
    put32(&mut d, Z_DESC + 0x48, 32);
    put32(&mut d, Z_DESC + 0x4C, DDPF_ZBUFFER);
    put32(&mut d, Z_DESC + 0x54, bpp);
    put32(
        &mut d,
        Z_DESC + 0x5C,
        if bpp == 32 { 0xFF_FFFF } else { 0xFFFF },
    );
    put32(&mut d, Z_DESC + 0x68, DDSCAPS_ZBUFFER | DDSCAPS_VIDEOMEMORY);
    put32(&mut d, TEX_DESC, 124);
    put32(&mut d, TEX_DESC + 0x04, flags);
    put32(&mut d, TEX_DESC + 0x08, 8);
    put32(&mut d, TEX_DESC + 0x0C, 8);
    put32(&mut d, TEX_DESC + 0x48, 32);
    put32(&mut d, TEX_DESC + 0x4C, DDPF_RGB);
    put32(&mut d, TEX_DESC + 0x54, 16);
    put32(&mut d, TEX_DESC + 0x58, 0xF800);
    put32(&mut d, TEX_DESC + 0x5C, 0x07E0);
    put32(&mut d, TEX_DESC + 0x60, 0x001F);
    put32(&mut d, TEX_DESC + 0x68, DDSCAPS_TEXTURE);
    let tex_desc = d[TEX_DESC as usize..TEX_DESC as usize + 124].to_vec();
    put(&mut d, FILL_DESC, &tex_desc);
    put32(&mut d, FILL_DESC + 0x08, 256);
    put32(&mut d, FILL_DESC + 0x0C, 256);
    let fill_caps = DDSCAPS_TEXTURE | DDSCAPS_VIDEOMEMORY;
    put32(&mut d, FILL_DESC + 0x68, fill_caps);
    if let Some(texels) = &scene.texture {
        for (i, t) in texels.iter().enumerate() {
            put(&mut d, TEXELS + 2 * i as u32, &t.to_le_bytes());
        }
    }
    // The vertices of the draws, one after another.
    let mut draws = Vec::new();
    let mut at = SCENE;
    for step in &scene.steps {
        if let Step::Draw(verts) = step {
            draws.push(at);
            for v in verts {
                put_f32s(&mut d, at, &[v.x, v.y, v.z, v.rhw]);
                put32(&mut d, at + 16, v.diffuse);
                put32(&mut d, at + 20, v.specular);
                put_f32s(&mut d, at + 24, &[v.u, v.v]);
                at += 32;
            }
        }
    }
    assert!(at <= RESULTS, "too many vertices");
    let ex = scene.api == Api::D3D7;
    program(&[USER32, DDRAW], &d, |a, l| {
        let mut c = Calls { next: 0 };
        let add_on = scene.card.is_some_and(|b| b != Board::Voodoo3);
        start_in(a, l, &mut c, ex, add_on, bpp)?;
        let iid = if ex { IID_D3D7 } else { IID_D3D3 };
        c.com(a, l, DD, QUERY_INTERFACE, &[Arg::At(iid), Arg::At(D3D)])?;
        c.com(
            a,
            l,
            DD,
            DD_CREATE_SURFACE,
            &[Arg::At(DESC), Arg::At(TARGET), Arg::I(0)],
        )?;
        if scene.card.is_some() {
            // The flip chain's back buffer is the target.
            a.mov(eax, dword_ptr(l.at(TARGET)))?;
            a.mov(dword_ptr(l.at(FRONT)), eax)?;
            c.com(
                a,
                l,
                TARGET,
                SURF_GET_ATTACHED_SURFACE,
                &[Arg::At(BACK_CAPS), Arg::At(TARGET)],
            )?;
        }
        if scene.zbuffer.is_some() {
            c.com(
                a,
                l,
                DD,
                DD_CREATE_SURFACE,
                &[Arg::At(Z_DESC), Arg::At(ZBUF), Arg::I(0)],
            )?;
            c.com(a, l, TARGET, SURF_ADD_ATTACHED_SURFACE, &[Arg::Val(ZBUF)])?;
        }
        let (set_state, draw) = if ex {
            c.com(
                a,
                l,
                D3D,
                D3D7_CREATE_DEVICE,
                &[Arg::At(IID_HAL), Arg::Val(TARGET), Arg::At(DEVICE)],
            )?;
            c.com(a, l, DEVICE, DEV7_SET_VIEWPORT, &[Arg::At(VIEW)])?;
            if scene.texture.is_some() {
                c.com(
                    a,
                    l,
                    DD,
                    DD_CREATE_SURFACE,
                    &[Arg::At(TEX_DESC), Arg::At(TEXTURE), Arg::I(0)],
                )?;
                c.com(
                    a,
                    l,
                    TEXTURE,
                    SURF_LOCK,
                    &[
                        Arg::I(0),
                        Arg::At(TEX_LOCK_DESC),
                        Arg::I(DDLOCK_WAIT),
                        Arg::I(0),
                    ],
                )?;
                copy(a, l, TEXELS, TEX_LOCK_DESC + 0x24, 128)?;
                c.com(a, l, TEXTURE, SURF_UNLOCK, &[Arg::I(0)])?;
                c.com(
                    a,
                    l,
                    DEVICE,
                    DEV7_SET_TEXTURE,
                    &[Arg::I(0), Arg::Val(TEXTURE)],
                )?;
            }
            let mut what = D3DCLEAR_TARGET;
            if scene.zbuffer.is_some() {
                what |= D3DCLEAR_ZBUFFER;
            }
            c.com(
                a,
                l,
                DEVICE,
                DEV7_CLEAR,
                &[
                    Arg::I(0),
                    Arg::I(0),
                    Arg::I(what),
                    Arg::I(scene.clear),
                    Arg::I(scene.zbuffer.unwrap_or(1.0).to_bits()),
                    Arg::I(0),
                ],
            )?;
            c.com(a, l, DEVICE, DEV7_BEGIN_SCENE, &[])?;
            (DEV7_SET_RENDER_STATE, DEV7_DRAW_PRIMITIVE)
        } else {
            c.com(
                a,
                l,
                D3D,
                D3D3_CREATE_DEVICE,
                &[
                    Arg::At(IID_HAL),
                    Arg::Val(TARGET),
                    Arg::At(DEVICE),
                    Arg::I(0),
                ],
            )?;
            c.com(
                a,
                l,
                D3D,
                D3D3_CREATE_VIEWPORT,
                &[Arg::At(VIEWPORT), Arg::I(0)],
            )?;
            c.com(a, l, DEVICE, DEV3_ADD_VIEWPORT, &[Arg::Val(VIEWPORT)])?;
            c.com(a, l, VIEWPORT, VP2_SET_VIEWPORT2, &[Arg::At(VIEW)])?;
            c.com(
                a,
                l,
                DEVICE,
                DEV3_SET_CURRENT_VIEWPORT,
                &[Arg::Val(VIEWPORT)],
            )?;
            let mut what = D3DCLEAR_TARGET;
            if scene.zbuffer.is_some() {
                what |= D3DCLEAR_ZBUFFER;
            }
            c.com(
                a,
                l,
                VIEWPORT,
                VP3_CLEAR2,
                &[
                    Arg::I(1),
                    Arg::At(RECT),
                    Arg::I(what),
                    Arg::I(scene.clear),
                    Arg::I(scene.zbuffer.unwrap_or(1.0).to_bits()),
                    Arg::I(0),
                ],
            )?;
            c.com(a, l, DEVICE, DEV3_BEGIN_SCENE, &[])?;
            (DEV3_SET_RENDER_STATE, DEV3_DRAW_PRIMITIVE)
        };
        let mut draws = draws.iter();
        for step in &scene.steps {
            match step {
                Step::State(s, v) => {
                    c.com(a, l, DEVICE, set_state, &[Arg::I(*s), Arg::I(*v)])?;
                }
                Step::Flip => {
                    let flags = DDFLIP_WAIT | DDFLIP_NOVSYNC;
                    c.com(a, l, FRONT, SURF_FLIP, &[Arg::I(0), Arg::I(flags)])?;
                }
                Step::Fill => {
                    let mut again = a.create_label();
                    a.set_label(&mut again)?;
                    push(a, l, &[Arg::At(FILL_DESC), Arg::At(FILLED), Arg::I(0)])?;
                    a.mov(eax, dword_ptr(l.at(DD)))?;
                    a.push(eax)?;
                    a.mov(eax, dword_ptr(eax))?;
                    a.call(dword_ptr(eax + 4 * DD_CREATE_SURFACE))?;
                    a.test(eax, eax)?;
                    a.jz(again)?;
                }
                Step::Draw(verts) => {
                    let at = *draws.next().unwrap();
                    c.com(
                        a,
                        l,
                        DEVICE,
                        draw,
                        &[
                            Arg::I(D3DPT_TRIANGLELIST),
                            Arg::I(TL_FVF),
                            Arg::At(at),
                            Arg::I(verts.len() as u32),
                            Arg::I(0),
                        ],
                    )?;
                }
            }
        }
        let end = if ex { DEV7_END_SCENE } else { DEV3_END_SCENE };
        c.com(a, l, DEVICE, end, &[])?;
        c.com(
            a,
            l,
            TARGET,
            SURF_LOCK,
            &[
                Arg::I(0),
                Arg::At(LOCK_DESC),
                Arg::I(DDLOCK_WAIT),
                Arg::I(0),
            ],
        )?;
        a.mov(dword_ptr(l.at(CALLS)), c.next)?;
        // The top left 64x64 pixels into PIXELS, a row at a time.
        let row_bytes = 64 * bpp / 8;
        a.mov(esi, dword_ptr(l.at(LOCK_DESC + 0x24)))?;
        a.mov(edi, l.at(PIXELS) as u32)?;
        a.mov(edx, 64)?;
        a.cld()?;
        let mut row = a.create_label();
        a.set_label(&mut row)?;
        a.mov(ecx, row_bytes / 4)?;
        a.rep().movsd()?;
        a.add(esi, dword_ptr(l.at(LOCK_DESC + 0x10)))?;
        a.sub(esi, row_bytes)?;
        a.dec(edx)?;
        a.jnz(row)?;
        finish(a, l)
    })
}

/// The render target's pixels after `scene`, row after row, 5-6-5,
/// once every call in it succeeded.
fn draw_scene(test: &str, scene: &Scene) -> Vec<u16> {
    let (exe, data) = scene_program(scene);
    let mut cpu = machine_with(test, scene.card, exe);
    run_until_done(&mut cpu, data);
    let n = peek(&cpu, data + CALLS);
    let r = results(&cpu, data, n);
    assert!(r[1..].iter().all(|&hr| hr == 0), "results: {r:X?}");
    (0..64 * 64)
        .map(|i| {
            let at = data + PIXELS + 2 * i;
            (peek(&cpu, at & !3) >> (8 * (at & 2))) as u16
        })
        .collect()
}

/// The render target's pixels after `scene` in a 32-bit mode, row
/// after row, X8R8G8B8.
fn draw_scene32(test: &str, scene: &Scene) -> Vec<u32> {
    let (exe, data) = scene_program_in(scene, 32);
    let mut cpu = machine_with(test, scene.card, exe);
    run_until_done(&mut cpu, data);
    let n = peek(&cpu, data + CALLS);
    let r = results(&cpu, data, n);
    assert!(r[1..].iter().all(|&hr| hr == 0), "results: {r:X?}");
    (0..64 * 64)
        .map(|i| peek(&cpu, data + PIXELS + 4 * i) & 0xFF_FFFF)
        .collect()
}

/// The 5-6-5 pixel at (x, y) as 8-bit red, green and blue.
fn rgb(pixels: &[u16], x: usize, y: usize) -> [u32; 3] {
    let p = pixels[y * 64 + x] as u32;
    let (r, g, b) = (p >> 11, (p >> 5) & 0x3F, p & 0x1F);
    [r << 3 | r >> 2, g << 2 | g >> 4, b << 3 | b >> 2]
}

/// Whether each channel of `got` is within `tolerance` of `want`.
fn near(got: [u32; 3], want: [u32; 3], tolerance: u32) -> bool {
    got.iter()
        .zip(want)
        .all(|(&g, w)| g.abs_diff(w) <= tolerance)
}

#[track_caller]
fn assert_rgb(pixels: &[u16], x: usize, y: usize, want: [u32; 3], tolerance: u32) {
    let got = rgb(pixels, x, y);
    assert!(
        near(got, want, tolerance),
        "pixel ({x}, {y}): {got:?}, expected {want:?}"
    );
}

const RED: u32 = 0xFFFF_0000;
const GREEN: u32 = 0xFF00_FF00;
const BLUE: u32 = 0xFF00_00FF;
const WHITE: u32 = 0xFFFF_FFFF;

/// A triangle with a red, a green and a blue corner, over most of the
/// target's top left half.
fn corners() -> Vec<Tl> {
    vec![
        tl(4.0, 4.0, 0.5, RED),
        tl(60.0, 4.0, 0.5, GREEN),
        tl(4.0, 60.0, 0.5, BLUE),
    ]
}

#[test]
fn a_flat_shaded_triangle_has_its_first_vertex_color() {
    let scene = Scene {
        api: Api::D3D7,
        card: None,
        clear: 0xFF00_0080,
        zbuffer: None,
        texture: None,
        steps: vec![Step::State(RS_SHADEMODE, 1), Step::Draw(corners())],
    };
    let p = draw_scene("flat", &scene);
    for (x, y) in [(8, 8), (40, 10), (10, 40), (20, 20)] {
        assert_rgb(&p, x, y, [255, 0, 0], 0);
    }
    // Outside the triangle, the clear color.
    assert_rgb(&p, 60, 60, [0, 0, 0x84], 4);
    assert_rgb(&p, 0, 0, [0, 0, 0x84], 4);
}

#[test]
fn a_gouraud_shaded_triangle_blends_its_corners() {
    let scene = Scene {
        api: Api::D3D7,
        card: None,
        clear: 0,
        zbuffer: None,
        texture: None,
        steps: vec![Step::Draw(corners())],
    };
    let p = draw_scene("gouraud", &scene);
    // Near each corner, mostly its color; between them, all three.
    assert_rgb(&p, 5, 5, [255, 0, 0], 16);
    assert_rgb(&p, 57, 5, [0, 255, 0], 24);
    assert_rgb(&p, 5, 57, [0, 0, 255], 24);
    assert_rgb(&p, 22, 22, [85, 85, 85], 24);
    assert_rgb(&p, 60, 60, [0, 0, 0], 0);
}

#[test]
fn direct3d_6_clears_through_its_viewport_and_draws() {
    let scene = Scene {
        api: Api::D3D6,
        card: None,
        clear: 0xFF00_0080,
        zbuffer: None,
        texture: None,
        steps: vec![Step::Draw(corners())],
    };
    let p = draw_scene("d3d6", &scene);
    assert_rgb(&p, 5, 5, [255, 0, 0], 16);
    assert_rgb(&p, 22, 22, [85, 85, 85], 24);
    assert_rgb(&p, 60, 60, [0, 0, 0x84], 4);
}

#[test]
fn a_textured_quad_shows_its_texels() {
    // Red, green, blue and white quarters.
    let texels = std::array::from_fn(|i| match (i % 8 >= 4, i / 8 >= 4) {
        (false, false) => 0xF800,
        (true, false) => 0x07E0,
        (false, true) => 0x001F,
        (true, true) => 0xFFFF,
    });
    let scene = Scene {
        api: Api::D3D7,
        card: None,
        clear: 0,
        zbuffer: None,
        texture: Some(texels),
        steps: vec![Step::Draw(quad(0.0, 0.0, 64.0, 64.0, 0.5, WHITE))],
    };
    let p = draw_scene("textured", &scene);
    assert_rgb(&p, 16, 16, [255, 0, 0], 8);
    assert_rgb(&p, 48, 16, [0, 255, 0], 8);
    assert_rgb(&p, 16, 48, [0, 0, 255], 8);
    assert_rgb(&p, 48, 48, [255, 255, 255], 8);
    // The last column and row are the quad's too.
    assert_rgb(&p, 63, 0, [0, 255, 0], 8);
    assert_rgb(&p, 63, 63, [255, 255, 255], 8);
}

#[test]
fn a_texture_draws_after_its_program_fills_video_memory() {
    // Unreal makes textures in video memory until there is none left,
    // after it makes its device and before it draws: the card's own
    // copies of textures still have room.
    let scene = Scene {
        api: Api::D3D7,
        card: None,
        clear: 0,
        zbuffer: None,
        texture: Some([0xF800; 64]),
        steps: vec![
            Step::Fill,
            Step::Draw(quad(0.0, 0.0, 64.0, 64.0, 0.5, WHITE)),
        ],
    };
    let p = draw_scene("fill_video_memory", &scene);
    assert_rgb(&p, 32, 32, [255, 0, 0], 8);
}

#[test]
fn the_z_buffer_keeps_the_nearer_of_two_quads() {
    let scene = Scene {
        api: Api::D3D7,
        card: None,
        clear: 0,
        zbuffer: Some(1.0),
        texture: None,
        steps: vec![
            Step::State(RS_ZENABLE, 1),
            Step::Draw(quad(0.0, 0.0, 32.0, 64.0, 0.25, GREEN)),
            Step::Draw(quad(0.0, 0.0, 64.0, 64.0, 0.75, RED)),
        ],
    };
    let p = draw_scene("ztest", &scene);
    assert_rgb(&p, 10, 30, [0, 255, 0], 0);
    assert_rgb(&p, 50, 30, [255, 0, 0], 0);
}

#[test]
fn a_quad_nearer_than_the_cleared_depth_passes_the_depth_test() {
    // Cleared to 0.5 through the viewport, as Expendable does, then
    // with LESSEQUAL a quad at 0.25 is drawn and one at 0.75 isn't: by
    // Z, and by W while the vertices' fog takes Z.
    for (test, fog) in [("zclear", 0), ("zclear_fog", 1)] {
        let at = |x0: f32, x1: f32, z: f32, rhw: f32, color: u32| {
            quad(x0, 0.0, x1, 64.0, z, color)
                .into_iter()
                .map(|v| Tl { rhw, ..v })
                .collect()
        };
        let scene = Scene {
            api: Api::D3D6,
            card: None,
            clear: RED,
            zbuffer: Some(0.5),
            texture: None,
            steps: vec![
                Step::State(RS_ZENABLE, 1),
                Step::State(RS_ZFUNC, 4),
                Step::State(RS_FOGENABLE, fog),
                Step::Draw(at(0.0, 32.0, 0.25, 4.0, GREEN)),
                Step::Draw(at(32.0, 64.0, 0.75, 1.0 / 3.0, BLUE)),
            ],
        };
        let p = draw_scene(test, &scene);
        assert_rgb(&p, 16, 30, [0, 255, 0], 0);
        assert_rgb(&p, 48, 30, [255, 0, 0], 0);
    }
}

#[test]
fn a_32_bit_target_keeps_its_clear_color_exactly_and_widens_drawn_pixels() {
    // The card draws at 16 bits into a shadow of the target and its
    // 24-bit z-buffer. Clear writes the exact color, so where nothing
    // passes the depth test the target keeps it; drawn pixels are the
    // card's 5-6-5 with their bits repeated.
    for (test, api) in [("target32_d3d7", Api::D3D7), ("target32_d3d6", Api::D3D6)] {
        let scene = Scene {
            api,
            card: None,
            clear: 0xFF12_3456,
            zbuffer: Some(0.5),
            texture: None,
            steps: vec![
                Step::State(RS_ZENABLE, 1),
                Step::State(RS_ZFUNC, 4),
                Step::Draw(quad(0.0, 0.0, 32.0, 64.0, 0.25, 0xFF80_40C0)),
                Step::Draw(quad(32.0, 0.0, 64.0, 64.0, 0.75, GREEN)),
            ],
        };
        let p = draw_scene32(test, &scene);
        assert_eq!(p[30 * 64 + 48], 0x12_3456, "{test}: cleared pixel");
        let drawn = p[30 * 64 + 16];
        let [b, g, r, _] = drawn.to_le_bytes().map(u32::from);
        assert_eq!(
            (r >> 5, b >> 5, g >> 6),
            (r & 7, b & 7, g & 3),
            "{test}: {drawn:06X}"
        );
        assert!(
            near([r, g, b], [0x80, 0x40, 0xC0], 8),
            "{test}: {drawn:06X}"
        );
    }
}

#[test]
fn the_voodoo3_draws_into_a_back_buffer_at_the_start_of_its_memory() {
    // The front buffer starts at 0; after a Flip the back buffer is
    // there, and the card draws into it.
    let scene = Scene {
        api: Api::D3D7,
        card: Some(Board::Voodoo3),
        clear: RED,
        zbuffer: None,
        texture: None,
        steps: vec![
            Step::Draw(quad(0.0, 0.0, 64.0, 64.0, 0.5, GREEN)),
            Step::Flip,
            Step::Draw(quad(0.0, 0.0, 64.0, 64.0, 0.5, BLUE)),
        ],
    };
    let p = draw_scene("flip_at_0", &scene);
    assert_rgb(&p, 10, 30, [0, 0, 255], 0);
    assert_rgb(&p, 50, 30, [0, 0, 255], 0);
}

#[test]
fn alpha_blending_mixes_with_the_target() {
    let scene = Scene {
        api: Api::D3D7,
        card: None,
        clear: BLUE,
        zbuffer: None,
        texture: None,
        steps: vec![
            Step::State(RS_ALPHABLENDENABLE, 1),
            Step::State(RS_SRCBLEND, 5),
            Step::State(RS_DESTBLEND, 6),
            Step::Draw(quad(0.0, 0.0, 32.0, 64.0, 0.5, 0x80FF_0000)),
        ],
    };
    let p = draw_scene("blend", &scene);
    assert_rgb(&p, 16, 30, [128, 0, 127], 12);
    assert_rgb(&p, 48, 30, [0, 0, 255], 0);
}

#[test]
fn vertex_fog_and_table_fog_mix_in_the_fog_color() {
    let fogged = |alpha: u32, x0: f32, x1: f32| {
        quad(x0, 0.0, x1, 32.0, 0.5, WHITE)
            .into_iter()
            .map(|v| Tl {
                specular: alpha << 24,
                ..v
            })
            .collect()
    };
    // Table fog by W, linear from 0 to 4: half at W 2.
    let table = quad(0.0, 32.0, 64.0, 64.0, 0.5, WHITE)
        .into_iter()
        .map(|v| Tl { rhw: 0.5, ..v })
        .collect();
    let scene = Scene {
        api: Api::D3D7,
        card: None,
        clear: 0,
        zbuffer: None,
        texture: None,
        steps: vec![
            Step::State(RS_FOGENABLE, 1),
            Step::State(RS_FOGCOLOR, 0x00_FF00),
            Step::Draw(fogged(0, 0.0, 32.0)),
            Step::Draw(fogged(0xFF, 32.0, 64.0)),
            Step::State(RS_FOGTABLEMODE, 3),
            Step::State(RS_FOGSTART, 0f32.to_bits()),
            Step::State(RS_FOGEND, 4f32.to_bits()),
            Step::Draw(table),
        ],
    };
    let p = draw_scene("fog", &scene);
    // All fog, none, and half.
    assert_rgb(&p, 16, 16, [0, 255, 0], 8);
    assert_rgb(&p, 48, 16, [255, 255, 255], 8);
    assert_rgb(&p, 32, 48, [128, 255, 128], 20);
}

/// A scene with the add-on card `board`: flat, gouraud, z-tested and
/// textured quads on its back buffer.
fn card_scene(test: &str, board: Board) {
    let texels = std::array::from_fn(|i| if i % 8 < 4 { 0xFFFF } else { 0x001F });
    let scene = Scene {
        api: Api::D3D7,
        card: Some(board),
        clear: 0xFF00_0080,
        zbuffer: Some(1.0),
        texture: Some(texels),
        steps: vec![
            Step::State(RS_ZENABLE, 1),
            Step::Draw(quad(0.0, 0.0, 32.0, 32.0, 0.25, GREEN)),
            Step::Draw(quad(0.0, 0.0, 64.0, 32.0, 0.75, WHITE)),
            Step::Draw(quad(0.0, 32.0, 64.0, 64.0, 0.5, WHITE)),
        ],
    };
    let p = draw_scene(test, &scene);
    // The nearer green quad, the texture's white and blue halves under
    // it, and the farther quad beside it.
    assert_rgb(&p, 8, 16, [0, 255, 0], 8);
    assert_rgb(&p, 48, 16, [0, 0, 255], 8);
    assert_rgb(&p, 8, 48, [255, 255, 255], 8);
    assert_rgb(&p, 48, 48, [0, 0, 255], 8);
}

#[test]
fn the_3dfx_device_draws_on_a_voodoo2() {
    card_scene("card_v2", Board::Voodoo2);
}

#[test]
fn the_3dfx_device_draws_on_a_voodoo_graphics() {
    card_scene("card_v1", Board::Standard);
}

#[test]
fn the_specular_color_adds_to_textured_and_untextured_triangles() {
    let gray = |textured: bool| Scene {
        api: Api::D3D7,
        card: None,
        clear: 0,
        zbuffer: Some(1.0),
        texture: textured.then_some([0xFFFF; 64]),
        steps: vec![
            Step::State(RS_ZENABLE, 1),
            Step::State(RS_SPECULARENABLE, 1),
            Step::Draw(
                quad(0.0, 0.0, 32.0, 64.0, 0.5, 0xFF80_8080)
                    .into_iter()
                    .map(|v| Tl {
                        specular: 0xFF40_4040,
                        ..v
                    })
                    .collect(),
            ),
        ],
    };
    // Half the white texture, or the diffuse gray, and a quarter more.
    for (test, textured) in [("specular_textured", true), ("specular", false)] {
        let p = draw_scene(test, &gray(textured));
        assert_rgb(&p, 16, 30, [192, 192, 192], 8);
        assert_rgb(&p, 48, 30, [0, 0, 0], 0);
    }
}
