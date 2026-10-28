//! The built-in Windows' screen on a Voodoo3, whose own memory and
//! modes the desktop, GDI and DirectDraw use: the desktop at 16 bits a
//! pixel in the card's extended mode, GDI drawing on it, DirectDraw's
//! full-screen modes set through the card's registers and flipped by its
//! display start, Lock giving the card's memory, and the desktop back
//! after. The programs are PE files made with rust-dos-pe's builder,
//! their code assembled with iced, as in win32_ddraw_tests.rs; each
//! leaves its results in its data and spins, and the test looks at them
//! and at the card while the program is still there.
//!
//! The 3dfx device of a Voodoo Graphics or Voodoo2 add-on card, beside
//! the machine's own display adapter, is DirectDraw's second device:
//! listed with 3dfx's GUID, set to the card's modes, its buffers locked,
//! filled, copied and flipped, and the VGA's picture back after.

use iced_x86::code_asm::*;
use rust_dos::cpu::{Cpu, HostConsole};
use rust_dos::exec::{NoHook, run_batch};
use rust_dos::video::adapter::{Adapter, VideoSetup};
use rust_dos::video::{self, Frame, bios};
use rust_dos::voodoo::Board;
use rust_dos::voodoo::banshee::{
    VID_DESKTOP_OVERLAY_STRIDE, VID_DESKTOP_START_ADDR, VID_PROC_CFG, VIDPROC_HALF_MODE,
};
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

/// A Voodoo3 machine with the built-in Windows on and `program` loaded
/// from the shell as T.EXE.
fn machine(test: &str, program: Vec<u8>) -> Cpu {
    let base = PathBuf::from("target/test_win32_screen").join(test);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join("T.EXE"), program).unwrap();
    let mut cpu = Cpu::new(base);
    bios::install(
        &mut cpu.bus,
        VideoSetup {
            adapter: Adapter::Voodoo3,
            ..Default::default()
        },
    );
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

/// The physical address of the program's `lin`, through its page tables.
fn physical(cpu: &Cpu, lin: u32) -> usize {
    let pde = cpu
        .bus
        .read_32((cpu.cr3 & !0xFFF) as usize + 4 * (lin >> 22) as usize);
    assert!(pde & 1 != 0, "no page table for {lin:X}");
    let pte = cpu
        .bus
        .read_32((pde & !0xFFF) as usize + 4 * ((lin >> 12) & 0x3FF) as usize);
    assert!(pte & 1 != 0, "no page at {lin:X}");
    (pte & !0xFFF) as usize + (lin & 0xFFF) as usize
}

/// The program's dword at `lin`.
fn peek(cpu: &Cpu, lin: u32) -> u32 {
    cpu.bus.read_32(physical(cpu, lin))
}

/// Run the program until DONE in its data is `step`.
fn run_until(cpu: &mut Cpu, data: u32, step: u32) {
    for _ in 0..400 {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        run_batch(cpu, &mut NoHook, false);
        // The program's memory is there while its code runs, with paging.
        let paged = cpu.cr0 & 0x8000_0001 == 0x8000_0001;
        if paged && peek(cpu, data + DONE) == step {
            return;
        }
    }
    panic!(
        "the program didn't get to step {step}: {:?}, at {:X}",
        printed(cpu),
        cpu.eip()
    );
}

/// Let the program waiting for its next step (`wait`) go on.
fn go_on(cpu: &mut Cpu, data: u32) {
    let at = physical(cpu, data + GO);
    let next = cpu.bus.read_32(at) + 1;
    cpu.bus.write_32(at, next);
}

// The data's layout.

const HWND: u32 = 0x00;
const DD: u32 = 0x04;
const FRONT: u32 = 0x08;
const BACK: u32 = 0x0C;
const PALETTE: u32 = 0x10;
const HDC: u32 = 0x14;
const TARGET: u32 = 0x18;
const ZBUF: u32 = 0x1C;
/// The results of the calls, in order.
const RESULTS: u32 = 0x40;
const DONE: u32 = 0xC0;
const GO: u32 = 0xC4;
/// Values the program copied for the test: pointers and pitches.
const SEEN: u32 = 0xD0;
const DESC: u32 = 0x100;
const CAPS: u32 = 0x180;
const LOCKED: u32 = 0x200;
const MODE: u32 = 0x280;
const CLASS: u32 = 0x320;
const ENTRIES: u32 = 0x400;
const DATA_SIZE: usize = 0x1000;

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

/// Copy the dword at data offset `from` to SEEN's slot `slot`.
fn see(a: &mut CodeAssembler, l: &Layout, from: u32, slot: u32) -> Result<(), IcedError> {
    a.mov(eax, dword_ptr(l.at(from)))?;
    a.mov(dword_ptr(l.at(SEEN + 4 * slot)), eax)
}

/// Step `step` done: DONE says so, and the program waits until the test
/// lets it go on (GO counts up to `step`).
fn wait(a: &mut CodeAssembler, l: &Layout, step: u32) -> Result<(), IcedError> {
    a.mov(dword_ptr(l.at(DONE)), step)?;
    let mut again = a.create_label();
    a.set_label(&mut again)?;
    a.cmp(dword_ptr(l.at(GO)), step as i32)?;
    a.jb(again)
}

/// The end of a program: DONE set to `step`, and it spins, its memory
/// there to look at.
fn finish(a: &mut CodeAssembler, l: &Layout, step: u32) -> Result<(), IcedError> {
    a.mov(dword_ptr(l.at(DONE)), step)?;
    let mut again = a.create_label();
    a.set_label(&mut again)?;
    a.jmp(again)
}

/// The results of the first `n` calls, read from the program's data.
fn results(cpu: &Cpu, data: u32, n: u32) -> Vec<u32> {
    (0..n).map(|i| peek(cpu, data + RESULTS + 4 * i)).collect()
}

fn seen(cpu: &Cpu, data: u32, slot: u32) -> u32 {
    peek(cpu, data + SEEN + 4 * slot)
}

// Methods' places in their vtables.
const DD_CREATE_PALETTE: u32 = 5;
const DD_CREATE_SURFACE: u32 = 6;
const DD_GET_DISPLAY_MODE: u32 = 12;
const DD_RESTORE_DISPLAY_MODE: u32 = 19;
const DD_SET_COOPERATIVE_LEVEL: u32 = 20;
const DD_SET_DISPLAY_MODE: u32 = 21;
const S_FLIP: u32 = 11;
const S_GET_ATTACHED_SURFACE: u32 = 12;
const S_LOCK: u32 = 25;
const S_SET_PALETTE: u32 = 31;
const S_UNLOCK: u32 = 32;

const DDSD_CAPS: u32 = 0x1;
const DDSD_HEIGHT: u32 = 0x2;
const DDSD_WIDTH: u32 = 0x4;
const DDSD_BACKBUFFERCOUNT: u32 = 0x20;
const DDSD_ZBUFFERBITDEPTH: u32 = 0x40;
const DDSD_PIXELFORMAT: u32 = 0x1000;
const DDSCAPS_BACKBUFFER: u32 = 0x4;
const DDSCAPS_COMPLEX: u32 = 0x8;
const DDSCAPS_FLIP: u32 = 0x10;
const DDSCAPS_OFFSCREENPLAIN: u32 = 0x40;
const DDSCAPS_PRIMARYSURFACE: u32 = 0x200;
const DDSCAPS_3DDEVICE: u32 = 0x2000;
const DDSCAPS_VIDEOMEMORY: u32 = 0x4000;
const DDSCAPS_TEXTURE: u32 = 0x1000;
const DDSCAPS_LOCALVIDMEM: u32 = 0x1000_0000;
const DDSCAPS_ZBUFFER: u32 = 0x2_0000;
const DDSCL_FULLSCREEN: u32 = 0x1;
const DDSCL_NORMAL: u32 = 0x8;
const DDSCL_EXCLUSIVE: u32 = 0x10;
const DDPCAPS_8BIT: u32 = 0x4;
const DDPCAPS_ALLOW256: u32 = 0x40;
const WS_POPUP: u32 = 0x8000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const BITSPIXEL: u32 = 12;

/// A DDSURFACEDESC at `offset` with `flags`, `caps` and the rest given.
fn desc(data: &mut [u8], offset: u32, flags: u32, caps: u32, w: u32, h: u32, backbuffers: u32) {
    put32(data, offset, 108);
    put32(data, offset + 0x04, flags);
    put32(data, offset + 0x08, h);
    put32(data, offset + 0x0C, w);
    put32(data, offset + 0x14, backbuffers);
    put32(data, offset + 0x68, caps);
}

const USER32: (&str, &[&str]) = ("USER32.DLL", &["CreateWindowExA", "GetDC"]);
const GDI32: (&str, &[&str]) = ("GDI32.DLL", &["GetDeviceCaps", "SetPixel", "GetPixel"]);
const DDRAW: (&str, &[&str]) = ("DDRAW.DLL", &["DirectDrawCreate"]);
const CREATE_WINDOW: usize = 0;
const GET_DC: usize = 1;
const GET_DEVICE_CAPS: usize = 2;
const SET_PIXEL: usize = 3;
const GET_PIXEL: usize = 4;
const DIRECT_DRAW_CREATE: usize = 5;

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

/// The card's pixel at (`x`, `y`) as the emulator's display shows it.
fn shown(cpu: &mut Cpu, x: usize, y: usize) -> [u8; 3] {
    let (width, height) = video::frame_size(&cpu.bus);
    let mut frame = Frame::new(width, height);
    video::render_screen(&mut frame, &cpu.bus);
    let at = (y * width as usize + x) * 4;
    [frame.rgba[at], frame.rgba[at + 1], frame.rgba[at + 2]]
}

fn card_reg(cpu: &Cpu, offset: u32) -> u32 {
    cpu.bus
        .voodoo
        .as_ref()
        .unwrap()
        .banshee()
        .unwrap()
        .reg(offset)
}

/// A word of the card's memory at byte `addr`.
fn card_word(cpu: &Cpu, addr: u32) -> u16 {
    cpu.bus
        .voodoo
        .as_ref()
        .unwrap()
        .frame_buffer()
        .get(addr as usize / 2)
}

/// The card's desktop size, pixel format (vidProcCfg bits 18-20) and
/// stride.
fn card_mode(cpu: &Cpu) -> (u32, u32, u32, u32) {
    let b = cpu.bus.voodoo.as_ref().unwrap().banshee().unwrap();
    assert!(b.extended(), "the card shows its own mode");
    let (w, h) = b.desktop_size();
    let cfg = b.reg(VID_PROC_CFG);
    let h = if cfg & VIDPROC_HALF_MODE != 0 {
        h / 2
    } else {
        h
    };
    (
        w,
        h,
        (cfg >> 18) & 7,
        b.reg(VID_DESKTOP_OVERLAY_STRIDE) & 0x3FFF,
    )
}

/// On the desktop: GDI's depth, a pixel set through GDI and read back,
/// and DirectDraw's display mode.
fn desktop_program() -> (Vec<u8>, u32) {
    let mut d = data();
    put32(&mut d, MODE, 108);
    program(&[USER32, GDI32, DDRAW], &d, |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c, (0, 0, 64, 64))?;
        c.api(a, l, GET_DC, &[Arg::I(0)])?;
        a.mov(dword_ptr(l.at(HDC)), eax)?;
        c.api(a, l, GET_DEVICE_CAPS, &[Arg::Val(HDC), Arg::I(BITSPIXEL)])?;
        // Red at (300, 200) and a gray at (301, 200).
        c.api(
            a,
            l,
            SET_PIXEL,
            &[Arg::Val(HDC), Arg::I(300), Arg::I(200), Arg::I(0x0000_00FF)],
        )?;
        c.api(
            a,
            l,
            SET_PIXEL,
            &[Arg::Val(HDC), Arg::I(301), Arg::I(200), Arg::I(0x0012_3456)],
        )?;
        c.api(a, l, GET_PIXEL, &[Arg::Val(HDC), Arg::I(301), Arg::I(200)])?;
        c.com(a, l, DD, DD_GET_DISPLAY_MODE, &[Arg::At(MODE)])?;
        finish(a, l, 1)
    })
}

#[test]
fn the_desktop_is_16_bits_in_the_cards_memory() {
    let (program, data) = desktop_program();
    let mut cpu = machine("desktop", program);
    run_until(&mut cpu, data, 1);
    let r = results(&cpu, data, 8);
    assert_eq!(r[1], 0, "DirectDrawCreate");
    assert_eq!(r[3], 16, "GetDeviceCaps(BITSPIXEL)");
    assert_eq!(r[4], 0x0000_00FF, "SetPixel");
    // The screen has the color in 5-6-5, which reads back widened.
    assert_eq!(r[6], 0x0010_3452, "GetPixel");
    assert_eq!(r[7], 0, "GetDisplayMode");
    // The card's mode: 640x480, 5-6-5, 1280 bytes a row, from 0.
    assert_eq!(card_mode(&cpu), (640, 480, 1, 1280));
    assert_eq!(card_reg(&cpu, VID_DESKTOP_START_ADDR), 0);
    assert_eq!(card_word(&cpu, 200 * 1280 + 300 * 2), 0xF800);
    assert_eq!(card_word(&cpu, 200 * 1280 + 301 * 2), 0x51A2);
    assert_eq!(shown(&mut cpu, 300, 200), [0xFF, 0, 0]);
    // DirectDraw's mode is the desktop's: 16 bits, 5-6-5.
    let mode = |o: u32| peek(&cpu, data + MODE + o);
    assert_eq!((mode(0x0C), mode(0x08), mode(0x10)), (640, 480, 1280));
    assert_eq!(mode(0x48 + 12), 16);
    assert_eq!(
        (mode(0x48 + 16), mode(0x48 + 20), mode(0x48 + 24)),
        (0xF800, 0x07E0, 0x001F)
    );
}

/// Full screen at 640x480x16, after the desktop showed (step 1): a flip
/// chain, a pixel written to the back buffer through Lock and a flip
/// that shows it (step 2); then the mode given back (step 3).
fn fullscreen_program() -> (Vec<u8>, u32) {
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
    program(&[USER32, GDI32, DDRAW], &d, |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c, (0, 0, 320, 240))?;
        wait(a, l, 1)?;
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
            &[Arg::I(640), Arg::I(480), Arg::I(16)],
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
        see(a, l, LOCKED + 0x24, 0)?;
        see(a, l, LOCKED + 0x10, 1)?;
        // Red and green at the start of the first row, blue on the
        // second.
        a.mov(edi, dword_ptr(l.at(LOCKED + 0x24)))?;
        a.mov(dword_ptr(edi), 0x07E0_F800)?;
        a.add(edi, dword_ptr(l.at(LOCKED + 0x10)))?;
        a.mov(word_ptr(edi), 0x001F)?;
        c.com(a, l, BACK, S_UNLOCK, &[Arg::I(0)])?;
        c.com(a, l, FRONT, S_FLIP, &[Arg::I(0), Arg::I(1)])?;
        wait(a, l, 2)?;
        c.com(a, l, DD, DD_RESTORE_DISPLAY_MODE, &[])?;
        c.com(
            a,
            l,
            DD,
            DD_SET_COOPERATIVE_LEVEL,
            &[Arg::Val(HWND), Arg::I(DDSCL_NORMAL)],
        )?;
        finish(a, l, 3)
    })
}

#[test]
fn a_full_screen_mode_flips_in_the_cards_memory_and_the_desktop_comes_back() {
    let (program, data) = fullscreen_program();
    let mut cpu = machine("fullscreen", program);
    run_until(&mut cpu, data, 1);
    // The desktop, with the window at its top left.
    let desktop = |cpu: &Cpu| {
        let b = cpu.bus.voodoo.as_ref().unwrap().frame_buffer();
        (0..640 * 480).map(|i| b.get(i)).collect::<Vec<_>>()
    };
    let before = desktop(&cpu);
    assert_eq!(card_mode(&cpu), (640, 480, 1, 1280));
    go_on(&mut cpu, data);
    run_until(&mut cpu, data, 2);
    assert_eq!(results(&cpu, data, 9)[1..], [0; 8]);
    assert_eq!(card_mode(&cpu), (640, 480, 1, 1280));
    // The back buffer was after the screen's 614400 bytes; the flip shows
    // it, and its pixels are the program's.
    let start = card_reg(&cpu, VID_DESKTOP_START_ADDR);
    assert_eq!(start, 640 * 480 * 2);
    assert_eq!(seen(&cpu, data, 1), 1280);
    assert_eq!(card_word(&cpu, start), 0xF800);
    assert_eq!(card_word(&cpu, start + 2), 0x07E0);
    assert_eq!(card_word(&cpu, start + 1280), 0x001F);
    assert_eq!(shown(&mut cpu, 0, 0), [0xFF, 0, 0]);
    assert_eq!(shown(&mut cpu, 1, 0), [0, 0xFF, 0]);
    assert_eq!(shown(&mut cpu, 0, 1), [0, 0, 0xFF]);
    // The pointer Lock gave is in the program's view of the card's
    // memory, which is its BAR1.
    let pointer = seen(&cpu, data, 0);
    let bar1 = cpu.bus.voodoo.as_ref().unwrap().banshee().unwrap().pci.bar1;
    assert_eq!(physical(&cpu, pointer) as u32, bar1 + start);

    go_on(&mut cpu, data);
    run_until(&mut cpu, data, 3);
    assert_eq!(results(&cpu, data, 11)[9..], [0; 2]);
    // The desktop's mode again, from the start of memory, repainted.
    // The window, which the mode changes made cover the screen, as
    // DirectDraw does on Windows 98, is over all of it: its corner with
    // its text as before, and its background where the desktop's was.
    assert_eq!(card_mode(&cpu), (640, 480, 1, 1280));
    assert_eq!(card_reg(&cpu, VID_DESKTOP_START_ADDR), 0);
    let after = desktop(&cpu);
    let at = |x: usize, y: usize| y * 640 + x;
    assert!(after[..at(320, 0)] == before[..at(320, 0)], "its top");
    assert!(before[at(639, 479)] != before[at(319, 239)]);
    assert!(after[at(639, 479)] == before[at(319, 239)], "covered");
}

/// Other modes: 320x200 at 8 bits with a palette, then 800x600 at 32.
fn modes_program() -> (Vec<u8>, u32) {
    let mut d = data();
    desc(&mut d, DESC, DDSD_CAPS, DDSCAPS_PRIMARYSURFACE, 0, 0, 0);
    for i in 0..256u32 {
        put(&mut d, ENTRIES + 4 * i, &[i as u8, 0x40, 255 - i as u8, 0]);
    }
    put32(&mut d, LOCKED, 108);
    program(&[USER32, GDI32, DDRAW], &d, |a, l| {
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
            FRONT,
            S_LOCK,
            &[Arg::I(0), Arg::At(LOCKED), Arg::I(1), Arg::I(0)],
        )?;
        a.mov(edi, dword_ptr(l.at(LOCKED + 0x24)))?;
        a.mov(byte_ptr(edi + 5), 7)?;
        c.com(a, l, FRONT, S_UNLOCK, &[Arg::I(0)])?;
        wait(a, l, 1)?;
        c.com(
            a,
            l,
            DD,
            DD_SET_DISPLAY_MODE,
            &[Arg::I(800), Arg::I(600), Arg::I(32)],
        )?;
        finish(a, l, 2)
    })
}

#[test]
fn eight_and_32_bit_modes_are_the_cards() {
    let (program, data) = modes_program();
    let mut cpu = machine("modes", program);
    run_until(&mut cpu, data, 1);
    assert_eq!(results(&cpu, data, 9)[1..], [0; 8]);
    // 320x200 shown with its lines doubled, 8 bits through the CLUT.
    assert_eq!(card_mode(&cpu), (320, 200, 0, 320));
    assert_ne!(card_reg(&cpu, VID_PROC_CFG) & VIDPROC_HALF_MODE, 0);
    let clut = cpu.bus.voodoo.as_ref().unwrap().banshee().unwrap().clut;
    assert_eq!(clut[7], 0x07_40F8);
    assert_eq!(clut[200], 0xC8_4037);
    assert_eq!(shown(&mut cpu, 5, 0), [7, 0x40, 0xF8]);
    assert_eq!(shown(&mut cpu, 5, 1), [7, 0x40, 0xF8]);
    let timing = cpu.bus.vga.timing();
    assert_eq!(timing.display, 400);
    assert!((timing.hz() - 70.0).abs() < 1.0, "{}", timing.hz());

    go_on(&mut cpu, data);
    run_until(&mut cpu, data, 2);
    assert_eq!(results(&cpu, data, 10)[9], 0);
    assert_eq!(card_mode(&cpu), (800, 600, 3, 3200));
    let timing = cpu.bus.vga.timing();
    assert_eq!(timing.display, 600);
    assert!((timing.hz() - 60.3).abs() < 1.0, "{}", timing.hz());
}

/// A 16-bit render target and a 16-bit z-buffer in video memory, each
/// locked and written.
fn buffers_program() -> (Vec<u8>, u32) {
    let mut d = data();
    desc(
        &mut d,
        DESC,
        DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT | DDSD_PIXELFORMAT,
        DDSCAPS_OFFSCREENPLAIN | DDSCAPS_3DDEVICE | DDSCAPS_VIDEOMEMORY,
        256,
        128,
        0,
    );
    let pf = DESC + 0x48;
    for (i, v) in [32, 0x40, 0, 16, 0xF800, 0x07E0, 0x001F].iter().enumerate() {
        put32(&mut d, pf + 4 * i as u32, *v);
    }
    desc(
        &mut d,
        CAPS,
        DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT | DDSD_ZBUFFERBITDEPTH,
        DDSCAPS_ZBUFFER | DDSCAPS_VIDEOMEMORY,
        256,
        128,
        0,
    );
    put32(&mut d, CAPS + 0x18, 16);
    put32(&mut d, LOCKED, 108);
    put32(&mut d, MODE, 108);
    program(&[USER32, GDI32, DDRAW], &d, |a, l| {
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
            &[Arg::At(DESC), Arg::At(TARGET), Arg::I(0)],
        )?;
        c.com(
            a,
            l,
            DD,
            DD_CREATE_SURFACE,
            &[Arg::At(CAPS), Arg::At(ZBUF), Arg::I(0)],
        )?;
        for (surface, desc_at, slot, value) in
            [(TARGET, LOCKED, 0, 0x1234u32), (ZBUF, MODE, 2, 0xFFFF)]
        {
            c.com(
                a,
                l,
                surface,
                S_LOCK,
                &[Arg::I(0), Arg::At(desc_at), Arg::I(1), Arg::I(0)],
            )?;
            see(a, l, desc_at + 0x24, slot)?;
            see(a, l, desc_at + 0x10, slot + 1)?;
            a.mov(edi, dword_ptr(l.at(desc_at + 0x24)))?;
            a.mov(word_ptr(edi + 2), value)?;
            c.com(a, l, surface, S_UNLOCK, &[Arg::I(0)])?;
        }
        finish(a, l, 1)
    })
}

#[test]
fn render_targets_and_z_buffers_are_in_the_cards_memory() {
    let (program, data) = buffers_program();
    let mut cpu = machine("buffers", program);
    run_until(&mut cpu, data, 1);
    assert_eq!(results(&cpu, data, 9)[1..], [0; 8]);
    let bar1 = cpu.bus.voodoo.as_ref().unwrap().banshee().unwrap().pci.bar1;
    let mut offsets = Vec::new();
    for (slot, value) in [(0, 0x1234), (2, 0xFFFF)] {
        let pointer = seen(&cpu, data, slot);
        assert_eq!(seen(&cpu, data, slot + 1), 512, "the pitch");
        let offset = physical(&cpu, pointer) as u32 - bar1;
        // After the desktop, aligned for the 3D engine.
        assert!(offset >= 640 * 480 * 2, "{offset:X}");
        assert_eq!(offset % 16, 0);
        assert_eq!(card_word(&cpu, offset + 2), value);
        offsets.push(offset);
    }
    assert!(offsets[1] >= offsets[0] + 512 * 128);
}

const S_SET_COLOR_KEY: u32 = 29;
const DDCKEY_SRCBLT: u32 = 8;
const DDBLT_KEYDESTOVERRIDE: u32 = 0x4000;
const DDBLT_KEYSRC: u32 = 0x8000;
/// The destination rectangles of the blits, 16 bytes each.
const BLT_RECTS: u32 = 0x800;
/// The source's color key.
const KEY: u32 = 0x880;
/// The source's middle two columns, a rectangle not at its corner.
const SRC_RECT: u32 = 0x890;
/// The source's key color, magenta.
const MAGENTA: u16 = 0xF81F;
/// The 4x2 source's pixels, row by row: the key and colors.
const SOURCE: [[u16; 4]; 2] = [
    [MAGENTA, 0x1234, MAGENTA, 0x5678],
    [0x9ABC, MAGENTA, 0xDEF0, MAGENTA],
];
/// The color the back buffer is filled with first.
const FILL: u16 = 0x0841;
/// The boxes of the back buffer the test looks at, (x, y, w, h).
const LOOKED_AT: [(u32, u32, u32, u32); 5] = [
    (100, 49, 6, 4),
    (200, 10, 4, 2),
    (299, 100, 10, 4),
    (400, 20, 2, 2),
    (500, 20, 4, 4),
];
/// Where the program copies those boxes' pixels, one box after another,
/// row by row.
const READBACK: u32 = 0xA00;

/// Full screen at 640x480x16 with a back buffer, which is filled, then
/// gets a 16-bit system memory surface blitted onto it: through the
/// source's color key at (101, 50), copied as it is at (200, 10),
/// stretched twice as wide and high through the key at (300, 100), its
/// middle two columns through the key at (400, 20) and stretched twice
/// as wide and high at (500, 20), and at (101, 50) again through a
/// destination key of the fill's color. The back buffer is then locked
/// and LOOKED_AT's boxes copied to READBACK. With `card`, the DirectDraw
/// object is the 3dfx device's, of a Voodoo Graphics or Voodoo2.
fn keyed_blit_program(card: bool) -> (Vec<u8>, u32) {
    let mut d = data();
    put32(&mut d, LOCKED, 108);
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
    desc(
        &mut d,
        SYSDESC,
        DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT | DDSD_PIXELFORMAT,
        DDSCAPS_OFFSCREENPLAIN | DDSCAPS_SYSTEMMEMORY,
        4,
        2,
        0,
    );
    let pf = SYSDESC + 0x48;
    for (i, v) in [32, 0x40, 0, 16, 0xF800, 0x07E0, 0x001F].iter().enumerate() {
        put32(&mut d, pf + 4 * i as u32, *v);
    }
    put32(&mut d, LOCK2, 108);
    put32(&mut d, FX, 100);
    put32(&mut d, FX + 0x50, FILL as u32);
    // The destination key: the fill's color.
    put32(&mut d, FX + 0x54, FILL as u32);
    put32(&mut d, FX + 0x58, FILL as u32);
    put32(&mut d, KEY, MAGENTA as u32);
    put32(&mut d, KEY + 4, MAGENTA as u32);
    let rects = [
        [101, 50, 105, 52],
        [200, 10, 204, 12],
        [300, 100, 308, 104],
        [400, 20, 402, 22],
        [500, 20, 504, 24],
    ];
    for (i, r) in rects.iter().enumerate() {
        for (j, v) in r.iter().enumerate() {
            put32(&mut d, BLT_RECTS + 16 * i as u32 + 4 * j as u32, *v);
        }
    }
    for (j, v) in [1, 0, 3, 2].iter().enumerate() {
        put32(&mut d, SRC_RECT + 4 * j as u32, *v);
    }
    let ddraw = if card { DDRAW_CARD } else { DDRAW };
    program(&[USER32, GDI32, ddraw], &d, |a, l| {
        let mut c = Calls { next: 0 };
        if card {
            device_callback(a, l)?;
        }
        start(a, l, &mut c, (0, 0, 64, 64))?;
        if card {
            // The 3dfx device's object in DD's place.
            c.api(
                a,
                l,
                DIRECT_DRAW_ENUMERATE,
                &[Arg::Val(CALLBACKS), Arg::I(0)],
            )?;
            c.api(
                a,
                l,
                DIRECT_DRAW_CREATE,
                &[Arg::At(DEVICE_GUID), Arg::At(DD), Arg::I(0)],
            )?;
        }
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
            &[Arg::I(640), Arg::I(480), Arg::I(16)],
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
            DD,
            DD_CREATE_SURFACE,
            &[Arg::At(SYSDESC), Arg::At(SYS), Arg::I(0)],
        )?;
        c.com(
            a,
            l,
            SYS,
            S_LOCK,
            &[Arg::I(0), Arg::At(LOCK2), Arg::I(1), Arg::I(0)],
        )?;
        a.mov(edi, dword_ptr(l.at(LOCK2 + 0x24)))?;
        for row in SOURCE {
            for (i, &pixel) in row.iter().enumerate() {
                a.mov(word_ptr(edi + 2 * i as i32), pixel as u32)?;
            }
            a.add(edi, dword_ptr(l.at(LOCK2 + 0x10)))?;
        }
        c.com(a, l, SYS, S_UNLOCK, &[Arg::I(0)])?;
        c.com(
            a,
            l,
            SYS,
            S_SET_COLOR_KEY,
            &[Arg::I(DDCKEY_SRCBLT), Arg::At(KEY)],
        )?;
        c.com(
            a,
            l,
            BACK,
            S_BLT,
            &[
                Arg::I(0),
                Arg::I(0),
                Arg::I(0),
                Arg::I(DDBLT_COLORFILL | DDBLT_WAIT),
                Arg::At(FX),
            ],
        )?;
        let blits = [
            (DDBLT_KEYSRC, Arg::I(0)),
            (0, Arg::I(0)),
            (DDBLT_KEYSRC, Arg::I(0)),
            (DDBLT_KEYSRC, Arg::At(SRC_RECT)),
            (0, Arg::At(SRC_RECT)),
        ];
        for (i, (flags, src_rect)) in blits.into_iter().enumerate() {
            c.com(
                a,
                l,
                BACK,
                S_BLT,
                &[
                    Arg::At(BLT_RECTS + 16 * i as u32),
                    Arg::Val(SYS),
                    src_rect,
                    Arg::I(flags | DDBLT_WAIT),
                    Arg::I(0),
                ],
            )?;
        }
        c.com(
            a,
            l,
            BACK,
            S_BLT,
            &[
                Arg::At(BLT_RECTS),
                Arg::Val(SYS),
                Arg::I(0),
                Arg::I(DDBLT_KEYDESTOVERRIDE | DDBLT_WAIT),
                Arg::At(FX),
            ],
        )?;
        c.com(
            a,
            l,
            BACK,
            S_LOCK,
            &[Arg::I(0), Arg::At(LOCKED), Arg::I(1), Arg::I(0)],
        )?;
        a.mov(edx, dword_ptr(l.at(LOCKED + 0x24)))?;
        a.mov(ebx, dword_ptr(l.at(LOCKED + 0x10)))?;
        a.cld()?;
        let mut to = READBACK;
        for (x, y, w, h) in LOOKED_AT {
            for row in y..y + h {
                a.imul_3(eax, ebx, row as i32)?;
                a.lea(esi, edx + eax + 2 * x as i32)?;
                a.mov(edi, l.at(to) as u32)?;
                a.mov(ecx, w)?;
                a.rep().movsw()?;
                to += 2 * w;
            }
        }
        c.com(a, l, BACK, S_UNLOCK, &[Arg::I(0)])?;
        finish(a, l, 1)
    })
}

/// The back buffer's pixel at (`x`, `y`) as the keyed blits' program
/// copied it, from one of LOOKED_AT's boxes.
fn looked_at(cpu: &Cpu, data: u32, x: u32, y: u32) -> u16 {
    let mut at = READBACK;
    for (left, top, w, h) in LOOKED_AT {
        if (left..left + w).contains(&x) && (top..top + h).contains(&y) {
            return peek(cpu, data + at + 2 * ((y - top) * w + x - left)) as u16;
        }
        at += 2 * w * h;
    }
    panic!("({x}, {y}) isn't in a box the program copied");
}

/// The keyed blits' program run on `cpu` to its end, its `calls` calls
/// all fine, and the pixels the color keys say kept and only those.
fn keyed_blits_keep_what_the_keys_say(cpu: &mut Cpu, data: u32, calls: u32) {
    run_until(cpu, data, 1);
    let r = results(cpu, data, calls);
    assert!(r[1..].iter().all(|&v| v == 0), "{r:X?}");
    let cpu = &*cpu;
    let pixel = |cpu: &Cpu, x: u32, y: u32| looked_at(cpu, data, x, y);
    let rows = |cpu: &Cpu, x: u32, y: u32, w: u32, h: u32| {
        (y..y + h)
            .map(|y| (x..x + w).map(|x| pixel(cpu, x, y)).collect::<Vec<_>>())
            .collect::<Vec<_>>()
    };
    let [[k, a, _, b], [c, _, e, _]] = SOURCE;
    // Through the source's key, then the key's pixels through the
    // destination's: the source as it is, framed by the fill.
    assert_eq!(
        rows(cpu, 100, 49, 6, 4),
        [
            [FILL; 6].to_vec(),
            vec![FILL, k, a, k, b, FILL],
            vec![FILL, c, k, e, k, FILL],
            [FILL; 6].to_vec(),
        ]
    );
    // Copied as it is, the key's pixels too.
    assert_eq!(rows(cpu, 200, 10, 4, 2), SOURCE.map(|r| r.to_vec()));
    // Each pixel twice across and down; the fill where the key was.
    let stretched: Vec<Vec<u16>> = SOURCE
        .iter()
        .flat_map(|row| {
            let wide: Vec<u16> = row
                .iter()
                .flat_map(|&p| [if p == MAGENTA { FILL } else { p }; 2])
                .collect();
            [wide.clone(), wide]
        })
        .collect();
    assert_eq!(rows(cpu, 300, 100, 8, 4), stretched);
    assert_eq!(pixel(cpu, 299, 100), FILL);
    assert_eq!(pixel(cpu, 308, 103), FILL);
    // The middle columns, from the source's second on: through the key,
    // and stretched with the key's pixels copied.
    assert_eq!(rows(cpu, 400, 20, 2, 2), [[a, FILL], [FILL, e]]);
    assert_eq!(
        rows(cpu, 500, 20, 4, 4),
        [[a, a, k, k], [a, a, k, k], [k, k, e, e], [k, k, e, e]]
    );
}

/// Blits from system memory into the card's memory, a 16-bit back
/// buffer, keep the pixels the color keys say and only those.
#[test]
fn keyed_blits_into_the_cards_memory_keep_what_the_keys_say() {
    let (program, data) = keyed_blit_program(false);
    let mut cpu = machine("keyed_blit", program);
    keyed_blits_keep_what_the_keys_say(&mut cpu, data, 19);
}

/// The same blits into the back buffer of a Voodoo Graphics' 3dfx
/// device, whose pixels go through the card's linear frame buffer, rows
/// starting at odd columns among them.
#[test]
fn keyed_blits_on_a_voodoo_graphics_keep_what_the_keys_say() {
    let (program, data) = keyed_blit_program(true);
    let mut cpu = card_machine("keyed_blit_v1", Some(Board::Standard), program);
    keyed_blits_keep_what_the_keys_say(&mut cpu, data, 21);
}

// The 3dfx device of a Voodoo Graphics or Voodoo2 add-on card.

/// A machine with its usual display adapter, the add-on card `board`
/// (or none), the built-in Windows on and `program` loaded as T.EXE.
fn card_machine(test: &str, board: Option<Board>, program: Vec<u8>) -> Cpu {
    let base = PathBuf::from("target/test_win32_screen").join(test);
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

const DDRAW_CARD: (&str, &[&str]) = ("DDRAW.DLL", &["DirectDrawCreate", "DirectDrawEnumerateA"]);
const DIRECT_DRAW_ENUMERATE: usize = 6;

// More of the data's layout.
const SYS: u32 = 0x20;
const DD2: u32 = 0x24;
/// The devices' count, then the 3dfx device's GUID, description and
/// driver name as DirectDrawEnumerate gave them.
const DEVICES: u32 = 0x400;
const DEVICE_GUID: u32 = 0x404;
const DEVICE_DESCRIPTION: u32 = 0x420;
const DEVICE_NAME: u32 = 0x440;
/// The callbacks' addresses.
const CALLBACKS: u32 = 0x460;
/// The modes' count, widths and heights.
const MODES: u32 = 0x500;
const ZDESC: u32 = 0x500;
const FX: u32 = 0x600;
const RECT_FILL: u32 = 0x680;
const RECT_SRC: u32 = 0x690;
const SYSDESC: u32 = 0x700;
const LOCK2: u32 = 0x780;

const DD_ENUM_DISPLAY_MODES: u32 = 8;
const DD_GET_MONITOR_FREQUENCY: u32 = 15;
const DD_GET_SCAN_LINE: u32 = 16;
const DDERR_VERTICALBLANKINPROGRESS: u32 = 0x8876_0219;
const S_BLT: u32 = 5;
const S_BLT_FAST: u32 = 7;
const S_IS_LOST: u32 = 24;
const S_GET_SURFACE_DESC: u32 = 22;
const DDSCAPS_SYSTEMMEMORY: u32 = 0x800;
const DDBLT_COLORFILL: u32 = 0x400;
const DDBLT_WAIT: u32 = 0x100_0000;
const DDFLIP_WAIT: u32 = 1;
const DDERR_SURFACELOST: u32 = 0x8876_01C2;

/// A callback's code, whose address goes in CALLBACKS' slot `slot`:
/// `body` makes it.
fn callback(
    a: &mut CodeAssembler,
    l: &Layout,
    slot: u32,
    body: impl FnOnce(&mut CodeAssembler) -> Result<(), IcedError>,
) -> Result<(), IcedError> {
    let mut over = a.create_label();
    a.call(over)?;
    body(a)?;
    a.set_label(&mut over)?;
    a.pop(eax)?;
    a.mov(dword_ptr(l.at(CALLBACKS + 4 * slot)), eax)
}

/// DirectDrawEnumerateA's callback (guid, description, name, context):
/// it counts the devices and keeps the GUID and the first bytes of the
/// strings of one with a GUID.
fn device_callback(a: &mut CodeAssembler, l: &Layout) -> Result<(), IcedError> {
    callback(a, l, 0, |a| {
        a.add(dword_ptr(l.at(DEVICES)), 1)?;
        let mut done = a.create_label();
        a.mov(eax, dword_ptr(esp + 4))?;
        a.test(eax, eax)?;
        a.jz(done)?;
        for (arg, to, dwords) in [
            (4, DEVICE_GUID, 4),
            (8, DEVICE_DESCRIPTION, 8),
            (12, DEVICE_NAME, 4),
        ] {
            a.mov(eax, dword_ptr(esp + arg))?;
            for i in 0..dwords {
                a.mov(ecx, dword_ptr(eax + 4 * i))?;
                a.mov(dword_ptr(l.at(to + 4 * i as u32)), ecx)?;
            }
        }
        a.set_label(&mut done)?;
        a.mov(eax, 1)?;
        a.ret_1(16)
    })
}

/// The program that lists the devices, makes the 3dfx device's
/// DirectDraw object and asks for its modes and its mode, and then the
/// display's object.
fn devices_program() -> (Vec<u8>, u32) {
    let mut d = data();
    put32(&mut d, MODE, 108);
    put32(&mut d, LOCKED, 108);
    program(&[USER32, GDI32, DDRAW_CARD], &d, |a, l| {
        let mut c = Calls { next: 0 };
        device_callback(a, l)?;
        // EnumDisplayModes' callback (desc, context): widths and heights.
        callback(a, l, 1, |a| {
            a.mov(ecx, dword_ptr(l.at(MODES)))?;
            a.mov(edx, dword_ptr(esp + 4))?;
            a.mov(eax, dword_ptr(edx + 0x0C))?;
            a.mov(dword_ptr(ecx * 4 + l.at(MODES + 4) as i32), eax)?;
            a.mov(eax, dword_ptr(edx + 0x08))?;
            a.mov(dword_ptr(ecx * 4 + l.at(MODES + 0x84) as i32), eax)?;
            a.add(dword_ptr(l.at(MODES)), 1)?;
            a.mov(eax, 1)?;
            a.ret_1(8)
        })?;
        c.api(
            a,
            l,
            DIRECT_DRAW_ENUMERATE,
            &[Arg::Val(CALLBACKS), Arg::I(0)],
        )?;
        c.api(
            a,
            l,
            DIRECT_DRAW_CREATE,
            &[Arg::At(DEVICE_GUID), Arg::At(DD), Arg::I(0)],
        )?;
        let mut display = a.create_label();
        a.test(eax, eax)?;
        a.jnz(display)?;
        c.com(
            a,
            l,
            DD,
            DD_ENUM_DISPLAY_MODES,
            &[Arg::I(0), Arg::I(0), Arg::I(0), Arg::Val(CALLBACKS + 4)],
        )?;
        c.com(a, l, DD, DD_GET_DISPLAY_MODE, &[Arg::At(MODE)])?;
        a.set_label(&mut display)?;
        c.api(
            a,
            l,
            DIRECT_DRAW_CREATE,
            &[Arg::I(0), Arg::At(DD2), Arg::I(0)],
        )?;
        c.com(
            a,
            l,
            DD2,
            DD_SET_COOPERATIVE_LEVEL,
            &[Arg::I(0), Arg::I(DDSCL_NORMAL)],
        )?;
        c.com(a, l, DD2, DD_GET_DISPLAY_MODE, &[Arg::At(LOCKED)])?;
        finish(a, l, 1)
    })
}

/// The string at `offset` of the program's data, up to its NUL or
/// `len` bytes.
fn text(cpu: &Cpu, data: u32, offset: u32, len: u32) -> String {
    let bytes: Vec<u8> = (0..len)
        .map(|i| peek(cpu, data + offset + i) as u8)
        .take_while(|&b| b != 0)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The program's 16 bytes at `offset`.
fn guid(cpu: &Cpu, data: u32, offset: u32) -> Vec<u8> {
    (0..16)
        .map(|i| peek(cpu, data + offset + i) as u8)
        .collect()
}

fn devices(test: &str, board: Board) -> (Cpu, u32) {
    let (program, data) = devices_program();
    let mut cpu = card_machine(test, Some(board), program);
    run_until(&mut cpu, data, 1);
    assert_eq!(peek(&cpu, data + DEVICES), 2, "the display and the card");
    assert_eq!(results(&cpu, data, 7)[1..], [0; 6]);
    // The card's mode before one is set, and the display's own.
    assert_eq!(peek(&cpu, data + MODE + 0x0C), 640);
    assert_eq!(peek(&cpu, data + MODE + 0x10), 2048);
    assert_eq!(peek(&cpu, data + MODE + 0x48 + 12), 16);
    assert_ne!(peek(&cpu, data + LOCKED + 0x0C), 0);
    assert_eq!(fbi_init0(&cpu) & 1, 0, "the VGA has the monitor");
    (cpu, data)
}

/// The modes EnumDisplayModes listed.
fn modes_listed(cpu: &Cpu, data: u32) -> Vec<(u32, u32)> {
    let n = peek(cpu, data + MODES);
    (0..n)
        .map(|i| {
            (
                peek(cpu, data + MODES + 4 + 4 * i),
                peek(cpu, data + MODES + 0x84 + 4 * i),
            )
        })
        .collect()
}

/// The card's fbiInit0: bit 0 takes the monitor from the VGA.
fn fbi_init0(cpu: &Cpu) -> u32 {
    cpu.bus.voodoo.as_ref().unwrap().reg[0x210 / 4]
}

#[test]
fn a_voodoo_graphics_is_a_second_directdraw_device() {
    let (cpu, data) = devices("devices_v1", Board::Standard);
    assert_eq!(
        guid(&cpu, data, DEVICE_GUID),
        [
            0x01, 0xFD, 0x0C, 0x3A, 0x20, 0x93, 0xCF, 0x11, 0xAC, 0xA1, 0x00, 0xA0, 0x24, 0x13,
            0xC2, 0xE2
        ]
    );
    assert_eq!(
        text(&cpu, data, DEVICE_DESCRIPTION, 32),
        "3dfx DirectX Driver"
    );
    assert_eq!(text(&cpu, data, DEVICE_NAME, 16), "mm3dfx");
    // Two buffers of 800 x 600 fit in its 2 MB.
    assert_eq!(
        modes_listed(&cpu, data),
        [(512, 384), (640, 400), (640, 480), (800, 600)]
    );
}

#[test]
fn a_voodoo2_is_a_second_directdraw_device() {
    let (cpu, data) = devices("devices_v2", Board::Voodoo2);
    assert_eq!(
        guid(&cpu, data, DEVICE_GUID),
        [
            0x00, 0xEA, 0x2B, 0x47, 0xDF, 0x40, 0xD1, 0x11, 0xA9, 0xDF, 0x00, 0x60, 0x97, 0xC2,
            0xED, 0xB2
        ]
    );
    assert_eq!(
        text(&cpu, data, DEVICE_DESCRIPTION, 32),
        "Voodoo2 DirectX 6 Driver"
    );
    assert_eq!(text(&cpu, data, DEVICE_NAME, 16), "3dfxV2");
    assert_eq!(
        modes_listed(&cpu, data),
        [(512, 384), (640, 400), (640, 480), (800, 600), (1024, 768)]
    );
}

#[test]
fn without_an_add_on_card_the_display_is_the_only_device() {
    let (program, data) = devices_program();
    let mut cpu = card_machine("devices_none", None, program);
    run_until(&mut cpu, data, 1);
    assert_eq!(peek(&cpu, data + DEVICES), 1);
    let r = results(&cpu, data, 7);
    // A GUID of zeros, GUID_NULL, is the display.
    assert_eq!(r[1..], [0; 6], "{r:X?}");
}

/// The program that sets a mode on the card, draws through Lock, Blt
/// and BltFast, flips, reads back, and gives the display back.
fn card_program() -> (Vec<u8>, u32) {
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
    desc(
        &mut d,
        ZDESC,
        DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT | DDSD_ZBUFFERBITDEPTH,
        DDSCAPS_ZBUFFER | DDSCAPS_VIDEOMEMORY,
        640,
        480,
        0,
    );
    put32(&mut d, ZDESC + 0x18, 16);
    desc(
        &mut d,
        SYSDESC,
        DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT | DDSD_PIXELFORMAT,
        DDSCAPS_OFFSCREENPLAIN | DDSCAPS_SYSTEMMEMORY,
        4,
        2,
        0,
    );
    let pf = SYSDESC + 0x48;
    for (i, v) in [32, 0x40, 0, 16, 0xF800, 0x07E0, 0x001F].iter().enumerate() {
        put32(&mut d, pf + 4 * i as u32, *v);
    }
    put32(&mut d, LOCKED, 108);
    put32(&mut d, MODE, 108);
    put32(&mut d, LOCK2, 108);
    put32(&mut d, FX, 100);
    put32(&mut d, FX + 0x50, 0x001F);
    for (i, v) in [10, 10, 20, 20].iter().enumerate() {
        put32(&mut d, RECT_FILL + 4 * i as u32, *v);
    }
    for (i, v) in [0, 0, 4, 2].iter().enumerate() {
        put32(&mut d, RECT_SRC + 4 * i as u32, *v);
    }
    program(&[USER32, GDI32, DDRAW_CARD], &d, |a, l| {
        let mut c = Calls { next: 0 };
        device_callback(a, l)?;
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
        c.api(
            a,
            l,
            DIRECT_DRAW_ENUMERATE,
            &[Arg::Val(CALLBACKS), Arg::I(0)],
        )?;
        c.api(
            a,
            l,
            DIRECT_DRAW_CREATE,
            &[Arg::At(DEVICE_GUID), Arg::At(DD), Arg::I(0)],
        )?;
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
            &[Arg::I(640), Arg::I(480), Arg::I(16)],
        )?;
        for (desc_at, out) in [(DESC, FRONT), (ZDESC, ZBUF), (SYSDESC, SYS)] {
            c.com(
                a,
                l,
                DD,
                DD_CREATE_SURFACE,
                &[Arg::At(desc_at), Arg::At(out), Arg::I(0)],
            )?;
        }
        c.com(
            a,
            l,
            FRONT,
            S_GET_ATTACHED_SURFACE,
            &[Arg::At(CAPS), Arg::At(BACK)],
        )?;
        let lock = |a: &mut CodeAssembler, c: &mut Calls, surface, desc_at| {
            c.com(
                a,
                l,
                surface,
                S_LOCK,
                &[Arg::I(0), Arg::At(desc_at), Arg::I(1), Arg::I(0)],
            )?;
            a.mov(edi, dword_ptr(l.at(desc_at + 0x24)))?;
            a.mov(esi, dword_ptr(l.at(desc_at + 0x10)))
        };
        // The back buffer: red and green at the start of the first row,
        // blue on the second, read back.
        lock(a, &mut c, BACK, LOCKED)?;
        see(a, l, LOCKED + 0x10, 1)?;
        a.mov(dword_ptr(edi), 0x07E0_F800)?;
        a.mov(word_ptr(edi + esi), 0x001F)?;
        a.mov(eax, dword_ptr(edi))?;
        a.mov(dword_ptr(l.at(SEEN + 8)), eax)?;
        c.com(a, l, BACK, S_UNLOCK, &[Arg::I(0)])?;
        // A depth in the Z buffer, read back.
        lock(a, &mut c, ZBUF, MODE)?;
        see(a, l, MODE + 0x10, 4)?;
        a.mov(word_ptr(edi + 4), 0x1234)?;
        a.mov(eax, dword_ptr(edi + 4))?;
        a.mov(dword_ptr(l.at(SEEN + 12)), eax)?;
        c.com(a, l, ZBUF, S_UNLOCK, &[Arg::I(0)])?;
        // Pixels in system memory, copied to the back buffer at
        // (100, 100), and a blue square filled at (10, 10).
        lock(a, &mut c, SYS, LOCK2)?;
        a.mov(dword_ptr(edi), 0xAAAA_5555u32)?;
        c.com(a, l, SYS, S_UNLOCK, &[Arg::I(0)])?;
        c.com(
            a,
            l,
            BACK,
            S_BLT_FAST,
            &[
                Arg::I(100),
                Arg::I(100),
                Arg::Val(SYS),
                Arg::I(0),
                Arg::I(0),
            ],
        )?;
        c.com(
            a,
            l,
            BACK,
            S_BLT,
            &[
                Arg::At(RECT_FILL),
                Arg::I(0),
                Arg::I(0),
                Arg::I(DDBLT_COLORFILL | DDBLT_WAIT),
                Arg::At(FX),
            ],
        )?;
        c.com(a, l, FRONT, S_FLIP, &[Arg::I(0), Arg::I(DDFLIP_WAIT)])?;
        // The front buffer now.
        lock(a, &mut c, FRONT, LOCKED)?;
        a.mov(eax, dword_ptr(edi))?;
        a.mov(dword_ptr(l.at(SEEN + 20)), eax)?;
        a.imul_3(eax, esi, 15)?;
        a.mov(eax, dword_ptr(edi + eax + 30))?;
        a.mov(dword_ptr(l.at(SEEN + 24)), eax)?;
        a.imul_3(eax, esi, 100)?;
        a.mov(eax, dword_ptr(edi + eax + 200))?;
        a.mov(dword_ptr(l.at(SEEN + 28)), eax)?;
        a.mov(eax, dword_ptr(edi + esi))?;
        a.mov(dword_ptr(l.at(SEEN + 32)), eax)?;
        c.com(a, l, FRONT, S_UNLOCK, &[Arg::I(0)])?;
        // Its corner copied to system memory.
        c.com(
            a,
            l,
            SYS,
            S_BLT,
            &[
                Arg::I(0),
                Arg::Val(FRONT),
                Arg::At(RECT_SRC),
                Arg::I(DDBLT_WAIT),
                Arg::I(0),
            ],
        )?;
        lock(a, &mut c, SYS, LOCK2)?;
        a.mov(eax, dword_ptr(edi))?;
        a.mov(dword_ptr(l.at(SEEN + 36)), eax)?;
        a.mov(eax, dword_ptr(edi + esi))?;
        a.mov(dword_ptr(l.at(SEEN + 40)), eax)?;
        c.com(a, l, SYS, S_UNLOCK, &[Arg::I(0)])?;
        // The card's own video timing.
        c.com(a, l, DD, DD_GET_MONITOR_FREQUENCY, &[Arg::At(SEEN + 44)])?;
        c.com(a, l, DD, DD_GET_SCAN_LINE, &[Arg::At(SEEN + 48)])?;
        // A texture, which asks for no memory, and its caps.
        a.mov(dword_ptr(l.at(SYSDESC + 0x68)), DDSCAPS_TEXTURE)?;
        c.com(
            a,
            l,
            DD,
            DD_CREATE_SURFACE,
            &[Arg::At(SYSDESC), Arg::At(DD2), Arg::I(0)],
        )?;
        c.com(a, l, DD2, S_GET_SURFACE_DESC, &[Arg::At(LOCK2)])?;
        see(a, l, LOCK2 + 0x68, 13)?;
        wait(a, l, 1)?;
        c.com(a, l, DD, DD_RESTORE_DISPLAY_MODE, &[])?;
        c.com(a, l, FRONT, S_IS_LOST, &[])?;
        finish(a, l, 2)
    })
}

fn draws_on_the_card(test: &str, board: Board) {
    let (program, data) = card_program();
    let mut cpu = card_machine(test, Some(board), program);
    run_until(&mut cpu, data, 1);
    let r = results(&cpu, data, 27);
    assert_eq!(r[1..24], [0; 23], "{r:X?}");
    assert!([0, DDERR_VERTICALBLANKINPROGRESS].contains(&r[24]));
    assert_eq!(fbi_init0(&cpu) & 1, 1, "the card has the monitor");
    let s = |slot| seen(&cpu, data, slot);
    assert_eq!(s(1), 2048, "the linear frame buffer's pitch");
    assert_eq!(s(2), 0x07E0_F800, "the back buffer read back");
    assert_eq!(s(3) & 0xFFFF, 0x1234, "the Z buffer read back");
    assert_eq!(s(4), 2048, "the Z buffer's pitch");
    assert_eq!(s(5), 0x07E0_F800, "the back buffer's pixels shown");
    assert_eq!(s(6), 0x001F_001F, "the fill");
    assert_eq!(s(7), 0xAAAA_5555, "the copy from system memory");
    assert_eq!(s(8) & 0xFFFF, 0x001F, "the second row");
    assert_eq!(s(9), 0x07E0_F800, "the copy to system memory");
    assert_eq!(s(10) & 0xFFFF, 0x001F);
    // The card's frame: 60 Hz, 480 lines and the blanking after them.
    assert_eq!(s(11), 60, "the card's refresh rate");
    assert!(s(12) < 480 + 24, "the card's line: {}", s(12));
    // Textures are in the card's memory.
    assert_eq!(r[25..27], [0, 0]);
    let texture = DDSCAPS_TEXTURE | DDSCAPS_VIDEOMEMORY | DDSCAPS_LOCALVIDMEM;
    assert_eq!(s(13), texture, "the texture's caps");
    go_on(&mut cpu, data);
    run_until(&mut cpu, data, 2);
    let r = results(&cpu, data, 29);
    assert_eq!(r[27..], [0, DDERR_SURFACELOST]);
    assert_eq!(fbi_init0(&cpu) & 1, 0, "the VGA has the monitor back");
}

#[test]
fn directdraw_draws_and_flips_on_a_voodoo_graphics() {
    draws_on_the_card("card_v1", Board::Standard);
}

#[test]
fn directdraw_draws_and_flips_on_a_voodoo2() {
    draws_on_the_card("card_v2", Board::Voodoo2);
}

#[test]
fn the_desktop_is_packed_24_bits_on_a_virge() {
    let (program, data) = desktop_program();
    let base = PathBuf::from("target/test_win32_screen").join("desktop_virge");
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join("T.EXE"), program).unwrap();
    let mut cpu = Cpu::new(base);
    let setup = VideoSetup {
        adapter: Adapter::S3Virge,
        ..Default::default()
    };
    bios::install(&mut cpu.bus, setup);
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    run_until(&mut cpu, data, 1);
    let r = results(&cpu, data, 8);
    assert_eq!(r[1], 0, "DirectDrawCreate");
    assert_eq!(r[3], 24, "GetDeviceCaps(BITSPIXEL)");
    assert_eq!(r[6], 0x0012_3456, "GetPixel");
    assert_eq!(r[7], 0, "GetDisplayMode");
    // Three bytes a pixel, blue first, 1920 a row.
    let at = 200 * 1920 + 300 * 3;
    assert_eq!(cpu.bus.vbe.vram[at..at + 6], [0, 0, 0xFF, 0x12, 0x34, 0x56]);
    assert_eq!(shown(&mut cpu, 300, 200), [0xFF, 0, 0]);
    let mode = |o: u32| peek(&cpu, data + MODE + o);
    assert_eq!((mode(0x0C), mode(0x08), mode(0x10)), (640, 480, 1920));
    assert_eq!(mode(0x48 + 12), 24);
}

// DirectPlay, with no service providers.

#[test]
fn directplay_objects_cant_be_made() {
    const DPERR_UNAVAILABLE: u32 = 0x8877_00FA;
    let mut d = data();
    // CLSID_DirectPlay, and IID_IDirectPlay3A after it.
    let clsid = [
        0x20, 0x6D, 0xEB, 0xD1, 0x23, 0x89, 0xD0, 0x11, 0x9D, 0x97, 0x00, 0xA0, 0xC9, 0x0A, 0x43,
        0xCB,
    ];
    let iid = [
        0x41, 0xFE, 0x3E, 0x13, 0xDC, 0x32, 0xD0, 0x11, 0x9C, 0xFB, 0x00, 0xA0, 0xC9, 0x0A, 0x43,
        0xCB,
    ];
    put(&mut d, DESC, &clsid);
    put(&mut d, DESC + 16, &iid);
    for out in [DD, FRONT, BACK] {
        put32(&mut d, out, 0x1234_5678);
    }
    let dplayx: (&str, &[&str]) = (
        "DPLAYX.DLL",
        &[
            "DirectPlayCreate",
            "DirectPlayEnumerateA",
            "DirectPlayLobbyCreateA",
        ],
    );
    let ole32: (&str, &[&str]) = ("OLE32.DLL", &["CoInitialize", "CoCreateInstance"]);
    let (program, data) = program(&[dplayx, ole32], &d, |a, l| {
        let mut c = Calls { next: 0 };
        let none = Arg::I(0);
        c.api(a, l, 0, &[Arg::At(DESC), Arg::At(DD), none])?;
        c.api(a, l, 1, &[Arg::I(BASE), none])?;
        c.api(a, l, 2, &[none, Arg::At(FRONT), none, none, none])?;
        c.api(a, l, 3, &[none])?;
        let iid = Arg::At(DESC + 16);
        c.api(
            a,
            l,
            4,
            &[Arg::At(DESC), none, Arg::I(1), iid, Arg::At(BACK)],
        )?;
        finish(a, l, 1)
    });
    let mut cpu = card_machine("directplay", None, program);
    run_until(&mut cpu, data, 1);
    let r = results(&cpu, data, 5);
    assert_eq!(
        r,
        [
            DPERR_UNAVAILABLE,
            0,
            DPERR_UNAVAILABLE,
            0,
            DPERR_UNAVAILABLE
        ]
    );
    for out in [DD, FRONT, BACK] {
        assert_eq!(peek(&cpu, data + out), 0);
    }
}

/// On a Voodoo3, DirectDraw made with GUID_NULL, as programs that keep
/// the display's NULL GUID in a structure pass it, and its caps.
#[test]
fn the_voodoo3_display_has_3d_and_takes_guid_null() {
    const DD_GET_CAPS: u32 = 11;
    let mut d = data();
    // DESC: GUID_NULL. CAPS: a DDCAPS of DirectX 6's size.
    put32(&mut d, CAPS, 380);
    let (program, data) = program(&[DDRAW], &d, |a, l| {
        let mut c = Calls { next: 0 };
        c.api(a, l, 0, &[Arg::At(DESC), Arg::At(DD), Arg::I(0)])?;
        c.com(a, l, DD, DD_GET_CAPS, &[Arg::At(CAPS), Arg::I(0)])?;
        finish(a, l, 1)
    });
    let mut cpu = machine("voodoo3_caps", program);
    run_until(&mut cpu, data, 1);
    assert_eq!(results(&cpu, data, 2), [0, 0]);
    let caps = peek(&cpu, data + CAPS + 4);
    assert_eq!(caps & 1, 1, "DDCAPS_3D: {caps:X}");
    let surface_caps = peek(&cpu, data + CAPS + 0x84);
    let zbuffer = 0x2_0000;
    assert_eq!(
        surface_caps & (DDSCAPS_3DDEVICE | zbuffer),
        DDSCAPS_3DDEVICE | zbuffer
    );
}

// IDirectDraw4: its ID, and the DirectDraw object's memory and
// identity.

const QUERY_INTERFACE: u32 = 0;
const DD4_GET_AVAILABLE_VID_MEM: u32 = 23;
const DD4_GET_DEVICE_IDENTIFIER: u32 = 27;
const DDERR_OUTOFVIDEOMEMORY: u32 = 0x8876_017C;
/// A DDDEVICEIDENTIFIER, 1072 bytes.
const IDENTIFIER: u32 = 0x900;
/// A 256x256 texture's DDSURFACEDESC, a DDSCAPS2 of textures, the
/// IDirectDraw4, the textures' interface and count, IDirectDraw4's ID
/// and a surface's description.
const TEX_DESC: u32 = 0xD40;
const TEX_CAPS: u32 = 0xDB0;
const DD4: u32 = 0xDC0;
const FILLED: u32 = 0xDC4;
const COUNT: u32 = 0xDC8;
const IID_DD4: u32 = 0xDD0;
const GOT_DESC: u32 = 0xE00;

/// The data with IDirectDraw4's ID.
fn data4() -> Vec<u8> {
    let mut d = data();
    put(
        &mut d,
        IID_DD4,
        &[
            0x9A, 0x50, 0x59, 0x9C, 0xBD, 0x39, 0xD1, 0x11, 0x8C, 0x4A, 0x00, 0xC0, 0x4F, 0xD9,
            0x30, 0xC5,
        ],
    );
    d
}

/// A Voodoo3 display's DirectDraw says what card it is, as Unreal's
/// Direct3D renderer reads it to pick its settings.
#[test]
fn the_voodoo3_display_identifies_itself() {
    let (program, data) = program(&[DDRAW], &data4(), |a, l| {
        let mut c = Calls { next: 0 };
        c.api(a, l, 0, &[Arg::I(0), Arg::At(DD), Arg::I(0)])?;
        c.com(a, l, DD, QUERY_INTERFACE, &[Arg::At(IID_DD4), Arg::At(DD4)])?;
        c.com(
            a,
            l,
            DD4,
            DD4_GET_DEVICE_IDENTIFIER,
            &[Arg::At(IDENTIFIER), Arg::I(0)],
        )?;
        finish(a, l, 1)
    });
    let mut cpu = machine("voodoo3_identifier", program);
    run_until(&mut cpu, data, 1);
    assert_eq!(results(&cpu, data, 3), [0, 0, 0]);
    assert_eq!(text(&cpu, data, IDENTIFIER + 512, 32), "3dfx Voodoo3");
    assert_eq!(peek(&cpu, data + IDENTIFIER + 1032), 0x121A, "the vendor");
    assert_eq!(peek(&cpu, data + IDENTIFIER + 1036), 5, "the device");
}

/// The program that makes 256x256 textures on the 3dfx device in video
/// memory until there is no more room, asking how much there is before
/// and after, and then one that asks for no memory in particular.
fn textures_program() -> (Vec<u8>, u32) {
    let mut d = data4();
    desc(
        &mut d,
        TEX_DESC,
        DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT | DDSD_PIXELFORMAT,
        DDSCAPS_TEXTURE | DDSCAPS_VIDEOMEMORY,
        256,
        256,
        0,
    );
    let pf = TEX_DESC + 0x48;
    for (i, v) in [32, 0x40, 0, 16, 0xF800, 0x07E0, 0x001F].iter().enumerate() {
        put32(&mut d, pf + 4 * i as u32, *v);
    }
    put32(&mut d, TEX_CAPS, DDSCAPS_TEXTURE);
    put32(&mut d, GOT_DESC, 108);
    program(&[USER32, GDI32, DDRAW_CARD], &d, |a, l| {
        let mut c = Calls { next: 0 };
        device_callback(a, l)?;
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
        c.api(
            a,
            l,
            DIRECT_DRAW_ENUMERATE,
            &[Arg::Val(CALLBACKS), Arg::I(0)],
        )?;
        c.api(
            a,
            l,
            DIRECT_DRAW_CREATE,
            &[Arg::At(DEVICE_GUID), Arg::At(DD), Arg::I(0)],
        )?;
        c.com(a, l, DD, QUERY_INTERFACE, &[Arg::At(IID_DD4), Arg::At(DD4)])?;
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
            &[Arg::I(640), Arg::I(480), Arg::I(16)],
        )?;
        let vid_mem = |a: &mut CodeAssembler, c: &mut Calls, slot: u32| {
            c.com(
                a,
                l,
                DD4,
                DD4_GET_AVAILABLE_VID_MEM,
                &[
                    Arg::At(TEX_CAPS),
                    Arg::At(SEEN + 4 * slot),
                    Arg::At(SEEN + 4 * slot + 4),
                ],
            )
        };
        vid_mem(a, &mut c, 0)?;
        // Textures until one can't be made: its error in SEEN's slot 2.
        let mut again = a.create_label();
        let mut full = a.create_label();
        a.set_label(&mut again)?;
        push(a, l, &[Arg::At(TEX_DESC), Arg::At(FILLED), Arg::I(0)])?;
        a.mov(eax, dword_ptr(l.at(DD)))?;
        a.push(eax)?;
        a.mov(eax, dword_ptr(eax))?;
        a.call(dword_ptr(eax + 4 * DD_CREATE_SURFACE))?;
        a.test(eax, eax)?;
        a.jnz(full)?;
        a.add(dword_ptr(l.at(COUNT)), 1)?;
        a.cmp(dword_ptr(l.at(COUNT)), 100)?;
        a.jb(again)?;
        a.set_label(&mut full)?;
        a.mov(dword_ptr(l.at(SEEN + 8)), eax)?;
        vid_mem(a, &mut c, 3)?;
        // One more, which may be in system memory.
        a.mov(dword_ptr(l.at(TEX_DESC + 0x68)), DDSCAPS_TEXTURE)?;
        c.com(
            a,
            l,
            DD,
            DD_CREATE_SURFACE,
            &[Arg::At(TEX_DESC), Arg::At(FILLED), Arg::I(0)],
        )?;
        c.com(a, l, FILLED, S_GET_SURFACE_DESC, &[Arg::At(GOT_DESC)])?;
        see(a, l, GOT_DESC + 0x68, 5)?;
        finish(a, l, 1)
    })
}

/// The 3dfx device's textures take the card's texture memory, its two
/// units' 4 MB on a Voodoo2: when it is full, one that asks for video
/// memory fails, and one that doesn't is in system memory.
#[test]
fn textures_fill_a_voodoo2s_texture_memory() {
    let (program, data) = textures_program();
    let mut cpu = card_machine("card_textures", Some(Board::Voodoo2), program);
    run_until(&mut cpu, data, 1);
    let r = results(&cpu, data, 9);
    assert_eq!(r[1..], [0; 8], "{r:X?}");
    let s = |slot| seen(&cpu, data, slot);
    assert_eq!((s(0), s(1)), (4 << 20, 4 << 20), "total and free before");
    assert_eq!(peek(&cpu, data + COUNT), 32, "128 KB textures in 4 MB");
    assert_eq!(s(2), DDERR_OUTOFVIDEOMEMORY);
    assert_eq!((s(3), s(4)), (4 << 20, 0), "total and free after");
    assert_eq!(s(5) & DDSCAPS_SYSTEMMEMORY, DDSCAPS_SYSTEMMEMORY);
}

const USER32_PAL: (&str, &[&str]) = ("USER32.DLL", &["CreateWindowExA", "GetDC"]);
const GDI32_PAL: (&str, &[&str]) = (
    "GDI32.DLL",
    &[
        "GetDeviceCaps",
        "SetPixel",
        "CreatePalette",
        "SelectPalette",
        "RealizePalette",
        "AnimatePalette",
        "GetSystemPaletteEntries",
        "GetNearestColor",
        "GetNearestPaletteIndex",
        "SetDIBitsToDevice",
    ],
);
const P_SET_PIXEL: usize = 3;
const CREATE_PALETTE: usize = 4;
const SELECT_PALETTE: usize = 5;
const REALIZE_PALETTE: usize = 6;
const ANIMATE_PALETTE: usize = 7;
const GET_SYSTEM_PALETTE_ENTRIES: usize = 8;
const GET_NEAREST_COLOR: usize = 9;
const GET_NEAREST_PALETTE_INDEX: usize = 10;
const SET_DI_BITS_TO_DEVICE: usize = 11;

// The palette program's data.
const LOGPALETTE: u32 = 0x400;
const ANIMATED: u32 = 0x480;
const SYSTEM: u32 = 0x500;
const BMI: u32 = 0x600;
const BITS: u32 = 0x700;

const PC_RESERVED: u32 = 0x01;
const PC_NOCOLLAPSE: u32 = 0x04;

/// On a desktop of 256 colours: GetDeviceCaps, a palette of four entries
/// realized, pixels set through it, one of its entries animated, the
/// system palette read back, the nearest colors, and a DIB_PAL_COLORS
/// bitmap drawn.
fn palette_program() -> (Vec<u8>, u32) {
    let mut d = data();
    put32(&mut d, LOGPALETTE, 0x300 | 4 << 16);
    put32(&mut d, LOGPALETTE + 4, 0x1E140A);
    put32(&mut d, LOGPALETTE + 8, 0x3C3228 | PC_RESERVED << 24);
    put32(&mut d, LOGPALETTE + 12, 0x800000);
    put32(&mut d, LOGPALETTE + 16, 0x1E140A | PC_NOCOLLAPSE << 24);
    put32(&mut d, ANIMATED, 0xC08040 | PC_RESERVED << 24);
    // A BITMAPINFOHEADER of 2x1 at 8 bits, and its four indices into the
    // DC's palette.
    put32(&mut d, BMI, 40);
    put32(&mut d, BMI + 4, 2);
    put32(&mut d, BMI + 8, 1);
    put32(&mut d, BMI + 12, 1 | 8 << 16);
    put32(&mut d, BMI + 32, 4);
    put(&mut d, BMI + 40, &[0, 0, 1, 0, 2, 0, 3, 0]);
    put(&mut d, BITS, &[3, 2, 0, 0]);
    program(&[USER32_PAL, GDI32_PAL], &d, |a, l| {
        let mut c = Calls { next: 0 };
        let (x, y, w, h) = (0, 0, 64, 64);
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
        c.api(a, l, GET_DC, &[Arg::I(0)])?;
        a.mov(dword_ptr(l.at(HDC)), eax)?;
        // BITSPIXEL, RASTERCAPS, SIZEPALETTE, NUMRESERVED, NUMCOLORS and
        // COLORRES.
        for index in [12, 38, 104, 106, 24, 108] {
            c.api(a, l, GET_DEVICE_CAPS, &[Arg::Val(HDC), Arg::I(index)])?;
        }
        c.api(a, l, CREATE_PALETTE, &[Arg::At(LOGPALETTE)])?;
        a.mov(dword_ptr(l.at(PALETTE)), eax)?;
        c.api(
            a,
            l,
            SELECT_PALETTE,
            &[Arg::Val(HDC), Arg::Val(PALETTE), Arg::I(0)],
        )?;
        c.api(a, l, REALIZE_PALETTE, &[Arg::Val(HDC)])?;
        c.api(a, l, REALIZE_PALETTE, &[Arg::Val(HDC)])?;
        // PALETTEINDEX(1), a plain RGB and PALETTERGB.
        for (x, color) in [(0, 0x0100_0001), (1, 0x1E140A), (2, 0x0200_0000 | 0x3C3228)] {
            c.api(
                a,
                l,
                P_SET_PIXEL,
                &[Arg::Val(HDC), Arg::I(x), Arg::I(0), Arg::I(color)],
            )?;
        }
        c.api(
            a,
            l,
            ANIMATE_PALETTE,
            &[Arg::Val(PALETTE), Arg::I(1), Arg::I(1), Arg::At(ANIMATED)],
        )?;
        c.api(
            a,
            l,
            GET_SYSTEM_PALETTE_ENTRIES,
            &[Arg::Val(HDC), Arg::I(10), Arg::I(3), Arg::At(SYSTEM)],
        )?;
        c.api(a, l, GET_NEAREST_COLOR, &[Arg::Val(HDC), Arg::I(0x1E140B)])?;
        c.api(
            a,
            l,
            GET_NEAREST_COLOR,
            &[Arg::Val(HDC), Arg::I(0x0100_0002)],
        )?;
        c.api(
            a,
            l,
            GET_NEAREST_PALETTE_INDEX,
            &[Arg::Val(PALETTE), Arg::I(0x3C3228)],
        )?;
        c.api(
            a,
            l,
            SET_DI_BITS_TO_DEVICE,
            &[
                Arg::Val(HDC),
                Arg::I(10),
                Arg::I(10),
                Arg::I(2),
                Arg::I(1),
                Arg::I(0),
                Arg::I(0),
                Arg::I(0),
                Arg::I(1),
                Arg::At(BITS),
                Arg::At(BMI),
                Arg::I(1),
            ],
        )?;
        finish(a, l, 1)
    })
}

/// With win32_depth=8 the desktop is 640x480 at 8 bits a pixel, its DAC
/// the system palette: a logical palette's entries take free system
/// entries (or match a static color), pixels are their indices, and
/// AnimatePalette changes the DAC.
#[test]
fn a_256_colour_desktop_has_a_system_palette() {
    let (program, data) = palette_program();
    let mut cpu = card_machine("palette", None, program);
    cpu.bus.win32.depth = rust_dos::config::DesktopDepth::Palette;
    run_until(&mut cpu, data, 1);
    let r = results(&cpu, data, 21);
    assert_eq!((r[2], r[3] & 0x100), (8, 0x100), "BITSPIXEL, RC_PALETTE");
    assert_eq!(r[4..8], [256, 20, 20, 18], "{r:X?}");
    assert_ne!(r[8], 0, "CreatePalette");
    assert_eq!(r[10], 4, "the first RealizePalette maps the four entries");
    assert_eq!(r[11], 0, "the second has nothing to do");
    assert_eq!(r[15], 1, "AnimatePalette");
    assert_eq!(r[16], 3, "GetSystemPaletteEntries");
    let system: Vec<u32> = (0..3)
        .map(|i| peek(&cpu, data + SYSTEM + 4 * i) & 0xFF_FFFF)
        .collect();
    assert_eq!(system, [0x1E140A, 0xC08040, 0x1E140A]);
    assert_eq!(r[17], 0, "a plain color's nearest is a static one, black");
    assert_eq!(r[18], 0x80_0000, "PALETTEINDEX(2) is static dark blue");
    // Entry 1 is animated to (0x40, 0x80, 0xC0) by now.
    assert_eq!(r[19], 0, "GetNearestPaletteIndex");
    assert_eq!(r[20], 1, "SetDIBitsToDevice's lines");
    let vbe = &cpu.bus.vbe;
    let pixel = |x: usize, y: usize| vbe.vram[y * 640 + x];
    assert_eq!((pixel(0, 0), pixel(1, 0), pixel(2, 0)), (11, 0, 11));
    assert_eq!((pixel(10, 10), pixel(11, 10)), (12, 4));
    let shift = if cpu.bus.vga.dac_8bit { 0 } else { 2 };
    let dac = &cpu.bus.vga.palette;
    assert_eq!(dac[11 * 3..12 * 3], [0x40 >> shift, 0x80 >> shift, 0xC0 >> shift]);
    assert_eq!(dac[10 * 3..11 * 3], [10 >> shift, 20 >> shift, 30 >> shift]);
}

const USER32_CURSOR: (&str, &[&str]) = (
    "USER32.DLL",
    &["CreateWindowExA", "LoadCursorA", "SetCursor", "SetCursorPos", "ShowCursor"],
);
const LOAD_CURSOR: usize = 1;
const SET_CURSOR: usize = 2;
const SET_CURSOR_POS: usize = 3;
const SHOW_CURSOR: usize = 4;
const IDC_ARROW: u32 = 32512;

/// A window on the desktop, the arrow set as the cursor and the cursor
/// put at (100, 100) (step 1); then the cursor hidden (step 2).
fn cursor_program() -> (Vec<u8>, u32) {
    program(&[USER32_CURSOR], &data(), |a, l| {
        let mut c = Calls { next: 0 };
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
        c.api(a, l, LOAD_CURSOR, &[Arg::I(0), Arg::I(IDC_ARROW)])?;
        a.push(eax)?;
        a.call(l.import(SET_CURSOR))?;
        c.keep(a, l)?;
        c.api(a, l, SET_CURSOR_POS, &[Arg::I(100), Arg::I(100)])?;
        wait(a, l, 1)?;
        c.api(a, l, SHOW_CURSOR, &[Arg::I(0)])?;
        finish(a, l, 2)
    })
}

/// The pixels (`x`, `y`) of the screen with the cursors drawn over it,
/// as the front ends and /api/screenshot show it.
fn with_cursors(cpu: &Cpu, at: &[(usize, usize)]) -> Vec<[u8; 3]> {
    let (width, height) = video::frame_size(&cpu.bus);
    let mut frame = Frame::new(width, height);
    video::render_screen(&mut frame, &cpu.bus);
    video::overlay::draw_cursors(&mut frame, &cpu.bus, false);
    at.iter()
        .map(|&(x, y)| {
            let i = (y * width as usize + x) * 4;
            [frame.rgba[i], frame.rgba[i + 1], frame.rgba[i + 2]]
        })
        .collect()
}

/// The built-in Windows draws the cursor SetCursor set where the mouse
/// is, on a desktop of 256 colours: the arrow's tip at the cursor's place
/// is black and its inside white. ShowCursor(FALSE) takes it away.
fn the_cursor_shows(test: &str, board: Option<Board>) {
    let (program, data) = cursor_program();
    let mut cpu = if board == Some(Board::Voodoo3) {
        machine(test, program)
    } else {
        card_machine(test, board, program)
    };
    cpu.bus.win32.depth = rust_dos::config::DesktopDepth::Palette;
    run_until(&mut cpu, data, 1);
    let r = results(&cpu, data, 4);
    assert_ne!(r[0], 0, "CreateWindowExA");
    assert_ne!(r[1], 0, "LoadCursorA(IDC_ARROW)");
    assert_eq!(r[3], 1, "SetCursorPos");
    let points = [(100, 100), (101, 102), (300, 300)];
    let shown = with_cursors(&cpu, &points);
    assert_eq!(shown[0], [0, 0, 0], "the arrow's tip");
    assert_eq!(shown[1], [0xFF, 0xFF, 0xFF], "the arrow's inside");
    let desktop = shown[2];
    assert_ne!(desktop, [0xFF, 0xFF, 0xFF], "the desktop isn't white");
    go_on(&mut cpu, data);
    run_until(&mut cpu, data, 2);
    assert_eq!(results(&cpu, data, 5)[4], u32::MAX, "ShowCursor(FALSE)");
    let hidden = with_cursors(&cpu, &points);
    assert_eq!(hidden[1], desktop, "no cursor over the desktop");
}

#[test]
fn the_cursor_shows_on_a_vesa_desktop() {
    the_cursor_shows("cursor_vesa", None);
}

/// A Voodoo3 shows the desktop in its extended mode, the 3dfx card's own
/// picture, and Windows' cursor is drawn over it.
#[test]
fn the_cursor_shows_on_a_voodoo3_desktop() {
    the_cursor_shows("cursor_voodoo3", Some(Board::Voodoo3));
}

/// A full-screen DirectDraw mode of `w` x `h` at 8 bits, the arrow set
/// as the cursor and the cursor put at (100, 100) (step 1).
fn mode_cursor_program(w: u32, h: u32) -> (Vec<u8>, u32) {
    program(&[USER32_CURSOR, DDRAW], &data(), |a, l| {
        let mut c = Calls { next: 0 };
        start(a, l, &mut c, (0, 0, 64, 64))?;
        c.com(
            a,
            l,
            DD,
            DD_SET_COOPERATIVE_LEVEL,
            &[Arg::Val(HWND), Arg::I(DDSCL_EXCLUSIVE | DDSCL_FULLSCREEN)],
        )?;
        c.com(a, l, DD, DD_SET_DISPLAY_MODE, &[Arg::I(w), Arg::I(h), Arg::I(8)])?;
        c.api(a, l, LOAD_CURSOR, &[Arg::I(0), Arg::I(IDC_ARROW)])?;
        a.push(eax)?;
        a.call(l.import(SET_CURSOR))?;
        c.keep(a, l)?;
        c.api(a, l, SET_CURSOR_POS, &[Arg::I(100), Arg::I(100)])?;
        finish(a, l, 1)
    })
}

/// Over a DirectDraw mode of 320 pixels across, the cursor is in the
/// mode's pixels, as large as they show: the arrow's inside, one pixel
/// right of the tip and two down, covers that pixel of the mode on the
/// frame, however much larger the frame shows it.
fn the_cursor_shows_in_a_small_mode(test: &str, board: Option<Board>, (w, h): (u32, u32)) {
    let (program, data) = mode_cursor_program(w, h);
    let mut cpu = if board == Some(Board::Voodoo3) {
        machine(test, program)
    } else {
        card_machine(test, board, program)
    };
    run_until(&mut cpu, data, 1);
    let r = results(&cpu, data, 7);
    assert_eq!(r[2..4], [0, 0], "SetCooperativeLevel, SetDisplayMode");
    assert_ne!(r[4], 0, "LoadCursorA(IDC_ARROW)");
    assert_eq!(r[6], 1, "SetCursorPos");
    let (frame_w, frame_h) = video::frame_size(&cpu.bus);
    let (sx, sy) = ((frame_w / w) as usize, (frame_h / h) as usize);
    let inside = (101 * sx, 102 * sy);
    let corners = [inside, (inside.0 + sx - 1, inside.1 + sy - 1)];
    for (i, pixel) in with_cursors(&cpu, &corners).into_iter().enumerate() {
        assert_eq!(pixel, [0xFF, 0xFF, 0xFF], "the arrow's inside, corner {i}");
    }
    let beside = with_cursors(&cpu, &[(120 * sx, 102 * sy)])[0];
    assert_ne!(beside, [0xFF, 0xFF, 0xFF], "beside the arrow");
}

/// The VESA kind shows 320x240 at twice its size both ways.
#[test]
fn the_cursor_shows_in_a_320_by_240_vesa_mode() {
    the_cursor_shows_in_a_small_mode("cursor_vesa_320", None, (320, 240));
}

/// A Voodoo3 shows 320x200 with its lines doubled.
#[test]
fn the_cursor_shows_in_a_320_by_200_voodoo3_mode() {
    the_cursor_shows_in_a_small_mode("cursor_voodoo3_320", Some(Board::Voodoo3), (320, 200));
}
