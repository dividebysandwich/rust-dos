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

/// Full screen: a 320x200 8-bit mode, a flip chain, a pixel written to
/// the back buffer through Lock and a flip that shows it.
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
/// DirectDrawCreate, after SCREEN_USER32's.
const S_DIRECT_DRAW_CREATE: usize = 9;

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
    (w, h): (u32, u32),
) -> Result<(), IcedError> {
    c.api(
        a,
        l,
        S_CREATE_WINDOW,
        &[
            Arg::I(0),
            Arg::At(CLASS),
            Arg::At(CLASS),
            Arg::I(WS_POPUP | WS_VISIBLE),
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
/// through IDirectDraw2 with `v2`, else through IDirectDraw.
fn leave_exclusive_program(v2: bool) -> (Vec<u8>, u32) {
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
        let normal = Arg::I(DDSCL_NORMAL);
        c.com(a, l, dd, DD_SET_COOPERATIVE_LEVEL, &[Arg::Val(HWND), normal])?;
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
    let (program, data) = leave_exclusive_program(true);
    let mut cpu = machine("leave_exclusive_v2", program);
    run_until_done(&mut cpu, data);
    assert_eq!(results(&mut cpu, data, 9)[1..], [0; 8]);
    // The desktop's mode is back, not the program's 8 bits.
    let bpp = cpu.bus.vbe.mode.map(|m| m.bpp);
    assert!(bpp.is_some_and(|b| b > 8), "{bpp:?}");
}

#[test]
fn leaving_exclusive_mode_through_idirectdraw_keeps_the_mode_with_gdis_palette() {
    let (program, data) = leave_exclusive_program(false);
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
