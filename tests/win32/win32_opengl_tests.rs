//! The built-in Windows' OpenGL (OPENGL32.DLL): Win32 programs that set
//! a pixel format on a window's DC, make a context on the Voodoo3 (and
//! some of them on a ToadForce), draw with glBegin and glEnd and with
//! vertex arrays (shaded, textured, depth tested), read the pixels back
//! with glReadPixels and swap the buffers with GDI's SwapBuffers; one
//! that draws full screen on a Voodoo2 beside the VGA; and one on a
//! machine without a 3dfx card, which loads the DLL but has no pixel
//! format; and GLU32.DLL's mipmaps, matrices and texts on top of it.
//! The programs are PE files made with rust-dos-pe's builder, their
//! code assembled with iced, as in win32_d3d_tests.rs. Each leaves its
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
    "glShadeModel",
    "glLightfv",
    "glLightf",
    "glMaterialfv",
    "glNormal3f",
    "glLightModeli",
    "glColorMaterial",
    "glGetLightfv",
    "glGetMaterialfv",
    "glIsEnabled",
    "glGetFloatv",
    "glPushAttrib",
    "glPopAttrib",
    "glScalef",
    "glNormalPointer",
    "glColorMask",
];
const GLU32: &[&str] = &[
    "gluBuild2DMipmaps",
    "gluOrtho2D",
    "gluErrorString",
    "gluGetString",
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
const SHADE_MODEL: usize = GL + 31;
const LIGHTFV: usize = GL + 32;
const LIGHTF: usize = GL + 33;
const MATERIALFV: usize = GL + 34;
const NORMAL3F: usize = GL + 35;
const LIGHT_MODELI: usize = GL + 36;
const COLOR_MATERIAL: usize = GL + 37;
const GET_LIGHTFV: usize = GL + 38;
const GET_MATERIALFV: usize = GL + 39;
const IS_ENABLED: usize = GL + 40;
const GET_FLOATV: usize = GL + 41;
const PUSH_ATTRIB: usize = GL + 42;
const POP_ATTRIB: usize = GL + 43;
const SCALEF: usize = GL + 44;
const NORMAL_POINTER: usize = GL + 45;
const COLOR_MASK: usize = GL + 46;
const GLU: usize = GL + OPENGL32.len();
const BUILD_2D_MIPMAPS: usize = GLU;
const ORTHO_2D: usize = GLU + 1;
const ERROR_STRING: usize = GLU + 2;
const GLU_GET_STRING: usize = GLU + 3;

/// A program importing the functions above, with `data` and the code
/// `f` makes, and where its data is.
fn program(
    data: &[u8],
    f: impl FnOnce(&mut CodeAssembler, &Layout) -> Result<(), IcedError>,
) -> (Vec<u8>, u32) {
    let mut b = Builder::exe(BASE);
    let list =
        |names: &[&'static str]| -> Vec<Imp> { names.iter().map(|n| Imp::Name(n)).collect() };
    let (user, gdi, gl, glu) = (list(USER32), list(GDI32), list(OPENGL32), list(GLU32));
    let iat = b
        .imports(&[
            ("USER32.DLL", &user),
            ("GDI32.DLL", &gdi),
            ("OPENGL32.DLL", &gl),
            ("GLU32.DLL", &glu),
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
    machine_with(test, voodoo3.then_some(Board::Voodoo3), program)
}

/// A machine with the built-in Windows on, `program` loaded from the
/// shell as T.EXE, and the 3dfx card `card`: a Voodoo3 is the display
/// adapter, a Voodoo2 a card beside the VGA.
fn machine_with(test: &str, card: Option<Board>, program: Vec<u8>) -> Cpu {
    let adapter = (card == Some(Board::Voodoo3)).then_some(Adapter::Voodoo3);
    boot(test, adapter, card, program)
}

/// A machine with the built-in Windows on, `program` loaded from the
/// shell as T.EXE, `adapter` as its display adapter (the VGA when
/// None), and the 3dfx card `voodoo`.
fn boot(test: &str, adapter: Option<Adapter>, voodoo: Option<Board>, program: Vec<u8>) -> Cpu {
    let base = PathBuf::from("target/test_win32_opengl").join(test);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join("T.EXE"), program).unwrap();
    let mut cpu = Cpu::new(base);
    if let Some(adapter) = adapter {
        let setup = VideoSetup {
            adapter,
            mono_monitor: false,
        };
        rust_dos::video::bios::install(&mut cpu.bus, setup);
    }
    if voodoo.is_some() {
        cpu.bus.configure_voodoo(voodoo);
    }
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    cpu
}

/// The display adapter that draws a test's scene.
#[derive(Clone, Copy, PartialEq)]
enum Draws {
    Voodoo3,
    ToadForce,
}

/// A machine with the built-in Windows on, `program` loaded from the
/// shell as T.EXE, and `card` as its display adapter.
fn machine_on(test: &str, card: Draws, program: Vec<u8>) -> Cpu {
    match card {
        Draws::Voodoo3 => machine(test, true, program),
        Draws::ToadForce => boot(
            &format!("{test}_toadforce"),
            Some(Adapter::ToadForce),
            None,
            program,
        ),
    }
}

/// On a ToadForce, the card drew and took every command its drivers
/// gave it.
fn card_drew(cpu: &Cpu, card: Draws) {
    if card != Draws::ToadForce {
        return;
    }
    let counts = cpu.bus.toadforce.as_ref().expect("a ToadForce").counts();
    assert_eq!(counts.rejected, 0, "{counts:?}");
    assert!(counts.primitives > 0, "{counts:?}");
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
const GL_POINTS: u32 = 0;
const GL_TRIANGLES: u32 = 4;
const GL_TRIANGLE_FAN: u32 = 6;
const GL_FLAT: u32 = 0x1D00;
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
/// make a context current on it: six results.
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
    shaded_triangle(Draws::Voodoo3);
}

#[test]
fn a_context_on_a_toadforce_draws_a_shaded_triangle_and_reads_it_back() {
    shaded_triangle(Draws::ToadForce);
}

fn shaded_triangle(card: Draws) {
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
    let mut cpu = machine_on("triangle", card, exe);
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
    if card == Draws::Voodoo3 {
        assert_eq!(peek_string(&cpu, r[6]), "3Dfx Interactive Inc.");
        assert!(peek_string(&cpu, r[7]).starts_with("3Dfx/Voodoo3"));
    } else {
        assert_eq!(peek_string(&cpu, r[6]), "Rust-DOS");
        assert_eq!(peek_string(&cpu, r[7]), "ToadForce/64 MB");
    }
    card_drew(&cpu, card);
    let extensions = peek_string(&cpu, r[8]);
    assert!(extensions.contains("GL_ARB_multitexture"));
    // As 3dfx's Voodoo3 ICD has it; Myth II 1.8.3 needs it of OpenGL 1.1.
    assert!(extensions.split(' ').any(|e| e == "GL_EXT_bgra"));
    assert_ne!(r[9], 0, "glActiveTextureARB");
    // SwapBuffers, and no error.
    assert_eq!(r[19], 1, "SwapBuffers");
    assert_eq!(r[20], 0, "glGetError");
    assert_eq!(pixel(&cpu, data, 4, 4), [255, 0, 0]);
    assert_eq!(pixel(&cpu, data, 60, 60), [0, 0, 255]);
    if card == Draws::ToadForce {
        // The window's buffers are of the desktop's format, 8 bits a
        // channel, so SwapBuffers is a copy of the card's (`BLT`) and
        // the processor reads nothing of the back buffer. The screen
        // has the triangle's red and the clear's blue, half of the
        // window's 64x64 pixels each.
        cpu.bus.toadforce_flush();
        let counts = cpu.bus.toadforce.as_ref().unwrap().counts();
        assert!(counts.blt >= 1, "{counts:?}");
        let screen = &cpu.bus.vbe.vram[..640 * 480 * 4];
        let count = |colour: u32| {
            let pixels = screen.as_chunks::<4>().0.iter();
            let same = |p: &&[u8; 4]| u32::from_le_bytes([p[0], p[1], p[2], 0]) == colour;
            pixels.filter(same).count()
        };
        assert!((1900..2200).contains(&count(0xFF_0000)), "red");
        assert!((1900..2200).contains(&count(0xFF)), "blue");
    }
}

#[test]
fn the_color_mask_on_a_toadforce_writes_the_channels_it_lets_through() {
    let (exe, data) = program(&data(), |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c)?;
        let blue = [Arg::F(0.0), Arg::F(0.0), Arg::F(1.0), Arg::F(1.0)];
        c.api(a, l, CLEAR_COLOR, &blue)?;
        c.api(a, l, CLEAR, &[Arg::I(GL_COLOR_BUFFER_BIT)])?;
        // White with only red let through over the bottom left half,
        // then green with nothing let through over the top right half.
        let red_only = [Arg::I(1), Arg::I(0), Arg::I(0), Arg::I(1)];
        c.api(a, l, COLOR_MASK, &red_only)?;
        triangle(a, l, &mut c, [1.0, 1.0, 1.0], BOTTOM_LEFT)?;
        c.api(a, l, COLOR_MASK, &[Arg::I(0); 4])?;
        triangle(a, l, &mut c, [0.0, 1.0, 0.0], TOP_RIGHT)?;
        read_back(a, l, &mut c)?;
        finish(a, l)
    });
    let mut cpu = machine_on("color_mask", Draws::ToadForce, exe);
    run_until_done(&mut cpu, data);
    card_drew(&cpu, Draws::ToadForce);
    // Six results of `start`, two of the clear, a mask and a
    // triangle's six twice, glReadPixels and SwapBuffers.
    let error = peek(&cpu, data + RESULTS + 4 * (6 + 2 + 2 * 7 + 2));
    assert_eq!(error, 0, "glGetError");
    // Red written over the blue, which stays.
    assert_eq!(pixel(&cpu, data, 4, 4), [255, 0, 255]);
    assert_eq!(pixel(&cpu, data, 60, 60), [0, 0, 255]);
}

#[test]
fn draws_of_one_state_on_a_toadforce_reach_direct3d_as_one() {
    // Three triangles, the last over the first, with a call of the
    // function `between` after the first: how often Direct3D's HAL
    // set the card up for a draw.
    let binds = |name: &str, between: Option<(usize, u32)>| {
        let (exe, data) = program(&data(), |a, l| {
            let mut c = Calls { next: 0 };
            start(a, l, &mut c)?;
            let both = [Arg::I(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT)];
            c.api(a, l, CLEAR, &both)?;
            triangle(a, l, &mut c, [1.0, 0.0, 0.0], BOTTOM_LEFT)?;
            if let Some((function, cap)) = between {
                c.api(a, l, function, &[Arg::I(cap)])?;
            }
            triangle(a, l, &mut c, [0.0, 1.0, 0.0], TOP_RIGHT)?;
            triangle(a, l, &mut c, [0.0, 0.0, 1.0], BOTTOM_LEFT)?;
            read_back(a, l, &mut c)?;
            finish(a, l)
        });
        let mut cpu = machine_on(name, Draws::ToadForce, exe);
        run_until_done(&mut cpu, data);
        card_drew(&cpu, Draws::ToadForce);
        // In the order they were given: the third over the first.
        assert_eq!(pixel(&cpu, data, 4, 4), [0, 0, 255], "{name}");
        assert_eq!(pixel(&cpu, data, 60, 60), [0, 255, 0], "{name}");
        rust_dos::win32::snapshot::ddraw(&cpu).binds[0]
    };
    assert_eq!(binds("waiting_one", None), 1);
    // A capability set to what it is leaves the state the same.
    assert_eq!(binds("waiting_same", Some((DISABLE, GL_DEPTH_TEST))), 1);
    // With another state the draws after it are a draw of their own.
    assert_eq!(binds("waiting_two", Some((ENABLE, GL_DEPTH_TEST))), 2);
}

#[test]
fn a_draw_on_a_toadforce_has_its_texture_as_it_was_when_it_was_given() {
    let mut d = data();
    for i in 0..4 {
        put(&mut d, TEXELS + 4 * i, &[255, 0, 0, 255]);
        put(&mut d, TEXELS + 0x40 + 4 * i, &[0, 255, 0, 255]);
    }
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
        c.api(a, l, GEN_TEXTURES, &[Arg::I(1), Arg::At(TEXTURE)])?;
        c.api(a, l, ENABLE_CLIENT_STATE, &[Arg::I(GL_VERTEX_ARRAY)])?;
        let vertices = [Arg::I(3), Arg::I(GL_FLOAT), Arg::I(0), Arg::At(VERTICES)];
        c.api(a, l, VERTEX_POINTER, &vertices)?;
        c.api(a, l, ENABLE_CLIENT_STATE, &[Arg::I(GL_TEXTURE_COORD_ARRAY)])?;
        let coords = [Arg::I(2), Arg::I(GL_FLOAT), Arg::I(0), Arg::At(TEX_COORDS)];
        c.api(a, l, TEX_COORD_POINTER, &coords)?;
        let replace = [
            Arg::I(GL_TEXTURE_ENV),
            Arg::I(GL_TEXTURE_ENV_MODE),
            Arg::I(GL_REPLACE),
        ];
        let quad = [
            Arg::I(GL_TRIANGLES),
            Arg::I(6),
            Arg::I(GL_UNSIGNED_INT),
            Arg::At(INDICES),
        ];
        // The window in red texels, then the texture's image again in
        // green and the right half of the window with that.
        texture_2x2(a, l, &mut c, TEXTURE, TEXELS)?;
        c.api(a, l, TEX_ENVI, &replace)?;
        c.api(a, l, DRAW_ELEMENTS, &quad)?;
        texture_2x2(a, l, &mut c, TEXTURE, TEXELS + 0x40)?;
        let right = [Arg::I(32), Arg::I(0), Arg::I(32), Arg::I(64)];
        c.api(a, l, VIEWPORT, &right)?;
        c.api(a, l, DRAW_ELEMENTS, &quad)?;
        read_back(a, l, &mut c)?;
        finish(a, l)
    });
    let mut cpu = machine_on("waiting_texture", Draws::ToadForce, exe);
    run_until_done(&mut cpu, data);
    card_drew(&cpu, Draws::ToadForce);
    assert_eq!(pixel(&cpu, data, 16, 32), [255, 0, 0]);
    assert_eq!(pixel(&cpu, data, 48, 32), [0, 255, 0]);
}

/// Draw a triangle of color `rgb` with the corners `corners`.
fn triangle(
    a: &mut CodeAssembler,
    l: &Layout,
    c: &mut Calls,
    rgb: [f32; 3],
    corners: [[f32; 2]; 3],
) -> Result<(), IcedError> {
    c.api(a, l, BEGIN, &[Arg::I(GL_TRIANGLES)])?;
    let [r, g, b] = rgb.map(Arg::F);
    c.api(a, l, COLOR3F, &[r, g, b])?;
    for [x, y] in corners {
        c.api(a, l, VERTEX2F, &[Arg::F(x), Arg::F(y)])?;
    }
    c.api(a, l, END, &[])
}

/// The bottom left half of the window, and the top right half.
const BOTTOM_LEFT: [[f32; 2]; 3] = [[-1.0, -1.0], [1.0, -1.0], [-1.0, 1.0]];
const TOP_RIGHT: [[f32; 2]; 3] = [[1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];

/// A program that clears the color and depth buffers and draws a red
/// triangle over the bottom left half and, with `clear`, then clears
/// the color buffer to blue and the depth buffer, before it draws a
/// green triangle over the top right half. The depth test is on for the
/// triangles with `depth_test`; without it the frames are drawn as Oni's
/// menus draw theirs.
fn two_triangles(clear: bool, depth_test: bool) -> (Cpu, u32) {
    let (exe, data) = program(&data(), |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c)?;
        let both = [Arg::I(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT)];
        c.api(a, l, CLEAR, &both)?;
        if depth_test {
            c.api(a, l, ENABLE, &[Arg::I(GL_DEPTH_TEST)])?;
        }
        triangle(a, l, &mut c, [1.0, 0.0, 0.0], BOTTOM_LEFT)?;
        if clear {
            let blue = [Arg::F(0.0), Arg::F(0.0), Arg::F(1.0), Arg::F(1.0)];
            c.api(a, l, CLEAR_COLOR, &blue)?;
            c.api(a, l, CLEAR, &both)?;
        }
        triangle(a, l, &mut c, [0.0, 1.0, 0.0], TOP_RIGHT)?;
        read_back(a, l, &mut c)?;
        finish(a, l)
    });
    let name = format!("two_triangles_{clear}_{depth_test}");
    let mut cpu = machine(&name, true, exe);
    run_until_done(&mut cpu, data);
    (cpu, data)
}

/// The card's colBufferAddr, colBufferStride, auxBufferAddr and
/// auxBufferStride.
fn buffer_registers(cpu: &Cpu) -> [u32; 4] {
    use rust_dos::voodoo::banshee::render3d::{
        AUX_BUFFER_ADDR, AUX_BUFFER_STRIDE, COL_BUFFER_ADDR, COL_BUFFER_STRIDE,
    };
    let v = cpu.bus.voodoo.as_ref().unwrap();
    [COL_BUFFER_ADDR, COL_BUFFER_STRIDE, AUX_BUFFER_ADDR, AUX_BUFFER_STRIDE].map(|r| v.reg[r])
}

#[test]
fn a_depth_clear_without_the_depth_test_gives_the_card_the_depth_buffer() {
    // The card's fastfill clears the depth buffer, and the card keeps
    // it as its depth buffer, where drawing with the depth test on puts
    // it: pointing the card at it for the fill alone and back would move
    // the buffer in the OpenGL renderer's layout at every clear.
    let (cleared, data) = two_triangles(true, false);
    assert_eq!(peek(&cleared, data + RESULTS + 4 * 23), 0, "glGetError");
    assert_eq!(pixel(&cleared, data, 4, 4), [0, 0, 255]);
    assert_eq!(pixel(&cleared, data, 60, 60), [0, 255, 0]);
    let (plain, _) = two_triangles(false, false);
    let (depth_tested, data) = two_triangles(false, true);
    assert_eq!(pixel(&depth_tested, data, 4, 4), [255, 0, 0]);
    let registers = buffer_registers(&cleared);
    assert_eq!(registers, buffer_registers(&depth_tested));
    assert_ne!(registers[2..], buffer_registers(&plain)[2..]);
}

/// A program that draws a red triangle with the depth test on in one
/// window and then, with `second`, makes a context in another window,
/// clears its color and depth buffers and draws a green triangle in it,
/// with the depth test on if `second` is true.
fn two_contexts(second: Option<bool>) -> (Cpu, u32) {
    let (exe, data) = program(&data(), |a, l| {
        let mut c = Calls { next: 0 };
        let both = [Arg::I(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT)];
        start(a, l, &mut c)?;
        c.api(a, l, ENABLE, &[Arg::I(GL_DEPTH_TEST)])?;
        c.api(a, l, CLEAR, &both)?;
        triangle(a, l, &mut c, [1.0, 0.0, 0.0], BOTTOM_LEFT)?;
        if let Some(depth_test) = second {
            start(a, l, &mut c)?;
            c.api(a, l, CLEAR, &both)?;
            if depth_test {
                c.api(a, l, ENABLE, &[Arg::I(GL_DEPTH_TEST)])?;
            }
            triangle(a, l, &mut c, [0.0, 1.0, 0.0], TOP_RIGHT)?;
        }
        read_back(a, l, &mut c)?;
        finish(a, l)
    });
    let name = format!("two_contexts_{second:?}");
    let mut cpu = machine(&name, true, exe);
    run_until_done(&mut cpu, data);
    (cpu, data)
}

#[test]
fn a_depth_clear_without_the_depth_test_takes_the_card_from_another_depth_buffer() {
    // The card has the first context's depth buffer from its drawing
    // when the second clears its own and draws with the test off: the
    // card keeps the second's, as when it draws with the test on.
    let (cleared, data) = two_contexts(Some(false));
    assert_eq!(pixel(&cleared, data, 60, 60), [0, 255, 0]);
    let (depth_tested, _) = two_contexts(Some(true));
    let (first, _) = two_contexts(None);
    let registers = buffer_registers(&cleared);
    assert_eq!(registers, buffer_registers(&depth_tested));
    assert_ne!(registers[2..], buffer_registers(&first)[2..]);
}

#[test]
fn a_window_swaps_its_buffers_once_a_frame() {
    // The swap interval is 1, as 3dfx's ICD has it: of ten swaps in a
    // row the first shows at once, and each of the others comes back
    // when the next frame's picture begins, 60 Hz frames of the 640x480
    // desktop apart.
    let (exe, data) = program(&data(), |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c)?;
        c.api(a, l, CLEAR, &[Arg::I(GL_COLOR_BUFFER_BIT)])?;
        for _ in 0..10 {
            c.api(a, l, SWAP_BUFFERS, &[Arg::Val(HDC)])?;
        }
        finish(a, l)
    });
    let mut cpu = machine("swap_interval", true, exe);
    // When each swap's result is stored, run in small batches.
    let mut times = Vec::new();
    for _ in 0..1_000_000 {
        if times.len() == 10 {
            break;
        }
        cpu.bus.start_batch(cpu.bus.clock.icount + 100);
        run_batch(&mut cpu, &mut NoHook, false);
        if cpu.cr0 & 0x8000_0001 != 0x8000_0001 {
            continue;
        }
        while times.len() < 10 && peek(&cpu, data + RESULTS + 4 * (7 + times.len() as u32)) != 0 {
            times.push(cpu.bus.clock.now_ns());
        }
    }
    let swaps: Vec<u32> = (7..17)
        .map(|i| peek(&cpu, data + RESULTS + 4 * i))
        .collect();
    assert_eq!(swaps, [1; 10], "SwapBuffers");
    // The desktop's frame, about 16.7 ms.
    let frame = cpu.bus.vga.peek_timing().frame_ns();
    assert!((16_600_000..16_800_000).contains(&frame), "{frame} ns");
    // A batch's hundred instructions late at most.
    let late = 100_000;
    assert!(times[1] - times[0] < frame + late, "{times:?}");
    for pair in times[1..].windows(2) {
        assert!(pair[1] - pair[0] < frame + late, "{times:?}");
        assert!(pair[1] - pair[0] > frame - late, "{times:?}");
    }
    for &t in &times[1..] {
        assert!(t % frame < late, "{t} ns: past a frame's start, {times:?}");
    }
}

#[test]
fn a_negative_swap_interval_fails_and_the_last_one_stays() {
    // WGL_EXT_swap_control: a negative interval fails. Kept, -1 would
    // have the next swap wait 2^32 - 2 frames.
    const SET_NAME: u32 = 0x160;
    const GET_NAME: u32 = 0x180;
    const PROCS: u32 = 0x10;
    let mut d = data();
    put(&mut d, SET_NAME, b"wglSwapIntervalEXT\0");
    put(&mut d, GET_NAME, b"wglGetSwapIntervalEXT\0");
    let (exe, data) = program(&d, |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c)?;
        c.api(a, l, WGL_GET_PROC_ADDRESS, &[Arg::At(SET_NAME)])?;
        a.mov(dword_ptr(l.at(PROCS)), eax)?;
        c.api(a, l, WGL_GET_PROC_ADDRESS, &[Arg::At(GET_NAME)])?;
        a.mov(dword_ptr(l.at(PROCS + 4)), eax)?;
        for interval in [2, -1] {
            a.push(interval)?;
            a.call(dword_ptr(l.at(PROCS)))?;
            a.mov(dword_ptr(l.at(RESULTS + 4 * c.next)), eax)?;
            c.next += 1;
        }
        a.call(dword_ptr(l.at(PROCS + 4)))?;
        a.mov(dword_ptr(l.at(RESULTS + 4 * c.next)), eax)?;
        finish(a, l)
    });
    let mut cpu = machine("negative_swap_interval", true, exe);
    run_until_done(&mut cpu, data);
    let r: Vec<u32> = (6..11)
        .map(|i| peek(&cpu, data + RESULTS + 4 * i))
        .collect();
    assert_ne!(r[0], 0, "wglSwapIntervalEXT");
    assert_ne!(r[1], 0, "wglGetSwapIntervalEXT");
    assert_eq!(r[2..], [1, 0, 2], "set 2, set -1, get");
}

#[test]
fn flat_shading_takes_the_color_of_the_vertex_opengl_gives() {
    // Two flat triangles, each vertex its own color: OpenGL colors an
    // independent triangle as its third vertex, and a fan's triangle
    // as its third too (Direct3D: the first and the second).
    let (exe, data) = program(&data(), |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c)?;
        let black = [Arg::F(0.0), Arg::F(0.0), Arg::F(0.0), Arg::F(1.0)];
        c.api(a, l, CLEAR_COLOR, &black)?;
        c.api(a, l, CLEAR, &[Arg::I(GL_COLOR_BUFFER_BIT)])?;
        c.api(a, l, SHADE_MODEL, &[Arg::I(GL_FLAT)])?;
        let vertex = |a: &mut CodeAssembler,
                      c: &mut Calls,
                      rgb: [f32; 3],
                      xy: [f32; 2]|
         -> Result<(), IcedError> {
            let [r, g, b] = rgb.map(Arg::F);
            c.api(a, l, COLOR3F, &[r, g, b])?;
            c.api(a, l, VERTEX2F, &[Arg::F(xy[0]), Arg::F(xy[1])])
        };
        // Red, green, blue over the bottom left half.
        c.api(a, l, BEGIN, &[Arg::I(GL_TRIANGLES)])?;
        vertex(a, &mut c, [1.0, 0.0, 0.0], [-1.0, -1.0])?;
        vertex(a, &mut c, [0.0, 1.0, 0.0], [1.0, -1.0])?;
        vertex(a, &mut c, [0.0, 0.0, 1.0], [-1.0, 1.0])?;
        c.api(a, l, END, &[])?;
        // Red, green, white over the top right half.
        c.api(a, l, BEGIN, &[Arg::I(GL_TRIANGLE_FAN)])?;
        vertex(a, &mut c, [1.0, 0.0, 0.0], [1.0, -1.0])?;
        vertex(a, &mut c, [0.0, 1.0, 0.0], [1.0, 1.0])?;
        vertex(a, &mut c, [1.0, 1.0, 1.0], [-1.0, 1.0])?;
        c.api(a, l, END, &[])?;
        read_back(a, l, &mut c)?;
        finish(a, l)
    });
    let mut cpu = machine("flat", true, exe);
    run_until_done(&mut cpu, data);
    // The last result is glGetError's.
    assert_eq!(peek(&cpu, data + RESULTS + 4 * 27), 0, "glGetError");
    assert_eq!(pixel(&cpu, data, 4, 4), [0, 0, 255]);
    assert_eq!(pixel(&cpu, data, 60, 60), [255, 255, 255]);
}

#[test]
fn a_point_on_a_pixels_centre_lights_that_pixel() {
    // OpenGL puts a pixel's centre half a pixel right of and above its
    // window coordinates, where the card has it too: a point at window
    // (10.5, 10.5) is pixel (10, 10). Moved the half pixel Direct3D's
    // vertices move by, it would light the pixel right of and below it.
    let (exe, data) = program(&data(), |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c)?;
        let black = [Arg::F(0.0), Arg::F(0.0), Arg::F(0.0), Arg::F(1.0)];
        c.api(a, l, CLEAR_COLOR, &black)?;
        c.api(a, l, CLEAR, &[Arg::I(GL_COLOR_BUFFER_BIT)])?;
        c.api(a, l, BEGIN, &[Arg::I(GL_POINTS)])?;
        c.api(a, l, COLOR3F, &[Arg::F(1.0), Arg::F(0.0), Arg::F(0.0)])?;
        // 10.5 of the 64 pixels: -1 + 10.5 / 32.
        let at = Arg::F(-0.671875);
        c.api(a, l, VERTEX2F, &[at, at])?;
        c.api(a, l, END, &[])?;
        read_back(a, l, &mut c)?;
        finish(a, l)
    });
    let mut cpu = machine("point_centre", true, exe);
    run_until_done(&mut cpu, data);
    assert_eq!(pixel(&cpu, data, 10, 10), [255, 0, 0]);
    assert_eq!(pixel(&cpu, data, 11, 9), [0, 0, 0]);
}

#[test]
fn a_triangle_reaching_far_off_the_window_is_drawn_where_it_covers_it() {
    // A red triangle whose bottom right corner is a hundred widths of
    // the view to the right: thousands of pixels past the window, past
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
    textured_depth_tested_quad(Draws::Voodoo3);
}

#[test]
fn vertex_arrays_on_a_toadforce_draw_a_textured_depth_tested_quad() {
    textured_depth_tested_quad(Draws::ToadForce);
}

fn textured_depth_tested_quad(card: Draws) {
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
    let mut cpu = machine_on("arrays", card, exe);
    run_until_done(&mut cpu, data);
    card_drew(&cpu, card);
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
fn without_a_3dfx_card_there_is_no_pixel_format() {
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

#[test]
fn on_a_voodoo2_a_context_draws_full_screen_on_the_card() {
    let (exe, data) = program(&data(), |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c)?;
        c.api(a, l, GET_STRING, &[Arg::I(GL_RENDERER)])?;
        let blue = [Arg::F(0.0), Arg::F(0.0), Arg::F(1.0), Arg::F(1.0)];
        c.api(a, l, CLEAR_COLOR, &blue)?;
        c.api(
            a,
            l,
            CLEAR,
            &[Arg::I(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT)],
        )?;
        // A red triangle over the bottom left half of the window.
        c.api(a, l, BEGIN, &[Arg::I(GL_TRIANGLES)])?;
        c.api(a, l, COLOR3F, &[Arg::F(1.0), Arg::F(0.0), Arg::F(0.0)])?;
        c.api(a, l, VERTEX2F, &[Arg::F(-1.0), Arg::F(-1.0)])?;
        c.api(a, l, VERTEX2F, &[Arg::F(1.0), Arg::F(-1.0)])?;
        c.api(a, l, VERTEX2F, &[Arg::F(-1.0), Arg::F(1.0)])?;
        c.api(a, l, END, &[])?;
        read_back(a, l, &mut c)?;
        finish(a, l)
    });
    let mut cpu = machine_with("voodoo2", Some(Board::Voodoo2), exe);
    run_until_done(&mut cpu, data);
    let r: Vec<u32> = (0..18)
        .map(|i| peek(&cpu, data + RESULTS + 4 * i))
        .collect();
    // 3dfx's Voodoo2 ICD: its double-buffered format for 32-bit color,
    // a context, made current.
    assert_eq!(r[2], 2, "the double-buffered pixel format");
    assert_eq!(r[3], 1, "SetPixelFormat");
    assert_ne!(r[4], 0, "wglCreateContext");
    assert_eq!(r[5], 1, "wglMakeCurrent");
    assert_eq!(peek_string(&cpu, r[6]), "3Dfx/Voodoo2/2 TMUs/4 MB");
    assert_eq!(r[16], 1, "SwapBuffers");
    assert_eq!(r[17], 0, "glGetError");
    // The window is the top left of the card's screen, which is in its
    // smallest mode, 512x384, and shows instead of the VGA's.
    let v = cpu.bus.voodoo.as_ref().unwrap();
    assert!(v.output(), "the card shows its picture");
    assert_eq!((v.fbi.width, v.fbi.height), (512, 384));
    assert_eq!(pixel(&cpu, data, 4, 4), [255, 0, 0]);
    assert_eq!(pixel(&cpu, data, 60, 60), [0, 0, 255]);
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
    lightmapped_wall(Draws::Voodoo3);
}

#[test]
fn two_texture_units_on_a_toadforce_modulate_a_lightmap() {
    lightmapped_wall(Draws::ToadForce);
}

fn lightmapped_wall(card: Draws) {
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
    let mut cpu = machine_on("multitexture", card, exe);
    run_until_done(&mut cpu, data);
    card_drew(&cpu, card);
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

const GLU_VERSION: u32 = 100_800;
const GLU_EXTENSIONS: u32 = 100_801;
const GLU_INVALID_ENUM: u32 = 100_900;
const GL_DEPTH_COMPONENT: u32 = 0x1902;

#[test]
fn glu_builds_mipmaps_of_an_image_of_no_power_of_two_and_sets_up_2d() {
    let mut d = data();
    // A 6x6 red image of RGB bytes, its rows of 18 bytes padded to 20
    // by the unpack alignment. glTexImage2D refuses a size that isn't
    // a power of two; gluBuild2DMipmaps scales it to 8x8 first.
    for y in 0..6 {
        for x in 0..6 {
            put(&mut d, TEXELS + 20 * y + 3 * x, &[255, 0, 0]);
        }
    }
    // A quad from (0, 0) to (1, 1), which gluOrtho2D(0, 2, 0, 2) puts
    // over the bottom left quarter of the window.
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
        let blue = [Arg::F(0.0), Arg::F(0.0), Arg::F(1.0), Arg::F(1.0)];
        c.api(a, l, CLEAR_COLOR, &blue)?;
        c.api(a, l, CLEAR, &[Arg::I(GL_COLOR_BUFFER_BIT)])?;
        c.api(a, l, GEN_TEXTURES, &[Arg::I(1), Arg::At(TEXTURE)])?;
        let bind = [Arg::I(GL_TEXTURE_2D), Arg::Val(TEXTURE)];
        c.api(a, l, BIND_TEXTURE, &bind)?;
        // 10: the mipmaps, 0.
        let image = |format| {
            [
                Arg::I(GL_TEXTURE_2D),
                Arg::I(3),
                Arg::I(6),
                Arg::I(6),
                Arg::I(format),
                Arg::I(GL_UNSIGNED_BYTE),
                Arg::At(TEXELS),
            ]
        };
        c.api(a, l, BUILD_2D_MIPMAPS, &image(GL_RGB))?;
        // 11: a format GLU refuses, GLU_INVALID_ENUM.
        c.api(a, l, BUILD_2D_MIPMAPS, &image(GL_DEPTH_COMPONENT))?;
        // 12 to 14: the texts.
        c.api(a, l, ERROR_STRING, &[Arg::I(GLU_INVALID_ENUM)])?;
        c.api(a, l, GLU_GET_STRING, &[Arg::I(GLU_VERSION)])?;
        c.api(a, l, GLU_GET_STRING, &[Arg::I(GLU_EXTENSIONS)])?;
        let mut ortho = Vec::new();
        for v in [0.0, 2.0, 0.0, 2.0] {
            ortho.extend(double(v));
        }
        c.api(a, l, ORTHO_2D, &ortho)?;
        let env = [
            Arg::I(GL_TEXTURE_ENV),
            Arg::I(GL_TEXTURE_ENV_MODE),
            Arg::I(GL_REPLACE),
        ];
        c.api(a, l, TEX_ENVI, &env)?;
        c.api(a, l, ENABLE, &[Arg::I(GL_TEXTURE_2D)])?;
        c.api(a, l, ENABLE_CLIENT_STATE, &[Arg::I(GL_VERTEX_ARRAY)])?;
        c.api(a, l, ENABLE_CLIENT_STATE, &[Arg::I(GL_TEXTURE_COORD_ARRAY)])?;
        let vertices = [Arg::I(3), Arg::I(GL_FLOAT), Arg::I(0), Arg::At(VERTICES)];
        c.api(a, l, VERTEX_POINTER, &vertices)?;
        let coords = [Arg::I(2), Arg::I(GL_FLOAT), Arg::I(0), Arg::At(TEX_COORDS)];
        c.api(a, l, TEX_COORD_POINTER, &coords)?;
        let quad = [
            Arg::I(GL_QUADS),
            Arg::I(4),
            Arg::I(GL_UNSIGNED_SHORT),
            Arg::At(INDICES),
        ];
        c.api(a, l, DRAW_ELEMENTS, &quad)?;
        read_back(a, l, &mut c)?;
        finish(a, l)
    });
    let mut cpu = machine("glu", true, exe);
    run_until_done(&mut cpu, data);
    let r = |i: u32| peek(&cpu, data + RESULTS + 4 * i);
    assert_eq!(r(10), 0, "gluBuild2DMipmaps");
    assert_eq!(r(11), GLU_INVALID_ENUM, "a depth image");
    assert_eq!(peek_string(&cpu, r(12)), "invalid enumerant");
    assert_eq!(peek_string(&cpu, r(13)), "1.2.2.0 Microsoft Corporation");
    // The built-in OpenGL lists GL_EXT_bgra on a Voodoo3, as 3dfx's ICD
    // does, so GLU does too.
    assert_eq!(peek_string(&cpu, r(14)), "GL_EXT_bgra");
    assert_eq!(r(25), 0, "glGetError");
    // The red texture in the bottom left quarter, the clear color
    // elsewhere.
    assert_eq!(pixel(&cpu, data, 8, 8), [255, 0, 0]);
    assert_eq!(pixel(&cpu, data, 24, 24), [255, 0, 0]);
    assert_eq!(pixel(&cpu, data, 48, 48), [0, 0, 255]);
    assert_eq!(pixel(&cpu, data, 48, 8), [0, 0, 255]);
}

// Lighting (OpenGL 1.1, section 2.13).
const GL_LIGHTING: u32 = 0x0B50;
const GL_LIGHT0: u32 = 0x4000;
const GL_DITHER: u32 = 0x0BD0;
const GL_COLOR_MATERIAL: u32 = 0x0B57;
const GL_DIFFUSE: u32 = 0x1201;
const GL_POSITION: u32 = 0x1203;
const GL_SPOT_DIRECTION: u32 = 0x1204;
const GL_SPOT_CUTOFF: u32 = 0x1206;
const GL_EMISSION: u32 = 0x1600;
const GL_FRONT: u32 = 0x0404;
const GL_BACK: u32 = 0x0405;
const GL_LIGHT_MODEL_TWO_SIDE: u32 = 0x0B52;
const GL_LIGHT_MODEL_AMBIENT: u32 = 0x0B53;
const GL_LIGHTING_BIT: u32 = 0x40;
const GL_SHORT: u32 = 0x1402;
const GL_NORMAL_ARRAY: u32 = 0x8075;
const GL_INVALID_VALUE: u32 = 0x0501;
const GL_INVALID_OPERATION: u32 = 0x0502;

/// Where the lighting tests keep the vectors they pass, and where the
/// queries write theirs.
const VECTORS: u32 = 0x600;
const OUT: u32 = 0x700;

/// A color read back from the 5-6-5 color buffer is `want` (0 to 1)
/// within its steps.
fn near(got: [u8; 3], want: [f32; 3]) {
    for k in 0..3 {
        let w = want[k] * 255.0;
        assert!(
            (got[k] as f32 - w).abs() <= 9.0,
            "{got:?} is not near {:?}",
            want.map(|x| x * 255.0)
        );
    }
}

/// The 4 floats of the program's data at `offset`.
fn floats(cpu: &Cpu, data: u32, offset: u32) -> [f32; 4] {
    std::array::from_fn(|i| f32::from_bits(peek(cpu, data + offset + 4 * i as u32)))
}

#[test]
fn lighting_colors_vertices_by_their_normals_from_calls_and_arrays() {
    let mut d = data();
    // Light 0's diffuse color.
    put_f32s(&mut d, VECTORS, &[1.0, 0.5, 0.25, 1.0]);
    // The top right triangle for the arrays, with shorts for normals
    // straight along z: 32767 is (2 * 32767 + 1) / 65535 = 1.
    put_f32s(
        &mut d,
        VERTICES,
        &[1.0, -1.0, 0.0, 1.0, 1.0, 0.0, -1.0, 1.0, 0.0],
    );
    for i in 0..3 {
        put(&mut d, TEX_COORDS + 6 * i, &[0, 0, 0, 0, 0xFF, 0x7F]);
    }
    for (i, v) in [0u16, 1, 2].iter().enumerate() {
        put(&mut d, INDICES + 2 * i as u32, &v.to_le_bytes());
    }
    let mut error_at = 0;
    let (exe, data) = program(&d, |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c)?;
        c.api(a, l, DISABLE, &[Arg::I(GL_DITHER)])?;
        c.api(a, l, CLEAR, &[Arg::I(GL_COLOR_BUFFER_BIT)])?;
        let diffuse = [Arg::I(GL_LIGHT0), Arg::I(GL_DIFFUSE), Arg::At(VECTORS)];
        c.api(a, l, LIGHTFV, &diffuse)?;
        c.api(a, l, ENABLE, &[Arg::I(GL_LIGHTING)])?;
        c.api(a, l, ENABLE, &[Arg::I(GL_LIGHT0)])?;
        // The bottom left triangle: its normal at 0.8 to the light, which
        // is along z at infinity. The color doesn't count.
        c.api(a, l, BEGIN, &[Arg::I(GL_TRIANGLES)])?;
        c.api(a, l, COLOR3F, &[Arg::F(0.0), Arg::F(0.0), Arg::F(1.0)])?;
        c.api(a, l, NORMAL3F, &[Arg::F(0.0), Arg::F(0.6), Arg::F(0.8)])?;
        for [x, y] in BOTTOM_LEFT {
            c.api(a, l, VERTEX2F, &[Arg::F(x), Arg::F(y)])?;
        }
        c.api(a, l, END, &[])?;
        // The top right one from arrays, green taken for the ambient and
        // diffuse material by GL_COLOR_MATERIAL.
        c.api(a, l, COLOR3F, &[Arg::F(0.0), Arg::F(1.0), Arg::F(0.0)])?;
        c.api(a, l, ENABLE, &[Arg::I(GL_COLOR_MATERIAL)])?;
        c.api(a, l, ENABLE_CLIENT_STATE, &[Arg::I(GL_VERTEX_ARRAY)])?;
        c.api(a, l, ENABLE_CLIENT_STATE, &[Arg::I(GL_NORMAL_ARRAY)])?;
        let vertices = [Arg::I(3), Arg::I(GL_FLOAT), Arg::I(0), Arg::At(VERTICES)];
        c.api(a, l, VERTEX_POINTER, &vertices)?;
        let normals = [Arg::I(GL_SHORT), Arg::I(0), Arg::At(TEX_COORDS)];
        c.api(a, l, NORMAL_POINTER, &normals)?;
        let triangle = [
            Arg::I(GL_TRIANGLES),
            Arg::I(3),
            Arg::I(GL_UNSIGNED_SHORT),
            Arg::At(INDICES),
        ];
        c.api(a, l, DRAW_ELEMENTS, &triangle)?;
        read_back(a, l, &mut c)?;
        error_at = c.next - 1;
        finish(a, l)
    });
    let mut cpu = machine("lighting", true, exe);
    run_until_done(&mut cpu, data);
    assert_eq!(peek(&cpu, data + RESULTS + 4 * error_at), 0, "glGetError");
    // Equation 2.1: the scene's ambient 0.2 times the material's 0.2,
    // and n.L 0.8 times the material's diffuse 0.8 times the light's.
    let lit = [0.04 + 0.64, 0.04 + 0.64 * 0.5, 0.04 + 0.64 * 0.25];
    near(pixel(&cpu, data, 4, 4), lit);
    // Green for both: 0.2 of it from the scene's ambient, and the light's
    // green, 0.5, at n.L 1.
    near(pixel(&cpu, data, 60, 60), [0.0, 0.7, 0.0]);
}

/// A program that lights two triangles from behind them, a red front
/// material and a green back one: the bottom left triangle wound
/// counter-clockwise, so its front shows, and the top right one
/// clockwise, so its back shows. With `two_side`, lighting is
/// two-sided.
fn two_sided(two_side: bool) -> (Cpu, u32) {
    let mut d = data();
    put_f32s(&mut d, VECTORS, &[0.0, 0.0, -1.0, 0.0]);
    put_f32s(&mut d, VECTORS + 16, &[1.0, 0.0, 0.0, 1.0]);
    put_f32s(&mut d, VECTORS + 32, &[0.0, 1.0, 0.0, 1.0]);
    let (exe, data) = program(&d, |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c)?;
        c.api(a, l, DISABLE, &[Arg::I(GL_DITHER)])?;
        c.api(a, l, CLEAR, &[Arg::I(GL_COLOR_BUFFER_BIT)])?;
        let position = [Arg::I(GL_LIGHT0), Arg::I(GL_POSITION), Arg::At(VECTORS)];
        c.api(a, l, LIGHTFV, &position)?;
        let front = [Arg::I(GL_FRONT), Arg::I(GL_DIFFUSE), Arg::At(VECTORS + 16)];
        c.api(a, l, MATERIALFV, &front)?;
        let back = [Arg::I(GL_BACK), Arg::I(GL_DIFFUSE), Arg::At(VECTORS + 32)];
        c.api(a, l, MATERIALFV, &back)?;
        let model = [Arg::I(GL_LIGHT_MODEL_TWO_SIDE), Arg::I(two_side as u32)];
        c.api(a, l, LIGHT_MODELI, &model)?;
        c.api(a, l, ENABLE, &[Arg::I(GL_LIGHTING)])?;
        c.api(a, l, ENABLE, &[Arg::I(GL_LIGHT0)])?;
        c.api(a, l, NORMAL3F, &[Arg::F(0.0), Arg::F(0.0), Arg::F(1.0)])?;
        c.api(a, l, BEGIN, &[Arg::I(GL_TRIANGLES)])?;
        for [x, y] in BOTTOM_LEFT {
            c.api(a, l, VERTEX2F, &[Arg::F(x), Arg::F(y)])?;
        }
        for [x, y] in TOP_RIGHT.iter().rev() {
            c.api(a, l, VERTEX2F, &[Arg::F(*x), Arg::F(*y)])?;
        }
        c.api(a, l, END, &[])?;
        read_back(a, l, &mut c)?;
        finish(a, l)
    });
    let mut cpu = machine(&format!("two_sided_{two_side}"), true, exe);
    run_until_done(&mut cpu, data);
    (cpu, data)
}

#[test]
fn two_sided_lighting_lights_the_back_that_shows_with_the_back_material() {
    let ambient = [0.04; 3];
    // The front faces away from the light: the ambient term alone.
    let (cpu, data) = two_sided(true);
    near(pixel(&cpu, data, 4, 4), ambient);
    // The back faces it, and is the back material's green.
    near(pixel(&cpu, data, 60, 60), [0.04, 1.0, 0.04]);
    // One-sided, the back shows the front's color, of the front
    // material and the normal as given.
    let (cpu, data) = two_sided(false);
    near(pixel(&cpu, data, 4, 4), ambient);
    near(pixel(&cpu, data, 60, 60), ambient);
}

#[test]
fn lighting_queries_give_back_what_was_set() {
    let mut d = data();
    put_f32s(&mut d, VECTORS, &[1.0, 2.0, 3.0, 1.0]);
    put_f32s(&mut d, VECTORS + 16, &[1.0, 0.0, 0.0, 0.0]);
    let mut at = Vec::new();
    let (exe, data) = program(&d, |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c)?;
        let light1 = GL_LIGHT0 + 1;
        // The position and spot direction go through the modelview matrix
        // as they are given, a scale by 2 along x.
        c.api(a, l, SCALEF, &[Arg::F(2.0), Arg::F(1.0), Arg::F(1.0)])?;
        let position = [Arg::I(light1), Arg::I(GL_POSITION), Arg::At(VECTORS)];
        c.api(a, l, LIGHTFV, &position)?;
        let spot = [
            Arg::I(light1),
            Arg::I(GL_SPOT_DIRECTION),
            Arg::At(VECTORS + 16),
        ];
        c.api(a, l, LIGHTFV, &spot)?;
        c.api(a, l, LOAD_IDENTITY, &[])?;
        let get = |p, out| [Arg::I(light1), Arg::I(p), Arg::At(out)];
        c.api(a, l, GET_LIGHTFV, &get(GL_POSITION, OUT))?;
        c.api(a, l, GET_LIGHTFV, &get(GL_SPOT_DIRECTION, OUT + 16))?;
        // A cutoff over 90 that isn't 180 is refused.
        let cutoff = [Arg::I(light1), Arg::I(GL_SPOT_CUTOFF), Arg::F(100.0)];
        c.api(a, l, LIGHTF, &cutoff)?;
        at.push(c.next);
        c.api(a, l, GET_ERROR, &[])?;
        c.api(a, l, ENABLE, &[Arg::I(GL_LIGHT0 + 3)])?;
        at.push(c.next);
        c.api(a, l, IS_ENABLED, &[Arg::I(GL_LIGHT0 + 3)])?;
        c.api(a, l, IS_ENABLED, &[Arg::I(GL_LIGHT0 + 2)])?;
        // The front emission follows the current color.
        c.api(a, l, ENABLE, &[Arg::I(GL_COLOR_MATERIAL)])?;
        let track = [Arg::I(GL_FRONT), Arg::I(GL_EMISSION)];
        c.api(a, l, COLOR_MATERIAL, &track)?;
        c.api(a, l, COLOR3F, &[Arg::F(0.5), Arg::F(0.25), Arg::F(1.0)])?;
        let emission = |face, out| [Arg::I(face), Arg::I(GL_EMISSION), Arg::At(out)];
        c.api(a, l, GET_MATERIALFV, &emission(GL_FRONT, OUT + 32))?;
        c.api(a, l, GET_MATERIALFV, &emission(GL_BACK, OUT + 48))?;
        let ambient = [Arg::I(GL_LIGHT_MODEL_AMBIENT), Arg::At(OUT + 64)];
        c.api(a, l, GET_FLOATV, &ambient)?;
        // glPopAttrib puts back the lighting group.
        c.api(a, l, ENABLE, &[Arg::I(GL_LIGHTING)])?;
        c.api(a, l, PUSH_ATTRIB, &[Arg::I(GL_LIGHTING_BIT)])?;
        c.api(a, l, DISABLE, &[Arg::I(GL_LIGHTING)])?;
        let model = [Arg::I(GL_LIGHT_MODEL_TWO_SIDE), Arg::I(1)];
        c.api(a, l, LIGHT_MODELI, &model)?;
        c.api(a, l, POP_ATTRIB, &[])?;
        at.push(c.next);
        c.api(a, l, IS_ENABLED, &[Arg::I(GL_LIGHTING)])?;
        let two_side = [Arg::I(GL_LIGHT_MODEL_TWO_SIDE), Arg::At(OUT + 80)];
        c.api(a, l, GET_FLOATV, &two_side)?;
        c.api(a, l, GET_ERROR, &[])?;
        // A query between glBegin and glEnd is refused.
        c.api(a, l, BEGIN, &[Arg::I(GL_TRIANGLES)])?;
        c.api(a, l, GET_LIGHTFV, &get(GL_POSITION, OUT + 96))?;
        c.api(a, l, END, &[])?;
        at.push(c.next);
        c.api(a, l, GET_ERROR, &[])?;
        finish(a, l)
    });
    let mut cpu = machine("lighting_queries", true, exe);
    run_until_done(&mut cpu, data);
    let r = |i: u32| peek(&cpu, data + RESULTS + 4 * i);
    assert_eq!(floats(&cpu, data, OUT), [2.0, 2.0, 3.0, 1.0]);
    assert_eq!(floats(&cpu, data, OUT + 16)[..3], [2.0, 0.0, 0.0]);
    assert_eq!(r(at[0]), GL_INVALID_VALUE, "a cutoff of 100");
    assert_eq!([r(at[1]), r(at[1] + 1)], [1, 0], "lights 3 and 2 on");
    assert_eq!(floats(&cpu, data, OUT + 32), [0.5, 0.25, 1.0, 1.0]);
    assert_eq!(floats(&cpu, data, OUT + 48), [0.0, 0.0, 0.0, 1.0]);
    assert_eq!(floats(&cpu, data, OUT + 64), [0.2, 0.2, 0.2, 1.0]);
    assert_eq!(r(at[2]), 1, "lighting on again");
    assert_eq!(floats(&cpu, data, OUT + 80)[0], 0.0, "one-sided again");
    assert_eq!(r(at[2] + 2), 0, "glGetError");
    assert_eq!(r(at[3]), GL_INVALID_OPERATION, "glGetLightfv in glBegin");
    assert_eq!(floats(&cpu, data, OUT + 96), [0.0; 4], "nothing written");
}

#[test]
fn the_3d_trace_names_opengls_enums_and_counts_its_calls() {
    let (exe, data) = program(&data(), |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c)?;
        c.api(a, l, GET_STRING, &[Arg::I(GL_VENDOR)])?;
        c.api(a, l, ENABLE, &[Arg::I(GL_DEPTH_TEST)])?;
        c.api(a, l, DEPTH_FUNC, &[Arg::I(GL_LESS)])?;
        triangle(a, l, &mut c, [1.0, 0.0, 0.0], BOTTOM_LEFT)?;
        triangle(a, l, &mut c, [0.0, 1.0, 0.0], TOP_RIGHT)?;
        finish(a, l)
    });
    let mut cpu = machine("trace3d", true, exe);
    let path = PathBuf::from("target/test_win32_opengl/trace3d/t.trace");
    rust_dos::win32::trace3d::start(&mut cpu, &path).unwrap();
    run_until_done(&mut cpu, data);
    rust_dos::win32::trace3d::flush(&mut cpu);
    let (events, counts) = super::win32_d3d_tests::read_trace(&path);
    let event = |start: &str| super::win32_d3d_tests::event(&events, &counts, start);
    let choose = event("GDI32.ChoosePixelFormat pfd[").0;
    // PFD_DRAW_TO_WINDOW, PFD_SUPPORT_OPENGL and PFD_DOUBLEBUFFER.
    let start = "GDI32.ChoosePixelFormat pfd[flags=0x25 ";
    assert!(choose.starts_with(start), "{choose}");
    assert!(choose.ends_with("] = 2"), "{choose}");
    let calls = [
        ("OPENGL32.glGetString(VENDOR) = \"3Dfx Interactive Inc.\"", 1),
        ("OPENGL32.glEnable(DEPTH_TEST)", 1),
        ("OPENGL32.glDepthFunc(LESS)", 1),
        ("OPENGL32.glBegin(TRIANGLES)", 2),
        ("OPENGL32.glVertex2f", 6),
        ("OPENGL32.glEnd", 2),
    ];
    for (call, n) in calls {
        assert_eq!(event(call), (call, n));
    }
}
