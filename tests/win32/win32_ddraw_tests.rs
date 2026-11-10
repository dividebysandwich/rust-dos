//! The built-in Windows' DirectDraw (DDRAW.DLL): Win32 programs that set
//! a display mode and draw on surfaces, in full screen and in a window on
//! the desktop. The programs are PE files made with rust-dos-pe's
//! builder, their code assembled with iced, as in win32_tests.rs. Each
//! leaves its results in its data and spins, and the test looks at them
//! and at the display while the program is still there.

use iced_x86::code_asm::*;
use rust_dos::cpu::{Cpu, HostConsole};
use rust_dos::exec::{NoHook, run_batch};
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

/// A machine with the built-in Windows on and `program` loaded from the
/// shell as T.EXE.
fn machine(test: &str, program: Vec<u8>) -> Cpu {
    let base = PathBuf::from("target/test_win32_ddraw").join(test);
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

/// Run the program until it says it is done (DONE in its data), with
/// its results there.
fn run_until_done(cpu: &mut Cpu, layout_data: u32) {
    for _ in 0..400 {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        run_batch(cpu, &mut NoHook, false);
        // The program's memory is there while its code runs, with paging.
        let paged = cpu.cr0 & 0x8000_0001 == 0x8000_0001;
        if paged && peek(cpu, layout_data + DONE) == 1 {
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
const FRONT: u32 = 0x08;
const BACK: u32 = 0x0C;
const PALETTE: u32 = 0x10;
const CLIPPER: u32 = 0x14;
const OFFSCREEN: u32 = 0x18;
const SOURCE: u32 = 0x1C;
/// FRONT's other interface, from QueryInterface.
const QUERIED: u32 = 0x20;
/// The HRESULTs of the calls, in order.
const RESULTS: u32 = 0x40;
const DONE: u32 = 0xC0;
const DESC: u32 = 0x100;
const CAPS: u32 = 0x180;
const LOCKED: u32 = 0x200;
const FX: u32 = 0x280;
const RECT: u32 = 0x300;
const KEY: u32 = 0x310;
const CLASS: u32 = 0x320;
const ENTRIES: u32 = 0x400;
const PIXELS: u32 = 0x800;
/// Where the polling program counts its calls of Flip (or of
/// GetFlipStatus), keeps the first one's result and the last one's, and
/// has GetScanLine put the line.
const POLLS: u32 = 0x880;
const LAST: u32 = 0x884;
const LINE: u32 = 0x888;
const FIRST: u32 = 0x88C;
const DATA_SIZE: usize = 0x900;

/// The data the programs start with: the window class's name.
fn data() -> Vec<u8> {
    let mut data = vec![0u8; DATA_SIZE];
    put(&mut data, CLASS, b"Static\0");
    data
}

fn put(data: &mut [u8], offset: u32, bytes: &[u8]) {
    data[offset as usize..offset as usize + bytes.len()].copy_from_slice(bytes);
}

fn put32(data: &mut [u8], offset: u32, value: u32) {
    put(data, offset, &value.to_le_bytes());
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

/// The end of a program: DONE set, and it spins, its memory there to
/// look at.
fn finish(a: &mut CodeAssembler, l: &Layout) -> Result<(), IcedError> {
    a.mov(dword_ptr(l.at(DONE)), 1)?;
    let mut again = a.create_label();
    a.set_label(&mut again)?;
    a.jmp(again)
}

/// The results of the first `n` calls, read from the program's data.
fn results(cpu: &mut Cpu, data: u32, n: u32) -> Vec<u32> {
    (0..n).map(|i| peek(cpu, data + RESULTS + 4 * i)).collect()
}

// Methods' places in their vtables.
const DD_CREATE_PALETTE: u32 = 5;
const DD_CREATE_SURFACE: u32 = 6;
const DD_GET_SCAN_LINE: u32 = 16;
const DD_SET_COOPERATIVE_LEVEL: u32 = 20;
const DD_SET_DISPLAY_MODE: u32 = 21;
const DD_WAIT_FOR_VERTICAL_BLANK: u32 = 22;
const S_QUERY_INTERFACE: u32 = 0;
const S_ADD_ATTACHED_SURFACE: u32 = 3;
const S_BLT: u32 = 5;
const S_FLIP: u32 = 11;
const S_GET_ATTACHED_SURFACE: u32 = 12;
const S_GET_CAPS: u32 = 14;
const S_GET_FLIP_STATUS: u32 = 18;
const S_LOCK: u32 = 25;
const S_SET_CLIPPER: u32 = 28;
const S_SET_COLOR_KEY: u32 = 29;
const S_SET_PALETTE: u32 = 31;
const S_UNLOCK: u32 = 32;
const PAL_SET_ENTRIES: u32 = 6;
const CLIP_SET_HWND: u32 = 8;

const DDSD_CAPS: u32 = 0x1;
const DDSD_HEIGHT: u32 = 0x2;
const DDSD_WIDTH: u32 = 0x4;
const DDSD_BACKBUFFERCOUNT: u32 = 0x20;
const DDSD_PIXELFORMAT: u32 = 0x1000;
const DDSCAPS_BACKBUFFER: u32 = 0x4;
const DDSCAPS_COMPLEX: u32 = 0x8;
const DDSCAPS_FLIP: u32 = 0x10;
const DDSCAPS_FRONTBUFFER: u32 = 0x20;
const DDSCAPS_OFFSCREENPLAIN: u32 = 0x40;
const DDSCAPS_PRIMARYSURFACE: u32 = 0x200;
const DDSCAPS_SYSTEMMEMORY: u32 = 0x800;
const DDSCL_FULLSCREEN: u32 = 0x1;
const DDSCL_NORMAL: u32 = 0x8;
const DDSCL_EXCLUSIVE: u32 = 0x10;
const DDBLT_COLORFILL: u32 = 0x400;
const DDBLT_KEYSRC: u32 = 0x8000;
const DDBLT_WAIT: u32 = 0x100_0000;
const DDWAITVB_BLOCKEND: u32 = 0x4;
const DDCKEY_SRCBLT: u32 = 0x8;
const DDPCAPS_8BIT: u32 = 0x4;
const DDPCAPS_ALLOW256: u32 = 0x40;
const WS_POPUP: u32 = 0x8000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_EX_TOPMOST: u32 = 0x8;

/// A DDSURFACEDESC at `offset` with `flags`, `caps` and the rest given.
fn desc(data: &mut [u8], offset: u32, flags: u32, caps: u32, w: u32, h: u32, backbuffers: u32) {
    put32(data, offset, 108);
    put32(data, offset + 0x04, flags);
    put32(data, offset + 0x08, h);
    put32(data, offset + 0x0C, w);
    put32(data, offset + 0x14, backbuffers);
    put32(data, offset + 0x68, caps);
}

/// A 32-bit RGB pixel format in the DDSURFACEDESC at `offset`.
fn rgb32(data: &mut [u8], offset: u32) {
    let pf = offset + 0x48;
    put32(data, pf, 32);
    put32(data, pf + 4, 0x40);
    put32(data, pf + 12, 32);
    put32(data, pf + 16, 0xFF_0000);
    put32(data, pf + 20, 0xFF00);
    put32(data, pf + 24, 0xFF);
}

const USER32: (&str, &[&str]) = ("USER32.DLL", &["CreateWindowExA"]);
const DDRAW: (&str, &[&str]) = (
    "DDRAW.DLL",
    &["DirectDrawCreate", "DirectDrawCreateClipper"],
);
const CREATE_WINDOW: usize = 0;
const DIRECT_DRAW_CREATE: usize = 1;
const DIRECT_DRAW_CREATE_CLIPPER: usize = 2;

/// Make a window, as CreateWindowExA's arguments say, and a DirectDraw
/// object.
fn start(
    a: &mut CodeAssembler,
    l: &Layout,
    c: &mut Calls,
    rect: (u32, u32, u32, u32),
) -> Result<(), IcedError> {
    let (x, y, w, h) = rect;
    c.api(
        a,
        l,
        CREATE_WINDOW,
        &[
            Arg::I(0),
            Arg::At(CLASS),
            Arg::At(CLASS),
            Arg::I(WS_POPUP | WS_VISIBLE),
            Arg::I(x),
            Arg::I(y),
            Arg::I(w),
            Arg::I(h),
            Arg::I(0),
            Arg::I(0),
            Arg::I(BASE),
            Arg::I(0),
        ],
    )?;
    a.mov(dword_ptr(l.at(HWND)), eax)?;
    c.api(
        a,
        l,
        DIRECT_DRAW_CREATE,
        &[Arg::I(0), Arg::At(DD), Arg::I(0)],
    )
}

/// The data of a program that makes a flip chain: its description at
/// DESC.
fn flip_chain_data() -> Vec<u8> {
    let mut d = data();
    desc(
        &mut d,
        DESC,
        DDSD_CAPS | DDSD_BACKBUFFERCOUNT,
        DDSCAPS_PRIMARYSURFACE | DDSCAPS_FLIP | DDSCAPS_COMPLEX,
        0,
        0,
        1,
    );
    d
}

/// A window, a DirectDraw object, a 320x200 8-bit mode and the flip
/// chain of flip_chain_data, its front buffer at FRONT.
fn start_flip_chain(a: &mut CodeAssembler, l: &Layout, c: &mut Calls) -> Result<(), IcedError> {
    start(a, l, c, (0, 0, 320, 200))?;
    c.com(
        a,
        l,
        DD,
        DD_SET_COOPERATIVE_LEVEL,
        &[Arg::Val(HWND), Arg::I(DDSCL_EXCLUSIVE | DDSCL_FULLSCREEN)],
    )?;
    c.com(
        a,
        l,
        DD,
        DD_SET_DISPLAY_MODE,
        &[Arg::I(320), Arg::I(200), Arg::I(8)],
    )?;
    c.com(
        a,
        l,
        DD,
        DD_CREATE_SURFACE,
        &[Arg::At(DESC), Arg::At(FRONT), Arg::I(0)],
    )
}

/// Full screen: a 320x200 8-bit mode, a flip chain, a pixel written to
/// the back buffer through Lock and a flip that shows it.
fn fullscreen_program() -> (Vec<u8>, u32) {
    let mut d = flip_chain_data();
    put32(&mut d, CAPS, DDSCAPS_BACKBUFFER);
    put32(&mut d, LOCKED, 108);
    program(&[USER32, DDRAW], &d, |a, l| {
        let mut c = Calls { next: 0 };
        start_flip_chain(a, l, &mut c)?;
        c.com(
            a,
            l,
            FRONT,
            S_GET_ATTACHED_SURFACE,
            &[Arg::At(CAPS), Arg::At(BACK)],
        )?;
        c.com(
            a,
            l,
            BACK,
            S_LOCK,
            &[Arg::I(0), Arg::At(LOCKED), Arg::I(1), Arg::I(0)],
        )?;
        // Four pixels at the start of the first row and of the second.
        a.mov(edi, dword_ptr(l.at(LOCKED + 0x24)))?;
        a.mov(dword_ptr(edi), 0x4433_2211)?;
        a.add(edi, dword_ptr(l.at(LOCKED + 0x10)))?;
        a.mov(dword_ptr(edi), 0x8877_6655u32 as i32)?;
        c.com(a, l, BACK, S_UNLOCK, &[Arg::I(0)])?;
        c.com(a, l, FRONT, S_FLIP, &[Arg::I(0), Arg::I(1)])?;
        finish(a, l)
    })
}

const IID_IDIRECTDRAWSURFACE4: [u8; 16] = [
    0x30, 0x86, 0x2B, 0x0B, 0x35, 0xAD, 0xD0, 0x11, 0x8E, 0xA6, 0x00, 0x60, 0x97, 0x97, 0xEA, 0x5B,
];

const IID_IDIRECTDRAWSURFACE7: [u8; 16] = [
    0x80, 0x5A, 0x67, 0x06, 0x9B, 0x3B, 0xD2, 0x11, 0xB9, 0x2F, 0x00, 0x60, 0x97, 0x97, 0xEA, 0x5B,
];

/// DDSCAPS2's last three words as a program that fills in dwCaps alone
/// leaves them: the first two are what Omikron's stack holds there.
const CAPS_GARBAGE: [u32; 3] = [0x1E0, 0x0090_E18C, 0x0012_FF80];
/// A second DDSCAPS2, after the one at CAPS.
const ZBUFFER_CAPS: u32 = CAPS + 0x10;
const DDSCAPS_ZBUFFER: u32 = 0x2_0000;
const DDERR_NOTFOUND: u32 = 0x8876_00FF;

/// A flip chain whose primary surface is asked through the interface
/// `iid` for its back buffer and then for a Z-buffer it doesn't have,
/// each with `after_caps` as the DDSCAPS2's last three words.
fn attached_caps_program(iid: &[u8; 16], after_caps: [u32; 3]) -> (Vec<u8>, u32) {
    let mut d = flip_chain_data();
    put(&mut d, IID, iid);
    for (at, caps) in [(CAPS, DDSCAPS_BACKBUFFER), (ZBUFFER_CAPS, DDSCAPS_ZBUFFER)] {
        put32(&mut d, at, caps);
        for (i, word) in after_caps.iter().enumerate() {
            put32(&mut d, at + 4 + 4 * i as u32, *word);
        }
    }
    program(&[USER32, DDRAW], &d, |a, l| {
        let mut c = Calls { next: 0 };
        start_flip_chain(a, l, &mut c)?;
        c.com(
            a,
            l,
            FRONT,
            S_QUERY_INTERFACE,
            &[Arg::At(IID), Arg::At(QUERIED)],
        )?;
        c.com(
            a,
            l,
            QUERIED,
            S_GET_ATTACHED_SURFACE,
            &[Arg::At(CAPS), Arg::At(BACK)],
        )?;
        c.com(
            a,
            l,
            QUERIED,
            S_GET_ATTACHED_SURFACE,
            &[Arg::At(ZBUFFER_CAPS), Arg::At(OFFSCREEN)],
        )?;
        finish(a, l)
    })
}

/// The results of attached_caps_program's calls after the window's, and
/// the pointer to the back buffer that it got.
fn attached_caps_results(test: &str, iid: &[u8; 16], after_caps: [u32; 3]) -> (Vec<u32>, u32) {
    let (program, data) = attached_caps_program(iid, after_caps);
    let mut cpu = machine(test, program);
    run_until_done(&mut cpu, data);
    let results = results(&mut cpu, data, 8)[1..].to_vec();
    (results, peek(&cpu, data + BACK))
}

/// IDirectDrawSurface4 compares dwCaps and doesn't look at the rest.
#[test]
fn surface4_looks_at_the_first_word_of_the_caps_for_an_attached_surface() {
    let (results, back) =
        attached_caps_results("attached_caps4", &IID_IDIRECTDRAWSURFACE4, CAPS_GARBAGE);
    assert_eq!(results, [0, 0, 0, 0, 0, 0, DDERR_NOTFOUND]);
    assert_ne!(back, 0);
}

/// IDirectDrawSurface7 compares all four words: with the last three
/// clear it finds the back buffer, and garbage in any one of them finds
/// nothing.
#[test]
fn surface7_looks_at_all_the_words_of_the_caps_for_an_attached_surface() {
    let (results, back) =
        attached_caps_results("attached_caps7_clear", &IID_IDIRECTDRAWSURFACE7, [0; 3]);
    assert_eq!(results, [0, 0, 0, 0, 0, 0, DDERR_NOTFOUND]);
    assert_ne!(back, 0);
    for (i, word) in CAPS_GARBAGE.into_iter().enumerate() {
        let mut after_caps = [0; 3];
        after_caps[i] = word;
        let test = format!("attached_caps7_word{i}");
        let (results, back) = attached_caps_results(&test, &IID_IDIRECTDRAWSURFACE7, after_caps);
        let expected = [0, 0, 0, 0, 0, DDERR_NOTFOUND, DDERR_NOTFOUND];
        assert_eq!(results, expected, "garbage in word {i} after dwCaps");
        assert_eq!(back, 0);
    }
}

#[test]
fn a_full_screen_program_draws_on_its_back_buffer_and_flips() {
    let (program, data) = fullscreen_program();
    let mut cpu = machine("fullscreen", program);
    run_until_done(&mut cpu, data);
    assert_eq!(results(&mut cpu, data, 9)[1..], [0; 8]);
    let mode = cpu.bus.vbe.mode.expect("a VBE mode");
    assert_eq!((mode.width, mode.height, mode.bpp), (320, 200, 8));
    // The back buffer was after the screen's 64000 bytes; the flip shows
    // it, and its pixels are the program's.
    let start = cpu.bus.vbe.start as usize;
    assert_eq!(start, 64000);
    let pitch = cpu.bus.vbe.pitch as usize;
    assert_eq!(pitch, 320);
    assert_eq!(cpu.bus.vbe.vram[start..start + 4], [0x11, 0x22, 0x33, 0x44]);
    let row = start + pitch;
    assert_eq!(cpu.bus.vbe.vram[row..row + 4], [0x55, 0x66, 0x77, 0x88]);
    // The pointer Lock gave is in the program's view of video memory.
    let pointer = peek(&cpu, data + LOCKED + 0x24);
    assert_eq!(pointer & 0x3F_FFFF, 64000);
}

const DDERR_WASSTILLDRAWING: u32 = 0x8876_021C;

/// What the polling program does between two calls of Flip.
enum Between {
    /// Nothing, as Enemy Infestation waits for its frames.
    Nothing,
    /// A loop of so many passes.
    Work(u32),
    /// A call of another built-in function, GetScanLine.
    Call,
}

/// What the polling program calls until it succeeds.
#[derive(Clone, Copy)]
enum Poll {
    /// Flip without DDFLIP_WAIT.
    Flip,
    /// GetFlipStatus with these flags, of the back buffer.
    FlipStatus(u32),
}

const DDGFS_CANFLIP: u32 = 1;
const DDGFS_ISFLIPDONE: u32 = 2;

/// Full screen with a flip chain: a flip at the start of a frame, so
/// that it shows a whole frame later, then Flip without DDFLIP_WAIT
/// called again and again until it succeeds, with `between` in between.
fn flip_polling_program(between: Between) -> (Vec<u8>, u32) {
    polling_program(Poll::Flip, between)
}

/// The same, with `poll` called again and again until it succeeds.
fn polling_program(poll: Poll, between: Between) -> (Vec<u8>, u32) {
    let mut d = data();
    desc(
        &mut d,
        DESC,
        DDSD_CAPS | DDSD_BACKBUFFERCOUNT,
        DDSCAPS_PRIMARYSURFACE | DDSCAPS_FLIP | DDSCAPS_COMPLEX,
        0,
        0,
        1,
    );
    put32(&mut d, CAPS, DDSCAPS_BACKBUFFER);
    program(&[USER32, DDRAW], &d, |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c, (0, 0, 320, 200))?;
        c.com(
            a,
            l,
            DD,
            DD_SET_COOPERATIVE_LEVEL,
            &[Arg::Val(HWND), Arg::I(DDSCL_EXCLUSIVE | DDSCL_FULLSCREEN)],
        )?;
        c.com(
            a,
            l,
            DD,
            DD_SET_DISPLAY_MODE,
            &[Arg::I(320), Arg::I(200), Arg::I(8)],
        )?;
        c.com(
            a,
            l,
            DD,
            DD_CREATE_SURFACE,
            &[Arg::At(DESC), Arg::At(FRONT), Arg::I(0)],
        )?;
        if let Poll::FlipStatus(_) = poll {
            c.com(
                a,
                l,
                FRONT,
                S_GET_ATTACHED_SURFACE,
                &[Arg::At(CAPS), Arg::At(BACK)],
            )?;
        }
        c.com(
            a,
            l,
            DD,
            DD_WAIT_FOR_VERTICAL_BLANK,
            &[Arg::I(DDWAITVB_BLOCKEND), Arg::I(0)],
        )?;
        c.com(a, l, FRONT, S_FLIP, &[Arg::I(0), Arg::I(1)])?;
        let mut again = a.create_label();
        a.set_label(&mut again)?;
        a.inc(dword_ptr(l.at(POLLS)))?;
        match between {
            Between::Nothing => {}
            Between::Work(passes) => {
                a.mov(ecx, passes)?;
                let mut busy = a.create_label();
                a.set_label(&mut busy)?;
                a.dec(ecx)?;
                a.jnz(busy)?;
            }
            Between::Call => {
                push(a, l, &[Arg::At(LINE)])?;
                a.mov(eax, dword_ptr(l.at(DD)))?;
                a.push(eax)?;
                a.mov(eax, dword_ptr(eax))?;
                a.call(dword_ptr(eax + 4 * DD_GET_SCAN_LINE))?;
            }
        }
        match poll {
            Poll::Flip => {
                push(a, l, &[Arg::I(0), Arg::I(0)])?;
                a.mov(eax, dword_ptr(l.at(FRONT)))?;
                a.push(eax)?;
                a.mov(eax, dword_ptr(eax))?;
                a.call(dword_ptr(eax + 4 * S_FLIP))?;
            }
            Poll::FlipStatus(flags) => {
                push(a, l, &[Arg::I(flags)])?;
                a.mov(eax, dword_ptr(l.at(BACK)))?;
                a.push(eax)?;
                a.mov(eax, dword_ptr(eax))?;
                a.call(dword_ptr(eax + 4 * S_GET_FLIP_STATUS))?;
            }
        }
        // The first call's result, kept.
        let mut kept = a.create_label();
        a.cmp(dword_ptr(l.at(POLLS)), 1)?;
        a.jne(kept)?;
        a.mov(dword_ptr(l.at(FIRST)), eax)?;
        a.set_label(&mut kept)?;
        a.cmp(eax, DDERR_WASSTILLDRAWING as i32)?;
        a.je(again)?;
        a.mov(dword_ptr(l.at(LAST)), eax)?;
        finish(a, l)
    })
}

#[test]
fn flip_polled_with_nothing_in_between_waits_for_the_frame() {
    let (program, data) = flip_polling_program(Between::Nothing);
    let mut cpu = machine("flip_polling", program);
    run_until_done(&mut cpu, data);
    assert_eq!(results(&mut cpu, data, 7)[1..], [0; 6]);
    // The flip came at the next frame, after 15 calls that failed and a
    // 16th that waited for the frame and flipped, instead of one call
    // for each pass of the loop until then.
    assert_eq!(peek(&cpu, data + LAST), 0);
    assert_eq!(peek(&cpu, data + POLLS), 16, "calls of Flip");
}

#[test]
fn flip_polled_with_work_in_between_runs_every_call() {
    // 3000 instructions between two calls: the program has more to do
    // than poll, and each call runs, failing until the frame.
    let (program, data) = flip_polling_program(Between::Work(1500));
    let mut cpu = machine("flip_polling_work", program);
    run_until_done(&mut cpu, data);
    assert_eq!(results(&mut cpu, data, 7)[1..], [0; 6]);
    assert_eq!(peek(&cpu, data + LAST), 0);
    let polls = peek(&cpu, data + POLLS);
    assert!(polls > 50, "{polls} calls of Flip");
}

#[test]
fn flip_polled_with_other_calls_in_between_runs_every_call() {
    // Another function called between two calls, as a loop that keeps
    // its sound buffer filled or its messages read while it waits: each
    // call of Flip runs, however few instructions the loop takes.
    let (program, data) = flip_polling_program(Between::Call);
    let mut cpu = machine("flip_polling_call", program);
    run_until_done(&mut cpu, data);
    assert_eq!(results(&mut cpu, data, 7)[1..], [0; 6]);
    assert_eq!(peek(&cpu, data + LAST), 0);
    let polls = peek(&cpu, data + POLLS);
    assert!(polls > 50, "{polls} calls of Flip");
}

#[test]
fn flip_status_polled_with_nothing_in_between_waits_for_the_frame() {
    // GetFlipStatus of the back buffer right after a flip: the flip is
    // still to show, as on Windows. A loop that only asks waits for the
    // frame at its 16th call, which then finds the flip done.
    let (program, data) = polling_program(Poll::FlipStatus(DDGFS_ISFLIPDONE), Between::Nothing);
    let mut cpu = machine("flip_status_polling", program);
    run_until_done(&mut cpu, data);
    assert_eq!(results(&mut cpu, data, 8)[1..], [0; 7]);
    assert_eq!(peek(&cpu, data + FIRST), DDERR_WASSTILLDRAWING);
    assert_eq!(peek(&cpu, data + LAST), 0);
    assert_eq!(peek(&cpu, data + POLLS), 16, "calls of GetFlipStatus");
}

#[test]
fn flip_status_polled_with_work_in_between_runs_every_call() {
    // DDGFS_CANFLIP the same: a flip would fail until the frame. With
    // more to do between two calls, each call runs.
    let (program, data) = polling_program(Poll::FlipStatus(DDGFS_CANFLIP), Between::Work(1500));
    let mut cpu = machine("flip_status_polling_work", program);
    run_until_done(&mut cpu, data);
    assert_eq!(results(&mut cpu, data, 8)[1..], [0; 7]);
    assert_eq!(peek(&cpu, data + FIRST), DDERR_WASSTILLDRAWING);
    assert_eq!(peek(&cpu, data + LAST), 0);
    let polls = peek(&cpu, data + POLLS);
    assert!(polls > 50, "{polls} calls of GetFlipStatus");
}

const DDERR_SURFACEBUSY: u32 = 0x8876_01AE;

#[test]
fn flip_status_says_a_locked_chain_cant_flip() {
    // With the back buffer locked, Flip would fail with
    // DDERR_SURFACEBUSY, and DDGFS_CANFLIP says so; DDGFS_ISFLIPDONE
    // doesn't, since no flip is pending. Once unlocked, it can flip.
    let mut d = data();
    desc(
        &mut d,
        DESC,
        DDSD_CAPS | DDSD_BACKBUFFERCOUNT,
        DDSCAPS_PRIMARYSURFACE | DDSCAPS_FLIP | DDSCAPS_COMPLEX,
        0,
        0,
        1,
    );
    put32(&mut d, CAPS, DDSCAPS_BACKBUFFER);
    put32(&mut d, LOCKED, 108);
    let (program, data) = program(&[USER32, DDRAW], &d, |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c, (0, 0, 320, 200))?;
        c.com(
            a,
            l,
            DD,
            DD_SET_COOPERATIVE_LEVEL,
            &[Arg::Val(HWND), Arg::I(DDSCL_EXCLUSIVE | DDSCL_FULLSCREEN)],
        )?;
        c.com(
            a,
            l,
            DD,
            DD_SET_DISPLAY_MODE,
            &[Arg::I(320), Arg::I(200), Arg::I(8)],
        )?;
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
            S_GET_ATTACHED_SURFACE,
            &[Arg::At(CAPS), Arg::At(BACK)],
        )?;
        c.com(
            a,
            l,
            BACK,
            S_LOCK,
            &[Arg::I(0), Arg::At(LOCKED), Arg::I(1), Arg::I(0)],
        )?;
        c.com(a, l, BACK, S_GET_FLIP_STATUS, &[Arg::I(DDGFS_CANFLIP)])?;
        c.com(a, l, BACK, S_GET_FLIP_STATUS, &[Arg::I(DDGFS_ISFLIPDONE)])?;
        c.com(a, l, BACK, S_UNLOCK, &[Arg::I(0)])?;
        c.com(a, l, FRONT, S_GET_FLIP_STATUS, &[Arg::I(DDGFS_CANFLIP)])?;
        finish(a, l)
    });
    let mut cpu = machine("flip_status_locked", program);
    run_until_done(&mut cpu, data);
    let r = results(&mut cpu, data, 11);
    assert_eq!(r[1..7], [0; 6]);
    assert_eq!(r[7..], [DDERR_SURFACEBUSY, 0, 0, 0]);
}

/// Full screen with a back buffer made on its own (DDSCAPS_BACKBUFFER
/// with a size) and attached to a plain primary surface, as Glover makes
/// its flip chain: a pixel written to the back buffer and a flip of the
/// primary that shows it. The back buffer has `back_caps` besides
/// DDSCAPS_BACKBUFFER. The two surfaces' caps go to CAPS after the
/// attach.
fn attached_back_buffer_program(back_caps: u32) -> (Vec<u8>, u32) {
    let mut d = data();
    desc(&mut d, DESC, DDSD_CAPS, DDSCAPS_PRIMARYSURFACE, 0, 0, 0);
    let back_desc = DESC + 0x70;
    desc(
        &mut d,
        back_desc,
        DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT,
        DDSCAPS_BACKBUFFER | back_caps,
        320,
        200,
        0,
    );
    put32(&mut d, LOCKED, 108);
    program(&[USER32, DDRAW], &d, |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c, (0, 0, 320, 200))?;
        c.com(
            a,
            l,
            DD,
            DD_SET_COOPERATIVE_LEVEL,
            &[Arg::Val(HWND), Arg::I(DDSCL_EXCLUSIVE | DDSCL_FULLSCREEN)],
        )?;
        c.com(
            a,
            l,
            DD,
            DD_SET_DISPLAY_MODE,
            &[Arg::I(320), Arg::I(200), Arg::I(8)],
        )?;
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
            DD,
            DD_CREATE_SURFACE,
            &[Arg::At(back_desc), Arg::At(BACK), Arg::I(0)],
        )?;
        c.com(a, l, FRONT, S_ADD_ATTACHED_SURFACE, &[Arg::Val(BACK)])?;
        c.com(a, l, FRONT, S_GET_CAPS, &[Arg::At(CAPS)])?;
        c.com(a, l, BACK, S_GET_CAPS, &[Arg::At(CAPS + 4)])?;
        c.com(
            a,
            l,
            BACK,
            S_LOCK,
            &[Arg::I(0), Arg::At(LOCKED), Arg::I(1), Arg::I(0)],
        )?;
        a.mov(edi, dword_ptr(l.at(LOCKED + 0x24)))?;
        a.mov(dword_ptr(edi), 0x4433_2211)?;
        c.com(a, l, BACK, S_UNLOCK, &[Arg::I(0)])?;
        c.com(a, l, FRONT, S_FLIP, &[Arg::I(0), Arg::I(1)])?;
        finish(a, l)
    })
}

#[test]
fn a_back_buffer_attached_to_the_primary_surface_flips_with_it() {
    let (program, data) = attached_back_buffer_program(0);
    let mut cpu = machine("attached_back_buffer", program);
    run_until_done(&mut cpu, data);
    assert_eq!(results(&mut cpu, data, 12)[1..], [0; 11]);
    // Windows 98 makes them a flip chain: both flip, and the primary is
    // its front buffer.
    let front = peek(&cpu, data + CAPS);
    let back = peek(&cpu, data + CAPS + 4);
    let flip_front = DDSCAPS_FLIP | DDSCAPS_FRONTBUFFER | DDSCAPS_PRIMARYSURFACE;
    assert_eq!(front & flip_front, flip_front);
    let flip_back = DDSCAPS_FLIP | DDSCAPS_BACKBUFFER;
    assert_eq!(back & flip_back, flip_back);
    // The flip shows the back buffer's pixels, away from the screen's
    // first 64000 bytes.
    let start = cpu.bus.vbe.start as usize;
    assert_ne!(start, 0);
    assert_eq!(cpu.bus.vbe.vram[start..start + 4], [0x11, 0x22, 0x33, 0x44]);
}

#[test]
fn a_back_buffer_in_system_memory_doesnt_flip_with_the_primary_surface() {
    let (program, data) = attached_back_buffer_program(DDSCAPS_SYSTEMMEMORY);
    let mut cpu = machine("attached_system_back_buffer", program);
    run_until_done(&mut cpu, data);
    // The attach works, but the screen can't show system memory, so
    // the two make no flip chain and the primary doesn't flip.
    const DDERR_NOTFLIPPABLE: u32 = 0x8876_0246;
    let mut expected = [0; 11];
    expected[10] = DDERR_NOTFLIPPABLE;
    assert_eq!(results(&mut cpu, data, 12)[1..], expected);
    let front = peek(&cpu, data + CAPS);
    let back = peek(&cpu, data + CAPS + 4);
    assert_eq!(front & (DDSCAPS_FLIP | DDSCAPS_FRONTBUFFER), 0);
    assert_eq!(back & DDSCAPS_FLIP, 0);
}

/// A primary surface that says it is a back buffer.
fn primary_back_buffer_program() -> (Vec<u8>, u32) {
    let mut d = data();
    desc(
        &mut d,
        DESC,
        DDSD_CAPS,
        DDSCAPS_PRIMARYSURFACE | DDSCAPS_BACKBUFFER,
        0,
        0,
        0,
    );
    program(&[USER32, DDRAW], &d, |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c, (0, 0, 320, 200))?;
        c.com(
            a,
            l,
            DD,
            DD_SET_COOPERATIVE_LEVEL,
            &[Arg::Val(HWND), Arg::I(DDSCL_EXCLUSIVE | DDSCL_FULLSCREEN)],
        )?;
        c.com(
            a,
            l,
            DD,
            DD_CREATE_SURFACE,
            &[Arg::At(DESC), Arg::At(FRONT), Arg::I(0)],
        )?;
        finish(a, l)
    })
}

#[test]
fn a_primary_surface_cant_be_a_back_buffer() {
    let (program, data) = primary_back_buffer_program();
    let mut cpu = machine("primary_back_buffer", program);
    run_until_done(&mut cpu, data);
    const DDERR_INVALIDCAPS: u32 = 0x8876_0064;
    assert_eq!(results(&mut cpu, data, 4)[1..], [0, 0, DDERR_INVALIDCAPS]);
}

/// A program in a window: an offscreen surface filled with a color and
/// blitted to the primary surface (the desktop), clipped to the window.
fn windowed_program() -> (Vec<u8>, u32) {
    let mut d = data();
    desc(&mut d, DESC, DDSD_CAPS, DDSCAPS_PRIMARYSURFACE, 0, 0, 0);
    desc(
        &mut d,
        LOCKED,
        DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT,
        DDSCAPS_OFFSCREENPLAIN,
        16,
        16,
        0,
    );
    put32(&mut d, FX, 100);
    put32(&mut d, FX + 0x50, 0x00FF_0000);
    // Half the blit is left of the window, which starts at (100, 50).
    for (i, v) in [92u32, 50, 108, 66].iter().enumerate() {
        put32(&mut d, RECT + 4 * i as u32, *v);
    }
    program(&[USER32, DDRAW], &d, |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c, (100, 50, 200, 100))?;
        c.com(
            a,
            l,
            DD,
            DD_SET_COOPERATIVE_LEVEL,
            &[Arg::Val(HWND), Arg::I(DDSCL_NORMAL)],
        )?;
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
            DD,
            DD_CREATE_SURFACE,
            &[Arg::At(LOCKED), Arg::At(OFFSCREEN), Arg::I(0)],
        )?;
        c.com(
            a,
            l,
            OFFSCREEN,
            S_BLT,
            &[
                Arg::I(0),
                Arg::I(0),
                Arg::I(0),
                Arg::I(DDBLT_COLORFILL | DDBLT_WAIT),
                Arg::At(FX),
            ],
        )?;
        c.api(
            a,
            l,
            DIRECT_DRAW_CREATE_CLIPPER,
            &[Arg::I(0), Arg::At(CLIPPER), Arg::I(0)],
        )?;
        c.com(a, l, CLIPPER, CLIP_SET_HWND, &[Arg::I(0), Arg::Val(HWND)])?;
        c.com(a, l, FRONT, S_SET_CLIPPER, &[Arg::Val(CLIPPER)])?;
        c.com(
            a,
            l,
            FRONT,
            S_BLT,
            &[
                Arg::At(RECT),
                Arg::Val(OFFSCREEN),
                Arg::I(0),
                Arg::I(DDBLT_WAIT),
                Arg::I(0),
            ],
        )?;
        finish(a, l)
    })
}

#[test]
fn a_windowed_program_blits_to_the_desktop_inside_its_window() {
    let (program, data) = windowed_program();
    let mut cpu = machine("windowed", program);
    run_until_done(&mut cpu, data);
    assert_eq!(results(&mut cpu, data, 10)[1..], [0; 9]);
    let pitch = cpu.bus.vbe.pitch as usize;
    let pixel = |cpu: &Cpu, x: usize, y: usize| {
        let at = y * pitch + x * 4;
        u32::from_le_bytes(cpu.bus.vbe.vram[at..at + 4].try_into().unwrap())
    };
    // In the window, red; left of it, the desktop as it was.
    for y in [50, 65] {
        for x in [100, 107] {
            assert_eq!(pixel(&cpu, x, y), 0x00FF_0000, "({x}, {y})");
        }
        assert_ne!(pixel(&cpu, 99, y), 0x00FF_0000);
        assert_ne!(pixel(&cpu, 92, y), 0x00FF_0000);
    }
    assert_ne!(pixel(&cpu, 100, 66), 0x00FF_0000);
}

/// A palette on the primary surface of an 8-bit mode, and an entry
/// changed after.
fn palette_program() -> (Vec<u8>, u32) {
    let mut d = data();
    desc(&mut d, DESC, DDSD_CAPS, DDSCAPS_PRIMARYSURFACE, 0, 0, 0);
    for i in 0..256u32 {
        put(&mut d, ENTRIES + 4 * i, &[i as u8, 0x40, 255 - i as u8, 0]);
    }
    put(&mut d, KEY, &[0x80, 0xC0, 0x10, 0]);
    program(&[USER32, DDRAW], &d, |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c, (0, 0, 640, 480))?;
        c.com(
            a,
            l,
            DD,
            DD_SET_COOPERATIVE_LEVEL,
            &[Arg::Val(HWND), Arg::I(DDSCL_EXCLUSIVE | DDSCL_FULLSCREEN)],
        )?;
        c.com(
            a,
            l,
            DD,
            DD_SET_DISPLAY_MODE,
            &[Arg::I(640), Arg::I(480), Arg::I(8)],
        )?;
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
            DD,
            DD_CREATE_PALETTE,
            &[
                Arg::I(DDPCAPS_8BIT | DDPCAPS_ALLOW256),
                Arg::At(ENTRIES),
                Arg::At(PALETTE),
                Arg::I(0),
            ],
        )?;
        c.com(a, l, FRONT, S_SET_PALETTE, &[Arg::Val(PALETTE)])?;
        c.com(
            a,
            l,
            PALETTE,
            PAL_SET_ENTRIES,
            &[Arg::I(0), Arg::I(7), Arg::I(1), Arg::At(KEY)],
        )?;
        finish(a, l)
    })
}

#[test]
fn a_palette_on_the_primary_surface_is_the_dacs() {
    let (program, data) = palette_program();
    let mut cpu = machine("palette", program);
    run_until_done(&mut cpu, data);
    assert_eq!(results(&mut cpu, data, 7)[1..], [0; 6]);
    let dac = |cpu: &Cpu, i: usize| cpu.bus.vga.palette[i * 3..i * 3 + 3].to_vec();
    // 6 bits a color, as the DAC has them.
    assert_eq!(dac(&cpu, 0), [0, 0x10, 0x3F]);
    assert_eq!(dac(&cpu, 200), [200 >> 2, 0x10, 55 >> 2]);
    assert_eq!(dac(&cpu, 7), [0x20, 0x30, 0x04]);
}

/// A blit through the source's color key, between system memory
/// surfaces, its result copied to PIXELS.
fn color_key_program() -> (Vec<u8>, u32) {
    let mut d = data();
    for offset in [DESC, LOCKED] {
        desc(
            &mut d,
            offset,
            DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT | DDSD_PIXELFORMAT,
            DDSCAPS_OFFSCREENPLAIN | DDSCAPS_SYSTEMMEMORY,
            4,
            1,
            0,
        );
        rgb32(&mut d, offset);
    }
    put32(&mut d, FX, 100);
    put32(&mut d, FX + 0x50, 0x0012_3456);
    put32(&mut d, KEY, 0x00FF_00FF);
    put32(&mut d, KEY + 4, 0x00FF_00FF);
    program(&[USER32, DDRAW], &d, |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c, (0, 0, 64, 64))?;
        c.com(
            a,
            l,
            DD,
            DD_SET_COOPERATIVE_LEVEL,
            &[Arg::Val(HWND), Arg::I(DDSCL_NORMAL)],
        )?;
        c.com(
            a,
            l,
            DD,
            DD_CREATE_SURFACE,
            &[Arg::At(DESC), Arg::At(OFFSCREEN), Arg::I(0)],
        )?;
        c.com(
            a,
            l,
            DD,
            DD_CREATE_SURFACE,
            &[Arg::At(LOCKED), Arg::At(SOURCE), Arg::I(0)],
        )?;
        // The source: the key, a color, the key, another.
        c.com(
            a,
            l,
            SOURCE,
            S_LOCK,
            &[Arg::I(0), Arg::At(LOCKED), Arg::I(1), Arg::I(0)],
        )?;
        a.mov(edi, dword_ptr(l.at(LOCKED + 0x24)))?;
        a.mov(dword_ptr(edi), 0x00FF_00FF)?;
        a.mov(dword_ptr(edi + 4), 0x0000_00AA)?;
        a.mov(dword_ptr(edi + 8), 0x00FF_00FF)?;
        a.mov(dword_ptr(edi + 12), 0x0000_BB00)?;
        c.com(a, l, SOURCE, S_UNLOCK, &[Arg::I(0)])?;
        c.com(
            a,
            l,
            SOURCE,
            S_SET_COLOR_KEY,
            &[Arg::I(DDCKEY_SRCBLT), Arg::At(KEY)],
        )?;
        c.com(
            a,
            l,
            OFFSCREEN,
            S_BLT,
            &[
                Arg::I(0),
                Arg::I(0),
                Arg::I(0),
                Arg::I(DDBLT_COLORFILL | DDBLT_WAIT),
                Arg::At(FX),
            ],
        )?;
        c.com(
            a,
            l,
            OFFSCREEN,
            S_BLT,
            &[
                Arg::I(0),
                Arg::Val(SOURCE),
                Arg::I(0),
                Arg::I(DDBLT_KEYSRC | DDBLT_WAIT),
                Arg::I(0),
            ],
        )?;
        c.com(
            a,
            l,
            OFFSCREEN,
            S_LOCK,
            &[Arg::I(0), Arg::At(DESC), Arg::I(1), Arg::I(0)],
        )?;
        a.mov(esi, dword_ptr(l.at(DESC + 0x24)))?;
        for i in 0..4 {
            a.mov(eax, dword_ptr(esi + 4 * i))?;
            a.mov(dword_ptr(l.at(PIXELS + 4 * i as u32)), eax)?;
        }
        c.com(a, l, OFFSCREEN, S_UNLOCK, &[Arg::I(0)])?;
        finish(a, l)
    })
}

#[test]
fn a_blit_through_a_color_key_leaves_the_keyed_pixels() {
    let (program, data) = color_key_program();
    let mut cpu = machine("color_key", program);
    run_until_done(&mut cpu, data);
    assert_eq!(results(&mut cpu, data, 11)[1..], [0; 10]);
    let pixels: Vec<u32> = (0..4).map(|i| peek(&cpu, data + PIXELS + 4 * i)).collect();
    assert_eq!(pixels, [0x0012_3456, 0xAA, 0x0012_3456, 0xBB00]);
}

#[test]
fn a_debugger_sees_the_surfaces_their_keys_and_their_pixels() {
    use rust_dos::win32::snapshot;
    let (program, data) = color_key_program();
    let mut cpu = machine("debug_surfaces", program);
    run_until_done(&mut cpu, data);
    let dd = snapshot::ddraw(&cpu);
    let keyed: Vec<_> = dd.surfaces.iter().filter(|s| !s.keys.is_empty()).collect();
    assert_eq!(keyed.len(), 1, "only the source has a key");
    let source = keyed[0];
    assert_eq!(
        (source.width, source.height, source.memory),
        (4, 1, "system")
    );
    assert_eq!(source.keys, [("src_blt", 0x00FF_00FF, 0x00FF_00FF)]);
    assert_eq!(source.interfaces, [peek(&cpu, data + SOURCE)]);
    let (w, h, rgba) = snapshot::surface_image(&cpu, source.id, true).unwrap();
    assert_eq!((w, h), (4, 1));
    let expected = [
        [255, 0, 255, 0],
        [0, 0, 0xAA, 255],
        [255, 0, 255, 0],
        [0, 0xBB, 0, 255],
    ];
    assert_eq!(rgba, expected.concat(), "the keyed pixels are transparent");
    let (_, _, plain) = snapshot::surface_image(&cpu, source.id, false).unwrap();
    assert_eq!(plain[3], 255, "without keyed, every pixel shows");
    let offscreen = dd
        .surfaces
        .iter()
        .find(|s| s.interfaces == [peek(&cpu, data + OFFSCREEN)])
        .unwrap();
    let (_, _, rgba) = snapshot::surface_image(&cpu, offscreen.id, false).unwrap();
    assert_eq!(
        rgba[..4],
        [0x12, 0x34, 0x56, 255],
        "the color fill the key left"
    );
    assert!(snapshot::surface_image(&cpu, 999, false).is_err());
}

// The screen's size, which follows the display mode.

const SCREEN_USER32: (&str, &[&str]) = (
    "USER32.DLL",
    &[
        "CreateWindowExA",
        "GetSystemMetrics",
        "SetCursorPos",
        "GetCursorPos",
        "PeekMessageA",
        "ChangeDisplaySettingsA",
        "EnumDisplaySettingsA",
        "GetDesktopWindow",
        "GetWindowRect",
        "IsWindowVisible",
        "GetWindowLongA",
    ],
);
const S_CREATE_WINDOW: usize = 0;
const S_GET_SYSTEM_METRICS: usize = 1;
const S_SET_CURSOR_POS: usize = 2;
const S_GET_CURSOR_POS: usize = 3;
const S_PEEK_MESSAGE: usize = 4;
const S_CHANGE_DISPLAY_SETTINGS: usize = 5;
const S_ENUM_DISPLAY_SETTINGS: usize = 6;
const S_GET_DESKTOP_WINDOW: usize = 7;
const S_GET_WINDOW_RECT: usize = 8;
const S_IS_WINDOW_VISIBLE: usize = 9;
const S_GET_WINDOW_LONG: usize = 10;
/// DirectDrawCreate, after SCREEN_USER32's.
const S_DIRECT_DRAW_CREATE: usize = 11;

const DD_RESTORE_DISPLAY_MODE: u32 = 19;
const WM_DISPLAYCHANGE: u32 = 0x7E;
const PM_REMOVE: u32 = 1;
const ENUM_CURRENT_SETTINGS: u32 = 0xFFFF_FFFF;
const DISP_CHANGE_BADMODE: u32 = -2i32 as u32;

// The screen programs' data, after the others'.
const POINT: u32 = 0x900;
const POINT2: u32 = 0x908;
const MSG: u32 = 0x910;
const MODE: u32 = 0x940;
const MODE2: u32 = 0x9C0;
const SCREEN_DATA_SIZE: usize = 0xA40;

/// The data of the screen programs: `data`'s, and room for their own.
fn screen_data() -> Vec<u8> {
    let mut d = data();
    d.resize(SCREEN_DATA_SIZE, 0);
    d
}

/// A visible popup window of `w` x `h` at the screen's corner.
fn screen_window(
    a: &mut CodeAssembler,
    l: &Layout,
    c: &mut Calls,
    size: (u32, u32),
) -> Result<(), IcedError> {
    styled_window(a, l, c, size, WS_POPUP | WS_VISIBLE)
}

/// A window of `style`, `w` x `h` at the screen's corner.
fn styled_window(
    a: &mut CodeAssembler,
    l: &Layout,
    c: &mut Calls,
    (w, h): (u32, u32),
    style: u32,
) -> Result<(), IcedError> {
    c.api(
        a,
        l,
        S_CREATE_WINDOW,
        &[
            Arg::I(0),
            Arg::At(CLASS),
            Arg::At(CLASS),
            Arg::I(style),
            Arg::I(0),
            Arg::I(0),
            Arg::I(w),
            Arg::I(h),
            Arg::I(0),
            Arg::I(0),
            Arg::I(BASE),
            Arg::I(0),
        ],
    )?;
    a.mov(dword_ptr(l.at(HWND)), eax)
}

/// GetSystemMetrics of SM_CXSCREEN and SM_CYSCREEN.
fn screen_metrics(a: &mut CodeAssembler, l: &Layout, c: &mut Calls) -> Result<(), IcedError> {
    c.api(a, l, S_GET_SYSTEM_METRICS, &[Arg::I(0)])?;
    c.api(a, l, S_GET_SYSTEM_METRICS, &[Arg::I(1)])
}

/// The first WM_DISPLAYCHANGE in the queue, taken out of it into MSG.
fn display_change(a: &mut CodeAssembler, l: &Layout, c: &mut Calls) -> Result<(), IcedError> {
    let filter = Arg::I(WM_DISPLAYCHANGE);
    let args = [Arg::At(MSG), Arg::I(0), filter, filter, Arg::I(PM_REMOVE)];
    c.api(a, l, S_PEEK_MESSAGE, &args)
}

/// Full screen at 800x600 from a 640x480 window, as Enemy Infestation
/// sets it: the screen's size, the cursor put at the bottom right, the
/// window's rectangle, the WM_DISPLAYCHANGE the window got, and the
/// size and the cursor after RestoreDisplayMode.
fn mode_screen_program() -> (Vec<u8>, u32) {
    program(
        &[SCREEN_USER32, ("DDRAW.DLL", &["DirectDrawCreate"])],
        &screen_data(),
        |a, l| {
            let mut c = Calls { next: 0 };
            screen_window(a, l, &mut c, (640, 480))?;
            let create = [Arg::I(0), Arg::At(DD), Arg::I(0)];
            c.api(a, l, S_DIRECT_DRAW_CREATE, &create)?;
            let level = Arg::I(DDSCL_EXCLUSIVE | DDSCL_FULLSCREEN);
            c.com(a, l, DD, DD_SET_COOPERATIVE_LEVEL, &[Arg::Val(HWND), level])?;
            let mode = [Arg::I(800), Arg::I(600), Arg::I(8)];
            c.com(a, l, DD, DD_SET_DISPLAY_MODE, &mode)?;
            screen_metrics(a, l, &mut c)?;
            c.api(a, l, S_SET_CURSOR_POS, &[Arg::I(760), Arg::I(580)])?;
            c.api(a, l, S_GET_CURSOR_POS, &[Arg::At(POINT)])?;
            let rect = [Arg::Val(HWND), Arg::At(RECT)];
            c.api(a, l, S_GET_WINDOW_RECT, &rect)?;
            display_change(a, l, &mut c)?;
            c.com(a, l, DD, DD_RESTORE_DISPLAY_MODE, &[])?;
            screen_metrics(a, l, &mut c)?;
            c.api(a, l, S_GET_CURSOR_POS, &[Arg::At(POINT2)])?;
            finish(a, l)
        },
    )
}

#[test]
fn the_screen_takes_the_size_of_the_mode_directdraw_sets() {
    let (program, data) = mode_screen_program();
    let mut cpu = machine("mode_screen", program);
    run_until_done(&mut cpu, data);
    let r = results(&mut cpu, data, 14);
    assert_eq!(r[1..4], [0; 3]);
    // GetSystemMetrics gives the mode's size, and the cursor goes to
    // its bottom right.
    assert_eq!(r[4..6], [800, 600]);
    assert_eq!(r[6..8], [1, 1]);
    let point = (peek(&cpu, data + POINT), peek(&cpu, data + POINT + 4));
    assert_eq!(point, (760, 580));
    // The window of exclusive mode covers the screen, where the cursor
    // reaches it.
    assert_eq!(r[8], 1);
    let rect: Vec<u32> = (0..4).map(|i| peek(&cpu, data + RECT + 4 * i)).collect();
    assert_eq!(rect, [0, 0, 800, 600]);
    // The window is told: 8 bits a pixel, 800x600.
    assert_eq!(r[9], 1);
    assert_eq!(peek(&cpu, data + MSG + 4), WM_DISPLAYCHANGE);
    assert_eq!(peek(&cpu, data + MSG + 8), 8);
    assert_eq!(peek(&cpu, data + MSG + 12), 600 << 16 | 800);
    // RestoreDisplayMode gives the desktop's size back, and the cursor
    // and the mouse stay on it.
    assert_eq!(r[10], 0);
    assert_eq!(r[11..13], [640, 480]);
    let point = (peek(&cpu, data + POINT2), peek(&cpu, data + POINT2 + 4));
    assert_eq!(point, (639, 479));
    let mouse = &cpu.bus.mouse;
    assert_eq!((mouse.max_x, mouse.max_y), (639, 479));
}

/// Full screen at 640x480 from a hidden popup window of that size that
/// the program never shows, as WarGames makes it: whether the window is
/// visible after SetDisplayMode, and its extended style then and after
/// SetCooperativeLevel with DDSCL_NORMAL.
fn hidden_window_program() -> (Vec<u8>, u32) {
    program(
        &[SCREEN_USER32, ("DDRAW.DLL", &["DirectDrawCreate"])],
        &screen_data(),
        |a, l| {
            let mut c = Calls { next: 0 };
            styled_window(a, l, &mut c, (640, 480), WS_POPUP)?;
            let create = [Arg::I(0), Arg::At(DD), Arg::I(0)];
            c.api(a, l, S_DIRECT_DRAW_CREATE, &create)?;
            let level = Arg::I(DDSCL_EXCLUSIVE | DDSCL_FULLSCREEN);
            c.com(a, l, DD, DD_SET_COOPERATIVE_LEVEL, &[Arg::Val(HWND), level])?;
            let mode = [Arg::I(640), Arg::I(480), Arg::I(8)];
            c.com(a, l, DD, DD_SET_DISPLAY_MODE, &mode)?;
            c.api(a, l, S_IS_WINDOW_VISIBLE, &[Arg::Val(HWND)])?;
            let gwl_exstyle = -20i32 as u32;
            let ex_style = [Arg::Val(HWND), Arg::I(gwl_exstyle)];
            c.api(a, l, S_GET_WINDOW_LONG, &ex_style)?;
            let normal = [Arg::Val(HWND), Arg::I(DDSCL_NORMAL)];
            c.com(a, l, DD, DD_SET_COOPERATIVE_LEVEL, &normal)?;
            c.api(a, l, S_GET_WINDOW_LONG, &ex_style)?;
            finish(a, l)
        },
    )
}

#[test]
fn the_hidden_window_of_exclusive_mode_is_shown_and_topmost() {
    let (program, data) = hidden_window_program();
    let mut cpu = machine("hidden_window", program);
    run_until_done(&mut cpu, data);
    let r = results(&mut cpu, data, 8);
    assert_eq!(r[1..4], [0; 3]);
    // Windows 98's DirectDraw shows the window of exclusive mode when the
    // mode is set, and makes it topmost, while it is the foreground
    // window: the mouse's clicks then reach it.
    assert_eq!(r[4], 1);
    assert_eq!(r[5] & WS_EX_TOPMOST, WS_EX_TOPMOST);
    // Leaving exclusive mode, SetCooperativeLevel makes it a plain
    // window again (SetWindowPos with HWND_NOTOPMOST).
    assert_eq!(r[6], 0);
    assert_eq!(r[7] & WS_EX_TOPMOST, 0);
}

/// A DEVMODEA at `offset` with `fields` and the size and depth given.
fn devmode(data: &mut [u8], offset: u32, fields: u32, (w, h, bpp): (u32, u32, u32)) {
    put32(data, offset + 36, 124);
    put32(data, offset + 40, fields);
    put32(data, offset + 104, bpp);
    put32(data, offset + 108, w);
    put32(data, offset + 112, h);
}

/// ChangeDisplaySettingsA: a size EnumDisplaySettingsA doesn't list,
/// then 800x600 with the depth left out, the screen's size, the
/// WM_DISPLAYCHANGE the window got, the desktop window's rectangle and
/// EnumDisplaySettingsA's mode now; with `restore`, then the desktop's
/// own mode again.
fn change_settings_program(restore: bool) -> (Vec<u8>, u32) {
    // DM_PELSWIDTH and DM_PELSHEIGHT.
    const SIZE: u32 = 0x18_0000;
    let mut d = screen_data();
    devmode(&mut d, MODE, SIZE, (1000, 700, 0));
    devmode(&mut d, MODE2, SIZE, (800, 600, 0));
    program(&[SCREEN_USER32], &d, |a, l| {
        let mut c = Calls { next: 0 };
        screen_window(a, l, &mut c, (200, 100))?;
        c.api(a, l, S_CHANGE_DISPLAY_SETTINGS, &[Arg::At(MODE), Arg::I(0)])?;
        c.api(
            a,
            l,
            S_CHANGE_DISPLAY_SETTINGS,
            &[Arg::At(MODE2), Arg::I(0)],
        )?;
        screen_metrics(a, l, &mut c)?;
        display_change(a, l, &mut c)?;
        c.api(a, l, S_GET_DESKTOP_WINDOW, &[])?;
        a.push(l.at(RECT) as i32)?;
        a.push(eax)?;
        a.call(l.import(S_GET_WINDOW_RECT))?;
        c.keep(a, l)?;
        let current = [Arg::I(0), Arg::I(ENUM_CURRENT_SETTINGS), Arg::At(MODE)];
        c.api(a, l, S_ENUM_DISPLAY_SETTINGS, &current)?;
        if restore {
            c.api(a, l, S_CHANGE_DISPLAY_SETTINGS, &[Arg::I(0), Arg::I(0)])?;
            screen_metrics(a, l, &mut c)?;
        }
        finish(a, l)
    })
}

#[test]
fn change_display_settings_sets_the_desktops_size() {
    let (program, data) = change_settings_program(false);
    let mut cpu = machine("change_settings", program);
    run_until_done(&mut cpu, data);
    let r = results(&mut cpu, data, 9);
    assert_eq!(r[1..3], [DISP_CHANGE_BADMODE, 0]);
    assert_eq!(r[3..5], [800, 600]);
    assert_eq!(r[5], 1);
    assert_eq!(peek(&cpu, data + MSG + 4), WM_DISPLAYCHANGE);
    assert_eq!(peek(&cpu, data + MSG + 12), 600 << 16 | 800);
    let rect: Vec<u32> = (0..4).map(|i| peek(&cpu, data + RECT + 4 * i)).collect();
    assert_eq!(rect, [0, 0, 800, 600]);
    assert_eq!(r[7..9], [1, 1]);
    let size = (peek(&cpu, data + MODE + 108), peek(&cpu, data + MODE + 112));
    assert_eq!(size, (800, 600));
    // The display shows the desktop at 800x600.
    let mode = cpu.bus.vbe.mode.expect("a VBE mode");
    assert_eq!((mode.width, mode.height), (800, 600));
}

#[test]
fn change_display_settings_without_a_mode_gives_the_desktops_back() {
    let (program, data) = change_settings_program(true);
    let mut cpu = machine("change_settings_back", program);
    run_until_done(&mut cpu, data);
    let r = results(&mut cpu, data, 12);
    assert_eq!(r[3..5], [800, 600]);
    assert_eq!(r[9..12], [0, 640, 480]);
    let mode = cpu.bus.vbe.mode.expect("a VBE mode");
    assert_eq!((mode.width, mode.height), (640, 480));
}

// GDI on the screen in a mode DirectDraw set.

const GDI_USER32: (&str, &[&str]) = ("USER32.DLL", &["CreateWindowExA", "GetDC", "FillRect"]);
const GDI_GDI32: (&str, &[&str]) = (
    "GDI32.DLL",
    &[
        "CreatePalette",
        "SelectPalette",
        "RealizePalette",
        "CreateSolidBrush",
    ],
);
const G_CREATE_WINDOW: usize = 0;
const G_GET_DC: usize = 1;
const G_FILL_RECT: usize = 2;
const G_DIRECT_DRAW_CREATE: usize = 3;
const G_CREATE_PALETTE: usize = 4;
const G_SELECT_PALETTE: usize = 5;
const G_REALIZE_PALETTE: usize = 6;
const G_CREATE_SOLID_BRUSH: usize = 7;

// The GDI program's data, after the others'.
const G_HDC: u32 = 0xA00;
const G_HPAL: u32 = 0xA04;
const G_BRUSH: u32 = 0xA08;
const G_RECT: u32 = 0xA10;
const G_LOGPAL: u32 = 0xA20;

/// The color the GDI program's palette has.
const G_COLOR: [u8; 3] = [0x40, 0x80, 0xC0];

/// Full screen at 640x480x8, then GDI on the window's DC, as Smacker
/// draws Myth's movies: a palette of one color realized, and a
/// rectangle filled with it from 10,10 to 20,20.
fn gdi_in_mode_program() -> (Vec<u8>, u32) {
    let mut d = screen_data();
    put32(&mut d, G_RECT, 10);
    put32(&mut d, G_RECT + 4, 10);
    put32(&mut d, G_RECT + 8, 20);
    put32(&mut d, G_RECT + 12, 20);
    // LOGPALETTE: version 0x300, one entry, PC_NOCOLLAPSE.
    put32(&mut d, G_LOGPAL, 0x0001_0300);
    put(
        &mut d,
        G_LOGPAL + 4,
        &[G_COLOR[0], G_COLOR[1], G_COLOR[2], 4],
    );
    let dlls = [
        GDI_USER32,
        ("DDRAW.DLL", &["DirectDrawCreate"][..]),
        GDI_GDI32,
    ];
    program(&dlls, &d, |a, l| {
        let mut c = Calls { next: 0 };
        c.api(
            a,
            l,
            G_CREATE_WINDOW,
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
        let create = [Arg::I(0), Arg::At(DD), Arg::I(0)];
        c.api(a, l, G_DIRECT_DRAW_CREATE, &create)?;
        let level = Arg::I(DDSCL_EXCLUSIVE | DDSCL_FULLSCREEN);
        c.com(a, l, DD, DD_SET_COOPERATIVE_LEVEL, &[Arg::Val(HWND), level])?;
        let mode = [Arg::I(640), Arg::I(480), Arg::I(8)];
        c.com(a, l, DD, DD_SET_DISPLAY_MODE, &mode)?;
        c.api(a, l, G_GET_DC, &[Arg::Val(HWND)])?;
        a.mov(dword_ptr(l.at(G_HDC)), eax)?;
        c.api(a, l, G_CREATE_PALETTE, &[Arg::At(G_LOGPAL)])?;
        a.mov(dword_ptr(l.at(G_HPAL)), eax)?;
        let select = [Arg::Val(G_HDC), Arg::Val(G_HPAL), Arg::I(0)];
        c.api(a, l, G_SELECT_PALETTE, &select)?;
        c.api(a, l, G_REALIZE_PALETTE, &[Arg::Val(G_HDC)])?;
        // PALETTEINDEX(0).
        c.api(a, l, G_CREATE_SOLID_BRUSH, &[Arg::I(0x0100_0000)])?;
        a.mov(dword_ptr(l.at(G_BRUSH)), eax)?;
        let fill = [Arg::Val(G_HDC), Arg::At(G_RECT), Arg::Val(G_BRUSH)];
        c.api(a, l, G_FILL_RECT, &fill)?;
        finish(a, l)
    })
}

#[test]
fn gdi_draws_on_the_screen_in_the_mode_directdraw_set() {
    let (program, data) = gdi_in_mode_program();
    let mut cpu = machine("gdi_in_mode", program);
    run_until_done(&mut cpu, data);
    let r = results(&mut cpu, data, 10);
    assert_eq!(r[1..4], [0; 3]);
    // The palette's color takes the first entry after the static ones,
    // in the DAC too, and the rectangle is that entry.
    assert_eq!(r[7], 1);
    assert_eq!(r[9], 1);
    let shift = if cpu.bus.vga.dac_8bit { 0 } else { 2 };
    let dac = &cpu.bus.vga.palette[10 * 3..11 * 3];
    assert_eq!(dac, G_COLOR.map(|v| v >> shift));
    let vram = &cpu.bus.vbe.vram;
    assert_eq!(vram[10 * 640 + 10..10 * 640 + 20], [10; 10]);
    assert_eq!(vram[19 * 640 + 19], 10);
    assert_eq!(vram[20 * 640 + 20], 0);
}

const DD_QUERY_INTERFACE: u32 = 0;
/// Where the program that leaves exclusive mode keeps the IDirectDraw2
/// it asks for, and IID_IDirectDraw2.
const DD2: u32 = 0x20;
const IID: u32 = 0x330;
const IID_IDIRECTDRAW2: [u8; 16] = [
    0xE0, 0xF3, 0xA6, 0xB3, 0x43, 0x2B, 0xCF, 0x11, 0xA2, 0xDE, 0x00, 0xAA, 0x00, 0xB9, 0x33, 0x56,
];

/// Full screen at 640x480 in 8 bits, a palette of its own on the primary
/// surface, and back to DDSCL_NORMAL with the object kept, as The Tone
/// Rebellion's windowed mode and MechCommander's assert dialog do: all
/// through IDirectDraw2 with `v2`, else through IDirectDraw. With
/// `again`, back to exclusive mode after it.
fn leave_exclusive_program(v2: bool, again: bool) -> (Vec<u8>, u32) {
    let mut d = data();
    desc(&mut d, DESC, DDSD_CAPS, DDSCAPS_PRIMARYSURFACE, 0, 0, 0);
    for i in 0..256u32 {
        put(&mut d, ENTRIES + 4 * i, &[i as u8, 0x40, 255 - i as u8, 0]);
    }
    put(&mut d, IID, &IID_IDIRECTDRAW2);
    program(&[USER32, DDRAW], &d, |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c, (0, 0, 640, 480))?;
        let dd = if v2 {
            let query = [Arg::At(IID), Arg::At(DD2)];
            c.com(a, l, DD, DD_QUERY_INTERFACE, &query)?;
            DD2
        } else {
            DD
        };
        let level = Arg::I(DDSCL_EXCLUSIVE | DDSCL_FULLSCREEN);
        c.com(a, l, dd, DD_SET_COOPERATIVE_LEVEL, &[Arg::Val(HWND), level])?;
        let mut mode = vec![Arg::I(640), Arg::I(480), Arg::I(8)];
        if v2 {
            mode.extend([Arg::I(0), Arg::I(0)]);
        }
        c.com(a, l, dd, DD_SET_DISPLAY_MODE, &mode)?;
        let primary = [Arg::At(DESC), Arg::At(FRONT), Arg::I(0)];
        c.com(a, l, dd, DD_CREATE_SURFACE, &primary)?;
        let caps = Arg::I(DDPCAPS_8BIT | DDPCAPS_ALLOW256);
        let palette = [caps, Arg::At(ENTRIES), Arg::At(PALETTE), Arg::I(0)];
        c.com(a, l, dd, DD_CREATE_PALETTE, &palette)?;
        c.com(a, l, FRONT, S_SET_PALETTE, &[Arg::Val(PALETTE)])?;
        let normal = [Arg::Val(HWND), Arg::I(DDSCL_NORMAL)];
        c.com(a, l, dd, DD_SET_COOPERATIVE_LEVEL, &normal)?;
        if again {
            c.com(a, l, dd, DD_SET_COOPERATIVE_LEVEL, &[Arg::Val(HWND), level])?;
        }
        finish(a, l)
    })
}

/// The DAC's entry `i`, as the DAC has it, and `color` as it has it.
fn dac_entry(cpu: &Cpu, i: usize, color: [u8; 3]) -> ([u8; 3], [u8; 3]) {
    let shift = if cpu.bus.vga.dac_8bit { 0 } else { 2 };
    let dac = &cpu.bus.vga.palette[i * 3..i * 3 + 3];
    ([dac[0], dac[1], dac[2]], color.map(|v| v >> shift))
}

#[test]
fn leaving_exclusive_mode_through_idirectdraw2_gives_the_desktop_back() {
    let (program, data) = leave_exclusive_program(true, false);
    let mut cpu = machine("leave_exclusive_v2", program);
    run_until_done(&mut cpu, data);
    assert_eq!(results(&mut cpu, data, 9)[1..], [0; 8]);
    // The desktop's mode is back, not the program's 8 bits.
    let bpp = cpu.bus.vbe.mode.map(|m| m.bpp);
    assert!(bpp.is_some_and(|b| b > 8), "{bpp:?}");
}

#[test]
fn leaving_exclusive_mode_through_idirectdraw_keeps_the_mode_with_gdis_palette() {
    let (program, data) = leave_exclusive_program(false, false);
    let mut cpu = machine("leave_exclusive_v1", program);
    run_until_done(&mut cpu, data);
    assert_eq!(results(&mut cpu, data, 8)[1..], [0; 7]);
    // Windows keeps the mode for a program that set its level through
    // IDirectDraw, but GDI's system palette is in the DAC again in place
    // of the program's, so what GDI draws has its colors: the static
    // ones at both ends.
    assert_eq!(cpu.bus.vbe.mode.map(|m| m.bpp), Some(8));
    for (i, color) in [(0, [0, 0, 0]), (1, [0x80, 0, 0]), (3, [0x80, 0x80, 0])] {
        let (dac, expected) = dac_entry(&cpu, i, color);
        assert_eq!(dac, expected, "entry {i}");
    }
    let (dac, expected) = dac_entry(&cpu, 255, [0xFF; 3]);
    assert_eq!(dac, expected, "entry 255");
}

#[test]
fn back_in_exclusive_mode_the_primary_surfaces_palette_is_the_dacs_again() {
    let (program, data) = leave_exclusive_program(false, true);
    let mut cpu = machine("leave_exclusive_again", program);
    run_until_done(&mut cpu, data);
    assert_eq!(results(&mut cpu, data, 9)[1..], [0; 8]);
    // GDI's static colors went in the DAC when exclusive mode ended, and
    // the program's palette is back in their place.
    for i in [0, 1, 255] {
        let (dac, expected) = dac_entry(&cpu, i, [i as u8, 0x40, 255 - i as u8]);
        assert_eq!(dac, expected, "entry {i}");
    }
}

const S_RESTORE: u32 = 27;
const DDSD_MIPMAPCOUNT: u32 = 0x2_0000;
const DDSCAPS_TEXTURE: u32 = 0x1000;
const DDSCAPS_VIDEOMEMORY: u32 = 0x4000;
const DDSCAPS_MIPMAP: u32 = 0x40_0000;
const MIPMAP_TOP: u32 = PALETTE;
const MIPMAP_SECOND: u32 = CLIPPER;
const MIPMAP_THIRD: u32 = SOURCE;
const LEVEL_CAPS: u32 = CAPS;

/// A mipmap of `size` by `size` 5-6-5 texels and `levels` levels described
/// at FX, and the caps that find its levels.
fn mipmap_desc(d: &mut [u8], caps: u32, size: u32, levels: u32) {
    desc(
        d,
        FX,
        DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT | DDSD_PIXELFORMAT | DDSD_MIPMAPCOUNT,
        DDSCAPS_TEXTURE | DDSCAPS_MIPMAP | DDSCAPS_COMPLEX | caps,
        size,
        size,
        0,
    );
    put32(d, FX + 0x18, levels);
    let pf = FX + 0x48;
    put32(d, pf, 32);
    put32(d, pf + 4, 0x40);
    put32(d, pf + 12, 16);
    put32(d, pf + 16, 0xF800);
    put32(d, pf + 20, 0x7E0);
    put32(d, pf + 24, 0x1F);
    put32(d, LEVEL_CAPS, DDSCAPS_TEXTURE | DDSCAPS_MIPMAP);
}

/// The program's start for a mipmap test: full screen in 320x200.
fn mipmap_start(a: &mut CodeAssembler, l: &Layout, c: &mut Calls) -> Result<(), IcedError> {
    start(a, l, c, (0, 0, 320, 200))?;
    c.com(
        a,
        l,
        DD,
        DD_SET_COOPERATIVE_LEVEL,
        &[Arg::Val(HWND), Arg::I(DDSCL_EXCLUSIVE | DDSCL_FULLSCREEN)],
    )?;
    c.com(
        a,
        l,
        DD,
        DD_SET_DISPLAY_MODE,
        &[Arg::I(320), Arg::I(200), Arg::I(8)],
    )
}

/// Make the mipmap FX describes, and get its second and third levels.
fn make_mipmap(a: &mut CodeAssembler, l: &Layout, c: &mut Calls) -> Result<(), IcedError> {
    c.com(
        a,
        l,
        DD,
        DD_CREATE_SURFACE,
        &[Arg::At(FX), Arg::At(MIPMAP_TOP), Arg::I(0)],
    )?;
    for (from, to) in [(MIPMAP_TOP, MIPMAP_SECOND), (MIPMAP_SECOND, MIPMAP_THIRD)] {
        c.com(
            a,
            l,
            from,
            S_GET_ATTACHED_SURFACE,
            &[Arg::At(LEVEL_CAPS), Arg::At(to)],
        )?;
    }
    Ok(())
}

/// A program that leaves a hole in video memory a level of the mipmap it
/// makes next fits in, and locks the mipmap's top three levels. With
/// `restored` it loses the mipmap to a mode change first, and restores
/// its second level before its top level.
fn mipmap_program(restored: bool) -> (Vec<u8>, u32) {
    const S_RELEASE: u32 = 2;
    const TOP: u32 = MIPMAP_TOP;
    const SECOND: u32 = MIPMAP_SECOND;
    const THIRD: u32 = MIPMAP_THIRD;
    let mut d = data();
    let plain = |d: &mut [u8], offset, size| {
        desc(
            d,
            offset,
            DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT,
            DDSCAPS_OFFSCREENPLAIN | DDSCAPS_VIDEOMEMORY,
            size,
            size,
            0,
        );
    };
    // The hole is between two blocks. At 8 bits a pixel it has 1024
    // bytes: the third level's 512 fit, the second's 2048 don't.
    plain(&mut d, DESC, 32);
    plain(&mut d, DESC + 0x70, 32);
    mipmap_desc(&mut d, DDSCAPS_VIDEOMEMORY, 64, 3);
    put32(&mut d, LOCKED, 108);
    program(&[USER32, DDRAW], &d, |a, l| {
        let mut c = Calls { next: 0 };
        mipmap_start(a, l, &mut c)?;
        for (desc, out) in [(DESC, FRONT), (DESC + 0x70, BACK), (DESC, OFFSCREEN)] {
            c.com(
                a,
                l,
                DD,
                DD_CREATE_SURFACE,
                &[Arg::At(desc), Arg::At(out), Arg::I(0)],
            )?;
        }
        c.com(a, l, BACK, S_RELEASE, &[])?;
        make_mipmap(a, l, &mut c)?;
        if restored {
            c.com(
                a,
                l,
                DD,
                DD_SET_DISPLAY_MODE,
                &[Arg::I(640), Arg::I(480), Arg::I(8)],
            )?;
            c.com(a, l, SECOND, S_RESTORE, &[])?;
            c.com(a, l, TOP, S_RESTORE, &[])?;
        }
        for (level, kept) in [(TOP, POLLS), (SECOND, LAST), (THIRD, LINE)] {
            c.com(
                a,
                l,
                level,
                S_LOCK,
                &[Arg::I(0), Arg::At(LOCKED), Arg::I(1), Arg::I(0)],
            )?;
            a.mov(eax, dword_ptr(l.at(LOCKED + 0x24)))?;
            a.mov(dword_ptr(l.at(kept)), eax)?;
            c.com(a, l, level, S_UNLOCK, &[Arg::I(0)])?;
        }
        finish(a, l)
    })
}

/// A mipmap's levels in video memory lie one after the other whatever
/// holes there are: a Banshee's or Voodoo3's texture unit has one base
/// address for all of them, and a level left elsewhere cuts the texture
/// off from its smaller levels.
#[test]
fn a_mipmaps_levels_lie_together_in_video_memory() {
    let (program, data) = mipmap_program(false);
    let mut cpu = machine("mipmap", program);
    run_until_done(&mut cpu, data);
    let results = results(&mut cpu, data, 17);
    assert_eq!(results[1..], [0; 16], "the calls' results");
    let (top, second, third) = (
        peek(&cpu, data + POLLS),
        peek(&cpu, data + LAST),
        peek(&cpu, data + LINE),
    );
    assert_eq!(second as i64 - top as i64, 64 * 64 * 2);
    assert_eq!(third as i64 - second as i64, 32 * 32 * 2);
}

/// Restoring a level before its top level brings the whole mipmap back
/// together: left to itself the level would take a block of its own, and
/// the texture would be cut off from it for good.
#[test]
fn a_mipmap_restored_from_a_level_keeps_its_levels_together() {
    let (program, data) = mipmap_program(true);
    let mut cpu = machine("mipmap_restored", program);
    run_until_done(&mut cpu, data);
    let results = results(&mut cpu, data, 20);
    assert_eq!(results[1..], [0; 19], "the calls' results");
    let (top, second, third) = (
        peek(&cpu, data + POLLS),
        peek(&cpu, data + LAST),
        peek(&cpu, data + LINE),
    );
    assert_eq!(second as i64 - top as i64, 64 * 64 * 2);
    assert_eq!(third as i64 - second as i64, 32 * 32 * 2);
}

/// A mipmap whose block doesn't fit in video memory is in system memory
/// with all its levels, though the second level would fit on its own.
#[test]
fn a_mipmap_too_big_for_video_memory_is_in_system_memory_with_its_levels() {
    let mut d = data();
    // 8 MB and 2 MB of the adapter's 4 MB.
    mipmap_desc(&mut d, 0, 2048, 3);
    let (program, data) = program(&[USER32, DDRAW], &d, |a, l| {
        let mut c = Calls { next: 0 };
        mipmap_start(a, l, &mut c)?;
        make_mipmap(a, l, &mut c)?;
        for (level, kept) in [
            (MIPMAP_TOP, POLLS),
            (MIPMAP_SECOND, LAST),
            (MIPMAP_THIRD, LINE),
        ] {
            c.com(a, l, level, S_GET_CAPS, &[Arg::At(kept)])?;
        }
        finish(a, l)
    });
    let mut cpu = machine("mipmap_system", program);
    run_until_done(&mut cpu, data);
    let results = results(&mut cpu, data, 10);
    assert_eq!(results[1..], [0; 9], "the calls' results");
    for at in [POLLS, LAST, LINE] {
        let caps = peek(&cpu, data + at);
        assert_eq!(
            caps & DDSCAPS_SYSTEMMEMORY,
            DDSCAPS_SYSTEMMEMORY,
            "{caps:#x}"
        );
        assert_eq!(caps & DDSCAPS_VIDEOMEMORY, 0, "{caps:#x}");
    }
}

// The frames `cycles=auto` sees (#524).

/// A loop of 40,000 instructions: more than end a burst of writes to
/// video memory (`autospeed::BURST_GAP`), as a game's work on its next
/// frame does.
fn work(a: &mut CodeAssembler) -> Result<(), IcedError> {
    work_for(a, 20_000)
}

/// A loop of `passes` passes, two instructions each.
fn work_for(a: &mut CodeAssembler, passes: u32) -> Result<(), IcedError> {
    a.mov(ecx, passes)?;
    let mut again = a.create_label();
    a.set_label(&mut again)?;
    a.dec(ecx)?;
    a.jnz(again)
}

/// `run_until_done`, with the display brought up to date after each
/// batch as the frontend does once a frame: at its retraces the frame
/// counts learn the size of the screen (`Activity::set_screen`).
fn run_displayed_until_done(cpu: &mut Cpu, layout_data: u32) {
    for _ in 0..400 {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        run_batch(cpu, &mut NoHook, false);
        cpu.bus.sync_display();
        let paged = cpu.cr0 & 0x8000_0001 == 0x8000_0001;
        if paged && peek(cpu, layout_data + DONE) == 1 {
            return;
        }
    }
    panic!("the program didn't finish: {:?}", printed(cpu));
}

/// The frames `cycles=auto` counted: those the built-in Windows showed,
/// and the program's own bursts and copies.
fn activity(cpu: &Cpu) -> [u64; 3] {
    let a = &cpu.bus.activity;
    [a.presents, a.bursts, a.copies]
}

const SHOWN_FRAMES: u32 = 5;

/// Full screen in a 320x200 8-bit mode, as Magic & Mayhem draws: each
/// frame made in system memory, copied to a surface in video memory out
/// of sight and then blitted to the primary surface, `SHOWN_FRAMES`
/// times. Then a pixel filled on the primary surface ends the last
/// frame's burst of writes.
fn blitting_program() -> (Vec<u8>, u32) {
    let mut d = data();
    desc(&mut d, DESC, DDSD_CAPS, DDSCAPS_PRIMARYSURFACE, 0, 0, 0);
    let plain = DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT;
    let system = DDSCAPS_OFFSCREENPLAIN | DDSCAPS_SYSTEMMEMORY;
    desc(&mut d, LOCKED, plain, system, 320, 200, 0);
    desc(&mut d, ENTRIES, plain, DDSCAPS_OFFSCREENPLAIN, 320, 200, 0);
    put32(&mut d, FX, 100);
    put32(&mut d, FX + 0x50, 7);
    for (i, v) in [0u32, 0, 1, 1].iter().enumerate() {
        put32(&mut d, RECT + 4 * i as u32, *v);
    }
    program(&[USER32, DDRAW], &d, |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c, (0, 0, 320, 200))?;
        let exclusive = DDSCL_EXCLUSIVE | DDSCL_FULLSCREEN;
        let level = [Arg::Val(HWND), Arg::I(exclusive)];
        c.com(a, l, DD, DD_SET_COOPERATIVE_LEVEL, &level)?;
        let mode = [Arg::I(320), Arg::I(200), Arg::I(8)];
        c.com(a, l, DD, DD_SET_DISPLAY_MODE, &mode)?;
        for (desc, at) in [(DESC, FRONT), (LOCKED, SOURCE), (ENTRIES, OFFSCREEN)] {
            let args = [Arg::At(desc), Arg::At(at), Arg::I(0)];
            c.com(a, l, DD, DD_CREATE_SURFACE, &args)?;
        }
        for _ in 0..SHOWN_FRAMES {
            for to in [OFFSCREEN, FRONT] {
                let args = [
                    Arg::I(0),
                    Arg::Val(SOURCE),
                    Arg::I(0),
                    Arg::I(DDBLT_WAIT),
                    Arg::I(0),
                ];
                c.com(a, l, to, S_BLT, &args)?;
                work(a)?;
            }
        }
        let fill = [
            Arg::At(RECT),
            Arg::I(0),
            Arg::I(0),
            Arg::I(DDBLT_COLORFILL | DDBLT_WAIT),
            Arg::At(FX),
        ];
        c.com(a, l, FRONT, S_BLT, &fill)?;
        finish(a, l)
    })
}

#[test]
fn a_frame_blitted_to_the_primary_surface_counts_and_one_out_of_sight_doesnt() {
    let (program, data) = blitting_program();
    let mut cpu = machine("blitted_frames", program);
    let before = activity(&cpu);
    run_displayed_until_done(&mut cpu, data);
    let calls = 8 + 2 * SHOWN_FRAMES as usize;
    assert_eq!(
        results(&mut cpu, data, calls as u32)[1..],
        vec![0; calls - 1]
    );
    // Each blit to the primary surface is a frame shown; the copies to
    // the surface out of sight, in the same video memory, are none, and
    // the pixel filled is too small to be one. The program itself wrote
    // nothing to video memory.
    let after = activity(&cpu);
    let counted: Vec<u64> = after.iter().zip(before).map(|(a, b)| a - b).collect();
    assert_eq!(counted, [SHOWN_FRAMES as u64, 0, 0]);
}

const GDI_FRAMES: u32 = 3;

/// GDI's drawing on the screen's DC, after some work: `GDI_FRAMES`
/// rectangles as big as the desktop, then as many of 8x8 pixels, and a
/// last one of those to end the bursts.
fn gdi_program() -> (Vec<u8>, u32) {
    let imports: &[(&str, &[&str])] = &[("USER32.DLL", &["GetDC"]), ("GDI32.DLL", &["PatBlt"])];
    const GET_DC: usize = 0;
    const PAT_BLT: usize = 1;
    const WHITENESS: u32 = 0x00FF_0062;
    program(imports, &data(), |a, l| {
        let mut c = Calls { next: 0 };
        c.api(a, l, GET_DC, &[Arg::I(0)])?;
        a.mov(dword_ptr(l.at(HWND)), eax)?;
        // Starting up for a few batches, as the desktop shows for frames
        // before a program draws on it.
        work_for(a, 200_000)?;
        let (w, h) = (640, 480);
        for size in [(w, h), (8, 8)] {
            for _ in 0..GDI_FRAMES {
                let args = [
                    Arg::Val(HWND),
                    Arg::I(0),
                    Arg::I(0),
                    Arg::I(size.0),
                    Arg::I(size.1),
                    Arg::I(WHITENESS),
                ];
                c.api(a, l, PAT_BLT, &args)?;
                work(a)?;
            }
        }
        let args = [
            Arg::Val(HWND),
            Arg::I(0),
            Arg::I(0),
            Arg::I(8),
            Arg::I(8),
            Arg::I(WHITENESS),
        ];
        c.api(a, l, PAT_BLT, &args)?;
        finish(a, l)
    })
}

#[test]
fn gdis_drawing_of_the_whole_screen_counts_as_frames_and_small_drawing_doesnt() {
    let (program, data) = gdi_program();
    let mut cpu = machine("gdi_frames", program);
    let before = activity(&cpu);
    run_displayed_until_done(&mut cpu, data);
    let calls = 2 + 2 * GDI_FRAMES as usize;
    let results = results(&mut cpu, data, calls as u32);
    assert!(results[1..].iter().all(|&r| r == 1), "{results:X?}");
    let after = activity(&cpu);
    let counted: Vec<u64> = after.iter().zip(before).map(|(a, b)| a - b).collect();
    assert_eq!(counted, [GDI_FRAMES as u64, 0, 0]);
}

const DD_ENUM_SURFACES: u32 = 9;
const DDENUMSURFACES_ALL: u32 = 0x1;
const DDENUMSURFACES_MATCH: u32 = 0x2;
const DDENUMSURFACES_CANBECREATED: u32 = 0x8;
const DDENUMSURFACES_DOESEXIST: u32 = 0x10;
/// What the EnumSurfaces callback saw: how often it was called, its
/// surface and the description's flags, height, width, pitch, bits per
/// pixel and caps; and the callback's address.
const E_COUNT: u32 = 0x890;
const E_SURFACE: u32 = 0x894;
const E_DESC: u32 = 0x898;
const E_CALLBACK: u32 = 0x8B0;

/// EnumSurfaces asked for a system memory surface that could be made,
/// with no size; then for one with DDENUMSURFACES_ALL, which can't go
/// with DDENUMSURFACES_CANBECREATED; then for the surfaces that exist.
fn can_be_created_program() -> (Vec<u8>, u32) {
    let mut d = data();
    let caps = DDSCAPS_OFFSCREENPLAIN | DDSCAPS_SYSTEMMEMORY;
    desc(&mut d, DESC, DDSD_CAPS, caps, 0, 0, 0);
    program(&[USER32, DDRAW], &d, |a, l| {
        // The call over the callback pushes the callback's address.
        let mut code = a.create_label();
        a.call(code)?;
        // The callback (surface, desc, context): it keeps what it got
        // and goes on (DDENUMRET_OK).
        a.mov(eax, dword_ptr(esp + 4))?;
        a.mov(dword_ptr(l.at(E_SURFACE)), eax)?;
        a.mov(eax, dword_ptr(esp + 8))?;
        for (i, field) in [0x04, 0x08, 0x0C, 0x10, 0x54, 0x68].iter().enumerate() {
            a.mov(ecx, dword_ptr(eax + *field))?;
            a.mov(dword_ptr(l.at(E_DESC + 4 * i as u32)), ecx)?;
        }
        a.inc(dword_ptr(l.at(E_COUNT)))?;
        a.mov(eax, 1)?;
        a.ret_1(12)?;
        a.set_label(&mut code)?;
        a.pop(eax)?;
        a.mov(dword_ptr(l.at(E_CALLBACK)), eax)?;
        let mut c = Calls { next: 0 };
        start(a, l, &mut c, (0, 0, 64, 64))?;
        let level = [Arg::Val(HWND), Arg::I(DDSCL_NORMAL)];
        c.com(a, l, DD, DD_SET_COOPERATIVE_LEVEL, &level)?;
        for flags in [
            DDENUMSURFACES_CANBECREATED | DDENUMSURFACES_MATCH,
            DDENUMSURFACES_CANBECREATED | DDENUMSURFACES_ALL,
            DDENUMSURFACES_DOESEXIST | DDENUMSURFACES_ALL,
        ] {
            let args = [
                Arg::I(flags),
                Arg::At(DESC),
                Arg::I(0),
                Arg::Val(E_CALLBACK),
            ];
            c.com(a, l, DD, DD_ENUM_SURFACES, &args)?;
        }
        finish(a, l)
    })
}

#[test]
fn enum_surfaces_describes_a_surface_that_can_be_created() {
    let (program, data) = can_be_created_program();
    let mut cpu = machine("can_be_created", program);
    run_until_done(&mut cpu, data);
    let r = results(&mut cpu, data, 6);
    assert_eq!(r[1..], [0, 0, 0, 0x8007_0057, 0], "{r:X?}");
    // One call, for the surface that could be made, with no surface:
    // the one made for it went, so the surfaces that exist are none.
    assert_eq!(peek(&cpu, data + E_COUNT), 1);
    assert_eq!(peek(&cpu, data + E_SURFACE), 0);
    let field = |i: u32| peek(&cpu, data + E_DESC + 4 * i);
    let flags = DDSD_CAPS | DDSD_HEIGHT | DDSD_WIDTH | DDSD_PIXELFORMAT;
    assert_eq!(field(0) & flags, flags);
    // Without a size in the description, it is 512 by 512.
    assert_eq!((field(1), field(2)), (512, 512));
    let bits = field(4);
    assert!(matches!(bits, 8 | 16 | 24 | 32), "{bits}");
    assert!(field(3) >= 512 * bits / 8, "pitch {}", field(3));
    let caps = DDSCAPS_OFFSCREENPLAIN | DDSCAPS_SYSTEMMEMORY;
    assert_eq!(field(5) & caps, caps);
}

// Blits from DXT surfaces. The expected pixels are what DirectX 7's
// DirectDraw wrote on Windows 98 SE for the same blocks and blits, read
// back by a probe program run there.

const S_BLT_FAST: u32 = 7;
const DDBLT_DDFX: u32 = 0x800;
const DDBLTFAST_SRCCOLORKEY: u32 = 0x1;
const DDBLTFAST_DESTCOLORKEY: u32 = 0x2;
const DDBLTFAST_WAIT: u32 = 0x10;
const DDBLTFX_MIRRORLEFTRIGHT: u32 = 0x2;
const DDPF_FOURCC: u32 = 0x4;
const DDPF_ALPHAPIXELS: u32 = 0x1;
const DDPF_RGB: u32 = 0x40;
const DDERR_INVALIDPARAMS: u32 = 0x8007_0057;
const E_NOTIMPL: u32 = 0x8000_4001;

/// The probe's DXT1 to DXT5 blocks: a four-colour block (0xFFE0 and
/// 0x0843) and a three-colour one (0x001F and 0xF800), codes 0 to 3 in
/// each row; DXT2 and DXT3 alpha 0 to 15; DXT4 and DXT5 the eight
/// values between 200 and 40, or the six between 40 and 200, codes 0 to
/// 7 twice.
fn probe_block(fourcc: &[u8; 4], i: usize) -> Vec<u8> {
    let colour: [u8; 8] = if i.is_multiple_of(2) {
        [0xE0, 0xFF, 0x43, 0x08, 0xE4, 0xE4, 0xE4, 0xE4]
    } else {
        [0x1F, 0x00, 0x00, 0xF8, 0xE4, 0xE4, 0xE4, 0xE4]
    };
    let codes = [0x88, 0xC6, 0xFA, 0x88, 0xC6, 0xFA];
    let alpha: Vec<u8> = match fourcc {
        b"DXT1" => vec![],
        b"DXT2" | b"DXT3" => vec![0x10, 0x32, 0x54, 0x76, 0x98, 0xBA, 0xDC, 0xFE],
        _ if i.is_multiple_of(2) => [&[200, 40][..], &codes].concat(),
        _ => [&[40, 200][..], &codes].concat(),
    };
    [alpha, colour.to_vec()].concat()
}

/// The 8x8 surface of four blocks the probe made: the four-colour block
/// top left and bottom right.
fn probe_blocks(fourcc: &[u8; 4]) -> Vec<u8> {
    [0, 1, 1, 0]
        .iter()
        .flat_map(|&i| probe_block(fourcc, i))
        .collect()
}

/// A DDSURFACEDESC at `offset` for an 8x8 system memory texture of the
/// DXT format `fourcc`.
fn dxt_desc(data: &mut [u8], offset: u32, fourcc: &[u8; 4], w: u32, h: u32) {
    let flags = DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT | DDSD_PIXELFORMAT;
    desc(
        data,
        offset,
        flags,
        DDSCAPS_TEXTURE | DDSCAPS_SYSTEMMEMORY,
        w,
        h,
        0,
    );
    put32(data, offset + 0x48, 32);
    put32(data, offset + 0x4C, DDPF_FOURCC);
    put(data, offset + 0x50, fourcc);
}

/// An RGB pixel format: bits, then the red, green, blue and alpha masks.
type Rgb = (u32, u32, u32, u32, u32);

/// A DDSURFACEDESC at `offset` for a system memory texture of `w` by
/// `h` pixels of the format `f`.
fn rgb_desc(data: &mut [u8], offset: u32, f: Rgb, w: u32, h: u32) {
    let flags = DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT | DDSD_PIXELFORMAT;
    desc(
        data,
        offset,
        flags,
        DDSCAPS_TEXTURE | DDSCAPS_SYSTEMMEMORY,
        w,
        h,
        0,
    );
    let pf = offset + 0x48;
    let alpha = if f.4 != 0 { DDPF_ALPHAPIXELS } else { 0 };
    put32(data, pf, 32);
    put32(data, pf + 4, DDPF_RGB | alpha);
    put32(data, pf + 12, f.0);
    put32(data, pf + 16, f.1);
    put32(data, pf + 20, f.2);
    put32(data, pf + 24, f.3);
    put32(data, pf + 28, f.4);
}

const R5G6B5: Rgb = (16, 0xF800, 0x07E0, 0x001F, 0);
const X1R5G5B5: Rgb = (16, 0x7C00, 0x03E0, 0x001F, 0);
const A1R5G5B5: Rgb = (16, 0x7C00, 0x03E0, 0x001F, 0x8000);
const A4R4G4B4: Rgb = (16, 0x0F00, 0x00F0, 0x000F, 0xF000);
const R8G8B8: Rgb = (24, 0xFF_0000, 0xFF00, 0xFF, 0);
const A8R8G8B8: Rgb = (32, 0xFF_0000, 0xFF00, 0xFF, 0xFF00_0000);
const X8R8G8B8: Rgb = (32, 0xFF_0000, 0xFF00, 0xFF, 0);

/// Where the DXT programs keep their blocks, more surface descriptions
/// and rectangles.
const BLOCKS: u32 = 0x900;
const DESC3: u32 = 0x980;
const DESC4: u32 = 0xA00;
const RECTS: u32 = 0xA80;
const DXT_DATA_SIZE: usize = 0xB00;

/// Copy `n` dwords from the data at `from` to the surface the
/// description at `desc` locked.
fn copy_in(
    a: &mut CodeAssembler,
    l: &Layout,
    from: u32,
    desc: u32,
    n: u32,
) -> Result<(), IcedError> {
    a.mov(edi, dword_ptr(l.at(desc + 0x24)))?;
    a.mov(esi, l.at(from) as u32)?;
    a.mov(ecx, n)?;
    a.cld()?;
    a.rep().movsd()
}

/// A window, a DirectDraw object and the normal cooperative level, then
/// the source of DESC at SOURCE, filled with BLOCKS, and the surfaces of
/// the other descriptions given.
fn dxt_start(
    a: &mut CodeAssembler,
    l: &Layout,
    c: &mut Calls,
    block_bytes: u32,
    others: &[(u32, u32)],
) -> Result<(), IcedError> {
    start(a, l, c, (0, 0, 64, 64))?;
    let level = [Arg::Val(HWND), Arg::I(DDSCL_NORMAL)];
    c.com(a, l, DD, DD_SET_COOPERATIVE_LEVEL, &level)?;
    let args = [Arg::At(DESC), Arg::At(SOURCE), Arg::I(0)];
    c.com(a, l, DD, DD_CREATE_SURFACE, &args)?;
    for &(desc, out) in others {
        c.com(
            a,
            l,
            DD,
            DD_CREATE_SURFACE,
            &[Arg::At(desc), Arg::At(out), Arg::I(0)],
        )?;
    }
    let lock = [Arg::I(0), Arg::At(DESC), Arg::I(1), Arg::I(0)];
    c.com(a, l, SOURCE, S_LOCK, &lock)?;
    copy_in(a, l, BLOCKS, DESC, block_bytes / 4)?;
    c.com(a, l, SOURCE, S_UNLOCK, &[Arg::I(0)])
}

/// Lock and unlock the surface at `object` with the description at
/// `desc`, which then says where its pixels are.
fn find_pixels(
    a: &mut CodeAssembler,
    l: &Layout,
    c: &mut Calls,
    object: u32,
    desc: u32,
) -> Result<(), IcedError> {
    c.com(
        a,
        l,
        object,
        S_LOCK,
        &[Arg::I(0), Arg::At(desc), Arg::I(1), Arg::I(0)],
    )?;
    c.com(a, l, object, S_UNLOCK, &[Arg::I(0)])
}

/// The pixels of `w` by `h` of `bytes` each of the surface a lock
/// described at `desc`, row by row.
fn locked_pixels(cpu: &Cpu, data: u32, desc: u32, (w, h): (u32, u32), bytes: u32) -> Vec<u32> {
    let pitch = peek(cpu, data + desc + 0x10);
    let bits = peek(cpu, data + desc + 0x24);
    let mut out = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let at = bits + y * pitch + x * bytes;
            // A pixel's bytes, read a byte at a time: a dword read could
            // run off the end of the surface's pages.
            let value = (0..bytes).fold(0, |v, i| {
                let b = peek(cpu, (at + i) & !3) >> (8 * ((at + i) & 3)) & 0xFF;
                v | b << (8 * i)
            });
            out.push(value);
        }
    }
    out
}

/// The probe's output: rows of hexadecimal pixels.
fn probe_pixels(text: &str) -> Vec<u32> {
    text.split_whitespace()
        .map(|v| u32::from_str_radix(v, 16).unwrap())
        .collect()
}

/// A program that BltFasts the probe's 8x8 surface of `fourcc` into an
/// 8x8 surface of the format `f`.
fn dxt_decode_program(fourcc: &[u8; 4], f: Rgb) -> (Vec<u8>, u32) {
    let mut d = data();
    d.resize(DXT_DATA_SIZE, 0);
    dxt_desc(&mut d, DESC, fourcc, 8, 8);
    rgb_desc(&mut d, LOCKED, f, 8, 8);
    let blocks = probe_blocks(fourcc);
    put(&mut d, BLOCKS, &blocks);
    program(&[USER32, DDRAW], &d, |a, l| {
        let mut c = Calls { next: 0 };
        dxt_start(a, l, &mut c, blocks.len() as u32, &[(LOCKED, OFFSCREEN)])?;
        let args = [
            Arg::I(0),
            Arg::I(0),
            Arg::Val(SOURCE),
            Arg::I(0),
            Arg::I(DDBLTFAST_WAIT),
        ];
        c.com(a, l, OFFSCREEN, S_BLT_FAST, &args)?;
        find_pixels(a, l, &mut c, OFFSCREEN, LOCKED)?;
        finish(a, l)
    })
}

/// The pixels a BltFast of the probe's surface of `fourcc` into an 8x8
/// surface of the format `f` leaves, and its result.
fn dxt_decoded(test: &str, fourcc: &[u8; 4], f: Rgb) -> (u32, Vec<u32>) {
    let (program, data) = dxt_decode_program(fourcc, f);
    let mut cpu = machine(test, program);
    run_until_done(&mut cpu, data);
    let r = results(&mut cpu, data, 10);
    assert_eq!(r[1..7], [0; 6], "{r:X?}");
    let pixels = locked_pixels(&cpu, data, LOCKED, (8, 8), f.0 / 8);
    (r[7], pixels)
}

fn assert_decodes_as_windows(test: &str, fourcc: &[u8; 4], f: Rgb, windows: &str) {
    let (hr, pixels) = dxt_decoded(test, fourcc, f);
    assert_eq!(hr, 0);
    let expected = probe_pixels(windows);
    let hex = |p: &[u32]| p.iter().map(|v| format!("{v:X}")).collect::<Vec<_>>();
    assert_eq!(hex(&pixels), hex(&expected));
}

#[test]
fn bltfast_decodes_dxt1_into_a1r5g5b5_as_windows() {
    let windows = "
        ffe0 8423 d6a1 ad62 801f fc00 bc0f 0000
        ffe0 8423 d6a1 ad62 801f fc00 bc0f 0000
        ffe0 8423 d6a1 ad62 801f fc00 bc0f 0000
        ffe0 8423 d6a1 ad62 801f fc00 bc0f 0000
        801f fc00 bc0f 0000 ffe0 8423 d6a1 ad62
        801f fc00 bc0f 0000 ffe0 8423 d6a1 ad62
        801f fc00 bc0f 0000 ffe0 8423 d6a1 ad62
        801f fc00 bc0f 0000 ffe0 8423 d6a1 ad62";
    assert_decodes_as_windows("dxt1_a1r5g5b5", b"DXT1", A1R5G5B5, windows);
}

#[test]
fn bltfast_decodes_dxt1_into_r8g8b8_as_windows() {
    let windows = "
        ffff00 080818 adad08 5a5a10 0000ff ff0000 7f007f 000000
        ffff00 080818 adad08 5a5a10 0000ff ff0000 7f007f 000000
        ffff00 080818 adad08 5a5a10 0000ff ff0000 7f007f 000000
        ffff00 080818 adad08 5a5a10 0000ff ff0000 7f007f 000000
        0000ff ff0000 7f007f 000000 ffff00 080818 adad08 5a5a10
        0000ff ff0000 7f007f 000000 ffff00 080818 adad08 5a5a10
        0000ff ff0000 7f007f 000000 ffff00 080818 adad08 5a5a10
        0000ff ff0000 7f007f 000000 ffff00 080818 adad08 5a5a10";
    assert_decodes_as_windows("dxt1_r8g8b8", b"DXT1", R8G8B8, windows);
}

/// DXT2's colours stay at most their alpha, which an X1R5G5B5 surface
/// drops.
#[test]
fn bltfast_decodes_dxt2_into_x1r5g5b5_as_windows() {
    let windows = "
        0000 0422 1081 18c2 0000 0800 1004 0000
        2100 0423 3181 2d62 0008 2800 300c 0000
        4620 0423 56a1 2d62 0011 4c00 3c0f 0000
        6720 0423 56a1 2d62 0019 6c00 3c0f 0000
        0000 0800 1004 0000 0000 0422 1081 18c2
        0008 2800 300c 0000 2100 0423 3181 2d62
        0011 4c00 3c0f 0000 4620 0423 56a1 2d62
        0019 6c00 3c0f 0000 6720 0423 56a1 2d62";
    assert_decodes_as_windows("dxt2_x1r5g5b5", b"DXT2", X1R5G5B5, windows);
}

#[test]
fn bltfast_decodes_dxt3_into_a4r4g4b4_as_windows() {
    let windows = "
        0ff0 1001 2aa0 3551 000f 1f00 2707 3000
        4ff0 5001 6aa0 7551 400f 5f00 6707 7000
        8ff0 9001 aaa0 b551 800f 9f00 a707 b000
        cff0 d001 eaa0 f551 c00f df00 e707 f000
        000f 1f00 2707 3000 0ff0 1001 2aa0 3551
        400f 5f00 6707 7000 4ff0 5001 6aa0 7551
        800f 9f00 a707 b000 8ff0 9001 aaa0 b551
        c00f df00 e707 f000 cff0 d001 eaa0 f551";
    assert_decodes_as_windows("dxt3_a4r4g4b4", b"DXT3", A4R4G4B4, windows);
}

/// An X8R8G8B8 surface gets the alpha in its top byte.
#[test]
fn bltfast_decodes_dxt4_into_x8r8g8b8_as_windows() {
    let windows = "
        c8c8c800 28080818 3f3f3f08 56565610 28000028 c8c80000 a87f007f 88000000
        6d6d6d00 83080818 9a9a9a08 b15a5a10 68000068 48480000 00000000 ff000000
        c8c8c800 28080818 3f3f3f08 56565610 28000028 c8c80000 a87f007f 88000000
        6d6d6d00 83080818 9a9a9a08 b15a5a10 68000068 48480000 00000000 ff000000
        28000028 c8c80000 a87f007f 88000000 c8c8c800 28080818 3f3f3f08 56565610
        68000068 48480000 00000000 ff000000 6d6d6d00 83080818 9a9a9a08 b15a5a10
        28000028 c8c80000 a87f007f 88000000 c8c8c800 28080818 3f3f3f08 56565610
        68000068 48480000 00000000 ff000000 6d6d6d00 83080818 9a9a9a08 b15a5a10";
    assert_decodes_as_windows("dxt4_x8r8g8b8", b"DXT4", X8R8G8B8, windows);
}

#[test]
fn bltfast_decodes_dxt5_into_a8r8g8b8_as_windows() {
    let windows = "
        c8ffff00 28080818 3fadad08 565a5a10 280000ff c8ff0000 a87f007f 88000000
        6dffff00 83080818 9aadad08 b15a5a10 680000ff 48ff0000 007f007f ff000000
        c8ffff00 28080818 3fadad08 565a5a10 280000ff c8ff0000 a87f007f 88000000
        6dffff00 83080818 9aadad08 b15a5a10 680000ff 48ff0000 007f007f ff000000
        280000ff c8ff0000 a87f007f 88000000 c8ffff00 28080818 3fadad08 565a5a10
        680000ff 48ff0000 007f007f ff000000 6dffff00 83080818 9aadad08 b15a5a10
        280000ff c8ff0000 a87f007f 88000000 c8ffff00 28080818 3fadad08 565a5a10
        680000ff 48ff0000 007f007f ff000000 6dffff00 83080818 9aadad08 b15a5a10";
    assert_decodes_as_windows("dxt5_a8r8g8b8", b"DXT5", A8R8G8B8, windows);
}

/// What The Operative: No One Lives Forever does with its DXT textures.
#[test]
fn bltfast_decodes_dxt5_into_a1r5g5b5_as_windows() {
    let windows = "
        ffe0 0423 56a1 2d62 001f fc00 bc0f 8000
        7fe0 8423 d6a1 ad62 001f 7c00 3c0f 8000
        ffe0 0423 56a1 2d62 001f fc00 bc0f 8000
        7fe0 8423 d6a1 ad62 001f 7c00 3c0f 8000
        001f fc00 bc0f 8000 ffe0 0423 56a1 2d62
        001f 7c00 3c0f 8000 7fe0 8423 d6a1 ad62
        001f fc00 bc0f 8000 ffe0 0423 56a1 2d62
        001f 7c00 3c0f 8000 7fe0 8423 d6a1 ad62";
    assert_decodes_as_windows("dxt5_a1r5g5b5", b"DXT5", A1R5G5B5, windows);
}

/// DXT2's and DXT4's premultiplied colours don't go to a format with
/// alpha.
#[test]
fn premultiplied_dxt_doesnt_go_to_a_format_with_alpha() {
    for (test, fourcc) in [("dxt2_alpha", b"DXT2"), ("dxt4_alpha", b"DXT4")] {
        let (hr, pixels) = dxt_decoded(test, fourcc, A8R8G8B8);
        assert_eq!(hr, E_NOTIMPL);
        assert!(pixels.iter().all(|&p| p == 0), "{pixels:X?}");
    }
}

/// Four DXT1 blocks of 8x8 pixels: red, green, blue, and white and
/// black and the two greys between in a pattern.
const FOUR_BLOCKS: [u8; 32] = [
    0x00, 0xF8, 0x1F, 0x00, 0x00, 0x00, 0x00, 0x00, //
    0xE0, 0x07, 0x1F, 0x00, 0x00, 0x00, 0x00, 0x00, //
    0x1F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    0xFF, 0xFF, 0x00, 0x00, 0x1B, 0x6C, 0xB1, 0xC6, //
];

/// FOUR_BLOCKS stretched, cut and mirrored into a 16x16 R5G6B5
/// surface, copied block by block into another DXT1 surface, and the
/// blits that fail.
fn dxt_blits_program() -> (Vec<u8>, u32) {
    let mut d = data();
    d.resize(DXT_DATA_SIZE, 0);
    dxt_desc(&mut d, DESC, b"DXT1", 8, 8);
    rgb_desc(&mut d, LOCKED, R5G6B5, 16, 16);
    dxt_desc(&mut d, DESC3, b"DXT1", 8, 8);
    dxt_desc(&mut d, DESC4, b"DXT3", 8, 8);
    put(&mut d, BLOCKS, &FOUR_BLOCKS);
    let rects: [[i32; 4]; 6] = [
        [0, 0, 4, 4],
        [8, 0, 16, 8],
        [2, 1, 7, 6],
        [4, 0, 8, 4],
        [2, 2, 6, 6],
        [0, 0, 16, 16],
    ];
    for (i, r) in rects.iter().enumerate() {
        for (j, v) in r.iter().enumerate() {
            put32(&mut d, RECTS + 16 * i as u32 + 4 * j as u32, *v as u32);
        }
    }
    put32(&mut d, FX, 100);
    put32(&mut d, FX + 4, DDBLTFX_MIRRORLEFTRIGHT);
    put32(&mut d, KEY, 0xFFE0);
    put32(&mut d, KEY + 4, 0xFFE0);
    let rect = |i: u32| Arg::At(RECTS + 16 * i);
    program(&[USER32, DDRAW], &d, |a, l| {
        let mut c = Calls { next: 0 };
        let others = [(LOCKED, OFFSCREEN), (DESC3, FRONT), (DESC4, BACK)];
        dxt_start(a, l, &mut c, 32, &others)?;
        let src = Arg::Val(SOURCE);
        let wait = Arg::I(DDBLT_WAIT);
        let fast = |x: u32, y: u32, r: Arg, flags: u32| {
            [Arg::I(x), Arg::I(y), src, r, Arg::I(DDBLTFAST_WAIT | flags)]
        };
        // 9: shrunk to 4x4; 10: mirrored at (8, 0); 11: a rectangle
        // within blocks to (0, 8).
        c.com(
            a,
            l,
            OFFSCREEN,
            S_BLT,
            &[rect(0), src, Arg::I(0), wait, Arg::I(0)],
        )?;
        let mirror = [
            rect(1),
            src,
            Arg::I(0),
            Arg::I(DDBLT_WAIT | DDBLT_DDFX),
            Arg::At(FX),
        ];
        c.com(a, l, OFFSCREEN, S_BLT, &mirror)?;
        c.com(a, l, OFFSCREEN, S_BLT_FAST, &fast(0, 8, rect(2), 0))?;
        // 12: the second block to the third place of a DXT1 surface; 13:
        // a rectangle within blocks; 14: into DXT3; 15: from R5G6B5.
        c.com(a, l, FRONT, S_BLT_FAST, &fast(0, 4, rect(3), 0))?;
        c.com(a, l, FRONT, S_BLT_FAST, &fast(0, 0, rect(4), 0))?;
        c.com(a, l, BACK, S_BLT_FAST, &fast(0, 0, Arg::I(0), 0))?;
        let rgb = [
            Arg::I(0),
            Arg::I(0),
            Arg::Val(OFFSCREEN),
            rect(0),
            Arg::I(DDBLTFAST_WAIT),
        ];
        c.com(a, l, FRONT, S_BLT_FAST, &rgb)?;
        // 16: a fill of a DXT surface.
        let fill = [
            Arg::I(0),
            Arg::I(0),
            Arg::I(0),
            Arg::I(DDBLT_COLORFILL | DDBLT_WAIT),
            Arg::At(FX),
        ];
        c.com(a, l, FRONT, S_BLT, &fill)?;
        // 17: a source key the source hasn't got; 18, 19, 20: one it
        // has; 21: a destination key.
        let all = rect(5);
        c.com(
            a,
            l,
            OFFSCREEN,
            S_BLT_FAST,
            &fast(0, 0, Arg::I(0), DDBLTFAST_SRCCOLORKEY),
        )?;
        c.com(
            a,
            l,
            SOURCE,
            S_SET_COLOR_KEY,
            &[Arg::I(DDCKEY_SRCBLT), Arg::At(KEY)],
        )?;
        c.com(
            a,
            l,
            OFFSCREEN,
            S_BLT_FAST,
            &fast(0, 0, Arg::I(0), DDBLTFAST_SRCCOLORKEY),
        )?;
        let keyed = [
            all,
            src,
            Arg::I(0),
            Arg::I(DDBLT_KEYSRC | DDBLT_WAIT),
            Arg::I(0),
        ];
        c.com(a, l, OFFSCREEN, S_BLT, &keyed)?;
        c.com(
            a,
            l,
            OFFSCREEN,
            S_BLT_FAST,
            &fast(0, 0, Arg::I(0), DDBLTFAST_DESTCOLORKEY),
        )?;
        // 22 to 25.
        find_pixels(a, l, &mut c, OFFSCREEN, LOCKED)?;
        find_pixels(a, l, &mut c, FRONT, DESC3)?;
        finish(a, l)
    })
}

#[test]
fn blits_from_dxt_stretch_cut_blocks_and_mirror_as_windows() {
    let (program, data) = dxt_blits_program();
    let mut cpu = machine("dxt_blits", program);
    run_until_done(&mut cpu, data);
    let r = results(&mut cpu, data, 26);
    assert_eq!(r[1..13], [0; 12], "{r:X?}");
    let pixels = locked_pixels(&cpu, data, LOCKED, (16, 16), 2);
    let at = |x: usize, y: usize, w: usize, h: usize| -> Vec<u32> {
        (y..y + h)
            .flat_map(|y| pixels[y * 16 + x..][..w].to_vec())
            .collect()
    };
    // Windows 98 SE's pixels: the shrink samples the odd pixels.
    let shrunk = "
        f800 f800 07e0 07e0
        f800 f800 07e0 07e0
        001f 001f 52aa 0000
        001f 001f 0000 52aa";
    assert_eq!(at(0, 0, 4, 4), probe_pixels(shrunk));
    let mirrored = "
        07e0 07e0 07e0 07e0 f800 f800 f800 f800
        07e0 07e0 07e0 07e0 f800 f800 f800 f800
        07e0 07e0 07e0 07e0 f800 f800 f800 f800
        07e0 07e0 07e0 07e0 f800 f800 f800 f800
        ffff 0000 ad55 52aa 001f 001f 001f 001f
        0000 ad55 52aa ffff 001f 001f 001f 001f
        ad55 52aa ffff 0000 001f 001f 001f 001f
        52aa ffff 0000 ad55 001f 001f 001f 001f";
    assert_eq!(at(8, 0, 8, 8), probe_pixels(mirrored));
    let cut = "
        f800 f800 07e0 07e0 07e0
        f800 f800 07e0 07e0 07e0
        f800 f800 07e0 07e0 07e0
        001f 001f 52aa ad55 0000
        001f 001f ffff 52aa ad55";
    assert_eq!(at(0, 8, 5, 5), probe_pixels(cut));
}

#[test]
fn a_dxt_surface_takes_whole_blocks_of_its_format_and_nothing_else() {
    let (program, data) = dxt_blits_program();
    let mut cpu = machine("dxt_to_dxt", program);
    run_until_done(&mut cpu, data);
    let r = results(&mut cpu, data, 26);
    assert_eq!(r[12], 0, "{r:X?}");
    // Windows compresses these; the built-in DirectDraw doesn't.
    assert_eq!(r[13..16], [E_NOTIMPL; 3], "{r:X?}");
    // A fill of a DXT surface fails on Windows too.
    assert_eq!(r[16], E_NOTIMPL);
    assert_eq!(r[22..26], [0; 4], "{r:X?}");
    let blocks = locked_pixels(&cpu, data, DESC3, (8, 1), 4);
    let green = [0x001F_07E0, 0];
    assert_eq!(blocks[..4], [0; 4], "{blocks:X?}");
    assert_eq!(blocks[4..6], green, "{blocks:X?}");
    assert_eq!(blocks[6..], [0; 2], "{blocks:X?}");
}

#[test]
fn colour_keys_on_a_blit_from_dxt_fail_as_on_windows() {
    let (program, data) = dxt_blits_program();
    let mut cpu = machine("dxt_keys", program);
    run_until_done(&mut cpu, data);
    let r = results(&mut cpu, data, 22);
    assert_eq!(
        r[17..22],
        [
            DDERR_INVALIDPARAMS,
            0,
            E_NOTIMPL,
            E_NOTIMPL,
            DDERR_INVALIDPARAMS
        ],
        "{r:X?}"
    );
}

/// A 6x6 DXT1 texture, and an 8x8 one asked for without a memory,
/// locked.
fn dxt_create_program() -> (Vec<u8>, u32) {
    let mut d = data();
    d.resize(DXT_DATA_SIZE, 0);
    dxt_desc(&mut d, DESC, b"DXT1", 6, 6);
    dxt_desc(&mut d, DESC3, b"DXT1", 8, 8);
    put32(&mut d, DESC3 + 0x68, DDSCAPS_TEXTURE);
    program(&[USER32, DDRAW], &d, |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c, (0, 0, 64, 64))?;
        let level = [Arg::Val(HWND), Arg::I(DDSCL_NORMAL)];
        c.com(a, l, DD, DD_SET_COOPERATIVE_LEVEL, &level)?;
        for (desc, out) in [(DESC, SOURCE), (DESC3, FRONT)] {
            let args = [Arg::At(desc), Arg::At(out), Arg::I(0)];
            c.com(a, l, DD, DD_CREATE_SURFACE, &args)?;
        }
        find_pixels(a, l, &mut c, FRONT, DESC3)?;
        finish(a, l)
    })
}

/// DirectX 7 on Windows 98 SE makes DXT surfaces of whole blocks only,
/// and puts one asked for without a memory in system memory, its pitch
/// the size of its blocks.
#[test]
fn dxt_surfaces_are_whole_blocks_in_system_memory() {
    let (program, data) = dxt_create_program();
    let mut cpu = machine("dxt_create", program);
    run_until_done(&mut cpu, data);
    let r = results(&mut cpu, data, 7);
    assert_eq!(r[3..7], [DDERR_INVALIDPARAMS, 0, 0, 0], "{r:X?}");
    let caps = peek(&cpu, data + DESC3 + 0x68);
    assert_eq!(caps & DDSCAPS_SYSTEMMEMORY, DDSCAPS_SYSTEMMEMORY);
    assert_eq!(peek(&cpu, data + DESC3 + 0x10), 32);
}
