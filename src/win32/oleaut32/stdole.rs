//! The built-in STDOLE2.TLB and STDOLE32.TLB: OLE Automation's own
//! types, which other libraries import. Dispinterfaces inherit
//! IDispatch's methods from them, and controls take their OLE_ types.
//! STDOLE32.TLB, version 1.0, has the first six; STDOLE2.TLB, 2.0, has
//! those and the OLE_ and FONT aliases, in the order Windows' file has
//! them, as libraries that import them by their places need. The font
//! and picture interfaces after those aren't here yet.

use super::model::*;
use crate::win32::com::Guid;

/// The library's GUID, the same for both versions.
pub const LIBID: Guid = Guid::new(0x0002_0430, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);

/// The files, as LoadTypeLib finds them in the system directory and the
/// registry names them.
pub const STDOLE2: &str = "STDOLE2.TLB";
pub const STDOLE32: &str = "STDOLE32.TLB";

/// TYPEFLAGS of these types.
const FHIDDEN: u16 = 0x10;
const FRESTRICTED: u16 = 0x200;
/// FUNCFLAGS: FRESTRICTED.
const FUNCFLAG_FRESTRICTED: u16 = 1;

/// A type's HREFTYPE: as in an MSFT file, its place times the size of a
/// type's entry there.
const fn href(index: usize) -> u32 {
    (index * 0x64) as u32
}
const GUID_HREF: u32 = href(0);
const DISPPARAMS_HREF: u32 = href(1);
const EXCEPINFO_HREF: u32 = href(2);
const IUNKNOWN_HREF: u32 = href(3);
const IENUMVARIANT_HREF: u32 = href(5);

/// The library of the version `major`: 1 or 2.
pub fn library(major: u16) -> TypeLib {
    let mut infos = vec![
        record(
            "GUID",
            FHIDDEN,
            16,
            &[
                ("Data1", TypeDesc::Base(VT_UI4), 0),
                ("Data2", TypeDesc::Base(VT_UI2), 4),
                ("Data3", TypeDesc::Base(VT_UI2), 6),
                (
                    "Data4",
                    TypeDesc::CArray(Box::new(TypeDesc::Base(VT_UI1)), vec![(8, 0)]),
                    8,
                ),
            ],
        ),
        record(
            "DISPPARAMS",
            FRESTRICTED,
            16,
            &[
                ("rgvarg", TypeDesc::ptr(TypeDesc::Base(VT_VARIANT)), 0),
                ("rgdispidNamedArgs", TypeDesc::ptr(TypeDesc::Base(VT_I4)), 4),
                ("cArgs", TypeDesc::Base(VT_UINT), 8),
                ("cNamedArgs", TypeDesc::Base(VT_UINT), 12),
            ],
        ),
        record(
            "EXCEPINFO",
            FRESTRICTED,
            32,
            &[
                ("wCode", TypeDesc::Base(VT_UI2), 0),
                ("wReserved", TypeDesc::Base(VT_UI2), 2),
                ("bstrSource", TypeDesc::Base(VT_BSTR), 4),
                ("bstrDescription", TypeDesc::Base(VT_BSTR), 8),
                ("bstrHelpFile", TypeDesc::Base(VT_BSTR), 12),
                ("dwHelpContext", TypeDesc::Base(VT_UI4), 16),
                ("pvReserved", TypeDesc::ptr(TypeDesc::Base(VT_VOID)), 20),
                (
                    "pfnDeferredFillIn",
                    TypeDesc::ptr(TypeDesc::Base(VT_VOID)),
                    24,
                ),
                ("scode", TypeDesc::Base(VT_ERROR), 28),
            ],
        ),
        interface(
            "IUnknown",
            Guid::new(0, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]),
            FHIDDEN,
            None,
            0x6000_0000,
            FUNCFLAG_FRESTRICTED,
            vec![
                method(
                    "QueryInterface",
                    VT_HRESULT,
                    vec![
                        param("riid", PARAMFLAG_FIN, ptr_user(GUID_HREF)),
                        param("ppvObj", PARAMFLAG_FOUT, ptr_ptr_void()),
                    ],
                ),
                method("AddRef", VT_UI4, vec![]),
                method("Release", VT_UI4, vec![]),
            ],
        ),
        interface(
            "IDispatch",
            Guid::new(0x0002_0400, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]),
            FRESTRICTED,
            Some(IUNKNOWN_HREF),
            0x6001_0000,
            FUNCFLAG_FRESTRICTED,
            vec![
                method(
                    "GetTypeInfoCount",
                    VT_HRESULT,
                    vec![param(
                        "pctinfo",
                        PARAMFLAG_FOUT,
                        TypeDesc::ptr(TypeDesc::Base(VT_UINT)),
                    )],
                ),
                method(
                    "GetTypeInfo",
                    VT_HRESULT,
                    vec![
                        param("itinfo", PARAMFLAG_FIN, TypeDesc::Base(VT_UINT)),
                        param("lcid", PARAMFLAG_FIN, TypeDesc::Base(VT_UI4)),
                        param("pptinfo", PARAMFLAG_FOUT, ptr_ptr_void()),
                    ],
                ),
                method(
                    "GetIDsOfNames",
                    VT_HRESULT,
                    vec![
                        param("riid", PARAMFLAG_FIN, ptr_user(GUID_HREF)),
                        param(
                            "rgszNames",
                            PARAMFLAG_FIN,
                            TypeDesc::ptr(TypeDesc::ptr(TypeDesc::Base(VT_I1))),
                        ),
                        param("cNames", PARAMFLAG_FIN, TypeDesc::Base(VT_UINT)),
                        param("lcid", PARAMFLAG_FIN, TypeDesc::Base(VT_UI4)),
                        param(
                            "rgdispid",
                            PARAMFLAG_FOUT,
                            TypeDesc::ptr(TypeDesc::Base(VT_I4)),
                        ),
                    ],
                ),
                method(
                    "Invoke",
                    VT_HRESULT,
                    vec![
                        param("dispidMember", PARAMFLAG_FIN, TypeDesc::Base(VT_I4)),
                        param("riid", PARAMFLAG_FIN, ptr_user(GUID_HREF)),
                        param("lcid", PARAMFLAG_FIN, TypeDesc::Base(VT_UI4)),
                        param("wFlags", PARAMFLAG_FIN, TypeDesc::Base(VT_UI2)),
                        param("pdispparams", PARAMFLAG_FIN, ptr_user(DISPPARAMS_HREF)),
                        param(
                            "pvarResult",
                            PARAMFLAG_FOUT,
                            TypeDesc::ptr(TypeDesc::Base(VT_VARIANT)),
                        ),
                        param("pexcepinfo", PARAMFLAG_FOUT, ptr_user(EXCEPINFO_HREF)),
                        param(
                            "puArgErr",
                            PARAMFLAG_FOUT,
                            TypeDesc::ptr(TypeDesc::Base(VT_UINT)),
                        ),
                    ],
                ),
            ],
        ),
        interface(
            "IEnumVARIANT",
            Guid::new(0x0002_0404, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]),
            FHIDDEN,
            Some(IUNKNOWN_HREF),
            0x6001_0000,
            0,
            vec![
                method(
                    "Next",
                    VT_HRESULT,
                    vec![
                        param("celt", PARAMFLAG_FIN, TypeDesc::Base(VT_UI4)),
                        param(
                            "rgvar",
                            PARAMFLAG_FIN,
                            TypeDesc::ptr(TypeDesc::Base(VT_VARIANT)),
                        ),
                        param(
                            "pceltFetched",
                            PARAMFLAG_FOUT,
                            TypeDesc::ptr(TypeDesc::Base(VT_UI4)),
                        ),
                    ],
                ),
                method(
                    "Skip",
                    VT_HRESULT,
                    vec![param("celt", PARAMFLAG_FIN, TypeDesc::Base(VT_UI4))],
                ),
                method("Reset", VT_HRESULT, vec![]),
                method(
                    "Clone",
                    VT_HRESULT,
                    vec![param(
                        "ppenum",
                        PARAMFLAG_FOUT,
                        TypeDesc::ptr(ptr_user(IENUMVARIANT_HREF)),
                    )],
                ),
            ],
        ),
    ];
    if major >= 2 {
        // OLE_ and FONT types: their GUIDs' first word, their flags and
        // what they are.
        let ole = |n: u32| {
            Guid::new(
                n,
                0xBE0F,
                0x101A,
                [0x8B, 0xBB, 0, 0xAA, 0, 0x30, 0x0C, 0xAB],
            )
        };
        let container = |n: u32| {
            Guid::new(
                n,
                0x9069,
                0x101B,
                [0xAE, 0x2D, 0x08, 0, 0x2B, 0x2E, 0xC7, 0x13],
            )
        };
        let aliases: &[(&str, Guid, u16, u16)] = &[
            ("OLE_COLOR", ole(0x6650_4301), 0, VT_UI4),
            ("OLE_XPOS_PIXELS", ole(0x6650_4302), FHIDDEN, VT_I4),
            ("OLE_YPOS_PIXELS", ole(0x6650_4303), FHIDDEN, VT_I4),
            ("OLE_XSIZE_PIXELS", ole(0x6650_4304), FHIDDEN, VT_I4),
            ("OLE_YSIZE_PIXELS", ole(0x6650_4305), FHIDDEN, VT_I4),
            ("OLE_XPOS_HIMETRIC", ole(0x6650_4306), FHIDDEN, VT_I4),
            ("OLE_YPOS_HIMETRIC", ole(0x6650_4307), FHIDDEN, VT_I4),
            ("OLE_XSIZE_HIMETRIC", ole(0x6650_4308), FHIDDEN, VT_I4),
            ("OLE_YSIZE_HIMETRIC", ole(0x6650_4309), FHIDDEN, VT_I4),
            ("OLE_XPOS_CONTAINER", container(0xBF03_0640), FHIDDEN, 4),
            ("OLE_YPOS_CONTAINER", container(0xBF03_0641), FHIDDEN, 4),
            ("OLE_XSIZE_CONTAINER", container(0xBF03_0642), FHIDDEN, 4),
            ("OLE_YSIZE_CONTAINER", container(0xBF03_0643), FHIDDEN, 4),
            ("OLE_HANDLE", ole(0x6650_4313), 0, VT_INT),
            ("OLE_OPTEXCLUSIVE", ole(0x6650_430B), 0, VT_BOOL),
            ("OLE_CANCELBOOL", container(0xBF03_0644), 0, VT_BOOL),
            (
                "OLE_ENABLEDEFAULTBOOL",
                container(0xBF03_0645),
                FHIDDEN,
                VT_BOOL,
            ),
        ];
        for &(name, guid, flags, vt) in aliases {
            infos.push(alias(name, guid, flags, vt));
        }
        let mut tristate = record("OLE_TRISTATE", 0, 4, &[]);
        tristate.kind = TKIND_ENUM;
        tristate.guid = ole(0x6650_430A);
        for (i, name) in ["Unchecked", "Checked", "Gray"].into_iter().enumerate() {
            tristate.vars.push(Var {
                memid: 0x4000_0000 + i as i32,
                name: Some(name.into()),
                doc: None,
                help_context: 0,
                ty: TypeDesc::Base(VT_INT),
                flags: 0,
                kind: VAR_CONST,
                value: VarValue::Const(Value::int(VT_I4, i as i32)),
            });
        }
        infos.push(tristate);
        let fonts: &[(&str, u32, u16)] = &[
            ("FONTNAME", 0x6650_430D, VT_BSTR),
            ("FONTSIZE", 0x6650_430E, 6),
            ("FONTBOLD", 0x6650_430F, VT_BOOL),
            ("FONTITALIC", 0x6650_4310, VT_BOOL),
            ("FONTUNDERSCORE", 0x6650_4311, VT_BOOL),
            ("FONTSTRIKETHROUGH", 0x6650_4312, VT_BOOL),
        ];
        for &(name, n, vt) in fonts {
            infos.push(alias(name, ole(n), FHIDDEN, vt));
        }
    }
    for (i, info) in infos.iter_mut().enumerate() {
        info.href = href(i);
    }
    TypeLib {
        format: Format::Builtin,
        guid: LIBID,
        lcid: 0,
        syskind: 1,
        major,
        minor: 0,
        flags: LIBFLAG_FHASDISKIMAGE,
        name: Some("stdole".into()),
        doc: Some("OLE Automation".into()),
        help_file: None,
        help_context: 0,
        infos,
        imports: Vec::new(),
        refs: Vec::new(),
        dispatch_href: None,
    }
}

fn ptr_user(href: u32) -> TypeDesc {
    TypeDesc::ptr(TypeDesc::User(href))
}

fn ptr_ptr_void() -> TypeDesc {
    TypeDesc::ptr(TypeDesc::ptr(TypeDesc::Base(VT_VOID)))
}

fn param(name: &str, flags: u16, ty: TypeDesc) -> Param {
    Param {
        name: Some(name.into()),
        ty,
        flags,
        default: None,
    }
}

fn method(name: &str, ret: u16, params: Vec<Param>) -> Func {
    Func {
        memid: 0,
        name: Some(name.into()),
        doc: None,
        help_context: 0,
        kind: FUNC_PUREVIRTUAL,
        invkind: INVOKE_FUNC,
        callconv: CC_STDCALL,
        opt: 0,
        vft: 0,
        flags: 0,
        ret: TypeDesc::Base(ret),
        params,
        entry: None,
    }
}

fn blank(name: &str, kind: u32, flags: u16, size: u32) -> TypeInfo {
    TypeInfo {
        kind,
        guid: Guid::default(),
        flags,
        major: 0,
        minor: 0,
        lcid: 0,
        size,
        align: size.clamp(1, 4) as u16,
        vft: 0,
        name: Some(name.into()),
        doc: None,
        help_context: 0,
        dll: None,
        alias: None,
        impls: Vec::new(),
        funcs: Vec::new(),
        vars: Vec::new(),
        href: 0,
        refs: Vec::new(),
    }
}

fn record(name: &str, flags: u16, size: u32, fields: &[(&str, TypeDesc, u32)]) -> TypeInfo {
    let mut info = blank(name, TKIND_RECORD, flags, size);
    for (i, (field, ty, offset)) in fields.iter().enumerate() {
        info.vars.push(Var {
            memid: 0x4000_0000 + i as i32,
            name: Some((*field).into()),
            doc: None,
            help_context: 0,
            ty: ty.clone(),
            flags: 0,
            kind: VAR_PERINSTANCE,
            value: VarValue::Offset(*offset),
        });
    }
    info
}

/// An interface whose methods start after `base`'s, numbered from
/// `memid`.
fn interface(
    name: &str,
    guid: Guid,
    flags: u16,
    base: Option<u32>,
    memid: i32,
    func_flags: u16,
    mut funcs: Vec<Func>,
) -> TypeInfo {
    let mut info = blank(name, TKIND_INTERFACE, flags, 4);
    info.guid = guid;
    // IUnknown's three methods come before those of an interface on it.
    let first = if base.is_some() { 3 } else { 0 };
    for (i, f) in funcs.iter_mut().enumerate() {
        f.memid = memid + i as i32;
        f.vft = 4 * (first + i) as i16;
        f.flags = func_flags;
    }
    info.vft = 4 * (first + funcs.len()) as u16;
    info.funcs = funcs;
    if let Some(href) = base {
        info.impls.push(Impl { href, flags: 0 });
    }
    info
}

fn alias(name: &str, guid: Guid, flags: u16, vt: u16) -> TypeInfo {
    let size = match vt {
        VT_BOOL => 2,
        6 => 8,
        _ => 4,
    };
    let mut info = blank(name, TKIND_ALIAS, flags, size);
    info.align = size as u16;
    info.guid = guid;
    info.alias = Some(TypeDesc::Base(vt));
    info
}
