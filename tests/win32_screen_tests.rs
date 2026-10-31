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

/// Let the next retrace come, as the frontend does before it draws.
fn next_retrace(cpu: &mut Cpu) {
    cpu.bus.clock.icount += cpu.bus.clock.cycles_per_ms() as u64 * 20;
    cpu.bus.sync_display();
}

#[test]
fn a_flip_on_the_card_counts_as_a_frame() {
    let (program, data) = fullscreen_program();
    let mut cpu = machine("flip_frames", program);
    run_until(&mut cpu, data, 1);
    go_on(&mut cpu, data);
    // The program stops right after its flip.
    run_until(&mut cpu, data, 2);
    assert_eq!(results(&cpu, data, 9)[1..], [0; 8]);
    // The flip is one frame, at the retrace after it, and nothing after
    // it is another.
    let before = cpu.bus.frames_drawn;
    for _ in 0..3 {
        next_retrace(&mut cpu);
    }
    assert_eq!(cpu.bus.frames_drawn - before, 1);
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
    // The card runs it at half the pixel clock, so the monitor shows each
    // pixel twice as wide as well as twice as high: the picture spans
    // the screen as a 640x400 one does, as mode 13h's does.
    assert_eq!(video::frame_size(&cpu.bus), (640, 400));
    for (x, y) in [(10, 0), (11, 0), (10, 1), (11, 1)] {
        assert_eq!(shown(&mut cpu, x, y), [7, 0x40, 0xF8], "({x}, {y})");
    }
    assert_eq!(shown(&mut cpu, 9, 0), [0, 0x40, 0xFF]);
    assert_eq!(shown(&mut cpu, 12, 0), [0, 0x40, 0xFF]);
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

const S_ADD_ATTACHED_SURFACE: u32 = 3;

/// The program that sets a mode on the card, makes a plain primary
/// surface and a back buffer on its own (as Glover does), attaches the
/// back buffer to the primary, fills a square of it with the card,
/// reads the square back and writes a pixel through Lock, and flips.
fn card_attached_back_buffer_program() -> (Vec<u8>, u32) {
    let mut d = data();
    desc(&mut d, DESC, DDSD_CAPS, DDSCAPS_PRIMARYSURFACE, 0, 0, 0);
    desc(
        &mut d,
        ZDESC,
        DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT,
        DDSCAPS_BACKBUFFER | DDSCAPS_3DDEVICE,
        640,
        480,
        0,
    );
    put32(&mut d, LOCKED, 108);
    put32(&mut d, LOCK2, 108);
    put32(&mut d, FX, 100);
    put32(&mut d, FX + 0x50, 0x001F);
    for (i, v) in [10, 10, 20, 20].iter().enumerate() {
        put32(&mut d, RECT_FILL + 4 * i as u32, *v);
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
        for (desc_at, out) in [(DESC, FRONT), (ZDESC, BACK)] {
            c.com(
                a,
                l,
                DD,
                DD_CREATE_SURFACE,
                &[Arg::At(desc_at), Arg::At(out), Arg::I(0)],
            )?;
        }
        c.com(a, l, FRONT, S_ADD_ATTACHED_SURFACE, &[Arg::Val(BACK)])?;
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
        // The square at (15, 15) through the pitch Lock gave, then red
        // at the start of the second row.
        lock(a, &mut c, BACK, LOCKED)?;
        see(a, l, LOCKED + 0x10, 1)?;
        a.imul_3(eax, esi, 15)?;
        a.mov(eax, dword_ptr(edi + eax + 30))?;
        a.mov(dword_ptr(l.at(SEEN + 8)), eax)?;
        a.mov(word_ptr(edi + esi), 0xF800)?;
        c.com(a, l, BACK, S_UNLOCK, &[Arg::I(0)])?;
        c.com(a, l, FRONT, S_FLIP, &[Arg::I(0), Arg::I(DDFLIP_WAIT)])?;
        // The front buffer now, and what GetSurfaceDesc says of the
        // back buffer.
        lock(a, &mut c, FRONT, LOCKED)?;
        a.mov(eax, dword_ptr(edi + esi))?;
        a.mov(dword_ptr(l.at(SEEN + 12)), eax)?;
        a.imul_3(eax, esi, 15)?;
        a.mov(eax, dword_ptr(edi + eax + 30))?;
        a.mov(dword_ptr(l.at(SEEN + 16)), eax)?;
        c.com(a, l, FRONT, S_UNLOCK, &[Arg::I(0)])?;
        c.com(a, l, BACK, S_GET_SURFACE_DESC, &[Arg::At(LOCK2)])?;
        see(a, l, LOCK2 + 0x10, 5)?;
        finish(a, l, 1)
    })
}

/// Glover's loading screen on a Voodoo2: a back buffer made on its own
/// and attached to the primary surface is the card's back buffer, so
/// its rows are the linear frame buffer's, 2048 bytes apart, as the
/// primary's and the Z buffer's are. With the 1280 bytes of a 640-pixel
/// surface in memory, the picture Glover wrote row by row came out
/// squeezed into the top 300 lines.
#[test]
fn a_back_buffer_attached_on_a_voodoo2_has_the_cards_pitch() {
    let (program, data) = card_attached_back_buffer_program();
    let mut cpu = card_machine("card_attached_back", Some(Board::Voodoo2), program);
    run_until(&mut cpu, data, 1);
    let r = results(&cpu, data, 15);
    assert_eq!(r[1..15], [0; 14], "{r:X?}");
    let s = |slot| seen(&cpu, data, slot);
    assert_eq!(s(1), 2048, "the pitch Lock gives");
    assert_eq!(s(2), 0x001F_001F, "the card's fill read back");
    assert_eq!(s(3) & 0xFFFF, 0xF800, "the second row shown");
    assert_eq!(s(4), 0x001F_001F, "the fill shown");
    assert_eq!(s(5), 2048, "the pitch GetSurfaceDesc gives");
}

/// A machine with the VESA adapter `adapter`, the program loaded.
fn vesa_machine(test: &str, adapter: Adapter, program: Vec<u8>) -> Cpu {
    let base = PathBuf::from("target/test_win32_screen").join(test);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join("T.EXE"), program).unwrap();
    let mut cpu = Cpu::new(base);
    let setup = VideoSetup {
        adapter,
        ..Default::default()
    };
    bios::install(&mut cpu.bus, setup);
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    cpu
}

#[test]
fn the_desktop_is_packed_24_bits_on_a_virge() {
    let (program, data) = desktop_program();
    let mut cpu = vesa_machine("desktop_virge", Adapter::S3Virge, program);
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

/// With win32_depth=16 the desktop on an S3 Trio is High Color, 5-6-5,
/// as a program that asks for 16 bits, such as Moto Racer, needs.
#[test]
fn a_high_color_desktop_on_an_s3_has_16_bits() {
    let (program, data) = desktop_program();
    let mut cpu = vesa_machine("desktop_16", Adapter::S3, program);
    cpu.bus.win32.depth = rust_dos::config::DesktopDepth::HighColor;
    run_until(&mut cpu, data, 1);
    let r = results(&cpu, data, 8);
    assert_eq!(r[1], 0, "DirectDrawCreate");
    assert_eq!(r[3], 16, "GetDeviceCaps(BITSPIXEL)");
    assert_eq!(r[6], 0x0010_3452, "GetPixel, 5-6-5 widened");
    assert_eq!(r[7], 0, "GetDisplayMode");
    let at = 200 * 1280 + 301 * 2;
    assert_eq!(cpu.bus.vbe.vram[at..at + 2], [0xA2, 0x51]);
    assert_eq!(shown(&mut cpu, 300, 200), [0xFF, 0, 0]);
    let mode = |o: u32| peek(&cpu, data + MODE + o);
    assert_eq!((mode(0x0C), mode(0x08), mode(0x10)), (640, 480, 1280));
    assert_eq!(mode(0x48 + 12), 16);
    assert_eq!(
        (mode(0x48 + 16), mode(0x48 + 20), mode(0x48 + 24)),
        (0xF800, 0x07E0, 0x001F)
    );
}

// DirectPlay, with no service provider it can use: objects are made,
// and list no connections; and its lobby, with no lobby.

const DPERR_INVALIDPARAMS: u32 = 0x8007_0057;
const DPERR_ALREADYINITIALIZED: u32 = 0x8877_0005;
const DPERR_BUFFERTOOSMALL: u32 = 0x8877_001E;
const DPERR_INVALIDFLAGS: u32 = 0x8877_0078;
const DPERR_INVALIDOBJECT: u32 = 0x8877_0082;
const DPERR_NOMESSAGES: u32 = 0x8877_00BE;
const DPERR_UNAVAILABLE: u32 = 0x8877_00FA;
const DPERR_UNINITIALIZED: u32 = 0x8877_0140;
const DPERR_NOTLOBBIED: u32 = 0x8877_042E;
const E_NOINTERFACE: u32 = 0x8000_4002;
const CLASS_E_NOAGGREGATION: u32 = 0x8004_0110;
// Methods' places in IDirectPlay2 to 4's vtables, in IDirectPlay's and
// in the lobby's.
const DP_QUERY_INTERFACE: u32 = 0;
const DP_ADD_REF: u32 = 1;
const DP_RELEASE: u32 = 2;
const DP_CLOSE: u32 = 4;
const DP_ENUM_SESSIONS: u32 = 13;
const DP_GET_CAPS: u32 = 14;
const DP_OPEN: u32 = 24;
const DP_RECEIVE: u32 = 25;
const DP_ENUM_CONNECTIONS: u32 = 35;
const DP_INITIALIZE_CONNECTION: u32 = 38;
const DP1_ENUM_SESSIONS: u32 = 14;
const DP1_INITIALIZE: u32 = 19;
const DPL_CREATE_ADDRESS: u32 = 4;
const DPL_ENUM_ADDRESS: u32 = 5;
const DPL_ENUM_ADDRESS_TYPES: u32 = 6;
const DPL_ENUM_LOCAL_APPLICATIONS: u32 = 7;
const DPL_GET_CONNECTION_SETTINGS: u32 = 8;
/// The data's GUIDs.
const DP_CLSID: u32 = 0x500;
const DP_IID3A: u32 = 0x510;
const DP_IID4A: u32 = 0x520;
const DP_GUID_NULL: u32 = 0x530;
const DP_IUNKNOWN: u32 = 0x540;
const DP_IID4: u32 = 0x550;
const DP_IID2A: u32 = 0x560;
const DPL_CLSID: u32 = 0x570;
const DPL_IID3A: u32 = 0x580;
const DP_TCPIP: u32 = 0x590;
const DP_INET: u32 = 0x5A0;
const DP_SERIAL: u32 = 0x5B0;
/// A host's name, a DPSESSIONDESC2 and a DPCAPS.
const DP_HOST: u32 = 0x5C0;
const DP_SESSION: u32 = 0x5D0;
const DP_CAPS: u32 = 0x620;
/// What the callbacks counted, Receive's player IDs and size, an
/// address's size, a vtable kept, the sizes of an address's chunks, and
/// the address.
const DP_CALLED: u32 = 0x650;
const DP_TYPES: u32 = 0x654;
const DP_FROM: u32 = 0x658;
const DP_TO: u32 = 0x65C;
const DP_SIZE: u32 = 0x660;
const DP_ADDRESS_SIZE: u32 = 0x664;
const DP_VTABLE: u32 = 0x668;
const DP_APPS: u32 = 0x66C;
const DP_SIZES: u32 = 0x670;
const DP_ADDRESS: u32 = 0x6A0;
/// Interfaces' pointers, past the ones the data's layout has.
const DP_UNKNOWN1: u32 = 0x740;
const DP_UNKNOWN2: u32 = 0x744;
const DP_PLAY4: u32 = 0x748;
const DP_PLAY2A: u32 = 0x74C;
const DP_NONE: u32 = 0x750;
const DP_LOBBY: u32 = 0x754;
const DP_LOBBY3A: u32 = 0x758;

/// A GUID's bytes as dplay.h writes it.
fn guid_bytes(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Vec<u8> {
    [
        &d1.to_le_bytes()[..],
        &d2.to_le_bytes(),
        &d3.to_le_bytes(),
        &d4,
    ]
    .concat()
}

/// The data of the DirectPlay tests: their GUIDs and structures, and
/// the pointers the calls give filled with something else.
fn directplay_data() -> Vec<u8> {
    let mut d = data();
    let guids = [
        (
            DP_CLSID,
            guid_bytes(
                0xD1EB_6D20,
                0x8923,
                0x11D0,
                [0x9D, 0x97, 0, 0xA0, 0xC9, 0x0A, 0x43, 0xCB],
            ),
        ),
        (
            DP_IID3A,
            guid_bytes(
                0x133E_FE41,
                0x32DC,
                0x11D0,
                [0x9C, 0xFB, 0, 0xA0, 0xC9, 0x0A, 0x43, 0xCB],
            ),
        ),
        (
            DP_IID4A,
            guid_bytes(
                0x0AB1_C531,
                0x4745,
                0x11D1,
                [0xA7, 0xA1, 0, 0, 0xF8, 0x03, 0xAB, 0xFC],
            ),
        ),
        (
            DP_IUNKNOWN,
            guid_bytes(0, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]),
        ),
        (
            DP_IID4,
            guid_bytes(
                0x0AB1_C530,
                0x4745,
                0x11D1,
                [0xA7, 0xA1, 0, 0, 0xF8, 0x03, 0xAB, 0xFC],
            ),
        ),
        (
            DP_IID2A,
            guid_bytes(
                0x9D46_0580,
                0xA822,
                0x11CF,
                [0x96, 0x0C, 0, 0x80, 0xC7, 0x53, 0x4E, 0x82],
            ),
        ),
        (
            DPL_CLSID,
            guid_bytes(
                0x2FE8_F810,
                0xB2A5,
                0x11D0,
                [0xA7, 0x87, 0, 0, 0xF8, 0x03, 0xAB, 0xFC],
            ),
        ),
        (
            DPL_IID3A,
            guid_bytes(
                0x2DB7_2491,
                0x652C,
                0x11D1,
                [0xA7, 0xA8, 0, 0, 0xF8, 0x03, 0xAB, 0xFC],
            ),
        ),
        (
            DP_TCPIP,
            guid_bytes(
                0x36E9_5EE0,
                0x8577,
                0x11CF,
                [0x96, 0x0C, 0, 0x80, 0xC7, 0x53, 0x4E, 0x82],
            ),
        ),
        (
            DP_INET,
            guid_bytes(
                0xC4A5_4DA0,
                0xE0AF,
                0x11CF,
                [0x9C, 0x4E, 0, 0xA0, 0xC9, 0x05, 0x42, 0x5E],
            ),
        ),
        (
            DP_SERIAL,
            guid_bytes(
                0x0F1D_6860,
                0x88D9,
                0x11CF,
                [0x9C, 0x4E, 0, 0xA0, 0xC9, 0x05, 0x42, 0x5E],
            ),
        ),
    ];
    for (at, guid) in guids {
        put(&mut d, at, &guid);
    }
    put(&mut d, DP_HOST, b"10.0.2.2\0");
    put32(&mut d, DP_SESSION, 80);
    put32(&mut d, DP_CAPS, 40);
    for out in [DD, FRONT, BACK, TARGET, DP_NONE] {
        put32(&mut d, out, 0x1234_5678);
    }
    d
}

/// A callback taking `args` arguments that counts its calls in the
/// data's dword `counter` and asks for more, in CALLBACKS' slot `slot`.
fn counting_callback(
    a: &mut CodeAssembler,
    l: &Layout,
    slot: u32,
    args: u32,
    counter: u32,
) -> Result<(), IcedError> {
    callback(a, l, slot, |a| {
        a.inc(dword_ptr(l.at(counter)))?;
        a.mov(eax, 1)?;
        a.ret_1(4 * args)
    })
}

/// EnumAddress's callback (type, size, data, context), in CALLBACKS'
/// slot `slot`: it counts the chunks in DP_CALLED and keeps their sizes
/// in DP_SIZES.
fn chunk_callback(a: &mut CodeAssembler, l: &Layout, slot: u32) -> Result<(), IcedError> {
    callback(a, l, slot, |a| {
        a.mov(eax, dword_ptr(l.at(DP_CALLED)))?;
        a.inc(dword_ptr(l.at(DP_CALLED)))?;
        a.shl(eax, 2)?;
        a.add(eax, l.at(DP_SIZES) as i32)?;
        a.mov(ecx, dword_ptr(esp + 8))?;
        a.mov(dword_ptr(eax), ecx)?;
        a.mov(eax, 1)?;
        a.ret_1(16)
    })
}

/// Keep the vtable of the interface at data offset `object` in
/// DP_VTABLE, to call it after the interface is gone.
fn keep_vtable(a: &mut CodeAssembler, l: &Layout, object: u32) -> Result<(), IcedError> {
    a.mov(eax, dword_ptr(l.at(object)))?;
    a.mov(eax, dword_ptr(eax))?;
    a.mov(dword_ptr(l.at(DP_VTABLE)), eax)
}

/// Call method `method` of the vtable kept in DP_VTABLE, with the
/// interface pointer at data offset `object` and no other arguments.
fn call_kept(
    c: &mut Calls,
    a: &mut CodeAssembler,
    l: &Layout,
    object: u32,
    method: u32,
) -> Result<(), IcedError> {
    a.push(dword_ptr(l.at(object)))?;
    a.mov(eax, dword_ptr(l.at(DP_VTABLE)))?;
    a.call(dword_ptr(eax + 4 * method))?;
    c.keep(a, l)
}

/// The program's dword at its data's offset `offset`.
fn data_word(cpu: &Cpu, data: u32, offset: u32) -> u32 {
    peek(cpu, data + offset)
}

/// CoCreateInstance makes a DirectPlay object, as Windows 98 does with
/// no network card. Each IID is an interface of its own with its own
/// count, IUnknown is the same pointer whichever interface it is asked
/// of, and an IID it hasn't gives E_NOINTERFACE. It lists no
/// connections, can't be given one the program made up, and the methods
/// that need one say it has none. A call through an interface of an
/// object that is gone gives DPERR_INVALIDOBJECT.
#[test]
fn cocreateinstance_makes_a_directplay_with_no_connections() {
    let ole32: (&str, &[&str]) = ("OLE32.DLL", &["CoInitialize", "CoCreateInstance"]);
    let (program, data) = program(&[ole32], &directplay_data(), |a, l| {
        counting_callback(a, l, 0, 6, DP_CALLED)?;
        counting_callback(a, l, 1, 4, DP_CALLED)?;
        let mut c = Calls { next: 0 };
        let none = Arg::I(0);
        c.api(a, l, 0, &[none])?;
        let create = [
            Arg::At(DP_CLSID),
            none,
            Arg::I(1),
            Arg::At(DP_IID3A),
            Arg::At(DD),
        ];
        c.api(a, l, 1, &create)?;
        let query = |iid, out| [Arg::At(iid), Arg::At(out)];
        c.com(a, l, DD, DP_QUERY_INTERFACE, &query(DP_IID4A, FRONT))?;
        c.com(
            a,
            l,
            DD,
            DP_QUERY_INTERFACE,
            &query(DP_IUNKNOWN, DP_UNKNOWN1),
        )?;
        c.com(
            a,
            l,
            FRONT,
            DP_QUERY_INTERFACE,
            &query(DP_IUNKNOWN, DP_UNKNOWN2),
        )?;
        c.com(a, l, FRONT, DP_QUERY_INTERFACE, &query(DP_IID4, DP_PLAY4))?;
        c.com(
            a,
            l,
            DP_PLAY4,
            DP_QUERY_INTERFACE,
            &query(DP_IID2A, DP_PLAY2A),
        )?;
        c.com(a, l, FRONT, DP_QUERY_INTERFACE, &query(DP_CLSID, DP_NONE))?;
        let enum_connections = [none, Arg::Val(CALLBACKS), none, none];
        c.com(a, l, FRONT, DP_ENUM_CONNECTIONS, &enum_connections)?;
        c.com(a, l, FRONT, DP_ENUM_CONNECTIONS, &[none, none, none, none])?;
        let session = Arg::At(DP_SESSION);
        let enum_sessions = [session, none, Arg::Val(CALLBACKS + 4), none, none];
        c.com(a, l, DP_PLAY4, DP_ENUM_SESSIONS, &enum_sessions)?;
        c.com(a, l, DP_PLAY2A, DP_OPEN, &[session, Arg::I(1)])?;
        // A connection the program made up: CLSID_DirectPlay's bytes.
        let made_up = [Arg::At(DP_CLSID), none];
        c.com(a, l, FRONT, DP_INITIALIZE_CONNECTION, &made_up)?;
        // No caps, whatever the arguments.
        c.com(a, l, FRONT, DP_GET_CAPS, &[none, none])?;
        let receive = |flags| {
            [
                Arg::At(DP_FROM),
                Arg::At(DP_TO),
                Arg::I(flags),
                none,
                Arg::At(DP_SIZE),
            ]
        };
        c.com(a, l, FRONT, DP_RECEIVE, &receive(0))?;
        c.com(a, l, FRONT, DP_RECEIVE, &receive(0x10))?;
        c.com(a, l, FRONT, DP_CLOSE, &[])?;
        for object in [FRONT, DP_PLAY2A, DP_PLAY4, DP_UNKNOWN2, DP_UNKNOWN1] {
            c.com(a, l, object, DP_RELEASE, &[])?;
        }
        keep_vtable(a, l, DD)?;
        c.com(a, l, DD, DP_RELEASE, &[])?;
        call_kept(&mut c, a, l, DD, DP_CLOSE)?;
        finish(a, l, 1)
    });
    let mut cpu = card_machine("directplay_cocreate", None, program);
    run_until(&mut cpu, data, 1);
    assert_eq!(
        results(&cpu, data, 24),
        [
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            E_NOINTERFACE,
            0,
            DPERR_INVALIDPARAMS,
            DPERR_UNINITIALIZED,
            DPERR_UNINITIALIZED,
            DPERR_UNAVAILABLE,
            DPERR_UNINITIALIZED,
            DPERR_NOMESSAGES,
            DPERR_INVALIDFLAGS,
            0,
            0,
            0,
            0,
            1,
            0,
            0,
            DPERR_INVALIDOBJECT
        ]
    );
    let word = |offset| data_word(&cpu, data, offset);
    let interfaces = [DD, FRONT, DP_PLAY4, DP_PLAY2A, DP_UNKNOWN1];
    for (i, &one) in interfaces.iter().enumerate() {
        assert!(
            word(one) != 0 && word(one) != 0x1234_5678,
            "{:X}",
            word(one)
        );
        for &other in &interfaces[i + 1..] {
            assert_ne!(word(one), word(other), "{one:X} and {other:X}");
        }
    }
    assert_eq!(word(DP_UNKNOWN1), word(DP_UNKNOWN2), "COM identity");
    assert_eq!(word(DP_NONE), 0);
    assert_eq!(word(DP_CALLED), 0, "callbacks called");
}

/// DirectPlayCreate with GUID_NULL makes an IDirectPlay with no service
/// provider, which has IDirectPlay3A too and keeps its interfaces'
/// counts apart. With any other provider it fails, as there is none,
/// and an outer object is refused before the arguments are looked at.
/// There are no providers to enumerate.
#[test]
fn directplaycreate_makes_a_directplay_with_no_service_provider() {
    let dplayx: (&str, &[&str]) = ("DPLAYX.DLL", &["DirectPlayCreate", "DirectPlayEnumerateA"]);
    let (program, data) = program(&[dplayx], &directplay_data(), |a, l| {
        let mut c = Calls { next: 0 };
        let none = Arg::I(0);
        let null = Arg::At(DP_GUID_NULL);
        c.api(a, l, 0, &[null, Arg::At(DD), none])?;
        c.api(a, l, 0, &[Arg::At(DP_CLSID), Arg::At(FRONT), none])?;
        c.api(a, l, 0, &[none, Arg::At(BACK), none])?;
        c.api(a, l, 0, &[none, Arg::At(BACK), Arg::I(1)])?;
        let query = [Arg::At(DP_IID3A), Arg::At(TARGET)];
        c.com(a, l, DD, DP_QUERY_INTERFACE, &query)?;
        // A callback that isn't code, which mustn't be called.
        let session = Arg::At(DP_SESSION);
        let enum_sessions = [session, none, Arg::I(BASE), none, none];
        c.com(a, l, TARGET, DP_ENUM_SESSIONS, &enum_sessions)?;
        c.com(a, l, DD, DP1_ENUM_SESSIONS, &enum_sessions)?;
        c.com(a, l, DD, DP1_INITIALIZE, &[null])?;
        c.com(a, l, TARGET, DP_RELEASE, &[])?;
        // The IDirectPlay is still there.
        c.com(a, l, DD, DP_CLOSE, &[])?;
        c.com(a, l, DD, DP_RELEASE, &[])?;
        c.api(a, l, 1, &[Arg::I(BASE), none])?;
        finish(a, l, 1)
    });
    let mut cpu = card_machine("directplay_create", None, program);
    run_until(&mut cpu, data, 1);
    assert_eq!(
        results(&cpu, data, 12),
        [
            0,
            DPERR_UNAVAILABLE,
            DPERR_INVALIDPARAMS,
            CLASS_E_NOAGGREGATION,
            0,
            DPERR_UNINITIALIZED,
            DPERR_UNINITIALIZED,
            DPERR_ALREADYINITIALIZED,
            0,
            0,
            0,
            0
        ]
    );
    let this = peek(&cpu, data + DD);
    assert!(this != 0 && this != 0x1234_5678, "{this:X}");
    assert_ne!(peek(&cpu, data + TARGET), this);
    assert_eq!(peek(&cpu, data + FRONT), 0);
    assert_eq!(peek(&cpu, data + BACK), 0x1234_5678, "BACK written");
}

/// A DirectPlay object is in the save state: loaded, the program goes
/// on with its interfaces and their counts.
#[test]
fn a_directplay_object_is_kept_in_a_saved_state() {
    let dplayx: (&str, &[&str]) = ("DPLAYX.DLL", &["DirectPlayCreate"]);
    let (program, data) = program(&[dplayx], &directplay_data(), |a, l| {
        let mut c = Calls { next: 0 };
        let none = Arg::I(0);
        c.api(a, l, 0, &[Arg::At(DP_GUID_NULL), Arg::At(DD), none])?;
        let query = [Arg::At(DP_IID4A), Arg::At(FRONT)];
        c.com(a, l, DD, DP_QUERY_INTERFACE, &query)?;
        wait(a, l, 1)?;
        c.com(a, l, FRONT, DP_ADD_REF, &[])?;
        let enum_sessions = [Arg::At(DP_SESSION), none, Arg::I(BASE), none, none];
        c.com(a, l, FRONT, DP_ENUM_SESSIONS, &enum_sessions)?;
        c.com(a, l, FRONT, DP_RELEASE, &[])?;
        c.com(a, l, FRONT, DP_RELEASE, &[])?;
        c.com(a, l, DD, DP_RELEASE, &[])?;
        finish(a, l, 2)
    });
    let mut cpu = card_machine("directplay_state", None, program);
    run_until(&mut cpu, data, 1);
    assert_eq!(results(&cpu, data, 2), [0, 0]);
    let state = rust_dos::savestate::machine::save(&cpu);
    let after = [2, DPERR_UNINITIALIZED, 1, 0, 0];
    go_on(&mut cpu, data);
    run_until(&mut cpu, data, 2);
    assert_eq!(results(&cpu, data, 7)[2..], after, "before the load");
    rust_dos::savestate::machine::load(&mut cpu, &state).unwrap();
    assert_eq!(results(&cpu, data, 7)[2..], [0; 5]);
    go_on(&mut cpu, data);
    run_until(&mut cpu, data, 2);
    assert_eq!(results(&cpu, data, 7)[2..], after, "after the load");
}

/// The lobby, made both ways: no lobby launched the program, so it has
/// no connection settings, and no application is registered. It builds
/// an address, the size first, enumerates it back chunk by chunk, and
/// lists the address types of the serial and TCP/IP providers.
#[test]
fn the_lobby_builds_addresses_and_is_not_lobbied() {
    let dplayx: (&str, &[&str]) = ("DPLAYX.DLL", &["DirectPlayLobbyCreateA"]);
    let ole32: (&str, &[&str]) = ("OLE32.DLL", &["CoInitialize", "CoCreateInstance"]);
    let (program, data) = program(&[dplayx, ole32], &directplay_data(), |a, l| {
        chunk_callback(a, l, 0)?;
        counting_callback(a, l, 1, 3, DP_TYPES)?;
        counting_callback(a, l, 2, 3, DP_APPS)?;
        let mut c = Calls { next: 0 };
        let none = Arg::I(0);
        c.api(a, l, 0, &[none, Arg::At(DP_LOBBY), none, none, none])?;
        c.api(a, l, 0, &[none, Arg::At(DP_NONE), Arg::I(1), none, none])?;
        c.api(a, l, 1, &[none])?;
        let create = [
            Arg::At(DPL_CLSID),
            none,
            Arg::I(1),
            Arg::At(DPL_IID3A),
            Arg::At(DP_LOBBY3A),
        ];
        c.api(a, l, 2, &create)?;
        let settings = [none, none, Arg::At(DP_SIZE)];
        c.com(a, l, DP_LOBBY, DPL_GET_CONNECTION_SETTINGS, &settings)?;
        c.com(a, l, DP_LOBBY3A, DPL_GET_CONNECTION_SETTINGS, &settings)?;
        let address = |buffer| {
            [
                Arg::At(DP_TCPIP),
                Arg::At(DP_INET),
                Arg::At(DP_HOST),
                Arg::I(9),
                buffer,
                Arg::At(DP_ADDRESS_SIZE),
            ]
        };
        c.com(a, l, DP_LOBBY3A, DPL_CREATE_ADDRESS, &address(none))?;
        let buffer = Arg::At(DP_ADDRESS);
        c.com(a, l, DP_LOBBY3A, DPL_CREATE_ADDRESS, &address(buffer))?;
        let chunks = [Arg::Val(CALLBACKS), buffer, Arg::Val(DP_ADDRESS_SIZE), none];
        c.com(a, l, DP_LOBBY3A, DPL_ENUM_ADDRESS, &chunks)?;
        let types = |provider| [Arg::Val(CALLBACKS + 4), Arg::At(provider), none, none];
        c.com(a, l, DP_LOBBY3A, DPL_ENUM_ADDRESS_TYPES, &types(DP_SERIAL))?;
        c.com(a, l, DP_LOBBY3A, DPL_ENUM_ADDRESS_TYPES, &types(DP_TCPIP))?;
        let apps = [Arg::Val(CALLBACKS + 8), none, none];
        c.com(a, l, DP_LOBBY, DPL_ENUM_LOCAL_APPLICATIONS, &apps)?;
        c.com(a, l, DP_LOBBY3A, DP_RELEASE, &[])?;
        c.com(a, l, DP_LOBBY, DP_RELEASE, &[])?;
        finish(a, l, 1)
    });
    let mut cpu = card_machine("directplay_lobby", None, program);
    run_until(&mut cpu, data, 1);
    assert_eq!(
        results(&cpu, data, 14),
        [
            0,
            CLASS_E_NOAGGREGATION,
            0,
            0,
            DPERR_NOTLOBBIED,
            DPERR_NOTLOBBIED,
            DPERR_BUFFERTOOSMALL,
            0,
            0,
            0,
            0,
            0,
            0,
            0
        ]
    );
    let word = |offset| data_word(&cpu, data, offset);
    assert_eq!(word(DP_NONE), 0);
    // DPAID_TotalSize, 4 bytes: 89; DPAID_ServiceProvider, 16 bytes:
    // TCP/IP's GUID; DPAID_INet, 9 bytes: the host.
    let size = 24 + 36 + 29;
    assert_eq!(word(DP_ADDRESS_SIZE), size);
    let address = [
        (0, 0x1318_F560),
        (16, 4),
        (20, size),
        (24, 0x07D9_16C0),
        (40, 16),
        (44, 0x36E9_5EE0),
        (60, 0xC4A5_4DA0),
        (76, 9),
        (80, u32::from_le_bytes(*b"10.0")),
    ];
    for (offset, value) in address {
        assert_eq!(word(DP_ADDRESS + offset), value, "the address at {offset}");
    }
    assert_eq!(word(DP_CALLED), 3, "chunks");
    let sizes: Vec<u32> = (0..3).map(|i| word(DP_SIZES + 4 * i)).collect();
    assert_eq!(sizes, [4, 16, 9]);
    // DPAID_ComPort; DPAID_INet, INetW and INetPort.
    assert_eq!(word(DP_TYPES), 4, "address types");
    assert_eq!(word(DP_APPS), 0, "applications");
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

const USER32_FIRST_CURSOR: (&str, &[&str]) = (
    "USER32.DLL",
    &[
        "CreateWindowExA",
        "GetCursor",
        "LoadCursorA",
        "SetCursor",
        "SetCursorPos",
    ],
);
const FIRST_GET_CURSOR: usize = 1;
const FIRST_LOAD_CURSOR: usize = 2;
const FIRST_SET_CURSOR: usize = 3;
const FIRST_SET_CURSOR_POS: usize = 4;

/// CreateWindowExA of a visible 64 x 64 popup at the screen's corner.
fn popup_window(c: &mut Calls, a: &mut CodeAssembler, l: &Layout) -> Result<(), IcedError> {
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
    )
}

/// Windows starts with the arrow as the cursor: a program that never
/// calls SetCursor gets the arrow's handle from GetCursor, its first
/// call, and the arrow shows where the mouse is.
#[test]
fn the_arrow_is_the_cursor_from_the_start() {
    let (program, data) = program(&[USER32_FIRST_CURSOR], &data(), |a, l| {
        let mut c = Calls { next: 0 };
        c.api(a, l, FIRST_GET_CURSOR, &[])?;
        popup_window(&mut c, a, l)?;
        c.api(a, l, FIRST_LOAD_CURSOR, &[Arg::I(0), Arg::I(IDC_ARROW)])?;
        c.api(a, l, FIRST_SET_CURSOR_POS, &[Arg::I(100), Arg::I(100)])?;
        finish(a, l, 1)
    });
    let mut cpu = card_machine("cursor_first", None, program);
    cpu.bus.win32.depth = rust_dos::config::DesktopDepth::Palette;
    run_until(&mut cpu, data, 1);
    let r = results(&cpu, data, 4);
    assert_ne!(r[0], 0, "GetCursor");
    assert_ne!(r[1], 0, "CreateWindowExA");
    assert_eq!(
        r[0], r[2],
        "GetCursor gives LoadCursorA(IDC_ARROW)'s cursor"
    );
    assert_eq!(r[3], 1, "SetCursorPos");
    let shown = with_cursors(&cpu, &[(100, 100), (101, 102)]);
    assert_eq!(shown[0], [0, 0, 0], "the arrow's tip");
    assert_eq!(shown[1], [0xFF, 0xFF, 0xFF], "the arrow's inside");
}

/// SetCursor(NULL) as a program's first call replaces the arrow USER
/// starts with: it returns the arrow, and the windows made after it
/// don't bring the arrow back.
#[test]
fn set_cursor_null_first_hides_the_arrow() {
    let (program, data) = program(&[USER32_FIRST_CURSOR], &data(), |a, l| {
        let mut c = Calls { next: 0 };
        c.api(a, l, FIRST_SET_CURSOR, &[Arg::I(0)])?;
        popup_window(&mut c, a, l)?;
        c.api(a, l, FIRST_GET_CURSOR, &[])?;
        c.api(a, l, FIRST_LOAD_CURSOR, &[Arg::I(0), Arg::I(IDC_ARROW)])?;
        c.api(a, l, FIRST_SET_CURSOR_POS, &[Arg::I(100), Arg::I(100)])?;
        finish(a, l, 1)
    });
    let mut cpu = card_machine("cursor_null_first", None, program);
    cpu.bus.win32.depth = rust_dos::config::DesktopDepth::Palette;
    run_until(&mut cpu, data, 1);
    let r = results(&cpu, data, 5);
    assert_eq!(r[0], r[3], "SetCursor(NULL) gives the arrow it replaced");
    assert_ne!(r[0], 0, "SetCursor(NULL)");
    assert_ne!(r[1], 0, "CreateWindowExA");
    assert_eq!(r[2], 0, "GetCursor after SetCursor(NULL)");
    assert_eq!(r[4], 1, "SetCursorPos");
    let shown = with_cursors(&cpu, &[(101, 102)]);
    assert_ne!(shown[0], [0xFF, 0xFF, 0xFF], "no arrow over the desktop");
}

const USER32_CLASS_CURSOR: (&str, &[&str]) = (
    "USER32.DLL",
    &[
        "CreateWindowExA",
        "GetCursor",
        "LoadCursorA",
        "SetCursorPos",
        "PeekMessageA",
    ],
);
const CLASS_GET_CURSOR: usize = 1;
const CLASS_LOAD_CURSOR: usize = 2;
const CLASS_SET_CURSOR_POS: usize = 3;
const CLASS_PEEK_MESSAGE: usize = 4;
const IDC_IBEAM: u32 = 32513;
const WM_MOUSEMOVE: u32 = 0x200;
const PM_REMOVE: u32 = 1;
/// The MSG PeekMessageA fills.
const CURSOR_MSG: u32 = 0xF00;

/// As the thread takes a mouse message, Windows sends the window under
/// the mouse WM_SETCURSOR, and DefWindowProc sets its class's cursor: an
/// Edit control's I-beam, which replaces the arrow.
#[test]
fn a_mouse_message_sets_the_class_cursor() {
    let mut d = data();
    put(&mut d, CLASS, b"Edit\0");
    let (program, data) = program(&[USER32_CLASS_CURSOR], &d, |a, l| {
        let mut c = Calls { next: 0 };
        popup_window(&mut c, a, l)?;
        c.api(a, l, CLASS_SET_CURSOR_POS, &[Arg::I(20), Arg::I(20)])?;
        c.api(a, l, CLASS_GET_CURSOR, &[])?;
        let mv = Arg::I(WM_MOUSEMOVE);
        let peek = [Arg::At(CURSOR_MSG), Arg::I(0), mv, mv, Arg::I(PM_REMOVE)];
        c.api(a, l, CLASS_PEEK_MESSAGE, &peek)?;
        c.api(a, l, CLASS_GET_CURSOR, &[])?;
        c.api(a, l, CLASS_LOAD_CURSOR, &[Arg::I(0), Arg::I(IDC_IBEAM)])?;
        c.api(a, l, CLASS_LOAD_CURSOR, &[Arg::I(0), Arg::I(IDC_ARROW)])?;
        finish(a, l, 1)
    });
    let mut cpu = card_machine("cursor_class", None, program);
    run_until(&mut cpu, data, 1);
    let r = results(&cpu, data, 7);
    assert_ne!(r[0], 0, "CreateWindowExA");
    assert_eq!(r[1], 1, "SetCursorPos");
    assert_eq!(r[2], r[6], "the arrow before the message");
    assert_eq!(r[3], 1, "PeekMessageA finds the WM_MOUSEMOVE");
    assert_eq!(r[4], r[5], "the I-beam after it");
    assert_ne!(r[4], r[6]);
}

const USER32_FIRST_MOVE: (&str, &[&str]) = (
    "USER32.DLL",
    &[
        "CreateWindowExA",
        "GetCursor",
        "LoadCursorA",
        "GetCursorPos",
        "PeekMessageA",
    ],
);
const MOVE_GET_CURSOR_POS: usize = 3;
/// The POINT GetCursorPos fills.
const CURSOR_POINT: u32 = 0xF40;

/// A program's first window, shown full screen, brings the desktop on
/// the screen with the cursor in its middle, over the window: Windows
/// marks the mouse moved as SetWindowPos shows it, so with the mouse
/// standing still the window gets a WM_MOUSEMOVE where the cursor is,
/// and the WM_SETCURSOR that comes with it sets the class's cursor. A
/// screen saver started from the prompt hides the cursor this way. The
/// cursor was there before the desktop came, so a saver that kept its
/// place before showing its window doesn't take the move for the mouse
/// moving.
#[test]
fn the_first_window_shown_under_the_cursor_gets_a_mouse_move() {
    let mut d = data();
    put(&mut d, CLASS, b"Edit\0");
    let (program, data) = program(&[USER32_FIRST_MOVE], &d, |a, l| {
        let mut c = Calls { next: 0 };
        c.api(a, l, MOVE_GET_CURSOR_POS, &[Arg::At(CURSOR_POINT)])?;
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
        c.api(a, l, CLASS_GET_CURSOR, &[])?;
        let mv = Arg::I(WM_MOUSEMOVE);
        let peek = [Arg::At(CURSOR_MSG), Arg::I(0), mv, mv, Arg::I(PM_REMOVE)];
        c.api(a, l, CLASS_PEEK_MESSAGE, &peek)?;
        c.api(a, l, CLASS_GET_CURSOR, &[])?;
        c.api(a, l, CLASS_LOAD_CURSOR, &[Arg::I(0), Arg::I(IDC_IBEAM)])?;
        c.api(a, l, CLASS_LOAD_CURSOR, &[Arg::I(0), Arg::I(IDC_ARROW)])?;
        finish(a, l, 1)
    });
    let mut cpu = card_machine("cursor_first_window_move", None, program);
    run_until(&mut cpu, data, 1);
    let r = results(&cpu, data, 7);
    assert_eq!(r[0], 1, "GetCursorPos");
    assert_ne!(r[1], 0, "CreateWindowExA");
    assert_eq!(r[2], r[6], "the arrow before the message");
    assert_eq!(r[3], 1, "PeekMessageA finds a WM_MOUSEMOVE");
    let msg = data + CURSOR_MSG;
    assert_eq!(peek(&cpu, msg), r[1], "for the window");
    let (x, y) = (cpu.bus.mouse.x as u32, cpu.bus.mouse.y as u32);
    assert_eq!(peek(&cpu, msg + 12), y << 16 | x, "where the cursor is");
    let point = data + CURSOR_POINT;
    let before = (peek(&cpu, point), peek(&cpu, point + 4));
    assert_eq!(before, (x, y), "where it was before the desktop");
    assert_eq!(r[4], r[5], "the I-beam after it");
    assert_ne!(r[4], r[6]);
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

/// `mode_cursor_program(w, h)` run to its step 1 on `board`, the mode
/// set: a Voodoo3 shows it on the card, the others on the VGA.
fn small_mode_machine(test: &str, board: Option<Board>, (w, h): (u32, u32)) -> (Cpu, u32) {
    let (program, data) = mode_cursor_program(w, h);
    let mut cpu = if board == Some(Board::Voodoo3) {
        machine(test, program)
    } else {
        card_machine(test, board, program)
    };
    run_until(&mut cpu, data, 1);
    let r = results(&cpu, data, 4);
    assert_eq!(r[2..4], [0, 0], "SetCooperativeLevel, SetDisplayMode");
    (cpu, data)
}

/// Over a DirectDraw mode of 320 pixels across, the cursor is in the
/// mode's pixels, as large as they show: the arrow's inside, one pixel
/// right of the tip and two down, covers that pixel of the mode on the
/// frame, however much larger the frame shows it.
fn the_cursor_shows_in_a_small_mode(test: &str, board: Option<Board>, (w, h): (u32, u32)) {
    let (cpu, data) = small_mode_machine(test, board, (w, h));
    let r = results(&cpu, data, 7);
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

/// A Voodoo3 shows 320x200 at twice its size both ways: its lines
/// doubled, and its pixels at half the pixel clock.
#[test]
fn the_cursor_shows_in_a_320_by_200_voodoo3_mode() {
    the_cursor_shows_in_a_small_mode("cursor_voodoo3_320", Some(Board::Voodoo3), (320, 200));
}

/// Over a DirectDraw mode of 320 pixels across, the host's pointer
/// spans the mode's pixels, whatever size the frame shows them at: the
/// middle of the picture is the middle of the screen, its bottom-right
/// corner the screen's, and a captured mouse moves the cursor as far as
/// the picture moves under it.
fn the_pointer_spans_a_small_mode(test: &str, board: Option<Board>, (w, h): (u32, u32)) {
    let (cpu, _) = small_mode_machine(test, board, (w, h));
    let (frame_w, frame_h) = video::frame_size(&cpu.bus);
    let frame = Frame::new(frame_w, frame_h);
    let (fw, fh) = (frame_w as i32, frame_h as i32);
    let (w, h) = (w as i32, h as i32);
    let to_mouse = |at| video::overlay::frame_to_mouse(&cpu.bus, &frame, at);
    assert_eq!(to_mouse((fw / 2, fh / 2)), (w / 2, h / 2), "the middle");
    assert_eq!(
        to_mouse((fw - 1, fh - 1)),
        (w - 1, h - 1),
        "the bottom-right corner"
    );
    let (sx, sy) = ((frame_w / w as u32) as f64, (frame_h / h as u32) as f64);
    let motion = video::overlay::frame_motion_to_mouse(&cpu.bus, &frame, (sx * 10.0, sy * 10.0));
    assert_eq!(motion, (10.0, 10.0), "ten of the mode's pixels");
}

#[test]
fn the_pointer_spans_a_320_by_240_vesa_mode() {
    the_pointer_spans_a_small_mode("pointer_vesa_320", None, (320, 240));
}

#[test]
fn the_pointer_spans_a_320_by_200_voodoo3_mode() {
    the_pointer_spans_a_small_mode("pointer_voodoo3_320", Some(Board::Voodoo3), (320, 200));
}
