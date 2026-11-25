//! Windows 3.1's GDI heap (`Shim::Win31GdiHeap`, src/win32/shims/win31):
//! a program with the shim finds its bitmaps through TOOLHELP as it did
//! on Windows 3.1, and BitBlt draws from the device BITMAP it changed, as
//! Lunicus's LUNIFX.DLL does. GDI's drawing into that bitmap goes where
//! the BITMAP's bmBits points, as Windows 3.1's display driver put it. A
//! program without the shim finds no GDI heap, and the log says once
//! which shim it may need. The programs are made byte by byte
//! (tests/common/win16) and end with the number of the first check that
//! failed, 0 when all held. They call InitTask first, as a program's C
//! startup code does, and run on real Windows for Workgroups 3.11 too,
//! which gives the results they expect (#2007).

use crate::win16;

use rust_dos::config::DesktopDepth;
use rust_dos::win32::shims;
use win16::{CODE, Code, program, run, run_with};

/// KERNEL's, GDI's and TOOLHELP's ordinals the programs call.
const GLOBAL_LOCK: u16 = 18;
const INIT_TASK: u16 = 91;
const PAT_BLT: u16 = 29;
const TEXT_OUT: u16 = 33;
const BIT_BLT: u16 = 34;
const SELECT_OBJECT: u16 = 45;
const CREATE_BITMAP: u16 = 48;
const CREATE_COMPATIBLE_DC: u16 = 52;
const DELETE_DC: u16 = 68;
const DELETE_OBJECT: u16 = 69;
const GET_BITMAP_BITS: u16 = 74;
const GET_CLIP_BOX: u16 = 77;
const SET_BITMAP_BITS: u16 = 106;
const GET_DIBITS: u16 = 441;
const GET_OBJECT: u16 = 82;
const GLOBAL_HANDLE_TO_SEL: u16 = 50;
const SYSTEM_HEAP_INFO: u16 = 71;
const MEMORY_READ: u16 = 78;

/// Where the programs keep things in their data segment: the bitmap and
/// DC read from and drawn into, GDI's selector, the first hGDISegment,
/// the offset MemoryRead reads at and what it read, a SYSHEAPINFO, the
/// pixels the device BITMAP is pointed at and those GetBitmapBits gave,
/// a string TextOut draws, the device BITMAP's bmBits before the program
/// changed it, where the program has the BITMAP, a clip box, the bits the
/// bitmap is made with, and a BITMAPINFO for GetDIBits, out of the way.
/// Below 10h is the instance data InitTask writes.
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
const TEXT: u16 = 0x70;
const ORIGINAL_BITS: u16 = 0x80;
const DEVICE: u16 = 0x84;
const CLIP: u16 = 0x88;
const OWN: u16 = 0x90;
const INFO_HEADER: u16 = 0x400;

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

/// End with `fail` unless the word at ES:BX+`field` is the word at `at`.
fn expect_field_is(code: &mut Code, field: u8, at: u16, fail: u8) {
    code.emit(&[0x26, 0x8B, 0x47, field]) // mov ax,es:[bx+field]
        .emit(&[0x3B, 0x06, lo(at), hi(at)]); // cmp ax,[at]
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

/// CreateBitmap(`width`, `height`, 1, 8, bits) into `at`, the bits at
/// `bits` in the data segment (NULL for 0), and a memory DC it is
/// selected into into `dc`.
fn bitmap_in_dc(code: &mut Code, width: u16, height: u16, bits: u16, at: u16, dc: u16) {
    code.push(width).push(height).push(1).push(8);
    if bits == 0 {
        code.push(0).push(0);
    } else {
        push_data(code, bits);
    }
    code.call("GDI", CREATE_BITMAP);
    store_ax(code, at);
    code.push(0).call("GDI", CREATE_COMPATIBLE_DC);
    store_ax(code, dc);
    push_word(code, dc);
    push_word(code, at);
    code.call("GDI", SELECT_OBJECT);
}

/// What LUNIFX.DLL does first, after InitTask: make a bitmap of 4 by 1
/// with bits and select it, find its device BITMAP through GDI's heap
/// (the handle table entry at the handle's offset, the object's global
/// handle at +0Ah of the object), set its size to 8 by 2 and point
/// bmBits at pixels of its own. It makes a bitmap of 8 by 2 to blit into
/// too, and keeps where the BITMAP is and where its bmBits pointed.
/// Without GDI's heap the program ends with 20h.
///
/// On Windows 3.1 a task's DLLs are initialized in InitTask: without it
/// TOOLHELP's LibMain hasn't run, and SystemHeapInfo fails (check 1).
fn change_device_bitmap() -> Code {
    let mut code = Code::new();
    code.call("KERNEL", INIT_TASK);
    for i in 0..(WIDTH * HEIGHT / 2) {
        set(&mut code, PIXELS + 2 * i, 0x0201 + 0x0202 * i);
    }
    // The bitmap's own bits: 11h, 22h, 33h, 44h. (Without bits, real
    // Windows 3.1 leaves them as the memory was.)
    set(&mut code, OWN, 0x2211);
    set(&mut code, OWN + 2, 0x4433);
    bitmap_in_dc(&mut code, 4, 1, OWN, BITMAP, DC);
    bitmap_in_dc(&mut code, WIDTH, HEIGHT, 0, TARGET, TARGET_DC);

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
    store_ax(&mut code, DEVICE);
    code.emit(&[0x89, 0x16, lo(DEVICE + 2), hi(DEVICE + 2)]); // mov [DEVICE+2],dx
    expect_field(&mut code, BM_WIDTH, 4, 8);
    expect_field(&mut code, BM_HEIGHT, 1, 9);
    expect_field(&mut code, BM_WIDTH_BYTES, 4, 10);
    expect_field(&mut code, 0x08, 0x0801, 11);
    for at in [0, 2] {
        code.emit(&[0x26, 0x8B, 0x47, BM_BITS + at as u8]); // mov ax,es:[bx+bmBits]
        store_ax(&mut code, ORIGINAL_BITS + at);
    }
    set_field(&mut code, BM_WIDTH, WIDTH);
    set_field(&mut code, BM_HEIGHT, HEIGHT);
    set_field(&mut code, BM_WIDTH_BYTES, WIDTH);
    set_field(&mut code, BM_BITS, PIXELS);
    code.emit(&[0x26, 0x8C, 0x5F, BM_BITS + 2]); // mov es:[bx+0Ch],ds
    code
}

/// BitBlt(target, 0, 0, 8, 2, dc, 0, 0, SRCCOPY), and the target's bits
/// must then be the pixels the device BITMAP points at: ends with
/// `fail`, or the next numbers for the pixels.
fn blit_shows_pixels(code: &mut Code, fail: u8) {
    push_word(code, TARGET_DC);
    code.push(0).push(0).push(WIDTH).push(HEIGHT);
    push_word(code, DC);
    code.push(0).push(0).push(0x00CC).push(0x0020);
    code.call("GDI", BIT_BLT);
    expect_ax(code, 1, fail);
    bits_are_pixels(code, TARGET, fail + 1);
}

/// GetBitmapBits(bitmap, 16, got), with the bitmap's handle at `bitmap`:
/// all 8 by 2 of its bits, which must be the pixels the device BITMAP
/// points at. Ends with `fail`, or the next numbers for the pixels.
fn bits_are_pixels(code: &mut Code, bitmap: u16, fail: u8) {
    push_word(code, bitmap);
    code.push(0).push(WIDTH * HEIGHT);
    push_data(code, GOT);
    code.call("GDI", GET_BITMAP_BITS);
    expect_ax(code, WIDTH * HEIGHT, fail);
    for i in 0..(WIDTH * HEIGHT / 2) {
        code.emit(&[0xA1, lo(GOT + 2 * i), hi(GOT + 2 * i)]) // mov ax,[GOT+i]
            .emit(&[0x3B, 0x06, lo(PIXELS + 2 * i), hi(PIXELS + 2 * i)]); // cmp ax,[PIXELS+i]
        or_fail(code, fail + 1 + i as u8);
    }
}

/// A program that does what LUNIFX.DLL does (`change_device_bitmap`) and
/// BitBlts the bitmap into one of 8 by 2, whose bits must then be the
/// pixels. Then it deletes the bitmap, whose handle table entry is then
/// a free handle of the local heap, its second word FFFFh, as on real
/// WfW 3.11 (the first is the next free handle there).
fn reads_gdis_heap() -> Code {
    let mut code = change_device_bitmap();
    blit_shows_pixels(&mut code, 12);

    // Deleted, the bitmap leaves GDI's heap.
    push_word(&mut code, DC);
    code.call("GDI", DELETE_DC);
    push_word(&mut code, BITMAP);
    code.call("GDI", DELETE_OBJECT);
    expect_ax(&mut code, 1, 30);
    read_gdi(&mut code, BITMAP, 0, 4);
    expect(&mut code, READ + 2, 0xFFFF, 31);
    code.emit(&[0xB0, 0x00]).exit_al();
    code
}

/// A program that changes the device BITMAP as LUNIFX.DLL does and then
/// does what real WfW 3.11 was seen to do with it (#2007). GetObject
/// gives the size the BITMAP now says, 8 by 2. A BitBlt from the
/// bitmap's DC copies the program's pixels. PatBlt BLACKNESS over the
/// first two pixels draws into them. The DC keeps the clip SelectObject
/// gave it, 4 by 1, so TextOut at (4, 0) and a BitBlt to (2, 1) draw
/// nothing. The next BitBlt from the bitmap copies the pixels with
/// PatBlt's change. GetBitmapBits then gives the bitmap's own 4 bytes,
/// those CreateBitmap made it with, and points bmBits back at them: from
/// then on PatBlt draws there, not into the program's pixels, and a
/// BitBlt from the bitmap copies them.
fn draws_into_the_changed_bitmap() -> Code {
    let mut code = change_device_bitmap();
    // GetObject(bitmap, 14, read): the size the BITMAP now says.
    push_word(&mut code, BITMAP);
    code.push(14);
    push_data(&mut code, READ);
    code.call("GDI", GET_OBJECT);
    expect_ax(&mut code, 14, 70);
    expect(&mut code, READ + 2, WIDTH, 71);
    expect(&mut code, READ + 4, HEIGHT, 72);
    blit_shows_pixels(&mut code, 12);
    set(&mut code, TEXT, u16::from(b'x'));

    // PatBlt(dc, 0, 0, 2, 1, BLACKNESS): the system palette's black, 0.
    push_word(&mut code, DC);
    code.push(0).push(0).push(2).push(1);
    code.push(0x0000).push(0x0042);
    code.call("GDI", PAT_BLT);
    expect_ax(&mut code, 1, 40);
    expect(&mut code, PIXELS, 0x0000, 41);

    // GetClipBox(dc, clip): SIMPLEREGION, the bitmap's size when
    // SelectObject put it in the DC.
    push_word(&mut code, DC);
    push_data(&mut code, CLIP);
    code.call("GDI", GET_CLIP_BOX);
    expect_ax(&mut code, 2, 46);
    expect(&mut code, CLIP, 0, 47);
    expect(&mut code, CLIP + 2, 0, 47);
    expect(&mut code, CLIP + 4, 4, 48);
    expect(&mut code, CLIP + 6, 1, 49);

    // TextOut(dc, 4, 0, "x", 1): outside the clip, so pixels 4 and 5
    // stay as they are.
    push_word(&mut code, DC);
    code.push(4).push(0);
    push_data(&mut code, TEXT);
    code.push(1).call("GDI", TEXT_OUT);
    expect_ax(&mut code, 1, 42);
    expect(&mut code, PIXELS + 4, 0x0605, 43);

    // BitBlt(dc, 2, 1, 2, 1, target, 0, 0, SRCCOPY): row 1 is outside
    // the clip too.
    push_word(&mut code, DC);
    code.push(2).push(1).push(2).push(1);
    push_word(&mut code, TARGET_DC);
    code.push(0).push(0).push(0x00CC).push(0x0020);
    code.call("GDI", BIT_BLT);
    expect_ax(&mut code, 1, 44);
    expect(&mut code, PIXELS + WIDTH + 2, 0x0C0B, 45);

    // What PatBlt drew shows in the next blit from the bitmap.
    blit_shows_pixels(&mut code, 50);

    // GetBitmapBits(bitmap, 16, got): the 4 bytes the bitmap was made
    // with, from its own bits.
    for i in 0..(WIDTH * HEIGHT / 2) {
        set(&mut code, GOT + 2 * i, 0xEEEE);
    }
    push_word(&mut code, BITMAP);
    code.push(0).push(WIDTH * HEIGHT);
    push_data(&mut code, GOT);
    code.call("GDI", GET_BITMAP_BITS);
    expect_ax(&mut code, 4, 60);
    expect(&mut code, GOT, 0x2211, 61);
    expect(&mut code, GOT + 2, 0x4433, 62);
    expect(&mut code, GOT + 4, 0xEEEE, 63);

    // The device BITMAP: bmBits where it pointed before the program
    // changed it, and the size the program gave it.
    code.emit(&[0xC4, 0x1E, lo(DEVICE), hi(DEVICE)]); // les bx,[DEVICE]
    expect_field_is(&mut code, BM_BITS, ORIGINAL_BITS, 64);
    expect_field_is(&mut code, BM_BITS + 2, ORIGINAL_BITS + 2, 65);
    expect_field(&mut code, BM_WIDTH, WIDTH, 66);
    expect_field(&mut code, BM_HEIGHT, HEIGHT, 67);
    expect_field(&mut code, BM_WIDTH_BYTES, WIDTH, 68);

    // PatBlt(dc, 0, 0, 1, 1, WHITENESS) now draws into the bitmap's own
    // bits, and the program's pixels stay as they are.
    push_word(&mut code, DC);
    code.push(0).push(0).push(1).push(1);
    code.push(0x00FF).push(0x0062);
    code.call("GDI", PAT_BLT);
    expect_ax(&mut code, 1, 80);
    expect(&mut code, PIXELS, 0x0000, 81);

    // BitBlt(target, 0, 0, 8, 2, dc, 0, 0, SRCCOPY) copies the bitmap's
    // own bits: white (FFh), then CreateBitmap's 22h, 33h, 44h.
    push_word(&mut code, TARGET_DC);
    code.push(0).push(0).push(WIDTH).push(HEIGHT);
    push_word(&mut code, DC);
    code.push(0).push(0).push(0x00CC).push(0x0020);
    code.call("GDI", BIT_BLT);
    expect_ax(&mut code, 1, 82);
    push_word(&mut code, TARGET);
    code.push(0).push(WIDTH * HEIGHT);
    push_data(&mut code, GOT);
    code.call("GDI", GET_BITMAP_BITS);
    expect_ax(&mut code, WIDTH * HEIGHT, 83);
    expect(&mut code, GOT, 0x22FF, 84);
    expect(&mut code, GOT + 2, 0x4433, 85);
    code.emit(&[0xB0, 0x00]).exit_al();
    code
}

/// A program that changes the device BITMAP as LUNIFX.DLL does and then
/// calls SetBitmapBits and GetDIBits on the bitmap, which on real WfW
/// 3.11 point bmBits back at the bitmap's own bits as GetBitmapBits does
/// (#2007). SetBitmapBits(bitmap, 16, A0h..AFh) takes the 4 bytes the
/// bitmap was made with into its own bits, and the program's pixels
/// stay as they are: a BitBlt from the bitmap copies A0h..A3h. The
/// program then points bmBits at its pixels again, and GetDIBits(bitmap,
/// 0, 2) of an 8 by 2 DIB gives the bitmap's own bits at the size the
/// BITMAP says: its second line (the DIB's rows go up) starts A0h..A3h.
fn bits_calls_point_bmbits_back() -> Code {
    let mut code = change_device_bitmap();
    for i in 0..(WIDTH * HEIGHT / 2) {
        set(&mut code, GOT + 2 * i, 0xA1A0 + 0x0202 * i);
    }
    push_word(&mut code, BITMAP);
    code.push(0).push(WIDTH * HEIGHT);
    push_data(&mut code, GOT);
    code.call("GDI", SET_BITMAP_BITS);
    expect_ax(&mut code, 4, 90);
    expect(&mut code, PIXELS, 0x0201, 91);
    code.emit(&[0xC4, 0x1E, lo(DEVICE), hi(DEVICE)]); // les bx,[DEVICE]
    expect_field_is(&mut code, BM_BITS, ORIGINAL_BITS, 92);
    expect_field_is(&mut code, BM_BITS + 2, ORIGINAL_BITS + 2, 92);
    expect_field(&mut code, BM_WIDTH, WIDTH, 93);
    expect_field(&mut code, BM_HEIGHT, HEIGHT, 93);

    // BitBlt(target, 0, 0, 8, 2, dc, 0, 0, SRCCOPY): the own bits.
    push_word(&mut code, TARGET_DC);
    code.push(0).push(0).push(WIDTH).push(HEIGHT);
    push_word(&mut code, DC);
    code.push(0).push(0).push(0x00CC).push(0x0020);
    code.call("GDI", BIT_BLT);
    expect_ax(&mut code, 1, 94);
    push_word(&mut code, TARGET);
    code.push(0).push(WIDTH * HEIGHT);
    push_data(&mut code, GOT);
    code.call("GDI", GET_BITMAP_BITS);
    expect_ax(&mut code, WIDTH * HEIGHT, 95);
    expect(&mut code, GOT, 0xA1A0, 96);
    expect(&mut code, GOT + 2, 0xA3A2, 96);

    // bmBits at the pixels again, then GetDIBits(target DC, bitmap, 0,
    // 2, got, info, DIB_RGB_COLORS) with an 8 by 2 header of 8 bits.
    code.emit(&[0xC4, 0x1E, lo(DEVICE), hi(DEVICE)]); // les bx,[DEVICE]
    set_field(&mut code, BM_BITS, PIXELS);
    code.emit(&[0x26, 0x8C, 0x5F, BM_BITS + 2]); // mov es:[bx+0Ch],ds
    for i in 0..(WIDTH * HEIGHT / 2) {
        set(&mut code, GOT + 2 * i, 0xEEEE);
    }
    for i in 0..20 {
        set(&mut code, INFO_HEADER + 2 * i, 0);
    }
    set(&mut code, INFO_HEADER, 40);
    set(&mut code, INFO_HEADER + 4, WIDTH);
    set(&mut code, INFO_HEADER + 8, HEIGHT);
    set(&mut code, INFO_HEADER + 12, 1);
    set(&mut code, INFO_HEADER + 14, 8);
    push_word(&mut code, TARGET_DC);
    push_word(&mut code, BITMAP);
    code.push(0).push(HEIGHT);
    push_data(&mut code, GOT);
    push_data(&mut code, INFO_HEADER);
    code.push(0).call("GDI", GET_DIBITS);
    expect_ax(&mut code, HEIGHT, 97);
    expect(&mut code, GOT + WIDTH, 0xA1A0, 98);
    expect(&mut code, GOT + WIDTH + 2, 0xA3A2, 98);
    code.emit(&[0xC4, 0x1E, lo(DEVICE), hi(DEVICE)]); // les bx,[DEVICE]
    expect_field_is(&mut code, BM_BITS, ORIGINAL_BITS, 99);
    expect_field_is(&mut code, BM_BITS + 2, ORIGINAL_BITS + 2, 99);
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

/// GDI's drawing into a bitmap whose device BITMAP the program changed
/// goes where its bmBits points, as Windows 3.1's display driver drew
/// there: the program sees it in its pixels, and the next BitBlt from the
/// bitmap copies it (#1900). The DC keeps its clip, and GetBitmapBits
/// points bmBits back at the bitmap's own bits, as on real WfW 3.11.
#[test]
fn gdi_draws_into_the_pixels_a_changed_device_bitmap_points_at() {
    let (code, log) = run_with(
        "gdi_heap_draws",
        program(&draws_into_the_changed_bitmap(), CODE),
        |cpu, _| {
            cpu.bus.win32.shims = shims::win31::WIN31;
            cpu.bus.win32.depth = DesktopDepth::Palette;
        },
    );
    assert_eq!(code, 0, "{log}");
}

/// SetBitmapBits and GetDIBits on a bitmap whose device BITMAP the
/// program changed go to the bitmap's own bits and point bmBits back at
/// them, as on real WfW 3.11.
#[test]
fn bits_calls_on_a_changed_device_bitmap_use_its_own_bits() {
    let (code, log) = run_with(
        "gdi_heap_bits",
        program(&bits_calls_point_bmbits_back(), CODE),
        |cpu, _| {
            cpu.bus.win32.shims = shims::win31::WIN31;
            cpu.bus.win32.depth = DesktopDepth::Palette;
        },
    );
    assert_eq!(code, 0, "{log}");
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
