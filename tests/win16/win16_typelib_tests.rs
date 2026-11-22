//! The built-in Windows' 16-bit TYPELIB as a Windows 3.1 program sees
//! it: Visual Basic 4's 16-bit runtime loads its program's type library
//! with LoadTypeLib, registers it, and reads its types. The programs are
//! made byte by byte (tests/common/win16) and end with the number of the
//! first check that failed, 0 when all held.
//!
//! The library is the SLTG fixture of the 32-bit OLEAUT32's tests
//! (src/win32/oleaut32/testdata, whose .txt file lists what Windows
//! reads from it), as the program's TYPELIB resource 2, the way Visual
//! Basic 4 puts its programs' libraries in them.

use crate::win16;

use rust_dos_ne as ne;
use win16::{CODE, Code, program_with_resources, run};

/// TYPELIB's ordinals.
const LOAD_TYPE_LIB: u16 = 3;
const REGISTER_TYPE_LIB: u16 = 10;
const LOAD_REG_TYPE_LIB: u16 = 11;
const QUERY_PATH_OF_REG_TYPE_LIB: u16 = 14;
const OA_BUILD_VERSION: u16 = 15;

/// ITypeLib's methods, by their place in the vtable.
const LIB_RELEASE: u8 = 2;
const GET_TYPE_INFO_COUNT: u8 = 3;
const GET_TYPE_INFO_OF_GUID: u8 = 6;
const GET_LIB_ATTR: u8 = 7;
const RELEASE_TLIB_ATTR: u8 = 12;
/// ITypeInfo's.
const INFO_RELEASE: u8 = 2;
const GET_TYPE_ATTR: u8 = 3;
const GET_FUNC_DESC: u8 = 5;
const GET_VAR_DESC: u8 = 6;
const GET_REF_TYPE_OF_IMPL_TYPE: u8 = 8;
const GET_DOCUMENTATION: u8 = 12;
const GET_REF_TYPE_INFO: u8 = 14;

/// Where the program keeps things in its data segment.
const LIB: u16 = 0x10;
const INFO: u16 = 0x14;
const ATTR: u16 = 0x18;
const BSTR: u16 = 0x1C;
const OUT: u16 = 0x20;
const INFO2: u16 = 0x24;
const GUID: u16 = 0x30;
const TEXT: u16 = 0x40;

/// The fixture, made a 16-bit library: its SYSKIND, after the help
/// file's name and the help context, SYS_WIN16.
fn library16() -> Vec<u8> {
    let mut data = include_bytes!("../../src/win32/oleaut32/testdata/rdtest-sltg.tlb").to_vec();
    let help = data
        .windows(10)
        .position(|w| w == b"rdtest.hlp")
        .expect("the help file's name");
    let syskind = help + 10 + 4;
    assert_eq!(
        &data[syskind..syskind + 2],
        &[1, 0],
        "SYS_WIN32 in the fixture"
    );
    data[syskind] = 0;
    data
}

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

/// push ds; push `at`: a far pointer to the data segment's `at`.
fn push_ds(code: &mut Code, at: u16) {
    code.emit(&[0x1E]).push(at);
}

/// push dword [at].
fn push_dword(code: &mut Code, at: u16) {
    code.emit(&[0xFF, 0x36, lo(at + 2), hi(at + 2)])
        .emit(&[0xFF, 0x36, lo(at), hi(at)]);
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

/// End with `fail` unless AX is `value`.
fn expect_ax(code: &mut Code, value: u16, fail: u8) {
    code.emit(&[0x3D, lo(value), hi(value)]);
    or_fail(code, fail);
}

/// les bx,[at].
fn les_bx(code: &mut Code, at: u16) {
    code.emit(&[0xC4, 0x1E, lo(at), hi(at)]);
}

/// End with `fail` unless the word at ES:[BX+`disp`] is `value`.
fn expect_es_bx(code: &mut Code, disp: u8, value: u16, fail: u8) {
    code.emit(&[0x26, 0x81, 0x7F, disp, lo(value), hi(value)]);
    or_fail(code, fail);
}

/// End with `fail` unless the byte at ES:[BX+`disp`] is `value`.
fn expect_es_bx_byte(code: &mut Code, disp: u8, value: u8, fail: u8) {
    code.emit(&[0x26, 0x80, 0x7F, disp, value]);
    or_fail(code, fail);
}

/// Call the method `method` of the object whose far pointer is at
/// `object`, with `push_args` pushing its arguments (`words` of them)
/// before the object's pointer, as C calls it, and take them off again.
fn call_method(code: &mut Code, object: u16, method: u8, push_args: impl Fn(&mut Code), words: u8) {
    push_args(code);
    les_bx(code, object);
    code.emit(&[0x06, 0x53]) // push es; push bx
        .emit(&[0x26, 0xC4, 0x1F]) // les bx,es:[bx]
        .emit(&[0x26, 0xFF, 0x5F, 4 * method]) // call far es:[bx+4*method]
        .emit(&[0x83, 0xC4, 2 * (words + 2)]); // add sp,...
}

/// Copy `bytes` to the data segment's `at`, padded to words.
fn put_bytes(code: &mut Code, at: u16, bytes: &[u8]) {
    let mut bytes = bytes.to_vec();
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

/// Copy `text` and a NUL to the data segment's `at`.
fn put_text(code: &mut Code, at: u16, text: &[u8]) {
    let mut bytes = text.to_vec();
    bytes.push(0);
    put_bytes(code, at, &bytes);
}

/// The GUID {11111111-2222-3333-4444-0000000000NN} of the fixture's
/// types.
fn fixture_guid(n: u8) -> [u8; 16] {
    [
        0x11, 0x11, 0x11, 0x11, 0x22, 0x22, 0x33, 0x33, 0x44, 0x44, 0, 0, 0, 0, 0, n,
    ]
}

/// LoadTypeLib("T.EXE\2", &LIB): S_OK, or end with `fail`.
fn load_own_library(code: &mut Code, fail: u8) {
    put_text(code, TEXT, b"T.EXE\\2");
    push_ds(code, TEXT);
    push_ds(code, LIB);
    code.call("TYPELIB", LOAD_TYPE_LIB);
    expect_dx_ax(code, 0, fail);
}

fn run_program(test: &str, code: &Code) -> (u8, String) {
    let library = library16();
    let resources = [(
        ne::ResId::Name("TYPELIB".into()),
        ne::ResId::Int(2),
        library.as_slice(),
    )];
    run(test, program_with_resources(code, CODE, &resources))
}

/// A program loads the library of its TYPELIB resource and reads it as
/// Windows reads the same library: its attributes, its seven types, the
/// interface IShape's attributes, its function Move and its name, the
/// constant Blue of the enum Colour, and the interface the class Shape
/// implements, a type of the same library.
#[test]
fn a_program_reads_its_type_library() {
    let mut code = Code::new();
    load_own_library(&mut code, 1);

    // GetTypeInfoCount: 7.
    call_method(&mut code, LIB, GET_TYPE_INFO_COUNT, |_| {}, 0);
    expect_ax(&mut code, 7, 2);

    // GetLibAttr: the GUID, SYS_WIN16 and version 1.2.
    call_method(&mut code, LIB, GET_LIB_ATTR, |c| push_ds(c, ATTR), 2);
    expect_dx_ax(&mut code, 0, 3);
    les_bx(&mut code, ATTR);
    expect_es_bx(&mut code, 0, 0x1111, 4);
    expect_es_bx(&mut code, 20, 0, 5);
    expect_es_bx(&mut code, 22, 1, 6);
    expect_es_bx(&mut code, 24, 2, 7);
    call_method(
        &mut code,
        LIB,
        RELEASE_TLIB_ATTR,
        |c| push_dword(c, ATTR),
        2,
    );

    // GetTypeInfoOfGuid(IShape): TKIND_INTERFACE with 4 functions.
    put_bytes(&mut code, GUID, &fixture_guid(1));
    call_method(
        &mut code,
        LIB,
        GET_TYPE_INFO_OF_GUID,
        |c| {
            push_ds(c, INFO);
            push_ds(c, GUID);
        },
        4,
    );
    expect_dx_ax(&mut code, 0, 8);
    call_method(&mut code, INFO, GET_TYPE_ATTR, |c| push_ds(c, ATTR), 2);
    expect_dx_ax(&mut code, 0, 9);
    les_bx(&mut code, ATTR);
    expect_es_bx(&mut code, 0, 0x1111, 10);
    expect_es_bx(&mut code, 40, 3, 11);
    expect_es_bx(&mut code, 42, 4, 12);

    // GetFuncDesc(2): Move, member 2, with two parameters of VT_I4.
    call_method(
        &mut code,
        INFO,
        GET_FUNC_DESC,
        |c| {
            push_ds(c, ATTR);
            c.push(2);
        },
        3,
    );
    expect_dx_ax(&mut code, 0, 13);
    les_bx(&mut code, ATTR);
    expect_es_bx(&mut code, 0, 2, 14);
    expect_es_bx(&mut code, 18, 2, 15);
    code.emit(&[0x26, 0xC4, 0x5F, 8]); // les bx,es:[bx+8]: the parameters
    expect_es_bx(&mut code, 4, 3, 16);
    expect_es_bx(&mut code, 16, 3, 17);

    // GetDocumentation(2, &name, NULL, NULL, NULL): "Move".
    call_method(
        &mut code,
        INFO,
        GET_DOCUMENTATION,
        |c| {
            c.push(0).push(0).push(0).push(0).push(0).push(0);
            push_ds(c, BSTR);
            c.push(0).push(2);
        },
        10,
    );
    expect_dx_ax(&mut code, 0, 18);
    les_bx(&mut code, BSTR);
    expect_es_bx_byte(&mut code, 0, b'M', 19);
    expect_es_bx_byte(&mut code, 3, b'e', 20);
    expect_es_bx_byte(&mut code, 4, 0, 21);
    call_method(&mut code, INFO, INFO_RELEASE, |_| {}, 0);

    // The enum Colour's third constant: Blue, a VT_I4 of 5.
    put_bytes(&mut code, GUID, &fixture_guid(5));
    call_method(
        &mut code,
        LIB,
        GET_TYPE_INFO_OF_GUID,
        |c| {
            push_ds(c, INFO);
            push_ds(c, GUID);
        },
        4,
    );
    expect_dx_ax(&mut code, 0, 22);
    call_method(
        &mut code,
        INFO,
        GET_VAR_DESC,
        |c| {
            push_ds(c, ATTR);
            c.push(2);
        },
        3,
    );
    expect_dx_ax(&mut code, 0, 23);
    les_bx(&mut code, ATTR);
    expect_es_bx(&mut code, 26, 2, 24); // VAR_CONST
    code.emit(&[0x26, 0xC4, 0x5F, 8]); // les bx,es:[bx+8]: the VARIANT
    expect_es_bx(&mut code, 0, 3, 25);
    expect_es_bx(&mut code, 8, 5, 26);
    call_method(&mut code, INFO, INFO_RELEASE, |_| {}, 0);

    // The class Shape implements IShape, by the HREFTYPE 0.
    put_bytes(&mut code, GUID, &fixture_guid(4));
    call_method(
        &mut code,
        LIB,
        GET_TYPE_INFO_OF_GUID,
        |c| {
            push_ds(c, INFO);
            push_ds(c, GUID);
        },
        4,
    );
    expect_dx_ax(&mut code, 0, 27);
    set(&mut code, OUT, 0xFFFF);
    call_method(
        &mut code,
        INFO,
        GET_REF_TYPE_OF_IMPL_TYPE,
        |c| {
            push_ds(c, OUT);
            c.push(0);
        },
        3,
    );
    expect_dx_ax(&mut code, 0, 28);
    call_method(
        &mut code,
        INFO,
        GET_REF_TYPE_INFO,
        |c| {
            push_ds(c, INFO2);
            push_dword(c, OUT);
        },
        4,
    );
    expect_dx_ax(&mut code, 0, 29);
    call_method(&mut code, INFO2, GET_TYPE_ATTR, |c| push_ds(c, ATTR), 2);
    expect_dx_ax(&mut code, 0, 30);
    les_bx(&mut code, ATTR);
    expect_es_bx(&mut code, 40, 3, 31);
    expect_es_bx_byte(&mut code, 15, 1, 32);
    call_method(&mut code, INFO2, INFO_RELEASE, |_| {}, 0);
    call_method(&mut code, INFO, INFO_RELEASE, |_| {}, 0);

    // The last reference to the library.
    call_method(&mut code, LIB, LIB_RELEASE, |_| {}, 0);
    expect_dx_ax(&mut code, 0, 33);
    code.emit(&[0xB0, 0x00]).exit_al();

    let (status, log) = run_program("typelib16_read", &code);
    assert_eq!(status, 0, "check {status} failed:\n{log}");
}

/// OaBuildVersion is Windows 98 SE's TYPELIB.DLL's, 10.3024, which
/// Visual Basic 4's runtime checks; a library RegisterTypeLib registers
/// is the 16-bit one QueryPathOfRegTypeLib and LoadRegTypeLib find; and
/// STDOLE.TLB, which Visual Basic's libraries import, is there with no
/// file of it.
#[test]
fn a_program_registers_its_type_library() {
    let mut code = Code::new();
    code.call("TYPELIB", OA_BUILD_VERSION);
    expect_dx_ax(&mut code, 0x000A_0BD0, 1);
    load_own_library(&mut code, 2);

    // RegisterTypeLib(lib, "T.EXE\2", NULL).
    push_dword(&mut code, LIB);
    push_ds(&mut code, TEXT);
    code.push(0).push(0);
    code.call("TYPELIB", REGISTER_TYPE_LIB);
    expect_dx_ax(&mut code, 0, 3);
    call_method(&mut code, LIB, LIB_RELEASE, |_| {}, 0);

    // QueryPathOfRegTypeLib(LIBID, 1, 2, 0, &BSTR): "T.EXE\2".
    put_bytes(
        &mut code,
        GUID,
        &[
            0x11, 0x11, 0x11, 0x11, 0x22, 0x22, 0x33, 0x33, 0x44, 0x44, 0x55, 0x55, 0x55, 0x55,
            0x55, 0x55,
        ],
    );
    push_ds(&mut code, GUID);
    code.push(1).push(2).push(0).push(0);
    push_ds(&mut code, BSTR);
    code.call("TYPELIB", QUERY_PATH_OF_REG_TYPE_LIB);
    expect_dx_ax(&mut code, 0, 4);
    les_bx(&mut code, BSTR);
    expect_es_bx_byte(&mut code, 0, b'T', 5);
    expect_es_bx_byte(&mut code, 6, b'2', 6);

    // LoadRegTypeLib(LIBID, 1, 2, 0, &LIB): the same library.
    push_ds(&mut code, GUID);
    code.push(1).push(2).push(0).push(0);
    push_ds(&mut code, LIB);
    code.call("TYPELIB", LOAD_REG_TYPE_LIB);
    expect_dx_ax(&mut code, 0, 7);
    call_method(&mut code, LIB, GET_TYPE_INFO_COUNT, |_| {}, 0);
    expect_ax(&mut code, 7, 8);
    call_method(&mut code, LIB, LIB_RELEASE, |_| {}, 0);

    // LoadTypeLib("STDOLE.TLB"): STDOLE 1.0, a 16-bit library.
    put_text(&mut code, TEXT, b"STDOLE.TLB");
    push_ds(&mut code, TEXT);
    push_ds(&mut code, LIB);
    code.call("TYPELIB", LOAD_TYPE_LIB);
    expect_dx_ax(&mut code, 0, 9);
    call_method(&mut code, LIB, GET_LIB_ATTR, |c| push_ds(c, ATTR), 2);
    expect_dx_ax(&mut code, 0, 10);
    les_bx(&mut code, ATTR);
    expect_es_bx(&mut code, 0, 0x0430, 11);
    expect_es_bx(&mut code, 2, 0x0002, 12);
    expect_es_bx(&mut code, 20, 0, 13);
    expect_es_bx(&mut code, 22, 1, 14);
    code.emit(&[0xB0, 0x00]).exit_al();

    let (status, log) = run_program("typelib16_register", &code);
    assert_eq!(status, 0, "check {status} failed:\n{log}");
}

// ITypeInfo::Invoke.

const INVOKE: u8 = 11;

/// The program's objects, each a far pointer to its vtable, which holds
/// the far addresses of the methods of IShape (Width's get and put, Move
/// and Name, at 0, 4, 8 and 12): one with C's methods, one whose Move is
/// Pascal's.
const OBJ: u16 = 0x50;
const OBJ_PASCAL: u16 = 0x54;
const VTABLE: u16 = 0x58;
const VTABLE_PASCAL: u16 = 0x68;
/// Invoke's DISPPARAMS, the named arguments' DISPIDs, up to two
/// arguments, the result, the EXCEPINFO and the argument in error.
const DP: u16 = 0x78;
const NAMED: u16 = 0x84;
const ARGS: u16 = 0x88;
const RESULT: u16 = 0xA8;
const EXCEPINFO: u16 = 0xB8;
const ARG_ERR: u16 = 0xD8;
/// What the methods were given: Move's two longs and the offset of its
/// object, and the long Width's put was given.
const MOVE_DX: u16 = 0xDC;
const MOVE_DY: u16 = 0xE0;
const MOVE_THIS: u16 = 0xE4;
const WIDTH: u16 = 0xE8;

const DISPATCH_METHOD: u16 = 1;
const DISPATCH_PROPERTYGET: u16 = 2;
const DISPATCH_PROPERTYPUT: u16 = 4;

/// The fixture as a 16-bit library whose IShape's methods have the
/// calling convention `cc` (CC_CDECL 1, CC_PASCAL 2) instead of the
/// 32-bit CC_STDCALL: the low 3 bits of the byte at 10h of each of its
/// four functions' records (magic 4Ch, the DISPID at 6), as
/// win32/oleaut32/sltg.rs reads them.
fn library16_with(cc: u8) -> Vec<u8> {
    let mut data = library16();
    let mut patched = 0;
    for p in 0..data.len() - 0x18 {
        let memid = u32::from_le_bytes(data[p + 6..p + 10].try_into().unwrap());
        if data[p] & !0x20 == 0x4C && (1..=3).contains(&memid) && data[p + 0x10] & 7 == 4 {
            data[p + 0x10] = data[p + 0x10] & !7 | cc;
            patched += 1;
        }
    }
    assert_eq!(patched, 4, "IShape's four functions in the fixture");
    data
}

/// mov word [at],ds.
fn set_ds(code: &mut Code, at: u16) {
    code.emit(&[0x8C, 0x1E, lo(at), hi(at)]);
}

/// mov word [at],offset; mov word [at+2],cs: the far address of the
/// code at `offset`.
fn set_code(code: &mut Code, at: u16, offset: u16) {
    set(code, at, offset);
    code.emit(&[0x8C, 0x0E, lo(at + 2), hi(at + 2)]);
}

/// mov ax,[bp+disp]; mov [at],ax.
fn store_arg(code: &mut Code, disp: u8, at: u16) {
    code.emit(&[0x8B, 0x46, disp, 0xA3, lo(at), hi(at)]);
}

/// xor ax,ax; xor dx,dx; pop bp; retf: S_OK.
fn return_s_ok(code: &mut Code) {
    code.emit(&[0x33, 0xC0, 0x33, 0xD2, 0x5D, 0xCB]);
}

/// The methods, far procedures with the object's pointer at [bp+6]:
/// their offsets (Width's get, put, Move, Name, Pascal's Move).
fn methods(code: &mut Code) -> [u16; 5] {
    let get = code.here();
    // *pv = 1234
    code.emit(&[0x55, 0x8B, 0xEC, 0xC4, 0x5E, 0x0A])
        .emit(&[0x26, 0xC7, 0x07, 0xD2, 0x04])
        .emit(&[0x26, 0xC7, 0x47, 0x02, 0x00, 0x00]);
    return_s_ok(code);
    let put = code.here();
    code.emit(&[0x55, 0x8B, 0xEC]);
    store_arg(code, 0x0A, WIDTH);
    store_arg(code, 0x0C, WIDTH + 2);
    return_s_ok(code);
    // C's Move(this, dx, dy): dx first after the object.
    let mv = code.here();
    code.emit(&[0x55, 0x8B, 0xEC]);
    store_arg(code, 0x06, MOVE_THIS);
    store_arg(code, 0x0A, MOVE_DX);
    store_arg(code, 0x0C, MOVE_DX + 2);
    store_arg(code, 0x0E, MOVE_DY);
    store_arg(code, 0x10, MOVE_DY + 2);
    return_s_ok(code);
    // Name fails with E_FAIL.
    let name = code.here();
    code.emit(&[0xB8, 0x05, 0x40, 0xBA, 0x00, 0x80, 0xCB]);
    // Pascal's Move: dx pushed first, dy after it, the object last.
    let pascal = code.here();
    code.emit(&[0x55, 0x8B, 0xEC]);
    store_arg(code, 0x06, MOVE_THIS);
    store_arg(code, 0x0E, MOVE_DX);
    store_arg(code, 0x10, MOVE_DX + 2);
    store_arg(code, 0x0A, MOVE_DY);
    store_arg(code, 0x0C, MOVE_DY + 2);
    return_s_ok(code);
    [get, put, mv, name, pascal]
}

/// DISPPARAMS with `count` arguments from ARGS, `named` of them named by
/// the DISPIDs at NAMED.
fn disp_params(code: &mut Code, count: u16, named: u16) {
    set(code, DP, ARGS);
    set_ds(code, DP + 2);
    set(code, DP + 4, NAMED);
    set_ds(code, DP + 6);
    set(code, DP + 8, count);
    set(code, DP + 10, named);
}

/// The VARIANT `n` of the arguments: the VARTYPE and the value's words.
fn arg(code: &mut Code, n: u16, vt: u16, value: [u16; 4]) {
    let at = ARGS + 16 * n;
    set(code, at, vt);
    for (i, w) in value.iter().enumerate() {
        set(code, at + 8 + 2 * i as u16, *w);
    }
}

/// Invoke(obj, memid, flags, &DP, &RESULT, &EXCEPINFO, &ARG_ERR) of the
/// type object at `info`, the result's VARTYPE FFFFh first.
fn invoke(code: &mut Code, info: u16, obj: u16, memid: u16, flags: u16) {
    set(code, RESULT, 0xFFFF);
    call_method(
        code,
        info,
        INVOKE,
        |c| {
            push_ds(c, ARG_ERR);
            push_ds(c, EXCEPINFO);
            push_ds(c, RESULT);
            push_ds(c, DP);
            c.push(flags);
            c.push(0).push(memid);
            push_ds(c, obj);
        },
        13,
    );
}

/// End with `fail` unless the word at the data segment's `at` is
/// `value`.
fn expect_word(code: &mut Code, at: u16, value: u16, fail: u8) {
    code.emit(&[0x81, 0x3E, lo(at), hi(at), lo(value), hi(value)]);
    or_fail(code, fail);
}

/// IShape of the library "T.EXE\`n`", its type object at `info`.
fn load_shape(code: &mut Code, n: u8, info: u16, fail: u8) {
    put_text(code, TEXT, &[b'T', b'.', b'E', b'X', b'E', b'\\', b'0' + n]);
    push_ds(code, TEXT);
    push_ds(code, LIB);
    code.call("TYPELIB", LOAD_TYPE_LIB);
    expect_dx_ax(code, 0, fail);
    put_bytes(code, GUID, &fixture_guid(1));
    call_method(
        code,
        LIB,
        GET_TYPE_INFO_OF_GUID,
        |c| {
            push_ds(c, info);
            push_ds(c, GUID);
        },
        4,
    );
    expect_dx_ax(code, 0, fail + 1);
    call_method(code, LIB, LIB_RELEASE, |_| {}, 0);
}

/// ITypeInfo::Invoke calls the object's methods through its vtable, as
/// Windows' does: the arguments changed to the parameters' types (an
/// I2 and an R8 to longs), a property's get with its [retval] in the
/// result and its put with the DISPID_PROPERTYPUT argument, a failed
/// HRESULT as DISP_E_EXCEPTION with the EXCEPINFO's scode, the errors
/// for a member there isn't, too many arguments and one of a type that
/// can't be changed (with its place), and a Pascal method with its
/// arguments pushed first first and the object's pointer last.
#[test]
fn invoke_calls_the_objects_methods() {
    let mut code = Code::new();
    code.emit(&[0xE9, 0, 0]); // jmp over the methods
    let [get, put, mv, name, pascal] = methods(&mut code);
    let main = code.here();
    code.patch(1, main - 3);

    for (i, m) in [get, put, mv, name].iter().enumerate() {
        set_code(&mut code, VTABLE + 4 * i as u16, *m);
        let m = if i == 2 { pascal } else { *m };
        set_code(&mut code, VTABLE_PASCAL + 4 * i as u16, m);
    }
    set(&mut code, OBJ, VTABLE);
    set_ds(&mut code, OBJ + 2);
    set(&mut code, OBJ_PASCAL, VTABLE_PASCAL);
    set_ds(&mut code, OBJ_PASCAL + 2);
    load_shape(&mut code, 2, INFO, 1);
    load_shape(&mut code, 3, INFO2, 3);

    // Move(5, 7.0): dx a VT_I2, dy (the first argument, the last
    // parameter) a VT_R8; the result VT_EMPTY.
    disp_params(&mut code, 2, 0);
    arg(&mut code, 0, 5, [0, 0, 0, 0x401C]);
    arg(&mut code, 1, 2, [5, 0, 0, 0]);
    invoke(&mut code, INFO, OBJ, 2, DISPATCH_METHOD);
    expect_dx_ax(&mut code, 0, 10);
    expect_word(&mut code, MOVE_DX, 5, 11);
    expect_word(&mut code, MOVE_DX + 2, 0, 12);
    expect_word(&mut code, MOVE_DY, 7, 13);
    expect_word(&mut code, MOVE_THIS, OBJ, 14);
    expect_word(&mut code, RESULT, 0, 15);

    // Width's get: a VT_I4 of 1234.
    disp_params(&mut code, 0, 0);
    invoke(&mut code, INFO, OBJ, 1, DISPATCH_PROPERTYGET);
    expect_dx_ax(&mut code, 0, 20);
    expect_word(&mut code, RESULT, 3, 21);
    expect_word(&mut code, RESULT + 8, 1234, 22);

    // Width's put of 42, the argument named DISPID_PROPERTYPUT.
    disp_params(&mut code, 1, 1);
    set(&mut code, NAMED, 0xFFFD);
    set(&mut code, NAMED + 2, 0xFFFF);
    arg(&mut code, 0, 2, [42, 0, 0, 0]);
    invoke(&mut code, INFO, OBJ, 1, DISPATCH_PROPERTYPUT);
    expect_dx_ax(&mut code, 0, 30);
    expect_word(&mut code, WIDTH, 42, 31);

    // Name returns E_FAIL: DISP_E_EXCEPTION, the scode E_FAIL.
    disp_params(&mut code, 0, 0);
    invoke(&mut code, INFO, OBJ, 3, DISPATCH_METHOD);
    expect_dx_ax(&mut code, 0x8002_0009, 40);
    expect_word(&mut code, EXCEPINFO + 28, 0x4005, 41);
    expect_word(&mut code, EXCEPINFO + 30, 0x8000, 42);

    // No member 9: DISP_E_MEMBERNOTFOUND. Move with three arguments:
    // DISP_E_BADPARAMCOUNT.
    invoke(&mut code, INFO, OBJ, 9, DISPATCH_METHOD);
    expect_dx_ax(&mut code, 0x8002_0003, 50);
    disp_params(&mut code, 3, 0);
    invoke(&mut code, INFO, OBJ, 2, DISPATCH_METHOD);
    expect_dx_ax(&mut code, 0x8002_000E, 51);

    // Move(VT_NULL, 1): DISP_E_TYPEMISMATCH, the argument in error the
    // second, dx.
    disp_params(&mut code, 2, 0);
    arg(&mut code, 0, 2, [1, 0, 0, 0]);
    arg(&mut code, 1, 1, [0, 0, 0, 0]);
    set(&mut code, ARG_ERR, 0xFFFF);
    invoke(&mut code, INFO, OBJ, 2, DISPATCH_METHOD);
    expect_dx_ax(&mut code, 0x8002_0005, 60);
    expect_word(&mut code, ARG_ERR, 1, 61);

    // Pascal's Move(5, 7).
    set(&mut code, MOVE_DX, 0);
    set(&mut code, MOVE_DY, 0);
    disp_params(&mut code, 2, 0);
    arg(&mut code, 0, 3, [7, 0, 0, 0]);
    arg(&mut code, 1, 3, [5, 0, 0, 0]);
    invoke(&mut code, INFO2, OBJ_PASCAL, 2, DISPATCH_METHOD);
    expect_dx_ax(&mut code, 0, 70);
    expect_word(&mut code, MOVE_DX, 5, 71);
    expect_word(&mut code, MOVE_DY, 7, 72);
    expect_word(&mut code, MOVE_THIS, OBJ_PASCAL, 73);

    call_method(&mut code, INFO, INFO_RELEASE, |_| {}, 0);
    call_method(&mut code, INFO2, INFO_RELEASE, |_| {}, 0);
    code.emit(&[0xB0, 0x00]).exit_al();

    let cdecl = library16_with(1);
    let pascal = library16_with(2);
    let resources = [
        (
            ne::ResId::Name("TYPELIB".into()),
            ne::ResId::Int(2),
            cdecl.as_slice(),
        ),
        (
            ne::ResId::Name("TYPELIB".into()),
            ne::ResId::Int(3),
            pascal.as_slice(),
        ),
    ];
    let program = program_with_resources(&code, CODE, &resources);
    let (status, log) = run("typelib16_invoke", program);
    assert_eq!(status, 0, "check {status} failed:\n{log}");
}
