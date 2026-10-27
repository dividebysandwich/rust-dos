//! The built-in Windows' OpenGL (OPENGL32.DLL): Win32 programs that set
//! a pixel format on a window's DC, make a context on the Voodoo3, draw
//! with glBegin and glEnd and with vertex arrays (shaded, textured,
//! depth tested), read the pixels back with glReadPixels and swap the
//! buffers with GDI's SwapBuffers; and one on a machine without a
//! Banshee or Voodoo3, which loads the DLL but has no pixel format. The
//! programs are PE files made with rust-dos-pe's builder, their code
//! assembled with iced, as in win32_d3d_tests.rs. Each leaves its
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
}

// The functions the programs import, and their places.
const USER32: &[&str] = &["CreateWindowExA", "GetDC"];
const GDI32: &[&str] = &["ChoosePixelFormat", "SetPixelFormat", "SwapBuffers"];
const OPENGL32: &[&str] = &[
    "wglCreateContext",
    "wglMakeCurrent",
    "wglGetProcAddress",
    "glGetString",
    "glGetError",
    "glClearColor",
    "glClear",
    "glBegin",
    "glEnd",
    "glColor3f",
    "glVertex2f",
    "glReadPixels",
    "glEnable",
    "glDepthFunc",
    "glGenTextures",
    "glBindTexture",
    "glTexImage2D",
    "glTexParameteri",
    "glTexEnvi",
    "glEnableClientState",
    "glVertexPointer",
    "glTexCoordPointer",
    "glDrawElements",
    "glVertex3f",
    "glDisable",
    "glViewport",
    "glMatrixMode",
    "glLoadIdentity",
    "glOrtho",
    "glFrustum",
    "glDepthRange",
];
const CREATE_WINDOW: usize = 0;
const GET_DC: usize = 1;
const CHOOSE_PIXEL_FORMAT: usize = 2;
const SET_PIXEL_FORMAT: usize = 3;
const SWAP_BUFFERS: usize = 4;
const GL: usize = 5;
const WGL_CREATE_CONTEXT: usize = GL;
const WGL_MAKE_CURRENT: usize = GL + 1;
const WGL_GET_PROC_ADDRESS: usize = GL + 2;
const GET_STRING: usize = GL + 3;
const GET_ERROR: usize = GL + 4;
const CLEAR_COLOR: usize = GL + 5;
const CLEAR: usize = GL + 6;
const BEGIN: usize = GL + 7;
const END: usize = GL + 8;
const COLOR3F: usize = GL + 9;
const VERTEX2F: usize = GL + 10;
const READ_PIXELS: usize = GL + 11;
const ENABLE: usize = GL + 12;
const DEPTH_FUNC: usize = GL + 13;
const GEN_TEXTURES: usize = GL + 14;
const BIND_TEXTURE: usize = GL + 15;
const TEX_IMAGE_2D: usize = GL + 16;
const TEX_PARAMETERI: usize = GL + 17;
const TEX_ENVI: usize = GL + 18;
const ENABLE_CLIENT_STATE: usize = GL + 19;
const VERTEX_POINTER: usize = GL + 20;
const TEX_COORD_POINTER: usize = GL + 21;
const DRAW_ELEMENTS: usize = GL + 22;
const VERTEX3F: usize = GL + 23;
const DISABLE: usize = GL + 24;
const VIEWPORT: usize = GL + 25;
const MATRIX_MODE: usize = GL + 26;
const LOAD_IDENTITY: usize = GL + 27;
const ORTHO: usize = GL + 28;
const FRUSTUM: usize = GL + 29;
const DEPTH_RANGE: usize = GL + 30;

/// A program importing the functions above, with `data` and the code
/// `f` makes, and where its data is.
fn program(
    data: &[u8],
    f: impl FnOnce(&mut CodeAssembler, &Layout) -> Result<(), IcedError>,
) -> (Vec<u8>, u32) {
    let mut b = Builder::exe(BASE);
    let list =
        |names: &[&'static str]| -> Vec<Imp> { names.iter().map(|n| Imp::Name(n)).collect() };
    let (user, gdi, gl) = (list(USER32), list(GDI32), list(OPENGL32));
    let iat = b
        .imports(&[
            ("USER32.DLL", &user),
            ("GDI32.DLL", &gdi),
            ("OPENGL32.DLL", &gl),
        ])
        .concat();
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

/// A machine with the built-in Windows on, `program` loaded from the
/// shell as T.EXE, and a Voodoo3 as its display adapter when `voodoo3`.
fn machine(test: &str, voodoo3: bool, program: Vec<u8>) -> Cpu {
    let base = PathBuf::from("target/test_win32_opengl").join(test);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join("T.EXE"), program).unwrap();
    let mut cpu = Cpu::new(base);
    if voodoo3 {
        let setup = VideoSetup {
            adapter: Adapter::Voodoo3,
            mono_monitor: false,
        };
        rust_dos::video::bios::install(&mut cpu.bus, setup);
        cpu.bus.configure_voodoo(Some(Board::Voodoo3));
    }
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

/// The program's bytes from `lin` to a zero.
fn peek_string(cpu: &Cpu, lin: u32) -> String {
    let mut out = Vec::new();
    for i in 0..256 {
        let b = (peek(cpu, (lin + i) & !3) >> (8 * ((lin + i) & 3))) as u8;
        if b == 0 {
            break;
        }
        out.push(b);
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Run the program until it says it is done (DONE in its data).
fn run_until_done(cpu: &mut Cpu, data: u32) {
    for _ in 0..400 {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        run_batch(cpu, &mut NoHook, false);
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
const HDC: u32 = 0x04;
const HGLRC: u32 = 0x08;
const TEXTURE: u32 = 0x0C;
const DONE: u32 = 0xF0;
const CLASS: u32 = 0x100;
const PFD: u32 = 0x110;
const PROC_NAME: u32 = 0x140;
const TEXELS: u32 = 0x200;
const VERTICES: u32 = 0x300;
const TEX_COORDS: u32 = 0x380;
const INDICES: u32 = 0x3C0;
/// The results of the calls, one after another.
const RESULTS: u32 = 0x400;
/// The pixels glReadPixels reads: 64x64, RGBA.
const PIXELS: u32 = 0x1000;
const DATA_SIZE: usize = 0x5000;

const WS_POPUP: u32 = 0x8000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;

const GL_VENDOR: u32 = 0x1F00;
const GL_RENDERER: u32 = 0x1F01;
const GL_EXTENSIONS: u32 = 0x1F03;
const GL_COLOR_BUFFER_BIT: u32 = 0x4000;
const GL_DEPTH_BUFFER_BIT: u32 = 0x100;
const GL_TRIANGLES: u32 = 4;
const GL_QUADS: u32 = 7;
const GL_RGBA: u32 = 0x1908;
const GL_RGB: u32 = 0x1907;
const GL_UNSIGNED_BYTE: u32 = 0x1401;
const GL_UNSIGNED_SHORT: u32 = 0x1403;
const GL_UNSIGNED_INT: u32 = 0x1405;
const GL_MODULATE: u32 = 0x2100;
const GL_PROJECTION: u32 = 0x1701;
const GL_MODELVIEW: u32 = 0x1700;
const GL_FLOAT: u32 = 0x1406;
const GL_DEPTH_TEST: u32 = 0x0B71;
const GL_LESS: u32 = 0x0201;
const GL_TEXTURE_2D: u32 = 0x0DE1;
const GL_TEXTURE_MIN_FILTER: u32 = 0x2801;
const GL_NEAREST: u32 = 0x2600;
const GL_TEXTURE_ENV: u32 = 0x2300;
const GL_TEXTURE_ENV_MODE: u32 = 0x2200;
const GL_REPLACE: u32 = 0x1E01;
const GL_VERTEX_ARRAY: u32 = 0x8074;
const GL_TEXTURE_COORD_ARRAY: u32 = 0x8078;

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

/// The data the programs start with: the window class's name, a
/// PIXELFORMATDESCRIPTOR asking for 32-bit color, double buffered,
/// with a depth buffer, and a function's name for wglGetProcAddress.
fn data() -> Vec<u8> {
    let mut d = vec![0u8; DATA_SIZE];
    put(&mut d, CLASS, b"Static\0");
    put(&mut d, PFD, &40u16.to_le_bytes());
    put(&mut d, PFD + 2, &1u16.to_le_bytes());
    // PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER.
    put32(&mut d, PFD + 4, 0x25);
    d[(PFD + 9) as usize] = 32;
    d[(PFD + 23) as usize] = 24;
    put(&mut d, PROC_NAME, b"glActiveTextureARB\0");
    d
}

/// An argument a program passes.
#[derive(Clone, Copy)]
enum Arg {
    /// A value.
    I(u32),
    /// A float.
    F(f32),
    /// The address of the data's byte at an offset.
    At(u32),
    /// The value in the data at an offset.
    Val(u32),
}

/// The program's calls, their results one after another in RESULTS.
struct Calls {
    next: u32,
}

impl Calls {
    /// Call the import `index` with `args`, and keep what it returns.
    fn api(
        &mut self,
        a: &mut CodeAssembler,
        l: &Layout,
        index: usize,
        args: &[Arg],
    ) -> Result<(), IcedError> {
        for arg in args.iter().rev() {
            match *arg {
                Arg::I(v) => a.push(v as i32)?,
                Arg::F(v) => a.push(v.to_bits() as i32)?,
                Arg::At(o) => a.push(l.at(o) as i32)?,
                Arg::Val(o) => a.push(dword_ptr(l.at(o)))?,
            }
        }
        a.call(dword_ptr(l.iat[index] as u64))?;
        a.mov(dword_ptr(l.at(RESULTS + 4 * self.next)), eax)?;
        self.next += 1;
        Ok(())
    }
}

/// The end of a program: DONE set, and it spins, its memory there to
/// look at.
fn finish(a: &mut CodeAssembler, l: &Layout) -> Result<(), IcedError> {
    a.mov(dword_ptr(l.at(DONE)), 1)?;
    let mut again = a.create_label();
    a.set_label(&mut again)?;
    a.jmp(again)
}

/// Make a 64x64 window and its DC, choose and set a pixel format, and
/// make a context current on it: five results.
fn start(a: &mut CodeAssembler, l: &Layout, c: &mut Calls) -> Result<(), IcedError> {
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
            Arg::I(64),
            Arg::I(64),
            Arg::I(0),
            Arg::I(0),
            Arg::I(BASE),
            Arg::I(0),
        ],
    )?;
    a.mov(dword_ptr(l.at(HWND)), eax)?;
    c.api(a, l, GET_DC, &[Arg::Val(HWND)])?;
    a.mov(dword_ptr(l.at(HDC)), eax)?;
    c.api(a, l, CHOOSE_PIXEL_FORMAT, &[Arg::Val(HDC), Arg::At(PFD)])?;
    a.push(l.at(PFD) as i32)?;
    a.push(eax)?;
    a.push(dword_ptr(l.at(HDC)))?;
    a.call(dword_ptr(l.iat[SET_PIXEL_FORMAT] as u64))?;
    a.mov(dword_ptr(l.at(RESULTS + 4 * c.next)), eax)?;
    c.next += 1;
    c.api(a, l, WGL_CREATE_CONTEXT, &[Arg::Val(HDC)])?;
    a.mov(dword_ptr(l.at(HGLRC)), eax)?;
    c.api(a, l, WGL_MAKE_CURRENT, &[Arg::Val(HDC), Arg::Val(HGLRC)])
}

/// Read the 64x64 pixels into PIXELS, swap the buffers and ask for the
/// error.
fn read_back(a: &mut CodeAssembler, l: &Layout, c: &mut Calls) -> Result<(), IcedError> {
    c.api(
        a,
        l,
        READ_PIXELS,
        &[
            Arg::I(0),
            Arg::I(0),
            Arg::I(64),
            Arg::I(64),
            Arg::I(GL_RGBA),
            Arg::I(GL_UNSIGNED_BYTE),
            Arg::At(PIXELS),
        ],
    )?;
    c.api(a, l, SWAP_BUFFERS, &[Arg::Val(HDC)])?;
    c.api(a, l, GET_ERROR, &[])
}

/// The pixel at (x, y) from the bottom left, red, green and blue.
fn pixel(cpu: &Cpu, data: u32, x: u32, y: u32) -> [u8; 3] {
    let p = peek(cpu, data + PIXELS + 4 * (y * 64 + x));
    [p as u8, (p >> 8) as u8, (p >> 16) as u8]
}

#[test]
fn a_context_draws_a_shaded_triangle_and_reads_it_back() {
    let (exe, data) = program(&data(), |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c)?;
        c.api(a, l, GET_STRING, &[Arg::I(GL_VENDOR)])?;
        c.api(a, l, GET_STRING, &[Arg::I(GL_RENDERER)])?;
        c.api(a, l, GET_STRING, &[Arg::I(GL_EXTENSIONS)])?;
        c.api(a, l, WGL_GET_PROC_ADDRESS, &[Arg::At(PROC_NAME)])?;
        let blue = [Arg::F(0.0), Arg::F(0.0), Arg::F(1.0), Arg::F(1.0)];
        c.api(a, l, CLEAR_COLOR, &blue)?;
        c.api(
            a,
            l,
            CLEAR,
            &[Arg::I(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT)],
        )?;
        // A red triangle over the bottom left half.
        c.api(a, l, BEGIN, &[Arg::I(GL_TRIANGLES)])?;
        c.api(a, l, COLOR3F, &[Arg::F(1.0), Arg::F(0.0), Arg::F(0.0)])?;
        c.api(a, l, VERTEX2F, &[Arg::F(-1.0), Arg::F(-1.0)])?;
        c.api(a, l, VERTEX2F, &[Arg::F(1.0), Arg::F(-1.0)])?;
        c.api(a, l, VERTEX2F, &[Arg::F(-1.0), Arg::F(1.0)])?;
        c.api(a, l, END, &[])?;
        read_back(a, l, &mut c)?;
        finish(a, l)
    });
    let mut cpu = machine("triangle", true, exe);
    run_until_done(&mut cpu, data);
    let r: Vec<u32> = (0..21)
        .map(|i| peek(&cpu, data + RESULTS + 4 * i))
        .collect();
    // The window, its DC, the double-buffered format, set, a context,
    // made current.
    assert_ne!(r[0], 0, "window");
    assert_ne!(r[1], 0, "DC");
    assert_eq!(r[2], 2, "the double-buffered pixel format");
    assert_eq!(r[3], 1, "SetPixelFormat");
    assert_ne!(r[4], 0, "wglCreateContext");
    assert_eq!(r[5], 1, "wglMakeCurrent");
    assert_eq!(peek_string(&cpu, r[6]), "3Dfx Interactive Inc.");
    assert!(peek_string(&cpu, r[7]).starts_with("3Dfx/Voodoo3"));
    assert!(peek_string(&cpu, r[8]).contains("GL_ARB_multitexture"));
    assert_ne!(r[9], 0, "glActiveTextureARB");
    // SwapBuffers, and no error.
    assert_eq!(r[19], 1, "SwapBuffers");
    assert_eq!(r[20], 0, "glGetError");
    assert_eq!(pixel(&cpu, data, 4, 4), [255, 0, 0]);
    assert_eq!(pixel(&cpu, data, 60, 60), [0, 0, 255]);
}

#[test]
fn a_triangle_reaching_far_off_the_window_is_drawn_where_it_covers_it() {
    // A red triangle whose bottom right corner is 200 widths of the
    // view to the right: thousands of pixels past the window, and past
    // the band the card's vertices are clipped to. OpenGL draws the
    // part in the view. The card's vertex registers hold 12.4 fixed
    // point, and a vertex clipped at their end wraps round to the far
    // left, where nothing of the triangle shows.
    let (exe, data) = program(&data(), |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c)?;
        let blue = [Arg::F(0.0), Arg::F(0.0), Arg::F(1.0), Arg::F(1.0)];
        c.api(a, l, CLEAR_COLOR, &blue)?;
        c.api(
            a,
            l,
            CLEAR,
            &[Arg::I(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT)],
        )?;
        c.api(a, l, BEGIN, &[Arg::I(GL_TRIANGLES)])?;
        c.api(a, l, COLOR3F, &[Arg::F(1.0), Arg::F(0.0), Arg::F(0.0)])?;
        c.api(a, l, VERTEX2F, &[Arg::F(-1.0), Arg::F(-1.0)])?;
        c.api(a, l, VERTEX2F, &[Arg::F(200.0), Arg::F(-1.0)])?;
        c.api(a, l, VERTEX2F, &[Arg::F(-1.0), Arg::F(1.0)])?;
        c.api(a, l, END, &[])?;
        read_back(a, l, &mut c)?;
        finish(a, l)
    });
    let mut cpu = machine("far_triangle", true, exe);
    run_until_done(&mut cpu, data);
    // SwapBuffers, and no error.
    assert_eq!(peek(&cpu, data + RESULTS + 4 * 15), 1, "SwapBuffers");
    assert_eq!(peek(&cpu, data + RESULTS + 4 * 16), 0, "glGetError");
    // Its top edge falls a hundredth of the view's height across the
    // window: the triangle covers the window but for its top right
    // corner.
    for (x, y) in [(4, 4), (60, 4), (4, 60), (60, 60)] {
        assert_eq!(pixel(&cpu, data, x, y), [255, 0, 0], "({x}, {y})");
    }
}

#[test]
fn vertex_arrays_draw_a_textured_depth_tested_quad() {
    let mut d = data();
    // A 2x2 green texture, RGBA (rows of RGB would be padded to 4
    // bytes by the unpack alignment).
    for i in 0..4 {
        put(&mut d, TEXELS + 4 * i, &[0, 255, 0, 255]);
    }
    // A quad over the top right quarter, then a nearer one over the
    // middle that the depth test keeps, and a farther one it drops.
    put_f32s(
        &mut d,
        VERTICES,
        &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0],
    );
    put_f32s(
        &mut d,
        TEX_COORDS,
        &[0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0],
    );
    for (i, v) in [0u16, 1, 2, 3].iter().enumerate() {
        put(&mut d, INDICES + 2 * i as u32, &v.to_le_bytes());
    }
    let (exe, data) = program(&d, |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c)?;
        c.api(
            a,
            l,
            CLEAR,
            &[Arg::I(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT)],
        )?;
        c.api(a, l, ENABLE, &[Arg::I(GL_DEPTH_TEST)])?;
        c.api(a, l, DEPTH_FUNC, &[Arg::I(GL_LESS)])?;
        c.api(a, l, GEN_TEXTURES, &[Arg::I(1), Arg::At(TEXTURE)])?;
        c.api(
            a,
            l,
            BIND_TEXTURE,
            &[Arg::I(GL_TEXTURE_2D), Arg::Val(TEXTURE)],
        )?;
        c.api(
            a,
            l,
            TEX_IMAGE_2D,
            &[
                Arg::I(GL_TEXTURE_2D),
                Arg::I(0),
                Arg::I(GL_RGB),
                Arg::I(2),
                Arg::I(2),
                Arg::I(0),
                Arg::I(GL_RGBA),
                Arg::I(GL_UNSIGNED_BYTE),
                Arg::At(TEXELS),
            ],
        )?;
        c.api(
            a,
            l,
            TEX_PARAMETERI,
            &[
                Arg::I(GL_TEXTURE_2D),
                Arg::I(GL_TEXTURE_MIN_FILTER),
                Arg::I(GL_NEAREST),
            ],
        )?;
        c.api(
            a,
            l,
            TEX_ENVI,
            &[
                Arg::I(GL_TEXTURE_ENV),
                Arg::I(GL_TEXTURE_ENV_MODE),
                Arg::I(GL_REPLACE),
            ],
        )?;
        c.api(a, l, ENABLE, &[Arg::I(GL_TEXTURE_2D)])?;
        c.api(a, l, ENABLE_CLIENT_STATE, &[Arg::I(GL_VERTEX_ARRAY)])?;
        c.api(a, l, ENABLE_CLIENT_STATE, &[Arg::I(GL_TEXTURE_COORD_ARRAY)])?;
        c.api(
            a,
            l,
            VERTEX_POINTER,
            &[Arg::I(3), Arg::I(GL_FLOAT), Arg::I(0), Arg::At(VERTICES)],
        )?;
        c.api(
            a,
            l,
            TEX_COORD_POINTER,
            &[Arg::I(2), Arg::I(GL_FLOAT), Arg::I(0), Arg::At(TEX_COORDS)],
        )?;
        c.api(
            a,
            l,
            DRAW_ELEMENTS,
            &[
                Arg::I(GL_QUADS),
                Arg::I(4),
                Arg::I(GL_UNSIGNED_SHORT),
                Arg::At(INDICES),
            ],
        )?;
        // Untextured red behind it, at z 0.5: hidden where the green
        // is, shown below it.
        c.api(a, l, DISABLE, &[Arg::I(GL_TEXTURE_2D)])?;
        c.api(a, l, BEGIN, &[Arg::I(GL_QUADS)])?;
        c.api(a, l, COLOR3F, &[Arg::F(1.0), Arg::F(0.0), Arg::F(0.0)])?;
        for (x, y) in [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
            c.api(a, l, VERTEX3F, &[Arg::F(x), Arg::F(y), Arg::F(0.5)])?;
        }
        c.api(a, l, END, &[])?;
        read_back(a, l, &mut c)?;
        finish(a, l)
    });
    let mut cpu = machine("arrays", true, exe);
    run_until_done(&mut cpu, data);
    let error = peek(&cpu, data + RESULTS + 4 * 30);
    assert_eq!(error, 0, "glGetError");
    // Green in the top right quarter, over the red; red below it in
    // the middle; black elsewhere.
    assert_eq!(pixel(&cpu, data, 48, 48), [0, 255, 0]);
    assert_eq!(pixel(&cpu, data, 36, 36), [0, 255, 0]);
    assert_eq!(pixel(&cpu, data, 24, 24), [255, 0, 0]);
    assert_eq!(pixel(&cpu, data, 4, 4), [0, 0, 0]);
}

#[test]
fn without_a_banshee_or_voodoo3_there_is_no_pixel_format() {
    let (exe, data) = program(&data(), |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c)?;
        finish(a, l)
    });
    let mut cpu = machine("no_card", false, exe);
    run_until_done(&mut cpu, data);
    let r: Vec<u32> = (0..6).map(|i| peek(&cpu, data + RESULTS + 4 * i)).collect();
    assert_ne!(r[1], 0, "DC");
    // No format to choose or set, no context.
    assert_eq!(r[2..5], [0, 0, 0]);
}

/// Upload a 2x2 texture of the RGBA texel `texel` as texture `name` (a
/// data offset that glGenTextures filled), with nearest filtering.
fn texture_2x2(
    a: &mut CodeAssembler,
    l: &Layout,
    c: &mut Calls,
    name: u32,
    texels: u32,
) -> Result<(), IcedError> {
    c.api(a, l, BIND_TEXTURE, &[Arg::I(GL_TEXTURE_2D), Arg::Val(name)])?;
    c.api(
        a,
        l,
        TEX_IMAGE_2D,
        &[
            Arg::I(GL_TEXTURE_2D),
            Arg::I(0),
            Arg::I(GL_RGB),
            Arg::I(2),
            Arg::I(2),
            Arg::I(0),
            Arg::I(GL_RGBA),
            Arg::I(GL_UNSIGNED_BYTE),
            Arg::At(texels),
        ],
    )?;
    c.api(
        a,
        l,
        TEX_PARAMETERI,
        &[
            Arg::I(GL_TEXTURE_2D),
            Arg::I(GL_TEXTURE_MIN_FILTER),
            Arg::I(GL_NEAREST),
        ],
    )?;
    c.api(a, l, ENABLE, &[Arg::I(GL_TEXTURE_2D)])
}

#[test]
fn two_texture_units_modulate_a_lightmap_as_quake_iii_draws_walls() {
    const LIGHTMAP: u32 = TEXTURE + 4;
    const LIGHT_TEXELS: u32 = TEXELS + 0x40;
    const ACTIVE: u32 = 0x20;
    const CLIENT_ACTIVE: u32 = 0x24;
    let mut d = data();
    for i in 0..4 {
        put(&mut d, TEXELS + 4 * i, &[0, 255, 0, 255]);
        put(&mut d, LIGHT_TEXELS + 4 * i, &[128, 128, 128, 255]);
    }
    put(&mut d, PROC_NAME + 0x20, b"glClientActiveTextureARB\0");
    // The whole window, as a quad of two triangles by 32-bit indices.
    put_f32s(
        &mut d,
        VERTICES,
        &[
            -1.0, -1.0, 0.0, 1.0, -1.0, 0.0, 1.0, 1.0, 0.0, -1.0, 1.0, 0.0,
        ],
    );
    put_f32s(
        &mut d,
        TEX_COORDS,
        &[0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0],
    );
    for (i, v) in [0u32, 1, 2, 0, 2, 3].iter().enumerate() {
        put32(&mut d, INDICES + 4 * i as u32, *v);
    }
    let (exe, data) = program(&d, |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c)?;
        c.api(a, l, WGL_GET_PROC_ADDRESS, &[Arg::At(PROC_NAME)])?;
        a.mov(dword_ptr(l.at(ACTIVE)), eax)?;
        c.api(a, l, WGL_GET_PROC_ADDRESS, &[Arg::At(PROC_NAME + 0x20)])?;
        a.mov(dword_ptr(l.at(CLIENT_ACTIVE)), eax)?;
        c.api(a, l, GEN_TEXTURES, &[Arg::I(2), Arg::At(TEXTURE)])?;
        c.api(a, l, ENABLE_CLIENT_STATE, &[Arg::I(GL_VERTEX_ARRAY)])?;
        let vertices = [Arg::I(3), Arg::I(GL_FLOAT), Arg::I(0), Arg::At(VERTICES)];
        c.api(a, l, VERTEX_POINTER, &vertices)?;
        // Each unit: its texture, modulated, and texture coordinates.
        for (unit, name, texels) in [(0, TEXTURE, TEXELS), (1, LIGHTMAP, LIGHT_TEXELS)] {
            for f in [ACTIVE, CLIENT_ACTIVE] {
                a.push(0x84C0 + unit)?;
                a.call(dword_ptr(l.at(f)))?;
            }
            texture_2x2(a, l, &mut c, name, texels)?;
            c.api(
                a,
                l,
                TEX_ENVI,
                &[
                    Arg::I(GL_TEXTURE_ENV),
                    Arg::I(GL_TEXTURE_ENV_MODE),
                    Arg::I(GL_MODULATE),
                ],
            )?;
            c.api(a, l, ENABLE_CLIENT_STATE, &[Arg::I(GL_TEXTURE_COORD_ARRAY)])?;
            let coords = [Arg::I(2), Arg::I(GL_FLOAT), Arg::I(0), Arg::At(TEX_COORDS)];
            c.api(a, l, TEX_COORD_POINTER, &coords)?;
        }
        c.api(
            a,
            l,
            DRAW_ELEMENTS,
            &[
                Arg::I(GL_TRIANGLES),
                Arg::I(6),
                Arg::I(GL_UNSIGNED_INT),
                Arg::At(INDICES),
            ],
        )?;
        read_back(a, l, &mut c)?;
        finish(a, l)
    });
    let mut cpu = machine("multitexture", true, exe);
    run_until_done(&mut cpu, data);
    let error = peek(&cpu, data + RESULTS + 4 * 25);
    assert_eq!(error, 0, "glGetError");
    // Green by the lightmap's half.
    let [r, g, b] = pixel(&cpu, data, 32, 32);
    assert!(r == 0 && b == 0 && (120..=136).contains(&g), "{r} {g} {b}");
}

/// Push a double as two dwords, low first.
fn double(v: f64) -> [Arg; 2] {
    let bits = v.to_bits();
    [Arg::I(bits as u32), Arg::I((bits >> 32) as u32)]
}

#[test]
fn a_small_viewport_in_a_corner_keeps_what_it_draws_there() {
    let (exe, data) = program(&data(), |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c)?;
        c.api(
            a,
            l,
            CLEAR,
            &[Arg::I(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT)],
        )?;
        // A 16x16 viewport at the bottom right, as Quake III's engine
        // draws a HUD model in a refdef of its own, with a perspective
        // projection and the depth range cut down.
        c.api(
            a,
            l,
            VIEWPORT,
            &[Arg::I(48), Arg::I(0), Arg::I(16), Arg::I(16)],
        )?;
        c.api(a, l, DEPTH_RANGE, &[double(0.0), double(0.3)].concat())?;
        c.api(a, l, MATRIX_MODE, &[Arg::I(GL_PROJECTION)])?;
        c.api(a, l, LOAD_IDENTITY, &[])?;
        let frustum = [
            double(-1.0),
            double(1.0),
            double(-1.0),
            double(1.0),
            double(1.0),
            double(100.0),
        ]
        .concat();
        c.api(a, l, FRUSTUM, &frustum)?;
        c.api(a, l, MATRIX_MODE, &[Arg::I(GL_MODELVIEW)])?;
        c.api(a, l, LOAD_IDENTITY, &[])?;
        // A tall red triangle filling the viewport's view at z -2.
        c.api(a, l, BEGIN, &[Arg::I(GL_TRIANGLES)])?;
        c.api(a, l, COLOR3F, &[Arg::F(1.0), Arg::F(0.0), Arg::F(0.0)])?;
        c.api(a, l, VERTEX3F, &[Arg::F(-4.0), Arg::F(-4.0), Arg::F(-2.0)])?;
        c.api(a, l, VERTEX3F, &[Arg::F(4.0), Arg::F(-4.0), Arg::F(-2.0)])?;
        c.api(a, l, VERTEX3F, &[Arg::F(0.0), Arg::F(4.0), Arg::F(-2.0)])?;
        c.api(a, l, END, &[])?;
        // The whole window again, and a 2D green square at its top left
        // with an orthographic projection.
        c.api(
            a,
            l,
            VIEWPORT,
            &[Arg::I(0), Arg::I(0), Arg::I(64), Arg::I(64)],
        )?;
        c.api(a, l, DEPTH_RANGE, &[double(0.0), double(1.0)].concat())?;
        c.api(a, l, MATRIX_MODE, &[Arg::I(GL_PROJECTION)])?;
        c.api(a, l, LOAD_IDENTITY, &[])?;
        let ortho = [
            double(0.0),
            double(64.0),
            double(64.0),
            double(0.0),
            double(0.0),
            double(1.0),
        ]
        .concat();
        c.api(a, l, ORTHO, &ortho)?;
        c.api(a, l, BEGIN, &[Arg::I(GL_QUADS)])?;
        c.api(a, l, COLOR3F, &[Arg::F(0.0), Arg::F(1.0), Arg::F(0.0)])?;
        for (x, y) in [(0.0, 0.0), (8.0, 0.0), (8.0, 8.0), (0.0, 8.0)] {
            c.api(a, l, VERTEX2F, &[Arg::F(x), Arg::F(y)])?;
        }
        c.api(a, l, END, &[])?;
        read_back(a, l, &mut c)?;
        finish(a, l)
    });
    let mut cpu = machine("small_viewport", true, exe);
    run_until_done(&mut cpu, data);
    // The calls before glGetError.
    let n = 34;
    assert_eq!(peek(&cpu, data + RESULTS + 4 * n), 0, "glGetError");
    // Red only inside the bottom right 16x16.
    assert_eq!(pixel(&cpu, data, 56, 4), [255, 0, 0]);
    for (x, y) in [(56, 24), (56, 40), (56, 60), (40, 4), (24, 8), (32, 32)] {
        assert_eq!(pixel(&cpu, data, x, y), [0, 0, 0], "({x}, {y})");
    }
    // Green at the top left, y from the bottom.
    assert_eq!(pixel(&cpu, data, 4, 60), [0, 255, 0]);
    assert_eq!(pixel(&cpu, data, 4, 52), [0, 0, 0]);
}

#[test]
fn a_viewport_hanging_off_the_window_keeps_its_center_where_opengl_puts_it() {
    let (exe, data) = program(&data(), |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c)?;
        c.api(a, l, CLEAR, &[Arg::I(GL_COLOR_BUFFER_BIT)])?;
        // As Alice draws its health vial: a viewport the window's size
        // moved off its left edge, the model at its center.
        let vp = [Arg::I((-24i32) as u32), Arg::I(8), Arg::I(64), Arg::I(64)];
        c.api(a, l, VIEWPORT, &vp)?;
        c.api(a, l, BEGIN, &[Arg::I(GL_QUADS)])?;
        c.api(a, l, COLOR3F, &[Arg::F(1.0), Arg::F(0.0), Arg::F(0.0)])?;
        for (x, y) in [
            (-0.125, -0.125),
            (0.125, -0.125),
            (0.125, 0.125),
            (-0.125, 0.125),
        ] {
            c.api(a, l, VERTEX2F, &[Arg::F(x), Arg::F(y)])?;
        }
        c.api(a, l, END, &[])?;
        read_back(a, l, &mut c)?;
        finish(a, l)
    });
    let mut cpu = machine("offset_viewport", true, exe);
    run_until_done(&mut cpu, data);
    // The quad's center is at x -24 + 32 = 8, y 8 + 32 = 40 from the
    // bottom, 8 pixels square.
    assert_eq!(pixel(&cpu, data, 8, 40), [255, 0, 0]);
    for (x, y) in [(20, 40), (32, 40), (8, 28), (8, 52)] {
        assert_eq!(pixel(&cpu, data, x, y), [0, 0, 0], "({x}, {y})");
    }
}
