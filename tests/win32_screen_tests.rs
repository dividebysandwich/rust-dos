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
    assert_eq!(card_mode(&cpu), (640, 480, 1, 1280));
    assert_eq!(card_reg(&cpu, VID_DESKTOP_START_ADDR), 0);
    assert!(desktop(&cpu) == before, "the desktop as it was");
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
const DDSCAPS_SYSTEMMEMORY: u32 = 0x800;
const DDBLT_COLORFILL: u32 = 0x400;
const DDBLT_WAIT: u32 = 0x100_0000;
const DDFLIP_WAIT: u32 = 1;
const DDERR_SURFACELOST: u32 = 0x8876_01C2;
const DDERR_INVALIDDIRECTDRAWGUID: u32 = 0x8876_0231;

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
    assert_eq!(r[1], DDERR_INVALIDDIRECTDRAWGUID, "a GUID of zeros");
    assert_eq!(r[4..], [0; 3], "the display's object");
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
    let r = results(&cpu, data, 25);
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
    go_on(&mut cpu, data);
    run_until(&mut cpu, data, 2);
    let r = results(&cpu, data, 27);
    assert_eq!(r[25..], [0, DDERR_SURFACELOST]);
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
