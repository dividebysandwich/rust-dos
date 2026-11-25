//! Windows 3.1's GDI heap (`Shim::Win31GdiHeap`, src/win32/shims/win31):
//! a program with the shim finds its bitmaps through TOOLHELP as it did
//! on Windows 3.1, and BitBlt draws from the device BITMAP it changed, as
//! Lunicus's LUNIFX.DLL does. A program without it finds no GDI heap,
//! and the log says once which shim it may need. The programs are made
//! byte by byte (tests/common/win16) and end with the number of the
//! first check that failed, 0 when all held.

use crate::win16;

use rust_dos::config::DesktopDepth;
use rust_dos::win32::shims;
use win16::{CODE, Code, program, run, run_with};

/// KERNEL's, GDI's and TOOLHELP's ordinals the programs call.
const GLOBAL_LOCK: u16 = 18;
const BIT_BLT: u16 = 34;
const SELECT_OBJECT: u16 = 45;
const CREATE_BITMAP: u16 = 48;
const CREATE_COMPATIBLE_DC: u16 = 52;
const DELETE_DC: u16 = 68;
const DELETE_OBJECT: u16 = 69;
const GET_BITMAP_BITS: u16 = 74;
const GLOBAL_HANDLE_TO_SEL: u16 = 50;
const SYSTEM_HEAP_INFO: u16 = 71;
const MEMORY_READ: u16 = 78;

/// Where the programs keep things in their data segment: the bitmap and
/// DC read from and drawn into, GDI's selector, the first hGDISegment,
/// the offset MemoryRead reads at and what it read, a SYSHEAPINFO, the
/// pixels the device BITMAP is pointed at and those GetBitmapBits gave.
const BITMAP: u16 = 0x10;
const DC: u16 = 0x12;
const TARGET: u16 = 0x14;
const TARGET_DC: u16 = 0x16;
const GDI_SEL: u16 = 0x18;
const FIRST: u16 = 0x1A;
const OFFSET: u16 = 0x1C;
const READ: u16 = 0x20;
const INFO: u16 = 0x30;
const H_GDI_SEGMENT: u16 = INFO + 0x0A;
const PIXELS: u16 = 0x40;
const GOT: u16 = 0x60;

/// The device BITMAP's fields LUNIFX.DLL writes.
const BM_WIDTH: u8 = 0x02;
const BM_HEIGHT: u8 = 0x04;
const BM_WIDTH_BYTES: u8 = 0x06;
const BM_BITS: u8 = 0x0A;

/// The size of the picture BitBlt copies.
const WIDTH: u16 = 8;
const HEIGHT: u16 = 2;

fn lo(v: u16) -> u8 {
    v as u8
}

fn hi(v: u16) -> u8 {
    (v >> 8) as u8
}

/// mov word [at],value.
fn set(code: &mut Code, at: u16, value: u16) {
    code.emit(&[0xC7, 0x06, lo(at), hi(at), lo(value), hi(value)]);
}

/// mov [at],ax.
fn store_ax(code: &mut Code, at: u16) {
    code.emit(&[0xA3, lo(at), hi(at)]);
}

/// push word [at].
fn push_word(code: &mut Code, at: u16) {
    code.emit(&[0xFF, 0x36, lo(at), hi(at)]);
}

/// push ds; push `at`: a far pointer into the data segment.
fn push_data(code: &mut Code, at: u16) {
    code.emit(&[0x1E]).push(at);
}

/// End with `fail` unless the last comparison found them equal.
fn or_fail(code: &mut Code, fail: u8) {
    code.emit(&[0x74, 0x06]) // je over the exit
        .emit(&[0xB0, fail, 0xB4, 0x4C, 0xCD, 0x21]);
}

/// End with `fail` unless AX is `value`.
fn expect_ax(code: &mut Code, value: u16, fail: u8) {
    code.emit(&[0x3D, lo(value), hi(value)]);
    or_fail(code, fail);
}

/// End with `fail` unless the word at `at` is `value`.
fn expect(code: &mut Code, at: u16, value: u16, fail: u8) {
    code.emit(&[0x81, 0x3E, lo(at), hi(at), lo(value), hi(value)]);
    or_fail(code, fail);
}

/// End with `fail` unless the word at ES:BX+`field` is `value`.
fn expect_field(code: &mut Code, field: u8, value: u16, fail: u8) {
    code.emit(&[0x26, 0x81, 0x7F, field, lo(value), hi(value)]);
    or_fail(code, fail);
}

/// mov word es:[bx+field],value.
fn set_field(code: &mut Code, field: u8, value: u16) {
    code.emit(&[0x26, 0xC7, 0x47, field, lo(value), hi(value)]);
}

/// SystemHeapInfo(ds:INFO), its dwSize set, which must give TRUE.
fn system_heap_info(code: &mut Code, fail: u8) {
    set(code, INFO, 0x0C);
    set(code, INFO + 2, 0);
    push_data(code, INFO);
    code.call("TOOLHELP", SYSTEM_HEAP_INFO);
    expect_ax(code, 1, fail);
}

/// MemoryRead(word [GDI_SEL], the word at `from` plus `add`, ds:READ,
/// `len`): the bytes at that offset of GDI's segment, the offset a dword
/// as LUNIFX.DLL works it out.
fn read_gdi(code: &mut Code, from: u16, add: u16, len: u16) {
    code.emit(&[0xA1, lo(from), hi(from)]); // mov ax,[from]
    store_ax(code, OFFSET);
    set(code, OFFSET + 2, 0);
    push_word(code, GDI_SEL);
    code.emit(&[0xA1, lo(OFFSET), hi(OFFSET)]) // mov ax,[OFFSET]
        .emit(&[0x8B, 0x16, lo(OFFSET + 2), hi(OFFSET + 2)]) // mov dx,[OFFSET+2]
        .emit(&[0x05, lo(add), hi(add)]) // add ax,add
        .emit(&[0x83, 0xD2, 0x00]) // adc dx,0
        .emit(&[0x52, 0x50]); // push dx; push ax
    push_data(code, READ);
    code.push(0).push(len).call("TOOLHELP", MEMORY_READ);
}

/// CreateBitmap(`width`, `height`, 1, 8, NULL) into `at`, and a memory
/// DC it is selected into into `dc`.
fn bitmap_in_dc(code: &mut Code, width: u16, height: u16, at: u16, dc: u16) {
    code.push(width)
        .push(height)
        .push(1)
        .push(8)
        .push(0)
        .push(0)
        .call("GDI", CREATE_BITMAP);
    store_ax(code, at);
    code.push(0).call("GDI", CREATE_COMPATIBLE_DC);
    store_ax(code, dc);
    push_word(code, dc);
    push_word(code, at);
    code.call("GDI", SELECT_OBJECT);
}

/// A program that does what LUNIFX.DLL does: it makes a bitmap of 4 by 1
/// and selects it, finds its device BITMAP through GDI's heap (the
/// handle table entry at the handle's offset, the object's global handle
/// at +0Ah of the object), sets its size to 8 by 2 and points bmBits at
/// pixels of its own, and BitBlts it into a bitmap of 8 by 2, whose bits
/// must then be those pixels. Then it deletes the bitmap, whose handle
/// table entry is then 0. Without GDI's heap it ends with 20h.
fn reads_gdis_heap() -> Code {
    let mut code = Code::new();
    for i in 0..(WIDTH * HEIGHT / 2) {
        set(&mut code, PIXELS + 2 * i, 0x0201 + 0x0202 * i);
    }
    bitmap_in_dc(&mut code, 4, 1, BITMAP, DC);
    bitmap_in_dc(&mut code, WIDTH, HEIGHT, TARGET, TARGET_DC);

    // Two calls, which give the same segment.
    system_heap_info(&mut code, 1);
    code.emit(&[0xA1, lo(H_GDI_SEGMENT), hi(H_GDI_SEGMENT)]); // mov ax,[hGDISegment]
    store_ax(&mut code, FIRST);
    system_heap_info(&mut code, 2);
    code.emit(&[0xA1, lo(H_GDI_SEGMENT), hi(H_GDI_SEGMENT)]) // mov ax,[hGDISegment]
        .emit(&[0x3B, 0x06, lo(FIRST), hi(FIRST)]); // cmp ax,[FIRST]
    or_fail(&mut code, 3);
    code.emit(&[0x83, 0x3E, lo(H_GDI_SEGMENT), hi(H_GDI_SEGMENT), 0x00]) // cmp word [hGDISegment],0
        .emit(&[0x75, 0x06]) // jne over the exit
        .emit(&[0xB0, 0x20, 0xB4, 0x4C, 0xCD, 0x21]);
    push_word(&mut code, H_GDI_SEGMENT);
    code.call("TOOLHELP", GLOBAL_HANDLE_TO_SEL);
    store_ax(&mut code, GDI_SEL);

    // The handle table entry: the object's offset, unlocked.
    read_gdi(&mut code, BITMAP, 0, 4);
    expect_ax(&mut code, 4, 4);
    expect(&mut code, READ + 2, 0, 5);
    // The object: a bitmap's magic, and its device BITMAP's handle.
    code.emit(&[0xA1, lo(READ), hi(READ)]); // mov ax,[READ]
    store_ax(&mut code, READ + 4);
    read_gdi(&mut code, READ + 4, 2, 2);
    expect(&mut code, READ, 0x4F4B, 6);
    read_gdi(&mut code, READ + 4, 0x0A, 2);
    expect_ax(&mut code, 2, 7);

    // The device BITMAP: what CreateBitmap made, and then LUNIFX's.
    push_word(&mut code, READ);
    code.call("KERNEL", GLOBAL_LOCK);
    code.emit(&[0x8E, 0xC2, 0x89, 0xC3]); // mov es,dx; mov bx,ax
    expect_field(&mut code, BM_WIDTH, 4, 8);
    expect_field(&mut code, BM_HEIGHT, 1, 9);
    expect_field(&mut code, BM_WIDTH_BYTES, 4, 10);
    expect_field(&mut code, 0x08, 0x0801, 11);
    set_field(&mut code, BM_WIDTH, WIDTH);
    set_field(&mut code, BM_HEIGHT, HEIGHT);
    set_field(&mut code, BM_WIDTH_BYTES, WIDTH);
    set_field(&mut code, BM_BITS, PIXELS);
    code.emit(&[0x26, 0x8C, 0x5F, BM_BITS + 2]); // mov es:[bx+0Ch],ds

    // BitBlt(target, 0, 0, 8, 2, dc, 0, 0, SRCCOPY).
    push_word(&mut code, TARGET_DC);
    code.push(0).push(0).push(WIDTH).push(HEIGHT);
    push_word(&mut code, DC);
    code.push(0).push(0).push(0x00CC).push(0x0020);
    code.call("GDI", BIT_BLT);
    expect_ax(&mut code, 1, 12);
    push_word(&mut code, TARGET);
    code.push(0).push(WIDTH * HEIGHT);
    push_data(&mut code, GOT);
    code.call("GDI", GET_BITMAP_BITS);
    expect_ax(&mut code, WIDTH * HEIGHT, 13);
    for i in 0..(WIDTH * HEIGHT / 2) {
        code.emit(&[0xA1, lo(GOT + 2 * i), hi(GOT + 2 * i)]) // mov ax,[GOT+i]
            .emit(&[0x3B, 0x06, lo(PIXELS + 2 * i), hi(PIXELS + 2 * i)]); // cmp ax,[PIXELS+i]
        or_fail(&mut code, 14 + i as u8);
    }

    // Deleted, the bitmap leaves GDI's heap.
    push_word(&mut code, DC);
    code.call("GDI", DELETE_DC);
    push_word(&mut code, BITMAP);
    code.call("GDI", DELETE_OBJECT);
    expect_ax(&mut code, 1, 30);
    read_gdi(&mut code, BITMAP, 0, 4);
    expect(&mut code, READ, 0, 31);
    code.emit(&[0xB0, 0x00]).exit_al();
    code
}

/// The log's hint for a program without the shim.
const HINT: &str = "asked TOOLHELP for GDI's heap";

/// With Windows 3.1's shims (`win32_shims=win31`), the program finds its
/// bitmap in GDI's heap, and BitBlt draws what the device BITMAP it
/// changed says, as on Windows 3.1. The desktop has 256 colours, as
/// Lunicus is run with: a memory DC takes a bitmap of 8 bits a pixel
/// only on a display of that depth.
#[test]
fn a_program_with_the_shim_reads_and_changes_gdis_heap() {
    let (code, log) = run_with(
        "gdi_heap_shim",
        program(&reads_gdis_heap(), CODE),
        |cpu, _| {
            cpu.bus.win32.shims = shims::win31::WIN31;
            cpu.bus.win32.depth = DesktopDepth::Palette;
        },
    );
    assert_eq!(code, 0, "{log}");
    assert!(!log.contains(HINT), "{log}");
}

/// Without the shim there is no GDI heap, as on Windows 98's GDI32, and
/// the log has a hint naming the program and the shim, once.
#[test]
fn a_program_without_the_shim_gets_no_gdi_heap_and_a_hint() {
    let (code, log) = run("gdi_heap_none", program(&reads_gdis_heap(), CODE));
    assert_eq!(code, 0x20, "{log}");
    assert_eq!(log.matches(HINT).count(), 1, "{log}");
    let line = log.lines().find(|l| l.contains(HINT)).unwrap_or_default();
    assert!(line.contains("T.EXE asked"), "{line}");
    assert!(line.contains("win32_shims=win31"), "{line}");
}
