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
