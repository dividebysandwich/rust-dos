//! The built-in Windows' 16-bit OLE 2 modules (COMPOBJ, OLE2, OLE2DISP,
//! OLE2NLS and STORAGE) as a Windows 3.1 program sees them: a program
//! made with Visual Basic 4's 16-bit edition imports all five through
//! VB40016.DLL. The programs are made byte by byte (tests/common/win16)
//! and end with the number of the first check that failed, 0 when all
//! held.

use crate::win16;

use rust_dos_ne as ne;
use win16::{CODE, Code, program, run};

/// The ordinals the program imports.
const OLE_INITIALIZE: u16 = 2; // OLE2
const OLE_UNINITIALIZE: u16 = 3;
const CO_GET_MALLOC: u16 = 4; // COMPOBJ
const STRING_FROM_CLSID: u16 = 19; // COMPOBJ
const IID_IUNKNOWN: u16 = 38; // COMPOBJ's variable _IID_IUnknown
const SYS_ALLOC_STRING_LEN: u16 = 4; // OLE2DISP
const SYS_STRING_LEN: u16 = 7;
const VARIANT_CHANGE_TYPE: u16 = 12;
const VAR_I4_FROM_STR: u16 = 56;
const VAR_BSTR_FROM_R8: u16 = 94;
const STG_CREATE_DOCFILE: u16 = 1; // STORAGE
const GET_USER_DEFAULT_LCID: u16 = 1; // OLE2NLS

/// IMalloc's methods, by their place in the vtable.
const ALLOC: u8 = 3;
const GET_SIZE: u8 = 6;

/// Where the program keeps things in its data segment.
const MALLOC: u16 = 0x10;
const BLOCK: u16 = 0x14;
const BSTR: u16 = 0x18;
const OUT: u16 = 0x1C;
const TEXT: u16 = 0x20;
const SOURCE: u16 = 0x40;
const DEST: u16 = 0x50;

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

/// mov [at],ax; mov [at+2],dx.
fn store_dx_ax(code: &mut Code, at: u16) {
    code.emit(&[0xA3, lo(at), hi(at)]);
    let at = at + 2;
    code.emit(&[0x89, 0x16, lo(at), hi(at)]);
}

/// push word [at].
fn push_word(code: &mut Code, at: u16) {
    code.emit(&[0xFF, 0x36, lo(at), hi(at)]);
}

/// push ds; push `at`: a far pointer to the data segment's `at`.
fn push_ds(code: &mut Code, at: u16) {
    code.emit(&[0x1E]).push(at);
}

/// End with `fail` unless the last comparison found them equal.
fn or_fail(code: &mut Code, fail: u8) {
    code.emit(&[0x74, 0x06]) // je over the exit
        .emit(&[0xB0, fail, 0xB4, 0x4C, 0xCD, 0x21]);
}

/// End with `fail` unless DX:AX is `value`.
fn expect_dx_ax(code: &mut Code, value: u32, fail: u8) {
    let (l, h) = (value as u16, (value >> 16) as u16);
    code.emit(&[0x3D, lo(l), hi(l)]);
    or_fail(code, fail);
    code.emit(&[0x81, 0xFA, lo(h), hi(h)]);
    or_fail(code, fail);
}

/// End with `fail` unless the word at `at` is `value`.
fn expect(code: &mut Code, at: u16, value: u16, fail: u8) {
    code.emit(&[0x81, 0x3E, lo(at), hi(at), lo(value), hi(value)]);
    or_fail(code, fail);
}

/// End with `fail` unless the byte at ES:[BX+`disp`] is `value`.
fn expect_es_bx(code: &mut Code, disp: i8, value: u8, fail: u8) {
    code.emit(&[0x26, 0x80, 0x7F, disp as u8, value]);
    or_fail(code, fail);
}

/// les bx,[at].
fn les_bx(code: &mut Code, at: u16) {
    code.emit(&[0xC4, 0x1E, lo(at), hi(at)]);
}

/// Call the method `method` of the task allocator (at MALLOC) with the
/// words `args` (pushed in this order, before the object's pointer), as
/// C calls it, and take them off again.
fn call_malloc(code: &mut Code, method: u8, push_args: impl Fn(&mut Code), words: u8) {
    push_args(code);
    les_bx(code, MALLOC);
    code.emit(&[0x06, 0x53]) // push es; push bx
        .emit(&[0x26, 0xC4, 0x1F]) // les bx,es:[bx]
        .emit(&[0x26, 0xFF, 0x5F, 4 * method]) // call far es:[bx+4*method]
        .emit(&[0x83, 0xC4, 2 * (words + 2)]); // add sp,...
}

/// Copy `text` and a NUL to the data segment's `at`.
fn put_text(code: &mut Code, at: u16, text: &[u8]) {
    let mut bytes = text.to_vec();
    bytes.push(0);
    if bytes.len() % 2 == 1 {
        bytes.push(0);
    }
    for (i, pair) in bytes.chunks(2).enumerate() {
        set(
            code,
            at + 2 * i as u16,
            u16::from_le_bytes([pair[0], pair[1]]),
        );
    }
}

/// A Visual Basic 4 program's first calls: OleInitialize, the task
/// allocator from CoGetMalloc and a block from it, BSTRs, a VARIANT
/// changed to text, numbers from and to text, an interface's GUID
/// imported as a variable, StringFromCLSID, the locale, and a compound
/// file, which the built-in Windows doesn't make (E_NOTIMPL).
#[test]
fn a_vb4_programs_ole_calls_work() {
    let mut code = Code::new();

    // OleInitialize(NULL): S_OK.
    code.push(0).push(0).call("OLE2", OLE_INITIALIZE);
    expect_dx_ax(&mut code, 0, 1);

    // CoGetMalloc(MEMCTX_TASK, &malloc): S_OK.
    code.push(0).push(1);
    push_ds(&mut code, MALLOC);
    code.call("COMPOBJ", CO_GET_MALLOC);
    expect_dx_ax(&mut code, 0, 2);

    // IMalloc::Alloc(100h): a block, whose IMalloc::GetSize is 100h.
    call_malloc(
        &mut code,
        ALLOC,
        |c| {
            c.push(0).push(0x100);
        },
        2,
    );
    store_dx_ax(&mut code, BLOCK);
    code.emit(&[0x85, 0xD2]); // test dx,dx
    code.emit(&[0x75, 0x06])
        .emit(&[0xB0, 3, 0xB4, 0x4C, 0xCD, 0x21]);
    call_malloc(
        &mut code,
        GET_SIZE,
        |c| {
            push_word(c, BLOCK + 2);
            push_word(c, BLOCK);
        },
        2,
    );
    expect_dx_ax(&mut code, 0x100, 4);

    // SysAllocStringLen("1234", 4): a BSTR, its length in the dword
    // before it, a NUL after it; SysStringLen 4.
    put_text(&mut code, TEXT, b"1234");
    push_ds(&mut code, TEXT);
    code.push(4).call("OLE2DISP", SYS_ALLOC_STRING_LEN);
    store_dx_ax(&mut code, BSTR);
    code.emit(&[0x52, 0x50]).call("OLE2DISP", SYS_STRING_LEN); // push dx; push ax
    code.emit(&[0x3D, 4, 0]);
    or_fail(&mut code, 5);
    les_bx(&mut code, BSTR);
    expect_es_bx(&mut code, -4, 4, 6);
    expect_es_bx(&mut code, 0, b'1', 7);
    expect_es_bx(&mut code, 4, 0, 8);

    // VarI4FromStr("1234", 0, 0, &out): 1234.
    push_ds(&mut code, TEXT);
    code.push(0).push(0).push(0).push(0);
    push_ds(&mut code, OUT);
    code.call("OLE2DISP", VAR_I4_FROM_STR);
    expect_dx_ax(&mut code, 0, 9);
    expect(&mut code, OUT, 1234, 10);

    // VariantChangeType(&dest, &source, 0, VT_BSTR) of the VT_I2 -5:
    // a VT_BSTR of "-5".
    set(&mut code, SOURCE, 2);
    set(&mut code, SOURCE + 8, (-5i16) as u16);
    set(&mut code, DEST, 0);
    push_ds(&mut code, DEST);
    push_ds(&mut code, SOURCE);
    code.push(0).push(8).call("OLE2DISP", VARIANT_CHANGE_TYPE);
    expect_dx_ax(&mut code, 0, 11);
    expect(&mut code, DEST, 8, 12);
    les_bx(&mut code, DEST + 8);
    expect_es_bx(&mut code, -4, 2, 13);
    expect_es_bx(&mut code, 0, b'-', 14);
    expect_es_bx(&mut code, 1, b'5', 15);

    // VarBstrFromR8(2.5, 0, 0, &bstr): "2.5". The double is pushed as
    // its high word first.
    let bits = 2.5f64.to_bits();
    for k in (0..4).rev() {
        code.push((bits >> (16 * k)) as u16);
    }
    code.push(0).push(0).push(0).push(0);
    push_ds(&mut code, BSTR);
    code.call("OLE2DISP", VAR_BSTR_FROM_R8);
    expect_dx_ax(&mut code, 0, 16);
    les_bx(&mut code, BSTR);
    expect_es_bx(&mut code, -4, 3, 17);
    expect_es_bx(&mut code, 1, b'.', 18);

    // _IID_IUnknown, {00000000-0000-0000-C000-000000000046}, imported:
    // mov bx,offset; mov ax,seg; mov es,ax.
    let compobj = code.module("COMPOBJ");
    let target = ne::Target::Ordinal {
        module: compobj,
        ordinal: IID_IUNKNOWN,
    };
    let at = code.here() + 1;
    code.emit(&[0xBB, 0xFF, 0xFF]);
    code.fixup(at, ne::ADDR_OFFSET, target.clone());
    let at = code.here() + 1;
    code.emit(&[0xB8, 0xFF, 0xFF]);
    code.fixup(at, ne::ADDR_SELECTOR, target);
    code.emit(&[0x8E, 0xC0]);
    expect_es_bx(&mut code, 0, 0, 19);
    expect_es_bx(&mut code, 8, 0xC0, 20);
    expect_es_bx(&mut code, 15, 0x46, 21);

    // StringFromCLSID(&IID_IUnknown, &text): its text, in a block of the
    // task allocator's.
    code.emit(&[0x06, 0x53]); // push es; push bx
    push_ds(&mut code, OUT);
    code.call("COMPOBJ", STRING_FROM_CLSID);
    expect_dx_ax(&mut code, 0, 22);
    les_bx(&mut code, OUT);
    expect_es_bx(&mut code, 0, b'{', 23);
    expect_es_bx(&mut code, 20, b'C', 24);
    expect_es_bx(&mut code, 37, b'}', 25);
    expect_es_bx(&mut code, 38, 0, 26);

    // GetUserDefaultLCID: English (United States), 0409h.
    code.call("OLE2NLS", GET_USER_DEFAULT_LCID);
    expect_dx_ax(&mut code, 0x0409, 27);

    // StgCreateDocfile(name, mode, 0, &storage): E_NOTIMPL, and NULL.
    set(&mut code, OUT, 1);
    push_ds(&mut code, TEXT);
    code.push(0).push(0x1012).push(0).push(0);
    push_ds(&mut code, OUT);
    code.call("STORAGE", STG_CREATE_DOCFILE);
    expect_dx_ax(&mut code, 0x8000_4001, 28);
    expect(&mut code, OUT, 0, 29);

    code.emit(&[0xB0, 0x00]).exit_al();
    let (exit, log) = run("ole2_vb4_calls", program(&code, CODE));
    assert_eq!(exit, 0, "check {exit} failed\n{log}");
}

/// Where the allocator test keeps its object: a vtable pointer and a
/// reference count, and the vtable (QueryInterface, AddRef, Release).
const OBJECT: u16 = 0x60;
const REFS: u16 = OBJECT + 4;
const VTABLE: u16 = 0x70;

/// A program that gives OleInitialize its own allocator keeps it alive
/// through COMPOBJ's reference: OleInitialize adds one, CoGetMalloc
/// another for its caller, and OleUninitialize lets COMPOBJ's go.
/// Visual Basic 4 releases its own reference right after OleInitialize,
/// so without COMPOBJ's its allocator frees itself.
#[test]
fn ole_initialize_holds_the_programs_allocator() {
    let mut code = Code::new();

    // AddRef and Release of the object, as C calls them: the count is
    // at this+4, and the new one comes back in DX:AX.
    let jump = code.here() + 1;
    code.emit(&[0xE9, 0, 0]);
    let method = |code: &mut Code, op: u8| {
        let at = code.here();
        code.emit(&[0x55, 0x8B, 0xEC]) // push bp; mov bp,sp
            .emit(&[0xC4, 0x5E, 0x06]) // les bx,[bp+6]
            .emit(&[0x26, 0xFF, op, 0x04]) // inc/dec word es:[bx+4]
            .emit(&[0x26, 0x8B, 0x47, 0x04]) // mov ax,es:[bx+4]
            .emit(&[0x33, 0xD2, 0x5D, 0xCB]); // xor dx,dx; pop bp; retf
        at
    };
    let add_ref = method(&mut code, 0x47);
    let release = method(&mut code, 0x4F);
    let start = code.here();
    code.patch(jump, start - (jump + 2));

    // The object, with one reference, its vtable in the data segment.
    set(&mut code, OBJECT, VTABLE);
    code.emit(&[0x8C, 0x1E, lo(OBJECT + 2), hi(OBJECT + 2)]); // mov [..],ds
    set(&mut code, REFS, 1);
    for (i, at) in [(1u16, add_ref), (2, release)] {
        set(&mut code, VTABLE + 4 * i, at);
        let seg = VTABLE + 4 * i + 2;
        code.emit(&[0x8C, 0x0E, lo(seg), hi(seg)]); // mov [..],cs
    }

    // OleInitialize(&object): S_OK, and a reference more.
    push_ds(&mut code, OBJECT);
    code.call("OLE2", OLE_INITIALIZE);
    expect_dx_ax(&mut code, 0, 1);
    expect(&mut code, REFS, 2, 2);

    // CoGetMalloc(MEMCTX_TASK, &malloc): the object, a reference more.
    code.push(0).push(1);
    push_ds(&mut code, MALLOC);
    code.call("COMPOBJ", CO_GET_MALLOC);
    expect_dx_ax(&mut code, 0, 3);
    expect(&mut code, MALLOC, OBJECT, 4);
    expect(&mut code, REFS, 3, 5);

    // OleUninitialize: COMPOBJ's reference goes.
    code.call("OLE2", OLE_UNINITIALIZE);
    expect(&mut code, REFS, 2, 6);

    code.emit(&[0xB0, 0x00]).exit_al();
    let (exit, log) = run("ole2_app_malloc", program(&code, CODE));
    assert_eq!(exit, 0, "check {exit} failed\n{log}");
}
