//! The built-in Windows' Glide (GLIDE2X.DLL): Win32 programs that open
//! the 3dfx card, draw shaded, textured, depth-tested, blended and fogged
//! triangles, swap and read the pixels back, on a Voodoo Graphics, a
//! Voodoo2 and a Voodoo3; one without a 3dfx card; and one that ends
//! without closing Glide. The programs are PE files made with
//! rust-dos-pe's builder, their code assembled with iced, as in
//! win32_ddraw_tests.rs.

use iced_x86::code_asm::*;
use rust_dos::cpu::{Cpu, HostConsole};
use rust_dos::exec::{NoHook, StopReason, run_batch};
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
    fn at(&self, offset: u32) -> u64 {
        (self.data + offset) as u64
    }
}

/// A program importing GLIDE2X.DLL's `GLIDE` and KERNEL32.DLL's
/// ExitProcess, with `data` and the code `f` makes, and where its data
/// is.
fn program(
    data: &[u8],
    f: impl FnOnce(&mut CodeAssembler, &Layout) -> Result<(), IcedError>,
) -> (Vec<u8>, u32) {
    let mut b = Builder::exe(BASE);
    let glide: Vec<Imp> = GLIDE.iter().map(|n| Imp::Name(n)).collect();
    let kernel = [Imp::Name("ExitProcess")];
    let iat = b
        .imports(&[("GLIDE2X.DLL", &glide), ("KERNEL32.DLL", &kernel)])
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

/// A machine with the built-in Windows on, the 3dfx card `board` and
/// `program` loaded from the shell as T.EXE.
fn machine(test: &str, board: Option<Board>, program: Vec<u8>) -> Cpu {
    let base = PathBuf::from("target/test_win32_glide").join(test);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join("T.EXE"), program).unwrap();
    let mut cpu = Cpu::new(base);
    cpu.bus.configure_voodoo(board);
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    cpu
}

fn printed(cpu: &Cpu) -> String {
    let console = cpu.host_console.as_ref().unwrap();
    let mut text = console.stdout.clone();
    text.extend(&console.stderr);
    String::from_utf8_lossy(&text).into_owned()
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

/// Run the program until it says it is done (DONE in its data).
fn run_until_done(cpu: &mut Cpu, data: u32) {
    for _ in 0..600 {
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

/// Run the program to its end.
fn run_to_exit(cpu: &mut Cpu) {
    for _ in 0..600 {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        if run_batch(cpu, &mut NoHook, false) == StopReason::ShellReloaded {
            return;
        }
    }
    panic!("the program didn't end: {:?}", printed(cpu));
}

// The functions the programs import, and their places.
const GLIDE: &[&str] = &[
    "_grGlideInit@0",
    "_grSstQueryHardware@4",
    "_grSstSelect@4",
    "_grSstWinOpen@28",
    "_grBufferClear@12",
    "_guColorCombineFunction@4",
    "_grDrawTriangle@12",
    "_grDepthBufferMode@4",
    "_grDepthBufferFunction@4",
    "_grDepthMask@4",
    "_grAlphaBlendFunction@16",
    "_guAlphaSource@4",
    "_grTexMinAddress@4",
    "_grTexDownloadMipMap@16",
    "_grTexSource@16",
    "_guTexCombineFunction@8",
    "_grFogColorValue@4",
    "_grFogMode@4",
    "_grFogTable@4",
    "_grLfbReadRegion@28",
    "_grBufferSwap@4",
    "_grBufferNumPending@0",
    "_grLfbLock@24",
    "_grLfbUnlock@8",
    "_grGlideShutdown@0",
    "_grSstScreenWidth@0",
    "_grTexFilterMode@12",
    "_grDitherMode@4",
];

fn import(name: &str) -> usize {
    GLIDE.iter().position(|n| *n == name).unwrap()
}

/// The functions' values the programs use.
const GR_RESOLUTION_640X480: u32 = 7;
const GR_REFRESH_60HZ: u32 = 0;
const GR_COLORFORMAT_ARGB: u32 = 0;
const GR_ORIGIN_UPPER_LEFT: u32 = 0;
const GR_COLORCOMBINE_ITRGB: u32 = 2;
const GR_COLORCOMBINE_DECAL_TEXTURE: u32 = 4;
const GR_ALPHASOURCE_ITERATED_ALPHA: u32 = 1;
const GR_TEXTURECOMBINE_DECAL: u32 = 1;
const GR_DEPTHBUFFER_DISABLE: u32 = 0;
const GR_DITHER_DISABLE: u32 = 0;
const GR_DEPTHBUFFER_ZBUFFER: u32 = 1;
const GR_CMP_LESS: u32 = 1;
const GR_BLEND_ZERO: u32 = 0;
const GR_BLEND_SRC_ALPHA: u32 = 1;
const GR_BLEND_ONE: u32 = 4;
const GR_BLEND_ONE_MINUS_SRC_ALPHA: u32 = 5;
const GR_FOG_DISABLE: u32 = 0;
const GR_FOG_WITH_TABLE: u32 = 2;
const GR_BUFFER_FRONTBUFFER: u32 = 0;
const GR_BUFFER_BACKBUFFER: u32 = 1;
const GR_LFB_READ_ONLY: u32 = 0;
const GR_LFBWRITEMODE_ANY: u32 = 0xFF;
const GR_MIPMAPLEVELMASK_BOTH: u32 = 3;

// The data's layout.

const RESULTS: u32 = 0x40;
const DONE: u32 = 0x140;
const HWCONFIG: u32 = 0x200;
const LFB_INFO: u32 = 0x300;
const TEX_INFO: u32 = 0x320;
const FOG_TABLE: u32 = 0x340;
/// The pixels read back, 16 bits each.
const PIXELS: u32 = 0x400;
/// A dword read through the locked frame buffer.
const LOCK_READ: u32 = 0x480;
/// An 8x8 texture of 5-6-5 texels: blue on the left, red on the right.
const TEXELS: u32 = 0x500;
const VERTICES: u32 = 0x600;
const DATA_SIZE: usize = 0x1000;

fn put(data: &mut [u8], offset: u32, bytes: &[u8]) {
    data[offset as usize..offset as usize + bytes.len()].copy_from_slice(bytes);
}

fn put32(data: &mut [u8], offset: u32, value: u32) {
    put(data, offset, &value.to_le_bytes());
}

/// A GrVertex: x, y, z, r, g, b, ooz, a, oow, then texture unit 0's
/// sow, tow and oow and unit 1's.
#[derive(Clone, Copy, Default)]
struct V {
    x: f32,
    y: f32,
    rgb: [f32; 3],
    a: f32,
    ooz: f32,
    st: [f32; 2],
}

const VERTEX: u32 = 60;

fn put_vertex(data: &mut [u8], index: u32, v: V) {
    let floats = [
        v.x, v.y, 0.0, v.rgb[0], v.rgb[1], v.rgb[2], v.ooz, v.a, 1.0, v.st[0], v.st[1], 1.0,
        v.st[0], v.st[1], 1.0,
    ];
    for (i, f) in floats.iter().enumerate() {
        put32(data, VERTICES + VERTEX * index + 4 * i as u32, f.to_bits());
    }
}

/// An argument a program passes.
#[derive(Clone, Copy)]
enum Arg {
    I(u32),
    At(u32),
    /// Vertex `n`'s address.
    Vtx(u32),
}

struct Calls {
    next: u32,
}

impl Calls {
    /// Call `name` with `args`, its result kept; the result's place.
    fn call(
        &mut self,
        a: &mut CodeAssembler,
        l: &Layout,
        name: &str,
        args: &[Arg],
    ) -> Result<u32, IcedError> {
        for arg in args.iter().rev() {
            match *arg {
                Arg::I(v) => a.push(v as i32)?,
                Arg::At(o) => a.push(l.at(o) as i32)?,
                Arg::Vtx(n) => a.push(l.at(VERTICES + VERTEX * n) as i32)?,
            }
        }
        a.call(dword_ptr(l.iat[import(name)] as u64))?;
        a.mov(dword_ptr(l.at(RESULTS + 4 * self.next)), eax)?;
        self.next += 1;
        Ok(self.next - 1)
    }

    fn triangle(
        &mut self,
        a: &mut CodeAssembler,
        l: &Layout,
        first: u32,
    ) -> Result<u32, IcedError> {
        self.call(
            a,
            l,
            "_grDrawTriangle@12",
            &[Arg::Vtx(first), Arg::Vtx(first + 1), Arg::Vtx(first + 2)],
        )
    }

    /// Read pixel (x, y) of `buffer` into PIXELS slot `slot`.
    fn pixel(
        &mut self,
        a: &mut CodeAssembler,
        l: &Layout,
        buffer: u32,
        (x, y): (u32, u32),
        slot: u32,
    ) -> Result<u32, IcedError> {
        self.call(
            a,
            l,
            "_grLfbReadRegion@28",
            &[
                Arg::I(buffer),
                Arg::I(x),
                Arg::I(y),
                Arg::I(1),
                Arg::I(1),
                Arg::I(2),
                Arg::At(PIXELS + 2 * slot),
            ],
        )
    }
}

/// The end of a program: DONE set, and it spins.
fn finish(a: &mut CodeAssembler, l: &Layout) -> Result<(), IcedError> {
    a.mov(dword_ptr(l.at(DONE)), 1)?;
    let mut again = a.create_label();
    a.set_label(&mut again)?;
    a.jmp(again)
}

fn result(cpu: &Cpu, data: u32, index: u32) -> u32 {
    peek(cpu, data + RESULTS + 4 * index)
}

fn pixel(cpu: &Cpu, data: u32, slot: u32) -> u16 {
    let dword = peek(cpu, data + PIXELS + 2 * (slot & !1));
    (dword >> (16 * (slot & 1))) as u16
}

/// A triangle `size` wide and high, its right angle at (x0, y0), the top
/// left.
fn tri(data: &mut [u8], first: u32, (x0, y0): (f32, f32), size: f32, v: V) {
    put_vertex(data, first, V { x: x0, y: y0, ..v });
    put_vertex(
        data,
        first + 1,
        V {
            x: x0 + size,
            y: y0,
            ..v
        },
    );
    put_vertex(
        data,
        first + 2,
        V {
            x: x0,
            y: y0 + size,
            ..v
        },
    );
}

const RED: [f32; 3] = [255.0, 0.0, 0.0];
const GREEN: [f32; 3] = [0.0, 255.0, 0.0];
const BLUE: [f32; 3] = [0.0, 0.0, 255.0];

// Where each test draws, and the pixel it reads there.
const SHADED: (u32, u32) = (20, 20);
const GOURAUD: (u32, u32) = (120, 30);
const DEPTH: (u32, u32) = (220, 20);
const BLENDED: (u32, u32) = (320, 20);
const TEXTURED_LEFT: (u32, u32) = (410, 30);
const TEXTURED_RIGHT: (u32, u32) = (450, 30);
const FOGGED: (u32, u32) = (520, 20);
const CLEARED: (u32, u32) = (600, 400);

/// The program the drawing tests run: it opens a 640x480 display and
/// draws a flat triangle, a Gouraud-shaded one, three overlapping ones
/// with the depth test, a blended one, a textured square and a fogged
/// triangle, reads pixels of the back buffer, swaps, reads one of the
/// front buffer and one through the locked frame buffer.
fn drawing_program() -> (Vec<u8>, u32) {
    let mut d = vec![0u8; DATA_SIZE];
    // Vertices 0-2: red.
    let flat = V {
        rgb: RED,
        a: 255.0,
        ..V::default()
    };
    tri(&mut d, 0, (10.0, 10.0), 60.0, flat);
    // 3-5: red, green and blue corners.
    put_vertex(
        &mut d,
        3,
        V {
            x: 100.0,
            y: 10.0,
            rgb: RED,
            ..V::default()
        },
    );
    put_vertex(
        &mut d,
        4,
        V {
            x: 180.0,
            y: 10.0,
            rgb: GREEN,
            ..V::default()
        },
    );
    put_vertex(
        &mut d,
        5,
        V {
            x: 100.0,
            y: 90.0,
            rgb: BLUE,
            ..V::default()
        },
    );
    // 6-14: blue far, green near, red between.
    tri(
        &mut d,
        6,
        (210.0, 10.0),
        60.0,
        V {
            rgb: BLUE,
            ooz: 40000.0,
            ..V::default()
        },
    );
    tri(
        &mut d,
        9,
        (210.0, 10.0),
        60.0,
        V {
            rgb: GREEN,
            ooz: 20000.0,
            ..V::default()
        },
    );
    tri(
        &mut d,
        12,
        (210.0, 10.0),
        60.0,
        V {
            rgb: RED,
            ooz: 30000.0,
            ..V::default()
        },
    );
    // 15-20: red, then white at half alpha over it.
    tri(&mut d, 15, (310.0, 10.0), 60.0, flat);
    tri(
        &mut d,
        18,
        (310.0, 10.0),
        60.0,
        V {
            rgb: [255.0; 3],
            a: 128.0,
            ..V::default()
        },
    );
    // 21-26: a 64-pixel square, the texture's 256 by 256 over it.
    let corner = |x: f32, y: f32| V {
        x: 400.0 + x * 64.0,
        y: 10.0 + y * 64.0,
        st: [x * 256.0, y * 256.0],
        ..V::default()
    };
    for (i, (x, y)) in [(0., 0.), (1., 0.), (1., 1.), (0., 0.), (1., 1.), (0., 1.)]
        .into_iter()
        .enumerate()
    {
        put_vertex(&mut d, 21 + i as u32, corner(x, y));
    }
    // 27-29: red under blue fog.
    tri(&mut d, 27, (510.0, 10.0), 60.0, flat);
    for i in 0..64 {
        d[(FOG_TABLE + i) as usize] = 255;
    }
    // The texture: an 8x8 level (GR_LOD_8), square, 5-6-5.
    for t in 0..8u32 {
        for s in 0..8u32 {
            let texel: u16 = if s < 4 { 0x001F } else { 0xF800 };
            put(&mut d, TEXELS + 2 * (t * 8 + s), &texel.to_le_bytes());
        }
    }
    for (i, v) in [5u32, 5, 3, 0xA].iter().enumerate() {
        put32(&mut d, TEX_INFO + 4 * i as u32, *v);
    }
    put32(&mut d, LFB_INFO, 20);
    program(&d, |a, l| {
        let mut c = Calls { next: 0 };
        c.call(a, l, "_grGlideInit@0", &[])?;
        c.call(a, l, "_grSstQueryHardware@4", &[Arg::At(HWCONFIG)])?;
        c.call(a, l, "_grSstSelect@4", &[Arg::I(0)])?;
        c.call(
            a,
            l,
            "_grSstWinOpen@28",
            &[
                Arg::I(0),
                Arg::I(GR_RESOLUTION_640X480),
                Arg::I(GR_REFRESH_60HZ),
                Arg::I(GR_COLORFORMAT_ARGB),
                Arg::I(GR_ORIGIN_UPPER_LEFT),
                Arg::I(2),
                Arg::I(1),
            ],
        )?;
        c.call(a, l, "_grSstScreenWidth@0", &[])?;
        // Exact colours, and depth buffering on until the depth test is
        // done, so the clear clears the depth buffer.
        c.call(a, l, "_grDitherMode@4", &[Arg::I(GR_DITHER_DISABLE)])?;
        c.call(
            a,
            l,
            "_grDepthBufferMode@4",
            &[Arg::I(GR_DEPTHBUFFER_ZBUFFER)],
        )?;
        c.call(a, l, "_grDepthBufferFunction@4", &[Arg::I(GR_CMP_LESS)])?;
        c.call(a, l, "_grDepthMask@4", &[Arg::I(1)])?;
        // Grey everywhere.
        c.call(
            a,
            l,
            "_grBufferClear@12",
            &[Arg::I(0x0080_8080), Arg::I(0), Arg::I(0xFFFF)],
        )?;
        c.call(
            a,
            l,
            "_guColorCombineFunction@4",
            &[Arg::I(GR_COLORCOMBINE_ITRGB)],
        )?;
        c.triangle(a, l, 0)?;
        c.triangle(a, l, 3)?;
        // The depth test.
        c.triangle(a, l, 6)?;
        c.triangle(a, l, 9)?;
        c.triangle(a, l, 12)?;
        c.call(a, l, "_grDepthMask@4", &[Arg::I(0)])?;
        c.call(
            a,
            l,
            "_grDepthBufferMode@4",
            &[Arg::I(GR_DEPTHBUFFER_DISABLE)],
        )?;
        // Blending.
        c.triangle(a, l, 15)?;
        c.call(
            a,
            l,
            "_guAlphaSource@4",
            &[Arg::I(GR_ALPHASOURCE_ITERATED_ALPHA)],
        )?;
        c.call(
            a,
            l,
            "_grAlphaBlendFunction@16",
            &[
                Arg::I(GR_BLEND_SRC_ALPHA),
                Arg::I(GR_BLEND_ONE_MINUS_SRC_ALPHA),
                Arg::I(GR_BLEND_ONE),
                Arg::I(GR_BLEND_ZERO),
            ],
        )?;
        c.triangle(a, l, 18)?;
        c.call(
            a,
            l,
            "_grAlphaBlendFunction@16",
            &[
                Arg::I(GR_BLEND_ONE),
                Arg::I(GR_BLEND_ZERO),
                Arg::I(GR_BLEND_ONE),
                Arg::I(GR_BLEND_ZERO),
            ],
        )?;
        // The texture, at the start of texture unit 0's memory.
        let start = c.call(a, l, "_grTexMinAddress@4", &[Arg::I(0)])?;
        a.mov(dword_ptr(l.at(TEX_INFO + 16)), l.at(TEXELS) as i32)?;
        a.mov(eax, dword_ptr(l.at(RESULTS + 4 * start)))?;
        a.push(l.at(TEX_INFO) as i32)?;
        a.push(GR_MIPMAPLEVELMASK_BOTH as i32)?;
        a.push(eax)?;
        a.push(0)?;
        a.call(dword_ptr(l.iat[import("_grTexDownloadMipMap@16")] as u64))?;
        a.mov(eax, dword_ptr(l.at(RESULTS + 4 * start)))?;
        a.push(l.at(TEX_INFO) as i32)?;
        a.push(GR_MIPMAPLEVELMASK_BOTH as i32)?;
        a.push(eax)?;
        a.push(0)?;
        a.call(dword_ptr(l.iat[import("_grTexSource@16")] as u64))?;
        c.call(
            a,
            l,
            "_grTexFilterMode@12",
            &[Arg::I(0), Arg::I(0), Arg::I(0)],
        )?;
        c.call(
            a,
            l,
            "_guTexCombineFunction@8",
            &[Arg::I(0), Arg::I(GR_TEXTURECOMBINE_DECAL)],
        )?;
        c.call(
            a,
            l,
            "_guColorCombineFunction@4",
            &[Arg::I(GR_COLORCOMBINE_DECAL_TEXTURE)],
        )?;
        c.triangle(a, l, 21)?;
        c.triangle(a, l, 24)?;
        c.call(
            a,
            l,
            "_guColorCombineFunction@4",
            &[Arg::I(GR_COLORCOMBINE_ITRGB)],
        )?;
        // Fog.
        c.call(a, l, "_grFogColorValue@4", &[Arg::I(0x0000_00FF)])?;
        c.call(a, l, "_grFogTable@4", &[Arg::At(FOG_TABLE)])?;
        c.call(a, l, "_grFogMode@4", &[Arg::I(GR_FOG_WITH_TABLE)])?;
        c.triangle(a, l, 27)?;
        c.call(a, l, "_grFogMode@4", &[Arg::I(GR_FOG_DISABLE)])?;
        // The pixels, from the back buffer.
        let points = [
            SHADED,
            GOURAUD,
            DEPTH,
            BLENDED,
            TEXTURED_LEFT,
            TEXTURED_RIGHT,
            FOGGED,
            CLEARED,
        ];
        for (slot, p) in points.into_iter().enumerate() {
            c.pixel(a, l, GR_BUFFER_BACKBUFFER, p, slot as u32)?;
        }
        // Swap, wait for it, and read the front buffer.
        c.call(a, l, "_grBufferSwap@4", &[Arg::I(1)])?;
        let mut wait = a.create_label();
        a.set_label(&mut wait)?;
        a.call(dword_ptr(l.iat[import("_grBufferNumPending@0")] as u64))?;
        a.test(eax, eax)?;
        a.jnz(wait)?;
        c.pixel(a, l, GR_BUFFER_FRONTBUFFER, SHADED, 8)?;
        // The front buffer through a lock: the dword at the shaded pixel.
        c.call(
            a,
            l,
            "_grLfbLock@24",
            &[
                Arg::I(GR_LFB_READ_ONLY),
                Arg::I(GR_BUFFER_FRONTBUFFER),
                Arg::I(GR_LFBWRITEMODE_ANY),
                Arg::I(GR_ORIGIN_UPPER_LEFT),
                Arg::I(0),
                Arg::At(LFB_INFO),
            ],
        )?;
        a.mov(eax, dword_ptr(l.at(LFB_INFO + 8)))?;
        a.imul_3(eax, eax, SHADED.1 as i32)?;
        a.add(eax, dword_ptr(l.at(LFB_INFO + 4)))?;
        a.mov(eax, dword_ptr(eax + 2 * SHADED.0))?;
        a.mov(dword_ptr(l.at(LOCK_READ)), eax)?;
        c.call(
            a,
            l,
            "_grLfbUnlock@8",
            &[Arg::I(GR_LFB_READ_ONLY), Arg::I(GR_BUFFER_FRONTBUFFER)],
        )?;
        finish(a, l)
    })
}

// The places of the results the tests look at.
const R_QUERY: u32 = 1;
const R_OPEN: u32 = 3;
const R_WIDTH: u32 = 4;

/// A 5-6-5 pixel's red, green and blue.
fn rgb(p: u16) -> (u16, u16, u16) {
    (p >> 11, (p >> 5) & 0x3F, p & 0x1F)
}

/// `memory`: the frame buffer and texture unit megabytes the board
/// reports.
fn draws(test: &str, board: Board, memory: (u32, u32)) {
    let (program, data) = drawing_program();
    let mut cpu = machine(test, Some(board), program);
    run_until_done(&mut cpu, data);
    assert_eq!(result(&cpu, data, R_QUERY), 1, "grSstQueryHardware");
    assert_eq!(peek(&cpu, data + HWCONFIG), 1, "one board");
    let config = |i: u32| peek(&cpu, data + HWCONFIG + 4 * i);
    assert_eq!(config(1), 0, "shows as a Voodoo Graphics, as in Glide 2");
    assert_eq!((config(2), config(7)), memory, "fbRam and tmuRam");
    assert_eq!(result(&cpu, data, R_OPEN), 1, "grSstWinOpen");
    assert_eq!(result(&cpu, data, R_WIDTH), 640);
    let p = |slot| pixel(&cpu, data, slot);
    assert_eq!(p(0), 0xF800, "the flat triangle");
    let (r, g, b) = rgb(p(1));
    assert!(r > 0 && g > 0 && b > 0, "Gouraud shading: {:04X}", p(1));
    assert_eq!(p(2), 0x07E0, "the depth test keeps the nearest");
    let (r, g, b) = rgb(p(3));
    assert!(
        r == 31 && (28..=36).contains(&g) && (14..=18).contains(&b),
        "blending: {:04X}",
        p(3)
    );
    assert_eq!(p(4), 0x001F, "the texture's left half");
    assert_eq!(p(5), 0xF800, "the texture's right half");
    assert_eq!(p(6), 0x001F, "fog");
    assert_eq!(p(7), 0x8410, "the clear");
    assert_eq!(p(8), 0xF800, "after the swap, in the front buffer");
    let locked = peek(&cpu, data + LOCK_READ) as u16;
    assert_eq!(locked, 0xF800, "through the locked frame buffer");
}

#[test]
fn glide_draws_on_a_voodoo_graphics() {
    draws("voodoo1", Board::Standard, (2, 2));
}

#[test]
fn glide_draws_on_a_voodoo2() {
    draws("voodoo2", Board::Voodoo2, (4, 2));
}

#[test]
fn glide_draws_on_a_voodoo3() {
    draws("voodoo3", Board::Voodoo3, (12, 2));
}

/// A program that asks for the card and tries to open it.
fn query_program() -> (Vec<u8>, u32) {
    let mut d = vec![0u8; DATA_SIZE];
    tri(&mut d, 0, (10.0, 10.0), 60.0, V::default());
    program(&d, |a, l| {
        let mut c = Calls { next: 0 };
        c.call(a, l, "_grGlideInit@0", &[])?;
        c.call(a, l, "_grSstQueryHardware@4", &[Arg::At(HWCONFIG)])?;
        c.call(a, l, "_grSstSelect@4", &[Arg::I(0)])?;
        c.call(
            a,
            l,
            "_grSstWinOpen@28",
            &[
                Arg::I(0),
                Arg::I(GR_RESOLUTION_640X480),
                Arg::I(GR_REFRESH_60HZ),
                Arg::I(GR_COLORFORMAT_ARGB),
                Arg::I(GR_ORIGIN_UPPER_LEFT),
                Arg::I(2),
                Arg::I(1),
            ],
        )?;
        c.triangle(a, l, 0)?;
        c.call(a, l, "_grGlideShutdown@0", &[])?;
        finish(a, l)
    })
}

#[test]
fn without_a_3dfx_card_glide_finds_none() {
    let (program, data) = query_program();
    let mut cpu = machine("none", None, program);
    run_until_done(&mut cpu, data);
    assert_eq!(result(&cpu, data, R_QUERY), 0, "grSstQueryHardware");
    assert_eq!(result(&cpu, data, R_OPEN), 0, "grSstWinOpen");
}

/// A program that opens the card, clears and swaps, and ends without
/// grSstWinClose or grGlideShutdown.
fn leaving_program() -> (Vec<u8>, u32) {
    let d = vec![0u8; DATA_SIZE];
    program(&d, |a, l| {
        let mut c = Calls { next: 0 };
        c.call(a, l, "_grGlideInit@0", &[])?;
        c.call(a, l, "_grSstQueryHardware@4", &[Arg::At(HWCONFIG)])?;
        c.call(
            a,
            l,
            "_grSstWinOpen@28",
            &[
                Arg::I(0),
                Arg::I(GR_RESOLUTION_640X480),
                Arg::I(GR_REFRESH_60HZ),
                Arg::I(GR_COLORFORMAT_ARGB),
                Arg::I(GR_ORIGIN_UPPER_LEFT),
                Arg::I(2),
                Arg::I(1),
            ],
        )?;
        c.call(
            a,
            l,
            "_grBufferClear@12",
            &[Arg::I(0x00FF_0000), Arg::I(0), Arg::I(0)],
        )?;
        c.call(a, l, "_grBufferSwap@4", &[Arg::I(0)])?;
        a.mov(dword_ptr(l.at(DONE)), 1)?;
        // A while for the test to see the card open.
        a.mov(ecx, 2_000_000)?;
        let mut spin = a.create_label();
        a.set_label(&mut spin)?;
        a.dec(ecx)?;
        a.jnz(spin)?;
        a.push(0)?;
        a.call(dword_ptr(l.iat[GLIDE.len()] as u64))?;
        a.int3()
    })
}

/// The card's fbiInit0 (Voodoo Graphics and Voodoo2): bit 0 takes the
/// monitor from the VGA.
fn fbi_init0(cpu: &Cpu) -> u32 {
    cpu.bus.voodoo.as_ref().unwrap().reg[0x210 / 4]
}

#[test]
fn a_program_ending_without_closing_glide_gives_the_screen_back() {
    let (program, data) = leaving_program();
    let mut cpu = machine("leaving", Some(Board::Voodoo2), program);
    run_until_done(&mut cpu, data);
    assert_eq!(fbi_init0(&cpu) & 1, 1, "the card has the monitor");
    run_to_exit(&mut cpu);
    assert_eq!(fbi_init0(&cpu) & 1, 0, "the VGA has the monitor back");
}

#[test]
fn a_program_ending_without_closing_glide_gives_a_voodoo3_its_mode_back() {
    let (program, data) = leaving_program();
    let mut cpu = machine("leaving_v3", Some(Board::Voodoo3), program);
    let before = cpu.bus.vga.crtc_regs;
    run_until_done(&mut cpu, data);
    assert_ne!(cpu.bus.vga.crtc_regs, before, "Glide's mode");
    run_to_exit(&mut cpu);
    assert_eq!(cpu.bus.vga.crtc_regs, before, "the mode the screen was in");
}
