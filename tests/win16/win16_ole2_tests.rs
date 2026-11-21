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
const GET_SCODE: u16 = 26;
const CO_REGISTER_MESSAGE_FILTER: u16 = 27;
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

/// Where the tests of the program's objects keep their object: a vtable
/// pointer, a reference count and the size its IMalloc::Alloc was last
/// asked for; the vtable (QueryInterface, AddRef, Release, Alloc); and
/// the block its Alloc gives.
const OBJECT: u16 = 0x60;
const REFS: u16 = OBJECT + 4;
const ASKED: u16 = OBJECT + 6;
const VTABLE: u16 = 0x70;
const OWN_BLOCK: u16 = 0xA0;

/// Make the program's object at OBJECT, with one reference: AddRef and
/// Release count at this+4 and return the new count in DX:AX, as C
/// calls them; Alloc keeps the size it is asked for at this+6 and gives
/// the data segment's OWN_BLOCK.
fn program_object(code: &mut Code) {
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
    let add_ref = method(code, 0x47);
    let release = method(code, 0x4F);
    let alloc = code.here();
    code.emit(&[0x55, 0x8B, 0xEC]) // push bp; mov bp,sp
        .emit(&[0xC4, 0x5E, 0x06]) // les bx,[bp+6]
        .emit(&[0x8B, 0x46, 0x0A]) // mov ax,[bp+10]
        .emit(&[0x26, 0x89, 0x47, 0x06]) // mov es:[bx+6],ax
        .emit(&[0xB8, lo(OWN_BLOCK), hi(OWN_BLOCK)]) // mov ax,OWN_BLOCK
        .emit(&[0x8C, 0xC2, 0x5D, 0xCB]); // mov dx,es; pop bp; retf
    let start = code.here();
    code.patch(jump, start - (jump + 2));

    set(code, OBJECT, VTABLE);
    code.emit(&[0x8C, 0x1E, lo(OBJECT + 2), hi(OBJECT + 2)]); // mov [..],ds
    set(code, REFS, 1);
    set(code, ASKED, 0);
    for (i, at) in [(1u16, add_ref), (2, release), (3, alloc)] {
        set(code, VTABLE + 4 * i, at);
        let seg = VTABLE + 4 * i + 2;
        code.emit(&[0x8C, 0x0E, lo(seg), hi(seg)]); // mov [..],cs
    }
}

/// A program that gives OleInitialize its own allocator keeps it alive
/// through COMPOBJ's reference: OleInitialize adds one, CoGetMalloc
/// another for its caller, and OleUninitialize lets COMPOBJ's go.
/// Visual Basic 4 releases its own reference right after OleInitialize,
/// so without COMPOBJ's its allocator frees itself.
#[test]
fn ole_initialize_holds_the_programs_allocator() {
    let mut code = Code::new();
    program_object(&mut code);

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

/// StringFromCLSID's text is the caller's to free with the allocator
/// CoGetMalloc gives it, so it comes from the program's own allocator
/// when OleInitialize was given one: its Alloc is asked for the text's
/// 39 bytes, and the text is in the block it gave.
#[test]
fn string_from_clsid_takes_the_programs_allocator() {
    let mut code = Code::new();
    program_object(&mut code);
    push_ds(&mut code, OBJECT);
    code.call("OLE2", OLE_INITIALIZE);
    expect_dx_ax(&mut code, 0, 1);

    // StringFromCLSID(&GUID_NULL in the data segment, &text).
    for i in 0..8 {
        set(&mut code, SOURCE + 2 * i, 0);
    }
    push_ds(&mut code, SOURCE);
    push_ds(&mut code, OUT);
    code.call("COMPOBJ", STRING_FROM_CLSID);
    expect_dx_ax(&mut code, 0, 2);
    expect(&mut code, ASKED, 39, 3);
    expect(&mut code, OUT, OWN_BLOCK, 4);
    les_bx(&mut code, OUT);
    expect_es_bx(&mut code, 0, b'{', 5);
    expect_es_bx(&mut code, 1, b'0', 6);
    expect_es_bx(&mut code, 37, b'}', 7);
    expect_es_bx(&mut code, 38, 0, 8);

    code.emit(&[0xB0, 0x00]).exit_al();
    let (exit, log) = run("ole2_string_app_malloc", program(&code, CODE));
    assert_eq!(exit, 0, "check {exit} failed\n{log}");
}

/// CoRegisterMessageFilter with no place for the filter before it
/// releases that one, and GetScode keeps a result's severity, facility
/// and code.
#[test]
fn a_replaced_message_filter_is_released() {
    let mut code = Code::new();
    program_object(&mut code);
    push_ds(&mut code, OBJECT);
    code.call("OLE2", OLE_INITIALIZE);

    // CoRegisterMessageFilter(&object, NULL): a reference more.
    push_ds(&mut code, OBJECT);
    code.push(0)
        .push(0)
        .call("COMPOBJ", CO_REGISTER_MESSAGE_FILTER);
    expect_dx_ax(&mut code, 0, 1);
    expect(&mut code, REFS, 3, 2);

    // Again: the new one's reference added, the old one's released.
    push_ds(&mut code, OBJECT);
    code.push(0)
        .push(0)
        .call("COMPOBJ", CO_REGISTER_MESSAGE_FILTER);
    expect_dx_ax(&mut code, 0, 3);
    expect(&mut code, REFS, 3, 4);

    // CoRegisterMessageFilter(NULL, NULL): the filter's released.
    code.push(0).push(0).push(0).push(0);
    code.call("COMPOBJ", CO_REGISTER_MESSAGE_FILTER);
    expect(&mut code, REFS, 2, 5);

    // GetScode(8FFF0005h): 800F0005h.
    code.push(0x8FFF).push(0x0005).call("COMPOBJ", GET_SCODE);
    expect_dx_ax(&mut code, 0x800F_0005, 6);

    code.emit(&[0xB0, 0x00]).exit_al();
    let (exit, log) = run("ole2_message_filter", program(&code, CODE));
    assert_eq!(exit, 0, "check {exit} failed\n{log}");
}

/// The ordinals the tests of OLE2DISP's blocks import.
const CO_CREATE_STANDARD_MALLOC: u16 = 71; // COMPOBJ
const SYS_ALLOC_STRING: u16 = 2; // OLE2DISP
const SYS_FREE_STRING: u16 = 6;
const VARIANT_CLEAR: u16 = 9;
const SAFE_ARRAY_CREATE: u16 = 15;
const SAFE_ARRAY_DESTROY: u16 = 16;
const SAFE_ARRAY_PUT_ELEMENT: u16 = 26;

/// IMalloc's methods, by their place in the vtable.
const FREE: u8 = 5;
const DID_ALLOC: u8 = 7;

/// The program's counting allocator: a vtable pointer, a reference
/// count, how many blocks its Alloc made and its Free freed, the block
/// last freed (offset, then selector at FREED_SEL), the standard
/// allocator it hands the blocks on to, and the size last asked for.
const COUNTER: u16 = 0xB0;
const COUNTER_REFS: u16 = COUNTER + 4;
const ALLOCS: u16 = COUNTER + 6;
const FREES: u16 = COUNTER + 8;
const FREED: u16 = COUNTER + 0xA;
const STANDARD: u16 = COUNTER + 0xC;
const COUNTER_ASKED: u16 = COUNTER + 0x10;
const FREED_SEL: u16 = COUNTER + 0x12;
const COUNTER_VTABLE: u16 = 0xC4;
/// A SAFEARRAYBOUND, an index and a safe array's pointer.
const BOUND: u16 = 0xE8;
const INDEX: u16 = 0xF0;
const ARRAY: u16 = 0xF4;

/// Make the counting allocator at COUNTER, with one reference: its
/// Alloc and Free count, keep what they were given, and hand on to the
/// standard allocator, which CoCreateStandardMalloc puts at STANDARD.
/// A debugging allocator a program wraps around the standard one does
/// the same.
fn counting_allocator(code: &mut Code) {
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
    let add_ref = method(code, 0x47);
    let release = method(code, 0x4F);
    // Call the standard allocator's `method` with the dword argument at
    // [bp+10], and return what it returned.
    let hand_on = |code: &mut Code, method: u8| {
        code.emit(&[0xFF, 0x76, 0x0C, 0xFF, 0x76, 0x0A]) // push the dword
            .emit(&[0x26, 0xFF, 0x77, 0x0E, 0x26, 0xFF, 0x77, 0x0C]) // push standard
            .emit(&[0x26, 0xC4, 0x5F, 0x0C]) // les bx,es:[bx+0Ch]
            .emit(&[0x26, 0xC4, 0x1F]) // les bx,es:[bx]
            .emit(&[0x26, 0xFF, 0x5F, 4 * method]) // call far es:[bx+4*method]
            .emit(&[0x83, 0xC4, 0x08, 0x5D, 0xCB]); // add sp,8; pop bp; retf
    };
    let alloc = code.here();
    code.emit(&[0x55, 0x8B, 0xEC]) // push bp; mov bp,sp
        .emit(&[0xC4, 0x5E, 0x06]) // les bx,[bp+6]
        .emit(&[0x26, 0xFF, 0x47, 0x06]) // inc word es:[bx+6]
        .emit(&[0x8B, 0x46, 0x0A]) // mov ax,[bp+10]
        .emit(&[0x26, 0x89, 0x47, 0x10]); // mov es:[bx+10h],ax
    hand_on(code, ALLOC);
    let free = code.here();
    code.emit(&[0x55, 0x8B, 0xEC]) // push bp; mov bp,sp
        .emit(&[0xC4, 0x5E, 0x06]) // les bx,[bp+6]
        .emit(&[0x26, 0xFF, 0x47, 0x08]) // inc word es:[bx+8]
        .emit(&[0x8B, 0x46, 0x0A]) // mov ax,[bp+10]
        .emit(&[0x26, 0x89, 0x47, 0x0A]) // mov es:[bx+0Ah],ax
        .emit(&[0x8B, 0x46, 0x0C]) // mov ax,[bp+12]
        .emit(&[0x26, 0x89, 0x47, 0x12]); // mov es:[bx+12h],ax
    hand_on(code, FREE);
    let start = code.here();
    code.patch(jump, start - (jump + 2));

    set(code, COUNTER, COUNTER_VTABLE);
    code.emit(&[0x8C, 0x1E, lo(COUNTER + 2), hi(COUNTER + 2)]); // mov [..],ds
    for at in [ALLOCS, FREES, FREED, FREED_SEL, COUNTER_ASKED] {
        set(code, at, 0);
    }
    set(code, COUNTER_REFS, 1);
    for (i, at) in [(1u16, add_ref), (2, release), (3, alloc), (5, free)] {
        set(code, COUNTER_VTABLE + 4 * i, at);
        let seg = COUNTER_VTABLE + 4 * i + 2;
        code.emit(&[0x8C, 0x0E, lo(seg), hi(seg)]); // mov [..],cs
    }
    // CoCreateStandardMalloc(MEMCTX_TASK, &standard).
    code.push(0).push(1);
    push_ds(code, STANDARD);
    code.call("COMPOBJ", CO_CREATE_STANDARD_MALLOC);
}

/// End with `fail` unless the word at `a` is the word at `b`.
fn expect_same(code: &mut Code, a: u16, b: u16, fail: u8) {
    code.emit(&[0xA1, lo(a), hi(a)]) // mov ax,[a]
        .emit(&[0x3B, 0x06, lo(b), hi(b)]); // cmp ax,[b]
    or_fail(code, fail);
}

/// OLE2DISP takes its blocks from the task allocator, the program's own
/// when OleInitialize was given one, and gives them back to it: a BSTR,
/// a safe array of BSTRs (its descriptor, its data, its element's copy)
/// and a VARIANT's text are made by the program's Alloc and freed by its
/// Free, when SysFreeString, SafeArrayDestroy and VariantClear free
/// them.
#[test]
fn ole2disp_takes_blocks_from_the_programs_allocator() {
    let mut code = Code::new();
    counting_allocator(&mut code);
    push_ds(&mut code, COUNTER);
    code.call("OLE2", OLE_INITIALIZE);
    expect_dx_ax(&mut code, 0, 1);

    // SysAllocString("abc"): a block of 3 + 5 bytes from the program's
    // Alloc, its text 4 bytes in.
    put_text(&mut code, TEXT, b"abc");
    push_ds(&mut code, TEXT);
    code.call("OLE2DISP", SYS_ALLOC_STRING);
    store_dx_ax(&mut code, BSTR);
    expect(&mut code, ALLOCS, 1, 2);
    expect(&mut code, COUNTER_ASKED, 8, 3);
    les_bx(&mut code, BSTR);
    expect_es_bx(&mut code, -4, 3, 4);
    expect_es_bx(&mut code, 0, b'a', 5);

    // SysFreeString: the program's Free gets the block.
    push_word(&mut code, BSTR + 2);
    push_word(&mut code, BSTR);
    code.call("OLE2DISP", SYS_FREE_STRING);
    expect(&mut code, FREES, 1, 6);
    code.emit(&[0x83, 0x2E, lo(BSTR), hi(BSTR), 4]); // sub word [BSTR],4
    expect_same(&mut code, FREED, BSTR, 7);
    expect_same(&mut code, FREED_SEL, BSTR + 2, 8);

    // SafeArrayCreate(VT_BSTR, 1, {2, 0}): its descriptor and its data.
    set(&mut code, BOUND, 2);
    for at in [BOUND + 2, BOUND + 4, BOUND + 6, INDEX, INDEX + 2] {
        set(&mut code, at, 0);
    }
    code.push(8).push(1);
    push_ds(&mut code, BOUND);
    code.call("OLE2DISP", SAFE_ARRAY_CREATE);
    store_dx_ax(&mut code, ARRAY);
    code.emit(&[0x85, 0xD2]); // test dx,dx
    code.emit(&[0x75, 0x06])
        .emit(&[0xB0, 9, 0xB4, 0x4C, 0xCD, 0x21]);
    expect(&mut code, ALLOCS, 3, 10);
    expect(&mut code, COUNTER_ASKED, 8, 11);

    // SafeArrayPutElement(psa, {0}, SysAllocString("abc")): a copy.
    push_ds(&mut code, TEXT);
    code.call("OLE2DISP", SYS_ALLOC_STRING);
    store_dx_ax(&mut code, BSTR);
    push_word(&mut code, ARRAY + 2);
    push_word(&mut code, ARRAY);
    push_ds(&mut code, INDEX);
    push_word(&mut code, BSTR + 2);
    push_word(&mut code, BSTR);
    code.call("OLE2DISP", SAFE_ARRAY_PUT_ELEMENT);
    expect_dx_ax(&mut code, 0, 12);
    expect(&mut code, ALLOCS, 5, 13);

    // SafeArrayDestroy: the element's copy, the data and the descriptor
    // go to the program's Free, the descriptor last.
    push_word(&mut code, ARRAY + 2);
    push_word(&mut code, ARRAY);
    code.call("OLE2DISP", SAFE_ARRAY_DESTROY);
    expect_dx_ax(&mut code, 0, 14);
    expect(&mut code, FREES, 4, 15);
    expect_same(&mut code, FREED, ARRAY, 16);
    expect_same(&mut code, FREED_SEL, ARRAY + 2, 17);

    // VariantChangeType(&dest, &source, 0, VT_BSTR) of the VT_I2 -5, then
    // VariantClear(&dest): the text's block made and freed.
    set(&mut code, SOURCE, 2);
    set(&mut code, SOURCE + 8, (-5i16) as u16);
    set(&mut code, DEST, 0);
    push_ds(&mut code, DEST);
    push_ds(&mut code, SOURCE);
    code.push(0).push(8).call("OLE2DISP", VARIANT_CHANGE_TYPE);
    expect_dx_ax(&mut code, 0, 18);
    expect(&mut code, ALLOCS, 6, 19);
    expect(&mut code, COUNTER_ASKED, 7, 20);
    push_ds(&mut code, DEST);
    code.call("OLE2DISP", VARIANT_CLEAR);
    expect_dx_ax(&mut code, 0, 21);
    expect(&mut code, FREES, 5, 22);
    expect(&mut code, DEST, 0, 23);

    // The BSTR the program made is still its own to free.
    push_word(&mut code, BSTR + 2);
    push_word(&mut code, BSTR);
    code.call("OLE2DISP", SYS_FREE_STRING);
    expect(&mut code, FREES, 6, 24);

    code.call("OLE2", OLE_UNINITIALIZE);
    code.emit(&[0xB0, 0x00]).exit_al();
    let (exit, log) = run("ole2disp_app_malloc", program(&code, CODE));
    assert_eq!(exit, 0, "check {exit} failed\n{log}");
}

/// A BSTR is a block of the task allocator's, 4 bytes before the text:
/// IMalloc::GetSize gives its size, DidAlloc owns it, and IMalloc::Free
/// frees it, as a program may instead of calling SysFreeString.
#[test]
fn a_bstr_is_freed_with_imalloc_free() {
    let mut code = Code::new();
    code.push(0).push(0).call("OLE2", OLE_INITIALIZE);
    expect_dx_ax(&mut code, 0, 1);
    code.push(0).push(1);
    push_ds(&mut code, MALLOC);
    code.call("COMPOBJ", CO_GET_MALLOC);
    expect_dx_ax(&mut code, 0, 2);

    // SysAllocString("hello"), and BLOCK its block.
    put_text(&mut code, TEXT, b"hello");
    push_ds(&mut code, TEXT);
    code.call("OLE2DISP", SYS_ALLOC_STRING);
    code.emit(&[0x2D, 4, 0]); // sub ax,4
    store_dx_ax(&mut code, BLOCK);
    let block = |c: &mut Code| {
        push_word(c, BLOCK + 2);
        push_word(c, BLOCK);
    };
    call_malloc(&mut code, GET_SIZE, block, 2);
    expect_dx_ax(&mut code, 10, 3);
    call_malloc(&mut code, DID_ALLOC, block, 2);
    expect_dx_ax(&mut code, 1, 4);
    call_malloc(&mut code, FREE, block, 2);

    // The block is free: the next BSTR of its size takes its place.
    push_ds(&mut code, TEXT);
    code.call("OLE2DISP", SYS_ALLOC_STRING);
    code.emit(&[0x2D, 4, 0]); // sub ax,4
    store_dx_ax(&mut code, OUT);
    expect_same(&mut code, OUT, BLOCK, 5);
    expect_same(&mut code, OUT + 2, BLOCK + 2, 6);

    code.emit(&[0xB0, 0x00]).exit_al();
    let (exit, log) = run("ole2disp_bstr_imalloc_free", program(&code, CODE));
    assert_eq!(exit, 0, "check {exit} failed\n{log}");
}
