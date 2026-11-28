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
/// file.
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

    // StgCreateDocfile(name, mode, 0, &storage): S_OK, and a storage.
    set(&mut code, OUT + 2, 0);
    push_ds(&mut code, TEXT);
    code.push(0).push(0x1012).push(0).push(0);
    push_ds(&mut code, OUT);
    code.call("STORAGE", STG_CREATE_DOCFILE);
    expect_dx_ax(&mut code, 0, 28);
    code.emit(&[0x83, 0x3E, lo(OUT + 2), hi(OUT + 2), 0x00]); // cmp word [..],0
    code.emit(&[0x75, 0x06]) // jne over the exit
        .emit(&[0xB0, 29, 0xB4, 0x4C, 0xCD, 0x21]);

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
const SAFE_ARRAY_GET_UBOUND: u16 = 19;
const SAFE_ARRAY_PUT_ELEMENT: u16 = 26;
const SAFE_ARRAY_REDIM: u16 = 42;

/// IMalloc's methods, by their place in the vtable.
const REALLOC: u8 = 4;
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
/// How many times the counting allocator's Realloc was called.
const REALLOCS: u16 = 0xF8;

/// Make the counting allocator at COUNTER, with one reference: its
/// Alloc and Free count and keep what they were given, its Realloc
/// counts, and all three hand on to the standard allocator, which
/// CoCreateStandardMalloc puts at STANDARD. A debugging allocator a
/// program wraps around the standard one does the same.
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
    // Realloc counts at REALLOCS and hands its two dwords on.
    let realloc = code.here();
    let count = (REALLOCS - COUNTER) as u8;
    code.emit(&[0x55, 0x8B, 0xEC]) // push bp; mov bp,sp
        .emit(&[0xC4, 0x5E, 0x06]) // les bx,[bp+6]
        .emit(&[0x26, 0xFF, 0x47, count]) // inc word es:[bx+count]
        .emit(&[0xFF, 0x76, 0x10, 0xFF, 0x76, 0x0E]) // push the size
        .emit(&[0xFF, 0x76, 0x0C, 0xFF, 0x76, 0x0A]) // push the block
        .emit(&[0x26, 0xFF, 0x77, 0x0E, 0x26, 0xFF, 0x77, 0x0C]) // push standard
        .emit(&[0x26, 0xC4, 0x5F, 0x0C]) // les bx,es:[bx+0Ch]
        .emit(&[0x26, 0xC4, 0x1F]) // les bx,es:[bx]
        .emit(&[0x26, 0xFF, 0x5F, 4 * REALLOC]) // call far es:[bx+4*REALLOC]
        .emit(&[0x83, 0xC4, 0x0C, 0x5D, 0xCB]); // add sp,12; pop bp; retf
    let start = code.here();
    code.patch(jump, start - (jump + 2));

    set(code, COUNTER, COUNTER_VTABLE);
    code.emit(&[0x8C, 0x1E, lo(COUNTER + 2), hi(COUNTER + 2)]); // mov [..],ds
    for at in [ALLOCS, FREES, FREED, FREED_SEL, COUNTER_ASKED, REALLOCS] {
        set(code, at, 0);
    }
    set(code, COUNTER_REFS, 1);
    for (i, at) in [
        (1u16, add_ref),
        (2, release),
        (3, alloc),
        (4, realloc),
        (5, free),
    ] {
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

/// SafeArrayRedim cutting an array of BSTRs, with the program's
/// allocator, as Windows' OLE2DISP does it: the elements cut are copied
/// to a block of their own (an Alloc), the data is made smaller (a
/// Realloc), and then the BSTRs cut and that block go to the program's
/// Free, the block last.
#[test]
fn safe_array_redim_cuts_through_the_programs_allocator() {
    let mut code = Code::new();
    counting_allocator(&mut code);
    push_ds(&mut code, COUNTER);
    code.call("OLE2", OLE_INITIALIZE);
    expect_dx_ax(&mut code, 0, 1);

    // SafeArrayCreate(VT_BSTR, 1, {3, 0}): its descriptor and its data.
    set(&mut code, BOUND, 3);
    for at in [BOUND + 2, BOUND + 4, BOUND + 6, INDEX + 2] {
        set(&mut code, at, 0);
    }
    code.push(8).push(1);
    push_ds(&mut code, BOUND);
    code.call("OLE2DISP", SAFE_ARRAY_CREATE);
    store_dx_ax(&mut code, ARRAY);
    expect(&mut code, ALLOCS, 2, 2);

    // Element 2 a copy of SysAllocString("abc"), which is then freed.
    put_text(&mut code, TEXT, b"abc");
    push_ds(&mut code, TEXT);
    code.call("OLE2DISP", SYS_ALLOC_STRING);
    store_dx_ax(&mut code, BSTR);
    set(&mut code, INDEX, 2);
    push_word(&mut code, ARRAY + 2);
    push_word(&mut code, ARRAY);
    push_ds(&mut code, INDEX);
    push_word(&mut code, BSTR + 2);
    push_word(&mut code, BSTR);
    code.call("OLE2DISP", SAFE_ARRAY_PUT_ELEMENT);
    expect_dx_ax(&mut code, 0, 3);
    push_word(&mut code, BSTR + 2);
    push_word(&mut code, BSTR);
    code.call("OLE2DISP", SYS_FREE_STRING);
    expect(&mut code, ALLOCS, 4, 4);
    expect(&mut code, FREES, 1, 5);

    // SafeArrayRedim(psa, {1, 0}): the two elements cut (8 bytes) go to
    // a block of their own, the data is reallocated, and element 2's
    // BSTR and that block are freed.
    set(&mut code, BOUND, 1);
    push_word(&mut code, ARRAY + 2);
    push_word(&mut code, ARRAY);
    push_ds(&mut code, BOUND);
    code.call("OLE2DISP", SAFE_ARRAY_REDIM);
    expect_dx_ax(&mut code, 0, 6);
    expect(&mut code, ALLOCS, 5, 7);
    expect(&mut code, COUNTER_ASKED, 8, 8);
    expect(&mut code, REALLOCS, 1, 9);
    expect(&mut code, FREES, 3, 10);

    // SafeArrayGetUBound(psa, 1, &bound): 0.
    set(&mut code, OUT, 0xFFFF);
    push_word(&mut code, ARRAY + 2);
    push_word(&mut code, ARRAY);
    code.push(1);
    push_ds(&mut code, OUT);
    code.call("OLE2DISP", SAFE_ARRAY_GET_UBOUND);
    expect_dx_ax(&mut code, 0, 11);
    expect(&mut code, OUT, 0, 12);

    // SafeArrayDestroy: the data and the descriptor.
    push_word(&mut code, ARRAY + 2);
    push_word(&mut code, ARRAY);
    code.call("OLE2DISP", SAFE_ARRAY_DESTROY);
    expect_dx_ax(&mut code, 0, 13);
    expect(&mut code, FREES, 5, 14);

    code.call("OLE2", OLE_UNINITIALIZE);
    code.emit(&[0xB0, 0x00]).exit_al();
    let (exit, log) = run("ole2disp_redim_app_malloc", program(&code, CODE));
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

/// OLE2DISP's ordinals of the error object's functions.
const SET_ERROR_INFO: u16 = 110;
const GET_ERROR_INFO: u16 = 111;
const CREATE_ERROR_INFO: u16 = 112;
const IID_IERRORINFO: u16 = 113; // OLE2DISP's variable _IID_IErrorInfo

/// The error object's methods: ICreateErrorInfo's and IErrorInfo's.
const QUERY_INTERFACE: u8 = 0;
const RELEASE: u8 = 2;
const SET_DESCRIPTION: u8 = 5;
const SET_HELP_CONTEXT: u8 = 7;
const GET_DESCRIPTION: u8 = 5;
const GET_HELP_CONTEXT: u8 = 7;

/// Where the program keeps the two interfaces of the error object.
const CREATE_INFO: u16 = 0x80;
const ERROR_INFO: u16 = 0x84;
const GOT: u16 = 0x88;

/// Call the method `method` of the object whose far pointer is at
/// `object`, with `push_args` pushing its arguments (`words` of them)
/// before the object's pointer, as C calls it, and take them off again.
fn call_object(code: &mut Code, object: u16, method: u8, push_args: impl Fn(&mut Code), words: u8) {
    push_args(code);
    les_bx(code, object);
    code.emit(&[0x06, 0x53]) // push es; push bx
        .emit(&[0x26, 0xC4, 0x1F]) // les bx,es:[bx]
        .emit(&[0x26, 0xFF, 0x5F, 4 * method]) // call far es:[bx+4*method]
        .emit(&[0x83, 0xC4, 2 * (words + 2)]); // add sp,...
}

/// Visual Basic 4's runtime raises a Basic error as Windows' OLE does:
/// CreateErrorInfo, the description and help context set through
/// ICreateErrorInfo, its IErrorInfo from QueryInterface given to
/// SetErrorInfo, and GetErrorInfo gives the same object back, whose
/// GetDescription is a copy of the text and GetHelpContext the number.
#[test]
fn a_program_raises_an_error_object() {
    let mut code = Code::new();
    code.push(0).push(0).call("OLE2", OLE_INITIALIZE);
    expect_dx_ax(&mut code, 0, 1);

    // CreateErrorInfo(&create_info): S_OK.
    push_ds(&mut code, CREATE_INFO);
    code.call("OLE2DISP", CREATE_ERROR_INFO);
    expect_dx_ax(&mut code, 0, 2);

    // SetDescription("Path not found"), SetHelpContext(76).
    put_text(&mut code, TEXT, b"Path not found");
    call_object(
        &mut code,
        CREATE_INFO,
        SET_DESCRIPTION,
        |c| push_ds(c, TEXT),
        2,
    );
    expect_dx_ax(&mut code, 0, 3);
    call_object(
        &mut code,
        CREATE_INFO,
        SET_HELP_CONTEXT,
        |c| {
            c.push(0).push(76);
        },
        2,
    );
    expect_dx_ax(&mut code, 0, 4);

    // QueryInterface(IID_IErrorInfo, &error_info): S_OK.
    call_object(
        &mut code,
        CREATE_INFO,
        QUERY_INTERFACE,
        |c| {
            push_ds(c, ERROR_INFO);
            // push seg _IID_IErrorInfo; push offset _IID_IErrorInfo
            let module = c.module("OLE2DISP");
            let at = c.here() + 1;
            c.emit(&[0x68, 0xFF, 0xFF]);
            c.fixup(
                at,
                ne::ADDR_SELECTOR,
                ne::Target::Ordinal {
                    module,
                    ordinal: IID_IERRORINFO,
                },
            );
            let at = c.here() + 1;
            c.emit(&[0x68, 0xFF, 0xFF]);
            c.fixup(
                at,
                ne::ADDR_OFFSET,
                ne::Target::Ordinal {
                    module,
                    ordinal: IID_IERRORINFO,
                },
            );
        },
        4,
    );
    expect_dx_ax(&mut code, 0, 5);
    call_object(&mut code, CREATE_INFO, RELEASE, |_| {}, 0);
    expect_dx_ax(&mut code, 1, 6);

    // SetErrorInfo(0, error_info), then GetErrorInfo(0, &got): the same
    // object.
    code.push(0).push(0);
    code.emit(&[0xFF, 0x36, lo(ERROR_INFO + 2), hi(ERROR_INFO + 2)])
        .emit(&[0xFF, 0x36, lo(ERROR_INFO), hi(ERROR_INFO)]);
    code.call("OLE2DISP", SET_ERROR_INFO);
    expect_dx_ax(&mut code, 0, 7);
    code.push(0).push(0);
    push_ds(&mut code, GOT);
    code.call("OLE2DISP", GET_ERROR_INFO);
    expect_dx_ax(&mut code, 0, 8);
    code.emit(&[0xA1, lo(GOT), hi(GOT)]) // mov ax,[got]
        .emit(&[0x3B, 0x06, lo(ERROR_INFO), hi(ERROR_INFO)]); // cmp ax,[error_info]
    or_fail(&mut code, 9);

    // GetDescription(&bstr): "Path not found"; GetHelpContext: 76.
    call_object(
        &mut code,
        ERROR_INFO,
        GET_DESCRIPTION,
        |c| push_ds(c, BSTR),
        2,
    );
    expect_dx_ax(&mut code, 0, 10);
    les_bx(&mut code, BSTR);
    expect_es_bx(&mut code, -4, 14, 11);
    expect_es_bx(&mut code, 0, b'P', 12);
    expect_es_bx(&mut code, 13, b'd', 13);
    call_object(
        &mut code,
        ERROR_INFO,
        GET_HELP_CONTEXT,
        |c| push_ds(c, OUT),
        2,
    );
    expect_dx_ax(&mut code, 0, 14);
    expect(&mut code, OUT, 76, 15);

    // The references SetErrorInfo and GetErrorInfo handed over, then
    // QueryInterface's: the last one frees the object.
    call_object(&mut code, ERROR_INFO, RELEASE, |_| {}, 0);
    expect_dx_ax(&mut code, 1, 16);
    call_object(&mut code, ERROR_INFO, RELEASE, |_| {}, 0);
    expect_dx_ax(&mut code, 0, 17);
    code.emit(&[0xB0, 0x00]).exit_al();

    let (exit, log) = run("ole2_error_info", program(&code, CODE));
    assert_eq!(exit, 0, "check {exit} failed\n{log}");
}

/// STORAGE's functions and the methods of its objects.
const STG_OPEN_STORAGE: u16 = 3;
const STG_IS_STORAGE_FILE: u16 = 5;
const CREATE_STREAM: u8 = 3;
const OPEN_STREAM: u8 = 4;
const ENUM_ELEMENTS: u8 = 11;
const READ: u8 = 3;
const WRITE: u8 = 4;
const STREAM_STAT: u8 = 12;
const NEXT: u8 = 3;

/// TITLE.TDF in `dir`: a compound file with a stream "Contents" of 11
/// bytes and a storage "Sub", as a Windows 3.1 title keeps its data.
fn title_file(dir: &std::path::Path) {
    let mut file = cfb::create(dir.join("TITLE.TDF")).unwrap();
    let mut stream = file.create_stream("/Contents").unwrap();
    std::io::Write::write_all(&mut stream, b"HELLO WORLD").unwrap();
    drop(stream);
    file.create_storage("/Sub").unwrap();
    file.flush().unwrap();
}

/// push ds; pop es; mov bx,`at`.
fn es_bx_ds(code: &mut Code, at: u16) {
    code.emit(&[0x1E, 0x07, 0xBB, lo(at), hi(at)]);
}

/// StgOpenStorage(path at `path`, NULL, STGM_SHARE_DENY_WRITE, NULL, 0,
/// &storage at `out`), as Pascal pushes it.
fn open_title(code: &mut Code, path: u16, out: u16) {
    push_ds(code, path);
    code.push(0).push(0).push(0).push(0x20);
    code.push(0).push(0).push(0).push(0);
    push_ds(code, out);
    code.call("STORAGE", STG_OPEN_STORAGE);
}

/// IStorage::OpenStream(name at `name`, NULL, STGM_SHARE_EXCLUSIVE, 0,
/// &stream at `out`) of the storage at `storage`.
fn open_stream(code: &mut Code, storage: u16, name: u16, out: u16) {
    call_object(
        code,
        storage,
        OPEN_STREAM,
        |c| {
            push_ds(c, out);
            c.push(0).push(0).push(0).push(0x10).push(0).push(0);
            push_ds(c, name);
        },
        10,
    );
}

/// IStorage::EnumElements(0, NULL, 0, &enumerator at `out`).
fn enum_elements(code: &mut Code, storage: u16, out: u16) {
    call_object(
        code,
        storage,
        ENUM_ELEMENTS,
        |c| {
            push_ds(c, out);
            c.push(0).push(0).push(0).push(0).push(0).push(0);
        },
        8,
    );
}

/// IEnumSTATSTG::Next(count, STATSTGs at `stats`, &fetched at `got`).
fn next(code: &mut Code, enumerator: u16, count: u16, stats: u16, got: u16) {
    call_object(
        code,
        enumerator,
        NEXT,
        |c| {
            push_ds(c, got);
            push_ds(c, stats);
            c.push(0).push(count);
        },
        6,
    );
}

/// IStream::Stat(&stat at `stats`, STATFLAG_DEFAULT) of the stream at
/// `stream`.
fn stream_stat(code: &mut Code, stream: u16, stats: u16) {
    call_object(
        code,
        stream,
        STREAM_STAT,
        |c| {
            c.push(0).push(0);
            push_ds(c, stats);
        },
        4,
    );
}

/// End with `fail` unless the first letters of the names of the two
/// STATSTGs at `stats` are C and S, in either order: "Contents" and
/// "Sub".
fn expect_both_names(code: &mut Code, stats: u16, fail: u8) {
    les_bx(code, stats);
    code.emit(&[0x26, 0x8A, 0x07]); // mov al,es:[bx]
    les_bx(code, stats + 72);
    code.emit(&[0x26, 0x02, 0x07]) // add al,es:[bx]
        .emit(&[0x3C, b'C' + b'S']); // cmp al,'C'+'S'
    or_fail(code, fail);
}

/// A Windows 3.1 title reads its compound file through STORAGE, as The
/// Magic School Bus Explores the Solar System reads MSBSOLAR.TDF:
/// StgOpenStorage, a stream opened, read and its Stat with its name in
/// ANSI, the storage's elements enumerated, StgIsStorageFile. Then a
/// new docfile made with a stream written, which the file has after
/// the last Release.
#[test]
fn a_program_reads_and_writes_compound_files() {
    const STORAGE: u16 = 0x10;
    const STREAM: u16 = 0x14;
    const ENUMERATOR: u16 = 0x18;
    const COUNT: u16 = 0x1C;
    const PATH: u16 = 0x20;
    const NAME: u16 = 0x30;
    const BUFFER: u16 = 0x40;
    const OTHER: u16 = 0x50;
    const STATS: u16 = 0x60;
    let mut code = Code::new();
    code.push(0).push(0).call("OLE2", OLE_INITIALIZE);
    put_text(&mut code, PATH, b"C:\\TITLE.TDF");
    put_text(&mut code, NAME, b"Contents");

    open_title(&mut code, PATH, STORAGE);
    expect_dx_ax(&mut code, 0, 1);
    open_stream(&mut code, STORAGE, NAME, STREAM);
    expect_dx_ax(&mut code, 0, 2);

    // IStream::Read(buffer, 16, &count): the stream's 11 bytes.
    call_object(
        &mut code,
        STREAM,
        READ,
        |c| {
            push_ds(c, COUNT);
            c.push(0).push(16);
            push_ds(c, BUFFER);
        },
        6,
    );
    expect_dx_ax(&mut code, 0, 3);
    expect(&mut code, COUNT, 11, 4);
    es_bx_ds(&mut code, BUFFER);
    expect_es_bx(&mut code, 0, b'H', 5);
    expect_es_bx(&mut code, 10, b'D', 6);

    // IStream::Stat: a stream (2) of 11 bytes, its name ANSI text.
    stream_stat(&mut code, STREAM, STATS);
    expect_dx_ax(&mut code, 0, 7);
    expect(&mut code, STATS + 4, 2, 8);
    expect(&mut code, STATS + 8, 11, 9);
    les_bx(&mut code, STATS);
    expect_es_bx(&mut code, 0, b'C', 10);
    expect_es_bx(&mut code, 7, b's', 11);
    expect_es_bx(&mut code, 8, 0, 12);

    // EnumElements, then Next(2): both elements, a stream (2) and a
    // storage (1); Next(1) after them: S_FALSE and none.
    enum_elements(&mut code, STORAGE, ENUMERATOR);
    expect_dx_ax(&mut code, 0, 13);
    next(&mut code, ENUMERATOR, 2, STATS, COUNT);
    expect_dx_ax(&mut code, 0, 14);
    expect(&mut code, COUNT, 2, 15);
    code.emit(&[0xA1, lo(STATS + 4), hi(STATS + 4)]) // mov ax,[type]
        .emit(&[0x03, 0x06, lo(STATS + 76), hi(STATS + 76)]) // add ax,[type]
        .emit(&[0x3D, 3, 0]); // cmp ax,3
    or_fail(&mut code, 16);
    expect_both_names(&mut code, STATS, 17);
    next(&mut code, ENUMERATOR, 1, STATS, COUNT);
    expect_dx_ax(&mut code, 1, 18);
    expect(&mut code, COUNT, 0, 19);
    for object in [ENUMERATOR, STREAM, STORAGE] {
        call_object(&mut code, object, RELEASE, |_| {}, 0);
    }

    // StgIsStorageFile: S_OK for the docfile, S_FALSE for the program.
    push_ds(&mut code, PATH);
    code.call("STORAGE", STG_IS_STORAGE_FILE);
    expect_dx_ax(&mut code, 0, 20);
    put_text(&mut code, OTHER, b"C:\\T.EXE");
    push_ds(&mut code, OTHER);
    code.call("STORAGE", STG_IS_STORAGE_FILE);
    expect_dx_ax(&mut code, 1, 21);

    // StgCreateDocfile("C:\NEW.STG", STGM_CREATE | STGM_READWRITE |
    // STGM_SHARE_EXCLUSIVE), CreateStream("Data"), Write("ABC").
    put_text(&mut code, PATH, b"C:\\NEW.STG");
    put_text(&mut code, NAME, b"Data");
    put_text(&mut code, BUFFER, b"ABC");
    push_ds(&mut code, PATH);
    code.push(0).push(0x1012).push(0).push(0);
    push_ds(&mut code, STORAGE);
    code.call("STORAGE", STG_CREATE_DOCFILE);
    expect_dx_ax(&mut code, 0, 22);
    call_object(
        &mut code,
        STORAGE,
        CREATE_STREAM,
        |c| {
            push_ds(c, STREAM);
            c.push(0).push(0).push(0).push(0).push(0).push(0x1012);
            push_ds(c, NAME);
        },
        10,
    );
    expect_dx_ax(&mut code, 0, 23);
    call_object(
        &mut code,
        STREAM,
        WRITE,
        |c| {
            c.push(0).push(0).push(0).push(3);
            push_ds(c, BUFFER);
        },
        6,
    );
    expect_dx_ax(&mut code, 0, 24);
    for object in [STREAM, STORAGE] {
        call_object(&mut code, object, RELEASE, |_| {}, 0);
    }
    code.emit(&[0xB0, 0x00]).exit_al();

    let mut dir = std::path::PathBuf::new();
    let (exit, log) = win16::run_with("ole2_storage", program(&code, CODE), |_, d| {
        title_file(d);
        dir = d.to_path_buf();
    });
    assert_eq!(exit, 0, "check {exit} failed\n{log}");
    let mut file = cfb::open(dir.join("NEW.STG")).unwrap();
    let mut data = Vec::new();
    let mut stream = file.open_stream("/Data").unwrap();
    std::io::Read::read_to_end(&mut stream, &mut data).unwrap();
    assert_eq!(data, b"ABC");
}

/// The names in STATSTGs are the program's to free with the allocator
/// CoGetMalloc gives it, so they come from the program's own allocator
/// when OleInitialize was given one: Stat asks its Alloc for one block,
/// and IEnumSTATSTG::Next for one for each element, in turn.
#[test]
fn storage_names_take_the_programs_allocator() {
    const STATS: u16 = 0x10;
    const STORAGE: u16 = 0xA0;
    const STREAM: u16 = 0xA4;
    const ENUMERATOR: u16 = 0xA8;
    const COUNT: u16 = 0xAC;
    const PATH: u16 = 0xDC;
    const NAME: u16 = 0xEA;
    let mut code = Code::new();
    counting_allocator(&mut code);
    push_ds(&mut code, COUNTER);
    code.call("OLE2", OLE_INITIALIZE);
    expect_dx_ax(&mut code, 0, 1);
    put_text(&mut code, PATH, b"C:\\TITLE.TDF");
    put_text(&mut code, NAME, b"Contents");
    open_title(&mut code, PATH, STORAGE);
    expect_dx_ax(&mut code, 0, 2);
    open_stream(&mut code, STORAGE, NAME, STREAM);
    expect_dx_ax(&mut code, 0, 3);

    stream_stat(&mut code, STREAM, STATS);
    expect_dx_ax(&mut code, 0, 4);
    expect(&mut code, ALLOCS, 1, 5);
    expect(&mut code, COUNTER_ASKED, 9, 6);
    les_bx(&mut code, STATS);
    expect_es_bx(&mut code, 0, b'C', 7);
    expect_es_bx(&mut code, 8, 0, 8);

    enum_elements(&mut code, STORAGE, ENUMERATOR);
    expect_dx_ax(&mut code, 0, 9);
    next(&mut code, ENUMERATOR, 2, STATS, COUNT);
    expect_dx_ax(&mut code, 0, 10);
    expect(&mut code, COUNT, 2, 11);
    expect(&mut code, ALLOCS, 3, 12);
    expect_both_names(&mut code, STATS, 13);
    for object in [ENUMERATOR, STREAM, STORAGE] {
        call_object(&mut code, object, RELEASE, |_| {}, 0);
    }
    code.emit(&[0xB0, 0x00]).exit_al();

    let (exit, log) = win16::run_with("ole2_storage_app_malloc", program(&code, CODE), |_, d| {
        title_file(d)
    });
    assert_eq!(exit, 0, "check {exit} failed\n{log}");
}

const CO_GET_CLASS_OBJECT: u16 = 7; // COMPOBJ
const CO_CREATE_INSTANCE: u16 = 13;
const REG_SET_VALUE: u16 = 5; // SHELL

/// Where the program keeps things for the in-process server: the class,
/// IID_IUnknown, the object the server makes (its vtable after it, and
/// how many times Release was called), the registry's path and the
/// server's name, and a second place for an interface.
const CLSID: u16 = 0x40;
const IID: u16 = 0x50;
const SERVED: u16 = 0x60;
const SERVED_VTABLE: u16 = 0x64;
const RELEASES: u16 = 0x74;
const KEY_PATH: u16 = 0x80;
const SERVER_NAME: u16 = 0xC0;
const OUT2: u16 = 0xD0;

/// SRV.DLL, an in-process server whose DllGetClassObject (ordinal 1)
/// makes its class object in the data segment of the class's GUID (the
/// program's, in the test), at SERVED: an IClassFactory whose
/// CreateInstance gives 1234:5678 and whose other methods are Release,
/// which counts at RELEASES.
fn server_dll() -> Vec<u8> {
    let mut c = vec![0xB8, 0x01, 0x00, 0xCB]; // LibMain: mov ax,1; retf
    let get_class_object = c.len() as u16;
    // DllGetClassObject(rclsid, riid, ppv), Pascal's order.
    c.extend([0x55, 0x8B, 0xEC]); // push bp; mov bp,sp
    c.extend([0xC4, 0x5E, 0x0E]); // les bx,[bp+0Eh]: rclsid
    // mov word es:[SERVED],SERVED_VTABLE; mov es:[SERVED+2],es
    c.extend([0x26, 0xC7, 0x06, lo(SERVED), hi(SERVED)]);
    c.extend([lo(SERVED_VTABLE), hi(SERVED_VTABLE)]);
    c.extend([0x26, 0x8C, 0x06, lo(SERVED + 2), hi(SERVED + 2)]);
    let mut methods = Vec::new();
    for slot in 0..4u16 {
        let at = SERVED_VTABLE + 4 * slot;
        c.extend([0x26, 0xC7, 0x06, lo(at), hi(at)]);
        methods.push(c.len());
        c.extend([0, 0]); // the method's offset, patched below
        c.extend([0x26, 0x8C, 0x0E, lo(at + 2), hi(at + 2)]); // mov es:[..],cs
    }
    c.extend([0x8C, 0xC0]); // mov ax,es
    c.extend([0xC4, 0x5E, 0x06]); // les bx,[bp+6]: ppv
    c.extend([0x26, 0xC7, 0x07, lo(SERVED), hi(SERVED)]); // mov word es:[bx],SERVED
    c.extend([0x26, 0x89, 0x47, 0x02]); // mov es:[bx+2],ax
    c.extend([0x31, 0xC0, 0x31, 0xD2]); // xor ax,ax; xor dx,dx
    c.extend([0x5D, 0xCA, 0x0C, 0x00]); // pop bp; retf 12

    // Release(this), as C calls it.
    let release = c.len() as u16;
    c.extend([0x55, 0x8B, 0xEC, 0xC4, 0x5E, 0x06]); // push bp; mov bp,sp; les bx,[bp+6]
    c.extend([0x26, 0xFF, 0x47, (RELEASES - SERVED) as u8]); // inc word es:[bx+..]
    c.extend([0x31, 0xC0, 0x31, 0xD2, 0x5D, 0xCB]); // xor ax,ax; xor dx,dx; pop bp; retf

    // CreateInstance(this, pUnkOuter, riid, ppv), as C calls it.
    let create = c.len() as u16;
    c.extend([0x55, 0x8B, 0xEC, 0xC4, 0x5E, 0x12]); // push bp; mov bp,sp; les bx,[bp+12h]
    c.extend([0x26, 0xC7, 0x07, 0x78, 0x56]); // mov word es:[bx],5678h
    c.extend([0x26, 0xC7, 0x47, 0x02, 0x34, 0x12]); // mov word es:[bx+2],1234h
    c.extend([0x31, 0xC0, 0x31, 0xD2, 0x5D, 0xCB]);

    for (slot, &at) in methods.iter().enumerate() {
        let target = if slot == 3 { create } else { release };
        c[at..at + 2].copy_from_slice(&target.to_le_bytes());
    }

    let mut b = ne::Builder::new("SRV", ne::LIBMODULE);
    let text = b.segment(ne::SegmentSpec {
        data: c,
        flags: CODE,
        alloc: 0,
        relocations: Vec::new(),
    });
    let entry = ne::Entry::Code {
        segment: text as u8,
        offset: get_class_object,
        flags: ne::ENTRY_EXPORTED,
        moveable: true,
    };
    b.entry(1, entry);
    b.export("DllGetClassObject", 1, false);
    b.entry_point(text, 0);
    b.build()
}

/// Push the dword `value`.
fn push_long(code: &mut Code, value: u32) {
    code.push((value >> 16) as u16).push(value as u16);
}

/// A class registered with an `InprocServer` in HKEY_CLASSES_ROOT\CLSID
/// and no class object in the task: CoGetClassObject loads the server
/// from the registry's name, runs its LibMain, and gives what its
/// DllGetClassObject gives, without releasing it. CoCreateInstance gives
/// the object the server's class factory makes, then releases the
/// factory. A class that isn't registered is REGDB_E_CLASSNOTREG. OLE 2's
/// COMPOBJ starts in-process servers so; the built-in one gave
/// REGDB_E_CLASSNOTREG for every class no task had registered, and The
/// Magic School Bus Explores the Solar System stops at its start without
/// the class MSPAM.DLL serves.
#[test]
fn an_in_process_server_gives_its_class_objects() {
    let mut code = Code::new();
    code.push(0).push(0).call("OLE2", OLE_INITIALIZE);
    expect_dx_ax(&mut code, 0, 1);

    // {11223344-5566-7788-99AA-BBCCDDEEFF00} and IID_IUnknown.
    let clsid = [
        0x44, 0x33, 0x22, 0x11, 0x66, 0x55, 0x88, 0x77, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF,
        0x00,
    ];
    let iid = [0, 0, 0, 0, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46];
    for i in 0..8 {
        let word = |g: &[u8; 16]| u16::from_le_bytes([g[2 * i], g[2 * i + 1]]);
        set(&mut code, CLSID + 2 * i as u16, word(&clsid));
        set(&mut code, IID + 2 * i as u16, word(&iid));
    }
    set(&mut code, RELEASES, 0);

    // RegSetValue(HKEY_CLASSES_ROOT, "CLSID\{...}\InprocServer", REG_SZ,
    // "SRV.DLL", 7).
    let key = b"CLSID\\{11223344-5566-7788-99AA-BBCCDDEEFF00}\\InprocServer";
    put_text(&mut code, KEY_PATH, key);
    put_text(&mut code, SERVER_NAME, b"SRV.DLL");
    push_long(&mut code, 1);
    push_ds(&mut code, KEY_PATH);
    push_long(&mut code, 1);
    push_ds(&mut code, SERVER_NAME);
    push_long(&mut code, 7);
    code.call("SHELL", REG_SET_VALUE);
    expect_dx_ax(&mut code, 0, 2);

    // CoGetClassObject(&CLSID, CLSCTX_INPROC_SERVER, NULL, &IID, &OUT).
    push_ds(&mut code, CLSID);
    push_long(&mut code, 1);
    push_long(&mut code, 0);
    push_ds(&mut code, IID);
    push_ds(&mut code, OUT);
    code.call("COMPOBJ", CO_GET_CLASS_OBJECT);
    expect_dx_ax(&mut code, 0, 3);
    expect(&mut code, OUT, SERVED, 4);
    expect(&mut code, RELEASES, 0, 5);

    // CoCreateInstance(&CLSID, NULL, CLSCTX_INPROC_SERVER, &IID, &OUT2).
    push_ds(&mut code, CLSID);
    push_long(&mut code, 0);
    push_long(&mut code, 1);
    push_ds(&mut code, IID);
    push_ds(&mut code, OUT2);
    code.call("COMPOBJ", CO_CREATE_INSTANCE);
    expect_dx_ax(&mut code, 0, 6);
    expect(&mut code, OUT2, 0x5678, 7);
    expect(&mut code, OUT2 + 2, 0x1234, 8);
    expect(&mut code, RELEASES, 1, 9);

    // A class with no server: {11223345-...}.
    set(&mut code, CLSID, 0x3345);
    push_ds(&mut code, CLSID);
    push_long(&mut code, 1);
    push_long(&mut code, 0);
    push_ds(&mut code, IID);
    push_ds(&mut code, OUT);
    code.call("COMPOBJ", CO_GET_CLASS_OBJECT);
    expect_dx_ax(&mut code, 0x8004_0154, 10);

    code.emit(&[0xB0, 0x00]).exit_al();
    let (exit, log) = win16::run_with("ole2_inproc_server", program(&code, CODE), |_, d| {
        std::fs::write(d.join("SRV.DLL"), server_dll()).unwrap();
    });
    assert_eq!(exit, 0, "check {exit} failed\n{log}");
    assert!(log.contains("Loaded SRV"), "{log}");
}
