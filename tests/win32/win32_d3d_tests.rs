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
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

const BASE: u32 = 0x40_0000;

/// Where a test program's data and imports are, for its code.
pub(super) struct Layout {
    data: u32,
    iat: Vec<u32>,
}

impl Layout {
    /// The address of byte `offset` of the data.
    pub(super) fn at(&self, offset: u32) -> u64 {
        (self.data + offset) as u64
    }

    /// The import address table slot of the function `index`.
    fn import(&self, index: usize) -> AsmMemoryOperand {
        dword_ptr(self.iat[index] as u64)
    }
}

/// A program importing `imports`, with `data` and the code `f` makes,
/// and where its data is.
pub(super) fn program(
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
pub(super) fn machine(test: &str, program: Vec<u8>) -> Cpu {
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

/// A machine with a ToadForce as its display adapter, the built-in
/// Windows on and `program` loaded from the shell as T.EXE.
pub(super) fn machine_toadforce(test: &str, program: Vec<u8>) -> Cpu {
    let base = PathBuf::from("target/test_win32_d3d").join(test);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join("T.EXE"), program).unwrap();
    let mut cpu = Cpu::new(base);
    let setup = VideoSetup {
        adapter: Adapter::ToadForce,
        mono_monitor: false,
    };
    rust_dos::video::bios::install(&mut cpu.bus, setup);
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
pub(super) fn peek(cpu: &Cpu, lin: u32) -> u32 {
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

pub(super) fn peek_f32(cpu: &Cpu, lin: u32) -> f32 {
    f32::from_bits(peek(cpu, lin))
}

/// Run the program until it says it is done (DONE in its data), with
/// its results there.
pub(super) fn run_until_done(cpu: &mut Cpu, data: u32) {
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
pub(super) const RESULTS: u32 = 0x1000;
pub(super) const DONE: u32 = 0xF0;
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

pub(super) fn put(data: &mut [u8], offset: u32, bytes: &[u8]) {
    data[offset as usize..offset as usize + bytes.len()].copy_from_slice(bytes);
}

pub(super) fn put32(data: &mut [u8], offset: u32, value: u32) {
    put(data, offset, &value.to_le_bytes());
}

pub(super) fn put_f32s(data: &mut [u8], offset: u32, values: &[f32]) {
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
pub(super) enum Arg {
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
pub(super) struct Calls {
    pub(super) next: u32,
}

impl Calls {
    fn keep(&mut self, a: &mut CodeAssembler, l: &Layout) -> Result<(), IcedError> {
        a.mov(dword_ptr(l.at(RESULTS + 4 * self.next)), eax)?;
        self.next += 1;
        Ok(())
    }

    /// Call the import `index` with `args`.
    pub(super) fn api(
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
    pub(super) fn com(
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
pub(super) fn copy(
    a: &mut CodeAssembler,
    l: &Layout,
    from: u32,
    to: u32,
    len: u32,
) -> Result<(), IcedError> {
    a.mov(edi, dword_ptr(l.at(to)))?;
    a.mov(esi, l.at(from) as u32)?;
    a.mov(ecx, len)?;
    a.cld()?;
    a.rep().movsb()
}

/// The end of a program: DONE set, and it spins, its memory there to
/// look at.
pub(super) fn finish(a: &mut CodeAssembler, l: &Layout) -> Result<(), IcedError> {
    a.mov(dword_ptr(l.at(DONE)), 1)?;
    let mut again = a.create_label();
    a.set_label(&mut again)?;
    a.jmp(again)
}

/// The results of the first `n` calls, read from the program's data.
pub(super) fn results(cpu: &Cpu, data: u32, n: u32) -> Vec<u32> {
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
const D3DPT_POINTLIST: u32 = 1;
const D3DPT_LINELIST: u32 = 2;
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

/// D3DVBCAPS_DONOTCLIP.
const D3DVBCAPS_DONOTCLIP: u32 = 0x1;
const D3DVOP_CLIP: u32 = 0x4;
const D3DTRANSFORMSTATE_PROJECTION: u32 = 3;
const D3DRENDERSTATE_CULLMODE: u32 = 22;
const D3DCULL_NONE: u32 = 1;
const D3DCULL_CW: u32 = 2;
const DEV7_SET_TRANSFORM: u32 = 11;
const PROJECTION: u32 = LIGHT;

/// Direct3D 7: a red triangle with a corner behind the eye, drawn with
/// DrawPrimitiveVB after the ProcessVertices calls `steps` (operations,
/// index and count) into a buffer with the caps `caps`. Its near edge is
/// across the screen at y 32, and its bottom edge at y 48; below that is
/// where the corner behind the eye goes on the screen. The render target's
/// pixels, row after row, 5-6-5.
fn draw_processed_triangle(test: &str, caps: u32, steps: &[(u32, u32, u32)]) -> Vec<u16> {
    let mut d = data();
    target_desc(&mut d, DESC, 124, 64, 64);
    put32(&mut d, LOCK_DESC, 124);
    put32(&mut d, VB_DESC, 16);
    put32(&mut d, VB_DESC + 8, D3DFVF_XYZ | D3DFVF_DIFFUSE);
    put32(&mut d, VB_DESC + 12, 3);
    put32(&mut d, VB_DESC + 16, 16);
    put32(&mut d, VB_DESC + 20, caps);
    put32(&mut d, VB_DESC + 24, D3DFVF_XYZRHW | D3DFVF_DIFFUSE);
    put32(&mut d, VB_DESC + 28, 3);
    put32(&mut d, VIEW + 8, 64);
    put32(&mut d, VIEW + 12, 64);
    put_f32s(&mut d, VIEW + 16, &[0.0, 1.0]);
    // A perspective projection, near plane 1 and far plane 10.
    let q = 10.0 / 9.0;
    put_f32s(
        &mut d,
        PROJECTION,
        &[
            1.0, 0.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, q, 1.0, //
            0.0, 0.0, -q, 0.0,
        ],
    );
    let corners = [[-1.0, -1.0, 2.0], [1.0, -1.0, 2.0], [0.0, 3.0, -2.0]];
    for (i, corner) in corners.iter().enumerate() {
        let at = VERTICES + 16 * i as u32;
        put_f32s(&mut d, at, corner);
        put32(&mut d, at + 12, 0xFFFF_0000);
    }
    let (exe, data) = program(&[USER32, DDRAW], &d, |a, l| {
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
        copy(a, l, VERTICES, LOCKED, 48)?;
        c.com(a, l, SOURCE, VB_UNLOCK, &[])?;
        c.com(a, l, DEVICE, DEV7_SET_VIEWPORT, &[Arg::At(VIEW)])?;
        c.com(
            a,
            l,
            DEVICE,
            DEV7_SET_TRANSFORM,
            &[Arg::I(D3DTRANSFORMSTATE_PROJECTION), Arg::At(PROJECTION)],
        )?;
        c.com(
            a,
            l,
            DEVICE,
            DEV7_SET_RENDER_STATE,
            &[Arg::I(D3DRENDERSTATE_CULLMODE), Arg::I(D3DCULL_NONE)],
        )?;
        for &(op, index, count) in steps {
            c.com(
                a,
                l,
                DEST,
                VB_PROCESS_VERTICES,
                &[
                    Arg::I(op),
                    Arg::I(index),
                    Arg::I(count),
                    Arg::Val(SOURCE),
                    Arg::I(index),
                    Arg::Val(DEVICE),
                    Arg::I(0),
                ],
            )?;
        }
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
        read_target(a, l, &mut c, 16)?;
        finish(a, l)
    });
    let mut cpu = machine(test, exe);
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

/// Whether the triangle of `draw_processed_triangle` came out clipped at
/// the near plane, and whether it came out as the wedge of a corner
/// behind the eye drawn at its screen position.
fn clipped_and_wedge(pixels: &[u16]) -> (bool, bool) {
    let red = |y: usize| rgb(pixels, 32, y)[0] > 0x80;
    (red(40), red(56))
}

#[test]
fn process_vertices_with_clip_keeps_what_the_draw_clips_with() {
    let op = D3DVOP_TRANSFORM | D3DVOP_CLIP;
    let pixels = draw_processed_triangle("vb_clip", 0, &[(op, 0, 3)]);
    assert_eq!(clipped_and_wedge(&pixels), (true, false));
}

#[test]
fn a_buffer_that_can_not_clip_is_drawn_from_its_screen_vertices() {
    let op = D3DVOP_TRANSFORM | D3DVOP_CLIP;
    let pixels = draw_processed_triangle("vb_donotclip", D3DVBCAPS_DONOTCLIP, &[(op, 0, 3)]);
    assert_eq!(clipped_and_wedge(&pixels), (false, true));
}

#[test]
fn process_vertices_without_clip_drops_what_was_kept() {
    let op = D3DVOP_TRANSFORM | D3DVOP_CLIP;
    let steps = [(op, 0, 3), (D3DVOP_TRANSFORM, 0, 3)];
    let pixels = draw_processed_triangle("vb_clip_dropped", 0, &steps);
    assert_eq!(clipped_and_wedge(&pixels), (false, true));
}

#[test]
fn a_corner_behind_the_eye_without_clip_data_is_still_clipped_by_the_others() {
    // Only the corner behind the eye has no clip coordinates kept: it
    // has them back from its screen position, rhw below 0.
    let op = D3DVOP_TRANSFORM | D3DVOP_CLIP;
    let steps = [(op, 0, 2), (D3DVOP_TRANSFORM, 2, 1)];
    let pixels = draw_processed_triangle("vb_clip_mixed", 0, &steps);
    assert_eq!(clipped_and_wedge(&pixels), (true, false));
}

// D3DOP_ instructions and D3DPROCESSVERTICES_COPY.
const D3DOP_TRIANGLE: u8 = 3;
const D3DOP_STATERENDER: u8 = 8;
const D3DOP_PROCESSVERTICES: u8 = 9;
const D3DOP_EXIT: u8 = 11;
const D3DPROCESSVERTICES_COPY: u32 = 2;

// Execute's flags.
const D3DEXECUTE_CLIPPED: u32 = 0x1;
const D3DEXECUTE_UNCLIPPED: u32 = 0x2;

/// Direct3D 5: a device on a 64x64 surface with a viewport, an execute
/// buffer that copies three screen vertices and draws a triangle of
/// them, and DrawPrimitive with the same vertices; then GetStats.
fn execute_program() -> (Vec<u8>, u32) {
    execute_program_with(0.5, 0, None)
}

/// `execute_program` with the vertices at depth `z`, `flags` for
/// Execute, and a STATERENDER instruction setting the render state
/// `state` between the buffer's PROCESSVERTICES and TRIANGLE.
fn execute_program_with(z: f32, flags: u32, state: Option<(u32, u32)>) -> (Vec<u8>, u32) {
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
        put_f32s(&mut d, v, &[*x, *y, z, 1.0]);
        put32(&mut d, v + 16, 0xFFFF_FFFF);
    }
    let code = VERTICES + 96;
    put(&mut d, code, &[D3DOP_PROCESSVERTICES, 16, 1, 0]);
    put32(&mut d, code + 4, D3DPROCESSVERTICES_COPY);
    put32(&mut d, code + 12, 3);
    let mut at = code + 20;
    if let Some((s, v)) = state {
        put(&mut d, at, &[D3DOP_STATERENDER, 8, 1, 0]);
        put32(&mut d, at + 4, s);
        put32(&mut d, at + 8, v);
        at += 12;
    }
    put(&mut d, at, &[D3DOP_TRIANGLE, 8, 1, 0]);
    put(&mut d, at + 4, &[0, 0, 1, 0, 2, 0, 0, 7]);
    put(&mut d, at + 12, &[D3DOP_EXIT, 0, 0, 0]);
    let length = at + 16 - code;
    // D3DEXECUTEBUFFERDESC: 256 bytes; D3DEXECUTEDATA: the vertices and
    // the instructions.
    put32(&mut d, EB_DESC, 20);
    put32(&mut d, EB_DESC + 4, 1);
    put32(&mut d, EB_DESC + 12, 256);
    put32(&mut d, EXECUTE_DATA, 48);
    put32(&mut d, EXECUTE_DATA + 8, 3);
    put32(&mut d, EXECUTE_DATA + 12, 96);
    put32(&mut d, EXECUTE_DATA + 16, length);
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
        copy(a, l, VERTICES, EB_DESC + 16, 96 + length)?;
        c.com(a, l, BUFFER, EB_UNLOCK, &[])?;
        c.com(a, l, BUFFER, EB_SET_EXECUTE_DATA, &[Arg::At(EXECUTE_DATA)])?;
        c.com(a, l, DEVICE, DEV2_BEGIN_SCENE, &[])?;
        c.com(
            a,
            l,
            DEVICE1,
            DEV1_EXECUTE,
            &[Arg::Val(BUFFER), Arg::Val(VIEWPORT), Arg::I(flags)],
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
    assert_eq!(execute_stats(&cpu, data), [2, 0, 0, 6]);
    // The white triangle, on the target's black.
    assert_eq!(locked_pixel(&cpu, data, 32, 20), 0xFFFF);
    assert_eq!(locked_pixel(&cpu, data, 32, 60), 0);
}

/// The pixel at (`x`, `y`) of the 16-bit target the program locked.
fn locked_pixel(cpu: &Cpu, data: u32, x: u32, y: u32) -> u16 {
    let bits = peek(cpu, data + LOCK_DESC + 0x24);
    let pitch = peek(cpu, data + LOCK_DESC + 0x10);
    let at = bits + y * pitch + 2 * x;
    (peek(cpu, at & !3) >> (8 * (at & 2))) as u16
}

/// D3DSTATS of the execute program: triangles, lines, points and
/// vertices processed.
fn execute_stats(cpu: &Cpu, data: u32) -> [u32; 4] {
    [4, 8, 12, 20].map(|o| peek(cpu, data + STATS + o))
}

#[test]
fn an_unclipped_execute_draws_screen_vertices_past_z_1() {
    // WarGames draws its screen vertices at depths of 4 to 33 with depth
    // testing off, through Execute with D3DEXECUTE_UNCLIPPED: Direct3D
    // doesn't clip them, and the triangle shows. DrawPrimitive's copy of
    // it is clipped at z 1 and draws nothing: one triangle in the stats.
    let (exe, data) = execute_program_with(4.0, D3DEXECUTE_UNCLIPPED, None);
    let mut cpu = machine("execute_unclipped", exe);
    run_until_done(&mut cpu, data);
    let r = results(&cpu, data, 22);
    assert!(r[1..].iter().all(|&hr| hr == 0), "results: {r:X?}");
    assert_eq!(execute_stats(&cpu, data), [1, 0, 0, 6]);
    assert_eq!(locked_pixel(&cpu, data, 32, 20), 0xFFFF);
    assert_eq!(locked_pixel(&cpu, data, 32, 60), 0);
}

#[test]
fn a_clipped_execute_clips_screen_vertices_past_z_1() {
    // D3DEXECUTE_CLIPPED, alone or with D3DEXECUTE_UNCLIPPED, keeps the
    // clipping at z 0 and 1: neither the buffer's triangle nor
    // DrawPrimitive's draws.
    let both = D3DEXECUTE_CLIPPED | D3DEXECUTE_UNCLIPPED;
    for flags in [D3DEXECUTE_CLIPPED, both] {
        let (exe, data) = execute_program_with(4.0, flags, None);
        let mut cpu = machine("execute_clipped", exe);
        run_until_done(&mut cpu, data);
        let r = results(&cpu, data, 22);
        assert!(r[1..].iter().all(|&hr| hr == 0), "flags {flags}: {r:X?}");
        assert_eq!(execute_stats(&cpu, data), [0, 0, 0, 6], "flags {flags}");
        assert_eq!(locked_pixel(&cpu, data, 32, 20), 0, "flags {flags}");
    }
}

#[test]
fn an_execute_draws_with_the_render_states_set_after_processvertices() {
    // A STATERENDER between PROCESSVERTICES and TRIANGLE sets the cull
    // mode the triangle is drawn with, as Wine's execute buffer draws
    // with the device's states at the TRIANGLE. Clockwise culling takes
    // the triangle out, and DrawPrimitive's after it too.
    let state = Some((D3DRENDERSTATE_CULLMODE, D3DCULL_CW));
    let (exe, data) = execute_program_with(0.5, 0, state);
    let mut cpu = machine("execute_state", exe);
    run_until_done(&mut cpu, data);
    let r = results(&cpu, data, 22);
    assert!(r[1..].iter().all(|&hr| hr == 0), "results: {r:X?}");
    assert_eq!(execute_stats(&cpu, data), [0, 0, 0, 6]);
    assert_eq!(locked_pixel(&cpu, data, 32, 20), 0);
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
const KEY: u32 = 0x998;
const SCENE: u32 = 0xA00;

/// A `Step::Cube`'s data: where the six DDSCAPS2, the faces' texels, a
/// face's interface and the loaded cube map's interface are after its
/// DDSURFACEDESC2, and its bytes.
const CUBE_CAPS: u32 = 124;
const CUBE_TEXELS: u32 = CUBE_CAPS + 6 * 16;
const CUBE_FACE: u32 = CUBE_TEXELS + 6 * 32;
const CUBE_LOADED: u32 = CUBE_FACE + 4;
const CUBE_BYTES: u32 = CUBE_LOADED + 4;
/// IDirect3DDevice7::Load.
const DEV7_LOAD: u32 = 43;

/// Where a `Step::Cube`'s cube map is filled, and what is sampled.
#[derive(Clone, Copy, PartialEq)]
enum Filled {
    /// In video memory, where the card samples it.
    Video,
    /// In system memory: the card samples the HAL's copy.
    System,
    /// In system memory, then loaded into one in video memory
    /// (IDirect3DDevice7::Load), which the card samples.
    Loaded,
}
const DDSCAPS2_CUBEMAP: u32 = 0x200;
const DDSCAPS2_CUBEMAP_POSITIVEX: u32 = 0x400;
const DDSCAPS2_CUBEMAP_ALLFACES: u32 = 0xFC00;

const DDSD_PIXELFORMAT: u32 = 0x1000;
const DDSCAPS_TEXTURE: u32 = 0x1000;
const DDSCAPS_ZBUFFER: u32 = 0x2_0000;
const DDSCAPS_VIDEOMEMORY: u32 = 0x4000;
const DDSCAPS_BACKBUFFER: u32 = 0x4;
const DDSCAPS_COMPLEX: u32 = 0x8;
const DDSCAPS_FLIP: u32 = 0x10;
const DDSCAPS_PRIMARYSURFACE: u32 = 0x200;
const DDSD_BACKBUFFERCOUNT: u32 = 0x20;
const DDPF_ALPHAPIXELS: u32 = 0x1;
const DDPF_RGB: u32 = 0x40;
const DDPF_ZBUFFER: u32 = 0x400;
const DDPF_STENCILBUFFER: u32 = 0x4000;
const DDLOCK_WAIT: u32 = 1;
const DDFLIP_WAIT: u32 = 1;
const DDFLIP_NOVSYNC: u32 = 8;
const D3DCLEAR_TARGET: u32 = 1;
const D3DCLEAR_ZBUFFER: u32 = 2;
const D3DCLEAR_STENCIL: u32 = 4;
const D3DERR_ZBUFFER_NOTPRESENT: u32 = 0x8876_0816;
const D3DERR_STENCILBUFFER_NOTPRESENT: u32 = 0x8876_0817;
/// D3DFVF_XYZRHW, DIFFUSE, SPECULAR and TEX1: a D3DTLVERTEX.
const TL_FVF: u32 = 0x1C4;

const SURF_ADD_ATTACHED_SURFACE: u32 = 3;
const SURF_BLT: u32 = 5;
const DDBLT_WAIT: u32 = 0x100_0000;
const DDBLT_DEPTHFILL: u32 = 0x200_0000;
const SURF_GET_ATTACHED_SURFACE: u32 = 12;
const SURF_FLIP: u32 = 11;
const SURF_LOCK: u32 = 25;
const SURF_SET_COLOR_KEY: u32 = 29;
const SURF_UNLOCK: u32 = 32;
const D3D3_CREATE_VIEWPORT: u32 = 6;
const D3D3_CREATE_DEVICE: u32 = 8;
const DEV3_ADD_VIEWPORT: u32 = 5;
const DEV3_BEGIN_SCENE: u32 = 9;
const DEV3_END_SCENE: u32 = 10;
const DEV3_SET_CURRENT_VIEWPORT: u32 = 12;
const DEV3_SET_RENDER_STATE: u32 = 22;
const DEV3_DRAW_PRIMITIVE: u32 = 28;
const VP3_CLEAR: u32 = 12;
const VP3_CLEAR2: u32 = 20;
const DEV7_CLEAR: u32 = 10;
const DEV7_SET_RENDER_STATE: u32 = 20;
const DEV7_DRAW_PRIMITIVE: u32 = 25;
const DEV7_SET_TEXTURE: u32 = 35;
const DEV7_SET_TEXTURE_STAGE_STATE: u32 = 37;
const DDCKEY_SRCBLT: u32 = 8;
const TSS_COLOROP: u32 = 1;
const TSS_ALPHAOP: u32 = 4;
const TOP_SELECTARG2: u32 = 3;
const TOP_SELECTARG1: u32 = 2;
const TOP_ADD: u32 = 7;
const TOP_BLENDDIFFUSEALPHA: u32 = 12;
const TOP_BLENDCURRENTALPHA: u32 = 16;
const TSS_ALPHAARG1: u32 = 5;
const TSS_ALPHAARG2: u32 = 6;
const TOP_MODULATE: u32 = 4;
const TOP_SUBTRACT: u32 = 10;
const TA_DIFFUSE: u32 = 0;
const TA_TEXTURE: u32 = 2;
const TA_TFACTOR: u32 = 3;

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
const RS_ALPHATESTENABLE: u32 = 15;
const RS_ALPHAREF: u32 = 24;
const RS_ALPHAFUNC: u32 = 25;
const RS_COLORKEYENABLE: u32 = 41;
const RS_TEXTUREFACTOR: u32 = 60;

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

/// `v` with the vertex fog factor `factor` (255 no fog, 0 all fog) in
/// its specular alpha.
fn with_fog(v: Tl, factor: u32) -> Tl {
    Tl {
        specular: factor << 24,
        ..v
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
    /// DrawPrimitive of another D3DPT_ type, such as a point or line list.
    Prims(u32, Vec<Tl>),
    /// A method of the device (Direct3D 7) with numbers for arguments
    /// and, after them, a pointer to words if there are any:
    /// SetTransform, SetLight, SetMaterial, LightEnable.
    Call(u32, Vec<u32>, Vec<u32>),
    /// DrawPrimitive of a triangle list of vertices that aren't
    /// transformed, laid out as the FVF: the words of all of them, and
    /// how many they are.
    World(u32, Vec<u32>, u32),
    /// 256x256 textures made in video memory until there is no more
    /// room (Direct3D 7), as Unreal does.
    Fill,
    /// Flip of the flip chain, without waiting for the frame.
    Flip,
    /// SetTextureStageState of stage 0 (Direct3D 7).
    Stage(u32, u32),
    /// SetTexture of the scene's texture on a stage (Direct3D 7).
    Texture(u32),
    /// A cube map of 4x4 565 faces, each of one colour (+x, -x, +y, -y,
    /// +z, -z), made, filled where `Filled` says and put on stage 0
    /// (Direct3D 7).
    Cube([u16; 6], Filled),
    /// SetColorKey of the texture: its source blit key (Direct3D 7).
    /// A scene has one key: every Key step sets the last one's.
    Key(u32),
    /// A clear with D3DCLEAR_ flags and an ARGB color, to depth 1:
    /// IDirect3DDevice7's Clear, or Direct3D 6's viewport's Clear2. Its
    /// result is the test's to check (`clear_scene`).
    Clear(u32, u32),
    /// The Direct3D 6 viewport's Clear with D3DCLEAR_ flags, to its
    /// background material, black without one. Its result is the
    /// test's to check.
    ViewportClear(u32),
    /// Blt with DDBLT_DEPTHFILL of the z-buffer to a value, in a
    /// rectangle (left, top, right, bottom) or all of it, as Omikron
    /// fills its z-buffer every frame.
    DepthFill(u32, Option<[i32; 4]>),
}

/// Which Direct3D draws a scene.
#[derive(Clone, Copy, PartialEq)]
enum Api {
    /// Direct3D 6: IDirect3DDevice3, cleared through its viewport.
    D3D6,
    /// Direct3D 7: IDirect3DDevice7.
    D3D7,
    /// Direct3D 7 on the T&L HAL device, which a card that transforms
    /// and lights has.
    D3D7Tnl,
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
    /// The texture is 4444, with alpha, in place of 565.
    texture_alpha: bool,
    /// The texture is in system memory, where no card reads it.
    texture_system: bool,
    steps: Vec<Step>,
}

/// IID_IDirect3DTnLHalDevice at `offset`.
fn tnl_guid(data: &mut [u8], offset: u32) {
    let d4 = [0xA4, 0x07, 0x00, 0xA0, 0xC9, 0x06, 0x29, 0xA8];
    guid(data, offset, 0xF504_9E78, 0x4861, 0x11D2, d4);
}

/// A program drawing `scene`, which ends with the target locked.
fn scene_program(scene: &Scene) -> (Vec<u8>, u32) {
    scene_program_in(scene, 16)
}

/// The same in a mode of `bpp` bits a pixel (16 or 32), whose z-buffer
/// has as many bits (24 of them depth at 32).
fn scene_program_in(scene: &Scene, bpp: u32) -> (Vec<u8>, u32) {
    scene_program_with(scene, bpp, false, &mut Vec::new())
}

/// The same, noting in `checked` where in the results the calls of the
/// scene's Clear and ViewportClear steps are. With `stencil` the
/// z-buffer of a 32-bit mode has 8 bits of stencil under its 24 of
/// depth, the format a ToadForce's EnumZBufferFormats gives.
fn scene_program_with(
    scene: &Scene,
    bpp: u32,
    stencil: bool,
    checked: &mut Vec<u32>,
) -> (Vec<u8>, u32) {
    let mut d = data();
    if scene.api == Api::D3D7Tnl {
        tnl_guid(&mut d, IID_HAL);
    }
    let size = if scene.api != Api::D3D6 { 124 } else { 108 };
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
    if scene.api != Api::D3D6 {
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
    if stencil {
        put32(&mut d, Z_DESC + 0x4C, DDPF_ZBUFFER | DDPF_STENCILBUFFER);
        put32(&mut d, Z_DESC + 0x58, 8);
        put32(&mut d, Z_DESC + 0x5C, 0xFFFF_FF00);
        put32(&mut d, Z_DESC + 0x60, 0xFF);
    }
    put32(&mut d, Z_DESC + 0x68, DDSCAPS_ZBUFFER | DDSCAPS_VIDEOMEMORY);
    put32(&mut d, TEX_DESC, 124);
    put32(&mut d, TEX_DESC + 0x04, flags);
    put32(&mut d, TEX_DESC + 0x08, 8);
    put32(&mut d, TEX_DESC + 0x0C, 8);
    put32(&mut d, TEX_DESC + 0x48, 32);
    put32(&mut d, TEX_DESC + 0x54, 16);
    if scene.texture_alpha {
        put32(&mut d, TEX_DESC + 0x4C, DDPF_RGB | DDPF_ALPHAPIXELS);
        put32(&mut d, TEX_DESC + 0x58, 0x0F00);
        put32(&mut d, TEX_DESC + 0x5C, 0x00F0);
        put32(&mut d, TEX_DESC + 0x60, 0x000F);
        put32(&mut d, TEX_DESC + 0x64, 0xF000);
    } else {
        put32(&mut d, TEX_DESC + 0x4C, DDPF_RGB);
        put32(&mut d, TEX_DESC + 0x58, 0xF800);
        put32(&mut d, TEX_DESC + 0x5C, 0x07E0);
        put32(&mut d, TEX_DESC + 0x60, 0x001F);
    }
    // DDSCAPS_SYSTEMMEMORY.
    let system = if scene.texture_system { 0x800 } else { 0 };
    put32(&mut d, TEX_DESC + 0x68, DDSCAPS_TEXTURE | system);
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
        if matches!(step, Step::Stage(..) | Step::Key(_)) {
            assert!(scene.api != Api::D3D6, "a Direct3D 7 step");
        }
        if matches!(step, Step::ViewportClear(_)) {
            assert!(scene.api == Api::D3D6, "a Direct3D 6 step");
        }
        if let Step::Key(key) = step {
            assert!(scene.texture.is_some(), "a key without a texture");
            put32(&mut d, KEY, *key);
            put32(&mut d, KEY + 4, *key);
        }
        if let Step::DepthFill(value, rect) = step {
            // The DDBLTFX, and after it the rectangle.
            draws.push(at);
            put32(&mut d, at, 100);
            put32(&mut d, at + 0x50, *value);
            for (i, v) in rect.unwrap_or_default().iter().enumerate() {
                put32(&mut d, at + 100 + 4 * i as u32, *v as u32);
            }
            at += 116;
        }
        if let Step::Cube(faces, filled) = step {
            // The cube map's DDSURFACEDESC2, the DDSCAPS2 that finds
            // each face, each face's texels, and a face's interface.
            draws.push(at);
            let desc = d[TEX_DESC as usize..TEX_DESC as usize + 124].to_vec();
            put(&mut d, at, &desc);
            put32(&mut d, at + 0x08, 4);
            put32(&mut d, at + 0x0C, 4);
            let memory = match filled {
                Filled::Video => 0,
                Filled::System | Filled::Loaded => DDSCAPS_SYSTEMMEMORY,
            };
            put32(
                &mut d,
                at + 0x68,
                DDSCAPS_TEXTURE | DDSCAPS_COMPLEX | memory,
            );
            put32(&mut d, at + 0x6C, DDSCAPS2_CUBEMAP | DDSCAPS2_CUBEMAP_ALLFACES);
            for (face, colour) in faces.iter().enumerate() {
                let face = face as u32;
                let bit = DDSCAPS2_CUBEMAP_POSITIVEX << face;
                put32(&mut d, at + CUBE_CAPS + 16 * face + 4, DDSCAPS2_CUBEMAP | bit);
                for texel in 0..16 {
                    let to = at + CUBE_TEXELS + 32 * face + 2 * texel;
                    put(&mut d, to, &colour.to_le_bytes());
                }
            }
            at += CUBE_BYTES;
        }
        if let Step::Call(_, _, words) | Step::World(_, words, _) = step {
            draws.push(at);
            for word in words {
                put32(&mut d, at, *word);
                at += 4;
            }
        }
        if let Step::Draw(verts) | Step::Prims(_, verts) = step {
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
    let ex = scene.api != Api::D3D6;
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
                Step::Stage(s, v) => {
                    let args = [Arg::I(0), Arg::I(*s), Arg::I(*v)];
                    c.com(a, l, DEVICE, DEV7_SET_TEXTURE_STAGE_STATE, &args)?;
                }
                Step::Texture(stage) => {
                    let args = [Arg::I(*stage), Arg::Val(TEXTURE)];
                    c.com(a, l, DEVICE, DEV7_SET_TEXTURE, &args)?;
                }
                Step::Cube(_, filled) => {
                    let at = *draws.next().unwrap();
                    let face_at = at + CUBE_FACE;
                    let args = [Arg::At(at), Arg::At(TEXTURE), Arg::I(0)];
                    c.com(a, l, DD, DD_CREATE_SURFACE, &args)?;
                    for face in 0..6 {
                        // The root is the first face, and has the others.
                        let object = if face == 0 {
                            TEXTURE
                        } else {
                            let args = [Arg::At(at + CUBE_CAPS + 16 * face), Arg::At(face_at)];
                            c.com(a, l, TEXTURE, SURF_GET_ATTACHED_SURFACE, &args)?;
                            face_at
                        };
                        let args = [
                            Arg::I(0),
                            Arg::At(TEX_LOCK_DESC),
                            Arg::I(DDLOCK_WAIT),
                            Arg::I(0),
                        ];
                        c.com(a, l, object, SURF_LOCK, &args)?;
                        copy(a, l, at + CUBE_TEXELS + 32 * face, TEX_LOCK_DESC + 0x24, 32)?;
                        c.com(a, l, object, SURF_UNLOCK, &[Arg::I(0)])?;
                    }
                    let mut sampled = TEXTURE;
                    if *filled == Filled::Loaded {
                        // A cube map in video memory of the same faces,
                        // which takes all of them from the filled one.
                        let caps = DDSCAPS_TEXTURE | DDSCAPS_COMPLEX | DDSCAPS_VIDEOMEMORY;
                        a.mov(dword_ptr(l.at(at + 0x68)), caps)?;
                        let args = [Arg::At(at), Arg::At(at + CUBE_LOADED), Arg::I(0)];
                        c.com(a, l, DD, DD_CREATE_SURFACE, &args)?;
                        let args = [
                            Arg::Val(at + CUBE_LOADED),
                            Arg::I(0),
                            Arg::Val(TEXTURE),
                            Arg::I(0),
                            Arg::I(DDSCAPS2_CUBEMAP_ALLFACES),
                        ];
                        c.com(a, l, DEVICE, DEV7_LOAD, &args)?;
                        sampled = at + CUBE_LOADED;
                    }
                    let args = [Arg::I(0), Arg::Val(sampled)];
                    c.com(a, l, DEVICE, DEV7_SET_TEXTURE, &args)?;
                }
                Step::Key(_) => {
                    let args = [Arg::I(DDCKEY_SRCBLT), Arg::At(KEY)];
                    c.com(a, l, TEXTURE, SURF_SET_COLOR_KEY, &args)?;
                }
                Step::Clear(flags, color) => {
                    checked.push(c.next);
                    let (object, method, count, rects) = if ex {
                        (DEVICE, DEV7_CLEAR, 0, Arg::I(0))
                    } else {
                        (VIEWPORT, VP3_CLEAR2, 1, Arg::At(RECT))
                    };
                    let args = [
                        Arg::I(count),
                        rects,
                        Arg::I(*flags),
                        Arg::I(*color),
                        Arg::I(1.0f32.to_bits()),
                        Arg::I(0),
                    ];
                    c.com(a, l, object, method, &args)?;
                }
                Step::ViewportClear(flags) => {
                    checked.push(c.next);
                    let args = [Arg::I(1), Arg::At(RECT), Arg::I(*flags)];
                    c.com(a, l, VIEWPORT, VP3_CLEAR, &args)?;
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
                Step::DepthFill(_, rect) => {
                    let at = *draws.next().unwrap();
                    let rect = if rect.is_some() {
                        Arg::At(at + 100)
                    } else {
                        Arg::I(0)
                    };
                    let flags = DDBLT_DEPTHFILL | DDBLT_WAIT;
                    let args = [rect, Arg::I(0), Arg::I(0), Arg::I(flags), Arg::At(at)];
                    c.com(a, l, ZBUF, SURF_BLT, &args)?;
                }
                Step::Call(method, numbers, words) => {
                    let at = *draws.next().unwrap();
                    let mut args: Vec<Arg> = numbers.iter().map(|n| Arg::I(*n)).collect();
                    if !words.is_empty() {
                        args.push(Arg::At(at));
                    }
                    c.com(a, l, DEVICE, *method, &args)?;
                }
                Step::World(fvf, _, count) => {
                    let at = *draws.next().unwrap();
                    let args = [
                        Arg::I(D3DPT_TRIANGLELIST),
                        Arg::I(*fvf),
                        Arg::At(at),
                        Arg::I(*count),
                        Arg::I(0),
                    ];
                    c.com(a, l, DEVICE, draw, &args)?;
                }
                Step::Draw(verts) | Step::Prims(_, verts) => {
                    let at = *draws.next().unwrap();
                    let kind = match step {
                        Step::Prims(kind, _) => *kind,
                        _ => D3DPT_TRIANGLELIST,
                    };
                    c.com(
                        a,
                        l,
                        DEVICE,
                        draw,
                        &[
                            Arg::I(kind),
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
        read_target(a, l, &mut c, bpp)?;
        finish(a, l)
    })
}

/// Lock the render target, note how many calls there were in CALLS, and
/// copy the target's top left 64x64 pixels into PIXELS, a row at a time.
fn read_target(
    a: &mut CodeAssembler,
    l: &Layout,
    c: &mut Calls,
    bpp: u32,
) -> Result<(), IcedError> {
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
    Ok(())
}

/// The render target's pixels after `scene`, row after row, 5-6-5,
/// once every call in it succeeded.
fn draw_scene(test: &str, scene: &Scene) -> Vec<u16> {
    let (r, pixels) = clear_scene(test, scene);
    assert!(r.iter().all(|&hr| hr == 0), "Clear results: {r:X?}");
    pixels
}

/// The results of `scene`'s Clear and ViewportClear steps, and the
/// render target's pixels after it, once every other call succeeded.
fn clear_scene(test: &str, scene: &Scene) -> (Vec<u32>, Vec<u16>) {
    clear_scene_on(test, scene, false)
}

/// The same scene drawn by a ToadForce's HAL.
fn draw_scene_toadforce(test: &str, scene: &Scene) -> Vec<u16> {
    let (r, pixels) = clear_scene_on(test, scene, true);
    assert!(r.iter().all(|&hr| hr == 0), "Clear results: {r:X?}");
    pixels
}

/// The render target's pixels after `scene` on a ToadForce in a 32-bit
/// mode, X8R8G8B8.
fn draw_scene32_toadforce(test: &str, scene: &Scene) -> Vec<u32> {
    let (exe, data) = scene_program_in(scene, 32);
    let mut cpu = machine_toadforce(test, exe);
    run_until_done(&mut cpu, data);
    let n = peek(&cpu, data + CALLS);
    let r = results(&cpu, data, n);
    assert!(r[1..].iter().all(|&hr| hr == 0), "results: {r:X?}");
    drew_on_the_card(&cpu, scene);
    (0..64 * 64)
        .map(|i| peek(&cpu, data + PIXELS + 4 * i) & 0xFF_FFFF)
        .collect()
}

/// The card drew the scene's primitives, and took every command of the
/// HAL's.
fn drew_on_the_card(cpu: &Cpu, scene: &Scene) {
    let counts = cpu.bus.toadforce.as_ref().unwrap().counts();
    assert_eq!(counts.rejected, 0);
    let draws = scene
        .steps
        .iter()
        .filter(|step| matches!(step, Step::Draw(_) | Step::Prims(..) | Step::World(..)))
        .count() as u64;
    let transformed = scene
        .steps
        .iter()
        .filter(|step| matches!(step, Step::World(fvf, ..) if fvf & 0xE != D3DFVF_XYZRHW))
        .count() as u64;
    // The card transforms the vertices that aren't transformed.
    assert_eq!(counts.transformed, transformed, "{counts:?}");
    assert!(counts.draws >= draws, "{counts:?}");
    assert!(draws == 0 || counts.primitives > 0, "{counts:?}");
}

fn clear_scene_on(test: &str, scene: &Scene, toadforce: bool) -> (Vec<u32>, Vec<u16>) {
    let mut checked = Vec::new();
    let (exe, data) = scene_program_with(scene, 16, false, &mut checked);
    let mut cpu = if toadforce {
        machine_toadforce(test, exe)
    } else {
        machine_with(test, scene.card, exe)
    };
    run_until_done(&mut cpu, data);
    if toadforce {
        drew_on_the_card(&cpu, scene);
    }
    let n = peek(&cpu, data + CALLS);
    let r = results(&cpu, data, n);
    let mut others = (1..n).filter(|i| !checked.contains(i));
    assert!(others.all(|i| r[i as usize] == 0), "results: {r:X?}");
    let pixels = (0..64 * 64)
        .map(|i| {
            let at = data + PIXELS + 2 * i;
            (peek(&cpu, at & !3) >> (8 * (at & 2))) as u16
        })
        .collect();
    (checked.iter().map(|&i| r[i as usize]).collect(), pixels)
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
        texture_alpha: false,
        texture_system: false,
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

/// A scene cleared to blue with `steps` after it, on a target with a
/// 16-bit z-buffer or none.
fn clear_test_scene(api: Api, zbuffer: bool, steps: Vec<Step>) -> Scene {
    Scene {
        api,
        card: None,
        clear: 0xFF00_00FF,
        zbuffer: zbuffer.then_some(1.0),
        texture: None,
        texture_alpha: false,
        texture_system: false,
        steps,
    }
}

/// IDirect3DDevice7's Clear and IDirect3DViewport3's Clear2 of a depth
/// buffer or a stencil on a target without a depth buffer fail with
/// D3DERR_ZBUFFER_NOTPRESENT and leave the target as it was, the
/// target's own clear in the same call included. DirectX 7 on Windows
/// 95 with a Voodoo3 does this with the HAL and the RGB device, and so
/// does Windows 11 (issue 616).
#[test]
fn a_clear_of_a_missing_depth_buffer_fails_and_clears_nothing() {
    for (test, api) in [("nozclear7", Api::D3D7), ("nozclear6", Api::D3D6)] {
        let steps = vec![
            Step::Clear(D3DCLEAR_TARGET | D3DCLEAR_ZBUFFER, RED),
            Step::Clear(D3DCLEAR_ZBUFFER, RED),
            Step::Clear(D3DCLEAR_TARGET | D3DCLEAR_STENCIL, RED),
        ];
        let (r, p) = clear_scene(test, &clear_test_scene(api, false, steps));
        assert_eq!(r, [D3DERR_ZBUFFER_NOTPRESENT; 3], "{test}");
        assert_rgb(&p, 0, 0, [0, 0, 0xFF], 4);
        assert_rgb(&p, 63, 63, [0, 0, 0xFF], 4);
        // The target alone clears.
        let steps = vec![Step::Clear(D3DCLEAR_TARGET, RED)];
        let (r, p) = clear_scene(test, &clear_test_scene(api, false, steps));
        assert_eq!(r, [0], "{test}");
        assert_rgb(&p, 0, 0, [0xFF, 0, 0], 4);
    }
}

/// With a depth buffer, which has no stencil bits, a clear of the
/// stencil fails with D3DERR_STENCILBUFFER_NOTPRESENT and clears
/// nothing, and one of the target and the depth buffer clears both.
#[test]
fn a_clear_of_a_missing_stencil_fails_and_clears_nothing() {
    for (test, api) in [("nostencil7", Api::D3D7), ("nostencil6", Api::D3D6)] {
        let steps = vec![Step::Clear(D3DCLEAR_TARGET | D3DCLEAR_STENCIL, RED)];
        let (r, p) = clear_scene(test, &clear_test_scene(api, true, steps));
        assert_eq!(r, [D3DERR_STENCILBUFFER_NOTPRESENT], "{test}");
        assert_rgb(&p, 0, 0, [0, 0, 0xFF], 4);
        let steps = vec![Step::Clear(D3DCLEAR_TARGET | D3DCLEAR_ZBUFFER, RED)];
        let (r, p) = clear_scene(test, &clear_test_scene(api, true, steps));
        assert_eq!(r, [0], "{test}");
        assert_rgb(&p, 0, 0, [0xFF, 0, 0], 4);
    }
}

/// A ToadForce draws stencil, and its EnumZBufferFormats gives a
/// 32-bit format with 8 bits of it. A z-buffer of that format keeps its
/// stencil bits: Clear with D3DCLEAR_STENCIL succeeds and clears all
/// three, as No One Lives Forever clears each frame, and the stencil
/// test then reads the value the Clear wrote.
#[test]
fn a_toadforce_clears_and_tests_the_stencil_of_a_z_buffer_with_one() {
    const RS_STENCILENABLE: u32 = 52;
    const RS_STENCILFUNC: u32 = 56;
    const RS_STENCILREF: u32 = 57;
    const D3DCMP_EQUAL: u32 = 3;
    let steps = vec![
        Step::Clear(D3DCLEAR_TARGET | D3DCLEAR_ZBUFFER | D3DCLEAR_STENCIL, BLUE),
        Step::State(RS_STENCILENABLE, 1),
        Step::State(RS_STENCILFUNC, D3DCMP_EQUAL),
        // The Clear wrote 0: a quad that wants 1 draws nothing.
        Step::State(RS_STENCILREF, 1),
        Step::Draw(quad(0.0, 0.0, 32.0, 64.0, 0.5, RED)),
        Step::State(RS_STENCILREF, 0),
        Step::Draw(quad(32.0, 0.0, 64.0, 64.0, 0.5, GREEN)),
    ];
    let scene = clear_test_scene(Api::D3D7, true, steps);
    let mut checked = Vec::new();
    let (exe, data) = scene_program_with(&scene, 32, true, &mut checked);
    let mut cpu = machine_toadforce("stencil_clear", exe);
    run_until_done(&mut cpu, data);
    let n = peek(&cpu, data + CALLS);
    let r = results(&cpu, data, n);
    assert!(r[1..].iter().all(|&hr| hr == 0), "results: {r:X?}");
    assert_eq!(checked.len(), 1);
    drew_on_the_card(&cpu, &scene);
    let pixel = |x: u32, y: u32| peek(&cpu, data + PIXELS + 4 * (64 * y + x)) & 0xFF_FFFF;
    assert_eq!(pixel(16, 32), 0x0000FF);
    assert_eq!(pixel(48, 32), 0x00FF00);
}

/// The viewport's older Clear, unlike Clear2, clears the target of one
/// without a depth buffer and succeeds when asked for the depth buffer
/// too, as Windows 95's DirectX 7 and Windows 11 do.
#[test]
fn the_viewports_clear_skips_a_missing_depth_buffer() {
    let steps = vec![Step::ViewportClear(D3DCLEAR_TARGET | D3DCLEAR_ZBUFFER)];
    let (r, p) = clear_scene("vpclear", &clear_test_scene(Api::D3D6, false, steps));
    assert_eq!(r, [0]);
    assert_rgb(&p, 0, 0, [0, 0, 0], 0);
    assert_rgb(&p, 63, 63, [0, 0, 0], 0);
}

/// The HAL on a Voodoo3 leaves the card's second lookup table, the one
/// Glide loads its gamma into, linear, where Glide's grSstWinOpen loads
/// 1.3. The table is the overlay's, which only Glide's display shows:
/// Direct3D's picture is the desktop, with the overlay off.
#[test]
fn direct3d_on_a_voodoo3_leaves_glides_gamma_table_linear() {
    let scene = Scene {
        api: Api::D3D7,
        card: None,
        clear: 0xFF00_0080,
        zbuffer: None,
        texture: None,
        texture_alpha: false,
        texture_system: false,
        steps: vec![Step::Draw(corners())],
    };
    let (exe, data) = scene_program(&scene);
    let mut cpu = machine("linear_gamma", exe);
    run_until_done(&mut cpu, data);
    let card = cpu.bus.voodoo.as_ref().unwrap().banshee().unwrap();
    for i in 0..256 {
        assert_eq!(card.clut[256 + i], (i * 0x01_0101) as u32, "entry {i}");
    }
    use rust_dos::voodoo::banshee::{
        VID_PROC_CFG, VIDPROC_DESKTOP_CLUT_SEL, VIDPROC_OVERLAY_ENABLE,
    };
    let cfg = card.reg(VID_PROC_CFG);
    assert_eq!(cfg & VIDPROC_OVERLAY_ENABLE, 0, "the overlay is off");
    assert_eq!(cfg & VIDPROC_DESKTOP_CLUT_SEL, 0, "the desktop's table");
}

#[test]
fn a_gouraud_shaded_triangle_blends_its_corners() {
    let scene = Scene {
        api: Api::D3D7,
        card: None,
        clear: 0,
        zbuffer: None,
        texture: None,
        texture_alpha: false,
        texture_system: false,
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
        texture_alpha: false,
        texture_system: false,
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
        texture_alpha: false,
        texture_system: false,
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
fn the_address_state_of_a_stage_sets_both_directions() {
    // D3DTSS_ADDRESS (12) is ADDRESSU and ADDRESSV at once. Giants sets
    // D3DTADDRESS_CLAMP with it for the tiles of its title picture. The
    // texture's first column and row are red and the rest is green, and
    // the quad's coordinates run from 1 to 2: wrapped, they repeat the
    // texture and its red edges, and clamped they stay on its last
    // texel.
    let texels = std::array::from_fn(|i| match (i % 8, i / 8) {
        (0, _) | (_, 0) => 0xF800,
        _ => 0x07E0,
    });
    let at = |x: f32, y: f32| Tl {
        u: 1.0 + x / 64.0,
        v: 1.0 + y / 64.0,
        ..tl(x, y, 0.5, WHITE)
    };
    let (a, b) = (at(0.0, 0.0), at(64.0, 0.0));
    let (c, d) = (at(0.0, 64.0), at(64.0, 64.0));
    let draw = |address: Option<u32>| {
        let mut steps: Vec<Step> = address.map(|a| Step::Stage(12, a)).into_iter().collect();
        steps.push(Step::Draw(vec![a, b, c, b, d, c]));
        let scene = Scene {
            api: Api::D3D7,
            card: None,
            clear: 0,
            zbuffer: None,
            texture: Some(texels),
            texture_alpha: false,
            texture_system: false,
            steps,
        };
        draw_scene("stage_address", &scene)
    };
    let wrapped = draw(None);
    assert_rgb(&wrapped, 2, 32, [255, 0, 0], 8);
    assert_rgb(&wrapped, 32, 2, [255, 0, 0], 8);
    assert_rgb(&wrapped, 32, 32, [0, 255, 0], 8);
    let clamped = draw(Some(3));
    for (x, y) in [(2, 32), (32, 2), (32, 32)] {
        assert_rgb(&clamped, x, y, [0, 255, 0], 8);
    }
}

#[test]
fn a_texel_centred_on_each_pixel_is_the_one_the_pixel_samples() {
    // Direct3D puts a pixel's centre on its integer coordinates. The 8x8
    // texture over 8x8 pixels, u and v from 0.5/8 to 8.5/8, puts each
    // texel's centre on a pixel's, as Drakan's menu sky does: the last
    // column and row sample the last texels. Sampled half a pixel
    // further on, where the card has a pixel's centre, they would sample
    // 1, which wraps to the first texels.
    let texels = std::array::from_fn(|i| match (i % 8, i / 8) {
        (0, _) | (_, 0) => 0xF800,
        _ => 0x07E0,
    });
    let at = |x: f32, y: f32| Tl {
        u: (x + 0.5) / 8.0,
        v: (y + 0.5) / 8.0,
        ..tl(x, y, 0.5, WHITE)
    };
    let (a, b) = (at(0.0, 0.0), at(8.0, 0.0));
    let (c, d) = (at(0.0, 8.0), at(8.0, 8.0));
    let scene = Scene {
        api: Api::D3D7,
        card: None,
        clear: 0,
        zbuffer: None,
        texture: Some(texels),
        texture_alpha: false,
        texture_system: false,
        steps: vec![Step::Draw(vec![a, b, c, b, d, c])],
    };
    let p = draw_scene("texel_centres", &scene);
    assert_rgb(&p, 0, 4, [255, 0, 0], 8);
    assert_rgb(&p, 4, 0, [255, 0, 0], 8);
    for (x, y) in [(1, 1), (7, 4), (4, 7), (7, 7)] {
        assert_rgb(&p, x, y, [0, 255, 0], 8);
    }
    // Nothing past the quad.
    assert_rgb(&p, 8, 4, [0, 0, 0], 8);
    assert_rgb(&p, 4, 8, [0, 0, 0], 8);
}

#[test]
fn points_and_lines_light_the_pixels_3dfxs_driver_lights() {
    // What 3dfx's own Direct3D driver (4.12.01.0666, Voodoo 3, in 86Box)
    // draws for these, read back pixel by pixel: a point lights the
    // pixel whose centre is nearest, the left or upper one when it is
    // half way between two (10.5 lights 10, 10.6 lights 11), and a line
    // along a whole coordinate lights that row or column.
    let point = |x: f32, y: f32| tl(x, y, 0.5, WHITE);
    let scene = Scene {
        api: Api::D3D7,
        card: None,
        clear: 0,
        zbuffer: None,
        texture: None,
        texture_alpha: false,
        texture_system: false,
        steps: vec![
            Step::Prims(
                D3DPT_POINTLIST,
                vec![
                    point(2.0, 2.0),
                    point(6.4, 6.4),
                    point(10.5, 10.5),
                    point(14.6, 14.6),
                ],
            ),
            Step::Prims(D3DPT_LINELIST, vec![point(20.0, 30.0), point(40.0, 30.0)]),
            Step::Prims(D3DPT_LINELIST, vec![point(50.0, 20.0), point(50.0, 40.0)]),
        ],
    };
    let p = draw_scene("points_and_lines", &scene);
    let lit = |x: usize, y: usize| p[y * 64 + x] != 0;
    for (x, y) in [(2, 2), (6, 6), (10, 10), (15, 15)] {
        assert!(lit(x, y), "pixel ({x}, {y})");
    }
    for (x, y) in [(3, 3), (7, 7), (11, 11), (14, 14)] {
        assert!(!lit(x, y), "pixel ({x}, {y}) lit");
    }
    assert!(lit(25, 30) && !lit(25, 29) && !lit(25, 31));
    assert!(lit(50, 25) && !lit(49, 25) && !lit(51, 25));
}

#[test]
fn blending_by_the_current_alpha_on_stage_0_blends_by_the_diffuse_alpha() {
    // White and blue halves, on a red quad with a diffuse alpha of 0x40:
    // a quarter of the texture and three quarters of the red, with
    // BLENDDIFFUSEALPHA and with BLENDCURRENTALPHA, whose current color
    // on stage 0 is the diffuse one.
    let texels = std::array::from_fn(|i| if i % 8 < 4 { 0xFFFF } else { 0x001F });
    for op in [TOP_BLENDDIFFUSEALPHA, TOP_BLENDCURRENTALPHA] {
        let scene = Scene {
            api: Api::D3D7,
            card: None,
            clear: 0,
            zbuffer: None,
            texture: Some(texels),
            texture_alpha: false,
            texture_system: false,
            steps: vec![
                Step::Stage(TSS_COLOROP, op),
                Step::Draw(quad(0.0, 0.0, 64.0, 64.0, 0.5, 0x40FF_0000)),
            ],
        };
        let p = draw_scene(&format!("blend_alpha{op}"), &scene);
        assert_rgb(&p, 16, 32, [255, 64, 64], 8);
        assert_rgb(&p, 48, 32, [191, 0, 64], 8);
    }
}

/// The steps that blend a scene by its alpha over the target, and give
/// stage 0 BLENDDIFFUSEALPHA and the alpha operation `op` of `a1` and
/// `a2`, with the texture factor `factor`.
fn diffuse_alpha_blend_steps(op: u32, a1: u32, a2: u32, factor: u32) -> Vec<Step> {
    vec![
        Step::State(RS_ALPHABLENDENABLE, 1),
        Step::State(RS_SRCBLEND, 5),
        Step::State(RS_DESTBLEND, 6),
        Step::State(RS_TEXTUREFACTOR, factor),
        Step::Stage(TSS_COLOROP, TOP_BLENDDIFFUSEALPHA),
        Step::Stage(TSS_ALPHAOP, op),
        Step::Stage(TSS_ALPHAARG1, a1),
        Step::Stage(TSS_ALPHAARG2, a2),
    ]
}

#[test]
fn blending_by_the_diffuse_alpha_ignores_the_texture_factor_of_the_alpha() {
    // A white texture of alpha 0x88 on a red quad, with alpha operations
    // that take the texture factor. The color blends the white and the
    // red by the diffuse alpha (#492), and then over black by the
    // stage's alpha. With a diffuse alpha of 0x40 the color is
    // (255, 64, 64), and the factor's alpha of 0xC0 would make it
    // (255, 191, 191); with 0xC0 and 0x40 it is the other way round.
    // The cases with the diffuse alpha as an argument run under an alpha
    // test too, which passes every pixel here.
    #[rustfmt::skip]
    let cases = [
        // 0x88 by 0xC0: about 0x66.
        (TOP_MODULATE, TA_TEXTURE, TA_TFACTOR, 0x40, 0xC0, false, [102, 26, 26]),
        // 0x40 by 0xC0: about 0x30.
        (TOP_MODULATE, TA_DIFFUSE, TA_TFACTOR, 0x40, 0xC0, false, [48, 12, 12]),
        (TOP_MODULATE, TA_DIFFUSE, TA_TFACTOR, 0x40, 0xC0, true, [48, 12, 12]),
        // 0xC0 by 0xC0: about 0x90.
        (TOP_MODULATE, TA_TFACTOR, TA_TFACTOR, 0x40, 0xC0, false, [145, 36, 36]),
        // 0x40 and 0xC0: 0xFF, clamped.
        (TOP_ADD, TA_DIFFUSE, TA_TFACTOR, 0x40, 0xC0, false, [255, 64, 64]),
        (TOP_ADD, TA_DIFFUSE, TA_TFACTOR, 0x40, 0xC0, true, [255, 64, 64]),
        // 0xC0 less 0x40: 0x80.
        (TOP_SUBTRACT, TA_DIFFUSE, TA_TFACTOR, 0xC0, 0x40, false, [128, 96, 96]),
        (TOP_SUBTRACT, TA_DIFFUSE, TA_TFACTOR, 0xC0, 0x40, true, [128, 96, 96]),
    ];
    for (i, (op, a1, a2, diffuse, factor, tested, want)) in cases.into_iter().enumerate() {
        let mut steps = diffuse_alpha_blend_steps(op, a1, a2, factor << 24 | 0xFF_FFFF);
        if tested {
            steps.extend([
                Step::State(RS_ALPHATESTENABLE, 1),
                Step::State(RS_ALPHAFUNC, 5),
                Step::State(RS_ALPHAREF, 0),
            ]);
        }
        let color = diffuse << 24 | 0xFF_0000;
        steps.push(Step::Draw(quad(0.0, 0.0, 64.0, 64.0, 0.5, color)));
        let scene = Scene {
            api: Api::D3D7,
            card: None,
            clear: 0,
            zbuffer: None,
            texture: Some([0x8FFF; 64]),
            texture_alpha: true,
            texture_system: false,
            steps,
        };
        let p = draw_scene(&format!("blend_diffuse_alpha_tfactor_color{i}"), &scene);
        assert_rgb(&p, 32, 32, want, 8);
    }
}

#[test]
fn blending_by_the_diffuse_alpha_keeps_the_alpha_of_the_texture_factor() {
    // The alpha operations with the texture factor under
    // BLENDDIFFUSEALPHA, white over black, which shows the alpha: the
    // texture's is 0x88, the diffuse 0x40 and the factor's 0x60.
    let cases = [
        (TOP_MODULATE, TA_TEXTURE, TA_TFACTOR, 51),
        (TOP_MODULATE, TA_DIFFUSE, TA_TFACTOR, 24),
        (TOP_MODULATE, TA_TFACTOR, TA_TFACTOR, 36),
        (TOP_ADD, TA_DIFFUSE, TA_TFACTOR, 160),
        (TOP_ADD, TA_TEXTURE, TA_TFACTOR, 232),
        (TOP_SUBTRACT, TA_TEXTURE, TA_TFACTOR, 40),
        (TOP_SUBTRACT, TA_TFACTOR, TA_DIFFUSE, 32),
        (TOP_SELECTARG1, TA_TFACTOR, TA_DIFFUSE, 96),
    ];
    for (i, (op, a1, a2, alpha)) in cases.into_iter().enumerate() {
        let mut steps = diffuse_alpha_blend_steps(op, a1, a2, 0x60FF_FFFF);
        steps.push(Step::Draw(quad(0.0, 0.0, 64.0, 64.0, 0.5, 0x40FF_FFFF)));
        let scene = Scene {
            api: Api::D3D7,
            card: None,
            clear: 0,
            zbuffer: None,
            texture: Some([0x8FFF; 64]),
            texture_alpha: true,
            texture_system: false,
            steps,
        };
        let p = draw_scene(&format!("blend_diffuse_alpha_tfactor{i}"), &scene);
        assert_rgb(&p, 32, 32, [alpha; 3], 8);
    }
}

#[test]
fn blending_by_the_diffuse_alpha_keeps_the_alpha_test_on_the_texture() {
    // A white texture of alpha 0 in its left half and 1 in its right,
    // with the alpha the texture's by the texture factor's and the
    // alpha test passing what is above 0: the left half stays out.
    let texels = std::array::from_fn(|i| if i % 8 < 4 { 0x0FFF } else { 0xFFFF });
    let mut steps = diffuse_alpha_blend_steps(TOP_MODULATE, TA_TEXTURE, TA_TFACTOR, 0x60FF_FFFF);
    steps[0] = Step::State(RS_ALPHABLENDENABLE, 0);
    steps.extend([
        Step::State(RS_ALPHATESTENABLE, 1),
        Step::State(RS_ALPHAFUNC, 5),
        Step::State(RS_ALPHAREF, 0),
        Step::Draw(quad(0.0, 0.0, 64.0, 64.0, 0.5, 0x40FF_FFFF)),
    ]);
    let scene = Scene {
        api: Api::D3D7,
        card: None,
        clear: BLUE,
        zbuffer: None,
        texture: Some(texels),
        texture_alpha: true,
        texture_system: false,
        steps,
    };
    let p = draw_scene("blend_diffuse_alpha_tfactor_test", &scene);
    assert_rgb(&p, 16, 32, [0, 0, 255], 4);
    assert_rgb(&p, 48, 32, [255, 255, 255], 8);
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
        texture_alpha: false,
        texture_system: false,
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
        texture_alpha: false,
        texture_system: false,
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
fn a_depth_fill_blt_resets_the_z_buffer_in_its_rectangle() {
    // A red quad near the eye, then a Blt that fills the left half of
    // the z-buffer with its farthest depth, as Omikron fills its
    // z-buffer each frame: a green quad behind the red one passes the
    // depth test on the left only. The Voodoo3 fills it with its 3D
    // engine's fastfill of the depth, as the HAL's Clear does.
    let scene = Scene {
        api: Api::D3D6,
        card: None,
        clear: 0,
        zbuffer: Some(1.0),
        texture: None,
        texture_alpha: false,
        texture_system: false,
        steps: vec![
            Step::State(RS_ZENABLE, 1),
            Step::Draw(quad(0.0, 0.0, 64.0, 64.0, 0.25, RED)),
            Step::DepthFill(0xFFFF, Some([0, 0, 32, 64])),
            Step::Draw(quad(0.0, 0.0, 64.0, 64.0, 0.75, GREEN)),
        ],
    };
    let p = draw_scene("depth_fill", &scene);
    assert_rgb(&p, 16, 30, [0, 255, 0], 0);
    assert_rgb(&p, 48, 30, [255, 0, 0], 0);
    // All of it, with no rectangle.
    let scene = Scene {
        steps: vec![
            Step::State(RS_ZENABLE, 1),
            Step::Draw(quad(0.0, 0.0, 64.0, 64.0, 0.25, RED)),
            Step::DepthFill(0xFFFF, None),
            Step::Draw(quad(0.0, 0.0, 64.0, 64.0, 0.75, GREEN)),
        ],
        ..scene
    };
    let p = draw_scene("depth_fill_all", &scene);
    assert_rgb(&p, 16, 30, [0, 255, 0], 0);
    assert_rgb(&p, 48, 30, [0, 255, 0], 0);
}

/// The card's fastfills over `scene`, from the machine's start.
fn fastfills(test: &str, scene: &Scene) -> u64 {
    let (exe, data) = scene_program_with(scene, 16, false, &mut Vec::new());
    let mut cpu = machine_with(test, scene.card, exe);
    run_until_done(&mut cpu, data);
    cpu.bus.voodoo.as_ref().unwrap().fastfills
}

#[test]
fn a_depth_fill_blt_is_one_fastfill_of_the_card_for_each_rectangle() {
    // The fill is the 3D engine's, as the HAL's Clear: with the CPU's
    // writes instead, the OpenGL renderer reads the rows back and draws
    // the triangles into them in software. The pixels are the same
    // either way, so the card's count of fastfills tells them apart.
    let draw = |z, color| Step::Draw(quad(0.0, 0.0, 64.0, 64.0, z, color));
    let scene = |steps| Scene {
        api: Api::D3D6,
        card: None,
        clear: 0,
        zbuffer: Some(1.0),
        texture: None,
        texture_alpha: false,
        texture_system: false,
        steps,
    };
    let plain = scene(vec![
        Step::State(RS_ZENABLE, 1),
        draw(0.25, RED),
        draw(0.75, GREEN),
    ]);
    let base = fastfills("depth_fill_count_plain", &plain);
    let tests = [
        ("depth_fill_count", Some([0, 0, 32, 64])),
        ("depth_fill_count_all", None),
    ];
    for (test, rect) in tests {
        let filled = scene(vec![
            Step::State(RS_ZENABLE, 1),
            draw(0.25, RED),
            Step::DepthFill(0xFFFF, rect),
            draw(0.75, GREEN),
        ]);
        assert_eq!(fastfills(test, &filled), base + 1, "{test}");
    }
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
            texture_alpha: false,
            texture_system: false,
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
            texture_alpha: false,
            texture_system: false,
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
        texture_alpha: false,
        texture_system: false,
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
        texture_alpha: false,
        texture_system: false,
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
            .map(|v| with_fog(v, alpha))
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
        texture_alpha: false,
        texture_system: false,
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

#[test]
fn flat_shading_keeps_each_vertex_fog() {
    // A red first vertex without fog, and fully fogged vertices of
    // another color: the face is red, fogged as the vertices say.
    let tri = vec![
        with_fog(tl(4.0, 4.0, 0.5, RED), 0xFF),
        with_fog(tl(60.0, 4.0, 0.5, BLUE), 0),
        with_fog(tl(4.0, 60.0, 0.5, BLUE), 0),
    ];
    let scene = Scene {
        api: Api::D3D7,
        card: None,
        clear: 0,
        zbuffer: None,
        texture: None,
        texture_alpha: false,
        texture_system: false,
        steps: vec![
            Step::State(RS_SHADEMODE, 1),
            Step::State(RS_FOGENABLE, 1),
            Step::State(RS_FOGCOLOR, 0x00_FF00),
            Step::Draw(tri),
        ],
    };
    let p = draw_scene("flat-fog", &scene);
    // At the first corner red, at the others the fog color, and half
    // of each between them.
    assert_rgb(&p, 5, 5, [255, 0, 0], 24);
    assert_rgb(&p, 56, 6, [0, 255, 0], 32);
    assert_rgb(&p, 6, 56, [0, 255, 0], 32);
    assert_rgb(&p, 18, 18, [128, 128, 0], 32);
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
        texture_alpha: false,
        texture_system: false,
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
fn keyed_texels_stay_out_whatever_the_alpha_on_a_voodoo2() {
    // A texture keyed on black in its left half, white in its right. The
    // stage's alpha is the diffuse one, as Moto Racer's textures without
    // alpha have it under MODULATE, or the program's alpha test lets
    // everything through: Direct3D leaves the keyed texels out all the
    // same.
    let texels = std::array::from_fn(|i| if i % 8 < 4 { 0 } else { 0xFFFF });
    let alpha_steps = [
        vec![Step::Stage(TSS_ALPHAOP, TOP_SELECTARG2)],
        vec![
            Step::State(RS_ALPHATESTENABLE, 1),
            Step::State(RS_ALPHAFUNC, 8),
        ],
    ];
    for (i, steps) in alpha_steps.into_iter().enumerate() {
        let mut all = vec![Step::Key(0), Step::State(RS_COLORKEYENABLE, 1)];
        all.extend(steps);
        all.push(Step::Draw(quad(0.0, 0.0, 64.0, 64.0, 0.5, WHITE)));
        let scene = Scene {
            api: Api::D3D7,
            card: Some(Board::Voodoo2),
            clear: 0xFF00_0080,
            zbuffer: None,
            texture: Some(texels),
            texture_alpha: false,
            texture_system: false,
            steps: all,
        };
        let p = draw_scene(&format!("card_v2_keyed{i}"), &scene);
        // The clear color where the texture is keyed, its white beside.
        assert_rgb(&p, 16, 32, [0, 0, 0x84], 4);
        assert_rgb(&p, 48, 32, [255, 255, 255], 8);
    }
}

#[test]
fn keyed_texels_blend_with_the_stages_alpha_on_a_voodoo2() {
    // The keyed texture again, blended by its alpha over blue, with a
    // diffuse alpha of 0. ALPHAOP ADD of the texture gives the texels
    // that aren't keyed alpha 1, so they come out white; the texture
    // factor's alpha selected, 0x80, mixes them half and half. The keyed
    // texels stay out either way.
    let texels = std::array::from_fn(|i| if i % 8 < 4 { 0 } else { 0xFFFF });
    let cases = [
        (vec![Step::Stage(TSS_ALPHAOP, TOP_ADD)], [255, 255, 255]),
        (
            vec![
                Step::Stage(TSS_ALPHAOP, TOP_SELECTARG1),
                Step::Stage(TSS_ALPHAARG1, TA_TFACTOR),
                Step::State(RS_TEXTUREFACTOR, 0x80FF_FFFF),
            ],
            [128, 128, 255],
        ),
    ];
    for (i, (steps, right)) in cases.into_iter().enumerate() {
        let mut all = vec![
            Step::Key(0),
            Step::State(RS_COLORKEYENABLE, 1),
            Step::State(RS_ALPHABLENDENABLE, 1),
            Step::State(RS_SRCBLEND, 5),
            Step::State(RS_DESTBLEND, 6),
        ];
        all.extend(steps);
        all.push(Step::Draw(quad(0.0, 0.0, 64.0, 64.0, 0.5, 0x00FF_FFFF)));
        let scene = Scene {
            api: Api::D3D7,
            card: Some(Board::Voodoo2),
            clear: BLUE,
            zbuffer: None,
            texture: Some(texels),
            texture_alpha: false,
            texture_system: false,
            steps: all,
        };
        let p = draw_scene(&format!("card_v2_keyed_blend{i}"), &scene);
        assert_rgb(&p, 16, 32, [0, 0, 255], 4);
        assert_rgb(&p, 48, 32, right, 12);
    }
}

#[test]
fn the_3dfx_device_draws_on_a_voodoo_graphics() {
    card_scene("card_v1", Board::Standard);
}

/// Where the card polling program counts its polls and keeps the first
/// one's result and the last one's.
const POLLS: u32 = 0x9A0;
const FIRST_POLL: u32 = 0x9A4;
const LAST_POLL: u32 = 0x9A8;
const DDERR_WASSTILLDRAWING: u32 = 0x8876_021C;
const SURF_GET_FLIP_STATUS: u32 = 18;
const DDGFS_ISFLIPDONE: u32 = 2;

/// A flip chain on the Voodoo2's 3dfx device and a flip, whose swap
/// waits for the card's next retrace; then, again and again with
/// nothing in between until it succeeds, Flip without DDFLIP_WAIT, or
/// GetFlipStatus of the back buffer when `status`.
fn card_polling_program(status: bool) -> (Vec<u8>, u32) {
    let mut d = data();
    guid(
        &mut d,
        CARD_GUID,
        0x472B_EA00,
        0x40DF,
        0x11D1,
        [0xA9, 0xDF, 0x00, 0x60, 0x97, 0xC2, 0xED, 0xB2],
    );
    put32(&mut d, DESC, 124);
    put32(&mut d, DESC + 0x04, DDSD_CAPS | DDSD_BACKBUFFERCOUNT);
    put32(&mut d, DESC + 0x14, 1);
    let caps = DDSCAPS_PRIMARYSURFACE | DDSCAPS_FLIP | DDSCAPS_COMPLEX | DDSCAPS_3DDEVICE;
    put32(&mut d, DESC + 0x68, caps);
    put32(&mut d, BACK_CAPS, DDSCAPS_BACKBUFFER);
    program(&[USER32, DDRAW], &d, |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c, true, true)?;
        c.com(
            a,
            l,
            DD,
            DD_CREATE_SURFACE,
            &[Arg::At(DESC), Arg::At(FRONT), Arg::I(0)],
        )?;
        c.com(
            a,
            l,
            FRONT,
            SURF_GET_ATTACHED_SURFACE,
            &[Arg::At(BACK_CAPS), Arg::At(TARGET)],
        )?;
        c.com(a, l, FRONT, SURF_FLIP, &[Arg::I(0), Arg::I(0)])?;
        let mut again = a.create_label();
        a.set_label(&mut again)?;
        a.inc(dword_ptr(l.at(POLLS)))?;
        let (object, method) = if status {
            push(a, l, &[Arg::I(DDGFS_ISFLIPDONE)])?;
            (TARGET, SURF_GET_FLIP_STATUS)
        } else {
            push(a, l, &[Arg::I(0), Arg::I(0)])?;
            (FRONT, SURF_FLIP)
        };
        a.mov(eax, dword_ptr(l.at(object)))?;
        a.push(eax)?;
        a.mov(eax, dword_ptr(eax))?;
        a.call(dword_ptr(eax + 4 * method))?;
        let mut kept = a.create_label();
        a.cmp(dword_ptr(l.at(POLLS)), 1)?;
        a.jne(kept)?;
        a.mov(dword_ptr(l.at(FIRST_POLL)), eax)?;
        a.set_label(&mut kept)?;
        a.cmp(eax, DDERR_WASSTILLDRAWING as i32)?;
        a.je(again)?;
        a.mov(dword_ptr(l.at(LAST_POLL)), eax)?;
        finish(a, l)
    })
}

#[test]
fn flip_polled_on_a_voodoo2_waits_for_the_swap() {
    // The swap is pending until the card's next retrace, and each call
    // of Flip fails until then. A loop that only polls waits for that
    // retrace at its 16th call, which flips, instead of making a call
    // for each pass until then.
    let (exe, data) = card_polling_program(false);
    let mut cpu = machine_with("card_flip_polling", Some(Board::Voodoo2), exe);
    run_until_done(&mut cpu, data);
    assert_eq!(results(&cpu, data, 7)[1..], [0; 6]);
    assert_eq!(peek(&cpu, data + FIRST_POLL), DDERR_WASSTILLDRAWING);
    assert_eq!(peek(&cpu, data + LAST_POLL), 0);
    assert_eq!(peek(&cpu, data + POLLS), 16, "calls of Flip");
}

#[test]
fn flip_status_on_a_voodoo2_follows_the_swap() {
    // GetFlipStatus says the flip is still to show while the card's swap
    // waits for its retrace, and a loop that only asks waits for it.
    let (exe, data) = card_polling_program(true);
    let mut cpu = machine_with("card_flip_status", Some(Board::Voodoo2), exe);
    run_until_done(&mut cpu, data);
    assert_eq!(results(&cpu, data, 7)[1..], [0; 6]);
    assert_eq!(peek(&cpu, data + FIRST_POLL), DDERR_WASSTILLDRAWING);
    assert_eq!(peek(&cpu, data + LAST_POLL), 0);
    assert_eq!(peek(&cpu, data + POLLS), 16, "calls of GetFlipStatus");
}

const SURF_GET_DC: u32 = 17;
const SURF_RELEASE_DC: u32 = 26;
/// Where the GDI program keeps the primary surface's DC.
const SURFACE_DC: u32 = 0x9B0;
const BLACKNESS: u32 = 0x0000_0042;
const DSTINVERT: u32 = 0x0055_0009;
const GDI32: (&str, &[&str]) = ("GDI32.DLL", &["PatBlt"]);
const PAT_BLT: usize = 3;

#[test]
fn gdi_draws_into_the_voodoo2s_primary_through_its_dc() {
    // Adrenix's Direct3D build draws its video and menus with GDI into
    // the DC of the primary surface on the card (issue 910): GDI reads
    // and writes the card's frame buffer through the DIB section on its
    // linear frame buffer, which takes words and dwords, not bytes. An
    // 8x4 rectangle made black, then 3x2 pixels of it inverted to white
    // from an odd column, which starts on a word and not a dword.
    let mut d = data();
    guid(
        &mut d,
        CARD_GUID,
        0x472B_EA00,
        0x40DF,
        0x11D1,
        [0xA9, 0xDF, 0x00, 0x60, 0x97, 0xC2, 0xED, 0xB2],
    );
    put32(&mut d, DESC, 124);
    put32(&mut d, DESC + 0x04, DDSD_CAPS);
    put32(&mut d, DESC + 0x68, DDSCAPS_PRIMARYSURFACE);
    put32(&mut d, LOCK_DESC, 124);
    let (exe, data) = program(&[USER32, DDRAW, GDI32], &d, |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c, true, true)?;
        c.com(
            a,
            l,
            DD,
            DD_CREATE_SURFACE,
            &[Arg::At(DESC), Arg::At(TARGET), Arg::I(0)],
        )?;
        c.com(a, l, TARGET, SURF_GET_DC, &[Arg::At(SURFACE_DC)])?;
        let dc = Arg::Val(SURFACE_DC);
        let fill = [dc, Arg::I(0), Arg::I(0), Arg::I(8), Arg::I(4)];
        c.api(a, l, PAT_BLT, &[&fill[..], &[Arg::I(BLACKNESS)]].concat())?;
        let invert = [dc, Arg::I(3), Arg::I(1), Arg::I(3), Arg::I(2)];
        c.api(a, l, PAT_BLT, &[&invert[..], &[Arg::I(DSTINVERT)]].concat())?;
        c.com(a, l, TARGET, SURF_RELEASE_DC, &[dc])?;
        read_target(a, l, &mut c, 16)?;
        finish(a, l)
    });
    let mut cpu = machine_with("card_gdi_primary", Some(Board::Voodoo2), exe);
    run_until_done(&mut cpu, data);
    let n = peek(&cpu, data + CALLS);
    // DirectDraw's calls succeed, and each PatBlt gives TRUE.
    assert_eq!(
        results(&cpu, data, n)[1..],
        [0, 0, 0, 0, 0, 1, 1, 0, 0],
        "results"
    );
    let pixel = |x: u32, y: u32| {
        let at = data + PIXELS + 2 * (64 * y + x);
        (peek(&cpu, at & !3) >> (8 * (at & 2))) as u16
    };
    for y in 0..4 {
        for x in 0..8 {
            let inverted = (3..6).contains(&x) && (1..3).contains(&y);
            let want = if inverted { 0xFFFF } else { 0 };
            assert_eq!(pixel(x, y), want, "pixel ({x}, {y})");
        }
    }
}

#[test]
fn the_specular_color_adds_to_textured_and_untextured_triangles() {
    let gray = |textured: bool| Scene {
        api: Api::D3D7,
        card: None,
        clear: 0,
        zbuffer: Some(1.0),
        texture: textured.then_some([0xFFFF; 64]),
        texture_alpha: false,
        texture_system: false,
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

// Depth buffers on textures drawn into (issue 877). The expected values
// are what DirectX 7 does on Windows 98 SE with 3dfx's Voodoo3 drivers
// 4.11.01.1151 and 4.12.01.0666.

const DDERR_CANNOTATTACHSURFACE: u32 = 0x8876_000A;
const SURF_DELETE_ATTACHED_SURFACE: u32 = 8;
const DEV7_SET_RENDER_TARGET: u32 = 8;
const DEV7_GET_RENDER_STATE: u32 = 21;
const D3DCMP_LESSEQUAL: u32 = 4;

const DDSCAPS_SYSTEMMEMORY: u32 = 0x800;

/// The DDSURFACEDESC2s of a 32x32 texture to draw into, of a 16x16
/// depth buffer, of a 32x32 off-screen surface to draw into and of a
/// 32x32 texture in system memory (in TEX_DESC's place, which this
/// program doesn't use otherwise); their interfaces; a 32x32 viewport;
/// ZENABLE as GetRenderState read it on the texture and on the
/// off-screen surface; the draws' vertices; the texture's pixels.
const RT_TEX_DESC: u32 = 0xA00;
const RT_ZSMALL_DESC: u32 = 0xA80;
const RT_OFF_DESC: u32 = 0xB00;
const RT_SYS_DESC: u32 = TEX_DESC;
const RT_TEX: u32 = 0xB80;
const RT_ZSMALL: u32 = 0xB84;
const RT_OFF: u32 = 0xB88;
const RT_SYS: u32 = 0xB8C;
const RT_VIEW: u32 = 0xB90;
const RT_ZENABLE_TEX: u32 = 0xBB0;
const RT_ZENABLE_OFF: u32 = 0xBB4;
const RT_VERTICES: u32 = 0xC00;
const RT_PIXELS: u32 = 0x4000;

/// Where `rt_z_program`'s calls are in its results.
#[derive(Default)]
struct RtZCalls {
    /// AddAttachedSurface of the 64x64 and of the 16x16 depth buffer to
    /// the 32x32 off-screen surface.
    offscreen_larger: u32,
    offscreen_smaller: u32,
    /// AddAttachedSurface of the 16x16 depth buffer to the 32x32
    /// texture in system memory.
    system_smaller: u32,
    /// AddAttachedSurface of the 16x16 depth buffer to the 32x32
    /// texture, then of the 64x64 one while the 16x16 one is on it.
    texture_smaller: u32,
    texture_second: u32,
    /// AddAttachedSurface of the 64x64 depth buffer to the 64x64
    /// target, then to the texture: on both at once.
    target_shared: u32,
    texture_shared: u32,
    /// The texture's Clear of its target and depth, and of its depth
    /// alone; the off-screen surface's Clear of a depth it doesn't have.
    texture_clear: u32,
    texture_clear_z: u32,
    offscreen_clear_z: u32,
}

/// Direct3D 7 on the Voodoo3's HAL: a 64x64 target with a 64x64 depth
/// buffer, which a 32x32 texture to draw into shares. With ZENABLE on,
/// the device switches to the texture, clears it red with depth 1 and
/// draws a green quad at depth 0.5 over its right half, then clears the
/// depth alone to 0 and draws one over the left half. Back on the
/// 64x64 target, cleared blue with depth 1 before, it draws a green
/// quad at depth 0.5 over all of it: where the texture's clear left
/// depth 0, the target stays blue. Last it switches to a 32x32
/// off-screen surface without a depth buffer.
fn rt_z_program(calls: &mut RtZCalls) -> (Vec<u8>, u32) {
    let mut d = data();
    target_desc(&mut d, DESC, 124, 64, 64);
    put32(&mut d, LOCK_DESC, 124);
    put32(&mut d, TEX_LOCK_DESC, 124);
    // The depth buffers: 64x64 and 16x16.
    let flags = DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT | DDSD_PIXELFORMAT;
    for (at, size) in [(Z_DESC, 64), (RT_ZSMALL_DESC, 16)] {
        put32(&mut d, at, 124);
        put32(&mut d, at + 0x04, flags);
        put32(&mut d, at + 0x08, size);
        put32(&mut d, at + 0x0C, size);
        put32(&mut d, at + 0x48, 32);
        put32(&mut d, at + 0x4C, DDPF_ZBUFFER);
        put32(&mut d, at + 0x54, 16);
        put32(&mut d, at + 0x5C, 0xFFFF);
        put32(&mut d, at + 0x68, DDSCAPS_ZBUFFER | DDSCAPS_VIDEOMEMORY);
    }
    // The 32x32 texture to draw into, 5-6-5 as Zanzarah's, and one in
    // system memory.
    let video = DDSCAPS_TEXTURE | DDSCAPS_3DDEVICE | DDSCAPS_VIDEOMEMORY;
    let system = DDSCAPS_TEXTURE | DDSCAPS_SYSTEMMEMORY;
    for (at, caps) in [(RT_TEX_DESC, video), (RT_SYS_DESC, system)] {
        put32(&mut d, at, 124);
        put32(&mut d, at + 0x04, flags);
        put32(&mut d, at + 0x08, 32);
        put32(&mut d, at + 0x0C, 32);
        put32(&mut d, at + 0x48, 32);
        put32(&mut d, at + 0x4C, DDPF_RGB);
        put32(&mut d, at + 0x54, 16);
        put32(&mut d, at + 0x58, 0xF800);
        put32(&mut d, at + 0x5C, 0x07E0);
        put32(&mut d, at + 0x60, 0x001F);
        put32(&mut d, at + 0x68, caps);
    }
    target_desc(&mut d, RT_OFF_DESC, 124, 32, 32);
    // D3DVIEWPORT7s: 64x64 and 32x32, depths 0 to 1.
    put32(&mut d, VIEW + 8, 64);
    put32(&mut d, VIEW + 12, 64);
    put_f32s(&mut d, VIEW + 16, &[0.0, 1.0]);
    put32(&mut d, RT_VIEW + 8, 32);
    put32(&mut d, RT_VIEW + 12, 32);
    put_f32s(&mut d, RT_VIEW + 16, &[0.0, 1.0]);
    // The quads: the texture's left and right halves, all of the target.
    let quads = [
        quad(0.0, 0.0, 16.0, 32.0, 0.5, GREEN),
        quad(16.0, 0.0, 32.0, 32.0, 0.5, GREEN),
        quad(0.0, 0.0, 64.0, 64.0, 0.5, GREEN),
    ];
    let mut at = RT_VERTICES;
    let mut quad_at = Vec::new();
    for q in &quads {
        quad_at.push(at);
        for v in q {
            put_f32s(&mut d, at, &[v.x, v.y, v.z, v.rhw]);
            put32(&mut d, at + 16, v.diffuse);
            put32(&mut d, at + 20, v.specular);
            put_f32s(&mut d, at + 24, &[v.u, v.v]);
            at += 32;
        }
    }
    assert!(at <= RESULTS, "too many vertices");
    program(&[USER32, DDRAW], &d, |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c, true, false)?;
        let args = [Arg::At(IID_D3D7), Arg::At(D3D)];
        c.com(a, l, DD, QUERY_INTERFACE, &args)?;
        for (desc, out) in [
            (DESC, TARGET),
            (Z_DESC, ZBUF),
            (RT_TEX_DESC, RT_TEX),
            (RT_ZSMALL_DESC, RT_ZSMALL),
            (RT_OFF_DESC, RT_OFF),
            (RT_SYS_DESC, RT_SYS),
        ] {
            let args = [Arg::At(desc), Arg::At(out), Arg::I(0)];
            c.com(a, l, DD, DD_CREATE_SURFACE, &args)?;
        }
        let attach = |a: &mut CodeAssembler, c: &mut Calls, to: u32, z: u32| {
            c.com(a, l, to, SURF_ADD_ATTACHED_SURFACE, &[Arg::Val(z)])
        };
        calls.offscreen_larger = c.next;
        attach(a, &mut c, RT_OFF, ZBUF)?;
        calls.offscreen_smaller = c.next;
        attach(a, &mut c, RT_OFF, RT_ZSMALL)?;
        calls.system_smaller = c.next;
        attach(a, &mut c, RT_SYS, RT_ZSMALL)?;
        calls.texture_smaller = c.next;
        attach(a, &mut c, RT_TEX, RT_ZSMALL)?;
        calls.texture_second = c.next;
        attach(a, &mut c, RT_TEX, ZBUF)?;
        let args = [Arg::I(0), Arg::Val(RT_ZSMALL)];
        c.com(a, l, RT_TEX, SURF_DELETE_ATTACHED_SURFACE, &args)?;
        calls.target_shared = c.next;
        attach(a, &mut c, TARGET, ZBUF)?;
        calls.texture_shared = c.next;
        attach(a, &mut c, RT_TEX, ZBUF)?;
        let args = [Arg::At(IID_HAL), Arg::Val(TARGET), Arg::At(DEVICE)];
        c.com(a, l, D3D, D3D7_CREATE_DEVICE, &args)?;
        c.com(a, l, DEVICE, DEV7_SET_VIEWPORT, &[Arg::At(VIEW)])?;
        let state = |a: &mut CodeAssembler, c: &mut Calls, s: u32, v: u32| {
            c.com(a, l, DEVICE, DEV7_SET_RENDER_STATE, &[Arg::I(s), Arg::I(v)])
        };
        state(a, &mut c, RS_ZENABLE, 1)?;
        state(a, &mut c, RS_ZFUNC, D3DCMP_LESSEQUAL)?;
        let clear = |a: &mut CodeAssembler, c: &mut Calls, flags: u32, color: u32, z: f32| {
            let args = [
                Arg::I(0),
                Arg::I(0),
                Arg::I(flags),
                Arg::I(color),
                Arg::I(z.to_bits()),
                Arg::I(0),
            ];
            c.com(a, l, DEVICE, DEV7_CLEAR, &args)
        };
        let draw = |a: &mut CodeAssembler, c: &mut Calls, at: u32| {
            c.com(a, l, DEVICE, DEV7_BEGIN_SCENE, &[])?;
            let args = [
                Arg::I(D3DPT_TRIANGLELIST),
                Arg::I(TL_FVF),
                Arg::At(at),
                Arg::I(6),
                Arg::I(0),
            ];
            c.com(a, l, DEVICE, DEV7_DRAW_PRIMITIVE, &args)?;
            c.com(a, l, DEVICE, DEV7_END_SCENE, &[])
        };
        let set_target = |a: &mut CodeAssembler, c: &mut Calls, target: u32| {
            let args = [Arg::Val(target), Arg::I(0)];
            c.com(a, l, DEVICE, DEV7_SET_RENDER_TARGET, &args)
        };
        let zenable = |a: &mut CodeAssembler, c: &mut Calls, out: u32| {
            let args = [Arg::I(RS_ZENABLE), Arg::At(out)];
            c.com(a, l, DEVICE, DEV7_GET_RENDER_STATE, &args)
        };
        clear(a, &mut c, D3DCLEAR_TARGET | D3DCLEAR_ZBUFFER, BLUE, 1.0)?;
        // The texture, sharing the depth buffer.
        set_target(a, &mut c, RT_TEX)?;
        c.com(a, l, DEVICE, DEV7_SET_VIEWPORT, &[Arg::At(RT_VIEW)])?;
        zenable(a, &mut c, RT_ZENABLE_TEX)?;
        calls.texture_clear = c.next;
        clear(a, &mut c, D3DCLEAR_TARGET | D3DCLEAR_ZBUFFER, RED, 1.0)?;
        draw(a, &mut c, quad_at[1])?;
        calls.texture_clear_z = c.next;
        clear(a, &mut c, D3DCLEAR_ZBUFFER, 0, 0.0)?;
        draw(a, &mut c, quad_at[0])?;
        // Back to the target.
        set_target(a, &mut c, TARGET)?;
        c.com(a, l, DEVICE, DEV7_SET_VIEWPORT, &[Arg::At(VIEW)])?;
        draw(a, &mut c, quad_at[2])?;
        // The off-screen surface without a depth buffer.
        set_target(a, &mut c, RT_OFF)?;
        zenable(a, &mut c, RT_ZENABLE_OFF)?;
        calls.offscreen_clear_z = c.next;
        clear(a, &mut c, D3DCLEAR_ZBUFFER, 0, 1.0)?;
        // The texture's pixels, then the target's.
        let args = [
            Arg::I(0),
            Arg::At(TEX_LOCK_DESC),
            Arg::I(DDLOCK_WAIT),
            Arg::I(0),
        ];
        c.com(a, l, RT_TEX, SURF_LOCK, &args)?;
        a.mov(esi, dword_ptr(l.at(TEX_LOCK_DESC + 0x24)))?;
        a.mov(edi, l.at(RT_PIXELS) as u32)?;
        a.mov(edx, 32)?;
        a.cld()?;
        let mut row = a.create_label();
        a.set_label(&mut row)?;
        a.mov(ecx, 16)?;
        a.rep().movsd()?;
        a.add(esi, dword_ptr(l.at(TEX_LOCK_DESC + 0x10)))?;
        a.sub(esi, 64)?;
        a.dec(edx)?;
        a.jnz(row)?;
        c.com(a, l, RT_TEX, SURF_UNLOCK, &[Arg::I(0)])?;
        read_target(a, l, &mut c, 16)?;
        finish(a, l)
    })
}

/// `rt_z_program` run to its end: the machine, where its data is, its
/// calls' results and where some of them are.
fn run_rt_z(test: &str) -> (Cpu, u32, Vec<u32>, RtZCalls) {
    let mut calls = RtZCalls::default();
    let (exe, data) = rt_z_program(&mut calls);
    let mut cpu = machine(test, exe);
    run_until_done(&mut cpu, data);
    let n = peek(&cpu, data + CALLS);
    let r = results(&cpu, data, n);
    (cpu, data, r, calls)
}

/// A texture in video memory takes a depth buffer of any size, smaller
/// or larger than itself, and one also attached to another surface; an
/// off-screen surface or a texture in system memory takes only one of
/// its own size. A surface takes one depth buffer at a time.
#[test]
fn a_texture_takes_a_depth_buffer_of_any_size() {
    let (_, _, r, calls) = run_rt_z("rtz_attach");
    let at = |i: u32| r[i as usize];
    assert_eq!(at(calls.offscreen_larger), DDERR_CANNOTATTACHSURFACE);
    assert_eq!(at(calls.offscreen_smaller), DDERR_CANNOTATTACHSURFACE);
    assert_eq!(at(calls.system_smaller), DDERR_CANNOTATTACHSURFACE);
    assert_eq!(at(calls.texture_smaller), 0);
    assert_eq!(at(calls.texture_second), DDERR_CANNOTATTACHSURFACE);
    assert_eq!(at(calls.target_shared), 0);
    assert_eq!(at(calls.texture_shared), 0);
}

/// A texture sharing the target's larger depth buffer, as Zanzarah's
/// share the screen's: its Clear of the depth succeeds and fills only
/// the depth buffer's pixels under the texture, and its draws are depth
/// tested. SetRenderTarget leaves ZENABLE on, on a surface without a
/// depth buffer too, whose Clear of the depth fails.
#[test]
fn a_texture_draws_with_a_larger_shared_depth_buffer() {
    let (cpu, data, r, calls) = run_rt_z("rtz_draw");
    let refused = [
        calls.offscreen_larger,
        calls.offscreen_smaller,
        calls.system_smaller,
        calls.texture_second,
        calls.offscreen_clear_z,
    ];
    for i in (1..r.len() as u32).filter(|i| !refused.contains(i)) {
        assert_eq!(r[i as usize], 0, "call {i}: {r:X?}");
    }
    let clear_z = r[calls.offscreen_clear_z as usize];
    assert_eq!(clear_z, D3DERR_ZBUFFER_NOTPRESENT);
    assert_eq!(peek(&cpu, data + RT_ZENABLE_TEX), 1);
    assert_eq!(peek(&cpu, data + RT_ZENABLE_OFF), 1);
    let pixel = |base: u32, width: u32, x: u32, y: u32| {
        let at = data + base + 2 * (width * y + x);
        (peek(&cpu, at & !3) >> (8 * (at & 2))) as u16
    };
    // The texture: the left quad behind depth 0, the right one drawn.
    assert_eq!(pixel(RT_PIXELS, 32, 4, 16), 0xF800);
    assert_eq!(pixel(RT_PIXELS, 32, 28, 16), 0x07E0);
    // The target: the quad behind the depth the texture's clear left in
    // the top left 32x32, drawn elsewhere.
    assert_eq!(pixel(PIXELS, 64, 4, 4), 0x001F);
    assert_eq!(pixel(PIXELS, 64, 28, 28), 0x001F);
    assert_eq!(pixel(PIXELS, 64, 40, 10), 0x07E0);
    assert_eq!(pixel(PIXELS, 64, 10, 40), 0x07E0);
}

// The 3D trace (`--trace-3d`).

/// The 3D trace at `path`: its events in order, without their numbers,
/// and the count of each.
pub(super) fn read_trace(path: &Path) -> (Vec<String>, HashMap<String, u64>) {
    let text = fs::read_to_string(path).unwrap();
    let events = text
        .lines()
        .map(|l| l.trim_start().split_once(' ').unwrap().1.to_string())
        .collect();
    let counts = fs::read_to_string(rust_dos::win32::trace3d::counts_path(path)).unwrap();
    let counts = counts
        .lines()
        .map(|l| {
            let (n, key) = l.split_once('\t').unwrap();
            (key.to_string(), n.parse().unwrap())
        })
        .collect();
    (events, counts)
}

/// The one event that starts with `start`, and its count.
#[track_caller]
pub(super) fn event<'a>(
    events: &'a [String],
    counts: &HashMap<String, u64>,
    start: &str,
) -> (&'a str, u64) {
    let found: Vec<&String> = events.iter().filter(|e| e.starts_with(start)).collect();
    assert_eq!(found.len(), 1, "{start:?} in {events:#?}");
    (found[0], counts[found[0]])
}

/// The scene the trace tests draw: a z-buffer and a texture, the W
/// buffer asked for, and two draws with the texture modulated.
fn traced_scene() -> Scene {
    Scene {
        api: Api::D3D7,
        card: None,
        clear: BLUE,
        zbuffer: Some(1.0),
        texture: Some([0xF800; 64]),
        texture_alpha: false,
        texture_system: false,
        steps: vec![
            Step::State(RS_ZENABLE, 2),
            Step::Stage(TSS_COLOROP, TOP_MODULATE),
            Step::Draw(corners()),
            Step::Draw(corners()),
        ],
    }
}

/// `exe` run to its end in the folder of `test`, with the 3D trace into
/// `t.trace` there when `trace`: the machine, and the trace's path.
fn run_traced(test: &str, exe: Vec<u8>, data: u32, trace: bool) -> (Cpu, PathBuf) {
    let mut cpu = machine(test, exe);
    let path = PathBuf::from("target/test_win32_d3d")
        .join(test)
        .join("t.trace");
    if trace {
        rust_dos::win32::trace3d::start(&mut cpu, &path).unwrap();
    }
    run_until_done(&mut cpu, data);
    rust_dos::win32::trace3d::flush(&mut cpu);
    (cpu, path)
}

#[test]
fn the_3d_trace_names_the_states_and_the_surfaces_of_each_call() {
    let (exe, data) = scene_program(&traced_scene());
    let (_, path) = run_traced("trace3d_scene", exe, data, true);
    let (events, counts) = read_trace(&path);
    let made = event(
        &events,
        &counts,
        "DDRAW.IDirectDraw_CreateSurface [64x64 OFFSCREENPLAIN|3DDEVICE] made [64x64 R5G6B5 ",
    );
    assert!(made.0.ends_with("] = OK"), "{made:?}");
    let attach = event(&events, &counts, "DDRAW.Surface_AddAttachedSurface [64x64 R5G6B5 ").0;
    let z = " gets [64x64 Z16 VIDEOMEMORY|ZBUFFER|LOCALVIDMEM] = OK";
    assert!(attach.ends_with(z), "{attach}");
    let clear = event(&events, &counts, "DDRAW.IDirect3DDevice7_Clear(").0;
    let start = "DDRAW.IDirect3DDevice7_Clear(rects=0,TARGET|ZBUFFER color=0xFF0000FF z=1.0 \
                 stencil=0) target=[64x64 R5G6B5 ";
    assert!(clear.starts_with(start), "{clear}");
    let z = " depth=[64x64 Z16 VIDEOMEMORY|ZBUFFER|LOCALVIDMEM] = OK";
    assert!(clear.ends_with(z), "{clear}");
    let states = [
        "DDRAW.IDirect3DDevice7_SetRenderState(ZENABLE,USEW) = OK",
        "DDRAW.IDirect3DDevice7_SetTextureStageState(0,COLOROP,MODULATE) = OK",
        "DDRAW.IDirect3DDevice7_SetTexture(0,R5G6B5) = OK",
    ];
    for s in states {
        assert_eq!(event(&events, &counts, s), (s, 1));
    }
    let draw =
        "DDRAW.IDirect3DDevice7_DrawPrimitive(TRIANGLELIST,XYZRHW|DIFFUSE|SPECULAR|TEX1) = OK";
    assert_eq!(event(&events, &counts, draw), (draw, 2));
    // The states the two draws used: once, counted twice.
    let (draw, n) = event(&events, &counts, "draw v7 ");
    assert_eq!(n, 2);
    let start = "draw v7 fvf=XYZRHW|DIFFUSE|SPECULAR|TEX1 z=USEW,1,LESSEQUAL ";
    assert!(draw.starts_with(start), "{draw}");
    assert!(
        draw.contains(" | s0 tex=1 c=MODULATE(TEXTURE,CURRENT) "),
        "{draw}"
    );
    assert!(!events.iter().any(|e| e.starts_with("process ")));
}

#[test]
fn the_3d_trace_names_the_vertex_format_of_an_idirect3ddevice3_draw() {
    // IDirect3DDevice3 shares IDirect3DDevice2's DrawPrimitive, whose
    // vertex type is a D3DVERTEXTYPE there and an FVF code here.
    let scene = Scene {
        api: Api::D3D6,
        steps: vec![Step::Draw(corners()), Step::Draw(corners())],
        ..traced_scene()
    };
    let (exe, data) = scene_program(&scene);
    let (_, path) = run_traced("trace3d_device3", exe, data, true);
    let (events, counts) = read_trace(&path);
    let draw =
        "DDRAW.IDirect3DDevice2_DrawPrimitive(TRIANGLELIST,XYZRHW|DIFFUSE|SPECULAR|TEX1) = OK";
    assert_eq!(event(&events, &counts, draw), (draw, 2));
}

#[test]
fn the_3d_trace_changes_nothing_and_without_it_no_file_is_written() {
    let draw = |test: &str, trace: bool| {
        let (exe, data) = scene_program(&traced_scene());
        let (cpu, path) = run_traced(test, exe, data, trace);
        let n = peek(&cpu, data + CALLS);
        let pixels: Vec<u32> = (0..32 * 64)
            .map(|i| peek(&cpu, data + PIXELS + 4 * i))
            .collect();
        let mut files: Vec<String> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        (results(&cpu, data, n), pixels, files)
    };
    let (r_off, pixels_off, files_off) = draw("trace3d_off", false);
    let (r_on, pixels_on, files_on) = draw("trace3d_on", true);
    assert!(r_off[1..].iter().all(|&hr| hr == 0), "results: {r_off:X?}");
    assert_eq!(r_on, r_off);
    assert_eq!(pixels_on, pixels_off);
    assert_eq!(files_off, ["T.EXE"]);
    assert_eq!(files_on, ["T.EXE", "t.trace", "t.trace.counts"]);
}

#[test]
fn the_3d_trace_tells_process_vertices_from_draws() {
    let (exe, data) = process_program();
    let (_, path) = run_traced("trace3d_process", exe, data, true);
    let (events, counts) = read_trace(&path);
    let (process, n) = event(&events, &counts, "process v7 ");
    assert!(
        process.starts_with("process v7 fvf=XYZ|NORMAL z="),
        "{process}"
    );
    assert!(process.contains(" lighting=1/1 "), "{process}");
    assert_eq!(n, 1);
    let (draw, n) = event(&events, &counts, "draw v7 ");
    assert!(draw.starts_with("draw v7 fvf=XYZRHW|DIFFUSE z="), "{draw}");
    assert_eq!(n, 1);
    let call = "DDRAW.IDirect3DVertexBuffer_ProcessVertices(op=0x401,flags=0x0) = OK";
    assert_eq!(event(&events, &counts, call), (call, 1));
}

const D3D7_ENUM_ZBUFFER_FORMATS: u32 = 6;
const DEV7_ENUM_TEXTURE_FORMATS: u32 = 4;
/// Where the program keeps its two callbacks' addresses.
const ENUM_OK: u32 = 0x2C;
const ENUM_CANCEL: u32 = 0x30;

#[test]
fn the_3d_trace_has_what_an_enumeration_offered_and_what_the_callback_said() {
    let mut d = data();
    target_desc(&mut d, DESC, 124, 64, 64);
    let (exe, data) = program(&[USER32, DDRAW], &d, |a, l| {
        let mut c = Calls { next: 0 };
        // The callbacks (item, context): one goes on, the other stops.
        // Each CALL over one pushes its address.
        let mut after_ok = a.create_label();
        a.call(after_ok)?;
        a.mov(eax, 1)?;
        a.ret_1(8)?;
        a.set_label(&mut after_ok)?;
        a.pop(dword_ptr(l.at(ENUM_OK)))?;
        let mut after_cancel = a.create_label();
        a.call(after_cancel)?;
        a.xor(eax, eax)?;
        a.ret_1(8)?;
        a.set_label(&mut after_cancel)?;
        a.pop(dword_ptr(l.at(ENUM_CANCEL)))?;
        start(a, l, &mut c, true, false)?;
        let args = [Arg::At(IID_D3D7), Arg::At(D3D)];
        c.com(a, l, DD, QUERY_INTERFACE, &args)?;
        let args = [Arg::At(IID_HAL), Arg::Val(ENUM_OK), Arg::I(0)];
        c.com(a, l, D3D, D3D7_ENUM_ZBUFFER_FORMATS, &args)?;
        let args = [Arg::At(DESC), Arg::At(TARGET), Arg::I(0)];
        c.com(a, l, DD, DD_CREATE_SURFACE, &args)?;
        let args = [Arg::At(IID_HAL), Arg::Val(TARGET), Arg::At(DEVICE)];
        c.com(a, l, D3D, D3D7_CREATE_DEVICE, &args)?;
        let args = [Arg::Val(ENUM_CANCEL), Arg::I(0)];
        c.com(a, l, DEVICE, DEV7_ENUM_TEXTURE_FORMATS, &args)?;
        finish(a, l)
    });
    let (cpu, path) = run_traced("trace3d_enum", exe, data, true);
    let r = results(&cpu, data, 9);
    assert!(r[1..].iter().all(|&hr| hr == 0), "results: {r:X?}");
    let (events, counts) = read_trace(&path);
    let z: Vec<&String> = events
        .iter()
        .filter(|e| e.starts_with("DDRAW.EnumZBufferFormats item "))
        .collect();
    assert!(!z.is_empty(), "{events:#?}");
    assert!(z.iter().all(|e| e.ends_with(" -> OK")), "{z:#?}");
    assert!(
        z.iter()
            .any(|e| e.as_str() == "DDRAW.EnumZBufferFormats item Z16 -> OK"),
        "{z:#?}"
    );
    // The callback stopped the list at its first item.
    let textures: Vec<&String> = events
        .iter()
        .filter(|e| e.starts_with("DDRAW.EnumTextureFormats item "))
        .collect();
    assert_eq!(textures.len(), 1, "{events:#?}");
    assert!(textures[0].ends_with(" -> CANCEL"), "{textures:?}");
    assert_eq!(counts[textures[0].as_str()], 1);
}

// The same scenes on a ToadForce, whose HAL sends them to the card as
// level 1 commands. The card's pixels are defined
// (`docs/toadforce-spec.md`), so these are exact.

fn toadforce_scene(steps: Vec<Step>) -> Scene {
    Scene {
        api: Api::D3D7,
        card: None,
        clear: 0,
        zbuffer: None,
        texture: None,
        texture_alpha: false,
        texture_system: false,
        steps,
    }
}

#[test]
fn a_toadforce_draws_a_gouraud_shaded_triangle() {
    let scene = toadforce_scene(vec![Step::Draw(corners())]);
    let p = draw_scene_toadforce("toadforce_gouraud", &scene);
    assert_rgb(&p, 5, 5, [255, 0, 0], 16);
    assert_rgb(&p, 57, 5, [0, 255, 0], 24);
    assert_rgb(&p, 5, 57, [0, 0, 255], 24);
    assert_rgb(&p, 22, 22, [85, 85, 85], 24);
    assert_rgb(&p, 60, 60, [0, 0, 0], 0);
}

#[test]
fn a_toadforce_samples_a_texture_where_it_is() {
    // Red, green, blue and white quarters.
    let texels = std::array::from_fn(|i| match (i % 8 >= 4, i / 8 >= 4) {
        (false, false) => 0xF800,
        (true, false) => 0x07E0,
        (false, true) => 0x001F,
        (true, true) => 0xFFFF,
    });
    let scene = Scene {
        texture: Some(texels),
        ..toadforce_scene(vec![Step::Draw(quad(0.0, 0.0, 64.0, 64.0, 0.5, WHITE))])
    };
    let p = draw_scene_toadforce("toadforce_textured", &scene);
    assert_rgb(&p, 16, 16, [255, 0, 0], 0);
    assert_rgb(&p, 48, 16, [0, 255, 0], 0);
    assert_rgb(&p, 16, 48, [0, 0, 255], 0);
    assert_rgb(&p, 48, 48, [255, 255, 255], 0);
    assert_rgb(&p, 63, 0, [0, 255, 0], 0);
    assert_rgb(&p, 63, 63, [255, 255, 255], 0);

    // With its color key on, the keyed texels are left out.
    let scene = Scene {
        clear: BLUE,
        texture: Some(texels),
        ..toadforce_scene(vec![
            Step::Key(0x07E0),
            Step::State(RS_COLORKEYENABLE, 1),
            Step::Draw(quad(0.0, 0.0, 64.0, 64.0, 0.5, WHITE)),
        ])
    };
    let p = draw_scene_toadforce("toadforce_keyed", &scene);
    assert_rgb(&p, 16, 16, [255, 0, 0], 0);
    assert_rgb(&p, 48, 16, [0, 0, 255], 0);
    assert_rgb(&p, 48, 48, [255, 255, 255], 0);
}

#[test]
fn a_toadforce_tests_depth_blends_and_fogs() {
    let scene = Scene {
        zbuffer: Some(1.0),
        ..toadforce_scene(vec![
            Step::State(RS_ZENABLE, 1),
            Step::Draw(quad(0.0, 0.0, 32.0, 64.0, 0.25, GREEN)),
            Step::Draw(quad(0.0, 0.0, 64.0, 64.0, 0.75, RED)),
        ])
    };
    let p = draw_scene_toadforce("toadforce_ztest", &scene);
    assert_rgb(&p, 10, 30, [0, 255, 0], 0);
    assert_rgb(&p, 50, 30, [255, 0, 0], 0);

    let scene = Scene {
        clear: BLUE,
        ..toadforce_scene(vec![
            Step::State(RS_ALPHABLENDENABLE, 1),
            Step::State(RS_SRCBLEND, 5),
            Step::State(RS_DESTBLEND, 6),
            Step::Draw(quad(0.0, 0.0, 32.0, 64.0, 0.5, 0x80FF_0000)),
        ])
    };
    let p = draw_scene_toadforce("toadforce_blend", &scene);
    assert_rgb(&p, 16, 30, [128, 0, 127], 12);
    assert_rgb(&p, 48, 30, [0, 0, 255], 0);

    let fogged = |alpha: u32, x0: f32, x1: f32| {
        quad(x0, 0.0, x1, 32.0, 0.5, WHITE)
            .into_iter()
            .map(|v| with_fog(v, alpha))
            .collect()
    };
    // Table fog, linear from 0 to 1: half at depth 0.5. The projection
    // matrix is the identity, with which Direct3D's table fog goes by
    // depth, whatever the vertices' w (a Voodoo has fog by w only).
    let table = quad(0.0, 32.0, 64.0, 64.0, 0.5, WHITE)
        .into_iter()
        .map(|v| Tl { rhw: 0.5, ..v })
        .collect();
    let scene = toadforce_scene(vec![
        Step::State(RS_FOGENABLE, 1),
        Step::State(RS_FOGCOLOR, 0x00_FF00),
        Step::Draw(fogged(0, 0.0, 32.0)),
        Step::Draw(fogged(0xFF, 32.0, 64.0)),
        Step::State(RS_FOGTABLEMODE, 3),
        Step::State(RS_FOGSTART, 0f32.to_bits()),
        Step::State(RS_FOGEND, 1f32.to_bits()),
        Step::Draw(table),
    ]);
    let p = draw_scene_toadforce("toadforce_fog", &scene);
    assert_rgb(&p, 16, 16, [0, 255, 0], 0);
    assert_rgb(&p, 48, 16, [255, 255, 255], 0);
    assert_rgb(&p, 32, 48, [128, 255, 128], 12);
}

/// The render target's pixels after `scene` on a ToadForce, with the
/// HAL's binds: all of them, and those that found the card's state
/// right already (`hal::toadforce::bind`). A debug build binds again
/// on each of those and stops if that sends the card anything.
fn scene_binds(test: &str, scene: &Scene) -> (Vec<u16>, [u64; 2]) {
    let mut checked = Vec::new();
    let (exe, data) = scene_program_with(scene, 16, false, &mut checked);
    let mut cpu = machine_toadforce(test, exe);
    run_until_done(&mut cpu, data);
    drew_on_the_card(&cpu, scene);
    let n = peek(&cpu, data + CALLS);
    let r = results(&cpu, data, n);
    assert!(r[1..].iter().all(|&hr| hr == 0), "results: {r:X?}");
    let pixels = (0..64 * 64)
        .map(|i| {
            let at = data + PIXELS + 2 * i;
            (peek(&cpu, at & !3) >> (8 * (at & 2))) as u16
        })
        .collect();
    (pixels, rust_dos::win32::snapshot::ddraw(&cpu).binds)
}

#[test]
fn a_toadforce_keeps_its_state_for_draws_of_the_same_states() {
    // Three draws with nothing set between them: the first binds the
    // card's state, the others find it there.
    let third = |n: f32| quad(21.0 * n, 0.0, 21.0 * (n + 1.0), 64.0, 0.5, RED);
    let scene = toadforce_scene(vec![
        Step::Draw(third(0.0)),
        Step::Draw(third(1.0)),
        Step::Draw(third(2.0)),
    ]);
    let (p, binds) = scene_binds("toadforce_kept", &scene);
    assert_eq!(binds, [3, 2]);
    for x in [10, 31, 52] {
        assert_rgb(&p, x, 30, [255, 0, 0], 0);
    }

    // A state set to the value it has is no change either.
    let scene = toadforce_scene(vec![
        Step::State(RS_SHADEMODE, 2),
        Step::Draw(third(0.0)),
        Step::State(RS_SHADEMODE, 2),
        Step::Stage(TSS_COLOROP, TOP_MODULATE),
        Step::Draw(third(1.0)),
    ]);
    let (_, binds) = scene_binds("toadforce_kept_same", &scene);
    assert_eq!(binds, [2, 1]);
}

#[test]
fn a_toadforce_draw_after_a_change_has_the_new_state() {
    let green = [0x07E0; 64];
    let (left, right) = (
        quad(0.0, 0.0, 32.0, 64.0, 0.5, WHITE),
        quad(32.0, 0.0, 64.0, 64.0, 0.5, WHITE),
    );
    let draws = |before: Vec<Step>, change: Vec<Step>| {
        let mut steps = before;
        steps.push(Step::Draw(left.clone()));
        steps.extend(change);
        steps.push(Step::Draw(right.clone()));
        steps
    };
    type Case = (&'static str, bool, Vec<Step>, Vec<Step>, [u32; 3]);
    let cases: Vec<Case> = vec![
        // A render state: the second quad is added to the blue behind
        // it.
        (
            "render",
            false,
            vec![Step::State(RS_SRCBLEND, 2), Step::State(RS_DESTBLEND, 2)],
            vec![Step::State(RS_ALPHABLENDENABLE, 1)],
            [0, 255, 255],
        ),
        // A stage state: the second quad has the vertices' white in
        // place of the texture's green.
        (
            "stage",
            false,
            vec![],
            vec![Step::Stage(TSS_COLOROP, TOP_SELECTARG2)],
            [255, 255, 255],
        ),
        // The texture's surface: with its color key set, the second
        // quad's texels are all left out. Where the card samples the
        // texture, and of the HAL's copy of one in system memory.
        (
            "key",
            false,
            vec![Step::State(RS_COLORKEYENABLE, 1)],
            vec![Step::Key(0x07E0)],
            [0, 0, 255],
        ),
        (
            "key_copy",
            true,
            vec![Step::State(RS_COLORKEYENABLE, 1)],
            vec![Step::Key(0x07E0)],
            [0, 0, 255],
        ),
    ];
    for (name, texture_system, before, change, want) in cases {
        let scene = Scene {
            clear: BLUE,
            texture: Some(green),
            texture_system,
            ..toadforce_scene(draws(before, change))
        };
        let (p, binds) = scene_binds(&format!("toadforce_change_{name}"), &scene);
        assert_eq!(binds, [2, 0], "{name}");
        assert_rgb(&p, 16, 30, [0, 255, 0], 0);
        assert_rgb(&p, 48, 30, want, 0);
    }

    // A clear takes the card's state to set the target: the draw after
    // it binds anew, and draws over what was cleared.
    let scene = Scene {
        clear: BLUE,
        ..toadforce_scene(vec![
            Step::Draw(left.clone()),
            Step::Clear(D3DCLEAR_TARGET, RED),
            Step::Draw(right.clone()),
        ])
    };
    let (p, binds) = scene_binds("toadforce_change_clear", &scene);
    assert_eq!(binds, [2, 0]);
    assert_rgb(&p, 16, 30, [255, 0, 0], 0);
    assert_rgb(&p, 48, 30, [255, 255, 255], 0);
}

#[test]
fn a_toadforce_draws_direct3d_6_flat_shading_points_and_lines() {
    let scene = Scene {
        api: Api::D3D6,
        clear: 0xFF00_0080,
        ..toadforce_scene(vec![Step::Draw(corners())])
    };
    let p = draw_scene_toadforce("toadforce_d3d6", &scene);
    assert_rgb(&p, 5, 5, [255, 0, 0], 16);
    assert_rgb(&p, 22, 22, [85, 85, 85], 24);
    assert_rgb(&p, 60, 60, [0, 0, 0x84], 4);

    let scene = toadforce_scene(vec![
        Step::State(RS_SHADEMODE, 1),
        Step::Draw(corners()),
    ]);
    let p = draw_scene_toadforce("toadforce_flat", &scene);
    assert_rgb(&p, 22, 22, [255, 0, 0], 0);
    assert_rgb(&p, 50, 8, [255, 0, 0], 0);

    let scene = toadforce_scene(vec![
        Step::Prims(D3DPT_POINTLIST, vec![tl(10.0, 10.0, 0.5, WHITE)]),
        Step::Prims(
            D3DPT_LINELIST,
            vec![tl(20.0, 20.0, 0.5, RED), tl(30.0, 20.0, 0.5, RED)],
        ),
    ]);
    let p = draw_scene_toadforce("toadforce_points", &scene);
    assert_rgb(&p, 10, 10, [255, 255, 255], 0);
    assert_rgb(&p, 11, 10, [0, 0, 0], 0);
    assert_rgb(&p, 20, 20, [255, 0, 0], 0);
    assert_rgb(&p, 29, 20, [255, 0, 0], 0);
    assert_rgb(&p, 25, 21, [0, 0, 0], 0);
}

#[test]
fn a_toadforce_draws_into_a_32_bit_target_with_a_24_bit_depth_buffer() {
    let scene = Scene {
        zbuffer: Some(1.0),
        ..toadforce_scene(vec![
            Step::State(RS_ZENABLE, 1),
            Step::Draw(quad(0.0, 0.0, 32.0, 64.0, 0.25, 0xFF12_3456)),
            Step::Draw(quad(0.0, 0.0, 64.0, 64.0, 0.75, 0xFFAB_CDEF)),
        ])
    };
    let p = draw_scene32_toadforce("toadforce_32", &scene);
    assert_eq!(p[30 * 64 + 10], 0x12_3456);
    assert_eq!(p[30 * 64 + 50], 0xAB_CDEF);
}

// Vertices the card transforms and lights itself.

const RS_CULLMODE: u32 = 22;
const RS_LIGHTING: u32 = 137;
const TS_WORLD: u32 = 1;
const TS_VIEW: u32 = 2;

fn bits(numbers: &[f32]) -> Vec<u32> {
    numbers.iter().map(|n| n.to_bits()).collect()
}

/// A triangle of positions and, if given, one normal or one color
/// for all three, as the words of its vertices.
fn world_triangle(p: [[f32; 3]; 3], normal: Option<[f32; 3]>, color: Option<u32>) -> Vec<u32> {
    let mut words = Vec::new();
    for position in p {
        words.extend(bits(&position));
        words.extend(normal.iter().flat_map(|n| bits(n)));
        words.extend(color);
    }
    words
}

/// A D3DLIGHT7 that is white: its type, position, direction and the
/// angles of a spot light's cones.
fn light7(kind: u32, position: [f32; 3], direction: [f32; 3], cones: [f32; 2]) -> Vec<u32> {
    let mut words = vec![kind];
    words.extend(bits(&[1.0; 4]));
    words.extend(bits(&[0.0; 8]));
    words.extend(bits(&position));
    words.extend(bits(&direction));
    // Its range and falloff, and no attenuation.
    words.extend(bits(&[10.0, 1.0, 1.0, 0.0, 0.0]));
    words.extend(bits(&cones));
    words
}

/// SetMaterial of a D3DMATERIAL7 with the diffuse color `diffuse`, and
/// light 0 as `light`, switched on.
fn lit_by(light: Vec<u32>, diffuse: [f32; 3]) -> Vec<Step> {
    let mut material = bits(&[diffuse[0], diffuse[1], diffuse[2], 1.0]);
    material.extend(bits(&[0.0; 13]));
    vec![
        Step::State(RS_CULLMODE, 1),
        Step::Call(DEV7_SET_MATERIAL, vec![], material),
        Step::Call(DEV7_SET_LIGHT, vec![0], light),
        Step::Call(DEV7_LIGHT_ENABLE, vec![0, 1], vec![]),
    ]
}

#[test]
fn a_toadforce_transforms_vertices_by_the_devices_matrices() {
    // The top left quarter's upper triangle, which the world matrix
    // moves into the top right quarter.
    let triangle = [[-1.0, 1.0, 0.5], [0.0, 1.0, 0.5], [-1.0, 0.0, 0.5]];
    let mut world = [0.0f32; 16];
    (world[0], world[5], world[10], world[15], world[12]) = (1.0, 1.0, 1.0, 1.0, 1.0);
    let scene = toadforce_scene(vec![
        Step::State(RS_LIGHTING, 0),
        Step::State(RS_CULLMODE, 1),
        Step::Call(DEV7_SET_TRANSFORM, vec![TS_WORLD], bits(&world)),
        Step::World(
            D3DFVF_XYZ | D3DFVF_DIFFUSE,
            world_triangle(triangle, None, Some(0xFF12_3456)),
            3,
        ),
    ]);
    let p = draw_scene32_toadforce("toadforce_world", &scene);
    assert_eq!(p[5 * 64 + 40], 0x12_3456);
    assert_eq!(p[5 * 64 + 34], 0x12_3456);
    assert_eq!(p[5 * 64 + 20], 0);
    assert_eq!(p[30 * 64 + 40], 0);
    // What is off the target's right edge is cut.
    assert_eq!(p[6 * 64 + 63], 0);
    assert_eq!(p[64 + 61], 0x12_3456);
}

#[test]
fn a_toadforce_lights_vertices_with_the_devices_lights_and_material() {
    // A light along z, on a surface that faces it and on one tilted
    // so that the cosine is 0.8.
    let facing = [[-1.0, 1.0, 0.5], [0.0, 1.0, 0.5], [-1.0, 0.0, 0.5]];
    let tilted = [[0.0, 1.0, 0.5], [1.0, 1.0, 0.5], [0.0, 0.0, 0.5]];
    let fvf = D3DFVF_XYZ | D3DFVF_NORMAL;
    let mut steps = lit_by(light7(3, [0.0; 3], [0.0, 0.0, 1.0], [0.0; 2]), [1.0, 0.5, 0.25]);
    steps.push(Step::World(
        fvf,
        world_triangle(facing, Some([0.0, 0.0, -1.0]), None),
        3,
    ));
    steps.push(Step::World(
        fvf,
        world_triangle(tilted, Some([0.0, 0.6, -0.8]), None),
        3,
    ));
    let p = draw_scene32_toadforce("toadforce_lit", &toadforce_scene(steps));
    assert_eq!(p[5 * 64 + 5], 0xFF_8040);
    assert_eq!(p[5 * 64 + 40], 0xCC_6633);
    assert_eq!(p[40 * 64 + 40], 0);
}

/// A spot light behind a camera that looks the other way round, at a
/// small triangle in the middle of its cone.
fn spot_scene(card: bool) -> Scene {
    let mut view = [0.0f32; 16];
    (view[0], view[5], view[10], view[15]) = (-1.0, 1.0, -1.0, 1.0);
    let triangle = [[-0.1, 0.1, -0.5], [0.1, 0.1, -0.5], [-0.1, -0.1, -0.5]];
    let spot = light7(2, [0.0, 0.0, 1.0], [0.0, 0.0, -1.0], [1.0, 2.0]);
    let mut steps = lit_by(spot, [1.0; 3]);
    steps.push(Step::Call(DEV7_SET_TRANSFORM, vec![TS_VIEW], bits(&view)));
    steps.push(Step::World(
        D3DFVF_XYZ | D3DFVF_NORMAL,
        world_triangle(triangle, Some([0.0, 0.0, 1.0]), None),
        3,
    ));
    let scene = toadforce_scene(steps);
    if card { scene } else { Scene { api: Api::D3D7, ..scene } }
}

#[test]
fn a_spot_light_points_where_the_view_matrix_turns_it() {
    // The card's transform and lighting.
    let p = draw_scene32_toadforce("toadforce_spot", &spot_scene(true));
    let lit = p[30 * 64 + 33];
    assert!(lit & 0xFF >= 0xF8 && lit >> 16 >= 0xF8, "{lit:X}");
    // The pipeline's, for a Voodoo3.
    let p = draw_scene("voodoo3_spot", &spot_scene(false));
    assert_rgb(&p, 33, 30, [255, 255, 255], 12);
}

/// A quad over the whole target, of positions alone, with a texture of
/// red, green, blue and white quarters on stage 0, whose coordinates
/// are the camera-space position through `matrix` with the
/// D3DTSS_TEXTURETRANSFORMFLAGS `flags`.
fn generated_scene(matrix: [f32; 16], flags: u32) -> Scene {
    let texels = std::array::from_fn(|i| match (i % 8 >= 4, i / 8 >= 4) {
        (false, false) => 0xF800,
        (true, false) => 0x07E0,
        (false, true) => 0x001F,
        (true, true) => 0xFFFF,
    });
    let quad = [
        [-1.0, 1.0, 0.5],
        [1.0, 1.0, 0.5],
        [-1.0, -1.0, 0.5],
        [1.0, 1.0, 0.5],
        [1.0, -1.0, 0.5],
        [-1.0, -1.0, 0.5],
    ];
    let scene = toadforce_scene(vec![
        Step::State(RS_LIGHTING, 0),
        Step::State(RS_CULLMODE, 1),
        Step::Stage(TSS_TEXCOORDINDEX, TCI_CAMERASPACEPOSITION),
        Step::Stage(TSS_TEXTURETRANSFORMFLAGS, flags),
        Step::Call(DEV7_SET_TRANSFORM, vec![16], bits(&matrix)),
        Step::World(D3DFVF_XYZ, quad.iter().flat_map(|p| bits(p)).collect(), 6),
    ]);
    Scene {
        texture: Some(texels),
        ..scene
    }
}

const TSS_TEXCOORDINDEX: u32 = 11;
const TSS_TEXTURETRANSFORMFLAGS: u32 = 24;
const TCI_CAMERASPACENORMAL: u32 = 0x1_0000;
const TCI_CAMERASPACEPOSITION: u32 = 0x2_0000;
const TTFF_PROJECTED: u32 = 0x100;
const RS_NORMALIZENORMALS: u32 = 143;

/// The quarters of `generated_scene`'s texture, each where the matrix
/// puts it.
fn assert_quarters(p: &[u16]) {
    assert_rgb(p, 16, 16, [255, 0, 0], 8);
    assert_rgb(p, 48, 16, [0, 255, 0], 8);
    assert_rgb(p, 16, 48, [0, 0, 255], 8);
    assert_rgb(p, 48, 48, [255, 255, 255], 8);
}

#[test]
fn texture_coordinates_made_from_the_position_go_through_the_texture_matrix() {
    // u is x / 2 + 0.5 and v is 0.5 - y / 2: the texture over the
    // target once. D3DTTFF_COUNT2.
    let mut matrix = [0.0f32; 16];
    (matrix[0], matrix[12], matrix[5], matrix[13]) = (0.5, 0.5, -0.5, 0.5);
    let scene = generated_scene(matrix, 2);
    // The pipeline's, for a Voodoo3, and the card's.
    assert_quarters(&draw_scene("voodoo3_generated", &scene));
    assert_quarters(&draw_scene_toadforce("toadforce_generated", &scene));
}

#[test]
fn projected_texture_coordinates_are_divided_by_the_last_one() {
    // Twice the coordinates, and 2 as the third: D3DTTFF_COUNT3 and
    // D3DTTFF_PROJECTED divide the first two by it.
    let mut matrix = [0.0f32; 16];
    (matrix[0], matrix[12], matrix[5], matrix[13]) = (1.0, 1.0, -1.0, 1.0);
    matrix[14] = 2.0;
    let scene = generated_scene(matrix, 3 | TTFF_PROJECTED);
    assert_quarters(&draw_scene("voodoo3_projected", &scene));
    assert_quarters(&draw_scene_toadforce("toadforce_projected", &scene));
    // Transformed vertices keep their set, and with no count the
    // fourth coordinate divides it, which is 1 for a set of two. The
    // ToadForce takes such vertices with four coordinates a set.
    let scene = Scene {
        steps: vec![
            Step::Stage(TSS_TEXTURETRANSFORMFLAGS, TTFF_PROJECTED),
            Step::Draw(quad(0.0, 0.0, 64.0, 64.0, 0.5, WHITE)),
        ],
        ..scene
    };
    assert_quarters(&draw_scene("voodoo3_projected_tl", &scene));
    assert_quarters(&draw_scene_toadforce("toadforce_projected_tl", &scene));
}

#[test]
fn normals_coordinates_are_made_from_are_normalized_without_lighting() {
    // Normals 10 long toward the quad's corners, made 1 long by
    // D3DRENDERSTATE_NORMALIZENORMALS: the texture matrix then puts the
    // texture over the target once. At their own length the texture
    // would repeat ten times, and the right quarters would be the left
    // ones' colours.
    let mut matrix = [0.0f32; 16];
    let k = 0.5f32.sqrt();
    (matrix[0], matrix[12], matrix[5], matrix[13]) = (k, 0.5, -k, 0.5);
    let n = 50.0f32.sqrt();
    let corners = [
        [-1.0, 1.0],
        [1.0, 1.0],
        [-1.0, -1.0],
        [1.0, 1.0],
        [1.0, -1.0],
        [-1.0, -1.0],
    ];
    let vertices = corners
        .iter()
        .flat_map(|&[x, y]| bits(&[x, y, 0.5, x * n, y * n, 0.0]))
        .collect();
    let scene = generated_scene(matrix, 2);
    let scene = Scene {
        steps: vec![
            Step::State(RS_LIGHTING, 0),
            Step::State(RS_CULLMODE, 1),
            Step::State(RS_NORMALIZENORMALS, 1),
            Step::Stage(TSS_TEXCOORDINDEX, TCI_CAMERASPACENORMAL),
            Step::Stage(TSS_TEXTURETRANSFORMFLAGS, 2),
            Step::Call(DEV7_SET_TRANSFORM, vec![16], bits(&matrix)),
            Step::World(D3DFVF_XYZ | D3DFVF_NORMAL, vertices, 6),
        ],
        ..scene
    };
    assert_quarters(&draw_scene("voodoo3_normalized", &scene));
    assert_quarters(&draw_scene_toadforce("toadforce_normalized", &scene));
}

/// The texture of red, green, blue and white quarters on four stages,
/// each with its own set of coordinates: the first three add up the
/// quarters their sets name, and the fourth multiplies by its own.
/// `sets` are the four sets' coordinates, the same at every vertex of
/// a quad over the target's top left quarter, of transformed vertices
/// or of positions the device's identity matrices leave where they are.
fn four_texture_scene(transformed: bool, sets: [[f32; 2]; 4]) -> Scene {
    let texels = std::array::from_fn(|i| match (i % 8 >= 4, i / 8 >= 4) {
        (false, false) => 0xF800,
        (true, false) => 0x07E0,
        (false, true) => 0x001F,
        (true, true) => 0xFFFF,
    });
    let corners: [[f32; 2]; 6] = [
        [0.0, 0.0],
        [1.0, 0.0],
        [0.0, 1.0],
        [1.0, 0.0],
        [1.0, 1.0],
        [0.0, 1.0],
    ];
    let mut words = Vec::new();
    for [x, y] in corners {
        match transformed {
            true => words.extend(bits(&[x * 32.0, y * 32.0, 0.5, 1.0])),
            false => words.extend(bits(&[x - 1.0, 1.0 - y, 0.5])),
        }
        words.extend(sets.iter().flat_map(|set| bits(set)));
    }
    let position = if transformed { D3DFVF_XYZRHW } else { D3DFVF_XYZ };
    let stage = |stage: u32, state: u32, value: u32| {
        Step::Call(DEV7_SET_TEXTURE_STAGE_STATE, vec![stage, state, value], vec![])
    };
    let scene = toadforce_scene(vec![
        Step::State(RS_LIGHTING, 0),
        Step::State(RS_CULLMODE, 1),
        Step::Texture(1),
        Step::Texture(2),
        Step::Texture(3),
        stage(0, TSS_COLOROP, TOP_SELECTARG1),
        stage(1, TSS_COLOROP, TOP_ADD),
        stage(2, TSS_COLOROP, TOP_ADD),
        stage(3, TSS_COLOROP, TOP_MODULATE),
        // D3DFVF_TEX4.
        Step::World(position | 0x400, words, 6),
    ]);
    Scene {
        texture: Some(texels),
        ..scene
    }
}

#[test]
fn a_toadforce_draws_with_four_textures_each_at_its_own_coordinates() {
    // The quarters' centres: red, green and blue add up to white, and
    // the fourth stage leaves what its own quarter has of it.
    let (red, green, blue) = ([0.25, 0.25], [0.75, 0.25], [0.25, 0.75]);
    for transformed in [true, false] {
        // The pipeline's screen vertices, and the card's transform.
        let draw = |fourth: [f32; 2]| {
            let scene = four_texture_scene(transformed, [red, green, blue, fourth]);
            draw_scene_toadforce("toadforce_four", &scene)
        };
        assert_rgb(&draw(blue), 16, 16, [0, 0, 255], 8);
        assert_rgb(&draw(green), 16, 16, [0, 255, 0], 8);
        let shot = draw(red);
        assert_rgb(&shot, 16, 16, [255, 0, 0], 8);
        // Past the quad.
        assert_rgb(&shot, 48, 16, [0, 0, 0], 0);
    }
}

/// A quad over the target's top left quarter with a cube map of six
/// colours, filled where `filled` says, sampled in the direction
/// `direction` at every vertex: of transformed vertices, or of
/// positions the device's identity matrices leave where they are.
fn cube_scene(transformed: bool, direction: [f32; 3], filled: Filled) -> Scene {
    let corners: [[f32; 2]; 6] = [
        [0.0, 0.0],
        [1.0, 0.0],
        [0.0, 1.0],
        [1.0, 0.0],
        [1.0, 1.0],
        [0.0, 1.0],
    ];
    let mut words = Vec::new();
    for [x, y] in corners {
        match transformed {
            true => words.extend(bits(&[x * 32.0, y * 32.0, 0.5, 1.0])),
            false => words.extend(bits(&[x - 1.0, 1.0 - y, 0.5])),
        }
        words.extend(bits(&direction));
    }
    let position = if transformed { D3DFVF_XYZRHW } else { D3DFVF_XYZ };
    toadforce_scene(vec![
        Step::State(RS_LIGHTING, 0),
        Step::State(RS_CULLMODE, 1),
        Step::Cube(CUBE_COLOURS.map(|(colour, _)| colour), filled),
        Step::Stage(TSS_COLOROP, TOP_SELECTARG1),
        // D3DFVF_TEX1 with D3DFVF_TEXCOORDSIZE3(0).
        Step::World(position | 0x100 | 1 << 16, words, 6),
    ])
}

/// The faces' colours as 565 texels and as the pixels they draw: red,
/// green, blue, yellow, cyan and magenta for +x, -x, +y, -y, +z, -z.
const CUBE_COLOURS: [(u16, [u32; 3]); 6] = [
    (0xF800, [255, 0, 0]),
    (0x07E0, [0, 255, 0]),
    (0x001F, [0, 0, 255]),
    (0xFFE0, [255, 255, 0]),
    (0x07FF, [0, 255, 255]),
    (0xF81F, [255, 0, 255]),
];

#[test]
fn a_toadforce_samples_a_cube_map_in_the_direction_of_its_coordinates() {
    for transformed in [true, false] {
        // The pipeline's screen vertices, and the card's transform.
        for (direction, (_, rgb)) in CUBE_DIRECTIONS.into_iter().zip(CUBE_COLOURS) {
            let scene = cube_scene(transformed, direction, Filled::Video);
            let p = draw_scene_toadforce("toadforce_cube", &scene);
            assert_rgb(&p, 16, 16, rgb, 8);
            // Past the quad.
            assert_rgb(&p, 48, 16, [0, 0, 0], 0);
        }
    }
}

/// Directions that point at the faces +x, -x, +y, -y, +z and -z.
const CUBE_DIRECTIONS: [[f32; 3]; 6] = [
    [1.0, 0.2, -0.3],
    [-1.0, 0.2, 0.3],
    [0.2, 1.0, 0.3],
    [-0.2, -1.0, 0.3],
    [0.2, -0.3, 1.0],
    [0.2, 0.3, -1.0],
];

#[test]
fn a_toadforce_samples_a_cube_map_from_system_memory_and_one_loaded_from_it() {
    // The HAL's copy of all six faces, and IDirect3DDevice7::Load's
    // copy of each face into a cube map in video memory.
    for filled in [Filled::System, Filled::Loaded] {
        for (direction, (_, rgb)) in CUBE_DIRECTIONS.into_iter().zip(CUBE_COLOURS) {
            let scene = cube_scene(true, direction, filled);
            let p = draw_scene_toadforce("toadforce_cube_copied", &scene);
            assert_rgb(&p, 16, 16, rgb, 8);
        }
    }
}

#[test]
fn the_t_and_l_hal_device_draws_as_the_hal_device_does() {
    let scene = |api| {
        let facing = [[-1.0, 1.0, 0.5], [0.0, 1.0, 0.5], [-1.0, 0.0, 0.5]];
        let light = light7(3, [0.0; 3], [0.0, 0.0, 1.0], [0.0; 2]);
        let mut steps = lit_by(light, [1.0, 0.5, 0.25]);
        steps.push(Step::World(
            D3DFVF_XYZ | D3DFVF_NORMAL,
            world_triangle(facing, Some([0.0, 0.0, -1.0]), None),
            3,
        ));
        steps.push(Step::Draw(quad(32.0, 32.0, 64.0, 64.0, 0.5, 0xFF12_3456)));
        Scene {
            api,
            ..toadforce_scene(steps)
        }
    };
    let p = draw_scene32_toadforce("toadforce_tnl_device", &scene(Api::D3D7Tnl));
    assert_eq!(p[5 * 64 + 5], 0xFF_8040);
    assert_eq!(p[40 * 64 + 40], 0x12_3456);
    // Every pixel is the HAL device's.
    let hal = draw_scene32_toadforce("toadforce_hal_device", &scene(Api::D3D7));
    assert!(p == hal, "the T&L HAL device's picture differs from the HAL's");
}

const D3D7_ENUM_DEVICES: u32 = 3;
const DEV7_GET_CAPS: u32 = 3;
/// Where the device's description is read to.
const CAPS7: u32 = 0x600;

/// A program that lists Direct3D 7's devices, makes the T&L HAL
/// device and reads its description: its results, with the machine
/// and the lines of the 3D trace.
fn tnl_device_program(test: &str, toadforce: bool) -> (Cpu, u32, Vec<String>) {
    let mut d = data();
    target_desc(&mut d, DESC, 124, 64, 64);
    tnl_guid(&mut d, IID_HAL);
    let (exe, data) = program(&[USER32, DDRAW], &d, |a, l| {
        let mut c = Calls { next: 0 };
        // The callback (description, name, device description,
        // context), which goes on.
        let mut after = a.create_label();
        a.call(after)?;
        a.mov(eax, 1)?;
        a.ret_1(16)?;
        a.set_label(&mut after)?;
        a.pop(dword_ptr(l.at(ENUM_OK)))?;
        start(a, l, &mut c, true, false)?;
        let args = [Arg::At(IID_D3D7), Arg::At(D3D)];
        c.com(a, l, DD, QUERY_INTERFACE, &args)?;
        let args = [Arg::Val(ENUM_OK), Arg::I(0)];
        c.com(a, l, D3D, D3D7_ENUM_DEVICES, &args)?;
        let args = [Arg::At(DESC), Arg::At(TARGET), Arg::I(0)];
        c.com(a, l, DD, DD_CREATE_SURFACE, &args)?;
        let args = [Arg::At(IID_HAL), Arg::Val(TARGET), Arg::At(DEVICE)];
        c.com(a, l, D3D, D3D7_CREATE_DEVICE, &args)?;
        // The calls as far as CreateDevice. Without a device there
        // is nothing to ask.
        a.mov(dword_ptr(l.at(CALLS)), c.next)?;
        let mut done = a.create_label();
        a.test(eax, eax)?;
        a.jnz(done)?;
        c.com(a, l, DEVICE, DEV7_GET_CAPS, &[Arg::At(CAPS7)])?;
        a.set_label(&mut done)?;
        finish(a, l)
    });
    let mut cpu = if toadforce {
        machine_toadforce(test, exe)
    } else {
        machine(test, exe)
    };
    let path = PathBuf::from("target/test_win32_d3d")
        .join(test)
        .join("t.trace");
    rust_dos::win32::trace3d::start(&mut cpu, &path).unwrap();
    run_until_done(&mut cpu, data);
    rust_dos::win32::trace3d::flush(&mut cpu);
    let (events, _) = read_trace(&path);
    let devices = events
        .into_iter()
        .filter(|e| e.starts_with("DDRAW.EnumDevices item "))
        .collect();
    (cpu, data, devices)
}

#[test]
fn a_toadforce_has_direct3d_7s_t_and_l_hal_device_beside_the_hal_one() {
    const TNL: u32 = 0x1_0000;
    let (cpu, data, devices) = tnl_device_program("tnl_device_toadforce", true);
    // The HAL device, then the T&L HAL device, which alone says the
    // card transforms and lights.
    assert_eq!(devices.len(), 2, "{devices:#?}");
    assert!(devices[0].contains("\"Direct3D HAL\""), "{devices:#?}");
    assert!(devices[1].contains("\"Direct3D T&L HAL\""), "{devices:#?}");
    let devcaps = |line: &str| {
        let at = line.find("devcaps=0x").unwrap() + 10;
        let hex: String = line[at..].chars().take_while(|c| c.is_ascii_hexdigit()).collect();
        u32::from_str_radix(&hex, 16).unwrap()
    };
    assert_eq!(devcaps(&devices[0]) & TNL, 0);
    assert_eq!(devcaps(&devices[1]) & TNL, TNL);
    let r = results(&cpu, data, peek(&cpu, data + CALLS) + 1);
    assert!(r[1..].iter().all(|&hr| hr == 0), "results: {r:X?}");
    // The device made says what it is: the capability, eight lights
    // on at once and its GUID.
    assert_eq!(peek(&cpu, data + CAPS7) & TNL, TNL);
    assert_eq!(peek(&cpu, data + CAPS7 + 0xBC), 8);
    assert_eq!(peek(&cpu, data + CAPS7 + 0xC4), 0xF504_9E78);
    // wMaxUserClipPlanes and, above it, wMaxVertexBlendMatrices: the
    // four world matrices the card blends a vertex between.
    assert_eq!(peek(&cpu, data + CAPS7 + 0xD4), 6 | 4 << 16);

    // A Voodoo3 has the HAL device alone, and makes no other.
    let (cpu, data, devices) = tnl_device_program("tnl_device_voodoo3", false);
    assert_eq!(devices.len(), 1, "{devices:#?}");
    assert_eq!(devcaps(&devices[0]) & TNL, 0);
    let r = results(&cpu, data, peek(&cpu, data + CALLS));
    assert_eq!(r.last(), Some(&0x8007_000E), "results: {r:X?}");
}

#[test]
fn a_toadforce_draws_with_a_copy_of_a_texture_in_system_memory() {
    let texels = std::array::from_fn(|i| match (i % 8 >= 4, i / 8 >= 4) {
        (false, false) => 0xF800,
        (true, false) => 0x07E0,
        (false, true) => 0x001F,
        (true, true) => 0xFFFF,
    });
    let scene = Scene {
        clear: BLUE,
        texture: Some(texels),
        texture_system: true,
        ..toadforce_scene(vec![
            Step::Draw(quad(0.0, 0.0, 64.0, 32.0, 0.5, WHITE)),
            // The key changes the copy the next draw is made with.
            Step::Key(0x07E0),
            Step::State(RS_COLORKEYENABLE, 1),
            Step::Draw(quad(0.0, 32.0, 64.0, 64.0, 0.5, WHITE)),
        ])
    };
    let p = draw_scene_toadforce("toadforce_copied", &scene);
    assert_rgb(&p, 16, 8, [255, 0, 0], 0);
    assert_rgb(&p, 48, 8, [0, 255, 0], 0);
    assert_rgb(&p, 16, 24, [0, 0, 255], 0);
    assert_rgb(&p, 48, 24, [255, 255, 255], 0);
    assert_rgb(&p, 16, 40, [255, 0, 0], 0);
    assert_rgb(&p, 48, 40, [0, 0, 255], 0);
    assert_rgb(&p, 48, 56, [255, 255, 255], 0);
}

// Vertices blended between world matrices (D3DRENDERSTATE_VERTEXBLEND),
// by the card on a ToadForce and by the pipeline on a Voodoo3.

const RS_VERTEXBLEND: u32 = 151;
const TS_WORLD1: u32 = 4;
const TS_WORLD2: u32 = 5;
const D3DFVF_XYZB1: u32 = 0x006;
const D3DFVF_XYZB2: u32 = 0x008;

/// The identity moved by `t`.
fn moved(t: [f32; 3]) -> Vec<u32> {
    let mut m = [0.0f32; 16];
    (m[0], m[5], m[10], m[15]) = (1.0, 1.0, 1.0, 1.0);
    m[12..15].copy_from_slice(&t);
    bits(&m)
}

/// The top left quarter's upper triangle, each vertex with two weights
/// of 0.25 and a colour, blended with D3DVBLEND_2WEIGHTS between the
/// world matrix, WORLD1, which moves a quarter's width to the right,
/// and WORLD2, which moves a quarter's height down. The half the
/// weights leave is WORLD2's: 8 pixels right and 16 down.
fn weighted_scene() -> Scene {
    let mut words = Vec::new();
    for position in [[-1.0, 1.0, 0.5], [0.0, 1.0, 0.5], [-1.0, 0.0, 0.5]] {
        words.extend(bits(&position));
        words.extend(bits(&[0.25, 0.25]));
        words.push(0xFF12_3456);
    }
    toadforce_scene(vec![
        Step::State(RS_LIGHTING, 0),
        Step::State(RS_CULLMODE, 1),
        Step::Call(DEV7_SET_TRANSFORM, vec![TS_WORLD1], moved([1.0, 0.0, 0.0])),
        Step::Call(DEV7_SET_TRANSFORM, vec![TS_WORLD2], moved([0.0, -1.0, 0.0])),
        Step::State(RS_VERTEXBLEND, 2),
        Step::World(D3DFVF_XYZB2 | D3DFVF_DIFFUSE, words, 3),
    ])
}

/// The same triangle with a normal along x and one weight of 0.5,
/// blended with D3DVBLEND_1WEIGHT between the world matrix and WORLD1,
/// which turns x into y. A white light shines toward -x on a white
/// material: the blended normal is half along x and half along y, and
/// the triangle is lit by half. Its corners come to (0, 32), (16, 16)
/// and (16, 48).
fn weighted_lit_scene() -> Scene {
    let mut words = Vec::new();
    for position in [[-1.0, 1.0, 0.5], [0.0, 1.0, 0.5], [-1.0, 0.0, 0.5]] {
        words.extend(bits(&position));
        words.extend(bits(&[0.5]));
        words.extend(bits(&[1.0, 0.0, 0.0]));
    }
    let mut turn = [0.0f32; 16];
    (turn[1], turn[4], turn[10], turn[15]) = (1.0, -1.0, 1.0, 1.0);
    let mut steps = lit_by(light7(3, [0.0; 3], [-1.0, 0.0, 0.0], [0.0; 2]), [1.0; 3]);
    steps.extend([
        Step::Call(DEV7_SET_TRANSFORM, vec![TS_WORLD1], bits(&turn)),
        Step::State(RS_VERTEXBLEND, 1),
        Step::World(D3DFVF_XYZB1 | D3DFVF_NORMAL, words, 3),
    ]);
    toadforce_scene(steps)
}

#[test]
fn a_toadforce_blends_vertices_between_the_world_matrices() {
    // The card's blending: `draw_scene32_toadforce` checks that the
    // card transformed the draw.
    let p = draw_scene32_toadforce("toadforce_weights", &weighted_scene());
    assert_eq!(p[20 * 64 + 12], 0x12_3456);
    assert_eq!(p[20 * 64 + 30], 0x12_3456);
    assert_eq!(p[40 * 64 + 12], 0x12_3456);
    // Where the triangle is without the weights, and past its edges.
    assert_eq!(p[5 * 64 + 5], 0);
    assert_eq!(p[20 * 64 + 5], 0);
    assert_eq!(p[40 * 64 + 30], 0);
    // The pipeline's, for a Voodoo3: the same triangle.
    let p = draw_scene("voodoo3_weights", &weighted_scene());
    for (x, y) in [(12, 20), (30, 20), (12, 40)] {
        assert_rgb(&p, x, y, [0x12, 0x34, 0x56], 8);
    }
    for (x, y) in [(5, 5), (5, 20), (30, 40)] {
        assert_rgb(&p, x, y, [0, 0, 0], 0);
    }
}

#[test]
fn a_toadforce_blends_the_normals_of_blended_vertices() {
    let p = draw_scene32_toadforce("toadforce_weights_lit", &weighted_lit_scene());
    for at in [32 * 64 + 10, 24 * 64 + 12, 40 * 64 + 13] {
        let lit = p[at];
        let channels = [lit >> 16, lit >> 8 & 0xFF, lit & 0xFF];
        assert!(channels.iter().all(|c| (0x7E..=0x81).contains(c)), "{lit:X}");
    }
    assert_eq!(p[5 * 64 + 5], 0);
    assert_eq!(p[32 * 64 + 20], 0);
    // The pipeline's, for a Voodoo3.
    let p = draw_scene("voodoo3_weights_lit", &weighted_lit_scene());
    for (x, y) in [(10, 32), (12, 24), (13, 40)] {
        assert_rgb(&p, x, y, [128, 128, 128], 8);
    }
    assert_rgb(&p, 5, 5, [0, 0, 0], 0);
    assert_rgb(&p, 20, 32, [0, 0, 0], 0);
}
